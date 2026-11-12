//! The Symphonie loader: a port of libopenmpt 0.8.7's `soundlib/Load_symmod.cpp` (see `mod.rs`
//! for the provenance and licence). It reads the chunks, unpacks the RLE and delta-coded blocks,
//! decodes the embedded sample files, runs Symphonie's load-time sample processing, renders the
//! virtual and transwave instruments, and converts the positions, sequences and pattern events
//! into rows of OpenMPT-style commands, as OpenMPT's loader does; `play.rs` plays those.
//!
//! Untrusted input: every chunk length is checked against the file, every unpacked size is
//! bounded (per chunk and in total), and the samples the processing makes (sustain repeats,
//! virtual and transwave renders) have a frame budget; past it a chunk reads as empty or a render
//! fails, as an allocation failure does in OpenMPT.

use std::collections::BTreeMap;

/// The channels a module may have (OpenMPT: `MAX_BASECHANNELS`).
pub const MAX_CHANNELS: usize = 192;
/// `MAX_SAMPLES` of OpenMPT: sample slots (1-based) a module may use.
const MAX_SAMPLES: usize = 4000;
/// `MAX_INSTRUMENTS` of OpenMPT.
const MAX_INSTRUMENTS: usize = 256;
/// Rows of one pattern (OpenMPT: `MAX_PATTERN_ROWS`).
const MAX_PATTERN_ROWS: usize = 4096;
/// Patterns (OpenMPT: the MPTM limit of 4000).
const MAX_PATTERNS: usize = 4000;
/// Order entries kept (OpenMPT keeps up to 65000 per sequence).
const MAX_ORDERS: usize = 65000;
/// Not in OpenMPT: the bytes one chunk may unpack to, and all chunks together.
const MAX_CHUNK_BYTES: usize = 64 << 20;
const MAX_TOTAL_BYTES: usize = 256 << 20;
/// Not in OpenMPT: the frames one sample may have after processing, and all samples together.
const MAX_SAMPLE_FRAMES: usize = 32 << 20;
const MAX_TOTAL_FRAMES: usize = 96 << 20;
/// Not in OpenMPT: sample operations the load-time filters and renders may do (each filter pass
/// over a sample costs its length, a virtual render its length times its voices).
const MAX_WORK: u64 = 1 << 28;
/// Not in OpenMPT: pattern cells (rows times channels) of all converted patterns.
const MAX_CELLS: usize = 8 << 20;

/// "No cue point" (OpenMPT's `MAX_SAMPLE_LENGTH`, which `RemoveAllCuePoints` writes).
pub const NO_CUE: u32 = 0x1000_0000;

/// Sample data as the loader holds it: 8- or 16-bit, mono (stereo files are split).
#[derive(Clone, Debug, Default)]
pub enum Data {
    #[default]
    Empty,
    I8(Vec<i8>),
    I16(Vec<i16>),
}

impl Data {
    pub fn len(&self) -> usize {
        match self {
            Data::Empty => 0,
            Data::I8(v) => v.len(),
            Data::I16(v) => v.len(),
        }
    }

    fn has_data(&self) -> bool {
        self.len() > 0
    }

    /// The sample at `i` on the 16-bit scale (0 past the end, as OpenMPT's zeroed lookahead).
    pub(crate) fn at16(&self, i: usize) -> i32 {
        match self {
            Data::Empty => 0,
            Data::I8(v) => v.get(i).map_or(0, |&x| x as i32 * 256),
            Data::I16(v) => v.get(i).map_or(0, |&x| x as i32),
        }
    }
}

/// A sample slot.
#[derive(Clone, Debug)]
pub struct Sample {
    pub data: Data,
    pub loop_on: bool,
    pub loop_start: u32,
    pub loop_end: u32,
    /// the first cue point (`cues[0]`): where a key-off with `VOLCMD_OFFSET` 1 jumps to
    pub cue: u32,
    pub c5: u32,
    /// the sample's global volume, 0..64
    pub gv: u8,
}

impl Default for Sample {
    fn default() -> Sample {
        // ModSample::Initialize(MOD_TYPE_IT): C5 8363 Hz, global volume 64, the default cues
        Sample { data: Data::Empty, loop_on: false, loop_start: 0, loop_end: 0, cue: 255 * 256 / 9, c5: 8363, gv: 64 }
    }
}

/// An instrument: one sample for every note.
#[derive(Clone, Debug)]
pub struct Instrument {
    pub sample: usize,
    /// global volume, 0..64
    pub gv: u8,
    /// routed past the echo DSP; also sets the channel filter off when it plays
    pub no_dsp: bool,
    pub name: String,
}

/// Effect column commands the conversion writes (OpenMPT's `CMD_*` of the same names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Cmd {
    #[default]
    None,
    ChannelVolume,
    ChannelVolSlide,
    PortaUp,
    PortaDown,
    TonePorta,
    TonePortaDuration,
    Vibrato,
    Tremolo,
    Retrig,
    Speed,
    Midi,
    OffsetPercentage,
}

/// Volume column commands the conversion writes (OpenMPT's `VOLCMD_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VolCmd {
    #[default]
    None,
    PlayControl,
    Offset,
    TonePorta,
    PortaUp,
    PortaDown,
}

pub const NOTE_NONE: u8 = 0;
pub const NOTE_KEYOFF: u8 = 0xFF;
const NOTE_MIN: i32 = 1;
const NOTE_MAX: i32 = 128;

/// One pattern cell (OpenMPT's `ModCommand`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    pub note: u8,
    pub instr: u8,
    pub volcmd: VolCmd,
    pub vol: u8,
    pub cmd: Cmd,
    pub param: u8,
}

impl Cell {
    fn set_fx(&mut self, cmd: Cmd, param: u8) {
        self.cmd = cmd;
        self.param = param;
    }
    fn set_vol(&mut self, v: VolCmd, vol: u8) {
        self.volcmd = v;
        self.vol = vol;
    }
    pub fn is_tone_porta(&self) -> bool {
        matches!(self.cmd, Cmd::TonePorta | Cmd::TonePortaDuration) || self.volcmd == VolCmd::TonePorta
    }
}

pub struct Pattern {
    pub rows: usize,
    pub cells: Vec<Cell>,
}

/// What a `Zxx` macro (`CMD_MIDI` 0x80 | n) does: the internal-device macros the loader writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Macro {
    /// not set up by the loader (OpenMPT's default fixed macros are never referenced)
    Unused,
    /// `F0F000cc F0F001rr F0F002mm`: cutoff, resonance, mode (0 lowpass, 1 highpass)
    Filter { cutoff: u8, reso: u8, highpass: bool },
    /// `F0F0007F F0F00100`: no filter
    FilterOff,
    /// `F0F080tt F0F081dd F0F082ff`: echo type, delay (ticks), feedback
    Echo { kind: u8, delay: u8, feedback: u8 },
}

pub struct Module {
    pub channels: usize,
    pub patterns: Vec<Pattern>,
    /// the order list; `None` is OpenMPT's "+++" skip index between sequences
    pub order: Vec<Option<usize>>,
    /// 1-based; slot 0 is unused
    pub samples: Vec<Sample>,
    /// 1-based; `None` for an empty instrument
    pub instruments: Vec<Option<Instrument>>,
    pub tempo: f64,
    pub zxx: Vec<Macro>,
    pub use_dsp: bool,
    /// `m_nSamplePreAmp`
    pub preamp: u32,
    pub tracker: &'static str,
    pub message: String,
}

/// Whether `data` starts with a Symphonie header (`ProbeFileHeaderSymMOD`).
pub fn test(data: &[u8]) -> bool {
    data.len() >= 16
        && &data[0..4] == b"SymM"
        && be32(data, 4) == 1
        && be32(data, 8) as i32 == -1
        && (1..=256).contains(&be32(data, 12))
}

fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// A cursor over the file, with OpenMPT's `FileReader` behaviour at the end: reads that do not
/// fit return 0 and leave the cursor at the end.
struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn left(&self) -> usize {
        self.d.len() - self.p
    }
    fn can_read(&self, n: usize) -> bool {
        self.left() >= n
    }
    fn u32(&mut self) -> u32 {
        if !self.can_read(4) {
            self.p = self.d.len();
            return 0;
        }
        let v = be32(self.d, self.p);
        self.p += 4;
        v
    }
    fn skip(&mut self, n: usize) {
        self.p = self.p.saturating_add(n).min(self.d.len());
    }
}

/// The decoding budget of one load.
struct Budget {
    bytes: usize,
    frames: usize,
    work: u64,
}

impl Budget {
    fn take_bytes(&mut self, n: usize) -> bool {
        if n > MAX_CHUNK_BYTES || n > self.bytes {
            return false;
        }
        self.bytes -= n;
        true
    }
    fn take_work(&mut self, n: u64) -> bool {
        if n > self.work {
            return false;
        }
        self.work -= n;
        true
    }
    fn take_frames(&mut self, n: usize) -> bool {
        if n > MAX_SAMPLE_FRAMES || n > self.frames {
            return false;
        }
        self.frames -= n;
        true
    }
}

