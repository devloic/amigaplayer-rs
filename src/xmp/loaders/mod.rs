//! libxmp's format loaders, one module per format.
//!
//! A loader ports a `loaders/*_load.c` of libxmp 4.7.4 (MIT licence, Copyright
//! (C) 1996-2026 Claudio Matsuoka and Hipolito Carraro Jr, see LICENSE-libxmp):
//! `test` takes the whole file, `load` fills a `ModuleData` left as
//! `libxmp_load_prologue` sets it, and is registered in `xmp::LOADERS`.

pub mod c669;
pub mod mtm;
