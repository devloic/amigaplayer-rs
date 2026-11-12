// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/EnvelopeGenerator.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

// A 15 bit [LFSR] is used to implement the envelope rates, in effect dividing
// the clock to the envelope counter by the currently selected rate period.
//
// In addition, another 5 bit counter is used to implement the exponential envelope decay,
// in effect further dividing the clock to the envelope counter.
// The period of this counter is set to 1, 2, 4, 8, 16, 30 at the envelope counter
// values 255, 93, 54, 26, 14, 6, respectively.
//
// [LFSR]: https://en.wikipedia.org/wiki/Linear_feedback_shift_register

/// The envelope state machine's distinct states. In addition to this,
/// envelope has a hold mode, which freezes envelope counter to zero
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Attack,
    DecaySustain,
    Release,
}

// Lookup table to convert from attack, decay, or release value to rate
// counter period.
//
// The rate counter is a 15 bit register which is left shifted each cycle.
// When the counter reaches a specific comparison value,
// the envelope counter is incremented (attack) or decremented
// (decay/release) and the rate counter is resetted.
//
// see [kevtris.org](http://blog.kevtris.org/?p=13)
static ADSR_TABLE: [u16; 16] = [
    0x007f, 0x3000, 0x1e00, 0x0660, 0x0182, 0x5573, 0x000e, 0x3805, 0x2424, 0x2220, 0x090c, 0x0ecd, 0x010e, 0x23f7,
    0x5237, 0x64a8,
];

pub struct EnvelopeGenerator {
    /// XOR shift register for ADSR prescaling
    lfsr: u16,

    /// Comparison value (period) of the rate counter before next event
    rate: u16,

    /// During release mode, the SID approximates envelope decay via piecewise
    /// linear decay rate
    exponential_counter: u32,

    // Comparison value (period) of the exponential decay counter before next decrement
    exponential_counter_period: u32,
    new_exponential_counter_period: u32,

    state_pipeline: u32,

    envelope_pipeline: u32,

    exponential_pipeline: u32,

    // Current envelope state
    state: State,
    next_state: State,

    /// Whether counter is enabled. Only switching to ATTACK can release envelope
    counter_enabled: bool,

    /// Gate bit
    gate: bool,

    reset_lfsr: bool,

    /// The current digital value of envelope output
    envelope_counter: u8,

    /// Attack register
    attack: u8,

    /// Decay register
    decay: u8,

    /// Sustain register
    sustain: u8,

    /// Release register
    release: u8,

    /// The ENV3 value, sampled at the first phase of the clock
    env3: u8,
}

impl EnvelopeGenerator {
    pub fn new() -> Self {
        EnvelopeGenerator {
            lfsr: 0x7fff,
            rate: 0,
            exponential_counter: 0,
            exponential_counter_period: 1,
            new_exponential_counter_period: 0,
            state_pipeline: 0,
            envelope_pipeline: 0,
            exponential_pipeline: 0,
            state: State::Release,
            next_state: State::Release,
            counter_enabled: true,
            gate: false,
            reset_lfsr: false,
            envelope_counter: 0xaa,
            attack: 0,
            decay: 0,
            sustain: 0,
            release: 0,
            env3: 0,
        }
    }

    /// Get the Envelope Generator digital output
    #[inline(always)]
    pub fn output(&self) -> u8 {
        self.envelope_counter
    }

    /// SID reset
    pub fn reset(&mut self) {
        // Counter is not changed on reset
        self.envelope_pipeline = 0;

        self.state_pipeline = 0;

        self.attack = 0;
        self.decay = 0;
        self.sustain = 0;
        self.release = 0;

        self.gate = false;

        self.reset_lfsr = true;

        self.exponential_counter = 0;
        self.exponential_counter_period = 1;
        self.new_exponential_counter_period = 0;

        self.state = State::Release;
        self.counter_enabled = true;
        self.rate = ADSR_TABLE[usize::from(self.release & 0x0f)];
    }

