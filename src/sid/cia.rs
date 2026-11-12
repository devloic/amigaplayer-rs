// Port of NostalgicPlayer's C# port of libsidplayfp (by Simon White, Antti Lankila and
// Leandro Nini): Source/Ports/LibSidPlayFp/C64/Cia/{Mos652x,Timer,TimerA,TimerB,
// InterruptSource,InterruptSource6526,InterruptSource8521,SerialPort,Tod}.cs and
// C64/C64Cia1.cs, C64/C64Cia2.cs, git e183943.
// Licence: GNU General Public License version 2 (LICENSE-LibSidPlayFp); the upstream
// files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more information."
//
// Structure changed for Rust: upstream's CIA is a web of objects (Mos652x owning
// TimerA, TimerB, InterruptSource, Tod and SerialPort, each holding references to the
// parent and the scheduler and calling each other and the parent's virtual methods).
// Here `Mos652x` is a plain data struct holding the sub-objects as plain data structs
// (`Timer`, `InterruptSource`, `Tod`, `SerialPort`), and what were their methods are
// methods of `impl C64` taking the CIA number `n` (0 = CIA1 at $dc00, 1 = CIA2 at
// $dd00) and, for the two timers, a timer selector `t` (0 = A, 1 = B). Upstream's
// events are `Ev` numbers (`Ev::cia(n, k)`) and `cia_event` runs what their callbacks /
// `DoEvent` ran. C64Cia1 / C64Cia2 are branches on `n` (Interrupt, PortB, and the
// `last_ta` bookkeeping of C64Cia1.Poke). InterruptSource6526 / 8521 are a branch on
// `InterruptSource::is8521`; as upstream's SetModel creates a new InterruptSource, here
// it replaces the data with a fresh one (events already scheduled stay on the
// scheduler and now run on the new data; upstream's would run on the discarded old
// object; set the model before the reset to avoid any difference).
// Not upstream's: SerialPort.SyncCntHistory shifts at most 8 times (more shifts give the
// same result as 8: the history byte is then all `cnt`), so no loop is unbounded.
// `cia_write(0, addr, ..)` takes the low byte of the bus address as C64Cia1.Poke does
// (the `last_ta` bookkeeping tests that byte unmasked).

use super::c64::C64;
use super::sched::{Ev, Phase};

/// Chip model
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CiaModel {
    /// Old CIA model, interrupts are delayed by 1 clock
    Mos6526 = 0,
    /// New CIA model
    Mos8521,
    /// A batch of old CIA model with unique serial port behaviour
    Mos6526W4485,
}

const PRA: u8 = 0;
const PRB: u8 = 1;
const DDRA: u8 = 2;
const DDRB: u8 = 3;
const TAL: u8 = 4;
const TAH: u8 = 5;
const TBL: u8 = 6;
const TBH: u8 = 7;
const TOD_TEN: u8 = 8;
const TOD_SEC: u8 = 9;
const TOD_MIN: u8 = 10;
const TOD_HR: u8 = 11;
const SDR: u8 = 12;
const ICR: u8 = 13;
const IDR: u8 = 13;
const CRA: usize = 14;
const CRB: usize = 15;

// Timer.cs
const CIAT_CR_START: i32 = 0x01;
const CIAT_STEP: i32 = 0x04;
const CIAT_CR_ONESHOT: i32 = 0x08;
const CIAT_CR_FLOAD: i32 = 0x10;
const CIAT_PHI2IN: i32 = 0x20;
const CIAT_CR_MASK: i32 = CIAT_CR_START | CIAT_CR_ONESHOT | CIAT_CR_FLOAD | CIAT_PHI2IN;

const CIAT_COUNT2: i32 = 0x100;
const CIAT_COUNT3: i32 = 0x200;

const CIAT_ONESHOT0: i32 = 0x08 << 8;
const CIAT_ONESHOT: i32 = 0x08 << 16;
const CIAT_LOAD1: i32 = 0x10 << 8;
const CIAT_LOAD: i32 = 0x10 << 16;

const CIAT_OUT: i32 = i32::MIN; // 0x80000000

// InterruptSource.cs
const INTERRUPT_NONE: u8 = 0;
const INTERRUPT_UNDERFLOW_A: u8 = 1 << 0;
const INTERRUPT_UNDERFLOW_B: u8 = 1 << 1;
const INTERRUPT_ALARM: u8 = 1 << 2;
#[allow(dead_code)]
const INTERRUPT_SP: u8 = 1 << 3;
#[allow(dead_code)]
const INTERRUPT_FLAG: u8 = 1 << 4;
const INTERRUPT_REQUEST: u8 = 1 << 7;

// Tod.cs
const TENTHS: usize = 0;
const SECONDS: usize = 1;
const MINUTES: usize = 2;
const HOURS: usize = 3;

/// The MOS652x timers (upstream's Timer / TimerA / TimerB)
#[derive(Clone)]
struct Timer {
    /// This is a tri-state:
    ///
    /// - when -1: CIA is completely stopped
    /// - when 0: CIA 1-clock events are ticking
    /// - otherwise: cycle skip event is ticking, and the value is the first
    ///   phi1 clock of skipping
    cia_event_pause_time: i64,
    /// PB6/PB7 Flipflop to signal underflows
    pb_toggle: bool,
    /// Current timer value
    timer: u16,
    /// Timer start value (Latch)
    latch: u16,
    /// Copy of regs[CRA/B]
    last_control_value: u8,
    /// CRA/CRB control register / state
    state: i32,
}

impl Timer {
    fn new() -> Self {
        Timer { cia_event_pause_time: 0, pb_toggle: false, timer: 0, latch: 0, last_control_value: 0, state: 0 }
    }

