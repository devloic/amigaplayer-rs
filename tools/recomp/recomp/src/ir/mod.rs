//! Intermediate representation between decode and emit (see `crates/recomp/IR.md`).
//!
//! * [`lift`]: one decoded instruction -> [`IrInsn`]: micro-ops on registers, memory, flags and
//!   the status register ([`IrOp`]) with explicit, *lazy* flag definitions ([`Lazy`]), and for
//!   control-flow instructions a [`Term`]inator. Every 68000 instruction is lifted.
//! * [`cfg`]: functions as basic blocks with typed edges (fall-through, branch, jump table, call
//!   + resume, dynamic jump/call, return, trap), entry aliases (every block start that is a
//!   dispatch entry), and flag liveness across the blocks of a function.
//! * [`emit`]: Rust from the IR, with the same crate layout (parts, lookup, FUNCTIONS) as
//!   [`crate::emit_crate`]'s; large functions are split into parts.
//! * [`eval`]: a direct evaluator of the micro-ops (differential tests against the interpreter).
//! * [`annot`]: the annotation layer (names, roles, signatures, struct layouts, context tags,
//!   functions keyed by section + offset) and loaders for the mapping drafts.
pub mod annot;
pub mod cfg;
pub mod emit;
pub mod eval;
pub mod json;
pub mod lift;

pub use cfg::{Block, Edge, IrFunc, IrProgram};
pub use lift::lift;

use m68k::{Cond, Ea, Instr, ShiftKind, Sz, Xn};

/// An operand of a micro-op: a temporary of the instruction or a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V {
    T(u32),
    C(u32),
}

/// Side-effect-free address expressions (the control addressing modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrExpr {
    /// `An + d`
    Disp(u8, i32),
    /// `An + d + Xn`
    Idx(u8, i32, Xn),
    /// An absolute or PC-relative address resolved at decode time.
    Abs(u32),
    /// `base + Xn` (PC-relative indexed, base resolved at decode time).
    PcIdx(u32, Xn),
}

/// Two-operand arithmetic with flags (`d op s`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArOp {
    Add,
    Sub,
    And,
    Or,
    Eor,
    Cmp,
}

/// One-operand arithmetic with lazy flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    /// NEG: `0 - v`
    Neg,
    /// NOT: `!v`
    Not,
}

/// Arithmetic that reads X (and only clears Z): always computed with its flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XOp {
    Addx,
    Subx,
    /// NEGX: `0 - s - X` (`d` unused)
    Negx,
    Abcd,
    Sbcd,
}

/// Flag-free 32-bit operations (`t = a op b`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    And,
    Or,
    Xor,
    /// `a & !b`
    AndNot,
    Add,
    Sub,
    /// `a << b` (b < 32)
    Shl,
    /// `a >> b` (b < 32)
    Shr,
    /// `a.rotate_left(b)`
    Rotl,
}

impl BinOp {
    pub fn apply(self, a: u32, b: u32) -> u32 {
        match self {
            BinOp::And => a & b,
            BinOp::Or => a | b,
            BinOp::Xor => a ^ b,
            BinOp::AndNot => a & !b,
            BinOp::Add => a.wrapping_add(b),
            BinOp::Sub => a.wrapping_sub(b),
            BinOp::Shl => a << b,
            BinOp::Shr => a >> b,
            BinOp::Rotl => a.rotate_left(b),
        }
    }
}

/// A byte destination (NBCD writes it only on some results).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dst {
    D(u8),
    M(V),
}

/// Flag bits as used by the liveness masks.
pub const FX: u8 = 0x10;
pub const FN: u8 = 0x08;
pub const FZ: u8 = 0x04;
pub const FV: u8 = 0x02;
pub const FC: u8 = 0x01;
pub const ALL: u8 = 0x1F;

/// Flags a condition reads.
pub fn cond_flags(c: Cond) -> u8 {
    match c {
        Cond::T | Cond::F => 0,
        Cond::Hi | Cond::Ls => FC | FZ,
        Cond::Cc | Cond::Cs => FC,
        Cond::Ne | Cond::Eq => FZ,
        Cond::Vc | Cond::Vs => FV,
        Cond::Pl | Cond::Mi => FN,
        Cond::Ge | Cond::Lt => FN | FV,
        Cond::Gt | Cond::Le => FN | FV | FZ,
    }
}

