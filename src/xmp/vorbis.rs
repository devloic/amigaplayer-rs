//! The Ogg Vorbis decoder libxmp uses for OggMod samples (`loaders/vorbis.c`,
//! stb_vorbis 1.22 by Sean Barrett), as libxmp configures it (no push-data,
//! stdio, comment, seek or float-conversion APIs; fast Huffman tables of 10
//! bits) and as `libxmp_vorbis_decode_memory` drives it.
//!
//! Only mono streams are decoded: libxmp's `oggdec` refuses a stream of any
//! other channel count after decoding it, so this port refuses it as soon as
//! the identification header says so, with the same result. That leaves out
//! channel coupling and the interleaved residue-type-2 paths, which no mono
//! stream reaches (stb_vorbis rejects coupling steps in a mono stream).
//!
//! Ported from libxmp 4.7.4. stb_vorbis is available under two licences
//! (MIT, or public domain); the MIT notice:
//!
//! Copyright (c) 2017 Sean Barrett
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
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
//! FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
//! DEALINGS IN THE SOFTWARE.

use std::f64::consts::PI;

const FAST_HUFFMAN_LENGTH: u32 = 10;
const FAST_HUFFMAN_TABLE_SIZE: usize = 1 << FAST_HUFFMAN_LENGTH;
const FAST_HUFFMAN_TABLE_MASK: u32 = (1 << FAST_HUFFMAN_LENGTH) - 1;
const EOP: i32 = -1;
const NO_CODE: u8 = 255;
const PACKET_ID: i32 = 1;
const PACKET_SETUP: i32 = 5;

/// setup_malloc / setup_temp_malloc's size check (a NULL return there is an
/// out-of-memory error); the allocation itself is tried, so that a damaged
/// header asking for gigabytes fails instead of aborting.
fn alloc<T: Clone>(siz_bytes: u64, n: usize, v: T) -> Option<Vec<T>> {
    let sz = siz_bytes as i32;
    if sz <= 0 || 0x7fff_ffffu64 - 7 < siz_bytes {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(n).ok()?;
    out.resize(n, v);
    Some(out)
}

#[derive(Default)]
struct Codebook {
    dimensions: i32,
    entries: i32,
    codeword_lengths: Vec<u8>,
    minimum_value: f32,
    delta_value: f32,
    value_bits: u8,
    lookup_type: u8,
    sequence_p: u8,
    sparse: u8,
    lookup_values: u64,
    multiplicands: Vec<f32>,
    /// None where stb_vorbis's pointer is NULL
    codewords: Option<Vec<u32>>,
    fast_huffman: Vec<i16>,
    sorted_codewords: Option<Vec<u32>>,
    /// stb_vorbis keeps this one past its allocation and stores -1 before it
    sorted_values: Vec<i32>,
    sorted_entries: i32,
}

impl Codebook {
    #[inline]
    fn sorted_value(&self, i: i32) -> i32 {
        self.sorted_values[(i + 1) as usize]
    }
}

#[derive(Clone)]
struct Floor1 {
    partitions: u8,
    partition_class_list: [u8; 32],
    class_dimensions: [u8; 16],
    class_subclasses: [u8; 16],
    class_masterbooks: [u8; 16],
    subclass_books: [[i16; 8]; 16],
    xlist: [u16; 31 * 8 + 2],
    sorted_order: [u8; 31 * 8 + 2],
    neighbors: [[u8; 2]; 31 * 8 + 2],
    floor1_multiplier: u8,
    rangebits: u8,
    values: i32,
}

impl Default for Floor1 {
    fn default() -> Self {
        Floor1 {
            partitions: 0,
            partition_class_list: [0; 32],
            class_dimensions: [0; 16],
            class_subclasses: [0; 16],
            class_masterbooks: [0; 16],
            subclass_books: [[0; 8]; 16],
            xlist: [0; 31 * 8 + 2],
            sorted_order: [0; 31 * 8 + 2],
            neighbors: [[0; 2]; 31 * 8 + 2],
            floor1_multiplier: 0,
            rangebits: 0,
            values: 0,
        }
    }
}

#[derive(Default)]
struct Residue {
    begin: u32,
    end: u32,
    part_size: u32,
    classifications: u8,
    classbook: u8,
    /// entries x classwords
    classdata: Vec<u8>,
    classwords: usize,
    residue_books: Vec<[i16; 8]>,
}

#[derive(Default, Clone, Copy)]
struct MappingChannel {
    mux: u8,
}

#[derive(Default)]
struct Mapping {
    chan: Vec<MappingChannel>,
    submaps: u8,
    submap_floor: [u8; 16],
    submap_residue: [u8; 16],
}

#[derive(Default, Clone, Copy)]
struct Mode {
    blockflag: u8,
    mapping: u8,
}

/// The Ogg page and packet reader with its bit accumulator.
struct Stream<'a> {
    data: &'a [u8],
    pos: usize,
    eof: bool,
    page_flag: u8,
    segment_count: i32,
    segments: [u8; 255],
    next_seg: i32,
    last_seg: bool,
    last_seg_which: i32,
    bytes_in_seg: u8,
    acc: u32,
    valid_bits: i32,
    end_seg_with_known_loc: i32,
    known_loc_for_packet: u32,
}

