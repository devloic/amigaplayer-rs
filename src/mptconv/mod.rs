//! PC tracker formats that libxmp 4.7.4 has no loader for, read as libopenmpt 0.8.7 reads them
//! and put into an Impulse Tracker or Scream Tracker 3 module for the libxmp port's own IT / S3M
//! loader (the format OpenMPT itself would save them as, `CSoundFile::GetBestSaveFormat`):
//!
//! - [`mt2`]: MadTracker 2 (`MT20`, `.mt2`), `Load_mt2.cpp`, as IT;
//! - [`dsm`]: Digisound Interface Kit (DSIK) internal format (`RIFF` / `DSMF`, `.dsm`),
//!   `Load_dsm.cpp`, as S3M;
//! - [`ams`]: Velvet Studio (`AMShdr`) and Extreme's Tracker (`Extreme`, `.ams`), `Load_ams.cpp`,
//!   as IT;
//! - [`gt2`]: Graoumf Tracker (`GTK`) and Graoumf Tracker 2 (`GT2`), `Load_gt2.cpp`, as IT.
//! - [`plm`]: Disorder Tracker 2 (`PLM\x1a`, `.plm`), `Load_plm.cpp`, as IT.
//!
//! This file holds the pieces of OpenMPT the three loaders share: the effect commands
//! (`ModCommand`, `EffectCommand`, `VolumeCommand`), the MOD / S3M / IT command readers
//! (`CSoundFile::ConvertModCommand`, `S3MConvert`, `ModCommand::ExtendedMODtoS3MEffect`,
//! `CombineEffects`, `GetEffectWeight`, `ConvertToVolCommand`), the writer of the commands as
//! IT / S3M effect letters (`CSoundFile::S3MSaveConvert`, with the meaning OpenMPT's player
//! gives the command in the source format where the two differ), the sample decoders
//! (`SampleIO`), `ModSample::TransposeToFrequency` and the bounded `FileReader` the loaders read
//! with. The module itself is written by `crate::mo3`'s IT / S3M writers.
//!
//! What the IT or S3M module cannot hold is set on the libxmp module after its loader ran
//! ([`Fixups`]): a tempo that is not a whole number of BPM or above 255, and commands of the
//! source format that IT has no letter for but libxmp has an effect for (key off at a tick,
//! envelope position, volume-column slides above 9), placed in the event's free effect slot.
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, Copyright (c) 2004-2026 OpenMPT Project
//! Developers and Contributors, Olivier Lapicque; see LICENSE-openmpt).

pub mod ams;
pub mod dsm;
pub mod gt2;
pub mod mt2;
pub mod plm;

use crate::mo3::{Cell as NCell, Env, Ins, Kind, Smp, Song};
use crate::xmp::module::{self as xm, ModuleData};

pub const NOTE_NONE: u8 = 0;
pub const NOTE_MIN: u8 = 1;
pub const NOTE_MAX: u8 = 120;
pub const NOTE_MIDDLEC: u8 = 61;
pub const NOTE_KEYOFF: u8 = 0xFF;
pub const NOTE_NOTECUT: u8 = 0xFE;
pub const NOTE_FADE: u8 = 0xFD;

/// The longest sample OpenMPT keeps (`MAX_SAMPLE_LENGTH`).
pub const MAX_SAMPLE_LENGTH: u32 = 0x1000_0000;

/// `EffectCommand`: the commands these loaders write.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Cmd {
    #[default]
    None,
    Arpeggio,
    PortaUp,
    PortaDown,
    TonePorta,
    Vibrato,
    TonePortaVol,
    VibratoVol,
    Tremolo,
    Panning8,
    Offset,
    VolumeSlide,
    PositionJump,
    Volume,
    PatternBreak,
    Retrig,
    Speed,
    Tempo,
    Tremor,
    ModCmdEx,
    S3mCmdEx,
    ChannelVolume,
    ChannelVolSlide,
    GlobalVolume,
    GlobalVolSlide,
    KeyOff,
    FineVibrato,
    Panbrello,
    XFinePortaUpDown,
    PanningSlide,
    SetEnvPosition,
    Midi,
    SmoothMidi,
    DelayCut,
    XParam,
    Dummy,
    ReverseOffset,
}

impl Cmd {
    /// `ModCommand::GetEffectWeight`: the place in OpenMPT's table.
    pub fn weight(self) -> u8 {
        use Cmd::*;
        match self {
            None => 0,
            Dummy => 1,
            XParam => 2,
            SetEnvPosition => 3,
            KeyOff => 5,
            Tremolo => 6,
            FineVibrato => 7,
            Vibrato => 8,
            XFinePortaUpDown => 9,
            Panbrello => 12,
            S3mCmdEx => 13,
            ModCmdEx => 14,
            DelayCut => 15,
            Midi => 16,
            SmoothMidi => 17,
            PanningSlide => 18,
            Panning8 => 19,
            PortaUp => 24,
            PortaDown => 28,
            VolumeSlide => 31,
            VibratoVol => 33,
            Volume => 34,
            ReverseOffset => 37,
            Offset => 39,
            Tremor => 40,
            Retrig => 41,
            Arpeggio => 43,
            TonePorta => 45,
            TonePortaVol => 46,
            ChannelVolSlide => 50,
            ChannelVolume => 51,
            GlobalVolSlide => 52,
            GlobalVolume => 53,
            Tempo => 54,
            Speed => 55,
            PositionJump => 56,
            PatternBreak => 57,
        }
    }
}

/// `VolumeCommand`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VolCmd {
    #[default]
    None,
    Volume,
    Panning,
    VolSlideUp,
    VolSlideDown,
    FineVolUp,
    FineVolDown,
    VibratoSpeed,
    VibratoDepth,
    TonePorta,
    PortaUp,
    PortaDown,
    PlayControl,
    Offset,
}

/// `ModCommand`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cell {
    pub note: u8,
    pub instr: u8,
    pub volcmd: VolCmd,
    pub vol: u8,
    pub cmd: Cmd,
    pub param: u8,
}

/// A pattern: `rows` rows of `channels` cells.
#[derive(Clone)]
pub struct Pattern {
    pub rows: usize,
    pub cells: Vec<Cell>,
}

impl Pattern {
    pub fn new(rows: usize, channels: usize) -> Pattern {
        Pattern { rows, cells: vec![Cell::default(); rows * channels] }
    }
}

