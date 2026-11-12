//! A Zilog Z80 interpreter for the AY player (`ay.rs`): the whole instruction set, documented
//! and undocumented (the CB, ED, DD, FD, DDCB and FDCB pages; IXH/IXL/IYH/IYL; SLL; the
//! register copies of the DDCB/FDCB rotates; the ED duplicates of NEG, RETN and IM; IN F,(C)
//! and OUT (C),0), with the flags bits 3 and 5, the MEMPTR register as far as BIT n,(HL) shows
//! it, the refresh register, the three interrupt modes and HALT.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from public
//! documentation of the processor: Zilog's Z80 CPU User Manual (UM0080: instruction set,
//! T-states, flags, interrupt modes) and Sean Young's "The Undocumented Z80 Documented"
//! (version 0.91; flags bits 3/5, block instruction flags, DAA, the DDCB register copies,
//! MEMPTR). No emulator's source code was consulted or transcribed.
//!
//! T-states follow the manual's tables. Contention and the exact bus cycles are not modelled
//! (the AY files run in flat RAM). The instruction stepping is bounded by its caller: every
//! instruction costs at least 4 T-states and the caller runs a fixed number of T-states.

/// The memory and I/O the processor sees. `t` is the processor's T-state count when the I/O
/// cycle happens (its start, close enough for audio).
pub trait Bus {
    fn read(&mut self, a: u16) -> u8;
    fn write(&mut self, a: u16, v: u8);
    fn input(&mut self, port: u16, t: u64) -> u8;
    fn output(&mut self, port: u16, v: u8, t: u64);
}

pub const CF: u8 = 0x01;
pub const NF: u8 = 0x02;
pub const PF: u8 = 0x04;
pub const XF: u8 = 0x08;
pub const HF: u8 = 0x10;
pub const YF: u8 = 0x20;
pub const ZF: u8 = 0x40;
pub const SF: u8 = 0x80;

const fn tables() -> ([u8; 256], [u8; 256]) {
    let mut sz53 = [0u8; 256];
    let mut sz53p = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let v = i as u8;
        let f = (v & (SF | YF | XF)) | if v == 0 { ZF } else { 0 };
        sz53[i] = f;
        sz53p[i] = f | if v.count_ones() % 2 == 0 { PF } else { 0 };
        i += 1;
    }
    (sz53, sz53p)
}

const TABLES: ([u8; 256], [u8; 256]) = tables();
const SZ53: [u8; 256] = TABLES.0;
const SZ53P: [u8; 256] = TABLES.1;

#[derive(Clone)]
pub struct Z80 {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    /// The alternate set: AF', BC', DE', HL'.
    pub af2: u16,
    pub bc2: u16,
    pub de2: u16,
    pub hl2: u16,
    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,
    /// MEMPTR ("WZ"), the internal address latch that shows in BIT n,(HL).
    pub wz: u16,
    pub i: u8,
    pub r: u8,
    pub iff1: bool,
    pub iff2: bool,
    pub im: u8,
    pub halted: bool,
    /// The instruction just run was EI: no interrupt before the next one.
    ei_delay: bool,
    /// The index register the current instruction uses in place of HL: 0 HL, 1 IX, 2 IY.
    xy: u8,
    /// T-states run since the processor was made.
    pub t: u64,
}

impl Default for Z80 {
    fn default() -> Self {
        Z80::new()
    }
}

impl Z80 {
    pub fn new() -> Z80 {
        Z80 {
            a: 0xFF,
            f: 0xFF,
            b: 0,
            c: 0,
            d: 0,
            e: 0,
            h: 0,
            l: 0,
            af2: 0,
            bc2: 0,
            de2: 0,
            hl2: 0,
            ix: 0,
            iy: 0,
            sp: 0xFFFF,
            pc: 0,
            wz: 0,
            i: 0,
            r: 0,
            iff1: false,
            iff2: false,
            im: 0,
            halted: false,
            ei_delay: false,
            xy: 0,
            t: 0,
        }
    }

    // ------------------------------------------------------------ register pairs

