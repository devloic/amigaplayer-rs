//! Mike Davies (`md.*`), the sound driver Mike Davies wrote for the US Gold / Tiertex
//! conversions of 1989-91 (Strider, Strider II, Mercs, Alien Storm, Die Hard 2, Dynasty
//! Wars, Indiana Jones 3 Action, Italy 1990 Winners Edition, World Class Soccer).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Mike Davies player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A `md.*` file is a memory image cut from the game: the game's driver code, its
//! variables, the song, instrument and period tables, the tracks and the samples,
//! with absolute pointers to the game's load address. The code is not run. It is
//! read only to find the data, by generic instruction signposts (the driver's
//! "playing" flag store gives the game address, the address loads and compares
//! before and after the play routine give the song, effect and instrument tables
//! and the period table), and to tell the four builds apart (track encoding, rest
//! flag, low-period clamp or skip, transposition of note 0, passes per tick). The
//! data is then prepared as the reference host does (the pointers converted to the
//! image's address, the instrument tables of Italy 1990 / World Class Soccer and of
//! Mercs repaired) and the engine (four voices, a main track and a called pattern
//! each, loops, instruments, 50 Hz) is written from its behaviour.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Mike Davies", detect, new };

/// The address at which the reference host places the image. The prepared data holds
/// absolute pointers to it, and the one read past the period table (Alien Storm song
/// 5) reads such a pointer, so the port prepares the image for this address, as the
/// specification's runs had it, and converts to the guest address only when writing
/// Paula's location registers.
const VIRT: u32 = 0xF000;

/// Track elements (the first unit).
const END_PATTERN: u16 = 0x94;
const CALL: u16 = 0x95;
const LOOP_END: u16 = 0x9D;
const LOOP_START: u16 = 0x9E;
const RESTART: u16 = 0xFF;
const STOP: u16 = 0xFE;

/// The "end words" the image-extent search of the preparation stops at.
const END_WORDS: [u16; 5] = [0x0094, 0x00FE, 0x00FF, 0x01FE, 0x01FF];

/// The waits between the output routine's write groups, in colour clocks: the
/// reference waits for 9 raster line changes, and its gaps from the last volume
/// write to the first DMA-on write and from the last DMA-on write to the first
/// loop write are 2,094 and 2,018 (medians, Die Hard 2 song 1).
const WAIT_ON: i64 = 4400;
const WAIT_LOOP: i64 = 4150;
/// From the period write to the volume write of a voice, in cycles (the mean of the two builds' paths).
const PER_VOL: [i64; 4] = [481, 504, 527, 517];
/// The cost in cycles of the sequencer paths (fitted), by path counter.
const COST: [(usize, i64); 6] = [(0, 172), (1, 232), (2, 324), (3, 167), (19, 403), (21, 15)];

/// The instrument record and the instrument count's step.
const REC: usize = 0x18;

// ---------------------------------------------------------------- recognition

