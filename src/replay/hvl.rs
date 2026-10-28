//! HivelyTracker (`.hvl`, mark "HVL"), ported from NostalgicPlayer's
//! HivelyTracker player (MIT, Thomas Neumann), itself from the original replayer
//! (hvl_replay, BSD 3-Clause, Copyright (c) 2006-2018, Pete Gordon: see
//! LICENSE-hivelytracker), which this follows where the two differ (the next
//! position after the last is 0, not the restart position; a song ends as
//! the original's flags say, not by visiting a position twice). HivelyTracker
//! is AHX's successor: up to 16 channels with a pan each, two effects a step
//! and a play list entry, and a speed multiplier of 1 to 4. AHX-format modules
//! ("THX") play through `ahx.rs`, which follows the Eagleplayer's binary
//! replayer instead.
//!
//! The original is a software mixer: every voice is a 640-byte waveform loop
//! (the instrument's wavelength repeated) played at 3546895 / period Hz at a
//! volume of 0 to 64, panned with a quarter sine, and summed. There is no
//! Amiga player to check against (the Eagleplayer refuses these), and four
//! Paula channels cannot play more than four voices, so the port feeds the
//! host's Eagleplayer amplifier (`Hw::voice_*`) instead: each channel's
//! waveform loop is written to a buffer in chip memory, and plays on two
//! virtual voices, one on the left and one on the right, whose volumes are
//! the channel's volume scaled by the pan's left and right gains.
//!
//! What the amplifier cannot do, as it is a plain 8-bit mixer:
//! - the song's mix gain (default 100%) is not applied, and the amplifier's own
//!   boost for the voice count applies;
//! - ring modulation (play list commands 7 and 8) multiplies the voice's
//!   waveform by the modulating one at the ratio of their periods, read from
//!   the start of the loop, where the original keeps the two positions
//!   running on; it is written into the voice's buffer.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;
use std::sync::Arc;

pub const OPEN: Open = Open { name: "HivelyTracker", detect, new };

/// A voice's waveform loop.
const BUF: usize = 0x280;

fn detect(d: &[u8]) -> bool {
    d.len() >= 16 && d.starts_with(b"HVL") && d[3] <= 1
}

include!("hvl_tables.rs");

/// Periods of the notes 1 to 60 (index 0: no note).
const PERIODS: [i32; 61] = [
    0x0000, 0x0d60, 0x0ca0, 0x0be8, 0x0b40, 0x0a98, 0x0a00, 0x0970, 0x08e8, 0x0868, 0x07f0, 0x0780, 0x0714, 0x06b0,
    0x0650, 0x05f4, 0x05a0, 0x054c, 0x0500, 0x04b8, 0x0474, 0x0434, 0x03f8, 0x03c0, 0x038a, 0x0358, 0x0328, 0x02fa,
    0x02d0, 0x02a6, 0x0280, 0x025c, 0x023a, 0x021a, 0x01fc, 0x01e0, 0x01c5, 0x01ac, 0x0194, 0x017d, 0x0168, 0x0153,
    0x0140, 0x012e, 0x011d, 0x010d, 0x00fe, 0x00f0, 0x00e2, 0x00d6, 0x00ca, 0x00be, 0x00b4, 0x00aa, 0x00a0, 0x0097,
    0x008f, 0x0087, 0x007f, 0x0078, 0x0071,
];

const VIBRATO: [i32; 64] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24, 0, -24, -49, -74, -97, -120, -141, -161, -180, -197, -212, -224, -235, -244,
    -250, -253, -255, -253, -250, -244, -235, -224, -212, -197, -180, -161, -141, -120, -97, -74, -49, -24,
];

const STEREO_LEFT: [i32; 5] = [128, 96, 64, 32, 0];
const STEREO_RIGHT: [i32; 5] = [128, 160, 193, 225, 255];

/// Where a wavelength's triangle or sawtooth starts.
const OFFSETS: [usize; 6] = [0x00, 0x04, 0x0C, 0x1C, 0x3C, 0x7C];

// ---------------------------------------------------------------- waveforms

/// One filter setting's waveforms.
struct WaveSet {
    sawtooths: [i8; 0xFC],
    triangles: [i8; 0xFC],
    squares: Vec<i8>,
    noise: Vec<i8>,
}

impl WaveSet {
    fn new() -> Self {
        WaveSet { sawtooths: [0; 0xFC], triangles: [0; 0xFC], squares: vec![0; 0x80 * 0x20], noise: vec![0; BUF * 3] }
    }
}

struct Waves {
    /// 31 low passes, the plain waveforms (31), 31 high passes.
    sets: Vec<WaveSet>,
    pan_left: [i32; 256],
    pan_right: [i32; 256],
}

fn sawtooth(b: &mut [i8], off: usize, len: usize) {
    let add = 256 / (len as i32 - 1);
    let mut val = -128;
    for i in 0..len {
        b[off + i] = val as i8;
        val += add;
    }
}

fn triangle(b: &mut [i8], off: usize, len: usize) {
    let d5 = (len >> 2) as i32;
    let d1 = 128 / d5;
    let mut o = off;
    let mut val = 0i32;
    for _ in 0..d5 {
        b[o] = val as i8;
        o += 1;
        val += d1;
    }
    b[o] = 0x7f;
    o += 1;
    if d5 != 1 {
        val = 128;
        for _ in 0..d5 - 1 {
            val -= d1;
            b[o] = val as i8;
            o += 1;
        }
    }
    let mut o2 = o - (len >> 1);
    for _ in 0..d5 * 2 {
        let c = b[o2];
        o2 += 1;
        b[o] = if c == 0x7f { -128 } else { c.wrapping_neg() };
        o += 1;
    }
}

fn square(b: &mut [i8]) {
    let mut o = 0;
    for i in 1..=0x20usize {
        for _ in 0..(0x40 - i) * 2 {
            b[o] = -128;
            o += 1;
        }
        for _ in 0..i * 2 {
            b[o] = 127;
            o += 1;
        }
    }
}

fn white_noise(b: &mut [i8]) {
    let mut ays: u32 = 0x41595321;
    for o in b.iter_mut() {
        let mut s = ays as u8;
        if ays & 0x100 != 0 {
            s = 0x7f;
            if ays & 0x8000 != 0 {
                s = 0x80;
            }
        }
        *o = s as i8;
        ays = ays.rotate_right(5);
        ays = (ays & 0xffffff00) | ((ays & 0xff) ^ 0x9a);
        let mut bx = ays as u16;
        ays = ays.rotate_left(2);
        let mut ax = ays as u16;
        bx = bx.wrapping_add(ax);
        ax ^= bx;
        ays = (ays & 0xffff0000) | ax as u32;
        ays = ays.rotate_right(3);
    }
}

fn clip8(x: i32) -> i32 {
    let top = (x >> 16) as i16;
    if top > 127 {
        127 << 16
    } else if top < -128 {
        -(128 << 16)
    } else {
        x
    }
}

/// One waveform through the filter at `freq`: `lo` and `hi` get the low and
/// high pass at the same place.
fn filter(input: &[i8], len: usize, freq: i32, k: &mut usize, lo: &mut [i8], hi: &mut [i8]) {
    let mut mid = (MID_FILTER[*k] as i32) << 8;
    let mut low = (LOW_FILTER[*k] as i32) << 8;
    *k += 1;
    for i in 0..len {
        let x = (input[i] as i32) << 16;
        let high = clip8(x - mid - low);
        mid = clip8(mid + (high >> 8) * freq);
        low = clip8(low + (mid >> 8) * freq);
        hi[i] = (high >> 16) as i8;
        lo[i] = (low >> 16) as i8;
    }
}