    pub fn af(&self) -> u16 {
        (self.a as u16) << 8 | self.f as u16
    }
    pub fn bc(&self) -> u16 {
        (self.b as u16) << 8 | self.c as u16
    }
    pub fn de(&self) -> u16 {
        (self.d as u16) << 8 | self.e as u16
    }
    pub fn hl(&self) -> u16 {
        (self.h as u16) << 8 | self.l as u16
    }
    pub fn set_af(&mut self, v: u16) {
        self.a = (v >> 8) as u8;
        self.f = v as u8;
    }
    pub fn set_bc(&mut self, v: u16) {
        self.b = (v >> 8) as u8;
        self.c = v as u8;
    }
    pub fn set_de(&mut self, v: u16) {
        self.d = (v >> 8) as u8;
        self.e = v as u8;
    }
    pub fn set_hl(&mut self, v: u16) {
        self.h = (v >> 8) as u8;
        self.l = v as u8;
    }

    /// HL, IX or IY, as the current prefix says.
    fn hx(&self) -> u16 {
        match self.xy {
            0 => self.hl(),
            1 => self.ix,
            _ => self.iy,
        }
    }
    fn set_hx(&mut self, v: u16) {
        match self.xy {
            0 => self.set_hl(v),
            1 => self.ix = v,
            _ => self.iy = v,
        }
    }

    /// BC, DE, HL/IX/IY, SP by the two-bit code of the instruction.
    fn rp(&self, p: u8) -> u16 {
        match p & 3 {
            0 => self.bc(),
            1 => self.de(),
            2 => self.hx(),
            _ => self.sp,
        }
    }
    fn set_rp(&mut self, p: u8, v: u16) {
        match p & 3 {
            0 => self.set_bc(v),
            1 => self.set_de(v),
            2 => self.set_hx(v),
            _ => self.sp = v,
        }
    }

    /// An 8-bit register by its three-bit code (not 6, the memory operand); H and L are the
    /// halves of IX or IY under a prefix.
    fn r8(&self, code: u8) -> u8 {
        match code & 7 {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => (self.hx() >> 8) as u8,
            5 => self.hx() as u8,
            7 => self.a,
            _ => 0,
        }
    }
    fn set_r8(&mut self, code: u8, v: u8) {
        match code & 7 {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => {
                let x = self.hx();
                self.set_hx((x & 0x00FF) | (v as u16) << 8);
            }
            5 => {
                let x = self.hx();
                self.set_hx((x & 0xFF00) | v as u16);
            }
            7 => self.a = v,
            _ => {}
        }
    }
    /// The plain registers, whatever the prefix (the register operand of LD r,(IX+d)).
    fn r8_plain(&self, code: u8) -> u8 {
        match code & 7 {
            4 => self.h,
            5 => self.l,
            c => self.r8(c),
        }
    }
    fn set_r8_plain(&mut self, code: u8, v: u8) {
        match code & 7 {
            4 => self.h = v,
            5 => self.l = v,
            c => self.set_r8(c, v),
        }
    }

    // ------------------------------------------------------------ fetching

