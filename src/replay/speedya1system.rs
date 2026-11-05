//! Speedy A1 System (Michael Winterberg, 1988-89; Eagleplayer "Speedy A1 System", prefix `sas.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A module is a 16-byte header (speed, start and end
//! position, the number 512), a 512-position order list of four 16-bit track numbers, 16-row tracks
//! of three bytes, five tables of 32 sample values and the 8SVX-style sample objects, whose bytes
//! the player never reads (only their addresses and lengths go to Paula). Four voices, a per-note
//! attack and release volume envelope and a period slide effect. Every register write is placed at
//! the cycle count the specification's ledger gives, including the raster-line waits of a note
//! start, which depend on the beam phase.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a position of 512
//! or more counts as the song's end (an order entry of 0xFFFF); a track number past the end of the
//! file reads as empty rows; a missing 0xFFFF terminator in the order list scans all 2048 words;
//! bytes asked for beyond the end of the file read as zero. The silent-sample address is a block of
//! zeros the replayer allocates itself (the reference's differs; the specification says that is the
//! one expected difference).

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Speedy A1 System", detect, new };

/// Colour clocks between two ticks (the CIA timer's period).
const TICK: u64 = 70940;
/// Colour clocks from the run's start to the first tick, before the scan costs.
const E0_BASE: u64 = 84319;
/// One raster line in colour clocks, and the lines per frame.
const LINE: u64 = 227;
const LINES: u64 = 313;

/// Periods by octave 0..4 and note 1..13.
const PERIODS: [[u16; 13]; 5] = [
    [0xD58, 0xCA0, 0xBE8, 0xB38, 0xA98, 0x938, 0x970, 0x8E8, 0x868, 0x7F0, 0x780, 0x710, 0x6B0],
    [0x6AC, 0x650, 0x5F4, 0x59C, 0x54C, 0x500, 0x4B8, 0x474, 0x434, 0x3F8, 0x3C0, 0x388, 0x358],
    [0x356, 0x328, 0x2FA, 0x2CE, 0x2A6, 0x280, 0x25C, 0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C4, 0x1AC],
    [0x1AB, 0x194, 0x17D, 0x167, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2, 0x0D6],
    [0x0D6, 0x0CA, 0x0BF, 0x0B4, 0x0AA, 0x0A0, 0x097, 0x08F, 0x087, 0x07F, 0x078, 0x071, 0x06B],
];
/// Envelope step lengths by nibble.
const ENV: [u16; 16] = [0, 1, 2, 3, 4, 6, 9, 13, 18, 24, 31, 39, 48, 50, 61, 72];

/// Accepted-note write offsets from the end of the wait, by envelope kind (none, release-only,
/// attack): AUDxLC, then AUDxLEN, AUDxPER and the slot end by voice.
const NOTE_LC: [u64; 3] = [514, 724, 710];
const NOTE_LEN: [[u64; 4]; 3] = [[693, 724, 755, 763], [915, 946, 977, 985], [901, 932, 963, 971]];
const NOTE_PER: [[u64; 4]; 3] = [[802, 864, 926, 942], [1024, 1086, 1148, 1164], [1010, 1072, 1134, 1150]];
const NOTE_END: [[u64; 4]; 3] = [[1105, 1198, 1291, 1315], [1297, 1390, 1483, 1507], [1283, 1376, 1469, 1493]];

const ADD_A: [u64; 4] = [0, 23, 46, 36];
const EF_ADD: [u64; 4] = [353, 384, 415, 423];
const EF_SUB: [u64; 4] = [372, 403, 434, 442];
const TAIL: [u64; 4] = [205, 236, 267, 275];

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn word(d: &[u8], o: usize) -> u16 {
    (byte(d, o) as u16) << 8 | byte(d, o + 1) as u16
}

