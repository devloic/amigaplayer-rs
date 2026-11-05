//! EarAche (`EA.*`, `MG.*`), the four-voice music format of Morten Grouleff (1990).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player (the Eagleplayer "EarAche"); no original code. A module holds the
//! song data only (rows, order list, instrument records, sample table, volume and pitch tables and
//! raw samples); the engine is this file. The data starts at the module's start, after an `EASO`
//! tag, or after a short branch header that carries a fixed word sequence (embedded below).
//!
//! Every register write is placed at the clock of the original's code: the tick's entry time is
//! known from the module (the specification's start-up formula), the cycle costs are the
//! specification's static ones, and the beam wait of every note start (nine changes of the raster
//! line, polled) uses the absolute colour clock.
//!
//! All modules this was checked against are synthetic (built from the specification; no real
//! EarAche module was available). Choices where the specification leaves the behaviour open:
//! a period-table index above 71 reads 0; a volume table that loops on 0xFF more than 64 times in
//! one step stops (the original hangs); an order list that never reaches an entry within 10 000
//! reads ends the song's playing (the original hangs); reads outside the file give 0.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "EarAche", detect, new };

/// Twelve bytes taken from the original player on the project owner's decision: the branch-opcode
/// word and the five-word key sequence that a branch-header module carries at its branch target.
static KEY: &[u8; 12] = include_bytes!("earache_branch_header_key.bin");

/// The ticks' period in colour clocks.
const TICK: u64 = 70_940;
const LINE: u64 = 227;

/// The player's period table (index 0..=70).
const PERIODS: [u16; 71] = [
    0x000, 0xD60, 0xCA0, 0xBEA, 0xB40, 0xAA0, 0x9FC, 0x976, 0x8EE, 0x86E, 0x7F4, 0x782, 0x716, 0x6B0, 0x650, 0x5F4,
    0x5A0, 0x54E, 0x502, 0x4BA, 0x476, 0x436, 0x3F9, 0x3C0, 0x38A, 0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C,
    0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC, 0x194, 0x17D, 0x168, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0,
    0x0E2, 0x0D6, 0x0CA, 0x0BE, 0x0B4, 0x0AA, 0x0A0, 0x097, 0x08F, 0x087, 0x07F, 0x078, 0x071, 0x071, 0x071, 0x071,
    0x071, 0x071, 0x071, 0x071, 0x071, 0x071, 0x071,
];

// per-channel cycle constants (SPEC 4.2, 4.3)
const NN: [u64; 4] = [81, 81, 63, 137];
const LC: [u64; 4] = [137, 168, 199, 207];
const VO: [u64; 4] = [864, 912, 950, 985];
const WS: [u64; 4] = [215, 246, 277, 285];
const NL: [u64; 4] = [18, 18, 0, 74];
const NS: [u64; 4] = [31, 31, 13, 87];
const EV: [u64; 4] = [229, 246, 253, 280];
const EP: [u64; 4] = [175, 206, 237, 245];
const ORDER: [usize; 4] = [2, 0, 1, 3];

const MAX_ORDER_READS: usize = 10_000;
const MAX_VOL_LOOPS: usize = 64;

// ---------------------------------------------------------------------------------
// Recognition and layout

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Plain,
    Easo,
    Branch,
}

#[derive(Clone, Copy, Debug)]
struct Lay {
    /// The data start `D`.
    d: usize,
    kind: Kind,
    order: u32,
    instr: u32,
    smptab: u32,
    envtab: u32,
    pittab: u32,
}

fn key_word(i: usize) -> u16 {
    u16::from_be_bytes([KEY[2 * i], KEY[2 * i + 1]])
}

/// A word that is non-zero, positive as a signed word and even.
fn ok_disp(w: u16) -> bool {
    w != 0 && w & 0x8000 == 0 && w & 1 == 0
}

