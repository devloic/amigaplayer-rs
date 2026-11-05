use super::*;
use m68k::CpuType;

fn put16(d: &mut Vec<u8>, v: u16) {
    d.extend_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut Vec<u8>, v: u32) {
    d.extend_from_slice(&v.to_be_bytes());
}

fn chunk(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = tag.to_vec();
    c.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    c.extend_from_slice(body);
    c
}

/// A module with one subsong of speed `speed` and voice masks `active` / `song`. Every voice's
/// order list is pattern `pat` then the end marker; pattern 0 is `rows` followed by the end byte.
/// Instrument 0: envelope 0 (volume 0x40), sample 0 (4 words, loop at byte 8, 2 words).
fn module(speed: u8, active: u8, song: u8, rows: &[u8], npat: u8) -> Vec<u8> {
    let mut s: Vec<u8> = Vec::new();
    // 13 header words, patched below
    for _ in 0..13 {
        put16(&mut s, 0);
    }
    let set = |s: &mut Vec<u8>, i: usize| {
        let v = s.len() as u16;
        s[2 * i..2 * i + 2].copy_from_slice(&v.to_be_bytes());
    };
    // voice table: four longs, relative to the table itself, order lists of two bytes each
    set(&mut s, 0);
    for v in 0..4 {
        put32(&mut s, 16 + 2 * v);
    }
    for _ in 0..4 {
        s.push(npat);
        s.push(0xFF);
    }
    set(&mut s, 1); // restart table: restart at 0
    put32(&mut s, 0);
    set(&mut s, 2); // subsong table
    s.extend_from_slice(&[active, 0, song, speed]);
    set(&mut s, 3);
    set(&mut s, 5); // pattern table: pattern 0 right after the one long, 1 and up after it
    put32(&mut s, 4);
    s.extend_from_slice(rows);
    s.push(0xFF);
    if s.len() & 1 != 0 {
        s.push(0);
    }
    set(&mut s, 6); // instruments
    s.extend_from_slice(&[0, 0, 0, 0, 0xFF, 0, 0, 0]);
    set(&mut s, 7); // envelope: volume 0x40, step 0, count 0, next 0
    s.extend_from_slice(&[0x40, 0, 0, 0]);
    set(&mut s, 8); // command words: 0 = tempo 5, 1 = fade, 2 = slide up 3
    put16(&mut s, 0x2005);
    put16(&mut s, 0x4000);
    put16(&mut s, 0x1003);
    set(&mut s, 9);
    put32(&mut s, 0);
    set(&mut s, 10);
    put16(&mut s, 4);
    set(&mut s, 11);
    put32(&mut s, 8);
    set(&mut s, 12);
    put16(&mut s, 2);
    let mut d = chunk(b"mpl1", &[0x60, 0x00, 0, 0, 0, 0, 0, 0]);
    d.extend(chunk(b"mdt1", &s));
    d.extend(chunk(b"msm1", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]));
    d
}

/// One row: instrument 0 with a note, holding for one more row.
const ROW: [u8; 3] = [0x81, 0x00, 0x05];

fn start(d: &[u8], base: u32) -> (Machine, Box<dyn Replayer>) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let r = (OPEN.new)(d, base, &mut m).expect("module accepted");
    m.bus.write_log = Some(Vec::new());
    (m, r)
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let (mut m, mut r) = start(d, 0x10000);
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    (m.bus.write_log.take().unwrap(), r.songend())
}

#[test]
fn detection() {
    let d = module(2, 15, 15, &ROW, 0);
    assert!(detect(&d));
    // a tag, an odd size in the first two chunks, a file shorter than its chunks
    for (o, v) in [(0usize, b'x'), (16, b'x'), (23, 0x01), (7, 0x09)] {
        let mut e = d.clone();
        e[o] = v;
        assert!(!detect(&e), "{o}");
    }
    assert!(!detect(&d[..d.len() - 1]));
    assert!(!detect(&d[..20]));
    assert!(!detect(&[]));
    // the fourth tag byte is not looked at
    let mut e = d.clone();
    e[3] = b'2';
    assert!(detect(&e));
}