/// What the detection finds: the sample tables' offset and the start-up costs.
struct Layout {
    tb: usize,
    /// Cost A of the specification's section 8 (order list and zero padding), in cycles.
    cost_a: u64,
}

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 0x1010 || word(d, 0) != 0 || be32(d, 0x0C) != 0x200 {
        return None;
    }
    // the order list: up to 2048 words, to the first 0xFFFF
    let mut tmax = 0u16;
    let mut w = 0u64;
    let mut m = 0u64;
    for i in 0..2048 {
        let x = word(d, 0x10 + 2 * i);
        if x == 0xFFFF {
            break;
        }
        if x >> 8 != 0 {
            return None;
        }
        if x >= tmax {
            m += 1;
        }
        tmax = tmax.max(x);
        w += 1;
    }
    // skip zero 32-bit words after the last track
    let mut p = 0x1010 + 48 * (tmax as usize + 1);
    let mut z = 0u64;
    loop {
        if p + 4 > d.len() {
            return None;
        }
        if be32(d, p) != 0 {
            break;
        }
        p += 4;
        z += 1;
    }
    if d.get(p + 0x280..p + 0x284)? != b"FORM" {
        return None;
    }
    Some(Layout { tb: p, cost_a: 315 + 74 * w + 8 * m + 21 * z })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum St {
    #[default]
    Idle,
    Attack,
    Release,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    st: St,
    vol: u16,
    per: u16,
    timer: u16,
    att: u16,
    rel: u16,
    smp: Option<usize>,
    cmd: u8,
    arg: u16,
}

pub struct SpeedyA1System {
    d: Vec<u8>,
    base: u32,
    tb: usize,
    speed: u32,
    start: u32,
    end: u32,
    silent: u32,
    /// The clock of the run's start, which the logs count from.
    origin: u64,
    counter: u32,
    pos: u32,
    row: i32,
    v: [Voice; 4],
    songend: bool,
}

/// Where a tick's writes go: the cycle count from the tick's start gives the colour clock.
struct Clk {
    t0: u64,
    /// The tick's start relative to the run's start (the beam's reference).
    rel: u64,
}

impl Clk {
    fn at(&self, hw: &mut Hw, c: u64) {
        hw.bus.clock = self.t0 + c / 2;
    }

    /// The raster line's low byte at the cycle count `c`.
    fn line(&self, c: u64) -> u64 {
        ((self.rel + c / 2) / LINE % LINES) & 0xFF
    }

    /// The nine-line wait entered at cycle count `c` (spec 7.4); returns the count after it.
    fn wait(&self, mut c: u64) -> u64 {
        c += 28;
        for _ in 0..9 {
            c += 16;
            let r = self.line(c);
            // bounded: the beam moves on within a line
            for _ in 0..4096 {
                c += 16;
                let l = self.line(c);
                c += 9;
                if l != r {
                    break;
                }
            }
            c += 10;
        }
        c + 44
    }
}

impl SpeedyA1System {
    fn table(&self, t: usize, i: usize) -> u32 {
        be32(&self.d, self.tb + 0x80 * t + 4 * i)
    }

    fn entry(&self, pos: u32, v: usize) -> u16 {
        if pos >= 512 {
            return 0xFFFF;
        }
        word(&self.d, 0x10 + 8 * pos as usize + 2 * v)
    }

    fn song_end(&mut self) {
        self.songend = true;
        self.pos = self.start;
        for v in self.v.iter_mut() {
            v.cmd = 0;
            v.arg = 0;
        }
    }

    fn run(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        let clk = Clk { t0, rel: t0.saturating_sub(self.origin) };
        let c: u64;
        if self.counter == 0 {
            self.counter = self.speed;
            let mut mask = 0u16;
            let mut b = 375u64;
            let mut e = 375u64;
            for v in 0..4 {
                let t = self.entry(self.pos, v);
                let (b0, b1, b2) = if t == 0xFFFF {
                    (0xFF, 0, 0)
                } else {
                    let o = 0x1010 + 48 * t as usize + 3 * self.row.clamp(0, 15) as usize;
                    (byte(&self.d, o), byte(&self.d, o + 1), byte(&self.d, o + 2))
                };
                if t == 0xFFFF || b0 == 0xFF {
                    // song end inside the row (spec 4.2, 6.4)
                    let ct = b + if t == 0xFFFF { 345 } else { 620 };
                    e = ct - ct % 2 + 16 + 156;
                    self.song_end();
                    self.row = -1;
                    break;
                }
                if b0 >= 0x50 {
                    self.v[v].cmd = b0;
                    self.v[v].arg = (b1 as u16) << 8 | b2 as u16;
                    e = b + 775;
                } else {
                    let oct = (b0 >> 4) as usize;
                    let n = (b0 & 15) as usize;
                    let p = if oct < 5 && (1..=13).contains(&n) { PERIODS[oct][n - 1] } else { 0 };
                    if p == 0 {
                        e = b + 670;
                    } else if b2 >= 0x20 {
                        e = b + 953;
                    } else {
                        e = self.note(hw, &clk, v, b, p, b1, b2 as usize);
                        mask |= 1 << v;
                    }
                }
                b = e + 73;
            }
            if mask != 0 {
                clk.at(hw, e + 118);
                hw.dmacon(0x8200 | mask);
                c = clk.wait(e + 136) + 100;
            } else {
                c = e + 150;
            }
            self.update(hw, &clk, c);
            // the row counter (spec 3.2)
            self.row += 1;
            if self.row >= 16 {
                self.row = 0;
                self.pos += 1;
                if self.pos == self.end {
                    self.song_end();
                }
            }
        } else {
            self.counter -= 1;
            c = 397;
            self.update(hw, &clk, c);
        }
    }

