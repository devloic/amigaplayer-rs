// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/FilterModelConfig6581.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use std::sync::OnceLock;

use super::ChipModel;
use super::dac::Dac;
use super::filter_model_config::{Dither, FilterModelConfig, to_u16};
use super::fmath;
use super::op_amp::OpAmp;
use super::spline::Point;

// Calculate parameters for 6581 filter emulation

/// The ratio of the resistors of the mixer input for filter signals
/// compared to the voice ones.
/// See the 6581 filter schematic
pub const VF_TR_RATIO: f64 = 1.07;

const DAC_BITS: u32 = 11;

// Power bricks generate voltages slightly out of spec
const VOLTAGE_SKEW: f64 = 1.015;

// Transistor parameters
const K: f64 = 1.380649e-23; // Boltzmann Constant
const Q: f64 = 1.602176634e-19; // Charge of an electron
const TEMP: f64 = 27.0; // Temperature in °C
const UT: f64 = (K * (TEMP + 273.15)) / Q;

// This is the SID 6581 op-amp voltage transfer function, measured on
// CAP1B/CAP1A on a chip marked MOS 6581R4AR 0687 14.
// All measured chips have op-amps with output voltages (and thus input
// voltages) within the range of 0.81V - 10.31V
static OPAMP_VOLTAGE: [Point; 33] = [
    Point::new(0.81, 10.31), // Approximate start of actual range
    Point::new(2.40, 10.31),
    Point::new(2.60, 10.30),
    Point::new(2.70, 10.29),
    Point::new(2.80, 10.26),
    Point::new(2.90, 10.17),
    Point::new(3.00, 10.04),
    Point::new(3.10, 9.83),
    Point::new(3.20, 9.58),
    Point::new(3.30, 9.32),
    Point::new(3.50, 8.69),
    Point::new(3.70, 8.00),
    Point::new(4.00, 6.89),
    Point::new(4.40, 5.21),
    Point::new(4.54, 4.54), // Working point (vi = vo)
    Point::new(4.60, 4.19),
    Point::new(4.80, 3.00),
    Point::new(4.90, 2.30), // Change of curvature
    Point::new(4.95, 2.03),
    Point::new(5.00, 1.88),
    Point::new(5.05, 1.77),
    Point::new(5.10, 1.69),
    Point::new(5.20, 1.58),
    Point::new(5.40, 1.44),
    Point::new(5.60, 1.33),
    Point::new(5.80, 1.26),
    Point::new(6.00, 1.21),
    Point::new(6.40, 1.12),
    Point::new(7.00, 1.02),
    Point::new(7.50, 0.97),
    Point::new(8.50, 0.89),
    Point::new(10.00, 0.81),
    Point::new(10.31, 0.81), // Approximate end of actual range
];

const CAPS_OLD: f64 = 2200e-12; // ASSY 326298 uses 2200pF caps
const CAPS_NEW: f64 = 470e-12; // Standard 470pF caps used on other ASSY

/// The immutable part of the 6581 model: the lookup tables. Built once, shared
/// by every `Sid` (upstream shares a mutable singleton)
pub struct Shared6581 {
    pub base: FilterModelConfig,

    // Transistor parameters
    wl_vcr: f64, // W/L for VCR
    wl_snake: f64, // W/L for "snake"

    // DAC parameters
    dac_zero: f64,
    dac_scale: f64,

    /// DAC lookup table
    dac: Dac,

    // Voltage Controlled Resistors
    vcr_n_vg: Vec<u16>,
    vcr_n_ids_term: Vec<f64>,

    // Voice DC offset LUT
    voice_dc: [f64; 256],
}

/// Compute log(1+x) without losing precision for small values of x
fn log1p(x: f64) -> f64 {
    fmath::ln(1.0 + x) - ((((1.0 + x) - 1.0) - x) / (1.0 + x))
}

