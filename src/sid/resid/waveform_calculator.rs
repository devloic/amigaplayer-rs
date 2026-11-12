// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/WaveformCalculator.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use std::sync::OnceLock;

use super::fmath;
use super::{ChipModel, CombinedWaveforms};

// Combined waveform calculator for WaveformGenerator.
// By combining waveforms, the bits of each waveform are effectively short
// circuited, a zero bit in one waveform will result in a zero output bit,
// thus the infamous claim that the waveforms are AND'ed.
// However, a zero bit in one waveform may also affect the neighboring bits
// in the output.
//
// Example:
//
//                 1 1
//     Bit #       1 0 9 8 7 6 5 4 3 2 1 0
//                 -----------------------
//     Sawtooth    0 0 0 1 1 1 1 1 1 0 0 0
//
//     Triangle    0 0 1 1 1 1 1 1 0 0 0 0
//
//     AND         0 0 0 1 1 1 1 1 0 0 0 0
//
//     Output      0 0 0 0 1 1 1 0 0 0 0 0
//
//
// Re-vectorized die photographs reveal the mechanism behind this behavior.
// Each waveform selector bit acts as a switch, which directly connects
// internal outputs into the waveform DAC inputs as follows:
//
// - Noise outputs the shift register bits to DAC inputs as described above.
//   Each output is also used as input to the next bit when the shift register
//   is shifted. Lower four bits are grounded.
// - Pulse connects a single line to all DAC inputs. The line is connected to
//   either 5V (pulse on) or 0V (pulse off) at bit 11, and ends at bit 0.
// - Triangle connects the upper 11 bits of the (MSB EOR'ed) accumulator to the
//   DAC inputs, so that DAC bit 0 = 0, DAC bit n = accumulator bit n - 1.
// - Sawtooth connects the upper 12 bits of the accumulator to the DAC inputs,
//   so that DAC bit n = accumulator bit n. Sawtooth blocks out the MSB from
//   the EOR used to generate the triangle waveform.
//
// We can thus draw the following conclusions:
//
// - The shift register may be written to by combined waveforms.
// - The pulse waveform interconnects all bits in combined waveforms via the
//   pulse line.
// - The combination of triangle and sawtooth interconnects neighboring bits
//   of the sawtooth waveform.
//
// Also in the 6581 the MSB of the oscillator, used as input for the
// triangle xor logic and the pulse adder's last bit, is connected directly
// to the waveform selector, while in the 8580 it is latched at sid_clk2
// before being forwarded to the selector. Thus in the 6581 if the sawtooth MSB
// is pulled down it might affect the oscillator's adder
// driving the top bit low.

/// One 4096 entry waveform table
pub type WaveTable = [i16; 4096];

/// The four waveforms (none, triangle, sawtooth, saw+triangle)
pub type WaveTables = [WaveTable; 4];

/// The five combined waveform pulldown tables
pub type PulldownTables = [WaveTable; 5];

#[derive(Clone, Copy)]
enum Dist {
    Exp,
    Linear,
    Quadratic,
}

/// Combined waveform model parameters
struct Cfg {
    dist_func: Dist,
    threshold: f32,
    top_bit: f32,
    pulse_strength: f32,
    distance1: f32,
    distance2: f32,
}

impl Cfg {
    const fn new(
        dist_func: Dist,
        threshold: f32,
        top_bit: f32,
        pulse_strength: f32,
        distance1: f32,
        distance2: f32,
    ) -> Self {
        Cfg {
            dist_func,
            threshold,
            top_bit,
            pulse_strength,
            distance1,
            distance2,
        }
    }
}