fn w(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

/// The first even offset at or after `from` where the words `pat` stand.
fn find(d: &[u8], from: usize, pat: &[u16]) -> Option<usize> {
    let mut o = from;
    while o + 2 * pat.len() <= d.len() {
        if pat.iter().enumerate().all(|(i, &p)| w(d, o + 2 * i) == p) {
            return Some(o);
        }
        o += 2;
    }
    None
}

/// The Eagleplayer's test (specification, section 10).
fn detect(d: &[u8]) -> bool {
    let a = if w(d, 0xF2) == 0x0C40 {
        0xF2
    } else if w(d, 0xEC) == 0x33C0 {
        0xEC
    } else {
        return false;
    };
    if be32(d, a - 4) != 0x7FFF_4E75 || be32(d, a - 8) != 0x6106_4CDF || be32(d, a - 12) != 0x48E7_FFFE {
        return false;
    }
    let Some(b) = find(d, a - 12, &[0x6000]) else { return false };
    let disp = w(d, b + 2) as i16;
    disp > 0 && disp % 2 == 0 && w(d, b + 2 + disp as usize) == 0x2D58
}

// ------------------------------------------------------------------ the layout

/// What the image's driver code says (file offsets).
#[derive(Clone, Debug, PartialEq)]
struct Lay {
    g: u32,
    songs: usize,
    songs_end: usize,
    fx: usize,
    fx_end: usize,
    ins: usize,
    /// The instrument table's end as the driver has it (before the count fix).
    ins_end: usize,
    periods: usize,
    call: u16,
    /// Byte-coded tracks (A2) rather than word-coded.
    bytes: bool,
    /// The instrument loader sets the note to 1, and note 0 is a rest (A1, A2).
    rest: bool,
    /// A period below `thr` is raised to it (A1, A2), else the voice is skipped.
    clamp: bool,
    thr: u16,
    /// Note 0 is left untransposed by the instrument as well (A1, A2).
    zero_both: bool,
    passes: usize,
}

fn operand(d: &[u8], at: usize, g: u32) -> Option<usize> {
    let p = be32(d, at + 2).wrapping_sub(g) as usize;
    (p < d.len()).then_some(p)
}

/// Finds the tables and the build (specification, sections 2 and 8).
fn layout(d: &[u8]) -> Option<Lay> {
    let e = find(d, 0, &[0x48E7, 0xFFFE])?;
    let a = find(d, e + 4, &[0x33FC, 0x0001])?;
    let g = be32(d, a + 4);
    let vb = find(d, a, &[0x4DF9])?;
    operand(d, vb, g)?;
    let s = find(d, a, &[0x41F9])?;
    let se = find(d, s + 6, &[0xB1FC])?;
    let x = find(d, se + 6, &[0x41F9])?;
    let xe = find(d, x + 6, &[0xB1FC])?;
    let (songs, songs_end, fx, fx_end) = (operand(d, s, g)?, operand(d, se, g)?, operand(d, x, g)?, operand(d, xe, g)?);
    let delay = find(d, xe + 6, &[0x303C])?;
    let mut p = delay + 4;
    let p = loop {
        p = find(d, p, &[0x48E7, 0xFFFE])?;
        if w(d, p + 8) == 0x6100 {
            break p;
        }
        p += 2;
    };
    // the play routine's list of word-sized calls
    let mut targets = Vec::new();
    let mut o = p + 4;
    while w(d, o) == 0x6100 {
        targets.push((o + 2).wrapping_add(w(d, o + 2) as i16 as isize as usize));
        o += 4;
    }
    let seq = *targets.first()?;
    let passes = targets.iter().filter(|&&t| t == seq).count();
    let i = find(d, p + 4, &[0x41F9])?;
    let ie = find(d, i + 6, &[0xB1FC])?;
    let t = find(d, ie + 6, &[0x4BF9])?;
    let (ins, ins_end, periods) = (operand(d, i, g)?, operand(d, ie, g)?, operand(d, t, g)?);
    let c = find(d, p + 4, &[0x0C40])?;
    let call = w(d, c + 2 + 0x16);
    // build properties
    let tf = find(d, e, &[0xD0ED, 0x001C])?;
    let bytes = w(d, tf + 4) == 0x7000;
    let mut rest = false;
    let mut r = e;
    while let Some(o) = find(d, r, &[0x3D7C, 0x0001, 0x0008]) {
        if o >= 4 && w(d, o - 2) == 0x000C && w(d, o - 4) & 0xFFF8 == 0x3D40 {
            rest = [0x3D7C, 0x0001, 0x0034].iter().enumerate().all(|(k, &v)| w(d, o + 6 + 2 * k) == v);
            break;
        }
        r = o + 2;
    }
    let mut lp = e;
    let (thr, clamp) = loop {
        let o = find(d, lp, &[0x0C6E])?;
        if w(d, o + 4) == 0x0004 {
            match w(d, o + 10) {
                0x33EE => break (w(d, o + 2), true),
                0x426E => break (w(d, o + 2), false),
                _ => {}
            }
        }
        lp = o + 2;
    };
    let n0 = find(d, seq, &[0x4A40])?;
    let zero_both = match (w(d, n0 + 2), w(d, n0 + 4)) {
        (0x6700, 10) | (0x670A, _) => true,
        (0x6700, 6) | (0x6706, _) => false,
        _ => return None,
    };
    if songs_end < songs || fx_end < fx || ins_end <= ins {
        return None;
    }
    Some(Lay { g, songs, songs_end, fx, fx_end, ins, ins_end, periods, call, bytes, rest, clamp, thr, zero_both, passes })
}

// ----------------------------------------------------------------- preparation

fn put32(d: &mut [u8], o: usize, v: u32) -> Option<()> {
    d.get_mut(o..o + 4)?.copy_from_slice(&v.to_be_bytes());
    Some(())
}

/// Prepares a copy of the image for address `VIRT` (specification, section 3):
/// the image and the instrument count.
fn prepare(d: &[u8], lay: &Lay) -> Option<(Vec<u8>, usize)> {
    let mut img = d.to_vec();
    let conv = |p: u32| p.wrapping_sub(lay.g).wrapping_add(VIRT);
    // 1. the instrument count
    let n = lay.ins_end - lay.ins;
    let count = if n % REC == 0 {
        n / REC
    } else if (n + 4) % REC == 0 {
        if img.get(0x7D8..0x7DC) == Some(b"FTD<") {
            // the missing loop field of the next-to-last record (Italy 1990, World Class Soccer)
            if img.len() < 0x774 {
                return None;
            }
            img.copy_within(0x764..0x770, 0x768);
            put32(&mut img, 0x764, 0)?;
        }
        (n + 4) / REC
    } else {
        return None;
    };
    // 2. the sample pointers
    let recs = (0..count).map(|k| lay.ins + REC * k).chain((0..(lay.fx_end - lay.fx) / REC).map(|k| lay.fx + REC * k));
    let (mut d2, mut d5) = (i32::MAX, None::<i32>);
    for o in recs.collect::<Vec<_>>() {
        if o + 8 > img.len() {
            return None;
        }
        let p = conv(be32(&img, o));
        put32(&mut img, o, p)?;
        d2 = d2.min(p as i32);
        let end = p.wrapping_add(be32(&img, o + 4).wrapping_mul(2)) as i32;
        match d5 {
            None => d5 = Some(end),
            Some(e) if p as i32 >= e => d5 = Some(end),
            _ => {}
        }
    }
    let d5 = d5?;
    // 3. the song table
    let mut d3 = i32::MIN;
    let mut o = lay.songs;
    while o < lay.songs_end {
        let v = be32(&img, o);
        if v != 0xFFFF_FFFF {
            let c = conv(v);
            put32(&mut img, o, c)?;
            d3 = d3.max(c as i32);
        }
        o += 4;
    }
    // 4. the call-pattern pointers in the tracks
    let off = |a: i32| (a as u32).wrapping_sub(VIRT) as usize;
    let scan = |img: &mut Vec<u8>, from: usize, to: usize, d3: &mut i32| -> Option<()> {
        let mut o = from;
        while o < to {
            if o + 2 > img.len() {
                return None;
            }
            if w(img, o) == lay.call {
                let c = conv(be32(img, o + 6));
                put32(img, o + 6, c)?;
                *d3 = (*d3).max(c as i32);
                o += 8;
            } else {
                o += 2;
            }
        }
        Some(())
    };
    let after_end_word = |img: &[u8], from: usize| -> Option<usize> {
        let mut o = from;
        while o + 2 <= img.len() {
            if END_WORDS.contains(&w(img, o)) {
                return Some(o + 2);
            }
            o += 2;
        }
        None
    };
    let size = if d3 < d5 {
        let mut dummy = d3;
        scan(&mut img, lay.fx_end, off(d2), &mut dummy)?;
        off(d5)
    } else {
        let mut a2 = after_end_word(&img, off(d3))?;
        let mut hi = d3;
        scan(&mut img, off(d5), a2, &mut hi)?;
        if (VIRT as usize + a2) as i32 <= hi {
            a2 = after_end_word(&img, off(hi))?;
        }
        a2
    };
    if size > d.len() {
        return None;
    }
    // 6. Mercs: two stop commands, and the last instrument's missing period field
    if size == 0x77AE {
        put32(&mut img, 0x7318, 0x00FE_00FE)?;
        if img.len() < 0x874 {
            return None;
        }
        img.copy_within(0x864..0x870, 0x868);
        put32(&mut img, 0x864, 488)?;
    }
    Some((img, count))
}

// -------------------------------------------------------------------- engine

#[derive(Clone, Copy, Default)]
struct Ctx {
    ptr: u32,
    idx: u16,
    lcnt: u16,
    lret: u16,
    tr: u16,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    per: u16,
    vol: u16,
    len: u16,
    lp: u16,
    smp: u32,
    trig: bool,
    dur: u16,
    prep: u16,
    itr: u16,
    note: u16,
    main: Ctx,
    pat: Ctx,
}

/// What reading a voice's next note ended with.
enum Next {
    Note(u16, u16),
    Restart,
}

pub struct MikeDavies {
    lay: Lay,
    img: Vec<u8>,
    /// Where file offset 0 lies in guest memory.
    base: u32,
    songs: u16,
    sub: u16,
    started: bool,
    songend: bool,
    playing: bool,
    song: u16,
    v: [Voice; 4],
    tl: [u32; 64],
}

impl MikeDavies {
    fn t(&mut self, id: usize) {
        self.tl[id] += 1;
    }

    /// The prepared image's byte at address `a` (VIRT-based); 0 outside it.
    fn b(&self, a: u32) -> u8 {
        let o = a.wrapping_sub(VIRT) as usize;
        self.img.get(o).copied().unwrap_or(0)
    }

    fn w(&self, a: u32) -> u16 {
        (self.b(a) as u16) << 8 | self.b(a.wrapping_add(1)) as u16
    }

    fn l(&self, a: u32) -> u32 {
        (self.w(a) as u32) << 16 | self.w(a.wrapping_add(2)) as u32
    }

    /// Unit `k` of the element at `a`.
    fn unit(&self, a: u32, k: u32) -> u16 {
        if self.lay.bytes {
            self.b(a.wrapping_add(k)) as u16
        } else {
            self.w(a.wrapping_add(2 * k))
        }
    }

    /// An element's size in the word and the byte encoding.
    fn size(&self, word: u16, byte: u16) -> u16 {
        if self.lay.bytes {
            byte
        } else {
            word
        }
    }

    fn stop(&mut self, hw: &mut Hw) {
        for v in &mut self.v {
            v.main.ptr = 0xFFFF_FFFF;
            v.trig = false;
            v.main.idx = 0;
            v.main.lcnt = 0;
            v.pat.ptr = 0;
            v.pat.idx = 0;
            v.pat.lcnt = 0;
            v.main.tr = 0;
        }
        self.playing = false;
        for ch in 0..4 {
            hw.vol(ch, 0);
        }
        hw.dmacon(0x000F);
    }

    fn start_song(&mut self, n: u16, hw: &mut Hw) {
        self.song = n;
        self.stop(hw);
        if n < 1 {
            return;
        }
        self.playing = true;
        hw.bus.write_word(0xDFF09E, 0x00FF);
        let e = self.lay.songs + 16 * (n as usize - 1);
        if e >= self.lay.songs_end {
            self.stop(hw);
            return;
        }
        for (k, v) in self.v.iter_mut().enumerate() {
            v.main.ptr = be32(&self.img, e + 4 * k);
            v.dur = 1;
        }
    }

    /// Reads voice `vi`'s next note (specification, 5.1): the note and its duration.
    fn next(&mut self, vi: usize, hw: &mut Hw) -> Next {
        for _ in 0..100_000 {
            let in_pat = self.v[vi].pat.ptr != 0;
            let c = if in_pat { self.v[vi].pat } else { self.v[vi].main };
            let a = c.ptr.wrapping_add(c.idx as i16 as i32 as u32);
            let x = self.unit(a, 0);
            let mut c2 = c;
            self.t(3);
            match x {
                0..=0x7F => {
                    self.t(10);
                    let dur = self.unit(a, 1);
                    c2.idx = c.idx.wrapping_add(self.size(4, 2));
                    self.set_ctx(vi, in_pat, c2);
                    return Next::Note(x, dur);
                }
                END_PATTERN => {
                    self.t(11);
                    c2.idx = 0;
                    let v = &mut self.v[vi];
                    v.prep = v.prep.wrapping_sub(1);
                    if v.prep == 0 {
                        c2.ptr = 0;
                        self.tl[12] += 1;
                    }
                    self.set_ctx(vi, in_pat, c2);
                }
                CALL => {
                    self.t(13);
                    let (count, tr, ptr) = (self.w(a.wrapping_add(2)), self.w(a.wrapping_add(4)), self.l(a.wrapping_add(6)));
                    let sz = self.size(10, 5);
                    let v = &mut self.v[vi];
                    v.prep = count;
                    v.pat.tr = tr;
                    v.pat.ptr = ptr;
                    v.main.idx = v.main.idx.wrapping_add(sz);
                }
                LOOP_END => {
                    self.t(14);
                    c2.lcnt = c.lcnt.wrapping_sub(1);
                    if c2.lcnt != 0 {
                        self.tl[15] += 1;
                    }
                    c2.idx = if c2.lcnt != 0 { c.lret } else { c.idx.wrapping_add(self.size(2, 1)) };
                    self.set_ctx(vi, in_pat, c2);
                }
                LOOP_START => {
                    self.t(16);
                    let n = self.unit(a, 1);
                    c2.idx = c.idx.wrapping_add(self.size(4, 2));
                    c2.lret = c2.idx;
                    c2.lcnt = if self.lay.bytes { c.lcnt & 0xFF00 | n } else { n };
                    self.set_ctx(vi, in_pat, c2);
                }
                RESTART => {
                    self.t(17);
                    self.songend = true;
                    self.start_song(self.song, hw);
                    return Next::Restart;
                }
                STOP => {
                    self.t(18);
                    self.songend = true;
                    self.stop(hw);
                    return Next::Restart;
                }
                _ => {
                    self.t(19);
                    let mut r = self.lay.ins + REC * (x & 0x7F) as usize;
                    if r >= self.lay.ins_end {
                        r = self.lay.ins;
                    }
                    let f = |k: usize| be16(&self.img, r + 4 * k + 2);
                    let smp = be32(&self.img, r);
                    let (len, per, vol, itr, lp) = (f(1), f(2), f(3), f(4), f(5));
                    let rest = self.lay.rest;
                    let v = &mut self.v[vi];
                    (v.smp, v.len, v.per, v.vol, v.itr, v.lp) = (smp, len, per, vol, itr, lp);
                    v.trig = true;
                    if rest {
                        v.note = 1;
                    }
                    c2.idx = c.idx.wrapping_add(self.size(2, 1));
                    self.set_ctx(vi, in_pat, c2);
                }
            }
        }
        // a track of commands that never reaches a note: the voice stops reading
        self.v[vi].main.ptr = 0xFFFF_FFFF;
        Next::Note(0, 0)
    }

    fn set_ctx(&mut self, vi: usize, in_pat: bool, c: Ctx) {
        if in_pat {
            self.v[vi].pat = c;
        } else {
            self.v[vi].main = c;
        }
    }

    /// One pass of the sequencer; true when a restart (0xFF or 0xFE) abandoned it.
    fn sequencer(&mut self, hw: &mut Hw) -> bool {
        if !self.playing {
            return false;
        }
        for vi in 0..4 {
            if self.v[vi].main.ptr & 0x8000_0000 != 0 {
                self.t(0);
                continue;
            }
            self.v[vi].dur = self.v[vi].dur.wrapping_sub(1);
            if self.v[vi].dur != 0 {
                self.t(1);
                continue;
            }
            self.t(2);
            let (mut x, dur) = match self.next(vi, hw) {
                Next::Restart => return true,
                Next::Note(x, d) => (x, d),
            };
            let v = &self.v[vi];
            let tr = if v.pat.ptr != 0 { v.pat.tr } else { v.main.tr };
            if self.lay.zero_both {
                if x != 0 {
                    x = x.wrapping_add(tr).wrapping_add(v.itr);
                }
            } else {
                if x != 0 {
                    x = x.wrapping_add(tr);
                }
                x = x.wrapping_add(v.itr);
            }
            self.t(20);
            if x != 0 {
                self.t(21);
            }
            let per = self.w((VIRT + self.lay.periods as u32).wrapping_add(x.wrapping_mul(2) as i16 as i32 as u32));
            let v = &mut self.v[vi];
            v.note = x;
            v.per = per;
            v.dur = dur;
            v.trig = true;
        }
        false
    }

    fn guest(&self, a: u32) -> u32 {
        self.base.wrapping_add(a.wrapping_sub(VIRT))
    }

    /// The output routine (specification, 6.2).
    fn output(&mut self, hw: &mut Hw) {
        // the time in 68000 cycles since the start of this pass; a write falls on the colour clock its
        // cycle is in
        let t0 = hw.bus.clock;
        let mut cy: i64 = COST.iter().map(|&(i, c)| self.tl[i] as i64 * c).sum();
        self.tl = [0; 64];
        macro_rules! at {
            () => {
                hw.bus.clock = hw.bus.clock.max(t0 + (cy.max(0) as u64) / 2)
            };
        }
        let mut wrote = false;
        let mut skipped = 0i64;
        let mut last = 0;
        for ch in 0..4 {
            if !self.v[ch].trig {
                if wrote {
                    skipped += 1;
                }
                continue;
            }
            if self.v[ch].per < self.lay.thr {
                if self.lay.clamp {
                    self.v[ch].per = self.lay.thr;
                } else {
                    self.v[ch].trig = false;
                    if wrote {
                        skipped += 1;
                    }
                    continue;
                }
            }
            let v = self.v[ch];
            if wrote {
                cy += 239 + 31 * last as i64 + 41 * skipped;
            }
            skipped = 0;
            last = ch;
            at!();
            hw.dmacon(1 << ch);
            cy += 76;
            at!();
            hw.loc(ch, self.guest(v.smp));
            cy += 20;
            at!();
            hw.len(ch, v.len);
            cy += 20;
            at!();
            hw.per(ch, v.per);
            cy += PER_VOL[ch];
            at!();
            hw.vol(ch, v.vol & 0x7F);
            wrote = true;
        }
        if !wrote {
            return;
        }
        // the original waits for the raster before it starts the voices: the mean wait is used
        cy += WAIT_ON;
        let mut first = true;
        for ch in 0..4 {
            let v = &self.v[ch];
            if v.trig && (!self.lay.rest || v.note != 0) {
                if !first {
                    cy += 85;
                }
                first = false;
                at!();
                hw.dmacon(0x8000 | 1 << ch);
            }
        }
        cy += WAIT_LOOP;
        let mut gap = 0;
        for ch in 0..4 {
            if !self.v[ch].trig {
                gap += 1;
                continue;
            }
            self.v[ch].trig = false;
            let v = self.v[ch];
            cy += 41 * gap;
            gap = 0;
            at!();
            if v.lp != 0 {
                hw.loc(ch, self.guest(v.smp.wrapping_add(v.lp as i16 as i32 as u32)));
                cy += 18;
                at!();
                hw.len(ch, (v.len.wrapping_mul(2).wrapping_sub(v.lp)) >> 1);
            } else {
                hw.loc(ch, self.guest(v.smp));
                cy += 18;
                at!();
                hw.len(ch, 2);
            }
            cy += 136;
        }
    }

    /// The play routine `passes` times; a restart begins again with the first pass.
    fn play(&mut self, hw: &mut Hw) {
        let mut restarts = 0;
        'tick: loop {
            for _ in 0..self.lay.passes {
                if self.sequencer(hw) {
                    restarts += 1;
                    if restarts > 8 {
                        // a song that restarts without ever reaching a note
                        self.stop(hw);
                    }
                    continue 'tick;
                }
                self.output(hw);
            }
            return;
        }
    }
}

