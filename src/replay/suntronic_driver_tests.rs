use super::*;
use m68k::CpuType;

const LINK: u32 = 0x5_0000;
const DD: usize = 0x200;
const LIST: usize = DD + 0x40;

fn put16(d: &mut [u8], o: usize, w: u16) {
    d[o..o + 2].copy_from_slice(&w.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, w: u32) {
    d[o..o + 4].copy_from_slice(&w.to_be_bytes());
}

/// A small family-A driver with the shapes the parse looks for: the sequence advance (list at
/// dd+0x40, stride 0x14, position at dd+0x1E), the voice load (voice pointer at dd+0x20, record
/// size 0x130), and a store of the position word to AUD0VOL so that a run shows; then the return.
/// Two list entries (eight stream pointers) and four instrument pointers, linked at `LINK`.
fn module() -> Vec<u8> {
    let mut d = vec![0u8; 0x400];
    let code: &[u16] = &[
        0x48E7, 0xFFFE, // movem.l d0-d7/a0-a6,-(a7)
        0x4DFA, (DD - 6) as u16, // lea dd(pc),a6
        0x4A2E, 0x0018, // tst.b $18(a6)
        0x43EE, 0x0040, // lea $40(a6),a1
        0x7000, // moveq #0,d0
        0x302E, 0x001E, // move.w $1e(a6),d0
        0xC0FC, 0x0014, // mulu #$14,d0
        0x4AB1, 0x0000, // tst.l (a1,d0.w)
        0x6604, // bne.b +4
        0x426E, 0x001E, // clr.w $1e(a6)       (the wrap)
        0x206E, 0x0020, // movea.l $20(a6),a0
        0x20B1, 0x0000, // move.l (a1,d0.w),(a0)
        0x2171, 0x0004, 0x0130, // move.l 4(a1,d0.w),$130(a0)
        0x33EE, 0x001E, 0x00DF, 0xF0A8, // move.w $1e(a6),$dff0a8
        0x526E, 0x001E, // addq.w #1,$1e(a6)
        0x4CDF, 0x7FFF, // movem.l (a7)+,d0-d7/a0-a6
        0x4E75, // rts
    ];
    for (i, w) in code.iter().enumerate() {
        put16(&mut d, 2 * i, *w);
    }
    // the end marker after two entries: the first byte after it is at LIST + 0x2C
    let a = (LIST + 0x2C) as u32;
    for i in 0..4 {
        put32(&mut d, DD + 0x24 + 4 * i, LINK + a + 0x10 * i as u32);
    }
    for e in 0..2 {
        for k in 0..4 {
            put32(&mut d, LIST + 0x14 * e + 4 * k, LINK + a + 0x40 + 0x10 * (4 * e + k) as u32);
        }
        put32(&mut d, LIST + 0x14 * e + 0x10, 0x0102_0304);
    }
    d
}

fn start(d: &[u8]) -> (Machine, Box<dyn Replayer>, u32) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let base = m.alloc(d.len() as u32);
    m.bus.write_bytes(base, d);
    let r = new(d, base, &mut m).expect("recognised");
    (m, r, base)
}

fn tick(m: &mut Machine, r: &mut Box<dyn Replayer>) {
    m.bus.clock += TICK;
    r.tick(&mut Hw { bus: &mut m.bus });
}

#[test]
fn the_parse_reads_the_driver_shapes() {
    let d = module();
    let drv = parse(&d).expect("parsed");
    assert_eq!((drv.fam, drv.dd, drv.list, drv.stride, drv.vvar, drv.rec), (Family::A, DD, LIST, 0x14, 0x20, 0x130));
    assert_eq!(drv.wrap, 0x20);
    let rl = reloc(&d, &drv).expect("relocated");
    assert_eq!(rl.origin, LINK);
    assert_eq!(rl.ents, 2);
    assert_eq!(rl.ptrs.len(), 12);
    assert!(detect(&d));
}

