//! A straightforward interpreter over [`Instr`]. It is the runtime fallback for code the static
//! recompiler did not translate (computed jumps to unknown places, code found only at run time).
use crate::alu::{self, Div, DivL};
use crate::bus::Bus;
use crate::cpu::*;
use crate::decode::decode_model;
use crate::types::*;

/// A resolved operand location.
#[derive(Clone, Copy, Debug)]
pub enum Loc {
    D(u8),
    A(u8),
    M(u32),
    I(u32),
}

#[inline(always)]
fn step_size(sz: Sz, r: u8) -> u32 {
    if sz == Sz::B && r == 7 { 2 } else { sz.bytes() }
}

/// Address of a control/memory EA without side effects on registers (for LEA/PEA/JMP/JSR/MOVEM).
/// The bus is read only by the memory-indirect 68020 modes.
#[inline(always)]
pub fn ea_addr<B: Bus>(c: &Cpu, b: &mut B, ea: &Ea) -> u32 {
    match *ea {
        Ea::Ind(r) => c.a[r as usize],
        Ea::Disp(r, d) => c.a[r as usize].wrapping_add(d as i32 as u32),
        Ea::Idx(r, d, x) => c.a[r as usize].wrapping_add(d as i32 as u32).wrapping_add(c.xn(x.reg, x.long)),
        Ea::AbsW(a) | Ea::AbsL(a) | Ea::PcDisp(a) => a,
        Ea::PcIdx(base, x) => base.wrapping_add(c.xn(x.reg, x.long)),
        Ea::Full(f) => full_addr(c, b, &f),
        _ => panic!("ea_addr on {:?}", ea),
    }
}

/// Address of a 68020 indexed EA ([`FullEa`]).
#[inline(always)]
pub fn full_addr<B: Bus>(c: &Cpu, b: &mut B, f: &FullEa) -> u32 {
    let base = f.base.map_or(0, |r| c.a[r as usize]).wrapping_add(f.bd);
    let index = f.index.map_or(0, |(x, scale)| c.xn(x.reg, x.long) << scale);
    match f.mem {
        MemInd::None => base.wrapping_add(index),
        MemInd::Pre(od) => b.read32(base.wrapping_add(index)).wrapping_add(od),
        MemInd::Post(od) => b.read32(base).wrapping_add(index).wrapping_add(od),
    }
}

/// Resolves an EA, applying (An)+ / -(An) side effects.
#[inline(always)]
pub fn resolve<B: Bus>(c: &mut Cpu, b: &mut B, ea: &Ea, sz: Sz) -> Loc {
    match *ea {
        Ea::D(r) => Loc::D(r),
        Ea::A(r) => Loc::A(r),
        Ea::Imm(v) => Loc::I(v),
        Ea::Post(r) => {
            let a = c.a[r as usize];
            c.a[r as usize] = a.wrapping_add(step_size(sz, r));
            Loc::M(a)
        }
        Ea::Pre(r) => {
            let a = c.a[r as usize].wrapping_sub(step_size(sz, r));
            c.a[r as usize] = a;
            Loc::M(a)
        }
        _ => Loc::M(ea_addr(c, b, ea)),
    }
}

#[inline(always)]
pub fn read<B: Bus>(c: &Cpu, b: &mut B, l: Loc, sz: Sz) -> u32 {
    match l {
        Loc::D(r) => c.d[r as usize] & sz.mask(),
        Loc::A(r) => c.a[r as usize] & sz.mask(),
        Loc::I(v) => v & sz.mask(),
        Loc::M(a) => match sz {
            Sz::B => b.read8(a) as u32,
            Sz::W => b.read16(a) as u32,
            Sz::L => b.read32(a),
        },
    }
}

#[inline(always)]
pub fn write<B: Bus>(c: &mut Cpu, b: &mut B, l: Loc, sz: Sz, v: u32) {
    match l {
        Loc::D(r) => c.d[r as usize] = alu::merge(sz, c.d[r as usize], v),
        Loc::A(r) => c.a[r as usize] = v, // only reached with full-width values
        Loc::I(_) => panic!("write to immediate"),
        Loc::M(a) => match sz {
            Sz::B => b.write8(a, v as u8),
            Sz::W => b.write16(a, v as u16),
            Sz::L => b.write32(a, v),
        },
    }
}

