//! Static code search ("IRA-style"): code in an image that no recorded run entered.
//!
//! A run only reaches the code its input reaches (this level, not the others; no game over, no
//! menus). This pass looks for the rest in the bytes themselves, starting from the entries and
//! seeds already known and spreading through what [`crate::discover`] already follows (direct
//! branches, calls, jumps) plus three kinds of places discovery cannot see:
//!
//! * [`Via::Table`]: runs of at least [`ScanOptions::min_table`] consecutive longs that are each the
//!   address of acceptable code (jump/handler/object tables);
//! * [`Via::Operand`]: addresses loaded by known code (`lea label,An`, `pea label`,
//!   `move.l #label,..`) that are acceptable code (callbacks, interrupt handlers installed by
//!   `move.l #h,$6c`);
//! * [`Via::Gap`] / [`Via::Adjacent`]: in the bytes no known instruction covers, a function that
//!   starts like one (`link`, `movem.l ..,-(sp)`), or any acceptable code that starts exactly where
//!   a known function ends.
//!
//! A candidate is *acceptable* only if the function explored from it decodes as valid 68000
//! (no illegal, line-A or line-F opcode; no mostly-rare instruction mix; no zeroed words), every
//! direct branch lands on a decodable instruction, every direct callee inside the image is itself
//! acceptable, it reaches a plausible terminator (`rts`, `rte`, `rtr`, `jmp`, `bra`), it has at
//! least [`ScanOptions::min_insns`] instructions, it does not overlap known code or data
//! ranges at a different instruction boundary, and it does not touch [`ScanOptions::volatile`] bytes. The search prefers precision: being wrong costs
//! build size and, for a code unit, validity (a unit runs only while every byte it compiled is in
//! memory, so a candidate in bytes the game later overwrites makes its unit invalid until they
//! are restored), never correctness: the unit is a faithful translation of the bytes.
//!
//! The passes repeat (new functions expose new gaps and tables) until nothing is added.
use crate::{discover, explore, has_prologue, Func, Image, Options};
use m68k::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// How a candidate was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Via {
    Table,
    Operand,
    Gap,
    Adjacent,
}

impl Via {
    pub fn name(self) -> &'static str {
        match self {
            Via::Table => "table",
            Via::Operand => "operand",
            Via::Gap => "gap",
            Via::Adjacent => "adjacent",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Fewest instructions of an accepted function found through a table or an operand.
    pub min_insns: usize,
    /// Fewest instructions of an accepted function found in a gap (prologue or adjacent).
    pub min_insns_gap: usize,
    /// Fewest consecutive acceptable pointers that make a table.
    pub min_table: usize,
    /// Passes at most.
    pub max_passes: usize,
    /// Which sources to use (all by default).
    pub tables: bool,
    pub operands: bool,
    pub gaps: bool,
    /// Half-open address ranges whose bytes are known to change while the code would be in use
    /// (variables, buffers): no accepted function may touch them.
    pub volatile: Vec<(u32, u32)>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { min_insns: 4, min_insns_gap: 8, min_table: 2, max_passes: 8, tables: true, operands: true, gaps: true, volatile: Vec::new() }
    }
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub addr: u32,
    pub via: Via,
    /// Instructions in the function explored from it (callees not included).
    pub insns: usize,
}

#[derive(Default, Debug)]
pub struct Report {
    pub accepted: Vec<Candidate>,
    /// Why candidates were turned down, with counts.
    pub rejected: BTreeMap<&'static str, usize>,
    pub passes: usize,
}

impl Report {
    pub fn count(&self, via: Via) -> usize {
        self.accepted.iter().filter(|c| c.via == via).count()
    }
}

/// Instruction extents of the known functions: start -> end.
fn extents(funcs: &BTreeMap<u32, Func>) -> BTreeMap<u32, u32> {
    funcs.values().flat_map(|f| f.insns.values().map(|i| (i.pc, i.next()))).collect()
}

fn overlaps(ext: &BTreeMap<u32, u32>, pc: u32, end: u32) -> bool {
    // a known instruction that starts before `end`, ends after `pc` and does not start at `pc`
    if let Some((&s, &e)) = ext.range(..end).next_back() {
        if e > pc && s != pc {
            // it can only be the last one starting before `end`; earlier ones that reach `pc`
            // are covered by checking the predecessor of `pc` too
            return true;
        }
    }
    if let Some((&s, &e)) = ext.range(..pc).next_back() {
        if e > pc && s != pc {
            return true;
        }
    }
    false
}