/// A lazy flag definition: the flags `defs` (bits X=0x10 N=8 Z=4 V=2 C=1) an operation may
/// write, which of them are read later (`live`), and which of them would be visible if the block
/// were left at its end (`exit`). Both masks are filled in by the CFG's liveness pass (all flags
/// until then).
///
/// * `defs & live != 0`: the flags are materialised (through the shared `m68k::alu` helpers).
/// * otherwise, `defs & exit != 0`: the flags are dead along every path that stays in native
///   code, but the next block start is a dispatcher exit when the instruction budget runs out
///   (and every SMC-guarded or block-stepped run ends there), so they are materialised only if
///   `cpu.ticks >= cpu.budget` -- the same test the next block's budget check makes, with the
///   same values, since nothing between changes either. The state the dispatcher, an interrupt
///   or the interpreter sees is therefore always exact.
/// * otherwise the operation is plain arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lazy {
    pub defs: u8,
    pub live: u8,
    pub exit: u8,
}

impl Lazy {
    pub fn new(defs: u8) -> Lazy {
        Lazy { defs, live: ALL, exit: ALL }
    }
    /// The flags are always computed.
    pub fn needed(&self) -> bool {
        self.defs & self.live != 0
    }
    /// The flags are computed only when the budget is exhausted.
    pub fn guarded(&self) -> bool {
        !self.needed() && self.defs & self.exit != 0
    }
}

/// Lifted micro-operations, executed in order. Temporaries are numbered per instruction and are
/// assigned once (SSA within the instruction).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrOp {
    /// `t = address`
    Addr { t: u32, ea: AddrExpr },
    /// `(An)+`: `t = An; An += step`
    PostInc { t: u32, an: u8, step: u32 },
    /// `-(An)`: `An -= step; t = An`
    PreDec { t: u32, an: u8, step: u32 },
    /// `t = Dn` masked to `sz`
    GetD { t: u32, r: u8, sz: Sz },
    /// `t = An` masked to `sz`
    GetA { t: u32, r: u8, sz: Sz },
    /// `t = memory[addr]` (zero-extended)
    Load { t: u32, sz: Sz, addr: V },
    /// `t = sign_extend16(v)`
    SextW { t: u32, v: V },
    /// `t = sign_extend8(v)`
    SextB { t: u32, v: V },
    /// `t = a op b` (no flags)
    Bin { t: u32, op: BinOp, a: V, b: V },
    /// `t = d op s` (no result for CMP), with lazy flags.
    Arith { t: Option<u32>, op: ArOp, sz: Sz, s: V, d: V, flags: Lazy },
    /// MOVE/TST/CLR/MOVEQ/SWAP/EXT/TAS flags on `v`: N, Z from it, V = C = 0 (lazy).
    LogicFlags { sz: Sz, v: V, flags: Lazy },
    /// NEG/NOT with lazy flags. The result is exact in its low `sz` bits.
    Unary { t: u32, op: UnOp, sz: Sz, v: V, flags: Lazy },
    /// ADDX/SUBX/NEGX/ABCD/SBCD (`d op s op X`), flags always computed.
    Extend { t: u32, op: XOp, sz: Sz, s: V, d: V },
    /// NBCD of `v` into `dst` (unwritten for Musashi's $9A case), flags always computed.
    Nbcd { v: V, dst: Dst },
    /// Shifts and rotates (`alu::shift`); `n` is the count (register counts already mod 64).
    Shift { t: u32, kind: ShiftKind, left: bool, sz: Sz, v: V, n: V, flags: Lazy },
    /// MULU/MULS (16x16 -> 32) with lazy flags.
    Mul { t: u32, signed: bool, s: V, d: V, flags: Lazy },
    /// DIVU/DIVS of `d` by `s` into Dn (unchanged on overflow); division by zero raises the
    /// exception with `pc` stacked and leaves.
    Div { signed: bool, s: V, d: V, dn: u8, pc: u32 },
    /// CHK of `d` against `bound`: out of bounds raises the exception with `pc` stacked.
    Chk { d: V, bound: V, pc: u32 },
    /// BTST/BCHG/BCLR/BSET flag: `Z = v & m == 0` (lazy).
    BitZ { v: V, m: V, flags: Lazy },
    /// Scc: `t = cond ? 0xFF : 0`
    Scc { t: u32, cond: Cond },
    /// `t = CCR` / `CCR = v` / `t = SR` / `SR = v` (stacks swap on S changes)
    GetCcr { t: u32 },
    SetCcr { v: V },
    GetSr { t: u32 },
    SetSr { v: V },
    /// The inactive stack pointer (USP in supervisor mode).
    GetOsp { t: u32 },
    SetOsp { v: V },
    /// `cpu.push16/32(v)`, `t = cpu.pop16/32()` (A7 updated)
    Push { sz: Sz, v: V },
    Pop { t: u32, sz: Sz },
    /// `Dn.sz = v` (partial sizes keep the upper bits)
    SetD { r: u8, sz: Sz, v: V },
    /// `An = v` (long)
    SetA { r: u8, v: V },
    /// `An = An + v` / `An - v`
    AddA { r: u8, v: V, sub: bool },
    /// `memory[addr].sz = v`
    Store { sz: Sz, addr: V, v: V },
    /// MOVE.L to `-(An)`: low word first (Musashi's order, reproduced by the interpreter).
    StorePd32 { addr: V, v: V },
    /// Privileged instruction: in user mode, the privilege violation exception (stacking the
    /// instruction's own `pc`) and leave.
    Priv { pc: u32 },
    /// TRAPV: if `cond`, raise `vector` with `pc` stacked and leave.
    TrapIf { cond: Cond, vector: u32, pc: u32 },
}

