//! MON (`mon.*`), the music system of Maniacs of Noise / Charles Deenen (Flimbo's
//! Quest, Battle Chess II, Dragon Wars, Lord of the Rings, Mysterious Worlds,
//! Unreal, Rubicon, the Habitual Novelty demo).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the observable behaviour of
//! UADE's Maniacs of Noise player (its Paula register writes) and the structure of
//! the corpus files, and this player was written from it alone and checked against
//! those writes.
//!
//! A module is a raw 68000 image: the game's own replay routine (three entry
//! points, about 1.7 KB, not run here), a dumped RAM-state area, then the song
//! data: a period table, a song table, an instrument table, order lists, patterns,
//! envelopes, vibrato tables and samples. All pointers in the data are offsets
//! from the start of the song table. The tables are located in the image by a few
//! short structural signatures of the code (see `scan`), and the behaviour of the
//! several builds of the routine is written from the specification: build 1 (the
//! oldest), build 2 (16-byte song entries), builds 3a/3b/3c (12-byte entries, ten
//! pattern commands, sustain envelopes; 3b with a pitch envelope and a voice enable
//! mask, 3c with pitch envelope and wave-sequence instruments). Four voices, one
//! Paula channel each, 50 Hz; the song length is set by the duration multiplier.
//!
//! Departures from / decisions beyond the specification are noted where they are
//! made ("decision").

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Maniacs of Noise", detect, new };

/// Endless loops in the data (a back-jump onto itself) give up after this many steps.
const GUARD: u32 = 4096;

// The 68000 cycles of the original's code paths (see `Clk`), by label: a label names a branch of
// this player (`pat.note` a note of a pattern, `smp.start` a sample start, ...); the cycles are
// fitted, as whole numbers, to every gap between two host calls of the reference runs (the
// Eagleplayer M.O.N. of EP206 on the engine; each gap's cycles exactly, from a second run whose
// count starts one cycle late) of 34 modules (every build of the corpus, 243,172 gaps, all
// exact; 7 modules held out of a first fit came out exact too). The several code variants of
// build 3 take the same cycles; builds 1 and 2 differ on a few paths. No multiplication or
// division time depends on its operands here. Not measured: the wave-sequence instruments of
// build 3c, which EP206's player does not play (it writes no sample for them; the port plays
// them as UADE's player does), and the instruments' pitch envelope of build 3b, which it leaves
// out too (Dragon Wars' game tune from 14.7 s on; a copy given one more such instrument confirms
// it).

/// Build 3 (all its code variants alike), by code path.
const COSTS: &[(&str, i32)] = &[
    ("dur", 118), ("env.loop", 92), ("env.rel", 82), ("env.step", 275), ("env.sus", 96), ("env.wait", -21),
    ("ord", 111), ("ord.c2", 64), ("ord.c5", 124), ("ord.c6", 314), ("pat", 89), ("pat.c0", 35), ("pat.c0.rep", 18),
    ("pat.c1", 106), ("pat.c4", 64), ("pat.c5", 144), ("pat.c5.end", 42), ("pat.c6", 82), ("pat.note", 254),
    ("pat.note.end", 22), ("per", 226), ("proc.per", 22), ("proc.vol", 202), ("sl.end", 10), ("sl.init2", 124),
    ("sl.init3", 125), ("sl.run", 351), ("smp.loop", 45), ("smp.loop.end", 223), ("smp.loop.len", 202),
    ("smp.sil", 27), ("smp.sil.end", 235), ("smp.sil.len", 190), ("smp.start.dma", 188), ("smp.start.end", 231),
    ("smp.start.len", 198), ("tick", 54), ("v", 138), ("v.off", 49), ("v.pat", 13), ("vib", 442), ("vib.del", 26),
    ("vib.loop", 75), ("vib.none", -21), ("vib.step", 238), ("vib.wait", 25), ("wt", 29), ("wt.dma", 277),
    ("wt.end", 221), ("wt.jump", 137), ("wt.nodma", 65), ("wt.reset", 86), ("wt.st2", 25), ("wt.wave", 157),
    ("wt.wave.loc", 166),
];
/// Build 2: the paths whose cycles differ from build 3's.
const B2_DELTA: &[(&str, i32)] = &[
    ("dur", -36), ("env.loop", -39), ("env.step", -8), ("env.wait", 21), ("pat.c1", -20), ("proc.per", -21),
    ("proc.vol", -28), ("vib.none", 21), ("wt.new", -25),
];
/// Build 1: the paths whose cycles differ from build 3's.
const B1_DELTA: &[(&str, i32)] = &[
    ("dur", -36), ("env.loop", -39), ("env.step", -8), ("env.wait", 21), ("pat.c0", 53), ("pat.c1", -43),
    ("pat.note.end", -16), ("pe.loop", 53), ("pe.none", -21), ("pe.step", 236), ("proc.per", -21), ("proc.vol", -28),
    ("vib.none", 21), ("wt.new", -25), ("wt.st2", 21), ("wt.wait", 21),
];

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

// ---------------------------------------------------------------------------------
// Time

/// The original's time: every Paula write of the reference (EP206's Eagleplayer M.O.N.) goes
/// through one of the host's ENPP_Poke* / ENPP_DMAMask calls, and the engine starts its count of
/// the 68000 cycles anew at each one, at a whole colour clock. A write therefore lands at the previous host call's end (or the tick's
/// start) plus half the cycles of the code paths taken since, rounded down; a DMA on waits up to
/// the tenth raster line after it, as the host's DMAMask does.
struct Clk {
    /// 68000 cycles since `tl`.
    cy: i64,
    /// The colour clock of the last host call's end, or of the tick's start.
    tl: u64,
    costs: &'static [(&'static str, i32)],
    /// The build's differences from `costs`.
    delta: &'static [(&'static str, i32)],
}

impl Clk {
    fn new(build: Build) -> Clk {
        let delta = match build {
            Build::B1 => B1_DELTA,
            Build::B2 => B2_DELTA,
            Build::B3 => &[],
        };
        Clk { cy: 0, tl: 0, costs: COSTS, delta }
    }

