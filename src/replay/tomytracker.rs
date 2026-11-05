//! Tomy Tracker (Amiga; Tom "Tomy" Pakarinen, 1992; Eagleplayer "Tomy Tracker", module prefix `sg.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player; no original code, and nothing is embedded (the period table, the
//! vibrato sine and the arpeggio phase sequence are values of the specification). A module is a
//! 704-byte song header (total size, song size, 31 sample records, restart, length, position
//! list), 1024-byte patterns of 64 rows of four 4-byte cells, then the 8-bit samples. Four
//! voices, a volume per sample, tone portamento, period slides, vibrato, volume slide, arpeggio,
//! pattern break, filter and speed commands. Every register write is placed at the clock of the
//! original's code, from the specification's cycle ledger, including the two nine-line raster
//! waits of each row tick (modelled from the beam phase) and the start-up calls' cost for the
//! first tick.
//!
//! No real module exists (only the specification's 12 synthetic ones), so the detection is the
//! specification's strict content rule and the replayer is registered last. One deliberate
//! tightening: the song length must be 1..127 (the format's range), where the rule says "below
//! 128".
//!
//! Choices where the specification is silent (all safe, none can panic or hang): bytes asked for
//! beyond the end of the module read as zero; a command byte that is not a multiple of 4 or is
//! above 13 * 4 is a row without command; an instrument byte that is not a multiple of 7, above
//! 31 * 7, or any instrument not yet set when a note starts, is a sample with volume 0 and a
//! one-word body at the sample base; a note number above 36 uses the last period; restart 0 or
//! above the song length restarts at position 0; set speed 0 is ignored; the arpeggio phase is
//! taken from the 16-bit difference of speed and counter for any speed; the host's volume, balance
//! and voice masks are not modelled (the volume is the module's volume).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Tomy Tracker", detect, new };

/// Colour clocks between two ticks (the CIA timer's period).
const TICK: u64 = 70_940;
const LINE: u64 = 227;
/// Offset of the first pattern.
const PATTERNS: usize = 704;

/// Period by note number (index 0 is no note).
const PERIODS: [u16; 37] = [
    0, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269,
    254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];
/// The vibrato sine.
const SINE: [u16; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24,
];

/// Cost before the volume write of channel 0..3, and after it.
const VOL_PRE: [u64; 4] = [160, 183, 206, 229];
const VOL_POST: [u64; 4] = [38, 38, 38, 28];
/// Cost of a sample start by channel.
const START: [u64; 4] = [123, 154, 185, 193];

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn word(d: &[u8], o: usize) -> u16 {
    ((byte(d, o) as u16) << 8) | byte(d, o + 1) as u16
}

fn long(d: &[u8], o: usize) -> u32 {
    ((word(d, o) as u32) << 16) | word(d, o + 2) as u32
}

/// The song size `S` and the pattern count, if the file is a module (spec 1.1).
fn header(d: &[u8]) -> Option<(usize, usize)> {
    if d.len() <= 1728 {
        return None;
    }
    let total = be32(d, 0);
    let s = be32(d, 4);
    if total == 0 || total > 0x20_0000 || total % 2 != 0 || s > total || s % 2 != 0 {
        return None;
    }
    let s = s as usize;
    if s < PATTERNS + 1024 || (s - PATTERNS) % 1024 != 0 {
        return None;
    }
    if d[444] != 0 || d[445] == 0 || d[445] >= 128 {
        return None;
    }
    let patterns = (s - PATTERNS) / 1024;
    let l = d[445] as usize;
    let max = (0..l).map(|i| be16(d, 448 + 2 * i) as usize).max()?;
    (max == (patterns - 1) * 1024).then_some((s, patterns))
}

fn detect(d: &[u8]) -> bool {
    header(d).is_some()
}

/// One sample record.
#[derive(Clone, Copy, Default)]
struct Sample {
    vol: u16,
    start: u32,
    len: u16,
    loop_start: u32,
    loop_len: u16,
}

#[derive(Clone, Copy, Default)]
struct Chan {
    ins: u8,
    note: u8,
    cmd: u8,
    par: u8,
    vol: u16,
    per: u16,
    target: u16,
    pspeed: u8,
    pdir: u8,
    vibcmd: u8,
    vibpos: u8,
    loop_start: u32,
    loop_len: u16,
}

