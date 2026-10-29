//! Sound Images (`tw.*`), Tony Williams' music driver for the Amiga games of
//! 1989-92 (Shinobi, Chuckie Egg II, Judge Dredd, Never Mind, Nitro, Narc, Smash TV,
//! International Ice Hockey, Super Monaco GP, Teenage Mutant Hero Turtles, Viz,
//! Future Basketball, MoonFall, Sharkey's Moll).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the driver's observable
//! behaviour (the Paula register writes of UADE's SoundImages player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A `tw` file is a flat 68k program image: the game's replay driver, its global
//! and per-voice state blocks, its tables, the songs and the samples. The code is
//! not run. Its data is found by structure (the targets of the PC-relative operands
//! of a few generic instructions, the 68000 encodings of which the specification
//! names), the driver revision (nine in the corpus) is told by a fingerprint, and
//! the engine is written here from its behaviour: four voices (three in the oldest
//! revision), each an order list of note streams; a stream is bytes, a note below
//! 0x80, ranges of commands above (step length, sample, envelope, start volume,
//! arpeggio) and a jump table of commands from 0x80 on. The state blocks keep the
//! layout of the file (pointer fields absolute, initial values from the file), as
//! the driver's reads of image bytes past its tables depend on them.
//!
//! Decisions taken from the black-box runs where the specification leaves a
//! choice:
//! - the audio interrupt (the loop pointer) is taken at the DMA start, as Paula
//!   raises it; UADE loses a race in some ticks and takes it a buffer later;
//! - Super Monaco GP in-game: the pending DMA and interrupt words left in the file
//!   crash UADE's run after one tick; they are cleared at load and the music plays
//!   (as UADE plays the file with them zeroed);
//! - Never Mind (R4): the volumes are UADE's, x 4, and the periods are written
//!   after a stop in the same tick;
//! - the two variable entries of the common jump table are told by their first
//!   instruction relative to their neighbours' (slide-down's instruction: slide-up;
//!   tie-off's operand: tie-on; else volume-down);
//! - commands past a revision's map (none in the corpus) are skipped.
//!
//! Recognition does not overlap the Dave Lowe New layout-A files, which the
//! Eagleplayer "Sound Images" player also takes: they begin with a header word,
//! not a branch, and `davelowe.rs` plays them.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Sound Images", detect, new };

/// The driver revisions (specification section 4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rev {
    /// Shinobi: three voices, no volume table.
    R1,
    /// Chuckie Egg II: linear envelopes, a jump table for everything below 0xE0.
    R2,
    /// Judge Dredd.
    R3,
    /// Never Mind: volumes first, periods after.
    R4,
    /// Future Basketball in-game: other global offsets.
    R5,
    /// Teenage Mutant Hero Turtles: 0x46-byte voice blocks, generation B gating.
    R6,
    /// The reference engine.
    A,
    /// Generation A with the command ranges shifted (Viz building site).
    A9c,
    /// Generation B: voices gated by an active byte.
    B,
}

/// The commands of the jump tables (specification section 8 and 10).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmd {
    NextPat,
    SlideDn,
    SlideUp,
    JumpList,
    Stop,
    Release,
    Rest,
    Rest0,
    Transp,
    GTransp,
    Vib,
    VibOff,
    Port,
    PortOff,
    Slide1,
    Slide5,
    Slide5Off,
    TieOn,
    TieOff,
    ArpOff,
    VolDn,
    VolUp,
    Fade,
    SfxEnd,
    /// R1: six envelope bytes.
    Env6,
    /// R2: seven envelope bytes.
    Env7,
    /// R2: the sample record of the operand.
    Sample,
    /// Start volume := operand.
    StartVol,
    /// R2: modulation set-up (three operands, no audible state).
    ModSet,
    ModOff,
    /// R2: the pulse-buffer builder.
    Builder,
    /// R2: start volume -= 1 / += 1, wrapping.
    VolDnWrap,
    VolUpWrap,
    /// R2: fade with rate and counter 1.
    Fade1,
}

use Cmd::*;

/// The maps of the fixed revisions, from 0x80 on.
const MAP_R1: &[Cmd] = &[
    NextPat, SlideDn, JumpList, Stop, Env6, Release, Rest, Rest0, GTransp, Vib, Port, PortOff, StartVol, Slide1, TieOn,
    TieOff, ArpOff,
];
const MAP_R2: &[Cmd] = &[
    NextPat, SlideUp, SlideDn, JumpList, Stop, Sample, Env7, Rest, Rest0, GTransp, Vib, Port, PortOff, StartVol, Fade1,
    SfxEnd, Slide1, TieOn, TieOff, ModSet, ModOff, Builder, VolDnWrap, VolUpWrap,
];
const MAP_R3: &[Cmd] = &[
    NextPat, SlideDn, JumpList, Stop, Release, Rest, Rest0, GTransp, Vib, Port, PortOff, StartVol, Slide1, TieOn, TieOff,
    ArpOff, VolDn, VolUp,
];
const MAP_R4: &[Cmd] = &[
    NextPat, SlideDn, JumpList, Stop, Release, Rest, Rest0, GTransp, Vib, Port, PortOff, StartVol, Slide1, TieOn, TieOff,
    ArpOff, SfxEnd, VolDn, VolUp,
];
const MAP_R5: &[Cmd] = &[
    NextPat, SlideDn, SlideUp, JumpList, Stop, Release, Rest, Rest0, Transp, Vib, VibOff, Port, PortOff, Slide1, Slide5,
    Slide5Off, TieOn, TieOff, ArpOff, VolDn, VolUp, Fade,
];
/// Generations A and B, A9C and R6; entries 0x8E and 0x91 vary by file.
const MAP_STD: &[Cmd] = &[
    NextPat, SlideDn, JumpList, Stop, Release, Rest, Rest0, Transp, Vib, Port, PortOff, Slide1, Slide5, Slide5Off, TieOn,
    TieOff, ArpOff, VolDn, VolUp, Fade, SfxEnd,
];

/// Loops over streams or order lists that never reach a note give up after this many steps.
const GUARD: u32 = 2048;

// ---------------------------------------------------------------- recognition

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The target of a brief-extension indexed operand whose extension word is at `x`.
fn d8(d: &[u8], x: usize) -> usize {
    (x as i64 + rb(d, x + 1) as i8 as i64).max(0) as usize
}

/// The first even offset in `from..to` where the words `pat` (None: any) sit.
fn scan(d: &[u8], from: usize, to: usize, pat: &[Option<u16>]) -> Option<usize> {
    let to = to.min(d.len().saturating_sub(2 * pat.len()));
    (from..to).step_by(2).find(|&o| pat.iter().enumerate().all(|(i, p)| p.is_none_or(|p| be16(d, o + 2 * i) == p)))
}

/// The voice-0 pitch hook of some generation-B play routines (section 9.4).
#[derive(Clone, Debug, Default, PartialEq)]
struct Hook {
    /// Songs for which the store is skipped (0: no song).
    skip: Vec<u8>,
    /// The only song the store applies to.
    only: Option<u8>,
}

impl Hook {
    fn applies(&self, song: u8) -> bool {
        !self.skip.contains(&song) && self.only.is_none_or(|n| n == song)
    }
}

#[derive(Clone, Debug)]
struct Layout {
    rev: Rev,
    /// File offsets of the voice blocks.
    blocks: Vec<usize>,
    /// The song base and the song-entry stride.
    sb: usize,
    stride: usize,
    nsub: usize,
    samples: usize,
    arp: usize,
    periods: usize,
    voltab: Option<usize>,
    /// The commands from 0x80 on.
    cmds: Vec<Cmd>,
    /// The immediate of the fade command.
    fade: u16,
    /// UADE's "music" flag (G+8 := 1 at start-up).
    music: bool,
    hook: Option<Hook>,
    /// The frame divider's counter (file offset) and mask.
    divider: Option<(usize, u16)>,
}

