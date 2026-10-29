//! zlib inflate, for the loaders that unpack a container (`tinfl_decompress_mem_to_heap`
//! of libxmp's `miniz_tinfl.c` with `TINFL_FLAG_PARSE_ZLIB_HEADER`).
//!
//! Ported from libxmp 4.7.4 (miniz 2.2.0, public domain; libxmp MIT licence,
//! Copyright (C) 1996-2026 Claudio Matsuoka and Hipolito Carraro Jr, see
//! LICENSE-libxmp). It accepts and refuses what tinfl does: the zlib header
//! checks, over-subscribed or incomplete Huffman tables, a distance reaching
//! before the output, a missing or wrong Adler-32.

const LEN_BASE: [u16; 31] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258, 0, 0,
];
const LEN_EXTRA: [u8; 31] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0, 0, 0];
const DIST_BASE: [u16; 32] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193,
    12289, 16385, 24577, 0, 0,
];
const DIST_EXTRA: [u8; 32] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 0, 0];
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u32,
    n: u32,
}

impl Bits<'_> {
    fn get(&mut self, k: u32) -> Option<u32> {
        while self.n < k {
            let b = *self.d.get(self.pos)?;
            self.pos += 1;
            self.buf |= (b as u32) << self.n;
            self.n += 8;
        }
        let v = if k == 0 { 0 } else { self.buf & ((1u32 << k) - 1) };
        self.buf >>= k;
        self.n -= k;
        Some(v)
    }

    /// Drops the bits left in the current byte; the next read starts on a byte boundary.
    fn align(&mut self) {
        // whole bytes still buffered go back to the input
        self.pos -= (self.n / 8) as usize;
        self.buf = 0;
        self.n = 0;
    }
}

/// A canonical Huffman table: codes per length and the symbols in code order.
struct Huff {
    count: [u16; 16],
    symbol: Vec<u16>,
}

impl Huff {
    /// tinfl's table check: an over-subscribed set, or an incomplete one with
    /// more than one symbol, is refused.
    fn new(lens: &[u8]) -> Option<Huff> {
        let mut count = [0u16; 16];
        for &l in lens {
            count[l as usize] += 1;
        }
        let used = lens.len() as u32 - count[0] as u32;
        let mut total = 0u32;
        for l in 1..16 {
            total += (count[l] as u32) << (16 - l);
        }
        if total != 65536 && used > 1 {
            return None;
        }
        let mut offs = [0u16; 17];
        for l in 1..16 {
            offs[l + 1] = offs[l] + count[l];
        }
        let mut symbol = vec![0u16; lens.len()];
        for (s, &l) in lens.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Some(Huff { count, symbol })
    }

    fn decode(&self, b: &mut Bits) -> Option<usize> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.get(1)? as i32;
            let count = self.count[len] as i32;
            if code - count < first {
                return self.symbol.get((index + (code - first)) as usize).map(|&s| s as usize);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        None
    }
}

/// Inflates a zlib stream; `None` where tinfl would fail.
pub fn zlib_decompress(src: &[u8]) -> Option<Vec<u8>> {
    if src.len() < 2 {
        return None;
    }
    let (h0, h1) = (src[0] as u32, src[1] as u32);
    if (h0 * 256 + h1) % 31 != 0 || h1 & 32 != 0 || h0 & 15 != 8 || (1u32 << (8 + (h0 >> 4))) > 32768 {
        return None;
    }
    let mut b = Bits { d: src, pos: 2, buf: 0, n: 0 };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = b.get(1)?;
        match b.get(2)? {
            0 => {
                b.align();
                let len = u16::from_le_bytes([*b.d.get(b.pos)?, *b.d.get(b.pos + 1)?]);
                let nlen = u16::from_le_bytes([*b.d.get(b.pos + 2)?, *b.d.get(b.pos + 3)?]);
                if len != !nlen {
                    return None;
                }
                b.pos += 4;
                out.extend_from_slice(b.d.get(b.pos..b.pos + len as usize)?);
                b.pos += len as usize;
            }
            t @ (1 | 2) => {
                let (lit, dist);
                if t == 1 {
                    let mut l = [8u8; 288];
                    l[144..256].fill(9);
                    l[256..280].fill(7);
                    lit = Huff::new(&l)?;
                    dist = Huff::new(&[5u8; 32])?;
                } else {
                    let nlit = 257 + b.get(5)? as usize;
                    let ndist = 1 + b.get(5)? as usize;
                    let ncl = 4 + b.get(4)? as usize;
                    let mut cl = [0u8; 19];
                    for &o in CL_ORDER.iter().take(ncl) {
                        cl[o] = b.get(3)? as u8;
                    }
                    let clh = Huff::new(&cl)?;
                    let mut lens = vec![0u8; nlit + ndist];
                    let mut i = 0;
                    while i < nlit + ndist {
                        let s = clh.decode(&mut b)?;
                        if s < 16 {
                            lens[i] = s as u8;
                            i += 1;
                        } else {
                            let (v, rep) = match s {
                                16 => {
                                    if i == 0 {
                                        return None;
                                    }
                                    (lens[i - 1], 3 + b.get(2)? as usize)
                                }
                                17 => (0, 3 + b.get(3)? as usize),
                                _ => (0, 11 + b.get(7)? as usize),
                            };
                            if i + rep > nlit + ndist {
                                return None;
                            }
                            lens[i..i + rep].fill(v);
                            i += rep;
                        }
                    }
                    lit = Huff::new(&lens[..nlit])?;
                    dist = Huff::new(&lens[nlit..])?;
                }
                loop {
                    let s = lit.decode(&mut b)?;
                    if s < 256 {
                        out.push(s as u8);
                    } else if s == 256 {
                        break;
                    } else {
                        let s = s - 257;
                        let len = LEN_BASE[s] as usize + b.get(LEN_EXTRA[s] as u32)? as usize;
                        let ds = dist.decode(&mut b)?;
                        if ds >= 30 {
                            return None;
                        }
                        let d = DIST_BASE[ds] as usize + b.get(DIST_EXTRA[ds] as u32)? as usize;
                        if d > out.len() {
                            return None;
                        }
                        let from = out.len() - d;
                        for k in 0..len {
                            out.push(out[from + k]);
                        }
                    }
                    if out.len() > (1 << 30) {
                        return None;
                    }
                }
            }
            _ => return None,
        }
        if last != 0 {
            break;
        }
    }
    // the Adler-32, big endian, from the next byte boundary
    b.align();
    let a = b.d.get(b.pos..b.pos + 4)?;
    let want = u32::from_be_bytes([a[0], a[1], a[2], a[3]]);
    let (mut s1, mut s2) = (1u32, 0u32);
    for &x in &out {
        s1 = (s1 + x as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    if (s2 << 16) + s1 != want {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn inflates_stored_and_checks_adler() {
        let stored = unhex("7801010500faff68656c6c6f062c0215");
        assert_eq!(zlib_decompress(&stored).unwrap(), b"hello");
        let mut bad = stored.clone();
        bad[15] ^= 1;
        assert!(zlib_decompress(&bad).is_none());
        assert!(zlib_decompress(&stored[..15]).is_none());
    }
}
