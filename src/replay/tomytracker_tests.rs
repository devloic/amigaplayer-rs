use super::*;
use m68k::CpuType;

/// A one-pattern module (positions: `positions`, pattern 0 only) whose first sample record is a
/// 16-word sample of volume 64 and whose cells are given by `cells` (row, channel, 4 bytes).
fn module(cells: &[(usize, usize, [u8; 4])], positions: usize) -> Vec<u8> {
    let s = PATTERNS + 1024;
    let mut d = vec![0u8; s + 64];
    let total = d.len() as u32;
    d[0..4].copy_from_slice(&total.to_be_bytes());
    d[4..8].copy_from_slice(&(s as u32).to_be_bytes());
    // sample 1: volume 64, start 0, 16 words, loop start 32, loop length 8
    d[8..10].copy_from_slice(&64u16.to_be_bytes());
    d[14..16].copy_from_slice(&16u16.to_be_bytes());
    d[16..20].copy_from_slice(&32u32.to_be_bytes());
    d[20..22].copy_from_slice(&8u16.to_be_bytes());
    d[442..444].copy_from_slice(&1u16.to_be_bytes());
    d[445] = positions as u8;
    for &(r, c, cell) in cells {
        let o = PATTERNS + 16 * r + 4 * c;
        d[o..o + 4].copy_from_slice(&cell);
    }
    d
}

fn player(d: &[u8]) -> TomyTracker {
    let (s, _) = header(d).unwrap();
    TomyTracker {
        d: d.to_vec(),
        sample_base: 0x20000 + s as u32,
        len: d[445] as usize,
        restart: 1,
        counter: 1,
        speed: 6,
        row: 0,
        pos: 0,
        dmabits: 0,
        port: 0,
        ch: [Chan::default(); 4],
        origin: 0,
        songend: false,
    }
}

const T0: u64 = 1_000_000;

