//! Paul Shields (`ps.*`), the music system Paul Shields wrote for Airball, Goldrunner
//! II, Jug, Amiga Soccer and Amnios (1987-91).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Paul Shields player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A `ps.*` file is pure data: sample headers, a song header (four voices' initial
//! volumes and track lists, and the envelope or instrument tables), track lists,
//! patterns, envelopes and the sample data. Three layouts (`FF`, `01`, `00`) are told
//! apart by their headers; two engines play them. Engine A (layouts `FF` and `01`)
//! has seven host-provided synthetic waves as instruments 16-22, absolute-period
//! notes and envelope tables; engine B (layout `00`) has an instrument table and
//! writes PER before VOL. Both play at the host's 50 Hz, one parse of a voice's
//! pattern bytes whenever its duration runs out, with volume and pitch envelopes,
//! slides, portamento and a release; a note's sample starts with the next tick's
//! DMA-on and its loop is written right after it.
//!
//! Decisions where the specification leaves the choice to the port:
//! * Engine A can index its period table below the start for notes below the table
//!   (the original then reads its own code). The port extends the table downwards by
//!   octaves instead (the period doubled per octave, at most 0xFFFF).
//! * Instrument 22 (the original plays 400 bytes of its own code) is a silent wave
//!   of the same length here. No corpus file uses it.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Paul Shields", detect, new };

/// The 96-entry pitch table: 8 steps per semitone, the lowest octave (largest periods).
const PITCH: [u16; 96] = [
    27364, 27167, 26972, 26778, 26585, 26394, 26204, 26015, 25828, 25642, 25458, 25275, 25093, 24912, 24733, 24555,
    24378, 24203, 24029, 23856, 23684, 23514, 23345, 23177, 23010, 22845, 22680, 22517, 22355, 22194, 22035, 21876,
    21719, 21562, 21407, 21253, 21100, 20949, 20798, 20648, 20500, 20352, 20206, 20060, 19916, 19773, 19631, 19489,
    19349, 19210, 19072, 18935, 18798, 18663, 18529, 18396, 18263, 18132, 18001, 17872, 17743, 17616, 17489, 17363,
    17238, 17114, 16991, 16869, 16747, 16627, 16507, 16389, 16271, 16154, 16037, 15922, 15807, 15694, 15581, 15469,
    15357, 15247, 15137, 15028, 14920, 14813, 14706, 14601, 14496, 14391, 14288, 14185, 14083, 13982, 13881, 13781,
];

/// Engine A's synthetic waves, instruments 16-21 (signed bytes); 22 is 400 bytes of silence.
const WAVES: [&[i8]; 6] = [
    &[127, 127, -127, -127],
    &[127, 127, 127, 127, -127, -127, -127, -127],
    &[0, 127, 0, -127],
    &[0, 89, 127, 89, 0, -89, -127, -89],
    &[127, 42, -43, -128],
    &[127, 90, 54, 17, -18, -55, -91, -128],
];
const WAVE22_LEN: u32 = 400;

/// Host bytes the voices point at before the music sets them: the "next track"
/// command and an envelope's hold marker.
const HOST_NEXT: u32 = 0xFFFF_0000;
const HOST_HOLD: u32 = 0xFFFF_0002;

/// The master volume.
const MASTER: i16 = 64;

/// The waits between the writes of a tick, in colour clocks (the reference's, roughly):
/// after a voice's step, and before the loop registers (about 8 raster lines).
const T_VOICE: u64 = 360;
const T_WRITE: u64 = 40;
const T_LOOP: u64 = 1900;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Layout {
    /// 32-byte sample headers, the song header at 0x200, engine A.
    FF,
    /// 10-byte sample headers, the song header at 0xA0, engine A.
    L01,
    /// 32-byte sample headers, the song header at 0x200, engine B.
    L00,
}

impl Layout {
    fn header(self) -> usize {
        if self == Layout::L01 {
            0xA0
        } else {
            0x200
        }
    }
    fn engine_a(self) -> bool {
        self != Layout::L00
    }
}

