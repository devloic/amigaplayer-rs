//! The library's player: [`Song`], [`detect`], [`formats`] and [`Error`].
//!
//! This module is re-exported at the crate root; see the crate documentation.

use crate::engine::{Backend, Playing};
use crate::open;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

/// What can go wrong. New variants may be added in a minor release.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// No replayer claims these bytes (use [`detect`] to ask without opening).
    NotRecognised,
    /// A replayer claims the bytes but cannot play them: corrupt or truncated data, or a
    /// variant it does not support (an AIFF compression, for instance). The text says why.
    Invalid(String),
    /// A replayer recognises the module but cannot set it up: files it reads beside the module
    /// are not among the companions given (or, for a module played from disk, not in its
    /// directory). [`detect`] names the format all the same. Hand the files over with
    /// [`Song::open_with`] and open it again.
    MissingCompanion {
        /// The format that recognised the module ([`detect`]'s answer, "TFMX Pro", "IFF-SMUS").
        format: String,
        /// The files it looked for and did not find, in the order it tried them, as paths
        /// relative to the module (`"smpl.tune"`, `"Instruments/Piano.instr"`), the form
        /// [`Song::open_with`] takes them in. Where a format knows other names for the same
        /// file (TFMX: `smpl.tune`, `tune.smpl`, `tune.sam`, `smpl.set`) each is listed, and one
        /// of them is enough; a format that stops at the first file it misses (IFF-SMUS at
        /// its first instrument) names that one, and more may be missing after it.
        wanted: Vec<String>,
    },
    /// The sample rate is outside `1000..=768000` Hz.
    InvalidRate(u32),
    /// [`Song::set_subsong`] was asked for a subsong outside `first..=last`.
    SubsongOutOfRange {
        /// The subsong asked for.
        requested: u16,
        /// The first subsong.
        first: u16,
        /// The last subsong.
        last: u16,
    },
    /// A replayer panicked on this input (a bug, caught so that it cannot take the host down; with `panic = "abort"`, as in the
    /// WebAssembly build, nothing is caught and the panic aborts instead).
    /// The [`Song`] that raised it is unusable and renders silence.
    Internal(String),
    /// The 68k code of a custom module failed while it played (see [`Options::allow_custom`]):
    /// a CPU exception (illegal instruction, address error, ...), or a call that did not
    /// return within its instruction budget. Reported by [`Song::error`]; the [`Song`] renders
    /// silence from there on. The text says what happened and where.
    Guest(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotRecognised => f.write_str("no replayer recognises this data"),
            Error::Invalid(why) => write!(f, "cannot play this module: {why}"),
            Error::MissingCompanion { format, wanted } => {
                write!(f, "{format} module, but a file it needs beside it is missing (looked for: {})", wanted.join(", "))
            }
            Error::InvalidRate(r) => write!(f, "sample rate {r} Hz is outside 1000..=768000"),
            Error::SubsongOutOfRange { requested, first, last } => {
                write!(f, "subsong {requested} is outside {first}..={last}")
            }
            Error::Internal(why) => write!(f, "internal error: {why}"),
            Error::Guest(why) => write!(f, "the module's own code failed: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// A function of an AmigaOS library that the 68k code of a custom module called and that the
/// emulated system does not implement (see [`Song::missing_calls`]). It answered 0 and did
/// nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MissingCall {
    /// The library (or device, or resource) the call went to, as the code opened it
    /// (`"exec.library"`, `"audio.device"`).
    pub library: String,
    /// The function's name from the library's interface description (`"AddTask"`), where the
    /// host knows it.
    pub function: Option<String>,
    /// The library vector offset that was called, negative as in the interface files (`-282`).
    pub lvo: i32,
    /// The replay cannot work without the call (the code waits for something the host never
    /// does): the song then stops, and [`Song::error`] names the call. Otherwise the module
    /// plays on, and the call only may make it play differently.
    pub fatal: bool,
}

impl fmt::Display for MissingCall {
    /// `exec.library/AddTask() (LVO -282)`, or `foo.library LVO -30` for a function the host
    /// has no name for.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.function {
            Some(n) => write!(f, "{}/{n}() (LVO {})", self.library, self.lvo),
            None => write!(f, "{} LVO {}", self.library, self.lvo),
        }
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// The first, last and current subsong of a [`Song`], in the numbering of the format
/// (many start at 0, some at 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Subsongs {
    /// The first subsong.
    pub first: u16,
    /// The last subsong (equal to `first` for a module with one).
    pub last: u16,
    /// The subsong that is playing.
    pub current: u16,
}

