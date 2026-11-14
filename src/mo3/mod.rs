//! Un4seen MO3 modules (`.mo3`; MO3 versions 0 to 5): IT, XM, S3M, MOD and MTM modules whose
//! music data is LZ-packed and whose samples are delta-coded or MP3 / Ogg Vorbis streams.
//!
//! The file is read as libopenmpt 0.8.7's `Load_mo3.cpp` reads it (the container, the packed
//! music data, the header, orders, tracks and patterns with libopenmpt's assignment of the
//! commands to the volume column and the effect column, instruments, samples, the plugin data
//! skipped, the VERS / PRHI / MIDI chunks, the sample data with its shared Ogg headers and
//! duplicates), and put back into a module of the original format ([`to_native`]), which the
//! libxmp port's own loader for that format then loads: IT, XM, S3M, MTM, and MOD through
//! `loaders::mod_load`. The module plays on the libxmp port's player and mixer, as the original
//! format does; libopenmpt (the reference) mixes on its own (see README).
//!
//! What a module of the original format cannot hold is dropped: OpenMPT's extensions (plugins,
//! the OMPT chunk, MPTM commands, OPL instruments), volume-column commands of another format,
//! stereo samples in XM, MOD and MTM (mixed to mono), samples over a format's length limit
//! (cut).
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, OpenMPT Project Developers and Contributors; see
//! LICENSE-openmpt). The writers of the native formats follow the formats' layouts as the
//! libxmp port's loaders read them.

mod codec;
pub mod unpack;
mod write;

use crate::xmp::load::Hio;
use crate::xmp::module::ModuleData;

/// The largest music data libopenmpt accepts (512 MB).
const MUSIC_LIMIT: u32 = 0x2000_0000;
/// `sizeof(MO3FileHeader)`.
const HEADER_SIZE: usize = 422;

/// The original format of the module.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Xm,
    It,
    S3m,
    Mod,
    Mtm,
}

impl Kind {
    /// The usual file extension of the format.
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Xm => "xm",
            Kind::It => "it",
            Kind::S3m => "s3m",
            Kind::Mod => "mod",
            Kind::Mtm => "mtm",
        }
    }
}

/// `MO3Envelope`.
#[derive(Clone, Copy, Default)]
pub(crate) struct Env {
    pub flags: u8,
    pub num: u8,
    pub sus_start: u8,
    pub sus_end: u8,
    pub loop_start: u8,
    pub loop_end: u8,
    pub points: [(i16, i16); 25],
}

impl Env {
    fn read(b: &[u8]) -> Env {
        let mut e = Env { flags: b[0], num: b[1], sus_start: b[2], sus_end: b[3], loop_start: b[4], loop_end: b[5], ..Default::default() };
        for (k, p) in e.points.iter_mut().enumerate() {
            let o = 6 + k * 4;
            *p = (i16::from_le_bytes([b[o], b[o + 1]]), i16::from_le_bytes([b[o + 2], b[o + 3]]));
        }
        e
    }

    /// The nodes as OpenMPT keeps them (`ConvertToMPT`): at most 25, ticks made increasing,
    /// values shifted by `shift` and clamped to 0..64.
    pub fn nodes(&self, shift: u32) -> Vec<(u16, u8)> {
        let mut v: Vec<(u16, u8)> = Vec::new();
        for k in 0..(self.num as usize).min(25) {
            let mut tick = self.points[k].0 as u16;
            if let Some(&(prev, _)) = v.last() {
                if tick < prev {
                    tick = prev.wrapping_add(1);
                }
            }
            let val = ((self.points[k].1 as i32) >> shift).clamp(0, 64) as u8;
            v.push((tick, val));
        }
        v
    }
}

/// `MO3Instrument`, with its names.
#[derive(Clone)]
pub(crate) struct Ins {
    pub name: Vec<u8>,
    pub filename: Vec<u8>,
    pub map: [(u16, u16); 120],
    pub vol: Env,
    pub pan: Env,
    pub pitch: Env,
    pub vib: [u8; 4],
    pub fadeout: u16,
    pub global_vol: u8,
    pub panning: u16,
    pub nna: u8,
    pub pps: u8,
    pub ppc: u8,
    pub dct: u8,
    pub dca: u8,
    pub vol_swing: u16,
    pub pan_swing: u16,
    pub cutoff: u8,
    pub resonance: u8,
}

impl Ins {
    fn read(name: Vec<u8>, filename: Vec<u8>, b: &[u8]) -> Ins {
        let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let mut map = [(0u16, 0u16); 120];
        for (k, e) in map.iter_mut().enumerate() {
            *e = (u16at(4 + k * 4), u16at(6 + k * 4));
        }
        Ins {
            name,
            filename,
            map,
            vol: Env::read(&b[484..590]),
            pan: Env::read(&b[590..696]),
            pitch: Env::read(&b[696..802]),
            vib: [b[802], b[803], b[804], b[805]],
            fadeout: u16at(806),
            global_vol: b[812],
            panning: u16at(813),
            nna: b[815],
            pps: b[816],
            ppc: b[817],
            dct: b[818],
            dca: b[819],
            vol_swing: u16at(820),
            pan_swing: u16at(822),
            cutoff: b[824],
            resonance: b[825],
        }
    }
}

