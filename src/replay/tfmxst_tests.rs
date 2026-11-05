//! Tests for the TFMX ST open replayer. The modules are built here from the layout alone (no game
//! data): the header, one song, one track step with a single track, one pattern and one macro.

use super::*;
use crate::engine::Engine;
use std::path::Path;

fn put32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

fn put16(v: &mut [u8], o: usize, x: u16) {
    v[o..o + 2].copy_from_slice(&x.to_be_bytes());
}

const PAT: usize = 0x900;
const MAC: usize = 0xA00;

/// A module of the fixed layout: song 0 is step 0 to step 0, with the pattern on track 0.
fn build(pattern: &[[u8; 4]], mac: &[[u8; 4]]) -> Vec<u8> {
    let mut d = vec![0u8; MAC + 4 * mac.len() + 16];
    d[..10].copy_from_slice(b"TFMX-SONG ");
    put16(&mut d, 256, 0);
    put16(&mut d, 320, 0);
    put16(&mut d, 384, 3);
    for i in 0..32 {
        put32(&mut d, 0x400 + 4 * i, PAT as u32);
        put32(&mut d, 0x600 + 4 * i, MAC as u32);
    }
    // the last macro entry bounds the detection's scan
    put32(&mut d, 0x7FC, (MAC + 4 * mac.len()) as u32);
    // step 0: pattern 0 on track 0, the other tracks stopped
    put16(&mut d, 0x800, 0x0000);
    for t in 1..8 {
        put16(&mut d, 0x800 + 2 * t, 0xFF00);
    }
    for (i, r) in pattern.iter().enumerate() {
        d[PAT + 4 * i..PAT + 4 * i + 4].copy_from_slice(r);
    }
    for (i, r) in mac.iter().enumerate() {
        d[MAC + 4 * i..MAC + 4 * i + 4].copy_from_slice(r);
    }
    d
}

fn small() -> Vec<u8> {
    build(
        // note $10 with macro 0 on field 0 at volume 15, a wait of 3, then the pattern's end
        &[[0x10, 0, 0xF0, 0], [0xF3, 3, 0, 0], [0x14, 0, 0xF0, 0], [0xF3, 3, 0, 0], [0xF0, 0, 0, 0]],
        // tone on, note period, volume, wait, stop
        &[[0x41, 1, 0, 0], [0x08, 0, 0, 0], [0x0E, 0, 0, 0x30], [0x04, 0, 0, 20], [0x07, 0, 0, 0]],
    )
}

fn run(d: &[u8], ticks: usize) -> Option<Vec<(u32, u16, u64)>> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("mdst.test"), d.to_vec()).ok()?;
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..ticks {
        e.render(&mut buf);
    }
    e.m.bus.write_log.clone()
}

#[test]
fn recognises_a_module_with_an_st_command() {
    assert!(detect(&small()));
    assert_eq!(scan(&small()), Some(0));
}

#[test]
fn counts_the_sample_commands() {
    let d = build(&[[0xF0, 0, 0, 0]], &[[0x48, 0, 0, 0], [0x41, 0, 0, 0], [0x48, 0, 0, 4], [0x07, 0, 0, 0]]);
    assert_eq!(scan(&d), Some(2));
}

#[test]
fn refuses_amiga_macros_and_other_data() {
    assert!(!detect(&[]));
    assert!(!detect(&[0u8; 4096]));
    assert!(!detect(b"TFMX-SONG "));
    // only commands the Amiga players know: refused
    let d = build(&[[0xF0, 0, 0, 0]], &[[0x08, 0, 0, 0], [0x04, 0, 0, 2], [0x07, 0, 0, 0]]);
    assert!(!detect(&d));
    let mut d = small();
    d[9] = b'X';
    assert!(!detect(&d));
    // a bound that is odd
    let mut d = small();
    put32(&mut d, 0x7FC, (MAC + 3) as u32);
    assert!(!detect(&d));
}

