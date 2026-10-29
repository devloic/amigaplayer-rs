//! The format-independent module model of libxmp (`xmp.h`, `common.h`, `effects.h`),
//! which every loader fills in.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp). The field names are libxmp's, so
//! a loader port reads like its C original.

pub const XMP_NAME_SIZE: usize = 64;
pub const XMP_KEY_OFF: u8 = 0x81;
pub const XMP_KEY_CUT: u8 = 0x82;
pub const XMP_KEY_FADE: u8 = 0x83;
pub const XMP_MAX_KEYS: usize = 121;
pub const XMP_MAX_ENV_POINTS: usize = 32;
pub const XMP_MAX_MOD_LENGTH: usize = 256;
pub const XMP_MAX_CHANNELS: usize = 64;
pub const XMP_MIN_BPM: i32 = 20;
pub const XMP_MARK_SKIP: u8 = 0xfe;
pub const XMP_MARK_END: u8 = 0xff;

pub const XMP_CHANNEL_SYNTH: i32 = 1 << 0;
pub const XMP_CHANNEL_MUTE: i32 = 1 << 1;
pub const XMP_CHANNEL_SPLIT: i32 = 1 << 2;
pub const XMP_CHANNEL_SURROUND: i32 = 1 << 4;

pub const XMP_ENVELOPE_ON: i32 = 1 << 0;
pub const XMP_ENVELOPE_SUS: i32 = 1 << 1;
pub const XMP_ENVELOPE_LOOP: i32 = 1 << 2;
pub const XMP_ENVELOPE_FLT: i32 = 1 << 3;
pub const XMP_ENVELOPE_SLOOP: i32 = 1 << 4;
pub const XMP_ENVELOPE_CARRY: i32 = 1 << 5;

pub const XMP_INST_NO_DEFAULT_PAN: i32 = -1;
pub const XMP_INST_NNA_CUT: i32 = 0;
pub const XMP_INST_NNA_CONT: i32 = 1;
pub const XMP_INST_NNA_OFF: i32 = 2;
pub const XMP_INST_NNA_FADE: i32 = 3;
pub const XMP_INST_DCT_OFF: i32 = 0;
pub const XMP_INST_DCT_NOTE: i32 = 1;
pub const XMP_INST_DCT_SMP: i32 = 2;
pub const XMP_INST_DCT_INST: i32 = 3;

pub const XMP_SAMPLE_16BIT: i32 = 1 << 0;
pub const XMP_SAMPLE_LOOP: i32 = 1 << 1;
pub const XMP_SAMPLE_LOOP_BIDIR: i32 = 1 << 2;
pub const XMP_SAMPLE_LOOP_REVERSE: i32 = 1 << 3;
pub const XMP_SAMPLE_LOOP_FULL: i32 = 1 << 4;
pub const XMP_SAMPLE_SLOOP: i32 = 1 << 5;
pub const XMP_SAMPLE_SLOOP_BIDIR: i32 = 1 << 6;
pub const XMP_SAMPLE_STEREO: i32 = 1 << 7;
pub const XMP_SAMPLE_SYNTH: i32 = 1 << 15;

pub const XMP_FLAGS_VBLANK: i32 = 1 << 0;
pub const XMP_FLAGS_FX9BUG: i32 = 1 << 1;
pub const XMP_FLAGS_FIXLOOP: i32 = 1 << 2;
pub const XMP_FLAGS_A500: i32 = 1 << 3;

// common.h
pub const PAL_RATE: f64 = 250.0;
pub const NTSC_RATE: f64 = 208.0;
pub const C4_PAL_RATE: i32 = 8287;
pub const C4_NTSC_RATE: i32 = 8363;
pub const DEFAULT_TIME_FACTOR: f64 = 10.0;
pub const MAX_SEQUENCES: usize = 255;
pub const NO_SEQUENCE: u8 = MAX_SEQUENCES as u8;
pub const MAX_SAMPLE_SIZE: i32 = 0x1000_0000;
pub const MAX_SAMPLES: usize = 1024;

