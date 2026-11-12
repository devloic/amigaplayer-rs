//! Shared building blocks of the decrunchers ported from "ancient" (Teemu Suutari's
//! decompression library, BSD-2-Clause), by way of its C# port LibAncient in NostalgicPlayer
//! (Thomas Neumann, MIT): input and output streams, bit readers, Huffman and variable length
//! code decoders, the dynamic Huffman tree of LH, the LZW dictionary, delta decoding and the
//! bzip2 flavour of CRC-32. Where the C# port and the C++ original differ, the C++ behaviour is
//! kept (it is what the decrunchers are checked against). See LICENSE-ancient for both notices.
//!
//! Errors are `None`: every read is bounds checked, so malformed data ends the decrunch instead
//! of panicking. Shifts by 32 or more wrap the count as x86 does, which is what the C++ code
//! does in practice on the hosts it was checked on.

/// ancient's `getMaxRawSize`/`getMaxPackedSize` is 128 MiB; this crate is stricter, since no
/// Amiga module comes near it and the output is allocated up front from the header.
pub(crate) const MAX_RAW: usize = 32 << 20;

#[inline]
pub(crate) fn shl(v: u32, n: u32) -> u32 {
    v.wrapping_shl(n)
}

#[inline]
pub(crate) fn shr(v: u32, n: u32) -> u32 {
    v.wrapping_shr(n)
}

/// `(1 << n) - 1` with the shift count wrapped like x86.
#[inline]
fn mask(n: u32) -> u32 {
    shl(1, n).wrapping_sub(1)
}

pub(crate) fn be16(d: &[u8], o: usize) -> Option<u32> {
    let s = d.get(o..o.checked_add(2)?)?;
    Some(u32::from(s[0]) << 8 | u32::from(s[1]))
}

pub(crate) fn be32(d: &[u8], o: usize) -> Option<u32> {
    let s = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// A source of bytes for the bit readers.
pub(crate) trait ByteSource {
    fn byte(&mut self) -> Option<u8>;
    fn be16(&mut self) -> Option<u32>;
    fn be32(&mut self) -> Option<u32>;
}

/// ancient's `ForwardInputStream`.
pub(crate) struct Fwd<'a> {
    buf: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Fwd<'a> {
    pub(crate) fn new(buf: &'a [u8], start: usize, end: usize) -> Option<Self> {
        if start > end || end > buf.len() {
            return None;
        }
        Some(Fwd { buf, pos: start, end })
    }
}

impl ByteSource for Fwd<'_> {
    fn byte(&mut self) -> Option<u8> {
        if self.pos >= self.end {
            return None;
        }
        self.pos += 1;
        Some(self.buf[self.pos - 1])
    }
    fn be16(&mut self) -> Option<u32> {
        let b0 = u32::from(self.byte()?);
        let b1 = u32::from(self.byte()?);
        Some(b0 << 8 | b1)
    }
    fn be32(&mut self) -> Option<u32> {
        Some(self.be16()? << 16 | self.be16()?)
    }
}

/// ancient's `BackwardInputStream`: reads `buf[pos - 1]` downwards to `end`.
pub(crate) struct Bwd<'a> {
    buf: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Bwd<'a> {
    /// Reads `buf[start..end]` from its end.
    pub(crate) fn new(buf: &'a [u8], start: usize, end: usize) -> Option<Self> {
        if end < start || end > buf.len() {
            return None;
        }
        Some(Bwd { buf, pos: end, end: start })
    }
}

impl ByteSource for Bwd<'_> {
    fn byte(&mut self) -> Option<u8> {
        if self.pos <= self.end {
            return None;
        }
        self.pos -= 1;
        Some(self.buf[self.pos])
    }
    fn be16(&mut self) -> Option<u32> {
        let b0 = u32::from(self.byte()?);
        let b1 = u32::from(self.byte()?);
        Some(b1 << 8 | b0)
    }
    fn be32(&mut self) -> Option<u32> {
        let lo = self.be16()?;
        let hi = self.be16()?;
        Some(hi << 16 | lo)
    }
}

/// A forward and a backward stream over the same bytes, linked as ancient's `link()` does:
/// each one's reads move the other's end, so the two can never cross.
pub(crate) struct Linked<'a> {
    buf: &'a [u8],
    fpos: usize,
    fend: usize,
    bpos: usize,
    bend: usize,
}

impl<'a> Linked<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Linked { buf, fpos: 0, fend: buf.len(), bpos: buf.len(), bend: 0 }
    }
    pub(crate) fn fwd(&mut self) -> LinkedFwd<'_, 'a> {
        LinkedFwd(self)
    }
    pub(crate) fn back_byte(&mut self) -> Option<u8> {
        if self.bpos <= self.bend {
            return None;
        }
        self.bpos -= 1;
        self.fend = self.bpos;
        Some(self.buf[self.bpos])
    }
}

