//! Rust from the IR.
//!
//! The function and crate structure is generated from the IR (blocks, edges, aliases), every
//! instruction is lowered from its micro-ops and terminator. The output has the same interface as
//! [`crate::emit_crate`]'s (`lookup`, `call`, `FUNCTIONS`, the per-function `f_xxxxxx`), so it is a
//! drop-in replacement. Functions with more instructions than the split limit become a small
//! dispatcher over parts of contiguous blocks (see [`emit_func`]).
use super::{plain_shift_count, AddrExpr, ArOp, BinOp, Block, CondSrc, Dst, FlagMode, Fused, IrFunc, IrInsn, IrOp, IrProgram, Term, UnOp, XOp, ALL, V};
use crate::emit::{cond_expr, Ctx, Mode};
use crate::{fn_name, Options, Stats, HEADER};
use m68k::{Cond, Ea, Model, ShiftKind, Sz, Xn};
use std::collections::BTreeMap;
use std::fmt::Write;

fn hex(v: u32) -> String {
    format!("0x{:x}", v)
}

fn v(x: V) -> String {
    match x {
        V::T(t) => format!("t{}", t),
        V::C(c) => format!("{}u32", hex(c)),
    }
}

fn xn(x: Xn) -> String {
    let r = if x.reg < 8 { format!("cpu.d[{}]", x.reg) } else { format!("cpu.a[{}]", x.reg - 8) };
    if x.long { r } else { format!("({} as i16 as i32 as u32)", r) }
}

fn sz(s: Sz) -> &'static str {
    s.rust()
}

fn masked(e: String, s: Sz) -> String {
    if s == Sz::L { e } else { format!("({} & {})", e, hex(s.mask())) }
}

fn shift_kind(k: ShiftKind) -> &'static str {
    match k {
        ShiftKind::As => "ShiftKind::As",
        ShiftKind::Ls => "ShiftKind::Ls",
        ShiftKind::Rox => "ShiftKind::Rox",
        ShiftKind::Ro => "ShiftKind::Ro",
    }
}

/// The flag-free form of a shift by `k` (1..=8), as `eval::plain_shift`.
fn plain_shift(kind: ShiftKind, left: bool, s: Sz, x: &str, k: u32) -> String {
    let m = hex(s.mask());
    match (kind, left) {
        (ShiftKind::As | ShiftKind::Ls, true) => format!("(({} << {}) & {})", x, k, m),
        (ShiftKind::Ls, false) => format!("(({} & {}) >> {})", x, m, k),
        (ShiftKind::As, false) => {
            let sx = match s {
                Sz::B => format!("({} as u8 as i8 as i32)", x),
                Sz::W => format!("({} as u16 as i16 as i32)", x),
                Sz::L => format!("({} as i32)", x),
            };
            format!("(({} >> {}) as u32 & {})", sx, k, m)
        }
        (ShiftKind::Ro, l) => {
            let k = k % s.bits();
            let f = if l { "rotate_left" } else { "rotate_right" };
            match (k, s) {
                (0, _) => format!("({} & {})", x, m),
                (_, Sz::B) => format!("(({} as u8).{}({}) as u32)", x, f, k),
                (_, Sz::W) => format!("(({} as u16).{}({}) as u32)", x, f, k),
                (_, Sz::L) => format!("{}.{}({})", x, f, k),
            }
        }
        (ShiftKind::Rox, _) => unreachable!("ROX has no plain form"),
    }
}

/// A fused condition (see [`CondSrc`]) as a Rust expression of the producer's exported operands
/// `f{k}a`, `f{k}b`; equal to `cond_expr(c)` on the flags the producer would have set.
pub fn fused_cond(f: Fused, c: Cond) -> String {
    let (a, b) = (format!("f{}a", f.from), format!("f{}b", f.from));
    let signed = |x: &str, s: Sz| match s {
        Sz::B => format!("({} as u8 as i8)", x),
        Sz::W => format!("({} as u16 as i16)", x),
        Sz::L => format!("({} as i32)", x),
    };
    match f.src {
        CondSrc::Sub(s) => {
            let (ua, ub) = (masked(a.clone(), s), masked(b.clone(), s));
            let (sa, sb) = (signed(&a, s), signed(&b, s));
            match c {
                Cond::Eq => format!("({} == {})", ub, ua),
                Cond::Ne => format!("({} != {})", ub, ua),
                Cond::Cs => format!("({} < {})", ub, ua),
                Cond::Cc => format!("({} >= {})", ub, ua),
                Cond::Hi => format!("({} > {})", ub, ua),
                Cond::Ls => format!("({} <= {})", ub, ua),
                Cond::Lt => format!("({} < {})", sb, sa),
                Cond::Ge => format!("({} >= {})", sb, sa),
                Cond::Gt => format!("({} > {})", sb, sa),
                Cond::Le => format!("({} <= {})", sb, sa),
                Cond::Mi => format!("({}.wrapping_sub({}) & {} != 0)", b, a, hex(s.msb())),
                Cond::Pl => format!("({}.wrapping_sub({}) & {} == 0)", b, a, hex(s.msb())),
                _ => unreachable!("{:?} from a subtraction", c),
            }
        }
        CondSrc::Add(s) => match c {
            Cond::Eq => format!("({}.wrapping_add({}) & {} == 0)", b, a, hex(s.mask())),
            Cond::Ne => format!("({}.wrapping_add({}) & {} != 0)", b, a, hex(s.mask())),
            Cond::Mi => format!("({}.wrapping_add({}) & {} != 0)", b, a, hex(s.msb())),
            Cond::Pl => format!("({}.wrapping_add({}) & {} == 0)", b, a, hex(s.msb())),
            _ => unreachable!("{:?} from an addition", c),
        },
        CondSrc::Logic(s) => match c {
            Cond::Eq | Cond::Ls => format!("({} & {} == 0)", a, hex(s.mask())),
            Cond::Ne | Cond::Hi => format!("({} & {} != 0)", a, hex(s.mask())),
            Cond::Mi | Cond::Lt => format!("({} & {} != 0)", a, hex(s.msb())),
            Cond::Pl | Cond::Ge => format!("({} & {} == 0)", a, hex(s.msb())),
            Cond::Gt => format!("({} > 0)", signed(&a, s)),
            Cond::Le => format!("({} <= 0)", signed(&a, s)),
            Cond::Cc | Cond::Vc | Cond::T => "true".into(),
            Cond::Cs | Cond::Vs | Cond::F => "false".into(),
        },
        CondSrc::Bit => match c {
            Cond::Eq => format!("({} & {} == 0)", a, b),
            Cond::Ne => format!("({} & {} != 0)", a, b),
            _ => unreachable!("{:?} from a bit test", c),
        },
    }
}

