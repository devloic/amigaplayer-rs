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
//! The write values and their order follow the specification. The write times are counted in
//! 68000 cycles from the tick's start, as the reference (the original player on this project's
//! 68000 engine, where every instruction has a fixed cost) spends them: the Paula conversion is
//! exact (its waits for the beam, its looks at the interrupt requests and the fixed costs between
//! its writes were measured with the reference's register reads logged; fed the reference's song
//! start and first event of each tick it gives 396,026 of 396,070 writes of 347 modules at the
//! same colour clock). The music code's time before it is a sum of measured costs of the code
//! paths a tick takes (`COST`), and the beam position at the song start comes from the length of
//! the original's start-up, which depends on the module (`reference_start`). All of it is from
//! counts of executed addresses and their cycles, related to the module's data and the port's
//! own state: all 962,425 writes of 330 modules (5 s each) are at the reference's colour
//! clock, counted from the song start.
//!
//! Embedded data: `jochenhippelst_data.bin` (52 bytes) is the volume table together with the bytes
//! that follow it in the original player, which a volume index above 17 reads. It is taken from the
//! original player on the project owner's decision; the 18 table entries are written out in the
//! specification and the rest are the bytes real modules reach. `jochenhippelst_noise.bin` (1024
//! bytes) is the noise buffer and `PULSE` the 4-byte pulse, both read from the original player's
//! memory while it plays (it fills them at its start-up, the same bytes in every run, and does not
//! change them afterwards), also on the project owner's decision: with them a tone or the noise is
//! the reference's sample data byte for byte.
//!
//! Choices where the specification is silent (all safe, none can panic or hang):
//! * the prefix's pointer to the module is found as the first PC-relative address load in the first
//!   128 words whose target holds a module tag; failing that the module is not recognised;
//! * a volume index past the 52 known values reads as 0;
//! * a digital sample number past the sample table reads the bytes that follow it in the module
//!   (zero past the module's end), and the sample command plays what the bytes at the table's
//!   place say when no table is there, as the original does;
//! * a pitch or volume program is run for a bounded number of steps per tick (4096);
//! * a command of the pitch program's set in the volume program ends the voice's update for the
//!   tick as the original's does (see `volume_program`); a byte from $E9 on, which the original
//!   crashes on, stops the program; a table entry that points outside the image reads zeros
//!   (the memory around the original's image);
//! * the two-file (companion sample) module path follows the description but is not verified
//!   against the reference.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Jochen Hippel ST", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// The volume table (18 values) and the 34 bytes after it.
static VOL_TABLE: &[u8] = include_bytes!("jochenhippelst_data.bin");
/// The noise buffer (1024 bytes) as the original player holds it while it plays: the same bytes in
/// every run and for every module, never changed after the player's start-up.
static NOISE: &[u8] = include_bytes!("jochenhippelst_noise.bin");
/// The pulse: a square wave of two samples per half period, as the original's.
const PULSE: [u8; 4] = [0xB2, 0xB2, 0x4D, 0x4D];

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


/// The Paula conversion's times, in 68000 cycles (two are one colour clock), measured on the
/// reference with its beam reads logged; every one of them is the same in all its runs.
///
/// From a note start's first length write to the first beam read of its second wait, by source.
const LEN_TO_WAIT: [u64; 4] = [129, 199, 191, 160];
/// From a note start's last write to the volume write, by channel (A, B, C) and by what was
/// started (tone, noise, the silent sample; C's silent case was never seen: taken as B's step).
const LEN_TO_VOL: [[u64; 3]; 3] = [[324, 378, 362], [445, 499, 483], [410, 464, 448]];
/// From a channel's period write to the next channel's first event (its volume write, or the
/// first beam read of its note start), for B and C, by the next channel's case (tone held, noise
/// held, tone started, noise started, silent; C's silent case was never seen: taken as B's step).
const HEAD: [[u64; 5]; 2] = [[800, 871, 475, 492, 447], [835, 906, 545, 562, 517]];
/// Colour clocks of a raster line.
const LINE: u64 = 227;
/// From the end of the music code to channel A's first event, by its case (tone held, noise
/// held, tone started, noise started, silent), with the tick's entry code.
const HEAD_A: [u64; 5] = [866, 937, 662, 679, 978];
/// From the original's call of the host at the song's end to the end of the music code, less
/// the paths counted after the call (measured on 193 song ends: one value in every build).
const END_CALL: i64 = 2214;

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
    /// A table is there (its first word says so). Without one the sample command still plays
    /// what the bytes at the table's place say.
    valid: bool,
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
    /// The 68000 cycles the original's look through the prefix takes (see `reference_start`).
    scan: u64,
}

/// A parsed module: the COSO image the player works on.
struct Module {
    /// The image. What is read outside it is zero, as the memory around the original's image is.
    b: Vec<u8>,
    /// Offset of the image in the file, for a COSO image (its guest address is the module's plus
    /// this); None for an image built from a raw module.
    in_file: Option<usize>,
    short: bool,
    mmme: bool,
    /// Which row of `COST` the module's ticks take: the table width for a COSO file, the
    /// prefix code's word for a raw module.
    new_type: bool,
    /// The command set of the pitch and volume programs (new or old): the prefix code's own word
    /// on the type (a file without prefix: 16-bit tables). A COSO file behind a prefix without
    /// the jump has 16-bit tables and the old set: its `$EA` is the sample command.
    ext_new: bool,
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
    /// Offset in the file of a raw module's sample table place (whether a table is there or not).
    raw_dig: Option<usize>,
    /// The sample table is in a companion file.
    lsmp: bool,
    prefix: Prefix,
    /// The 68000 cycles of the original's check of this module and of its conversion of a raw
    /// module (see `reference_start`).
    check_cycles: u64,
    conv_cycles: u64,
}

fn tag_at(d: &[u8], o: usize) -> Option<[u8; 4]> {
    d.get(o..o + 4).map(|b| [b[0], b[1], b[2], b[3]])
}

/// The offset of the module's tag: 0, or the target of the first PC-relative address load of the
/// first 128 words that points at one; with it the number of words looked at to find it.
fn find_tag(d: &[u8]) -> Option<(usize, usize)> {
    let is_tag = |o: usize| matches!(tag_at(d, o), Some(t) if &t == b"COSO" || &t == b"TFMX" || &t == b"MMME");
    if is_tag(0) {
        return Some((0, 0));
    }
    for i in 0..128 {
        let w = be16(d, 2 * i);
        if w & 0xF1FF == 0x41FA {
            let t = 2 * i as i64 + 2 + be16(d, 2 * i + 2) as i16 as i64;
            if t > 0 && is_tag(t as usize) {
                return Some((t as usize, i + 1));
            }
        }
    }
    None
}