#[test]
fn the_start_relocates_and_fills_the_voice_block() {
    let d = module();
    let (m, _r, base) = start(&d);
    let a = (LIST + 0x2C) as u32;
    assert_eq!(m.bus.r32(base + (DD + 0x24) as u32), base + a);
    assert_eq!(m.bus.r32(base + LIST as u32), base + a + 0x40);
    let block = m.bus.r32(base + DD as u32 + 0x20);
    assert_ne!(block, 0);
    for v in 0..4u32 {
        assert_eq!(m.bus.r32(block + v * 0x130), base + a + 0x40 + 0x10 * v);
        assert_eq!(m.bus.r8(block + v * 0x130 + 0x14), 0xFF);
        assert_eq!(m.bus.r8(block + 0x12E + v * 0x130), v as u8 + 1);
    }
    // the start values of family A
    assert_eq!(m.bus.r8(base + DD as u32 + 0x14), 5);
    assert_eq!(m.bus.r8(base + DD as u32 + 0x15), 0xFF);
    assert_eq!(m.bus.r8(base + DD as u32 + 0x16), 6);
}

#[test]
fn the_driver_runs_each_tick_and_its_wrap_is_the_song_end() {
    let d = module();
    let (mut m, mut r, _) = start(&d);
    for _ in 0..3 {
        tick(&mut m, &mut r);
    }
    let vols: Vec<u16> = m.bus.write_log.as_ref().unwrap().iter().filter(|e| e.0 == 0xDFF0A8).map(|e| e.1).collect();
    // positions 0, 1, then entry 2 is the end marker: the wrap clears it to 0
    assert_eq!(vols, vec![0, 1, 0]);
    assert!(r.songend());
}

#[test]
fn malformed_modules_are_refused_or_stop_safely() {
    let d = module();
    assert!(!detect(&[]));
    assert!(!detect(&d[..0x3FF]));
    assert!(!detect(&[0xFF; 0x800]));
    assert!(!detect(&[0; 0x800]));
    // the two builds `suntronic.rs` models are left to it
    let mut a = d.clone();
    put16(&mut a, 6, 0x6A4 - 6);
    assert!(parse(&a).is_none());
    // another family word, a data block outside the file, no sequence advance, no voice load
    for (o, w) in [(8usize, 0x4A2E_0014u32), (4, 0x4DFA_7FF0), (0x1A, 0x4E71_4E71), (0x28, 0x4E71_4E71)] {
        let mut bad = d.clone();
        put32(&mut bad, o, w);
        assert!(!detect(&bad), "offset {o:#x}");
    }
    // no end marker before the file ends
    let mut bad = d.clone();
    for o in (LIST..0x400).step_by(4) {
        put32(&mut bad, o, LINK + 0x300);
    }
    assert!(!detect(&bad));
    // a driver that never returns: the call is cut at its cycle budget and the song ends
    let mut hang = d.clone();
    put16(&mut hang, 0x3A, 0x60FE); // bra.b * in place of the movem restore
    put16(&mut hang, 0x3C, 0x4E75); // keep a return shape for the parse
    put32(&mut hang, 0x3E, 0x4CDF_7FFF);
    put16(&mut hang, 0x42, 0x4E75);
    let (mut m, mut r, _) = start(&hang);
    tick(&mut m, &mut r);
    assert!(r.songend());
    tick(&mut m, &mut r);
    // an illegal instruction ends the song too
    let mut ill = d.clone();
    put16(&mut ill, 0x32, 0x4AFC);
    let (mut m, mut r, _) = start(&ill);
    tick(&mut m, &mut r);
    assert!(r.songend());
    // garbage after the header shapes: never a panic
    let mut x = 0x1234_5678u32;
    for _ in 0..200 {
        let mut g = d.clone();
        for b in g.iter_mut().skip(0x44) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *b = x as u8;
        }
        if detect(&g) {
            let mut m = Machine::new(CpuType::M68000);
            let base = m.alloc(g.len() as u32);
            m.bus.write_bytes(base, &g);
            if let Some(mut r) = new(&g, base, &mut m) {
                tick(&mut m, &mut r);
            }
        }
    }
}
