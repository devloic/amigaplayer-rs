use super::*;
use crate::engine::Engine;
use std::path::Path;

// Synthetic modules built from the specification's layout (no game data).

const ORIGIN: u32 = 0x1000;
const TABLE: usize = 5000;
const REC: usize = 5100;
const ROWS: usize = 6000;

fn put32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

/// A module with one instrument whose record is at `REC`, every position pointing at one pattern of
/// rows (`rows`), the rest zero.
fn build(rows: &[[u8; 3]]) -> Vec<u8> {
    let mut d = vec![0u8; 8192];
    put32(&mut d, 0, ORIGIN + 64);
    for i in 1..5 {
        let p = be32(&d, 4 * i - 4);
        put32(&mut d, 4 * i, p + 1024);
    }
    for i in 5..8 {
        let p = be32(&d, 4 * i - 4);
        put32(&mut d, 4 * i, p + 256);
    }
    d[35] = 1;
    put32(&mut d, 36, ORIGIN + TABLE as u32);
    d[41] = 15;
    d[42] = 0x40;
    d[44] = 3;
    put32(&mut d, 46, ORIGIN);
    for i in 0..1024 {
        put32(&mut d, 64 + 4 * i, ORIGIN + ROWS as u32);
    }
    put32(&mut d, TABLE + 4, ORIGIN + REC as u32);
    // record: sample length 32 words, volume 64, looped
    d[REC..REC + 2].copy_from_slice(&32u16.to_be_bytes());
    d[REC + 2] = 64;
    d[REC + 10..REC + 12].copy_from_slice(&16u16.to_be_bytes());
    for (i, r) in rows.iter().enumerate() {
        d[ROWS + 3 * i..ROWS + 3 * i + 3].copy_from_slice(r);
    }
    d
}

fn run(d: &[u8], n: usize) -> Vec<(u32, u16, u64)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if e.play_data(Path::new("thm.test"), d.to_vec()).is_err() {
        return Vec::new();
    }
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..n {
        e.render(&mut buf);
    }
    e.m.bus.write_log.clone().unwrap_or_default()
}

#[test]
fn recognises_the_layout_and_nothing_else() {
    let d = build(&[]);
    assert!(detect(&d));
    assert!(!detect(&[]));
    assert!(!detect(&d[..6000]));
    let mut t = d.clone();
    put32(&mut t, 46, 0);
    assert!(!detect(&t));
    let mut t = d.clone();
    put32(&mut t, 46, ORIGIN + 1);
    assert!(!detect(&t));
    // a pattern pointer beyond the file
    let mut t = d.clone();
    put32(&mut t, 64, ORIGIN + 0x10_0000);
    assert!(!detect(&t));
    // a list that is not 1024 bytes after the previous one
    let mut t = d.clone();
    put32(&mut t, 8, ORIGIN + 3000);
    assert!(!detect(&t));
}

#[test]
fn tolerates_garbage_without_panicking() {
    let d = build(&[[0x10, 1, 0x00], [0xCF, 1, 0x80], [0xFF, 0xFF, 0xFF]]);
    let mut seed = 0x1234_5678u32;
    for _ in 0..40 {
        let mut t = d.clone();
        for _ in 0..24 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = (seed >> 8) as usize % t.len();
            // leave the identifying header alone so that detection still passes
            if o < 64 {
                continue;
            }
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            t[o] = (seed >> 16) as u8;
        }
        let _ = run(&t, 6);
    }
}

#[test]
fn render_is_deterministic_and_writes_registers() {
    let d = build(&[[0x10, 1, 0x00], [0x12, 1, 0x00], [0x14, 1, 0x00], [0x00, 0, 0x00]]);
    let a = run(&d, 40);
    let b = run(&d, 40);
    assert_eq!(a, b);
    assert!(a.iter().any(|w| w.0 == 0xDFF096), "no DMACON write");
    // times never decrease
    assert!(a.windows(2).all(|w| w[1].2 >= w[0].2));
}
