//! Self-modifying code protection.
//!
//! Generated code is a snapshot of the program's instructions at recompile time. [`CodeGuard`]
//! wraps any [`Bus`] and watches writes into the bytes each compiled function was built from; a
//! write there *invalidates* the function, and [`crate::runtime::run_with`] (given the guard's
//! [`SmcHandle`] in [`crate::runtime::RunOptions::smc`]) then interprets that code instead of
//! calling the stale native version. If the bytes are later restored to their original values the
//! function becomes valid again.
//!
//! # Cost
//!
//! Reads are forwarded untouched. Each write does one bit test in a 256-byte-page bitmap (two for
//! accesses that cross a page); only writes into a page that holds compiled code take the slow
//! path, which compares the written bytes with the exact instruction ranges of the functions on
//! that page. The dispatcher pays one `Cell<bool>` load per dispatch until the first invalidation.
//!
//! # What is guaranteed
//!
//! Generated code cannot be interrupted from inside a bus write (it only returns to the dispatcher
//! at its own exits and at the budget check at the top of every basic block), so the guarantee
//! depends on where the write happened:
//!
//! * **Every dispatch after an invalidating write is correct**: invalidated functions are never
//!   called again (until their bytes match the original again), and once anything is invalid,
//!   every function that could reach invalid code without going through the dispatcher (a direct
//!   `bsr`/`jsr` to an invalid or itself-affected function, or any computed call, which the
//!   generated code resolves with its own lookup) runs *block-stepped*: one basic block per call
//!   (by lowering `cpu.budget` to `ticks + 1`), so direct calls return to the dispatcher before the
//!   callee's first block and are checked there.
//! * **Static writers** (found by [`FuncInfo::from_lookup`]: a store to an absolute address inside
//!   compiled code, or an operand address formed as a constant): block-stepped from the start, so
//!   their patches are honoured from the next basic block on. A function that stores into its
//!   *own* code that way ([`FuncInfo::patches_self`]) is always interpreted, so even a patch of the
//!   next instruction in the same block is exact.
//! * **Writes made by interpreted code, or by block-stepped native code**: exact from the next
//!   basic block on. Instructions after the store *in the same basic block* still run the
//!   compiled (old) version; such a write is recorded with `stale_risk = true`.
//! * **Dynamic writes (computed address) made by full-speed native code before anything was
//!   invalid** (policy [`SmcPolicy::Adaptive`]): the native call that made the write continues
//!   with compiled code until it returns to the dispatcher (including code it calls directly).
//!   Each such write is recorded as an [`SmcEvent`] with `stale_risk = true`.
//!   [`SmcPolicy::Strict`] block-steps every native call from the start (slower) and bounds this
//!   to the rest of the basic block.
//!
//! So with `Strict` the only inexact case is a dynamic write that patches an instruction later in
//! the same basic block, and every inexact case is reported in the events.
//!
//! Writes that bypass the guard (made on `guard.inner` directly) are not seen.
use crate::bus::{Bus, ADDR_MASK};
use crate::decode::decode;
use crate::types::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

pub const PAGE_SHIFT: u32 = 8;
const PAGE: usize = 1 << PAGE_SHIFT;
const NPAGES: usize = 1 << (24 - PAGE_SHIFT);

/// The bytes one compiled function was built from, and how it can reach other native code
/// without going through the dispatcher.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FuncInfo {
    pub entry: u32,
    /// Half-open byte ranges `[start, end)`: its instructions and any jump-table data compiled
    /// into it.
    pub ranges: Vec<(u32, u32)>,
    /// Targets of `bsr`/`jsr abs` (compiled as direct Rust calls).
    pub callees: Vec<u32>,
    /// Has computed calls (`jsr (An)`, `jsr d16(An)`, ...): generated code resolves those with its
    /// own lookup, so they can reach any function.
    pub dyn_calls: bool,
    /// Statically a self-modifying-code writer: it stores to an absolute address inside compiled
    /// code, or forms (`lea`/`pea`/`movea #`/`move.l #`) an address inside compiled code that is
    /// not the start of an instruction (an operand). Such functions are block-stepped from the
    /// start under [`SmcPolicy::Adaptive`], so their patches take effect at the next block.
    pub writes_code: bool,
    /// Stores to an absolute address inside its *own* code: such a patch can land later in the
    /// same basic block, which compiled code cannot honour, so the function is always
    /// interpreted while guarded (both policies).
    pub patches_self: bool,
}

