//! Tests for the Special FX ST open replayer. The modules are built here from the layout alone (no
//! game data, no copy of any module): a skeleton that carries only the byte patterns the table
//! search needs, the tables, and one or three voice sequences.

use super::*;
use crate::engine::Engine;
use std::path::Path;

const SUB: usize = 0x300;
const PAT: usize = 0x308;
const DRUM: usize = 0x320;
const TRIG: usize = 0x330;
const ARP: usize = 0x340;
const SLD: usize = 0x350;
const ENV: usize = 0x360;
const SLIST: usize = 0x370;
const DIGI: usize = 0x380;
const SEQ_A: usize = 0x400;
const SEQ_B: usize = 0x420;
const SEQ_C: usize = 0x440;
const PATS: usize = 0x500;
const ENVDATA: usize = 0x600;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A displacement word at `x` that reaches `target`.
fn disp(d: &mut [u8], x: usize, target: usize) {
    put16(d, x, (target as i64 - x as i64) as i16 as u16);
}

/// Entry `n` of the word table at `table`, pointing at `target`.
fn entry(d: &mut [u8], table: usize, n: usize, target: usize) {
    disp(d, table + 2 * n, target);
}

/// The skeleton: the six voice record tails, the driver's search patterns (all table references by
/// displacement), the period table marker at 2000, and the tables at fixed places.
fn skeleton() -> Vec<u8> {
    let mut d = vec![0u8; 2400];
    put16(&mut d, 0, 0x6000);
    for (o, v) in [(0x10, 0x0009_0800), (0x5A, 0x0112_0900), (0xA4, 0x0224_0A00), (0xEE, 0x0009_0800), (0x138, 0x0112_0900), (0x182, 0x0224_0A00)] {
        put32(&mut d, o, v);
    }
    put16(&mut d, 0x202, 1);
    put16(&mut d, 0x208, 0x41FA);
    disp(&mut d, 0x20A, SUB);
    put16(&mut d, 0x20C, 0xE740);
    disp(&mut d, 0x220, PAT);
    put32(&mut d, 0x222, 0x4880_D040);
    put16(&mut d, 0x230, 0x41FA);
    disp(&mut d, 0x232, DRUM);
    put32(&mut d, 0x240, 0x08C7_0007);
    put16(&mut d, 0x250, 0x41FA);
    disp(&mut d, 0x252, TRIG);
    put32(&mut d, 0x260, 0x0887_0002);
    put16(&mut d, 0x264, 0x41FA);
    disp(&mut d, 0x266, ARP);
    put32(&mut d, 0x270, 0x08C7_0002);
    put16(&mut d, 0x278, 0x41FA);
    disp(&mut d, 0x27A, SLD);
    put16(&mut d, 0x280, 0x41FA);
    disp(&mut d, 0x282, ENV);
    put16(&mut d, 0x28C, 0x41FA);
    disp(&mut d, 0x28E, SLIST);
    put32(&mut d, 0x290, 0x7000_101A);
    put16(&mut d, 0x2A0, 0x41FA);
    disp(&mut d, 0x2A2, DIGI);
    put32(&mut d, 2000, 0x0EF8_0E10);
    d
}

/// One voice (A) on a sequence that plays pattern 0 and loops (a jump back); pattern 0 is `pat`.
/// Envelope 0 is one step to volume 15 held.
fn one_voice(pat: &[u8]) -> Vec<u8> {
    let mut d = skeleton();
    disp(&mut d, SUB, SEQ_A);
    entry(&mut d, PAT, 0, PATS);
    d[SEQ_A..SEQ_A + 3].copy_from_slice(&[0x00, 0x81, 0xFE]);
    d[PATS..PATS + pat.len()].copy_from_slice(pat);
    entry(&mut d, ENV, 0, ENVDATA);
    d[ENVDATA..ENVDATA + 4].copy_from_slice(&[1, 15, 0, 15]);
    d
}

/// The tone-only note pattern of the reference runs: mixer tone, envelope 0, note 0x18, 10 ticks.
const NOTE: [u8; 5] = [0x83, 0xE0, 0x18, 0x0A, 0x82];

fn run(d: &[u8], ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if e.play_data(Path::new("doda.test"), d.to_vec()).is_err() {
        return Vec::new();
    }
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..ticks {
        e.render(&mut buf);
    }
    e.m.bus.write_log.clone().unwrap()
}

