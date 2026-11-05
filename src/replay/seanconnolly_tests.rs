use super::*;
use crate::engine::Engine;
use std::path::Path;

/// `SC_MODULE=file SC_TICKS=n SC_LOG=out [SC_SUBSONG=k] [SC_PHASE=c] [SC_NORESTART=1] cargo test --release dump -- --ignored`:
/// the register log of `n` ticks of a module (the first tick call on raster phase `c`).
#[test]
#[ignore]
fn dump() {
    let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("SC_MODULE"), std::env::var("SC_TICKS"), std::env::var("SC_LOG")) else {
        return;
    };
    if std::env::var("SC_NORESTART").is_ok() {
        NO_RESTART.store(true, Ordering::Relaxed);
    }
    if let Some(p) = std::env::var("SC_PHASE").ok().and_then(|s| s.parse::<u64>().ok()) {
        PHASE.store(p, Ordering::Relaxed);
    }
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
    let sub: u32 = std::env::var("SC_SUBSONG").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
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
// Synthetic modules, built here from the specification's layout (no game data)

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// An instrument record (16 bytes).
#[derive(Clone, Copy)]
struct Inst {
    b: [u8; 16],
}

impl Inst {
    /// Mode 0, volume 0 climbing to 4 by 2, released to 0 by 1, wave position `wave`.
    fn plain(wave: u8) -> Inst {
        let mut b = [0u8; 16];
        b[2] = 4;
        b[4] = 0x21;
        b[9] = wave;
        Inst { b }
    }
}

/// A module whose voice 0 plays `pattern` through `track0` (voices 1..3 are switched off by
/// their tracks' first token), with instrument 0 = `inst`.
fn module(track0: &[u8], pattern: &[u8], inst: Inst) -> Vec<u8> {
    let mut d = vec![0u8; 0x3400];
    for i in 0..4usize {
        let o = 4 * i;
        put16(&mut d, o, 0x6000);
        put16(&mut d, o + 2, (0x3AC + 8 * i - o - 2) as u16);
    }
    put32(&mut d, 0x3AC, 0x48E7_7FFE);
    put32(&mut d, 0x3B0, 0xE988_41FA);
    d[O_INFO..O_INFO + 11].copy_from_slice(b"Test module");
    for (i, w) in [1u16, 2, 4, 8].iter().enumerate() {
        put16(&mut d, O_WORDS + 2 * i, *w);
    }
    let mut p = 0x2A60u32;
    for i in 0..256usize {
        let v = match i {
            0 => 0x3580,
            1 => 0x3280,
            2 => 0x2FA0,
            3 => 0x2D00,
            4 => 0x2A60,
            _ => {
                p = (p * 94 / 100).max(0x71);
                p
            }
        };
        put16(&mut d, O_PERIOD + 2 * i, v as u16);
    }
    // pattern 0
    put32(&mut d, O_PATPTR, 0);
    d[O_PATDATA..O_PATDATA + pattern.len()].copy_from_slice(pattern);
    // subsong 0: voice 0 plays track0, the others a track that switches them off
    let t1 = track0.len() + 2;
    put32(&mut d, O_SONGS, 0);
    for v in 1..4 {
        put32(&mut d, O_SONGS + 4 * v, t1 as u32);
    }
    d[O_TRACKS..O_TRACKS + track0.len()].copy_from_slice(track0);
    d[O_TRACKS + t1] = 0xFE;
    d[O_INST..O_INST + 16].copy_from_slice(&inst.b);
    // audible waves, so that the engine counts the song as heard
    for i in 0..0x100 {
        d[O_WAVE + i] = (i as u8).wrapping_mul(37) | 0x20;
    }
    d
}

/// A song that plays two notes of `len` rows (six ticks each) on voice 0, then loops (the song end
/// comes when the second note starts).
fn simple_song(note: u8, len: u8, inst: Inst) -> Vec<u8> {
    module(&[0x00, 0xFF, 0x00], &[0x7F + len, 0xC0, note, note + 1, 0xFF], inst)
}

type W = (u32, u16, u64);

fn run(d: Vec<u8>, steps: usize) -> Vec<W> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("x.dat"), d).unwrap();
    assert_eq!(e.playing.as_ref().unwrap().format, "Sean Connolly");
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..steps {
        e.render(&mut buf);
    }
    e.m.bus.write_log.take().unwrap()
}

/// Whether the song end was reported within `steps` ticks.
fn song_end(d: Vec<u8>, steps: usize) -> Option<usize> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("x.dat"), d).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    for t in 0..steps {
        e.render(&mut buf);
        if e.songend() {
            return Some(t);
        }
    }
    None
}

