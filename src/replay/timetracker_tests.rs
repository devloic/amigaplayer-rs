use super::*;
use crate::engine::Engine;
use std::path::Path;

const DIR: &str = "/tmk-test/";

/// A one-subsong single-file module: one sample (`SMP` bytes, a ramp, no loop, finetune 0, volume
/// 64), a position list of `positions` patterns, the first pattern's rows `rows` (row number and
/// the four voices' entries, as bytes) and silence in every other row.
fn module(positions: &[u8], rows: &[(usize, [&[u8]; 4])], delta: bool, with_samples: bool) -> Vec<u8> {
    const SMP: usize = 64;
    let npat = *positions.iter().max().unwrap() as usize + 1;
    let mut d = b"TMK".to_vec();
    d.push(1);
    d.push(positions.len() as u8);
    d.push(1 | if delta { 0x80 } else { 0 });
    // the sample: length (words), repeat start, repeat length, finetune, volume
    d.extend_from_slice(&[0, (SMP / 2) as u8, 0, 0, 0, 1, 0, 64]);
    for &p in positions {
        d.push(p * 2);
    }
    if positions.len() % 2 == 1 {
        d.push(0);
    }
    let tab = d.len();
    d.resize(tab + npat * 128, 0);
    // every row starts as the silent one, which follows the table
    let silent = npat * 128;
    for r in 0..npat * 64 {
        d[tab + 2 * r..tab + 2 * r + 2].copy_from_slice(&(silent as u16).to_be_bytes());
    }
    d.extend_from_slice(&[0x80; 4]);
    for (r, e) in rows {
        let at = (d.len() - tab) as u16;
        d[tab + 2 * r..tab + 2 * r + 2].copy_from_slice(&at.to_be_bytes());
        for v in e {
            d.extend_from_slice(v);
        }
    }
    if with_samples {
        let mut s: Vec<u8> = (0..SMP as u8).map(|i| i.wrapping_mul(3).wrapping_add(5)).collect();
        if delta {
            // stored as differences: each byte minus the one after it
            let plain = s.clone();
            for i in 0..SMP - 1 {
                s[i] = plain[i].wrapping_sub(plain[i + 1]);
            }
        }
        d.extend_from_slice(&s);
    }
    d
}

type Log = Vec<(u32, u16, u64)>;

fn play(path: &str, d: Vec<u8>, companion: Option<Vec<u8>>, ticks: usize) -> Option<(Log, bool)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if let Some(c) = companion {
        let p = Path::new(path);
        let name = p.file_name().unwrap().to_string_lossy().replace("TMK.", "SMP.");
        e.m.os.mem_files.insert(p.parent().unwrap().join(name), c);
    }
    e.play_data(Path::new(path), d).ok()?;
    if e.playing.as_ref()?.format != "TimeTracker" {
        return None;
    }
    let mut out = vec![0f32; 2 * 960];
    let mut end = false;
    for _ in 0..=ticks {
        e.render(&mut out);
        end |= e.songend();
    }
    Some((e.m.bus.write_log.take().unwrap(), end))
}

/// The audio-register writes of each tick (address, value), the start group first.
fn ticks(l: &Log) -> Vec<Vec<(u32, u16)>> {
    let mut t: Vec<Vec<(u32, u16)>> = Vec::new();
    let mut last = None;
    for &(a, v, c) in l {
        if !(0xDFF096..=0xDFF0DF).contains(&a) {
            continue;
        }
        if last.is_none_or(|p| c - p > 20000) {
            t.push(Vec::new());
        }
        last = Some(c);
        t.last_mut().unwrap().push((a, v));
    }
    t
}

const VOL: [u32; 4] = [0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8];
const PER: [u32; 4] = [0xDFF0A6, 0xDFF0B6, 0xDFF0C6, 0xDFF0D6];
const DMACON: u32 = 0xDFF096;

fn simple() -> Vec<u8> {
    // row 0: voice 0 plays C-1 with sample 1 (the short form), voice 2 an effect-only entry
    module(&[0], &[(0, [&[0x81, 1], &[0x80], &[0xC9, 0x00], &[0x80]])], false, true)
}

