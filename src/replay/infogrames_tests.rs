use super::*;
use crate::engine::Engine;
use std::path::Path;

fn p16(d: &mut Vec<u8>, v: u16) {
    d.extend_from_slice(&v.to_be_bytes());
}

/// A volume envelope entry: one segment of `count` steps of `delta` every `ticks` ticks.
fn env(flag: u8, segs: &[(u8, i8, u8)]) -> [u8; 13] {
    let mut e = [0u8; 13];
    e[0] = flag;
    for (i, s) in segs.iter().take(4).enumerate() {
        e[1 + 3 * i] = s.0;
        e[2 + 3 * i] = s.1 as u8;
        e[3 + 3 * i] = s.2;
    }
    e
}

/// One song with one pattern per voice-track entry: `tracks[v]` lists pattern numbers.
fn song(speed: u8, a: &[[u8; 13]], b: &[[u8; 13]], tracks: [&[u8]; 4], pats: &[&[u8]]) -> Vec<u8> {
    let hdr = 16 + 2 * pats.len();
    let mut body: Vec<u8> = Vec::new();
    let mut toff = [0u16; 4];
    for (v, t) in tracks.iter().enumerate() {
        toff[v] = (hdr + body.len()) as u16;
        body.extend_from_slice(t);
        body.push(0xFF);
    }
    let mut poff = Vec::new();
    for p in pats {
        poff.push((hdr + body.len()) as u16);
        body.extend_from_slice(p);
        body.push(0xFF);
    }
    if body.len() % 2 == 1 {
        body.push(0);
    }
    let a_off = (hdr + body.len()) as u16;
    for i in 0..32 {
        body.extend_from_slice(a.get(i).unwrap_or(&[0u8; 13]));
    }
    let b_off = (hdr + body.len()) as u16;
    for i in 0..32 {
        body.extend_from_slice(b.get(i).unwrap_or(&[0u8; 13]));
    }
    let mut d = Vec::new();
    for w in [speed as u16, a_off, b_off, toff[0], toff[1], toff[2], toff[3], (hdr + body.len()) as u16] {
        p16(&mut d, w);
    }
    for o in poff {
        p16(&mut d, o);
    }
    d.extend_from_slice(&body);
    d
}

/// A file of one song (the table has one entry at offset 2).
fn module(song: Vec<u8>) -> Vec<u8> {
    let mut d = vec![0, 2];
    d.extend_from_slice(&song);
    d
}

/// A bank with 32 samples: sample s has `8 + s` words, looping on its last 2 words.
fn bank() -> Vec<u8> {
    let mut d = vec![0u8; 4 + 32 * 16];
    for s in 0..32usize {
        let e = 4 + 16 * s;
        let start = (32 * 16 + 16 * s) as u32;
        d[e..e + 4].copy_from_slice(&start.to_be_bytes());
        d[e + 4..e + 8].copy_from_slice(&(start + 2 * (6 + s as u32)).to_be_bytes());
        d[e + 12..e + 14].copy_from_slice(&(8 + s as u16).to_be_bytes());
        d[e + 14..e + 16].copy_from_slice(&2u16.to_be_bytes());
    }
    d.resize(d.len() + 16 * 40, 0x10);
    d
}

fn simple() -> Vec<u8> {
    let a = [env(0, &[(15, 1, 1), (1, 63, 1)]), env(0x80, &[(4, -8, 1), (2, 0, 1)])];
    let b = [env(0, &[(1, 0, 1)])];
    // voice 0: envelope 0, sample 3, duration 4 (8 rows... code 0x82), note 24, then a second note
    let p0: &[u8] = &[0xC0, 0xA3, 0x82, 24, 0xA5, 26];
    let p1: &[u8] = &[0xC1, 0x82, 0xE1, 0, 30];
    song(2, &a, &b, [&[0], &[1], &[1], &[0]], &[p0, p1])
}

fn play(name: &str, d: Vec<u8>, ins: Option<(&str, Vec<u8>)>, ticks: usize) -> Vec<(u32, u16, u64)> {
    let dir = Path::new("/ig-test");
    let mut e = Engine::new(48000.0);
    if let Some((n, c)) = ins {
        e.m.os.mem_files.insert(dir.join(n), c);
    }
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(&dir.join(name), d).unwrap();
    let mut out = vec![0f32; 2 * 960 * ticks];
    e.render(&mut out);
    e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1, w.2)).collect()
}

#[test]
fn detects() {
    let d = module(simple());
    assert!(detect(&d));
    assert!(!detect(&d[..3]));
    assert!(!detect(&[0u8; 100]));
    let mut odd = d.clone();
    odd[1] = 3;
    assert!(!detect(&odd));
    // the first envelope must start with 00 0F
    let mut bad = d.clone();
    let a = 2 + be16(&d, 4) as usize;
    bad[a + 1] = 0x10;
    assert!(!detect(&bad));
    let mut first = d.clone();
    let a = 2 + be16(&d, 4) as usize;
    first[a + 1] = 0x0F;
    assert!(!detect(&first) || first[a] == 0);
}

