//! XPK-SQSH depacker, a port of libxmp 4.7.4's `unsqsh.c`.
//!
//! The original is licensed as follows.
//!
//! XPK-SQSH depacker
//! Algorithm from the portable decruncher by Bert Jahn (24.12.97)
//! Checksum added by Sipos Attila <h430827@stud.u-szeged.hu>
//! Rewritten for libxmp by Claudio Matsuoka
//!
//! Copyright (C) 2013-2026 Claudio Matsuoka
//!
//! Permission is hereby granted, free of charge, to any person obtaining a
//! copy of this software and associated documentation files (the "Software"),
//! to deal in the Software without restriction, including without limitation
//! the rights to use, copy, modify, merge, publish, distribute, sublicense,
//! and/or sell copies of the Software, and to permit persons to whom the
//! Software is furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
//! THE SOFTWARE.

use super::alloc_zeroed;

const CTABLE: [u8; 56] = [
    2, 3, 4, 5, 6, 7, 8, 0, //
    3, 2, 4, 5, 6, 7, 8, 0, //
    4, 3, 5, 2, 6, 7, 8, 0, //
    5, 4, 6, 2, 3, 7, 8, 0, //
    6, 5, 7, 2, 3, 4, 8, 0, //
    7, 6, 8, 2, 3, 4, 5, 0, //
    8, 7, 6, 2, 3, 4, 5, 0,
];

struct Io<'a> {
    /// Source buffer, zero padded past the packed data.
    buf: &'a [u8],
    /// Index in `buf` of the block's first packed byte.
    src: usize,
    /// Index in `dest` of the next byte to write.
    dest: usize,
    /// Bit offset from `src`.
    offs: i32,
    /// Packed block size in bits.
    srclen: i32,
}

impl Io<'_> {
    fn byte(&self, i: usize) -> u32 {
        self.buf.get(i).copied().unwrap_or(0) as u32
    }

    fn has_bits(&self, count: i32) -> bool {
        count <= self.srclen - self.offs
    }

    fn read24(&self) -> i32 {
        let p = self.src.wrapping_add((self.offs >> 3) as usize);
        (self.byte(p) << 16 | self.byte(p.wrapping_add(1)) << 8 | self.byte(p.wrapping_add(2))) as i32
    }

    fn get_bits(&mut self, count: i32) -> i32 {
        if !self.has_bits(count) {
            return -1;
        }
        let mut r = self.read24();
        r <<= self.offs % 8;
        r &= 0xff_ffff;
        r >>= 24 - count;
        self.offs += count;
        r
    }

    /// May return negative values; the caller checks `has_bits` first.
    fn get_bits_final(&mut self, count: i32) -> i32 {
        let r = self.read24();
        let r = ((r as u32) << ((self.offs % 8) + 8) as u32) as i32;
        self.offs += count;
        r.wrapping_shr((32 - count) as u32)
    }
}

fn xchecksum(buf: &[u8], c: usize, packed: usize) -> u32 {
    // The three bytes after the packed data count as zero.
    let at = |i: usize| -> u32 {
        if i >= c + packed && i < c + packed + 3 {
            0
        } else {
            buf.get(i).copied().unwrap_or(0) as u32
        }
    };
    let mut sum = 0u32;
    for k in 0..(packed + 3) >> 2 {
        let p = c + 4 * k;
        sum ^= at(p) << 24 | at(p + 1) << 16 | at(p + 2) << 8 | at(p + 3);
    }
    (sum ^ (sum >> 16)) & 0xffff
}

fn copy_data(io: &mut Io, dest: &mut [u8], dest_end: usize, mut d1: i32, data: &mut i32) -> i32 {
    let copy_len_bits = |io: &mut Io| -> i32 {
        if io.get_bits(1) == 0 {
            io.get_bits(1) + 2
        } else if io.get_bits(1) == 0 {
            io.get_bits(1) + 4
        } else if io.get_bits(1) == 0 {
            io.get_bits(1) + 6
        } else if io.get_bits(1) == 0 {
            io.get_bits(3) + 8
        } else {
            io.get_bits(5) + 16
        }
    };
    let mut copy_len = copy_len_bits(io);

    let mut r = io.get_bits(1);
    if copy_len < 0 || r < 0 {
        return -1;
    }
    let (count, dest_offset);
    if r == 0 {
        r = io.get_bits(1);
        if r < 0 {
            return -1;
        }
        if r == 0 {
            count = 8;
            dest_offset = 0i64;
        } else {
            count = 14;
            dest_offset = -0x1100;
        }
    } else {
        count = 12;
        dest_offset = -0x100;
    }

    copy_len -= 3;
    if copy_len >= 0 {
        if copy_len != 0 {
            d1 -= 1;
        }
        d1 -= 1;
        if d1 < 0 {
            d1 = 0;
        }
    }
    copy_len += 2;

    let r = io.get_bits(count);
    if r < 0 {
        return -1;
    }
    let copy_src = io.dest as i64 + dest_offset - r as i64 - 1;

    // Sanity check
    if copy_src < 0 || copy_src + copy_len as i64 >= dest_end as i64 {
        return -1;
    }
    let mut s = copy_src as usize;
    loop {
        if io.dest >= dest.len() || s >= dest.len() {
            return -1;
        }
        dest[io.dest] = dest[s];
        io.dest += 1;
        s += 1;
        let more = copy_len != 0;
        copy_len -= 1;
        if !more {
            break;
        }
    }
    *data = dest[s - 1] as i32;
    d1
}

