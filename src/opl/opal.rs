//! The Opal OPL3 emulator, ported from libopenmpt 0.8.7's `soundlib/opal.h`.
//!
//! Provenance, from the header of `opal.h`:
//!
//! > This is the Opal OPL3 emulator from Reality Adlib Tracker v2.0a
//! > (http://www.3eality.com/productions/reality-adlib-tracker).
//! > It was released by Shayde/Reality into the public domain.
//! > Minor modifications to silence some warnings and fix a bug in the envelope
//! > generator have been applied.
//! > Additional fixes by JP Cimalando.
//! >
//! > The Opal OPL3 emulator.
//! >
//! > Note: this is not a complete emulator, just enough for Reality Adlib Tracker tunes.
//! >
//! > Missing features compared to a real OPL3:
//! >
//! > - Timers/interrupts
//! > - OPL3 enable bit (it defaults to always on)
//! > - CSW mode
//! > - Test register
//! > - Percussion mode
//!
//! The port keeps the C++ integer widths and their wrap-arounds (int16 envelope
//! levels, uint16 phases and levels, the attack step's arithmetic shift of a
//! negative product) so that it gives the same samples as `opal.h` compiled with
//! g++ for the same register writes (checked by `tests::matches_opal_h`, whose
//! expected values come from a harness built from `opal.h`). The pointers of the
//! C++ (operator to channel, channel to operators and 4-op pair) are indices.
//! Public domain, like the original.

const OPL3_SAMPLE_RATE: i32 = 49716;
const NUM_CHANNELS: usize = 18;
const NUM_OPERATORS: usize = 36;

const ENV_OFF: i32 = -1;
const ENV_ATT: i32 = 0;
const ENV_DEC: i32 = 1;
const ENV_SUS: i32 = 2;
const ENV_REL: i32 = 3;

const RATE_TABLES: [[u16; 8]; 4] =
    [[1, 0, 1, 0, 1, 0, 1, 0], [1, 0, 1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0, 0, 0], [1, 0, 0, 0, 0, 0, 0, 0]];

#[rustfmt::skip]
const EXP_TABLE: [u16; 256] = [
    1018, 1013, 1007, 1002,  996,  991,  986,  980,  975,  969,  964,  959,  953,  948,  942,  937,
     932,  927,  921,  916,  911,  906,  900,  895,  890,  885,  880,  874,  869,  864,  859,  854,
     849,  844,  839,  834,  829,  824,  819,  814,  809,  804,  799,  794,  789,  784,  779,  774,
     770,  765,  760,  755,  750,  745,  741,  736,  731,  726,  722,  717,  712,  708,  703,  698,
     693,  689,  684,  680,  675,  670,  666,  661,  657,  652,  648,  643,  639,  634,  630,  625,
     621,  616,  612,  607,  603,  599,  594,  590,  585,  581,  577,  572,  568,  564,  560,  555,
     551,  547,  542,  538,  534,  530,  526,  521,  517,  513,  509,  505,  501,  496,  492,  488,
     484,  480,  476,  472,  468,  464,  460,  456,  452,  448,  444,  440,  436,  432,  428,  424,
     420,  416,  412,  409,  405,  401,  397,  393,  389,  385,  382,  378,  374,  370,  367,  363,
     359,  355,  352,  348,  344,  340,  337,  333,  329,  326,  322,  318,  315,  311,  308,  304,
     300,  297,  293,  290,  286,  283,  279,  276,  272,  268,  265,  262,  258,  255,  251,  248,
     244,  241,  237,  234,  231,  227,  224,  220,  217,  214,  210,  207,  204,  200,  197,  194,
     190,  187,  184,  181,  177,  174,  171,  168,  164,  161,  158,  155,  152,  148,  145,  142,
     139,  136,  133,  130,  126,  123,  120,  117,  114,  111,  108,  105,  102,   99,   96,   93,
      90,   87,   84,   81,   78,   75,   72,   69,   66,   63,   60,   57,   54,   51,   48,   45,
      42,   40,   37,   34,   31,   28,   25,   22,   20,   17,   14,   11,    8,    6,    3,    0,
];

