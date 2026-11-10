//! Amiga module replay as a library: the open replayers of amigaplayer-rs.
//!
//! The crate plays about a hundred Amiga music formats (ProTracker and its relatives, TFMX,
//! Future Composer, Sonic Arranger, MED, AHX/HVL, Digital Mugician, Oktalyzer, ...) and the PC
//! tracker formats of libxmp (XM, S3M, IT, ...), plus AIFF recordings. The Amiga replayers are
//! Rust ports that drive an emulated Paula (and the CIAs, and for a few formats the 68000)
//! register write for register write as the originals do, so the output is that of the
//! machine, filters included. The result is a [`Song`]: open some bytes, ask for audio.
//!
//! ```
//! use amigaplayer_rs::Song;
//!
//! # let module_bytes = amigaplayer_rs::testdata::minimal_mod();
//! // `module_bytes` is the content of a module file; the name is only a hint.
//! let mut song = Song::open("tune.mod", &module_bytes, 44100)?;
//! println!("{} ({})", song.title(), song.format());
//!
//! let mut pcm = vec![0i16; 2 * 44100];   // one second, interleaved stereo
//! while !song.songend() && song.elapsed_secs() < 1.0 {
//!     song.render_i16(&mut pcm);
//!     // hand `pcm` to the sound card or a file
//! }
//! # Ok::<(), amigaplayer_rs::Error>(())
//! ```
//!
//! # What is in the API
//!
//! * [`detect`] and [`formats`]: ask what would play some bytes, without starting it.
//! * [`Song`]: [`open`](Song::open) / [`open_with`](Song::open_with) (companion files such as
//!   TFMX `smpl.*`, Mark Cooksey `mcs.*` are passed in memory: nothing touches the file system),
//!   [`render_f32`](Song::render_f32) / [`render_i16`](Song::render_i16),
//!   [`subsongs`](Song::subsongs) / [`set_subsong`](Song::set_subsong),
//!   [`songend`](Song::songend), [`duration_hint`](Song::duration_hint),
//!   [`restart`](Song::restart) / [`seek`](Song::seek), [`title`](Song::title),
//!   [`format`](Song::format).
//! * [`unpack`]: recognise and undo the file-level packers (PowerPacker, Pack-Ice, XPK SQSH,
//!   MMCMP, StoneCracker) without playing anything.
//! * [`Error`]: a typed, `#[non_exhaustive]` error. Garbage input gives an `Err`, not a panic (a replayer bug that panics is caught and reported as `Error::Internal`, except under `panic = "abort"`).
//!
//! Rendering is deterministic: the same bytes, rate and subsong give the same samples on every
//! run and platform that has IEEE floats.
//!
//! # Cargo features
//!
//! | feature    | default | what it adds |
//! |------------|---------|--------------|
//! | `host-68k` | yes     | loading and running the original 68k DeliPlayers, Eagleplayers and DeliGenies (the application's mode of operation); not needed by the library |
//! | `ui`       | yes     | the SDL3 player window and the `amigaplayer-rs` binary (implies `host-68k`) |
//! | `native`   | yes     | the players recompiled to Rust, when generated from the user's own player files (implies `host-68k`) |
//!
//! A library user wants `default-features = false`:
//!
//! ```toml
//! [dependencies]
//! amigaplayer = { git = "https://fg.lolo.asia/adminlolo/amigaplayer-rs.git", package = "amigaplayer-rs", default-features = false }
//! ```
//!
//! # Stability
//!
//! The crate is 0.x: within a `0.y` series the items documented here keep their meaning and
//! signatures (new ones may appear; [`Error`] and [`Subsongs`] are `#[non_exhaustive]`); a
//! `0.(y+1)` may change them. The modules in the source tree (`engine`, `machine`, `replay`,
//! `paula`, ...) are `#[doc(hidden)]`: they exist for the application, the WebAssembly and C
//! crates and the tools of this repository, and may change at any time. In a build with
//! `default-features = false` the 68k player hosting is compiled out of the engine, but the
//! emulated machine stays: the open replayers run on it.
//!
//! # Licence
//!
//! GPL-2.0-or-later, as a whole and in every file; individual files keep the attribution and
//! the licence texts of the sources they are ported from (`LICENSE-*` in the repository:
//! NostalgicPlayer, libxmp, pt2-clone, ahx2play, ... all GPL-compatible). **Linking this crate
//! makes your program a combined work that must be distributed under GPL-2.0-or-later
//! compatible terms** (GPL-2.0-or-later, GPL-3.0-or-later, AGPL-3.0-or-later; not
//! GPL-2.0-only).
//!
//! # Environment variables
//!
//! The emulation reads a few `DELI_*` environment variables for debugging and for comparing
//! with the original players (for example `DELI_WRITE_LOG`, `DELI_DL_CPU`). A host should not set
//! them; unset they give the documented behaviour.

