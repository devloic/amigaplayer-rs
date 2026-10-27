//! Lowers decoded 68000 instructions to Rust source.
//!
//! The generated statements mirror `m68k::interp::exec` operation by operation (same EA side-effect
//! order, same bus accesses in the same order, same `alu` helpers), with every decode-time constant
//! folded: register numbers, sizes, displacements, PC-relative addresses, immediates and branch
//! targets become literals, and condition codes become plain boolean expressions.
//!
//! For [`Model::M68020`] it also emits the 68020 instructions and EAs the decoder produces in that
//! mode, and the 68020 forms of exceptions, RTE, MOVE from SR and MOVEC, called directly (the generated
//! code does not depend on `cpu.model`). The 68000 output is exactly what it was before the model
//! existed.
use m68k::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// How control transfers are emitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Every instruction is its own function and always returns to the caller (fuzz tests).
    Single,
    /// One Rust function per 68000 function, `loop { match pc { .. } }` over its basic blocks.
    /// `direct`: JSR/BSR to a known function call it as a Rust function and resume in place when
    /// it returns to the expected address; otherwise every call returns to the dispatcher.
    Func { direct: bool },
}

pub struct Ctx<'a> {
    pub mode: Mode,
    /// Block labels of the function being emitted.
    pub labels: &'a BTreeSet<u32>,
    /// Entry address -> Rust function name, for direct calls.
    pub funcs: &'a BTreeMap<u32, String>,
    /// Base register and its known value (small-data base) for guarded direct calls through
    /// `jsr d16(An)` into data-section stubs (Manx/Aztec C: a4).
    pub stub_base: Option<(u8, u32)>,
    /// Statically resolved targets of `jsr d16(a4)` stubs: stub address -> final target.
    pub stubs: &'a BTreeMap<u32, u32>,
    /// Cycle counting ([`crate::Options::cycles`]).
    pub cycles: bool,
    /// The CPU the code is emitted for ([`crate::Options::model`]).
    pub model: Model,
}

impl Ctx<'_> {
    /// The budget test of a block start (and of guarded flags, which must agree with it).
    pub fn budget_test(&self) -> &'static str {
        if self.cycles { "(cpu.ticks >= cpu.budget || cpu.cycles >= cpu.cycle_budget)" } else { "cpu.ticks >= cpu.budget" }
    }
    /// Block entry: `n` instructions (their cycles are counted one by one, `charge`).
    pub fn count(&self, n: usize, _c: u32) -> String {
        format!("cpu.ticks += {};", n)
    }
    /// Before an instruction: its cycles, and the new total for the bus (`Bus::cycles`), so
    /// that a bus access sees the time the instruction ends at, as with the interpreter.
    pub fn charge(&self, ins: &m68k::Instr) -> String {
        if self.cycles {
            format!("        cpu.cycles += {}; bus.cycles(cpu.cycles);\n", m68k::cycles::cycles(ins))
        } else {
            String::new()
        }
    }
}

/// Cycles of a block ([`m68k::cycles`]).
pub fn block_cycles<'a>(insns: impl IntoIterator<Item = &'a m68k::Instr>) -> u32 {
    insns.into_iter().map(m68k::cycles::cycles).sum()
}

fn hex(v: u32) -> String {
    format!("0x{:x}", v)
}

/// A resolved operand, as Rust expressions.
#[derive(Clone)]
enum L {
    D(u8),
    A(u8),
    I(u32),
    /// Memory at the address held in this local variable.
    M(String),
}

struct E<'a, 'b> {
    o: &'a mut String,
    cx: &'a Ctx<'b>,
    nvar: u32,
    /// Instructions after this one in the block (already counted in `cpu.ticks` at block entry).
    rest: u32,
}

fn step(sz: Sz, r: u8) -> u32 {
    if sz == Sz::B && r == 7 { 2 } else { sz.bytes() }
}

fn xn(x: Xn) -> String {
    let r = if x.reg < 8 { format!("cpu.d[{}]", x.reg) } else { format!("cpu.a[{}]", x.reg - 8) };
    if x.long { r } else { format!("({} as i16 as i32 as u32)", r) }
}

/// Address expression of a 68020 indexed EA: `interp::full_addr` with every constant folded.
fn full(f: &FullEa) -> String {
    let base = match f.base {
        Some(r) => add_const(&format!("cpu.a[{}]", r), f.bd as i32),
        None => format!("{}u32", hex(f.bd)),
    };
    let index = f.index.map(|(x, s)| if s == 0 { xn(x) } else { format!("({} << {})", xn(x), s) });
    let plus = |e: String, i: &Option<String>| match i {
        Some(i) => format!("{}.wrapping_add({})", e, i),
        None => e,
    };
    match f.mem {
        MemInd::None => plus(base, &index),
        MemInd::Pre(od) => add_const(&format!("bus.read32({})", plus(base, &index)), od as i32),
        MemInd::Post(od) => add_const(&plus(format!("bus.read32({})", base), &index), od as i32),
    }
}

fn add_const(base: &str, d: i32) -> String {
    if d == 0 { base.to_string() } else { format!("{}.wrapping_add({})", base, hex(d as u32)) }
}

