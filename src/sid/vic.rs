// Port of NostalgicPlayer's C# port of libsidplayfp (by Simon White, Antti Lankila and
// Leandro Nini): Source/Ports/LibSidPlayFp/C64/Vic_II/Mos656x.cs, Sprites.cs, Lightpen.cs
// and C64/C64Vic.cs, git e183943.
// Licence: GNU General Public License version 2 (LICENSE-LibSidPlayFp); the upstream
// files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more information."
//
// Structure changed for Rust: `Mos656x` is plain data (registers, raster counters,
// `Sprites`, `Lightpen`); everything upstream does through the scheduler or the
// environment is a method of `impl C64`. Upstream's Mos656x is itself an Event
// (`Ev::VIC`, `vic_do_event`) and owns three EventCallbacks (`Ev::VIC_BADLINE`,
// `Ev::VIC_RASTER_Y`, `Ev::VIC_LIGHTPEN`), all dispatched by `vic_event`. The
// delegate selecting the model's clock function is a `fn(&mut C64)` pointer. The
// virtual `Interrupt` / `SetBa` are `interrupt_irq` / `set_ba` of C64. C64Vic's
// IBank Peek/Poke are `vic_read` / `vic_write` (which mask the address with $3f).
// MOS 6567/6569/6572/6573 emulation. Not cycle exact but good enough for SID playback.

use super::c64::C64;
use super::sched::{Ev, EventClock, Phase};

/// The chip model (upstream's `model_t`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VicModel {
    /// OLD NTSC CHIP
    Mos6567R56A = 0,
    /// NTSC-M
    Mos6567R8,
    /// PAL-B
    Mos6569,
    /// PAL-N
    Mos6572,
    /// PAL-M
    Mos6573,
}

/// Cycle # at which the VIC takes the bus in a bad line (BA goes low)
const VICII_FETCH_CYCLE: u32 = 11;

const VICII_SCREEN_TEXTCOLS: u32 = 40;

/// Raster IRQ flag
const IRQ_RASTER: u8 = 1 << 0;

/// Lightpen IRQ flag
const IRQ_LIGHTPEN: u8 = 1 << 3;

/// First line when we can check for bad lines
const FIRST_DMA_LINE: u32 = 0x30;

/// Last line when we check for bad lines
const LAST_DMA_LINE: u32 = 0xf7;

type ClockFunc = fn(&mut C64) -> EventClock;

/// (rasterLines, cyclesPerLine, clock) of a model
fn model_data(m: VicModel) -> (u32, u32, ClockFunc) {
    match m {
        VicModel::Mos6567R56A => (262, 64, C64::vic_clock_old_ntsc), // Old NTSC (MOS6567R56A)
        VicModel::Mos6567R8 => (263, 65, C64::vic_clock_ntsc),       // NTSC-M   (MOS6567R8)
        VicModel::Mos6569 => (312, 63, C64::vic_clock_pal),          // PAL-B    (MOS6569R1, MOS6569R3)
        VicModel::Mos6572 => (312, 65, C64::vic_clock_ntsc),         // PAL-N    (MOS6572)
        VicModel::Mos6573 => (263, 65, C64::vic_clock_ntsc),         // PAL-M    (MOS6573)
    }
}

// ---------------------------------------------------------------------------------------
// Lightpen emulation. Does not reflect model differences
// ---------------------------------------------------------------------------------------

struct Lightpen {
    /// Last VIC raster line
    last_line: u32,
    /// VIC cycles per line
    cycles_per_line: u32,
    /// X coordinate
    lp_x: u32,
    /// Y coordinate
    lp_y: u32,
    /// Has lightpen IRQ been triggered in this frame already?
    is_triggered: bool,
}

impl Lightpen {
    fn new() -> Self {
        Lightpen { last_line: 0, cycles_per_line: 0, lp_x: 0, lp_y: 0, is_triggered: false }
    }