#[rustfmt::skip]
const LOG_SIN_TABLE: [u16; 256] = [
    2137, 1731, 1543, 1419, 1326, 1252, 1190, 1137, 1091, 1050, 1013,  979,  949,  920,  894,  869,
     846,  825,  804,  785,  767,  749,  732,  717,  701,  687,  672,  659,  646,  633,  621,  609,
     598,  587,  576,  566,  556,  546,  536,  527,  518,  509,  501,  492,  484,  476,  468,  461,
     453,  446,  439,  432,  425,  418,  411,  405,  399,  392,  386,  380,  375,  369,  363,  358,
     352,  347,  341,  336,  331,  326,  321,  316,  311,  307,  302,  297,  293,  289,  284,  280,
     276,  271,  267,  263,  259,  255,  251,  248,  244,  240,  236,  233,  229,  226,  222,  219,
     215,  212,  209,  205,  202,  199,  196,  193,  190,  187,  184,  181,  178,  175,  172,  169,
     167,  164,  161,  159,  156,  153,  151,  148,  146,  143,  141,  138,  136,  134,  131,  129,
     127,  125,  122,  120,  118,  116,  114,  112,  110,  108,  106,  104,  102,  100,   98,   96,
      94,   92,   91,   89,   87,   85,   83,   82,   80,   78,   77,   75,   74,   72,   70,   69,
      67,   66,   64,   63,   62,   60,   59,   57,   56,   55,   53,   52,   51,   49,   48,   47,
      46,   45,   43,   42,   41,   40,   39,   38,   37,   36,   35,   34,   33,   32,   31,   30,
      29,   28,   27,   26,   25,   24,   23,   23,   22,   21,   20,   20,   19,   18,   17,   17,
      16,   15,   15,   14,   13,   13,   12,   12,   11,   10,   10,    9,    9,    8,    8,    7,
       7,    7,    6,    6,    5,    5,    5,    4,    4,    4,    3,    3,    3,    2,    2,    2,
       2,    1,    1,    1,    1,    1,    1,    1,    0,    0,    0,    0,    0,    0,    0,    0,
];

#[rustfmt::skip]
const LEVTAB: [u8; 128] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 12, 16, 20, 24, 28, 32,
    0, 0, 0, 0, 0, 12, 20, 28, 32, 40, 44, 48, 52, 56, 60, 64,
    0, 0, 0, 20, 32, 44, 52, 60, 64, 72, 76, 80, 84, 88, 92, 96,
    0, 0, 32, 52, 64, 76, 84, 92, 96, 104, 108, 112, 116, 120, 124, 128,
    0, 32, 64, 84, 96, 108, 116, 124, 128, 136, 140, 144, 148, 152, 156, 160,
    0, 64, 96, 116, 128, 140, 148, 156, 160, 168, 172, 176, 180, 184, 188, 192,
    0, 96, 128, 148, 160, 172, 180, 188, 192, 200, 204, 208, 212, 216, 220, 224,
];

/// A single FM operator.
#[derive(Clone, Copy)]
struct Operator {
    chan: usize,
    phase: u32,
    waveform: u16,
    freq_mult_times2: u16,
    envelope_stage: i32,
    envelope_level: i16,
    output_level: u16,
    attack_rate: u16,
    decay_rate: u16,
    sustain_level: u16,
    release_rate: u16,
    attack_shift: u16,
    attack_mask: u16,
    attack_add: u16,
    attack_tab: usize,
    decay_shift: u16,
    decay_mask: u16,
    decay_add: u16,
    decay_tab: usize,
    release_shift: u16,
    release_mask: u16,
    release_add: u16,
    release_tab: usize,
    key_scale_shift: u16,
    key_scale_level: u16,
    out: [i16; 2],
    key_on: bool,
    key_scale_rate: bool,
    sustain_mode: bool,
    tremolo_enable: bool,
    vibrato_enable: bool,
}

impl Default for Operator {
    fn default() -> Self {
        Operator {
            chan: 0,
            phase: 0,
            waveform: 0,
            freq_mult_times2: 1,
            envelope_stage: ENV_OFF,
            envelope_level: 0x1ff,
            output_level: 0,
            attack_rate: 0,
            decay_rate: 0,
            sustain_level: 0,
            release_rate: 0,
            attack_shift: 0,
            attack_mask: 0,
            attack_add: 0,
            attack_tab: 0,
            decay_shift: 0,
            decay_mask: 0,
            decay_add: 0,
            decay_tab: 0,
            release_shift: 0,
            release_mask: 0,
            release_add: 0,
            release_tab: 0,
            key_scale_shift: 0,
            key_scale_level: 0,
            out: [0; 2],
            key_on: false,
            key_scale_rate: false,
            sustain_mode: false,
            tremolo_enable: false,
            vibrato_enable: false,
        }
    }
}

