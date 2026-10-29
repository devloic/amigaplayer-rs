//! The MUSE container of Jazz Jackrabbit 2 J2B files (`loaders/muse_load.c`): a
//! 24-byte header and a zlib stream holding a Galaxy Music System 5.0 or 4.0 module.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use super::{gal4, gal5};
use crate::xmp::inflate::zlib_decompress;
use crate::xmp::load::Hio;
use crate::xmp::module::ModuleData;

/// `LIBXMP_DEPACK_LIMIT`.
const DEPACK_LIMIT: usize = 512 << 20;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 8 && &d[..4] == b"MUSE" && {
        let r = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
        r == 0xdeadbeaf || r == 0xdeadbabe
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let inlen = f.size();
    if inlen < 24 || inlen >= DEPACK_LIMIT {
        return Err(());
    }
    f.seek(24);
    let input = f.read_vec(inlen - 24);
    let out = zlib_decompress(&input).ok_or(())?;
    let mut g = Hio::new(&out);
    if gal5::test(&out) {
        gal5::load(m, &mut g)
    } else if gal4::test(&out) {
        gal4::load(m, &mut g)
    } else {
        Err(())
    }
}