    /// An accepted note (spec 4.3) in the slot beginning at `b`; returns the slot's end.
    #[allow(clippy::too_many_arguments)]
    fn note(&mut self, hw: &mut Hw, clk: &Clk, v: usize, b: u64, p: u16, b1: u8, i: usize) -> u64 {
        clk.at(hw, b + 1084);
        hw.dmacon(1 << v);
        let x = clk.wait(b + 1102);
        clk.at(hw, x + 118);
        hw.loc(v, self.silent);
        clk.at(hw, x + 134);
        hw.len(v, 5);
        clk.at(hw, x + 150);
        hw.vol(v, 0);
        clk.at(hw, x + 166);
        hw.per(v, 0xCA);
        let a2 = self.base.wrapping_add(self.tb as u32).wrapping_add(self.table(2, i));
        let (t1, t3, t4) = (self.table(1, i), self.table(3, i), self.table(4, i));
        let kind = if b1 == 0 {
            0
        } else if b1 < 0x10 {
            1
        } else {
            2
        };
        let len = if kind == 0 {
            ((t1.wrapping_sub(0x68) as i32) >> 1) as u16
        } else {
            (t3.wrapping_add(t4) >> 1) as u16
        };
        let vs = &mut self.v[v];
        match kind {
            0 => {
                vs.st = St::Idle;
                vs.vol = 0x40;
                vs.smp = None;
            }
            1 => {
                vs.st = St::Release;
                vs.vol = 0x40;
                vs.att = ENV[0];
                vs.rel = ENV[(b1 & 15) as usize];
                vs.timer = vs.rel;
                vs.smp = Some(i);
            }
            _ => {
                vs.st = St::Attack;
                vs.vol = 0;
                vs.att = ENV[(b1 >> 4) as usize];
                vs.rel = ENV[(b1 & 15) as usize];
                vs.timer = vs.att;
                vs.smp = Some(i);
            }
        }
        vs.per = p;
        clk.at(hw, x + NOTE_LC[kind]);
        hw.loc(v, a2);
        clk.at(hw, x + NOTE_LEN[kind][v]);
        hw.len(v, len);
        clk.at(hw, x + NOTE_PER[kind][v]);
        hw.per(v, p);
        x + NOTE_END[kind][v]
    }