// Quirks
pub const QUIRK_S3MLOOP: i32 = 1 << 0;
pub const QUIRK_ENVFADE: i32 = 1 << 1;
pub const QUIRK_PROTRACK: i32 = 1 << 2;
pub const QUIRK_RTONCE: i32 = 1 << 3;
pub const QUIRK_ST3BUGS: i32 = 1 << 4;
pub const QUIRK_FINEFX: i32 = 1 << 5;
pub const QUIRK_VSALL: i32 = 1 << 6;
pub const QUIRK_PBALL: i32 = 1 << 7;
pub const QUIRK_PERPAT: i32 = 1 << 8;
pub const QUIRK_VOLPDN: i32 = 1 << 9;
pub const QUIRK_UNISLD: i32 = 1 << 10;
pub const QUIRK_ITVPOR: i32 = 1 << 11;
pub const QUIRK_INVLOOP: i32 = 1 << 13;
pub const QUIRK_INSVOL: i32 = 1 << 14;
pub const QUIRK_VIRTUAL: i32 = 1 << 15;
pub const QUIRK_FILTER: i32 = 1 << 16;
pub const QUIRK_IGSTPOR: i32 = 1 << 17;
pub const QUIRK_KEYOFF: i32 = 1 << 18;
pub const QUIRK_VIBHALF: i32 = 1 << 19;
pub const QUIRK_VIBALL: i32 = 1 << 20;
pub const QUIRK_VIBINV: i32 = 1 << 21;
pub const QUIRK_PRENV: i32 = 1 << 22;
pub const QUIRK_ITOLDFX: i32 = 1 << 23;
pub const QUIRK_S3MRTG: i32 = 1 << 24;
pub const QUIRK_FT2ENV: i32 = 1 << 25;
pub const QUIRK_FT2BUGS: i32 = 1 << 26;
pub const QUIRK_MARKER: i32 = 1 << 27;
pub const QUIRK_NOBPM: i32 = 1 << 28;
pub const QUIRK_ARPMEM: i32 = 1 << 29;
pub const QUIRK_RSTCHN: i32 = 1 << 30;

pub const QUIRKS_ST3: i32 = QUIRK_S3MLOOP | QUIRK_VOLPDN | QUIRK_FINEFX | QUIRK_S3MRTG | QUIRK_MARKER | QUIRK_RSTCHN;
pub const QUIRKS_FT2: i32 = 0;
pub const QUIRKS_IT: i32 = QUIRK_S3MLOOP
    | QUIRK_FINEFX
    | QUIRK_VIBALL
    | QUIRK_ENVFADE
    | QUIRK_ITVPOR
    | QUIRK_KEYOFF
    | QUIRK_VIRTUAL
    | QUIRK_FILTER
    | QUIRK_RSTCHN
    | QUIRK_IGSTPOR
    | QUIRK_S3MRTG
    | QUIRK_MARKER;

// Flow modes (pattern loop and jump quirks)
pub const FLOW_LOOP_GLOBAL_TARGET: i32 = 1 << 0;
pub const FLOW_LOOP_GLOBAL_COUNT: i32 = 1 << 1;
pub const FLOW_LOOP_END_ADVANCES: i32 = 1 << 2;
pub const FLOW_LOOP_END_CANCELS: i32 = 1 << 3;
pub const FLOW_LOOP_PATTERN_RESET: i32 = 1 << 4;
pub const FLOW_LOOP_INIT_SAMEROW: i32 = 1 << 5;
pub const FLOW_LOOP_FIRST_EFFECT: i32 = 1 << 6;
pub const FLOW_LOOP_ONE_AT_A_TIME: i32 = 1 << 7;
pub const FLOW_LOOP_IGNORE_TARGET: i32 = 1 << 8;
pub const FLOW_LOOP_DELAY_BREAK: i32 = 1 << 9;
pub const FLOW_LOOP_DELAY_JUMP: i32 = 1 << 10;
pub const FLOW_LOOP_UNSET_BREAK: i32 = 1 << 11;
pub const FLOW_LOOP_UNSET_JUMP: i32 = 1 << 12;
pub const FLOW_LOOP_SHARED_BREAK: i32 = 1 << 13;
pub const FLOW_JUMP_THEN_BREAK: i32 = 1 << 28;
pub const FLOW_JUMP_QUEUED: i32 = 1 << 29;
pub const FLOW_JUMP_NO_ROW_SET: i32 = 1 << 30;
pub const FLOW_MODE_GENERIC: i32 = 0;
pub const FLOW_LOOP_GLOBAL: i32 = FLOW_LOOP_GLOBAL_TARGET | FLOW_LOOP_GLOBAL_COUNT;
pub const FLOW_LOOP_NO_BREAK_JUMP: i32 =
    FLOW_LOOP_DELAY_BREAK | FLOW_LOOP_DELAY_JUMP | FLOW_LOOP_UNSET_BREAK | FLOW_LOOP_UNSET_JUMP;
