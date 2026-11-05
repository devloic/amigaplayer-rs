use super::*;
use crate::engine::Engine;
use std::path::Path;

fn p16(v: &mut [u8], o: usize, x: u16) {
    v[o..o + 2].copy_from_slice(&x.to_be_bytes());
}

fn p32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

/// A song file: division and the tracks' event bytes.
fn song(format: u16, division: u16, tracks: &[&[u8]]) -> Vec<u8> {
    let mut d = b"MThd".to_vec();
    d.extend_from_slice(&6u32.to_be_bytes());
    d.extend_from_slice(&format.to_be_bytes());
    d.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    d.extend_from_slice(&division.to_be_bytes());
    for t in tracks {
        d.extend_from_slice(b"MTrk");
        d.extend_from_slice(&(t.len() as u32).to_be_bytes());
        d.extend_from_slice(t);
    }
    d
}

/// A bank of one program with two regions: notes up to 59 (base 48) play the sample at 64, the
/// others (base 60) the sample at 80; both are 16 bytes long.
fn bank() -> Vec<u8> {
    let mut d = vec![0u8; 96];
    d[0..4].copy_from_slice(b"BNKS");
    p32(&mut d, 4, 1);
    p32(&mut d, 8, 12);
    p16(&mut d, 12, 2);
    p32(&mut d, 14, (-32i32) as u32);
    d[18] = 48;
    d[20] = 59;
    p32(&mut d, 22, (-40i32) as u32);
    d[26] = 60;
    d[28] = 127;
    p32(&mut d, 32, (-64i32) as u32);
    p32(&mut d, 36, 16);
    p32(&mut d, 40, (-80i32) as u32);
    p32(&mut d, 44, 16);
    d
}

/// One note: channel 0, note 60, velocity 100, off after `len` MIDI ticks, then the end.
fn one_note() -> Vec<u8> {
    song(
        0,
        96,
        &[&[0x00, 0xC0, 0x00, 0x00, 0x90, 60, 100, 8, 0x80, 60, 0, 0x10, 0xFF, 0x2F, 0x00]],
    )
}

fn play(path: &str, d: Vec<u8>, bank: Option<(&str, Vec<u8>)>, secs: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let dir = Path::new("/ml-test");
    let mut e = Engine::new(48000.0);
    if let Some((n, c)) = bank {
        e.m.os.mem_files.insert(dir.join(n), c);
    }
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if e.play_data(&dir.join(path), d).is_err() {
        return (Vec::new(), false);
    }
    let mut out = vec![0f32; 2 * 48000 * secs];
    e.render(&mut out);
    let ended = e.songend();
    (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1, w.2)).collect(), ended)
}

#[test]
fn detection() {
    let d = one_note();
    assert!(detect(&d));
    for (o, v) in [(0usize, b'X'), (7, 7), (9, 2), (11, 0), (13, 0), (14, b'X')] {
        let mut e = d.clone();
        e[o] = v;
        if o == 11 || o == 13 {
            e[o - 1] = 0;
        }
        assert!(!detect(&e), "byte {o}");
    }
    assert!(!detect(&d[..17]));
    assert!(!detect(&[]));
    assert!(!detect(&bank()));
}

#[test]
fn companion_names() {
    assert_eq!(companion("MIDI.entity").as_deref(), Some("SMPL.entity"));
    assert_eq!(companion("midi.Entity").as_deref(), Some("SMPL.Entity"));
    assert_eq!(companion("entity.mid").as_deref(), Some("entity.BSP"));
    assert_eq!(companion("ENTITY.MID").as_deref(), Some("ENTITY.BSP"));
    assert_eq!(companion("entity.midi"), None);
    assert_eq!(companion("x"), None);
}

#[test]
fn bank_rules() {
    let b = Bank::parse(&bank()).unwrap();
    assert_eq!(b.programs.len(), 1);
    assert_eq!(b.programs[0].len(), 2);
    assert_eq!(b.header(40), (80, 16));
    // 60 + one program 54 + two regions (39 + 51) + two headers 30
    assert_eq!(b.cost, 60 + 54 + 2 * 90 + 2 * 30);
    let mut e = bank();
    e[0] = b'X';
    assert!(Bank::parse(&e).is_none(), "magic");
    let mut e = bank();
    p16(&mut e, 12, 0);
    assert!(Bank::parse(&e).is_none(), "no regions");
    let mut e = bank();
    p32(&mut e, 44, 100);
    assert!(Bank::parse(&e).is_none(), "samples past the file");
    let mut e = bank();
    p32(&mut e, 14, 32);
    assert!(Bank::parse(&e).is_none(), "positive reference");
    assert!(Bank::parse(&bank()[..40]).is_none());
}