/// `DecodeSymChunk`: a length-prefixed chunk, RLE-unpacked when it starts with `PACK\xFF\xFF`.
fn decode_chunk(r: &mut Reader, b: &mut Budget) -> Vec<u8> {
    let packed = r.u32() as usize;
    if !r.can_read(packed) {
        r.p = r.d.len();
        return Vec::new();
    }
    let chunk = &r.d[r.p..r.p + packed];
    r.p += packed;
    if packed >= 10 && chunk.starts_with(b"PACK\xFF\xFF") {
        let mut unpacked = be32(chunk, 6) as usize;
        // six bytes turn into up to 255 * 4 bytes at best: a ratio of 1:170
        let max = (packed - 10).saturating_mul(170).min(u32::MAX as usize);
        unpacked = unpacked.min(max);
        if !b.take_bytes(unpacked) {
            return Vec::new();
        }
        let mut data = vec![0u8; unpacked];
        let (mut off, mut remain) = (0usize, unpacked);
        let mut i = 10;
        while i < chunk.len() {
            let kind = chunk[i] as i8;
            i += 1;
            let u8_at = |i: usize| chunk.get(i).copied().unwrap_or(0);
            match kind {
                0 => {
                    let len = u8_at(i) as usize;
                    i = (i + 1).min(chunk.len());
                    if remain >= len && chunk.len() - i >= len {
                        data[off..off + len].copy_from_slice(&chunk[i..i + len]);
                        i += len;
                        off += len;
                        remain -= len;
                    } else {
                        break;
                    }
                }
                1 => {
                    let len = u8_at(i) as usize;
                    i = (i + 1).min(chunk.len());
                    if remain >= len * 4 && chunk.len() - i >= 4 {
                        let dw = [chunk[i], chunk[i + 1], chunk[i + 2], chunk[i + 3]];
                        i += 4;
                        remain -= len * 4;
                        for _ in 0..len {
                            data[off..off + 4].copy_from_slice(&dw);
                            off += 4;
                        }
                    } else {
                        break;
                    }
                }
                2 => {
                    if remain >= 8 && chunk.len() - i >= 4 {
                        let dw = [chunk[i], chunk[i + 1], chunk[i + 2], chunk[i + 3]];
                        i += 4;
                        data[off..off + 4].copy_from_slice(&dw);
                        data[off + 4..off + 8].copy_from_slice(&dw);
                        off += 8;
                        remain -= 8;
                    } else {
                        break;
                    }
                }
                3 => {
                    let len = u8_at(i) as usize;
                    i = (i + 1).min(chunk.len());
                    if remain >= len {
                        off += len;
                        remain -= len;
                    } else {
                        break;
                    }
                }
                _ => break, // -1 ends the data; anything else is an error
            }
        }
        if remain != 0 {
            return Vec::new();
        }
        data
    } else {
        if !b.take_bytes(packed) {
            return Vec::new();
        }
        chunk.to_vec()
    }
}

/// `DecodeSample8`: delta-coded bytes.
fn decode_sample8(r: &mut Reader, b: &mut Budget) -> Vec<u8> {
    let mut data = decode_chunk(r, b);
    let mut last = 0u8;
    for v in data.iter_mut() {
        last = last.wrapping_add(*v);
        *v = last;
    }
    data
}

/// `DecodeSample16`: blocks of 2048 delta-coded LSBs then 2048 MSBs; a partial last block is
/// left as it is.
fn decode_sample16(r: &mut Reader, b: &mut Budget) -> Vec<u8> {
    let mut data = decode_chunk(r, b);
    let mut buf = [0u8; 4096];
    for block in 0..data.len() / 4096 {
        let o = block * 4096;
        let mut last = 0u8;
        for i in 0..2048 {
            last = last.wrapping_add(data[o + i]);
            buf[i * 2 + 1] = last;
        }
        for i in 0..2048 {
            last = last.wrapping_add(data[o + i + 2048]);
            buf[i * 2] = last;
        }
        data[o..o + 4096].copy_from_slice(&buf);
    }
    data
}

/// A decoded sample file: interleaved channels (1 or 2).
struct Raw {
    data: Data,
    channels: usize,
}

/// Interleaved frames of `crate::aiff::Pcm`-style data as 8- or 16-bit samples, the first two
/// channels.
fn pcm_to_raw(d: &[u8], start: usize, frames: usize, channels: usize, bits: u32, enc: crate::aiff::Enc) -> Option<Raw> {
    use crate::aiff::Enc;
    if channels == 0 || bits == 0 || bits > 64 {
        return None;
    }
    let bytes = bits.div_ceil(8) as usize;
    let fsize = bytes.checked_mul(channels)?;
    let frames = frames.min(d.len().saturating_sub(start) / fsize);
    let keep = channels.min(2);
    let n = frames.checked_mul(keep)?;
    let read = |o: usize| -> i32 {
        let s = &d[o..o + bytes];
        match enc {
            Enc::Unsigned8 => (s[0] as i32 - 128) << 8,
            Enc::F32 => (f32::from_be_bytes([s[0], s[1], s[2], s[3]]).clamp(-1.0, 1.0) * 32767.0).round() as i32,
            Enc::F64 => {
                let mut a = [0u8; 8];
                a.copy_from_slice(&s[..8]);
                (f64::from_be_bytes(a).clamp(-1.0, 1.0) * 32767.0).round() as i32
            }
            Enc::Be => {
                let hi = s[0] as i8 as i32;
                if bytes == 1 { hi << 8 } else { (hi << 8) | s[1] as i32 }
            }
            Enc::Le => {
                let hi = s[bytes - 1] as i8 as i32;
                if bytes == 1 { hi << 8 } else { (hi << 8) | s[bytes - 2] as i32 }
            }
        }
    };
    if bits <= 8 && matches!(enc, Enc::Be | Enc::Le | Enc::Unsigned8) {
        let mut v = Vec::with_capacity(n);
        for f in 0..frames {
            for c in 0..keep {
                v.push((read(start + f * fsize + c * bytes) >> 8) as i8);
            }
        }
        Some(Raw { data: Data::I8(v), channels: keep })
    } else {
        let mut v = Vec::with_capacity(n);
        for f in 0..frames {
            for c in 0..keep {
                v.push(read(start + f * fsize + c * bytes) as i16);
            }
        }
        Some(Raw { data: Data::I16(v), channels: keep })
    }
}

/// An AIFF / AIFF-C file of plain PCM (`ReadAIFFSample`'s common case).
fn read_aiff(d: &[u8]) -> Option<Raw> {
    if !crate::aiff::detect(d) {
        return None;
    }
    let aifc = &d[8..12] == b"AIFC";
    let (mut comm, mut ssnd) = (None, None);
    let mut o = 12usize;
    while o + 8 <= d.len() {
        let size = be32(d, o + 4) as usize;
        let body = o + 8;
        let stop = body.saturating_add(size).min(d.len());
        match &d[o..o + 4] {
            b"COMM" => comm = Some((body, stop)),
            b"SSND" => ssnd = Some((body, stop)),
            _ => {}
        }
        o = body.checked_add(size)?.checked_add(size & 1)?;
    }
    let (c, ce) = comm?;
    let (s, se) = ssnd?;
    if ce - c < 18 || se - s < 8 {
        return None;
    }
    let channels = be16(d, c) as usize;
    let frames = be32(d, c + 2) as usize;
    let bits = be16(d, c + 6) as u32;
    let mut enc = crate::aiff::Enc::Be;
    if aifc && ce - c >= 22 {
        enc = match &d[c + 18..c + 22] {
            b"NONE" | b"twos" => crate::aiff::Enc::Be,
            b"sowt" => crate::aiff::Enc::Le,
            _ => return None,
        };
    }
    if channels == 0 || channels > 64 || !(1..=32).contains(&bits) {
        return None;
    }
    let start = s + 8 + be32(d, s) as usize;
    if start > se {
        return None;
    }
    pcm_to_raw(&d[..se], start, frames, channels, bits, enc)
}

/// The sample readers `ReadSymMOD` tries in turn: IFF, WAV, AIFF, then `ReadRawSymSample`
/// (MAESTRO, 16BT or raw signed 8-bit). The IFF 8SVX and RIFF WAVE decoders are this crate's
/// own (`crate::iff8svx`, `crate::riffwave`).
fn read_sample_file(d: &[u8]) -> Option<Raw> {
    let from_pcm = |p: crate::aiff::Pcm| pcm_to_raw(&p.data, p.start, p.frames, p.channels, p.bits, p.enc);
    if crate::iff8svx::detect(d) {
        if let Some(r) = crate::iff8svx::decode(d.to_vec()).ok().and_then(from_pcm) {
            return Some(r);
        }
    }
    if crate::riffwave::detect(d) {
        if let Some(r) = crate::riffwave::decode(d.to_vec()).ok().and_then(from_pcm) {
            return Some(r);
        }
    }
    if let Some(r) = read_aiff(d) {
        return Some(r);
    }
    // ReadRawSymSample
    if d.starts_with(b"MAESTRO") {
        // (a word past the end reads as 0)
        let stereo = d.len() < 16 || be32(d, 12) == 0;
        let channels = if stereo { 2 } else { 1 };
        let body = d.get(24..).unwrap_or(&[]);
        let n = body.len() / (2 * channels) * channels;
        let v: Vec<i16> = (0..n).map(|i| i16::from_be_bytes([body[i * 2], body[i * 2 + 1]])).collect();
        return (!v.is_empty()).then_some(Raw { data: Data::I16(v), channels });
    }
    if d.starts_with(b"16BT") {
        // the magic is read as sample data, then silenced (Symphonie's anti-click took care of it)
        let n = d.len() / 2;
        let mut v: Vec<i16> = (0..n).map(|i| i16::from_be_bytes([d[i * 2], d[i * 2 + 1]])).collect();
        for x in v.iter_mut().take(2) {
            *x = 0;
        }
        return (!v.is_empty()).then_some(Raw { data: Data::I16(v), channels: 1 });
    }
    let v: Vec<i8> = d.iter().map(|&b| b as i8).collect();
    (!v.is_empty()).then_some(Raw { data: Data::I8(v), channels: 1 })
}