// ---------------------------------------------------------------- recognition

fn w(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

fn equal4(d: &[u8], o: usize) -> bool {
    (1..4).all(|k| w(d, o + 4 * k) == w(d, o))
}

fn pointer(d: &[u8], at: usize) -> Option<usize> {
    let o = w(d, at);
    (o != 0 && o < 0x8000 && o % 2 == 0).then_some(o as usize)
}

/// The layout, by the Eagleplayer's test (specification, section 2).
fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 530 || be32(d, 0) != 0 || be32(d, 4) != 0 || w(d, 8) != 0 {
        return None;
    }
    // the first rule that holds as a whole (the volume words of the first are 0 in
    // the 32-byte layouts' unused sample headers)
    let long_at = |at: usize, v: u32| pointer(d, at).is_some_and(|o| o + 4 <= d.len() && be32(d, o) == v);
    if equal4(d, 164) && long_at(160, 0x00B4_00B6) {
        return Some(Layout::L01);
    }
    if equal4(d, 516) && long_at(512, 0x0214_0216) {
        return Some(Layout::FF);
    }
    if equal4(d, 514) && pointer(d, 516).is_some_and(|o| o + 2 <= d.len() && matches!(w(d, o - 2), 0xFFEC | 0xFFE8)) {
        return Some(Layout::L00);
    }
    None
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sample {
    /// Offsets into the module (or host addresses when `host`).
    lc: u32,
    len: u16,
    llc: u32,
    llen: u16,
    fine: u16,
    defvol: u16,
    /// The loop is the host's silence buffer.
    silent_loop: bool,
    host: bool,
}

/// The sample data start (section 2): after the end command of the list before the
/// last-starting one, searched from that list's start.
fn sample_start(d: &[u8], lay: Layout) -> Option<usize> {
    let h = lay.header();
    let first = if lay.engine_a() { h + 6 } else { h + 4 };
    let mut m: i16 = 0;
    for k in 0..4 {
        m = m.max(w(d, first + 4 * k) as i16);
    }
    if m < 2 {
        return None;
    }
    let m = m as usize;
    let t = w(d, m - 2);
    let mut o = m;
    while o + 2 <= d.len() {
        if w(d, o) == t {
            return Some(o + 2);
        }
        o += 2;
    }
    None
}

/// The 16 module samples (offsets into the module).
fn samples(d: &[u8], lay: Layout) -> Option<Vec<Sample>> {
    let mut at = sample_start(d, lay)? as u32;
    let mut out = Vec::new();
    for k in 0..16 {
        let (len, ls, ll, fine, defvol) = if lay == Layout::L01 {
            let o = 10 * k;
            (w(d, o + 2), w(d, o + 4), w(d, o + 6), w(d, o + 8), 0)
        } else {
            let o = 32 * k;
            (w(d, o + 0x16), w(d, o + 0x1A), w(d, o + 0x1C), w(d, o + 0x1E), w(d, o + 0x18))
        };
        let silent = ll < 2;
        out.push(Sample {
            lc: at,
            len,
            llc: at + ls as u32,
            llen: if silent { 2 } else { ll },
            fine,
            defvol,
            silent_loop: silent,
            host: false,
        });
        at += 2 * len as u32;
    }
    (at as usize <= d.len()).then_some(out)
}

fn detect(d: &[u8]) -> bool {
    layout(d).and_then(|l| samples(d, l)).is_some()
}

// ------------------------------------------------------------------- the engine

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    active: bool,
    rel: bool,
    abs: bool,
    vol: i16,
    vtgt: i16,
    vspd: u16,
    vcnt: u16,
    volofs: u16,
    defvol: u16,
    env: i16,
    per: u16,
    frac: u16,
    slide: u32,
    det: u16,
    tr: u16,
    fine: u16,
    vr: u8,
    vc: u8,
    venvs: u32,
    venv: u32,
    pr: u8,
    pc: u8,
    penvs: u32,
    penv: u32,
    lc: u32,
    len: u16,
    llc: u32,
    llen: u16,
    lst: u32,
    lpos: u32,
    patstart: u32,
    pat: u32,
    rep: u16,
    dur: u16,
}