/// The log split into ticks: a tick starts at a DMA-off write that is followed by a channel 0
/// location write; the stop call's writes belong to no tick.
fn ticks(log: &[W]) -> Vec<Vec<W>> {
    let mut out: Vec<Vec<W>> = Vec::new();
    let mut cur = false;
    for (i, w) in log.iter().enumerate() {
        let reg = w.0 & 0xFFF;
        if reg == 0x096 && w.1 & 0x8000 == 0 {
            let next = log.get(i + 1).map_or(0, |n| n.0 & 0xFFF);
            if next == 0x0A0 {
                out.push(Vec::new());
                cur = true;
            } else if next == 0x0A6 && w.1 == 0xF {
                cur = false;
            }
        }
        if cur {
            out.last_mut().unwrap().push(*w);
        }
    }
    out
}

fn reg_writes(t: &[W], reg: u32) -> Vec<(u16, u64)> {
    t.iter().filter(|w| w.0 & 0xFFF == reg).map(|w| (w.1, w.2)).collect()
}

/// The long written to a channel's location register in a tick.
fn loc(t: &[W], ch: u32) -> u32 {
    let hi = reg_writes(t, 0x0A0 + 16 * ch)[0].0 as u32;
    let lo = reg_writes(t, 0x0A2 + 16 * ch)[0].0 as u32;
    hi << 16 | lo
}

#[test]
fn detection() {
    let d = simple_song(0x10, 2, Inst::plain(3));
    assert!(detect(&d));
    // each part of the test matters
    for (o, v) in [(0usize, 0x61u8), (4, 0x61), (8, 0x61), (0x3AC, 0x49), (0x3B2, 0x00), (O_WORDS + 1, 3), (O_PERIOD + 1, 0x81)] {
        let mut x = d.clone();
        x[o] = v;
        assert!(!detect(&x), "offset {o:#x}");
    }
    // the other two generations are not specified
    let mut t1 = d.clone();
    put32(&mut t1, 0x3B0, 0x323C_00DD);
    assert!(!detect(&t1));
    let mut t3 = d.clone();
    put32(&mut t3, 0x3AC, 0x48E7_40F0);
    put32(&mut t3, 0x3B0, 0x4A00_6B0A);
    assert!(!detect(&t3));
    assert!(!detect(&[]));
    assert!(!detect(&d[..MIN_LEN - 1]));
    assert!(!detect(&[0u8; 0x4000]));
}

#[test]
fn fixed_write_offsets_and_first_ticks_carry_the_image() {
    let mut d = simple_song(0x10, 2, Inst::plain(3));
    // the register images of the file, as the first ticks write them
    for v in 0..4 {
        put32(&mut d, O_LAST + 16 * v, 0x0003_0000 + v as u32);
        put16(&mut d, O_LAST + 16 * v + 4, 0x70 + v as u16);
        put16(&mut d, O_LAST + 16 * v + 6, 0x100 + v as u16);
        put16(&mut d, O_LAST + 16 * v + 8, 0x30 + v as u16);
    }
    let log = run(d, 8);
    let ts = ticks(&log);
    assert!(ts.len() >= 6);
    let t = &ts[0];
    let c = t[0].2 - 362;
    let rel: Vec<(u32, u64)> = t.iter().map(|w| (w.0 & 0xFFF, w.2 - c)).collect();
    let want: [(u32, u64); 22] = [
        (0x096, 362),
        (0x0A0, 400),
        (0x0A2, 400),
        (0x0A4, 426),
        (0x0A6, 426),
        (0x0A8, 526),
        (0x0B0, 546),
        (0x0B2, 546),
        (0x0B4, 574),
        (0x0B6, 574),
        (0x0B8, 674),
        (0x0C0, 694),
        (0x0C2, 694),
        (0x0C4, 722),
        (0x0C6, 722),
        (0x0C8, 817),
        (0x0D0, 837),
        (0x0D2, 837),
        (0x0D4, 865),
        (0x0D6, 865),
        (0x0D8, 965),
        (0x096, 0),
    ];
    assert_eq!(&rel[..21], &want[..21]);
    // literal image values in the first tick; the final DMA enable comes after ten raster edges
    assert_eq!(t[1].1, 3);
    assert_eq!(t[2].1, 0);
    assert_eq!(t[3].1, 0x70);
    assert_eq!(t[5].1, 0x30);
    let last = t.last().unwrap();
    assert_eq!((last.0 & 0xFFF, last.1), (0x096, 0x8200));
    assert!(last.2 - c > 5000 && last.2 - c < 7500, "{}", last.2 - c);
}