    /// A code path of the original.
    fn k(&mut self, l: &'static str) {
        for t in [self.costs, self.delta] {
            if let Ok(i) = t.binary_search_by(|e| e.0.cmp(l)) {
                self.cy += t[i].1 as i64;
            }
        }
    }

    fn start(&mut self, hw: &mut Hw) {
        self.tl = hw.bus.clock;
        self.cy = 0;
    }

    /// Up to the clock of the next host call.
    fn at(&mut self, hw: &mut Hw) {
        let t = self.tl + self.cy.max(0) as u64 / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
        self.cy = 0;
    }

    fn done(&mut self, hw: &mut Hw) {
        self.tl = hw.bus.clock;
    }

    fn dmacon(&mut self, hw: &mut Hw, v: u16) {
        self.at(hw);
        hw.dmacon(v);
        if v & 0x8000 != 0 {
            let (now, line) = (hw.bus.clock, crate::paula::LINE);
            hw.bus.clock = (now / line + 10) * line;
        }
        self.done(hw);
    }

    fn loc(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.at(hw);
        hw.loc(ch, a);
        self.done(hw);
    }

    fn len(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.at(hw);
        hw.len(ch, v);
        self.done(hw);
    }

    fn per(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.at(hw);
        hw.per(ch, v);
        self.done(hw);
    }

    fn vol(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.at(hw);
        hw.vol(ch, v);
        self.done(hw);
    }
}

fn rw(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

// ---------------------------------------------------------------------------------
// Recognition and location of the tables

/// Three `JMP d16(PC)` at 0, 4 and 8; the code at the first target begins
/// `LEA d16(PC),A5` and `BCLR #0,d16(A5)`.
fn detect(d: &[u8]) -> bool {
    if d.len() < 0x40 || rw(d, 0) != 0x4EFA || rw(d, 4) != 0x4EFA || rw(d, 8) != 0x4EFA {
        return false;
    }
    let t0 = (2 + rw(d, 2) as i16 as isize) as usize;
    t0 + 8 <= d.len() && rw(d, t0) == 0x4BFA && be32(d, t0 + 4) == 0x08AD_0000
}

/// The build, reduced to feature flags.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Build {
    /// Only 0xFB..0xFF are commands, no rest, other slide arithmetic.
    B1,
    /// 16-byte song entries, six pattern commands, absolute envelope loops.
    B2,
    /// 12-byte entries, ten pattern commands.
    B3,
}

#[derive(Clone, Debug)]
struct Layout {
    build: Build,
    base: usize,
    nsub: u8,
    esz: usize,
    inst: usize,
    pt: usize,
    sil: usize,
    /// Pitch envelope present (builds 3b, 3c; always in builds 1/2).
    pitch: bool,
    /// Voice enable mask (3b).
    emask: u8,
    /// Wave-sequence instruments (3c): the pointer table.
    wt: Option<usize>,
}

fn find(d: &[u8], from: usize, to: usize, pat: &[u8]) -> Option<usize> {
    let to = to.min(d.len());
    (from..to.saturating_sub(pat.len() - 1)).step_by(2).find(|&o| d[o..o + pat.len()] == *pat)
}

/// Locates the tables. The scans run over the 16-bit-aligned positions of the code
/// block, from the init routine up to the state area.
fn scan(d: &[u8]) -> Option<Layout> {
    if !detect(d) {
        return None;
    }
    let t0 = (2 + rw(d, 2) as i16 as isize) as usize;
    let q = (6 + rw(d, 6) as i16 as isize) as usize;
    let state = (t0 as isize + 2 + rw(d, t0 + 2) as i16 as isize) as usize;
    if state > d.len() || q > state || t0 > state {
        return None;
    }
    // BASE: LEA d16(PC),A0 ; MOVE.L A0,xx(A5) ; MOVEQ #0,D0 ; MOVE.B nn(A5),D0
    let mut found = None;
    for o in (t0..state).step_by(2) {
        if rw(d, o) == 0x41FA && rw(d, o + 4) == 0x2B48 && rw(d, o + 8) == 0x7000 && rw(d, o + 10) == 0x102D {
            found = Some(o);
            break;
        }
    }
    let o = found?;
    let base = (o as isize + 2 + rw(d, o + 2) as i16 as isize) as usize;
    let nsub = rb(d, state + rw(d, o + 12) as usize);
    let esz = match (rw(d, o + 14), rw(d, o + 16)) {
        (0xE948, _) => 16,
        (0xC0FC, 12) => 12,
        _ => return None,
    };
    // the period table: LEA dd(A5),A3 ; MOVE.W 0(A3,D2.W),D2
    let o = (q..state).step_by(2).find(|&o| rw(d, o) == 0x47ED && rw(d, o + 4) == 0x3433 && rw(d, o + 6) == 0x2000)?;
    let pt = state + rw(d, o + 2) as usize;
    let build = if esz == 16 {
        // build 1 compares a note byte with 0xFE
        if find(d, q, state, &[0x0C, 0x02, 0x00, 0xFE]).is_some() {
            Build::B1
        } else {
            Build::B2
        }
    } else {
        Build::B3
    };
    let pitch = build != Build::B3 || find(d, q, state, &[0x4A, 0x68, 0x00, 0x30, 0x67, 0x52, 0x53, 0x28, 0x00, 0x4D]).is_some();
    let mut emask = 0x0F;
    let mut wt = None;
    if build == Build::B3 {
        // LEA dd(PC),A3 ; BTST D7,(A3): the enable mask byte
        if let Some(o) = (q..state).step_by(2).find(|&o| rw(d, o) == 0x47FA && rw(d, o + 4) == 0x0F13) {
            let a = (o as isize + 2 + rw(d, o + 2) as i16 as isize) as usize;
            emask = rb(d, a);
        }
        // CMPI.B #2,$40(A0) ; BNE: wave-sequence instruments; LEA dd(PC),A2 ; MOVE.W 0(A2),$38(A0)
        if find(d, q, state, &[0x0C, 0x28, 0x00, 0x02, 0x00, 0x40, 0x66, 0x5C]).is_some() {
            if let Some(o) = (q..state).step_by(2).find(|&o| rw(d, o) == 0x45FA && rw(d, o + 4) == 0x3172 && rw(d, o + 6) == 0) {
                wt = Some((o as isize + 2 + rw(d, o + 2) as i16 as isize) as usize);
            }
        }
    }
    let inst = base + esz * nsub as usize;
    Some(Layout { build, base, nsub, esz, inst, pt, sil: pt.checked_sub(14)?, pitch, emask, wt })
}

