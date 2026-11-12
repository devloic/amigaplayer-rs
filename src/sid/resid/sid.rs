// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/Sid.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::dac::Dac;
use super::external_filter::ExternalFilter;
use super::filter::Filter;
use super::resample::{Resampler, TwoPassSincResampler, ZeroOrderResampler};
use super::voice::Voice;
use super::waveform_calculator::{build_pulldown_table, get_wave_table};
use super::{ChipModel, CombinedWaveforms, SamplingMethod};

// MOS6581/MOS8580 emulation

// The waveform D/A converter introduces a DC offset in the signal
// to the envelope multiplying D/A converter. The "zero" level of
// the waveform D/A converter can be found as follows:
//
// Measure the "zero" voltage of voice 3 on the SID audio output
// pin, routing only voice 3 to the mixer ($d417 = $0b, $d418 =
// $0f, all other registers zeroed).
//
// Then set the sustain level for voice 3 to maximum and search for
// the waveform output value yielding the same voltage as found
// above. This is done by trying out different waveform output
// values until the correct value is found, e.g. with the following
// program:
//
//       lda #$08
//       sta $d412
//       lda #$0b
//       sta $d417
//       lda #$0f
//       sta $d418
//       lda #$f0
//       sta $d414
//       lda #$21
//       sta $d412
//       lda #$01
//       sta $d40e
//
//       ldx #$00
//       lda #$38        ; Tweak this to find the "zero" level
// l     cmp $d41b
//       bne l
//       stx $d40e       ; Stop frequency counter - freeze waveform output
//       brk
//
// The waveform output range is 0x000 to 0xfff, so the "zero"
// level should ideally have been 0x800. In the measured chip, the
// waveform output "zero" level was found to be 0x380 (i.e. $d41b
// = 0x38) at an audio output voltage of 5.94V.
//
// With knowledge of the mixer op-amp characteristics, further estimates
// of waveform voltages can be obtained by sampling the EXT IN pin.
// From EXT IN samples, the corresponding waveform output can be found by
// using the model for the mixer.
//
// Such measurements have been done on a chip marked MOS 6581R4AR
// 0687 14, and the following results have been obtained:
// * The full range of one voice is approximately 1.5V.
// * The "zero" level rides at approximately 5.0V.
//
//
// zero-x did the measuring on the 8580 (https://sourceforge.net/p/vice-emu/bugs/1036/#c5b3):
// When it sits on basic from powerup it's at 4.72
// Run 1.prg and check the output pin level.
// Then run 2.prg and adjust it until the output level is the same...
// 0x94-0xA8 gives me the same 4.72 1.prg shows.
// On another 8580 it's 0x90-0x9C
// Third chip 0x94-0xA8
// Fourth chip 0x90-0xA4
// On the 8580 that plays digis the output is 4.66 and 0x93 is the only value to reach that.
// To me that seems as regular 8580s have somewhat wide 0-level range,
// whereas that digi-compatible 8580 has it very narrow.
// On my 6581R4AR has 0x3A as the only value giving the same output level as 1.prg
//
// Note: this needs more in-depth analysis.
// There is no evidence in the schematics for a DC offset in the 8580
// and it would make digis too loud
const OFFSET_6581: u32 = 0x380;

const ENV_DAC_BITS: u32 = 8;
const OSC_DAC_BITS: u32 = 12;

// Bus value stays alive for some time after each operation.
// Values differs between chip models, the timings used here
// are taken from VICE [1].
// See also the discussion "How do I reliably detect 6581/8580 sid?" on CSDb [2].
//
//   Results from real C64 (testprogs/SID/bitfade/delayfrq0.prg):
//
//   (new SID) (250469/8580R5) (250469/8580R5)
//   delayfrq0    ~7a000        ~108000
//
//   (old SID) (250407/6581)
//   delayfrq0    ~01d00
//
// [1]: http://sourceforge.net/p/vice-emu/patches/99/
// [2]: http://noname.c64.org/csdb/forums/?roomid=11&topicid=29025&showallposts=1
const BUS_TTL_6581: i32 = 0x01d00;
const BUS_TTL_8580: i32 = 0xa2000;