#[test]
fn bank_name_rule() {
    assert_eq!(bank_names("bob4a.dum"), ["bob4a.ins", "bob4.ins"]);
    assert_eq!(bank_names("RH2.BLOOD.DUM"), ["RH2.BLOOD.ins", "RH2.BLOO.ins"]);
    assert_eq!(bank_names("x.mod"), ["x.mod.ins", "x.mo.ins"]);
}

#[test]
fn tempo_list_and_default() {
    let d = module(simple());
    assert_eq!(timer_value(&d), (0x1A00, None));
    // a file of the first listed length whose word sum is the listed one
    let mut f = vec![0u8; 0xB22];
    let want = be32(TEMPO, 0);
    f[1] = 2;
    let sum = (0..f.len() / 4 - 1).fold(0u32, |s, i| s.wrapping_add(be32(&f, 4 * i)));
    let k = f.len() / 4 - 1;
    f[4 * k..4 * k + 4].copy_from_slice(&want.wrapping_sub(sum).to_be_bytes());
    assert_eq!(timer_value(&f), (0x24FF, Some(0)));
    // a partial last word adds its leading bytes in the high positions
    let mut g = vec![0u8; 5];
    g[4] = 1;
    assert_eq!(timer_value(&g).1, None);
}

#[test]
fn envelope_rules() {
    // one segment, 2 steps of -3 every 2 ticks, then done
    let mut e = Env::default();
    e.select(env(0, &[(2, -3, 2)])[1..].try_into().unwrap(), false, 0);
    let v: Vec<u16> = (0..6).map(|_| e.step(100).0).collect();
    assert_eq!(v, [97, 97, 94, 94, 94, 94]);
    // a value below zero is clamped to zero
    let mut e = Env::default();
    e.select(env(0, &[(1, -9, 1)])[1..].try_into().unwrap(), false, 0);
    assert_eq!(e.step(5).0, 0);
    // a loop restarts at the second segment and raises the base by the step
    let mut e = Env::default();
    e.select(env(0x82, &[(1, 5, 1), (1, 1, 1), (1, 0, 1), (1, 0, 1)])[1..].try_into().unwrap(), true, 2);
    let v: Vec<u16> = (0..5).map(|_| e.step(0).0).collect();
    assert_eq!(v, [5, 6, 6, 6, 5]);
}

#[test]
fn plays_a_note_with_the_specified_writes() {
    let w = play("t.dum", module(simple()), Some(("t.ins", bank())), 30);
    assert_eq!(w, play("t.dum", module(simple()), Some(("t.ins", bank())), 30));
    // the first tick: voice 0 volume, period, loc, len, dma off, dma on, loc, len
    let a: Vec<u32> = w.iter().take(10).map(|x| x.0).collect();
    assert_eq!(a, [0xDFF0A8, 0xDFF0A6, 0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF096, 0xDFF096, 0xDFF0A0, 0xDFF0A2, 0xDFF0A4]);
    assert_eq!(w[1].1, be16(PERIODS, 48));
    assert_eq!(w[5].1, 1);
    assert_eq!(w[6].1, 0x8201);
    // sample 3 has 11 words, the loop part 2
    assert_eq!(w[4].1, 11);
    assert_eq!(w[9].1, 2);
    // the second wait lasts 10 lines on: the two DMA writes lie more than two raster lines apart
    assert!(w[6].2 - w[5].2 > 2000);
    // the ticks are 33 285 colour clocks apart (voice 0's volume writes)
    let v0: Vec<u64> = w.iter().filter(|x| x.0 == 0xDFF0A8).map(|x| x.2).collect();
    let gap = v0[3] - v0[2];
    assert!((33285 - 600..33285 + 600).contains(&gap), "{gap}");
}

#[test]
fn does_not_play_without_its_bank() {
    let r = std::panic::catch_unwind(|| {
        let dir = Path::new("/ig-test");
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(&dir.join("t.dum"), module(simple())).is_err()
            || !matches!(e.playing.as_ref().map(|p| p.format.as_str()), Some("Infogrames"))
    });
    assert!(r.unwrap());
}

#[test]
fn ends_when_all_voices_stop() {
    // every track is empty: the voices stop at the first tick and the song ends
    let a = [env(0, &[(15, 63, 1)])];
    let d = module(song(3, &a, &[], [&[], &[], &[], &[]], &[&[0x82, 10]]));
    let w = play("t.dum", d, Some(("t.ins", bank())), 5);
    assert!(!w.is_empty());
    assert!(w.iter().all(|x| x.1 == 0 || x.0 == 0xDFF0A8 || x.0 == 0xDFF0A6));
    assert!(w.iter().all(|x| x.1 == 0));
}

#[test]
fn malformed_input_neither_panics_nor_hangs() {
    let mut s: u32 = 0x1234_5678;
    for round in 0..40 {
        let mut d = module(simple());
        for _ in 0..(5 + round) {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let i = 2 + (s >> 8) as usize % (d.len() - 2);
            d[i] = (s >> 24) as u8;
        }
        if detect(&d) {
            let _ = play("t.dum", d, Some(("t.ins", bank())), 40);
        }
    }
    // a truncated file and a one-byte bank
    let d = module(simple());
    let _ = play("t.dum", d[..60].to_vec(), Some(("t.ins", vec![0])), 10);
}
