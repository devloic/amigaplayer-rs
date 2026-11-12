use super::*;
use crate::machine::Machine;
use m68k::CpuType;

fn longs(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

/// A block of records: the offset list (with `tail` appended to it), then the records.
fn block(recs: &[Vec<u8>], tail: &[u32], pad: bool) -> Vec<u8> {
    let n = recs.len() + tail.len();
    let mut offs = Vec::new();
    let mut body = Vec::new();
    for r in recs {
        offs.push((4 * n + body.len()) as u32);
        body.extend_from_slice(r);
        if pad && body.len() % 2 == 1 {
            body.push(0);
        }
    }
    offs.extend_from_slice(tail);
    let mut b = longs(&offs);
    b.extend(body);
    if b.len() % 2 == 1 {
        b.push(0);
    }
    b
}

/// A module: voice count `a`, the track sequences (words), patterns, instruments, tables.
fn module(a: u16, seqs: &[&[u16]], pats: &[&[u8]], insts: &[Vec<u8>], tables: &[&[u8]]) -> Vec<u8> {
    let b0 = block(&tables.iter().map(|t| t.to_vec()).collect::<Vec<_>>(), &[], false);
    let mut so = Vec::new();
    let mut body = Vec::new();
    for i in 0..32 {
        so.push((4 + 4 * 32 + body.len()) as u32);
        let s: &[u16] = seqs.get(i).copied().unwrap_or(&[0xFFFE]);
        for w in s {
            body.extend_from_slice(&w.to_be_bytes());
        }
    }
    let mut b1 = Vec::new();
    b1.extend_from_slice(&a.to_be_bytes());
    b1.extend_from_slice(&(seqs.len() as u16).to_be_bytes());
    b1.extend(longs(&so));
    b1.extend(body);
    let b2 = block(&pats.iter().map(|p| p.to_vec()).collect::<Vec<_>>(), &[], false);
    let b3 = block(insts, &[0xFFFF_FFFF], true);
    let mut d = b"E.M.S. V6.05\x00\x01".to_vec();
    let (l0, l1, l2) = (b0.len() as u32, b1.len() as u32, b2.len() as u32);
    d.extend(longs(&[l0, l1, l2, l0 + l1 + l2, b3.len() as u32]));
    d.extend(b0);
    d.extend(b1);
    d.extend(b2);
    d.extend(b3);
    d
}

fn ramp(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7) as u8).collect()
}

/// The left and right output streams (write order) of the first `blocks` mixer blocks.
fn streams(d: &[u8], blocks: usize) -> (Vec<u8>, Vec<u8>, Ems6) {
    let mut r = Ems6::build(d, 0x20000).unwrap();
    let (mut l, mut rr) = (Vec::new(), Vec::new());
    for _ in 0..blocks {
        let (o, n) = r.block();
        l.extend_from_slice(&o[0][..n]);
        rr.extend_from_slice(&o[1][..n]);
    }
    (l, rr, r)
}

const PAT: &[u8] = &[0xFE, 0x00, 0xF7, 0x40, 0x40, 0x00, 0x00, 0xFD, 0x10, 0x30, 0xFF];

#[test]
fn detect_accepts_the_layout_and_rejects_changes() {
    let d = module(4, &[&[0, 0xFFFF, 0]], &[PAT], &[ramp(0x800)], &[&[0, 0xFE]]);
    assert!(detect(&d));
    let mut x = d.clone();
    x[0] = b'X';
    assert!(!detect(&x));
    // the version digits are not looked at
    let mut x = d.clone();
    x[10] = 0xFF;
    assert!(detect(&x));
    // block 0's length zero, the voice count or the track count zero
    let mut x = d.clone();
    x[0x0E..0x12].fill(0);
    assert!(!detect(&x));
    let b1 = 0x22 + be32(&d, 0x0E) as usize;
    for o in [b1, b1 + 2] {
        let mut x = d.clone();
        x[o..o + 2].fill(0);
        assert!(!detect(&x));
    }
    assert!(!detect(&d[..63]));
    assert!(detect(&d[..64]));
}