impl Subsongs {
    /// How many subsongs there are.
    pub fn count(&self) -> u32 {
        (self.last.saturating_sub(self.first) as u32) + 1
    }
}

/// The directory companion files are placed in, as far as the replayers can tell.
const DIR: &str = "/mod";
const MIN_RATE: u32 = 1000;
const MAX_RATE: u32 = 768_000;

/// The name of the format that would play `data`, if any open replayer claims it, without
/// starting it. Packed modules (PowerPacker and others) and the ProWizard-packed
/// ProTracker variants are unpacked for the check, as [`Song::open`] does.
///
/// ```
/// assert_eq!(amigaplayer_rs::detect(b"not a module"), None);
/// ```
pub fn detect(data: &[u8]) -> Option<String> {
    // a replayer's detector that panics (a bug) claims nothing
    catch_unwind(AssertUnwindSafe(|| open::detect(data))).ok().flatten()
}

/// As [`detect`], told the module's file name and given the companion files
/// [`Song::open_with`] takes. Where the module alone passes more than one replayer's check
/// (Infogrames and Rob Hubbard 2 modules pass each other's; the sample bank beside the module
/// decides), the song is started to see which of them plays it, and that format is named; if
/// it does not start, the answer is [`detect`]'s.
///
/// ```
/// assert_eq!(amigaplayer_rs::detect_with("x", b"not a module", &[]), None);
/// ```
pub fn detect_with(name: &str, data: &[u8], companions: &[(&str, &[u8])]) -> Option<String> {
    let first = detect(data)?;
    if !catch_unwind(AssertUnwindSafe(|| open::claimed_by_several(data))).unwrap_or(false) {
        return Some(first);
    }
    match Song::open_with(name, data, companions, 48000) {
        Ok(s) => Some(s.format().to_string()),
        Err(_) => Some(first),
    }
}

/// Whether `data` is a custom module: an AmigaOS executable that carries its own replay
/// routine behind a DeliTracker player header (`CUST.*` files, mostly game music ripped with
/// its player). Packed files are unpacked for the check; nothing of the module is run.
/// [`Song::open_opts`] plays one when [`Options::allow_custom`] is set, unless an open
/// replayer claims the file first ([`detect`] says so).
///
/// ```
/// assert!(amigaplayer_rs::detect_custom(&amigaplayer_rs::testdata::minimal_custom()));
/// assert!(!amigaplayer_rs::detect_custom(&amigaplayer_rs::testdata::minimal_mod()));
/// ```
#[cfg(feature = "custom-modules")]
pub fn detect_custom(data: &[u8]) -> bool {
    catch_unwind(AssertUnwindSafe(|| open::detect_custom(data))).unwrap_or(false)
}

/// Choices for [`Song::open_opts`]. `Options::default()` is what [`Song::open`] does.
///
/// ```
/// # #[cfg(feature = "custom-modules")] {
/// use amigaplayer_rs::{Options, Song};
/// let module = amigaplayer_rs::testdata::minimal_custom();
/// let mut song = Song::open_opts("cust.tune", &module, &[], 44100, &Options::new().allow_custom(true))?;
/// assert_eq!(song.format(), "Custom");
/// let mut pcm = vec![0i16; 2 * 4410];
/// song.render_i16(&mut pcm);
/// assert!(song.error().is_none());
/// # }
/// # Ok::<(), amigaplayer_rs::Error>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    custom: bool,
}

