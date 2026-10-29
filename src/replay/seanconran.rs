//! Sean Conran (`scr.*`), the music driver Sean Conran wrote for UK games of 1990-92
//! (Hong Kong Phooey, Ruff and Reddy, Top Cat, Yogi Bear and Friends, Wacky Races,
//! Defenders of the Earth, Infestation, Mega Twins, WWF Wrestlemania, Count Duckula
//! 2, Videokid).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Sean Conran player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A `scr.*` file is a raw memory image of the game's replay routine, its music and
//! its samples. The code is not run. It keeps its state in its own instructions'
//! immediates (counters, flags, envelope pointers, the current note, the master
//! volume), so the port keeps a mutable copy of the image and finds those cells, and
//! the few tables, by reading the code structurally: generic instruction shapes (the
//! play prologue, the four voice blocks with their command tables, whose handlers are
//! classified by shape, the effects and final blocks, the stop stub and the init
//! routine) act as signatures. The engine is written from the behaviour: four voices,
//! 50 Hz, a pattern parse when a voice's counter runs out, a cut before a command,
//! per-voice volume and pitch envelopes, the period table and the volume table read
//! from the image, position lists that loop or stop, and (Mega Twins) a one-shot
//! attack and loop by the audio interrupt.
//!
//! Decisions where the specification leaves the choice to the port:
//! * The image is kept at the reference host's address 0xF000 (its pointer cells hold
//!   absolute addresses) and converted to the guest address only for Paula's location
//!   registers; reads outside the image return 0.
//! * The audio interrupt's writes (Mega Twins) are made right after the DMA-on that
//!   raises it.
//! * A `hang` end of a position list (the original spins; never reached) stops the song,
//!   and an invalid command (a BOGUS slot, a byte that is not a table entry) ends the
//!   voice's parse for the tick.
//! * A subsong starts from the file's image again (the original keeps the image it
//!   left).

use super::m68kdec::{decode, Ea, Insn, K};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Sean Conran", detect, new };

/// The address the image's pointer cells are relative to (the reference host's load address).
const VIRT: u32 = 0xF000;
const HWBASE: u32 = 0xDFF096;

/// The waits between writes, in colour clocks (rough: the times are not compared).
const T_WRITE: u64 = 24;
const T_VOICE: u64 = 300;
/// The audio interrupt's latency after the DMA-on that raises it (the reference's
/// shortest, 160 colour clocks).
const T_IRQ: u64 = 160;

fn w(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

// ------------------------------------------------------------------ recognition

/// Rule 2.1: the period table's first entries and the sound-effect-bank exclusion.
fn header(d: &[u8]) -> bool {
    if d.len() < 0x400 {
        return false;
    }
    let c = if matches!(be32(d, 0), 0x0FFF_0FE2 | 0x1000_0FE2) && be32(d, 4) == 0x0FC4_0FA7 && be32(d, 8) == 0x0F8B_0F6E {
        292
    } else if be32(d, 0) == 0x0F1C_0F0E && be32(d, 4) == 0x0F00_0EF2 && be32(d, 8) == 0x0EE4_0ED6 {
        452
    } else {
        return false;
    };
    (0..128).all(|k| {
        let o = c + 2 * k;
        o + 4 <= d.len() && be32(d, o) != 0x7F7F_7F7F && w(d, o) != 0xFFFF
    })
}

fn detect(d: &[u8]) -> bool {
    header(d) && layout(d).is_some()
}

// ------------------------------------------------------------------ the layout

/// Where a store goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tgt {
    Img(usize),
    Hw(u32),
}

/// A constant store or register write of the code.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Act {
    t: Tgt,
    size: u8,
    val: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum NegKind {
    Loop,
    StopFf,
    StopNeg,
    StopNow,
    Hang,
    None,
}