pub struct TomyTracker {
    d: Vec<u8>,
    /// Guest address of the sample data.
    sample_base: u32,
    len: usize,
    restart: usize,
    counter: u16,
    speed: u16,
    row: usize,
    pos: usize,
    dmabits: u16,
    /// The CIA-A port value the filter command modifies (starts 0, reads back what was written).
    port: u8,
    ch: [Chan; 4],
    origin: u64,
    songend: bool,
}

/// Where a tick's writes go: the cycle count `t` from the tick's start gives the colour clock.
struct Clk {
    t0: u64,
    /// The tick's start relative to the run's start (the beam's reference).
    rel: u64,
    t: u64,
}

impl Clk {
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.t / 2;
    }

    fn line(&self) -> u64 {
        (self.rel + self.t / 2) / LINE
    }

    /// The nine-line wait (spec 7.3).
    fn wait(&mut self) {
        self.t += 28;
        for _ in 0..9 {
            self.t += 16;
            let l0 = self.line();
            // bounded: the beam moves on within a line
            for _ in 0..4096 {
                self.t += 16;
                if self.line() != l0 {
                    break;
                }
                self.t += 9;
            }
            self.t += 19;
        }
        self.t += 44;
    }
}

fn s16(v: u16) -> i16 {
    v as i16
}

impl TomyTracker {
    fn sample(&self, ins: u8) -> Sample {
        // an instrument byte is 7 times the number; anything else is read as an empty sample
        if ins == 0 || ins % 7 != 0 || ins / 7 > 31 {
            return Sample { len: 1, loop_len: 1, ..Sample::default() };
        }
        let o = 8 + 14 * (ins as usize / 7 - 1);
        Sample {
            vol: word(&self.d, o),
            start: long(&self.d, o + 2),
            len: word(&self.d, o + 6),
            loop_start: long(&self.d, o + 8),
            loop_len: word(&self.d, o + 12),
        }
    }

    fn period(n: usize) -> u16 {
        PERIODS[n.min(36)]
    }

    /// The volume write of channel `c` (spec 6, 7.2).
    fn vol_write(&self, hw: &mut Hw, clk: &mut Clk, c: usize) {
        clk.t += VOL_PRE[c];
        clk.at(hw);
        hw.vol(c, self.ch[c].vol & 0x7F);
        clk.t += VOL_POST[c];
    }

    fn per_write(&self, hw: &mut Hw, clk: &mut Clk, c: usize, per: u16) {
        clk.at(hw);
        hw.per(c, per);
    }

    fn run(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        let mut clk = Clk { t0, rel: t0.saturating_sub(self.origin), t: 277 };
        self.counter = self.counter.wrapping_sub(1);
        if self.counter != 0 {
            self.effects(hw, &mut clk);
        } else {
            self.counter = self.speed;
            self.row_tick(hw, &mut clk);
        }
    }