/// `MO3Sample::MO3SampleFlags`.
pub(crate) mod sflag {
    pub const BIT16: u16 = 0x01;
    pub const LOOP: u16 = 0x10;
    pub const PINGPONG: u16 = 0x20;
    pub const SUSTAIN: u16 = 0x100;
    pub const SUSTAIN_PINGPONG: u16 = 0x200;
    pub const STEREO: u16 = 0x400;
    pub const MPEG: u16 = 0x1000;
    pub const OGG: u16 = 0x3000;
    pub const SHARED_OGG: u16 = 0x7000;
    pub const DELTA: u16 = 0x2000;
    pub const PREDICTION: u16 = 0x4000;
    pub const COMPRESSION: u16 = 0xF000;
}

/// `MO3Sample`, with its names and the decoded sample data.
#[derive(Clone)]
pub(crate) struct Smp {
    pub name: Vec<u8>,
    pub filename: Vec<u8>,
    pub freq: u32,
    pub transpose: i8,
    pub volume: u8,
    pub panning: u16,
    pub length: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub flags: u16,
    pub vib_type: u8,
    pub vib_sweep: u8,
    pub vib_depth: u8,
    pub vib_rate: u8,
    pub global_vol: u8,
    pub sus_start: u32,
    pub sus_end: u32,
    pub compressed: i32,
    pub encoder_delay: u16,
    pub shared_header: i16,
    /// The sample's data, interleaved: frames of `channels` values of 8 or 16 bits (`bits16`).
    pub data: Vec<i16>,
    pub frames: usize,
    pub channels: usize,
    pub bits16: bool,
}

impl Smp {
    fn read(name: Vec<u8>, filename: Vec<u8>, b: &[u8]) -> Smp {
        let u32at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let flags = u16::from_le_bytes([b[20], b[21]]);
        Smp {
            name,
            filename,
            freq: u32at(0),
            transpose: b[4] as i8,
            volume: b[5],
            panning: u16::from_le_bytes([b[6], b[7]]),
            length: u32at(8),
            loop_start: u32at(12),
            loop_end: u32at(16),
            flags,
            vib_type: b[22],
            vib_sweep: b[23],
            vib_depth: b[24],
            vib_rate: b[25],
            global_vol: b[26],
            sus_start: u32at(27),
            sus_end: u32at(31),
            compressed: u32at(35) as i32,
            encoder_delay: u16::from_le_bytes([b[39], b[40]]),
            shared_header: 0,
            data: Vec::new(),
            frames: 0,
            channels: if flags & sflag::STEREO != 0 { 2 } else { 1 },
            bits16: flags & sflag::BIT16 != 0,
        }
    }

    /// The sample rate of C-5 (IT, S3M), as `MO3Sample::ConvertToMPT` gives it.
    pub fn c5speed(&self, hertz: bool) -> u32 {
        if hertz {
            self.freq
        } else {
            let e = (self.freq as i32 as f64 + 1408.0) / 1536.0;
            let v = (8363.0 * 2f64.powf(e)).round();
            v.clamp(0.0, u32::MAX as f64) as u32
        }
    }

    /// OpenMPT's finetune of the sample (MOD, XM, MTM: -128..127).
    pub fn finetune(&self, kind: Kind) -> i8 {
        let f = self.freq as u8 as i8;
        if kind == Kind::Mtm { f } else { f.wrapping_sub(-128i8) }
    }
}

/// One cell of a pattern in the original format's terms: the note as that format writes it
/// (for MOD the index in the period table from 1), the instrument (0 = none), the volume column byte and the effect.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct Cell {
    pub note: Option<u8>,
    pub ins: u8,
    pub vol: Option<u8>,
    pub fx: Option<(u8, u8)>,
}

/// An MO3 module, read.
pub(crate) struct Song {
    pub kind: Kind,
    pub title: Vec<u8>,
    pub channels: usize,
    pub restart: u16,
    pub speed: u8,
    pub tempo: u8,
    pub flags: u32,
    pub global_vol: u8,
    pub pan_separation: u8,
    pub sample_volume: i8,
    pub chn_volume: [u8; 64],
    pub chn_pan: [u8; 64],
    pub sfx_macros: [u8; 16],
    pub fixed_macros: [[u8; 2]; 128],
    pub orders: Vec<u8>,
    /// rows of cells (one per channel), or None for a pattern libopenmpt does not create
    pub patterns: Vec<Option<Vec<Vec<Cell>>>>,
    pub instruments: Vec<Ins>,
    pub sample_mode: bool,
    pub samples: Vec<Smp>,
    pub hertz: bool,
    pub cwtv: u16,
    pub cmwt: u16,
    pub tracker: Vec<u8>,
    pub mtm_version: Option<u8>,
    pub rows_per_beat: Option<(u8, u8)>,
    pub midi: Option<Vec<u8>>,
}

