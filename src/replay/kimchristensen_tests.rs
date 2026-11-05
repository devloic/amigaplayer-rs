use super::*;
use m68k::CpuType;

const ORIGIN: u32 = 0x1000;
const POS: usize = 200;
const PAT: usize = 400;
const INS: usize = 1000;
const SMP: usize = 1400;

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A module built in code: signature words, positions, patterns, one instrument (number 1) and one
/// sample. `rows` lists (pattern, channel, row, note, instrument).
fn module(header: u32, positions: &[u8], rows: &[(usize, usize, usize, u8, u8)], ins: [u8; 32]) -> Vec<u8> {
    let mut d = vec![0u8; 1800];
    let a = |o: usize| ORIGIN + o as u32;
    d[0..2].copy_from_slice(&0x207Cu16.to_be_bytes());
    put32(&mut d, 2, a(POS));
    d[6..8].copy_from_slice(&0x0680u16.to_be_bytes());
    put32(&mut d, 8, a(PAT));
    put32(&mut d, 12, a(30));
    d[16..18].copy_from_slice(&0xE341u16.to_be_bytes());
    d[18..20].copy_from_slice(&0x227Cu16.to_be_bytes());
    put32(&mut d, 20, a(INS));
    d[24..26].copy_from_slice(&0x0680u16.to_be_bytes());
    put32(&mut d, 26, a(SMP));
    d[32..34].copy_from_slice(&0x0087u16.to_be_bytes());
    put32(&mut d, POS - 4, header);
    d[POS..POS + positions.len()].copy_from_slice(positions);
    for &(p, c, r, n, i) in rows {
        let o = PAT + p * 132 + 4 + c * 32 + r * 2;
        d[o] = n;
        d[o + 1] = i;
    }
    d[INS + 32..INS + 64].copy_from_slice(&ins);
    // one sample entry, its data right after the list
    put32(&mut d, SMP, a(SMP + 6));
    d[SMP + 4..SMP + 6].copy_from_slice(&10u16.to_be_bytes());
    d
}

/// Instrument: volume 0 -> 64 in 4 ticks, then hold; no pitch motion; no vibrato; sample 0.
fn ins_simple() -> [u8; 32] {
    let mut i = [0u8; 32];
    i[0] = 64;
    i[1] = 4;
    i[3] = 0;
    i[10] = 24;
    i[13] = 24;
    i[24..26].copy_from_slice(&5u16.to_be_bytes());
    i
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let mut r = KimChristensen::build(d, 0x10000).unwrap();
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    let log = m.bus.write_log.take().unwrap();
    (log, r.songend())
}

#[test]
fn detect_and_layout() {
    let d = module(0x0300_0002, &[0], &[], ins_simple());
    assert!(detect(&d));
    let l = layout(&d).unwrap();
    assert_eq!((l.origin, l.pos, l.pat, l.ins, l.smp), (ORIGIN, POS, PAT, INS, SMP));
    assert!(!detect(&d[..1799]));
    let mut e = d.clone();
    e[0] = 0;
    assert!(!detect(&e));
    assert!(!detect(&[]));
}

#[test]
fn sample_check_rejects_short_module() {
    let mut d = module(0, &[0], &[], ins_simple());
    put32(&mut d, SMP, ORIGIN + 0x7000);
    d[SMP + 6..SMP + 10].fill(0);
    assert!(KimChristensen::build(&d, 0).is_none());
    let mut d = module(0, &[0], &[], ins_simple());
    put32(&mut d, SMP, 0);
    assert!(KimChristensen::build(&d, 0).is_none());
}

#[test]
fn period_table_and_state_words() {
    let d = module(0x0301_0102, &[0], &[], ins_simple());
    let r = KimChristensen::build(&d, 0x10000).unwrap();
    assert_eq!(r.table(1), 0x87);
    assert_eq!(r.table(42), 0x5A0);
    assert_eq!(r.table(45), 0x0300 | 1);
    assert_eq!(r.table(46), 0x0102);
    assert_eq!(r.table(44), (0x10000 + POS as u32 - 8) as u16);
    assert_eq!(r.table(200), 0);
}

#[test]
fn div_rules() {
    assert_eq!(div(255 * 256, 1), 0xFF00);
    assert_eq!(div(64 * 256, 4), 4096);
    assert_eq!(div(-64 * 256, 3), (-5461i16) as u16);
    assert_eq!(div(1000, 0), 0);
}

