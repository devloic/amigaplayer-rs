//! The 6502 interpreter of the Pokeynoise replayer: the 151 documented opcodes with the deviations
//! of the specification (section 3): a stack that does not wrap and ends the call, binary-only
//! arithmetic, a BRK that skips one byte, the odd zero flag after addition and subtraction, a PHP
//! that pushes the stored B bit, a zero-page pointer read that does not wrap, the OS exit and
//! set-vector addresses, and the two hooked hardware reads (RANDOM and VCOUNT).
//!
//! Written from the specification; the instruction semantics are the usual ones of the documented
//! 6502 set. Safety choice where the specification leaves the loop unbounded: a call that runs
//! more than `MAX_STEPS` instructions fails, so a module that waits for something that never
//! happens cannot hang the host.

/// Instructions one call may run before it is counted as failed.
const MAX_STEPS: u64 = 4_000_000;

/// The player's random generator (section 4.5).
#[derive(Clone)]
pub struct Rng {
    pub s1: u32,
    pub s2: u32,
}

impl Rng {
    pub fn new() -> Rng {
        Rng { s1: 0x3F3F_751F, s2: 0x1717_9834 }
    }

    /// One step; the JOY1DAT value is 0 in this host.
    pub fn step(&mut self) -> u32 {
        let mut r = self.s1.wrapping_add(self.s2);
        self.s1 = r;
        r = r.rotate_right(4);
        let lo = (r as u16).wrapping_sub((self.s2 >> 16) as u16);
        r = (r & 0xFFFF_0000) | lo as u32;
        self.s2 ^= r;
        self.s1 = self.s1.wrapping_add(0x5656_5311);
        r
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Imm,
    Zp,
    ZpX,
    ZpY,
    Abs,
    AbsX,
    AbsY,
    IndX,
    IndY,
}

pub struct Cpu {
    pub mem: Box<[u8; 65536]>,
    pub rng: Rng,
    a: u8,
    x: u8,
    y: u8,
    /// The stack pointer word (section 3.2): starts at $0200, points at the last pushed byte.
    sp: u16,
    pc: u16,
    c: bool,
    z: bool,
    n: bool,
    v: bool,
    d: bool,
    i: bool,
    b: bool,
    /// The zero flag as it was when the carry was last set by a listed instruction.
    zc: bool,
    /// Estimated colour clocks spent in the current call.
    pub cycles: u64,
    /// Instructions run in the current call.
    pub steps: u64,
    /// The colour clock at the start of the current call, and a function of the elapsed time that
    /// gives the VCOUNT value.
    pub clock0: u64,
}

/// Why a call ended.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum End {
    Ok,
    Fail,
}

/// The VCOUNT value (raster line / 2, lines 0..312) at colour clock `t`.
fn vcount(t: u64) -> u8 {
    (((t + VPHASE) / 227 % 313) / 2) as u8
}

/// Estimated colour clocks of an instruction: a base plus an addition for the addressing mode and
/// for some kinds. The specification gives only 30 to 40 and about 31 for a NOP; these values are
/// fitted to the start of the writes in the real reference runs (by instruction kind and
/// addressing mode) and to the one run that shows VCOUNT (a read, 40 NOPs and a second read).
const BASE: u64 = 33;
const NOP_COST: u64 = 29;

fn mode_cost(m: Mode) -> u64 {
    match m {
        Mode::Imm => 12,
        Mode::Zp => 23,
        Mode::ZpX | Mode::ZpY => 26,
        Mode::Abs => 33,
        Mode::AbsX | Mode::AbsY => 22,
        Mode::IndX | Mode::IndY => 27,
    }
}

/// Colour clocks added to the host clock for the beam position: the original's first tick falls
/// at another point of the frame than this host's (a constant fitted to the one reference run that
/// shows VCOUNT; the specification only estimates it).
const VPHASE: u64 = 7537;

impl Cpu {
    pub fn new() -> Cpu {
        Cpu {
            mem: vec![0u8; 65536].into_boxed_slice().try_into().expect("64 KB"),
            rng: Rng::new(),
            a: 0,
            x: 0,
            y: 0,
            sp: 0x200,
            pc: 0,
            c: false,
            z: false,
            n: false,
            v: false,
            d: false,
            i: false,
            b: false,
            zc: false,
            cycles: 0,
            steps: 0,
            clock0: 0,
        }
    }

