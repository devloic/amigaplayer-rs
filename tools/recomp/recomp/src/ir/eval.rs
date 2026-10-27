//! A direct evaluator of the micro-ops: the reference semantics of a lifted instruction, used to
//! test the lifter against `m68k::interp::exec`. Lazy flags are evaluated as the generated code
//! computes them ([`IrOp::flag_mode`]): exact, guarded by `cpu.ticks >= cpu.budget`, or plain.
use super::{plain_shift_count, AddrExpr, ArOp, Body, Dst, FlagMode, IrInsn, IrOp, Term, UnOp, XOp, V};
use m68k::{alu, Bus, Cpu, ShiftKind, Sz, Xn};
use std::collections::HashMap;

fn xn(c: &Cpu, x: Xn) -> u32 {
    let r = if x.reg < 8 { c.d[x.reg as usize] } else { c.a[x.reg as usize - 8] };
    if x.long { r } else { r as i16 as i32 as u32 }
}

fn addr(c: &Cpu, ea: AddrExpr) -> u32 {
    match ea {
        AddrExpr::Disp(r, d) => c.a[r as usize].wrapping_add(d as u32),
        AddrExpr::Idx(r, d, x) => c.a[r as usize].wrapping_add(d as u32).wrapping_add(xn(c, x)),
        AddrExpr::Abs(a) => a,
        AddrExpr::PcIdx(b, x) => b.wrapping_add(xn(c, x)),
    }
}

/// The flag-free result of a shift with a plain form (see [`plain_shift_count`]).
pub fn plain_shift(kind: ShiftKind, left: bool, sz: Sz, v: u32, k: u32) -> u32 {
    let (m, bits) = (sz.mask(), sz.bits());
    let v = v & m;
    match (kind, left) {
        (ShiftKind::As | ShiftKind::Ls, true) => (v << k) & m,
        (ShiftKind::Ls, false) => v >> k,
        (ShiftKind::As, false) => ((sz.sext(v) as i32) >> k) as u32 & m,
        (ShiftKind::Ro, l) => {
            let k = k % bits;
            if k == 0 {
                v
            } else if l {
                ((v << k) | (v >> (bits - k))) & m
            } else {
                ((v >> k) | (v << (bits - k))) & m
            }
        }
        (ShiftKind::Rox, _) => unreachable!("ROX has no plain form"),
    }
}

fn store<B: Bus>(b: &mut B, sz: Sz, a: u32, x: u32) {
    match sz {
        Sz::B => b.write8(a, x as u8),
        Sz::W => b.write16(a, x as u16),
        Sz::L => b.write32(a, x),
    }
}