    /// Spec 4: the row tick.
    fn row_tick(&mut self, hw: &mut Hw, clk: &mut Clk) {
        clk.t += 102;
        self.dmabits = 0;
        let rowbase = PATTERNS + word(&self.d, 448 + 2 * self.pos.min(127)) as usize + 16 * self.row;
        for c in 0..4 {
            if c > 0 {
                clk.t += 20;
            }
            clk.t += 53;
            let o = rowbase + 4 * c;
            let (b0, b1, b2, b3) = (byte(&self.d, o), byte(&self.d, o + 1), byte(&self.d, o + 2), byte(&self.d, o + 3));
            // a command byte that is not 4 times 0..13 is a row without command (safe choice)
            let cmd = if b0 % 4 == 0 && b0 <= 13 * 4 { b0 / 4 } else { 0 };
            self.ch[c].cmd = cmd;
            self.ch[c].par = b1;
            if b2 != 0 {
                clk.t += 40;
                self.ch[c].ins = b2;
                self.ch[c].vol = self.sample(b2).vol;
            } else {
                clk.t += 34;
            }
            clk.t += 29;
            if b3 == 0 {
                clk.t += 25;
                if cmd >= 9 {
                    clk.t += 26;
                    self.row_command(hw, clk, c);
                }
                clk.t += 16;
                let per = self.ch[c].per;
                self.per_write(hw, clk, c, per);
                clk.t += 42;
                self.vol_write(hw, clk, c);
                clk.t += 22;
                continue;
            }
            let p = Self::period((b3 / 2) as usize);
            clk.t += 27;
            if cmd != 1 {
                clk.t += 13;
            }
            if cmd == 1 || cmd == 2 {
                self.porta_setup(c, p, clk);
                clk.t += 42;
                self.vol_write(hw, clk, c);
                clk.t += 22;
                if c == 3 {
                    clk.t += 10;
                }
            } else {
                self.start_note(hw, clk, c, p, b3);
            }
        }
        // the end of the row (spec 4.6)
        clk.t += 20;
        clk.at(hw);
        hw.dmacon(self.dmabits);
        self.dmabits |= 0x8000;
        clk.t += 61;
        self.row += 1;
        if self.row >= 64 {
            self.row = 0;
            clk.t += 53;
            if self.pos + 1 >= self.len {
                clk.t += 56 + 72;
                clk.t -= clk.t % 2;
                clk.t += 16 + 44;
                self.songend = true;
                self.pos = if self.restart >= 1 && self.restart <= self.len { self.restart - 1 } else { 0 };
            } else {
                self.pos += 1;
            }
            clk.t += 24;
        }
        clk.t += 456;
        clk.wait();
        clk.t += 20;
        clk.at(hw);
        hw.dmacon(self.dmabits);
        clk.t += 38;
        clk.wait();
        for c in 0..4 {
            clk.t += if c == 0 { 20 } else { 24 };
            clk.at(hw);
            hw.loc(c, self.ch[c].loop_start);
            clk.t += if c == 0 { 12 } else { 16 };
            clk.at(hw);
            hw.len(c, self.ch[c].loop_len);
        }
    }

    /// Spec 4.2: a note with a portamento command.
    fn porta_setup(&mut self, c: usize, p: u16, clk: &mut Clk) {
        clk.t += 73;
        let ch = &mut self.ch[c];
        ch.pdir = 0;
        ch.pspeed = (ch.vol >> 8) as u8;
        ch.target = p;
        if s16(p) == s16(ch.per) {
            ch.target = 0;
            clk.t += 12;
        } else if s16(p) < s16(ch.per) {
            ch.pdir = 0xFF;
            clk.t += 25;
        } else {
            clk.t += 9;
        }
    }

    /// Spec 4.3: a note that starts the sample.
    fn start_note(&mut self, hw: &mut Hw, clk: &mut Clk, c: usize, p: u16, b3: u8) {
        let s = self.sample(self.ch[c].ins);
        {
            let ch = &mut self.ch[c];
            ch.per = p;
            ch.note = b3;
        }
        self.dmabits |= 1 << c;
        self.ch[c].vibpos = 0;
        clk.t += 62 + START[c] + 20;
        clk.at(hw);
        hw.loc(c, self.sample_base.wrapping_add(s.start));
        clk.t += 12;
        clk.at(hw);
        hw.len(c, s.len);
        self.ch[c].loop_start = self.sample_base.wrapping_add(s.loop_start);
        self.ch[c].loop_len = s.loop_len;
        clk.t += 57;
        if self.ch[c].cmd >= 9 {
            clk.t += 26;
            self.row_command(hw, clk, c);
        }
        clk.t += 16;
        let per = self.ch[c].per;
        self.per_write(hw, clk, c, per);
        clk.t += 42;
        self.vol_write(hw, clk, c);
        clk.t += 22;
    }

    /// Spec 4.4: the row commands 9..13 of channel `c`.
    fn row_command(&mut self, hw: &mut Hw, clk: &mut Clk, c: usize) {
        let par = self.ch[c].par;
        match self.ch[c].cmd {
            10 => {
                clk.t += 30;
                self.ch[c].vol = (self.ch[c].vol & 0xFF00) | par as u16;
            }
            11 => {
                clk.t += 26;
                self.row = 63;
            }
            12 => {
                clk.t += 24;
                self.port &= !2;
                clk.at(hw);
                hw.bus.write_byte(0xBFE001, self.port);
                clk.t += 32;
                self.port |= par;
                clk.at(hw);
                hw.bus.write_byte(0xBFE001, self.port);
                clk.t += 10;
            }
            13 => {
                clk.t += 50;
                // speed 0 would wrap the counter to 65535 and stall the song: ignored (safe choice)
                if par != 0 {
                    self.speed = (self.speed & 0xFF00) | par as u16;
                    self.counter = (self.counter & 0xFF00) | par as u16;
                }
            }
            _ => clk.t += 10,
        }
    }

