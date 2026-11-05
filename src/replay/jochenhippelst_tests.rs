//! Tests for the Jochen Hippel ST open replayer. The modules are built here from the layout
//! alone (no game data): a small image with one pitch program, one volume program, one pattern.

use super::*;
use crate::engine::Engine;
use std::path::Path;

fn put32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

fn put16(v: &mut [u8], o: usize, x: u16) {
    v[o..o + 2].copy_from_slice(&x.to_be_bytes());
}

/// An image with the pitch, instrument and pattern tables each followed by their data, then the
/// position list, one subsong and the (empty) extra table.
fn build(tag: &[u8; 4], fs: &[u8], inst: &[u8], pat: &[u8], speed: u16) -> Vec<u8> {
    let mut h = vec![0u8; 0x40];
    h[..4].copy_from_slice(b"COSO");
    h[0x20..0x24].copy_from_slice(tag);
    put16(&mut h, 0x24, 0);
    put16(&mut h, 0x26, 0);
    put16(&mut h, 0x30, 1);
    put16(&mut h, 0x32, 0);
    let mut body: Vec<u8> = Vec::new();
    let mut offs = Vec::new();
    for blk in [fs, inst, pat] {
        let off = 0x40 + body.len();
        offs.push(off);
        let data_at = off + 2;
        body.extend_from_slice(&(data_at as u16).to_be_bytes());
        body.extend_from_slice(blk);
        if body.len() & 1 == 1 {
            body.push(0);
        }
    }
    let t_pos = 0x40 + body.len();
    // one position: pattern 0 on every voice, no transpose, no flags
    body.extend_from_slice(&[0; 12]);
    let t_sub = 0x40 + body.len();
    body.extend_from_slice(&[0, 0, 0, 0, (speed >> 8) as u8, speed as u8]);
    let t_x = 0x40 + body.len();
    body.extend_from_slice(&[0; 6]);
    put32(&mut h, 4, offs[0] as u32);
    put32(&mut h, 8, offs[1] as u32);
    put32(&mut h, 12, offs[2] as u32);
    put32(&mut h, 16, t_pos as u32);
    put32(&mut h, 20, t_sub as u32);
    put32(&mut h, 24, t_x as u32);
    put32(&mut h, 28, (0x40 + body.len()) as u32);
    h.extend_from_slice(&body);
    h
}

fn small() -> Vec<u8> {
    // pitch program: a note, then hold; instrument: speed 1, pitch program 0, then a volume and hold
    build(b"TFMX", &[0x10, 0xE1], &[1, 0, 0, 0, 0, 0x10, 0xE1], &[0x10, 0x00, 0xFF], 3)
}

fn run(d: &[u8], ticks: usize) -> Option<Vec<(u32, u16, u64)>> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("test.hst"), d.to_vec()).ok()?;
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..ticks {
        e.render(&mut buf);
    }
    e.m.bus.write_log.clone()
}

#[test]
fn tables_have_their_sizes() {
    assert_eq!(VOL_TABLE.len(), 52);
    assert_eq!(YM_PERIODS.len(), 96);
}

#[test]
fn recognises_a_coso_image_with_the_tag() {
    assert!(detect(&small()));
    let mut d = small();
    d[0x20..0x24].copy_from_slice(b"MMME");
    assert!(detect(&d));
}

#[test]
fn refuses_other_data() {
    assert!(!detect(&[]));
    assert!(!detect(&[0u8; 200]));
    assert!(!detect(b"COSO"));
    let mut d = small();
    d[0x20..0x24].copy_from_slice(b"XXXX");
    assert!(!detect(&d));
}

#[test]
fn truncated_and_damaged_images_do_not_panic() {
    let d = small();
    for n in 0..d.len() {
        let _ = detect(&d[..n]);
        let _ = parse(&d[..n]);
    }
    for i in 0..d.len() {
        for x in [0u8, 0xFF, 0x80] {
            let mut e = d.clone();
            e[i] = x;
            let _ = parse(&e);
        }
    }
}

#[test]
fn damaged_images_play_without_panic() {
    let d = small();
    for i in (0x24..d.len()).step_by(3) {
        let mut e = d.clone();
        e[i] = 0xFF;
        let _ = run(&e, 20);
    }
}

#[test]
fn plays_and_writes_channel_registers() {
    let l = run(&small(), 8).expect("module plays");
    assert!(l.iter().any(|w| (0xDFF0A0..0xDFF0E0).contains(&w.0)), "no channel register written");
    assert!(l.iter().any(|w| w.0 == 0xDFF096), "DMACON never written: {:x?}", &l[..l.len().min(12)]);
}

#[test]
fn runs_are_deterministic() {
    let a = run(&small(), 12).unwrap();
    let b = run(&small(), 12).unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_tag_far_into_the_data_is_not_found() {
    let mut d = vec![0u8; 4096];
    d[3000..3004].copy_from_slice(b"TFMX");
    assert!(!detect(&d));
}
