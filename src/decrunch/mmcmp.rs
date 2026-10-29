//! MMCMP (Music Module Compressor) depacker, a port of libxmp 4.7.4's
//! `mmcmp.c`.
//!
//! Based on the public domain version by Olivier Lapicque (public domain,
//! from libxmp); libxmp's rewrite carries this notice:
//!
//! Rewritten for libxmp by Claudio Matsuoka
//!
//! Copyright (C) 2012-2024 Claudio Matsuoka
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

use super::{alloc_zeroed, DEPACK_LIMIT};

const MMCMP_COMP: i32 = 0x0001;
const MMCMP_DELTA: i32 = 0x0002;
const MMCMP_16BIT: i32 = 0x0004;
const MMCMP_ABS16: i32 = 0x0200;

const CMD_8BITS: [u32; 8] = [0x01, 0x03, 0x07, 0x0f, 0x1e, 0x3c, 0x78, 0xf8];
const FETCH_8BIT: [u32; 8] = [3, 3, 3, 3, 2, 1, 0, 0];
const CMD_16BIT: [u32; 16] = [
    0x0001, 0x0003, 0x0007, 0x000f, 0x001e, 0x003c, 0x0078, 0x00f0, //
    0x01f0, 0x03f0, 0x07f0, 0x0ff0, 0x1ff0, 0x3ff0, 0x7ff0, 0xfff0,
];
const FETCH_16BIT: [u32; 16] = [4, 4, 4, 4, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// A seekable input; single byte reads past its end yield 0xff, as the original's.
struct In<'a> {
    d: &'a [u8],
    pos: usize,
}

impl In<'_> {
    fn read8(&mut self) -> u32 {
        let v = self.d.get(self.pos).copied().unwrap_or(0xff);
        self.pos = self.pos.saturating_add(1);
        v as u32
    }

    /// Like fread: returns the number of bytes copied.
    fn read(&mut self, out: &mut [u8]) -> usize {
        let avail = self.d.len().saturating_sub(self.pos);
        let n = out.len().min(avail);
        if n > 0 {
            out[..n].copy_from_slice(&self.d[self.pos..self.pos + n]);
        }
        self.pos = self.pos.saturating_add(n);
        n
    }

    fn le16(&mut self) -> Option<u32> {
        let mut b = [0u8; 2];
        (self.read(&mut b) == 2).then(|| u16::from_le_bytes(b) as u32)
    }

    fn le32(&mut self) -> Option<u32> {
        let mut b = [0u8; 4];
        (self.read(&mut b) == 4).then(|| u32::from_le_bytes(b))
    }
}

struct BitBuffer {
    count: u32,
    buffer: u32,
}

impl BitBuffer {
    fn get(&mut self, f: &mut In, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        while self.count < 24 {
            self.buffer |= f.read8() << self.count;
            self.count += 8;
        }
        let bits = self.buffer & ((1u32 << n) - 1);
        self.buffer >>= n;
        self.count -= n;
        bits
    }
}

struct Out {
    buf: Vec<u8>,
    pos: usize,
}

impl Out {
    fn seek(&mut self, pos: i32) -> bool {
        if pos < 0 || pos as usize >= self.buf.len() {
            return false;
        }
        self.pos = pos as usize;
        true
    }

    fn write8(&mut self, v: u8) -> bool {
        if self.pos >= self.buf.len() {
            return false;
        }
        self.buf[self.pos] = v;
        self.pos += 1;
        true
    }

    fn write16l(&mut self, v: u16) {
        // Blocks may write half words at the very end of the file.
        if self.write8(v as u8) {
            self.write8((v >> 8) as u8);
        }
    }
}

struct Block {
    flags: i32,
    tt_entries: i32,
    num_bits: i32,
}

struct Sub {
    unpk_pos: i32,
    unpk_size: i32,
}

fn block_copy(sub: &[Sub], input: &mut In, out: &mut Out) -> bool {
    for s in sub {
        let size = out.buf.len();
        if s.unpk_pos as usize >= size || s.unpk_size as usize > size - s.unpk_pos as usize {
            return false;
        }
        let p = s.unpk_pos as usize;
        if input.read(&mut out.buf[p..p + s.unpk_size as usize]) < s.unpk_size as usize {
            return false;
        }
    }
    true
}

fn block_unpack_16bit(block: &Block, sub: &[Sub], input: &mut In, out: &mut Out) -> bool {
    let mut bb = BitBuffer { count: 0, buffer: 0 };
    let mut pos: u32 = 0;
    let mut oldval: u32 = 0;
    let mut numbits = block.num_bits as u8;

    if !out.seek(sub[0].unpk_pos) {
        return false;
    }
    input.pos = input.pos.saturating_add(block.tt_entries as usize);

    let mut j = 0usize;
    while j < sub.len() {
        let size = sub[j].unpk_size as u32;
        let mut newval: u32 = 0x10000;
        let mut d = bb.get(input, numbits as u32 + 1);

        if d >= CMD_16BIT[numbits as usize] {
            let fetch = FETCH_16BIT[numbits as usize];
            let newbits = bb.get(input, fetch).wrapping_add((d - CMD_16BIT[numbits as usize]) << fetch);
            if newbits != numbits as u32 {
                numbits = (newbits & 0x0f) as u8;
            } else {
                d = bb.get(input, 4);
                if d == 0x0f {
                    if bb.get(input, 1) != 0 {
                        break;
                    }
                    newval = 0xffff;
                } else {
                    newval = 0xfff0 + d;
                }
            }
        } else {
            newval = d;
        }

        if newval < 0x10000 {
            if newval & 1 != 0 {
                newval = (newval.wrapping_add(1) >> 1).wrapping_neg();
            } else {
                newval >>= 1;
            }
            if block.flags & MMCMP_DELTA != 0 {
                newval = newval.wrapping_add(oldval);
                oldval = newval;
            } else if block.flags & MMCMP_ABS16 == 0 {
                newval ^= 0x8000;
            }
            pos = pos.wrapping_add(2);
            out.write16l(newval as u16);
        }

        if pos >= size {
            j += 1;
            if j >= sub.len() {
                break;
            }
            pos = 0;
            if !out.seek(sub[j].unpk_pos) {
                return false;
            }
        }
    }
    true
}

