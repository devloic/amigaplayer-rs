// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/ExternalFilter.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

// The audio output stage in a Commodore 64 consists of two STC networks, a
// low-pass RC filter with 3 dB frequency 16kHz followed by a DC-blocker which
// acts as a high-pass filter with a cutoff dependent on the attached audio
// equipment impedance. Here we suppose an impedance of 10kOhm resulting
// in a 3 dB attenuation at 1.6Hz.
// The STC networks are connected with a [BJT] based [common collector]
// used as a voltage follower (featuring a 2SC1815 NPN transistor).
//
// To operate properly the 6581 audio output needs a pull-down resistor,
// 1KOhm is the recommended value.
//
// ~~~
//                                 9/12V
// -----+
// audio|       10k                  |
//      +---o----R---o--------o-----(K)          +-----
//  out |   |        |        |      |           |audio
// -----+   R 1k     C 1000   |      |    10 uF  |
//          |        |  pF    +-C----o-----C-----+ 10k
//                             470   |           |
//         GND      GND         pF   R 1K        | amp
//          *                   **   |           +-----
//
//                                  GND
// ~~~
//
// * Only for the 6581.
// ** The C64c board additionally includes a [bootstrap] capacitor to increase
//    the input impedance of the common collector.
//
// The C128 boards include an additional capacitor and a resistor
// to the ouptput stage.
//
//  ~~~
//                                  9/12V
//  -----+
//  audio|       10k                  |
//       +---o----R---o--------o-----(K)                    +-----
//   out |   |        |        |      |                     |audio
//  -----+   R 1k     C 1000   |      |        10 uF        |
//           |        |  pF    +-C----o-----o----C---o------+ ~10k
//                              470   |     |        |      |
//          GND      GND         pF   R 1K  C 220pF  R 47K  | amp
//           *                        |     |        |      +-----
//
//                                   GND   GND      GND
//  ~~~
//
// [BJT]: https://en.wikipedia.org/wiki/Bipolar_junction_transistor
// [common collector]: https://en.wikipedia.org/wiki/Common_collector
// [bootstrap]: https://en.wikipedia.org/wiki/Bootstrapping_(electronics)

pub struct ExternalFilter {
    /// Lowpass filter voltage
    vlp: i32,

    /// Highpass filter voltage
    vhp: i32,

    w0lp_1_s7: i32,

    w0hp_1_s17: i32,
}

impl ExternalFilter {
    pub fn new() -> Self {
        let mut this = ExternalFilter {
            vlp: 0,
            vhp: 0,
            w0lp_1_s7: 0,
            w0hp_1_s17: 0,
        };

        this.reset();

        this
    }

    /// Setup of the external filter sampling parameters
    pub fn set_clock_frequency(&mut self, frequency: f64) {
        let dt = 1.0 / frequency;

        // Low-pass:  R = 10kOhm, C = 1000pF; w0l = dt/(dt+RC) = 1e-6/(1e-6+1e4*1e-9) = 0.091
        // Cutoff 1/2*PI*RC = 1/2*PI*1e4*1e-9 = 15915.5 Hz
        self.w0lp_1_s7 = (((dt / (dt + get_rc(10e3, 1000e-12))) * f64::from(1 << 7)) + 0.5) as i32;

        // High-pass: R = 10kOhm, C = 10uF;   w0h = dt/(dt+RC) = 1e-6/(1e-6+1e4*1e-5) = 0.00000999
        // Cutoff 1/2*PI*RC = 1/2*PI*1e4*1e-5 = 1.59155 Hz
        self.w0hp_1_s17 = (((dt / (dt + get_rc(10e3, 10e-6))) * f64::from(1 << 17)) + 0.5) as i32;
    }

    /// SID reset
    pub fn reset(&mut self) {
        // State of filter
        self.vlp = 0; // 1 << (15 + 11)
        self.vhp = 0;
    }

    /// SID clocking
    #[inline(always)]
    pub fn clock(&mut self, input: i32) -> i32 {
        let vi = input.wrapping_shl(11);
        let d_vlp = self.w0lp_1_s7.wrapping_mul(vi.wrapping_sub(self.vlp)) >> 7;
        let d_vhp = self.w0hp_1_s17.wrapping_mul(self.vlp.wrapping_sub(self.vhp)) >> 17;

        self.vlp = self.vlp.wrapping_add(d_vlp);
        self.vhp = self.vhp.wrapping_add(d_vhp);

        self.vlp.wrapping_sub(self.vhp) >> 11
    }
}

/// Get the 3 dB attenuation point
#[inline(always)]
fn get_rc(res: f64, cap: f64) -> f64 {
    res * cap
}
