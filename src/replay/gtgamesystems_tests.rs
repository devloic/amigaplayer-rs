use super::*;
use crate::engine::{Backend, Engine, Playing};
use std::path::Path;

/// A module of the files' layout (SPEC section 2): `samples` (data, volume, loop start, loop length)
/// one after the other, the three voices' records with the end record appended.
fn module(samples: &[(Vec<u8>, u16, u16, u16)], voices: [&[(u16, u16, u16)]; 3]) -> Vec<u8> {
    let mut ents = Vec::new();
    let mut data = Vec::new();
    for (d, vol, ls, ll) in samples {
        ents.push((data.len() as u32, d.len() as u16, *vol, *ls, *ll));
        data.extend_from_slice(d);
    }
    ents.resize(8, (0, 0, 0, 0, 0));
    let mut vd: Vec<Vec<u8>> = Vec::new();
    for v in voices {
        let mut b = Vec::new();
        for &(p, s, t) in v.iter().chain([(0xFFFF, 0xFFFF, 0xFFFF)].iter()) {
            for w in [p, s, t] {
                b.extend(w.to_be_bytes());
            }
        }
        vd.push(b);
    }
    let offs = [data.len(), data.len() + vd[0].len(), data.len() + vd[0].len() + vd[1].len()];
    let mut d = Vec::new();
    let size = 0x70 + data.len() + vd.iter().map(|v| v.len()).sum::<usize>();
    d.extend((size as u32).to_be_bytes());
    for (o, l, v, ls, ll) in ents {
        d.extend(o.to_be_bytes());
        for w in [l, v, ls, ll] {
            d.extend(w.to_be_bytes());
        }
    }
    for o in offs {
        d.extend((o as u32).to_be_bytes());
    }
    d.extend(data);
    for v in vd {
        d.extend(v);
    }
    d
}

fn wave(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 37) % 200) as u8 ^ 0x80).collect()
}

fn std_samples() -> Vec<(Vec<u8>, u16, u16, u16)> {
    vec![(wave(1000), 64, 0, 2), (wave(2000), 64, 0, 2), (wave(600), 48, 0, 2), (wave(1000), 64, 100, 400)]
}

/// Plays `d` for `secs` seconds on the engine: the write log (register, value, clock).
fn play(d: Vec<u8>, secs: f64) -> (Engine, Vec<(u32, u16, u64)>) {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/gt-test/DUX.test"), d).unwrap();
    assert!(matches!(&e.playing, Some(Playing { backend: Backend::Replay(_), .. })));
    let mut out = vec![0f32; 2 * (48000.0 * secs) as usize];
    e.render(&mut out);
    let log = e.m.bus.write_log.take().unwrap();
    (e, log)
}