    /// Spec 5: the effect tick.
    fn effects(&mut self, hw: &mut Hw, clk: &mut Clk) {
        clk.t += 28;
        for c in 0..4 {
            clk.t += 17;
            let cmd = self.ch[c].cmd;
            if cmd == 0 {
                clk.t += 30;
                continue;
            }
            clk.t += 17;
            if cmd >= 9 {
                clk.t += 30;
                continue;
            }
            clk.t += 30;
            match cmd {
                1 | 2 => self.porta(hw, clk, c),
                3 => {
                    clk.t += 41;
                    let ch = &mut self.ch[c];
                    let mut v = s16(ch.per).wrapping_sub(ch.par as i16);
                    if v < 0x71 {
                        v = 0x71;
                        clk.t += 4;
                    }
                    ch.per = v as u16;
                    clk.t += 20;
                    let per = ch.per;
                    self.per_write(hw, clk, c, per);
                    clk.t += 10;
                }
                4 => {
                    clk.t += 37;
                    let ch = &mut self.ch[c];
                    let mut v = s16(ch.per).wrapping_add(ch.par as i16);
                    if v >= 0x358 {
                        v = 0x358;
                        clk.t += 8;
                    }
                    ch.per = v as u16;
                    clk.t += 20;
                    let per = ch.per;
                    self.per_write(hw, clk, c, per);
                    clk.t += 10;
                }
                5 | 6 => {
                    self.vibrato(hw, clk, c);
                    if cmd == 6 {
                        self.vol_slide(hw, clk, c);
                    }
                }
                7 => self.vol_slide(hw, clk, c),
                8 => self.arpeggio(hw, clk, c),
                _ => {}
            }
            clk.t += 30;
        }
    }

    /// Spec 5.1.
    fn porta(&mut self, hw: &mut Hw, clk: &mut Clk, c: usize) {
        clk.t += 17;
        let ch = &mut self.ch[c];
        if ch.par != 0 {
            clk.t += 20;
            ch.pspeed = ch.par;
            ch.par = 0;
        }
        clk.t += 21;
        if ch.target == 0 {
            clk.t += 10;
            return;
        }
        clk.t += 33 + 49;
        let reached;
        if ch.pdir == 0 {
            ch.per = ch.per.wrapping_add(ch.pspeed as u16);
            reached = !(s16(ch.target) > s16(ch.per));
        } else {
            ch.per = ch.per.wrapping_sub(ch.pspeed as u16);
            reached = !(s16(ch.target) < s16(ch.per));
        }
        if reached {
            ch.per = ch.target;
            ch.target = 0;
            clk.t += 24;
        }
        clk.t += 16;
        let per = ch.per;
        self.per_write(hw, clk, c, per);
        clk.t += 10;
    }

    /// Spec 5.4: vibrato (the volume slide of command 6 follows in the caller).
    fn vibrato(&mut self, hw: &mut Hw, clk: &mut Clk, c: usize) {
        clk.t += 17;
        let ch = &mut self.ch[c];
        if ch.par != 0 {
            clk.t += 12;
            ch.vibcmd = ch.par;
        }
        clk.t += 175;
        let s = SINE[(ch.vibpos as usize >> 2) & 31] as i32;
        let d = ((s * (ch.vibcmd & 15) as i32) >> 7) as u16;
        let per = if ch.vibpos < 0x80 {
            clk.t += 14;
            ch.per.wrapping_add(d)
        } else {
            clk.t += 4;
            ch.per.wrapping_sub(d)
        };
        clk.t += 8;
        ch.vibpos = ch.vibpos.wrapping_add((ch.vibcmd >> 2) & 0x3C);
        self.per_write(hw, clk, c, per);
        clk.t += 54 + 11;
    }