/// Lowering context of one instruction.
pub struct Lw<'a, 'b> {
    pub cx: &'a Ctx<'b>,
    /// Instructions after this one in the block (already counted in `cpu.ticks` at block entry).
    pub rest: u32,
    /// How the generated code leaves its Rust function (`return;`, `return false;` in parts).
    pub ret: &'a str,
    /// The instruction's condition, when fused (else the condition is read from the flags).
    pub cond: Option<String>,
}

impl Lw<'_, '_> {
    fn cond(&self, c: Cond) -> String {
        self.cond.clone().unwrap_or_else(|| cond_expr(c).to_string())
    }
    fn fix(&self) -> String {
        if self.rest > 0 { format!("cpu.ticks -= {}; ", self.rest) } else { String::new() }
    }
    fn exception(&self, vector: u32, pc: u32) -> String {
        format!("{}cpu.exception(bus, {}, {}); {}", self.fix(), vector, hex(pc), self.ret)
    }
    fn goto(&self, t: u32) -> String {
        match self.cx.mode {
            Mode::Func { .. } if self.cx.labels.contains(&t) => format!("pc = {}; continue;", hex(t)),
            _ => format!("cpu.pc = {}; {}", hex(t), self.ret),
        }
    }
    /// After the return address has been pushed and `cpu.pc` set to the callee.
    fn call_static(&self, o: &mut Vec<String>, target: u32, next: u32) {
        match self.cx.mode {
            Mode::Func { direct: true } if self.cx.funcs.contains_key(&target) && self.cx.labels.contains(&next) => {
                o.push(format!("{}(cpu, bus);", self.cx.funcs[&target]));
                o.push(format!("if cpu.pc != {} {{ {} }}", hex(next), self.ret));
                o.push(format!("pc = {}; continue;", hex(next)));
            }
            _ => o.push(self.ret.to_string()),
        }
    }
    fn call_dynamic(&self, o: &mut Vec<String>, next: u32) {
        match self.cx.mode {
            Mode::Func { direct: true } if self.cx.labels.contains(&next) => {
                o.push(format!("if !crate::call(cpu, bus) || cpu.pc != {} {{ {} }}", hex(next), self.ret));
                o.push(format!("pc = {}; continue;", hex(next)));
            }
            _ => o.push(self.ret.to_string()),
        }
    }
}

/// `exact` when the flags are needed, `plain` when not, both behind the budget test when guarded.
fn flagged(w: &Lw, mode: FlagMode, t: Option<u32>, exact: String, plain: String) -> String {
    let g = w.cx.budget_test();
    match (t, mode) {
        (Some(t), FlagMode::Exact) => format!("let t{} = {};", t, exact),
        (Some(t), FlagMode::Plain) => format!("let t{} = {};", t, plain),
        (Some(t), FlagMode::Guarded) => format!("let t{} = if {} {{ {} }} else {{ {} }};", t, g, exact, plain),
        (None, FlagMode::Exact) => format!("{};", exact),
        (None, FlagMode::Plain) => String::new(),
        (None, FlagMode::Guarded) => format!("if {} {{ {}; }}", g, exact),
    }
}

