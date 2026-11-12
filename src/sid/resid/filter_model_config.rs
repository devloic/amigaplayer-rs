// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/FilterModelConfig.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::op_amp::OpAmp;
use super::spline::{Point, Spline};

/// Offset of the highpass summer table with `i` inputs.
/// The highpass summer has 2 - 6 inputs (bandpass, lowpass, and 0 - 4 voices)
pub fn summer_offset(i: usize) -> usize {
    if i == 0 {
        return 0;
    }

    summer_offset(i - 1) + ((2 + i - 1) << 16)
}

/// Offset of the mixer table with `i` inputs.
/// The mixer has 0 - 7 inputs (0 - 4 voices and 0 - 3 filter outputs)
pub fn mixer_offset(i: usize) -> usize {
    if i == 0 {
        return 0;
    }

    if i == 1 {
        return 1;
    }

    mixer_offset(i - 1) + ((i - 1) << 16)
}

/// Hack to add quick dither when converting values from float to int
/// and avoid quantization noise.
/// Hopefully this can be removed the day we move all the analog part
/// processing to floats.
///
/// Not sure about the effect of using such small buffer of numbers
/// since the random sequence repeats every 1024 values but for
/// now it seems to do the job
///
/// Upstream fills the buffer from the shared .NET random generator, so its
/// output differs from run to run. Here it is a fixed-seed xorshift64
/// sequence, which makes every table and every rendered sample reproducible.
pub struct Dither {
    buffer: Box<[f64; 1024]>,
    index: usize,
}

impl Dither {
    pub fn new() -> Self {
        let mut buffer = Box::new([0.0f64; 1024]);
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;

        for b in buffer.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *b = (state >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0);
        }

        Dither { buffer, index: 0 }
    }

    #[inline(always)]
    pub fn get_noise(&mut self) -> f64 {
        self.index = (self.index + 1) & 0x3ff;

        self.buffer[self.index]
    }
}

/// Parameters, tables and conversions shared by the 6581 and 8580 models
pub struct FilterModelConfig {
    /// Capacitor value
    pub c: f64,

    /// Threshold voltage
    pub vth: f64,

    /// Vdd - Vth
    pub vddt: f64,

    // Derived stuff
    pub v_min: f64,
    pub v_max: f64,
    pub denorm: f64,
    pub norm: f64,

    /// Fixed point scaling for 16 bit op-amp output
    pub n16: f64,

    pub voice_voltage_range: f64,

    /// Current factor coefficient for op-amp integrators
    pub curr_factor_coeff: f64,

    // Lookup tables for gain and summer op-amps in output stage / filter
    pub mixer: Vec<u16>,
    pub summer: Vec<u16>,
    pub volume: Vec<u16>,
    pub resonance: Vec<u16>,

    /// Reverse op-amp transfer function
    opamp_rev: Vec<u16>,
}

#[inline(always)]
fn to_u16_dither(x: f64, noise: f64) -> u16 {
    let tmp = (x + noise) as i32;
    tmp as u16
}

#[inline(always)]
pub fn to_u16(x: f64) -> u16 {
    to_u16_dither(x, 0.5)
}

impl FilterModelConfig {
    pub fn new(
        vvr: f64,
        c: f64,
        vdd: f64,
        vth: f64,
        u_cox: f64,
        opamp_voltage: &[Point],
    ) -> Self {
        let vddt = vdd - vth;
        let v_min = opamp_voltage[0].x;
        let v_max = vddt.max(opamp_voltage[0].y);
        let denorm = v_max - v_min;
        let norm = 1.0 / denorm;
        let n16 = norm * 65535.0;
        let curr_factor_coeff = denorm * (u_cox / 2.0 * 1.0e-6 / c);

        let mut this = FilterModelConfig {
            c,
            vth,
            vddt,
            v_min,
            v_max,
            denorm,
            norm,
            n16,
            voice_voltage_range: vvr,
            curr_factor_coeff,
            mixer: vec![0; mixer_offset(8)],
            summer: vec![0; summer_offset(5)],
            volume: vec![0; 16 * (1 << 16)],
            resonance: vec![0; 16 * (1 << 16)],
            opamp_rev: vec![0; 1 << 16],
        };

        // Convert op-amp voltage transfer to 16 bit values
        let scaled_voltage: Vec<Point> = opamp_voltage
            .iter()
            .map(|p| {
                Point::new(
                    n16 * (p.x - p.y) / 2.0
                        // We add 32768 to get a positive number in the range [0-65535]
                        + 32768.0,
                    n16 * (p.x - v_min),
                )
            })
            .collect();

        // Create lookup table mapping capacitor voltage to op amp input voltage
        let mut s = Spline::new(&scaled_voltage);

        for x in 0..(1usize << 16) {
            let out = s.evaluate(x as f64);

            // When interpolating outside range the first element may be negative
            this.opamp_rev[x] = if out.x > 0.0 { to_u16(out.x) } else { 0 };
        }

        this
    }

    #[inline(always)]
    pub fn get_opamp_rev(&self, i: i32) -> u16 {
        // Upstream indexes the table directly; stay in range
        self.opamp_rev[(i as u32).min(0xffff) as usize]
    }

