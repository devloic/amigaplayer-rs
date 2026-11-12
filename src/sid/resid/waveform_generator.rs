// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/WaveformGenerator.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::waveform_calculator::{PulldownTables, WaveTable, WaveTables};

// A 24 bit accumulator is the basis for waveform generation.
// FREQ is added to the lower 16 bits of the accumulator each cycle.
// The accumulator is set to zero when TEST is set, and starts counting
// when TEST is cleared.
//
// Waveforms are generated as follows:
//
// - No waveform:
// When no waveform is selected, the DAC input is floating.
//
//
// - Triangle:
// The upper 12 bits of the accumulator are used.
// The MSB is used to create the falling edge of the triangle by inverting
// the lower 11 bits. The MSB is thrown away and the lower 11 bits are
// left-shifted (half the resolution, full amplitude).
// Ring modulation substitutes the MSB with MSB EOR NOT sync_source MSB.
//
//
// - Sawtooth:
// The output is identical to the upper 12 bits of the accumulator.
//
//
// - Pulse:
// The upper 12 bits of the accumulator are used.
// These bits are compared to the pulse width register by a 12 bit digital
// comparator; output is either all one or all zero bits.
// The pulse setting is delayed one cycle after the compare.
// The test bit, when set to one, holds the pulse waveform output at 0xfff
// regardless of the pulse width setting.
//
//
// - Noise:
// The noise output is taken from intermediate bits of a 23-bit shift register
// which is clocked by bit 19 of the accumulator.
// The shift is delayed 2 cycles after bit 19 is set high.
//
// Operation: Calculate EOR result, shift register, set bit 0 = result.
//
//                    reset  +--------------------------------------------+
//                      |    |                                            |
//               test--OR-->EOR<--+                                       |
//                      |         |                                       |
//                      2 2 2 1 1 1 1 1 1 1 1 1 1                         |
//     Register bits:   2 1 0 9 8 7 6 5 4 3 2 1 0 9 8 7 6 5 4 3 2 1 0 <---+
//                          |   |       |     |   |       |     |   |
//     Waveform bits:       1   1       9     8   7       6     5   4
//                          1   0
//
// The low 4 waveform bits are zero (grounded).

// Number of cycles after which the waveform output fades to 0 when setting
// the waveform register to 0.
// Values measured on warm chips (6581R3/R4 and 8580R5)
// checking OSC3.
// Times vary wildly with temperature and may differ
// from chip to chip so the numbers here represent
// only the big difference between the old and new models.
//
// See [VICE Bug #290](http://sourceforge.net/p/vice-emu/bugs/290/)
// and [VICE Bug #1128](http://sourceforge.net/p/vice-emu/bugs/1128/)
// ~95ms
const FLOATING_OUTPUT_TTL_6581R3: u32 = 54000;
const FLOATING_OUTPUT_FADE_6581R3: u32 = 1400;

// ~1s
const FLOATING_OUTPUT_TTL_8580R5: u32 = 800000;
const FLOATING_OUTPUT_FADE_8580R5: u32 = 50000;

// Number of cycles after which the shift register is reset
// when the test bit is set.
// Values measured on warm chips (6581R3/R4 and 8580R5)
// checking OSC3.
// Times vary wildly with temperature and may differ
// from chip to chip so the numbers here represent
// only the big difference between the old and new models
// ~210ms
const SHIFT_REGISTER_RESET_6581R3: u32 = 50000;
const SHIFT_REGISTER_FADE_6581R3: u32 = 15000;
// ~2.8s
const SHIFT_REGISTER_RESET_8580R5: u32 = 986000;
const SHIFT_REGISTER_FADE_8580R5: u32 = 314300;

const SHIFT_MASK: u32 = !(
    (1u32 << 2) |   // Bit 20
    (1u32 << 4) |   // Bit 18
    (1u32 << 8) |   // Bit 14
    (1u32 << 11) |  // Bit 11
    (1u32 << 13) |  // Bit  9
    (1u32 << 17) |  // Bit  5
    (1u32 << 20) |  // Bit  2
    (1u32 << 22)    // Bit  0
);

pub struct WaveformGenerator {
    model_wave: &'static WaveTables,
    model_pulldown: &'static PulldownTables,

    wave: &'static WaveTable,
    pulldown: Option<&'static WaveTable>,

    /// PWout = (PWn/40.95)%
    pw: u32,

    shift_register: u32,

    /// Shift register is latched when transitioning to shift phase 1
    shift_latch: u32,