impl Options {
    /// The defaults: open replayers only.
    pub fn new() -> Options {
        Options::default()
    }

    /// Lets a custom module (see [`detect_custom`]) play: its own 68k code runs on the emulated
    /// 68000, in the emulated machine's memory. Off by default, and only in builds with the
    /// `custom-modules` feature. No external DeliPlayer or Eagleplayer is ever loaded by a [`Song`].
    ///
    /// What bounds that code: it sees the companion files given to `open_opts` and no file of
    /// the host; every call into it has an instruction budget, after which (as after a CPU
    /// exception) the song stops, renders silence and reports [`Error::Guest`] through
    /// [`Song::error`]. See `docs/LIBRARY.md`, "Custom modules".
    #[cfg(feature = "custom-modules")]
    pub fn allow_custom(mut self, on: bool) -> Options {
        self.custom = on;
        self
    }
}

/// The furthest [`Song::seek`] goes, in seconds.
pub const MAX_SEEK_SECS: f64 = 86_400.0;

/// The names of the formats the library can play: every open replayer, the built-in
/// ProTracker, the recorded-audio decoders (AIFF, IFF 8SVX, IFF 16SV, RIFF WAVE, MPEG audio), the AY, NSF and SNDH
/// players and the PC tracker loaders of the libxmp port. Several
/// of them may be reported by [`Song::format`] and [`detect`] under a more specific
/// name (a version or variant).
pub fn formats() -> Vec<&'static str> {
    let mut v = vec![
        "ProTracker",
        "AIFF",
        "IFF 8SVX",
        "IFF 16SV",
        "RIFF WAVE",
        "MPEG audio layer 1",
        "MPEG audio layer 2",
        "MPEG audio layer 3",
        "ZXAYEMUL",
        "ZXAYAMAD",
        "NSF",
        "SNDH",
    ];
    #[cfg(feature = "sid")]
    v.extend(["PSID", "RSID"]);
    v.extend(crate::replay::REPLAYERS.iter().map(|o| o.name));
    v.extend(crate::xmp::LOADERS.iter().filter(|l| !crate::xmp::LEFT_TO_OTHER_PLAYERS.contains(&l.name)).map(|l| l.name));
    v
}

/// A module being played on the emulated Amiga (or the PC tracker mixer) at a fixed
/// sample rate. Output is stereo, interleaved left/right.
///
/// A `Song` is `Send` (move it to the audio thread) but not `Sync`.
///
/// ```
/// use amigaplayer_rs::Song;
/// # let module = amigaplayer_rs::testdata::minimal_mod();
/// let mut song = Song::open("tune.mod", &module, 48000)?;
/// let mut pcm = vec![0i16; 2 * 4800];          // 0.1 s of stereo frames
/// song.render_i16(&mut pcm);
/// assert_eq!(song.format(), "ProTracker");
/// # Ok::<(), amigaplayer_rs::Error>(())
/// ```
pub struct Song {
    e: open::Song,
    name: String,
    data: Vec<u8>,
    files: Vec<(PathBuf, Vec<u8>)>,
    rate: u32,
    poisoned: bool,
    opts: Options,
    /// Why the song stopped by itself, if it did.
    failed: Option<Error>,
    /// Title, format and subsongs as last seen while the song played (a song whose 68k code
    /// failed has none of its own any more).
    meta: (String, String, Subsongs),
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
}

fn clean_name(name: &str) -> String {
    let n = name.rsplit(['/', '\\']).next().unwrap_or("");
    if n.is_empty() { "module".to_string() } else { n.to_string() }
}

fn clean_rel(rel: &str) -> PathBuf {
    rel.split(['/', '\\']).filter(|s| !s.is_empty() && *s != "." && *s != "..").collect()
}

