use super::*;
use crate::engine::Engine;
use std::path::Path;

/// `BP_MODULE=file BP_TICKS=n BP_LOG=out [BP_SUBSONG=k] cargo test --release dump -- --ignored`:
/// the register log of `n` ticks of a module (`k` subsongs stepped on first, as the host does).
#[test]
#[ignore]
fn dump() {
    let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("BP_MODULE"), std::env::var("BP_TICKS"), std::env::var("BP_LOG")) else {
        return;
    };
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    // BP_PHASE=c: the first tick call on raster phase c (the reference run's)
    if let Some(c) = std::env::var("BP_PHASE").ok().and_then(|s| s.parse::<u64>().ok()) {
        PHASE.store(c, std::sync::atomic::Ordering::Relaxed);
    }
    e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
    let sub: u32 = std::env::var("BP_SUBSONG").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    for _ in 0..sub {
        e.change_subsong(1).unwrap();
    }
    println!("format {}", e.playing.as_ref().unwrap().format);
    let mut buf = vec![0f32; 2 * 960];
    let n: usize = ticks.parse().unwrap();
    for _ in 0..n {
        e.render(&mut buf);
    }
    let l = e.m.bus.write_log.take().unwrap();
    let text: String = l.iter().map(|(a, v, t)| format!("{a:06x} {v:04x} {t}\n")).collect();
    std::fs::write(out, text).unwrap();
}

// ---------------------------------------------------------------------------------
// Synthetic modules (built here from the specification's layout; no game data)

/// A sample header: start offset, end offset, loop length, volume, finetune.
type Hdr = (u32, u32, u32, u8, u8);

/// A sample file with 31 headers (`spls`: with the four letters in front), then filler data up to
/// the largest end offset.
fn smp_file(hdrs: &[Hdr], spls: bool) -> Vec<u8> {
    let mut t = vec![0u8; 496];
    for (i, h) in hdrs.iter().enumerate() {
        let o = 16 * i;
        t[o..o + 4].copy_from_slice(&h.0.to_be_bytes());
        t[o + 4..o + 8].copy_from_slice(&h.1.to_be_bytes());
        t[o + 8..o + 12].copy_from_slice(&h.2.to_be_bytes());
        t[o + 12] = h.3;
        t[o + 13] = h.4;
    }
    let end = hdrs.iter().map(|h| h.1).max().unwrap_or(500);
    for i in hdrs.len()..31 {
        let o = 16 * i;
        t[o..o + 4].copy_from_slice(&end.to_be_bytes());
        t[o + 4..o + 8].copy_from_slice(&end.to_be_bytes());
    }
    t[..4].copy_from_slice(&hdrs.first().map_or(500, |h| h.0).to_be_bytes());
    let mut d = Vec::new();
    if spls {
        d.extend_from_slice(b"SPLS");
    }
    d.extend_from_slice(&t);
    let fill = (end as usize).saturating_sub(if spls { 500 } else { 496 });
    d.extend((0..fill).map(|i| (i as u8).wrapping_mul(7)));
    d
}

/// A song file: header bytes, positions (V track numbers each), tracks (64 cell indices) and cells.
fn song(e: u8, v: u8, restart: u8, positions: &[Vec<u8>], tracks: &[Vec<u16>], cells: &[[u8; 4]]) -> Vec<u8> {
    let mut d = vec![0x53, 0x8F, 0x4E, 0x47, 0x2E, e, v, restart];
    for p in positions {
        d.extend_from_slice(p);
    }
    d.extend_from_slice(&[0xDD, 0x48]);
    for t in tracks {
        assert_eq!(t.len(), 64);
        for &c in t {
            if e == b'B' {
                d.push(c as u8);
            } else {
                d.extend_from_slice(&c.to_be_bytes());
            }
        }
    }
    d.extend_from_slice(&[0xD8, 0xF1]);
    for c in cells {
        d.extend_from_slice(c);
    }
    d
}

fn track(rows: &[(usize, u16)]) -> Vec<u16> {
    let mut t = vec![0u16; 64];
    for &(r, c) in rows {
        t[r] = c;
    }
    t
}

