//! A small 68000 instruction decoder for the structural scans of the open replayers
//! that read a module's driver code to find its data (it is never run): an
//! instruction's length, its kind and its operands, with the image offsets of the
//! operands' extension words (code that keeps state in its own immediates).

use super::be16;

/// An effective address. Offsets are into the decoded image; `pos` is where an
/// operand's extension (the immediate, the absolute address) is stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ea {
    None,
    D(u8),
    A(u8),
    Ind(u8),
    Post(u8),
    Pre(u8),
    Disp(u8, i16),
    Idx(u8, i8, u16),
    AbsW(u32),
    AbsL { addr: u32, pos: usize },
    /// `d16(pc)`: the target offset.
    Pc(usize),
    /// `d8(pc,Xn)`: the base offset and the extension word.
    PcIdx(usize, u16),
    Imm { val: u32, pos: usize },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum K {
    Move,
    Movea,
    Moveq,
    Lea,
    Pea,
    Bra,
    Bsr,
    /// A conditional branch; `cond` is the condition code.
    Bcc,
    Dbcc,
    Jmp,
    Jsr,
    Rts,
    Rte,
    Nop,
    Clr,
    Scc,
    Tst,
    Addq,
    Subq,
    /// The immediate forms: `ori`, `andi`, `subi`, `addi`, `eori`, `cmpi`.
    Imm(u8),
    Add,
    Sub,
    Cmp,
    And,
    Or,
    Eor,
    Adda,
    Suba,
    Cmpa,
    /// Bit operations: 0 btst, 1 bchg, 2 bclr, 3 bset (static if `src` is an immediate).
    Bit(u8),
    /// Shifts and rotates (register forms: `data` is the count or -1 for a register count).
    Shift,
    Ext,
    Swap,
    Movem,
    /// `mulu.w` and `muls.w` (the source is a word, the destination a data register).
    Mulu,
    Muls,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct Insn {
    pub at: usize,
    pub len: usize,
    pub k: K,
    /// 1, 2 or 4 bytes (0 when unsized).
    pub size: u8,
    pub src: Ea,
    pub dst: Ea,
    pub cond: u8,
    /// Branch target.
    pub target: usize,
    /// `moveq`/`addq`/`subq` data.
    pub data: i32,
    /// Shifts: the direction is left, and the kind (0 arithmetic, 1 logical, 2 rotate with
    /// extend, 3 rotate).
    pub left: bool,
    pub shk: u8,
}

impl Insn {
    pub fn next(&self) -> usize {
        self.at + self.len
    }
}

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl Rd<'_> {
    fn w(&mut self) -> Option<u16> {
        if self.p + 2 > self.d.len() {
            return None;
        }
        let v = be16(self.d, self.p);
        self.p += 2;
        Some(v)
    }
    fn l(&mut self) -> Option<u32> {
        let h = self.w()? as u32;
        Some(h << 16 | self.w()? as u32)
    }
    fn ea(&mut self, mode: u16, reg: u16, size: u8) -> Option<Ea> {
        let r = reg as u8;
        Some(match mode {
            0 => Ea::D(r),
            1 => Ea::A(r),
            2 => Ea::Ind(r),
            3 => Ea::Post(r),
            4 => Ea::Pre(r),
            5 => Ea::Disp(r, self.w()? as i16),
            6 => {
                let e = self.w()?;
                Ea::Idx(r, e as u8 as i8, e)
            }
            _ => match reg {
                0 => Ea::AbsW(self.w()? as i16 as i32 as u32),
                1 => {
                    let pos = self.p;
                    Ea::AbsL { addr: self.l()?, pos }
                }
                2 => {
                    let at = self.p;
                    Ea::Pc((at as i64 + self.w()? as i16 as i64) as usize)
                }
                3 => {
                    let at = self.p;
                    let e = self.w()?;
                    Ea::PcIdx((at as i64 + e as u8 as i8 as i64) as usize, e)
                }
                4 => {
                    let pos = self.p;
                    let val = match size {
                        4 => self.l()?,
                        1 => (self.w()? & 0xFF) as u32,
                        _ => self.w()? as u32,
                    };
                    Ea::Imm { val, pos: if size == 1 { pos + 1 } else { pos } }
                }
                _ => return None,
            },
        })
    }
}