impl Song {
    /// Starts `data` at `rate` Hz. `name` is the module's file name (a path is cut down to
    /// its last component): some formats are told apart by it, and companion files are
    /// looked up beside it.
    pub fn open(name: &str, data: &[u8], rate: u32) -> Result<Song> {
        Song::open_with(name, data, &[], rate)
    }

    /// As [`Song::open`], with the companion files some formats read given in memory, each by
    /// its path relative to the module (`"smpl.tune"` for a TFMX module, `"Instruments/Piano"`
    /// for an IFF-SMUS score). Nothing is read from the file system.
    pub fn open_with(name: &str, data: &[u8], companions: &[(&str, &[u8])], rate: u32) -> Result<Song> {
        Song::open_opts(name, data, companions, rate, &Options::default())
    }

    /// As [`Song::open_with`], with [`Options`]: custom modules, when
    /// `Options::allow_custom` is set (`custom-modules` builds).
    pub fn open_opts(name: &str, data: &[u8], companions: &[(&str, &[u8])], rate: u32, opts: &Options) -> Result<Song> {
        if !(MIN_RATE..=MAX_RATE).contains(&rate) {
            return Err(Error::InvalidRate(rate));
        }
        let name = clean_name(name);
        // (the format recognised by its file name needs it to be told)
        let known = {
            // restores the caller's name even if detection panics
            struct Restore(String);
            impl Drop for Restore {
                fn drop(&mut self) {
                    crate::replay::adpcmmono::set_name(&self.0);
                }
            }
            let _restore = Restore(crate::replay::adpcmmono::current_name());
            crate::replay::adpcmmono::set_name(&name);
            catch_unwind(AssertUnwindSafe(|| open::detect(data).is_some() || Self::custom(data, opts)))
        };
        let known = known.map_err(|p| Error::Internal(panic_text(p)))?;
        if !known {
            return Err(Error::NotRecognised);
        }
        let files: Vec<(PathBuf, Vec<u8>)> =
            companions.iter().map(|(rel, d)| (PathBuf::from(DIR).join(clean_rel(rel)), d.to_vec())).collect();
        let e = Self::start(&name, data, &files, rate, opts)?;
        let meta = Self::meta_of(&e);
        Ok(Song { e, name, data: data.to_vec(), files, rate, poisoned: false, opts: opts.clone(), failed: None, meta })
    }

    #[cfg(feature = "custom-modules")]
    fn custom(data: &[u8], opts: &Options) -> bool {
        opts.custom && open::detect_custom(data)
    }

    #[cfg(not(feature = "custom-modules"))]
    fn custom(_data: &[u8], _opts: &Options) -> bool {
        false
    }

    fn meta_of(e: &open::Song) -> (String, String, Subsongs) {
        let (first, last, current) = e.subsongs();
        (e.title().to_string(), e.format().to_string(), Subsongs { first, last, current })
    }

    /// Notes a failure of the song's 68k code, and keeps what describes the song while it plays.
    fn check(&mut self) {
        if self.failed.is_some() {
            return;
        }
        match self.e.failure() {
            Some(why) => self.failed = Some(Error::Guest(why.to_string())),
            None => {
                let (first, last, current) = self.e.subsongs();
                self.meta.2 = Subsongs { first, last, current };
            }
        }
    }

    fn start(name: &str, data: &[u8], files: &[(PathBuf, Vec<u8>)], rate: u32, opts: &Options) -> Result<open::Song> {
        let path = PathBuf::from(DIR).join(name);
        let open = || {
            #[cfg(feature = "custom-modules")]
            if opts.custom {
                return open::Song::open_custom(&path, data.to_vec(), files.to_vec(), rate);
            }
            let _ = opts;
            open::Song::open_with(&path, data.to_vec(), files.to_vec(), rate)
        };
        match catch_unwind(AssertUnwindSafe(open)) {
            Ok(Ok(s)) => Ok(s),
            Ok(Err(e)) => Err(match e.downcast_ref::<crate::engine::MissingCompanion>() {
                Some(m) => Error::MissingCompanion { format: m.format.clone(), wanted: m.wanted.clone() },
                None => Error::Invalid(e.to_string()),
            }),
            Err(p) => Err(Error::Internal(panic_text(p))),
        }
    }

