//! SCUMM (`scumm.*`), the sound routines of the Amiga versions of Lucasfilm's
//! Maniac Mansion and Zak McKracken.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's SCUMM_Player and the structure of
//! the corpus files), and this player was written from it alone and checked against
//! those writes. ScummVM was not consulted.
//!
//! Each file is one sound resource of the game: a 68k routine with its data, which
//! the game calls once a tick. It is not run. The routine belongs to one of four
//! families, recognised by short instruction signatures, whose numeric parameters
//! (periods, steps, limits, table offsets, the end action of each track) are read
//! from the instructions that carry them; the families' behaviour is written here:
//! A, a period sweep over the four channels (Zak's teleport); B, two bouncing
//! periods a channel pair (Zak's alarm); C, a two-slot sequencer of four tracks on
//! channels 2 and 3 (airport, kazoo, mars); D, a four-slot sequencer (the Maniac
//! Mansion title and piano tune, the Zak title). The routines change their image
//! (instrument state, and in three of them sample bytes): the port keeps its own
//! copy for the state and makes the sample changes in guest memory, where Paula
//! plays them.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "SCUMM", detect, new };

/// The routine's register save, its first instruction (movem.l d1-d7/a0-a6,-(sp)).
const MOVEM: u32 = 0x48E7_7FFE;
/// The sequencers' second instruction (movea.l a3,a2; movea.l a3,a5).
const SEQ: u32 = 0x244B_2A4B;
/// The effects' second instruction (bclr #4,d0).
const FX: u32 = 0x0880_0004;

/// The scratch area the player owns for the routine (`a0`).
const SCRATCH: usize = 0x120;

// Gaps between writes in colour clocks, from the reference runs: they place the
// writes against Paula (DMA off to on in particular), they are not compared. The routine's
// time is counted in 68000 cycles (`Scumm::w`), a write falling at half the cycles since the
// call's start, rounded down, as the engine runs guest code: these constants count twice, and
// where the reference's cycle count is odd (measured by a second run whose count starts a cycle
// late) the wait is a cycle shorter or longer at its use (family C's update: 2 * 233 - 1 before
// the first write, 2 * 63 - 1 from LEN to PER, a cycle more after the volume write unless a
// release still sounds; a note's VOL to DMA on 2 * 44 - 1).
/// From the tick to the routine's first instructions.
const T_ENTRY: u64 = 140;
/// Before a group of channel writes (a note start or a slot update).
const T_GROUP: u64 = 134;
/// Before a note's DMA off.
const T_STOP: u64 = 88;
/// From a slot's DMA off to the next note's first write (the start sequence's own T_GROUP included).
const OFF_TO_START: u64 = 322 - T_GROUP;
/// After a note's DMA on, on top of the update's own wait that follows.
const AFTER_START: u64 = 149;
/// The first call's first two notes (the later ones go as the other calls'): the wait before
/// their first write, and after the second one's DMA on.
const COLD_START: [u64; 2] = [439, 411];
const COLD_AFTER: u64 = 118;
/// Family C, on top of the plain work: a looping track end, the second note start of a call (its
/// DMA off comes later, its DMA on's follow-up sooner), what a slot's update costs after its volume
/// write when the duration runs out, when the release switches the slot off, and when both
/// envelope indexes have stopped at their maximum.
// Family D (the four-slot sequencer), in 68000 cycles; the first index is 1 for the Maniac Mansion
// build (a note start first turns its channel's DMA off), 0 for the Zak build.
/// From the call's entry to the first volume write of the first call.
const D_START: [u64; 2] = [255, 301];
/// A track examination that finds nothing due, per track (the Zak build's last track takes longer).
const D_FAIL: [[u64; 4]; 2] = [[69, 69, 69, 89], [73, 73, 73, 73]];
/// From the end of the examinations (or the previous note's tail) to a note's first write.
const D_LEAD: [u64; 2] = [804, 430];
/// The very first note of the run takes this much longer.
const D_FIRST_EXTRA: [u64; 2] = [47, 63];
/// A note whose slot still plays a note: the slot's release.
const D_STOP_ACTIVE: u64 = 30;
const D_OFF_TO_LOC: u64 = 406;
const D_VOL_TO_ON: [u64; 2] = [60, 56];
/// After a note's DMA on, before the same track is examined again (track 0's TRACK0_LESS less).
const D_AFTER: [u64; 2] = [229, 245];
/// The slot pass: before an update's first write, after its last, after a note-end DMA off, for a
/// slot that is not active, for a slot whose note started in this call.
const D_UPD_PRE: u64 = 243;
const D_UPD_POST: u64 = 222;
const D_OFF_POST: u64 = 149;
const D_IDLE: u64 = 21;
const D_JUST: u64 = 304;
/// From an update's volume write to the DMA off that ends the note.
const D_VOL_TO_OFF: [u64; 2] = [153, 149];
/// The bookkeeping of track 0 (its words at the start of the scratch area) takes 8 cycles less
/// than the other tracks', after a note (D) and at a track's looping end (C).
const TRACK0_LESS: u64 = 8;
const LOOP_END: u64 = 70;
const SECOND_STOP: u64 = 34;
const SECOND_AFTER: u64 = 31;
const DUR_END: u64 = 29;
const REL_OFF: u64 = 18;
const SATURATED: u64 = 16;

// ------------------------------------------------------------------ recognition

