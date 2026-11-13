//! The NES CPU (Ricoh 2A03): a 6502 without decimal mode, instruction by instruction with
//! each instruction's cycle count.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from public
//! documentation only: the NESdev wiki's "Instruction reference" (operations, flags, the V
//! formulas of ADC / SBC, B and bit 5 pushed as 1 by PHP / BRK and ignored by PLP / RTI, the
//! JMP ($xxFF) page wrap, cycle counts and the page-crossing and branch penalties, decimal mode
//! disabled on the 2A03), "CPU unofficial opcodes" (the opcode matrix with every unofficial
//! opcode's mnemonic and addressing mode) and "Programming with unofficial opcodes" (what the
//! combined operations do and their cycle counts). No emulator's code was consulted.
//!
//! Not modelled: the dummy reads and writes inside an instruction (only the reads and writes
//! an instruction means are made), interrupts other than BRK (an NSF has none to take), the
//! exact results of the unstable opcodes ($8B XAA, $93/$9F AHX, $9B TAS, $9C SHY, $9E SHX,
//! $BB LAS: their usual "magic constant $FF" forms). The twelve STP opcodes stop the CPU
//! (`jammed`).

pub trait Bus {
    fn read(&mut self, a: u16) -> u8;
    fn write(&mut self, a: u16, v: u8);
}

pub const C: u8 = 0x01;
pub const Z: u8 = 0x02;
pub const I: u8 = 0x04;
pub const D: u8 = 0x08;
pub const B: u8 = 0x10;
pub const U: u8 = 0x20;
pub const V: u8 = 0x40;
pub const N: u8 = 0x80;

#[derive(Clone, Debug)]
pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub s: u8,
    pub p: u8,
    pub pc: u16,
    /// Cycles run since `new`.
    pub cycles: u64,
    /// An STP opcode stopped the CPU: `step` does nothing until it is cleared.
    pub jammed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Imm,
    Zp,
    Zpx,
    Zpy,
    Abs,
    Abx,
    Aby,
    Izx,
    Izy,
}

/// What the instruction does with its operand, for the cycle count: a read (the indexed modes
/// take a cycle more on a page crossing), a write or a read-modify-write (always the longest).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    R,
    W,
    M,
}

