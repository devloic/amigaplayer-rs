//! XPK-SMPL (Huffman coded deltas, for samples). Ported from LibAncient's
//! `SmplDecompressor.cs` (NostalgicPlayer, Thomas Neumann, MIT), a port of ancient's
//! `SMPLDecompressor.cpp` (Teemu Suutari, BSD-2-Clause); see LICENSE-ancient.

use crate::decrunch::ancient::{be16, Fwd, FwdOut, Huffman, Msb};

pub(super) fn check(chunk: &[u8]) -> Option<()> {
    (be16(chunk, 0)? == 1).then_some(())
}

pub(super) fn decompress(chunk: &[u8], out: &mut [u8]) -> Option<()> {
    check(chunk)?;
    let mut input = Fwd::new(chunk, 2, chunk.len())?;
    let mut bits = Msb::default();
    let n = out.len();
    let mut o = FwdOut::new(out, 0, n)?;
    let mut dec = Huffman::default();
    for i in 0..256 {
        let mut len = bits.bits8(&mut input, 4)?;
        if len == 0 {
            continue;
        }
        if len == 15 {
            len = bits.bits8(&mut input, 4)? + 15;
        }
        let code = bits.bits8(&mut input, len)?;
        dec.insert(len, code, i)?;
    }
    let mut accum = 0u8;
    while !o.eof() {
        let code = dec.decode(|| bits.bits8(&mut input, 1))?;
        accum = accum.wrapping_add(code as u8);
        o.write(accum)?;
    }
    Some(())
}