#[test]
fn detects_single_file_and_split_forms() {
    let d = simple();
    assert!(detect(&d));
    // the song file alone (the two-file form): the samples are not needed to be recognised
    assert!(detect(&d[..d.len() - 64]));
    // not a TimeTracker module
    let mut x = d.clone();
    x[3] = 0;
    assert!(!detect(&x));
    x = d.clone();
    x[0] = b'M';
    assert!(!detect(&x));
    // sample volume above 64, finetune above 15, a song of no positions, no samples
    for (o, v) in [(13usize, 65u8), (12, 16), (4, 0), (5, 0)] {
        x = d.clone();
        x[o] = v;
        assert!(!detect(&x), "byte {o}");
    }
    // a ProTracker module that happens to be titled "TMK"
    let mut pt = vec![0u8; 1084 + 1024];
    pt[..3].copy_from_slice(b"TMK");
    pt[3] = b'x';
    pt[950] = 1;
    pt[1080..1084].copy_from_slice(b"M.K.");
    assert!(!detect(&pt));
    assert!(!detect(&[]));
    assert!(!detect(b"TMK\x01"));
}

#[test]
fn truncated_and_corrupt_input_is_survived() {
    let d = simple();
    for n in 0..d.len() {
        let _ = detect(&d[..n]);
    }
    let mut x = d.clone();
    for i in 4..x.len().min(200) {
        let keep = x[i];
        x[i] = 0xFF;
        if detect(&x) {
            let _ = play("/tmk-test/x.tmk", x.clone(), None, 40);
        }
        x[i] = keep;
    }
}

#[test]
fn start_group_and_first_row() {
    let (log, _) = play("/tmk-test/a.tmk", simple(), None, 8).unwrap();
    let t = ticks(&log);
    // the host's start: two passes of the four volumes
    assert_eq!(t[0].len(), 8);
    assert!(t[0].iter().all(|w| VOL.contains(&w.0)));
    // ticks 1..5 are effect ticks: the volumes, then every voice's period (no effect)
    for k in 1..=5 {
        let want: Vec<(u32, u16)> = VOL.iter().map(|&a| (a, 0)).chain(PER.iter().map(|&a| (a, 0))).collect();
        assert_eq!(t[k], want, "tick {k}");
    }
    // tick 6 is the first row
    let r = &t[6];
    let mut want: Vec<(u32, u16)> = VOL.iter().map(|&a| (a, 0)).collect();
    // voice 0: the period (its previous entry was empty), the sample's volume, DMA off, pointer,
    // length, the note's period
    want.push((PER[0], 0));
    want.push((VOL[0], 64));
    want.push((DMACON, 1));
    let loc = r.iter().find(|w| w.0 == 0xDFF0A0).unwrap().1;
    let lo = r.iter().find(|w| w.0 == 0xDFF0A2).unwrap().1;
    assert!(loc != 0 || lo != 0);
    want.push((0xDFF0A0, loc));
    want.push((0xDFF0A2, lo));
    want.push((0xDFF0A4, 32));
    want.push((PER[0], 856));
    // voice 1 (empty), voice 2 (an effect-only entry: effect 9, no note): the period first
    want.push((PER[1], 0));
    want.push((PER[2], 0));
    want.push((PER[3], 0));
    // the restart: DMA on, then the four loop pairs (voice 0: the sample's own start, one word)
    want.push((DMACON, 0x8001));
    for v in 0..4u32 {
        let (h, l, n) = if v == 0 { (loc, lo, 1) } else { (0, 0, 0) };
        want.push((0xDFF0A0 + 16 * v, h));
        want.push((0xDFF0A2 + 16 * v, l));
        want.push((0xDFF0A4 + 16 * v, n));
    }
    assert_eq!(*r, want);
}

#[test]
fn gaps_in_a_tick_follow_the_original() {
    let (log, _) = play("/tmk-test/a.tmk", simple(), None, 3).unwrap();
    // an effect tick: the volumes 181, 208 and 219 colour clocks apart, the first 265 after the call
    let l: Vec<_> = log.iter().filter(|w| (0xDFF0A8..=0xDFF0D8).contains(&w.0) && (w.0 & 15) == 8).collect();
    let k = l.len() - 4;
    assert_eq!(l[k + 1].2 - l[k].2, 181);
    assert_eq!(l[k + 2].2 - l[k + 1].2, 208);
    assert_eq!(l[k + 3].2 - l[k + 2].2, 219);
    // ticks 70940 colour clocks apart
    assert_eq!(l[k].2 - l[k - 4].2, 70940);
}