    /// Emulation of pipeline causing bit 19 to clock the shift register
    shift_pipeline: i32,

    ring_msb_mask: u32,
    no_noise: u32,
    noise_output: u32,
    no_noise_or_noise_output: u32,
    no_pulse: u32,
    pulse_output: u32,

    waveform_output: u32,

    /// Current accumulator value
    pub accumulator: u32,

    /// Fout = (Fn*Fclk/16777216)Hz
    freq: u32,

    /// 8580 tri/saw pipeline
    tri_saw_pipeline: u32,

    /// The OSC3 value
    osc3: u32,

    /// Remaining time to fully reset shift register
    shift_register_reset: u32,

    /// The wave signal TTL when no waveform is selected
    floating_output_ttl: u32,

    /// The control register right-shifted 4 bits; used for output function table lookup
    waveform: u8,

    // The control register bits. Gate is handled by EnvelopeGenerator
    test: bool,
    pub sync: bool,

    /// Test bit is latched at phi2 for the noise XOR
    test_or_reset: bool,

    /// Tell whether the accumulator MSB was set high on this cycle
    pub msb_rising: bool,

    is6581: bool,
}

impl WaveformGenerator {
    pub fn new(model_wave: &'static WaveTables, model_pulldown: &'static PulldownTables, is6581: bool) -> Self {
        WaveformGenerator {
            model_wave,
            model_pulldown,
            wave: &model_wave[0],
            pulldown: None,
            pw: 0,
            shift_register: 0,
            shift_latch: 0,
            shift_pipeline: 0,
            ring_msb_mask: 0,
            no_noise: 0,
            noise_output: 0,
            no_noise_or_noise_output: 0,
            no_pulse: 0,
            pulse_output: 0,
            waveform_output: 0,
            accumulator: 0x555555, // Accumulator's even bits are high on powerup
            freq: 0,
            tri_saw_pipeline: 0x555,
            osc3: 0,
            shift_register_reset: 0,
            floating_output_ttl: 0,
            waveform: 0,
            test: false,
            sync: false,
            test_or_reset: false,
            msb_rising: false,
            is6581,
        }
    }

    pub fn set_waveform_models(&mut self, models: &'static WaveTables) {
        self.model_wave = models;
    }

    pub fn set_pulldown_models(&mut self, models: &'static PulldownTables) {
        self.model_pulldown = models;
    }

    /// Set the chip model.
    /// Must be called before any operation
    pub fn set_model(&mut self, new_is6581: bool) {
        self.is6581 = new_is6581;
    }

    pub fn read_freq(&self) -> u32 {
        self.freq
    }

    pub fn read_test(&self) -> bool {
        self.test
    }

    /// Write FREQ LO register
    pub fn write_freq_lo(&mut self, freq_lo: u8) {
        self.freq = (self.freq & 0xff00) | u32::from(freq_lo);
    }

    /// Write FREQ HI register
    pub fn write_freq_hi(&mut self, freq_hi: u8) {
        self.freq = ((u32::from(freq_hi) << 8) & 0xff00) | (self.freq & 0xff);
    }

    /// Write PW LO register
    pub fn write_pw_lo(&mut self, pw_lo: u8) {
        self.pw = (self.pw & 0xf00) | u32::from(pw_lo);
    }

    /// Write PW HI register
    pub fn write_pw_hi(&mut self, pw_hi: u8) {
        self.pw = ((u32::from(pw_hi) << 8) & 0xf00) | (self.pw & 0x0ff);
    }