// Parameters derived with the Monte Carlo method based on
// samplings from real machines.
// Code and data available in the project repository [1].
// Sampling program made by Dag Lem [2]
//
// The score here reported is the acoustic error
// calculated XORing the estimated and the sampled values.
// In parentheses the number of mispredicted bits.
//
// [1] https://github.com/libsidplayfp/combined-waveforms
// [2] https://github.com/daglem/reDIP-SID/blob/master/research/combsample.d64
static CONFIG_AVERAGE: [[Cfg; 5]; 2] = [
    [
        // 6581 R3 0486S sampled by Trurl
        // TS  error  3555 (324/32768) [RMS: 73.98]
        Cfg::new(Dist::Exp, 0.877322257f32, 1.11349654f32, 0.0f32, 2.14537621f32, 9.08618164f32),
        // PT  error  4590 (124/32768) [RMS: 68.90]
        Cfg::new(Dist::Linear, 0.941692829f32, 1.0f32, 1.80072665f32, 0.033124879f32, 0.232303441f32),
        // PS  error 19352 (763/32768) [RMS: 96.91]
        Cfg::new(Dist::Linear, 2.20329857f32, 1.04501438f32, 10.5146885f32, 0.277294368f32, 0.143747061f32),
        // PTS error  5068 ( 94/32768) [RMS: 41.69]
        Cfg::new(Dist::Linear, 1.09762526f32, 0.975265801f32, 1.52196741f32, 0.151528224f32, 0.841949463f32),
        // NP  guessed
        Cfg::new(Dist::Exp, 0.96f32, 1.0f32, 2.5f32, 1.1f32, 1.2f32),
    ],
    [
        // 8580 R5 1088 sampled by reFX-Mike
        // TS  error 10660 (353/32768) [RMS: 58.34]
        Cfg::new(Dist::Exp, 0.853578329f32, 1.09615636f32, 0.0f32, 1.8819375f32, 6.80794907f32),
        // PT  error 10635 (289/32768) [RMS: 108.81]
        Cfg::new(Dist::Exp, 0.929835618f32, 1.0f32, 1.12836814f32, 1.10453653f32, 1.48065746f32),
        // PS  error 12255 (554/32768) [RMS: 102.27]
        Cfg::new(Dist::Quadratic, 0.911938608f32, 0.996440411f32, 1.2278074f32, 0.000117214302f32, 0.18948476f32),
        // PTS error  6913 (127/32768) [RMS: 55.80]
        Cfg::new(Dist::Exp, 0.938004673f32, 1.04827631f32, 1.21178246f32, 0.915959001f32, 1.42698038f32),
        // NP  guessed
        Cfg::new(Dist::Exp, 0.95f32, 1.0f32, 1.15f32, 1.0f32, 1.45f32),
    ],
];

static CONFIG_WEAK: [[Cfg; 5]; 2] = [
    [
        // 6581 R2 4383 sampled by ltx128
        // TS  error 1474 (198/32768) [RMS: 62.81]
        Cfg::new(Dist::Exp, 0.892563999f32, 1.11905622f32, 0.0f32, 2.21876144f32, 9.63837719f32),
        // PT  error  612 (102/32768) [RMS: 43.71]
        Cfg::new(Dist::Linear, 1.01262534f32, 1.0f32, 2.46070528f32, 0.0537485816f32, 0.0986242667f32),
        // PS  error 8135 (575/32768) [RMS: 75.10]
        Cfg::new(Dist::Linear, 2.14896345f32, 1.0216713f32, 10.5400085f32, 0.244498149f32, 0.126134038f32),
        // PTS error 2489 (60/32768) [RMS: 24.41]
        Cfg::new(Dist::Linear, 1.22330308f32, 0.933797896f32, 2.83245254f32, 0.0615176819f32, 0.323831677f32),
        // NP  guessed
        Cfg::new(Dist::Exp, 0.96f32, 1.0f32, 2.5f32, 1.1f32, 1.2f32),
    ],
    [
        // 8580 R5 4887 sampled by reFX-Mike
        // TS  error  741 (76/32768) [RMS: 53.74]
        Cfg::new(Dist::Exp, 0.812351167f32, 1.1727736f32, 0.0f32, 1.87459648f32, 2.31578159f32),
        // PT  error 7199 (192/32768) [RMS: 88.43]
        Cfg::new(Dist::Exp, 0.917997837f32, 1.0f32, 1.01248944f32, 1.05761552f32, 1.37529826f32),
        // PS  error 9856 (332/32768) [RMS: 86.29]
        Cfg::new(Dist::Quadratic, 0.968754232f32, 1.00669801f32, 1.29909098f32, 0.00962483883f32, 0.146850556f32),
        // PTS error 4809 (60/32768) [RMS: 45.37]
        Cfg::new(Dist::Exp, 0.941834152f32, 1.06401193f32, 0.991132736f32, 0.995310068f32, 1.41105855f32),
        // NP  guessed
        Cfg::new(Dist::Exp, 0.95f32, 1.0f32, 1.15f32, 1.0f32, 1.45f32),
    ],
];

