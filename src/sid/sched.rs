// Port of NostalgicPlayer's C# port of libsidplayfp (by Simon White, Antti Lankila and
// Leandro Nini): Source/Ports/LibSidPlayFp/EventScheduler.cs, Event.cs, EventCallback.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibSidPlayFp);
// the upstream files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more information."
//
// Structure changed for Rust: upstream's events are objects with a callback, linked
// through a `next` reference. Here every event of the machine has a fixed number
// (`Ev`), the list is linked through an array of those numbers, and the machine
// (c64.rs) calls the event's code when `clock` hands its number out. The order in
// which events fire is upstream's: by trigger time, and among equal times in the
// order they were scheduled.

/// The scheduler's clock: twice the system clock (one count per phase).
pub type EventClock = i64;

/// C64 system runs actions at system clock high and low states. The PHI1 corresponds
/// to the auxiliary chip activity and PHI2 to CPU activity. For any clock, PHI1s are
/// before PHI2s.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Phi1 = 0,
    Phi2 = 1,
}

/// An event of the machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ev(pub u8);

impl Ev {
    /// "CPU-nosteal"
    pub const CPU_NOSTEAL: Ev = Ev(0);
    /// "CPU-steal"
    pub const CPU_STEAL: Ev = Ev(1);
    /// "Remove IRQ"
    pub const CPU_CLEAR_INT: Ev = Ev(2);
    /// the VIC-II's own event (Mos656x is an Event)
    pub const VIC: Ev = Ev(3);
    /// "Update AEC signal"
    pub const VIC_BADLINE: Ev = Ev(4);
    /// "RasterY changed"
    pub const VIC_RASTER_Y: Ev = Ev(5);
    /// "Trigger lightpen"
    pub const VIC_LIGHTPEN: Ev = Ev(6);

    /// First event of CIA 1; CIA `n`'s event `k` is `CIA_BASE + n * CIA_COUNT + k`.
    pub const CIA_BASE: u8 = 7;
    /// Events per CIA.
    pub const CIA_COUNT: u8 = 14;
    /// "CIA Interrupt"
    pub const CIA_INTERRUPT: u8 = 0;
    /// "CIA update ICR"
    pub const CIA_UPDATE_IDR: u8 = 1;
    /// "CIA set IRQ"
    pub const CIA_SET_IRQ: u8 = 2;
    /// "CIA clear IRQ"
    pub const CIA_CLEAR_IRQ: u8 = 3;
    /// "flip CNT"
    pub const CIA_FLIP_CNT: u8 = 4;
    /// "flip fake"
    pub const CIA_FLIP_FAKE: u8 = 5;
    /// "start SDR"
    pub const CIA_START_SDR: u8 = 6;
    /// the serial port's own event ("Serial Port interrupt")
    pub const CIA_SERIAL: u8 = 7;
    /// timer A's own event
    pub const CIA_TIMER_A: u8 = 8;
    /// timer A's "Skip CIA clock decrement cycles"
    pub const CIA_TIMER_A_SKIP: u8 = 9;
    /// timer B's own event
    pub const CIA_TIMER_B: u8 = 10;
    /// timer B's "Skip CIA clock decrement cycles"
    pub const CIA_TIMER_B_SKIP: u8 = 11;
    /// "CIA B counts A"
    pub const CIA_B_TICK: u8 = 12;
    /// the time of day clock's own event
    pub const CIA_TOD: u8 = 13;

    /// Number of events.
    pub const COUNT: usize = (Ev::CIA_BASE + 2 * Ev::CIA_COUNT) as usize;

    /// Event `k` of CIA `n` (0 or 1).
    #[inline]
    pub const fn cia(n: usize, k: u8) -> Ev {
        Ev(Ev::CIA_BASE + (n as u8 & 1) * Ev::CIA_COUNT + k)
    }
}

const NONE: u8 = 0xff;

/// Fast EventScheduler, which maintains a linked list of Events. This scheduler takes
/// negligible time even when it is used to schedule events for nearly every clock.
///
/// Events occur on an internal clock which is 2x the visible clock. The visible clock
/// is divided to two phases called phi1 and phi2.
///
/// The phi1 clocks are used by VIC and CIA chips, phi2 clocks by CPU.
///
/// Scheduling an event for a phi1 clock when system is in phi2 causes the event to be
/// moved to the next phi1 cycle. Correspondingly, requesting a phi1 time when system
/// is in phi2 returns the value of the next phi1.
pub struct EventScheduler {
    /// The first event of the chain
    first: u8,
    /// EventScheduler's current clock
    current_time: EventClock,
    trigger_time: [EventClock; Ev::COUNT],
    next: [u8; Ev::COUNT],
    /// One bit per event in the queue. Not upstream's: there, scheduling an event that
    /// is already pending corrupts the list; here it is cancelled first, so the list
    /// can never become a cycle whatever the emulated program does.
    pending: u64,
}

