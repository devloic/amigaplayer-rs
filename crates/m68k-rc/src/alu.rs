//! Flag-exact instruction semantics. These are shared by the interpreter and by generated code, and
//! are checked against Musashi (and the SingleStepTests suite) by the harness.
//! Values are passed as `u32`; only the low `sz` bits are significant on input.
use crate::cpu::Cpu;
use crate::types::{Cond, ShiftKind, Sz};

#[inline(always)]
pub fn set_nz(c: &mut Cpu, sz: Sz, r: u32) {
    c.n = r & sz.msb() != 0;
    c.z = r & sz.mask() == 0;
}

/// MOVE/TST/AND/OR/EOR/NOT/CLR flags: N Z from the result, V = C = 0, X unchanged.
#[inline(always)]
pub fn logic(c: &mut Cpu, sz: Sz, r: u32) -> u32 {
    let r = r & sz.mask();
    set_nz(c, sz, r);
    c.v = false;
    c.c = false;
    r
}

#[inline(always)]
pub fn add(c: &mut Cpu, sz: Sz, s: u32, d: u32) -> u32 {
    let m = sz.mask();
    let (s, d) = (s & m, d & m);
    let r = s.wrapping_add(d) & m;
    let msb = sz.msb();
    let carry = ((s & d) | (!r & (s | d))) & msb != 0;
    c.v = ((s ^ r) & (d ^ r)) & msb != 0;
    c.c = carry;
    c.x = carry;
    set_nz(c, sz, r);
    r
}

#[inline(always)]
pub fn sub(c: &mut Cpu, sz: Sz, s: u32, d: u32) -> u32 {
    let m = sz.mask();
    let (s, d) = (s & m, d & m);
    let r = d.wrapping_sub(s) & m;
    let msb = sz.msb();
    let borrow = ((s & !d) | (r & !d) | (s & r)) & msb != 0;
    c.v = ((s ^ d) & (r ^ d)) & msb != 0;
    c.c = borrow;
    c.x = borrow;
    set_nz(c, sz, r);
    r
}

/// CMP/CMPA/CMPM/CMPI: like SUB without X and without storing.
#[inline(always)]
pub fn cmp(c: &mut Cpu, sz: Sz, s: u32, d: u32) {
    let m = sz.mask();
    let (s, d) = (s & m, d & m);
    let r = d.wrapping_sub(s) & m;
    let msb = sz.msb();
    c.c = ((s & !d) | (r & !d) | (s & r)) & msb != 0;
    c.v = ((s ^ d) & (r ^ d)) & msb != 0;
    set_nz(c, sz, r);
}

/// ADDX: Z is only cleared (never set), so multi-precision sums test zero over all parts.
#[inline(always)]
pub fn addx(c: &mut Cpu, sz: Sz, s: u32, d: u32) -> u32 {
    let m = sz.mask();
    let (s, d) = (s & m, d & m);
    let r = s.wrapping_add(d).wrapping_add(c.x as u32) & m;
    let msb = sz.msb();
    let carry = ((s & d) | (!r & (s | d))) & msb != 0;
    c.v = ((s ^ r) & (d ^ r)) & msb != 0;
    c.c = carry;
    c.x = carry;
    c.n = r & msb != 0;
    if r != 0 {
        c.z = false;
    }
    r
}

#[inline(always)]
pub fn subx(c: &mut Cpu, sz: Sz, s: u32, d: u32) -> u32 {
    let m = sz.mask();
    let (s, d) = (s & m, d & m);
    let r = d.wrapping_sub(s).wrapping_sub(c.x as u32) & m;
    let msb = sz.msb();
    let borrow = ((s & !d) | (r & !d) | (s & r)) & msb != 0;
    c.v = ((s ^ d) & (r ^ d)) & msb != 0;
    c.c = borrow;
    c.x = borrow;
    c.n = r & msb != 0;
    if r != 0 {
        c.z = false;
    }
    r
}

#[inline(always)]
pub fn neg(c: &mut Cpu, sz: Sz, d: u32) -> u32 {
    sub(c, sz, d, 0)
}

