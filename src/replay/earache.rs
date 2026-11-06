//! EarAche (Morten Grouleff, 1990; Eagleplayer "EarAche", module prefixes `ea.` and `mg.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A module holds the song data (row words, an order
//! list, 16-byte instrument records, a sample table, volume and pitch tables) and 8-bit samples;
//! the data may start at the file's start, after the text `EASO`, or after a short branch header.
//! Four voices with per-note volume and pitch tables or a three-step arpeggio, and a note length
//! that cuts the voice. Every register write is placed at the colour clock the specification's
//! cycle ledger gives, including the player's wait for nine raster-line changes at each note start.
//!
//! The six words that tell a branch header (the branch-opcode word and the five words at its
//! target) are embedded from `earache_branchkey.bin`, taken from the original player on the
//! project owner's decision. The 71-entry period table is given as values in the specification.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a period index
//! above 71 reads as period 0; a volume table whose first byte is 0xFF stops instead of looping
//! for ever; an order list that keeps landing on end markers (more than 64 skip rounds in one row
//! step) ends the row step with no row and reports song end; bytes asked for beyond the end of the
//! file read as zero. Host volume, balance and voice masks are not modelled (the identity at the
//! reference's default level).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "EarAche", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
const E0_BASE: u64 = 71897;
/// The branch-opcode word, then the five words found at the branch target.
const KEY: &[u8; 12] = include_bytes!("earache_branchkey.bin");

const PERIODS: [u16; 71] = [
    0x000, 0xD60, 0xCA0, 0xBEA, 0xB40, 0xAA0, 0x9FC, 0x976, 0x8EE, 0x86E, 0x7F4, 0x782, 0x716, 0x6B0, 0x650, 0x5F4,
    0x5A0, 0x54E, 0x502, 0x4BA, 0x476, 0x436, 0x3F9, 0x3C0, 0x38A, 0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C,
    0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC, 0x194, 0x17D, 0x168, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0,
    0x0E2, 0x0D6, 0x0CA, 0x0BE, 0x0B4, 0x0AA, 0x0A0, 0x097, 0x08F, 0x087, 0x07F, 0x078, 0x071, 0x071, 0x071, 0x071,
    0x071, 0x071, 0x071, 0x071, 0x071, 0x071, 0x071,
];

const NN: [u64; 4] = [81, 81, 63, 137];
const LC: [u64; 4] = [137, 168, 199, 207];
const VO: [u64; 4] = [864, 912, 950, 985];
const WS: [u64; 4] = [215, 246, 277, 285];
const NL: [u64; 4] = [18, 18, 0, 74];
const NS: [u64; 4] = [31, 31, 13, 87];
const EV: [u64; 4] = [229, 246, 253, 280];
const EP: [u64; 4] = [175, 206, 237, 245];
const ORDER: [usize; 4] = [2, 0, 1, 3];

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn word(d: &[u8], o: usize) -> u16 {
    (byte(d, o) as u16) << 8 | byte(d, o.wrapping_add(1)) as u16
}

fn long(d: &[u8], o: usize) -> u32 {
    (word(d, o) as u32) << 16 | word(d, o.wrapping_add(2)) as u32
}

/// A word that is non-zero, positive as a signed word and even.
fn good_offset(w: u16) -> bool {
    w != 0 && w & 0x8001 == 0
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    Easo,
    Branch,
}