#[test]
fn detect_follows_the_check() {
    let d = module(&std_samples(), [&[(300, 0, 6)], &[(320, 1, 6)], &[(340, 2, 6)]]);
    assert!(detect(&d));
    // bytes after the size are allowed
    let mut x = d.clone();
    x.extend([0xFF; 6]);
    assert!(detect(&x));
    // the size past the file, or not at an end record
    let mut x = d.clone();
    x[3] += 2;
    assert!(!detect(&x));
    let mut x = d.clone();
    x[3] -= 2;
    assert!(!detect(&x));
    // an end record broken (voice 0's, voice 2's)
    let v1 = 0x70 + be32(&d, 0x68) as usize;
    let mut x = d.clone();
    x[v1 - 1] = 0;
    assert!(!detect(&x));
    let mut x = d.clone();
    let n = x.len();
    x[n - 6] = 0;
    assert!(!detect(&x));
    // a sample offset past the size
    let mut x = d.clone();
    x[4..8].copy_from_slice(&0x100000u32.to_be_bytes());
    assert!(!detect(&x));
    // voices out of order
    let mut x = d.clone();
    x[0x68..0x6C].copy_from_slice(&(be32(&d, 0x68) - 6).to_be_bytes());
    assert!(!detect(&x));
    for n in [0usize, 4, 0x70, 0x82, 900, 5000] {
        let r: Vec<u8> = (0..n as u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        assert!(!detect(&r));
        assert!(!detect(&vec![0xFF; n]));
        assert!(!detect(&vec![0; n]));
    }
}

/// The writes of channel `k` (registers $DFF0A0 + 16k ..) and DMACON writes of its bit.
fn chan(log: &[(u32, u16, u64)], k: u32) -> Vec<(u32, u16, u64)> {
    log.iter()
        .copied()
        .filter(|w| (w.0 & !0xF) == 0xDFF0A0 + 16 * k || (w.0 == 0xDFF096 && w.1 & 0x7FFF == 1 << k))
        .collect()
}

#[test]
fn a_note_is_a_flush_and_a_write_then_period_and_volume() {
    let d = module(&std_samples(), [&[(300, 0, 6)], &[(320, 2, 6)], &[(340, 1, 6)]]);
    let (e, log) = play(d, 0.05);
    let base = e.m.deli.list.first().map(|l| l.0).unwrap() + 0x70;
    let c1 = chan(&log, 1);
    let regs: Vec<(u32, u16)> = c1.iter().take(8).map(|w| (w.0, w.1)).collect();
    let a = base + 3000;
    assert_eq!(
        regs,
        [
            (0xDFF096, 2),
            (0xDFF0B0, (a >> 16) as u16),
            (0xDFF0B2, a as u16),
            (0xDFF0B4, 300),
            (0xDFF096, 2),
            (0xDFF096, 0x8002),
            (0xDFF0B6, 320),
            (0xDFF0B8, 48),
        ]
    );
    // the flush 242 colour clocks before the write, which falls in one clock
    assert_eq!(c1[1].2 - c1[0].2, 242);
    assert!(c1[1..6].iter().all(|w| w.2 == c1[1].2));
}

#[test]
fn tick_times_follow_the_cycle_model() {
    // all three voices at once: flushes 689 cc apart (SPEC section 6)
    let d = module(&std_samples(), [&[(300, 0, 6)], &[(320, 1, 6)], &[(340, 2, 6)]]);
    let (_, log) = play(d, 0.05);
    let fl: Vec<u64> = log.iter().filter(|w| w.0 == 0xDFF096 && w.1 < 8).map(|w| w.2).collect();
    assert_eq!(fl[1] - fl[0], 242);
    assert_eq!(fl[3] - fl[2], 242);
    assert_eq!(fl[2] - fl[0], 689);
    assert_eq!(fl[4] - fl[2], 689);
}

#[test]
fn a_looped_sample_sends_a_second_endless_write() {
    let d = module(&std_samples(), [&[(300, 3, 6)], &[(320, 1, 6)], &[(340, 2, 6)]]);
    let mut r = GtGameSystems::build(&d, 0x10000, &layout(&d).unwrap());
    let (req, lp) = r.note_req(3, 300);
    assert_eq!(req.cycles, 1);
    assert_eq!(lp, Some((req.data + 100, 400)));
    // sign extension of lengths and loop values; no loop for loop start 0
    let mut x = d.clone();
    let e = 4 + 12 * 3;
    x[e + 4..e + 6].copy_from_slice(&0x9000u16.to_be_bytes());
    x[e + 8..e + 10].copy_from_slice(&0xFFFFu16.to_be_bytes());
    x[e + 10..e + 12].copy_from_slice(&0xFFF0u16.to_be_bytes());
    r.d = x.clone();
    let (req, lp) = r.note_req(3, 300);
    assert_eq!(req.len, 0xFFFF_9000);
    assert_eq!(lp, Some((req.data.wrapping_sub(1), 0xFFFF_FFF0)));
    x[e + 8..e + 10].copy_from_slice(&0u16.to_be_bytes());
    r.d = x;
    assert_eq!(r.note_req(3, 300).1, None);
    // volume: the word AND 0x3FF; samples 8 and up play nothing
    let mut x = d.clone();
    x[e + 6..e + 8].copy_from_slice(&0x8040u16.to_be_bytes());
    r.d = x;
    assert_eq!(r.note_req(3, 300).0.vol, 64);
    assert_eq!(r.note_req(8, 300).0.len, 0);
    // the loop's write goes to the same request after the note's and waits in the device's queue
    // (SPEC section 7: the device then reads cycles 0 from the request when the note starts, so the
    // note's sample repeats and the loop is never taken, as in the reference)
    let lay = layout(&d).unwrap();
    let mut r = GtGameSystems::build(&d, 0x10000, &lay);
    let mut m = Machine::new(m68k::CpuType::M68000);
    let mut hw = Hw { bus: &mut m.bus };
    r.tick(&mut hw);
    let start = 0x10000 + 0x70 + 3600;
    assert_eq!((r.v[0].req.data, r.v[0].req.len, r.v[0].req.cycles), (start + 100, 400, 0));
    assert!(r.v[0].ch.latched && r.v[0].ch.queued == 1);
    let (e, log) = play(d, 0.5);
    let base = e.m.deli.list.first().map(|l| l.0).unwrap() + 0x70;
    let c0 = chan(&log, 0);
    assert!(c0.iter().any(|w| w.0 == 0xDFF0A2 && w.1 == (base + 3600) as u16), "{c0:x?}");
    assert!(!c0.iter().any(|w| w.0 == 0xDFF0A2 && w.1 == (base + 3700) as u16), "{c0:x?}");
}

#[test]
fn rests_durations_and_the_song_end() {
    // voice 0: a rest first, then notes of 1 tick; the others end after 4 ticks
    let d = module(&std_samples(), [&[(0xFFFF, 0xFFFF, 2), (300, 0, 1), (310, 0, 1)], &[(320, 1, 4)], &[(340, 2, 4)]]);
    let (e, log) = play(d, 1.0);
    let c0 = chan(&log, 0);
    let flushes: Vec<u64> = c0.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 1).map(|w| w.2).collect();
    assert!(flushes.len() >= 2);
    // the song end is reported when the voices end (4 ticks)
    assert!(e.songend());
    // duration 0 waits 65 536 ticks
    let d = module(&std_samples(), [&[(300, 0, 0)], &[(320, 1, 4)], &[(340, 2, 4)]]);
    let lay = layout(&d).unwrap();
    let mut r = GtGameSystems::build(&d, 0x10000, &lay);
    let mut m = Machine::new(m68k::CpuType::M68000);
    let mut hw = Hw { bus: &mut m.bus };
    r.tick(&mut hw);
    for _ in 0..1000 {
        r.prepare(&mut hw, 0);
        assert!(!r.v[0].note);
    }
}