    /// Write CONTROL REGISTER register
    pub fn write_control_reg(&mut self, control: u8) {
        let waveform_prev = self.waveform;
        let test_prev = self.test;

        self.waveform = (control >> 4) & 0x0f;
        self.test = (control & 0x08) != 0;
        self.sync = (control & 0x02) != 0;

        // Substitution of accumulator MSB when sawtooth = 0, ring_mod = 1
        let c = i32::from(control);
        self.ring_msb_mask = ((((!c >> 5) & (c >> 2)) & 0x1) as u32) << 23;

        if self.waveform != waveform_prev {
            // Set up waveform tables
            self.set_wave();

            // We assume tha combinations include noise
            // behave the same as without
            self.set_pulldown();

            // No_noise and no_pulse are used in Set_Waveform_Output() as bitmasks to
            // only let the noise or pulse influence the output when the noise or pulse
            // waveforms are selected
            self.no_noise = if (self.waveform & 0x8) != 0 { 0x000 } else { 0xfff };
            self.set_no_noise_or_noise_output();
            self.no_pulse = if (self.waveform & 0x4) != 0 { 0x000 } else { 0xfff };

            if self.waveform == 0 {
                // Change to floating DAC input.
                // Reset fading time for floating DAC input
                self.floating_output_ttl = if self.is6581 {
                    FLOATING_OUTPUT_TTL_6581R3
                } else {
                    FLOATING_OUTPUT_TTL_8580R5
                };
            }
        }

        if self.test != test_prev {
            if self.test {
                // Reset accumulator
                self.accumulator = 0;

                // Flush shift pipeline
                self.shift_pipeline = 0;

                // Latch the shift register value
                self.shift_latch = self.shift_register;

                // Set reset time for shift register
                self.shift_register_reset = if self.is6581 {
                    SHIFT_REGISTER_RESET_6581R3
                } else {
                    SHIFT_REGISTER_RESET_8580R5
                };
            } else {
                // When the test bit is falling, the second phase of the shift is
                // completed by enabling SRAM write
                self.shift_phase2(waveform_prev, self.waveform);
            }
        }
    }

    /// SID reset
    pub fn reset(&mut self) {
        // Accumulator is not changed on reset
        self.freq = 0;
        self.pw = 0;

        self.msb_rising = false;

        self.waveform = 0;
        self.osc3 = 0;

        self.test = false;
        self.sync = false;

        self.wave = &self.model_wave[0];
        self.pulldown = None;

        self.ring_msb_mask = 0;
        self.no_noise = 0xfff;
        self.no_pulse = 0xfff;
        self.pulse_output = 0xfff;

        self.shift_register_reset = 0;
        self.shift_register = 0x7fffff;

        // When reset is released the shift register is clocked once
        // so the lower bit is zeroed out
        // bit0 = (bit22 | test) ^ bit17 = 1 ^ 1 = 0
        self.test_or_reset = true;
        self.shift_latch = self.shift_register;
        self.shift_phase2(0, 0);

        self.shift_pipeline = 0;

        self.waveform_output = 0;
        self.floating_output_ttl = 0;
    }

    /// Read OSC3 value
    pub fn read_osc(&self) -> u8 {
        (self.osc3 >> 4) as u8
    }

    /// SID clocking
    #[inline(always)]
    pub fn clock(&mut self) {
        if self.test {
            if self.shift_register_reset != 0 && {
                self.shift_register_reset -= 1;
                self.shift_register_reset == 0
            } {
                self.shift_reg_bit_fade();
                self.shift_latch = self.shift_register;

                // New noise waveform output
                self.set_noise_output();
            }

            // Latch the test bit value for shift phase 2
            self.test_or_reset = true;

            // The test bit sets pulse high
            self.pulse_output = 0xfff;
        } else {
            // Calculate new accumulator value
            let accumulator_old = self.accumulator;
            self.accumulator = accumulator_old.wrapping_add(self.freq) & 0xffffff;

            // Check which bit have changed from low to high
            let accumulator_bits_set = !accumulator_old & self.accumulator;

            // Check whether the MSB is set high. This is used for synchronization
            self.msb_rising = (accumulator_bits_set & 0x800000) != 0;

            // Shift noise register once for each time accumulator bit 19 is set high.
            // The shift is delayed 2 cycles
            if (accumulator_bits_set & 0x080000) != 0 {
                // Pipeline: Detect rising bit, shift phase 1, shift phase 2
                self.shift_pipeline = 2;
            } else if self.shift_pipeline != 0 {
                self.shift_pipeline -= 1;

                match self.shift_pipeline {
                    0 => self.shift_phase2(self.waveform, self.waveform),

                    1 => {
                        // Start shift phase 1
                        self.test_or_reset = false;
                        self.shift_latch = self.shift_register;
                    }

                    _ => {}
                }
            }
        }
    }