impl Stream<'_> {
    fn get8(&mut self) -> u8 {
        if self.pos >= self.data.len() {
            self.eof = true;
            return 0;
        }
        let b = self.data[self.pos];
        self.pos += 1;
        b
    }

    fn get32(&mut self) -> u32 {
        let mut x = self.get8() as u32;
        x = x.wrapping_add((self.get8() as u32) << 8);
        x = x.wrapping_add((self.get8() as u32) << 16);
        x.wrapping_add((self.get8() as u32) << 24)
    }

    fn getn(&mut self, out: &mut [u8]) -> bool {
        let n = out.len();
        if self.pos + n > self.data.len() {
            self.eof = true;
            return false;
        }
        out.copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        true
    }

    fn skip(&mut self, n: i32) {
        self.pos += n as usize;
        if self.pos >= self.data.len() {
            self.eof = true;
        }
    }

    fn capture_pattern(&mut self) -> bool {
        self.get8() == 0x4f && self.get8() == 0x67 && self.get8() == 0x67 && self.get8() == 0x53
    }

    fn start_page_no_capturepattern(&mut self) -> bool {
        if self.get8() != 0 {
            return false;
        }
        self.page_flag = self.get8();
        let loc0 = self.get32();
        let loc1 = self.get32();
        self.get32();
        self.get32();
        self.get32();
        self.segment_count = self.get8() as i32;
        let mut seg = [0u8; 255];
        if !self.getn(&mut seg[..self.segment_count as usize]) {
            return false;
        }
        self.segments[..self.segment_count as usize].copy_from_slice(&seg[..self.segment_count as usize]);
        self.end_seg_with_known_loc = -2;
        if loc0 != !0u32 || loc1 != !0u32 {
            let mut i = self.segment_count - 1;
            while i >= 0 {
                if self.segments[i as usize] < 255 {
                    break;
                }
                i -= 1;
            }
            if i >= 0 {
                self.end_seg_with_known_loc = i;
                self.known_loc_for_packet = loc0;
            }
        }
        self.next_seg = 0;
        true
    }

    fn start_page(&mut self) -> bool {
        if !self.capture_pattern() {
            return false;
        }
        self.start_page_no_capturepattern()
    }

    fn start_packet(&mut self) -> bool {
        while self.next_seg == -1 {
            if !self.start_page() {
                return false;
            }
            if self.page_flag & 1 != 0 {
                return false;
            }
        }
        self.last_seg = false;
        self.valid_bits = 0;
        self.bytes_in_seg = 0;
        true
    }

    fn maybe_start_packet(&mut self) -> bool {
        if self.next_seg == -1 {
            let x = self.get8();
            if self.eof {
                return false;
            }
            if x != 0x4f || self.get8() != 0x67 || self.get8() != 0x67 || self.get8() != 0x53 {
                return false;
            }
            if !self.start_page_no_capturepattern() {
                return false;
            }
            if self.page_flag & 1 != 0 {
                self.last_seg = false;
                self.bytes_in_seg = 0;
                return false;
            }
        }
        self.start_packet()
    }

    fn next_segment(&mut self) -> i32 {
        if self.last_seg {
            return 0;
        }
        if self.next_seg == -1 {
            self.last_seg_which = self.segment_count - 1;
            if !self.start_page() {
                self.last_seg = true;
                return 0;
            }
            if self.page_flag & 1 == 0 {
                return 0;
            }
        }
        let len = self.segments[self.next_seg as usize] as i32;
        self.next_seg += 1;
        if len < 255 {
            self.last_seg = true;
            self.last_seg_which = self.next_seg - 1;
        }
        if self.next_seg >= self.segment_count {
            self.next_seg = -1;
        }
        self.bytes_in_seg = len as u8;
        len
    }

    fn get8_packet_raw(&mut self) -> i32 {
        if self.bytes_in_seg == 0 {
            if self.last_seg || self.next_segment() == 0 {
                return EOP;
            }
        }
        self.bytes_in_seg -= 1;
        self.get8() as i32
    }

    fn get8_packet(&mut self) -> i32 {
        let x = self.get8_packet_raw();
        self.valid_bits = 0;
        x
    }

    fn flush_packet(&mut self) {
        while self.get8_packet_raw() != EOP {}
    }

    fn get_bits(&mut self, n: i32) -> u32 {
        if self.valid_bits < 0 {
            return 0;
        }
        if self.valid_bits < n {
            if n > 24 {
                let z = self.get_bits(24);
                return z.wrapping_add(self.get_bits(n - 24) << 24);
            }
            if self.valid_bits == 0 {
                self.acc = 0;
            }
            while self.valid_bits < n {
                let z = self.get8_packet_raw();
                if z == EOP {
                    self.valid_bits = EOP;
                    return 0;
                }
                self.acc = self.acc.wrapping_add((z as u32) << self.valid_bits);
                self.valid_bits += 8;
            }
        }
        let z = self.acc & ((1u32 << n) - 1);
        self.acc >>= n;
        self.valid_bits -= n;
        z
    }

    fn prep_huffman(&mut self) {
        if self.valid_bits <= 24 {
            if self.valid_bits == 0 {
                self.acc = 0;
            }
            loop {
                if self.last_seg && self.bytes_in_seg == 0 {
                    return;
                }
                let z = self.get8_packet_raw();
                if z == EOP {
                    return;
                }
                self.acc = self.acc.wrapping_add((z as u32) << self.valid_bits);
                self.valid_bits += 8;
                if self.valid_bits > 24 {
                    break;
                }
            }
        }
    }

    fn codebook_decode_scalar_raw(&mut self, c: &Codebook) -> i32 {
        self.prep_huffman();
        if c.codewords.is_none() && c.sorted_codewords.is_none() {
            return -1;
        }
        let use_sorted = if c.entries > 8 { c.sorted_codewords.is_some() } else { c.codewords.is_none() };
        if use_sorted {
            let sc = c.sorted_codewords.as_ref().unwrap();
            let code = bit_reverse(self.acc);
            let mut x = 0i32;
            let mut n = c.sorted_entries;
            while n > 1 {
                let m = x + (n >> 1);
                if sc[m as usize] <= code {
                    x = m;
                    n -= n >> 1;
                } else {
                    n >>= 1;
                }
            }
            if c.sparse == 0 {
                x = c.sorted_value(x);
            }
            let len = c.codeword_lengths[x as usize] as i32;
            if self.valid_bits >= len {
                self.acc >>= len;
                self.valid_bits -= len;
                return x;
            }
            self.valid_bits = 0;
            return -1;
        }
        let cw = c.codewords.as_ref().unwrap();
        for i in 0..c.entries as usize {
            let l = c.codeword_lengths[i];
            if l == NO_CODE {
                continue;
            }
            if cw[i] == self.acc & ((1u32 << l) - 1) {
                if self.valid_bits >= l as i32 {
                    self.acc >>= l;
                    self.valid_bits -= l as i32;
                    return i as i32;
                }
                self.valid_bits = 0;
                return -1;
            }
        }
        self.valid_bits = 0;
        -1
    }

    /// DECODE_RAW
    fn decode_raw(&mut self, c: &Codebook) -> i32 {
        if self.valid_bits < FAST_HUFFMAN_LENGTH as i32 {
            self.prep_huffman();
        }
        let mut z = c.fast_huffman[(self.acc & FAST_HUFFMAN_TABLE_MASK) as usize] as i32;
        if z >= 0 {
            let n = c.codeword_lengths[z as usize] as i32;
            self.acc >>= n;
            self.valid_bits -= n;
            if self.valid_bits < 0 {
                self.valid_bits = 0;
                z = -1;
            }
        } else {
            z = self.codebook_decode_scalar_raw(c);
        }
        z
    }

    /// DECODE: DECODE_RAW, then a sparse book's sorted index to its symbol
    fn decode(&mut self, c: &Codebook) -> i32 {
        let mut z = self.decode_raw(c);
        if c.sparse != 0 && z >= 0 {
            z = c.sorted_value(z);
        }
        z
    }

    fn codebook_decode_start(&mut self, c: &Codebook) -> i32 {
        let mut z = -1;
        if c.lookup_type != 0 {
            z = self.decode_raw(c);
            if z < 0 && self.bytes_in_seg == 0 && self.last_seg {
                return z;
            }
        }
        z
    }

    fn codebook_decode(&mut self, c: &Codebook, output: &mut [f32], off: usize, len: i32) -> bool {
        let mut z = self.codebook_decode_start(c);
        if z < 0 {
            return false;
        }
        let len = len.min(c.dimensions);
        z *= c.dimensions;
        if c.sequence_p != 0 {
            let mut last = 0f32;
            for i in 0..len.max(0) {
                let val = c.multiplicands[(z + i) as usize] + last;
                output[off + i as usize] += val;
                last = val + c.minimum_value;
            }
        } else {
            let last = 0f32;
            for i in 0..len.max(0) {
                output[off + i as usize] += c.multiplicands[(z + i) as usize] + last;
            }
        }
        true
    }

    fn codebook_decode_step(&mut self, c: &Codebook, output: &mut [f32], off: usize, len: i32, step: i32) -> bool {
        let mut z = self.codebook_decode_start(c);
        let mut last = 0f32;
        if z < 0 {
            return false;
        }
        let len = len.min(c.dimensions);
        z *= c.dimensions;
        for i in 0..len.max(0) {
            let val = c.multiplicands[(z + i) as usize] + last;
            output[off + (i * step) as usize] += val;
            if c.sequence_p != 0 {
                last = val;
            }
        }
        true
    }
}

fn bit_reverse(n: u32) -> u32 {
    let n = ((n & 0xAAAA_AAAA) >> 1) | ((n & 0x5555_5555) << 1);
    let n = ((n & 0xCCCC_CCCC) >> 2) | ((n & 0x3333_3333) << 2);
    let n = ((n & 0xF0F0_F0F0) >> 4) | ((n & 0x0F0F_0F0F) << 4);
    let n = ((n & 0xFF00_FF00) >> 8) | ((n & 0x00FF_00FF) << 8);
    (n >> 16) | (n << 16)
}

fn ilog(n: i32) -> i32 {
    const LOG2_4: [i8; 16] = [0, 1, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4];
    if n < 0 {
        return 0;
    }
    let l = |v: i32| LOG2_4[v as usize] as i32;
    if n < (1 << 14) {
        if n < (1 << 4) {
            l(n)
        } else if n < (1 << 9) {
            5 + l(n >> 5)
        } else {
            10 + l(n >> 10)
        }
    } else if n < (1 << 24) {
        if n < (1 << 19) {
            15 + l(n >> 15)
        } else {
            20 + l(n >> 20)
        }
    } else if n < (1 << 29) {
        25 + l(n >> 25)
    } else {
        30 + l(n >> 30)
    }
}

fn float32_unpack(x: u32) -> f32 {
    let mantissa = x & 0x1f_ffff;
    let sign = x & 0x8000_0000;
    let exp = ((x & 0x7fe0_0000) >> 21) as i32;
    let res = if sign != 0 { -(mantissa as f64) } else { mantissa as f64 };
    // ldexp((float)res, exp - 788): exp - 788 is in [-788, 235], so 2^e is a
    // normal double and the product is exact
    let e = exp - 788;
    let p = f64::from_bits(((e + 1023) as u64) << 52);
    ((res as f32) as f64 * p) as f32
}

fn add_entry(c: &mut Codebook, huff_code: u32, symbol: i32, count: i32, len: u8, values: &mut [u32]) {
    let cw = c.codewords.as_mut().unwrap();
    if c.sparse == 0 {
        cw[symbol as usize] = huff_code;
    } else {
        cw[count as usize] = huff_code;
        c.codeword_lengths[count as usize] = len;
        values[count as usize] = symbol as u32;
    }
}

fn compute_codewords(c: &mut Codebook, len: &[u8], n: i32, values: &mut [u32]) -> bool {
    let mut available = [0u32; 32];
    let mut m = 0;
    let mut k = 0;
    while k < n {
        if len[k as usize] < 255 {
            break;
        }
        k += 1;
    }
    if k == n {
        return true;
    }
    add_entry(c, 0, k, m, len[k as usize], values);
    m += 1;
    for i in 1..=len[k as usize] as usize {
        available[i] = 1u32 << (32 - i);
    }
    for i in (k + 1)..n {
        let mut z = len[i as usize] as i32;
        if z == 255 {
            continue;
        }
        while z > 0 && available[z as usize] == 0 {
            z -= 1;
        }
        if z == 0 {
            return false;
        }
        let res = available[z as usize];
        available[z as usize] = 0;
        add_entry(c, bit_reverse(res), i, m, len[i as usize], values);
        m += 1;
        if z != len[i as usize] as i32 {
            let mut y = len[i as usize] as i32;
            while y > z {
                available[y as usize] = res.wrapping_add(1u32 << (32 - y));
                y -= 1;
            }
        }
    }
    true
}

fn compute_accelerated_huffman(c: &mut Codebook) {
    c.fast_huffman = vec![-1; FAST_HUFFMAN_TABLE_SIZE];
    let len = if c.sparse != 0 { c.sorted_entries } else { c.entries }.min(32767);
    for i in 0..len.max(0) as usize {
        let l = c.codeword_lengths[i] as u32;
        if l <= FAST_HUFFMAN_LENGTH {
            let mut z = if c.sparse != 0 {
                bit_reverse(c.sorted_codewords.as_ref().unwrap()[i])
            } else {
                c.codewords.as_ref().unwrap()[i]
            };
            while z < FAST_HUFFMAN_TABLE_SIZE as u32 {
                c.fast_huffman[z as usize] = i as i16;
                z += 1 << l;
            }
        }
    }
}

fn include_in_sort(c: &Codebook, len: u8) -> bool {
    if c.sparse != 0 {
        return true;
    }
    if len == NO_CODE {
        return false;
    }
    len as u32 > FAST_HUFFMAN_LENGTH
}

