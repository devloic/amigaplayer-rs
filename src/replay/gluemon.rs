//! GlueMon (GlueMaster / Northstar; Eagleplayer "GlueMon", module prefixes `glue.` and `gm.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes and
//! cycle costs of the 68k reference player; no original code. A module is the 8-byte mark `GLUE`
//! plus four bytes, an 8-character name, start volume, decay step and decay floor for three or
//! four voices, speed and rows per pattern, four 32-byte waveforms that loop for the whole song,
//! an order list and the patterns (one byte per voice per row). Voice 3 is either a fourth
//! melodic voice or, when the word at 0x1A reads `RM`, a drum voice with eight drum kinds that
//! play the player's own noise buffer (a shift-and-add generator rewritten every tick), its drum
//! wave, voice 3's waveform or the pattern data, with a per-tick period sweep. Every write is
//! placed at the clock of the original's code from the specification's cycle ledger.
//!
//! The 32-byte drum wave is generated here from the specification's rule (the square's two levels
//! were taken from the reference's audio DMA fetches, a black-box run).
//!
//! Choices where the specification is silent (all safe, none can panic or hang): speed or rows of
//! zero make the file not a module; bytes asked for beyond the end of the file read as zero; a
//! pattern number of 64 or more plays pattern 0; a note byte whose period index is past the table
//! (98..253 other than 0xFE) is ignored like 0xFF; the song-length and pattern-count info of the
//! original is not reported (the host does not show them).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "GlueMon", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// Colour clock at which the reference enters the initialisation routine.
const INIT_AT: u64 = 415;
const HEAD: [u8; 8] = [0x47, 0x4C, 0x55, 0x45, 0xB8, 0xB3, 0xAA, 0xBA];

const PERIODS: [u16; 49] = [
    3424, 3232, 3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1808, 1712, 1616, 1524, 1440, 1356, 1280,
    1208, 1140, 1076, 1016, 960, 904, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 452, 428, 404, 381,
    360, 339, 320, 302, 285, 269, 254, 240, 226, 214,
];

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn detect(d: &[u8]) -> bool {
    d.len() > 0x9F && d[..8] == HEAD
}

/// The drum wave: a four-byte square (two high, two low), a rising ramp from 0 in steps of 10
/// (20 bytes) and a falling ramp from 180 in steps of 20 (8 bytes).
fn drum_wave() -> [u8; 32] {
    let mut w = [0u8; 32];
    w[..4].copy_from_slice(&[0x7F, 0x7F, 0x81, 0x81]);
    for i in 0..20 {
        w[4 + i] = (10 * i) as u8;
    }
    for i in 0..8 {
        w[24 + i] = (180 - 20 * i) as u8;
    }
    w
}

#[derive(Clone, Copy, PartialEq)]
enum Src {
    DrumWave,
    Noise,
    Wave3,
}

pub struct GlueMon {
    d: Vec<u8>,
    base: u32,
    noise: u32,
    drum: bool,
    start: [u16; 4],
    decay: [u16; 4],
    floor: [u16; 4],
    speed: u8,
    rowbytes: u16,
    patterns_at: usize,
    vol: [u16; 4],
    tc: u8,
    pos: u16,
    ord: u16,
    pat: usize,
    ended: bool,
    songend: bool,
    r: u32,
    dp: u16,
    ds: u16,
    /// Master volume and the two balance levels (the host sets all three to 64).
    master: u32,
    bal: [u32; 2],
}

impl GlueMon {
    fn build(d: &[u8], base: u32, noise: u32) -> Option<Self> {
        let speed = byte(d, 0x1C);
        let rows = byte(d, 0x1D);
        if speed == 0 || rows == 0 {
            return None;
        }
        let drum = byte(d, 0x1A) == 0x52 && byte(d, 0x1B) == 0x4D;
        let mut start = [0u16; 4];
        let mut decay = [0u16; 4];
        let mut floor = [0u16; 4];
        for v in 0..4 {
            if v == 3 && drum {
                decay[3] = 1;
            } else {
                start[v] = byte(d, 0x10 + 3 * v) as u16;
                decay[v] = byte(d, 0x11 + 3 * v) as u16;
                floor[v] = byte(d, 0x12 + 3 * v) as u16;
            }
        }
        let n = byte(d, 0x9E);
        Some(GlueMon {
            d: d.to_vec(),
            base,
            noise,
            drum,
            start,
            decay,
            floor,
            speed,
            rowbytes: (rows as u16) * 4,
            patterns_at: 8 + ((n as usize + 0x97) & 0xFF),
            vol: [0; 4],
            tc: speed - 1,
            pos: (rows as u16) * 4 - 4,
            ord: (n as u16).wrapping_sub(2) & 0xFF,
            pat: 0,
            ended: false,
            songend: false,
            r: 0x986F_A0B9,
            dp: 0,
            ds: 0,
            master: 64,
            bal: [64, 64],
        })
    }

