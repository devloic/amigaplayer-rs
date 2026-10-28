//! 8520 CIA: the two interval timers, interrupt control and port A (for
//! the LED/filter bit). Timers count E-clock ticks.

pub const ECLOCK_PAL: f64 = 709_379.0;

#[derive(Clone)]
struct Timer {
    latch: u16,
    counter: i64,
    cr: u8,
}

/// Power-on state: latch and counter $FFFF, stopped (the 6526/8520 reset sets the timer
/// latches to all ones; UADE's CIA_reset and WinUAE do the same). A player that starts a
/// timer before writing its latch (the Musicline Editor's) therefore gets its first
/// underflow 65536 E-clock ticks later, not at once.
impl Default for Timer {
    fn default() -> Self {
        Timer { latch: 0xFFFF, counter: 0xFFFF, cr: 0 }
    }
}

impl Timer {
    fn running(&self) -> bool {
        self.cr & 1 != 0
    }
    /// Returns the number of underflows during `ticks` E-clock ticks.
    fn advance(&mut self, ticks: u64) -> u32 {
        if !self.running() {
            return 0;
        }
        let period = self.latch as i64 + 1;
        self.counter -= ticks as i64;
        let mut n = 0;
        if self.counter < 0 {
            if self.cr & 0x08 != 0 {
                // one-shot: stop and reload
                self.cr &= !1;
                self.counter = self.latch as i64;
                return 1;
            }
            n = ((-self.counter - 1) / period + 1) as u32;
            self.counter += n as i64 * period;
        }
        n
    }

    /// E-clock ticks until the next underflow, if running.
    fn until_underflow(&self) -> Option<u64> {
        self.running().then(|| self.counter.max(0) as u64 + 1)
    }
}

#[derive(Clone, Default)]
pub struct Cia {
    ta: Timer,
    tb: Timer,
    pub icr_mask: u8,
    pub icr_data: u8,
    pub pra: u8,
    pub prb: u8,
    ddra: u8,
    ddrb: u8,
    sdr: u8,
    /// ICR bits (TA=1, TB=2) that fired and are unmasked, not yet taken by the interrupt dispatcher.
    pending: u8,
}

impl Cia {
    pub fn read(&mut self, reg: u32) -> u8 {
        match reg & 0xF {
            0x0 => self.pra,
            0x1 => self.prb,
            0x2 => self.ddra,
            0x3 => self.ddrb,
            0x4 => self.ta.counter as u16 as u8,
            0x5 => ((self.ta.counter as u16) >> 8) as u8,
            0x6 => self.tb.counter as u16 as u8,
            0x7 => ((self.tb.counter as u16) >> 8) as u8,
            0xC => self.sdr,
            0xD => {
                let v = self.icr_data | if self.icr_data & self.icr_mask != 0 { 0x80 } else { 0 };
                self.icr_data = 0;
                v
            }
            0xE => self.ta.cr,
            0xF => self.tb.cr,
            _ => 0,
        }
    }

    pub fn write(&mut self, reg: u32, v: u8) {
        match reg & 0xF {
            0x0 => self.pra = v,
            0x1 => self.prb = v,
            0x2 => self.ddra = v,
            0x3 => self.ddrb = v,
            0x4 => self.ta.latch = (self.ta.latch & 0xFF00) | v as u16,
            0x5 => {
                self.ta.latch = (self.ta.latch & 0x00FF) | ((v as u16) << 8);
                if !self.ta.running() {
                    self.ta.counter = self.ta.latch as i64;
                    if self.ta.cr & 0x08 != 0 {
                        self.ta.cr |= 1;
                    }
                }
            }
            0x6 => self.tb.latch = (self.tb.latch & 0xFF00) | v as u16,
            0x7 => {
                self.tb.latch = (self.tb.latch & 0x00FF) | ((v as u16) << 8);
                if !self.tb.running() {
                    self.tb.counter = self.tb.latch as i64;
                    if self.tb.cr & 0x08 != 0 {
                        self.tb.cr |= 1;
                    }
                }
            }
            0xC => self.sdr = v,
            0xD => {
                if v & 0x80 != 0 {
                    self.icr_mask |= v & 0x1F;
                } else {
                    self.icr_mask &= !(v & 0x1F);
                }
            }
            0xE => {
                if v & 0x10 != 0 {
                    self.ta.counter = self.ta.latch as i64;
                }
                self.ta.cr = v & !0x10;
            }
            0xF => {
                if v & 0x10 != 0 {
                    self.tb.counter = self.tb.latch as i64;
                }
                self.tb.cr = v & !0x10;
            }
            _ => {}
        }
    }

    /// Advance both timers. Returns the ICR bits (TA=1, TB=2) that fired.
    pub fn advance(&mut self, ticks: u64) -> u8 {
        let mut fired = 0;
        if self.ta.advance(ticks) > 0 {
            fired |= 1;
        }
        // timer B counting timer A underflows (CRB bits 5-6 = 10) is not used
        // by music players, so B always counts E-clock ticks here.
        if self.tb.advance(ticks) > 0 {
            fired |= 2;
        }
        self.icr_data |= fired;
        self.pending |= fired & self.icr_mask;
        fired
    }

    /// The unmasked underflows since the last call (masked at the moment they happened).
    pub fn take_pending(&mut self) -> u8 {
        std::mem::take(&mut self.pending)
    }

    /// E-clock ticks until either timer underflows.
    pub fn until_underflow(&self) -> Option<u64> {
        match (self.ta.until_underflow(), self.tb.until_underflow()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

}