/// `CSoundFile::ConvertModCommand`: a MOD / XM effect.
pub fn convert_mod_command(m: &mut Cell, command: u8, param: u8) {
    use Cmd::*;
    const TRANS: [Cmd; 39] = [
        Arpeggio, PortaUp, PortaDown, TonePorta, Vibrato, TonePortaVol, VibratoVol, Tremolo, //
        Panning8, Offset, VolumeSlide, PositionJump, Volume, PatternBreak, ModCmdEx, Tempo, //
        GlobalVolume, GlobalVolSlide, None, None, KeyOff, SetEnvPosition, None, None, //
        None, PanningSlide, None, Retrig, None, Tremor, None, None, //
        Dummy, XFinePortaUpDown, Panbrello, Midi, SmoothMidi, SmoothMidi, XParam,
    ];
    m.cmd = None;
    m.param = param;
    if command == 0 && param == 0 {
        m.cmd = None;
    } else if command == 0x0F && param < 0x20 {
        m.cmd = Speed;
    } else if (command as usize) < TRANS.len() {
        m.cmd = TRANS[command as usize];
        if m.cmd == PatternBreak {
            m.param = (m.param >> 4).wrapping_mul(10).wrapping_add(m.param & 0x0F);
        }
    }
}

/// `CSoundFile::S3MConvert`: an S3M / IT effect letter (1 = A).
pub fn s3m_convert(m: &mut Cell, command: u8, param: u8, from_it: bool) {
    use Cmd::*;
    m.param = param;
    m.cmd = match command | 0x40 {
        b'@' => {
            if m.param != 0 {
                Dummy
            } else {
                None
            }
        }
        b'A' => Speed,
        b'B' => PositionJump,
        b'C' => {
            if !from_it {
                m.param = (m.param >> 4).wrapping_mul(10).wrapping_add(m.param & 0x0F);
            }
            PatternBreak
        }
        b'D' => VolumeSlide,
        b'E' => PortaDown,
        b'F' => PortaUp,
        b'G' => TonePorta,
        b'H' => Vibrato,
        b'I' => Tremor,
        b'J' => Arpeggio,
        b'K' => VibratoVol,
        b'L' => TonePortaVol,
        b'M' => ChannelVolume,
        b'N' => ChannelVolSlide,
        b'O' => Offset,
        b'P' => PanningSlide,
        b'Q' => Retrig,
        b'R' => Tremolo,
        b'S' => S3mCmdEx,
        b'T' => Tempo,
        b'U' => FineVibrato,
        b'V' => GlobalVolume,
        b'W' => GlobalVolSlide,
        b'X' => Panning8,
        b'Y' => Panbrello,
        b'Z' => Midi,
        b'\\' => {
            if from_it {
                SmoothMidi
            } else {
                Midi
            }
        }
        b']' if from_it => DelayCut,
        b'[' if from_it => XParam,
        // ('^', '_': OpenMPT's finetune commands, which nothing here plays)
        0x72 if from_it => KeyOff,
        0x73 if from_it => SetEnvPosition,
        _ => None,
    };
}

/// `ModCommand::ExtendedMODtoS3MEffect`.
pub fn extended_mod_to_s3m(m: &mut Cell) {
    if m.cmd != Cmd::ModCmdEx {
        return;
    }
    m.cmd = Cmd::S3mCmdEx;
    let p = m.param;
    match p & 0xF0 {
        0x00 => m.cmd = Cmd::None,
        0x10 => {
            m.cmd = Cmd::PortaUp;
            m.param |= 0xF0;
        }
        0x20 => {
            m.cmd = Cmd::PortaDown;
            m.param |= 0xF0;
        }
        0x30 => m.param = (p & 0x0F) | 0x10,
        0x40 => m.param = (p & 0x03) | 0x30,
        0x50 => m.param = (p ^ 0x58) | 0x20,
        0x60 => m.param = (p & 0x0F) | 0xB0,
        0x70 => m.param = (p & 0x03) | 0x40,
        0x90 => {
            m.cmd = Cmd::Retrig;
            m.param = p & 0x0F;
        }
        0xA0 => {
            if p & 0x0F != 0 {
                m.cmd = Cmd::VolumeSlide;
                m.param = (p << 4) | 0x0F;
            } else {
                m.cmd = Cmd::None;
            }
        }
        0xB0 => {
            if p & 0x0F != 0 {
                m.cmd = Cmd::VolumeSlide;
                m.param = 0xF0 | (p & 0x0F).min(0x0E);
            } else {
                m.cmd = Cmd::None;
            }
        }
        0xC0 => {
            if p == 0xC0 {
                m.cmd = Cmd::None;
                m.note = NOTE_NOTECUT;
            }
        }
        0xD0 => {
            if p == 0xD0 {
                m.cmd = Cmd::None;
            }
        }
        _ => {}
    }
}

/// `ImpulseTrackerPortaVolCmd`.
const IT_PORTA_VOL: [u8; 10] = [0x00, 0x01, 0x04, 0x08, 0x10, 0x20, 0x40, 0x60, 0x80, 0xFF];

/// `ModCommand::ConvertToVolCommand`.
pub fn convert_to_vol_command(effect: Cmd, param: u8, force: bool) -> (VolCmd, u8) {
    let none = (VolCmd::None, 0);
    match effect {
        Cmd::Volume => (VolCmd::Volume, param.min(64)),
        Cmd::PortaUp => {
            if !force && (param & 3 != 0 || param >= 0xE0) {
                none
            } else {
                (VolCmd::PortaUp, param / 4)
            }
        }
        Cmd::PortaDown => {
            if !force && (param & 3 != 0 || param >= 0xE0) {
                none
            } else {
                (VolCmd::PortaDown, param / 4)
            }
        }
        Cmd::TonePorta => {
            if param >= 0xF0 {
                return (VolCmd::TonePorta, 9);
            }
            for (n, &v) in IT_PORTA_VOL.iter().enumerate() {
                if if force { param <= v } else { param == v } {
                    return (VolCmd::TonePorta, n as u8);
                }
            }
            none
        }
        Cmd::Vibrato => {
            let p = if force {
                (param & 0x0F).min(9)
            } else if (param & 0x0F) > 9 || param & 0xF0 != 0 {
                return none;
            } else {
                param
            };
            (VolCmd::VibratoDepth, p & 0x0F)
        }
        Cmd::FineVibrato => {
            if force {
                (VolCmd::VibratoDepth, 0)
            } else if param != 0 {
                none
            } else {
                (VolCmd::VibratoDepth, param)
            }
        }
        Cmd::Panning8 => (VolCmd::Panning, if param == 255 { 64 } else { param / 4 }),
        Cmd::VolumeSlide => {
            if param == 0 {
                none
            } else if param & 0x0F == 0 {
                (VolCmd::VolSlideUp, param >> 4)
            } else if param & 0xF0 == 0 {
                (VolCmd::VolSlideDown, param)
            } else if param & 0x0F == 0x0F {
                (VolCmd::FineVolUp, param >> 4)
            } else if param & 0xF0 == 0xF0 {
                (VolCmd::FineVolDown, param & 0x0F)
            } else {
                none
            }
        }
        Cmd::S3mCmdEx => match param & 0xF0 {
            0x80 => (VolCmd::Panning, ((param & 0x0F) << 2) + 2),
            0x90 if param >= 0x9E && force => (VolCmd::PlayControl, param - 0x9E + 2),
            _ => none,
        },
        Cmd::ModCmdEx => match param & 0xF0 {
            0x80 => (VolCmd::Panning, ((param & 0x0F) << 2) + 2),
            0xA0 => (VolCmd::FineVolUp, param & 0x0F),
            0xB0 => (VolCmd::FineVolDown, param & 0x0F),
            _ => none,
        },
        _ => none,
    }
}