/// Lowers one micro-op to Rust statements.
pub fn lower(op: &IrOp, w: &Lw) -> String {
    let mode = op.flag_mode();
    match *op {
        IrOp::Addr { t, ea } => {
            let e = match ea {
                AddrExpr::Disp(r, 0) => format!("cpu.a[{}]", r),
                AddrExpr::Disp(r, d) => format!("cpu.a[{}].wrapping_add({})", r, hex(d as u32)),
                AddrExpr::Idx(r, d, x) => format!("cpu.a[{}].wrapping_add({}).wrapping_add({})", r, hex(d as u32), xn(x)),
                AddrExpr::Abs(a) => format!("{}u32", hex(a)),
                AddrExpr::PcIdx(b, x) => format!("{}u32.wrapping_add({})", hex(b), xn(x)),
            };
            format!("let t{}: u32 = {};", t, e)
        }
        IrOp::PostInc { t, an, step } => format!("let t{t} = cpu.a[{an}]; cpu.a[{an}] = t{t}.wrapping_add({step});"),
        IrOp::PreDec { t, an, step } => format!("let t{t} = cpu.a[{an}].wrapping_sub({step}); cpu.a[{an}] = t{t};"),
        IrOp::GetD { t, r, sz: s } => format!("let t{} = {};", t, masked(format!("cpu.d[{}]", r), s)),
        IrOp::GetA { t, r, sz: s } => format!("let t{} = {};", t, masked(format!("cpu.a[{}]", r), s)),
        IrOp::Load { t, sz: s, addr } => match s {
            Sz::B => format!("let t{} = bus.read8({}) as u32;", t, v(addr)),
            Sz::W => format!("let t{} = bus.read16({}) as u32;", t, v(addr)),
            Sz::L => format!("let t{} = bus.read32({});", t, v(addr)),
        },
        IrOp::SextW { t, v: x } => format!("let t{} = {} as u16 as i16 as i32 as u32;", t, v(x)),
        IrOp::SextB { t, v: x } => format!("let t{} = {} as u8 as i8 as i32 as u32;", t, v(x)),
        IrOp::Bin { t, op, a, b } => {
            let (a, b) = (v(a), v(b));
            let e = match op {
                BinOp::And => format!("{} & {}", a, b),
                BinOp::Or => format!("{} | {}", a, b),
                BinOp::Xor => format!("{} ^ {}", a, b),
                BinOp::AndNot => format!("{} & !{}", a, b),
                BinOp::Add => format!("{}.wrapping_add({})", a, b),
                BinOp::Sub => format!("{}.wrapping_sub({})", a, b),
                BinOp::Shl => format!("{} << {}", a, b),
                BinOp::Shr => format!("{} >> {}", a, b),
                BinOp::Rotl => format!("{}.rotate_left({})", a, b),
            };
            format!("let t{} = {};", t, e)
        }
        IrOp::Arith { t, op, sz: s, s: a, d, .. } => {
            let (a, d) = (v(a), v(d));
            let exact = match op {
                ArOp::Add => format!("alu::add(cpu, {}, {}, {})", sz(s), a, d),
                ArOp::Sub => format!("alu::sub(cpu, {}, {}, {})", sz(s), a, d),
                ArOp::Cmp => format!("alu::cmp(cpu, {}, {}, {})", sz(s), a, d),
                ArOp::And => format!("alu::logic(cpu, {}, {} & {})", sz(s), d, a),
                ArOp::Or => format!("alu::logic(cpu, {}, {} | {})", sz(s), d, a),
                ArOp::Eor => format!("alu::logic(cpu, {}, {} ^ {})", sz(s), d, a),
            };
            let plain = match op {
                ArOp::Add => format!("{}.wrapping_add({})", d, a),
                ArOp::Sub => format!("{}.wrapping_sub({})", d, a),
                ArOp::Cmp => String::new(),
                ArOp::And => format!("{} & {}", d, a),
                ArOp::Or => format!("{} | {}", d, a),
                ArOp::Eor => format!("{} ^ {}", d, a),
            };
            flagged(w, mode, t, exact, plain)
        }
        IrOp::LogicFlags { sz: s, v: x, .. } => flagged(w, mode, None, format!("alu::logic(cpu, {}, {})", sz(s), v(x)), String::new()),
        IrOp::Unary { t, op, sz: s, v: x, .. } => {
            let x = v(x);
            let (exact, plain) = match op {
                UnOp::Neg => (format!("alu::neg(cpu, {}, {})", sz(s), x), format!("0u32.wrapping_sub({})", x)),
                UnOp::Not => (format!("alu::logic(cpu, {}, !{})", sz(s), x), format!("!{}", x)),
            };
            flagged(w, mode, Some(t), exact, plain)
        }
        IrOp::Extend { t, op, sz: s, s: a, d } => {
            let (a, d) = (v(a), v(d));
            let e = match op {
                XOp::Addx => format!("alu::addx(cpu, {}, {}, {})", sz(s), a, d),
                XOp::Subx => format!("alu::subx(cpu, {}, {}, {})", sz(s), a, d),
                XOp::Negx => format!("alu::negx(cpu, {}, {})", sz(s), a),
                XOp::Abcd => format!("alu::abcd(cpu, {}, {})", a, d),
                XOp::Sbcd => format!("alu::sbcd(cpu, {}, {})", a, d),
            };
            format!("let t{} = {};", t, e)
        }
        IrOp::Nbcd { v: x, dst } => {
            let wr = match dst {
                Dst::D(r) => format!("cpu.d[{r}] = (cpu.d[{r}] & !0xff) | (r & 0xff);", r = r),
                Dst::M(a) => format!("bus.write8({}, r as u8);", v(a)),
            };
            format!("if let Some(r) = alu::nbcd(cpu, {}) {{ {} }}", v(x), wr)
        }
        IrOp::Shift { t, kind, left, sz: s, v: x, n, .. } => {
            let exact = format!("alu::shift(cpu, {}, {}, {}, {}, {})", shift_kind(kind), left, sz(s), v(x), v(n));
            let plain = match plain_shift_count(kind, n) {
                Some(k) => plain_shift(kind, left, s, &v(x), k),
                None => String::new(),
            };
            flagged(w, mode, Some(t), exact, plain)
        }
        IrOp::Mul { t, signed, s: a, d, .. } => {
            let (a, d) = (v(a), v(d));
            let (exact, plain) = if signed {
                (format!("alu::muls(cpu, {}, {})", a, d), format!("({} as u16 as i16 as i32).wrapping_mul({} as u16 as i16 as i32) as u32", a, d))
            } else {
                (format!("alu::mulu(cpu, {}, {})", a, d), format!("({} & 0xffff) * ({} & 0xffff)", a, d))
            };
            flagged(w, mode, Some(t), exact, plain)
        }
        IrOp::Div { signed, s: a, d, dn, pc } => format!(
            "match alu::{}(cpu, {}, {}) {{ alu::Div::Ok(v) => cpu.d[{}] = v, alu::Div::Overflow => {{}} alu::Div::Zero => {{ {} }} }}",
            if signed { "divs" } else { "divu" }, v(a), v(d), dn, w.exception(5, pc)),
        IrOp::Chk { d, bound, pc } => format!("if alu::chk(cpu, {}, {}) {{ {} }}", v(d), v(bound), w.exception(6, pc)),
        IrOp::BitZ { v: x, m, .. } => match mode {
            FlagMode::Exact => format!("cpu.z = {} & {} == 0;", v(x), v(m)),
            FlagMode::Guarded => format!("if {} {{ cpu.z = {} & {} == 0; }}", w.cx.budget_test(), v(x), v(m)),
            FlagMode::Plain => String::new(),
        },
        IrOp::Scc { t, cond } => format!("let t{} = if {} {{ 0xFFu32 }} else {{ 0 }};", t, w.cond(cond)),
        IrOp::GetCcr { t } => format!("let t{} = cpu.ccr() as u32;", t),
        IrOp::SetCcr { v: x } => format!("cpu.set_ccr({} as u8);", v(x)),
        IrOp::GetSr { t } => format!("let t{} = cpu.sr() as u32;", t),
        IrOp::SetSr { v: x } => format!("cpu.set_sr({} as u16);", v(x)),
        IrOp::GetOsp { t } => format!("let t{} = cpu.other_sp;", t),
        IrOp::SetOsp { v: x } => format!("cpu.other_sp = {};", v(x)),
        IrOp::Push { sz: s, v: x } => match s {
            Sz::L => format!("cpu.push32(bus, {});", v(x)),
            _ => format!("cpu.push16(bus, {} as u16);", v(x)),
        },
        IrOp::Pop { t, sz: s } => match s {
            Sz::L => format!("let t{} = cpu.pop32(bus);", t),
            _ => format!("let t{} = cpu.pop16(bus) as u32;", t),
        },
        IrOp::SetD { r, sz: s, v: x } => match s {
            Sz::L => format!("cpu.d[{}] = {};", r, v(x)),
            _ => format!("cpu.d[{r}] = (cpu.d[{r}] & !{m}) | ({x} & {m});", r = r, m = hex(s.mask()), x = v(x)),
        },
        IrOp::SetA { r, v: x } => format!("cpu.a[{}] = {};", r, v(x)),
        IrOp::AddA { r, v: x, sub } => format!("cpu.a[{r}] = cpu.a[{r}].{f}({x});", r = r, f = if sub { "wrapping_sub" } else { "wrapping_add" }, x = v(x)),
        IrOp::Store { sz: s, addr, v: x } => match s {
            Sz::B => format!("bus.write8({}, {} as u8);", v(addr), v(x)),
            Sz::W => format!("bus.write16({}, {} as u16);", v(addr), v(x)),
            Sz::L => format!("bus.write32({}, {});", v(addr), v(x)),
        },
        IrOp::StorePd32 { addr, v: x } => format!("m68k::interp::write_pd32(bus, {}, {});", v(addr), v(x)),
        IrOp::Priv { pc } => format!("if !cpu.s {{ {} }}", w.exception(8, pc)),
        IrOp::TrapIf { cond, vector, pc } => format!("if {} {{ {} }}", cond_expr(cond), w.exception(vector, pc)),
    }
}

