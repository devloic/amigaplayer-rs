//! Pack-Ice (Ice!/ICE!) depacker, a port of libxmp 4.7.4's `ice.c`,
//! `ice_unpack.c` and `ice_unpack_fn.c`.
//!
//! The original is licensed as follows.
//!
//! Extended Module Player
//! Copyright (C) 2024-2025 Alice Rowan <petrifiedrowan@gmail.com>
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
//!
//! Implementation largely based on this post by nocash:
//! <https://eab.abime.net/showpost.php?p=1617809&postcount=7>

use super::{alloc_zeroed, DEPACK_LIMIT};

const OLD_MAGIC: u32 = u32::from_be_bytes(*b"Ice!");
const NEW_MAGIC: u32 = u32::from_be_bytes(*b"ICE!");
const HACKED_MAGICS: [[u8; 4]; 5] = [*b"-CJ-", *b"MICK", *b"SHE!", *b"TMM!", *b"TSM!"];

const VERSION_113: i32 = 113;
const VERSION_21X: i32 = 210;
const VERSION_21X_OR_220: i32 = 215;
const VERSION_220: i32 = 220;
const VERSION_23X: i32 = 230;

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// The stream is read backwards, from its end towards its start.
struct Ice<'a> {
    stream: &'a [u8],
    /// Bytes of `stream` not consumed yet.
    pos: usize,
    eof: bool,
    version: i32,
    bits: u32,
    bits_left: i32,
}