/// `ModCommand::CombineEffects`: true when the second command was merged into the first.
pub fn combine_effects(e1: &mut Cmd, p1: &mut u8, e2: &mut Cmd, p2: &mut u8) -> bool {
    if *e1 == Cmd::VolumeSlide && (*e2 == Cmd::Vibrato || *e2 == Cmd::TonePortaVol) && *p2 == 0 {
        *e1 = if *e2 == Cmd::Vibrato { Cmd::VibratoVol } else { Cmd::TonePortaVol };
        *e2 = Cmd::None;
        true
    } else if *e2 == Cmd::VolumeSlide && (*e1 == Cmd::Vibrato || *e1 == Cmd::TonePortaVol) && *p1 == 0 {
        *e1 = if *e1 == Cmd::Vibrato { Cmd::VibratoVol } else { Cmd::TonePortaVol };
        *p1 = *p2;
        *e2 = Cmd::None;
        true
    } else if *e1 == Cmd::Offset && *e2 == Cmd::S3mCmdEx && *p2 == 0x9F {
        *e1 = Cmd::ReverseOffset;
        *e2 = Cmd::None;
        true
    } else if *e1 == Cmd::S3mCmdEx && *p1 == 0x9F && *e2 == Cmd::Offset {
        *e1 = Cmd::ReverseOffset;
        *p1 = *p2;
        *e2 = Cmd::None;
        true
    } else {
        false
    }
}

/// `MOD2XMFineTune`.
pub fn mod2xm_finetune(v: u8) -> i32 {
    (v.wrapping_shl(4) as i8) as i32
}

/// `ModSample::TransposeToFrequency(transpose, finetune)`.
pub fn transpose_to_frequency(transpose: i32, finetune: i32) -> u32 {
    (8363.0 * 2f64.powf((transpose as f64 * 128.0 + finetune as f64) / 1536.0)).round() as u32
}

// ---------------------------------------------------------------------------------------------
// Reading

/// OpenMPT's `FileReader` over a byte slice: reads past the end fail (or read zeros) and
/// never panic.
#[derive(Clone)]
pub struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(d: &'a [u8]) -> Reader<'a> {
        Reader { d, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn len(&self) -> usize {
        self.d.len()
    }
    pub fn left(&self) -> usize {
        self.d.len() - self.pos
    }
    pub fn can_read(&self, n: usize) -> bool {
        self.left() >= n
    }
    pub fn rewind(&mut self) {
        self.pos = 0;
    }
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.d.len());
    }
    /// `Skip`: false (and at the end) when fewer than `n` bytes are left.
    pub fn skip(&mut self, n: usize) -> bool {
        if self.can_read(n) {
            self.pos += n;
            true
        } else {
            self.pos = self.d.len();
            false
        }
    }
    /// `ReadStruct` / `ReadArray`: `n` bytes, or None (nothing read) when fewer are left.
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.can_read(n) {
            let s = &self.d[self.pos..self.pos + n];
            self.pos += n;
            Some(s)
        } else {
            None
        }
    }
    /// `ReadStruct` into a zeroed buffer: `n` bytes, zeros when fewer are left.
    pub fn struct_or_zero(&mut self, n: usize) -> (Vec<u8>, bool) {
        match self.bytes(n) {
            Some(b) => (b.to_vec(), true),
            None => (vec![0; n], false),
        }
    }
    /// `ReadStructPartial(size, partial)`: the first `partial` bytes (at most `size`, zeros for
    /// what is missing), then the cursor `partial` bytes further.
    pub fn struct_partial(&mut self, size: usize, partial: usize) -> Vec<u8> {
        let mut v = vec![0u8; size];
        let k = size.min(partial).min(self.left());
        v[..k].copy_from_slice(&self.d[self.pos..self.pos + k]);
        self.skip(partial);
        v
    }
    /// `ReadChunk`: the next `n` bytes (fewer at the end) as a reader of their own.
    pub fn chunk(&mut self, n: usize) -> Reader<'a> {
        let k = n.min(self.left());
        let r = Reader { d: &self.d[self.pos..self.pos + k], pos: 0 };
        self.pos += k;
        r
    }
    pub fn u8(&mut self) -> u8 {
        self.bytes(1).map(|b| b[0]).unwrap_or(0)
    }
    pub fn u16(&mut self) -> u16 {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0)
    }
    pub fn u32(&mut self) -> u32 {
        self.bytes(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0)
    }
    pub fn f64(&mut self) -> f64 {
        self.bytes(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).unwrap_or(0.0)
    }
    /// `ReadSizedString<uint8le>`: a length byte, then that many bytes (None when the length
    /// cannot be read; what is there of a truncated string).
    pub fn sized_string(&mut self) -> Option<Vec<u8>> {
        let n = self.bytes(1)?[0] as usize;
        let k = n.min(self.left());
        let s = self.d[self.pos..self.pos + k].to_vec();
        self.skip(n);
        Some(s)
    }
    /// `ReadNullString`: up to a NUL (consumed) or the end.
    pub fn null_string(&mut self) -> Vec<u8> {
        let mut v = Vec::new();
        while self.pos < self.d.len() {
            let c = self.d[self.pos];
            self.pos += 1;
            if c == 0 {
                break;
            }
            v.push(c);
        }
        v
    }
    /// The bytes from the cursor to the end.
    pub fn rest(&self) -> &'a [u8] {
        &self.d[self.pos..]
    }
}

/// A NUL-terminated (or full) name, trailing spaces dropped (`mpt::String::ReadBuf`).
pub fn name(b: &[u8]) -> Vec<u8> {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let mut v = b[..n].to_vec();
    while v.last() == Some(&b' ') {
        v.pop();
    }
    v
}

/// The sample encodings of `SampleIO` the loaders use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enc {
    Signed,
    Unsigned,
    Delta,
    /// MadTracker 2: delta; in a stereo sample the right channel is a difference from the left.
    Mt2,
}

/// A sample's decoded data: interleaved frames of 1 or 2 channels of 8 or 16-bit values.
#[derive(Clone, Default)]
pub struct SampleData {
    pub data: Vec<i16>,
    pub frames: usize,
    pub channels: usize,
    pub bits16: bool,
}