/// Runs a lifted instruction (sets `pc` to the next instruction first, as `exec`, then to
/// wherever its terminator or an exception goes). Always true (every instruction is lifted).
pub fn exec<B: Bus>(c: &mut Cpu, b: &mut B, ins: &IrInsn) -> bool {
    let Body::Lifted(ops) = &ins.body;
    c.pc = ins.instr.next();
    let mut t: HashMap<u32, u32> = HashMap::new();
    let val = |t: &HashMap<u32, u32>, v: V| match v {
        V::T(n) => t[&n],
        V::C(k) => k,
    };
    for op in ops {
        let exact = match op.flag_mode() {
            FlagMode::Exact => true,
            FlagMode::Guarded => c.ticks >= c.budget || c.cycles >= c.cycle_budget,
            FlagMode::Plain => false,
        };
        match *op {
            IrOp::Addr { t: n, ea } => {
                t.insert(n, addr(c, ea));
            }
            IrOp::PostInc { t: n, an, step } => {
                let a = c.a[an as usize];
                c.a[an as usize] = a.wrapping_add(step);
                t.insert(n, a);
            }
            IrOp::PreDec { t: n, an, step } => {
                let a = c.a[an as usize].wrapping_sub(step);
                c.a[an as usize] = a;
                t.insert(n, a);
            }
            IrOp::GetD { t: n, r, sz } => {
                t.insert(n, c.d[r as usize] & sz.mask());
            }
            IrOp::GetA { t: n, r, sz } => {
                t.insert(n, c.a[r as usize] & sz.mask());
            }
            IrOp::Load { t: n, sz, addr } => {
                let a = val(&t, addr);
                let v = match sz {
                    Sz::B => b.read8(a) as u32,
                    Sz::W => b.read16(a) as u32,
                    Sz::L => b.read32(a),
                };
                t.insert(n, v);
            }
            IrOp::SextW { t: n, v } => {
                let x = val(&t, v);
                t.insert(n, x as u16 as i16 as i32 as u32);
            }
            IrOp::SextB { t: n, v } => {
                let x = val(&t, v);
                t.insert(n, x as u8 as i8 as i32 as u32);
            }
            IrOp::Bin { t: n, op, a, b: y } => {
                let r = op.apply(val(&t, a), val(&t, y));
                t.insert(n, r);
            }
            IrOp::Arith { t: n, op, sz, s, d, .. } => {
                let (s, d) = (val(&t, s), val(&t, d));
                let r = if exact {
                    match op {
                        ArOp::Add => alu::add(c, sz, s, d),
                        ArOp::Sub => alu::sub(c, sz, s, d),
                        ArOp::Cmp => {
                            alu::cmp(c, sz, s, d);
                            0
                        }
                        ArOp::And => alu::logic(c, sz, d & s),
                        ArOp::Or => alu::logic(c, sz, d | s),
                        ArOp::Eor => alu::logic(c, sz, d ^ s),
                    }
                } else {
                    match op {
                        ArOp::Add => d.wrapping_add(s),
                        ArOp::Sub => d.wrapping_sub(s),
                        ArOp::Cmp => 0,
                        ArOp::And => d & s,
                        ArOp::Or => d | s,
                        ArOp::Eor => d ^ s,
                    }
                };
                if let Some(n) = n {
                    t.insert(n, r);
                }
            }
            IrOp::LogicFlags { sz, v, .. } => {
                if exact {
                    let x = val(&t, v);
                    alu::logic(c, sz, x);
                }
            }
            IrOp::Unary { t: n, op, sz, v, .. } => {
                let x = val(&t, v);
                let r = match (op, exact) {
                    (UnOp::Neg, true) => alu::neg(c, sz, x),
                    (UnOp::Not, true) => alu::logic(c, sz, !x),
                    (UnOp::Neg, false) => 0u32.wrapping_sub(x),
                    (UnOp::Not, false) => !x,
                };
                t.insert(n, r);
            }
            IrOp::Extend { t: n, op, sz, s, d } => {
                let (s, d) = (val(&t, s), val(&t, d));
                let r = match op {
                    XOp::Addx => alu::addx(c, sz, s, d),
                    XOp::Subx => alu::subx(c, sz, s, d),
                    XOp::Negx => alu::negx(c, sz, s),
                    XOp::Abcd => alu::abcd(c, s, d),
                    XOp::Sbcd => alu::sbcd(c, s, d),
                };
                t.insert(n, r);
            }
            IrOp::Nbcd { v, dst } => {
                let x = val(&t, v);
                if let Some(r) = alu::nbcd(c, x) {
                    match dst {
                        Dst::D(rn) => c.d[rn as usize] = alu::merge(Sz::B, c.d[rn as usize], r),
                        Dst::M(a) => b.write8(val(&t, a), r as u8),
                    }
                }
            }
            IrOp::Shift { t: n, kind, left, sz, v, n: cnt, .. } => {
                let (x, k) = (val(&t, v), val(&t, cnt));
                let r = if exact {
                    alu::shift(c, kind, left, sz, x, k)
                } else {
                    plain_shift(kind, left, sz, x, plain_shift_count(kind, cnt).unwrap())
                };
                t.insert(n, r);
            }
            IrOp::Mul { t: n, signed, s, d, .. } => {
                let (s, d) = (val(&t, s), val(&t, d));
                let r = match (signed, exact) {
                    (false, true) => alu::mulu(c, s, d),
                    (true, true) => alu::muls(c, s, d),
                    (false, false) => (s & 0xFFFF) * (d & 0xFFFF),
                    (true, false) => (s as u16 as i16 as i32).wrapping_mul(d as u16 as i16 as i32) as u32,
                };
                t.insert(n, r);
            }
            IrOp::Div { signed, s, d, dn, pc } => {
                let (s, d) = (val(&t, s), val(&t, d));
                match if signed { alu::divs(c, s, d) } else { alu::divu(c, s, d) } {
                    alu::Div::Ok(v) => c.d[dn as usize] = v,
                    alu::Div::Overflow => {}
                    alu::Div::Zero => {
                        c.exception(b, 5, pc);
                        return true;
                    }
                }
            }
            IrOp::Chk { d, bound, pc } => {
                let (d, bound) = (val(&t, d), val(&t, bound));
                if alu::chk(c, d, bound) {
                    c.exception(b, 6, pc);
                    return true;
                }
            }
            IrOp::BitZ { v, m, .. } => {
                if exact {
                    c.z = val(&t, v) & val(&t, m) == 0;
                }
            }
            IrOp::Scc { t: n, cond } => {
                t.insert(n, if alu::cond(c, cond) { 0xFF } else { 0 });
            }
            IrOp::GetCcr { t: n } => {
                t.insert(n, c.ccr() as u32);
            }
            IrOp::SetCcr { v } => {
                let x = val(&t, v);
                c.set_ccr(x as u8);
            }
            IrOp::GetSr { t: n } => {
                t.insert(n, c.sr() as u32);
            }
            IrOp::SetSr { v } => {
                let x = val(&t, v);
                c.set_sr(x as u16);
            }
            IrOp::GetOsp { t: n } => {
                t.insert(n, c.other_sp);
            }
            IrOp::SetOsp { v } => c.other_sp = val(&t, v),
            IrOp::Push { sz, v } => {
                let x = val(&t, v);
                if sz == Sz::L { c.push32(b, x) } else { c.push16(b, x as u16) }
            }
            IrOp::Pop { t: n, sz } => {
                let x = if sz == Sz::L { c.pop32(b) } else { c.pop16(b) as u32 };
                t.insert(n, x);
            }
            IrOp::SetD { r, sz, v } => {
                let (x, m) = (val(&t, v), sz.mask());
                c.d[r as usize] = (c.d[r as usize] & !m) | (x & m);
            }
            IrOp::SetA { r, v } => c.a[r as usize] = val(&t, v),
            IrOp::AddA { r, v, sub } => {
                let x = val(&t, v);
                let a = &mut c.a[r as usize];
                *a = if sub { a.wrapping_sub(x) } else { a.wrapping_add(x) };
            }
            IrOp::Store { sz, addr, v } => {
                let (a, x) = (val(&t, addr), val(&t, v));
                store(b, sz, a, x);
            }
            IrOp::StorePd32 { addr, v } => {
                let (a, x) = (val(&t, addr), val(&t, v));
                m68k::interp::write_pd32(b, a, x);
            }
            IrOp::Priv { pc } => {
                if !c.s {
                    c.exception(b, 8, pc);
                    return true;
                }
            }
            IrOp::TrapIf { cond, vector, pc } => {
                if alu::cond(c, cond) {
                    c.exception(b, vector, pc);
                    return true;
                }
            }
        }
    }
    match ins.term {
        None => {}
        Some(Term::Branch { cond, target }) => {
            if alu::cond(c, cond) {
                c.pc = target;
            }
        }
        Some(Term::Dbcc { cond, dn, target }) => {
            if !alu::cond(c, cond) {
                let w = (c.d[dn as usize] as u16).wrapping_sub(1);
                c.d[dn as usize] = (c.d[dn as usize] & 0xFFFF_0000) | w as u32;
                if w != 0xFFFF {
                    c.pc = target;
                }
            }
        }
        Some(Term::Goto { target, .. }) | Some(Term::Call { target }) => c.pc = target,
        Some(Term::DynCall { target, .. }) | Some(Term::DynJump { target }) => c.pc = val(&t, target),
        Some(Term::Return { pc }) => c.pc = val(&t, pc),
        Some(Term::Exception { vector, pc }) => c.exception(b, vector, pc),
        Some(Term::Stop) => c.stopped = true,
    }
    true
}