/// What a filter that does not exist yet must be told when it is built.
///
/// Upstream builds both filters (and their large lookup tables) in the
/// constructor and forwards every register write and parameter to both. Here a
/// filter is built the first time its chip model is selected (building one
/// model's tables takes about a second), so what was forwarded to the other
/// model is remembered here and replayed on creation.
struct Pending {
    fc_lo: Option<u8>,
    fc_hi: Option<u8>,
    res_filt: Option<u8>,
    mode_vol: Option<u8>,
    enabled: bool,
    input: i16,
    curve_6581: Option<f64>,
    range_6581: Option<f64>,
    old_caps_6581: Option<bool>,
    curve_8580: Option<f64>,
}

impl Pending {
    fn apply(&self, filter: &mut Filter) {
        filter.input(self.input);

        if let Some(v) = self.curve_6581 {
            filter.set_filter_6581_curve(v);
        }
        if let Some(v) = self.range_6581 {
            filter.set_filter_6581_range(v);
        }
        if let Some(v) = self.old_caps_6581 {
            filter.enable_old_caps(v);
        }
        if let Some(v) = self.curve_8580 {
            filter.set_filter_8580_curve(v);
        }

        // Same as the Reset of the constructor, then the register writes since
        filter.reset();
        filter.enable(self.enabled);

        if let Some(v) = self.fc_lo {
            filter.write_fc_lo(v);
        }
        if let Some(v) = self.fc_hi {
            filter.write_fc_hi(v);
        }
        if let Some(v) = self.res_filt {
            filter.write_res_filt(v);
        }
        if let Some(v) = self.mode_vol {
            filter.write_mode_vol(v);
        }
    }
}

pub struct Sid {
    /// SID voices
    voice: [Voice; 3],

    /// Filter used, if model is set to 6581
    filter6581: Option<Filter>,

    /// Filter used, if model is set to 8580
    filter8580: Option<Filter>,

    /// Resampler used by audio generation code
    resampler: Option<Resampler>,

    /// External filter that provides high-pass and low-pass filtering
    /// to adjust sound tone slightly
    external_filter: ExternalFilter,

    /// Used to amplify the output by x/2 to get an adequate playback volume
    scale_factor: i32,

    /// Time to live for the last written value
    bus_value_ttl: i32,

    /// Current chip model's bus value TTL
    model_ttl: i32,

    /// Time until VoiceSync() must run
    next_voice_sync: u32,

    /// Currently active chip model
    model: ChipModel,

    /// Currently selected combined waveforms strength
    cws: CombinedWaveforms,

    /// Last written value
    bus_value: u8,

    /// Paddle coordinates
    paddle_x: u8,
    paddle_y: u8,

    /// Emulated non-linearity of the envelope DAC
    env_dac: Box<[f32; 256]>,

    /// Emulated non-linearity of the oscillator DAC
    osc_dac: Box<[f32; 4096]>,

    pending: Pending,
}

impl Default for Sid {
    fn default() -> Self {
        Self::new()
    }
}

impl Sid {
    /// Creates a SID emulating a CSG 8580, builds the tables of that model (a
    /// second or so in release mode, once per process). No sampling parameters
    /// are set: call `set_sampling_parameters` before `clock`
    pub fn new() -> Self {
        let wave_tables = get_wave_table();
        let pulldown_tables = build_pulldown_table(ChipModel::Csg8580, CombinedWaveforms::Average);

        let mut sid = Sid {
            voice: [
                Voice::new(wave_tables, pulldown_tables, false),
                Voice::new(wave_tables, pulldown_tables, false),
                Voice::new(wave_tables, pulldown_tables, false),
            ],
            filter6581: None,
            filter8580: None,
            resampler: None,
            external_filter: ExternalFilter::new(),
            scale_factor: 0,
            bus_value_ttl: 0,
            model_ttl: 0,
            next_voice_sync: 0,
            model: ChipModel::Csg8580,
            cws: CombinedWaveforms::Average,
            bus_value: 0,
            paddle_x: 0xff,
            paddle_y: 0xff,
            env_dac: Box::new([0.0; 256]),
            osc_dac: Box::new([0.0; 4096]),
            pending: Pending {
                fc_lo: None,
                fc_hi: None,
                res_filt: None,
                mode_vol: None,
                enabled: true,
                input: 0,
                curve_6581: None,
                range_6581: None,
                old_caps_6581: None,
                curve_8580: None,
            },
        };

        sid.set_chip_model(ChipModel::Csg8580);
        sid.reset();

        sid
    }