/// `SymEvent`: command, note, param, inst. Ordered as OpenMPT orders them (`std::tie`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Event {
    command: u8,
    note: i8,
    param: u8,
    inst: u8,
}

const KEY_ON: u8 = 0;
const VOL_SLIDE_UP: u8 = 1;
const VOL_SLIDE_DOWN: u8 = 2;
const PITCH_SLIDE_UP: u8 = 3;
const PITCH_SLIDE_DOWN: u8 = 4;
const REPLAY_FROM: u8 = 5;
const FROM_AND_PITCH: u8 = 6;
const SET_FROM_ADD: u8 = 7;
const FROM_ADD: u8 = 8;
const SET_SPEED: u8 = 9;
const ADD_PITCH: u8 = 10;
const ADD_VOLUME: u8 = 11;
const TREMOLO: u8 = 12;
const VIBRATO: u8 = 13;
const SAMPLE_VIB: u8 = 14;
const PITCH_SLIDE_TO: u8 = 15;
const RETRIG: u8 = 16;
const EMPHASIS: u8 = 17;
const ADD_HALF_TONE: u8 = 18;
const CV: u8 = 19;
const CV_ADD: u8 = 20;
const FILTER: u8 = 23;
const DSP_ECHO: u8 = 24;
const DSP_DELAY: u8 = 25;

const VOL_COMMAND: u8 = 200;
const STOP_SAMPLE: u8 = 254;
const CONT_SAMPLE: u8 = 253;
const KEY_OFF: u8 = 251;
const SPEED_DOWN: u8 = 250;
const SPEED_UP: u8 = 249;
const SET_PITCH: u8 = 248;
const PITCH_UP: u8 = 247;
const PITCH_DOWN: u8 = 246;
const PITCH_UP2: u8 = 245;
const PITCH_DOWN2: u8 = 244;
const PITCH_UP3: u8 = 243;
const PITCH_DOWN3: u8 = 242;

impl Event {
    fn read(d: &[u8]) -> Event {
        Event { command: d[0], note: d[1] as i8, param: d[2], inst: d[3] }
    }
    fn is_global(&self) -> bool {
        matches!(self.command, SET_SPEED | DSP_ECHO | DSP_DELAY)
            || (self.command == KEY_ON && (self.param == SPEED_UP || self.param == SPEED_DOWN))
    }
}

/// The 20-byte `ViRT` header of a virtual or transwave instrument.
#[derive(Clone, Copy)]
struct VirtHeader {
    valid: bool,
    version: u16,
    num_events: u16,
    event_size: u16,
}

impl VirtHeader {
    fn read(d: &[u8]) -> VirtHeader {
        // id, zero, filler, version, mix info, filler, eos, events, max events, event size
        let valid = &d[0..4] == b"ViRT" && d[4] == 0 && be16(d, 6) <= 1 && be16(d, 12) == 0 && be16(d, 16) == 20;
        VirtHeader { valid, version: be16(d, 6), num_events: be16(d, 14), event_size: be16(d, 18) }
    }
    fn is_virtual(&self) -> bool {
        self.valid && self.version == 0 && self.num_events <= 20 && self.event_size == 4
    }
    fn is_transwave(&self) -> bool {
        self.valid && self.version == 1 && self.num_events == 2 && self.event_size == 10
    }
}

const TYPE_LOOP: i8 = 4;
const TYPE_SUSTAIN: i8 = 8;
const CH_STEREO_L: u8 = 1;
const CH_STEREO_R: u8 = 2;
const PLAY_REVERSE: u8 = 1;
const AS_QUEUE: u8 = 2;
const MIRROR_X: u8 = 4;
const NEW_LOOP_SYSTEM: u8 = 16;
const NO_TRANSPOSE: u8 = 1;
const NO_DSP: u8 = 2;
const SYNC_PLAY: u8 = 4;

/// `SymInstrument` (256 bytes).
#[derive(Clone)]
struct SymInstrument {
    virt: [u8; 128],
    kind: i8,
    loop_start_high: u8,
    loop_len_high: u8,
    num_repetitions: u8,
    channel: u8,
    volume: u8,
    finetune: i8,
    transpose: i8,
    sample_flags: u8,
    filter: i8,
    inst_flags: u8,
    downsample: u8,
    loop_start_fine: u16,
    loop_len_fine: u16,
    filter_flags: u8,
    num_filter_points: u8,
    filter_points: [(u8, u8); 4],
    vol_fade_flag: u8,
    vol_fade_from: u8,
    vol_fade_to: u8,
}

impl SymInstrument {
    fn read(d: &[u8]) -> SymInstrument {
        let mut virt = [0u8; 128];
        virt.copy_from_slice(&d[..128]);
        let b = &d[128..];
        SymInstrument {
            virt,
            kind: b[0] as i8,
            loop_start_high: b[1],
            loop_len_high: b[2],
            num_repetitions: b[3],
            channel: b[4],
            volume: b[6],
            finetune: b[10] as i8,
            transpose: b[11] as i8,
            sample_flags: b[12],
            filter: b[13] as i8,
            inst_flags: b[14],
            downsample: b[15],
            // 16-17 unused, 18 info, 19-20 range, 21 unused
            loop_start_fine: be16(b, 22),
            loop_len_fine: be16(b, 24),
            // 26-31 unused
            filter_flags: b[32],
            num_filter_points: b[33],
            filter_points: [(b[34], b[35]), (b[36], b[37]), (b[38], b[39]), (b[40], b[41])],
            vol_fade_flag: b[42],
            vol_fade_from: b[43],
            vol_fade_to: b[44],
        }
    }
    fn header(&self) -> VirtHeader {
        VirtHeader::read(&self.virt)
    }
    fn is_virtual(&self) -> bool {
        self.header().valid
    }
    fn is_empty(&self) -> bool {
        self.virt[0] == 0 || self.kind < 0
    }
    fn name(&self) -> String {
        let end = self.virt.iter().position(|&c| c == 0).unwrap_or(128);
        self.virt[..end].iter().map(|&c| if (0x20..0x7F).contains(&c) || c >= 0xA0 { c as char } else { ' ' }).collect()
    }
    fn events(&self) -> [Event; 20] {
        let mut e = [Event::default(); 20];
        for (i, x) in e.iter_mut().enumerate() {
            *x = Event::read(&self.virt[20 + i * 4..]);
        }
        e
    }

    /// `GetSampleLoop`: loop start and length in frames.
    fn sample_loop(&self, s: &Sample) -> (u32, u32) {
        if self.kind != TYPE_LOOP && self.kind != TYPE_SUSTAIN {
            return (0, 0);
        }
        let len = s.data.len() as u64;
        let mut ls = self.loop_start_high.min(100) as u64;
        let mut ll = self.loop_len_high.min(100) as u64;
        if self.sample_flags & NEW_LOOP_SYSTEM != 0 {
            ls = (ls << 16) + self.loop_start_fine as u64;
            ll = (ll << 16) + self.loop_len_fine as u64;
            let scale = len as f64 / (100u64 << 16) as f64;
            ls = len.min((ls as f64 * scale) as u64);
            ll = (len - ls).min((ll as f64 * scale) as u64);
        } else if s.data.has_data() {
            // the order of the operations is the original's (UpdateRecalcLoop)
            ls = ((ls << 7) / 100) * (len >> 7);
            ll = (len - ls).min(((ll << 7) / 100) * (len >> 7));
            let threshold = if matches!(s.data, Data::I16(_)) { 6 * 256 } else { 6 };
            let val = |i: u64| -> i32 {
                match &s.data {
                    Data::I8(v) => v.get(i as usize).map_or(0, |&x| x as i32),
                    Data::I16(v) => v.get(i as usize).map_or(0, |&x| x as i32),
                    Data::Empty => 0,
                }
            };
            let at_start = val(ls);
            let mut end = ls + ll;
            while ll > 0 {
                if (val(end) - at_start).abs() < threshold {
                    break;
                }
                end -= 1;
                ll -= 1;
            }
        }
        (ls as u32, ll as u32)
    }