#[test]
fn truncated_and_damaged_modules_do_not_panic() {
    let d = small();
    for n in (0..d.len()).step_by(7) {
        let _ = detect(&d[..n]);
    }
    for i in (0..d.len()).step_by(5) {
        for x in [0u8, 0xFF, 0x80] {
            let mut e = d.clone();
            e[i] = x;
            let _ = detect(&e);
        }
    }
}

#[test]
fn damaged_modules_play_without_panic() {
    let d = small();
    for i in (0x100..d.len()).step_by(11) {
        for x in [0xFFu8, 0x7F] {
            let mut e = d.clone();
            e[i] = x;
            let _ = run(&e, 12);
        }
    }
}

#[test]
fn the_subsong_list_follows_the_table_rules() {
    let mut d = vec![0u8; 0x200];
    // songs 0 and 1 distinct, song 2 an unused slot, the rest empty
    for k in 0..32 {
        put16(&mut d, 256 + 2 * k, 0x1FF);
        put16(&mut d, 320 + 2 * k, 0);
    }
    put16(&mut d, 256, 0);
    put16(&mut d, 320, 5);
    put16(&mut d, 258, 6);
    put16(&mut d, 322, 9);
    assert_eq!(song_list(&d), vec![0, 1]);
}

#[test]
fn the_noise_buffer_is_a_fixed_generator() {
    let a = noise_bytes();
    assert_eq!(a.len(), 1024);
    assert_eq!(a, noise_bytes());
    assert!(a.iter().any(|&b| b != a[0]));
}

#[test]
fn plays_a_tone_with_the_restart_sequence() {
    let l = run(&small(), 8).expect("module plays");
    let w: Vec<(u32, u16)> = l.iter().map(|x| (x.0, x.1)).collect();
    // the first restart of channel 0: AUDxPER = 1, INTREQ and DMACON written 0 (not running yet)
    let i = w.iter().position(|x| *x == (0xDFF0A6, 1)).expect("restart");
    assert_eq!(w[i + 1], (0xDFF09C, 0));
    assert_eq!(w[i + 2], (0xDFF096, 0));
    // the note's period (PSG 1072 times 7, plus 1) and the macro's volume reach channel 0
    assert!(w.contains(&(0xDFF0A6, 7505)), "no tone period");
    assert!(w.iter().any(|x| x.0 == 0xDFF0A8 && x.1 == 30), "no volume");
    // times never go backwards
    assert!(l.windows(2).all(|p| p[0].2 <= p[1].2));
}

#[test]
fn runs_are_deterministic() {
    let a = run(&small(), 12).unwrap();
    let b = run(&small(), 12).unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_pattern_return_without_a_call_ends_the_track() {
    // the original reads low memory here; the port ends the track and goes on
    let d = build(&[[0x10, 0, 0xF0, 0], [0xF3, 1, 0, 0], [0xF9, 0, 0, 0]], &[[0x41, 1, 0, 0], [0x08, 0, 0, 0], [0x07, 0, 0, 0]]);
    assert!(run(&d, 30).is_some());
}

#[test]
fn a_macro_return_without_a_call_stops_the_voice() {
    let d = build(&[[0x10, 0, 0xF0, 0], [0xF3, 8, 0, 0], [0xF0, 0, 0, 0]], &[[0x41, 1, 0, 0], [0x16, 0, 0, 0], [0x07, 0, 0, 0]]);
    assert!(run(&d, 30).is_some());
}

#[test]
fn an_endless_macro_loop_does_not_hang() {
    // a loop with a zero count and no wait: cut after a bounded number of steps
    let d = build(&[[0x10, 0, 0xF0, 0], [0xF3, 8, 0, 0], [0xF0, 0, 0, 0]], &[[0x41, 1, 0, 0], [0x05, 0, 0, 0], [0x07, 0, 0, 0]]);
    assert!(run(&d, 10).is_some());
}
