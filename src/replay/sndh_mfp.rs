//! The Atari ST's MC68901 multi-function peripheral (MFP) for the SNDH player (`sndh.rs`): the
//! four timers A to D with their prescalers, the interrupt enable / pending / mask / in-service
//! registers, the vector register and the interrupt delivery to the CPU (level 6).
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the MC68901's data
//! sheet and the Atari ST hardware documentation (register map at $FFFA01 + 2 n; the timers'
//! delay-mode prescalers 4, 10, 16, 50, 64, 100 and 200 of the 2.4576 MHz clock; the data
//! registers counting down and reloading, 0 meaning 256; the sixteen interrupt sources, numbered
//! 15 (GPI7, timer A) down to 0 with the higher number the higher priority; vector number = the
//! vector register's high nibble + the source number). The behaviours of the MFP with a "no
//! software end-of-interrupt" vector register (the default here) are those of the data sheet's
//! automatic end of interrupt mode: the in-service bit is never set.
//!
//! Timer A's and B's event-count mode counts the external events given with `event_a` (the STE
//! DMA sound's end-of-frame signal); the pulse-width modes are not modelled (the timer stops).

/// The MFP clock, Hz.
pub const MFP_HZ: u64 = 2_457_600;

/// Register offsets (odd bytes of $FFFA00).
mod r {
    pub const IERA: usize = 0x07;
    pub const IERB: usize = 0x09;
    pub const IPRA: usize = 0x0B;
    pub const IPRB: usize = 0x0D;
    pub const ISRA: usize = 0x0F;
    pub const ISRB: usize = 0x11;
    pub const IMRA: usize = 0x13;
    pub const IMRB: usize = 0x15;
    pub const VR: usize = 0x17;
    pub const TACR: usize = 0x19;
    pub const TBCR: usize = 0x1B;
    pub const TCDCR: usize = 0x1D;
    pub const TADR: usize = 0x1F;
    pub const TBDR: usize = 0x21;
    pub const TCDR: usize = 0x23;
    pub const TDDR: usize = 0x25;
}

/// The interrupt source number (bit of the 16-bit pending word) of each timer A, B, C, D.
const TIMER_IRQ: [u8; 4] = [13, 8, 5, 4];

const PRESCALE: [u32; 8] = [0, 4, 10, 16, 50, 64, 100, 200];

#[derive(Clone, Copy, Default)]
struct Timer {
    /// Control: low 3 bits the delay-mode prescaler choice, bit 3 (timers A and B) the event mode.
    ctrl: u8,
    /// The value the counter reloads (0 = 256).
    reload: u8,
    /// The counter now (1..=256).
    count: u16,
    /// MFP clocks accumulated towards the next prescaled tick.
    sub: u32,
}

impl Timer {
    fn period(&self) -> u32 {
        if self.ctrl & 8 != 0 { 0 } else { PRESCALE[(self.ctrl & 7) as usize] }
    }
    fn reload_value(&self) -> u16 {
        if self.reload == 0 { 256 } else { self.reload as u16 }
    }
}

#[derive(Clone)]
pub struct Mfp {
    regs: [u8; 0x30],
    timers: [Timer; 4],
    /// The pending word: bit n is source n.
    pending: u16,
    /// The CPU clocks' remainder towards the MFP clock: `acc` counts in units of 1/CPU_HZ * MFP_HZ.
    acc: u64,
    /// The CPU's clock, Hz, the unit of `advance`.
    cpu_hz: u64,
}

impl Mfp {
    pub fn new(cpu_hz: u64) -> Mfp {
        let mut m = Mfp { regs: [0; 0x30], timers: [Timer::default(); 4], pending: 0, acc: 0, cpu_hz };
        // the operating system leaves timer C's interrupt enabled and unmasked (its vector is
        // the tune's to set); no software end of interrupt
        m.regs[r::IERB] = 1 << 5;
        m.regs[r::IMRB] = 1 << 5;
        m.regs[r::VR] = 0x40;
        m
    }

    fn word(&self, a: usize, b: usize) -> u16 {
        (self.regs[a] as u16) << 8 | self.regs[b] as u16
    }

    pub fn read(&self, off: usize) -> u8 {
        let off = off & 0xFF;
        if off & 1 == 0 || off >= self.regs.len() {
            return 0xFF;
        }
        match off {
            r::IPRA => (self.pending >> 8) as u8,
            r::IPRB => self.pending as u8,
            r::TADR => self.counter(0),
            r::TBDR => self.counter(1),
            r::TCDR => self.counter(2),
            r::TDDR => self.counter(3),
            _ => self.regs[off],
        }
    }

    fn counter(&self, t: usize) -> u8 {
        let c = &self.timers[t];
        if c.count == 0 { c.reload } else { (c.count & 0xFF) as u8 }
    }