    /// 12-bit waveform output. `prev_accumulator` is the accumulator of the
    /// previous voice (the ring modulation source)
    #[inline(always)]
    pub fn output(&mut self, prev_accumulator: u32) -> u32 {
        // Set output value
        if self.waveform != 0 {
            let ix = (((self.accumulator ^ (!prev_accumulator & self.ring_msb_mask)) >> 12) & 0xfff) as usize;

            // The bit masks no_pulse and no_noise are used to achieve branch-free
            // calculation of the output value
            self.waveform_output = (self.wave[ix] as i32 as u32)
                & (self.no_pulse | self.pulse_output)
                & self.no_noise_or_noise_output;
            if let Some(pulldown) = self.pulldown {
                self.waveform_output = pulldown[(self.waveform_output & 0xfff) as usize] as i32 as u32;
            }

            // Triangle/Sawtooth output is delayed half cycle on 8580.
            // This will appear as a one cycle delay on OSC3 as it is latched
            // in the first phase of the clock
            if (self.waveform & 3) != 0 && !self.is6581 {
                self.osc3 = self.tri_saw_pipeline & (self.no_pulse | self.pulse_output) & self.no_noise_or_noise_output;
                if let Some(pulldown) = self.pulldown {
                    self.osc3 = pulldown[(self.osc3 & 0xfff) as usize] as i32 as u32;
                }

                self.tri_saw_pipeline = self.wave[ix] as i32 as u32;
            } else {
                self.osc3 = self.waveform_output;
            }

            // In the 6581 the top bit of the accumulator may be driven low by combined waveforms
            // when the sawtooth is selected
            if self.is6581 && (self.waveform & 2) != 0 && (self.waveform_output & 0x800) == 0 {
                self.msb_rising = false;
                self.accumulator &= 0x7fffff;
            }

            self.write_shift_register();
        } else {
            // Age floating DAC input
            if self.floating_output_ttl != 0 && {
                self.floating_output_ttl -= 1;
                self.floating_output_ttl == 0
            } {
                self.wave_bit_fade();
            }
        }

        // The pulse level is defined as (accumulator >> 12) >= pw ? 0xfff : 0x000.
        // The expression -((accumulator >> 12) >= pw) & 0xfff yields the same
        // results without any branching (and thus without any pipeline stalls).
        // NB! This expression relies on that the result of a boolean expression
        // is either 0 or 1, and furthermore requires two's complement integer.
        // A few more cycles may be saved by storing the pulse width left shifted
        // 12 bits, and dropping the and with 0xfff (this is valid since pulse is
        // used as a bit mask on 12 bit values), yielding the expression
        // -(accumulator >= pw24). However this only results in negligible savings.

        // The result of the pulse width compare is delayed one cycle.
        // Push next pulse level into pulse level pipeline
        self.pulse_output = if (self.accumulator >> 12) >= self.pw { 0xfff } else { 0x000 };

        self.waveform_output
    }

    /// Synchronize oscillators (see `Sid::voice_sync`): true when the next
    /// voice's accumulator has to be zeroed.
    ///
    /// A special case occurs when a sync source is synced itself on the same
    /// cycle as when its MSB is set high. In this case the destination will
    /// not be synched. This has been verified by sampling OSC3
    #[inline(always)]
    pub fn must_sync_next(&self, next_sync: bool, prev_msb_rising: bool) -> bool {
        self.msb_rising && next_sync && !(self.sync && prev_msb_rising)
    }

    #[inline(always)]
    fn set_wave(&mut self) {
        self.wave = &self.model_wave[usize::from(self.waveform & 3)];
    }

    #[inline(always)]
    fn set_pulldown(&mut self) {
        // We assume tha combinations including noise
        // behave the same as without
        self.pulldown = match self.waveform & 0x7 {
            3 => Some(&self.model_pulldown[0]),
            4 => {
                if (self.waveform & 0x8) != 0 {
                    Some(&self.model_pulldown[4])
                } else {
                    None
                }
            }
            5 => Some(&self.model_pulldown[1]),
            6 => Some(&self.model_pulldown[2]),
            7 => Some(&self.model_pulldown[3]),
            _ => None,
        };
    }