    /// Set filter curve parameter for 6581 model
    pub fn set_filter_6581_curve(&mut self, filter_curve: f64) {
        self.pending.curve_6581 = Some(filter_curve);

        if let Some(f) = &mut self.filter6581 {
            f.set_filter_6581_curve(filter_curve);
        }
    }

    /// Set filter range parameter for 6581 model
    pub fn set_filter_6581_range(&mut self, adjustment: f64) {
        self.pending.range_6581 = Some(adjustment);

        if let Some(f) = &mut self.filter6581 {
            f.set_filter_6581_range(adjustment);
        }
    }

    /// Set filter curve parameter for 8580 model
    pub fn set_filter_8580_curve(&mut self, filter_curve: f64) {
        self.pending.curve_8580 = Some(filter_curve);

        if let Some(f) = &mut self.filter8580 {
            f.set_filter_8580_curve(filter_curve);
        }
    }

    /// Enable filter emulation
    pub fn enable_filter(&mut self, enable: bool) {
        self.pending.enabled = enable;

        if let Some(f) = &mut self.filter6581 {
            f.enable(enable);
        }
        if let Some(f) = &mut self.filter8580 {
            f.enable(enable);
        }
    }

    /// Enable/disable old caps (2200pF) for 6581 model
    pub fn enable_old_6581_caps(&mut self, enable: bool) {
        self.pending.old_caps_6581 = Some(enable);

        if let Some(f) = &mut self.filter6581 {
            f.enable_old_caps(enable);
        }
    }

    /// Set chip model
    pub fn set_chip_model(&mut self, new_model: ChipModel) {
        // Build the filter of the model when it is used the first time
        let slot = match new_model {
            ChipModel::Mos6581 => &mut self.filter6581,
            ChipModel::Csg8580 => &mut self.filter8580,
        };

        if slot.is_none() {
            let mut filter = match new_model {
                ChipModel::Mos6581 => Filter::new_6581(),
                ChipModel::Csg8580 => Filter::new_8580(),
            };
            self.pending.apply(&mut filter);
            *slot = Some(filter);
        }

        match new_model {
            ChipModel::Mos6581 => {
                self.scale_factor = 3;
                self.model_ttl = BUS_TTL_6581;
            }

            ChipModel::Csg8580 => {
                self.scale_factor = 5;
                self.model_ttl = BUS_TTL_8580;
            }
        }

        self.model = new_model;

        // Calculate waveform-related tables
        let wave_tables = get_wave_table();
        let pulldown_tables = build_pulldown_table(self.model, self.cws);

        // Calculate envelope DAC table
        {
            let mut dac_builder = Dac::new(ENV_DAC_BITS);
            dac_builder.kinked_dac(self.model);

            for i in 0..(1u32 << ENV_DAC_BITS) {
                self.env_dac[i as usize] = dac_builder.get_output(i, false) as f32;
            }
        }

        // Calculate oscillator DAC table
        let is6581 = self.model == ChipModel::Mos6581;

        {
            let mut dac_builder = Dac::new(OSC_DAC_BITS);
            dac_builder.kinked_dac(self.model);

            let offset = dac_builder.get_output(if is6581 { OFFSET_6581 } else { 0x7ff }, is6581);

            for i in 0..(1u32 << OSC_DAC_BITS) {
                let dac_value = dac_builder.get_output(i, is6581);
                self.osc_dac[i as usize] = (dac_value - offset) as f32;
            }
        }

        // Set voice tables
        for v in self.voice.iter_mut() {
            v.wave.set_model(is6581);
            v.wave.set_waveform_models(wave_tables);
            v.wave.set_pulldown_models(pulldown_tables);
        }

        if let Some(f) = self.current_filter() {
            f.restart();
        }
    }

    /// Get currently emulated chip model
    pub fn get_chip_model(&self) -> ChipModel {
        self.model
    }

