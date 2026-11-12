//! Amiga LZX archives (`.lzx`), read in memory.
//!
//! # Origin and licence
//!
//! The Amiga LZX format was never published by its authors (Jonathan Forbes and Tomi
//! Poutanen, 1995; the Microsoft LZX of CAB and CHM files is a later cousin that differs in
//! its details). Its layout is known from `unlzx.c`, "LZX Extract in (supposedly) portable C"
//! by Dr Mek (with thanks to Dan Fraser in its comments; created 1998; pipe support added by
//! Erik Meusel, 2001), which is distributed under the Unlicense in
//! <https://github.com/nhoudelot/unlzx> (the file itself carries no licence notice).
//! What this module takes from it is the format: the 10-byte info header, the 31-byte header
//! of each file with its CRC, the merged-file groups, the block layout (three-bit block type,
//! the offset tree of type 3, the 24-bit block length, the literal tree sent as deltas through
//! a 20-symbol tree), the canonical Huffman codes, the offset and length slots, and the 16-bit
//! big-endian words the bits are read from, least significant bit first. These are facts of
//! the format, and the slot tables are computed here from their rule (they are listed as data
//! in `unlzx.c`). The code is written from that understanding and is not a translation of
//! the C: a bit reader on a 64-bit accumulator that reads on demand, a table-and-canonical
//! Huffman decoder, a decoder that yields exactly as many bytes as a caller asks for (so a
//! file is a request and a merged group one stream), a 64 KiB window ring, and errors instead
//! of the C's buffer overruns. `LICENSE-unlzx` at the repository root holds the Unlicense text.
//!
//! # Format
//!
//! `"LZX"`, a flags byte (1 damage protection, 2 locked), six more bytes; then per file a
//! 31-byte header (attributes at 0, unpacked size at 2 and packed size at 6, little-endian;
//! machine type 10, pack mode 11 (0 stored, 2 crunched), flags 12, comment length 14, date at
//! 18 (big-endian, bit fields), data CRC-32 at 22 and header CRC-32 at 26 (little-endian), name
//! length 30), the name, the comment, then the packed data. A file whose packed size is 0 is
//! *merged* with the following files: it is crunched in the stream of the first following file
//! that has packed data, in the order of the headers, one stream with one history.

use super::{clean_name, text, ArchiveError, Out};

type R<T> = Result<T, &'static str>;

/// The size of the history window.
const WINDOW: usize = 1 << 16;
/// The longest code the format has.
const MAX_BITS: usize = 16;
/// Codes up to this many bits are decoded through a table.
const FAST_BITS: usize = 10;

// ---- CRC-32 (the usual one: polynomial 0xEDB88320, as zlib's `crc32(prev, data)`) ----------

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