/// `MO3FileHeader::MO3HeaderFlags`.
pub(crate) mod hflag {
    pub const LINEAR: u32 = 0x0001;
    pub const S3M: u32 = 0x0002;
    pub const S3M_FAST_SLIDES: u32 = 0x0004;
    pub const MTM: u32 = 0x0008;
    pub const S3M_AMIGA_LIMITS: u32 = 0x0010;
    pub const MOD: u32 = 0x0080;
    pub const IT: u32 = 0x0100;
    pub const INSTRUMENT_MODE: u32 = 0x0200;
    pub const IT_COMPAT_GXX: u32 = 0x0400;
    pub const IT_OLD_FX: u32 = 0x0800;
    pub const HAS_PLUGINS: u32 = 0x100000;
}

/// Whether `d` is an MO3 file (`ValidateHeader`).
pub fn test(d: &[u8]) -> bool {
    if d.len() < 8 || &d[..3] != b"MO3" || d[3] > 5 {
        return false;
    }
    let size = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
    size as usize > HEADER_SIZE && size < MUSIC_LIMIT
}

/// A reader over the unpacked music data (libopenmpt's `FileReader` reads: a short read fails
/// a struct, gives 0 for a number).
struct R<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn left(&self) -> usize {
        self.d.len().saturating_sub(self.p)
    }
    fn u8(&mut self) -> u8 {
        let v = self.d.get(self.p).copied().unwrap_or(0);
        self.p = (self.p + 1).min(self.d.len());
        v
    }
    fn u16(&mut self) -> u16 {
        if self.left() < 2 {
            self.p = self.d.len();
            return 0;
        }
        let v = u16::from_le_bytes([self.d[self.p], self.d[self.p + 1]]);
        self.p += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        if self.left() < 4 {
            self.p = self.d.len();
            return 0;
        }
        let v = u32::from_le_bytes([self.d[self.p], self.d[self.p + 1], self.d[self.p + 2], self.d[self.p + 3]]);
        self.p += 4;
        v
    }
    /// `ReadStruct` of `n` bytes.
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.left() < n {
            return None;
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Some(s)
    }
    /// `ReadChunk`: up to `n` bytes.
    fn chunk(&mut self, n: usize) -> &'a [u8] {
        let n = n.min(self.left());
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        s
    }
    /// `ReadNullString`.
    fn cstr(&mut self) -> Vec<u8> {
        let rest = &self.d[self.p.min(self.d.len())..];
        let n = rest.iter().position(|&b| b == 0);
        let s = rest[..n.unwrap_or(rest.len())].to_vec();
        self.p += n.map_or(rest.len(), |n| n + 1);
        s
    }
}

/// `ImpulseTrackerPortaVolCmd`.
const IT_PORTA_VOL: [u8; 10] = [0x00, 0x01, 0x04, 0x08, 0x10, 0x20, 0x40, 0x60, 0x80, 0xFF];

