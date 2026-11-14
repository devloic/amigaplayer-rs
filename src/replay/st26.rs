//! SoundTracker 2.6 (tag `MTN\0`) and IceTracker (`IT10`) modules, after NostalgicPlayer's
//! ModTracker player (Copyright (c) Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer): its identification (`ModTrackerIdentifier.TestModule`: the
//! tag at offset 1464) and its loader (`LoadSoundTracker26`: the number of tracks must be
//! the highest track referenced plus one, and no sample may miss more than 512 bytes).
//!
//! The header is a 31-instrument one: the title, 31 samples, the song length, the number
//! of tracks and then 128 positions of four track numbers each, one per voice, and the
//! tag. The tracks follow, 64 rows of a long each, and the samples, played where they lie in
//! guest memory.
//!
//! The replay is written from black-box measurements of the EP206 "Soundtracker 2.6"
//! Eagleplayer (its Paula write logs over the corpus and modified copies) and general
//! knowledge of the NoiseTracker effect set: Fxy with both digits set alternates the speeds
//! x and y row by row, C sets Paula's volume only, E sets the filter LED. Every write is at
//! the reference's colour clock: the player calls the host for each write, at the previous
//! call's end plus half the 68000 cycles of the code paths taken (`COSTS`, `VCOSTS`, fitted to
//! every write gap of the 26 corpus files), and the song starts after `start_clocks`.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN_ST26: Open = Open { name: "SoundTracker 2.6", detect: |d| kind(d) == Some(Kind::St26), new: |d, b, m| new(d, b, m, Kind::St26) };
pub const OPEN_ICE: Open = Open { name: "IceTracker", detect: |d| kind(d) == Some(Kind::Ice), new: |d, b, m| new(d, b, m, Kind::Ice) };

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    St26,
    Ice,
}

const HEADER: usize = 1468;

fn kind(d: &[u8]) -> Option<Kind> {
    if d.len() < HEADER {
        return None;
    }
    let k = match &d[1464..1468] {
        b"MTN\0" => Kind::St26,
        b"IT10" => Kind::Ice,
        _ => return None,
    };
    let (song_len, ntracks) = (d[950] as usize, d[951] as usize);
    // NostalgicPlayer reads the positions of up to 255 song positions from a table of 128
    // (and fails past it); the table has room for 128, and none for 0
    if song_len == 0 || song_len > 128 {
        return None;
    }
    if d[952..1464].iter().copied().max().unwrap() as usize + 1 != ntracks {
        return None;
    }
    let mut at = HEADER + ntracks * 256;
    if d.len() < at {
        return None;
    }
    for i in 0..31 {
        let o = 20 + i * 30;
        let len = u16::from_be_bytes([d[o + 22], d[o + 23]]) as usize * 2;
        if len != 0 {
            if d.len().saturating_sub(at) + 512 < len {
                return None;
            }
            at += len;
        }
    }
    Some(k)
}

/// The periods of three octaves (C-1 to B-3).
const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
    226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// The vibrato's half sine.
const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24,
];

/// 68000 cycles of the code paths (the labels passed to `Clk::k`); a path not listed costs nothing.
const COSTS: &[(&str, i32)] = &[
    ("arp.0", 17), ("arp.1", 6), ("arp.i", 52), ("dma", 234), ("down.clip", 40), ("ev", 215), ("ev.arp", 259),
    ("ev.down", 206), ("ev.per", 58), ("ev.up", 187), ("ev.xo", 129), ("evt.1", -28), ("evt.2", -28), ("evt.3", -12),
    ("evt.4", 34), ("evt.a", 391), ("evt.none", -28), ("evt.x", 17), ("fx.b", 141), ("fx.c", 406), ("fx.d", 96),
    ("fx.e.set", 131), ("fx.f", 242), ("fx.f2", 20), ("fx.none", 131), ("rep", 202), ("rep.len", 211),
    ("rep.len0", 4), ("rv", 304), ("rv.len", 211), ("rv.len0", 4), ("rv.loc", 206), ("rv.note", 584),
    ("rv.note.lp", 14), ("rv.ns", -21), ("rv.ns.lp", -31), ("rv.ns.tp", -31), ("rv.per", 194), ("rv.smp", 550),
    ("rv.smp.lp", 44), ("rv.tp", 520), ("rv.tp.down", 16), ("t", 394), ("t.row", 19), ("tp.down", 284),
    ("tp.end", 36), ("tp.speed", 28), ("tp.up", 284), ("vib.neg", -10), ("vib.set", 391), ("vs.bot", 16),
    ("vs.down", 8),
];
/// The cycles a path costs more on one voice (label, voice, cycles).
const VCOSTS: &[(&str, u8, i32)] = &[
    ("fx.c", 1, 23), ("fx.c", 2, 46), ("fx.c", 3, 36), ("rep", 3, 8), ("rv.nosmp", 0, 23), ("rv.note", 1, 31),
    ("rv.note", 2, 62), ("rv.note", 3, 70), ("rv.smp", 2, 23), ("rv.smp", 3, 13), ("vs.down", 2, 23),
    ("vs.down", 3, 13),
];