fn crc32(prev: u32, data: &[u8]) -> u32 {
    let mut c = !prev;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

// ---- the file headers ---------------------------------------------------------------------

const INFO_LEN: usize = 10;
const HEADER_LEN: usize = 31;

/// A file's header. The date and attributes are parsed for completeness (the archive API has
/// no field for them).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    /// Protection bits as the archive lists them (bit 0 read, 1 write, 2 delete, 3 execute,
    /// 4 archive, 5 hold, 6 script, 7 pure).
    pub attributes: u8,
    pub unpacked: u32,
    pub packed: u32,
    pub machine: u8,
    pub mode: u8,
    pub flags: u8,
    /// `(year, month 1..=12, day, hour, minute, second)`; the month and day are as stored
    /// (a damaged field can be out of range).
    pub date: (u16, u8, u8, u8, u8, u8),
    pub crc: u32,
    pub name: Vec<u8>,
    pub comment: Vec<u8>,
    /// The bytes the header takes, with its name and comment.
    pub len: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum HeaderError {
    Truncated,
    Crc,
}

fn le32(d: &[u8]) -> u32 {
    u32::from_le_bytes([d[0], d[1], d[2], d[3]])
}

/// The header at `d[at..]`.
fn parse_header(d: &[u8], at: usize) -> Result<Header, HeaderError> {
    let h = d.get(at..at.saturating_add(HEADER_LEN)).ok_or(HeaderError::Truncated)?;
    let (nlen, clen) = (h[30] as usize, h[14] as usize);
    let rest = d.get(at + HEADER_LEN..at + HEADER_LEN + nlen + clen).ok_or(HeaderError::Truncated)?;
    let (name, comment) = rest.split_at(nlen);
    // the header CRC covers the header with its own field zeroed, then the name, the comment
    let mut zeroed = [0u8; HEADER_LEN];
    zeroed.copy_from_slice(h);
    zeroed[26..30].fill(0);
    let sum = crc32(crc32(crc32(0, &zeroed), name), comment);
    if sum != le32(&h[26..30]) {
        return Err(HeaderError::Crc);
    }
    let packed_date = u32::from_be_bytes([h[18], h[19], h[20], h[21]]);
    Ok(Header {
        attributes: h[0],
        unpacked: le32(&h[2..6]),
        packed: le32(&h[6..10]),
        machine: h[10],
        mode: h[11],
        flags: h[12],
        date: (
            1970 + ((packed_date >> 17) & 63) as u16,
            1 + ((packed_date >> 23) & 15) as u8,
            (packed_date >> 27) as u8,
            ((packed_date >> 12) & 31) as u8,
            ((packed_date >> 6) & 63) as u8,
            (packed_date & 63) as u8,
        ),
        crc: le32(&h[22..26]),
        name: name.to_vec(),
        comment: comment.to_vec(),
        len: HEADER_LEN + nlen + clen,
    })
}

/// Whether `d` starts like an LZX archive: the info header, and, when the first file's header
/// is all there, a header CRC that matches. (A file cut inside its first header still counts:
/// [`extract`] then says it is truncated.)
pub(crate) fn is_lzx(d: &[u8]) -> bool {
    if d.len() < INFO_LEN || &d[..3] != b"LZX" || d[3] & !3 != 0 {
        return false;
    }
    match parse_header(d, INFO_LEN) {
        Ok(_) => true,
        Err(HeaderError::Truncated) => true,
        Err(HeaderError::Crc) => false,
    }
}

// ---- bits ---------------------------------------------------------------------------------

/// The packed data as a stream of bits: 16-bit big-endian words, each read from its least
/// significant bit. Reads past the end give zero bits and are an error once consumed.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    /// Bits in `acc`.
    n: u32,
    /// How many of them are padding past the end of `data`.
    pad: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0, acc: 0, n: 0, pad: 0 }
    }

    fn fill(&mut self) {
        while self.n <= 48 {
            let word = match self.data.get(self.pos..) {
                Some([a, b, ..]) => u16::from_be_bytes([*a, *b]),
                Some([a]) => {
                    self.pad += 8;
                    (*a as u16) << 8
                }
                _ => {
                    self.pad += 16;
                    0
                }
            };
            self.acc |= (word as u64) << self.n;
            self.n += 16;
            self.pos += 2;
        }
    }

    /// The next `k` (at most 16) bits, not consumed.
    fn peek(&mut self, k: u32) -> u32 {
        self.fill();
        (self.acc & ((1u64 << k) - 1)) as u32
    }

    fn eat(&mut self, k: u32) -> R<()> {
        self.acc >>= k;
        self.n -= k;
        if self.n < self.pad {
            return Err("ran out of data");
        }
        Ok(())
    }

    fn get(&mut self, k: u32) -> R<u32> {
        let v = self.peek(k);
        self.eat(k)?;
        Ok(v)
    }
}

// ---- Huffman codes ------------------------------------------------------------------------

/// A canonical Huffman code (shorter codes first, then by symbol; the first bit read is the
/// code's most significant bit), complete.
struct Huffman {
    /// For codes of up to `FAST_BITS`: `symbol << 5 | length` at the index of the next
    /// `FAST_BITS` bits of the stream, 0 for a longer code.
    fast: Vec<u16>,
    /// How many codes have each length.
    count: [u16; MAX_BITS + 1],
    /// The symbols by (length, value).
    sorted: Vec<u16>,
}