impl<'a, 'b> E<'a, 'b> {
    fn line(&mut self, s: &str) {
        self.o.push_str("        ");
        self.o.push_str(s);
        self.o.push('\n');
    }
    fn var(&mut self, p: &str) -> String {
        self.nvar += 1;
        format!("{}{}", p, self.nvar)
    }
    /// Address expression of a control EA (no side effects).
    fn addr(&self, ea: &Ea) -> String {
        match *ea {
            Ea::Ind(r) => format!("cpu.a[{}]", r),
            Ea::Disp(r, d) => add_const(&format!("cpu.a[{}]", r), d as i32),
            Ea::Idx(r, d, x) => format!("{}.wrapping_add({})", add_const(&format!("cpu.a[{}]", r), d as i32), xn(x)),
            Ea::AbsW(a) | Ea::AbsL(a) | Ea::PcDisp(a) => hex(a),
            Ea::PcIdx(base, x) => format!("{}u32.wrapping_add({})", hex(base), xn(x)),
            Ea::Full(f) => full(&f),
            _ => panic!("addr of {:?}", ea),
        }
    }
    fn resolve(&mut self, ea: &Ea, sz: Sz) -> L {
        match *ea {
            Ea::D(r) => L::D(r),
            Ea::A(r) => L::A(r),
            Ea::Imm(v) => L::I(v),
            Ea::Post(r) => {
                let v = self.var("ea");
                self.line(&format!("let {} = cpu.a[{}]; cpu.a[{}] = {}.wrapping_add({});", v, r, r, v, step(sz, r)));
                L::M(v)
            }
            Ea::Pre(r) => {
                let v = self.var("ea");
                self.line(&format!("let {} = cpu.a[{}].wrapping_sub({}); cpu.a[{}] = {};", v, r, step(sz, r), r, v));
                L::M(v)
            }
            _ => {
                let v = self.var("ea");
                let a = self.addr(ea);
                self.line(&format!("let {}: u32 = {};", v, a));
                L::M(v)
            }
        }
    }
    fn read(&self, l: &L, sz: Sz) -> String {
        let m = |e: String| if sz == Sz::L { e } else { format!("({} & {})", e, hex(sz.mask())) };
        match l {
            L::D(r) => m(format!("cpu.d[{}]", r)),
            L::A(r) => m(format!("cpu.a[{}]", r)),
            L::I(v) => format!("{}u32", hex(v & sz.mask())),
            L::M(a) => match sz {
                Sz::B => format!("(bus.read8({}) as u32)", a),
                Sz::W => format!("(bus.read16({}) as u32)", a),
                Sz::L => format!("bus.read32({})", a),
            },
        }
    }
    fn write(&mut self, l: &L, sz: Sz, v: &str) {
        let s = match l {
            L::D(r) => match sz {
                Sz::L => format!("cpu.d[{}] = {};", r, v),
                _ => format!("cpu.d[{r}] = (cpu.d[{r}] & !{m}) | ({v} & {m});", r = r, m = hex(sz.mask()), v = v),
            },
            L::A(r) => format!("cpu.a[{}] = {};", r, v),
            L::I(_) => panic!("write to immediate"),
            L::M(a) => match sz {
                Sz::B => format!("bus.write8({}, {} as u8);", a, v),
                Sz::W => format!("bus.write16({}, {} as u16);", a, v),
                Sz::L => format!("bus.write32({}, {});", a, v),
            },
        };
        self.line(&s);
    }
    /// Read-into-local helper.
    fn load(&mut self, l: &L, sz: Sz, name: &str) -> String {
        let e = self.read(l, sz);
        let v = self.var(name);
        self.line(&format!("let {} = {};", v, e));
        v
    }

    // ---- control flow -------------------------------------------------------------------------
    fn goto(&mut self, t: u32) {
        match self.cx.mode {
            Mode::Func { .. } if self.cx.labels.contains(&t) => self.line(&format!("pc = {}; continue;", hex(t))),
            _ => self.line(&format!("cpu.pc = {}; return;", hex(t))),
        }
    }
    /// Jump to a run-time address held in `v`.
    fn goto_dyn(&mut self, v: &str) {
        match self.cx.mode {
            Mode::Func { .. } => self.line(&format!("pc = {}; continue;", v)),
            Mode::Single => self.line(&format!("cpu.pc = {}; return;", v)),
        }
    }
    fn leave(&mut self) {
        self.line("return;");
    }
    fn fix(&self) -> String {
        if self.rest > 0 { format!("cpu.ticks -= {}; ", self.rest) } else { String::new() }
    }
    /// `cpu.exception`, or its 68020 form.
    fn exc(&self) -> &'static str {
        if self.cx.model == Model::M68020 { "exception_020" } else { "exception" }
    }
    fn exception(&mut self, vector: u32, pc: u32) {
        let f = self.fix();
        let x = self.exc();
        self.line(&format!("{}cpu.{}(bus, {}, {}); return;", f, x, vector, hex(pc)));
    }
    fn privileged(&mut self, pc: u32) {
        let f = self.fix();
        let x = self.exc();
        self.line(&format!("if !cpu.s {{ {}cpu.{}(bus, 8, {}); return; }}", f, x, hex(pc)));
    }
    /// The statement raising a zero divide / CHK / TRAPV exception after the instruction at `pc`.
    fn trap(&self, vector: u32, pc: u32, next: u32) -> String {
        let f = self.fix();
        if self.cx.model == Model::M68020 {
            format!("{}cpu.trap_020(bus, {}, {}, {}); return;", f, vector, hex(next), hex(pc))
        } else {
            format!("{}cpu.exception(bus, {}, {}); return;", f, vector, hex(next))
        }
    }
    /// After the return address has been pushed and `cpu.pc` set to the callee.
    fn call_static(&mut self, target: u32, next: u32) {
        match self.cx.mode {
            Mode::Func { direct: true } if self.cx.funcs.contains_key(&target) && self.cx.labels.contains(&next) => {
                let f = &self.cx.funcs[&target];
                self.line(&format!("{}(cpu, bus);", f));
                self.line(&format!("if cpu.pc != {} {{ return; }}", hex(next)));
                self.line(&format!("pc = {}; continue;", hex(next)));
            }
            _ => self.leave(),
        }
    }
    fn call_dynamic(&mut self, next: u32) {
        match self.cx.mode {
            Mode::Func { direct: true } if self.cx.labels.contains(&next) => {
                self.line(&format!("if !crate::call(cpu, bus) || cpu.pc != {} {{ return; }}", hex(next)));
                self.line(&format!("pc = {}; continue;", hex(next)));
            }
            _ => self.leave(),
        }
    }
}