#[test]
fn volume_curve() {
    // spec 4.5: 2 * k, k = i below 16, 16 + (i - 16) / 2 below 32, 24 + (i - 32) / 4 above
    assert_eq!(MidiLoriciel::volume(0), 0);
    assert_eq!(MidiLoriciel::volume(30), 30);
    assert_eq!(MidiLoriciel::volume(32), 32);
    assert_eq!(MidiLoriciel::volume(64), 48);
    assert_eq!(MidiLoriciel::volume(100), 56);
    assert_eq!(MidiLoriciel::volume(127), 62);
}

#[test]
fn match_costs() {
    // a candidate costs 58, a hit 8 more, a miss 18 more, the end of the list 21
    assert_eq!(mcost(&[0xF0, 0xFF], 0xF0), 66);
    assert_eq!(mcost(&[0xF0, 0xFF], 0xFF), 76 + 66);
    assert_eq!(mcost(&[0xF0, 0xFF], 0xF1), 76 * 2 + 21);
    assert_eq!(vcost(1), 53 + 12);
    assert_eq!(vcost(2), 106 + 44 + 12);
}

#[test]
fn plays_a_note_and_the_silence_loop() {
    let (w, ended) = play("MIDI.t", one_note(), Some(("SMPL.t", bank())), 2);
    assert!(!w.is_empty(), "the pair is taken");
    // the start-up: four zero volumes, again, then the reset writes
    assert_eq!(w[0].0, 0xDFF0A8);
    assert_eq!(w[8].0, 0xDFF0A8);
    assert_eq!((w[12].0, w[12].1), (0xDFF096, 0x000F));
    assert_eq!((w[13].0, w[13].1), (0xDFF09E, 0x00FF));
    // the note: LC (high, low), LEN 8 words, PER 373 (note 60 - base 60), VOL 56, DMACON
    let i = w.iter().position(|x| x.0 == 0xDFF096 && x.1 == 0x8001).unwrap();
    assert_eq!(w[i - 1], (0xDFF0A8, 56, w[i - 1].2));
    assert_eq!((w[i - 2].0, w[i - 2].1), (0xDFF0A6, 373));
    assert_eq!((w[i - 3].0, w[i - 3].1), (0xDFF0A4, 8));
    let lc = ((w[i - 5].1 as u32) << 16) | w[i - 4].1 as u32;
    assert_eq!((w[i - 5].0, w[i - 4].0), (0xDFF0A0, 0xDFF0A2));
    assert_eq!(w[i - 5].2, w[i - 4].2, "LC high and low share a clock");
    // the tail's silence block is the bank's start; the second region's sample is 80 further on
    let silence_lc = w[i + 1..].iter().find(|x| x.0 == 0xDFF0A2).unwrap().1 as u32;
    assert_eq!(lc & 0xFFFF, (silence_lc + 80) & 0xFFFF);
    // the note-off after 8 MIDI ticks (2 player ticks) switches the voice off, once
    let offs: Vec<_> = w.iter().filter(|x| x.0 == 0xDFF096 && x.1 == 0x0001).collect();
    assert!(!offs.is_empty());
    // the song ended and started again
    assert!(ended);
    assert!(w.iter().filter(|x| x.0 == 0xDFF096 && x.1 == 0x8001).count() >= 2);
}

