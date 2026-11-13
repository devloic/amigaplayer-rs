//! The NES APU (the sound part of the Ricoh 2A03 / 2A07): two pulse channels, triangle, noise,
//! DMC, the frame counter, length counters, envelopes, sweep and the nonlinear mixer, stepped
//! one CPU cycle at a time.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the NESdev wiki's
//! pages "APU" (register map, $4015), "APU Frame Counter" (the 4- and 5-step sequences, their
//! NTSC and PAL timings, the IRQ flag, the effect of a $4017 write), "APU Length Counter" (the
//! table, halt, enable), "APU Envelope", "APU Sweep" (ones' / two's complement negation,
//! muting, the divider), "APU Pulse" (duty sequences read downwards, period < 8 muting),
//! "APU Triangle" (32-step sequence, linear counter), "APU Noise" (periods in CPU cycles, the
//! 15-bit LFSR and its two modes), "APU DMC" (rate tables in CPU cycles, memory reader, output
//! unit) and "APU Mixer" (the lookup-table form of the nonlinear mix). No emulator's code was
//! consulted.
//!
//! Timing: the pulse timers run on APU cycles (every second CPU cycle); the triangle timer on
//! every CPU cycle; the noise and DMC periods are counted in CPU cycles as the tables give
//! them. The frame counter's steps, half-way APU cycles on the wiki, fall on the CPU cycle
//! twice their count (NTSC 7457, 14913, 22371, 29829, wrap at 29830). A $4017 write resets the
//! sequence 3 or 4 CPU cycles later (on the APU cycle's parity), with both clocks at once when
//! its bit 7 is set. Not modelled: a $4011 write landing on an output clock, the early RP2A03's
//! noise period, the frame counter's IRQ held over three cycles.

/// Length counter loads, by the 5-bit index of $4003 / $4007 / $400B / $400F bits 7-3.
pub const LENGTH: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28, 32,
    30,
];

/// Pulse duty sequences, in the order of the sequencer's index.
pub const DUTY: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 0, 0, 0],
    [1, 0, 0, 1, 1, 1, 1, 1],
];

pub const TRIANGLE: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// Noise periods in CPU cycles.
pub const NOISE_NTSC: [u16; 16] = [4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068];
pub const NOISE_PAL: [u16; 16] = [4, 8, 14, 30, 60, 88, 118, 148, 188, 236, 354, 472, 708, 944, 1890, 3778];

/// DMC output periods in CPU cycles.
pub const DMC_NTSC: [u16; 16] = [428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54];
pub const DMC_PAL: [u16; 16] = [398, 354, 316, 298, 276, 236, 210, 198, 176, 148, 132, 118, 98, 78, 66, 50];

/// The frame counter's steps in CPU cycles: quarter-frame clocks, half-frame clocks, the cycle
/// that sets the IRQ flag (4-step only) and the sequence's length.
struct Seq {
    quarter: [u32; 4],
    half: [u32; 2],
    irq: u32,
    len: u32,
}

const SEQ4_NTSC: Seq = Seq { quarter: [7457, 14913, 22371, 29829], half: [14913, 29829], irq: 29829, len: 29830 };
const SEQ5_NTSC: Seq = Seq { quarter: [7457, 14913, 22371, 37281], half: [14913, 37281], irq: u32::MAX, len: 37282 };
const SEQ4_PAL: Seq = Seq { quarter: [8313, 16627, 24939, 33253], half: [16627, 33253], irq: 33253, len: 33254 };
const SEQ5_PAL: Seq = Seq { quarter: [8313, 16627, 24939, 41565], half: [16627, 41565], irq: u32::MAX, len: 41566 };

#[derive(Clone, Default, Debug)]
pub struct Envelope {
    pub start: bool,
    pub divider: u8,
    pub decay: u8,
    /// The register's low 4 bits, the loop (= length halt) and constant-volume flags.
    pub param: u8,
    pub looped: bool,
    pub constant: bool,
}

impl Envelope {
    fn clock(&mut self) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = self.param;
        } else if self.divider == 0 {
            self.divider = self.param;
            if self.decay > 0 {
                self.decay -= 1;
            } else if self.looped {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }
    pub fn volume(&self) -> u8 {
        if self.constant {
            self.param
        } else {
            self.decay
        }
    }
    fn set(&mut self, v: u8) {
        self.param = v & 15;
        self.constant = v & 0x10 != 0;
        self.looped = v & 0x20 != 0;
    }
}