    /// Set combined waveforms strength
    pub fn set_combined_waveforms(&mut self, new_cws: CombinedWaveforms) {
        self.cws = new_cws;

        // Rebuild waveform related tables
        let pulldown_tables = build_pulldown_table(self.model, self.cws);

        for v in self.voice.iter_mut() {
            v.wave.set_pulldown_models(pulldown_tables);
        }
    }

    /// SID reset
    pub fn reset(&mut self) {
        for v in self.voice.iter_mut() {
            v.reset();
        }

        if let Some(f) = &mut self.filter6581 {
            f.reset();
        }
        if let Some(f) = &mut self.filter8580 {
            f.reset();
        }

        // The reset state, to replay for a filter that is built later
        self.pending.fc_lo = None;
        self.pending.fc_hi = None;
        self.pending.res_filt = None;
        self.pending.mode_vol = None;

        self.external_filter.reset();

        if let Some(r) = &mut self.resampler {
            r.reset();
        }

        self.bus_value = 0;
        self.bus_value_ttl = 0;
        self.voice_sync(false);
    }

    /// 16-bit input (EXT IN). Write 16-bit sample to audio input.
    /// NB! The caller is responsible for keeping the value within 16
    /// bits. Note that to mix in an external audio signal, the signal
    /// should be resampled to 1MHz first to avoid sampling noise
    pub fn input(&mut self, value: i32) {
        let value = value as i16;

        self.pending.input = value;

        if let Some(f) = &mut self.filter6581 {
            f.input(value);
        }
        if let Some(f) = &mut self.filter8580 {
            f.input(value);
        }
    }

    /// Read registers.
    ///
    /// Reading a write only register returns the last char written to
    /// any SID register. The individual bits in this value start to fade
    /// down towards zero after a few cycles. All bits reach zero within
    /// approximately $2000 - $4000 cycles. It has been claimed that this
    /// fading happens in an orderly fashion, however sampling of write
    /// only registers reveals that this is not the case.
    /// NOTE: This is not correctly modeled.
    /// The actual use of write only registers has largely been made in
    /// the belief that all SID registers are readable. To support this
    /// belief the read would have to be done immediately after a write
    /// to the same register (remember that an intermediate write to
    /// another register would yield that value instead). With this in
    /// mind we return the last value written to any SID register for
    /// $2000 cycles without modeling the bit fading
    pub fn read(&mut self, offset: u8) -> u8 {
        match offset & 0x1f {
            // X value of paddle
            0x19 => {
                self.bus_value = self.paddle_x;
                self.bus_value_ttl = self.model_ttl;
            }

            // Y value of paddle
            0x1a => {
                self.bus_value = self.paddle_y;
                self.bus_value_ttl = self.model_ttl;
            }

            // Voice #3 waveform output
            0x1b => {
                self.bus_value = self.voice[2].wave.read_osc();
                self.bus_value_ttl = self.model_ttl;
            }

            // Voice #3 ADSR output
            0x1c => {
                self.bus_value = self.voice[2].envelope.read_env();
                self.bus_value_ttl = self.model_ttl;
            }

            _ => {
                // Reading from a write-only or non-existing register
                // makes the bus discharge faster.
                // Emulate this by halving the residual TTL
                self.bus_value_ttl /= 2;
            }
        }

        self.bus_value
    }