    /// The voice update (spec 5, 6.3) with the block starting at cycle count `u`.
    fn update(&mut self, hw: &mut Hw, clk: &Clk, mut u: u64) {
        for v in 0..4 {
            let vs = self.v[v];
            // reload class: 0 none, 1 idle, 2 loop
            let (class, rc) = if vs.st == St::Idle {
                (1, 67)
            } else if let Some(i) = vs.smp.filter(|&i| self.table(4, i) != 0) {
                let a2 = self.base.wrapping_add(self.tb as u32).wrapping_add(self.table(2, i));
                clk.at(hw, u + 450);
                hw.loc(v, a2.wrapping_add(self.table(3, i)));
                clk.at(hw, u + 472);
                hw.len(v, (self.table(4, i) >> 1) as u16);
                (2, 239)
            } else {
                (0, 0)
            };
            if class == 1 {
                clk.at(hw, u + 278);
                hw.loc(v, self.silent);
                clk.at(hw, u + 300);
                hw.len(v, 5);
            }
            let s = &mut self.v[v];
            let mut ef = 0;
            if s.cmd == 0x50 || s.cmd == 0x51 {
                let add = s.cmd == 0x50;
                s.per = if add { s.per.wrapping_add(s.arg) } else { s.per.wrapping_sub(s.arg) };
                clk.at(hw, u + if add { 792 } else { 811 } + rc);
                hw.per(v, s.per);
                ef = if add { EF_ADD[v] } else { EF_SUB[v] };
            }
            let vo: i64;
            if s.cmd == 0x52 {
                s.vol = s.att;
                vo = (1134 + ADD_A[v] + rc) as i64;
            } else {
                let ev: i64 = match s.st {
                    St::Idle => -32,
                    St::Attack => {
                        if s.timer != 0 {
                            s.timer -= 1;
                            0
                        } else if s.vol != 0x40 {
                            s.vol = s.vol.wrapping_add(8);
                            s.timer = s.att.wrapping_sub(1);
                            83
                        } else {
                            s.st = St::Release;
                            s.timer = s.rel.wrapping_sub(1);
                            let ev = if s.rel == 1 { 237 } else { 154 };
                            Self::release_step(s);
                            ev
                        }
                    }
                    St::Release => {
                        let ev = if s.timer != 0 {
                            19
                        } else if s.vol != 0 {
                            102
                        } else {
                            76
                        };
                        Self::release_step(s);
                        ev
                    }
                };
                vo = (1120 + ADD_A[v] + rc + ef) as i64 + ev;
            }
            let vo = vo as u64;
            clk.at(hw, u + vo);
            hw.vol(v, s.vol & 0x7F);
            u += vo + TAIL[v] + 45;
        }
    }

    /// The release rule of spec 5.3, once.
    fn release_step(s: &mut Voice) {
        if s.timer != 0 {
            s.timer -= 1;
        } else if s.vol != 0 {
            s.vol = s.vol.wrapping_sub(8);
            s.timer = s.rel.wrapping_sub(1);
        } else {
            s.st = St::Idle;
        }
    }
}

impl Replayer for SpeedyA1System {
    fn format(&self) -> &'static str {
        "Speedy A1 System"
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

/// The first tick's cycle-derived delay: cost A (order list, padding) and B (the sample scan).
fn start_cost(d: &[u8], l: &Layout) -> u64 {
    let (mut ne, mut nm, mut nn) = (0u64, 0u64, 0u64);
    let mut dd = 0i32;
    for i in 0..32 {
        let t0 = be32(d, l.tb + 4 * i);
        if t0 >> 24 != 0 {
            ne += 1;
        } else if dd >= t0 as i32 {
            nn += 1;
        } else {
            nm += 1;
            dd = t0.wrapping_add(be32(d, l.tb + 0x80 + 4 * i)) as i32;
        }
    }
    let b = 6 + 53 * ne + 110 * nm + 84 * nn;
    l.cost_a / 2 + b / 2
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = layout(d)?;
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    // the reference's start-up counts from the run's origin, which is START_PHASE colour clocks
    // before the engine's aligned start
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let e0 = E0_BASE + start_cost(d, &l);
    {
        let mut hw = Hw { bus: &mut m.bus };
        for off in [E0_BASE_VOL1, E0_BASE_VOL2] {
            for (v, c) in [440u64, 705, 993, 1271].iter().enumerate() {
                hw.bus.clock = origin + e0 - off + c / 2;
                hw.vol(v, 0);
            }
        }
    }
    // the first tick follows one period after the clock left here
    m.bus.clock = (origin + e0 - TICK).max(m.bus.clock);
    Some(Box::new(SpeedyA1System::build(d, &l, base, silent, origin)))
}

/// The start-up volume writes' distances before the first tick (spec 8).
const E0_BASE_VOL1: u64 = 84191;
const E0_BASE_VOL2: u64 = 83527;

impl SpeedyA1System {
    fn build(d: &[u8], l: &Layout, base: u32, silent: u32, origin: u64) -> Self {
        SpeedyA1System {
            d: d.to_vec(),
            base,
            tb: l.tb,
            speed: be32(d, 0),
            start: be32(d, 4),
            end: be32(d, 8),
            silent,
            origin,
            counter: 0,
            pos: be32(d, 4),
            row: 0,
            v: [Voice::default(); 4],
            songend: false,
        }
    }
}

#[cfg(test)]
#[path = "speedya1system_tests.rs"]
mod tests;