const G: usize = 4;

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 0x800 || be16(d, 0) != 0x6000 {
        return None;
    }
    let v = be16(d, 2) as usize;
    if !v.is_multiple_of(2) || !(0x60..=0xC0).contains(&v) {
        return None;
    }
    let p = v + 2;
    // movem.l d0-d3/a0-a2/a5/a6,-(sp), then lea $DFF000,a6 within the next words
    if be16(d, p) != 0x48E7 || be16(d, p + 2) != 0xF0E6 {
        return None;
    }
    if !(p + 4..p + 0x10).step_by(2).any(|o| be16(d, o) == 0x4DF9 && be32(d, o + 2) == 0xDFF000) {
        return None;
    }
    // the channel blocks: lea d16(pc),a0 targets that hold a Paula register block
    let mut blocks: [Option<usize>; 4] = [None; 4];
    for o in (p..p + 0x100).step_by(2) {
        if be16(d, o) != 0x41FA {
            continue;
        }
        let t = o as i64 + 2 + be16(d, o + 2) as i16 as i64;
        if t < 0 || t as usize + 0x48 > d.len() {
            continue;
        }
        let t = t as usize;
        for (k, b) in blocks.iter_mut().enumerate() {
            if b.is_none() && be32(d, t + 0x0A) == 0xDFF0A0 + 0x10 * k as u32 {
                *b = Some(t);
            }
        }
    }
    let nb = blocks.iter().take_while(|b| b.is_some()).count();
    if nb < 3 || blocks[nb..].iter().any(|b| b.is_some()) {
        return None;
    }
    let blocks: Vec<usize> = blocks[..nb].iter().map(|b| b.unwrap()).collect();
    let r6_layout = blocks.iter().enumerate().all(|(k, &b)| be16(d, b + 0x20) == 0x8200 | 1 << k);
    let std_layout = blocks.iter().enumerate().all(|(k, &b)| be16(d, b + 0x22) == 0x8200 | 1 << k);
    if !r6_layout && !std_layout {
        return None;
    }
    // the song base: the first lea (d8,pc,d0.w),a1
    let lea = scan(d, p, d.len(), &[Some(0x43FB)])?;
    let sb = d8(d, lea + 2);
    let stride = if be16(d, lea - 2) == 10 { 10 } else { 8 };
    let limit = (p / 2..lea / 2).rev().map(|i| 2 * i).find(|&o| be16(d, o) == 0xB03C).map(|o| rb(d, o + 3))?;
    let nv = nb;
    let mut smallest = usize::MAX;
    for i in 0..limit as usize {
        for c in 0..nv {
            let w = be16(d, sb + stride * i + 2 * c) as usize;
            if w != 0 {
                smallest = smallest.min(w);
            }
        }
    }
    let nsub = if smallest == usize::MAX { 0 } else { smallest / stride };
    if sb >= d.len() || nsub == 0 {
        return None;
    }
    // the sample table: lea (d8,pc,d0.w),a2 after mulu.w #12,d0
    let samples = d8(d, scan(d, p, d.len(), &[Some(0xC0FC), Some(0x000C), Some(0x45FB)])? + 6);
    // the arpeggio table: move.w (d8,pc,d0.w),d0 then lea (d8,pc,d0.w),a2 with the same target
    let mut arp = 0;
    let mut o = p;
    while let Some(a) = scan(d, o, d.len(), &[Some(0x303B), None, Some(0x45FB)]) {
        if d8(d, a + 2) == d8(d, a + 6) {
            arp = d8(d, a + 2);
            break;
        }
        o = a + 2;
    }
    let periods = d8(d, scan(d, p, d.len(), &[Some(0x317B), None, Some(0x0006)])? + 2);
    // the volume table and the master-volume field
    let mut voltab = None;
    let mut master = 6;
    for m in [6u16, 4] {
        if let Some(o) = scan(d, p, d.len(), &[Some(0xC0ED), Some(m), Some(0xE848), Some(0xD040)]) {
            if be16(d, o + 8) & 0x3F == 0x3B {
                voltab = Some(d8(d, o + 10));
                master = m;
            }
            break;
        }
    }
    // the command jump table and the dispatcher's range bounds
    let jt = scan(d, p, d.len(), &[Some(0x0240), Some(0x007F), Some(0xD040), Some(0x303B)])?;
    let table = d8(d, jt + 8);
    let entries = (p / 2..jt / 2).rev().map(|i| 2 * i).find(|&o| be16(d, o) == 0xB03C).map(|o| rb(d, o + 3))?;
    let entries = (entries as usize).checked_sub(0x80)?;
    let bounds: Vec<u8> = (jt.saturating_sub(0x40)..jt).step_by(2).filter(|&o| be16(d, o) == 0xB03C).map(|o| rb(d, o + 3)).collect();
    let handler = |i: usize| (table as i64 + be16(d, table + 2 * i) as i16 as i64).max(0) as usize;
    let area = |w: u16| scan(d, p, p + 0x200, &[Some(0x4A28), Some(w)]).is_some();
    let rev = if nb == 3 {
        Rev::R1
    } else if voltab.is_none() && !bounds.contains(&0xC0) {
        Rev::R2
    } else if voltab.is_none() {
        Rev::R4
    } else if master == 4 {
        Rev::R5
    } else if area(0x45) {
        Rev::R6
    } else if area(0x47) {
        Rev::B
    } else if bounds.contains(&0xD0) {
        Rev::A
    } else if bounds.contains(&0x9C) {
        Rev::A9c
    } else {
        Rev::R3
    };
    if (rev == Rev::R6) != r6_layout {
        return None;
    }
    let mut cmds: Vec<Cmd> = match rev {
        Rev::R1 => MAP_R1.to_vec(),
        Rev::R2 => MAP_R2.to_vec(),
        Rev::R3 => MAP_R3.to_vec(),
        Rev::R4 => MAP_R4.to_vec(),
        Rev::R5 => MAP_R5.to_vec(),
        _ => MAP_STD.to_vec(),
    };
    cmds.truncate(entries);
    let mut fade = 0;
    if matches!(rev, Rev::A | Rev::A9c | Rev::B | Rev::R6) {
        if entries < MAP_STD.len() {
            return None;
        }
        // entries 0x8E and 0x91 are tieon, voldn or slideup: a slideup handler begins
        // with the instruction of slidedn's (another immediate), a tieon handler with
        // tieoff's operation on the same bit (the second word the same)
        let first = |i: usize| (be16(d, handler(i)), be16(d, handler(i) + 2));
        let (dn, toff) = (first(1), first(0x0F));
        for i in [0x0E, 0x11] {
            let h = first(i);
            cmds[i] = if h.0 == dn.0 && h.1 != dn.1 {
                SlideUp
            } else if h.1 == toff.1 && h.0 != toff.0 {
                TieOn
            } else {
                VolDn
            };
        }
        fade = be16(d, handler(0x13) + 2);
    } else if rev == Rev::R5 {
        fade = be16(d, handler(0x15) + 2);
    }
    // the host's music flag: a write of d0 to AUD3VOL before the first bsr.w
    let first_bsr = scan(d, p, d.len(), &[Some(0x6100)]).unwrap_or(p);
    let music = scan(d, p, first_bsr, &[Some(0x3D40), Some(0x00D8)]).is_some();
    // the voice-0 pitch hook: tests of the running song, then the store of G+8
    let mut hook = None;
    if rev == Rev::B {
        // the first DMACON write: move.w d16(a5),$96(a6)
        let dma = scan(d, p, p + 0x80, &[Some(0x3D6D), None, Some(0x0096)]).unwrap_or(p + 0x80);
        if let Some(st) = scan(d, p, dma, &[Some(0x316D), Some(0x0008), Some(0x0006)]) {
            let mut h = Hook::default();
            let mut o = p;
            while o < st {
                if be16(d, o) == 0x4A2D && be16(d, o + 2) == 0x0001 && rb(d, o + 4) == 0x67 {
                    h.skip.push(0);
                    o += 6;
                } else if be16(d, o) == 0x0C2D && be16(d, o + 4) == 0x0001 && matches!(rb(d, o + 6), 0x66 | 0x67) {
                    let n = rb(d, o + 3);
                    if rb(d, o + 6) == 0x67 {
                        h.skip.push(n);
                    } else {
                        h.only = Some(n);
                    }
                    o += 8;
                } else {
                    o += 2;
                }
            }
            hook = Some(h);
        }
    }
    // the frame divider: lea counter(pc),a0 ; addq.w #1,(a0) ; andi.w #mask,(a0) ; beq
    let mut divider = None;
    if let Some(o) = scan(d, p + 0x0E, p + 0x1A, &[Some(0x41FA), None, Some(0x5250), Some(0x0250)])
        && rb(d, o + 10) == 0x67
    {
        let t = o as i64 + 2 + be16(d, o + 2) as i16 as i64;
        if t > 0 && (t as usize) + 2 <= d.len() {
            divider = Some((t as usize, be16(d, o + 8)));
        }
    }
    Some(Layout { rev, blocks, sb, stride, nsub, samples, arp, periods, voltab, cmds, fade, music, hook, divider })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

