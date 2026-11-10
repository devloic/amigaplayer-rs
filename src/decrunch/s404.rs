//! StoneCracker S404 depacker, a port of libxmp 4.7.4's `s404_dec.c`.
//!
//! Public domain, from libxmp. The original notice reads:
//!
//! StoneCracker S404 algorithm data decompression routine
//! (c) 2006 Jouni 'Mr.Spiv' Korhonen. The code is in public domain.
//!
//! Modified for xmp by Claudio Matsuoka, Jan 2010.

use super::{alloc_zeroed, DEPACK_LIMIT};

struct Bits<'a> {
    word: u32,
    left: i32,
    /// Byte index of the next 16-bit word (counted back from the end).
    src: isize,
    /// Start of the compressed data.
    org: isize,
    buf: &'a [u8],
}

impl Bits<'_> {
    fn u16_at(&self, i: isize) -> u32 {
        if i < 0 || i as usize + 1 >= self.buf.len() {
            return 0;
        }
        (self.buf[i as usize] as u32) << 8 | self.buf[i as usize + 1] as u32
    }

    /// Gets `nbits` from the compressed stream, or -1 past its start.
    fn getb(&mut self, mut nbits: i32) -> i32 {
        self.word &= 0xffff;
        if self.left < nbits {
            self.word <<= self.left as u32;
            if self.org > self.src {
                return -1;
            }
            self.word |= self.u16_at(self.src);
            self.src -= 2;
            nbits -= self.left;
            self.left = 16;
        }
        self.left -= nbits;
        self.word = self.word.wrapping_shl(nbits as u32);
        (self.word >> 16) as i32
    }
}

fn decompress(buf: &[u8], src_start: usize, dst_length: i32, src_length: i32) -> Option<Vec<u8>> {
    let mut dst = alloc_zeroed(dst_length as usize)?;
    let mut d = dst_length as isize; // write index, counting down
    let mut o_len = dst_length;
    let mut bs = Bits {
        word: 0,
        left: 0,
        src: (src_start + src_length as usize) as isize,
        org: src_start as isize,
        buf,
    };
    bs.left = (bs.u16_at(bs.src) & 0x000f) as i32;
    bs.src -= 2;
    bs.word = bs.u16_at(bs.src);
    bs.src -= 2;
    let eff = bs.u16_at(bs.src) as i32;
    bs.src -= 2;
    if !(6..16).contains(&eff) {
        return None;
    }

    macro_rules! get {
        ($n:expr) => {{
            let x = bs.getb($n);
            if x < 0 {
                return None;
            }
            x as u32
        }};
    }

    while o_len > 0 {
        let mut w: u32 = get!(9);
        if w < 0x100 {
            if d <= 0 {
                return None;
            }
            d -= 1;
            dst[d as usize] = w as u8;
            o_len -= 1;
        } else if w == 0x13e || w == 0x13f {
            w <<= 4;
            w |= get!(4);
            let mut n = (w & 0x1f) as i32 + 14;
            o_len -= n;
            while n > 0 {
                n -= 1;
                let b = get!(8);
                if d <= 0 {
                    return None;
                }
                d -= 1;
                dst[d as usize] = b as u8;
            }
        } else {
            let n: i32;
            if w >= 0x180 {
                // copy 2-3
                n = if w & 0x40 != 0 { 3 } else { 2 };
                if w & 0x20 != 0 {
                    w = (w & 0x1f) << (eff - 5);
                    w |= get!(eff - 5);
                    w = (w + 544) & 0xffff;
                } else if w & 0x30 != 0 {
                    w = (w & 0x0f) << 1;
                    w |= get!(1);
                } else {
                    w = (w & 0x0f) << 5;
                    w |= get!(5);
                    w = (w + 32) & 0xffff;
                }
            } else if w >= 0x140 {
                // copy 4-7
                n = ((w & 0x30) >> 4) as i32 + 4;
                if w & 0x08 != 0 {
                    w = (w & 0x07) << (eff - 3);
                    w |= get!(eff - 3);
                    w = (w + 544) & 0xffff;
                } else if w & 0x0c != 0 {
                    w = (w & 0x03) << 3;
                    w |= get!(3);
                } else {
                    w = (w & 0x03) << 7;
                    w |= get!(7);
                    w = (w + 32) & 0xffff;
                }
            } else if w >= 0x120 {
                // copy 8-22
                n = ((w & 0x1e) >> 1) as i32 + 8;
                if w & 0x01 != 0 {
                    w = get!(eff);
                    w = (w + 544) & 0xffff;
                } else {
                    w = get!(6);
                    if w & 0x20 != 0 {
                        w &= 0x1f;
                    } else {
                        w <<= 4;
                        w |= get!(4);
                        w = (w + 32) & 0xffff;
                    }
                }
            } else {
                w = (w & 0x1f) << 3;
                w |= get!(3);
                let mut nn = 23i32;
                while w == 0xff {
                    nn = nn.wrapping_add(w as i32);
                    w = get!(8);
                }
                n = nn.wrapping_add(w as i32);

                // Unchecked in the original: -1 becomes 0xffff.
                w = bs.getb(7) as u32 & 0xffff;
                if w & 0x40 != 0 {
                    w = (w & 0x3f) << (eff - 6);
                    w |= get!(eff - 6);
                    w = (w + 544) & 0xffff;
                } else if w & 0x20 != 0 {
                    w &= 0x1f;
                } else {
                    w <<= 4;
                    w |= get!(4);
                    w = (w + 32) & 0xffff;
                }
            }

            o_len = o_len.wrapping_sub(n);
            for _ in 0..n.max(0) {
                d -= 1;
                if d < 0 || d + w as isize + 1 >= dst_length as isize {
                    return None;
                }
                dst[d as usize] = dst[d as usize + w as usize + 1];
            }
        }
    }
    Some(dst)
}

pub fn test(data: &[u8]) -> bool {
    data.len() >= 4 && &data[..4] == b"S404"
}

pub fn depack(data: &[u8]) -> Option<Vec<u8>> {
    let inlen = data.len();
    if inlen <= 16 || !test(data) {
        return None;
    }
    let be = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as i32;
    let s_len = be(4); // security length
    let o_len = be(8); // depacked length
    let p_len = be(12); // packed length
    if s_len < 0 || o_len <= 0 || p_len <= 6 {
        return None;
    }
    if p_len as i64 > inlen as i64 - 18 {
        return None;
    }
    // Best case ratio of the sliding window is about 255:1.
    if p_len < o_len / 255 {
        return None;
    }
    if o_len as usize > DEPACK_LIMIT {
        return None;
    }
    decompress(data, 16, o_len, p_len)
}
