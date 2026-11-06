use super::*;
use crate::engine::Engine;
use std::path::PathBuf;

/// A pattern row word.
fn row(note: u32, ins: u32, flags: u32, vol: u32, data: u32, legato: u32) -> u32 {
    legato << 31 | note << 24 | ins << 20 | flags << 16 | (vol & 0xFF) << 8 | data
}

fn note(n: u32, ins: u32) -> u32 {
    row(n, ins, 0, 0, 0, 0)
}

/// A song: one 4-entry position per element (pattern, volume offset, instrument offset, transpose
/// per voice), patterns of up to 32 rows, one slot (volume 40, length 8, repeat 4 at 2) named "s1".
fn song(speed: u8, start: u8, end: u8, tracks: &[[(u8, u8, u8, u8); 4]], pats: &[Vec<u32>], tail: &[u8]) -> Vec<u8> {
    let mut d = vec![tracks.len() as u8 - 1, pats.len() as u8 - 1, start, end, speed];
    for i in 0..16 {
        let mut s = [0u8; 54];
        if i == 0 {
            s[4..6].copy_from_slice(b"s1");
            s[44] = 40;
            s[46..48].copy_from_slice(&8u16.to_be_bytes());
            s[48..50].copy_from_slice(&4u16.to_be_bytes());
            s[50..52].copy_from_slice(&2u16.to_be_bytes());
        }
        d.extend(s);
    }
    for t in tracks {
        for &(p, v, i, tr) in t {
            d.extend([p, v, i, tr]);
        }
    }
    for p in pats {
        let mut p = p.clone();
        p.resize(32, 0);
        for r in p {
            d.extend(r.to_be_bytes());
        }
    }
    d.extend(tail);
    d
}

fn plain_tracks() -> Vec<[(u8, u8, u8, u8); 4]> {
    vec![[(0, 0, 0, 0), (1, 0, 0, 0), (1, 0, 0, 0), (1, 0, 0, 0)]]
}