#[test]
fn blocks_are_280_or_282_and_the_first_is_282() {
    let d = module(4, &[&[0, 0xFFFF, 0]], &[PAT], &[ramp(0x800)], &[&[0, 0xFE]]);
    let mut r = Ems6::build(&d, 0x20000).unwrap();
    let ns: Vec<usize> = (0..16).map(|_| r.block().1).collect();
    assert_eq!(ns[0], 282);
    assert!(ns.iter().all(|&n| n == 280 || n == 282));
    // 97 / 512 of the blocks are long
    let long = (0..5120).filter(|_| r.block().1 == 282).count();
    assert!((965..=975).contains(&long), "{long}");
}

#[test]
fn a_note_mixes_the_sample_at_its_step_and_volume() {
    // note 0x30: period 856, step floor(floor(3579264 / 856) * 65536 / 3504)
    let d = module(4, &[&[0, 0xFFFF, 0]], &[PAT], &[ramp(0x800), ramp(4)], &[&[0, 0xFE]]);
    let (l, rr, _) = streams(&d, 2);
    let step = (3_579_264u64 / 856 * 65536 / 3504) as i64;
    assert_eq!(step, 78198);
    for (j, &x) in l.iter().enumerate().take(500) {
        let s = ((j as i64 * step) >> 16) as usize * 7 % 256;
        let want = ((s as u8 as i8 as i32 * 64) >> 6) / 4;
        assert_eq!(x, want as u8, "sample {j}");
    }
    assert!(rr.iter().all(|&x| x == 0));
}

#[test]
fn the_volume_envelope_moves_toward_its_target() {
    // F7 V T S D: from 0 toward 64 by 8 every second tick
    let pat = [0xFE, 0x00, 0xF7, 0x00, 0x40, 0x08, 0x01, 0xFD, 0x10, 0x30, 0xFF];
    let d = module(1, &[&[0, 0xFFFF, 0]], &[&pat], &[vec![0x40; 0x4000], ramp(4)], &[&[0, 0xFE]]);
    let mut r = Ems6::build(&d, 0x20000).unwrap();
    let mut vols = Vec::new();
    for _ in 0..8 {
        r.tick_tracks();
        vols.push(r.tracks[0].v.vol);
    }
    assert_eq!(vols, [0, 8, 8, 16, 16, 24, 24, 32]);
}

#[test]
fn the_song_ends_when_every_track_has_looped() {
    let p2: &[u8] = &[0xFE, 0x00, 0xF7, 0x40, 0x40, 0x00, 0x00, 0xFD, 0x04, 0x3C, 0xFF];
    let p1: &[u8] = &[0xFE, 0x00, 0xF7, 0x40, 0x40, 0x00, 0x00, 0xFD, 0x04, 0x30, 0xFF];
    let d = module(4, &[&[0, 0xFFFF, 0], &[1, 1, 1, 0xFFFF, 0]], &[p1, p2], &[ramp(0x800)], &[&[0, 0xFE]]);
    let mut r = Ems6::build(&d, 0x20000).unwrap();
    let mut ticks = 0;
    while !r.songend && ticks < 1000 {
        r.tick_tracks();
        ticks += 1;
    }
    // three patterns of 4 rows at speed 6
    assert_eq!(ticks, 73);
}

#[test]
fn start_up_writes_and_timer() {
    let d = module(4, &[&[0, 0xFFFF, 0]], &[PAT], &[ramp(0x800)], &[&[0, 0xFE]]);
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let r = new(&d, 0x10000, &mut m).unwrap();
    let log = m.bus.write_log.take().unwrap();
    let first = log.first().unwrap();
    assert_eq!((first.0, first.1, first.2), (0xDFF096, 0x000F, 3218));
    let on = log.iter().find(|w| w.0 == 0xDFF096 && w.1 == 0x800F).unwrap();
    assert_eq!(on.2, 194130);
    assert!(log.iter().any(|w| w.0 == 0xDFF0A6 && w.1 == 0xFD));
    // the song starts 22 colour clocks after the timer (256739); the engine's first tick is
    // the reference's second interrupt
    assert_eq!(m.bus.clock, 256761);
    assert_eq!(m.bus.clock + r.timer_delay() + r.tick_clocks(), 256739 + 2 * 71050);
    assert!(r.needs_fast_reference_cpu());
}