impl Default for EventScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl EventScheduler {
    pub fn new() -> Self {
        EventScheduler { first: NONE, current_time: 0, trigger_time: [0; Ev::COUNT], next: [NONE; Ev::COUNT], pending: 0 }
    }

    /// Add event to pending queue.
    ///
    /// At PHI2, specify cycles=0 and Phase=PHI1 to fire on the very next PHI1.
    ///
    #[inline]
    pub fn schedule_phase(&mut self, ev: Ev, cycles: u32, phase: Phase) {
        // This strange formulation always selects the next available slot regardless of specified phase
        let t = self.current_time + ((self.current_time & 1) ^ phase as i64) + (cycles.wrapping_shl(1)) as i64;
        self.insert(ev, t);
    }

    /// Add event to pending queue in the same phase as current event.
    #[inline]
    pub fn schedule(&mut self, ev: Ev, cycles: u32) {
        let t = self.current_time + (cycles.wrapping_shl(1)) as i64;
        self.insert(ev, t);
    }

    /// Cancel all pending events and reset time.
    pub fn reset(&mut self) {
        self.first = NONE;
        self.current_time = 0;
        self.next = [NONE; Ev::COUNT];
        self.pending = 0;
    }

    /// Cancel event if pending.
    #[inline]
    pub fn cancel(&mut self, ev: Ev) {
        let e = ev.0;
        if self.pending & (1 << e) == 0 {
            return;
        }
        self.pending &= !(1 << e);
        if self.first == e {
            self.first = self.next[e as usize];
            return;
        }
        let mut scan = self.first;
        while scan != NONE {
            let n = self.next[scan as usize];
            if n == e {
                self.next[scan as usize] = self.next[e as usize];
                break;
            }
            scan = n;
        }
    }

    /// Take the next event off the queue and advance system time to it. The caller
    /// runs the event's code. `None` when no event is pending (upstream would fail).
    #[inline]
    pub fn clock(&mut self) -> Option<Ev> {
        let e = self.first;
        if e == NONE {
            return None;
        }
        self.first = self.next[e as usize];
        self.pending &= !(1 << e);
        self.current_time = self.trigger_time[e as usize];
        Some(Ev(e))
    }

    /// Check if an event is in the queue.
    #[inline]
    pub fn is_pending(&self, ev: Ev) -> bool {
        self.pending & (1 << ev.0) != 0
    }

    /// Get time with respect to a specific clock phase.
    #[inline]
    pub fn get_time(&self, phase: Phase) -> EventClock {
        (self.current_time + (phase as i64 ^ 1)) >> 1
    }

    /// Return current clock phase.
    #[inline]
    pub fn phase(&self) -> Phase {
        if self.current_time & 1 == 0 { Phase::Phi1 } else { Phase::Phi2 }
    }

    /// Time left until the event's trigger time (in scheduler counts: half cycles).
    #[inline]
    pub fn remaining(&self, ev: Ev) -> EventClock {
        self.trigger_time[ev.0 as usize] - self.current_time
    }

    /// Scan the event queue and schedule event for execution.
    #[inline]
    fn insert(&mut self, ev: Ev, t: EventClock) {
        let e = ev.0;
        self.cancel(ev);
        self.pending |= 1 << e;
        self.trigger_time[e as usize] = t;
        // Find the right spot where to tuck this new event
        if self.first == NONE || self.trigger_time[self.first as usize] > t {
            self.next[e as usize] = self.first;
            self.first = e;
            return;
        }
        let mut scan = self.first;
        loop {
            let n = self.next[scan as usize];
            if n == NONE || self.trigger_time[n as usize] > t {
                self.next[e as usize] = n;
                self.next[scan as usize] = e;
                break;
            }
            scan = n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_and_phases() {
        let mut s = EventScheduler::new();
        s.schedule_phase(Ev::VIC, 2, Phase::Phi1);
        s.schedule_phase(Ev::CPU_NOSTEAL, 0, Phase::Phi2);
        s.schedule_phase(Ev::CPU_STEAL, 2, Phase::Phi1); // same time as VIC: after it
        assert!(s.is_pending(Ev::VIC));
        assert_eq!(s.clock(), Some(Ev::CPU_NOSTEAL));
        assert_eq!(s.phase(), Phase::Phi2);
        assert_eq!(s.get_time(Phase::Phi2), 0);
        assert_eq!(s.get_time(Phase::Phi1), 1);
        assert_eq!(s.remaining(Ev::VIC), 3);
        assert_eq!(s.clock(), Some(Ev::VIC));
        assert_eq!(s.clock(), Some(Ev::CPU_STEAL));
        assert_eq!(s.clock(), None);
        s.schedule(Ev::VIC, 1);
        s.schedule(Ev::VIC_BADLINE, 1);
        s.cancel(Ev::VIC);
        assert!(!s.is_pending(Ev::VIC));
        assert_eq!(s.clock(), Some(Ev::VIC_BADLINE));
    }
}
