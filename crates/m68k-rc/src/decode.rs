//! 68000 instruction decoder. Every opcode word decodes to something: words that are not a valid
//! 68000 instruction decode to [`Op::Illegal`], [`Op::LineA`] or [`Op::LineF`], which raise the
//! corresponding exception, exactly as the hardware does.
//!
//! [`decode_model`] with [`Model::M68020`] also decodes the parts of the 68010/68020 instruction set
//! that 68020 Amiga players use: MULU.L/MULS.L and DIVU.L/DIVS.L (32 and 64-bit forms), EXTB.L,
//! MOVE from CCR, MOVEC, Bcc/BRA/BSR.L, TST of An, PC-relative and immediate operands, and the 68020
//! extension words of the indexed modes: brief format with a scale factor, full format with base
//! and index suppression, base and outer displacements and memory indirection ([`Ea::Full`]).
//! Everything else the 68020 adds still decodes to [`Op::Illegal`] (which is how the recompiler's
//! callers see that a program needs more). In 68000 mode the output is exactly as before: the
//! scale and full-format bits of a brief extension word are ignored, as on the 68000.
use crate::types::*;

/// Addressing-mode classes from the 68000 manual.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    All,
    Data,
    #[allow(dead_code)]
    Mem,
    Ctrl,
    Alt,
    DataAlt,
    MemAlt,
    CtrlAlt,
}

fn class_ok(c: Class, mode: u16, reg: u16) -> bool {
    // Index: 0 Dn, 1 An, 2 (An), 3 (An)+, 4 -(An), 5 d16(An), 6 d8(An,Xn), 7 abs.w, 8 abs.l,
    // 9 d16(PC), 10 d8(PC,Xn), 11 #imm
    let k = match mode {
        0..=6 => mode,
        7 => match reg {
            0 => 7,
            1 => 8,
            2 => 9,
            3 => 10,
            4 => 11,
            _ => return false,
        },
        _ => unreachable!(),
    };
    let allowed: u16 = match c {
        Class::All => 0b1111_1111_1111,
        Class::Data => 0b1111_1111_1101,
        Class::Mem => 0b1111_1111_1100,
        Class::Ctrl => 0b0111_1110_0100,
        Class::Alt => 0b0001_1111_1111,
        Class::DataAlt => 0b0001_1111_1101,
        Class::MemAlt => 0b0001_1111_1100,
        Class::CtrlAlt => 0b0001_1110_0100,
    };
    allowed & (1 << k) != 0
}

struct D<'a, F: FnMut(u32) -> u16> {
    fetch: &'a mut F,
    model: Model,
    pc: u32,
    /// Address of the next word to read.
    p: u32,
}

