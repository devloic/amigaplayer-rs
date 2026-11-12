use super::chip::Ay;
use super::z80::*;
use super::*;
use crate::aiff::Stream;

// ------------------------------------------------------------------ Z80

struct Ram {
    mem: Vec<u8>,
    outs: Vec<(u16, u8)>,
    inval: u8,
}

impl Bus for Ram {
    fn read(&mut self, a: u16) -> u8 {
        self.mem[a as usize]
    }
    fn write(&mut self, a: u16, v: u8) {
        self.mem[a as usize] = v;
    }
    fn input(&mut self, _p: u16, _t: u64) -> u8 {
        self.inval
    }
    fn output(&mut self, p: u16, v: u8, _t: u64) {
        self.outs.push((p, v));
    }
}

/// Runs `code` at $8000 until PC reaches its end; returns the processor and the memory.
fn run(code: &[u8], setup: impl FnOnce(&mut Z80, &mut Ram)) -> (Z80, Ram) {
    let mut m = Ram { mem: vec![0; 65536], outs: Vec::new(), inval: 0 };
    m.mem[0x8000..0x8000 + code.len()].copy_from_slice(code);
    let mut z = Z80::new();
    z.pc = 0x8000;
    z.sp = 0xF000;
    z.f = 0;
    setup(&mut z, &mut m);
    let end = 0x8000 + code.len() as u16;
    for _ in 0..10_000 {
        if z.pc == end {
            break;
        }
        z.step(&mut m);
    }
    assert_eq!(z.pc, end, "did not reach the end");
    (z, m)
}

#[test]
fn z80_add_sub_flags() {
    // LD A,$7F; ADD A,1: overflow into the sign, half carry
    let (z, _) = run(&[0x3E, 0x7F, 0xC6, 0x01], |_, _| {});
    assert_eq!((z.a, z.f), (0x80, SF | HF | PF));
    // XOR A; SUB 1: $FF, borrow, half borrow, flags 3 and 5 from the result
    let (z, _) = run(&[0xAF, 0xD6, 0x01], |_, _| {});
    assert_eq!((z.a, z.f), (0xFF, SF | YF | HF | XF | NF | CF));
    // XOR A; CP $28: flags 3 and 5 come from the operand
    let (z, _) = run(&[0xAF, 0xFE, 0x28], |_, _| {});
    assert_eq!(z.a, 0);
    assert_eq!(z.f & (YF | XF), YF | XF);
    assert_eq!(z.f & (CF | NF | SF), CF | NF | SF);
    // LD A,$80; ADD A,$80: zero, carry, overflow
    let (z, _) = run(&[0x3E, 0x80, 0xC6, 0x80], |_, _| {});
    assert_eq!((z.a, z.f), (0, ZF | PF | CF));
    // SCF; LD A,$10; ADC A,$0F: 1 + $10 + $0F = $20, half carry
    let (z, _) = run(&[0x37, 0x3E, 0x10, 0xCE, 0x0F], |_, _| {});
    assert_eq!((z.a, z.f), (0x20, YF | HF));
    // INC and DEC at the edges keep the carry
    let (z, _) = run(&[0x37, 0x3E, 0x7F, 0x3C], |_, _| {});
    assert_eq!((z.a, z.f), (0x80, SF | HF | PF | CF));
    let (z, _) = run(&[0xAF, 0x3D], |_, _| {});
    assert_eq!((z.a, z.f), (0xFF, SF | YF | HF | XF | NF));
}

#[test]
fn z80_daa() {
    // $15 + $27 = $3C, DAA: $42
    let (z, _) = run(&[0x3E, 0x15, 0xC6, 0x27, 0x27], |_, _| {});
    assert_eq!((z.a, z.f), (0x42, HF | PF));
    // $42 - $15 = $2D, DAA: $27
    let (z, _) = run(&[0x3E, 0x42, 0xD6, 0x15, 0x27], |_, _| {});
    assert_eq!((z.a, z.f), (0x27, NF | PF | YF));
    // $99 + 1 = $9A, DAA: $00 with carry
    let (z, _) = run(&[0x3E, 0x99, 0xC6, 0x01, 0x27], |_, _| {});
    assert_eq!(z.a, 0);
    assert_eq!(z.f & (CF | ZF), CF | ZF);
}