/// The original's clock: it writes Paula through the host's ENPP calls, each at the previous call's
/// end plus half the 68000 cycles of the code paths taken since.
struct Clk {
    cy: i64,
    tl: u64,
    /// The voice whose code runs (4: none).
    v: u8,
}

impl Clk {
    fn new() -> Clk {
        Clk { cy: 0, tl: 0, v: 4 }
    }

    fn k(&mut self, l: &'static str) {
        if let Ok(i) = COSTS.binary_search_by(|e| e.0.cmp(l)) {
            self.cy += COSTS[i].1 as i64;
        }
        if self.v < 4 {
            if let Ok(i) = VCOSTS.binary_search_by(|e| (e.0, e.1).cmp(&(l, self.v))) {
                self.cy += VCOSTS[i].2 as i64;
            }
        }
    }

    fn start(&mut self, hw: &mut Hw) {
        self.tl = hw.bus.clock;
        self.cy = 0;
        self.v = 4;
    }

    fn at(&mut self, hw: &mut Hw) {
        let t = self.tl + self.cy.max(0) as u64 / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
        self.cy = 0;
    }

    /// A write of CIA-A's port, which is no host call: the clock of the next call is still counted
    /// from the last one.
    fn cia(&mut self, hw: &mut Hw, v: u8) {
        let t = self.tl + self.cy.max(0) as u64 / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
        hw.bus.write_byte(0xBFE001, v);
    }

    fn done(&mut self, hw: &mut Hw) {
        self.tl = hw.bus.clock;
    }