    /// `ConvertToMPT`: the instrument's settings and Symphonie's load-time sample processing.
    fn convert(&self, ins: &mut Instrument, s: &mut Sample, budget: &mut Budget) {
        if !self.is_virtual() {
            ins.name = self.name();
        }
        s.loop_on = false;
        let (ls, ll) = self.sample_loop(s);
        if self.kind == TYPE_LOOP && ll > 0 {
            s.loop_on = true;
            s.loop_start = ls;
            s.loop_end = ls + ll;
        }
        // volume 0..199, default 100: spread over the sample's and the instrument's global volume
        let eff: u8 = if self.volume > 0 && self.volume < 200 { (self.volume.min(100) as u32 * 128 / 100) as u8 } else { 128 };
        s.gv = eff.max(64) / 2;
        ins.gv = eff.min(64);
        s.c5 = 40460;
        let amount = -(self.downsample as f64) + self.transpose as f64 / 12.0 + self.finetune as f64 / (128.0 * 12.0);
        s.c5 = (s.c5 as f64 * 2f64.powf(amount)).round().clamp(0.0, u32::MAX as f64) as u32;
        ins.no_dsp = self.inst_flags & NO_DSP != 0;
        if !s.data.has_data() {
            return;
        }
        if self.sample_flags & PLAY_REVERSE != 0 {
            match &mut s.data {
                Data::I8(v) => v.reverse(),
                Data::I16(v) => v.reverse(),
                Data::Empty => {}
            }
        }
        if self.sample_flags & MIRROR_X != 0 {
            match &mut s.data {
                Data::I8(v) => v.iter_mut().for_each(|x| *x = !*x),
                Data::I16(v) => v.iter_mut().for_each(|x| *x = !*x),
                Data::Empty => {}
            }
        }
        // always 16-bit data for the processing (heavily filtered 8-bit samples)
        let fade = self.vol_fade_flag == 2 && self.vol_fade_from <= 100 && self.vol_fade_to <= 100;
        if let Data::I8(v) = &s.data {
            if self.filter_flags != 0 || fade || self.filter != 0 {
                s.data = Data::I16(v.iter().map(|&x| (x as i16) << 8).collect());
            }
        }
        if let Data::I16(d) = &mut s.data {
            let size = d.len() as i64;
            let passes = self.filter.unsigned_abs() as u64 + (self.filter_flags != 0) as u64;
            if !budget.take_work(passes * size as u64) {
                return;
            }
            if self.filter < 0 {
                for _ in 0..-(self.filter as i32) {
                    let mut mix = d[0] as i32;
                    for x in d.iter_mut() {
                        mix = (*x as i32 - (mix >> 1)) >> 1;
                        *x = mix as i16;
                    }
                }
            }
            if fade {
                let mut amp: i32 = (self.vol_fade_from as i32) << 24;
                let inc = muldivr(self.vol_fade_to as i64 - self.vol_fade_from as i64, 1 << 24, size) as i32;
                for x in d.iter_mut() {
                    *x = muldivr(*x as i64, amp as i64, 100 << 24) as i16;
                    amp = amp.wrapping_add(inc);
                }
            }
            if self.filter_flags != 0 {
                let p = &self.filter_points;
                let mut cutoff: i32 = (p[0].0 as i32) << 23;
                let mut reso: i32 = (p[0].1 as i32) << 23;
                let two = self.num_filter_points > 1;
                let cstep = if two { muldivr(p[1].0 as i64 - p[0].0 as i64, 1 << 23, size) as i32 } else { 0 };
                let rstep = if two { muldivr(p[1].1 as i64 - p[0].1 as i64, 1 << 23, size) as i32 } else { 0 };
                let hp = if self.filter_flags & 2 != 0 { 2 } else { 0 };
                let mut st = [0i32; 3];
                for x in d.iter_mut() {
                    let cc = cutoff / (1 << 23);
                    let rr = reso / (1 << 23);
                    cutoff = cutoff.wrapping_add(cstep);
                    reso = reso.wrapping_add(rstep);
                    st[2] = ((*x as i32) >> 1).wrapping_sub(st[0]);
                    st[1] = st[1].wrapping_add(cc.wrapping_mul(st[2]) >> 8);
                    st[0] = st[0].wrapping_add(cc.wrapping_mul(st[1]) >> 6);
                    st[0] = st[0].wrapping_add(rr.wrapping_mul(st[0]) >> 6);
                    st[0] >>= 2;
                    *x = st[hp].clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                }
            }
            if self.filter > 0 {
                for _ in 0..self.filter {
                    let mut mix = d[0] as i32;
                    for x in d.iter_mut() {
                        mix = (*x as i32 + *x as i32 + mix) / 3;
                        *x = mix as i16;
                    }
                }
            }
        }
        // Symphonie normalises the samples at load time (OpenMPT: to the full range)
        fn normalize<T: Copy + Into<i32>>(v: &mut [T], scale: i32, from: impl Fn(i32) -> T) {
            let (mn, mx) = v.iter().fold((i32::MAX, i32::MIN), |(a, b), &x| (a.min(x.into()), b.max(x.into())));
            let max = (-mn).max(mx);
            if max >= scale || max == 0 {
                return;
            }
            for x in v.iter_mut() {
                *x = from((*x).into() * scale / max);
            }
        }
        match &mut s.data {
            Data::I8(v) => normalize(v, 127, |x| x as i8),
            Data::I16(v) => normalize(v, 32767, |x| x as i16),
            Data::Empty => {}
        }
        // "non-destructive" over-amplification with a hard knee
        if self.volume > 100 && self.volume < 200 {
            let gain = self.volume as i32;
            let knee = 16384 * (200 - gain) / 100;
            let inv = 32768 - knee;
            let amp = |v: i32| {
                if v > knee {
                    (v - knee) * knee / inv + inv
                } else if v < -knee {
                    (v + knee) * knee / inv - inv
                } else {
                    v * inv / knee
                }
            };
            match &mut s.data {
                Data::I8(v) => v.iter_mut().for_each(|x| *x = (amp(*x as i32 * 256) / 256).clamp(-128, 127) as i8),
                Data::I16(v) => v.iter_mut().for_each(|x| *x = amp(*x as i32).clamp(-32768, 32767) as i16),
                Data::Empty => {}
            }
        }
        // last, as Symphonie would do it during playback: a sustain instrument's loop repeated
        s.cue = NO_CUE;
        let reps = self.num_repetitions as u64;
        if self.kind == TYPE_SUSTAIN && reps > 0 && ll > 0 {
            let (ls, ll) = (ls as u64, ll as u64);
            s.cue = (ls + ll * (reps + 1)).min(u32::MAX as u64) as u32;
            let len = s.data.len() as u64;
            let new_len = len + ll * reps;
            if 0x1000_0000 / reps < ll || new_len > 0x1000_0000 || !budget.take_frames((ll * reps) as usize) {
                return;
            }
            fn expand<T: Copy>(v: &[T], ls: usize, ll: usize, reps: usize) -> Vec<T> {
                let mut out = Vec::with_capacity(v.len() + ll * reps);
                out.extend_from_slice(&v[..ls + ll]);
                for _ in 0..reps {
                    out.extend_from_slice(&v[ls..ls + ll]);
                }
                out.extend_from_slice(&v[ls + ll..]);
                out
            }
            let (ls, ll, reps) = (ls as usize, ll as usize, reps as usize);
            if ls + ll > len as usize {
                return;
            }
            s.data = match &s.data {
                Data::I8(v) => Data::I8(expand(v, ls, ll, reps)),
                Data::I16(v) => Data::I16(expand(v, ls, ll, reps)),
                Data::Empty => Data::Empty,
            };
        }
    }
}

/// `Util::muldivr`: (a * b + c / 2) / c, truncated towards zero and saturated to 32 bits.
fn muldivr(a: i64, b: i64, c: i64) -> i64 {
    if c == 0 {
        return 0;
    }
    ((a * b + c / 2) / c).clamp(i32::MIN as i64, i32::MAX as i64)
}

/// `SymVirtualInst::Render`: the instrument's note events mixed (or queued) into one sample.
fn render_virtual(h: &VirtHeader, events: &[Event; 20], as_queue: bool, samples: &[Sample], num_samples: usize, boost: u16, budget: &mut Budget) -> Option<Sample> {
    let n = h.num_events as usize;
    if n < 1 || n > 20 || events[0].inst as usize >= num_samples {
        return None;
    }
    let events = &events[..n];
    let c5_0 = samples[events[0].inst as usize + 1].c5.max(1) as f64;
    let rate = 1.0 / c5_0;
    let mut total: u64 = 0;
    for e in events.iter().take(if as_queue { n } else { 1 }) {
        if e.inst as usize >= num_samples || e.note < 0 {
            continue;
        }
        let src = &samples[e.inst as usize + 1];
        let len = src.data.len() as f64 * 2f64.powf((e.note as f64 - events[0].note as f64) / -12.0) * src.c5 as f64 * rate;
        total = total.saturating_add(len.round().clamp(0.0, u32::MAX as f64) as u64);
    }
    if total == 0 || total > 0x1000_0000 || !budget.take_work(total * n as u64) || !budget.take_frames(total as usize) {
        return None;
    }
    let total = total as usize;
    struct V<'a> {
        src: &'a Data,
        len: usize,
        pos: f64,
        inc: f64,
        start: usize,
        vol: i64,
        on: bool,
    }
    let mut voices = Vec::new();
    let mut last_offset: u64 = 0;
    for e in events {
        if e.inst as usize >= num_samples || e.note < 0 {
            voices.push(None);
            continue;
        }
        let finetune = if (PITCH_DOWN3..=PITCH_UP).contains(&e.param) {
            const PITCH: [i32; 6] = [-4, 4, -2, 2, -1, 1];
            PITCH[(e.param - PITCH_DOWN3) as usize]
        } else {
            0
        };
        let src = &samples[e.inst as usize + 1];
        let inc = 2f64.powf((e.note as f64 - events[0].note as f64) / 12.0 + finetune as f64 / 96.0) * src.c5 as f64 * rate;
        // OpenMPT's 32.32 fixed point
        let inc = (inc * 4294967296.0).round() / 4294967296.0;
        if inc <= 0.0 || !inc.is_finite() {
            voices.push(None);
            continue;
        }
        let len = src.data.len();
        let start = if as_queue {
            let s = last_offset;
            last_offset = last_offset.saturating_add((len as f64 / inc).round().clamp(0.0, u32::MAX as f64) as u64);
            s.min(u32::MAX as u64) as usize
        } else {
            0
        };
        // avoid clipping the filters if the virtual sample is also filtered later (303 emulator)
        let mut vol = 4096 * boost as i64 / 10000;
        if !as_queue {
            vol /= n as i64;
        }
        voices.push(Some(V { src: &src.data, len, pos: 0.0, inc, start, vol, on: len > 0 }));
    }
    let mut out = vec![0i16; total];
    let mut buf = vec![0i64; 512];
    let mut w = 0;
    while w < total {
        let count = (total - w).min(512);
        buf[..count].fill(0);
        for v in voices.iter_mut().flatten() {
            if !v.on {
                continue;
            }
            if v.start >= count {
                v.start -= count;
                continue;
            }
            let left = ((v.len as f64 - v.pos) / v.inc).round().max(0.0) as usize;
            let proc = (count - v.start).min(left);
            for o in &mut buf[v.start..v.start + proc] {
                let i = v.pos as usize;
                let frac = ((v.pos - i as f64) * 65536.0) as i64;
                let a = v.src.at16(i) as i64;
                let b = if i + 1 < v.len { v.src.at16(i + 1) as i64 } else { 0 };
                let s = a + (((b - a) * frac) >> 16);
                *o += s * v.vol;
                v.pos += v.inc;
            }
            v.start = 0;
            if v.pos as usize >= v.len {
                v.on = false;
            }
        }
        for (o, &m) in out[w..w + count].iter_mut().zip(&buf[..count]) {
            *o = ((m + 2048) >> 12).clamp(-32768, 32767) as i16;
        }
        w += count;
    }
    Some(Sample { data: Data::I16(out), ..Sample::default() })
}

