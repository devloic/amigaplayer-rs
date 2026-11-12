// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/Filter8580.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::filter_model_config::Dither;
use super::filter_model_config8580::FilterModelConfig8580;
use super::integrator8580::Integrator8580;

// MOS8580 filter emulation

// Filter for 8580 chip
// --------------------
// The 8580 filter stage had been redesigned to be more linear and robust
// against temperature change. It also features real op-amps and a
// revisited resonance model.
// The filter schematics below are reverse engineered from re-vectorized
// and annotated die photographs. Credits to Michael Huth for the microscope
// photographs of the die, Tommi Lempinen for re-vectorizating and annotating
// the images and ttlworks from forum.6502.org for the circuit analysis.
//
// ~~~
//
//               +---------------------------------------------------+
//               |    $17      +----Rf-+                             |
//               |             |       |                             |
//               |      D4&!D5 o- \-R3-o                             |
//               |             |       |                    $17      |
//               |     !D4&!D5 o- \-R2-o                             |
//               |             |       |  +---R8-- \--+  !D6&D7      |
//               |      D4&!D5 o- \-R1-o  |           |              |
//               |             |       |  o---RC-- \--o   D6&D7      |
//               |   +---------o--<A]--o--o           |              |
//               |   |                    o---R4-- \--o  D6&!D7      |
//               |   |                    |           |              |
//               |   |                    +---Ri-- \--o !D6&!D7      |
//               |   |                                |              |
// $17           |   |                    (CAP2B)     |  (CAP1B)     |
// 0=to mixer    |   +--R7--+  +---R7--+      +---C---o      +---C---o
// 1=to filter   |          |  |       |      |       |      |       |
//               +------R7--o--o--[A>--o--Rfc-o--[A>--o--Rfc-o--[A>--o
//     ve (EXT IN)          |          |              |              |
// D3  \ --------------R12--o          |              | (CAP2A)      | (CAP1A)
//     |   v3               |          | vhp          | vbp          | vlp
// D2  |   \ -----------R7--o    +-----+              |              |
//     |   |   v2           |    |                    |              |
// D1  |   |   \ -------R7--o    |   +----------------+              |
//     |   |   |   v1       |    |   |                               |
// D0  |   |   |   \ ---R7--+    |   |   +---------------------------+
//     |   |   |   |             |   |   |
//     R9  R5  R5  R5            R5  R5  R5
//     |   |   |   | $18         |   |   |  $18
//     |    \  |   | D7: 1=open   \   \   \ D6 - D4: 0=open
//     |   |   |   |             |   |   |
//     +---o---o---o-------------o---o---+
//                 |
//                 |               D3 +--/ --1R4--+
//                 |   +---R8--+      |           |  +---R2--+
//                 |   |       |   D2 o--/ --2R4--o  |       |
//                 +---o--[A>--o------o           o--o--[A>--o-- vo (AUDIO OUT)
//                                 D1 o--/ --4R4--o
//                        $18         |           |
//                        0=open   D0 +--/ --8R4--+
//
//
//
// Resonance
// ---------
// For resonance, we have two tiny DACs that controls both the input
// and feedback resistances.
//
// The "resistors" are switched in as follows by bits in register $17:
//
// feedback:
// R1: bit4&!bit5
// R2: !bit4&bit5
// R3: bit4&bit5
// Rf: always on
//
// input:
// R4: bit6&!bit7
// R8: !bit6&bit7
// RC: bit6&bit7
// Ri: !(R4|R8|RC) = !(bit6|bit7) = !bit6&!bit7
//
//
// The relative "resistor" values are approximately (using channel length):
//
// R1 = 15.3*Ri
// R2 =  7.3*Ri
// R3 =  4.7*Ri
// Rf =  1.4*Ri
// R4 =  1.4*Ri
// R8 =  2.0*Ri
// RC =  2.8*Ri
//
//
// Approximate values for 1/Q can now be found as follows (assuming an
// ideal op-amp):
//
// res  feedback  input  -gain (1/Q)
// ---  --------  -----  ----------
//  0   Rf        Ri     Rf/Ri      = 1/(Ri*(1/Rf))      = 1/0.71
//  1   Rf|R1     Ri     (Rf|R1)/Ri = 1/(Ri*(1/Rf+1/R1)) = 1/0.78
//  2   Rf|R2     Ri     (Rf|R2)/Ri = 1/(Ri*(1/Rf+1/R2)) = 1/0.85
//  3   Rf|R3     Ri     (Rf|R3)/Ri = 1/(Ri*(1/Rf+1/R3)) = 1/0.92
//  4   Rf        R4     Rf/R4      = 1/(R4*(1/Rf))      = 1/1.00
//  5   Rf|R1     R4     (Rf|R1)/R4 = 1/(R4*(1/Rf+1/R1)) = 1/1.10
//  6   Rf|R2     R4     (Rf|R2)/R4 = 1/(R4*(1/Rf+1/R2)) = 1/1.20
//  7   Rf|R3     R4     (Rf|R3)/R4 = 1/(R4*(1/Rf+1/R3)) = 1/1.30
//  8   Rf        R8     Rf/R8      = 1/(R8*(1/Rf))      = 1/1.43
//  9   Rf|R1     R8     (Rf|R1)/R8 = 1/(R8*(1/Rf+1/R1)) = 1/1.56
//  A   Rf|R2     R8     (Rf|R2)/R8 = 1/(R8*(1/Rf+1/R2)) = 1/1.70
//  B   Rf|R3     R8     (Rf|R3)/R8 = 1/(R8*(1/Rf+1/R3)) = 1/1.86
//  C   Rf        RC     Rf/RC      = 1/(RC*(1/Rf))      = 1/2.00
//  D   Rf|R1     RC     (Rf|R1)/RC = 1/(RC*(1/Rf+1/R1)) = 1/2.18
//  E   Rf|R2     RC     (Rf|R2)/RC = 1/(RC*(1/Rf+1/R2)) = 1/2.38
//  F   Rf|R3     RC     (Rf|R3)/RC = 1/(RC*(1/Rf+1/R3)) = 1/2.60
//
//
// These data indicate that the following function for 1/Q has been
// modeled in the MOS 8580:
//
//    1/Q = 2^(1/2)*2^(-x/8) = 2^(1/2 - x/8) = 2^((4 - x)/8)
//
//
//
// Op-amps
// -------
// Unlike the 6581, the 8580 has real OpAmps.
//
// Temperature compensated differential amplifier:
//
//                9V
//
//                |
//      +-------o-o-o-------+
//      |       |   |       |
//      |       R   R       |
//      +--||   |   |   ||--+
//         ||---o   o---||
//      +--||   |   |   ||--+
//      |       |   |       |
//      o-----+ |   |       o--- Va
//      |     | |   |       |
//      +--|| | |   |   ||--+
//         ||-o-+---+---||
//      +--||   |   |   ||--+
//      |       |   |       |
//              |   |
//     GND      |   |      GND
//          ||--+   +--||
// in- -----||         ||------ in+
//          ||----o----||
//                |
//                8 Current sink
//                |
//
//               GND
//
// Inverter + non-inverting output amplifier:
//
// Va ---o---||-------------------o--------------------+
//       |                        |               9V   |
//       |             +----------+----------+     |   |
//       |        9V   |          |     9V   | ||--+   |
//       |         |   |      9V  |      |   +-||      |
//       |         R   |       |  |  ||--+     ||--+   |
//       |         |   |   ||--+  +--||            o---o--- Vout
//       |         o---o---||        ||--+     ||--+
//       |         |       ||--+         o-----||
//       |     ||--+           |     ||--+     ||--+
//       +-----||              o-----||            |
//             ||--+           |     ||--+
//                 |           R         |        GND
//                             |
//                GND                   GND
//                            GND
//
//
//
// Virtual ground
// --------------
// A PolySi resitive voltage divider provides the voltage
// for the positive input of the filter op-amps.
//
//     5V
//          +----------+
//      |   |   |\     |
//      R1  +---|-\    |
// 5V   |       |A >---o--- Vref
//      o-------|+/
//  |   |       |/
// R10  R4
//  |   |
//  o---+
//  |
// R10
//  |
//
// GND
//
// Rn = n*R1
//
//
//
// Rfc - freq control DAC resistance ladder
// ----------------------------------------
// The 8580 has 11 bits for frequency control, but 12 bit DACs.
// If those 11 bits would be '0', the impedance of the DACs would be "infinitely high".
// To get around this, there is an 11 input NOR gate below the DACs sensing those 11 bits.
// If all are 0, the NOR gate gives the gate control voltage to the 12 bit DAC LSB.
//
//     ----o---o--...--o---o---o---
//         |   |       |   |   |
//       Rb10 Rb9 ... Rb1 Rb0  R0
//         |   |       |   |   |
//     ----o---o--...--o---o---o---
//
//
//
// Crystal stabilized precision switched capacitor voltage divider
// ---------------------------------------------------------------
// There is a FET working as a temperature sensor close to the DACs which changes the gate voltage
// of the frequency control DACs according to the temperature of the DACs,
// to reduce the effects of temperature on the filter curve.
// An asynchronous 3 bit binary counter, running at the speed of PHI2, drives two big capacitors
// whose AC resistance is then used as a voltage divider.
// This implicates that frequency difference between PAL and NTSC might shift the filter curve by 4% or such.
//
//                                |\  OpAmp has a smaller capacitor than the other OPs
//                        Vref ---|+\
//                                |A >---o--- Vdac
//                        +-------|-/    |
//                        |       |/     |
//                        |              |
//       C1               |     C2       |
//   +---||---o---+   +---o-----||-------o
//   |        |   |   |   |              |
//   o----+   |   -----   |              |
//   |    |   |   -----   +----+   +-----o
//   |    -----     |          |   |     |
//   |    -----     |          -----     |
//   |      |       |          -----     |
//   |    +-----------+          |       |
//        | /Q      Q |          +-------+
//  GND   +-----------+      FET close to DAC
//        |   clk/8   |      working as temperature sensor
//        +-----------+