/// A single channel, which can contain two or more operators.
#[derive(Clone, Copy)]
struct Channel {
    op: [Option<usize>; 4],
    freq: u16,
    octave: u16,
    phase_step: u32,
    key_scale_number: u16,
    feedback_shift: u16,
    modulation_type: u16,
    channel_pair: Option<usize>,
    enable: bool,
    left_enable: bool,
    right_enable: bool,
}

impl Default for Channel {
    fn default() -> Self {
        Channel {
            op: [None; 4],
            freq: 0,
            octave: 0,
            phase_step: 0,
            key_scale_number: 0,
            feedback_shift: 0,
            modulation_type: 0,
            channel_pair: None,
            enable: true,
            left_enable: true,
            right_enable: true,
        }
    }
}

/// The Opal class: 18 channels of 2 operators (6 pairs can be 4-op), stereo
/// output resampled from the OPL3's 49716 Hz to the rate given.
#[derive(Clone)]
pub struct Opal {
    sample_rate: i32,
    sample_accum: i32,
    last_output: [i16; 2],
    curr_output: [i16; 2],
    chan: [Channel; NUM_CHANNELS],
    op: [Operator; NUM_OPERATORS],
    clock: u16,
    tremolo_clock: u16,
    tremolo_level: u16,
    vibrato_tick: u16,
    vibrato_clock: u16,
    note_sel: bool,
    tremolo_depth: bool,
    vibrato_depth: bool,
}