impl Waves {
    fn new() -> Self {
        let mut w = Waves { sets: (0..63).map(|_| WaveSet::new()).collect(), pan_left: [0; 256], pan_right: [0; 256] };
        let q: f64 = 3.14159265 * 2.0 / 4.0;
        let (mut aa, mut ab) = (q, 0.0f64);
        for i in 0..256 {
            w.pan_left[i] = (aa.sin() * 255.0) as i32;
            w.pan_right[i] = (ab.sin() * 255.0) as i32;
            aa += q / 256.0;
            ab += q / 256.0;
        }
        w.pan_left[255] = 0;
        w.pan_right[0] = 0;
        {
            let s = &mut w.sets[31];
            for (k, &o) in OFFSETS.iter().enumerate() {
                sawtooth(&mut s.sawtooths, o, 4 << k);
                triangle(&mut s.triangles, o, 4 << k);
            }
            square(&mut s.squares);
            white_noise(&mut s.noise);
        }
        let (low_sets, rest) = w.sets.split_at_mut(31);
        let (src, high_sets) = rest.split_first_mut().unwrap();
        let mut k = 0;
        let mut freq = 25;
        for i in 0..31 {
            let (lo, hi) = (&mut low_sets[i], &mut high_sets[i]);
            for (j, &o) in OFFSETS.iter().enumerate() {
                let len = 4 << j;
                filter(&src.sawtooths[o..], len, freq, &mut k, &mut lo.sawtooths[o..], &mut hi.sawtooths[o..]);
            }
            for (j, &o) in OFFSETS.iter().enumerate() {
                let len = 4 << j;
                filter(&src.triangles[o..], len, freq, &mut k, &mut lo.triangles[o..], &mut hi.triangles[o..]);
            }
            filter(&src.noise, BUF * 3, freq, &mut k, &mut lo.noise, &mut hi.noise);
            for j in 0..32 {
                let o = j * 0x80;
                filter(&src.squares[o..], 0x80, freq, &mut k, &mut lo.squares[o..], &mut hi.squares[o..]);
            }
            freq += 9;
        }
        w
    }
}

// --------------------------------------------------------------------- song

#[derive(Clone, Default)]
struct Envelope {
    a_frames: i32,
    a_volume: i32,
    d_frames: i32,
    d_volume: i32,
    s_frames: i32,
    r_frames: i32,
    r_volume: i32,
}

#[derive(Clone, Default)]
struct PlEntry {
    note: i32,
    fixed: bool,
    waveform: i32,
    fx: [i32; 2],
    fx_param: [i32; 2],
}

#[derive(Clone, Default)]
struct Instrument {
    name: String,
    volume: i32,
    wave_length: i32,
    envelope: Envelope,
    filter_lower: i32,
    filter_upper: i32,
    filter_speed: i32,
    square_lower: i32,
    square_upper: i32,
    square_speed: i32,
    vibrato_delay: i32,
    vibrato_depth: i32,
    vibrato_speed: i32,
    hard_cut_frames: i32,
    hard_cut_release: bool,
    pl_speed: i32,
    pl: Vec<PlEntry>,
}

#[derive(Clone, Copy, Default)]
struct Step {
    note: i32,
    instrument: i32,
    fx: i32,
    fx_param: i32,
    fx_b: i32,
    fx_b_param: i32,
}

struct Song {
    name: String,
    restart: i32,
    positions: usize,
    track_length: i32,
    channels: usize,
    speed_multiplier: i32,
    default_pan: [i32; 2],
    subsongs: Vec<u16>,
    /// `positions * channels` track numbers and transposes.
    pos_track: Vec<u8>,
    pos_transpose: Vec<i8>,
    tracks: Vec<Vec<Step>>,
    instruments: Vec<Instrument>,
}

fn cstr(d: &[u8], at: usize) -> (String, usize) {
    let s = d.get(at..).unwrap_or(&[]);
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    (s[..n].iter().map(|&c| c as char).collect(), at + n + 1)
}

fn load(d: &[u8]) -> Option<Song> {
    if !detect(d) {
        return None;
    }
    let flag = d[6];
    let positions = (((flag & 0xf) as usize) << 8) | d[7] as usize;
    let channels = ((d[8] >> 2) + 4) as usize;
    let restart = (((d[8] & 3) as i32) << 8) | d[9] as i32;
    let track_length = d[10] as i32;
    let track_nr = d[11] as usize;
    let instrument_nr = d[12] as usize;
    let subsong_nr = d[13] as usize;
    let stereo = (d[15] as usize).min(4);
    if positions < 1 || positions > 1000 || track_length < 1 || track_length > 64 || instrument_nr > 63 {
        return None;
    }
    let mut p = 16;
    let mut subsongs = Vec::new();
    for _ in 0..subsong_nr {
        subsongs.push(be16(d, p));
        p += 2;
    }
    let n = positions * channels;
    let body = d.get(p..p + 2 * n)?;
    let pos_track = (0..n).map(|i| body[2 * i]).collect();
    let pos_transpose = (0..n).map(|i| body[2 * i + 1] as i8).collect();
    p += 2 * n;
    let mut tracks = Vec::new();
    for i in 0..=track_nr {
        let mut t = vec![Step::default(); track_length as usize];
        if !(flag & 0x80 != 0 && i == 0) {
            for s in t.iter_mut() {
                let b1 = *d.get(p)?;
                p += 1;
                if b1 == 0x3f {
                    continue;
                }
                let b = d.get(p..p + 4)?;
                p += 4;
                *s = Step {
                    note: b1 as i32,
                    instrument: b[0] as i32,
                    fx: (b[1] >> 4) as i32,
                    fx_param: b[2] as i32,
                    fx_b: (b[1] & 0xf) as i32,
                    fx_b_param: b[3] as i32,
                };
            }
        }
        tracks.push(t);
    }
    let mut instruments = vec![Instrument::default(); instrument_nr + 1];
    for ins in instruments.iter_mut().skip(1) {
        let h = d.get(p..p + 22)?;
        p += 22;
        ins.volume = h[0] as i32;
        ins.wave_length = ((h[1] & 7) as i32).min(5);
        ins.envelope = Envelope {
            a_frames: h[2] as i32,
            a_volume: h[3] as i32,
            d_frames: h[4] as i32,
            d_volume: h[5] as i32,
            s_frames: h[6] as i32,
            r_frames: h[7] as i32,
            r_volume: h[8] as i32,
        };
        ins.filter_speed = (((h[1] >> 3) & 0x1f) | ((h[12] >> 2) & 0x20)) as i32;
        ins.filter_lower = (h[12] & 0x7f) as i32;
        ins.vibrato_delay = h[13] as i32;
        ins.hard_cut_frames = ((h[14] >> 4) & 7) as i32;
        ins.hard_cut_release = h[14] & 0x80 != 0;
        ins.vibrato_depth = (h[14] & 0xf) as i32;
        ins.vibrato_speed = h[15] as i32;
        ins.square_lower = h[16] as i32;
        ins.square_upper = h[17] as i32;
        ins.square_speed = h[18] as i32;
        ins.filter_upper = (h[19] & 0x3f) as i32;
        ins.pl_speed = h[20] as i32;
        let len = h[21] as usize;
        for _ in 0..len {
            let e = d.get(p..p + 5)?;
            p += 5;
            ins.pl.push(PlEntry {
                fx: [(e[0] & 0xf) as i32, ((e[1] >> 3) & 0xf) as i32],
                waveform: (e[1] & 7) as i32,
                fixed: (e[2] >> 6) & 1 != 0,
                note: (e[2] & 0x3f) as i32,
                fx_param: [e[3] as i32, e[4] as i32],
            });
        }
    }
    // the name is where the header says; the instruments' follow it
    let (name, mut q) = cstr(d, ((d[4] as usize) << 8) | d[5] as usize);
    for ins in instruments.iter_mut().skip(1) {
        let (n, next) = cstr(d, q);
        ins.name = n;
        q = next;
    }
    Some(Song {
        name,
        restart: restart.min(positions as i32 - 1),
        positions,
        track_length,
        channels,
        speed_multiplier: (((flag >> 5) & 3) + 1) as i32,
        default_pan: [STEREO_LEFT[stereo], STEREO_RIGHT[stereo]],
        subsongs,
        pos_track,
        pos_transpose,
        tracks,
        instruments,
    })
}