/// `SymTranswaveInst`: morphs between two sample loops over 80 cycles.
fn render_transwave(virt: &[u8; 128], s1: &Sample, s2: &Sample, budget: &mut Budget) -> Option<Sample> {
    let point = |o: usize| (be32(virt, o + 4), be32(virt, o + 8));
    let convert = |(ls, ll): (u32, u32), s: &Sample| -> (u64, u64) {
        let len = s.data.len() as u64;
        let scale = len as f64 / (100u64 << 16) as f64;
        let start = (scale * ls.min(100 << 16) as f64) as u64;
        let length = (scale * ll.min(100 << 16) as f64) as u64;
        (start, (len.saturating_sub(start)).min(length))
    };
    let (l1s, l1l) = convert(point(20), s1);
    let (l2s, l2l) = convert(point(36), s2);
    if l1l < 1 || l1l > 0x1000_0000 / (4 * 80) {
        return None;
    }
    let cycle = l1l * 4;
    let total = cycle * 80;
    if !budget.take_frames(total as usize) {
        return None;
    }
    let f1 = l1l as f64 / cycle as f64;
    let f2 = l2l as f64 / cycle as f64;
    let interp = |s: &Sample, off: f64| -> f64 {
        if !s.data.has_data() {
            return 0.0;
        }
        let i = off as usize;
        let f = off - i as f64;
        s.data.at16(i) as f64 * (1.0 - f) + s.data.at16(i + 1) as f64 * f
    };
    let mut out = vec![0i16; total as usize];
    let amp_f = 1.0 / total as f64;
    for i in 0..cycle {
        let v1 = interp(s1, l1s as f64 + i as f64 * f1);
        let v2 = interp(s2, l2s as f64 + i as f64 * f2);
        let mut w = i;
        for _ in 0..80 {
            let amp = w as f64 * amp_f;
            out[w as usize] = (v1 * (1.0 - amp) + v2 * amp).round().clamp(-32768.0, 32767.0) as i16;
            w += cycle;
        }
    }
    Some(Sample { data: Data::I16(out), ..Sample::default() })
}

/// `SymSequence` (16 bytes).
struct Sequence {
    start: u16,
    length: u16,
    info: i16,
    transpose: i16,
}

/// `SymPosition` (32 bytes).
#[derive(Clone, Copy)]
struct Position {
    loop_num: u16,
    pattern: u16,
    start: u16,
    length: u16,
    speed: u16,
    transpose: i16,
}

impl Position {
    /// what OpenMPT keys its converted patterns by
    fn key(&self) -> (u16, u16, u16, i16, u16) {
        (self.pattern, self.start, self.length, self.transpose, self.speed)
    }
}

/// `ConvertDSP`: the macro a Filter or DSP Echo event becomes (DSP Delay: none).
fn convert_dsp(e: &Event) -> Option<Macro> {
    match e.command {
        FILTER => {
            let kind = (e.note as u8) % 5;
            let freq = e.param as f64 * 10000.0 / 240.0;
            // FrequencyToCutOff with the extended filter range
            let cutoff = ((freq.ln() - 4.8737671609324025) * (20.0 / std::f64::consts::LN_2)).clamp(0.0, 127.0).round() as u8;
            let reso = (127.min(e.inst as i32 * 127 / 185)) as u8;
            Some(match kind {
                1 => Macro::Filter { cutoff, reso, highpass: false },
                2 => Macro::Filter { cutoff, reso, highpass: true },
                _ => Macro::FilterOff,
            })
        }
        DSP_ECHO => {
            let kind = if e.note < 5 { e.note as u8 } else { 0 };
            let delay = if e.param < 128 { e.param } else { 127 };
            let feedback = if e.inst < 128 { e.inst } else { 127 };
            Some(Macro::Echo { kind, delay, feedback })
        }
        _ => None,
    }
}

/// `MapToClosestMidiMacro`
fn closest_macro(e: &Event, map: &mut BTreeMap<Event, u8>) -> u8 {
    if e.command == DSP_DELAY {
        return 0;
    }
    let (mut best, mut dist) = (0u8, u32::MAX);
    for (k, &v) in map.iter() {
        if e.command != k.command || e.note != k.note {
            continue;
        }
        let d1 = (e.param as u32).wrapping_sub(k.param as u32);
        let d2 = (e.inst as u32).wrapping_sub(k.inst as u32);
        let d = d1.wrapping_mul(d1).wrapping_add(d2.wrapping_mul(d2));
        if d >= dist {
            continue;
        }
        best = v;
        dist = d;
    }
    map.insert(*e, best);
    best
}