#[test]
fn z80_16bit_arithmetic() {
    // LD HL,$7FFF; LD BC,1; SCF; CCF; ADC HL,BC: overflow, sign, half carry
    let (z, _) = run(&[0x21, 0xFF, 0x7F, 0x01, 0x01, 0x00, 0x37, 0x3F, 0xED, 0x4A], |_, _| {});
    assert_eq!(z.hl(), 0x8000);
    assert_eq!(z.f, SF | HF | PF);
    // LD HL,0; LD DE,1; OR A; SBC HL,DE: $FFFF with borrow
    let (z, _) = run(&[0x21, 0, 0, 0x11, 1, 0, 0xB7, 0xED, 0x52], |_, _| {});
    assert_eq!(z.hl(), 0xFFFF);
    assert_eq!(z.f, SF | YF | HF | XF | NF | CF);
    // ADD HL,HL keeps S, Z and P/V, takes 3/5 from the high byte
    let (z, _) = run(&[0x21, 0x00, 0x88, 0x29], |z, _| z.f = 0);
    assert_eq!(z.hl(), 0x1000);
    assert_eq!(z.f, CF | HF);
}

#[test]
fn z80_undocumented() {
    // SLL B (CB 30): shifts in a 1
    let (z, _) = run(&[0x06, 0x80, 0xCB, 0x30], |_, _| {});
    assert_eq!(z.b, 0x01);
    assert_eq!(z.f & CF, CF);
    // LD IXH,$12; LD IXL,$34; LD A,IXH; LD B,IXL
    let (z, _) = run(&[0xDD, 0x26, 0x12, 0xDD, 0x2E, 0x34, 0xDD, 0x7C, 0xDD, 0x45], |_, _| {});
    assert_eq!((z.ix, z.a, z.b), (0x1234, 0x12, 0x34));
    // LD H,(IX+1) loads the real H, not IXH
    let (z, _) = run(&[0xDD, 0x21, 0x00, 0x90, 0xDD, 0x66, 0x01], |_, m| m.mem[0x9001] = 0x5A);
    assert_eq!((z.h, z.ix), (0x5A, 0x9000));
    // RLC (IX+2) with the result copied into B (DD CB 02 00)
    let (z, m) = run(&[0xDD, 0x21, 0x00, 0x90, 0xDD, 0xCB, 0x02, 0x00], |_, m| m.mem[0x9002] = 0x81);
    assert_eq!((m.mem[0x9002], z.b), (0x03, 0x03));
    // SET 0,(IY-1) into C (FD CB FF C1)
    let (z, m) = run(&[0xFD, 0x21, 0x00, 0x90, 0xFD, 0xCB, 0xFF, 0xC1], |_, _| {});
    assert_eq!((m.mem[0x8FFF], z.c), (0x01, 0x01));
    // ED 4C is NEG too
    let (z, _) = run(&[0x3E, 0x01, 0xED, 0x4C], |_, _| {});
    assert_eq!(z.a, 0xFF);
    // IN F,(C) (ED 70) sets the flags only; OUT (C),0 (ED 71)
    let (z, m) = run(&[0x01, 0xFE, 0x00, 0xED, 0x70, 0xED, 0x71], |_, m| m.inval = 0x00);
    assert_eq!(z.f & (ZF | PF), ZF | PF);
    assert_eq!(m.outs, vec![(0x00FE, 0)]);
    // BIT 0,(HL): flags 3 and 5 from MEMPTR, here $2801 after LD A,($2800)
    let (z, _) = run(&[0x3A, 0x00, 0x28, 0x21, 0x00, 0x90, 0xCB, 0x46], |_, _| {});
    assert_eq!(z.f & (YF | XF), YF | XF);
    // BIT 7,A with bit 7 set: S; Z and P/V clear
    let (z, _) = run(&[0x3E, 0x80, 0xCB, 0x7F], |_, _| {});
    assert_eq!(z.f & (SF | ZF | PF | HF), SF | HF);
}