impl Huffman {
    /// The code for these code lengths (0 = unused); `None` unless it is complete (the
    /// lengths use all the code space exactly).
    fn new(lens: &[u8]) -> Option<Huffman> {
        let mut count = [0u16; MAX_BITS + 1];
        for &l in lens {
            *count.get_mut(l as usize)? += 1;
        }
        count[0] = 0;
        let space: u32 = (1..=MAX_BITS).map(|l| (count[l] as u32) << (MAX_BITS - l)).sum();
        if space != 1 << MAX_BITS {
            return None;
        }
        let mut start = [0u16; MAX_BITS + 2];
        for l in 1..=MAX_BITS {
            start[l + 1] = start[l] + count[l];
        }
        let mut sorted = vec![0u16; start[MAX_BITS + 1] as usize];
        let mut next_code = [0u32; MAX_BITS + 1];
        let mut code = 0u32;
        for l in 1..=MAX_BITS {
            code = (code + count[l - 1] as u32) << 1;
            next_code[l] = code;
        }
        let mut fast = vec![0u16; 1 << FAST_BITS];
        let mut put = start;
        for (sym, &l) in lens.iter().enumerate() {
            let l = l as usize;
            if l == 0 {
                continue;
            }
            sorted[put[l] as usize] = sym as u16;
            put[l] += 1;
            let c = next_code[l];
            next_code[l] += 1;
            if l <= FAST_BITS {
                // the stream reads the code's top bit first, so index by the reversed code
                let rev = (c.reverse_bits() >> (32 - l)) as usize;
                for i in (rev..1 << FAST_BITS).step_by(1 << l) {
                    fast[i] = (sym as u16) << 5 | l as u16;
                }
            }
        }
        Some(Huffman { fast, count, sorted })
    }

    fn decode(&self, bits: &mut Bits) -> R<u16> {
        let w = bits.peek(MAX_BITS as u32);
        let e = self.fast[w as usize & ((1 << FAST_BITS) - 1)];
        if e != 0 {
            bits.eat((e & 31) as u32)?;
            return Ok(e >> 5);
        }
        let (mut code, mut first, mut index) = (0u32, 0u32, 0u32);
        for l in 1..=MAX_BITS {
            code |= (w >> (l - 1)) & 1;
            let n = self.count[l] as u32;
            if code < first + n {
                bits.eat(l as u32)?;
                return Ok(self.sorted[(index + code - first) as usize]);
            }
            index += n;
            first = (first + n) << 1;
            code <<= 1;
        }
        Err("bad Huffman code")
    }
}

// ---- the decruncher -----------------------------------------------------------------------

/// Extra bits of the offset and length slots (the same rule for both): slots 0 to 3 have
/// none, slot `i` has `i / 2 - 1`.
fn slot_bits(slot: usize) -> u32 {
    if slot < 4 { 0 } else { (slot / 2 - 1) as u32 }
}

