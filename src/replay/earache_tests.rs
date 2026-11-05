//! Tests on small synthetic modules built in code (no module files).
use super::*;
use crate::engine::Engine;
use std::path::Path;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// The data block (plain kind): four rows, an order list of one entry (rows 0..1, speed 2) and an
/// end marker that jumps to it, two instrument records, one volume table, one pitch table and a
/// sample table of one entry. `row0` is the first row's four channel words.
fn block(row0: [u16; 4]) -> Vec<u8> {
    let mut d = vec![0u8; 0x100];
    put32(&mut d, 0, 0x18);
    put32(&mut d, 4, 0x38);
    put32(&mut d, 8, 0x40);
    put32(&mut d, 12, 0xE0);
    put32(&mut d, 16, 0x60);
    put32(&mut d, 20, 0xA0);
    for (c, w) in row0.iter().enumerate() {
        put16(&mut d, 0x18 + 2 * c, *w);
    }
    put16(&mut d, 0x38, 0);
    put16(&mut d, 0x3A, 0x2001);
    put16(&mut d, 0x3C, 0xFFFF);
    put16(&mut d, 0x3E, 0);
    // instrument 0: sample 0, play once, volume table 0, pitch table 0, start volume 0x40
    d[0x40 + 9] = 0x40;
    // sample table entry 0: start 0xF0 (in 4-byte units), 4 words
    put16(&mut d, 0xE0, 0x3C);
    put16(&mut d, 0xE2, 4);
    d
}

fn easo(b: &[u8]) -> Vec<u8> {
    let mut d = b"EASO".to_vec();
    d.extend_from_slice(b);
    d
}

fn branch(b: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 20];
    d[0..2].copy_from_slice(&KEY[0..2]);
    put16(&mut d, 2, 6);
    d[4..6].copy_from_slice(&KEY[0..2]);
    put16(&mut d, 6, 2);
    d[8..18].copy_from_slice(&KEY[2..12]);
    put16(&mut d, 18, 2);
    // d3 = 2 puts the data start right after the word
    d.truncate(20);
    d.extend_from_slice(b);
    d
}

type Log = Vec<(u32, u16, u64)>;

fn play(d: Vec<u8>, ticks: usize) -> Option<(Log, bool)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/ea-test/ea.test"), d).ok()?;
    if e.playing.as_ref()?.format != "EarAche" {
        return None;
    }
    let mut out = vec![0f32; 2 * 960];
    let mut end = false;
    for _ in 0..=ticks {
        e.render(&mut out);
        end |= e.songend();
    }
    let log = e.m.bus.write_log.take().unwrap();
    Some((log, end))
}

#[test]
fn detects_the_three_kinds() {
    let b = block([0; 4]);
    assert!(detect(&b));
    assert_eq!(locate(&b).unwrap().kind, Kind::Plain);
    let e = easo(&b);
    assert!(detect(&e));
    assert_eq!(locate(&e).unwrap().kind, Kind::Easo);
    assert_eq!(locate(&e).unwrap().d, 4);
    let br = branch(&b);
    assert!(detect(&br));
    assert_eq!(locate(&br).unwrap().kind, Kind::Branch);
    assert_eq!(locate(&br).unwrap().d, 20);
}

#[test]
fn rejects_what_is_not_a_module() {
    assert!(!detect(&[]));
    assert!(!detect(&[0; 10]));
    assert!(!detect(&[0x55; 400]));
    let mut b = block([0; 4]);
    b[3] = 0x20; // h0 is not 0x18
    assert!(!detect(&b));
    let mut b = block([0; 4]);
    put32(&mut b, 4, 0x1C); // order list not a multiple of 8 after the rows
    assert!(!detect(&b));
    let mut b = block([0; 4]);
    put32(&mut b, 16, 0x48); // volume tables not a multiple of 16 after the instruments
    assert!(!detect(&b));
    let mut br = branch(&block([0; 4]));
    br[9] ^= 1; // key word differs
    assert!(!detect(&br));
    let mut br = branch(&block([0; 4]));
    br[3] = 5; // odd displacement
    assert!(!detect(&br));
    assert!(!detect(&block([0; 4])[..20]));
}

#[test]
fn note_start_writes_in_order() {
    // channel 0: note 24, instrument 0
    let (log, end) = play(block([24 << 8, 0, 0, 0]), 6).unwrap();
    let w: Vec<(u32, u16)> = log.iter().map(|x| (x.0, x.1)).collect();
    let i = w.iter().position(|&x| x == (0xDFF0A6, 0x78)).expect("first period write");
    let want = [
        (0xDFF0A6, 0x78),
        (0xDFF096, 0x0001),
        (0xDFF0A4, 4),
        (0xDFF0A0, 0),
        (0xDFF0A2, 0),
        (0xDFF0A8, 0x40),
        (0xDFF0A6, 0x38A),
        (0xDFF096, 0x8001),
        (0xDFF0A4, 1),
    ];
    for (k, &(a, v)) in want.iter().enumerate() {
        if a == 0xDFF0A0 || a == 0xDFF0A2 {
            assert_eq!(w[i + k].0, a);
            continue;
        }
        assert_eq!(w[i + k], (a, v), "write {k}");
    }
    // the address pair is the sample's: the module's load address + 4 * 0x3C
    let lc = w[i + 3].1 as u32 * 0x10000 + w[i + 4].1 as u32;
    assert!(lc >= 0xF0 && lc % 4 == 0);
    assert!(end, "the end marker is read on the third row tick");
}

#[test]
fn deterministic_and_bounded() {
    let b = block([24 << 8 | 0, 0, 30 << 8, 0]);
    let a = play(b.clone(), 40).unwrap();
    let c = play(b, 40).unwrap();
    assert_eq!(a.0, c.0);
    assert!(a.0.len() > 10);
}

#[test]
fn damaged_modules_do_not_panic() {
    let base = block([24 << 8, 5 << 8 | 0, 71 << 8, 255 << 8 | 255]);
    let mut seed = 0x1234_5678u32;
    for _ in 0..200 {
        let mut d = base.clone();
        for _ in 0..6 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = (seed >> 8) as usize % d.len();
            // keep the six header longs recognisable most of the time
            if o >= 24 || seed & 7 == 0 {
                d[o] = (seed >> 24) as u8;
            }
        }
        let _ = play(d, 30);
    }
}

#[test]
fn a_chain_of_end_markers_terminates() {
    let mut b = block([0; 4]);
    // the end marker at 0x3C jumps to itself: sixteen jumps, a skip, then the next entry
    put16(&mut b, 0x3E, 1);
    assert!(play(b, 8).is_some());
}
