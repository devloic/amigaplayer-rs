//! Jochen Hippel ST (Eagleplayer "Jochen Hippel ST", module prefixes `hst.` and `sdc.`).
//!
//! Clean-room, written from a behaviour specification drawn up from the Paula register writes of
//! the 68k reference player and black-box runs; no original code. The module container is the
//! one of the Amiga "COSO" family (a header of table offsets, pitch programs, instruments with a
//! volume program, patterns, positions and subsongs), either as a COSO image or as a raw "TFMX"
//! module that may follow a few hundred bytes of prefix code. The engine is different from
//! `hippel.rs`: it was written for the Atari ST and keeps a software model of a YM2149 (three tone
//! channels, one noise generator, a mixer and a volume per channel), updated once per tick, and
//! turns that model into Paula writes: tone is a 4-byte pulse looped at a period derived from the
//! YM period, noise a 1024-byte buffer, silence a one-word sample, and a fourth voice plays the
//! digital samples (or the "volume as sample" trick of the ST) on a fourth Paula channel.
//!
//! The write values and their order follow the specification. The write times follow its measured
//! medians (the original polls the beam position inside every note start, which the spec could only
//! measure, and its per-tick cost is not reproduced).
//!
//! Embedded data: `jochenhippelst_data.bin` (52 bytes) is the volume table together with the bytes
//! that follow it in the original player, which a volume index above 17 reads. It is taken from the
//! original player on the project owner's decision; the 18 table entries are written out in the
//! specification and the rest are the bytes real modules reach.
//!
//! Choices where the specification is silent (all safe, none can panic or hang):
//! * the prefix's pointer to the module is found as the first PC-relative address load in the first
//!   128 words whose target holds a module tag; failing that the module is not recognised;
//! * a volume index past the 52 known values reads as 0;
//! * a digital sample number past the sample table is ignored;
//! * a pitch or volume program is run for a bounded number of steps per tick (4096);
//! * an unknown command in the volume program stops it; an instrument or pitch program number
//!   past the table's count is taken as 0, and a table entry that points outside the image reads
//!   as the empty stream;
//! * the two-file (companion sample) module path follows the description but is not verified
//!   against the reference.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Jochen Hippel ST", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// The volume table (18 values) and the 34 bytes after it.
static VOL_TABLE: &[u8] = include_bytes!("jochenhippelst_data.bin");

/// The YM period for each note (96 notes, then zeros up to 127).
const YM_PERIODS: [u16; 96] = [
    0xEEE, 0xE17, 0xD4D, 0xC8E, 0xBD9, 0xB2F, 0xA8E, 0x9F7, 0x967, 0x8E0, 0x861, 0x7E8, 0x777, 0x70B, 0x6A6, 0x647, 0x5EC, 0x597,
    0x547, 0x4FB, 0x4B3, 0x470, 0x430, 0x3F4, 0x3BB, 0x385, 0x353, 0x323, 0x2F6, 0x2CB, 0x2A3, 0x27D, 0x259, 0x238, 0x218, 0x1FA,
    0x1DD, 0x1C2, 0x1A9, 0x191, 0x17B, 0x165, 0x151, 0x13E, 0x12C, 0x11C, 0x10C, 0xFD, 0xEE, 0xE1, 0xD4, 0xC8, 0xBD, 0xB2, 0xA8,
    0x9F, 0x96, 0x8E, 0x86, 0x7E, 0x77, 0x70, 0x6A, 0x64, 0x5E, 0x59, 0x54, 0x4F, 0x4B, 0x47, 0x43, 0x3F, 0x3B, 0x38, 0x35, 0x32,
    0x2F, 0x2C, 0x2A, 0x27, 0x25, 0x23, 0x21, 0x1F, 0x1D, 0x1C, 0x1A, 0x19, 0x17, 0x16, 0x15, 0x13, 0x12, 0x11, 0x10, 0xF,
];

/// The levels of the digital voice's one-byte "volume as sample" trick.
const LEVELS: [u8; 18] = [0x80, 0x93, 0xA7, 0xBF, 0xCF, 0xDF, 0xEF, 0xFF, 0x0F, 0x1F, 0x2F, 0x3F, 0x4F, 0x5F, 0x6F, 0x7F, 0x00, 0xFF];

/// Paula channel and DMA mask of the four sound sources A, B, C and the digital voice.
const CHAN: [(usize, u16); 4] = [(0, 1), (3, 8), (2, 4), (1, 2)];

/// The play code's cost before the first write of a tick (the measured baseline).
const FIRST_GAP: u64 = 1790;

/// Colour clocks from the last write of a note start's first half (the length) to the interrupt
/// request, by source.
const LEN_TO_INTREQ: [u64; 4] = [195, 198, 195, 189];

fn vol_table(i: usize) -> u16 {
    VOL_TABLE.get(i).copied().unwrap_or(0) as u16
}

fn digital_note(i: usize) -> u32 {
    (1024.0 * 2f64.powf(i as f64 / 12.0)) as u32
}

// ------------------------------------------------------------------ the module

/// A table of digital samples: its bytes from the table's start and where they are in guest memory.
#[derive(Clone)]
struct Dig {
    data: Vec<u8>,
    base: u32,
}

impl Dig {
    fn start(&self, n: usize) -> u16 {
        be16(&self.data, 8 * n)
    }
    fn mark(&self, n: usize) -> u16 {
        be16(&self.data, 8 * n + 2)
    }
    /// The number of entries (up to the first with a zero start).
    fn count(&self) -> usize {
        (0..self.data.len() / 8).take_while(|&n| self.start(n) != 0).count()
    }
}