    // This is what happens when the lfsr is clocked:
    //
    // cycle 0: bit 19 of the accumulator goes from low to high, the
    // noise register acts normally, the output may pulldown a bit;
    //
    // cycle 1: first phase of the shift, the bits are interconnected
    // and the output of each bit is latched into the following. The
    // output may overwrite the latched value.
    //
    // cycle 2: second phase of the shift, the latched value becomes
    // active in the first half of the clock and from the second half
    // the register returns to normal operation.
    //
    // When the test or reset lines are active the first phase is
    // executed at every cycle until the signal is released triggering
    // the second phase.
    //
    //      |       |    bit n     |   bit n+1
    //      | bit19 | latch output | latch output
    // -----+-------+--------------+--------------
    // phi1 |   0   |   A <-> A    |   B <-> B
    // phi2 |   0   |   A <-> A    |   B <-> B
    // -----+-------+--------------+--------------
    // phi1 |   1   |   A <-> A    |   B <-> B      <- bit19 raises
    // phi2 |   1   |   A <-> A    |   B <-> B
    // -----+-------+--------------+--------------
    // phi1 |   1   |   X     A  --|-> A     B      <- shift phase 1
    // phi2 |   1   |   X     A  --|-> A     B
    // -----+-------+--------------+--------------
    // phi1 |   1   |   X --> X    |   A --> A      <- shift phase 2
    // phi2 |   1   |   X <-> X    |   A <-> A
    //
    //
    // Normal cycles
    // -------------
    // Normally, when noise is selected along with another waveform,
    // c1 and c2 are closed and the output bits pull down the
    // corresponding shift register bits.
    //
    //        noi_out_x             noi_out_x+1
    //          ^                     ^
    //          |                     |
    //          +-------------+       +-------------+
    //          |             |       |             |
    //          +---o<|---+   |       +---o<|---+   |
    //          |         |   |       |         |   |
    //       c2 |      c1 |   |    c2 |      c1 |   |
    //          |         |   |       |         |   |
    //  >---/---+---|>o---+   +---/---+---|>o---+   +---/--->
    //      LC                    LC                    LC
    //
    //
    // Shift phase 1
    // -------------
    // During shift phase 1 c1 and c2 are open, the SR bits are floating
    // and will be driven by the output of combined waveforms,
    // or slowly turn high.
    //
    //        noi_out_x             noi_out_x+1
    //          ^                     ^
    //          |                     |
    //          +-------------+       +-------------+
    //          |             |       |             |
    //          +---o<|---+   |       +---o<|---+   |
    //          |         |   |       |         |   |
    //       c2 /      c1 /   |    c2 /      c1 /   |
    //          |         |   |       |         |   |
    //  >-------+---|>o---+   +-------+---|>o---+   +------->
    //      LC                    LC                    LC
    //
    //
    // Shift phase 2 (phi1)
    // --------------------
    // During the first half cycle of shift phase 2 c1 is closed
    // so the value from of noi_out_x-1 enters the bit.
    //
    //        noi_out_x             noi_out_x+1
    //          ^                     ^
    //          |                     |
    //          +-------------+       +-------------+
    //          |             |       |             |
    //          +---o<|---+   |       +---o<|---+   |
    //          |         |   |       |         |   |
    //       c2 /      c1 |   |    c2 /      c1 |   |
    //          |         |   |       |         |   |
    //  >---/---+---|>o---+   +---/---+---|>o---+   +---/--->
    //      LC                    LC                    LC
    //
    //
    // Shift phase 2 (phi2)
    // --------------------
    // On the second half of shift phase 2 c2 closes and we're back to
    // normal cycles.
    #[inline(always)]
    fn do_writeback(waveform_old: u8, waveform_new: u8, is6581: bool) -> bool {
        // No writeback without combined waveforms
        if waveform_old <= 8 {
            // Fixes SID/noisewriteback/noise_writeback_test2-{old,new}
            return false;
        }

        if waveform_new < 8 {
            return false;
        }

        if waveform_new == 8
            // Breaks noise_writeback_check_F_to_8_old
            // but fixes simple and scan
            && waveform_old != 0xf
        {
            // Fixes
            // noise_writeback_check_9_to_8_old
            // noise_writeback_check_A_to_8_old
            // noise_writeback_check_B_to_8_old
            // noise_writeback_check_D_to_8_old
            // noise_writeback_check_E_to_8_old
            // noise_writeback_check_F_to_8_old
            // noise_writeback_check_9_to_8_new
            // noise_writeback_check_A_to_8_new
            // noise_writeback_check_D_to_8_new
            // noise_writeback_check_E_to_8_new
            // noise_writeback_test1-{old,new}
            return false;
        }

        // What's happening here?
        if is6581
            && (((waveform_old & 0x3) == 0x1 && (waveform_new & 0x3) == 0x2)
                || ((waveform_old & 0x3) == 0x2 && (waveform_new & 0x3) == 0x1))
        {
            // Fixes
            // noise_writeback_check_9_to_A_old
            // noise_writeback_check_9_to_E_old
            // noise_writeback_check_A_to_9_old
            // noise_writeback_check_A_to_D_old
            // noise_writeback_check_D_to_A_old
            // noise_writeback_check_E_to_9_old
            return false;
        }

        if waveform_old == 0xc {
            // Fixes
            // noise_writeback_check_C_to_A_new
            return false;
        }

        if waveform_new == 0xc {
            // Fixes
            // noise_writeback_check_9_to_C_old
            // noise_writeback_check_A_to_C_old
            return false;
        }

        // Ok, do the writeback
        true
    }