#[derive(Clone, Default, Debug)]
pub struct Pulse {
    /// 0 for pulse 1 (ones' complement sweep negation), 1 for pulse 2.
    pub ch: u8,
    pub enabled: bool,
    pub duty: u8,
    pub seq: u8,
    pub period: u16,
    pub timer: u16,
    pub length: u8,
    pub env: Envelope,
    pub sweep_on: bool,
    pub sweep_period: u8,
    pub sweep_negate: bool,
    pub sweep_shift: u8,
    pub sweep_reload: bool,
    pub sweep_div: u8,
}

impl Pulse {
    pub fn target(&self) -> u16 {
        let c = self.period >> self.sweep_shift;
        if self.sweep_negate {
            self.period.saturating_sub(c + if self.ch == 0 { 1 } else { 0 })
        } else {
            self.period + c
        }
    }
    pub fn muted(&self) -> bool {
        self.period < 8 || self.target() > 0x7FF
    }
    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            self.seq = self.seq.wrapping_sub(1) & 7;
        } else {
            self.timer -= 1;
        }
    }
    fn clock_sweep(&mut self) {
        if self.sweep_div == 0 && self.sweep_on && self.sweep_shift > 0 && !self.muted() {
            self.period = self.target();
        }
        if self.sweep_div == 0 || self.sweep_reload {
            self.sweep_div = self.sweep_period;
            self.sweep_reload = false;
        } else {
            self.sweep_div -= 1;
        }
    }
    pub fn output(&self) -> u8 {
        if self.length == 0 || self.muted() || DUTY[self.duty as usize][self.seq as usize] == 0 {
            0
        } else {
            self.env.volume()
        }
    }
    fn write(&mut self, r: u16, v: u8) {
        match r {
            0 => {
                self.duty = v >> 6;
                self.env.set(v);
            }
            1 => {
                self.sweep_on = v & 0x80 != 0;
                self.sweep_period = (v >> 4) & 7;
                self.sweep_negate = v & 8 != 0;
                self.sweep_shift = v & 7;
                self.sweep_reload = true;
            }
            2 => self.period = (self.period & 0x700) | v as u16,
            _ => {
                self.period = (self.period & 0xFF) | ((v as u16 & 7) << 8);
                if self.enabled {
                    self.length = LENGTH[(v >> 3) as usize];
                }
                self.seq = 0;
                self.env.start = true;
            }
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct Triangle {
    pub enabled: bool,
    pub control: bool,
    pub reload_value: u8,
    pub linear: u8,
    pub reload: bool,
    pub period: u16,
    pub timer: u16,
    pub seq: u8,
    pub length: u8,
}

impl Triangle {
    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            if self.linear > 0 && self.length > 0 {
                self.seq = (self.seq + 1) & 31;
            }
        } else {
            self.timer -= 1;
        }
    }
    fn clock_linear(&mut self) {
        if self.reload {
            self.linear = self.reload_value;
        } else if self.linear > 0 {
            self.linear -= 1;
        }
        if !self.control {
            self.reload = false;
        }
    }
    pub fn output(&self) -> u8 {
        TRIANGLE[self.seq as usize]
    }
}

#[derive(Clone, Debug)]
pub struct Noise {
    pub enabled: bool,
    pub env: Envelope,
    pub mode: bool,
    pub period: u16,
    pub timer: u16,
    pub lfsr: u16,
    pub length: u8,
}

