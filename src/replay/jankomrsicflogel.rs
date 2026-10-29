//! Janko Mrsic-Flogel (`jmf.*`), the three-voice music driver of Bomb Jack, Fire
//! Blaster, Luxor, Protector, Space Station and War Zone (Paradox Software and
//! others, 1986-88).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. A file is an AmigaDOS load file of one code hunk (a `J.FLOGEL`
//! signature and the init, play and subsong fields at its head) that carries its
//! own replayer; the code is never run. It keeps its constants in its own
//! instructions, so they are read from the image structurally with the 68000
//! decoder (`m68kdec`): the gate word (a `tst.w` at the top of the play routine),
//! the step length (the `move.b #n` that reloads the step counter), the three
//! channels' mode jump tables (nine `bra.w` each) with the shape of each mode's
//! handler (the sample pointer and length written, the period routine called,
//! the shift of the period, the DMA word), the period routine's table and note
//! offset, the subsong start triples of the init routine, and the restart block
//! (the Paula writes of the restart, in order).
//!
//! The engine is written from the behaviour: a step every N ticks, three
//! channels each with a duration counter, a mode and a stream pointer; the
//! stream grammar (`FF` restarts, `FE x` sets the mode, any other byte is a
//! note's duration followed by what the mode reads); the host's volume write
//! (56) after each period. The first restart of a run after the init is the
//! song end; playback goes on with the streams restarted.
//!
//! Decided where the specification is silent: the first tick of a run is the
//! init (the restart block) and the first step comes in the next; a mode the
//! jump table does not have reads nothing and writes nothing; reads outside the
//! image give 0 and the period table is read from the image (so entries past
//! the 80 listed come from the module); a note offset applies at its width (8
//! bits wraps, 16 does not); the shift of the period (when a handler has one) is
//! to the left in 16 bits; the write moments within a tick are fitted to the
//! reference runs (the constants `T_*`, see below), those of the modes the
//! corpus does not use by analogy.

use super::m68kdec::{decode, Ea, Insn, K};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Janko Mrsic-Flogel", detect, new };

/// Host volume (56 = the 64 of the Eagleplayer's volume and balance times 7/8).
const HOST_VOL: u16 = 56;

// Write moments, in colour clocks (fitted to the reference runs).
/// Tick entry to the first channel's first write, without and with a gate word.
const T_BASE: i64 = 224;
const T_GATE: i64 = 10;
/// A channel that is not due: the time it adds to the next one's start.
const T_IDLE: [i64; 3] = [30, 26, 0];
/// An `FE x` mode command.
const T_FE: i64 = 32;
/// End of a note event to the next channel's block.
const T_NEXT: i64 = 126;
/// A DMA-off action: block start to its write.
const T_OFF: i64 = -6;
/// Block start to the period write of a period-only action (by analogy).
const T_PERIOD_ONLY: i64 = 76;
/// A mode that reads a byte and does nothing (by analogy).
const T_SKIP: i64 = 40;
/// Pointer to length, length to period (length 16), pointer to period (no length).
const T_PTR_LEN: i64 = 14;
const T_LEN_PER: i64 = 86;
const T_PTR_PER: i64 = 82;
/// The same when the handler shifts the period.
const T_PTR_LEN_SH: i64 = 52;
const T_LEN_PER_SH: i64 = 42;
/// Period to volume (third voice), volume to DMA.
const T_PER_VOL: [i64; 3] = [78, 78, 76];
const T_VOL_DMA: i64 = 20;
/// The restart block's write gaps.
/// End-of-stream test to the restart block, and each immediate store before its first write.
const T_R_BASE: i64 = 8;
const T_R_STORE: i64 = 10;
const T_R_PTR_LEN: i64 = 14;
const T_R_DEFAULT: i64 = 10;
const T_R_VOL_DMA: i64 = 74;
/// The same in the init, which the host calls outside the tick.
const T_R_VOL_DMA_INIT: i64 = 26;
const T_R_DMA_INT: i64 = 14;
const T_R_INT_PTR: i64 = 134;

fn detect(d: &[u8]) -> bool {
    image(d).is_some()
}

/// The code image (the hunk's data) of a module, when the head matches.
fn image(d: &[u8]) -> Option<&[u8]> {
    if d.len() < 56 || be32(d, 0) != 0x3F3 || d[20] == 0 {
        return None;
    }
    if d[32..36] != [0x70, 0xFF, 0x4E, 0x75] || &d[36..44] != b"J.FLOGEL" {
        return None;
    }
    if be32(d, 44) == 0 || be32(d, 48) == 0 || be32(d, 52) == 0 {
        return None;
    }
    let n = (be32(d, 0x1C) & 0x3FFF_FFFF) as usize * 4;
    let img = d.get(32..)?;
    Some(&img[..n.min(img.len())])
}

/// How a note byte becomes a table entry: the offset added and the width it wraps at.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rule {
    base: usize,
    add: i32,
    mask: u32,
    /// Colour clocks the offset instruction adds to the period routine.
    cost: i64,
}