    /// Write registers
    pub fn write(&mut self, offset: u8, value: u8) {
        self.bus_value = value;
        self.bus_value_ttl = self.model_ttl;

        let offset = offset & 0x1f;

        match offset {
            // Voices 1 to 3: frequency (low byte, high byte), pulse width (low
            // byte, bits #8-#15), control register, attack and decay length,
            // sustain volume and release length
            0x00..=0x14 => {
                let v = &mut self.voice[usize::from(offset / 7)];

                match offset % 7 {
                    0 => v.wave.write_freq_lo(value),
                    1 => v.wave.write_freq_hi(value),
                    2 => v.wave.write_pw_lo(value),
                    3 => v.wave.write_pw_hi(value),
                    4 => v.write_control_reg(value),
                    5 => v.envelope.write_attack_decay(value),
                    _ => v.envelope.write_sustain_release(value),
                }
            }

            // Filter cut off frequency (bits #0-#2)
            0x15 => {
                self.pending.fc_lo = Some(value);

                if let Some(f) = &mut self.filter6581 {
                    f.write_fc_lo(value);
                }
                if let Some(f) = &mut self.filter8580 {
                    f.write_fc_lo(value);
                }
            }

            // Filter cut off frequency (bits #3-#10)
            0x16 => {
                self.pending.fc_hi = Some(value);

                if let Some(f) = &mut self.filter6581 {
                    f.write_fc_hi(value);
                }
                if let Some(f) = &mut self.filter8580 {
                    f.write_fc_hi(value);
                }
            }

            // Filter control
            0x17 => {
                self.pending.res_filt = Some(value);

                if let Some(f) = &mut self.filter6581 {
                    f.write_res_filt(value);
                }
                if let Some(f) = &mut self.filter8580 {
                    f.write_res_filt(value);
                }
            }

            // Volume and filter modes
            0x18 => {
                self.pending.mode_vol = Some(value);

                if let Some(f) = &mut self.filter6581 {
                    f.write_mode_vol(value);
                }
                if let Some(f) = &mut self.filter8580 {
                    f.write_mode_vol(value);
                }
            }

            _ => {}
        }

        // Update voice sync just in case
        self.voice_sync(false);
    }

    /// Clock SID forward using chosen output resampling algorithm.
    ///
    /// Returns the number of samples written to `buf`. Once `buf` is full the
    /// chip is still clocked for all the `cycles` but no sample is stored.
    /// Without sampling parameters (`set_sampling_parameters`) no samples are
    /// produced either
    #[inline]
    pub fn clock(&mut self, mut cycles: u32, buf: &mut [i16]) -> usize {
        self.age_bus_value(cycles as i32);
        let mut s = 0;

        while cycles != 0 {
            let delta_t = self.next_voice_sync.min(cycles);

            if delta_t > 0 {
                self.run_cycles(delta_t, buf, &mut s);

                cycles -= delta_t;
                self.next_voice_sync -= delta_t;
            }

            if self.next_voice_sync == 0 {
                self.voice_sync(true);
            }
        }

        s
    }

    /// Setting of SID sampling parameters.
    ///
    /// Use a clock freqency of 985248Hz for PAL C64, 1022730Hz for
    /// NTSC C64. The default end of passband frequency is
    /// pass_freq = 0.9*sample_freq/2 for sample frequencies up to
    /// ~ 44.1kHz, and 20kHz for higher sample frequencies.
    ///
    /// For resampling, the ratio between the clock frequency and the
    /// sample frequency is limited as follows:
    /// 125*clock_freq/sample_freq < 16384
    /// E.g. provided a clock frequency of ~ 1MHz, the sample frequency
    /// can not be set lower than ~ 8kHz. A lower sample frequency would
    /// make the resampling code overfill its 16k sample ring buffer.
    ///
    /// The end of passband frequency is also limited:
    /// pass_freq <= 0.9*sample_freq/2
    ///
    /// E.g. for a 44.1kHz sampling rate the end of passband frequency
    /// is limited to slightly below 20kHz.
    /// This constraint ensures that the FIR table is not overfilled
    ///
    /// Frequencies the resampler can not handle (not finite or not positive, a
    /// sample rate above the clock) are an error and change nothing
    pub fn set_sampling_parameters(
        &mut self,
        clock_frequency: f64,
        method: SamplingMethod,
        sampling_frequency: f64,
    ) -> Result<(), &'static str> {
        let resampler = match method {
            SamplingMethod::Decimate => {
                Resampler::ZeroOrder(ZeroOrderResampler::new(clock_frequency, sampling_frequency)?)
            }

            SamplingMethod::Resample => Resampler::TwoPassSinc(Box::new(TwoPassSincResampler::create(
                clock_frequency,
                sampling_frequency,
            )?)),
        };

        self.external_filter.set_clock_frequency(clock_frequency);
        self.resampler = Some(resampler);

