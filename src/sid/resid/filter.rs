// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/Filter.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::filter6581::Filter6581;
use super::filter8580::Filter8580;
use super::filter_model_config::{Dither, FilterModelConfig, mixer_offset, summer_offset};
use super::filter_model_config6581::VF_TR_RATIO;

// SID filter base class. The C# abstract class with its two subclasses is an
// enum here, so that the per cycle calls are static.

enum Variant {
    M6581(Box<Filter6581>),
    M8580(Box<Filter8580>),
}

/// Table lookup. Upstream indexes the arrays directly (an exception when the
/// integrators ever drift out of range); out of range reads give 0 here
#[inline(always)]
fn lookup(table: &[u16], base: usize, idx: i32) -> i32 {
    let i = base as isize + idx as isize;

    if i < 0 {
        return 0;
    }

    table.get(i as usize).map_or(0, |&v| i32::from(v))
}

pub struct Filter {
    variant: Variant,

    base: &'static FilterModelConfig,

    dither: Dither,

    // Offsets into the model tables
    /// Current filter/voice mixer setting
    current_mixer: usize,

    /// Filter input summer setting
    current_summer: usize,

    /// Filter resonance value
    current_resonance: usize,

    /// Current volume amplifier setting
    current_volume: usize,

    /// Filter highpass state
    vhp: i32,

    /// Filter bandpass state
    vbp: i32,

    /// Filter lowpass state
    vlp: i32,

    /// Filter external input
    ext_in: f32,

    /// Filter cutoff frequency
    fc: u16,

    // Routing to filter or outside filter
    filt1: bool,
    filt2: bool,
    filt3: bool,
    filt_e: bool,

    /// Switch voice 3 off
    voice3_off: bool,

    // Highpass, bandpass, and lowpass filter modes
    hp: bool,
    bp: bool,
    lp: bool,

    /// Current volume
    vol: u8,

    /// Filter enabled
    enabled: bool,

    /// Selects which inputs to route through filter
    filt: u8,
}

impl Filter {
    pub fn new_6581() -> Self {
        let mut dither = Dither::new();
        let f = Box::new(Filter6581::new(&mut dither));
        let base = f.fmc.base();

        Self::with_variant(Variant::M6581(f), base, dither)
    }

    pub fn new_8580() -> Self {
        let mut dither = Dither::new();
        let f = Box::new(Filter8580::new(&mut dither));
        let base = &f.fmc.base;

        Self::with_variant(Variant::M8580(f), base, dither)
    }

    fn with_variant(variant: Variant, base: &'static FilterModelConfig, dither: Dither) -> Self {
        let mut this = Filter {
            variant,
            base,
            dither,
            current_mixer: 0,
            current_summer: 0,
            current_resonance: 0,
            current_volume: 0,
            vhp: 0,
            vbp: 0,
            vlp: 0,
            ext_in: 0.0,
            fc: 0,
            filt1: false,
            filt2: false,
            filt3: false,
            filt_e: false,
            voice3_off: false,
            hp: false,
            bp: false,
            lp: false,
            vol: 0,
            enabled: true,
            filt: 0,
        };

        this.input(0);

        this
    }

    /// Set the 6581 filter curve (no effect on the 8580)
    pub fn set_filter_6581_curve(&mut self, filter_curve: f64) {
        let fc = self.fc;

        if let Variant::M6581(f) = &mut self.variant {
            f.set_filter_curve(filter_curve, fc, &mut self.dither);
        }
    }

    /// Set the 6581 filter range (no effect on the 8580)
    pub fn set_filter_6581_range(&mut self, adjustment: f64) {
        if let Variant::M6581(f) = &mut self.variant {
            f.set_filter_range(adjustment);
        }
    }

    /// Enable/disable old caps for 6581 model (no effect on the 8580)
    pub fn enable_old_caps(&mut self, enable: bool) {
        if let Variant::M6581(f) = &mut self.variant {
            f.enable_old_caps(enable);
        }
    }

    /// Set the 8580 filter curve (no effect on the 6581)
    pub fn set_filter_8580_curve(&mut self, curve_position: f64) {
        if let Variant::M8580(f) = &mut self.variant {
            f.set_filter_curve(curve_position, &mut self.dither);
        }
    }

    /// Write frequency cutoff low register
    pub fn write_fc_lo(&mut self, fc_lo: u8) {
        self.fc = (self.fc & 0x7f8) | u16::from(fc_lo & 0x007);
        self.update_center_frequency();
    }

    /// Write frequency cutoff high register
    pub fn write_fc_hi(&mut self, fc_hi: u8) {
        self.fc = ((u16::from(fc_hi) << 3) & 0x7f8) | (self.fc & 0x007);
        self.update_center_frequency();
    }

    /// Write resonance/filter register
    pub fn write_res_filt(&mut self, res_filt: u8) {
        self.filt = res_filt;

        self.update_resonance((res_filt >> 4) & 0x0f);

        if self.enabled {
            self.filt1 = (self.filt & 0x01) != 0;
            self.filt2 = (self.filt & 0x02) != 0;
            self.filt3 = (self.filt & 0x04) != 0;
            self.filt_e = (self.filt & 0x08) != 0;
        }

        self.update_mixing();
    }

    /// Write filter mode/volume register
    pub fn write_mode_vol(&mut self, mode_vol: u8) {
        self.vol = mode_vol & 0x0f;
        self.lp = (mode_vol & 0x10) != 0;
        self.bp = (mode_vol & 0x20) != 0;
        self.hp = (mode_vol & 0x40) != 0;
        self.voice3_off = (mode_vol & 0x80) != 0;

        self.update_mixing();
    }

