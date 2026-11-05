//! Tests for the Forgotten Worlds open replayer. The modules are built here from the layout of the
//! specification alone (no game data, no copy of any module). The replayer is driven directly:
//! `OPEN.new` on a machine, then `tick` at the times it asks for, so the tests do not depend on
//! which replayer the engine's detection picks for an `FWMP` file.

use super::*;
use crate::engine::Engine;

const DMACON: u32 = 0xDFF096;
const INTREQ: u32 = 0xDFF09C;

fn put16(d: &mut Vec<u8>, v: u16) {
    d.extend_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut Vec<u8>, v: u32) {
    d.extend_from_slice(&v.to_be_bytes());
}

/// One instrument: waveform entry number, sustain, attack/start, decay, hold, release.
#[derive(Clone, Copy)]
struct Inst {
    wave: u8,
    sustain: u8,
    attack: u8,
    decay: i8,
    hold: u8,
    release: i8,
}

/// A module of four channel streams, patterns, instruments and waveforms of 16 bytes each
/// (first pass 8 words, loop at byte 8 for 4 words).
fn module(streams: [&[u8]; 4], patterns: &[&[u8]], insts: &[Inst], waves: usize) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    // the four start words, then the streams
    let mut at = 8usize;
    for s in streams {
        put16(&mut body, at as u16);
        at += s.len();
    }
    for s in streams {
        body.extend_from_slice(s);
    }
    let pt = body.len();
    let n = patterns.len();
    let mut off = 2 * n;
    let mut table: Vec<u8> = Vec::new();
    for p in patterns {
        put16(&mut table, off as u16);
        off += p.len();
    }
    body.extend_from_slice(&table);
    for p in patterns {
        body.extend_from_slice(p);
    }
    let it = body.len();
    for i in insts {
        body.extend_from_slice(&[i.wave * 8, i.sustain, i.attack, i.decay as u8, i.hold, i.release as u8, 0, 0]);
    }
    let wd = body.len();
    let dir = 8 * waves;
    for w in 0..waves {
        put16(&mut body, (dir + 16 * w) as u16);
        put16(&mut body, 8);
        put16(&mut body, 8);
        put16(&mut body, 4);
    }
    for w in 0..waves {
        for k in 0..16 {
            body.push((w * 16 + k) as u8);
        }
    }
    let mut d = b"FWMP".to_vec();
    put32(&mut d, pt as u32);
    put32(&mut d, it as u32);
    put32(&mut d, wd as u32);
    d.extend_from_slice(&body);
    d
}

const IDLE: &[u8] = &[0x86, 0xFF, 0x82];

type Log = Vec<(u32, u16, u64)>;

/// Plays `d` for `ticks` ticks; the module is at guest address `ADDR`.
const ADDR: u32 = 0x40000;

fn run(d: &[u8], ticks: usize) -> (Log, u32) {
    let mut e = Engine::new(48000.0);
    e.m.bus.write_log = Some(Vec::new());
    e.m.bus.write_bytes(ADDR, d);
    let Some(mut r) = (OPEN.new)(d, ADDR, &mut e.m) else { return (Vec::new(), ADDR) };
    for _ in 0..ticks {
        let at = r.next_tick_at().unwrap();
        e.m.bus.clock = e.m.bus.clock.max(at);
        r.tick(&mut Hw { bus: &mut e.m.bus });
    }
    (e.m.bus.write_log.clone().unwrap(), ADDR)
}

/// The writes of tick `k` (from its DMACON 0x800F to the next one), the first 24 skipped.
fn tick_writes(l: &Log, k: usize) -> Vec<(u32, u16, u64)> {
    let ticks: Vec<usize> = (0..l.len()).filter(|&i| l[i].0 == DMACON && l[i].1 == 0x800F).collect();
    let a = ticks[k];
    let b = ticks.get(k + 1).copied().unwrap_or(l.len());
    l[a..b].to_vec()
}

fn two_inst() -> Vec<Inst> {
    vec![
        Inst { wave: 0, sustain: 0x20, attack: 0x10, decay: -8, hold: 3, release: -0x10 },
        Inst { wave: 1, sustain: 0, attack: 0x40, decay: 0, hold: 0, release: 0 },
    ]
}

#[test]
fn detection_is_the_magic_only() {
    let d = module([IDLE; 4], &[&[0x8A]], &two_inst(), 2);
    assert!(detect(&d));
    assert!(!detect(&[]));
    assert!(!detect(b"FWMP"));
    let mut e = d.clone();
    e[3] = b'Q';
    assert!(!detect(&e));
    assert!(selected(&d).is_none() || std::env::var_os("DELI_FW_REDO").is_some());
}

