// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/ (36 files),
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

//! MOS 6581 / CSG 8580 SID chip emulation (reSIDfp).
//!
//! ```ignore
//! let mut sid = Sid::new(); // an 8580
//! sid.set_chip_model(ChipModel::Mos6581);
//! sid.set_sampling_parameters(985248.0, SamplingMethod::Resample, 48000.0)?;
//! sid.write(0x18, 0x0f); // volume 15
//! let mut buf = [0i16; 1024];
//! let n = sid.clock(20000, &mut buf); // n samples written
//! ```
//!
//! The 6581 and 8580 lookup tables (a few million op-amp solutions each) are
//! built the first time a chip model is selected, in one thread, and shared by
//! all `Sid`s afterwards. Everything is deterministic: the same register
//! script gives the same samples on every platform.
//!
//! Changes from upstream, besides the ones forced by the language:
//!
//! - no `std` transcendental function is used (see `fmath`), the dither noise
//!   of the lookup tables and of the filter is a fixed-seed sequence instead of
//!   the shared .NET random generator;
//! - upstream's `WaveformCalculator.BuildPulldownTable` returns the waveform
//!   table instead of the pulldown table when it builds a table (it is correct
//!   only when the table is cached); the pulldown table is returned every time;
//! - the filter range and old caps settings are per `Sid` (upstream keeps them
//!   in a singleton shared by all 6581 filters);
//! - the filter of a chip model is built when the model is first selected.

#![allow(dead_code)]
#![allow(clippy::excessive_precision)]

mod dac;
mod envelope_generator;
mod external_filter;
mod filter;
mod filter6581;
mod filter8580;
mod filter_model_config;
mod filter_model_config6581;
mod filter_model_config8580;
mod fmath;
mod integrator6581;
mod integrator8580;
mod op_amp;
mod resample;
mod sid;
mod spline;
mod voice;
mod waveform_calculator;
mod waveform_generator;

pub use sid::Sid;

/// Supported SID chips
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipModel {
    /// Old MOS6581
    Mos6581,

    /// New CSG8580
    Csg8580,
}

/// Strength of the combined waveforms
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombinedWaveforms {
    Average,
    Weak,
    Strong,
}