fn locate(d: &[u8]) -> Option<Lay> {
    let (p, kind) = if be16(d, 0) == 0 {
        (0usize, Kind::Plain)
    } else if d.get(0..4) == Some(b"EASO") {
        (4, Kind::Easo)
    } else {
        if be16(d, 0) != key_word(0) || be16(d, 4) != key_word(0) {
            return None;
        }
        let d1 = be16(d, 2);
        if !ok_disp(d1) || !ok_disp(be16(d, 6)) {
            return None;
        }
        let t = 2 + d1 as usize;
        for i in 0..5 {
            if be16(d, t + 2 * i) != key_word(1 + i) {
                return None;
            }
        }
        let d3 = be16(d, t + 10);
        if !ok_disp(d3) {
            return None;
        }
        (t + 10 + d3 as usize, Kind::Branch)
    };
    if p + 24 > d.len() {
        return None;
    }
    let h: Vec<u32> = (0..6).map(|i| be32(d, p + 4 * i)).collect();
    if h[0] != 0x18 {
        return None;
    }
    let a = h[1].wrapping_sub(h[0]) as i32;
    if a <= 0 || a % 8 != 0 {
        return None;
    }
    let b = h[4].wrapping_sub(h[2]) as i32;
    if b <= 0 || b % 16 != 0 {
        return None;
    }
    Some(Lay { d: p, kind, order: h[1], instr: h[2], smptab: h[3], envtab: h[4], pittab: h[5] })
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

/// The start-up time of the reference host (SPEC 4.1): the first tick's entry clock.
fn first_tick(d: &[u8], l: &Lay) -> u64 {
    let rd = |o: u64| be16(d, l.d.wrapping_add(o as usize));
    // song length: entries before the first one that starts with 0xFFFF
    let mut len = 0u64;
    while len < 0x4000 && rd(l.order as u64 + 4 * len) != 0xFFFF && l.d + (l.order as usize) + 4 * (len as usize) < d.len() {
        len += 1;
    }
    // samples: entries before the first zero long; K entries lower than an earlier maximum
    let (mut n, mut k, mut maxstart) = (0u64, 0u64, 0u16);
    while n < 0x4000 {
        let o = l.smptab as u64 + 4 * n;
        if l.d + o as usize >= d.len() {
            break;
        }
        let (st, ln) = (rd(o), rd(o + 2));
        if st == 0 && ln == 0 {
            break;
        }
        if n > 0 && st < maxstart {
            k += 1;
        }
        maxstart = maxstart.max(st);
        n += 1;
    }
    let (c, h) = match l.kind {
        Kind::Plain => (101, 0),
        Kind::Easo => (124, 29),
        Kind::Branch => (255, 63),
    };
    let j = (465 + 47 * len + 84 * n + h).saturating_sub(20 * k) / 2;
    71_897 + c + j
}

// ---------------------------------------------------------------------------------
// State

#[derive(Clone, Copy, Default)]
struct Voice {
    vcount: u8,
    vspeed: u8,
    vptr: i64,
    vloop: i64,
    pcount: u8,
    pspeed: u8,
    arp3: u8,
    astep: u8,
    pptr: i64,
    ploop: i64,
    arp: bool,
    note: u16,
    period: u16,
    cut: u8,
}

pub struct EarAche {
    d: Vec<u8>,
    base: u32,
    lay: Lay,
    silent: u32,
    v: [Voice; 4],
    row: u16,
    last: u16,
    pos: i64,
    counter: u8,
    speed: u8,
    /// The entry clock of the running tick.
    e: u64,
    songend: bool,
    stopped: bool,
    first_delay: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = locate(d)?;
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    m.bus.write_bytes(silent, &[0; 4]);
    let e0 = first_tick(d, &lay);
    // the engine starts the timer one period after the clock now; the first tick falls on e0
    let due = m.bus.clock + TICK;
    let first_delay = if e0 >= due { e0 - due } else { (e0 % LINE + LINE - due % LINE) % LINE };
    Some(Box::new(EarAche {
        d: d.to_vec(),
        base,
        lay,
        silent,
        v: Default::default(),
        row: 0,
        last: 0,
        pos: lay.order as i64,
        counter: 1,
        speed: 1,
        e: 0,
        songend: false,
        stopped: false,
        first_delay,
    }))
}

impl EarAche {
    /// A byte at `o` from the data start; outside the file it is 0.
    fn rb(&self, o: i64) -> u8 {
        let a = self.lay.d as i64 + o;
        if a < 0 {
            return 0;
        }
        self.d.get(a as usize).copied().unwrap_or(0)
    }

    fn rw(&self, o: i64) -> u16 {
        u16::from_be_bytes([self.rb(o), self.rb(o + 1)])
    }

    fn table(&self, idx: usize) -> u16 {
        match idx {
            0..=70 => PERIODS[idx],
            71 => ((self.counter as u16) << 8) | self.speed as u16,
            // the original reads its own state and code here: not specified, 0 chosen
            _ => 0,
        }
    }

    fn at(&self, hw: &mut Hw, u: u64) {
        hw.bus.clock = self.e + u / 2;
    }

    fn line(&self, u: u64) -> u64 {
        (self.e + u / 2) / LINE
    }

    /// The wait of nine changes of the raster line, polled (SPEC 4.4); the poll that saw the last.
    fn line_wait(&self, u: u64) -> u64 {
        let mut r1 = u;
        let mut r = u;
        for i in 0..9 {
            let l = self.line(r1);
            r = r1 + 16;
            while self.line(r) == l {
                r += 25;
            }
            if i < 8 {
                r1 = r + 35;
            }
        }
        r
    }

    /// The row step (SPEC 3.2). Returns the tick's time `u` at the start of the channel part.
    fn row_step(&mut self) -> u64 {
        self.row = self.row.wrapping_add(1);
        if (self.row as i16) <= (self.last as i16) {
            return 418;
        }
        let order = self.lay.order as i64;
        let (mut r, mut j, mut f, mut x) = (0u64, 0u64, 0u64, 0u64);
        let mut a = self.pos;
        let mut found = None;
        let mut reads = 0;
        while found.is_none() {
            reads += 1;
            if reads > MAX_ORDER_READS {
                // the original never leaves this loop: end the playing
                self.stopped = true;
                return 418;
            }
            let w = self.rw(a);
            a += 2;
            if w != 0xFFFF {
                found = Some(w);
                break;
            }
            r += 1;
            self.songend = true;
            for _ in 0..16 {
                let jw = self.rw(a);
                a = order + 4 * (jw & 0xFFF) as i64;
                j += 1;
                let w = self.rw(a);
                a += 2;
                if w != 0xFFFF {
                    found = Some(w);
                    break;
                }
                f += 1;
            }
            if found.is_none() {
                a += 2;
                x += 1;
            }
        }
        let w = found.unwrap_or(0);
        self.row = w & 0xFFF;
        let y = self.rw(a);
        a += 2;
        self.last = y & 0xFFF;
        let sp = ((y >> 12) & 15) as u8;
        self.speed = sp;
        self.counter = sp;
        self.pos = a;
        595 + 145 * r + 97 * j + 6 * f + 53 * x
    }

    fn tick_inner(&mut self, hw: &mut Hw) {
        if self.stopped {
            return;
        }
        self.e = hw.bus.clock;
        self.counter = self.counter.wrapping_sub(1);
        let mut u = 305;
        if self.counter == 0 {
            self.counter = self.speed;
            u = self.row_step();
            if self.stopped {
                return;
            }
            u = self.channels(hw, u);
        }
        self.voices(hw, u);
    }

    /// The channel part of a row tick (SPEC 3.2 step 3, 3.3, 4.2).
    fn channels(&mut self, hw: &mut Hw, mut u: u64) -> u64 {
        let off = ((self.row as u32 * 8) & 0xFFFF) as u16 as i16 as i64;
        for c in 0..4 {
            let w = self.rw(0x18 + off + 2 * c as i64);
            if w == 0 {
                u += NN[c];
                continue;
            }
            u = self.note_start(hw, c, (w >> 8) as usize, (w & 0xFF) as i64, u);
        }
        u
    }

    fn note_start(&mut self, hw: &mut Hw, c: usize, n: usize, inum: i64, mut u: u64) -> u64 {
        let i = self.lay.instr as i64 + 16 * inum;
        let ib = |k: i64| self.rb(i + k);
        let (i0, i1, i2, i3) = (ib(0), ib(1), ib(2), ib(3));
        let (i4, i5, i6, i7, i9) = (ib(4), ib(5), ib(6), ib(7), ib(9));
        let fine = ib(10) as i8 as i32;
        let b = 1u16 << c;
        u += 133;
        self.at(hw, u);
        hw.per(c, 0x78);
        u += 24;
        self.at(hw, u);
        hw.dmacon(b);
        let s = self.lay.smptab as i64 + 4 * (i0 & 0x7F) as i64;
        let (sstart, slen) = (self.rw(s), self.rw(s + 2));
        u += 118;
        self.at(hw, u);
        hw.len(c, slen);
        u += LC[c];
        self.at(hw, u);
        hw.loc(c, self.base.wrapping_add(self.lay.d as u32).wrapping_add(4 * sstart as u32));
        let envp = self.lay.envtab as i64 + 64 * (i2 & 0x7F) as i64;
        let arp = i3 & 0x80 != 0;
        {
            let v = &mut self.v[c];
            v.vptr = envp;
            v.vloop = envp;
            if arp {
                v.arp = true;
                v.arp3 = i3 & 0x7F;
                v.astep = 2;
            } else {
                v.arp = false;
                let p = self_pit(&self.lay, i3);
                v.pptr = p;
                v.ploop = p;
            }
            v.vcount = i4;
            v.vspeed = i5;
            v.pcount = i6;
            v.pspeed = i7;
            v.note = n as u16;
        }
        let q = n / 12;
        let sh = (5usize.wrapping_sub(q)) & 7;
        let mut f = fine as i16;
        if sh != 0 {
            f = ((fine as i32) << sh) as i16;
        }
        let period = self.table(n).wrapping_add(f as u16);
        self.v[c].period = period;
        u += VO[c] - if arp { 22 } else { 0 } - if sh == 0 { 14 } else { 0 };
        self.at(hw, u);
        hw.vol(c, (i9 & 0x7F) as u16);
        u += WS[c];
        let r = self.line_wait(u);
        u = r + 75;
        self.at(hw, u);
        hw.per(c, period);
        u += 36;
        self.at(hw, u);
        hw.dmacon(0x8000 | b);
        if i1 == 0 {
            u += 39;
            self.at(hw, u);
            hw.len(c, 1);
            u += NL[c];
            self.v[c].cut = 0;
        } else {
            u += NS[c];
            self.v[c].cut = i1.wrapping_add(1);
        }
        u
    }

    /// The voice part (SPEC 3.4, 3.5, 4.3).
    fn voices(&mut self, hw: &mut Hw, mut u: u64) {
        for (slot, &c) in ORDER.iter().enumerate() {
            if slot > 0 {
                u += 50;
            }
            if self.v[c].period == 0 {
                u += 27;
                continue;
            }
            let b = 1u16 << c;
            // cut
            if self.v[c].cut == 0 {
                u += 105;
            } else {
                self.v[c].cut -= 1;
                if self.v[c].cut == 0 {
                    u += 103;
                    self.at(hw, u);
                    hw.dmacon(b);
                    u += 24;
                    self.at(hw, u);
                    hw.loc(c, self.silent);
                    u += 61;
                    self.v[c].period = 0;
                } else {
                    u += 124;
                }
            }
            // volume table
            if self.v[c].vcount != 0 {
                self.v[c].vcount -= 1;
                if self.v[c].vcount == 0 {
                    let mut p = self.v[c].vptr;
                    let mut loops = 0;
                    loop {
                        let byte = self.rb(p);
                        if byte <= 0x40 {
                            u += EV[c];
                            self.at(hw, u);
                            hw.vol(c, byte as u16);
                            u += EP[c];
                            self.v[c].vptr = p + 1;
                            self.v[c].vcount = self.v[c].vspeed;
                            break;
                        } else if byte == 0xFF {
                            u += 80;
                            p = self.v[c].vloop;
                            loops += 1;
                            if loops > MAX_VOL_LOOPS {
                                self.v[c].vcount = 0;
                                break;
                            }
                        } else {
                            u += 111;
                            self.v[c].vcount = 0;
                            break;
                        }
                    }
                } else {
                    u += 27;
                }
            }
            // pitch
            if self.v[c].arp {
                let v = &mut self.v[c];
                v.astep = (v.astep + 1) & 3;
                let mut o = 0u8;
                if v.astep == 0 {
                    u += 142;
                } else {
                    o = [v.pcount, v.pspeed, v.arp3][v.astep as usize - 1];
                    if o == 0 {
                        v.astep = 2;
                        u += 191;
                    } else {
                        u += 175;
                    }
                }
                let idx = (o as usize + v.note as usize) & 0x7FF;
                let per = self.table(idx);
                self.at(hw, u);
                hw.per(c, per);
            } else if self.v[c].pcount == 0 {
                u += 26;
            } else {
                self.v[c].pcount -= 1;
                if self.v[c].pcount != 0 {
                    u += 57;
                } else {
                    u += 149;
                    let mut q = self.v[c].pptr;
                    let mut x = self.rb(q) as i8;
                    if x > 0x40 {
                        u += 24;
                        q = self.v[c].ploop;
                        x = self.rb(q) as i8;
                    }
                    let mut p = (self.v[c].period as i16).wrapping_add(x as i16);
                    if p < 100 {
                        p = 100;
                        u += 8;
                    }
                    self.at(hw, u);
                    hw.per(c, p as u16);
                    u += 42;
                    self.v[c].period = p as u16;
                    self.v[c].pptr = q + 1;
                    self.v[c].pcount = self.v[c].pspeed;
                }
            }
        }
    }
}

fn self_pit(l: &Lay, i3: u8) -> i64 {
    l.pittab as i64 + 64 * i3 as i64
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

    fn timer_delay(&self) -> u64 {
        self.first_delay
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }
}

#[cfg(test)]
#[path = "earache_tests.rs"]
mod tests;