/// What a mode's handler does, read from its code.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Handler {
    ptr: Option<u32>,
    len: Option<u16>,
    shift: u8,
    period: Option<Rule>,
    /// A note byte is read and ignored (no period routine).
    skip: bool,
    dma_on: Option<u16>,
    dma_off: Option<u16>,
}

/// One Paula write of the restart block.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Rw {
    /// A sample pointer for voice `ch`, image offset.
    Ptr(usize, u32),
    Word(u32, u16),
}

#[derive(Clone, Debug)]
struct Layout {
    gate: Option<usize>,
    step: u8,
    chans: [Vec<Handler>; 3],
    starts: Vec<[u32; 3]>,
    restart: Vec<Rw>,
    /// Colour clocks from the end-of-stream test to the restart block's first write.
    r_first: i64,
    nsub: usize,
}

fn dff(a: u32) -> bool {
    (0xDFF000..0xDFF200).contains(&a)
}

fn imm_abs(i: &Insn) -> Option<(u32, u32)> {
    match (i.src, i.dst) {
        (Ea::Imm { val, .. }, Ea::AbsL { addr, .. }) => Some((val, addr)),
        _ => None,
    }
}

/// The period routine: the table and the note offset.
fn period_rule(img: &[u8], at: usize) -> Option<Rule> {
    let (mut base, mut add, mut mask, mut cost) = (None, 0i32, 0xFFFFu32, 0i64);
    let mut p = at;
    for _ in 0..24 {
        let i = decode(img, p)?;
        match (i.k, i.dst) {
            (K::Rts, _) => return Some(Rule { base: base?, add, mask, cost }),
            (K::Lea, Ea::A(2)) => {
                if let Ea::AbsL { addr, .. } = i.src {
                    base = Some(addr as usize);
                }
            }
            (K::Imm(n @ (2 | 3)), Ea::D(1)) => {
                if let Ea::Imm { val, .. } = i.src {
                    add = if n == 2 { -(val as i32) } else { val as i32 };
                    mask = if i.size == 1 { 0xFF } else { 0xFFFF };
                    cost = 4;
                }
            }
            (K::Addq | K::Subq, Ea::D(1)) => {
                add = if i.k == K::Subq { -i.data } else { i.data };
                mask = if i.size == 1 { 0xFF } else { 0xFFFF };
                cost = 2;
            }
            (K::Add | K::Sub, Ea::D(1)) => {
                if let Ea::AbsL { addr, .. } = i.src {
                    let v = if i.size == 1 { *img.get(addr as usize)? as i32 } else { be16(img, addr as usize) as i32 };
                    add = if i.k == K::Sub { -v } else { v };
                    mask = if i.size == 1 { 0xFF } else { 0xFFFF };
                    cost = 8;
                }
            }
            _ => {}
        }
        p = i.next();
    }
    None
}

/// A mode's handler, following its branches to the store of the stream pointer.
fn handler(img: &[u8], entry: usize) -> Option<Handler> {
    let mut h = Handler::default();
    let mut at = entry;
    let mut seen = Vec::new();
    let mut read = false;
    for _ in 0..64 {
        if seen.contains(&at) {
            break;
        }
        seen.push(at);
        let i = decode(img, at)?;
        match i.k {
            K::Bra => {
                at = i.target;
                continue;
            }
            K::Rts => break,
            K::Bsr => {
                h.period = period_rule(img, i.target);
                h.skip = false;
            }
            K::Move => {
                if let Some((v, a)) = imm_abs(&i) {
                    if dff(a) && i.size == 4 && a & 0xF == 0xA {
                        h.ptr = Some(v);
                    } else if dff(a) && i.size == 2 && a & 0xF == 4 {
                        h.len = Some(v as u16);
                    } else if a == 0xDFF096 && i.size == 2 {
                        if v & 0x8000 != 0 {
                            h.dma_on = Some(v as u16);
                        } else {
                            h.dma_off = Some(v as u16);
                        }
                    }
                } else if i.size == 1 && matches!(i.src, Ea::Post(1)) {
                    read = true;
                } else if i.size == 4 && i.src == Ea::A(1) && matches!(i.dst, Ea::AbsL { .. }) {
                    // the stream pointer stored back: the handler is over
                    break;
                }
            }
            K::Shift if i.dst == Ea::D(1) && i.data > 0 && be16(img, i.at) & 0x100 != 0 => {
                h.shift = i.data as u8;
            }
            _ => {}
        }
        at = i.next();
    }
    h.skip = read && h.period.is_none();
    Some(h)
}

/// The mode table at `table`: the handlers of its leading `bra.w` entries.
fn modes(img: &[u8], table: usize) -> Option<Vec<Handler>> {
    let mut v = Vec::new();
    while v.len() < 32 {
        match decode(img, table + 4 * v.len()) {
            Some(i) if i.k == K::Bra && i.len == 4 => v.push(handler(img, i.target)?),
            _ => break,
        }
    }
    (v.len() >= 9).then_some(v)
}