/// What the prefix code (the bytes before the module) says.
#[derive(Clone, Copy, Default)]
struct Prefix {
    /// An indirect jump is in it: the new command set.
    jump: bool,
    /// The byte before the first timer-data word.
    timer: Option<u8>,
    /// The two code patterns found (bit 1 and bit 0 of the type).
    kind: u8,
}

/// A parsed module: the COSO image the player works on.
struct Module {
    /// The image, then the empty instrument and pattern.
    b: Vec<u8>,
    null_inst: usize,
    /// Offset of the image in the file, for a COSO image (its guest address is the module's plus
    /// this); None for an image built from a raw module.
    in_file: Option<usize>,
    short: bool,
    mmme: bool,
    new_type: bool,
    n_fs: usize,
    n_inst: usize,
    t_fs: usize,
    t_inst: usize,
    t_pat: usize,
    t_pos: usize,
    t_sub: usize,
    n_sub: usize,
    /// Offset in the file of the digital sample table, if any (the samples are read from the module's own memory).
    dig_file: Option<usize>,
    /// The sample table is in a companion file.
    lsmp: bool,
    prefix: Prefix,
}

fn tag_at(d: &[u8], o: usize) -> Option<[u8; 4]> {
    d.get(o..o + 4).map(|b| [b[0], b[1], b[2], b[3]])
}

/// The offset of the module's tag: 0, or the target of the first PC-relative address load of the
/// first 128 words that points at one.
fn find_tag(d: &[u8]) -> Option<usize> {
    let is_tag = |o: usize| matches!(tag_at(d, o), Some(t) if &t == b"COSO" || &t == b"TFMX" || &t == b"MMME");
    if is_tag(0) {
        return Some(0);
    }
    for i in 0..128 {
        let w = be16(d, 2 * i);
        if w & 0xF1FF == 0x41FA {
            let t = 2 * i as i64 + 2 + be16(d, 2 * i + 2) as i16 as i64;
            if t > 0 && is_tag(t as usize) {
                return Some(t as usize);
            }
        }
    }
    None
}

/// What the prefix code holds (everything before the tag).
fn scan_prefix(d: &[u8], tag: usize) -> Prefix {
    let mut p = Prefix::default();
    let end = tag.min(d.len()) & !1;
    let mut i = 0;
    while i < end {
        let w = be16(d, i);
        if w == 0x4EFB {
            p.jump = true;
        }
        if p.timer.is_none() && matches!(w, 0xFA23 | 0xFA1F | 0xFA25 | 0xFA21) && i > 0 {
            p.timer = Some(d[i - 1]);
        }
        if i + 4 <= end {
            let l = be32(d, i);
            if l == 0x0003_C686 {
                p.kind |= 2;
            }
            if l == 0x4840_49FA {
                p.kind |= 1;
            }
        }
        i += 2;
    }
    p
}

/// The image size limit (a raw module's tables are counted by 16-bit fields).
const MAX_IMAGE: usize = 8 << 20;

/// Trims a pitch-program or instrument block of a raw module: through its last end or jump
/// command, the whole block when it has none.
fn trim_block(b: &[u8]) -> &[u8] {
    for i in (0..b.len()).rev() {
        match b[i] {
            0xE1 => return &b[..=i],
            0xE0 => return &b[..(i + 2).min(b.len())],
            _ => {}
        }
    }
    b
}

fn parse(d: &[u8]) -> Option<Module> {
    let tag = find_tag(d)?;
    let h = &d[tag..];
    let prefix = if tag > 0 { scan_prefix(d, tag) } else { Prefix::default() };
    match tag_at(h, 0)? {
        t if &t == b"COSO" => parse_coso(h, tag, prefix),
        t if &t == b"TFMX" || &t == b"MMME" => parse_raw(h, tag, prefix),
        _ => None,
    }
}

fn parse_coso(h: &[u8], tag: usize, prefix: Prefix) -> Option<Module> {
    if h.len() < 0x44 {
        return None;
    }
    let mmme = tag_at(h, 0x20)? == *b"MMME";
    if !mmme && tag_at(h, 0x20)? != *b"TFMX" {
        return None;
    }
    if be16(h, 0x30) == 0 || be32(h, 0x18) == 0 {
        return None;
    }
    let off = |o: usize| be32(h, o) as usize;
    let (t_fs, t_inst, t_pat, t_pos, t_sub) = (off(4), off(8), off(12), off(0x10), off(0x14));
    if [t_fs, t_inst, t_pat, t_pos, t_sub].iter().any(|&t| t >= h.len()) {
        return None;
    }
    let short = be16(h, 0x40) != 0;
    let dpos = off(0x18) + 6 * (be16(h, 0x32) as usize + 1);
    let dig_at = matches!(be16(h, dpos), 0x80 | 0x100).then_some(dpos).filter(|&p| p < h.len());
    let mut b = h.to_vec();
    let (null_inst, _) = add_nulls(&mut b);
    let m = Module {
        b,
        null_inst,
        in_file: Some(0),
        short,
        mmme,
        new_type: short,
        n_fs: be16(h, 0x24) as usize + 1,
        n_inst: be16(h, 0x26) as usize + 1,
        t_fs,
        t_inst,
        t_pat,
        t_pos,
        t_sub,
        n_sub: be16(h, 0x30) as usize,
        dig_file: dig_at.map(|p| tag + p),
        lsmp: false,
        prefix,
    };
    // the Amiga flavour of the format starts its pitch programs with a command that the ST
    // flavour has other uses for: more of those than the opposite shape and it is not ours
    let (mut p, mut n) = (0, 0);
    for i in 0..m.n_fs.min(0x400) {
        if let Some(o) = m.entry(m.t_fs, i) {
            if m.g(o as i64) == 0xE2 {
                if m.g(o as i64 + 1) < 0x80 {
                    p += 1;
                } else {
                    n += 1;
                }
            }
        }
    }
    (n >= p).then_some(m)
}