fn compute_sorted_huffman(c: &mut Codebook, lengths: &[u8], values: &[u32]) {
    let se = c.sorted_entries as usize;
    {
        let cw = c.codewords.as_ref().unwrap();
        let mut sc = c.sorted_codewords.take().unwrap();
        if c.sparse == 0 {
            let mut k = 0;
            for i in 0..c.entries as usize {
                if include_in_sort(c, lengths[i]) {
                    sc[k] = bit_reverse(cw[i]);
                    k += 1;
                }
            }
        } else {
            for i in 0..se {
                sc[i] = bit_reverse(cw[i]);
            }
        }
        sc[..se].sort_unstable();
        sc[se] = 0xffff_ffff;
        c.sorted_codewords = Some(sc);
    }
    let len = if c.sparse != 0 { c.sorted_entries } else { c.entries };
    for i in 0..len as usize {
        let huff_len = if c.sparse != 0 { lengths[values[i] as usize] } else { lengths[i] };
        if include_in_sort(c, huff_len) {
            let code = bit_reverse(c.codewords.as_ref().unwrap()[i]);
            let sc = c.sorted_codewords.as_ref().unwrap();
            let mut x = 0i32;
            let mut n = c.sorted_entries;
            while n > 1 {
                let m = x + (n >> 1);
                if sc[m as usize] <= code {
                    x = m;
                    n -= n >> 1;
                } else {
                    n >>= 1;
                }
            }
            if c.sparse != 0 {
                c.sorted_values[(x + 1) as usize] = values[i] as i32;
                c.codeword_lengths[x as usize] = huff_len;
            } else {
                c.sorted_values[(x + 1) as usize] = i as i32;
            }
        }
    }
}

fn lookup1_values(entries: i32, dim: i32) -> i32 {
    let l = super::aormath::log((entries as f32) as f64) as f32;
    let mut r = ((l / dim as f32) as f64).exp().floor() as i32;
    if super::aormath::pow((r as f32 + 1.0) as f64, dim as f64).floor() as i32 <= entries {
        r += 1;
    }
    if super::aormath::pow((r as f32 + 1.0) as f64, dim as f64) <= entries as f64 {
        return -1;
    }
    if super::aormath::pow(r as f32 as f64, dim as f64).floor() as i32 > entries {
        return -1;
    }
    r
}

/// One block size's twiddle factors, window and bit-reverse table.
#[derive(Default)]
struct Block {
    a: Vec<f32>,
    b: Vec<f32>,
    c: Vec<f32>,
    window: Vec<f32>,
    bit_reverse: Vec<u16>,
}

fn init_blocksize(n: usize) -> Block {
    let (n2, n4, n8) = (n >> 1, n >> 2, n >> 3);
    let mut a = vec![0f32; n2];
    let mut b = vec![0f32; n2];
    let mut c = vec![0f32; n4];
    let nf = n as f64;
    for k in 0..n4 {
        let k2 = 2 * k;
        a[k2] = ((4 * k) as f64 * PI / nf).cos() as f32;
        a[k2 + 1] = (-((4 * k) as f64 * PI / nf).sin()) as f32;
        b[k2] = ((k2 + 1) as f64 * PI / nf / 2.0).cos() as f32 * 0.5f32;
        b[k2 + 1] = ((k2 + 1) as f64 * PI / nf / 2.0).sin() as f32 * 0.5f32;
    }
    for k in 0..n8 {
        let k2 = 2 * k;
        c[k2] = ((2 * (k2 + 1)) as f64 * PI / nf).cos() as f32;
        c[k2 + 1] = (-((2 * (k2 + 1)) as f64 * PI / nf).sin()) as f32;
    }
    let mut window = vec![0f32; n2];
    for (i, w) in window.iter_mut().enumerate() {
        let s = ((i as f64 + 0.5) / n2 as f64 * 0.5 * PI).sin() as f32;
        *w = (0.5 * PI * (s * s) as f64).sin() as f32;
    }
    let ld = ilog(n as i32) - 1;
    let bit_reverse = (0..n8).map(|i| ((self::bit_reverse(i as u32) >> (32 - ld + 3)) << 2) as u16).collect();
    Block { a, b, c, window, bit_reverse }
}

// ---- inverse MDCT; indices stand in for stb_vorbis's pointers ----

fn imdct_step3_iter0_loop(n: i32, e: &mut [f32], i_off: isize, k_off: isize, a: &[f32]) {
    let mut ee0 = i_off;
    let mut ee2 = ee0 + k_off;
    let mut ai = 0usize;
    let ix = |p: isize| p as usize;
    for _ in 0..(n >> 2) {
        for q in 0..4isize {
            let o = -2 * q;
            let k00_20 = e[ix(ee0 + o)] - e[ix(ee2 + o)];
            let k01_21 = e[ix(ee0 + o - 1)] - e[ix(ee2 + o - 1)];
            e[ix(ee0 + o)] += e[ix(ee2 + o)];
            e[ix(ee0 + o - 1)] += e[ix(ee2 + o - 1)];
            e[ix(ee2 + o)] = k00_20 * a[ai] - k01_21 * a[ai + 1];
            e[ix(ee2 + o - 1)] = k01_21 * a[ai] + k00_20 * a[ai + 1];
            ai += 8;
        }
        ee0 -= 8;
        ee2 -= 8;
    }
}

fn imdct_step3_inner_r_loop(lim: i32, e: &mut [f32], d0: isize, k_off: isize, a: &[f32], k1: usize) {
    let mut e0 = d0;
    let mut e2 = e0 + k_off;
    let mut ai = 0usize;
    let ix = |p: isize| p as usize;
    for _ in 0..(lim >> 2) {
        for q in 0..4isize {
            let o = -2 * q;
            let k00_20 = e[ix(e0 + o)] - e[ix(e2 + o)];
            let k01_21 = e[ix(e0 + o - 1)] - e[ix(e2 + o - 1)];
            e[ix(e0 + o)] += e[ix(e2 + o)];
            e[ix(e0 + o - 1)] += e[ix(e2 + o - 1)];
            e[ix(e2 + o)] = k00_20 * a[ai] - k01_21 * a[ai + 1];
            e[ix(e2 + o - 1)] = k01_21 * a[ai] + k00_20 * a[ai + 1];
            ai += k1;
        }
        e0 -= 8;
        e2 -= 8;
    }
}

#[allow(clippy::too_many_arguments)]
fn imdct_step3_inner_s_loop(n: i32, e: &mut [f32], i_off: isize, k_off: isize, a: &[f32], a_base: usize, a_off: usize, k0: isize) {
    let aa = [
        a[a_base],
        a[a_base + 1],
        a[a_base + a_off],
        a[a_base + a_off + 1],
        a[a_base + a_off * 2],
        a[a_base + a_off * 2 + 1],
        a[a_base + a_off * 3],
        a[a_base + a_off * 3 + 1],
    ];
    let mut ee0 = i_off;
    let mut ee2 = ee0 + k_off;
    let ix = |p: isize| p as usize;
    for _ in 0..n {
        for q in 0..4isize {
            let o = -2 * q;
            let (c0, c1) = (aa[2 * q as usize], aa[2 * q as usize + 1]);
            let k00 = e[ix(ee0 + o)] - e[ix(ee2 + o)];
            let k11 = e[ix(ee0 + o - 1)] - e[ix(ee2 + o - 1)];
            e[ix(ee0 + o)] += e[ix(ee2 + o)];
            e[ix(ee0 + o - 1)] += e[ix(ee2 + o - 1)];
            e[ix(ee2 + o)] = k00 * c0 - k11 * c1;
            e[ix(ee2 + o - 1)] = k11 * c0 + k00 * c1;
        }
        ee0 -= k0;
        ee2 -= k0;
    }
}

fn iter_54(e: &mut [f32], z: isize) {
    let g = |e: &[f32], o: isize| e[(z + o) as usize];
    let k00 = g(e, 0) - g(e, -4);
    let y0 = g(e, 0) + g(e, -4);
    let y2 = g(e, -2) + g(e, -6);
    let k22 = g(e, -2) - g(e, -6);
    e[z as usize] = y0 + y2;
    e[(z - 2) as usize] = y0 - y2;
    let k33 = g(e, -3) - g(e, -7);
    e[(z - 4) as usize] = k00 + k33;
    e[(z - 6) as usize] = k00 - k33;
    let k11 = g(e, -1) - g(e, -5);
    let y1 = g(e, -1) + g(e, -5);
    let y3 = g(e, -3) + g(e, -7);
    e[(z - 1) as usize] = y1 + y3;
    e[(z - 3) as usize] = y1 - y3;
    e[(z - 5) as usize] = k11 - k22;
    e[(z - 7) as usize] = k11 + k22;
}

fn imdct_step3_inner_s_loop_ld654(n: i32, e: &mut [f32], i_off: isize, a: &[f32], base_n: usize) {
    let a2 = a[base_n >> 3];
    let mut z = i_off;
    let base = z - 16 * n as isize;
    let ix = |p: isize| p as usize;
    while z > base {
        let k00 = e[ix(z)] - e[ix(z - 8)];
        let k11 = e[ix(z - 1)] - e[ix(z - 9)];
        let l00 = e[ix(z - 2)] - e[ix(z - 10)];
        let l11 = e[ix(z - 3)] - e[ix(z - 11)];
        e[ix(z)] += e[ix(z - 8)];
        e[ix(z - 1)] += e[ix(z - 9)];
        e[ix(z - 2)] += e[ix(z - 10)];
        e[ix(z - 3)] += e[ix(z - 11)];
        e[ix(z - 8)] = k00;
        e[ix(z - 9)] = k11;
        e[ix(z - 10)] = (l00 + l11) * a2;
        e[ix(z - 11)] = (l11 - l00) * a2;
        let k00 = e[ix(z - 4)] - e[ix(z - 12)];
        let k11 = e[ix(z - 5)] - e[ix(z - 13)];
        let l00 = e[ix(z - 6)] - e[ix(z - 14)];
        let l11 = e[ix(z - 7)] - e[ix(z - 15)];
        e[ix(z - 4)] += e[ix(z - 12)];
        e[ix(z - 5)] += e[ix(z - 13)];
        e[ix(z - 6)] += e[ix(z - 14)];
        e[ix(z - 7)] += e[ix(z - 15)];
        e[ix(z - 12)] = k11;
        e[ix(z - 13)] = -k00;
        e[ix(z - 14)] = (l11 - l00) * a2;
        e[ix(z - 15)] = (l00 + l11) * -a2;
        iter_54(e, z);
        iter_54(e, z - 8);
        z -= 16;
    }
}