/// What the prefix code holds: the words before the tag, up to the indirect jump when there is
/// one (the original looks no further: the time it takes says so).
fn scan_prefix(d: &[u8], tag: usize) -> Prefix {
    let mut p = Prefix::default();
    let end = tag.min(d.len()) & !1;
    let mut i = 0;
    let mut timers = 0;
    while i < end && !p.jump {
        let w = be16(d, i);
        p.scan += 178;
        if w == 0x4EFB {
            p.jump = true;
        }
        if matches!(w, 0xFA23 | 0xFA1F | 0xFA25 | 0xFA21) {
            timers += 1;
            if p.timer.is_none() && i > 0 {
                p.timer = Some(d[i - 1]);
            }
        }
        if i + 4 <= end {
            let l = be32(d, i);
            if l == 0x0003_C686 {
                p.kind |= 2;
                p.scan += 8;
            }
            if l == 0x4840_49FA {
                p.kind |= 1;
                p.scan += 8;
            }
        }
        i += 2;
    }
    // each timer word found, the timer value's divisions (once), the loop's way out
    p.scan += 25 * timers + if timers > 0 { 290 } else { 0 };
    p.scan = if p.jump { p.scan - 15 } else { p.scan + 4 };
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

/// The 68000 cycles the original's trim of a 64-byte block takes: its look back from the end
/// for the last end or jump command, then the copy of what is kept.
fn trim_cycles(b: &[u8]) -> u64 {
    let kept = trim_block(b).len() as u64;
    let back = match b.iter().rposition(|&x| x == 0xE1 || x == 0xE0) {
        Some(i) => 54 * (b.len() - 1 - i) as u64 + if b[i] == 0xE1 { 23 } else { 48 },
        None => 54 * b.len() as u64 + 14,
    };
    166 + back + 22 * kept
}

fn parse(d: &[u8]) -> Option<Module> {
    let (tag, words) = find_tag(d)?;
    let h = &d[tag..];
    let prefix = if tag > 0 { scan_prefix(d, tag) } else { Prefix::default() };
    let mut m = match tag_at(h, 0)? {
        t if &t == b"COSO" => parse_coso(h, tag, prefix),
        t if &t == b"TFMX" || &t == b"MMME" => parse_raw(h, tag, prefix),
        _ => None,
    }?;
    m.ext_new = if tag > 0 { prefix.jump } else { m.in_file.is_some() && m.short };
    // the original's check: the look for the tag, and for a TFMX module the first byte of
    // every pitch program
    let coso = m.in_file.is_some() as u64;
    m.check_cycles = 245 + 174 * coso;
    if tag > 0 {
        m.check_cycles += 135 + 39 * words as u64;
    }
    if !m.mmme {
        let n = m.n_fs.min(0x400);
        let e2 = (0..n).filter(|&i| m.g(m.entry(m.t_fs, i) as i64) == 0xE2).count() as u64;
        m.check_cycles += 152 + (85 + 39 * coso) * n as u64 + 21 * e2 - 96 * coso;
    }
    Some(m)
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
    let m = Module {
        b: h.to_vec(),
        in_file: Some(0),
        short,
        mmme,
        new_type: short,
        ext_new: false,
        n_fs: be16(h, 0x24) as usize + 1,
        n_inst: be16(h, 0x26) as usize + 1,
        t_fs,
        t_inst,
        t_pat,
        t_pos,
        t_sub,
        n_sub: be16(h, 0x30) as usize,
        dig_file: Some(tag + dpos),
        raw_dig: None,
        lsmp: false,
        prefix,
        check_cycles: 0,
        conv_cycles: 0,
    };
    // the Amiga flavour of the format starts its pitch programs with a command that the ST
    // flavour has other uses for: more of those than the opposite shape and it is not ours
    let (mut p, mut n) = (0, 0);
    for i in 0..m.n_fs.min(0x400) {
        let o = m.entry(m.t_fs, i);
        if m.g(o as i64) == 0xE2 {
            if m.g(o as i64 + 1) < 0x80 {
                p += 1;
            } else {
                n += 1;
            }
        }
    }
    (n >= p).then_some(m)
}

/// The empty instrument (and pitch program) the player starts from: it is the player's own data,
/// not part of the image, and is read at this offset.
const NULL_AT: usize = 0x7000_0000;
const NULL_STREAM: [u8; 8] = [1, 0, 0, 0, 0, 0, 0, 0xE1];
/// Where a table entry that cannot be an offset into the image points: memory that reads as zero.
const OUTSIDE: usize = 0x6000_0000;

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
    let new_type = prefix.jump;
    // the image's tables hold 16-bit offsets, whatever the command set
    let es = 2;
    // the image: header, three tables, the trimmed blocks and compact patterns, then the lists
    let mut b = vec![0u8; 0x40];
    b[..4].copy_from_slice(b"COSO");
    b[0x20..0x40].copy_from_slice(&h[..0x20]);
    let put_off = |b: &mut Vec<u8>, at: usize, v: usize| b[at..at + 2].copy_from_slice(&(v as u16).to_be_bytes());
    // each table is followed by the data its entries point at
    // the 68000 cycles the original's conversion takes, counted along (see `reference_start`)
    let mut cycles = 1567 + 70 * npos as u64 + 46 * (w16 as u64 + 1) + 50 * (w18 as u64 + 1);
    let t_fs = b.len();
    b.resize(t_fs + es * nfs, 0);
    for i in 0..nfs {
        let at = b.len();
        put_off(&mut b, t_fs + es * i, at);
        let block = &h[0x20 + 64 * i..0x20 + 64 * i + 64];
        b.extend_from_slice(trim_block(block));
        cycles += trim_cycles(block);
    }
    // the next table starts on an even offset, as in the COSO files
    if b.len() & 1 != 0 {
        b.push(0);
        cycles += 12;
    }
    let t_inst = b.len();
    b.resize(t_inst + es * ninst, 0);
    for i in 0..ninst {
        let at = b.len();
        put_off(&mut b, t_inst + es * i, at);
        let s = 0x20 + 64 * (nfs + i);
        b.extend_from_slice(trim_block(&h[s..s + 64]));
        cycles += trim_cycles(&h[s..s + 64]);
    }
    if b.len() & 1 != 0 {
        b.push(0);
        cycles += 12;
    }
    let t_pat = b.len();
    b.resize(t_pat + es * npat, 0);
    for i in 0..npat {
        let at = b.len();
        put_off(&mut b, t_pat + es * i, at);
        let s = o_pat + i * pat_size;
        cycles += compact_pattern(h, s, pat_size, &mut b);
        if b.len() > MAX_IMAGE {
            return None;
        }
    }
    if b.len() & 1 != 0 {
        b.push(0);
        cycles += 12;
    }
    let t_pos = b.len();
    b.extend_from_slice(&h[o_pos..o_sub]);
    let t_sub = b.len();
    b.extend_from_slice(&h[o_sub..o_ext]);
    let t_ext = b.len();
    b.extend_from_slice(&h[o_ext..o_dig]);
    // the image ends here: the sample table stays in the module (the seventh offset is the
    // distance to it, which depends on where the image is in memory: `new` fills it in); without
    // a sample table the seventh offset is the image's end
    let t_end = b.len();
    for (o, v) in [(4, t_fs), (8, t_inst), (12, t_pat), (0x10, t_pos), (0x14, t_sub), (0x18, t_ext), (0x1C, t_end)] {
        b[o..o + 4].copy_from_slice(&(v as u32).to_be_bytes());
    }
    Some(Module {
        b,
        in_file: None,
        short: true,
        mmme,
        new_type,
        ext_new: false,
        n_fs: nfs,
        n_inst: ninst,
        t_fs,
        t_inst,
        t_pat,
        t_pos,
        t_sub,
        n_sub: w16,
        dig_file: (!lsmp).then_some(tag + o_dig),
        raw_dig: matches!(be16(h, o_dig), 0x80 | 0x100).then_some(tag + o_dig),
        lsmp,
        prefix,
        check_cycles: 0,
        conv_cycles: cycles,
    })
}