/// The empty instrument (and pitch program) and the empty pattern the player starts from.
fn add_nulls(b: &mut Vec<u8>) -> (usize, usize) {
    let ni = b.len();
    b.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0xE1]);
    let np = b.len();
    b.extend_from_slice(&[0xFF, 0xFF]);
    (ni, np)
}

fn parse_raw(h: &[u8], tag: usize, prefix: Prefix) -> Option<Module> {
    if h.len() < 0x20 {
        return None;
    }
    let mmme = &h[..4] == b"MMME";
    let (w4, w6, w8, w10, w12, w16, w18) = (
        be16(h, 4) as usize,
        be16(h, 6) as usize,
        be16(h, 8) as usize,
        be16(h, 10) as usize,
        be16(h, 12) as usize,
        be16(h, 16) as usize,
        be16(h, 18) as usize,
    );
    let (nfs, ninst, npat, npos) = (w4 + 1, w6 + 1, w8 + 1, w10 + 1);
    if !mmme {
        // detection on the first byte of every pitch program
        if w4 >= 0x200 || w16 == 0 {
            return None;
        }
        let (mut p, mut n) = (0, 0);
        for i in 0..nfs {
            let o = 0x20 + 64 * i;
            if h.get(o) == Some(&0xE2) {
                match h.get(o + 1) {
                    Some(&x) if x < 0x80 => p += 1,
                    Some(_) => n += 1,
                    None => {}
                }
            }
        }
        if n < p {
            return None;
        }
    }
    let pat_size = w12 + (w12 & 1);
    let o_pat = 0x20 + 64 * (nfs + ninst);
    let o_pos = o_pat + npat * pat_size;
    let o_sub = o_pos + npos * 12;
    let o_ext = o_sub + (w16 + 1) * 6;
    let o_dig = o_ext + (w18 + 1) * 6;
    if o_dig > h.len() || o_dig > MAX_IMAGE || pat_size == 0 {
        return None;
    }
    let lsmp = &h[0x1C..0x20] == b"LSMP";
    let dig_ok = !lsmp && matches!(be16(h, o_dig), 0x80 | 0x100);
    let new_type = prefix.jump;
    let es = if new_type { 2 } else { 4 };
    // the image: header, three tables, the trimmed blocks and compact patterns, then the lists
    let mut b = vec![0u8; 0x40];
    b[..4].copy_from_slice(b"COSO");
    b[0x20..0x40].copy_from_slice(&h[..0x20]);
    let put_off = |b: &mut Vec<u8>, at: usize, v: usize| {
        if new_type {
            b[at..at + 2].copy_from_slice(&(v as u16).to_be_bytes());
        } else {
            b[at..at + 4].copy_from_slice(&(v as u32).to_be_bytes());
        }
    };
    // each table is followed by the data its entries point at
    let t_fs = b.len();
    b.resize(t_fs + es * nfs, 0);
    for i in 0..nfs {
        let at = b.len();
        put_off(&mut b, t_fs + es * i, at);
        b.extend_from_slice(trim_block(&h[0x20 + 64 * i..0x20 + 64 * i + 64]));
    }
    // the next table starts on an even offset, as in the COSO files
    if b.len() & 1 != 0 {
        b.push(0);
    }
    let t_inst = b.len();
    b.resize(t_inst + es * ninst, 0);
    for i in 0..ninst {
        let at = b.len();
        put_off(&mut b, t_inst + es * i, at);
        let s = 0x20 + 64 * (nfs + i);
        b.extend_from_slice(trim_block(&h[s..s + 64]));
    }
    if b.len() & 1 != 0 {
        b.push(0);
    }
    let t_pat = b.len();
    b.resize(t_pat + es * npat, 0);
    for i in 0..npat {
        let at = b.len();
        put_off(&mut b, t_pat + es * i, at);
        let s = o_pat + i * pat_size;
        compact_pattern(h, s, pat_size, &mut b);
        if b.len() > MAX_IMAGE {
            return None;
        }
    }
    if b.len() & 1 != 0 {
        b.push(0);
    }
    let t_pos = b.len();
    b.extend_from_slice(&h[o_pos..o_sub]);
    let t_sub = b.len();
    b.extend_from_slice(&h[o_sub..o_ext]);
    let t_ext = b.len();
    b.extend_from_slice(&h[o_ext..o_dig]);
    let t_dig = b.len();
    b.extend_from_slice(&h[o_dig..]);
    for (o, v) in [(4, t_fs), (8, t_inst), (12, t_pat), (0x10, t_pos), (0x14, t_sub), (0x18, t_ext), (0x1C, t_dig)] {
        b[o..o + 4].copy_from_slice(&(v as u32).to_be_bytes());
    }
    let (null_inst, _) = add_nulls(&mut b);
    Some(Module {
        b,
        null_inst,
        in_file: None,
        short: new_type,
        mmme,
        new_type,
        n_fs: nfs,
        n_inst: ninst,
        t_fs,
        t_inst,
        t_pat,
        t_pos,
        t_sub,
        n_sub: w16,
        dig_file: dig_ok.then_some(tag + o_dig),
        lsmp,
        prefix,
    })
}