    /// Set VIC screen size
    fn set_screen_size(&mut self, height: u32, width: u32) {
        self.last_line = height.wrapping_sub(1);
        self.cycles_per_line = width;
    }

    fn reset(&mut self) {
        self.lp_x = 0;
        self.lp_y = 0;
        self.is_triggered = false;
    }

    /// Return the low byte of x coordinate
    fn get_x(&self) -> u8 {
        self.lp_x as u8
    }

    /// Return the low byte of y coordinate
    fn get_y(&self) -> u8 {
        self.lp_y as u8
    }

    /// Retrigger lightpen on vertical blank
    fn retrigger(&mut self) -> bool {
        if self.is_triggered {
            return false;
        }
        self.is_triggered = true;
        self.lp_x = match self.cycles_per_line {
            65 => 0xd5,
            _ => 0xd1, // 63 and default
        };
        self.lp_y = 0;
        true
    }

    /// Trigger lightpen from CIA
    fn trigger(&mut self, line_cycle: u32, raster_y: u32) -> bool {
        if self.is_triggered {
            return false;
        }
        self.is_triggered = true;

        // Don't latch on the last line, except on the first cycle
        if raster_y == self.last_line && line_cycle > 0 {
            return false;
        }

        // Latch current coordinates
        self.lp_x = self.get_x_pos(line_cycle) as u32 + 2; // + 1 for MOS 85XX
        self.lp_y = raster_y;

        // On 6569R1 and 6567R56A the interrupt is triggered only
        // when the line is low on the first cycle of the frame
        true // false for old chip revisions
    }

    /// Untrigger lightpen from CIA
    fn untrigger(&mut self) {
        self.is_triggered = false;
    }

    /// Transform line cycle into x coordinate
    fn get_x_pos(&self, mut line_cycle: u32) -> u8 {
        if line_cycle < 13 {
            line_cycle = line_cycle.wrapping_add(self.cycles_per_line);
        }
        line_cycle = line_cycle.wrapping_sub(13);

        // On NTSC the xpos is not incremented at lineCycle 61
        if self.cycles_per_line == 65 && line_cycle > (61 - 13) {
            line_cycle = line_cycle.wrapping_sub(1);
        }
        line_cycle.wrapping_shl(2) as u8
    }
}

// ---------------------------------------------------------------------------------------
// Sprites handling
// ---------------------------------------------------------------------------------------

const SPRITES: usize = 8;
const ENABLE: usize = 0x15;
const Y_EXPANSION: usize = 0x17;

struct Sprites {
    exp_flop: u8,
    dma: u8,
    mc_base: [u8; SPRITES],
    mc: [u8; SPRITES],
}

impl Sprites {
    fn new() -> Self {
        Sprites { exp_flop: 0, dma: 0, mc_base: [0; SPRITES], mc: [0; SPRITES] }
    }

    fn reset(&mut self) {
        self.exp_flop = 0xff;
        self.dma = 0;
        self.mc_base = [0; SPRITES];
        self.mc = [0; SPRITES];
    }

    /// Update mc values in one pass after the DMA has been processed
    fn update_mc(&mut self) {
        let mut mask: u8 = 1;
        for i in 0..SPRITES {
            if self.dma & mask != 0 {
                self.mc[i] = self.mc[i].wrapping_add(3) & 0x3f;
            }
            mask = mask.wrapping_shl(1);
        }
    }

    /// Update mc base value
    fn update_mc_base(&mut self) {
        let mut mask: u8 = 1;
        for i in 0..SPRITES {
            if self.exp_flop & mask != 0 {
                self.mc_base[i] = self.mc[i];
                if self.mc_base[i] == 0x3f {
                    self.dma &= !mask;
                }
            }
            mask = mask.wrapping_shl(1);
        }
    }

    /// Calculate sprite expansion
    fn check_exp(&mut self, regs: &[u8; 0x40]) {
        self.exp_flop ^= self.dma & regs[Y_EXPANSION];
    }