#[inline(always)]
pub fn negx(c: &mut Cpu, sz: Sz, d: u32) -> u32 {
    subx(c, sz, d, 0)
}

/// ABCD with Musashi's (documented-as-undefined) N and V behaviour.
#[inline(always)]
pub fn abcd(c: &mut Cpu, s: u32, d: u32) -> u32 {
    let mut r = (s & 0x0F) + (d & 0x0F) + c.x as u32;
    let v_pre = !r;
    if r > 9 {
        r += 6;
    }
    r += (s & 0xF0) + (d & 0xF0);
    let carry = r > 0x99;
    if carry {
        r -= 0xA0;
    }
    c.x = carry;
    c.c = carry;
    c.v = (v_pre & r) & 0x80 != 0;
    c.n = r & 0x80 != 0;
    let r = r & 0xFF;
    if r != 0 {
        c.z = false;
    }
    r
}

#[inline(always)]
pub fn sbcd(c: &mut Cpu, s: u32, d: u32) -> u32 {
    let mut r = (d & 0x0F).wrapping_sub(s & 0x0F).wrapping_sub(c.x as u32);
    let v_pre = !r;
    if r > 9 {
        r = r.wrapping_sub(6);
    }
    r = r.wrapping_add(d & 0xF0).wrapping_sub(s & 0xF0);
    let carry = r > 0x99;
    if carry {
        r = r.wrapping_add(0xA0);
    }
    c.x = carry;
    c.c = carry;
    let r = r & 0xFF;
    c.v = (v_pre & r) & 0x80 != 0;
    c.n = r & 0x80 != 0;
    if r != 0 {
        c.z = false;
    }
    r
}

/// NBCD. Returns `None` when the destination is left unwritten (result $9A case, as in Musashi).
#[inline(always)]
pub fn nbcd(c: &mut Cpu, d: u32) -> Option<u32> {
    let mut r = 0x9Au32.wrapping_sub(d).wrapping_sub(c.x as u32) & 0xFF;
    let out;
    if r != 0x9A {
        let v_pre = !r;
        if r & 0x0F == 0x0A {
            r = (r & 0xF0) + 0x10;
        }
        r &= 0xFF;
        c.v = (v_pre & r) & 0x80 != 0;
        if r != 0 {
            c.z = false;
        }
        c.c = true;
        c.x = true;
        out = Some(r);
    } else {
        c.v = false;
        c.c = false;
        c.x = false;
        out = None;
    }
    c.n = r & 0x80 != 0;
    out
}

// The 68020 BCD instructions, as the `m68k` crate's 68020 does them (its hardware model, checked
// against real CPUs by WinUAE's cputest): the digit corrections differ from Musashi's for invalid
// digits; N comes from the result, V is cleared, Z is cleared by a nonzero result only.

#[inline(always)]
fn bcd_flags_020(c: &mut Cpu, carry: bool, r: u32) -> u32 {
    c.x = carry;
    c.c = carry;
    if r != 0 {
        c.z = false;
    }
    c.n = r & 0x80 != 0;
    c.v = false;
    r
}

#[inline(always)]
pub fn abcd_020(c: &mut Cpu, s: u32, d: u32) -> u32 {
    let (s, d, x) = (s & 0xFF, d & 0xFF, c.x as u32);
    let lo = (s & 0x0F) + (d & 0x0F) + x;
    let mut r = (s & 0xF0) + (d & 0xF0) + lo;
    if lo > 9 {
        r += 6;
    }
    let carry = r & 0x3F0 > 0x90;
    if carry {
        r += 0x60;
    }
    bcd_flags_020(c, carry, r & 0xFF)
}

