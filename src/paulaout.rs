//! Paula's output stage as UADE computes it, to the bit: the "default" resampler
//! (each channel's output averaged over the colour clocks of an output sample),
//! the hard left/right mix, and the A500/A1200 and LED filters. Checked against
//! the per-channel outputs and final samples of UADE's `--write-audio` dumps.
//!
//! Time is in colour clocks (3546895 per second, PAL). The caller advances the
//! stage in steps no longer than `until_sample()`, giving the four channel
//! outputs (sample × volume) that held during the step. A step that ends on a
//! sample boundary produces a sample.

pub const COLOUR_CLOCKS_PAL: u32 = 3_546_895;
/// The rate UADE resets the emulation at (UADE_DEFAULT_FREQUENCY).
const UADE_RESET_RATE: u32 = 44100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    None,
    A500,
    A1200,
}

/// C's `float` arithmetic at the point UADE adds its denormal offset: the sum
/// is taken in double and rounded back to float.
const DENORMAL_OFFSET: f64 = 1e-10;

/// The 1st order low-pass coefficient b0 for a -3 dB point at `cutoff`, with
/// UADE's bilinear transform correction (and its mix of float and double).
fn rc_a0(rate: u32, cutoff: u32) -> f32 {
    if cutoff >= rate / 2 {
        return 1.0;
    }
    let omega = (2.0 * std::f64::consts::PI * cutoff as f64 / rate as f64) as f32;
    let omega = (((omega / 2.0) as f64).tan() * 2.0) as f32;
    1.0 / (1.0 + 1.0 / omega)
}

#[derive(Clone)]
pub struct OutputStage {
    rate: u32,
    interval: f32,
    next: f32,
    acc: [i32; 4],
    /// Colour clocks in `acc` (the same for the four channels, which UADE counts apart).
    acc_time: i32,
    filter: Filter,
    /// The LED filter (CIA-A PRA bit 1 low).
    pub led: bool,
    a500_a0: (f32, f32),
    led_a0: f32,
    rc: [[f32; 5]; 2],
}