pub fn cond_expr(c: Cond) -> &'static str {
    match c {
        Cond::T => "true",
        Cond::F => "false",
        Cond::Hi => "(!cpu.c && !cpu.z)",
        Cond::Ls => "(cpu.c || cpu.z)",
        Cond::Cc => "!cpu.c",
        Cond::Cs => "cpu.c",
        Cond::Ne => "!cpu.z",
        Cond::Eq => "cpu.z",
        Cond::Vc => "!cpu.v",
        Cond::Vs => "cpu.v",
        Cond::Pl => "!cpu.n",
        Cond::Mi => "cpu.n",
        Cond::Ge => "(cpu.n == cpu.v)",
        Cond::Lt => "(cpu.n != cpu.v)",
        Cond::Gt => "(cpu.n == cpu.v && !cpu.z)",
        Cond::Le => "(cpu.z || cpu.n != cpu.v)",
    }
}

fn shift_kind(k: ShiftKind) -> &'static str {
    match k {
        ShiftKind::As => "ShiftKind::As",
        ShiftKind::Ls => "ShiftKind::Ls",
        ShiftKind::Rox => "ShiftKind::Rox",
        ShiftKind::Ro => "ShiftKind::Ro",
    }
}

/// True if control never continues to the next instruction in sequence.
pub fn is_terminator(op: &Op) -> bool {
    matches!(op, Op::Bra { .. } | Op::Jmp { .. } | Op::Rts | Op::Rte | Op::Rtr | Op::Illegal | Op::LineA | Op::LineF | Op::Trap { .. } | Op::Stop { .. })
}