fn new(d: &[u8], base: u32, _m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    Some(Box::new(SoundImages::build(d.to_vec(), base, lay)))
}

// ---------------------------------------------------------------- the engine

/// Global-block offsets of one revision.
#[derive(Clone, Copy, Debug)]
struct Globals {
    run: usize,
    fade: usize,
    master: usize,
    dma: usize,
    int: usize,
    tempo: usize,
    latch: usize,
    gtransp: Option<usize>,
}

// voice flags (+0x16)
const F_ARP: u8 = 1;
const F_SLIDE: u8 = 2;
const F_PORT: u8 = 4;
const F_VIBDIR: u8 = 8;
const F_TIE: u8 = 0x10;
const F_SLIDE5: u8 = 0x20;
const F_PULSE_DOWN: u8 = 0x40;

pub struct SoundImages {
    orig: Vec<u8>,
    /// The driver's memory: the file with its state blocks as they evolve.
    img: Vec<u8>,
    base: u32,
    lay: Layout,
    g: Globals,
    rest: u8,
    sub: u16,
    started: bool,
    marked: Vec<bool>,
    songend: bool,
    /// Guest bytes the pulse generators wrote (restored at a restart).
    touched: Vec<u32>,
    /// The song started has voices with a header.
    had_marks: bool,
}

impl SoundImages {
    fn build(img: Vec<u8>, base: u32, lay: Layout) -> SoundImages {
        let g = match lay.rev {
            Rev::R5 => Globals { run: 1, fade: 2, master: 4, dma: 0xA, int: 0xC, tempo: 0x10, latch: 0x12, gtransp: None },
            Rev::R4 => Globals { run: 1, fade: 4, master: 6, dma: 0xE, int: 0x10, tempo: 0x14, latch: 0x16, gtransp: Some(0x17) },
            Rev::R2 => Globals { run: 2, fade: 4, master: 6, dma: 0xC, int: 0xE, tempo: 0x12, latch: 0x14, gtransp: Some(0x15) },
            Rev::R1 | Rev::R3 => {
                Globals { run: 1, fade: 4, master: 6, dma: 0xC, int: 0xE, tempo: 0x12, latch: 0x14, gtransp: Some(0x15) }
            }
            _ => Globals { run: 1, fade: 4, master: 6, dma: 0xC, int: 0xE, tempo: 0x12, latch: 0x14, gtransp: None },
        };
        let rest = 0x80 + lay.cmds.iter().position(|&c| c == Rest).unwrap_or(0x7F) as u8;
        let n = lay.blocks.len();
        SoundImages {
            orig: img.clone(),
            img,
            base,
            lay,
            g,
            rest,
            sub: 0,
            started: false,
            marked: vec![false; n],
            songend: false,
            touched: Vec::new(),
            had_marks: false,
        }
    }

    // memory of the image, by file offset
    fn b(&self, o: usize) -> u8 {
        rb(&self.img, o)
    }
    fn w(&self, o: usize) -> u16 {
        be16(&self.img, o)
    }
    fn l(&self, o: usize) -> u32 {
        be32(&self.img, o)
    }
    fn sb_(&mut self, o: usize, v: u8) {
        if let Some(x) = self.img.get_mut(o) {
            *x = v;
        }
    }
    fn sw(&mut self, o: usize, v: u16) {
        self.sb_(o, (v >> 8) as u8);
        self.sb_(o + 1, v as u8);
    }
    fn sl(&mut self, o: usize, v: u32) {
        self.sw(o, (v >> 16) as u16);
        self.sw(o + 2, v as u16);
    }
    /// The file offset of an absolute address.
    fn off(&self, a: u32) -> usize {
        a.wrapping_sub(self.base) as usize
    }
    fn abs(&self, o: usize) -> u32 {
        self.base.wrapping_add(o as u32)
    }

    // the global block
    fn gb(&self, f: usize) -> u8 {
        self.b(G + f)
    }
    fn sgb(&mut self, f: usize, v: u8) {
        self.sb_(G + f, v);
    }
    fn gw(&self, f: usize) -> u16 {
        self.w(G + f)
    }
    fn sgw(&mut self, f: usize, v: u16) {
        self.sw(G + f, v);
    }

    /// A voice-block field offset (R6's fields from 0x20 on sit two bytes lower).
    fn fo(&self, k: usize, f: usize) -> usize {
        self.lay.blocks[k] + if self.lay.rev == Rev::R6 && f >= 0x20 { f - 2 } else { f }
    }
    fn vb(&self, k: usize, f: usize) -> u8 {
        self.b(self.fo(k, f))
    }
    fn svb(&mut self, k: usize, f: usize, v: u8) {
        let o = self.fo(k, f);
        self.sb_(o, v);
    }
    fn vw(&self, k: usize, f: usize) -> u16 {
        self.w(self.fo(k, f))
    }
    fn svw(&mut self, k: usize, f: usize, v: u16) {
        let o = self.fo(k, f);
        self.sw(o, v);
    }
    fn vl(&self, k: usize, f: usize) -> u32 {
        self.l(self.fo(k, f))
    }
    fn svl(&mut self, k: usize, f: usize, v: u32) {
        let o = self.fo(k, f);
        self.sl(o, v);
    }
    fn flags(&self, k: usize) -> u8 {
        self.vb(k, 0x16)
    }
    fn set_flags(&mut self, k: usize, f: u8) {
        self.svb(k, 0x16, f);
    }

    fn nv(&self) -> usize {
        self.lay.blocks.len()
    }
    fn rev(&self) -> Rev {
        self.lay.rev
    }
    fn gen_b(&self) -> bool {
        matches!(self.rev(), Rev::B | Rev::R6)
    }
    fn old(&self) -> bool {
        matches!(self.rev(), Rev::R1 | Rev::R2 | Rev::R4)
    }
    fn master_start(&self) -> u16 {
        if self.old() { 0x40 } else { 0x10 }
    }
    /// The transposition byte of a voice: global in R1-R4.
    fn transp_at(&self, k: usize) -> usize {
        match self.g.gtransp {
            Some(o) => G + o,
            None => self.fo(k, 0x46),
        }
    }
    /// The voice's active byte (generation B and R6) or running byte (R4).
    fn active_at(&self, k: usize) -> usize {
        if self.rev() == Rev::R4 { self.fo(k, 0x46) } else { self.fo(k, 0x47) }
    }
    fn pulse_buffer(&self) -> u32 {
        self.abs(G).wrapping_add(self.l(self.lay.samples))
    }
    fn pulse_index_at(&self) -> usize {
        if self.rev() == Rev::R4 { G + 0x12 } else { G + 0x10 }
    }

    /// UADE's start-up before the first call (section 5).
    fn startup(&mut self, hw: &mut Hw) {
        self.img.clone_from(&self.orig);
        for a in std::mem::take(&mut self.touched) {
            let o = self.off(a);
            hw.bus.w8(a, rb(&self.orig, o));
        }
        self.sgb(0, self.sub as u8 + 1);
        for f in 1..8 {
            self.sgb(f, 0);
        }
        if self.lay.music {
            self.sgb(8, 1);
        }
        // SMGP in-game: interrupts left pending in the file; play the music instead
        // of UADE's one-tick crash (section 9.3)
        if self.rev() == Rev::B && self.gw(self.g.int) & 0x0780 != 0 {
            self.sgw(self.g.dma, 0);
            self.sgw(self.g.int, 0);
        }
        self.marked = vec![false; self.nv()];
        self.had_marks = false;
        self.songend = false;
        self.started = true;
    }

    fn running(&self) -> bool {
        self.gb(self.g.run) != 0
    }