    fn dmacon(&mut self, hw: &mut Hw, v: u16) {
        self.at(hw);
        hw.dmacon(v);
        if v & 0x8000 != 0 {
            // the host's wait for the audio DMA: to the tenth raster line after the call's
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

#[derive(Clone, Copy, Default)]
struct Smp {
    looped: bool,
    start: u32,
    len: u16,
    vol: u8,
    loop_start: u32,
    replen: u16,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    looped: bool,
    // the cell of the row
    note: u16,
    cmd: u8,
    arg: u8,
    // the sample
    start: u32,
    len: u16,
    loop_start: u32,
    replen: u16,
    vol: u8,
    period: u16,
    // tone portamento
    wanted: u16,
    down: bool,
    speed: u8,
    // vibrato
    vibcmd: u8,
    vibpos: u8,
}

struct St26 {
    name: &'static str,
    title: String,
    d: Vec<u8>,
    smp: [Smp; 32],
    song_len: usize,
    pos: usize,
    row: usize,
    speed: u8,
    /// The other speed of an Fxy, taken by every other row.
    alt: Option<u8>,
    counter: u8,
    brk: bool,
    jump: Option<usize>,
    v: [Voice; 4],
    songend: bool,
    c: Clk,
}

fn stub_libraries(m: &mut Machine) {
    for (name, trap) in [("mpega.library", 0xA061), ("asyncio.library", 0xA062)] {
        let base = crate::os::make_library(m, name, crate::os::LibKind::Stub, 200, 0x100, 40);
        for i in 1..=200 {
            m.bus.w16(base - i * 6, trap);
        }
    }
}

/// The colour clocks from the player's start to its first tick, measured on the reference
/// (modified copies of one module, exact on the 26 corpus files and on random copies): a base, per
/// sample that has data, and per step of a scan of the 512 track numbers from the last to the
/// first that keeps the highest seen so far (counted when a number is at least as high, the
/// starting value 0 included, so trailing zeros count); IceTracker modules take a little longer.
fn start_clocks(d: &[u8], k: Kind) -> u64 {
    let mut hi = 0;
    let mut steps = 0;
    for &t in d[952..1464].iter().rev() {
        if t >= hi {
            hi = t;
            steps += 1;
        }
    }
    let samples = (0..31).filter(|i| d[20 + i * 30 + 22..20 + i * 30 + 24] != [0, 0]).count() as u64;
    23_997 + 14 * steps + 12 * samples + if k == Kind::Ice { 20 } else { 0 }
}

fn new(d: &[u8], base: u32, m: &mut Machine, k: Kind) -> Option<Box<dyn Replayer>> {
    if kind(d) != Some(k) {
        return None;
    }
    let mut smp = [Smp::default(); 32];
    let mut at = base + (HEADER + d[951] as usize * 256) as u32;
    for (i, s) in smp.iter_mut().enumerate().skip(1) {
        let o = 20 + (i - 1) * 30;
        let w = |x: usize| u16::from_be_bytes([d[o + x], d[o + x + 1]]);
        let (len, rep, replen) = (w(22), w(26), w(28));
        s.start = at;
        s.vol = d[o + 25];
        s.looped = rep != 0;
        if rep != 0 {
            s.loop_start = at + 2 * rep as u32;
            s.len = rep.wrapping_add(replen);
        } else {
            s.loop_start = at;
            s.len = len;
        }
        s.replen = if replen > 1 { replen } else { 0 };
        at += 2 * len as u32;
    }
    stub_libraries(m);
    m.bus.clock += start_clocks(d, k);
    let name = if k == Kind::St26 { OPEN_ST26.name } else { OPEN_ICE.name };
    let title = String::from_utf8_lossy(&d[..20]).trim_end_matches('\0').to_string();
    Some(Box::new(St26 {
        name,
        title,
        d: d[..HEADER + d[951] as usize * 256].to_vec(),
        smp,
        song_len: d[950] as usize,
        pos: 0,
        row: 0,
        speed: 6,
        alt: None,
        counter: 0,
        brk: false,
        jump: None,
        v: Default::default(),
        songend: false,
        c: Clk::new(),
    }))
}

impl St26 {
    /// A new row: the voices' cells, the DMA of the voices that start a sample, their repeats.
    fn row(&mut self, hw: &mut Hw) {
        self.c.k("row");
        let mut mask = 0u16;
        for k in 0..4 {
            self.c.v = k as u8;
            self.c.k("rv");
            let t = self.d[952 + self.pos * 4 + k] as usize;
            let o = HEADER + t * 256 + self.row * 4;
            let c = [self.d[o], self.d[o + 1], self.d[o + 2], self.d[o + 3]];
            let s = ((c[0] & 0xF0) | (c[2] >> 4)) as usize;
            let v = &mut self.v[k];
            v.note = u16::from_be_bytes([c[0] & 0x0F, c[1]]);
            v.cmd = c[2] & 0x0F;
            v.arg = c[3];
            if s != 0 && s < 32 {
                self.c.k("rv.smp");
                let sm = self.smp[s];
                if sm.looped {
                    self.c.k("rv.smp.lp");
                }
                if sm.replen == 0 {
                    self.c.k("rv.smp.rl0");
                }
                v.looped = sm.looped;
                v.start = sm.start;
                v.len = sm.len;
                v.loop_start = sm.loop_start;
                v.replen = sm.replen;
                v.vol = sm.vol;
                let vol = v.vol as u16;
                self.c.vol(hw, k, vol);
            }
            let v = &mut self.v[k];
            if s == 0 {
                self.c.k("rv.nosmp");
                if v.note != 0 {
                    self.c.k(if v.cmd == 3 { "rv.ns.tp" } else if v.looped { "rv.ns.lp" } else { "rv.ns" });
                }
            }
            if v.note != 0 {
                if v.cmd == 3 {
                    self.c.k("rv.tp");
                    v.wanted = v.note;
                    v.down = v.note < v.period;
                    if v.down {
                        self.c.k("rv.tp.down");
                    }
                    if v.note == v.period {
                        self.c.k("rv.tp.eq");
                        v.wanted = 0;
                    }
                } else {
                    self.c.k("rv.note");
                    if v.looped {
                        self.c.k("rv.note.lp");
                    }
                    v.period = v.note;
                    v.vibpos = 0;
                    let (start, len, per) = (v.start, v.len, v.period);
                    self.c.dmacon(hw, 1 << k);
                    self.c.k("rv.loc");
                    self.c.loc(hw, k, start);
                    self.c.k("rv.len");
                    if len == 0 {
                        self.c.k("rv.len0");
                    }
                    self.c.len(hw, k, len);
                    self.c.k("rv.per");
                    self.c.per(hw, k, per);
                    mask |= 1 << k;
                }
            }
            self.row_effect(k, hw);
        }
        self.c.v = 4;
        self.c.k("dma");
        self.c.dmacon(hw, 0x8000 | mask);
        for k in (0..4).rev() {
            self.c.v = k as u8;
            self.c.k("rep");
            let (a, l) = (self.v[k].loop_start, self.v[k].replen);
            self.c.loc(hw, k, a);
            self.c.k("rep.len");
            if l == 0 {
                self.c.k("rep.len0");
            }
            self.c.len(hw, k, l);
        }
        self.c.v = 4;
        self.c.k("next");
        self.row += 1;
        if let Some(p) = self.jump.take() {
            self.c.k("next.jump");
            // a jump back (or to itself) is the song's end
            if p <= self.pos {
                self.songend = true;
            }
            self.pos = p;
            self.row = 0;
            self.brk = false;
            if self.pos >= self.song_len {
                self.pos = 0;
                self.songend = true;
            }
        } else if self.brk || self.row >= 64 {
            self.c.k("next.pos");
            self.brk = false;
            self.row = 0;
            self.pos += 1;
            if self.pos >= self.song_len {
                self.c.k("next.wrap");
                self.pos = 0;
                self.songend = true;
            }
        }
    }

    /// The commands that act on the row's tick.
    fn row_effect(&mut self, k: usize, hw: &mut Hw) {
        let v = &mut self.v[k];
        match v.cmd {
            0xB => {
                self.c.k("fx.b");
                self.jump = Some(v.arg as usize);
            }
            0xC => {
                self.c.k("fx.c");
                // Paula's volume only: a volume slide starts again from the sample's
                let vol = v.arg.min(64) as u16;
                self.c.vol(hw, k, vol);
            }
            0xD => {
                self.c.k("fx.d");
                self.brk = true;
            }
            0xE => {
                // the LED bit of CIA-A's port cleared, then set again for an odd argument
                self.c.k("fx.e");
                let set = v.arg & 1 != 0;
                let pra = hw.bus.ciaa.pra;
                self.c.cia(hw, pra & !2);
                if set {
                    self.c.k("fx.e.set");
                    self.c.cia(hw, (pra & !2) | 2);
                }
            }
            0xF => {
                self.c.k("fx.f");
                let (x, y) = (v.arg >> 4, v.arg & 15);
                if x != 0 && y != 0 {
                    // two speeds, the rows taking them in turn (the first one first)
                    self.c.k("fx.f2");
                    self.speed = x;
                    self.alt = Some(y);
                    self.counter = 0;
                } else if v.arg & 0x1F != 0 {
                    self.speed = v.arg & 0x1F;
                    self.alt = None;
                    self.counter = 0;
                }
            }
            _ => self.c.k("fx.none"),
        }
    }

    /// The commands of the ticks between rows.
    fn effects(&mut self, k: usize, hw: &mut Hw) {
        self.c.v = k as u8;
        self.c.k("ev");
        let counter = self.counter;
        let v = &mut self.v[k];
        let per = match v.cmd {
            0 if v.arg != 0 => {
                self.c.k("ev.arp");
                let n = match counter % 3 {
                    1 => {
                        self.c.k("arp.1");
                        v.arg >> 4
                    }
                    2 => {
                        self.c.k("arp.2");
                        v.arg & 15
                    }
                    _ => {
                        self.c.k("arp.0");
                        0
                    }
                };
                if n == 0 {
                    v.period
                } else {
                    // the note's place in the table, searched from the lowest note
                    let i = PERIODS.iter().position(|&p| v.period >= p).unwrap_or(35);
                    for _ in 0..=i {
                        self.c.k("arp.i");
                    }
                    PERIODS.get(i + n as usize).copied().unwrap_or(0)
                }
            }
            1 => {
                self.c.k("ev.up");
                v.period = v.period.wrapping_sub(v.arg as u16);
                if v.period < 113 {
                    self.c.k("up.clip");
                    v.period = 113;
                }
                v.period
            }
            2 => {
                self.c.k("ev.down");
                v.period = v.period.wrapping_add(v.arg as u16);
                if v.period > 856 {
                    self.c.k("down.clip");
                    v.period = 856;
                }
                v.period
            }
            3 => {
                self.c.k("ev.tp");
                if v.arg != 0 {
                    self.c.k("tp.speed");
                    v.speed = v.arg;
                    v.arg = 0;
                }
                if v.wanted == 0 {
                    // the slide is done: no write
                    self.c.k("ev.tp.done");
                    return;
                }
                {
                    if v.down {
                        self.c.k("tp.down");
                        v.period = v.period.wrapping_sub(v.speed as u16);
                        if v.period <= v.wanted {
                            self.c.k("tp.end");
                            v.period = v.wanted;
                            v.wanted = 0;
                        }
                    } else {
                        self.c.k("tp.up");
                        v.period = v.period.wrapping_add(v.speed as u16);
                        if v.period >= v.wanted {
                            self.c.k("tp.end");
                            v.period = v.wanted;
                            v.wanted = 0;
                        }
                    }
                }
                v.period
            }
            4 => {
                self.c.k("ev.vib");
                if v.arg != 0 {
                    self.c.k("vib.set");
                    v.vibcmd = v.arg;
                }
                let sn = SINE[(v.vibpos >> 2) as usize & 31];
                for _ in 0..sn.count_ones() {
                    self.c.k("mu.s");
                }
                for _ in 0..(v.vibcmd & 15).count_ones() {
                    self.c.k("mu.d");
                }
                let a = (sn as u16 * (v.vibcmd & 15) as u16) >> 6;
                if v.vibpos & 0x80 != 0 {
                    self.c.k("vib.neg");
                }
                let p = if v.vibpos & 0x80 != 0 { v.period.wrapping_sub(a) } else { v.period.wrapping_add(a) };
                v.vibpos = v.vibpos.wrapping_add((v.vibcmd >> 2) & 0x3C);
                p
            }
            _ => {
                self.c.k("ev.per");
                self.c.k(if v.cmd == 0 { "ev.x0" } else { "ev.xo" });
                v.period
            }
        };
        self.c.per(hw, k, per);
        let v = &mut self.v[k];
        self.c.k(match v.cmd {
            0 if v.arg == 0 => "evt.none",
            0 => "evt.0",
            1 => "evt.1",
            2 => "evt.2",
            3 => "evt.3",
            4 => "evt.4",
            0xA => "evt.a",
            _ => "evt.x",
        });
        if v.cmd == 0xA {
            self.c.k("ev.vs");
            let up = v.arg >> 4;
            if up != 0 {
                self.c.k("vs.up");
                if v.vol + up > 64 {
                    self.c.k("vs.top");
                }
                v.vol = (v.vol + up).min(64);
            } else {
                self.c.k("vs.down");
                if v.vol < v.arg & 15 {
                    self.c.k("vs.bot");
                }
                v.vol = v.vol.saturating_sub(v.arg & 15);
            }
            let vol = v.vol as u16;
            self.c.vol(hw, k, vol);
        }
    }
}

impl Replayer for St26 {
    fn format(&self) -> &'static str {
        self.name
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.c.start(hw);
        self.c.k("t");
        self.counter += 1;
        if self.counter >= self.speed {
            self.c.k("t.row");
            self.counter = 0;
            if let Some(a) = self.alt {
                self.alt = Some(self.speed);
                self.speed = a;
            }
            self.row(hw);
        } else {
            for k in 0..4 {
                self.effects(k, hw);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A module with one 64-byte sample (volume 48, no loop), `ntracks` tracks with `cells`
    /// at (track, row), and `positions` (four tracks each).
    fn module(tag: &[u8; 4], ntracks: usize, positions: &[[u8; 4]], cells: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
        let mut d = vec![0u8; HEADER];
        d[..8].copy_from_slice(b"st26 mod");
        let o = 20;
        d[o..o + 4].copy_from_slice(b"smp1");
        d[o + 22..o + 24].copy_from_slice(&32u16.to_be_bytes());
        d[o + 25] = 48;
        d[o + 28..o + 30].copy_from_slice(&1u16.to_be_bytes());
        for i in 1..31 {
            d[20 + i * 30 + 28..20 + i * 30 + 30].copy_from_slice(&1u16.to_be_bytes());
        }
        d[950] = positions.len() as u8;
        d[951] = ntracks as u8;
        for (p, t) in positions.iter().enumerate() {
            d[952 + p * 4..956 + p * 4].copy_from_slice(t);
        }
        d[1464..1468].copy_from_slice(tag);
        let mut tr = vec![0u8; ntracks * 256];
        for &(t, r, c) in cells {
            tr[t * 256 + r * 4..t * 256 + r * 4 + 4].copy_from_slice(&c);
        }
        d.extend(tr);
        d.extend((0..64).map(|i| (i * 4) as u8));
        d
    }

    fn cell(p: u16, s: u8, e: u8, a: u8) -> [u8; 4] {
        [(s & 0xf0) | (p >> 8) as u8, p as u8, (s << 4) | e, a]
    }

    #[test]
    fn identifies_the_tags() {
        let d = module(b"MTN\0", 2, &[[0, 1, 0, 1]], &[]);
        assert_eq!(kind(&d), Some(Kind::St26));
        let mut ice = d.clone();
        ice[1464..1468].copy_from_slice(b"IT10");
        assert_eq!(kind(&ice), Some(Kind::Ice));
        // the number of tracks must be the highest one referenced plus one
        let mut bad = d.clone();
        bad[951] = 3;
        assert_eq!(kind(&bad), None);
        // no tag, song length 0, too short, missing most of a sample
        bad = d.clone();
        bad[1464..1468].copy_from_slice(b"M.K.");
        assert_eq!(kind(&bad), None);
        bad = d.clone();
        bad[950] = 0;
        assert_eq!(kind(&bad), None);
        assert_eq!(kind(&d[..1467]), None);
        bad = d.clone();
        bad[20 + 22..20 + 24].copy_from_slice(&1000u16.to_be_bytes());
        assert_eq!(kind(&bad), None);
        // and the 15-instrument SoundTrackers leave them alone
        assert_eq!(crate::replay::soundtracker::variant(&d), None);
    }

    #[test]
    fn plays_from_the_samples_in_place() {
        let d = module(b"IT10", 2, &[[1, 0, 0, 0]], &[(1, 0, cell(428, 1, 0xc, 0x20))]);
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.ice"), d.clone()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "IceTracker");
        let mut out = vec![0f32; 2 * 960 * 10];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        assert!(reg(0xDFF0A6).contains(&428));
        assert!(reg(0xDFF0A4).contains(&32));
        assert!(reg(0xDFF0A8).contains(&0x20));
        // AUDxLC is the sample's own address in the module: after the two tracks
        let lc: Vec<usize> = reg(0xDFF0A0).iter().zip(reg(0xDFF0A2)).map(|(&h, l)| ((h as usize) << 16) | l as usize).collect();
        assert!(lc.iter().any(|&at| at != 0 && e.m.bus.ram[at + 2..at + 8] == d[HEADER + 512 + 2..HEADER + 512 + 8]));
        assert_eq!(crate::open::detect(&d).as_deref(), Some("IceTracker"));
    }

    #[test]
    fn starts_after_the_scan_of_the_tracks() {
        // one sample; the scan from the end: 508 trailing zeros, the 1s of indices 3 and 1, not the 0s
        let d = module(b"MTN\0", 2, &[[0, 1, 0, 1]], &[]);
        assert_eq!(start_clocks(&d, Kind::St26), 23_997 + 14 * 510 + 12);
        let mut ice = d.clone();
        ice[1464..1468].copy_from_slice(b"IT10");
        assert_eq!(start_clocks(&ice, Kind::Ice), 23_997 + 14 * 510 + 12 + 20);
        // a number at least as high as the highest after it is a step
        let mut up = d.clone();
        up[952] = 1;
        assert_eq!(start_clocks(&up, Kind::St26), 23_997 + 14 * 511 + 12);
    }
}