        Ok(())
    }

    /// Set paddle coordinates
    pub fn set_paddle(&mut self, x: u8, y: u8) {
        self.paddle_x = x;
        self.paddle_y = y;
    }

    fn current_filter(&mut self) -> Option<&mut Filter> {
        match self.model {
            ChipModel::Mos6581 => self.filter6581.as_mut(),
            ChipModel::Csg8580 => self.filter8580.as_mut(),
        }
    }

    /// Clock the waveform generators, the envelope generators, the internal and
    /// external filters and the resampler `cycles` times
    #[inline(always)]
    #[allow(clippy::collapsible_if)] // no let chains, to build with older compilers
    fn run_cycles(&mut self, cycles: u32, buf: &mut [i16], s: &mut usize) {
        let Sid {
            voice,
            filter6581,
            filter8580,
            resampler,
            external_filter,
            scale_factor,
            model,
            env_dac,
            osc_dac,
            ..
        } = self;

        let filter = match model {
            ChipModel::Mos6581 => filter6581.as_mut(),
            ChipModel::Csg8580 => filter8580.as_mut(),
        };

        // The filter of the current model always exists
        let Some(filter) = filter else {
            return;
        };

        let env_dac: &[f32; 256] = env_dac;
        let osc_dac: &[f32; 4096] = osc_dac;

        for _ in 0..cycles {
            // Clock waveform generators
            voice[0].wave.clock();
            voice[1].wave.clock();
            voice[2].wave.clock();

            // Clock envelope generators
            voice[0].envelope.clock();
            voice[1].envelope.clock();
            voice[2].envelope.clock();

            // Waveform outputs (the ring modulation source of a voice is the
            // accumulator of the previous one, as the earlier outputs left it)
            let acc2 = voice[2].wave.accumulator;
            let wav0 = voice[0].output(acc2, osc_dac, env_dac);
            let acc0 = voice[0].wave.accumulator;
            let wav1 = voice[1].output(acc0, osc_dac, env_dac);
            let acc1 = voice[1].wave.accumulator;
            let wav2 = voice[2].output(acc1, osc_dac, env_dac);

            // Clock internal and external filters
            let filt_output = filter.clock(
                [wav0, wav1, wav2],
                [
                    voice[0].envelope.output(),
                    voice[1].envelope.output(),
                    voice[2].envelope.output(),
                ],
            );
            let ex_filt_input = i32::from(filt_output) + i32::from(i16::MIN);
            let output = external_filter.clock(ex_filt_input);

            if let Some(r) = resampler {
                if r.input(output) && *s < buf.len() {
                    buf[*s] = r.get_output(*scale_factor);
                    *s += 1;
                }
            }
        }
    }

    /// Calculate the number of cycles according to current parameters
    /// that it takes to reach sync
    fn voice_sync(&mut self, sync: bool) {
        if sync {
            // Synchronize the 3 waveform generators. The modulating (previous)
            // voice of voice i is (i + 2) % 3, the following one (i + 1) % 3
            for i in 0..3 {
                let next = (i + 1) % 3;
                let prev = (i + 2) % 3;

                let next_sync = self.voice[next].wave.sync;
                let prev_msb_rising = self.voice[prev].wave.msb_rising;

                if self.voice[i].wave.must_sync_next(next_sync, prev_msb_rising) {
                    self.voice[next].wave.accumulator = 0;
                }
            }
        }

        // Calculate the time to next voice sync
        self.next_voice_sync = i32::MAX as u32;

        for i in 0..3 {
            let wave = &self.voice[i].wave;
            let freq = wave.read_freq();

            if wave.read_test() || freq == 0 || !self.voice[(i + 1) % 3].wave.sync {
                continue;
            }

            let accumulator = wave.accumulator;
            let this_voice_sync = (0x7fffff_u32.wrapping_sub(accumulator) & 0xffffff) / freq + 1;

            if this_voice_sync < self.next_voice_sync {
                self.next_voice_sync = this_voice_sync;
            }
        }
    }

    /// Age the bus value and zero it if it's TTL has expired
    #[inline(always)]
    fn age_bus_value(&mut self, n: i32) {
        if self.bus_value_ttl != 0 {
            self.bus_value_ttl = self.bus_value_ttl.wrapping_sub(n);

            if self.bus_value_ttl <= 0 {
                self.bus_value = 0;
                self.bus_value_ttl = 0;
            }
        }
    }
}
