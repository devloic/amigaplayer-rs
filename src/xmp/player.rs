//! libxmp's player core: `player.c` (frame loop, envelopes, volume, pitch and pan
//! processing), `read_event.c` (`read_event_mod`, `read_event_ft2`, `read_event_st3`), `lfo.c`,
//! `period.c`, `flow.c` and the RNG of `rng.c`.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp). The functions keep libxmp's names
//! and order of operations (the float arithmetic included), so the output can be
//! compared with libxmp's sample for sample.

use super::filter::filter_setup;
use super::mixer::{
    DSP_EFFECT_CUTOFF, DSP_EFFECT_FILTER_A0, DSP_EFFECT_FILTER_B0, DSP_EFFECT_FILTER_B1, DSP_EFFECT_RESONANCE, MixerData, MixerVoice,
    PAN_SURROUND,
};
use super::module::*;

// channel flags (player.h)
pub const VOL_SLIDE: i32 = 1 << 0;
pub const PAN_SLIDE: i32 = 1 << 1;
pub const TONEPORTA: i32 = 1 << 2;
pub const PITCHBEND: i32 = 1 << 3;
pub const VIBRATO: i32 = 1 << 4;
pub const TREMOLO: i32 = 1 << 5;
pub const FINE_VOLS: i32 = 1 << 6;
pub const FINE_BEND: i32 = 1 << 7;
pub const OFFSET: i32 = 1 << 8;
pub const TRK_VSLIDE: i32 = 1 << 9;
pub const TRK_FVSLIDE: i32 = 1 << 10;
pub const NEW_INS: i32 = 1 << 11;
pub const NEW_VOL: i32 = 1 << 12;
pub const VOL_SLIDE_2: i32 = 1 << 13;
pub const NOTE_SLIDE: i32 = 1 << 14;
pub const FINE_NSLIDE: i32 = 1 << 15;
pub const NEW_NOTE: i32 = 1 << 16;
pub const FINE_TPORTA: i32 = 1 << 17;
pub const RETRIG: i32 = 1 << 18;
pub const PANBRELLO: i32 = 1 << 19;
pub const GVOL_SLIDE: i32 = 1 << 20;
pub const TEMPO_SLIDE: i32 = 1 << 21;
pub const VENV_PAUSE: i32 = 1 << 22;
pub const PENV_PAUSE: i32 = 1 << 23;
pub const FENV_PAUSE: i32 = 1 << 24;
pub const FINE_VOLS_2: i32 = 1 << 25;
pub const KEY_OFF: i32 = 1 << 26;
pub const TREMOR: i32 = 1 << 27;
pub const MIDI_MACRO: i32 = 1 << 28;

// note flags
pub const NOTE_FADEOUT: i32 = 1 << 0;
pub const NOTE_ENV_RELEASE: i32 = 1 << 1;
pub const NOTE_END: i32 = 1 << 2;
pub const NOTE_CUT: i32 = 1 << 3;
pub const NOTE_ENV_END: i32 = 1 << 4;
pub const NOTE_SAMPLE_END: i32 = 1 << 5;
pub const NOTE_SET: i32 = 1 << 6;
pub const NOTE_SUSEXIT: i32 = 1 << 7;
pub const NOTE_KEY_CUT: i32 = 1 << 8;
pub const NOTE_GLISSANDO: i32 = 1 << 9;
pub const NOTE_SAMPLE_RELEASE: i32 = 1 << 10;
pub const NOTE_RELEASE: i32 = NOTE_ENV_RELEASE | NOTE_SAMPLE_RELEASE;

pub const TREMOR_SUPPRESS: u8 = 0x40;
pub const TREMOR_ON: u8 = 0x80;

// virtual.h
pub const VIRT_ACTION_CUT: i32 = XMP_INST_NNA_CUT;
pub const VIRT_ACTION_CONT: i32 = XMP_INST_NNA_CONT;
pub const VIRT_ACTION_OFF: i32 = XMP_INST_NNA_OFF;
pub const VIRT_ACTION_FADE: i32 = XMP_INST_NNA_FADE;
pub const VIRT_ACTIVE: i32 = 0x100;
pub const VIRT_INVALID: i32 = -1;

// period.h
pub const PERIOD_BASE: f64 = 13696.0;
pub const MAX_PERIOD_L: f64 = 0x1e00 as f64;
pub const MIN_PERIOD_L: f64 = 0.0;
pub const MIN_NOTE_MOD: i32 = 48;
pub const MAX_NOTE_MOD: i32 = 83;

pub(crate) const ROWDELAY_ON: i32 = 1 << 0;
pub(crate) const ROWDELAY_FIRST_FRAME: i32 = 1 << 1;

/// Multi retrigger volume changes: add, multiply, divide.
const RVAL: [(i32, i32, i32); 17] = [
    (0, 1, 1),
    (-1, 1, 1),
    (-2, 1, 1),
    (-4, 1, 1),
    (-8, 1, 1),
    (-16, 1, 1),
    (0, 2, 3),
    (0, 1, 2),
    (0, 1, 1),
    (1, 1, 1),
    (2, 1, 1),
    (4, 1, 1),
    (8, 1, 1),
    (16, 1, 1),
    (0, 3, 2),
    (0, 2, 1),
    (0, 0, 1),
];

#[derive(Clone, Copy, Default, Debug)]
pub struct Lfo {
    pub type_: i32,
    pub rate: i32,
    pub depth: i32,
    pub phase: i32,
}

impl Lfo {
    pub fn update(&mut self) {
        self.phase += self.rate;
        self.phase &= WAVEFORM_SIZE - 1;
    }
}

const WAVEFORM_SIZE: i32 = 64;
const SINE_WAVE: [i32; 64] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141,
    120, 97, 74, 49, 24, 0, -24, -49, -74, -97, -120, -141, -161, -180, -197, -212, -224, -235, -244, -250, -253, -255, -253, -250,
    -244, -235, -224, -212, -197, -180, -161, -141, -120, -97, -74, -49, -24,
];