#[test]
fn delta_samples_are_decoded_and_sample_heads_zeroed() {
    // the same ramp stored plain and delta coded: the sample memory ends up identical
    let rows: &[(usize, [&[u8]; 4])] = &[(0, [&[0x81, 1], &[0x80], &[0x80], &[0x80]])];
    let first_ptr = |d: Vec<u8>| {
        let (log, _) = play("/tmk-test/d.tmk", d, None, 8).unwrap();
        log.iter().find(|w| w.0 == 0xDFF0A2).unwrap().1
    };
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/tmk-test/d.tmk"), module(&[0], rows, true, true)).unwrap();
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..8 {
        e.render(&mut out);
    }
    let log = e.m.bus.write_log.take().unwrap();
    let hi = log.iter().find(|w| w.0 == 0xDFF0A0).unwrap().1 as u32;
    let lo = log.iter().find(|w| w.0 == 0xDFF0A2).unwrap().1 as u32;
    let at = (hi << 16 | lo) as usize;
    let s: Vec<u8> = (0..64u8).map(|i| i.wrapping_mul(3).wrapping_add(5)).collect();
    assert_eq!(&e.m.bus.ram[at..at + 4], &[0, 0, 0, 0]);
    assert_eq!(&e.m.bus.ram[at + 4..at + 64], &s[4..]);
    // (the plain module's pointer is in the same place relative to its end)
    let _ = first_ptr(module(&[0], rows, false, true));
}

#[test]
fn companion_file_form() {
    let d = simple();
    let song = d[..d.len() - 64].to_vec();
    let samples = d[d.len() - 64..].to_vec();
    let (a, _) = play("/tmk-test/TMK.a", song.clone(), Some(samples), 8).unwrap();
    // without the companion nothing plays it
    assert!(play("/tmk-test/TMK.a", song, None, 8).is_none());
    let (b, _) = play("/tmk-test/a.tmk", d, None, 8).unwrap();
    assert_eq!(ticks(&a).len(), ticks(&b).len());
    // the same writes, apart from where the samples lie
    let strip = |l: &Log| -> Vec<(u32, u16)> { l.iter().filter(|w| !(0xDFF0A0..=0xDFF0A2).contains(&w.0)).map(|w| (w.0, w.1)).collect() };
    assert_eq!(strip(&a), strip(&b));
}

#[test]
fn first_porta_after_start_slides_by_zero() {
    // row 0: C-1 with a slide up of 4 (effect 1) on voice 0, long form: note 1, sample 1, effect 1
    let d = module(&[0], &[(0, [&[0x02, 0x11, 0x04], &[0x80], &[0x80], &[0x80]])], false, true);
    let (log, _) = play("/tmk-test/p.tmk", d, None, 13).unwrap();
    let t = ticks(&log);
    let p0: Vec<u16> = (7..=11).map(|k| t[k].iter().find(|w| w.0 == PER[0]).unwrap().1).collect();
    // the mask is 0 for the first slide of the song: the period is written unchanged, then it moves
    assert_eq!(p0, [856, 852, 848, 844, 840]);
}

#[test]
fn bxx_reports_the_song_end_at_once() {
    let d = module(&[0, 0], &[(0, [&[0xCB, 0x00], &[0x80], &[0x80], &[0x80]])], false, true);
    let (_, end) = play("/tmk-test/b.tmk", d, None, 7).unwrap();
    assert!(end);
    let d = module(&[0], &[], false, true);
    let (_, end) = play("/tmk-test/b.tmk", d, None, 7).unwrap();
    assert!(!end);
    // a song of one pattern of silence ends after its 64 rows of 6 ticks
    let d = module(&[0], &[], false, true);
    let (_, end) = play("/tmk-test/b.tmk", d, None, 64 * 6 + 2).unwrap();
    assert!(end);
}

#[test]
fn deterministic() {
    let a = play("/tmk-test/a.tmk", simple(), None, 30).unwrap().0;
    let b = play("/tmk-test/a.tmk", simple(), None, 30).unwrap().0;
    assert_eq!(a, b);
}

#[test]
fn raster_wait_ends_nine_lines_on() {
    let mut e = Engine::new(48000.0);
    let mut hw = Hw { bus: &mut e.m.bus };
    for start in [0u64, 100, 226, 227, 5000] {
        hw.bus.clock = start;
        raster_wait(&mut hw, 54);
        let used = hw.bus.clock - start;
        // nine line changes: between eight and nine lines, plus the poll granularity and the tail
        assert!((8 * 227 + 54..9 * 227 + 54 + 40).contains(&used), "{start}: {used}");
    }
}
