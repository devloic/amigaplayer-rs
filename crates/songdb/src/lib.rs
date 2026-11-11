// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Vendored from audacious-uade-rust (https://fg.lolo.asia/adminlolo/audacious-uade-rust.git,
// commit c3ea17194c768987c48751cd97d638ccb04b1472), file crates/au-songdb/src/lib.rs, whose crate
// manifest gives the licence above. It is a Rust port of src/songdb/songdb.h of
// audacious-uade by Matti Tiainen (https://github.com/mvtiaine/audacious-uade), which carries:
//     SPDX-License-Identifier: LGPL-2.1-or-later
//     Copyright (C) 2014-2026 Matti Tiainen <mvtiaine@cc.hut.fi>
// (audacious-uade as a whole is GPL-2.0-or-later; its src/common/ and src/songdb/ are also
// offered under LGPL-2.1-or-later). The texts are COPYING and COPYING.LGPL beside Cargo.toml.
// Changes for amigaplayer-rs: the `au_common` items the database uses are modules of this crate
// (`songend`, `strings`); `xxh32` and `key()` are added.

//! Port of audacious-uade's `src/songdb/`.
//!
//! A read-mostly database of ~476,000 modules: song lengths per subsong,
//! format and channel count, and author/album/publisher/year metadata. It
//! ships as four `.tsv` files in `conf/songdb/` (9.5 MB) that are not really
//! tab-separated — see [`codec`] for the three encodings they actually use.
//!
//! # Keys
//!
//! Every entry is keyed by a 12-character lowercase hex string built as
//! `XXH32` of the first [`XXH_MAX_BYTES`] of the file, followed by the low
//! 16 bits of its size — 48 bits in total. The plugins assemble it with
//! `common::to_hex`, whose zero padding is what keeps the two halves
//! unambiguous.
//!
//! # Layout
//!
//! | Rust | C |
//! |---|---|
//! | [`codec`] | the anonymous namespace at the top of `songdb.cc` |
//! | [`db`] | the rest of `songdb.cc` |
//! | [`blacklist`] | `blacklist.cc` |
//! | [`songend`] | `common/songend.h` |
//! | [`strings`] | `common/strings.h` (the part the database uses) |
//!
//! [`xxh32`] and [`key`] are additions of this vendored copy: the hash the keys are
//! made of, and the key of a file's bytes.
//!
//! # The data is not part of this crate
//!
//! The four `.tsv` files are audacious-uade's `conf/songdb/`, licensed CC BY-NC-SA 4.0
//! (not a software licence, and not compatible with distributing them in a GPL work):
//! they are read from a directory at run time ([`SongDb::init`]) and are neither
//! shipped with, compiled into nor downloaded by this crate.

pub mod blacklist;
pub mod codec;
pub mod db;
pub mod songend;
pub mod strings;
pub mod write;
pub mod xxh32;

pub use db::SongDb;
pub use songend::{SongEnd, SongEndStatus};

/// Port of `songdb::XXH_MAX_BYTES`: only the first 256 KiB of a file are
/// hashed, so large modules stay cheap to identify.
pub const XXH_MAX_BYTES: usize = 256 * 1024;

/// The database key of a file: 12 lowercase hex digits, `XXH32` (seed 0) of its first
/// [`XXH_MAX_BYTES`] bytes, then the low 16 bits of its length. (An addition of this
/// vendored copy: the plugins of audacious-uade build the key the same way.)
pub fn key(data: &[u8]) -> String {
    format!("{:08x}{:04x}", xxh32::xxh32(&data[..data.len().min(XXH_MAX_BYTES)], 0), data.len() & 0xffff)
}

/// Port of `songdb::Source`.
///
/// The discriminants match the C's, which uses them to index both its
/// `tsvfiles` table and its `initialized[]` flags. `Songlengths` starts at 1
/// because 0 is the internal hash-index source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Source {
    Songlengths = 1,
    ModInfos = 2,
    Metadata = 3,
}

/// Port of `songdb::MetaData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetaData {
    pub author: String,
    pub album: String,
    pub publisher: String,
    /// A full year (e.g. 1991), or 0 when unknown. Stored in the database as
    /// a one-byte offset from 1900.
    pub year: u16,
}

/// Port of `songdb::ModInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModInfo {
    pub format: String,
    pub channels: u8,
}

/// Port of `songdb::SubSongInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubSongInfo {
    pub subsong: u8,
    pub songend: SongEnd,
    /// Set when this subsong renders identically to another in the module.
    pub is_duplicate: bool,
}

/// Port of `songdb::Info`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Info {
    pub subsongs: Vec<SubSongInfo>,
    pub modinfo: Option<ModInfo>,
    pub metadata: Option<MetaData>,
}

impl Info {
    /// Port of `Info::min_subsong`. Returns -1 when there are no subsongs,
    /// as the C does.
    pub fn min_subsong(&self) -> i32 {
        self.subsongs.first().map_or(-1, |s| s.subsong as i32)
    }

    /// Port of `Info::max_subsong`.
    pub fn max_subsong(&self) -> i32 {
        self.subsongs.last().map_or(-1, |s| s.subsong as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(n: u8) -> SubSongInfo {
        SubSongInfo {
            subsong: n,
            songend: SongEnd::new(SongEndStatus::Player, 1000),
            is_duplicate: false,
        }
    }

    #[test]
    fn subsong_bounds_report_minus_one_when_empty() {
        let empty = Info::default();
        assert_eq!(empty.min_subsong(), -1);
        assert_eq!(empty.max_subsong(), -1);
    }

    /// The bounds come from the ends of the list, not from its length — a
    /// module whose subsongs start at 2 must report 2, not 0 or 1.
    #[test]
    fn subsong_bounds_read_the_ends_of_the_list() {
        let info = Info { subsongs: vec![sub(2), sub(3), sub(4)], ..Default::default() };
        assert_eq!(info.min_subsong(), 2);
        assert_eq!(info.max_subsong(), 4);

        let single = Info { subsongs: vec![sub(7)], ..Default::default() };
        assert_eq!(single.min_subsong(), 7);
        assert_eq!(single.max_subsong(), 7);
    }

    #[test]
    fn source_discriminants_match_the_c() {
        assert_eq!(Source::Songlengths as u8, 1);
        assert_eq!(Source::ModInfos as u8, 2);
        assert_eq!(Source::Metadata as u8, 3);
    }

    #[test]
    fn hash_window_is_256_kib() {
        assert_eq!(XXH_MAX_BYTES, 262_144);
    }
}