/// The commands of one event (pairs of command and parameter) as a cell of the original format,
/// with libopenmpt's choices of where each one goes (volume column or effect).
fn event(kind: Kind, cmds: &[[u8; 2]]) -> Cell {
    let mut c = Cell::default();
    let xm = kind == Kind::Xm;
    let it = kind == Kind::It;
    let st = matches!(kind, Kind::It | Kind::S3m);
    for &[cmd, p] in cmds {
        let fx = |n: u8| Some((n, p));
        match cmd {
            0x01 => {
                let n = match kind {
                    Kind::Xm => match p {
                        0..=95 => p + 1,
                        96..=119 => 0,
                        _ => 97,
                    },
                    Kind::It => match p {
                        0..=119 => p,
                        0xFF => 255,
                        0xFE => 254,
                        _ => 246,
                    },
                    Kind::S3m => match p {
                        0..=119 => ((p / 12) << 4) | (p % 12),
                        _ => 254,
                    },
                    // the index in the period table (`write` turns it into a period); 0 = none
                    Kind::Mod => match p {
                        12..=95 => p - 11,
                        _ => 0,
                    },
                    Kind::Mtm => match p {
                        24..=86 => p - 23,
                        _ => 0,
                    },
                };
                c.note = if n == 0 && matches!(kind, Kind::Xm | Kind::Mod | Kind::Mtm) { None } else { Some(n) };
            }
            0x02 => c.ins = p.wrapping_add(1),
            0x03 => c.fx = fx(if st { 10 } else { 0 }),
            0x04 => c.fx = fx(if st { 6 } else { 1 }),
            0x05 => c.fx = fx(if st { 5 } else { 2 }),
            0x06 => {
                if c.vol.is_none() && xm && p & 0x0F == 0 {
                    c.vol = Some(0xF0 | (p >> 4));
                } else if c.vol.is_none() && it && IT_PORTA_VOL.contains(&p) {
                    c.vol = Some(193 + IT_PORTA_VOL.iter().position(|&v| v == p).unwrap_or(0) as u8);
                } else {
                    c.fx = fx(if st { 7 } else { 3 });
                }
            }
            0x07 => {
                if c.vol.is_none() && p < 10 && it {
                    c.vol = Some(203 + p);
                } else {
                    c.fx = fx(if st { 8 } else { 4 });
                }
            }
            0x08 => c.fx = fx(if st { 12 } else { 5 }),
            0x09 => c.fx = fx(if st { 11 } else { 6 }),
            0x0A => c.fx = fx(if st { 18 } else { 7 }),
            0x0B => {
                if c.vol.is_none() && it && p == 0xFF {
                    c.vol = Some(128 + 64);
                } else if c.vol.is_none() && it && p & 0x03 == 0 {
                    c.vol = Some(128 + p / 4);
                } else if c.vol.is_none() && xm && p & 0x0F == 0 {
                    c.vol = Some(0xC0 | (p >> 4));
                } else {
                    c.fx = fx(if st { 24 } else { 8 });
                }
            }
            0x0C => c.fx = fx(if st { 15 } else { 9 }),
            0x0D => c.fx = fx(if st { 4 } else { 0x0A }),
            0x0E => c.fx = fx(if st { 2 } else { 0x0B }),
            0x0F => {
                if kind != Kind::Mod && c.vol.is_none() && p <= 64 && matches!(kind, Kind::Xm | Kind::It | Kind::S3m) {
                    c.vol = Some(if xm { 0x10 + p } else { p });
                } else if !st {
                    c.fx = fx(0x0C);
                }
                // (IT and S3M have no volume effect)
            }
            0x10 => c.fx = fx(if st { 3 } else { 0x0D }),
            0x11 => {
                if !st {
                    c.fx = fx(0x0E);
                }
            }
            0x12 => c.fx = fx(if st { if p < 0x20 { 1 } else { 20 } } else { 0x0F }),
            0x13 if xm => c.fx = fx(29),
            0x13 if st => c.fx = fx(9),
            0x14 | 0x15 if xm => {
                let (up, down) = if cmd == 0x14 { (0x70, 0x60) } else { (0x90, 0x80) };
                c.vol = Some(if p & 0xF0 != 0 { up | (p >> 4) } else { down | (p & 0x0F) });
            }
            0x16 => c.fx = fx(if st { 22 } else { 16 }),
            0x17 if xm => c.fx = fx(17),
            0x18 if xm => c.fx = fx(20),
            0x19 if xm => c.fx = fx(21),
            0x1A if xm => c.fx = fx(25),
            0x1A if st => c.fx = fx(16),
            0x1B if xm => c.vol = Some(if p & 0xF0 != 0 { 0xE0 | (p >> 4) } else { 0xD0 | (p & 0x0F) }),
            0x1C if xm => c.fx = fx(27),
            0x1C if st => c.fx = fx(17),
            0x1D if xm => c.fx = Some((33, 0x10 | p)),
            0x1E if xm => c.fx = Some((33, 0x20 | p)),
            0x1F if xm => c.vol = Some(0xA0 | (p & 0x0F)),
            0x20 if xm => c.vol = Some(0xB0 | (p & 0x0F)),
            0x21 if st => c.fx = fx(1),
            0x22 if st => {
                c.fx = Some(match c.fx {
                    Some((7, _)) => (12, p),
                    Some((8, _)) => (11, p),
                    _ => (4, p),
                });
            }
            0x23 if st => c.fx = fx(5),
            0x24 if st => c.fx = fx(6),
            0x25 if st => c.fx = fx(9),
            0x26 if st => c.fx = fx(17),
            0x27 if st => c.fx = fx(21),
            0x28 if st => c.fx = fx(13),
            0x29 if st => c.fx = fx(14),
            0x2A if st => c.fx = fx(16),
            0x2B if st => c.fx = fx(19),
            0x2C if st => c.fx = fx(20),
            0x2D if st => c.fx = fx(23),
            0x2E if st => c.fx = fx(25),
            0x2E if xm => c.fx = fx(34),
            0x2F if st => c.fx = fx(26),
            0x2F if xm => c.fx = fx(35),
            0x30 if it => {
                if p < 40 {
                    c.vol = Some(65 + p);
                }
            }
            0x31 if it => c.vol = Some(105 + p.min(9)),
            0x32 if it => c.vol = Some(115 + p.min(9)),
            0x33 if xm => c.fx = fx(32),
            _ => {}
        }
    }
    c
}

