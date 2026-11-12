//! XPK-LHLB (the LH algorithm of lh.library, an adaptive Huffman LZ77), also the code of
//! PowerPlayer. Ported from LibAncient's `LhDecompressor.cs` (NostalgicPlayer, Thomas Neumann,
//! MIT), a port of ancient's `LHDecompressor.cpp` (Teemu Suutari, BSD-2-Clause); see
//! LICENSE-ancient.

use crate::decrunch::ancient::{DynHuffman, Fwd, FwdOut, Msb, Vlc};

/// ancient's `decompressLhLib`: decodes into `out` until the end code or until `out` is full,
/// and returns the number of bytes written.
pub(crate) fn decompress_lhlib(out: &mut [u8], packed: &[u8]) -> Option<usize> {
    let mut input = Fwd::new(packed, 0, packed.len())?;
    let mut bits = Msb::default();
    let n = out.len();
    let mut o = FwdOut::new(out, 0, n)?;
    // Same logic as Cholok's Pascal implementation. Differences to LH1: no halving of the
    // frequencies at 32k, 317 instead of 314 entries, no end code, different distance/count
    // logic.
    let mut dec = DynHuffman::new(317);
    let vlc = Vlc::new(&[5, 5, 6, 6, 6, 7, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10]);
    while !o.eof() {
        let code = dec.decode(|| bits.bits16(&mut input, 1))?;
        if code == 316 {
            break;
        }
        if dec.max_freq() < 0x8000 {
            dec.update(code)?;
        }
        if code < 256 {
            o.write(code as u8)?;
        } else {
            let base = bits.bits16(&mut input, 4)?;
            let distance = vlc.decode(base, |c| bits.bits16(&mut input, c))?;
            let count = (code - 255) as usize;
            // The LH bug on a zero distance: zeros are written.
            if distance != 0 {
                o.copy(distance as usize, count)?;
            } else {
                for _ in 0..count {
                    o.write(0)?;
                }
            }
        }
    }
    Some(o.written())
}

pub(super) fn decompress(chunk: &[u8], out: &mut [u8]) -> Option<()> {
    let len = decompress_lhlib(out, chunk)?;
    out[len..].fill(0);
    Some(())
}