/// The register writes of the log grouped into ticks (more than 20 000 colour clocks apart).
fn ticks(l: &[(u32, u16, u64)]) -> Vec<Vec<(u32, u16, u64)>> {
    let mut g: Vec<Vec<(u32, u16, u64)>> = Vec::new();
    let mut last = None;
    for &w in l {
        if w.0 & 0xFFFF00 != 0xDFF000 {
            continue;
        }
        if last.map_or(true, |t| w.2 - t > 20000) {
            g.push(Vec::new());
        }
        last = Some(w.2);
        g.last_mut().unwrap().push(w);
    }
    g
}

/// The period the emulator writes for a YM tone period.
fn conv(p: u32) -> u16 {
    (((p & 0xFFF) * 0x38CC >> 10) + 1) as u16
}

/// The last PER write to Paula channel `ch` of each tick.
fn pers(l: &[(u32, u16, u64)], ch: u32) -> Vec<u16> {
    ticks(l).iter().filter_map(|t| t.iter().rev().find(|w| w.0 == 0xDFF0A6 + 16 * ch).map(|w| w.1)).collect()
}

#[test]
fn the_skeleton_is_recognised_and_its_tables_found() {
    let d = one_voice(&NOTE);
    assert!(detect(&d));
    let t = locate(&d, 0).expect("tables");
    assert_eq!(t.subsong, SUB as i64);
    assert_eq!(t.pattern, PAT as i64);
    assert_eq!((t.drum, t.trigger, t.arp, t.slide, t.env, t.slist), (DRUM as i64, TRIG as i64, ARP as i64, SLD as i64, ENV as i64, SLIST as i64));
    assert_eq!(t.digital, DIGI as i64);
    assert_eq!(t.count, 1);
    // a module loaded elsewhere: the tables are addresses in its memory
    let t = locate(&d, 0x4000).unwrap();
    assert_eq!(t.subsong, 0x4000 + SUB as i64);
}

#[test]
fn the_subsong_count_comes_from_the_table_distance() {
    let mut d = one_voice(&NOTE);
    disp(&mut d, 0x220, SUB + 24);
    assert_eq!(locate(&d, 0).unwrap().count, 3);
}

#[test]
fn detection_rules() {
    assert!(!detect(&[]));
    assert!(!detect(&[0u8; 4096]));
    let d = skeleton();
    assert!(detect(&d));
    let mut e = d.clone();
    e.truncate(2300);
    assert!(!detect(&e));
    // the first longword: 0, 0x101, word 0x0101 and 0x6000 are accepted, anything else is not
    for (first, ok) in [(0x0000_0000u32, true), (0x0000_0101, true), (0x0101_FFFF, true), (0x6000_1234, true), (0x1234_5678, false)] {
        let mut e = d.clone();
        put32(&mut e, 0, first);
        assert_eq!(detect(&e), ok, "{first:08x}");
    }
    // a damaged voice record tail, a missing period marker
    let mut e = d.clone();
    put32(&mut e, 0x138, 0);
    assert!(!detect(&e));
    let mut e = d.clone();
    put32(&mut e, 2000, 0);
    assert!(!detect(&e));
    // the marker may sit later in the file
    let mut e = d.clone();
    put32(&mut e, 2000, 0);
    put32(&mut e, 2100, 0x0EF8_0E10);
    assert!(detect(&e));
}

#[test]
fn a_missing_table_pattern_fails_the_load_without_panic() {
    let d = one_voice(&NOTE);
    for (o, n) in [(0x20C, 2usize), (0x222, 4), (0x240, 4), (0x260, 4), (0x270, 4)] {
        let mut e = d.clone();
        e[o..o + n].fill(0);
        assert!(locate(&e, 0).is_none(), "{o:x}");
    }
}

#[test]
fn truncated_and_damaged_modules_do_not_panic() {
    let d = one_voice(&NOTE);
    for n in (0..d.len()).step_by(37) {
        let _ = detect(&d[..n]);
        let _ = locate(&d[..n], 0);
    }
    for i in (0..d.len()).step_by(13) {
        for x in [0u8, 0xFF, 0x80, 0x7F] {
            let mut e = d.clone();
            e[i] = x;
            let _ = detect(&e);
            let _ = locate(&e, 0);
        }
    }
}