fn layout(d: &[u8]) -> Option<Layout> {
    let img = image(d)?;
    let (play, init) = (be32(img, 0x0C) as usize, be32(img, 0x10) as usize);
    let nsub = be32(img, 0x14) as usize;
    if play >= img.len() || init >= img.len() || nsub == 0 || nsub > 64 {
        return None;
    }
    // the play routine: the gate word, the step length, the three mode tables
    let mut at = play;
    let mut gate = None;
    let first = decode(img, at)?;
    if first.k == K::Tst && first.size == 2 {
        if let Ea::AbsL { addr, .. } = first.dst {
            gate = Some(addr as usize);
            at = first.next();
        }
    }
    let (mut counted, mut step, mut jmp, mut tables) = (false, None, false, Vec::new());
    let (mut ff, mut fe) = (false, false);
    for _ in 0..1500 {
        let i = decode(img, at)?;
        match i.k {
            K::Subq | K::Imm(2) if !counted && i.size == 1 && matches!(i.dst, Ea::AbsL { .. }) => {
                counted = true;
            }
            K::Move if counted && step.is_none() && i.size == 1 => {
                if let Some((v, a)) = imm_abs(&i) {
                    if !dff(a) {
                        step = Some(v as u8);
                    }
                }
            }
            K::Imm(6) if i.size == 1 && i.dst == Ea::D(0) => match i.src {
                Ea::Imm { val: 0xFF, .. } => ff = true,
                Ea::Imm { val: 0xFE, .. } => fe = true,
                _ => {}
            },
            K::Lea if i.dst == Ea::A(2) => {
                if let Ea::AbsL { addr, .. } = i.src {
                    tables.push(addr as usize);
                }
            }
            K::Jmp if i.dst == Ea::Ind(2) => jmp = true,
            _ => {}
        }
        if tables.len() == 3 && jmp {
            break;
        }
        at = i.next();
    }
    if tables.len() != 3 || !jmp || !ff || !fe {
        return None;
    }
    let chans = [modes(img, tables[0])?, modes(img, tables[1])?, modes(img, tables[2])?];
    // the init routine: the subsong start triples, then the restart block
    let (mut starts, mut targets) = (Vec::new(), Vec::new());
    let mut at = init;
    loop {
        let i = decode(img, at)?;
        if matches!(i.k, K::Tst | K::Imm(6) | K::Subq) && i.dst == Ea::D(0) {
            let b = decode(img, i.next())?;
            if b.k != K::Bcc {
                return None;
            }
            at = b.next();
        }
        let mut t = [0u32; 3];
        for v in &mut t {
            let m = decode(img, at)?;
            *v = match (m.k, imm_abs(&m)) {
                (K::Move, Some((val, _))) if m.size == 4 => val,
                _ => return None,
            };
            at = m.next();
        }
        let b = decode(img, at)?;
        if b.k != K::Bra {
            return None;
        }
        starts.push(t);
        targets.push(b.target);
        at = b.next();
        match decode(img, at) {
            Some(n) if matches!(n.k, K::Tst | K::Imm(6) | K::Subq) && n.dst == Ea::D(0) => {}
            Some(n) if n.k == K::Move && n.size == 4 && imm_abs(&n).is_some() => {}
            _ => break,
        }
        if starts.len() > 64 {
            return None;
        }
    }
    if targets.iter().any(|&t| t != targets[0]) {
        return None;
    }
    let mut restart = Vec::new();
    let mut r_first = T_R_BASE;
    let mut at = targets[0];
    for _ in 0..300 {
        let i = decode(img, at)?;
        if i.k == K::Rts {
            break;
        }
        if i.k == K::Move {
            if let Some((v, a)) = imm_abs(&i) {
                if restart.is_empty() {
                    // the stores before the first Paula write, and a word write itself
                    if !dff(a) {
                        r_first += T_R_STORE;
                    } else if i.size == 2 {
                        r_first += T_R_STORE;
                    }
                }
                if dff(a) {
                    if i.size == 4 && matches!(a & 0xF, 0 | 0xA) && (0xDFF0A0..0xDFF0E0).contains(&a) {
                        restart.push(Rw::Ptr(((a - 0xDFF0A0) >> 4) as usize, v));
                    } else if i.size == 4 {
                        restart.push(Rw::Word(a, (v >> 16) as u16));
                        restart.push(Rw::Word(a + 2, v as u16));
                    } else {
                        restart.push(Rw::Word(a, v as u16));
                    }
                }
            }
        }
        at = i.next();
    }
    if restart.is_empty() {
        return None;
    }
    Some(Layout { gate, step: step?, chans, starts, restart, r_first, nsub })
}

struct Jmf {
    img: Vec<u8>,
    /// Guest address of the code image.
    base: u32,
    lay: Layout,
    title: String,
    sub: usize,
    started: bool,
    songend: bool,
    step: u8,
    dur: [u8; 3],
    mode: [u8; 3],
    ptr: [usize; 3],
    /// The stall the frame interrupt put into this tick so far, and where it starts.
    shift: i64,
    ti: Option<i64>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Ptr,
    Len,
    Vol,
    Dma,
    Int,
    Other,
}