/// How a voice's parse ended.
#[derive(Clone, Copy, PartialEq)]
enum Event {
    Note,
    Rest,
    Stop,
}

struct PaulShields {
    d: Vec<u8>,
    lay: Layout,
    base: u32,
    /// Instruments: the 16 samples, then (engine A) the 7 synthetic waves.
    smp: Vec<Sample>,
    silence: u32,
    /// The first table of the song header (pitch envelopes or instruments) and the
    /// volume-envelope table (engine A).
    pt: u32,
    vt: u32,
    v: [Voice; 4],
    dacc: u16,
    iacc: u16,
    started: bool,
    songend: bool,
}

impl PaulShields {
    fn b(&self, a: u32) -> u8 {
        match a {
            HOST_NEXT => {
                if self.lay.engine_a() {
                    0x0C
                } else {
                    0x90
                }
            }
            HOST_HOLD => 0x80,
            _ => self.d.get(a as usize).copied().unwrap_or(0),
        }
    }

    fn w(&self, a: u32) -> u16 {
        (self.b(a) as u16) << 8 | self.b(a.wrapping_add(1)) as u16
    }

    /// The guest address of a sample address.
    fn guest(&self, s: &Sample, a: u32) -> u32 {
        if s.host {
            a
        } else {
            self.base.wrapping_add(a)
        }
    }

    fn init(&mut self, hw: &mut Hw) {
        hw.bus.write_word(0xDFF09A, 0x0780);
        hw.dmacon(0x000F);
        hw.bus.write_word(0xDFF09E, 0x00FF);
        if !self.lay.engine_a() {
            self.filter(hw, true);
        }
        self.dacc = 0;
        self.iacc = 0;
        let h = self.lay.header() as u32;
        let pairs = if self.lay.engine_a() {
            self.pt = self.w(h) as u32;
            self.vt = self.w(h + 2) as u32;
            h + 4
        } else {
            self.pt = self.w(h) as u32;
            h + 2
        };
        let (lc, len) = if self.lay.engine_a() { (self.smp[16].lc, 1) } else { (self.silence, 1) };
        for k in 0..4 {
            let ch = 3 - k;
            let vol = self.w(pairs + 4 * k as u32) as i16;
            let list = self.w(pairs + 4 * k as u32 + 2) as u32;
            self.v[ch] = Voice {
                active: true,
                vol,
                vtgt: vol,
                lst: list,
                lpos: list,
                rep: 1,
                dur: 1,
                pat: HOST_NEXT,
                venvs: HOST_HOLD,
                venv: HOST_HOLD,
                penvs: HOST_HOLD,
                penv: HOST_HOLD,
                lc,
                len,
                llc: lc,
                llen: len,
                ..Voice::default()
            };
        }
    }

    /// CIA-A PRA bit 1: the LED filter on (bit clear) or off.
    fn filter(&self, hw: &mut Hw, on: bool) {
        let x = hw.bus.read_byte(0xBFE001);
        hw.bus.write_byte(0xBFE001, if on { x & !2 } else { x | 2 });
    }

    /// conv(n) (section 5.5).
    fn conv(&self, ch: usize, n: u16) -> u16 {
        let v = &self.v[ch];
        let mut x = n.wrapping_add(v.tr).wrapping_add(v.fine) as i16;
        if !self.lay.engine_a() && x < 0 {
            return 0xFFFF;
        }
        let mut o = -1i32;
        loop {
            o += 1;
            x = x.wrapping_sub(96);
            if x < 0 {
                break;
            }
        }
        let i = 96 + x as i32;
        let p = if i >= 0 {
            let p = PITCH[i as usize];
            if o >= 16 {
                0
            } else {
                p >> o
            }
        } else {
            // below the table: the table extended down by octaves
            let oct = (-i + 95) / 96;
            let p = PITCH[(i + 96 * oct) as usize] as u32;
            if oct >= 16 {
                0xFFFF
            } else {
                (p << oct).min(0xFFFF) as u16
            }
        };
        p.wrapping_add(v.det)
    }