fn rare(op: &Op) -> bool {
    matches!(
        op,
        Op::Abcd { .. }
            | Op::Sbcd { .. }
            | Op::Nbcd { .. }
            | Op::Movep { .. }
            | Op::Cmpm { .. }
            | Op::Chk { .. }
            | Op::Reset
            | Op::Stop { .. }
            | Op::MoveUsp { .. }
            | Op::Trapv
            | Op::Rtr
            | Op::Tas { .. }
            | Op::Negx { .. }
            | Op::LogicSr { .. }
    )
}

/// Whether the function explored from `addr` is acceptable as code (see the module comment).
fn check(img: &Image, opts: &Options, ext: &BTreeMap<u32, u32>, volatile: &[(u32, u32)], addr: u32, min: usize) -> Result<Func, &'static str> {
    if addr & 1 != 0 {
        return Err("odd address");
    }
    if img.seg(addr).is_none() || !img.is_code(addr) || img.is_excluded(addr) {
        return Err("not code");
    }
    let f = explore(img, addr, opts);
    if f.insns.is_empty() {
        return Err("does not decode");
    }
    if f.insns.len() > 6000 {
        return Err("runaway");
    }
    let mut rares = 0;
    let mut term = false;
    for i in f.insns.values() {
        match i.op {
            Op::Illegal | Op::LineA | Op::LineF => return Err("illegal opcode"),
            Op::Alu { op: AluOp::Or, sz: Sz::B, src: Ea::Imm(0), dst: Ea::D(0) } => return Err("zeroed words"),
            Op::Rts | Op::Rte | Op::Rtr | Op::Jmp { .. } | Op::Bra { .. } => term = true,
            _ => {}
        }
        if rare(&i.op) {
            rares += 1;
        }
        if overlaps(ext, i.pc, i.next()) {
            return Err("overlaps known code");
        }
        if volatile.iter().any(|&(lo, hi)| i.pc < hi && i.next() > lo) {
            return Err("in bytes that change");
        }
    }
    if rares * 6 > f.insns.len() {
        return Err("implausible instruction mix");
    }
    if f.leaders.iter().any(|l| !f.insns.contains_key(l)) {
        return Err("branch into undecodable bytes");
    }
    for &c in &f.callees {
        if c & 1 != 0 {
            return Err("odd call target");
        }
        // a call into the image must land on code (a callee outside it is another unit or ROM)
        if img.seg(c).is_some() && !crate::plausible_code(img, c) {
            return Err("callee is not code");
        }
    }
    if !term {
        return Err("no terminator");
    }
    if f.insns.len() < min {
        return Err("too short");
    }
    Ok(f)
}

/// The address a known instruction loads (`lea label,An`, `pea label`, `move.l #label,..`).
fn loaded_address(op: &Op) -> Option<u32> {
    match *op {
        Op::Lea { src: Ea::AbsL(a) | Ea::AbsW(a) | Ea::PcDisp(a), .. } => Some(a),
        Op::Pea { src: Ea::AbsL(a) | Ea::AbsW(a) | Ea::PcDisp(a) } => Some(a),
        Op::Move { sz: Sz::L, src: Ea::Imm(a), .. } | Op::MoveA { sz: Sz::L, src: Ea::Imm(a), .. } => Some(a),
        _ => None,
    }
}

/// [`check`], and the same for every function the candidate calls (inside the image, not already
/// known): discovery compiles them with it, so one that is garbage or volatile spoils the candidate.
fn check_closure(img: &Image, opts: &Options, ext: &BTreeMap<u32, u32>, volatile: &[(u32, u32)], addr: u32, min: usize) -> Result<Func, &'static str> {
    let f = check(img, opts, ext, volatile, addr, min)?;
    let mut seen: BTreeSet<u32> = BTreeSet::from([addr]);
    let mut work: Vec<u32> = f.callees.iter().copied().collect();
    while let Some(c) = work.pop() {
        if !seen.insert(c) || ext.contains_key(&c) || img.seg(c).is_none() {
            continue;
        }
        if seen.len() > 256 {
            return Err("call tree too large");
        }
        match check(img, opts, ext, volatile, c, 1) {
            Ok(g) => work.extend(g.callees.iter().copied()),
            Err(_) => return Err("callee rejected"),
        }
    }
    Ok(f)
}

struct Scan<'a> {
    img: &'a Image,
    opts: &'a Options,
    so: &'a ScanOptions,
    seeds: Vec<u32>,
    funcs: BTreeMap<u32, Func>,
    ext: BTreeMap<u32, u32>,
    /// check() results by (address, minimum), for the addresses tried this pass
    cache: HashMap<(u32, usize), Result<usize, &'static str>>,
    rep: Report,
}

