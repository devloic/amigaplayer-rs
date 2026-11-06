use super::*;
use m68k::CpuType;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// Plain module: rows at 0x18, order at 0x40, instruments at 0x60, sample table at 0xA0, volume
/// tables at 0xC0, pitch tables at 0x140, sample at 0x1C0. Row 1 starts instrument 0 on voice 0
/// at note `note`; the order plays rows 1..1 at speed 2, then ends with a jump to entry 0.
fn module(note: u8) -> Vec<u8> {
    let mut d = vec![0u8; 0x200];
    for (i, v) in [0x18u32, 0x40, 0x60, 0xA0, 0xC0, 0x140].iter().enumerate() {
        put32(&mut d, 4 * i, *v);
    }
    put16(&mut d, 0x18 + 8, (note as u16) << 8);
    put16(&mut d, 0x40, 1);
    put16(&mut d, 0x42, 0x2001);
    put16(&mut d, 0x44, 0xFFFF);
    put16(&mut d, 0x46, 0);
    d[0x60 + 9] = 0x40;
    d[0x60 + 1] = 0;
    put16(&mut d, 0xA0, 0x1C0 / 4);
    put16(&mut d, 0xA2, 8);
    d
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    m.bus.clock = 1_000_000;
    let mut r = new(d, 0x10000, &mut m).unwrap();
    for _ in 0..ticks {
        m.bus.clock += TICK;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    (m.bus.write_log.take().unwrap_or_default(), r.songend())
}

#[test]
fn detects_plain_and_rejects_garbage() {
    let d = module(13);
    assert!(detect(&d));
    assert_eq!(locate(&d).map(|x| x.0), Some(0));
    assert!(!detect(&d[..10]));
    assert!(!detect(&[0xFF; 64]));
    let mut bad = d.clone();
    put32(&mut bad, 0, 0x20);
    assert!(!detect(&bad));
}

#[test]
fn detects_easo_and_branch() {
    let d = module(13);
    let mut e = b"EASO".to_vec();
    e.extend_from_slice(&d);
    assert_eq!(locate(&e).map(|x| x.0), Some(4));
    // branch header: opcode, offset, opcode, offset, key words at the target, then d3
    let mut b = vec![0u8; 2 + 2 + 2 + 2 + 8];
    b[..2].copy_from_slice(&KEY[..2]);
    put16(&mut b, 2, 6);
    b[4..6].copy_from_slice(&KEY[..2]);
    put16(&mut b, 6, 2);
    b.extend_from_slice(&[0, 0]);
    b.truncate(8);
    b.extend_from_slice(&KEY[2..12]);
    b.extend_from_slice(&[0, 4, 0, 0]);
    assert_eq!(b.len(), 22);
    // target = 2 + 6 = 8; d3 at 18; data at 18 + 4 -> pad so the data starts at 22
    put16(&mut b, 18, 4);
    b.extend_from_slice(&d);
    assert_eq!(locate(&b).map(|x| x.0), Some(22));
}

#[test]
fn first_note_writes_in_order() {
    let (w, _) = run(&module(13), 1);
    let regs: Vec<u32> = w.iter().map(|x| x.0).collect();
    // the first tick reads the order entry; row 1 has the note: PER, DMA off, LEN, LC (two words),
    // VOL, PER, DMA on, LEN(1)
    assert_eq!(&regs[..9], &[0xDFF0A6, 0xDFF096, 0xDFF0A4, 0xDFF0A0, 0xDFF0A2, 0xDFF0A8, 0xDFF0A6, 0xDFF096, 0xDFF0A4]);
    assert_eq!(w[0].1, 0x78);
    assert_eq!(w[6].1, 0x06B0);
}

#[test]
fn deterministic_and_bounded() {
    let d = module(0);
    assert_eq!(run(&d, 30), run(&d, 30));
    // garbage in the tables: no panic
    let mut g = module(200);
    for i in 0x60..0x1C0 {
        g[i] = (i * 37) as u8;
    }
    run(&g, 50);
}

#[test]
fn song_end_reported() {
    let (_, end) = run(&module(13), 8);
    assert!(end);
}
