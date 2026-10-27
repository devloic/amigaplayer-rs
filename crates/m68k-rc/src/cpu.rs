//! Programmer-visible 68000 state.
use crate::bus::Bus;
use crate::types::Model;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu {
    pub d: [u32; 8],
    /// A0-A7; A7 is the *active* stack pointer (SSP in supervisor mode, USP in user mode).
    pub a: [u32; 8],
    pub pc: u32,
    pub x: bool,
    pub n: bool,
    pub z: bool,
    pub v: bool,
    pub c: bool,
    /// Supervisor bit.
    pub s: bool,
    /// Trace bit (stored, not emulated).
    pub t: bool,
    pub imask: u8,
    /// The inactive stack pointer: USP while in supervisor mode, SSP while in user mode.
    pub other_sp: u32,
    /// Set by STOP.
    pub stopped: bool,
    /// Basic blocks / instructions executed, for budgets and statistics.
    pub ticks: u64,
    /// Instructions run by the fallback interpreter.
    pub interp_count: u64,
    /// Generated code and the dispatcher return to the caller once `ticks >= budget`.
    pub budget: u64,
    /// Clock cycles executed ([`crate::cycles`]): advanced by the interpreter, and by generated
    /// code compiled with cycle counts (`m68k-recomp --cycles`), which adds a block's sum on entry.
    pub cycles: u64,
    /// With cycle counts, generated code and the dispatcher also return once
    /// `cycles >= cycle_budget` (u64::MAX: no limit).
    pub cycle_budget: u64,
    /// The CPU the interpreter decodes for and whose exception frames it builds. Generated code
    /// does not read it: it was compiled for one model.
    pub model: Model,
    /// 68010+ control registers (MOVEC, [`Model::M68020`] only; all 0 on the 68000). The VBR
    /// relocates the vectors of the 68020 exception entries.
    pub vbr: u32,
    /// Source / destination function codes (3 bits, stored only).
    pub sfc: u32,
    pub dfc: u32,
    /// Cache control (the 68020's EI and FI bits; the clear strobes are not stored) and cache
    /// address registers, stored only: there is no cache.
    pub cacr: u32,
    pub caar: u32,
    /// The master stack pointer. The M bit is not modelled (it is always clear: the supervisor A7
    /// is the ISP), so the MSP is never the active stack and MOVEC only stores it.
    pub msp: u32,
}

impl Default for Cpu {
    fn default() -> Self {
        Cpu {
            d: [0; 8], a: [0; 8], pc: 0, x: false, n: false, z: false, v: false, c: false,
            s: false, t: false, imask: 0, other_sp: 0, stopped: false, ticks: 0, interp_count: 0,
            budget: u64::MAX, cycles: 0, cycle_budget: u64::MAX, model: Model::M68000,
            vbr: 0, sfc: 0, dfc: 0, cacr: 0, caar: 0, msp: 0,
        }
    }
}

pub const VEC_ILLEGAL: u32 = 4;
pub const VEC_ZERO_DIVIDE: u32 = 5;
pub const VEC_CHK: u32 = 6;
pub const VEC_TRAPV: u32 = 7;
pub const VEC_PRIVILEGE: u32 = 8;
pub const VEC_LINE_A: u32 = 10;
pub const VEC_LINE_F: u32 = 11;
pub const VEC_FORMAT: u32 = 14;
pub const VEC_TRAP0: u32 = 32;