pub(crate) struct LinkedFwd<'b, 'a>(&'b mut Linked<'a>);

impl ByteSource for LinkedFwd<'_, '_> {
    fn byte(&mut self) -> Option<u8> {
        let s = &mut *self.0;
        if s.fpos >= s.fend {
            return None;
        }
        s.fpos += 1;
        s.bend = s.fpos;
        Some(s.buf[s.fpos - 1])
    }
    fn be16(&mut self) -> Option<u32> {
        let b0 = u32::from(self.byte()?);
        let b1 = u32::from(self.byte()?);
        Some(b0 << 8 | b1)
    }
    fn be32(&mut self) -> Option<u32> {
        Some(self.be16()? << 16 | self.be16()?)
    }
}

/// ancient's `MSBBitReader`.
#[derive(Default)]
pub(crate) struct Msb {
    content: u32,
    len: u32,
}

impl Msb {
    pub(crate) fn reset(&mut self, content: u32, len: u8) {
        self.content = content;
        self.len = u32::from(len);
    }
    fn generic(&mut self, mut count: u32, mut word: impl FnMut() -> Option<(u32, u32)>) -> Option<u32> {
        if count > 32 {
            return None;
        }
        let mut ret = 0u32;
        while count != 0 {
            if self.len == 0 {
                (self.content, self.len) = word()?;
            }
            let m = (count & 0xff).min(self.len);
            self.len -= m;
            ret = shl(ret, m) | (shr(self.content, self.len) & mask(m));
            count -= m;
            if m == 0 {
                // A zero-length word (count wrapped at 256 bits): the C++ loop never ends.
                return None;
            }
        }
        Some(ret)
    }
    pub(crate) fn bits8(&mut self, s: &mut impl ByteSource, n: u32) -> Option<u32> {
        self.generic(n, || Some((u32::from(s.byte()?), 8)))
    }
    pub(crate) fn bits16(&mut self, s: &mut impl ByteSource, n: u32) -> Option<u32> {
        self.generic(n, || Some((s.be16()?, 16)))
    }
    pub(crate) fn bits32(&mut self, s: &mut impl ByteSource, n: u32) -> Option<u32> {
        self.generic(n, || Some((s.be32()?, 32)))
    }
}

/// ancient's `LSBBitReader`.
#[derive(Default)]
pub(crate) struct Lsb {
    content: u32,
    len: u32,
}

impl Lsb {
    /// `len` is the C++ `uint8_t`, so it wraps at 256.
    pub(crate) fn reset(&mut self, content: u32, len: u8) {
        self.content = content;
        self.len = u32::from(len);
    }
    fn generic(&mut self, mut count: u32, mut word: impl FnMut() -> Option<(u32, u32)>) -> Option<u32> {
        if count > 32 {
            return None;
        }
        let (mut ret, mut pos) = (0u32, 0u32);
        while count != 0 {
            if self.len == 0 {
                (self.content, self.len) = word()?;
            }
            let m = (count & 0xff).min(self.len);
            ret |= shl(self.content & mask(m), pos);
            self.content = shr(self.content, m);
            self.len -= m;
            count -= m;
            pos += m;
        }
        Some(ret)
    }
    pub(crate) fn bits8(&mut self, s: &mut impl ByteSource, n: u32) -> Option<u32> {
        self.generic(n, || Some((u32::from(s.byte()?), 8)))
    }
    pub(crate) fn bits32(&mut self, s: &mut impl ByteSource, n: u32) -> Option<u32> {
        self.generic(n, || Some((s.be32()?, 32)))
    }
}

/// ancient's `ForwardOutputStream` over `buf[start..end]`. Copies may reach back before `start`
/// (to the previous XPK chunks) only through [`FwdOut::copy_prev`].
pub(crate) struct FwdOut<'a> {
    buf: &'a mut [u8],
    start: usize,
    pos: usize,
    end: usize,
}