/// `SampleIO::ReadSample` for raw PCM: `frames` frames (mono, or stereo with the channels one
/// after the other) from `r`, which moves past the bytes read. Beyond OpenMPT: a sample is cut
/// to the data the file holds, so a damaged length cannot allocate more than the file's size.
pub fn read_pcm(r: &mut Reader, frames: u32, bits16: bool, stereo: bool, enc: Enc) -> SampleData {
    let channels = if stereo { 2 } else { 1 };
    let bps = if bits16 { 2 } else { 1 };
    let mut frames = frames.min(MAX_SAMPLE_LENGTH) as usize;
    frames = frames.min(r.left() / (bps * channels) + if r.left() % (bps * channels) != 0 { 1 } else { 0 });
    let mut out = SampleData { data: vec![0; frames * channels], frames, channels, bits16 };
    if frames == 0 {
        return out;
    }
    let src = r.rest();
    let mut used = 0usize;
    for ch in 0..channels {
        let base = ch * frames * bps;
        let avail = src.len().saturating_sub(base).min(frames * bps);
        let n = avail / bps;
        let mut acc: u16 = 0;
        for k in 0..n {
            let o = base + k * bps;
            let raw: u16 = if bits16 { u16::from_le_bytes([src[o], src[o + 1]]) } else { src[o] as u16 };
            let v: i16 = match (enc, bits16) {
                (Enc::Signed, false) => raw as u8 as i8 as i16,
                (Enc::Unsigned, false) => (raw as u8 ^ 0x80) as i8 as i16,
                (Enc::Delta | Enc::Mt2, false) => {
                    acc = (acc as u8).wrapping_add(raw as u8) as u16;
                    acc as u8 as i8 as i16
                }
                (Enc::Signed, true) => raw as i16,
                (Enc::Unsigned, true) => (raw ^ 0x8000) as i16,
                (Enc::Delta | Enc::Mt2, true) => {
                    acc = acc.wrapping_add(raw);
                    acc as i16
                }
            };
            out.data[k * channels + ch] = v;
        }
        used += n * bps;
    }
    if enc == Enc::Mt2 && channels == 2 {
        for f in out.data.chunks_exact_mut(2) {
            f[1] = if bits16 { (f[0] as u16).wrapping_add(f[1] as u16) as i16 } else { (f[0] as u8).wrapping_add(f[1] as u8) as i8 as i16 };
        }
    }
    r.skip(used);
    out
}

// ---------------------------------------------------------------------------------------------
// The module, as OpenMPT holds it, put into IT or S3M

/// `InstrumentEnvelope`: nodes (tick, value 0..64), flags as `crate::mo3::Env` takes them
/// (1 on, 2 sustain, 4 loop, 0x10 filter).
#[derive(Clone, Default)]
pub struct Envelope {
    pub flags: u8,
    pub nodes: Vec<(u16, u8)>,
    pub loop_start: u8,
    pub loop_end: u8,
    pub sus_start: u8,
    pub sus_end: u8,
}

/// `ModInstrument`, the fields these formats set.
#[derive(Clone)]
pub struct Instrument {
    pub name: Vec<u8>,
    /// note (0-based) -> (note 0-based, sample 1-based, 0 = none)
    pub keyboard: [(u8, u16); 120],
    pub fadeout: u32,
    /// IT values: 0 cut, 1 continue, 2 off, 3 fade
    pub nna: u8,
    pub dct: u8,
    pub dna: u8,
    /// 0..64
    pub global_vol: u8,
    /// 0..256, or None
    pub pan: Option<u16>,
    pub vol_env: Envelope,
    pub pan_env: Envelope,
    pub pitch_env: Envelope,
    /// IT IFC / IFR bytes (bit 7: on)
    pub cutoff: u8,
    pub resonance: u8,
}

impl Instrument {
    /// `ModInstrument(sample)`: every note to itself and `sample`.
    pub fn new(sample: u16) -> Instrument {
        let mut keyboard = [(0u8, 0u16); 120];
        for (k, e) in keyboard.iter_mut().enumerate() {
            *e = (k as u8, sample);
        }
        Instrument {
            name: Vec::new(),
            keyboard,
            fadeout: 256,
            nna: 0,
            dct: 0,
            dna: 0,
            global_vol: 64,
            pan: None,
            vol_env: Envelope::default(),
            pan_env: Envelope::default(),
            pitch_env: Envelope::default(),
            cutoff: 0,
            resonance: 0,
        }
    }
}

/// `ModSample`, the fields these formats set.
#[derive(Clone, Default)]
pub struct Sample {
    pub name: Vec<u8>,
    pub filename: Vec<u8>,
    pub c5speed: u32,
    /// 0..256
    pub volume: u16,
    /// 0..64
    pub global_vol: u8,
    /// 0..256, or None
    pub pan: Option<u16>,
    pub surround: bool,
    pub length: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub looped: bool,
    pub pingpong: bool,
    pub bits16: bool,
    pub stereo: bool,
    /// OpenMPT's `VibratoType`: 0 sine, 1 square, 2 ramp up, 3 ramp down, 4 random
    pub vib_type: u8,
    pub vib_sweep: u8,
    pub vib_depth: u8,
    pub vib_rate: u8,
    pub data: SampleData,
}

impl Sample {
    /// `ModSample::Initialize`.
    pub fn new() -> Sample {
        Sample { c5speed: 8363, volume: 256, global_vol: 64, ..Default::default() }
    }

    /// `ModSample::SanitizeLoops` (the loop of the length the data has).
    fn sanitized_loop(&self) -> Option<(u32, u32)> {
        let len = self.data.frames as u32;
        let end = self.loop_end.min(len);
        if self.looped && self.loop_start < end && end > 0 {
            Some((self.loop_start, end))
        } else {
            None
        }
    }
}

/// The module as the loaders leave it (`CSoundFile`), before it is written as IT or S3M.
pub struct Module {
    pub title: Vec<u8>,
    pub channels: usize,
    pub speed: u8,
    /// classic BPM, possibly fractional
    pub tempo: f64,
    /// 0..256 (OpenMPT's `m_nDefaultGlobalVolume`)
    pub global_vol: u32,
    pub preamp: u32,
    pub linear: bool,
    pub old_effects: bool,
    pub compat_gxx: bool,
    /// per channel: 0..256 pan, surround, volume 0..64
    pub chn_pan: Vec<u16>,
    pub chn_surround: Vec<bool>,
    pub chn_vol: Vec<u8>,
    /// pattern indices, 0xFE skip ("+++"), 0xFF end ("---") as in OpenMPT's order list
    pub orders: Vec<u16>,
    pub patterns: Vec<Option<Pattern>>,
    pub instruments: Vec<Option<Instrument>>,
    pub samples: Vec<Sample>,
    /// The semantics of the source format OpenMPT's player follows (see [`Rules`]).
    pub rules: Rules,
}

/// How OpenMPT's player reads some commands in the source format, where IT (or S3M) reads
/// them otherwise.
#[derive(Clone, Copy, Default)]
pub struct Rules {
    /// MOD / XM nibble priority of volume slides (`VolumeSlide`: MT2).
    pub xm_volslides: bool,
    /// no fine portamento in 1xx / 2xx (`UseCombinedPortamentoCommands` false: MT2).
    pub no_fine_porta: bool,
}

/// `OrderIndex` constants of OpenMPT.
pub const ORDER_SKIP: u16 = 0xFFFE;
pub const ORDER_END: u16 = 0xFFFF;