fn run_with(uds: Vec<u8>, smp: Option<Vec<u8>>, ticks: usize) -> (Vec<(u32, u16)>, Option<String>) {
    let dir = Path::new("/bp-test");
    let mut e = Engine::new(48000.0);
    if let Some(s) = smp {
        e.m.os.mem_files.insert(dir.join("smp.test"), s);
    }
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if e.play_data(&dir.join("uds.test"), uds).is_err() {
        return (Vec::new(), None);
    }
    let fmt = e.playing.as_ref().map(|p| p.format.to_string());
    let mut out = vec![0f32; 2 * 960 * ticks];
    e.render(&mut out);
    (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), fmt)
}

fn std_hdrs() -> Vec<Hdr> {
    // sample 1: 40 bytes at 500, no loop, volume 0x30; sample 2: 64 bytes, loop of 16, volume 0x20, finetune 2
    vec![(500, 540, 0, 0x30, 0), (540, 604, 16, 0x20, 2)]
}

fn simple(rows: &[(usize, u16)], cells: &[[u8; 4]]) -> Vec<u8> {
    song(b'B', b'4', 0, &[vec![0, 1, 1, 1]], &[track(rows), track(&[])], cells)
}

fn regs(log: &[(u32, u16)], reg: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == reg).map(|w| w.1).collect()
}

// ---------------------------------------------------------------------------------
// Tables (spec 6.2 and 6.7)

#[test]
fn period_table_matches_the_specification() {
    let row = |r: i32| (0..36).map(|i| ptab(r * 36 + i)).collect::<Vec<_>>();
    assert_eq!(
        row(0),
        [
            856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202,
            190, 180, 170, 160, 151, 143, 135, 127, 120, 113
        ]
    );
    // the first of rows 1, 8 and 15 and the last of rows 1 and 15
    assert_eq!([ptab(36), ptab(8 * 36), ptab(15 * 36)], [850, 907, 862]);
    assert_eq!([ptab(71), ptab(575)], [113, 114]);
    // the word in front of the table, and reads past it
    assert_eq!(ptab(-1), 0x71AE);
    assert_eq!([ptab(576), ptab(-2), ptab(100_000)], [0, 0, 0]);
}

#[test]
fn waveforms_match_the_specification() {
    let sine: Vec<u32> = (0..64).map(|i| wave(0, i)).collect();
    assert_eq!(
        &sine[..32],
        [0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24]
    );
    assert_eq!(sine[32..], sine[..32]);
    let tri: Vec<u32> = (0..64).map(|i| wave(1, i)).collect();
    assert_eq!(&tri[..4], [0, 8, 16, 24]);
    assert_eq!(tri[31], 248);
    assert_eq!(&tri[32..35], [255, 248, 240]);
    assert_eq!(tri[63], 8);
    assert!((0..64).all(|i| wave(2, i) == 255));
}

// ---------------------------------------------------------------------------------
// Recognition and layout

#[test]
fn recognises_and_parses_the_song_file() {
    let cells = [[0u8; 4]; 3];
    let d = song(b'B', b'4', 0x7F, &[vec![0, 1, 1, 1], vec![1, 1, 0, 0]], &[track(&[(0, 2)]), track(&[])], &cells);
    assert!(detect(&d));
    let l = layout(&d).unwrap();
    assert_eq!((l.e, l.v, l.restart, l.positions, l.cells), (1, 4, 0, 2, 3));
    assert_eq!(l.pool, 8 + 8 + 2);
    assert_eq!(l.dict, l.pool + 128 + 2);
    // word entries, eight tracks per position
    let w = song(b'W', b'8', 1, &[vec![0, 1, 1, 1, 0, 0, 0, 0]], &[track(&[(3, 2)]), track(&[])], &cells);
    let l = layout(&w).unwrap();
    assert_eq!((l.e, l.v, l.restart, l.positions, l.cells), (2, 8, 1, 1, 3));
    // refused: bad magic, no terminator, cells beyond the file, too short
    let mut bad = d.clone();
    bad[0] = 0;
    assert!(!detect(&bad));
    assert!(!detect(&d[..d.len() - 5]));
    assert!(!detect(&d[..12]));
    assert!(!detect(&[]));
    let bad = song(b'B', b'4', 0, &[vec![0, 1, 1, 1]], &[track(&[(0, 200)]), track(&[])], &cells);
    assert!(!detect(&bad));
}

#[test]
fn sample_table_forms() {
    let s = smp_file(&std_hdrs(), true);
    assert_eq!(sample_table(&s), Some((4, 31)));
    let s = smp_file(&std_hdrs(), false);
    assert_eq!(sample_table(&s), Some((0, 31)));
    assert_eq!(sample_table(&[0u8; 40]), None);
    assert_eq!(sample_table(&[]), None);
}