// ---------------------------------------------------------------------------------
// Replayer

#[derive(Clone, Default)]
struct Voice {
    olist: usize,
    optr: usize,
    pstart: usize,
    pat: usize,
    /// Address of the current instrument; 0 is "none yet" (decision, see `inst`).
    inst: usize,
    rpt: u8,
    transpose: u8,
    note: u8,
    reset: bool,
    dur: u16,
    chanvol: u16,
    cur: u16,
    pout: u16,
    outper: u16,
    outvol: u16,
    state: u8,
    running: bool,
    newinst: bool,
    shorten: u16,
    plen: u16,
    pcnt: u16,
    ptgt: u16,
    pdelta: u16,
    prem: u16,
    eidx: u8,
    ecnt: u8,
    sus: bool,
    pidx: u8,
    pcntr: u8,
    ppitch: u16,
    poff: u16,
    vdel: u8,
    vidx: u8,
    vcnt: u8,
    voff: u16,
    wpos: u16,
    wcnt: u8,
    wlen: u16,
    wspd: u8,
    waddr: u32,
}

struct Mon {
    d: Vec<u8>,
    gbase: u32,
    lay: Layout,
    title: String,
    sub: u16,
    started: bool,
    playing: bool,
    songend: bool,
    mask: u8,
    tempo: u8,
    initvol: u8,
    master: u8,
    fdir: u8,
    fcnt: u8,
    fspeed: u8,
    ftarget: u8,
    /// The register that tracks the current instrument across voices.
    a1: usize,
    v: [Voice; 4],
    c: Clk,
}

fn sx(base: usize, w: u16) -> usize {
    (base as isize).wrapping_add(w as i16 as isize) as usize
}

impl Mon {
    fn ptr(&self, w: u16) -> usize {
        sx(self.lay.base, w)
    }

    /// A byte of the instrument at `a1`. Decision (spec 8.2): a voice that plays a note
    /// before any instrument select reads an instrument record that is all zero except
    /// byte +0 = 1, long +4 = 0x0D00 and word +10 = 0x1350 (what the original finds in
    /// low memory); it happens in one corpus file.
    fn ib(&self, a1: usize, off: usize) -> u8 {
        if a1 == 0 {
            return match off {
                0 => 1,
                6 => 0x0D,
                10 => 0x13,
                11 => 0x50,
                _ => 0,
            };
        }
        rb(&self.d, a1 + off)
    }

    fn iw(&self, a1: usize, off: usize) -> u16 {
        (self.ib(a1, off) as u16) << 8 | self.ib(a1, off + 1) as u16
    }

    fn il(&self, a1: usize, off: usize) -> u32 {
        (self.iw(a1, off) as u32) << 16 | self.iw(a1, off + 2) as u32
    }

    fn ptword(&self, n: u8) -> u32 {
        rw(&self.d, self.lay.pt + 2 * n as usize) as u32
    }

    fn period(&mut self, n: u8, w: u16) -> u16 {
        let pt = self.ptword(n);
        self.c.k("per");
        ((pt * w as u32) >> 10) as u16
    }

    fn start(&mut self) {
        self.started = true;
        self.playing = false;
        self.songend = false;
        let n = self.sub as usize + 1;
        if n > self.lay.nsub as usize {
            return;
        }
        let e = self.lay.base + self.lay.esz * (n - 1);
        self.playing = true;
        self.fdir = 0;
        self.fcnt = 0;
        self.fspeed = 0;
        self.ftarget = 0;
        self.initvol = rb(&self.d, e + 8);
        self.tempo = rb(&self.d, e + 10);
        // EP206 starts the master volume at the entry's volume (Flimbo's Quest: 40, every write
        // 40/64 of the volume), not at 64
        self.master = self.initvol;
        self.mask = 0x0F;
        self.a1 = 0;
        for ch in 0..4 {
            let w = rw(&self.d, e + 2 * ch);
            let mut v = Voice::default();
            if w == 0 {
                self.mask &= !(1 << ch);
            } else {
                v.olist = self.ptr(w);
                v.optr = v.olist;
                v.rpt = 1;
                v.reset = true;
            }
            self.v[ch] = v;
        }
    }

    /// ORDER: false when the end-of-music command stops the tick.
    fn order(&mut self, ch: usize, hw: &mut Hw) -> bool {
        const L: [&str; 16] = [
            "ord.c0", "ord.c1", "ord.c2", "ord.c3", "ord.c4", "ord.c5", "ord.c6", "ord.c7", "ord.c8", "ord.c9", "ord.c10",
            "ord.c11", "ord.c12", "ord.c13", "ord.c14", "ord.c15",
        ];
        for _ in 0..GUARD {
            self.c.k("ord");
            let w = rw(&self.d, self.v[ch].optr);
            self.v[ch].optr += 2;
            if w < 0xF000 {
                self.c.k("ord.pat");
                let p = self.ptr(w);
                self.v[ch].pstart = p;
                self.v[ch].pat = p;
                return true;
            }
            let a = w as u8;
            self.c.k(L[((w >> 8) & 15) as usize]);
            match (w >> 8) & 15 {
                0 => self.v[ch].transpose = a,
                1 => self.v[ch].rpt = a,
                2 => {
                    self.fdir = 1;
                    self.fspeed = a;
                    self.fcnt = a;
                    self.master = 0;
                    self.ftarget = self.initvol;
                }
                3 => {
                    self.fdir = 0xFF;
                    self.fspeed = a;
                    self.fcnt = a;
                    self.ftarget = 0;
                }
                4 => self.tempo = a,
                5 => {
                    // EP206 reports the song's end on every jump (any voice, any target), and
                    // plays on
                    self.v[ch].optr = self.v[ch].olist + 2 * a as usize;
                    self.songend = true;
                }
                6 => {
                    self.playing = false;
                    self.songend = true;
                    self.c.dmacon(hw, 0);
                    return false;
                }
                _ => {}
            }
        }
        true
    }

