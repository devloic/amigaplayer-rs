use super::*;
use m68k::CpuType;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

const PATA: usize = 0x2200;

/// A module in the first known layout: one subsong, two lines (the second is the end line),
/// pattern 1 on track 0 with the rows `rows`, an instrument 0 with a short envelope.
fn module(rows: &[u16]) -> Vec<u8> {
    let lay = LAYOUTS[0];
    let mut d = vec![0u8; 0x2400];
    for i in 0..5 {
        put16(&mut d, 4 * i, 0x6000);
        put16(&mut d, 4 * i + 2, 0x0100);
    }
    // target of the fifth slot: 18 + 0x0100
    put16(&mut d, 18 + 0x100, 0x23C8);
    put16(&mut d, 18 + 0x106, 0x4E75);
    for (i, p) in PERIODS.iter().enumerate() {
        put16(&mut d, lay.per + 2 * i, *p);
    }
    // line 0: track 0 pattern 1; line 1: the same
    d[lay.seq] = 1;
    d[lay.seq + 24] = 1;
    // subsong table: start 0, loop 0, end 1, speed 2, no mute; entry 1 repeats entry 0 of the
    // first array so that there is one subsong
    for (a, v) in [(0usize, 0u16), (1, 0), (2, 1), (3, 2), (4, 0)] {
        put16(&mut d, lay.sub + 0x20 * a, v);
    }
    put16(&mut d, lay.sub + 2, 0);
    put32(&mut d, lay.pat, LINK as u32 + PATA as u32);
    for (i, r) in rows.iter().enumerate() {
        put16(&mut d, PATA + 2 * i, *r);
    }
    put16(&mut d, PATA + 2 * rows.len(), 0xFFFF);
    let b = lay.ins;
    put32(&mut d, b, 0x100);
    put16(&mut d, b + 0x06, 16);
    put32(&mut d, b + 0x08, 0x200);
    put16(&mut d, b + 0x0E, 8);
    put16(&mut d, b + 0x18, 1);
    put16(&mut d, b + 0x1A, 4);
    put16(&mut d, b + 0x1C, 0);
    put16(&mut d, b + 0x1E, 0x20);
    put16(&mut d, b + 0x20, 1);
    put16(&mut d, b + 0x22, 0xFFFF);
    put16(&mut d, b + 0x26, 0x10);
    put16(&mut d, b + 0x28, 1);
    put16(&mut d, b + 0x2A, 0xFFFC);
    d
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let (lay, n) = locate(d).unwrap();
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let mut r = DynamicSynthesizer::build(d.to_vec(), lay, n, 0x10000);
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    (m.bus.write_log.take().unwrap(), r.songend())
}

#[test]
fn detects_the_known_layout() {
    let d = module(&[0x0100, 0, 0]);
    assert!(detect(&d));
    assert_eq!(locate(&d).unwrap().1, 1);
}

#[test]
fn rejects_other_layouts_and_drivers() {
    let d = module(&[0x0100]);
    // the period table moved: an unknown layout
    let mut e = d.clone();
    let lay = LAYOUTS[0];
    e.copy_within(lay.per..lay.per + 96, lay.per + 2);
    assert!(!detect(&e));
    // a broken slot
    let mut e = d.clone();
    put16(&mut e, 8, 0x6100);
    assert!(!detect(&e));
    let mut e = d.clone();
    put16(&mut e, 6, 0x0101);
    assert!(!detect(&e));
    // the fifth slot's target
    let mut e = d.clone();
    put16(&mut e, 18 + 0x106, 0x4E71);
    assert!(!detect(&e));
    // inconsistent subsong table
    let mut e = d.clone();
    put16(&mut e, lay.sub + 0x40, 500);
    assert!(!detect(&e));
    assert!(!detect(&d[..100]));
    assert!(!detect(&[]));
}