#[test]
fn subsongs_come_from_the_restart_and_jump_commands() {
    // position 0 has no B or 8, position 1 has an 8 (-> start 2), position 2 a B (-> start 3)
    let cells = [[0, 0, 0, 0], [0, 0, 0x20, 0], [0, 0, 0x2C, 0]];
    let d = song(
        b'B',
        b'4',
        0,
        &[vec![0, 0, 0, 0], vec![1, 0, 0, 0], vec![2, 0, 0, 0]],
        &[track(&[]), track(&[(5, 1)]), track(&[(9, 2)])],
        &cells,
    );
    let l = layout(&d).unwrap();
    let (n, starts) = scan_subsongs(&d, &l);
    assert_eq!((n, starts), (2, vec![0, 2, 3]));
    let d = simple(&[], &[[0; 4]]);
    let (n, starts) = scan_subsongs(&d, &layout(&d).unwrap());
    assert_eq!((n, starts), (1, vec![0]));
}

// ---------------------------------------------------------------------------------
// Replay

#[test]
fn plays_a_note_with_the_specified_write_sequence() {
    // channel 0, row 0: note 24 (index 12 -> period 453), sample 1
    let d = simple(&[(0, 1)], &[[0; 4], [24, 1, 0, 0]]);
    let (a, fmt) = run_with(d.clone(), Some(smp_file(&std_hdrs(), false)), 40);
    assert_eq!(fmt.as_deref(), Some("Blade Packer"));
    let (b, _) = run_with(d.clone(), Some(smp_file(&std_hdrs(), false)), 40);
    assert_eq!(a, b);
    // the host's init: four volume zeros, the filter bit, DMA enable
    assert_eq!(&a[..6], [(0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0), (0xBFE001, 2), (0xDFF096, 0xF)]);
    // tick 0 starts all four voices on the silent word: DMA off 0xF, on 0x800F, loop part 0x800F
    let dma = regs(&a, 0xDFF096);
    assert_eq!(&dma[1..4], [0xF, 0x800F, 0x800F]);
    // the voice 1 period is 0x11C on tick 0; the row starts in tick 0's sequencer run, so tick 1 writes 453
    let per = regs(&a, 0xDFF0A6);
    assert_eq!(&per[..3], [0x11C, 453, 453]);
    // the volume is the sample's default after the row
    assert_eq!(regs(&a, 0xDFF0A8)[2], 0x30);
    // the sample is programmed with its length of 20 words and started on voice 1
    let len = regs(&a, 0xDFF0A4);
    assert!(len.contains(&20), "{len:?}");
    assert!(dma.contains(&0x8001));
    assert!(regs(&a, 0xDFF0A8).len() >= 40);
}

#[test]
fn a_looped_sample_programs_its_loop_in_the_second_phase() {
    let d = simple(&[(0, 1)], &[[0; 4], [30, 2, 0, 0]]);
    let (a, _) = run_with(d, Some(smp_file(&std_hdrs(), true)), 8);
    // sample 2 has finetune 2: the period is row 2, note index 15 - 1
    assert!(regs(&a, 0xDFF0A6).contains(&(ptab(2 * 36 + 15 - 1) as u16)));
    let len = regs(&a, 0xDFF0A4);
    // first phase (64 / 2 = 32 words), then the loop part (16 / 2 = 8 words)
    let i = len.iter().position(|&l| l == 32).expect("first phase");
    assert_eq!(len[i + 1], 8);
}

#[test]
fn speed_command_sets_the_row_length() {
    // row 0: F with parameter 3 and a note; row 1: another note. A row lasts 3 ticks.
    let cells = [[0; 4], [24, 1, 0x3C, 3], [26, 1, 0, 0]];
    let d = simple(&[(0, 1), (1, 2)], &cells);
    let (a, _) = run_with(d, Some(smp_file(&std_hdrs(), false)), 12);
    let per = regs(&a, 0xDFF0A6);
    // tick 0 writes the initial period, ticks 1-3 the first note (453), tick 4 on the second (428)
    assert_eq!(&per[..8], [0x11C, 453, 453, 453, 428, 428, 428, 428]);
}