    fn emask_ok(&self, ch: usize) -> bool {
        self.lay.emask & (1 << ch) != 0
    }

    /// `d * tempo - shorten` (16 bit); builds 3 take the shortening, only once.
    fn dur_of(&mut self, ch: usize, d: u8) -> u16 {
        self.c.k("dur");
        let mut x = (d as u16).wrapping_mul(self.tempo as u16);
        if self.lay.build == Build::B3 {
            if self.v[ch].shorten != 0 {
                self.c.k("dur.short");
            }
            x = x.wrapping_sub(self.v[ch].shorten);
            self.v[ch].shorten = 0;
        }
        x
    }

    /// PATTERN: false when the tick is aborted by the end-of-music command.
    fn pattern(&mut self, ch: usize, hw: &mut Hw) -> bool {
        const L: [&str; 16] = [
            "pat.c0", "pat.c1", "pat.c2", "pat.c3", "pat.c4", "pat.c5", "pat.c6", "pat.c7", "pat.c8", "pat.c9", "pat.c10",
            "pat.c11", "pat.c12", "pat.c13", "pat.c14", "pat.c15",
        ];
        let b1 = self.lay.build == Build::B1;
        let b3 = self.lay.build == Build::B3;
        for _ in 0..GUARD {
            self.c.k("pat");
            let b = rb(&self.d, self.v[ch].pat);
            self.v[ch].pat += 1;
            let cmd = if b1 {
                match b {
                    0xFF => 0,
                    0xFE => 1,
                    0xFD => 2,
                    0xFC => 3,
                    0xFB => 4,
                    _ => 99,
                }
            } else if b < 0x80 {
                99
            } else {
                b & 15
            };
            if cmd == 99 {
                self.c.k("pat.note");
                let n = b.wrapping_add(self.v[ch].transpose);
                let w = self.iw(self.a1, 2);
                let p = self.period(n, w);
                let d = rb(&self.d, self.v[ch].pat);
                self.v[ch].pat += 1;
                let dur = self.dur_of(ch, d);
                let vdel = self.ib(self.a1, 0x12);
                let v = &mut self.v[ch];
                v.note = n;
                v.cur = p;
                v.pout = p;
                v.dur = dur;
                v.eidx = 0;
                v.ecnt = 1;
                v.pcntr = 1;
                v.pidx = 0;
                v.poff = 0;
                v.vdel = vdel;
                v.vcnt = 1;
                v.vidx = 0;
                v.voff = 0;
                v.state = 2;
                v.running = true;
                if b3 {
                    v.sus = true;
                } else {
                    v.wcnt = 1;
                }
                if self.emask_ok(ch) {
                    self.c.dmacon(hw, 1 << ch);
                } else {
                    self.c.k("pat.note.masked");
                }
                self.c.k("pat.note.end");
                return true;
            }
            self.c.k(L[cmd as usize & 15]);
            match cmd {
                0 => {
                    let v = &mut self.v[ch];
                    v.rpt = v.rpt.wrapping_sub(1);
                    if v.rpt != 0 {
                        self.c.k("pat.c0.rep");
                        v.pat = v.pstart;
                    } else {
                        v.rpt = 1;
                        if !self.order(ch, hw) {
                            return false;
                        }
                    }
                }
                1 => {
                    let i = rb(&self.d, self.v[ch].pat) as usize;
                    self.v[ch].pat += 1;
                    let a = self.lay.inst + 64 * i;
                    self.a1 = a;
                    let cv = self.ib(a, 0x0E) as u16;
                    let pp = self.iw(a, 0x14);
                    let v = &mut self.v[ch];
                    v.inst = a;
                    v.newinst = true;
                    v.chanvol = cv;
                    if b3 && self.lay.pitch {
                        v.ppitch = pp;
                    }
                }
                2 => {
                    self.v[ch].chanvol = rb(&self.d, self.v[ch].pat) as u16;
                    self.v[ch].pat += 1;
                }
                3 => {
                    let n = rb(&self.d, self.v[ch].pat).wrapping_add(self.v[ch].transpose);
                    let d = rb(&self.d, self.v[ch].pat + 1);
                    self.v[ch].pat += 2;
                    let w = self.iw(self.a1, 2);
                    self.v[ch].ptgt = self.period(n, w);
                    self.v[ch].plen = (d as u16).wrapping_mul(self.tempo as u16);
                }
                4 => {
                    let n = rb(&self.d, self.v[ch].pat).wrapping_add(self.v[ch].transpose);
                    let ds = rb(&self.d, self.v[ch].pat + 1);
                    let d = rb(&self.d, self.v[ch].pat + 2);
                    self.v[ch].pat += 3;
                    let w = self.iw(self.a1, 2);
                    let p = self.period(n, w);
                    let dur = self.dur_of(ch, d);
                    let v = &mut self.v[ch];
                    v.ptgt = v.cur;
                    v.cur = p;
                    v.pout = p;
                    v.plen = (ds as u16).wrapping_mul(self.tempo as u16);
                    v.dur = dur;
                    v.state = 3;
                    v.running = true;
                    return true;
                }
                5 if !b1 => {
                    let d = rb(&self.d, self.v[ch].pat);
                    self.v[ch].pat += 1;
                    let dur = self.dur_of(ch, d);
                    self.v[ch].dur = dur;
                    if self.emask_ok(ch) {
                        self.c.vol(hw, ch, 0);
                    } else {
                        self.c.k("pat.c5.masked");
                    }
                    self.c.k("pat.c5.end");
                    self.v[ch].running = false;
                    return true;
                }
                6 if b3 => {
                    let hi = rb(&self.d, self.v[ch].pat) as u16;
                    let lo = rb(&self.d, self.v[ch].pat + 1) as u16;
                    self.v[ch].pat += 2;
                    let v = &mut self.v[ch];
                    v.pcntr = 1;
                    v.pidx = 0;
                    v.poff = 0;
                    if self.lay.pitch {
                        v.ppitch = hi << 8 | lo;
                    }
                }
                7 if b3 => {
                    let d = rb(&self.d, self.v[ch].pat);
                    self.v[ch].pat += 1;
                    self.v[ch].dur = d as u16;
                    self.v[ch].running = false;
                    self.c.dmacon(hw, 1 << ch);
                    self.c.k("pat.c7.end");
                    return true;
                }
                8 if b3 => {
                    self.v[ch].shorten = rb(&self.d, self.v[ch].pat) as u16;
                    self.v[ch].pat += 1;
                }
                9 if b3 => {
                    let d = rb(&self.d, self.v[ch].pat);
                    self.v[ch].pat += 1;
                    let dur = self.dur_of(ch, d);
                    self.v[ch].dur = dur;
                    self.v[ch].sus = false;
                    return true;
                }
                // not defined by any file: consumed as a no-op (decision)
                _ => {}
            }
        }
        true
    }

