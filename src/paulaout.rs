//! Paula's output stage as UADE computes it, to the bit: the "default" resampler
//! (each channel's output averaged over the colour clocks of an output sample),
//! the hard left/right mix, and the A500/A1200 and LED filters; or UADE's Sinc
//! resampler (BLEP synthesis with a table per filter state, which replaces the RC
//! filters). Checked against the per-channel outputs and final samples of UADE's
//! `--write-audio` dumps, and against uade123's output.
//!
//! The Sinc resampler is a port of `sample16si_sinc_handler` and `sinc_prehandler` of UADE's
//! audio.c (Copyright 1995-1997 Bernd Schmidt, 1996 Marcus Sundberg, 1996 Manfred Thole,
//! 2005 Heikki Orsila, 2005 Antti S. Lankila: the UAE and UADE sound core, GNU GPL version 2
//! or later as the UADE distribution's COPYING says, COPYING.GPL being version 2, June 1991),
//! with the table of [`crate::sinctable`].
//!
//! Time is in colour clocks (3546895 per second, PAL). The caller advances the
//! stage in steps no longer than `until_sample()`, giving the four channel
//! outputs (sample × volume) that held during the step. A step that ends on a
//! sample boundary produces a sample.

pub const COLOUR_CLOCKS_PAL: u32 = 3_546_895;
/// The rate UADE resets the emulation at (UADE_DEFAULT_FREQUENCY).
const UADE_RESET_RATE: u32 = 44100;

/// How Paula's output is resampled to the output rate (UADE's `--resampler`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Resampler {
    /// Each channel averaged over the colour clocks of an output sample, then the RC filters.
    UadeDefault,
    /// BLEP mixing with UADE's windowed-sinc tables; the table of the filter model and LED
    /// state does the filtering.
    #[default]
    Sinc,
}

/// The BLEP queue of one channel holds this many output changes (a power of two); an
/// entry is used for 2048 colour clocks (`sinctable::SINC_LEN`), then dropped.
const SINC_QUEUE_LENGTH: usize = 256;

#[derive(Clone)]
struct Sinc {
    /// Colour clocks since the start (wraps; only differences are used).
    time: i32,
    /// Each channel's output level as of the last change.
    state: [i32; 4],
    head: [usize; 4],
    /// (time of the change, size of the change), newest first from `head`.
    queue: Vec<[(i32, i32); SINC_QUEUE_LENGTH]>,
}

impl Sinc {
    fn new() -> Self {
        Sinc { time: 0, state: [0; 4], head: [0; 4], queue: vec![[(0, 0); SINC_QUEUE_LENGTH]; 4] }
    }

    /// UADE's `sinc_prehandler`: the channels output `out` for `clocks`.
    #[inline(always)]
    fn advance(&mut self, out: [i32; 4], clocks: u32) {
        for i in 0..4 {
            if self.state[i] != out[i] {
                self.head[i] = (self.head[i].wrapping_sub(1)) & (SINC_QUEUE_LENGTH - 1);
                self.queue[i][self.head[i]] = (self.time, out[i].wrapping_sub(self.state[i]));
                self.state[i] = out[i];
            }
        }
        self.time = self.time.wrapping_add(clocks as i32);
    }

