//! The LZW and sigma-delta unpackers for Digital Symphony (`loaders/lzw.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use super::load::Hio;

pub const LZW_FLAG_SYMQUIRKS: i32 = 0x100;
/// `LZW_FLAGS_SYM`: 13 bits maximum and Digital Symphony's quirks.
pub const LZW_FLAGS_SYM: i32 = 13 | LZW_FLAG_SYMQUIRKS;

const LZW_NO_CODE: u16 = 0xffff;
const LZW_CODE_CLEAR: i32 = 256;
const LZW_CODE_SYM_EOF: i32 = 257;

struct BitStream {
    buf: u32,
    num_read: usize,
    max_read: usize,
    bits: i32,
}

impl BitStream {
    fn new(max_read: usize) -> Self {
        BitStream { buf: 0, num_read: 0, max_read, bits: 0 }
    }

    fn read(&mut self, f: &mut Hio, bits: i32) -> i32 {
        if self.bits < bits {
            while self.bits < bits {
                if self.num_read >= self.max_read {
                    return -1;
                }
                let byte = f.read8();
                self.buf |= (byte as u32) << self.bits;
                self.bits += 8;
                self.num_read += 1;
            }
            if f.take_error() {
                return -1;
            }
        }
        let ret = (self.buf & ((1u32 << bits) - 1)) as i32;
        self.buf >>= bits;
        self.bits -= bits;
        ret
    }
}

#[derive(Clone, Copy, Default)]
struct Code {
    prev: u16,
    length: u16,
    value: u8,
}

struct Tree {
    codes: Vec<Code>,
    bits: i32,
    length: usize,
    maxlength: usize,
    defaultlength: usize,
    alloclength: usize,
    previous_code: u16,
    new_inc: bool,
    previous_first_char: u8,
}

impl Tree {
    fn new(flags: i32) -> Option<Tree> {
        let maxbits = flags & 15;
        let bits = 9;
        if maxbits < bits || maxbits > 16 {
            return None;
        }
        let alloclength = 1usize << maxbits;
        let mut codes = vec![Code::default(); alloclength];
        for (i, c) in codes.iter_mut().enumerate().take(256) {
            c.length = 1;
            c.value = i as u8;
            c.prev = LZW_NO_CODE;
        }
        Some(Tree {
            codes,
            bits,
            length: 258,
            maxlength: 1 << bits,
            defaultlength: 258,
            alloclength,
            previous_code: LZW_NO_CODE,
            new_inc: false,
            previous_first_char: 0,
        })
    }

    fn add(&mut self) {
        if self.length >= self.alloclength {
            return;
        }
        let cur = self.length;
        self.length += 1;
        // increase bitwidth if the NEXT code would be maxlength
        if self.length >= self.maxlength && self.length < self.alloclength {
            self.maxlength <<= 1;
            self.bits += 1;
            self.new_inc = true;
        }
        let prev_length = self.codes[self.previous_code as usize].length;
        let c = &mut self.codes[cur];
        c.prev = self.previous_code;
        c.value = self.previous_first_char;
        c.length = if prev_length != 0 { prev_length.wrapping_add(1) } else { 0 };
    }

    fn clear(&mut self) {
        self.bits = 9;
        self.maxlength = 1 << self.bits;
        self.length = self.defaultlength;
        self.previous_code = LZW_NO_CODE;
    }

    fn output(&mut self, code: u16, dest: &mut [u8], pos: &mut usize) -> Result<(), ()> {
        let mut code = code;
        let mut current = self.codes[code as usize];
        let length = current.length as usize;
        let left = dest.len() - *pos;
        if length == 0 || length > left {
            return Err(());
        }
        for i in (1..length).rev() {
            dest[*pos + i] = current.value;
            code = current.prev;
            current = self.codes[code as usize];
        }
        dest[*pos] = code as u8;
        *pos += length;
        self.previous_first_char = code as u8;
        Ok(())
    }