/// Reads an MO3 file. None when libopenmpt would not load it.
pub(crate) fn read(file: &[u8]) -> Option<Song> {
    if !test(file) {
        return None;
    }
    let version = file[3];
    let music_size = u32::from_le_bytes([file[4], file[5], file[6], file[7]]);
    let (mut pos, mut compressed) = (8usize, None);
    if version >= 5 {
        let c = u32::from_le_bytes(file.get(8..12)?.try_into().ok()?) as usize;
        if file.len() - 12 < c {
            return None;
        }
        compressed = Some(c);
        pos = 12;
    }
    let (music, complete, end) = unpack::unpack_music(file, pos, music_size);
    let mut r = R { d: &music, p: 0 };
    let title = r.cstr();
    let _message = r.cstr();
    let h = r.take(HEADER_SIZE)?;
    let u16at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let channels = h[0] as usize;
    let num_orders = u16at(1) as usize;
    let restart = u16at(3);
    let num_patterns = u16at(5) as usize;
    let num_tracks = u16at(7) as usize;
    let num_instruments = u16at(9) as usize;
    let num_samples = u16at(11) as usize;
    let flags = u32::from_le_bytes([h[15], h[16], h[17], h[18]]);
    // (MAX_BASECHANNELS, MAX_INSTRUMENTS, MAX_SAMPLES)
    if channels == 0 || channels > 127 || restart as usize > num_orders || num_instruments >= 256 || num_samples >= 4000 {
        return None;
    }
    let kind = if flags & hflag::IT != 0 {
        Kind::It
    } else if flags & hflag::S3M != 0 {
        Kind::S3m
    } else if flags & hflag::MOD != 0 {
        Kind::Mod
    } else if flags & hflag::MTM != 0 {
        Kind::Mtm
    } else {
        Kind::Xm
    };
    let mut song = Song {
        kind,
        title,
        channels,
        restart,
        speed: h[13],
        tempo: h[14],
        flags,
        global_vol: h[19],
        pan_separation: h[20],
        sample_volume: h[21] as i8,
        chn_volume: h[22..86].try_into().ok()?,
        chn_pan: h[86..150].try_into().ok()?,
        sfx_macros: h[150..166].try_into().ok()?,
        fixed_macros: [[0; 2]; 128],
        orders: Vec::new(),
        patterns: Vec::new(),
        instruments: Vec::new(),
        sample_mode: kind != Kind::Xm && flags & hflag::INSTRUMENT_MODE == 0,
        samples: Vec::new(),
        hertz: version >= 5 || flags & hflag::LINEAR == 0,
        cwtv: 0,
        cmwt: 0,
        tracker: Vec::new(),
        mtm_version: None,
        rows_per_beat: None,
        midi: None,
    };
    for (i, m) in song.fixed_macros.iter_mut().enumerate() {
        *m = [h[166 + i * 2], h[167 + i * 2]];
    }
    song.orders = r.chunk(num_orders).to_vec();
    let track_idx = r.chunk(num_patterns * channels * 2);
    let pat_len = r.chunk(num_patterns * 2);
    let mut tracks: Vec<&[u8]> = Vec::with_capacity(num_tracks.min(1 << 16));
    for _ in 0..num_tracks {
        let len = r.u32() as usize;
        if len >= 0x20_0000 {
            return None;
        }
        tracks.push(r.chunk(len));
    }

    // patterns
    let mut ti = R { d: track_idx, p: 0 };
    let mut pl = R { d: pat_len, p: 0 };
    for _ in 0..num_patterns {
        let rows = pl.u16() as usize;
        // (`Patterns.Insert` refuses 0 rows and more than MAX_PATTERN_ROWS; the pattern's
        // track indices are then not read)
        if rows == 0 || rows > 1024 {
            song.patterns.push(None);
            continue;
        }
        let mut grid = vec![vec![Cell::default(); channels]; rows];
        for chn in 0..channels {
            let t = ti.u16() as usize;
            let Some(track) = tracks.get(t) else { continue };
            let mut tr = R { d: track, p: 0 };
            let mut row = 0;
            let mut cmds: Vec<[u8; 2]> = Vec::with_capacity(16);
            while row < rows {
                let b = tr.u8();
                if b == 0 {
                    break;
                }
                let (n, rep) = ((b & 0x0F) as usize, (b >> 4) as usize);
                cmds.clear();
                for _ in 0..n {
                    let a = tr.u8();
                    let p = tr.u8();
                    cmds.push([a, p]);
                }
                let cell = event(kind, &cmds);
                let target = (row + rep).min(rows);
                while row < target {
                    grid[row][chn] = cell;
                    row += 1;
                }
            }
        }
        song.patterns.push(Some(grid));
    }

    // instruments
    for _ in 0..num_instruments {
        let name = r.cstr();
        let filename = if version >= 5 { r.cstr() } else { Vec::new() };
        let b = r.take(826)?;
        if !song.sample_mode {
            song.instruments.push(Ins::read(name, filename, b));
        }
    }

    // sample headers
    for _ in 0..num_samples {
        let name = r.cstr();
        let filename = if version >= 5 { r.cstr() } else { Vec::new() };
        let b = r.take(41)?;
        let mut s = Smp::read(name, filename, b);
        if version >= 5 && s.flags & sflag::COMPRESSION == sflag::SHARED_OGG {
            s.shared_header = r.u16() as i16;
        }
        song.samples.push(s);
    }

    // plugin data (not played)
    if flags & hflag::HAS_PLUGINS != 0 && r.left() >= 1 {
        let pflags = r.u8();
        if pflags & 1 != 0 {
            for _ in 0..channels {
                r.u32();
            }
        }
        while r.left() >= 1 {
            let plug = r.u8();
            if plug == 0 {
                break;
            }
            let len = r.u32();
            if len >= music_size || (music_size - len) < r.p as u32 {
                return None;
            }
            r.chunk(len as usize);
        }
    }

    // chunks
    while r.left() >= 8 {
        let id = r.u32().to_le_bytes();
        let len = r.u32();
        if len >= music_size || (music_size - len) < r.p as u32 {
            return None;
        }
        let c = r.chunk(len as usize);
        let mut cr = R { d: c, p: 0 };
        match &id {
            b"VERS" => match kind {
                Kind::It => {
                    song.cwtv = cr.u16();
                    song.cmwt = cr.u16();
                }
                Kind::S3m => song.cwtv = cr.u16(),
                Kind::Xm => song.tracker = c[..c.len().min(32)].to_vec(),
                Kind::Mtm => song.mtm_version = Some(cr.u8()),
                Kind::Mod => {}
            },
            b"PRHI" => song.rows_per_beat = Some((cr.u8(), cr.u8())),
            b"MIDI" if c.len() >= 4896 => song.midi = Some(c[..4896].to_vec()),
            _ => {}
        }
    }

    // sample data
    let mut fpos = if version < 5 {
        if !complete {
            return None;
        }
        end
    } else {
        12 + compressed.unwrap_or(0)
    };
    let mut chunks: Vec<Option<&[u8]>> = vec![None; num_samples];
    for (i, s) in song.samples.iter_mut().enumerate() {
        let compression = s.flags & sflag::COMPRESSION;
        if compression == 0 && s.compressed == 0 {
            // uncompressed: stereo as all left, then all right
            let mut frames = s.length as usize;
            let bps = if s.bits16 { 2 } else { 1 };
            let want = frames.saturating_mul(bps).saturating_mul(s.channels);
            let avail = file.len().saturating_sub(fpos).min(want);
            let raw = &file[fpos.min(file.len())..fpos.min(file.len()) + avail];
            fpos += avail;
            // (a sample cut short by the end of the file: libopenmpt pads it with silence, here
            // it ends with the file's data)
            frames = frames.min(avail / bps);
            if frames == 0 {
                continue;
            }
            let mut data = vec![0i16; frames * s.channels];
            for ch in 0..s.channels {
                for k in 0..frames {
                    let o = (ch * s.length as usize + k) * bps;
                    if o + bps > raw.len() {
                        break;
                    }
                    data[k * s.channels + ch] = if s.bits16 { i16::from_le_bytes([raw[o], raw[o + 1]]) } else { raw[o] as i8 as i16 };
                }
            }
            s.data = data;
            s.frames = frames;
        } else if s.compressed > 0 {
            let n = (s.compressed as usize).min(file.len().saturating_sub(fpos));
            chunks[i] = Some(&file[fpos.min(file.len())..fpos.min(file.len()) + n]);
            fpos += n;
        }
    }
    for i in 0..num_samples {
        let s = &song.samples[i];
        if s.compressed < 0 && (i as i64 + 1 + s.compressed as i64) > 0 {
            // a duplicate of an earlier sample
            let src = (i as i64 + s.compressed as i64) as usize;
            let (data, frames, channels, bits16) = {
                let o = &song.samples[src];
                (o.data.clone(), o.frames, o.channels, o.bits16)
            };
            let s = &mut song.samples[i];
            (s.data, s.frames, s.channels, s.bits16) = (data, frames, channels, bits16);
            continue;
        }
        let Some(chunk) = chunks[i] else { continue };
        if s.length == 0 {
            continue;
        }
        let compression = s.flags & sflag::COMPRESSION;
        let (channels, bits16) = (s.channels, s.bits16);
        let mut frames = s.length as usize;
        match compression {
            sflag::DELTA | sflag::PREDICTION => {
                // at best two bits a sample point
                let per_byte = 4 / channels;
                frames = frames.min(chunk.len().saturating_mul(per_byte));
                let data = match (compression == sflag::DELTA, bits16) {
                    (true, false) => unpack::delta8(chunk, frames, channels).into_iter().map(|v| v as i16).collect(),
                    (true, true) => unpack::delta16(chunk, frames, channels),
                    (false, false) => unpack::prediction8(chunk, frames, channels).into_iter().map(|v| v as i16).collect(),
                    (false, true) => unpack::prediction16(chunk, frames, channels),
                };
                let s = &mut song.samples[i];
                s.data = data;
                s.frames = frames;
            }
            sflag::OGG | sflag::SHARED_OGG => {
                // (libopenmpt allocates no sample over MAX_SAMPLE_LENGTH frames; libxmp loads
                // none over MAX_SAMPLE_SIZE bytes)
                if frames > 0x1000_0000 || frames * channels * if bits16 { 2 } else { 1 } > crate::xmp::module::MAX_SAMPLE_SIZE as usize {
                    continue;
                }
                let header_size = s.encoder_delay as usize;
                let shared_idx = if i as i64 + 1 + s.shared_header as i64 > 0 { (i as i64 + 1 + s.shared_header as i64) as usize } else { i + 1 };
                let shared = shared_idx != i + 1 && shared_idx > 0 && shared_idx <= num_samples && header_size > 0;
                let header: &[u8] = if shared {
                    let Some(hc) = chunks[shared_idx - 1] else { continue };
                    if hc.len() < header_size {
                        continue;
                    }
                    &hc[..header_size]
                } else {
                    chunk
                };
                if let Some(v) = codec::vorbis(header, chunk, shared, frames, channels, bits16) {
                    let s = &mut song.samples[i];
                    s.frames = v.len() / channels;
                    s.data = v.into_iter().map(|x| x as i16).collect();
                }
            }
            sflag::MPEG => {
                if let Some((v, ch)) = codec::mp3(chunk, s.encoder_delay, s.length) {
                    let s = &mut song.samples[i];
                    s.frames = v.len() / ch;
                    s.data = v;
                    s.channels = ch;
                    s.bits16 = true;
                }
            }
            _ => {}
        }
    }
    Some(song)
}