fn run(r: &mut TomyTracker, ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    for k in 0..ticks {
        m.bus.clock = T0 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    m.bus.write_log.take().unwrap()
}

#[test]
fn detection() {
    let d = module(&[], 1);
    assert!(detect(&d));
    assert!(header(&d).is_some());
    // each rule of the specification
    let mut e = d.clone();
    e[445] = 0;
    assert!(!detect(&e), "length 0");
    e[445] = 128;
    assert!(!detect(&e), "length 128");
    let mut e = d.clone();
    e[444] = 1;
    assert!(!detect(&e));
    let mut e = d.clone();
    e[449] = 2; // largest position entry no longer the last pattern
    assert!(!detect(&e));
    let mut e = d.clone();
    e[7] = 2; // song size not on a pattern boundary
    assert!(!detect(&e));
    let mut e = d.clone();
    e[3] = 1; // odd total size
    assert!(!detect(&e));
    assert!(!detect(&d[..1728]));
    assert!(!detect(&[]));
    assert!(!detect(&[0u8; 4000]));
}

#[test]
fn no_other_replayer_claims_it() {
    let d = module(&[], 1);
    let names: Vec<&str> = crate::replay::REPLAYERS.iter().filter(|o| (o.detect)(&d)).map(|o| o.name).collect();
    assert_eq!(names, ["Tomy Tracker"]);
}

#[test]
fn note_start_writes() {
    // channel 0 plays note 13 (period 428) with instrument 1; the others are empty
    let d = module(&[(0, 0, [0, 0, 7, 26])], 1);
    let mut r = player(&d);
    let log = run(&mut r, 1);
    let rel: Vec<(u32, u16, u64)> = log.iter().map(|w| (w.0, w.1, w.2 - T0)).collect();
    let s = 0x20000 + (PATTERNS + 1024) as u32;
    // the specification's typical times for a row tick whose channel 0 starts a sample
    assert_eq!(rel[0], (0xDFF0A0, (s >> 16) as u16, 373));
    assert_eq!(rel[1], (0xDFF0A2, s as u16, 373));
    assert_eq!(rel[2], (0xDFF0A4, 16, 379));
    assert_eq!(rel[3], (0xDFF0A6, 428, 415));
    assert_eq!(rel[4], (0xDFF0A8, 64, 516));
    assert_eq!(rel[5], (0xDFF0B6, 0, 635));
    assert_eq!(rel[6], (0xDFF0B8, 0, 747));
    assert_eq!(rel[7], (0xDFF0C6, 0, 866));
    assert_eq!(rel[8], (0xDFF0C8, 0, 990));
    assert_eq!(rel[9], (0xDFF0D6, 0, 1108));
    assert_eq!(rel[10], (0xDFF0D8, 0, 1244));
    assert_eq!(rel[11], (0xDFF096, 0x0001, 1279));
    assert_eq!(rel[12].0, 0xDFF096);
    assert_eq!(rel[12].1, 0x8001);
    // the loop writes of channel 0: loop start and length of the sample
    assert_eq!(rel[13].0, 0xDFF0A0);
    assert_eq!(rel[15], (0xDFF0A4, 8, rel[13].2 + 6));
    // the beam waits are bounded by nine lines each
    assert!(rel[12].2 - rel[11].2 >= 2146 - 400 && rel[12].2 - rel[11].2 <= 2491);
    // five effect ticks without writes (no commands), then the next row
    let log = run(&mut player(&d), 7);
    assert_eq!(log.iter().filter(|w| w.0 == 0xDFF096).count(), 4);
}

#[test]
fn empty_row_times() {
    let d = module(&[], 1);
    let log = run(&mut player(&d), 1);
    let rel: Vec<u64> = log.iter().map(|w| w.2 - T0).collect();
    assert_eq!(log[0].0, 0xDFF0A6);
    // PER0 268, VOL0 369, PER1 487, VOL1 600, PER2 718, VOL2 842, PER3 961, VOL3 1096, DMACON 1131
    assert_eq!(&rel[..9], [268, 369, 487, 600, 718, 842, 961, 1096, 1131]);
}

#[test]
fn commands_do_not_panic_or_hang() {
    // every command byte and parameter on every channel, wild instruments and notes
    let mut cells = Vec::new();
    for r in 0..64usize {
        for c in 0..4usize {
            let x = (r * 4 + c) as u8;
            cells.push((r, c, [x.wrapping_mul(7), x.wrapping_mul(37), x.wrapping_mul(13), x.wrapping_mul(29)]));
        }
    }
    let d = module(&cells, 1);
    let mut r = player(&d);
    let log = run(&mut r, 600);
    assert!(!log.is_empty());
    // a short file reads zeros past its end
    let mut r = player(&d);
    r.d.truncate(1800);
    run(&mut r, 100);
}

#[test]
fn song_end_and_restart() {
    let d = module(&[], 1);
    let mut r = player(&d);
    run(&mut r, 6 * 63 + 1);
    assert!(r.songend());
    assert_eq!((r.pos, r.row), (0, 0));
    // restart beyond the song length restarts at the first position
    let mut r = player(&d);
    r.restart = 99;
    run(&mut r, 6 * 63 + 1);
    assert_eq!(r.pos, 0);
}

#[test]
fn filter_and_speed() {
    // filter 0x41 then speed 3 (command numbers 12 and 13, times 4)
    let d = module(&[(0, 0, [48, 0x41, 0, 0]), (1, 1, [52, 3, 0, 0])], 1);
    let mut r = player(&d);
    let log = run(&mut r, 8);
    let cia: Vec<(u32, u16)> = log.iter().filter(|w| w.0 == 0xBFE001).map(|w| (w.0, w.1)).collect();
    assert_eq!(cia, [(0xBFE001, 0), (0xBFE001, 0x41)]);
    // speed 0 is ignored
    let d = module(&[(0, 0, [52, 0, 0, 0])], 1);
    let mut r = player(&d);
    run(&mut r, 3);
    assert_eq!(r.speed, 6);
}

#[test]
fn deterministic() {
    let d = module(&[(0, 0, [0, 0, 7, 26]), (2, 1, [4 * 8, 0x12, 7, 40])], 1);
    let a = run(&mut player(&d), 40);
    let b = run(&mut player(&d), 40);
    assert_eq!(a, b);
}
