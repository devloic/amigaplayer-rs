//! File-level decrunchers: [`packer`] and [`unpack`].
//!
//! Many Amiga modules were distributed crunched with a general-purpose packer. These functions
//! recognise those wrappers and undo them, independently of any player. [`Song::open`]
//! (crate::Song::open) and [`detect`](crate::detect) already unpack their input for the check,
//! so a host only needs this module to get at the raw bytes (to save them, to inspect them, or
//! to pass them to something else).
//!
//! | [`Packer`]                | recognised by |
//! |---------------------------|---------------|
//! | [`PowerPacker`](Packer::PowerPacker) | `PP20` |
//! | [`XpkSqsh`](Packer::XpkSqsh)         | XPK container with the SQSH method |
//! | [`Mmcmp`](Packer::Mmcmp)             | `ziRCONia` MMCMP header |
//! | [`PackIce`](Packer::PackIce)         | `Ice!` / `ICE!` |
//! | [`StoneCracker`](Packer::StoneCracker) | `S404` |
//!
//! ```
//! use amigaplayer_rs::unpack::{packer, unpack};
//!
//! assert_eq!(packer(b"plain data, not packed by anything"), None);
//! assert!(unpack(b"plain data, not packed by anything").unwrap().is_none());
//! ```
//!
//! The unpackers are ports of libxmp 4.7.4's depackers (Pack-Ice, XPK SQSH, StoneCracker S404,
//! MMCMP) and of a PowerPacker decruncher; see the licence notes in the source files.

use crate::decrunch::{self, Kind};
use crate::song::{Error, Result};

/// A packer whose wrapper this crate can undo. New variants may be added in a minor release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Packer {
    /// PowerPacker 2.0 (`PP20`).
    PowerPacker,
    /// XPK container using the SQSH method.
    XpkSqsh,
    /// Music Module Compressor (MMCMP).
    Mmcmp,
    /// Pack-Ice (`Ice!` and `ICE!`).
    PackIce,
    /// StoneCracker 4.04 (`S404`).
    StoneCracker,
}

impl Packer {
    /// A display name for the packer.
    pub fn name(self) -> &'static str {
        match self {
            Packer::PowerPacker => "PowerPacker",
            Packer::XpkSqsh => "XPK SQSH",
            Packer::Mmcmp => "MMCMP",
            Packer::PackIce => "Pack-Ice",
            Packer::StoneCracker => "StoneCracker S404",
        }
    }
}

impl std::fmt::Display for Packer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl From<Kind> for Packer {
    fn from(k: Kind) -> Packer {
        match k {
            Kind::PowerPacker => Packer::PowerPacker,
            Kind::XpkSqsh => Packer::XpkSqsh,
            Kind::Mmcmp => Packer::Mmcmp,
            Kind::PackIce => Packer::PackIce,
            Kind::StoneCracker => Packer::StoneCracker,
        }
    }
}

/// Which packer made `data`, going by its header only; nothing is unpacked. `None` when `data`
/// does not look packed (a header that matches by chance is still reported: [`unpack`] tells
/// whether it really is).
pub fn packer(data: &[u8]) -> Option<Packer> {
    decrunch::identify(data).map(Packer::from)
}

/// Unpacks `data`. `Ok(None)` when it is not packed with a known packer, `Ok(Some((packer,
/// bytes)))` with the unpacked bytes otherwise, and [`Error::Invalid`] when the header says it
/// is packed but the data is corrupt or truncated.
pub fn unpack(data: &[u8]) -> Result<Option<(Packer, Vec<u8>)>> {
    match decrunch::unpack_identified(data) {
        None => Ok(None),
        Some((kind, Some(bytes))) => Ok(Some((kind.into(), bytes))),
        Some((kind, None)) => {
            Err(Error::Invalid(format!("{} data is corrupt or truncated", Packer::from(kind))))
        }
    }
}