impl OutputStage {
    pub fn new(rate: u32, filter: Filter) -> Self {
        let interval = COLOUR_CLOCKS_PAL as f32 / rate as f32;
        OutputStage {
            rate,
            interval,
            // UADE resets Paula at its default rate and only then gets the real
            // one, so its first sample window is always 44.1 kHz long
            next: COLOUR_CLOCKS_PAL as f32 / UADE_RESET_RATE as f32,
            acc: [0; 4],
            acc_time: 0,
            filter,
            led: false,
            a500_a0: (rc_a0(rate, 6200), rc_a0(rate, 20000)),
            led_a0: rc_a0(rate, 7000),
            rc: [[0.0; 5]; 2],
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn filter(&self) -> Filter {
        self.filter
    }

    /// Switches the filter model; its state starts again from silence.
    /// The sample grid and the filters' history restart, as at Paula's reset (the
    /// filter chosen and the LED state stay).
    pub fn restart(&mut self) {
        let fresh = OutputStage::new(self.rate, self.filter);
        (self.next, self.acc, self.acc_time, self.rc) = (fresh.next, fresh.acc, fresh.acc_time, fresh.rc);
    }

    pub fn set_filter(&mut self, filter: Filter) {
        if filter != self.filter {
            self.filter = filter;
            self.rc = [[0.0; 5]; 2];
        }
    }

    /// Colour clocks until the next output sample.
    #[inline]
    pub fn until_sample(&self) -> u32 {
        // `next` is never negative here: a step shorter than the rounded window
        // leaves at least its integer part, and one that ends a sample adds a whole
        // interval (over 18 clocks) to the at most half a clock it overshot by. For a
        // value of 0 or more, conversion (truncation) is floor, without the libm call
        // `f32::floor` is on x86-64 without SSE4.1.
        let n = self.next;
        let mut r = if n >= 0.0 { n as u32 } else { n.floor() as u32 };
        if n - r as f32 >= 0.5 {
            r += 1;
        }
        r
    }

    /// The channels output `out` for `clocks` (at most `until_sample()`) colour
    /// clocks. Returns the sample that ends there, if one does.
    #[inline]
    pub fn step(&mut self, out: [i32; 4], clocks: u32) -> Option<(i16, i16)> {
        let rounded = self.until_sample();
        self.step_rounded(out, clocks, rounded)
    }

    /// `step` with `until_sample()` already known (`rounded`). Inlined into its callers so
    /// that `out` stays in registers (built there as four words and read back as one
    /// vector, it stalls on store forwarding).
    #[inline(always)]
    pub fn step_rounded(&mut self, out: [i32; 4], clocks: u32, rounded: u32) -> Option<(i16, i16)> {
        debug_assert_eq!(rounded, self.until_sample());
        debug_assert!(clocks <= rounded);
        self.next -= clocks as f32;
        for i in 0..4 {
            self.acc[i] += out[i] * clocks as i32;
        }
        self.acc_time += clocks as i32;
        if clocks != rounded {
            return None;
        }
        self.next += self.interval;
        let mut o = [0i32; 4];
        if self.acc_time != 0 {
            for i in 0..4 {
                // C integer division (towards zero)
                o[i] = self.acc[i] / self.acc_time;
            }
        }
        self.acc = [0; 4];
        self.acc_time = 0;
        // channel outputs are -128*64..127*64; two of them, shifted to 16 bits
        let (l, r) = ((o[0] + o[3]) << 1, (o[1] + o[2]) << 1);
        if self.filter == Filter::None {
            return Some((l as i16, r as i16));
        }
        Some((self.apply_filter(l, 0), self.apply_filter(r, 1)))
    }

    /// Advances `clocks` colour clocks of constant output, calling `emit` for
    /// every sample.
    pub fn run(&mut self, out: [i32; 4], mut clocks: u32, mut emit: impl FnMut(i16, i16)) {
        while clocks > 0 {
            let until = self.until_sample();
            let n = clocks.min(until);
            if let Some((l, r)) = self.step_rounded(out, n, until) {
                emit(l, r);
            }
            clocks -= n;
        }
    }

    fn apply_filter(&mut self, input: i32, side: usize) -> i16 {
        let x = input as f32;
        let fs = &mut self.rc[side];
        let (normal, led) = match self.filter {
            Filter::A500 => {
                let (a1, a2) = self.a500_a0;
                let a = self.led_a0;
                fs[0] = ((a1 * x + (1.0 - a1) * fs[0]) as f64 + DENORMAL_OFFSET) as f32;
                fs[1] = a2 * fs[0] + (1.0 - a2) * fs[1];
                fs[2] = a * fs[1] + (1.0 - a) * fs[2];
                fs[3] = a * fs[2] + (1.0 - a) * fs[3];
                fs[4] = a * fs[3] + (1.0 - a) * fs[4];
                (fs[1], fs[4])
            }
            Filter::A1200 => {
                let a = self.led_a0;
                fs[1] = ((a * x + (1.0 - a) * fs[1]) as f64 + DENORMAL_OFFSET) as f32;
                fs[2] = a * fs[1] + (1.0 - a) * fs[2];
                fs[3] = a * fs[2] + (1.0 - a) * fs[3];
                (x, fs[3])
            }
            Filter::None => (x, x),
        };
        // C converts float to int by truncation, then clamps
        (if self.led { led } else { normal } as i32).clamp(-32768, 32767) as i16
    }
}

/// uade123's panning effect (`-p`, default 0.7): `amount` in [0, 2].
pub fn pan_amount(amount: f32) -> i32 {
    (amount as f64 * 256.0 / 2.0) as i32
}

pub fn pan(l: i16, r: i16, amount: i32) -> (i16, i16) {
    let (l, r) = (l as i32, r as i32);
    let m = (r - l) * amount;
    ((((l << 8) + m) >> 8) as i16, (((r << 8) - m) >> 8) as i16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficients_and_boundaries() {
        // the first window is 44.1 kHz long (80.4 colour clocks), then one every
        // 73.9 at 48 kHz
        let mut s = OutputStage::new(48000, Filter::None);
        let mut lens = Vec::new();
        for _ in 0..8 {
            let n = s.until_sample();
            lens.push(n);
            assert!(s.step([0; 4], n).is_some());
        }
        assert_eq!(lens[0], 80);
        assert!(lens[1..].iter().all(|&n| n == 73 || n == 74), "{lens:?}");
        assert_eq!(rc_a0(48000, 30000), 1.0);
        assert_eq!(pan_amount(0.7), 89);
        assert_eq!(pan(-100, 100, 128), (0, 0));
    }

    #[test]
    fn averages_and_mixes() {
        let mut s = OutputStage::new(48000, Filter::None);
        let n = s.until_sample();
        // channel 0 at 100 for half the window, 0 after; channel 1 at -64*127 throughout
        assert!(s.step([100, -8128, 0, 0], n / 2).is_none());
        let got = s.step([0, -8128, 0, 0], n - n / 2).unwrap();
        assert_eq!(got, ((((100 * (n / 2) as i32) / n as i32) << 1) as i16, -16256));
    }

    /// Recomputes every output sample of a UADE `--write-audio` dump from the
    /// per-channel outputs it records. The dump must start at reset, which the
    /// patched uadecore of the comparison does with UADE_WRITE_AUDIO_ALL=1:
    /// UADE_WRITE_AUDIO=dump.bin UADE_RATE=48000 UADE_FILTER=none|a500|a1200 [UADE_LED=1]
    #[test]
    #[ignore]
    fn matches_uade_write_audio() {
        let data = std::fs::read(std::env::var("UADE_WRITE_AUDIO").unwrap()).unwrap();
        let rate: u32 = std::env::var("UADE_RATE").map(|r| r.parse().unwrap()).unwrap_or(48000);
        let filter = match std::env::var("UADE_FILTER").as_deref() {
            Ok("a500") => Filter::A500,
            Ok("a1200") => Filter::A1200,
            _ => Filter::None,
        };
        let mut s = OutputStage::new(rate, filter);
        s.led = std::env::var("UADE_LED").as_deref() == Ok("1");
        let (mut carry, mut out, mut uade, mut ours) = (0u32, [0i32; 4], Vec::new(), Vec::new());
        for f in data[16..].chunks_exact(12) {
            let td = u32::from_be_bytes([f[0], f[1], f[2], f[3]]);
            if td & 0x8000_0000 != 0 {
                // a register or output event; a step with events keeps its time here
                carry += td & 0xFF_FFFF;
                if f[5] == 8 {
                    uade.push(i16::from_be_bytes([f[6], f[7]]));
                }
                continue;
            }
            for (c, o) in out.iter_mut().enumerate() {
                *o = i16::from_be_bytes([f[4 + 2 * c], f[5 + 2 * c]]) as i32;
            }
            // UADE splits its steps at sample boundaries, so one never crosses one
            if let Some((l, r)) = s.step(out, td + std::mem::take(&mut carry)) {
                ours.extend([l, r]);
            }
        }
        let first = ours.iter().zip(&uade).position(|(a, b)| a != b);
        assert_eq!(first, None, "first difference at sample {}", first.unwrap_or(0) / 2);
        assert_eq!(ours.len(), uade.len());
        eprintln!("{} samples bit-exact", ours.len() / 2);
    }
}