pub const FLOW_MODE_ST2: i32 = FLOW_JUMP_QUEUED;
pub const FLOW_MODE_ST3_301: i32 = FLOW_LOOP_GLOBAL
    | FLOW_LOOP_PATTERN_RESET
    | FLOW_LOOP_END_ADVANCES
    | FLOW_LOOP_INIT_SAMEROW
    | FLOW_JUMP_NO_ROW_SET;
pub const FLOW_MODE_ST3_321: i32 =
    FLOW_LOOP_GLOBAL | FLOW_LOOP_PATTERN_RESET | FLOW_LOOP_END_ADVANCES | FLOW_LOOP_NO_BREAK_JUMP | FLOW_JUMP_NO_ROW_SET;
pub const FLOW_MODE_IT_100: i32 = FLOW_LOOP_GLOBAL | FLOW_LOOP_UNSET_BREAK | FLOW_LOOP_UNSET_JUMP | FLOW_JUMP_NO_ROW_SET;
pub const FLOW_MODE_IT_104: i32 = FLOW_LOOP_UNSET_BREAK | FLOW_LOOP_UNSET_JUMP | FLOW_JUMP_NO_ROW_SET;
pub const FLOW_MODE_IT_200: i32 = FLOW_MODE_IT_104 | FLOW_LOOP_DELAY_BREAK;
pub const FLOW_MODE_IT_210: i32 = FLOW_MODE_IT_200 | FLOW_LOOP_END_ADVANCES;
pub const FLOW_MODE_MPT_116: i32 = FLOW_LOOP_ONE_AT_A_TIME | FLOW_LOOP_NO_BREAK_JUMP | FLOW_JUMP_NO_ROW_SET;
pub const FLOW_MODE_ORPHEUS: i32 =
    FLOW_LOOP_PATTERN_RESET | FLOW_LOOP_SHARED_BREAK | FLOW_LOOP_UNSET_BREAK | FLOW_JUMP_THEN_BREAK;
pub const FLOW_MODE_LIQUID: i32 = FLOW_LOOP_END_CANCELS | FLOW_LOOP_IGNORE_TARGET | FLOW_LOOP_SHARED_BREAK;
pub const FLOW_MODE_LIQUID_COMPAT: i32 = FLOW_MODE_LIQUID | FLOW_LOOP_PATTERN_RESET;
pub const FLOW_MODE_OCTALYSER: i32 =
    FLOW_LOOP_GLOBAL | FLOW_LOOP_IGNORE_TARGET | FLOW_LOOP_END_CANCELS | FLOW_LOOP_NO_BREAK_JUMP;
pub const FLOW_MODE_DTM_2015: i32 = FLOW_LOOP_GLOBAL | FLOW_LOOP_FIRST_EFFECT | FLOW_LOOP_NO_BREAK_JUMP;
pub const FLOW_MODE_DTM_2015_6CH: i32 =
    FLOW_LOOP_GLOBAL | FLOW_LOOP_FIRST_EFFECT | FLOW_LOOP_DELAY_BREAK | FLOW_LOOP_UNSET_BREAK | FLOW_LOOP_SHARED_BREAK;