/// Re-encodes a raw pattern (2-byte rows, a third byte before the row when the flags ask for one)
/// as the compact row stream: waits as row speeds, rows, the end marker.
fn compact_pattern(h: &[u8], start: usize, size: usize, out: &mut Vec<u8>) {
    let rows = size / 2;
    let byte = |o: isize| -> u8 { if o < 0 { 0 } else { h.get(o as usize).copied().unwrap_or(0) } };
    let note = |i: usize| byte(start as isize + 2 * i as isize);
    let end = (0..rows).find(|&i| note(i) == 1).unwrap_or(rows);
    let real: Vec<usize> = (0..end).filter(|&i| note(i) != 0).collect();
    match real.first() {
        None => {
            if end > 0 {
                out.extend_from_slice(&[0xFD, (end - 1) as u8]);
            }
        }
        Some(&r0) => {
            if r0 > 0 {
                out.extend_from_slice(&[0xFD, (r0 - 1) as u8]);
            }
            let mut prev = None;
            for (k, &i) in real.iter().enumerate() {
                let next = real.get(k + 1).copied().unwrap_or(end);
                let d = (next - i - 1) as u8;
                // the speed is stated only where it changes
                if prev != Some(d) {
                    out.extend_from_slice(&[0xFE, d]);
                    prev = Some(d);
                }
                let flags = byte(start as isize + 2 * i as isize + 1);
                out.push(note(i));
                out.push(flags);
                if flags & 0xE0 != 0 {
                    out.push(byte(start as isize + 2 * i as isize - 1));
                }
            }
        }
    }
    out.push(0xFF);
}

fn detect(d: &[u8]) -> bool {
    parse(d).is_some()
}

impl Module {
    fn g(&self, o: i64) -> u8 {
        if o < 0 {
            0
        } else {
            self.b.get(o as usize).copied().unwrap_or(0)
        }
    }

    /// The offset a table entry points at, None if it is outside the image.
    fn entry(&self, tab: usize, i: usize) -> Option<usize> {
        let o = if self.short {
            let v = be16(&self.b, tab + 2 * i) as i16 as i64;
            if v < 0 {
                return None;
            }
            v as usize
        } else {
            be32(&self.b, tab + 4 * i) as usize
        };
        (o < self.b.len()).then_some(o)
    }
}

// ------------------------------------------------------------------ the player

#[derive(Clone, Copy, Default)]
struct Voice {
    // sequencer
    pos_ptr: usize,
    pos_off: usize,
    pat: usize,
    row_counter: u8,
    row_speed: u8,
    ta: u8,
    tb: u8,
    vol_red: u8,
    row_note: u8,
    row_flags: u8,
    row_param: u8,
    porta: i32,
    // instrument and programs
    inst: usize,
    vs_idx: i32,
    vs_wait: u8,
    vs_speed: u8,
    vs_hold: u8,
    volume: u8,
    fs: usize,
    fs_idx: i32,
    fs_wait: u8,
    note: u8,
    vib_speed: u8,
    vib_depth: u8,
    vib_delay: u8,
    vib_pos: u8,
    vib_flag: u8,
    mode: u8,
    noise: u8,
    pitch_end: u8,
    sid_src: u8,
    sid_par: u8,
    // the YM image of the voice
    ym_per: u16,
    ym_vol: u8,
}

/// The digital voice's state.
#[derive(Clone, Copy)]
struct Digital {
    state: u8,
    start: u32,
    len1: u16,
    lp: u32,
    len2: u16,
    per: u16,
    /// 0..64, or None for "the volume of channel A".
    vol: Option<u16>,
}

pub struct JochenHippelSt {
    m: Module,
    dig: Option<Dig>,
    // guest memory the player allocated
    noise: u32,
    pulse: u32,
    silent: u32,
    level_cell: u32,
    // song state
    sub: u16,
    counter: u8,
    speed: u8,
    songpos: i32,
    sub_start: u16,
    sub_end: u16,
    v: [Voice; 3],
    mixer: u8,
    noise_reg: u8,
    d: Digital,
    /// The digital "Period" constant and whether the sample command has a note argument.
    period: u16,
    note_args: bool,
    // the Paula side
    state: [u8; 3],
    running: [bool; 4],
    a_vol: u16,
    /// The level byte the digital voice plays in SID mode.
    level: u8,
    songend: bool,
}

impl JochenHippelSt {
    fn init_sound(&mut self, n: u16) {
        self.sub = n;
        let e = self.m.t_sub + 6 * (n as usize).saturating_sub(1);
        let (mut start, mut end, mut speed) = (be16(&self.m.b, e), be16(&self.m.b, e + 2), be16(&self.m.b, e + 4));
        if speed == 0 {
            speed = 4;
        }
        if (end as i16).wrapping_sub(start as i16) < 0 {
            // the table entry is patched to an empty song, in the module's memory
            start = 0;
            end = 0;
            if e + 4 <= self.m.b.len() {
                self.m.b[e..e + 4].fill(0);
            }
        }
        self.sub_start = start;
        self.sub_end = end;
        self.counter = 1;
        self.speed = speed as u8;
        self.songpos = end as i32 - start as i32;
        self.songend = false;
        for i in 0..3 {
            let mut v = Voice::default();
            v.inst = self.m.null_inst;
            v.fs = self.m.null_inst;
            // the empty instrument's volume program is read from the stream's first byte
            v.vs_idx = -5;
            v.vs_wait = 1;
            v.vs_speed = 1;
            v.sid_src = 0xFF;
            v.pos_ptr = self.m.t_pos + start as usize * 12 + i * 4;
            let e = v.pos_ptr;
            v.pos_off = 12;
            v.pat = self.pattern(self.m.g(e as i64) as usize);
            v.ta = self.m.g(e as i64 + 1);
            v.tb = self.m.g(e as i64 + 2);
            let f = self.m.g(e as i64 + 3);
            if f >> 4 == 0xF {
                v.vol_red = f & 0xF;
            }
            self.v[i] = v;
        }
        self.reset_ym();
    }