/// Emits one instruction. Returns true if execution may fall through to `ins.next()`
/// (the caller then emits the fall-through jump if the block ends here).
pub fn emit_instr(out: &mut String, ins: &Instr, cx: &Ctx, rest: u32, live: u8) -> bool {
    let mut e = E { o: out, cx, nvar: 0, rest };
    let (defs, _) = flag_effects(&ins.op);
    // Dead-flag elimination: none of the flags this instruction writes is read later.
    let dead = defs != 0 && live & defs == 0;
    let pc = ins.pc;
    let next = ins.next();
    let _ = writeln!(e.o, "        // {:06x}: {}", pc, ins);
    e.o.push_str(&cx.charge(ins));
    e.line("{");
    let mut falls = true;
    match ins.op {
        Op::Alu { op, sz, src, dst } => {
            let sl = e.resolve(&src, sz);
            let s = e.load(&sl, sz, "s");
            let dl = e.resolve(&dst, sz);
            let d = e.load(&dl, sz, "d");
            let r = match (op, dead) {
                (AluOp::Or, false) => format!("alu::logic(cpu, {}, {} | {})", sz.rust(), d, s),
                (AluOp::And, false) => format!("alu::logic(cpu, {}, {} & {})", sz.rust(), d, s),
                (AluOp::Eor, false) => format!("alu::logic(cpu, {}, {} ^ {})", sz.rust(), d, s),
                (AluOp::Add, false) => format!("alu::add(cpu, {}, {}, {})", sz.rust(), s, d),
                (AluOp::Sub, false) => format!("alu::sub(cpu, {}, {}, {})", sz.rust(), s, d),
                (AluOp::Cmp, false) => format!("alu::cmp(cpu, {}, {}, {})", sz.rust(), s, d),
                (AluOp::Or, true) => format!("{} | {}", d, s),
                (AluOp::And, true) => format!("{} & {}", d, s),
                (AluOp::Eor, true) => format!("{} ^ {}", d, s),
                (AluOp::Add, true) => format!("{}.wrapping_add({})", d, s),
                (AluOp::Sub, true) => format!("{}.wrapping_sub({})", d, s),
                (AluOp::Cmp, true) => String::new(),
            };
            if op == AluOp::Cmp {
                if !dead {
                    e.line(&format!("{};", r));
                }
            } else {
                e.line(&format!("let r = {};", r));
                e.write(&dl, sz, "r");
            }
        }
        Op::AddA { sz, src, an } | Op::SubA { sz, src, an } | Op::CmpA { sz, src, an } => {
            let sl = e.resolve(&src, sz);
            let raw = e.read(&sl, sz);
            let s = if sz == Sz::W { format!("({} as u16 as i16 as i32 as u32)", raw) } else { raw };
            e.line(&format!("let s = {};", s));
            match ins.op {
                Op::AddA { .. } => e.line(&format!("cpu.a[{a}] = cpu.a[{a}].wrapping_add(s);", a = an)),
                Op::SubA { .. } => e.line(&format!("cpu.a[{a}] = cpu.a[{a}].wrapping_sub(s);", a = an)),
                _ => e.line(&format!("let d = cpu.a[{}]; alu::cmp(cpu, Sz::L, s, d);", an)),
            }
        }
        Op::LogicSr { op, imm, sr } => {
            let o = match op {
                AluOp::Or => "|",
                AluOp::And => "&",
                _ => "^",
            };
            if sr {
                e.privileged(pc);
                e.line(&format!("let v = cpu.sr() {} {}; cpu.set_sr(v);", o, hex(imm as u32)));
            } else {
                e.line(&format!("let v = cpu.ccr() {} {}; cpu.set_ccr(v);", o, hex(imm as u32 & 0xFF)));
            }
        }
        Op::Bit { op, num, dst } => {
            let n = match num {
                BitNum::Imm(n) => hex(n as u32),
                BitNum::Reg(r) => format!("cpu.d[{}]", r),
            };
            let (l, sz, w) = if let Ea::D(r) = dst { (L::D(r), Sz::L, 31) } else { (e.resolve(&dst, Sz::B), Sz::B, 7) };
            e.line(&format!("let m = 1u32 << ({} & {});", n, w));
            let v = e.load(&l, sz, "v");
            e.line(&format!("cpu.z = {} & m == 0;", v));
            match op {
                BitOp::Tst => {}
                BitOp::Chg => e.write(&l, sz, &format!("({} ^ m)", v)),
                BitOp::Clr => e.write(&l, sz, &format!("({} & !m)", v)),
                BitOp::Set => e.write(&l, sz, &format!("({} | m)", v)),
            }
        }
        Op::Movep { sz, dn, an, disp, to_mem } => {
            e.line(&format!("let ea: u32 = {};", add_const(&format!("cpu.a[{}]", an), disp as i32)));
            let n = sz.bytes();
            if to_mem {
                e.line(&format!("let v = cpu.d[{}];", dn));
                for i in 0..n {
                    e.line(&format!("bus.write8(ea.wrapping_add({}), (v >> {}) as u8);", 2 * i, 8 * (n - 1 - i)));
                }
            } else {
                let parts: Vec<String> = (0..n).map(|i| format!("((bus.read8(ea.wrapping_add({})) as u32) << {})", 2 * i, 8 * (n - 1 - i))).collect();
                e.line(&format!("let v = {};", parts.join(" | ")));
                e.write(&L::D(dn), sz, "v");
            }
        }
        Op::Move { sz, src, dst } => {
            let sl = e.resolve(&src, sz);
            let v = e.load(&sl, sz, "v");
            let dl = e.resolve(&dst, sz);
            if !dead {
                e.line(&format!("alu::logic(cpu, {}, {});", sz.rust(), v));
            }
            match (sz, dst, &dl) {
                (Sz::L, Ea::Pre(_), L::M(a)) => e.line(&format!("m68k::interp::write_pd32(bus, {}, {});", a, v)),
                _ => e.write(&dl, sz, &v),
            }
        }
        Op::MoveA { sz, src, an } => {
            let sl = e.resolve(&src, sz);
            let raw = e.read(&sl, sz);
            let s = if sz == Sz::W { format!("({} as u16 as i16 as i32 as u32)", raw) } else { raw };
            e.line(&format!("cpu.a[{}] = {};", an, s));
        }
        Op::Negx { sz, ea } | Op::Neg { sz, ea } | Op::Not { sz, ea } => {
            let l = e.resolve(&ea, sz);
            let v = e.load(&l, sz, "v");
            let r = match ins.op {
                Op::Negx { .. } => format!("alu::negx(cpu, {}, {})", sz.rust(), v),
                Op::Neg { .. } => format!("alu::neg(cpu, {}, {})", sz.rust(), v),
                _ => format!("alu::logic(cpu, {}, !{})", sz.rust(), v),
            };
            e.line(&format!("let r = {};", r));
            e.write(&l, sz, "r");
        }
        Op::Clr { sz, ea } => {
            let l = e.resolve(&ea, sz);
            if !dead {
                e.line(&format!("alu::logic(cpu, {}, 0);", sz.rust()));
            }
            e.write(&l, sz, "0u32");
        }
        Op::Tst { sz, ea } => {
            let l = e.resolve(&ea, sz);
            let v = e.load(&l, sz, "v");
            if !dead {
                e.line(&format!("alu::logic(cpu, {}, {});", sz.rust(), v));
            }
        }
        Op::MoveFromSr { ea } => {
            if cx.model == Model::M68020 {
                e.privileged(pc);
            }
            let l = e.resolve(&ea, Sz::W);
            e.line("let v = cpu.sr() as u32;");
            e.write(&l, Sz::W, "v");
        }
        Op::MoveToCcr { ea } => {
            let l = e.resolve(&ea, Sz::W);
            let v = e.read(&l, Sz::W);
            e.line(&format!("cpu.set_ccr({} as u8);", v));
        }
        Op::MoveToSr { ea } => {
            e.privileged(pc);
            let l = e.resolve(&ea, Sz::W);
            let v = e.read(&l, Sz::W);
            e.line(&format!("cpu.set_sr({} as u16);", v));
        }
        Op::Chk { src, dn } => {
            let l = e.resolve(&src, Sz::W);
            let b = e.load(&l, Sz::W, "b");
            let t = e.trap(6, pc, next);
            let f = if cx.model == Model::M68020 { "chk_020" } else { "chk" };
            e.line(&format!("let dn = cpu.d[{}]; if alu::{}(cpu, dn, {}) {{ {} }}", dn, f, b, t));
        }
        Op::Lea { src, an } => {
            let a = e.addr(&src);
            e.line(&format!("cpu.a[{}] = {};", an, a));
        }
        Op::Pea { src } => {
            let a = e.addr(&src);
            e.line(&format!("let v = {}; cpu.push32(bus, v);", a));
        }
        Op::Nbcd { ea } => {
            let l = e.resolve(&ea, Sz::B);
            let v = e.load(&l, Sz::B, "v");
            if cx.model == Model::M68020 {
                e.line(&format!("let r = alu::nbcd_020(cpu, {});", v));
                e.write(&l, Sz::B, "r");
            } else {
                e.line(&format!("if let Some(r) = alu::nbcd(cpu, {}) {{", v));
                e.write(&l, Sz::B, "r");
                e.line("}");
            }
        }
        Op::Swap { dn } => e.line(&format!("let v = cpu.d[{d}].rotate_left(16); cpu.d[{d}] = alu::logic(cpu, Sz::L, v);", d = dn)),
        Op::Ext { sz, dn } => {
            if sz == Sz::W {
                e.line(&format!("let r = (cpu.d[{}] as u8 as i8 as i16 as u16) as u32;", dn));
                e.line("alu::logic(cpu, Sz::W, r);");
                e.write(&L::D(dn), Sz::W, "r");
            } else {
                e.line(&format!("let v = cpu.d[{d}] as u16 as i16 as i32 as u32; cpu.d[{d}] = alu::logic(cpu, Sz::L, v);", d = dn));
            }
        }
        Op::Movem { sz, to_mem, mask, ea } => {
            let n = sz.bytes();
            let reg = |i: u16| if i < 8 { format!("cpu.d[{}]", i) } else { format!("cpu.a[{}]", i - 8) };
            if to_mem {
                if let Ea::Pre(r) = ea {
                    e.line(&format!("let mut a = cpu.a[{}];", r));
                    for i in 0..16u16 {
                        if mask & (1 << i) != 0 {
                            e.line(&format!("a = a.wrapping_sub({});", n));
                            let mut v = reg(15 - i);
                            // The 68020 stores the base register as decremented by one transfer.
                            if cx.model == Model::M68020 && 15 - i == 8 + r as u16 {
                                v = format!("{}.wrapping_sub({})", v, n);
                            }
                            if sz == Sz::W {
                                e.line(&format!("bus.write16(a, {} as u16);", v));
                            } else {
                                e.line(&format!("m68k::interp::write_pd32(bus, a, {});", v));
                            }
                        }
                    }
                    e.line(&format!("cpu.a[{}] = a;", r));
                } else {
                    let a0 = e.addr(&ea);
                    e.line(&format!("let mut a: u32 = {};", a0));
                    for i in 0..16u16 {
                        if mask & (1 << i) != 0 {
                            let v = reg(i);
                            if sz == Sz::W {
                                e.line(&format!("bus.write16(a, {} as u16);", v));
                            } else {
                                e.line(&format!("bus.write32(a, {});", v));
                            }
                            e.line(&format!("a = a.wrapping_add({});", n));
                        }
                    }
                    e.line("let _ = a;");
                }
            } else {
                let (a0, post) = match ea {
                    Ea::Post(r) => (format!("cpu.a[{}]", r), Some(r)),
                    _ => (e.addr(&ea), None),
                };
                e.line(&format!("let mut a: u32 = {};", a0));
                for i in 0..16u16 {
                    if mask & (1 << i) != 0 {
                        let rd = if sz == Sz::W { "bus.read16(a) as i16 as i32 as u32".to_string() } else { "bus.read32(a)".to_string() };
                        e.line(&format!("{} = {};", reg(i), rd));
                        e.line(&format!("a = a.wrapping_add({});", n));
                    }
                }
                match post {
                    Some(r) => e.line(&format!("cpu.a[{}] = a;", r)),
                    None => e.line("let _ = a;"),
                }
            }
        }
        Op::Tas { ea } => {
            let l = e.resolve(&ea, Sz::B);
            let v = e.load(&l, Sz::B, "v");
            e.line(&format!("alu::logic(cpu, Sz::B, {});", v));
            e.write(&l, Sz::B, &format!("({} | 0x80)", v));
        }
        Op::Illegal => {
            e.exception(4, pc);
            falls = false;
        }
        Op::LineA => {
            e.exception(10, pc);
            falls = false;
        }
        Op::LineF => {
            e.exception(11, pc);
            falls = false;
        }
        Op::Trap { n } => {
            e.exception(32 + n as u32, next);
            falls = false;
        }
        Op::Link { an, disp } => {
            if an == 7 && cx.model == Model::M68020 {
                // The 68020 pushes A7 as it was.
                e.line("let v = cpu.a[7]; cpu.push32(bus, v);");
            } else if an == 7 {
                e.line("cpu.a[7] = cpu.a[7].wrapping_sub(4); bus.write32(cpu.a[7], cpu.a[7]);");
            } else {
                e.line(&format!("let v = cpu.a[{a}]; cpu.push32(bus, v); cpu.a[{a}] = cpu.a[7];", a = an));
            }
            e.line(&format!("cpu.a[7] = cpu.a[7].wrapping_add({});", hex(disp as i32 as u32)));
        }
        Op::Unlk { an } => {
            if an == 7 {
                e.line("cpu.a[7] = bus.read32(cpu.a[7]);");
            } else {
                e.line(&format!("cpu.a[7] = cpu.a[{a}]; cpu.a[{a}] = cpu.pop32(bus);", a = an));
            }
        }
        Op::MoveUsp { to_usp, an } => {
            e.privileged(pc);
            if to_usp {
                e.line(&format!("cpu.other_sp = cpu.a[{}];", an));
            } else {
                e.line(&format!("cpu.a[{}] = cpu.other_sp;", an));
            }
        }
        Op::Reset => e.privileged(pc),
        Op::Nop => {}
        Op::Stop { imm } => {
            e.privileged(pc);
            e.line(&format!("cpu.set_sr({}); cpu.stopped = true; cpu.pc = {}; return;", hex(imm as u32), hex(next)));
            falls = false;
        }
        Op::Rte => {
            e.privileged(pc);
            if cx.model == Model::M68020 {
                e.line(&format!("cpu.rte_020(bus, {}); return;", hex(pc)));
            } else {
                e.line("let sr = cpu.pop16(bus); cpu.pc = cpu.pop32(bus); cpu.set_sr(sr); return;");
            }
            falls = false;
        }
        Op::Rts => {
            e.line("cpu.pc = cpu.pop32(bus); return;");
            falls = false;
        }
        Op::Rtr => {
            e.line("let ccr = cpu.pop16(bus); cpu.set_ccr(ccr as u8); cpu.pc = cpu.pop32(bus); return;");
            falls = false;
        }
        Op::Trapv => {
            let t = e.trap(7, pc, next);
            e.line(&format!("if cpu.v {{ {} }}", t))
        }
        Op::Jsr { ea } => {
            let static_t = match ea {
                Ea::AbsW(a) | Ea::AbsL(a) | Ea::PcDisp(a) => Some(a),
                _ => None,
            };
            match (static_t, ea) {
                (Some(t), _) => {
                    e.line(&format!("cpu.push32(bus, {}); cpu.pc = {};", hex(next), hex(t)));
                    e.call_static(t, next);
                }
                (None, Ea::Disp(r, d)) if cx.stub_base.map_or(false, |(br, _)| br == r) && matches!(cx.mode, Mode::Func { direct: true }) => {
                    // Manx/Aztec C call through a jump stub in the data section: `jsr d16(a4)`.
                    let stub = cx.stub_base.unwrap().1.wrapping_add(d as i32 as u32);
                    let a = e.addr(&ea);
                    e.line(&format!("let t: u32 = {};", a));
                    e.line(&format!("cpu.push32(bus, {}); cpu.pc = t;", hex(next)));
                    let fin = cx.stubs.get(&stub).copied();
                    match fin.and_then(|f| cx.funcs.get(&f).map(|n| (f, n))) {
                        Some((f, name)) if cx.labels.contains(&next) => {
                            e.line(&format!("if t == {} {{ // stub -> {:06x}", hex(stub), f));
                            // +1: the stub's own `jmp` instruction, skipped here
                            e.line(&format!("    {} cpu.pc = {}; {}(cpu, bus);", if cx.cycles { "cpu.ticks += 1; cpu.cycles += 12; bus.cycles(cpu.cycles);" } else { "cpu.ticks += 1;" }, hex(f), name));
                            e.line(&format!("    if cpu.pc != {} {{ return; }}", hex(next)));
                            e.line(&format!("    pc = {}; continue;", hex(next)));
                            e.line("}");
                            e.call_dynamic(next);
                        }
                        _ => e.call_dynamic(next),
                    }
                }
                _ => {
                    let a = e.addr(&ea);
                    e.line(&format!("let t: u32 = {};", a));
                    e.line(&format!("cpu.push32(bus, {}); cpu.pc = t;", hex(next)));
                    e.call_dynamic(next);
                }
            }
            falls = false;
        }
        Op::Jmp { ea } => {
            match ea {
                Ea::AbsW(a) | Ea::AbsL(a) | Ea::PcDisp(a) => match cx.mode {
                    Mode::Func { direct: true } if !cx.labels.contains(&a) && cx.funcs.contains_key(&a) => {
                        e.line(&format!("cpu.pc = {}; {}(cpu, bus); return;", hex(a), cx.funcs[&a]));
                    }
                    _ => e.goto(a),
                },
                _ => {
                    let a = e.addr(&ea);
                    e.line(&format!("let t: u32 = {};", a));
                    e.goto_dyn("t");
                }
            }
            falls = false;
        }
        Op::Scc { cond, ea } => {
            let l = e.resolve(&ea, Sz::B);
            e.write(&l, Sz::B, &format!("(if {} {{ 0xFFu32 }} else {{ 0 }})", cond_expr(cond)));
        }
        Op::Dbcc { cond, dn, target } => {
            e.line(&format!("if !{} {{", cond_expr(cond)));
            e.line(&format!("    let w = (cpu.d[{d}] as u16).wrapping_sub(1); cpu.d[{d}] = (cpu.d[{d}] & 0xFFFF_0000) | w as u32;", d = dn));
            e.line("    if w != 0xFFFF {");
            e.goto(target);
            e.line("    }");
            e.line("}");
        }
        Op::Bra { target } => {
            e.goto(target);
            falls = false;
        }
        Op::Bsr { target } => {
            e.line(&format!("cpu.push32(bus, {}); cpu.pc = {};", hex(next), hex(target)));
            e.call_static(target, next);
            falls = false;
        }
        Op::Bcc { cond, target } => {
            e.line(&format!("if {} {{", cond_expr(cond)));
            e.goto(target);
            e.line("}");
        }
        Op::Moveq { data, dn } => {
            if dead {
                e.line(&format!("cpu.d[{}] = {};", dn, hex(data as i32 as u32)));
            } else {
                e.line(&format!("cpu.d[{}] = alu::logic(cpu, Sz::L, {});", dn, hex(data as i32 as u32)));
            }
        }
        Op::Divu { src, dn } | Op::Divs { src, dn } => {
            let l = e.resolve(&src, Sz::W);
            let s = e.load(&l, Sz::W, "s");
            let f = if matches!(ins.op, Op::Divu { .. }) { "divu" } else { "divs" };
            e.line(&format!("let dv = cpu.d[{}];", dn));
            e.line(&format!("match alu::{}(cpu, {}, dv) {{", f, s));
            e.line(&format!("    alu::Div::Ok(v) => cpu.d[{}] = v,", dn));
            e.line("    alu::Div::Overflow => {}");
            let t = e.trap(5, pc, next);
            e.line(&format!("    alu::Div::Zero => {{ {} }}", t));
            e.line("}");
        }
        Op::Mulu { src, dn } | Op::Muls { src, dn } => {
            let l = e.resolve(&src, Sz::W);
            let s = e.load(&l, Sz::W, "s");
            let f = if matches!(ins.op, Op::Mulu { .. }) { "mulu" } else { "muls" };
            e.line(&format!("let dv = cpu.d[{d}]; cpu.d[{d}] = alu::{f}(cpu, {s}, dv);", d = dn, f = f, s = s));
        }
        Op::Abcd { rx, ry, mem } | Op::Sbcd { rx, ry, mem } => {
            let f = match (matches!(ins.op, Op::Abcd { .. }), cx.model) {
                (true, Model::M68000) => "abcd",
                (false, Model::M68000) => "sbcd",
                (true, Model::M68020) => "abcd_020",
                (false, Model::M68020) => "sbcd_020",
            };
            if mem {
                let sl = e.resolve(&Ea::Pre(ry), Sz::B);
                let s = e.load(&sl, Sz::B, "s");
                let dl = e.resolve(&Ea::Pre(rx), Sz::B);
                let d = e.load(&dl, Sz::B, "d");
                e.line(&format!("let r = alu::{}(cpu, {}, {});", f, s, d));
                e.write(&dl, Sz::B, "r");
            } else {
                e.line(&format!("let (s, d) = (cpu.d[{}], cpu.d[{}]); let r = alu::{}(cpu, s, d);", ry, rx, f));
                e.write(&L::D(rx), Sz::B, "r");
            }
        }
        Op::Addx { sz, rx, ry, mem } | Op::Subx { sz, rx, ry, mem } => {
            let f = if matches!(ins.op, Op::Addx { .. }) { "addx" } else { "subx" };
            let (s, dl) = if mem {
                let sl = e.resolve(&Ea::Pre(ry), sz);
                let s = e.load(&sl, sz, "s");
                (s, e.resolve(&Ea::Pre(rx), sz))
            } else {
                e.line(&format!("let s0 = cpu.d[{}];", ry));
                ("s0".to_string(), L::D(rx))
            };
            let d = e.load(&dl, sz, "d");
            e.line(&format!("let r = alu::{}(cpu, {}, {}, {});", f, sz.rust(), s, d));
            e.write(&dl, sz, "r");
        }
        Op::Cmpm { sz, ax, ay } => {
            let sl = e.resolve(&Ea::Post(ay), sz);
            let s = e.load(&sl, sz, "s");
            let dl = e.resolve(&Ea::Post(ax), sz);
            let d = e.load(&dl, sz, "d");
            e.line(&format!("alu::cmp(cpu, {}, {}, {});", sz.rust(), s, d));
        }
        Op::Exg { rx, ry } => {
            let r = |i: u8| if i < 8 { format!("cpu.d[{}]", i) } else { format!("cpu.a[{}]", i - 8) };
            e.line(&format!("let t = {x}; {x} = {y}; {y} = t;", x = r(rx), y = r(ry)));
        }
        Op::ShiftReg { kind, left, sz, count, dn } => {
            let cnt = match count {
                ShiftCount::Imm(n) => format!("{}", n),
                ShiftCount::Reg(r) => format!("(cpu.d[{}] & 63)", r),
            };
            e.line(&format!("let (v, n) = (cpu.d[{}], {}); let r = alu::shift(cpu, {}, {}, {}, v, n);", dn, cnt, shift_kind(kind), left, sz.rust()));
            e.write(&L::D(dn), sz, "r");
        }
        Op::ShiftMem { kind, left, ea } => {
            let l = e.resolve(&ea, Sz::W);
            let v = e.load(&l, Sz::W, "v");
            e.line(&format!("let r = alu::shift(cpu, {}, {}, Sz::W, {}, 1);", shift_kind(kind), left, v));
            e.write(&l, Sz::W, "r");
        }
        Op::MoveFromCcr { ea } => {
            let l = e.resolve(&ea, Sz::W);
            e.line("let v = cpu.ccr() as u32;");
            e.write(&l, Sz::W, "v");
        }
        Op::Movec { to_ctrl, reg, ctrl } => {
            // As `interp::exec`: the privilege check, then the code. An unknown code raises an
            // illegal instruction exception; the test is a constant, written out so that the
            // instruction still looks as if it fell through (it is not a block terminator).
            e.privileged(pc);
            let r = if reg < 8 { format!("cpu.d[{}]", reg) } else { format!("cpu.a[{}]", reg - 8) };
            if !Cpu::movec_legal(ctrl) {
                let f = e.fix();
                let x = e.exc();
                e.line(&format!("if !m68k::Cpu::movec_legal({c}) {{ {f}cpu.{x}(bus, 4, {p}); return; }}", c = hex(ctrl as u32), f = f, x = x, p = hex(pc)));
            } else if to_ctrl {
                e.line(&format!("let v = {}; cpu.movec_write({}, v);", r, hex(ctrl as u32)));
            } else {
                e.line(&format!("{} = cpu.movec_read({});", r, hex(ctrl as u32)));
            }
        }
        Op::Extb { dn } => {
            if dead {
                e.line(&format!("cpu.d[{d}] = cpu.d[{d}] as u8 as i8 as i32 as u32;", d = dn));
            } else {
                e.line(&format!("let v = cpu.d[{d}] as u8 as i8 as i32 as u32; cpu.d[{d}] = alu::logic(cpu, Sz::L, v);", d = dn));
            }
        }
        Op::MulL { signed, src, dl, dh } => {
            let l = e.resolve(&src, Sz::L);
            let s = e.load(&l, Sz::L, "s");
            e.line(&format!("let (hi, lo) = alu::mull(cpu, {}, {}, {}, cpu.d[{}]);", signed, dh.is_some(), s, dl));
            // Dl first, then Dh (the 68020 order: Dh wins if they are the same register).
            e.line(&format!("cpu.d[{}] = lo;", dl));
            if let Some(h) = dh {
                e.line(&format!("cpu.d[{}] = hi;", h));
            }
        }
        Op::DivL { signed, src, dq, dr, wide } => {
            let l = e.resolve(&src, Sz::L);
            let s = e.load(&l, Sz::L, "s");
            let hi = if wide { format!("Some(cpu.d[{}])", dr) } else { "None".to_string() };
            e.line(&format!("match alu::divl(cpu, {}, {}, {}, cpu.d[{}]) {{", signed, s, hi, dq));
            e.line(&format!("    alu::DivL::Ok(q, r) => {{ cpu.d[{}] = r; cpu.d[{}] = q; }}", dr, dq));
            e.line("    alu::DivL::Overflow => {}");
            let t = e.trap(5, pc, next);
            e.line(&format!("    alu::DivL::Zero => {{ {} }}", t));
            e.line("}");
        }
    }
    e.line("}");
    falls
}