fn inverse_mdct(buffer: &mut [f32], n: usize, blk: &Block, buf2: &mut Vec<f32>) {
    let (n2, n4, n8) = (n >> 1, n >> 2, n >> 3);
    let ni = n as isize;
    let (n2i, n4i, n8i) = (n2 as isize, n4 as isize, n8 as isize);
    let a = &blk.a[..];
    buf2.clear();
    buf2.resize(n2, 0.0);
    {
        let mut d = n2i - 2;
        let mut aa = 0usize;
        let mut e = 0usize;
        while e != n2 {
            buf2[(d + 1) as usize] = buffer[e] * a[aa] - buffer[e + 2] * a[aa + 1];
            buf2[d as usize] = buffer[e] * a[aa + 1] + buffer[e + 2] * a[aa];
            d -= 2;
            aa += 2;
            e += 4;
        }
        let mut e = n2i - 3;
        while d >= 0 {
            let (e0, e2) = (buffer[e as usize], buffer[(e + 2) as usize]);
            buf2[(d + 1) as usize] = -e2 * a[aa] - -e0 * a[aa + 1];
            buf2[d as usize] = -e2 * a[aa + 1] + -e0 * a[aa];
            d -= 2;
            aa += 2;
            e -= 4;
        }
    }
    let u = buffer;
    let v = &mut buf2[..];
    {
        let mut aa = n2i - 8;
        let (mut e0, mut e1, mut d0, mut d1) = (n4, 0usize, n4, 0usize);
        while aa >= 0 {
            let ai = aa as usize;
            let v41_21 = v[e0 + 1] - v[e1 + 1];
            let v40_20 = v[e0] - v[e1];
            u[d0 + 1] = v[e0 + 1] + v[e1 + 1];
            u[d0] = v[e0] + v[e1];
            u[d1 + 1] = v41_21 * a[ai + 4] - v40_20 * a[ai + 5];
            u[d1] = v40_20 * a[ai + 4] + v41_21 * a[ai + 5];
            let v41_21 = v[e0 + 3] - v[e1 + 3];
            let v40_20 = v[e0 + 2] - v[e1 + 2];
            u[d0 + 3] = v[e0 + 3] + v[e1 + 3];
            u[d0 + 2] = v[e0 + 2] + v[e1 + 2];
            u[d1 + 3] = v41_21 * a[ai] - v40_20 * a[ai + 1];
            u[d1 + 2] = v40_20 * a[ai] + v41_21 * a[ai + 1];
            aa -= 8;
            d0 += 4;
            d1 += 4;
            e0 += 4;
            e1 += 4;
        }
    }
    let ld = ilog(n as i32) - 1;
    imdct_step3_iter0_loop((n >> 4) as i32, u, n2i - 1, -(ni >> 3), a);
    imdct_step3_iter0_loop((n >> 4) as i32, u, n2i - 1 - n4i, -(ni >> 3), a);
    for k in 0..4 {
        imdct_step3_inner_r_loop((n >> 5) as i32, u, n2i - 1 - n8i * k, -(ni >> 4), a, 16);
    }
    let mut l = 2i32;
    while l < (ld - 3) >> 1 {
        let k0 = ni >> (l + 2);
        let k0_2 = k0 >> 1;
        let lim = 1isize << (l + 1);
        for i in 0..lim {
            imdct_step3_inner_r_loop((n >> (l + 4)) as i32, u, n2i - 1 - k0 * i, -k0_2, a, 1 << (l + 3));
        }
        l += 1;
    }
    while l < ld - 6 {
        let k0 = ni >> (l + 2);
        let k1 = 1usize << (l + 3);
        let k0_2 = k0 >> 1;
        let rlim = n >> (l + 6);
        let lim = 1i32 << (l + 1);
        let mut a0 = 0usize;
        let mut i_off = n2i - 1;
        for _ in 0..rlim {
            imdct_step3_inner_s_loop(lim, u, i_off, -k0_2, a, a0, k1, k0);
            a0 += k1 * 4;
            i_off -= 8;
        }
        l += 1;
    }
    imdct_step3_inner_s_loop_ld654((n >> 5) as i32, u, n2i - 1, a, n);
    {
        let bitrev = &blk.bit_reverse;
        let mut br = 0usize;
        let mut d0 = n4i - 4;
        let mut d1 = n2i - 4;
        while d0 >= 0 {
            let (p0, p1) = (d0 as usize, d1 as usize);
            let k4 = bitrev[br] as usize;
            v[p1 + 3] = u[k4];
            v[p1 + 2] = u[k4 + 1];
            v[p0 + 3] = u[k4 + 2];
            v[p0 + 2] = u[k4 + 3];
            let k4 = bitrev[br + 1] as usize;
            v[p1 + 1] = u[k4];
            v[p1] = u[k4 + 1];
            v[p0 + 1] = u[k4 + 2];
            v[p0] = u[k4 + 3];
            d0 -= 4;
            d1 -= 4;
            br += 2;
        }
    }
    {
        let c = &blk.c;
        let mut ci = 0usize;
        let mut d = 0isize;
        let mut e = n2i - 4;
        while d < e {
            let (di, ei) = (d as usize, e as usize);
            let a02 = v[di] - v[ei + 2];
            let a11 = v[di + 1] + v[ei + 3];
            let b0 = c[ci + 1] * a02 + c[ci] * a11;
            let b1 = c[ci + 1] * a11 - c[ci] * a02;
            let b2 = v[di] + v[ei + 2];
            let b3 = v[di + 1] - v[ei + 3];
            v[di] = b2 + b0;
            v[di + 1] = b3 + b1;
            v[ei + 2] = b2 - b0;
            v[ei + 3] = b1 - b3;
            let a02 = v[di + 2] - v[ei];
            let a11 = v[di + 3] + v[ei + 1];
            let b0 = c[ci + 3] * a02 + c[ci + 2] * a11;
            let b1 = c[ci + 3] * a11 - c[ci + 2] * a02;
            let b2 = v[di + 2] + v[ei];
            let b3 = v[di + 3] - v[ei + 1];
            v[di + 2] = b2 + b0;
            v[di + 3] = b3 + b1;
            v[ei] = b2 - b0;
            v[ei + 1] = b1 - b3;
            ci += 4;
            d += 4;
            e -= 4;
        }
    }
    {
        let b = &blk.b;
        let mut bi = n2i - 8;
        let mut e = n2i - 8;
        let (mut d0, mut d1, mut d2, mut d3) = (0isize, n2i - 4, n2i, ni - 4);
        while e >= 0 {
            let (ei, bb) = (e as usize, bi as usize);
            let (p0, p1, p2, p3) = (d0 as usize, d1 as usize, d2 as usize, d3 as usize);
            let q3 = v[ei + 6] * b[bb + 7] - v[ei + 7] * b[bb + 6];
            let q2 = -v[ei + 6] * b[bb + 6] - v[ei + 7] * b[bb + 7];
            u[p0] = q3;
            u[p1 + 3] = -q3;
            u[p2] = q2;
            u[p3 + 3] = q2;
            let q1 = v[ei + 4] * b[bb + 5] - v[ei + 5] * b[bb + 4];
            let q0 = -v[ei + 4] * b[bb + 4] - v[ei + 5] * b[bb + 5];
            u[p0 + 1] = q1;
            u[p1 + 2] = -q1;
            u[p2 + 1] = q0;
            u[p3 + 2] = q0;
            let q3 = v[ei + 2] * b[bb + 3] - v[ei + 3] * b[bb + 2];
            let q2 = -v[ei + 2] * b[bb + 2] - v[ei + 3] * b[bb + 3];
            u[p0 + 2] = q3;
            u[p1 + 1] = -q3;
            u[p2 + 2] = q2;
            u[p3 + 1] = q2;
            let q1 = v[ei] * b[bb + 1] - v[ei + 1] * b[bb];
            let q0 = -v[ei] * b[bb] - v[ei + 1] * b[bb + 1];
            u[p0 + 3] = q1;
            u[p1] = -q1;
            u[p2 + 3] = q0;
            u[p3] = q0;
            bi -= 8;
            e -= 8;
            d0 += 4;
            d2 += 4;
            d1 -= 4;
            d3 -= 4;
        }
    }
}

