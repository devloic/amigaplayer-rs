//! The Yamaha YM2149 of the Atari ST, for the SNDH player (`sndh.rs`): three square-wave tone
//! generators, the 17-bit noise generator, the envelope generator with its 32 steps and ten
//! distinct shapes, the mixer and the 5-bit logarithmic output levels, run at the output rate.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the YM2149's data
//! sheet (register map and widths; tone f = clock / (16 TP); noise f = clock / (16 NP); envelope
//! ramp of 32 steps lasting 256 EP clocks; the CONT / ATT / ALT / HOLD shape bits; a 4-bit volume
//! register acts as the odd levels 2 v + 1 of the 32 envelope levels) and the commonly documented
//! noise register (a 17-bit shift register whose input is the exclusive-or of bits 0 and 2). The
//! structure of the output follows AtariAudio (MIT, see `LICENSE-AtariAudio`), read as a
//! behavioural reference: the chip's state machine runs at 250 kHz (the 2 MHz clock over 8, the
//! rate of its tone counters; the noise counter runs every second tick), every tick takes the
//! output level of the three channels' 5-bit levels together, each output sample is the average of
//! the ticks it spans (at the output rate), and a moving average of the last 2048 samples is
//! subtracted from it (the chip's output is not centred on zero).
//!
//! A tone period of 0 or 1 makes the channel's square wave run at the tick rate; a period of 0
//! holds it high (the sample-playing trick of writing the volume register at a high rate). When
//! the tune writes a period of 0 or 1 inside a timer interrupt handler, the channel's square wave
//! restarts when the handler ends (the "sync-square" effect: the handler's write is the sync).
//!
//! The levels of the three channels are summed by an analogue stage that saturates, so the output
//! is not the sum of the channels' own levels. The output is taken from a table measured on an ST:
//! the mixing table of AtariAudio 1.30 by Arnaud Carre (MIT, see `LICENSE-AtariAudio`; data
//! measured by Paulo Simoes on real hardware as 16 x 16 x 16 levels, log-interpolated to 32 per
//! channel by Arnaud Carre), embedded as `sndh_ym_mix.bin` (data only, no code). (An earlier
//! version fitted a model to it, 1 % rms off: the audio could not be sample-exact.)

use std::sync::OnceLock;

/// The chip's step rate: the 2 MHz clock over 8.
const STEP_HZ: u32 = 250_000;

/// The measured mixing table (`sndh_ym_mix.bin`, 32768 little-endian u16, index a | b << 5 |
/// c << 10): AtariAudio 1.30's `s_ym2149RecordedMixTable` (MIT, `LICENSE-AtariAudio`), measured
/// on an ST by Paulo Simoes (16 x 16 x 16) and log-interpolated to 32 levels a channel by Arnaud
/// Carre. Data only, extracted from `src/ym2149_tables.h` of AtariAudio 1.30.
static MIX_BIN: &[u8; 65536] = include_bytes!("sndh_ym_mix.bin");

/// The output for the three channels' levels a | b << 5 | c << 10.
pub(super) fn mix_table() -> &'static [u16] {
    static T: OnceLock<Vec<u16>> = OnceLock::new();
    T.get_or_init(|| MIX_BIN.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect())
}

/// The envelope's levels for a shape: 128 positions, the first pass (64) then the 64 it repeats
/// or holds; each 32-step segment is a ramp up or down, or the held level.
pub(super) fn envelope(shape: u8) -> [u8; 128] {
    let (cont, att, alt, hold) = (shape & 8 != 0, shape & 4 != 0, shape & 2 != 0, shape & 1 != 0);
    let ramp = |up: bool, i: usize| if up { i as u8 } else { 31 - i as u8 };
    let mut out = [0u8; 128];
    for seg in 0..4 {
        for i in 0..32 {
            out[seg * 32 + i] = if seg == 0 {
                ramp(att, i)
            } else if !cont {
                0
            } else if hold {
                // held at the end of the first ramp, or at the other end with ALTERNATE
                let end = if att { 31 } else { 0 };
                if alt { 31 - end } else { end }
            } else {
                // each further ramp reverses with ALTERNATE
                ramp(att ^ (alt && seg % 2 == 1), i)
            };
        }
    }
    out
}

#[derive(Clone)]
pub struct Ym {
    regs: [u8; 14],
    tone_cnt: [u32; 3],
    tone_period: [u32; 3],
    /// The square waves' states.
    edge: [bool; 3],
    /// The mixer's disable bits: a disabled source passes.
    tone_off: [bool; 3],
    noise_off: [bool; 3],
    /// A period of 0 or 1 was written in a timer interrupt: the square wave restarts at its end.
    edge_reset: [bool; 3],
    in_irq: bool,
    env: [u8; 128],
    env_cnt: u32,
    env_pos: i32,
    env_period: u32,
    noise_cnt: u32,
    noise_period: u32,
    noise_rack: u32,
    noise_hi: bool,
    noise_half: bool,
    host_rate: u32,
    /// The output rate accumulated towards the next chip step, in units of 1 / host rate.
    inner: u32,
    /// The moving average that is subtracted.
    dc: Vec<i32>,
    dc_pos: usize,
    dc_sum: i32,
}

/// The length of the moving average (2048 values, about 20 ms at 44.1 kHz).
const DC_LEN: usize = 2048;