/// A libxmp effect placed in an event's free slot after the IT / S3M loader ran: (order of
/// the written module's pattern, row, channel, effect, parameter).
type Extra = (usize, usize, usize, u8, u8);

/// What the IT / S3M module cannot hold, set on the libxmp module after its loader ran.
#[derive(Default)]
pub struct Fixups {
    /// The initial tempo in classic BPM when it is not a whole number in 32..255: the module's
    /// bpm and time factor are set so that the ticks last what this tempo gives, and the tempo
    /// commands are scaled against the time factor.
    pub tempo: Option<f64>,
    pub extras: Vec<Extra>,
    /// The filter cutoffs and resonances are OpenMPT's of the extended range (MT2): the module
    /// is played with that filter (`ModuleData::mpt_ext_filter`).
    pub ext_filter: bool,
}

impl Fixups {
    pub fn apply(&self, m: &mut ModuleData) {
        m.mpt_ext_filter = self.ext_filter;
        for &(pat, row, ch, fxt, fxp) in &self.extras {
            if pat >= m.module.xxp.len() || ch >= m.module.chn || row >= m.module.xxp[pat].rows {
                continue;
            }
            let e = m.module.event_mut(pat, ch, row);
            if e.f2t == 0 {
                e.f2t = fxt;
                e.f2p = fxp;
            } else if e.fxt == 0 {
                e.fxt = fxt;
                e.fxp = fxp;
            }
        }
        if let Some(t) = self.tempo.filter(|t| t.is_finite() && *t > 0.0) {
            // the ticks last what `t` gives; the time factor scales every tempo, so a tempo
            // command T is written as the T that plays T with it (one equal to the initial tempo
            // plays exactly that)
            let b = t.round().clamp(1.0, 1000.0);
            m.module.bpm = b as i32;
            m.time_factor = xm::DEFAULT_TIME_FACTOR * b / t;
            for track in m.module.xxt.iter_mut() {
                for e in track.event.iter_mut() {
                    for (fxt, fxp) in [(e.fxt, &mut e.fxp), (e.f2t, &mut e.f2p)] {
                        if (fxt == xm::FX_IT_BPM || fxt == xm::FX_S3M_BPM) && *fxp >= 0x20 {
                            *fxp = (*fxp as f64 * b / t).round().clamp(32.0, 255.0) as u8;
                        }
                    }
                }
            }
        }
    }
}

/// `CSoundFile::S3MSaveConvert` (IT: `to_it`), with the meaning the source format's commands
/// have in OpenMPT's player: an effect letter and parameter, and the libxmp effect for what IT
/// has no letter for.
fn save_effect(c: &Cell, to_it: bool, rules: Rules) -> (Option<(u8, u8)>, Option<(u8, u8)>) {
    use Cmd::*;
    let mut param = c.param;
    let letter: u8 = match c.cmd {
        None => 0,
        Dummy => {
            if param != 0 {
                b'@'
            } else {
                0
            }
        }
        Speed => {
            if param == 0 {
                0
            } else {
                b'A'
            }
        }
        PositionJump => b'B',
        PatternBreak => {
            if !to_it {
                param = ((param / 10) << 4) + (param % 10);
            }
            b'C'
        }
        VolumeSlide => {
            if rules.xm_volslides && param != 0 {
                param = if param & 0xF0 != 0 { param & 0xF0 } else { param & 0x0F };
            }
            b'D'
        }
        PortaDown => {
            if rules.no_fine_porta && param >= 0xE0 {
                param = 0xDF;
            }
            b'E'
        }
        PortaUp => {
            if rules.no_fine_porta && param >= 0xE0 {
                param = 0xDF;
            }
            b'F'
        }
        TonePorta => b'G',
        Vibrato => b'H',
        Tremor => b'I',
        Arpeggio => b'J',
        VibratoVol => {
            if rules.xm_volslides && param != 0 {
                param = if param & 0xF0 != 0 { param & 0xF0 } else { param & 0x0F };
            }
            b'K'
        }
        TonePortaVol => {
            if rules.xm_volslides && param != 0 {
                param = if param & 0xF0 != 0 { param & 0xF0 } else { param & 0x0F };
            }
            b'L'
        }
        ChannelVolume => b'M',
        ChannelVolSlide => b'N',
        Offset | ReverseOffset => b'O',
        PanningSlide => b'P',
        Retrig => b'Q',
        Tremolo => b'R',
        S3mCmdEx => b'S',
        Tempo => {
            param = param.max(0x20);
            b'T'
        }
        FineVibrato => b'U',
        GlobalVolume => b'V',
        GlobalVolSlide => b'W',
        Panning8 => b'X',
        Panbrello => b'Y',
        Midi | SmoothMidi => b'Z',
        XFinePortaUpDown => match param & 0xF0 {
            0x10 => {
                param = (param & 0x0F) | 0xE0;
                b'F'
            }
            0x20 => {
                param = (param & 0x0F) | 0xE0;
                b'E'
            }
            0x90 => b'S',
            _ => 0,
        },
        ModCmdEx => {
            let mut m = *c;
            extended_mod_to_s3m(&mut m);
            return save_effect(&m, to_it, rules);
        }
        KeyOff => return (Option::None, Some((xm::FX_KEYOFF, param))),
        SetEnvPosition => return (Option::None, Some((xm::FX_ENVPOS, param))),
        // (`build` puts it in the volume column when that is free or holds a volume)
        Volume => return (Option::None, Some((xm::FX_VOLSET, param.min(64)))),
        DelayCut | XParam => 0,
    };
    if letter == 0 || (!to_it && matches!(letter, b'@' | b'M' | b'N' | b'P' | b'W' | b'Y' | b'Z')) {
        return (Option::None, Option::None);
    }
    (Some((letter - 0x40, param)), Option::None)
}

/// The IT volume column byte of a volume-column command, or the libxmp effect for one whose
/// value IT's column cannot hold.
fn save_volcmd(c: &Cell) -> (Option<u8>, Option<(u8, u8)>) {
    let v = c.vol;
    let col = |base: u8, max: u8, fx: u8| -> (Option<u8>, Option<(u8, u8)>) {
        if v <= max {
            (Some(base + v), None)
        } else {
            (None, Some((fx, v)))
        }
    };
    match c.volcmd {
        VolCmd::None | VolCmd::VibratoSpeed | VolCmd::PlayControl => (None, None),
        VolCmd::Volume => (Some(v.min(64)), None),
        VolCmd::Panning => (Some(128 + v.min(64)), None),
        VolCmd::FineVolUp => col(65, 9, xm::FX_F_VSLIDE_UP_2),
        VolCmd::FineVolDown => col(75, 9, xm::FX_F_VSLIDE_DN_2),
        VolCmd::VolSlideUp => col(85, 9, xm::FX_VSLIDE_UP_2),
        VolCmd::VolSlideDown => col(95, 9, xm::FX_VSLIDE_DN_2),
        VolCmd::PortaDown => (Some(105 + v.min(9)), None),
        VolCmd::PortaUp => (Some(115 + v.min(9)), None),
        VolCmd::TonePorta => (Some(193 + v.min(9)), None),
        VolCmd::VibratoDepth => (Some(203 + v.min(9)), None),
        // OpenMPT's offset column (cue points of 2048 frames, `SetDefaultCuePoints`); 0 repeats
        // the last offset
        VolCmd::Offset => {
            if (1..=9).contains(&v) {
                (None, Some((xm::FX_OFFSET, v * 8)))
            } else {
                (None, None)
            }
        }
    }
}