    /// The volume envelope (6.4).
    fn envelope(&mut self, ch: usize) {
        let b3 = self.lay.build == Build::B3;
        self.c.k("env");
        let v = &mut self.v[ch];
        v.ecnt = v.ecnt.wrapping_sub(1);
        if v.ecnt != 0 {
            self.c.k("env.wait");
            return;
        }
        let a1 = self.a1;
        let (reload, ptrw) = (self.ib(a1, 0x0F), self.iw(a1, 0x0C));
        let a2 = sx(self.lay.base, ptrw);
        let d = &self.d;
        let c = &mut self.c;
        let v = &mut self.v[ch];
        c.k("env.step");
        v.ecnt = reload;
        let mut x = 0u8;
        for _ in 0..GUARD {
            x = rb(d, a2 + v.eidx as usize);
            if b3 {
                if x == 0x81 && !v.sus {
                    c.k("env.rel");
                    v.eidx = v.eidx.wrapping_add(2);
                    continue;
                }
                if x == 0x80 || x == 0x81 {
                    c.k(if x == 0x80 { "env.loop" } else { "env.sus" });
                    let k = rb(d, a2 + v.eidx.wrapping_add(1) as usize);
                    v.eidx = v.eidx.wrapping_sub(k);
                    continue;
                }
                v.eidx = v.eidx.wrapping_add(1);
                break;
            } else {
                let mut g = 0;
                while x & 0x80 != 0 && g < GUARD {
                    c.k("env.loop");
                    v.eidx = x & 0x7F;
                    x = rb(d, a2 + v.eidx as usize);
                    g += 1;
                }
                v.eidx = v.eidx.wrapping_add(1);
                break;
            }
        }
        let a = (x as u32 * v.chanvol as u32) >> 6;
        v.outvol = (((a * self.master as u32) >> 6) & 0xFFFF) as u16;
    }

    /// The slide (6.6).
    fn slide(&mut self, ch: usize) {
        let b1 = self.lay.build == Build::B1;
        let c = &mut self.c;
        let v = &mut self.v[ch];
        c.k("sl");
        if v.plen == 0 {
            c.k("sl.none");
            return;
        }
        let init = v.state == 3 || v.state == 2;
        if v.state == 3 {
            v.state = 0;
        }
        if init {
            c.k(if v.state == 2 { "sl.init2" } else { "sl.init3" });
            v.pcnt = 1;
            v.pout = v.ptgt;
            if b1 {
                let diff = v.ptgt.wrapping_sub(v.cur) as i16 as i32;
                let len = v.plen as i16 as i32;
                v.pdelta = (diff / len) as u16;
                v.prem = (diff % len) as u16;
            } else {
                v.pdelta = v.ptgt.wrapping_sub(v.cur);
            }
        } else if b1 {
            // decision: build 1 is written from the specification only (its one file never slides)
            c.k("sl.b1");
            v.pout = v.pout.wrapping_sub(v.pdelta);
            v.plen = v.plen.wrapping_sub(1);
            if v.plen == 0 {
                c.k("sl.b1.end");
                v.pout = v.pout.wrapping_sub(v.prem);
            }
        } else {
            c.k("sl.run");
            let q = (v.pcnt as i16 as i32 * v.pdelta as i16 as i32) / (v.plen as i16 as i32);
            v.pout = v.ptgt.wrapping_sub(q as u16);
            if v.pcnt == v.plen {
                c.k("sl.end");
                v.plen = 0;
            } else {
                v.pcnt = v.pcnt.wrapping_add(1);
            }
        }
    }

    /// The pitch envelope (6.7).
    fn pitch_env(&mut self, ch: usize) {
        if !self.lay.pitch {
            return;
        }
        let b3 = self.lay.build == Build::B3;
        let a1 = self.a1;
        let ptrw = if b3 { self.v[ch].ppitch } else { self.iw(a1, 0x14) };
        self.c.k("pe");
        if ptrw == 0 {
            self.c.k("pe.none");
            return;
        }
        let speed = self.ib(a1, 0x16);
        let a2 = self.ptr(ptrw);
        let c = &mut self.c;
        let v = &mut self.v[ch];
        v.pcntr = v.pcntr.wrapping_sub(1);
        if v.pcntr != 0 {
            c.k("pe.wait");
            return;
        }
        c.k("pe.step");
        v.pcntr = speed;
        let d = &self.d;
        let mut x = rb(d, a2 + v.pidx as usize);
        for _ in 0..GUARD {
            if b3 {
                if x != 0x80 {
                    break;
                }
                let k = rb(d, a2 + v.pidx.wrapping_add(1) as usize);
                v.pidx = v.pidx.wrapping_sub(k);
            } else {
                if x & 0x80 == 0 {
                    break;
                }
                v.pidx = x & 0x7F;
            }
            c.k("pe.loop");
            x = rb(d, a2 + v.pidx as usize);
        }
        v.pidx = v.pidx.wrapping_add(1);
        let n = x.wrapping_add(v.note);
        let w = self.iw(a1, 2) as u32;
        let pt = rw(d, self.lay.pt + 2 * n as usize) as u32;
        let p = (((pt * w) >> 8) as u16) >> 2;
        self.v[ch].poff = p.wrapping_sub(self.v[ch].cur);
    }