pub const FLOW_MODE_DTM_19: i32 = FLOW_LOOP_GLOBAL_TARGET | FLOW_LOOP_UNSET_BREAK | FLOW_LOOP_SHARED_BREAK;

/// `m->read_event_type`: which tracker's note/instrument handling `read_event` follows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadEvent {
    Mod,
    Ft2,
    St3,
    It,
    Med,
}

/// `m->period_type`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PeriodType {
    Amiga,
    ModRng,
    Linear,
    Cspd,
}

// Effects (effects.h)
pub const FX_ARPEGGIO: u8 = 0x00;
pub const FX_PORTA_UP: u8 = 0x01;
pub const FX_PORTA_DN: u8 = 0x02;
pub const FX_TONEPORTA: u8 = 0x03;
pub const FX_VIBRATO: u8 = 0x04;
pub const FX_TONE_VSLIDE: u8 = 0x05;
pub const FX_VIBRA_VSLIDE: u8 = 0x06;
pub const FX_TREMOLO: u8 = 0x07;
pub const FX_SETPAN: u8 = 0x08;
pub const FX_OFFSET: u8 = 0x09;
pub const FX_VOLSLIDE: u8 = 0x0a;
pub const FX_JUMP: u8 = 0x0b;
pub const FX_VOLSET: u8 = 0x0c;
pub const FX_BREAK: u8 = 0x0d;
pub const FX_EXTENDED: u8 = 0x0e;
pub const FX_SPEED: u8 = 0x0f;
pub const FX_GLOBALVOL: u8 = 0x10;
pub const FX_GVOL_SLIDE: u8 = 0x11;
pub const FX_KEYOFF: u8 = 0x14;
pub const FX_ENVPOS: u8 = 0x15;
pub const FX_PANSLIDE: u8 = 0x19;
pub const FX_MULTI_RETRIG: u8 = 0x1b;
pub const FX_TREMOR: u8 = 0x1d;
pub const FX_XF_PORTA: u8 = 0x21;
pub const EX_FILTER: u8 = 0x00;
pub const EX_F_PORTA_UP: u8 = 0x01;
pub const EX_F_PORTA_DN: u8 = 0x02;
pub const EX_GLISS: u8 = 0x03;
pub const EX_VIBRATO_WF: u8 = 0x04;
pub const EX_FINETUNE: u8 = 0x05;
pub const EX_PATTERN_LOOP: u8 = 0x06;
pub const EX_TREMOLO_WF: u8 = 0x07;
pub const EX_SETPAN: u8 = 0x08;
pub const EX_RETRIG: u8 = 0x09;
pub const EX_F_VSLIDE_UP: u8 = 0x0a;
pub const EX_F_VSLIDE_DN: u8 = 0x0b;
pub const EX_CUT: u8 = 0x0c;
pub const EX_DELAY: u8 = 0x0d;
pub const EX_PATT_DELAY: u8 = 0x0e;
pub const EX_INVLOOP: u8 = 0x0f;
pub const XX_XF_PORTA_UP: u8 = 0x01;
pub const XX_XF_PORTA_DN: u8 = 0x02;
pub const FX_OKT_ARP3: u8 = 0x70;
pub const FX_OKT_ARP4: u8 = 0x71;
pub const FX_OKT_ARP5: u8 = 0x72;
pub const FX_NSLIDE2_DN: u8 = 0x73;
pub const FX_NSLIDE2_UP: u8 = 0x74;
pub const FX_F_NSLIDE_DN: u8 = 0x75;
pub const FX_F_NSLIDE_UP: u8 = 0x76;
pub const FX_PER_PORTA_DN: u8 = 0x78;
pub const FX_PER_PORTA_UP: u8 = 0x79;
pub const FX_PER_TPORTA: u8 = 0x7a;
pub const FX_PER_VIBRATO: u8 = 0x7b;
pub const FX_PER_VSLD_UP: u8 = 0x7c;
pub const FX_PER_VSLD_DN: u8 = 0x7d;
pub const FX_SPEED_CP: u8 = 0x7e;
pub const FX_PER_CANCEL: u8 = 0x7f;
pub const FX_669_PORTA_UP: u8 = 0x60;
pub const FX_669_PORTA_DN: u8 = 0x61;
pub const FX_669_TPORTA: u8 = 0x62;
pub const FX_669_FINETUNE: u8 = 0x63;
pub const FX_669_VIBRATO: u8 = 0x64;
pub const FX_FAR_PORTA_UP: u8 = 0x65;
pub const FX_FAR_PORTA_DN: u8 = 0x66;
pub const FX_FAR_TPORTA: u8 = 0x67;
pub const FX_FAR_TEMPO: u8 = 0x68;
pub const FX_FAR_F_TEMPO: u8 = 0x69;
pub const FX_FAR_VIBDEPTH: u8 = 0x6a;
pub const FX_FAR_VIBRATO: u8 = 0x6b;
pub const FX_FAR_SLIDEVOL: u8 = 0x6c;
pub const FX_FAR_RETRIG: u8 = 0x6d;
pub const FX_FAR_DELAY: u8 = 0x6e;
pub const FX_ULT_TEMPO: u8 = 0x5f;
pub const FX_ULT_TPORTA: u8 = 0x6f;
pub const FX_TRK_VOL: u8 = 0x80;
pub const FX_TRK_VSLIDE: u8 = 0x81;
pub const FX_TRK_FVSLIDE: u8 = 0x82;
pub const FX_IT_INSTFUNC: u8 = 0x83;
pub const FX_FLT_CUTOFF: u8 = 0x84;
pub const FX_FLT_RESN: u8 = 0x85;
pub const FX_IT_BPM: u8 = 0x87;
pub const FX_IT_ROWDELAY: u8 = 0x88;
pub const FX_IT_PANSLIDE: u8 = 0x89;
pub const FX_PANBRELLO: u8 = 0x8a;
pub const FX_PANBRELLO_WF: u8 = 0x8b;
pub const FX_HIOFFSET: u8 = 0x8c;
pub const FX_SURROUND: u8 = 0x8d;
pub const FX_IT_BREAK: u8 = 0x8e;
pub const FX_REVERSE: u8 = 0x8f;
pub const FX_MACRO_SET: u8 = 0xbd;
pub const FX_MACRO: u8 = 0xbe;
pub const FX_MACROSMOOTH: u8 = 0xbf;
pub const FX_HOLD_DECAY: u8 = 0x90;
pub const FX_SETPITCH: u8 = 0x91;
pub const FX_VIBRATO2: u8 = 0x92;
pub const FX_MED_RETRIG: u8 = 0x93;
pub const FX_IMF_FPORTA_DN: u8 = 0x9a;
pub const FX_IMF_FPORTA_UP: u8 = 0x9b;
pub const FX_NSLIDE_DN: u8 = 0x9c;
pub const FX_NSLIDE_UP: u8 = 0x9d;
pub const FX_NSLIDE_R_UP: u8 = 0x9e;
pub const FX_NSLIDE_R_DN: u8 = 0x9f;
pub const FX_VOLSLIDE_UP: u8 = 0xa0;
pub const FX_VOLSLIDE_DN: u8 = 0xa1;
pub const FX_ICE_SPEED: u8 = 0xa2;
pub const FX_S3M_SPEED: u8 = 0xa3;
pub const FX_VOLSLIDE_2: u8 = 0xa4;
pub const FX_F_VSLIDE: u8 = 0xa5;
pub const FX_FINETUNE: u8 = 0xa6;
pub const FX_CHORUS: u8 = 0xa9;
pub const FX_REVERB: u8 = 0xaa;
pub const FX_S3M_BPM: u8 = 0xab;
pub const FX_FINE_VIBRATO: u8 = 0xac;
pub const FX_F_VSLIDE_UP: u8 = 0xad;
pub const FX_F_VSLIDE_DN: u8 = 0xae;
pub const FX_F_PORTA_UP: u8 = 0xaf;
pub const FX_F_PORTA_DN: u8 = 0xb0;
pub const FX_MED_HOLD: u8 = 0xb1;
pub const FX_MEGAARP: u8 = 0xb2;
pub const FX_PATT_DELAY: u8 = 0xb3;
pub const FX_S3M_ARPEGGIO: u8 = 0xb4;
pub const FX_PANSL_NOMEM: u8 = 0xb5;
pub const FX_VOL_ADD: u8 = 0xb6;
pub const FX_VOL_SUB: u8 = 0xb7;
pub const FX_PITCH_ADD: u8 = 0xb8;
pub const FX_PITCH_SUB: u8 = 0xb9;
pub const FX_LINE_JUMP: u8 = 0xba;
pub const FX_RETRIG: u8 = 0xbb;
pub const FX_VSLIDE_UP_2: u8 = 0xc0;
pub const FX_VSLIDE_DN_2: u8 = 0xc1;
pub const FX_F_VSLIDE_UP_2: u8 = 0xc2;
pub const FX_F_VSLIDE_DN_2: u8 = 0xc3;