    fn volume_out(&self, k: usize) -> u16 {
        let v = self.vw(k, 0x08) as u32;
        let m = self.gw(self.g.master) as u32;
        let x = (v * m) & 0xFFFF;
        match self.lay.voltab {
            Some(t) => self.w(t + 2 * (x >> 4) as usize) & 0x7F,
            None if self.rev() == Rev::R4 && self.gb(8) != 0 => ((x >> 6) * 4) as u16,
            None => (x >> 6) as u16,
        }
    }

    /// The step-1 work: the pending DMACON and the audio interrupt.
    fn pending(&mut self, hw: &mut Hw, takes_start: bool) {
        let dma = self.gw(self.g.dma);
        let e = self.gw(self.g.int);
        hw.dmacon(dma);
        self.sgw(self.g.dma, 0);
        self.sgw(self.g.int, 0);
        // a song start in this tick turns the interrupts off before they are taken
        if e & 0x0780 == 0 || (takes_start && self.rev() != Rev::B) {
            return;
        }
        hw.wait(300);
        let mut done = 0;
        for k in 0..self.nv() {
            let bit = 0x80u16 << k;
            if e & bit == 0 {
                continue;
            }
            // R2's handler tests the wrong register for channel D
            if self.rev() == Rev::R2 && k == 3 {
                continue;
            }
            let mut blk = k;
            if self.rev() == Rev::R4 && k == 3 {
                let a = self.l(G + 0x0A);
                if a == 0 {
                    continue;
                }
                match self.lay.blocks.iter().position(|&b| self.abs(b) == a) {
                    Some(i) => blk = i,
                    None => continue,
                }
            }
            hw.loc(k, self.vl(blk, 0x1A));
            hw.len(k, self.vw(blk, 0x20));
            done |= bit;
        }
        if done != 0 {
            // the handler's acknowledge (also what tells its writes apart in the log)
            hw.bus.write_word(0xDFF09A, done);
            hw.bus.write_word(0xDFF09C, done);
        }
        hw.wait(100);
    }

    /// Whether this tick's request step copies the request byte into the latch.
    fn takes_latch(&self) -> bool {
        match self.rev() {
            Rev::A | Rev::A9c | Rev::R3 | Rev::R2 => true,
            Rev::R6 => self.gw(8) != 0 && !self.running(),
            Rev::R4 => self.gb(8) != 0 && !self.running(),
            _ => !self.running(),
        }
    }

    /// What the latch will hold in this tick's request step.
    fn next_latch(&self) -> u8 {
        if self.takes_latch() { self.gb(0) } else { self.gb(self.g.latch) }
    }

    fn take_latch(&mut self) {
        if self.takes_latch() {
            let r = self.gb(0);
            self.sgb(self.g.latch, r);
        }
    }

    fn request(&mut self, hw: &mut Hw) {
        let latch = self.gb(self.g.latch);
        if latch == 0 {
            return;
        }
        if latch as usize > self.lay.nsub {
            self.stop(hw);
            return;
        }
        self.start_song(latch, hw);
    }

    fn start_song(&mut self, n: u8, hw: &mut Hw) {
        let g = self.g;
        self.sgb(g.run, n);
        if self.rev() != Rev::B {
            hw.dmacon(0x000F);
        }
        self.sgb(g.tempo + 1, 1);
        self.sgb(g.fade, 0);
        self.sgb(0, 0);
        self.sgb(g.latch, 0);
        if let Some(t) = g.gtransp {
            self.sgb(t, 0);
        }
        let e = self.lay.sb + self.lay.stride * (n as usize - 1);
        if self.rev() == Rev::R6 {
            for k in 0..self.nv() {
                let a = self.active_at(k);
                self.sb_(a, 0);
            }
        }
        for k in 0..self.nv() {
            let hw_ = self.w(e + 2 * k) as usize;
            self.marked[k] = hw_ != 0;
            if hw_ == 0 && self.gen_b() {
                continue;
            }
            let h = self.lay.sb + hw_;
            let order = self.abs(h + 2);
            let stream = self.abs(self.lay.sb + self.w(h) as usize);
            self.svl(k, 0x0E, order);
            self.svl(k, 0x12, stream);
            self.svb(k, 0x16, 0);
            self.svb(k, 0x2E, 0);
            if g.gtransp.is_none() {
                self.svb(k, 0x46, 0);
            }
            self.svb(k, 0x17, 1);
            if self.gen_b() || self.rev() == Rev::R4 {
                let a = self.active_at(k);
                self.sb_(a, 1);
            }
        }
        self.had_marks = self.marked.iter().any(|&m| m);
        let tempo = self.b(e + 2 * self.nv());
        self.sgb(g.tempo, tempo);
        let m = self.master_start();
        self.sgw(g.master, m);
        if self.rev() == Rev::B {
            self.sgw(8, 0x358);
        }
    }

    /// The full stop (section 7.5).
    fn stop(&mut self, hw: &mut Hw) {
        let g = self.g;
        hw.dmacon(0x000F);
        if self.rev() == Rev::R5 {
            for c in 0..4 {
                hw.vol(c, 0);
            }
        }
        let n = if self.rev() == Rev::R2 { 8 } else { 4 };
        for f in 0..n {
            self.sgb(f, 0);
        }
        self.sgb(g.latch, 0);
        self.sgw(g.fade, 0);
        let m = self.master_start();
        self.sgw(g.master, m);
        if self.gen_b() || self.rev() == Rev::R4 {
            for k in 0..self.nv() {
                let a = self.active_at(k);
                self.sb_(a, 0);
            }
        }
    }

    fn fade(&mut self, hw: &mut Hw) {
        let g = self.g;
        let rate = self.gb(g.fade);
        if rate == 0 {
            return;
        }
        let c = self.gb(g.fade + 1).wrapping_sub(1);
        self.sgb(g.fade + 1, c);
        if c != 0 {
            return;
        }
        self.sgb(g.fade + 1, rate);
        let m = self.gw(g.master);
        if self.old() {
            let m = if m >= 0x20 { m - (m >> 3) } else { m.wrapping_sub(1) };
            self.sgw(g.master, m);
            if (m as i16) < 0 {
                self.stop(hw);
            }
        } else {
            let m = m.wrapping_sub(1);
            self.sgw(g.master, m);
            if m == 0 {
                self.stop(hw);
            }
        }
    }

    fn tempo_tick(&mut self) -> bool {
        let t = self.g.tempo + 1;
        let c = self.gb(t).wrapping_sub(1);
        self.sgb(t, c);
        c == 0
    }

    fn tempo_reload(&mut self) {
        let t = self.g.tempo;
        if self.gb(t + 1) == 0 {
            let r = self.gb(t);
            self.sgb(t + 1, r);
        }
    }

    /// The generation-A tick (A, A9C, B, R3, R5, R6).
    fn tick_a(&mut self, hw: &mut Hw) {
        let takes = self.next_latch() != 0;
        self.pending(hw, takes);
        self.take_latch();
        self.request(hw);
        if !self.gen_b() && !self.running() {
            return;
        }
        let tt = self.tempo_tick();
        for k in 0..4 {
            let go = match self.rev() {
                Rev::B | Rev::R6 => self.b(self.active_at(k)) != 0,
                Rev::R3 => true,
                _ => self.running(),
            };
            if !go {
                continue;
            }
            hw.per(k, self.vw(k, 0x06));
            hw.vol(k, self.volume_out(k));
            self.step(k, tt, hw);
            hw.wait(150);
        }
        self.fade(hw);
        self.tempo_reload();
    }

    /// R1 and R2: periods and volumes after the voices.
    fn tick_12(&mut self, hw: &mut Hw) {
        let takes = self.next_latch() != 0;
        self.pending(hw, takes);
        self.take_latch();
        self.request(hw);
        if !self.running() {
            return;
        }
        if self.rev() == Rev::R2 {
            let a = self.abs(self.lay.blocks[3]);
            self.sl(G + 8, a);
        }
        let tt = self.tempo_tick();
        for k in 0..self.nv() {
            self.step(k, tt, hw);
            if self.rev() == Rev::R2 && k == 3 {
                self.pulse(3, hw);
            }
            hw.wait(150);
        }
        self.fade(hw);
        self.tempo_reload();
        if self.running() {
            for k in 0..self.nv() {
                hw.per(k, self.vw(k, 0x06));
                hw.vol(k, self.volume_out(k));
            }
        }
    }