impl FuncInfo {
    /// Rebuilds the function table from a generated crate's own data: `entries` (its `FUNCTIONS`),
    /// `native(pc)` (its `lookup`, as a pointer identity) probed over `[lo, hi)` to find every
    /// dispatch entry, and the program bytes (`fetch`). Each function's ranges are found by
    /// recursive descent from its entry and from every dispatch entry the lookup maps to it, the
    /// same intra-procedural edges the recompiler follows (branches, fall-through after calls and
    /// traps, `move.w tbl(pc,Dn.w),Dn ; jmp base(pc,Dn.w)` jump tables including the table data).
    /// This over-approximates rather than misses code: extra bytes only cause extra invalidations.
    pub fn from_lookup(entries: &[u32], lo: u32, hi: u32, native: &dyn Fn(u32) -> Option<usize>, fetch: &mut dyn FnMut(u32) -> u16) -> Vec<FuncInfo> {
        let mut by_ptr: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, &e) in entries.iter().enumerate() {
            if let Some(p) = native(e) {
                by_ptr.entry(p).or_default().push(i);
            }
        }
        let mut seeds: Vec<Vec<u32>> = entries.iter().map(|&e| vec![e]).collect();
        let mut a = lo & !1;
        while a < hi {
            if let Some(p) = native(a) {
                if let Some(ids) = by_ptr.get(&p) {
                    for &i in ids {
                        if entries[i] != a {
                            seeds[i].push(a);
                        }
                    }
                }
            }
            a += 2;
        }
        let found: Vec<(FuncInfo, Vec<(u32, bool)>, Vec<u32>)> = entries.iter().zip(seeds).map(|(&e, s)| explore(e, &s, lo, hi, fetch)).collect();
        let code = merge(found.iter().flat_map(|f| f.0.ranges.iter().copied()).collect());
        let starts: BTreeSet<u32> = found.iter().flat_map(|f| f.2.iter().copied()).collect();
        let in_code = |a: u32| {
            let i = code.partition_point(|r| r.1 <= a);
            i < code.len() && code[i].0 <= a
        };
        found
            .into_iter()
            .map(|(mut f, refs, _)| {
                f.writes_code = refs.iter().any(|&(a, store)| in_code(a) && (store || !starts.contains(&a)));
                let own = |a: u32| f.ranges.iter().any(|&(s, e)| a >= s && a < e);
                f.patches_self = refs.iter().any(|&(a, store)| store && own(a));
                f
            })
            .collect()
    }
}

/// Addresses an instruction stores to absolutely (`true`) or forms as a constant (`false`).
fn address_refs(op: &Op, out: &mut Vec<(u32, bool)>) {
    let abs = |e: &Ea| match *e {
        Ea::AbsW(a) | Ea::AbsL(a) => Some(a & ADDR_MASK),
        _ => None,
    };
    let store = match *op {
        Op::Alu { op, dst, .. } if op != AluOp::Cmp => abs(&dst),
        Op::Bit { op, dst, .. } if op != BitOp::Tst => abs(&dst),
        Op::Move { dst, .. } => abs(&dst),
        Op::Negx { ea, .. } | Op::Clr { ea, .. } | Op::Neg { ea, .. } | Op::Not { ea, .. } | Op::Nbcd { ea } | Op::Tas { ea } | Op::Scc { ea, .. } | Op::ShiftMem { ea, .. } | Op::MoveFromSr { ea } => abs(&ea),
        Op::Movem { to_mem: true, ea, .. } => abs(&ea),
        _ => None,
    };
    if let Some(a) = store {
        // A word/long store can touch the next bytes too.
        for k in 0..4 {
            out.push((a.wrapping_add(k), true));
        }
    }
    let formed = match *op {
        Op::Lea { src, .. } | Op::Pea { src } => abs(&src),
        Op::MoveA { src: Ea::Imm(v), .. } | Op::Move { sz: Sz::L, src: Ea::Imm(v), .. } => Some(v & ADDR_MASK),
        _ => None,
    };
    if let Some(a) = formed {
        out.push((a, false));
    }
}