/// Lowers a terminator. Returns true if execution may fall through to the next instruction.
pub fn lower_term(o: &mut Vec<String>, term: &Term, ins: &m68k::Instr, w: &Lw) -> bool {
    let cx = w.cx;
    let next = ins.next();
    match *term {
        Term::Branch { cond, target } => {
            o.push(format!("if {} {{", w.cond(cond)));
            o.push(format!("    {}", w.goto(target)));
            o.push("}".into());
            return true;
        }
        Term::Dbcc { cond, dn, target } => {
            o.push(format!("if !{} {{", w.cond(cond)));
            o.push(format!("    let w = (cpu.d[{d}] as u16).wrapping_sub(1); cpu.d[{d}] = (cpu.d[{d}] & 0xFFFF_0000) | w as u32;", d = dn));
            o.push(format!("    if w != 0xFFFF {{ {} }}", w.goto(target)));
            o.push("}".into());
            return true;
        }
        Term::Goto { target, jmp } => match cx.mode {
            Mode::Func { direct: true } if jmp && !cx.labels.contains(&target) && cx.funcs.contains_key(&target) => {
                o.push(format!("cpu.pc = {}; {}(cpu, bus); {}", hex(target), cx.funcs[&target], w.ret));
            }
            _ => o.push(w.goto(target)),
        },
        Term::Call { target } => {
            o.push(format!("cpu.pc = {};", hex(target)));
            w.call_static(o, target, next);
        }
        Term::DynCall { target, ea } => {
            o.push(format!("cpu.pc = {};", v(target)));
            match (ea, cx.stub_base) {
                (Ea::Disp(r, d), Some((br, base))) if br == r && matches!(cx.mode, Mode::Func { direct: true }) => {
                    // Manx/Aztec C call through a jump stub in the data section: `jsr d16(a4)`.
                    let stub = base.wrapping_add(d as i32 as u32);
                    match cx.stubs.get(&stub).copied().and_then(|f| cx.funcs.get(&f).map(|n| (f, n))) {
                        Some((f, name)) if cx.labels.contains(&next) => {
                            o.push(format!("if {} == {} {{ // stub -> {:06x}", v(target), hex(stub), f));
                            // +1: the stub's own `jmp` instruction, skipped here
                            o.push(format!("    {} cpu.pc = {}; {}(cpu, bus);", if cx.cycles { "cpu.ticks += 1; cpu.cycles += 12; bus.cycles(cpu.cycles);" } else { "cpu.ticks += 1;" }, hex(f), name));
                            o.push(format!("    if cpu.pc != {} {{ {} }}", hex(next), w.ret));
                            o.push(format!("    pc = {}; continue;", hex(next)));
                            o.push("}".into());
                            w.call_dynamic(o, next);
                        }
                        _ => w.call_dynamic(o, next),
                    }
                }
                _ => w.call_dynamic(o, next),
            }
        }
        Term::DynJump { target } => match cx.mode {
            Mode::Func { .. } => o.push(format!("pc = {}; continue;", v(target))),
            Mode::Single => o.push(format!("cpu.pc = {}; {}", v(target), w.ret)),
        },
        Term::Return { pc } => o.push(format!("cpu.pc = {}; {}", v(pc), w.ret)),
        Term::Exception { vector, pc } => o.push(w.exception(vector, pc)),
        Term::Stop => o.push(format!("cpu.stopped = true; cpu.pc = {}; {}", hex(next), w.ret)),
    }
    false
}

