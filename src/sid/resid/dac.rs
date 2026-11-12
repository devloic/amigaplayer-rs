// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/Dac.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::ChipModel;

// Estimate DAC nonlinearity.
// The SID DACs are built up as R-2R ladder as follows:
//
//         n  n-1      2   1   0    VGND
//         |   |       |   |   |      |   Termination
//        2R  2R      2R  2R  2R     2R   only for
//         |   |       |   |   |      |   MOS 8580
//     Vo -o-R-o-R-...-o-R-o-R--    --+
//
//
// All MOS 6581 DACs are missing a termination resistor at bit 0. This causes
// pronounced errors for the lower 4 - 5 bits (e.g. the output for bit 0 is
// actually equal to the output for bit 1), resulting in DAC discontinuities
// for the lower bits.
// In addition to this, the 6581 DACs exhibit further severe discontinuities
// for higher bits, which may be explained by a less than perfect match between
// the R and 2R resistors, or by output impedance in the NMOS transistors
// providing the bit voltages. A good approximation of the actual DAC output is
// achieved for 2R/R ~ 2.20.
// NOTE: from the schematics the 2R/R ratio seems pretty accurate.
//
// The MOS 8580 DACs, on the other hand, do not exhibit any discontinuities.
// These DACs include the correct termination resistor, and also seem to have
// very accurately matched R and 2R resistors (2R/R = 2.00).
//
// On the 6581 the output of the waveform and envelope DACs go through
// a voltage follower built with two NMOS:
//
//             Vdd
//
//              |
//            |-+
// Vin -------|    T1 (enhancement-mode)
//            |-+
//              |
//              o-------- Vout
//              |
//            |-+
//        +---|    T2 (depletion-mode)
//        |   |-+
//        |     |
//
//       GND   GND

const R_INFINITY: f64 = 1e6;

const MOSFET_LEAKAGE_6581: f64 = 0.0075;
const MOSFET_LEAKAGE_8580: f64 = 0.0035;

pub struct Dac {
    /// DAC leakage
    ///
    /// "Even in standard transistors a small amount of current leaks even when they are technically switched off."
    leakage: f64,

    /// Analog values
    dac: Vec<f64>,
}

impl Dac {
    pub fn new(bits: u32) -> Self {
        Dac {
            leakage: 0.0,
            dac: vec![0.0; bits as usize],
        }
    }

    /// Build DAC model for specific chip
    pub fn kinked_dac(&mut self, chip_model: ChipModel) {
        let dac_length = self.dac.len() as u32;

        // Non-linearity parameter, 8580 DACs are perfectly linear
        let two_r_div_r = if chip_model == ChipModel::Mos6581 { 2.20 } else { 2.00 };

        // 6581 DACs are not terminated by a 2R resistor
        let term = chip_model == ChipModel::Csg8580;

        self.leakage = if chip_model == ChipModel::Mos6581 {
            MOSFET_LEAKAGE_6581
        } else {
            MOSFET_LEAKAGE_8580
        };

        let mut v_sum = 0.0;

        // Calculate voltage contribution by each individual bit in the R-2R ladder
        for set_bit in 0..dac_length {
            let mut vn = 1.0; // Normalized bit voltage
            let r = 1.0; // Normalized R
            let two_r = two_r_div_r * r; // 2R
            let mut rn = if term {
                two_r // Rn = 2R for correct termination,
            } else {
                R_INFINITY // INFINITY for missing termination
            };

            let mut bit = 0;

            // Calculate DAC "tail" resistance by repeated parallel substitution
            while bit < set_bit {
                rn = if rn == R_INFINITY {
                    r + two_r
                } else {
                    r + (two_r * rn) / (two_r + rn) // R + 2R || Rn
                };
                bit += 1;
            }

            // Source transformation for bit voltage
            if rn == R_INFINITY {
                rn = two_r;
            } else {
                rn = (two_r * rn) / (two_r + rn); // 2R || Rn
                vn = vn * rn / two_r;
            }

            // Calculate DAC output voltage by repeated source transformation from
            // the "tail"
            bit += 1;
            while bit < dac_length {
                rn += r;
                let i = vn / rn;

                rn = (two_r * rn) / (two_r + rn); // 2R || Rn
                vn = rn * i;
                bit += 1;
            }

            self.dac[set_bit as usize] = vn;
            v_sum += vn;
        }

        // Normalize to integerish behaviour
        for d in self.dac.iter_mut() {
            *d /= v_sum;
        }
    }

    /// Get the Vo output for a given combination of input bits
    pub fn get_output(&self, input: u32, saturate: bool) -> f64 {
        let mut dac_value = 0.0;

        for (i, &d) in self.dac.iter().enumerate() {
            let transistor_on = (input & (1u32 << i)) != 0;
            dac_value += if transistor_on { d } else { d * self.leakage };
        }

        // Rough attempt at modeling the MDAC saturation for the 6581.
        // Things are actually more complex, the saturation is likely
        // caused by the two NMOS source followers, one at the output
        // of the waveform DAC and the second at the output of the MDAC.
        // The buffers are also supposed to introduce a DC offset.
        // As a first step we use a cubic model for saturation and
        // apply it only at the waveform output, providing a decent
        // result without any runtime overhead
        if saturate {
            const GAIN: f64 = 1.1;
            const SAT: f64 = 1.1;
            dac_value = GAIN * dac_value + (1.0 - GAIN) * SAT * dac_value * dac_value * dac_value;
        }

        dac_value
    }
}