/// Different methods to generate the samples
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplingMethod {
    /// Linear interpolation (fast but low quality)
    Decimate,

    /// Sinc resampling (high quality but CPU intensive)
    Resample,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: f64 = 985248.0;
    const RATE: f64 = 48000.0;

    fn new_sid(model: ChipModel, method: SamplingMethod) -> Sid {
        let mut sid = Sid::new();
        sid.set_chip_model(model);
        sid.set_sampling_parameters(CLOCK, method, RATE).unwrap();
        sid.reset();
        sid
    }

    fn set_freq(sid: &mut Sid, voice: u8, reg: u32) {
        sid.write(voice * 7, (reg & 0xff) as u8);
        sid.write(voice * 7 + 1, (reg >> 8) as u8);
    }

    fn freq_reg(hz: f64) -> u32 {
        (hz * 16777216.0 / CLOCK) as u32
    }

    /// Voice 1: sawtooth, AD 0, SR $F0, volume 15
    fn start_saw(sid: &mut Sid, hz: f64) {
        set_freq(sid, 0, freq_reg(hz));
        sid.write(0x05, 0x00);
        sid.write(0x06, 0xf0);
        sid.write(0x18, 0x0f);
        sid.write(0x04, 0x21);
    }

    fn render(sid: &mut Sid, seconds: f64) -> Vec<i16> {
        let cycles = (CLOCK * seconds) as u32;
        let mut buf = vec![0i16; (RATE * seconds) as usize + 16];
        let n = sid.clock(cycles, &mut buf);
        buf.truncate(n);
        buf
    }

    fn rms(s: &[i16]) -> f64 {
        (s.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / s.len() as f64).sqrt()
    }

    /// Upward zero crossings per second
    fn crossing_rate(s: &[i16]) -> f64 {
        let mut n = 0;
        for w in s.windows(2) {
            if w[0] < 0 && w[1] >= 0 {
                n += 1;
            }
        }
        f64::from(n) * RATE / s.len() as f64
    }

    fn check_saw(model: ChipModel, method: SamplingMethod) {
        let mut sid = new_sid(model, method);
        start_saw(&mut sid, 440.0);

        let out = render(&mut sid, 1.0);
        // The resamplers turn cycles per sample into an integer: 0.1% more samples than 48000
        assert!((47999..=48050).contains(&out.len()), "{} samples", out.len());
        assert!(rms(&out) > 500.0, "silent: rms {}", rms(&out));

        // Skip the first 100 ms (envelope attack, filter settling)
        let tail = &out[4800..];
        let f = crossing_rate(tail);
        assert!((f - 440.0).abs() < 4.4, "{model:?} {method:?}: {f} Hz");
    }

    #[test]
    fn sawtooth_440hz() {
        for model in [ChipModel::Mos6581, ChipModel::Csg8580] {
            for method in [SamplingMethod::Resample, SamplingMethod::Decimate] {
                check_saw(model, method);
            }
        }
    }

    /// RMS of the first difference: the part of the signal that is not a
    /// (slowly settling) DC level
    fn ac_rms(s: &[i16]) -> f64 {
        let d: Vec<i16> = s.windows(2).map(|w| w[1].wrapping_sub(w[0])).collect();
        rms(&d)
    }

    #[test]
    fn release_goes_silent() {
        for model in [ChipModel::Mos6581, ChipModel::Csg8580] {
            let mut sid = new_sid(model, SamplingMethod::Resample);
            start_saw(&mut sid, 440.0);
            let on = render(&mut sid, 0.3);
            assert!(ac_rms(&on[4800..]) > 100.0);

            sid.write(0x04, 0x20); // gate off
            let _ = render(&mut sid, 0.2);
            let off = render(&mut sid, 0.3);
            assert!(ac_rms(&off) < 5.0, "{model:?}: ac rms after release {}", ac_rms(&off));
        }
    }

    #[test]
    fn lowpass_attenuates_high_note() {
        for model in [ChipModel::Mos6581, ChipModel::Csg8580] {
            let run = |filter: bool| {
                let mut sid = new_sid(model, SamplingMethod::Resample);
                sid.enable_filter(filter);
                sid.write(0x15, 0x00);
                sid.write(0x16, 0x10); // low cutoff
                sid.write(0x17, 0x01); // voice 1 through the filter, no resonance
                start_saw(&mut sid, 10000.0);
                sid.write(0x18, 0x1f); // low-pass, volume 15
                let out = render(&mut sid, 0.6);
                ac_rms(&out[14400..])
            };

            let off = run(false);
            let on = run(true);
            assert!(off > 300.0, "{model:?}: unfiltered {off}");
            assert!(on < off * 0.2, "{model:?}: filtered {on}, unfiltered {off}");
        }
    }

    #[test]
    fn deterministic() {
        for model in [ChipModel::Mos6581, ChipModel::Csg8580] {
            let run = || {
                let mut sid = new_sid(model, SamplingMethod::Resample);
                sid.write(0x15, 0x00);
                sid.write(0x16, 0x40);
                sid.write(0x17, 0xf1);
                start_saw(&mut sid, 300.0);
                sid.write(0x18, 0x1f);
                set_freq(&mut sid, 1, 3000);
                sid.write(0x0b, 0x81);
                sid.write(0x0c, 0x00);
                sid.write(0x0d, 0xf0);
                render(&mut sid, 0.25)
            };

            let a = run();
            let b = run();
            assert_eq!(a, b);
            assert!(rms(&a) > 100.0);
        }
    }

    #[test]
    fn random_writes_do_not_panic() {
        for model in [ChipModel::Mos6581, ChipModel::Csg8580] {
            for method in [SamplingMethod::Resample, SamplingMethod::Decimate] {
                for cws in [CombinedWaveforms::Average, CombinedWaveforms::Weak, CombinedWaveforms::Strong] {
                    let mut sid = new_sid(model, method);
                    sid.set_combined_waveforms(cws);

                    let mut state: u64 = 0x1234_5678_9abc_def1;
                    let mut next = move || {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state
                    };

                    let mut buf = [0i16; 64];
                    let mut total = 0;
                    for _ in 0..20000 {
                        let r = next();
                        sid.write((r & 0xff) as u8, ((r >> 8) & 0xff) as u8);
                        let _ = sid.read(((r >> 16) & 0xff) as u8);
                        if r >> 60 == 0 {
                            sid.input((r >> 20) as i32);
                        }
                        // The buffer fills up: the chip must keep running
                        total += sid.clock(((r >> 32) % 40) as u32 + 1, &mut buf);
                    }
                    assert!(total > 0);

                    // Switching the model on the way, and a reset
                    sid.set_chip_model(if model == ChipModel::Mos6581 {
                        ChipModel::Csg8580
                    } else {
                        ChipModel::Mos6581
                    });
                    for _ in 0..20000 {
                        let r = next();
                        sid.write((r & 0xff) as u8, ((r >> 8) & 0xff) as u8);
                        sid.clock(((r >> 32) % 40) as u32 + 1, &mut buf);
                    }
                    sid.reset();
                    sid.clock(1000, &mut buf);
                }
            }
        }
    }

    #[test]
    fn full_buffer_still_advances_the_chip() {
        let mut a = new_sid(ChipModel::Csg8580, SamplingMethod::Decimate);
        let mut b = new_sid(ChipModel::Csg8580, SamplingMethod::Decimate);
        start_saw(&mut a, 440.0);
        start_saw(&mut b, 440.0);

        let mut small = [0i16; 10];
        let n = a.clock(98524, &mut small);
        assert_eq!(n, 10);

        let mut big = vec![0i16; 5000];
        let m = b.clock(98524, &mut big);
        assert!(m > 4000);
        assert_eq!(small[..], big[..10]);

        // Both chips are at the same point now
        let x = a.clock(5000, &mut big);
        let mut big2 = vec![0i16; 5000];
        let y = b.clock(5000, &mut big2);
        assert_eq!(x, y);
        assert_eq!(big[..x], big2[..y]);

        // Empty buffer
        assert_eq!(a.clock(1000, &mut []), 0);
    }

    #[test]
    fn bad_sampling_parameters_are_errors() {
        let mut sid = Sid::new();
        for (clock, rate) in [
            (0.0, 48000.0),
            (CLOCK, 0.0),
            (f64::NAN, 48000.0),
            (CLOCK, f64::INFINITY),
            (CLOCK, -1.0),
            (44100.0, 48000.0),
        ] {
            for method in [SamplingMethod::Resample, SamplingMethod::Decimate] {
                let r = sid.set_sampling_parameters(clock, method, rate);
                assert!(r.is_err(), "{clock} {rate} {method:?}");
            }
        }

        // Odd but finite rates: an error or a working resampler, never a panic
        for rate in [100.0, 1000.0, 8000.0, 11025.0, 22050.0, 44100.0, 96000.0, 192000.0, 500000.0] {
            for method in [SamplingMethod::Resample, SamplingMethod::Decimate] {
                if sid.set_sampling_parameters(CLOCK, method, rate).is_ok() {
                    let mut buf = [0i16; 256];
                    sid.clock(20000, &mut buf);
                }
            }
        }

        // No sampling parameters: nothing is produced, nothing breaks
        let mut fresh = Sid::new();
        let mut buf = [0i16; 16];
        assert_eq!(fresh.clock(1000, &mut buf), 0);
    }

    #[test]
    fn registers_are_masked() {
        let mut sid = new_sid(ChipModel::Csg8580, SamplingMethod::Decimate);
        sid.write(0x04 | 0x20, 0x21);
        sid.write(0xff, 1);
        let _ = sid.read(0xff);
        // $d419/$d41a paddles read 0xff
        assert_eq!(sid.read(0x19 | 0x20), 0xff);
    }
}