/// The size of a pattern as `crate::mo3`'s IT writer packs it.
fn it_pattern_size(grid: &[Vec<NCell>]) -> usize {
    let mut n = 0;
    for row in grid {
        for c in row {
            let fx = c.fx.filter(|&(t, p)| t != 0 || p != 0);
            let k = c.note.is_some() as usize + (c.ins != 0) as usize + c.vol.is_some() as usize + 2 * fx.is_some() as usize;
            if k > 0 {
                n += 2 + k;
            }
        }
        n += 1;
    }
    n
}

/// Beyond what IT holds (64 KB of packed pattern): the last rows are emptied until it fits.
fn fit_it_pattern(grid: &mut [Vec<NCell>]) {
    let mut size = it_pattern_size(grid);
    let mut r = grid.len();
    while size > 0xFFFF && r > 0 {
        r -= 1;
        for c in grid[r].iter_mut() {
            *c = NCell::default();
        }
        size = it_pattern_size(grid);
    }
}

/// The envelope as OpenMPT plays it (`InstrumentEnvelope::Sanitize`: the first node at tick 0,
/// no node before the one ahead of it, the loop and sustain within the nodes), with the nodes
/// that share a tick made one: OpenMPT's `GetValueFromPosition` takes the first of them at that
/// tick (libxmp the last, and IT cannot hold two on one tick), and the loop and sustain points
/// that were on one of them on that one.
fn to_env(e: &Envelope, shift: u32) -> Env {
    let mut out = Env { flags: e.flags, ..Default::default() };
    let mut nodes: Vec<(u16, u8)> = Vec::with_capacity(e.nodes.len());
    let mut index = Vec::with_capacity(e.nodes.len());
    for (k, &(t, v)) in e.nodes.iter().enumerate() {
        let t = if k == 0 { 0 } else { t.max(nodes.last().map_or(0, |n: &(u16, u8)| n.0)) };
        if nodes.last().is_none_or(|n| n.0 != t) {
            nodes.push((t, v));
        }
        index.push((nodes.len() - 1) as u8);
    }
    let last = e.nodes.len().saturating_sub(1) as u8;
    let at = |i: u8| index.get(i.min(last) as usize).copied().unwrap_or(0);
    let loop_end = e.loop_end.min(last);
    let sus_end = e.sus_end.min(last);
    let n = nodes.len().min(25);
    out.num = n as u8;
    for (k, &(t, v)) in nodes.iter().take(n).enumerate() {
        out.points[k] = (t as i16, ((v as i32) << shift) as i16);
    }
    out.loop_start = at(e.loop_start.min(loop_end));
    out.loop_end = at(loop_end);
    out.sus_start = at(e.sus_start.min(sus_end));
    out.sus_end = at(sus_end);
    out
}

/// IT's sample vibrato waveform for OpenMPT's `VibratoType`.
fn it_vib_type(t: u8) -> u8 {
    match t {
        1 => 2,
        2 | 3 => 1,
        4 => 3,
        _ => 0,
    }
}

/// The channel panning as `crate::mo3`'s writers take it (127: surround).
fn chn_pan(pan: u16, surround: bool) -> u8 {
    if surround {
        127
    } else {
        match pan.min(255) as u8 {
            127 => 128,
            p => p,
        }
    }
}

