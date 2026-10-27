//! Lifting decoded instructions to micro-ops.
//!
//! The micro-op order follows `m68k::interp::exec` (and therefore the instruction emitter)
//! exactly: resolve the source EA (with its (An)+/-(An) side effect), read it, resolve the
//! destination EA, read it if the operation reads it, compute, set flags, write. The bus sees the
//! same accesses in the same order. Every instruction is lifted; control flow ends in a [`Term`].
use super::{cond_flags, AddrExpr, ArOp, BinOp, Body, Dst, IrInsn, IrOp, Lazy, Term, UnOp, XOp, ALL, V};
use crate::emit::flag_effects;
use m68k::*;

/// Where an operand lives once its EA is resolved.
#[derive(Clone, Copy)]
enum Loc {
    D(u8),
    A(u8),
    Imm(u32),
    Mem(V),
}

struct Lifter {
    ops: Vec<IrOp>,
    nt: u32,
}

fn step(sz: Sz, r: u8) -> u32 {
    if sz == Sz::B && r == 7 { 2 } else { sz.bytes() }
}

impl Lifter {
    fn t(&mut self) -> u32 {
        self.nt += 1;
        self.nt
    }
    fn addr(ea: &Ea) -> Option<AddrExpr> {
        Some(match *ea {
            Ea::Ind(r) => AddrExpr::Disp(r, 0),
            Ea::Disp(r, d) => AddrExpr::Disp(r, d as i32),
            Ea::Idx(r, d, x) => AddrExpr::Idx(r, d as i32, x),
            Ea::AbsW(a) | Ea::AbsL(a) | Ea::PcDisp(a) => AddrExpr::Abs(a),
            Ea::PcIdx(b, x) => AddrExpr::PcIdx(b, x),
            _ => return None,
        })
    }
    /// `t = address of a control EA`
    fn ea_addr(&mut self, ea: &Ea) -> V {
        let t = self.t();
        self.ops.push(IrOp::Addr { t, ea: Self::addr(ea).expect("control EA (68020 EAs: the IR is 68000-only)") });
        V::T(t)
    }
    fn resolve(&mut self, ea: &Ea, sz: Sz) -> Loc {
        match *ea {
            Ea::D(r) => Loc::D(r),
            Ea::A(r) => Loc::A(r),
            Ea::Imm(v) => Loc::Imm(v),
            Ea::Post(r) => {
                let t = self.t();
                self.ops.push(IrOp::PostInc { t, an: r, step: step(sz, r) });
                Loc::Mem(V::T(t))
            }
            Ea::Pre(r) => {
                let t = self.t();
                self.ops.push(IrOp::PreDec { t, an: r, step: step(sz, r) });
                Loc::Mem(V::T(t))
            }
            _ => Loc::Mem(self.ea_addr(ea)),
        }
    }
    fn read(&mut self, l: Loc, sz: Sz) -> V {
        match l {
            Loc::Imm(v) => V::C(v & sz.mask()),
            Loc::D(r) => self.get_d(r, sz),
            Loc::A(r) => {
                let t = self.t();
                self.ops.push(IrOp::GetA { t, r, sz });
                V::T(t)
            }
            Loc::Mem(a) => self.load(sz, a),
        }
    }
    fn write(&mut self, l: Loc, sz: Sz, v: V) {
        match l {
            Loc::D(r) => self.ops.push(IrOp::SetD { r, sz, v }),
            Loc::A(r) => self.ops.push(IrOp::SetA { r, v }),
            Loc::Mem(a) => self.ops.push(IrOp::Store { sz, addr: a, v }),
            Loc::Imm(_) => panic!("write to an immediate"),
        }
    }
    fn get_d(&mut self, r: u8, sz: Sz) -> V {
        let t = self.t();
        self.ops.push(IrOp::GetD { t, r, sz });
        V::T(t)
    }
    fn get_a(&mut self, r: u8) -> V {
        let t = self.t();
        self.ops.push(IrOp::GetA { t, r, sz: Sz::L });
        V::T(t)
    }
    /// D0-D7 = 0-7, A0-A7 = 8-15
    fn get_r(&mut self, r: u8) -> V {
        if r < 8 { self.get_d(r, Sz::L) } else { self.get_a(r - 8) }
    }
    fn set_r(&mut self, r: u8, v: V) {
        self.ops.push(if r < 8 { IrOp::SetD { r, sz: Sz::L, v } } else { IrOp::SetA { r: r - 8, v } });
    }
    fn load(&mut self, sz: Sz, addr: V) -> V {
        let t = self.t();
        self.ops.push(IrOp::Load { t, sz, addr });
        V::T(t)
    }
    fn sext_w(&mut self, v: V) -> V {
        if let V::C(k) = v {
            return V::C(k as u16 as i16 as i32 as u32);
        }
        let t = self.t();
        self.ops.push(IrOp::SextW { t, v });
        V::T(t)
    }
    /// `a op b`, folded when both are constants.
    fn bin(&mut self, op: BinOp, a: V, b: V) -> V {
        if let (V::C(x), V::C(y)) = (a, b) {
            return V::C(op.apply(x, y));
        }
        let t = self.t();
        self.ops.push(IrOp::Bin { t, op, a, b });
        V::T(t)
    }
    fn pop(&mut self, sz: Sz) -> V {
        let t = self.t();
        self.ops.push(IrOp::Pop { t, sz });
        V::T(t)
    }
}