/// The smallest value of a slot: 0, 1, 2, 3, 4, 6, 8, 12, 16, 24, ... (`(2 | i & 1) << extra`).
fn slot_base(slot: usize) -> usize {
    if slot < 4 { slot } else { (2 | (slot & 1)) << slot_bits(slot) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockType {
    /// Offsets are slots with raw extra bits.
    Verbatim,
    /// Offsets of 3 or more extra bits send their upper bits raw and the low three through an
    /// eight-symbol tree.
    Aligned,
}

/// The decrunching state of one stream (a file, or a merged group): the bit reader, the
/// block being decoded, the code lengths that persist from block to block, and the window.
struct Decoder<'a> {
    bits: Bits<'a>,
    window: Vec<u8>,
    /// Bytes produced so far (the window holds the last `WINDOW` of them).
    produced: usize,
    /// Symbols left in the current block (as bytes of output).
    block_left: usize,
    block_type: BlockType,
    /// The match being copied: its length left and its distance.
    pending: usize,
    distance: usize,
    last_distance: usize,
    /// The literal / match code lengths, kept for the delta coding of the next block.
    lens: [u8; 768],
    literals: Option<Huffman>,
    offsets: Option<Huffman>,
}

impl<'a> Decoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        Decoder {
            bits: Bits::new(data),
            window: vec![0; WINDOW],
            produced: 0,
            block_left: 0,
            block_type: BlockType::Verbatim,
            pending: 0,
            distance: 0,
            last_distance: 1,
            lens: [0; 768],
            literals: None,
            offsets: None,
        }
    }

    /// Reads a block header and the code tables that go with it.
    fn block_header(&mut self) -> R<()> {
        let b = &mut self.bits;
        self.block_type = match b.get(3)? {
            2 => BlockType::Verbatim,
            3 => BlockType::Aligned,
            _ => return Err("unknown block type"),
        };
        if self.block_type == BlockType::Aligned {
            let mut l = [0u8; 8];
            for v in &mut l {
                *v = b.get(3)? as u8;
            }
            self.offsets = Some(Huffman::new(&l).ok_or("bad offset code")?);
        }
        let hi = b.get(8)? as usize;
        let mid = b.get(8)? as usize;
        let lo = b.get(8)? as usize;
        let length = hi << 16 | mid << 8 | lo;

        // The 768 code lengths (256 literals, then 512 match symbols) come in two runs, 0..256
        // and 256..768, each preceded by 20 four-bit lengths of a tree for its symbols:
        //   0..=16  the new length is (old + 17 - symbol) % 17, a delta from the last block's;
        //   17      a run of zeros (3 + 4 bits, + 1 in the first run);
        //   18      a longer run of zeros (19 + 6 bits, or 5 bits and 20 + in the second run);
        //   19      a run of one new length: (3 + 1 bit, + 1 in the first run), then its
        //           delta symbol.
        let mut pos = 0;
        for (extra, end) in [(1usize, 256usize), (0, 768)] {
            let mut tl = [0u8; 20];
            for v in &mut tl {
                *v = b.get(4)? as u8;
            }
            let tree = Huffman::new(&tl).ok_or("bad code-length code")?;
            while pos < end {
                let s = tree.decode(b)? as usize;
                let (run, value) = match s {
                    17 => (3 + extra + b.get(4)? as usize, 0),
                    18 => (19 + extra + b.get(6 - extra as u32)? as usize, 0),
                    19 => {
                        let run = 3 + extra + b.get(1)? as usize;
                        let s2 = tree.decode(b)? as usize;
                        if s2 > 16 {
                            return Err("bad code-length symbol");
                        }
                        (run, (self.lens[pos] as usize + 17 - s2) % 17)
                    }
                    _ => (1, (self.lens[pos] as usize + 17 - s) % 17),
                };
                let stop = (pos + run).min(end);
                self.lens[pos..stop].fill(value as u8);
                pos = stop;
            }
        }
        self.literals = Some(Huffman::new(&self.lens).ok_or("bad literal code")?);
        self.block_left = length;
        Ok(())
    }

    fn put(&mut self, byte: u8, out: &mut Option<&mut Vec<u8>>) {
        self.window[self.produced & (WINDOW - 1)] = byte;
        self.produced += 1;
        if let Some(o) = out {
            o.push(byte);
        }
    }

    /// Appends exactly `n` more bytes of the stream to `out` (or drops them).
    fn read(&mut self, mut n: usize, mut out: Option<&mut Vec<u8>>) -> R<()> {
        while n > 0 {
            if self.pending > 0 {
                let k = self.pending.min(n);
                for _ in 0..k {
                    let b = self.window[(self.produced - self.distance) & (WINDOW - 1)];
                    self.put(b, &mut out);
                }
                self.pending -= k;
                n -= k;
                continue;
            }
            if self.block_left == 0 {
                self.block_header()?;
                continue;
            }
            let sym = self.literals.as_ref().ok_or("no literal code")?.decode(&mut self.bits)? as usize;
            if sym < 256 {
                self.put(sym as u8, &mut out);
                self.block_left -= 1;
                n -= 1;
                continue;
            }
            let sym = sym - 256;
            let slot = sym & 31;
            let extra = slot_bits(slot);
            let mut distance = slot_base(slot);
            if extra >= 3 && self.block_type == BlockType::Aligned {
                distance += (self.bits.get(extra - 3)? as usize) << 3;
                let o = self.offsets.as_ref().ok_or("no offset code")?;
                distance += o.decode(&mut self.bits)? as usize;
            } else {
                distance += self.bits.get(extra)? as usize;
                if distance == 0 {
                    distance = self.last_distance;
                }
            }
            self.last_distance = distance;
            let lslot = (sym >> 5) & 15;
            let len = slot_base(lslot) + 3 + self.bits.get(slot_bits(lslot))? as usize;
            if len > self.block_left {
                return Err("a match runs past its block");
            }
            if distance > self.produced.min(WINDOW - 1) {
                return Err("a match reaches before the start of the data");
            }
            self.block_left -= len;
            self.pending = len;
            self.distance = distance;
        }
        Ok(())
    }
}