// ------------------------------------------------------------------ replayer

/// Where a voice's waveform comes from.
#[derive(Clone, Copy, PartialEq)]
enum Wave {
    None,
    Triangle(usize),
    Sawtooth(usize),
    Noise(usize),
    /// The temporary square of a voice.
    Square(usize),
}

#[derive(Clone)]
struct Voice {
    track: i32,
    transpose: i32,
    next_track: i32,
    override_transpose: i32,
    adsr_volume: i32,
    adsr: Envelope,
    instrument: usize,
    instr_period: i32,
    track_period: i32,
    vibrato_period: i32,
    note_max_volume: i32,
    perf_sub_volume: i32,
    track_master_volume: i32,
    new_waveform: bool,
    waveform: i32,
    plant_square: bool,
    plant_period: bool,
    ignore_square: bool,
    track_on: bool,
    fixed_note: bool,
    volume_slide_up: i32,
    volume_slide_down: i32,
    hard_cut: i32,
    hard_cut_release: bool,
    hard_cut_release_f: i32,
    period_slide_speed: i32,
    period_slide_period: i32,
    period_slide_limit: i32,
    period_slide_on: bool,
    period_slide_with_limit: bool,
    period_perf_slide_speed: i32,
    period_perf_slide_period: i32,
    period_perf_slide_on: bool,
    vibrato_delay: i32,
    vibrato_current: i32,
    vibrato_depth: i32,
    vibrato_speed: i32,
    square_on: bool,
    square_init: bool,
    square_wait: i32,
    square_lower: i32,
    square_upper: i32,
    square_pos: i32,
    square_sign: i32,
    square_sliding_in: bool,
    filter_on: bool,
    filter_init: bool,
    filter_wait: i32,
    filter_lower: i32,
    filter_upper: i32,
    filter_pos: i32,
    filter_sign: i32,
    filter_speed: i32,
    filter_sliding_in: bool,
    ignore_filter: i32,
    perf_current: usize,
    perf_speed: i32,
    perf_wait: i32,
    wave_length: i32,
    note_delay_wait: i32,
    note_delay_on: bool,
    note_cut_wait: i32,
    note_cut_on: bool,
    pan: i32,
    set_pan: i32,
    ring_plant_period: bool,
    ring_base_period: i32,
    ring_audio_period: i32,
    ring_new_waveform: bool,
    ring_waveform: i32,
    ring_fixed_period: bool,
    ring_mix: bool,
    audio_source: Wave,
    audio_offset: usize,
    ring_audio_source: Wave,
    ring_audio_offset: usize,
    audio_period: i32,
    audio_volume: i32,
    wn_random: i32,
    // what the mixer reads
    voice_volume: i32,
    voice_period: i32,
    voice_buffer: [i8; BUF + 1],
    ring_buffer: [i8; BUF + 1],
    square_temp: [i8; 0x80],
}

impl Voice {
    fn new(num: usize, song: &Song) -> Self {
        let pan = if num % 4 == 0 || num % 4 == 3 { song.default_pan[0] } else { song.default_pan[1] };
        Voice {
            track: 0,
            transpose: 0,
            next_track: 0,
            override_transpose: 1000,
            adsr_volume: 0,
            adsr: Envelope::default(),
            instrument: 0,
            instr_period: 0,
            track_period: 0,
            vibrato_period: 0,
            note_max_volume: 0,
            perf_sub_volume: 0,
            track_master_volume: 0x40,
            new_waveform: false,
            waveform: 0,
            plant_square: false,
            plant_period: false,
            ignore_square: false,
            track_on: true,
            fixed_note: false,
            volume_slide_up: 0,
            volume_slide_down: 0,
            hard_cut: 0,
            hard_cut_release: false,
            hard_cut_release_f: 0,
            period_slide_speed: 0,
            period_slide_period: 0,
            period_slide_limit: 0,
            period_slide_on: false,
            period_slide_with_limit: false,
            period_perf_slide_speed: 0,
            period_perf_slide_period: 0,
            period_perf_slide_on: false,
            vibrato_delay: 0,
            vibrato_current: 0,
            vibrato_depth: 0,
            vibrato_speed: 0,
            square_on: false,
            square_init: false,
            square_wait: 0,
            square_lower: 0,
            square_upper: 0,
            square_pos: 0,
            square_sign: 0,
            square_sliding_in: false,
            filter_on: false,
            filter_init: false,
            filter_wait: 0,
            filter_lower: 0,
            filter_upper: 0,
            filter_pos: 32,
            filter_sign: 0,
            filter_speed: 0,
            filter_sliding_in: false,
            ignore_filter: 0,
            perf_current: 0,
            perf_speed: 0,
            perf_wait: 0,
            wave_length: 0,
            note_delay_wait: 0,
            note_delay_on: false,
            note_cut_wait: 0,
            note_cut_on: false,
            pan,
            set_pan: pan,
            ring_plant_period: false,
            ring_base_period: 0,
            ring_audio_period: 0,
            ring_new_waveform: false,
            ring_waveform: 0,
            ring_fixed_period: false,
            ring_mix: false,
            audio_source: Wave::None,
            audio_offset: 0,
            ring_audio_source: Wave::None,
            ring_audio_offset: 0,
            audio_period: 0,
            audio_volume: 0,
            wn_random: 0x280,
            voice_volume: 0,
            voice_period: 0,
            voice_buffer: [0; BUF + 1],
            ring_buffer: [0; BUF + 1],
            square_temp: [0; 0x80],
        }
    }
}

/// The player proper, without the hardware.
struct Core {
    song: Arc<Song>,
    waves: Arc<Waves>,
    voices: Vec<Voice>,
    pos_nr: i32,
    pos_jump: i32,
    pattern_break: bool,
    note_nr: i32,
    pos_jump_note: i32,
    tempo: i32,
    step_wait: i16,
    get_new_position: bool,
    songend: bool,
    /// The voice whose square the "square" waveform is (the original has one
    /// pointer for all voices).
    square_wave: Option<usize>,
}

impl Core {
    fn new(song: Arc<Song>, waves: Arc<Waves>) -> Self {
        let mut c = Core {
            song,
            waves,
            voices: Vec::new(),
            pos_nr: 0,
            pos_jump: 0,
            pattern_break: false,
            note_nr: 0,
            pos_jump_note: 0,
            tempo: 6,
            step_wait: 0,
            get_new_position: true,
            songend: false,
            square_wave: None,
        };
        c.init_subsong(0);
        c
    }

    /// The original's InitSubsong; false past the last.
    fn init_subsong(&mut self, n: usize) -> bool {
        let song = self.song.clone();
        if n > song.subsongs.len() {
            return false;
        }
        self.pos_nr = if n > 0 { song.subsongs[n - 1] as i32 } else { 0 };
        self.pos_jump = 0;
        self.pattern_break = false;
        self.note_nr = 0;
        self.pos_jump_note = 0;
        self.tempo = 6;
        self.step_wait = 0;
        self.get_new_position = true;
        self.songend = false;
        self.voices = (0..song.channels).map(|v| Voice::new(v, &song)).collect();
        true
    }

    fn wave(&self, w: Wave, off: usize) -> i8 {
        match w {
            Wave::None => 0,
            Wave::Triangle(f) => self.waves.sets[f].triangles.get(off).copied().unwrap_or(0),
            Wave::Sawtooth(f) => self.waves.sets[f].sawtooths.get(off).copied().unwrap_or(0),
            Wave::Noise(f) => self.waves.sets[f].noise.get(off).copied().unwrap_or(0),
            Wave::Square(v) => self.voices[v].square_temp.get(off).copied().unwrap_or(0),
        }
    }