    /// Get PB6/PB7 flipflop state
    #[inline]
    fn get_pb(&self, reg: u8) -> bool {
        if reg & 0x04 != 0 { self.pb_toggle } else { self.state & CIAT_OUT != 0 }
    }
}

/// The interrupt source (upstream's InterruptSource and its 6526 / 8521 subclasses)
#[derive(Clone)]
struct InterruptSource {
    /// true: InterruptSource8521, false: InterruptSource6526
    is8521: bool,
    // Clock when clear was called last
    last_clear: i64,
    last_set: i64,
    /// Interrupt control register
    icr: u8,
    /// Interrupt data register
    idr: u8,
    idr_temp: u8,
    /// Have we already scheduled CIA->CPU interrupt transition?
    scheduled: bool,
    /// Is the irq pin asserted?
    asserted: bool,
}

impl InterruptSource {
    fn new(is8521: bool) -> Self {
        InterruptSource { is8521, last_clear: 0, last_set: 0, icr: 0, idr: 0, idr_temp: 0, scheduled: false, asserted: false }
    }
}

/// TOD implementation taken from Vice
#[derive(Clone)]
struct Tod {
    cycles: i64,
    period: i64,
    tod_tick_counter: u32,
    is_latched: bool,
    is_stopped: bool,
    clock: [u8; 4],
    latch: [u8; 4],
    alarm: [u8; 4],
}

impl Tod {
    fn new() -> Self {
        Tod {
            cycles: 0,
            period: !0, // Dummy
            tod_tick_counter: 0,
            is_latched: false,
            is_stopped: false,
            clock: [0; 4],
            latch: [0; 4],
            alarm: [0; 4],
        }
    }
}

/// Emulation of the serial port
#[derive(Clone)]
struct SerialPort {
    last_sync: i64,
    count: i32,
    cnt: u8,
    cnt_history: u8,
    loaded: bool,
    pending: bool,
    force_finish: bool,
    model4485: bool,
}

impl SerialPort {
    fn new() -> Self {
        SerialPort { last_sync: 0, count: 0, cnt: 0, cnt_history: 0, loaded: false, pending: false, force_finish: false, model4485: false }
    }
}

/// A MOS 6526 / 8521 CIA (upstream's Mos652x, with C64Cia1 / C64Cia2 chosen by the
/// index of the chip in `C64::cia`).
#[derive(Clone)]
pub struct Mos652x {
    /// These are all CIA registers
    pub regs: [u8; 0x10],
    timer: [Timer; 2],
    intr: InterruptSource,
    tod: Tod,
    serial: SerialPort,
    /// C64Cia1.last_ta
    last_ta: u16,
}

impl Default for Mos652x {
    fn default() -> Self {
        Self::new()
    }
}

impl Mos652x {
    /// Plain data, as upstream's constructor leaves it before `Reset()` (which needs the
    /// scheduler: call `C64::cia_reset(n)` after creating the machine).
    pub fn new() -> Self {
        Mos652x {
            regs: [0; 0x10],
            timer: [Timer::new(), Timer::new()],
            intr: InterruptSource::new(false),
            tod: Tod::new(),
            serial: SerialPort::new(),
            last_ta: 0,
        }
    }
}

impl C64 {
    /// Dispatch event `k` (the `Ev::CIA_*` number) of CIA `n`
    pub fn cia_event(&mut self, n: usize, k: u8) {
        let n = n & 1;
        match k {
            Ev::CIA_INTERRUPT => self.is_interrupt_event(n),
            Ev::CIA_UPDATE_IDR => self.is_update_idr(n),
            Ev::CIA_SET_IRQ => self.is_set_irq(n),
            Ev::CIA_CLEAR_IRQ => self.is_clear_irq(n),
            Ev::CIA_FLIP_CNT => self.sp_flip_cnt(n),
            Ev::CIA_FLIP_FAKE => {} // FlipFake
            Ev::CIA_START_SDR => self.sp_do_start_sdr(n),
            Ev::CIA_SERIAL => self.cia_sp_interrupt(n),
            Ev::CIA_TIMER_A => self.timer_do_event(n, 0),
            Ev::CIA_TIMER_A_SKIP => self.timer_cycle_skipping_event(n, 0),
            Ev::CIA_TIMER_B => self.timer_do_event(n, 1),
            Ev::CIA_TIMER_B_SKIP => self.timer_cycle_skipping_event(n, 1),
            Ev::CIA_B_TICK => self.cia_b_tick(n),
            Ev::CIA_TOD => self.tod_do_event(n),
            _ => {}
        }
    }

    /// Reset CIA (Mos652x.Reset; for CIA1 also C64Cia1.Reset's `last_ta = 0`)
    pub fn cia_reset(&mut self, n: usize) {
        let n = n & 1;
        if n == 0 {
            self.cia[0].last_ta = 0;
        }
        self.cia[n].regs = [0; 0x10];

        self.sp_reset(n);

        // Reset timers
        self.timer_reset(n, 0);
        self.timer_reset(n, 1);

        // Reset interrupt source
        self.is_reset(n);

        // Reset tod
        self.tod_reset(n);

        self.sched.cancel(Ev::cia(n, Ev::CIA_B_TICK));
    }

    /// Select chip model
    pub fn cia_set_model(&mut self, n: usize, m: CiaModel) {
        let n = n & 1;
        match m {
            CiaModel::Mos6526W4485 | CiaModel::Mos6526 => {
                self.cia[n].serial.model4485 = m == CiaModel::Mos6526W4485;
                self.cia[n].intr = InterruptSource::new(false);
            }
            CiaModel::Mos8521 => {
                self.cia[n].serial.model4485 = false;
                self.cia[n].intr = InterruptSource::new(true);
            }
        }
    }