    /// Write control register
    pub fn write_control_reg(&mut self, control: u8) {
        let gate_next = (control & 0x01) != 0;

        if gate_next != self.gate {
            self.gate = gate_next;

            // The rate counter is never reset, thus there will be a delay before the
            // envelope counter starts counting up (attack) or down (release)
            if gate_next {
                // Gate bit on: Start attack, decay, sustain
                self.next_state = State::Attack;
                self.state_pipeline = 2;

                if self.reset_lfsr || self.exponential_pipeline == 2 {
                    self.envelope_pipeline = if self.exponential_counter_period == 1 || self.exponential_pipeline == 2 {
                        2
                    } else {
                        4
                    };
                } else if self.exponential_pipeline == 1 {
                    self.state_pipeline = 3;
                }
            } else {
                // Gate bit off: Start release
                self.next_state = State::Release;
                self.state_pipeline = if self.envelope_pipeline > 0 { 3 } else { 2 };
            }
        }
    }

    /// Write attack/decay register
    pub fn write_attack_decay(&mut self, attack_decay: u8) {
        self.attack = (attack_decay >> 4) & 0x0f;
        self.decay = attack_decay & 0x0f;

        if self.state == State::Attack {
            self.rate = ADSR_TABLE[usize::from(self.attack)];
        } else if self.state == State::DecaySustain {
            self.rate = ADSR_TABLE[usize::from(self.decay)];
        }
    }

    /// Write sustain/release register
    pub fn write_sustain_release(&mut self, sustain_release: u8) {
        // From the sustain levels it follows that both the low and high 4 bits
        // of the envelope counter are compared to the 4-bit sustain value
        // This has been verified by sampling ENV3.
        //
        // For a detailed description see:
        // http://ploguechipsounds.blogspot.it/2010/11/new-research-on-sid-adsr.html
        self.sustain = (sustain_release & 0xf0) | ((sustain_release >> 4) & 0x0f);

        self.release = sustain_release & 0x0f;

        if self.state == State::Release {
            self.rate = ADSR_TABLE[usize::from(self.release)];
        }
    }

    /// Return the envelope current value
    pub fn read_env(&self) -> u8 {
        self.env3
    }

    /// SID clocking
    #[inline(always)]
    pub fn clock(&mut self) {
        self.env3 = self.envelope_counter;

        if self.new_exponential_counter_period > 0 {
            self.exponential_counter_period = self.new_exponential_counter_period;
            self.new_exponential_counter_period = 0;
        }

        if self.state_pipeline != 0 {
            self.state_change();
        }

        if self.envelope_pipeline != 0 && {
            self.envelope_pipeline -= 1;
            self.envelope_pipeline == 0
        } {
            if self.counter_enabled {
                if self.state == State::Attack {
                    self.envelope_counter = self.envelope_counter.wrapping_add(1);
                    if self.envelope_counter == 0xff {
                        self.next_state = State::DecaySustain;
                        self.state_pipeline = 3;
                    }
                } else if self.state == State::DecaySustain || self.state == State::Release {
                    self.envelope_counter = self.envelope_counter.wrapping_sub(1);
                    if self.envelope_counter == 0x00 {
                        self.counter_enabled = false;
                    }
                }

                self.set_exponential_counter();
            }
        } else if self.exponential_pipeline != 0 && {
            self.exponential_pipeline -= 1;
            self.exponential_pipeline == 0
        } {
            self.exponential_counter = 0;

            if (self.state == State::DecaySustain && self.envelope_counter != self.sustain)
                || self.state == State::Release
            {
                // The envelope counter can flip from 0x00 to 0xff by changing state to
                // attack, then to release. The envelope counter will then continue
                // counting down in the release state.
                // This has been verified by sampling ENV3
                self.envelope_pipeline = 1;
            }
        } else if self.reset_lfsr {
            self.lfsr = 0x7fff;
            self.reset_lfsr = false;

            if self.state == State::Attack {
                // The first envelope step in the attack state also resets the exponential
                // counter. This has been verified by sampling ENV3
                self.exponential_counter = 0; // NOTE this is actually delayed one cycle, not modeled

                // The envelope counter can flip from 0xff to 0x00 by changing state to
                // release, then to attack. The envelope counter is then frozen at
                // zero; to unlock this situation the state must be changed to release,
                // then to attack. This has been verified by sampling ENV3
                self.envelope_pipeline = 2;
            } else {
                if self.counter_enabled {
                    self.exponential_counter = self.exponential_counter.wrapping_add(1);
                    if self.exponential_counter == self.exponential_counter_period {
                        self.exponential_pipeline = if self.exponential_counter_period != 1 { 2 } else { 1 };
                    }
                }
            }
        }

        // ADSR delay bug.
        // If the rate counter comparison value is set below the current value of the
        // rate counter, the counter will continue counting up until it wraps around
        // to zero at 2^15 = 0x8000, and then count rate_period - 1 before the
        // envelope can constly be stepped.
        // This has been verified by sampling ENV3.

        // Check to see if LFSR matches table value
        if self.lfsr != self.rate {
            // It wasn't a match, clock the LFSR once
            // by performing XOR on last 2 bits
            let lfsr = u32::from(self.lfsr);
            let feedback = ((lfsr << 14) ^ (lfsr << 13)) & 0x4000;
            self.lfsr = ((lfsr >> 1) | feedback) as u16;
        } else {
            self.reset_lfsr = true;
        }
    }