impl Scan<'_> {
    fn known(&self, a: u32) -> bool {
        self.ext.contains_key(&a)
    }

    fn try_addr(&mut self, a: u32, min: usize) -> bool {
        if let Some(r) = self.cache.get(&(a, min)) {
            return r.is_ok();
        }
        let r = check_closure(self.img, self.opts, &self.ext, &self.so.volatile, a, min).map(|f| f.insns.len());
        self.cache.insert((a, min), r);
        r.is_ok()
    }

    fn reject(&mut self, why: &'static str) {
        *self.rep.rejected.entry(why).or_default() += 1;
    }

    /// Accepts candidates that passed `try_addr`; returns whether any was new.
    fn accept(&mut self, found: &[(u32, Via)]) -> bool {
        let mut any = false;
        for &(a, via) in found {
            if self.known(a) || self.seeds.contains(&a) {
                continue;
            }
            // re-check against the extents as they are now (an earlier candidate of this pass may
            // have taken the bytes)
            let min = if matches!(via, Via::Gap | Via::Adjacent) { self.so.min_insns_gap } else { self.so.min_insns };
            match check_closure(self.img, self.opts, &self.ext, &self.so.volatile, a, min) {
                Ok(f) => {
                    self.rep.accepted.push(Candidate { addr: a, via, insns: f.insns.len() });
                    self.seeds.push(a);
                    for i in f.insns.values() {
                        self.ext.insert(i.pc, i.next());
                    }
                    any = true;
                }
                Err(w) => self.reject(w),
            }
        }
        any
    }

    fn tables(&mut self) -> Vec<(u32, Via)> {
        let mut out = Vec::new();
        let img = self.img;
        for s in img.segs.iter().filter(|s| s.code) {
            let (lo, hi) = (s.base, s.base + s.data.len() as u32);
            let mut a = lo;
            while a + 4 <= hi {
                // a run of acceptable pointers from `a`
                let mut run = Vec::new();
                let mut p = a;
                while p + 4 <= hi && !self.ext.range(..p + 4).next_back().map_or(false, |(_, &e)| e > p) {
                    let v = match img.long(p) {
                        Some(v) => v,
                        None => break,
                    };
                    if v < lo || v >= hi || v & 1 != 0 || !self.try_addr(v, self.so.min_insns) {
                        break;
                    }
                    run.push(v);
                    p += 4;
                }
                if run.len() >= self.so.min_table {
                    out.extend(run.iter().map(|&v| (v, Via::Table)));
                    a = p;
                } else {
                    a += 2;
                }
            }
        }
        out
    }

    fn operands(&mut self) -> Vec<(u32, Via)> {
        let mut seen = BTreeSet::new();
        let loads: Vec<u32> = self.funcs.values().flat_map(|f| f.insns.values().filter_map(|i| loaded_address(&i.op))).collect();
        let mut out = Vec::new();
        for a in loads {
            if !seen.insert(a) || self.known(a) || a & 1 != 0 || img_range(self.img, a).is_none() {
                continue;
            }
            if self.try_addr(a, self.so.min_insns) {
                out.push((a, Via::Operand));
            }
        }
        out
    }

    fn gaps(&mut self) -> Vec<(u32, Via)> {
        let mut out = Vec::new();
        let img = self.img;
        // where known functions end: any acceptable code starting there is adjacent
        let ends: BTreeSet<u32> = self
            .funcs
            .values()
            .flat_map(|f| f.insns.values().filter(|i| crate::emit::is_terminator(&i.op)).map(|i| i.next()))
            .collect();
        for s in img.segs.iter().filter(|s| s.code) {
            let (lo, hi) = (s.base, s.base + s.data.len() as u32);
            let mut a = lo;
            while a + 2 <= hi {
                if self.ext.range(..=a).next_back().map_or(false, |(_, &e)| e > a) {
                    a += 2;
                    continue;
                }
                let prologue = has_prologue(img, a);
                let adjacent = ends.contains(&a);
                if (prologue || adjacent) && self.try_addr(a, self.so.min_insns_gap) {
                    out.push((a, if prologue { Via::Gap } else { Via::Adjacent }));
                    // skip the body so a prologue inside it is not another start
                    let f = explore(img, a, self.opts);
                    let end = f.insns.values().map(|i| i.next()).max().unwrap_or(a + 2);
                    a = end.max(a + 2);
                    continue;
                }
                a += 2;
            }
        }
        out
    }
}

fn img_range(img: &Image, a: u32) -> Option<()> {
    img.seg(a).filter(|s| s.code).map(|_| ())
}