/// The module in `file` put back into its original format: the file and the format, or None
/// when it is not an MO3 file libopenmpt would load.
pub fn to_native(file: &[u8]) -> Option<(Vec<u8>, Kind)> {
    let song = read(file)?;
    let out = write::write(&song)?;
    Some((out, song.kind))
}

/// The MO3 loader of `xmp::LOADERS`: the module in its original format, loaded by the libxmp
/// port's loader of that format; the format name says both.
pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut file = vec![0u8; f.size()];
    f.seek(0);
    f.read(&mut file);
    let version = file.get(3).copied().unwrap_or(0);
    let (native, kind) = to_native(&file).ok_or(())?;
    let mut h = Hio::new(&native);
    use crate::xmp::loaders;
    match kind {
        Kind::Xm => loaders::xm::load(m, &mut h)?,
        Kind::It => loaders::it::load(m, &mut h)?,
        Kind::S3m => loaders::s3m::load(m, &mut h)?,
        Kind::Mtm => loaders::mtm::load(m, &mut h)?,
        Kind::Mod => loaders::mod_load::load(m, &mut h)?,
    }
    m.module.type_ = format!("Un4seen MO3 v{version} ({})", m.module.type_);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `data` in MO3's LZ coding as literals only: the first byte verbatim, then a control byte
    /// of eight 0 bits before every eight literals.
    fn pack_literals(data: &[u8]) -> Vec<u8> {
        let mut o = vec![data[0]];
        for c in data[1..].chunks(8) {
            o.push(0);
            o.extend_from_slice(c);
        }
        o
    }

    /// An MO3 v1 file of an XM module: one channel, one 64-row pattern (C-4, instrument 1,
    /// volume 32, then an arpeggio with 0x37 on row 1 and a volume-column slide on row 2), one
    /// instrument and one raw 8-bit sample of 16 bytes.
    fn synth_xm() -> Vec<u8> {
        let mut m = b"tiny\0\0".to_vec();
        let mut h = [0u8; HEADER_SIZE];
        h[0] = 1; // channels
        h[1] = 1; // orders
        h[5] = 1; // patterns
        h[7] = 1; // tracks
        h[9] = 1; // instruments
        h[11] = 1; // samples
        h[13] = 5;
        h[14] = 130;
        h[15] = 1; // linear slides
        h[19] = 64;
        m.extend_from_slice(&h);
        m.push(0); // orders
        m.extend_from_slice(&[0, 0]); // the pattern's track
        m.extend_from_slice(&[64, 0]); // its rows
        let track = [0x13, 0x01, 48, 0x02, 0, 0x0F, 32, 0x11, 0x03, 0x37, 0x11, 0x14, 0x20, 0];
        m.extend_from_slice(&(track.len() as u32).to_le_bytes());
        m.extend_from_slice(&track);
        m.extend_from_slice(b"ins\0");
        let mut ins = [0u8; 826];
        for k in 0..120 {
            ins[4 + k * 4] = k as u8;
        }
        ins[813..815].copy_from_slice(&0xFFFFu16.to_le_bytes());
        m.extend_from_slice(&ins);
        m.extend_from_slice(b"smp\0");
        let mut s = [0u8; 41];
        s[0] = 128; // finetune 0
        s[5] = 48;
        s[6..8].copy_from_slice(&0xFFFFu16.to_le_bytes());
        s[8] = 16;
        m.extend_from_slice(&s);
        let mut f = b"MO3\x01".to_vec();
        f.extend_from_slice(&(m.len() as u32).to_le_bytes());
        f.extend_from_slice(&pack_literals(&m));
        f.extend((0..16).map(|i| (i * 8) as u8));
        f
    }

    #[test]
    fn header_test() {
        let f = synth_xm();
        assert!(test(&f));
        let mut v6 = f.clone();
        v6[3] = 6;
        assert!(!test(&v6));
        assert!(!test(b"MO3\x01\x10\0\0\0"));
    }

    #[test]
    fn literal_music_unpacks() {
        let data: Vec<u8> = (0..100u8).collect();
        let (out, complete, end) = unpack::unpack_music(&pack_literals(&data), 0, 100);
        assert_eq!((out, complete, end), (data, true, 100 + 13));
    }

    #[test]
    fn loads_synthetic_xm() {
        let f = synth_xm();
        let (native, kind) = to_native(&f).unwrap();
        assert_eq!(kind, Kind::Xm);
        assert!(crate::xmp::loaders::xm::test(&native));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&f)).unwrap();
        assert_eq!(m.module.type_, "Un4seen MO3 v1 (FastTracker v2.00 XM 1.04)");
        assert_eq!((m.module.chn, m.module.ins, m.module.spd, m.module.bpm), (1, 1, 5, 130));
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.note, e.ins, e.vol), (49 + 12, 1, 32 + 1));
        let e = *m.module.event(0, 0, 1);
        assert_eq!((e.fxt, e.fxp), (0, 0x37));
        let e = *m.module.event(0, 0, 2);
        assert_eq!((e.vol, e.f2t, e.f2p), (0, crate::xmp::module::FX_VOLSLIDE_2, 0x20));
        assert_eq!(m.module.xxs[0].len, 16);
        assert_eq!(m.module.xxi[0].sub[0].vol, 48);
    }

    /// Damaged files (cut short, bytes changed) load or are refused, never panic.
    #[test]
    fn damaged_files() {
        let f = synth_xm();
        for n in 0..f.len() {
            let mut m = ModuleData::default();
            let _ = load(&mut m, &mut Hio::new(&f[..n]));
        }
        for at in 0..f.len() {
            for v in [0x00, 0xFF, f[at].wrapping_add(1), 0x80] {
                let mut g = f.clone();
                g[at] = v;
                let mut m = ModuleData::default();
                let _ = load(&mut m, &mut Hio::new(&g));
            }
        }
    }

    /// The same on the files in `$MO3_FUZZ_IN` (cut at 64 points, 300 bytes changed each).
    #[test]
    #[ignore]
    fn damaged_real_files() {
        let Ok(inp) = std::env::var("MO3_FUZZ_IN") else { return };
        let mut seed = 0x1234_5678u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for p in std::fs::read_dir(&inp).unwrap().flatten().map(|e| e.path()) {
            let f = std::fs::read(&p).unwrap();
            println!("{}", p.display());
            for k in 1..64 {
                let _ = to_native(&f[..f.len() * k / 64]);
            }
            for _ in 0..300 {
                let mut g = f.clone();
                let at = rnd() as usize % g.len().min(4096);
                g[at] = rnd() as u8;
                let _ = to_native(&g);
            }
        }
    }

    /// Writes the modules of the MO3 files in `$MO3_DUMP_IN` put back into their formats to
    /// `$MO3_DUMP_OUT` (for checks against a reference player):
    /// `MO3_DUMP_IN=dir MO3_DUMP_OUT=dir cargo test --release mo3::tests::dump -- --ignored`
    #[test]
    #[ignore]
    fn dump() {
        let (Ok(inp), Ok(out)) = (std::env::var("MO3_DUMP_IN"), std::env::var("MO3_DUMP_OUT")) else { return };
        let mut names: Vec<_> = std::fs::read_dir(&inp).unwrap().flatten().map(|e| e.path()).collect();
        names.sort();
        for p in names {
            let d = std::fs::read(&p).unwrap();
            match super::to_native(&d) {
                Some((m, k)) => {
                    let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
                    std::fs::write(std::path::Path::new(&out).join(format!("{stem}.{}", k.ext())), m).unwrap();
                }
                None => println!("refused: {}", p.display()),
            }
        }
    }
}
