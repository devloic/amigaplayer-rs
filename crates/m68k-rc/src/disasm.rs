//! Motorola-syntax text for [`Instr`] (used in comments of the generated code and in reports).
use crate::types::*;
use std::fmt::{self, Display, Write};

fn reg(r: u8) -> String {
    if r < 8 { format!("d{}", r) } else { format!("a{}", r - 8) }
}

impl Display for Ea {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Ea::D(r) => write!(f, "d{}", r),
            Ea::A(r) => write!(f, "a{}", r),
            Ea::Ind(r) => write!(f, "(a{})", r),
            Ea::Post(r) => write!(f, "(a{})+", r),
            Ea::Pre(r) => write!(f, "-(a{})", r),
            Ea::Disp(r, d) => write!(f, "{}(a{})", d, r),
            Ea::Idx(r, d, x) => write!(f, "{}(a{},{}.{})", d, r, reg(x.reg), if x.long { "l" } else { "w" }),
            Ea::AbsW(a) => write!(f, "(${:x}).w", a as u16),
            Ea::AbsL(a) => write!(f, "(${:x}).l", a),
            Ea::PcDisp(a) => write!(f, "${:x}(pc)", a),
            Ea::PcIdx(a, x) => write!(f, "${:x}(pc,{}.{})", a, reg(x.reg), if x.long { "l" } else { "w" }),
            Ea::Imm(v) => write!(f, "#${:x}", v),
            Ea::Full(x) => {
                // `([bd,base,Xn*s],od)`; a PC base is shown as the absolute address it gives.
                let base = match (x.base, x.pc) {
                    (Some(r), _) => format!(",a{}", r),
                    (None, true) => ",pc".to_string(),
                    (None, false) => String::new(),
                };
                let bd = if x.pc { format!("${:x}", x.bd) } else { format!("{}", x.bd as i32) };
                let idx = x.index.map_or(String::new(), |(i, sc)| {
                    format!(",{}.{}*{}", reg(i.reg), if i.long { "l" } else { "w" }, 1 << sc)
                });
                match x.mem {
                    MemInd::None => write!(f, "({}{}{})", bd, base, idx),
                    MemInd::Pre(od) => write!(f, "([{}{}{}],{})", bd, base, idx, od as i32),
                    MemInd::Post(od) => write!(f, "([{}{}]{},{})", bd, base, idx, od as i32),
                }
            }
        }
    }
}

fn mask_list(mask: u16, pre: bool) -> String {
    let mut s = String::new();
    for i in 0..16 {
        let bit = if pre { 15 - i } else { i };
        if mask & (1 << bit) != 0 {
            if !s.is_empty() {
                s.push('/');
            }
            s.push_str(&reg(i as u8));
        }
    }
    s
}

