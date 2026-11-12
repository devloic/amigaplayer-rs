//! The Yamaha YM2149 of the Atari ST, for the SNDH player (`sndh.rs`): three square-wave tone
//! generators, the 17-bit noise generator, the envelope generator with its 32 steps and ten
//! distinct shapes, the mixer and the 5-bit logarithmic output levels.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the YM2149's data
//! sheet (register map and widths; tone f = clock / (16 TP); noise f = clock / (16 NP); envelope
//! ramp of 32 steps lasting 256 EP clocks; the CONT / ATT / ALT / HOLD shape bits; a 4-bit
//! volume register acts as the odd levels 2 v + 1 of the 32 envelope levels; about 1.5 dB per
//! level) and the commonly documented noise register (a 17-bit shift register whose input is the
//! exclusive-or of bits 0 and 2). The output levels are the 1.5 dB geometric series, 2^((n-31)/4)
//! of full scale for level n; the chip's measured, non-linear summing of the three channels is not
//! modelled (the channels are added). The AY-3-8910 of the Spectrum player (`ay_chip.rs`) is a
//! different chip (16 envelope steps, no 5-bit levels) and is left alone.
//!
//! The chip runs in steps of 8 input clocks (250 kHz at the ST's 2 MHz), the rate of its tone
//! counters; the noise counter runs every second step. A tone period of 0 holds that channel's
//! square wave high (the sample-playing trick of writing the volume register at a high rate); a
//! noise period of 0 acts as 1, and an envelope period of 0 as 1.

#[derive(Clone)]
pub struct Ym {
    regs: [u8; 16],
    tone_cnt: [u16; 3],
    tone: [bool; 3],
    noise_cnt: u8,
    lfsr: u32,
    noise: bool,
    half: bool,
    env_cnt: u16,
    /// Steps into the current ramp (0..=31).
    env_pos: u8,
    env_up: bool,
    env_held: bool,
    /// Output amplitude of each of the 32 levels, full scale 1.0; level 0 is silent.
    pub dac: [f32; 32],
}

/// The levels fall by a factor 2^(-1/STEP_LOG2) per level: 1.77 dB, so that the 16 volumes are
/// 3.5 dB apart. (The data sheet says about 1.5 dB; the output of the reference player's chip,
/// measured at each of the 16 volumes of one channel, falls 3.5 dB per volume step on average.)
const STEP_LOG2: f32 = 3.4;

/// The register masks: the unused bits read back as zero (registers 14 and 15 are the I/O
/// ports and keep every bit).
const MASK: [u8; 16] = [0xFF, 0x0F, 0xFF, 0x0F, 0xFF, 0x0F, 0x1F, 0xFF, 0x1F, 0x1F, 0x1F, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF];

impl Default for Ym {
    fn default() -> Self {
        Ym::new()
    }
}

impl Ym {
    pub fn new() -> Ym {
        let mut dac = [0f32; 32];
        for (n, d) in dac.iter_mut().enumerate().skip(1) {
            *d = 2f32.powf((n as f32 - 31.0) / STEP_LOG2);
        }
        Ym {
            regs: [0; 16],
            tone_cnt: [0; 3],
            tone: [false; 3],
            noise_cnt: 0,
            lfsr: 1,
            noise: true,
            half: false,
            env_cnt: 0,
            env_pos: 0,
            env_up: false,
            env_held: true,
            dac,
        }
    }

    /// The chip's reset: all registers zero but the mixer, which has every source off.
    pub fn reset(&mut self) {
        let dac = self.dac;
        *self = Ym::new();
        self.dac = dac;
        self.regs[7] = 0x3F;
    }

    pub fn read(&self, r: u8) -> u8 {
        self.regs[(r & 15) as usize]
    }

    pub fn write(&mut self, r: u8, v: u8) {
        let r = (r & 15) as usize;
        self.regs[r] = v & MASK[r];
        if r == 13 {
            self.env_cnt = 0;
            self.env_pos = 0;
            self.env_up = v & 4 != 0;
            self.env_held = false;
        }
    }

    /// The tone period register pair of channel `ch` (0 stays 0).
    fn tone_period(&self, ch: usize) -> u16 {
        self.regs[2 * ch] as u16 | (self.regs[2 * ch + 1] as u16) << 8
    }

    /// The envelope's level (0..=31) now.
    pub fn env_level(&self) -> u8 {
        if self.env_up {
            self.env_pos
        } else {
            31 - self.env_pos
        }
    }

    /// The envelope's next step, by the shape bits of register 13.
    fn env_step(&mut self) {
        if self.env_held {
            return;
        }
        if self.env_pos < 31 {
            self.env_pos += 1;
            return;
        }
        let s = self.regs[13];
        let (cont, alt, hold) = (s & 8 != 0, s & 2 != 0, s & 1 != 0);
        if !cont {
            // shapes 0-7: one ramp, then 0
            self.env_up = false;
            self.env_held = true;
            self.env_pos = 31;
        } else if hold {
            // the level stays where the ramp ended, or at the other end with ALTERNATE
            if alt {
                self.env_up = !self.env_up;
            }
            self.env_held = true;
        } else {
            if alt {
                self.env_up = !self.env_up;
            }
            self.env_pos = 0;
        }
    }

    /// One step of 8 input clocks; returns the output amplitude of the three channels together
    /// (1.0 for one channel at full volume, saturating towards 1.5 for three).
    pub fn step(&mut self) -> f32 {
        for ch in 0..3 {
            let p = self.tone_period(ch);
            self.tone_cnt[ch] = self.tone_cnt[ch].wrapping_add(1);
            if self.tone_cnt[ch] >= p {
                self.tone_cnt[ch] = 0;
                self.tone[ch] = !self.tone[ch];
            }
        }
        self.half = !self.half;
        if self.half {
            self.noise_cnt = self.noise_cnt.wrapping_add(1);
            if self.noise_cnt >= self.regs[6] & 0x1F {
                self.noise_cnt = 0;
                let bit = (self.lfsr ^ (self.lfsr >> 2)) & 1;
                self.lfsr = (self.lfsr >> 1) | (bit << 16);
                self.noise = bit != 0;
            }
        }
        // the envelope counts in steps of 8 clocks: a ramp of 32 steps lasts 256 EP clocks
        self.env_cnt = self.env_cnt.wrapping_add(1);
        let ep = (self.regs[11] as u16 | (self.regs[12] as u16) << 8).max(1);
        if self.env_cnt >= ep {
            self.env_cnt = 0;
            self.env_step();
        }
        let mix = self.regs[7];
        let env = self.env_level();
        let mut sum = 0.0;
        for ch in 0..3 {
            let t = self.tone[ch] || self.tone_period(ch) == 0 || mix & (1 << ch) != 0;
            let n = self.noise || mix & (8 << ch) != 0;
            if t && n {
                let v = self.regs[8 + ch];
                let level = if v & 0x10 != 0 { env } else { (v & 15) * 2 + 1 };
                sum += self.dac[level as usize];
            }
        }
        // The channels share one output stage, so the sum saturates: the output levels of the
        // reference player's chip with one, two and three channels at full volume are 1 : 1.15 :
        // 1.13 in RMS (independent tones), which s / (1 + s / 3) reproduces (scaled for 1.0 at one
        // channel at full volume).
        sum / (1.0 + sum / 3.0) * (4.0 / 3.0)
    }
}