    #[inline(always)]
    pub fn get_normalized_value(&self, value: f64, dither: &mut Dither) -> u16 {
        to_u16_dither(self.n16 * (value - self.v_min), dither.get_noise())
    }

    #[inline(always)]
    pub fn get_normalized_current_factor(&self, coeff: f64, n: i32, wl: f64) -> u16 {
        to_u16((1 << n) as f64 * coeff * wl)
    }

    #[inline(always)]
    pub fn get_nv_min(&self) -> u16 {
        to_u16(self.n16 * self.v_min)
    }

    /// `voice_dc` is the model's DC offset of the voice for the envelope value
    #[inline(always)]
    pub fn get_normalized_voice(&self, value: f32, voice_dc: f64, dither: &mut Dither) -> i32 {
        self.get_normalized_value(self.get_voice_voltage(value, voice_dc), dither) as i32
    }

    #[inline(always)]
    fn get_voice_voltage(&self, value: f32, voice_dc: f64) -> f64 {
        (value as f64 * self.voice_voltage_range) + voice_dc
    }

    /// The filter summer operates at n ~ 1, and has 5 fundamentally
    /// different input configurations (2 - 6 input "resistors")
    ///
    /// Note that all "on" transistors are modeled as one. This is not
    /// entirely accurate, since the input for each transistor is
    /// different, and transistors are not linear components. However
    /// modeling all transistors separately would be extremely costly
    pub fn build_summer_table(&mut self, op_amp_model: &mut OpAmp, dither: &mut Dither) {
        let r_n16 = 1.0 / self.n16;

        let mut idx = 0;
        for i in 0..5 {
            let i_div = 2 + i; // 2 - 6 input "resistors"
            let size = i_div << 16;
            let n = i_div as f64;
            let r_i_div = 1.0 / i_div as f64;

            op_amp_model.reset();

            for vi in 0..size {
                let v_in = self.v_min + vi as f64 * r_n16 * r_i_div; // vMin .. vMax
                let v = op_amp_model.solve(n, v_in);
                self.summer[idx] = self.get_normalized_value(v, dither);
                idx += 1;
            }
        }
    }

    /// The audio mixer operates at n ~ 8/6 (6581) or 8/5 (8580), and has
    /// 8 fundamentally different input configurations (0 - 7 input
    /// "resistors").
    ///
    /// All "on", transistors are modeled as one - see comments above
    /// for the filter summer
    pub fn build_mixer_table(&mut self, op_amp_model: &mut OpAmp, n_ratio: f64, dither: &mut Dither) {
        let r_n16 = 1.0 / self.n16;

        let mut idx = 0;
        for i in 0..8usize {
            let i_div = if i == 0 { 1 } else { i };
            let size = if i == 0 { 1 } else { i << 16 };
            let n = i as f64 * n_ratio;
            let r_i_div = 1.0 / i_div as f64;

            op_amp_model.reset();

            for vi in 0..size {
                let v_in = self.v_min + vi as f64 * r_n16 * r_i_div; // vMin .. vMax
                let v = op_amp_model.solve(n, v_in);
                self.mixer[idx] = self.get_normalized_value(v, dither);
                idx += 1;
            }
        }
    }

    /// 4 bit "resistor" ladders in the audio output gain necessitate 16
    /// gain tables. From die photographs of the volume "resistor"
    /// ladders it follows that gain ~ vol/12 (6581) or vol/16 (8580)
    /// (assuming ideal op-amps and ideal "resistors")
    pub fn build_volume_table(&mut self, op_amp_model: &mut OpAmp, n_divisor: f64, dither: &mut Dither) {
        let r_n16 = 1.0 / self.n16;

        let mut idx = 0;
        for n8 in 0..16 {
            let size = 1 << 16;
            let n = n8 as f64 / n_divisor;

            op_amp_model.reset();

            for vi in 0..size {
                let v_in = self.v_min + vi as f64 * r_n16; // vMin .. vMax
                let v = op_amp_model.solve(n, v_in);
                self.volume[idx] = self.get_normalized_value(v, dither);
                idx += 1;
            }
        }
    }

    /// 4 bit "resistor" ladders in the bandpass resonance gain
    /// necessitate 16 gain tables. From die photographs of the bandpass
    /// "resistor" ladders it follows that 1/Q ~ ~res/8 (6581) or
    /// 2^((4 - res)/8) (8580)
    /// (assuming ideal op-amps and ideal "resistors")
    pub fn build_resonance_table(&mut self, op_amp_model: &mut OpAmp, resonance_n: &[f64; 16], dither: &mut Dither) {
        let r_n16 = 1.0 / self.n16;

        let mut idx = 0;
        for &n in resonance_n.iter() {
            let size = 1 << 16;

            op_amp_model.reset();

            for vi in 0..size {
                let v_in = self.v_min + vi as f64 * r_n16; // vMin .. vMax
                let v = op_amp_model.solve(n, v_in);
                self.resonance[idx] = self.get_normalized_value(v, dither);
                idx += 1;
            }
        }
    }

    pub fn calc_curr_factor_coeff(&self, u_cox: f64) -> f64 {
        self.denorm * (u_cox / 2.0 * 1.0e-6 / self.c)
    }
}