impl<'a> FwdOut<'a> {
    pub(crate) fn new(buf: &'a mut [u8], start: usize, end: usize) -> Option<Self> {
        if start > end || end > buf.len() {
            return None;
        }
        Some(FwdOut { buf, start, pos: start, end })
    }
    pub(crate) fn eof(&self) -> bool {
        self.pos == self.end
    }
    /// Bytes written so far.
    pub(crate) fn written(&self) -> usize {
        self.pos - self.start
    }
    pub(crate) fn write(&mut self, v: u8) -> Option<()> {
        if self.pos >= self.end {
            return None;
        }
        self.buf[self.pos] = v;
        self.pos += 1;
        Some(())
    }
    pub(crate) fn copy(&mut self, distance: usize, count: usize) -> Option<()> {
        self.copy_from(distance, count, self.start)
    }
    /// The copy of ancient's `copy(distance, count, previousData)`, where the previous data is
    /// everything in the buffer before `start`.
    pub(crate) fn copy_prev(&mut self, distance: usize, count: usize) -> Option<()> {
        self.copy_from(distance, count, 0)
    }
    fn copy_from(&mut self, distance: usize, count: usize, floor: usize) -> Option<()> {
        if self.pos.checked_add(count)? > self.end {
            return None;
        }
        if distance == 0 || floor.checked_add(distance)? > self.pos {
            return None;
        }
        for _ in 0..count {
            self.buf[self.pos] = self.buf[self.pos - distance];
            self.pos += 1;
        }
        Some(())
    }
}

/// ancient's `BackwardOutputStream`: fills `buf[start..end]` from its end.
pub(crate) struct BwdOut<'a> {
    buf: &'a mut [u8],
    start: usize,
    pos: usize,
    end: usize,
}

impl<'a> BwdOut<'a> {
    pub(crate) fn new(buf: &'a mut [u8], start: usize, end: usize) -> Option<Self> {
        if start > end || end > buf.len() {
            return None;
        }
        Some(BwdOut { buf, start, pos: end, end })
    }
    pub(crate) fn eof(&self) -> bool {
        self.pos == self.start
    }
    pub(crate) fn write(&mut self, v: u8) -> Option<()> {
        if self.pos <= self.start {
            return None;
        }
        self.pos -= 1;
        self.buf[self.pos] = v;
        Some(())
    }
    pub(crate) fn copy(&mut self, distance: usize, count: usize) -> Option<()> {
        if distance == 0
            || self.start.checked_add(count)? > self.pos
            || self.pos.checked_add(distance)? > self.end
        {
            return None;
        }
        for _ in 0..count {
            self.buf[self.pos - 1] = self.buf[self.pos + distance - 1];
            self.pos -= 1;
        }
        Some(())
    }
}

#[derive(Clone, Copy)]
struct Node {
    left: u32,
    right: u32,
    value: u32,
}

/// ancient's `HuffmanDecoder`, a binary tree in an array (values are stored as `u32`).
#[derive(Default)]
pub(crate) struct Huffman {
    table: Vec<Node>,
}

impl Huffman {
    pub(crate) fn with(codes: &[(u32, u32, u32)]) -> Option<Self> {
        let mut h = Huffman::default();
        for &(len, code, value) in codes {
            h.insert(len, code, value)?;
        }
        Some(h)
    }

    pub(crate) fn decode(&self, mut bit: impl FnMut() -> Option<u32>) -> Option<u32> {
        if self.table.is_empty() {
            return None;
        }
        let mut i = 0usize;
        while self.table[i].left != 0 || self.table[i].right != 0 {
            i = if bit()? != 0 { self.table[i].right } else { self.table[i].left } as usize;
            if i == 0 {
                return None;
            }
        }
        Some(self.table[i].value)
    }

    pub(crate) fn insert(&mut self, length: u32, code: u32, value: u32) -> Option<()> {
        let mut i = 0usize;
        let mut len = self.table.len();
        for current_bit in (0..=length).rev() {
            let code_bit = current_bit != 0 && (shr(code, current_bit - 1) & 1) != 0;
            if i != len {
                let n = self.table[i];
                if current_bit == 0 || (n.left == 0 && n.right == 0) {
                    return None;
                }
                let slot = if code_bit { n.right } else { n.left };
                if slot == 0 {
                    let parent = i;
                    i = len;
                    if code_bit {
                        self.table[parent].right = i as u32;
                    } else {
                        self.table[parent].left = i as u32;
                    }
                } else {
                    i = slot as usize;
                }
            } else {
                let next = (len + 1) as u32;
                self.table.push(Node {
                    left: if current_bit != 0 && !code_bit { next } else { 0 },
                    right: if current_bit != 0 && code_bit { next } else { 0 },
                    value: if current_bit != 0 { 0 } else { value },
                });
                len += 1;
                i += 1;
            }
        }
        Some(())
    }

