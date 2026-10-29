//! The structural locator of the Steve Barrett replayer: it derives the build layout (the
//! offsets of the player's variables and tables, and the build flags) from the module's own
//! code, by following the specification's rules L1 to L12. The code is decoded with `m68kdec`
//! and never run.
//!
//! Terms. A *data location* is an operand that names a fixed place in the module: a
//! PC-relative operand, or an operand relative to the pointer-base register (the register the
//! `lea` before the first sample's `lea` writes). An *address load* is a `lea` of a data
//! location. A *routine* is the set of instructions reached from an address by following the
//! flow (a subroutine call continues at the next instruction only), taken in ascending address
//! order; "first", "next", "after" and "span" (the instructions after an address load up to the
//! next address load) refer to that order.
//!
//! Any rule that finds no instruction, or another number than it states, makes the whole
//! locator return `None` (the file is refused, so that another engine can take it).

use super::be16;
use crate::replay::m68kdec::{decode, Ea, Insn, K};
use std::collections::BTreeMap;

/// How the build differs from the others (specification L4.11, L5.5, L6.2, L7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Build {
    /// The early family: no slide, arpeggio or portamento.
    pub early: bool,
    /// The sample init gives the samples loops (and runs the stop routine).
    pub loops: bool,
    /// Song numbers 1..=limit start a song.
    pub limit: u8,
    /// The repeat stage covers all four voices (else voice 0 only).
    pub all_voices: bool,
    pub portamento: bool,
    /// The two instrument words are swapped (repeat length first).
    pub swap: bool,
    /// The host waits before the repeat stage.
    pub wait: bool,
}

/// Offsets of the player's variables in a build (0 where the build has none).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(super) struct Lay {
    pub flag: usize,
    pub pertab: usize,
    pub perout: usize,
    pub vrec: usize,
    pub cnt: usize,
    pub started: usize,
    pub speed: usize,
    pub spdcnt: usize,
    pub vibdepth: usize,
    pub vibshift: usize,
    pub arpparam: usize,
    pub rowbytes: usize,
    pub regoff: usize,
    pub p_steps: usize,
    pub p_lo: usize,
    pub p_lo2: usize,
    pub p_dir: usize,
    pub p_tgt: usize,
    pub dmamask: usize,
    pub arpint: usize,
    pub baseper: usize,
    pub p_delta: usize,
    pub smpstart: usize,
    pub replen: usize,
    pub repoff: usize,
    pub patptr: usize,
    pub seqptr: usize,
    pub curseq: usize,
    pub curpat: usize,
    pub vibdelta: usize,
    pub vibacc: usize,
    pub arpbuf: usize,
    pub smpaddr: usize,
    pub instr: usize,
    pub env: usize,
    pub songs: usize,
    pub pattab: usize,
    pub samples: usize,
}

/// What the locator found.
pub(super) struct Located {
    pub lay: Lay,
    pub build: Build,
    pub subs: usize,
}

/// Custom-chip addresses the rules name (base plus decimal offset).
const AUD0PER: u32 = 0xDFF000 + 166;
const AUD0VOL: u32 = 0xDFF000 + 168;
const DMACON: u32 = 0xDFF000 + 150;
/// The text `FORM`.
const FORM: u32 = u32::from_be_bytes(*b"FORM");

macro_rules! need {
    ($c:expr) => {
        if !($c) {
            return None;
        }
    };
}

// ------------------------------------------------------------------ routines

/// The routine at `start` (L0.5): its instructions in ascending address order.
fn flow(d: &[u8], start: usize) -> Option<Vec<Insn>> {
    let mut seen: BTreeMap<usize, Insn> = BTreeMap::new();
    let mut work = vec![start];
    while let Some(a) = work.pop() {
        if seen.contains_key(&a) {
            continue;
        }
        need!(a % 2 == 0 && seen.len() < 8000);
        let i = decode(d, a)?;
        match i.k {
            K::Rts => {}
            K::Bra => work.push(i.target),
            K::Bcc | K::Dbcc => {
                work.push(i.next());
                work.push(i.target);
            }
            K::Bsr => {
                need!(i.target % 2 == 0 && i.target < d.len());
                work.push(i.next());
            }
            K::Jmp | K::Jsr | K::Rte => return None,
            _ => work.push(i.next()),
        }
        seen.insert(a, i);
    }
    let v: Vec<Insn> = seen.into_values().collect();
    need!(v.windows(2).all(|w| w[0].next() <= w[1].at));
    Some(v)
}