pub fn msn(x: u8) -> u8 {
    (x & 0xf0) >> 4
}

pub fn lsn(x: u8) -> u8 {
    x & 0x0f
}

/// `struct xmp_event`: one cell of a track.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Event {
    pub note: u8,
    pub ins: u8,
    pub vol: u8,
    pub fxt: u8,
    pub fxp: u8,
    pub f2t: u8,
    pub f2p: u8,
    pub flag: u8,
}

#[derive(Clone, Default, Debug)]
pub struct Track {
    pub rows: usize,
    pub event: Vec<Event>,
}

impl Track {
    pub fn new(rows: usize) -> Track {
        Track { rows, event: vec![Event::default(); rows] }
    }
}

/// `struct xmp_pattern`: its rows and a track number per channel.
#[derive(Clone, Default, Debug)]
pub struct Pattern {
    pub rows: usize,
    pub index: Vec<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    pub flg: i32,
    pub npt: i32,
    pub scl: i32,
    pub sus: i32,
    pub sue: i32,
    pub lps: i32,
    pub lpe: i32,
    pub data: [i16; XMP_MAX_ENV_POINTS * 2],
}

impl Default for Envelope {
    fn default() -> Self {
        Envelope { flg: 0, npt: 0, scl: 0, sus: 0, sue: 0, lps: 0, lpe: 0, data: [0; XMP_MAX_ENV_POINTS * 2] }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct SubInstrument {
    pub vol: i32,
    pub gvl: i32,
    pub pan: i32,
    pub xpo: i32,
    pub fin: i32,
    pub vwf: i32,
    pub vde: i32,
    pub vra: i32,
    pub vsw: i32,
    pub rvv: i32,
    pub sid: i32,
    pub nna: i32,
    pub dct: i32,
    pub dca: i32,
    pub ifc: i32,
    pub ifr: i32,
}

/// The key map entry of an instrument: subinstrument and transpose.
#[derive(Clone, Copy, Default, Debug)]
pub struct KeyMap {
    pub ins: u8,
    pub xpo: i8,
}

#[derive(Clone, Debug)]
pub struct Instrument {
    pub name: String,
    pub vol: i32,
    pub nsm: i32,
    pub rls: i32,
    pub aei: Envelope,
    pub pei: Envelope,
    pub fei: Envelope,
    pub map: [KeyMap; XMP_MAX_KEYS],
    pub sub: Vec<SubInstrument>,
}

impl Default for Instrument {
    fn default() -> Self {
        Instrument {
            name: String::new(),
            vol: 0,
            nsm: 0,
            rls: 0,
            aei: Envelope::default(),
            pei: Envelope::default(),
            fei: Envelope::default(),
            map: [KeyMap::default(); XMP_MAX_KEYS],
            sub: Vec::new(),
        }
    }
}

/// Guard bytes before a sample's data (libxmp's `xxs->data` points 4 bytes into
/// its allocation, for interpolation reading before the start).
pub const SAMPLE_GUARD: usize = 4;

/// `struct xmp_sample`. `data` holds the guard bytes, the sample (16-bit ones as
/// native little-endian words, stereo ones interleaved) and libxmp's extra frames
/// after it, as `libxmp_load_sample` lays them out; `None` for no data.
#[derive(Clone, Default, Debug)]
pub struct Sample {
    pub name: String,
    pub len: i32,
    pub lps: i32,
    pub lpe: i32,
    pub flg: i32,
    pub data: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug)]
pub struct ChannelInfo {
    pub pan: i32,
    pub vol: i32,
    pub flg: i32,
}

impl Default for ChannelInfo {
    fn default() -> Self {
        ChannelInfo { pan: 0x80, vol: 0x40, flg: 0 }
    }
}

/// `struct xmp_module`.
#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub type_: String,
    pub pat: usize,
    pub trk: usize,
    pub chn: usize,
    pub ins: usize,
    pub smp: usize,
    pub spd: i32,
    pub bpm: i32,
    pub len: usize,
    pub rst: usize,
    pub gvl: i32,
    pub xxp: Vec<Pattern>,
    pub xxt: Vec<Track>,
    pub xxi: Vec<Instrument>,
    pub xxs: Vec<Sample>,
    pub xxc: [ChannelInfo; XMP_MAX_CHANNELS],
    pub xxo: [u8; XMP_MAX_MOD_LENGTH],
}

