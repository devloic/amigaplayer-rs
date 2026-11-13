//! Imploder (File Imploder, `IMP!` and the variants other crunchers and productions used:
//! `ATN!`, `EDAM`, `M.H.`, `BDPI`, `CHFI`, `RDC9`, `Dupa`, `FLT!`, `PARA`) and XPK-IMPL, its XPK
//! sub-library. Translated from `IMPDecompressor.cpp` of ancient (Teemu Suutari, BSD-2-Clause;
//! see LICENSE-ancient). LibAncient, NostalgicPlayer's C# port, has no Imploder, so this is a
//! direct port of the C++, with the same checks (the file checksum included, for the headers
//! whose checksum constant is known).
//!
//! Errors are `None`: every read and copy is bounds checked, the output is allocated from the
//! header (at most `MAX_RAW`) and written exactly once from its end.

use super::alloc_zeroed;
use super::ancient::{be16, be32, BwdOut, ByteSource, Huffman, Msb, MAX_RAW};

/// Bytes after `end` in a file: 0x2e of tables, then the checksum. XPK has no checksum.
const TAIL: usize = 0x32;
const TAIL_XPK: usize = 0x2e;

/// The checksum constant of a header (0: the files are not checked), `None` when it is not one.
fn addition(hdr: &[u8]) -> Option<u32> {
    Some(match hdr {
        b"ATN!" | b"EDAM" | b"IMP!" | b"M.H." => 7,
        b"BDPI" => 0x6e8,
        b"CHFI" => 0xfe4,
        b"RDC9" | b"Dupa" | b"FLT!" | b"PARA" => 0,
        _ => return None,
    })
}

struct Header {
    raw: usize,
    end: usize,
}

fn header(d: &[u8], tail: usize, xpk: bool) -> Option<Header> {
    if d.len() < tail {
        return None;
    }
    let raw = be32(d, 4)? as usize;
    let end = be32(d, 8)? as usize;
    if end & 1 != 0 || end < 0xc || end.checked_add(tail)? > d.len() {
        return None;
    }
    if !xpk && (raw == 0 || raw > MAX_RAW || end > MAX_RAW) {
        return None;
    }
    Some(Header { raw, end })
}

fn file_header(d: &[u8]) -> Option<Header> {
    let add = addition(d.get(..4)?)?;
    let h = header(d, TAIL, false)?;
    if add != 0 {
        let sum = d[..h.end + 0x2e]
            .chunks_exact(2)
            .fold(add, |s, w| s.wrapping_add(u32::from(w[0]) << 8 | u32::from(w[1])));
        if be32(d, h.end + 0x2e)? != sum {
            return None;
        }
    }
    Some(h)
}

pub(crate) fn test(d: &[u8]) -> bool {
    file_header(d).is_some()
}

/// The packed stream, read backwards from `end` with ancient's odd ordering of the first twelve
/// bytes.
struct Input<'a> {
    buf: &'a [u8],
    cur: usize,
    refo: usize,
}

impl<'a> Input<'a> {
    fn new(buf: &'a [u8], end: usize) -> Option<Self> {
        let mut cur = end;
        if *buf.get(end.checked_add(16)?)? & 0x80 == 0 {
            cur = cur.checked_sub(1)?;
        }
        Some(Input { buf, cur, refo: end })
    }
}

impl ByteSource for Input<'_> {
    fn byte(&mut self) -> Option<u8> {
        if self.cur == 0 {
            return None;
        }
        self.cur -= 1;
        let i = self.cur;
        let at = match i {
            0..=3 => self.refo + i + 8,
            4..=7 => self.refo + i,
            8..=11 => self.refo + i - 8,
            _ => i,
        };
        self.buf.get(at).copied()
    }
    fn be16(&mut self) -> Option<u32> {
        let hi = u32::from(self.byte()?);
        Some(hi << 8 | u32::from(self.byte()?))
    }
    fn be32(&mut self) -> Option<u32> {
        Some(self.be16()? << 16 | self.be16()?)
    }
}

fn run(d: &[u8], end: usize, out: &mut [u8]) -> Option<()> {
    let mut input = Input::new(d, end)?;
    let mut bits = Msb::default();
    // The anchor bit is not always at the right place: take the empty bits out.
    let half = *d.get(end + 17)?;
    for i in 0..7u8 {
        if half & (1 << i) != 0 {
            bits.reset(u32::from(half >> (i + 1)), 7 - i);
            break;
        }
    }
    let n = out.len();
    let mut o = BwdOut::new(out, 0, n)?;

    let mut dist_values = [[0u32; 4]; 2];
    for i in 0..8 {
        dist_values[i >> 2][i & 3] = be16(d, end + 18 + i * 2)?;
    }
    let mut dist_bits = [[0u32; 4]; 3];
    for i in 0..12 {
        dist_bits[i >> 2][i & 3] = u32::from(*d.get(end + 34 + i)?);
    }
    // Length, distance and literal counts are intertwined.
    let lld = Huffman::with(&[
        (1, 0b00000, 0),
        (2, 0b00010, 1),
        (3, 0b00110, 2),
        (4, 0b01110, 3),
        (5, 0b11110, 4),
        (5, 0b11111, 5),
    ])?;
    let lld2 = Huffman::with(&[(1, 0b00, 0), (2, 0b10, 1), (2, 0b11, 2)])?;
    const LIT_LENGTHS: [u32; 4] = [6, 10, 10, 18];
    const LIT_BITS: [[u32; 4]; 3] = [[1, 1, 1, 1], [2, 3, 3, 4], [4, 5, 7, 14]];

    let mut lit = be32(d, end + 12)?;
    loop {
        for _ in 0..lit {
            o.write(input.byte()?)?;
        }
        if o.eof() {
            return Some(());
        }
        let i0 = lld.decode(|| bits.bits8(&mut input, 1))?;
        let selector = i0.min(3) as usize;
        let mut count = i0 + 2;
        if count == 6 {
            count += bits.bits8(&mut input, 3)?;
        } else if count == 7 {
            count = u32::from(input.byte()?);
            if count == 0 {
                return None;
            }
        }
        let i1 = lld2.decode(|| bits.bits8(&mut input, 1))? as usize;
        lit = (i1 as u32) * 2;
        if lit == 4 {
            lit = LIT_LENGTHS[selector];
        }
        lit += bits.bits8(&mut input, LIT_BITS[i1][selector])?;
        let i2 = lld2.decode(|| bits.bits8(&mut input, 1))? as usize;
        let base = if i2 != 0 { dist_values[i2 - 1][selector] } else { 0 };
        let distance = 1 + base + bits.bits8(&mut input, dist_bits[i2][selector])?;
        o.copy(distance as usize, count as usize)?;
    }
}

pub(crate) fn depack(d: &[u8]) -> Option<Vec<u8>> {
    let h = file_header(d)?;
    let mut out = alloc_zeroed(h.raw)?;
    run(d, h.end, &mut out)?;
    Some(out)
}

/// The sub-decompressor constructor's checks of XPK-IMPL (the chunk is the file without its
/// last four bytes, the checksum).
pub(super) fn check(chunk: &[u8]) -> Option<()> {
    header(chunk, TAIL_XPK, true).map(|_| ())
}

pub(super) fn decompress(chunk: &[u8], out: &mut [u8]) -> Option<()> {
    let h = header(chunk, TAIL_XPK, true)?;
    if h.raw != out.len() {
        return None;
    }
    run(chunk, h.end, out)
}