impl Shared6581 {
    fn build() -> Self {
        let mut dither = Dither::new();

        let base = FilterModelConfig::new(
            1.5,                  // Voice voltage range FIXME should theoretically be ~3.571V
            CAPS_NEW,             // Capacitor value
            12.0 * VOLTAGE_SKEW,  // Vdd
            1.31,                 // Vth
            20e-6,                // uCox
            &OPAMP_VOLTAGE,
        );

        let mut this = Shared6581 {
            base,
            wl_vcr: 9.0 / 1.0,
            wl_snake: 1.0 / 115.0,
            dac_zero: 7.15,
            dac_scale: 2.63,
            dac: Dac::new(DAC_BITS),
            vcr_n_vg: vec![0; 1 << 16],
            vcr_n_ids_term: vec![0.0; 1 << 16],
            voice_dc: [0.0; 256],
        };

        this.dac.kinked_dac(ChipModel::Mos6581);

        {
            let mut env_dac = Dac::new(8);
            env_dac.kinked_dac(ChipModel::Mos6581);

            for i in 0..256 {
                let env_i = env_dac.get_output(i as u32, false);
                this.voice_dc[i] = (5.0 * VOLTAGE_SKEW) + (0.2143 * env_i);
            }
        }

        let vddt = this.base.vddt;
        let v_min = this.base.v_min;
        let v_max = this.base.v_max;

        // Create lookup tables for gains / summers
        {
            // Filter summer
            let mut op_amp_model = OpAmp::new(&OPAMP_VOLTAGE, vddt, v_min, v_max);
            this.base.build_summer_table(&mut op_amp_model, &mut dither);
        }

        {
            // Filter mixer
            let mut op_amp_model = OpAmp::new(&OPAMP_VOLTAGE, vddt, v_min, v_max);
            this.base
                .build_mixer_table(&mut op_amp_model, 8.0 / (6.0 * VF_TR_RATIO), &mut dither);
        }

        {
            // Filter gain
            let mut op_amp_model = OpAmp::new(&OPAMP_VOLTAGE, vddt, v_min, v_max);
            this.base.build_volume_table(&mut op_amp_model, 12.0, &mut dither);
        }

        {
            // Filter resonance
            let mut op_amp_model = OpAmp::new(&OPAMP_VOLTAGE, vddt, v_min, v_max);

            // Build temp n table
            let mut resonance_n = [0.0f64; 16];

            for (n8, r) in resonance_n.iter_mut().enumerate() {
                *r = ((!n8 & 0xf) as f64) / 8.0;
            }

            this.base.build_resonance_table(&mut op_amp_model, &resonance_n, &mut dither);
        }

        {
            // Filter VCR Vg
            let n_vddt = this.base.n16 * (vddt - v_min);

            for i in 0..(1u32 << 16) {
                // The table index is right-shifted 16 times in order to fit in
                // 16 bits; the argument to sqrt is thus multiplied by (1 << 16)
                this.vcr_n_vg[i as usize] = to_u16(n_vddt - ((i << 16) as f64).sqrt());
            }
        }

        {
            // Filter VCR Ids
            //
            //  EKV model:
            //
            //  Ids = Is * (if - ir)
            //  Is = (2 * u*Cox * Ut^2)/k * W/L
            //  if = ln^2(1 + e^((k*(Vg - Vt) - Vs)/(2*Ut))
            //  ir = ln^2(1 + e^((k*(Vg - Vt) - Vd)/(2*Ut))

            // Moderate inversion characteristic current
            // will be multiplied by uCox later
            let is = (2.0 * UT * UT) * this.wl_vcr;

            // Normalize current factor for 1 cycle at 1Mhz
            let n15 = this.base.norm * 32767.0;
            let n_is = n15 * 1.0e-6 / this.base.c * is;

            // kVgt_Vx = k*(Vg - Vt) - Vx
            // I.e. if k != 1.0, Vg must be scaled accordingly
            let r_n16_2ut = 1.0 / (this.base.n16 * 2.0 * UT);

            for i in 0..(1i32 << 16) {
                let k_vgt_vx = i + i32::from(i16::MIN);
                let log_term = log1p(fmath::exp(k_vgt_vx as f64 * r_n16_2ut));

                // Scaled by m*2^15
                this.vcr_n_ids_term[i as usize] = n_is * log_term * log_term;
            }
        }

        this
    }
}

