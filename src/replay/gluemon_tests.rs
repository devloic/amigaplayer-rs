use super::*;
use m68k::CpuType;

/// A module: speed, rows, order list (with its end marker), drum mode flag and the pattern bytes.
fn module(speed: u8, rows: u8, orders: &[u8], drum: bool, pats: &[&[u8]]) -> Vec<u8> {
    let mut m = HEAD.to_vec();
    m.extend_from_slice(b"TEST\0\0\0\0");
    for v in 0..4 {
        m.extend_from_slice(&[64, 4, 0].map(|b| if v == 3 { b } else { b }));
    }
    if drum {
        m[0x1A] = 0x52;
        m[0x1B] = 0x4D;
    }
    m.extend_from_slice(&[speed, rows]);
    for v in 0..4u8 {
        m.extend((0..32).map(|i| i * 4 + v));
    }
    m.push(orders.len() as u8);
    m.extend_from_slice(orders);
    for p in pats {
        m.extend_from_slice(p);
    }
    m
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let noise = 0x30000;
    let mut r = GlueMon::build(d, 0x10000, noise).unwrap();
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    let log = m.bus.write_log.take().unwrap();
    (log, r.songend())
}

#[test]
fn detect_rules() {
    let d = module(2, 2, &[0, 0xFF], false, &[&[0xFF; 8]]);
    assert!(detect(&d));
    let mut e = d.clone();
    e[4] = 0;
    assert!(!detect(&e));
    assert!(!detect(&d[..0x9F]));
    assert!(!detect(&[]));
}

#[test]
fn bad_speed_or_rows_not_a_module() {
    let d = module(0, 2, &[0, 0xFF], false, &[&[0xFF; 8]]);
    assert!(GlueMon::build(&d, 0, 0).is_none());
    let d = module(2, 0, &[0, 0xFF], false, &[]);
    assert!(GlueMon::build(&d, 0, 0).is_none());
}

#[test]
fn drum_wave_shape() {
    let w = drum_wave();
    assert_eq!(&w[..4], &[0x7F, 0x7F, 0x81, 0x81]);
    assert_eq!(w[4], 0);
    assert_eq!(w[23], 190);
    assert_eq!(w[24], 180);
    assert_eq!(w[31], 40);
}

#[test]
fn first_tick_note_and_volumes() {
    // one row: note byte 24 (period index 12) on voice 0, nothing elsewhere
    let d = module(3, 1, &[0, 0xFF], false, &[&[24, 0xFF, 0xFF, 0xFE]]);
    let (log, _) = run(&d, 1);
    let per: Vec<_> = log.iter().filter(|w| w.0 == 0xDFF0A6).collect();
    assert_eq!(per.len(), 1);
    assert_eq!(per[0].1, 1712);
    // the first tick's volumes: voice 0 at the start volume, the rest silent
    let vols: Vec<u16> = log.iter().filter(|w| w.0 & 0xF & 0xFFFF == 8 && w.0 >= 0xDFF0A0).map(|w| w.1).collect();
    assert_eq!(vols, vec![64, 0, 0, 0]);
}

#[test]
fn decay_wraps_like_the_original() {
    let mut d = module(1, 2, &[0, 0xFF], false, &[&[2, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]]);
    d[0x10] = 5;
    d[0x11] = 7;
    d[0x12] = 0;
    let (log, _) = run(&d, 2);
    let v0: Vec<u16> = log.iter().filter(|w| w.0 == 0xDFF0A8).map(|w| w.1).collect();
    // start 5, then 5 - 7 wraps to 65534 (written as 1022)
    assert_eq!(v0, vec![5, 1022]);
}

#[test]
fn song_end_on_second_wrap_and_determinism() {
    let d = module(1, 1, &[0, 0xFF], false, &[&[0xFF; 4]]);
    let (a, ended_a) = run(&d, 1);
    assert!(!ended_a);
    let (b, ended_b) = run(&d, 2);
    assert!(ended_b);
    let (c, _) = run(&d, 2);
    assert_eq!(b, c);
    assert!(b.len() > a.len());
}

#[test]
fn drum_restart_writes() {
    // drum kind 0xC8 on the first row: DMA off, LC at the drum wave, DMA on, then the sweep
    let d = module(2, 1, &[0, 0xFF], true, &[&[0xFF, 0xFF, 0xFF, 0xC8]]);
    let (log, _) = run(&d, 1);
    let seq: Vec<(u32, u16)> = log.iter().map(|w| (w.0, w.1)).collect();
    let i = seq.iter().position(|w| *w == (0xDFF096, 0x0008)).unwrap();
    assert_eq!(seq[i + 1], (0xDFF0D0, 3));
    assert_eq!(seq[i + 2], (0xDFF0D2, 0x0020));
    assert_eq!(seq[i + 3], (0xDFF096, 0x8008));
    // the sweep: 0x0710 - 0xFF01 = 0x080F
    assert_eq!(seq[i + 4], (0xDFF0D6, 0x080F));
}

#[test]
fn malformed_input_does_not_panic() {
    let mut d = module(1, 255, &[0, 1, 2, 3, 4, 0xFF], false, &[&[0xFF, 200, 0xFD, 99]]);
    d.truncate(0xA5);
    let (_, _) = run(&d, 600);
    let mut d = module(1, 4, &[0xFF], true, &[&[0x10, 0xD0, 0xFE, 0xC9]]);
    d[0x9E] = 0;
    let (_, _) = run(&d, 100);
    let mut d = module(1, 1, &[0x80, 0xFF], false, &[]);
    d[0x9E] = 0xFF;
    let (_, _) = run(&d, 100);
}