#[test]
fn z80_block_instructions() {
    // LDI with A=0 and the byte $0A: flag 3 from bit 3, flag 5 from bit 1; BC 2 -> 1: P/V
    let (z, m) = run(&[0xAF, 0x21, 0x00, 0x90, 0x11, 0x00, 0xA0, 0x01, 0x02, 0x00, 0xED, 0xA0], |_, m| {
        m.mem[0x9000] = 0x0A
    });
    assert_eq!(m.mem[0xA000], 0x0A);
    assert_eq!((z.hl(), z.de(), z.bc()), (0x9001, 0xA001, 1));
    assert_eq!(z.f & (YF | XF | PF | HF | NF), YF | XF | PF);
    // LDIR copies 4 bytes
    let (z, m) = run(&[0x21, 0x00, 0x90, 0x11, 0x00, 0xA0, 0x01, 0x04, 0x00, 0xED, 0xB0], |_, m| {
        m.mem[0x9000..0x9004].copy_from_slice(&[1, 2, 3, 4])
    });
    assert_eq!(&m.mem[0xA000..0xA004], &[1, 2, 3, 4]);
    assert_eq!(z.bc(), 0);
    assert_eq!(z.f & PF, 0);
    // CPIR stops at the match: A=3 found at $9002
    let (z, _) = run(&[0x3E, 0x03, 0x21, 0x00, 0x90, 0x01, 0x10, 0x00, 0xED, 0xB1], |_, m| {
        m.mem[0x9000..0x9004].copy_from_slice(&[1, 2, 3, 4])
    });
    assert_eq!((z.hl(), z.bc()), (0x9003, 13));
    assert_eq!(z.f & (ZF | PF | NF), ZF | PF | NF);
    // OUTI puts B on the bus after its decrement
    let (z, m) = run(&[0x01, 0xFD, 0x10, 0x21, 0x00, 0x90, 0xED, 0xA3], |_, m| m.mem[0x9000] = 0x77);
    assert_eq!(m.outs, vec![(0x0FFD, 0x77)]);
    assert_eq!(z.b, 0x0F);
    // RLD and RRD
    let (z, m) = run(&[0x3E, 0x12, 0x21, 0x00, 0x90, 0xED, 0x6F], |_, m| m.mem[0x9000] = 0x34);
    assert_eq!((z.a, m.mem[0x9000]), (0x13, 0x42));
    let (z, m) = run(&[0x3E, 0x12, 0x21, 0x00, 0x90, 0xED, 0x67], |_, m| m.mem[0x9000] = 0x34);
    assert_eq!((z.a, m.mem[0x9000]), (0x14, 0x23));
}

#[test]
fn z80_exchange_stack_and_jumps() {
    // EX AF,AF'; EXX; PUSH/POP; CALL/RET; DJNZ
    let code = [
        0x3E, 0x11, 0x08, 0x3E, 0x22, // A=$11, EX AF, A=$22
        0x01, 0x34, 0x12, 0xD9, 0x01, 0x78, 0x56, // BC=$1234, EXX, BC=$5678
        0xC5, 0xD1, // PUSH BC; POP DE
        0xCD, 0x14, 0x80, // CALL $8014
        0x18, 0x05, // JR +5 (to the end)
        0x06, 0x03, 0x10, 0xFE, 0xC9, // $8014: LD B,3; DJNZ $; RET
    ];
    let (z, _) = run(&code, |_, _| {});
    assert_eq!(z.a, 0x22);
    assert_eq!(z.af2 >> 8, 0x11);
    assert_eq!(z.bc2, 0x1234);
    assert_eq!(z.de(), 0x5678);
    assert_eq!(z.b, 0);
    assert_eq!(z.sp, 0xF000);
}

#[test]
fn z80_tstates_and_refresh() {
    // LD B,3 (7); DJNZ $ x3 (13+13+8); NOP (4)
    let (z, _) = run(&[0x06, 0x03, 0x10, 0xFE, 0x00], |_, _| {});
    assert_eq!(z.t, 7 + 13 + 13 + 8 + 4);
    assert_eq!(z.r & 0x7F, 5);
    // LD A,(IX+1) is 19 T-states and two refresh steps; LD (IX+1),5 is 19
    let (z, _) = run(&[0xDD, 0x7E, 0x01, 0xDD, 0x36, 0x01, 0x05], |_, _| {});
    assert_eq!(z.t, 38);
    assert_eq!(z.r & 0x7F, 4);
    // INC (IX+0) 23, RLC (IX+0) 23, BIT 0,(IX+0) 20
    let (z, _) = run(&[0xDD, 0x34, 0x00, 0xDD, 0xCB, 0x00, 0x06, 0xDD, 0xCB, 0x00, 0x46], |_, _| {});
    assert_eq!(z.t, 23 + 23 + 20);
    // LD A,R sees the refresh counter, P/V = IFF2
    let (z, _) = run(&[0x00, 0x00, 0xED, 0x5F], |z, _| z.iff2 = true);
    assert_eq!(z.a, 4);
    assert_eq!(z.f & PF, PF);
}