#[inline(always)]
pub fn sbcd_020(c: &mut Cpu, s: u32, d: u32) -> u32 {
    let (s, d, x) = (s & 0xFF, d & 0xFF, c.x as u32);
    let lo = (d & 0x0F).wrapping_sub(s & 0x0F).wrapping_sub(x) & 0xFFFF;
    let mut r = (d & 0xF0).wrapping_sub(s & 0xF0).wrapping_add(lo);
    let mut bcd = 0;
    if lo & 0xF0 != 0 {
        r = r.wrapping_sub(6);
        bcd = 6;
    }
    if d.wrapping_sub(s).wrapping_sub(x) & 0x100 != 0 {
        r = r.wrapping_sub(0x60);
    }
    let carry = d.wrapping_sub(s).wrapping_sub(bcd).wrapping_sub(x) & 0x300 != 0;
    bcd_flags_020(c, carry, r & 0xFF)
}

/// NBCD (68020): always writes its result.
#[inline(always)]
pub fn nbcd_020(c: &mut Cpu, d: u32) -> u32 {
    let (d, x) = (d & 0xFF, c.x as u32);
    let lo = 0u32.wrapping_sub(d & 0x0F).wrapping_sub(x) & 0xFFFF;
    let hi = 0u32.wrapping_sub(d & 0xF0) & 0xFFFF;
    let mut r = hi.wrapping_add(if lo > 9 { lo.wrapping_sub(6) } else { lo }) & 0xFFFF;
    let carry = r & 0x1F0 > 0x90;
    if carry {
        r = r.wrapping_sub(0x60);
    }
    bcd_flags_020(c, carry, r & 0xFF)
}

/// All shifts and rotates. `cnt` is the raw count (register counts are already taken mod 64).
#[inline(always)]
pub fn shift(c: &mut Cpu, kind: ShiftKind, left: bool, sz: Sz, v: u32, cnt: u32) -> u32 {
    let bits = sz.bits();
    let m = sz.mask();
    let msb = sz.msb();
    let v = v & m;
    match (kind, left) {
        (ShiftKind::As, true) => {
            if cnt == 0 {
                c.c = false;
                c.v = false;
                set_nz(c, sz, v);
                return v;
            }
            let (r, carry, ovf) = if cnt < bits {
                let r = (v << cnt) & m;
                let carry = (v >> (bits - cnt)) & 1 != 0;
                // top cnt+1 bits of the source must be all equal for V = 0
                let top = (m << (bits - cnt - 1)) & m;
                let t = v & top;
                (r, carry, !(t == 0 || t == top))
            } else {
                (0, cnt == bits && v & 1 != 0, v != 0)
            };
            c.x = carry;
            c.c = carry;
            c.v = ovf;
            set_nz(c, sz, r);
            r
        }
        (ShiftKind::As, false) => {
            if cnt == 0 {
                c.c = false;
                c.v = false;
                set_nz(c, sz, v);
                return v;
            }
            let sv = sz.sext(v) as i32;
            let (r, carry) = if cnt < bits {
                ((sv >> cnt) as u32 & m, (v >> (cnt - 1)) & 1 != 0)
            } else {
                let neg = v & msb != 0;
                (if neg { m } else { 0 }, neg)
            };
            c.x = carry;
            c.c = carry;
            c.v = false;
            set_nz(c, sz, r);
            r
        }
        (ShiftKind::Ls, true) => {
            if cnt == 0 {
                c.c = false;
                c.v = false;
                set_nz(c, sz, v);
                return v;
            }
            let (r, carry) = if cnt <= bits {
                (((v as u64) << cnt) as u32 & m, (v >> (bits - cnt)) & 1 != 0)
            } else {
                (0, false)
            };
            c.x = carry;
            c.c = carry;
            c.v = false;
            set_nz(c, sz, r);
            r
        }
        (ShiftKind::Ls, false) => {
            if cnt == 0 {
                c.c = false;
                c.v = false;
                set_nz(c, sz, v);
                return v;
            }
            let (r, carry) = if cnt <= bits {
                (((v as u64) >> cnt) as u32 & m, (v >> (cnt - 1)) & 1 != 0)
            } else {
                (0, false)
            };
            c.x = carry;
            c.c = carry;
            c.v = false;
            set_nz(c, sz, r);
            r
        }
        (ShiftKind::Ro, l) => {
            c.v = false;
            if cnt == 0 {
                c.c = false;
                set_nz(c, sz, v);
                return v;
            }
            let k = cnt % bits;
            let r = if k == 0 {
                v
            } else if l {
                ((v << k) | (v >> (bits - k))) & m
            } else {
                ((v >> k) | (v << (bits - k))) & m
            };
            c.c = if l { r & 1 != 0 } else { r & msb != 0 };
            set_nz(c, sz, r);
            r
        }
        (ShiftKind::Rox, l) => {
            c.v = false;
            if cnt == 0 {
                c.c = c.x;
                set_nz(c, sz, v);
                return v;
            }
            let k = (cnt % (bits + 1)) as u64;
            let w = bits as u64 + 1;
            let full: u64 = ((c.x as u64) << bits) | v as u64;
            let wm: u64 = (1u64 << w) - 1;
            let rot = if k == 0 {
                full
            } else if l {
                ((full << k) | (full >> (w - k))) & wm
            } else {
                ((full >> k) | (full << (w - k))) & wm
            };
            let x = (rot >> bits) & 1 != 0;
            let r = rot as u32 & m;
            c.x = x;
            c.c = x;
            set_nz(c, sz, r);
            r
        }
    }
}