/// How control leaves an instruction that does not simply fall through to the next one. The
/// micro-ops before it have done the instruction's side effects (return address pushed, target
/// computed into a temporary, SR restored, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// Bcc: to `target` if `cond`, else on to the next instruction.
    Branch { cond: Cond, target: u32 },
    /// DBcc: if not `cond`, `Dn.w -= 1` and to `target` unless it became -1.
    Dbcc { cond: Cond, dn: u8, target: u32 },
    /// BRA (`jmp: false`), JMP abs/pc (`jmp: true`, which the emitter may turn into a direct
    /// Rust tail call): inside the function or a tail call.
    Goto { target: u32, jmp: bool },
    /// BSR, JSR abs/pc (return address already pushed).
    Call { target: u32 },
    /// Computed JSR to `target` (return address pushed); `ea` is the instruction's operand (for
    /// the small-data stub calls).
    DynCall { target: V, ea: Ea },
    /// Computed JMP.
    DynJump { target: V },
    /// RTS/RTR/RTE: on to `pc`.
    Return { pc: V },
    /// TRAP, ILLEGAL, line A/F: exception entry with `pc` stacked.
    Exception { vector: u32, pc: u32 },
    /// STOP (SR already set): stopped, `pc` = the next instruction.
    Stop,
}

/// Where a fused condition's flags come from: the flag-setting micro-op of an earlier
/// instruction in the same block, with only flag-neutral instructions between (see
/// [`cfg::fuse`]). The condition is then computed from that op's operands, exported as
/// `f{k}a`/`f{k}b` (k: the producer's index in the block), instead of from the flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondSrc {
    /// CMP/SUB: `b - a` at `sz` (every condition but VC/VS).
    Sub(Sz),
    /// ADD: `b + a` at `sz` (EQ, NE, MI, PL).
    Add(Sz),
    /// MOVE/TST/AND/OR/EOR/...: N, Z of `a` at `sz`, V = C = 0 (every condition).
    Logic(Sz),
    /// BTST/BCHG/BCLR/BSET: Z = `a & b == 0` (EQ, NE).
    Bit,
}

/// A consumer's fused condition: the producer's index in the block and what it computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fused {
    pub from: usize,
    pub src: CondSrc,
}

/// How an instruction is represented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// Fully described by the micro-ops (and the terminator, if any).
    Lifted(Vec<IrOp>),
}

/// One instruction in the IR.
#[derive(Clone, Debug)]
pub struct IrInsn {
    pub instr: Instr,
    pub body: Body,
    pub term: Option<Term>,
    /// (flags always written, flags read) for liveness: `crate::emit::flag_effects`, with the
    /// reads of conditions narrowed to the flags they test and intra-function jumps reading none.
    pub flag_defs: u8,
    pub flag_uses: u8,
    /// Flags read after this instruction (liveness across the function's blocks).
    pub live_after: u8,
    /// Flags read after this instruction if the block were left at its end (all flags live at
    /// the block end): what must be exact at a dispatcher exit.
    pub live_exit: u8,
    /// This instruction's condition (Bcc, DBcc, Scc) is computed from an earlier instruction's
    /// operands; it then reads no flags (`flag_uses` is 0).
    pub fused: Option<Fused>,
    /// A later instruction's condition uses this instruction's operands (see [`CondSrc`]).
    pub export: Option<(V, V)>,
}

