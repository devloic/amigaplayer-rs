//! 68000 instruction timing: clock cycles per instruction, effective-address calculation included,
//! from the tables of the M68000 User's Manual (section 8, "Instruction execution times"), for
//! code running with no wait states (an A500 CPU in chip RAM loses cycles to DMA on top of this;
//! that is the machine's business, not this table's).
//!
//! The count is static (one number per decoded instruction), so that the recompiler can add a
//! whole block's sum on entry and the interpreter adds the same figure; where the real count
//! depends on data or on the branch taken, a fixed figure is used:
//!
//! | instruction | real | used |
//! |---|---|---|
//! | Bcc.b / Bcc.w | taken 10, not taken 8 / 12 | 9 / 11 (the mean) |
//! | DBcc | loop 10, condition true 12, expired 14 | 10 (the loop) |
//! | Scc Dn | false 4, true 6 | 5 |
//! | MULU / MULS | 38 + 2n (n = ones / 01-10 pairs of the source) | 54 + ea (n = 8) |
//! | DIVU / DIVS | at most 140 / 158 | 124 / 142 + ea (UNSURE: about 90 % of the worst case) |
//! | shift/rotate Dn by Dm | 6 + 2n (.b/.w), 8 + 2n (.l), n = Dm mod 64 | n = 4 (UNSURE) |
//! | TRAPV, CHK | no trap 4 / 10 + ea | no trap (the exception adds its own time) |
//!
//! Exceptions taken (TRAP, CHK, zero divide, interrupts) are counted by whoever takes them (see
//! [`INTERRUPT`]); a taken TRAP is 34 here because the TRAP instruction always traps.
//!
//! The 68020 additions ([`Model::M68020`]) have no 68000 figure; they get rough ones (UNSURE):
//! 68020 indexed EAs as the 68000 indexed modes plus 4 per memory indirection, MULx.L/DIVx.L 40
//! + ea (the `m68k` crate's figure), EXTB.L 4, MOVE from CCR as MOVE from SR.

use crate::types::*;

/// Interrupt acknowledge and exception processing (autovector), as the 68000 UM gives it.
pub const INTERRUPT: u32 = 44;

/// Effective-address calculation time (byte/word, long).
fn ea(e: &Ea, sz: Sz) -> u32 {
    let l = sz == Sz::L;
    let (w, ll) = match e {
        Ea::D(_) | Ea::A(_) => (0, 0),
        Ea::Ind(_) | Ea::Post(_) => (4, 8),
        Ea::Pre(_) => (6, 10),
        Ea::Disp(..) | Ea::AbsW(_) | Ea::PcDisp(_) => (8, 12),
        Ea::Idx(..) | Ea::PcIdx(..) => (10, 14),
        Ea::Full(f) => if f.mem == MemInd::None { (10, 14) } else { (14, 18) },
        Ea::AbsL(_) => (12, 16),
        Ea::Imm(_) => (4, 8),
    };
    if l { ll } else { w }
}

/// Destination write time of MOVE (the source's time comes from [`ea`]).
fn move_dst(e: &Ea, sz: Sz) -> u32 {
    let l = sz == Sz::L;
    match e {
        Ea::D(_) | Ea::A(_) | Ea::Imm(_) => 0,
        Ea::Ind(_) | Ea::Post(_) | Ea::Pre(_) => if l { 8 } else { 4 },
        Ea::Disp(..) | Ea::AbsW(_) | Ea::PcDisp(_) => if l { 12 } else { 8 },
        Ea::Idx(..) | Ea::PcIdx(..) => if l { 14 } else { 10 },
        Ea::Full(f) => (if l { 14 } else { 10 }) + if f.mem == MemInd::None { 0 } else { 4 },
        Ea::AbsL(_) => if l { 16 } else { 12 },
    }
}

/// JMP/JSR/LEA/PEA control-address times: (jmp, jsr, lea, pea).
fn control(e: &Ea) -> (u32, u32, u32, u32) {
    match e {
        Ea::Ind(_) => (8, 16, 4, 12),
        Ea::Disp(..) | Ea::AbsW(_) | Ea::PcDisp(_) => (10, 18, 8, 16),
        Ea::Idx(..) | Ea::PcIdx(..) => (14, 22, 12, 20),
        Ea::AbsL(_) => (12, 20, 12, 20),
        _ => (8, 16, 4, 12),
    }
}

fn is_reg(e: &Ea) -> bool {
    matches!(e, Ea::D(_) | Ea::A(_))
}

