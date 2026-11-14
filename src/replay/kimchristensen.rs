//! Kim Christensen (Eagleplayer "Kim Christensen", module prefix `kim.`; the driver of the game
//! Rotor).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes and
//! cycle costs of the 68k reference player; no original code and nothing embedded. A module is a
//! raw memory image of a game: code, tables, positions, 16-row patterns of four channels, 32-byte
//! instrument records (four-segment volume envelope, two-segment pitch envelope, vibrato) and a
//! sample list, with absolute addresses that are turned into file offsets by locating the
//! signature words. The period table is the player's own (43 values, given in the spec); notes past
//! it read the driver's state words.
//!
//! Every write is placed at the clock of the original's code from the spec's cycle ledger,
//! including the two waits of nine raster-line changes after the envelopes.
//!
//! Choices where the spec is silent (all safe, none can panic or hang): a period index of 55 or
//! more reads as 0; reads past the end of the file give 0; a position list made only of 0xFF
//! entries stops skipping after 256 steps; an instrument's sample number past the list reads
//! address 0.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Kim Christensen", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// The first tick's colour clock in the reference host (its entry into the player after its own
/// start-up): a base for one sample entry in the sample check, 26 per word searched before the
/// first signature word and between the last two, and the cost of each further sample entry. The
/// phase of this clock in the raster line decides the waits of spec 4.4, so it is modelled from
/// the reference's runs (it is a property of the host, not of the format); the sample term is
/// measured for 1 to 5 entries and continued at the last step beyond. The count runs in 68000
/// cycles, 52 a word searched, one less over an odd number of words between the last two
/// signature words: with such a gap the clock lands a colour clock earlier (the bases are for an
/// even one; the files of 1, 3 and 4 entries measured all have a gap of 1 word).
fn first_tick_at(searched: u64, gap_odd: bool, nsmp: u64) -> u64 {
    const BASE: [u64; 6] = [74053, 74053, 74080, 74116, 74150, 74184];
    let n = nsmp.min(1000) as usize;
    let b = if n < BASE.len() { BASE[n] } else { BASE[5] + 34 * (n as u64 - 5) };
    b + 26 * searched - gap_odd as u64
}

const PERIODS: [u16; 43] = [
    0, 0x087, 0x08F, 0x097, 0x0A0, 0x0AA, 0x0B4, 0x0BE, 0x0CA, 0x0D6, 0x0E2, 0x0F0, 0x0FE, 0x10D, 0x11D, 0x12E,
    0x140, 0x153, 0x168, 0x17D, 0x194, 0x1AC, 0x1C5, 0x1E0, 0x1FC, 0x21A, 0x23A, 0x25C, 0x280, 0x2A8, 0x2D0,
    0x30E, 0x328, 0x358, 0x38A, 0x3C0, 0x3F8, 0x434, 0x474, 0x4B8, 0x500, 0x550, 0x5A0,
];

const LV: [u64; 4] = [177, 194, 201, 228];
const VP: [u64; 4] = [163, 194, 225, 233];
const PE: [u64; 4] = [236, 267, 298, 306];
const EVP: [u64; 4] = [151, 182, 213, 221];
const EPN: [u64; 3] = [262, 269, 296];

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn word(d: &[u8], o: usize) -> u16 {
    ((byte(d, o) as u16) << 8) | byte(d, o + 1) as u16
}

fn long(d: &[u8], o: usize) -> u32 {
    ((word(d, o) as u32) << 16) | word(d, o + 2) as u32
}

/// The next offset at or after `from` (even steps) whose word is `w`.
fn find(d: &[u8], from: usize, w: u16) -> Option<usize> {
    let mut o = from & !1;
    while o + 2 <= d.len() {
        if word(d, o) == w {
            return Some(o);
        }
        o += 2;
    }
    None
}

fn detect(d: &[u8]) -> bool {
    if d.len() < 1800 {
        return false;
    }
    let Some(first) = (0..100).map(|i| i * 2).find(|&o| word(d, o) == 0x207C) else {
        return false;
    };
    let mut budget = 800u32;
    let mut o = first + 2;
    for w in [0x0680u16, 0xE341, 0x227C, 0x0680, 0x0087] {
        loop {
            if o + 2 > d.len() {
                return false;
            }
            if word(d, o) == w {
                o += 2;
                break;
            }
            if budget == 0 {
                return false;
            }
            budget -= 1;
            o += 2;
        }
    }
    true
}