    fn step_of(&self, track: i32, note: i32) -> Step {
        self.song.tracks.get(track as usize).and_then(|t| t.get(note as usize)).copied().unwrap_or_default()
    }

    fn play_irq(&mut self) {
        let song = self.song.clone();
        let ch = song.channels;
        if self.step_wait == 0 {
            if self.get_new_position {
                let pos = (self.pos_nr.max(0) as usize).min(song.positions - 1);
                let next = if pos + 1 == song.positions { 0 } else { pos + 1 };
                for i in 0..ch {
                    let v = &mut self.voices[i];
                    v.track = song.pos_track[pos * ch + i] as i32;
                    v.transpose = song.pos_transpose[pos * ch + i] as i32;
                    v.next_track = song.pos_track[next * ch + i] as i32;
                }
                self.get_new_position = false;
            }
            for i in 0..ch {
                self.process_step(i);
            }
            self.step_wait = self.tempo as i16;
        }
        for i in 0..ch {
            self.process_frame(i);
        }
        self.step_wait = self.step_wait.wrapping_sub(1);
        if self.step_wait == 0 {
            if !self.pattern_break {
                self.note_nr += 1;
                if self.note_nr >= song.track_length {
                    self.pos_jump = self.pos_nr + 1;
                    self.pos_jump_note = 0;
                    self.pattern_break = true;
                }
            }
            if self.pattern_break {
                self.pattern_break = false;
                self.pos_nr = self.pos_jump;
                self.note_nr = self.pos_jump_note;
                if self.pos_nr >= song.positions as i32 {
                    self.songend = true;
                    self.pos_nr = song.restart;
                }
                self.pos_jump_note = 0;
                self.pos_jump = 0;
                self.get_new_position = true;
            }
        }
        for i in 0..ch {
            self.set_audio(i);
        }
    }

    /// The note delay of one of a step's effects; true if the step waits.
    fn note_delay(&mut self, v: usize, fx: i32, param: i32, done: &mut bool) -> bool {
        if fx & 0xf == 0xe && param & 0xf0 == 0xd0 {
            let tempo = self.tempo;
            let voice = &mut self.voices[v];
            if voice.note_delay_on {
                voice.note_delay_on = false;
                *done = true;
            } else if (param & 0x0f) < tempo {
                voice.note_delay_wait = param & 0x0f;
                if voice.note_delay_wait != 0 {
                    voice.note_delay_on = true;
                    return true;
                }
            }
        }
        false
    }

    fn process_step(&mut self, v: usize) {
        if !self.voices[v].track_on {
            return;
        }
        let song = self.song.clone();
        self.voices[v].volume_slide_up = 0;
        self.voices[v].volume_slide_down = 0;
        let pos = (self.pos_nr.max(0) as usize).min(song.positions - 1);
        let track = song.pos_track[pos * song.channels + v] as i32;
        let step = self.step_of(track, self.note_nr);
        let mut note = step.note;
        let mut done = false;
        if self.note_delay(v, step.fx, step.fx_param, &mut done) {
            return;
        }
        if !done && self.note_delay(v, step.fx_b, step.fx_b_param, &mut done) {
            return;
        }
        if note != 0 {
            self.voices[v].override_transpose = 1000;
        }
        self.step_fx1(v, step.fx & 0xf, step.fx_param);
        self.step_fx1(v, step.fx_b & 0xf, step.fx_b_param);
        if step.instrument != 0 && step.instrument as usize <= song.instruments.len() - 1 {
            let ins = &song.instruments[step.instrument as usize];
            let voice = &mut self.voices[v];
            voice.pan = voice.set_pan;
            voice.period_slide_speed = 0;
            voice.period_slide_period = 0;
            voice.period_slide_limit = 0;
            voice.perf_sub_volume = 0x40;
            voice.adsr_volume = 0;
            voice.instrument = step.instrument as usize;
            let (s, d) = (&ins.envelope, &mut voice.adsr);
            d.a_frames = s.a_frames;
            d.a_volume = if d.a_frames != 0 { s.a_volume * 256 / d.a_frames } else { s.a_volume * 256 };
            d.d_frames = s.d_frames;
            d.d_volume = if d.d_frames != 0 { (s.d_volume - s.a_volume) * 256 / d.d_frames } else { s.d_volume * 256 };
            d.s_frames = s.s_frames;
            d.r_frames = s.r_frames;
            d.r_volume = if d.r_frames != 0 { (s.r_volume - s.d_volume) * 256 / d.r_frames } else { s.r_volume * 256 };
            voice.wave_length = ins.wave_length;
            voice.note_max_volume = ins.volume;
            voice.vibrato_current = 0;
            voice.vibrato_delay = ins.vibrato_delay;
            voice.vibrato_depth = ins.vibrato_depth;
            voice.vibrato_speed = ins.vibrato_speed;
            voice.vibrato_period = 0;
            voice.hard_cut_release = ins.hard_cut_release;
            voice.hard_cut = ins.hard_cut_frames;
            voice.ignore_square = false;
            voice.square_sliding_in = false;
            voice.square_wait = 0;
            voice.square_on = false;
            let sh = 5 - voice.wave_length;
            let (mut lower, mut upper) = (ins.square_lower >> sh, ins.square_upper >> sh);
            if upper < lower {
                std::mem::swap(&mut upper, &mut lower);
            }
            voice.square_upper = upper;
            voice.square_lower = lower;
            voice.ignore_filter = 0;
            voice.filter_wait = 0;
            voice.filter_on = false;
            voice.filter_sliding_in = false;
            let mut d6 = ins.filter_speed;
            let (mut d3, mut d4) = (ins.filter_lower, ins.filter_upper);
            if d3 & 0x80 != 0 {
                d6 |= 0x20;
            }
            if d4 & 0x80 != 0 {
                d6 |= 0x40;
            }
            voice.filter_speed = d6;
            d3 &= !0x80;
            d4 &= !0x80;
            if d3 > d4 {
                std::mem::swap(&mut d3, &mut d4);
            }
            voice.filter_upper = d4;
            voice.filter_lower = d3;
            voice.filter_pos = 32;
            voice.perf_wait = 0;
            voice.perf_current = 0;
            voice.perf_speed = ins.pl_speed;
            voice.ring_mix = false;
            voice.ring_plant_period = false;
            voice.ring_new_waveform = false;
        }
        self.voices[v].period_slide_on = false;
        self.step_fx2(v, step.fx & 0xf, step.fx_param, &mut note);
        self.step_fx2(v, step.fx_b & 0xf, step.fx_b_param, &mut note);
        if note != 0 {
            let voice = &mut self.voices[v];
            voice.track_period = note;
            voice.plant_period = true;
        }
        self.step_fx3(v, step.fx & 0xf, step.fx_param);
        self.step_fx3(v, step.fx_b & 0xf, step.fx_b_param);
    }