fn unsqsh_block(io: &mut Io, dest: &mut [u8], dest_end: usize) -> bool {
    let (mut d1, mut d2, mut old_count) = (0i32, 0i32, 0i32);

    io.offs = 0;
    let mut data = io.byte(io.src) as i32;
    io.src += 1;
    if io.dest >= dest.len() {
        return false;
    }
    dest[io.dest] = data as u8;
    io.dest += 1;

    loop {
        let r = io.get_bits(1);
        if r < 0 {
            return false;
        }
        let (mut count, unpack_len);
        if d1 < 8 {
            if r != 0 {
                d1 = copy_data(io, dest, dest_end, d1, &mut data);
                if d1 < 0 {
                    return false;
                }
                d2 -= d2 >> 3;
                if io.dest < dest_end {
                    continue;
                }
                break;
            }
            unpack_len = 0;
            count = 8;
        } else if r != 0 {
            count = 8;
            if count == old_count {
                if d2 >= 20 {
                    unpack_len = 1;
                    d2 += 8;
                } else {
                    unpack_len = 0;
                }
            } else {
                count = old_count;
                unpack_len = 4;
                d2 += 8;
            }
        } else {
            let r = io.get_bits(1);
            if r < 0 {
                return false;
            }
            if r == 0 {
                d1 = copy_data(io, dest, dest_end, d1, &mut data);
                if d1 < 0 {
                    return false;
                }
                d2 -= d2 >> 3;
                if io.dest < dest_end {
                    continue;
                }
                break;
            }
            let r = io.get_bits(1);
            if r < 0 {
                return false;
            }
            if r == 0 {
                count = 2;
            } else {
                let r = io.get_bits(1);
                if r < 0 {
                    return false;
                }
                if r != 0 {
                    io.offs -= 1;
                    count = io.get_bits(3);
                    if count < 0 {
                        return false;
                    }
                } else {
                    count = 3;
                }
            }
            let idx = 8 * old_count + count - 17;
            if !(0..CTABLE.len() as i32).contains(&idx) {
                return false;
            }
            count = CTABLE[idx as usize] as i32;
            if count != 8 {
                unpack_len = 4;
                d2 += 8;
            } else if d2 >= 20 {
                unpack_len = 1;
                d2 += 8;
            } else {
                unpack_len = 0;
            }
        }

        if !io.has_bits(count * (unpack_len + 2)) {
            return false;
        }
        for _ in 0..=unpack_len {
            data = data.wrapping_sub(io.get_bits_final(count));
            if io.dest >= dest.len() {
                return false;
            }
            dest[io.dest] = data as u8;
            io.dest += 1;
        }
        if d1 != 31 {
            d1 += 1;
        }
        old_count = count;
        d2 -= d2 >> 3;
        if io.dest >= dest_end {
            break;
        }
    }
    true
}

fn unsqsh(src: &[u8], srclen: i32, dest: &mut [u8], destlen: i32) -> i32 {
    let mut len = destlen;
    let mut decrunched = 0i32;
    let mut c = 20usize;
    let mut io_dest = 0usize;
    let be16 = |b: &[u8], i: usize| (b[i] as i32) << 8 | b[i + 1] as i32;

    while len != 0 {
        // Sanity check
        if c as i64 + 8 > srclen as i64 {
            return -1;
        }
        let ty = src[c];
        c += 2; // type, hchk
        let sum = be16(src, c);
        c += 2;
        let mut packed = be16(src, c);
        c += 2;
        let unpacked = be16(src, c);
        c += 2;

        // Sanity check
        if packed <= 0 || unpacked <= 0 {
            return -1;
        }
        if c as i64 + packed as i64 + 3 > srclen as i64 {
            return -1;
        }

        let lchk = xchecksum(src, c, packed as usize);
        if lchk as i32 != sum {
            return decrunched;
        }

        if ty == 0 {
            // verbatim block
            decrunched += packed;
            if decrunched > destlen {
                return -1;
            }
            let p = packed as usize;
            dest[io_dest..io_dest + p].copy_from_slice(&src[c..c + p]);
            io_dest += p;
            c += p;
            len -= packed;
            continue;
        }
        if ty != 1 {
            // unknown type
            return decrunched;
        }

        let block_src = c + 2;
        let block_srclen = packed << 3;

        len -= unpacked;
        decrunched += unpacked;
        // Sanity check
        if decrunched > destlen {
            return -1;
        }

        packed = (packed + 3) & 0xfffc;
        c += packed as usize;
        let dest_end = io_dest + unpacked as usize;

        let mut io = Io { buf: src, src: block_src, dest: io_dest, offs: 0, srclen: block_srclen };
        if !unsqsh_block(&mut io, dest, dest_end) {
            return -1;
        }
        io_dest = dest_end;
    }
    decrunched
}

pub fn test(data: &[u8]) -> bool {
    data.len() >= 12 && &data[..4] == b"XPKF" && &data[8..12] == b"SQSH"
}

pub fn depack(data: &[u8]) -> Option<Vec<u8>> {
    if !test(data) || data.len() < 16 {
        return None;
    }
    let be = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as i32;
    let srclen = be(4);
    // Sanity check
    if srclen <= 8 || srclen > 0x100000 {
        return None;
    }
    let destlen = be(12);
    if destlen <= 0 || destlen > 0x100000 {
        return None;
    }
    let need = srclen as usize - 8;
    if data.len() < 16 + need {
        return None;
    }
    let mut src = alloc_zeroed(srclen as usize + 3)?;
    src[..need].copy_from_slice(&data[16..16 + need]);
    let mut dest = alloc_zeroed(destlen as usize + 100)?;
    if unsqsh(&mut src, srclen, &mut dest, destlen) != destlen {
        return None;
    }
    dest.truncate(destlen as usize);
    Some(dest)
}