/// `CSoundFile::ReadSymMOD`: the module in `data`, or why it was refused.
pub fn load(data: &[u8]) -> Result<Module, String> {
    if !test(data) {
        return Err("Symphonie: not a SymMOD file".into());
    }
    let num_channels = be32(data, 12) as usize;
    let channels = num_channels.min(MAX_CHANNELS);
    let mut r = Reader { d: data, p: 16 };
    let mut budget = Budget { bytes: MAX_TOTAL_BYTES, frames: MAX_TOTAL_FRAMES, work: MAX_WORK };
    let mut track_len = 0usize;
    let mut tempo = 125.0;
    let mut boost: u16 = 2500;
    let mut pro = false;
    let mut external = false;
    let mut positions: Vec<Position> = Vec::new();
    let mut sequences: Vec<Sequence> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let mut instruments: Vec<SymInstrument> = Vec::new();
    let mut message = String::new();
    // 1-based sample slots; m_nSamples is samples.len() - 1
    let mut samples: Vec<Sample> = vec![Sample::default()];
    let can_add = |s: &Vec<Sample>| s.len() - 1 < MAX_SAMPLES - 1;

    while r.can_read(4) {
        let kind = r.u32() as i32;
        match kind {
            -1 => r.skip(4),
            -2 => {
                track_len = r.u32() as usize;
                if track_len > 1024 {
                    return Err("Symphonie: track length over 1024".into());
                }
            }
            -5 => {
                if r.u32() & 0xFFFF != 4 {
                    return Err("Symphonie: unknown event size".into());
                }
            }
            -6 => {
                // TEMPO's four decimals; SetDefaultTempo makes an integer part of 0 the default 125
                let t = (1.24 * r.u32().min(800) as f64 * 10000.0).round() / 10000.0;
                tempo = if t < 1.0 { 125.0 } else { t };
            }
            -4 | -3 => r.skip(4),
            10 => {
                boost = r.u32().min(10000) as u16;
                pro = true;
            }
            11 | 12 => {
                pro = true;
                let _ = r.u32(); // stereo detune / phase: not supported (as in OpenMPT)
            }
            -7 => {
                r.skip(4);
                if samples.len() == 1 {
                    external = true;
                }
            }
            -10 => {
                if positions.is_empty() {
                    let d = decode_chunk(&mut r, &mut budget);
                    positions = d
                        .chunks_exact(32)
                        .map(|p| Position {
                            loop_num: be16(p, 4),
                            pattern: be16(p, 8),
                            start: be16(p, 10),
                            length: be16(p, 12),
                            speed: be16(p, 14),
                            transpose: be16(p, 16) as i16,
                        })
                        .collect();
                } else {
                    let n = r.u32() as usize;
                    r.skip(n);
                }
            }
            -11 | -17 | -18 => {
                if samples.len() - 1 >= instruments.len() {
                    // (OpenMPT reads on from the chunk's length word)
                    continue;
                }
                if !external && can_add(&samples) {
                    let unpacked = match kind {
                        -11 => {
                            let n = r.u32() as usize;
                            let n = n.min(r.left());
                            let d = r.d[r.p..r.p + n].to_vec();
                            r.p += n;
                            d
                        }
                        -17 => decode_sample8(&mut r, &mut budget),
                        _ => decode_sample16(&mut r, &mut budget),
                    };
                    let slot = samples.len();
                    let mut s = Sample::default();
                    let raw = read_sample_file(&unpacked);
                    let mut right_left = None;
                    if let Some(raw) = raw {
                        if !budget.take_frames(raw.data.len()) {
                            // past the budget: an empty sample
                        } else if raw.channels == 2 {
                            let (l, rr) = split(raw.data);
                            right_left = Some((rr, l));
                        } else {
                            s.data = raw.data;
                        }
                    }
                    if let Some((right, left)) = right_left {
                        // stereo: two consecutive mono samples, right first then left, as
                        // OpenMPT splits them for Symphonie's stereo instrument pairs
                        if can_add(&samples) {
                            s.data = right;
                            samples.push(s);
                            samples.push(Sample { data: left, ..Sample::default() });
                        } else {
                            s.data = left;
                            samples.push(s);
                        }
                    } else {
                        samples.push(s);
                        if slot < instruments.len() && instruments[slot].channel == CH_STEREO_R && can_add(&samples) {
                            // keeps exit.symmod's samples aligned (MoveNextMonoInstrument)
                            samples.push(Sample::default());
                        }
                    }
                } else {
                    let n = r.u32() as usize;
                    r.skip(n);
                }
            }
            -12 => {
                if can_add(&samples) {
                    samples.push(Sample::default());
                }
            }
            -13 => {
                if events.is_empty() {
                    let d = decode_chunk(&mut r, &mut budget);
                    events = d.chunks_exact(4).map(Event::read).collect();
                } else {
                    let n = r.u32() as usize;
                    r.skip(n);
                }
            }
            -14 => {
                if instruments.is_empty() {
                    let d = decode_chunk(&mut r, &mut budget);
                    instruments = d.chunks_exact(256).map(SymInstrument::read).collect();
                } else {
                    let n = r.u32() as usize;
                    r.skip(n);
                }
            }
            -15 => {
                if sequences.is_empty() {
                    let d = decode_chunk(&mut r, &mut budget);
                    sequences = d
                        .chunks_exact(16)
                        .map(|s| Sequence { start: be16(s, 0), length: be16(s, 2), info: be16(s, 6) as i16, transpose: be16(s, 8) as i16 })
                        .collect();
                } else {
                    let n = r.u32() as usize;
                    r.skip(n);
                }
            }
            -16 => {
                let t = decode_chunk(&mut r, &mut budget);
                if !t.is_empty() && message.is_empty() {
                    message = t.iter().take(64 << 10).map(|&c| if c == b'\n' || (0x20..0x7F).contains(&c) || c >= 0xA0 { c as char } else { ' ' }).collect();
                }
            }
            -19 | -20 | -21 => {
                let n = r.u32() as usize;
                r.skip(n);
            }
            _ => {} // unknown (garbage at the end of some files): read on
        }
    }

    if track_len == 0 || instruments.is_empty() {
        return Err("Symphonie: no track length or no instruments".into());
    }
    if positions.is_empty() || events.is_empty() || sequences.is_empty() {
        return Err("Symphonie: no positions, patterns or sequences".into());
    }
    if instruments.len() >= MAX_INSTRUMENTS {
        instruments.truncate(MAX_INSTRUMENTS - 1);
    }
    let num_ins = instruments.len();
    while samples.len() - 1 < num_ins {
        samples.push(Sample::default());
    }
    let mut out_ins: Vec<Option<Instrument>> = vec![None; num_ins + 1];

    // instruments: the regular ones first, then the virtual ones, which mix them
    for pass in 0..2 {
        for ins in 1..=num_ins {
            let sym = &mut instruments[ins - 1];
            if sym.is_empty() || sym.is_virtual() != (pass == 1) {
                continue;
            }
            let num_samples = samples.len() - 1;
            let mut sample = ins;
            let h = sym.header();
            if h.is_virtual() {
                let first = sym.events()[0].inst as usize;
                match render_virtual(&h, &sym.events(), sym.sample_flags & AS_QUEUE != 0, &samples, num_samples, boost, &mut budget) {
                    Some(s) => {
                        samples[sample] = s;
                        if first < num_ins {
                            let ds = instruments[first].downsample;
                            let sym = &mut instruments[ins - 1];
                            sym.downsample = sym.downsample.wrapping_add(ds);
                        }
                    }
                    None => sample = first + 1,
                }
            } else if h.is_transwave() {
                let src1 = be16(&sym.virt, 20) as usize;
                let src2 = be16(&sym.virt, 36) as usize;
                if src1 < num_samples {
                    let empty = Sample::default();
                    let s2 = if src2 < num_samples { &samples[src2 + 1] } else { &empty };
                    if let Some(s) = render_transwave(&sym.virt, &samples[src1 + 1], s2, &mut budget) {
                        samples[sample] = s;
                        // an octave lower than the source, but 4x oversampled
                        sym.transpose = sym.transpose.wrapping_add(12);
                    }
                }
            }
            let sym = instruments[ins - 1].clone();
            let mut instr = Instrument { sample, gv: 64, no_dsp: false, name: String::new() };
            if sample < samples.len() {
                sym.convert(&mut instr, &mut samples[sample], &mut budget);
            }
            out_ins[ins] = Some(instr);
        }
    }

    // patterns
    let mut pattern_map: BTreeMap<(u16, u16, u16, i16, u16), usize> = BTreeMap::new();
    let mut macro_map: BTreeMap<Event, u8> = BTreeMap::new();
    let mut zxx = vec![Macro::Unused; 128];
    let mut use_dsp = false;
    let pattern_size = num_channels * track_len;
    let num_patterns = events.len() / pattern_size;
    let mut patterns: Vec<Pattern> = Vec::new();
    let mut order: Vec<Option<usize>> = Vec::new();
    let mut cells_left = MAX_CELLS;

    #[derive(Clone)]
    struct ChnState {
        vol_slide: f32,
        vol_slide_amt: f32,
        pitch_slide: f32,
        pitch_slide_amt: f32,
        stopped: bool,
        last_note: u8,
        last_inst: u8,
        last_vol: u8,
        channel_vol: u8,
        calc_vol: u8,
        from_add: u8,
        retrig_vibrato: bool,
        cur_vibrato: u8,
        retrig_tremolo: bool,
        cur_tremolo: u8,
        svib_speed: u8,
        svib_depth: u8,
        porta_amt: u8,
        svib_phase: u16,
        retrig_remain: u16,
        porta_remain: u16,
    }
    let mut st = vec![
        ChnState {
            vol_slide: 0.0,
            vol_slide_amt: 0.0,
            pitch_slide: 0.0,
            pitch_slide_amt: 0.0,
            stopped: false,
            last_note: 0,
            last_inst: 0,
            last_vol: 64,
            channel_vol: 100,
            calc_vol: 64,
            from_add: 0,
            retrig_vibrato: false,
            cur_vibrato: 0,
            retrig_tremolo: false,
            cur_tremolo: 0,
            svib_speed: 0,
            svib_depth: 0,
            porta_amt: 0,
            svib_phase: 0,
            retrig_remain: 0,
            porta_remain: 0,
        };
        channels
    ];
    let sat_round_u8 = |x: f64| x.round().clamp(0.0, 255.0) as u8;
    let muldivr_u = |a: u32, b: u32, c: u32| ((a as u64 * b as u64 + c as u64 / 2) / c as u64) as u32;

    'seq: for seq in &sequences {
        if seq.info == 1 {
            continue;
        }
        if seq.info == -1 {
            break;
        }
        let (start, length) = (seq.start as usize, seq.length as usize);
        if start >= positions.len() || length > positions.len() || length == 0 || positions.len() - length < start {
            continue;
        }
        if !order.is_empty() {
            order.push(None);
        }
        for pi in start..start + length {
            // the sequence's transpose, before the map is looked up
            positions[pi].transpose = positions[pi].transpose.wrapping_add(seq.transpose);
            let pos = positions[pi];
            let pat_index;
            if let Some(&p) = pattern_map.get(&pos.key()) {
                pat_index = p;
            } else {
                let rows = pos.length as usize;
                if rows == 0 || rows > MAX_PATTERN_ROWS || patterns.len() >= MAX_PATTERNS || rows * channels > cells_left {
                    break 'seq;
                }
                cells_left -= rows * channels;
                pat_index = patterns.len();
                patterns.push(Pattern { rows, cells: vec![Cell::default(); rows * channels] });
                pattern_map.insert(pos.key(), pat_index);
                if pos.pattern as usize >= num_patterns || pos.start as usize >= track_len {
                    // (OpenMPT leaves the position's transpose changed here)
                    continue;
                }
                let mut speed = pos.speed as u8;
                // this may intentionally read into the next pattern
                let mut src = pos.pattern as usize * pattern_size + pos.start as usize * num_channels;
                let mut sync_cmd = Cell::default();
                let pat = &mut patterns[pat_index];
                for row in 0..rows {
                    let base = row * channels;
                    let mut apply_sync = false;
                    for chn in 0..num_channels {
                        let event = events.get(src).copied().unwrap_or_default();
                        if src < events.len() {
                            src += 1;
                        }
                        if chn >= channels {
                            continue;
                        }
                        let mut note: i8 = if (0..=84).contains(&event.note) { event.note + 25 } else { -1 };
                        let orig_inst = event.inst as usize;
                        let mut mapped: u8 = 0;
                        if orig_inst < num_ins {
                            mapped = orig_inst as u8 + 1;
                            if instruments[orig_inst].inst_flags & NO_TRANSPOSE == 0 && note >= 0 {
                                let n = (note as i32 + pos.transpose as i32) as i8 as i32;
                                note = n.clamp(NOTE_MIN, NOTE_MAX) as i8;
                            }
                        }
                        // a stereo duplicate in this cell gives way to a non-empty event
                        {
                            let m = &mut pat.cells[base + chn];
                            if m.note != NOTE_NONE
                                && (event.command != KEY_ON || event.note != -1 || event.inst != 0 || event.param != 0)
                                && m.instr > 0
                                && (m.instr as usize) <= num_ins
                                && instruments[m.instr as usize - 1].channel == CH_STEREO_R
                            {
                                *m = Cell::default();
                            }
                        }
                        if apply_sync {
                            apply_sync = false;
                            pat.cells[base + chn] = sync_cmd;
                            if pat.cells[base + chn].cmd == Cmd::None && st[chn].calc_vol != st[chn - 1].calc_vol {
                                st[chn].calc_vol = st[chn - 1].calc_vol;
                                let v = st[chn].calc_vol;
                                pat.cells[base + chn].set_fx(Cmd::ChannelVolume, v);
                            }
                            if !event.is_global() {
                                continue;
                            }
                        }
                        let mut apply_volume = false;
                        let mut m = pat.cells[base + chn];
                        let cs = &mut st[chn];
                        let note_u = note as u8;
                        match event.command {
                            KEY_ON => {
                                if event.param > VOL_COMMAND {
                                    match event.param {
                                        STOP_SAMPLE => {
                                            m.set_vol(VolCmd::PlayControl, 0);
                                            cs.stopped = true;
                                        }
                                        CONT_SAMPLE => {
                                            m.set_vol(VolCmd::PlayControl, 1);
                                            cs.stopped = false;
                                        }
                                        KEY_OFF => {
                                            if m.note == NOTE_NONE {
                                                m.note = cs.last_note;
                                            }
                                            m.set_vol(VolCmd::Offset, 1);
                                        }
                                        SPEED_DOWN => {
                                            if speed > 1 {
                                                speed -= 1;
                                                m.set_fx(Cmd::Speed, speed);
                                            }
                                        }
                                        SPEED_UP => {
                                            if speed < 0xFF {
                                                speed += 1;
                                                m.set_fx(Cmd::Speed, speed);
                                            }
                                        }
                                        SET_PITCH => {
                                            cs.last_note = note_u;
                                            if mapped == cs.last_inst {
                                                m.note = note_u;
                                                m.set_fx(Cmd::TonePortaDuration, 0);
                                                cs.pitch_slide = 0.0;
                                                cs.porta_remain = 0;
                                                cs.retrig_vibrato = true;
                                                cs.retrig_tremolo = true;
                                            }
                                        }
                                        // fine portamentos of up to half a semitone
                                        PITCH_UP => m.set_fx(Cmd::PortaUp, 0xF2),
                                        PITCH_DOWN => m.set_fx(Cmd::PortaDown, 0xF2),
                                        PITCH_UP2 => m.set_fx(Cmd::PortaUp, 0xF4),
                                        PITCH_DOWN2 => m.set_fx(Cmd::PortaDown, 0xF4),
                                        PITCH_UP3 => m.set_fx(Cmd::PortaUp, 0xF8),
                                        PITCH_DOWN3 => m.set_fx(Cmd::PortaDown, 0xF8),
                                        _ => {}
                                    }
                                } else if event.note >= 0 || event.param < 100 {
                                    if event.note >= 0 {
                                        m.note = note_u;
                                        cs.last_note = note_u;
                                        m.instr = mapped;
                                        cs.last_inst = mapped;
                                        cs.pitch_slide = 0.0;
                                        cs.porta_remain = 0;
                                        cs.retrig_vibrato = true;
                                        cs.retrig_tremolo = true;
                                    }
                                    if event.param > 0 {
                                        cs.last_vol = sat_round_u8(event.param as f64 * 0.64);
                                        if cs.vol_slide != 0.0 {
                                            apply_volume = true;
                                        }
                                        cs.vol_slide = 0.0;
                                    }
                                }
                                let new_vol = muldivr_u(cs.last_vol as u32, cs.channel_vol as u32, 100) as u8;
                                if apply_volume || cs.calc_vol != new_vol {
                                    cs.calc_vol = new_vol;
                                    m.set_fx(Cmd::ChannelVolume, new_vol);
                                }
                                // a stereo instrument's key-on plays on both channels
                                if event.note > 0
                                    && chn < channels - 1
                                    && chn % 2 == 0
                                    && orig_inst < num_ins
                                    && instruments[orig_inst].channel == CH_STEREO_L
                                {
                                    let mut next = m;
                                    next.instr = next.instr.wrapping_add(1);
                                    pat.cells[base + chn + 1] = next;
                                    let (a, b) = st.split_at_mut(chn + 1);
                                    let (cs, nx) = (&a[chn], &mut b[0]);
                                    nx.last_vol = cs.last_vol;
                                    nx.vol_slide = cs.vol_slide;
                                    nx.vol_slide_amt = cs.vol_slide_amt;
                                    nx.pitch_slide = cs.pitch_slide;
                                    nx.pitch_slide_amt = cs.pitch_slide_amt;
                                    nx.retrig_remain = cs.retrig_remain;
                                }
                            }
                            // Symphonie's very fine slides run at the output rate: approximated
                            // with normal volume commands from the pattern speed
                            VOL_SLIDE_UP => {
                                cs.vol_slide_amt = 0.0;
                                cs.vol_slide = event.param as f32 * 0.0333;
                            }
                            VOL_SLIDE_DOWN => {
                                cs.vol_slide_amt = 0.0;
                                cs.vol_slide = event.param as f32 * -0.0333;
                            }
                            ADD_VOLUME => m.cmd = Cmd::None,
                            TREMOLO => {
                                let sp = 15.min(event.inst >> 3);
                                let dp = 15.min(event.param >> 3);
                                cs.cur_tremolo = (sp << 4) | dp;
                                if cs.cur_tremolo != 0 {
                                    cs.retrig_tremolo = true;
                                } else {
                                    m.set_fx(Cmd::Tremolo, 0);
                                }
                            }
                            PITCH_SLIDE_UP => {
                                cs.pitch_slide_amt = 0.0;
                                cs.pitch_slide = event.param as f32 * 0.0333;
                                cs.porta_remain = 0;
                            }
                            PITCH_SLIDE_DOWN => {
                                cs.pitch_slide_amt = 0.0;
                                cs.pitch_slide = event.param as f32 * -0.0333;
                                cs.porta_remain = 0;
                            }
                            PITCH_SLIDE_TO => {
                                if note >= 0 && event.param > 0 {
                                    let distance = ((note_u as i32 - cs.last_note as i32) * 32).abs();
                                    cs.pitch_slide = 0.0;
                                    m.note = note_u;
                                    cs.last_note = note_u;
                                    cs.porta_amt = (distance / (2 * event.param as i32)).clamp(0, 255) as u8;
                                    m.set_fx(Cmd::TonePorta, cs.porta_amt);
                                    let ticks = cs.porta_amt as i32 * (speed as i32 - 1);
                                    cs.porta_remain = (distance - distance.min(ticks)) as u16;
                                }
                            }
                            ADD_PITCH => m.cmd = Cmd::None, // "about 4 half notes" for -128..127
                            VIBRATO => {
                                let sp = 15.min(event.inst >> 3);
                                let dp = 15.min(event.param);
                                cs.cur_vibrato = (sp << 4) | dp;
                                if cs.cur_vibrato != 0 {
                                    cs.retrig_vibrato = true;
                                } else {
                                    m.set_fx(Cmd::Vibrato, 0);
                                }
                            }
                            ADD_HALF_TONE => {
                                let n = cs.last_note.wrapping_add(event.param) as i32;
                                m.note = n.clamp(NOTE_MIN, NOTE_MAX) as u8;
                                cs.last_note = m.note;
                                m.set_fx(Cmd::TonePortaDuration, 0);
                                cs.porta_remain = 0;
                            }
                            FILTER | DSP_ECHO | DSP_DELAY => {
                                if let Some(&p) = macro_map.get(&event).filter(|&&p| p != 0) {
                                    m.set_fx(Cmd::Midi, p);
                                } else if macro_map.len() < 128 {
                                    let param = macro_map.len() as u8;
                                    if let Some(mac) = convert_dsp(&event) {
                                        zxx[param as usize] = mac;
                                        macro_map.insert(event, 0x80 | param);
                                        m.set_fx(Cmd::Midi, 0x80 | param);
                                        if event.command == DSP_ECHO || event.command == DSP_DELAY {
                                            use_dsp = true;
                                        }
                                    }
                                } else {
                                    let p = closest_macro(&event, &mut macro_map);
                                    if p != 0 {
                                        m.set_fx(Cmd::Midi, p);
                                    }
                                }
                            }
                            RETRIG => {
                                // the note <param> times every <inst>+1 ticks, on into the next rows
                                if event.param >= 1 {
                                    m.set_fx(Cmd::Retrig, 15.min(event.inst as u32 + 1) as u8);
                                    cs.retrig_remain = (event.param as u32 * (event.inst as u32 + 1)) as u16;
                                }
                            }
                            SET_SPEED => {
                                speed = if event.param != 0 { event.param } else { 4 };
                                m.set_fx(Cmd::Speed, speed);
                            }
                            EMPHASIS => m.cmd = Cmd::None, // a fade on the sample level: not done
                            CV => {
                                if event.note == 0 || event.note == 4 {
                                    let pan: u32 = if event.note == 4 { event.inst as u32 } else { 128 };
                                    let vol: u32 = (event.param as u32).min(100);
                                    let vol_l = (vol * 128.min(256 - pan) / 128) as u8;
                                    let vol_r = (vol * pan.min(128) / 128) as u8;
                                    if vol_l != cs.channel_vol {
                                        cs.channel_vol = vol_l;
                                        cs.calc_vol = muldivr_u(cs.last_vol as u32, cs.channel_vol as u32, 100) as u8;
                                        m.set_fx(Cmd::ChannelVolume, cs.calc_vol);
                                    }
                                    if event.note == 4 && chn < channels - 1 && st[chn + 1].channel_vol != vol_r {
                                        st[chn + 1].channel_vol = vol_r;
                                        let cs = &mut st[chn];
                                        cs.calc_vol = muldivr_u(cs.last_vol as u32, cs.channel_vol as u32, 100) as u8;
                                        let v = cs.calc_vol;
                                        pat.cells[base + chn + 1].set_fx(Cmd::ChannelVolume, v);
                                    }
                                }
                            }
                            CV_ADD => m.cmd = Cmd::None,
                            SET_FROM_ADD => {
                                cs.from_add = event.param;
                                cs.svib_speed = 0;
                                cs.svib_depth = 0;
                            }
                            FROM_ADD => cs.from_add = cs.from_add.wrapping_add(event.param),
                            SAMPLE_VIB => {
                                cs.svib_speed = event.inst;
                                cs.svib_depth = event.param;
                            }
                            FROM_AND_PITCH | REPLAY_FROM => {
                                if event.command == FROM_AND_PITCH {
                                    cs.last_note = note_u;
                                    m.instr = mapped;
                                    cs.last_inst = mapped;
                                }
                                m.note = cs.last_note;
                                if note >= 0 {
                                    m.instr = mapped;
                                    cs.last_inst = mapped;
                                }
                                if event.command == REPLAY_FROM {
                                    m.set_vol(VolCmd::TonePorta, 1);
                                }
                                // FromAndPitch with offset 0 is often a key-on that keeps the slides
                                if event.param != 0 || cs.from_add != 0 || cs.svib_depth != 0 {
                                    let mut vib = 0.0;
                                    if cs.svib_depth != 0 {
                                        vib = cs.svib_depth as f64
                                            * ((cs.svib_phase as f64 * (std::f64::consts::PI * 2.0 / 1024.0) + 1.5 * std::f64::consts::PI).sin() - 1.0)
                                            / 4.0;
                                    }
                                    m.set_fx(Cmd::OffsetPercentage, sat_round_u8(event.param as f64 + cs.from_add as f64 + vib));
                                }
                                cs.porta_remain = 0;
                            }
                            _ => {}
                        }
                        let cs = &mut st[chn];
                        // any event that plays a note re-enables the continuous effects
                        if m.note != NOTE_NONE {
                            cs.stopped = false;
                        } else if cs.stopped {
                            pat.cells[base + chn] = m;
                            continue;
                        }
                        if cs.retrig_remain != 0 {
                            cs.retrig_remain = cs.retrig_remain.max(speed as u16) - speed as u16;
                            if m.cmd == Cmd::None {
                                m.set_fx(Cmd::Retrig, 0);
                            }
                        }
                        let spd = speed as f32;
                        if cs.vol_slide != 0.0 {
                            cs.vol_slide_amt += cs.vol_slide * spd;
                            if m.cmd == Cmd::None {
                                if speed > 1 && cs.vol_slide_amt >= spd - 1.0 {
                                    let a = 15.min(sat_round_u8((cs.vol_slide_amt / (spd - 1.0)) as f64));
                                    cs.vol_slide_amt -= (a as f32) * (spd - 1.0);
                                    m.set_fx(Cmd::ChannelVolSlide, a << 4);
                                } else if cs.vol_slide_amt >= 1.0 {
                                    let a = 15.min(sat_round_u8(cs.vol_slide_amt as f64));
                                    cs.vol_slide_amt -= a as f32;
                                    m.set_fx(Cmd::ChannelVolSlide, (a << 4) | 0x0F);
                                } else if speed > 1 && cs.vol_slide_amt <= -(spd - 1.0) {
                                    let a = 15.min(sat_round_u8((-cs.vol_slide_amt / (spd - 1.0)) as f64));
                                    cs.vol_slide_amt += (a as f32) * (spd - 1.0);
                                    m.set_fx(Cmd::ChannelVolSlide, a);
                                } else if cs.vol_slide_amt <= -1.0 {
                                    let a = 14.min(sat_round_u8(-cs.vol_slide_amt as f64));
                                    cs.vol_slide_amt += a as f32;
                                    m.set_fx(Cmd::ChannelVolSlide, a | 0xF0);
                                }
                            }
                        }
                        if cs.pitch_slide != 0.0 {
                            cs.pitch_slide_amt += cs.pitch_slide * spd;
                            if m.cmd == Cmd::None {
                                if speed > 1 && cs.pitch_slide_amt >= spd - 1.0 {
                                    let a = 0xDF.min(sat_round_u8((cs.pitch_slide_amt / (spd - 1.0)) as f64));
                                    cs.pitch_slide_amt -= (a as f32) * (spd - 1.0);
                                    m.set_fx(Cmd::PortaUp, a);
                                } else if cs.pitch_slide_amt >= 1.0 {
                                    let a = 15.min(sat_round_u8(cs.pitch_slide_amt as f64));
                                    cs.pitch_slide_amt -= a as f32;
                                    m.set_fx(Cmd::PortaUp, a | 0xF0);
                                } else if speed > 1 && cs.pitch_slide_amt <= -(spd - 1.0) {
                                    let a = 0xDF.min(sat_round_u8((-cs.pitch_slide_amt / (spd - 1.0)) as f64));
                                    cs.pitch_slide_amt += (a as f32) * (spd - 1.0);
                                    m.set_fx(Cmd::PortaDown, a);
                                } else if cs.pitch_slide_amt <= -1.0 {
                                    let a = 14.min(sat_round_u8(-cs.pitch_slide_amt as f64));
                                    cs.pitch_slide_amt += a as f32;
                                    m.set_fx(Cmd::PortaDown, a | 0xF0);
                                }
                            } else if m.volcmd == VolCmd::None {
                                if speed > 1 && cs.pitch_slide_amt / 4.0 >= spd - 1.0 {
                                    let a = 9.min(sat_round_u8((cs.pitch_slide_amt / (spd - 1.0)) as f64) / 4);
                                    cs.pitch_slide_amt -= (a as f32) * (spd - 1.0) * 4.0;
                                    m.set_vol(VolCmd::PortaUp, a);
                                } else if speed > 1 && cs.pitch_slide_amt / 4.0 <= -(spd - 1.0) {
                                    let a = 9.min(sat_round_u8((-cs.pitch_slide_amt / (spd - 1.0)) as f64) / 4);
                                    cs.pitch_slide_amt += (a as f32) * (spd - 1.0) * 4.0;
                                    m.set_vol(VolCmd::PortaDown, a);
                                }
                            }
                        }
                        if m.cmd == Cmd::None && cs.cur_vibrato != 0 && cs.retrig_vibrato {
                            m.set_fx(Cmd::Vibrato, cs.cur_vibrato);
                            cs.retrig_vibrato = false;
                        }
                        if m.cmd == Cmd::None && cs.cur_tremolo != 0 && cs.retrig_tremolo {
                            m.set_fx(Cmd::Tremolo, cs.cur_tremolo);
                            cs.retrig_tremolo = false;
                        }
                        if m.cmd != Cmd::TonePorta && cs.porta_remain != 0 {
                            if m.cmd == Cmd::None {
                                m.cmd = Cmd::TonePorta;
                            } else {
                                m.volcmd = VolCmd::TonePorta;
                            }
                            let step = (cs.porta_amt as i32 * (speed as i32 - 1)) as u16;
                            cs.porta_remain -= cs.porta_remain.min(step);
                        }
                        cs.svib_phase = (cs.svib_phase as u32 + cs.svib_speed as u32 * speed as u32) as u16 & 1023;
                        pat.cells[base + chn] = m;
                        if chn % 2 == 0 && cs.last_inst != 0 && (cs.last_inst as usize) <= num_ins {
                            let li = &instruments[cs.last_inst as usize - 1];
                            if li.inst_flags & SYNC_PLAY != 0 {
                                sync_cmd = m;
                                apply_sync = true;
                                if sync_cmd.instr != 0 && li.channel == CH_STEREO_L {
                                    sync_cmd.instr = sync_cmd.instr.wrapping_add(1);
                                }
                            }
                        }
                    }
                }
                write_speed(pat, channels, pos.speed as u8);
            }
            let reps = pos.loop_num.max(1) as usize;
            for _ in 0..reps {
                if order.len() >= MAX_ORDERS {
                    break;
                }
                order.push(Some(pat_index));
            }
            // undo the transpose tweak
            positions[pi].transpose = positions[pi].transpose.wrapping_sub(seq.transpose);
        }
    }
    let tracker = if !pro {
        "Symphonie"
    } else if num_ins <= 128 {
        "Symphonie Pro"
    } else {
        "Symphonie Pro 256"
    };
    let _ = external; // external samples (full Amiga paths) are not loaded, as without MPT_EXTERNAL_SAMPLES
    Ok(Module {
        channels,
        patterns,
        order,
        samples,
        instruments: out_ins,
        tempo,
        zxx,
        use_dsp,
        preamp: (512 / channels as u32).clamp(16, 128),
        tracker,
        message,
    })
}

/// A stereo sample's left and right halves.
fn split(d: Data) -> (Data, Data) {
    match d {
        Data::I8(v) => {
            let (l, r): (Vec<i8>, Vec<i8>) = v.chunks_exact(2).map(|p| (p[0], p[1])).unzip();
            (Data::I8(l), Data::I8(r))
        }
        Data::I16(v) => {
            let (l, r): (Vec<i16>, Vec<i16>) = v.chunks_exact(2).map(|p| (p[0], p[1])).unzip();
            (Data::I16(l), Data::I16(r))
        }
        Data::Empty => (Data::Empty, Data::Empty),
    }
}

/// `CPattern::WriteEffect(EffectWriter(CMD_SPEED, speed).Row(0).RetryNextRow())`: the speed on
/// the first row that has it or a free effect column.
fn write_speed(p: &mut Pattern, channels: usize, speed: u8) {
    for row in 0..p.rows {
        let cells = &mut p.cells[row * channels..(row + 1) * channels];
        if cells.iter().any(|c| c.cmd == Cmd::Speed) {
            return;
        }
        if let Some(c) = cells.iter_mut().find(|c| c.cmd == Cmd::None) {
            c.set_fx(Cmd::Speed, speed);
            return;
        }
    }
}
