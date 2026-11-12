//! XPK-MASH (LZ77 with Huffman coded literal runs). Ported from LibAncient's
//! `MashDecompressor.cs` (NostalgicPlayer, Thomas Neumann, MIT), a port of ancient's
//! `MASHDecompressor.cpp` (Teemu Suutari, BSD-2-Clause); see LICENSE-ancient.

use crate::decrunch::ancient::{ByteSource, Fwd, FwdOut, Huffman, Msb, Vlc};

pub(super) fn decompress(chunk: &[u8], out: &mut [u8]) -> Option<()> {
    let mut input = Fwd::new(chunk, 0, chunk.len())?;
    let mut bits = Msb::default();
    let raw = out.len();
    let mut o = FwdOut::new(out, 0, raw)?;
    let lit_dec = Huffman::with(&[
        (1, 0b000000, 0),
        (2, 0b000010, 1),
        (3, 0b000110, 2),
        (4, 0b001110, 3),
        (5, 0b011110, 4),
        (6, 0b111110, 5),
        (6, 0b111111, 6),
    ])?;
    let vlc = Vlc::new(&[5, 7, 9, 10, 11, 12, 13, 14]);
    macro_rules! rb {
        ($n:expr) => {
            bits.bits8(&mut input, $n)?
        };
    }
    while !o.eof() {
        let mut lit_len = lit_dec.decode(|| bits.bits8(&mut input, 1))?;
        if lit_len == 6 {
            let mut lit_bits = 1u32;
            while lit_bits <= 17 {
                if rb!(1) == 0 {
                    break;
                }
                lit_bits += 1;
            }
            if lit_bits == 17 {
                return None;
            }
            lit_len = rb!(lit_bits).wrapping_add(1u32.wrapping_shl(lit_bits)).wrapping_add(4);
        }
        for _ in 0..lit_len {
            o.write(input.byte()?)?;
        }
        let (count, distance);
        if rb!(1) != 0 {
            let mut count_bits = 1u32;
            while count_bits <= 16 {
                if rb!(1) == 0 {
                    break;
                }
                count_bits += 1;
            }
            if count_bits == 16 {
                return None;
            }
            count = rb!(count_bits).wrapping_add(1u32.wrapping_shl(count_bits)).wrapping_add(2);
            let base = rb!(3);
            distance = vlc.decode(base, |n| bits.bits8(&mut input, n))?;
        } else if rb!(1) != 0 {
            let base = rb!(3);
            distance = vlc.decode(base, |n| bits.bits8(&mut input, n))?;
            count = 3;
        } else {
            distance = rb!(9);
            count = 2;
        }
        // The C++ "hacks to make it work": a zero distance at the end of the stream, and an
        // almost systematic extra byte at the end.
        if distance == 0 && o.eof() {
            break;
        }
        let count = (count as usize).min(raw - o.written());
        o.copy(distance as usize, count)?;
    }
    Some(())
}