    fn step_fx1(&mut self, v: usize, fx: i32, mut param: i32) {
        let song = self.song.clone();
        match fx {
            0x0 => {
                if param & 0x0f > 0 && param & 0x0f <= 9 {
                    self.pos_jump = param & 0x0f;
                }
            }
            0x5 | 0xa => {
                self.voices[v].volume_slide_down = param & 0x0f;
                self.voices[v].volume_slide_up = param >> 4;
            }
            0x7 => {
                if param > 127 {
                    param -= 256;
                }
                self.voices[v].pan = param + 128;
                self.voices[v].set_pan = param + 128;
            }
            0xb => {
                self.pos_jump = self.pos_jump * 100 + (param & 0x0f) + (param >> 4) * 10;
                self.pattern_break = true;
                if self.pos_jump <= self.pos_nr {
                    self.songend = true;
                }
            }
            0xd => {
                self.pos_jump = self.pos_nr + 1;
                self.pos_jump_note = (param & 0x0f) + (param >> 4) * 10;
                self.pattern_break = true;
                if self.pos_jump_note > song.track_length {
                    self.pos_jump_note = 0;
                }
            }
            0xe => {
                if param >> 4 == 0xc && (param & 0x0f) < self.tempo {
                    let voice = &mut self.voices[v];
                    voice.note_cut_wait = param & 0x0f;
                    if voice.note_cut_wait != 0 {
                        voice.note_cut_on = true;
                        voice.hard_cut_release = false;
                    }
                }
            }
            0xf => {
                self.tempo = param;
                if param == 0 {
                    self.songend = true;
                }
            }
            _ => {}
        }
    }

    fn step_fx2(&mut self, v: usize, fx: i32, param: i32, note: &mut i32) {
        match fx {
            0x9 => {
                let voice = &mut self.voices[v];
                voice.square_pos = param >> (5 - voice.wave_length);
                voice.ignore_square = true;
            }
            0x3 | 0x5 => {
                let voice = &mut self.voices[v];
                if fx == 0x3 && param != 0 {
                    voice.period_slide_speed = param;
                }
                if *note != 0 {
                    let new = PERIODS[(*note).clamp(0, 60) as usize];
                    let mut diff = PERIODS[voice.track_period.clamp(0, 60) as usize];
                    diff -= new;
                    let new = diff + voice.period_slide_period;
                    if new != 0 {
                        voice.period_slide_limit = -diff;
                    }
                }
                voice.period_slide_on = true;
                voice.period_slide_with_limit = true;
                *note = 0;
            }
            _ => {}
        }
    }