    /// Check if sprite is displayed
    fn check_display(&mut self) {
        self.mc = self.mc_base;
    }

    /// Calculate sprite DMA
    fn check_dma(&mut self, raster_y: u32, regs: &[u8; 0x40]) {
        let y = (raster_y & 0xff) as u8;
        let mut mask: u8 = 1;
        for i in 0..SPRITES {
            if (regs[ENABLE] & mask) != 0 && y == regs[(i << 1) + 1] && (self.dma & mask) == 0 {
                self.dma |= mask;
                self.mc_base[i] = 0;
                self.exp_flop |= mask;
            }
            mask = mask.wrapping_shl(1);
        }
    }

    /// Calculate line crunch
    fn line_crunch(&mut self, data: u8, line_cycle: u32) {
        let mut mask: u8 = 1;
        for i in 0..SPRITES {
            if (data & mask) == 0 && (self.exp_flop & mask) == 0 {
                // Sprite crunch
                if line_cycle == 14 {
                    let mc_i = self.mc[i];
                    let mc_base_i = self.mc_base[i];
                    self.mc[i] = (0x2a & (mc_base_i & mc_i)) | (0x15 & (mc_base_i | mc_i));
                    // mcbase will be set from mc on the following clock call
                }
                self.exp_flop |= mask;
            }
            mask = mask.wrapping_shl(1);
        }
    }

    /// Check if DMA is active for sprites
    #[inline]
    fn is_dma(&self, val: u32) -> bool {
        (self.dma as u32 & val) != 0
    }
}

// ---------------------------------------------------------------------------------------
// Mos656x
// ---------------------------------------------------------------------------------------

pub struct Mos656x {
    /// Current model clock function
    clock: ClockFunc,
    /// Current raster clock
    raster_clk: EventClock,
    /// Number of cycles per line
    cycles_per_line: u32,
    /// Number of raster lines
    max_rasters: u32,
    /// Current visible line
    line_cycle: u32,
    /// Current raster line
    raster_y: u32,
    /// Vertical scrolling value
    y_scroll: u32,
    /// Are bad lines enabled for this frame?
    are_bad_lines_enabled: bool,
    /// Is the current line a bad line
    is_bad_line: bool,
    /// Is rasterYIrq condition true?
    raster_y_irq_condition: bool,
    /// Set when new frame starts
    v_blanking: bool,
    /// Is CIA asserting lightpen?
    lp_asserted: bool,
    /// Internal IRQ flags
    irq_flags: u8,
    /// Masks for the IRQ flags
    irq_mask: u8,
    /// Memory for chip registers
    regs: [u8; 0x40],
    /// Light pen
    lp: Lightpen,
    /// The 8 sprites data
    sprites: Sprites,
}

impl Default for Mos656x {
    fn default() -> Self {
        Self::new()
    }
}

impl Mos656x {
    /// Upstream's constructor runs `Chip(MOS6569)`, which resets and schedules the
    /// event; the scheduler is not reachable here, so this only sets the state: call
    /// `vic_chip` / `vic_reset` on the machine to start the VIC.
    pub fn new() -> Self {
        let (rasters, cycles, clock) = model_data(VicModel::Mos6569);
        let mut lp = Lightpen::new();
        lp.set_screen_size(rasters, cycles);
        let mut sprites = Sprites::new();
        sprites.reset();
        Mos656x {
            clock,
            raster_clk: 0,
            cycles_per_line: cycles,
            max_rasters: rasters,
            line_cycle: 0,
            raster_y: rasters - 1,
            y_scroll: 0,
            are_bad_lines_enabled: false,
            is_bad_line: false,
            raster_y_irq_condition: false,
            v_blanking: false,
            lp_asserted: false,
            irq_flags: 0,
            irq_mask: 0,
            regs: [0; 0x40],
            lp,
            sprites,
        }
    }