    /// Set day-of-time event occurrence of date
    pub fn cia_set_day_of_time_rate(&mut self, n: usize, clock: u32) {
        self.cia[n & 1].tod.period = (clock as i64).wrapping_mul(1 << 7);
    }

    /// C64Cia1.GetTimerA
    pub fn cia1_get_timer_a(&self) -> u16 {
        self.cia[0].last_ta
    }

    /// Read CIA register (Mos652x.Read)
    pub fn cia_read(&mut self, n: usize, addr: u8) -> u8 {
        let n = n & 1;
        let addr = addr & 0x0f;

        self.timer_sync_with_cpu(n, 0);
        self.timer_wake_up_after_sync_with_cpu(n, 0);
        self.timer_sync_with_cpu(n, 1);
        self.timer_wake_up_after_sync_with_cpu(n, 1);

        match addr {
            // Simulate a serial port
            PRA => {
                let r = &self.cia[n].regs;
                r[PRA as usize] | !r[DDRA as usize]
            }
            PRB => {
                let r = &self.cia[n].regs;
                let v = r[PRB as usize] | !r[DDRB as usize];
                self.adjust_data_port(n, v)
            }
            TAL => self.cia[n].timer[0].timer as u8,
            TAH => (self.cia[n].timer[0].timer >> 8) as u8,
            TBL => self.cia[n].timer[1].timer as u8,
            TBH => (self.cia[n].timer[1].timer >> 8) as u8,
            TOD_TEN | TOD_SEC | TOD_MIN | TOD_HR => self.tod_read(n, addr - TOD_TEN),
            IDR => self.is_clear(n),
            14 => {
                let c = &self.cia[n];
                (c.regs[CRA] & 0xee) | (c.timer[0].state & 1) as u8
            }
            15 => {
                let c = &self.cia[n];
                (c.regs[CRB] & 0xee) | (c.timer[1].state & 1) as u8
            }
            _ => self.cia[n].regs[addr as usize],
        }
    }

    /// Write CIA register (Mos652x.Write; for CIA1 with C64Cia1.Poke's `last_ta` bookkeeping)
    pub fn cia_write(&mut self, n: usize, addr: u8, data: u8) {
        let n = n & 1;
        self.cia_write_inner(n, addr, data);

        if n == 0 {
            // Save the value written to Timer A
            if addr == 0x04 || addr == 0x05 {
                let t = self.cia[0].timer[0].timer;
                if t != 0 {
                    self.cia[0].last_ta = t;
                }
            }
        }
    }

    fn cia_write_inner(&mut self, n: usize, addr: u8, data: u8) {
        let addr = addr & 0x0f;

        self.timer_sync_with_cpu(n, 0);
        self.timer_sync_with_cpu(n, 1);

        let old_data = self.cia[n].regs[addr as usize];
        self.cia[n].regs[addr as usize] = data;

        match addr {
            PRA | DDRA => {
                // PortA(): does nothing in C64Cia1 and C64Cia2
            }
            PRB | DDRB => {
                // PortB(): C64Cia1 only
                if n == 0 {
                    let r = &self.cia[0].regs;
                    let pb = r[PRB as usize] | !r[DDRB as usize];

                    // We should call AdjustDataPort here
                    // but we're only interested in bit 4
                    self.lightpen(pb & 0x10 != 0);
                }
            }
            TAL => self.timer_latch_lo(n, 0, data),
            TAH => self.timer_latch_hi(n, 0, data),
            TBL => self.timer_latch_lo(n, 1, data),
            TBH => self.timer_latch_hi(n, 1, data),
            TOD_TEN | TOD_SEC | TOD_MIN | TOD_HR => self.tod_write(n, addr - TOD_TEN, data),
            SDR => self.sp_start_sdr(n),
            ICR => self.is_set(n, data),
            14 => {
                if (data ^ old_data) & 0x40 != 0 {
                    self.sp_switch_serial_direction(n, data & 0x40 == 0);
                }

                if (data & 1 != 0) && (old_data & 1 == 0) {
                    // Reset the underflow flipflop for the data port
                    self.cia[n].timer[0].pb_toggle = true;
                }

                self.timer_set_control_register(n, 0, data);
            }
            _ => {
                // CRB
                if (data & 1 != 0) && (old_data & 1 == 0) {
                    // Reset the underflow flipflop for the data port
                    self.cia[n].timer[1].pb_toggle = true;
                }

                self.timer_set_control_register(n, 1, data | ((data & 0x40) >> 1));
            }
        }

        self.timer_wake_up_after_sync_with_cpu(n, 0);
        self.timer_wake_up_after_sync_with_cpu(n, 1);
    }

    /// Virtual `Interrupt(state)`: C64Cia1 -> InterruptIrq, C64Cia2 -> InterruptNmi
    #[inline]
    fn cia_interrupt(&mut self, n: usize, state: bool) {
        if n == 0 {
            self.interrupt_irq(state);
        } else if state {
            self.interrupt_nmi();
        }
    }

    /// Handle the serial port
    fn cia_handle_serial_port(&mut self, n: usize) {
        if self.cia[n].regs[CRA] & 0x40 != 0 {
            self.sp_handle(n);
        }
    }

    /// Timer A underflow
    fn cia_underflow_a(&mut self, n: usize) {
        self.is_trigger(n, INTERRUPT_UNDERFLOW_A);

        if self.cia[n].regs[CRB] & 0x41 == 0x41 {
            // timerB.Started()
            if self.cia[n].timer[1].state & CIAT_CR_START != 0 {
                self.sched.schedule_phase(Ev::cia(n, Ev::CIA_B_TICK), 0, Phase::Phi2);
            }
        }
    }