/// The routine's offset in the file.
fn routine(d: &[u8]) -> Option<usize> {
    if d.len() < 16 {
        return None;
    }
    let c = if be16(d, 4) == 0x6000 { (6 + be16(d, 6) as i16 as isize) as usize } else { 4 };
    if c % 2 != 0 || c + 8 > d.len() || be32(d, c) != MOVEM {
        return None;
    }
    matches!(be32(d, c + 4), SEQ | FX).then_some(c)
}

/// The code to search: from the routine to the data (or the file's end if the data
/// comes first).
fn code(d: &[u8], c: usize) -> &[u8] {
    let data = 4 + be16(d, 2) as usize;
    let end = if data > c { data.min(d.len()) } else { d.len() };
    &d[..end]
}

/// The even offsets at or after `from` where `pat` starts.
fn find_all<'a>(d: &'a [u8], from: usize, pat: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    (from..d.len().saturating_sub(pat.len() - 1)).step_by(2).filter(move |&i| &d[i..i + pat.len()] == pat)
}

fn find(d: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    find_all(d, from, pat).next()
}

/// The word after the first `pat`.
fn imm16(d: &[u8], from: usize, pat: &[u8]) -> Option<u16> {
    find(d, from, pat).filter(|&i| i + pat.len() + 2 <= d.len()).map(|i| be16(d, i + pat.len()))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Vol {
    /// `p - K`
    Minus(u16),
    /// `K - p`
    From(u16),
}

#[derive(Clone, Debug, PartialEq)]
struct Sweep {
    p0: u16,
    vol: Vol,
    /// The step of `base` every fourth tick (signed).
    step: i16,
    lim: u16,
    /// Ends when `base <= lim` (else when `base >= lim`).
    le: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Siren {
    w2: u16,
    d6: u16,
    d7: u16,
    dec: u16,
    s: [u16; 8],
    lim: [u16; 4],
}

/// What a track does at its end marker.
#[derive(Clone, Debug, PartialEq)]
struct EndAction {
    /// The scratch words cleared (offsets in `a0`).
    clears: Vec<u16>,
    /// Loops (re-tests the track) or stops (the call returns C=1).
    looping: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Seq {
    four: bool,
    inst: usize,
    env: usize,
    tracks: [usize; 4],
    ends: Vec<EndAction>,
    /// kazoo: the offset after the attack sample whose 4 bytes a note start clears.
    clear_at: Option<u16>,
    /// Maniac Mansion build: a note start first turns its channel's DMA off.
    dma_off_first: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Family {
    Sweep(Sweep),
    Siren(Siren),
    Seq(Seq),
}

fn sweep(d: &[u8], c: usize) -> Option<Sweep> {
    let p0 = imm16(d, c, &[0x06, 0x44])?;
    let i = find(d, c, &[0xE9, 0x4C, 0xD8, 0x68, 0x00, 0x02])? + 6;
    let vol = match (be16(d, i), be16(d, i + 2), be16(d, i + 4)) {
        (0x3A04, 0x0445, k) => Vol::Minus(k),
        (0x3A3C, k, 0x9A44) => Vol::From(k),
        _ => return None,
    };
    let j = find(d, c, &[0x02, 0x50, 0x00, 0x03])?;
    if d.get(j + 4) != Some(&0x66) {
        return None;
    }
    let (op, lim_op) = (be16(d, j + 6), j + 10);
    if op & 0xF0FF != 0x5068 || be16(d, j + 8) != 2 || be16(d, lim_op) != 0x0C68 || be16(d, lim_op + 4) != 2 {
        return None;
    }
    let q = match (op >> 9) & 7 {
        0 => 8,
        q => q as i16,
    };
    let step = if op & 0x100 != 0 { -q } else { q };
    let le = match d.get(lim_op + 6) {
        Some(0x6F) => true,
        Some(0x6C) => false,
        _ => return None,
    };
    Some(Sweep { p0, vol, step, lim: be16(d, lim_op + 2), le })
}

fn siren(d: &[u8], c: usize) -> Option<Siren> {
    let w2 = imm16(d, c, &[0x47, 0xEB])?;
    let d6 = imm16(d, c, &[0x3C, 0x3C])?;
    let d7 = imm16(d, c, &[0x3E, 0x3C])?;
    let dec = imm16(d, c, &[0x04, 0x46])?;
    // eight consecutive move.w #imm,(a0)+
    let i = find(d, c, &[0x30, 0xFC])?;
    let mut s = [0; 8];
    for (k, v) in s.iter_mut().enumerate() {
        if be16(d, i + 4 * k) != 0x30FC {
            return None;
        }
        *v = be16(d, i + 4 * k + 2);
    }
    let l: Vec<u16> = find_all(d, c, &[0x0C, 0x47]).take(4).map(|i| be16(d, i + 2)).collect();
    let lim: [u16; 4] = l.try_into().ok()?;
    Some(Siren { w2, d6, d7, dec, s, lim })
}

/// An offset parameter: `adda.w #imm` (sign-extended) or `adda.l #imm`, whichever
/// comes first; the `n`-th one (for the track tables).
fn adda(d: &[u8], c: usize, w: [u8; 2], l: [u8; 2], n: usize) -> Option<i32> {
    let mut v: Vec<(usize, i32)> = find_all(d, c, &w).map(|i| (i, be16(d, i + 2) as i16 as i32)).collect();
    v.extend(find_all(d, c, &l).map(|i| (i, be32(d, i + 2) as i32)));
    v.sort();
    v.get(n).map(|x| x.1)
}

fn end_action(d: &[u8], at: usize) -> Option<EndAction> {
    // after cmpi.w #-1,2(a4): the bne over the end action
    let mut i = at + 6;
    if d.get(i) != Some(&0x66) {
        return None;
    }
    i += if d.get(i + 1) == Some(&0) { 4 } else { 2 };
    let mut clears = Vec::new();
    for _ in 0..16 {
        match be16(d, i) {
            0x317C if be16(d, i + 2) == 0 => {
                clears.push(be16(d, i + 4));
                i += 6;
            }
            0x30BC if be16(d, i + 2) == 0 => {
                clears.push(0);
                i += 4;
            }
            0x4268 => {
                clears.push(be16(d, i + 2));
                i += 4;
            }
            0x4250 => {
                clears.push(0);
                i += 2;
            }
            0x265F => return Some(EndAction { clears, looping: false }),
            w if w >> 8 == 0x60 => return Some(EndAction { clears, looping: true }),
            _ => return None,
        }
    }
    None
}

fn sequencer(d: &[u8], c: usize, size: usize) -> Option<Seq> {
    let four = find(&d[..(c + 0x400).min(d.len())], c, &[0x33, 0xFC, 0x80, 0x0F, 0x00, 0xDF, 0xF0, 0x96]).is_some();
    let off = |v: i32| (4 + v as i64).try_into().ok().filter(|&o: &usize| o < size);
    let inst = off(adda(d, c, [0xD4, 0xFC], [0xD5, 0xFC], 0)?)?;
    let env = off(adda(d, c, [0xDA, 0xFC], [0xDB, 0xFC], 0)?)?;
    let mut tracks = [0; 4];
    let mut cum = 0i32;
    for (k, t) in tracks.iter_mut().enumerate() {
        cum = cum.wrapping_add(adda(d, c, [0xD6, 0xFC], [0xD7, 0xFC], k)?);
        *t = off(cum)?;
    }
    let ends: Vec<EndAction> =
        find_all(d, c, &[0x0C, 0x6C, 0xFF, 0xFF, 0x00, 0x02]).take(4).map(|i| end_action(d, i)).collect::<Option<_>>()?;
    if ends.len() != 4 {
        return None;
    }
    let clear_at = find(d, c, &[0x23, 0x7C, 0, 0, 0, 0]).map(|i| be16(d, i + 6));
    let dma_off_first = four && find(d, c, &[0x02, 0x41, 0x00, 0x03, 0x42, 0x47]).is_some();
    Some(Seq { four, inst, env, tracks, ends, clear_at, dma_off_first })
}

fn family(d: &[u8]) -> Option<Family> {
    let c = routine(d)?;
    let k = code(d, c);
    if be32(d, c + 4) == FX {
        if be32(d, c + 8) == 0x6758_30BC {
            sweep(k, c).map(Family::Sweep)
        } else {
            siren(k, c).map(Family::Siren)
        }
    } else {
        sequencer(k, c, d.len()).map(Family::Seq)
    }
}

fn detect(d: &[u8]) -> bool {
    family(d).is_some()
}

// ------------------------------------------------------------------ player

struct Scumm {
    fam: Family,
    /// The routine's image (the module), changed as the routine changes it.
    img: Vec<u8>,
    base: u32,
    title: String,
    /// The player's scratch area (`a0`).
    a0: [u8; SCRATCH],
    first: bool,
    /// The engine has asked for the first tick's time.
    armed: std::cell::Cell<bool>,
    songend: bool,
    /// Notes started so far by the first call (its code runs slower than the later calls').
    cold: u8,
    in_first: bool,
    /// Note starts made so far in this call.
    nstart: u8,
    /// The call's start (the clock of its tick) and the 68000 cycles its code has run since:
    /// a write falls at `t0 + cyc / 2`, the half cycle dropped, as the engine runs guest code.
    t0: u64,
    cyc: u64,
    /// The track whose note is starting.
    track: usize,
}

impl Scumm {
    /// `a1`: the data base, absolute.
    fn a1(&self) -> u32 {
        self.base + 4 + be16(&self.img, 2) as u32
    }

    fn sw(&self, o: usize) -> u16 {
        be16(&self.a0, o)
    }
    fn set_sw(&mut self, o: usize, v: u16) {
        if o + 2 <= SCRATCH {
            self.a0[o..o + 2].copy_from_slice(&v.to_be_bytes());
        }
    }
    fn set_sb(&mut self, o: usize, v: u8) {
        if o < SCRATCH {
            self.a0[o] = v;
        }
    }

    fn iw(&self, o: usize) -> u16 {
        be16(&self.img, o)
    }
    /// A word of the image changed by the routine (in the copy and in guest memory).
    fn set_iw(&mut self, o: usize, v: u16, hw: &mut Hw) {
        if o + 2 <= self.img.len() {
            self.img[o..o + 2].copy_from_slice(&v.to_be_bytes());
        }
        let a = self.base.wrapping_add(o as u32);
        if (a as usize) + 2 <= hw.bus.ram.len() {
            hw.bus.ram[a as usize..a as usize + 2].copy_from_slice(&v.to_be_bytes());
        }
    }
    /// Clears the 4 bytes at absolute address `a` (guest memory, and the copy where
    /// it holds the module).
    fn clear4(&mut self, a: u32, hw: &mut Hw) {
        let o = a.wrapping_sub(self.base) as usize;
        if o + 4 <= self.img.len() {
            self.img[o..o + 4].fill(0);
        }
        if (a as usize) + 4 <= hw.bus.ram.len() {
            hw.bus.ram[a as usize..a as usize + 4].fill(0);
        }
    }

    /// Lets `c` 68000 cycles of the routine pass.
    fn w(&mut self, hw: &mut Hw, c: u64) {
        self.cyc += c;
        hw.bus.clock = self.t0 + self.cyc / 2;
    }

    fn call(&mut self, cmd: u32, hw: &mut Hw) -> bool {
        self.t0 = hw.bus.clock;
        self.cyc = 0;
        // the sequencers' entry: a colour clock more, their first write's half cycle (see T_GROUP)
        self.w(hw, 2 * T_ENTRY + 2 * matches!(self.fam, Family::Seq(_)) as u64);
        let start = cmd & 0x10 != 0;
        let d0 = cmd & !0x10;
        match self.fam.clone() {
            Family::Sweep(s) => self.sweep(&s, start, d0 as u16, hw),
            Family::Siren(s) => self.siren(&s, start, d0 as u16, hw),
            Family::Seq(s) => self.seq(&s, start, d0 as u16, hw),
        }
    }

    // ---- family A

    fn sweep(&mut self, s: &Sweep, start: bool, d0: u16, hw: &mut Hw) -> bool {
        let a1 = self.a1();
        let cnt = if start { 0 } else { self.sw(0) };
        let (p, v) = if start {
            (cnt.wrapping_add(s.p0), 0x3F)
        } else {
            let p = (cnt << 4).wrapping_add(self.sw(2));
            let v = match s.vol {
                Vol::Minus(k) => p.wrapping_sub(k),
                Vol::From(k) => k.wrapping_sub(p),
            };
            (p, if v as i16 > 0x3F { 0x3F } else { v })
        };
        if start {
            self.set_sw(0, 0);
            self.w(hw, 2 * 8);
            hw.dmacon(d0);
            self.w(hw, 2 * SWEEP_AFTER_DMA);
        }
        let mut j = 0u16;
        for ch in 0..4 {
            self.w(hw, 2 * if ch == 2 { SWEEP_CH2_WAIT } else { 27 });
            if d0 & 1 << ch != 0 {
                if j == cnt {
                    hw.loc(ch, a1);
                    self.w(hw, 2 * 8);
                    hw.len(ch, 16);
                    self.w(hw, 2 * 6);
                    hw.per(ch, p);
                    self.w(hw, 2 * if start { 8 } else { 6 });
                    hw.vol(ch, v);
                }
                j = j.wrapping_add(1);
            }
        }
        if start {
            self.set_sw(0, cnt.wrapping_add(1));
            self.w(hw, 2 * (34 - (SWEEP_CH2_WAIT - 27)));
            hw.dmacon(d0 | 0x8000);
            self.w(hw, 2 * 26);
            self.set_sw(2, s.p0);
            return false;
        }
        let cnt = cnt.wrapping_add(1) & 3;
        self.set_sw(0, cnt);
        if cnt != 0 {
            return false;
        }
        let base = self.sw(2).wrapping_add(s.step as u16);
        self.set_sw(2, base);
        let end = if s.le { base as i16 <= s.lim as i16 } else { base as i16 >= s.lim as i16 };
        if end {
            self.w(hw, 2 * 62);
            hw.dmacon(d0);
        }
        end
    }

    // ---- family B

    fn siren(&mut self, s: &Siren, start: bool, d0: u16, hw: &mut Hw) -> bool {
        if start {
            self.w(hw, 2 * 6);
            hw.dmacon(d0);
            let (mut wa, mut wb) = (self.a1(), self.a1().wrapping_add(s.w2 as u32));
            let (mut p, mut q) = (s.d6, s.d7);
            for ch in 0..4 {
                self.w(hw, 2 * 39);
                if d0 & 1 << ch != 0 {
                    hw.loc(ch, wa);
                    self.w(hw, 2 * 8);
                    hw.len(ch, 16);
                    self.w(hw, 2 * 6);
                    hw.per(ch, p);
                    self.w(hw, 2 * 8);
                    hw.vol(ch, 0x3F);
                    std::mem::swap(&mut p, &mut q);
                    std::mem::swap(&mut wa, &mut wb);
                    p = p.wrapping_sub(s.dec);
                    q = q.wrapping_sub(s.dec);
                }
            }
            self.w(hw, 2 * 38);
            hw.dmacon(d0 | 0x8000);
            for (k, v) in s.s.iter().enumerate() {
                self.set_sw(2 * k, *v);
            }
            return false;
        }
        let mut j = 0;
        for ch in 0..4 {
            self.w(hw, 2 * 27);
            if d0 & 1 << ch != 0 {
                hw.per(ch, self.sw(2 * j));
                j += 1;
            }
        }
        let bounce = |p: u16, d: u16, lo: u16, hi: u16| {
            let (mut p, mut d) = (p.wrapping_add(d) as i16, d as i16);
            if p < lo as i16 {
                p = lo as i16;
                d = d.wrapping_neg();
            }
            if p > hi as i16 {
                p = hi as i16;
                d = d.wrapping_neg();
            }
            (p as u16, d as u16)
        };
        for (o, lo, hi) in [(0, s.lim[0], s.lim[1]), (8, s.lim[0], s.lim[1]), (4, s.lim[2], s.lim[3]), (12, s.lim[2], s.lim[3])] {
            let (p, d) = bounce(self.sw(o), self.sw(o + 2), lo, hi);
            self.set_sw(o, p);
            self.set_sw(o + 2, d);
        }
        false
    }

    // ---- families C and D

    fn env(&self, s: &Seq, n: u16, i: u16) -> u16 {
        self.iw(s.env + 512 * n as usize + 2 * i as usize)
    }

    /// The slot's channel and DMA bit.
    fn chan(s: &Seq, slot: u16, d0: u16) -> (usize, u16) {
        if s.four {
            let ch = (slot & 3) as usize;
            (ch, 1 << ch)
        } else if d0 == 3 {
            let ch = (slot & 1) as usize;
            (ch, 1 << ch)
        } else {
            let ch = 2 + (slot & 1) as usize;
            (ch, 1 << ch)
        }
    }

    fn seq(&mut self, s: &Seq, start: bool, d0: u16, hw: &mut Hw) -> bool {
        let slots = if s.four { 4 } else { 2 };
        self.in_first = start;
        self.nstart = 0;
        if s.four {
            self.set_sw(0x16, 0);
        }
        if start {
            self.cold = 0;
            for k in 0..8 {
                self.set_sw(2 * k, 0);
            }
            for x in 0..slots {
                self.set_sb(0x10 + x, 0xFF);
            }
            self.w(hw, if s.four { D_START[s.dma_off_first as usize] } else { 2 * 108 - 1 });
            if s.four {
                for ch in 0..4 {
                    hw.vol(ch, 0);
                    self.w(hw, 2 * 10);
                }
                hw.dmacon(0x800F);
            } else {
                hw.dmacon(d0 & 0xF);
            }
        }
        for k in 0..4 {
            for _guard in 0..4096 {
                let e = s.tracks[k] + 16 * self.sw(2 * k) as usize;
                let time = self.iw(e);
                if (self.sw(8 + 2 * k) as i16) < time as i16 {
                    if s.four {
                        self.w(hw, D_FAIL[s.dma_off_first as usize][k]);
                    }
                    break;
                }
                let padd = self.iw(e + 2);
                if padd == 0xFFFF {
                    let a = &s.ends[k];
                    for &o in &a.clears {
                        self.set_sw(o as usize, 0);
                    }
                    if !a.looping {
                        return true;
                    }
                    if !s.four {
                        self.w(hw, 2 * LOOP_END - if k == 0 { TRACK0_LESS } else { 0 });
                    }
                    continue;
                }
                let (dur, slot, inst) = (self.iw(e + 4), self.iw(e + 6), self.iw(e + 8));
                self.track = k;
                self.stop(s, slot, d0, hw);
                self.start(s, inst, dur, padd, slot, d0, hw);
                self.nstart = self.nstart.saturating_add(1);
                self.set_sw(2 * k, self.sw(2 * k).wrapping_add(1));
            }
        }
        for x in 0..slots {
            let n = self.a0[0x10 + x];
            if (n as i8) >= 0 {
                self.update(s, n as u16, d0, hw);
            } else if s.four {
                self.w(hw, D_IDLE);
            }
        }
        for k in 0..4 {
            self.set_sw(8 + 2 * k, self.sw(8 + 2 * k).wrapping_add(1));
        }
        if s.four {
            self.set_sw(0x14, self.sw(8) >> 7);
        }
        false
    }

    fn stop(&mut self, s: &Seq, slot: u16, d0: u16, hw: &mut Hw) {
        let x = (slot & if s.four { 3 } else { 1 }) as usize;
        if (self.a0[0x10 + x] as i8) < 0 {
            return;
        }
        if !s.four {
            self.w(hw, if self.nstart > 0 && !self.in_first { 2 * (T_STOP + SECOND_STOP) - 1 } else { 2 * T_STOP - 2 });
            hw.dmacon(((1 << x) | (1 << (x + 2))) & d0);
            self.w(hw, 2 * OFF_TO_START + 1);
        } else {
            self.w(hw, D_STOP_ACTIVE);
        }
        self.a0[0x10 + x] = 0xFF;
    }

    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, s: &Seq, n: u16, dur: u16, padd: u16, slot: u16, d0: u16, hw: &mut Hw) {
        self.set_sb(0x10 + slot as usize, n as u8);
        let r = s.inst + 32 * n as usize;
        self.set_iw(r + 0x1A, slot, hw);
        self.set_iw(r + 0x04, dur, hw);
        self.set_iw(r + 0x0A, padd, hw);
        self.set_iw(r + 0x02, 0, hw);
        self.set_iw(r + 0x0E, 0, hw);
        self.set_iw(r + 0x06, 0, hw);
        let (ch, bit) = Self::chan(s, slot, d0);
        let sample = self.a1().wrapping_add(self.iw(r + 0x14) as i16 as u32);
        if let Some(o) = s.clear_at {
            self.clear4(sample.wrapping_add(o as u32), hw);
            self.w(hw, 2 * 12);
        }
        let cold = self.in_first && !s.four;
        let m = s.dma_off_first as usize;
        self.w(hw, match (cold, self.cold) {
            (true, 0) => 2 * COLD_START[0],
            (true, 1) => 2 * COLD_START[1],
            _ if s.four => D_LEAD[m] + if self.in_first && self.nstart == 0 { D_FIRST_EXTRA[m] } else { 0 },
            _ => 2 * T_GROUP,
        });
        if s.dma_off_first {
            hw.dmacon(bit);
            self.w(hw, if s.four { D_OFF_TO_LOC } else { 2 * 134 });
        }
        hw.loc(ch, sample);
        self.w(hw, 2 * 16);
        hw.len(ch, self.iw(r + 0x18) >> 1);
        self.w(hw, 2 * 40);
        let (pe, ve) = (self.iw(r + 0x0C), self.iw(r));
        hw.per(ch, self.env(s, pe, 0).wrapping_add(padd));
        self.w(hw, 2 * 30);
        hw.vol(ch, self.env(s, ve, 0));
        self.w(hw, if s.four { D_VOL_TO_ON[m] } else { 2 * 44 - 1 });
        hw.dmacon(0x8000 | bit);
        self.w(hw, if cold && self.cold == 1 {
            2 * COLD_AFTER + 2
        } else if s.four {
            D_AFTER[m] - if self.track == 0 { TRACK0_LESS } else { 0 }
        } else if !self.in_first && self.nstart > 0 {
            2 * (AFTER_START - SECOND_AFTER) + 2
        } else {
            2 * AFTER_START + 1
        });
        if cold {
            self.cold = self.cold.saturating_add(1);
        }
    }

    fn update(&mut self, s: &Seq, n: u16, d0: u16, hw: &mut Hw) {
        let r = s.inst + 32 * n as usize;
        let vi = self.iw(r + 0x02);
        let m = s.dma_off_first as usize;
        if s.four && vi == 0 {
            self.set_iw(r + 0x02, 1, hw);
            self.w(hw, D_JUST);
            return;
        }
        let slot = self.iw(r + 0x1A);
        let (ch, _) = Self::chan(s, slot, d0);
        let lp = self.a1().wrapping_add(self.iw(r + 0x16) as i16 as u32);
        let len = self.iw(r + 0x10) >> 1;
        self.w(hw, if s.four { D_UPD_PRE } else { 2 * 233 - 1 });
        hw.loc(ch, lp);
        self.w(hw, 2 * 16);
        hw.len(ch, len);
        if s.four && len == 1 {
            self.clear4(lp, hw);
        }
        self.w(hw, if s.four { 2 * (71 + 10 * (len == 1) as u64) } else { 2 * 63 - 1 });
        let (pe, ve, pi, padd) = (self.iw(r + 0x0C), self.iw(r), self.iw(r + 0x0E), self.iw(r + 0x0A));
        hw.per(ch, self.env(s, pe, pi).wrapping_add(padd));
        self.w(hw, 2 * 42);
        let v = self.env(s, ve, vi);
        hw.vol(ch, v);
        let mut extra = 0;
        let inc = |x: u16| if x.wrapping_add(1) == 0xFF { 0xFE } else { x.wrapping_add(1) };
        self.set_iw(r + 0x02, inc(vi), hw);
        self.set_iw(r + 0x0E, inc(pi), hw);
        let phase = self.iw(r + 0x06);
        if phase == 0 {
            let dur = self.iw(r + 0x04).wrapping_sub(1);
            self.set_iw(r + 0x04, dur, hw);
            if dur as i16 <= 0 {
                if !s.four {
                    extra += DUR_END;
                }
                if s.four {
                    self.w(hw, D_VOL_TO_OFF[m]);
                    hw.dmacon(1 << (slot & 3));
                    self.set_sb(0x10 + (slot & 3) as usize, 0xFF);
                    self.w(hw, D_OFF_POST);
                    return;
                } else {
                    self.set_iw(r + 0x06, 1, hw);
                    self.set_iw(r + 0x02, 0, hw);
                    self.set_iw(r + 0x0E, 0, hw);
                }
            }
        } else if phase == 1 && v as i16 <= 0 {
            if !s.four {
                extra += REL_OFF;
            }
            // indexed by the instrument number, not the slot (the original's slip)
            self.set_sb(0x10 + n as usize, 0xFF);
        }
        if !s.four && vi == 0xFE && pi == 0xFE {
            extra += SATURATED;
        }
        // C: a release still sounding takes a cycle less than the other paths (the update's
        // wait before its first write has the other cycle of the 233 colour clocks)
        self.w(hw, if s.four { D_UPD_POST } else { 2 * extra + if phase == 1 && v as i16 > 0 { 0 } else { 2 } });
    }
}

/// Colour clocks the original's start-up takes before its timer starts: the song's sound counts
/// from here (what a render keeps of Paula's output), and the first tick follows later.
const INIT_CLOCKS: u64 = 190;
/// `INIT_CLOCKS` per family, relative to the song's start (where the engine aligns Paula's
/// output and line grids): the original's song starts this much before the first tick's
/// fitted time (the sweeps and the siren, the plain sequencers and Zak's, Maniac Mansion's:
/// 18, 35 and 23 colour clocks before).
fn init_clocks(fam: &Family) -> u64 {
    match fam {
        Family::Sweep(_) | Family::Siren(_) => INIT_CLOCKS - 18,
        Family::Seq(Seq { four: true, dma_off_first: true, .. }) => INIT_CLOCKS - 23,
        Family::Seq(_) => INIT_CLOCKS - 35,
    }
}
/// From the start of the timer to the first tick: the first call's entry, in a line, from the
/// write logs (the sweeps' first call, with all four channels on at once, enters later).
const FIRST_DELAY: u64 = 201;
const SWEEP_FIRST_DELAY: u64 = 211;
/// The second tick follows the first by a period and this much (the sweeps': less).
const FIRST_PERIOD: u64 = 61;
const SWEEP_FIRST_PERIOD: u64 = 36;
/// The sirens': the first call enters a little later and the second tick follows a plain period.
const SIREN_FIRST_DELAY: u64 = FIRST_DELAY + 6;
const SIREN_FIRST_PERIOD: u64 = 0;
/// The Zak title's second tick follows the first by less.
const FIRST_PERIOD_ZAK: u64 = 30;
/// A sweep's start: extra wait between the all-channels DMA write and the first voice's writes.
const SWEEP_AFTER_DMA: u64 = 14;
/// The sweep loop's wait before the third channel is a colour clock longer than the others'
/// (the voices 2 and 3 write one later in their ticks, in every tick).
const SWEEP_CH2_WAIT: u64 = 28;

impl Replayer for Scumm {
    fn format(&self) -> &'static str {
        "SCUMM"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        if !self.armed.replace(true) {
            // the engine's first question: the time from the song's start to the first tick
            // (a period on top of `INIT_CLOCKS`, which `new` has taken already)
            let first = match self.fam {
                Family::Sweep(_) => SWEEP_FIRST_DELAY,
                Family::Siren(_) => SIREN_FIRST_DELAY,
                _ => FIRST_DELAY,
            };
            return super::DT_TICK + first - init_clocks(&self.fam);
        }
        if self.first {
            super::DT_TICK
                + match self.fam {
                    Family::Sweep(_) => SWEEP_FIRST_PERIOD,
                    Family::Siren(_) => SIREN_FIRST_PERIOD,
                    Family::Seq(Seq { four: true, dma_off_first: false, .. }) => FIRST_PERIOD_ZAK,
                    _ => FIRST_PERIOD,
                }
        } else {
            super::DT_TICK
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        // the player's commands: restart on the first call, then the channel mask
        let cmd = if std::mem::replace(&mut self.first, false) { 0xFFFF_FFFF } else { 0x0000_000F };
        if self.call(cmd, hw) {
            self.songend = true;
        }
    }

    fn songend(&self) -> bool {
        self.songend
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let fam = family(d)?;
    let title = m.bus.read_cstr(m.deli.file_buf);
    // DeliTracker leaves the module's path in guest memory; a sweep's voices run through low
    // memory (their DMA starts before the loop registers are written, with a length of 64K
    // words), so the text is heard, as in the original's run
    let full = m.bus.read_cstr(m.deli.dir_buf) + &title;
    m.bus.write_cstr(m.deli.path_buf, &full);
    // the original's start-up runs this long before the timer starts: the sound of the song
    // counts from its end (the timer's delay below is that much shorter)
    m.bus.clock += init_clocks(&fam);
    let p = Scumm { fam, img: d.to_vec(), base, title, a0: [0; SCRATCH], first: true, armed: std::cell::Cell::new(false), songend: false, cold: 0, in_first: false, nstart: 0, t0: 0, cyc: 0, track: 0 };
    Some(Box::new(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put(d: &mut [u8], o: usize, words: &[u16]) {
        for (i, w) in words.iter().enumerate() {
            d[o + 2 * i..o + 2 * i + 2].copy_from_slice(&w.to_be_bytes());
        }
    }

    /// A family A routine (the signatures of the specification): P0 0x100, volume
    /// p - 0x50, base -4 every fourth call, ends at base <= 0xF0; the wave at 0x40.
    fn sweep_module() -> Vec<u8> {
        let mut d = vec![0u8; 0x60];
        put(&mut d, 0, &[0x5C, 0x3C]);
        put(&mut d, 4, &[0x48E7, 0x7FFE, 0x0880, 0x0004, 0x6758, 0x30BC]);
        put(&mut d, 0x10, &[0x0644, 0x0100]);
        put(&mut d, 0x14, &[0xE94C, 0xD868, 0x0002, 0x3A04, 0x0445, 0x0050]);
        put(&mut d, 0x20, &[0x0250, 0x0003, 0x660C, 0x5968, 0x0002, 0x0C68, 0x00F0, 0x0002, 0x6F0A]);
        d
    }

    /// A family C routine: instruments at 0x100, envelopes at 0x140, four tracks
    /// (0x540, +0x20, +0x10, +0x10); track 0 stops at its end, tracks 1-3 reset and
    /// loop. Instrument 0: volume envelope 0 (40, 30, 20, 0), pitch envelope 1 (400,
    /// 410, ...), attack sample at a1+0x500 (16 bytes), loop at a1+0x508 (8 bytes).
    /// Track 0: the note at time 0 (period offset 2, 2 ticks long), its end at 5.
    fn seq_module() -> Vec<u8> {
        let mut d = vec![0u8; 0x620];
        put(&mut d, 0, &[0x0620, 0x00FC]);
        put(&mut d, 4, &[0x48E7, 0x7FFE, 0x244B, 0x2A4B]);
        put(&mut d, 0x0C, &[0xD5FC, 0, 0x00FC, 0xDBFC, 0, 0x013C]);
        put(&mut d, 0x18, &[0xD7FC, 0, 0x053C, 0xD7FC, 0, 0x0020, 0xD7FC, 0, 0x0010, 0xD7FC, 0, 0x0010]);
        put(&mut d, 0x30, &[0x0C6C, 0xFFFF, 0x0002, 0x6602, 0x265F]);
        put(&mut d, 0x3A, &[0x0C6C, 0xFFFF, 0x0002, 0x6608, 0x4268, 0x0002, 0x4268, 0x000A, 0x60F0]);
        put(&mut d, 0x4C, &[0x0C6C, 0xFFFF, 0x0002, 0x6608, 0x4268, 0x0004, 0x4268, 0x000C, 0x60F0]);
        put(&mut d, 0x5E, &[0x0C6C, 0xFFFF, 0x0002, 0x6608, 0x4268, 0x0006, 0x4268, 0x000E, 0x60F0]);
        // instrument 0
        put(&mut d, 0x100, &[0]);
        put(&mut d, 0x10C, &[1]);
        put(&mut d, 0x110, &[8]);
        put(&mut d, 0x114, &[0x500, 0x508, 16]);
        put(&mut d, 0x140, &[40, 30, 20, 0]);
        let pitch: Vec<u16> = (0..256).map(|i| 400 + 10 * i).collect();
        put(&mut d, 0x340, &pitch);
        put(&mut d, 0x540, &[0, 2, 2, 0, 0, 0, 0, 0, 5, 0xFFFF]);
        for t in [0x560, 0x570, 0x580] {
            put(&mut d, t, &[0x7FFF, 0, 1, 1, 0]);
        }
        d
    }

    fn play(d: Vec<u8>, ticks: usize) -> (Engine, Vec<(u32, u16, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/scumm-test/scumm.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "SCUMM");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (e, log)
    }

    #[test]
    fn families_and_parameters() {
        let Some(Family::Sweep(s)) = family(&sweep_module()) else { panic!() };
        assert_eq!(s, Sweep { p0: 0x100, vol: Vol::Minus(0x50), step: -4, lim: 0xF0, le: true });
        let Some(Family::Seq(s)) = family(&seq_module()) else { panic!() };
        assert_eq!((s.four, s.inst, s.env, s.tracks), (false, 0x100, 0x140, [0x540, 0x560, 0x570, 0x580]));
        assert_eq!(s.ends[0], EndAction { clears: vec![], looping: false });
        assert_eq!(s.ends[3], EndAction { clears: vec![6, 0xE], looping: true });
        // not a routine, an unknown end action, a missing parameter
        let mut d = seq_module();
        put(&mut d, 4, &[0x48E7, 0x7FFF]);
        assert!(!detect(&d));
        let mut d = seq_module();
        put(&mut d, 0x38, &[0x4E75]);
        assert!(!detect(&d));
        let mut d = sweep_module();
        put(&mut d, 0x10, &[0x4E71]);
        assert!(!detect(&d));
        assert!(!detect(&[0u8; 0x100]));
    }

    fn per_writes(log: &[(u32, u16, u64)], reg: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == reg).map(|w| w.1).collect()
    }

    #[test]
    fn sweep_steps_through_the_channels() {
        let (e, log) = play(sweep_module(), 18);
        let dma: Vec<u16> = per_writes(&log, 0xDFF096);
        assert_eq!(dma[..2], [0xFFEF, 0xFFEF]);
        // a channel a call, period (cnt << 4) + base, the base 4 lower every 4 calls
        let pers: Vec<(u32, u16)> = log.iter().filter(|w| [0xDFF0A6, 0xDFF0B6, 0xDFF0C6, 0xDFF0D6].contains(&w.0)).map(|w| (w.0, w.1)).collect();
        assert_eq!(pers[..5], [(0xDFF0A6, 0x100), (0xDFF0B6, 0x110), (0xDFF0C6, 0x120), (0xDFF0D6, 0x130), (0xDFF0A6, 0xFC)]);
        assert_eq!(per_writes(&log, 0xDFF0A8)[..2], [0x3F, 0x3F]);
        assert_eq!(per_writes(&log, 0xDFF0A4)[0], 16);
        // the base reaches 0xF0 in the 16th call: DMA off, the end
        assert_eq!(*dma.last().unwrap(), 0x000F);
        assert!(e.songend());
    }

    #[test]
    fn sequencer_plays_a_note_and_stops() {
        let (e, log) = play(seq_module(), 8);
        assert_eq!(per_writes(&log, 0xDFF096), [0x000F, 0x8004]);
        // the note start, then an update a call: loop sample, envelopes from index 0;
        // after 2 calls the release restarts them
        assert_eq!(per_writes(&log, 0xDFF0C4), [8, 4, 4, 4, 4, 4]);
        assert_eq!(per_writes(&log, 0xDFF0C6), [402, 402, 412, 402, 412, 422]);
        assert_eq!(per_writes(&log, 0xDFF0C8), [40, 40, 30, 40, 30, 20]);
        let lo = per_writes(&log, 0xDFF0C2);
        assert_eq!(lo[1].wrapping_sub(lo[0]), 8);
        // the call at time 5 meets track 0's end: it returns C=1 and writes nothing
        assert!(e.songend());
        assert!(per_writes(&log, 0xDFF0D6).is_empty());
    }
}