#[test]
fn initialisation_writes_and_first_tick_time() {
    let d = module([IDLE; 4], &[&[0x8A]], &two_inst(), 2);
    let (l, _) = run(&d, 2);
    // 24 initialisation writes: LC hi/lo, LEN 2, PER 0x7C, VOL 0 per channel, then four strays
    let regs: Vec<(u32, u16)> = l[..24].iter().map(|w| (w.0, w.1)).collect();
    for ch in 0..4usize {
        let r = 0xDFF0A0 + 16 * ch as u32;
        assert_eq!(l[ch * 5].0, r);
        assert_eq!(l[ch * 5 + 1].0, r + 2);
        assert_eq!(l[ch * 5].2, l[ch * 5 + 1].2, "LC hi and lo at the same clock");
        assert_eq!((l[ch * 5 + 2].0, l[ch * 5 + 2].1), (r + 4, 2));
        assert_eq!((l[ch * 5 + 3].0, l[ch * 5 + 3].1), (r + 6, 0x7C));
        assert_eq!((l[ch * 5 + 4].0, l[ch * 5 + 4].1), (r + 8, 0));
        assert_eq!(l[ch * 5 + 2].2 - l[ch * 5].2, 8);
        assert_eq!(l[ch * 5 + 3].2 - l[ch * 5 + 2].2, 8);
        assert_eq!(l[ch * 5 + 4].2 - l[ch * 5 + 3].2, 8);
        if ch < 3 {
            assert_eq!(l[ch * 5 + 5].2 - l[ch * 5 + 4].2, 15);
        }
    }
    assert_eq!(regs[20].0, 0xDFF17E);
    assert_eq!(regs[20].1, 0x00FF);
    assert_eq!(l[20].2 - l[19].2, 17);
    assert_eq!((regs[21], regs[22], regs[23]), ((0xDFF176, 0x000F), (0xDFF17C, 0x0780), (0xDFF17A, 0x0780)));
    assert_eq!(l[23].2 - l[0].2, 182);
    assert_eq!(l[24].0, DMACON);
    assert_eq!(l[24].2 - l[0].2, 71249);
    // the second tick comes 70940 clocks later (nothing to wait for in an idle song)
    let t = tick_writes(&l, 1);
    assert_eq!(t[0].2 - l[24].2, 70940);
}

#[test]
fn note_start_sequence() {
    // channel 0: select instrument 0, note 12 for 5 ticks, then rest
    let s0: &[u8] = &[0x84, 0x00, 12, 5, 0x86, 0xFF, 0x82];
    let d = module([s0, IDLE, IDLE, IDLE], &[&[0x8A]], &two_inst(), 2);
    let (l, base) = run(&d, 2);
    let t = tick_writes(&l, 0);
    let regs: Vec<(u32, u16)> = t.iter().map(|w| (w.0, w.1)).collect();
    let wd = 16 + crate::replay::be32(&d, 12) as usize;
    let sample = base + wd as u32 + 16; // directory of two entries, first sample
    assert_eq!(regs[0], (DMACON, 0x800F));
    assert_eq!(regs[1], (INTREQ, 0x0080));
    assert_eq!(regs[2], (DMACON, 0x0001));
    assert_eq!(regs[3], (0xDFF0A6, 1));
    assert_eq!(regs[4], (0xDFF0A8, 0));
    assert_eq!(regs[5], (INTREQ, 0x0080));
    assert_eq!(regs[6], (0xDFF0AA, 0));
    assert_eq!(regs[7], (0xDFF0AA, 0));
    assert_eq!(regs[8], (0xDFF0A0, (sample >> 16) as u16));
    assert_eq!(regs[9], (0xDFF0A2, sample as u16));
    assert_eq!(regs[10], (0xDFF0A4, 8));
    assert_eq!(regs[11], (0xDFF0A6, PERIODS[12]));
    assert_eq!(regs[12], (0xDFF0A8, 0x10));
    assert_eq!(regs[13], (DMACON, 0x8001));
    assert_eq!(regs[14], (INTREQ, 0x0080));
    assert_eq!(regs[15], (INTREQ, 0x0080));
    let lp = sample + 8;
    assert_eq!(regs[16], (0xDFF0A0, (lp >> 16) as u16));
    assert_eq!(regs[17], (0xDFF0A2, lp as u16));
    assert_eq!(regs[18], (0xDFF0A4, 4));
    assert_eq!(regs.len(), 19);
    // the fixed gaps of the cost model (cycles / 2, rounded down from the tick start)
    let c = |i: usize| t[i].2 - t[0].2;
    assert_eq!(c(2) - c(1), 6);
    assert_eq!(c(3) - c(2), 8);
    assert_eq!(c(4) - c(3), 8);
    assert_eq!(c(7) - c(6) >= 20, true);
    assert_eq!(c(9), c(8));
    assert_eq!(c(10) - c(9), 10);
    assert_eq!(c(11) - c(10), 10);
    assert_eq!(c(12) - c(11), 84);
    assert_eq!(c(13) - c(12), 12);
    assert_eq!(c(14) - c(13), 13);
    assert_eq!(c(17) - c(16), 0);
    assert_eq!(c(18) - c(17), 10);
}