impl Ym {
    pub fn new(host_rate: u32) -> Ym {
        let mut y = Ym {
            regs: [0; 14],
            tone_cnt: [0; 3],
            tone_period: [0; 3],
            edge: [true, true, false],
            tone_off: [false; 3],
            noise_off: [false; 3],
            edge_reset: [false; 3],
            in_irq: false,
            env: envelope(0),
            env_cnt: 0,
            env_pos: -64,
            env_period: 0,
            noise_cnt: 0,
            noise_period: 0,
            noise_rack: 1,
            noise_hi: true,
            noise_half: false,
            host_rate: host_rate.max(1),
            inner: 0,
            dc: vec![0; DC_LEN],
            dc_pos: 0,
            dc_sum: 0,
        };
        // the chip's reset: all registers zero but the mixer, which has every source off (the
        // square waves' own states are unpredictable on a chip: two high, one low, as in the
        // reference player's first run)
        for r in 0..14u8 {
            y.write(r, if r == 7 { 0x3F } else { 0 });
        }
        // (the envelope sits in its second pass, so that a tune that never writes the shape
        // register has a silent envelope rather than the shape 0 ramp's first level)
        y.env_pos = 0;
        y
    }

    pub fn write(&mut self, r: u8, v: u8) {
        const MASK: [u8; 14] = [0xFF, 0x0F, 0xFF, 0x0F, 0xFF, 0x0F, 0x1F, 0x3F, 0x1F, 0x1F, 0x1F, 0xFF, 0xFF, 0x0F];
        let r = r as usize;
        if r >= 14 {
            return;
        }
        self.regs[r] = v & MASK[r];
        match r {
            0..=5 => {
                let ch = r >> 1;
                self.tone_period[ch] = (self.regs[2 * ch + 1] as u32) << 8 | self.regs[2 * ch] as u32;
                if self.tone_period[ch] <= 1 && self.in_irq {
                    self.edge_reset[ch] = true;
                }
            }
            6 => self.noise_period = self.regs[6] as u32,
            7 => {
                for ch in 0..3 {
                    self.tone_off[ch] = v >> ch & 1 != 0;
                    self.noise_off[ch] = v >> (3 + ch) & 1 != 0;
                }
            }
            11 | 12 => self.env_period = (self.regs[12] as u32) << 8 | self.regs[11] as u32,
            13 => {
                self.env = envelope(self.regs[13]);
                self.env_pos = -64;
                self.env_cnt = 0;
            }
            _ => {}
        }
    }

    /// A timer interrupt handler starts or ends (the sync-square writes are recognised inside).
    pub fn set_in_irq(&mut self, inside: bool) {
        if !inside {
            for ch in 0..3 {
                if self.edge_reset[ch] {
                    self.edge[ch] = !self.edge[ch];
                    self.tone_cnt[ch] = 0;
                    self.edge_reset[ch] = false;
                }
            }
        }
        self.in_irq = inside;
    }

    /// One step of the chip (8 clocks); the output for the three channels' levels now.
    fn step(&mut self) -> i32 {
        // the channels that sound, by the state before this step
        let mut pass = [false; 3];
        for ch in 0..3 {
            pass[ch] = (self.edge[ch] || self.tone_off[ch]) && (self.noise_hi || self.noise_off[ch]);
        }
        for ch in 0..3 {
            self.tone_cnt[ch] += 1;
            if self.tone_cnt[ch] >= self.tone_period[ch] {
                self.edge[ch] = !self.edge[ch];
                self.tone_cnt[ch] = 0;
            }
            if self.tone_period[ch] == 0 {
                pass[ch] = true;
            }
        }
        self.env_cnt += 1;
        if self.env_cnt >= self.env_period {
            self.env_pos += 1;
            if self.env_pos > 0 {
                self.env_pos &= 63;
            }
            self.env_cnt = 0;
        }
        self.noise_half = !self.noise_half;
        if self.noise_half {
            self.noise_cnt += 1;
            if self.noise_cnt >= self.noise_period {
                let bit = (self.noise_rack ^ (self.noise_rack >> 2)) & 1;
                self.noise_hi = bit != 0;
                self.noise_rack = (self.noise_rack >> 1) | (bit << 16);
                self.noise_cnt = 0;
            }
        }
        let env = self.env[(self.env_pos + 64) as usize] as usize;
        let mut idx = 0usize;
        for ch in 0..3 {
            if pass[ch] {
                let v = self.regs[8 + ch];
                let level = if v & 0x10 != 0 { env } else { ((v & 15) as usize) << 1 | 1 };
                idx |= level << (5 * ch);
            }
        }
        mix_table()[idx] as i32
    }

    /// The next output sample, in 16-bit units (the average of the chip's steps it spans, less
    /// the moving average of the last 2048 samples).
    pub fn sample(&mut self) -> i32 {
        let (mut acc, mut n) = (0i32, 0i32);
        loop {
            acc += self.step();
            n += 1;
            self.inner += self.host_rate;
            if self.inner >= STEP_HZ {
                break;
            }
        }
        self.inner -= STEP_HZ;
        let v = acc / n;
        self.dc_sum -= self.dc[self.dc_pos];
        self.dc_sum += v;
        self.dc[self.dc_pos] = v;
        self.dc_pos = (self.dc_pos + 1) & (DC_LEN - 1);
        v - (self.dc_sum >> 11)
    }
}