/// Where the data is, from the signature words (spec 1.2); None when the layout is not found or
/// the sample check of 1.3 fails.
struct Layout {
    origin: u32,
    pos: usize,
    pat: usize,
    ins: usize,
    smp: usize,
    /// Words searched before the first signature word and between the last two.
    searched: u64,
    /// Whether the words searched between the last two signature words are an odd number.
    gap_odd: bool,
    nsmp: u64,
}

fn layout(d: &[u8]) -> Option<Layout> {
    let o = find(d, 0, 0x207C)?;
    let first_sig = o;
    let a_pos = long(d, o + 2);
    let o = find(d, o + 6, 0x0680)?;
    let a_pat = long(d, o + 2);
    let o = find(d, o + 6, 0xE341)?;
    let a_per = long(d, o.checked_sub(4)?);
    let o = find(d, o + 2, 0x227C)?;
    let a_ins = long(d, o + 2);
    let o = find(d, o + 6, 0x0680)?;
    let a_smp = long(d, o + 2);
    let after_smp = o + 6;
    let o = find(d, after_smp, 0x0087)?;
    let t = o.checked_sub(2)?;
    let origin = a_per.wrapping_sub(t as u32);
    let off = |a: u32| a.wrapping_sub(origin) as usize;
    let mut l = Layout { origin, pos: off(a_pos), pat: off(a_pat), ins: off(a_ins), smp: off(a_smp), searched: (first_sig / 2 + (o - after_smp) / 2) as u64, gap_odd: (o - after_smp) / 2 % 2 == 1, nsmp: 0 };
    // the sample list check
    let first = long(d, l.smp);
    if first == 0 || first & 0x8000_0000 != 0 {
        return None;
    }
    let nsmp = (first.wrapping_sub(a_smp) / 6) & 0xFFFF;
    l.nsmp = nsmp as u64;
    let mut end: i32 = 0;
    for i in 0..nsmp as usize {
        let a = long(d, l.smp + i * 6) as i32;
        let len = word(d, l.smp + i * 6 + 4) as i32;
        if end <= a {
            end = a.wrapping_add(len);
        }
    }
    if end.wrapping_sub(origin as i32) > d.len() as i32 {
        return None;
    }
    Some(l)
}

/// The signed 32-bit quotient by a byte as the 68000 divides: 0 for a zero divisor, the dividend's
/// low half when the quotient does not fit 16 bits.
fn div(x: i32, d: u8) -> u16 {
    if d == 0 {
        return 0;
    }
    let q = x / d as i32;
    if (-32768..=32767).contains(&q) {
        q as u16
    } else {
        x as u16
    }
}

#[derive(Clone, Copy, Default)]
struct Chan {
    start: u32,
    len: u16,
    rstart: u32,
    rlen: u16,
    vol: u16,
    per: u16,
    vcount: [u8; 4],
    vdelta: [u16; 4],
    pdelay: [u8; 2],
    pcount: [u8; 2],
    pdelta: [u16; 2],
    vdelay: u8,
    vc: u8,
    vreload: u8,
    vstep: u16,
}

pub struct KimChristensen {
    d: Vec<u8>,
    base: u32,
    l: Layout,
    speed: u8,
    hdr1: u8,
    repeat: u8,
    last: u8,
    pos: u8,
    row: u8,
    counter: u8,
    dma: u16,
    loopmask: u16,
    ch: [Chan; 4],
    songend: bool,
}

impl KimChristensen {
    fn build(d: &[u8], base: u32) -> Option<Self> {
        if !detect(d) {
            return None;
        }
        let l = layout(d)?;
        let mut h = long(d, l.pos.wrapping_sub(4));
        if h == 0 {
            h = 0x0200_0077;
        }
        let [speed, hdr1, repeat, last] = h.to_be_bytes();
        Some(KimChristensen {
            d: d.to_vec(),
            base,
            l,
            speed,
            hdr1,
            repeat,
            last,
            pos: repeat,
            row: 0,
            counter: 0,
            dma: 0,
            loopmask: 0,
            ch: [Chan::default(); 4],
            songend: false,
        })
    }

    fn empty_addr(&self) -> u32 {
        self.base.wrapping_add(self.l.pos as u32).wrapping_sub(8)
    }

