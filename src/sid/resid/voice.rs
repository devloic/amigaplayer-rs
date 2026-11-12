// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/Voice.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::envelope_generator::EnvelopeGenerator;
use super::waveform_calculator::{PulldownTables, WaveTables};
use super::waveform_generator::WaveformGenerator;

// Representation of SID voice block.
//
// The three voices live in an array in `Sid`; the modulator (previous) voice
// of the C# `SetOtherVoices` is passed to `output` as its accumulator, and
// the sync between voices is done by `Sid`.

pub struct Voice {
    pub wave: WaveformGenerator,
    pub envelope: EnvelopeGenerator,
}

impl Voice {
    pub fn new(model_wave: &'static WaveTables, model_pulldown: &'static PulldownTables, is6581: bool) -> Self {
        Voice {
            wave: WaveformGenerator::new(model_wave, model_pulldown, is6581),
            envelope: EnvelopeGenerator::new(),
        }
    }

    /// Amplitude modulated waveform output.
    ///
    /// The waveform DAC generates a voltage between virtual ground and
    /// Vdd (5-12 V for the 6581 and 4,75-9 V for the 8580) corresponding
    /// to oscillator state 0 .. 4095.
    ///
    /// The envelope DAC generates a voltage between waveform gen output
    /// and the virtual ground level, corresponding to envelope state
    /// 0 .. 255.
    ///
    /// Ideal range [-2048*255, 2047*255]
    ///
    /// `wav_dac` is the DAC LUT for analog waveform output and `env_dac`
    /// the one for analog envelope output
    #[inline(always)]
    pub fn output(&mut self, prev_accumulator: u32, wav_dac: &[f32; 4096], env_dac: &[f32; 256]) -> f32 {
        let wav = self.wave.output(prev_accumulator);
        let env = self.envelope.output();

        // DAC imperfections are emulated by using the digital output
        // as an index into a DAC lookup table
        wav_dac[(wav & 0xfff) as usize] * env_dac[usize::from(env)]
    }

    /// Write control register
    pub fn write_control_reg(&mut self, control: u8) {
        self.wave.write_control_reg(control);
        self.envelope.write_control_reg(control);
    }

    /// SID reset
    pub fn reset(&mut self) {
        self.wave.reset();
        self.envelope.reset();
    }
}