    /// Spec 5.5: volume slide, then the volume write.
    fn vol_slide(&mut self, hw: &mut Hw, clk: &mut Clk, c: usize) {
        clk.t += 29;
        let ch = &mut self.ch[c];
        if ch.par & 15 != 0 {
            clk.t += 25;
            let v = s16(ch.vol).wrapping_sub((ch.par & 15) as i16);
            ch.vol = if v < 0 {
                clk.t += 12;
                0
            } else {
                v as u16
            };
        } else {
            clk.t += 55;
            let v = s16(ch.vol).wrapping_add((ch.par >> 4) as i16);
            ch.vol = if v >= 64 {
                clk.t += 16;
                64
            } else {
                v as u16
            };
        }
        clk.t += 62;
        self.vol_write(hw, clk, c);
        clk.t += 38;
    }

    /// Spec 5.6.
    fn arpeggio(&mut self, hw: &mut Hw, clk: &mut Clk, c: usize) {
        clk.t += 55;
        let k = self.speed.wrapping_sub(self.counter) % 3;
        let ch = &self.ch[c];
        if k == 0 {
            clk.t += 16;
            let per = ch.per;
            self.per_write(hw, clk, c, per);
            clk.t += 10;
            return;
        }
        clk.t += 13;
        let n = if k == 1 {
            clk.t += 24;
            ch.par >> 4
        } else {
            clk.t += 8;
            ch.par & 15
        };
        clk.t += 33;
        let mut i = (ch.note.wrapping_add(2 * n)) as usize;
        if i > 0x48 {
            i = 0x48;
            clk.t += 4;
        }
        clk.t += 18;
        let per = Self::period(i / 2);
        self.per_write(hw, clk, c, per);
        clk.t += 10;
    }
}

impl Replayer for TomyTracker {
    fn format(&self) -> &'static str {
        "Tomy Tracker"
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

/// The clock of the start-up calls (spec 7.5): the four start-up volume writes' times relative to
/// the run's start, and the clock at which the last call ends.
fn startup(d: &[u8], s: usize, len: usize) -> (Vec<u64>, u64) {
    let _ = s;
    let l = len as u64;
    let mut u = 0u64;
    let mut max = 0u16;
    for i in 0..len {
        let e = word(d, 448 + 2 * i);
        if i == 0 || e >= max {
            u += 1;
        }
        max = max.max(e);
    }
    let n = (0..31).filter(|&i| word(d, 8 + 14 * i + 6) != 0).count() as u64;
    let det = (452 + 35 * l + 8 * u) / 2;
    let init = (4624 + 8 * n + 42 * l) / 2;
    let query = 24 / 2;
    let vol = 1392 / 2;
    let start = 1188 / 2;
    let c4 = 100 + det + init + query;
    let c5 = c4 + vol;
    let mut w = Vec::new();
    for c0 in [c4, c5] {
        for t in [434u64, 707, 1003, 1322] {
            w.push(c0 + t / 2);
        }
    }
    (w, c5 + vol + start)
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (s, _) = header(d)?;
    // the reference refuses a module whose stated size is larger than the loaded file
    if be32(d, 0) as usize > d.len() {
        return None;
    }
    let len = d[445] as usize;
    let (writes, end) = startup(d, s, len);
    // the reference's start-up counts from the run's origin, START_PHASE colour clocks before the
    // engine's aligned start; the first tick follows one period after the clock left here
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    {
        let mut hw = Hw { bus: &mut m.bus };
        for (i, w) in writes.iter().enumerate() {
            hw.bus.clock = origin + w;
            hw.vol(i % 4, 0);
        }
    }
    m.bus.clock = (origin + end).max(m.bus.clock);
    Some(Box::new(TomyTracker {
        d: d.to_vec(),
        sample_base: base.wrapping_add(s as u32),
        len,
        restart: word(d, 442) as usize,
        counter: 1,
        speed: 6,
        row: 0,
        pos: 0,
        dmabits: 0,
        port: 0,
        ch: [Chan::default(); 4],
        origin,
        songend: false,
    }))
}

#[cfg(test)]
#[path = "tomytracker_tests.rs"]
mod tests;
