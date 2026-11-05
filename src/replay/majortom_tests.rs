use super::*;
use m68k::CpuType;

const O: [usize; 13] = [0x40, 0x41, 0x42, 0x44, 0x54, 0x64, 0x74, 0xF4, 0x174, 0x1F4, 0x274, 0x2F4, 0xAF4];
const SIZE: usize = 0xAF4;

fn put(d: &mut [u8], at: usize, b: &[u8]) {
    d[at..at + b.len()].copy_from_slice(b);
}

/// A compact module: song length `len`, restart 0, the given speed, positions all pattern 0,
/// room for 2 patterns, 4 sequences per bank and 4 waveforms.
fn module(len: u8, speed: u16) -> Vec<u8> {
    let mut d = vec![0u8; SIZE];
    put(&mut d, 0, b"ARP.\x01\x00");
    for (i, o) in O.iter().enumerate() {
        put(&mut d, 6 + 4 * i, &(*o as u32).to_be_bytes());
    }
    d[O[0]] = len;
    put(&mut d, O[2], &speed.to_be_bytes());
    d
}

/// Instrument `i` using sequence `i` of every bank; each sequence is its speed and its steps.
fn instrument(d: &mut [u8], i: usize, vol: &[u8], wave: &[u8], arp: &[u8], pit: &[u8]) {
    put(d, O[5] + 4 * i, &[i as u8; 4]);
    for (b, s) in [vol, wave, arp, pit].into_iter().enumerate() {
        put(d, O[6 + b] + 64 * i, s);
    }
}

fn entry(d: &mut [u8], row: usize, voice: usize, e: [u8; 4]) {
    put(d, O[11] + row * 16 + 4 * voice, &e);
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let mut r = MajorTom::build(d, 0x10000, 0x30000);
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    let log = m.bus.write_log.take().unwrap();
    (log, r.songend())
}

