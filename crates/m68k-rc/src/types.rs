//! Instruction representation produced by the decoder.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sz {
    B,
    W,
    L,
}

impl Sz {
    #[inline(always)]
    pub const fn bytes(self) -> u32 {
        match self {
            Sz::B => 1,
            Sz::W => 2,
            Sz::L => 4,
        }
    }
    #[inline(always)]
    pub const fn mask(self) -> u32 {
        match self {
            Sz::B => 0xFF,
            Sz::W => 0xFFFF,
            Sz::L => 0xFFFF_FFFF,
        }
    }
    #[inline(always)]
    pub const fn msb(self) -> u32 {
        match self {
            Sz::B => 0x80,
            Sz::W => 0x8000,
            Sz::L => 0x8000_0000,
        }
    }
    #[inline(always)]
    pub const fn bits(self) -> u32 {
        self.bytes() * 8
    }
    /// Sign-extends the low `self` bits of `v` to 32 bits.
    #[inline(always)]
    pub const fn sext(self, v: u32) -> u32 {
        match self {
            Sz::B => v as u8 as i8 as i32 as u32,
            Sz::W => v as u16 as i16 as i32 as u32,
            Sz::L => v,
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Sz::B => "b",
            Sz::W => "w",
            Sz::L => "l",
        }
    }
    pub fn rust(self) -> &'static str {
        match self {
            Sz::B => "Sz::B",
            Sz::W => "Sz::W",
            Sz::L => "Sz::L",
        }
    }
}

/// Index register of the (d8,An,Xn) / (d8,PC,Xn) modes. `reg` 0-7 = D0-D7, 8-15 = A0-A7.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Xn {
    pub reg: u8,
    pub long: bool,
}

/// The CPU a decoder (and the code decoded with it) follows. The 68000 is the default everywhere;
/// the 68020 model adds what the 68020 players use (see [`crate::decode`]) and the 68010+ exception
/// stack frames, RTE and the privileged MOVE from SR.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Model {
    #[default]
    M68000,
    M68020,
}

/// Memory indirection of a 68020 full-format EA.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemInd {
    /// `(bd,An,Xn)`: no memory access.
    None,
    /// `([bd,An,Xn],od)`: the index is added before the pointer is read.
    Pre(u32),
    /// `([bd,An],Xn,od)`: the index is added to the pointer read.
    Post(u32),
}

/// A 68020 indexed EA (mode 6 or `(PC,Xn)` with a brief extension word that has a scale factor, or
/// with a full extension word): `addr = base + bd + (index << scale)`, then the memory indirection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FullEa {
    /// An (0-7), or `None` when the base is suppressed or PC-relative. A PC base is resolved by the
    /// decoder into `bd`, which then holds the absolute address.
    pub base: Option<u8>,
    /// The base displacement, sign-extended (with a PC base: plus the extension word's address).
    pub bd: u32,
    /// Index register and scale shift (0-3), unless suppressed.
    pub index: Option<(Xn, u8)>,
    /// Outer displacements are sign-extended.
    pub mem: MemInd,
    /// PC-relative (for listings only: the PC is already in `bd`).
    pub pc: bool,
}

/// Effective address. PC-relative modes are resolved by the decoder: `PcDisp` holds the absolute
/// address, `PcIdx` the base (extension word address + d8) to which the index register is added.
/// `Full` is only produced in 68020 mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ea {
    D(u8),
    A(u8),
    Ind(u8),
    Post(u8),
    Pre(u8),
    Disp(u8, i16),
    Idx(u8, i8, Xn),
    AbsW(u32),
    AbsL(u32),
    PcDisp(u32),
    PcIdx(u32, Xn),
    Imm(u32),
    Full(FullEa),
}