#[rustfmt::skip]
static INVERSE_DB_TABLE: [f32; 256] = [
  1.0649863e-07, 1.1341951e-07, 1.2079015e-07, 1.2863978e-07,
  1.3699951e-07, 1.4590251e-07, 1.5538408e-07, 1.6548181e-07,
  1.7623575e-07, 1.8768855e-07, 1.9988561e-07, 2.1287530e-07,
  2.2670913e-07, 2.4144197e-07, 2.5713223e-07, 2.7384213e-07,
  2.9163793e-07, 3.1059021e-07, 3.3077411e-07, 3.5226968e-07,
  3.7516214e-07, 3.9954229e-07, 4.2550680e-07, 4.5315863e-07,
  4.8260743e-07, 5.1396998e-07, 5.4737065e-07, 5.8294187e-07,
  6.2082472e-07, 6.6116941e-07, 7.0413592e-07, 7.4989464e-07,
  7.9862701e-07, 8.5052630e-07, 9.0579828e-07, 9.6466216e-07,
  1.0273513e-06, 1.0941144e-06, 1.1652161e-06, 1.2409384e-06,
  1.3215816e-06, 1.4074654e-06, 1.4989305e-06, 1.5963394e-06,
  1.7000785e-06, 1.8105592e-06, 1.9282195e-06, 2.0535261e-06,
  2.1869758e-06, 2.3290978e-06, 2.4804557e-06, 2.6416497e-06,
  2.8133190e-06, 2.9961443e-06, 3.1908506e-06, 3.3982101e-06,
  3.6190449e-06, 3.8542308e-06, 4.1047004e-06, 4.3714470e-06,
  4.6555282e-06, 4.9580707e-06, 5.2802740e-06, 5.6234160e-06,
  5.9888572e-06, 6.3780469e-06, 6.7925283e-06, 7.2339451e-06,
  7.7040476e-06, 8.2047000e-06, 8.7378876e-06, 9.3057248e-06,
  9.9104632e-06, 1.0554501e-05, 1.1240392e-05, 1.1970856e-05,
  1.2748789e-05, 1.3577278e-05, 1.4459606e-05, 1.5399272e-05,
  1.6400004e-05, 1.7465768e-05, 1.8600792e-05, 1.9809576e-05,
  2.1096914e-05, 2.2467911e-05, 2.3928002e-05, 2.5482978e-05,
  2.7139006e-05, 2.8902651e-05, 3.0780908e-05, 3.2781225e-05,
  3.4911534e-05, 3.7180282e-05, 3.9596466e-05, 4.2169667e-05,
  4.4910090e-05, 4.7828601e-05, 5.0936773e-05, 5.4246931e-05,
  5.7772202e-05, 6.1526565e-05, 6.5524908e-05, 6.9783085e-05,
  7.4317983e-05, 7.9147585e-05, 8.4291040e-05, 8.9768747e-05,
  9.5602426e-05, 0.00010181521, 0.00010843174, 0.00011547824,
  0.00012298267, 0.00013097477, 0.00013948625, 0.00014855085,
  0.00015820453, 0.00016848555, 0.00017943469, 0.00019109536,
  0.00020351382, 0.00021673929, 0.00023082423, 0.00024582449,
  0.00026179955, 0.00027881276, 0.00029693158, 0.00031622787,
  0.00033677814, 0.00035866388, 0.00038197188, 0.00040679456,
  0.00043323036, 0.00046138411, 0.00049136745, 0.00052329927,
  0.00055730621, 0.00059352311, 0.00063209358, 0.00067317058,
  0.00071691700, 0.00076350630, 0.00081312324, 0.00086596457,
  0.00092223983, 0.00098217216, 0.0010459992, 0.0011139742,
  0.0011863665, 0.0012634633, 0.0013455702, 0.0014330129,
  0.0015261382, 0.0016253153, 0.0017309374, 0.0018434235,
  0.0019632195, 0.0020908006, 0.0022266726, 0.0023713743,
  0.0025254795, 0.0026895994, 0.0028643847, 0.0030505286,
  0.0032487691, 0.0034598925, 0.0036847358, 0.0039241906,
  0.0041792066, 0.0044507950, 0.0047400328, 0.0050480668,
  0.0053761186, 0.0057254891, 0.0060975636, 0.0064938176,
  0.0069158225, 0.0073652516, 0.0078438871, 0.0083536271,
  0.0088964928, 0.009474637, 0.010090352, 0.010746080,
  0.011444421, 0.012188144, 0.012980198, 0.013823725,
  0.014722068, 0.015678791, 0.016697687, 0.017782797,
  0.018938423, 0.020169149, 0.021479854, 0.022875735,
  0.024362330, 0.025945531, 0.027631618, 0.029427276,
  0.031339626, 0.033376252, 0.035545228, 0.037855157,
  0.040315199, 0.042935108, 0.045725273, 0.048696758,
  0.051861348, 0.055231591, 0.058820850, 0.062643361,
  0.066714279, 0.071049749, 0.075666962, 0.080584227,
  0.085821044, 0.091398179, 0.097337747, 0.10366330,
  0.11039993, 0.11757434, 0.12521498, 0.13335215,
  0.14201813, 0.15124727, 0.16107617, 0.17154380,
  0.18269168, 0.19456402, 0.20720788, 0.22067342,
  0.23501402, 0.25028656, 0.26655159, 0.28387361,
  0.30232132, 0.32196786, 0.34289114, 0.36517414,
  0.38890521, 0.41417847, 0.44109412, 0.46975890,
  0.50028648, 0.53279791, 0.56742212, 0.60429640,
  0.64356699, 0.68538959, 0.72993007, 0.77736504,
  0.82788260, 0.88168307, 0.9389798, 1.0,
];

fn predict_point(x: i32, x0: i32, x1: i32, y0: i32, y1: i32) -> i32 {
    let dy = y1 - y0;
    let adx = x1 - x0;
    let err = dy.abs() * (x - x0);
    let off = err / adx;
    if dy < 0 {
        y0 - off
    } else {
        y0 + off
    }
}

fn draw_line(output: &mut [f32], x0: i32, y0: i32, x1: i32, y1: i32, n: i32) {
    let dy = y1 - y0;
    let adx = x1 - x0;
    let mut ady = dy.abs();
    let mut x = x0;
    let mut y = y0;
    let mut err = 0;
    let base = dy / adx;
    let sy = if dy < 0 { base - 1 } else { base + 1 };
    ady -= base.abs() * adx;
    let x1 = x1.min(n);
    if x < x1 {
        output[x as usize] *= INVERSE_DB_TABLE[(y as u32 & 255) as usize];
        x += 1;
        while x < x1 {
            err += ady;
            if err >= adx {
                err -= adx;
                y += sy;
            } else {
                y += base;
            }
            output[x as usize] *= INVERSE_DB_TABLE[(y as u32 & 255) as usize];
            x += 1;
        }
    }
}

/// The decoder state of one mono stream.
struct Vorb<'a> {
    s: Stream<'a>,
    blocksize: [usize; 2],
    codebooks: Vec<Codebook>,
    floor_config: Vec<Floor1>,
    residue_types: [u16; 64],
    residue_config: Vec<Residue>,
    mapping: Vec<Mapping>,
    mode_count: i32,
    mode_config: [Mode; 64],
    channel_buffer: Vec<f32>,
    previous_window: Vec<f32>,
    previous_length: i32,
    final_y: Vec<i16>,
    current_loc: u32,
    current_loc_valid: bool,
    first_decode: bool,
    discard_samples_deferred: i32,
    blocks: [Block; 2],
    buf2: Vec<f32>,
}

impl<'a> Vorb<'a> {
    fn new(data: &'a [u8]) -> Self {
        Vorb {
            s: Stream {
                data,
                pos: 0,
                eof: false,
                page_flag: 0,
                segment_count: 0,
                segments: [0; 255],
                next_seg: 0,
                last_seg: false,
                last_seg_which: 0,
                bytes_in_seg: 0,
                acc: 0,
                valid_bits: 0,
                end_seg_with_known_loc: 0,
                known_loc_for_packet: 0,
            },
            blocksize: [0; 2],
            codebooks: Vec::new(),
            floor_config: Vec::new(),
            residue_types: [0; 64],
            residue_config: Vec::new(),
            mapping: Vec::new(),
            mode_count: 0,
            mode_config: [Mode::default(); 64],
            channel_buffer: Vec::new(),
            previous_window: Vec::new(),
            previous_length: 0,
            final_y: Vec::new(),
            current_loc: 0,
            current_loc_valid: false,
            first_decode: false,
            discard_samples_deferred: 0,
            blocks: [Block::default(), Block::default()],
            buf2: Vec::new(),
        }
    }