#[test]
fn z80_interrupts() {
    let mut m = Ram { mem: vec![0; 65536], outs: Vec::new(), inval: 0 };
    // IM 1: EI; HALT at $8000, the interrupt goes to $0038
    m.mem[0x8000..0x8004].copy_from_slice(&[0xED, 0x56, 0xFB, 0x76]);
    let mut z = Z80::new();
    z.pc = 0x8000;
    z.sp = 0xF000;
    z.step(&mut m);
    z.step(&mut m);
    // right after EI: refused
    assert_eq!(z.interrupt(&mut m), 0);
    z.step(&mut m);
    assert!(z.halted);
    assert_eq!(z.interrupt(&mut m), 13);
    assert!(!z.halted && !z.iff1);
    assert_eq!(z.pc, 0x0038);
    assert_eq!(m.mem[0xEFFE] as u16 | (m.mem[0xEFFF] as u16) << 8, 0x8004);
    // IM 2: the vector at I*256+$FF
    let mut z = Z80::new();
    m.mem[0x80FF] = 0x34;
    m.mem[0x8100] = 0x12;
    z.i = 0x80;
    z.im = 2;
    z.iff1 = true;
    z.sp = 0xF000;
    assert_eq!(z.interrupt(&mut m), 19);
    assert_eq!(z.pc, 0x1234);
    // DI: refused
    assert_eq!(z.interrupt(&mut m), 0);
}

#[test]
fn z80_prefix_run_is_bounded() {
    let mut m = Ram { mem: vec![0xDD; 65536], outs: Vec::new(), inval: 0 };
    let mut z = Z80::new();
    for _ in 0..1000 {
        assert_eq!(z.step(&mut m), 4);
    }
}

// ------------------------------------------------------------------ AY

#[test]
fn ay_tone_frequency() {
    for tp in [1u16, 100, 252, 4095] {
        let mut a = Ay::new();
        a.write(0, tp as u8);
        a.write(1, (tp >> 8) as u8);
        a.write(7, 0x3E);
        a.write(8, 15);
        let steps = 200_000;
        let mut rises = 0;
        let mut last = 0;
        for _ in 0..steps {
            let o = a.step()[0];
            if o != 0 && last == 0 {
                rises += 1;
            }
            last = o;
        }
        // f = clock/(16 TP): one cycle every 2 TP steps of 8 clocks
        let expect = steps as f64 / (2.0 * tp as f64);
        assert!((rises as f64 - expect).abs() <= 1.0, "TP {tp}: {rises} cycles, {expect} expected");
    }
}

#[test]
fn ay_volume_table() {
    let a = Ay::new();
    assert_eq!(a.dac[0], 0.0);
    assert_eq!(a.dac[15], 1.0);
    assert!((a.dac[13] - 0.5).abs() < 1e-6);
    for n in 1..16 {
        assert!(a.dac[n] > a.dac[n - 1]);
    }
}

/// The envelope level after each of `n` steps (envelope period 1: a step every 2 chip steps).
fn env_seq(shape: u8, n: usize) -> Vec<u8> {
    let mut a = Ay::new();
    a.write(11, 1);
    a.write(12, 0);
    a.write(13, shape);
    let mut v = vec![a.env_level()];
    for _ in 1..n {
        a.step();
        a.step();
        v.push(a.env_level());
    }
    v
}

#[test]
fn ay_envelope_shapes() {
    let up: Vec<u8> = (0..16).collect();
    let down: Vec<u8> = (0..16).rev().collect();
    let cat = |parts: &[&[u8]]| parts.concat();
    let z16 = [0u8; 16];
    let f16 = [15u8; 16];
    for s in 0..16u8 {
        let want = match s {
            0..=3 | 9 => cat(&[&down, &z16, &z16]),
            4..=7 | 15 => cat(&[&up, &z16, &z16]),
            8 => cat(&[&down, &down, &down]),
            10 => cat(&[&down, &up, &down]),
            11 => cat(&[&down, &f16, &f16]),
            12 => cat(&[&up, &up, &up]),
            13 => cat(&[&up, &f16, &f16]),
            _ => cat(&[&up, &down, &up]),
        };
        assert_eq!(env_seq(s, 48), want, "shape {s}");
    }
}