    /// Read the value of the raster line IRQ.
    ///
    /// Return the raster line when to trigger an IRQ
    #[inline]
    fn read_raster_line_irq(&self) -> u32 {
        self.regs[0x12] as u32 + (((self.regs[0x11] & 0x80) as u32) << 1)
    }

    /// Read the DEN flag which tells whether the display is enabled
    #[inline]
    fn read_den(&self) -> bool {
        (self.regs[0x11] & 0x10) != 0
    }

    #[inline]
    fn evaluate_is_bad_line(&self) -> bool {
        self.are_bad_lines_enabled
            && self.raster_y >= FIRST_DMA_LINE
            && self.raster_y <= LAST_DMA_LINE
            && (self.raster_y & 7) == self.y_scroll
    }

    /// Get previous value of Y raster
    #[inline]
    fn old_raster_y(&self) -> u32 {
        (if self.raster_y > 0 { self.raster_y } else { self.max_rasters }).wrapping_sub(1)
    }
}

impl C64 {
    /// Dispatcher of the VIC's events.
    pub fn vic_event(&mut self, ev: Ev) {
        match ev {
            Ev::VIC => self.vic_do_event(),
            Ev::VIC_BADLINE => self.vic_bad_line_state_change(),
            Ev::VIC_RASTER_Y => self.vic_raster_y_irq_edge_detector(),
            Ev::VIC_LIGHTPEN => self.vic_lightpen_trigger(),
            _ => {}
        }
    }

    /// The `DoEvent` of upstream's Mos656x: the per-cycle state machine. Always leaves
    /// `Ev::VIC` scheduled.
    fn vic_do_event(&mut self) {
        let phase = self.sched.phase();
        let cycles = self.sched.get_time(phase).wrapping_sub(self.vic.raster_clk);

        let mut delay: EventClock;

        if cycles != 0 {
            // Update x raster
            self.vic.raster_clk = self.vic.raster_clk.wrapping_add(cycles);
            self.vic.line_cycle = self.vic.line_cycle.wrapping_add(cycles as u32);
            self.vic.line_cycle %= self.vic.cycles_per_line.max(1);

            delay = (self.vic.clock)(self);
            if delay < 1 {
                delay = 1;
            }
        } else {
            delay = 1;
        }

        let d = (delay - phase as EventClock).max(0) as u32;
        self.sched.schedule_phase(Ev::VIC, d, Phase::Phi1);
    }

    /// Reset VIC-II
    pub fn vic_reset(&mut self) {
        let v = &mut self.vic;
        v.irq_flags = 0;
        v.irq_mask = 0;
        v.y_scroll = 0;
        v.raster_y = v.max_rasters - 1;
        v.line_cycle = 0;
        v.are_bad_lines_enabled = false;
        v.is_bad_line = false;
        v.raster_y_irq_condition = false;
        v.raster_clk = 0;
        v.v_blanking = false;
        v.lp_asserted = false;

        v.regs = [0; 0x40];

        v.lp.reset();
        v.sprites.reset();

        self.sched.cancel(Ev::VIC);
        self.sched.schedule_phase(Ev::VIC, 0, Phase::Phi1);
    }

    /// Set chip model
    pub fn vic_chip(&mut self, model: VicModel) {
        let (rasters, cycles, clock) = model_data(model);
        self.vic.max_rasters = rasters;
        self.vic.cycles_per_line = cycles;
        self.vic.clock = clock;

        self.vic.lp.set_screen_size(rasters, cycles);

        self.vic_reset();
    }