/// `Util::muldivr`: (a * b + c / 2) / c in 64 bits, saturated to int32.
fn muldivr(a: i32, b: i32, c: i32) -> i32 {
    ((a as i64 * b as i64 + (c / 2) as i64) / c as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

impl Opal {
    pub fn new(sample_rate: i32) -> Opal {
        let mut o = Opal {
            sample_rate: 0,
            sample_accum: 0,
            last_output: [0; 2],
            curr_output: [0; 2],
            chan: [Channel::default(); NUM_CHANNELS],
            op: [Operator::default(); NUM_OPERATORS],
            clock: 0,
            tremolo_clock: 0,
            tremolo_level: 0,
            vibrato_tick: 0,
            vibrato_clock: 0,
            note_sel: false,
            tremolo_depth: false,
            vibrato_depth: false,
        };
        o.init(sample_rate);
        o
    }

    fn set_operators(&mut self, c: usize, ops: [Option<usize>; 4]) {
        self.chan[c].op = ops;
        for o in ops.into_iter().flatten() {
            self.op[o].chan = c;
        }
    }

    fn init(&mut self, sample_rate: i32) {
        const CHAN_OPS: [usize; NUM_CHANNELS] = [0, 1, 2, 6, 7, 8, 12, 13, 14, 18, 19, 20, 24, 25, 26, 30, 31, 32];
        for (i, &op) in CHAN_OPS.iter().enumerate() {
            if i < 3 || (9..12).contains(&i) {
                self.set_operators(i, [Some(op), Some(op + 3), Some(op + 6), Some(op + 9)]);
            } else {
                self.set_operators(i, [Some(op), Some(op + 3), None, None]);
            }
        }
        for i in 0..NUM_OPERATORS {
            self.op_compute_rates(i);
        }
        self.set_sample_rate(sample_rate);
    }

    pub fn set_sample_rate(&mut self, mut sample_rate: i32) {
        if sample_rate == 0 {
            sample_rate = OPL3_SAMPLE_RATE;
        }
        self.sample_rate = sample_rate;
        self.sample_accum = 0;
        self.last_output = [0; 2];
        self.curr_output = [0; 2];
    }

    /// Write a value to an OPL3 register.
    pub fn port(&mut self, reg_num: u16, val: u8) {
        const OP_LOOKUP: [i8; 32] =
            [0, 1, 2, 3, 4, 5, -1, -1, 6, 7, 8, 9, 10, 11, -1, -1, 12, 13, 14, 15, 16, 17, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1];
        let type_ = reg_num & 0xe0;

        if reg_num == 0xbd {
            self.tremolo_depth = val & 0x80 != 0;
            self.vibrato_depth = val & 0x40 != 0;
            return;
        }

        if type_ == 0x00 {
            if reg_num == 0x104 {
                let mut mask = 1u8;
                for i in 0..6 {
                    let chan = if i < 3 { i } else { i + 6 };
                    let (primary, secondary) = (chan, chan + 3);
                    if val & mask != 0 {
                        self.chan[primary].channel_pair = Some(secondary);
                        self.chan[secondary].enable = false;
                    } else {
                        self.chan[primary].channel_pair = None;
                        self.chan[secondary].enable = true;
                    }
                    mask = mask.wrapping_shl(1);
                }
            } else if reg_num == 0x08 {
                self.note_sel = val & 0x40 != 0;
                for i in 0..NUM_CHANNELS {
                    self.chan_compute_key_scale_number(i);
                }
            }
        } else if (0xa0..=0xc0).contains(&type_) {
            let mut chan_num = (reg_num & 15) as usize;
            if chan_num >= 9 {
                return;
            }
            if reg_num & 0x100 != 0 {
                chan_num += 9;
            }
            let pair = self.chan[chan_num].channel_pair;
            let chans = [Some(chan_num), pair];
            match reg_num & 0xf0 {
                0xa0 => {
                    for c in chans.into_iter().flatten() {
                        self.chan_set_frequency_low(c, val as u16);
                    }
                }
                0xb0 => {
                    for c in chans.into_iter().flatten() {
                        self.chan_set_key_on(c, val & 0x20 != 0);
                        self.chan_set_octave(c, (val >> 2 & 7) as u16);
                        self.chan_set_frequency_high(c, (val & 3) as u16);
                    }
                }
                0xc0 => {
                    let ch = &mut self.chan[chan_num];
                    ch.right_enable = val & 0x20 != 0;
                    ch.left_enable = val & 0x10 != 0;
                    let fb = (val >> 1 & 7) as u16;
                    ch.feedback_shift = if fb != 0 { 9 - fb } else { 0 };
                    ch.modulation_type = (val & 1) as u16;
                }
                _ => {}
            }
        } else if (0x20..=0x80).contains(&type_) || type_ == 0xe0 {
            let op_num = OP_LOOKUP[(reg_num & 0x1f) as usize];
            if op_num < 0 {
                return;
            }
            let mut op_num = op_num as usize;
            if reg_num & 0x100 != 0 {
                op_num += 18;
            }
            match type_ {
                0x20 => {
                    let op = &mut self.op[op_num];
                    op.tremolo_enable = val & 0x80 != 0;
                    op.vibrato_enable = val & 0x40 != 0;
                    op.sustain_mode = val & 0x20 != 0;
                    op.key_scale_rate = val & 0x10 != 0;
                    self.op_compute_rates(op_num);
                    const MUL_TIMES_2: [u16; 16] = [1, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 20, 24, 24, 30, 30];
                    self.op[op_num].freq_mult_times2 = MUL_TIMES_2[(val & 15) as usize];
                }
                0x40 => {
                    const KSL_SHIFT: [u16; 4] = [8, 1, 2, 0];
                    self.op[op_num].key_scale_shift = KSL_SHIFT[(val >> 6) as usize];
                    self.op_compute_key_scale_level(op_num);
                    self.op[op_num].output_level = (val & 0x3f) as u16 * 4;
                }
                0x60 => {
                    self.op[op_num].attack_rate = (val >> 4) as u16;
                    self.op_compute_rates(op_num);
                    self.op[op_num].decay_rate = (val & 15) as u16;
                    self.op_compute_rates(op_num);
                }
                0x80 => {
                    let level = (val >> 4) as u16;
                    self.op[op_num].sustain_level = (if level < 15 { level } else { 31 }) * 16;
                    self.op[op_num].release_rate = (val & 15) as u16;
                    self.op_compute_rates(op_num);
                }
                0xe0 => self.op[op_num].waveform = (val & 7) as u16,
                _ => {}
            }
        }
    }

    /// Generate a stereo sample at the sample rate given.
    pub fn sample(&mut self) -> (i16, i16) {
        while self.sample_accum >= self.sample_rate {
            self.last_output = self.curr_output;
            self.curr_output = self.output();
            self.sample_accum -= self.sample_rate;
        }
        let fract = muldivr(self.sample_accum, 65536, self.sample_rate);
        let mix = |last: i16, curr: i16| -> i16 {
            (last as i32).wrapping_add(fract.wrapping_mul(curr as i32 - last as i32) / 65536) as i16
        };
        let l = mix(self.last_output[0], self.curr_output[0]);
        let r = mix(self.last_output[1], self.curr_output[1]);
        self.sample_accum += OPL3_SAMPLE_RATE;
        (l, r)
    }

    /// One output of the chip, at the OPL3 sample rate.
    fn output(&mut self) -> [i16; 2] {
        let (mut leftmix, mut rightmix) = (0i32, 0i32);
        for i in 0..NUM_CHANNELS {
            let (l, r) = self.chan_output(i);
            leftmix += l as i32;
            rightmix += r as i32;
        }
        let left = leftmix.clamp(-0x8000, 0x7fff) as i16;
        let right = rightmix.clamp(-0x8000, 0x7fff) as i16;

        self.clock = self.clock.wrapping_add(1);

        // Tremolo: a 13440 sample triangle wave, peak 26, trough 0
        self.tremolo_clock = (self.tremolo_clock + 1) % 13440;
        self.tremolo_level =
            (if self.tremolo_clock < 13440 / 2 { self.tremolo_clock } else { 13440 - self.tremolo_clock }) / 256;
        if !self.tremolo_depth {
            self.tremolo_level >>= 2;
        }

        // Vibrato: an 8 step triangle wave, cycled every 1024 samples
        self.vibrato_tick += 1;
        if self.vibrato_tick >= 1024 {
            self.vibrato_tick = 0;
            self.vibrato_clock = (self.vibrato_clock + 1) & 7;
        }
        [left, right]
    }

    fn chan_output(&mut self, c: usize) -> (i16, i16) {
        let ch = self.chan[c];
        if !ch.enable {
            return (0, 0);
        }
        let mut vibrato: i16 = ((ch.freq >> 7) & 7) as i16;
        if !self.vibrato_depth {
            vibrato >>= 1;
        }
        // 0  3  7  3  0  -3  -7  -3
        let clk = self.vibrato_clock;
        if clk & 3 == 0 {
            vibrato = 0;
        } else {
            if clk & 1 != 0 {
                vibrato >>= 1;
            }
            vibrato = ((vibrato as i32) << ch.octave) as i16;
            if clk & 4 != 0 {
                vibrato = vibrato.wrapping_neg();
            }
        }

        let ksn = ch.key_scale_number;
        let ps = ch.phase_step;
        let fb = ch.feedback_shift as i16;
        let o = |i: usize| ch.op[i].unwrap_or(0);
        let out: i16;

        if let Some(pair) = ch.channel_pair {
            if self.chan[pair].modulation_type == 0 {
                if ch.modulation_type == 0 {
                    // feedback -> modulator -> modulator -> modulator -> carrier
                    let mut v = self.op_output(o(0), ksn, ps, vibrato, 0, fb);
                    v = self.op_output(o(1), ksn, ps, vibrato, v, 0);
                    v = self.op_output(o(2), ksn, ps, vibrato, v, 0);
                    out = self.op_output(o(3), ksn, ps, vibrato, v, 0);
                } else {
                    // (feedback -> carrier) + (modulator -> modulator -> carrier)
                    let v = self.op_output(o(0), ksn, ps, vibrato, 0, fb);
                    let mut acc = self.op_output(o(1), ksn, ps, vibrato, 0, 0);
                    acc = self.op_output(o(2), ksn, ps, vibrato, acc, 0);
                    out = v.wrapping_add(self.op_output(o(3), ksn, ps, vibrato, acc, 0));
                }
            } else if ch.modulation_type == 0 {
                // (feedback -> modulator -> carrier) + (modulator -> carrier)
                let mut v = self.op_output(o(0), ksn, ps, vibrato, 0, fb);
                v = self.op_output(o(1), ksn, ps, vibrato, v, 0);
                let acc = self.op_output(o(2), ksn, ps, vibrato, 0, 0);
                out = v.wrapping_add(self.op_output(o(3), ksn, ps, vibrato, acc, 0));
            } else {
                // (feedback -> carrier) + (modulator -> carrier) + carrier
                let mut v = self.op_output(o(0), ksn, ps, vibrato, 0, fb);
                let acc = self.op_output(o(1), ksn, ps, vibrato, 0, 0);
                v = v.wrapping_add(self.op_output(o(2), ksn, ps, vibrato, acc, 0));
                out = v.wrapping_add(self.op_output(o(3), ksn, ps, vibrato, 0, 0));
            }
        } else if ch.modulation_type == 0 {
            // Frequency modulation (well, phase modulation technically)
            let v = self.op_output(o(0), ksn, ps, vibrato, 0, fb);
            out = self.op_output(o(1), ksn, ps, vibrato, v, 0);
        } else {
            // Additive
            let v = self.op_output(o(0), ksn, ps, vibrato, 0, fb);
            out = v.wrapping_add(self.op_output(o(1), ksn, ps, vibrato, 0, 0));
        }

        (if ch.left_enable { out } else { 0 }, if ch.right_enable { out } else { 0 })
    }

    fn chan_set_frequency_low(&mut self, c: usize, freq: u16) {
        let ch = &mut self.chan[c];
        ch.freq = (ch.freq & 0x300) | (freq & 0xff);
        ch.phase_step = (ch.freq as u32) << ch.octave;
    }

    fn chan_set_frequency_high(&mut self, c: usize, freq: u16) {
        let ch = &mut self.chan[c];
        ch.freq = (ch.freq & 0xff) | ((freq & 3) << 8);
        ch.phase_step = (ch.freq as u32) << ch.octave;
        self.chan_compute_key_scale_number(c);
    }

    fn chan_set_octave(&mut self, c: usize, oct: u16) {
        let ch = &mut self.chan[c];
        ch.octave = oct & 7;
        ch.phase_step = (ch.freq as u32) << ch.octave;
        self.chan_compute_key_scale_number(c);
    }

    fn chan_set_key_on(&mut self, c: usize, on: bool) {
        let ops = self.chan[c].op;
        self.op_set_key_on(ops[0].unwrap_or(0), on);
        self.op_set_key_on(ops[1].unwrap_or(0), on);
    }

    fn chan_compute_key_scale_number(&mut self, c: usize) {
        let ch = &mut self.chan[c];
        let lsb = if self.note_sel { ch.freq >> 9 } else { (ch.freq >> 8) & 1 };
        ch.key_scale_number = ch.octave << 1 | lsb;
        let ops = ch.op;
        for o in ops.into_iter().flatten() {
            self.op_compute_rates(o);
            self.op_compute_key_scale_level(o);
        }
    }

    /// `Operator::Output`.
    fn op_output(&mut self, i: usize, _keyscalenum: u16, mut phase_step: u32, vibrato: i16, mut modv: i16, fbshift: i16) -> i16 {
        let clock = self.clock;
        let tremolo_level = self.tremolo_level;
        let op = &mut self.op[i];

        if op.vibrato_enable {
            phase_step = phase_step.wrapping_add(vibrato as i32 as u32);
        }
        op.phase = op.phase.wrapping_add(phase_step.wrapping_mul(op.freq_mult_times2 as u32) / 2);

        let level = ((op.envelope_level as i32
            + op.output_level as i32
            + op.key_scale_level as i32
            + if op.tremolo_enable { tremolo_level as i32 } else { 0 })
            << 3) as u16;

        match op.envelope_stage {
            ENV_ATT => {
                let tab = RATE_TABLES[op.attack_tab][(clock >> op.attack_shift & 7) as usize];
                let mut add = ((((op.attack_add >> tab) as i32) * !(op.envelope_level as i32)) >> 3) as u16;
                if op.attack_rate == 0 {
                    add = 0;
                }
                if op.attack_mask != 0 && clock & op.attack_mask != 0 {
                    add = 0;
                }
                op.envelope_level = (op.envelope_level as i32 + add as i32) as i16;
                if op.envelope_level <= 0 {
                    op.envelope_level = 0;
                    op.envelope_stage = ENV_DEC;
                }
            }
            ENV_DEC => {
                let tab = RATE_TABLES[op.decay_tab][(clock >> op.decay_shift & 7) as usize];
                let mut add = op.decay_add >> tab;
                if op.decay_rate == 0 {
                    add = 0;
                }
                if op.decay_mask != 0 && clock & op.decay_mask != 0 {
                    add = 0;
                }
                op.envelope_level = (op.envelope_level as i32 + add as i32) as i16;
                if op.envelope_level as i32 >= op.sustain_level as i32 {
                    op.envelope_level = op.sustain_level as i16;
                    op.envelope_stage = ENV_SUS;
                }
            }
            ENV_SUS | ENV_REL => {
                if !(op.envelope_stage == ENV_SUS && op.sustain_mode) {
                    let tab = RATE_TABLES[op.release_tab][(clock >> op.release_shift & 7) as usize];
                    let mut add = op.release_add >> tab;
                    if op.release_rate == 0 {
                        add = 0;
                    }
                    if op.release_mask != 0 && clock & op.release_mask != 0 {
                        add = 0;
                    }
                    op.envelope_level = (op.envelope_level as i32 + add as i32) as i16;
                    if op.envelope_level >= 0x1ff {
                        op.envelope_level = 0x1ff;
                        op.envelope_stage = ENV_OFF;
                        op.out = [0; 2];
                        return 0;
                    }
                }
            }
            _ => {
                op.out = [0; 2];
                return 0;
            }
        }

        // Feedback? Modulate by a blend of the last two samples
        if fbshift != 0 {
            modv = (modv as i32 + ((op.out[0] as i32 + op.out[1] as i32) >> fbshift)) as i16;
        }

        let phase = ((op.phase >> 10) as u16 as i32 + modv as i32) as u16;
        let mut offset = phase & 0xff;
        let logsin: u16;
        let mut negate = false;

        match op.waveform {
            0 => {
                if phase & 0x100 != 0 {
                    offset ^= 0xff;
                }
                logsin = LOG_SIN_TABLE[offset as usize];
                negate = phase & 0x200 != 0;
            }
            1 => {
                if phase & 0x200 != 0 {
                    offset = 0;
                } else if phase & 0x100 != 0 {
                    offset ^= 0xff;
                }
                logsin = LOG_SIN_TABLE[offset as usize];
            }
            2 => {
                if phase & 0x100 != 0 {
                    offset ^= 0xff;
                }
                logsin = LOG_SIN_TABLE[offset as usize];
            }
            3 => {
                if phase & 0x100 != 0 {
                    offset = 0;
                }
                logsin = LOG_SIN_TABLE[offset as usize];
            }
            4 => {
                if phase & 0x200 != 0 {
                    offset = 0;
                } else {
                    if phase & 0x80 != 0 {
                        offset ^= 0xff;
                    }
                    offset = (offset + offset) & 0xff;
                    negate = phase & 0x100 != 0;
                }
                logsin = LOG_SIN_TABLE[offset as usize];
            }
            5 => {
                if phase & 0x200 != 0 {
                    offset = 0;
                } else {
                    offset = (offset + offset) & 0xff;
                    if phase & 0x80 != 0 {
                        offset ^= 0xff;
                    }
                }
                logsin = LOG_SIN_TABLE[offset as usize];
            }
            6 => {
                logsin = 0;
                negate = phase & 0x200 != 0;
            }
            _ => {
                let mut l = phase & 0x1ff;
                if phase & 0x200 != 0 {
                    l ^= 0x1ff;
                    negate = true;
                }
                logsin = l << 3;
            }
        }

        let mut mix = logsin.wrapping_add(level);
        if mix > 0x1fff {
            mix = 0x1fff;
        }

        // The exponential's table, read at the 8 LSBs, plus the hidden bit,
        // shifted by the MSBs
        let mut v = ((EXP_TABLE[(mix & 0xff) as usize] as u32 + 1024) >> (mix >> 8)) as i16;
        v = v.wrapping_add(v);
        if negate {
            v = !v;
        }

        op.out[1] = op.out[0];
        op.out[0] = v;
        v
    }

    fn op_set_key_on(&mut self, i: usize, on: bool) {
        let op = &mut self.op[i];
        if op.key_on == on {
            return;
        }
        op.key_on = on;
        if on {
            // The highest attack rate is instant; it bypasses the attack phase
            if op.attack_rate == 15 {
                op.envelope_stage = ENV_DEC;
                op.envelope_level = 0;
            } else {
                op.envelope_stage = ENV_ATT;
            }
            op.phase = 0;
        } else if op.envelope_stage != ENV_OFF && op.envelope_stage != ENV_REL {
            op.envelope_stage = ENV_REL;
        }
    }

    /// `Operator::ComputeRates`: actual rate = rate * 4 + Rof (the key scale
    /// number, all of it with KSR, its top two bits without).
    fn op_compute_rates(&mut self, i: usize) {
        let ksn = self.chan[self.op[i].chan].key_scale_number as i32;
        let op = &mut self.op[i];
        let ks = ksn >> if op.key_scale_rate { 0 } else { 2 };
        let rate = |r: u16| -> (u16, u16, u16, usize) {
            let combined = r as i32 * 4 + ks;
            let (high, low) = (combined >> 2, combined & 3);
            let shift = if high < 12 { (12 - high) as u16 } else { 0 };
            let mask = ((1i32 << shift) - 1) as u16;
            let add = if high < 12 { 1 } else { (1i32 << (high - 12)) as u16 };
            (shift, mask, add, low as usize)
        };
        (op.attack_shift, op.attack_mask, op.attack_add, op.attack_tab) = rate(op.attack_rate);
        if op.attack_rate == 15 {
            op.attack_add = 0xfff;
        }
        (op.decay_shift, op.decay_mask, op.decay_add, op.decay_tab) = rate(op.decay_rate);
        (op.release_shift, op.release_mask, op.release_add, op.release_tab) = rate(op.release_rate);
    }

    fn op_compute_key_scale_level(&mut self, i: usize) {
        let ch = self.chan[self.op[i].chan];
        let idx = ((ch.octave << 4) | (ch.freq >> 6)) as usize;
        let op = &mut self.op[i];
        // (a shift of 8, KSL 0, gives 0: shifted as the int it is promoted to in C++)
        op.key_scale_level = LEVTAB[idx] as u16 >> op.key_scale_shift;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The register script of `matches_opal_h`: a 2-op FM voice keyed on, its
    /// carrier level lowered, a second, additive voice on the other bank, a
    /// key off, at 44.1 kHz.
    const SCRIPT: &[(u16, u8, u32)] = &[
        (0x105, 1, 0),
        (0x104, 0, 0),
        (0xbd, 0xc0, 0),
        (0x20, 0x21, 0),
        (0x23, 0xe1, 0),
        (0x40, 0x8f, 0),
        (0x43, 0x06, 0),
        (0x60, 0xf2, 0),
        (0x63, 0xb4, 0),
        (0x80, 0x55, 0),
        (0x83, 0x37, 0),
        (0xe0, 0x01, 0),
        (0xe3, 0x02, 0),
        (0xc0, 0x3a, 0),
        (0xa0, 0x41, 0),
        (0xb0, 0x32, 3000),
        (0x43, 0x1a, 2000),
        (0x128, 0x42, 0),
        (0x12b, 0x01, 0),
        (0x148, 0x10, 0),
        (0x14b, 0x00, 0),
        (0x168, 0x8c, 0),
        (0x16b, 0x7a, 0),
        (0x188, 0x2f, 0),
        (0x18b, 0x1f, 0),
        (0x1e8, 0x04, 0),
        (0x1eb, 0x07, 0),
        (0x1c3, 0x1d, 0),
        (0x1a3, 0x98, 0),
        (0x1b3, 0x2d, 4000),
        (0xb0, 0x12, 5000),
    ];

    /// The port against `opal.h` compiled with g++: the same samples as the C++
    /// for SCRIPT (and for 1000 random register scripts, checked outside the
    /// repository); here the sums and a few samples the C++ printed.
    #[test]
    fn matches_opal_h() {
        let mut o = Opal::new(44100);
        let mut v: Vec<i64> = Vec::new();
        for &(reg, val, n) in SCRIPT {
            o.port(reg, val);
            for _ in 0..n {
                let (l, r) = o.sample();
                v.push(l as i64);
                v.push(r as i64);
            }
        }
        let sum: i64 = v.iter().sum();
        let sq: i64 = v.iter().map(|x| x * x).sum();
        let at = [1000usize, 6001, 10002, 20003, 27999].map(|i| v[i]);
        // printed by the C++ harness for the same script
        assert_eq!((v.len(), sum, sq, at), (28000, 10004900, 13707267592, [639, 188, 183, 102, 2]));
    }
}