impl Default for Module {
    fn default() -> Self {
        Module {
            name: String::new(),
            type_: String::new(),
            pat: 0,
            trk: 0,
            chn: 4,
            ins: 0,
            smp: 0,
            spd: 6,
            bpm: 125,
            len: 0,
            rst: 0,
            gvl: 0,
            xxp: Vec::new(),
            xxt: Vec::new(),
            xxi: Vec::new(),
            xxs: Vec::new(),
            xxc: [ChannelInfo::default(); XMP_MAX_CHANNELS],
            xxo: [0; XMP_MAX_MOD_LENGTH],
        }
    }
}

impl Module {
    /// `EVENT(pat, chn, row)`.
    pub fn event(&self, pat: usize, chn: usize, row: usize) -> &Event {
        &self.xxt[self.xxp[pat].index[chn]].event[row]
    }

    pub fn event_mut(&mut self, pat: usize, chn: usize, row: usize) -> &mut Event {
        let t = self.xxp[pat].index[chn];
        &mut self.xxt[t].event[row]
    }

    /// `libxmp_init_pattern` + `libxmp_alloc_pattern_tracks` for every pattern:
    /// `pat` patterns of `rows` rows, each with its own `chn` tracks.
    pub fn alloc_pattern_tracks(&mut self, num: usize, rows: usize) {
        let base = num * self.chn;
        if self.xxt.len() < base + self.chn {
            self.xxt.resize(base + self.chn, Track::default());
        }
        for i in 0..self.chn {
            self.xxt[base + i] = Track::new(rows);
        }
        if self.xxp.len() <= num {
            self.xxp.resize(num + 1, Pattern::default());
        }
        self.xxp[num] = Pattern { rows, index: (base..base + self.chn).collect() };
    }
}