#[inline(always)]
pub fn mulu(c: &mut Cpu, s: u32, d: u32) -> u32 {
    let r = (s & 0xFFFF) * (d & 0xFFFF);
    logic(c, Sz::L, r)
}

#[inline(always)]
pub fn muls(c: &mut Cpu, s: u32, d: u32) -> u32 {
    let r = (s as u16 as i16 as i32).wrapping_mul(d as u16 as i16 as i32) as u32;
    logic(c, Sz::L, r)
}

pub enum Div {
    /// New value of the destination data register.
    Ok(u32),
    /// Overflow: register unchanged, V set.
    Overflow,
    /// Division by zero: the caller raises the exception.
    Zero,
}

#[inline(always)]
pub fn divu(c: &mut Cpu, s: u32, d: u32) -> Div {
    let s = s & 0xFFFF;
    if s == 0 {
        // Musashi leaves the flags as they are (the real 68000 clears C; see README).
        return Div::Zero;
    }
    let q = d / s;
    let r = d % s;
    if q < 0x10000 {
        c.z = q == 0;
        c.n = q & 0x8000 != 0;
        c.v = false;
        c.c = false;
        Div::Ok((q & 0xFFFF) | (r << 16))
    } else {
        c.v = true;
        Div::Overflow
    }
}

#[inline(always)]
pub fn divs(c: &mut Cpu, s: u32, d: u32) -> Div {
    let s = s as u16 as i16 as i32;
    if s == 0 {
        return Div::Zero;
    }
    if d == 0x8000_0000 && s == -1 {
        // Musashi's special case.
        c.z = true;
        c.n = false;
        c.v = false;
        c.c = false;
        return Div::Ok(0);
    }
    let dd = d as i32;
    let q = dd / s;
    let r = dd % s;
    if q == q as i16 as i32 {
        c.z = q as i16 == 0;
        c.n = q & 0x8000 != 0;
        c.v = false;
        c.c = false;
        Div::Ok((q as u32 & 0xFFFF) | ((r as u32) << 16))
    } else {
        c.v = true;
        Div::Overflow
    }
}

/// MULU.L/MULS.L (68020): the 64-bit product as `(high, low)`. With `wide` (`<ea>,Dh:Dl`) N and Z
/// come from all 64 bits and V is cleared; otherwise from the low long, with V set when the product
/// does not fit in it. C is cleared, X unchanged.
#[inline(always)]
pub fn mull(c: &mut Cpu, signed: bool, wide: bool, s: u32, d: u32) -> (u32, u32) {
    let p = if signed { (s as i32 as i64).wrapping_mul(d as i32 as i64) as u64 } else { s as u64 * d as u64 };
    let (hi, lo) = ((p >> 32) as u32, p as u32);
    if wide {
        c.n = hi & 0x8000_0000 != 0;
        c.z = p == 0;
        c.v = false;
    } else {
        c.n = lo & 0x8000_0000 != 0;
        c.z = lo == 0;
        c.v = if signed { hi != ((lo as i32) >> 31) as u32 } else { hi != 0 };
    }
    c.c = false;
    (hi, lo)
}