    /// Read VIC register
    pub fn vic_read(&mut self, addr: u8) -> u8 {
        let addr = addr & 0x3f;

        // Sync up timers
        self.vic_sync();

        let v = &self.vic;
        match addr {
            // Control register 1
            0x11 => (v.regs[addr as usize] & 0x7f) | (((v.raster_y & 0x100) >> 1) as u8),
            // Raster counter
            0x12 => (v.raster_y & 0xff) as u8,
            0x13 => v.lp.get_x(),
            0x14 => v.lp.get_y(),
            // Interrupt pending register
            0x19 => v.irq_flags | 0x70,
            // Interrupt mask register
            0x1a => v.irq_mask | 0xf0,
            _ => {
                // For addresses < $20 read from register directly
                if addr < 0x20 {
                    return v.regs[addr as usize];
                }
                // For addresses < $2f set bits of high nibble to 1
                if addr < 0x2f {
                    return v.regs[addr as usize] | 0xf0;
                }
                // For addresses >= 0x2f return $ff
                0xff
            }
        }
    }

    /// Write to VIC register
    pub fn vic_write(&mut self, addr: u8, data: u8) {
        let addr = addr & 0x3f;

        self.vic.regs[addr as usize] = data;

        // Sync up timers
        self.vic_sync();

        match addr {
            // Control register 1
            0x11 => {
                let old_y_scroll = self.vic.y_scroll;
                self.vic.y_scroll = (data & 7) as u32;

                // This is the funniest part... handle bad line tricks
                let was_bad_lines_enabled = self.vic.are_bad_lines_enabled;

                if self.vic.raster_y == FIRST_DMA_LINE && self.vic.line_cycle == 0 {
                    self.vic.are_bad_lines_enabled = self.vic.read_den();
                }

                if self.vic.old_raster_y() == FIRST_DMA_LINE && self.vic.read_den() {
                    self.vic.are_bad_lines_enabled = true;
                }

                if (old_y_scroll != self.vic.y_scroll || self.vic.are_bad_lines_enabled != was_bad_lines_enabled)
                    && self.vic.raster_y >= FIRST_DMA_LINE
                    && self.vic.raster_y <= LAST_DMA_LINE
                {
                    // Check whether bad line state has changed
                    let was_bad_line = was_bad_lines_enabled && old_y_scroll == (self.vic.raster_y & 7);
                    let now_bad_line = self.vic.are_bad_lines_enabled && self.vic.y_scroll == (self.vic.raster_y & 7);

                    if now_bad_line != was_bad_line {
                        let old_bad_line = self.vic.is_bad_line;

                        if was_bad_line {
                            if self.vic.line_cycle < VICII_FETCH_CYCLE {
                                self.vic.is_bad_line = false;
                            }
                        } else {
                            // Bad line may be generated during fetch interval
                            //   (VICII_FETCH_CYCLE <= lineCycle < VICII_FETCH_CYCLE + VICII_SCREEN_TEXTCOLS + 3)
                            // or outside the fetch interval but before raster yCounter is incremented
                            //   (lineCycle <= VICII_FETCH_CYCLE + VICII_SCREEN_TEXTCOLS + 6)
                            if self.vic.line_cycle <= VICII_FETCH_CYCLE + VICII_SCREEN_TEXTCOLS + 6 {
                                self.vic.is_bad_line = true;
                            }
                        }

                        if self.vic.is_bad_line != old_bad_line {
                            self.sched.schedule_phase(Ev::VIC_BADLINE, 0, Phase::Phi1);
                        }
                    }
                }

                // goto case 0x12
                self.sched.schedule_phase(Ev::VIC_RASTER_Y, 0, Phase::Phi1);
            }

            // Raster counter
            0x12 => {
                // Check raster Y IRQ condition changes at the next PHI1
                self.sched.schedule_phase(Ev::VIC_RASTER_Y, 0, Phase::Phi1);
            }

            0x17 => {
                let lc = self.vic.line_cycle;
                self.vic.sprites.line_crunch(data, lc);
            }

            // VIC interrupt flag register
            0x19 => {
                self.vic.irq_flags &= ((!data & 0x0f) | 0x80) as u8;
                self.vic_handle_irq_state();
            }

            // IRQ mask register
            0x1a => {
                self.vic.irq_mask = data & 0x0f;
                self.vic_handle_irq_state();
            }

            _ => {}
        }
    }