    /// The period table, with the driver's state words after it (spec 1.8).
    fn table(&self, i: u8) -> u16 {
        let i = i as usize;
        let b = self.base;
        let hl = |a: u32, k: usize| if k == 0 { (a >> 16) as u16 } else { a as u16 };
        match i {
            0..=42 => PERIODS[i],
            43 | 44 => hl(self.empty_addr(), i - 43),
            45 => ((self.speed as u16) << 8) | self.hdr1 as u16,
            46 => ((self.repeat as u16) << 8) | self.last as u16,
            47 | 48 => hl(b.wrapping_add(self.l.pos as u32), i - 47),
            49 | 50 => hl(b.wrapping_add(self.l.pat as u32), i - 49),
            51 | 52 => hl(b.wrapping_add(self.l.ins as u32), i - 51),
            53 | 54 => hl(b.wrapping_add(self.l.smp as u32), i - 53),
            _ => 0,
        }
    }

    fn ins(&self, i: usize, o: usize) -> u8 {
        byte(&self.d, self.l.ins + i * 32 + o)
    }

    /// Starts a note on channel `c` (spec 3.3); returns the cycle cost term (Z counts and loop).
    fn start_note(&mut self, c: usize, n: u8, ins: u8, hw: &mut Hw) -> (u64, u64, bool) {
        let i = ins as usize;
        let s = self.ins(i, 19) as usize;
        let mut ch = self.ch[c];
        ch.per = self.table(n) << 4;
        let looped = self.ins(i, 18) != 0;
        if looped {
            self.loopmask |= 1 << c;
        }
        let sa = long(&self.d, self.l.smp + s * 6);
        let a = self.base.wrapping_add(sa).wrapping_sub(self.l.origin);
        let il = |o: usize| long(&self.d, self.l.ins + i * 32 + o);
        ch.start = a.wrapping_add(il(20));
        ch.rstart = a.wrapping_add(il(26));
        ch.len = word(&self.d, self.l.ins + i * 32 + 24);
        ch.rlen = word(&self.d, self.l.ins + i * 32 + 30);
        ch.vol = 0;
        let mut prev = 0i32;
        let mut zv = 0;
        for k in 0..4 {
            let target = self.ins(i, 2 * k) as i32;
            let dur = self.ins(i, 2 * k + 1);
            if dur == 0 {
                zv += 1;
            }
            ch.vcount[k] = dur;
            ch.vdelta[k] = div((target - prev) * 256, dur);
            prev = target;
        }
        let mut m = n;
        let mut zp = 0;
        for k in 0..2 {
            ch.pdelay[k] = self.ins(i, 8 + 3 * k);
            let cnt = self.ins(i, 9 + 3 * k);
            if cnt == 0 {
                zp += 1;
            }
            ch.pcount[k] = cnt;
            let t0 = self.table(m) as i32;
            m = m.wrapping_add(self.ins(i, 10 + 3 * k)).wrapping_sub(24);
            let t1 = self.table(m) as i32;
            ch.pdelta[k] = div((t1 - t0) * 16, cnt);
        }
        ch.vdelay = self.ins(i, 14);
        ch.vreload = self.ins(i, 15);
        ch.vc = ch.vreload >> 1;
        ch.vstep = (self.ins(i, 16) as u16).wrapping_sub(99);
        self.ch[c] = ch;
        let _ = hw;
        (zv, zp, looped)
    }

    /// Line number at half-colour-clock offset `x` from the tick start `e`.
    fn line(e: u64, x: u64) -> u64 {
        (e + x / 2) / 227
    }

    /// One wait for nine raster-line changes starting at `u` (spec 4.4); returns the time of the
    /// ninth change.
    fn raster_wait(e: u64, u: u64) -> u64 {
        let mut r1 = u;
        let mut r = u;
        for k in 0..9 {
            let l = Self::line(e, r1);
            r = r1 + 16;
            while Self::line(e, r) == l {
                r += 25;
            }
            if k < 8 {
                r1 = r + 35;
            }
        }
        r
    }