/// (flags always written, flags read) used by the liveness pass: [`flag_effects`] (shared with the
/// instruction emitter), except that conditions read only the flags they test and jumps inside
/// the function read none (their edges carry the successors' needs; edges that leave native code
/// make every flag live).
pub fn ir_flag_effects(op: &Op) -> (u8, u8) {
    match *op {
        Op::Bcc { cond, .. } | Op::Dbcc { cond, .. } | Op::Scc { cond, .. } => (0, cond_flags(cond)),
        Op::Bra { .. } | Op::Jmp { .. } => (0, 0),
        _ => flag_effects(op),
    }
}

/// Lifts one instruction.
pub fn lift(ins: &Instr) -> IrInsn {
    let (defs, uses) = ir_flag_effects(&ins.op);
    let (ops, term) = lift_ops(ins);
    IrInsn { instr: *ins, body: Body::Lifted(ops), term, flag_defs: defs, flag_uses: uses, live_after: ALL, live_exit: ALL, fused: None, export: None }
}

fn alu_op(op: AluOp) -> ArOp {
    match op {
        AluOp::Add => ArOp::Add,
        AluOp::Sub => ArOp::Sub,
        AluOp::And => ArOp::And,
        AluOp::Or => ArOp::Or,
        AluOp::Eor => ArOp::Eor,
        AluOp::Cmp => ArOp::Cmp,
    }
}

