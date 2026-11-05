use super::*;
use m68k::CpuType;

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A module with speed `speed`: position 0 plays track 1 (`rows`, then empty rows) on voice 0 and
/// the empty track 0 on the others; position 1 ends the order list. Sample slot 0 is an object
/// with a loop (first part 0x100, loop 0x40 bytes), slot 1 has none.
fn module(speed: u32, rows: &[[u8; 3]]) -> Vec<u8> {
    let tb = 0x1010 + 48 * 2;
    let mut d = vec![0u8; tb + 0x280 + 0x400];
    put32(&mut d, 0, speed);
    put32(&mut d, 8, 0);
    put32(&mut d, 0x0C, 0x200);
    // position 0: voice 0 track 1, the others track 0; position 1: end of the list
    d[0x10 + 1] = 1;
    d[0x10 + 8..0x10 + 10].copy_from_slice(&[0xFF, 0xFF]);
    for (i, r) in rows.iter().enumerate() {
        let o = 0x1010 + 48 + 3 * i;
        d[o..o + 3].copy_from_slice(r);
    }
    for i in 0..2 {
        put32(&mut d, tb + 4 * i, 0x280 + 0x200 * i as u32);
        put32(&mut d, tb + 0x80 + 4 * i, 0x200);
        put32(&mut d, tb + 0x100 + 4 * i, 0x280 + 0x68 + 0x200 * i as u32);
        put32(&mut d, tb + 0x180 + 4 * i, 0x100);
    }
    put32(&mut d, tb + 0x200, 0x40);
    d[tb + 0x280..tb + 0x284].copy_from_slice(b"FORM");
    d
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let l = layout(d).unwrap();
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let mut r = SpeedyA1System::build(d, &l, 0x10000, 0x20000, 0);
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    (m.bus.write_log.take().unwrap(), r.songend())
}

#[test]
fn detection() {
    let d = module(2, &[[0x31, 0x11, 0]]);
    assert!(detect(&d));
    let tb = 0x1010 + 96;
    assert_eq!(layout(&d).unwrap().tb, tb);
    // each condition in turn
    let mut e = d.clone();
    e[1] = 1;
    assert!(!detect(&e));
    let mut e = d.clone();
    put32(&mut e, 0x0C, 0x100);
    assert!(!detect(&e));
    let mut e = d.clone();
    e[tb + 0x280] = b'X';
    assert!(!detect(&e));
    // a track number of 256 or more before the terminator
    let mut e = d.clone();
    e[0x10 + 2] = 1;
    assert!(!detect(&e));
    // zero padding before the tables is skipped
    let mut e = d[..tb].to_vec();
    e.extend_from_slice(&[0u8; 8]);
    e.extend_from_slice(&d[tb..]);
    assert!(detect(&e));
    assert!(!detect(&[]));
    assert!(!detect(&d[..0x1000]));
    assert!(!detect(&d[..tb + 0x283]));
}

#[test]
fn unterminated_order_list_is_scanned_to_its_end() {
    let mut d = module(0, &[]);
    d[0x10 + 8..0x10 + 10].copy_from_slice(&[0, 0]);
    assert!(detect(&d));
    let (w, _) = run(&d, 40);
    assert!(!w.is_empty());
}

#[test]
fn raster_wait_is_bounded_and_nine_lines_long() {
    for rel in [0u64, 1, 100, 226, 227, 70000, 123_456_789] {
        let clk = Clk { t0: 1_000_000, rel };
        let x = clk.wait(1000);
        // at least nine line changes, and not more than ten lines plus the fixed costs
        assert!(x >= 1000 + 28 + 9 * 70, "{x}");
        assert!(x <= 1000 + 28 + 10 * 2 * 227 + 9 * 60 + 44, "{x}");
    }
}