impl Ice<'_> {
    fn read_byte(&mut self) -> i32 {
        if self.pos < 1 {
            self.eof = true;
            return -1;
        }
        self.pos -= 1;
        self.stream[self.pos] as i32
    }

    fn read_u16le(&mut self) -> i32 {
        if self.pos < 2 {
            self.eof = true;
            return -1;
        }
        self.pos -= 2;
        self.stream[self.pos] as i32 | (self.stream[self.pos + 1] as i32) << 8
    }

    fn peek_u32(&mut self) -> u32 {
        if self.pos < 4 {
            self.eof = true;
            return 0;
        }
        be32(&self.stream[self.pos - 4..self.pos])
    }

    fn init_buffer(&mut self) -> bool {
        self.eof = false;
        self.pos = self.stream.len();
        if self.pos == 0 {
            self.eof = true;
            return false;
        }
        // Version filtering for ambiguous Ice! files: the first bit of the
        // stream, as an 8-bit or a 32-bit word, must be set.
        if self.version == VERSION_21X_OR_220 {
            let peek = self.peek_u32();
            if (!peek & 0x80) != 0 && (peek & 0x8000_0000) != 0 {
                self.version = VERSION_21X;
            } else if (peek & 0x80) != 0 && (!peek & 0x8000_0000) != 0 {
                self.version = VERSION_220;
            }
        }
        true
    }

    /// The preloaded word has no terminating bit: its lowest set bit is it.
    fn preload_adjust(&mut self) -> bool {
        let mut tmp = self.bits >> (32 - self.bits_left) as u32;
        if (!self.bits) & 0x8000_0000 != 0 {
            return false;
        }
        while (!tmp) & 1 != 0 {
            tmp >>= 1;
            self.bits_left -= 1;
        }
        tmp >>= 1;
        self.bits_left -= 1;
        if self.bits_left != 0 {
            tmp <<= (32 - self.bits_left) as u32;
        }
        self.bits = tmp;
        true
    }

    fn load(&mut self, wide: bool) {
        if wide {
            self.bits = self.peek_u32();
            self.bits_left += 32;
            self.pos = self.pos.saturating_sub(4);
        } else {
            self.bits = (self.read_byte() as u32) << 24;
            self.bits_left += 8;
        }
    }

    fn read_bits(&mut self, wide: bool, num: i32) -> i32 {
        let ret;
        if wide {
            let left = num - self.bits_left;
            let mut r = self.bits >> (32 - num) as u32;
            self.bits_left -= num;
            if left <= 0 {
                self.bits <<= num as u32;
            } else {
                self.load(true);
                r |= self.bits >> (32 - left) as u32;
                self.bits <<= left as u32;
            }
            ret = r;
        } else {
            if num > self.bits_left {
                if num > 8 && num - 8 > self.bits_left {
                    // Two bytes can be loaded safely: the backwards stream
                    // order makes them little endian.
                    let v = self.read_u16le() as u32;
                    self.bits |= v.wrapping_shl((16 - self.bits_left) as u32);
                    self.bits_left += 16;
                } else {
                    let v = self.read_byte() as u32;
                    self.bits |= v.wrapping_shl((24 - self.bits_left) as u32);
                    self.bits_left += 8;
                }
            }
            ret = self.bits >> (32 - num) as u32;
            self.bits <<= num as u32;
            self.bits_left -= num;
        }
        ret as i32
    }

    fn literal_length(&mut self, wide: bool) -> i32 {
        let mut length = self.read_bits(wide, 1);
        if length == 1 {
            length = self.read_bits(wide, 1) + 1;
        }
        if length == 2 {
            length = self.read_bits(wide, 2) + 2;
        }
        if length == 5 {
            length = self.read_bits(wide, 2) + 5;
        }
        if length == 8 {
            length = self.read_bits(wide, 3) + 8;
        }
        if length == 15 {
            if wide && self.version == VERSION_113 {
                length = self.read_bits(wide, 10) + 15;
            } else {
                length = self.read_bits(wide, 8) + 15;
                if length == 270 {
                    length = self.read_bits(wide, 15) + 270;
                }
            }
        }
        if self.eof {
            return -1;
        }
        length
    }

    fn window_length(&mut self, wide: bool) -> i32 {
        let length = if self.read_bits(wide, 1) == 0 {
            2
        } else if self.read_bits(wide, 1) == 0 {
            3
        } else if self.read_bits(wide, 1) == 0 {
            4 + self.read_bits(wide, 1)
        } else if self.read_bits(wide, 1) == 0 {
            6 + self.read_bits(wide, 2)
        } else {
            10 + self.read_bits(wide, 10)
        };
        if self.eof {
            return -1;
        }
        length
    }

    fn window_distance(&mut self, wide: bool, length: i32) -> i32 {
        let dist;
        if length == 2 {
            let mut d = 1 + self.read_bits(wide, 7);
            if d >= 65 {
                d = ((d - 65) << 3) + 65 + self.read_bits(wide, 3);
            }
            dist = d;
        } else if self.read_bits(wide, 1) == 0 {
            dist = 33 + self.read_bits(wide, 8);
        } else if self.read_bits(wide, 1) == 0 {
            dist = 1 + self.read_bits(wide, 5);
        } else {
            dist = 289 + self.read_bits(wide, 12);
        }
        if self.eof {
            return -1;
        }
        dist
    }

    fn at_stream_start(&self) -> bool {
        self.pos == 0 && self.bits_left <= 0
    }

    fn unpack_fn(&mut self, wide: bool, dest: &mut [u8]) -> bool {
        let dest_len = dest.len();
        let mut pos = dest_len;

        // Streams ending with a window copy expect a final zero-length
        // literal block, so don't terminate before it.
        loop {
            let length = self.literal_length(wide);
            if length < 0 || length as usize > pos {
                return false;
            }
            for _ in 0..length {
                let b = self.read_byte();
                if b < 0 {
                    return false;
                }
                pos -= 1;
                dest[pos] = b as u8;
            }
            if pos == 0 {
                break;
            }

            let length = self.window_length(wide);
            if length <= 0 {
                return false;
            }
            let mut dist = self.window_distance(wide, length);
            if dist <= 0 {
                return false;
            }
            // The distance is relative to the last byte written, not to the
            // current position.
            if wide {
                dist += length - 1;
            } else if dist > 1 {
                dist += length - 2;
            }
            if length as usize > pos {
                return false;
            }
            let mut window_pos = dist as usize + pos;
            if window_pos > dest_len {
                return false;
            }
            for _ in 0..length {
                pos -= 1;
                window_pos -= 1;
                dest[pos] = dest[window_pos];
            }
        }

        // Bitplane filter (optional).
        if self.version >= VERSION_21X && self.read_bits(wide, 1) == 1 {
            let mut length = 320 * 200 / 16;
            if !self.at_stream_start() && self.read_bits(wide, 1) == 1 {
                length = self.read_bits(wide, 16) + 1;
                if self.eof {
                    return false;
                }
            }
            if !bitplane_filter(dest, length) {
                return false;
            }
        }

        self.at_stream_start()
    }

    fn unpack_one(&mut self, wide: bool, dest: &mut [u8]) -> bool {
        self.bits = 0;
        self.bits_left = 0;
        self.load(wide);
        if self.eof || !self.preload_adjust() {
            return false;
        }
        self.unpack_fn(wide, dest)
    }

    fn unpack(&mut self, dest: &mut [u8]) -> bool {
        if !self.init_buffer() {
            return false;
        }
        // Ambiguous files are tried as 8-bit first (32-bit streams are less
        // likely to be ambiguous).
        if self.version >= VERSION_21X_OR_220 && self.unpack_one(false, dest) {
            return true;
        }
        if self.version == VERSION_21X_OR_220 && !self.init_buffer() {
            return false;
        }
        self.version <= VERSION_21X_OR_220 && self.unpack_one(true, dest)
    }
}