    /// R4: volumes first, periods after.
    fn tick_4(&mut self, hw: &mut Hw) {
        let takes = self.gb(8) != 0 && self.next_latch() != 0;
        self.pending(hw, takes);
        let dblk = self.l(G + 0x0A);
        for k in 0..4 {
            let v = if k == 3 {
                match self.lay.blocks.iter().position(|&b| self.abs(b) == dblk) {
                    Some(i) if dblk != 0 => self.volume_out(i),
                    _ => 0,
                }
            } else {
                self.volume_out(k)
            };
            hw.vol(k, v);
        }
        if self.gb(8) != 0 {
            self.take_latch();
            self.request(hw);
            if !self.running() {
                return;
            }
        }
        let a = self.abs(self.lay.blocks[3]);
        self.sl(G + 0x0A, a);
        let tt = self.tempo_tick();
        for k in 0..4 {
            if self.b(self.active_at(k)) != 0 {
                self.step(k, tt, hw);
                if k == 3 {
                    self.pulse(3, hw);
                }
            }
            hw.wait(150);
        }
        self.fade(hw);
        self.tempo_reload();
        // written after a stop in this tick too (the reference's last ticks)
        for k in 0..4 {
            hw.per(k, self.vw(k, 0x06));
        }
    }

    /// The pulse-buffer generators of R2 and R4 (section 9.6).
    fn pulse(&mut self, k: usize, hw: &mut Hw) {
        let at = self.pulse_index_at();
        let r4 = self.rev() == Rev::R4;
        let mut i = self.w(at);
        if r4 {
            i >>= 1;
        }
        let buf = self.pulse_buffer();
        let f = self.flags(k);
        if f & F_PULSE_DOWN != 0 {
            self.poke(buf.wrapping_add(i as u32), 0x80, hw);
            i = i.wrapping_sub(1);
            if i == if r4 { 6 } else { 0x14 } {
                self.set_flags(k, f & !F_PULSE_DOWN);
            } else {
                self.sw(at, i);
            }
        } else {
            self.poke(buf.wrapping_add(i as u32), 0x7F, hw);
            i = i.wrapping_add(1);
            if i < 0x22 {
                self.sw(at, i);
            } else {
                self.set_flags(k, f | F_PULSE_DOWN);
            }
        }
    }

    fn poke(&mut self, a: u32, v: u8, hw: &mut Hw) {
        let o = self.off(a);
        if o < self.img.len() {
            self.img[o] = v;
            hw.bus.w8(a, v);
            if !self.touched.contains(&a) {
                self.touched.push(a);
            }
        }
    }

    /// Recomputes the period from a note (section 7.3).
    fn set_period(&mut self, k: usize, note: u8) {
        let n = note.wrapping_add(self.b(self.transp_at(k)));
        let idx = if matches!(self.rev(), Rev::R1 | Rev::R2 | Rev::R3 | Rev::R4) { n & 0x7F } else { n & 0x3F };
        let mut per = self.w(self.lay.periods + 2 * idx as usize);
        let mut f = self.flags(k);
        if f & F_PORT != 0 {
            per = per.wrapping_add(self.vw(k, 0x34));
            let t = self.vb(k, 0x36);
            self.svb(k, 0x31, t);
        }
        if f & F_SLIDE5 != 0 {
            f &= !F_SLIDE;
            self.set_flags(k, f);
        }
        self.svw(k, 0x06, per);
    }

    fn stream_byte(&mut self, k: usize) -> u8 {
        let s = self.vl(k, 0x12);
        self.svl(k, 0x12, s.wrapping_add(1));
        self.b(self.off(s))
    }

    /// One voice's work in a running tick (section 7.3).
    fn step(&mut self, k: usize, tt: bool, hw: &mut Hw) {
        if tt {
            let cd = self.vb(k, 0x17).wrapping_sub(1);
            self.svb(k, 0x17, cd);
            if cd == 0 {
                self.fetch(k, hw);
                return;
            }
            let sl = self.vb(k, 0x19);
            if sl != 0 {
                let n = self.vb(k, 0x18).wrapping_add(sl);
                self.svb(k, 0x18, n);
                self.set_period(k, n);
                hw.loc(k, self.vl(k, 0x00));
                hw.len(k, self.vw(k, 0x04));
            }
            let next = self.b(self.off(self.vl(k, 0x12)));
            if self.vb(k, 0x2A) == cd && next != self.rest {
                self.svb(k, 0x2B, 1);
                self.svb(k, 0x19, 0);
                let f = self.flags(k) & !F_SLIDE;
                let f = f | if f & F_SLIDE5 != 0 { F_SLIDE } else { 0 };
                self.set_flags(k, f);
            }
        }
        self.envelope(k);
        let f = self.flags(k);
        if f & F_SLIDE != 0 {
            self.slide(k);
        }
        if f & F_PORT != 0 {
            self.slide(k);
        }
        if f & F_ARP != 0 && !matches!(self.rev(), Rev::R2 | Rev::R4) {
            let pat = self.vl(k, 0x42);
            let i = self.vb(k, 0x41);
            let mut b = self.b(self.off(pat.wrapping_add(i as u32)));
            let mut ni = i.wrapping_add(1);
            if b & 0x80 != 0 {
                ni = 0;
                b &= 0x7F;
            }
            self.svb(k, 0x41, ni);
            let n = self.vb(k, 0x18).wrapping_add(b);
            self.set_period(k, n);
        }
        self.vibrato(k);
    }

    fn envelope(&mut self, k: usize) {
        let linear = self.rev() == Rev::R2;
        let vol = self.vb(k, 0x09);
        let timer = |s: &mut Self, t: usize| -> bool {
            if linear {
                return true;
            }
            let c = s.vb(k, t).wrapping_sub(1);
            if c == 0 {
                let r = s.vb(k, t + 1);
                s.svb(k, t, r);
                true
            } else {
                s.svb(k, t, c);
                false
            }
        };
        if self.vb(k, 0x2C) != 0 {
            if timer(self, 0x3A) {
                let n = self.vb(k, 0x2C) - 1;
                self.svb(k, 0x2C, n);
                let s = self.vb(k, 0x25);
                self.svb(k, 0x09, vol.wrapping_add(s));
            }
        } else if self.vb(k, 0x2D) != 0 {
            if timer(self, 0x3C) {
                let n = self.vb(k, 0x2D) - 1;
                self.svb(k, 0x2D, n);
                let s = self.vb(k, 0x27);
                self.svb(k, 0x09, vol.saturating_sub(s));
            }
        } else if self.vb(k, 0x2B) != 0 && timer(self, 0x3E) {
            let s = self.vb(k, 0x29);
            self.svb(k, 0x09, vol.saturating_sub(s));
        }
    }

    fn slide(&mut self, k: usize) {
        let c = self.vb(k, 0x31);
        if c == 0 {
            return;
        }
        self.svb(k, 0x31, c - 1);
        let s = self.vw(k, 0x32);
        let p = self.vw(k, 0x06);
        let p = if s & 0x8000 != 0 { p.wrapping_sub(s & 0x7FFF) } else { p.wrapping_add(s) };
        self.svw(k, 0x06, p);
    }

    fn vibrato(&mut self, k: usize) {
        let depth = self.vb(k, 0x2E);
        if depth == 0 {
            return;
        }
        let s = self.vl(k, 0x12);
        let delay = self.vb(k, if self.rev() == Rev::R2 { 0x3F } else { 0x40 });
        let run = self.b(self.off(s.wrapping_sub(1))) == self.rest
            || self.vb(k, 0x37) as i32 - self.vb(k, 0x17) as i32 >= delay as i32;
        if !run {
            return;
        }
        let f = self.flags(k);
        let p = self.vw(k, 0x06);
        let p = if f & F_VIBDIR != 0 { p.wrapping_add(depth as u16) } else { p.wrapping_sub(depth as u16) };
        self.svw(k, 0x06, p);
        let c = self.vb(k, 0x30).wrapping_sub(1);
        if c == 0 {
            let h = self.vb(k, 0x2F).wrapping_mul(2);
            self.svb(k, 0x30, h);
            self.set_flags(k, f ^ F_VIBDIR);
        } else {
            self.svb(k, 0x30, c);
        }
    }