/// Recursive descent over intra-procedural edges from `seeds`, staying in `[lo, hi)`.
fn explore(entry: u32, seeds: &[u32], lo: u32, hi: u32, fetch: &mut dyn FnMut(u32) -> u16) -> (FuncInfo, Vec<(u32, bool)>, Vec<u32>) {
    let mut refs = Vec::new();
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut bytes: Vec<(u32, u32)> = Vec::new();
    let mut callees = BTreeSet::new();
    let mut dyn_calls = false;
    let mut work: Vec<u32> = seeds.to_vec();
    let inside = |a: u32| a >= lo && a < hi && a & 1 == 0;
    while let Some(start) = work.pop() {
        let mut p = start;
        let mut prev: Vec<Instr> = Vec::new();
        while inside(p) && seen.insert(p) && seen.len() < 1 << 20 {
            let ins = decode(p, &mut |a| fetch(a));
            if ins.next() > hi {
                break;
            }
            bytes.push((p, ins.next()));
            address_refs(&ins.op, &mut refs);
            let next = ins.next();
            let mut stop = false;
            match ins.op {
                Op::Bcc { target, .. } | Op::Dbcc { target, .. } => {
                    work.push(target);
                    work.push(next);
                    stop = true;
                }
                Op::Bra { target } => {
                    work.push(target);
                    stop = true;
                }
                Op::Bsr { target } => {
                    callees.insert(target);
                    work.push(next);
                    stop = true;
                }
                Op::Jsr { ea } => {
                    match ea {
                        Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) => {
                            callees.insert(t);
                        }
                        _ => dyn_calls = true,
                    }
                    work.push(next);
                    stop = true;
                }
                Op::Jmp { ea } => {
                    if let Ea::PcIdx(base, x) = ea {
                        if let Some((tbl, n)) = jump_table(&prev, x) {
                            bytes.push((tbl, tbl + 2 * n));
                            for i in 0..n {
                                let w = fetch(tbl + 2 * i) as i16 as i32 as u32;
                                work.push(base.wrapping_add(w));
                            }
                        }
                    }
                    stop = true;
                }
                Op::Trap { .. } => {
                    work.push(next);
                    stop = true;
                }
                Op::Rts | Op::Rte | Op::Rtr | Op::Illegal | Op::LineA | Op::LineF | Op::Stop { .. } => stop = true,
                _ => {}
            }
            if stop {
                break;
            }
            prev.push(ins);
            p = next;
        }
    }
    let starts = seen.iter().copied().collect();
    (FuncInfo { entry, ranges: merge(bytes), callees: callees.into_iter().collect(), dyn_calls, writes_code: false, patches_self: false }, refs, starts)
}

/// `cmp #N,Dn ; ... ; move.w tbl(pc,Dn.w),Dn` right before the `jmp base(pc,Dn.w)`.
fn jump_table(prev: &[Instr], x: Xn) -> Option<(u32, u32)> {
    let n = prev.len();
    let tbl = match prev.last()?.op {
        Op::Move { sz: Sz::W, src: Ea::PcIdx(t, x2), dst: Ea::D(r) } if x2 == x && r == x.reg => t,
        _ => return None,
    };
    let count = prev[..n - 1].iter().rev().take(4).find_map(|i| match i.op {
        Op::Alu { op: AluOp::Cmp, sz: Sz::L | Sz::W, src: Ea::Imm(c), dst: Ea::D(r) } if r == x.reg => Some(c),
        _ => None,
    })?;
    if count > 1024 {
        return None;
    }
    Some((tbl, count))
}