type Log = Vec<(u32, u16, u64)>;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str, samples: &[(&str, Vec<u8>)]) -> Dir {
        let p = std::env::temp_dir().join(format!("zm-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("Samples")).unwrap();
        for (n, b) in samples {
            std::fs::write(p.join("Samples").join(n), b).unwrap();
        }
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn play(tag: &str, d: Vec<u8>, with_sample: bool, ticks: usize) -> Option<(Log, bool)> {
    let smp: Vec<(&str, Vec<u8>)> = if with_sample { vec![("s1", (0..16u8).collect())] } else { vec![] };
    let dir = Dir::new(tag, &smp);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(&dir.0.join("sng.test"), d).ok()?;
    if e.playing.as_ref()?.format != "ZoundMonitor" {
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

fn split(log: &Log) -> Vec<Vec<(u32, u16, u64)>> {
    let mut v: Vec<Vec<(u32, u16, u64)>> = Vec::new();
    let mut last = 0;
    for &x in log {
        if v.is_empty() || x.2 - last > 30000 {
            v.push(Vec::new());
        }
        last = x.2;
        v.last_mut().unwrap().push(x);
    }
    v
}

const DMACON: u32 = 0xDFF096;
fn per(c: u32) -> u32 {
    0xDFF0A6 + 16 * c
}
fn vol(c: u32) -> u32 {
    0xDFF0A8 + 16 * c
}
fn len(c: u32) -> u32 {
    0xDFF0A4 + 16 * c
}
fn lc(c: u32) -> u32 {
    0xDFF0A0 + 16 * c
}

#[test]
fn detects_both_trailers() {
    let p = vec![note(13, 1)];
    for tail in [&b"df0:Samples\0"[..], b"Xamp", b"dfx:", b"dumpamp"] {
        let d = song(6, 0, 1, &plain_tracks(), &[p.clone(), vec![]], tail);
        let ok = tail != b"dumpamp";
        assert_eq!(detect(&d), ok, "{tail:?}");
    }
    let d = song(6, 0, 1, &plain_tracks(), &[p.clone(), vec![]], b"df0:x");
    // the trailer must follow the patterns, and the file must go on past the end
    assert!(!detect(&d[..d.len() - 5]));
    assert!(!detect(&d[..d.len() - 2]));
    assert!(!detect(&[]));
    assert!(!detect(&[0, 0, 0, 1, 6]));
    let mut bad = d.clone();
    bad[2 + 0] = 9;
    assert!(detect(&bad));
    let mut bad = d.clone();
    bad[0] = 5;
    assert!(!detect(&bad));
}

#[test]
fn period_table_and_state_words() {
    let d = song(6, 2, 7, &plain_tracks(), &[vec![], vec![]], b"df0:");
    let mut z = Zm {
        d: d.clone(),
        t_max: 0,
        p_max: 1,
        start: 2,
        end: 7,
        speed: 6,
        addr: [0; 16],
        len: [0; 16],
        pos: 3,
        row: 9,
        tick: 4,
        mask: 5,
        v: [Voice::default(); 4],
        songend: false,
        base_cc: 0,
        t: 0,
    };
    assert_eq!(z.ptab(0), 0);
    assert_eq!(z.ptab(1), 856);
    assert_eq!(z.ptab(36), 113);
    assert_eq!(z.ptab(37), 0x0206);
    assert_eq!(z.ptab(38), 15);
    assert_eq!(z.ptab(39), 1);
    assert_eq!(z.ptab(40), 4);
    assert_eq!(z.ptab(41), 0x0903);
    assert_eq!(z.ptab(42), 0x0207);
    assert_eq!(z.ptab(44), 5);
    z.v[1].vol = 33;
    z.v[1].per = 400;
    z.v[1].bit = 2;
    assert_eq!(z.ptab(45 + 20), 33);
    assert_eq!(z.ptab(45 + 20 + 2), 400);
    assert_eq!(z.ptab(45 + 20 + 3), 2);
    assert_eq!(z.ptab(125), 0xF400);
    assert_eq!(z.ptab(126), 0);
    assert_eq!(z.ptab(255), 0);
}

#[test]
fn missing_sample_file_refuses_the_module() {
    let d = song(6, 0, 1, &plain_tracks(), &[vec![note(13, 1)], vec![]], b"df0:");
    assert!(play("missing", d.clone(), false, 2).is_none());
    assert!(play("present", d, true, 2).is_some());
}

#[test]
fn first_row_tick_follows_the_specified_gaps() {
    // voice 0 starts note 13 with instrument 1 (volume 40, in range); no effect
    let d = song(6, 0, 1, &plain_tracks(), &[vec![note(13, 1)], vec![]], b"df0:");
    let (log, _) = play("gaps", d, true, 3).unwrap();
    let t = split(&log);
    let t0 = &t[0];
    let regs: Vec<u32> = t0.iter().map(|x| x.0).collect();
    assert_eq!(&regs[..6], &[DMACON, lc(0), lc(0) + 2, len(0), per(0), vol(0)]);
    assert_eq!(t0[0].1, 1);
    assert_eq!(t0[3].1, 8);
    assert_eq!(t0[4].1, 428);
    assert_eq!(t0[5].1, 40);
    let rel = |i: usize| t0[i].2 - t0[0].2;
    // from the DMACON write: LC +38, LEN +48, PER +125, VOL +276 (the specification's costs, in cc)
    assert_eq!(t0[1].2, t0[2].2);
    assert!((rel(1) as i64 - 38).abs() <= 1, "{}", rel(1));
    assert!((rel(3) as i64 - 48).abs() <= 1);
    assert!((rel(4) as i64 - 125).abs() <= 1);
    assert!((rel(5) as i64 - 276).abs() <= 1);
    // voice 1 has no note: its effect step writes LC and LEN only
    assert_eq!(t0[6].0, lc(1));
    // the next tick switches the voice on (mask 1 | 0x8200) and then rewrites LC/LEN with the repeat part
    let t1 = &t[1];
    assert_eq!((t1[0].0, t1[0].1), (DMACON, 0x8201));
    assert_eq!(t1[1].0, lc(0));
    assert_eq!(t1[3].0, len(0));
    assert_eq!(t1[3].1, 4);
    // the first write after the wait of nine raster lines: its clock is 183 cc past the entry for the
    // DMACON, and the wait puts the next write 2000 to 2400 cc after
    assert!(t1[1].2 - t1[0].2 > 1900 && t1[1].2 - t1[0].2 < 2400, "{}", t1[1].2 - t1[0].2);
    // ticks are 70 940 cc apart: the second tick's DMACON falls 183 cc after its entry, the first
    // one's 706 cc after its entry (volume in range)
    assert_eq!(t1[0].2 + 706 - 183 - t0[0].2, 70940);
}

#[test]
fn volume_clamps_and_offsets() {
    // volume 40 + row offset 100 -> 64; +(-100) -> 0; track offset added
    let p = vec![row(13, 1, 0, 100, 0, 0), 0, 0, 0, 0, 0, row(13, 1, 0, 0xFF - 99, 0, 0)];
    let d = song(1, 0, 1, &plain_tracks(), &[p, vec![]], b"df0:");
    let (log, _) = play("vol", d, true, 8).unwrap();
    let vols: Vec<u16> = log.iter().filter(|x| x.0 == vol(0)).map(|x| x.1).collect();
    assert_eq!(vols[0], 64);
    assert_eq!(vols[1], 0);
}

#[test]
fn arpeggio_writes_on_even_counts() {
    // speed 8, note 13 with arpeggio offsets 4 and 7 (data 0x47): periods of 13, 17, 20, 13 ...
    let p = vec![row(13, 1, 1, 0, 0x47, 0)];
    let d = song(8, 0, 1, &plain_tracks(), &[p, vec![]], b"df0:");
    let (log, _) = play("arp", d, true, 8).unwrap();
    let pers: Vec<u16> = log.iter().filter(|x| x.0 == per(0)).map(|x| x.1).collect();
    // the row tick, then effect ticks 1..7: counts 1..7 write at 2, 4, 6
    assert_eq!(&pers[..4], &[428, PERIODS[13 + 4], PERIODS[13 + 7], PERIODS[13]]);
}

#[test]
fn slide_and_portamento() {
    // slide by -3 each effect tick
    let p = vec![row(13, 1, 2, 0, 0xFD, 0)];
    let d = song(4, 0, 1, &plain_tracks(), &[p, vec![]], b"df0:");
    let (log, _) = play("slide", d, true, 4).unwrap();
    let pers: Vec<u16> = log.iter().filter(|x| x.0 == per(0)).map(|x| x.1).collect();
    assert_eq!(&pers[..4], &[428, 425, 422, 419]);
    // portamento from note 13 (428) to the next row's note 25 (214), duration 2 rows at speed 3
    let p = vec![row(13, 1, 3, 0, 2, 0), note(25, 0)];
    let d = song(3, 0, 1, &plain_tracks(), &[p, vec![]], b"df0:");
    let (log, _) = play("porta", d, true, 8).unwrap();
    let pers: Vec<u16> = log.iter().filter(|x| x.0 == per(0)).map(|x| x.1).collect();
    // pdur = 2 * 3 = 6; delta = 214 - 428 = -214: step k writes 428 + trunc(-214 k / 6)
    let want: Vec<u16> = std::iter::once(428).chain((1..=2).map(|k: i32| (428 + (-214 * k) / 6) as u16)).collect();
    assert_eq!(&pers[..3], &want[..]);
}

#[test]
fn note_off_and_legato() {
    // note 63 switches the voice off and writes nothing else; a legato row with the same
    // instrument does not restart
    let p = vec![note(13, 1), 0, row(0x3F, 0, 0, 0, 0, 0), 0, row(14, 1, 0, 0, 0, 1), row(15, 1, 0, 0, 0, 1)];
    let d = song(1, 0, 1, &plain_tracks(), &[p, vec![]], b"df0:");
    let (log, _) = play("legato", d, true, 6).unwrap();
    let t = split(&log);
    // tick 2 (note off): DMACON 1 and no period
    let off: Vec<_> = t[2].iter().filter(|x| x.0 == DMACON || x.0 == per(0)).collect();
    assert_eq!(off.len(), 1);
    assert_eq!(off[0].1, 1);
    // tick 4 (first legato row: last is 0, so it restarts), tick 5 (same instrument: no DMACON)
    assert!(t[4].iter().any(|x| x.0 == DMACON && x.1 == 1));
    assert!(!t[5].iter().any(|x| x.0 == DMACON));
    assert!(t[5].iter().any(|x| x.0 == per(0) && x.1 == PERIODS[15]));
}

#[test]
fn song_end_and_wrap() {
    // two positions, speed 1: 32 rows each; the song end is signalled when the position reaches 2
    let tr = vec![[(0, 0, 0, 0); 4], [(0, 0, 0, 0); 4]];
    let d = song(1, 0, 2, &tr, &[vec![note(13, 1)], vec![]], b"df0:");
    let (_, end) = play("end", d.clone(), true, 40).unwrap();
    assert!(!end);
    let (log, end) = play("end2", d, true, 70).unwrap();
    assert!(end);
    // the song restarts at position 0: a row tick's DMACON appears again
    assert!(log.iter().filter(|x| x.0 == DMACON && x.1 == 1).count() > 2);
}

#[test]
fn iff_samples_use_the_fixed_header() {
    // a FORM file: the data start at offset 104, the length is the word at 100 divided by two
    let mut f = b"FORM".to_vec();
    f.resize(100, 0);
    f.extend(32u32.to_be_bytes());
    f.extend((0..32u8).map(|x| x + 1));
    let d = song(6, 0, 1, &plain_tracks(), &[vec![note(13, 1)], vec![]], b"df0:");
    let dir = Dir::new("iff", &[("s1", f)]);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(&dir.0.join("sng.test"), d).unwrap();
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..3 {
        e.render(&mut out);
    }
    let log = e.m.bus.write_log.take().unwrap();
    let l = log.iter().find(|x| x.0 == len(0)).unwrap();
    // the slot's length of 8 words is replaced by 32 / 2
    assert_eq!(l.1, 16);
    let addr = {
        let hi = log.iter().find(|x| x.0 == lc(0)).unwrap().1 as u32;
        let lo = log.iter().find(|x| x.0 == lc(0) + 2).unwrap().1 as u32;
        hi << 16 | lo
    };
    // the first word is cleared, the rest is the body
    assert_eq!((0..4).map(|i| e.m.bus.r8(addr + i)).collect::<Vec<u8>>(), vec![0, 0, 3, 4]);
}

#[test]
fn deterministic_and_bounded_on_garbage() {
    let p = vec![row(13, 1, 3, 0, 4, 0), note(40, 15), row(0x3F, 0, 1, 0x7F, 0xFF, 1), row(37, 9, 2, 3, 0xFF, 0)];
    let d = song(0, 250, 3, &plain_tracks(), &[p, vec![]], b"df0:");
    let a = play("det1", d.clone(), true, 10).unwrap().0;
    let b = play("det2", d, true, 10).unwrap().0;
    assert_eq!(a.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(), b.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>());
    // random bytes with a valid trailer must not panic
    let mut seed = 12345u32;
    for n in 0..30 {
        let mut d: Vec<u8> = (0..1500)
            .map(|_| {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                (seed >> 16) as u8
            })
            .collect();
        d[0] = 0;
        d[1] = 0;
        let e = 869 + 16 + 128;
        d[e..e + 4].copy_from_slice(b"df0:");
        for s in 0..16 {
            d[5 + 54 * s + 4] = 0;
        }
        let _ = play(&format!("fuzz{n}"), d, false, 12);
    }
}