    /// Next pattern: the next stream of the order list (section 7.4).
    fn next_pattern(&mut self, k: usize) {
        let f = self.flags(k) & !F_ARP;
        self.set_flags(k, f);
        for _ in 0..GUARD {
            let o = self.vl(k, 0x0E);
            let w = self.w(self.off(o));
            if w != 0 {
                self.svl(k, 0x0E, o.wrapping_add(2));
                let s = self.abs(self.lay.sb + w as usize);
                self.svl(k, 0x12, s);
                return;
            }
            let pos = self.w(self.off(o.wrapping_add(2)));
            let a = self.abs(self.lay.sb + pos as usize);
            self.svl(k, 0x0E, a);
            if k < self.marked.len() {
                self.marked[k] = false;
            }
        }
    }

    fn sample(&mut self, k: usize, i: usize) {
        let r = self.lay.samples + 12 * i;
        let g = self.abs(G);
        let (st, len, ls, ll) = (self.l(r), self.w(r + 4), self.l(r + 6), self.w(r + 10));
        self.svl(k, 0x00, g.wrapping_add(st));
        self.svw(k, 0x04, len);
        self.svl(k, 0x1A, g.wrapping_add(ls));
        self.svw(k, 0x20, ll);
    }

    fn envelope3(&mut self, k: usize, b: u8) {
        self.svb(k, 0x24, b & 0xF);
        let o1 = self.stream_byte(k);
        let o2 = self.stream_byte(k);
        let o3 = self.stream_byte(k);
        self.svb(k, 0x25, o1 >> 4);
        self.svb(k, 0x26, o1 & 0xF);
        self.svb(k, 0x27, o2 >> 4);
        self.svb(k, 0x28, o2 & 0xF);
        self.timers(k, o3);
    }

    /// High nibble: attack timer and reload; low nibble: decay timer and reload.
    fn timers(&mut self, k: usize, o: u8) {
        for (f, v) in [(0x3A, o >> 4), (0x3B, o >> 4), (0x3C, o & 0xF), (0x3D, o & 0xF)] {
            self.svb(k, f, v);
        }
    }

    fn arpeggio(&mut self, k: usize, i: usize) {
        let w = self.w(self.lay.arp + 2 * i) as u32;
        let a = self.abs(self.lay.arp).wrapping_add(w);
        self.svl(k, 0x42, a);
        self.svb(k, 0x41, 0);
        let f = self.flags(k) | F_ARP;
        self.set_flags(k, f);
    }

    fn note(&mut self, k: usize, b: u8, hw: &mut Hw) {
        self.svb(k, 0x18, b);
        self.set_period(k, b);
        let mut f = self.flags(k);
        let start = f & F_TIE == 0;
        if start {
            hw.loc(k, self.vl(k, 0x00));
            hw.len(k, self.vw(k, 0x04));
            let g = self.g;
            let int = self.gw(g.int) | self.vw(k, 0x38);
            self.sgw(g.int, int);
            let dma = self.gw(g.dma) | self.vw(k, 0x22);
            self.sgw(g.dma, dma);
            hw.dmacon(self.vw(k, 0x22) & 0xF);
            let sv = self.vb(k, 0x24);
            self.svb(k, 0x09, sv);
            let a = self.vb(k, 0x26);
            self.svb(k, 0x2C, a);
            let d = self.vb(k, 0x28);
            self.svb(k, 0x2D, d);
            self.svb(k, 0x2B, 0);
            if matches!(self.rev(), Rev::R2 | Rev::R4) && self.vl(k, 0x00) == self.pulse_buffer() {
                let at = self.pulse_index_at();
                self.sw(at, 0x20);
            }
        }
        if self.rev() == Rev::R5 {
            f |= F_VIBDIR;
            self.set_flags(k, f);
        }
        if start || self.rev() != Rev::R1 {
            let h = self.vb(k, 0x2F);
            self.svb(k, 0x30, h);
        }
        let l = self.vb(k, 0x37);
        self.svb(k, 0x17, l);
    }

    fn rest_(&mut self, k: usize) {
        let l = self.vb(k, 0x37);
        self.svb(k, 0x17, l);
    }

    /// Reads the stream up to a note or a rest (section 7.4).
    fn fetch(&mut self, k: usize, hw: &mut Hw) {
        let rev = self.rev();
        for _ in 0..GUARD {
            let b = self.stream_byte(k);
            if b < 0x80 {
                self.note(k, b, hw);
                return;
            }
            if b >= 0xE0 {
                self.svb(k, 0x37, b - 0xDF);
                continue;
            }
            // the ranges of each revision
            match rev {
                Rev::A | Rev::B | Rev::R5 | Rev::R6 => match b {
                    0xD0..=0xDF => {
                        self.sample(k, (b - 0xD0) as usize);
                        continue;
                    }
                    0xC0..=0xCF => {
                        self.envelope3(k, b);
                        continue;
                    }
                    0xB0..=0xBF => {
                        self.svb(k, 0x24, b & 0xF);
                        continue;
                    }
                    0xA0..=0xAF => {
                        self.arpeggio(k, (b - 0xA0) as usize);
                        continue;
                    }
                    _ => {}
                },
                Rev::A9c => match b {
                    0xC0..=0xDF => {
                        self.sample(k, (b - 0xC0) as usize);
                        continue;
                    }
                    0xB0..=0xBF => {
                        self.envelope3(k, b);
                        continue;
                    }
                    0xA0..=0xAF => {
                        self.svb(k, 0x24, b & 0xF);
                        continue;
                    }
                    0x9C..=0x9F => {
                        self.arpeggio(k, (b - 0x9C) as usize);
                        continue;
                    }
                    _ => {}
                },
                Rev::R3 | Rev::R4 => match b {
                    0xC0..=0xDF => {
                        self.sample(k, (b - 0xC0) as usize);
                        continue;
                    }
                    0xB0..=0xBF => {
                        self.envelope3(k, b);
                        continue;
                    }
                    0xA0..=0xAF => {
                        self.arpeggio(k, (b - 0xA0) as usize);
                        continue;
                    }
                    _ => {}
                },
                Rev::R1 => match b {
                    0xC0..=0xDF => {
                        self.sample(k, (b - 0xC0) as usize);
                        continue;
                    }
                    0xB0..=0xBF => {
                        self.arpeggio(k, (b - 0xB0) as usize);
                        continue;
                    }
                    _ => {}
                },
                Rev::R2 => {}
            }
            let Some(&cmd) = self.lay.cmds.get((b & 0x7F) as usize) else {
                // beyond the map: not used by any corpus stream
                continue;
            };
            if !self.command(k, cmd, hw) {
                return;
            }
        }
    }