/// Emits instruction `k` of a block (comment and a `{}` scope; operands a later fused condition
/// needs are exported to `f{k}a`/`f{k}b`, declared before the scope). Returns true if execution
/// may fall through to the next instruction.
pub fn emit_insn(o: &mut String, ins: &IrInsn, w: &Lw, k: usize) -> bool {
    let _ = writeln!(o, "        // {:06x}: {}", ins.instr.pc, ins.instr);
    o.push_str(&w.cx.charge(&ins.instr));
    if ins.export.is_some() {
        let _ = writeln!(o, "        let f{k}a: u32; let f{k}b: u32;", k = k);
    }
    o.push_str("        {\n");
    for op in ins.ops() {
        let s = lower(op, w);
        if !s.is_empty() {
            let _ = writeln!(o, "        {}", s);
        }
    }
    if let Some((a, b)) = ins.export {
        let _ = writeln!(o, "        f{k}a = {}; f{k}b = {};", v(a), v(b), k = k);
    }
    let mut falls = true;
    if let Some(t) = &ins.term {
        let mut lines = vec![];
        falls = lower_term(&mut lines, t, &ins.instr, w);
        for l in lines {
            let _ = writeln!(o, "        {}", l);
        }
    }
    o.push_str("        }\n");
    falls
}