/// The kind of source of a store.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Src {
    /// A data register.
    Reg,
    Imm(u32),
    /// Another data location.
    Loc(usize),
    /// Address-register indirect, no displacement.
    Ind,
    Other,
}

/// The pointer base P and the pointer-base register.
struct Cx<'a> {
    d: &'a [u8],
    p: i64,
    pr: u8,
}

impl Cx<'_> {
    /// The place a data-location operand names (inside the file).
    fn loc(&self, e: Ea) -> Option<usize> {
        let p = match e {
            Ea::Pc(t) => t as isize as i64,
            Ea::Disp(r, dd) if r == self.pr => self.p + dd as i64,
            _ => return None,
        };
        (p >= 0 && (p as usize) < self.d.len()).then_some(p as usize)
    }
    /// An address load: (target, register).
    fn lea(&self, i: &Insn) -> Option<(usize, u8)> {
        if i.k != K::Lea {
            return None;
        }
        let Ea::A(r) = i.dst else { return None };
        Some((self.loc(i.src)?, r))
    }
    /// A load of P into the pointer-base register.
    fn p_load(&self, i: &Insn) -> bool {
        i.k == K::Lea && i.dst == Ea::A(self.pr) && matches!(i.src, Ea::Pc(t) if t as isize as i64 == self.p)
    }
    /// A move of `sz` bytes to a data location: (place, source kind).
    fn store(&self, i: &Insn, sz: u8) -> Option<(usize, Src)> {
        if i.k != K::Move || i.size != sz {
            return None;
        }
        let p = self.loc(i.dst)?;
        let s = match i.src {
            Ea::D(_) => Src::Reg,
            Ea::Imm { val, .. } => Src::Imm(val),
            Ea::Ind(_) => Src::Ind,
            e => self.loc(e).map_or(Src::Other, Src::Loc),
        };
        Some((p, s))
    }
    /// A move of `sz` bytes from a data location into a data register: the place.
    fn load(&self, i: &Insn, sz: u8) -> Option<usize> {
        if i.k == K::Move && i.size == sz && matches!(i.dst, Ea::D(_)) {
            self.loc(i.src)
        } else {
            None
        }
    }
    /// A clear of `sz` bytes of a data location.
    fn clear(&self, i: &Insn, sz: u8) -> Option<usize> {
        if i.k == K::Clr && i.size == sz { self.loc(i.dst) } else { None }
    }
    /// An add (`add == true`) or subtract quick 1 of `sz` bytes of a data location.
    fn quick1(&self, i: &Insn, add: bool, sz: u8) -> Option<usize> {
        if i.k == if add { K::Addq } else { K::Subq } && i.size == sz && i.data == 1 {
            self.loc(i.dst)
        } else {
            None
        }
    }
    /// The places of the address loads, with their indices: (index, target, register).
    fn loads(&self, r: &[Insn]) -> Vec<(usize, usize, u8)> {
        r.iter().enumerate().filter_map(|(n, i)| self.lea(i).map(|(t, g)| (n, t, g))).collect()
    }
    /// The span of the address load number `k` of `loads`: the instructions after it, up to
    /// (not including) the next address load, or `end`.
    fn span<'r>(&self, r: &'r [Insn], loads: &[(usize, usize, u8)], k: usize, end: usize) -> &'r [Insn] {
        let from = loads[k].0 + 1;
        let to = loads.get(k + 1).map_or(end, |l| l.0).min(end);
        &r[from.min(to)..to]
    }
    fn imm_op(&self, i: &Insn, code: u8, sz: u8, val: u32) -> bool {
        i.k == K::Imm(code) && i.size == sz && matches!(i.src, Ea::Imm { val: v, .. } if v == val)
    }
    /// A byte compare with the immediate `val` (compare-immediate or compare with an immediate).
    fn cmp_imm(&self, i: &Insn, sz: u8, val: u32) -> bool {
        (i.k == K::Imm(6) || i.k == K::Cmp) && i.size == sz && matches!(i.src, Ea::Imm { val: v, .. } if v == val)
    }
}