#[test]
fn tick_spacing_follows_the_tempo() {
    // a note that is never released keeps its voice's tail writes going every tick
    // 500000 us per quarter at division 96: latch 14916, 74585 colour clocks
    let s = song(0, 96, &[&[0x00, 0x90, 60, 100, 0x7F, 0xFF, 0x2F, 0x00]]);
    let (w, _) = play("MIDI.t", s, Some(("SMPL.t", bank())), 1);
    let lens: Vec<u64> = w.iter().filter(|x| x.0 == 0xDFF0A4 && x.1 == 2).map(|x| x.2).collect();
    assert!(lens.windows(2).any(|p| p[1] - p[0] == 74_585), "{lens:?}");
    // a faster tempo (120000 us: latch 3580) from the tick after next
    let s = song(0, 96, &[&[0x00, 0xFF, 0x51, 0x03, 0x01, 0xD4, 0xC0, 0x00, 0x90, 60, 100, 0x7F, 0xFF, 0x2F, 0x00]]);
    let (w, _) = play("MIDI.t", s, Some(("SMPL.t", bank())), 1);
    let lens: Vec<u64> = w.iter().filter(|x| x.0 == 0xDFF0A4 && x.1 == 2).map(|x| x.2).collect();
    assert!(lens.windows(2).any(|p| p[1] - p[0] == 17_905), "{lens:?}");
}

#[test]
fn voices_are_stolen_and_released_by_channel() {
    // five simultaneous notes on channel 0 use voices 0..3, then 3 (the highest owned by the channel)
    let mut t = vec![0x00, 0xC0, 0x00];
    for n in [60u8, 62, 64, 65, 67] {
        t.extend_from_slice(&[0x00, 0x90, n, 100]);
    }
    t.extend_from_slice(&[0x40, 0xFF, 0x2F, 0x00]);
    let (w, _) = play("MIDI.t", song(0, 96, &[&t]), Some(("SMPL.t", bank())), 1);
    let on: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF096 && x.1 & 0x8000 != 0).map(|x| x.1).take(5).collect();
    assert_eq!(on, [0x8001, 0x8002, 0x8004, 0x8008, 0x8008]);
}

#[test]
fn companion_is_needed() {
    let (w, _) = play("MIDI.t", one_note(), None, 1);
    assert!(w.is_empty());
    let (w, _) = play("other.name", one_note(), Some(("SMPL.t", bank())), 1);
    assert!(w.is_empty());
    let mut b = bank();
    b[0] = b'X';
    let (w, _) = play("MIDI.t", one_note(), Some(("SMPL.t", b)), 1);
    assert!(w.is_empty());
    // the .mid / .BSP spelling
    let (w, _) = play("t.mid", one_note(), Some(("t.BSP", bank())), 1);
    assert!(!w.is_empty());
    // a song whose track is cut short is refused
    let d = one_note();
    let (w, _) = play("MIDI.t", d[..d.len() - 3].to_vec(), Some(("SMPL.t", bank())), 1);
    assert!(w.is_empty());
}

#[test]
fn deterministic() {
    let a = play("MIDI.t", one_note(), Some(("SMPL.t", bank())), 1);
    let b = play("MIDI.t", one_note(), Some(("SMPL.t", bank())), 1);
    assert_eq!(a, b);
}

#[test]
fn damaged_songs_do_not_panic_or_hang() {
    let base = one_note();
    let mut seed = 12345u32;
    for k in 0..40 {
        let mut d = base.clone();
        for _ in 0..1 + k % 4 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let o = 18 + (seed >> 8) as usize % (d.len() - 18);
            d[o] = (seed >> 20) as u8;
        }
        // a track that never ends, and one that runs off the file
        if k % 7 == 0 {
            let n = d.len();
            d[n - 3] = 0x00;
        }
        let _ = play("MIDI.t", d, Some(("SMPL.t", bank())), 1);
    }
    // tempo events with a zero divisor are ignored
    let s = song(0, 96, &[&[0x00, 0xFF, 0x51, 0x03, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x2F, 0x00]]);
    let _ = play("MIDI.t", s, Some(("SMPL.t", bank())), 1);
    // program changes to a program that does not exist, notes far from the base
    let s = song(0, 96, &[&[0x00, 0xC0, 0x7F, 0x00, 0x90, 0x7F, 0x7F, 0x00, 0x90, 0x00, 0x01, 0x00, 0xFF, 0x2F, 0x00]]);
    let (w, _) = play("MIDI.t", s, Some(("SMPL.t", bank())), 1);
    assert!(w.iter().any(|x| x.0 == 0xDFF096 && x.1 == 0x8001));
}