#[test]
fn first_tick_writes_in_order() {
    let d = module(0x0200_0000, &[0], &[(0, 1, 0, 13, 1)], ins_simple());
    let (log, _) = run(&d, 1);
    let regs: Vec<u32> = log.iter().map(|w| w.0).collect();
    // channel 1 note start: LC (two words), LEN, VOL, PER; DMA off; VOL/PER of 4 channels; DMA on;
    // LC/LEN of the started channel
    assert_eq!(&regs[..5], &[0xDFF0B0, 0xDFF0B2, 0xDFF0B4, 0xDFF0B8, 0xDFF0B6]);
    assert_eq!(log[1].1, (0x10000 + 1406) as u16);
    assert_eq!(log[2].1, 5);
    assert_eq!(log[4].1, 0x10D);
    assert_eq!((regs[5], log[5].1), (0xDFF096, 0x0002));
    assert_eq!(regs[6], 0xDFF0A8);
    assert_eq!(regs.iter().filter(|&&r| r == 0xDFF096).count(), 2);
    let last = log.len() - 1;
    assert_eq!((log[last].0, log[last].1), (0xDFF0B4, 2));
    // times only move forward and stay inside the tick
    assert!(log.windows(2).all(|w| w[0].2 <= w[1].2));
    assert!(log[last].2 - 1_000_000 < 20_000);
    // the volume after one envelope step: 64/4 = 16 in 8.8, written as 16
    let v1: Vec<u16> = log.iter().filter(|w| w.0 == 0xDFF0B8).map(|w| w.1).collect();
    assert_eq!(v1, vec![0, 16]);
}

#[test]
fn raster_wait_ends_on_ninth_line_change() {
    let e = 70_000;
    let r = KimChristensen::raster_wait(e, 1000);
    let l0 = KimChristensen::line(e, 1000);
    assert_eq!(KimChristensen::line(e, r), l0 + 9);
}

#[test]
fn song_end_at_the_wrap_and_determinism() {
    // speed 0: a row every tick; repeat 0, last 0: one position of 16 rows
    let d2 = module(0x0001_0000, &[0], &[], ins_simple());
    let (a, ended_a) = run(&d2, 15);
    assert!(!ended_a);
    let (b, ended_b) = run(&d2, 16);
    assert!(ended_b);
    let (c, _) = run(&d2, 16);
    assert_eq!(b, c);
    assert!(b.len() > a.len());
}

#[test]
fn zero_header_uses_defaults() {
    let d = module(0, &[0], &[], ins_simple());
    let r = KimChristensen::build(&d, 0).unwrap();
    assert_eq!((r.speed, r.repeat, r.last), (2, 0, 0x77));
}

#[test]
fn skipped_positions_and_out_of_range_notes_do_not_panic() {
    let rows = [(0, 0, 0, 255, 255), (0, 1, 0, 200, 7), (0, 2, 0, 50, 1), (0, 3, 0, 43, 1)];
    let d = module(0x0100_0003, &[0xFF, 0xFF, 0, 0xFF], &rows, ins_simple());
    let (_, _) = run(&d, 100);
    // all positions skipped: bounded
    let d = module(0x0100_0000, &[0xFF; 256], &[], ins_simple());
    let (_, _) = run(&d, 5);
}

#[test]
fn malformed_input_does_not_panic() {
    let d = module(0x0100_0003, &[0], &[(0, 0, 0, 5, 1)], ins_simple());
    for n in [0, 10, 100, 1799] {
        let _ = detect(&d[..n]);
        let _ = KimChristensen::build(&d[..n], 0);
    }
    let mut x = 12345u32;
    for _ in 0..50 {
        let mut e = d.clone();
        for _ in 0..40 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = (x >> 8) as usize % e.len();
            e[o] = (x >> 24) as u8;
        }
        if let Some(mut r) = KimChristensen::build(&e, 0x10000) {
            let mut m = Machine::new(CpuType::M68000);
            for k in 0..40 {
                m.bus.clock = 1_000_000 + TICK * k;
                let mut hw = Hw { bus: &mut m.bus };
                r.tick(&mut hw);
            }
        }
    }
}