#[test]
fn note_start_writes_in_order() {
    // note 0x10 + 1 on instrument 0, then a rest
    let d = module(&[0x1000, 0, 0, 0]);
    let (w, _) = run(&d, 4);
    let regs: Vec<u32> = w.iter().take(8).map(|x| x.0).collect();
    // DMA off, LC high and low, LEN, PER, then the CIA write and the four volumes
    assert_eq!(regs, vec![0xDFF096, 0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF0A6, 0xBFE001, 0xDFF0A8, 0xDFF0B8]);
    assert_eq!(w[0].1, 1);
    assert_eq!(((w[1].1 as u32) << 16) | w[2].1 as u32, 0x10100);
    assert_eq!(w[3].1, 16);
    assert_eq!(w[4].1, PERIODS[0x11]);
    assert_eq!(w[1].2, w[2].2);
    // the first tick: 374 cycles to the track, 1769 for the start, 52 for the one replacement while
    // choosing, 6 * 18 for the six tracks that own voice 0 at the start; the tick is 11 cycles early
    assert_eq!(w[0].2, (2 * 1_000_000 - 11 + 374 + 1769 + 52 + 108) / 2);
}

#[test]
fn dma_is_turned_on_in_the_next_ticks_and_loops() {
    let d = module(&[0x1000, 0, 0, 0]);
    let (w, _) = run(&d, 5);
    let on: Vec<_> = w.iter().filter(|x| x.0 == 0xDFF096 && x.1 == 0x8201).collect();
    assert!(!on.is_empty());
    // the loop registers follow the DMA-on write
    let lp = w.iter().position(|x| x.0 == 0xDFF0A2 && x.1 == 0x0200 + 0x0000).is_some();
    assert!(lp || w.iter().any(|x| x.0 == 0xDFF0A4 && x.1 == 8));
}

#[test]
fn song_end_is_raised_when_the_final_line_loads() {
    // two rows per line: the end marker follows the second row; speed 2
    let d = module(&[0x1000, 0]);
    let (_, end) = run(&d, 2);
    assert!(!end);
    let (_, end) = run(&d, 12);
    assert!(end);
}

#[test]
fn volume_register_is_masked_to_seven_bits() {
    let mut d = module(&[0x1000, 0, 0, 0]);
    // start volume 0x7FF0 clamps to 0x40, a negative sustain wraps through the mask
    put16(&mut d, LAYOUTS[0].ins + 0x26, 0x8001);
    let (w, _) = run(&d, 40);
    assert!(w.iter().filter(|x| x.0 == 0xDFF0A8).all(|x| x.1 < 0x80));
}

#[test]
fn damaged_tables_never_hang_or_panic() {
    let base = module(&[0x1000, 0x8000, 0x1234, 0xFFFF, 0, 0x1100]);
    let lay = LAYOUTS[0];
    let mut seed = 12345u32;
    for _ in 0..200 {
        let mut d = base.clone();
        for _ in 0..20 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = lay.seq + (seed >> 8) as usize % (d.len() - lay.seq);
            d[o] = (seed >> 24) as u8;
        }
        if locate(&d).is_some() {
            run(&d, 300);
        }
    }
    // speed 0, and a pattern reference with no target
    let mut d = base.clone();
    put16(&mut d, lay.sub + 0x60, 0);
    run(&d, 100);
    let mut d = base;
    d[lay.seq + 4] = 5;
    put32(&mut d, lay.pat + 16, 0);
    d[lay.seq + 8] = 2;
    put32(&mut d, lay.pat + 4, 0xFFFF_0000);
    run(&d, 100);
}

#[test]
fn deterministic() {
    let d = module(&[0x1000, 0, 0x8000, 0x1210]);
    assert_eq!(run(&d, 60).0, run(&d, 60).0);
}

#[test]
fn a_subsong_change_writes_one_init_group_per_step() {
    let d = module(&[0x1000, 0]);
    let (lay, n) = locate(&d).unwrap();
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let mut r = DynamicSynthesizer::build(d, lay, n, 0x10000);
    m.bus.clock = 5000;
    r.set_subsong(0, &mut Hw { bus: &mut m.bus });
    let w = m.bus.write_log.take().unwrap();
    assert_eq!(w.len(), 21);
    assert_eq!((w[0].0, w[0].1), (0xDFF096, 0x000F));
    assert_eq!(w[5].2, 5000 + 82);
    assert_eq!(r.subsong_tick_at(), Some(5000 + 70926 + 1572));
}