    pub fn r8(&self, a: u16) -> u8 {
        self.mem[a as usize]
    }

    pub fn w8(&mut self, a: u16, v: u8) {
        self.mem[a as usize] = v;
    }

    /// A read of the checked forms (section 3.5).
    fn rd_hw(&mut self, a: u16) -> u8 {
        if (0xD000..=0xD7FF).contains(&a) {
            if a == 0xD20A {
                return ((self.rng.step() & 0xFFFF) >> 5) as u8;
            }
            if a == 0xD40B {
                return vcount(self.clock0 + self.cycles);
            }
        }
        self.mem[a as usize]
    }

    fn fetch(&mut self) -> u8 {
        let v = self.mem[self.pc as usize];
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch() as u16;
        lo | (self.fetch() as u16) << 8
    }

    fn pointer(&self, zp: u16) -> u16 {
        // the high byte comes from zp + 1 without wrapping inside the zero page
        self.mem[zp as usize] as u16 | (self.mem[zp as usize + 1] as u16) << 8
    }

    fn ea(&mut self, m: Mode) -> u16 {
        self.cycles += mode_cost(m);
        match m {
            Mode::Imm => {
                let a = self.pc;
                self.pc = self.pc.wrapping_add(1);
                a
            }
            Mode::Zp => self.fetch() as u16,
            Mode::ZpX => self.fetch().wrapping_add(self.x) as u16,
            Mode::ZpY => self.fetch().wrapping_add(self.y) as u16,
            Mode::Abs => self.fetch16(),
            Mode::AbsX => self.fetch16().wrapping_add(self.x as u16),
            Mode::AbsY => self.fetch16().wrapping_add(self.y as u16),
            Mode::IndX => {
                let z = self.fetch().wrapping_add(self.x) as u16;
                self.pointer(z)
            }
            Mode::IndY => {
                let z = self.fetch() as u16;
                self.pointer(z).wrapping_add(self.y as u16)
            }
        }
    }

    fn nz(&mut self, v: u8) {
        self.z = v == 0;
        self.n = v & 0x80 != 0;
    }

    /// Sets Z and N from a result and remembers Z (an instruction that sets the carry).
    fn nzc(&mut self, v: u8) {
        self.nz(v);
        self.zc = self.z;
    }

    fn adc(&mut self, m: u8) {
        let s = self.a as u16 + m as u16 + self.c as u16;
        let r = s as u8;
        self.v = (!(self.a ^ m) & (self.a ^ r)) & 0x80 != 0;
        self.c = s > 0xFF;
        self.a = r;
        self.n = r & 0x80 != 0;
        // zero: only a non-zero result clears it; a zero result keeps the remembered flag
        self.z = if r != 0 { false } else { self.zc };
        self.zc = self.z;
    }

    fn cmp(&mut self, r: u8, m: u8) {
        self.c = r >= m;
        self.nzc(r.wrapping_sub(m));
    }

    fn pbyte(&self) -> u8 {
        (self.n as u8) << 7
            | (self.v as u8) << 6
            | 0x20
            | (self.b as u8) << 4
            | (self.d as u8) << 3
            | (self.i as u8) << 2
            | (self.z as u8) << 1
            | self.c as u8
    }

    fn set_p(&mut self, p: u8) {
        self.n = p & 0x80 != 0;
        self.v = p & 0x40 != 0;
        self.b = p & 0x10 != 0;
        self.d = p & 0x08 != 0;
        self.i = p & 0x04 != 0;
        self.z = p & 0x02 != 0;
        self.c = p & 0x01 != 0;
        self.zc = self.z;
    }

    /// A push; false if the stack page is exhausted.
    fn push(&mut self, v: u8) -> bool {
        self.sp = self.sp.wrapping_sub(1);
        self.mem[self.sp as usize] = v;
        self.sp >= 0x100
    }

    /// A pull; None if the stack is already at its top.
    fn pull(&mut self) -> Option<u8> {
        if self.sp > 0x200 {
            return None;
        }
        let v = self.mem[self.sp as usize];
        self.sp += 1;
        Some(v)
    }