    fn read_codebook(&mut self) -> Option<Codebook> {
        let f = &mut self.s;
        let mut c = Codebook::default();
        if f.get_bits(8) != 0x42 || f.get_bits(8) != 0x43 || f.get_bits(8) != 0x56 {
            return None;
        }
        let x = f.get_bits(8) as u8;
        c.dimensions = ((f.get_bits(8) << 8) + x as u32) as i32;
        let x = f.get_bits(8) as u8;
        let y = f.get_bits(8) as u8;
        c.entries = ((f.get_bits(8) << 16) + ((y as u32) << 8) + x as u32) as i32;
        let ordered = f.get_bits(1);
        c.sparse = if ordered != 0 { 0 } else { f.get_bits(1) as u8 };
        if c.dimensions == 0 && c.entries != 0 {
            return None;
        }
        if f.valid_bits < 0 {
            return None;
        }
        let entries = c.entries as usize;
        let mut lengths = alloc(c.entries as u64, entries, 0u8)?;
        let mut total = 0;
        if ordered != 0 {
            let mut current_entry = 0i32;
            let mut current_length = f.get_bits(5) as i32 + 1;
            while current_entry < c.entries {
                let limit = c.entries - current_entry;
                let n = f.get_bits(ilog(limit)) as i32;
                if f.valid_bits < 0 || current_length >= 32 || current_entry + n > c.entries {
                    return None;
                }
                for l in &mut lengths[current_entry as usize..(current_entry + n) as usize] {
                    *l = current_length as u8;
                }
                current_entry += n;
                current_length += 1;
            }
        } else {
            for l in lengths.iter_mut() {
                let present = if c.sparse != 0 { f.get_bits(1) } else { 1 };
                if f.valid_bits < 0 {
                    return None;
                }
                if present != 0 {
                    *l = f.get_bits(5) as u8 + 1;
                    total += 1;
                    if *l == 32 {
                        return None;
                    }
                } else {
                    *l = NO_CODE;
                }
            }
        }
        if c.sparse != 0 && total >= c.entries >> 2 {
            c.sparse = 0;
        }
        let sorted_count = if c.sparse != 0 {
            total
        } else {
            lengths.iter().filter(|&&l| l > 10 && l != NO_CODE).count() as i32
        };
        c.sorted_entries = sorted_count;
        let se = sorted_count as usize;
        let mut values: Vec<u32> = Vec::new();
        if c.sparse == 0 {
            c.codewords = Some(alloc(4 * c.entries as u64, entries, 0u32)?);
            c.codeword_lengths = lengths.clone();
        } else if se != 0 {
            c.codeword_lengths = alloc(se as u64, se, 0u8)?;
            c.codewords = Some(alloc(4 * se as u64, se, 0u32)?);
            values = alloc(4 * se as u64, se, 0u32)?;
        } else {
            // a sparse book without a used entry: stb_vorbis leaves its
            // codewords NULL
            c.codewords = Some(Vec::new());
        }
        let ne = c.entries;
        if !compute_codewords(&mut c, &lengths, ne, &mut values) {
            return None;
        }
        if se != 0 {
            c.sorted_codewords = Some(alloc(4 * (se as u64 + 1), se + 1, 0u32)?);
            c.sorted_values = alloc(4 * (se as u64 + 1), se + 1, 0i32)?;
            c.sorted_values[0] = -1;
            compute_sorted_huffman(&mut c, &lengths, &values);
        }
        if c.sparse != 0 {
            c.codewords = None;
        }
        compute_accelerated_huffman(&mut c);

        let f = &mut self.s;
        c.lookup_type = f.get_bits(4) as u8;
        if c.lookup_type > 2 {
            return None;
        }
        if c.lookup_type > 0 {
            c.minimum_value = float32_unpack(f.get_bits(32));
            c.delta_value = float32_unpack(f.get_bits(32));
            c.value_bits = f.get_bits(4) as u8 + 1;
            c.sequence_p = f.get_bits(1) as u8;
            if c.lookup_type == 1 {
                let values = lookup1_values(c.entries, c.dimensions);
                if values < 0 {
                    return None;
                }
                c.lookup_values = values as u32 as u64;
            } else {
                c.lookup_values = c.entries as u64 * c.dimensions as u64;
            }
            if c.lookup_values == 0 {
                return None;
            }
            let mut mults = alloc(2 * c.lookup_values, c.lookup_values as usize, 0u16)?;
            for m in mults.iter_mut() {
                let q = f.get_bits(c.value_bits as i32);
                if f.valid_bits < 0 {
                    return None;
                }
                *m = q as u16;
            }
            if c.lookup_type == 1 {
                let sparse = c.sparse != 0;
                let mut last = 0f32;
                let dims = c.dimensions as usize;
                let skip = sparse && c.sorted_entries == 0;
                if !skip {
                    let len = if sparse { se } else { entries };
                    c.multiplicands = alloc(4 * len as u64 * dims as u64, len * dims, 0f32)?;
                    for j in 0..len {
                        let z = if sparse { c.sorted_values[j + 1] as u32 } else { j as u32 };
                        let mut div = 1u32;
                        for k in 0..dims {
                            let off = ((z / div) as u64 % c.lookup_values) as usize;
                            let val = mults[off] as f32 * c.delta_value + c.minimum_value + last;
                            c.multiplicands[j * dims + k] = val;
                            if c.sequence_p != 0 {
                                last = val;
                            }
                            if k + 1 < dims {
                                if div as u64 > u32::MAX as u64 / (c.lookup_values as u32) as u64 {
                                    return None;
                                }
                                div = div.wrapping_mul(c.lookup_values as u32);
                            }
                        }
                    }
                    c.lookup_type = 2;
                }
            } else {
                let mut last = 0f32;
                let lv = c.lookup_values as usize;
                c.multiplicands = alloc(4 * c.lookup_values, lv, 0f32)?;
                for j in 0..lv {
                    let val = mults[j] as f32 * c.delta_value + c.minimum_value + last;
                    c.multiplicands[j] = val;
                    if c.sequence_p != 0 {
                        last = val;
                    }
                }
            }
        }
        Some(c)
    }

    /// start_decoder; None where it fails, and for a stream that isn't mono
    fn start_decoder(&mut self) -> Option<()> {
        self.first_decode = true;
        let f = &mut self.s;
        if !f.start_page() {
            return None;
        }
        if f.page_flag & 2 == 0 || f.page_flag & 4 != 0 || f.page_flag & 1 != 0 || f.segment_count != 1 {
            return None;
        }
        // 64 would be an Ogg skeleton stream, refused too
        if f.segments[0] != 30 {
            return None;
        }
        if f.get8() as i32 != PACKET_ID {
            return None;
        }
        let mut header = [0u8; 6];
        if !f.getn(&mut header) || &header != b"vorbis" {
            return None;
        }
        if f.get32() != 0 {
            return None;
        }
        // channels: 0 and more than 16 are errors, and libxmp's oggdec refuses
        // anything but 1 once decoded
        if f.get8() != 1 {
            return None;
        }
        if f.get32() == 0 {
            return None;
        }
        f.get32();
        f.get32();
        f.get32();
        let x = f.get8();
        let (log0, log1) = (x & 15, x >> 4);
        if !(6..=13).contains(&log0) || !(6..=13).contains(&log1) || log0 > log1 {
            return None;
        }
        self.blocksize = [1 << log0, 1 << log1];
        if f.get8() & 1 == 0 {
            return None;
        }
        if !f.start_page() || !f.start_packet() {
            return None;
        }
        loop {
            let len = f.next_segment();
            f.skip(len);
            f.bytes_in_seg = 0;
            if len == 0 {
                break;
            }
        }
        if !f.start_packet() {
            return None;
        }
        if f.get8_packet() != PACKET_SETUP {
            return None;
        }
        for h in header.iter_mut() {
            *h = f.get8_packet() as u8;
        }
        if &header != b"vorbis" {
            return None;
        }
        let codebook_count = f.get_bits(8) as i32 + 1;
        if f.valid_bits < 0 {
            return None;
        }
        for _ in 0..codebook_count {
            let c = self.read_codebook()?;
            self.codebooks.push(c);
        }
        let f = &mut self.s;
        // time domain transforms, unused
        let x = f.get_bits(6) + 1;
        for _ in 0..x {
            if f.get_bits(16) != 0 {
                return None;
            }
        }
        let floor_count = f.get_bits(6) as i32 + 1;
        if f.valid_bits < 0 {
            return None;
        }
        let mut longest_floorlist = 0;
        for _ in 0..floor_count {
            let floor_type = f.get_bits(16);
            // floor 0 is not supported by stb_vorbis
            if floor_type != 1 {
                return None;
            }
            let mut g = Floor1 { partitions: f.get_bits(5) as u8, ..Default::default() };
            let mut max_class = -1i32;
            for j in 0..g.partitions as usize {
                g.partition_class_list[j] = f.get_bits(4) as u8;
                max_class = max_class.max(g.partition_class_list[j] as i32);
            }
            for j in 0..(max_class + 1) as usize {
                g.class_dimensions[j] = f.get_bits(3) as u8 + 1;
                g.class_subclasses[j] = f.get_bits(2) as u8;
                if f.valid_bits < 0 {
                    return None;
                }
                if g.class_subclasses[j] != 0 {
                    g.class_masterbooks[j] = f.get_bits(8) as u8;
                    if g.class_masterbooks[j] as i32 >= codebook_count {
                        return None;
                    }
                }
                for k in 0..(1usize << g.class_subclasses[j]) {
                    g.subclass_books[j][k] = (f.get_bits(8) as i16).wrapping_sub(1);
                    if g.subclass_books[j][k] as i32 >= codebook_count {
                        return None;
                    }
                }
            }
            g.floor1_multiplier = f.get_bits(2) as u8 + 1;
            g.rangebits = f.get_bits(4) as u8;
            g.xlist[0] = 0;
            g.xlist[1] = 1 << g.rangebits;
            g.values = 2;
            for j in 0..g.partitions as usize {
                let c = g.partition_class_list[j] as usize;
                for _ in 0..g.class_dimensions[c] {
                    g.xlist[g.values as usize] = f.get_bits(g.rangebits as i32) as u16;
                    g.values += 1;
                }
            }
            let nv = g.values as usize;
            let mut p: Vec<(u16, u16)> = (0..nv).map(|j| (g.xlist[j], j as u16)).collect();
            // equal x are refused below, so the order of the sort is moot
            p.sort_by_key(|e| e.0);
            for j in 0..nv - 1 {
                if p[j].0 == p[j + 1].0 {
                    return None;
                }
            }
            for j in 0..nv {
                g.sorted_order[j] = p[j].1 as u8;
            }
            for j in 2..nv {
                let (mut low, mut hi) = (0usize, 0usize);
                let (mut lo_v, mut hi_v) = (-1i32, 65536i32);
                let xn = g.xlist[j] as i32;
                for i in 0..j {
                    let xi = g.xlist[i] as i32;
                    if xi > lo_v && xi < xn {
                        low = i;
                        lo_v = xi;
                    }
                    if xi < hi_v && xi > xn {
                        hi = i;
                        hi_v = xi;
                    }
                }
                g.neighbors[j] = [low as u8, hi as u8];
            }
            longest_floorlist = longest_floorlist.max(g.values);
            self.floor_config.push(g);
        }
        let residue_count = f.get_bits(6) as i32 + 1;
        if f.valid_bits < 0 {
            return None;
        }
        for i in 0..residue_count as usize {
            let mut r = Residue::default();
            self.residue_types[i] = f.get_bits(16) as u16;
            if self.residue_types[i] > 2 {
                return None;
            }
            r.begin = f.get_bits(24);
            r.end = f.get_bits(24);
            if r.end < r.begin {
                return None;
            }
            r.part_size = f.get_bits(24) + 1;
            r.classifications = f.get_bits(6) as u8 + 1;
            r.classbook = f.get_bits(8) as u8;
            if f.valid_bits < 0 || r.classbook as i32 >= codebook_count {
                return None;
            }
            let mut cascade = [0u8; 64];
            for cj in cascade.iter_mut().take(r.classifications as usize) {
                let low_bits = f.get_bits(3) as u8;
                let high_bits = if f.get_bits(1) != 0 { f.get_bits(5) as u8 } else { 0 };
                *cj = high_bits.wrapping_mul(8).wrapping_add(low_bits);
            }
            if f.valid_bits < 0 {
                return None;
            }
            r.residue_books = alloc(16 * r.classifications as u64, r.classifications as usize, [0i16; 8])?;
            for j in 0..r.classifications as usize {
                for k in 0..8 {
                    if cascade[j] & (1 << k) != 0 {
                        r.residue_books[j][k] = f.get_bits(8) as i16;
                        if f.valid_bits < 0 || r.residue_books[j][k] as i32 >= codebook_count {
                            return None;
                        }
                    } else {
                        r.residue_books[j][k] = -1;
                    }
                }
            }
            let cb = &self.codebooks[r.classbook as usize];
            let (centries, classwords) = (cb.entries as usize, cb.dimensions as usize);
            r.classdata = alloc(8 * centries as u64, centries * classwords, 0u8)?;
            r.classwords = classwords;
            for j in 0..centries {
                let mut temp = j;
                for k in (0..classwords).rev() {
                    r.classdata[j * classwords + k] = (temp % r.classifications as usize) as u8;
                    temp /= r.classifications as usize;
                }
            }
            self.residue_config.push(r);
        }
        let mapping_count = f.get_bits(6) as i32 + 1;
        if f.valid_bits < 0 {
            return None;
        }
        for _ in 0..mapping_count {
            let mut m = Mapping::default();
            if f.get_bits(16) != 0 {
                return None;
            }
            m.chan = vec![MappingChannel::default(); 1];
            m.submaps = if f.get_bits(1) != 0 { f.get_bits(4) as u8 + 1 } else { 1 };
            if f.get_bits(1) != 0 {
                // coupling steps: at least one, between two different channels
                // of the one there is (ilog(0) = 0 bits each)
                return None;
            }
            if f.get_bits(2) != 0 {
                return None;
            }
            if m.submaps > 1 {
                m.chan[0].mux = f.get_bits(4) as u8;
                if m.chan[0].mux >= m.submaps {
                    return None;
                }
            } else {
                m.chan[0].mux = 0;
            }
            for j in 0..m.submaps as usize {
                f.get_bits(8);
                m.submap_floor[j] = f.get_bits(8) as u8;
                m.submap_residue[j] = f.get_bits(8) as u8;
                if m.submap_floor[j] as i32 >= floor_count || m.submap_residue[j] as i32 >= residue_count {
                    return None;
                }
            }
            self.mapping.push(m);
        }
        self.mode_count = f.get_bits(6) as i32 + 1;
        for i in 0..self.mode_count as usize {
            let blockflag = f.get_bits(1) as u8;
            let windowtype = f.get_bits(16);
            let transformtype = f.get_bits(16);
            let mapping = f.get_bits(8) as u8;
            if f.valid_bits < 0 || windowtype != 0 || transformtype != 0 || mapping as i32 >= mapping_count {
                return None;
            }
            self.mode_config[i] = Mode { blockflag, mapping };
        }
        f.flush_packet();
        self.previous_length = 0;
        self.channel_buffer = vec![0f32; self.blocksize[1]];
        self.previous_window = vec![0f32; self.blocksize[1] / 2];
        self.final_y = vec![0i16; longest_floorlist as usize];
        self.blocks = [init_blocksize(self.blocksize[0]), init_blocksize(self.blocksize[1])];
        Some(())
    }