    fn run(&mut self, hw: &mut Hw) {
        let e = hw.bus.clock;
        let at = |hw: &mut Hw, u: u64| hw.bus.clock = e + u / 2;
        self.counter = self.counter.wrapping_sub(1);
        let row_tick = self.counter & 0x80 != 0;
        let mut u: u64;
        if row_tick {
            self.dma = 0;
            let mut skips = 0u64;
            let mut p = byte(&self.d, self.l.pos + self.pos as usize);
            while p == 0xFF && skips < 256 {
                self.pos = self.pos.wrapping_add(1);
                skips += 1;
                p = byte(&self.d, self.l.pos + self.pos as usize);
            }
            self.counter = self.speed;
            let rowaddr = self.l.pat + p as usize * 132 + 4 + self.row as usize * 2;
            self.row += 1;
            let mut u0 = 678 + 69 * skips;
            if self.row == 16 {
                self.row = 0;
                self.pos = self.pos.wrapping_add(1);
                u0 += 117;
                if self.pos > self.last {
                    self.pos = self.repeat;
                    self.songend = true;
                    u0 += 178;
                }
            }
            self.loopmask = 0;
            u = u0;
            for c in 0..4 {
                let n = byte(&self.d, rowaddr + c * 32);
                if n == 0 {
                    u += 57;
                    continue;
                }
                let ins = byte(&self.d, rowaddr + c * 32 + 1);
                let (zv, zp, lp) = self.start_note(c, n, ins, hw);
                self.dma |= 1 << c;
                u += 3619 - 138 * zv - 138 * zp + 20 * lp as u64;
                at(hw, u);
                hw.loc(c, self.ch[c].start);
                u += 20;
                at(hw, u);
                hw.len(c, self.ch[c].len);
                u += LV[c];
                at(hw, u);
                hw.vol(c, 0);
                u += VP[c];
                at(hw, u);
                hw.per(c, ((self.ch[c].per as i16) >> 4) as u16);
                u += PE[c];
            }
            u += 27;
            at(hw, u);
            hw.dmacon(self.dma);
            u += 35;
        } else {
            u = 328;
        }
        // envelopes
        const CV: [u64; 5] = [0, 27, 50, 63, 35];
        const CP: [u64; 5] = [0, 39, 50, 75, 47];
        for c in 0..4 {
            let ch = &mut self.ch[c];
            let mut cost = 0;
            match (0..4).find(|&k| ch.vcount[k] != 0) {
                Some(k) => {
                    ch.vcount[k] -= 1;
                    ch.vol = ch.vol.wrapping_add(ch.vdelta[k]);
                    cost += CV[k];
                }
                None => cost += CV[4],
            }
            if ch.pdelay[0] != 0 {
                ch.pdelay[0] -= 1;
                cost += CP[0];
            } else if ch.pcount[0] != 0 {
                ch.pcount[0] -= 1;
                ch.per = ch.per.wrapping_add(ch.pdelta[0]);
                cost += CP[1];
            } else if ch.pdelay[1] != 0 {
                ch.pdelay[1] -= 1;
                cost += CP[2];
            } else if ch.pcount[1] != 0 {
                ch.pcount[1] -= 1;
                ch.per = ch.per.wrapping_add(ch.pdelta[1]);
                cost += CP[3];
            } else {
                cost += CP[4];
            }
            if ch.vdelay != 0 {
                ch.vdelay -= 1;
                cost += 430;
            } else if ch.vc != 0 {
                ch.vc -= 1;
                ch.per = ch.per.wrapping_add(ch.vstep);
                cost += 469;
            } else {
                ch.vc = ch.vreload;
                ch.vstep = ch.vstep.wrapping_neg();
                cost += 467;
            }
            u += cost;
        }
        for c in 0..4 {
            at(hw, u);
            hw.vol(c, ((self.ch[c].vol as i16 >> 8) & 0x3FF) as u16);
            u += EVP[c];
            at(hw, u);
            hw.per(c, (self.ch[c].per as i16 >> 4) as u16);
            if c < 3 {
                u += EPN[c];
            }
        }
        let r = Self::raster_wait(e, u + 120);
        u = r + 133;
        self.dma |= 0x8000;
        at(hw, u);
        hw.dmacon(self.dma);
        let r2 = Self::raster_wait(e, u + 62);
        u = r2 + 146;
        for c in 0..4 {
            if self.dma & (1 << c) == 0 {
                u += 47;
                continue;
            }
            u += 47;
            at(hw, u);
            if self.loopmask & (1 << c) != 0 {
                hw.loc(c, self.ch[c].rstart);
                u += 20;
                at(hw, u);
                hw.len(c, self.ch[c].rlen);
                u += 41;
            } else {
                hw.loc(c, self.empty_addr());
                u += 16;
                at(hw, u);
                hw.len(c, 2);
                u += 51;
            }
        }
        at(hw, u);
    }
}

impl Replayer for KimChristensen {
    fn format(&self) -> &'static str {
        "Kim Christensen"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.run(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let r = KimChristensen::build(d, base)?;
    // the reference counts from the run's origin, START_PHASE colour clocks before the engine's
    // aligned start; its timer starts one period before the first tick, and the song's audio
    // counts from that moment, so the clock is left there
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let e0 = first_tick_at(r.l.searched, r.l.gap_odd, r.l.nsmp);
    m.bus.clock = (origin + e0 - TICK).max(m.bus.clock);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "kimchristensen_tests.rs"]
mod tests;