/// One block as a `match` arm: budget check, instructions, fall-through.
fn emit_block(o: &mut String, f: &IrFunc, b: &Block, cx: &Ctx, opts: &Options, ret: &str) {
    let n = b.insns.len();
    let _ = writeln!(o, "    0x{:x} => {{", b.start);
    let _ = writeln!(o, "        if {} {{ cpu.pc = 0x{:x}; {} }} {}", cx.budget_test(), b.start, ret, cx.count(n, crate::emit::block_cycles(b.insns.iter().map(|i| &i.instr))));
    let mut falls = true;
    for (k, ins) in b.insns.iter().enumerate() {
        let w = Lw { cx, rest: (n - 1 - k) as u32, ret, cond: ins.fused.zip(ins.cond()).map(|(f, c)| fused_cond(f, c)) };
        if opts.no_flag_elim {
            let mut i = ins.clone();
            i.set_live(ALL, ALL);
            falls = emit_insn(o, &i, &w, k);
        } else {
            falls = emit_insn(o, ins, &w, k);
        }
    }
    if falls {
        let last = b.end();
        if cx.labels.contains(&last) && f.blocks.contains_key(&last) {
            let _ = writeln!(o, "        pc = 0x{:x}; continue;", last);
        } else {
            let _ = writeln!(o, "        cpu.pc = 0x{:x}; {}", last, ret);
        }
    }
    let _ = writeln!(o, "    }}");
}

/// Splits the blocks (in address order) into parts of at most `max` instructions each (a block
/// is never split). One part when the function fits or `max` is 0.
pub fn partition(f: &IrFunc, max: usize) -> Vec<Vec<u32>> {
    let mut parts: Vec<Vec<u32>> = vec![vec![]];
    if max == 0 || f.insn_count() <= max {
        parts[0] = f.blocks.keys().copied().collect();
        return parts;
    }
    let mut n = 0;
    for b in f.blocks.values() {
        let k = b.insns.len();
        if n > 0 && n + k > max {
            parts.push(vec![]);
            n = 0;
        }
        parts.last_mut().unwrap().push(b.start);
        n += k;
    }
    parts
}

/// Emits one function from its IR (same entry semantics as [`crate::emit_func`]). A function
/// with more than `split` instructions (0: never) becomes a dispatcher over parts:
///
/// ```text
/// pub fn f_xxxxxx(cpu, bus) { loop { let more = match cpu.pc { <blocks of part k> => f_xxxxxx_pk(cpu, bus), .., _ => return }; if !more { return; } } }
/// fn f_xxxxxx_pk(cpu, bus) -> bool { let mut pc = cpu.pc; loop { match pc { <blocks>, _ => { cpu.pc = pc; return true; } } } }
/// ```
///
/// A part runs its blocks as the whole function would; a `continue` to a block of another part
/// (or to an address that is no block) leaves the part with `cpu.pc` set and `true`, and the
/// dispatcher enters the part that has the block, or leaves the function. Every other exit is the
/// function's own (`return false`). Nothing else changes: the budget checks, tick counts and
/// state at every exit are the same as unsplit.
#[allow(clippy::too_many_arguments)]
pub fn emit_func(f: &IrFunc, mode: Mode, funcs: &BTreeMap<u32, String>, stubs: &BTreeMap<u32, u32>, opts: &Options, doc: Option<&str>, split: usize) -> String {
    emit_func_with(f, mode, funcs, stubs, opts, doc, split, &Extras::default())
}

/// What the annotations add to the emission (see [`crate::ir::annot`]): nothing by default.
#[derive(Clone, Debug, Default)]
pub struct Extras {
    /// Entry -> the code words (address, value) a `Guarded` function was compiled from; the
    /// function checks them at entry and interprets one instruction instead if any changed.
    pub guards: BTreeMap<u32, Vec<(u32, u16)>>,
    /// Entry -> map name, written as `#[doc(alias = ..)]` (the Rust function name is unchanged,
    /// the dispatch tables and the games refer to it).
    pub aliases: BTreeMap<u32, String>,
    /// Readable names (`--names`): function names, doc lines, `names.txt`. None: `f_xxxxxx`.
    pub naming: Option<crate::names::Naming>,
}