    fn decode_residue(&mut self, ch: usize, n: usize, rn: usize, do_not_decode: bool) {
        let f = &mut self.s;
        let r = &self.residue_config[rn];
        let rtype = self.residue_types[rn];
        let classwords = self.codebooks[r.classbook as usize].dimensions;
        let actual_size = if rtype == 2 { n as u32 * 2 } else { n as u32 };
        let limit_r_begin = r.begin.min(actual_size);
        let limit_r_end = r.end.min(actual_size);
        let n_read = (limit_r_end - limit_r_begin) as i32;
        let part_read = (n_read as u32 / r.part_size) as i32;
        // a mono stream: one channel in this submap, or none
        if ch == 0 {
            return;
        }
        let target = &mut self.channel_buffer;
        if !do_not_decode {
            for t in &mut target[..n] {
                *t = 0.0;
            }
        }
        if do_not_decode {
            // with one channel stb_vorbis takes its generic path whatever the
            // residue type, and that reads nothing for an undecoded channel
            return;
        }
        let mut part_classdata = vec![0usize; part_read.max(0) as usize + 1];
        for pass in 0..8 {
            let mut pcount = 0i32;
            let mut class_set = 0usize;
            while pcount < part_read {
                if pass == 0 && !do_not_decode {
                    let temp = f.decode(&self.codebooks[r.classbook as usize]);
                    if temp == EOP {
                        return;
                    }
                    part_classdata[class_set] = temp as usize;
                }
                let mut i = 0;
                while i < classwords && pcount < part_read {
                    if !do_not_decode {
                        let c = r.classdata[part_classdata[class_set] * r.classwords + i as usize] as usize;
                        let b = r.residue_books[c][pass];
                        if b >= 0 {
                            let offset = r.begin.wrapping_add((pcount as u32).wrapping_mul(r.part_size)) as i32;
                            let nn = r.part_size as i32;
                            let book = &self.codebooks[b as usize];
                            if !residue_decode(f, book, target, offset, nn, rtype) {
                                return;
                            }
                        }
                    }
                    i += 1;
                    pcount += 1;
                }
                class_set += 1;
            }
        }
    }

    fn do_floor(&mut self, map: usize, n: usize) {
        let n2 = (n >> 1) as i32;
        let m = &self.mapping[map];
        let s = m.chan[0].mux as usize;
        let g = &self.floor_config[m.submap_floor[s] as usize];
        let target = &mut self.channel_buffer;
        let fy = &self.final_y;
        let mult = g.floor1_multiplier as i32;
        let mut lx = 0i32;
        let mut ly = fy[0] as i32 * mult;
        for q in 1..g.values as usize {
            let j = g.sorted_order[q] as usize;
            if fy[j] >= 0 {
                let hy = fy[j] as i32 * mult;
                let hx = g.xlist[j] as i32;
                if lx != hx {
                    draw_line(target, lx, ly, hx, hy, n2);
                }
                lx = hx;
                ly = hy;
            }
        }
        if lx < n2 {
            for t in &mut target[lx as usize..n2 as usize] {
                *t *= INVERSE_DB_TABLE[(ly & 255) as usize];
            }
        }
    }

    /// vorbis_decode_initial: (left_start, left_end, right_start, right_end, mode)
    fn decode_initial(&mut self) -> Option<(i32, i32, i32, i32, usize)> {
        let f = &mut self.s;
        loop {
            if f.eof || !f.maybe_start_packet() {
                return None;
            }
            if f.get_bits(1) != 0 {
                while f.get8_packet() != EOP {}
                continue;
            }
            break;
        }
        let i = f.get_bits(ilog(self.mode_count - 1)) as i32;
        if i >= self.mode_count {
            return None;
        }
        let m = self.mode_config[i as usize];
        let (n, prev, next);
        if m.blockflag != 0 {
            n = self.blocksize[1] as i32;
            prev = f.get_bits(1);
            next = f.get_bits(1);
        } else {
            prev = 0;
            next = 0;
            n = self.blocksize[0] as i32;
        }
        let bs0 = self.blocksize[0] as i32;
        let window_center = n >> 1;
        let (ls, le) = if m.blockflag != 0 && prev == 0 { ((n - bs0) >> 2, (n + bs0) >> 2) } else { (0, window_center) };
        let (rs, re) = if m.blockflag != 0 && next == 0 { ((n * 3 - bs0) >> 2, (n * 3 + bs0) >> 2) } else { (window_center, n) };
        Some((ls, le, rs, re, i as usize))
    }

