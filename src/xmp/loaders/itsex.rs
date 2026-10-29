//! Impulse Tracker's compressed samples (`loaders/itsex.c`): the 8- and 16-bit
//! IT 2.14 / 2.15 bit-packed delta decoders.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp). The decompressor itself is
//! Olivier Lapicque's public domain code, its input stream rewritten for libxmp
//! by Alice Rowan (2023-2024).

use crate::xmp::load::Hio;

/// The bit reader over one compressed block (`struct it_stream`).
struct ItStream {
    pos: usize,
    left: usize,
    bits: u32,
    /// An int in libxmp: it goes negative when the last, partial word of a
    /// block runs out, so that the next read fails.
    num_bits: i32,
    err: bool,
}

fn mask(n: u32) -> u32 {
    (1u32 << n).wrapping_sub(1)
}

impl ItStream {
    fn new() -> Self {
        ItStream { pos: 0, left: 0, bits: 0, num_bits: 0, err: false }
    }

    /// `read_bits`: `n` bits, low bits first; `tmp` holds the block, zero
    /// padded to a multiple of 4 bytes.
    fn read_bits(&mut self, tmp: &[u8], n: u32) -> u32 {
        if n == 0 || n >= 32 {
            self.err = true;
            return 0;
        }
        let mut n = n;
        let mut retval = self.bits & mask(n);
        if self.num_bits < n as i32 {
            let offset = self.num_bits as u32;
            if self.left == 0 {
                self.err = true;
                return 0;
            }
            let p = &tmp[self.pos..self.pos + 4];
            self.bits = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            let used = self.left.min(4);
            self.num_bits = used as i32 * 8;
            self.pos += 4;
            self.left -= used;
            n -= offset;
            retval |= (self.bits & mask(n)) << offset;
        }
        self.bits >>= n;
        self.num_bits -= n as i32;
        retval
    }

    /// `init_block`: the next block's length and data from `src` into `tmp`.
    fn init_block(&mut self, tmp: &mut [u8], src: &mut Hio) -> bool {
        self.pos = 0;
        self.left = src.read16l() as usize;
        self.bits = 0;
        self.num_bits = 0;
        self.err = false;
        if tmp.len() < ((self.left + 4) & !3) {
            return false;
        }
        if src.read(&mut tmp[..self.left]) < self.left {
            return false;
        }
        let mut i = self.left;
        while i & 3 != 0 {
            tmp[i] = 0;
            i += 1;
        }
        true
    }
}

/// `itsex_decompress8`: `len` 8-bit samples into `dst`. False on an error, with
/// what was decoded until then left in `dst`.
pub fn decompress8(src: &mut Hio, dst: &mut [u8], len: usize, tmp: &mut [u8], it215: bool) -> bool {
    let mut inp = ItStream::new();
    let mut block_count = 0u32;
    let (mut left, mut temp, mut temp2) = (0u8, 0u8, 0u8);
    let mut len = len;
    let mut base = 0usize;
    while len > 0 {
        if block_count == 0 {
            block_count = 0x8000;
            left = 9;
            temp = 0;
            temp2 = 0;
            if !inp.init_block(tmp, src) {
                return false;
            }
        }
        let d = (block_count as usize).min(len);
        let mut pos = 0usize;
        loop {
            let mut bits = inp.read_bits(tmp, left as u32) as u16;
            if inp.err {
                return false;
            }
            // 0: unpack the byte, 1: skip it, 2: next (width change)
            let action;
            if left < 7 {
                let i = 1u32 << (left - 1);
                let j = bits as u32 & 0xffff;
                if i != j {
                    action = 0;
                } else {
                    bits = ((inp.read_bits(tmp, 3) + 1) & 0xff) as u16;
                    if inp.err {
                        return false;
                    }
                    left = if (bits as u8) < left { bits as u8 } else { ((bits + 1) & 0xff) as u8 };
                    action = 2;
                }
            } else if left < 9 {
                let i: u16 = (0xffu16 >> (9 - left)) + 4;
                let j: u16 = i.wrapping_sub(8);
                if bits <= j || bits > i {
                    action = 0;
                } else {
                    bits = bits.wrapping_sub(j);
                    left = if ((bits & 0xff) as u8) < left { (bits & 0xff) as u8 } else { (bits.wrapping_add(1) & 0xff) as u8 };
                    action = 2;
                }
            } else if left >= 10 {
                action = 1;
            } else if bits >= 256 {
                left = (bits.wrapping_add(1) & 0xff) as u8;
                action = 2;
            } else {
                action = 0;
            }
            if action == 0 {
                if left < 8 {
                    let shift = 8 - left;
                    let c = ((bits << shift) as u8 as i8) >> shift;
                    bits = c as i16 as u16;
                }
                bits = bits.wrapping_add(temp as u16);
                temp = bits as u8;
                temp2 = temp2.wrapping_add(temp);
                dst[base + pos] = if it215 { temp2 } else { temp };
            }
            if action <= 1 {
                pos += 1;
            }
            if pos >= d {
                break;
            }
        }
        block_count -= d as u32;
        len -= d;
        base += d;
    }
    true
}

/// `itsex_decompress16`: `len` 16-bit samples into `dst`.
pub fn decompress16(src: &mut Hio, dst: &mut [i16], len: usize, tmp: &mut [u8], it215: bool) -> bool {
    let mut inp = ItStream::new();
    let mut block_count = 0u32;
    let mut left = 0u8;
    let (mut temp, mut temp2) = (0i16, 0i16);
    let mut len = len;
    let mut base = 0usize;
    while len > 0 {
        if block_count == 0 {
            block_count = 0x4000;
            left = 17;
            temp = 0;
            temp2 = 0;
            if !inp.init_block(tmp, src) {
                return false;
            }
        }
        let d = (block_count as usize).min(len);
        let mut pos = 0usize;
        loop {
            let mut bits = inp.read_bits(tmp, left as u32);
            if inp.err {
                return false;
            }
            let action;
            if left < 7 {
                let i = 1u32 << (left - 1);
                if i != bits {
                    action = 0;
                } else {
                    bits = inp.read_bits(tmp, 4) + 1;
                    if inp.err {
                        return false;
                    }
                    left = if ((bits & 0xff) as u8) < left { (bits & 0xff) as u8 } else { (bits.wrapping_add(1) & 0xff) as u8 };
                    action = 2;
                }
            } else if left < 17 {
                let i: u32 = (0xffffu32 >> (17 - left)) + 8;
                let j: u32 = i.wrapping_sub(16) & 0xffff;
                if bits <= j || bits > (i & 0xffff) {
                    action = 0;
                } else {
                    bits = bits.wrapping_sub(j);
                    left = if ((bits & 0xff) as u8) < left { (bits & 0xff) as u8 } else { (bits.wrapping_add(1) & 0xff) as u8 };
                    action = 2;
                }
            } else if left >= 18 {
                action = 1;
            } else if bits >= 0x10000 {
                left = (bits.wrapping_add(1) & 0xff) as u8;
                action = 2;
            } else {
                action = 0;
            }
            if action == 0 {
                if left < 16 {
                    let shift = 16 - left;
                    let c = ((bits << shift) as u16 as i16) >> shift;
                    bits = c as i32 as u32;
                }
                bits = bits.wrapping_add(temp as i32 as u32);
                temp = bits as i16;
                temp2 = temp2.wrapping_add(temp);
                dst[base + pos] = if it215 { temp2 } else { temp };
            }
            if action <= 1 {
                pos += 1;
            }
            if pos >= d {
                break;
            }
        }
        block_count -= d as u32;
        len -= d;
        base += d;
    }
    true
}
