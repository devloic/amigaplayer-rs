// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Vendored from audacious-uade-rust (https://fg.lolo.asia/adminlolo/audacious-uade-rust.git,
// commit c3ea17194c768987c48751cd97d638ccb04b1472), file crates/au-common/src/songend.rs, whose crate
// manifest gives the licence above. It is a Rust port of src/common/songend.h of
// audacious-uade by Matti Tiainen (https://github.com/mvtiaine/audacious-uade), which carries:
//     SPDX-License-Identifier: LGPL-2.1-or-later
//     Copyright (C) 2014-2026 Matti Tiainen <mvtiaine@cc.hut.fi>
// (audacious-uade as a whole is GPL-2.0-or-later; its src/common/ and src/songdb/ are also
// offered under LGPL-2.1-or-later). The texts are COPYING and COPYING.LGPL beside Cargo.toml.
// Changes for amigaplayer-rs: none.

//! Port of `common/songend.h`.
//!
//! How a decode finished, and how long it ran. The status is what the song
//! database records alongside a length, so its string spellings are a
//! *serialisation format*, not display text — see [`SongEndStatus::as_str`].

/// Port of `common::SongEnd::Status`.
///
/// The discriminants are pinned to the C's because they are written to and
/// read back from the song database as integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i8)]
pub enum SongEndStatus {
    Error = -1,
    None = 0,
    Player = 1,
    Timeout = 2,
    DetectSilence = 3,
    // The remaining variants only arise when precalculating.
    DetectLoop = 4,
    DetectVolume = 5,
    DetectRepeat = 6,
    PlayerPlusSilence = 7,
    PlayerPlusVolume = 8,
    LoopPlusSilence = 9,
    LoopPlusVolume = 10,
    NoSound = 11,
}

impl SongEndStatus {
    /// Port of `SongEnd::status_string`.
    ///
    /// These spellings are parsed back by the songdb tooling, so they are
    /// fixed strings rather than anything derived from the variant name —
    /// note `"silence"` for [`Self::DetectSilence`] and the `+` in the
    /// compound forms.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::None => "none",
            Self::Player => "player",
            Self::Timeout => "timeout",
            Self::DetectSilence => "silence",
            Self::DetectLoop => "loop",
            Self::DetectVolume => "volume",
            Self::DetectRepeat => "repeat",
            Self::PlayerPlusSilence => "player+silence",
            Self::PlayerPlusVolume => "player+volume",
            Self::LoopPlusSilence => "loop+silence",
            Self::LoopPlusVolume => "loop+volume",
            Self::NoSound => "nosound",
        }
    }

    /// Inverse of [`Self::as_str`], for reading the database back.
    ///
    /// The C has no such function — it only ever writes these — but the
    /// round trip is what makes the differential tests possible, and a
    /// spelling that fails to round-trip here is a spelling that would have
    /// silently corrupted a lookup.
    ///
    /// Also available as [`std::str::FromStr`], which is what callers
    /// parsing a database row will reach for; this form returns `Option`
    /// because there is only one way to fail.
    pub fn from_status_string(s: &str) -> Option<Self> {
        Some(match s {
            "error" => Self::Error,
            "none" => Self::None,
            "player" => Self::Player,
            "timeout" => Self::Timeout,
            "silence" => Self::DetectSilence,
            "loop" => Self::DetectLoop,
            "volume" => Self::DetectVolume,
            "repeat" => Self::DetectRepeat,
            "player+silence" => Self::PlayerPlusSilence,
            "player+volume" => Self::PlayerPlusVolume,
            "loop+silence" => Self::LoopPlusSilence,
            "loop+volume" => Self::LoopPlusVolume,
            "nosound" => Self::NoSound,
            _ => return None,
        })
    }

    /// Port of the C's implicit `int8_t` conversion, used where the status
    /// crosses into stored data.
    pub const fn as_i8(self) -> i8 {
        self as i8
    }

    pub fn from_i8(v: i8) -> Option<Self> {
        Some(match v {
            -1 => Self::Error,
            0 => Self::None,
            1 => Self::Player,
            2 => Self::Timeout,
            3 => Self::DetectSilence,
            4 => Self::DetectLoop,
            5 => Self::DetectVolume,
            6 => Self::DetectRepeat,
            7 => Self::PlayerPlusSilence,
            8 => Self::PlayerPlusVolume,
            9 => Self::LoopPlusSilence,
            10 => Self::LoopPlusVolume,
            11 => Self::NoSound,
            _ => return None,
        })
    }
}