impl Noise {
    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period - 1;
            let other = if self.mode { 6 } else { 1 };
            let fb = (self.lfsr ^ (self.lfsr >> other)) & 1;
            self.lfsr = (self.lfsr >> 1) | fb << 14;
        } else {
            self.timer -= 1;
        }
    }
    pub fn output(&self) -> u8 {
        if self.length == 0 || self.lfsr & 1 != 0 {
            0
        } else {
            self.env.volume()
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct Dmc {
    pub irq_enabled: bool,
    pub looped: bool,
    pub period: u16,
    pub timer: u16,
    pub level: u8,
    pub sample_addr: u16,
    pub sample_len: u16,
    pub addr: u16,
    pub remaining: u16,
    pub buffer: Option<u8>,
    pub shift: u8,
    pub bits: u8,
    pub silence: bool,
    pub irq: bool,
}

impl Dmc {
    fn restart(&mut self) {
        self.addr = self.sample_addr;
        self.remaining = self.sample_len;
    }
    fn clock_timer(&mut self) {
        if self.timer > 0 {
            self.timer -= 1;
            return;
        }
        self.timer = self.period - 1;
        if !self.silence {
            if self.shift & 1 != 0 {
                if self.level <= 125 {
                    self.level += 2;
                }
            } else if self.level >= 2 {
                self.level -= 2;
            }
        }
        self.shift >>= 1;
        self.bits = self.bits.saturating_sub(1);
        if self.bits == 0 {
            self.bits = 8;
            match self.buffer.take() {
                Some(b) => {
                    self.silence = false;
                    self.shift = b;
                }
                None => self.silence = true,
            }
        }
    }
    /// The address the memory reader wants to fetch, when its buffer is empty and bytes remain.
    pub fn wants(&self) -> Option<u16> {
        (self.buffer.is_none() && self.remaining > 0).then_some(self.addr)
    }
    pub fn fill(&mut self, v: u8) {
        self.buffer = Some(v);
        self.addr = if self.addr == 0xFFFF { 0x8000 } else { self.addr + 1 };
        self.remaining -= 1;
        if self.remaining == 0 {
            if self.looped {
                self.restart();
            } else if self.irq_enabled {
                self.irq = true;
            }
        }
    }
}

pub struct Apu {
    pub pal: bool,
    pub pulse: [Pulse; 2],
    pub tri: Triangle,
    pub noise: Noise,
    pub dmc: Dmc,
    /// The frame counter: 5-step mode, IRQ inhibit, the CPU cycle within the sequence, the
    /// IRQ flag, and a pending reset (CPU cycles to go, the value written).
    pub five: bool,
    pub inhibit: bool,
    pub fc: u32,
    pub frame_irq: bool,
    pending: Option<(u8, u8)>,
    /// Odd CPU cycle: the second half of an APU cycle.
    odd: bool,
    pulse_table: [f32; 31],
    tnd_table: [f32; 203],
}

impl Apu {
    pub fn new(pal: bool) -> Apu {
        let mut pulse_table = [0f32; 31];
        for (n, t) in pulse_table.iter_mut().enumerate().skip(1) {
            *t = 95.52 / (8128.0 / n as f32 + 100.0);
        }
        let mut tnd_table = [0f32; 203];
        for (n, t) in tnd_table.iter_mut().enumerate().skip(1) {
            *t = 163.67 / (24329.0 / n as f32 + 100.0);
        }
        let mut a = Apu {
            pal,
            pulse: [Pulse { ch: 0, ..Default::default() }, Pulse { ch: 1, ..Default::default() }],
            tri: Triangle::default(),
            noise: Noise { enabled: false, env: Envelope::default(), mode: false, period: 4, timer: 0, lfsr: 1, length: 0 },
            dmc: Dmc { bits: 8, silence: true, ..Default::default() },
            five: false,
            inhibit: false,
            fc: 0,
            frame_irq: false,
            pending: None,
            odd: false,
            pulse_table,
            tnd_table,
        };
        a.dmc.period = a.dmc_table()[0];
        a.noise.period = a.noise_table()[0];
        a
    }

    fn noise_table(&self) -> &'static [u16; 16] {
        if self.pal {
            &NOISE_PAL
        } else {
            &NOISE_NTSC
        }
    }
    fn dmc_table(&self) -> &'static [u16; 16] {
        if self.pal {
            &DMC_PAL
        } else {
            &DMC_NTSC
        }
    }
    fn seq(&self) -> &'static Seq {
        match (self.pal, self.five) {
            (false, false) => &SEQ4_NTSC,
            (false, true) => &SEQ5_NTSC,
            (true, false) => &SEQ4_PAL,
            (true, true) => &SEQ5_PAL,
        }
    }

    pub fn write(&mut self, a: u16, v: u8) {
        match a {
            0x4000..=0x4003 => self.pulse[0].write(a - 0x4000, v),
            0x4004..=0x4007 => self.pulse[1].write(a - 0x4004, v),
            0x4008 => {
                self.tri.control = v & 0x80 != 0;
                self.tri.reload_value = v & 0x7F;
            }
            0x400A => self.tri.period = (self.tri.period & 0x700) | v as u16,
            0x400B => {
                self.tri.period = (self.tri.period & 0xFF) | ((v as u16 & 7) << 8);
                if self.tri.enabled {
                    self.tri.length = LENGTH[(v >> 3) as usize];
                }
                self.tri.reload = true;
            }
            0x400C => self.noise.env.set(v),
            0x400E => {
                self.noise.mode = v & 0x80 != 0;
                self.noise.period = self.noise_table()[(v & 15) as usize];
            }
            0x400F => {
                if self.noise.enabled {
                    self.noise.length = LENGTH[(v >> 3) as usize];
                }
                self.noise.env.start = true;
            }
            0x4010 => {
                self.dmc.irq_enabled = v & 0x80 != 0;
                self.dmc.looped = v & 0x40 != 0;
                self.dmc.period = self.dmc_table()[(v & 15) as usize];
                if !self.dmc.irq_enabled {
                    self.dmc.irq = false;
                }
            }
            0x4011 => self.dmc.level = v & 0x7F,
            0x4012 => self.dmc.sample_addr = 0xC000 | (v as u16) << 6,
            0x4013 => self.dmc.sample_len = (v as u16) << 4 | 1,
            0x4015 => {
                for (i, p) in self.pulse.iter_mut().enumerate() {
                    p.enabled = v & (1 << i) != 0;
                    if !p.enabled {
                        p.length = 0;
                    }
                }
                self.tri.enabled = v & 4 != 0;
                if !self.tri.enabled {
                    self.tri.length = 0;
                }
                self.noise.enabled = v & 8 != 0;
                if !self.noise.enabled {
                    self.noise.length = 0;
                }
                if v & 0x10 == 0 {
                    self.dmc.remaining = 0;
                } else if self.dmc.remaining == 0 {
                    self.dmc.restart();
                }
                self.dmc.irq = false;
            }
            0x4017 => {
                self.inhibit = v & 0x40 != 0;
                if self.inhibit {
                    self.frame_irq = false;
                }
                self.pending = Some((if self.odd { 4 } else { 3 }, v));
            }
            _ => {}
        }
    }

    /// A read of $4015 (it clears the frame interrupt flag).
    pub fn read_status(&mut self) -> u8 {
        let v = (self.pulse[0].length > 0) as u8
            | ((self.pulse[1].length > 0) as u8) << 1
            | ((self.tri.length > 0) as u8) << 2
            | ((self.noise.length > 0) as u8) << 3
            | ((self.dmc.remaining > 0) as u8) << 4
            | (self.frame_irq as u8) << 6
            | (self.dmc.irq as u8) << 7;
        self.frame_irq = false;
        v
    }

    fn quarter(&mut self) {
        self.pulse[0].env.clock();
        self.pulse[1].env.clock();
        self.noise.env.clock();
        self.tri.clock_linear();
    }

    fn half(&mut self) {
        for p in &mut self.pulse {
            if p.length > 0 && !p.env.looped {
                p.length -= 1;
            }
            p.clock_sweep();
        }
        if self.tri.length > 0 && !self.tri.control {
            self.tri.length -= 1;
        }
        if self.noise.length > 0 && !self.noise.env.looped {
            self.noise.length -= 1;
        }
    }

    fn clock_frame(&mut self) {
        if let Some((n, v)) = self.pending {
            if n <= 1 {
                self.pending = None;
                self.five = v & 0x80 != 0;
                self.fc = 0;
                if self.five {
                    self.quarter();
                    self.half();
                }
                return;
            }
            self.pending = Some((n - 1, v));
        }
        self.fc += 1;
        let s = self.seq();
        if s.quarter.contains(&self.fc) {
            self.quarter();
        }
        if s.half.contains(&self.fc) {
            self.half();
        }
        if self.fc == s.irq && !self.inhibit {
            self.frame_irq = true;
        }
        if self.fc >= s.len {
            self.fc = 0;
        }
    }

    /// One CPU cycle; returns the mixed output (0 to about 1).
    pub fn clock(&mut self) -> f32 {
        self.clock_frame();
        self.tri.clock_timer();
        self.noise.clock_timer();
        self.dmc.clock_timer();
        if self.odd {
            self.pulse[0].clock_timer();
            self.pulse[1].clock_timer();
        }
        self.odd = !self.odd;
        self.mix()
    }

    pub fn mix(&self) -> f32 {
        let p = self.pulse[0].output() + self.pulse[1].output();
        let t = 3 * self.tri.output() as usize + 2 * self.noise.output() as usize + self.dmc.level as usize;
        self.pulse_table[p as usize] + self.tnd_table[t]
    }
}