/// `struct ord_data`: what the scan found at the start of an order.
#[derive(Clone, Copy, Debug)]
pub struct OrdData {
    pub time: f64,
    pub speed: i32,
    pub bpm: i32,
    pub gvl: i32,
    pub start_row: i32,
    pub st26_speed: i32,
}

impl Default for OrdData {
    fn default() -> Self {
        OrdData { time: -1.0, speed: 0, bpm: 0, gvl: 0, start_row: 0, st26_speed: 0 }
    }
}

/// `struct extra_sample_data`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExtraSample {
    pub c5spd: f64,
    pub sus: i32,
    pub sue: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Sequence {
    pub entry_point: usize,
    pub duration: i32,
}

/// `struct module_data`: the module and how it is to be played.
#[derive(Clone, Debug)]
pub struct ModuleData {
    pub module: Module,
    pub comment: String,
    pub rrate: f64,
    pub time_factor: f64,
    pub c4rate: i32,
    pub volbase: i32,
    pub gvolbase: i32,
    pub gvol: i32,
    pub mvolbase: i32,
    pub mvol: i32,
    /// Volume translation table (PTM, ARCH, COCO).
    pub vol_table: Option<&'static [i32]>,
    pub quirk: i32,
    pub flow_mode: i32,
    pub read_event_type: ReadEvent,
    pub period_type: PeriodType,
    pub smpctl: i32,
    pub defpan: i32,
    pub xxo_info: [OrdData; XMP_MAX_MOD_LENGTH],
    pub num_sequences: usize,
    pub seq_data: [Sequence; MAX_SEQUENCES],
    pub xtra: Vec<ExtraSample>,
    pub compare_vblank: bool,
    /// Scan counters, a row each per order (`m->scan_cnt`).
    pub scan_cnt: Vec<Vec<u8>>,
}