use Kind::*;
use Mode::*;

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Cpu {
        Cpu { a: 0, x: 0, y: 0, s: 0xFD, p: I | U, pc: 0, cycles: 0, jammed: false }
    }

    fn fetch(&mut self, b: &mut impl Bus) -> u8 {
        let v = b.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16(&mut self, b: &mut impl Bus) -> u16 {
        let lo = self.fetch(b) as u16;
        lo | (self.fetch(b) as u16) << 8
    }

    fn rd16_zp(b: &mut impl Bus, z: u8) -> u16 {
        b.read(z as u16) as u16 | (b.read(z.wrapping_add(1) as u16) as u16) << 8
    }

    pub fn push(&mut self, b: &mut impl Bus, v: u8) {
        b.write(0x100 | self.s as u16, v);
        self.s = self.s.wrapping_sub(1);
    }

    pub fn pull(&mut self, b: &mut impl Bus) -> u8 {
        self.s = self.s.wrapping_add(1);
        b.read(0x100 | self.s as u16)
    }

    fn nz(&mut self, v: u8) {
        self.p = (self.p & !(N | Z)) | (v & N) | if v == 0 { Z } else { 0 };
    }

    fn flag(&mut self, f: u8, on: bool) {
        if on {
            self.p |= f;
        } else {
            self.p &= !f;
        }
    }

    /// The operand's address (for `Imm`, the operand byte's), and the cycles of the mode.
    fn ea(&mut self, b: &mut impl Bus, m: Mode, k: Kind) -> (u16, u32) {
        let cross = |base: u16, a: u16| (base & 0xFF00 != a & 0xFF00) as u32;
        match m {
            Imm => {
                let a = self.pc;
                self.pc = self.pc.wrapping_add(1);
                (a, 2)
            }
            Zp => (self.fetch(b) as u16, if k == M { 5 } else { 3 }),
            Zpx => (self.fetch(b).wrapping_add(self.x) as u16, if k == M { 6 } else { 4 }),
            Zpy => (self.fetch(b).wrapping_add(self.y) as u16, if k == M { 6 } else { 4 }),
            Abs => (self.fetch16(b), if k == M { 6 } else { 4 }),
            Abx | Aby => {
                let base = self.fetch16(b);
                let a = base.wrapping_add(if m == Abx { self.x } else { self.y } as u16);
                (a, match k {
                    R => 4 + cross(base, a),
                    W => 5,
                    M => 7,
                })
            }
            Izx => {
                let z = self.fetch(b).wrapping_add(self.x);
                (Self::rd16_zp(b, z), if k == M { 8 } else { 6 })
            }
            Izy => {
                let z = self.fetch(b);
                let base = Self::rd16_zp(b, z);
                let a = base.wrapping_add(self.y as u16);
                (a, match k {
                    R => 5 + cross(base, a),
                    W => 6,
                    M => 8,
                })
            }
        }
    }

    /// The addressing mode of an opcode that has an operand in memory (or an immediate).
    fn mode(op: u8) -> Mode {
        let bbb = (op >> 2) & 7;
        let cc = op & 3;
        let hi = op >> 4;
        // the X-indexed modes of STX / LDX / SAX / LAX / SHX / AHX index by Y
        let by_y = (hi == 9 || hi == 0xB) && cc >= 2;
        match (cc, bbb) {
            (_, 0) if cc & 1 == 1 => Izx,
            (_, 0) => Imm,
            (_, 1) => Zp,
            (_, 2) => Imm,
            (_, 3) => Abs,
            (_, 4) => Izy,
            (_, 5) => {
                if by_y {
                    Zpy
                } else {
                    Zpx
                }
            }
            (_, 6) => Aby,
            _ => {
                if by_y {
                    Aby
                } else {
                    Abx
                }
            }
        }
    }

    fn operand(&mut self, b: &mut impl Bus, op: u8) -> (u8, u32) {
        let (a, c) = self.ea(b, Self::mode(op), R);
        (b.read(a), c)
    }

    fn adc(&mut self, m: u8) {
        let s = self.a as u16 + m as u16 + (self.p & C) as u16;
        let r = s as u8;
        self.flag(C, s > 0xFF);
        self.flag(V, (r ^ self.a) & (r ^ m) & 0x80 != 0);
        self.a = r;
        self.nz(r);
    }

    fn sbc(&mut self, m: u8) {
        // A - M - (1 - C) is A + !M + C
        self.adc(!m);
    }

    fn cmp(&mut self, r: u8, m: u8) {
        let d = r.wrapping_sub(m);
        self.flag(C, r >= m);
        self.nz(d);
    }

    fn asl(&mut self, v: u8) -> u8 {
        self.flag(C, v & 0x80 != 0);
        let r = v << 1;
        self.nz(r);
        r
    }
    fn lsr(&mut self, v: u8) -> u8 {
        self.flag(C, v & 1 != 0);
        let r = v >> 1;
        self.nz(r);
        r
    }
    fn rol(&mut self, v: u8) -> u8 {
        let r = v << 1 | (self.p & C);
        self.flag(C, v & 0x80 != 0);
        self.nz(r);
        r
    }
    fn ror(&mut self, v: u8) -> u8 {
        let r = v >> 1 | (self.p & C) << 7;
        self.flag(C, v & 1 != 0);
        self.nz(r);
        r
    }

    /// Read-modify-write: the new value is written back; returns it and the cycles.
    fn rmw(&mut self, b: &mut impl Bus, op: u8, f: impl FnOnce(&mut Self, u8) -> u8) -> (u8, u32) {
        let (a, c) = self.ea(b, Self::mode(op), M);
        let v = b.read(a);
        let r = f(self, v);
        b.write(a, r);
        (r, c)
    }

    fn store(&mut self, b: &mut impl Bus, op: u8, v: u8) -> u32 {
        let (a, c) = self.ea(b, Self::mode(op), W);
        b.write(a, v);
        c
    }

    /// The unstable stores (SHY, SHX, AHX, TAS): `v` AND the high byte of the base address
    /// plus one.
    fn store_hi(&mut self, b: &mut impl Bus, op: u8, v: u8) -> u32 {
        let (a, c) = self.ea(b, Self::mode(op), W);
        let idx = if op == 0x9C { self.x } else { self.y };
        let base_hi = (a.wrapping_sub(idx as u16) >> 8) as u8;
        b.write(a, v & base_hi.wrapping_add(1));
        c
    }

    fn branch(&mut self, b: &mut impl Bus, cond: bool) -> u32 {
        let off = self.fetch(b) as i8;
        if !cond {
            return 2;
        }
        let t = self.pc.wrapping_add(off as u16);
        let c = if t & 0xFF00 != self.pc & 0xFF00 { 4 } else { 3 };
        self.pc = t;
        c
    }

    /// Runs one instruction; returns its cycles.
    pub fn step(&mut self, b: &mut impl Bus) -> u32 {
        if self.jammed {
            return 2;
        }
        let op = self.fetch(b);
        let c = self.exec(b, op);
        self.cycles += c as u64;
        c
    }

    fn exec(&mut self, b: &mut impl Bus, op: u8) -> u32 {
        match op {
            // ---------------------------------------------------------------- ALU group
            0x01 | 0x05 | 0x09 | 0x0D | 0x11 | 0x15 | 0x19 | 0x1D => {
                let (v, c) = self.operand(b, op);
                self.a |= v;
                self.nz(self.a);
                c
            }
            0x21 | 0x25 | 0x29 | 0x2D | 0x31 | 0x35 | 0x39 | 0x3D => {
                let (v, c) = self.operand(b, op);
                self.a &= v;
                self.nz(self.a);
                c
            }
            0x41 | 0x45 | 0x49 | 0x4D | 0x51 | 0x55 | 0x59 | 0x5D => {
                let (v, c) = self.operand(b, op);
                self.a ^= v;
                self.nz(self.a);
                c
            }
            0x61 | 0x65 | 0x69 | 0x6D | 0x71 | 0x75 | 0x79 | 0x7D => {
                let (v, c) = self.operand(b, op);
                self.adc(v);
                c
            }
            0x81 | 0x85 | 0x8D | 0x91 | 0x95 | 0x99 | 0x9D => self.store(b, op, self.a),
            0xA1 | 0xA5 | 0xA9 | 0xAD | 0xB1 | 0xB5 | 0xB9 | 0xBD => {
                let (v, c) = self.operand(b, op);
                self.a = v;
                self.nz(v);
                c
            }
            0xC1 | 0xC5 | 0xC9 | 0xCD | 0xD1 | 0xD5 | 0xD9 | 0xDD => {
                let (v, c) = self.operand(b, op);
                self.cmp(self.a, v);
                c
            }
            0xE1 | 0xE5 | 0xE9 | 0xED | 0xF1 | 0xF5 | 0xF9 | 0xFD | 0xEB => {
                let (v, c) = self.operand(b, op);
                self.sbc(v);
                c
            }
            // ---------------------------------------------------------------- shifts, INC, DEC
            0x0A => {
                self.a = self.asl(self.a);
                2
            }
            0x4A => {
                self.a = self.lsr(self.a);
                2
            }
            0x2A => {
                self.a = self.rol(self.a);
                2
            }
            0x6A => {
                self.a = self.ror(self.a);
                2
            }
            0x06 | 0x0E | 0x16 | 0x1E => self.rmw(b, op, Self::asl).1,
            0x46 | 0x4E | 0x56 | 0x5E => self.rmw(b, op, Self::lsr).1,
            0x26 | 0x2E | 0x36 | 0x3E => self.rmw(b, op, Self::rol).1,
            0x66 | 0x6E | 0x76 | 0x7E => self.rmw(b, op, Self::ror).1,
            0xC6 | 0xCE | 0xD6 | 0xDE => {
                self.rmw(b, op, |s, v| {
                    let r = v.wrapping_sub(1);
                    s.nz(r);
                    r
                })
                .1
            }
            0xE6 | 0xEE | 0xF6 | 0xFE => {
                self.rmw(b, op, |s, v| {
                    let r = v.wrapping_add(1);
                    s.nz(r);
                    r
                })
                .1
            }
            // ---------------------------------------------------------------- X, Y loads and stores
            0xA2 | 0xA6 | 0xAE | 0xB6 | 0xBE => {
                let (v, c) = self.operand(b, op);
                self.x = v;
                self.nz(v);
                c
            }
            0xA0 | 0xA4 | 0xAC | 0xB4 | 0xBC => {
                let (v, c) = self.operand(b, op);
                self.y = v;
                self.nz(v);
                c
            }
            0x86 | 0x8E | 0x96 => self.store(b, op, self.x),
            0x84 | 0x8C | 0x94 => self.store(b, op, self.y),
            0xE0 | 0xE4 | 0xEC => {
                let (v, c) = self.operand(b, op);
                self.cmp(self.x, v);
                c
            }
            0xC0 | 0xC4 | 0xCC => {
                let (v, c) = self.operand(b, op);
                self.cmp(self.y, v);
                c
            }
            0x24 | 0x2C => {
                let (v, c) = self.operand(b, op);
                self.flag(Z, self.a & v == 0);
                self.p = (self.p & !(N | V)) | (v & (N | V));
                c
            }
            // ---------------------------------------------------------------- implied
            0xAA => {
                self.x = self.a;
                self.nz(self.x);
                2
            }
            0xA8 => {
                self.y = self.a;
                self.nz(self.y);
                2
            }
            0x8A => {
                self.a = self.x;
                self.nz(self.a);
                2
            }
            0x98 => {
                self.a = self.y;
                self.nz(self.a);
                2
            }
            0xBA => {
                self.x = self.s;
                self.nz(self.x);
                2
            }
            0x9A => {
                self.s = self.x;
                2
            }
            0xE8 => {
                self.x = self.x.wrapping_add(1);
                self.nz(self.x);
                2
            }
            0xC8 => {
                self.y = self.y.wrapping_add(1);
                self.nz(self.y);
                2
            }
            0xCA => {
                self.x = self.x.wrapping_sub(1);
                self.nz(self.x);
                2
            }
            0x88 => {
                self.y = self.y.wrapping_sub(1);
                self.nz(self.y);
                2
            }
            0x18 => {
                self.p &= !C;
                2
            }
            0x38 => {
                self.p |= C;
                2
            }
            0x58 => {
                self.p &= !I;
                2
            }
            0x78 => {
                self.p |= I;
                2
            }
            0xB8 => {
                self.p &= !V;
                2
            }
            0xD8 => {
                self.p &= !D;
                2
            }
            0xF8 => {
                self.p |= D;
                2
            }
            0xEA | 0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => 2,
            // ---------------------------------------------------------------- stack
            0x48 => {
                self.push(b, self.a);
                3
            }
            0x08 => {
                self.push(b, self.p | B | U);
                3
            }
            0x68 => {
                self.a = self.pull(b);
                self.nz(self.a);
                4
            }
            0x28 => {
                self.p = (self.pull(b) & !B) | U;
                4
            }
            // ---------------------------------------------------------------- flow
            0x4C => {
                self.pc = self.fetch16(b);
                3
            }
            0x6C => {
                let p = self.fetch16(b);
                // the high byte comes from the same page
                let hi = (p & 0xFF00) | (p.wrapping_add(1) & 0x00FF);
                self.pc = b.read(p) as u16 | (b.read(hi) as u16) << 8;
                5
            }
            0x20 => {
                let t = self.fetch16(b);
                let r = self.pc.wrapping_sub(1);
                self.push(b, (r >> 8) as u8);
                self.push(b, r as u8);
                self.pc = t;
                6
            }
            0x60 => {
                let lo = self.pull(b) as u16;
                let hi = self.pull(b) as u16;
                self.pc = (lo | hi << 8).wrapping_add(1);
                6
            }
            0x40 => {
                self.p = (self.pull(b) & !B) | U;
                let lo = self.pull(b) as u16;
                let hi = self.pull(b) as u16;
                self.pc = lo | hi << 8;
                6
            }
            0x00 => {
                let r = self.pc.wrapping_add(1);
                self.push(b, (r >> 8) as u8);
                self.push(b, r as u8);
                self.push(b, self.p | B | U);
                self.p |= I;
                self.pc = b.read(0xFFFE) as u16 | (b.read(0xFFFF) as u16) << 8;
                7
            }
            0x10 => self.branch(b, self.p & N == 0),
            0x30 => self.branch(b, self.p & N != 0),
            0x50 => self.branch(b, self.p & V == 0),
            0x70 => self.branch(b, self.p & V != 0),
            0x90 => self.branch(b, self.p & C == 0),
            0xB0 => self.branch(b, self.p & C != 0),
            0xD0 => self.branch(b, self.p & Z == 0),
            0xF0 => self.branch(b, self.p & Z != 0),
            // ---------------------------------------------------------------- unofficial
            0x02 | 0x12 | 0x22 | 0x32 | 0x42 | 0x52 | 0x62 | 0x72 | 0x92 | 0xB2 | 0xD2 | 0xF2 => {
                self.pc = self.pc.wrapping_sub(1);
                self.jammed = true;
                2
            }
            // NOPs that read an operand
            0x80 | 0x82 | 0x89 | 0xC2 | 0xE2 | 0x04 | 0x44 | 0x64 | 0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 | 0x0C | 0x1C
            | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => self.operand(b, op).1,
            0x03 | 0x07 | 0x0F | 0x13 | 0x17 | 0x1B | 0x1F => {
                let (r, c) = self.rmw(b, op, Self::asl);
                self.a |= r;
                self.nz(self.a);
                c
            }
            0x23 | 0x27 | 0x2F | 0x33 | 0x37 | 0x3B | 0x3F => {
                let (r, c) = self.rmw(b, op, Self::rol);
                self.a &= r;
                self.nz(self.a);
                c
            }
            0x43 | 0x47 | 0x4F | 0x53 | 0x57 | 0x5B | 0x5F => {
                let (r, c) = self.rmw(b, op, Self::lsr);
                self.a ^= r;
                self.nz(self.a);
                c
            }
            0x63 | 0x67 | 0x6F | 0x73 | 0x77 | 0x7B | 0x7F => {
                let (r, c) = self.rmw(b, op, Self::ror);
                self.adc(r);
                c
            }
            0x83 | 0x87 | 0x8F | 0x97 => self.store(b, op, self.a & self.x),
            0xA3 | 0xA7 | 0xAB | 0xAF | 0xB3 | 0xB7 | 0xBF => {
                let (v, c) = self.operand(b, op);
                self.a = v;
                self.x = v;
                self.nz(v);
                c
            }
            0xC3 | 0xC7 | 0xCF | 0xD3 | 0xD7 | 0xDB | 0xDF => {
                let (r, c) = self.rmw(b, op, |_, v| v.wrapping_sub(1));
                self.cmp(self.a, r);
                c
            }
            0xE3 | 0xE7 | 0xEF | 0xF3 | 0xF7 | 0xFB | 0xFF => {
                let (r, c) = self.rmw(b, op, |_, v| v.wrapping_add(1));
                self.sbc(r);
                c
            }
            0x0B | 0x2B => {
                let (v, c) = self.operand(b, op);
                self.a &= v;
                self.nz(self.a);
                self.flag(C, self.a & 0x80 != 0);
                c
            }
            0x4B => {
                let (v, c) = self.operand(b, op);
                self.a &= v;
                self.a = self.lsr(self.a);
                c
            }
            0x6B => {
                let (v, c) = self.operand(b, op);
                let t = self.a & v;
                self.a = t >> 1 | (self.p & C) << 7;
                self.nz(self.a);
                self.flag(C, self.a & 0x40 != 0);
                self.flag(V, ((self.a >> 6) ^ (self.a >> 5)) & 1 != 0);
                c
            }
            0xCB => {
                let (v, c) = self.operand(b, op);
                let t = self.a & self.x;
                self.flag(C, t >= v);
                self.x = t.wrapping_sub(v);
                self.nz(self.x);
                c
            }
            0x8B => {
                let (v, c) = self.operand(b, op);
                self.a = self.x & v;
                self.nz(self.a);
                c
            }
            0xBB => {
                let (v, c) = self.operand(b, op);
                let r = v & self.s;
                self.a = r;
                self.x = r;
                self.s = r;
                self.nz(r);
                c
            }
            0x93 | 0x9F => self.store_hi(b, op, self.a & self.x),
            0x9B => {
                self.s = self.a & self.x;
                self.store_hi(b, op, self.s)
            }
            0x9C => self.store_hi(b, op, self.y),
            0x9E => self.store_hi(b, op, self.x),
        }
    }
}