/// [`emit_func`] with annotation extras.
#[allow(clippy::too_many_arguments)]
pub fn emit_func_with(f: &IrFunc, mode: Mode, funcs: &BTreeMap<u32, String>, stubs: &BTreeMap<u32, u32>, opts: &Options, doc: Option<&str>, split: usize, extras: &Extras) -> String {
    let mut o = String::new();
    let labels = f.labels();
    let stub_base = if opts.profile.data_stubs() { opts.base_reg } else { None };
    let cx = Ctx { mode, labels: &labels, funcs, stub_base, stubs, cycles: opts.cycles, model: Model::M68000 };
    let parts = partition(f, split);
    if let Some(d) = doc {
        let _ = writeln!(o, "/// {}", d);
    }
    let _ = writeln!(o, "/// 68000 function at ${:06x}: {} instructions, {} blocks (from the IR{}{}).", f.entry, f.insn_count(), labels.len(),
        f.key.map(|(s, off)| format!(", {}:{:05x}", s, off)).unwrap_or_default(),
        if parts.len() > 1 { format!("; {} parts", parts.len()) } else { String::new() });
    let naming = extras.naming.as_ref();
    if let Some(n) = naming {
        n.write_doc(&mut o, f.entry);
    }
    let name = naming.map_or_else(|| fn_name(f.entry), |n| n.name(f.entry));
    if let Some(a) = extras.aliases.get(&f.entry) {
        let _ = writeln!(o, "#[doc(alias = {:?})]", a);
    }
    let guard = extras.guards.get(&f.entry);
    if guard.is_some() {
        let _ = writeln!(o, "/// Guarded: the code words are checked at entry; if one changed, one instruction is interpreted instead.");
    }
    // The check runs when the function is entered from outside (cpu.pc is its entry or an alias).
    let emit_guard = |o: &mut String, ret: &str| {
        let Some(g) = guard else { return };
        let words: Vec<String> = g.iter().map(|(a, w)| format!("(0x{:x}, 0x{:04x})", a, w)).collect();
        let _ = writeln!(o, "    const CODE: [(u32, u16); {}] = [{}];", g.len(), words.join(", "));
        let _ = writeln!(o, "    if CODE.iter().any(|&(a, w)| bus.read16(a) != w) {{");
        let _ = writeln!(o, "        let ins = m68k::decode::decode(cpu.pc, &mut |a| bus.fetch16(a));");
        let _ = writeln!(o, "        cpu.cycles += m68k::cycles::cycles(&ins) as u64;");
        let _ = writeln!(o, "        bus.cycles(cpu.cycles);");
        let _ = writeln!(o, "        m68k::interp::exec(cpu, bus, &ins);");
        let _ = writeln!(o, "        cpu.ticks += 1;");
        let _ = writeln!(o, "        cpu.interp_count += 1;");
        let _ = writeln!(o, "        {}", ret);
        let _ = writeln!(o, "    }}");
    };
    if parts.len() == 1 {
        let _ = writeln!(o, "pub fn {}<B: Bus>(cpu: &mut Cpu, bus: &mut B) {{", name);
        emit_guard(&mut o, "return;");
        let _ = writeln!(o, "    let mut pc = cpu.pc;");
        let _ = writeln!(o, "    loop {{");
        let _ = writeln!(o, "    match pc {{");
        for b in f.blocks.values() {
            emit_block(&mut o, f, b, &cx, opts, "return;");
        }
        let _ = writeln!(o, "    _ => {{ cpu.pc = pc; return; }}");
        let _ = writeln!(o, "    }}");
        let _ = writeln!(o, "    }}");
        let _ = writeln!(o, "}}\n");
        if let Some(n) = naming {
            n.write_alias(&mut o, f.entry);
        }
        return o;
    }
    let _ = writeln!(o, "pub fn {}<B: Bus>(cpu: &mut Cpu, bus: &mut B) {{", name);
    emit_guard(&mut o, "return;");
    let _ = writeln!(o, "    loop {{");
    let _ = writeln!(o, "        let more = match cpu.pc {{");
    for (k, p) in parts.iter().enumerate() {
        let pats: Vec<String> = p.iter().map(|a| format!("0x{:x}", a)).collect();
        let _ = writeln!(o, "            {} => {}_p{}(cpu, bus),", pats.join(" | "), name, k);
    }
    let _ = writeln!(o, "            _ => return,");
    let _ = writeln!(o, "        }};");
    let _ = writeln!(o, "        if !more {{ return; }}");
    let _ = writeln!(o, "    }}");
    let _ = writeln!(o, "}}\n");
    if let Some(n) = naming {
        n.write_alias(&mut o, f.entry);
    }
    for (k, p) in parts.iter().enumerate() {
        let _ = writeln!(o, "/// Part {} of {} (${:06x}-${:06x}).", k, name, p[0], f.blocks[p.last().unwrap()].end());
        let _ = writeln!(o, "#[inline(never)]");
        let _ = writeln!(o, "fn {}_p{}<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> bool {{", name, k);
        let _ = writeln!(o, "    let mut pc = cpu.pc;");
        let _ = writeln!(o, "    loop {{");
        let _ = writeln!(o, "    match pc {{");
        for a in p {
            emit_block(&mut o, f, &f.blocks[a], &cx, opts, "return false;");
        }
        let _ = writeln!(o, "    _ => {{ cpu.pc = pc; return true; }}");
        let _ = writeln!(o, "    }}");
        let _ = writeln!(o, "    }}");
        let _ = writeln!(o, "}}\n");
    }
    o
}

/// One instruction as its own function body (fuzz tests, [`Mode::Single`]): the lowered
/// instruction, then `cpu.pc = next` if it falls through.
pub fn emit_single(o: &mut String, ins: &IrInsn, cx: &Ctx) {
    let w = Lw { cx, rest: 0, ret: "return;", cond: None };
    if emit_insn(o, ins, &w, 0) {
        let _ = writeln!(o, "        cpu.pc = 0x{:x};", ins.instr.next());
    }
}

/// Writes the crate from the IR: same files and interface as [`crate::emit_crate`]. `split`: the
/// largest function emitted in one piece, in instructions (0: no limit). The dispatch table is a
/// sorted address table searched by `lookup`, and one `match` arm per function, rather than one
/// arm per address.
#[allow(clippy::too_many_arguments)]
pub fn emit_crate(dir: &std::path::Path, crate_name: &str, m68k_path: &std::path::Path, prog: &IrProgram, mode: Mode, opts: &Options, stubs: &BTreeMap<u32, u32>, names: &BTreeMap<u32, String>, extra: &str, split: usize) -> std::io::Result<Stats> {
    emit_crate_with(dir, crate_name, m68k_path, prog, mode, opts, stubs, names, extra, split, &Extras::default())
}

