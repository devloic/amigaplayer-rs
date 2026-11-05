use super::*;
use crate::engine::Engine;
use std::path::Path;

// Synthetic modules, built from the specification's layout (no game data).

const INSTR: usize = 64;
const POS: usize = INSTR + 2 * 64;
const PATS: usize = POS + 64;
const SDATA: usize = PATS + 1024;

fn put32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

fn put16(v: &mut [u8], o: usize, x: u16) {
    v[o..o + 2].copy_from_slice(&x.to_be_bytes());
}

/// One subsong (positions 0 to 1, restart `restart`), instrument 0 a looped sample, instrument 1 a
/// synth with a sweep and an envelope, pattern 0 with `rows` entries (row, voice, bytes).
fn build(restart: u16, rows: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
    let mut d = vec![0u8; SDATA + 64];
    d[..4].copy_from_slice(b"PSA\0");
    d[4..36].copy_from_slice(b"Synthetic test module           ");
    let len = d.len() as u32;
    put32(&mut d, 36, len);
    put32(&mut d, 40, INSTR as u32);
    put32(&mut d, 44, POS as u32);
    put32(&mut d, 48, PATS as u32);
    put16(&mut d, 56, 0);
    put16(&mut d, 58, 2);
    put16(&mut d, 60, restart);
    // instrument 0: sample, 16 words, loop at +32 of 4 words, volume 0x30
    put32(&mut d, INSTR, SDATA as u32);
    put16(&mut d, INSTR + 4, 16);
    put32(&mut d, INSTR + 6, SDATA as u32 + 32);
    put16(&mut d, INSTR + 10, 4);
    put16(&mut d, INSTR + 12, 0x30);
    // instrument 1: synth, window of 8 words, offset 4, sweep step 2 every 2 ticks, 3 steps
    let i1 = INSTR + 64;
    put16(&mut d, i1 + 4, 8);
    put16(&mut d, i1 + 12, 0x20);
    d[i1 + 14..i1 + 22].copy_from_slice(&[1, 4, 2, 2, 3, 1, 2, 3]);
    // envelope: attack 4 steps of 1 every 1 tick
    d[i1 + 22..i1 + 26].copy_from_slice(&[1, 1, 1, 4]);
    for (r, v, e) in rows {
        d[PATS + 16 * r + 4 * v..PATS + 16 * r + 4 * v + 4].copy_from_slice(e);
    }
    for i in 0..64 {
        d[SDATA + i] = (i * 3) as u8;
    }
    d
}

fn start(d: &[u8], ticks: usize) -> (Engine, Vec<(u32, u16, u64)>) {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("test.psa"), d.to_vec()).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..ticks {
        e.render(&mut buf);
    }
    let l = e.m.bus.write_log.clone().unwrap();
    (e, l)
}

#[test]
fn recognises_the_magic_and_nothing_else() {
    let d = build(0xFFFF, &[]);
    assert!(detect(&d));
    let mut t = d.clone();
    t[3] = 1;
    assert!(!detect(&t));
    assert!(!detect(&[]));
    assert!(!detect(&d[..40]));
    // an instrument table beyond the file
    let mut t = d.clone();
    put32(&mut t, 40, 0x10_0000);
    assert!(!detect(&t));
}

#[test]
fn start_up_duration_follows_the_instrument_classes() {
    let d = build(0xFFFF, &[]);
    // instrument 0 starts at SDATA (larger than every earlier one), instrument 1 has offset 0
    assert_eq!(init_duration(&d, INSTR, POS), (598 + 122 + 47) / 2);
    // an offset below the running maximum counts as "other"
    let mut t = d.clone();
    put32(&mut t, INSTR + 64, 2);
    assert_eq!(init_duration(&t, INSTR, POS), (598 + 122 + 62) / 2);
}