    fn branch(&mut self, cond: bool) {
        let o = self.fetch() as i8;
        if cond {
            self.pc = self.pc.wrapping_add(o as i16 as u16);
        }
    }

    /// The OS set-vector entry (section 3.4).
    fn set_vector(&mut self) {
        match self.a {
            6 => {
                self.mem[0x222] = self.y;
                self.mem[0x223] = self.x;
            }
            7 => {
                self.mem[0x224] = self.y;
                self.mem[0x225] = self.x;
            }
            _ => {}
        }
    }

    /// Return as RTS does: true if the call is over.
    fn do_rts(&mut self) -> bool {
        let lo = self.mem[self.sp as usize] as u16;
        let hi = self.mem[(self.sp as usize + 1) & 0xFFFF] as u16;
        self.sp = self.sp.wrapping_add(2);
        if self.sp > 0x200 {
            return true;
        }
        self.pc = (hi << 8 | lo).wrapping_add(1);
        false
    }

    /// Runs one call (section 3.1) from `addr` with the registers given; `clock0` is the colour
    /// clock at its start.
    pub fn call(&mut self, addr: u16, a: u8, x: u8, y: u8, clock0: u64) -> End {
        self.mem[0xD01E] = 0x01;
        self.mem[0xD40F] = 0x40;
        self.a = a;
        self.x = x;
        self.y = y;
        self.sp = 0x200;
        self.pc = addr;
        self.c = false;
        self.z = false;
        self.n = false;
        self.v = false;
        self.d = false;
        self.i = false;
        self.b = false;
        self.zc = false;
        self.cycles = 0;
        self.steps = 0;
        self.clock0 = clock0;
        loop {
            self.steps += 1;
            if self.steps > MAX_STEPS {
                return End::Fail;
            }
            match self.step() {
                Some(e) => return e,
                None => {}
            }
        }
    }

    /// One instruction; Some when the call ends.
    fn step(&mut self) -> Option<End> {
        let op = self.fetch();
        self.cycles += if op == 0xEA { NOP_COST } else { BASE };
        match op {
            // branches
            _ if op & 0x1F == 0x10 => self.cycles += 6,
            // stack pushes and pulls
            0x08 | 0x28 | 0x48 | 0x68 => self.cycles += 14,
            // JSR, RTI, RTS, JMP
            0x20 | 0x40 | 0x60 | 0x4C | 0x6C => self.cycles += 44,
            // stores to absolute addresses
            0x8D | 0x8E | 0x8C => self.cycles += 7,
            _ => {}
        }
        let cc = op & 3;
        let aaa = op >> 5;
        let bbb = (op >> 2) & 7;
        match cc {
            1 => {
                let mode = [Mode::IndX, Mode::Zp, Mode::Imm, Mode::Abs, Mode::IndY, Mode::ZpX, Mode::AbsY, Mode::AbsX][bbb as usize];
                if aaa == 4 {
                    if mode == Mode::Imm {
                        return Some(End::Fail);
                    }
                    let ea = self.ea(mode);
                    self.mem[ea as usize] = self.a;
                    return None;
                }
                let ea = self.ea(mode);
                // checked forms: LDA and CMP in the absolute and indirect modes
                let checked = matches!(mode, Mode::Abs | Mode::AbsX | Mode::AbsY | Mode::IndX | Mode::IndY);
                let m = if (aaa == 5 && checked) || (aaa == 6 && mode == Mode::Abs) { self.rd_hw(ea) } else { self.mem[ea as usize] };
                match aaa {
                    0 => {
                        self.a |= m;
                        let a = self.a;
                        self.nz(a);
                    }
                    1 => {
                        self.a &= m;
                        let a = self.a;
                        self.nz(a);
                    }
                    2 => {
                        self.a ^= m;
                        let a = self.a;
                        self.nz(a);
                    }
                    3 => self.adc(m),
                    5 => {
                        self.a = m;
                        self.nz(m);
                    }
                    6 => {
                        let a = self.a;
                        self.cmp(a, m);
                    }
                    _ => self.adc(!m),
                }
                None
            }
            2 => self.group2(op, aaa, bbb),
            0 => self.group0(op, aaa, bbb),
            _ => Some(End::Fail),
        }
    }