impl Replayer for MikeDavies {
    fn format(&self) -> &'static str {
        "Mike Davies"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.songs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (1..=self.songs).contains(&n) {
            self.sub = n;
            self.started = false;
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            // the host's volume set-up, then the start of the song
            self.started = true;
            self.songend = false;
            for ch in 0..4 {
                hw.vol(ch, 0);
            }
            self.start_song(self.sub, hw);
        }
        self.tl = [0; 64];
        self.play(hw);
    }
}

fn new(d: &[u8], base: u32, _m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let lay = layout(d)?;
    let songs = (lay.songs_end - lay.songs) / 16;
    if songs == 0 {
        return None;
    }
    let (img, _) = prepare(d, &lay)?;
    Some(Box::new(MikeDavies {
        lay,
        img,
        base,
        songs: songs as u16,
        sub: 1,
        started: false,
        songend: false,
        playing: false,
        song: 0,
        v: Default::default(),
        tl: [0; 64],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn p16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn p32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    fn words(d: &mut [u8], o: usize, ws: &[u16]) {
        for (i, &v) in ws.iter().enumerate() {
            p16(d, o + 2 * i, v);
        }
    }

    const G: u32 = 0x2_0000;

    /// A synthetic image: the signposts the scan looks for (an A1-like build unless
    /// `b` says otherwise), one song with voice 0's track `track` at 0x340 and a
    /// pattern `pat` at 0x380, one instrument (period 0x150, volume 40, a 16-byte
    /// sample at 0x400 looping from byte 4), a period table at 0x200 of 0x100 + 2n, but 4 for note 0.
    fn module(track: &[u16], pat: &[u16], bytes: bool, rest: bool, clamp: bool, zero_both: bool) -> Vec<u8> {
        let mut d = vec![0u8; 0x420];
        words(&mut d, 0xE6, &[0x48E7, 0xFFFE, 0x6106, 0x4CDF, 0x7FFF, 0x4E75, 0x0C40, 0x0000, 0x6000, 0x0010]);
        p16(&mut d, 0x108, 0x2D58);
        words(&mut d, 0x100, &[0x33FC, 0x0001]);
        p32(&mut d, 0x104, G);
        let op = |d: &mut Vec<u8>, at: usize, code: u16, off: u32| {
            p16(d, at, code);
            p32(d, at + 2, G + off);
        };
        op(&mut d, 0x10C, 0x4DF9, 0x10);
        op(&mut d, 0x112, 0x41F9, 0x300);
        op(&mut d, 0x118, 0xB1FC, 0x310);
        op(&mut d, 0x11E, 0x41F9, 0x310);
        op(&mut d, 0x124, 0xB1FC, 0x310);
        words(&mut d, 0x12A, &[0x303C, 0x0000]);
        // the play routine: the sequencer at 0x16C, the output at 0x1A0
        words(&mut d, 0x12E, &[0x48E7, 0xFFFE, 0x6100, 0x16C - 0x134, 0x6100, 0x1A0 - 0x138, 0x4CDF, 0x7FFF, 0x4E75]);
        op(&mut d, 0x140, 0x41F9, 0x310);
        op(&mut d, 0x146, 0xB1FC, 0x328);
        op(&mut d, 0x14C, 0x4BF9, 0x200);
        words(&mut d, 0x152, &[0x0C40, 0x0000]);
        p16(&mut d, 0x16A, 0x0095);
        words(&mut d, 0x16C, &[0x4A40, 0x6700, if zero_both { 10 } else { 6 }]);
        words(&mut d, 0x172, &[0xD0ED, 0x001C, if bytes { 0x7000 } else { 0x3010 }]);
        words(&mut d, 0x178, &[0x3D40, 0x000C, 0x3D7C, 0x0001, 0x0008]);
        if rest {
            words(&mut d, 0x182, &[0x3D7C, 0x0001, 0x0034]);
        }
        words(&mut d, 0x18A, &[0x0C6E, 0x007C, 0x0004]);
        p16(&mut d, 0x194, if clamp { 0x33EE } else { 0x426E });
        p16(&mut d, 0x1A0, 0x4E75);
        for n in 1..0x40 {
            p16(&mut d, 0x200 + 2 * n, 0x100 + 2 * n as u16);
        }
        p16(&mut d, 0x200, 4);
        // the song, the instrument, the track, the pattern
        p32(&mut d, 0x300, G + 0x340);
        for k in 1..4 {
            p32(&mut d, 0x300 + 4 * k, 0xFFFF_FFFF);
        }
        let ins = [G + 0x400, 8, 0x150, 40, 0, 4];
        for (k, &v) in ins.iter().enumerate() {
            p32(&mut d, 0x310 + 4 * k, v);
        }
        let put = |d: &mut Vec<u8>, at: usize, ws: &[u16]| {
            let mut o = at;
            let mut i = 0;
            while i < ws.len() {
                if ws[i] == CALL {
                    // call: count, transpose, the pattern's game address
                    let (n, t) = (ws[i + 1], ws[i + 2]);
                    if bytes {
                        d[o] = CALL as u8;
                    } else {
                        p16(d, o, CALL);
                    }
                    p16(d, o + 2, n);
                    p16(d, o + 4, t);
                    p32(d, o + 6, G + 0x380);
                    o += if bytes { 5 } else { 10 };
                    i += 3;
                } else {
                    if bytes {
                        d[o] = ws[i] as u8;
                        o += 1;
                    } else {
                        p16(d, o, ws[i]);
                        o += 2;
                    }
                    i += 1;
                }
            }
        };
        put(&mut d, 0x340, track);
        put(&mut d, 0x380, pat);
        d
    }

    fn a1(track: &[u16], pat: &[u16]) -> Vec<u8> {
        module(track, pat, false, true, true, true)
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, bool, Vec<(u32, u16, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("md.test"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * (960 * ticks + 100)];
        e.render(&mut out);
        let end = e.songend();
        (format, end, e.m.bus.write_log.take().unwrap())
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_and_reads_the_layout() {
        let d = a1(&[0x80, 0x10, 3, 0xFF], &[0x94]);
        assert!(detect(&d));
        let lay = layout(&d).unwrap();
        assert_eq!((lay.g, lay.songs, lay.songs_end, lay.ins, lay.ins_end, lay.periods, lay.call), (G, 0x300, 0x310, 0x310, 0x328, 0x200, 0x95));
        assert!(!lay.bytes && lay.rest && lay.clamp && lay.zero_both && lay.thr == 0x7C && lay.passes == 1);
        let lay = layout(&module(&[0xFF], &[0x94], true, false, false, false)).unwrap();
        assert!(lay.bytes && !lay.rest && !lay.clamp && !lay.zero_both);
        // the branch's target must be the expected instruction
        let mut e = d.clone();
        p16(&mut e, 0x108, 0x2D59);
        assert!(!detect(&e));
        let mut e = d.clone();
        p32(&mut e, 0xEA, 0x6106_4CDE);
        assert!(!detect(&e));
        assert!(!detect(&d[..0x100]));
        assert!(!detect(b"FORM\0\0\0\0EMODEMIC"));
    }

    #[test]
    fn prepares_the_pointers_for_the_image_address() {
        let d = a1(&[0x80, CALL, 2, 1, 0, 0x10, 3, 0xFF], &[0x05, 2, 0x94]);
        let lay = layout(&d).unwrap();
        let (img, count) = prepare(&d, &lay).unwrap();
        assert_eq!(count, 1);
        assert_eq!(be32(&img, 0x310), VIRT + 0x400);
        assert_eq!(be32(&img, 0x300), VIRT + 0x340);
        assert_eq!(be32(&img, 0x304), 0xFFFF_FFFF);
        assert_eq!(be32(&img, 0x342 + 6), VIRT + 0x380);
        // a table one long short of a whole record gets one more
        let mut e = d.clone();
        p32(&mut e, 0x148, G + 0x324);
        let lay = layout(&e).unwrap();
        assert_eq!(prepare(&e, &lay).unwrap().1, 1);
        p32(&mut e, 0x148, G + 0x322);
        let lay = layout(&e).unwrap();
        assert!(prepare(&e, &lay).is_none());
    }

    #[test]
    fn plays_instrument_pattern_calls_and_restarts() {
        // instrument 0, a pattern played twice transposed by 1, a note, the restart
        let d = a1(&[0x80, CALL, 2, 1, 0x10, 3, 0xFF], &[0x05, 2, 0x94]);
        let (format, end, log) = play(d, 9);
        assert_eq!(format, "Mike Davies");
        assert!(end);
        // start-up: VOL 0 x 4, then the song's stop: VOL 0 x 4, DMACON 000F, ADKCON 00FF
        let audio: Vec<u32> = log.iter().map(|w| w.0).filter(|&a| a == 0xDFF096 || a == 0xDFF09E || (0xDFF0A0..0xDFF0E0).contains(&a)).collect();
        assert_eq!(audio[..10], [0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8, 0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8, 0xDFF096, 0xDFF09E]);
        // the pattern's note 5 transposed by 1, twice, then the track's note, then again after the restart
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per[..4], [0x100 + 2 * 6, 0x100 + 2 * 6, 0x100 + 2 * 0x10, 0x100 + 2 * 6]);
        // the sample, then its loop from byte 4 (6 words)
        let len = reg(&log, 0xDFF0A4);
        assert_eq!(len[..2], [8, 6]);
        let lc: Vec<u16> = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1].wrapping_sub(lc[0]), 4);
        assert_eq!(reg(&log, 0xDFF0A8)[..3], [0, 0, 40]);
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..3], [0x000F, 0x0001, 0x8001]);
    }

    #[test]
    fn low_periods_clamp_or_skip_and_note_zero_rests() {
        // note 0 with the rest flag: written, but its DMA stays off
        let (_, _, log) = play(a1(&[0x80, 0x00, 50, 0xFE], &[0x94]), 3);
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001) && !dma.contains(&0x8001));
        // period 4 below the threshold: clamped to 0x7C ...
        assert_eq!(reg(&log, 0xDFF0A6), [0x7C]);
        // ... or the voice skipped, with no writes at all (the A3/B builds)
        let (_, _, log) = play(module(&[0x80, 0x00, 50, 0xFE], &[0x94], false, false, false, false), 3);
        assert!(reg(&log, 0xDFF0A6).is_empty() && !reg(&log, 0xDFF096).contains(&0x0001));
    }

    #[test]
    fn byte_tracks_and_loops() {
        // a loop of 3 around one note, then the stop
        let d = module(&[0x80, LOOP_START, 3, 0x08, 1, LOOP_END, 0xFE], &[0x94], true, true, true, true);
        let (_, end, log) = play(d, 8);
        assert!(end);
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per, [0x100 + 2 * 8; 3]);
    }
}