const ALL: u8 = 0x1F;

/// (flags written, flags read) of an instruction, for dead-flag elimination. Instructions that can
/// trap read every flag (the exception frame stacks SR); partial writers (Z only cleared by ADDX,
/// X untouched by a register shift of 0) are counted as not writing that flag.
pub fn flag_effects(op: &Op) -> (u8, u8) {
    match *op {
        Op::Alu { op: AluOp::Add | AluOp::Sub, .. } => (ALL, 0),
        Op::Alu { .. } => (0x0F, 0),
        Op::AddA { .. } | Op::SubA { .. } | Op::MoveA { .. } | Op::Lea { .. } | Op::Pea { .. } | Op::Movep { .. }
        | Op::Movem { .. } | Op::Exg { .. } | Op::Link { .. } | Op::Unlk { .. } | Op::Nop => (0, 0),
        Op::CmpA { .. } | Op::Cmpm { .. } | Op::Move { .. } | Op::Clr { .. } | Op::Not { .. } | Op::Tst { .. }
        | Op::Swap { .. } | Op::Ext { .. } | Op::Moveq { .. } | Op::Tas { .. } | Op::Mulu { .. } | Op::Muls { .. }
        | Op::Extb { .. } | Op::MulL { .. } => (0x0F, 0),
        Op::Neg { .. } => (ALL, 0),
        Op::Negx { .. } | Op::Addx { .. } | Op::Subx { .. } | Op::Abcd { .. } | Op::Sbcd { .. } | Op::Nbcd { .. } => (0x1B, 0x14),
        Op::Bit { .. } => (0x04, 0),
        Op::MoveToCcr { .. } => (ALL, 0),
        Op::ShiftReg { kind, count, .. } => {
            let imm = matches!(count, ShiftCount::Imm(_));
            match kind {
                ShiftKind::Ro => (0x0F, 0),
                ShiftKind::Rox => (if imm { ALL } else { 0x0F }, 0x10),
                _ => (if imm { ALL } else { 0x0F }, 0),
            }
        }
        Op::ShiftMem { kind, .. } => match kind {
            ShiftKind::Ro => (0x0F, 0),
            ShiftKind::Rox => (ALL, 0x10),
            _ => (ALL, 0),
        },
        // Everything else reads all flags (conditions, SR moves, traps, privileged, control flow).
        _ => (0, ALL),
    }
}

/// Flags live after each instruction of a block (all flags live at the block end).
pub fn flag_liveness(block: &[Instr]) -> Vec<u8> {
    let mut live = ALL;
    let mut out = vec![0; block.len()];
    for (i, ins) in block.iter().enumerate().rev() {
        out[i] = live;
        let (d, u) = flag_effects(&ins.op);
        live = (live & !d) | u;
    }
    out
}
