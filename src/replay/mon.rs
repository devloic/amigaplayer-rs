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
/// Colour clocks between the writes of two voices (about 570 in the reference).
const VOICE_GAP: u64 = 440;

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
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

    fn period(&self, n: u8, w: u16) -> u16 {
        ((self.ptword(n) * w as u32) >> 10) as u16
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
        self.master = 64;
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
        for _ in 0..GUARD {
            let w = rw(&self.d, self.v[ch].optr);
            self.v[ch].optr += 2;
            if w < 0xF000 {
                let p = self.ptr(w);
                self.v[ch].pstart = p;
                self.v[ch].pat = p;
                return true;
            }
            let a = w as u8;
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
                5 => self.v[ch].optr = self.v[ch].olist + 2 * a as usize,
                6 => {
                    self.playing = false;
                    self.songend = true;
                    hw.dmacon(0);
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
        let mut x = (d as u16).wrapping_mul(self.tempo as u16);
        if self.lay.build == Build::B3 {
            x = x.wrapping_sub(self.v[ch].shorten);
            self.v[ch].shorten = 0;
        }
        x
    }

    /// PATTERN: false when the tick is aborted by the end-of-music command.
    fn pattern(&mut self, ch: usize, hw: &mut Hw) -> bool {
        let b1 = self.lay.build == Build::B1;
        let b3 = self.lay.build == Build::B3;
        for _ in 0..GUARD {
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
                    hw.dmacon(1 << ch);
                }
                return true;
            }
            match cmd {
                0 => {
                    let v = &mut self.v[ch];
                    v.rpt = v.rpt.wrapping_sub(1);
                    if v.rpt != 0 {
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
                        hw.vol(ch, 0);
                    }
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
                    hw.dmacon(1 << ch);
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
        let v = &mut self.v[ch];
        v.ecnt = v.ecnt.wrapping_sub(1);
        if v.ecnt != 0 {
            return;
        }
        let a1 = self.a1;
        let (reload, ptrw) = (self.ib(a1, 0x0F), self.iw(a1, 0x0C));
        let a2 = sx(self.lay.base, ptrw);
        let d = &self.d;
        let v = &mut self.v[ch];
        v.ecnt = reload;
        let mut x = 0u8;
        for _ in 0..GUARD {
            x = rb(d, a2 + v.eidx as usize);
            if b3 {
                if x == 0x81 && !v.sus {
                    v.eidx = v.eidx.wrapping_add(2);
                    continue;
                }
                if x == 0x80 || x == 0x81 {
                    let k = rb(d, a2 + v.eidx.wrapping_add(1) as usize);
                    v.eidx = v.eidx.wrapping_sub(k);
                    continue;
                }
                v.eidx = v.eidx.wrapping_add(1);
                break;
            } else {
                let mut g = 0;
                while x & 0x80 != 0 && g < GUARD {
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
        let v = &mut self.v[ch];
        if v.plen == 0 {
            return;
        }
        let init = v.state == 3 || v.state == 2;
        if v.state == 3 {
            v.state = 0;
        }
        if init {
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
            v.pout = v.pout.wrapping_sub(v.pdelta);
            v.plen = v.plen.wrapping_sub(1);
            if v.plen == 0 {
                v.pout = v.pout.wrapping_sub(v.prem);
            }
        } else {
            let q = (v.pcnt as i16 as i32 * v.pdelta as i16 as i32) / (v.plen as i16 as i32);
            v.pout = v.ptgt.wrapping_sub(q as u16);
            if v.pcnt == v.plen {
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
        if ptrw == 0 {
            return;
        }
        let speed = self.ib(a1, 0x16);
        let a2 = self.ptr(ptrw);
        let v = &mut self.v[ch];
        v.pcntr = v.pcntr.wrapping_sub(1);
        if v.pcntr != 0 {
            return;
        }
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
        let v = &mut self.v[ch];
        if tw != 0 {
            if v.vdel != 0 {
                v.vdel -= 1;
            } else {
                v.vcnt = v.vcnt.wrapping_sub(1);
                if v.vcnt == 0 {
                    v.vcnt = reload;
                    let mut x = rb(d, a2 + v.vidx as usize);
                    for _ in 0..GUARD {
                        if x != 0x80 {
                            break;
                        }
                        let k = rb(d, a2 + v.vidx.wrapping_add(1) as usize);
                        v.vidx = if b3 { v.vidx.wrapping_sub(k) } else { k };
                        x = rb(d, a2 + v.vidx as usize);
                    }
                    v.vidx = v.vidx.wrapping_add(1);
                    let d1 = (v.outper >> 7) as i16 as i32;
                    let d0 = x.wrapping_neg() as i8 as i32;
                    v.voff = (d0 * d1) as u16;
                }
            }
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
        if has_sample {
            let s = (base as u32).wrapping_add(self.il(a1, 4));
            let len = self.iw(a1, 8);
            let lp = self.iw(a1, 10);
            let v = &mut self.v[ch];
            if v.state != 0 {
                v.state -= 1;
                if v.state != 0 {
                    hw.loc(ch, g.wrapping_add(s));
                    hw.len(ch, len);
                    hw.dmacon(0x8200 | 1 << ch);
                } else if lp == 0xFFFF {
                    hw.loc(ch, g.wrapping_add(self.lay.sil as u32));
                    hw.len(ch, 4);
                } else {
                    // build 1 alone takes the loop start in bytes
                    let off = if self.lay.build == Build::B1 { lp as u32 } else { (2 * lp as i16 as i32) as u32 };
                    hw.loc(ch, g.wrapping_add(s).wrapping_add(off));
                    hw.len(ch, len.wrapping_sub(lp));
                }
            }
        } else if self.lay.build == Build::B3 {
            if let Some(wt) = self.lay.wt {
                self.wave_seq(ch, a1, wt, hw);
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
        if st == 2 && (self.v[ch].newinst || ty != 2) {
            self.v[ch].wpos = 0;
            self.v[ch].wcnt = 0;
            run = true;
        } else {
            self.v[ch].wcnt = self.v[ch].wcnt.wrapping_sub(1);
            run = self.v[ch].wcnt == 0;
            if run {
                self.v[ch].wcnt = speed;
            }
        }
        if !run {
            return;
        }
        for _ in 0..GUARD {
            let p = a2 + self.v[ch].wpos as usize;
            let w = rw(&self.d, p);
            let arg = rw(&self.d, p + 2);
            match w {
                0xFFFD => {
                    self.v[ch].wlen = arg;
                    self.v[ch].wpos = self.v[ch].wpos.wrapping_add(4);
                }
                0xFFFE => {
                    self.v[ch].outper = arg;
                    self.v[ch].wpos = self.v[ch].wpos.wrapping_add(4);
                }
                0xFFFF => self.v[ch].wpos = self.v[ch].wpos.wrapping_sub(arg),
                _ => {
                    hw.len(ch, self.v[ch].wlen);
                    hw.loc(ch, g.wrapping_add(self.ptr(w) as u32));
                    self.v[ch].wcnt = speed;
                    self.v[ch].wpos = self.v[ch].wpos.wrapping_add(2);
                    if self.v[ch].state == 2 {
                        self.v[ch].state = 0;
                        hw.dmacon(0x8200 | 1 << ch);
                    }
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
        let v = &mut self.v[ch];
        if v.state == 2 && (v.newinst || flags & 1 != 0) {
            v.wpos = 0;
            v.wcnt = 1;
        }
        v.wcnt = v.wcnt.wrapping_sub(1);
        if v.wcnt == 0 {
            v.wcnt = v.wspd;
            for _ in 0..GUARD {
                let x = rb(d, seq + v.wpos as usize);
                match x {
                    0xFE => {
                        let k = rb(d, seq + v.wpos as usize + 1);
                        v.wspd = k;
                        v.wcnt = k;
                        v.wpos = v.wpos.wrapping_add(2);
                    }
                    0xFF => {
                        let k = rb(d, seq + v.wpos as usize + 1) as u16;
                        v.wpos = (v.wpos.wrapping_sub(k)) & 0xFF;
                    }
                    _ => {
                        v.wpos = v.wpos.wrapping_add(1);
                        let e = wt + 4 * x as usize;
                        v.wlen = rw(d, e);
                        v.waddr = sx(self.lay.base, rw(d, e + 2)) as u32;
                        break;
                    }
                }
            }
        }
        hw.loc(ch, g.wrapping_add(v.waddr));
        hw.len(ch, v.wlen);
        if v.state == 2 {
            v.state = 0;
            hw.dmacon(0x8200 | 1 << ch);
        }
    }

    fn process(&mut self, ch: usize, hw: &mut Hw) {
        if !self.v[ch].running {
            return;
        }
        if !self.emask_ok(ch) {
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
        hw.per(ch, self.v[ch].outper);
        hw.vol(ch, self.v[ch].outvol);
    }

    /// One voice's tick; false when the end-of-music command aborted the whole tick.
    fn voice_tick(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if self.v[ch].reset {
            self.v[ch].reset = false;
            return self.order(ch, hw) && self.pattern(ch, hw);
        }
        self.a1 = self.v[ch].inst;
        self.v[ch].dur = self.v[ch].dur.wrapping_sub(1);
        if self.v[ch].dur == 0 {
            self.pattern(ch, hw)
        } else {
            self.process(ch, hw);
            true
        }
    }

    fn fade_step(&mut self, hw: &mut Hw) {
        if self.fdir == 0 {
            return;
        }
        self.fcnt = self.fcnt.wrapping_sub(1);
        if self.fcnt != 0 {
            return;
        }
        self.fcnt = self.fspeed;
        self.master = self.master.wrapping_add(self.fdir);
        if self.master != self.ftarget {
            return;
        }
        self.fdir = 0;
        if self.master == 0 {
            hw.dmacon(self.mask as u16);
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
        if !self.playing {
            return;
        }
        for ch in (0..4).rev() {
            if self.mask & (1 << ch) == 0 {
                continue;
            }
            hw.wait(VOICE_GAP);
            if !self.voice_tick(ch, hw) {
                return;
            }
        }
        self.fade_step(hw);
    }
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
        assert!(dma.contains(&0x0001) && dma.contains(&0x8201));
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