#[test]
fn volume_command_writes_the_raw_byte() {
    let cells = [[0; 4], [24, 1, 0x30, 200]];
    let d = simple(&[(0, 1)], &cells);
    let (a, _) = run_with(d, Some(smp_file(&std_hdrs(), false)), 6);
    assert_eq!(regs(&a, 0xDFF0A8)[2], 200);
}

#[test]
fn song_end_is_reported_by_the_jump_command() {
    let cells = [[0; 4], [0, 0, 0x2C, 0]];
    let d = simple(&[(2, 1)], &cells);
    let dir = Path::new("/bp-test");
    let mut e = Engine::new(48000.0);
    e.m.os.mem_files.insert(dir.join("smp.test"), smp_file(&std_hdrs(), false));
    e.open_replayers = true;
    e.play_data(&dir.join("uds.test"), d).unwrap();
    assert!(!e.songend());
    let mut buf = vec![0f32; 2 * 960 * 30];
    e.render(&mut buf);
    assert!(e.songend());
}

#[test]
fn without_the_sample_file_no_replayer_takes_the_module() {
    let d = simple(&[(0, 1)], &[[0; 4], [24, 1, 0, 0]]);
    let (log, fmt) = run_with(d.clone(), None, 4);
    assert!(fmt.as_deref() != Some("Blade Packer"), "{log:?}");
    // a sample file of neither form
    let (_, fmt) = run_with(d, Some(vec![1u8; 600]), 4);
    assert!(fmt.as_deref() != Some("Blade Packer"));
}

/// Cases the specification leaves undefined end the voice or ignore the cell: no panic, no hang.
#[test]
fn undefined_cases_are_safe() {
    let cmds: [[u8; 4]; 12] = [
        [24, 0, 0, 0],       // note before any sample
        [0, 0, 0, 0x37],     // arpeggio before any sample or note
        [24, 40, 0, 0],      // sample number beyond the table
        [24, 1, 0x05, 0],    // command byte not a multiple of 4
        [24, 1, 0xFC, 0x55], // command byte above 60
        [0, 0, 0x2C, 0x7F],  // jump past the list
        [0, 0, 0x34, 0x7F],  // break past the pattern
        [24, 1, 0x38, 0x05], // E5 with finetune 5
        [24, 3, 0, 0x37],    // arpeggio, sample with finetune above 15
        [24, 1, 0x3C, 0],    // speed 0: 256 ticks a row
        [200, 1, 0, 0],      // odd, huge note
        [24, 1, 0x24, 0xFF], // 9 with a huge offset
    ];
    let mut hdrs = std_hdrs();
    hdrs.push((604, 640, 0, 0x40, 99));
    for (i, c) in cmds.iter().enumerate() {
        for e in [b'B', b'W'] {
            let mut cells = vec![[0; 4], *c];
            cells.push([48, 1, 0x00, 0x00]);
            let d = song(e, b'4', 0, &[vec![0, 1, 1, 1]], &[track(&[(0, 1), (1, 2), (2, 1)]), track(&[])], &cells);
            let (log, fmt) = run_with(d, Some(smp_file(&hdrs, i % 2 == 0)), 300);
            assert_eq!(fmt.as_deref(), Some("Blade Packer"), "case {i}");
            assert!(!log.is_empty());
        }
    }
}

/// Random byte changes to a working module never panic or hang.
#[test]
fn mutated_modules_are_safe() {
    let cells = [
        [0u8; 4],
        [24, 1, 0x00, 0x37],
        [26, 2, 0x10, 0x04],
        [28, 1, 0x20, 0x04],
        [30, 1, 0x28, 0x77],
        [0, 0, 0x34, 0x12],
        [32, 2, 0x38, 0x95],
    ];
    let base = song(
        b'B',
        b'4',
        0,
        &[vec![0, 1, 2, 3], vec![1, 0, 3, 2]],
        &[track(&[(0, 1), (4, 2), (8, 3)]), track(&[(0, 4), (4, 5), (8, 6)]), track(&[(2, 3)]), track(&[(1, 6)])],
        &cells,
    );
    let smp = smp_file(&std_hdrs(), false);
    let mut seed = 0x1234_5678u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..40 {
        let mut d = base.clone();
        for _ in 0..6 {
            let i = rnd() as usize % d.len();
            d[i] = rnd() as u8;
        }
        let mut s = smp.clone();
        let i = rnd() as usize % 200;
        s[i] = rnd() as u8;
        let _ = run_with(d, Some(s), 120);
    }
}