    /// Timer B underflow
    fn cia_underflow_b(&mut self, n: usize) {
        self.is_trigger(n, INTERRUPT_UNDERFLOW_B);
    }

    /// Trigger an interrupt from TOD
    fn cia_tod_interrupt(&mut self, n: usize) {
        self.is_trigger(n, INTERRUPT_ALARM);
    }

    /// Trigger an interrupt from Serial Port
    fn cia_sp_interrupt(&mut self, n: usize) {
        self.is_trigger(n, INTERRUPT_SP);
    }

    /// This event exists solely to break the ambiguity of what
    /// scheduling on top of PHI1 causes, because there is no ordering
    /// between events on same phase. Thus it is scheduled in PHI2 to
    /// ensure the b.event() is run once before the value changes.
    ///
    /// - PHI1 a.event() (which calls underflow())
    /// - PHI1 b.event()
    /// - PHI2 bTick.event()
    /// - PHI1 a.event()
    /// - PHI1 b.event()
    fn cia_b_tick(&mut self, n: usize) {
        // timerB.Cascade()
        // We pretend that we are CPU doing a write to ctrl register
        self.timer_sync_with_cpu(n, 1);
        self.cia[n].timer[1].state |= CIAT_STEP;
        self.timer_wake_up_after_sync_with_cpu(n, 1);
    }

    /// Timers can appear on the port
    fn adjust_data_port(&self, n: usize, mut data: u8) -> u8 {
        let c = &self.cia[n];
        if c.regs[CRA] & 0x02 != 0 {
            data &= 0xbf;

            if c.timer[0].get_pb(c.regs[CRA]) {
                data |= 0x40;
            }
        }

        if c.regs[CRB] & 0x02 != 0 {
            data &= 0x7f;

            if c.timer[1].get_pb(c.regs[CRB]) {
                data |= 0x80;
            }
        }

        data
    }

    // ---------------------------------------------------------------- Timer

    #[inline]
    fn timer_ev(n: usize, t: usize) -> Ev {
        Ev::cia(n, Ev::CIA_TIMER_A + 2 * t as u8)
    }

    #[inline]
    fn timer_skip_ev(n: usize, t: usize) -> Ev {
        Ev::cia(n, Ev::CIA_TIMER_A_SKIP + 2 * t as u8)
    }

    /// Set CRA/CRB control register
    fn timer_set_control_register(&mut self, n: usize, t: usize, cr: u8) {
        let tm = &mut self.cia[n].timer[t];
        tm.state &= !CIAT_CR_MASK;
        tm.state |= (cr as i32 & CIAT_CR_MASK) ^ CIAT_PHI2IN;
        tm.last_control_value = cr;
    }

    /// Perform cycle skipping manually
    ///
    /// Clocks the CIA up to the state it should be in, and stops all
    /// events
    fn timer_sync_with_cpu(&mut self, n: usize, t: usize) {
        if self.cia[n].timer[t].cia_event_pause_time > 0 {
            self.sched.cancel(Self::timer_skip_ev(n, t));
            let elapsed = self.sched.get_time(Phase::Phi2) - self.cia[n].timer[t].cia_event_pause_time;

            // It's possible for CIA to determine that it wants to go to sleep starting from the next
            // cycle, and then save its plans aborted by CPU. Thus, we must avoid modifying
            // the CIA state if the first sleep clock was still in the future
            if elapsed >= 0 {
                let tm = &mut self.cia[n].timer[t];
                tm.timer = tm.timer.wrapping_sub(elapsed as u16);
                self.timer_clock(n, t);
            }
        }

        if self.cia[n].timer[t].cia_event_pause_time == 0 {
            self.sched.cancel(Self::timer_ev(n, t));
        }

        self.cia[n].timer[t].cia_event_pause_time = -1;
    }

    /// Counterpart of SyncWithCpu(), starts the event ticking if it is
    /// needed. No Clock() call or anything such is permissible here!
    fn timer_wake_up_after_sync_with_cpu(&mut self, n: usize, t: usize) {
        self.cia[n].timer[t].cia_event_pause_time = 0;
        self.sched.schedule_phase(Self::timer_ev(n, t), 0, Phase::Phi1);
    }

    /// Reset timer
    fn timer_reset(&mut self, n: usize, t: usize) {
        self.sched.cancel(Self::timer_ev(n, t));
        let tm = &mut self.cia[n].timer[t];
        tm.timer = 0xffff;
        tm.latch = 0xffff;
        tm.pb_toggle = false;
        tm.state = 0;
        tm.last_control_value = 0;
        tm.cia_event_pause_time = 0;
        self.sched.schedule_phase(Self::timer_ev(n, t), 1, Phase::Phi1);
    }

    /// Set low byte of timer start value (Latch)
    fn timer_latch_lo(&mut self, n: usize, t: usize, data: u8) {
        let tm = &mut self.cia[n].timer[t];
        tm.latch = (tm.latch & 0xff00) | data as u16;
        if tm.state & CIAT_LOAD != 0 {
            tm.timer = tm.latch;
        }
    }

    /// Set high byte of timer start value (Latch)
    fn timer_latch_hi(&mut self, n: usize, t: usize, data: u8) {
        let tm = &mut self.cia[n].timer[t];
        tm.latch = (tm.latch & 0x00ff) | ((data as u16) << 8);
        if tm.state & CIAT_LOAD != 0 {
            tm.timer = tm.latch;
        } else if tm.state & CIAT_CR_START == 0 {
            // Reload timer if stopped
            tm.state |= CIAT_LOAD1;
        }
    }