/// Undoes the bitplane filter on the last `stored_size * 8` bytes of `dest`.
pub(super) fn bitplane_filter(dest: &mut [u8], stored_size: i32) -> bool {
    if stored_size < 0 || stored_size as usize * 8 > dest.len() {
        return false;
    }
    // bit_conv[b] spreads bit i of b to bit 16 * (i % 4) + i / 4.
    let mut bit_conv = [0u64; 256];
    for (b, v) in bit_conv.iter_mut().enumerate() {
        for i in 0..8 {
            if b >> i & 1 != 0 {
                *v |= 1u64 << (16 * (i % 4) + i / 4);
            }
        }
    }
    let start = dest.len() - stored_size as usize * 8;
    for p in dest[start..].chunks_exact_mut(8) {
        let mut planes = 0u64;
        planes |= bit_conv[p[6] as usize] << 14;
        planes |= bit_conv[p[7] as usize] << 12;
        planes |= bit_conv[p[4] as usize] << 10;
        planes |= bit_conv[p[5] as usize] << 8;
        planes |= bit_conv[p[2] as usize] << 6;
        planes |= bit_conv[p[3] as usize] << 4;
        planes |= bit_conv[p[0] as usize] << 2;
        planes |= bit_conv[p[1] as usize];
        p.copy_from_slice(&planes.to_be_bytes());
    }
    true
}

/// A very rough upper bound of the uncompressed size, to filter bad files
/// before allocating a buffer.
fn uncompressed_bound(in_len: u32) -> i64 {
    // Best compression: a dictionary lookup of length 1033, encoded in 14
    // bits, plus a mandatory distance of 7 bits.
    in_len as i64 * ((1033 * 8 + 20) / (14 + 7))
}

fn check_lengths(outlen: i64, inlen: usize) -> bool {
    if outlen <= 0 || outlen > DEPACK_LIMIT as i64 {
        return false;
    }
    if inlen >= i32::MAX as usize {
        return false;
    }
    outlen <= uncompressed_bound(inlen as u32)
}

/// Pack-Ice v1: "Ice!" in the last four bytes, the unpacked size before it.
pub fn test1(data: &[u8]) -> bool {
    data.len() >= 8 && be32(&data[data.len() - 4..]) == OLD_MAGIC
}

pub fn unpack1(data: &[u8]) -> Option<Vec<u8>> {
    if !test1(data) {
        return None;
    }
    let sz = be32(&data[data.len() - 8..]) as i64;
    if !check_lengths(sz, data.len()) {
        return None;
    }
    let mut out = alloc_zeroed(sz as usize)?;
    let mut ice = Ice {
        stream: &data[..data.len() - 8],
        pos: 0,
        eof: false,
        version: VERSION_113,
        bits: 0,
        bits_left: 0,
    };
    ice.unpack(&mut out).then_some(out)
}

/// Pack-Ice v2: a 12 byte header (magic, packed size, unpacked size).
pub fn test2(data: &[u8]) -> bool {
    data.len() >= 12
        && (be32(data) == OLD_MAGIC
            || be32(data) == NEW_MAGIC
            || HACKED_MAGICS.contains(&[data[0], data[1], data[2], data[3]]))
}

pub fn unpack2(data: &[u8]) -> Option<Vec<u8>> {
    if !test2(data) {
        return None;
    }
    let sz = be32(&data[8..]) as i64;
    if !check_lengths(sz, data.len()) {
        return None;
    }
    if be32(&data[4..]) as usize != data.len() {
        return None;
    }
    let magic = be32(data);
    let version = if magic == OLD_MAGIC {
        // Ice! may use a 32-bit or an 8-bit buffer.
        VERSION_21X_OR_220
    } else if magic == NEW_MAGIC {
        // ICE! always uses an 8-bit buffer.
        VERSION_23X
    } else {
        // Most hacked magics used older versions.
        VERSION_21X
    };
    let mut out = alloc_zeroed(sz as usize)?;
    let mut ice = Ice { stream: &data[12..], pos: 0, eof: false, version, bits: 0, bits_left: 0 };
    ice.unpack(&mut out).then_some(out)
}