    /// ancient's `createOrderlyHuffmanTable`: canonical codes from bit lengths, symbols of
    /// equal length in index order.
    pub(crate) fn orderly(bit_lengths: &[u8]) -> Option<Self> {
        let n = bit_lengths.len();
        let (mut min_depth, mut max_depth) = (32u32, 0u32);
        let mut first = [usize::MAX; 33];
        let mut last = [0usize; 33];
        let mut next = vec![0usize; n];
        for (i, &l) in bit_lengths.iter().enumerate() {
            let l = u32::from(l);
            if l > 32 {
                return None;
            }
            if l != 0 {
                min_depth = min_depth.min(l);
                max_depth = max_depth.max(l);
                let l = l as usize;
                if first[l] == usize::MAX {
                    first[l] = i;
                } else {
                    next[last[l]] = i;
                }
                last[l] = i;
            }
        }
        if max_depth == 0 {
            return None;
        }
        let mut h = Huffman::default();
        let mut code = 0u32;
        for depth in min_depth..=max_depth {
            let d = depth as usize;
            if first[d] != usize::MAX {
                next[last[d]] = n;
            }
            let mut i = first[d];
            while i < n {
                h.insert(depth, shr(code, max_depth - depth), i as u32)?;
                code = code.wrapping_add(shl(1, max_depth - depth));
                i = next[i];
            }
        }
        Some(h)
    }
}

/// ancient's `VariableLengthCodeDecoder`: a negative entry restarts the running offset.
pub(crate) struct Vlc {
    bits: Vec<u32>,
    offsets: Vec<u32>,
}

impl Vlc {
    pub(crate) fn new(args: &[i32]) -> Self {
        let mut offsets = Vec::with_capacity(args.len());
        let mut length = 0u32;
        for &v in args {
            if v < 0 {
                offsets.push(0);
                length = shl(1, v.unsigned_abs());
            } else {
                offsets.push(length);
                length = length.wrapping_add(shl(1, v as u32));
            }
        }
        Vlc { bits: args.iter().map(|v| v.unsigned_abs()).collect(), offsets }
    }

    pub(crate) fn decode(&self, base: u32, read: impl FnOnce(u32) -> Option<u32>) -> Option<u32> {
        let b = base as usize;
        if b >= self.offsets.len() {
            return None;
        }
        Some(self.offsets[b].wrapping_add(read(self.bits[b])?))
    }
}

#[derive(Clone, Copy, Default)]
struct DynNode {
    freq: u32,
    index: u32,
    parent: u32,
    left: u32,
    right: u32,
}

/// ancient's `DynamicHuffmanDecoder` (the adaptive Huffman tree of LH / LHLB).
pub(crate) struct DynHuffman {
    max: u32,
    count: u32,
    nodes: Vec<DynNode>,
    map: Vec<u32>,
}

impl DynHuffman {
    pub(crate) fn new(max: u32) -> Self {
        let n = (max * 2 - 1) as usize;
        let count = max;
        let mut nodes = vec![DynNode::default(); n];
        let mut map = vec![0u32; n];
        for i in 0..count {
            nodes[i as usize] = DynNode {
                freq: 1,
                index: i + (max - count) * 2,
                parent: max * 2 - count + (i >> 1),
                left: 0,
                right: 0,
            };
            map[(i + (max - count) * 2) as usize] = i;
        }
        let mut j = 0u32;
        for i in max * 2 - count..max * 2 - 1 {
            let l = if j >= count { j + (max - count) * 2 } else { j };
            let r = if j + 1 >= count { j + 1 + (max - count) * 2 } else { j + 1 };
            nodes[i as usize] = DynNode {
                freq: nodes[l as usize].freq + nodes[r as usize].freq,
                index: i,
                parent: max + (i >> 1),
                left: l,
                right: r,
            };
            map[i as usize] = i;
            j += 2;
        }
        DynHuffman { max, count, nodes, map }
    }

    pub(crate) fn decode(&self, mut bit: impl FnMut() -> Option<u32>) -> Option<u32> {
        if self.count == 0 {
            return None;
        }
        if self.count == 1 {
            return Some(0);
        }
        let mut code = self.max * 2 - 2;
        while code >= self.max {
            let n = &self.nodes[code as usize];
            code = if bit()? != 0 { n.right } else { n.left };
        }
        Some(code)
    }

