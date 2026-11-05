//! Future Composer BSI (module magic `FUCO`, name prefixes `bfc.`, `bsi.`, `fc-bsi.`; UADE player
//! "FutureComposer BSI").
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. This is NOT Future Composer 1.3/1.4 (`futurecomposer.rs`):
//! a module has fixed-size blocks (four 256-byte voice sequences of one-byte entries, 64 patterns of
//! 256 bytes of notes and commands, 64 sixteen-byte instrument records, then plain PCM). Each voice
//! runs its own pattern; a tick processes voices 3, 2, 1, 0 every `speed + 1` ticks. Every write is
//! placed at the cycle count of the specification's ledger, including the raster wait of a looped
//! note (modelled from the beam phase of the reference's timeline).
//!
//! Choices where the specification is silent (all safe, none can panic or hang): bytes asked for
//! beyond the end of the file read as zero; a file shorter than 0x47FC bytes is not recognised;
//! sample starts and lengths wrap in 32 bits; a song end (0xFE entry or the 256-read limit) stops
//! the whole song from the next tick on, as the specification says.

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Future Composer BSI", detect, new };

/// Colour clocks between two ticks (the host's 50 Hz timer).
const TICK: u64 = 70940;
/// The first tick's start in the reference's timeline.
const E0: u64 = 75559;
const SEQ: usize = 0x0004;
const PAT: usize = 0x0404;
const INS: usize = 0x4408;
const SMP: usize = 0x47FC;

/// The format's period table (specification 1.5), indexed by note + transpose.
const PERIODS: [u16; 84] = [
    1712, 1616, 1520, 1440, 1360, 1280, 1200, 1136, 1072, 1016, 960, 904, //
    856, 808, 760, 720, 680, 640, 600, 568, 536, 508, 480, 452, //
    428, 404, 380, 360, 340, 320, 300, 284, 268, 254, 240, 226, //
    214, 202, 190, 180, 170, 165, 150, 142, 134, 127, 120, 113, //
    107, 101, 95, 90, 85, 83, 75, 71, 67, 64, 60, 57, //
    54, 51, 48, 45, 43, 41, 38, 36, 34, 32, 30, 28, //
    27, 25, 24, 23, 21, 20, 19, 18, 17, 16, 15, 14,
];

#[derive(Clone, Copy, PartialEq)]
enum Porta {
    Off,
    Up,
    Down,
}

#[derive(Clone, Copy)]
struct Voice {
    spos: u8,
    pnum: u16,
    ppos: u16,
    rep: u16,
    countdown: i16,
    duration: u8,
    inst: usize,
    transpose: u8,
    mode: Porta,
    pspeed: u16,
    period: u16,
    volume: u8,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            spos: 0,
            pnum: 0,
            ppos: 0xFFFF,
            rep: 1,
            countdown: 0,
            duration: 16,
            inst: 0,
            transpose: 0,
            mode: Porta::Off,
            pspeed: 0,
            period: 0,
            volume: 64,
        }
    }
}

pub struct FcBsi {
    d: Vec<u8>,
    base: u32,
    /// Sample start (offset in the module) of each of the 64 instruments.
    starts: [u32; 64],
    speed: u16,
    counter: i16,
    ended: bool,
    reads: u16,
    v: [Voice; 4],
    /// Ticks run so far (for the beam phase of the reference's timeline).
    ticks: u64,
    /// Cycle cursor of the current tick (half colour clocks) and the tick's start clock.
    u: u64,
    t0: u64,
}

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn detect(d: &[u8]) -> bool {
    d.len() >= SMP && &d[..4] == b"FUCO"
}

impl FcBsi {
    fn build(d: &[u8], base: u32) -> Self {
        let mut starts = [0u32; 64];
        let mut acc = SMP as u32;
        for (i, s) in starts.iter_mut().enumerate() {
            *s = acc;
            acc = acc.wrapping_add(be32(d, INS + 16 * i + 4));
        }
        FcBsi {
            d: d.to_vec(),
            base,
            starts,
            speed: 2,
            counter: 0,
            ended: false,
            reads: 0,
            v: [Voice::default(); 4],
            ticks: 0,
            u: 0,
            t0: 0,
        }
    }