    /// Trigger the lightpen. Sets the lightpen usage flag
    pub fn vic_trigger_lightpen(&mut self) {
        self.vic.lp_asserted = true;

        self.sched.schedule(Ev::VIC_LIGHTPEN, 1);
    }

    /// Clears the lightpen usage flag
    pub fn vic_clear_lightpen(&mut self) {
        self.vic.lp_asserted = false;
    }

    /// Signal CPU interrupt if requested by VIC
    fn vic_handle_irq_state(&mut self) {
        // Signal an IRQ unless we already signaled it
        if (self.vic.irq_flags & self.vic.irq_mask & 0x0f) != 0 {
            if (self.vic.irq_flags & 0x80) == 0 {
                self.interrupt_irq(true);
                self.vic.irq_flags |= 0x80;
            }
        } else if (self.vic.irq_flags & 0x80) != 0 {
            self.interrupt_irq(false);
            self.vic.irq_flags &= 0x7f;
        }
    }

    fn vic_clock_pal(&mut self) -> EventClock {
        let mut delay: EventClock = 1;

        match self.vic.line_cycle {
            0 => {
                self.vic_check_vblank();
                self.vic_end_dma(2);
            }
            1 => {
                self.vic_vblank();
                self.vic_start_dma(5);

                // No sprites before next compulsory cycle
                if !self.vic.sprites.is_dma(0xf8) {
                    delay = 10;
                }
            }
            2 => self.vic_end_dma(3),
            3 => self.vic_start_dma(6),
            4 => self.vic_end_dma(4),
            5 => self.vic_start_dma(7),
            6 => {
                self.vic_end_dma(5);

                delay = if self.vic.sprites.is_dma(0xc0) { 2 } else { 5 };
            }
            7 => {}
            8 => {
                self.vic_end_dma(6);

                delay = 2;
            }
            9 => {}
            10 => self.vic_end_dma7(),
            11 => {
                self.vic_start_bad_line();

                delay = 3;
            }
            12 => delay = 2,
            13 => {}
            14 => self.vic.sprites.update_mc(),
            15 => {
                self.vic.sprites.update_mc_base();

                delay = 39;
            }
            54 => {
                let ry = self.vic.raster_y;
                self.vic.sprites.check_dma(ry, &self.vic.regs);
                self.vic_start_dma0();
            }
            55 => {
                let ry = self.vic.raster_y;
                self.vic.sprites.check_dma(ry, &self.vic.regs); // Phi1
                self.vic.sprites.check_exp(&self.vic.regs); // Phi2
                self.vic_start_dma0();
            }
            56 => self.vic_start_dma(1),
            57 => {
                self.vic.sprites.check_display();

                // No sprites before next compulsory cycle
                if !self.vic.sprites.is_dma(0x1f) {
                    delay = 6;
                }
            }
            58 => self.vic_start_dma(2),
            59 => self.vic_end_dma(0),
            60 => self.vic_start_dma(3),
            61 => self.vic_end_dma(1),
            62 => self.vic_start_dma(4),
            lc => delay = 54 - lc as EventClock,
        }

        delay
    }