    /// The emulation reset: the YM shadows and the digital voice.
    fn reset_ym(&mut self) {
        self.mixer = 0x3B;
        self.noise_reg = 0;
        for v in self.v.iter_mut() {
            v.ym_vol = 0;
            v.ym_per = 0x400;
        }
        self.v[2].ym_vol = 0x10;
        self.d.state = 0;
        self.state = [0; 3];
    }

    fn pattern(&self, n: usize) -> usize {
        // a pattern number whose entry points outside the image reads the memory past it (zeros)
        self.m.entry(self.m.t_pat, n).unwrap_or(self.m.b.len() + 64)
    }

    // -------------------------------------------------------------- one tick's music

    fn play(&mut self) {
        self.voice(0);
        self.sid();
        self.voice(1);
        self.voice(2);
        self.tracker();
    }

    fn voice(&mut self, i: usize) {
        self.v[i].noise = 0;
        self.pitch_program(i);
        self.volume_program(i);
        self.period_mixer(i);
    }

    fn pitch_program(&mut self, i: usize) {
        if self.v[i].fs_wait != 0 {
            self.v[i].fs_wait -= 1;
            return;
        }
        let new = self.m.new_type;
        let has_dig = self.dig.is_some();
        for _ in 0..4096 {
            let v = self.v[i];
            let at = v.fs as i64 + v.fs_idx as i64;
            let c = self.m.g(at);
            let arg = self.m.g(at + 1);
            // in the old command set the bytes from $EB on are notes, and $EA is one when
            // the module has no digital samples
            let is_cmd = (0xE0..=0xEF).contains(&c) && (new || c < 0xEB) && !(c == 0xEA && !new && !has_dig);
            if !is_cmd {
                let v = &mut self.v[i];
                v.note = c;
                v.fs_idx += 1;
                return;
            }
            let v = &mut self.v[i];
            match c {
                0xE0 => v.fs_idx = if new { arg as i32 } else { (arg & 0x3F) as i32 },
                0xE1 => {
                    if new {
                        v.note = self.m.g(at - 1);
                    }
                    return;
                }
                0xE2 => {
                    v.vs_idx = 0;
                    v.fs_idx += 1;
                }
                0xE3 => {
                    v.vib_speed = arg;
                    v.vib_depth = self.m.g(at + 2);
                    v.fs_idx += 3;
                }
                0xE4 => {
                    v.mode = 0;
                    v.noise = arg;
                    v.fs_idx += 2;
                }
                0xE5 => {
                    v.mode = 1;
                    v.fs_idx += 1;
                }
                0xE6 => {
                    v.mode = 2;
                    v.fs_idx += 1;
                }
                0xE7 => {
                    if new {
                        // the program restarts at the image's first byte
                        v.fs = 0;
                        v.fs_idx = 0;
                    } else {
                        self.d.state = 0;
                        v.fs_idx += 1;
                    }
                }
                0xE8 => {
                    v.fs_idx += 2;
                    v.fs_wait = arg;
                    if v.fs_wait != 0 {
                        v.fs_wait -= 1;
                        return;
                    }
                }
                0xE9 => v.fs_idx += 2,
                0xEA if new => {
                    v.row_flags = 0x20;
                    v.row_param = arg;
                    v.fs_idx += 2;
                }
                0xEA | 0xEC => {
                    let two = self.note_args;
                    v.fs_idx += if two { 3 } else { 2 };
                    let note_arg = self.m.g(at + 2);
                    self.digital_command(i, arg as usize, note_arg);
                }
                0xEB => {
                    v.pitch_end = arg;
                    v.fs_idx += 2;
                }
                0xED => {
                    self.d.state = 0;
                    v.fs_idx += 1;
                }
                0xEE => {
                    v.sid_src = arg;
                    v.fs_idx += 2;
                }
                _ => {
                    v.sid_par = arg;
                    v.fs_idx += 2;
                }
            }
        }
    }

    /// The digital sample command: sample `n`, with a note argument for the converted kind.
    fn digital_command(&mut self, i: usize, n: usize, note_arg: u8) {
        let Some(dig) = &self.dig else { return };
        if n >= dig.count() {
            return;
        }
        let start = dig.start(n) as u32;
        let mark = dig.mark(n);
        let addr = dig.base.wrapping_add(start);
        let (len1, lp, len2) = match mark {
            0 => (((dig.start(n + 1) as u32).wrapping_sub(start) / 2) as u16, self.silent, 1),
            0x10 => (8, addr, 8),
            end => (((end as u32).wrapping_sub(start) / 2) as u16, self.silent, 1),
        };
        let mut per = self.period;
        if self.note_args && note_arg & 0x80 == 0 {
            let v = &self.v[i];
            let idx = (note_arg.wrapping_add(v.row_note).wrapping_add(v.ta) & 0x7F) as usize;
            if idx < 96 {
                let t = digital_note(idx);
                let q = (self.period as u32 * 0x2000) / (t / 8).max(1);
                per = (q as u16).wrapping_add(1);
            }
        }
        self.d = Digital { state: 1, start: addr, len1, lp, len2, per, vol: Some(64) };
    }

    fn volume_program(&mut self, i: usize) {
        let new = self.m.new_type;
        let v = &mut self.v[i];
        if v.vs_hold != 0 {
            v.vs_hold -= 1;
            return;
        }
        v.vs_wait = v.vs_wait.wrapping_sub(1);
        if v.vs_wait != 0 {
            return;
        }
        v.vs_wait = v.vs_speed;
        for _ in 0..4096 {
            let at = v.inst as i64 + 5 + v.vs_idx as i64;
            let c = self.m.g(at);
            match c {
                0..=0xDF => {
                    v.volume = c;
                    v.vs_idx += 1;
                    return;
                }
                0xE0 => {
                    let n = self.m.g(at + 1);
                    let n = if new { n } else { n & 0x3F };
                    v.vs_idx = n as i32 - 5;
                }
                0xE1 => {
                    if new {
                        v.volume = self.m.g(at - 1);
                    }
                    return;
                }
                0xE8 => {
                    v.vs_hold = self.m.g(at + 1);
                    v.vs_idx += 2;
                    if v.vs_hold != 0 {
                        v.vs_hold -= 1;
                        return;
                    }
                }
                _ => return,
            }
        }
    }