/// The data start and the kind of header, if the file is a module (spec 1.1, 1.2).
fn locate(d: &[u8]) -> Option<(usize, Kind)> {
    if d.len() < 4 {
        return None;
    }
    let (p, kind) = if word(d, 0) == 0 {
        (0, Kind::Plain)
    } else if &d[..4] == b"EASO" {
        (4, Kind::Easo)
    } else {
        let op = &KEY[..2];
        if &d[..2] != op || !good_offset(word(d, 2)) || &d[4..6.min(d.len())] != op || !good_offset(word(d, 6)) {
            return None;
        }
        let t = 2 + word(d, 2) as usize;
        if t + 12 > d.len() || d[t..t + 10] != KEY[2..12] {
            return None;
        }
        let d3 = word(d, t + 10);
        if !good_offset(d3) {
            return None;
        }
        (t + 10 + d3 as usize, Kind::Branch)
    };
    if p + 24 > d.len() {
        return None;
    }
    let h0 = long(d, p);
    let h1 = long(d, p + 4) as i32;
    let h2 = long(d, p + 8) as i32;
    let h4 = long(d, p + 16) as i32;
    let a = h1.wrapping_sub(h0 as i32);
    let b = h4.wrapping_sub(h2);
    if h0 != 0x18 || a <= 0 || a % 8 != 0 || b <= 0 || b % 16 != 0 {
        return None;
    }
    Some((p, kind))
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

#[derive(Clone, Copy, Default)]
struct Voice {
    vcount: u8,
    vspeed: u8,
    vptr: usize,
    vloop: usize,
    pcount: u8,
    pspeed: u8,
    arp3: u8,
    astep: u8,
    pptr: usize,
    ploop: usize,
    arp: bool,
    note: u16,
    period: u16,
    cut: u8,
}

pub struct EarAche {
    d: Vec<u8>,
    /// Guest address of the data block.
    data_base: u32,
    dstart: usize,
    silent: u32,
    /// The run's origin (clock 0 of the reference).
    origin: u64,
    row: u16,
    last: u16,
    pos: usize,
    counter: u8,
    speed: u8,
    v: [Voice; 4],
    songend: bool,
}

/// The period table as the player reads it: index 71 is the tick counter and speed.
fn table(idx: usize, counter: u8, speed: u8) -> u16 {
    match idx {
        0..=70 => PERIODS[idx],
        71 => (counter as u16) << 8 | speed as u16,
        _ => 0,
    }
}

/// Places a write: the tick's entry clock plus half the cycle count.
struct Stamp {
    e: u64,
}

impl Stamp {
    fn at(&self, hw: &mut Hw, u: u64) {
        hw.bus.clock = self.e + u / 2;
    }
}

impl EarAche {
    fn rd(&self, o: usize) -> u8 {
        byte(&self.d, self.dstart.wrapping_add(o))
    }

    fn rw(&self, o: usize) -> u16 {
        word(&self.d, self.dstart.wrapping_add(o))
    }

    fn hdr(&self, i: usize) -> usize {
        long(&self.d, self.dstart + 4 * i) as usize
    }

    /// Spec 4.4: nine raster-line changes from cycle `u`; returns the poll that saw the last.
    fn wait(&self, st: &Stamp, u: u64) -> u64 {
        let erel = st.e.wrapping_sub(self.origin);
        let line = |x: u64| (erel + x / 2) / 227;
        let mut r1 = u;
        let mut r = u;
        for i in 0..9 {
            let l = line(r1);
            r = r1 + 16;
            while line(r) == l {
                r += 25;
            }
            if i < 8 {
                r1 = r + 35;
            }
        }
        r
    }

    /// Spec 3.3, with the cursor `u` at the start of the channel's slot; returns the cursor after it.
    fn note_start(&mut self, hw: &mut Hw, st: &Stamp, c: usize, n: u8, ins: u8, mut u: u64) -> u64 {
        let i = self.hdr(2) + ins as usize * 16;
        let ib = |k: usize| self.rd(i + k);
        let (i0, i1, i2, i3, i4, i5, i6, i7, i9, i10) = (ib(0), ib(1), ib(2), ib(3), ib(4), ib(5), ib(6), ib(7), ib(9), ib(10));
        u += 133;
        st.at(hw, u);
        hw.per(c, 0x78);
        u += 24;
        st.at(hw, u);
        hw.dmacon(1 << c);
        let se = self.hdr(3) + 4 * (i0 & 0x7F) as usize;
        let (sstart, slen) = (self.rw(se), self.rw(se + 2));
        u += 118;
        st.at(hw, u);
        hw.len(c, slen);
        u += LC[c];
        st.at(hw, u);
        hw.loc(c, self.data_base.wrapping_add(4 * sstart as u32));
        let envp = self.hdr(4) + 64 * (i2 & 0x7F) as usize;
        let arp = i3 & 0x80 != 0;
        let v = &mut self.v[c];
        v.vptr = envp;
        v.vloop = envp;
        if arp {
            v.arp = true;
            v.arp3 = i3 & 0x7F;
            v.astep = 2;
        } else {
            v.arp = false;
            let pp = self.hdr(5) + 64 * i3 as usize;
            let v = &mut self.v[c];
            v.pptr = pp;
            v.ploop = pp;
        }
        let v = &mut self.v[c];
        v.vcount = i4;
        v.vspeed = i5;
        v.pcount = i6;
        v.pspeed = i7;
        v.note = n as u16;
        let s = (5 - (n as i32 / 12)) & 7;
        let mut f = (i10 as i8) as i16 as u16;
        if s != 0 {
            f <<= s;
        }
        v.period = table(n as usize, self.counter, self.speed).wrapping_add(f);
        let vol = (i9 & 0x7F) as u16;
        u += VO[c] - if arp { 22 } else { 0 } - if s == 0 { 14 } else { 0 };
        st.at(hw, u);
        hw.vol(c, vol);
        u += WS[c];
        let r = self.wait(st, u);
        u = r + 75;
        st.at(hw, u);
        hw.per(c, self.v[c].period);
        u += 36;
        st.at(hw, u);
        hw.dmacon(0x8000 | 1 << c);
        if i1 == 0 {
            u += 39;
            st.at(hw, u);
            hw.len(c, 1);
            self.v[c].cut = 0;
            u += NL[c];
        } else {
            self.v[c].cut = i1.wrapping_add(1);
            u += NS[c];
        }
        u
    }

    /// Spec 3.2: the row step; returns the cursor after the channel part.
    fn row_step(&mut self, hw: &mut Hw, st: &Stamp) -> u64 {
        self.row = self.row.wrapping_add(1);
        let mut u = 418;
        if (self.row as i16) > (self.last as i16) {
            let (mut r, mut j, mut f, mut x) = (0u64, 0u64, 0u64, 0u64);
            let mut a = self.pos;
            let mut found = false;
            'outer: for _ in 0..64 {
                let w = self.rw(a);
                a += 2;
                if w != 0xFFFF {
                    self.row = w & 0xFFF;
                    let y = self.rw(a);
                    a += 2;
                    self.last = y & 0xFFF;
                    let sp = ((y >> 12) & 15) as u8;
                    self.speed = sp;
                    self.counter = sp;
                    self.pos = a;
                    found = true;
                    break;
                }
                r += 1;
                self.songend = true;
                for _ in 0..16 {
                    let jw = self.rw(a);
                    a = self.hdr(1) + 4 * (jw & 0xFFF) as usize;
                    let w = self.rw(a);
                    a += 2;
                    j += 1;
                    if w != 0xFFFF {
                        self.row = w & 0xFFF;
                        let y = self.rw(a);
                        a += 2;
                        self.last = y & 0xFFF;
                        let sp = ((y >> 12) & 15) as u8;
                        self.speed = sp;
                        self.counter = sp;
                        self.pos = a;
                        found = true;
                        break 'outer;
                    }
                    f += 1;
                }
                a += 2;
                x += 1;
            }
            u = 595 + 145 * r + 97 * j + 6 * f + 53 * x;
            if !found {
                // endless chain of end markers: no row this tick
                self.last = 0;
                self.row = 0;
                self.pos = self.hdr(1);
                self.speed = 1;
                self.counter = 1;
                for c in 0..4 {
                    u += NN[c];
                }
                return u;
            }
        }
        let ro = ((self.row as usize).wrapping_mul(8) & 0xFFFF) as i16 as isize;
        let rows = self.hdr(0);
        for c in 0..4 {
            let w = self.rw((rows as isize + ro) as usize + 2 * c);
            if w == 0 {
                u += NN[c];
            } else {
                u = self.note_start(hw, st, c, (w >> 8) as u8, w as u8, u);
            }
        }
        u
    }

    /// Spec 3.4, 3.5 and 4.3: the voice part from cursor `u`.
    fn voices(&mut self, hw: &mut Hw, st: &Stamp, mut u: u64) {
        for (slot, &c) in ORDER.iter().enumerate() {
            if slot > 0 {
                u += 50;
            }
            if self.v[c].period == 0 {
                u += 27;
                continue;
            }
            // cut
            if self.v[c].cut == 0 {
                u += 105;
            } else {
                self.v[c].cut -= 1;
                if self.v[c].cut != 0 {
                    u += 124;
                } else {
                    u += 103;
                    st.at(hw, u);
                    hw.dmacon(1 << c);
                    u += 24;
                    st.at(hw, u);
                    hw.loc(c, self.silent);
                    u += 61;
                    self.v[c].period = 0;
                }
            }
            // volume table
            if self.v[c].vcount != 0 {
                self.v[c].vcount -= 1;
                if self.v[c].vcount != 0 {
                    u += 27;
                } else {
                    let mut p = self.v[c].vptr;
                    let mut b = self.rd(p);
                    p += 1;
                    if b == 0xFF {
                        u += 80;
                        p = self.v[c].vloop;
                        b = self.rd(p);
                        p += 1;
                        if b == 0xFF {
                            // a table that only loops: the original never ends it; stop here
                            b = 0x41;
                        }
                    }
                    if b <= 0x40 {
                        u += EV[c];
                        st.at(hw, u);
                        hw.vol(c, b as u16);
                        u += EP[c];
                        self.v[c].vptr = p;
                        self.v[c].vcount = self.v[c].vspeed;
                    } else {
                        u += 111;
                        self.v[c].vcount = 0;
                    }
                }
            }
            // pitch
            if self.v[c].arp {
                let v = &mut self.v[c];
                v.astep = (v.astep + 1) & 3;
                let mut o = 0u8;
                if v.astep != 0 {
                    o = match v.astep {
                        1 => v.pcount,
                        2 => v.pspeed,
                        _ => v.arp3,
                    };
                    if o == 0 {
                        v.astep = 2;
                    }
                }
                u += if v.astep == 0 {
                    142
                } else if o != 0 {
                    175
                } else {
                    191
                };
                let per = table((o as usize + v.note as usize) & 0x7FF, self.counter, self.speed);
                st.at(hw, u);
                hw.per(c, per);
            } else if self.v[c].pcount == 0 {
                u += 26;
            } else {
                self.v[c].pcount -= 1;
                if self.v[c].pcount != 0 {
                    u += 57;
                } else {
                    let mut p = self.v[c].pptr;
                    let mut x = self.rd(p) as i8;
                    p += 1;
                    u += 149;
                    if x > 0x40 {
                        p = self.v[c].ploop;
                        x = self.rd(p) as i8;
                        p += 1;
                        u += 24;
                    }
                    let mut n = (self.v[c].period as i16).wrapping_add(x as i16);
                    if n < 100 {
                        n = 100;
                        u += 8;
                    }
                    st.at(hw, u);
                    hw.per(c, n as u16);
                    u += 42;
                    let v = &mut self.v[c];
                    v.period = n as u16;
                    v.pptr = p;
                    v.pcount = v.pspeed;
                }
            }
        }
    }
}