/// Fetches, decodes and executes one instruction at `c.pc`.
pub fn step<B: Bus>(c: &mut Cpu, b: &mut B) {
    let ins = decode_model(c.pc, &mut |a| b.fetch16(a), c.model);
    exec(c, b, &ins);
}

/// Executes a decoded instruction. On return `c.pc` is the next PC (or exception handler).
pub fn exec<B: Bus>(c: &mut Cpu, b: &mut B, ins: &Instr) {
    let next = ins.next();
    c.pc = next;
    match ins.op {
        Op::Alu { op, sz, src, dst } => {
            let sl = resolve(c, b, &src, sz);
            let s = read(c, b, sl, sz);
            let dl = resolve(c, b, &dst, sz);
            let d = read(c, b, dl, sz);
            let r = match op {
                AluOp::Or => alu::logic(c, sz, d | s),
                AluOp::And => alu::logic(c, sz, d & s),
                AluOp::Eor => alu::logic(c, sz, d ^ s),
                AluOp::Add => alu::add(c, sz, s, d),
                AluOp::Sub => alu::sub(c, sz, s, d),
                AluOp::Cmp => {
                    alu::cmp(c, sz, s, d);
                    return;
                }
            };
            write(c, b, dl, sz, r);
        }
        Op::AddA { sz, src, an } | Op::SubA { sz, src, an } | Op::CmpA { sz, src, an } => {
            let sl = resolve(c, b, &src, sz);
            let s = sz.sext(read(c, b, sl, sz));
            let a = c.a[an as usize];
            match ins.op {
                Op::AddA { .. } => c.a[an as usize] = a.wrapping_add(s),
                Op::SubA { .. } => c.a[an as usize] = a.wrapping_sub(s),
                _ => alu::cmp(c, Sz::L, s, a),
            }
        }
        Op::LogicSr { op, imm, sr } => {
            if sr {
                if !c.s {
                    c.exception(b, VEC_PRIVILEGE, ins.pc);
                    return;
                }
                let v = c.sr();
                let r = match op {
                    AluOp::Or => v | imm,
                    AluOp::And => v & imm,
                    _ => v ^ imm,
                };
                c.set_sr(r);
            } else {
                let v = c.ccr();
                let i = imm as u8;
                let r = match op {
                    AluOp::Or => v | i,
                    AluOp::And => v & i,
                    _ => v ^ i,
                };
                c.set_ccr(r);
            }
        }
        Op::Bit { op, num, dst } => {
            let n = match num {
                BitNum::Imm(n) => n as u32,
                BitNum::Reg(r) => c.d[r as usize],
            };
            if let Ea::D(r) = dst {
                let mask = 1u32 << (n & 31);
                let v = c.d[r as usize];
                c.z = v & mask == 0;
                match op {
                    BitOp::Tst => {}
                    BitOp::Chg => c.d[r as usize] = v ^ mask,
                    BitOp::Clr => c.d[r as usize] = v & !mask,
                    BitOp::Set => c.d[r as usize] = v | mask,
                }
            } else {
                let mask = 1u32 << (n & 7);
                let l = resolve(c, b, &dst, Sz::B);
                let v = read(c, b, l, Sz::B);
                c.z = v & mask == 0;
                match op {
                    BitOp::Tst => {}
                    BitOp::Chg => write(c, b, l, Sz::B, v ^ mask),
                    BitOp::Clr => write(c, b, l, Sz::B, v & !mask),
                    BitOp::Set => write(c, b, l, Sz::B, v | mask),
                }
            }
        }
        Op::Movep { sz, dn, an, disp, to_mem } => {
            let ea = c.a[an as usize].wrapping_add(disp as i32 as u32);
            let n = sz.bytes();
            if to_mem {
                let v = c.d[dn as usize];
                for i in 0..n {
                    b.write8(ea.wrapping_add(2 * i), (v >> (8 * (n - 1 - i))) as u8);
                }
            } else {
                let mut v = 0u32;
                for i in 0..n {
                    v = (v << 8) | b.read8(ea.wrapping_add(2 * i)) as u32;
                }
                c.d[dn as usize] = alu::merge(sz, c.d[dn as usize], v);
            }
        }
        Op::Move { sz, src, dst } => {
            let sl = resolve(c, b, &src, sz);
            let v = read(c, b, sl, sz);
            let dl = resolve(c, b, &dst, sz);
            alu::logic(c, sz, v);
            match (sz, dst, dl) {
                // MOVE.L to -(An) stores the low word first (68000 and Musashi).
                (Sz::L, Ea::Pre(_), Loc::M(a)) => write_pd32(b, a, v),
                _ => write(c, b, dl, sz, v),
            }
        }
        Op::MoveA { sz, src, an } => {
            let sl = resolve(c, b, &src, sz);
            c.a[an as usize] = sz.sext(read(c, b, sl, sz));
        }
        Op::Negx { sz, ea } | Op::Neg { sz, ea } | Op::Not { sz, ea } => {
            let l = resolve(c, b, &ea, sz);
            let v = read(c, b, l, sz);
            let r = match ins.op {
                Op::Negx { .. } => alu::negx(c, sz, v),
                Op::Neg { .. } => alu::neg(c, sz, v),
                _ => alu::logic(c, sz, !v),
            };
            write(c, b, l, sz, r);
        }
        Op::Clr { sz, ea } => {
            let l = resolve(c, b, &ea, sz);
            alu::logic(c, sz, 0);
            write(c, b, l, sz, 0);
        }
        Op::Tst { sz, ea } => {
            let l = resolve(c, b, &ea, sz);
            let v = read(c, b, l, sz);
            alu::logic(c, sz, v);
        }
        Op::MoveFromSr { ea } => {
            // Privileged from the 68010 on.
            if c.model == Model::M68020 && !c.s {
                c.exception(b, VEC_PRIVILEGE, ins.pc);
                return;
            }
            let l = resolve(c, b, &ea, Sz::W);
            let v = c.sr() as u32;
            write(c, b, l, Sz::W, v);
        }
        Op::MoveToCcr { ea } => {
            let l = resolve(c, b, &ea, Sz::W);
            let v = read(c, b, l, Sz::W);
            c.set_ccr(v as u8);
        }
        Op::MoveToSr { ea } => {
            if !c.s {
                c.exception(b, VEC_PRIVILEGE, ins.pc);
                return;
            }
            let l = resolve(c, b, &ea, Sz::W);
            let v = read(c, b, l, Sz::W);
            c.set_sr(v as u16);
        }
        Op::Chk { src, dn } => {
            let l = resolve(c, b, &src, Sz::W);
            let bound = read(c, b, l, Sz::W);
            let dn = c.d[dn as usize];
            let trap = if c.model == Model::M68020 { alu::chk_020(c, dn, bound) } else { alu::chk(c, dn, bound) };
            if trap {
                c.trap(b, VEC_CHK, next, ins.pc);
            }
        }
        Op::Lea { src, an } => c.a[an as usize] = ea_addr(c, b, &src),
        Op::Pea { src } => {
            let a = ea_addr(c, b, &src);
            c.push32(b, a);
        }
        Op::Nbcd { ea } => {
            let l = resolve(c, b, &ea, Sz::B);
            let v = read(c, b, l, Sz::B);
            if c.model == Model::M68020 {
                let r = alu::nbcd_020(c, v);
                write(c, b, l, Sz::B, r);
            } else if let Some(r) = alu::nbcd(c, v) {
                write(c, b, l, Sz::B, r);
            }
        }
        Op::Swap { dn } => {
            let v = c.d[dn as usize].rotate_left(16);
            c.d[dn as usize] = alu::logic(c, Sz::L, v);
        }
        Op::Ext { sz, dn } => {
            let v = c.d[dn as usize];
            if sz == Sz::W {
                let r = Sz::B.sext(v) & 0xFFFF;
                alu::logic(c, Sz::W, r);
                c.d[dn as usize] = alu::merge(Sz::W, v, r);
            } else {
                let r = Sz::W.sext(v);
                c.d[dn as usize] = alu::logic(c, Sz::L, r);
            }
        }
        Op::Movem { sz, to_mem, mask, ea } => movem(c, b, sz, to_mem, mask, &ea),
        Op::Tas { ea } => {
            let l = resolve(c, b, &ea, Sz::B);
            let v = read(c, b, l, Sz::B);
            alu::logic(c, Sz::B, v);
            write(c, b, l, Sz::B, v | 0x80);
        }
        Op::Illegal => c.exception(b, VEC_ILLEGAL, ins.pc),
        Op::LineA => c.exception(b, VEC_LINE_A, ins.pc),
        Op::LineF => c.exception(b, VEC_LINE_F, ins.pc),
        Op::Trap { n } => c.exception(b, VEC_TRAP0 + n as u32, next),
        Op::Link { an, disp } => {
            if an == 7 && c.model == Model::M68020 {
                // The 68020 pushes A7 as it was.
                let v = c.a[7];
                c.push32(b, v);
            } else if an == 7 {
                c.a[7] = c.a[7].wrapping_sub(4);
                b.write32(c.a[7], c.a[7]);
            } else {
                let v = c.a[an as usize];
                c.push32(b, v);
                c.a[an as usize] = c.a[7];
            }
            c.a[7] = c.a[7].wrapping_add(disp as i32 as u32);
        }
        Op::Unlk { an } => {
            if an == 7 {
                c.a[7] = b.read32(c.a[7]);
            } else {
                c.a[7] = c.a[an as usize];
                c.a[an as usize] = c.pop32(b);
            }
        }
        Op::MoveUsp { to_usp, an } => {
            if !c.s {
                c.exception(b, VEC_PRIVILEGE, ins.pc);
                return;
            }
            if to_usp {
                c.other_sp = c.a[an as usize];
            } else {
                c.a[an as usize] = c.other_sp;
            }
        }
        Op::Reset => {
            if !c.s {
                c.exception(b, VEC_PRIVILEGE, ins.pc);
            }
        }
        Op::Nop => {}
        Op::Stop { imm } => {
            if !c.s {
                c.exception(b, VEC_PRIVILEGE, ins.pc);
                return;
            }
            c.set_sr(imm);
            c.stopped = true;
        }
        Op::Rte => {
            if !c.s {
                c.exception(b, VEC_PRIVILEGE, ins.pc);
                return;
            }
            if c.model == Model::M68020 {
                c.rte_020(b, ins.pc);
                return;
            }
            let sr = c.pop16(b);
            let pc = c.pop32(b);
            c.pc = pc;
            c.set_sr(sr);
        }
        Op::Rts => c.pc = c.pop32(b),
        Op::Trapv => {
            if c.v {
                c.trap(b, VEC_TRAPV, next, ins.pc);
            }
        }
        Op::Rtr => {
            let ccr = c.pop16(b);
            c.set_ccr(ccr as u8);
            c.pc = c.pop32(b);
        }
        Op::Jsr { ea } => {
            let t = ea_addr(c, b, &ea);
            c.push32(b, next);
            c.pc = t;
        }
        Op::Jmp { ea } => c.pc = ea_addr(c, b, &ea),
        Op::Scc { cond, ea } => {
            let l = resolve(c, b, &ea, Sz::B);
            let v = if alu::cond(c, cond) { 0xFF } else { 0 };
            write(c, b, l, Sz::B, v);
        }
        Op::Dbcc { cond, dn, target } => {
            if !alu::cond(c, cond) {
                let w = (c.d[dn as usize] as u16).wrapping_sub(1);
                c.d[dn as usize] = alu::merge(Sz::W, c.d[dn as usize], w as u32);
                if w != 0xFFFF {
                    c.pc = target;
                }
            }
        }
        Op::Bra { target } => c.pc = target,
        Op::Bsr { target } => {
            c.push32(b, next);
            c.pc = target;
        }
        Op::Bcc { cond, target } => {
            if alu::cond(c, cond) {
                c.pc = target;
            }
        }
        Op::Moveq { data, dn } => c.d[dn as usize] = alu::logic(c, Sz::L, data as i32 as u32),
        Op::Divu { src, dn } | Op::Divs { src, dn } => {
            let l = resolve(c, b, &src, Sz::W);
            let s = read(c, b, l, Sz::W);
            let d = c.d[dn as usize];
            let r = if matches!(ins.op, Op::Divu { .. }) { alu::divu(c, s, d) } else { alu::divs(c, s, d) };
            match r {
                Div::Ok(v) => c.d[dn as usize] = v,
                Div::Overflow => {}
                Div::Zero => c.trap(b, VEC_ZERO_DIVIDE, next, ins.pc),
            }
        }
        Op::Mulu { src, dn } | Op::Muls { src, dn } => {
            let l = resolve(c, b, &src, Sz::W);
            let s = read(c, b, l, Sz::W);
            let d = c.d[dn as usize];
            c.d[dn as usize] = if matches!(ins.op, Op::Mulu { .. }) { alu::mulu(c, s, d) } else { alu::muls(c, s, d) };
        }
        Op::Abcd { rx, ry, mem } | Op::Sbcd { rx, ry, mem } => {
            let abcd = matches!(ins.op, Op::Abcd { .. });
            let (s, dl) = if mem {
                let sl = resolve(c, b, &Ea::Pre(ry), Sz::B);
                let s = read(c, b, sl, Sz::B);
                (s, resolve(c, b, &Ea::Pre(rx), Sz::B))
            } else {
                (c.d[ry as usize], Loc::D(rx))
            };
            let d = if mem { read(c, b, dl, Sz::B) } else { c.d[rx as usize] };
            let r = match (abcd, c.model) {
                (true, Model::M68000) => alu::abcd(c, s, d),
                (false, Model::M68000) => alu::sbcd(c, s, d),
                (true, Model::M68020) => alu::abcd_020(c, s, d),
                (false, Model::M68020) => alu::sbcd_020(c, s, d),
            };
            write(c, b, dl, Sz::B, r);
        }
        Op::Addx { sz, rx, ry, mem } | Op::Subx { sz, rx, ry, mem } => {
            let add = matches!(ins.op, Op::Addx { .. });
            let (s, dl) = if mem {
                let sl = resolve(c, b, &Ea::Pre(ry), sz);
                let s = read(c, b, sl, sz);
                (s, resolve(c, b, &Ea::Pre(rx), sz))
            } else {
                (c.d[ry as usize], Loc::D(rx))
            };
            let d = read(c, b, dl, sz);
            let r = if add { alu::addx(c, sz, s, d) } else { alu::subx(c, sz, s, d) };
            write(c, b, dl, sz, r);
        }
        Op::Cmpm { sz, ax, ay } => {
            let sl = resolve(c, b, &Ea::Post(ay), sz);
            let s = read(c, b, sl, sz);
            let dl = resolve(c, b, &Ea::Post(ax), sz);
            let d = read(c, b, dl, sz);
            alu::cmp(c, sz, s, d);
        }
        Op::Exg { rx, ry } => {
            let get = |c: &Cpu, r: u8| if r < 8 { c.d[r as usize] } else { c.a[r as usize - 8] };
            let (vx, vy) = (get(c, rx), get(c, ry));
            let mut set = |r: u8, v: u32| if r < 8 { c.d[r as usize] = v } else { c.a[r as usize - 8] = v };
            set(rx, vy);
            set(ry, vx);
        }
        Op::ShiftReg { kind, left, sz, count, dn } => {
            let cnt = match count {
                ShiftCount::Imm(n) => n as u32,
                ShiftCount::Reg(r) => c.d[r as usize] & 63,
            };
            let v = c.d[dn as usize];
            let r = alu::shift(c, kind, left, sz, v, cnt);
            c.d[dn as usize] = alu::merge(sz, v, r);
        }
        Op::ShiftMem { kind, left, ea } => {
            let l = resolve(c, b, &ea, Sz::W);
            let v = read(c, b, l, Sz::W);
            let r = alu::shift(c, kind, left, Sz::W, v, 1);
            write(c, b, l, Sz::W, r);
        }
        Op::MoveFromCcr { ea } => {
            let l = resolve(c, b, &ea, Sz::W);
            let v = c.ccr() as u32;
            write(c, b, l, Sz::W, v);
        }
        Op::Movec { to_ctrl, reg, ctrl } => {
            // The privilege check comes first: in user mode even an unknown code is a privilege
            // violation.
            if !c.s {
                c.exception(b, VEC_PRIVILEGE, ins.pc);
                return;
            }
            if !Cpu::movec_legal(ctrl) {
                c.exception(b, VEC_ILLEGAL, ins.pc);
                return;
            }
            if to_ctrl {
                let v = if reg < 8 { c.d[reg as usize] } else { c.a[(reg - 8) as usize] };
                c.movec_write(ctrl, v);
            } else {
                let v = c.movec_read(ctrl);
                if reg < 8 { c.d[reg as usize] = v } else { c.a[(reg - 8) as usize] = v }
            }
        }
        Op::Extb { dn } => {
            let v = Sz::B.sext(c.d[dn as usize]);
            c.d[dn as usize] = alu::logic(c, Sz::L, v);
        }
        Op::MulL { signed, src, dl, dh } => {
            let l = resolve(c, b, &src, Sz::L);
            let s = read(c, b, l, Sz::L);
            let (hi, lo) = alu::mull(c, signed, dh.is_some(), s, c.d[dl as usize]);
            // Dl first, then Dh (the 68020 order: Dh wins if they are the same register).
            c.d[dl as usize] = lo;
            if let Some(h) = dh {
                c.d[h as usize] = hi;
            }
        }
        Op::DivL { signed, src, dq, dr, wide } => {
            let l = resolve(c, b, &src, Sz::L);
            let s = read(c, b, l, Sz::L);
            let hi = if wide { Some(c.d[dr as usize]) } else { None };
            match alu::divl(c, signed, s, hi, c.d[dq as usize]) {
                DivL::Ok(q, r) => {
                    c.d[dr as usize] = r;
                    c.d[dq as usize] = q;
                }
                DivL::Overflow => {}
                DivL::Zero => c.trap(b, VEC_ZERO_DIVIDE, next, ins.pc),
            }
        }
    }
}