    /// Timer event (PrivateEvent.DoEvent)
    fn timer_do_event(&mut self, n: usize, t: usize) {
        self.timer_clock(n, t);
        self.timer_reschedule(n, t);
    }

    /// Perform scheduled cycle skipping, and resume
    fn timer_cycle_skipping_event(&mut self, n: usize, t: usize) {
        let elapsed = self.sched.get_time(Phase::Phi1) - self.cia[n].timer[t].cia_event_pause_time;
        let tm = &mut self.cia[n].timer[t];
        tm.cia_event_pause_time = 0;
        tm.timer = tm.timer.wrapping_sub(elapsed as u16);
        self.timer_do_event(n, t);
    }

    /// Execute one CIA state transition
    fn timer_clock(&mut self, n: usize, t: usize) {
        let tm = &mut self.cia[n].timer[t];
        if tm.state & CIAT_COUNT3 != 0 {
            tm.timer = tm.timer.wrapping_sub(1);
        }

        // ciatimer.c block start
        let mut adj = tm.state & (CIAT_CR_START | CIAT_CR_ONESHOT | CIAT_PHI2IN);

        if tm.state & (CIAT_CR_START | CIAT_PHI2IN) == (CIAT_CR_START | CIAT_PHI2IN) {
            adj |= CIAT_COUNT2;
        }

        if (tm.state & CIAT_COUNT2 != 0) || (tm.state & (CIAT_STEP | CIAT_CR_START) == (CIAT_STEP | CIAT_CR_START)) {
            adj |= CIAT_COUNT3;
        }

        // CR_FLOAD -> LOAD1, CR_ONESHOT -> ONESHOT0, LOAD1 -> LOAD, ONESHOT0 -> ONESHOT
        adj |= (tm.state & (CIAT_CR_FLOAD | CIAT_CR_ONESHOT | CIAT_LOAD1 | CIAT_ONESHOT0)) << 8;
        tm.state = adj;
        // ciatimer.c block end

        if tm.timer == 0 && tm.state & CIAT_COUNT3 != 0 {
            tm.state |= CIAT_LOAD | CIAT_OUT;

            if tm.state & (CIAT_ONESHOT | CIAT_ONESHOT0) != 0 {
                tm.state &= !(CIAT_CR_START | CIAT_COUNT2);
            }

            // By setting bits 2&3 of the control register,
            // PB6/PB7 will be toggled between high and low at each underflow
            let toogle = tm.last_control_value & 0x06 == 6;
            tm.pb_toggle = toogle && !tm.pb_toggle;

            if t == 0 {
                // Implementation of the serial port (TimerA only)
                self.cia_handle_serial_port(n);

                // Timer A signals underflow handling: IRQ/B-count
                self.cia_underflow_a(n);
            } else {
                self.cia_underflow_b(n);
            }
        }

        let tm = &mut self.cia[n].timer[t];
        if tm.state & CIAT_LOAD != 0 {
            tm.timer = tm.latch;
            tm.state &= !CIAT_COUNT3;
        }
    }

    /// Reschedule CIA event at the earliest interesting time.
    /// If CIA timer is stopped or is programmed to just count down,
    /// the events are paused
    fn timer_reschedule(&mut self, n: usize, t: usize) {
        // There are only two subcases to consider.
        //
        // - are we counting, and if so, are we going to
        //   continue counting?
        // - have we stopped, and are there no conditions to force a new beginning?
        //
        // Additionally, there are numerous flags that are present only in passing manner,
        // but which we need to let cycle through the CIA state machine
        let state = self.cia[n].timer[t].state;
        let timer = self.cia[n].timer[t].timer;
        let unwanted = CIAT_OUT | CIAT_CR_FLOAD | CIAT_LOAD1 | CIAT_LOAD;
        if state & unwanted != 0 {
            self.sched.schedule(Self::timer_ev(n, t), 1);
            return;
        }

        if state & CIAT_COUNT3 != 0 {
            // Test the conditions that keep COUNT2 and thus COUNT3 alive, and also
            // ensure that all of them are set indicating steady state operation
            let wanted = CIAT_CR_START | CIAT_PHI2IN | CIAT_COUNT2 | CIAT_COUNT3;
            if timer > 2 && state & wanted == wanted {
                // We executed this cycle, therefore the pauseTime is +1. If we are called
                // to execute on the very next clock, we need to get 0 because there's
                // another timer-- in it
                self.cia[n].timer[t].cia_event_pause_time = self.sched.get_time(Phase::Phi1) + 1;

                // Execute event slightly before the next underflow
                self.sched.schedule(Self::timer_skip_ev(n, t), timer as u32 - 1);
                return;
            }

            // Play safe, keep on ticking
            self.sched.schedule(Self::timer_ev(n, t), 1);
        } else {
            // Test conditions that result in CIA activity in next clocks.
            // If none, stop
            let unwanted1 = CIAT_CR_START | CIAT_PHI2IN;
            let unwanted2 = CIAT_CR_START | CIAT_STEP;

            if (state & unwanted1 == unwanted1) || (state & unwanted2 == unwanted2) {
                self.sched.schedule(Self::timer_ev(n, t), 1);
                return;
            }

            self.cia[n].timer[t].cia_event_pause_time = -1;
        }
    }

    // ------------------------------------------------------ InterruptSource

    /// Clear pending interrupts, but do not signal to CPU we lost them.
    /// It is assumed that all components get reset() calls in
    /// synchronous manner
    fn is_reset(&mut self, n: usize) {
        let i = &mut self.cia[n].intr;
        i.last_clear = 0;
        i.last_set = 0;

        i.icr = 0;
        i.idr = 0;

        self.sched.cancel(Ev::cia(n, Ev::CIA_UPDATE_IDR));
        self.sched.cancel(Ev::cia(n, Ev::CIA_SET_IRQ));
        self.sched.cancel(Ev::cia(n, Ev::CIA_CLEAR_IRQ));
        self.sched.cancel(Ev::cia(n, Ev::CIA_INTERRUPT));
        let i = &mut self.cia[n].intr;
        i.scheduled = false;

        i.asserted = false;
    }