    /// vorbis_decode_packet_rest: Some((len, left_start))
    fn decode_packet_rest(&mut self, mode: usize, mut left_start: i32, right_start: i32, right_end: i32) -> Option<(i32, i32)> {
        let m = self.mode_config[mode];
        let n = self.blocksize[m.blockflag as usize];
        let map = m.mapping as usize;
        let n2 = n >> 1;
        let zero_channel;
        {
            let f = &mut self.s;
            let mp = &self.mapping[map];
            let s = mp.chan[0].mux as usize;
            let g = &self.floor_config[mp.submap_floor[s] as usize];
            if f.get_bits(1) != 0 {
                const RANGE_LIST: [i32; 4] = [256, 128, 86, 64];
                let range = RANGE_LIST[g.floor1_multiplier as usize - 1];
                let mut offset = 2usize;
                let fy = &mut self.final_y;
                fy[0] = f.get_bits(ilog(range) - 1) as i16;
                fy[1] = f.get_bits(ilog(range) - 1) as i16;
                for j in 0..g.partitions as usize {
                    let pclass = g.partition_class_list[j] as usize;
                    let cdim = g.class_dimensions[pclass];
                    let cbits = g.class_subclasses[pclass] as i32;
                    let csub = (1 << cbits) - 1;
                    let mut cval = 0i32;
                    if cbits != 0 {
                        cval = f.decode(&self.codebooks[g.class_masterbooks[pclass] as usize]);
                    }
                    for _ in 0..cdim {
                        let book = g.subclass_books[pclass][(cval & csub) as usize];
                        cval >>= cbits;
                        if book >= 0 {
                            fy[offset] = f.decode(&self.codebooks[book as usize]) as i16;
                        } else {
                            fy[offset] = 0;
                        }
                        offset += 1;
                    }
                }
                if f.valid_bits == EOP {
                    zero_channel = true;
                } else {
                    let mut step2_flag = [false; 256];
                    step2_flag[0] = true;
                    step2_flag[1] = true;
                    for j in 2..g.values as usize {
                        let [low, high] = g.neighbors[j];
                        let (low, high) = (low as usize, high as usize);
                        let pred = predict_point(
                            g.xlist[j] as i32,
                            g.xlist[low] as i32,
                            g.xlist[high] as i32,
                            fy[low] as i32,
                            fy[high] as i32,
                        );
                        let val = fy[j] as i32;
                        let highroom = range - pred;
                        let lowroom = pred;
                        let room = if highroom < lowroom { highroom * 2 } else { lowroom * 2 };
                        if val != 0 {
                            step2_flag[low] = true;
                            step2_flag[high] = true;
                            step2_flag[j] = true;
                            let y = if val >= room {
                                if highroom > lowroom {
                                    val - lowroom + pred
                                } else {
                                    pred - val + highroom - 1
                                }
                            } else if val & 1 != 0 {
                                pred - ((val + 1) >> 1)
                            } else {
                                pred + (val >> 1)
                            };
                            fy[j] = y as i16;
                        } else {
                            step2_flag[j] = false;
                            fy[j] = pred as i16;
                        }
                    }
                    for j in 0..g.values as usize {
                        if !step2_flag[j] {
                            fy[j] = -1;
                        }
                    }
                    zero_channel = false;
                }
            } else {
                zero_channel = true;
            }
        }
        // no coupling in a mono stream: zero_channel stays really_zero_channel
        for i in 0..self.mapping[map].submaps as usize {
            let ch = if self.mapping[map].chan[0].mux as usize == i { 1 } else { 0 };
            let r = self.mapping[map].submap_residue[i] as usize;
            self.decode_residue(ch, n2, r, zero_channel);
        }
        if zero_channel {
            for t in &mut self.channel_buffer[..n2] {
                *t = 0.0;
            }
        } else {
            self.do_floor(map, n);
        }
        let blk = &self.blocks[m.blockflag as usize];
        inverse_mdct(&mut self.channel_buffer, n, blk, &mut self.buf2);
        let f = &mut self.s;
        f.flush_packet();
        let (n, n2) = (n as i32, n2 as i32);
        if self.first_decode {
            self.current_loc = 0u32.wrapping_sub(n2 as u32);
            self.discard_samples_deferred = n - right_end;
            self.current_loc_valid = true;
            self.first_decode = false;
        } else if self.discard_samples_deferred != 0 {
            if self.discard_samples_deferred >= right_start - left_start {
                self.discard_samples_deferred -= right_start - left_start;
                left_start = right_start;
            } else {
                left_start += self.discard_samples_deferred;
                self.discard_samples_deferred = 0;
            }
        }
        if f.last_seg_which == f.end_seg_with_known_loc {
            if self.current_loc_valid && f.page_flag & 4 != 0 {
                let current_end = f.known_loc_for_packet;
                if current_end < self.current_loc.wrapping_add((right_end - left_start) as u32) {
                    let mut len = if current_end < self.current_loc { 0 } else { current_end.wrapping_sub(self.current_loc) as i32 };
                    len += left_start;
                    if len > right_end {
                        len = right_end;
                    }
                    self.current_loc = self.current_loc.wrapping_add(len as u32);
                    return Some((len, left_start));
                }
            }
            self.current_loc = f.known_loc_for_packet.wrapping_sub((n2 - left_start) as u32);
            self.current_loc_valid = true;
        }
        if self.current_loc_valid {
            self.current_loc = self.current_loc.wrapping_add((right_start - left_start) as u32);
        }
        Some((right_end, left_start))
    }

    /// vorbis_decode_packet: Some((len, left, right))
    fn decode_packet(&mut self) -> Option<(i32, i32, i32)> {
        let (left, _left_end, right, right_end, mode) = self.decode_initial()?;
        let (len, left) = self.decode_packet_rest(mode, left, right, right_end)?;
        Some((len, left, right))
    }

    /// vorbis_finish_frame
    fn finish_frame(&mut self, len: i32, left: i32, mut right: i32) -> i32 {
        if self.previous_length != 0 {
            let n = self.previous_length;
            let w = if n << 1 == self.blocksize[0] as i32 {
                &self.blocks[0].window
            } else if n << 1 == self.blocksize[1] as i32 {
                &self.blocks[1].window
            } else {
                return 0;
            };
            for j in 0..n as usize {
                let k = left as usize + j;
                self.channel_buffer[k] = self.channel_buffer[k] * w[j] + self.previous_window[j] * w[n as usize - 1 - j];
            }
        }
        let prev = self.previous_length;
        self.previous_length = len - right;
        let mut j = 0;
        while right + j < len {
            self.previous_window[j as usize] = self.channel_buffer[(right + j) as usize];
            j += 1;
        }
        if prev == 0 {
            return 0;
        }
        if len < right {
            right = len;
        }
        right - left
    }
}

fn residue_decode(f: &mut Stream, book: &Codebook, target: &mut [f32], mut offset: i32, n: i32, rtype: u16) -> bool {
    if rtype == 0 {
        let step = n / book.dimensions;
        for k in 0..step {
            if !f.codebook_decode_step(book, target, (offset + k) as usize, n - offset - k, step) {
                return false;
            }
        }
    } else {
        let mut k = 0;
        while k < n {
            if !f.codebook_decode(book, target, offset as usize, n - k) {
                return false;
            }
            k += book.dimensions;
            offset += book.dimensions;
        }
    }
    true
}

/// The FAST_SCALED_FLOAT_TO_INT conversion of copy_samples.
///
/// A damaged stream can decode to NaN (infinite codebook values from
/// float32_unpack, then inf - inf), and the conversion keeps the NaN's sign:
/// in libxmp that is whatever GCC's operand order makes of x86's default NaN
/// (negative) and the IMDCT's negations, sample by sample, and wasm leaves it
/// unspecified. The port takes every NaN as the default NaN, so that native and
/// wasm builds agree; on such streams it is not bit-exact with libxmp.
fn float_to_s16(x: f32) -> i16 {
    if x.is_nan() {
        return -32768;
    }
    const MAGIC: f32 = 1.5f32 * (1 << (23 - 15)) as f32 + 0.5f32 / (1 << 15) as f32;
    const ADDEND: u32 = ((150 - 15) << 23) + (1 << 22);
    let t = x + MAGIC;
    let v = t.to_bits().wrapping_sub(ADDEND) as i32;
    if (v as u32).wrapping_add(32768) > 65535 {
        if v < 0 {
            -32768
        } else {
            32767
        }
    } else {
        v as i16
    }
}

/// `libxmp_vorbis_decode_memory` for a mono stream: its samples, or None
/// where it fails (or the stream isn't mono, which libxmp's oggdec refuses).
pub fn decode_memory_mono(data: &[u8]) -> Option<Vec<i16>> {
    let mut v = Vorb::new(data);
    v.start_decoder()?;
    // vorbis_pump_first_frame
    if let Some((len, left, right)) = v.decode_packet() {
        v.finish_frame(len, left, right);
    }
    let mut out = Vec::new();
    loop {
        // libxmp_vorbis_get_frame_short -> get_frame_float -> copy_samples
        let Some((len, left, right)) = v.decode_packet() else { break };
        let n = v.finish_frame(len, left, right);
        if n == 0 {
            break;
        }
        if n < 0 {
            // libxmp steps its output position back over the samples it has
            if out.len() < n.unsigned_abs() as usize {
                return None;
            }
            out.truncate(out.len() - n.unsigned_abs() as usize);
            continue;
        }
        out.extend(v.channel_buffer[left as usize..(left + n) as usize].iter().map(|&x| float_to_s16(x)));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn float_to_s16_rounds_and_clamps() {
        use super::float_to_s16;
        assert_eq!(float_to_s16(0.0), 0);
        assert_eq!(float_to_s16(0.5), 16384);
        assert_eq!(float_to_s16(-0.5), -16384);
        assert_eq!(float_to_s16(1.5), 32767);
        assert_eq!(float_to_s16(-1.5), -32768);
        assert_eq!(float_to_s16(f32::NAN), -32768);
        assert_eq!(float_to_s16(-f32::NAN), -32768);
    }

    #[test]
    fn refuses_non_ogg() {
        assert!(super::decode_memory_mono(b"not an ogg stream at all, really").is_none());
    }
}