#[test]
fn damaged_modules_play_without_panic() {
    let d = one_voice(&NOTE);
    for i in (0x200..0x700).step_by(23) {
        for x in [0xFFu8, 0x80, 0x82] {
            let mut e = d.clone();
            e[i] = x;
            if detect(&e) {
                let _ = run(&e, 6);
            }
        }
    }
}

#[test]
fn the_period_table_is_the_drivers() {
    assert_eq!(PERIODS.len(), 102);
    assert_eq!((PERIODS[0], PERIODS[24], PERIODS[36], PERIODS[101]), (0xEF8, 0x3BE, 0x1DF, 9));
    // octaves: the table halves every twelve notes, up to rounding
    for i in 0..60 {
        let (a, b) = (PERIODS[i] as i32, PERIODS[i + 12] as i32);
        assert!((a - 2 * b).abs() <= 4, "{i}");
    }
}

#[test]
fn the_noise_block_is_a_generator() {
    let a = noise_bytes();
    assert_eq!(a.len(), 1024);
    assert_eq!(a, noise_bytes());
    assert!(a.iter().any(|&b| b != a[0]));
}

#[test]
fn the_first_tick_restarts_the_voice_channels_as_the_original_does() {
    let l = run(&one_voice(&NOTE), 3);
    let t = ticks(&l);
    assert!(t.len() >= 2);
    let w = &t[0];
    let regs: Vec<(u32, u16)> = w.iter().map(|x| (x.0, x.1)).collect();
    // voice A (channel 0): period 1, INTREQ (nothing playing), DMACON, the pulse sample, length 1,
    // the period, then the second half with DMA on
    assert_eq!(regs[0], (0xDFF0A6, 1));
    assert_eq!(regs[1], (0xDFF09C, 0));
    assert_eq!(regs[2], (0xDFF096, 0));
    assert_eq!(regs[3].0, 0xDFF0A0);
    assert_eq!(regs[4].0, 0xDFF0A2);
    assert_eq!(regs[5], (0xDFF0A4, 1));
    assert_eq!(regs[6], (0xDFF0A6, conv(0x3BE)));
    assert_eq!(regs[7], (0xDFF09C, 0x80));
    assert_eq!(regs[8], (0xDFF096, 0x8001));
    assert_eq!(regs[11], (0xDFF0A4, 1));
    // volume 64 from the envelope, then the period
    assert_eq!(regs[12], (0xDFF0A8, 0x40));
    assert_eq!(regs[13], (0xDFF0A6, conv(0x3BE)));
    // the same loop sample twice; the cycle ledger's gaps
    assert_eq!(regs[3], regs[9]);
    let at = |i: usize| w[i].2;
    assert_eq!(at(1) - at(0), 26);
    assert_eq!(at(2) - at(1), 6);
    assert_eq!(at(5) - at(4), 6);
    assert_eq!(at(13) - at(12), 12);
    // voice B (channel 3) is silent: restarted with the silent word, period 0x100, volume 0
    assert_eq!(regs[14], (0xDFF0D6, 1));
    assert!(regs.contains(&(0xDFF0D8, 0)));
    assert!(regs.contains(&(0xDFF0D6, 0x100)));
    // the second tick holds the tone (no restart of channel 0): volume and period only
    let t1: Vec<u32> = t[1].iter().map(|x| x.0).take(2).collect();
    assert_eq!(t1, vec![0xDFF0A8, 0xDFF0A6]);
}

#[test]
fn a_run_is_deterministic() {
    let d = one_voice(&NOTE);
    assert_eq!(run(&d, 8), run(&d, 8));
}

#[test]
fn ticks_are_70940_colour_clocks_apart() {
    let l = run(&one_voice(&NOTE), 5);
    let t = ticks(&l);
    // the same step of the voice B restart in consecutive ticks: the second tick's writes of a held
    // voice start a whole tick later
    let first: Vec<u64> = t.iter().map(|g| g[0].2).collect();
    assert!(t.len() >= 4);
    // tick starts differ by one period less the restart waits of tick 0
    assert!(first[2] - first[1] <= 70940 + 300 && first[2] - first[1] + 300 >= 70940);
}

#[test]
fn the_envelope_sets_the_volume_and_a_rest_silences_it() {
    // a note of 3 ticks, a rest of 2 ticks, then the pattern ends
    let d = one_voice(&[0x83, 0xE0, 0x18, 0x03, 0x87, 0x02, 0x82]);
    let l = run(&d, 8);
    let vols: Vec<u16> = ticks(&l).iter().filter_map(|t| t.iter().rev().find(|w| w.0 == 0xDFF0A8).map(|w| w.1)).collect();
    assert_eq!(&vols[..6], &[64, 64, 64, 0, 0, 64]);
}

