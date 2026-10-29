//! libxmp's format loaders, one module per format.
//!
//! A loader ports a `loaders/*_load.c` of libxmp 4.7.4 (MIT licence, Copyright
//! (C) 1996-2026 Claudio Matsuoka and Hipolito Carraro Jr, see LICENSE-libxmp):
//! `test` takes the whole file, `load` fills a `ModuleData` left as
//! `libxmp_load_prologue` sets it, and is registered in `xmp::LOADERS`.

pub mod amf;
pub mod asylum;
pub mod c669;
pub mod gal4;
pub mod gal5;
pub mod dt;
pub mod far;
pub mod gdm;
pub mod imf;
pub mod liq;
pub mod mgt;
pub mod masi;
pub mod masi16;
pub mod mdl;
pub mod mfp;
pub mod mtm;
pub mod no;
pub mod muse;
pub mod ult;
pub mod rtm;
pub mod stx;