#[test]
fn ay_noise_lfsr_period() {
    let mut a = Ay::new();
    a.write(6, 1);
    // one shift every 2 chip steps; the 17-bit register returns after 2^17 - 1 shifts
    let mut n = 0u32;
    loop {
        a.step();
        a.step();
        n += 1;
        if a_lfsr(&a) == 1 || n > 200_000 {
            break;
        }
    }
    assert_eq!(n, 131_071);
}

fn a_lfsr(a: &Ay) -> u32 {
    a.lfsr_state()
}

// ------------------------------------------------------------------ files

/// A ZXAYEMUL file: one block of `code` at `addr`, init and interrupt addresses, per song
/// (length, fade, hi register).
fn ay_file(addr: u16, code: &[u8], init: u16, irq: u16, songs: &[(u16, u16, u8)]) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"ZXAYEMUL");
    d.extend_from_slice(&[0, 0, 0, 0]); // versions, special player
    let author_at = d.len();
    d.extend_from_slice(&[0, 0, 0, 0]); // author, misc (filled below)
    d.push(songs.len() as u8 - 1);
    d.push(0);
    d.extend_from_slice(&2u16.to_be_bytes()); // songs structure right after
    let ss = d.len();
    d.resize(ss + 4 * songs.len(), 0);
    let fix = |d: &mut Vec<u8>, at: usize, to: usize| {
        let v = (to as i64 - at as i64) as i16;
        d[at..at + 2].copy_from_slice(&v.to_be_bytes());
    };
    let name = d.len();
    d.extend_from_slice(b"test\0");
    fix(&mut d, author_at, name);
    fix(&mut d, author_at + 2, name);
    for (i, &(len, fade, hi)) in songs.iter().enumerate() {
        let sd = d.len();
        fix(&mut d, ss + 4 * i, name);
        fix(&mut d, ss + 4 * i + 2, sd);
        d.extend_from_slice(&[0, 1, 2, 3]);
        d.extend_from_slice(&len.to_be_bytes());
        d.extend_from_slice(&fade.to_be_bytes());
        d.extend_from_slice(&[hi, 0]);
        d.extend_from_slice(&[0, 4, 0, 10]); // points at +4 (sd+14), addresses at +10 (sd+22)
        d.extend_from_slice(&0xF000u16.to_be_bytes());
        d.extend_from_slice(&init.to_be_bytes());
        d.extend_from_slice(&irq.to_be_bytes());
        // sd+20: padding to sd+22
        d.extend_from_slice(&[0, 0]);
        let blk = d.len();
        d.extend_from_slice(&addr.to_be_bytes());
        d.extend_from_slice(&(code.len() as u16).to_be_bytes());
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&[0, 0]); // end of the block list
        let data = d.len();
        d.extend_from_slice(code);
        fix(&mut d, blk + 4, data);
    }
    d
}

/// Code that sets AY register `r` to `v` (Spectrum ports).
fn spec_set(r: u8, v: u8) -> Vec<u8> {
    vec![0x01, 0xFD, 0xFF, 0x3E, r, 0xED, 0x79, 0x06, 0xBF, 0x3E, v, 0xED, 0x79]
}

/// A tune whose init routine plays a steady tone on channel A with period `tp`, by
/// Spectrum ports or CPC ports.
fn tone_tune(tp: u16, cpc: bool, songs: &[(u16, u16, u8)]) -> Vec<u8> {
    let regs = [(0u8, tp as u8), (1, (tp >> 8) as u8), (7, 0x3E), (8, 15)];
    let mut code = Vec::new();
    for (r, v) in regs {
        if cpc {
            // LD BC,$F400+r; OUT (C),C; LD BC,$F6C0; OUT (C),C; LD BC,$F600; OUT (C),C;
            // LD BC,$F400+v; OUT (C),C; LD BC,$F680; OUT (C),C; LD BC,$F600; OUT (C),C
            for (b, c) in [(0xF4, r), (0xF6, 0xC0), (0xF6, 0x00), (0xF4, v), (0xF6, 0x80), (0xF6, 0x00)] {
                code.extend_from_slice(&[0x01, c, b, 0xED, 0x49]);
            }
        } else {
            code.extend(spec_set(r, v));
        }
    }
    code.push(0xC9); // RET (init)
    code.push(0xC9); // the interrupt routine
    let irq = 0x8000 + code.len() as u16 - 1;
    ay_file(0x8000, &code, 0x8000, irq, songs)
}