fn only<T>(mut it: impl Iterator<Item = T>) -> Option<T> {
    let a = it.next()?;
    it.next().is_none().then_some(a)
}

/// The targets of the subroutine calls of a routine, in order.
fn calls(r: &[Insn]) -> Vec<usize> {
    r.iter().filter(|i| i.k == K::Bsr).map(|i| i.target).collect()
}

fn has_loc_operand(cx: &Cx, i: &Insn) -> bool {
    cx.loc(i.src).is_some() || cx.loc(i.dst).is_some()
}

// ------------------------------------------------------------------ the locator

/// Everything the locator needs from the detection: the four entries, the first sample, the
/// pointer base and its register.
pub(super) struct Det {
    pub entries: [usize; 4],
    pub first: usize,
    pub pbase: i64,
    pub preg: u8,
}

/// The layout and flags of the module, or `None` (refused).
pub(super) fn locate(d: &[u8], det: &Det) -> Option<Located> {
    let cx = Cx { d, p: det.pbase, pr: det.preg };
    let [e0, e1, e2, e3] = det.entries;

    // L1: the song-init and play routines start with a load of P
    for e in [e1, e2] {
        need!(cx.p_load(&decode(d, e)?));
    }
    let si = flow(d, e0)?;
    let sg = flow(d, e1)?;
    let pl = flow(d, e2)?;
    let st = flow(d, e3)?;

    // L2: the called routines
    let mut targets: Vec<usize> = Vec::new();
    for t in calls(&pl) {
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    let called: Vec<(usize, Vec<Insn>)> = targets.iter().map(|&t| Some((t, flow(d, t)?))).collect::<Option<_>>()?;

    // X0: nothing but loads of P writes the pointer-base register
    let writes_pr = |i: &Insn| i.dst == Ea::A(cx.pr) && i.k != K::Cmpa && !cx.p_load(i);
    need!(![&si, &sg, &pl, &st].into_iter().chain(called.iter().map(|c| &c.1)).any(|r| r.iter().any(writes_pr)));

    let pick = |pred: &dyn Fn(&[Insn]) -> bool| -> Vec<usize> {
        called.iter().filter(|c| pred(&c.1)).map(|c| c.0).collect()
    };
    let one = |v: Vec<usize>| -> Option<usize> {
        need!(v.len() == 1);
        Some(v[0])
    };
    let t_seq = one(pick(&|r| r.iter().any(|i| cx.imm_op(i, 3, 1, 64))))?;
    let t_instr = one(pick(&|r| r.iter().any(|i| i.k == K::Mulu && matches!(i.src, Ea::Imm { val: 6, .. }))))?;
    let t_out = one(pick(&|r| {
        r.iter().any(|i| {
            (i.k == K::Movea && i.size == 4 && matches!(i.src, Ea::Imm { val, .. } if val == AUD0PER) && matches!(i.dst, Ea::A(_)))
                || (i.k == K::Lea && matches!(i.src, Ea::AbsL { addr, .. } if addr == AUD0PER) && matches!(i.dst, Ea::A(_)))
        })
    }))?;
    let t_port = {
        let v = pick(&|r| r.iter().any(|i| i.k == K::Shift && i.left && i.shk == 3 && i.size == 2));
        need!(v.len() <= 1);
        v.first().copied()
    };
    let t_stopclr = one(pick(&|r| r.first().is_some_and(|i| i.k == K::Bsr && i.target == e3)))?;
    let t_setper = one(pick(&|r| {
        r.iter().all(|i| i.k != K::Bsr)
            && r.iter().filter(|i| cx.lea(i).is_some()).count() == 3
            && r.iter().filter(|i| has_loc_operand(&cx, i)).count() == 3
    }))?;
    let rt = |t: usize| &called.iter().find(|c| c.0 == t).unwrap().1;
    // envelope: the call after the (single) call to the instrument routine
    let pl_calls = calls(&pl);
    need!(pl_calls.iter().filter(|&&t| t == t_instr).count() == 1);
    let k = pl_calls.iter().position(|&t| t == t_instr)?;
    let t_env = *pl_calls.get(k + 1)?;
    let env_r = &called.iter().find(|c| c.0 == t_env)?.1;
    let t_arp = only(calls(env_r).into_iter())?;
    let arp_r = flow(d, t_arp)?;
    let (seq_r, instr_r, out_r, setper_r) = (rt(t_seq), rt(t_instr), rt(t_out), rt(t_setper));
    let stopclr_r = rt(t_stopclr);

    // L3
    let flag = only(sg.iter().filter_map(|i| cx.store(i, 1).filter(|s| s.1 == Src::Reg)).map(|s| s.0))?;

    // L4: the play routine
    let q = pl.iter().position(|i| i.k == K::Moveq)?;
    need!(pl[q].data >= 0);
    let limit = pl[q].data as u8;
    {
        let c = pl.get(q + 1)?;
        need!((c.k == K::Cmp || c.k == K::Imm(6)) && c.size == 1);
        let b = pl.get(q + 2)?;
        need!(b.k == K::Bcc && b.cond == 5);
    }
    let pl_loads = cx.loads(&pl);
    let after_q: Vec<&(usize, usize, u8)> = pl_loads.iter().filter(|l| l.0 > q).collect();
    need!(after_q.len() >= 3);
    let (songs, seqptr, vrec) = (after_q[0].1, after_q[1].1, after_q[2].1);
    let bstores = |r: &[Insn]| -> Vec<(usize, usize, Src)> {
        r.iter().enumerate().filter_map(|(n, i)| cx.store(i, 1).map(|(p, s)| (n, p, s))).collect()
    };
    let wstores = |r: &[Insn]| -> Vec<(usize, usize, Src)> {
        r.iter().enumerate().filter_map(|(n, i)| cx.store(i, 2).map(|(p, s)| (n, p, s))).collect()
    };
    let pl_b = bstores(&pl);
    let started = pl_b.iter().find(|s| s.2 == Src::Reg)?.1;
    let dmamask = only(pl_b.iter().filter(|s| s.2 == Src::Imm(15)))?.1;
    let cnt_s = only(pl_b.iter().filter(|s| s.2 == Src::Imm(3)))?;
    let cnt = cnt_s.1;
    need!(matches!(pl[cnt_s.0].dst, Ea::Disp(r, _) if r == cx.pr));
    let regoff = only(pl_b.iter().filter(|s| s.2 == Src::Ind))?.1;
    let rowbytes = pl.iter().find_map(|i| cx.clear(i, 1))?;
    let (vs_n, vibshift) = only(pl.iter().enumerate().filter_map(|(n, i)| {
        if cx.imm_op(i, 1, 1, 15) { cx.loc(i.dst).map(|p| (n, p)) } else { None }
    }))?;
    let vibdepth = pl_b.iter().find(|s| s.0 > vs_n && s.2 == Src::Reg)?.1;
    let pl_w = wstores(&pl);
    let vibdelta = pl_w.first()?.1;
    let vibacc = only(pl_w.iter().filter(|s| s.2 == Src::Ind))?.1;
    let (speed, spdcnt) = only(pl_b.iter().filter_map(|s| if let Src::Loc(q) = s.2 { Some((q, s.1)) } else { None }))?;
    let full = pl.iter().any(|i| cx.cmp_imm(i, 1, 125));

    // L5: the called routines
    let seq_clr: Vec<usize> = stopclr_r.iter().filter_map(|i| cx.clear(i, 1)).collect();
    need!(seq_clr == [spdcnt, started]);
    let sp_loads: Vec<usize> = cx.loads(setper_r).iter().map(|l| l.1).collect();
    let (pertab, baseper, perout) = (sp_loads[0], sp_loads[1], sp_loads[2]);
    let sq_loads = cx.loads(seq_r);
    need!(sq_loads.len() == 3 && sq_loads[0].1 == seqptr);
    let (pattab, patptr) = (sq_loads[1].1, sq_loads[2].1);
    let sq_l: Vec<usize> = seq_r.iter().filter_map(|i| cx.store(i, 4).map(|s| s.0)).collect();
    need!(sq_l.len() >= 2);
    let (curseq, curpat) = (sq_l[0], *sq_l.last()?);
    let en_loads = cx.loads(env_r);
    need!(en_loads.len() == 1);
    let env = en_loads[0].1;
    let arpparam = only(env_r.iter().filter_map(|i| cx.store(i, 1).map(|s| s.0)))?;
    let ar_loads = cx.loads(&arp_r);
    let (arpint, arpbuf);
    if full {
        let mut ai = None;
        let mut ab = None;
        for k in 0..ar_loads.len() {
            let sp = cx.span(&arp_r, &ar_loads, k, arp_r.len());
            let reg = ar_loads[k].2;
            if ai.is_none() && sp.iter().any(|i| i.k == K::Move && i.size == 1 && i.src == Ea::Ind(reg)) {
                ai = Some(ar_loads[k].1);
            }
            if ab.is_none() && sp.iter().any(|i| i.k == K::Move && i.size == 1 && i.dst == Ea::Ind(reg)) {
                ab = Some(ar_loads[k].1);
            }
        }
        arpint = ai?;
        arpbuf = ab?;
    } else {
        need!(ar_loads.is_empty());
        arpint = 0;
        arpbuf = 0;
    }
    // the instrument routine
    let in_loads = cx.loads(instr_r);
    need!(in_loads.len() == 5);
    let sp = |k: usize| cx.span(instr_r, &in_loads, k, instr_r.len());
    let instr_k = only((0..5).filter(|&k| {
        let reg = in_loads[k].2;
        [1i16, 6, 7].iter().all(|&dd| sp(k).iter().any(|i| i.k == K::Move && i.size == 1 && i.src == Ea::Disp(reg, dd)))
    }))?;
    let instr = in_loads[instr_k].1;
    let ireg = in_loads[instr_k].2;
    let mut copies: Vec<(i16, usize)> = Vec::new();
    for k in 0..5 {
        let reg = in_loads[k].2;
        for i in sp(k) {
            if i.k == K::Move && i.size == 2 && i.dst == Ea::Ind(reg) {
                if let Ea::Disp(r, dd) = i.src {
                    if r == ireg && (dd == 2 || dd == 4) {
                        copies.push((dd, in_loads[k].1));
                    }
                }
            }
        }
    }
    need!(copies.len() == 2 && copies.iter().any(|c| c.0 == 2) && copies.iter().any(|c| c.0 == 4));
    let mulu = instr_r.iter().position(|i| i.k == K::Mulu && matches!(i.src, Ea::Imm { val: 6, .. }))?;
    let Ea::D(mreg) = instr_r[mulu].dst else { return None };
    let smpaddr = in_loads.iter().find(|l| {
        l.0 > mulu && instr_r.get(l.0 + 1).is_some_and(|n| n.k == K::Adda && n.src == Ea::D(mreg) && n.dst == Ea::A(l.2))
    })?.1;
    let smpstart = only((0..5).filter(|&k| {
        let reg = in_loads[k].2;
        sp(k).iter().any(|i| i.k == K::Move && i.size == 4 && i.dst == Ea::Ind(reg))
    }))?;
    let smpstart = in_loads[smpstart].1;

    // L6: the sample init
    let si_loads = cx.loads(&si);
    let load_of = |reg: u8| only(si_loads.iter().filter(|l| l.2 == reg)).map(|l| l.1);
    let cmp_form = si.iter().find(|i| i.k == K::Imm(6) && i.size == 4 && matches!(i.src, Ea::Imm { val, .. } if val == FORM))?;
    let sreg = match cmp_form.dst {
        Ea::Post(r) | Ea::Ind(r) => r,
        _ => return None,
    };
    let samples = load_of(sreg)?;
    need!(samples == det.first);
    let loops = si.iter().any(|i| i.k == K::Clr && i.size == 2 && i.dst == Ea::Ind(sreg));
    let wp = only(si.iter().filter(|i| i.k == K::Move && i.size == 2 && matches!(i.dst, Ea::Post(_))))?;
    let Ea::Post(wreg) = wp.dst else { return None };
    need!(load_of(wreg)? == smpaddr);
    let q8 = only(si.iter().filter(|i| i.k == K::Addq && i.size == 4 && i.data == 8 && matches!(i.dst, Ea::A(_))))?;
    let Ea::A(qreg) = q8.dst else { return None };
    need!(load_of(qreg)? == instr + if loops { 2 } else { 0 });

    // L7: the repeat stage
    let o = only(pl.iter().enumerate().filter(|(_, i)| i.k == K::Bsr && i.target == t_out))?.0;
    let sub = only(pl.iter().enumerate().filter(|(_, i)| cx.quick1(i, false, 1) == Some(spdcnt)))?.0;
    need!(sub > o);
    let stage = &pl[o + 1..sub];
    let st_loads = cx.loads(stage);
    let (mut replen, mut repoff, mut st_smp) = (None, None, None);
    let (mut halving, mut zero_test) = (false, false);
    for k in 0..st_loads.len() {
        let reg = st_loads[k].2;
        let spn = cx.span(stage, &st_loads, k, stage.len());
        if let Some(m) = spn.iter().position(|i| i.k == K::Move && i.size == 2 && i.src == Ea::Ind(reg) && matches!(i.dst, Ea::D(_))) {
            let Ea::D(rd) = spn[m].dst else { return None };
            let anded = spn[m + 1..].iter().any(|i| cx.imm_op(i, 1, 4, 4095) && i.dst == Ea::D(rd));
            if anded {
                need!(repoff.is_none());
                repoff = Some(st_loads[k].1);
            } else {
                need!(replen.is_none());
                replen = Some(st_loads[k].1);
                halving = spn[m + 1..].iter().any(|i| i.k == K::Shift && !i.left && i.shk == 1 && i.size == 2 && i.data == 1 && i.dst == Ea::D(rd));
                zero_test = spn.get(m + 1).is_some_and(|i| i.k == K::Bcc && i.cond == 7);
            }
        }
        if spn.iter().any(|i| i.k == K::Add && i.size == 4 && i.src == Ea::Ind(reg)) {
            need!(st_smp.is_none());
            st_smp = Some(st_loads[k].1);
        }
    }
    let (replen, repoff) = (replen?, repoff?);
    need!(st_smp? == smpstart);
    let has_loop = stage.iter().any(|i| i.k == K::Dbcc);
    let all_voices = if has_loop && halving && !zero_test {
        true
    } else if !has_loop && !halving && zero_test {
        let lb: Vec<usize> = stage.iter().filter_map(|i| cx.load(i, 1)).collect();
        need!(lb == [regoff]);
        false
    } else {
        return None;
    };
    let c2 = copies.iter().find(|c| c.0 == 2)?.1;
    let c4 = copies.iter().find(|c| c.0 == 4)?.1;
    need!((c2 == replen && c4 == repoff) || (c2 == repoff && c4 == replen));
    let swap = c2 == replen;
    let wait = {
        let qs: Vec<usize> = stage.iter().enumerate().filter(|(_, i)| i.k == K::Moveq).map(|(n, _)| n).take(3).collect();
        qs.len() == 3 && qs[1] == qs[0] + 1 && qs[2] == qs[0] + 2 && [3, 12, 6].iter().zip(&qs).all(|(&v, &n)| stage[n].data == v)
    };
    // portamento
    let mut lay_p = (0, 0, 0, 0, 0, 0, 0);
    let portamento = t_port.is_some();
    if let Some(tp) = t_port {
        need!(full);
        let pr_r = rt(tp);
        let p_tgt = pr_r.iter().find_map(|i| cx.load(i, 1))?;
        let p_dir = only(pr_r.iter().filter_map(|i| cx.store(i, 1).filter(|s| s.1 == Src::Imm(1)).map(|s| s.0)))?;
        let p_delta = pr_r.iter().find_map(|i| cx.store(i, 2).map(|s| s.0))?;
        let p_lo = only(pr_r.iter().filter_map(|i| {
            if i.k == K::Add && i.size == 1 && matches!(i.dst, Ea::D(_)) { cx.loc(i.src) } else { None }
        }))?;
        let p_steps = only(pr_r.iter().filter_map(|i| cx.store(i, 1).filter(|s| s.1 == Src::Reg).map(|s| s.0)))?;
        let lo_n = only(pl_b.iter().filter(|s| s.1 == p_lo && s.2 == Src::Reg))?.0;
        let prev = pl_b.iter().find(|s| s.0 + 1 == lo_n && s.2 == Src::Reg)?;
        let p_lo2 = prev.1;
        lay_p = (p_tgt, p_dir, p_delta, p_lo, p_steps, p_lo2, 1);
    }
    let (p_tgt, p_dir, p_delta, p_lo, p_steps, p_lo2, _) = lay_p;

    // L8
    need!(pattab > songs && (pattab - songs) % 8 == 0);
    let subs = (pattab - songs) / 8;

    let lay = Lay {
        flag, pertab, perout, vrec, cnt, started, speed, spdcnt, vibdepth, vibshift, arpparam, rowbytes, regoff, p_steps, p_lo,
        p_lo2, p_dir, p_tgt, dmamask, arpint, baseper, p_delta, smpstart, replen, repoff, patptr, seqptr, curseq, curpat,
        vibdelta, vibacc, arpbuf, smpaddr, instr, env, songs, pattab, samples,
    };
    let build = Build { early: !full, loops, limit, all_voices, portamento, swap, wait };

    // L9: cross-checks
    // X2: the stop routine
    {
        let sl = cx.loads(&st);
        need!(sl.len() == 1 && sl[0].1 == vrec + 96);
        let r = st.iter().find_map(|i| {
            (i.k == K::Movea && i.size == 4 && matches!(i.src, Ea::Imm { val, .. } if val == AUD0VOL)).then_some(i.dst)
        })?;
        let Ea::A(r) = r else { return None };
        let disps: Vec<i16> = st
            .iter()
            .filter(|i| i.k == K::Move && i.size == 2)
            .filter_map(|i| match i.dst {
                Ea::Ind(g) if g == r => Some(0),
                Ea::Disp(g, dd) if g == r => Some(dd),
                _ => None,
            })
            .collect();
        need!(disps == [0, 16, 32, 48]);
    }
    // X3: the output routine
    {
        let ol: Vec<usize> = cx.loads(out_r).iter().map(|l| l.1).collect();
        need!(ol == [perout, vrec + 96]);
        need!(only(out_r.iter().filter_map(|i| cx.load(i, 1)))? == dmamask);
        need!(out_r.iter().any(|i| cx.imm_op(i, 5, 2, 65535)));
    }
    // vr: the voice-record register
    let vreg = {
        let regs: Vec<u8> = pl_loads.iter().filter(|l| l.1 == vrec).map(|l| l.2).collect();
        need!(!regs.is_empty() && regs.iter().all(|&r| r == regs[0]));
        regs[0]
    };
    // X4: the sequence advance
    need!(seq_r.iter().any(|i| i.k == K::Move && i.size == 1 && i.src == Ea::Disp(vreg, 12) && matches!(i.dst, Ea::D(_))));
    need!(seq_r.iter().any(|i| i.k == K::Addq && i.size == 1 && i.data == 1 && i.dst == Ea::Disp(vreg, 12)));
    // X5
    need!(pl.iter().find_map(|i| cx.load(i, 1))? == flag);
    need!(pl.iter().any(|i| cx.clear(i, 1) == Some(flag)));
    need!(pl.iter().any(|i| i.k == K::Scc && i.cond == 0 && i.size == 1 && cx.loc(i.dst) == Some(flag)));
    need!(pl_b.iter().any(|s| s.1 == started && s.2 == Src::Imm(1)));
    need!(pl.iter().any(|i| cx.quick1(i, false, 1) == Some(cnt)));
    need!(pl.iter().any(|i| cx.quick1(i, true, 1) == Some(rowbytes)));
    // X6
    {
        let bits: Vec<&Insn> = pl.iter().filter(|i| matches!(i.k, K::Bit(2) | K::Bit(3)) && matches!(i.src, Ea::D(_))).collect();
        need!(!bits.is_empty() && bits.iter().all(|i| cx.loc(i.dst) == Some(dmamask)));
    }
    // X7
    {
        let pt: Vec<usize> = pl_loads.iter().map(|l| l.1).collect();
        for t in [patptr, pertab, baseper, perout, env, replen, repoff, smpstart] {
            need!(pt.contains(&t));
        }
        if full {
            need!(pt.contains(&arpbuf));
            need!(pl_b.iter().any(|s| s.1 == arpparam && s.2 == Src::Reg));
            need!(arp_r.iter().any(|i| cx.load(i, 1) == Some(arpparam)));
        }
    }
    // X8
    need!(pl.iter().any(|i| i.k == K::Add && i.size == 2 && cx.loc(i.src) == Some(vibdelta)));
    need!(pl.iter().any(|i| i.k == K::Sub && i.size == 2 && cx.loc(i.src) == Some(vibdelta)));
    need!(pl.iter().any(|i| cx.load(i, 1) == Some(vibdepth)));
    // X9
    {
        let sl: Vec<&(usize, usize, u8)> = pl_loads.iter().filter(|l| l.1 == songs).collect();
        need!(sl.len() == 1);
        let reg = sl[0].2;
        let n = pl.iter().filter(|i| i.k == K::Adda && i.size == 2 && i.src == Ea::Post(reg) && matches!(i.dst, Ea::A(_))).count();
        need!(n == 4);
    }
    // X10
    {
        let rs: [&[Insn]; 5] = [&pl, seq_r, instr_r, env_r, &arp_r];
        let disp_ok = |e: Ea| match e {
            Ea::Disp(r, dd) if r == vreg => dd % 4 == 0 && (0..=124).contains(&dd),
            _ => true,
        };
        need!(rs.iter().all(|r| r.iter().all(|i| disp_ok(i.src) && disp_ok(i.dst))));
        let dm = only(pl.iter().enumerate().filter(|(_, i)| {
            i.k == K::Move && i.size == 2 && matches!(i.dst, Ea::AbsL { addr, .. } if addr == DMACON)
        }))?
        .0;
        let prev = pl.get(dm.checked_sub(1)?)?;
        need!(prev.k == K::Move && prev.size == 1 && prev.src == Ea::Disp(vreg, 100));
    }
    // X11
    if portamento {
        need!(pl_b.iter().any(|s| s.1 == p_tgt));
        need!(rt(t_port?).iter().any(|i| cx.clear(i, 1) == Some(p_dir)));
        need!(pl.iter().any(|i| i.k == K::Add && i.size == 1 && matches!(i.dst, Ea::D(_)) && cx.loc(i.src) == Some(p_lo2)));
        need!(pl_loads.iter().any(|l| l.1 == p_delta));
        need!(rt(t_port?).iter().any(|i| cx.load(i, 1) == Some(speed)));
    }
    // X12
    {
        let and_end = pl.iter().filter(|i| cx.imm_op(i, 1, 4, 4095)).map(|i| i.next()).max()?;
        need!(and_end <= 2300 && out_r.last()?.next() <= 2300);
    }

    // L10: data checks
    let at = |o: usize| d.get(o).copied();
    for p in [flag, vrec, songs, pattab, samples] {
        need!(p < d.len());
    }
    for v in 0..4usize {
        need!(at(vrec + v) == Some(16 * v as u8) && at(vrec + 100 + v) == Some(1 << v));
        need!(!full || at(vrec + 4 + v) == Some(10 * v as u8));
    }
    if full {
        need!((0..6).all(|i| at(arpint + i) == Some([3, 5, 6, 7, 8, 9][i])));
    }
    for (o, v) in [(flag, 1), (started, 0), (speed, 1), (spdcnt, 0), (dmamask, 15), (cnt, 0)] {
        need!(at(o) == Some(v));
    }
    need!(pertab + 72 + 2 <= d.len());
    need!((1..36).all(|i| be16(d, pertab + 2 * i) > be16(d, pertab + 2 * i + 2)));
    need!(pattab + 2 <= d.len() && env + 8 <= d.len() && instr + 8 <= d.len());

    Some(Located { lay, build, subs })
}