    /// The vibrato (6.8).
    fn vibrato(&mut self, ch: usize) {
        let a1 = self.a1;
        let b3 = self.lay.build == Build::B3;
        let tw = self.iw(a1, 0x10);
        let reload = self.ib(a1, 0x13);
        let a2 = self.ptr(tw);
        let d = &self.d;
        let c = &mut self.c;
        let v = &mut self.v[ch];
        c.k("vib");
        if tw != 0 {
            if v.vdel != 0 {
                c.k("vib.del");
                v.vdel -= 1;
            } else {
                v.vcnt = v.vcnt.wrapping_sub(1);
                if v.vcnt == 0 {
                    c.k("vib.step");
                    v.vcnt = reload;
                    let mut x = rb(d, a2 + v.vidx as usize);
                    for _ in 0..GUARD {
                        if x != 0x80 {
                            break;
                        }
                        c.k("vib.loop");
                        let k = rb(d, a2 + v.vidx.wrapping_add(1) as usize);
                        v.vidx = if b3 { v.vidx.wrapping_sub(k) } else { k };
                        x = rb(d, a2 + v.vidx as usize);
                    }
                    v.vidx = v.vidx.wrapping_add(1);
                    let d1 = (v.outper >> 7) as i16 as i32;
                    let d0 = x.wrapping_neg() as i8 as i32;
                    v.voff = (d0 * d1) as u16;
                } else {
                    c.k("vib.wait");
                }
            }
        } else {
            c.k("vib.none");
        }
        v.outper = v.outper.wrapping_add(v.voff);
    }

    /// Step (e): the sample start / loop switch / wave handling (6.5, 9.2, 9.5).
    fn sample(&mut self, ch: usize, hw: &mut Hw) {
        let a1 = self.a1;
        let ty = self.ib(a1, 0);
        let g = self.gbase;
        let base = self.lay.base;
        let has_sample = if self.lay.build == Build::B3 { ty != 0 } else { ty == 0 };
        self.c.k("smp");
        if has_sample {
            let s = (base as u32).wrapping_add(self.il(a1, 4));
            let len = self.iw(a1, 8);
            let lp = self.iw(a1, 10);
            let c = &mut self.c;
            let v = &mut self.v[ch];
            if v.state != 0 {
                v.state -= 1;
                if v.state != 0 {
                    c.k("smp.start");
                    c.loc(hw, ch, g.wrapping_add(s));
                    c.k("smp.start.len");
                    c.len(hw, ch, len);
                    c.k("smp.start.dma");
                    c.dmacon(hw, 0x8000 | 1 << ch);
                    c.k("smp.start.end");
                } else if lp == 0xFFFF {
                    c.k("smp.sil");
                    c.loc(hw, ch, g.wrapping_add(self.lay.sil as u32));
                    c.k("smp.sil.len");
                    c.len(hw, ch, 4);
                    c.k("smp.sil.end");
                } else {
                    c.k("smp.loop");
                    // build 1 alone takes the loop start in bytes
                    let off = if self.lay.build == Build::B1 { lp as u32 } else { (2 * lp as i16 as i32) as u32 };
                    c.loc(hw, ch, g.wrapping_add(s).wrapping_add(off));
                    c.k("smp.loop.len");
                    c.len(hw, ch, len.wrapping_sub(lp));
                    c.k("smp.loop.end");
                }
            } else {
                c.k("smp.idle");
            }
        } else if self.lay.build == Build::B3 {
            if let Some(wt) = self.lay.wt {
                self.wave_seq(ch, a1, wt, hw);
            } else {
                self.c.k("smp.nowave");
            }
        } else {
            self.wave_table(ch, a1, hw);
        }
    }

    /// Build 1/2 wave-table instruments (type non-zero), spec 9.2.
    fn wave_table(&mut self, ch: usize, a1: usize, hw: &mut Hw) {
        let ty = self.ib(a1, 0);
        let speed = self.ib(a1, 1);
        let g = self.gbase;
        let list = self.iw(a1, 4);
        let a2 = self.ptr(list);
        let st = self.v[ch].state;
        let run;
        self.c.k("wt");
        self.c.k(if ty == 2 { "wt.t2" } else { "wt.tn" });
        if st == 2 {
            self.c.k("wt.st2");
        }
        if st == 2 && self.v[ch].newinst {
            self.c.k("wt.new");
        }
        if st == 2 && (self.v[ch].newinst || ty != 2) {
            self.c.k("wt.reset");
            self.v[ch].wpos = 0;
            self.v[ch].wcnt = 0;
            run = true;
        } else {
            self.v[ch].wcnt = self.v[ch].wcnt.wrapping_sub(1);
            run = self.v[ch].wcnt == 0;
            if run {
                self.c.k("wt.run");
                self.v[ch].wcnt = speed;
            }
        }
        if !run {
            self.c.k("wt.wait");
            return;
        }
        for _ in 0..GUARD {
            let p = a2 + self.v[ch].wpos as usize;
            let w = rw(&self.d, p);
            let arg = rw(&self.d, p + 2);
            match w {
                0xFFFD => {
                    self.c.k("wt.len");
                    self.v[ch].wlen = arg;
                    self.v[ch].wpos = self.v[ch].wpos.wrapping_add(4);
                }
                0xFFFE => {
                    self.c.k("wt.per");
                    self.v[ch].outper = arg;
                    self.v[ch].wpos = self.v[ch].wpos.wrapping_add(4);
                }
                0xFFFF => {
                    self.c.k("wt.jump");
                    self.v[ch].wpos = self.v[ch].wpos.wrapping_sub(arg)
                }
                _ => {
                    self.c.k("wt.wave");
                    self.c.len(hw, ch, self.v[ch].wlen);
                    self.c.k("wt.wave.loc");
                    let a = g.wrapping_add(self.ptr(w) as u32);
                    self.c.loc(hw, ch, a);
                    self.v[ch].wcnt = speed;
                    self.v[ch].wpos = self.v[ch].wpos.wrapping_add(2);
                    if self.v[ch].state == 2 {
                        self.v[ch].state = 0;
                        self.c.k("wt.dma");
                        self.c.dmacon(hw, 0x8000 | 1 << ch);
                    } else {
                        self.c.k("wt.nodma");
                    }
                    self.c.k("wt.end");
                    break;
                }
            }
        }
    }