impl Jmf {
    fn rd(&self, a: usize) -> u8 {
        self.img.get(a).copied().unwrap_or(0)
    }

    fn start(&self) -> [u32; 3] {
        self.lay.starts[self.sub.min(self.lay.starts.len() - 1)]
    }

    /// Lets the clock reach `t0 + off`, plus the stall of the frame interrupt's
    /// handler if it falls inside this tick's burst.
    fn at(&mut self, hw: &mut Hw, t0: u64, off: i64) {
        let nominal = t0 as i64 + off;
        if self.shift == 0 && self.ti.is_some_and(|ti| nominal >= ti) {
            self.shift = T_VBL;
        }
        let target = nominal + self.shift;
        if target > hw.bus.clock as i64 {
            hw.wait((target - hw.bus.clock as i64) as u64);
        }
    }

    /// The restart: the counters and pointers, then the block's writes from `off`.
    fn restart(&mut self, hw: &mut Hw, t0: u64, off: i64, init: bool) {
        self.step = 1;
        self.dur = [1; 3];
        self.mode = [0; 3];
        let s = self.start();
        self.ptr = [s[0] as usize, s[1] as usize, s[2] as usize];
        let mut t = off;
        let mut prev: Option<Kind> = None;
        for w in self.lay.restart.clone() {
            let kind = match w {
                Rw::Ptr(..) => Kind::Ptr,
                Rw::Word(a, _) => match a {
                    0xDFF096 => Kind::Dma,
                    0xDFF09A => Kind::Int,
                    _ if (0xDFF0A0..0xDFF0E0).contains(&a) && a & 0xF == 4 => Kind::Len,
                    _ if (0xDFF0A0..0xDFF0E0).contains(&a) && a & 0xF == 8 => Kind::Vol,
                    _ => Kind::Other,
                },
            };
            if let Some(p) = prev {
                t += match (p, kind) {
                    (Kind::Ptr, Kind::Len) | (Kind::Ptr, Kind::Vol) => T_R_PTR_LEN,
                    (Kind::Vol, Kind::Dma) if init => T_R_VOL_DMA_INIT,
                    (Kind::Vol, Kind::Dma) => T_R_VOL_DMA,
                    (Kind::Dma, Kind::Int) => T_R_DMA_INT,
                    (Kind::Int, Kind::Ptr) => T_R_INT_PTR,
                    _ => T_R_DEFAULT,
                };
            }
            prev = Some(kind);
            self.at(hw, t0, t);
            match w {
                Rw::Ptr(c, v) => hw.loc(c, self.base.wrapping_add(v)),
                Rw::Word(0xDFF096, v) => hw.dmacon(v),
                Rw::Word(a, v) => hw.bus.write_word(a, v),
            }
        }
    }

    /// The period of note byte `b` by `r`.
    fn period(&self, r: &Rule, b: u8, shift: u8) -> u16 {
        let n = (b as i64 + r.add as i64) as u32 & r.mask;
        let d1 = ((n * 2) as u16) as i16 as i64;
        let p = be16(&self.img, (r.base as i64 + d1).max(0) as usize);
        p << shift.min(15)
    }

    /// Runs mode `m`'s action for voice `c` with the cursor at `cur`; returns the new cursor.
    fn action(&mut self, hw: &mut Hw, t0: u64, c: usize, m: u8, cur: i64) -> i64 {
        let Some(h) = self.lay.chans[c].get(m as usize).copied() else { return cur };
        let b = if h.period.is_some() || h.skip {
            let b = self.rd(self.ptr[c]);
            self.ptr[c] += 1;
            b
        } else {
            0
        };
        if h.skip {
            return cur + T_SKIP;
        }
        let mut t = cur;
        let mut wrote = false;
        if let Some(off) = h.dma_off {
            self.at(hw, t0, t + T_OFF);
            hw.dmacon(off);
            return t + T_OFF + T_NEXT;
        }
        let vol = if c < 3 { HOST_VOL } else { 0 };
        if let Some(p) = h.ptr {
            self.at(hw, t0, t);
            hw.loc(c, self.base.wrapping_add(p));
            wrote = true;
        }
        if let Some(l) = h.len {
            t += if h.shift > 0 { T_PTR_LEN_SH } else { T_PTR_LEN };
            self.at(hw, t0, t);
            hw.len(c, l);
            wrote = true;
        }
        if let Some(r) = h.period {
            t += match (h.ptr.is_some(), h.len.is_some()) {
                (_, true) => (if h.shift > 0 { T_LEN_PER_SH } else { T_LEN_PER }) + r.cost,
                (true, false) => T_PTR_PER + r.cost,
                (false, false) => T_PERIOD_ONLY + r.cost,
            };
            self.at(hw, t0, t);
            hw.per(c, self.period(&r, b, h.shift));
            t += T_PER_VOL[c.min(2)];
            self.at(hw, t0, t);
            hw.vol(c, vol);
            wrote = true;
        }
        if let Some(on) = h.dma_on {
            t += T_VOL_DMA;
            self.at(hw, t0, t);
            hw.dmacon(on);
            wrote = true;
        }
        if wrote {
            t + T_NEXT
        } else {
            cur + T_SKIP
        }
    }
}