    pub(crate) fn update(&mut self, mut code: u32) -> Option<()> {
        if code >= self.count {
            return None;
        }
        if self.count == 1 {
            self.nodes[0].freq = 1;
            return Some(());
        }
        let root = self.max * 2 - 2;
        while code != root {
            let c = code as usize;
            self.nodes[c].freq += 1;
            let index = self.nodes[c].index;
            let mut dest = index;
            let freq = self.nodes[c].freq;
            while dest != root && freq > self.nodes[self.map[dest as usize + 1] as usize].freq {
                dest += 1;
            }
            if index != dest {
                let d = self.map[dest as usize] as usize;
                let t = self.nodes[c].index;
                self.nodes[c].index = self.nodes[d].index;
                self.nodes[d].index = t;
                self.map.swap(index as usize, dest as usize);
                // Swap the parents' links to the two nodes (the C++ swaps two references,
                // which may be the two leaves of one parent), then the parents themselves.
                let pc = self.nodes[c].parent as usize;
                let pd = self.nodes[d].parent as usize;
                let c_left = self.nodes[pc].left == c as u32;
                let d_left = self.nodes[pd].left == d as u32;
                let vc = if c_left { self.nodes[pc].left } else { self.nodes[pc].right };
                let vd = if d_left { self.nodes[pd].left } else { self.nodes[pd].right };
                if c_left {
                    self.nodes[pc].left = vd;
                } else {
                    self.nodes[pc].right = vd;
                }
                if d_left {
                    self.nodes[pd].left = vc;
                } else {
                    self.nodes[pd].right = vc;
                }
                let t = self.nodes[c].parent;
                self.nodes[c].parent = self.nodes[d].parent;
                self.nodes[d].parent = t;
            }
            code = self.nodes[c].parent;
        }
        self.nodes[code as usize].freq += 1;
        Some(())
    }

    pub(crate) fn max_freq(&self) -> u32 {
        self.nodes[(self.max * 2 - 2) as usize].freq
    }
}

/// ancient's `LZWDecoder`.
pub(crate) struct Lzw {
    max_code: u32,
    literals: u32,
    stack_len: usize,
    free: u32,
    prev: u32,
    new_code: u32,
    prefix: Vec<u32>,
    suffix: Vec<u8>,
    stack: Vec<u8>,
}

impl Lzw {
    pub(crate) fn new(max_code: u32, literals: u32, stack_len: u32, first: u32) -> Self {
        let n = max_code.saturating_sub(literals) as usize;
        Lzw {
            max_code,
            literals,
            stack_len: stack_len as usize,
            free: literals,
            prev: first,
            new_code: 0,
            prefix: vec![0; n],
            suffix: vec![0; n],
            stack: vec![0; stack_len as usize],
        }
    }
    pub(crate) fn reset(&mut self, first: u32) {
        self.free = self.literals;
        self.prev = first;
    }
    pub(crate) fn add(&mut self, code: u32) {
        if self.free < self.max_code {
            let k = (self.free - self.literals) as usize;
            self.suffix[k] = self.new_code as u8;
            self.prefix[k] = self.prev;
            self.free += 1;
        }
        self.prev = code;
    }
    fn suffix_of(&self, v: u32) -> Option<u32> {
        if v >= self.free {
            return None;
        }
        Some(if v < self.literals { v } else { u32::from(self.suffix[(v - self.literals) as usize]) })
    }
    pub(crate) fn write(
        &mut self,
        mut code: u32,
        add_new: bool,
        mut out: impl FnMut(u8) -> Option<()>,
    ) -> Option<()> {
        let mut sp = 0usize;
        let tmp = self.new_code;
        if add_new {
            code = self.prev;
        }
        self.new_code = self.suffix_of(code)?;
        while code >= self.literals {
            if sp + 1 >= self.stack_len {
                return None;
            }
            self.stack[sp] = self.new_code as u8;
            sp += 1;
            code = self.prefix[(code - self.literals) as usize];
            self.new_code = self.suffix_of(code)?;
        }
        *self.stack.get_mut(sp)? = self.new_code as u8;
        sp += 1;
        while sp != 0 {
            sp -= 1;
            out(self.stack[sp])?;
        }
        if add_new {
            out(tmp as u8)?;
        }
        Some(())
    }
    pub(crate) fn is_literal(&self, code: u32) -> bool {
        code < self.free
    }
}

/// ancient's `DLTADecode`: a running byte sum.
pub(crate) fn delta_decode(d: &mut [u8]) {
    let mut ctr = 0u8;
    for b in d {
        ctr = ctr.wrapping_add(*b);
        *b = ctr;
    }
}

const fn crc32_rev_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04c1_1db7 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC32_REV: [u32; 256] = crc32_rev_table();

/// ancient's `CRC32Rev` (the MSB-first CRC-32 of bzip2); `None` for an empty range, which the
/// C++ code rejects as out of bounds.
pub(crate) fn crc32_rev(d: &[u8], acc: u32) -> Option<u32> {
    if d.is_empty() {
        return None;
    }
    let mut a = !acc;
    for &b in d {
        a = (a << 8) ^ CRC32_REV[((a >> 24) ^ u32::from(b)) as usize];
    }
    Some(!a)
}