    fn step_fx3(&mut self, v: usize, fx: i32, mut param: i32) {
        let channels = self.voices.len();
        match fx {
            0x1 => {
                let voice = &mut self.voices[v];
                voice.period_slide_speed = -param;
                voice.period_slide_on = true;
                voice.period_slide_with_limit = false;
            }
            0x2 => {
                let voice = &mut self.voices[v];
                voice.period_slide_speed = param;
                voice.period_slide_on = true;
                voice.period_slide_with_limit = false;
            }
            0x4 => {
                let voice = &mut self.voices[v];
                if param == 0 || param == 0x40 {
                    return;
                }
                if param < 0x40 {
                    voice.ignore_filter = param;
                    return;
                }
                if param > 0x7f {
                    return;
                }
                voice.filter_pos = param - 0x40;
            }
            0xc => {
                if param <= 0x40 {
                    self.voices[v].note_max_volume = param;
                    return;
                }
                param -= 0x50;
                if param < 0 {
                    return;
                }
                if param <= 0x40 {
                    for i in 0..channels {
                        self.voices[i].track_master_volume = param;
                    }
                    return;
                }
                param -= 0xa0 - 0x50;
                if param < 0 {
                    return;
                }
                if param <= 0x40 {
                    self.voices[v].track_master_volume = param;
                }
            }
            0xe => {
                let voice = &mut self.voices[v];
                match param >> 4 {
                    0x1 => {
                        voice.period_slide_period = wrap16(voice.period_slide_period - (param & 0x0f));
                        voice.plant_period = true;
                    }
                    0x2 => {
                        voice.period_slide_period = wrap16(voice.period_slide_period + (param & 0x0f));
                        voice.plant_period = true;
                    }
                    0x4 => voice.vibrato_depth = param & 0x0f,
                    0xa => voice.note_max_volume = (voice.note_max_volume + (param & 0x0f)).min(0x40),
                    0xb => voice.note_max_volume = (voice.note_max_volume - (param & 0x0f)).max(0),
                    0xf => {
                        if param & 0xf == 1 {
                            voice.override_transpose = voice.transpose;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn process_frame(&mut self, v: usize) {
        if !self.voices[v].track_on {
            return;
        }
        let song = self.song.clone();
        if self.voices[v].note_delay_on {
            if self.voices[v].note_delay_wait <= 0 {
                self.process_step(v);
            } else {
                self.voices[v].note_delay_wait -= 1;
            }
        }
        let tempo = self.tempo;
        if self.voices[v].hard_cut != 0 {
            let voice = &self.voices[v];
            let next_instrument = if self.note_nr + 1 < song.track_length {
                self.step_of(voice.track, self.note_nr + 1).instrument
            } else {
                self.step_of(voice.next_track, 0).instrument
            };
            if next_instrument != 0 {
                let d1 = (tempo - voice.hard_cut).max(0);
                let voice = &mut self.voices[v];
                if !voice.note_cut_on {
                    voice.note_cut_on = true;
                    voice.note_cut_wait = d1;
                    voice.hard_cut_release_f = -(d1 - tempo);
                } else {
                    voice.hard_cut = 0;
                }
            }
        }
        let ins = &song.instruments[self.voices[v].instrument];
        let voice = &mut self.voices[v];
        if voice.note_cut_on {
            if voice.note_cut_wait <= 0 {
                voice.note_cut_on = false;
                if voice.hard_cut_release {
                    voice.adsr.r_frames = voice.hard_cut_release_f;
                    voice.adsr.r_volume = 0;
                    if voice.adsr.r_frames > 0 {
                        voice.adsr.r_volume = -(voice.adsr_volume - (ins.envelope.r_volume << 8)) / voice.adsr.r_frames;
                    }
                    voice.adsr.a_frames = 0;
                    voice.adsr.d_frames = 0;
                    voice.adsr.s_frames = 0;
                } else {
                    voice.note_max_volume = 0;
                }
            } else {
                voice.note_cut_wait -= 1;
            }
        }
        // envelope
        if voice.adsr.a_frames != 0 {
            voice.adsr_volume += voice.adsr.a_volume;
            voice.adsr.a_frames -= 1;
            if voice.adsr.a_frames <= 0 {
                voice.adsr_volume = ins.envelope.a_volume << 8;
            }
        } else if voice.adsr.d_frames != 0 {
            voice.adsr_volume += voice.adsr.d_volume;
            voice.adsr.d_frames -= 1;
            if voice.adsr.d_frames <= 0 {
                voice.adsr_volume = ins.envelope.d_volume << 8;
            }
        } else if voice.adsr.s_frames != 0 {
            voice.adsr.s_frames -= 1;
        } else if voice.adsr.r_frames != 0 {
            voice.adsr_volume += voice.adsr.r_volume;
            voice.adsr.r_frames -= 1;
            if voice.adsr.r_frames <= 0 {
                voice.adsr_volume = ins.envelope.r_volume << 8;
            }
        }
        voice.note_max_volume = (voice.note_max_volume + voice.volume_slide_up - voice.volume_slide_down).clamp(0, 0x40);
        // portamento
        if voice.period_slide_on {
            if voice.period_slide_with_limit {
                let mut d0 = voice.period_slide_period - voice.period_slide_limit;
                let mut d2 = voice.period_slide_speed;
                if d0 > 0 {
                    d2 = -d2;
                }
                if d0 != 0 {
                    let d3 = (d0 + d2) ^ d0;
                    d0 = if d3 >= 0 { voice.period_slide_period + d2 } else { voice.period_slide_limit };
                    voice.period_slide_period = d0;
                    voice.plant_period = true;
                }
            } else {
                voice.period_slide_period = wrap16(voice.period_slide_period + voice.period_slide_speed);
                voice.plant_period = true;
            }
        }
        // vibrato
        if voice.vibrato_depth != 0 {
            if voice.vibrato_delay <= 0 {
                voice.vibrato_period = (VIBRATO[voice.vibrato_current as usize & 63] * voice.vibrato_depth) >> 7;
                voice.plant_period = true;
                voice.vibrato_current = (voice.vibrato_current + voice.vibrato_speed) & 0x3f;
            } else {
                voice.vibrato_delay -= 1;
            }
        }
        // play list
        if voice.instrument != 0 {
            if voice.perf_current < ins.pl.len() {
                let overflow = voice.perf_wait == 128;
                voice.perf_wait -= 1;
                if overflow || (voice.perf_wait as i8) <= 0 {
                    let cur = voice.perf_current;
                    voice.perf_current += 1;
                    voice.perf_wait = voice.perf_speed;
                    let entry = &ins.pl[cur];
                    if entry.waveform != 0 {
                        voice.waveform = entry.waveform - 1;
                        voice.new_waveform = true;
                        voice.period_perf_slide_speed = 0;
                        voice.period_perf_slide_period = 0;
                    }
                    voice.period_perf_slide_on = false;
                    for i in 0..2 {
                        self.pl_command(v, entry.fx[i], entry.fx_param[i]);
                    }
                    let voice = &mut self.voices[v];
                    if entry.note != 0 {
                        voice.instr_period = entry.note;
                        voice.plant_period = true;
                        voice.fixed_note = entry.fixed;
                    }
                }
            } else if voice.perf_wait != 0 {
                voice.perf_wait -= 1;
            } else {
                voice.period_perf_slide_speed = 0;
            }
        }
        let voice = &mut self.voices[v];
        if voice.period_perf_slide_on {
            voice.period_perf_slide_period = wrap16(voice.period_perf_slide_period - voice.period_perf_slide_speed);
            if voice.period_perf_slide_period != 0 {
                voice.plant_period = true;
            }
        }
        if voice.waveform == 2 && voice.square_on {
            voice.square_wait -= 1;
            if voice.square_wait <= 0 {
                let (d1, d2) = (voice.square_lower, voice.square_upper);
                let mut d3 = voice.square_pos;
                if voice.square_init {
                    voice.square_init = false;
                    if d3 <= d1 {
                        voice.square_sliding_in = true;
                        voice.square_sign = 1;
                    } else if d3 >= d2 {
                        voice.square_sliding_in = true;
                        voice.square_sign = -1;
                    }
                }
                if d1 == d3 || d2 == d3 {
                    if voice.square_sliding_in {
                        voice.square_sliding_in = false;
                    } else {
                        voice.square_sign = -voice.square_sign;
                    }
                }
                d3 += voice.square_sign;
                voice.square_pos = d3;
                voice.plant_square = true;
                voice.square_wait = ins.square_speed;
            }
        }
        if voice.filter_on {
            voice.filter_wait -= 1;
            if voice.filter_wait <= 0 {
                let (d1, d2) = (voice.filter_lower, voice.filter_upper);
                let mut d3 = voice.filter_pos;
                if voice.filter_init {
                    voice.filter_init = false;
                    if d3 <= d1 {
                        voice.filter_sliding_in = true;
                        voice.filter_sign = 1;
                    } else if d3 >= d2 {
                        voice.filter_sliding_in = true;
                        voice.filter_sign = -1;
                    }
                }
                let f_max = if voice.filter_speed < 4 { 5 - voice.filter_speed } else { 1 };
                for _ in 0..f_max {
                    if d1 == d3 || d2 == d3 {
                        if voice.filter_sliding_in {
                            voice.filter_sliding_in = false;
                        } else {
                            voice.filter_sign = -voice.filter_sign;
                        }
                    }
                    d3 += voice.filter_sign;
                }
                voice.filter_pos = d3.clamp(1, 63);
                voice.new_waveform = true;
                voice.filter_wait = (voice.filter_speed - 3).max(1);
            }
        }
        if voice.waveform == 2 || voice.plant_square {
            let set = &self.waves.sets[(voice.filter_pos.clamp(1, 63) - 1) as usize];
            let mut x = voice.square_pos << (5 - voice.wave_length);
            if x > 0x20 {
                x = 0x40 - x;
            }
            let mut off = if x > 0 { ((x - 1) << 7) as usize } else { 0 };
            let delta = (32 >> voice.wave_length) as usize;
            self.square_wave = Some(v);
            for i in 0..(1usize << voice.wave_length) * 4 {
                voice.square_temp[i] = set.squares.get(off).copied().unwrap_or(0);
                off += delta;
            }
            voice.new_waveform = true;
            voice.waveform = 2;
            voice.plant_square = false;
        }
        if voice.waveform == 3 {
            voice.new_waveform = true;
        }
        if voice.ring_new_waveform {
            if voice.ring_waveform > 1 {
                voice.ring_waveform = 1;
            }
            voice.ring_audio_source =
                if voice.ring_waveform == 0 { Wave::Triangle(31) } else { Wave::Sawtooth(31) };
            voice.ring_audio_offset = OFFSETS[voice.wave_length as usize];
        }
        if voice.new_waveform {
            let filter = (voice.filter_pos.clamp(1, 63) - 1) as usize;
            voice.audio_source = match voice.waveform {
                0 => Wave::Triangle(filter),
                1 => Wave::Sawtooth(filter),
                2 => self.square_wave.map_or(Wave::None, Wave::Square),
                _ => Wave::Noise(filter),
            };
            let mut offset = 0;
            if voice.waveform < 2 {
                offset = OFFSETS[voice.wave_length as usize];
            }
            if voice.waveform == 3 {
                offset = ((voice.wn_random & (2 * 0x280 - 1)) & !1) as usize;
                voice.wn_random = voice.wn_random.wrapping_add(2239384);
                voice.wn_random = (((voice.wn_random >> 8) | (voice.wn_random << 24))
                    .wrapping_add(782323)
                    ^ 75)
                    .wrapping_sub(6735);
            }
            voice.audio_offset = offset;
        }
        let transpose = if voice.override_transpose != 1000 { voice.override_transpose } else { voice.transpose };
        if voice.ring_audio_source != Wave::None {
            let mut p = voice.ring_base_period;
            if !voice.ring_fixed_period {
                p += transpose + voice.track_period - 1;
            }
            p = p.clamp(0, 5 * 12);
            p = PERIODS[p as usize];
            if !voice.ring_fixed_period {
                p = wrap16(p + voice.period_slide_period);
            }
            p = wrap16(p + voice.period_perf_slide_period + voice.vibrato_period);
            voice.ring_audio_period = p.clamp(0x71, 0xd60);
        }
        let mut p = voice.instr_period;
        if !voice.fixed_note {
            p += transpose + voice.track_period - 1;
        }
        p = p.clamp(0, 5 * 12);
        p = PERIODS[p as usize];
        if !voice.fixed_note {
            p = wrap16(p + voice.period_slide_period);
        }
        p = wrap16(p + voice.period_perf_slide_period + voice.vibrato_period);
        voice.audio_period = p.clamp(0x71, 0xd60);
        voice.audio_volume = ((((((voice.adsr_volume >> 8) * voice.note_max_volume) >> 6) * voice.perf_sub_volume) >> 6)
            * voice.track_master_volume)
            >> 6;
    }

    fn pl_command(&mut self, v: usize, fx: i32, mut param: i32) {
        let (fx, param0) = (fx & 0xff, param & 0xff);
        param = param0;
        let voice = &mut self.voices[v];
        match fx {
            0 => {
                if param > 0 && param < 0x40 {
                    if voice.ignore_filter != 0 {
                        voice.filter_pos = voice.ignore_filter;
                        voice.ignore_filter = 0;
                    } else {
                        voice.filter_pos = param;
                    }
                    voice.new_waveform = true;
                }
            }
            1 => {
                voice.period_perf_slide_speed = param;
                voice.period_perf_slide_on = true;
            }
            2 => {
                voice.period_perf_slide_speed = -param;
                voice.period_perf_slide_on = true;
            }
            3 => {
                if !voice.ignore_square {
                    voice.square_pos = param >> (5 - voice.wave_length);
                } else {
                    voice.ignore_square = false;
                }
            }
            4 => {
                if param == 0 {
                    voice.square_on = !voice.square_on;
                    voice.square_init = voice.square_on;
                    voice.square_sign = 1;
                } else {
                    if param & 0x0f != 0 {
                        voice.square_on = !voice.square_on;
                        voice.square_init = voice.square_on;
                        voice.square_sign = if param & 0x0f == 0x0f { -1 } else { 1 };
                    }
                    if param & 0xf0 != 0 {
                        voice.filter_on = !voice.filter_on;
                        voice.filter_init = voice.filter_on;
                        voice.filter_sign = if param & 0xf0 == 0xf0 { -1 } else { 1 };
                    }
                }
            }
            5 => voice.perf_current = param as usize,
            7 | 8 => {
                if param >= 1 && param <= 0x3c {
                    voice.ring_base_period = param;
                    voice.ring_fixed_period = true;
                } else if param >= 0x81 && param <= 0xbc {
                    voice.ring_base_period = param - 0x80;
                    voice.ring_fixed_period = false;
                } else {
                    voice.ring_base_period = 0;
                    voice.ring_fixed_period = false;
                    voice.ring_new_waveform = false;
                    voice.ring_audio_source = Wave::None;
                    voice.ring_audio_offset = 0;
                    voice.ring_mix = false;
                    return;
                }
                voice.ring_waveform = if fx == 7 { 0 } else { 1 };
                voice.ring_new_waveform = true;
                voice.ring_plant_period = true;
            }
            9 => {
                if param > 127 {
                    param -= 256;
                }
                voice.pan = param + 128;
            }
            12 => {
                if param <= 0x40 {
                    voice.note_max_volume = param;
                    return;
                }
                param -= 0x50;
                if param < 0 {
                    return;
                }
                if param <= 0x40 {
                    voice.perf_sub_volume = param;
                    return;
                }
                param -= 0xa0 - 0x50;
                if param < 0 {
                    return;
                }
                if param <= 0x40 {
                    voice.track_master_volume = param;
                }
            }
            15 => {
                voice.perf_speed = param;
                voice.perf_wait = param;
            }
            _ => {}
        }
    }

    fn set_audio(&mut self, v: usize) {
        let song = self.song.clone();
        if !self.voices[v].track_on {
            self.voices[v].voice_volume = 0;
            return;
        }
        let wl = self.voices[v].wave_length;
        let voice = &self.voices[v];
        let mut buf = voice.voice_buffer;
        let mut ring = voice.ring_buffer;
        let mut ring_mix = voice.ring_mix;
        let loops = (1usize << (5 - wl)) * 5;
        let loop_len = 4usize << wl;
        let fill = |this: &Core, dst: &mut [i8; BUF + 1], src: Wave, off: usize, noise: bool| {
            if noise {
                for i in 0..BUF {
                    dst[i] = this.wave(src, off + i);
                }
            } else {
                for l in 0..loops {
                    for i in 0..loop_len {
                        dst[l * loop_len + i] = this.wave(src, off + i);
                    }
                }
            }
            dst[BUF] = dst[0];
        };
        let new_waveform = voice.new_waveform;
        if new_waveform {
            fill(self, &mut buf, voice.audio_source, voice.audio_offset, voice.waveform == 3);
        }
        if voice.ring_new_waveform {
            fill(self, &mut ring, voice.ring_audio_source, voice.ring_audio_offset, false);
            ring_mix = true;
        }
        let voice = &mut self.voices[v];
        voice.voice_volume = voice.audio_volume;
        if voice.plant_period {
            voice.plant_period = false;
            voice.voice_period = voice.audio_period;
        }
        voice.ring_plant_period = false;
        voice.voice_buffer = buf;
        voice.ring_buffer = ring;
        voice.ring_mix = ring_mix;
        let _ = song;
    }
}

// -------------------------------------------------------------- the host

pub struct Hvl {
    core: Core,
    title: String,
    channels: usize,
    subsongs: u16,
    sub: u16,
    /// The buffers (a channel's 0x280 bytes) in chip memory.
    chip: u32,
    first_tick: bool,
    last_period: Vec<i32>,
    last_vol: Vec<[i32; 2]>,
    last_buf: Vec<Vec<i8>>,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let song = load(d)?;
    let channels = song.channels;
    let chip = m.alloc((BUF * channels) as u32);
    // the AudioStructs' sides: the channel's left voice, then its right
    let structs = m.alloc(2 * channels as u32 * crate::amplifier::aus::SIZE);
    if chip == 0 || structs == 0 {
        return None;
    }
    for v in 0..2 * channels as u32 {
        m.bus.w16(structs + v * crate::amplifier::aus::SIZE + crate::amplifier::aus::PAN, if v % 2 == 0 { 1 } else { 0xFFFF });
    }
    use crate::eagle::epamb;
    if !crate::amplifier::start(m, 2 * channels, structs, epamb::DIRECT | epamb::BIT8 | epamb::CHIP_RAM) {
        return None;
    }
    let title = song.name.clone();
    let subsongs = song.subsongs.len() as u16;
    let core = Core::new(Arc::new(song), Arc::new(Waves::new()));
    Some(Box::new(Hvl {
        core,
        title,
        channels,
        subsongs,
        sub: 0,
        chip,
        first_tick: true,
        last_period: vec![0; channels],
        last_vol: vec![[0; 2]; channels],
        last_buf: vec![Vec::new(); channels],
    }))
}

/// A C `int16` after an addition.
fn wrap16(x: i32) -> i32 {
    x as i16 as i32
}

fn mask(n: usize) -> u32 {
    if n >= 32 {
        u32::MAX
    } else {
        (1 << n) - 1
    }
}

impl Hvl {
    /// A voice's waveform as the mixer plays it.
    fn buffer(core: &Core, c: usize) -> Vec<i8> {
        let v = &core.voices[c];
        if !v.ring_mix || v.ring_audio_source == Wave::None {
            return v.voice_buffer[..BUF].to_vec();
        }
        // the modulating waveform at the ratio of the two periods
        let (a, r) = (v.voice_period.max(1) as i64, v.ring_audio_period.max(1) as i64);
        (0..BUF).map(|i| ((v.voice_buffer[i] as i32 * v.ring_buffer[(i as i64 * a / r) as usize % BUF] as i32) >> 7) as i8).collect()
    }
}

impl Replayer for Hvl {
    fn format(&self) -> &'static str {
        "HivelyTracker"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        super::DT_TICK / self.core.song.speed_multiplier as u64
    }

    fn songend(&self) -> bool {
        self.core.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsongs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if self.core.init_subsong(n as usize) {
            self.sub = n;
        }
        let _ = hw;
        self.first_tick = true;
    }

    fn tick(&mut self, hw: &mut Hw) {
        let n = self.channels;
        if self.first_tick {
            self.first_tick = false;
            hw.voice_dma(mask(2 * n), false);
            for c in 0..n {
                for s in 0..2 {
                    let v = 2 * c + s;
                    hw.voice_loc(v, self.chip + (BUF * c) as u32);
                    hw.voice_len(v, (BUF / 2) as u16);
                    hw.voice_per(v, 0x71);
                    hw.voice_vol(v, 0);
                }
                self.last_period[c] = 0x71;
                self.last_vol[c] = [0; 2];
                self.last_buf[c].clear();
            }
            hw.voice_dma(mask(2 * n), true);
        }
        self.core.play_irq();
        for c in 0..n {
            let buf = Self::buffer(&self.core, c);
            if buf != self.last_buf[c] {
                let bytes: Vec<u8> = buf.iter().map(|&b| b as u8).collect();
                hw.bus.write_bytes(self.chip + (BUF * c) as u32, &bytes);
                self.last_buf[c] = buf;
            }
            let v = &self.core.voices[c];
            if v.voice_period != self.last_period[c] && v.voice_period != 0 {
                self.last_period[c] = v.voice_period;
                hw.voice_per(2 * c, v.voice_period as u16);
                hw.voice_per(2 * c + 1, v.voice_period as u16);
            }
            let pan = v.pan.clamp(0, 255) as usize;
            let vol = v.voice_volume.clamp(0, 64);
            let vols = [
                (vol * self.core.waves.pan_left[pan] + 127) / 255,
                (vol * self.core.waves.pan_right[pan] + 127) / 255,
            ];
            for s in 0..2 {
                if vols[s] != self.last_vol[c][s] {
                    self.last_vol[c][s] = vols[s];
                    hw.voice_vol(2 * c + s, vols[s] as u16);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A 4-channel module: one position, one track of 4 rows (row 0: note 25
    /// with instrument 1), and an instrument whose play
    /// list starts a sawtooth of wavelength 128.
    pub fn module(channels: u8) -> Vec<u8> {
        let mut d = b"HVL\x01".to_vec();
        d.extend([0, 0]); // the name's offset, patched below
        d.extend([0x00, 0x01]); // speed multiplier 1, 1 position
        d.extend([(channels - 4) << 2, 0]); // channels, restart 0
        d.extend([4, 0, 1, 0]); // 4 rows a track, track 0 the highest, 1 instrument, no subsongs
        d.extend([100, 2]); // mix gain 100%, stereo mode 2
        for _ in 0..channels {
            d.extend([0, 0]); // position 0: track 0, transpose 0
        }
        // row 0: note 25, instrument 1; rows 1 to 3 empty
        d.extend([25, 1, 0, 0, 0]);
        d.extend([0x3f; 3]);
        // volume 64, wavelength 5; attack 1 frame to 64, decay 1 to 64, sustain 10, release 1 to 0
        d.extend([64, 5, 1, 64, 1, 64, 10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0x20, 0x3F, 1, 0x1F, 1, 1]);
        d.extend([0x00, 0x02, 0x00, 0x00, 0x00]); // play list: waveform 2 (sawtooth)
        let name = d.len();
        d[4] = (name >> 8) as u8;
        d[5] = name as u8;
        d.extend(b"test\0inst\0");
        d
    }

    #[test]
    fn subsongs_start_at_their_positions() {
        let mut d = module(4);
        d[7] = 2; // 2 positions
        d[13] = 1; // 1 subsong, from position 1
        d.splice(16..16, [0, 1]);
        d.splice(26..26, [0; 8]); // position 1
        let name = (((d[4] as usize) << 8) | d[5] as usize) + 10;
        (d[4], d[5]) = ((name >> 8) as u8, name as u8);
        let song = Arc::new(load(&d).unwrap());
        assert_eq!((song.positions, song.subsongs.clone(), song.name.as_str()), (2, vec![1], "test"));
        let mut c = Core::new(song, Arc::new(Waves::new()));
        assert!(c.init_subsong(1));
        assert_eq!(c.pos_nr, 1);
        assert!(!c.init_subsong(2));
        assert!(c.init_subsong(0));
        assert_eq!(c.pos_nr, 0);
    }

    /// Damaged modules load or are refused, and play without a panic.
    #[test]
    fn survives_damage() {
        let waves = Arc::new(Waves::new());
        let mut seed = 12345u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as usize
        };
        for _ in 0..100 {
            let mut d = module(4 + (next() % 12) as u8);
            for _ in 0..1 + next() % 6 {
                let at = 6 + next() % (d.len() - 6);
                d[at] = next() as u8;
            }
            if let Some(song) = load(&d) {
                let mut c = Core::new(Arc::new(song), waves.clone());
                for _ in 0..300 {
                    c.play_irq();
                }
            }
        }
    }

    #[test]
    fn detects_only_hvl() {
        assert!(detect(&module(4)));
        assert!(!detect(b"THX\x01\0\0\0\0\0\0\0\0\0\0\0\0"));
        assert!(!detect(&module(4)[..8]));
    }

    #[test]
    fn plays_a_sawtooth_on_the_amplifier() {
        for channels in [4u8, 6, 16] {
            let mut e = Engine::new(48000.0);
            e.open_replayers = true;
            e.play_data(Path::new("test.hvl"), module(channels)).unwrap();
            let p = e.playing.as_ref().unwrap();
            assert_eq!(p.format, "HivelyTracker");
            assert_eq!(p.module_name, "test");
            let mut out = vec![0f32; 2 * 9600];
            e.render(&mut out);
            assert!(out.iter().any(|&s| s != 0.0), "no sound");
        }
    }

    #[test]
    fn core_steps_the_voice() {
        let song = Arc::new(load(&module(5)).unwrap());
        assert_eq!(song.channels, 5);
        let mut c = Core::new(song, Arc::new(Waves::new()));
        c.play_irq();
        let v = &c.voices[0];
        // note 25 (the table's 24th, as the play list's note is 0), the envelope's first step, a sawtooth of 128 bytes
        assert_eq!(v.voice_period, PERIODS[24]);
        assert_eq!(v.voice_volume, 64);
        assert_eq!(&v.voice_buffer[..4], &[-128, -126, -124, -122]);
        assert_eq!(v.voice_buffer[128], -128);
        assert_eq!(v.voice_buffer[BUF], v.voice_buffer[0]);
        // the stereo mode 2 pans: voice 0 left of centre, voice 1 right
        assert_eq!((c.voices[0].pan, c.voices[1].pan), (64, 193));
    }

    /// Writes the waveforms (HVL_OUT) in the reference's layout: each of the
    /// 63 filter settings' triangles, sawtooths, squares and noise.
    #[test]
    #[ignore]
    fn dump_waves() {
        let w = Waves::new();
        let mut o = Vec::new();
        for s in &w.sets {
            o.extend(s.triangles.iter().chain(&s.sawtooths).chain(&s.squares).chain(&s.noise).map(|&b| b as u8));
        }
        std::fs::write(std::env::var("HVL_OUT").unwrap(), o).unwrap();
    }

    /// Writes the per-tick state of the voices (volume, period, pan, a hash of
    /// the waveform loop) of the module in HVL_FILE, subsong HVL_SUB, for
    /// HVL_TICKS ticks, to HVL_OUT, in the format of the reference's dump
    /// harness. Run by hand: `cargo test --lib hvl::tests::dump -- --ignored`.
    #[test]
    #[ignore]
    fn dump() {
        use std::fmt::Write;
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let song = Arc::new(load(&std::fs::read(env("HVL_FILE")).unwrap()).unwrap());
        let (sub, ticks) = (env("HVL_SUB").parse().unwrap_or(0), env("HVL_TICKS").parse().unwrap_or(1000));
        let fnv = |b: &[i8]| b[..BUF].iter().fold(2166136261u32, |h, &x| (h ^ x as u8 as u32).wrapping_mul(16777619));
        let mut c = Core::new(song.clone(), Arc::new(Waves::new()));
        let mut o = String::new();
        if !c.init_subsong(sub) {
            o.push_str("nosub\n");
        } else {
            writeln!(o, "ch {} sub {} mult {}", song.channels, song.subsongs.len(), song.speed_multiplier).unwrap();
            for t in 0..ticks {
                c.play_irq();
                write!(o, "{t}{}", if c.songend { " E" } else { "" }).unwrap();
                for v in &c.voices {
                    write!(o, " |{},{},{},{:08x}", v.voice_volume as u8, v.voice_period as i16, v.pan, fnv(&v.voice_buffer))
                        .unwrap();
                    if v.ring_mix {
                        write!(o, ",{:08x}", fnv(&v.ring_buffer)).unwrap();
                    }
                }
                o.push('\n');
                if c.songend {
                    break;
                }
            }
        }
        std::fs::write(env("HVL_OUT"), o).unwrap();
    }
}