impl Replayer for Jmf {
    fn format(&self) -> &'static str {
        "Janko Mrsic-Flogel"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.nsub as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.sub = n as usize;
        self.started = false;
        self.songend = false;
    }

    fn tick(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        self.shift = 0;
        self.ti = Some(vbl_start(t0));
        if !self.started {
            self.started = true;
            self.songend = false;
            self.restart(hw, t0, 0, true);
            return;
        }
        if let Some(g) = self.lay.gate {
            if be16(&self.img, g) != 0 {
                return;
            }
        }
        self.step = self.step.wrapping_sub(1);
        if self.step != 0 {
            return;
        }
        self.step = self.lay.step;
        let mut cur = T_BASE + if self.lay.gate.is_some() { T_GATE } else { 0 };
        for c in 0..3 {
            self.dur[c] = self.dur[c].wrapping_sub(1);
            if self.dur[c] != 0 {
                cur += T_IDLE[c];
                continue;
            }
            loop {
                let b = self.rd(self.ptr[c]);
                self.ptr[c] += 1;
                match b {
                    0xFF => {
                        self.songend = true;
                        self.restart(hw, t0, cur + self.lay.r_first, false);
                        return;
                    }
                    0xFE => {
                        self.mode[c] = self.rd(self.ptr[c]);
                        self.ptr[c] += 1;
                        cur += T_FE;
                    }
                    d => {
                        self.dur[c] = d;
                        cur = self.action(hw, t0, c, self.mode[c], cur);
                        break;
                    }
                }
            }
        }
    }
}

/// The frame interrupt of the reference host (a vertical blank every 70824 colour clocks, its
/// handler's acknowledge write at `V_FIRST` + n * `V_PERIOD`, the handler taking `T_VBL` and
/// starting `V_LEAD` before that write) preempts the tick's burst, which the CIA tick (every
/// 70940) drifts through.
const V_FIRST: i64 = 70872;
const V_PERIOD: i64 = 70824;
const V_LEAD: i64 = 40;
const T_VBL: i64 = 66;

/// Where the first frame interrupt starting at or after `t0` starts.
fn vbl_start(t0: u64) -> i64 {
    let t = t0 as i64 + V_LEAD - V_FIRST;
    let n = if t <= 0 { 0 } else { (t + V_PERIOD - 1) / V_PERIOD };
    V_FIRST + n * V_PERIOD - V_LEAD
}


fn new(d: &[u8], base: u32, _m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    let img = image(d)?.to_vec();
    let name = be32(&img, 0x1C) as usize;
    let title: String = img.get(name..).map_or(String::new(), |s| {
        s.iter().take_while(|&&b| b != 0 && b < 0x80).map(|&b| b as char).collect()
    });
    Some(Box::new(Jmf {
        img,
        base: base + 32,
        lay,
        title,
        sub: 0,
        started: false,
        songend: false,
        step: 1,
        dur: [1; 3],
        mode: [0; 3],
        ptr: [0; 3],
        shift: 0,
        ti: None,
    }))
}


#[cfg(test)]
mod tests {
    use super::super::m68kenc as enc;
    use super::*;
    use crate::engine::Engine;
    use std::collections::HashMap;
    use std::path::Path;

    // Image offsets of the synthetic module's variables, tables and streams.
    const GATE: u32 = 0x100;
    const STEP: u32 = 0x102;
    const DUR: u32 = 0x104; // three bytes
    const MODE: u32 = 0x108; // three bytes
    const PTR: u32 = 0x110; // three longs
    const START: u32 = 0x120; // three longs
    const TABLE: u32 = 0xC00;
    const STREAM: [u32; 3] = [0xE00, 0xE20, 0xE40];
    const STREAM1: [u32; 3] = [0xE60, 0xE70, 0xE80];
    const MAIN: u32 = 0xF00;
    const SECOND: u32 = 0xF41;
    const VOICE_D: u32 = 0xF80;

    #[derive(Clone, Copy)]
    struct Opts {
        step: u8,
        subsongs: u32,
        note_sub5: bool,
        gate: bool,
    }

    /// A tiny assembler with label fixups for `bra.w` and `bsr.w`.
    struct Asm {
        b: Vec<u8>,
        labels: HashMap<String, usize>,
        fix: Vec<(usize, String)>,
    }