    fn inc_r(&mut self) {
        self.r = (self.r & 0x80) | (self.r.wrapping_add(1) & 0x7F);
    }
    fn fetch<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }
    fn fetch16<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        hi << 8 | lo
    }
    fn read16<B: Bus>(bus: &mut B, a: u16) -> u16 {
        let lo = bus.read(a) as u16;
        let hi = bus.read(a.wrapping_add(1)) as u16;
        hi << 8 | lo
    }
    fn write16<B: Bus>(bus: &mut B, a: u16, v: u16) {
        bus.write(a, v as u8);
        bus.write(a.wrapping_add(1), (v >> 8) as u8);
    }
    fn push<B: Bus>(&mut self, bus: &mut B, v: u16) {
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, (v >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, v as u8);
    }
    fn pop<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let v = Self::read16(bus, self.sp);
        self.sp = self.sp.wrapping_add(2);
        v
    }

    /// The address of the memory operand: (HL), or (IX+d)/(IY+d) with the displacement
    /// fetched; the second value is the extra T-states the displacement costs.
    fn addr_m<B: Bus>(&mut self, bus: &mut B) -> (u16, u32) {
        if self.xy == 0 {
            (self.hl(), 0)
        } else {
            let d = self.fetch(bus) as i8;
            let a = self.hx().wrapping_add(d as u16);
            self.wz = a;
            (a, 8)
        }
    }

    // ------------------------------------------------------------ ALU

    fn add8(&mut self, v: u8, carry: bool) {
        let a = self.a;
        let r16 = a as u16 + v as u16 + carry as u16;
        let r = r16 as u8;
        let mut f = SZ53[r as usize] | ((a ^ v ^ r) & HF);
        if r16 > 0xFF {
            f |= CF;
        }
        if (a ^ v) & 0x80 == 0 && (a ^ r) & 0x80 != 0 {
            f |= PF;
        }
        self.a = r;
        self.f = f;
    }

    /// Subtraction; `store` false is CP (flags 3 and 5 from the operand).
    fn sub8(&mut self, v: u8, carry: bool, store: bool) {
        let a = self.a;
        let r16 = (a as u16).wrapping_sub(v as u16).wrapping_sub(carry as u16);
        let r = r16 as u8;
        let mut f = (SZ53[r as usize] & (SF | ZF)) | NF | ((a ^ v ^ r) & HF);
        if r16 > 0xFF {
            f |= CF;
        }
        if (a ^ v) & 0x80 != 0 && (a ^ r) & 0x80 != 0 {
            f |= PF;
        }
        f |= if store { r } else { v } & (YF | XF);
        self.f = f;
        if store {
            self.a = r;
        }
    }

    fn alu(&mut self, op: u8, v: u8) {
        match op & 7 {
            0 => self.add8(v, false),
            1 => {
                let c = self.f & CF != 0;
                self.add8(v, c)
            }
            2 => self.sub8(v, false, true),
            3 => {
                let c = self.f & CF != 0;
                self.sub8(v, c, true)
            }
            4 => {
                self.a &= v;
                self.f = SZ53P[self.a as usize] | HF;
            }
            5 => {
                self.a ^= v;
                self.f = SZ53P[self.a as usize];
            }
            6 => {
                self.a |= v;
                self.f = SZ53P[self.a as usize];
            }
            _ => self.sub8(v, false, false),
        }
    }

    fn inc8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        let mut f = (self.f & CF) | SZ53[r as usize];
        if r == 0x80 {
            f |= PF;
        }
        if r & 0x0F == 0 {
            f |= HF;
        }
        self.f = f;
        r
    }

    fn dec8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        let mut f = (self.f & CF) | NF | SZ53[r as usize];
        if v == 0x80 {
            f |= PF;
        }
        if v & 0x0F == 0 {
            f |= HF;
        }
        self.f = f;
        r
    }

    fn add16(&mut self, a: u16, b: u16) -> u16 {
        let r32 = a as u32 + b as u32;
        let r = r32 as u16;
        let mut f = (self.f & (SF | ZF | PF)) | ((r >> 8) as u8 & (YF | XF)) | (((a ^ b ^ r) >> 8) as u8 & HF);
        if r32 > 0xFFFF {
            f |= CF;
        }
        self.f = f;
        self.wz = a.wrapping_add(1);
        r
    }

    fn adc16(&mut self, v: u16) {
        let hl = self.hl();
        let c = (self.f & CF) as u32;
        let r32 = hl as u32 + v as u32 + c;
        let r = r32 as u16;
        let mut f = ((r >> 8) as u8 & (SF | YF | XF)) | (((hl ^ v ^ r) >> 8) as u8 & HF);
        if r == 0 {
            f |= ZF;
        }
        if r32 > 0xFFFF {
            f |= CF;
        }
        if (hl ^ v) & 0x8000 == 0 && (hl ^ r) & 0x8000 != 0 {
            f |= PF;
        }
        self.f = f;
        self.wz = hl.wrapping_add(1);
        self.set_hl(r);
    }

    fn sbc16(&mut self, v: u16) {
        let hl = self.hl();
        let c = (self.f & CF) as u32;
        let r32 = (hl as u32).wrapping_sub(v as u32).wrapping_sub(c);
        let r = r32 as u16;
        let mut f = NF | ((r >> 8) as u8 & (SF | YF | XF)) | (((hl ^ v ^ r) >> 8) as u8 & HF);
        if r == 0 {
            f |= ZF;
        }
        if r32 > 0xFFFF {
            f |= CF;
        }
        if (hl ^ v) & 0x8000 != 0 && (hl ^ r) & 0x8000 != 0 {
            f |= PF;
        }
        self.f = f;
        self.wz = hl.wrapping_add(1);
        self.set_hl(r);
    }

    fn daa(&mut self) {
        let a = self.a;
        let mut corr = 0u8;
        let mut c = self.f & CF;
        if self.f & HF != 0 || a & 0x0F > 9 {
            corr |= 0x06;
        }
        if c != 0 || a > 0x99 {
            corr |= 0x60;
            c = CF;
        }
        let n = self.f & NF != 0;
        let r = if n { a.wrapping_sub(corr) } else { a.wrapping_add(corr) };
        let h = if n { self.f & HF != 0 && a & 0x0F < 6 } else { a & 0x0F > 9 };
        self.f = SZ53P[r as usize] | (self.f & NF) | c | if h { HF } else { 0 };
        self.a = r;
    }

    /// The CB-page shifts and rotates (0 RLC, 1 RRC, 2 RL, 3 RR, 4 SLA, 5 SRA, 6 SLL, 7 SRL).
    fn rot(&mut self, op: u8, v: u8) -> u8 {
        let c = self.f & CF;
        let (r, nc) = match op & 7 {
            0 => (v.rotate_left(1), v >> 7),
            1 => (v.rotate_right(1), v & 1),
            2 => ((v << 1) | c, v >> 7),
            3 => ((v >> 1) | (c << 7), v & 1),
            4 => (v << 1, v >> 7),
            5 => ((v >> 1) | (v & 0x80), v & 1),
            6 => ((v << 1) | 1, v >> 7),
            _ => (v >> 1, v & 1),
        };
        self.f = SZ53P[r as usize] | nc;
        r
    }

    /// BIT n: flags 3 and 5 come from `xy` (the operand, MEMPTR's high byte, or the indexed
    /// address's high byte).
    fn bit(&mut self, n: u8, v: u8, xy: u8) {
        let b = v & (1 << n);
        let mut f = (self.f & CF) | HF | (xy & (YF | XF));
        if b == 0 {
            f |= ZF | PF;
        }
        if n == 7 && b != 0 {
            f |= SF;
        }
        self.f = f;
    }

    fn cond(&self, c: u8) -> bool {
        match c & 7 {
            0 => self.f & ZF == 0,
            1 => self.f & ZF != 0,
            2 => self.f & CF == 0,
            3 => self.f & CF != 0,
            4 => self.f & PF == 0,
            5 => self.f & PF != 0,
            6 => self.f & SF == 0,
            _ => self.f & SF != 0,
        }
    }

    // ------------------------------------------------------------ running

    /// Accepts a maskable interrupt if it is enabled (the data bus holds $FF, so mode 0 runs
    /// RST $38, as on the Spectrum). Returns the T-states taken, 0 if refused.
    pub fn interrupt<B: Bus>(&mut self, bus: &mut B) -> u32 {
        if !self.iff1 || self.ei_delay {
            return 0;
        }
        self.halted = false;
        self.iff1 = false;
        self.iff2 = false;
        self.inc_r();
        let pc = self.pc;
        self.push(bus, pc);
        let t = if self.im == 2 {
            let v = (self.i as u16) << 8 | 0xFF;
            self.pc = Self::read16(bus, v);
            19
        } else {
            self.pc = 0x0038;
            13
        };
        self.wz = self.pc;
        self.t += t as u64;
        t
    }

    /// Runs one instruction (with its prefixes); while halted, one NOP. Returns the T-states.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        self.ei_delay = false;
        if self.halted {
            self.inc_r();
            self.t += 4;
            return 4;
        }
        let mut t = 0;
        self.xy = 0;
        let mut op = self.fetch(bus);
        self.inc_r();
        if op == 0xDD || op == 0xFD {
            self.xy = if op == 0xDD { 1 } else { 2 };
            t += 4;
            op = self.fetch(bus);
            if op == 0xDD || op == 0xFD {
                // a prefix followed by another counts as a NOP of its own; the next step
                // starts at the second (so a run of prefixes cannot hold up the caller)
                self.pc = self.pc.wrapping_sub(1);
                self.xy = 0;
                self.t += 4;
                return 4;
            }
            self.inc_r();
        }
        t += match op {
            0xCB if self.xy != 0 => self.exec_xycb(bus),
            0xCB => {
                let op2 = self.fetch(bus);
                self.inc_r();
                self.exec_cb(bus, op2)
            }
            0xED => {
                self.xy = 0;
                let op2 = self.fetch(bus);
                self.inc_r();
                self.exec_ed(bus, op2)
            }
            _ => self.exec(bus, op),
        };
        self.xy = 0;
        self.t += t as u64;
        t
    }

    /// Skips `n` T-states of a HALT (as `n / 4` NOPs).
    pub fn halt_skip(&mut self, n: u64) {
        let k = n.div_ceil(4);
        self.r = (self.r & 0x80) | ((self.r as u64 + k) as u8 & 0x7F);
        self.t += k * 4;
    }

    fn exec<B: Bus>(&mut self, bus: &mut B, op: u8) -> u32 {
        let y = (op >> 3) & 7;
        let z = op & 7;
        let p = (op >> 4) & 3;
        match op {
            0x00 => 4,
            0x01 | 0x11 | 0x21 | 0x31 => {
                let v = self.fetch16(bus);
                self.set_rp(p, v);
                10
            }
            0x02 | 0x12 => {
                let a = if op == 0x02 { self.bc() } else { self.de() };
                bus.write(a, self.a);
                self.wz = (self.a as u16) << 8 | (a.wrapping_add(1) & 0xFF);
                7
            }
            0x0A | 0x1A => {
                let a = if op == 0x0A { self.bc() } else { self.de() };
                self.a = bus.read(a);
                self.wz = a.wrapping_add(1);
                7
            }
            0x03 | 0x13 | 0x23 | 0x33 => {
                let v = self.rp(p).wrapping_add(1);
                self.set_rp(p, v);
                6
            }
            0x0B | 0x1B | 0x2B | 0x3B => {
                let v = self.rp(p).wrapping_sub(1);
                self.set_rp(p, v);
                6
            }
            _ if op & 0xC7 == 0x04 || op & 0xC7 == 0x05 => {
                let dec = op & 1 != 0;
                if y == 6 {
                    let (a, x) = self.addr_m(bus);
                    let v = bus.read(a);
                    let r = if dec { self.dec8(v) } else { self.inc8(v) };
                    bus.write(a, r);
                    11 + x
                } else {
                    let v = self.r8(y);
                    let r = if dec { self.dec8(v) } else { self.inc8(v) };
                    self.set_r8(y, r);
                    4
                }
            }
            _ if op & 0xC7 == 0x06 => {
                if y == 6 {
                    let (a, x) = self.addr_m(bus);
                    let n = self.fetch(bus);
                    bus.write(a, n);
                    if x != 0 {
                        15
                    } else {
                        10
                    }
                } else {
                    let n = self.fetch(bus);
                    self.set_r8(y, n);
                    7
                }
            }
            0x07 | 0x0F | 0x17 | 0x1F => {
                let a = self.a;
                let c = self.f & CF;
                let (r, nc) = match op {
                    0x07 => (a.rotate_left(1), a >> 7),
                    0x0F => (a.rotate_right(1), a & 1),
                    0x17 => ((a << 1) | c, a >> 7),
                    _ => ((a >> 1) | (c << 7), a & 1),
                };
                self.a = r;
                self.f = (self.f & (SF | ZF | PF)) | (r & (YF | XF)) | nc;
                4
            }
            0x08 => {
                let af = self.af();
                self.set_af(self.af2);
                self.af2 = af;
                4
            }
            0x09 | 0x19 | 0x29 | 0x39 => {
                let a = self.hx();
                let b = self.rp(p);
                let r = self.add16(a, b);
                self.set_hx(r);
                11
            }
            0x10 => {
                let d = self.fetch(bus) as i8;
                self.b = self.b.wrapping_sub(1);
                if self.b != 0 {
                    self.pc = self.pc.wrapping_add(d as u16);
                    self.wz = self.pc;
                    13
                } else {
                    8
                }
            }
            0x18 => {
                let d = self.fetch(bus) as i8;
                self.pc = self.pc.wrapping_add(d as u16);
                self.wz = self.pc;
                12
            }
            0x20 | 0x28 | 0x30 | 0x38 => {
                let d = self.fetch(bus) as i8;
                if self.cond(y - 4) {
                    self.pc = self.pc.wrapping_add(d as u16);
                    self.wz = self.pc;
                    12
                } else {
                    7
                }
            }
            0x22 => {
                let a = self.fetch16(bus);
                Self::write16(bus, a, self.hx());
                self.wz = a.wrapping_add(1);
                16
            }
            0x2A => {
                let a = self.fetch16(bus);
                let v = Self::read16(bus, a);
                self.set_hx(v);
                self.wz = a.wrapping_add(1);
                16
            }
            0x32 => {
                let a = self.fetch16(bus);
                bus.write(a, self.a);
                self.wz = (self.a as u16) << 8 | (a.wrapping_add(1) & 0xFF);
                13
            }
            0x3A => {
                let a = self.fetch16(bus);
                self.a = bus.read(a);
                self.wz = a.wrapping_add(1);
                13
            }
            0x27 => {
                self.daa();
                4
            }
            0x2F => {
                self.a = !self.a;
                self.f = (self.f & (SF | ZF | PF | CF)) | HF | NF | (self.a & (YF | XF));
                4
            }
            0x37 => {
                self.f = (self.f & (SF | ZF | PF)) | (self.a & (YF | XF)) | CF;
                4
            }
            0x3F => {
                let c = self.f & CF;
                self.f = (self.f & (SF | ZF | PF)) | (self.a & (YF | XF)) | if c != 0 { HF } else { CF };
                4
            }
            0x76 => {
                self.halted = true;
                4
            }
            0x40..=0x7F => {
                if y == 6 {
                    let (a, x) = self.addr_m(bus);
                    bus.write(a, self.r8_plain(z));
                    7 + x
                } else if z == 6 {
                    let (a, x) = self.addr_m(bus);
                    let v = bus.read(a);
                    self.set_r8_plain(y, v);
                    7 + x
                } else {
                    let v = self.r8(z);
                    self.set_r8(y, v);
                    4
                }
            }
            0x80..=0xBF => {
                if z == 6 {
                    let (a, x) = self.addr_m(bus);
                    let v = bus.read(a);
                    self.alu(y, v);
                    7 + x
                } else {
                    let v = self.r8(z);
                    self.alu(y, v);
                    4
                }
            }
            _ if op & 0xC7 == 0xC0 => {
                if self.cond(y) {
                    self.pc = self.pop(bus);
                    self.wz = self.pc;
                    11
                } else {
                    5
                }
            }
            _ if op & 0xCF == 0xC1 => {
                let v = self.pop(bus);
                if p == 3 {
                    self.set_af(v);
                } else {
                    self.set_rp(p, v);
                }
                10
            }
            _ if op & 0xC7 == 0xC2 => {
                let a = self.fetch16(bus);
                self.wz = a;
                if self.cond(y) {
                    self.pc = a;
                }
                10
            }
            0xC3 => {
                let a = self.fetch16(bus);
                self.wz = a;
                self.pc = a;
                10
            }
            _ if op & 0xC7 == 0xC4 => {
                let a = self.fetch16(bus);
                self.wz = a;
                if self.cond(y) {
                    let pc = self.pc;
                    self.push(bus, pc);
                    self.pc = a;
                    17
                } else {
                    10
                }
            }
            _ if op & 0xCF == 0xC5 => {
                let v = if p == 3 { self.af() } else { self.rp(p) };
                self.push(bus, v);
                11
            }
            _ if op & 0xC7 == 0xC6 => {
                let n = self.fetch(bus);
                self.alu(y, n);
                7
            }
            _ if op & 0xC7 == 0xC7 => {
                let pc = self.pc;
                self.push(bus, pc);
                self.pc = (y as u16) * 8;
                self.wz = self.pc;
                11
            }
            0xC9 => {
                self.pc = self.pop(bus);
                self.wz = self.pc;
                10
            }
            0xCD => {
                let a = self.fetch16(bus);
                self.wz = a;
                let pc = self.pc;
                self.push(bus, pc);
                self.pc = a;
                17
            }
            0xD3 => {
                let n = self.fetch(bus);
                let port = (self.a as u16) << 8 | n as u16;
                bus.output(port, self.a, self.t);
                self.wz = (self.a as u16) << 8 | (n.wrapping_add(1) as u16);
                11
            }
            0xDB => {
                let n = self.fetch(bus);
                let port = (self.a as u16) << 8 | n as u16;
                self.wz = port.wrapping_add(1);
                self.a = bus.input(port, self.t);
                11
            }
            0xD9 => {
                let (bc, de, hl) = (self.bc(), self.de(), self.hl());
                self.set_bc(self.bc2);
                self.set_de(self.de2);
                self.set_hl(self.hl2);
                (self.bc2, self.de2, self.hl2) = (bc, de, hl);
                4
            }
            0xE3 => {
                let v = Self::read16(bus, self.sp);
                Self::write16(bus, self.sp, self.hx());
                self.set_hx(v);
                self.wz = v;
                19
            }
            0xE9 => {
                self.pc = self.hx();
                4
            }
            0xEB => {
                let de = self.de();
                self.set_de(self.hl());
                self.set_hl(de);
                4
            }
            0xF3 => {
                self.iff1 = false;
                self.iff2 = false;
                4
            }
            0xF9 => {
                self.sp = self.hx();
                6
            }
            0xFB => {
                self.iff1 = true;
                self.iff2 = true;
                self.ei_delay = true;
                4
            }
            // 0xCB, 0xDD, 0xED and 0xFD are taken by `step`
            _ => 4,
        }
    }

    fn exec_cb<B: Bus>(&mut self, bus: &mut B, op: u8) -> u32 {
        let y = (op >> 3) & 7;
        let z = op & 7;
        let hl = self.hl();
        let v = if z == 6 { bus.read(hl) } else { self.r8(z) };
        match op >> 6 {
            0 => {
                let r = self.rot(y, v);
                if z == 6 {
                    bus.write(hl, r);
                    15
                } else {
                    self.set_r8(z, r);
                    8
                }
            }
            1 => {
                if z == 6 {
                    self.bit(y, v, (self.wz >> 8) as u8);
                    12
                } else {
                    self.bit(y, v, v);
                    8
                }
            }
            k => {
                let r = if k == 2 { v & !(1 << y) } else { v | (1 << y) };
                if z == 6 {
                    bus.write(hl, r);
                    15
                } else {
                    self.set_r8(z, r);
                    8
                }
            }
        }
    }

    /// DDCB d op / FDCB d op: the operation on (IX+d); the rotates, RES and SET also copy
    /// the result into the register the low three bits name (unless 6).
    fn exec_xycb<B: Bus>(&mut self, bus: &mut B) -> u32 {
        let d = self.fetch(bus) as i8;
        let op = self.fetch(bus);
        let a = self.hx().wrapping_add(d as u16);
        self.wz = a;
        let y = (op >> 3) & 7;
        let z = op & 7;
        let v = bus.read(a);
        let r = match op >> 6 {
            0 => self.rot(y, v),
            1 => {
                self.bit(y, v, (a >> 8) as u8);
                return 16;
            }
            2 => v & !(1 << y),
            _ => v | (1 << y),
        };
        bus.write(a, r);
        if z != 6 {
            self.set_r8_plain(z, r);
        }
        19
    }

    fn exec_ed<B: Bus>(&mut self, bus: &mut B, op: u8) -> u32 {
        let y = (op >> 3) & 7;
        let p = (op >> 4) & 3;
        match op {
            0x40..=0x7F => match op & 7 {
                0 => {
                    let port = self.bc();
                    let v = bus.input(port, self.t);
                    self.wz = port.wrapping_add(1);
                    if y != 6 {
                        self.set_r8(y, v);
                    }
                    self.f = (self.f & CF) | SZ53P[v as usize];
                    12
                }
                1 => {
                    let port = self.bc();
                    let v = if y == 6 { 0 } else { self.r8(y) };
                    bus.output(port, v, self.t);
                    self.wz = port.wrapping_add(1);
                    12
                }
                2 => {
                    let v = self.rp(p);
                    if op & 8 == 0 {
                        self.sbc16(v);
                    } else {
                        self.adc16(v);
                    }
                    15
                }
                3 => {
                    let a = self.fetch16(bus);
                    if op & 8 == 0 {
                        Self::write16(bus, a, self.rp(p));
                    } else {
                        let v = Self::read16(bus, a);
                        self.set_rp(p, v);
                    }
                    self.wz = a.wrapping_add(1);
                    20
                }
                4 => {
                    let a = self.a;
                    self.a = 0;
                    self.sub8(a, false, true);
                    8
                }
                5 => {
                    self.pc = self.pop(bus);
                    self.wz = self.pc;
                    self.iff1 = self.iff2;
                    14
                }
                6 => {
                    self.im = match y & 3 {
                        0 | 1 => 0,
                        2 => 1,
                        _ => 2,
                    };
                    8
                }
                _ => match y {
                    0 => {
                        self.i = self.a;
                        9
                    }
                    1 => {
                        self.r = self.a;
                        9
                    }
                    2 | 3 => {
                        let v = if y == 2 { self.i } else { self.r };
                        self.a = v;
                        self.f = (self.f & CF) | SZ53[v as usize] | if self.iff2 { PF } else { 0 };
                        9
                    }
                    4 | 5 => {
                        let hl = self.hl();
                        let m = bus.read(hl);
                        let a = self.a;
                        let (na, nm) = if y == 4 {
                            // RRD
                            ((a & 0xF0) | (m & 0x0F), (a << 4) | (m >> 4))
                        } else {
                            // RLD
                            ((a & 0xF0) | (m >> 4), (m << 4) | (a & 0x0F))
                        };
                        bus.write(hl, nm);
                        self.a = na;
                        self.f = (self.f & CF) | SZ53P[na as usize];
                        self.wz = hl.wrapping_add(1);
                        18
                    }
                    _ => 8,
                },
            },
            0xA0..=0xA3 | 0xA8..=0xAB | 0xB0..=0xB3 | 0xB8..=0xBB => self.block(bus, op),
            _ => 8,
        }
    }

    /// LDI/CPI/INI/OUTI and their decrementing and repeating forms.
    fn block<B: Bus>(&mut self, bus: &mut B, op: u8) -> u32 {
        let dec = op & 8 != 0;
        let rep = op & 0x10 != 0;
        let step = |v: u16| if dec { v.wrapping_sub(1) } else { v.wrapping_add(1) };
        match op & 3 {
            0 => {
                let v = bus.read(self.hl());
                bus.write(self.de(), v);
                self.set_hl(step(self.hl()));
                self.set_de(step(self.de()));
                let bc = self.bc().wrapping_sub(1);
                self.set_bc(bc);
                let n = v.wrapping_add(self.a);
                self.f = (self.f & (SF | ZF | CF)) | (n & XF) | ((n & 0x02) << 4) | if bc != 0 { PF } else { 0 };
                if rep && bc != 0 {
                    self.pc = self.pc.wrapping_sub(2);
                    self.wz = self.pc.wrapping_add(1);
                    21
                } else {
                    16
                }
            }
            1 => {
                let v = bus.read(self.hl());
                let r = self.a.wrapping_sub(v);
                self.set_hl(step(self.hl()));
                self.wz = step(self.wz);
                let bc = self.bc().wrapping_sub(1);
                self.set_bc(bc);
                let h = (self.a ^ v ^ r) & HF;
                let n = r.wrapping_sub((h != 0) as u8);
                self.f = (self.f & CF)
                    | NF
                    | h
                    | (SZ53[r as usize] & (SF | ZF))
                    | (n & XF)
                    | ((n & 0x02) << 4)
                    | if bc != 0 { PF } else { 0 };
                if rep && bc != 0 && r != 0 {
                    self.pc = self.pc.wrapping_sub(2);
                    self.wz = self.pc.wrapping_add(1);
                    21
                } else {
                    16
                }
            }
            k => {
                let (v, kk) = if k == 2 {
                    // INI/IND
                    let port = self.bc();
                    let v = bus.input(port, self.t);
                    self.wz = step(port);
                    bus.write(self.hl(), v);
                    self.b = self.b.wrapping_sub(1);
                    self.set_hl(step(self.hl()));
                    let c = if dec { self.c.wrapping_sub(1) } else { self.c.wrapping_add(1) };
                    (v, v as u16 + c as u16)
                } else {
                    // OUTI/OUTD: B is decremented before the port is put on the bus
                    let v = bus.read(self.hl());
                    self.b = self.b.wrapping_sub(1);
                    let port = self.bc();
                    self.wz = step(port);
                    bus.output(port, v, self.t);
                    self.set_hl(step(self.hl()));
                    (v, v as u16 + self.l as u16)
                };
                let mut f = SZ53[self.b as usize];
                if v & 0x80 != 0 {
                    f |= NF;
                }
                if kk > 0xFF {
                    f |= HF | CF;
                }
                f |= SZ53P[((kk as u8 & 7) ^ self.b) as usize] & PF;
                self.f = f;
                if rep && self.b != 0 {
                    self.pc = self.pc.wrapping_sub(2);
                    21
                } else {
                    16
                }
            }
        }
    }
}