/// [`emit_crate`] with annotation extras.
#[allow(clippy::too_many_arguments)]
pub fn emit_crate_with(dir: &std::path::Path, crate_name: &str, m68k_path: &std::path::Path, prog: &IrProgram, mode: Mode, opts: &Options, stubs: &BTreeMap<u32, u32>, names: &BTreeMap<u32, String>, extra: &str, split: usize, extras: &Extras) -> std::io::Result<Stats> {
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::write(dir.join("Cargo.toml"), format!(
        "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nm68k = {{ package = \"m68k-rc\", path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
        crate_name, m68k_path.display()))?;
    if let Some(n) = &extras.naming {
        std::fs::write(dir.join("names.txt"), &n.file)?;
    }
    let fn_name = |e: u32| extras.naming.as_ref().map_or_else(|| fn_name(e), |n| n.name(e));
    let fnames: BTreeMap<u32, String> = prog.funcs.keys().map(|&e| (e, format!("crate::{}", fn_name(e)))).collect();
    let mut stats = Stats::default();
    let mut lib = String::from(HEADER);
    lib.push_str("use m68k::{Bus, Cpu};\nuse m68k::runtime::NativeFn;\n\n");
    let entries: Vec<&IrFunc> = prog.funcs.values().collect();
    for (i, part) in entries.chunks(40).enumerate() {
        let mut s = String::from(HEADER);
        s.push_str("use m68k::{alu, Bus, Cpu, Sz, ShiftKind};\n\n");
        for f in part {
            s.push_str(&emit_func_with(f, mode, &fnames, stubs, opts, names.get(&f.entry).map(|s| s.as_str()), split, extras));
            stats.functions += 1;
            stats.instructions += f.insn_count();
            stats.blocks += f.labels().len();
            stats.jump_tables += f.jump_tables.len();
            stats.dyn_jumps += f.dyn_jumps as usize;
            stats.dyn_calls += f.dyn_calls as usize;
        }
        std::fs::write(dir.join("src").join(format!("part{}.rs", i)), s)?;
        let _ = writeln!(lib, "mod part{};\npub use part{}::*;", i, i);
    }
    // Dispatch table from the entry aliases: the first function to claim an address owns it.
    let mut table: BTreeMap<u32, u32> = BTreeMap::new();
    for f in prog.funcs.values() {
        table.entry(f.entry).or_insert(f.entry);
    }
    for f in prog.funcs.values() {
        for &a in &f.aliases {
            table.entry(a).or_insert(f.entry);
        }
    }
    stats.entries = table.len();
    let index: BTreeMap<u32, usize> = prog.funcs.keys().enumerate().map(|(i, &e)| (e, i)).collect();
    lib.push_str("\n/// Native code for a 68000 address, if any.\n#[inline]\npub fn lookup<B: Bus>(pc: u32) -> Option<NativeFn<B>> {\n    let i = ENTRIES.binary_search(&pc).ok()?;\n    Some(native::<B>(OWNER[i]))\n}\n\n");
    lib.push_str("/// Dispatch entries (sorted) and the index in `FUNCTIONS` of the function that owns each.\n");
    let _ = writeln!(lib, "static ENTRIES: [u32; {}] = [", table.len());
    let addrs: Vec<String> = table.keys().map(|a| format!("0x{:x}", a)).collect();
    for c in addrs.chunks(16) {
        let _ = writeln!(lib, "    {},", c.join(", "));
    }
    lib.push_str("];\n");
    let _ = writeln!(lib, "static OWNER: [u16; {}] = [", table.len());
    let owners: Vec<String> = table.values().map(|f| format!("{}", index[f])).collect();
    for c in owners.chunks(32) {
        let _ = writeln!(lib, "    {},", c.join(", "));
    }
    lib.push_str("];\n\n");
    assert!(prog.funcs.len() <= u16::MAX as usize + 1, "too many functions for the u16 owner table");
    lib.push_str("/// The function with index `i` in `FUNCTIONS`.\n#[inline(never)]\nfn native<B: Bus>(i: u16) -> NativeFn<B> {\n    match i {\n");
    for (&e, &i) in &index {
        let _ = writeln!(lib, "        {} => {}::<B>,", i, fn_name(e));
    }
    lib.push_str("        _ => unreachable!(),\n    }\n}\n\n");
    lib.push_str("/// Calls the native code for `cpu.pc`; false if there is none.\n#[inline]\npub fn call<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> bool {\n    match lookup::<B>(cpu.pc) {\n        Some(f) => { f(cpu, bus); true }\n        None => false,\n    }\n}\n\n");
    lib.push_str("/// Function entry points.\npub const FUNCTIONS: &[u32] = &[");
    for e in prog.funcs.keys() {
        let _ = write!(lib, "0x{:x}, ", e);
    }
    lib.push_str("];\n");
    lib.push_str(extra);
    std::fs::write(dir.join("src/lib.rs"), lib)?;
    Ok(stats)
}