#[test]
fn envelope_stages_follow_the_arithmetic() {
    // note 0 on channel 0 with instrument 0: start 0x10, attack +0x10, decay -8 to sustain 0x20,
    // hold 3 ticks, release -0x10
    let s0: &[u8] = &[0x84, 0x00, 0, 40, 0x86, 0xFF, 0x82];
    let d = module([s0, IDLE, IDLE, IDLE], &[&[0x8A]], &two_inst(), 2);
    let (l, _) = run(&d, 14);
    let vols: Vec<u16> = (1..14)
        .map(|k| tick_writes(&l, k).iter().find(|w| w.0 == 0xDFF0A8).map_or(0xFFFF, |w| w.1))
        .collect();
    // attack: 0x20, 0x30, 0x40 (clamped, decay next); decay: 0x38, 0x30, 0x28, 0x20 (sustain: hold);
    // hold: counter 3 -> 2, 1, 0 (release), release: 0x10, 0 (negative -> 0), then stopped 0
    assert_eq!(vols, vec![0x20, 0x30, 0x40, 0x38, 0x30, 0x28, 0x20, 0x20, 0x20, 0x20, 0x10, 0, 0]);
}

#[test]
fn an_instrument_without_hold_never_writes_the_volume_again() {
    let s0: &[u8] = &[0x84, 0x01, 0, 10, 0x86, 0xFF, 0x82];
    let d = module([s0, IDLE, IDLE, IDLE], &[&[0x8A]], &two_inst(), 2);
    let (l, _) = run(&d, 6);
    assert!(tick_writes(&l, 0).iter().any(|w| w.0 == 0xDFF0A8 && w.1 == 0x40));
    for k in 1..6 {
        assert!(!tick_writes(&l, k).iter().any(|w| w.0 == 0xDFF0A8), "tick {k}");
    }
}

#[test]
fn pattern_call_return_and_restart() {
    // pattern 0 holds a note and returns; the stream calls it twice, then restarts
    let pat: &[u8] = &[0, 2, 0x8A];
    let s0: &[u8] = &[0x84, 0x00, 0x88, 0x00, 0x88, 0x00, 0x82];
    let d = module([s0, IDLE, IDLE, IDLE], &[pat], &two_inst(), 2);
    let (l, _) = run(&d, 12);
    // a note every 2 ticks: ticks 0, 2, 4, ... each start INTREQ then DMA off
    let starts: Vec<usize> = (0..12).filter(|&k| tick_writes(&l, k).iter().any(|w| w.0 == DMACON && w.1 == 0x0001)).collect();
    assert_eq!(starts, vec![0, 2, 4, 6, 8, 10]);
}

#[test]
fn the_end_command_stops_the_tick_and_is_read_again() {
    // channel 1 ends on tick 1 (after one tick of rest); channel 2's turn is then skipped
    let s1: &[u8] = &[0x86, 0x01, 0x80];
    let s2: &[u8] = &[0x84, 0x00, 0, 1, 0x86, 0xFF, 0x82];
    let d = module([IDLE, s1, s2, IDLE], &[&[0x8A]], &two_inst(), 2);
    let (l, _) = run(&d, 4);
    // tick 0: channel 2 starts a note; tick 1: channel 1 reads the end command, channel 2 not served
    assert!(tick_writes(&l, 0).iter().any(|w| w.0 == DMACON && w.1 == 0x0004));
    let t1 = tick_writes(&l, 1);
    assert!(!t1.iter().any(|w| w.0 == 0xDFF0C8), "channel 2 was served: {t1:?}");
    // the tick is short: only the DMACON write
    assert_eq!(t1.len(), 1);
}

