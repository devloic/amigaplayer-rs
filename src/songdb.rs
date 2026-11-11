//! Song lengths and metadata from the song database of audacious-uade (feature `songdb`).
//!
//! The Amiga replayers do not know how long a song is ([`Song::duration_hint`](crate::Song::duration_hint)
//! is `None` for them). audacious-uade's database does, for some 476,000 module files: the
//! length of each subsong as its tools measured it and how the song ended there, the format
//! and channel count, and author, album, publisher and year where its sources name them. A
//! file is found by a [`key`] made of its bytes, whatever it is called.
//!
//! ```no_run
//! use amigaplayer_rs::songdb::SongDb;
//!
//! let db = SongDb::open("/usr/share/audacious-uade/songdb")?;   // once: it is Send + Sync
//! let bytes = std::fs::read("AHX.Cruisin")?;
//! if let Some(info) = db.lookup(&bytes) {
//!     println!("{} by {} ({})", info.format, info.author, info.year);
//!     if let Some(s) = info.subsong(0) {
//!         println!("{} ms, ended by: {}", s.length_ms, s.status);
//!     }
//! }
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # The data is not included
//!
//! The reader is the crate `amigaplayer-songdb` (`crates/songdb`: a vendored copy of the
//! `au-songdb` crate of audacious-uade-rust, itself a port of audacious-uade by Matti
//! Tiainen; LGPL-2.1-or-later OR GPL-2.0-or-later, credits in each file and its README).
//! The **database** is the directory `conf/songdb/` of audacious-uade (`xxh32idx.tsv`,
//! `songlengths.tsv`, `modinfos.tsv`, `metadata.tsv`; 9.5 MB), licensed **CC BY-NC-SA 4.0**:
//! it is not in this repository, not compiled in and not downloaded by any build. Get it
//! from <https://github.com/mvtiaine/audacious-uade> or an audacious-uade package and give
//! its directory to [`SongDb::open`]; its licence (non-commercial, share-alike, attribution)
//! is then yours to keep.
//!
//! # What the lengths are
//!
//! They were measured with other replayers (UADE and the players audacious-uade bundles), at
//! the moment their song-end detection fired ([`SongEndStatus`] says which one). They are a
//! good estimate for a progress bar or a playlist; as a place to cut the audio they can come
//! before the last note has died away, or differ from where this crate's replayer ends.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

pub use amigaplayer_songdb::SongEndStatus;

/// The database key of a file's bytes: 12 lowercase hex digits, the XXH32 (seed 0) of the
/// first 262,144 bytes, then the low 16 bits of the length:
/// `format!("{:08x}{:04x}", xxh32(&data[..min(len, 262144)], 0), len & 0xffff)`.
/// The bytes are the file's as it is on disk (not unpacked).
///
/// ```
/// assert_eq!(amigaplayer_rs::songdb::key(b""), "02cc5d050000");
/// ```
pub fn key(data: &[u8]) -> String {
    amigaplayer_songdb::key(data)
}

/// What the database knows about one subsong.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SubSongInfo {
    /// The subsong's number, in the numbering of UADE (which is this crate's for the formats
    /// both play; see [`Subsongs`](crate::Subsongs)).
    pub subsong: u8,
    /// Its length in milliseconds (stored in steps of 20 ms; at most 87 minutes).
    pub length_ms: u32,
    /// How the song ended when it was measured: by the player's own song end, a detected
    /// loop, silence, a time limit, ... `status.as_str()` is the database tools' spelling
    /// (`"player"`, `"player+volume"`, `"loop"`, `"silence"`, ...).
    pub status: SongEndStatus,
    /// The subsong renders the same audio as an earlier one of the module.
    pub is_duplicate: bool,
}

/// What the database knows about one file. Strings are empty, and numbers 0, where it knows
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Info {
    /// The file's [`key`].
    pub key: String,
    /// The subsongs, ascending and consecutive. Empty only for a database loaded without
    /// its lengths.
    pub subsongs: Vec<SubSongInfo>,
    /// The format as the database's tools name it ("AHX v2", "Protracker", "TFMX Pro", ...):
    /// not the names of [`Song::format`](crate::Song::format).
    pub format: String,
    /// The number of channels, 0 if not known.
    pub channels: u8,
    /// The author (several are joined with " & ").
    pub author: String,
    /// The album: the game, demo or disk the module comes from.
    pub album: String,
    /// The publisher or group.
    pub publisher: String,
    /// The year, 0 if not known.
    pub year: u16,
}

impl Info {
    /// The entry of subsong `n`, if the database has one.
    pub fn subsong(&self, n: u16) -> Option<&SubSongInfo> {
        self.subsongs.iter().find(|s| s.subsong as u16 == n)
    }
}

/// The song database, loaded into memory (about 19 MB for the real one, in about a fifth
/// of a second). Lookups take `&self`: share one between threads.
pub struct SongDb {
    db: amigaplayer_songdb::SongDb,
}

fn damaged(dir: &Path, why: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, format!("the song database in {} is damaged: {why}", dir.display()))
}

impl SongDb {
    /// Loads the database from `dir`, which holds `xxh32idx.tsv`, `songlengths.tsv`,
    /// `modinfos.tsv` and `metadata.tsv`. A missing file is an error (`NotFound`), and so is
    /// a file that does not parse (`InvalidData`): never a panic.
    pub fn open(dir: impl AsRef<Path>) -> std::io::Result<SongDb> {
        let dir = dir.as_ref();
        match catch_unwind(AssertUnwindSafe(|| amigaplayer_songdb::SongDb::init(dir, &[]))) {
            Ok(Ok(db)) if db.is_empty() => Err(damaged(dir, "xxh32idx.tsv is empty")),
            Ok(Ok(db)) => Ok(SongDb { db }),
            Ok(Err(e)) => Err(std::io::Error::new(e.kind(), format!("song database in {}: {e}", dir.display()))),
            Err(p) => {
                let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
                Err(damaged(dir, why.as_deref().unwrap_or("a file does not parse")))
            }
        }
    }

    /// The number of files the database knows.
    pub fn len(&self) -> usize {
        // (row 0 is a reserved key)
        self.db.len().saturating_sub(1)
    }

    /// Whether the database knows no file.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What the database knows about the file with these bytes (the file as it is on disk),
    /// or `None` if it is not in it.
    pub fn lookup(&self, data: &[u8]) -> Option<Info> {
        self.lookup_key(&key(data))
    }

    /// As [`SongDb::lookup`], by [`key`]: 12 hex digits (either case). Anything else is `None`.
    pub fn lookup_key(&self, key: &str) -> Option<Info> {
        if key.len() != 12 || !key.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let key = key.to_ascii_lowercase();
        // (the reserved row)
        if key == "000000000000" {
            return None;
        }
        let i = catch_unwind(AssertUnwindSafe(|| self.db.lookup(&key))).ok().flatten()?;
        let (format, channels) = i.modinfo.map_or((String::new(), 0), |m| (m.format, m.channels));
        let m = i.metadata.unwrap_or_default();
        Some(Info {
            key,
            subsongs: i
                .subsongs
                .into_iter()
                .map(|s| SubSongInfo { subsong: s.subsong, length_ms: s.songend.length, status: s.songend.status, is_duplicate: s.is_duplicate })
                .collect(),
            format,
            channels,
            author: m.author,
            album: m.album,
            publisher: m.publisher,
            year: m.year,
        })
    }
}

impl std::fmt::Debug for SongDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SongDb({} files)", self.len())
    }
}