fn sz(bits: u16) -> u8 {
    match bits {
        0 => 1,
        1 => 2,
        _ => 4,
    }
}

/// Decodes the instruction at `at` (68000 instructions only).
pub fn decode(d: &[u8], at: usize) -> Option<Insn> {
    let mut r = Rd { d, p: at };
    let op = r.w()?;
    let mode = (op >> 3) & 7;
    let reg = op & 7;
    let reg9 = (op >> 9) & 7;
    let mut i = Insn { at, len: 0, k: K::Other, size: 0, src: Ea::None, dst: Ea::None, cond: 0, target: 0, data: 0, left: false, shk: 0 };
    match op >> 12 {
        0x0 => {
            if op & 0x0100 != 0 {
                if mode == 1 {
                    // movep
                    r.w()?;
                } else {
                    i.k = K::Bit(((op >> 6) & 3) as u8);
                    i.src = Ea::D(reg9 as u8);
                    i.dst = r.ea(mode, reg, 1)?;
                }
            } else if reg9 == 4 {
                i.k = K::Bit(((op >> 6) & 3) as u8);
                let pos = r.p;
                i.src = Ea::Imm { val: (r.w()? & 0xFF) as u32, pos: pos + 1 };
                i.dst = r.ea(mode, reg, 1)?;
            } else {
                let s = (op >> 6) & 3;
                if s == 3 {
                    return None;
                }
                i.k = K::Imm(reg9 as u8);
                i.size = sz(s);
                if mode == 7 && reg == 4 {
                    // to CCR/SR
                    r.w()?;
                } else {
                    i.src = r.ea(7, 4, i.size)?;
                    i.dst = r.ea(mode, reg, i.size)?;
                }
            }
        }
        0x1..=0x3 => {
            i.size = match op >> 12 {
                1 => 1,
                2 => 4,
                _ => 2,
            };
            i.src = r.ea(mode, reg, i.size)?;
            let dm = (op >> 6) & 7;
            i.dst = r.ea(dm, reg9, i.size)?;
            i.k = if dm == 1 { K::Movea } else { K::Move };
        }
        0x4 => {
            if op & 0x01C0 == 0x01C0 {
                i.k = K::Lea;
                i.src = r.ea(mode, reg, 4)?;
                i.dst = Ea::A(reg9 as u8);
            } else if op & 0x01C0 == 0x0180 {
                // chk
                i.src = r.ea(mode, reg, 2)?;
            } else {
                match op {
                    0x4E71 => i.k = K::Nop,
                    0x4E75 => i.k = K::Rts,
                    0x4E73 => i.k = K::Rte,
                    0x4E70 | 0x4E76 | 0x4E77 | 0x4AFC => {}
                    0x4E72 => {
                        r.w()?;
                    }
                    _ if op & 0xFFF0 == 0x4E40 || op & 0xFFF0 == 0x4E60 => {}
                    _ if op & 0xFFF8 == 0x4E50 => {
                        r.w()?;
                    }
                    _ if op & 0xFFF8 == 0x4E58 => {}
                    _ if op & 0xFFC0 == 0x4E80 => {
                        i.k = K::Jsr;
                        i.dst = r.ea(mode, reg, 4)?;
                    }
                    _ if op & 0xFFC0 == 0x4EC0 => {
                        i.k = K::Jmp;
                        i.dst = r.ea(mode, reg, 4)?;
                    }
                    _ if op & 0xFFF8 == 0x4840 => {
                        i.k = K::Swap;
                        i.dst = Ea::D(reg as u8);
                    }
                    _ if op & 0xFFC0 == 0x4840 => {
                        i.k = K::Pea;
                        i.src = r.ea(mode, reg, 4)?;
                    }
                    _ if (op & 0xFFB8 == 0x4880) => {
                        i.k = K::Ext;
                        i.dst = Ea::D(reg as u8);
                    }
                    _ if op & 0xFB80 == 0x4880 => {
                        i.k = K::Movem;
                        r.w()?;
                        let s = if op & 0x40 != 0 { 4 } else { 2 };
                        i.size = s;
                        let e = r.ea(mode, reg, s)?;
                        if op & 0x0400 != 0 {
                            i.src = e;
                        } else {
                            i.dst = e;
                        }
                    }
                    _ => {
                        let s = (op >> 6) & 3;
                        let k = match op & 0xFF00 {
                            0x4200 => K::Clr,
                            0x4A00 => K::Tst,
                            _ => K::Other,
                        };
                        if s == 3 {
                            // move from/to sr/ccr, tas
                            i.dst = r.ea(mode, reg, 2)?;
                        } else {
                            i.k = k;
                            i.size = sz(s);
                            i.dst = r.ea(mode, reg, i.size)?;
                        }
                    }
                }
            }
        }
        0x5 => {
            let s = (op >> 6) & 3;
            if s == 3 {
                i.cond = ((op >> 8) & 15) as u8;
                if mode == 1 {
                    i.k = K::Dbcc;
                    i.dst = Ea::D(reg as u8);
                    let at2 = r.p;
                    i.target = (at2 as i64 + r.w()? as i16 as i64) as usize;
                } else {
                    i.k = K::Scc;
                    i.size = 1;
                    i.dst = r.ea(mode, reg, 1)?;
                }
            } else {
                i.k = if op & 0x0100 != 0 { K::Subq } else { K::Addq };
                i.size = sz(s);
                i.data = if reg9 == 0 { 8 } else { reg9 as i32 };
                i.dst = r.ea(mode, reg, i.size)?;
            }
        }
        0x6 => {
            i.cond = ((op >> 8) & 15) as u8;
            i.k = match i.cond {
                0 => K::Bra,
                1 => K::Bsr,
                _ => K::Bcc,
            };
            let b = op as u8;
            let base = at + 2;
            i.size = if b == 0 { 2 } else { 1 };
            let disp = if b == 0 { r.w()? as i16 as i64 } else { b as i8 as i64 };
            i.target = (base as i64 + disp) as usize;
        }
        0x7 => {
            if op & 0x100 != 0 {
                return None;
            }
            i.k = K::Moveq;
            i.data = op as u8 as i8 as i32;
            i.size = 4;
            i.dst = Ea::D(reg9 as u8);
        }
        0x8 | 0x9 | 0xB | 0xC | 0xD => {
            let om = (op >> 6) & 7;
            let line = op >> 12;
            if om == 3 || om == 7 {
                let s = if om == 3 { 2 } else { 4 };
                match line {
                    0x9 | 0xD | 0xB => {
                        i.k = match line {
                            0x9 => K::Suba,
                            0xD => K::Adda,
                            _ => K::Cmpa,
                        };
                        i.size = s;
                        i.src = r.ea(mode, reg, s)?;
                        i.dst = Ea::A(reg9 as u8);
                    }
                    _ => {
                        // mul/div
                        if line == 0xC {
                            i.k = if om == 3 { K::Mulu } else { K::Muls };
                            i.size = 2;
                        }
                        i.src = r.ea(mode, reg, 2)?;
                        i.dst = Ea::D(reg9 as u8);
                    }
                }
            } else {
                let s = sz(om & 3);
                i.size = s;
                let to_ea = om & 4 != 0;
                if to_ea && mode <= 1 && line != 0xB {
                    // addx/subx/abcd/sbcd/exg
                    if line == 0xC && (op & 0x01F8 == 0x0140 || op & 0x01F8 == 0x0148 || op & 0x01F8 == 0x0188) {
                        i.k = K::Other;
                    }
                } else if line == 0xB && to_ea && mode == 1 {
                    // cmpm
                } else {
                    i.k = match (line, to_ea) {
                        (0x8, _) => K::Or,
                        (0x9, _) => K::Sub,
                        (0xC, _) => K::And,
                        (0xD, _) => K::Add,
                        (0xB, false) => K::Cmp,
                        _ => K::Eor,
                    };
                    let e = r.ea(mode, reg, s)?;
                    if to_ea {
                        i.src = Ea::D(reg9 as u8);
                        i.dst = e;
                    } else {
                        i.src = e;
                        i.dst = Ea::D(reg9 as u8);
                    }
                }
            }
        }
        0xE => {
            i.k = K::Shift;
            i.left = op & 0x0100 != 0;
            if (op >> 6) & 3 == 3 {
                i.shk = ((op >> 9) & 3) as u8;
                i.size = 2;
                i.dst = r.ea(mode, reg, 2)?;
            } else {
                i.shk = ((op >> 3) & 3) as u8;
                i.size = sz((op >> 6) & 3);
                i.dst = Ea::D(reg as u8);
                i.data = if op & 0x20 != 0 {
                    -1
                } else if reg9 == 0 {
                    8
                } else {
                    reg9 as i32
                };
            }
        }
        _ => return None,
    }
    i.len = r.p - at;
    Some(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(ws: &[u16]) -> Insn {
        let b: Vec<u8> = ws.iter().flat_map(|w| w.to_be_bytes()).collect();
        decode(&b, 0).unwrap()
    }

    #[test]
    fn multiplies() {
        let i = dec(&[0xC0FC, 0x0400]);
        assert_eq!((i.k, i.len, i.src, i.dst), (K::Mulu, 4, Ea::Imm { val: 0x400, pos: 2 }, Ea::D(0)));
        assert_eq!(dec(&[0xC1FC, 0x0400]).k, K::Muls);
        assert_eq!(dec(&[0x80FC, 0x0400]).k, K::Other);
    }

    #[test]
    fn lengths_and_operands() {
        let i = dec(&[0x41F9, 0x0001, 0x2345]);
        assert_eq!((i.k, i.len, i.src), (K::Lea, 6, Ea::AbsL { addr: 0x12345, pos: 2 }));
        let i = dec(&[0x4BFA, 0xFFFE]);
        assert_eq!((i.k, i.src, i.dst), (K::Lea, Ea::Pc(0), Ea::A(5)));
        let i = dec(&[0x7AFF]);
        assert_eq!((i.k, i.data, i.dst), (K::Moveq, -1, Ea::D(5)));
        let i = dec(&[0x1B58, 0x0010]);
        assert_eq!((i.k, i.size, i.src, i.dst, i.len), (K::Move, 1, Ea::Post(0), Ea::Disp(5, 16), 4));
        let i = dec(&[0x33FC, 0x1234, 0x00DF, 0xF0A4]);
        assert_eq!((i.src, i.dst, i.len), (Ea::Imm { val: 0x1234, pos: 2 }, Ea::AbsL { addr: 0xDFF0A4, pos: 4 }, 8));
        let i = dec(&[0x6000, 0x0010]);
        assert_eq!((i.k, i.target, i.len), (K::Bra, 0x12, 4));
        let i = dec(&[0x6BFE]);
        assert_eq!((i.k, i.cond, i.target), (K::Bcc, 11, 0));
        let i = dec(&[0x4EFB, 0x0002]);
        assert_eq!((i.k, i.dst), (K::Jmp, Ea::PcIdx(4, 0x0002)));
        let i = dec(&[0x08F9, 0x0001, 0x00BF, 0xE001]);
        assert_eq!((i.k, i.len), (K::Bit(3), 8));
        let i = dec(&[0xE449]);
        assert_eq!((i.k, i.data), (K::Shift, 2));
        assert_eq!((i.left, i.shk), (false, 1));
        // asl.b #1,d2 / lsr.w #2,d1 / roxl.l #3,d0: direction and kind
        let i = dec(&[0xE302]);
        assert_eq!((i.k, i.size, i.data, i.left, i.shk, i.dst), (K::Shift, 1, 1, true, 0, Ea::D(2)));
        let i = dec(&[0xE7B0]);
        assert_eq!((i.left, i.shk, i.size), (true, 2, 4));
        let i = dec(&[0x0C10, 0x0001]);
        assert_eq!((i.k, i.size, i.src), (K::Imm(6), 1, Ea::Imm { val: 1, pos: 3 }));
        let i = dec(&[0x5310]);
        assert_eq!((i.k, i.data, i.dst), (K::Subq, 1, Ea::Ind(0)));
        let i = dec(&[0x50D0]);
        assert_eq!((i.k, i.cond, i.dst), (K::Scc, 0, Ea::Ind(0)));
        let i = dec(&[0x48E7, 0xFFFE]);
        assert_eq!((i.k, i.len), (K::Movem, 4));
    }
}
