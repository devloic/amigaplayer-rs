//! The Atari ST's MC68901 multi-function peripheral (MFP) for the SNDH player (`sndh.rs`): the
//! four timers A to D with their prescalers and the GPI7 line, as the tunes use them for a
//! digidrum, a SID voice, a sync-buzzer or the STE DMA sound's end-of-frame count.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the MC68901's data
//! sheet and the Atari ST hardware documentation (register map at $FFFA01 + 2 n; the timers'
//! delay-mode prescalers 4, 10, 16, 50, 64, 100 and 200 of the 2.4576 MHz clock; the data
//! registers counting down and reloading; the vectors of the ST's operating system, 0x134 timer A,
//! 0x120 B, 0x114 C, 0x110 D, 0x13C GPI7), and with the timing of AtariAudio (MIT, see
//! `LICENSE-AtariAudio`) as a behavioural reference, so that the register writes the interrupt
//! handlers make fall on the same output sample as in that player.
//!
//! The timing model is the host's: the timers advance once per output sample (`tick`), not per
//! CPU clock. Each tick adds the timer's prescaled rate to an accumulator and takes one data
//! register step each time the accumulator passes the output rate, so a timer runs at its exact
//! rate on average and its underflows land on the output-sample grid. A timer counts only while its
//! interrupt-enable bit is set; the interrupt-mask bit gates the call of the handler; there is no
//! pending register, no in-service register and no vector number (the vectors are fixed).
//!
//! Event-count mode (timer A with control bit 3, and the GPI7 line) counts the external events
//! given with `event_a`, the STE DMA sound's end of frame. The pulse-width modes are not modelled
//! (they count events as the event mode does).

/// The MFP clock, Hz.
pub const MFP_HZ: u32 = 2_457_600;

/// Timers A, B, C, D, then the GPI7 line (which behaves as an event timer counting to 1).
pub const TIMERS: usize = 5;

/// The interrupt vector address of each of them.
pub const VECTORS: [u32; TIMERS] = [0x134, 0x120, 0x114, 0x110, 0x13C];

#[derive(Clone, Copy, Default)]
struct Timer {
    /// The interrupt-enable bit: only an enabled timer counts.
    enable: bool,
    /// The interrupt-mask bit: an underflow calls the handler only when set.
    mask: bool,
    /// Control: low 3 bits the delay-mode prescaler choice (0 stopped), bit 3 the event mode.
    ctrl: u8,
    /// The counter now.
    count: u8,
    /// The value it reloads.
    reload: u8,
    /// MFP clocks accumulated towards the next step, in units of 1 / host rate.
    inner: u32,
    /// An external event waits to be counted.
    event: bool,
}

impl Timer {
    fn restart(&mut self) {
        self.inner = 0;
        self.count = self.reload;
    }

    fn counter_mode(&self) -> bool {
        self.ctrl & 7 != 0 && self.ctrl & 8 == 0
    }

    fn set_enable(&mut self, en: bool) {
        if self.enable != en && en && self.counter_mode() {
            self.restart();
        }
        self.enable = en;
    }

    fn set_data(&mut self, v: u8) {
        self.reload = v;
        if self.ctrl == 0 {
            self.restart();
        }
    }

    /// One output sample passes; true when the timer underflowed and its handler is to be called.
    fn tick(&mut self, host_rate: u32) -> bool {
        let mut fire = false;
        if self.enable {
            if self.ctrl & 8 != 0 {
                if self.event {
                    self.count = self.count.wrapping_sub(1);
                    if self.count == 0 {
                        self.count = self.reload;
                        fire = true;
                    }
                    self.event = false;
                }
            } else if self.ctrl & 7 != 0 {
                const PRESCALE: [u32; 8] = [0, 4, 10, 16, 50, 64, 100, 200];
                self.inner += MFP_HZ / PRESCALE[(self.ctrl & 7) as usize];
                // (nearly always zero or one pass; a prescale of 4 at a low output rate a few)
                while self.inner >= host_rate {
                    self.count = self.count.wrapping_sub(1);
                    if self.count == 0 {
                        self.count = self.reload;
                        fire = true;
                    }
                    self.inner -= host_rate;
                }
            }
        }
        fire && self.mask
    }
}

#[derive(Clone)]
pub struct Mfp {
    regs: [u8; 0x30],
    timers: [Timer; TIMERS],
    host_rate: u32,
}

impl Mfp {
    pub fn new(host_rate: u32) -> Mfp {
        let mut m = Mfp { regs: [0; 0x30], timers: [Timer::default(); TIMERS], host_rate: host_rate.max(1) };
        // the operating system leaves timer C's interrupt enabled and unmasked (its vector is the
        // tune's to set); GPI7 is an event timer with a count of 1
        m.timers[2].enable = true;
        m.timers[2].mask = true;
        m.timers[4].ctrl = 8;
        m.timers[4].reload = 1;
        m.timers[4].count = 1;
        m
    }

    pub fn read(&self, off: usize) -> u8 {
        let off = off & 0xFF;
        if off & 1 == 0 || off >= self.regs.len() {
            return 0xFF;
        }
        match off {
            0x1F => self.timers[0].count,
            0x21 => self.timers[1].count,
            0x23 => self.timers[2].count,
            0x25 => self.timers[3].count,
            _ => self.regs[off],
        }
    }

    pub fn write(&mut self, off: usize, v: u8) {
        let off = off & 0xFF;
        if off & 1 == 0 || off >= self.regs.len() {
            return;
        }
        let t = &mut self.timers;
        match off {
            0x19 => t[0].ctrl = v & 0x0F,
            0x1B => t[1].ctrl = v & 0x0F,
            0x1D => {
                t[2].ctrl = (v >> 4) & 7;
                t[3].ctrl = v & 7;
            }
            0x1F => t[0].set_data(v),
            0x21 => t[1].set_data(v),
            0x23 => t[2].set_data(v),
            0x25 => t[3].set_data(v),
            0x07 => {
                t[0].set_enable(v & 0x20 != 0);
                t[1].set_enable(v & 0x01 != 0);
                t[4].set_enable(v & 0x80 != 0);
            }
            0x09 => {
                t[2].set_enable(v & 0x20 != 0);
                t[3].set_enable(v & 0x10 != 0);
            }
            0x13 => {
                t[0].mask = v & 0x20 != 0;
                t[1].mask = v & 0x01 != 0;
                t[4].mask = v & 0x80 != 0;
            }
            0x15 => {
                t[2].mask = v & 0x20 != 0;
                t[3].mask = v & 0x10 != 0;
            }
            _ => {}
        }
        self.regs[off] = v;
    }

    /// One output sample passes for timer `t` (0 to 4); true when its handler is to be called.
    pub fn tick(&mut self, t: usize) -> bool {
        self.timers[t].tick(self.host_rate)
    }

    /// The STE DMA sound's end-of-frame signal: an event for timer A and for GPI7.
    pub fn event_a(&mut self) {
        self.timers[0].event = true;
        self.timers[4].event = true;
    }
}