impl IrInsn {
    pub fn ops(&self) -> &[IrOp] {
        let Body::Lifted(ops) = &self.body;
        ops
    }
    /// Sets the liveness masks and the lazy flags of the micro-ops.
    pub fn set_live(&mut self, live: u8, exit: u8) {
        self.live_after = live;
        self.live_exit = exit;
        let Body::Lifted(ops) = &mut self.body;
        for op in ops {
            if let Some(f) = op.lazy_mut() {
                f.live = live;
                f.exit = exit;
            }
        }
    }
}

impl IrInsn {
    /// The condition this instruction tests, if any.
    pub fn cond(&self) -> Option<Cond> {
        match self.term {
            Some(Term::Branch { cond, .. }) | Some(Term::Dbcc { cond, .. }) => return Some(cond),
            _ => {}
        }
        self.ops().iter().find_map(|o| if let IrOp::Scc { cond, .. } = o { Some(*cond) } else { None })
    }
    /// What this instruction's flags tell about `c`, and the operands to compute it from.
    pub fn cond_source(&self, c: Cond) -> Option<(CondSrc, V, V)> {
        let mut lazy = self.ops().iter().filter(|o| o.lazy().is_some());
        let op = lazy.next()?;
        if lazy.next().is_some() {
            return None;
        }
        let nz = matches!(c, Cond::Eq | Cond::Ne | Cond::Mi | Cond::Pl);
        match *op {
            IrOp::Arith { op: ArOp::Cmp | ArOp::Sub, sz, s, d, .. } if !matches!(c, Cond::Vc | Cond::Vs) => Some((CondSrc::Sub(sz), s, d)),
            IrOp::Arith { op: ArOp::Add, sz, s, d, .. } if nz => Some((CondSrc::Add(sz), s, d)),
            IrOp::Arith { op: ArOp::And | ArOp::Or | ArOp::Eor, sz, t: Some(t), .. } => Some((CondSrc::Logic(sz), V::T(t), V::C(0))),
            IrOp::LogicFlags { sz, v, .. } => Some((CondSrc::Logic(sz), v, V::C(0))),
            IrOp::BitZ { v, m, .. } if matches!(c, Cond::Eq | Cond::Ne) => Some((CondSrc::Bit, v, m)),
            _ => None,
        }
    }
}

impl IrOp {
    pub fn lazy_mut(&mut self) -> Option<&mut Lazy> {
        match self {
            IrOp::Arith { flags, .. }
            | IrOp::LogicFlags { flags, .. }
            | IrOp::Unary { flags, .. }
            | IrOp::Shift { flags, .. }
            | IrOp::Mul { flags, .. }
            | IrOp::BitZ { flags, .. } => Some(flags),
            _ => None,
        }
    }
    pub fn lazy(&self) -> Option<&Lazy> {
        match self {
            IrOp::Arith { flags, .. }
            | IrOp::LogicFlags { flags, .. }
            | IrOp::Unary { flags, .. }
            | IrOp::Shift { flags, .. }
            | IrOp::Mul { flags, .. }
            | IrOp::BitZ { flags, .. } => Some(flags),
            _ => None,
        }
    }
}

/// How a lazily flagged micro-op is computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagMode {
    /// With its flags (`m68k::alu`).
    Exact,
    /// With its flags if `cpu.ticks >= cpu.budget`, plain otherwise (see [`Lazy`]).
    Guarded,
    /// Without flags.
    Plain,
}

/// The count of a shift that has a flag-free form: an immediate 1..=8, any kind but ROXL/ROXR
/// (which read X).
pub fn plain_shift_count(kind: ShiftKind, n: V) -> Option<u32> {
    match (kind, n) {
        (ShiftKind::Rox, _) => None,
        (_, V::C(k)) if (1..=8).contains(&k) => Some(k),
        _ => None,
    }
}

impl IrOp {
    /// How this op is computed (`Exact` for ops without lazy flags).
    pub fn flag_mode(&self) -> FlagMode {
        if let IrOp::Shift { kind, n, .. } = self {
            if plain_shift_count(*kind, *n).is_none() {
                return FlagMode::Exact;
            }
        }
        match self.lazy() {
            Some(f) if f.needed() => FlagMode::Exact,
            Some(f) if f.guarded() => FlagMode::Guarded,
            Some(_) => FlagMode::Plain,
            None => FlagMode::Exact,
        }
    }
}