impl<'a, F: FnMut(u32) -> u16> D<'a, F> {
    fn word(&mut self) -> u16 {
        let w = (self.fetch)(self.p);
        self.p = self.p.wrapping_add(2);
        w
    }
    fn long(&mut self) -> u32 {
        let h = self.word() as u32;
        (h << 16) | self.word() as u32
    }
    fn imm(&mut self, sz: Sz) -> u32 {
        match sz {
            Sz::B => self.word() as u32 & 0xFF,
            Sz::W => self.word() as u32,
            Sz::L => self.long(),
        }
    }
    fn xn(w: u16) -> Xn {
        Xn { reg: ((w >> 12) & 15) as u8, long: w & 0x0800 != 0 }
    }
    /// Decodes the EA `mode`/`reg`, reading its extension words. `sz` is used for immediates.
    fn ea(&mut self, mode: u16, reg: u16, sz: Sz) -> Ea {
        let r = reg as u8;
        match mode {
            0 => Ea::D(r),
            1 => Ea::A(r),
            2 => Ea::Ind(r),
            3 => Ea::Post(r),
            4 => Ea::Pre(r),
            5 => Ea::Disp(r, self.word() as i16),
            6 => {
                let w = self.word();
                if self.model == Model::M68020 && w & 0x0700 != 0 {
                    return self.ext020(w, Some(r), 0);
                }
                Ea::Idx(r, w as u8 as i8, Self::xn(w))
            }
            7 => match reg {
                0 => Ea::AbsW(self.word() as i16 as i32 as u32),
                1 => Ea::AbsL(self.long()),
                2 => {
                    let base = self.p;
                    Ea::PcDisp(base.wrapping_add(self.word() as i16 as i32 as u32))
                }
                3 => {
                    let base = self.p;
                    let w = self.word();
                    if self.model == Model::M68020 && w & 0x0700 != 0 {
                        return self.ext020(w, None, base);
                    }
                    Ea::PcIdx(base.wrapping_add(w as u8 as i8 as i32 as u32), Self::xn(w))
                }
                4 => Ea::Imm(self.imm(sz)),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}

impl<'a, F: FnMut(u32) -> u16> D<'a, F> {
    /// A 68020 extension word `w` (already read, at `pc_base` for a PC base) with a scale factor or
    /// in the full format. Reserved size fields are read as the `m68k` crate's 68020 core reads
    /// them: a reserved base displacement size as null, the reserved index/indirect combinations
    /// as the neighbouring defined ones.
    fn ext020(&mut self, w: u16, an: Option<u8>, pc_base: u32) -> Ea {
        let scale = ((w >> 9) & 3) as u8;
        let pc = an.is_none();
        if w & 0x0100 == 0 {
            let d8 = w as u8 as i8 as i32 as u32;
            let bd = if pc { pc_base.wrapping_add(d8) } else { d8 };
            return Ea::Full(FullEa { base: an, bd, index: Some((Self::xn(w), scale)), mem: MemInd::None, pc });
        }
        let bs = w & 0x0080 != 0;
        let is = w & 0x0040 != 0;
        let mut bd = match (w >> 4) & 3 {
            2 => self.word() as i16 as i32 as u32,
            3 => self.long(),
            _ => 0,
        };
        let base = if bs { None } else { an };
        if pc && !bs {
            bd = bd.wrapping_add(pc_base);
        }
        let iis = w & 7;
        let od = match iis & 3 {
            2 => self.word() as i16 as i32 as u32,
            3 => self.long(),
            _ => 0,
        };
        let mem = if iis == 0 {
            MemInd::None
        } else if iis & 4 != 0 {
            MemInd::Post(od)
        } else {
            MemInd::Pre(od)
        };
        let index = if is { None } else { Some((Self::xn(w), scale)) };
        Ea::Full(FullEa { base, bd, index, mem, pc: pc && !bs })
    }
}

fn size2(b: u16) -> Option<Sz> {
    match b & 3 {
        0 => Some(Sz::B),
        1 => Some(Sz::W),
        2 => Some(Sz::L),
        _ => None,
    }
}

/// Decodes the 68000 instruction at `pc`. `fetch(addr)` returns the big-endian word at `addr`.
pub fn decode<F: FnMut(u32) -> u16>(pc: u32, fetch: &mut F) -> Instr {
    decode_model(pc, fetch, Model::M68000)
}

/// Decodes the instruction at `pc` for `model`.
pub fn decode_model<F: FnMut(u32) -> u16>(pc: u32, fetch: &mut F, model: Model) -> Instr {
    let mut d = D { fetch, model, pc, p: pc };
    let opcode = d.word();
    let op = decode_op(&mut d, opcode).unwrap_or(match opcode >> 12 {
        0xA => Op::LineA,
        0xF => Op::LineF,
        _ => Op::Illegal,
    });
    // Illegal encodings consume only the opcode word.
    let len = match op {
        Op::Illegal | Op::LineA | Op::LineF => 2,
        _ => d.p.wrapping_sub(d.pc),
    };
    Instr { pc, len, opcode, op }
}

fn decode_op<F: FnMut(u32) -> u16>(d: &mut D<F>, op: u16) -> Option<Op> {
    let mode = (op >> 3) & 7;
    let reg = op & 7;
    let rx = ((op >> 9) & 7) as u8;
    let ry = reg as u8;
    let ok = |c: Class| class_ok(c, mode, reg);
    let _ = d.pc;
    Some(match op >> 12 {
        0x0 => {
            if op & 0x0100 != 0 {
                if mode == 1 {
                    let disp = d.word() as i16;
                    let opm = (op >> 6) & 7;
                    Op::Movep {
                        sz: if opm & 1 == 0 { Sz::W } else { Sz::L },
                        dn: rx,
                        an: ry,
                        disp,
                        to_mem: opm >= 6,
                    }
                } else {
                    let bop = [BitOp::Tst, BitOp::Chg, BitOp::Clr, BitOp::Set][((op >> 6) & 3) as usize];
                    let c = if bop == BitOp::Tst { Class::Data } else { Class::DataAlt };
                    if !ok(c) {
                        return None;
                    }
                    let dst = d.ea(mode, reg, Sz::B);
                    Op::Bit { op: bop, num: BitNum::Reg(rx), dst }
                }
            } else {
                let kind = (op >> 9) & 7;
                if kind == 4 {
                    let bop = [BitOp::Tst, BitOp::Chg, BitOp::Clr, BitOp::Set][((op >> 6) & 3) as usize];
                    let c = if bop == BitOp::Tst { Class::Data } else { Class::DataAlt };
                    if !ok(c) || (mode == 7 && reg == 4) {
                        return None;
                    }
                    let n = d.word() as u8;
                    let dst = d.ea(mode, reg, Sz::B);
                    return Some(Op::Bit { op: bop, num: BitNum::Imm(n), dst });
                }
                if kind == 7 {
                    return None;
                }
                let aop = [AluOp::Or, AluOp::And, AluOp::Sub, AluOp::Add, AluOp::Or, AluOp::Eor, AluOp::Cmp][kind as usize];
                if matches!(kind, 0 | 1 | 5) && (op & 0xFF) == 0x3C {
                    let imm = d.word() & 0xFF;
                    return Some(Op::LogicSr { op: aop, imm, sr: false });
                }
                if matches!(kind, 0 | 1 | 5) && (op & 0xFF) == 0x7C {
                    let imm = d.word();
                    return Some(Op::LogicSr { op: aop, imm, sr: true });
                }
                let sz = size2(op >> 6)?;
                if !ok(Class::DataAlt) {
                    return None;
                }
                let src = Ea::Imm(d.imm(sz));
                let dst = d.ea(mode, reg, sz);
                Op::Alu { op: aop, sz, src, dst }
            }
        }
        0x1 | 0x2 | 0x3 => {
            let sz = match op >> 12 {
                1 => Sz::B,
                3 => Sz::W,
                _ => Sz::L,
            };
            if !ok(Class::All) || (sz == Sz::B && mode == 1) {
                return None;
            }
            let dmode = (op >> 6) & 7;
            let dreg = (op >> 9) & 7;
            if dmode == 1 {
                if sz == Sz::B {
                    return None;
                }
                let src = d.ea(mode, reg, sz);
                Op::MoveA { sz, src, an: dreg as u8 }
            } else {
                if !class_ok(Class::DataAlt, dmode, dreg) {
                    return None;
                }
                let src = d.ea(mode, reg, sz);
                let dst = d.ea(dmode, dreg, sz);
                Op::Move { sz, src, dst }
            }
        }
        0x4 => decode_4(d, op)?,
        0x5 => {
            if (op >> 6) & 3 == 3 {
                let cond = Cond::from_bits(op >> 8);
                if mode == 1 {
                    let base = d.p;
                    let disp = d.word() as i16 as i32 as u32;
                    Op::Dbcc { cond, dn: ry, target: base.wrapping_add(disp) }
                } else {
                    if !ok(Class::DataAlt) {
                        return None;
                    }
                    Op::Scc { cond, ea: d.ea(mode, reg, Sz::B) }
                }
            } else {
                let sz = size2(op >> 6)?;
                let data = if rx == 0 { 8 } else { rx as u32 };
                if !ok(Class::Alt) || (sz == Sz::B && mode == 1) {
                    return None;
                }
                let sub = op & 0x0100 != 0;
                if mode == 1 {
                    if sub {
                        Op::SubA { sz: Sz::L, src: Ea::Imm(data), an: ry }
                    } else {
                        Op::AddA { sz: Sz::L, src: Ea::Imm(data), an: ry }
                    }
                } else {
                    let dst = d.ea(mode, reg, sz);
                    Op::Alu { op: if sub { AluOp::Sub } else { AluOp::Add }, sz, src: Ea::Imm(data), dst }
                }
            }
        }
        0x6 => {
            let base = d.p;
            let disp8 = op as u8;
            let disp = if disp8 == 0 {
                d.word() as i16 as i32 as u32
            } else if disp8 == 0xFF && d.model == Model::M68020 {
                d.long()
            } else {
                disp8 as i8 as i32 as u32
            };
            let target = base.wrapping_add(disp);
            match (op >> 8) & 15 {
                0 => Op::Bra { target },
                1 => Op::Bsr { target },
                c => Op::Bcc { cond: Cond::from_bits(c), target },
            }
        }
        0x7 => {
            if op & 0x0100 != 0 {
                return None;
            }
            Op::Moveq { data: op as u8 as i8, dn: rx }
        }
        0x8 | 0xC => {
            let is_and = op >> 12 == 0xC;
            if (op & 0x1C0) == 0x0C0 || (op & 0x1C0) == 0x1C0 {
                if !ok(Class::Data) {
                    return None;
                }
                let src = d.ea(mode, reg, Sz::W);
                let signed = op & 0x100 != 0;
                return Some(match (is_and, signed) {
                    (false, false) => Op::Divu { src, dn: rx },
                    (false, true) => Op::Divs { src, dn: rx },
                    (true, false) => Op::Mulu { src, dn: rx },
                    (true, true) => Op::Muls { src, dn: rx },
                });
            }
            if (op & 0x1F0) == 0x100 {
                let mem = op & 8 != 0;
                return Some(if is_and { Op::Abcd { rx, ry, mem } } else { Op::Sbcd { rx, ry, mem } });
            }
            if is_and {
                match op & 0x1F8 {
                    0x140 => return Some(Op::Exg { rx, ry }),
                    0x148 => return Some(Op::Exg { rx: rx + 8, ry: ry + 8 }),
                    0x188 => return Some(Op::Exg { rx, ry: ry + 8 }),
                    _ => {}
                }
            }
            let sz = size2(op >> 6)?;
            let aop = if is_and { AluOp::And } else { AluOp::Or };
            if op & 0x100 == 0 {
                if !ok(Class::Data) {
                    return None;
                }
                let src = d.ea(mode, reg, sz);
                Op::Alu { op: aop, sz, src, dst: Ea::D(rx) }
            } else {
                if !ok(Class::MemAlt) {
                    return None;
                }
                let dst = d.ea(mode, reg, sz);
                Op::Alu { op: aop, sz, src: Ea::D(rx), dst }
            }
        }
        0x9 | 0xD => {
            let add = op >> 12 == 0xD;
            let opm = (op >> 6) & 7;
            if opm == 3 || opm == 7 {
                if !ok(Class::All) {
                    return None;
                }
                let sz = if opm == 3 { Sz::W } else { Sz::L };
                let src = d.ea(mode, reg, sz);
                return Some(if add { Op::AddA { sz, src, an: rx } } else { Op::SubA { sz, src, an: rx } });
            }
            let sz = size2(op >> 6)?;
            if op & 0x100 != 0 && mode <= 1 {
                let mem = mode == 1;
                return Some(if add { Op::Addx { sz, rx, ry, mem } } else { Op::Subx { sz, rx, ry, mem } });
            }
            let aop = if add { AluOp::Add } else { AluOp::Sub };
            if op & 0x100 == 0 {
                if !ok(Class::All) || (sz == Sz::B && mode == 1) {
                    return None;
                }
                let src = d.ea(mode, reg, sz);
                Op::Alu { op: aop, sz, src, dst: Ea::D(rx) }
            } else {
                if !ok(Class::MemAlt) {
                    return None;
                }
                let dst = d.ea(mode, reg, sz);
                Op::Alu { op: aop, sz, src: Ea::D(rx), dst }
            }
        }
        0xB => {
            let opm = (op >> 6) & 7;
            if opm == 3 || opm == 7 {
                if !ok(Class::All) {
                    return None;
                }
                let sz = if opm == 3 { Sz::W } else { Sz::L };
                let src = d.ea(mode, reg, sz);
                return Some(Op::CmpA { sz, src, an: rx });
            }
            let sz = size2(op >> 6)?;
            if op & 0x100 == 0 {
                if !ok(Class::All) || (sz == Sz::B && mode == 1) {
                    return None;
                }
                let src = d.ea(mode, reg, sz);
                Op::Alu { op: AluOp::Cmp, sz, src, dst: Ea::D(rx) }
            } else if mode == 1 {
                Op::Cmpm { sz, ax: rx, ay: ry }
            } else {
                if !ok(Class::DataAlt) {
                    return None;
                }
                let dst = d.ea(mode, reg, sz);
                Op::Alu { op: AluOp::Eor, sz, src: Ea::D(rx), dst }
            }
        }
        0xE => {
            let left = op & 0x100 != 0;
            if (op >> 6) & 3 == 3 {
                if op & 0x0800 != 0 || !ok(Class::MemAlt) {
                    return None;
                }
                let kind = [ShiftKind::As, ShiftKind::Ls, ShiftKind::Rox, ShiftKind::Ro][((op >> 9) & 3) as usize];
                Op::ShiftMem { kind, left, ea: d.ea(mode, reg, Sz::W) }
            } else {
                let sz = size2(op >> 6)?;
                let kind = [ShiftKind::As, ShiftKind::Ls, ShiftKind::Rox, ShiftKind::Ro][((op >> 3) & 3) as usize];
                let count = if op & 0x20 != 0 {
                    ShiftCount::Reg(rx)
                } else {
                    ShiftCount::Imm(if rx == 0 { 8 } else { rx })
                };
                Op::ShiftReg { kind, left, sz, count, dn: ry }
            }
        }
        _ => return None,
    })
}

fn decode_4<F: FnMut(u32) -> u16>(d: &mut D<F>, op: u16) -> Option<Op> {
    let mode = (op >> 3) & 7;
    let reg = op & 7;
    let rx = ((op >> 9) & 7) as u8;
    let ry = reg as u8;
    let ok = |c: Class| class_ok(c, mode, reg);
    match op {
        0x4AFC => return Some(Op::Illegal),
        0x4E70 => return Some(Op::Reset),
        0x4E71 => return Some(Op::Nop),
        0x4E72 => return Some(Op::Stop { imm: d.word() }),
        0x4E73 => return Some(Op::Rte),
        0x4E75 => return Some(Op::Rts),
        0x4E76 => return Some(Op::Trapv),
        0x4E77 => return Some(Op::Rtr),
        _ => {}
    }
    if d.model == Model::M68020 {
        if let Some(r) = decode_4_020(d, op) {
            return r;
        }
    }
    match op & 0xFFF0 {
        0x4E40 => return Some(Op::Trap { n: (op & 15) as u8 }),
        0x4E50 if op & 8 == 0 => return Some(Op::Link { an: ry, disp: d.word() as i16 }),
        0x4E50 => return Some(Op::Unlk { an: ry }),
        0x4E60 => return Some(Op::MoveUsp { to_usp: op & 8 == 0, an: ry }),
        _ => {}
    }
    match op & 0xFFC0 {
        0x4E80 => return if ok(Class::Ctrl) { Some(Op::Jsr { ea: d.ea(mode, reg, Sz::L) }) } else { None },
        0x4EC0 => return if ok(Class::Ctrl) { Some(Op::Jmp { ea: d.ea(mode, reg, Sz::L) }) } else { None },
        0x40C0 => return if ok(Class::DataAlt) { Some(Op::MoveFromSr { ea: d.ea(mode, reg, Sz::W) }) } else { None },
        0x44C0 => return if ok(Class::Data) { Some(Op::MoveToCcr { ea: d.ea(mode, reg, Sz::W) }) } else { None },
        0x46C0 => return if ok(Class::Data) { Some(Op::MoveToSr { ea: d.ea(mode, reg, Sz::W) }) } else { None },
        0x4800 => return if ok(Class::DataAlt) { Some(Op::Nbcd { ea: d.ea(mode, reg, Sz::B) }) } else { None },
        0x4840 => {
            if mode == 0 {
                return Some(Op::Swap { dn: ry });
            }
            return if ok(Class::Ctrl) { Some(Op::Pea { src: d.ea(mode, reg, Sz::L) }) } else { None };
        }
        0x4880 | 0x48C0 => {
            let sz = if op & 0x40 != 0 { Sz::L } else { Sz::W };
            if mode == 0 {
                return Some(Op::Ext { sz, dn: ry });
            }
            if !(ok(Class::CtrlAlt) || mode == 4) {
                return None;
            }
            let mask = d.word();
            return Some(Op::Movem { sz, to_mem: true, mask, ea: d.ea(mode, reg, sz) });
        }
        0x4C80 | 0x4CC0 => {
            let sz = if op & 0x40 != 0 { Sz::L } else { Sz::W };
            if !(ok(Class::Ctrl) || mode == 3) {
                return None;
            }
            let mask = d.word();
            return Some(Op::Movem { sz, to_mem: false, mask, ea: d.ea(mode, reg, sz) });
        }
        0x4AC0 => return if ok(Class::DataAlt) { Some(Op::Tas { ea: d.ea(mode, reg, Sz::B) }) } else { None },
        _ => {}
    }
    match op & 0xF1C0 {
        0x41C0 => return if ok(Class::Ctrl) { Some(Op::Lea { src: d.ea(mode, reg, Sz::L), an: rx }) } else { None },
        0x4180 => return if ok(Class::Data) { Some(Op::Chk { src: d.ea(mode, reg, Sz::W), dn: rx }) } else { None },
        _ => {}
    }
    let sz = size2(op >> 6)?;
    match op & 0xFF00 {
        0x4000 | 0x4200 | 0x4400 | 0x4600 | 0x4A00 => {
            if !ok(Class::DataAlt) {
                return None;
            }
            let ea = d.ea(mode, reg, sz);
            Some(match op & 0xFF00 {
                0x4000 => Op::Negx { sz, ea },
                0x4200 => Op::Clr { sz, ea },
                0x4400 => Op::Neg { sz, ea },
                0x4600 => Op::Not { sz, ea },
                _ => Op::Tst { sz, ea },
            })
        }
        _ => None,
    }
}

/// The 68010/68020 additions to line 4: `Some(result)` for the opcodes they define (`Some(None)`:
/// one of them with an invalid EA), `None` for everything the 68000 decoder handles as before.
fn decode_4_020<F: FnMut(u32) -> u16>(d: &mut D<F>, op: u16) -> Option<Option<Op>> {
    let mode = (op >> 3) & 7;
    let reg = op & 7;
    let ok = |c: Class| class_ok(c, mode, reg);
    if op & 0xFFF8 == 0x49C0 {
        return Some(Some(Op::Extb { dn: reg as u8 }));
    }
    if op & 0xFFFE == 0x4E7A {
        let x = d.word();
        return Some(Some(Op::Movec { to_ctrl: op & 1 != 0, reg: (x >> 12) as u8, ctrl: x & 0x0FFF }));
    }
    match op & 0xFFC0 {
        0x42C0 => return Some(if ok(Class::DataAlt) { Some(Op::MoveFromCcr { ea: d.ea(mode, reg, Sz::W) }) } else { None }),
        0x4C00 | 0x4C40 => {
            if !ok(Class::Data) {
                return Some(None);
            }
            let x = d.word();
            let signed = x & 0x0800 != 0;
            let wide = x & 0x0400 != 0;
            let (hi, lo) = ((x & 7) as u8, ((x >> 12) & 7) as u8);
            let src = d.ea(mode, reg, Sz::L);
            return Some(Some(if op & 0x40 == 0 {
                Op::MulL { signed, src, dl: lo, dh: if wide { Some(hi) } else { None } }
            } else {
                Op::DivL { signed, src, dq: lo, dr: hi, wide }
            }));
        }
        _ => {}
    }
    // TST takes every mode on the 68020 (An only as .w/.l).
    if op & 0xFF00 == 0x4A00 && op & 0xC0 != 0xC0 && !ok(Class::DataAlt) && ok(Class::All) {
        let sz = size2(op >> 6)?;
        if sz == Sz::B && mode == 1 {
            return Some(None);
        }
        return Some(Some(Op::Tst { sz, ea: d.ea(mode, reg, sz) }));
    }
    None
}