fn regs(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

#[test]
fn detect_rules() {
    let d = module(1, 3);
    assert!(detect(&d));
    let mut e = d.clone();
    e[3] = b'X';
    assert!(!detect(&e));
    // an offset outside the file
    let mut e = d.clone();
    put(&mut e, 6 + 4 * 11, &(SIZE as u32 + 10).to_be_bytes());
    assert!(!detect(&e));
    assert!(!detect(&d[..20]));
    assert!(!detect(&[]));
    // the unused thirteenth offset may point past the end
    let mut e = d.clone();
    put(&mut e, 6 + 48, &0x10_0000u32.to_be_bytes());
    assert!(detect(&e));
}

#[test]
fn period_table_edges() {
    assert_eq!(table(0), 0x0D40);
    assert_eq!(table(87), 0x0070);
    assert_eq!(table(88), 0x4246);
    assert_eq!(table(89), u16::from_be_bytes([OVERRUN[0], OVERRUN[1]]));
    assert_eq!(table(103), u16::from_be_bytes([OVERRUN[28], OVERRUN[29]]));
    assert_eq!(table(180), 0x0076);
    assert_eq!(table(104), 0);
    assert_eq!(table(390), 0);
}

#[test]
fn first_row_on_the_fourth_tick_and_new_voice_waits_a_tick() {
    let mut d = module(1, 6);
    // note 0x31 (octave 3, semitone 1): period index 25, on voice 0 with instrument 1
    entry(&mut d, 0, 0, [0x31, 1, 0, 0]);
    instrument(&mut d, 1, &[1, 40], &[1, 1], &[1, 0], &[1, 0]);
    let (log, _) = run(&d, 4);
    assert!(regs(&log, 0xDFF0A6).is_empty(), "the voice starts on the next tick");
    let (log, _) = run(&d, 5);
    assert_eq!(regs(&log, 0xDFF0A6), vec![PERIODS[25]]);
    assert_eq!(regs(&log, 0xDFF0A4), vec![16]);
    assert_eq!(regs(&log, 0xDFF0A8), vec![40]);
    // the waveform address is the waveform sequence's value 1: module + 0x274 + 32
    assert_eq!(regs(&log, 0xDFF0A2).last().copied(), Some((0x10000 + 0x274 + 32) as u16));
}

#[test]
fn every_tick_ends_with_dma_off_then_on() {
    let d = module(1, 6);
    let (log, _) = run(&d, 3);
    let dma: Vec<u16> = regs(&log, 0xDFF096);
    assert_eq!(dma, vec![0, 0x800F, 0, 0x800F, 0, 0x800F]);
    // times: off, then on 10 colour clocks later
    let t: Vec<u64> = log.iter().filter(|w| w.0 == 0xDFF096).map(|w| w.2).collect();
    assert_eq!(t[1] - t[0], 10);
}

#[test]
fn noise_waveform() {
    let mut d = module(1, 6);
    entry(&mut d, 0, 1, [0x20, 1, 0, 0]);
    instrument(&mut d, 1, &[1, 30], &[1, 0xFF], &[1, 0], &[1, 0]);
    let (log, _) = run(&d, 6);
    let on: Vec<_> = log.iter().map(|w| (w.0, w.1)).collect();
    let i = on.iter().position(|w| *w == (0xDFF096, 0x0002)).expect("voice off");
    assert_eq!(on[i + 1].0, 0xDFF0B0);
    assert_eq!(on[i + 3], (0xDFF0B4, 700));
    assert_eq!(on[i + 4], (0xDFF0B6, 200));
    // the pitch step then leaves the length alone (noise): no 16-word length for voice 1
    assert!(regs(&log, 0xDFF0B4).iter().all(|&l| l != 16));
}

#[test]
fn song_end_after_the_last_pattern() {
    // speed 1: a row per tick, the first on tick 4; 64 rows of the only position
    let d = module(1, 1);
    let (_, e) = run(&d, 66);
    assert!(!e);
    let (_, e) = run(&d, 67);
    assert!(e);
}

#[test]
fn pattern_break_at_the_last_position_wraps_without_song_end() {
    let mut d = module(1, 1);
    entry(&mut d, 0, 0, [0, 0, 0x0D, 0]);
    let (log, e) = run(&d, 300);
    assert!(!e);
    assert!(!log.is_empty());
}

#[test]
fn speed_effect_changes_the_row_length() {
    let mut d = module(2, 1);
    entry(&mut d, 0, 0, [0, 0, 0x0F, 2]);
    // rows of two ticks from the second row on: the 64 rows of position 0 end later than at speed 1
    let (_, e) = run(&d, 67);
    assert!(!e);
    let (_, e) = run(&d, 140);
    assert!(!e);
    let (_, e) = run(&d, 4 + 2 * 128 + 2);
    assert!(e);
}

#[test]
fn start_writes_follow_the_ledger() {
    let mut d = module(2, 3);
    d[O[1]] = 0;
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    m.bus.clock = crate::engine::START_PHASE;
    let r = new(&d, 0x10000, &mut m);
    assert!(r.is_some());
    let log = m.bus.write_log.take().unwrap();
    // the host's two volume calls, then sound init: all four voices' registers, then DMA off
    assert_eq!(log[0], (0xDFF0A8, 0, 401));
    assert_eq!(log[7], (0xDFF0D8, 0, 1551));
    // three scanned bytes (all 0: 35 cycles each), then the ledger of the specification
    let t = 3852 + 3 * 35 + 40 + 16;
    let lc = log.iter().find(|w| w.0 == 0xDFF0A0).unwrap();
    assert_eq!(lc.2, 1600 + t / 2);
    assert_eq!(log.last().unwrap().0, 0xDFF096);
    assert_eq!(log.last().unwrap().1, 0x000F);
    // one period after sound init returns the first tick is due
    let end = 1600 + (3852 + 3 * 35 + 40 + 16 + 4 + 48 + 8 + 48 + 4 + 48 + 576 + 16 + 140) / 2;
    assert!(m.bus.clock >= end);
}

#[test]
fn malformed_input_does_not_panic() {
    // random bytes behind a valid magic and offsets, many ticks
    let mut seed = 0x1234_5678u32;
    for round in 0..8 {
        let mut d = module(1 + round as u8 * 3, 1 + (round as u16 % 3));
        for b in d.iter_mut().skip(0x44) {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (seed >> 24) as u8;
        }
        let _ = run(&d, 600);
    }
    // offsets that point at the end, a zero speed and an all-zero body
    let mut d = module(0, 0);
    put(&mut d, 6 + 4 * 11, &(SIZE as u32 - 1).to_be_bytes());
    let _ = run(&d, 600);
    // a jump in the waveform sequence reads far outside the module and reads zero
    let mut d = module(1, 1);
    entry(&mut d, 0, 0, [0x31, 1, 0, 0]);
    instrument(&mut d, 1, &[1, 40], &[1, 0xF0, 3, 0xFF, 0xFF, 0xFF], &[1, 0], &[1, 0]);
    put(&mut d, O[7] + 64 + 24, &[0x7F, 0xFF, 0xFF, 0xFF]);
    let _ = run(&d, 100);
}

#[test]
fn determinism() {
    let mut d = module(2, 2);
    entry(&mut d, 0, 0, [0x31, 1, 3, 4]);
    entry(&mut d, 1, 0, [0x34, 0, 1, 8]);
    instrument(&mut d, 1, &[2, 40, 30, 20], &[1, 1, 2, 3], &[1, 0, 4, 0x13], &[1, 0, 1, 0x12]);
    let a = run(&d, 500);
    let b = run(&d, 500);
    assert_eq!(a, b);
}