    /// Set interrupt control mask bits
    fn is_set(&mut self, n: usize, interrupt_mask: u8) {
        let i = &mut self.cia[n].intr;
        if interrupt_mask & INTERRUPT_REQUEST != 0 {
            i.icr |= interrupt_mask & !INTERRUPT_REQUEST;
        } else {
            i.icr &= !interrupt_mask;
        }

        if !self.is_ack0(n) {
            self.is_trigger(n, INTERRUPT_NONE);
        }

        self.cia[n].intr.last_set = self.sched.get_time(Phase::Phi2);
    }

    /// Clear interrupt state (InterruptSource.Clear, and for the 6526 the override's mask)
    fn is_clear(&mut self, n: usize) -> u8 {
        self.cia[n].intr.last_clear = self.sched.get_time(Phase::Phi2);

        self.sched.schedule_phase(Ev::cia(n, Ev::CIA_CLEAR_IRQ), 0, Phase::Phi1);

        if !self.sched.is_pending(Ev::cia(n, Ev::CIA_UPDATE_IDR)) {
            self.sched.schedule_phase(Ev::cia(n, Ev::CIA_UPDATE_IDR), 1, Phase::Phi1);
            self.cia[n].intr.idr_temp = 0;
        }

        let old_idr = self.cia[n].intr.idr;
        if !self.cia[n].intr.is8521 {
            // InterruptSource6526.Clear
            self.cia[n].intr.idr &= INTERRUPT_REQUEST;
        }
        old_idr
    }

    /// Trigger an interrupt (the 6526 / 8521 overrides)
    fn is_trigger(&mut self, n: usize, interrupt_mask: u8) {
        if self.cia[n].intr.is8521 {
            if self.is_is_triggered(n, interrupt_mask) {
                self.is_schedule(n, 0);
            }
        } else {
            if self.is_is_triggered(n, interrupt_mask) {
                // Interrupts are delayed by 1 clk on old CIAs
                self.is_schedule(n, 1);
            }

            // If timer B underflows during the acknowledge cycle
            // it triggers an interrupt as expected
            // but the second bit in icr is not set
            if interrupt_mask == INTERRUPT_UNDERFLOW_B && self.is_ack0(n) {
                let i = &mut self.cia[n].intr;
                i.idr &= !INTERRUPT_UNDERFLOW_B;
                i.idr_temp &= !INTERRUPT_UNDERFLOW_B;
            }
        }
    }

    /// The `TriggerInterrupt` override
    fn is_trigger_interrupt(&mut self, n: usize) {
        if self.cia[n].intr.is8521 {
            let i = &mut self.cia[n].intr;
            i.idr |= INTERRUPT_REQUEST;
            i.idr_temp |= INTERRUPT_REQUEST;

            if self.is_ack0(n) {
                self.is_schedule_irq(n);
            }
        } else {
            self.cia[n].intr.idr |= INTERRUPT_REQUEST;
        }
    }

    /// Check if interrupts were acknowledged during previous cycle
    #[inline]
    fn is_ack0(&self, n: usize) -> bool {
        self.sched.get_time(Phase::Phi2) == self.cia[n].intr.last_clear.wrapping_add(1)
    }

    /// Schedules an IRQ asserting state transition for next cycle
    fn is_schedule(&mut self, n: usize, delay: u32) {
        if !self.cia[n].intr.scheduled {
            self.sched.schedule_phase(Ev::cia(n, Ev::CIA_INTERRUPT), delay, Phase::Phi1);
            self.cia[n].intr.scheduled = true;
        }
    }

    fn is_schedule_irq(&mut self, n: usize) {
        self.sched.schedule_phase(Ev::cia(n, Ev::CIA_SET_IRQ), 1, Phase::Phi1);
    }

    fn is_is_triggered(&mut self, n: usize, interrupt_mask: u8) -> bool {
        let i = &mut self.cia[n].intr;
        i.idr |= interrupt_mask;
        i.idr_temp |= interrupt_mask;

        // InterruptMasked
        if (if interrupt_mask != INTERRUPT_NONE { interrupt_mask } else { i.idr }) & i.icr != 0 {
            return true;
        }

        if interrupt_mask == INTERRUPT_NONE && self.is_write0(n) {
            // Cancel pending interrupts
            if self.cia[n].intr.scheduled {
                self.sched.cancel(Ev::cia(n, Ev::CIA_INTERRUPT));
                self.cia[n].intr.scheduled = false;
            }
        }

        false
    }

    /// Signal interrupt to CPU (the interrupt event)
    fn is_interrupt_event(&mut self, n: usize) {
        if self.cia[n].intr.idr & INTERRUPT_REQUEST == 0 {
            self.is_trigger_interrupt(n);
            self.is_set_irq(n);
        }

        self.cia[n].intr.scheduled = false;
    }

    fn is_update_idr(&mut self, n: usize) {
        self.cia[n].intr.idr = self.cia[n].intr.idr_temp;

        if self.is_ack0(n) {
            self.sched.schedule_phase(Ev::cia(n, Ev::CIA_UPDATE_IDR), 1, Phase::Phi1);
            self.cia[n].intr.idr_temp = 0;
        }
    }

    fn is_set_irq(&mut self, n: usize) {
        if !self.is_ack0(n) && !self.cia[n].intr.asserted {
            self.cia_interrupt(n, true);
            self.cia[n].intr.asserted = true;
        }
    }