    /// One jump-table command; false when it ends the fetch.
    fn command(&mut self, k: usize, cmd: Cmd, hw: &mut Hw) -> bool {
        let g = self.g;
        match cmd {
            NextPat => self.next_pattern(k),
            SlideDn => self.svb(k, 0x19, 0xFF),
            SlideUp => self.svb(k, 0x19, 1),
            JumpList => {
                let hi = self.stream_byte(k) as u16;
                let lo = self.stream_byte(k) as u16;
                let a = self.abs(self.lay.sb + (hi << 8 | lo) as usize);
                self.svl(k, 0x0E, a);
                self.next_pattern(k);
            }
            Stop => {
                self.stop(hw);
                if self.rev() == Rev::R1 {
                    self.sgb(1, 0);
                }
                return false;
            }
            Release => {
                let t = self.stream_byte(k);
                self.svb(k, 0x2A, t);
                let o = self.stream_byte(k);
                self.svb(k, 0x3E, o >> 4);
                self.svb(k, 0x3F, o >> 4);
                self.svb(k, 0x29, o & 0xF);
            }
            Rest => {
                self.rest_(k);
                return false;
            }
            Rest0 => {
                self.svb(k, 0x09, 0);
                self.rest_(k);
                return false;
            }
            Transp | GTransp => {
                let v = self.stream_byte(k);
                let at = self.transp_at(k);
                self.sb_(at, v);
            }
            Vib => {
                let delay = self.stream_byte(k);
                let depth = self.stream_byte(k);
                let half = self.stream_byte(k);
                self.svb(k, if self.rev() == Rev::R2 { 0x3F } else { 0x40 }, delay);
                self.svb(k, 0x2E, depth);
                self.svb(k, 0x2F, half);
            }
            VibOff => self.svb(k, 0x2E, 0),
            Port => {
                for f in [0x35, 0x34, 0x33, 0x32, 0x36] {
                    let v = self.stream_byte(k);
                    self.svb(k, f, v);
                }
                let f = self.flags(k) | F_PORT;
                self.set_flags(k, f);
            }
            PortOff => {
                let f = self.flags(k) & !F_PORT;
                self.set_flags(k, f);
            }
            Slide1 | Slide5 => {
                let f = self.flags(k) | if cmd == Slide1 { F_SLIDE } else { F_SLIDE5 };
                self.set_flags(k, f);
                for f in [0x33, 0x32, 0x31] {
                    let v = self.stream_byte(k);
                    self.svb(k, f, v);
                }
            }
            Slide5Off => {
                let f = self.flags(k) & !F_SLIDE5;
                self.set_flags(k, f);
            }
            TieOn => {
                let f = self.flags(k) | F_TIE;
                self.set_flags(k, f);
            }
            TieOff => {
                let f = self.flags(k) & !F_TIE;
                self.set_flags(k, f);
            }
            ArpOff => {
                let f = self.flags(k) & !F_ARP;
                self.set_flags(k, f);
            }
            VolDn => {
                let v = self.vb(k, 0x24);
                if v != 0 {
                    self.svb(k, 0x24, v - 1);
                }
            }
            VolUp => {
                let v = self.vb(k, 0x24);
                if v < 15 {
                    self.svb(k, 0x24, v + 1);
                }
            }
            VolDnWrap | VolUpWrap => {
                let v = self.vb(k, 0x24);
                self.svb(k, 0x24, if cmd == VolDnWrap { v.wrapping_sub(1) } else { v.wrapping_add(1) });
            }
            Fade => {
                if self.rev() == Rev::R5 {
                    let b = self.lay.fade as u8;
                    self.sgb(g.fade, b);
                    self.sgb(g.fade + 1, b);
                } else {
                    self.sgw(g.fade, self.lay.fade);
                }
            }
            Fade1 => {
                self.sgb(g.fade, 1);
                self.sgb(g.fade + 1, 1);
            }
            SfxEnd => {
                match self.rev() {
                    Rev::B | Rev::R6 => {
                        self.svb(k, 0x09, 0);
                        hw.dmacon(1 << k);
                        if self.rev() == Rev::R6 {
                            hw.vol(k, 0);
                            hw.bus.write_word(0xDFF0AA + 16 * k as u32, 0);
                        }
                        let a = self.active_at(k);
                        self.sb_(a, 0);
                    }
                    Rev::R4 => {
                        self.sgb(3, 0);
                        let a = self.active_at(k);
                        self.sb_(a, 0);
                        if self.gb(8) != 0 {
                            hw.vol(3, 0);
                            hw.dmacon(8);
                        } else {
                            self.svb(k, 0x09, 0);
                            hw.vol(k, 0);
                            hw.dmacon(1 << k);
                        }
                    }
                    _ => {
                        self.sgb(3, 0);
                        hw.dmacon(8);
                    }
                }
                return false;
            }
            Env6 => {
                for f in 0x24..=0x28 {
                    let v = self.stream_byte(k);
                    self.svb(k, f, v);
                }
                let o = self.stream_byte(k);
                self.timers(k, o);
            }
            Env7 => {
                for f in 0x24..=0x2A {
                    let v = self.stream_byte(k);
                    self.svb(k, f, v);
                }
            }
            Sample => {
                let i = self.stream_byte(k) as usize;
                self.sample(k, i);
            }
            StartVol => {
                let v = self.stream_byte(k);
                self.svb(k, 0x24, v);
            }
            ModSet => {
                for _ in 0..3 {
                    self.stream_byte(k);
                }
            }
            ModOff => {}
            Builder => {
                let n = self.stream_byte(k) as u32;
                let buf = self.pulse_buffer();
                for i in 0..=n {
                    self.poke(buf.wrapping_add(i), 0x7F, hw);
                }
                self.poke(buf.wrapping_add(n + 1), 0x80, hw);
            }
        }
        true
    }
}