impl Ea {
    pub fn is_mem(&self) -> bool {
        !matches!(self, Ea::D(_) | Ea::A(_) | Ea::Imm(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cond {
    T,
    F,
    Hi,
    Ls,
    Cc,
    Cs,
    Ne,
    Eq,
    Vc,
    Vs,
    Pl,
    Mi,
    Ge,
    Lt,
    Gt,
    Le,
}

impl Cond {
    pub fn from_bits(b: u16) -> Cond {
        use Cond::*;
        [T, F, Hi, Ls, Cc, Cs, Ne, Eq, Vc, Vs, Pl, Mi, Ge, Lt, Gt, Le][(b & 15) as usize]
    }
    pub fn name(self) -> &'static str {
        use Cond::*;
        match self {
            T => "t",
            F => "f",
            Hi => "hi",
            Ls => "ls",
            Cc => "cc",
            Cs => "cs",
            Ne => "ne",
            Eq => "eq",
            Vc => "vc",
            Vs => "vs",
            Pl => "pl",
            Mi => "mi",
            Ge => "ge",
            Lt => "lt",
            Gt => "gt",
            Le => "le",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AluOp {
    Or,
    And,
    Sub,
    Add,
    Eor,
    Cmp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShiftKind {
    As,
    Ls,
    Rox,
    Ro,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BitOp {
    Tst,
    Chg,
    Clr,
    Set,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BitNum {
    Imm(u8),
    Reg(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShiftCount {
    Imm(u8),
    Reg(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// OR/AND/SUB/ADD/EOR/CMP and their immediate forms: `dst = dst op src` (CMP only sets flags).
    Alu { op: AluOp, sz: Sz, src: Ea, dst: Ea },
    /// ADDA/SUBA/CMPA, and ADDQ/SUBQ to An (as `sz: L` with an immediate source).
    AddA { sz: Sz, src: Ea, an: u8 },
    SubA { sz: Sz, src: Ea, an: u8 },
    CmpA { sz: Sz, src: Ea, an: u8 },
    /// ORI/ANDI/EORI to CCR (`sr: false`) or SR (`sr: true`).
    LogicSr { op: AluOp, imm: u16, sr: bool },
    Bit { op: BitOp, num: BitNum, dst: Ea },
    Movep { sz: Sz, dn: u8, an: u8, disp: i16, to_mem: bool },
    Move { sz: Sz, src: Ea, dst: Ea },
    MoveA { sz: Sz, src: Ea, an: u8 },
    Negx { sz: Sz, ea: Ea },
    Clr { sz: Sz, ea: Ea },
    Neg { sz: Sz, ea: Ea },
    Not { sz: Sz, ea: Ea },
    Tst { sz: Sz, ea: Ea },
    MoveFromSr { ea: Ea },
    MoveToCcr { ea: Ea },
    MoveToSr { ea: Ea },
    Chk { src: Ea, dn: u8 },
    Lea { src: Ea, an: u8 },
    Pea { src: Ea },
    Nbcd { ea: Ea },
    Swap { dn: u8 },
    Ext { sz: Sz, dn: u8 },
    /// MOVEM. `mask` is as encoded (reversed for -(An)).
    Movem { sz: Sz, to_mem: bool, mask: u16, ea: Ea },
    Tas { ea: Ea },
    Illegal,
    Trap { n: u8 },
    Link { an: u8, disp: i16 },
    Unlk { an: u8 },
    MoveUsp { to_usp: bool, an: u8 },
    Reset,
    Nop,
    Stop { imm: u16 },
    Rte,
    Rts,
    Trapv,
    Rtr,
    Jsr { ea: Ea },
    Jmp { ea: Ea },
    Scc { cond: Cond, ea: Ea },
    Dbcc { cond: Cond, dn: u8, target: u32 },
    Bra { target: u32 },
    Bsr { target: u32 },
    Bcc { cond: Cond, target: u32 },
    Moveq { data: i8, dn: u8 },
    Divu { src: Ea, dn: u8 },
    Divs { src: Ea, dn: u8 },
    Mulu { src: Ea, dn: u8 },
    Muls { src: Ea, dn: u8 },
    /// ABCD/SBCD/ADDX/SUBX: register form `Dy,Dx` or memory form `-(Ay),-(Ax)`.
    Abcd { rx: u8, ry: u8, mem: bool },
    Sbcd { rx: u8, ry: u8, mem: bool },
    Addx { sz: Sz, rx: u8, ry: u8, mem: bool },
    Subx { sz: Sz, rx: u8, ry: u8, mem: bool },
    Cmpm { sz: Sz, ax: u8, ay: u8 },
    /// EXG: `rx`/`ry` are 0-15 (D0-D7, A0-A7).
    Exg { rx: u8, ry: u8 },
    ShiftReg { kind: ShiftKind, left: bool, sz: Sz, count: ShiftCount, dn: u8 },
    ShiftMem { kind: ShiftKind, left: bool, ea: Ea },
    // ---- 68010/68020 (decoded in 68020 mode only)
    /// MOVE from CCR (68010).
    MoveFromCcr { ea: Ea },
    /// EXTB.L.
    Extb { dn: u8 },
    /// MULU.L/MULS.L `<ea>,Dl` (`dh: None`, 32-bit product) or `<ea>,Dh:Dl` (64-bit product).
    MulL { signed: bool, src: Ea, dl: u8, dh: Option<u8> },
    /// DIVU.L/DIVS.L: `Dq = Dq / <ea>` with the remainder in `Dr` (none kept when `Dr == Dq`);
    /// `wide`: the dividend is the 64-bit `Dr:Dq`.
    DivL { signed: bool, src: Ea, dq: u8, dr: u8, wide: bool },
    /// MOVEC (68010+): `to_ctrl` Rn,Rc (else Rc,Rn); `reg` 0-15 = D0-D7, A0-A7; `ctrl` the 12-bit
    /// control register code, as encoded (an unknown one raises an illegal instruction exception
    /// when executed in supervisor mode, see [`crate::Cpu::movec_legal`]).
    Movec { to_ctrl: bool, reg: u8, ctrl: u16 },
    LineA,
    LineF,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Instr {
    pub pc: u32,
    /// Length in bytes (opcode + extension words).
    pub len: u32,
    pub opcode: u16,
    pub op: Op,
}

impl Instr {
    pub fn next(&self) -> u32 {
        self.pc.wrapping_add(self.len)
    }
}