    /// Build 3c wave-sequence instruments (type 0), spec 9.5.
    fn wave_seq(&mut self, ch: usize, a1: usize, wt: usize, hw: &mut Hw) {
        let flags = self.ib(a1, 1);
        let g = self.gbase;
        let seq = self.ptr(self.iw(a1, 4));
        let d = &self.d;
        let c = &mut self.c;
        let v = &mut self.v[ch];
        c.k("ws");
        if v.state == 2 && (v.newinst || flags & 1 != 0) {
            c.k("ws.reset");
            v.wpos = 0;
            v.wcnt = 1;
        }
        v.wcnt = v.wcnt.wrapping_sub(1);
        if v.wcnt == 0 {
            c.k("ws.step");
            v.wcnt = v.wspd;
            for _ in 0..GUARD {
                let x = rb(d, seq + v.wpos as usize);
                match x {
                    0xFE => {
                        c.k("ws.speed");
                        let k = rb(d, seq + v.wpos as usize + 1);
                        v.wspd = k;
                        v.wcnt = k;
                        v.wpos = v.wpos.wrapping_add(2);
                    }
                    0xFF => {
                        c.k("ws.jump");
                        let k = rb(d, seq + v.wpos as usize + 1) as u16;
                        v.wpos = (v.wpos.wrapping_sub(k)) & 0xFF;
                    }
                    _ => {
                        c.k("ws.wave");
                        v.wpos = v.wpos.wrapping_add(1);
                        let e = wt + 4 * x as usize;
                        v.wlen = rw(d, e);
                        v.waddr = sx(self.lay.base, rw(d, e + 2)) as u32;
                        break;
                    }
                }
            }
        } else {
            c.k("ws.wait");
        }
        c.loc(hw, ch, g.wrapping_add(v.waddr));
        c.k("ws.len");
        c.len(hw, ch, v.wlen);
        if v.state == 2 {
            v.state = 0;
            c.k("ws.dma");
            c.dmacon(hw, 0x8000 | 1 << ch);
        }
        c.k("ws.end");
    }

    fn process(&mut self, ch: usize, hw: &mut Hw) {
        self.c.k("proc");
        if !self.v[ch].running {
            self.c.k("proc.off");
            return;
        }
        if !self.emask_ok(ch) {
            self.c.k("proc.masked");
            self.v[ch].state = 0;
            return;
        }
        self.envelope(ch);
        self.slide(ch);
        self.pitch_env(ch);
        let v = &mut self.v[ch];
        v.outper = v.pout.wrapping_add(v.poff);
        self.vibrato(ch);
        self.sample(ch, hw);
        self.v[ch].newinst = false;
        self.c.k("proc.per");
        self.c.per(hw, ch, self.v[ch].outper);
        self.c.k("proc.vol");
        self.c.vol(hw, ch, self.v[ch].outvol);
        self.c.k("proc.end");
    }

    /// One voice's tick; false when the end-of-music command aborted the whole tick.
    fn voice_tick(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if self.v[ch].reset {
            self.c.k("v.reset");
            self.v[ch].reset = false;
            return self.order(ch, hw) && self.pattern(ch, hw);
        }
        self.a1 = self.v[ch].inst;
        self.v[ch].dur = self.v[ch].dur.wrapping_sub(1);
        if self.v[ch].dur == 0 {
            self.c.k("v.pat");
            self.pattern(ch, hw)
        } else {
            self.c.k("v.proc");
            self.process(ch, hw);
            true
        }
    }

    fn fade_step(&mut self, hw: &mut Hw) {
        self.c.k("fade");
        if self.fdir == 0 {
            return;
        }
        self.c.k("fade.on");
        self.fcnt = self.fcnt.wrapping_sub(1);
        if self.fcnt != 0 {
            return;
        }
        self.c.k("fade.step");
        self.fcnt = self.fspeed;
        self.master = self.master.wrapping_add(self.fdir);
        if self.master != self.ftarget {
            return;
        }
        self.fdir = 0;
        if self.master == 0 {
            self.c.k("fade.end");
            self.c.dmacon(hw, self.mask as u16);
            self.playing = false;
            self.songend = true;
        }
    }
}

impl Replayer for Mon {
    fn format(&self) -> &'static str {
        "Maniacs of Noise"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, (self.lay.nsub.max(1) - 1) as u16, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if let Some(on) = led_of(&self.d, &self.lay, n) {
            if on { hw.bus.ciaa.pra &= !2 } else { hw.bus.ciaa.pra |= 2 }
        }
        self.sub = n;
        self.started = false;
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.start();
        }
        self.c.start(hw);
        if !self.playing {
            return;
        }
        self.c.k("tick");
        for ch in (0..4).rev() {
            if self.mask & (1 << ch) == 0 {
                self.c.k("v.off");
                continue;
            }
            self.c.k("v");
            if !self.voice_tick(ch, hw) {
                return;
            }
        }
        self.fade_step(hw);
    }
}

/// The time from the Eagleplayer's call before InitPlayer to the end of InitSound (the song's
/// start), as the reference takes it: the gaps between its calls and host calls, each half its
/// 68000 cycles rounded down (measured on the 34 corpus files, both parities, all exact). Only the
/// module's own init code differs by build, and InitSound's last part takes 10 cycles more for
/// a song entry that switches the LED on and for each voice the entry leaves off. The calls
/// start on the raster line where the host starts every call (the line phase matters: the DMA-on
/// waits run to a line).
fn start_clocks(build: Build, led: bool, off: i64) -> u64 {
    let (a, b, c) = match build {
        Build::B1 => (607, 654, 1367),
        Build::B2 => (489, 536, 1377),
        Build::B3 => (584, 631, 1640),
    };
    let gaps = [a, 26, 67, 8987, 22, b, 22, c, 949 + 10 * led as i64 + 10 * off];
    gaps.iter().map(|g| (g / 2) as u64).sum()
}