#[test]
fn malformed_input_does_not_panic_or_hang() {
    let wild: Vec<u8> = (0..300u32).map(|i| (i * 37 % 251) as u8).collect();
    let pats: [&[u8]; 5] = [&[0xFF], &[0xF4, 0x00, 0x30], &[0xFB, 0x30, 0x01, 0x00, 0x7F], &wild, &[0xF6]];
    for p in pats {
        for seq in [&[0u16, 0xFFFF, 0][..], &[0xF902, 0xFFFF, 0], &[0xFB00, 0, 0x7FFF, 0xFFFF, 0x9999], &[0xFFFF, 0]] {
            let d = module(3, &[seq, &[0, 0xFFFF, 1]], &[p], &[ramp(64), b"SYN@".to_vec(), vec![]], &[&[0xFF], &[]]);
            let (_, _, _) = streams(&d, 300);
        }
    }
    // truncated files and wild bytes after a valid start
    let d = module(4, &[&[0, 0xFFFF, 0]], &[PAT], &[ramp(0x800)], &[&[0, 0xFE]]);
    for n in [64usize, 80, 120, 200, d.len() - 1] {
        if detect(&d[..n]) {
            let _ = streams(&d[..n], 100);
        }
    }
    let mut x = d.clone();
    for (i, b) in x.iter_mut().enumerate().skip(0x40) {
        *b = (i.wrapping_mul(2654435761) >> 13) as u8;
    }
    if detect(&x) {
        let _ = streams(&x, 300);
    }
    for n in [0usize, 1, 17, 63, 64, 900] {
        let r: Vec<u8> = (0..n as u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        assert!(!detect(&r));
    }
}

const SCRATCH: &str = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust";

#[test]
fn real_modules_when_present() {
    let mut n = 0;
    for dir in ["dl-modules/EMS_v6/files", "dl-modules/EMS_v6/more"] {
        let Ok(rd) = std::fs::read_dir(format!("{SCRATCH}/{dir}")) else { continue };
        for e in rd.flatten() {
            let d = std::fs::read(e.path()).unwrap();
            assert!(detect(&d), "{}", e.path().display());
            let (l, r, _) = streams(&d, 400);
            assert!(l.iter().chain(r.iter()).any(|&x| x != 0), "{}", e.path().display());
            n += 1;
        }
    }
    if n > 0 {
        assert_eq!(n, 10);
    }
}

/// The output streams equal the reference's (its ring writes in write order, recorded by the
/// scratch tool `refstream.py`) for the modules whose music stays within the pitch table.
#[test]
fn equals_the_reference_streams_when_present() {
    let w = format!("{SCRATCH}/fixround/port-ems6");
    let mut seen = 0;
    for name in [
        "perception test(amiga)",
        "a sporting chance",
        "tropical islands",
        "stormlord",
        "synthtest",
        "perception",
        "softsynthtest",
    ] {
        let Ok(d) = std::fs::read(format!("{w}/m/real/{name}.ems")) else { continue };
        let (Ok(rl), Ok(rr)) =
            (std::fs::read(format!("{w}/out/rs-{name}.ems.l")), std::fs::read(format!("{w}/out/rs-{name}.ems.r")))
        else {
            continue;
        };
        // at most 12000 blocks (240 s): Tropical Islands parts from the reference at block 12380
        let blocks = (rl.len() / 281).min(12000);
        let (l, r, _) = streams(&d, blocks);
        let n = l.len().min(rl.len());
        assert!(n > 100_000, "{name}");
        assert_eq!(l[..n], rl[..n], "{name} left");
        assert_eq!(r[..n], rr[..n], "{name} right");
        seen += 1;
    }
    if std::path::Path::new(&format!("{w}/out")).is_dir() {
        assert_eq!(seen, 7);
    }
}

/// `EMS6_LISTS=list.txt[:list.txt...]`: no file named in the lists (one path a line) is taken.
/// Run with `--ignored`.
#[test]
#[ignore]
fn takes_no_module_of_the_corpus_lists() {
    let Ok(lists) = std::env::var("EMS6_LISTS") else { return };
    let (mut read, mut taken) = (0, Vec::new());
    for l in lists.split(':') {
        for p in std::fs::read_to_string(l).unwrap_or_default().lines().filter(|p| !p.starts_with('#')) {
            let Ok(d) = std::fs::read(p.trim()) else { continue };
            read += 1;
            if detect(&d) {
                taken.push(p.to_string());
            }
        }
    }
    eprintln!("{read} files read, {} taken", taken.len());
    assert!(taken.is_empty(), "{taken:?}");
}