#[test]
fn first_tick_writes_a_note_in_the_specified_order() {
    // an envelope note (0x31 = octave 3, note 1), sample 0
    let d = module(2, &[[0x31, 0x11, 0]]);
    let (w, _) = run(&d, 1);
    let regs: Vec<u32> = w.iter().map(|x| x.0).collect();
    // DMA off, parking (LC two words, LEN, VOL, PER), the note (LC two, LEN, PER), DMA on, then the
    // voice updates: voice 0 idle-less attack with a loop (LC two, LEN, VOL), voices 1..3 idle
    let head = [0xDFF096, 0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF0A8, 0xDFF0A6, 0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF0A6, 0xDFF096];
    assert_eq!(&regs[..head.len()], &head);
    assert_eq!(w[0].1, 1);
    assert_eq!(w[10].1, 0x8201);
    assert_eq!(w[4].1, 0);
    assert_eq!(w[5].1, 0xCA);
    // the note: period of octave 3 note 1, length (first part + loop) / 2 words
    assert_eq!(w[9].1, 0x1AB);
    assert_eq!(w[8].1, (0x100 + 0x40) / 2);
    // clocks never run backwards
    assert!(w.windows(2).all(|p| p[0].2 <= p[1].2));
    // the update: loop reload, then a volume write of 0 (attack starts at volume 0, timer non-zero)
    let upd = &w[11..];
    assert_eq!(upd[0].0, 0xDFF0A0);
    assert_eq!(upd[2].0, 0xDFF0A4);
    assert_eq!(upd[2].1, 0x40 / 2);
    assert_eq!(upd[3], (0xDFF0A8, 0, upd[3].2));
}

#[test]
fn idle_voices_rewrite_the_silent_sample_and_volume_every_tick() {
    let d = module(2, &[]);
    let (w, _) = run(&d, 2);
    // light tick: four voices, each LC (two words), LEN 5, VOL 0
    let last: Vec<_> = w.iter().rev().take(16).collect();
    assert_eq!(last.iter().filter(|x| x.0 == 0xDFF0A4 || x.0 == 0xDFF0B4 || x.0 == 0xDFF0C4 || x.0 == 0xDFF0D4).count(), 4);
    assert!(last.iter().all(|x| x.0 & 0xF != 0 || x.1 == 0 || x.1 == 2));
}

#[test]
fn song_end_by_order_entry_and_row_byte() {
    // the empty track, then position 1 ends the list: 16 rows of speed 0 later
    let d = module(0, &[]);
    let (_, end) = run(&d, 16);
    assert!(!end);
    let (_, end) = run(&d, 17);
    assert!(end);
    // a row byte 0xFF at row 3
    let d = module(0, &[[0, 0, 0], [0, 0, 0], [0, 0, 0], [0xFF, 0, 0]]);
    let (_, end) = run(&d, 3);
    assert!(!end);
    let (_, end) = run(&d, 4);
    assert!(end);
}

#[test]
fn slide_command_changes_the_period_every_tick() {
    let d = module(5, &[[0x31, 0x11, 0], [0x50, 0, 4]]);
    // speed 5: the command row follows after six ticks; the first note's period is 0x1AB
    let (w, _) = run(&d, 9);
    let pers: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    assert_eq!(pers[..2], [0xCA, 0x1AB]);
    assert_eq!(pers[2..], [0x1AF, 0x1B3, 0x1B7]);
}

#[test]
fn volume_envelope_steps_by_eight() {
    // attack value map[1] = 1 (a step every tick), release value 0 is never reached in this run
    let d = module(20, &[[0x31, 0x10, 0]]);
    let (w, _) = run(&d, 6);
    let vols: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A8).map(|x| x.1).collect();
    // the parking write, then one volume per tick: 0 (timer set to 1 at the start), 0, 8, 16 ...
    assert_eq!(vols[0], 0);
    assert!(vols[1..].windows(2).all(|p| p[1] >= p[0]));
    assert_eq!(*vols.last().unwrap(), 40);
}

#[test]
fn malformed_input_does_not_panic() {
    let mut s = 0x1234_5678u32;
    let base = module(3, &[[0x31, 0x11, 0], [0x52, 0, 0], [0x51, 0xFF, 0xFF], [0x20, 0x01, 0x25], [0x44, 0xFF, 0x1F]]);
    for round in 0..40 {
        let mut d = base.clone();
        for _ in 0..60 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = (s >> 8) as usize % d.len();
            // keep the detection's structure for most rounds
            if o >= 0x1010 || round % 4 == 0 {
                d[o] = (s >> 24) as u8;
            }
        }
        if layout(&d).is_some() {
            run(&d, 60);
        }
    }
    // table values that overflow
    let mut d = base.clone();
    let tb = layout(&d).unwrap().tb;
    put32(&mut d, tb + 0x80, 0xFFFF_FFF0);
    put32(&mut d, tb + 0x180, 0xFFFF_FFFF);
    put32(&mut d, tb + 0x200, 0xFFFF_FFFF);
    run(&d, 80);
}

#[test]
fn deterministic() {
    let d = module(1, &[[0x31, 0x11, 0], [0x24, 0x35, 1], [0x52, 0, 0]]);
    assert_eq!(run(&d, 50).0, run(&d, 50).0);
}
