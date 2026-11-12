//! X-Tracker modules (`DDMF`, D-Lusion's Delusion Digital Music Format), played with a software
//! mixer of their own.
//!
//! The loader (`load.rs`) is a port of libopenmpt 0.8.7's `soundlib/Load_dmf.cpp` (BSD-3-Clause,
//! Copyright (c) 2004-2026 OpenMPT Project Developers and Contributors; Johannes Schultz wrote
//! it, "based on DMF.TXT, DMF_EFFC.TXT, trial and error and some invaluable hints by Zatzen"):
//! the chunks, the per-order conversion of the X-Tracker tracks to OpenMPT's commands (tempo in
//! ticks or beats, the effect conversions and their memories, the global track), the DMF
//! Huffman sample compression, and the OpenMPT pieces it calls (`CPattern::WriteEffect`,
//! `ModCommand::FillInTwoCommands`, `ConvertToVolCommand`, `ReadOrderFromFile`,
//! `ModSample::SanitizeLoops`). The playback (`play.rs`) gives those commands the meaning
//! OpenMPT's player (`Snd_fx.cpp`, `Sndmix.cpp`, `Fastmix.cpp`, `ModChannel.cpp`, same licence)
//! gives them in a DMF module; the sine table and the slide tables are OpenMPT's (the slide
//! arithmetic is shared with `crate::symmod` in `crate::mptmath`). See `LICENSE-openmpt`, which
//! has to travel with these files.
//!
//! What it plays: every command OpenMPT's loader writes (notes, note cut and key off, note
//! delay, retrigger, sample offsets with the high offset, reverse play, pitch slides, tone
//! portamento, vibrato, tremor, arpeggio, fine tune, volume, volume slides, tremolo, panning,
//! panning slides, panbrello, speed and tempo, row and frame delays, the sequence loop as a
//! position jump). Song end: the order list played to its end, an end-of-song entry or an entry
//! played again, as OpenMPT ends it; one subsong. The channels start centred.

pub mod load;
pub mod play;
#[cfg(test)]
mod tests;

pub use load::{load, test, Module};

/// An X-Tracker module playing at a sample rate, rendering 16-bit stereo.
pub struct Dmf {
    p: play::Player,
    format: String,
    tmp: Vec<f32>,
}

impl Dmf {
    /// Loads `data` and starts it at `rate` Hz; None for what is not a module this plays.
    pub fn new(data: &[u8], rate: u32) -> Option<Dmf> {
        let m = load::load(data).ok()?;
        let format = format!("Delusion Digital Music Format v{} ({})", m.version, m.tracker);
        let p = play::Player::new(m, rate);
        if p.ended() {
            return None; // no pattern in the order list
        }
        Some(Dmf { p, format, tmp: Vec::new() })
    }

    /// "Delusion Digital Music Format v8 (X-Tracker)": the file's version and tracker.
    pub fn format(&self) -> &str {
        &self.format
    }

    /// The song name.
    pub fn title(&self) -> &str {
        &self.p.module().title
    }

    /// The composer the file names.
    pub fn artist(&self) -> &str {
        &self.p.module().artist
    }

    /// Nearest-sample interpolation instead of linear.
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