impl Cpu {
    #[inline(always)]
    pub fn ccr(&self) -> u8 {
        (self.x as u8) << 4 | (self.n as u8) << 3 | (self.z as u8) << 2 | (self.v as u8) << 1 | self.c as u8
    }
    #[inline(always)]
    pub fn set_ccr(&mut self, v: u8) {
        self.x = v & 0x10 != 0;
        self.n = v & 8 != 0;
        self.z = v & 4 != 0;
        self.v = v & 2 != 0;
        self.c = v & 1 != 0;
    }
    #[inline(always)]
    pub fn sr(&self) -> u16 {
        (self.t as u16) << 15 | (self.s as u16) << 13 | ((self.imask as u16) & 7) << 8 | self.ccr() as u16
    }
    /// Writes SR (68000 mask $A71F), switching stacks if S changes.
    #[inline(always)]
    pub fn set_sr(&mut self, v: u16) {
        self.t = v & 0x8000 != 0;
        self.imask = ((v >> 8) & 7) as u8;
        self.set_s(v & 0x2000 != 0);
        self.set_ccr(v as u8);
    }
    #[inline(always)]
    pub fn set_s(&mut self, s: bool) {
        if s != self.s {
            std::mem::swap(&mut self.a[7], &mut self.other_sp);
            self.s = s;
        }
    }
    pub fn usp(&self) -> u32 {
        if self.s { self.other_sp } else { self.a[7] }
    }
    pub fn ssp(&self) -> u32 {
        if self.s { self.a[7] } else { self.other_sp }
    }
    #[inline(always)]
    pub fn push32<B: Bus>(&mut self, b: &mut B, v: u32) {
        self.a[7] = self.a[7].wrapping_sub(4);
        b.write32(self.a[7], v);
    }
    #[inline(always)]
    pub fn push16<B: Bus>(&mut self, b: &mut B, v: u16) {
        self.a[7] = self.a[7].wrapping_sub(2);
        b.write16(self.a[7], v);
    }
    #[inline(always)]
    pub fn pop32<B: Bus>(&mut self, b: &mut B) -> u32 {
        let v = b.read32(self.a[7]);
        self.a[7] = self.a[7].wrapping_add(4);
        v
    }
    #[inline(always)]
    pub fn pop16<B: Bus>(&mut self, b: &mut B) -> u16 {
        let v = b.read16(self.a[7]);
        self.a[7] = self.a[7].wrapping_add(2);
        v
    }
    /// Group 1/2 exception entry (68000 short frame: PC then SR), PC := vector. `pc` is the PC
    /// stacked (next instruction for traps, the faulting instruction for illegal/privilege).
    /// With [`Model::M68020`], [`Cpu::exception_020`].
    #[cold]
    pub fn exception<B: Bus>(&mut self, b: &mut B, vector: u32, pc: u32) {
        if self.model == Model::M68020 {
            return self.exception_020(b, vector, pc);
        }
        let sr = self.sr();
        self.t = false;
        self.set_s(true);
        self.push32(b, pc);
        self.push16(b, sr);
        self.pc = b.read32(vector * 4);
    }
    /// 68010+ exception entry with a format 0 frame (vector offset, PC, SR), through the VBR.
    #[cold]
    pub fn exception_020<B: Bus>(&mut self, b: &mut B, vector: u32, pc: u32) {
        let sr = self.sr();
        self.t = false;
        self.set_s(true);
        self.push16(b, (vector << 2) as u16);
        self.push32(b, pc);
        self.push16(b, sr);
        self.pc = b.read32(self.vbr.wrapping_add(vector * 4));
    }
    /// The exceptions an instruction raises after it completes (zero divide, CHK, TRAPV): `next` is
    /// stacked. On the 68020 ([`Cpu::trap_020`]) the frame is format 2 and also holds `ins`, the
    /// instruction's address.
    #[cold]
    pub fn trap<B: Bus>(&mut self, b: &mut B, vector: u32, next: u32, ins: u32) {
        if self.model == Model::M68020 {
            return self.trap_020(b, vector, next, ins);
        }
        self.exception(b, vector, next)
    }
    #[cold]
    pub fn trap_020<B: Bus>(&mut self, b: &mut B, vector: u32, next: u32, ins: u32) {
        let sr = self.sr();
        self.t = false;
        self.set_s(true);
        self.push32(b, ins);
        self.push16(b, 0x2000 | (vector << 2) as u16);
        self.push32(b, next);
        self.push16(b, sr);
        self.pc = b.read32(self.vbr.wrapping_add(vector * 4));
    }
    /// 68010+ RTE, once the privilege check passed: frames of format 0, 1 (throwaway: the SR is
    /// restored and the next frame read) and 2; any other format raises a format error at `pc`,
    /// the RTE's address. That includes the 68020 bus fault frames ($A, $B), which only a bus
    /// error creates (never here); the `m68k` crate's 68020 returns through them.
    pub fn rte_020<B: Bus>(&mut self, b: &mut B, pc: u32) {
        loop {
            let format = b.read16(self.a[7].wrapping_add(6)) >> 12;
            match format {
                0 | 2 => {
                    let sr = self.pop16(b);
                    self.pc = self.pop32(b);
                    self.a[7] = self.a[7].wrapping_add(if format == 0 { 2 } else { 6 });
                    self.set_sr(sr);
                    return;
                }
                1 => {
                    let sr = self.pop16(b);
                    self.a[7] = self.a[7].wrapping_add(6);
                    self.set_sr(sr);
                }
                _ => return self.exception_020(b, crate::cpu::VEC_FORMAT, pc),
            }
        }
    }
    /// Whether `code` names a 68020 control register (MOVEC): SFC, DFC, CACR, USP, VBR, CAAR,
    /// MSP, ISP. Any other code raises an illegal instruction exception.
    pub const fn movec_legal(code: u16) -> bool {
        matches!(code, 0x000 | 0x001 | 0x002 | 0x800 | 0x801 | 0x802 | 0x803 | 0x804)
    }
    /// MOVEC Rc,Rn once the privilege and code checks passed: the control register `code`.
    /// As the `m68k` crate's 68020 reads them; with M clear, the ISP is the supervisor A7.
    pub fn movec_read(&self, code: u16) -> u32 {
        match code {
            0x000 => self.sfc,
            0x001 => self.dfc,
            0x002 => self.cacr,
            0x800 => self.usp(),
            0x801 => self.vbr,
            0x802 => self.caar,
            0x803 => self.msp,
            0x804 => self.ssp(),
            _ => 0,
        }
    }
    /// MOVEC Rn,Rc once the privilege and code checks passed.
    pub fn movec_write(&mut self, code: u16, v: u32) {
        match code {
            0x000 => self.sfc = v & 7,
            0x001 => self.dfc = v & 7,
            // EI and FI; CEI and CI only strobe the (absent) cache.
            0x002 => self.cacr = v & 3,
            0x800 => {
                if self.s { self.other_sp = v } else { self.a[7] = v }
            }
            0x801 => self.vbr = v,
            0x802 => self.caar = v,
            0x803 => self.msp = v,
            0x804 => {
                if self.s { self.a[7] = v } else { self.other_sp = v }
            }
            _ => {}
        }
    }
    /// Index register value for (d8,An,Xn) modes.
    #[inline(always)]
    pub fn xn(&self, reg: u8, long: bool) -> u32 {
        let v = if reg < 8 { self.d[reg as usize] } else { self.a[(reg - 8) as usize] };
        if long { v } else { v as u16 as i16 as i32 as u32 }
    }
}