    fn period_mixer(&mut self, i: usize) {
        let v = self.v[i];
        let n = if v.note & 0x80 == 0 { v.note.wrapping_add(v.row_note).wrapping_add(v.ta) } else { v.note } & 0x7F;
        let p0 = YM_PERIODS.get(n as usize).copied().unwrap_or(0);
        let (tone, nz) = (1u8 << i, 8u8 << i);
        let mut noise = v.noise;
        match v.mode {
            0 => self.mixer &= !(tone | nz),
            1 => {
                self.mixer = (self.mixer | tone) & !nz;
                noise = if v.note & 0x80 == 0 { v.row_note.wrapping_add(v.note) } else { v.note & 0x7F };
            }
            _ => self.mixer = (self.mixer & !tone) | nz,
        }
        if noise != 0 {
            self.noise_reg = !noise & 0x1F;
        }
        let mut per = p0 as i32;
        let mmme = self.m.mmme;
        let vm = &mut self.v[i];
        if vm.vib_delay != 0 {
            vm.vib_delay -= 1;
        } else if !mmme {
            let depth = if vm.vib_depth & 0x80 == 0 { vm.vib_depth as i32 * 2 } else { (vm.vib_depth & 0x7F) as i32 };
            let mut pos = vm.vib_pos as i32;
            let hold = vm.vib_flag & 0x81 == 0x81;
            if !hold {
                if vm.vib_flag & 0x20 == 0 {
                    pos -= vm.vib_speed as i32;
                    if pos < 0 {
                        vm.vib_flag |= 0x20;
                        pos = 0;
                    }
                } else {
                    pos += vm.vib_speed as i32;
                    if pos >= depth {
                        vm.vib_flag &= !0x20;
                        pos = depth;
                    }
                }
                vm.vib_pos = pos as u8;
            }
            let mut delta = pos - depth / 2;
            let mut k = 2 * n as u32 + 0xA0;
            if k < 0x100 {
                loop {
                    delta *= 2;
                    k += 0x18;
                    if k >= 0x100 {
                        break;
                    }
                }
            }
            per += delta;
            vm.vib_flag ^= 1;
        } else {
            let depth = vm.vib_depth as i32 * 2;
            let mut pos = vm.vib_pos as i32;
            if vm.vib_flag & 0x20 == 0 {
                pos -= vm.vib_speed as i32;
                if pos < 0 {
                    vm.vib_flag |= 0x20;
                    pos = 0;
                }
            } else {
                pos += vm.vib_speed as i32;
                if pos >= depth {
                    vm.vib_flag &= !0x20;
                    pos = depth;
                }
            }
            vm.vib_pos = pos as u8;
            let delta = pos - vm.vib_depth as i32;
            per = (p0 as i32) + ((delta * p0 as i32) >> 10);
        }
        if vm.row_flags & 0x20 != 0 {
            let sp = vm.row_param as i8 as i32;
            if !mmme {
                vm.porta = vm.porta.wrapping_add(sp << 12);
                per -= (vm.porta >> 16) as i32;
            } else {
                vm.porta = vm.porta.wrapping_add(sp) & 0xFFFF;
                let acc = vm.porta as i16 as i32;
                per = p0 as i32 + (per - p0 as i32) - ((acc * p0 as i32) >> 10);
            }
        }
        let mut per = per as u16;
        if mmme && vm.pitch_end & 2 != 0 {
            per = 0;
        }
        vm.ym_per = per;
        let vol = vm.volume.wrapping_sub(vm.vol_red);
        vm.ym_vol = if vol & 0x80 != 0 { 0 } else { vol };
    }

    /// The digital voice's "SID" mode (after voice 0's update).
    fn sid(&mut self) {
        let mode = self.v[0].sid_src;
        if mode & 0x80 != 0 {
            return;
        }
        let src = match mode {
            0 => None,
            2 => Some(self.v[1].ym_per),
            3 => Some(self.v[2].ym_per),
            4 => Some(0),
            _ => Some(self.v[0].ym_per),
        };
        let Some(p) = src else {
            self.v[0].sid_src = 0xFF;
            self.d.state = 0;
            return;
        };
        if mode != 4 {
            if p <= 0x10 || p > 0xEEE {
                self.v[0].sid_src = 0xFF;
                self.d.state = 0;
                return;
            }
            self.d.per = p.wrapping_mul(7).wrapping_add(1);
        }
        self.level = LEVELS[(self.v[0].ym_vol & 15) as usize];
        self.d.start = self.level_cell;
        self.d.lp = self.level_cell;
        self.d.len1 = 1;
        self.d.len2 = 1;
        self.d.vol = None;
        self.d.state = 1;
    }

    fn tracker(&mut self) {
        self.counter = self.counter.wrapping_sub(1);
        if self.counter != 0 {
            return;
        }
        self.counter = self.speed;
        for i in 0..3 {
            self.sequencer(i);
        }
    }