impl Display for Instr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut s = String::new();
        let shift_name = |k: ShiftKind, l: bool| {
            format!("{}{}", match k {
                ShiftKind::As => "as",
                ShiftKind::Ls => "ls",
                ShiftKind::Rox => "rox",
                ShiftKind::Ro => "ro",
            }, if l { "l" } else { "r" })
        };
        let _ = match self.op {
            Op::Alu { op, sz, src, dst } => {
                let n = match op {
                    AluOp::Or => "or",
                    AluOp::And => "and",
                    AluOp::Sub => "sub",
                    AluOp::Add => "add",
                    AluOp::Eor => "eor",
                    AluOp::Cmp => "cmp",
                };
                let i = if matches!(src, Ea::Imm(_)) && !matches!(dst, Ea::D(_) if false) { "i" } else { "" };
                write!(s, "{}{}.{} {},{}", n, i, sz.suffix(), src, dst)
            }
            Op::AddA { sz, src, an } => write!(s, "adda.{} {},a{}", sz.suffix(), src, an),
            Op::SubA { sz, src, an } => write!(s, "suba.{} {},a{}", sz.suffix(), src, an),
            Op::CmpA { sz, src, an } => write!(s, "cmpa.{} {},a{}", sz.suffix(), src, an),
            Op::LogicSr { op, imm, sr } => write!(s, "{}i #${:x},{}", match op {
                AluOp::Or => "or",
                AluOp::And => "and",
                _ => "eor",
            }, imm, if sr { "sr" } else { "ccr" }),
            Op::Bit { op, num, dst } => write!(s, "b{} {},{}", match op {
                BitOp::Tst => "tst",
                BitOp::Chg => "chg",
                BitOp::Clr => "clr",
                BitOp::Set => "set",
            }, match num {
                BitNum::Imm(n) => format!("#{}", n),
                BitNum::Reg(r) => format!("d{}", r),
            }, dst),
            Op::Movep { sz, dn, an, disp, to_mem } => {
                if to_mem {
                    write!(s, "movep.{} d{},{}(a{})", sz.suffix(), dn, disp, an)
                } else {
                    write!(s, "movep.{} {}(a{}),d{}", sz.suffix(), disp, an, dn)
                }
            }
            Op::Move { sz, src, dst } => write!(s, "move.{} {},{}", sz.suffix(), src, dst),
            Op::MoveA { sz, src, an } => write!(s, "movea.{} {},a{}", sz.suffix(), src, an),
            Op::Negx { sz, ea } => write!(s, "negx.{} {}", sz.suffix(), ea),
            Op::Clr { sz, ea } => write!(s, "clr.{} {}", sz.suffix(), ea),
            Op::Neg { sz, ea } => write!(s, "neg.{} {}", sz.suffix(), ea),
            Op::Not { sz, ea } => write!(s, "not.{} {}", sz.suffix(), ea),
            Op::Tst { sz, ea } => write!(s, "tst.{} {}", sz.suffix(), ea),
            Op::MoveFromSr { ea } => write!(s, "move sr,{}", ea),
            Op::MoveToCcr { ea } => write!(s, "move {},ccr", ea),
            Op::MoveToSr { ea } => write!(s, "move {},sr", ea),
            Op::Chk { src, dn } => write!(s, "chk.w {},d{}", src, dn),
            Op::Lea { src, an } => write!(s, "lea {},a{}", src, an),
            Op::Pea { src } => write!(s, "pea {}", src),
            Op::Nbcd { ea } => write!(s, "nbcd {}", ea),
            Op::Swap { dn } => write!(s, "swap d{}", dn),
            Op::Ext { sz, dn } => write!(s, "ext.{} d{}", sz.suffix(), dn),
            Op::Movem { sz, to_mem, mask, ea } => {
                let l = mask_list(mask, matches!(ea, Ea::Pre(_)));
                if to_mem {
                    write!(s, "movem.{} {},{}", sz.suffix(), l, ea)
                } else {
                    write!(s, "movem.{} {},{}", sz.suffix(), ea, l)
                }
            }
            Op::Tas { ea } => write!(s, "tas {}", ea),
            Op::Illegal => write!(s, "illegal ; ${:04x}", self.opcode),
            Op::Trap { n } => write!(s, "trap #{}", n),
            Op::Link { an, disp } => write!(s, "link a{},#{}", an, disp),
            Op::Unlk { an } => write!(s, "unlk a{}", an),
            Op::MoveUsp { to_usp, an } => {
                if to_usp { write!(s, "move a{},usp", an) } else { write!(s, "move usp,a{}", an) }
            }
            Op::Reset => write!(s, "reset"),
            Op::Nop => write!(s, "nop"),
            Op::Stop { imm } => write!(s, "stop #${:x}", imm),
            Op::Rte => write!(s, "rte"),
            Op::Rts => write!(s, "rts"),
            Op::Trapv => write!(s, "trapv"),
            Op::Rtr => write!(s, "rtr"),
            Op::Jsr { ea } => write!(s, "jsr {}", ea),
            Op::Jmp { ea } => write!(s, "jmp {}", ea),
            Op::Scc { cond, ea } => write!(s, "s{} {}", cond.name(), ea),
            Op::Dbcc { cond, dn, target } => write!(s, "db{} d{},${:x}", cond.name(), dn, target),
            Op::Bra { target } => write!(s, "bra ${:x}", target),
            Op::Bsr { target } => write!(s, "bsr ${:x}", target),
            Op::Bcc { cond, target } => write!(s, "b{} ${:x}", cond.name(), target),
            Op::Moveq { data, dn } => write!(s, "moveq #{},d{}", data, dn),
            Op::Divu { src, dn } => write!(s, "divu.w {},d{}", src, dn),
            Op::Divs { src, dn } => write!(s, "divs.w {},d{}", src, dn),
            Op::Mulu { src, dn } => write!(s, "mulu.w {},d{}", src, dn),
            Op::Muls { src, dn } => write!(s, "muls.w {},d{}", src, dn),
            Op::Abcd { rx, ry, mem } | Op::Sbcd { rx, ry, mem } => {
                let n = if matches!(self.op, Op::Abcd { .. }) { "abcd" } else { "sbcd" };
                if mem { write!(s, "{} -(a{}),-(a{})", n, ry, rx) } else { write!(s, "{} d{},d{}", n, ry, rx) }
            }
            Op::Addx { sz, rx, ry, mem } | Op::Subx { sz, rx, ry, mem } => {
                let n = if matches!(self.op, Op::Addx { .. }) { "addx" } else { "subx" };
                if mem {
                    write!(s, "{}.{} -(a{}),-(a{})", n, sz.suffix(), ry, rx)
                } else {
                    write!(s, "{}.{} d{},d{}", n, sz.suffix(), ry, rx)
                }
            }
            Op::Cmpm { sz, ax, ay } => write!(s, "cmpm.{} (a{})+,(a{})+", sz.suffix(), ay, ax),
            Op::Exg { rx, ry } => write!(s, "exg {},{}", reg(rx), reg(ry)),
            Op::ShiftReg { kind, left, sz, count, dn } => write!(s, "{}.{} {},d{}", shift_name(kind, left), sz.suffix(), match count {
                ShiftCount::Imm(n) => format!("#{}", n),
                ShiftCount::Reg(r) => format!("d{}", r),
            }, dn),
            Op::ShiftMem { kind, left, ea } => write!(s, "{}.w {}", shift_name(kind, left), ea),
            Op::MoveFromCcr { ea } => write!(s, "move ccr,{}", ea),
            Op::Extb { dn } => write!(s, "extb.l d{}", dn),
            Op::Movec { to_ctrl, reg: r, ctrl } => {
                let rc = match ctrl {
                    0x000 => "sfc".to_string(),
                    0x001 => "dfc".to_string(),
                    0x002 => "cacr".to_string(),
                    0x800 => "usp".to_string(),
                    0x801 => "vbr".to_string(),
                    0x802 => "caar".to_string(),
                    0x803 => "msp".to_string(),
                    0x804 => "isp".to_string(),
                    c => format!("${:03x}", c),
                };
                if to_ctrl { write!(s, "movec {},{}", reg(r), rc) } else { write!(s, "movec {},{}", rc, reg(r)) }
            }
            Op::MulL { signed, src, dl, dh } => match dh {
                Some(h) => write!(s, "mul{}.l {},d{}:d{}", if signed { "s" } else { "u" }, src, h, dl),
                None => write!(s, "mul{}.l {},d{}", if signed { "s" } else { "u" }, src, dl),
            },
            Op::DivL { signed, src, dq, dr, wide } => {
                let n = if signed { "s" } else { "u" };
                if wide {
                    write!(s, "div{}.l {},d{}:d{}", n, src, dr, dq)
                } else if dr != dq {
                    write!(s, "div{}l.l {},d{}:d{}", n, src, dr, dq)
                } else {
                    write!(s, "div{}.l {},d{}", n, src, dq)
                }
            }
            Op::LineA => write!(s, "line-a ${:04x}", self.opcode),
            Op::LineF => write!(s, "line-f ${:04x}", self.opcode),
        };
        f.write_str(&s)
    }
}