    /// A duration byte (a 0 byte is followed by a word).
    fn duration(&mut self, ch: usize) -> u16 {
        let p = self.v[ch].pat;
        let b = self.b(p);
        if b == 0 {
            self.v[ch].pat = p.wrapping_add(3);
            self.w(p.wrapping_add(1))
        } else {
            self.v[ch].pat = p.wrapping_add(1);
            b as u16
        }
    }

    fn next_byte(&mut self, ch: usize) -> u8 {
        let p = self.v[ch].pat;
        self.v[ch].pat = p.wrapping_add(1);
        self.b(p)
    }

    fn instrument(&mut self, ch: usize, i: usize) {
        let Some(s) = self.smp.get(i).copied() else { return };
        let lc = self.guest(&s, s.lc);
        let llc = if s.silent_loop { self.silence } else { self.guest(&s, s.llc) };
        let v = &mut self.v[ch];
        v.lc = lc;
        v.len = s.len;
        v.llc = llc;
        v.llen = s.llen;
        v.fine = s.fine;
        v.defvol = s.defvol;
    }

    /// An envelope at module offset `e`: its reload byte, then the bytes.
    fn set_env(&mut self, ch: usize, vol: bool, e: u32) {
        let r = self.b(e);
        let v = &mut self.v[ch];
        if vol {
            v.vr = r;
            v.vc = r;
            v.venvs = e.wrapping_add(1);
            v.venv = e.wrapping_add(1);
        } else {
            v.pr = r;
            v.pc = r;
            v.penvs = e.wrapping_add(1);
            v.penv = e.wrapping_add(1);
        }
    }

    fn portamento(&mut self, ch: usize, n1: u8, n2: u8) {
        let (mut a, mut b) = (n1 as u16 * 8, n2 as u16 * 8);
        if self.lay.engine_a() && self.v[ch].abs {
            a = self.conv(ch, a);
            b = self.conv(ch, b);
        }
        self.v[ch].per = a;
        let d = b.wrapping_sub(a) as i16;
        let neg = d < 0;
        let d = d.unsigned_abs() as u32;
        let dur = self.duration(ch);
        self.v[ch].dur = dur;
        let mut s = if dur == 0 {
            0
        } else {
            let dur = dur as u32;
            ((d / dur) << 16) + ((((d % dur) << 16) / dur) & 0xFFFF)
        };
        if neg {
            s = s.wrapping_neg();
        }
        self.v[ch].slide = s;
    }

    /// The note start (section 5.3).
    fn note_start(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.v[ch];
        v.env = if v.volofs != 0 { v.volofs as i16 } else { v.defvol as i16 };
        v.vc = v.vr;
        v.venv = v.venvs;
        v.pc = v.pr;
        v.penv = v.penvs;
        v.rel = false;
        let (lc, len) = (v.lc, v.len);
        hw.dmacon(1 << ch);
        hw.wait(T_WRITE);
        hw.loc(ch, lc);
        hw.len(ch, len);
        hw.wait(T_WRITE);
        self.iacc |= 1 << (7 + ch);
        self.dacc |= 0x8200 | 1 << ch;
    }