    /// The sample rate the song renders at.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// The format that plays the module ("ProTracker", "Sonic Arranger", "Impulse Tracker"...).
    pub fn format(&self) -> &str {
        &self.meta.1
    }

    /// The module's own title, or its file name when it has none.
    pub fn title(&self) -> &str {
        &self.meta.0
    }

    /// Why the song has stopped by itself and renders silence, if it has: [`Error::Guest`]
    /// when the 68k code of a custom module failed (a CPU exception, a call that did not
    /// return), [`Error::Internal`] when a replayer panicked. `None` while all is well.
    /// A host that streams checks this (as it checks [`Song::songend`]) after a render.
    pub fn error(&self) -> Option<&Error> {
        self.failed.as_ref()
    }

    /// The library functions the 68k code of a custom module has called so far that the
    /// emulated AmigaOS does not implement, each once, in the order first called (at most 64).
    /// Empty for every other format. A [`MissingCall::fatal`] one has also stopped the song
    /// ([`Song::error`] is then [`Error::Guest`] naming it); the others are warnings: the
    /// module plays on, and may play differently than on an Amiga. Kept across
    /// [`Song::set_subsong`]; [`Song::restart`] and [`Song::seek`] start the list again (the
    /// code makes its calls again as it plays again).
    pub fn missing_calls(&self) -> &[MissingCall] {
        &self.e.engine().m.os.missing_calls
    }

    /// The first, last and current subsong.
    pub fn subsongs(&self) -> Subsongs {
        if self.failed.is_some() {
            return self.meta.2;
        }
        let (first, last, current) = self.e.subsongs();
        Subsongs { first, last, current }
    }

    /// Restarts at subsong `n`. Errors with [`Error::SubsongOutOfRange`] outside the range.
    pub fn set_subsong(&mut self, n: u16) -> Result<()> {
        let s = self.subsongs();
        if !(s.first..=s.last).contains(&n) {
            return Err(Error::SubsongOutOfRange { requested: n, first: s.first, last: s.last });
        }
        if self.poisoned {
            return Err(Error::Internal("the song is unusable after an earlier panic".into()));
        }
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let r = match catch_unwind(AssertUnwindSafe(|| self.e.set_subsong(n))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(Error::Invalid(e.to_string())),
            Err(p) => {
                self.poisoned = true;
                let e = Error::Internal(panic_text(p));
                self.failed = Some(e.clone());
                return Err(e);
            }
        };
        self.check();
        match (&self.failed, r) {
            (Some(e), _) => Err(e.clone()),
            (None, r) => r,
        }
    }

    /// The replayer has reported the end of the song. Many Amiga players never do: they
    /// loop for ever, so a host should also stop at a time limit or after a stretch of silence.
    pub fn songend(&self) -> bool {
        !self.poisoned && self.e.songend()
    }

    /// Seconds rendered since the subsong started (or since [`Song::restart`]).
    pub fn elapsed_secs(&self) -> f64 {
        self.e.elapsed_secs()
    }

    /// The length of the (current sub)song in seconds, when the format states it: recordings
    /// (AIFF) and the PC tracker formats (libxmp's scan). `None` for the Amiga replayers, which
    /// do not know how long they will play.
    pub fn duration_hint(&self) -> Option<f64> {
        match &self.e.engine().playing {
            Some(Playing { backend: Backend::Pcm(x), .. }) => x.duration_hint(),
            Some(Playing { backend: Backend::Mix(x), .. }) => {
                let ms = x.duration_ms(x.subsong());
                (ms > 0).then(|| ms as f64 / 1000.0)
            }
            _ => None,
        }
    }