    impl Asm {
        fn w(&mut self, ws: &[u16]) {
            for w in ws {
                self.b.extend_from_slice(&w.to_be_bytes());
            }
        }
        fn l(&mut self, v: u32) {
            self.b.extend_from_slice(&v.to_be_bytes());
        }
        fn label(&mut self, n: &str) {
            self.labels.insert(n.to_string(), self.b.len());
        }
        /// A `bcc.w` (condition `cond`) to the label `n`.
        fn br(&mut self, cond: u16, n: &str) {
            self.w(&enc::bcc_w(cond, 0));
            self.fix.push((self.b.len() - 2, n.to_string()));
        }
        fn code(&mut self, ws: Vec<u16>) {
            self.w(&ws);
        }
        fn at(&mut self, pos: usize) {
            assert!(self.b.len() <= pos, "{:#x} > {:#x}", self.b.len(), pos);
            self.b.resize(pos, 0);
        }
        fn finish(mut self) -> Vec<u8> {
            for (p, n) in std::mem::take(&mut self.fix) {
                let d = self.labels[&n] as i64 - p as i64;
                self.b[p..p + 2].copy_from_slice(&(d as i16).to_be_bytes());
            }
            self.b
        }
    }

    fn reg(c: usize, off: u32) -> u32 {
        0xDFF0A0 + 16 * c as u32 + off
    }

    /// A synthetic module in the corpus's shapes.
    fn module(o: Opts) -> Vec<u8> {
        use enc::{BEQ, BNE, BRA, BSR, W};
        let mut a = Asm { b: Vec::new(), labels: HashMap::new(), fix: Vec::new() };
        // the signature: moveq #-1,d0 ; rts
        a.code([enc::moveq(0, 0xFF), enc::rts()].concat());
        a.b.extend_from_slice(b"J.FLOGEL");
        a.l(0x300);
        a.l(0x200);
        a.l(o.subsongs);
        a.l(3);
        a.l(0x30);
        a.at(0x30);
        a.b.extend_from_slice(b"Synthetic\0");
        // the init: one start triple per subsong, then the restart block
        a.at(0x200);
        for s in 0..o.subsongs {
            if s == 0 {
                a.code(enc::tst_w_d(0));
                a.code(enc::bne_s(0x22));
            } else if s + 1 < o.subsongs {
                a.code(enc::cmpi_w_d(s as u16, 0));
                a.code(enc::bne_s(0x22));
            }
            let st = if s == 0 { STREAM } else { STREAM1 };
            for (c, v) in st.iter().enumerate() {
                a.code(enc::move_l_imm_abs(*v, START + 4 * c as u32));
            }
            a.br(BRA, "restart");
        }
        a.at(0x260);
        a.label("restart");
        a.code(enc::move_b_imm_abs(0, STEP));
        for c in 0..3 {
            a.code(enc::move_l_imm_abs(MAIN, reg(c, 0)));
            a.code(enc::move_w_imm_abs(0x10, reg(c, 4)));
            a.code(enc::move_w_imm_abs(0x38, reg(c, 8)));
        }
        a.code(enc::move_l_imm_abs(VOICE_D, reg(3, 0)));
        a.code(enc::move_w_imm_abs(0x40, reg(3, 8)));
        a.code(enc::move_w_imm_abs(0xF, 0xDFF096));
        a.code(enc::rts());
        // the play routine
        a.at(0x300);
        if o.gate {
            a.code(enc::tst_abs(W, GATE));
            a.code(enc::bne_s(10));
        }
        a.code(enc::subi_b_abs(1, STEP));
        a.code(enc::beq_s(2));
        a.code(enc::rts());
        a.code(enc::move_b_imm_abs(o.step, STEP));
        for c in 0..3usize {
            let cu = c as u32;
            let next = format!("next{c}");
            a.code(enc::subi_b_abs(1, DUR + cu));
            a.br(BNE, &next);
            a.code(enc::movea_l_abs(1, PTR + 4 * cu));
            a.code(enc::move_b_post_d(1, 0));
            a.code(enc::cmpi_b_d(0xFF, 0));
            a.br(BEQ, "restart");
            a.code(enc::cmpi_b_d(0xFE, 0));
            a.code(enc::bne_s(10));
            a.code(enc::move_b_post_d(1, 0));
            a.code(enc::move_b_d_abs(0, MODE + cu));
            a.code(enc::bra_s(-26));
            a.code(enc::move_b_d_abs(0, DUR + cu));
            a.code(enc::moveq(0, 0));
            a.code(enc::move_b_abs_d(0, MODE + cu));
            // lea table,a2: the table follows the jump
            let lea = enc::lea_abs(2, 0);
            let tail = enc::asl_w_imm(2, 0).len() + enc::adda_l_d(0, 2).len() + enc::jmp_ind(2).len();
            let table = a.b.len() + 2 * (lea.len() + tail);
            a.code(enc::lea_abs(2, table as u32));
            a.code(enc::asl_w_imm(2, 0));
            a.code(enc::adda_l_d(0, 2));
            a.code(enc::jmp_ind(2));
            let modes = ["m0", "m1", "m2", "m3", "m2", "m3", "m3", "m0", "m8"];
            for m in modes {
                a.br(BRA, &format!("{m}_{c}"));
            }
            let store = format!("store_{c}");
            let dma_on = 0x8200 | (1 << c);
            a.label(&store);
            a.code(enc::move_l_a_abs(1, PTR + 4 * cu));
            a.br(BRA, &next);
            // modes 0 and 7: main sample, length 16
            a.label(&format!("m0_{c}"));
            a.code(enc::move_l_imm_abs(MAIN, reg(c, 0xA)));
            a.code(enc::move_w_imm_abs(0x10, reg(c, 4)));
            a.br(BSR, "period");
            a.code(enc::move_w_d_abs(1, reg(c, 6)));
            a.code(enc::move_w_imm_abs(dma_on, 0xDFF096));
            a.br(BRA, &store);
            // mode 1: second sample, length 4, period shifted left by 4
            a.label(&format!("m1_{c}"));
            a.code(enc::move_l_imm_abs(SECOND, reg(c, 0xA)));
            a.code(enc::move_w_imm_abs(4, reg(c, 4)));
            a.br(BSR, "period");
            a.code(enc::asl_w_imm(4, 1));
            a.code(enc::move_w_d_abs(1, reg(c, 6)));
            a.code(enc::move_w_imm_abs(dma_on, 0xDFF096));
            a.br(BRA, &store);
            // modes 2 and 4: an ignored byte
            a.label(&format!("m2_{c}"));
            a.code(enc::move_b_post_d(1, 1));
            a.br(BRA, &store);
            // modes 3, 5 and 6: period only
            a.label(&format!("m3_{c}"));
            a.br(BSR, "period");
            a.code(enc::move_w_d_abs(1, reg(c, 6)));
            a.code(enc::move_w_imm_abs(dma_on, 0xDFF096));
            a.br(BRA, &store);
            // mode 8: this voice's DMA off
            a.label(&format!("m8_{c}"));
            a.code(enc::move_w_imm_abs(1 << c, 0xDFF096));
            a.br(BRA, &store);
            a.label(&next);
        }
        a.code(enc::rts());
        // the period routine
        a.label("period");
        a.code(enc::moveq(1, 0));
        a.code(enc::move_b_post_d(1, 1));
        if o.note_sub5 {
            a.code(enc::subi_b_d(5, 1));
        }
        a.code(enc::add_w_dd(1, 1));
        a.code(enc::lea_abs(2, TABLE));
        a.code(enc::move_w_idx_d(2, 1, 1));
        a.code(enc::rts());
        // the period table: entry n is 100 + n, 256 entries
        a.at(TABLE as usize);
        for n in 0..256u16 {
            a.w(&[100 + n]);
        }
        // the streams of subsong 0
        a.at(STREAM[0] as usize);
        a.b.extend_from_slice(&[0xFE, 0, 2, 10, 0xFE, 1, 1, 7, 0xFE, 8, 1, 0xFF]);
        a.at(STREAM[1] as usize);
        a.b.extend_from_slice(&[0xFE, 3, 3, 5, 0xFF]);
        a.at(STREAM[2] as usize);
        a.b.extend_from_slice(&[0xFE, 2, 4, 99, 0xFF]);
        // subsong 1: one period-only note on A, then the end
        a.at(STREAM1[0] as usize);
        a.b.extend_from_slice(&[0xFE, 3, 1, 20, 0xFF]);
        a.at(STREAM1[1] as usize);
        a.b.push(0xFF);
        a.at(STREAM1[2] as usize);
        a.b.push(0xFF);
        // sample data
        a.at(MAIN as usize);
        for n in 0..0x80u8 {
            a.b.push(n.wrapping_mul(7));
        }
        a.at(0x1000);
        let mut img = a.finish();
        // the file: hunk head, the code, the hunk end
        let words = (img.len() / 4) as u32;
        let mut f = Vec::new();
        for v in [0x3F3u32, 0, 1, 0, 0, 0x4000_0000 | words, 0x3E9, words] {
            f.extend_from_slice(&v.to_be_bytes());
        }
        f.append(&mut img);
        f.extend_from_slice(&0x3F2u32.to_be_bytes());
        f
    }