    /// The track step at a pattern end (section 5.7); `None` when the voice stopped.
    fn track_step(&mut self, ch: usize) -> Option<()> {
        let a = self.lay.engine_a();
        self.v[ch].rep = self.v[ch].rep.wrapping_sub(1);
        if self.v[ch].rep != 0 {
            self.v[ch].pat = self.v[ch].patstart;
            return Some(());
        }
        // a list of commands only, with no pattern, would never end
        for _ in 0..0x10000 {
            let p = self.v[ch].lpos;
            let wd = self.w(p);
            self.v[ch].lpos = p.wrapping_add(2);
            if wd < 0xFFE8 {
                self.v[ch].rep = 1;
                let mut wd = wd;
                if wd & 0x8000 != 0 {
                    wd &= 0x7FFF;
                    let r = self.w(self.v[ch].lpos);
                    let v = &mut self.v[ch];
                    v.rep = r;
                    v.lpos = v.lpos.wrapping_add(2);
                }
                let v = &mut self.v[ch];
                v.patstart = wd as u32;
                v.pat = wd as u32;
                return Some(());
            }
            let w1 = self.w(self.v[ch].lpos);
            let w2 = self.w(self.v[ch].lpos.wrapping_add(2));
            let take = |n: u32, s: &mut Self| s.v[ch].lpos = s.v[ch].lpos.wrapping_add(2 * n);
            let cmd = match (a, wd) {
                (true, 0xFFE8) | (false, 0xFFF0) => 0,
                (true, 0xFFEC) | (false, 0xFFF8) => 1,
                (true, 0xFFF0) | (false, 0xFFEC) => 2,
                (true, 0xFFF4) | (false, 0xFFE8) => 3,
                (true, 0xFFF8) | (false, 0xFFF4) => 4,
                _ => 5,
            };
            match cmd {
                0 => {
                    take(2, self);
                    let v = &mut self.v[ch];
                    v.vtgt = w1 as i16;
                    v.vspd = w2;
                    v.vcnt = w2;
                }
                1 => {
                    take(1, self);
                    self.v[ch].det = w1;
                }
                2 => {
                    let v = &mut self.v[ch];
                    v.lpos = v.lst;
                    if ch == if a { 1 } else { 0 } {
                        self.songend = true;
                    }
                }
                3 => {
                    let v = &mut self.v[ch];
                    v.active = false;
                    if a {
                        v.abs = false;
                        v.rel = false;
                    }
                    self.songend = true;
                    return None;
                }
                4 => {
                    take(2, self);
                    for v in &mut self.v {
                        v.vtgt = w1 as i16;
                        v.vspd = w2;
                        v.vcnt = w2;
                    }
                }
                _ => {
                    take(1, self);
                    self.v[ch].tr = w1;
                }
            }
        }
        self.v[ch].active = false;
        None
    }

    /// Parses pattern bytes up to a note, a portamento or a rest (section 5.2).
    fn parse(&mut self, ch: usize, hw: &mut Hw) -> Event {
        let a = self.lay.engine_a();
        for _ in 0..0x10000 {
            let c = self.next_byte(ch);
            if a {
                match c {
                    0x00 => {
                        let n = self.next_byte(ch) as u16 * 8;
                        let n = if self.v[ch].abs { self.conv(ch, n) } else { n };
                        self.v[ch].per = n;
                        self.v[ch].dur = self.duration(ch);
                        self.v[ch].slide = 0;
                        self.note_start(ch, hw);
                        return Event::Note;
                    }
                    0x04 => {
                        let i = self.next_byte(ch) as usize;
                        self.instrument(ch, i);
                    }
                    0x08 => {
                        let b = self.next_byte(ch);
                        let v = &mut self.v[ch];
                        v.volofs = v.volofs & 0xFF00 | b as u16;
                    }
                    0x0C => {
                        if self.track_step(ch).is_none() {
                            return Event::Stop;
                        }
                    }
                    0x10 => {
                        self.v[ch].rel = true;
                        self.v[ch].dur = self.duration(ch);
                        return Event::Rest;
                    }
                    0x14 | 0x1C => {
                        let i = self.next_byte(ch) as u32;
                        self.v[ch].abs = c == 0x14;
                        let e = self.w(self.pt.wrapping_add(i)) as u32;
                        self.set_env(ch, false, e);
                    }
                    0x18 => {
                        let i = self.next_byte(ch) as u32;
                        let e = self.w(self.vt.wrapping_add(i)) as u32;
                        self.set_env(ch, true, e);
                    }
                    0x20 => {
                        let n1 = self.next_byte(ch);
                        let n2 = self.next_byte(ch);
                        self.portamento(ch, n1, n2);
                        self.note_start(ch, hw);
                        return Event::Note;
                    }
                    // not a command of the engine: skipped
                    _ => {}
                }
            } else {
                match c {
                    0x00..=0x7F => {
                        self.v[ch].per = c as u16 * 8;
                        self.v[ch].dur = self.duration(ch);
                        self.v[ch].slide = 0;
                        self.note_start(ch, hw);
                        return Event::Note;
                    }
                    0x80 => {
                        self.v[ch].rel = true;
                        self.v[ch].dur = self.duration(ch);
                        return Event::Rest;
                    }
                    0x84 => {
                        let n1 = self.next_byte(ch);
                        let n2 = self.next_byte(ch);
                        self.portamento(ch, n1, n2);
                        self.note_start(ch, hw);
                        return Event::Note;
                    }
                    0x88 => {
                        let b = self.next_byte(ch) as u16;
                        let v = &mut self.v[ch];
                        v.volofs = v.volofs & 0xFF00 | b;
                        v.env = (v.env as u16 & 0xFF00 | b) as i16;
                    }
                    0x8C => {
                        let i = self.next_byte(ch) as u32;
                        let r = self.pt.wrapping_add(i);
                        let (s, ve, pe) = (self.w(r), self.w(r + 2), self.w(r + 4));
                        self.instrument(ch, (s / 16) as usize);
                        self.set_env(ch, true, ve as u32);
                        self.set_env(ch, false, pe as u32);
                    }
                    0x90 => {
                        if self.track_step(ch).is_none() {
                            return Event::Stop;
                        }
                    }
                    0x94 => self.filter(hw, true),
                    0x98 => self.filter(hw, false),
                    _ => {}
                }
            }
        }
        self.v[ch].active = false;
        Event::Stop
    }