/// The module written as IT or S3M (`kind`), with the fixups of what the file cannot hold, or
/// None when it cannot be written.
pub fn build(md: &Module, kind: Kind) -> Option<(Vec<u8>, Fixups)> {
    let to_it = kind == Kind::It;
    let max_ch = if to_it { 64 } else { 32 };
    let channels = md.channels.clamp(1, max_ch);
    let mut fix = Fixups::default();

    // the patterns the order list plays, renumbered from 0 (IT and S3M stop at 0xFE)
    let mut map: Vec<Option<u8>> = vec![None; md.patterns.len()];
    let mut used: Vec<usize> = Vec::new();
    let mut orders: Vec<u8> = Vec::new();
    for &o in &md.orders {
        let b = match o {
            ORDER_SKIP => 0xFE,
            ORDER_END => 0xFF,
            p => {
                let p = p as usize;
                match md.patterns.get(p) {
                    Some(Some(_)) => {
                        if map[p].is_none() && used.len() < 0xFE {
                            map[p] = Some(used.len() as u8);
                            used.push(p);
                        }
                        match map[p] {
                            Some(n) => n,
                            None => 0xFE,
                        }
                    }
                    // a pattern OpenMPT does not have: skipped by its player
                    _ => 0xFE,
                }
            }
        };
        orders.push(b);
    }
    if used.is_empty() {
        return None;
    }

    let mut patterns: Vec<Option<Vec<Vec<NCell>>>> = Vec::with_capacity(used.len());
    for (np, &p) in used.iter().enumerate() {
        let pat = md.patterns[p].as_ref()?;
        let mut grid = Vec::with_capacity(pat.rows);
        for r in 0..pat.rows {
            let mut row = Vec::with_capacity(channels);
            for ch in 0..channels {
                let mut c = pat.cells.get(r * md.channels + ch).copied().unwrap_or_default();
                // OpenMPT's set-volume command (MOD / XM Cxx), which IT and S3M only have in the
                // volume column, as OpenMPT saves it: there, over a volume the column holds (the
                // command is applied after the column on the row's first tick); beside another
                // volume-column command, a libxmp effect in the event's free slot
                if c.cmd == Cmd::Volume && (!to_it || matches!(c.volcmd, VolCmd::None | VolCmd::Volume)) {
                    c.volcmd = VolCmd::Volume;
                    c.vol = c.param.min(64);
                    c.cmd = Cmd::None;
                }
                let mut n = NCell::default();
                n.note = match c.note {
                    NOTE_NONE => None,
                    NOTE_MIN..=NOTE_MAX => {
                        if to_it {
                            Some(c.note - 1)
                        } else if c.note >= 13 {
                            let v = c.note - 13;
                            Some(((v / 12) << 4) | (v % 12))
                        } else {
                            None
                        }
                    }
                    NOTE_KEYOFF => Some(if to_it { 255 } else { 254 }),
                    NOTE_NOTECUT => Some(254),
                    NOTE_FADE => Some(if to_it { 246 } else { 254 }),
                    _ => None,
                };
                n.ins = c.instr;
                let (vol, vx) = if to_it {
                    save_volcmd(&c)
                } else {
                    (if c.volcmd == VolCmd::Volume { Some(c.vol.min(64)) } else { None }, None)
                };
                n.vol = vol;
                let (fx, ex) = save_effect(&c, to_it, md.rules);
                n.fx = fx;
                if to_it {
                    for e in [vx, ex].into_iter().flatten() {
                        fix.extras.push((np, r, ch, e.0, e.1));
                    }
                }
                row.push(n);
            }
            grid.push(row);
        }
        if to_it {
            fit_it_pattern(&mut grid);
        }
        patterns.push(Some(grid));
    }

    let mut chn_pan_a = [128u8; 64];
    let mut chn_vol_a = [64u8; 64];
    for ch in 0..channels {
        chn_pan_a[ch] = chn_pan(md.chn_pan.get(ch).copied().unwrap_or(128), md.chn_surround.get(ch).copied().unwrap_or(false));
        chn_vol_a[ch] = md.chn_vol.get(ch).copied().unwrap_or(64).min(64);
    }

    let samples: Vec<Smp> = md
        .samples
        .iter()
        .map(|s| {
            let lp = s.sanitized_loop();
            let mut flags = 0u16;
            if s.data.bits16 {
                flags |= crate::mo3::sflag::BIT16;
            }
            if lp.is_some() {
                flags |= crate::mo3::sflag::LOOP;
                if s.pingpong {
                    flags |= crate::mo3::sflag::PINGPONG;
                }
            }
            Smp {
                name: s.name.clone(),
                filename: s.filename.clone(),
                freq: s.c5speed,
                transpose: 0,
                volume: ((s.volume.min(256) + 2) / 4) as u8,
                panning: s.pan.map(|p| p.min(256)).unwrap_or(0xFFFF),
                length: s.data.frames as u32,
                loop_start: lp.map(|l| l.0).unwrap_or(0),
                loop_end: lp.map(|l| l.1).unwrap_or(0),
                flags,
                vib_type: it_vib_type(s.vib_type),
                vib_sweep: s.vib_sweep,
                vib_depth: s.vib_depth,
                vib_rate: s.vib_rate,
                global_vol: s.global_vol.min(64),
                sus_start: 0,
                sus_end: 0,
                compressed: 0,
                encoder_delay: 0,
                shared_header: 0,
                data: s.data.data.clone(),
                frames: s.data.frames,
                channels: s.data.channels.max(1),
                bits16: s.data.bits16,
            }
        })
        .collect();

    let sample_mode = md.instruments.is_empty();
    let instruments: Vec<Ins> = md
        .instruments
        .iter()
        .map(|i| {
            let i = i.clone().unwrap_or_else(|| Instrument::new(0));
            let mut map = [(0u16, 0xFFFFu16); 120];
            for (k, e) in map.iter_mut().enumerate() {
                let (n, s) = i.keyboard[k];
                *e = (n.min(119) as u16, if s == 0 { 0xFFFF } else { s - 1 });
            }
            Ins {
                name: i.name.clone(),
                filename: Vec::new(),
                map,
                vol: to_env(&i.vol_env, 0),
                pan: to_env(&i.pan_env, 0),
                pitch: to_env(&i.pitch_env, 5),
                vib: [0; 4],
                fadeout: i.fadeout.min(0xFFFF) as u16,
                global_vol: (i.global_vol.min(64) as u16 * 2) as u8,
                panning: i.pan.map(|p| p.min(256)).unwrap_or(0xFFFF),
                nna: i.nna,
                pps: 0,
                ppc: 60,
                dct: i.dct,
                dca: i.dna,
                vol_swing: 0,
                pan_swing: 0,
                cutoff: i.cutoff,
                resonance: i.resonance,
            }
        })
        .collect();

    let mut flags = 0u32;
    if md.linear {
        flags |= crate::mo3::hflag::LINEAR;
    }
    if md.old_effects {
        flags |= crate::mo3::hflag::IT_OLD_FX;
    }
    if md.compat_gxx {
        flags |= crate::mo3::hflag::IT_COMPAT_GXX;
    }
    let tempo_int = md.tempo.round();
    let whole = (md.tempo - tempo_int).abs() < 1e-9 && (32.0..=255.0).contains(&tempo_int);
    if !whole {
        fix.tempo = Some(md.tempo);
    }
    let song = Song {
        kind,
        title: md.title.clone(),
        channels,
        restart: 0,
        speed: md.speed.max(1),
        tempo: tempo_int.clamp(32.0, 255.0) as u8,
        flags,
        global_vol: if to_it { (md.global_vol.min(256) / 2) as u8 } else { (md.global_vol.min(256) / 4) as u8 },
        pan_separation: 128,
        sample_volume: 0,
        chn_volume: chn_vol_a,
        chn_pan: chn_pan_a,
        sfx_macros: [0; 16],
        fixed_macros: [[0; 2]; 128],
        orders,
        patterns,
        instruments,
        sample_mode,
        samples,
        hertz: true,
        cwtv: if to_it { 0x0214 } else { 0x1321 },
        cmwt: 0x0214,
        tracker: Vec::new(),
        mtm_version: None,
        rows_per_beat: None,
        midi: None,
        mix_volume: Some(md.preamp),
    };
    let out = crate::mo3::write::write(&song)?;
    Some((out, fix))
}

/// Loads `native` (written by [`build`]) with the libxmp port's IT or S3M loader, applies the
/// fixups and names the format `name (the written format)`.
pub fn load_native(m: &mut ModuleData, native: &[u8], kind: Kind, fix: &Fixups, name: &str) -> Result<(), ()> {
    let mut h = crate::xmp::load::Hio::new(native);
    match kind {
        Kind::It => crate::xmp::loaders::it::load(m, &mut h)?,
        Kind::S3m => crate::xmp::loaders::s3m::load(m, &mut h)?,
        _ => return Err(()),
    }
    fix.apply(m);
    m.module.type_ = name.to_string();
    Ok(())
}

/// The whole file of a loader's `Hio`.
pub fn whole_file(f: &mut crate::xmp::load::Hio) -> Vec<u8> {
    let mut file = vec![0u8; f.size()];
    f.seek(0);
    f.read(&mut file);
    file
}

#[cfg(test)]
mod tests {
    use crate::xmp::load::Hio;
    use crate::xmp::module::ModuleData;

