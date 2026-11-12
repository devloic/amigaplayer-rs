//! PowerPlayer Music Compressor (`SFHD` 1.x, `SFCD` 2.x with delta coding): the LH code of
//! XPK-LHLB behind a 12-byte header. Ported from LibAncient's `PowerPlayerDecompressor.cs`
//! (NostalgicPlayer, Thomas Neumann, MIT), a port of ancient's `PMCDecompressor.cpp` (Teemu
//! Suutari, BSD-2-Clause); see LICENSE-ancient.

use super::alloc_zeroed;
use super::ancient::{be32, delta_decode, MAX_RAW};
use super::xpk::lhlb::decompress_lhlib;

/// (version, unpacked size, end of the packed data).
fn header(d: &[u8]) -> Option<(u32, usize, usize)> {
    if d.len() < 12 {
        return None;
    }
    let ver = match &d[..4] {
        b"SFHD" => 0,
        b"SFCD" => 1,
        _ => return None,
    };
    let raw = be32(d, 4)? as usize;
    if raw == 0 || raw > MAX_RAW {
        return None;
    }
    let packed = (be32(d, 8)? as usize).checked_add(12)?;
    if packed > d.len() || packed > 0x800_0000 {
        return None;
    }
    Some((ver, raw, packed))
}

pub(crate) fn test(d: &[u8]) -> bool {
    header(d).is_some()
}

pub(crate) fn depack(d: &[u8]) -> Option<Vec<u8>> {
    let (ver, raw, packed) = header(d)?;
    let mut out = alloc_zeroed(raw)?;
    let length = decompress_lhlib(&mut out, &d[12..packed])?;
    if length == 0 {
        return None;
    }
    if ver != 0 {
        delta_decode(&mut out);
        let fill = out[length - 1];
        out[length..].fill(fill);
    }
    // Version 1.x: the rest stays zero.
    Some(out)
}