/// Where the bytes of a group's files come from.
enum Source<'a> {
    Stored { data: &'a [u8], pos: usize },
    Crunched(Box<Decoder<'a>>),
}

impl Source<'_> {
    fn read(&mut self, n: usize, out: Option<&mut Vec<u8>>) -> R<()> {
        match self {
            Source::Stored { data, pos } => {
                let chunk = data.get(*pos..pos.saturating_add(n)).ok_or("ran out of data")?;
                *pos += n;
                if let Some(o) = out {
                    o.extend_from_slice(chunk);
                }
                Ok(())
            }
            Source::Crunched(d) => d.read(n, out),
        }
    }
}

/// One file of a group.
struct Node {
    /// The name as written (for messages).
    raw: String,
    /// The cleaned name, `None` when nothing usable is left.
    name: Option<String>,
    size: u32,
    crc: u32,
}

/// Extracts the files of one group from its packed data `src`.
fn extract_group(nodes: &[Node], mode: u8, src: &[u8], truncated: bool, out: &mut Out) {
    let mut source = match mode {
        0 => Source::Stored { data: src, pos: 0 },
        2 => Source::Crunched(Box::new(Decoder::new(src))),
        m => {
            for n in nodes {
                out.skip(n.name.clone().unwrap_or_else(|| n.raw.clone()), format!("unsupported LZX pack mode {m}"));
            }
            return;
        }
    };
    for (i, node) in nodes.iter().enumerate() {
        let shown = node.name.clone().unwrap_or_else(|| node.raw.clone());
        let size = node.size as usize;
        let keep = match &node.name {
            None => {
                out.skip(shown.clone(), "no usable name");
                false
            }
            Some(n) => out.room(n, size as u64),
        };
        if !keep && size > out.limits.max_entry {
            // too big to run through just to keep the stream going: the rest of the group is lost
            for n in &nodes[i + 1..] {
                out.skip(n.name.clone().unwrap_or_else(|| n.raw.clone()), "follows a file that is too large, in the same merged group");
            }
            return;
        }
        let mut buf = Vec::new();
        if keep {
            buf.reserve(size.min(1 << 20));
        }
        if let Err(why) = source.read(size, keep.then_some(&mut buf)) {
            let why = if truncated && why == "ran out of data" { "LZX data is truncated".to_string() } else { format!("LZX data error: {why}") };
            out.skip(shown, why);
            for n in &nodes[i + 1..] {
                out.skip(n.name.clone().unwrap_or_else(|| n.raw.clone()), "follows a file that could not be read, in the same merged group");
            }
            return;
        }
        if keep {
            if crc32(0, &buf) != node.crc {
                out.skip(shown, "LZX CRC error");
            } else if let Some(name) = node.name.clone() {
                out.push(name, || Ok(buf));
            }
        }
    }
}

/// Extracts the files of the LZX archive `data` into `out`.
pub(crate) fn extract(data: &[u8], out: &mut Out) -> Result<(), ArchiveError> {
    if !is_lzx(data) {
        return Err(ArchiveError("not an LZX archive".into()));
    }
    let mut pos = INFO_LEN;
    let mut group: Vec<Node> = Vec::new();
    let mut first = true;
    while pos < data.len() {
        let h = match parse_header(data, pos) {
            Ok(h) => h,
            Err(e) => {
                let why = if e == HeaderError::Crc { "a file header's checksum is wrong" } else { "a file header is cut short" };
                if first {
                    return Err(ArchiveError(format!("LZX: {why}")));
                }
                out.skip("", format!("LZX archive damaged after this point: {why}"));
                group.clear();
                break;
            }
        };
        first = false;
        pos += h.len;
        let raw = text(&h.name);
        group.push(Node { name: clean_name(&raw), raw, size: h.unpacked, crc: h.crc });
        if h.packed == 0 {
            continue;
        }
        let end = pos.saturating_add(h.packed as usize);
        let packed = &data[pos..end.min(data.len())];
        extract_group(&group, h.mode, packed, end > data.len(), out);
        group.clear();
        pos = end;
    }
    // files with no packed data at the end: only empty ones can be had
    for n in group {
        let shown = n.name.clone().unwrap_or_else(|| n.raw.clone());
        match n.name {
            Some(name) if n.size == 0 && n.crc == 0 => {
                if out.room(&name, 0) {
                    out.push(name, || Ok(Vec::new()));
                }
            }
            _ => out.skip(shown, "LZX data is missing"),
        }
    }
    if out.entries.is_empty() && out.skipped.is_empty() {
        return Err(ArchiveError("LZX: no file".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
