//! Symphonie and Symphonie Pro modules (`SymM`), played with a software mixer of their own.
//!
//! The loader (`load.rs`) is a port of libopenmpt 0.8.7's `soundlib/Load_symmod.cpp`
//! (BSD-3-Clause, Copyright (c) 2004-2026 OpenMPT Project Developers and Contributors; Devin
//! Acker and the OpenMPT devs wrote it, "based in part on Patrick Meng's Java-based Symphonie
//! player and its source"): the chunk list, the packed and delta-coded sample data, the
//! instrument conversions (reverse, invert, 8 to 16 bits, high-pass, volume fade, filter sweep,
//! normalise, amplify, sustain repeat), the virtual and transwave instruments rendered from the
//! others, and the conversion of the Symphonie events to OpenMPT's commands, with the filter and
//! echo macros. The echo is a port of libopenmpt's `soundlib/plugins/SymMODEcho.cpp` (same
//! licence). The playback (`play.rs`) gives those commands the meaning OpenMPT's player
//! (`Snd_fx.cpp`, `Sndmix.cpp`, `Snd_flt.cpp`, same licence) gives them in an MPTM module with
//! IT behaviour, linear frequency slides and frequencies for periods; the tables there (the IT
//! sine, the linear slide tables, the IT portamento column) are OpenMPT's. See
//! `LICENSE-openmpt`, which has to travel with these files.
//!
//! Sample files embedded in the instruments go through the repository's own decoders: IFF 8SVX
//! (`iff8svx`), RIFF WAVE (`riffwave`) and AIFF; the MAESTRO and 16BT headers and raw 8-bit data
//! are read as OpenMPT reads them.
//!
//! What it plays: every command OpenMPT's loader writes (notes, key-off, stop/continue sample,
//! volumes, volume slides, pitch slides, tone portamento, vibrato, tremolo, retrigger, sample
//! offset and cue jump, speed, filter, echo). Song end: the order list played once to its end,
//! as OpenMPT does without looping; one subsong. The panning is hard, even channels left and odd
//! right, as the loader sets it.

pub mod load;
pub mod play;
#[cfg(test)]
mod tests;

pub use load::{load, test, Module};

/// A Symphonie module playing at a sample rate, rendering 16-bit stereo.
pub struct Symmod {
    p: play::Player,
    title: String,
    tmp: Vec<f32>,
}

impl Symmod {
    /// Loads `data` and starts it at `rate` Hz; None for what is not a module this plays.
    pub fn new(data: &[u8], rate: u32) -> Option<Symmod> {
        let m = load::load(data).ok()?;
        let title = m.message.lines().next().unwrap_or("").trim().to_string();
        let p = play::Player::new(m, rate);
        if p.ended() {
            return None; // no pattern in the order list
        }
        Some(Symmod { p, title, tmp: Vec::new() })
    }

    /// "Symphonie", "Symphonie Pro" or "Symphonie Pro 256" (the tracker the file says).
    pub fn format(&self) -> &str {
        self.p.module().tracker
    }

    /// Symphonie modules have no title: the first line of the song's info text, if any.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Nearest-sample interpolation instead of linear (spline plays as linear).
    pub fn set_nearest(&mut self, nearest: bool) {
        self.p.set_nearest(nearest);
    }

    pub fn duration_ms(&self) -> i32 {
        self.p.duration_ms()
    }

    pub fn start(&mut self) {
        self.p.restart();
    }

    pub fn ended(&self) -> bool {
        self.p.ended()
    }

    pub fn position(&self) -> (i32, i32, i32) {
        self.p.position()
    }

    /// Fills `out` (interleaved stereo) and returns the frames written: fewer at the end.
    pub fn render(&mut self, out: &mut [i16]) -> usize {
        self.tmp.resize(out.len() & !1, 0.0);
        let n = self.p.render(&mut self.tmp);
        for (o, x) in out.iter_mut().zip(&self.tmp[..n * 2]) {
            *o = (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        }
        n
    }
}