    /// Renders interleaved stereo frames in -1.0..=1.0 into `out` (`out.len() / 2` frames; a
    /// trailing odd sample is left untouched). Returns the number of frames written. After
    /// [`Song::songend`] the replayer may keep going (it loops) or fall silent.
    pub fn render_f32(&mut self, out: &mut [f32]) -> usize {
        let n = out.len() & !1;
        if self.poisoned {
            out[..n].fill(0.0);
            return n / 2;
        }
        match catch_unwind(AssertUnwindSafe(|| self.e.render(&mut out[..n]))) {
            Ok(()) => {
                self.check();
                n / 2
            }
            Err(p) => {
                self.poisoned = true;
                self.failed = Some(Error::Internal(panic_text(p)));
                out[..n].fill(0.0);
                n / 2
            }
        }
    }

    /// As [`Song::render_f32`], converted to signed 16 bit (rounded, clipped). Returns the
    /// number of stereo *frames* written, so `2 * n` samples of `out` are filled.
    pub fn render_i16(&mut self, out: &mut [i16]) -> usize {
        let n = out.len() & !1;
        let mut buf = [0f32; 2048];
        let mut done = 0;
        while done < n {
            let k = (n - done).min(buf.len());
            self.render_f32(&mut buf[..k]);
            for (o, &x) in out[done..done + k].iter_mut().zip(&buf[..k]) {
                *o = (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
            }
            done += k;
        }
        n / 2
    }

    /// Starts the current subsong again from its beginning. Rendering after a restart gives
    /// the same samples as the first time (the emulation is deterministic).
    pub fn restart(&mut self) -> Result<()> {
        let sub = self.subsongs().current;
        self.e = Self::start(&self.name, &self.data, &self.files, self.rate, &self.opts)?;
        self.poisoned = false;
        self.failed = None;
        self.meta = Self::meta_of(&self.e);
        if sub != self.subsongs().current {
            self.set_subsong(sub)?;
        }
        Ok(())
    }

    /// Positions the song `secs` seconds into the current subsong by restarting and rendering
    /// (and discarding) up to there: the cost is that of playing it, tens to hundreds of times
    /// faster than real time. Stops early at [`Song::songend`].
    ///
    /// `secs` must be finite and not negative; it is capped at [`MAX_SEEK_SECS`] (24 hours) so
    /// that a huge value cannot play a looping song for ever. Otherwise `Error::Invalid`.
    pub fn seek(&mut self, secs: f64) -> Result<()> {
        if !secs.is_finite() || secs < 0.0 {
            return Err(Error::Invalid(format!("cannot seek to {secs} seconds")));
        }
        self.restart()?;
        let target = (secs.min(MAX_SEEK_SECS) * self.rate as f64) as u64;
        let mut buf = [0f32; 4096];
        let mut done = 0u64;
        while done < target && !self.songend() && self.failed.is_none() {
            let k = ((target - done) as usize).min(buf.len() / 2);
            self.render_f32(&mut buf[..k * 2]);
            done += k as u64;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn song_is_send() {
        assert_send::<Song>();
        assert_send::<Error>();
    }

    #[test]
    fn seek_rejects_bad_times() {
        let m = crate::testdata::minimal_mod();
        let mut s = Song::open("a.mod", &m, 8000).unwrap();
        for t in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(matches!(s.seek(t), Err(Error::Invalid(_))), "{t}");
        }
        assert!(s.seek(0.5).is_ok());
    }

    #[test]
    fn rate_is_checked() {
        let m = crate::testdata::minimal_mod();
        assert_eq!(Song::open("a.mod", &m, 0).err(), Some(Error::InvalidRate(0)));
    }
}