/// Re-encodes a raw pattern (2-byte rows, a third byte before the row when the flags ask for one)
/// as the compact row stream: waits as row speeds, rows, the end marker. Returns the 68000
/// cycles the original takes for it (see `reference_start`).
fn compact_pattern(h: &[u8], start: usize, size: usize, out: &mut Vec<u8>) -> u64 {
    let rows = size / 2;
    let byte = |o: isize| -> u8 { if o < 0 { 0 } else { h.get(o as usize).copied().unwrap_or(0) } };
    let note = |i: usize| byte(start as isize + 2 * i as isize);
    let end = (0..rows).find(|&i| note(i) == 1).unwrap_or(rows);
    let real: Vec<usize> = (0..end).filter(|&i| note(i) != 0).collect();
    // the cycles: per pattern, per empty row before the first note, per note and per empty row
    // after it, and what each of the cases below adds
    let mut cycles = 262 + 21 * (end < rows) as u64;
    match real.first() {
        None => {
            if end > 0 {
                out.extend_from_slice(&[0xFD, (end - 1) as u8]);
            }
            cycles = 219 + 63 * end as u64 + 21 * (end < rows) as u64;
        }
        Some(&r0) => {
            // the leading wait sets the speed as well
            let mut prev = None;
            if r0 > 0 {
                out.extend_from_slice(&[0xFD, (r0 - 1) as u8]);
                prev = Some((r0 - 1) as u8);
                cycles += 32 + 63 * r0 as u64;
            }
            for (k, &i) in real.iter().enumerate() {
                let next = real.get(k + 1).copied().unwrap_or(end);
                let d = (next - i - 1) as u8;
                let last = k + 1 == real.len();
                cycles += 124 + 67 * (next - i - 1) as u64;
                // the speed is stated only where it changes
                if prev != Some(d) {
                    out.extend_from_slice(&[0xFE, d]);
                    prev = Some(d);
                    cycles += if last { 24 } else { 105 };
                } else if !last {
                    cycles += 81;
                }
                let flags = byte(start as isize + 2 * i as isize + 1);
                out.push(note(i));
                out.push(flags);
                if flags & 0xE0 != 0 {
                    // the byte before the row; the first row's is the pattern's last byte
                    let before = if i == 0 { size - 1 } else { 2 * i - 1 };
                    out.push(byte((start + before) as isize));
                    cycles += if i == 0 { 43 } else { 27 };
                }
            }
        }
    }
    out.push(0xFF);
    cycles
}

fn detect(d: &[u8]) -> bool {
    parse(d).is_some()
}

impl Module {
    fn g(&self, o: i64) -> u8 {
        if o < 0 {
            0
        } else if o >= NULL_AT as i64 {
            NULL_STREAM.get((o - NULL_AT as i64) as usize).copied().unwrap_or(0)
        } else {
            self.b.get(o as usize).copied().unwrap_or(0)
        }
    }