#[test]
fn malformed_input_is_safe() {
    let d = module(2, 15, 15, &ROW, 0);
    // every truncation of a valid module that still passes detection, and every byte of the song
    // data replaced, must play without panicking
    let mut m = Machine::new(CpuType::M68000);
    for cut in 0..d.len() {
        if let Some(mut r) = (OPEN.new)(&d[..cut], 0x10000, &mut m) {
            for k in 0..4 {
                m.bus.clock += TICK * k;
                r.tick(&mut Hw { bus: &mut m.bus });
            }
        }
    }
    for o in (40..d.len()).step_by(3) {
        for v in [0x00, 0x7F, 0xFE, 0xFF] {
            let mut e = d.clone();
            e[o] = v;
            if let Some(mut r) = (OPEN.new)(&e, 0x10000, &mut m) {
                for k in 0..40 {
                    m.bus.clock += TICK * k;
                    r.tick(&mut Hw { bus: &mut m.bus });
                }
            }
        }
    }
}

#[test]
fn start_block_and_first_ticks() {
    let d = module(2, 15, 15, &ROW, 0);
    let (mut m, mut r) = start(&d, 0x10000);
    // from a fresh machine: the host's eight zero volumes, DMACON, four volumes
    let mut m2 = Machine::new(CpuType::M68000);
    m2.bus.write_log = Some(Vec::new());
    let _ = (OPEN.new)(&d, 0x10000, &mut m2).unwrap();
    let log = m2.bus.write_log.take().unwrap();
    assert_eq!(log.len(), 13);
    assert_eq!((log[8].0, log[8].1), (0xDFF096, 0x000F));
    assert!(log[9..].iter().all(|w| w.1 == 0));
    // the first tick comes 74267 colour clocks after the last start write
    assert_eq!(r.tick_clocks() + r.timer_delay() + ORIGIN, 74267);
    assert_eq!(log[12].2 - log[8].2, 40);
    // tick 0: DMA on, then period and volume of the four voices, all zero (nothing started yet)
    m.bus.clock = 1_000_000;
    r.tick(&mut Hw { bus: &mut m.bus });
    let t0 = m.bus.write_log.take().unwrap();
    let regs: Vec<u32> = t0.iter().map(|w| w.0).collect();
    assert_eq!(regs, [0xDFF096, 0xDFF0A6, 0xDFF0A8, 0xDFF0B6, 0xDFF0B8, 0xDFF0C6, 0xDFF0C8, 0xDFF0D6, 0xDFF0D8]);
    assert_eq!(t0[0].1, 0x800F);
    // the periods are zero; voices 2 and 3, not yet stepped, have already fetched the envelope's volume
    assert!(t0[1..].iter().filter(|w| w.0 & 15 == 6).all(|w| w.1 == 0));
    let vols: Vec<u16> = t0.iter().filter(|w| w.0 & 15 == 8).map(|w| w.1).collect();
    assert_eq!(vols, [0, 0, 0x40, 0x40], "{t0:x?}");
    // the first write: entry 242 + prologue 76 + phase 0 130 + two idle-free voice steps, DMA on
    assert!(t0[0].2 > 1_000_000 && t0[0].2 < 1_000_000 + 4000);
}

#[test]
fn row_end_writes_pointers_then_period_and_volume() {
    let d = module(2, 15, 15, &ROW, 0);
    let (log, _) = run(&d, 2);
    // song data and sample data places: chunk 1 is 16 bytes
    let c2 = 16usize;
    let c2len = u32::from_be_bytes([d[c2 + 4], d[c2 + 5], d[c2 + 6], d[c2 + 7]]) as usize;
    let smp = 0x10000u32 + (c2 + c2len + 8) as u32;
    let tick1: Vec<_> = log.iter().filter(|w| w.2 >= 1_000_000 + TICK).collect();
    // the last tick of the row switches off the voices that start a sample
    assert_eq!((tick1[0].0, tick1[0].1), (0xDFF096, 0x000F));
    let mut i = 1;
    for v in 0..4u32 {
        assert_eq!((tick1[i].0, tick1[i].1), (0xDFF0A4 + 16 * v, 4));
        assert_eq!((tick1[i + 1].0, tick1[i + 1].1), (0xDFF0A0 + 16 * v, (smp >> 16) as u16));
        assert_eq!((tick1[i + 2].0, tick1[i + 2].1), (0xDFF0A2 + 16 * v, smp as u16));
        i += 3;
    }
    for v in 0..4u32 {
        assert_eq!((tick1[i].0, tick1[i].1), (0xDFF0A6 + 16 * v, PERIODS[5]));
        assert_eq!((tick1[i + 1].0, tick1[i + 1].1), (0xDFF0A8 + 16 * v, 0x40));
        i += 2;
    }
    assert_eq!(i, tick1.len());
    // pointer writes of one voice share a clock, and later writes are later
    assert_eq!(tick1[2].2, tick1[3].2);
    assert!(tick1.windows(2).all(|w| w[0].2 <= w[1].2));
}

