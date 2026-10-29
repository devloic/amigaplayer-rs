//! A port of libxmp's player core and software mixer, for the PC tracker
//! formats that only libxmp (or OpenMPT) plays: a format-independent module
//! model (`module`), the loader helpers (`load`), the player (`player`,
//! `effects`, `virt`, `scan`), the 16-bit stereo mixer (`mixer`) and one loader
//! per format (`loaders`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! A loader fills a [`module::ModuleData`] the way its libxmp counterpart does:
//! the module (`mod`), the quirk and flow flags, `read_event_type`,
//! `period_type`, `c4rate` and the rest. [`XmpPlayer`] then runs libxmp's
//! epilogue, scan and player on it and renders 16-bit stereo at any rate.

pub mod effects;
pub mod load;
pub mod loaders;
pub mod mixer;
pub mod module;
pub mod player;
pub mod scan;
mod spline_lut;
pub mod virt;

use load::Hio;
use module::ModuleData;
use player::Context;

/// A libxmp loader: `test` is the loader's test function, `load` its loader.
pub struct Loader {
    pub name: &'static str,
    pub test: fn(&[u8]) -> bool,
    pub load: fn(&mut ModuleData, &mut Hio) -> Result<(), ()>,
}

/// The loaders, in libxmp's order (`format_loaders` of `format.c`).
pub static LOADERS: &[Loader] = &[
    Loader { name: "MultiTracker", test: loaders::mtm::test, load: loaders::mtm::load },
    Loader { name: "Composer 669", test: loaders::c669::test, load: loaders::c669::load },
];

/// The loader that claims `data`, if any.
pub fn detect(data: &[u8]) -> Option<&'static Loader> {
    LOADERS.iter().find(|l| (l.test)(data))
}

/// `xmp_load_module`: the module in `data`, checked and scanned.
pub fn load_module(data: &[u8]) -> Option<Context> {
    let loader = detect(data)?;
    let mut m = ModuleData::default();
    (loader.load)(&mut m, &mut Hio::new(data)).ok()?;
    load::load_epilogue(&mut m).ok()?;
    let mut ctx = Context::new(m);
    ctx.scan_sequences();
    Some(ctx)
}

/// A module playing through libxmp's player and mixer.
pub struct XmpPlayer {
    ctx: Context,
    rate: i32,
    /// The rest of the last tick, interleaved stereo.
    pending: Vec<i16>,
    at: usize,
    ended: bool,
    subsong: usize,
}

impl XmpPlayer {
    /// Loads `data` and starts it at `rate` Hz, sequence 0.
    pub fn new(data: &[u8], rate: u32) -> Option<XmpPlayer> {
        let ctx = load_module(data)?;
        let mut p = XmpPlayer { ctx, rate: rate as i32, pending: Vec::new(), at: 0, ended: false, subsong: 0 };
        p.start(0);
        Some(p)
    }

    /// libxmp's module type ("Composer 669", "MultiTracker 1.00 MTM"...).
    pub fn format(&self) -> &str {
        &self.ctx.m.module.type_
    }

    pub fn title(&self) -> &str {
        &self.ctx.m.module.name
    }

    /// The sequences libxmp's scan found.
    pub fn subsongs(&self) -> usize {
        self.ctx.m.num_sequences.max(1)
    }

    pub fn subsong(&self) -> usize {
        self.subsong
    }

    /// The scanned duration of a sequence, in milliseconds.
    pub fn duration_ms(&self, seq: usize) -> i32 {
        self.ctx.m.seq_data.get(seq).map_or(0, |s| s.duration)
    }

    /// `xmp_start_player`, then `xmp_set_position` to the sequence's entry
    /// point (what `xmp_set_subsong`... the player's sequence selection does).
    pub fn start(&mut self, seq: usize) {
        let seq = seq.min(self.subsongs() - 1);
        // a fresh player state that keeps what the load-time scan found
        let old = std::mem::take(&mut self.ctx.p);
        self.ctx.p.scan = old.scan;
        self.ctx.p.sequence_control = old.sequence_control;
        self.ctx.p.scan_time_factor = old.scan_time_factor;
        self.ctx.start_player(self.rate);
        if seq > 0 {
            let ep = self.ctx.m.seq_data[seq].entry_point as i32;
            self.ctx.set_position(ep, 0);
        }
        self.subsong = seq;
        self.pending.clear();
        self.at = 0;
        self.ended = false;
    }

    /// True once the module ended or looped.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Fills `out` (interleaved stereo) and returns the frames written: fewer
    /// than asked only at the end of the module, which is where libxmp's frame
    /// loop stops (`-XMP_END`) or the loop counter reaches 1.
    pub fn render(&mut self, out: &mut [i16]) -> usize {
        let want = out.len() / 2;
        let mut done = 0;
        while done < want {
            if self.at >= self.pending.len() {
                if self.ended || !self.next_tick() {
                    self.ended = true;
                    break;
                }
            }
            let n = ((self.pending.len() - self.at) / 2).min(want - done);
            out[done * 2..(done + n) * 2].copy_from_slice(&self.pending[self.at..self.at + n * 2]);
            self.at += n * 2;
            done += n;
        }
        done
    }

    fn next_tick(&mut self) -> bool {
        if !self.ctx.play_frame() || self.ctx.p.loop_count >= 1 {
            return false;
        }
        let n = (self.ctx.s.ticksize * 2) as usize;
        self.pending.clear();
        self.pending.extend_from_slice(&self.ctx.s.buffer[..n]);
        self.at = 0;
        true
    }

    /// The order, pattern and row playing (for display).
    pub fn position(&self) -> (i32, i32, i32) {
        let p = &self.ctx.p;
        let pat = self.ctx.m.module.xxo.get(p.ord.max(0) as usize).copied().unwrap_or(0) as i32;
        (p.ord, pat, p.row)
    }
}