    fn is_clear_irq(&mut self, n: usize) {
        if self.cia[n].intr.asserted {
            self.cia_interrupt(n, false);
            self.cia[n].intr.asserted = false;
        }
    }

    /// Check if interrupts were acknowledged during previous cycle
    #[inline]
    fn is_write0(&self, n: usize) -> bool {
        self.sched.get_time(Phase::Phi2) == self.cia[n].intr.last_set.wrapping_add(1)
    }

    // ----------------------------------------------------------- SerialPort

    fn sp_reset(&mut self, n: usize) {
        let now = self.sched.get_time(Phase::Phi1);
        let s = &mut self.cia[n].serial;
        s.count = 0;
        s.cnt_history = 0;
        s.cnt = 1;
        s.loaded = false;
        s.pending = false;
        s.force_finish = false;

        s.last_sync = now;
    }

    fn sp_start_sdr(&mut self, n: usize) {
        self.sched.schedule(Ev::cia(n, Ev::CIA_START_SDR), 1);
    }

    fn sp_switch_serial_direction(&mut self, n: usize, input: bool) {
        self.sp_sync_cnt_history(n);

        if input {
            let cnt_val: u8 = if self.cia[n].serial.model4485 { 0x7 } else { 0x6 };
            self.cia[n].serial.force_finish = (self.cia[n].serial.cnt_history & cnt_val) != cnt_val;

            if !self.cia[n].serial.force_finish
                && self.cia[n].serial.count != 2
                && self.sched.remaining(Ev::cia(n, Ev::CIA_FLIP_CNT)) == 1
            {
                self.cia[n].serial.force_finish = true;
            }
        } else if self.cia[n].serial.force_finish {
            self.sched.cancel(Ev::cia(n, Ev::CIA_SERIAL));
            self.sched.schedule(Ev::cia(n, Ev::CIA_SERIAL), 2);
            self.cia[n].serial.force_finish = false;
        }

        let s = &mut self.cia[n].serial;
        s.cnt = 1;
        s.cnt_history |= 1;

        self.sched.cancel(Ev::cia(n, Ev::CIA_FLIP_CNT));
        self.sched.cancel(Ev::cia(n, Ev::CIA_FLIP_FAKE));

        let s = &mut self.cia[n].serial;
        s.count = 0;
        s.loaded = false;
        s.pending = false;
    }

    fn sp_handle(&mut self, n: usize) {
        let s = &mut self.cia[n].serial;
        if s.loaded && s.count == 0 {
            // Output rate 8 bits at ta / 2
            s.count = 16;
        }

        if s.count == 0 {
            return;
        }

        if self.sched.is_pending(Ev::cia(n, Ev::CIA_FLIP_FAKE)) || self.sched.is_pending(Ev::cia(n, Ev::CIA_FLIP_CNT)) {
            self.sched.cancel(Ev::cia(n, Ev::CIA_FLIP_FAKE));
            self.sched.schedule(Ev::cia(n, Ev::CIA_FLIP_FAKE), 2);
        } else {
            self.sched.schedule(Ev::cia(n, Ev::CIA_FLIP_CNT), 2);
        }
    }

    fn sp_sync_cnt_history(&mut self, n: usize) {
        let time = self.sched.get_time(Phase::Phi1);
        let s = &mut self.cia[n].serial;
        let clocks = time.wrapping_sub(s.last_sync);
        s.last_sync = time;

        // At most 8 shifts matter: the history byte is then all `cnt`
        let mut i = 0i64;
        while i < clocks && i < 8 {
            s.cnt_history = (s.cnt_history << 1) | s.cnt;
            i += 1;
        }
    }

    fn sp_do_start_sdr(&mut self, n: usize) {
        let s = &mut self.cia[n].serial;
        if !s.loaded {
            s.loaded = true;
        } else {
            s.pending = true;
        }
    }

    fn sp_flip_cnt(&mut self, n: usize) {
        if self.cia[n].serial.count == 0 {
            return;
        }

        self.sp_sync_cnt_history(n);

        let s = &mut self.cia[n].serial;
        s.cnt ^= 1;

        s.count -= 1;
        if s.count == 1 {
            self.sched.cancel(Ev::cia(n, Ev::CIA_SERIAL));
            self.sched.schedule(Ev::cia(n, Ev::CIA_SERIAL), 2);

            let s = &mut self.cia[n].serial;
            s.loaded = s.pending;
            s.pending = false;
        }
    }

    // ------------------------------------------------------------------ Tod

    /// Reset TOD
    fn tod_reset(&mut self, n: usize) {
        let t = &mut self.cia[n].tod;
        t.cycles = 0;
        t.tod_tick_counter = 0;

        t.clock = [0; 4];
        t.clock[HOURS] = 1; // The most common value
        t.latch = t.clock;
        t.alarm = [0; 4];

        t.is_latched = false;
        t.is_stopped = true;

        self.sched.schedule_phase(Ev::cia(n, Ev::CIA_TOD), 0, Phase::Phi1);
    }

    /// Read TOD register
    fn tod_read(&mut self, n: usize, reg: u8) -> u8 {
        let t = &mut self.cia[n].tod;
        let reg = reg as usize & 3;
        // TOD clock is latched by reading Hours, and released
        // upon reading Tenths of Seconds. The counter itself
        // keeps ticking all the time.
        // Also note that this latching is different from the input one
        if !t.is_latched {
            t.latch = t.clock;
        }

        if reg == TENTHS {
            t.is_latched = false;
        } else if reg == HOURS {
            t.is_latched = true;
        }

        t.latch[reg]
    }