#[test]
fn an_arpeggio_alternates_the_period() {
    let mut d = one_voice(&[0x83, 0xE0, 0xA1, 0x18, 0x08, 0x82]);
    entry(&mut d, ARP, 1, 0x620);
    d[0x620..0x624].copy_from_slice(&[1, 2, 0, 12]);
    let l = run(&d, 6);
    let p = pers(&l, 0);
    let (lo, hi) = (conv(PERIODS[24] as u32), conv(PERIODS[36] as u32));
    assert_eq!(&p[..4], &[lo, hi, lo, hi]);
}

#[test]
fn a_slide_adds_its_delta_every_tick() {
    let mut d = one_voice(&[0x83, 0xE0, 0xC1, 0x18, 0x08, 0x82]);
    entry(&mut d, SLD, 1, 0x620);
    // five ticks of +3, then hold (count 0)
    d[0x620..0x626].copy_from_slice(&[0, 5, 0, 3, 0, 0]);
    let l = run(&d, 6);
    let p = pers(&l, 0);
    let base = PERIODS[24] as u32;
    // the YM period grows by 3 each tick from the note's own tick on
    assert_eq!(p[0], conv(base + 3));
    assert_eq!(p[1], conv(base + 6));
    assert_eq!(p[2], conv(base + 9));
}

#[test]
fn transpose_and_repeat_in_the_sequence() {
    // sequence: repeat 2, transpose 12, pattern 0; the pattern is one short note
    let mut d = one_voice(&[0x83, 0xE0, 0x10, 0x01, 0x82]);
    d[SEQ_A..SEQ_A + 7].copy_from_slice(&[0x82, 0x02, 0x83, 12, 0x00, 0x81, 0xFE]);
    let l = run(&d, 5);
    let p = pers(&l, 0);
    // note 0x10 plus 12 on every repeat of the pattern
    assert_eq!(p[0], conv(PERIODS[28] as u32));
    assert_eq!(p[1], conv(PERIODS[28] as u32));
}

#[test]
fn three_voices_use_their_own_channels() {
    let mut d = one_voice(&NOTE);
    disp(&mut d, SUB + 2, SEQ_B);
    disp(&mut d, SUB + 4, SEQ_C);
    d[SEQ_B..SEQ_B + 3].copy_from_slice(&[0x00, 0x81, 0xFE]);
    d[SEQ_C..SEQ_C + 3].copy_from_slice(&[0x00, 0x81, 0xFE]);
    let l = run(&d, 3);
    let t = ticks(&l);
    // channel 3 (voice B) and channel 2 (voice C) get tone periods too
    for ch in [3u32, 2] {
        assert!(t[0].iter().any(|w| w.0 == 0xDFF0A6 + 16 * ch && w.1 == conv(0x3BE)), "channel {ch}");
    }
}

#[test]
fn a_voice_that_stops_ends_the_song() {
    let d = one_voice(&[0x83, 0xE0, 0x18, 0x02, 0x80]);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("doda.test"), d).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    let mut ended = None;
    for t in 0..12 {
        e.render(&mut buf);
        if ended.is_none() && e.songend() {
            ended = Some(t);
        }
    }
    assert!(ended.is_some());
}

#[test]
fn notes_outside_the_period_table_read_the_embedded_words_or_the_table_addresses() {
    let d = one_voice(&NOTE);
    let t = locate(&d, 0x4000).unwrap();
    let s = SpecialFxSt {
        d: d.clone(),
        base: 0x4000,
        tb: t,
        sub: 1,
        v: [Voice::default(); 3],
        r: [0; 11],
        er: [0; 11],
        se: [0; 4],
        rec: [Rec::default(); 4],
        nc: 0,
        length: 1,
        noise: 0,
        pulse: 0,
        silent: 0,
        songend: false,
        call: 0,
        t: 0,
        next_at: None,
    };
    // indices 102.. give the located table addresses (arpeggio first), high word first
    let a = t.arp as u32;
    assert_eq!(s.period(102), (a >> 16) as u16);
    assert_eq!(s.period(103), a as u16);
    // the player's code around the table is not embedded: zero
    assert_eq!((s.period(-1), s.period(-128), s.period(122), s.period(127)), (0, 0, 0, 0));
}
