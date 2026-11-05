use super::*;
use crate::engine::Engine;
use std::path::Path;

fn put16(h: &mut [u8], o: usize, v: u16) {
    h[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(h: &mut [u8], o: usize, v: u32) {
    h[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A module of the given layout: the Wanted Team header, the period table, the global
/// transposition and the family's own initial values; everything else zero. The streams
/// are filled in by the caller (`h` is the code hunk).
fn blank(l: &Layout, size: usize, songs: u32) -> Vec<u8> {
    let mut h = vec![0u8; size];
    put32(&mut h, 0, 0x70FF_4E75);
    h[4..12].copy_from_slice(b"!SOPROL!");
    put32(&mut h, 0x0C, 0x100);
    put32(&mut h, 0x10, 0x104);
    put32(&mut h, 0x14, songs);
    put32(&mut h, 0x18, 0x38);
    h[0x38..0x38 + 5].copy_from_slice(b"Test\0");
    put32(&mut h, 0x34, l.timer);
    for (i, p) in PERIODS.iter().enumerate() {
        put16(&mut h, l.period + 2 * i, *p);
    }
    h[l.transp] = l.transp_val;
    if l.fam == Fam::F1 {
        put32(&mut h, l.sbase, l.sbase_val);
    }
    h
}

/// The file around a code hunk.
fn file(h: &[u8]) -> Vec<u8> {
    let n = (h.len() / 4) as u32;
    let mut d = vec![0u8; 0x20];
    put32(&mut d, 0, 0x3F3);
    put32(&mut d, 8, 1);
    put32(&mut d, 0x14, 0x4000_0000 | n);
    put32(&mut d, 0x18, 0x3E9);
    put32(&mut d, 0x1C, n);
    d.extend_from_slice(h);
    d.extend_from_slice(&0x3F2u32.to_be_bytes());
    d
}

/// Makes every voice of a layout loop on a jump to itself, except the streams `skip` lists.
fn idle(h: &mut [u8], l: &Layout, song: usize, skip: &[usize]) {
    for v in 0..4 {
        if skip.contains(&v) {
            continue;
        }
        let p = l.starts[song][v] as usize;
        h[p] = 0xF3;
        put32(h, p + 1, p as u32);
    }
}

type Log = Vec<(u32, u16, u64)>;

fn play(d: Vec<u8>, ticks: usize, sub: u32) -> Option<Log> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/spl-test/spl.test"), d).ok()?;
    if e.playing.as_ref()?.format != "Sound Programming Language" {
        return None;
    }
    for _ in 0..sub {
        e.change_subsong(1).ok()?;
    }
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..ticks {
        e.render(&mut out);
    }
    e.m.bus.write_log.take()
}

fn writes(l: &Log, lo: u32, hi: u32) -> Vec<(u32, u16)> {
    l.iter().filter(|w| w.0 >= lo && w.0 < hi).map(|w| (w.0, w.1)).collect()
}

const AUD: u32 = 0xDFF0A0;

#[test]
fn detects_known_layouts_only() {
    let l = &LAYOUTS[3];
    let h = blank(l, 0x1000, 2);
    assert!(detect(&file(&h)));
    // not a hunk file
    let mut d = file(&h);
    d[3] = 0;
    assert!(!detect(&d));
    // the tag
    let mut d = file(&h);
    d[0x24] = b'?';
    assert!(!detect(&d));
    // a header field that must not be zero
    let mut h2 = h.clone();
    put32(&mut h2, 0x10, 0);
    assert!(!detect(&file(&h2)));
    // the period table is where the layout puts it
    let mut h3 = h.clone();
    put16(&mut h3, l.period + 4, 0x1234);
    assert!(!detect(&file(&h3)));
    // too short for the tables
    assert!(!detect(&file(&h[..0x300])));
    assert!(!detect(&file(&h)[..0x30]));
}

#[test]
fn every_layout_is_recognised_from_its_own_data() {
    for (i, l) in LAYOUTS.iter().enumerate() {
        let h = blank(l, 0x2000, l.starts.len() as u32);
        let found = layout(&h).expect("layout found");
        assert_eq!((found.recs, found.period, found.timer, found.gm), (l.recs, l.period, l.timer, l.gm), "layout {i}");
    }
}

#[test]
fn parses_title_and_subsongs() {
    let l = &LAYOUTS[3];
    let h = blank(l, 0x1000, 2);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("/spl-test/spl.test"), file(&h)).unwrap();
    let p = e.playing.as_ref().unwrap();
    assert_eq!(p.format, "Sound Programming Language");
    assert_eq!(p.module_name, "Test");
    assert_eq!((p.sub_min, p.sub_max), (0, 1));
}

#[test]
fn transposition_skips_the_thirteenth_semitone() {
    assert_eq!(Spl::transpose(0x05, 0), 0x05);
    assert_eq!(Spl::transpose(0x0B, 1), 0x10);
    assert_eq!(Spl::transpose(0x0A, 3), 0x11);
    assert_eq!(Spl::transpose(0x00, 12), 0x10);
    assert_eq!(Spl::transpose(0x1B, 2), 0x21);
}

/// Family 3: an instrument load, a note on voice 0; the other voices idle.
fn hp_module() -> Vec<u8> {
    let l = &LAYOUTS[3];
    let mut h = blank(l, 0x1000, 2);
    idle(&mut h, l, 0, &[0]);
    idle(&mut h, l, 1, &[0]);
    let p = l.starts[0][0] as usize;
    h[p] = 0xF0;
    put32(&mut h, p + 1, 0x400); // sample start
    put16(&mut h, p + 5, 0x20); // length in words
    h[p + 9] = 2; // octave offset
    h[p + 16] = 0x7F; // volume (record byte 0x0F)
    h[p + 17] = 0xC1; // duration 1
    h[p + 18] = 0x00; // a note
    h[p + 19] = 0xF3;
    put32(&mut h, p + 20, p as u32 + 17);
    file(&h)
}

#[test]
fn family3_note_on_writes() {
    let log = play(hp_module(), 14, 0).unwrap();
    let w = writes(&log, 0xDFF096, 0xDFF0E0);
    // the first note-on after the start writes: LEN=1 PER=1, DMACON off, then the sample
    let i = w.iter().position(|x| *x == (AUD + 4, 1)).unwrap();
    // the note 0 transposed by 5 steps: semitone 5, octave 0+2: 0xA000 >> 2
    let per = 0xA000u16 >> 2;
    assert_eq!(w[i + 1], (AUD + 6, 1));
    assert_eq!(w[i + 2], (0xDFF096, 0x0001));
    assert_eq!(w[i + 3], (AUD + 6, per));
    assert_eq!(w[i + 4].0, AUD);
    assert_eq!(w[i + 5].0, AUD + 2);
    assert_eq!(w[i + 6], (AUD + 4, 0x20));
    // the byte volume 0x7F is logged masked to 7 bits
    assert_eq!(w[i + 7], (AUD + 8, 0x7F));
    assert_eq!(w[i + 8], (0xDFF096, 0x8001));
    // the instrument stays loaded: the next note (after the duration) has the same writes
    let j = w[i + 9..].iter().position(|x| *x == (AUD + 4, 1)).unwrap() + i + 9;
    assert_eq!(w[j + 3], (AUD + 6, per));
}

#[test]
fn raster_wait_follows_the_note_start() {
    let log = play(hp_module(), 14, 0).unwrap();
    let i = log.iter().position(|x| x.0 == AUD + 4 && x.1 == 1).unwrap();
    // DMACON off follows PER=1 after nine raster lines: 41 + 9 * 227 colour clocks at least,
    // and not more than ten lines plus the post time
    let gap = log[i + 2].2 - log[i + 1].2;
    assert!((1900..2700).contains(&gap), "gap {gap}");
    assert_eq!(log[i + 1].2, log[i].2);
}

#[test]
fn deterministic() {
    let a = play(hp_module(), 14, 0).unwrap();
    let b = play(hp_module(), 14, 0).unwrap();
    assert!(a.len() > 20);
    assert_eq!(a, b);
}

#[test]
fn second_subsong_repeats_the_start() {
    let log = play(hp_module(), 14, 1).unwrap();
    // the host's DMACON 0xF and volume clears precede the second start's preset
    let dma_f = log.iter().filter(|x| x.0 == 0xDFF096 && x.1 == 0x000F).count();
    assert!(dma_f >= 1);
    let presets = log.iter().filter(|x| x.0 == 0xDFF09E && x.1 == 0x0088).count();
    assert_eq!(presets, 2);
}

/// Family 1: the parser runs every eighth tick pair 0 and 5.
#[test]
fn family1_tempo_cycle() {
    let l = &LAYOUTS[0];
    let mut h = blank(l, 0x1500, 1);
    idle(&mut h, l, 0, &[0]);
    let p = l.starts[0][0] as usize;
    h[p] = 0xF0;
    // the record: sample 0, length 8 words, flags 0, volume 0x40
    put16(&mut h, p + 1 + 4, 8);
    put16(&mut h, p + 1 + 0x1E, 0x40);
    h[p + 0x23] = 0xC0; // duration 0: a note at every step
    for k in 0..40 {
        h[p + 0x24 + k] = 0x03;
    }
    // a transposition of 9 would take long; the global byte is part of the layout
    let log = play(file(&h), 160, 0).unwrap();
    let starts: Vec<u64> = log.iter().filter(|x| x.0 == AUD + 4 && x.1 == 1).map(|x| x.2).collect();
    assert!(starts.len() >= 6, "{} note-ons", starts.len());
    let period = (l.timer as u64 + 1) * 5;
    for k in 1..6 {
        // 5 ticks, 3 ticks, 5 ticks ...
        let d = starts[k] - starts[k - 1];
        let n = if k % 2 == 1 { 5 } else { 3 };
        assert!(d > n * period - 3000 && d < n * period + 3000, "step {k}: {d} for {n} ticks");
    }
}

/// Family 2, Clever & Smart's call stack: the third nested call on voice 3 writes into the
/// length table of the samples.
#[test]
fn family2_stack_overflow_lands_in_the_length_table() {
    let l = &LAYOUTS[1];
    let mut h = blank(l, 0x1400, 1);
    for v in 0..3 {
        let p = l.starts[0][v] as usize;
        h[p] = 0xF3;
        put32(&mut h, p + 1, p as u32);
    }
    let p = l.starts[0][3] as usize;
    // three nested calls, then the instrument load and a note
    h[p] = 0xF1;
    put32(&mut h, p + 1, 0x1000);
    h[0x1000] = 0xF1;
    put32(&mut h, 0x1001, 0x1010);
    h[0x1010] = 0xF1;
    put32(&mut h, 0x1011, 0x1020);
    h[0x1020] = 0xF0;
    put16(&mut h, 0x1021, 1); // sample number 1
    put16(&mut h, 0x1021 + 0x16, 0x40);
    h[0x1021 + 0x1A] = 0x05;
    h[0x1021 + 0x1B] = 0xF3;
    put32(&mut h, 0x1021 + 0x1C, 0x1021 + 0x1B);
    put32(&mut h, l.offs + 4, 0x1100); // sample 1 starts at 0x1100
    let log = play(file(&h), 3, 0).unwrap();
    // the third push stores BASE + 0x1011 at the table: entry 1 holds its low word, which
    // the note then uses as the sample's length (BASE of the reference run)
    let pushed = l.ref_base + 0x1011;
    let len = (pushed & 0xFFFF) as u16 / 2;
    let w = writes(&log, 0xDFF0D0, 0xDFF0E0);
    assert!(w.contains(&(0xDFF0D4, len)), "{w:x?} want {len:x}");
}

/// Family 2: a note byte 0 starts nothing.
#[test]
fn family2_zero_note_writes_nothing() {
    let l = &LAYOUTS[1];
    let mut h = blank(l, 0x1400, 1);
    idle(&mut h, l, 0, &[0]);
    let p = l.starts[0][0] as usize;
    h[p] = 0xF0;
    h[p + 1 + 0x1A] = 0x00;
    h[p + 1 + 0x1B] = 0xF3;
    put32(&mut h, p + 1 + 0x1C, p as u32 + 1 + 0x1B);
    let log = play(file(&h), 4, 0).unwrap();
    assert!(!log.iter().any(|x| x.0 == AUD + 4 && x.1 == 1));
}