#[test]
fn deterministic() {
    let d = module(3, 15, 15, &ROW, 0);
    assert_eq!(run(&d, 30), run(&d, 30));
}

#[test]
fn song_end_when_every_voice_restarts() {
    let d = module(2, 15, 15, &ROW, 0);
    let (_, end) = run(&d, 4);
    assert!(!end);
    let (_, end) = run(&d, 12);
    assert!(end);
    // a voice that is not active never ends, so the song never ends
    let d = module(2, 7, 15, &ROW, 0);
    assert!(!run(&d, 60).1);
}

#[test]
fn speed_one_never_steps_voices_two_and_three() {
    let d = module(1, 15, 15, &ROW, 0);
    let (log, _) = run(&d, 3);
    // a row per tick; only voices 0 and 1 ever get a pointer write
    assert!(log.iter().any(|w| w.0 == 0xDFF0A4));
    assert!(log.iter().any(|w| w.0 == 0xDFF0B4));
    assert!(!log.iter().any(|w| w.0 == 0xDFF0C4 || w.0 == 0xDFF0D4));
}

#[test]
fn three_voice_mask_outputs_only_the_song_mask() {
    let d = module(2, 7, 7, &ROW, 0);
    let (log, _) = run(&d, 2);
    assert!(!log.iter().any(|w| w.0 == 0xDFF0D6 || w.0 == 0xDFF0D8));
    assert!(log.iter().any(|w| w.0 == 0xDFF0C6));
}

#[test]
fn command_rows_and_notes_out_of_range() {
    // a tempo command row (F = 0x41, command 0), then a note row with index 100 (above the table),
    // a tie note, a fade command and a slide command
    let rows = [0x41, 0x00, 0x01, 100, 0x00, 0xA5, 0x41, 0x01, 0x41, 0x02];
    let d = module(2, 15, 15, &rows, 0);
    let (log, _) = run(&d, 40);
    assert!(!log.is_empty());
    // the out-of-table note takes the last ordinary period
    assert!(log.iter().any(|w| w.0 == 0xDFF0A6 && w.1 == 0x050));
}

#[test]
fn out_of_limit_pattern_count_is_safe() {
    // an order list naming pattern 100 and 200, far past the one pattern table entry
    for npat in [100u8, 127] {
        let d = module(2, 15, 15, &ROW, npat);
        let (log, _) = run(&d, 300);
        assert!(!log.is_empty());
    }
    let mut d = module(2, 15, 15, &ROW, 0);
    // switch voice 0's order list to a transpose entry followed by pattern 200
    // chunk 1 (16), chunk header (8), 26 bytes of header words, 16 bytes of voice table
    let pos = 16 + 8 + 26 + 16;
    d[pos] = 0x85;
    d[pos + 1] = 200;
    let (log, _) = run(&d, 300);
    assert!(!log.is_empty());
}

#[test]
fn subsong_is_clamped_and_restarts() {
    let d = module(2, 15, 15, &ROW, 0);
    let (mut m, mut r) = start(&d, 0x10000);
    assert_eq!(r.subsongs(), (0, 0, 0));
    m.bus.clock = 5_000_000;
    r.set_subsong(9, &mut Hw { bus: &mut m.bus });
    assert_eq!(r.subsongs().2, 0);
    assert_eq!(r.subsong_tick_at(), Some(5_000_050 + 74267));
}