    pub fn write(&mut self, off: usize, v: u8) {
        let off = off & 0xFF;
        if off & 1 == 0 || off >= self.regs.len() {
            return;
        }
        match off {
            r::IERA | r::IERB => {
                self.regs[off] = v;
                // a disabled source has no pending interrupt
                if off == r::IERA {
                    self.pending &= !(((!v) as u16) << 8);
                } else {
                    self.pending &= !((!v) as u16);
                }
            }
            r::IPRA => self.pending &= (v as u16) << 8 | 0xFF,
            r::IPRB => self.pending &= 0xFF00 | v as u16,
            r::ISRA | r::ISRB => self.regs[off] &= v,
            r::TACR | r::TBCR => {
                let t = (off - r::TACR) / 2;
                self.set_ctrl(t, v & 0x0F);
                self.regs[off] = v;
            }
            r::TCDCR => {
                self.set_ctrl(2, (v >> 4) & 7);
                self.set_ctrl(3, v & 7);
                self.regs[off] = v;
            }
            r::TADR | r::TBDR | r::TCDR | r::TDDR => {
                let t = (off - r::TADR) / 2;
                self.timers[t].reload = v;
                // a stopped timer loads its counter
                if self.timers[t].period() == 0 && self.timers[t].ctrl & 8 == 0 {
                    self.timers[t].count = self.timers[t].reload_value();
                }
                self.regs[off] = v;
            }
            _ => self.regs[off] = v,
        }
    }

    fn set_ctrl(&mut self, t: usize, v: u8) {
        let tm = &mut self.timers[t];
        let was_stopped = tm.period() == 0 && tm.ctrl & 8 == 0;
        tm.ctrl = v;
        // a pulse-width mode (A and B, bit 3 with bits 0-2 other than 0) is not modelled: stopped
        if v & 8 != 0 && v & 7 != 0 {
            tm.ctrl = 0;
        }
        if was_stopped && (tm.period() != 0 || tm.ctrl & 8 != 0) {
            // starting: the prescaler begins afresh, the counter keeps what was loaded
            tm.sub = 0;
            if tm.count == 0 {
                tm.count = tm.reload_value();
            }
        }
    }

    fn raise(&mut self, t: usize) {
        let n = TIMER_IRQ[t];
        let en = if n >= 8 { self.regs[r::IERA] >> (n - 8) } else { self.regs[r::IERB] >> n };
        if en & 1 != 0 {
            self.pending |= 1 << n;
        }
    }

    /// Counts `steps` timer decrements of timer `t`, raising its interrupt on each underflow.
    fn count_down(&mut self, t: usize, mut steps: u64) {
        while steps > 0 {
            let tm = &mut self.timers[t];
            if tm.count == 0 {
                tm.count = tm.reload_value();
            }
            let c = tm.count as u64;
            if steps >= c {
                steps -= c;
                tm.count = tm.reload_value();
                self.raise(t);
            } else {
                tm.count -= steps as u16;
                steps = 0;
            }
        }
    }

    /// An external event on timer A's input (the STE DMA sound's end of frame).
    pub fn event_a(&mut self) {
        if self.timers[0].ctrl & 8 != 0 {
            self.count_down(0, 1);
        }
    }

    /// Lets `cycles` CPU clocks pass.
    pub fn advance(&mut self, cycles: u64) {
        if cycles == 0 {
            return;
        }
        self.acc += cycles * MFP_HZ;
        let clocks = self.acc / self.cpu_hz;
        self.acc %= self.cpu_hz;
        if clocks == 0 {
            return;
        }
        for t in 0..4 {
            let p = self.timers[t].period();
            if p == 0 {
                continue;
            }
            let total = self.timers[t].sub as u64 + clocks;
            self.timers[t].sub = (total % p as u64) as u32;
            self.count_down(t, total / p as u64);
        }
    }

    /// CPU clocks until the next timer underflow, if a timer runs.
    pub fn next_event(&self) -> Option<u64> {
        let mut best: Option<u64> = None;
        for t in 0..4 {
            let tm = &self.timers[t];
            let p = tm.period() as u64;
            if p == 0 {
                continue;
            }
            let count = if tm.count == 0 { tm.reload_value() } else { tm.count } as u64;
            // MFP clocks to the underflow: the rest of the current prescale period plus the others
            let clocks = (p - tm.sub as u64) + (count - 1) * p;
            // CPU clocks until that many MFP clocks have passed (rounded up)
            let need = clocks * self.cpu_hz;
            let cpu = (need.saturating_sub(self.acc)).div_ceil(MFP_HZ).max(1);
            best = Some(best.map_or(cpu, |b| b.min(cpu)));
        }
        best
    }

    /// The highest-priority pending, unmasked source that is not blocked by one in service.
    fn deliverable(&self) -> Option<u8> {
        let ready = self.pending & self.word(r::IMRA, r::IMRB);
        let in_service = self.word(r::ISRA, r::ISRB);
        let blocked_from = if in_service == 0 { 0 } else { 16 - in_service.leading_zeros() as u8 };
        (0..16u8).rev().find(|&n| ready & (1 << n) != 0 && n >= blocked_from)
    }

    /// Whether an interrupt is waiting for the CPU.
    pub fn irq(&self) -> bool {
        self.deliverable().is_some()
    }

    /// The interrupt acknowledge cycle: the vector number of the interrupt taken. Clears its
    /// pending bit; with the vector register's software end-of-interrupt bit (3) set it also
    /// marks it in service.
    pub fn acknowledge(&mut self) -> u32 {
        let Some(n) = self.deliverable() else {
            // spurious: the vector of source 15 is as near as the MFP comes
            return (self.regs[r::VR] & 0xF0) as u32 | 15;
        };
        self.pending &= !(1 << n);
        if self.regs[r::VR] & 8 != 0 {
            if n >= 8 {
                self.regs[r::ISRA] |= 1 << (n - 8);
            } else {
                self.regs[r::ISRB] |= 1 << n;
            }
        }
        (self.regs[r::VR] & 0xF0) as u32 | n as u32
    }
}