    #[inline(always)]
    fn get_noise_writeback(waveform_output: u32) -> u32 {
        ((waveform_output & (1u32 << 11)) >> 9) |   // Bit 11 -> bit 20
        ((waveform_output & (1u32 << 10)) >> 6) |   // Bit 10 -> bit 18
        ((waveform_output & (1u32 << 9)) >> 1) |    // Bit  9 -> bit 14
        ((waveform_output & (1u32 << 8)) << 3) |    // Bit  8 -> bit 11
        ((waveform_output & (1u32 << 7)) << 6) |    // Bit  7 -> bit  9
        ((waveform_output & (1u32 << 6)) << 11) |   // Bit  6 -> bit  5
        ((waveform_output & (1u32 << 5)) << 15) |   // Bit  5 -> bit  2
        ((waveform_output & (1u32 << 4)) << 18)     // Bit  4 -> bit  0
    }

    /// Perform the actual shifting, moving the latched value into
    /// following bits. The XORing for bit0 is done in this cycle using
    /// the test bit latched during the previous phi2 cycle
    fn shift_phase2(&mut self, waveform_old: u8, waveform_new: u8) {
        if Self::do_writeback(waveform_old, waveform_new, self.is6581) {
            // If noise is combined with another waveform the output drives the SR bits
            self.shift_latch = (self.shift_register & SHIFT_MASK) | Self::get_noise_writeback(self.waveform_output);
        }

        // Bit0 = (bit22 | test | reset) ^ bit17 = 1 ^ bit17 = ~bit17
        let bit22 = (u32::from(self.test_or_reset) | self.shift_latch) << 22;
        let bit0 = (bit22 ^ (self.shift_latch << 17)) & (1 << 22);

        self.shift_register = (self.shift_latch >> 1) | bit0;

        self.set_noise_output();
    }

    fn write_shift_register(&mut self) {
        if self.waveform > 0x8 {
            // Write changes to the shift register output caused by combined waveforms
            // back into the shift register
            if self.shift_pipeline != 1 && !self.test {
                // The output pulls down the SR bits
                self.shift_register &= SHIFT_MASK | Self::get_noise_writeback(self.waveform_output);
                self.noise_output &= self.waveform_output;
            } else {
                // Shift phase 1 the output drives the SR bits
                self.noise_output = self.waveform_output;
            }

            self.set_no_noise_or_noise_output();
        }
    }

    fn set_noise_output(&mut self) {
        self.noise_output = ((self.shift_register & (1u32 << 2)) << 9) |    // Bit 20 -> bit 11
            ((self.shift_register & (1u32 << 4)) << 6) |                     // Bit 18 -> bit 10
            ((self.shift_register & (1u32 << 8)) << 1) |                     // Bit 14 -> bit  9
            ((self.shift_register & (1u32 << 11)) >> 3) |                    // Bit 11 -> bit  8
            ((self.shift_register & (1u32 << 13)) >> 6) |                    // Bit  9 -> bit  7
            ((self.shift_register & (1u32 << 17)) >> 11) |                   // Bit  5 -> bit  6
            ((self.shift_register & (1u32 << 20)) >> 15) |                   // Bit  2 -> bit  5
            ((self.shift_register & (1u32 << 22)) >> 18);                    // Bit  0 -> bit  4

        self.set_no_noise_or_noise_output();
    }

    fn set_no_noise_or_noise_output(&mut self) {
        self.no_noise_or_noise_output = self.no_noise | self.noise_output;
    }

    fn wave_bit_fade(&mut self) {
        self.waveform_output &= self.waveform_output >> 1;
        self.osc3 = self.waveform_output;

        if self.waveform_output != 0 {
            self.floating_output_ttl = if self.is6581 {
                FLOATING_OUTPUT_FADE_6581R3
            } else {
                FLOATING_OUTPUT_FADE_8580R5
            };
        }
    }

    fn shift_reg_bit_fade(&mut self) {
        self.shift_register |= self.shift_register >> 1;
        self.shift_register |= 0x400000;

        if self.shift_register != 0x7fffff {
            self.shift_register_reset = if self.is6581 {
                SHIFT_REGISTER_FADE_6581R3
            } else {
                SHIFT_REGISTER_FADE_8580R5
            };
        }
    }
}