fn render(p: &mut AyPlayer, secs: f64) -> Vec<f32> {
    let mut v = vec![0f32; (secs * 48000.0) as usize * 2];
    p.render(&mut v);
    v
}

/// The fundamental of the left channel by counting rising zero crossings.
fn freq(v: &[f32]) -> f64 {
    let l: Vec<f32> = v.iter().step_by(2).copied().collect();
    let skip = 4800;
    let mut n = 0;
    let (mut first, mut last) = (0, 0);
    for i in skip + 1..l.len() {
        if l[i - 1] < 0.0 && l[i] >= 0.0 {
            if n == 0 {
                first = i;
            }
            last = i;
            n += 1;
        }
    }
    (n - 1) as f64 * 48000.0 / (last - first) as f64
}

#[test]
fn file_detection_and_parse() {
    let d = tone_tune(252, false, &[(100, 10, 0), (0, 0, 1)]);
    assert!(detect(&d));
    let f = parse(&d).unwrap();
    assert_eq!(f.songs.len(), 2);
    assert_eq!(f.author, "test");
    assert_eq!((f.songs[0].length, f.songs[0].fade, f.songs[1].hi), (100, 10, 1));
    assert_eq!(f.songs[0].blocks[0].0, 0x8000);
    let mut bad = d.clone();
    bad[4] = b'X';
    assert!(!detect(&bad));
}

#[test]
fn malformed_files() {
    let d = tone_tune(252, false, &[(100, 10, 0)]);
    // every truncation either is refused or plays without panicking
    for n in 0..d.len() {
        let t = &d[..n];
        if detect(t) {
            let mut p = AyPlayer::new(t, 48000).unwrap();
            render(&mut p, 0.1);
        }
    }
    // pointers out of the file, a block longer than memory, huge song count
    let mut x = d.clone();
    x[18] = 0x7F;
    assert!(!detect(&x));
    let mut x = d.clone();
    x[16] = 0xFF;
    if let Some(f) = parse(&x) {
        assert!(f.songs.len() <= 256);
    }
    let code = vec![0x18, 0xFE]; // JR $: never halts, interrupts off
    let mut y = ay_file(0xFFFF, &code, 0xFFFF, 0, &[(0, 0, 0)]);
    let at = y.len() - 2 - 4 - 2;
    y[at..at + 2].copy_from_slice(&0xFFFFu16.to_be_bytes()); // a block length past the file
    let mut p = AyPlayer::new(&y, 48000).unwrap();
    let t = std::time::Instant::now();
    render(&mut p, 2.0);
    assert!(t.elapsed().as_secs_f64() < 2.0);
    // random bytes after the magic
    let mut s = 0x1234_5678u32;
    for _ in 0..200 {
        let mut r = b"ZXAYEMUL".to_vec();
        for _ in 0..300 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            r.push(s as u8);
        }
        if detect(&r) {
            let mut p = AyPlayer::new(&r, 48000).unwrap();
            render(&mut p, 0.05);
        }
    }
}

#[test]
fn tone_pitch_spectrum_and_cpc() {
    let mut p = AyPlayer::new(&tone_tune(252, false, &[(0, 0, 0)]), 48000).unwrap();
    assert!(!p.is_cpc());
    let v = render(&mut p, 1.0);
    let f = freq(&v);
    let want = 1_773_450.0 / (16.0 * 252.0);
    assert!((f - want).abs() < 0.5, "{f} Hz, {want} expected");
    // the same tune by the CPC's ports: the AY at 1 MHz
    let mut p = AyPlayer::new(&tone_tune(142, true, &[(0, 0, 0)]), 48000).unwrap();
    assert!(p.is_cpc());
    let v = render(&mut p, 1.0);
    let f = freq(&v);
    let want = 1_000_000.0 / (16.0 * 142.0);
    assert!((f - want).abs() < 0.5, "CPC {f} Hz, {want} expected");
    // channel A is on the left only
    let r: f32 = v.iter().skip(1).step_by(2).map(|x| x.abs()).sum();
    assert!(r < 1.0, "right {r}");
}