impl Replayer for EarAche {
    fn format(&self) -> &'static str {
        "EarAche"
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
        let st = Stamp { e: hw.bus.clock };
        self.counter = self.counter.wrapping_sub(1);
        let u = if self.counter == 0 {
            self.counter = self.speed;
            self.row_step(hw, &st)
        } else {
            305
        };
        self.voices(hw, &st, u);
    }
}

/// Song length, sample count and the sample table's out-of-order count (spec 1.10, 4.1).
fn counts(d: &[u8], p: usize) -> (u64, u64, u64) {
    let o = p + long(d, p + 4) as usize;
    let mut l = 0u64;
    while word(d, o.wrapping_add(4 * l as usize)) != 0xFFFF && 4 * (l as usize) < d.len() {
        l += 1;
    }
    let s = p + long(d, p + 12) as usize;
    let (mut n, mut k, mut max) = (0u64, 0u64, 0u16);
    while long(d, s.wrapping_add(4 * n as usize)) != 0 && 4 * (n as usize) < d.len() {
        let st = word(d, s + 4 * n as usize);
        if n > 0 && st < max {
            k += 1;
        }
        max = max.max(st);
        n += 1;
    }
    (l, n, k)
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (p, kind) = locate(d)?;
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    let (l, n, k) = counts(d, p);
    let (c, h) = match kind {
        Kind::Plain => (101, 0),
        Kind::Easo => (124, 29),
        Kind::Branch => (255, 63),
    };
    let j = ((465 + 47 * l as i64 + 84 * n as i64 - 20 * k as i64 + h) / 2) as u64;
    let e0 = E0_BASE + c + j;
    // as the other ports: the reference counts from the run's origin, START_PHASE colour clocks
    // before the engine's aligned start; the clock is left one period before the first tick
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    m.bus.clock = (origin + e0 - TICK).max(m.bus.clock);
    Some(Box::new(EarAche {
        d: d.to_vec(),
        data_base: base.wrapping_add(p as u32),
        dstart: p,
        silent,
        origin,
        row: 0,
        last: 0,
        pos: long(d, p + 4) as usize,
        counter: 1,
        speed: 1,
        v: [Voice::default(); 4],
        songend: false,
    }))
}

#[cfg(test)]
#[path = "earache_tests.rs"]
mod tests;