fn merge(mut v: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    v.sort();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (s, e) in v {
        match out.last_mut() {
            Some(l) if s <= l.1 => l.1 = l.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmcPolicy {
    /// Full speed until the first invalidation, then block-stepping where needed.
    Adaptive,
    /// Every native call is block-stepped from the start.
    Strict,
}

/// What was running when a write hit compiled code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmcContext {
    /// Not inside `run_with` (set-up code, HLE outside the dispatcher, ...).
    Outside,
    /// The fallback interpreter, executing the instruction at this address.
    Interp(u32),
    /// A full-speed native call entered at this address.
    Native(u32),
    /// A block-stepped native call (one basic block) entered at this address.
    Block(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmcEvent {
    /// Written address and size in bytes.
    pub addr: u32,
    pub size: u8,
    /// Entry of the function whose code was hit.
    pub func: u32,
    pub context: SmcContext,
    /// The write came from full-speed native code, which kept running compiled code until it
    /// returned to the dispatcher.
    pub stale_risk: bool,
}

/// How the dispatcher may run a native function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admit {
    Fast,
    Block,
    Interpret,
}

struct State {
    funcs: Vec<FuncInfo>,
    by_entry: HashMap<u32, usize>,
    page_funcs: HashMap<u32, Vec<usize>>,
    orig: HashMap<u32, Box<[u8; PAGE]>>,
    cur: HashMap<u32, Box<[u8; PAGE]>>,
    invalid: Vec<bool>,
    dirty: Vec<bool>,
    tainted: Vec<bool>,
    ptrs: HashMap<usize, Vec<usize>>,
    ptrs_built: bool,
    events: Vec<SmcEvent>,
    event_count: u64,
    invalidations: u64,
    revalidations: u64,
}

/// Guard state shared by [`CodeGuard`] (which records writes) and the dispatcher.
pub struct SmcShared {
    strict: bool,
    /// Some function is invalid (the dispatcher must look closer).
    active: Cell<bool>,
    ctx: Cell<SmcContext>,
    st: RefCell<State>,
}

pub type SmcHandle = Rc<SmcShared>;

/// Keeps this many events; later ones are only counted.
pub const MAX_EVENTS: usize = 4096;

impl SmcShared {
    pub fn set_context(&self, c: SmcContext) {
        self.ctx.set(c);
    }
    /// True while some function is invalid.
    pub fn any_invalid(&self) -> bool {
        self.st.borrow().invalid.iter().any(|&b| b)
    }
    /// Entries of the functions detected as static code writers.
    pub fn static_writers(&self) -> Vec<u32> {
        self.st.borrow().funcs.iter().filter(|f| f.writes_code).map(|f| f.entry).collect()
    }
    /// Entries of the functions always interpreted because they patch their own code.
    pub fn self_patchers(&self) -> Vec<u32> {
        self.st.borrow().funcs.iter().filter(|f| f.patches_self).map(|f| f.entry).collect()
    }
    /// Entries of the functions currently block-stepped because of SMC (Adaptive policy).
    pub fn block_stepped(&self) -> Vec<u32> {
        let st = self.st.borrow();
        st.funcs.iter().zip(&st.tainted).filter(|(_, &b)| b).map(|(f, _)| f.entry).collect()
    }
    pub fn is_invalid(&self, entry: u32) -> bool {
        let st = self.st.borrow();
        st.by_entry.get(&entry).map_or(false, |&i| st.invalid[i])
    }
    pub fn events(&self) -> Vec<SmcEvent> {
        self.st.borrow().events.clone()
    }
    /// (events, invalidations, revalidations).
    pub fn counts(&self) -> (u64, u64, u64) {
        let st = self.st.borrow();
        (st.event_count, st.invalidations, st.revalidations)
    }
    /// Entries of the functions currently invalid.
    pub fn invalid_functions(&self) -> Vec<u32> {
        let st = self.st.borrow();
        st.funcs.iter().zip(&st.invalid).filter(|(_, &b)| b).map(|(f, _)| f.entry).collect()
    }
    /// Number of events so far (also counts the ones not kept).
    pub fn event_count(&self) -> u64 {
        self.st.borrow().event_count
    }
    /// After a block step over `[lo, hi)`: events since `since` (an earlier `event_count()`) that
    /// wrote into that block may have hit instructions the block still ran in compiled form, so
    /// they are marked `stale_risk`.
    pub fn check_block(&self, since: u64, lo: u32, hi: u32) {
        let mut st = self.st.borrow_mut();
        let kept = st.events.len() as u64;
        let first = (since.min(kept)) as usize;
        for e in &mut st.events[first..] {
            let end = e.addr.wrapping_add(e.size as u32);
            if e.addr < hi && end > lo {
                e.stale_risk = true;
            }
        }
    }
    /// Maps native function pointers to functions (called by `run_with` with its lookup).
    pub fn bind_lookup(&self, native: &dyn Fn(u32) -> Option<usize>) {
        let mut st = self.st.borrow_mut();
        if st.ptrs_built {
            return;
        }
        let mut m: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, f) in st.funcs.iter().enumerate() {
            if let Some(p) = native(f.entry) {
                m.entry(p).or_default().push(i);
            }
        }
        st.ptrs = m;
        st.ptrs_built = true;
    }

    /// How the dispatcher may run the native function `ptr` (from the lookup).
    #[inline]
    pub fn admit(&self, ptr: usize) -> Admit {
        if !self.active.get() {
            return if self.strict { Admit::Block } else { Admit::Fast };
        }
        self.admit_slow(ptr)
    }

    #[cold]
    fn admit_slow(&self, ptr: usize) -> Admit {
        let mut st = self.st.borrow_mut();
        let ids = match st.ptrs.get(&ptr) {
            Some(ids) => ids.clone(),
            None => return Admit::Block, // native code we know nothing about (HLE, other crates)
        };
        if ids.iter().any(|&i| st.funcs[i].patches_self) {
            return Admit::Interpret;
        }
        let mut changed = false;
        for &i in &ids {
            if st.invalid[i] && st.dirty[i] {
                st.dirty[i] = false;
                if st.matches_original(i) {
                    st.invalid[i] = false;
                    st.revalidations += 1;
                    changed = true;
                }
            }
        }
        if changed {
            st.recompute_taint();
            self.active.set(st.needs_checks());
        }
        if ids.iter().any(|&i| st.invalid[i]) {
            Admit::Interpret
        } else if self.strict || ids.iter().any(|&i| st.tainted[i]) {
            Admit::Block
        } else {
            Admit::Fast
        }
    }

    #[cold]
    fn on_write(&self, a: u32, bytes: &[u8]) {
        let mut st = self.st.borrow_mut();
        let ctx = self.ctx.get();
        let mut newly = false;
        let mut hit: Vec<usize> = Vec::new();
        for (k, &b) in bytes.iter().enumerate() {
            let addr = a.wrapping_add(k as u32) & ADDR_MASK;
            let page = addr >> PAGE_SHIFT;
            let off = (addr as usize) & (PAGE - 1);
            match st.cur.get_mut(&page) {
                Some(p) => p[off] = b,
                None => continue,
            }
            let ids = match st.page_funcs.get(&page) {
                Some(v) => v.clone(),
                None => continue,
            };
            for i in ids {
                if !hit.contains(&i) && st.funcs[i].ranges.iter().any(|&(s, e)| addr >= s && addr < e) {
                    hit.push(i);
                }
            }
        }
        for i in hit {
            st.dirty[i] = true;
            if !st.invalid[i] {
                st.invalid[i] = true;
                st.invalidations += 1;
                newly = true;
            }
            st.event_count += 1;
            if st.events.len() < MAX_EVENTS {
                let ev = SmcEvent { addr: a & ADDR_MASK, size: bytes.len() as u8, func: st.funcs[i].entry, context: ctx, stale_risk: matches!(ctx, SmcContext::Native(_)) };
                st.events.push(ev);
            }
        }
        if newly {
            st.recompute_taint();
            self.active.set(true);
        }
    }
}

impl State {
    fn needs_checks(&self) -> bool {
        self.invalid.iter().any(|&b| b) || self.tainted.iter().any(|&b| b) || self.funcs.iter().any(|f| f.patches_self)
    }
    fn matches_original(&self, i: usize) -> bool {
        for &(s, e) in &self.funcs[i].ranges {
            for a in s..e {
                let a = a & ADDR_MASK;
                let page = a >> PAGE_SHIFT;
                let off = (a as usize) & (PAGE - 1);
                if let (Some(o), Some(c)) = (self.orig.get(&page), self.cur.get(&page)) {
                    if o[off] != c[off] {
                        return false;
                    }
                }
            }
        }
        true
    }
    /// tainted = valid but able to reach invalid code (or a static code writer) without passing
    /// through the dispatcher, or a static code writer itself.
    fn recompute_taint(&mut self) {
        let n = self.funcs.len();
        let any = self.invalid.iter().any(|&b| b) || self.funcs.iter().any(|f| f.writes_code || f.patches_self);
        for i in 0..n {
            self.tainted[i] = any && !self.invalid[i] && !self.funcs[i].patches_self && (self.funcs[i].dyn_calls || self.funcs[i].writes_code);
        }
        if !any {
            return;
        }
        loop {
            let mut changed = false;
            for i in 0..n {
                if self.tainted[i] || self.invalid[i] {
                    continue;
                }
                if self.funcs[i].patches_self {
                    continue;
                }
                let t = self.funcs[i].callees.iter().any(|c| match self.by_entry.get(c) {
                    Some(&j) => self.invalid[j] || self.tainted[j] || self.funcs[j].patches_self,
                    None => false, // not compiled: never called directly, the call goes through the dispatcher
                });
                if t {
                    self.tainted[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }
}

/// A [`Bus`] wrapper that invalidates compiled functions when their code bytes are written.
pub struct CodeGuard<B> {
    pub inner: B,
    bits: Box<[u64]>,
    shared: SmcHandle,
}

impl<B: Bus> CodeGuard<B> {
    /// Guards the code of `funcs` (see [`FuncInfo::from_lookup`]); the original bytes are read from
    /// `inner` now.
    pub fn new(mut inner: B, funcs: Vec<FuncInfo>, policy: SmcPolicy) -> CodeGuard<B> {
        let mut bits = vec![0u64; NPAGES / 64].into_boxed_slice();
        let mut page_funcs: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut orig: HashMap<u32, Box<[u8; PAGE]>> = HashMap::new();
        let mut by_entry = HashMap::new();
        for (i, f) in funcs.iter().enumerate() {
            by_entry.entry(f.entry).or_insert(i);
            for &(s, e) in &f.ranges {
                if e <= s {
                    continue;
                }
                let (p0, p1) = ((s & ADDR_MASK) >> PAGE_SHIFT, ((e - 1) & ADDR_MASK) >> PAGE_SHIFT);
                let mut p = p0;
                loop {
                    bits[(p / 64) as usize] |= 1 << (p % 64);
                    let v = page_funcs.entry(p).or_default();
                    if v.last() != Some(&i) {
                        v.push(i);
                    }
                    if !orig.contains_key(&p) {
                        let mut b = Box::new([0u8; PAGE]);
                        for (k, x) in b.iter_mut().enumerate() {
                            *x = inner.read8((p << PAGE_SHIFT) + k as u32);
                        }
                        orig.insert(p, b);
                    }
                    if p == p1 {
                        break;
                    }
                    p = (p + 1) % NPAGES as u32;
                }
            }
        }
        let n = funcs.len();
        let cur = orig.clone();
        let mut st = State {
            funcs, by_entry, page_funcs, orig, cur, invalid: vec![false; n], dirty: vec![false; n], tainted: vec![false; n],
            ptrs: HashMap::new(), ptrs_built: false, events: Vec::new(), event_count: 0, invalidations: 0, revalidations: 0,
        };
        st.recompute_taint();
        let active = st.needs_checks();
        let shared = Rc::new(SmcShared { strict: policy == SmcPolicy::Strict, active: Cell::new(active), ctx: Cell::new(SmcContext::Outside), st: RefCell::new(st) });
        CodeGuard { inner, bits, shared }
    }
    /// The handle to pass to `RunOptions::smc`.
    pub fn handle(&self) -> SmcHandle {
        self.shared.clone()
    }
    #[inline(always)]
    fn guarded(&self, a: u32, len: u32) -> bool {
        let p0 = (a & ADDR_MASK) >> PAGE_SHIFT;
        let p1 = (a.wrapping_add(len - 1) & ADDR_MASK) >> PAGE_SHIFT;
        (self.bits[(p0 / 64) as usize] >> (p0 % 64) | self.bits[(p1 / 64) as usize] >> (p1 % 64)) & 1 != 0
    }
}

impl<B: Bus> Bus for CodeGuard<B> {
    #[inline(always)]
    fn read8(&mut self, a: u32) -> u8 {
        self.inner.read8(a)
    }
    #[inline(always)]
    fn read16(&mut self, a: u32) -> u16 {
        self.inner.read16(a)
    }
    #[inline(always)]
    fn read32(&mut self, a: u32) -> u32 {
        self.inner.read32(a)
    }
    #[inline(always)]
    fn fetch16(&mut self, a: u32) -> u16 {
        self.inner.fetch16(a)
    }
    #[inline(always)]
    fn write8(&mut self, a: u32, v: u8) {
        self.inner.write8(a, v);
        if self.guarded(a, 1) {
            self.shared.on_write(a, &[v]);
        }
    }
    #[inline(always)]
    fn write16(&mut self, a: u32, v: u16) {
        self.inner.write16(a, v);
        if self.guarded(a, 2) {
            self.shared.on_write(a, &v.to_be_bytes());
        }
    }
    #[inline(always)]
    fn write32(&mut self, a: u32, v: u32) {
        self.inner.write32(a, v);
        if self.guarded(a, 4) {
            self.shared.on_write(a, &v.to_be_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cpu, FlatMem};

    fn nf(_c: &mut Cpu, _b: &mut CodeGuard<FlatMem>) {}

    #[test]
    fn invalidate_and_revalidate() {
        let mut m = FlatMem::default();
        m.load(0x1000, &[0x4e, 0x71, 0x4e, 0x75]);
        let f = FuncInfo { entry: 0x1000, ranges: vec![(0x1000, 0x1004)], ..Default::default() };
        let mut g = CodeGuard::new(m, vec![f], SmcPolicy::Adaptive);
        let h = g.handle();
        let p = nf as NativeFnPtr as usize;
        h.bind_lookup(&|pc| if pc == 0x1000 { Some(p) } else { None });
        assert_eq!(h.admit(p), Admit::Fast);
        g.write32(0x1004, 0); // same page, outside the function
        g.write8(0x0fff, 1);
        assert_eq!(h.admit(p), Admit::Fast);
        assert_eq!(h.counts(), (0, 0, 0));
        g.write16(0x1003, 0x7612); // last byte of the function changes: invalidates
        assert_eq!(h.admit(p), Admit::Interpret);
        assert_eq!(h.invalid_functions(), vec![0x1000]);
        g.write8(0x1003, 0x75); // original bytes again
        assert_eq!(h.admit(p), Admit::Fast);
        assert_eq!(h.counts(), (2, 1, 1));
        assert_eq!(g.inner.mem[0x1004], 0x12);
    }

    type NativeFnPtr = fn(&mut Cpu, &mut CodeGuard<FlatMem>);

    #[test]
    fn analysis_finds_writers() {
        // $1000: addq.l #1,$1006.w ; rts ; $1006: nop ; rts (patched by the first function)
        let code: [u16; 6] = [0x52B8, 0x1008, 0x4E75, 0x4E71, 0x4E71, 0x4E75];
        let mut words = std::collections::HashMap::new();
        for (i, w) in code.iter().enumerate() {
            words.insert(0x1000 + 2 * i as u32, *w);
        }
        let fs = FuncInfo::from_lookup(&[0x1000, 0x1006], 0x1000, 0x100c, &|pc| if pc == 0x1000 || pc == 0x1006 { Some(pc as usize) } else { None }, &mut |a| *words.get(&a).unwrap_or(&0));
        assert_eq!(fs[0].ranges, vec![(0x1000, 0x1006)]);
        assert_eq!(fs[1].ranges, vec![(0x1006, 0x100c)]);
        assert!(fs[0].writes_code && !fs[0].patches_self);
        assert!(!fs[1].writes_code);
    }
}