#[test]
fn song_end_is_reported_when_all_channels_ended() {
    let end: &[u8] = &[0x80];
    let d = module([end; 4], &[&[0x8A]], &two_inst(), 2);
    let mut e = Engine::new(48000.0);
    e.m.bus.write_bytes(ADDR, &d);
    let mut r = (OPEN.new)(&d, ADDR, &mut e.m).unwrap();
    assert!(!r.songend());
    // each tick ends at the first end command: four ticks of cnt wrap until all have read it
    for _ in 0..600 {
        let at = r.next_tick_at().unwrap();
        e.m.bus.clock = e.m.bus.clock.max(at);
        r.tick(&mut Hw { bus: &mut e.m.bus });
        if r.songend() {
            return;
        }
    }
    panic!("no song end");
}

#[test]
fn notes_at_or_above_46_get_period_zero() {
    let s0: &[u8] = &[0x84, 0x00, 46, 3, 0x86, 0xFF, 0x82];
    let d = module([s0, IDLE, IDLE, IDLE], &[&[0x8A]], &two_inst(), 2);
    let (l, _) = run(&d, 1);
    let t = tick_writes(&l, 0);
    assert!(t.iter().any(|w| w.0 == 0xDFF0A6 && w.1 == 0));
    assert_eq!(PERIODS[45], 127);
}

#[test]
fn a_note_without_an_instrument_writes_nothing() {
    let s0: &[u8] = &[5, 3, 0x86, 0xFF, 0x82];
    let d = module([s0, IDLE, IDLE, IDLE], &[&[0x8A]], &two_inst(), 2);
    let (l, _) = run(&d, 2);
    assert_eq!(tick_writes(&l, 0).len(), 1);
}

#[test]
fn the_instrument_quirk_adds_the_high_byte() {
    // an index of 32 reads the waveform directory 256 bytes further on (out of the module here:
    // zeros), so the sample start is the directory's base and the length 0
    let s0: &[u8] = &[0x84, 32, 0, 3, 0x86, 0xFF, 0x82];
    let d = module([s0, IDLE, IDLE, IDLE], &[&[0x8A]], &two_inst(), 2);
    let (l, base) = run(&d, 1);
    let t = tick_writes(&l, 0);
    let wd = 16 + crate::replay::be32(&d, 12) as u32;
    let lc: Vec<u16> = t.iter().filter(|w| w.0 == 0xDFF0A0 || w.0 == 0xDFF0A2).map(|w| w.1).collect();
    assert_eq!(lc.len(), 4);
    assert_eq!(((lc[0] as u32) << 16) | lc[1] as u32, base + wd);
}

#[test]
fn malformed_modules_do_not_panic_and_runs_are_deterministic() {
    let good = module([&[0x84, 0x00, 3, 4, 0x88, 0x00, 0x82], IDLE, IDLE, IDLE], &[&[7, 3, 0x8A]], &two_inst(), 2);
    // truncations at every length
    for n in (4..good.len()).step_by(7) {
        let _ = run(&good[..n], 20);
    }
    // header offsets far out of range, patterns calling themselves, endless restarts
    let mut bad = good.clone();
    bad[4..8].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
    bad[8..12].copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
    let _ = run(&bad, 20);
    let looping: &[u8] = &[0x82];
    let d = module([looping, &[0x88, 0x00], &[0x8E, 0x01], &[0x8A]], &[&[0x88, 0x00]], &two_inst(), 2);
    let _ = run(&d, 20);
    // pseudo-random bodies
    let mut x = 0x1234_5678u32;
    for _ in 0..30 {
        let mut m = b"FWMP".to_vec();
        for _ in 0..600 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            m.push((x >> 24) as u8);
        }
        m[4..8].copy_from_slice(&40u32.to_be_bytes());
        m[8..12].copy_from_slice(&80u32.to_be_bytes());
        m[12..16].copy_from_slice(&120u32.to_be_bytes());
        let _ = run(&m, 30);
    }
    let (a, _) = run(&good, 30);
    let (b, _) = run(&good, 30);
    assert_eq!(a, b);
}

#[test]
fn a_wait_that_never_ends_is_given_up() {
    // two notes on one channel in consecutive ticks restart a channel whose DMA is off; the
    // polls must end whatever Paula does
    let s0: &[u8] = &[0x84, 0x00, 1, 1, 2, 1, 3, 1, 0x86, 0xFF, 0x82];
    let d = module([s0, IDLE, IDLE, IDLE], &[&[0x8A]], &two_inst(), 2);
    let (l, _) = run(&d, 5);
    assert!(l.len() > 24);
}