    fn sequencer(&mut self, i: usize) {
        let v = &mut self.v[i];
        v.row_counter = v.row_counter.wrapping_sub(1);
        if v.row_counter & 0x80 == 0 {
            return;
        }
        v.row_counter = v.row_speed;
        for _ in 0..64 {
            let at = self.v[i].pat as i64;
            let c = self.m.g(at);
            match c {
                0xFF => {
                    if i == 0 {
                        self.songpos -= 1;
                        if self.songpos < 0 {
                            self.songend = true;
                            self.reset_ym();
                            self.v[0].sid_src = 0xFF;
                            self.songpos = self.sub_end as i32 - self.sub_start as i32;
                            for v in self.v.iter_mut() {
                                v.pos_off = 0;
                            }
                        }
                    }
                    let v = &mut self.v[i];
                    let e = (v.pos_ptr + v.pos_off) as i64;
                    let (pat, ta, tb, f) = (self.m.g(e), self.m.g(e + 1), self.m.g(e + 2), self.m.g(e + 3));
                    v.pos_off += 12;
                    v.ta = ta;
                    v.tb = tb;
                    match f >> 4 {
                        0xF => v.vol_red = f & 0xF,
                        0xE => self.speed = f & 0xF,
                        _ => {}
                    }
                    self.v[i].pat = self.pattern(pat as usize);
                }
                0xFE | 0xFD => {
                    let n = self.m.g(at + 1);
                    let v = &mut self.v[i];
                    v.row_speed = n;
                    v.row_counter = n;
                    v.pat += 2;
                    if c == 0xFD {
                        return;
                    }
                }
                _ => {
                    let flags = self.m.g(at + 1);
                    let v = &mut self.v[i];
                    v.row_note = c;
                    v.row_flags = flags;
                    v.pat += 2;
                    if flags & 0xE0 != 0 {
                        v.row_param = self.m.g(at + 2);
                        v.pat += 1;
                    }
                    v.porta = 0;
                    if c & 0x80 != 0 {
                        return;
                    }
                    self.new_note(i, flags);
                    return;
                }
            }
        }
    }

    fn new_note(&mut self, i: usize, flags: u8) {
        let tb = self.v[i].tb;
        let mut n = (flags & 0x1F).wrapping_add(tb) as usize;
        if n >= self.m.n_inst {
            n = 0;
        }
        let inst = self.m.entry(self.m.t_inst, n).unwrap_or(self.m.null_inst);
        let g = |o: usize| self.m.g((inst + o) as i64);
        let (spd, pnum, vs, vd, vdl) = (g(0), g(1), g(2), g(3), g(4));
        let mut prog = if flags & 0x40 != 0 { self.v[i].row_param as usize } else { pnum as usize };
        if prog >= self.m.n_fs {
            prog = 0;
        }
        let fs = self.m.entry(self.m.t_fs, prog).unwrap_or(self.m.null_inst);
        let v = &mut self.v[i];
        v.inst = inst;
        v.vs_speed = spd;
        v.vs_wait = spd;
        v.vs_idx = 0;
        v.vs_hold = 0;
        v.vib_speed = vs;
        v.vib_depth = vd;
        v.vib_delay = vdl;
        v.vib_pos = vd;
        // the MMME vibrato starts upwards
        v.vib_flag = if self.m.mmme { 0x20 } else { 0 };
        v.fs = fs;
        v.fs_idx = 0;
        v.fs_wait = 0;
    }

    // -------------------------------------------------------------- Paula

    fn irq(hw: &mut Hw, v: u16) {
        m68k::AddressBus::write_word(hw.bus, 0xDFF09C, v);
    }

    /// The note-start sequence of source `c`: the channel is stopped, started on the first
    /// sample for one pass, and given its loop.
    fn note_start(&mut self, hw: &mut Hw, c: usize, sa: u32, la: u32, l1: u16, l2: u16) {
        let (ch, mask) = CHAN[c];
        let ibit = mask << 7;
        let running = self.running[c];
        hw.per(ch, 1);
        hw.wait(26);
        Self::irq(hw, if running { ibit } else { 0 });
        hw.wait(6);
        hw.dmacon(if running { mask } else { 0 });
        if running {
            // the original polls the channel's audio flag, a look every 14.5 colour clocks
            let mut t2 = 2 * hw.bus.clock;
            for _ in 0..5000 {
                hw.bus.paula.sync(t2 / 2, &hw.bus.ram);
                if hw.bus.paula.intreq & ibit != 0 {
                    break;
                }
                t2 += 29;
            }
            hw.bus.clock = t2 / 2;
        }
        hw.wait(20);
        hw.loc(ch, sa);
        hw.wait(6);
        hw.len(ch, l1);
        hw.wait(LEN_TO_INTREQ[c]);
        Self::irq(hw, ibit);
        hw.wait(6);
        hw.dmacon(0x8000 | mask);
        self.running[c] = true;
        hw.wait(421);
        hw.loc(ch, la);
        hw.wait(6);
        hw.len(ch, l2);
    }

    fn paula(&mut self, hw: &mut Hw) {
        let d7 = !self.mixer & 0x3F;
        for c in 0..3 {
            let (ch, _) = CHAN[c];
            let tone = d7 & (1 << c) != 0;
            let noise = d7 & (8 << c) != 0;
            let (per, vol);
            if tone {
                if self.state[c] != 1 {
                    self.state[c] = 1;
                    self.note_start(hw, c, self.pulse, self.pulse, 2, 2);
                }
                per = ((self.v[c].ym_per & 0xFFF) as u32 * 7 + 1) as u16;
                vol = vol_table(self.v[c].ym_vol as usize);
            } else if noise {
                if self.state[c] != 2 {
                    self.state[c] = 2;
                    self.note_start(hw, c, self.noise, self.noise, 0x200, 0x200);
                }
                per = 0x280 - 0x10 * (self.noise_reg & 0x1F) as u16;
                vol = vol_table(self.v[c].ym_vol as usize);
            } else {
                self.note_start(hw, c, self.silent, self.silent, 1, 1);
                self.state[c] = 0;
                per = 0x100;
                vol = 0;
            }
            hw.wait(20);
            hw.vol(ch, vol);
            if c == 0 {
                self.a_vol = vol;
            }
            hw.wait(12);
            hw.per(ch, per);
            hw.wait(400);
        }
        // the digital voice
        let d = self.d;
        if d.state == 1 && d.vol.is_none() {
            hw.bus.write_bytes(self.level_cell - 1, &[self.level, self.level]);
        }
        match d.state {
            0 => {
                self.note_start(hw, 3, self.silent, self.silent, 1, 1);
            }
            1 => {
                self.d.state = 2;
                self.note_start(hw, 3, d.start, d.lp, d.len1, d.len2);
            }
            _ => return,
        }
        hw.wait(20);
        hw.per(1, d.per);
        hw.wait(243);
        hw.vol(1, d.vol.unwrap_or(self.a_vol));
    }
}