#[test]
fn note_start_write_order_and_clocks() {
    // voice 0: sample note 13; voice 1: synth note 20 with the volume effect
    let d = build(0xFFFF, &[(0, 0, [13, 0, 0, 0]), (0, 1, [20, 1, 12, 0x25])]);
    let (_e, log) = start(&d, 3);
    let dur = init_duration(&d, INSTR, POS);
    // head: two volume set-ups, the subsong init, then the first tick 70940 after its end
    let tick0 = 168 + dur + 2 * 648 + 976 + 70940;
    let w: Vec<_> = log.iter().filter(|e| e.2 >= tick0).take(14).collect();
    // sample note: DMACON stop, LC hi, LC lo, LEN, VOL, PER
    assert_eq!((w[0].0, w[0].1), (0xDFF096, 0x0001));
    assert_eq!(w[0].2, tick0 + (242 + 41 + 162 + 9 + 18 + 82 + 63 + 15 + 17 + 16) / 2);
    let base = w[1].1 as u32;
    assert_eq!((w[1].0, w[2].0), (0xDFF0A0, 0xDFF0A2));
    let lc = (base << 16 | w[2].1 as u32) as usize;
    assert_eq!(w[3].0, 0xDFF0A4);
    assert_eq!(w[3].1, 16);
    assert_eq!((w[4].0, w[4].1), (0xDFF0A8, 0x30));
    assert_eq!((w[5].0, w[5].1), (0xDFF0A6, PERIODS[12]));
    // the sample's address is the module's load address plus its offset
    assert_eq!(lc % 0x1_0000_0000, lc);
    // synth note on voice 1: DMACON stop, LEN, LC, VOL (the effect's value), PER
    assert_eq!((w[6].0, w[6].1), (0xDFF096, 0x0002));
    assert_eq!((w[7].0, w[7].1), (0xDFF0B4, 8));
    assert_eq!((w[8].0, w[9].0), (0xDFF0B0, 0xDFF0B2));
    assert_eq!((w[10].0, w[10].1), (0xDFF0B8, 0x25));
    assert_eq!((w[11].0, w[11].1), (0xDFF0B6, PERIODS[19]));
    // the effect's own check writes the volume again
    assert_eq!((w[12].0, w[12].1), (0xDFF0B8, 0x25));
}

#[test]
fn the_dma_step_sets_the_started_voices_and_loops_the_sample() {
    let d = build(0xFFFF, &[(0, 0, [13, 0, 0, 0])]);
    let (_e, log) = start(&d, 3);
    let dmacon: Vec<_> = log.iter().filter(|e| e.0 == 0xDFF096).map(|e| e.1).collect();
    // start-up 0x000F, the init's 0x000F, the note's stop 1, then 0x8001, then plain 0x8000
    assert!(dmacon.windows(3).any(|w| w == [1, 0x8001, 0x8000]));
    // after the set bit the loop pointer (silent loop not used: the instrument has a loop) and length
    let i = log.iter().position(|e| e.0 == 0xDFF096 && e.1 == 0x8001).unwrap();
    let rest: Vec<_> = log[i + 1..].iter().take(3).collect();
    assert_eq!((rest[0].0, rest[1].0), (0xDFF0A0, 0xDFF0A2));
    assert_eq!((rest[2].0, rest[2].1), (0xDFF0A4, 4));
}

#[test]
fn the_song_stops_at_its_end_with_the_stop_writes() {
    let d = build(0xFFFF, &[]);
    // 64 rows of 7 ticks: the stop comes in the tick of row 63's successor
    let (e, log) = start(&d, 1600);
    assert!(e.songend());
    let i = log.iter().rposition(|e| e.0 == 0xDFF096 && e.1 == 0x000F).unwrap();
    let vols: Vec<_> = log[i + 1..].iter().take(4).map(|e| (e.0, e.1)).collect();
    assert_eq!(vols, vec![(0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0)]);
    // afterwards only DMACON 0x8000 each tick (no voice writes)
    assert!(log[i + 5..].iter().filter(|e| e.0 != 0xDFF096).count() <= 8 * 5);
}

#[test]
fn runs_are_deterministic_and_reach_every_effect_without_panicking() {
    let mut rows = Vec::new();
    for r in 0..16 {
        for v in 0..4 {
            let eff = (r * 4 + v) as u8 % 16;
            rows.push((r, v, [(r as u8 * 3 + v as u8) % 40, (v % 3) as u8, eff, 0x37 + r as u8]));
        }
    }
    let d = build(0, &rows);
    let (_, a) = start(&d, 300);
    let (_, b) = start(&d, 300);
    assert_eq!(a, b);
    assert!(a.len() > 100);
}

#[test]
fn malformed_input_does_not_panic() {
    let good = build(0xFFFF, &[(0, 0, [13, 0, 0, 0]), (1, 1, [200, 99, 0xFF, 0xFF])]);
    for n in (0..good.len()).step_by(61) {
        let _ = detect(&good[..n]);
        if detect(&good[..n]) {
            let _ = start(&good[..n], 4);
        }
    }
    // offsets and counts pointing outside the file
    let mut t = good.clone();
    put32(&mut t, 44, 0xFFFF_FFF0);
    put32(&mut t, 48, 0xFFFF_0000);
    let _ = start(&t, 10);
    let mut t = good;
    put16(&mut t, 56, 0xFFF0);
    put16(&mut t, 58, 0xFFF1);
    let _ = start(&t, 10);
}