/// Searches `img` for code beyond what `seeds` reach. Returns the accepted candidates (to be added
/// to the seeds) and why the others were turned down.
pub fn scan(img: &Image, seeds: &[u32], opts: &Options, so: &ScanOptions) -> Report {
    let funcs = discover(img, seeds, opts);
    let ext = extents(&funcs);
    let mut s = Scan { img, opts, so, seeds: seeds.to_vec(), funcs, ext, cache: HashMap::new(), rep: Report::default() };
    for pass in 0..so.max_passes {
        s.cache.clear();
        let mut any = false;
        // the order is the confidence order: pointers in tables, then addresses code loads, then gaps
        for src in 0..3 {
            let found = match src {
                0 if so.tables => s.tables(),
                1 if so.operands => s.operands(),
                2 if so.gaps => s.gaps(),
                _ => continue,
            };
            let new = s.accept(&found);
            if new {
                // the accepted functions' callees and branch targets, and their own loads
                s.funcs = discover(img, &s.seeds, opts);
                s.ext = extents(&s.funcs);
                s.cache.clear();
                any = true;
            }
        }
        s.rep.passes = pass + 1;
        if !any {
            break;
        }
    }
    s.rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Segment;

    const BASE: u32 = 0x1000;

    fn image(words: &[(u32, &[u16])], len: usize) -> Image {
        let mut data = vec![0u8; len];
        for (at, ws) in words {
            for (i, w) in ws.iter().enumerate() {
                let o = (*at - BASE) as usize + 2 * i;
                data[o..o + 2].copy_from_slice(&w.to_be_bytes());
            }
        }
        Image { segs: vec![Segment { name: "t".into(), base: BASE, data, code: true }], ..Default::default() }
    }

    // moveq #1,d0 ; moveq #2,d1 ; add.l d1,d0 ; move.l d0,d2 ; rts
    const FUNC: [u16; 5] = [0x7001, 0x7202, 0xD081, 0x2400, 0x4E75];

    fn addrs(r: &Report) -> Vec<u32> {
        let mut v: Vec<u32> = r.accepted.iter().map(|c| c.addr).collect();
        v.sort();
        v
    }

    #[test]
    fn finds_functions_in_a_pointer_table() {
        // known entry $1000: rts. Table at $1010: $1100, $1120, $1140 (functions) and $1004 (an odd+junk)
        let t: [u16; 8] = [0, 0x1100, 0, 0x1120, 0, 0x1140, 0, 0x1001];
        let img = image(&[(0x1000, &[0x4E75]), (0x1010, &t), (0x1100, &FUNC), (0x1120, &FUNC), (0x1140, &FUNC)], 0x200);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions { gaps: false, operands: false, ..Default::default() });
        assert_eq!(addrs(&r), vec![0x1100, 0x1120, 0x1140], "{:?}", r);
        assert!(r.accepted.iter().all(|c| c.via == Via::Table));
    }

    #[test]
    fn a_lone_pointer_is_not_a_table() {
        let img = image(&[(0x1000, &[0x4E75]), (0x1010, &[0, 0x1100]), (0x1100, &FUNC)], 0x200);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions { gaps: false, operands: false, ..Default::default() });
        assert!(r.accepted.is_empty(), "{:?}", r);
    }

    #[test]
    fn rejects_garbage_and_illegal_opcodes() {
        // line-F opcode in the middle, and a function whose bytes are zeros
        let bad: [u16; 5] = [0x7001, 0xF000, 0xD081, 0x2400, 0x4E75];
        let img = image(&[(0x1000, &[0x4E75]), (0x1010, &[0, 0x1100, 0, 0x1120]), (0x1100, &bad)], 0x200);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions::default());
        assert!(r.accepted.is_empty(), "{:?}", r);
    }

    #[test]
    fn needs_a_terminator_and_enough_instructions() {
        let short: [u16; 2] = [0x7001, 0x4E75];
        let runs_off: [u16; 4] = [0x7001, 0x7202, 0xD081, 0x2400];
        let img = image(&[(0x1000, &[0x4E75]), (0x1010, &[0, 0x1100, 0, 0x1120]), (0x1100, &short), (0x1120, &runs_off)], 0x130);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions::default());
        assert!(r.accepted.is_empty(), "{:?}", r);
    }

    #[test]
    fn finds_a_function_loaded_by_known_code() {
        // $1000: lea $1100.l,a0 ; jsr (a0) ; rts   -- the callee is only reachable through a0
        let known: [u16; 5] = [0x41F9, 0x0000, 0x1100, 0x4E90, 0x4E75];
        let img = image(&[(0x1000, &known), (0x1100, &FUNC)], 0x200);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions::default());
        assert_eq!(addrs(&r), vec![0x1100], "{:?}", r);
        assert_eq!(r.accepted[0].via, Via::Operand);
    }

    #[test]
    fn finds_a_gap_function_with_a_prologue_but_not_zeros() {
        // link a6,#-4 ; moveq ; moveq ; add ; move ; move ; move ; unlk a6 ; rts  (8+ instructions)
        let f: [u16; 10] = [0x4E56, 0xFFFC, 0x7001, 0x7202, 0xD081, 0x2400, 0x2600, 0x2800, 0x4E5E, 0x4E75];
        let img = image(&[(0x1000, &[0x4E75]), (0x1080, &f)], 0x200);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions::default());
        assert_eq!(addrs(&r), vec![0x1080], "{:?}", r);
        assert_eq!(r.accepted[0].via, Via::Gap);
    }

    #[test]
    fn code_after_a_known_function_is_adjacent() {
        // known: $1000 moveq ; rts.  Right after: ten plain instructions and an rts
        let f: [u16; 11] = [0x7001, 0x7202, 0xD081, 0x2400, 0x2600, 0x2800, 0xD282, 0xD483, 0x7405, 0x7607, 0x4E75];
        let img = image(&[(0x1000, &[0x7000, 0x4E75]), (0x1004, &f)], 0x200);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions::default());
        assert_eq!(addrs(&r), vec![0x1004], "{:?}", r);
        assert_eq!(r.accepted[0].via, Via::Adjacent);
    }

    #[test]
    fn does_not_start_in_the_middle_of_known_code() {
        // $1000: move.l #$7001D081,d0 ; rts.  The immediate's halves decode as code ($1002 = moveq #1,d0 ...)
        let known: [u16; 4] = [0x203C, 0x7001, 0xD081, 0x4E75];
        let t: [u16; 4] = [0, 0x1002, 0, 0x1002];
        let img = image(&[(0x1000, &known), (0x1010, &t)], 0x200);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions::default());
        assert!(r.accepted.is_empty(), "{:?}", r);
    }

    #[test]
    fn callees_of_found_code_must_be_acceptable_too() {
        // $1100: bsr.w to $1200 (line-F garbage) then moveq... rts
        let f: [u16; 6] = [0x6100, 0x00FE, 0x7001, 0x7202, 0xD081, 0x4E75];
        let img = image(&[(0x1000, &[0x4E75]), (0x1010, &[0, 0x1100, 0, 0x1100]), (0x1100, &f), (0x1200, &[0xF000, 0x4E75])], 0x300);
        let r = scan(&img, &[0x1000], &Options::default(), &ScanOptions::default());
        // $1100 itself is fine as a function (its callee is not a seed); the callee, entered by the
        // call, decodes to a line-F opcode, so the call is rejected
        assert!(r.accepted.is_empty(), "{:?}", r);
    }

    #[test]
    fn volatile_bytes_are_never_used() {
        let t: [u16; 8] = [0, 0x1100, 0, 0x1120, 0, 0x1140, 0, 0x1100];
        let img = image(&[(0x1000, &[0x4E75]), (0x1010, &t), (0x1100, &FUNC), (0x1120, &FUNC), (0x1140, &FUNC)], 0x200);
        // the middle function's bytes change while the code would be in use
        let so = ScanOptions { gaps: false, operands: false, volatile: vec![(0x1124, 0x1126)], ..Default::default() };
        let r = scan(&img, &[0x1000], &Options::default(), &so);
        assert_eq!(addrs(&r), vec![0x1100, 0x1140], "the volatile function is left out, the pair after it is a table: {:?}", r);
    }

    #[test]
    fn a_volatile_callee_spoils_its_caller() {
        // $1100: bsr.w $1200 ; moveq ; moveq ; add ; rts, with the callee in volatile bytes
        let f: [u16; 6] = [0x6100, 0x00FE, 0x7001, 0x7202, 0xD081, 0x4E75];
        let img = image(&[(0x1000, &[0x4E75]), (0x1010, &[0, 0x1100, 0, 0x1100]), (0x1100, &f), (0x1200, &FUNC)], 0x300);
        let ok = scan(&img, &[0x1000], &Options::default(), &ScanOptions::default());
        assert_eq!(addrs(&ok).first(), Some(&0x1100), "{:?}", ok);
        let so = ScanOptions { volatile: vec![(0x1200, 0x1202)], ..Default::default() };
        let r = scan(&img, &[0x1000], &Options::default(), &so);
        assert!(r.accepted.is_empty(), "{:?}", r);
    }
}