    /// Moves the clock to the cursor and lets the caller write.
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.u / 2;
    }

    /// The beam's line at the cursor, in the reference's timeline.
    fn line(&self) -> u64 {
        (E0 + TICK * self.ticks.saturating_sub(1) + self.u / 2) / 227
    }

    fn raster_wait(&mut self) {
        for _ in 0..15 {
            self.u += 16;
            let l = self.line();
            loop {
                self.u += 16;
                let s = self.line();
                self.u += 9;
                if s != l {
                    break;
                }
            }
            self.u += 10;
        }
    }

    /// The song ends (specification 3.7 and 5.5): the voice's processing ends.
    fn song_end(&mut self) {
        self.ended = true;
        self.u += 192;
        self.u = 2 * (self.u / 2) + 16;
        self.u += 140;
    }

    /// One read counts towards the limit; true when the limit ends the song.
    fn limit(&mut self) -> bool {
        self.reads += 1;
        self.u += 55;
        if self.reads >= 256 {
            self.song_end();
            return true;
        }
        false
    }

    /// Pattern change (3.3); true when the song ended.
    fn pattern_change(&mut self, hw: &mut Hw, v: usize) -> bool {
        self.u += 67;
        self.v[v].ppos = 0;
        self.v[v].rep = self.v[v].rep.wrapping_sub(1);
        if self.v[v].rep != 0 {
            return false;
        }
        self.v[v].rep = 1;
        self.u += 26;
        loop {
            if self.limit() {
                return true;
            }
            self.u += 119;
            let sp = self.v[v].spos;
            let b = byte(&self.d, SEQ + 256 * v + sp as usize);
            let next = byte(&self.d, SEQ + 256 * v + sp.wrapping_add(1) as usize);
            self.v[v].spos = sp.wrapping_add(1);
            match b {
                0x00..=0x3F => {
                    self.v[v].pnum = b as u16;
                    self.u += 62;
                    return false;
                }
                0x40..=0x7F => {
                    self.v[v].rep = b as u16 - 0x3F;
                    self.u += 99;
                }
                0x80..=0xBF if b & 0x3F <= 0x23 => {
                    self.v[v].transpose = b & 0x3F;
                    self.u += 156;
                }
                0x80..=0xFB => {
                    self.u += if b < 0xC0 { 128 } else { 101 };
                    self.u += 16;
                    self.at(hw);
                    hw.bus.write_byte(0xBFE001, 2 * (b & 1));
                    self.u += 40;
                }
                0xFC => {
                    self.v[v].spos = sp.wrapping_add(2);
                    self.v[v].volume = next;
                    self.u += 101 + 14;
                    self.at(hw);
                    hw.vol(v, next as u16);
                    self.u += 72;
                }
                0xFD => {
                    self.v[v].spos = next;
                    self.v[v].rep = 1;
                    self.u += 152;
                }
                0xFE => {
                    self.u += 95;
                    self.song_end();
                    return true;
                }
                0xFF => {
                    self.v[v].spos = 0;
                    self.v[v].rep = 1;
                    self.u += 176;
                }
            }
        }
    }

    /// A note on voice `v` (3.4, 5.4).
    fn note(&mut self, hw: &mut Hw, v: usize, n: u8) {
        let vs = self.v[v];
        let rec = INS + 16 * vs.inst;
        let len = be32(&self.d, rec + 4);
        let loop_start = be32(&self.d, rec + 8);
        let loop_len = be32(&self.d, rec + 12);
        let start = self.base.wrapping_add(self.starts[vs.inst & 63]);
        self.u += 131;
        if len == 0 {
            self.u += 86;
        } else {
            let bit = 1u16 << v;
            self.u += 28;
            self.at(hw);
            hw.dmacon(bit);
            self.u += 4658 + 32;
            self.at(hw);
            hw.loc(v, start);
            self.u += 42;
            self.at(hw);
            hw.len(v, (len >> 1) as u16);
            let p = PERIODS.get(n as usize + vs.transpose as usize).copied().unwrap_or(0).wrapping_add(vs.pspeed);
            self.v[v].period = p;
            self.u += 88;
            self.at(hw);
            hw.per(v, p);
            self.u += 74;
            self.at(hw);
            hw.dmacon(0x8000 | bit);
            self.u += 29;
            if loop_len == 0 {
                self.u += 18;
                self.at(hw);
                hw.len(v, 1);
                self.u += 10;
            } else {
                self.u += 4;
                self.raster_wait();
                self.u += 42;
                self.at(hw);
                hw.len(v, (loop_len >> 1) as u16);
                self.u += 56;
                self.at(hw);
                hw.loc(v, start.wrapping_add(loop_start));
            }
            self.u += 32;
            self.at(hw);
            hw.vol(v, vs.volume as u16);
            self.u += 86;
        }
        let vs = &mut self.v[v];
        vs.countdown = vs.duration as i16;
        vs.ppos = vs.ppos.wrapping_add(1);
    }

    fn voice(&mut self, hw: &mut Hw, v: usize) {
        self.reads = 0;
        self.u += 95;
        let vs = self.v[v];
        if vs.mode != Porta::Off {
            self.u += 135;
            let p = if vs.mode == Porta::Up { vs.period.wrapping_sub(vs.pspeed) } else { vs.period.wrapping_add(vs.pspeed) };
            if vs.mode == Porta::Up {
                self.u += 4;
            }
            let mut q = p;
            if p <= 127 {
                self.u += 8;
                q = 127;
            }
            if p >= 2048 {
                self.u += 8;
                q = 2048;
            }
            self.u += 14;
            self.at(hw);
            hw.per(v, q);
            self.u += 14;
            self.v[v].period = q;
        }
        self.u += 37;
        if self.v[v].ppos & 0xFF == 0xFF && self.pattern_change(hw, v) {
            return;
        }
        loop {
            // the row countdown
            self.u += 41;
            let c = self.v[v].countdown.wrapping_sub(1);
            self.v[v].countdown = c;
            if c > 0 {
                self.u += 11 + 16;
                return;
            }
            if c < 0 {
                self.u += 11;
            }
            // read pattern bytes until a note, or the end of the pattern
            loop {
                if self.limit() {
                    return;
                }
                self.u += 127;
                let vs = &mut self.v[v];
                let idx = (vs.ppos.wrapping_add(vs.pnum.wrapping_mul(256))) as i16 as i64;
                let b = if idx >= 0 { byte(&self.d, PAT + idx as usize) } else { 0 };
                let step = |p: u16| (p & 0xFF00) | (p.wrapping_add(1) & 0xFF);
                match b {
                    0x00..=0x2F => {
                        self.u += 19;
                        self.note(hw, v, b);
                        return;
                    }
                    0xFF => break,
                    0x30..=0x6C => {
                        vs.pspeed = 2 * (b as u16 - 0x30);
                        vs.ppos = step(vs.ppos);
                        self.u += 191;
                    }
                    0x6D..=0x6F => {
                        vs.mode = match b {
                            0x6D => Porta::Down,
                            0x6E => Porta::Off,
                            _ => Porta::Up,
                        };
                        vs.ppos = step(vs.ppos);
                        self.u += match b {
                            0x6D => 141,
                            0x6E => 160,
                            _ => 179,
                        };
                    }
                    0x70..=0x7F => {
                        self.speed = (b & 0x0F) as u16;
                        vs.ppos = step(vs.ppos);
                        self.u += 131;
                    }
                    0x80..=0xBF => {
                        let dur = b & 0x3F;
                        vs.duration = if dur == 0 { 64 } else { dur };
                        vs.ppos = step(vs.ppos);
                        self.u += 171 + if dur == 0 { 8 } else { 0 };
                    }
                    0xC0..=0xFE => {
                        vs.inst = (b & 0x3F) as usize;
                        vs.ppos = step(vs.ppos);
                        self.u += 146;
                    }
                }
            }
            // end of pattern byte
            if self.pattern_change(hw, v) {
                return;
            }
        }
    }
}

impl Replayer for FcBsi {
    fn format(&self) -> &'static str {
        "Future Composer BSI"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.ended
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticks += 1;
        self.t0 = hw.bus.clock;
        self.u = 200;
        if self.ended {
            self.u += 20;
            self.at(hw);
            hw.dmacon(0x000F);
            return;
        }
        self.u += 35;
        self.counter = self.counter.wrapping_sub(1);
        if self.counter >= 0 {
            return;
        }
        self.counter = self.speed as i16;
        self.u += 32;
        for v in (0..4).rev() {
            self.u += 18;
            self.voice(hw, v);
            self.u += 10;
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    // the first tick follows E0 colour clocks after the run's origin; the song starts counting one
    // period earlier (the engine's aligned start is START_PHASE into a video frame)
    let t = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    m.bus.clock = (t + E0 - TICK).max(m.bus.clock);
    Some(Box::new(FcBsi::build(d, base)))
}

#[cfg(test)]
#[path = "futurecomposerbsi_tests.rs"]
mod tests;