fn lift_ops(ins: &Instr) -> (Vec<IrOp>, Option<Term>) {
    let mut l = Lifter { ops: vec![], nt: 0 };
    // The lazy flags of the instruction's flag-setting op: what `flag_effects` says it writes.
    let lazy = Lazy::new(flag_effects(&ins.op).0);
    let pc = ins.pc;
    let next = ins.next();
    let mut term = None;
    match ins.op {
        Op::MoveFromCcr { .. } | Op::Extb { .. } | Op::MulL { .. } | Op::DivL { .. } | Op::Movec { .. } => {
            panic!("${:06x}: {}: a 68020 instruction (the IR is 68000-only)", pc, ins)
        }
        Op::Nop => {}
        Op::Reset => l.ops.push(IrOp::Priv { pc }),
        Op::Moveq { data, dn } => {
            let v = V::C(data as i32 as u32);
            l.ops.push(IrOp::LogicFlags { sz: Sz::L, v, flags: lazy });
            l.ops.push(IrOp::SetD { r: dn, sz: Sz::L, v });
        }
        Op::Move { sz, src, dst } => {
            let sl = l.resolve(&src, sz);
            let v = l.read(sl, sz);
            let dl = l.resolve(&dst, sz);
            l.ops.push(IrOp::LogicFlags { sz, v, flags: lazy });
            match (sz, dst, dl) {
                (Sz::L, Ea::Pre(_), Loc::Mem(a)) => l.ops.push(IrOp::StorePd32 { addr: a, v }),
                _ => l.write(dl, sz, v),
            }
        }
        Op::MoveA { sz, src, an } => {
            let sl = l.resolve(&src, sz);
            let mut v = l.read(sl, sz);
            if sz == Sz::W {
                v = l.sext_w(v);
            }
            l.ops.push(IrOp::SetA { r: an, v });
        }
        Op::Lea { src, an } => {
            let a = l.ea_addr(&src);
            l.ops.push(IrOp::SetA { r: an, v: a });
        }
        Op::Pea { src } => {
            let a = l.ea_addr(&src);
            l.ops.push(IrOp::Push { sz: Sz::L, v: a });
        }
        Op::Alu { op, sz, src, dst } => {
            let op = alu_op(op);
            let sl = l.resolve(&src, sz);
            let s = l.read(sl, sz);
            let dl = l.resolve(&dst, sz);
            let d = l.read(dl, sz);
            if op == ArOp::Cmp {
                l.ops.push(IrOp::Arith { t: None, op, sz, s, d, flags: lazy });
            } else {
                let t = l.t();
                l.ops.push(IrOp::Arith { t: Some(t), op, sz, s, d, flags: lazy });
                l.write(dl, sz, V::T(t));
            }
        }
        Op::AddA { sz, src, an } | Op::SubA { sz, src, an } => {
            let sl = l.resolve(&src, sz);
            let mut v = l.read(sl, sz);
            if sz == Sz::W {
                v = l.sext_w(v);
            }
            l.ops.push(IrOp::AddA { r: an, v, sub: matches!(ins.op, Op::SubA { .. }) });
        }
        Op::CmpA { sz, src, an } => {
            let sl = l.resolve(&src, sz);
            let mut s = l.read(sl, sz);
            if sz == Sz::W {
                s = l.sext_w(s);
            }
            let d = l.get_a(an);
            l.ops.push(IrOp::Arith { t: None, op: ArOp::Cmp, sz: Sz::L, s, d, flags: lazy });
        }
        Op::Cmpm { sz, ax, ay } => {
            let sl = l.resolve(&Ea::Post(ay), sz);
            let s = l.read(sl, sz);
            let dl = l.resolve(&Ea::Post(ax), sz);
            let d = l.read(dl, sz);
            l.ops.push(IrOp::Arith { t: None, op: ArOp::Cmp, sz, s, d, flags: lazy });
        }
        Op::Clr { sz, ea } => {
            let dl = l.resolve(&ea, sz);
            l.ops.push(IrOp::LogicFlags { sz, v: V::C(0), flags: lazy });
            l.write(dl, sz, V::C(0));
        }
        Op::Tst { sz, ea } => {
            let sl = l.resolve(&ea, sz);
            let v = l.read(sl, sz);
            l.ops.push(IrOp::LogicFlags { sz, v, flags: lazy });
        }
        Op::Exg { rx, ry } => {
            let x = l.get_r(rx);
            let y = l.get_r(ry);
            l.set_r(rx, y);
            l.set_r(ry, x);
        }
        Op::Neg { sz, ea } | Op::Not { sz, ea } => {
            let dl = l.resolve(&ea, sz);
            let v = l.read(dl, sz);
            let t = l.t();
            let op = if matches!(ins.op, Op::Neg { .. }) { UnOp::Neg } else { UnOp::Not };
            l.ops.push(IrOp::Unary { t, op, sz, v, flags: lazy });
            l.write(dl, sz, V::T(t));
        }
        Op::Negx { sz, ea } => {
            let dl = l.resolve(&ea, sz);
            let v = l.read(dl, sz);
            let t = l.t();
            l.ops.push(IrOp::Extend { t, op: XOp::Negx, sz, s: v, d: V::C(0) });
            l.write(dl, sz, V::T(t));
        }
        Op::Addx { sz, rx, ry, mem } | Op::Subx { sz, rx, ry, mem } => {
            let op = if matches!(ins.op, Op::Addx { .. }) { XOp::Addx } else { XOp::Subx };
            let (s, dl) = if mem {
                let sl = l.resolve(&Ea::Pre(ry), sz);
                let s = l.read(sl, sz);
                (s, l.resolve(&Ea::Pre(rx), sz))
            } else {
                (l.get_d(ry, Sz::L), Loc::D(rx))
            };
            let d = l.read(dl, sz);
            let t = l.t();
            l.ops.push(IrOp::Extend { t, op, sz, s, d });
            l.write(dl, sz, V::T(t));
        }
        Op::Abcd { rx, ry, mem } | Op::Sbcd { rx, ry, mem } => {
            let op = if matches!(ins.op, Op::Abcd { .. }) { XOp::Abcd } else { XOp::Sbcd };
            let (s, dl, d) = if mem {
                let sl = l.resolve(&Ea::Pre(ry), Sz::B);
                let s = l.read(sl, Sz::B);
                let dl = l.resolve(&Ea::Pre(rx), Sz::B);
                let d = l.read(dl, Sz::B);
                (s, dl, d)
            } else {
                let s = l.get_d(ry, Sz::L);
                let d = l.get_d(rx, Sz::L);
                (s, Loc::D(rx), d)
            };
            let t = l.t();
            l.ops.push(IrOp::Extend { t, op, sz: Sz::B, s, d });
            l.write(dl, Sz::B, V::T(t));
        }
        Op::Nbcd { ea } => {
            let dl = l.resolve(&ea, Sz::B);
            let v = l.read(dl, Sz::B);
            let dst = match dl {
                Loc::D(r) => Dst::D(r),
                Loc::Mem(a) => Dst::M(a),
                _ => panic!("nbcd destination"),
            };
            l.ops.push(IrOp::Nbcd { v, dst });
        }
        Op::Swap { dn } => {
            let v = l.get_d(dn, Sz::L);
            let r = l.bin(BinOp::Rotl, v, V::C(16));
            l.ops.push(IrOp::LogicFlags { sz: Sz::L, v: r, flags: lazy });
            l.ops.push(IrOp::SetD { r: dn, sz: Sz::L, v: r });
        }
        Op::Ext { sz, dn } => {
            let v = l.get_d(dn, Sz::L);
            let t = l.t();
            if sz == Sz::W {
                l.ops.push(IrOp::SextB { t, v });
            } else {
                l.ops.push(IrOp::SextW { t, v });
            }
            l.ops.push(IrOp::LogicFlags { sz, v: V::T(t), flags: lazy });
            l.ops.push(IrOp::SetD { r: dn, sz, v: V::T(t) });
        }
        Op::Tas { ea } => {
            let dl = l.resolve(&ea, Sz::B);
            let v = l.read(dl, Sz::B);
            l.ops.push(IrOp::LogicFlags { sz: Sz::B, v, flags: lazy });
            let r = l.bin(BinOp::Or, v, V::C(0x80));
            l.write(dl, Sz::B, r);
        }
        Op::ShiftReg { kind, left, sz, count, dn } => {
            let v = l.get_d(dn, Sz::L);
            let n = match count {
                ShiftCount::Imm(n) => V::C(n as u32),
                ShiftCount::Reg(r) => {
                    let c = l.get_d(r, Sz::L);
                    l.bin(BinOp::And, c, V::C(63))
                }
            };
            let t = l.t();
            l.ops.push(IrOp::Shift { t, kind, left, sz, v, n, flags: lazy });
            l.ops.push(IrOp::SetD { r: dn, sz, v: V::T(t) });
        }
        Op::ShiftMem { kind, left, ea } => {
            let dl = l.resolve(&ea, Sz::W);
            let v = l.read(dl, Sz::W);
            let t = l.t();
            l.ops.push(IrOp::Shift { t, kind, left, sz: Sz::W, v, n: V::C(1), flags: lazy });
            l.write(dl, Sz::W, V::T(t));
        }
        Op::Mulu { src, dn } | Op::Muls { src, dn } => {
            let sl = l.resolve(&src, Sz::W);
            let s = l.read(sl, Sz::W);
            let d = l.get_d(dn, Sz::L);
            let t = l.t();
            l.ops.push(IrOp::Mul { t, signed: matches!(ins.op, Op::Muls { .. }), s, d, flags: lazy });
            l.ops.push(IrOp::SetD { r: dn, sz: Sz::L, v: V::T(t) });
        }
        Op::Divu { src, dn } | Op::Divs { src, dn } => {
            let sl = l.resolve(&src, Sz::W);
            let s = l.read(sl, Sz::W);
            let d = l.get_d(dn, Sz::L);
            l.ops.push(IrOp::Div { signed: matches!(ins.op, Op::Divs { .. }), s, d, dn, pc: next });
        }
        Op::Chk { src, dn } => {
            let sl = l.resolve(&src, Sz::W);
            let bound = l.read(sl, Sz::W);
            let d = l.get_d(dn, Sz::L);
            l.ops.push(IrOp::Chk { d, bound, pc: next });
        }
        Op::Bit { op, num, dst } => {
            let n = match num {
                BitNum::Imm(n) => V::C(n as u32),
                BitNum::Reg(r) => l.get_d(r, Sz::L),
            };
            let (dl, sz, w) = if let Ea::D(r) = dst { (Loc::D(r), Sz::L, 31) } else { (l.resolve(&dst, Sz::B), Sz::B, 7) };
            let k = l.bin(BinOp::And, n, V::C(w));
            let m = l.bin(BinOp::Shl, V::C(1), k);
            let v = l.read(dl, sz);
            l.ops.push(IrOp::BitZ { v, m, flags: lazy });
            let r = match op {
                BitOp::Tst => None,
                BitOp::Chg => Some(l.bin(BinOp::Xor, v, m)),
                BitOp::Clr => Some(l.bin(BinOp::AndNot, v, m)),
                BitOp::Set => Some(l.bin(BinOp::Or, v, m)),
            };
            if let Some(r) = r {
                l.write(dl, sz, r);
            }
        }
        Op::Movep { sz, dn, an, disp, to_mem } => {
            let n = sz.bytes();
            let at = |l: &mut Lifter, i: u32| {
                let t = l.t();
                l.ops.push(IrOp::Addr { t, ea: AddrExpr::Disp(an, disp as i32 + 2 * i as i32) });
                V::T(t)
            };
            if to_mem {
                let v = l.get_d(dn, Sz::L);
                for i in 0..n {
                    let a = at(&mut l, i);
                    let b = l.bin(BinOp::Shr, v, V::C(8 * (n - 1 - i)));
                    l.ops.push(IrOp::Store { sz: Sz::B, addr: a, v: b });
                }
            } else {
                let mut acc = V::C(0);
                for i in 0..n {
                    let a = at(&mut l, i);
                    let b = l.load(Sz::B, a);
                    let sh = l.bin(BinOp::Shl, b, V::C(8 * (n - 1 - i)));
                    acc = l.bin(BinOp::Or, acc, sh);
                }
                l.ops.push(IrOp::SetD { r: dn, sz, v: acc });
            }
        }
        Op::Movem { sz, to_mem, mask, ea } => {
            let n = V::C(sz.bytes());
            if to_mem {
                if let Ea::Pre(r) = ea {
                    // mask bit 0 = A7 ... bit 15 = D0, stored from A7 down to D0
                    let mut a = l.get_a(r);
                    for i in 0..16u8 {
                        if mask & (1 << i) != 0 {
                            a = l.bin(BinOp::Sub, a, n);
                            let v = l.get_r(15 - i);
                            l.ops.push(if sz == Sz::W { IrOp::Store { sz: Sz::W, addr: a, v } } else { IrOp::StorePd32 { addr: a, v } });
                        }
                    }
                    l.ops.push(IrOp::SetA { r, v: a });
                } else {
                    let mut a = l.ea_addr(&ea);
                    for i in 0..16u8 {
                        if mask & (1 << i) != 0 {
                            let v = l.get_r(i);
                            l.ops.push(IrOp::Store { sz, addr: a, v });
                            a = l.bin(BinOp::Add, a, n);
                        }
                    }
                }
            } else {
                let (mut a, post) = match ea {
                    Ea::Post(r) => (l.get_a(r), Some(r)),
                    _ => (l.ea_addr(&ea), None),
                };
                for i in 0..16u8 {
                    if mask & (1 << i) != 0 {
                        let mut v = l.load(sz, a);
                        if sz == Sz::W {
                            v = l.sext_w(v);
                        }
                        l.set_r(i, v);
                        a = l.bin(BinOp::Add, a, n);
                    }
                }
                if let Some(r) = post {
                    l.ops.push(IrOp::SetA { r, v: a });
                }
            }
        }
        Op::LogicSr { op, imm, sr } => {
            let bop = match op {
                AluOp::Or => BinOp::Or,
                AluOp::And => BinOp::And,
                _ => BinOp::Xor,
            };
            let t = l.t();
            if sr {
                l.ops.push(IrOp::Priv { pc });
                l.ops.push(IrOp::GetSr { t });
                let v = l.bin(bop, V::T(t), V::C(imm as u32));
                l.ops.push(IrOp::SetSr { v });
            } else {
                l.ops.push(IrOp::GetCcr { t });
                let v = l.bin(bop, V::T(t), V::C(imm as u32 & 0xFF));
                l.ops.push(IrOp::SetCcr { v });
            }
        }
        Op::MoveFromSr { ea } => {
            let dl = l.resolve(&ea, Sz::W);
            let t = l.t();
            l.ops.push(IrOp::GetSr { t });
            l.write(dl, Sz::W, V::T(t));
        }
        Op::MoveToCcr { ea } => {
            let sl = l.resolve(&ea, Sz::W);
            let v = l.read(sl, Sz::W);
            l.ops.push(IrOp::SetCcr { v });
        }
        Op::MoveToSr { ea } => {
            l.ops.push(IrOp::Priv { pc });
            let sl = l.resolve(&ea, Sz::W);
            let v = l.read(sl, Sz::W);
            l.ops.push(IrOp::SetSr { v });
        }
        Op::MoveUsp { to_usp, an } => {
            l.ops.push(IrOp::Priv { pc });
            if to_usp {
                let v = l.get_a(an);
                l.ops.push(IrOp::SetOsp { v });
            } else {
                let t = l.t();
                l.ops.push(IrOp::GetOsp { t });
                l.ops.push(IrOp::SetA { r: an, v: V::T(t) });
            }
        }
        Op::Link { an, disp } => {
            if an == 7 {
                l.ops.push(IrOp::AddA { r: 7, v: V::C(4), sub: true });
                let a = l.get_a(7);
                l.ops.push(IrOp::Store { sz: Sz::L, addr: a, v: a });
            } else {
                let v = l.get_a(an);
                l.ops.push(IrOp::Push { sz: Sz::L, v });
                let sp = l.get_a(7);
                l.ops.push(IrOp::SetA { r: an, v: sp });
            }
            l.ops.push(IrOp::AddA { r: 7, v: V::C(disp as i32 as u32), sub: false });
        }
        Op::Unlk { an } => {
            if an == 7 {
                let a = l.get_a(7);
                let v = l.load(Sz::L, a);
                l.ops.push(IrOp::SetA { r: 7, v });
            } else {
                let fp = l.get_a(an);
                l.ops.push(IrOp::SetA { r: 7, v: fp });
                let v = l.pop(Sz::L);
                l.ops.push(IrOp::SetA { r: an, v });
            }
        }
        Op::Scc { cond, ea } => {
            let dl = l.resolve(&ea, Sz::B);
            let t = l.t();
            l.ops.push(IrOp::Scc { t, cond });
            l.write(dl, Sz::B, V::T(t));
        }
        Op::Trapv => l.ops.push(IrOp::TrapIf { cond: Cond::Vs, vector: 7, pc: next }),
        // ---- control flow ------------------------------------------------------------------
        Op::Bcc { cond, target } => term = Some(Term::Branch { cond, target }),
        Op::Dbcc { cond, dn, target } => term = Some(Term::Dbcc { cond, dn, target }),
        Op::Bra { target } => term = Some(Term::Goto { target, jmp: false }),
        Op::Bsr { target } => {
            l.ops.push(IrOp::Push { sz: Sz::L, v: V::C(next) });
            term = Some(Term::Call { target });
        }
        Op::Jsr { ea } => match ea {
            Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) => {
                l.ops.push(IrOp::Push { sz: Sz::L, v: V::C(next) });
                term = Some(Term::Call { target: t });
            }
            _ => {
                let t = l.ea_addr(&ea);
                l.ops.push(IrOp::Push { sz: Sz::L, v: V::C(next) });
                term = Some(Term::DynCall { target: t, ea });
            }
        },
        Op::Jmp { ea } => match ea {
            Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) => term = Some(Term::Goto { target: t, jmp: true }),
            _ => {
                let t = l.ea_addr(&ea);
                term = Some(Term::DynJump { target: t });
            }
        },
        Op::Rts => {
            let p = l.pop(Sz::L);
            term = Some(Term::Return { pc: p });
        }
        Op::Rtr => {
            let c = l.pop(Sz::W);
            l.ops.push(IrOp::SetCcr { v: c });
            let p = l.pop(Sz::L);
            term = Some(Term::Return { pc: p });
        }
        Op::Rte => {
            l.ops.push(IrOp::Priv { pc });
            let s = l.pop(Sz::W);
            let p = l.pop(Sz::L);
            l.ops.push(IrOp::SetSr { v: s });
            term = Some(Term::Return { pc: p });
        }
        Op::Stop { imm } => {
            l.ops.push(IrOp::Priv { pc });
            l.ops.push(IrOp::SetSr { v: V::C(imm as u32) });
            term = Some(Term::Stop);
        }
        Op::Trap { n } => term = Some(Term::Exception { vector: 32 + n as u32, pc: next }),
        Op::Illegal => term = Some(Term::Exception { vector: 4, pc }),
        Op::LineA => term = Some(Term::Exception { vector: 10, pc }),
        Op::LineF => term = Some(Term::Exception { vector: 11, pc }),
    }
    (l.ops, term)
}