/// The LED the song init sets for subsong `sub`: on when the
/// byte 9 of the song's entry is not 0.
fn led_of(d: &[u8], lay: &Layout, sub: u16) -> Option<bool> {
    (sub < lay.nsub as u16).then(|| rb(d, lay.base + lay.esz * sub as usize + 9) != 0)
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = scan(d)?;
    let title = m.bus.read_cstr(m.deli.file_buf);
    let led = led_of(d, &lay, 0);
    if let Some(on) = led {
        // the song init: ENPP_PokeCommand 1 with the song entry's byte 9
        if on { m.bus.ciaa.pra &= !2 } else { m.bus.ciaa.pra |= 2 }
    }
    let build = lay.build;
    let off = (0..4).filter(|ch| lay.nsub > 0 && rw(d, lay.base + 2 * ch) == 0).count() as i64;
    m.bus.clock += start_clocks(build, led == Some(true), off);
    Some(Box::new(Mon {
        d: d.to_vec(),
        gbase: base,
        lay,
        title,
        sub: 0,
        started: false,
        playing: false,
        songend: false,
        mask: 0,
        tempo: 0,
        initvol: 0,
        master: 64,
        fdir: 0,
        fcnt: 0,
        fspeed: 0,
        ftarget: 0,
        a1: 0,
        v: Default::default(),
        c: Clk::new(build),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    /// A build-3a image: branch table, init and play routines with the signature
    /// instructions, a period table, a one-song table (12-byte entries), one
    /// instrument and a voice that plays one note (length 3 ticks) and ends the music.
    fn module() -> Vec<u8> {
        let mut d = vec![0u8; 0x800];
        for (k, t) in [(0usize, 0x10usize), (4, 0x40), (8, 0x60)] {
            put16(&mut d, k, 0x4EFA);
            put16(&mut d, k + 2, (t - k - 2) as u16);
        }
        put16(&mut d, 0x10, 0x4BFA);
        put16(&mut d, 0x12, (0x80 - 0x12) as u16);
        d[0x14..0x18].copy_from_slice(&[0x08, 0xAD, 0x00, 0x00]);
        put16(&mut d, 0x18, 0x41FA);
        put16(&mut d, 0x1A, (0x600 - 0x1A) as u16);
        put16(&mut d, 0x1C, 0x2B48);
        put16(&mut d, 0x20, 0x7000);
        put16(&mut d, 0x22, 0x102D);
        put16(&mut d, 0x24, 0x10);
        put16(&mut d, 0x26, 0xC0FC);
        put16(&mut d, 0x28, 12);
        put16(&mut d, 0x40, 0x47ED);
        put16(&mut d, 0x42, 0x180);
        put16(&mut d, 0x44, 0x3433);
        put16(&mut d, 0x46, 0x2000);
        d[0x80 + 0x10] = 1; // one subsong
        put16(&mut d, 0x200 + 48, 2048); // period of note 24 at w = 0x400
        put16(&mut d, 0x600, 0x40); // voice 0 order list
        d[0x608] = 64;
        d[0x60A] = 1; // tempo
        // instrument 0: sample at BASE + 0x100, 4 words, loop at word 2
        let i = 0x60C;
        d[i] = 1;
        put16(&mut d, i + 2, 0x400);
        put16(&mut d, i + 4, 0);
        put16(&mut d, i + 6, 0x100);
        put16(&mut d, i + 8, 4);
        put16(&mut d, i + 10, 2);
        put16(&mut d, i + 12, 0x20);
        d[i + 14] = 64;
        d[i + 15] = 1;
        d[0x620..0x623].copy_from_slice(&[0x40, 0x80, 0x01]);
        put16(&mut d, 0x640, 0x50);
        put16(&mut d, 0x642, 0xF600);
        d[0x650..0x656].copy_from_slice(&[0x81, 0x00, 24, 3, 0x80, 0x00]);
        d
    }

    #[test]
    fn recognises_and_locates() {
        let d = module();
        assert!(detect(&d));
        let l = scan(&d).unwrap();
        assert_eq!((l.build, l.base, l.nsub, l.esz, l.inst, l.pt, l.sil), (Build::B3, 0x600, 1, 12, 0x60C, 0x200, 0x1F2));
        assert!(!l.pitch && l.wt.is_none() && l.emask == 0x0F);
        // 16-byte entries make build 2
        let mut e = d.clone();
        put16(&mut e, 0x26, 0xE948);
        assert_eq!(scan(&e).unwrap().build, Build::B2);
    }

    #[test]
    fn rejects_other_files() {
        let d = module();
        assert!(!detect(&d[..0x20]));
        assert!(!detect(&[0u8; 0x100]));
        let mut e = d.clone();
        put16(&mut e, 8, 0x4EF9);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 0x10, 0x4BF9);
        assert!(!detect(&e));
        let mut e = d.clone();
        e[0x16] = 1;
        assert!(!detect(&e));
        // the init routine without its signature: no tables, no replayer
        let mut e = d.clone();
        put16(&mut e, 0x18, 0);
        assert!(scan(&e).is_none());
    }

    fn run(data: Vec<u8>, ticks: usize) -> (Vec<(u32, u16)>, bool) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/mon-test/mon.test"), data).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Maniacs of Noise");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let end = e.songend();
        (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), end)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note_and_ends() {
        let (log, end) = run(module(), 8);
        // the note stops the DMA, the sample starts a tick later, the loop a tick after that
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [4, 2]);
        assert_eq!(reg(&log, 0xDFF0A6)[0], 2048);
        // envelope 64 * chanvol 64 / 64 * master 64 / 64
        assert_eq!(reg(&log, 0xDFF0A8)[0], 64);
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001) && dma.contains(&0x8001));
        // the pattern ends, the order list says end of music: DMACON 0 and song end
        assert!(dma.contains(&0x0000));
        assert!(end);
    }

    #[test]
    fn disabled_voice_writes_nothing() {
        let mut d = module();
        put16(&mut d, 0x600, 0); // voice 0 off: nothing plays, no song end
        let (log, end) = run(d, 6);
        assert!(reg(&log, 0xDFF0A6).is_empty());
        assert!(!end);
    }
}