    fn decode(&mut self, code: u16, dest: &mut [u8], pos: &mut usize) -> Result<(), ()> {
        let mut kwkwk = false;
        if code as usize > self.length {
            return Err(());
        }
        if code as usize == self.length {
            if self.previous_code == LZW_NO_CODE {
                return Err(());
            }
            self.add();
            self.previous_code = code;
            kwkwk = true;
        }
        let result = self.output(code, dest, pos);
        if result.is_ok() && !kwkwk {
            if self.previous_code != LZW_NO_CODE {
                self.add();
            }
            self.previous_code = code;
        }
        result
    }
}

/// `libxmp_read_lzw`: `dest` (its whole length) filled from at most `max_read_len`
/// bytes of `f`; a stream that ends or breaks early leaves the rest zero.
pub fn read_lzw(dest: &mut [u8], max_read_len: usize, flags: i32, f: &mut Hio) -> Result<(), ()> {
    let mut bs = BitStream::new(max_read_len);
    let mut lzw = Tree::new(flags).ok_or(())?;
    let mut pos = 0usize;

    while pos < dest.len() {
        let code = bs.read(f, lzw.bits);
        if code < 0 {
            break;
        }
        if code == LZW_CODE_CLEAR {
            lzw.clear();
            continue;
        } else if flags & LZW_FLAG_SYMQUIRKS != 0 && code == LZW_CODE_SYM_EOF {
            break;
        }
        lzw.new_inc = false;
        if lzw.decode(code as u16, dest, &mut pos).is_err() {
            break;
        }
    }

    if pos < dest.len() {
        for b in dest[pos..].iter_mut() {
            *b = 0;
        }
    } else if flags & LZW_FLAG_SYMQUIRKS != 0 {
        // Digital Symphony: read the final EOF code; when the last code should
        // have widened the codes, the EOF uses the old width
        if lzw.new_inc {
            lzw.bits -= 1;
        }
        let _ = bs.read(f, lzw.bits);
    }

    if flags & LZW_FLAG_SYMQUIRKS != 0 {
        // the compressed stream is 4 aligned
        let mut n = bs.num_read;
        while n & 3 != 0 {
            f.read8();
            n += 1;
        }
    }
    Ok(())
}

/// `libxmp_read_sigma_delta`: Digital Symphony's sigma-delta samples, after the
/// unpacker of OpenMPT by Saga Musix.
pub fn read_sigma_delta(dest: &mut [u8], max_read_len: usize, f: &mut Hio) -> Result<(), ()> {
    if dest.is_empty() {
        return Ok(());
    }
    let mut bs = BitStream::new(max_read_len);
    // doesn't count towards the alignment
    let max_runlength = f.read8() as i32;
    let mut runlength = 0;
    let mut bits = 8;
    // does count
    let mut accumulator = bs.read(f, bits) as u8;
    dest[0] = accumulator;
    let mut pos = 1;

    while pos < dest.len() {
        let value = bs.read(f, bits);
        if value < 0 {
            return Err(());
        }
        // expand bitwidth
        if value == 0 {
            if bits >= 9 {
                return Err(());
            }
            bits += 1;
            runlength = 0;
            continue;
        }
        if value & 1 != 0 {
            accumulator = accumulator.wrapping_sub((value >> 1) as u8);
        } else {
            accumulator = accumulator.wrapping_add((value >> 1) as u8);
        }
        dest[pos] = accumulator;
        pos += 1;

        // a high bit set resets the run length
        if value >> (bits - 1) != 0 {
            runlength = 0;
            continue;
        }
        // reduce bitwidth
        runlength += 1;
        if runlength >= max_runlength {
            if bits > 1 {
                bits -= 1;
            }
            runlength = 0;
        }
    }

    // bitstreams are aligned to 4
    let mut total = bs.num_read;
    while total & 3 != 0 {
        f.read8();
        total += 1;
    }
    Ok(())
}