    fn group2(&mut self, op: u8, aaa: u8, bbb: u8) -> Option<End> {
        // implied operations of this group
        match op {
            0x8A => {
                self.a = self.x;
                let a = self.a;
                self.nz(a);
                return None;
            }
            0xAA => {
                self.x = self.a;
                let x = self.x;
                self.nz(x);
                return None;
            }
            0xCA => {
                self.x = self.x.wrapping_sub(1);
                let x = self.x;
                self.nz(x);
                return None;
            }
            0xEA => return None,
            0x9A => {
                self.sp = 0x101 + self.x as u16;
                return None;
            }
            0xBA => {
                self.x = self.sp.wrapping_sub(0x101) as u8;
                let x = self.x;
                self.nz(x);
                return None;
            }
            _ => {}
        }
        let ldx_stx = aaa == 4 || aaa == 5;
        let mode = match bbb {
            0 if op == 0xA2 => Mode::Imm,
            1 => Mode::Zp,
            2 if aaa <= 3 => return self.accumulator(aaa),
            3 => Mode::Abs,
            5 => {
                if ldx_stx {
                    Mode::ZpY
                } else {
                    Mode::ZpX
                }
            }
            7 if aaa == 5 => Mode::AbsY,
            7 if aaa != 4 => Mode::AbsX,
            _ => return Some(End::Fail),
        };
        let ea = self.ea(mode);
        match aaa {
            4 => {
                self.mem[ea as usize] = self.x;
            }
            5 => {
                // checked: absolute and absolute,Y
                let m = if matches!(mode, Mode::Abs | Mode::AbsY) { self.rd_hw(ea) } else { self.mem[ea as usize] };
                self.x = m;
                self.nz(m);
            }
            6 => {
                let r = self.mem[ea as usize].wrapping_sub(1);
                self.mem[ea as usize] = r;
                self.nz(r);
            }
            7 => {
                let r = self.mem[ea as usize].wrapping_add(1);
                self.mem[ea as usize] = r;
                self.nz(r);
            }
            _ => {
                let m = self.mem[ea as usize];
                let r = self.shift(aaa, m);
                self.mem[ea as usize] = r;
            }
        }
        None
    }

    fn accumulator(&mut self, aaa: u8) -> Option<End> {
        let a = self.a;
        self.a = self.shift(aaa, a);
        None
    }

    /// ASL, ROL, LSR, ROR (aaa 0..3) on a value; sets C, N, Z.
    fn shift(&mut self, aaa: u8, m: u8) -> u8 {
        let cin = self.c as u8;
        let (r, c) = match aaa {
            0 => (m << 1, m & 0x80 != 0),
            1 => (m << 1 | cin, m & 0x80 != 0),
            2 => (m >> 1, m & 1 != 0),
            _ => (m >> 1 | cin << 7, m & 1 != 0),
        };
        self.c = c;
        self.nzc(r);
        r
    }