    /// One envelope step (section 5.4); returns the delta to add, if any.
    fn env_step(&mut self, ch: usize, vol: bool) -> Option<i16> {
        let v = &mut self.v[ch];
        let (cnt, cur, start, reload) = if vol {
            (&mut v.vc, &mut v.venv, v.venvs, v.vr)
        } else {
            (&mut v.pc, &mut v.penv, v.penvs, v.pr)
        };
        *cnt = cnt.wrapping_sub(1);
        if *cnt != 0 {
            return None;
        }
        let mut p = *cur;
        for _ in 0..0x100 {
            let c = match p {
                HOST_HOLD => 0x80,
                _ => self.d.get(p as usize).copied().unwrap_or(0),
            };
            p = p.wrapping_add(1);
            match c {
                0x80 => return None,
                0x81 => p = start,
                _ => {
                    let v = &mut self.v[ch];
                    if vol {
                        v.vc = reload;
                        v.venv = p;
                    } else {
                        v.pc = reload;
                        v.penv = p;
                    }
                    return Some(c as i8 as i16);
                }
            }
        }
        None
    }

    fn release_or_env(&mut self, ch: usize) {
        if self.v[ch].rel {
            if self.v[ch].env >= 0 {
                self.v[ch].env = self.v[ch].env.wrapping_sub(2);
            }
        } else if let Some(d) = self.env_step(ch, true) {
            self.v[ch].env = self.v[ch].env.wrapping_add(d);
        }
    }

    fn write_vol(&mut self, ch: usize, hw: &mut Hw) {
        let v = &self.v[ch];
        let x = v.vol.wrapping_add(v.env).wrapping_sub(MASTER).clamp(0, 64);
        hw.vol(ch, x as u16);
    }

    fn write_per(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.v[ch];
        let pf = ((v.per as u32) << 16 | v.frac as u32).wrapping_add(v.slide);
        v.per = (pf >> 16) as u16;
        v.frac = pf as u16;
        if let Some(d) = self.env_step(ch, false) {
            self.v[ch].per = self.v[ch].per.wrapping_add(d as u16);
        }
        let v = self.v[ch];
        let p = if self.lay.engine_a() && v.abs { v.per } else { self.conv(ch, v.per) };
        hw.per(ch, p);
    }