#[test]
fn song_start_follows_the_init() {
    let d = module(&std_samples(), [&[(300, 0, 6), (320, 0, 6)], &[(320, 1, 12)], &[(340, 2, 12)]]);
    let lay = layout(&d).unwrap();
    // records of voice 0 with its end: 3; entries with a length: 4
    assert_eq!(init_cycles(&d, &lay), 12883 + 43 * 3 + 8 * 4);
}

#[test]
fn malformed_input_does_not_panic_or_hang() {
    let base = module(&std_samples(), [&[(300, 0, 6), (0xFFFF, 0, 3)], &[(320, 9, 1)], &[(340, 0x7FFF, 2)]]);
    let mut cases = vec![base.clone()];
    // wild entries: offsets near the size, huge lengths, odd values
    let mut x = base.clone();
    for i in 0..8 {
        let e = 4 + 12 * i;
        x[e..e + 4].copy_from_slice(&((base.len() as u32 - 1) - i as u32).to_be_bytes());
        x[e + 4..e + 12].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x80, 0x00, 0x7F, 0xFF]);
    }
    cases.push(x);
    // voice 0 of its end record only, records of zeros
    cases.push(module(&std_samples(), [&[], &[(0, 0, 0)], &[(0, 0, 1); 40]]));
    // a voice 0 start misplaced (not on a record)
    let mut x = base.clone();
    x[0x67] += 2;
    cases.push(x);
    for d in cases {
        assert!(detect(&d));
        let (_, log) = play(d, 1.5);
        assert!(log.len() < 100_000);
    }
}

#[test]
fn real_modules_when_present() {
    let dir = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/dl-modules/G&T_Game_Systems/files";
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        let d = std::fs::read(&p).unwrap();
        let is = p.extension().is_some_and(|x| x == "dux");
        assert_eq!(detect(&d), is, "{}", p.display());
        if is {
            n += 1;
            let (_, log) = play(d, 2.0);
            assert!(log.iter().any(|w| (w.0 & 0xF) == 8 && w.0 >= 0xDFF0A8 && w.1 == 64), "{}", p.display());
        }
    }
    assert_eq!(n, 7);
}

/// `GT_LISTS=list.txt[:list.txt...]`: no file named in the lists (one path a line) is taken,
/// except G&T Game Systems ones (`.dux`, `dux.`). Run with `--ignored`.
#[test]
#[ignore]
fn takes_no_module_of_the_corpus_lists() {
    let Ok(lists) = std::env::var("GT_LISTS") else { return };
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
    assert!(taken.iter().all(|p| p.to_lowercase().ends_with(".dux") || p.to_lowercase().contains("/dux.")), "{taken:?}");
}