    fn vic_clock_ntsc(&mut self) -> EventClock {
        let mut delay: EventClock = 1;

        match self.vic.line_cycle {
            0 => {
                self.vic_check_vblank();
                self.vic_start_dma(5);
            }
            1 => {
                self.vic_vblank();
                self.vic_end_dma(3);

                // No sprites before next compulsory cycle
                if !self.vic.sprites.is_dma(0xf0) {
                    delay = 10;
                }
            }
            2 => self.vic_start_dma(6),
            3 => self.vic_end_dma(4),
            4 => self.vic_start_dma(7),
            5 => {
                self.vic_end_dma(5);

                delay = if self.vic.sprites.is_dma(0xc0) { 2 } else { 6 };
            }
            6 => {}
            7 => {
                self.vic_end_dma(6);

                delay = 2;
            }
            8 => {}
            9 => {
                self.vic_end_dma7();

                delay = 2;
            }
            10 => {}
            11 => {
                self.vic_start_bad_line();

                delay = 3;
            }
            12 => delay = 2,
            13 => {}
            14 => self.vic.sprites.update_mc(),
            15 => {
                self.vic.sprites.update_mc_base();

                delay = 39;
            }
            54 => self.set_ba(true),
            55 => {
                let ry = self.vic.raster_y;
                self.vic.sprites.check_dma(ry, &self.vic.regs); // Phi1
                self.vic.sprites.check_exp(&self.vic.regs); // Phi2
                self.vic_start_dma0();
            }
            56 => {
                let ry = self.vic.raster_y;
                self.vic.sprites.check_dma(ry, &self.vic.regs);
                self.vic_start_dma0();
            }
            57 => self.vic_start_dma(1),
            58 => {
                self.vic.sprites.check_display();

                // No sprites before next compulsory cycle
                if !self.vic.sprites.is_dma(0x1f) {
                    delay = 7;
                }
            }
            59 => self.vic_start_dma(2),
            60 => self.vic_end_dma(0),
            61 => self.vic_start_dma(3),
            62 => self.vic_end_dma(1),
            63 => self.vic_start_dma(4),
            64 => self.vic_end_dma(2),
            lc => delay = 54 - lc as EventClock,
        }

        delay
    }

    fn vic_clock_old_ntsc(&mut self) -> EventClock {
        let mut delay: EventClock = 1;

        match self.vic.line_cycle {
            0 => {
                self.vic_check_vblank();
                self.vic_end_dma(2);
            }
            1 => {
                self.vic_vblank();
                self.vic_start_dma(5);

                // No sprites before next compulsory cycle
                if !self.vic.sprites.is_dma(0xf8) {
                    delay = 10;
                }
            }
            2 => self.vic_end_dma(3),
            3 => self.vic_start_dma(6),
            4 => self.vic_end_dma(4),
            5 => self.vic_start_dma(7),
            6 => {
                self.vic_end_dma(5);

                delay = if self.vic.sprites.is_dma(0xc0) { 2 } else { 5 };
            }
            7 => {}
            8 => {
                self.vic_end_dma(6);

                delay = 2;
            }
            9 => {}
            10 => self.vic_end_dma7(),
            11 => {
                self.vic_start_bad_line();

                delay = 3;
            }
            12 => delay = 2,
            13 => {}
            14 => self.vic.sprites.update_mc(),
            15 => {
                self.vic.sprites.update_mc_base();

                delay = 39;
            }
            54 => self.set_ba(true),
            55 => {
                let ry = self.vic.raster_y;
                self.vic.sprites.check_dma(ry, &self.vic.regs); // Phi1
                self.vic.sprites.check_exp(&self.vic.regs); // Phi2
                self.vic_start_dma0();
            }
            56 => {
                let ry = self.vic.raster_y;
                self.vic.sprites.check_dma(ry, &self.vic.regs);
                self.vic_start_dma0();
            }
            57 => {
                self.vic.sprites.check_display();
                self.vic_start_dma(1);

                // No sprites before next compulsory cycle
                delay = if self.vic.sprites.is_dma(0x1f) { 2 } else { 7 };
            }
            58 => {}
            59 => self.vic_start_dma(2),
            60 => self.vic_end_dma(0),
            61 => self.vic_start_dma(3),
            62 => self.vic_end_dma(1),
            63 => self.vic_start_dma(4),
            lc => delay = 54 - lc as EventClock,
        }

        delay
    }

    /// AEC state was updated
    fn vic_bad_line_state_change(&mut self) {
        let b = !self.vic.is_bad_line;
        self.set_ba(b);
    }