    #[inline(always)]
    fn get_normalized_voice(&mut self, v: f32, env: u8) -> i32 {
        match &self.variant {
            Variant::M6581(f) => self.base.get_normalized_voice(v, f.fmc.get_voice_dc(env), &mut self.dither),
            Variant::M8580(f) => self.base.get_normalized_voice(v, f.fmc.get_voice_dc(), &mut self.dither),
        }
    }

    /// The filter input resistors on the 6581 are slightly bigger than
    /// the voice ones, scale the values accordingly
    #[inline(always)]
    fn get_normalized_mixer_voice(&mut self, v: f32, env: u8) -> i32 {
        match &self.variant {
            Variant::M6581(f) => {
                self.base
                    .get_normalized_voice(v * (VF_TR_RATIO as f32), f.fmc.get_voice_dc(env), &mut self.dither)
            }
            Variant::M8580(f) => self.base.get_normalized_voice(v, f.fmc.get_voice_dc(), &mut self.dither),
        }
    }

    /// SID clocking - 1 cycle. `wav` are the three voices' waveform outputs
    /// (including the waveform and envelope DACs), `env` their envelope outputs
    #[inline(always)]
    pub fn clock(&mut self, wav: [f32; 3], env: [u8; 3]) -> u16 {
        let ext_in = self.ext_in;

        // Voltage summer for filter input
        let mut vsum: i32 = 0;
        if self.filt1 {
            vsum += self.get_normalized_voice(wav[0], env[0]);
        }
        if self.filt2 {
            vsum += self.get_normalized_voice(wav[1], env[1]);
        }
        if self.filt3 {
            vsum += self.get_normalized_voice(wav[2], env[2]);
        }
        if self.filt_e {
            vsum += self.get_normalized_voice(ext_in, 0);
        }
        vsum = vsum.wrapping_add(self.vlp);
        vsum = vsum.wrapping_add(lookup(&self.base.resonance, self.current_resonance, self.vbp));

        // Filter
        self.vhp = lookup(&self.base.summer, self.current_summer, vsum);
        let (vbp, vlp) = match &mut self.variant {
            Variant::M6581(f) => f.solve(self.vhp),
            Variant::M8580(f) => f.solve(self.vhp),
        };
        self.vbp = vbp;
        self.vlp = vlp;

        let mut vfilt: i32 = 0;

        if self.lp {
            vfilt = vfilt.wrapping_add(self.vlp);
        }

        if self.bp {
            vfilt = vfilt.wrapping_add(self.vbp);
        }

        if self.hp {
            vfilt = vfilt.wrapping_add(self.vhp);
        }

        // Voltage summer for mixer input
        let mut vmix: i32 = 0;
        if !self.filt1 {
            vmix += self.get_normalized_mixer_voice(wav[0], env[0]);
        }
        if !self.filt2 {
            vmix += self.get_normalized_mixer_voice(wav[1], env[1]);
        }

        // Voice 3 is silenced by voice3off if it is not routed through the filter
        if !(self.filt3 || self.voice3_off) {
            vmix += self.get_normalized_mixer_voice(wav[2], env[2]);
        }
        if !self.filt_e {
            vmix += self.get_normalized_mixer_voice(ext_in, 0);
        }
        vmix = vmix.wrapping_add(vfilt);

        let mixed = lookup(&self.base.mixer, self.current_mixer, vmix);
        lookup(&self.base.volume, self.current_volume, mixed) as u16
    }

    /// Enable filter
    pub fn enable(&mut self, enable: bool) {
        self.enabled = enable;

        if self.enabled {
            self.write_res_filt(self.filt);
        } else {
            self.filt1 = false;
            self.filt2 = false;
            self.filt3 = false;
            self.filt_e = false;
        }
    }

    /// SID reset
    pub fn reset(&mut self) {
        self.write_fc_lo(0);
        self.write_fc_hi(0);
        self.write_mode_vol(0);
        self.write_res_filt(0);
    }

    /// Apply a signal to EXT-IN
    pub fn input(&mut self, input: i16) {
        self.ext_in = f32::from(input) / 65535.0f32;
    }

    pub fn restart(&mut self) {
        match &mut self.variant {
            Variant::M6581(f) => f.restart_integrators(),
            Variant::M8580(f) => f.restart_integrators(),
        }

        self.vhp = 0;
        self.vlp = 0;
        self.vbp = 0;
    }

    /// Update filter cutoff frequency
    fn update_center_frequency(&mut self) {
        match &mut self.variant {
            Variant::M6581(f) => f.update_center_frequency(self.fc),
            Variant::M8580(f) => f.update_center_frequency(self.fc),
        }
    }

    /// Update filter resonance
    fn update_resonance(&mut self, res: u8) {
        self.current_resonance = usize::from(res) * (1 << 16);
    }

    /// Mixing configuration modified (offsets change)
    fn update_mixing(&mut self) {
        self.current_volume = usize::from(self.vol) * (1 << 16);

        let mut n_sum = 0;
        let mut n_mix = 0;

        if self.filt1 {
            n_sum += 1;
        } else {
            n_mix += 1;
        }

        if self.filt2 {
            n_sum += 1;
        } else {
            n_mix += 1;
        }

        if self.filt3 {
            n_sum += 1;
        } else if !self.voice3_off {
            n_mix += 1;
        }

        if self.filt_e {
            n_sum += 1;
        } else {
            n_mix += 1;
        }

        self.current_summer = summer_offset(n_sum);

        if self.lp {
            n_mix += 1;
        }

        if self.bp {
            n_mix += 1;
        }

        if self.hp {
            n_mix += 1;
        }

        self.current_mixer = mixer_offset(n_mix);
    }
}