/// Long store with predecrement order: low word at `a + 2` first, then the high word at `a`.
#[inline(always)]
pub fn write_pd32<B: Bus>(b: &mut B, a: u32, v: u32) {
    b.write16(a.wrapping_add(2), v as u16);
    b.write16(a, (v >> 16) as u16);
}

/// MOVEM in Musashi's transfer order.
pub fn movem<B: Bus>(c: &mut Cpu, b: &mut B, sz: Sz, to_mem: bool, mask: u16, ea: &Ea) {
    let n = sz.bytes();
    let get = |c: &Cpu, i: usize| if i < 8 { c.d[i] } else { c.a[i - 8] };
    if to_mem {
        if let Ea::Pre(r) = *ea {
            // Mask bit 0 = A7 ... bit 15 = D0; registers are stored from A7 down to D0.
            let mut a = c.a[r as usize];
            for i in 0..16 {
                if mask & (1 << i) != 0 {
                    a = a.wrapping_sub(n);
                    let mut v = get(c, 15 - i);
                    // The 68020 stores the base register as decremented by one transfer.
                    if c.model == Model::M68020 && 15 - i == 8 + r as usize {
                        v = v.wrapping_sub(n);
                    }
                    if sz == Sz::W { b.write16(a, v as u16) } else { write_pd32(b, a, v) }
                }
            }
            c.a[r as usize] = a;
        } else {
            let mut a = ea_addr(c, b, ea);
            for i in 0..16 {
                if mask & (1 << i) != 0 {
                    let v = get(c, i);
                    if sz == Sz::W { b.write16(a, v as u16) } else { b.write32(a, v) }
                    a = a.wrapping_add(n);
                }
            }
        }
    } else {
        let (mut a, post) = match *ea {
            Ea::Post(r) => (c.a[r as usize], Some(r)),
            _ => (ea_addr(c, b, ea), None),
        };
        for i in 0..16 {
            if mask & (1 << i) != 0 {
                let v = if sz == Sz::W { b.read16(a) as i16 as i32 as u32 } else { b.read32(a) };
                if i < 8 { c.d[i] = v } else { c.a[i - 8] = v }
                a = a.wrapping_add(n);
            }
        }
        if let Some(r) = post {
            c.a[r as usize] = a;
        }
    }
}