#[test]
fn song_length_fade_and_subsongs() {
    let d = tone_tune(100, false, &[(10, 5, 0), (0, 0, 1)]);
    let mut p = AyPlayer::new(&d, 48000).unwrap();
    assert_eq!(p.subsongs(), (0, 1, 0));
    assert_eq!(p.duration_secs(), Some(0.3));
    let mut n = 0;
    while !p.ended() {
        render(&mut p, 0.02);
        n += 1;
        assert!(n < 100);
    }
    assert!((14..=16).contains(&n), "{n}");
    // after the fade: silence
    let v = render(&mut p, 0.1);
    assert!(v[1920..].iter().all(|&x| x.abs() < 0.01));
    p.set_subsong(1);
    assert_eq!(p.subsongs(), (0, 1, 1));
    assert_eq!(p.duration_secs(), None);
    render(&mut p, 1.0);
    assert!(!p.ended());
}

#[test]
fn beeper() {
    // interrupt routine: toggle bit 4 of port $FE each frame (a 25 Hz square)
    let code = [
        0xC9, // init
        0x3A, 0x00, 0x90, 0xEE, 0x10, 0x32, 0x00, 0x90, 0xD3, 0xFE, 0xC9, // LD A,($9000); XOR $10; LD ($9000),A; OUT ($FE),A
    ];
    let d = ay_file(0x8000, &code, 0x8000, 0x8001, &[(0, 0, 0)]);
    let mut p = AyPlayer::new(&d, 48000).unwrap();
    let v = render(&mut p, 2.0);
    let f = freq(&v);
    assert!((f - 25.0).abs() < 0.2, "{f}");
}

#[test]
fn deterministic() {
    let d = tone_tune(77, false, &[(0, 0, 0)]);
    let a = render(&mut AyPlayer::new(&d, 44100).unwrap(), 0.5);
    let b = render(&mut AyPlayer::new(&d, 44100).unwrap(), 0.5);
    assert_eq!(a, b);
}

#[test]
fn engine_plays_it() {
    let d = tone_tune(252, false, &[(0, 0, 0), (0, 0, 1)]);
    let mut p = player(&d, 48000).unwrap();
    assert_eq!(p.format(), "ZXAYEMUL");
    assert_eq!(p.subsongs(), (0, 1, 0));
    let mut v = vec![0f32; 9600];
    assert_eq!(p.render(&mut v), 4800);
    p.set_subsong(1);
    assert_eq!(p.subsongs(), (0, 1, 1));
}

const DIR: &str = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/dl-modules/PlayAY";

/// The 13 modland test files (when present here): every song plays, not silent.
#[test]
fn test_files_play() {
    let Ok(rd) = std::fs::read_dir(DIR) else { return };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    files.sort();
    for f in files {
        let d = std::fs::read(&f).unwrap();
        assert!(detect(&d), "{f:?}");
        let mut p = AyPlayer::new(&d, 48000).unwrap();
        let (_, last, _) = p.subsongs();
        for s in 0..=last {
            p.set_subsong(s);
            let v = render(&mut p, 5.0);
            let rms = (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
            assert!(rms > 0.01, "{f:?} song {s}: rms {rms}");
        }
    }
}

/// Diagnostics: AY_DIAG=<file> prints, per frame, the register writes.
#[test]
#[ignore]
fn diag() {
    let Ok(f) = std::env::var("AY_DIAG") else { return };
    let d = std::fs::read(f).unwrap();
    let mut p = AyPlayer::new(&d, 48000).unwrap();
    p.start(std::env::var("AY_SONG").ok().and_then(|s| s.parse().ok()).unwrap_or(0));
    let frames: u64 = std::env::var("AY_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    for fr in 0..frames {
        p.frame();
        let w: Vec<String> = p.m.events.iter().take(40).map(|(t, e)| format!("{t}:{e:?}")).collect();
        println!("frame {fr} pc={:04x} halted={} n={} {}", p.z.pc, p.z.halted, p.m.events.len(), w.join(" "));
    }
}