    /// UADE's `sample16si_sinc_handler`: the four channels' levels now, with `winsinc` the
    /// table for the filter state.
    fn sample(&self, winsinc: &[i32; crate::sinctable::SINC_LEN]) -> [i32; 4] {
        let mut o = [0i32; 4];
        for (i, o) in o.iter_mut().enumerate() {
            // (C's int arithmetic, which wraps in practice; the total fits)
            let mut sum = self.state[i].wrapping_shl(17);
            let mut pos = self.head[i];
            for _ in 0..SINC_QUEUE_LENGTH {
                let (t, change) = self.queue[i][pos];
                let age = self.time.wrapping_sub(t);
                if age >= crate::sinctable::SINC_LEN as i32 {
                    break;
                }
                sum = sum.wrapping_sub(winsinc[age as usize].wrapping_mul(change));
                pos = (pos + 1) & (SINC_QUEUE_LENGTH - 1);
            }
            *o = sum >> 16;
        }
        o
    }
}

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
    resampler: Resampler,
    /// The Sinc resampler's state, while it is the one in use.
    sinc: Option<Box<Sinc>>,
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
            resampler: Resampler::UadeDefault,
            sinc: None,
        }
    }

    pub fn resampler(&self) -> Resampler {
        self.resampler
    }

    /// Switches the resampler; the Sinc one starts from silence.
    pub fn set_resampler(&mut self, r: Resampler) {
        if r != self.resampler {
            self.resampler = r;
            self.acc = [0; 4];
            self.acc_time = 0;
            self.sinc = (r == Resampler::Sinc).then(|| Box::new(Sinc::new()));
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
        if self.sinc.is_some() {
            self.sinc = Some(Box::new(Sinc::new()));
        }
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
        if let Some(sinc) = &mut self.sinc {
            sinc.advance(out, clocks);
            if clocks != rounded {
                return None;
            }
            self.next += self.interval;
            let n = match self.filter {
                Filter::A500 => 0,
                Filter::A1200 => 2,
                Filter::None => 4,
            } + (self.filter != Filter::None && self.led) as usize;
            let o = sinc.sample(&crate::sinctable::WINSINC_INTEGRAL[n]);
            return Some(((o[0] + o[3]).clamp(-32768, 32767) as i16, (o[1] + o[2]).clamp(-32768, 32767) as i16));
        }
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
    /// [UADE_RESAMPLER=sinc]. The dump holds no LED state: with UADE_LED=toggle both
    /// states are computed and every UADE sample must be one of the two (the LED
    /// timeline is then inferred, and reported).
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
        let led = std::env::var("UADE_LED").ok();
        let toggle = led.as_deref() == Some("toggle");
        let sinc = std::env::var("UADE_RESAMPLER").as_deref() == Ok("sinc");
        let mk = |led: bool| {
            let mut s = OutputStage::new(rate, filter);
            if sinc {
                s.set_resampler(Resampler::Sinc);
            }
            s.led = led;
            s
        };
        // the second stage is only used when toggling
        let (mut s, mut s_on) = (mk(led.as_deref() == Some("1")), mk(true));
        let (mut carry, mut out, mut uade, mut ours, mut ours_on) = (0u32, [0i32; 4], Vec::new(), Vec::new(), Vec::new());
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
            let t = td + std::mem::take(&mut carry);
            if let Some((l, r)) = s.step(out, t) {
                ours.extend([l, r]);
            }
            if toggle {
                if let Some((l, r)) = s_on.step(out, t) {
                    ours_on.extend([l, r]);
                }
            }
        }
        if toggle {
            let (mut bad, mut on_only, mut off_only, mut both, mut changes, mut state) = (None, 0, 0, 0, 0, None);
            for (k, u) in uade.iter().enumerate() {
                let (a, b) = (ours.get(k), ours_on.get(k));
                match (Some(u) == a, Some(u) == b) {
                    (false, false) => {
                        bad = bad.or(Some(k));
                    }
                    (true, true) => both += 1,
                    (true, false) | (false, true) => {
                        let now = Some(u) == b;
                        if now { on_only += 1 } else { off_only += 1 }
                        if state.is_some_and(|x| x != now) {
                            changes += 1;
                        }
                        state = Some(now);
                    }
                }
            }
            assert_eq!(bad, None, "first sample of neither LED state at index {}", bad.unwrap_or(0) / 2);
            assert_eq!(ours.len(), uade.len());
            eprintln!(
                "{} values bit-exact in one LED state or the other: {off_only} LED off only, {on_only} LED on only, {both} either; {changes} LED changes inferred",
                uade.len()
            );
            return;
        }
        let first = ours.iter().zip(&uade).position(|(a, b)| a != b);
        assert_eq!(first, None, "first difference at sample {}", first.unwrap_or(0) / 2);
        assert_eq!(ours.len(), uade.len());
        eprintln!("{} samples bit-exact", ours.len() / 2);
    }

    #[test]
    fn sinc_dc_and_impulse() {
        // a constant level: the blep of its one change sums to the level (twice, as 16-bit)
        for (filter, led) in [(Filter::None, false), (Filter::A500, false), (Filter::A500, true), (Filter::A1200, false), (Filter::A1200, true)] {
            let mut s = OutputStage::new(48000, filter);
            s.set_resampler(Resampler::Sinc);
            s.led = led;
            let mut last = None;
            for _ in 0..200 {
                let n = s.until_sample();
                last = s.step([1000, 0, 0, -500], n);
            }
            let (l, r) = last.unwrap();
            // channels 0 and 3 left: 1000 - 500, scaled by 2
            assert!((l as i32 - 1000).abs() <= 1, "{filter:?} led {led}: {l}");
            assert_eq!(r, 0);
        }
        // one change of +8000 for a single colour clock: the sinc response is a short pulse
        // whose sum over the output samples is its area (8000 for one colour clock, times 2),
        // spread over the output samples (73.9 clocks each), with no output before it
        let mut s = OutputStage::new(48000, Filter::None);
        s.set_resampler(Resampler::Sinc);
        let mut total = 0i64;
        for k in 0..60 {
            let n = s.until_sample();
            let out = if k == 3 { [8000, 0, 0, 0] } else { [0; 4] };
            // the pulse takes the first colour clock of the window; the rest is silent
            let got = if k == 3 {
                s.step(out, 1);
                s.step([0; 4], n - 1)
            } else {
                s.step(out, n)
            };
            let (l, _) = got.unwrap();
            if k < 3 {
                assert_eq!(l, 0);
            }
            total += l as i64;
        }
        let expect = 2.0 * 8000.0 / 73.9;
        assert!((total as f64 - expect).abs() < 0.1 * expect, "pulse area {total}, expected about {expect}");
    }

    #[test]
    fn default_resampler_is_unchanged_by_the_sinc_state() {
        // Resampler::UadeDefault has no Sinc state and the stage starts there
        let s = OutputStage::new(48000, Filter::A500);
        assert_eq!(s.resampler(), Resampler::UadeDefault);
        assert!(s.sinc.is_none());
    }
}