    // This is what happens on chip during state switching, based on die
    // reverse engineering and transistor level emulation.
    //
    // Attack
    //
    //  0 - Gate on
    //  1 - Counting direction changes
    //      During this cycle the decay rate is "accidentally" activated
    //  2 - Counter is being inverted
    //      Now the attack rate is correctly activated
    //      Counter is enabled
    //  3 - Counter will be counting upward from now on
    //
    // Decay
    //
    //  0 - Counter == $ff
    //  1 - Counting direction changes
    //      The attack state is still active
    //  2 - Counter is being inverted
    //      During this cycle the decay state is activated
    //  3 - Counter will be counting downward from now on
    //
    // Release
    //
    //  0 - Gate off
    //  1 - During this cycle the release state is activated if coming from sustain/decay
    // *2 - Counter is being inverted, the release state is activated
    // *3 - Counter will be counting downward from now on
    //
    //  (* only if coming directly from Attack state)
    //
    // Freeze
    //
    //  0 - Counter == $00
    //  1 - Nothing
    //  2 - Counter is disabled
    #[inline(always)]
    fn state_change(&mut self) {
        self.state_pipeline -= 1;

        match self.next_state {
            State::Attack => {
                if self.state_pipeline == 1 {
                    // The decay rate is "accidentally" enabled during first cycle of attach phase
                    self.rate = ADSR_TABLE[usize::from(self.decay)];
                } else if self.state_pipeline == 0 {
                    self.state = State::Attack;

                    // The attack rate is correctly enabled during second cycle of attach phase
                    self.rate = ADSR_TABLE[usize::from(self.attack)];
                    self.counter_enabled = true;
                }
            }

            State::DecaySustain => {
                if self.state_pipeline == 0 {
                    self.state = State::DecaySustain;
                    self.rate = ADSR_TABLE[usize::from(self.decay)];
                }
            }

            State::Release => {
                if (self.state == State::Attack && self.state_pipeline == 0)
                    || (self.state == State::DecaySustain && self.state_pipeline == 1)
                {
                    self.state = State::Release;
                    self.rate = ADSR_TABLE[usize::from(self.release)];
                }
            }
        }
    }

    #[inline(always)]
    fn set_exponential_counter(&mut self) {
        // Check for change of exponential counter period.
        //
        // For a detailed description see:
        // http://ploguechipsounds.blogspot.it/2010/03/sid-6581r3-adsr-tables-up-close.html
        match self.envelope_counter {
            0xff | 0x00 => self.new_exponential_counter_period = 1,
            0x5d => self.new_exponential_counter_period = 2,
            0x36 => self.new_exponential_counter_period = 4,
            0x1a => self.new_exponential_counter_period = 8,
            0x0e => self.new_exponential_counter_period = 16,
            0x06 => self.new_exponential_counter_period = 30,
            _ => {}
        }
    }
}
