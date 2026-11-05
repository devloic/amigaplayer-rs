//! The open replayers and the built-in ProTracker alone, for a host that brings its
//! own player for everything else (the archive app: UADE and libxmp there). No 68k
//! player files are loaded, so a `Song` plays only what `replay::REPLAYERS` and the
//! built-in ProTracker (with ProWizard's conversions of packed modules) know, on the
//! emulated Paula, and the PC tracker formats of the libxmp port (`crate::xmp`) on
//! its own mixer.

use crate::engine::{Backend, Engine, Playing};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// The format the open replayers or the built-in ProTracker would play `data` as,
/// if any (after decrunching and ProWizard's conversion, as the engine does). The
/// 15-instrument modules that `replay::soundtracker` identifies are claimed, by
/// variant ("Ultimate SoundTracker 1.8", "SoundTracker II", ...). The engine's
/// last-resort 15-instrument heuristic ("SoundTracker") is not: it is loose, and the
/// host's own players take the modules it alone would claim.
pub fn detect(data: &[u8]) -> Option<String> {
    if crate::aiff::detect(data) {
        return Some("AIFF".to_string());
    }
    let data = crate::decrunch::decrunch(data.to_vec());
    let data = crate::prowizard::convert(data, Path::new(""));
    if let Some(f) = crate::protracker::ProTracker::detect(&data).filter(|f| *f != "SoundTracker") {
        return Some(f.to_string());
    }
    if let Some(o) = crate::replay::REPLAYERS.iter().find(|o| (o.detect)(&data)) {
        return Some(o.name.to_string());
    }
    crate::xmp::detect_open(&data)?;
    crate::xmp::load_module(&data).map(|c| c.m.module.type_)
}

pub struct Song {
    e: Engine,
}

impl Song {
    /// Starts `data` at `rate` Hz. `path` is where companion files (TFMX `smpl.*`,
    /// SMUS `Instruments/`) are looked for, and its name what name checks see.
    pub fn open(path: &Path, data: Vec<u8>, rate: u32) -> Result<Song> {
        Song::open_with(path, data, Vec::new(), rate)
    }

    /// As `open`, with the companion files given in memory (each at its host path,
    /// e.g. beside `path`) for a host with no file system.
    pub fn open_with(path: &Path, data: Vec<u8>, files: Vec<(PathBuf, Vec<u8>)>, rate: u32) -> Result<Song> {
        let mut e = Engine::new(rate as f64);
        e.m.os.mem_files = files.into_iter().collect();
        // the module too: some players read it again by its name (IFF-SMUS)
        e.m.os.mem_files.insert(path.to_path_buf(), data.clone());
        e.open_replayers = true;
        e.play_data(path, data)?;
        match &e.playing {
            Some(Playing { backend: Backend::Replay(_) | Backend::Mix(_) | Backend::Pcm(_), .. }) => {}
            Some(Playing { backend: Backend::Native(_), format, .. }) if format != "SoundTracker" => {}
            _ => bail!("no open replayer plays this module"),
        }
        Ok(Song { e })
    }

    /// Fills `out` with interleaved stereo frames in -1..1.
    pub fn render(&mut self, out: &mut [f32]) {
        self.e.render(out);
    }

    /// The first, last and current subsong.
    pub fn subsongs(&self) -> (u16, u16, u16) {
        self.e.playing.as_ref().map_or((0, 0, 0), |p| (p.sub_min, p.sub_max, p.sub))
    }

    /// Restarts at subsong `n`, if it is in range.
    pub fn set_subsong(&mut self, n: u16) -> Result<()> {
        let (lo, hi, cur) = self.subsongs();
        if (lo..=hi).contains(&n) && n != cur {
            self.e.change_subsong(n as i32 - cur as i32)?;
        }
        Ok(())
    }

    /// The replayer has reported the end of the song (many never do).
    pub fn songend(&self) -> bool {
        self.e.songend()
    }

    pub fn format(&self) -> &str {
        self.e.playing.as_ref().map_or("", |p| &p.format)
    }

    pub fn title(&self) -> &str {
        self.e.playing.as_ref().map_or("", |p| &p.module_name)
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.e
    }

    /// Seconds played since the subsong started.
    pub fn elapsed_secs(&self) -> f64 {
        self.e.elapsed_secs()
    }
}