/// W/L ratio of frequency DAC bit 0,
/// other bit are proportional.
/// When no bit are selected a resistance with half
/// W/L ratio is selected
const DAC_WL0: f64 = 0.00615;

pub struct Filter8580 {
    pub fmc: &'static FilterModelConfig8580,

    /// VCR + associated capacitor connected to highpass output
    pub hp_integrator: Integrator8580,

    /// VCR + associated capacitor connected to bandpass output
    pub bp_integrator: Integrator8580,
}

impl Filter8580 {
    pub fn new(dither: &mut Dither) -> Self {
        let fmc = FilterModelConfig8580::get_instance();
        let hp_integrator = Integrator8580::new(fmc, dither);
        let bp_integrator = Integrator8580::new(fmc, dither);

        let mut this = Filter8580 {
            fmc,
            hp_integrator,
            bp_integrator,
        };

        this.set_filter_curve(0.5, dither);

        this
    }

    /// Set filter curve type based on single parameter.
    ///
    /// curvePosition 0 .. 1, where 0 sets center frequency high ("light")
    /// and 1 sets it low ("dark"), default is 0.5
    pub fn set_filter_curve(&mut self, curve_position: f64, dither: &mut Dither) {
        // Adjust cp
        // 1.2 <= cp <= 1.8
        let cp = 1.8 - curve_position * 3.0 / 5.0;

        self.hp_integrator.set_v(self.fmc, cp, dither);
        self.bp_integrator.set_v(self.fmc, cp, dither);
    }

    pub fn restart_integrators(&mut self) {
        self.hp_integrator.restart();
        self.bp_integrator.restart();
    }

    /// Set filter cutoff frequency
    pub fn update_center_frequency(&mut self, fc: u16) {
        let fc = u32::from(fc);
        let mut dac_wl = DAC_WL0;

        let wl = if fc != 0 {
            let mut sum = 0.0;

            for i in 0..11 {
                if (fc & (1 << i)) != 0 {
                    sum += dac_wl;
                }

                dac_wl *= 2.0;
            }

            sum
        } else {
            dac_wl / 2.0
        };

        self.hp_integrator.set_fc(self.fmc, wl);
        self.bp_integrator.set_fc(self.fmc, wl);
    }

    /// Both integrators of the filter loop: returns (vbp, vlp)
    #[inline(always)]
    pub fn solve(&mut self, vhp: i32) -> (i32, i32) {
        let vbp = self.hp_integrator.solve(vhp, self.fmc);
        let vlp = self.bp_integrator.solve(vbp, self.fmc);

        (vbp, vlp)
    }
}