#[test]
fn note_start_in_tick_four_is_written_in_tick_five() {
    let d = simple_song(0x10, 2, Inst::plain(3));
    let want_per = be16(&d, O_PERIOD + 0x20);
    let log = run(d, 12);
    let ts = ticks(&log);
    let t4 = &ts[4];
    // the module sits at a 16-byte aligned address; the wave is eight words from the wave table
    let base = loc(t4, 0).wrapping_sub(O_WAVE as u32 + (3 << 4));
    assert!(base % 16 == 0 && base > 0 && base < 0x100000, "{base:#x}");
    assert_eq!(reg_writes(t4, 0x0A4)[0].0, 8);
    assert_eq!(reg_writes(t4, 0x0A6)[0].0, want_per);
    // the first tick's images are the (zero) file images; no note has started before tick 4
    assert_eq!(reg_writes(&ts[3], 0x0A4)[0].0, 0);
    // the DMA-off before the images and the enable after them carry the previous tick's mask: the
    // note started in tick 3, so tick 4 clears and sets voice 0, tick 3 and tick 5 do not
    let dma4 = reg_writes(t4, 0x096);
    assert_eq!((dma4[0].0, dma4.last().unwrap().0), (1, 0x8201));
    for k in [3usize, 5] {
        let d = reg_writes(&ts[k], 0x096);
        assert_eq!((d[0].0, d.last().unwrap().0), (0, 0x8200));
    }
    // the inactive voices 3, 2, 1 write their DMA-off bit directly, 122 colour clocks apart
    let c = t4[0].2 - 362;
    let off: Vec<(u16, u64)> = reg_writes(t4, 0x096).iter().filter(|w| w.0 != 0 && w.0 < 16 && w.1 > c + 362).map(|w| (w.0, w.1 - c)).collect();
    assert_eq!(off, [(8, 1178), (4, 1300), (2, 1422)]);
    // a different wave position moves the location by sixteen bytes a step
    let log2 = run(simple_song(0x10, 2, Inst::plain(5)), 12);
    let t4b = &ticks(&log2)[4];
    assert_eq!(loc(t4b, 0) - loc(t4, 0), 2 << 4);
}

#[test]
fn envelope_climbs_and_releases() {
    let log = run(simple_song(0x10, 40, Inst::plain(3)), 40);
    let mut vols: Vec<u16> = Vec::new();
    for t in &ticks(&log) {
        for w in reg_writes(t, 0x0A8) {
            if vols.last() != Some(&w.0) {
                vols.push(w.0);
            }
        }
    }
    assert_eq!(vols, [0, 2, 4, 3, 2, 1, 0]);
}

#[test]
fn song_end_restarts_the_subsong() {
    let d = simple_song(0x10, 4, Inst::plain(3));
    let end = song_end(d.clone(), 60).expect("the track loop is the song end");
    // the second note starts after the first one's four rows of six ticks
    assert!((26..=29).contains(&end), "{end}");
    let log = run(d, 120);
    // the stop call: DMA off, period and volume zero for the four channels, DMA off again
    let stop: Vec<&W> = log.iter().filter(|w| w.0 & 0xFFF == 0x096 && w.1 == 0x000F).collect();
    assert!(stop.len() >= 2);
    assert_eq!(stop[1].2 - stop[0].2, 54);
    // and the song plays again: notes start again
    let n = ticks(&log).iter().filter(|t| reg_writes(t, 0x096).last().is_some_and(|w| w.0 & 15 != 0)).count();
    assert!(n >= 3, "{n}");
}

#[test]
fn all_voices_off_is_a_song_end() {
    // the only track starts with the off token
    let d = module(&[0xFE], &[0x80, 0xC0, 0x10, 0xFF], Inst::plain(3));
    // a song that ends unheard in its first seconds is skipped by the engine, as for any player
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("x.dat"), d).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..12 {
        e.render(&mut buf);
    }
    assert!(e.playing.as_ref().unwrap().sub >= 1);
}

#[test]
fn raster_wait_counts_ten_edges() {
    let tod = |t: u64| t / 2 / LINE;
    for x in [0u64, 1, 225, 454, 1000, 99_999, 3_000_001] {
        let l = wait_end(x);
        let edges = tod(l) - tod(x);
        assert!((10..=11).contains(&edges), "{x}: {edges}");
        assert!(l >= x + 9 * 454);
    }
}

#[test]
fn deterministic() {
    let d = simple_song(0x14, 3, Inst::plain(3));
    assert_eq!(run(d.clone(), 60), run(d, 60));
}

#[test]
fn garbage_does_not_panic() {
    let clean = simple_song(0x10, 2, Inst::plain(3));
    let mut s = 0x1234_5678u32;
    for seed in 0..6u32 {
        let mut d = clean.clone();
        for i in O_PATPTR..d.len() {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223 + seed);
            if (s >> 24) % 3 != 0 {
                d[i] = (s >> 16) as u8;
            }
        }
        // keep what the detection reads
        d[O_WORDS..O_WORDS + 8].copy_from_slice(&clean[O_WORDS..O_WORDS + 8]);
        d[O_PERIOD..O_PERIOD + 10].copy_from_slice(&clean[O_PERIOD..O_PERIOD + 10]);
        assert!(detect(&d));
        run(d, 150);
    }
}

#[test]
fn truncated_module_is_safe() {
    let d = simple_song(0x10, 2, Inst::plain(3));
    // just past the minimum: every table read beyond the end gives zero
    let log = run(d[..MIN_LEN + 2].to_vec(), 80);
    assert!(!log.is_empty());
}