    /// Write TOD register
    fn tod_write(&mut self, n: usize, reg: u8, mut data: u8) {
        let reg = reg as usize & 3;
        let crb = self.cia[n].regs[CRB];
        let t = &mut self.cia[n].tod;
        match reg {
            // Time of Day clock 1/10 s
            TENTHS => {
                data &= 0x0f;
            }

            // Time of Day clock sec
            // Time of Day clock min
            SECONDS | MINUTES => {
                data &= 0x7f;
            }

            // Time of Day clock hour
            _ => {
                // Force bits 6-5 = 0
                data &= 0x9f;

                // Flip AM/PM on hour 12 on the rising edge of the comparator
                if (data & 0x1f) == 0x12 && (crb & 0x80) == 0 {
                    data ^= 0x80;
                }
            }
        }

        let mut changed = false;
        if crb & 0x80 != 0 {
            // Set alarm
            if t.alarm[reg] != data {
                changed = true;
                t.alarm[reg] = data;
            }
        } else {
            // Set time
            if reg == TENTHS {
                // The tick counter is kept clear while the clock
                // is not running and then restarted by writing to the 10th
                // seconds register
                if t.is_stopped {
                    t.tod_tick_counter = 0;
                    t.is_stopped = false;
                }
            } else if reg == HOURS {
                t.is_stopped = true;
            }

            if t.clock[reg] != data {
                // see https://sourceforge.net/p/vice-emu/bugs/1988/
                // Flip AM/PM on hour 12 on the rising edge of the comparator
                //if ((reg == HOURS) && ((data & 0x1f) == 0x12))
                //	data ^= 0x80;

                changed = true;
                t.clock[reg] = data;
            }
        }

        // Check alarm
        if changed {
            self.tod_check_alarm(n);
        }
    }

    /// The TOD's event
    fn tod_do_event(&mut self, n: usize) {
        let t = &mut self.cia[n].tod;
        t.cycles = t.cycles.wrapping_add(t.period);

        // Fixed precision 25.7
        let c = (t.cycles >> 7) as u32;
        self.sched.schedule(Ev::cia(n, Ev::CIA_TOD), c);
        let cra = self.cia[n].regs[CRA];
        let t = &mut self.cia[n].tod;
        t.cycles &= 0x7f; // Just keep the decimal part

        if !t.is_stopped {
            // The divider which divides the 50 or 60 Hz power supply ticks into
            // 10 Hz uses a 3-bit ring counter, which goes 000, 001, 011, 111, 110,
            // 100.
            // For 50 Hz: matches at 110 (like "4")
            // For 60 Hz: matches at 100 (like "5")
            // (the middle bit of the match value is CRA7)
            // After a match there is a 1 tick delay (until the next power supply
            // tick) and then the 1/10 seconds counter increases, and the ring
            // resets to 000
            //
            // todtickcounter bits are mirrored to save an ANDing
            if t.tod_tick_counter == (0x1 | ((cra as u32 & 0x80) >> 6)) {
                // Reset the counter and update the timer
                t.tod_tick_counter = 0;
                self.tod_update_counters(n);
            } else {
                // Count 50/60 Hz power supply ticks
                t.tod_tick_counter = (t.tod_tick_counter >> 1) | ((!t.tod_tick_counter << 2) & 0x4);
            }
        }
    }

    fn tod_update_counters(&mut self, n: usize) {
        let t = &mut self.cia[n].tod;
        // Advance the counters
        // - individual counters are 4 bit
        //   except for sh and mh which are 3 bits
        let mut ts = t.clock[TENTHS] & 0x0f;
        let mut sl = t.clock[SECONDS] & 0x0f;
        let mut sh = (t.clock[SECONDS] >> 4) & 0x07;
        let mut ml = t.clock[MINUTES] & 0x0f;
        let mut mh = (t.clock[MINUTES] >> 4) & 0x07;
        let mut hl = t.clock[HOURS] & 0x0f;
        let mut hh = (t.clock[HOURS] >> 4) & 0x01;
        let mut pm = t.clock[HOURS] & 0x80;

        // Tenth seconds (0-9)
        ts = (ts + 1) & 0x0f;
        if ts == 10 {
            ts = 0;

            // Seconds (0-59)
            sl = (sl + 1) & 0x0f; // x0...x9
            if sl == 10 {
                sl = 0;
                sh = (sh + 1) & 0x07; // 0x...5x
                if sh == 6 {
                    sh = 0;

                    // Minutes (0-59)
                    ml = (ml + 1) & 0x0f; // x0...x9
                    if ml == 10 {
                        ml = 0;
                        mh = (mh + 1) & 0x07; // 0x...5x
                        if mh == 6 {
                            mh = 0;

                            // Hours (1-12)
                            // Flip from 09:59:59 to 10:00:00
                            // or from 12:59:59 to 01:00:00
                            if (hl == 2 && hh == 1) || (hl == 9 && hh == 0) {
                                hl = hh;
                                hh ^= 1;
                            } else {
                                hl = (hl + 1) & 0x0f;

                                // Toggle the am/pm flag when reaching 12
                                if hl == 2 && hh == 1 {
                                    pm ^= 0x80;
                                }
                            }
                        }
                    }
                }
            }
        }

        t.clock[TENTHS] = ts;
        t.clock[SECONDS] = sl | (sh << 4);
        t.clock[MINUTES] = ml | (mh << 4);
        t.clock[HOURS] = hl | (hh << 4) | pm;

        self.tod_check_alarm(n);
    }

    fn tod_check_alarm(&mut self, n: usize) {
        if self.cia[n].tod.clock == self.cia[n].tod.alarm {
            self.cia_tod_interrupt(n);
        }
    }
}
