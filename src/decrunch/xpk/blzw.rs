//! XPK-BLZW (LZW). Ported from LibAncient's `BlzwDecompressor.cs` (NostalgicPlayer, Thomas
//! Neumann, MIT), a port of ancient's `BLZWDecompressor.cpp` (Teemu Suutari, BSD-2-Clause);
//! see LICENSE-ancient.

use crate::decrunch::ancient::{be16, Fwd, FwdOut, Lzw, Msb};

pub(super) fn check(chunk: &[u8]) -> Option<(u32, u32)> {
    let max_bits = be16(chunk, 0)?;
    if !(9..=20).contains(&max_bits) {
        return None;
    }
    Some((max_bits, be16(chunk, 2)? + 5))
}

pub(super) fn decompress(chunk: &[u8], out: &mut [u8]) -> Option<()> {
    let (max_bits, stack_len) = check(chunk)?;
    let mut input = Fwd::new(chunk, 4, chunk.len())?;
    let mut bits = Msb::default();
    let n = out.len();
    let mut o = FwdOut::new(out, 0, n)?;
    let mut code_bits = 9u32;
    let first = bits.bits8(&mut input, code_bits)?;
    let mut dec = Lzw::new(1 << max_bits, 259, stack_len, first);
    dec.write(first, false, |b| o.write(b))?;
    while !o.eof() {
        let code = bits.bits8(&mut input, code_bits)?;
        match code {
            256 => return None,
            257 => {
                code_bits = 9;
                let first = bits.bits8(&mut input, code_bits)?;
                dec.reset(first);
                dec.write(first, false, |b| o.write(b))?;
            }
            258 => {
                if code_bits >= 24 {
                    return None;
                }
                code_bits += 1;
            }
            _ => {
                let add_new = !dec.is_literal(code);
                dec.write(code, add_new, |b| o.write(b))?;
                dec.add(code);
            }
        }
    }
    Some(())
}