    fn pattern_addr(&self) -> u32 {
        self.base.wrapping_add((self.patterns_at + self.pat * self.rowbytes as usize) as u32)
    }

    fn row_byte(&self, v: usize) -> u8 {
        byte(&self.d, self.patterns_at + self.pat * self.rowbytes as usize + self.pos as usize + v)
    }

    fn refill_noise(&mut self, hw: &mut Hw) {
        let mut buf = [0u8; 32];
        for i in 0..8 {
            self.r = self.r.rotate_left(7).wrapping_add(0x6ECA_756D) ^ 0x9E59_A92B;
            buf[4 * i..4 * i + 4].copy_from_slice(&self.r.to_be_bytes());
        }
        hw.bus.write_bytes(self.noise, &buf);
    }

    fn src_addr(&self, s: Src) -> u32 {
        match s {
            Src::DrumWave => self.noise + 32,
            Src::Noise => self.noise,
            Src::Wave3 => self.base + 0x7E,
        }
    }

    fn run(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        let mut t: u64 = 194;
        self.refill_noise(hw);
        t += 76 * 8 + 28;
        for v in 0..4 {
            t += 33;
            if self.vol[v] > self.floor[v] {
                self.vol[v] = self.vol[v].wrapping_sub(self.decay[v]);
                t += 28;
            }
        }
        t += 49;
        self.tc = self.tc.wrapping_add(1);
        let mut sweep = false;
        if self.tc < self.speed {
            t += 23;
            if self.drum {
                t += 10;
                sweep = true;
            }
        } else {
            self.tc = 0;
            t += 69;
            self.pos = self.pos.wrapping_add(4);
            if self.pos >= self.rowbytes {
                t += 99;
                self.pos = 0;
                self.ord = self.ord.wrapping_add(1);
                let mut e = byte(&self.d, 0x9F + self.ord as usize);
                if e == 0xFF {
                    t += 173;
                    self.ord = 0;
                    e = byte(&self.d, 0x9F);
                    if self.ended {
                        t += 64;
                        self.songend = true;
                    } else {
                        t += 26;
                        self.ended = true;
                    }
                    t += 132;
                }
                // a pattern number past the 64 the format has: pattern 0 (unspecified)
                self.pat = if e >= 64 { 0 } else { e as usize };
                t += 60;
            }
            t += 28;
            let voices = if self.drum { 3 } else { 4 };
            for v in 0..voices {
                if v == 3 {
                    t += 21;
                }
                let b = self.row_byte(v);
                t += 31;
                if b == 0xFE {
                    self.vol[v] = 0;
                    t += 26;
                } else {
                    t += if v == 3 { 19 } else { 17 };
                    if b != 0xFF && (b >> 1) < 49 {
                        t += 76;
                        hw.bus.clock = t0 + t / 2;
                        hw.per(v, PERIODS[(b >> 1) as usize]);
                        self.vol[v] = self.start[v];
                        t += 20;
                        if v == 3 {
                            t += 10;
                        }
                    } else if b != 0xFF {
                        // unsupported note byte: ignored (the fixed costs of a note are kept)
                        t += 96;
                        if v == 3 {
                            t += 10;
                        }
                    }
                }
            }
            if self.drum {
                t += 21;
                let b = self.row_byte(3);
                t += 33;
                if b != 0xFF {
                    let kind = match b {
                        0xC8..=0xCF => Some(b - 0xC8),
                        _ => None,
                    };
                    let (src, set) = match kind {
                        Some(j) => {
                            t += 17 * (j as u64 + 1);
                            let (src, dp, ds, vol, dec) = match j {
                                0 => (Src::DrumWave, 0x0710, 0xFF01, 64, 8),
                                1 => (Src::Noise, 0x0710, 0x0014, 64, 4),
                                2 => (Src::Wave3, 0x01C4, 0xFFCE, 64, 4),
                                3 => (Src::Wave3, 0x025C, 0xFFCE, 64, 4),
                                4 => (Src::Wave3, 0x0388, 0xFFCE, 64, 4),
                                5 => (Src::Wave3, 0x04B8, 0xFFCE, 64, 4),
                                6 => (Src::Noise, 0x025C, 0xFFF6, 48, 16),
                                _ => (Src::Noise, 0x025C, 0xFFF6, 40, 2),
                            };
                            t += if matches!(j, 0 | 1 | 6 | 7) { 86 } else { 90 };
                            (Some(src), Some((dp, ds, vol, dec)))
                        }
                        None => {
                            t += 17 * 8;
                            (None, None)
                        }
                    };
                    if let Some((dp, ds, vol, dec)) = set {
                        self.dp = dp;
                        self.ds = ds;
                        self.vol[3] = vol;
                        self.decay[3] = dec;
                    }
                    let addr = match src {
                        Some(s) => self.src_addr(s),
                        None => self.pattern_addr(),
                    };
                    t += 16;
                    hw.bus.clock = t0 + t / 2;
                    hw.dmacon(0x0008);
                    t += 16;
                    hw.bus.clock = t0 + t / 2;
                    hw.loc(3, addr);
                    t += 16;
                    hw.bus.clock = t0 + t / 2;
                    hw.dmacon(0x8008);
                }
                sweep = true;
            }
        }
        if sweep {
            self.dp = self.dp.wrapping_sub(self.ds);
            t += 48;
            hw.bus.clock = t0 + t / 2;
            hw.per(3, self.dp);
        }
        t += 116;
        let gl = (self.bal[0] * self.master) >> 6;
        let gr = (self.bal[1] * self.master) >> 6;
        let value = |v: usize, g: u32| ((self.vol[v] as u32 * g) as u16 >> 6) as u16;
        for (i, (v, g)) in [(0, gl), (3, gl), (1, gr), (2, gr)].into_iter().enumerate() {
            if i == 2 {
                t += 92;
            }
            t += 96;
            hw.bus.clock = t0 + t / 2;
            hw.vol(v, value(v, g));
        }
        t += 192;
        hw.bus.clock = t0 + t / 2;
    }
}