static CONFIG_STRONG: [[Cfg; 5]; 2] = [
    [
        // 6581 R2 0384 sampled by Trurl
        // TS  error 20337 (1579/32768) [RMS: 88.57]
        Cfg::new(Dist::Exp, 0.000637792516f32, 1.56725872f32, 0.0f32, 0.00036806846f32, 1.51800942f32),
        // PT  error  5190 (238/32768) [RMS: 83.54]
        Cfg::new(Dist::Linear, 0.924780309f32, 1.0f32, 1.96809769f32, 0.0888123438f32, 0.234606609f32),
        // PS  error 31015 (2181/32768) [RMS: 114.99]
        Cfg::new(Dist::Linear, 1.2328074f32, 0.73079139f32, 3.9719491f32, 0.00156516861f32, 0.314677745f32),
        // PTS error  9874 (201/32768) [RMS: 52.30]
        Cfg::new(Dist::Linear, 1.08558261f32, 0.857638359f32, 1.52781796f32, 0.152927235f32, 1.02657032f32),
        // NP  guessed
        Cfg::new(Dist::Exp, 0.96f32, 1.0f32, 2.5f32, 1.1f32, 1.2f32),
    ],
    [
        // 8580 R5 1489 sampled by reFX-Mike
        // TS  error  4837 (388/32768) [RMS: 76.07]
        Cfg::new(Dist::Exp, 0.89762634f32, 56.7594185f32, 0.0f32, 7.68995237f32, 12.0754194f32),
        // PT  error  9266 (508/32768) [RMS: 127.83]
        Cfg::new(Dist::Exp, 0.87147671f32, 1.0f32, 1.44887495f32, 1.05899632f32, 1.43786001f32),
        // PS  error 13168 (718/32768) [RMS: 123.35]
        Cfg::new(Dist::Quadratic, 0.89255774f32, 1.2253896f32, 1.75615835f32, 0.0245045591f32, 0.12982437f32),
        // PTS error  6702 (300/32768) [RMS: 71.01]
        Cfg::new(Dist::Linear, 0.91124934f32, 0.963609755f32, 0.909965038f32, 1.07445884f32, 1.82399702f32),
        // NP  guessed
        Cfg::new(Dist::Exp, 0.95f32, 1.0f32, 1.15f32, 1.0f32, 1.45f32),
    ],
];

static WAVE_TABLES: OnceLock<Box<WaveTables>> = OnceLock::new();

/// Index: chip model (6581, 8580) * 3 + combined waveforms strength
/// (average, weak, strong)
static PULLDOWN_TABLES: [OnceLock<Box<PulldownTables>>; 6] = [const { OnceLock::new() }; 6];

/// Get the waveform table for use by WaveformGenerator
pub fn get_wave_table() -> &'static WaveTables {
    WAVE_TABLES.get_or_init(|| {
        let mut wf_table: Box<WaveTables> = Box::new([[0; 4096]; 4]);

        // Build waveform table
        for idx in 0..4096u32 {
            let saw = idx as i16;
            let tri = tri_xor(idx) as i16;

            wf_table[0][idx as usize] = 0xfff;
            wf_table[1][idx as usize] = tri;
            wf_table[2][idx as usize] = saw;
            wf_table[3][idx as usize] = saw & (saw << 1);
        }

        wf_table
    })
}