impl Replayer for SoundImages {
    fn format(&self) -> &'static str {
        "Sound Images"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.nsub as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.sub = n;
        self.started = false;
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.startup(hw);
        }
        if let Some((c, mask)) = self.lay.divider {
            let v = self.w(c).wrapping_add(1) & mask;
            self.sw(c, v);
            if v == 0 {
                return;
            }
        }
        if let Some(h) = &self.lay.hook
            && h.applies(self.gb(1))
        {
            let p = self.gw(8);
            self.svw(0, 0x06, p);
        }
        match self.rev() {
            Rev::R1 | Rev::R2 => self.tick_12(hw),
            Rev::R4 => self.tick_4(hw),
            _ => self.tick_a(hw),
        }
        // UADE's song end (section 5): the song stopped, or every voice with a
        // header has read the terminator of its order list
        if !self.running() || self.had_marks && !self.marked.iter().any(|&m| m) {
            self.songend = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn put32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    fn words(d: &mut [u8], o: usize, w: &[u16]) {
        for (i, &v) in w.iter().enumerate() {
            put16(d, o + 2 * i, v);
        }
    }

    const BLOCKS: usize = 0x600;

    /// A generation-A module: the locator instructions, each next to its table
    /// (brief displacements reach 127 bytes), two songs whose four voices share one
    /// header and one stream (sample 0, start volume 8, length 3, note 12, next
    /// pattern), an order list of the stream once then the loop.
    fn module() -> Vec<u8> {
        let mut d = vec![0u8; 0x800];
        words(&mut d, 0, &[0x6000, 0x0060]);
        let p = 0x62;
        words(&mut d, p, &[0x48E7, 0xF0E6, 0x4DF9, 0x00DF, 0xF000]);
        for k in 0..4 {
            let o = p + 10 + 4 * k;
            let b = BLOCKS + 0x48 * k;
            words(&mut d, o, &[0x41FA, (b - (o + 2)) as u16]);
            put32(&mut d, b + 0x0A, 0xDFF0A0 + 0x10 * k as u32);
            put16(&mut d, b + 0x22, 0x8200 | 1 << k);
            put16(&mut d, b + 0x38, 0x8000 | 0x80 << k);
            d[b + 0x37] = 0x10;
        }
        // song start: cmp.b #2,d0 ; mulu.w #10,d0 ; lea (d8,pc,d0.w),a1 -> SB 0x90
        words(&mut d, 0x7C, &[0xB03C, 0x0002, 0xC0FC, 0x000A, 0x43FB, 0x000A]);
        let sb = 0x90;
        for e in 0..2 {
            words(&mut d, sb + 10 * e, &[0x14, 0x14, 0x14, 0x14, (e as u16 + 1) << 8]);
        }
        words(&mut d, sb + 0x14, &[0x20, 0x20, 0, 0x16]);
        d[sb + 0x20..sb + 0x25].copy_from_slice(&[0xD0, 0xB8, 0xE2, 0x0C, 0x80]);
        // the dispatcher's bounds and the jump table at 0xE0
        words(&mut d, 0xC0, &[0xB03C, 0xE0, 0xB03C, 0xD0, 0xB03C, 0xC0, 0xB03C, 0xB0, 0xB03C, 0xA0]);
        words(&mut d, 0xD4, &[0x0240, 0x007F, 0xD040, 0x303B, 0x0004]);
        for i in 0..21 {
            let h = 0x300 + 4 * i;
            put16(&mut d, 0xE0 + 2 * i, (h - 0xE0) as u16);
            words(&mut d, h, &[0x4E71, 0x4E71]);
        }
        // first words: slidedn, X (tieoff's operation on another bit pattern: tieon),
        // tieoff, Y (slidedn's instruction with another immediate: slideup), fade
        words(&mut d, 0x300 + 4, &[0x1111, 0x00FF]);
        words(&mut d, 0x300 + 4 * 0x0E, &[0x2223, 0x0004]);
        words(&mut d, 0x300 + 4 * 0x0F, &[0x2222, 0x0004]);
        words(&mut d, 0x300 + 4 * 0x11, &[0x1111, 0x0001]);
        words(&mut d, 0x300 + 4 * 0x13, &[0x5555, 0x2020]);
        // the sample table at 0x120: one record, attack 8 words at 0x400, loop 4 at 0x408
        words(&mut d, 0x10A, &[0xC0FC, 0x000C, 0x45FB, 0x0010]);
        put32(&mut d, 0x120, 0x400 - 4);
        put16(&mut d, 0x124, 8);
        put32(&mut d, 0x126, 0x408 - 4);
        put16(&mut d, 0x12A, 4);
        // the arpeggio table at 0x140, the period table at 0x150, the volume table at 0x1E0
        words(&mut d, 0x12C, &[0x303B, 0x0012, 0x45FB, 0x000E]);
        words(&mut d, 0x14A, &[0x317B, 0x0004, 0x0006]);
        for i in 0..64 {
            put16(&mut d, 0x150 + 2 * i, 1000 - 10 * i as u16);
        }
        words(&mut d, 0x1D0, &[0xC0ED, 0x0006, 0xE848, 0xD040, 0x303B, 0x0006]);
        for i in 0..16 {
            put16(&mut d, 0x1E0 + 2 * i, 4 * i as u16);
        }
        d
    }

    fn ch0(log: &[(u32, u16, u64)]) -> Vec<(u32, u16)> {
        // channel 0's registers and the DMACON writes other than the other voices' own
        log.iter()
            .filter(|w| (0xDFF0A0..0xDFF0AA).contains(&w.0) || w.0 == 0xDFF096 && ![2, 4, 8].contains(&w.1))
            .map(|w| (w.0, w.1))
            .collect()
    }

    #[test]
    fn locates_the_tables_and_tells_the_revision() {
        let d = module();
        let l = layout(&d).expect("recognised");
        assert_eq!(l.rev, Rev::A);
        assert_eq!((l.sb, l.stride, l.nsub), (0x90, 10, 2));
        assert_eq!((l.samples, l.arp, l.periods, l.voltab), (0x120, 0x140, 0x150, Some(0x1E0)));
        assert_eq!((l.cmds[0x0E], l.cmds[0x11], l.fade), (TieOn, SlideUp, 0x2020));
        assert_eq!(l.blocks, (0..4).map(|k| BLOCKS + 0x48 * k).collect::<Vec<_>>());
        // a test of the active byte in the play area: generation B
        let mut b = d.clone();
        words(&mut b, 0x200, &[0x4A28, 0x0047]);
        assert_eq!(layout(&b).map(|l| l.rev), Some(Rev::B));
        // three blocks: R1 (its eight-byte entries)
        let mut r1 = d.clone();
        words(&mut r1, 0x78, &[0x4E71, 0x4E71]);
        words(&mut r1, 0x82, &[0x4E71]);
        assert_eq!(layout(&r1).map(|l| (l.rev, l.stride)), Some((Rev::R1, 8)));
    }

    #[test]
    fn refuses_other_files() {
        let d = module();
        assert!(detect(&d));
        for (o, v) in [(0usize, 0x6002u16), (2, 0x0061), (0x64, 0xF0E7), (BLOCKS + 0x22, 0x8202), (0x84, 0x4E71)] {
            let mut e = d.clone();
            put16(&mut e, o, v);
            assert!(!detect(&e), "{o:#x}");
        }
        assert!(!detect(&d[..0x7FE]));
        assert!(!detect(&[0u8; 0x1000]));
        // the songs table names no header: no subsong
        let mut e = d.clone();
        e[0x90..0xA2].fill(0);
        assert!(!detect(&e));
    }

    #[test]
    fn plays_notes_loops_and_ends_at_the_order_list_terminator() {
        let d = module();
        let (format, log, end, base) = run(Path::new("tw.test"), d, 0, 8);
        assert_eq!(format, "Sound Images");
        let lc = |a: u32| [(0xDFF0A0, (a >> 16) as u16), (0xDFF0A2, a as u16)];
        let mut want = vec![(0xDFF096, 0x0000), (0xDFF096, 0x000F), (0xDFF0A6, 0), (0xDFF0A8, 0)];
        want.extend(lc(base + 0x400));
        want.extend([(0xDFF0A4, 8), (0xDFF096, 0x0001)]);
        // tick 1: the four voices' DMA, the interrupt installs the loop, then the note
        want.push((0xDFF096, 0x820F));
        want.extend(lc(base + 0x408));
        want.extend([(0xDFF0A4, 4), (0xDFF0A6, 880), (0xDFF0A8, 32)]);
        let got = ch0(&log);
        assert_eq!(got[..want.len()], want[..]);
        // the countdown of 3 tempo ticks: the stream again on tick 3
        let starts: Vec<u64> = log.iter().filter(|w| w.0 == 0xDFF0A4 && w.1 == 8).map(|w| (w.2 + 3000) / 70940).collect();
        assert_eq!(starts[1] - starts[0], 3);
        // song end at tick 6: every voice read the terminator
        assert!(end.is_some());
        let (_, _, end6, _) = run(Path::new("tw.test"), module(), 0, 6);
        assert!(end6.is_none());
    }

    #[test]
    fn second_subsong_has_its_tempo() {
        let d = module();
        let (_, log, _, _) = run(Path::new("tw.test"), d, 1, 10);
        let starts: Vec<u64> = log.iter().filter(|w| w.0 == 0xDFF0A4 && w.1 == 8).map(|w| (w.2 + 3000) / 70940).collect();
        // tempo 2: three steps of two ticks each
        assert_eq!(starts[1] - starts[0], 6);
    }

    /// Plays `data` at subsong `sub` for `ticks` ticks: the write log and the bus
    /// clock at which the song end was first seen.
    fn run(path: &Path, data: Vec<u8>, sub: usize, ticks: usize) -> (String, Vec<(u32, u16, u64)>, Option<u64>, u32) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(path, data).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let base = e.m.deli.list.first().map_or(0, |l| l.0);
        for _ in 0..sub {
            e.change_subsong(1).unwrap();
        }
        let mut buf = vec![0f32; 2 * 960];
        let mut end = None;
        let mut left = ticks;
        while left > 0 {
            e.render(&mut buf);
            left -= 1;
            if end.is_none() && e.songend() {
                end = Some(e.m.bus.clock);
                left = left.min(3);
            }
        }
        (format, e.m.bus.write_log.take().unwrap(), end, base)
    }

    /// `TW_MODS=dir TW_OUT=dir [TW_ONLY=substr] cargo test --release corpus_dump -- --ignored`:
    /// the write log of every subsong of every module under `dir/<game>/`, 1000 ticks or
    /// to the song end, with the song-end clock, for the comparison with UADE's logs.
    #[test]
    #[ignore]
    fn corpus_dump() {
        let (Ok(mods), Ok(out)) = (std::env::var("TW_MODS"), std::env::var("TW_OUT")) else { return };
        let only = std::env::var("TW_ONLY").unwrap_or_default();
        let mut games: Vec<_> = std::fs::read_dir(&mods).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        games.sort();
        for g in games {
            let mut files: Vec<_> = std::fs::read_dir(&g).unwrap().flatten().map(|e| e.path()).collect();
            files.sort();
            for f in files {
                let name = format!("{}__{}", g.file_name().unwrap().to_string_lossy(), f.file_name().unwrap().to_string_lossy());
                if !name.contains(&only) {
                    continue;
                }
                let data = std::fs::read(&f).unwrap();
                let Some(lay) = layout(&data) else {
                    println!("{name} NODETECT");
                    continue;
                };
                println!("{name} {:?} subsongs {} 8E/91 {:?} {:?} fade {:04x}", lay.rev, lay.nsub, lay.cmds.get(0x0E), lay.cmds.get(0x11), lay.fade);
                for s in 0..lay.nsub {
                    let (format, log, end, base) = run(&f, data.clone(), s, 1000);
                    assert_eq!(format, "Sound Images");
                    let sfx = if s == 0 { String::new() } else { format!(".s{}", s + 1) };
                    let mut text = format!("# rev {:?}\n# base {base}\n# songend {}\n", lay.rev, end.map_or(-1, |c| c as i64));
                    for (a, v, t) in log {
                        text += &format!("{a:06x} {v:04x} {t}\n");
                    }
                    std::fs::write(Path::new(&out).join(format!("{name}{sfx}.port.log")), text).unwrap();
                }
            }
        }
    }
}
