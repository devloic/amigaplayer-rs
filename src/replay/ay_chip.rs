//! The General Instrument AY-3-8910/8912 sound generator for the AY player (`ay.rs`): three
//! square-wave tone generators, the 17-bit noise generator, the envelope generator with its
//! sixteen shapes, the mixer and the 16-level logarithmic output.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from General
//! Instrument's AY-3-8910/8912 data sheet (register map and widths; tone f = clock/(16 TP);
//! noise f = clock/(16 NP); envelope period 256 EP / clock for its 16 steps; the shape table of
//! CONTINUE/ATTACK/ALTERNATE/HOLD; the 16 volume levels about 3 dB apart) and the commonly
//! documented noise register (a 17-bit shift register fed back from bits 0 and 3). No emulator's
//! source code or measured table was consulted or transcribed: the output levels are the data
//! sheet's 3 dB steps, 2^((n-15)/2) of full scale for level n, with level 0 silent.
//!
//! The chip runs in steps of 8 input clocks (the tone counters' rate); noise and envelope count
//! every second step. A period register of 0 acts as 1.

#[derive(Clone)]
pub struct Ay {
    regs: [u8; 16],
    tone_cnt: [u16; 3],
    tone: [bool; 3],
    noise_cnt: u8,
    lfsr: u32,
    noise: bool,
    /// The divide-by-two between the tone rate and the noise/envelope rate.
    half: bool,
    env_cnt: u16,
    /// Steps into the current ramp (0..=15).
    env_pos: u8,
    /// The current ramp rises.
    env_up: bool,
    env_held: bool,
    /// The output level of each of the 16 steps, full scale 1.0.
    pub dac: [f32; 16],
}

/// The register masks: the unused bits of each register read back as zero.
const MASK: [u8; 16] = [0xFF, 0x0F, 0xFF, 0x0F, 0xFF, 0x0F, 0x1F, 0xFF, 0x1F, 0x1F, 0x1F, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF];

impl Default for Ay {
    fn default() -> Self {
        Ay::new()
    }
}

impl Ay {
    pub fn new() -> Ay {
        let mut dac = [0f32; 16];
        for (n, d) in dac.iter_mut().enumerate().skip(1) {
            *d = 2f32.powf((n as f32 - 15.0) / 2.0);
        }
        Ay {
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

    /// The chip's reset: all registers zero.
    pub fn reset(&mut self) {
        let dac = self.dac;
        *self = Ay::new();
        self.dac = dac;
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

    fn tone_period(&self, ch: usize) -> u16 {
        (self.regs[2 * ch] as u16 | (self.regs[2 * ch + 1] as u16) << 8).max(1)
    }

    /// The noise shift register.
    pub fn lfsr_state(&self) -> u32 {
        self.lfsr
    }

    pub fn env_level(&self) -> u8 {
        if self.env_up {
            self.env_pos
        } else {
            15 - self.env_pos
        }
    }

    /// The envelope's next step, by the shape bits of register 13.
    fn env_step(&mut self) {
        if self.env_held {
            return;
        }
        if self.env_pos < 15 {
            self.env_pos += 1;
            return;
        }
        let s = self.regs[13];
        let (cont, alt, hold) = (s & 8 != 0, s & 2 != 0, s & 1 != 0);
        if !cont {
            // shapes 0-7: one ramp, then 0
            self.env_up = false;
            self.env_held = true;
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

    /// One step of 8 input clocks; returns the output level (0..=15, 0 also for an output
    /// that is off) of the three channels.
    pub fn step(&mut self) -> [u8; 3] {
        for ch in 0..3 {
            let p = self.tone_period(ch);
            self.tone_cnt[ch] += 1;
            if self.tone_cnt[ch] >= p {
                self.tone_cnt[ch] = 0;
                self.tone[ch] = !self.tone[ch];
            }
        }
        self.half = !self.half;
        if self.half {
            self.noise_cnt += 1;
            if self.noise_cnt >= (self.regs[6] & 0x1F).max(1) {
                self.noise_cnt = 0;
                let bit = (self.lfsr ^ (self.lfsr >> 3)) & 1;
                self.lfsr = (self.lfsr >> 1) | (bit << 16);
                self.noise = self.lfsr & 1 != 0;
            }
            self.env_cnt = self.env_cnt.wrapping_add(1);
            let ep = (self.regs[11] as u16 | (self.regs[12] as u16) << 8).max(1);
            if self.env_cnt >= ep {
                self.env_cnt = 0;
                self.env_step();
            }
        }
        let mix = self.regs[7];
        let env = self.env_level();
        let mut out = [0u8; 3];
        for (ch, o) in out.iter_mut().enumerate() {
            let t = self.tone[ch] || mix & (1 << ch) != 0;
            let n = self.noise || mix & (8 << ch) != 0;
            if t && n {
                let v = self.regs[8 + ch];
                *o = if v & 0x10 != 0 { env } else { v & 15 };
            }
        }
        out
    }
}