    fn group0(&mut self, op: u8, aaa: u8, bbb: u8) -> Option<End> {
        match op {
            0x00 => {
                // BRK skips the byte after it
                self.pc = self.pc.wrapping_add(1);
            }
            0x08 => {
                let p = self.pbyte();
                if !self.push(p) {
                    return Some(End::Fail);
                }
            }
            0x10 => {
                let c = !self.n;
                self.branch(c)
            }
            0x30 => {
                let c = self.n;
                self.branch(c)
            }
            0x50 => {
                let c = !self.v;
                self.branch(c)
            }
            0x70 => {
                let c = self.v;
                self.branch(c)
            }
            0x90 => {
                let c = !self.c;
                self.branch(c)
            }
            0xB0 => {
                let c = self.c;
                self.branch(c)
            }
            0xD0 => {
                let c = !self.z;
                self.branch(c)
            }
            0xF0 => {
                let c = self.z;
                self.branch(c)
            }
            0x18 => self.c = false,
            0x38 => self.c = true,
            0x58 => self.i = false,
            0x78 => self.i = true,
            0xB8 => self.v = false,
            0xD8 => self.d = false,
            0xF8 => self.d = true,
            0x20 => {
                let t = self.fetch16();
                if t == 0xE45C {
                    self.set_vector();
                    return None;
                }
                let r = self.pc.wrapping_sub(1);
                self.sp = self.sp.wrapping_sub(2);
                self.mem[self.sp as usize] = r as u8;
                self.mem[self.sp as usize + 1] = (r >> 8) as u8;
                if self.sp < 0x100 {
                    return Some(End::Fail);
                }
                self.pc = t;
            }
            0x24 | 0x2C => {
                let ea = self.ea(if op == 0x24 { Mode::Zp } else { Mode::Abs });
                let m = self.mem[ea as usize];
                self.z = m & self.a == 0;
                self.n = m & 0x80 != 0;
                self.v = m & 0x40 != 0;
            }
            0x28 => match self.pull() {
                Some(p) => self.set_p(p),
                None => return Some(End::Fail),
            },
            0x40 => {
                let p = self.mem[self.sp as usize];
                self.sp = self.sp.wrapping_add(1);
                self.set_p(p);
                let lo = self.mem[self.sp as usize] as u16;
                let hi = self.mem[(self.sp as usize + 1) & 0xFFFF] as u16;
                self.sp = self.sp.wrapping_add(2);
                if self.sp > 0x200 {
                    return Some(End::Ok);
                }
                self.pc = hi << 8 | lo;
            }
            0x48 => {
                let a = self.a;
                if !self.push(a) {
                    return Some(End::Fail);
                }
            }
            0x4C => {
                let t = self.fetch16();
                if t == 0xE462 || t == 0xE45F {
                    return Some(End::Ok);
                }
                if t == 0xE45C {
                    self.set_vector();
                    if self.do_rts() {
                        return Some(End::Ok);
                    }
                    return None;
                }
                self.pc = t;
            }
            0x6C => {
                let p = self.fetch16();
                let lo = self.mem[p as usize] as u16;
                let hi = self.mem[(p & 0xFF00 | (p.wrapping_add(1) & 0xFF)) as usize] as u16;
                let t = hi << 8 | lo;
                if t == 0xE462 || t == 0xE45F {
                    return Some(End::Ok);
                }
                self.pc = t;
            }
            0x60 => {
                if self.do_rts() {
                    return Some(End::Ok);
                }
            }
            0x68 => match self.pull() {
                Some(v) => {
                    self.a = v;
                    self.nz(v);
                }
                None => return Some(End::Fail),
            },
            0x84 | 0x8C | 0x94 => {
                let ea = self.ea(match op {
                    0x84 => Mode::Zp,
                    0x8C => Mode::Abs,
                    _ => Mode::ZpX,
                });
                self.mem[ea as usize] = self.y;
            }
            0x88 => {
                self.y = self.y.wrapping_sub(1);
                let y = self.y;
                self.nz(y);
            }
            0x98 => {
                self.a = self.y;
                let a = self.a;
                self.nz(a);
            }
            0xA8 => {
                self.y = self.a;
                let y = self.y;
                self.nz(y);
            }
            0xC8 => {
                self.y = self.y.wrapping_add(1);
                let y = self.y;
                self.nz(y);
            }
            0xE8 => {
                self.x = self.x.wrapping_add(1);
                let x = self.x;
                self.nz(x);
            }
            0xA0 | 0xA4 | 0xAC | 0xB4 | 0xBC => {
                let mode = match op {
                    0xA0 => Mode::Imm,
                    0xA4 => Mode::Zp,
                    0xAC => Mode::Abs,
                    0xB4 => Mode::ZpX,
                    _ => Mode::AbsX,
                };
                let ea = self.ea(mode);
                let m = if matches!(mode, Mode::Abs | Mode::AbsX) { self.rd_hw(ea) } else { self.mem[ea as usize] };
                self.y = m;
                self.nz(m);
            }
            0xC0 | 0xC4 | 0xCC | 0xE0 | 0xE4 | 0xEC => {
                let mode = match op & 0x0C {
                    0x00 => Mode::Imm,
                    0x04 => Mode::Zp,
                    _ => Mode::Abs,
                };
                let ea = self.ea(mode);
                let m = if mode == Mode::Abs { self.rd_hw(ea) } else { self.mem[ea as usize] };
                let r = if op < 0xE0 { self.y } else { self.x };
                self.cmp(r, m);
            }
            _ => {
                let _ = (aaa, bbb);
                return Some(End::Fail);
            }
        }
        None
    }
}