/// Build pulldown table for use by WaveformGenerator.
///
/// Upstream returns the waveform table (instead of the pulldown table) from the
/// call that builds a table, and only later calls get the cached pulldown
/// table; this returns the pulldown table every time, as libsidplayfp does
pub fn build_pulldown_table(model: ChipModel, cws: CombinedWaveforms) -> &'static PulldownTables {
    let model_idx = if model == ChipModel::Mos6581 { 0 } else { 1 };
    let cws_idx = match cws {
        CombinedWaveforms::Average => 0,
        CombinedWaveforms::Weak => 1,
        CombinedWaveforms::Strong => 2,
    };

    PULLDOWN_TABLES[model_idx * 3 + cws_idx].get_or_init(|| {
        let cfg_array = match cws {
            CombinedWaveforms::Average => &CONFIG_AVERAGE[model_idx],
            CombinedWaveforms::Weak => &CONFIG_WEAK[model_idx],
            CombinedWaveforms::Strong => &CONFIG_STRONG[model_idx],
        };

        let mut pd_table: Box<PulldownTables> = Box::new([[0; 4096]; 5]);

        for (wav, cfg) in cfg_array.iter().enumerate() {
            let mut distance_table = [0.0f32; 12 * 2 + 1];
            distance_table[12] = 1.0;

            for i in (1..=12).rev() {
                distance_table[12 - i] = distance(cfg.dist_func, cfg.distance1, i as i32);
                distance_table[12 + i] = distance(cfg.dist_func, cfg.distance2, i as i32);
            }

            for idx in 0..4096u32 {
                pd_table[wav][idx as usize] =
                    calculate_pulldown(&distance_table, cfg.top_bit, cfg.pulse_strength, cfg.threshold, idx);
            }
        }

        pd_table
    })
}

// Distance methods

fn distance(dist: Dist, distance: f32, i: i32) -> f32 {
    match dist {
        Dist::Exp => exponential_distance(distance, i),
        Dist::Linear => linear_distance(distance, i),
        Dist::Quadratic => quadratic_distance(distance, i),
    }
}

fn exponential_distance(distance: f32, i: i32) -> f32 {
    fmath::powi(f64::from(distance), -i) as f32
}

fn linear_distance(distance: f32, i: i32) -> f32 {
    1.0f32 / (1.0f32 + i as f32 * distance)
}

fn quadratic_distance(distance: f32, i: i32) -> f32 {
    1.0f32 / (1.0f32 + (i * i) as f32 * distance)
}

/// Calculate triangle waveform
fn tri_xor(val: u32) -> u32 {
    (if (val & 0x800) == 0 { val } else { val ^ 0xfff }) << 1
}

/// Generate bitstate based on emulation of combined waves pulldown
fn calculate_pulldown(
    distance_table: &[f32; 25],
    top_bit: f32,
    pulse_strength: f32,
    threshold: f32,
    accumulator: u32,
) -> i16 {
    let mut bit = [0.0f32; 12];

    for (i, b) in bit.iter_mut().enumerate() {
        *b = if (accumulator & (1u32 << i)) != 0 { 1.0 } else { 0.0 };
    }

    bit[11] *= top_bit;

    let mut pulldown = [0.0f32; 12];

    for sb in 0..12usize {
        let mut avg = 0.0f32;
        let mut n = 0.0f32;

        for cb in 0..12usize {
            if cb == sb {
                continue;
            }

            let weight = distance_table[sb + 12 - cb];
            avg += (1.0f32 - bit[cb]) * weight;
            n += weight;
        }

        avg -= pulse_strength;

        pulldown[sb] = avg / n;
    }

    // Get the predicted value
    let mut value: i16 = 0;

    for i in 0..12 {
        let bit_value = if bit[i] > 0.0 { 1.0f32 - pulldown[i] } else { 0.0f32 };
        if bit_value > threshold {
            value |= (1u32 << i) as i16;
        }
    }

    value
}