#![warn(missing_docs)]

mod song;
pub mod unpack;

pub use song::{detect, formats, Error, Result, Song, Subsongs};

pub mod testdata;

// The internals: public for the application, the WebAssembly and C crates and the tools of
// this repository, hidden from the documentation, and not covered by any stability promise.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod aiff;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod amplifier;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod bus;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod cia;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod deli;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod decrunch;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod eagle;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod engine;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod heap;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod hunk;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod machine;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod notes;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod open;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod os;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod paula;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod paulaout;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod protracker;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod prowizard;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod replay;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod trace;
#[doc(hidden)]
#[allow(missing_docs)]
pub mod xmp;

#[cfg(feature = "host-68k")]
use std::path::{Path, PathBuf};

/// The sample rate the application renders at.
#[doc(hidden)]
pub const SAMPLE_RATE: u32 = 48000;

/// The DeliPlayers directory: `$DELIPLAYERS`, `~/.config/amigaplayer-rs/DeliPlayers`, the older
/// `~/.config/deliplayer/DeliPlayers`, then `./DeliPlayers`, whichever exists first.
#[cfg(feature = "host-68k")]
#[doc(hidden)]
pub fn default_players_dir() -> Option<PathBuf> {
    let mut c = Vec::new();
    if let Ok(p) = std::env::var("DELIPLAYERS") {
        c.push(PathBuf::from(p));
    }
    if let Ok(h) = std::env::var("HOME") {
        c.push(Path::new(&h).join(".config/amigaplayer-rs/DeliPlayers"));
        c.push(Path::new(&h).join(".config/deliplayer/DeliPlayers"));
    }
    c.push(PathBuf::from("DeliPlayers"));
    c.into_iter().find(|p| p.is_dir())
}

/// The DeliGenies directory: `$DELIGENIES`, `~/.config/amigaplayer-rs/DeliGenies`, the older
/// `~/.config/deliplayer/DeliGenies`, then `./DeliGenies`, whichever exists first.
#[cfg(feature = "host-68k")]
#[doc(hidden)]
pub fn default_genies_dir() -> Option<PathBuf> {
    let mut c = Vec::new();
    if let Ok(p) = std::env::var("DELIGENIES") {
        c.push(PathBuf::from(p));
    }
    if let Ok(h) = std::env::var("HOME") {
        c.push(Path::new(&h).join(".config/amigaplayer-rs/DeliGenies"));
        c.push(Path::new(&h).join(".config/deliplayer/DeliGenies"));
    }
    c.push(PathBuf::from("DeliGenies"));
    c.into_iter().find(|p| p.is_dir())
}

/// The Eagleplayers directory: `$EAGLEPLAYERS`, `~/.config/amigaplayer-rs/Eagleplayers`, the older
/// `~/.config/deliplayer/Eagleplayers`, then `./Eagleplayers`, whichever exists first.
#[cfg(feature = "host-68k")]
#[doc(hidden)]
pub fn default_eagleplayers_dir() -> Option<PathBuf> {
    let mut c = Vec::new();
    if let Ok(p) = std::env::var("EAGLEPLAYERS") {
        c.push(PathBuf::from(p));
    }
    if let Ok(h) = std::env::var("HOME") {
        c.push(Path::new(&h).join(".config/amigaplayer-rs/Eagleplayers"));
        c.push(Path::new(&h).join(".config/deliplayer/Eagleplayers"));
    }
    c.push(PathBuf::from("Eagleplayers"));
    c.into_iter().find(|p| p.is_dir())
}