static SHARED: OnceLock<Shared6581> = OnceLock::new();

/// A 6581 model: the shared tables plus the filter range and old caps
/// parameters of one `Sid`
pub struct FilterModelConfig6581 {
    shared: &'static Shared6581,

    /// Transconductance coefficient: u*Cox
    u_cox: f64,

    /// Current factor coefficient for op-amp integrators
    curr_factor_coeff: f64,

    vcr_mult: f64,
    old_caps: bool,
}

impl FilterModelConfig6581 {
    /// Builds the shared tables the first time (a second or so in release mode)
    pub fn new() -> Self {
        let shared = SHARED.get_or_init(Shared6581::build);

        let mut this = FilterModelConfig6581 {
            shared,
            u_cox: 20e-6,
            curr_factor_coeff: shared.base.curr_factor_coeff,
            vcr_mult: 0.0,
            old_caps: false,
        };

        this.update_params();

        this
    }

    pub fn base(&self) -> &'static FilterModelConfig {
        &self.shared.base
    }

    pub fn set_filter_range(&mut self, adjustment: f64) {
        // Clamp into allowed range (written out like Math.Clamp: a NaN passes through)
        #[allow(clippy::manual_clamp)]
        let adjustment = if adjustment < 0.0 {
            0.0
        } else if adjustment > 1.0 {
            1.0
        } else {
            adjustment
        };

        // Get the new uCox value, in the range [1,40]
        let new_u_cox = (1.0 + (39.0 * adjustment)) * 1e-6;

        // Ignore small changes
        if (self.u_cox - new_u_cox).abs() < 1e-12 {
            return;
        }

        self.u_cox = new_u_cox;
        self.update_params();
    }

    pub fn enable_old_caps(&mut self, enable: bool) {
        if self.old_caps == enable {
            return;
        }

        self.old_caps = enable;
        self.update_params();
    }

    /// Construct an 11 bit cutoff frequency DAC output voltage table
    pub fn get_dac(&self, adjustment: f64, dither: &mut Dither) -> Vec<u16> {
        let s = self.shared;
        let new_dac_zero = self.get_dac_zero(adjustment);

        let mut f0_dac = vec![0u16; 1 << DAC_BITS];

        for (i, v) in f0_dac.iter_mut().enumerate() {
            let fcd = s.dac.get_output(i as u32, false);
            *v = s.base.get_normalized_value(new_dac_zero + (fcd * s.dac_scale), dither);
        }

        f0_dac
    }

    #[inline(always)]
    pub fn get_wl_snake(&self) -> f64 {
        self.shared.wl_snake
    }

    #[inline(always)]
    pub fn get_normalized_current_factor(&self, n: i32, wl: f64) -> u16 {
        self.shared.base.get_normalized_current_factor(self.curr_factor_coeff, n, wl)
    }

    #[inline(always)]
    pub fn get_vcr_n_vg(&self, i: u32) -> u16 {
        self.shared.vcr_n_vg[(i & 0xffff) as usize]
    }

    #[inline(always)]
    pub fn get_vcr_n_ids_term(&self, i: i32) -> u16 {
        // Upstream indexes the table directly; stay in range
        let i = i.clamp(0, 0xffff) as usize;
        to_u16(self.shared.vcr_n_ids_term[i] * self.vcr_mult)
    }

    /// On 6581 the DC offset varies between ~5.0V and ~5.214V depending
    /// on the envelope value
    #[inline(always)]
    pub fn get_voice_dc(&self, env: u8) -> f64 {
        self.shared.voice_dc[env as usize]
    }

    fn get_dac_zero(&self, adjustment: f64) -> f64 {
        self.shared.dac_zero + (2.0 * adjustment) - 1.0
    }

    fn update_params(&mut self) {
        self.curr_factor_coeff = self.shared.base.calc_curr_factor_coeff(self.u_cox);
        self.vcr_mult = self.u_cox;

        if self.old_caps {
            let caps_mult = CAPS_NEW / CAPS_OLD;
            self.curr_factor_coeff *= caps_mult;
            self.vcr_mult *= caps_mult;
        }
    }
}