/// Clock cycles of `ins` (see the module documentation for the data-dependent cases).
pub fn cycles(ins: &Instr) -> u32 {
    let group = ins.opcode >> 12;
    match ins.op {
        Op::Alu { op, sz, src, dst } => {
            let l = sz == Sz::L;
            let imm_group = group == 0; // ORI/ANDI/SUBI/ADDI/EORI/CMPI
            let quick = group == 5; // ADDQ/SUBQ
            match (dst, op) {
                (Ea::D(_), AluOp::Cmp) if imm_group => if l { 14 } else { 8 },
                (Ea::D(_), _) if quick => if l { 8 } else { 4 },
                (Ea::D(_), _) if imm_group => if l { 16 } else { 8 },
                (Ea::D(_), AluOp::Eor) => if l { 8 } else { 4 },
                (Ea::D(_), AluOp::Cmp) => if l { 6 + ea(&src, sz) } else { 4 + ea(&src, sz) },
                (Ea::D(_), _) => if l { if is_reg(&src) || matches!(src, Ea::Imm(_)) { 8 } else { 6 + ea(&src, sz) } } else { 4 + ea(&src, sz) },
                (_, AluOp::Cmp) => if l { 12 + ea(&dst, sz) } else { 8 + ea(&dst, sz) },
                (_, _) if quick => if l { 12 + ea(&dst, sz) } else { 8 + ea(&dst, sz) },
                (_, _) if imm_group => if l { 20 + ea(&dst, sz) } else { 12 + ea(&dst, sz) },
                _ => if l { 12 + ea(&dst, sz) } else { 8 + ea(&dst, sz) },
            }
        }
        Op::AddA { sz, src, .. } | Op::SubA { sz, src, .. } => {
            if group == 5 {
                8 // ADDQ/SUBQ to An
            } else if sz == Sz::L {
                if is_reg(&src) || matches!(src, Ea::Imm(_)) { 8 } else { 6 + ea(&src, sz) }
            } else {
                8 + ea(&src, sz)
            }
        }
        Op::CmpA { sz, src, .. } => 6 + ea(&src, sz),
        Op::LogicSr { .. } => 20,
        Op::Bit { op, num, dst } => {
            let stat = matches!(num, BitNum::Imm(_));
            match (dst, op) {
                (Ea::D(_), BitOp::Tst) => if stat { 10 } else { 6 },
                (Ea::D(_), BitOp::Clr) => if stat { 14 } else { 10 },
                (Ea::D(_), _) => if stat { 12 } else { 8 },
                (_, BitOp::Tst) => (if stat { 8 } else { 4 }) + ea(&dst, Sz::B),
                (_, _) => (if stat { 12 } else { 8 }) + ea(&dst, Sz::B),
            }
        }
        Op::Movep { sz, .. } => if sz == Sz::L { 24 } else { 16 },
        Op::Move { sz, src, dst } => 4 + ea(&src, sz) + move_dst(&dst, sz),
        Op::MoveA { sz, src, .. } => 4 + ea(&src, sz),
        Op::Negx { sz, ea: e } | Op::Clr { sz, ea: e } | Op::Neg { sz, ea: e } | Op::Not { sz, ea: e } => match e {
            Ea::D(_) => if sz == Sz::L { 6 } else { 4 },
            _ => (if sz == Sz::L { 12 } else { 8 }) + ea(&e, sz),
        },
        Op::Tst { sz, ea: e } => 4 + ea(&e, sz),
        Op::MoveFromSr { ea: e } => if is_reg(&e) { 6 } else { 8 + ea(&e, Sz::W) },
        Op::MoveToCcr { ea: e } | Op::MoveToSr { ea: e } => 12 + ea(&e, Sz::W),
        Op::Chk { src, .. } => 10 + ea(&src, Sz::W),
        Op::Lea { src, .. } => control(&src).2,
        Op::Pea { src } => control(&src).3,
        Op::Nbcd { ea: e } => if is_reg(&e) { 6 } else { 8 + ea(&e, Sz::B) },
        Op::Swap { .. } | Op::Ext { .. } => 4,
        Op::Movem { sz, to_mem, mask, ea: e } => {
            let n = mask.count_ones();
            let per = if sz == Sz::L { 8 } else { 4 };
            let base = match (to_mem, e) {
                (false, Ea::Ind(_) | Ea::Post(_)) => 12,
                (false, Ea::Disp(..) | Ea::AbsW(_) | Ea::PcDisp(_)) => 16,
                (false, Ea::Idx(..) | Ea::PcIdx(..)) => 18,
                (false, _) => 20,
                (true, Ea::Ind(_) | Ea::Pre(_)) => 8,
                (true, Ea::Disp(..) | Ea::AbsW(_)) => 12,
                (true, Ea::Idx(..)) => 14,
                (true, _) => 16,
            };
            base + per * n
        }
        Op::Tas { ea: e } => if is_reg(&e) { 4 } else { 10 + ea(&e, Sz::B) },
        Op::Illegal | Op::LineA | Op::LineF | Op::Trap { .. } => 34,
        Op::Link { .. } => 16,
        Op::Unlk { .. } => 12,
        Op::MoveUsp { .. } => 4,
        Op::Reset => 132,
        Op::Nop | Op::Stop { .. } | Op::Trapv => 4,
        Op::Rte | Op::Rtr => 20,
        Op::Rts => 16,
        Op::Jsr { ea: e } => control(&e).1,
        Op::Jmp { ea: e } => control(&e).0,
        Op::Scc { ea: e, .. } => if is_reg(&e) { 5 } else { 8 + ea(&e, Sz::B) },
        Op::Dbcc { .. } => 10,
        Op::Bra { .. } => 10,
        Op::Bsr { .. } => 18,
        Op::Bcc { .. } => if ins.len == 2 { 9 } else { 11 },
        Op::Moveq { .. } => 4,
        Op::Divu { src, .. } => 124 + ea(&src, Sz::W),
        Op::Divs { src, .. } => 142 + ea(&src, Sz::W),
        Op::Mulu { src, .. } | Op::Muls { src, .. } => 54 + ea(&src, Sz::W),
        Op::Abcd { mem, .. } | Op::Sbcd { mem, .. } => if mem { 18 } else { 6 },
        Op::Addx { sz, mem, .. } | Op::Subx { sz, mem, .. } => match (mem, sz == Sz::L) {
            (false, false) => 4,
            (false, true) => 8,
            (true, false) => 18,
            (true, true) => 30,
        },
        Op::Cmpm { sz, .. } => if sz == Sz::L { 20 } else { 12 },
        Op::Exg { .. } => 6,
        Op::ShiftReg { sz, count, .. } => {
            let n = match count {
                ShiftCount::Imm(n) => n as u32,
                ShiftCount::Reg(_) => 4,
            };
            (if sz == Sz::L { 8 } else { 6 }) + 2 * n
        }
        Op::ShiftMem { ea: e, .. } => 8 + ea(&e, Sz::W),
        Op::MoveFromCcr { ea: e } => if is_reg(&e) { 6 } else { 8 + ea(&e, Sz::W) },
        Op::Extb { .. } => 4,
        // 68020 timing, as the `m68k` crate counts it.
        Op::Movec { .. } => 12,
        Op::MulL { src, .. } | Op::DivL { src, .. } => 40 + ea(&src, Sz::L),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode;

    fn c(words: &[u16]) -> u32 {
        let ins = decode(0x1000, &mut |a| words[((a - 0x1000) / 2) as usize]);
        cycles(&ins)
    }

    #[test]
    fn manual_examples() {
        assert_eq!(c(&[0x7001]), 4); // moveq #1,d0
        assert_eq!(c(&[0x3010]), 8); // move.w (a0),d0
        assert_eq!(c(&[0x2290]), 20); // move.l (a0),(a1)
        assert_eq!(c(&[0x23f9, 0, 0, 0, 0]), 36); // move.l abs.l,abs.l
        assert_eq!(c(&[0x5240]), 4); // addq.w #1,d0
        assert_eq!(c(&[0x5280]), 8); // addq.l #1,d0
        assert_eq!(c(&[0x5248]), 8); // addq.w #1,a0
        assert_eq!(c(&[0x0640, 0x0010]), 8); // addi.w #$10,d0
        assert_eq!(c(&[0x0680, 0, 0x10]), 16); // addi.l #$10,d0
        assert_eq!(c(&[0x0c40, 0x0010]), 8); // cmpi.w #$10,d0
        assert_eq!(c(&[0x0c80, 0, 0x10]), 14); // cmpi.l #$10,d0
        assert_eq!(c(&[0xd041]), 4); // add.w d1,d0
        assert_eq!(c(&[0xd081]), 8); // add.l d1,d0
        assert_eq!(c(&[0xd090]), 14); // add.l (a0),d0
        assert_eq!(c(&[0xd150]), 12); // add.w d0,(a0)
        assert_eq!(c(&[0x4e75]), 16); // rts
        assert_eq!(c(&[0x4eb9, 0, 0]), 20); // jsr abs.l
        assert_eq!(c(&[0x41e8, 0x0010]), 8); // lea 16(a0),a0
        assert_eq!(c(&[0x48e7, 0xc0c0]), 40); // movem.l d0-d1/a0-a1,-(a7): 8 + 8 * 4
        assert_eq!(c(&[0x51c8, 0xfffe]), 10); // dbra d0
        assert_eq!(c(&[0xe548]), 10); // lsl.w #2,d0
        assert_eq!(c(&[0x4a68, 0x0004]), 12); // tst.w 4(a0)
        assert_eq!(c(&[0x0828, 0x0003, 0x0004]), 16); // btst #3,4(a0)
    }
}