impl Replayer for JochenHippelSt {
    fn format(&self) -> &'static str {
        "Jochen Hippel ST"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.m.n_sub.clamp(1, 0xFFFF) as u16, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.init_sound(n.max(1));
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.play();
        hw.wait(FIRST_GAP);
        self.paula(hw);
    }
}

fn build_sounds(m: &mut Machine) -> Option<(u32, u32, u32, u32)> {
    // the noise buffer, the pulse, one word that carries the level byte of the SID trick, and
    // the silent word, in the order the original keeps them
    let base = m.alloc(1024 + 16);
    if base == 0 {
        return None;
    }
    let mut x: u32 = 0x1234_5678;
    for k in 0..1024 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        m.bus.write_bytes(base + k, &[(x >> 16) as u8]);
    }
    let pulse = base + 1024;
    m.bus.write_bytes(pulse, &[0x7F, 0x7F, 0x80, 0x80]);
    Some((base, pulse, pulse + 5, pulse + 6))
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let md = parse(d)?;
    let tag = find_tag(d)?;
    let (noise, pulse, level_cell, silent) = build_sounds(m)?;
    // where the image is in guest memory
    let _guest = match md.in_file {
        Some(o) => base + (tag + o) as u32,
        None => {
            let a = m.alloc(md.b.len() as u32);
            if a == 0 {
                return None;
            }
            m.bus.write_bytes(a, &md.b);
            a
        }
    };
    let dig = if md.lsmp {
        let file = m.bus.read_cstr(m.deli.file_buf);
        let names = companion(&file);
        let smp = names.iter().find_map(|n| crate::deli::read_companion(m, n).1.ok())?;
        let a = m.alloc(smp.len() as u32 + 4);
        if a == 0 {
            return None;
        }
        m.bus.write_bytes(a, &smp);
        matches!(be16(&smp, 0), 0x80 | 0x100).then(|| Dig { data: smp, base: a })
    } else {
        md.dig_file.filter(|&o| o < d.len()).map(|o| Dig { data: d[o..].to_vec(), base: base + o as u32 })
    };
    let mut period = 0x248u16;
    if let Some(t) = md.prefix.timer.filter(|&t| t != 0) {
        let q1 = 2_457_600u32 / t as u32;
        let q1 = if q1 > 0xFFFF { 2_457_600 & 0xFFFF } else { q1 };
        let v = q1 >> 2;
        if v != 0 {
            let q = 0x36_1F10u32 / v;
            let q = if q > 0xFFFF { 0x1F10 } else { q };
            period = (q as u16).wrapping_add(1);
        }
    }
    let note_args = md.prefix.kind == 1 && dig.is_some();
    if note_args {
        if let Some(dg) = &dig {
            // the nibble-coded sample area becomes signed bytes in place, when its first long has
            // no bit set in any high nibble
            let end = (0..dg.count()).map(|n| dg.start(n) as usize).last().unwrap_or(0);
            let first = dg.start(0) as usize;
            if end > first && end <= dg.data.len() && be32(&dg.data, first) & 0xF0F0_F0F0 == 0 {
                let conv: Vec<u8> = dg.data[first..end]
                    .iter()
                    .map(|&b| if b & 0x80 == 0 { LEVELS[(b & 15) as usize] } else if b == 0x88 { LEVELS[17] } else { LEVELS[16] })
                    .collect();
                m.bus.write_bytes(dg.base + first as u32, &conv);
            }
        }
    }
    m.bus.write_bytes(level_cell - 1, &[LEVELS[0]]);
    let mut p = JochenHippelSt {
        m: md,
        dig,
        noise,
        pulse,
        silent,
        level_cell,
        sub: 1,
        counter: 1,
        speed: 4,
        songpos: 0,
        sub_start: 0,
        sub_end: 0,
        v: [Voice::default(); 3],
        mixer: 0x3B,
        noise_reg: 0,
        d: Digital { state: 0, start: 0, len1: 1, lp: 0, len2: 1, per: 0x240, vol: Some(0x40) },
        period,
        note_args,
        state: [0; 3],
        running: [false; 4],
        a_vol: 0,
        level: 0,
        songend: false,
    };
    p.init_sound(1);
    Some(Box::new(p))
}

/// The companion sample file's names: the module's name with the leading `hst.` or `sdc.` replaced
/// by `smp.`, then `smp.set` in the same directory.
fn companion(file: &str) -> Vec<String> {
    let name = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    if name.len() > 4 && name.is_char_boundary(4) {
        let head = &name[..4];
        if head.eq_ignore_ascii_case("hst.") || head.eq_ignore_ascii_case("sdc.") {
            v.push(format!("SMP.{}", &name[4..]));
        }
    }
    v.push("SMP.set".to_string());
    v
}

#[cfg(test)]
#[path = "jochenhippelst_tests.rs"]
mod tests;