    /// RasterY IRQ edge detector
    fn vic_raster_y_irq_edge_detector(&mut self) {
        let old = self.vic.raster_y_irq_condition;
        self.vic.raster_y_irq_condition = self.vic.raster_y == self.vic.read_raster_line_irq();

        if !old && self.vic.raster_y_irq_condition {
            self.vic_activate_irq_flag(IRQ_RASTER);
        }
    }

    fn vic_lightpen_trigger(&mut self) {
        // Synchronise simulation
        self.vic_sync();

        let (lc, ry) = (self.vic.line_cycle, self.vic.raster_y);
        if self.vic.lp.trigger(lc, ry) {
            self.vic_activate_irq_flag(IRQ_LIGHTPEN);
        }
    }

    /// Set an IRQ flag and trigger an IRQ if the corresponding IRQ mask
    /// is set. The IRQ gets activated, i.e. flag 0x80 gets set, if it
    /// was not active before
    fn vic_activate_irq_flag(&mut self, flag: u8) {
        self.vic.irq_flags |= flag;
        self.vic_handle_irq_state();
    }

    #[inline]
    fn vic_sync(&mut self) {
        self.sched.cancel(Ev::VIC);
        self.vic_do_event();
    }

    /// Check for vertical blanking
    #[inline]
    fn vic_check_vblank(&mut self) {
        // IRQ occurred (xraster != 0)
        if self.vic.raster_y == self.vic.max_rasters.wrapping_sub(1) {
            self.vic.v_blanking = true;
        }

        // Check DEN bit on first cycle of the line following the first DMA line
        if self.vic.raster_y == FIRST_DMA_LINE && !self.vic.are_bad_lines_enabled && self.vic.read_den() {
            self.vic.are_bad_lines_enabled = true;
        }

        // Disallow bad lines after the last possible one has passed
        if self.vic.raster_y == LAST_DMA_LINE {
            self.vic.are_bad_lines_enabled = false;
        }

        self.vic.is_bad_line = false;

        if !self.vic.v_blanking {
            self.vic.raster_y = self.vic.raster_y.wrapping_add(1);
            self.vic_raster_y_irq_edge_detector();

            if self.vic.raster_y == FIRST_DMA_LINE && !self.vic.are_bad_lines_enabled {
                self.vic.are_bad_lines_enabled = self.vic.read_den();
            }
        }

        if self.vic.evaluate_is_bad_line() {
            self.vic.is_bad_line = true;
        }
    }

    /// Vertical blank (line 0)
    #[inline]
    fn vic_vblank(&mut self) {
        if self.vic.v_blanking {
            self.vic.v_blanking = false;
            self.vic.raster_y = 0;
            self.vic_raster_y_irq_edge_detector();
            self.vic.lp.untrigger();

            if self.vic.lp_asserted && self.vic.lp.retrigger() {
                self.vic_activate_irq_flag(IRQ_LIGHTPEN);
            }
        }
    }

    /// Start DMA for sprite n
    #[inline]
    fn vic_start_dma(&mut self, n: u32) {
        if self.vic.sprites.is_dma(0x01 << n) {
            self.set_ba(false);
        }
    }

    /// Start DMA for sprite 0
    #[inline]
    fn vic_start_dma0(&mut self) {
        let b = !self.vic.sprites.is_dma(0x01);
        self.set_ba(b);
    }

    /// End DMA for sprite n
    #[inline]
    fn vic_end_dma(&mut self, n: u32) {
        if !self.vic.sprites.is_dma(0x06 << n) {
            self.set_ba(true);
        }
    }

    /// End DMA for sprite 7
    #[inline]
    fn vic_end_dma7(&mut self) {
        self.set_ba(true);
    }

    /// Start bad line
    #[inline]
    fn vic_start_bad_line(&mut self) {
        if self.vic.is_bad_line {
            self.set_ba(false);
        }
    }
}
