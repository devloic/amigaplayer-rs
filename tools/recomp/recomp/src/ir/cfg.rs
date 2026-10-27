//! Functions as control-flow graphs over IR instructions.
//!
//! Blocks are cut exactly where the instruction emitter cuts them (every leader of the discovered
//! [`Func`]; after branches, calls, jumps and terminators), so a block is also the unit of the
//! instruction budget, and every block start is a dispatch entry.
use super::{lift, IrInsn, ALL};
use crate::emit::is_terminator;
use crate::{Func, Image};
use m68k::*;
use std::collections::{BTreeMap, BTreeSet};

/// A control-flow edge out of a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Falls through to the next block (block ended at a label).
    Fall(u32),
    /// Conditional branch (Bcc/DBcc) taken.
    Branch(u32),
    /// Unconditional branch inside the function.
    Jump(u32),
    /// Jump table entry (index, target), statically resolved.
    Table(usize, u32),
    /// Jump to another function's entry (tail call).
    TailCall(u32),
    /// Static call (BSR/JSR abs/pc) to `target`, returning to `resume`.
    Call { target: u32, resume: u32 },
    /// Computed call (library vector, stub, function pointer) returning to `resume`.
    DynCall { resume: u32 },
    /// Computed jump with no known targets: leaves the function to the dispatcher.
    DynJump,
    /// RTS/RTE/RTR.
    Return,
    /// TRAP: the handler normally returns to `resume`.
    Trap { resume: u32 },
    /// Exception, STOP, or leaving the decoded code: back to the dispatcher.
    Exit,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub start: u32,
    pub insns: Vec<IrInsn>,
    pub edges: Vec<Edge>,
}

impl Block {
    pub fn end(&self) -> u32 {
        self.insns.last().map(|i| i.instr.next()).unwrap_or(self.start)
    }
}

#[derive(Clone, Debug)]
pub struct IrFunc {
    pub entry: u32,
    /// (section index, offset of the entry in it), when the section bases are known.
    pub key: Option<(usize, u32)>,
    pub blocks: BTreeMap<u32, Block>,
    /// Block starts that are entry points of this function (all leaders with code: returns into
    /// the middle, exception returns and computed jumps may enter there); `entry` is first.
    pub aliases: Vec<u32>,
    /// Leaders without decodable code (left to the dispatcher).
    pub dead_labels: BTreeSet<u32>,
    pub callees: BTreeSet<u32>,
    pub jump_tables: Vec<(u32, Vec<u32>)>,
    pub dyn_jumps: u32,
    pub dyn_calls: u32,
}

impl IrFunc {
    /// The block labels (as the instruction emitter sees them).
    pub fn labels(&self) -> BTreeSet<u32> {
        let mut s: BTreeSet<u32> = self.blocks.keys().copied().collect();
        s.extend(self.dead_labels.iter().copied());
        s
    }
    pub fn insn_count(&self) -> usize {
        self.blocks.values().map(|b| b.insns.len()).sum()
    }
}

#[derive(Clone, Debug, Default)]
pub struct IrProgram {
    pub bases: Vec<u32>,
    pub funcs: BTreeMap<u32, IrFunc>,
}

fn ends_block(op: &Op) -> bool {
    is_terminator(op) || matches!(op, Op::Bcc { .. } | Op::Dbcc { .. } | Op::Bsr { .. } | Op::Jsr { .. } | Op::Jmp { .. })
}

/// Section index and offset of `a`, given the section bases (the highest base <= a).
pub fn key_of(bases: &[u32], a: u32) -> Option<(usize, u32)> {
    let mut best: Option<(usize, u32)> = None;
    for (i, &b) in bases.iter().enumerate() {
        if a >= b && best.map_or(true, |(_, o)| a - b < o) {
            best = Some((i, a - b));
        }
    }
    best
}

/// Builds the IR of one discovered function.
pub fn build(f: &Func, bases: &[u32]) -> IrFunc {
    let labels = &f.leaders;
    let tables: BTreeMap<u32, &Vec<u32>> = f.jump_tables.iter().map(|(a, t)| (*a, t)).collect();
    let mut blocks = BTreeMap::new();
    let mut dead = BTreeSet::new();
    for &l in labels {
        if !f.insns.contains_key(&l) {
            dead.insert(l);
            continue;
        }
        let mut insns: Vec<IrInsn> = Vec::new();
        let mut p = l;
        let mut edges = Vec::new();
        while let Some(ins) = f.insns.get(&p) {
            insns.push(lift(ins));
            p = ins.next();
            if ends_block(&ins.op) {
                break;
            }
            if labels.contains(&p) {
                break;
            }
        }
        let last = insns.last().unwrap().instr;
        let next = last.next();
        match last.op {
            Op::Bcc { target, .. } | Op::Dbcc { target, .. } => {
                edges.push(Edge::Branch(target));
                edges.push(Edge::Fall(next));
            }
            Op::Bra { target } => edges.push(if labels.contains(&target) { Edge::Jump(target) } else { Edge::TailCall(target) }),
            Op::Bsr { target } => edges.push(Edge::Call { target, resume: next }),
            Op::Jsr { ea: Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) } => edges.push(Edge::Call { target: t, resume: next }),
            Op::Jsr { .. } => edges.push(Edge::DynCall { resume: next }),
            Op::Jmp { ea: Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) } => edges.push(if labels.contains(&t) { Edge::Jump(t) } else { Edge::TailCall(t) }),
            Op::Jmp { .. } => match tables.get(&last.pc) {
                Some(ts) => edges.extend(ts.iter().enumerate().map(|(i, &t)| Edge::Table(i, t))),
                None => edges.push(Edge::DynJump),
            },
            Op::Rts | Op::Rte | Op::Rtr => edges.push(Edge::Return),
            Op::Trap { .. } => edges.push(Edge::Trap { resume: next }),
            _ if is_terminator(&last.op) => edges.push(Edge::Exit),
            _ => edges.push(if labels.contains(&next) && f.insns.contains_key(&next) { Edge::Fall(next) } else { Edge::Exit }),
        }
        fuse(&mut insns);
        blocks.insert(l, Block { start: l, insns, edges });
    }
    liveness(&mut blocks);
    let mut aliases = vec![f.entry];
    aliases.extend(blocks.keys().copied().filter(|&a| a != f.entry));
    IrFunc {
        entry: f.entry,
        key: key_of(bases, f.entry),
        blocks,
        aliases,
        dead_labels: dead,
        callees: f.callees.clone(),
        jump_tables: f.jump_tables.clone(),
        dyn_jumps: f.dyn_jumps,
        dyn_calls: f.dyn_calls,
    }
}