    /// A voice's step (section 5.1).
    fn voice(&mut self, ch: usize, hw: &mut Hw) {
        if !self.v[ch].active {
            return;
        }
        let v = &mut self.v[ch];
        v.vcnt = v.vcnt.wrapping_sub(1);
        if v.vcnt == 0 && v.vol != v.vtgt {
            v.vol += if v.vol < v.vtgt { 1 } else { -1 };
            v.vcnt = v.vspd;
        }
        v.dur = v.dur.wrapping_sub(1);
        let ev = if v.dur == 0 { Some(self.parse(ch, hw)) } else { None };
        if ev == Some(Event::Stop) {
            return;
        }
        if self.lay.engine_a() {
            self.release_or_env(ch);
            self.write_vol(ch, hw);
            hw.wait(T_WRITE);
            self.write_per(ch, hw);
        } else {
            match ev {
                Some(Event::Note) => {
                    if let Some(d) = self.env_step(ch, true) {
                        self.v[ch].env = self.v[ch].env.wrapping_add(d);
                    }
                }
                Some(Event::Rest) => {}
                _ => self.release_or_env(ch),
            }
            self.write_per(ch, hw);
            hw.wait(T_WRITE);
            self.write_vol(ch, hw);
        }
        hw.wait(T_VOICE);
    }
}