    /// A DSIK RIFF DSM file: one channel, one 64-row pattern (C-4, instrument 1, volume 32 and
    /// a volume slide 0x04 on row 0), one signed 8-bit sample of 16 bytes.
    fn synth_dsm() -> Vec<u8> {
        let mut song = [0u8; 192];
        song[..4].copy_from_slice(b"tiny");
        song[36] = 1; // orders
        song[38] = 1; // samples
        song[40] = 1; // patterns
        song[42] = 1; // channels
        (song[44], song[45], song[46], song[47]) = (64, 0x40, 6, 125);
        song[48] = 0x40;
        let mut inst = [0u8; 64 + 16];
        inst[13] = 0x02; // signed
        inst[15] = 64;
        inst[16] = 16;
        inst[32..34].copy_from_slice(&8363u16.to_le_bytes());
        for (i, v) in inst[64..].iter_mut().enumerate() {
            *v = (i as u8).wrapping_mul(16);
        }
        let mut patt = vec![0u8, 0, 0xF0, 49, 1, 32, 0x0A, 0x04];
        patt.extend_from_slice(&[0; 64]);
        let mut f = b"RIFF\0\0\0\0DSMF".to_vec();
        for (magic, body) in [(b"SONG", &song[..]), (b"INST", &inst[..]), (b"PATT", &patt[..])] {
            f.extend_from_slice(magic);
            f.extend_from_slice(&(body.len() as u32).to_le_bytes());
            f.extend_from_slice(body);
        }
        let n = f.len() as u32 - 8;
        f[4..8].copy_from_slice(&n.to_le_bytes());
        f
    }

    #[test]
    fn loads_synthetic_dsm() {
        let f = synth_dsm();
        assert!(super::dsm::test(&f));
        let mut m = ModuleData::default();
        super::dsm::load(&mut m, &mut Hio::new(&f)).unwrap();
        assert!(m.module.type_.starts_with("DSIK RIFF DSM"), "{}", m.module.type_);
        assert_eq!((m.module.chn, m.module.ins, m.module.spd, m.module.bpm), (1, 1, 6, 125));
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.note, e.ins, e.vol), (49 + 12, 1, 32 + 1));
        assert_eq!((e.fxt, e.fxp), (crate::xmp::module::FX_VOLSLIDE, 0x04));
        assert_eq!(m.module.xxs[0].len, 16);
    }

    /// A Dynamic Studio file: one channel, one order, one 16-frame sample, one pattern whose
    /// first row has C-3 (byte 0x38 = 28 * 2), instrument 1, command 0x0C (volume) 0x20 and
    /// second row command 0x08 param 0x14 (default volume slide up).
    fn synth_dynamic() -> Vec<u8> {
        let mut f = b"DSm\x1Atitle".to_vec();
        f.resize(64, 0);
        f[4] = 0x20;
        (f[45], f[46], f[47], f[48], f[49]) = (1, 1, 1, 0, 100);
        f.push(0x08); // channel pan
        f.push(0); // order 0
        f.extend_from_slice(b"TRACK001"); // track name
        let mut sh = [0u8; 32];
        sh[..4].copy_from_slice(b"smp1");
        sh[23] = 16; // length
        sh[26] = 64; // volume
        f.extend_from_slice(&sh);
        let mut pat = vec![0u8; 64 * 4];
        pat[..4].copy_from_slice(&[1, 56, 0x0C, 0x20]);
        pat[4..8].copy_from_slice(&[0, 0, 0x08, 0x14]);
        f.extend_from_slice(&pat);
        f.extend((0..16u8).map(|i| i.wrapping_mul(16)));
        f
    }

    #[test]
    fn loads_synthetic_dynamic_studio() {
        let f = synth_dynamic();
        assert!(super::dsm::test_dynamic(&f));
        assert!(!super::dsm::test(&f));
        let mut m = ModuleData::default();
        super::dsm::load_dynamic(&mut m, &mut Hio::new(&f)).unwrap();
        assert!(m.module.type_.starts_with("Dynamic Studio"), "{}", m.module.type_);
        assert_eq!((m.module.chn, m.module.ins, m.module.spd, m.module.bpm), (1, 1, 6, 125));
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.note, e.ins, e.vol), (28 + 36, 1, 32 + 1));
        assert_eq!(m.module.xxs[0].len, 16);
        let md = super::dsm::read_dynamic(&f).unwrap();
        assert_eq!(md.patterns[0].as_ref().unwrap().cells[1].cmd, super::Cmd::VolumeSlide);
        assert_eq!(md.patterns[0].as_ref().unwrap().cells[1].param, 0x40);
        assert_eq!(md.samples[0].c5speed, 8363);
    }

    #[test]
    fn damaged_dynamic_studio() {
        let f = synth_dynamic();
        for n in 0..f.len() {
            let _ = super::dsm::load_dynamic(&mut ModuleData::default(), &mut Hio::new(&f[..n]));
        }
        for at in 0..f.len() {
            for v in [0x00, 0xFF, f[at].wrapping_add(1), 0x80] {
                let mut g = f.clone();
                g[at] = v;
                let _ = super::dsm::load_dynamic(&mut ModuleData::default(), &mut Hio::new(&g));
            }
        }
    }

    /// Damaged files (cut short, bytes changed) load or are refused, never panic; the same for
    /// the MadTracker 2 and AMS loaders on a DSM file given their magic.
    #[test]
    fn damaged_files() {
        let dsm = synth_dsm();
        let mut ams = b"AMShdr\x1A\x04tiny\x02\x02\x01\x01\x00\x01\x00".to_vec();
        ams.extend_from_slice(&dsm[12..]);
        let mut mt2 = b"MT20\0\0\0\0\x05\x02".to_vec();
        mt2.resize(126 + 256, 0);
        mt2[112] = 1;
        mt2.extend_from_slice(&dsm[12..]);
        type Load = fn(&mut ModuleData, &mut Hio) -> Result<(), ()>;
        let loaders: [(&[u8], Load); 3] =
            [(&dsm, super::dsm::load), (&ams, super::ams::load), (&mt2, super::mt2::load)];
        for (f, load) in loaders {
            for n in 0..f.len() {
                let _ = load(&mut ModuleData::default(), &mut Hio::new(&f[..n]));
            }
            for at in 0..f.len() {
                for v in [0x00, 0xFF, f[at].wrapping_add(1), 0x80] {
                    let mut g = f.to_vec();
                    g[at] = v;
                    let _ = load(&mut ModuleData::default(), &mut Hio::new(&g));
                }
            }
        }
    }

    /// The same on the MT2, DSM and AMS files in `$MPTCONV_FUZZ_IN` (cut at 64 points, 300
    /// bytes changed each).
    #[test]
    #[ignore]
    fn damaged_real_files() {
        let Ok(inp) = std::env::var("MPTCONV_FUZZ_IN") else { return };
        let mut seed = 0x1234_5678u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let load = |f: &[u8]| {
            let mut m = ModuleData::default();
            let mut h = Hio::new(f);
            let _ = if super::mt2::test(f) {
                super::mt2::load(&mut m, &mut h)
            } else if super::dsm::test(f) {
                super::dsm::load(&mut m, &mut h)
            } else {
                super::ams::load(&mut m, &mut h)
            };
        };
        for p in std::fs::read_dir(&inp).unwrap().flatten().map(|e| e.path()) {
            let f = std::fs::read(&p).unwrap();
            println!("{}", p.display());
            for k in 1..64 {
                load(&f[..f.len() * k / 64]);
            }
            for _ in 0..300 {
                let mut g = f.clone();
                let at = rnd() as usize % g.len().min(4096);
                g[at] = rnd() as u8;
                load(&g);
            }
        }
    }
}