/// Fuses conditions with the instruction that set their flags: for each Bcc/DBcc/Scc, the
/// nearest earlier instruction of the block that touches any flag; if it is a CMP/SUB, ADD,
/// logic op/MOVE/TST or bit test that determines the condition ([`IrInsn::cond_source`]), the
/// condition is computed from its operands and reads no flags. Only flag-neutral instructions
/// (no flag written or read: MOVEA, LEA, ADDA, MOVEM, ...) may lie between, so the flags the
/// condition would read are exactly the producer's. Whether the producer still computes its flags
/// is then up to the liveness pass, as for any other definition.
pub fn fuse(insns: &mut [IrInsn]) {
    for j in 0..insns.len() {
        let c = match insns[j].cond() {
            Some(c) if super::cond_flags(c) != 0 => c,
            _ => continue,
        };
        for k in (0..j).rev() {
            if insns[k].flag_defs == 0 && insns[k].flag_uses == 0 {
                continue;
            }
            if let Some((src, a, b)) = insns[k].cond_source(c) {
                insns[k].export = Some((a, b));
                insns[j].fused = Some(super::Fused { from: k, src });
                insns[j].flag_uses = 0;
            }
            break;
        }
    }
}

/// Flags read at the start of a block, given the flags read after it.
fn transfer(b: &Block, mut live: u8) -> u8 {
    for i in b.insns.iter().rev() {
        live = (live & !i.flag_defs) | i.flag_uses;
    }
    live
}

/// Flags read after leaving `b` along its edges. Only the edges that stay in native code of the
/// same function (`pc = target; continue`) see the target's needs; everything else (calls, computed
/// jumps and calls, jump tables -- whose computed target may also miss every block --, returns,
/// traps, exits, tail calls and jumps to leaders without code) leaves to code that may read any
/// flag, so all flags are live there.
fn live_out(b: &Block, live_in: &BTreeMap<u32, u8>) -> u8 {
    let mut live = 0;
    for e in &b.edges {
        live |= match e {
            Edge::Fall(t) | Edge::Branch(t) | Edge::Jump(t) => live_in.get(t).copied().unwrap_or(ALL),
            _ => ALL,
        };
    }
    live
}

/// Flag liveness across the blocks of a function (backward, to a fixed point), then per
/// instruction: `live_after` from the blocks' successors, and `live_exit` with all flags live at
/// the block's end (what a dispatcher exit at the next block start would see).
pub fn liveness(blocks: &mut BTreeMap<u32, Block>) {
    let mut live_in: BTreeMap<u32, u8> = blocks.keys().map(|&a| (a, 0u8)).collect();
    loop {
        let mut changed = false;
        for (a, b) in blocks.iter().rev() {
            let v = transfer(b, live_out(b, &live_in));
            let e = live_in.get_mut(a).unwrap();
            if *e | v != *e {
                *e |= v;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let outs: Vec<(u32, u8)> = blocks.values().map(|b| (b.start, live_out(b, &live_in))).collect();
    for (a, out) in outs {
        block_liveness(blocks.get_mut(&a).unwrap(), out);
    }
}

/// Sets the liveness masks of a block's instructions, given the flags read after it.
pub fn block_liveness(b: &mut Block, out: u8) {
    let (mut live, mut exit) = (out, ALL);
    for i in b.insns.iter_mut().rev() {
        i.set_live(live, exit);
        live = (live & !i.flag_defs) | i.flag_uses;
        exit = (exit & !i.flag_defs) | i.flag_uses;
    }
}

/// Lifts every discovered function.
pub fn build_program(funcs: &BTreeMap<u32, Func>, img: &Image) -> IrProgram {
    let bases: Vec<u32> = img.segs.iter().map(|s| s.base).collect();
    IrProgram { funcs: funcs.values().map(|f| (f.entry, build(f, &bases))).collect(), bases }
}

/// Instruction and flag statistics of a program.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiftStats {
    pub instructions: usize,
    /// Lazily flagged micro-ops: computed with flags, guarded by the budget test, plain.
    pub exact: usize,
    pub guarded: usize,
    pub plain: usize,
}

pub fn lift_stats(p: &IrProgram) -> LiftStats {
    let mut n = LiftStats::default();
    for f in p.funcs.values() {
        for b in f.blocks.values() {
            for i in &b.insns {
                n.instructions += 1;
                for op in i.ops() {
                    if op.lazy().is_some() {
                        match op.flag_mode() {
                            super::FlagMode::Exact => n.exact += 1,
                            super::FlagMode::Guarded => n.guarded += 1,
                            super::FlagMode::Plain => n.plain += 1,
                        }
                    }
                }
            }
        }
    }
    n
}