impl Replayer for GlueMon {
    fn format(&self) -> &'static str {
        "GlueMon"
    }

    fn title(&self) -> String {
        let name = &self.d[8..16.min(self.d.len())];
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        name[..end].iter().map(|&b| if (32..127).contains(&b) { b as char } else { '?' }).collect()
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

/// The initialisation routine's writes at their ledger clocks; returns the cycle count at its end.
fn init_writes(hw: &mut Hw, call: u64, base: u32, drum: bool) -> u64 {
    let mut t: u64 = 0;
    let at = |hw: &mut Hw, t: u64| hw.bus.clock = call + t / 2;
    t += 366;
    at(hw, t);
    hw.dmacon(0x000F);
    for v in 0..4 {
        t += [36, 32, 32, 48][v];
        at(hw, t);
        hw.loc(v, base + 0x1E + 0x20 * v as u32);
        t += 16;
        at(hw, t);
        hw.len(v, 16);
    }
    t += 16;
    at(hw, t);
    hw.dmacon(0x800F);
    t += if drum { 2491 } else { 2513 };
    at(hw, t);
    hw.vol(0, 0);
    for v in 1..4 {
        t += 16;
        at(hw, t);
        hw.vol(v, 0);
    }
    t + 320
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let buf = m.alloc(64);
    if buf == 0 {
        return None;
    }
    m.bus.write_bytes(buf + 32, &drum_wave());
    let r = GlueMon::build(d, base, buf)?;
    // the reference's start-up counts from the run's origin, START_PHASE colour clocks before the
    // engine's aligned start; the first tick follows one period after the clock left here
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let end = {
        let mut hw = Hw { bus: &mut m.bus };
        init_writes(&mut hw, origin + INIT_AT, base, r.drum)
    };
    m.bus.clock = (origin + INIT_AT + end / 2).max(m.bus.clock);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "gluemon_tests.rs"]
mod tests;