impl Default for ModuleData {
    /// The state `libxmp_load_prologue` leaves for a loader.
    fn default() -> Self {
        let mut m = ModuleData {
            module: Module::default(),
            comment: String::new(),
            rrate: PAL_RATE,
            time_factor: DEFAULT_TIME_FACTOR,
            c4rate: C4_PAL_RATE,
            volbase: 0x40,
            gvolbase: 0x40,
            gvol: 0x40,
            mvolbase: 0,
            mvol: 0,
            vol_table: None,
            quirk: 0,
            flow_mode: FLOW_MODE_GENERIC,
            read_event_type: ReadEvent::Mod,
            period_type: PeriodType::Amiga,
            smpctl: 0,
            defpan: 100,
            xxo_info: [OrdData::default(); XMP_MAX_MOD_LENGTH],
            num_sequences: 0,
            seq_data: [Sequence::default(); MAX_SEQUENCES],
            xtra: Vec::new(),
            compare_vblank: false,
            scan_cnt: Vec::new(),
        };
        for i in 0..XMP_MAX_CHANNELS {
            let pan = (((i as i32 + 1) / 2) % 2) * 0xff;
            m.module.xxc[i].pan = 0x80 + (pan - 0x80) * m.defpan / 100;
            m.module.xxc[i].vol = 0x40;
            m.module.xxc[i].flg = 0;
        }
        m
    }
}

impl ModuleData {
    pub fn has_quirk(&self, q: i32) -> bool {
        self.quirk & q != 0
    }

    pub fn has_flow_mode(&self, f: i32) -> bool {
        self.flow_mode & f != 0
    }

    /// `DEFPAN(x)` of loader.h.
    pub fn defpan(&self, x: i32) -> i32 {
        0x80 + (x - 0x80) * self.defpan / 100
    }

    /// `libxmp_init_instrument`: `ins` instruments and `smp` samples, the samples'
    /// C5 speed the module's current `c4rate`.
    pub fn init_instrument(&mut self) {
        let (ins, smp) = (self.module.ins, self.module.smp);
        self.module.xxi = vec![Instrument::default(); ins];
        self.module.xxs = vec![Sample::default(); smp];
        self.xtra = vec![ExtraSample { c5spd: self.c4rate as f64, sus: 0, sue: 0 }; smp];
    }
}

/// libxmp's `libxmp_copy_adjust`: the first `n` bytes of `r` up to a NUL, with
/// non-printable bytes turned into dots and the trailing spaces dropped.
pub fn copy_adjust(r: &[u8], n: usize) -> String {
    let s: String = r
        .iter()
        .take(n)
        .take_while(|&&b| b != 0)
        .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
        .collect();
    s.trim_end().to_string()
}