#[derive(Clone, Copy, Default, Debug)]
pub struct LfoMem {
    pub lfo: Lfo,
    pub memory: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Arpeggio {
    pub val: [i8; 16],
    pub size: i32,
    pub count: i32,
    pub memory: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct InsVib {
    pub lfo: Lfo,
    pub sweep: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Offset {
    pub val: i32,
    pub val2: i32,
    pub memory: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Retrig {
    pub val: i32,
    pub count: i32,
    pub type_: i32,
    pub limit: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Tremor {
    pub up: u8,
    pub down: u8,
    pub count: u8,
    pub memory: u8,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Vol {
    pub slide: i32,
    pub fslide: i32,
    pub slide2: i32,
    pub memory: i32,
    pub fslide2: i32,
    pub memory2: i32,
    pub target: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct UpDown {
    pub up_memory: i32,
    pub down_memory: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Slide3 {
    pub slide: i32,
    pub fslide: i32,
    pub memory: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Freq {
    pub slide: i32,
    pub fslide: f64,
    pub memory: i32,
    pub down_memory: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Porta {
    pub target: f64,
    pub dir: i32,
    pub slide: i32,
    pub memory: i32,
    pub note_memory: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct FinePorta {
    pub up_memory: i32,
    pub down_memory: i32,
    pub xf_up_memory: i32,
    pub xf_down_memory: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Pan {
    pub val: i32,
    pub slide: i32,
    pub fslide: i32,
    pub memory: i32,
    pub surround: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct InvLoop {
    pub speed: i32,
    pub count: i32,
    pub pos: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Filter {
    pub cutoff: i32,
    pub resonance: i32,
    pub envelope: i32,
    pub can_disable: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Macro {
    pub val: f32,
    pub target: f32,
    pub slide: f32,
    pub active: i32,
    pub finalvol: i32,
    pub notepan: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct NoteSlide {
    pub slide: i32,
    pub fslide: i32,
    pub speed: i32,
    pub count: i32,
}

/// `struct channel_data`: a track's (or a virtual channel's) player state.
#[derive(Clone, Copy, Default, Debug)]
pub struct ChannelData {
    pub extra: super::extras::ChannelExtra,
    pub flags: i32,
    pub per_flags: i32,
    pub note_flags: i32,
    pub note: i32,
    pub key: i32,
    pub period: f64,
    pub per_adj: f64,
    pub finetune: i32,
    pub ins: i32,
    pub old_ins: i32,
    pub smp: i32,
    pub mastervol: i32,
    pub delay: i32,
    pub keyoff: i32,
    pub fadeout: i32,
    pub ins_fade: i32,
    pub volume: i32,
    pub gvl: i32,
    pub rvv: i32,
    pub rpv: i32,
    pub split: u8,
    pub pair: u8,
    pub v_idx: i32,
    pub p_idx: i32,
    pub f_idx: i32,
    pub key_porta: i32,
    pub vibrato: LfoMem,
    pub tremolo: LfoMem,
    pub panbrello: LfoMem,
    pub arpeggio: Arpeggio,
    pub insvib: InsVib,
    pub offset: Offset,
    pub retrig: Retrig,
    pub tremor: Tremor,
    pub vol: Vol,
    pub fine_vol: UpDown,
    pub gvol: Slide3,
    pub trackvol: Slide3,
    pub freq: Freq,
    pub porta: Porta,
    pub fine_porta: FinePorta,
    pub pan: Pan,
    pub invloop: InvLoop,
    pub tempo_slide: i32,
    pub filter: Filter,
    pub macro_: Macro,
    pub noteslide: NoteSlide,
    pub delayed_event: Event,
    pub delayed_ins: i32,
    pub key_memory: i32,
    pub info_pitchbend: i32,
    pub info_position: i32,
    pub info_finalvol: i32,
    pub info_finalpan: i32,
}

impl ChannelData {
    fn test(&self, f: i32) -> bool {
        self.flags & f != 0
    }
    fn set(&mut self, f: i32) {
        self.flags |= f;
    }
    fn reset(&mut self, f: i32) {
        self.flags &= !f;
    }
    fn test_per(&self, f: i32) -> bool {
        self.per_flags & f != 0
    }
    #[allow(dead_code)]
    fn set_per(&mut self, f: i32) {
        self.per_flags |= f;
    }
    fn reset_per(&mut self, f: i32) {
        self.per_flags &= !f;
    }
    pub fn test_note(&self, f: i32) -> bool {
        self.note_flags & f != 0
    }
    pub fn set_note(&mut self, f: i32) {
        self.note_flags |= f;
    }
    pub fn reset_note(&mut self, f: i32) {
        self.note_flags &= !f;
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct PatternLoop {
    pub start: i32,
    pub count: i32,
}

/// `struct flow_control`.
#[derive(Clone, Default, Debug)]
pub struct FlowControl {
    pub pbreak: i32,
    pub jump: i32,
    pub delay: i32,
    pub jumpline: i32,
    pub loop_dest: i32,
    pub loop_param: i32,
    pub loop_start: i32,
    pub loop_count: i32,
    pub loop_active_num: i32,
    pub loop_: Vec<PatternLoop>,
    pub num_rows: i32,
    pub end_point: i32,
    pub rowdelay: i32,
    pub rowdelay_set: i32,
    pub force_reposition: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct VirtChannel {
    pub count: i32,
    pub map: i32,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct ScanData {
    pub time: f64,
    pub row: i32,
    pub ord: i32,
    pub num: i32,
}

#[derive(Clone, Default, Debug)]
pub struct VirtControl {
    pub num_tracks: i32,
    pub virt_channels: i32,
    pub virt_used: i32,
    pub maxvoc: i32,
    pub virt_channel: Vec<VirtChannel>,
    pub voice_array: Vec<MixerVoice>,
}

/// `struct player_data`.
#[derive(Clone, Debug)]
pub struct PlayerData {
    pub ord: i32,
    pub pos: i32,
    pub row: i32,
    pub frame: i32,
    pub speed: i32,
    pub bpm: i32,
    pub mode: i32,
    pub player_flags: i32,
    pub flags: i32,
    pub time_factor_relative: f64,
    pub scan_time_factor: f64,
    pub current_time: f64,
    pub loop_count: i32,
    pub sequence: i32,
    pub bad_sequence: i32,
    pub sequence_control: [u8; XMP_MAX_MOD_LENGTH],
    pub smix_vol: i32,
    pub master_vol: i32,
    pub gvol: i32,
    pub flow: FlowControl,
    pub scan: Vec<ScanData>,
    pub xc_data: Vec<ChannelData>,
    pub channel_vol: [i32; XMP_MAX_CHANNELS],
    pub channel_mute: [bool; XMP_MAX_CHANNELS],
    pub virt: VirtControl,
    pub inject_event: [Event; XMP_MAX_CHANNELS],
    pub st26_speed: i32,
    pub filter: i32,
}

impl Default for PlayerData {
    fn default() -> Self {
        PlayerData {
            ord: 0,
            pos: 0,
            row: 0,
            frame: 0,
            speed: 0,
            bpm: 0,
            mode: 0,
            player_flags: 0,
            flags: 0,
            time_factor_relative: 1.0,
            scan_time_factor: DEFAULT_TIME_FACTOR,
            current_time: 0.0,
            loop_count: 0,
            sequence: 0,
            bad_sequence: 0,
            sequence_control: [NO_SEQUENCE; XMP_MAX_MOD_LENGTH],
            smix_vol: 100,
            master_vol: 100,
            gvol: 0,
            flow: FlowControl::default(),
            scan: vec![ScanData::default()],
            xc_data: Vec::new(),
            channel_vol: [100; XMP_MAX_CHANNELS],
            channel_mute: [false; XMP_MAX_CHANNELS],
            virt: VirtControl::default(),
            inject_event: [Event::default(); XMP_MAX_CHANNELS],
            st26_speed: 0,
            filter: 0,
        }
    }
}

/// libxmp's `struct context_data`: a loaded module, its player and the mixer.
pub struct Context {
    pub m: ModuleData,
    pub p: PlayerData,
    pub s: MixerData,
    pub rng: u32,
    pub playing: bool,
}

/// `pow`, kept a call to the C library's `pow` (and not folded into `exp2` by
/// the compiler) as libxmp's is.
#[inline]
pub(crate) fn pow2(x: f64) -> f64 {
    std::hint::black_box(2.0f64).powf(x)
}

fn libxmp_round(val: f64) -> f64 {
    if val >= 0.0 { (val + 0.5).floor() } else { (val - 0.5).ceil() }
}

/// `libxmp_note_to_period_mix`: the software mixer's period of note `n` bent by
/// `b` (1/12800 semitone units).
pub fn note_to_period_mix(n: i32, b: i32) -> f64 {
    let d = n as f64 + b as f64 / 12800.0;
    PERIOD_BASE / pow2(d / 12.0)
}

/// `libxmp_c2spd_to_note`.
pub fn c2spd_to_note(c2spd: i32) -> (i32, i32) {
    if c2spd <= 0 {
        return (0, 0);
    }
    let c = (1536.0 * (c2spd as f64 / 8363.0).ln() / std::f64::consts::LN_2) as i32;
    (c / 128, c % 128)
}

/// `libxmp_period_to_note` (MOD loaders).
pub fn period_to_note(p: i32) -> i32 {
    if p <= 0 {
        return 0;
    }
    libxmp_round(12.0 * (PERIOD_BASE / p as f64).ln() / std::f64::consts::LN_2) as i32 + 1
}

/// `libxmp_gus_frequency_steps`: a Gravis Ultrasound frequency offset in Hz for
/// a number of steps. The card's rate is 44100 up to 14 active channels and
/// round(14 * 44100 / active_channels) for 15 to 32.
pub fn gus_frequency_steps(num_steps: i32, num_channels_active: i32) -> f64 {
    const GUS_RATES: [f64; 19] = [
        44100.0, 41160.0, 38587.5, 36317.65, 34300.0, 32494.74, 30870.0, 29400.0, 28063.64, 26843.48, 25725.0, 24696.0, 23746.15,
        22866.67, 22050.0, 21289.66, 20580.0, 19916.13, 19294.75,
    ];
    let n = num_channels_active.clamp(14, 32);
    (num_steps as f64 * GUS_RATES[(n - 14) as usize]) / 1024.0
}

/// `libxmp_note_to_period`.
pub fn note_to_period(m: &ModuleData, n: i32, f: i32, adj: f64) -> f64 {
    let d = n as f64 + f as f64 / 128.0;
    let mut per = match m.period_type {
        PeriodType::Linear => (240.0 - d) * 16.0,
        PeriodType::Cspd => 8363.0 * pow2(n as f64 / 12.0) / 32.0 + f as f64,
        _ => PERIOD_BASE / pow2(d / 12.0),
    };
    if adj > 0.1 {
        per *= adj;
    }
    per
}

/// `libxmp_period_to_bend`.
pub fn period_to_bend(m: &ModuleData, p: f64, n: i32, adj: f64) -> i32 {
    if n == 0 || p < 0.1 {
        return 0;
    }
    match m.period_type {
        PeriodType::Linear => (100.0 * (8.0 * (((240 - n) * 16) as f64 - p))) as i32,
        PeriodType::Cspd => {
            let d = note_to_period(m, n, 0, adj);
            libxmp_round(100.0 * (1536.0 / std::f64::consts::LN_2) * (p / d).ln()) as i32
        }
        _ => {
            let d = note_to_period(m, n, 0, adj);
            libxmp_round(100.0 * (1536.0 / std::f64::consts::LN_2) * (d / p).ln()) as i32
        }
    }
}

// Envelopes

fn check_envelope_end(env: &Envelope, x: i32) -> bool {
    if env.flg & XMP_ENVELOPE_ON == 0 || env.npt <= 0 {
        return false;
    }
    let idx = ((env.npt - 1) * 2) as usize;
    if (x >= env.data[idx] as i32 || idx == 0) && env.flg & XMP_ENVELOPE_LOOP == 0 {
        return true;
    }
    false
}

fn get_envelope(env: &Envelope, x: i32, def: i32) -> i32 {
    let data = &env.data;
    if x < 0 || env.flg & XMP_ENVELOPE_ON == 0 || env.npt <= 0 {
        return def;
    }
    let mut idx = ((env.npt - 1) * 2) as usize;
    let mut x1 = data[idx] as i32;
    if x >= x1 || idx == 0 {
        return data[idx + 1] as i32;
    }
    loop {
        idx -= 2;
        x1 = data[idx] as i32;
        if !(idx > 0 && x1 > x) {
            break;
        }
    }
    let y1 = data[idx + 1] as i32;
    let x2 = data[idx + 2] as i32;
    let y2 = data[idx + 3] as i32;
    if x < x1 || x2 < x1 {
        return y1;
    }
    if x2 == x1 { y2 } else { ((y2 - y1) * (x - x1) / (x2 - x1)) + y1 }
}

fn update_envelope_generic(env: &Envelope, mut x: i32, mut release: bool) -> i32 {
    let data = &env.data;
    let has_loop = env.flg & XMP_ENVELOPE_LOOP != 0;
    let mut has_sus = env.flg & XMP_ENVELOPE_SUS != 0;
    let lps = (env.lps << 1) as usize;
    let lpe = (env.lpe << 1) as usize;
    let sus = (env.sus << 1) as usize;
    if has_loop && has_sus && sus == lpe && !release {
        has_sus = false;
    }
    if has_loop && x > data[lpe] as i32 + 1 {
        release = true;
    } else if has_sus && x > data[sus] as i32 + 1 {
        release = true;
    }
    if has_sus && !release && x >= data[sus] as i32 {
        x = data[sus] as i32;
    }
    if has_loop && x >= data[lpe] as i32 && !(release && has_sus && sus == lpe) {
        x = data[lps] as i32;
    }
    x
}

fn update_envelope_xm(env: &Envelope, mut x: i32, mut release: bool) -> i32 {
    let data = &env.data;
    let has_loop = env.flg & XMP_ENVELOPE_LOOP != 0;
    let has_sus = env.flg & XMP_ENVELOPE_SUS != 0;
    let lps = (env.lps << 1) as usize;
    let lpe = (env.lpe << 1) as usize;
    let sus = (env.sus << 1) as usize;
    if has_sus && x > data[sus] as i32 + 1 {
        release = true;
    }
    if has_sus && !release && x >= data[sus] as i32 {
        x = data[sus] as i32;
    }
    if has_loop && x == data[lpe] as i32 && !(release && has_sus && sus == lpe) {
        x = data[lps] as i32;
    }
    x
}

fn update_envelope_it(env: &Envelope, mut x: i32, release: bool, key_off: bool) -> i32 {
    let data = &env.data;
    let has_loop = env.flg & XMP_ENVELOPE_LOOP != 0;
    let has_sus = env.flg & XMP_ENVELOPE_SUS != 0;
    let lps = (env.lps << 1) as usize;
    let lpe = (env.lpe << 1) as usize;
    let sus = (env.sus << 1) as usize;
    let sue = (env.sue << 1) as usize;
    if has_sus && key_off && x == data[sue] as i32 + 1 {
        x = data[sus] as i32;
    } else if has_sus && !release {
        if x == data[sue] as i32 + 1 {
            x = data[sus] as i32;
        }
    } else if has_loop && x > data[lpe] as i32 {
        x = data[lps] as i32;
    }
    x
}

fn check_envelope_fade(env: &Envelope, x: i32) -> i32 {
    if env.flg & XMP_ENVELOPE_ON == 0 {
        return 0;
    }
    let idx = ((env.npt - 1).max(0) * 2) as usize;
    if x > env.data[idx] as i32 {
        if env.data[idx + 1] == 0 {
            return -1;
        } else {
            return 1;
        }
    }
    0
}

impl Context {
    pub fn new(m: ModuleData) -> Context {
        Context { m, p: PlayerData::default(), s: MixerData::default(), rng: 0, playing: false }
    }

    fn has_quirk(&self, q: i32) -> bool {
        self.m.quirk & q != 0
    }

    fn has_flow_mode(&self, f: i32) -> bool {
        self.m.flow_mode & f != 0
    }

    pub(crate) fn is_valid_instrument(&self, x: i32) -> bool {
        (x as u32 as usize) < self.m.module.ins && self.m.module.xxi[x as usize].nsm > 0
    }

    fn is_valid_sample(&self, x: i32) -> bool {
        (x as u32 as usize) < self.m.module.smp && self.m.module.xxs[x as usize].data.is_some()
    }

    pub(crate) fn is_valid_note(x: i32) -> bool {
        (x as u32 as usize) < XMP_MAX_KEYS
    }

    /// `libxmp_get_random`.
    pub fn get_random(&mut self, range: u32) -> u32 {
        let mut state = self.rng;
        if state == 0 {
            state = 1;
        }
        state ^= state << 13;
        state ^= state >> 17;
        state <<= 5;
        self.rng = state;
        ((range as u64 * state as u64) >> 32) as u32
    }

    // lfo.c

    fn get_lfo_mod(&mut self, lfo: &Lfo) -> i32 {
        if lfo.rate == 0 {
            return 0;
        }
        let val = match lfo.type_ {
            0 => SINE_WAVE[lfo.phase as usize & 63],
            1 => 255 - (lfo.phase << 3),
            2 => {
                if lfo.phase < WAVEFORM_SIZE / 2 {
                    255
                } else {
                    -255
                }
            }
            3 => self.get_random(512) as i32 - 256,
            669 => lfo.phase & 1,
            _ => return 0,
        };
        val.wrapping_mul(lfo.depth)
    }

    fn get_lfo_st3(&mut self, lfo: &Lfo) -> i32 {
        if lfo.rate == 0 {
            return 0;
        }
        if lfo.type_ == 2 {
            let val = if lfo.phase < WAVEFORM_SIZE / 2 { 255 } else { 0 };
            return val * lfo.depth;
        }
        self.get_lfo_mod(lfo)
    }

    fn get_lfo_ft2(&mut self, lfo: &Lfo) -> i32 {
        if lfo.rate == 0 {
            return 0;
        }
        if lfo.type_ == 1 {
            let phase = (lfo.phase + (WAVEFORM_SIZE >> 1)) % WAVEFORM_SIZE;
            let val = (phase << 3) - 255;
            return val * lfo.depth;
        }
        self.get_lfo_mod(lfo)
    }

    /// `libxmp_lfo_get`.
    pub fn lfo_get(&mut self, lfo: Lfo, is_vibrato: bool) -> i32 {
        match self.m.read_event_type {
            ReadEvent::St3 => self.get_lfo_st3(&lfo),
            ReadEvent::Ft2 => {
                if is_vibrato {
                    self.get_lfo_ft2(&lfo)
                } else {
                    self.get_lfo_mod(&lfo)
                }
            }
            ReadEvent::It => self.get_lfo_st3(&lfo),
            _ => self.get_lfo_mod(&lfo),
        }
    }

    fn update_envelope(&self, env: &Envelope, mut x: i32, release: bool, key_off: bool) -> i32 {
        if x < 0xffff {
            x += 1;
        }
        if x < 0 {
            return -1;
        }
        if env.flg & XMP_ENVELOPE_ON == 0 || env.npt <= 0 {
            return x;
        }
        if self.m.read_event_type == ReadEvent::It {
            update_envelope_it(env, x, release, key_off)
        } else if !self.has_quirk(QUIRK_FT2ENV) {
            update_envelope_generic(env, x, release)
        } else {
            update_envelope_xm(env, x, release)
        }
    }

    fn ft2_arpeggio(&self, chn: usize) -> i32 {
        let xc = &self.p.xc_data[chn];
        if xc.arpeggio.val[1] == 0 && xc.arpeggio.val[2] == 0 {
            return 0;
        }
        if self.p.frame == 0 {
            return 0;
        }
        let i = self.p.speed - (self.p.frame % self.p.speed);
        if i == 16 {
            0
        } else if i > 16 {
            xc.arpeggio.val[2] as i32
        } else {
            xc.arpeggio.val[(i % 3) as usize] as i32
        }
    }

    fn arpeggio(&mut self, chn: usize) -> i32 {
        let arp = if self.has_quirk(QUIRK_FT2BUGS) {
            self.ft2_arpeggio(chn)
        } else {
            let xc = &self.p.xc_data[chn];
            xc.arpeggio.val[(xc.arpeggio.count as usize) & 15] as i32
        };
        let xc = &mut self.p.xc_data[chn];
        xc.arpeggio.count += 1;
        if xc.arpeggio.size != 0 {
            xc.arpeggio.count %= xc.arpeggio.size;
        }
        arp
    }

    fn is_first_frame(&self) -> bool {
        match self.m.read_event_type {
            ReadEvent::It | ReadEvent::St3 => self.p.frame % self.p.speed == 0,
            _ => self.p.frame == 0,
        }
    }

    fn reset_channels(&mut self) {
        let volbase = self.m.volbase;
        let extra = self.m.extra.new_channel_extras();
        for xc in self.p.xc_data.iter_mut() {
            *xc = ChannelData::default();
            xc.ins = -1;
            xc.old_ins = 0;
            xc.key = -1;
            xc.volume = volbase;
            xc.extra = extra;
        }
        let chn = self.m.module.chn;
        for i in 0..self.p.virt.num_tracks as usize {
            let c = self.m.module.xxc[i.min(XMP_MAX_CHANNELS - 1)];
            {
                let xc = &mut self.p.xc_data[i];
                if i >= chn {
                    xc.mastervol = 0x40;
                    xc.pan.val = 0x80;
                } else {
                    xc.mastervol = c.vol;
                    xc.pan.val = c.pan;
                }
                xc.filter.cutoff = 0xff;
            }
            if c.flg & XMP_CHANNEL_SPLIT != 0 {
                let split = (((c.flg & 0x30) >> 4) + 1) as u8;
                self.p.xc_data[i].split = split;
                for j in 0..i {
                    if self.m.module.xxc[j].flg & XMP_CHANNEL_SPLIT != 0 && self.p.xc_data[j].split == split {
                        self.p.xc_data[j].pair = i as u8;
                        self.p.xc_data[i].pair = j as u8;
                    }
                }
            } else {
                self.p.xc_data[i].split = 0;
            }
            if c.flg & XMP_CHANNEL_SURROUND != 0 {
                self.p.xc_data[i].pan.surround = 1;
            }
        }
    }

    fn check_delay(&mut self, e: &Event, chn: usize) -> bool {
        if (e.fxt == FX_SPEED && e.fxp < 0x20) || e.fxt == FX_S3M_SPEED {
            if e.fxp != 0 {
                self.p.speed = e.fxp as i32;
            }
        }
        if (e.f2t == FX_SPEED && e.f2p < 0x20) || e.f2t == FX_S3M_SPEED {
            if e.f2p != 0 {
                self.p.speed = e.f2p as i32;
            }
        }
        let xc = &mut self.p.xc_data[chn];
        let delay = if e.fxt == FX_EXTENDED && msn(e.fxp) == EX_DELAY && lsn(e.fxp) != 0 {
            Some(lsn(e.fxp) as i32 + 1)
        } else if e.f2t == FX_EXTENDED && msn(e.f2p) == EX_DELAY && lsn(e.f2p) != 0 {
            Some(lsn(e.f2p) as i32 + 1)
        } else if e.fxt == FX_MED_RETRIG && msn(e.fxp) != 0 {
            xc.reset(RETRIG);
            Some(msn(e.fxp) as i32 + 1)
        } else if e.f2t == FX_MED_RETRIG && msn(e.f2p) != 0 {
            xc.reset(RETRIG);
            Some(msn(e.fxp) as i32 + 1)
        } else {
            None
        };
        match delay {
            None => false,
            Some(d) => {
                xc.delay = d;
                xc.delayed_event = *e;
                if e.ins != 0 {
                    xc.delayed_ins = e.ins as i32;
                }
                true
            }
        }
    }

    fn read_row(&mut self, pat: usize, row: i32) {
        for chn in 0..self.m.module.chn {
            let track = &self.m.module.xxt[self.m.module.xxp[pat].index[chn]];
            let event = if (row as usize) < track.rows { track.event[row as usize] } else { Event::default() };
            if self.m.read_event_type == ReadEvent::Ft2 {
                let xc = &mut self.p.xc_data[chn];
                xc.keyoff = 0;
                xc.reset(TREMOR);
            }
            if !self.check_delay(&event, chn) {
                let f = &self.p.flow;
                if f.rowdelay_set == 0 || ((f.rowdelay_set & ROWDELAY_FIRST_FRAME) != 0 && f.rowdelay > 0) {
                    self.read_event(&event, chn);
                }
            } else if self.m.read_event_type == ReadEvent::It {
                self.p.xc_data[chn].flags = 0;
            }
        }
    }

    fn get_channel_vol(&self, chn: usize) -> i32 {
        if (chn as i32) < self.p.virt.num_tracks {
            return self.p.channel_vol[chn];
        }
        if chn as i32 >= self.p.virt.virt_channels {
            return 0;
        }
        let root = self.virt_getroot(chn as i32);
        if root < 0 {
            return 0;
        }
        self.p.channel_vol[root as usize]
    }

    fn tremor_ft2(&mut self, chn: usize, mut finalvol: i32) -> i32 {
        let frame = self.p.frame;
        let xc = &mut self.p.xc_data[chn];
        if xc.test(TREMOR) && frame != 0 {
            xc.tremor.count &= !TREMOR_SUPPRESS;
            if xc.tremor.count == 0 {
                xc.tremor.count = xc.tremor.up | TREMOR_ON;
            } else if xc.tremor.count == TREMOR_ON {
                xc.tremor.count = xc.tremor.down;
            } else {
                xc.tremor.count = xc.tremor.count.wrapping_sub(1);
            }
        }
        if xc.tremor.count & (TREMOR_ON | TREMOR_SUPPRESS) == 0 {
            finalvol = 0;
        }
        finalvol
    }

    fn tremor_s3m(&mut self, chn: usize, mut finalvol: i32) -> i32 {
        let xc = &mut self.p.xc_data[chn];
        if xc.test(TREMOR) {
            if xc.tremor.count == 0 {
                xc.tremor.count = xc.tremor.up | TREMOR_ON;
            } else if xc.tremor.count == TREMOR_ON {
                xc.tremor.count = xc.tremor.down;
            }
            xc.tremor.count = xc.tremor.count.wrapping_sub(1);
            if xc.tremor.count & TREMOR_ON == 0 {
                finalvol = 0;
            }
        }
        finalvol
    }

    fn delayed_keyoff(&mut self, chn: usize) {
        let ins = self.p.xc_data[chn].ins;
        match self.m.read_event_type {
            ReadEvent::Ft2 => {
                if self.p.frame >= self.p.speed {
                    return;
                }
                let env_on = self.get_instrument(ins).is_some_and(|i| i.aei.flg & XMP_ENVELOPE_ON != 0);
                let xc = &mut self.p.xc_data[chn];
                if env_on {
                    xc.set_note(NOTE_RELEASE);
                } else {
                    xc.volume = 0;
                }
                xc.set_note(NOTE_FADEOUT);
            }
            _ => self.p.xc_data[chn].set_note(NOTE_RELEASE),
        }
    }

    /// `libxmp_get_instrument` (no sound effects instruments).
    pub fn get_instrument(&self, ins: i32) -> Option<&Instrument> {
        if ins < 0 { None } else { self.m.module.xxi.get(ins as usize).filter(|_| (ins as usize) < self.m.module.ins) }
    }

    fn process_volume(&mut self, chn: usize, act: i32) {
        let ins = self.p.xc_data[chn].ins;
        let Some(instrument) = self.get_instrument(ins) else { return };
        let aei = instrument.aei;
        let ins_vol = instrument.vol;
        let mut fade = false;

        if self.has_quirk(QUIRK_KEYOFF) {
            let xc = &self.p.xc_data[chn];
            if (xc.test_note(NOTE_ENV_RELEASE) || act == VIRT_ACTION_OFF)
                && (aei.flg & XMP_ENVELOPE_ON == 0 || aei.flg & XMP_ENVELOPE_LOOP != 0)
            {
                fade = true;
            }
        } else if self.m.read_event_type != ReadEvent::Ft2 {
            let xc = &mut self.p.xc_data[chn];
            if aei.flg & XMP_ENVELOPE_ON == 0 && xc.test_note(NOTE_ENV_RELEASE) {
                xc.fadeout = 0;
            }
            if xc.test_note(NOTE_ENV_RELEASE) || act == VIRT_ACTION_OFF {
                fade = true;
            }
        }

        if !self.p.xc_data[chn].test_per(VENV_PAUSE) {
            let xc = &self.p.xc_data[chn];
            let release = xc.test_note(NOTE_ENV_RELEASE) || act == VIRT_ACTION_OFF;
            let v = self.update_envelope(&aei, xc.v_idx, release, xc.test(KEY_OFF));
            self.p.xc_data[chn].v_idx = v;
        }
        let v_idx = self.p.xc_data[chn].v_idx;
        let vol_envelope = get_envelope(&aei, v_idx, 64) as u16 as i32;
        if check_envelope_end(&aei, v_idx) {
            let xc = &mut self.p.xc_data[chn];
            if vol_envelope == 0 {
                xc.set_note(NOTE_END);
            }
            xc.set_note(NOTE_ENV_END);
        }
        match check_envelope_fade(&aei, v_idx) {
            -1 => self.p.xc_data[chn].set_note(NOTE_END),
            0 => {}
            _ => {
                if self.has_quirk(QUIRK_ENVFADE) {
                    self.p.xc_data[chn].set_note(NOTE_FADEOUT);
                }
            }
        }
        {
            let xc = &mut self.p.xc_data[chn];
            if xc.test_note(NOTE_FADEOUT) || act == VIRT_ACTION_FADE {
                fade = true;
            }
            if fade {
                if xc.fadeout > xc.ins_fade {
                    xc.fadeout -= xc.ins_fade;
                } else {
                    xc.fadeout = 0;
                    xc.set_note(NOTE_END);
                }
            }
        }
        if self.p.xc_data[chn].test_note(NOTE_END) && chn as i32 >= self.p.virt.num_tracks {
            self.virt_resetchannel(chn as i32);
            return;
        }

        let mut finalvol = self.extras_get_volume(chn);
        if self.m.read_event_type == ReadEvent::It {
            let xc = &self.p.xc_data[chn];
            finalvol = xc.volume * (100 - xc.rvv) / 100;
        }
        if self.p.xc_data[chn].test(TREMOLO) {
            let first = self.is_first_frame();
            if !first || !self.has_quirk(QUIRK_PROTRACK) {
                let lfo = self.p.xc_data[chn].tremolo.lfo;
                finalvol += self.lfo_get(lfo, false) / (1 << 6);
            }
            if !first || self.has_quirk(QUIRK_VIBALL) {
                self.p.xc_data[chn].tremolo.lfo.update();
            }
        }
        finalvol = finalvol.clamp(0, self.m.volbase);
        let xc = &self.p.xc_data[chn];
        finalvol = finalvol.wrapping_mul(xc.fadeout) >> 6;
        let a = vol_envelope.wrapping_mul(self.p.gvol).wrapping_mul(xc.mastervol) / self.m.gvolbase;
        let b = finalvol.wrapping_mul(0x40) / self.m.volbase;
        finalvol = ((a.wrapping_mul(b)) as u32 >> 18) as i32;
        finalvol = finalvol * self.get_channel_vol(chn) / 100;
        if let Some(t) = self.m.vol_table {
            finalvol = if self.m.volbase == 0xff {
                t[(finalvol >> 2) as usize] << 2
            } else {
                t[(finalvol >> 4) as usize] << 4
            };
        }
        if self.has_quirk(QUIRK_INSVOL) {
            finalvol = (finalvol * ins_vol * self.p.xc_data[chn].gvl) >> 12;
        }
        finalvol = if self.m.read_event_type == ReadEvent::Ft2 {
            self.tremor_ft2(chn, finalvol)
        } else {
            self.tremor_s3m(chn, finalvol)
        };
        self.p.xc_data[chn].macro_.finalvol = finalvol;
        if chn < self.m.module.chn {
            finalvol = finalvol * self.p.master_vol / 100;
        } else {
            finalvol = finalvol * self.p.smix_vol / 100;
        }
        let xc = &mut self.p.xc_data[chn];
        xc.info_finalvol = if xc.test_note(NOTE_SAMPLE_END) { 0 } else { finalvol };
        let (split, pair) = (xc.split, xc.pair);
        self.virt_setvol(chn as i32, finalvol);
        if split != 0 {
            self.virt_setvol(pair as i32, finalvol);
        }
    }

    fn process_frequency(&mut self, chn: usize, act: i32) {
        let ins = self.p.xc_data[chn].ins;
        let Some(instrument) = self.get_instrument(ins) else { return };
        let fei = instrument.fei;

        if !self.p.xc_data[chn].test_per(FENV_PAUSE) {
            let xc = &self.p.xc_data[chn];
            let release = xc.test_note(NOTE_ENV_RELEASE) || act == VIRT_ACTION_OFF;
            let f = self.update_envelope(&fei, xc.f_idx, release, xc.test(KEY_OFF));
            self.p.xc_data[chn].f_idx = f;
        }
        let frq_envelope = get_envelope(&fei, self.p.xc_data[chn].f_idx, 0);

        if self.p.xc_data[chn].test(NOTE_SLIDE) {
            let m = &self.m;
            let xc = &mut self.p.xc_data[chn];
            if xc.noteslide.count == 0 {
                xc.note += xc.noteslide.slide;
                xc.period = note_to_period(m, xc.note, xc.finetune, xc.per_adj);
                xc.noteslide.count = xc.noteslide.speed;
            }
            xc.noteslide.count -= 1;
            let note = xc.note;
            self.virt_setnote(chn as i32, note);
        }

        // instrument vibrato
        let insvib = self.p.xc_data[chn].insvib;
        let mut vibrato = 1.0 * self.lfo_get(insvib.lfo, true) as f64 / (4096 * (1 + insvib.sweep)) as f64;
        {
            let xc = &mut self.p.xc_data[chn];
            xc.insvib.lfo.update();
            if xc.insvib.sweep > 1 {
                xc.insvib.sweep -= 2;
            } else {
                xc.insvib.sweep = 0;
            }
        }

        if self.p.xc_data[chn].test(VIBRATO) || self.p.xc_data[chn].test_per(VIBRATO) {
            let first = self.is_first_frame();
            if !first || !self.has_quirk(QUIRK_PROTRACK) {
                let shift = if self.has_quirk(QUIRK_VIBHALF) { 10 } else { 9 };
                let lfo = self.p.xc_data[chn].vibrato.lfo;
                let vib = self.lfo_get(lfo, true) / (1 << shift);
                if self.has_quirk(QUIRK_VIBINV) {
                    vibrato -= vib as f64;
                } else {
                    vibrato += vib as f64;
                }
            }
            if !first || self.has_quirk(QUIRK_VIBALL) {
                self.p.xc_data[chn].vibrato.lfo.update();
            }
        }

        let mut period = self.p.xc_data[chn].period;
        period += self.extras_get_period(chn) as f64;
        if self.has_quirk(QUIRK_ST3BUGS) && period < 0.25 {
            self.virt_resetchannel(chn as i32);
        }
        if period < 0.1 {
            period = 0.1;
        }

        let mut arp = self.arpeggio(chn);

        let (note, per_adj, finetune) = {
            let xc = &self.p.xc_data[chn];
            (xc.note, xc.per_adj, xc.finetune)
        };
        let mut linear_bend = period_to_bend(&self.m, period + vibrato, note, per_adj);

        {
            let xc = &self.p.xc_data[chn];
            if xc.test_note(NOTE_GLISSANDO) && xc.test(TONEPORTA) {
                if linear_bend > 0 {
                    linear_bend = (linear_bend + 6400) / 12800 * 12800;
                } else if linear_bend < 0 {
                    linear_bend = (linear_bend - 6400) / 12800 * 12800;
                }
            }
        }

        if self.has_quirk(QUIRK_FT2BUGS) && arp != 0 {
            linear_bend = linear_bend / 12800 * 12800 + finetune * 100;
            if note + arp > 107 && self.p.speed - (self.p.frame % self.p.speed) > 0 {
                arp = 108 - note;
            }
        }

        if self.p.xc_data[chn].f_idx >= 0 && fei.flg & XMP_ENVELOPE_FLT == 0 {
            linear_bend += frq_envelope * 128;
        }

        if arp != 0 {
            linear_bend += (100 << 7) * arp;
            if self.has_quirk(QUIRK_PROTRACK) {
                if note + arp > MAX_NOTE_MOD + 1 {
                    linear_bend -= 12800 * (3 * 12);
                } else if note + arp > MAX_NOTE_MOD {
                    self.virt_setvol(chn as i32, 0);
                }
            }
        }

        linear_bend += self.extras_get_linear_bend(chn);

        let mut final_period = note_to_period_mix(note, linear_bend);
        if self.has_quirk(QUIRK_ST3BUGS) && final_period < 16.239270 {
            final_period = 16.239270;
        }
        self.virt_setperiod(chn as i32, final_period);
        self.p.xc_data[chn].info_pitchbend = linear_bend >> 7;

        // Process filter
        if !self.has_quirk(QUIRK_FILTER) {
            return;
        }
        let cutoff;
        {
            let xc = &mut self.p.xc_data[chn];
            if xc.f_idx >= 0 && fei.flg & XMP_ENVELOPE_FLT != 0 {
                if frq_envelope < 0xfe {
                    xc.filter.envelope = frq_envelope;
                }
                cutoff = ((xc.filter.cutoff * xc.filter.envelope) >> 8).min(0xff);
            } else {
                cutoff = xc.filter.cutoff.min(0xff);
            }
        }
        let resonance = self.p.xc_data[chn].filter.resonance;
        // IT: cutoff 127 + resonance 0 turns off the filter, but this is only
        // applied when playing a new note without toneporta. All other
        // combinations take effect immediately. See OpenMPT filter-reset.it,
        // filter-reset-carry.it
        if cutoff < 0xfe || resonance > 0 || self.p.xc_data[chn].filter.can_disable != 0 {
            let (a0, b0, b1) = filter_setup(self.s.freq, cutoff, resonance);
            self.virt_seteffect(chn as i32, DSP_EFFECT_FILTER_A0, a0);
            self.virt_seteffect(chn as i32, DSP_EFFECT_FILTER_B0, b0);
            self.virt_seteffect(chn as i32, DSP_EFFECT_FILTER_B1, b1);
            self.virt_seteffect(chn as i32, DSP_EFFECT_RESONANCE, resonance);
            self.virt_seteffect(chn as i32, DSP_EFFECT_CUTOFF, cutoff);
            self.p.xc_data[chn].filter.can_disable = 0;
        }
    }

    fn process_pan(&mut self, chn: usize, act: i32) {
        let ins = self.p.xc_data[chn].ins;
        let Some(instrument) = self.get_instrument(ins) else { return };
        let pei = instrument.pei;
        if !self.p.xc_data[chn].test_per(PENV_PAUSE) {
            let xc = &self.p.xc_data[chn];
            let release = xc.test_note(NOTE_ENV_RELEASE) || act == VIRT_ACTION_OFF;
            let pi = self.update_envelope(&pei, xc.p_idx, release, xc.test(KEY_OFF));
            self.p.xc_data[chn].p_idx = pi;
        }
        let pan_envelope = get_envelope(&pei, self.p.xc_data[chn].p_idx, 32);
        let mut panbrello = 0;
        if self.p.xc_data[chn].test(PANBRELLO) {
            let lfo = self.p.xc_data[chn].panbrello.lfo;
            panbrello = self.lfo_get(lfo, false) / 512;
            if self.is_first_frame() {
                self.p.xc_data[chn].panbrello.lfo.update();
            }
        }
        let is_it = self.m.read_event_type == ReadEvent::It;
        let mix = self.s.mix;
        let xc = &mut self.p.xc_data[chn];
        xc.macro_.notepan = xc.pan.val + panbrello + 0x80;
        let channel_pan = xc.pan.val;
        let mut finalpan = channel_pan + panbrello + (pan_envelope - 32) * (128 - (xc.pan.val - 128).abs()) / 32;
        if is_it {
            finalpan += xc.rpv * 4;
        }
        finalpan = finalpan.clamp(0, 255);
        if xc.pan.surround != 0 {
            finalpan = 0;
        } else {
            finalpan = (finalpan - 0x80) * mix / 100;
        }
        xc.info_finalpan = finalpan + 0x80;
        let surround = xc.pan.surround != 0;
        self.virt_setpan(chn as i32, if surround { PAN_SURROUND } else { finalpan });
    }

    fn update_volume(&mut self, chn: usize) {
        let vsall = self.has_quirk(QUIRK_VSALL);
        let (frame, speed) = (self.p.frame, self.p.speed);
        let rowdelay_set = self.p.flow.rowdelay_set;
        let volbase = self.m.volbase;
        let gvolbase = self.m.gvolbase;
        let xc = &mut self.p.xc_data[chn];
        if frame % speed != 0 || vsall {
            if xc.test(GVOL_SLIDE) {
                self.p.gvol += xc.gvol.slide;
            }
            if xc.test(VOL_SLIDE) || xc.test_per(VOL_SLIDE) {
                xc.volume += xc.vol.slide;
            }
            if xc.test_per(VOL_SLIDE) {
                if xc.vol.slide > 0 {
                    let target = (xc.vol.target - 1).max(volbase);
                    if xc.volume > target {
                        xc.volume = target;
                        xc.reset_per(VOL_SLIDE);
                    }
                }
                if xc.vol.slide < 0 {
                    let target = if xc.vol.target > 0 { 0.min(xc.vol.target - 1) } else { 0 };
                    if xc.volume < target {
                        xc.volume = target;
                        xc.reset_per(VOL_SLIDE);
                    }
                }
            }
            if xc.test(VOL_SLIDE_2) {
                xc.volume += xc.vol.slide2;
            }
            if xc.test(TRK_VSLIDE) {
                xc.mastervol += xc.trackvol.slide;
            }
        }
        if frame % speed == 0 {
            if xc.test(FINE_VOLS) {
                xc.volume += xc.vol.fslide;
            }
            if xc.test(FINE_VOLS_2) && (rowdelay_set == 0 || rowdelay_set & ROWDELAY_FIRST_FRAME != 0) {
                xc.volume += xc.vol.fslide2;
            }
            if xc.test(TRK_FVSLIDE) {
                xc.mastervol += xc.trackvol.fslide;
            }
            if xc.test(GVOL_SLIDE) {
                self.p.gvol += xc.gvol.fslide;
            }
        }
        xc.volume = xc.volume.clamp(0, volbase);
        self.p.gvol = self.p.gvol.clamp(0, gvolbase);
        xc.mastervol = xc.mastervol.clamp(0, volbase);
        if xc.split != 0 {
            let (pair, v) = (xc.pair as usize, xc.volume);
            self.p.xc_data[pair].volume = v;
        }
    }

    fn update_frequency(&mut self, chn: usize) {
        let first = self.is_first_frame();
        let protrack = self.has_quirk(QUIRK_PROTRACK);
        if !first || self.has_quirk(QUIRK_PBALL) {
            let xc = &mut self.p.xc_data[chn];
            if xc.test(PITCHBEND) || xc.test_per(PITCHBEND) {
                xc.period += xc.freq.slide as f64;
                if protrack {
                    xc.porta.target = xc.period;
                }
            }
            if (xc.test(TONEPORTA) || xc.test_per(TONEPORTA)) && xc.porta.target > 0.0 {
                let mut end = false;
                if xc.porta.dir > 0 {
                    xc.period += xc.porta.slide as f64;
                    if xc.period >= xc.porta.target {
                        end = true;
                    }
                } else {
                    xc.period -= xc.porta.slide as f64;
                    if xc.period <= xc.porta.target {
                        end = true;
                    }
                }
                if end {
                    xc.period = xc.porta.target;
                    xc.porta.dir = 0;
                    xc.reset(TONEPORTA);
                    xc.reset_per(TONEPORTA);
                    if protrack {
                        xc.porta.target = -1.0;
                    }
                }
            }
        }
        if first {
            let m = &self.m;
            let xc = &mut self.p.xc_data[chn];
            if xc.test(FINE_BEND) {
                xc.period += xc.freq.fslide;
            }
            if xc.test(FINE_NSLIDE) {
                xc.note += xc.noteslide.fslide;
                xc.period = note_to_period(m, xc.note, xc.finetune, xc.per_adj);
            }
        }
        match self.m.period_type {
            PeriodType::Linear => {
                let xc = &mut self.p.xc_data[chn];
                xc.period = xc.period.clamp(MIN_PERIOD_L, MAX_PERIOD_L);
            }
            PeriodType::ModRng => {
                let fine = self.p.xc_data[chn].finetune;
                let min_period = note_to_period(&self.m, MAX_NOTE_MOD, fine, 0.0);
                let max_period = note_to_period(&self.m, MIN_NOTE_MOD, fine, 0.0);
                let xc = &mut self.p.xc_data[chn];
                if xc.period < min_period {
                    xc.period = min_period;
                } else if xc.period > max_period {
                    xc.period = max_period;
                }
            }
            _ => {}
        }
        if self.p.xc_data[chn].period < 0.25 {
            self.virt_setvol(chn as i32, 0);
        }
    }

    fn update_pan(&mut self, chn: usize) {
        let first = self.is_first_frame();
        let xc = &mut self.p.xc_data[chn];
        if xc.test(PAN_SLIDE) {
            if first {
                xc.pan.val += xc.pan.fslide;
            } else {
                xc.pan.val += xc.pan.slide;
            }
            xc.pan.val = xc.pan.val.clamp(0, 0xff);
        }
    }

    fn update_invloop(&mut self, chn: usize) {
        const INVLOOP_TABLE: [i32; 16] = [0, 5, 6, 7, 8, 10, 11, 13, 16, 19, 22, 26, 32, 43, 64, 128];
        let frame = self.p.frame;
        let smp = self.p.xc_data[chn].smp;
        {
            let xc = &mut self.p.xc_data[chn];
            if frame == 0 && xc.test(NEW_INS) {
                xc.invloop.pos = 0;
            }
            xc.invloop.count += INVLOOP_TABLE[(xc.invloop.speed & 15) as usize];
        }
        let (mut lps, mut lpe, mut len) = (-1, -1, -1);
        let mut flg = 0;
        if smp >= 0 && (smp as usize) < self.m.module.smp {
            let xxs = &self.m.module.xxs[smp as usize];
            len = xxs.len;
            flg = xxs.flg;
            if xxs.flg & XMP_SAMPLE_LOOP != 0 {
                lps = xxs.lps;
                lpe = xxs.lpe;
            } else if xxs.flg & XMP_SAMPLE_SLOOP != 0 {
                lps = self.m.xtra[smp as usize].sus;
                lpe = self.m.xtra[smp as usize].sue;
            }
        }
        let xc = &mut self.p.xc_data[chn];
        if lps >= 0 && lps <= lpe && xc.invloop.count >= 128 {
            xc.invloop.count = 0;
            xc.invloop.pos += 1;
            if xc.invloop.pos > lpe - lps {
                xc.invloop.pos = 0;
            }
            if lps >= len || lpe > len {
                return;
            }
            let at = (lps + xc.invloop.pos) as usize;
            if flg & XMP_SAMPLE_16BIT == 0 {
                if let Some(d) = self.m.module.xxs[smp as usize].data.as_mut() {
                    if let Some(b) = d.get_mut(SAMPLE_GUARD + at) {
                        *b ^= 0xff;
                    }
                }
            }
        }
    }

    fn is_valid_instrument_or_sfx(&self, x: i32) -> bool {
        self.is_valid_instrument(x)
    }

    fn play_channel(&mut self, chn: usize) {
        self.p.xc_data[chn].info_finalvol = 0;

        if !self.is_first_frame() && self.p.xc_data[chn].test(TEMPO_SLIDE) {
            self.p.bpm += self.p.xc_data[chn].tempo_slide;
            self.p.bpm = self.p.bpm.clamp(0x20, 0xff);
        }

        if self.p.xc_data[chn].delay > 0 {
            self.p.xc_data[chn].delay -= 1;
            if self.p.xc_data[chn].delay == 0 {
                let e = self.p.xc_data[chn].delayed_event;
                self.read_event(&e, chn);
            }
        }

        // IT MIDI macros (QUIRK_FILTER) are not ported yet.

        let act = self.virt_cstat(chn as i32);
        if act == VIRT_INVALID {
            self.update_volume(chn);
            return;
        }
        if self.p.frame == 0 && act != VIRT_ACTIVE {
            let ins = self.p.xc_data[chn].ins;
            if !self.is_valid_instrument_or_sfx(ins) || act == VIRT_ACTION_CUT {
                self.virt_resetchannel(chn as i32);
                return;
            }
        }
        if !self.is_valid_instrument_or_sfx(self.p.xc_data[chn].ins) {
            return;
        }

        self.play_extras(chn);

        if self.p.xc_data[chn].test(RETRIG) {
            let s3mrtg = self.has_quirk(QUIRK_S3MRTG);
            let cond = {
                let xc = &mut self.p.xc_data[chn];
                xc.retrig.count -= 1;
                if s3mrtg { xc.retrig.count <= 0 } else { xc.retrig.count == 0 }
            };
            if cond {
                if self.p.xc_data[chn].retrig.type_ < 0x10 {
                    self.virt_voicepos(chn as i32, 0.0);
                } else {
                    self.p.xc_data[chn].set_note(NOTE_END);
                }
                let xc = &mut self.p.xc_data[chn];
                let (s, mm, d) = RVAL[(xc.retrig.type_ as usize).min(16)];
                xc.volume += s;
                xc.volume *= mm;
                xc.volume /= d;
                xc.retrig.count = xc.retrig.val;
                if xc.retrig.limit > 0 {
                    xc.retrig.limit -= 1;
                    if xc.retrig.limit == 0 {
                        xc.reset(RETRIG);
                    }
                }
            }
        }

        if self.p.xc_data[chn].keyoff != 0 {
            self.p.xc_data[chn].keyoff -= 1;
            if self.p.xc_data[chn].keyoff == 0 {
                self.delayed_keyoff(chn);
            }
        }

        let rel = self.p.xc_data[chn].test_note(NOTE_SAMPLE_RELEASE);
        self.virt_release(chn as i32, rel);

        self.update_volume(chn);
        self.update_frequency(chn);
        self.update_pan(chn);

        self.process_volume(chn, act);
        self.process_frequency(chn, act);
        self.process_pan(chn, act);

        if self.has_quirk(QUIRK_PROTRACK | QUIRK_INVLOOP) && (self.p.xc_data[chn].ins as u32 as usize) < self.m.module.ins {
            self.update_invloop(chn);
        }

        let xc = &mut self.p.xc_data[chn];
        if xc.test_note(NOTE_SUSEXIT) {
            xc.set_note(NOTE_ENV_RELEASE);
        }
        self.p.xc_data[chn].info_position = self.virt_getvoicepos(chn as i32) as i32;
    }

    fn inject_event(&mut self) {
        for chn in 0..self.m.module.chn {
            let e = self.p.inject_event[chn];
            if e.flag > 0 {
                self.read_event(&e, chn);
                self.p.inject_event[chn].flag = 0;
            }
        }
    }

    // Sequencing

    fn skip_invalid_orders(&mut self) -> bool {
        let md = &self.m.module;
        if self.p.ord < 0 {
            self.p.ord = 0;
        }
        loop {
            if self.p.ord as usize >= md.len || (self.has_quirk(QUIRK_MARKER) && md.xxo[self.p.ord as usize] == XMP_MARK_END) {
                return false;
            }
            if (md.xxo[self.p.ord as usize] as usize) < md.pat {
                return true;
            }
            self.p.ord += 1;
        }
    }

    fn next_order(&mut self, mut last_ord: i32) -> bool {
        self.p.ord += 1;
        if !self.skip_invalid_orders() {
            let md = &self.m.module;
            let entry = self.m.seq_data[self.p.sequence as usize].entry_point as i32;
            if md.rst > md.len || md.xxo[md.rst] as usize >= md.pat || self.p.ord < entry {
                self.p.ord = entry;
            } else if self.get_sequence(md.rst as i32) == self.p.sequence {
                self.p.ord = md.rst as i32;
            } else {
                self.p.ord = entry;
            }
            if !self.skip_invalid_orders() {
                self.p.bad_sequence = 1;
                return false;
            }
            self.p.gvol = self.m.xxo_info[self.p.ord as usize].gvl;
            last_ord = -1;
        }
        self.p.bad_sequence = 0;
        if last_ord != self.p.ord {
            self.p.current_time = self.m.xxo_info[self.p.ord as usize].time;
        }
        let md = &self.m.module;
        let f = &mut self.p.flow;
        f.num_rows = md.xxp[md.xxo[self.p.ord as usize] as usize].rows as i32;
        if f.jumpline >= f.num_rows {
            f.jumpline = 0;
        }
        self.p.row = f.jumpline;
        f.jumpline = 0;
        self.p.pos = self.p.ord;
        self.p.frame = 0;

        if self.has_flow_mode(FLOW_LOOP_PATTERN_RESET) {
            let f = &mut self.p.flow;
            f.loop_start = -1;
            f.loop_count = 0;
            for l in f.loop_.iter_mut().take(md.chn) {
                *l = PatternLoop::default();
            }
        }
        if self.has_quirk(QUIRK_PERPAT) {
            for xc in self.p.xc_data.iter_mut().take(md.chn) {
                xc.per_flags = 0;
            }
        }
        true
    }

    fn next_row(&mut self) -> bool {
        let last_ord = self.p.ord;
        self.p.frame = 0;
        let f = &mut self.p.flow;
        f.delay = 0;
        f.loop_param = -1;
        if f.pbreak != 0 {
            f.pbreak = 0;
            f.loop_dest = -1;
            if f.jump != -1 {
                self.p.ord = f.jump - 1;
                f.jump = -1;
            }
            return self.next_order(last_ord);
        }
        if f.rowdelay == 0 {
            self.p.row += 1;
            f.rowdelay_set = 0;
        } else {
            f.rowdelay -= 1;
        }
        if f.loop_dest >= 0 {
            self.p.row = f.loop_dest;
            f.loop_dest = -1;
        }
        if self.p.row >= f.num_rows {
            return self.next_order(last_ord);
        }
        true
    }

    /// `libxmp_get_frame_time`.
    pub fn get_frame_time(&self) -> f64 {
        if self.p.bpm == 0 {
            return 0.0;
        }
        self.p.scan_time_factor * self.m.rrate / self.p.bpm as f64
    }

    fn update_from_ord_info(&mut self) {
        let oinfo = self.m.xxo_info[self.p.ord.clamp(0, XMP_MAX_MOD_LENGTH as i32 - 1) as usize];
        if oinfo.speed != 0 {
            self.p.speed = oinfo.speed;
        }
        self.p.bpm = oinfo.bpm;
        self.p.gvol = oinfo.gvl;
        self.p.current_time = oinfo.time;
        self.p.st26_speed = oinfo.st26_speed;
    }

    /// `libxmp_reset_flow`.
    pub fn reset_flow(&mut self) {
        let f = &mut self.p.flow;
        f.jumpline = 0;
        f.jump = -1;
        f.pbreak = 0;
        f.loop_dest = -1;
        f.loop_param = -1;
        f.loop_start = -1;
        f.loop_count = 0;
        f.loop_active_num = 0;
        f.delay = 0;
        f.rowdelay = 0;
        f.rowdelay_set = 0;
        f.force_reposition = 0;
        let chn = self.m.module.chn;
        for l in f.loop_.iter_mut().take(chn) {
            *l = PatternLoop::default();
        }
    }

    /// `xmp_start_player` at `rate` Hz, stereo 16-bit.
    pub fn start_player(&mut self, rate: i32) {
        self.mixer_on(rate);
        let p = &mut self.p;
        p.time_factor_relative = 1.0;
        p.master_vol = 100;
        p.smix_vol = 100;
        p.gvol = self.m.volbase;
        p.pos = 0;
        p.ord = 0;
        p.frame = -1;
        p.row = 0;
        p.current_time = 0.0;
        p.loop_count = 0;
        p.sequence = 0;
        p.bad_sequence = 0;
        for i in 0..XMP_MAX_CHANNELS {
            p.channel_mute[i] = i < self.m.module.chn && self.m.module.xxc[i].flg & XMP_CHANNEL_MUTE != 0;
            p.channel_vol[i] = 100;
        }
        if !self.skip_invalid_orders() {
            self.p.ord = 0;
            self.p.scan[0].ord = 0;
            self.p.row = 0;
            self.p.scan[0].row = 0;
            self.p.bad_sequence = 1;
            self.p.flow.end_point = 0;
            self.p.flow.num_rows = 0;
        } else {
            let md = &self.m.module;
            self.p.flow.num_rows = md.xxp[md.xxo[self.p.ord as usize] as usize].rows as i32;
            self.p.flow.end_point = self.p.scan[0].num;
        }
        self.update_from_ord_info();
        self.virt_on(self.m.module.chn as i32);
        let vc = self.p.virt.virt_channels as usize;
        self.p.flow.loop_ = vec![PatternLoop::default(); vc];
        self.reset_flow();
        self.p.xc_data = vec![ChannelData::default(); vc];
        self.p.loop_count = 0;
        for xc in self.p.xc_data.iter_mut() {
            xc.filter.cutoff = 0xff;
        }
        self.reset_channels();
        self.playing = true;
    }

    fn check_end_of_module(&mut self) {
        let seq = self.p.sequence as usize;
        let sc = self.p.scan[seq];
        if self.p.ord == sc.ord && self.p.row == sc.row {
            let f = &mut self.p.flow;
            if f.end_point == 0 {
                self.p.loop_count += 1;
                f.end_point = sc.num;
            }
            f.end_point -= 1;
        }
    }

    /// `xmp_play_frame`: one tick, mixed into `s.buffer`. False at the end
    /// (`-XMP_END`).
    pub fn play_frame(&mut self) -> bool {
        if !self.playing || self.m.module.len == 0 {
            return false;
        }
        if self.p.ord != self.p.pos || self.p.flow.force_reposition != 0 {
            let start = self.m.seq_data[self.p.sequence as usize].entry_point as i32;
            self.p.flow.force_reposition = 0;
            if self.p.pos == -2 {
                return false;
            }
            if self.p.pos == -1 {
                self.p.pos = start;
            }
            let sc = self.p.scan[self.p.sequence as usize];
            if self.p.pos == start {
                self.p.flow.end_point = sc.num;
            }
            if self.p.pos > sc.ord {
                self.p.flow.end_point = 0;
            }
            self.p.flow.jump = -1;
            self.p.ord = self.p.pos - 1;
            if self.p.ord < start {
                self.p.ord = start - 1;
            }
            if !self.next_order(-1) {
                return false;
            }
            self.update_from_ord_info();
            self.virt_reset();
            self.reset_channels();
        } else {
            if self.p.bad_sequence != 0 {
                return false;
            }
            self.p.frame += 1;
            if self.p.frame >= self.p.speed * (1 + self.p.flow.delay) {
                if self.has_quirk(QUIRK_PROTRACK) && self.p.flow.delay != 0 && self.p.flow.pbreak != 0 {
                    if !self.next_row() {
                        return false;
                    }
                    self.check_end_of_module();
                }
                if !self.next_row() {
                    return false;
                }
            }
        }

        for xc in self.p.xc_data.iter_mut().take(self.m.module.chn) {
            xc.reset(KEY_OFF);
        }

        if self.p.frame == 0 {
            self.check_end_of_module();
            let pat = self.m.module.xxo[self.p.ord as usize] as usize;
            self.read_row(pat, self.p.row);
            let st26 = self.p.st26_speed;
            if st26 != 0 {
                if st26 & 0x10000 != 0 {
                    self.p.speed = (st26 & 0xff00) >> 8;
                } else {
                    self.p.speed = st26 & 0xff;
                }
                self.p.st26_speed ^= 0x10000;
            }
        }

        self.inject_event();

        for i in 0..self.p.virt.virt_channels as usize {
            self.play_channel(i);
        }

        self.p.flow.rowdelay_set &= !ROWDELAY_FIRST_FRAME;
        self.p.current_time += self.get_frame_time();
        self.softmixer();
        true
    }

    /// `set_position` of control.c (what `xmp_set_position` does).
    pub fn set_position(&mut self, mut pos: i32, dir: i32) {
        let seq = if dir == 0 { self.get_sequence(pos) } else { self.p.sequence };
        if seq < 0 || seq as usize >= self.m.num_sequences {
            return;
        }
        let has_marker = self.has_quirk(QUIRK_MARKER);
        self.p.sequence = seq;
        let len = self.m.module.len as i32;
        if pos >= 0 {
            while has_marker && pos > 0 && pos < len - 1 && self.m.module.xxo[pos as usize] == XMP_MARK_SKIP {
                if dir < 0 {
                    pos -= 1;
                } else {
                    pos += 1;
                }
            }
            if pos >= len {
                return;
            }
            let pat = self.m.module.xxo[pos as usize];
            if (pat as usize) < self.m.module.pat {
                if has_marker && pat == XMP_MARK_END {
                    return;
                }
                let sc = self.p.scan[seq as usize];
                if pos > sc.ord {
                    self.p.flow.end_point = 0;
                } else {
                    self.p.flow.num_rows = self.m.module.xxp[pat as usize].rows as i32;
                    self.p.flow.end_point = sc.num;
                    self.p.flow.jumpline = 0;
                }
            }
        }
        if pos < len {
            self.p.pos = if pos == 0 { -1 } else { pos };
            self.reset_flow();
        }
    }

    // flow.c

    /// `libxmp_process_pattern_loop`, on the player's flow control or (`scan`) on
    /// a scan's.
    pub fn process_pattern_loop(&self, f: &mut FlowControl, chn: usize, row: i32, fxp: i32) {
        let mut looped = false;
        if self.has_flow_mode(FLOW_LOOP_FIRST_EFFECT) && f.loop_param >= 0 {
            return;
        }
        f.loop_param = fxp;
        let global_target = self.has_flow_mode(FLOW_LOOP_GLOBAL_TARGET);
        let global_count = self.has_flow_mode(FLOW_LOOP_GLOBAL_COUNT);
        let mut start = if global_target { f.loop_start } else { f.loop_[chn].start };
        let mut count = if global_count { f.loop_count } else { f.loop_[chn].count };

        if fxp == 0 {
            if self.has_flow_mode(FLOW_LOOP_IGNORE_TARGET) && count >= 1 {
                return;
            }
            start = row;
            if self.has_quirk(QUIRK_FT2BUGS) {
                f.jumpline = row;
            }
        } else {
            if start < 0 {
                start = if self.has_flow_mode(FLOW_LOOP_INIT_SAMEROW) { row } else { 0 };
            }
            if count != 0 {
                count -= 1;
                if count != 0 {
                    f.loop_dest = start;
                    looped = true;
                } else {
                    if self.has_flow_mode(FLOW_LOOP_END_ADVANCES) {
                        start = row + 1;
                    }
                    if self.has_flow_mode(FLOW_LOOP_END_CANCELS) {
                        f.loop_dest = -1;
                    }
                    f.loop_active_num -= 1;
                }
            } else {
                if self.has_flow_mode(FLOW_LOOP_ONE_AT_A_TIME) {
                    for i in 0..self.m.module.chn {
                        if i != chn && f.loop_[i].count != 0 {
                            // write back what changed so far (the start, if it was unset)
                            if global_target {
                                f.loop_start = start;
                            } else {
                                f.loop_[chn].start = start;
                            }
                            return;
                        }
                    }
                }
                count = fxp;
                f.loop_dest = start;
                f.loop_active_num += 1;
                looped = true;
            }
        }
        if global_target {
            f.loop_start = start;
        } else {
            f.loop_[chn].start = start;
        }
        if global_count {
            f.loop_count = count;
        } else {
            f.loop_[chn].count = count;
        }

        if looped && f.pbreak != 0 {
            if self.has_flow_mode(FLOW_LOOP_SHARED_BREAK) {
                f.jumpline = f.loop_dest;
            }
            if self.has_flow_mode(FLOW_LOOP_UNSET_BREAK) && f.jump < 0 {
                f.pbreak = 0;
            }
            if self.has_flow_mode(FLOW_LOOP_UNSET_JUMP) && f.jump >= 0 {
                f.pbreak = 0;
                f.jump = -1;
            }
        }
    }

    pub fn process_pattern_jump(&self, f: &mut FlowControl, fxp: i32) {
        if self.has_flow_mode(FLOW_LOOP_DELAY_JUMP) && f.loop_dest >= 0 {
            return;
        }
        f.pbreak = 1;
        f.jump = fxp;
        if !self.has_flow_mode(FLOW_JUMP_NO_ROW_SET) {
            f.jumpline = 0;
        }
    }

    pub fn process_pattern_break(&self, f: &mut FlowControl, fxp: i32) {
        if self.has_flow_mode(FLOW_LOOP_DELAY_BREAK) && f.loop_dest >= 0 {
            return;
        }
        f.pbreak = 1;
        f.jumpline = fxp;
    }

    pub fn process_line_jump(&self, f: &mut FlowControl, ord: i32, fxp: i32) {
        if f.pbreak == 0 {
            f.pbreak = 1;
            f.jump = ord;
        }
        f.jumpline = fxp;
    }

    /// `libxmp_get_sequence`.
    pub fn get_sequence(&self, ord: i32) -> i32 {
        if ord < 0 || ord as usize > self.m.module.len {
            return NO_SEQUENCE as i32;
        }
        self.p.sequence_control[(ord as usize).min(XMP_MAX_MOD_LENGTH - 1)] as i32
    }

    // read_event.c

    fn get_subinstrument(&self, ins: i32, key: i32) -> Option<SubInstrument> {
        if self.is_valid_instrument(ins) {
            let instrument = &self.m.module.xxi[ins as usize];
            if Self::is_valid_note(key) {
                let mapped = instrument.map[key as usize].ins;
                if mapped != 0xff && (mapped as i32) < instrument.nsm {
                    return instrument.sub.get(mapped as usize).copied();
                }
            } else if instrument.nsm > 0 {
                return instrument.sub.first().copied();
            }
        }
        None
    }

    fn reset_envelopes(&mut self, chn: usize) {
        let ins = self.p.xc_data[chn].ins;
        if !self.is_valid_instrument(ins) {
            return;
        }
        let xc = &mut self.p.xc_data[chn];
        xc.reset_note(NOTE_ENV_END);
        xc.v_idx = -1;
        xc.p_idx = -1;
        xc.f_idx = -1;
    }

    fn set_effect_defaults(&mut self, note: i32, sub: Option<SubInstrument>, chn: usize, is_toneporta: bool) {
        let protrack = self.has_quirk(QUIRK_PROTRACK);
        let xc = &mut self.p.xc_data[chn];
        if let Some(sub) = sub {
            if note >= 0 {
                if !protrack {
                    xc.finetune = sub.fin;
                }
                xc.gvl = sub.gvl;
                if sub.ifc & 0x80 != 0 {
                    xc.filter.cutoff = (sub.ifc - 0x80) * 2;
                }
                xc.filter.envelope = 0x100;
                if sub.ifr & 0x80 != 0 {
                    xc.filter.resonance = (sub.ifr - 0x80) * 2;
                }
                xc.filter.can_disable = !is_toneporta as i32;
                xc.insvib.lfo.depth = sub.vde;
                xc.insvib.lfo.rate = (sub.vra + 2) >> 2;
                xc.insvib.lfo.type_ = sub.vwf;
                xc.insvib.sweep = sub.vsw;
                xc.vibrato.lfo.phase = 0;
                xc.tremolo.lfo.phase = 0;
            }
        }
        xc.delay = 0;
        xc.tremor.up = 0;
        xc.tremor.down = 0;
        xc.arpeggio.val[0] = 0;
        xc.arpeggio.count = 0;
        xc.arpeggio.size = 1;
        if is_toneporta {
            xc.porta.slide = 0;
        }
    }

    fn set_channel_volume(xc: &mut ChannelData, vol: i32) {
        if vol >= 0 {
            xc.volume = vol;
            xc.set(NEW_VOL);
        }
    }

    fn set_channel_pan(xc: &mut ChannelData, pan: i32) {
        if pan >= 0 {
            xc.pan.val = pan;
            xc.pan.surround = 0;
        }
    }

    fn set_period(&mut self, note: i32, sub: Option<SubInstrument>, chn: usize, is_toneporta: bool) {
        let protrack = self.has_quirk(QUIRK_PROTRACK);
        if sub.is_none() && !protrack {
            return;
        }
        if note >= 0 {
            let (fin, adj) = (self.p.xc_data[chn].finetune, self.p.xc_data[chn].per_adj);
            let per = note_to_period(&self.m, note, fin, adj);
            let xc = &mut self.p.xc_data[chn];
            if !protrack || (note > 0 && is_toneporta) {
                xc.porta.target = per;
            }
            if xc.period < 1.0 || !is_toneporta {
                xc.period = per;
            }
        }
    }

    fn is_toneporta_fx(x: u8) -> bool {
        x == FX_TONEPORTA || x == FX_TONE_VSLIDE || x == FX_PER_TPORTA || x == FX_ULT_TPORTA || x == FX_FAR_TPORTA
    }

    fn is_mod_retrig(x: u8, p: u8) -> bool {
        x == FX_EXTENDED && msn(p) == EX_RETRIG && lsn(p) != 0
    }

    fn set_patch(&mut self, chn: usize, ins: i32, smp: i32, note: i32) {
        self.virt_setpatch(chn as i32, ins, smp, note, 0, 0, 0, 0);
    }

    fn read_event_mod(&mut self, e: &Event, chn: usize) {
        let protrack = self.has_quirk(QUIRK_PROTRACK);
        let mut note = -1;
        let mut sub: Option<SubInstrument> = None;
        let mut new_invalid_ins = false;
        let mut new_swap_ins = false;
        self.p.xc_data[chn].flags = 0;
        let is_toneporta = Self::is_toneporta_fx(e.fxt) || Self::is_toneporta_fx(e.f2t);
        let is_retrig = Self::is_mod_retrig(e.fxt, e.fxp) || Self::is_mod_retrig(e.f2t, e.f2p);

        if Self::is_valid_note(e.note as i32 - 1) && !is_toneporta {
            self.p.xc_data[chn].key = e.note as i32 - 1;
        }
        if e.ins != 0 {
            let ins = e.ins as i32 - 1;
            {
                let xc = &mut self.p.xc_data[chn];
                xc.set(NEW_INS);
                xc.fadeout = 0x10000;
                xc.per_flags = 0;
                xc.offset.val = 0;
                xc.reset_note(NOTE_RELEASE | NOTE_FADEOUT);
            }
            if self.is_valid_instrument(ins) {
                let key = self.p.xc_data[chn].key;
                sub = self.get_subinstrument(ins, key);
                if let Some(s) = sub {
                    new_swap_ins = true;
                    let xc = &mut self.p.xc_data[chn];
                    if protrack {
                        xc.finetune = s.fin;
                        xc.ins = ins;
                    }
                    if xc.split == 0 && e.note != XMP_KEY_OFF {
                        Self::set_channel_volume(xc, s.vol);
                        Self::set_channel_pan(xc, s.pan);
                    }
                }
                if !is_toneporta {
                    let rls = self.m.module.xxi[ins as usize].rls;
                    let xc = &mut self.p.xc_data[chn];
                    xc.ins = ins;
                    xc.ins_fade = rls;
                }
            } else {
                new_invalid_ins = true;
                if !protrack || is_retrig {
                    self.virt_resetchannel(chn as i32);
                } else {
                    self.virt_queuepatch(chn as i32, -1, -1, 0);
                }
            }
        }

        if e.note != 0 {
            {
                let xc = &mut self.p.xc_data[chn];
                xc.set(NEW_NOTE);
                xc.reset_per(VOL_SLIDE);
            }
            if e.note == XMP_KEY_OFF {
                self.p.xc_data[chn].set_note(NOTE_RELEASE);
            } else if !is_toneporta && Self::is_valid_note(e.note as i32 - 1) {
                self.p.xc_data[chn].reset_note(NOTE_END);
                let (ins, key) = (self.p.xc_data[chn].ins, self.p.xc_data[chn].key);
                sub = self.get_subinstrument(ins, key);
                if let Some(s) = sub {
                    let transp = self.m.module.xxi[ins as usize].map[key as usize].xpo as i32;
                    note = key + s.xpo + transp;
                    let mut smp = s.sid;
                    if new_invalid_ins || !self.is_valid_sample(smp) {
                        smp = -1;
                    }
                    if smp >= 0 && (smp as usize) < self.m.module.smp {
                        self.set_patch(chn, ins, smp, note);
                        new_swap_ins = false;
                        self.p.xc_data[chn].smp = smp;
                    }
                } else {
                    let xc = &mut self.p.xc_data[chn];
                    xc.flags = 0;
                    note = xc.key;
                }
            }
            if note >= 0 {
                let xc = &mut self.p.xc_data[chn];
                xc.note = note;
                xc.set_note(NOTE_SET);
            }
        }

        if new_swap_ins && protrack && self.p.xc_data[chn].test_note(NOTE_SET) {
            if let Some(s) = sub {
                let smp = if self.is_valid_sample(s.sid) { s.sid } else { -1 };
                let n = self.p.xc_data[chn].note;
                self.virt_queuepatch(chn as i32, e.ins as i32 - 1, smp, n);
                self.p.xc_data[chn].smp = s.sid;
            }
        }

        let (ins, key) = (self.p.xc_data[chn].ins, self.p.xc_data[chn].key);
        let sub = self.get_subinstrument(ins, key);

        self.set_effect_defaults(note, sub, chn, is_toneporta);
        if e.ins != 0 && sub.is_some() {
            self.reset_envelopes(chn);
        }

        Self::set_channel_volume(&mut self.p.xc_data[chn], e.vol as i32 - 1);
        if e.vol != 0 {
            self.p.xc_data[chn].reset_per(VOL_SLIDE);
        }

        self.process_fx(chn, e, 1);
        self.process_fx(chn, e, 0);

        if e.fxt == FX_PITCH_ADD || e.fxt == FX_PITCH_SUB {
            let xc = &self.p.xc_data[chn];
            let per = note_to_period(&self.m, note, xc.finetune, xc.per_adj);
            self.p.xc_data[chn].period = per;
        } else {
            self.set_period(note, sub, chn, is_toneporta);
        }

        if sub.is_none() {
            return;
        }

        if note >= 0 && !new_invalid_ins {
            let off = self.p.xc_data[chn].offset.val as f64;
            self.virt_voicepos(chn as i32, off);
        } else if new_swap_ins && is_retrig && protrack {
            self.virt_voicepos(chn as i32, 0.0);
        }

        let fx9bug = self.p.flags & XMP_FLAGS_FX9BUG != 0;
        let xc = &mut self.p.xc_data[chn];
        if xc.test(OFFSET) {
            if protrack || fx9bug {
                xc.offset.val += xc.offset.val2;
            }
            xc.reset(OFFSET);
        }
    }

    fn sustain_check(env: &Envelope, idx: i32) -> bool {
        env.flg & XMP_ENVELOPE_ON != 0
            && env.flg & XMP_ENVELOPE_SUS != 0
            && env.flg & XMP_ENVELOPE_LOOP == 0
            && idx == env.data[(env.sus << 1) as usize] as i32
    }

    /// `set_period_ft2`: an instrument number alone keeps the portamento target
    /// (OpenMPT Porta-Pickup.xm); only a note with toneporta sets it.
    fn set_period_ft2(&mut self, key: i32, note: i32, sub: Option<SubInstrument>, chn: usize, is_toneporta: bool) {
        if Self::is_valid_note(key - 1) && is_toneporta {
            // the target updates even for invalid instruments, transpose +0
            let mut n = key - 1;
            if let Some(s) = sub {
                let ins = self.p.xc_data[chn].ins as usize;
                n += self.m.module.xxi[ins].map[(key - 1) as usize].xpo as i32;
                n += s.xpo;
            }
            let (fin, adj) = (self.p.xc_data[chn].finetune, self.p.xc_data[chn].per_adj);
            self.p.xc_data[chn].porta.target = note_to_period(&self.m, n, fin, adj);
        }
        if sub.is_some() && note >= 0 {
            let xc = &self.p.xc_data[chn];
            if xc.period < 1.0 || !is_toneporta {
                let per = note_to_period(&self.m, note, xc.finetune, xc.per_adj);
                self.p.xc_data[chn].period = per;
            }
        }
    }

    /// `read_event_ft2`: Fasttracker 2's note, instrument and key-off handling,
    /// with its quirks (the OpenMPT and libxmp test cases the C code names).
    fn read_event_ft2(&mut self, e: &Event, chn: usize) {
        // FT2 ignores notes next to an out-of-range note delay, checked against
        // the song speed only (OpenMPT DelayCombination.xm)
        if self.p.frame >= self.p.speed {
            return;
        }
        let ft2bugs = self.has_quirk(QUIRK_FT2BUGS);
        let mut ev = *e;
        self.p.xc_data[chn].flags = 0;
        let mut note = -1;
        let mut key = ev.note as i32;
        let mut is_toneporta = false;
        let mut is_delayed = false;

        if ev.fxt == FX_EXTENDED && msn(ev.fxp) == EX_DELAY && lsn(ev.fxp) != 0 {
            // no note + delay: note memory (ft2_delay_note_memory.xm)
            let xc = &self.p.xc_data[chn];
            if key == 0 {
                key = if xc.key_memory != 0 { xc.key_memory } else { xc.key + 1 };
            }
            // key off + no ins# + delay + volume column pan: ignore the pan
            if ft2bugs && key == XMP_KEY_OFF as i32 && ev.ins == 0 && ev.f2t == FX_SETPAN {
                ev.f2t = 0;
                ev.f2p = 0;
            }
            is_delayed = true;
        }

        if Self::is_toneporta_fx(ev.fxt) || Self::is_toneporta_fx(ev.f2t) {
            is_toneporta = true;
            // Mx + 3xx/5xy: 3xx uses the rate of the volume column
            if ft2bugs && ev.fxt == FX_TONEPORTA && Self::is_toneporta_fx(ev.f2t) {
                ev.fxp = 0;
            }
        }

        // FT2 deletes K00 and, without volume column toneporta, turns the note
        // into a key off (ft2_k00_is_note_off.xm)
        if ev.fxt == FX_KEYOFF && ev.fxp == 0 {
            ev.fxt = 0;
            if !is_toneporta {
                key = XMP_KEY_OFF as i32;
            }
        }

        // instrument: only a new valid note without toneporta/K00 changes it
        if ev.ins != 0 {
            let xc = &mut self.p.xc_data[chn];
            xc.set(NEW_INS);
            xc.per_flags = 0;
        }
        if Self::is_valid_note(key - 1) && !is_toneporta {
            // a note without instrument loads the last referenced one
            let ins = if ev.ins != 0 { ev.ins as i32 } else { self.p.xc_data[chn].old_ins };
            let mut smp = -1;
            let mut n = key - 1;
            self.p.xc_data[chn].key_memory = key;
            let mut rls = None;
            if self.is_valid_instrument(ins - 1) {
                rls = Some(self.m.module.xxi[(ins - 1) as usize].rls);
                if let Some(s) = self.get_subinstrument(ins - 1, key - 1) {
                    n += self.m.module.xxi[(ins - 1) as usize].map[(key - 1) as usize].xpo as i32;
                    n += s.xpo;
                    smp = s.sid;
                }
            }
            // the fade needs an ins# (ft2_instrument_fade_update.xm)
            if ev.ins != 0 {
                self.p.xc_data[chn].ins_fade = rls.unwrap_or(0x80 << 1);
            }
            // B-(-1) updates key/instrument/sample but not the note; notes
            // past A#9 or below B-(-1) act as no key (ft2_note_range.xm)
            if (FT2_NOTE_BN1..=FT2_NOTE_AS9).contains(&n) {
                let vins = self.is_valid_instrument(ins - 1);
                let vsmp = self.is_valid_sample(smp);
                let xc = &mut self.p.xc_data[chn];
                if n >= FT2_NOTE_C0 {
                    xc.note = n;
                }
                xc.key = key - 1;
                xc.ins = if vins { ins - 1 } else { -1 };
                xc.smp = if vsmp { smp } else { -1 };
            } else {
                key = 0;
            }
        }
        // the new instrument, or the one FT2 keeps
        let (ins, k) = (self.p.xc_data[chn].ins, self.p.xc_data[chn].key);
        let sub = self.get_subinstrument(ins, k);

        // note: none for toneporta
        if key != 0 {
            self.p.xc_data[chn].set(NEW_NOTE);
        }
        if Self::is_valid_note(key - 1) && !is_toneporta {
            self.p.xc_data[chn].reset_note(NOTE_END);
            // sent even for an invalid sample, which cuts the channel
            let xc = &self.p.xc_data[chn];
            let (i, s, n) = (xc.ins, xc.smp, xc.note);
            self.set_patch(chn, i, s, n);
            note = self.p.xc_data[chn].note;
        }

        // key off and envelopes
        if key == XMP_KEY_OFF as i32 {
            let ins = self.p.xc_data[chn].ins;
            let env = if self.is_valid_instrument(ins) { Some(self.m.module.xxi[ins as usize].aei) } else { None };
            let xc = &mut self.p.xc_data[chn];
            match env {
                Some(env) if env.flg & XMP_ENVELOPE_ON != 0 => {
                    if Self::sustain_check(&env, xc.v_idx) {
                        // a release that takes effect in the next frame (OpenMPT EnvOff.xm)
                        xc.set_note(NOTE_SUSEXIT);
                    } else {
                        xc.set_note(NOTE_RELEASE);
                    }
                }
                // no volume envelope: volume cut (ft2_note_off_fade.xm)
                _ => Self::set_channel_volume(xc, 0),
            }
            // a key off always starts the fadeout
            xc.set_note(NOTE_FADEOUT);
        }
        if (ev.ins != 0 && key != XMP_KEY_OFF as i32) || is_delayed {
            // ins# without key off/K00, and delayed rows, reset release/fadeout
            {
                let xc = &mut self.p.xc_data[chn];
                xc.fadeout = 0x10000;
                xc.reset_note(NOTE_FADEOUT);
                xc.reset_note(NOTE_RELEASE | NOTE_SUSEXIT);
            }
            if sub.is_some() {
                // only with a valid active instrument (ft2_envelope_invalid_ins.xm)
                self.reset_envelopes(chn);
            }
            // the tremor count resets with the fadeout (ft2_tremor_reset.xm)
            self.p.xc_data[chn].tremor.count = TREMOR_SUPPRESS;
        }

        self.set_effect_defaults(note, sub, chn, is_toneporta);

        if ev.ins != 0 {
            // any ins#: the active sample's defaults, volume 0 / pan 0x80 if
            // invalid (ft2_invalid_ins_defaults.xm)
            let xc = &mut self.p.xc_data[chn];
            Self::set_channel_volume(xc, sub.map_or(0, |s| s.vol));
            Self::set_channel_pan(xc, sub.map_or(0x80, |s| s.pan));
        }

        Self::set_channel_volume(&mut self.p.xc_data[chn], ev.vol as i32 - 1);

        // FT2 always resets the sample offset
        self.p.xc_data[chn].offset.val = 0;

        self.process_fx(chn, &ev, 1);
        self.process_fx(chn, &ev, 0);
        self.set_period_ft2(key, note, sub, chn, is_toneporta);

        if self.p.xc_data[chn].test(NEW_VOL) {
            // tremor is overridden by volume changes (OpenMPT TremorRecover.xm)
            self.p.xc_data[chn].tremor.count |= TREMOR_SUPPRESS;
        }

        if note >= 0 {
            // the offset needs a valid note + 9xx + no toneporta; its memory is
            // set only then, and offsets past the end cut (ft2_offset_memory.xm)
            if ft2bugs && self.p.xc_data[chn].test(OFFSET) {
                let (smp, off) = (self.p.xc_data[chn].smp, self.p.xc_data[chn].offset.val);
                self.p.xc_data[chn].offset.memory = (off & 0xff00) >> 8;
                if !self.is_valid_sample(smp) || off >= self.m.module.xxs[smp as usize].len {
                    self.virt_resetchannel(chn as i32);
                }
            }
            let off = self.p.xc_data[chn].offset.val as f64;
            self.virt_voicepos(chn as i32, off);
        }
    }

    /// `read_event_st3`: Scream Tracker 3's note and instrument handling.
    fn read_event_st3(&mut self, e: &Event, chn: usize) {
        self.p.xc_data[chn].flags = 0;
        let mut note = -1;
        let mut not_same_ins = false;
        let mut is_toneporta = Self::is_toneporta_fx(e.fxt) || Self::is_toneporta_fx(e.f2t);

        if self.virt_mapchannel(chn as i32) < 0 && self.p.xc_data[chn].ins != e.ins as i32 - 1 {
            is_toneporta = false;
        }

        // instrument
        if Self::is_valid_note(e.note as i32 - 1) && !is_toneporta {
            self.p.xc_data[chn].key = e.note as i32 - 1;
        }
        if e.ins != 0 {
            let ins = e.ins as i32 - 1;
            {
                let xc = &mut self.p.xc_data[chn];
                xc.set(NEW_INS);
                xc.fadeout = 0x10000;
                xc.per_flags = 0;
                xc.offset.val = 0;
                xc.reset_note(NOTE_RELEASE | NOTE_FADEOUT);
            }
            if self.is_valid_instrument(ins) {
                if self.p.xc_data[chn].ins != ins {
                    not_same_ins = true;
                    if !is_toneporta {
                        let rls = self.m.module.xxi[ins as usize].rls;
                        let xc = &mut self.p.xc_data[chn];
                        xc.ins = ins;
                        xc.ins_fade = rls;
                    }
                }
                // the new instrument's volume
                let key = self.p.xc_data[chn].key;
                if let Some(s) = self.get_subinstrument(ins, key) {
                    if e.note != XMP_KEY_OFF {
                        let xc = &mut self.p.xc_data[chn];
                        Self::set_channel_volume(xc, s.vol);
                        Self::set_channel_pan(xc, s.pan);
                    }
                }
            } else {
                // invalid instruments are ignored
                self.p.xc_data[chn].flags = 0;
            }
        }

        // note
        if e.note != 0 {
            self.p.xc_data[chn].set(NEW_NOTE);
            if e.note == XMP_KEY_OFF {
                self.p.xc_data[chn].set_note(NOTE_RELEASE);
            } else if is_toneporta {
                // always retrig in tone portamento (7spirits.s3m, mod.Biomechanoid)
                if not_same_ins {
                    self.p.xc_data[chn].offset.val = 0;
                }
            } else if Self::is_valid_note(e.note as i32 - 1) {
                self.p.xc_data[chn].reset_note(NOTE_END);
                let (ins, key) = (self.p.xc_data[chn].ins, self.p.xc_data[chn].key);
                if let Some(s) = self.get_subinstrument(ins, key) {
                    let transp = self.m.module.xxi[ins as usize].map[key as usize].xpo as i32;
                    note = key + s.xpo + transp;
                    let mut smp = s.sid;
                    if !self.is_valid_sample(smp) {
                        smp = -1;
                    }
                    if smp >= 0 && (smp as usize) < self.m.module.smp {
                        self.set_patch(chn, ins, smp, note);
                        self.p.xc_data[chn].smp = smp;
                    }
                } else {
                    self.p.xc_data[chn].flags = 0;
                }
            }
        }

        // the playing subinstrument, which may not be e.ins's with toneporta
        let (ins, key) = (self.p.xc_data[chn].ins, self.p.xc_data[chn].key);
        let sub = self.get_subinstrument(ins, key);

        self.set_effect_defaults(note, sub, chn, is_toneporta);
        if e.ins != 0 && sub.is_some() {
            self.reset_envelopes(chn);
        }

        Self::set_channel_volume(&mut self.p.xc_data[chn], e.vol as i32 - 1);

        self.process_fx(chn, e, 1);
        self.process_fx(chn, e, 0);
        self.set_period(note, sub, chn, is_toneporta);

        if sub.is_none() {
            return;
        }

        if note >= 0 {
            self.p.xc_data[chn].note = note;
            let off = self.p.xc_data[chn].offset.val as f64;
            self.virt_voicepos(chn as i32, off);
        }

        if self.has_quirk(QUIRK_ST3BUGS) && self.p.xc_data[chn].test(NEW_VOL) {
            let (gvol, volbase) = (self.p.gvol, self.m.volbase);
            let xc = &mut self.p.xc_data[chn];
            xc.volume = xc.volume * gvol / volbase;
        }
    }

    /// `libxmp_read_event`. The ProTracker-style (`read_event_mod`), FT2 and ST3
    /// handlings are ported; the IT and MED ones come with the loaders that
    /// select them, and until then play as MOD.
    pub fn read_event(&mut self, e: &Event, chn: usize) {
        let xc = &mut self.p.xc_data[chn];
        if e.ins != 0 {
            xc.old_ins = e.ins as i32;
        }
        if xc.test_note(NOTE_SAMPLE_END) {
            xc.set_note(NOTE_END);
        }
        match self.m.read_event_type {
            ReadEvent::Ft2 => self.read_event_ft2(e, chn),
            ReadEvent::St3 => self.read_event_st3(e, chn),
            _ => self.read_event_mod(e, chn),
        }
    }
}
