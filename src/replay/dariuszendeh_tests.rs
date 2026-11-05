use super::*;
use m68k::CpuType;

const SONG: usize = 0x100;
const REC: usize = 0x200;
const PERT: usize = 0x480;
const SMP: usize = 0x400;
const DATA: usize = 0x500;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn addr(d: &mut [u8], o: usize, op: u16, tgt: usize) {
    put16(d, o, op);
    put16(d, o + 2, (tgt as i64 - (o as i64 + 2)) as i16 as u16);
}

/// A variant B module: 3 steps per row, 2 song rows. Record 1 step 1 plays sample 1 with
/// base index 2, volume 0 (plays at 1), flag 0 (retrigger).
fn module() -> Vec<u8> {
    let mut d = vec![0u8; 0x600];
    put16(&mut d, 0, 0x48E7);
    put16(&mut d, 2, 0x00F0);
    addr(&mut d, 4, 0x41FA, 0x100);
    put16(&mut d, 8, 0x4CD8);
    put16(&mut d, 10, 0x0600);
    put16(&mut d, 12, 0x4A00);
    put16(&mut d, 0x10, 0x0C00);
    put16(&mut d, 0x12, 0x00FF);
    addr(&mut d, 0x14, 0x41FA, SONG);
    d[0x18..0x24].copy_from_slice(&SCAN);
    addr(&mut d, 0x24, 0x45FA, REC);
    put16(&mut d, 0x28, 0xC0FC);
    put16(&mut d, 0x2A, 0x0062);
    put16(&mut d, 0x2C, 0xE742);
    addr(&mut d, 0x30, 0x41FA, SMP);
    addr(&mut d, 0x34, 0x41FA, DATA);
    addr(&mut d, 0x38, 0x41FA, PERT);
    // song: row 0 = records 0,1,0,1 with notes 0,1,2,3; row 1 the same; end
    for r in 0..2 {
        let o = SONG + 8 * r;
        d[o..o + 8].copy_from_slice(&[0, 0, 1, 1, 0, 2, 1, 3]);
    }
    d[SONG + 16] = 0xFF;
    // records: record 0 has 3 steps and an end; record 1 step 1 is the note
    d[REC + 12] = 0xFF;
    let r1 = REC + 0x62;
    d[r1 + 4..r1 + 8].copy_from_slice(&[1, 2, 0, 0]);
    // sample table (8-byte entries): sample 1 at offset 0x20, 16 words
    d[SMP + 8..SMP + 12].copy_from_slice(&0x20u32.to_be_bytes());
    put16(&mut d, SMP + 8 + 6, 16);
    for i in 0..42 {
        put16(&mut d, PERT + 2 * i, 1440 - 10 * i as u16);
    }
    d
}

#[test]
fn detects_and_locates() {
    let d = module();
    let l = locate(&d).expect("module");
    assert_eq!((l.var, l.rs, l.esz), (Variant::B, 0x62, 8));
    assert_eq!((l.samples, l.data, l.periods, l.records, l.song, l.steps), (SMP, DATA, PERT, REC, SONG, 3));
    assert!(detect(&d));
    assert_eq!(l.scan, 0x18);
}

#[test]
fn slide_marker_makes_variant_c() {
    let mut d = module();
    d[0x3C..0x40].copy_from_slice(&0x0883_0007u32.to_be_bytes());
    assert_eq!(locate(&d).unwrap().var, Variant::C);
}

#[test]
fn rejects_garbage_and_truncation() {
    assert!(!detect(&[]));
    assert!(!detect(&[0u8; 0x100]));
    let d = module();
    for n in (0..d.len()).step_by(7) {
        let _ = detect(&d[..n]);
    }
    let mut e = d.clone();
    e[0] = 0x12;
    assert!(!detect(&e));
}

fn run(d: &[u8], ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let l = locate(d).unwrap();
    let mut r = DariusZendeh { d: d.to_vec(), base: 0x10000, l, row: 0, step: 0, prev: [0; 4], mask: 0, gate: true };
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    m.bus.write_log.take().unwrap()
}

#[test]
fn gate_then_row_writes() {
    let d = module();
    let w = run(&d, 2);
    // gate tick: one DMACON write of the (empty) mask at +253
    assert_eq!(w[0], (0xDFF096, 0, 1_000_000 + 253));
    // row tick: step 1 of row 0; channel 0 uses record 0 (empty entry: sample 0, length 0), channel 1 record 1
    let t = 1_000_000 + TICK;
    assert_eq!(w[1].0, 0xDFF0A4);
    assert_eq!(w[1].2, t + 366);
    assert_eq!(w.len(), 1 + 4 * 5 + 1);
}

#[test]
fn row_values_of_record_one() {
    let d = module();
    let w = run(&d, 2);
    // channel 1: LEN 16 words, pointer base + data + 0x20, period table[2 + note 1], volume 1
    let i = w.iter().position(|e| e.0 == 0xDFF0B4).unwrap();
    assert_eq!(w[i].1, 16);
    assert_eq!(w[i + 1], (0xDFF0B0, 1, w[i].2 + 23));
    assert_eq!(w[i + 2].1, (0x10000 + DATA as u32 + 0x20) as u16);
    assert_eq!(w[i + 3].1, 1440 - 30);
    assert_eq!(w[i + 4].1, 1);
    // the last write is DMACON 0x820F
    assert_eq!(w.last().unwrap().0, 0xDFF096);
    assert_eq!(w.last().unwrap().1, 0x820F);
}

#[test]
fn row_advance_loops_without_hanging() {
    let d = module();
    // 3 steps per row, 2 rows: 7 row ticks wrap the song twice; no panic, deterministic
    let a = run(&d, 40);
    let b = run(&d, 40);
    assert_eq!(a, b);
    assert!(a.len() > 100);
}

#[test]
fn start_up_runs_in_new() {
    let d = module();
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    assert!(new(&d, 0x10000, &mut m).is_some());
    let log = m.bus.write_log.take().unwrap();
    // starts with DMACON 0x000F and ends with it after the four volume clears
    assert_eq!((log[0].0, log[0].1), (0xDFF096, 0x000F));
    let n = log.len();
    assert_eq!((log[n - 1].0, log[n - 1].1), (0xDFF096, 0x000F));
    assert_eq!(log[n - 5].1, 0);
}