fn block_unpack_8bit(block: &Block, sub: &[Sub], input: &mut In, out: &mut Out) -> bool {
    let mut bb = BitBuffer { count: 0, buffer: 0 };
    let mut pos: u32 = 0;
    let mut oldval: u32 = 0;
    let seekpos = input.pos.saturating_add(block.tt_entries as usize);
    let mut numbits = block.num_bits as u8;

    // The original reads at most 0x100 bytes into a zeroed table.
    let mut ptable = [0u8; 0x100];
    if input.read(&mut ptable) < block.tt_entries as usize {
        return false;
    }

    if !out.seek(sub[0].unpk_pos) {
        return false;
    }
    input.pos = seekpos;

    let mut j = 0usize;
    while j < sub.len() {
        let size = sub[j].unpk_size as u32;
        let mut newval: u32 = 0x100;
        let mut d = bb.get(input, numbits as u32 + 1);

        if d >= CMD_8BITS[numbits as usize] {
            let fetch = FETCH_8BIT[numbits as usize];
            let newbits = bb.get(input, fetch).wrapping_add((d - CMD_8BITS[numbits as usize]) << fetch);
            if newbits != numbits as u32 {
                numbits = (newbits & 0x07) as u8;
            } else {
                d = bb.get(input, 3);
                if d == 7 {
                    if bb.get(input, 1) != 0 {
                        break;
                    }
                    newval = 0xff;
                } else {
                    newval = 0xf8 + d;
                }
            }
        } else {
            newval = d;
        }

        if newval < 0x100 {
            let mut n = ptable[newval as usize] as u32;
            if block.flags & MMCMP_DELTA != 0 {
                n = n.wrapping_add(oldval);
                oldval = n;
            }
            pos = pos.wrapping_add(1);
            out.write8(n as u8);
        }

        if pos >= size {
            j += 1;
            if j >= sub.len() {
                break;
            }
            pos = 0;
            if !out.seek(sub[j].unpk_pos) {
                return false;
            }
        }
    }
    true
}

pub fn test(data: &[u8]) -> bool {
    data.len() >= 8 && &data[..8] == b"ziRCONia"
}

pub fn depack(data: &[u8]) -> Option<Vec<u8>> {
    if !test(data) {
        return None;
    }
    let mut input = In { d: data, pos: 8 };
    if input.le16()? != 14 {
        return None; // header size
    }
    let _version = input.le16()?;
    let nblocks = input.le16()? as usize;
    let filesize = input.le32()? as i32;
    let blktable = input.le32()? as i32;
    let _glb_comp = input.read8();
    let _fmt_comp = input.read8();
    if input.pos > data.len() {
        return None;
    }

    if nblocks == 0 || filesize < 16 || filesize as usize > DEPACK_LIMIT {
        return None;
    }

    // Block table
    if blktable < 0 {
        return None;
    }
    input.pos = blktable as usize;
    let mut out = Out { buf: alloc_zeroed(filesize as usize)?, pos: 0 };
    let mut table = Vec::new();
    for _ in 0..nblocks {
        table.push(input.le32()?);
    }

    for &t in &table {
        input.pos = t as usize;
        let mut b = [0u8; 20];
        if input.read(&mut b) != 20 {
            return None;
        }
        let le32 = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as i32;
        let le16 = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as i32;
        let unpk_size = le32(0);
        let pk_size = le32(4);
        let _xor_chk = le32(8);
        let sub_blk = le16(12);
        let block = Block { flags: le16(14), tt_entries: le16(16), num_bits: le16(18) };

        // Sanity check
        if unpk_size <= 0 || pk_size <= 0 {
            return None;
        }
        if block.tt_entries < 0 || pk_size <= block.tt_entries {
            return None;
        }
        if sub_blk <= 0 {
            return None;
        }
        if block.flags & MMCMP_COMP != 0 {
            if block.flags & MMCMP_16BIT != 0 {
                if block.num_bits >= 16 {
                    return None;
                }
            } else if block.num_bits >= 8 {
                return None;
            }
        }

        let mut sub = Vec::new();
        for _ in 0..sub_blk {
            let mut s = [0u8; 8];
            if input.read(&mut s) != 8 {
                return None;
            }
            let unpk_pos = i32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            let unpk_size = i32::from_le_bytes([s[4], s[5], s[6], s[7]]);
            // Sanity check
            if unpk_pos < 0 || unpk_size < 0 {
                return None;
            }
            sub.push(Sub { unpk_pos, unpk_size });
        }

        let ok = if block.flags & MMCMP_COMP == 0 {
            block_copy(&sub, &mut input, &mut out) // data is not packed
        } else if block.flags & MMCMP_16BIT != 0 {
            block_unpack_16bit(&block, &sub, &mut input, &mut out)
        } else {
            block_unpack_8bit(&block, &sub, &mut input, &mut out)
        };
        if !ok {
            return None;
        }
    }
    Some(out.buf)
}