/// The error [`std::str::FromStr`] yields for an unrecognised spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseSongEndStatusError;

impl std::fmt::Display for ParseSongEndStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not a song-end status spelling")
    }
}

impl std::error::Error for ParseSongEndStatusError {}

impl std::str::FromStr for SongEndStatus {
    type Err = ParseSongEndStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_status_string(s).ok_or(ParseSongEndStatusError)
    }
}

impl std::fmt::Display for SongEndStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Port of `common::SongEnd`.
///
/// `length` is in milliseconds and defaults to 0, as in the C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SongEnd {
    pub status: SongEndStatus,
    pub length: u32,
}

impl SongEnd {
    pub const fn new(status: SongEndStatus, length: u32) -> Self {
        Self { status, length }
    }

    pub const fn status_string(&self) -> &'static str {
        self.status.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The C's `status_string` is a `switch` with an `assert(false)` default,
    /// so every declared variant must have a spelling. This is the Rust
    /// equivalent of that assertion: it walks the full variant list rather
    /// than spot-checking, so adding a variant without a spelling fails here
    /// instead of at a database write.
    const ALL: &[SongEndStatus] = &[
        SongEndStatus::Error,
        SongEndStatus::None,
        SongEndStatus::Player,
        SongEndStatus::Timeout,
        SongEndStatus::DetectSilence,
        SongEndStatus::DetectLoop,
        SongEndStatus::DetectVolume,
        SongEndStatus::DetectRepeat,
        SongEndStatus::PlayerPlusSilence,
        SongEndStatus::PlayerPlusVolume,
        SongEndStatus::LoopPlusSilence,
        SongEndStatus::LoopPlusVolume,
        SongEndStatus::NoSound,
    ];

    #[test]
    fn discriminants_match_the_c() {
        // Pinned literally: these integers reach the song database, so
        // deriving them from position in ALL would make the test agree with
        // whatever the enum happens to say.
        let expected: [(SongEndStatus, i8); 13] = [
            (SongEndStatus::Error, -1),
            (SongEndStatus::None, 0),
            (SongEndStatus::Player, 1),
            (SongEndStatus::Timeout, 2),
            (SongEndStatus::DetectSilence, 3),
            (SongEndStatus::DetectLoop, 4),
            (SongEndStatus::DetectVolume, 5),
            (SongEndStatus::DetectRepeat, 6),
            (SongEndStatus::PlayerPlusSilence, 7),
            (SongEndStatus::PlayerPlusVolume, 8),
            (SongEndStatus::LoopPlusSilence, 9),
            (SongEndStatus::LoopPlusVolume, 10),
            (SongEndStatus::NoSound, 11),
        ];
        for (status, want) in expected {
            assert_eq!(status.as_i8(), want, "{status:?}");
            assert_eq!(SongEndStatus::from_i8(want), Some(status));
        }
        assert_eq!(expected.len(), ALL.len(), "ALL is missing a variant");
    }

    #[test]
    fn spellings_match_the_c() {
        let expected = [
            "error",
            "none",
            "player",
            "timeout",
            "silence",
            "loop",
            "volume",
            "repeat",
            "player+silence",
            "player+volume",
            "loop+silence",
            "loop+volume",
            "nosound",
        ];
        for (status, want) in ALL.iter().zip(expected) {
            assert_eq!(status.as_str(), want, "{status:?}");
        }
    }

    #[test]
    fn spellings_round_trip_and_are_distinct() {
        let mut seen = std::collections::HashSet::new();
        for &status in ALL {
            assert!(seen.insert(status.as_str()), "duplicate spelling {status:?}");
            assert_eq!(SongEndStatus::from_status_string(status.as_str()), Some(status));
            // ...and via the trait, which is what row parsing will use.
            assert_eq!(status.as_str().parse::<SongEndStatus>(), Ok(status));
        }
    }

    #[test]
    fn out_of_range_discriminants_are_rejected() {
        // The C's `default: assert(false); return "error"` would silently
        // become "error" in a release build. Rust makes it representable as
        // absence instead, so callers must decide.
        assert_eq!(SongEndStatus::from_i8(12), None);
        assert_eq!(SongEndStatus::from_i8(-2), None);
        assert_eq!(SongEndStatus::from_status_string("player+repeat"), None);
        assert!("player+repeat".parse::<SongEndStatus>().is_err());
    }
}