impl Replayer for PaulShields {
    fn format(&self) -> &'static str {
        "Paul Shields"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.songend = false;
            self.init(hw);
        }
        let itmp = self.iacc;
        hw.dmacon(self.dacc);
        hw.wait(T_WRITE);
        self.dacc = 0;
        self.iacc = 0;
        for ch in (0..4).rev() {
            self.voice(ch, hw);
        }
        if itmp != 0 {
            hw.wait(T_LOOP);
        }
        for ch in (0..4).rev() {
            if itmp & 1 << (7 + ch) != 0 {
                let v = self.v[ch];
                hw.loc(ch, v.llc);
                hw.len(ch, v.llen);
                hw.wait(T_WRITE);
            }
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    let mut smp = samples(d, lay)?;
    let silence = m.alloc(4);
    if silence == 0 {
        return None;
    }
    if lay.engine_a() {
        let mut size = WAVE22_LEN;
        for wv in WAVES {
            size += wv.len() as u32;
        }
        let mut a = m.alloc(size);
        if a == 0 {
            return None;
        }
        for k in 0..7 {
            let n = if k < 6 { WAVES[k].len() as u32 } else { WAVE22_LEN };
            if k < 6 {
                for (i, &b) in WAVES[k].iter().enumerate() {
                    m.bus.ram[a as usize + i] = b as u8;
                }
            }
            smp.push(Sample { lc: a, len: (n / 2) as u16, llc: a, llen: (n / 2) as u16, host: true, ..Sample::default() });
            a += n;
        }
    }
    Some(Box::new(PaulShields {
        d: d.to_vec(),
        lay,
        base,
        smp,
        silence,
        pt: 0,
        vt: 0,
        v: Default::default(),
        dacc: 0,
        iacc: 0,
        started: false,
        songend: false,
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

    fn words(d: &mut [u8], o: usize, ws: &[u16]) {
        for (i, &v) in ws.iter().enumerate() {
            p16(d, o + 2 * i, v);
        }
    }

    /// A synthetic layout-FF module: voices 3-1 rest in a loop, voice 0 plays
    /// instrument 0 (8 bytes at 0x292, default volume 10, no loop) with the pattern
    /// bytes `pat` (at 0x280), then stops.
    fn module(pat: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x29A];
        words(&mut d, 0x16, &[4, 10, 0, 0, 0]);
        // PT, VT, then (vol, list) for channels 3, 2, 1, 0
        words(&mut d, 0x200, &[0x214, 0x218, 64, 0x250, 64, 0x250, 64, 0x250, 64, 0x254]);
        words(&mut d, 0x214, &[0x0214, 0x0216, 0x0240]);
        d[0x240] = 1;
        d[0x241] = 0x80;
        words(&mut d, 0x250, &[0x0260, 0xFFF0, 0x0280, 0xFFF4]);
        d[0x260..0x265].copy_from_slice(&[0x10, 0, 0x10, 0, 0x0C]);
        d[0x280..0x280 + pat.len()].copy_from_slice(pat);
        p16(&mut d, 0x290, 0xFFF0);
        d[0x292..0x29A].copy_from_slice(&[0, 40, 80, 120, 0, 0x80, 0xC0, 0xF0]);
        d
    }

    fn play(d: Vec<u8>, ticks: usize) -> (bool, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("ps.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Paul Shields");
        let mut out = vec![0f32; 2 * (960 * ticks + 100)];
        e.render(&mut out);
        let end = e.songend();
        (end, e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_the_three_layouts() {
        let d = module(&[0x04, 0, 0x00, 12, 3, 0x0C]);
        assert_eq!(layout(&d), Some(Layout::FF));
        assert_eq!(sample_start(&d, Layout::FF), Some(0x292));
        assert!(detect(&d));
        // the sample data must fit
        assert!(!detect(&d[..0x298]));
        let mut e = d.clone();
        e[3] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        p16(&mut e, 0x214, 0x0215);
        assert!(!detect(&e));
        // layout 00: the first list preceded by a track command
        let mut b = d.clone();
        words(&mut b, 0x200, &[0x300, 64, 0x252, 64, 0x252, 64, 0x252, 64, 0x252]);
        p16(&mut b, 0x250, 0xFFEC);
        assert_eq!(layout(&b), Some(Layout::L00));
        // layout 01: the header at 0xA0
        let mut c = vec![0u8; 0x220];
        words(&mut c, 0xA0, &[0xB4, 0xB6, 30, 0x100, 30, 0x100, 30, 0x100, 30, 0x100]);
        words(&mut c, 0xB4, &[0x00B4, 0x00B6]);
        assert_eq!(layout(&c), Some(Layout::L01));
        assert!(!detect(&c[..0x200]));
    }

    #[test]
    fn plays_a_note_then_its_loop_and_stops() {
        let (end, log) = play(module(&[0x04, 0, 0x00, 12, 3, 0x0C]), 6);
        assert!(end);
        // init, then each tick's DMACON: off-mask first, the note's DMA on the next tick
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..5], [0x000F, 0x0000, 0x0001, 0x8201, 0x0000]);
        // note 12 (per 96): one octave up from the table's first entry
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per[0], PITCH[0] >> 1);
        // the resting voices: release below 0, VOL 0, the table's lowest period
        assert_eq!(reg(&log, 0xDFF0D6)[0], PITCH[0]);
        assert_eq!(reg(&log, 0xDFF0D8)[0], 0);
        // the default volume, the sample, then the silence loop of 2 words
        assert_eq!(reg(&log, 0xDFF0A8)[0], 10);
        assert_eq!(reg(&log, 0xDFF0A4), [4, 2]);
        // voice 0 stops in tick 4: three ticks of writes
        assert_eq!(per.len(), 3);
    }

    #[test]
    fn portamento_slides_by_the_divided_distance() {
        // from note 0 to note 1 (8 steps) over 3 ticks: 2 + 0xAAAA/65536 a tick, the
        // first step in the note's own tick
        let (_, log) = play(module(&[0x04, 0, 0x20, 0, 1, 3, 0x0C]), 6);
        assert_eq!(reg(&log, 0xDFF0A6), [PITCH[2], PITCH[5], PITCH[7]]);
    }

    #[test]
    fn notes_below_the_table_extend_it_by_octaves() {
        // note 0 with a fine tune of -8: one octave below entry 88
        let mut d = module(&[0x04, 0, 0x00, 0, 3, 0x0C]);
        p16(&mut d, 0x1E, 0xFFF8);
        let (_, log) = play(d, 3);
        assert_eq!(reg(&log, 0xDFF0A6)[0], PITCH[88] * 2);
    }
}