    const BASE: Opts = Opts { step: 2, subsongs: 1, note_sub5: false, gate: true };

    fn play(d: Vec<u8>, ticks: usize, sub: u32) -> (Engine, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("x.jmf"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Janko Mrsic-Flogel");
        for _ in 0..sub {
            e.change_subsong(1).unwrap();
        }
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.clone().unwrap().iter().map(|w| (w.0, w.1)).collect();
        (e, log)
    }

    fn of(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_the_head() {
        let d = module(BASE);
        assert!(detect(&d));
        for (o, v) in [(3usize, 0xF2u8), (20, 0), (32, 0x71), (37, b'x'), (46, 0), (50, 0), (55, 0)] {
            let mut e = d.clone();
            e[o] = v;
            assert!(!detect(&e), "{o}");
        }
        assert!(!detect(&d[..40]));
    }

    #[test]
    fn reads_the_constants_from_the_code() {
        let l = layout(&module(BASE)).unwrap();
        assert_eq!((l.gate, l.step, l.nsub), (Some(GATE as usize), 2, 1));
        assert_eq!(l.starts, vec![[STREAM[0], STREAM[1], STREAM[2]]]);
        let h = l.chans[0][0];
        assert_eq!((h.ptr, h.len, h.shift, h.dma_on), (Some(MAIN), Some(16), 0, Some(0x8201)));
        let r = h.period.unwrap();
        assert_eq!((r.base, r.add, r.mask), (TABLE as usize, 0, 0xFFFF));
        let h = l.chans[1][1];
        assert_eq!((h.ptr, h.len, h.shift, h.dma_on), (Some(SECOND), Some(4), 4, Some(0x8202)));
        assert!(l.chans[2][2].skip && l.chans[2][4].skip);
        assert!(l.chans[1][3].ptr.is_none() && l.chans[1][3].period.is_some());
        assert_eq!(l.chans[0][7], l.chans[0][0]);
        assert_eq!(l.chans[0][8].dma_off, Some(1));
        assert_eq!(l.chans[2][8].dma_off, Some(4));
        // the restart block, in order
        assert_eq!(l.restart.len(), 12);
        assert_eq!(l.restart[0], Rw::Ptr(0, MAIN));
        assert_eq!(l.restart[9], Rw::Ptr(3, VOICE_D));
        assert_eq!(l.restart[11], Rw::Word(0xDFF096, 0x000F));
        // no gate: the play routine starts with the step counter
        let l = layout(&module(Opts { gate: false, ..BASE })).unwrap();
        assert_eq!((l.gate, l.step), (None, 2));
        // the note offset of a byte subtraction
        let l = layout(&module(Opts { note_sub5: true, ..BASE })).unwrap();
        let r = l.chans[0][0].period.unwrap();
        assert_eq!((r.add, r.mask), (-5, 0xFF));
        // two subsongs, two triples
        let l = layout(&module(Opts { subsongs: 2, ..BASE })).unwrap();
        assert_eq!(l.starts.len(), 2);
        assert_eq!(l.starts[1], [STREAM1[0], STREAM1[1], STREAM1[2]]);
        // a file with a different head is refused
        assert!(layout(&module(BASE)[..64]).is_none());
    }

    #[test]
    fn plays_the_streams_in_the_documented_order() {
        let (e, log) = play(module(BASE), 9, 0);
        assert!(e.songend());
        // the init's block, the notes (a step every 2 ticks), the DMA off, the restart's block
        let dma = of(&log, 0xDFF096);
        assert_eq!(dma, [0x000F, 0x8201, 0x8202, 0x8201, 0x0001, 0x000F]);
        // voice A: note 10 as is, then note 7 with the period shifted left by 4
        assert_eq!(of(&log, 0xDFF0A6), [110, 107u16 << 4]);
        // voice B: period only, note 5; voice C wrote nothing (its ignored byte)
        assert_eq!(of(&log, 0xDFF0B6), [105]);
        assert!(of(&log, 0xDFF0C6).is_empty());
        // the length writes: init, main sample, second sample, restart
        assert_eq!(of(&log, 0xDFF0A4), [16, 16, 4, 16]);
        // the host's volume 56 after each period; the blocks' constants raw
        assert_eq!(of(&log, 0xDFF0A8), [56, 56, 56, 56]);
        assert_eq!(of(&log, 0xDFF0D8), [64, 64]);
        // the second sample's odd address is kept in the register
        let lc = of(&log, 0xDFF0A2);
        assert_eq!(lc[2] - lc[0], (SECOND - MAIN) as u16);
        assert_eq!(lc[1], lc[0]);
    }

    #[test]
    fn the_note_offset_wraps_at_its_width() {
        let mut d = module(Opts { note_sub5: true, ..BASE });
        // note 3 on voice A: entry 254
        d[32 + STREAM[0] as usize + 3] = 3;
        let (_, log) = play(d, 3, 0);
        assert_eq!(of(&log, 0xDFF0A6)[0], 100 + 254);
        let (_, log) = play(module(Opts { note_sub5: true, ..BASE }), 3, 0);
        assert_eq!(of(&log, 0xDFF0A6)[0], 100 + 5);
    }

    #[test]
    fn a_set_gate_word_silences_the_replayer() {
        let mut d = module(BASE);
        d[32 + GATE as usize + 1] = 1;
        let (_, log) = play(d, 6, 0);
        assert_eq!(of(&log, 0xDFF096), [0x000F]);
    }

    #[test]
    fn subsong_one_uses_its_own_streams() {
        let (e, log) = play(module(Opts { subsongs: 2, ..BASE }), 4, 1);
        assert_eq!(e.playing.as_ref().unwrap().sub, 1);
        // note 20 on voice A, mode 3 (period only)
        assert!(of(&log, 0xDFF0A6).contains(&120));
    }

    #[test]
    fn the_frame_interrupt_starts_before_its_acknowledge() {
        assert_eq!(vbl_start(0), V_FIRST - V_LEAD);
        assert_eq!(vbl_start((V_FIRST - V_LEAD) as u64), V_FIRST - V_LEAD);
        assert_eq!(vbl_start((V_FIRST - V_LEAD + 1) as u64), V_FIRST - V_LEAD + V_PERIOD);
    }
}