pub enum DivL {
    /// Quotient and remainder, to be written remainder first (the quotient wins if `Dr == Dq`).
    Ok(u32, u32),
    /// Overflow: registers unchanged, V set, C cleared, N and Z unchanged (the manual leaves them
    /// undefined; this is what the `m68k` crate's 68020 does).
    Overflow,
    /// Division by zero: flags unchanged; the caller raises the exception.
    Zero,
}

/// DIVU.L/DIVS.L (68020) of `hi:lo` (`hi: None`: the 32-bit dividend `lo`) by `s`. N and Z from the
/// quotient, V = C = 0, X unchanged.
#[inline(always)]
pub fn divl(c: &mut Cpu, signed: bool, s: u32, hi: Option<u32>, lo: u32) -> DivL {
    if s == 0 {
        return DivL::Zero;
    }
    let (q, r) = if signed {
        let n = match hi {
            Some(h) => ((h as u64) << 32 | lo as u64) as i64,
            None => lo as i32 as i64,
        };
        let s = s as i32 as i64;
        if n == i64::MIN && s == -1 {
            c.v = true;
            c.c = false;
            return DivL::Overflow;
        }
        let q = n / s;
        if q != q as i32 as i64 {
            c.v = true;
            c.c = false;
            return DivL::Overflow;
        }
        (q as u32, (n % s) as u32)
    } else {
        let n = (hi.unwrap_or(0) as u64) << 32 | lo as u64;
        let q = n / s as u64;
        if q > u32::MAX as u64 {
            c.v = true;
            c.c = false;
            return DivL::Overflow;
        }
        (q as u32, (n % s as u64) as u32)
    };
    c.n = q & 0x8000_0000 != 0;
    c.z = q == 0;
    c.v = false;
    c.c = false;
    DivL::Ok(q, r)
}

/// CHK flags before the bound test; returns true if the exception must be taken.
#[inline(always)]
pub fn chk(c: &mut Cpu, dn: u32, bound: u32) -> bool {
    let src = dn as u16 as i16;
    let bound = bound as u16 as i16;
    c.z = src == 0;
    c.v = false;
    c.c = false;
    if src >= 0 && src <= bound {
        return false;
    }
    c.n = src < 0;
    true
}

/// [`chk`] as the `m68k` crate's 68020 sets the flags the manual leaves undefined: N from the sign
/// of Dn, Z, V and C cleared, whether or not the exception is taken.
#[inline(always)]
pub fn chk_020(c: &mut Cpu, dn: u32, bound: u32) -> bool {
    let src = dn as u16 as i16;
    let bound = bound as u16 as i16;
    c.n = src < 0;
    c.z = false;
    c.v = false;
    c.c = false;
    !(src >= 0 && src <= bound)
}

#[inline(always)]
pub fn cond(c: &Cpu, cc: Cond) -> bool {
    match cc {
        Cond::T => true,
        Cond::F => false,
        Cond::Hi => !c.c && !c.z,
        Cond::Ls => c.c || c.z,
        Cond::Cc => !c.c,
        Cond::Cs => c.c,
        Cond::Ne => !c.z,
        Cond::Eq => c.z,
        Cond::Vc => !c.v,
        Cond::Vs => c.v,
        Cond::Pl => !c.n,
        Cond::Mi => c.n,
        Cond::Ge => c.n == c.v,
        Cond::Lt => c.n != c.v,
        Cond::Gt => c.n == c.v && !c.z,
        Cond::Le => c.z || c.n != c.v,
    }
}

/// Merges `v` (low `sz` bits) into data register value `old`.
#[inline(always)]
pub fn merge(sz: Sz, old: u32, v: u32) -> u32 {
    (old & !sz.mask()) | (v & sz.mask())
}