    /// The offset a table entry points at (`OUTSIDE` when it is not in the image: the memory
    /// around the original's image is zero).
    fn entry(&self, tab: usize, i: usize) -> usize {
        let o = if self.short {
            let v = be16(&self.b, tab + 2 * i) as i16;
            if v < 0 {
                return OUTSIDE;
            }
            v as usize
        } else {
            be32(&self.b, tab + 4 * i) as usize
        };
        if o < self.b.len() { o } else { OUTSIDE }
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
    /// The period as computed, before the pitch-end byte silences it: what the SID voice takes.
    sid_per: u16,
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

/// The code paths of a tick that are counted for the music code's time (see `COST`). Two are
/// columns of the table that nothing counts any more.
#[derive(Clone, Copy)]
#[repr(usize)]
#[allow(dead_code)]
enum F {
    FsWait, FsNote, FsNoteHi, FsE0, FsE1, FsE2, FsE3, FsE4, FsE5, FsE6, FsE7, FsE8Ret, FsE8Go, FsE9, FsEaNew, FsDig, FsDigNone, FsDigPast,
    FsDigM0, FsDigM10, FsDigEnd, FsDigNote, FsDigNoteHi, FsDigNoteOff, FsEb, FsEd, FsEe, FsEf,
    VsHold, VsWait, VsVal, VsE0, VsE1, VsE8Ret, VsE8Go, VsOther,
    PmNoteHi, PmMode0, PmMode1, PmMode1Hi, PmMode2, PmNoise, PmDelay, PmHold, PmDown, PmDownClamp, PmUp, PmUpClamp, PmDepthHi, PmShift, PmShiftAny,
    PmPorta, PmEnd, PmVolNeg, PmPerZero,
    SidOff, SidStop, SidRange, SidPlay, SidPlay4, SidSrc2, SidSrc3,
    TrIdle, TrRowIdle, TrFf, TrFf0, TrEnd, TrPosF, TrPosE, TrFe, TrFd, TrRow, TrRowParam, TrRowHi, TrNote, TrNote40, TrInstPast, TrProgPast,
    PaTone, PaToneNew, PaNoise, PaNoiseNew, PaSilent,
    // what the module is (the same in every tick): a COSO file; one with digital samples; one
    // with note arguments in the sample command; the type the prefix code gives
    GCoso, GDig, GArgs, GKind1, GKind2, GKind3,
    VsE1Store, VsE0Mask, FsE1Store, FsE0Mask, SidPlay5, SidRangeHi, TrNoteLong, TrFfLong, FsStepCoso, PmEndAny, PmEnd8, PmEnd4, PmEnd1, PmDeltaNeg, FsE2Ext, FsStepOld, FsStepLtEb, FsStepLtEbNoDig, FsStepGeF0, SidRange10, FsE7Old, FsDigOld,
    N,
}

/// The 68000 cycles from a tick's start to channel A's first event (its volume write, or the
/// first beam read of its note start), as a sum over the code paths the tick took: one row per
/// build of the original (old or new command set, without or with the MMME pitch code), one cost
/// per path of `F`, then a constant (and an unused column). The reference's cycles per tick were
/// measured from the addresses it executes and solved for whole-cycle costs of these paths:
/// they give the reference's count on every tick measured (66,716 ticks of 330 modules). A cost
/// is the sum over the addresses a path adds, so it can be negative (a path that skips more than
/// it runs). Not measured, and taken as the other build's step: the SID section's leave on a
/// period above $EEE in the build without the MMME code (17 cycles after the leave on a period
/// of $10 or less), and the old set's `$E7` and sample command in a COSO file with 16-bit
/// tables and the MMME code (`FsE7Old`, `FsDigOld`: measured without the MMME code only).
#[rustfmt::skip]
const COST: [[i32; F::N as usize + 2]; 4] = [
    [-187, -93, 0, 183, -62, 185, 150, 146, 134, 134, 165, 0, 187, 126, 0, 444, 0, 0, 1, 14, 0, 0, 66, 0, 0, 0, 0, 0, -1, 0, 113, 160, 142, 206, 207, 0, -24, -7, 67, 4, 0, 36, -168, 0, -4, 26, 0, 18, 0, 23, 0, 88, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, -2266, -678, 321, 25, 132, 17, 24, 86, -533, 0, 16, -485, 0, 12, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 67, 19, 0, 0, 0, 0, 5108, 0],
    [-187, -93, 0, 183, -62, 185, 150, 146, 134, 134, 165, 0, 187, 126, 0, 444, 0, 0, 1, 14, 0, 0, 66, 0, 0, 0, 0, 0, -1, 0, 113, 160, 142, 206, 207, 0, -24, -7, 67, 4, 0, 36, -204, 0, -4, 26, 0, 16, 0, 0, 0, 156, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, -2266, -678, 321, 25, 132, 17, 24, 86, -533, 0, 16, -485, 0, 12, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 36, 18, 0, 0, 0, 0, 0, 0, 0, 0, 67, 19, 0, 0, 0, 0, 5279, 0],
    [-206, -93, 0, 194, -62, 204, 169, 165, 153, 153, 244, 0, 206, 145, 196, 436, 0, 0, 1, 14, 0, 318, 294, -228, 0, 157, 153, 0, -1, 0, 113, 152, 142, 206, 207, 0, -24, -7, 67, 4, 0, 36, -168, 0, -4, 26, 0, 18, 0, 23, 0, 88, 0, 4, 0, -363, -306, -221, -51, -146, 5, 12, -2266, -678, 321, 25, 132, 17, 24, 86, -533, 0, 16, -485, 0, 12, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 30, 8, 30, 8, 0, -204, 0, 0, 0, 0, 0, 0, 0, 8, -14, 48, 0, 19, -19, 0, -60, 27, 5528, 0],
    [-206, -93, 0, 194, -62, 204, 169, 165, 153, 153, 244, 0, 206, 145, 196, 436, 0, 0, 1, 14, 0, 318, 294, -228, 153, 157, 153, 0, -1, 0, 113, 152, 142, 206, 207, 0, -24, -7, 67, 4, 0, 36, -204, 0, -4, 26, 0, 16, 0, 0, 0, 156, 4, 4, 0, -363, -306, -221, -51, -146, 5, 12, -2266, -678, 321, 25, 132, 17, 24, 86, -533, 0, 16, -485, 0, 12, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 30, 8, 30, 8, 0, -204, 0, 0, 0, 87, -19, 0, 0, 0, -14, 67, 0, 0, -19, 0, -60, 27, 5699, 0],
];

pub struct JochenHippelSt {
    f: [u32; F::N as usize],
    /// The paths counted up to the song's end, in the tick that has it (see `END_CALL`).
    f_end: Option<[u32; F::N as usize]>,
    /// The reference's clock at the song start (see `reference_start`).
    ref_start: u64,
    /// The clock at the tick's start and the 68000 cycles since.
    t0: u64,
    cy: u64,
    /// What to add to the clock to have the reference's beam position.
    beam: u64,
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
    first_tick_pending: bool,
}

impl JochenHippelSt {
    fn init_sound(&mut self, n: u16) {
        self.sub = n;
        let e = self.m.t_sub + 6 * (n as usize).saturating_sub(1);
        let (mut start, mut end, mut speed) = (be16(&self.m.b, e), be16(&self.m.b, e + 2), be16(&self.m.b, e + 4));
        if speed == 0 {
            // the default speed is written into the table (a voice whose positions run past the
            // list reads it as a transpose)
            speed = 4;
            if e + 6 <= self.m.b.len() {
                self.m.b[e + 4..e + 6].copy_from_slice(&[0, 4]);
            }
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
            v.inst = NULL_AT;
            v.fs = NULL_AT;
            // a voice starts as after the tone-only command
            v.mode = 2;
            // and, with the MMME pitch code, with its vibrato going up
            v.vib_flag = if self.m.mmme { 0x20 } else { 0 };
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
        self.m.entry(self.m.t_pat, n)
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
        let r = self.volume_program(i);
        if let Some(c) = r {
            self.v[i].ym_per = c as u16;
            self.v[i].sid_per = c as u16;
            self.v[i].ym_vol = 0;
            return;
        }
        self.period_mixer(i);
    }

    fn pitch_program(&mut self, i: usize) {
        if self.v[i].fs_wait != 0 {
            self.v[i].fs_wait -= 1;
            self.f[F::FsWait as usize] += 1;
            return;
        }
        let new = self.m.ext_new;
        let has_dig = self.dig.as_ref().is_some_and(|d| d.valid);
        for _ in 0..4096 {
            let v = self.v[i];
            let at = v.fs as i64 + v.fs_idx as i64;
            let c = self.m.g(at);
            if c >= 0xF0 {
                self.f[F::FsStepGeF0 as usize] += 1;
            }
            if !self.m.ext_new {
                self.f[F::FsStepOld as usize] += 1;
                if c < 0xEB {
                    self.f[F::FsStepLtEb as usize] += 1;
                    if !has_dig {
                        self.f[F::FsStepLtEbNoDig as usize] += 1;
                    }
                }
            } else if c == 0xE2 {
                self.f[F::FsE2Ext as usize] += 1;
            }
            if c == 0xE1 && self.m.ext_new {
                self.f[F::FsE1Store as usize] += 1;
            }
            if c == 0xE0 && !self.m.ext_new {
                self.f[F::FsE0Mask as usize] += 1;
            }
            let arg = self.m.g(at + 1);
            // in the old command set the bytes from $EB on are notes, and $EA is one when
            // the module has no digital samples
            let is_cmd = (0xE0..=0xEF).contains(&c) && (new || c < 0xEB) && !(c == 0xEA && !new && !has_dig);
            if !is_cmd {
                let v = &mut self.v[i];
                v.note = c;
                v.fs_idx += 1;
                self.f[F::FsNote as usize] += 1;
                if c & 0x80 != 0 {
                    self.f[F::FsNoteHi as usize] += 1;
                }
                return;
            }
            self.f[match c {
                0xE0 => F::FsE0,
                0xE1 => F::FsE1,
                0xE2 => F::FsE2,
                0xE3 => F::FsE3,
                0xE4 => F::FsE4,
                0xE5 => F::FsE5,
                0xE6 => F::FsE6,
                0xE7 => F::FsE7,
                0xE8 => if arg != 0 { F::FsE8Ret } else { F::FsE8Go },
                0xE9 => F::FsE9,
                0xEA if new => F::FsEaNew,
                0xEA | 0xEC => F::FsDig,
                0xEB => F::FsEb,
                0xED => F::FsEd,
                0xEE => F::FsEe,
                _ => F::FsEf,
            } as usize] += 1;
            if !new && c == 0xE7 {
                self.f[F::FsE7Old as usize] += 1;
            }
            if !new && c == 0xEA {
                self.f[F::FsDigOld as usize] += 1;
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
                    v.vs_wait = 1;
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
        let Some(dig) = &self.dig else {
            self.f[F::FsDigNone as usize] += 1;
            return;
        };
        let start = dig.start(n) as u32;
        let mark = dig.mark(n);
        let addr = dig.base.wrapping_add(start);
        self.f[match mark {
            0 => F::FsDigM0,
            0x10 => F::FsDigM10,
            _ => F::FsDigEnd,
        } as usize] += 1;
        if self.note_args {
            self.f[if note_arg & 0x80 != 0 { F::FsDigNoteHi } else { F::FsDigNote } as usize] += 1;
        }
        let (len1, lp, len2) = match mark {
            0 => (((dig.start(n + 1) as u32).wrapping_sub(start) / 2) as u16, self.silent, 1),
            0x10 => (8, addr, 8),
            end => (((end as u32).wrapping_sub(start) / 2) as u16, self.silent, 1),
        };
        let mut per = self.period;
        if self.note_args {
            let v = &self.v[i];
            // a note with bit 7 set is absolute, as in the pitch program
            let n = if note_arg & 0x80 == 0 { note_arg.wrapping_add(v.row_note).wrapping_add(v.ta) } else { note_arg };
            let idx = (n & 0x7F) as usize;
            if idx >= 96 {
                self.f[F::FsDigNoteOff as usize] += 1;
            }
            if idx < 96 {
                let t = digital_note(idx);
                let q = (self.period as u32 * 0x2000) / (t / 8).max(1);
                per = (q as u16).wrapping_add(1);
            }
        }
        self.d = Digital { state: 1, start: addr, len1, lp, len2, per, vol: Some(64) };
    }

    /// Runs the volume program. Returns the command byte when the program holds one of the pitch
    /// program's commands that the volume program has no use for ($E2 to $E7): the original then
    /// leaves the voice's update, with that byte as the voice's period and a zero volume.
    fn volume_program(&mut self, i: usize) -> Option<u8> {
        let new = self.m.ext_new;
        let v = &mut self.v[i];
        if v.vs_hold != 0 {
            v.vs_hold -= 1;
            self.f[F::VsHold as usize] += 1;
            return None;
        }
        v.vs_wait = v.vs_wait.wrapping_sub(1);
        if v.vs_wait != 0 {
            self.f[F::VsWait as usize] += 1;
            return None;
        }
        v.vs_wait = v.vs_speed;
        for _ in 0..4096 {
            let at = v.inst as i64 + 5 + v.vs_idx as i64;
            let c = self.m.g(at);
            self.f[match c {
                0..=0xDF => F::VsVal,
                0xE0 => F::VsE0,
                0xE1 => F::VsE1,
                0xE8 => if self.m.g(at + 1) != 0 { F::VsE8Ret } else { F::VsE8Go },
                _ => F::VsOther,
            } as usize] += 1;
            match c {
                0..=0xDF => {
                    v.volume = c;
                    v.vs_idx += 1;
                    return None;
                }
                0xE0 => {
                    let n = self.m.g(at + 1);
                    if !new {
                        self.f[F::VsE0Mask as usize] += 1;
                    }
                    let n = if new { n } else { n & 0x3F };
                    v.vs_idx = n as i32 - 5;
                }
                0xE1 => {
                    if new {
                        self.f[F::VsE1Store as usize] += 1;
                        v.volume = self.m.g(at - 1);
                    }
                    return None;
                }
                0xE8 => {
                    v.vs_hold = self.m.g(at + 1);
                    v.vs_idx += 2;
                    if v.vs_hold != 0 {
                        v.vs_hold -= 1;
                        return None;
                    }
                    // a zero hold goes back to the speed counter (just reloaded)
                    v.vs_wait = v.vs_wait.wrapping_sub(1);
                    if v.vs_wait != 0 {
                        return None;
                    }
                    v.vs_wait = v.vs_speed;
                }
                0xE2..=0xE7 => return Some(c),
                _ => return None,
            }
        }
        None
    }

    fn period_mixer(&mut self, i: usize) {
        let v = self.v[i];
        let n = if v.note & 0x80 == 0 { v.note.wrapping_add(v.row_note).wrapping_add(v.ta) } else { v.note } & 0x7F;
        let p0 = YM_PERIODS.get(n as usize).copied().unwrap_or(0);
        let (tone, nz) = (1u8 << i, 8u8 << i);
        let mut noise = v.noise;
        if v.note & 0x80 != 0 {
            self.f[F::PmNoteHi as usize] += 1;
        }
        self.f[match v.mode {
            0 => F::PmMode0,
            1 => F::PmMode1,
            _ => F::PmMode2,
        } as usize] += 1;
        if v.mode == 1 && v.note & 0x80 != 0 {
            self.f[F::PmMode1Hi as usize] += 1;
        }
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
            self.f[F::PmNoise as usize] += 1;
        }
        let mut per = p0 as i32;
        let mmme = self.m.mmme;
        let vm = &mut self.v[i];
        if vm.vib_delay != 0 {
            vm.vib_delay -= 1;
            self.f[F::PmDelay as usize] += 1;
        } else if !mmme {
            let depth = if vm.vib_depth & 0x80 == 0 { vm.vib_depth as i32 * 2 } else { (vm.vib_depth & 0x7F) as i32 };
            if vm.vib_depth & 0x80 != 0 {
                self.f[F::PmDepthHi as usize] += 1;
            }
            let mut pos = vm.vib_pos as i32;
            let hold = vm.vib_flag & 0x81 == 0x81;
            if hold {
                self.f[F::PmHold as usize] += 1;
            }
            if !hold {
                if vm.vib_flag & 0x20 == 0 {
                    pos -= vm.vib_speed as i32;
                    self.f[F::PmDown as usize] += 1;
                    if pos < 0 {
                        vm.vib_flag |= 0x20;
                        pos = 0;
                        self.f[F::PmDownClamp as usize] += 1;
                    }
                } else {
                    pos += vm.vib_speed as i32;
                    self.f[F::PmUp as usize] += 1;
                    if pos >= depth {
                        vm.vib_flag &= !0x20;
                        pos = depth;
                        self.f[F::PmUpClamp as usize] += 1;
                    }
                }
                vm.vib_pos = pos as u8;
            }
            let mut delta = pos - depth / 2;
            if delta < 0 {
                self.f[F::PmDeltaNeg as usize] += 1;
            }
            let mut k = 2 * n as u32 + 0xA0;
            if k < 0x100 {
                self.f[F::PmShiftAny as usize] += 1;
                loop {
                    self.f[F::PmShift as usize] += 1;
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
                self.f[F::PmDown as usize] += 1;
                if pos < 0 {
                    vm.vib_flag |= 0x20;
                    pos = 0;
                    self.f[F::PmDownClamp as usize] += 1;
                }
            } else {
                pos += vm.vib_speed as i32;
                self.f[F::PmUp as usize] += 1;
                if pos > depth {
                    vm.vib_flag &= !0x20;
                    pos = depth;
                    self.f[F::PmUpClamp as usize] += 1;
                }
            }
            vm.vib_pos = pos as u8;
            let delta = pos - vm.vib_depth as i32;
            per = (p0 as i32) + ((delta * p0 as i32) >> 10);
        }
        if vm.row_flags & 0x20 != 0 {
            self.f[F::PmPorta as usize] += 1;
            let sp = vm.row_param as i8 as i32;
            if !mmme {
                vm.porta = vm.porta.wrapping_add(sp << 12);
                per -= (vm.porta >> 16) as i32;
            } else {
                vm.porta = vm.porta.wrapping_add(sp) & 0xFFFF;
                let acc = vm.porta as i16 as i32;
                per -= (acc * per) >> 10;
            }
        }
        let mut per = per as u16;
        vm.sid_per = per;
        if mmme && vm.pitch_end != 0 {
            self.f[F::PmEndAny as usize] += 1;
            for (bit, k) in [(8, F::PmEnd8), (4, F::PmEnd4), (1, F::PmEnd1)] {
                if vm.pitch_end & bit != 0 {
                    self.f[k as usize] += 1;
                }
            }
        }
        if mmme && vm.pitch_end & 2 != 0 {
            per = 0;
            self.f[F::PmEnd as usize] += 1;
        }
        vm.ym_per = per;
        let vol = vm.volume.wrapping_sub(vm.vol_red);
        if vol & 0x80 != 0 {
            self.f[F::PmVolNeg as usize] += 1;
        }
        if p0 == 0 {
            self.f[F::PmPerZero as usize] += 1;
        }
        vm.ym_vol = if vol & 0x80 != 0 { 0 } else { vol };
    }

    /// The digital voice's "SID" mode (after voice 0's update).
    fn sid(&mut self) {
        let mode = self.v[0].sid_src;
        if mode & 0x80 != 0 {
            self.f[F::SidOff as usize] += 1;
            return;
        }
        self.f[match mode {
            0 => F::SidStop,
            2 => F::SidSrc2,
            3 => F::SidSrc3,
            4 => F::SidPlay4,
            1 => F::SidPlay,
            _ => F::SidPlay5,
        } as usize] += 1;
        let src = match mode {
            0 => None,
            2 => Some(self.v[1].sid_per),
            3 => Some(self.v[2].sid_per),
            4 => Some(0),
            // voice 0's own is the period it plays (zero when its pitch-end byte silences it)
            _ => Some(self.v[0].ym_per),
        };
        let Some(p) = src else {
            self.v[0].sid_src = 0xFF;
            self.d.state = 0;
            return;
        };
        if mode != 4 {
            if p <= 0x10 || p > 0xEEE {
                // the original's first test is a signed one: a period of $8000 or more leaves
                // where one of $10 or less does, one above $EEE a test later
                self.f[if p as i16 <= 0x10 { F::SidRange } else { F::SidRangeHi } as usize] += 1;
                if p == 0x10 {
                    self.f[F::SidRange10 as usize] += 1;
                }
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
            self.f[F::TrIdle as usize] += 1;
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
            self.f[F::TrRowIdle as usize] += 1;
            return;
        }
        v.row_counter = v.row_speed;
        for _ in 0..64 {
            let at = self.v[i].pat as i64;
            let c = self.m.g(at);
            match c {
                0xFF => {
                    self.f[F::TrFf as usize] += 1;
                    if i == 0 {
                        self.f[F::TrFf0 as usize] += 1;
                        self.songpos -= 1;
                        if self.songpos < 0 {
                            self.f[F::TrEnd as usize] += 1;
                            self.f_end = Some(self.f);
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
                        0xF => {
                            v.vol_red = f & 0xF;
                            self.f[F::TrPosF as usize] += 1;
                        }
                        0xE => {
                            self.speed = f & 0xF;
                            self.f[F::TrPosE as usize] += 1;
                        }
                        _ => {}
                    }
                    if !self.m.short {
                        self.f[F::TrFfLong as usize] += 1;
                    }
                    self.v[i].pat = self.pattern(pat as usize);
                }
                0xFE | 0xFD => {
                    let n = self.m.g(at + 1);
                    self.f[if c == 0xFD { F::TrFd } else { F::TrFe } as usize] += 1;
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
                    self.f[F::TrRow as usize] += 1;
                    if flags & 0xE0 != 0 {
                        self.f[F::TrRowParam as usize] += 1;
                    }
                    self.f[if c & 0x80 != 0 { F::TrRowHi } else { F::TrNote } as usize] += 1;
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
        if !self.m.short {
            self.f[F::TrNoteLong as usize] += 1;
        }
        let tb = self.v[i].tb;
        let mut n = (flags & 0x1F).wrapping_add(tb) as usize;
        if n >= self.m.n_inst {
            n = 0;
            self.f[F::TrInstPast as usize] += 1;
        }
        if flags & 0x40 != 0 {
            self.f[F::TrNote40 as usize] += 1;
        }
        let inst = self.m.entry(self.m.t_inst, n);
        let g = |o: usize| self.m.g((inst + o) as i64);
        let (spd, pnum, vs, vd, vdl) = (g(0), g(1), g(2), g(3), g(4));
        let mut prog = if flags & 0x40 != 0 { self.v[i].row_param as usize } else { pnum as usize };
        if prog >= self.m.n_fs {
            prog = 0;
            self.f[F::TrProgPast as usize] += 1;
        }
        let fs = self.m.entry(self.m.t_fs, prog);
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

    /// Moves the clock to the tick's cycle `self.cy` (two 68000 cycles are one colour clock).
    fn now(&self, hw: &mut Hw) {
        hw.bus.clock = hw.bus.clock.max(self.t0 + self.cy / 2);
    }

    /// The original's wait for the beam, entered at cycle `self.cy` with its first read of the
    /// beam position: it reads again 12 cycles later and then every 21 until the line has changed,
    /// then every 25 cycles until the horizontal position is 22 or more. Leaves `self.cy` at the
    /// last read.
    fn beam_wait(&mut self) {
        let pos = |t: u64| self.t0 + t / 2 + self.beam;
        let mut t = self.cy;
        let line = pos(t) / LINE;
        t += 12;
        while pos(t) / LINE == line {
            t += 21;
        }
        t += 25;
        while pos(t) % LINE < 22 {
            t += 25;
        }
        self.cy = t;
    }

    /// The original's look at a channel's interrupt request, every 29 cycles from `self.cy`
    /// until it is set (one look only when `wait` is false); what follows comes 29 cycles after
    /// the last look.
    fn poll(&mut self, hw: &mut Hw, ibit: u16, wait: bool) {
        if wait {
            for _ in 0..20000 {
                let c = self.t0 + self.cy / 2;
                hw.bus.paula.sync(c.max(hw.bus.clock), &hw.bus.ram);
                if hw.bus.paula.intreq & ibit != 0 {
                    break;
                }
                self.cy += 29;
            }
        }
        self.cy += 29;
    }

    /// The note-start sequence of source `c`, its first beam read at `self.cy`: the channel is
    /// stopped, started on the first sample for one pass, and given its loop. Leaves `self.cy`
    /// at the last write (the loop's length).
    fn note_start(&mut self, hw: &mut Hw, c: usize, sa: u32, la: u32, l1: u16, l2: u16) {
        let (ch, mask) = CHAN[c];
        let ibit = mask << 7;
        let running = self.running[c];
        self.beam_wait();
        self.cy += 25;
        self.now(hw);
        hw.per(ch, 1);
        self.cy += 52;
        self.now(hw);
        Self::irq(hw, if running { ibit } else { 0 });
        self.cy += 12;
        self.now(hw);
        hw.dmacon(if running { mask } else { 0 });
        self.cy += 12;
        self.poll(hw, ibit, running);
        self.now(hw);
        hw.loc(ch, sa);
        self.cy += 12;
        self.now(hw);
        hw.len(ch, l1);
        self.cy += LEN_TO_WAIT[c];
        self.beam_wait();
        self.cy += 73;
        self.now(hw);
        Self::irq(hw, ibit);
        self.cy += 12;
        self.now(hw);
        hw.dmacon(0x8000 | mask);
        self.running[c] = true;
        self.cy += 12;
        self.poll(hw, ibit, true);
        self.beam_wait();
        self.cy += 21;
        self.now(hw);
        hw.loc(ch, la);
        self.cy += 12;
        self.now(hw);
        hw.len(ch, l2);
    }

    /// What a tone channel does this tick: 0 holds its tone, 1 holds its noise, 2 starts the
    /// tone, 3 starts the noise, 4 is silent (and starts the silent sample).
    fn channel_case(&self, c: usize) -> usize {
        let d7 = !self.mixer & 0x3F;
        if d7 & (1 << c) != 0 {
            if self.state[c] != 1 { 2 } else { 0 }
        } else if d7 & (8 << c) != 0 {
            if self.state[c] != 2 { 3 } else { 1 }
        } else {
            4
        }
    }

    /// The YM image as Paula writes; `self.cy` is the cycle of channel A's first event.
    fn paula(&mut self, hw: &mut Hw) {
        for c in 0..3 {
            let (ch, _) = CHAN[c];
            let case = self.channel_case(c);
            if c > 0 {
                self.cy += HEAD[c - 1][case];
            }
            let (per, vol);
            match case {
                0 | 2 => {
                    if case == 2 {
                        self.state[c] = 1;
                        self.note_start(hw, c, self.pulse, self.pulse, 2, 2);
                        self.cy += LEN_TO_VOL[c][0];
                    }
                    per = ((self.v[c].ym_per & 0xFFF) as u32 * 7 + 1) as u16;
                    vol = vol_table(self.v[c].ym_vol as usize);
                }
                1 | 3 => {
                    if case == 3 {
                        self.state[c] = 2;
                        self.note_start(hw, c, self.noise, self.noise, 0x200, 0x200);
                        self.cy += LEN_TO_VOL[c][1];
                    }
                    per = 0x280 - 0x10 * (self.noise_reg & 0x1F) as u16;
                    vol = vol_table(self.v[c].ym_vol as usize);
                }
                _ => {
                    self.note_start(hw, c, self.silent, self.silent, 1, 1);
                    self.cy += LEN_TO_VOL[c][2];
                    self.state[c] = 0;
                    per = 0x100;
                    vol = 0;
                }
            }
            self.now(hw);
            hw.vol(ch, vol);
            if c == 0 {
                self.a_vol = vol;
            }
            self.cy += 24;
            self.now(hw);
            hw.per(ch, per);
        }
        // the digital voice
        let d = self.d;
        if d.state == 1 && d.vol.is_none() {
            hw.bus.write_bytes(self.level_cell - 1, &[self.level, self.level]);
        }
        match d.state {
            0 => {
                self.cy += 314;
                self.note_start(hw, 3, self.silent, self.silent, 1, 1);
            }
            1 => {
                self.d.state = 2;
                self.cy += 385;
                self.note_start(hw, 3, d.start, d.lp, d.len1, d.len2);
            }
            _ => return,
        }
        self.cy += 40;
        self.now(hw);
        hw.per(1, d.per);
        self.cy += if d.vol.is_none() { 499 } else { 487 };
        self.now(hw);
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
        self.f = [0; F::N as usize];
        self.f_end = None;
        self.play();
        let k = [F::PaTone, F::PaNoise, F::PaToneNew, F::PaNoiseNew, F::PaSilent][self.channel_case(0)];
        self.f[k as usize] += 1;
        self.f[F::GCoso as usize] = self.m.in_file.is_some() as u32;
        self.f[F::GDig as usize] = self.dig.is_some() as u32;
        self.f[F::GArgs as usize] = self.note_args as u32;
        if let Some(k) = [None, Some(F::GKind1), Some(F::GKind2), Some(F::GKind3)][(self.m.prefix.kind & 3) as usize] {
            self.f[k as usize] = 1;
        }
        self.t0 = hw.bus.clock;
        let cost = &COST[2 * self.m.new_type as usize + self.m.mmme as usize];
        let mut cy = cost[F::N as usize] as i64 + HEAD_A[self.channel_case(0)] as i64;
        if self.first_tick_pending {
            // the beam's position is the reference's: its clock at the song start, one tick ago
            let own = self.t0.saturating_sub(TICK);
            self.beam = (self.ref_start + LINE * (own / LINE + 1) - own) % LINE;
            self.first_tick_pending = false;
        }
        for (n, c) in self.f.iter().zip(cost) {
            cy += *n as i64 * *c as i64;
        }
        if let Some(f) = self.f_end {
            // the original tells the host of the song's end from inside the tick, and the host
            // goes on at a whole colour clock: a call at an odd cycle loses that cycle
            let mut at = cost[F::N as usize] as i64 - END_CALL;
            for (n, c) in f.iter().zip(cost) {
                at += *n as i64 * *c as i64;
            }
            cy -= at & 1;
        }
        self.cy = cy.clamp(0, TICK as i64) as u64;
        self.paula(hw);
    }
}

/// The reference's clock at the song start, which gives the beam position its waits see. The
/// original's start-up is a chain of calls, each ending on a whole colour clock: the check, the
/// initialisation and the song's set-up, with short calls of constant length between. Their
/// lengths were measured (68000 cycles per executed address, counted per module and related to
/// the module's data) and are equal to the figures here on all 330 modules compared:
/// * the check (`Module::check_cycles`): the words up to the prefix's address load, and for a
///   TFMX module a look at every pitch program;
/// * the initialisation: 292051 cycles (nearly all of it the noise buffer's filling), 12 more
///   with the MMME tag, 4 more for a COSO file with 16-bit tables; the look through the prefix
///   (`Prefix::scan`); the sample table (`table`: the word at its place when that is inside
///   the file, `samples` entries when it is a table); the prefix patterns' cases, among them
///   the samples' conversion in place (`converted`: bytes, and those with the top bit set);
///   and a raw module's conversion (`Module::conv_cycles`);
/// * the set-up: 4169, less 54 with 16-bit tables, 18 more for a zero speed and 16 per voice
///   whose first position sets a volume.
///
/// The initialisation calls the host three times (four for a raw module, which asks for the
/// image's memory), and each stretch between ends on a whole clock as well: the first is 145479
/// clocks, the last 8, and a raw module's sample table is looked at before the memory request.
/// Not observed, so guessed from the nearest case: a table word of $100 in a raw module (taken
/// as $80), a companion sample file (as no table), a TFMX-tagged COSO file without prefix (the
/// prefixed one's figures).
fn reference_start(m: &Module, table: Option<u16>, samples: usize, converted: Option<(usize, usize)>) -> u64 {
    let coso = m.in_file.is_some();
    let valid = matches!(table, Some(0x80 | 0x100));
    let mut init = 292_051 + 12 * m.mmme as u64 + 4 * (coso && m.short) as u64 + m.prefix.scan + m.conv_cycles;
    // a raw module's stretch before its memory request
    let mut before = 0;
    match table {
        Some(w) if coso => {
            init += 29 + if w != 0x80 { 21 } else { 0 } + if valid { 47 + 33 * samples as u64 } else { 17 };
        }
        Some(_) => {
            init += 33 + if valid { 57 + 33 * samples as u64 } else { 38 };
            before = 738 + if valid { 56 + 33 * samples as u64 } else { 71 };
        }
        None if !coso => before = 738,
        None => {}
    }
    if m.prefix.kind != 0 {
        init += 17;
    }
    if m.prefix.kind == 2 {
        init += 25 + if valid { 77 } else { 0 };
    }
    if let Some((bytes, high)) = converted {
        init += 42 + 58 * bytes as u64 + 23 * high as u64;
    }
    let init_clocks = 145_479 + before / 2 + (init - 290_975 - before) / 2 + 8;
    let e = m.t_sub;
    let (mut start, end, speed) = (be16(&m.b, e), be16(&m.b, e + 2), be16(&m.b, e + 4));
    if (end as i16).wrapping_sub(start as i16) < 0 {
        start = 0;
    }
    let volumes = (0..3).filter(|i| m.g((m.t_pos + start as usize * 12 + i * 4 + 3) as i64) >> 4 == 0xF).count() as u64;
    let setup = 4169 - 54 * m.short as u64 + 18 * (speed == 0) as u64 + 16 * volumes;
    // the host's start, the check, four short calls and the two timer set-ups between
    100 + m.check_cycles / 2 + 20 + init_clocks + 12 + 18 + 118 + 118 + setup / 2
}

fn build_sounds(m: &mut Machine) -> Option<(u32, u32, u32, u32)> {
    // the noise buffer, the pulse, one word that carries the level byte of the SID trick, and
    // the silent word, in the order the original keeps them (one block of 1032 bytes)
    let top = std::mem::replace(&mut m.alloc_top, true);
    let base = m.alloc(1024 + 16);
    m.alloc_top = top;
    if base == 0 {
        return None;
    }
    m.bus.write_bytes(base, NOISE);
    let pulse = base + 1024;
    m.bus.write_bytes(pulse, &PULSE);
    Some((base, pulse, pulse + 5, pulse + 6))
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let mut md = parse(d)?;
    let (noise, pulse, level_cell, silent) = build_sounds(m)?;
    // a raw module's image is in guest memory as the original's is: allocated after the module,
    // its seventh offset the distance from the image to the module's sample table
    let mut image_end = 0;
    if md.in_file.is_none() {
        let a = m.alloc(md.b.len() as u32);
        if a == 0 {
            return None;
        }
        image_end = a + md.b.len() as u32;
        if let Some(o) = md.raw_dig {
            // (a pitch program sent to the image's start plays these bytes as notes)
            md.b[0x1C..0x20].copy_from_slice(&(base + o as u32).wrapping_sub(a).to_be_bytes());
        }
        m.bus.write_bytes(a, &md.b);
    }
    let dig = if md.lsmp {
        let file = m.bus.read_cstr(m.deli.file_buf);
        let names = companion(&file);
        let smp = names.iter().find_map(|n| crate::deli::read_companion(m, n).1.ok())?;
        let a = m.alloc(smp.len() as u32 + 4);
        if a == 0 {
            return None;
        }
        m.bus.write_bytes(a, &smp);
        matches!(be16(&smp, 0), 0x80 | 0x100).then(|| Dig { data: smp, base: a, valid: true })
    } else {
        // the table's place: in the module, or (a raw module without one) where its image ends
        md.dig_file.map(|o| {
            let data = d.get(o..).unwrap_or(&[]).to_vec();
            let valid = matches!(be16(&data, 0), 0x80 | 0x100);
            match (md.in_file, valid) {
                (None, false) => Dig { data: Vec::new(), base: image_end, valid },
                _ => Dig { data, base: base + o as u32, valid },
            }
        })
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
    let has_dig = dig.as_ref().is_some_and(|d| d.valid);
    let note_args = md.prefix.kind != 0 && has_dig;
    let mut converted = None;
    if md.prefix.kind == 2 && has_dig {
        if let Some(dg) = &dig {
            // the nibble-coded sample area becomes signed bytes in place, when its first long has
            // no bit set in any high nibble (only with the first of the two prefix patterns alone)
            let end = (0..dg.count()).map(|n| dg.start(n) as usize).last().unwrap_or(0);
            let first = dg.start(0) as usize;
            if end > first && end <= dg.data.len() && be32(&dg.data, first) & 0xF0F0_F0F0 == 0 {
                let conv: Vec<u8> = dg.data[first..end]
                    .iter()
                    .map(|&b| if b & 0x80 == 0 { LEVELS[(b & 15) as usize] } else if b == 0x88 { LEVELS[17] } else { LEVELS[16] })
                    .collect();
                m.bus.write_bytes(dg.base + first as u32, &conv);
                converted = Some((end - first, dg.data[first..end].iter().filter(|&&b| b & 0x80 != 0).count()));
            }
        }
    }
    m.bus.write_bytes(level_cell - 1, &[LEVELS[0], LEVELS[0]]);
    // the word at the sample table's place, when that is inside the file
    let table = md.dig_file.filter(|&o| o < d.len() && !md.lsmp).map(|o| be16(d, o));
    let ref_start = reference_start(&md, table, dig.as_ref().filter(|d| d.valid).map_or(0, |d| d.count()), converted);
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
        first_tick_pending: true,
        f: [0; F::N as usize],
        f_end: None,
        ref_start,
        t0: 0,
        cy: 0,
        beam: 0,
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