#[derive(Clone, Debug, PartialEq)]
struct NextPat {
    pos: usize,
    tab: usize,
    double: bool,
    neg: NegKind,
    /// The position routine's first instruction, its NEG target and its STOP code.
    start: usize,
    negat: Option<usize>,
    stop: Option<usize>,
    direct: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Cmd {
    SetB(usize),
    SetPtr { cell: usize, scratch: usize },
    Call { ret: usize, scratch: usize },
    Ret { flag: usize, ret: usize },
    Sample { scratch: usize, extra: Vec<Act> },
    SampleRec { scratch: usize },
    MasterRel { mv: usize },
    /// The note and duration commands with their constant stores and register
    /// writes before them: NOTE / TRIG (`note`), DUR / RETRIG, REST (`rest`).
    /// `look` is the lookahead's code (after the PAT store).
    Event { pre: Vec<Act>, note: Option<usize>, cnt: usize, pat: usize, rest: bool, look: usize },
    NextPat,
    Bogus,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Eff {
    eff: usize,
    vrs: usize,
    venvs: usize,
    venvp: usize,
    volb: usize,
    mask: Option<u8>,
    volidx: usize,
    prs: usize,
    penvs: usize,
    penvp: usize,
    /// The note word's cell (its high byte; NOTE is the low byte).
    note: usize,
    double: bool,
    shift: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct Voice {
    cnt: usize,
    neg_form: bool,
    cut: usize,
    cutacts: Vec<Act>,
    pat: usize,
    cmds: Vec<Cmd>,
    np: NextPat,
    eff: Eff,
    /// The RET handler's flag (CALL sets it).
    flag: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct Fade {
    fade: usize,
    fc: usize,
    reload: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct Final {
    fade: Option<Fade>,
    mv: usize,
    mvsub: u8,
    vt: usize,
    vols: [usize; 4],
    led: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Stub {
    at: usize,
    flag: usize,
    set: u8,
    bmi: bool,
    writes: Vec<(u32, u16)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum InitEv {
    Store(usize, u8, u32),
    Hw(u32, u16),
    SubWord(usize),
    MasterCopy(usize, usize),
    Led,
}

#[derive(Clone, Debug, PartialEq)]
struct Layout {
    init: usize,
    play: usize,
    run: usize,
    /// The prologue's `bset` (Some(true)) or `bclr` of the LED.
    led: Option<bool>,
    d2: i8,
    d3: i8,
    voices: Vec<Voice>,
    fin: Final,
    stub: Stub,
    initev: Vec<InitEv>,
    subs: usize,
    songs: Option<usize>,
    posstart: Option<[usize; 4]>,
    irq: Option<[(usize, usize); 4]>,
    mega: bool,
}

/// What the code's registers hold, as far as the scan follows them.
#[derive(Clone, Copy, Default)]
struct Ctx {
    a: [Option<Tgt>; 8],
    d: [Option<u32>; 8],
    /// The register that holds RUN (0xFF whenever the code runs).
    run: Option<u8>,
}

impl Ctx {
    fn addr(&self, e: Ea) -> Option<Tgt> {
        let off = |t: Tgt, d: i32| match t {
            Tgt::Img(o) => Tgt::Img((o as i64 + d as i64) as usize),
            Tgt::Hw(h) => Tgt::Hw(h.wrapping_add(d as u32)),
        };
        match e {
            Ea::Ind(r) => self.a[r as usize],
            Ea::Disp(r, d) => self.a[r as usize].map(|t| off(t, d as i32)),
            Ea::AbsL { addr, .. } if addr >= 0xBF_0000 => Some(Tgt::Hw(addr)),
            _ => None,
        }
    }

    fn reg(&self, r: u8, size: u8) -> Option<u32> {
        if Some(r) == self.run {
            return Some(if size == 1 { 0xFF } else if size == 2 { 0xFFFF } else { 0xFFFF_FFFF });
        }
        self.d[r as usize]
    }

    /// A constant store: `move #imm/dK`, `clr`, `st`/`sf`.
    fn store(&self, i: &Insn) -> Option<Act> {
        let (size, val) = match i.k {
            K::Move => (
                i.size,
                match i.src {
                    Ea::Imm { val, .. } => val,
                    Ea::D(r) => self.reg(r, i.size)?,
                    _ => return None,
                },
            ),
            K::Clr => (i.size, 0),
            K::Scc if i.cond <= 1 => (1, if i.cond == 0 { 0xFF } else { 0 }),
            _ => return None,
        };
        let mask = match size {
            1 => 0xFF,
            2 => 0xFFFF,
            _ => 0xFFFF_FFFF,
        };
        Some(Act { t: self.addr(i.dst)?, size, val: val & mask })
    }

    /// Follows `lea`, `moveq` (and forgets what other writes change).
    fn step(&mut self, i: &Insn) {
        match (i.k, i.src, i.dst) {
            (K::Lea, Ea::Pc(t), Ea::A(r)) => self.a[r as usize] = Some(Tgt::Img(t)),
            (K::Lea, Ea::AbsL { addr, .. }, Ea::A(r)) => {
                self.a[r as usize] = (addr >= 0xBF_0000).then_some(Tgt::Hw(addr));
            }
            (K::Moveq, _, Ea::D(r)) => {
                self.d[r as usize] = Some(i.data as u32);
                if self.run == Some(r) {
                    self.run = None;
                }
            }
            (_, _, Ea::A(r)) if i.k != K::Cmpa => self.a[r as usize] = None,
            (K::Move | K::Add | K::Sub | K::And | K::Or | K::Addq | K::Subq | K::Clr | K::Shift | K::Ext | K::Swap, _, Ea::D(r)) => {
                self.d[r as usize] = None;
            }
            _ => {}
        }
    }
}

fn dec(d: &[u8], at: usize) -> Option<Insn> {
    decode(d, at)
}

fn img_disp(ctx: &Ctx, e: Ea) -> Option<usize> {
    match ctx.addr(e)? {
        Tgt::Img(o) => Some(o),
        _ => None,
    }
}

fn hw_disp(ctx: &Ctx, e: Ea) -> Option<u32> {
    match ctx.addr(e)? {
        Tgt::Hw(h) => Some(h),
        _ => None,
    }
}

const NE: u8 = 6;
const EQ: u8 = 7;
const MI: u8 = 11;
const CS: u8 = 5;

fn is_bcc(i: &Insn, c: u8) -> bool {
    i.k == K::Bcc && i.cond == c
}

/// The search of 3.1 and the subsong count.
struct Entry {
    init: usize,
    play: usize,
    subs: usize,
    songs: Option<usize>,
}

fn entry(d: &[u8]) -> Option<Entry> {
    let mut o = if w(d, 0) == 0x0F1C { 452 } else { 292 };
    let init = loop {
        if o + 4 > d.len() || be32(d, o) == 0 {
            return None;
        }
        if matches!(w(d, o), 0x45FA | 0x43F9 | 0x41F9) {
            break o;
        }
        o += 2;
    };
    let mut o = init;
    let one = loop {
        if o + 2 > d.len() {
            return None;
        }
        match w(d, o) {
            0x7000 => break true,
            0x7200 => break false,
            _ => o += 2,
        }
    };
    let (subs, songs) = if one {
        while o + 4 <= d.len() && w(d, o) != 0x41FA {
            o += 2;
        }
        (1, None)
    } else {
        let t = loop {
            if o + 4 > d.len() {
                return None;
            }
            match w(d, o) {
                0x43FA => break (o as i64 + 2 + w(d, o + 2) as i16 as i64) as usize,
                0x43EA => break w(d, o + 2) as usize,
                _ => o += 2,
            }
        };
        let first = w(d, t) as usize;
        if first <= t || (first - t) % 8 != 0 {
            return None;
        }
        ((first - t) / 8, Some(t))
    };
    while o + 2 <= d.len() && !matches!(w(d, o), 0x1080 | 0x4210 | 0x50D0) {
        o += 2;
    }
    let play = o + 4;
    (play + 2 <= d.len() && subs > 0).then_some(Entry { init, play, subs, songs })
}

/// The play prologue (4.1): RUN, the stop stub, the constants, the base registers.
fn prologue(d: &[u8], play: usize) -> Option<(Ctx, usize, usize, Option<bool>)> {
    let i = dec(d, play)?;
    let Ea::D(rr) = i.dst else { return None };
    if i.k != K::Moveq {
        return None;
    }
    let b = dec(d, i.next())?;
    if !is_bcc(&b, EQ) {
        return None;
    }
    let mut ctx = Ctx::default();
    let mut p = b.next();
    let mut led = None;
    loop {
        let i = dec(d, p)?;
        match (i.k, i.src, i.dst) {
            (K::Moveq, _, _) | (K::Lea, _, _) => ctx.step(&i),
            (K::Bit(k @ (2 | 3)), Ea::Imm { val: 1, .. }, Ea::AbsL { addr: 0xBFE001, .. }) => led = Some(k == 3),
            _ => break,
        }
        p = i.next();
    }
    ctx.run = Some(rr);
    let has_base = ctx.a.iter().any(|a| *a == Some(Tgt::Img(0)));
    let has_hw = ctx.a.iter().any(|a| *a == Some(Tgt::Hw(HWBASE)));
    (has_base && has_hw).then_some((ctx, p, b.target, led))
}

/// Instructions from `p`, following unconditional `bra`s to other targets than `stops`.
fn follow(d: &[u8], p: usize, stops: &[usize]) -> Option<Insn> {
    let mut p = p;
    for _ in 0..8 {
        let i = dec(d, p)?;
        if i.k == K::Bra && !stops.contains(&i.target) {
            p = i.target;
            continue;
        }
        return Some(i);
    }
    None
}

/// The position routine of a NEXTPAT handler (4.3, 5.4).
fn nextpat(d: &[u8], h: usize, ctx: &Ctx, stub: usize, run: usize) -> Option<NextPat> {
    let mut p = h;
    // the handler may go to a shared routine first (with a return address in a2)
    for _ in 0..4 {
        let i = dec(d, p)?;
        match (i.k, i.src, i.dst) {
            (K::Bra | K::Bsr, _, _) => p = i.target,
            (K::Jmp | K::Jsr, _, Ea::Pc(t)) => p = t,
            (K::Lea, _, Ea::A(r)) if r != 0 => p = i.next(),
            _ => break,
        }
    }
    let start = p;
    let i = dec(d, p)?;
    let (K::Lea, Ea::AbsL { pos, .. }, Ea::A(0)) = (i.k, i.src, i.dst) else { return None };
    let m = dec(d, i.next())?;
    if !(m.k == K::Move && m.size == 1 && m.src == Ea::Post(0) && m.dst == Ea::D(0)) {
        return None;
    }
    let mut p = m.next();
    let mut negat = None;
    let mut tab = None;
    let mut double = false;
    for _ in 0..12 {
        let i = dec(d, p)?;
        match (i.k, i.src, i.dst) {
            (K::Bcc, _, _) if i.cond == MI && negat.is_none() && tab.is_none() => negat = Some(i.target),
            (K::Lea, Ea::Pc(t), Ea::A(0)) => tab = Some(t),
            (K::Add, Ea::D(0), Ea::D(0)) if i.size == 2 => double = true,
            (K::Movea, Ea::Idx(0, _, _), Ea::A(0)) => break,
            _ => {}
        }
        p = i.next();
    }
    let tab = tab?;
    let mut np = NextPat { pos, tab, double, neg: NegKind::None, start, negat, stop: None, direct: false };
    let Some(n) = negat else { return Some(np) };
    let a = dec(d, n)?;
    let b = dec(d, a.next())?;
    // a store to the RUN cell
    let run_store = |i: &Insn| matches!(i.k, K::Move | K::Clr | K::Scc) && img_disp(ctx, i.dst) == Some(run);
    match (a.k, a.src, a.dst) {
        (K::Cmp, Ea::Ind(0), Ea::D(r)) if (Some(r) == ctx.run || ctx.d[r as usize].map(|v| v as u8) == Some(0xFF)) && is_bcc(&b, EQ) => {
            np.neg = NegKind::StopFf;
            np.stop = Some(b.target);
        }
        (K::Imm(6), Ea::Imm { val: 0xFF, .. }, Ea::Ind(0)) if is_bcc(&b, EQ) => {
            np.neg = NegKind::StopFf;
            np.stop = Some(b.target);
        }
        (K::Move, Ea::Ind(0), Ea::D(0)) if is_bcc(&b, MI) => {
            np.neg = NegKind::StopNeg;
            np.stop = Some(b.target);
        }
        (K::Move, Ea::Ind(0), Ea::D(0)) if b.k == K::Suba => np.neg = NegKind::Loop,
        _ if {
            // back into the routine's first instruction, by a branch or falling into it
            let mut q = n;
            let mut back = false;
            for _ in 0..4 {
                let Some(i) = dec(d, q) else { break };
                if i.k == K::Bra {
                    back = i.target == start;
                    break;
                }
                q = i.next();
                if q == start {
                    back = true;
                    break;
                }
            }
            back
        } =>
        {
            np.neg = NegKind::Hang
        }
        _ if run_store(&a) => {
            np.neg = NegKind::StopNow;
            np.stop = Some(n);
        }
        _ => return None,
    }
    if let Some(s) = np.stop {
        let mut i = dec(d, s)?;
        if i.k != K::Bra {
            i = dec(d, i.next())?;
        }
        np.direct = i.k == K::Bra && i.target >= stub + 4;
    }
    Some(np)
}

/// Classifies a command handler (4.3).
fn handler(d: &[u8], h: usize, ctx: &Ctx, nextvoice: usize, loop_at: usize, stub: usize, run: usize) -> Option<Cmd> {
    let base = ctx.a.iter().position(|a| *a == Some(Tgt::Img(0)))? as u8;
    let i0 = dec(d, h)?;
    let i1 = dec(d, i0.next())?;
    let mb = |i: &Insn, dst: Ea| i.k == K::Move && i.size == 1 && i.src == Ea::Post(0) && i.dst == dst;
    // lea S(pc),a1 ; move.b (a0)+,(a1)+ ...
    // lea S,aN (pc- or base-relative) ; move.b (a0)+,(aN)+ n times
    let scratch = |at: usize, n: usize| -> Option<(usize, usize, u8)> {
        let l = dec(d, at)?;
        let (K::Lea, Ea::A(ra)) = (l.k, l.dst) else { return None };
        let s = match l.src {
            Ea::Pc(s) => s,
            e => img_disp(ctx, e)?,
        };
        if ra == 0 {
            return None;
        }
        let mut p = l.next();
        for _ in 0..n {
            let m = dec(d, p)?;
            if !mb(&m, Ea::Post(ra)) {
                return None;
            }
            p = m.next();
        }
        Some((s, p, ra))
    };
    if nextpat(d, h, ctx, stub, run).is_some() {
        return Some(Cmd::NextPat);
    }
    if i0.k == K::Addq && i0.data == 1 && i0.dst == Ea::A(0) {
        let (s, p, ra) = scratch(i0.next(), 3)?;
        let a = dec(d, p)?;
        let b = dec(d, a.next())?;
        let c = dec(d, b.next())?;
        if !(a.k == K::Movea && a.src == Ea::Pre(ra) && b.k == K::Adda && b.src == Ea::A(base) && c.k == K::Move && c.size == 4) {
            return None;
        }
        hw_disp(ctx, c.dst)?;
        let mut extra = Vec::new();
        let mut p = c.next();
        for _ in 0..8 {
            let i = dec(d, p)?;
            if matches!(i.k, K::Bra | K::Jmp) {
                return Some(Cmd::Sample { scratch: s, extra });
            }
            extra.push(ctx.store(&i)?);
            p = i.next();
        }
        return None;
    }
    if let Some((s, p, ra)) = scratch(h, 2) {
        let a = dec(d, p)?;
        if a.k == K::Move && a.size == 4 && a.src == Ea::A(0) {
            return Some(Cmd::Call { ret: img_disp(ctx, a.dst)?, scratch: s });
        }
        let b = dec(d, a.next())?;
        let c = dec(d, b.next())?;
        if a.k == K::Movea && a.src == Ea::Pre(ra) && b.k == K::Adda && b.src == Ea::A(base) {
            if c.k == K::Move && c.size == 4 && c.src == Ea::A(ra) {
                return Some(Cmd::SetPtr { cell: img_disp(ctx, c.dst)?, scratch: s });
            }
            if c.k == K::Move && c.size == 4 && c.src == Ea::Post(ra) {
                return Some(Cmd::SampleRec { scratch: s });
            }
        }
        return None;
    }
    if mb(&i0, Ea::D(1)) {
        if let (K::Add, Ea::Pc(mv), Ea::D(1)) = (i1.k, i1.src, i1.dst) {
            return Some(Cmd::MasterRel { mv });
        }
        return None;
    }
    if i0.k == K::Moveq && i0.dst == Ea::D(1) && is_bcc(&i1, EQ) {
        let mut p = i1.next();
        for _ in 0..4 {
            let i = dec(d, p)?;
            if let (K::Lea, Ea::AbsL { pos, .. }, Ea::A(0)) = (i.k, i.src, i.dst) {
                return Some(Cmd::Ret { flag: i0.at + 1, ret: pos });
            }
            p = i.next();
        }
        return None;
    }
    if let (K::Lea, Ea::AbsL { .. }, Ea::A(0)) = (i0.k, i0.src, i0.dst) {
        return Some(if mb(&i1, Ea::D(0)) { Cmd::NextPat } else { Cmd::Bogus });
    }
    // the stores, then the pattern bytes into cells, then PAT
    let stops = [nextvoice, loop_at];
    let mut pre = Vec::new();
    let mut cells = Vec::new();
    let mut p = h;
    for _ in 0..24 {
        let i = follow(d, p, &stops)?;
        if mb(&i, i.dst) && i.src == Ea::Post(0) && img_disp(ctx, i.dst).is_some() {
            cells.push(img_disp(ctx, i.dst)?);
        } else if i.k == K::Move && i.size == 4 && i.src == Ea::A(0) {
            let pat = img_disp(ctx, i.dst)?;
            let n = dec(d, i.next())?;
            let rest = n.k == K::Bra && n.target == nextvoice;
            return match (cells.len(), rest) {
                (2, false) => Some(Cmd::Event { pre, note: Some(cells[0]), cnt: cells[1], pat, rest: false, look: n.at }),
                (1, _) => Some(Cmd::Event { pre, note: None, cnt: cells[0], pat, rest, look: n.at }),
                _ => None,
            };
        } else if i.k == K::Bra && i.target == loop_at && cells.len() == 1 && pre.is_empty() {
            return Some(Cmd::SetB(cells[0]));
        } else if !cells.is_empty() {
            return None;
        } else {
            pre.push(ctx.store(&i)?);
        }
        p = i.next();
    }
    None
}

/// The effects block (4.4).
fn effects(d: &[u8], e: usize, ctx: &Ctx) -> Option<(Eff, u32)> {
    let mut f = Eff::default();
    let mut moveqs = Vec::new();
    let mut leas = Vec::new();
    let mut rests = Vec::new();
    let mut p = e;
    for _ in 0..40 {
        let i = dec(d, p)?;
        match (i.k, i.src, i.dst) {
            (K::Moveq, _, Ea::D(1)) => moveqs.push(i.at + 1),
            (K::Bcc, _, _) if i.cond == MI => rests.push(i.target),
            (K::Lea, Ea::AbsL { pos, .. }, Ea::A(0)) => leas.push(pos),
            (K::And, Ea::D(r), Ea::D(1)) if i.size == 1 => f.mask = Some(ctx.d[r as usize]? as u8),
            (K::Move, Ea::D(1), _) if i.size == 1 => f.volidx = img_disp(ctx, i.dst)?,
            (K::Move, Ea::Imm { pos, .. }, Ea::D(1)) if i.size == 2 => f.note = pos,
            (K::Add, Ea::D(1), Ea::D(1)) if i.size == 2 => f.double = true,
            (K::Shift, _, Ea::D(1)) => f.shift = i.at,
            (K::Move, Ea::D(1), _) if i.size == 2 => {
                let per = hw_disp(ctx, i.dst)?;
                // EFF, VRS, VOLB, (the clamp's moveq #0), PRS
                if moveqs.len() < 4 || leas.len() < 2 || rests.len() < 2 {
                    return None;
                }
                f.eff = moveqs[0];
                f.vrs = moveqs[1];
                f.volb = moveqs[2];
                f.prs = *moveqs.last()?;
                f.venvp = leas[0];
                f.penvp = leas[1];
                let rest_lea = |at: usize| -> Option<usize> {
                    let i = dec(d, at)?;
                    match (i.k, i.src) {
                        (K::Lea, Ea::AbsL { pos, .. }) => Some(pos),
                        _ => None,
                    }
                };
                f.venvs = rest_lea(rests[0])?;
                f.penvs = rest_lea(rests[1])?;
                return (f.note != 0 && f.shift != 0 && f.volidx != 0).then_some((f, per));
            }
            _ => {}
        }
        p = i.next();
    }
    None
}

/// One voice block (4.2); returns it and the next block's start.
fn voice(d: &[u8], p: usize, ctx: &Ctx, stub: usize, run: usize) -> Option<(Voice, usize)> {
    let i = dec(d, p)?;
    let (cnt, neg_form, after) = match (i.k, i.src, i.dst) {
        (K::Lea, Ea::Pc(c), Ea::A(0)) => {
            let s = dec(d, i.next())?;
            let c1 = dec(d, s.next())?;
            let b = dec(d, c1.next())?;
            if !(s.k == K::Subq && s.dst == Ea::Ind(0) && c1.k == K::Imm(6) && is_bcc(&b, CS)) {
                return None;
            }
            (c, false, b)
        }
        (K::Subq, _, _) if i.size == 1 && i.data == 1 => {
            let c = img_disp(ctx, i.dst)?;
            let n = dec(d, i.next())?;
            if is_bcc(&n, MI) {
                (c, true, n)
            } else {
                let b = dec(d, n.next())?;
                if !(n.k == K::Imm(6) && is_bcc(&b, CS)) {
                    return None;
                }
                (c, false, b)
            }
        }
        _ => return None,
    };
    let parse = after.target;
    let bne = dec(d, after.next())?;
    let cutq = dec(d, bne.next())?;
    let bmi = dec(d, cutq.next())?;
    if !(is_bcc(&bne, NE) && cutq.k == K::Moveq && cutq.dst == Ea::D(1) && is_bcc(&bmi, MI)) {
        return None;
    }
    let mut cutacts = Vec::new();
    let mut q = bmi.next();
    let nextvoice = loop {
        let i = dec(d, q)?;
        if i.k == K::Bra {
            break i.target;
        }
        cutacts.push(ctx.store(&i)?);
        if cutacts.len() > 8 {
            return None;
        }
        q = i.next();
    };
    // PARSE: lea PAT.l,a0 ; move.b (a0)+,d0 ; jmp TAB(pc,d0.w)
    let l = dec(d, parse)?;
    let (K::Lea, Ea::AbsL { pos: pat, .. }, Ea::A(0)) = (l.k, l.src, l.dst) else { return None };
    let m = dec(d, l.next())?;
    let j = dec(d, m.next())?;
    let (K::Jmp, Ea::PcIdx(tab, _)) = (j.k, j.dst) else { return None };
    let loop_at = m.at;
    let mut hs = Vec::new();
    let mut o = tab;
    while o + 4 <= d.len() && w(d, o) == 0x6000 && hs.len() < 64 {
        hs.push((o as i64 + 2 + w(d, o + 2) as i16 as i64) as usize);
        o += 4;
    }
    hs.push(o);
    let mut cmds = Vec::new();
    let mut np = None;
    for &h in &hs {
        let c = handler(d, h, ctx, nextvoice, loop_at, stub, run).unwrap_or(Cmd::Bogus);
        if c == Cmd::NextPat && np.is_none() {
            np = nextpat(d, h, ctx, stub, run);
        }
        cmds.push(c);
    }
    let flag = cmds.iter().find_map(|c| match c {
        Cmd::Ret { flag, .. } => Some(*flag),
        _ => None,
    });
    let (eff, _) = effects(d, bne.target, ctx)?;
    Some((Voice { cnt, neg_form, cut: cutq.at + 1, cutacts, pat, cmds, np: np?, eff, flag }, nextvoice))
}

/// The final block (4.5).
fn final_block(d: &[u8], f: usize, ctx: &Ctx) -> Option<Final> {
    let mut p = f;
    let mut fade = None;
    let i = dec(d, p)?;
    let n = dec(d, i.next())?;
    if i.k == K::Moveq && i.dst == Ea::D(0) && is_bcc(&n, EQ) {
        let s = dec(d, n.next())?;
        let b = dec(d, s.next())?;
        let r = dec(d, b.next())?;
        let fc = img_disp(ctx, s.dst)?;
        let Ea::Imm { pos, .. } = r.src else { return None };
        fade = Some(Fade { fade: i.at + 1, fc, reload: pos });
        p = n.target;
    }
    let mut mv = None;
    let mut mvsub = None;
    let mut vt = None;
    let mut vols = Vec::new();
    let mut led = false;
    for _ in 0..60 {
        let i = dec(d, p)?;
        match (i.k, i.src, i.dst) {
            (K::Moveq, _, Ea::D(0)) if mv.is_none() => mv = Some(i.at + 1),
            (K::Sub, Ea::D(r), Ea::D(0)) if mvsub.is_none() => mvsub = Some(ctx.d[r as usize]? as u8),
            (K::Lea, Ea::Pc(t), Ea::A(0)) => vt = Some(t),
            (K::Moveq, _, Ea::D(1)) => vols.push(i.at + 1),
            (K::Bit(3), Ea::Imm { val: 1, .. }, Ea::AbsL { addr: 0xBFE001, .. }) => led = true,
            (K::Rts, _, _) => break,
            _ => {}
        }
        p = i.next();
    }
    let vols: [usize; 4] = vols.try_into().ok()?;
    Some(Final { fade, mv: mv?, mvsub: mvsub?, vt: vt?, vols, led })
}

/// The stop stub (4.6).
fn stop_stub(d: &[u8], s: usize, pctx: &Ctx) -> Option<Stub> {
    let i = dec(d, s)?;
    let t = dec(d, i.next())?;
    if !(i.k == K::Moveq && i.dst == Ea::D(0) && t.k == K::Bcc && (t.cond == EQ || t.cond == MI)) {
        return None;
    }
    let mut ctx = *pctx;
    ctx.run = None;
    ctx.d[0] = Some(i.data as u32);
    let mut writes = Vec::new();
    let mut set = None;
    let mut p = t.next();
    for _ in 0..24 {
        let i = dec(d, p)?;
        if i.k == K::Rts {
            break;
        }
        if let Some(a) = ctx.store(&i) {
            match a.t {
                Tgt::Hw(h) => writes.push((h, a.val as u16)),
                Tgt::Img(o) if o == s + 1 => set = Some(a.val as u8),
                _ => {}
            }
        } else if let (K::Move, Ea::D(_), Some(Tgt::Hw(h))) = (i.k, i.src, ctx.addr(i.dst)) {
            writes.push((h, 0));
        }
        ctx.step(&i);
        p = i.next();
    }
    Some(Stub { at: s, flag: s + 1, set: set?, bmi: t.cond == MI, writes })
}

/// The init walk (4.7).
fn init_walk(d: &[u8], init: usize) -> Option<(Vec<InitEv>, Option<[usize; 4]>, bool)> {
    let mut ctx = Ctx::default();
    let mut ev = Vec::new();
    let mut d0_param = true;
    let mut pos = Vec::new();
    let mut mega = false;
    let mut p = init;
    for _ in 0..400 {
        let i = dec(d, p)?;
        if i.k == K::Rts {
            let posstart = pos.try_into().ok();
            return Some((ev, posstart, mega));
        }
        if let (K::Move, Ea::A(0), Ea::AbsW(0x70)) = (i.k, i.src, i.dst) {
            mega = true;
        }
        if let (K::Lea, Ea::Pc(t), Ea::A(0)) = (i.k, i.src, i.dst) {
            if let Some(n) = dec(d, i.next()) {
                if let (K::Lea, Ea::Pc(_), Ea::A(4)) = (n.k, n.src, n.dst) {
                    pos.push(t);
                }
            }
        }
        match (i.k, i.src, i.dst) {
            (K::Bit(3), Ea::Imm { val: 1, .. }, Ea::AbsL { addr: 0xBFE001, .. }) => ev.push(InitEv::Led),
            (K::Move, Ea::D(0), _) if i.size == 2 && d0_param && img_disp(&ctx, i.dst).is_some() => {
                ev.push(InitEv::SubWord(img_disp(&ctx, i.dst)?));
            }
            (K::Move, Ea::Pc(src), _) if i.size == 1 && img_disp(&ctx, i.dst).is_some() => {
                ev.push(InitEv::MasterCopy(src, img_disp(&ctx, i.dst)?));
            }
            (K::Move, Ea::Disp(..), _) if i.size == 1 && img_disp(&ctx, i.src).is_some() && img_disp(&ctx, i.dst).is_some() => {
                ev.push(InitEv::MasterCopy(img_disp(&ctx, i.src)?, img_disp(&ctx, i.dst)?));
            }
            _ => {
                if let Some(a) = ctx.store(&i) {
                    match a.t {
                        Tgt::Hw(h) => {
                            if a.size == 4 {
                                ev.push(InitEv::Hw(h, (a.val >> 16) as u16));
                                ev.push(InitEv::Hw(h + 2, a.val as u16));
                            } else {
                                ev.push(InitEv::Hw(h, a.val as u16));
                            }
                        }
                        Tgt::Img(o) => {
                            if !(a.size == 2 && o >= 2 && w(d, o - 2) == 0x4EEA) && o < d.len() {
                                ev.push(InitEv::Store(o, a.size, a.val));
                            }
                        }
                    }
                }
            }
        }
        if matches!(i.dst, Ea::D(0)) && !matches!(i.k, K::Cmp | K::Tst | K::Bit(0)) {
            d0_param = false;
        }
        ctx.step(&i);
        p = i.next();
    }
    None
}

/// The interrupt handler's LC'/LEN' cells (4.8).
fn irq_cells(d: &[u8]) -> Option<[(usize, usize); 4]> {
    let mut out = [(0, 0); 4];
    for (k, c) in out.iter_mut().enumerate() {
        let lc = 0xDFF0A0 + 16 * k as u32;
        let mut o = 0;
        let found = loop {
            if o + 18 > d.len().min(0x400) {
                break None;
            }
            if w(d, o) == 0x23FC && be32(d, o + 6) == lc && w(d, o + 10) == 0x33FC && be32(d, o + 14) == lc + 4 {
                break Some((o + 2, o + 12));
            }
            o += 2;
        };
        *c = found?;
    }
    Some(out)
}

fn layout(d: &[u8]) -> Option<Layout> {
    let e = entry(d)?;
    let (ctx, mut p, stub_at, led) = prologue(d, e.play)?;
    let stub = stop_stub(d, stub_at, &ctx)?;
    let mut voices = Vec::new();
    for _ in 0..4 {
        let (v, n) = voice(d, p, &ctx, stub_at, e.play + 1)?;
        voices.push(v);
        p = n;
    }
    let fin = final_block(d, p, &ctx)?;
    let (initev, posstart, mega) = init_walk(d, e.init)?;
    if e.songs.is_none() && posstart.is_none() {
        return None;
    }
    let irq = if mega { Some(irq_cells(d)?) } else { None };
    // the Eagleplayer's InitPlayer also wants "Sean" after the end-of-list target
    let mut o = e.init;
    let negat = loop {
        if o + 4 > d.len() {
            return None;
        }
        if w(d, o) == 0x1018 && d[o + 2] == 0x6B {
            break (o as i64 + 4 + d[o + 3] as i8 as i64) as usize;
        }
        o += 2;
    };
    let end = d.len().min(negat + 32767);
    if negat >= end || !d[negat..end].windows(4).any(|x| x == b"Sean") {
        return None;
    }
    // the lookahead's two constants: the compares of its code, in order
    let look = voices[0].cmds.iter().find_map(|c| match c {
        Cmd::Event { rest: false, look, .. } => Some(*look),
        _ => None,
    })?;
    let mut lk = Vec::new();
    let mut q = look;
    for _ in 0..8 {
        let i = dec(d, q)?;
        match (i.k, i.src, i.dst) {
            (K::Imm(6), Ea::Imm { val, .. }, _) => lk.push(val as i8),
            (K::Cmp, Ea::D(r), _) if ctx.d[r as usize].is_some() && !matches!(i.dst, Ea::D(x) if ctx.d[x as usize].is_some()) => {
                lk.push(ctx.d[r as usize]? as i8)
            }
            (K::Cmp, _, Ea::D(r)) => lk.push(ctx.d[r as usize]? as i8),
            _ => {}
        }
        q = i.next();
    }
    let [d3, d2, ..] = lk[..] else { return None };
    Some(Layout {
        init: e.init,
        play: e.play,
        run: e.play + 1,
        led,
        d2,
        d3,
        voices,
        fin,
        stub,
        initev,
        subs: e.subs,
        songs: e.songs,
        posstart,
        irq,
        mega,
    })
}

// ------------------------------------------------------------------- the engine

struct SeanConran {
    lay: Layout,
    pristine: Vec<u8>,
    img: Vec<u8>,
    base: u32,
    sub: u16,
    started: bool,
    songend: bool,
    armed: [bool; 4],
}

/// How a voice's parse ended.
enum Ended {
    Effects,
    Next,
    Stop,
}

impl SeanConran {
    fn b(&self, a: u32) -> u8 {
        let o = a.wrapping_sub(VIRT) as usize;
        self.img.get(o).copied().unwrap_or(0)
    }
    fn ib(&self, o: usize) -> u8 {
        self.img.get(o).copied().unwrap_or(0)
    }
    fn iw(&self, o: usize) -> u16 {
        (self.ib(o) as u16) << 8 | self.ib(o + 1) as u16
    }
    fn il(&self, o: usize) -> u32 {
        (self.iw(o) as u32) << 16 | self.iw(o + 2) as u32
    }
    fn set_b(&mut self, o: usize, v: u8) {
        if let Some(x) = self.img.get_mut(o) {
            *x = v;
        }
    }
    fn set_w(&mut self, o: usize, v: u16) {
        self.set_b(o, (v >> 8) as u8);
        self.set_b(o + 1, v as u8);
    }
    fn set_l(&mut self, o: usize, v: u32) {
        self.set_w(o, (v >> 16) as u16);
        self.set_w(o + 2, v as u16);
    }

    fn guest(&self, a: u32) -> u32 {
        self.base.wrapping_add(a.wrapping_sub(VIRT))
    }

    fn act(&mut self, a: &Act, hw: &mut Hw) {
        match a.t {
            Tgt::Img(o) => match a.size {
                1 => self.set_b(o, a.val as u8),
                2 => self.set_w(o, a.val as u16),
                _ => self.set_l(o, a.val),
            },
            Tgt::Hw(h) => {
                if a.size == 4 {
                    hw.bus.write_word(h, (a.val >> 16) as u16);
                    hw.bus.write_word(h + 2, a.val as u16);
                } else {
                    hw.bus.write_word(h, a.val as u16);
                }
                if h == HWBASE {
                    self.dma_on(a.val as u16, hw);
                }
                hw.wait(T_WRITE);
            }
        }
    }

    /// The audio interrupt (5.9): a DMA-on of an armed channel loads its loop.
    fn dma_on(&mut self, v: u16, hw: &mut Hw) {
        let Some(irq) = self.lay.irq else { return };
        if v & 0x8000 == 0 {
            return;
        }
        for (k, &(lc, len)) in irq.iter().enumerate() {
            if v & 1 << k != 0 && self.armed[k] {
                // the interrupt's latency: Paula takes the attack's location first
                hw.wait(T_IRQ);
                self.armed[k] = false;
                hw.bus.write_word(0xDFF09A, 0x80 << k);
                hw.bus.write_word(0xDFF09C, 0x80 << k);
                let a = self.il(lc);
                hw.loc(k, self.guest(a));
                hw.len(k, self.iw(len));
                hw.wait(T_WRITE);
            }
        }
    }

    fn led(&self, hw: &mut Hw, set: bool) {
        let x = hw.bus.read_byte(0xBFE001);
        hw.bus.write_byte(0xBFE001, if set { x | 2 } else { x & !2 });
    }

    /// The song start (5.1) for subsong `s`.
    fn start(&mut self, s: u16, hw: &mut Hw) {
        self.img = self.pristine.clone();
        self.songend = false;
        if self.lay.mega {
            // the wrapper's relocation of channel 2's LC' and its cache-fix words (6.1)
            if be32(&self.img, 588) == 0x3B76 {
                self.set_l(588, 0x3B76 - 5236 + VIRT);
            }
            for (k, v) in [0x02E6, 0x02FE, 0x0316, 0x032E].into_iter().enumerate() {
                self.set_w(self.lay.init + 272 + 40 * k, v);
            }
        }
        if be32(&self.img, self.lay.init + 150) == 0x0002_0A69 {
            self.set_b(self.lay.init + 2221, 2);
        }
        let ev = self.lay.initev.clone();
        for e in ev {
            match e {
                InitEv::Store(o, size, val) => self.act(&Act { t: Tgt::Img(o), size, val }, hw),
                InitEv::Hw(h, v) => {
                    hw.bus.write_word(h, v);
                    hw.wait(T_WRITE);
                }
                InitEv::SubWord(o) => self.set_w(o, 0x0F00 + s),
                InitEv::MasterCopy(src, dst) => {
                    let v = self.ib(src);
                    self.set_b(dst, v);
                }
                InitEv::Led => self.led(hw, true),
            }
        }
        for v in 0..4 {
            let list = match (self.lay.songs, self.lay.posstart) {
                (Some(t), _) => self.iw(t + 8 * s as usize + 2 * v) as usize,
                (None, Some(p)) => p[v],
                _ => return,
            };
            let test = self.lay.songs.is_none() && self.lay.voices[v].np.neg == NegKind::Loop;
            self.set_l(self.lay.voices[v].np.pos, VIRT + list as u32);
            self.position(v, test);
        }
        self.armed = [self.lay.irq.is_some(); 4];
    }

    /// A position read (5.4); false when the song stops.
    fn position(&mut self, v: usize, test: bool) -> bool {
        let np = self.lay.voices[v].np.clone();
        let mut pos = self.il(np.pos);
        let mut c = self.b(pos);
        pos = pos.wrapping_add(1);
        let mut guard = 0;
        while test && (c as i8) < 0 && np.neg != NegKind::None {
            if v == 0 {
                self.songend = true;
            }
            guard += 1;
            if guard > 64 {
                return false;
            }
            let op = self.b(pos);
            match np.neg {
                NegKind::Loop => {
                    pos = pos.wrapping_sub(op as u32);
                    c = self.b(pos);
                    pos = pos.wrapping_add(1);
                    continue;
                }
                NegKind::StopFf if op == 0xFF => return false,
                NegKind::StopNeg if (op as i8) < 0 => return false,
                NegKind::StopFf | NegKind::StopNeg => {
                    pos = pos.wrapping_sub(op as u32);
                    c = self.b(pos);
                    pos = pos.wrapping_add(1);
                }
                _ => return false,
            }
            break;
        }
        self.set_l(np.pos, pos);
        let idx = if np.double { 2 * c as usize } else { c as usize };
        let p = VIRT.wrapping_add(self.iw(np.tab + idx) as i16 as i32 as u32);
        self.set_l(self.lay.voices[v].pat, p);
        true
    }

    /// The lookahead after a note or duration (4.3).
    fn lookahead(&mut self, v: usize, mut q: u32) {
        for _ in 0..64 {
            let c = self.b(q) as i8;
            if c < self.lay.d3 {
                self.set_b(self.lay.voices[v].cut, 0xFF);
                return;
            }
            if c == self.lay.d2 {
                q = q.wrapping_add(2);
                continue;
            }
            break;
        }
        self.set_b(self.lay.voices[v].cut, 0);
    }

    /// Parses voice `v`'s pattern bytes up to the end of an event (5.3).
    fn parse(&mut self, v: usize, hw: &mut Hw) -> Ended {
        let mut a0 = self.il(self.lay.voices[v].pat);
        for _ in 0..256 {
            let c = self.b(a0) as usize;
            a0 = a0.wrapping_add(1);
            let cmd = if c % 4 == 0 { self.lay.voices[v].cmds.get(c / 4).cloned() } else { None };
            let Some(cmd) = cmd else { return Ended::Next };
            let op2 = |s: &Self, a: u32| (s.b(a), s.b(a.wrapping_add(1)));
            match cmd {
                Cmd::SetB(x) => {
                    let b = self.b(a0);
                    self.set_b(x, b);
                    a0 = a0.wrapping_add(1);
                }
                Cmd::SetPtr { cell, scratch } => {
                    let (h, l) = op2(self, a0);
                    a0 = a0.wrapping_add(2);
                    self.set_b(scratch, h);
                    self.set_b(scratch + 1, l);
                    let x = VIRT.wrapping_add(((h as u16) << 8 | l as u16) as i16 as i32 as u32);
                    self.set_l(cell, x);
                }
                Cmd::Call { ret, scratch } => {
                    let (h, l) = op2(self, a0);
                    a0 = a0.wrapping_add(2);
                    self.set_b(scratch, h);
                    self.set_b(scratch + 1, l);
                    self.set_l(ret, a0);
                    a0 = VIRT.wrapping_add(((h as u16) << 8 | l as u16) as i16 as i32 as u32);
                    if let Some(f) = self.lay.voices[v].flag {
                        self.set_b(f, 0xFF);
                    }
                }
                Cmd::Ret { flag, ret } => {
                    if self.ib(flag) != 0 {
                        self.set_b(flag, 0);
                        a0 = self.il(ret);
                    }
                }
                Cmd::Sample { scratch, extra } => {
                    a0 = a0.wrapping_add(1);
                    for k in 0..3 {
                        let b = self.b(a0);
                        a0 = a0.wrapping_add(1);
                        self.set_b(scratch + k, b);
                    }
                    let off = self.il(scratch - 1);
                    hw.loc(v, self.base.wrapping_add(off));
                    hw.wait(T_WRITE);
                    for a in &extra {
                        self.act(a, hw);
                    }
                }
                Cmd::SampleRec { scratch } => {
                    let (h, l) = op2(self, a0);
                    a0 = a0.wrapping_add(2);
                    self.set_b(scratch, h);
                    self.set_b(scratch + 1, l);
                    let r = ((h as u16) << 8 | l as u16) as i16 as isize as usize;
                    let (lc, len, lc2, len2) = (self.il(r), self.iw(r + 4), self.il(r + 6), self.iw(r + 10));
                    hw.loc(v, self.base.wrapping_add(lc));
                    hw.len(v, len);
                    if let Some(irq) = self.lay.irq {
                        self.set_l(irq[v].0, VIRT.wrapping_add(lc2));
                        self.set_w(irq[v].1, len2);
                    }
                    hw.bus.write_word(0xDFF09C, 0x80 << v);
                    hw.bus.write_word(0xDFF09A, 0xC000 | 0x80 << v);
                    self.armed[v] = true;
                    hw.per(v, 0xFFFF);
                    hw.wait(T_WRITE);
                    hw.dmacon(0x8200 | 1 << v);
                    self.dma_on(0x8200 | 1 << v, hw);
                    hw.wait(T_WRITE);
                }
                Cmd::MasterRel { mv } => {
                    let x = self.b(a0).wrapping_add(self.ib(mv));
                    a0 = a0.wrapping_add(1);
                    let x = if (x as i8) < 0 { 0 } else { x.min(15) };
                    self.set_b(mv, x);
                }
                Cmd::Event { pre, note, cnt, pat, rest, .. } => {
                    for a in &pre {
                        self.act(a, hw);
                    }
                    if let Some(n) = note {
                        let b = self.b(a0);
                        self.set_b(n, b);
                        a0 = a0.wrapping_add(1);
                    }
                    let b = self.b(a0);
                    self.set_b(cnt, b);
                    a0 = a0.wrapping_add(1);
                    self.set_l(pat, a0);
                    if rest {
                        return Ended::Next;
                    }
                    self.lookahead(v, a0);
                    return Ended::Effects;
                }
                Cmd::NextPat => {
                    let test = self.lay.voices[v].np.neg != NegKind::None;
                    if !self.position(v, test) {
                        return Ended::Stop;
                    }
                    a0 = self.il(self.lay.voices[v].pat);
                }
                Cmd::Bogus => return Ended::Next,
            }
        }
        Ended::Next
    }

    /// One envelope step: the next byte of the envelope whose restart flag, start and
    /// pointer cells are given; the 0x80 marker loops back.
    fn envelope(&mut self, rs: usize, start: usize, ptr: usize) -> u8 {
        let mut a = if (self.ib(rs) as i8) < 0 {
            self.set_b(rs, 0);
            self.il(start)
        } else {
            let a = self.il(ptr);
            if self.b(a) == 0x80 {
                a.wrapping_sub(self.b(a.wrapping_add(1)) as u32)
            } else {
                a
            }
        };
        let x = self.b(a);
        a = a.wrapping_add(1);
        self.set_l(ptr, a);
        x
    }

    /// The effects (5.5).
    fn effects(&mut self, v: usize, hw: &mut Hw) {
        let f = self.lay.voices[v].eff.clone();
        if self.ib(f.eff) == 0 {
            return;
        }
        let e = self.envelope(f.vrs, f.venvs, f.venvp);
        let mut x = self.ib(f.volb).wrapping_add(e);
        if (x as i8) < 0 {
            x = 0;
        }
        if let Some(m) = f.mask {
            x &= m;
        }
        self.set_b(f.volidx, x);
        let e = self.envelope(f.prs, f.penvs, f.penvp);
        let nw = self.iw(f.note);
        let mut dd = nw & 0xFF00 | (nw as u8).wrapping_add(e) as u16;
        if f.double {
            dd = dd.wrapping_add(dd);
        }
        let mut p = self.iw(dd as i16 as isize as usize);
        let sh = self.iw(f.shift);
        let n = match (sh >> 9) & 7 {
            0 => 8,
            n => n,
        };
        p = if sh & 0x100 != 0 { p << n } else { p >> n };
        hw.per(v, p);
        hw.wait(T_WRITE);
    }

    /// The stop stub (5.8).
    fn stub(&mut self, direct: bool, hw: &mut Hw) {
        let s = self.lay.stub.clone();
        if !direct {
            let sf = self.ib(s.flag);
            if if s.bmi { (sf as i8) < 0 } else { sf == 0 } {
                return;
            }
        }
        for &(h, v) in &s.writes {
            hw.bus.write_word(h, v);
            hw.wait(T_WRITE);
        }
        self.set_b(s.flag, s.set);
    }

    fn play(&mut self, hw: &mut Hw) {
        if self.ib(self.lay.run) == 0 {
            self.stub(false, hw);
            return;
        }
        if let Some(set) = self.lay.led {
            self.led(hw, set);
        }
        for v in 0..4 {
            let vc = &self.lay.voices[v];
            let (cnt, neg_form, cut) = (vc.cnt, vc.neg_form, vc.cut);
            let c = self.ib(cnt).wrapping_sub(1);
            self.set_b(cnt, c);
            let parse = if neg_form { (c as i8) < 0 } else { c == 0 };
            let ended = if parse {
                self.parse(v, hw)
            } else if c != if neg_form { 0 } else { 1 } || (self.ib(cut) as i8) < 0 {
                Ended::Effects
            } else {
                let acts = self.lay.voices[v].cutacts.clone();
                for a in &acts {
                    self.act(a, hw);
                }
                Ended::Next
            };
            match ended {
                Ended::Effects => self.effects(v, hw),
                Ended::Next => {}
                Ended::Stop => {
                    self.set_b(self.lay.run, 0);
                    let direct = self.lay.voices[v].np.direct;
                    self.stub(direct, hw);
                    return;
                }
            }
            hw.wait(T_VOICE);
        }
        let f = self.lay.fin.clone();
        if let Some(fd) = &f.fade {
            if self.ib(fd.fade) != 0 {
                let fc = self.iw(fd.fc).wrapping_sub(1);
                self.set_w(fd.fc, fc);
                if fc == 0 {
                    let r = self.iw(fd.reload);
                    self.set_w(fd.fc, r);
                    let mv = self.ib(f.mv).wrapping_sub(1);
                    self.set_b(f.mv, mv);
                    if mv == 0 {
                        self.set_b(self.lay.run, 0);
                    }
                }
            }
        }
        let d0 = self.ib(f.mv).wrapping_sub(f.mvsub);
        for v in 0..4 {
            let x = self.ib(f.vols[v]).wrapping_add(d0);
            let vol = if (x as i8) < 0 { 0 } else { self.iw(f.vt + 2 * x as usize) };
            hw.vol(v, vol);
            hw.wait(T_WRITE);
        }
        if f.led {
            self.led(hw, true);
        }
    }
}

impl Replayer for SeanConran {
    fn format(&self) -> &'static str {
        "Sean Conran"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.subs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (n as usize) < self.lay.subs {
            self.sub = n;
            self.started = false;
        }
    }

    fn audio_irq_ch(&mut self, _ch: usize, _hw: &mut Hw) -> bool {
        // the interrupt's writes are made with the DMA-on that raises it
        true
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.start(self.sub, hw);
        }
        self.play(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !header(d) {
        return None;
    }
    let lay = layout(d)?;
    // the LED bit its init and play routines write (bset: the filter off) is set before
    // the first sample plays: the original's start-up (the song's precalculation,
    // seconds on a 68000) is over by then, with the init and a first run of the
    // play routine behind it
    if lay.initev.iter().any(|e| matches!(e, InitEv::Led)) || lay.led == Some(true) {
        m.bus.ciaa.pra |= 2;
    }
    Some(Box::new(SeanConran {
        lay,
        pristine: d.to_vec(),
        img: d.to_vec(),
        base,
        sub: 0,
        started: false,
        songend: false,
        armed: [false; 4],
    }))
}

#[cfg(test)]
#[path = "seanconran_tests.rs"]
mod tests;
