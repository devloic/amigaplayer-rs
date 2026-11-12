// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/FilterModelConfig8580.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use std::sync::OnceLock;

use super::filter_model_config::{Dither, FilterModelConfig};
use super::op_amp::OpAmp;
use super::spline::Point;

// Calculate parameters for 8580 filter emulation

// Reference voltage generated from Vcc by a voltage divider
const VREF: f64 = 4.75;

// Power bricks generate voltages slightly out of spec
const VOLTAGE_SKEW: f64 = 1.01;

// R1 = 15.3*Ri
// R2 =  7.3*Ri
// R3 =  4.7*Ri
// Rf =  1.4*Ri
// R4 =  1.4*Ri
// R8 =  2.0*Ri
// RC =  2.8*Ri
//
// res  feedback  input
// ---  --------  -----
//  0   Rf        Ri
//  1   Rf|R1     Ri
//  2   Rf|R2     Ri
//  3   Rf|R3     Ri
//  4   Rf        R4
//  5   Rf|R1     R4
//  6   Rf|R2     R4
//  7   Rf|R3     R4
//  8   Rf        R8
//  9   Rf|R1     R8
//  A   Rf|R2     R8
//  B   Rf|R3     R8
//  C   Rf        RC
//  D   Rf|R1     RC
//  E   Rf|R2     RC
//  F   Rf|R3     RC
#[allow(clippy::eq_op)]
const RES_GAIN: [f64; 16] = [
    1.4 / 1.0,                       // Rf/Ri        1.4
    ((1.4 * 15.3) / (1.4 + 15.3)) / 1.0, // (Rf|R1)/Ri   1.28263
    ((1.4 * 7.3) / (1.4 + 7.3)) / 1.0,   // (Rf|R2)/Ri   1.17471
    ((1.4 * 4.7) / (1.4 + 4.7)) / 1.0,   // (Rf|R3)/Ri   1.07869
    1.4 / 1.4,                       // Rf/R4        1
    ((1.4 * 15.3) / (1.4 + 15.3)) / 1.4, // (Rf|R1)/R4   0.916168
    ((1.4 * 7.3) / (1.4 + 7.3)) / 1.4,   // (Rf|R2)/R4   0.83908
    ((1.4 * 4.7) / (1.4 + 4.7)) / 1.4,   // (Rf|R3)/R4   0.770492
    1.4 / 2.0,                       // Rf/R8        0.7
    ((1.4 * 15.3) / (1.4 + 15.3)) / 2.0, // (Rf|R1)/R8   0.641317
    ((1.4 * 7.3) / (1.4 + 7.3)) / 2.0,   // (Rf|R2)/R8   0.587356
    ((1.4 * 4.7) / (1.4 + 4.7)) / 2.0,   // (Rf|R3)/R8   0.539344
    1.4 / 2.8,                       // Rf/RC        0.5
    ((1.4 * 15.3) / (1.4 + 15.3)) / 2.8, // (Rf|R1)/RC   0.458084
    ((1.4 * 7.3) / (1.4 + 7.3)) / 2.8,   // (Rf|R2)/RC   0.41954
    ((1.4 * 4.7) / (1.4 + 4.7)) / 2.8,   // (Rf|R3)/RC   0.385246
];

// This is the SID 8580 op-amp voltage transfer function, measured on
// CAP1B/CAP1A on a chip marked CSG 8580R5 1690 25
static OPAMP_VOLTAGE: [Point; 21] = [
    Point::new(1.30, 8.91), // Approximate start of actual range
    Point::new(4.76, 8.91),
    Point::new(4.77, 8.90),
    Point::new(4.78, 8.88),
    Point::new(4.785, 8.86),
    Point::new(4.79, 8.80),
    Point::new(4.795, 8.60),
    Point::new(4.80, 8.25),
    Point::new(4.805, 7.50),
    Point::new(4.81, 6.10),
    Point::new(4.815, 4.05), // Change of curvature
    Point::new(4.82, 2.27),
    Point::new(4.825, 1.65),
    Point::new(4.83, 1.55),
    Point::new(4.84, 1.47),
    Point::new(4.85, 1.43),
    Point::new(4.87, 1.37),
    Point::new(4.90, 1.34),
    Point::new(5.00, 1.30),
    Point::new(5.10, 1.30),
    Point::new(8.91, 1.30), // Approximate end of actual range
];

/// The 8580 model. It has no adjustable parameters, so one immutable instance
/// is shared by every `Sid`
pub struct FilterModelConfig8580 {
    pub base: FilterModelConfig,
}

static INSTANCE: OnceLock<FilterModelConfig8580> = OnceLock::new();

impl FilterModelConfig8580 {
    fn build() -> Self {
        let mut dither = Dither::new();

        let mut base = FilterModelConfig::new(
            0.24,  // Voice voltage range FIXME should theoretically be ~0,474V
            22e-9, // Capacitor value
            9.09,  // Vdd
            0.80,  // Vth
            100e-6, // uCox
            &OPAMP_VOLTAGE,
        );

        let vddt = base.vddt;
        let v_min = base.v_min;
        let v_max = base.v_max;

        // Create lookup tables for gains / summers
        {
            // Filter summer
            let mut op_amp_model = OpAmp::new(&OPAMP_VOLTAGE, vddt, v_min, v_max);
            base.build_summer_table(&mut op_amp_model, &mut dither);
        }

        {
            // Filter mixer
            let mut op_amp_model = OpAmp::new(&OPAMP_VOLTAGE, vddt, v_min, v_max);
            base.build_mixer_table(&mut op_amp_model, 8.0 / 5.0, &mut dither);
        }

        {
            // Filter gain
            let mut op_amp_model = OpAmp::new(&OPAMP_VOLTAGE, vddt, v_min, v_max);
            base.build_volume_table(&mut op_amp_model, 16.0, &mut dither);
        }

        {
            // Filter resonance
            let mut op_amp_model = OpAmp::new(&OPAMP_VOLTAGE, vddt, v_min, v_max);
            base.build_resonance_table(&mut op_amp_model, &RES_GAIN, &mut dither);
        }

        FilterModelConfig8580 { base }
    }

    /// Builds the tables the first time (a second or so in release mode)
    pub fn get_instance() -> &'static FilterModelConfig8580 {
        INSTANCE.get_or_init(FilterModelConfig8580::build)
    }

    #[inline(always)]
    pub fn get_v_ref(&self) -> f64 {
        VREF * VOLTAGE_SKEW
    }

    #[inline(always)]
    pub fn get_voice_dc(&self) -> f64 {
        self.get_v_ref()
    }

    #[inline(always)]
    pub fn get_normalized_current_factor(&self, n: i32, wl: f64) -> u16 {
        self.base.get_normalized_current_factor(self.base.curr_factor_coeff, n, wl)
    }
}
