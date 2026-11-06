//! MaxTrax (`MXTX.*`, `mxtx.*`), the MIDI-like music driver of Joe Pearce and David "Talin" Joiner
//! (used by Dune II, Darkseed, A-Train, Might and Magic III, Legend of Kyrandia, SimEarth and
//! others), playing on Paula through the audio.device's write queue as the Eagleplayer does.
//!
//! This is a translation of the MaxTrax music driver, "MaxTrax Music Player - audio device
//! handler" by Talin (David Joiner) and Joe Pearce, Copyright 1991, which the Wanted Team's
//! Eagleplayer source (SRC_MaxTrax, `MaxTrax_v2.asm`) embeds under the GNU Lesser General Public
//! License, version 2.1 or (at your option) any later version. It is combined here into a work
//! under the GNU General Public License, version 2 or later; `LICENSE-MaxTrax` carries the LGPL
//! text and the notice, and has to travel with this file. The file loader, the sequencer
//! (`MusicServer`), the note and voice handling (`NoteOn`, `NoteOff`, `pick_voice`, `CalcNote`,
//! `CalcVolume`, the envelope manager), the MIDI controllers, the logarithmic period arithmetic
//! (`IntAlg` and its table) and the feature set the Eagleplayer builds (no modulation,
//! no microtonal table, no per-channel full volume) follow that source, down to the widths of its
//! 68000 arithmetic. Only that driver is used: the Eagleplayer's own glue around it (the
//! DeliTracker interface, its sample list, its volume and balance handling) is not, and the
//! parts of the driver that serve other programs (PlayNote, PlaySound, the extra server,
//! the file reading through DOS) are left out.
//!
//! The driver does not poke Paula: it queues CMD_WRITE requests on the audio.device (an attack
//! write that plays once, a sustain write that loops) and changes period and volume with
//! ADCMD_PERVOL every frame. `Dev` below is that device as far as the driver uses it: the
//! channel's registers take the next queued write when the running one starts its pass, which Paula
//! announces with the channel's audio interrupt (`audio_irq_ch`); the period and volume of a
//! write that asked for them are set when its first pass starts. The moments of the writes
//! within a tick follow the reference's register-write logs: the time the driver takes depends on
//! the path of the code and a few operands (see the T_ constants), fitted to five modules, so that
//! the voice writes of a frame fall on the reference's colour clocks and nine in ten of all writes
//! within 300 of them; the first tick falls where the reference's does (its set-up time grows with
//! the module's size) and the ticks come every 71 051 colour clocks. Every score loops,
//! as the Eagleplayer's does, though the 2007 source plays one of fewer than 16 events once.
//! Whether Paula takes a voice's flush and rewrite within one word-time as a restart depends on the
//! phase of the old sample's word, which follows the exact moments of the writes; where they are
//! off by tens of colour clocks, a stolen voice's sustain can start a tick or more earlier or
//! later than the original's.
//!
//! Format (all words and longs big-endian): `MXTX`, tempo, flags (bit 0: audio filter, bit 1:
//! note velocities count, bit 15: a 256-byte microtonal table follows), the number of scores,
//! each with its event count and 6-byte events (command, data, start time, stop time), then the
//! number of samples and for each one a 20-byte header, its attack and release envelopes (4 bytes
//! per segment) and its octaves' waveforms (each octave twice as long as the one before). A file
//! with no samples takes them from `SMPL.<name>`, `SMPL.set` or `MXTX.main` beside it.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;
use std::collections::VecDeque;

pub const OPEN: Open = Open { name: "MaxTrax", detect, new };

const NUM_VOICES: usize = 4;
const NUM_CHANNELS: usize = 16;
const NUM_PATCHES: usize = 64;
const NUM_REPEATS: usize = 4;

const K_VALUE: i32 = 0x09fd77;
const PREF_PERIOD: i32 = 0x08fd77;
const PERIOD_LIMIT: i32 = 0x06f73d;
const NO_BEND: u16 = 64 << 7;
const MAX_BEND_RANGE: u8 = 24;
/// The driver's frame rate (the Eagleplayer fixes it at 50), and the frame in 24.8 milliseconds.
const FREQUENCY: u16 = 50;
const FRAME_UNIT: u32 = (1000 << 8) / FREQUENCY as u32;

const VOICE_STOLEN: u8 = 1;
const VOICE_PORTAMENTO: u8 = 1 << 1;
const VOICE_DAMPER: u8 = 1 << 2;
const VOICE_RECALC: u8 = 1 << 4;

const ENV_FREE: u8 = 0;
const ENV_HALT: u8 = 1;
const ENV_DECAY: u8 = 2;
const ENV_RELEASE: u8 = 3;
const ENV_SUSTAIN: u8 = 4;
const ENV_ATTACK: u8 = 5;
const ENV_START: u8 = 6;

const CHAN_PAN: u8 = 1;
const CHAN_MONO: u8 = 1 << 1;
const CHAN_PORTAMENTO: u8 = 1 << 2;
const CHAN_DAMPER: u8 = 1 << 3;
const CHAN_ALTERED: u8 = 1 << 7;

const MUSIC_PLAYING: u8 = 1;
const MUSIC_ADDED_NOTE: u8 = 1 << 1;
const MUSIC_VELOCITY: u8 = 1 << 3;
const MUSIC_LOOP: u8 = 1 << 4;

const COMMAND_TEMPO: u8 = 0x80;
const COMMAND_SPECIAL: u8 = 0xa0;
const COMMAND_CONTROL: u8 = 0xb0;
const COMMAND_PROGRAM: u8 = 0xc0;
const COMMAND_BEND: u8 = 0xe0;
const COMMAND_END: u8 = 0xff;

const SPECIAL_MARK: u8 = 0;
const SPECIAL_BEGINREP: u8 = 2;
const SPECIAL_ENDREP: u8 = 3;

const MUSIC_PRI_SCORE: u16 = 0;
const MUST_HAVE_SIDE: u16 = 8;

/// Events are 6 bytes.
const EV: usize = 6;

// The time the driver's code takes, in colour clocks, as the reference's write logs show it (fitted
// to five modules, Darkseed MX06, Contraption Zack's BlueJam and march, and Might and Magic III's
// bank and cyber2: nine in ten writes fall within 300 colour clocks of the reference's, and the
// frame's voice writes within one). The costs depend on the path the code takes and on a few of
// its operands, as the 68000's instruction times do; the ones of paths the five modules do not
// reach are the nearest known.

/// The reference's timer interrupts come every 71 051 colour clocks.
const TICK_CLOCKS: u64 = 71_051;
/// Colour clocks from the start of the reference run to its first tick, to the end of its set-up
/// with an empty module, and per 1000 bytes of module (fitted to reference runs): the set-up's
/// time is not part of the audio, so the first tick falls the difference after the sound's start.
const FIRST_TICK: u64 = 72_000;
const SETUP_BASE: u64 = 28_000;
const SETUP_PER_KB: u64 = 44;

/// A frame's start to the end of its check of the note-offs, with no voice in use ...
const T_FRAME: u64 = 289;
/// ... to which a voice in use adds one, and a voice with a note-off still to come another.
const T_LOOP_VOICE: u64 = 9;
const T_LOOP_PENDING: u64 = 41;
/// A note-off whose time has come.
const T_NOTE_OFF: u64 = 223;
/// A pitch bend event (the other events take no time worth the name).
const T_EV_BEND: u64 = 358;

/// A voice without a channel, and one that holds its volume, in the envelope manager; one that halts.
const T_FREE: u64 = 38;
const T_SUSRET: u64 = 96;
const T_HALT: u64 = 300;
/// One voice's envelope work up to its period and volume write, for the path not known otherwise;
/// a multiplication of the volume calculation, which the code skips for a factor of 128 or more;
/// the note calculation after a change of the channel's pitch.
const T_VOICE: u64 = 467;
const T_MUL: u64 = 37;
const T_EARLY: u64 = 288;
const T_LOW_VOL: u64 = 131;
const T_CALC_NOTE: u64 = 556;
/// From the last voice of a frame to the end of the server, where the audio interrupts are taken.
const T_TICK_END: u64 = 647;

/// A note: from the start of its event to its first write (the audio.device's CMD_WRITE), plus a
/// step for each octave the calculation of its period goes down the patch's samples; for a note
/// that takes a voice over, the time before the voice's DMA stop and the one from there to the new
/// write; with no period calculated (the note is out of range), the time is shorter.
const T_CALC: u64 = 1518;
const T_OCTAVE: u64 = 60;
const T_STEAL_PRE: u64 = 838;
const T_CALC_STOLEN: u64 = 903;
const T_NO_PERIOD: u64 = 158;
/// The driver's DMA wait after the attack's write (nine raster lines' worth of polling in its code,
/// which the emulated machine runs shorter), and what follows the sustain's.
const T_DMA_WAIT: u64 = 575;
const T_NOTE_END: u64 = 17;

const ALOG: [u16; 257] = [
    0, 178, 356, 535, 714, 893, 1073, 1254, 1435, 1617, 1799, 1981, 2164, 2348, 2532, 2716, 2902, 3087, 3273, 3460,
    3647, 3834, 4022, 4211, 4400, 4590, 4780, 4971, 5162, 5353, 5546, 5738, 5932, 6125, 6320, 6514, 6710, 6906, 7102,
    7299, 7496, 7694, 7893, 8092, 8292, 8492, 8693, 8894, 9096, 9298, 9501, 9704, 9908, 10113, 10318, 10524, 10730,
    10937, 11144, 11352, 11560, 11769, 11979, 12189, 12400, 12611, 12823, 13036, 13249, 13462, 13676, 13891, 14106,
    14322, 14539, 14756, 14974, 15192, 15411, 15630, 15850, 16071, 16292, 16514, 16737, 16960, 17183, 17408, 17633,
    17858, 18084, 18311, 18538, 18766, 18995, 19224, 19454, 19684, 19915, 20147, 20379, 20612, 20846, 21080, 21315,
    21550, 21786, 22023, 22260, 22498, 22737, 22977, 23216, 23457, 23698, 23940, 24183, 24426, 24670, 24915, 25160,
    25406, 25652, 25900, 26148, 26396, 26645, 26895, 27146, 27397, 27649, 27902, 28155, 28409, 28664, 28919, 29175,
    29432, 29690, 29948, 30207, 30466, 30727, 30988, 31249, 31512, 31775, 32039, 32303, 32568, 32834, 33101, 33369,
    33637, 33906, 34175, 34446, 34717, 34988, 35261, 35534, 35808, 36083, 36359, 36635, 36912, 37190, 37468, 37747,
    38028, 38308, 38590, 38872, 39155, 39439, 39724, 40009, 40295, 40582, 40870, 41158, 41448, 41738, 42029, 42320,
    42613, 42906, 43200, 43495, 43790, 44087, 44384, 44682, 44981, 45280, 45581, 45882, 46184, 46487, 46791, 47095,
    47401, 47707, 48014, 48322, 48631, 48940, 49251, 49562, 49874, 50187, 50500, 50815, 51131, 51447, 51764, 52082,
    52401, 52721, 53041, 53363, 53685, 54008, 54333, 54658, 54983, 55310, 55638, 55966, 56296, 56626, 56957, 57289,
    57622, 57956, 58291, 58627, 58964, 59301, 59640, 59979, 60319, 60661, 61003, 61346, 61690, 62035, 62381, 62727,
    63075, 63424, 63774, 64124, 64476, 64828, 65182, 0,
];

/// The antilogarithm of a 16.16 logarithm, as the driver computes it (`IntAlg`), to 32 bits.
fn int_alg(d0: u32) -> u32 {
    let idx = ((d0 as u16) >> 8) as usize;
    let mut d1 = ALOG[idx] as u32;
    let d2w = ALOG[idx + 1].wrapping_sub(ALOG[idx]);
    // d1's upper word is d0's: the integer part of the logarithm, which the code shifts by
    d1 |= d0 & 0xFFFF_0000;
    let lo = ((d0 as u16) << 8).wrapping_add(128);
    let d0 = (d0 & 0xFFFF_0000) | lo as u32;
    let prod = d2w as u32 * lo as u32;
    let add = (prod >> 16) as u16;
    let (sum, carry) = (d1 as u16).overflowing_add(add);
    let d1 = (d1 & 0xFFFF_0000) | sum as u32;
    // swap, set bit 0, rotate right by one
    let d1 = d1.rotate_left(16) | 1;
    let d1 = d1.rotate_right(1);
    let n = (d0 >> 16) as u16;
    if n >= 32 {
        return 0;
    }
    let sh = (n as u8 ^ 31) & 63;
    // lsr.l by `sh`: X is the last bit shifted out (unchanged for a shift of 0)
    let (v, x) = if sh == 0 { (d1, carry) } else { (d1 >> sh, d1 >> (sh - 1) & 1 != 0) };
    v.wrapping_add(x as u32)
}

/// DIVS.W: the register after dividing the long `d0` by the word `dv`; unchanged when the
/// quotient does not fit a word (or the divisor is zero, where the 68000 would trap).
fn divs_w(d0: u32, dv: u16) -> u32 {
    let (n, d) = (d0 as i32 as i64, dv as i16 as i64);
    if d == 0 {
        return d0;
    }
    let q = n / d;
    if !(-32768..=32767).contains(&q) {
        return d0;
    }
    let r = n % d;
    ((r as u16 as u32) << 16) | q as u16 as u32
}

/// DIVU.W.
fn divu_w(d0: u32, dv: u16) -> u32 {
    if dv == 0 {
        return d0;
    }
    let q = d0 / dv as u32;
    if q > 0xFFFF {
        return d0;
    }
    ((d0 % dv as u32) << 16) | q
}

/// The driver's long signed division (`divs`: the quotient of the magnitudes, negated when the
/// signs differ).
fn divs_l(d0: u32, d1: u32) -> u32 {
    let (mut a, mut b, mut neg) = (d0 as i32, d1 as i32, false);
    if a < 0 {
        a = a.wrapping_neg();
        neg = true;
    }
    if b < 0 {
        b = b.wrapping_neg();
        neg = !neg;
    }
    if b == 0 {
        return 0;
    }
    let q = (a as u32) / (b as u32);
    if neg {
        q.wrapping_neg()
    } else {
        q
    }
}

#[derive(Clone, Copy)]
struct Sample {
    next: Option<usize>,
    /// Guest address of the waveform.
    wave: u32,
    attack: u32,
    sustain: u32,
}

#[derive(Clone, Copy, Default)]
struct Patch {
    sample: Option<usize>,
    /// Offsets of the envelope segments in the sample file; always set for a loaded patch, even
    /// with a count of 0.
    attack: Option<usize>,
    release: Option<usize>,
    attack_count: u16,
    release_count: u16,
    volume: u16,
    tune: i16,
}

#[derive(Clone, Copy, Default)]
struct Chan {
    patch: usize,
    rpn: u16,
    portamento_time: u16,
    pitch_bend: u16,
    real_bend: u16,
    range: u8,
    volume: u8,
    voices_active: u8,
    flags: u8,
    last_note: u8,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    chan: Option<usize>,
    patch: usize,
    envelope: Option<usize>,
    last_ticks: u32,
    ticks_left: u32,
    porta_ticks: u32,
    incr_volume: i32,
    period_offset: u32,
    envelope_left: u16,
    note_volume: u16,
    base_volume: u16,
    last_period: u16,
    base_note: u8,
    end_note: u8,
    priority: u8,
    status: u8,
    flags: u8,
    last_volume: u8,
}

#[derive(Clone, Copy, Default)]
struct NoteOffEv {
    command: u8,
    data: u8,
    stop_time: i32,
}

/// One audio.device CMD_WRITE.
#[derive(Clone, Copy, PartialEq)]
struct Req {
    id: u32,
    data: u32,
    /// Bytes.
    len: u32,
    /// Passes; 0 repeats forever.
    cycles: u16,
    /// ADIOF_PERVOL: the period and volume are set when the write starts.
    pervol: bool,
    per: u16,
    vol: u16,
}

#[derive(Default)]
struct DevChan {
    passes: u32,
    playing: Option<Req>,
    latched: Option<Req>,
    queue: VecDeque<Req>,
}

pub struct MaxTrax {
    /// The absolute time of the first tick, until the engine has taken it.
    resume: Option<u64>,
    title: String,
    /// The module (the score and, with samples in it, their data), as the driver reads it.
    d: Vec<u8>,
    /// The file the samples are in: `d`, or the companion.
    sd: Vec<u8>,
    scores: Vec<(usize, u32)>,
    samples: Vec<Sample>,
    asample: [Option<usize>; NUM_PATCHES],
    patches: [Patch; NUM_PATCHES],
    chans: [Chan; NUM_CHANNELS],
    voices: [Voice; NUM_VOICES],
    dev: [DevChan; NUM_VOICES],
    next_id: u32,
    // global data
    ticks: u32,
    tick_unit: u32,
    tempo_time: u32,
    tempo_ticks: u32,
    current_time: u32,
    current_score: usize,
    current: usize,
    repeat_point: [usize; NUM_REPEATS],
    repeat_count: [u8; NUM_REPEATS],
    note_off: [NoteOffEv; NUM_VOICES],
    tempo: u16,
    current_tempo: u16,
    start_tempo: u16,
    delta_tempo: u16,
    repeat_total: u8,
    last_voice: usize,
    flags: u8,
    filter: u16,
    right_round: u8,
    left_round: u8,
    songend: bool,
    /// The path the voice took in this turn of the envelope manager, which its time follows.
    cls: String,
    sub: u16,
    intena_set: bool,
}

/// The checks of the Eagleplayer's recognition: the header, the scores, and a sample count of at
/// most 64 after them. Returns the offset of the sample count.
fn scan(d: &[u8]) -> Option<usize> {
    if d.get(..4)? != b"MXTX" {
        return None;
    }
    let tempo = be16(d, 4);
    if tempo == 0 || tempo & 0x8000 != 0 {
        return None;
    }
    let mut o = 8;
    if be16(d, 6) & 0x8000 != 0 {
        o += 256;
    }
    let n = be16(d, o) as usize;
    o += 2;
    if n == 0 || n > 256 {
        return None;
    }
    for _ in 0..n {
        let c = be32(d, o);
        if c == 0 || c & 0x8000_0000 != 0 {
            return None;
        }
        o = o.checked_add(4 + c as usize * EV)?;
        if o >= d.len() {
            return None;
        }
    }
    (o + 2 <= d.len() && be16(d, o) <= 64).then_some(o)
}

fn detect(d: &[u8]) -> bool {
    scan(d).is_some()
}

/// Where the sample file of `file` may be: `SMPL.` for the prefix, `SMPL.set`, `MXTX.main`.
fn companions(file: &str) -> Vec<String> {
    let leaf = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    if leaf.len() > 4 && leaf.is_char_boundary(4) {
        v.push(format!("SMPL{}", &leaf[4..]));
    }
    v.push("SMPL.set".to_string());
    v.push("MXTX.main".to_string());
    v
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let so = scan(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let nscores = be16(d, if be16(d, 6) & 0x8000 != 0 { 8 + 256 } else { 8 }) as usize;
    // the samples: in the module, or in a companion file
    let (sd, so2) = if be16(d, so) != 0 {
        (d.to_vec(), so)
    } else {
        let mut found = None;
        for name in companions(&file) {
            if let Ok(f) = crate::deli::read_companion(m, &name).1 {
                if let Some(o) = sample_offset(&f) {
                    found = Some((f, o));
                    break;
                }
            }
        }
        found?
    };
    let mut mx = MaxTrax {
        title: file.rsplit(['/', ':']).next().unwrap_or(&file).to_string(),
        d: d.to_vec(),
        sd: Vec::new(),
        scores: Vec::new(),
        samples: Vec::new(),
        asample: [None; NUM_PATCHES],
        patches: [Patch::default(); NUM_PATCHES],
        chans: [Chan::default(); NUM_CHANNELS],
        voices: [Voice::default(); NUM_VOICES],
        dev: Default::default(),
        next_id: 1,
        ticks: 0,
        tick_unit: 0,
        tempo_time: 0,
        tempo_ticks: 0,
        current_time: 0,
        current_score: 0,
        current: 0,
        repeat_point: [0; NUM_REPEATS],
        repeat_count: [0; NUM_REPEATS],
        note_off: [NoteOffEv { command: 0xff, data: 0, stop_time: 0 }; NUM_VOICES],
        tempo: 120,
        current_tempo: 0,
        start_tempo: 0,
        delta_tempo: 0,
        repeat_total: 0,
        last_voice: 0,
        flags: 0,
        filter: 1,
        right_round: 0,
        left_round: 0,
        songend: false,
        resume: None,
        cls: String::new(),
        sub: 0,
        intena_set: false,
    };
    mx.load_perf(nscores);
    mx.sd = sd;
    mx.install_samples(m, so2)?;
    mx.open_music();
    let mut hw = Hw { bus: &mut m.bus };
    mx.init_sound(&mut hw, 0);
    let setup = SETUP_BASE + d.len() as u64 * SETUP_PER_KB / 1000;
    mx.resume = Some(hw.bus.clock + FIRST_TICK.saturating_sub(setup));
    Some(Box::new(mx))
}

/// In a sample file: the offset of its sample count (`MXTX` file with any number of scores).
fn sample_offset(f: &[u8]) -> Option<usize> {
    if f.get(..4)? != b"MXTX" {
        return None;
    }
    let tempo = be16(f, 4);
    if tempo == 0 || tempo & 0x8000 != 0 {
        return None;
    }
    let mut o = 8;
    if be16(f, 6) & 0x8000 != 0 {
        o += 256;
    }
    let n = be16(f, o) as usize;
    o += 2;
    if n > 256 {
        return None;
    }
    for _ in 0..n {
        let c = be32(f, o);
        if c == 0 || c & 0x8000_0000 != 0 {
            return None;
        }
        o = o.checked_add(4 + c as usize * EV)?;
    }
    (o + 2 <= f.len() && be16(f, o) <= 64).then_some(o)
}

impl MaxTrax {
    /// `LoadPerf`: tempo, filter, velocity flag and the scores.
    fn load_perf(&mut self, nscores: usize) {
        self.tempo = be16(&self.d, 4);
        let fl = be16(&self.d, 6);
        self.filter = fl & 1;
        self.flags &= !MUSIC_VELOCITY;
        if fl & 2 != 0 {
            self.flags |= MUSIC_VELOCITY;
        }
        let mut o = if fl & 0x8000 != 0 { 8 + 256 } else { 8 };
        o += 2;
        for _ in 0..nscores {
            let c = be32(&self.d, o);
            o += 4;
            self.scores.push((o, c));
            o += c as usize * EV;
        }
    }

    /// `INSTALLSAMPLES`: the patches, their envelopes and the sample chains, reading the sample
    /// file at `o` (the sample count). Each octave's waveform is copied to chip memory of its
    /// own, as the driver allocates it.
    fn install_samples(&mut self, m: &mut Machine, o: usize) -> Option<()> {
        let n = be16(&self.sd, o) as usize;
        let mut a2 = o + 2;
        for _ in 0..n {
            let h = a2;
            let number = be16(&self.sd, h) as usize;
            let tune = be16(&self.sd, h + 2);
            let volume = be16(&self.sd, h + 4);
            let mut octaves = be16(&self.sd, h + 6);
            let mut alen = be32(&self.sd, h + 8);
            let mut slen = be32(&self.sd, h + 12);
            let acount = be16(&self.sd, h + 16);
            let rcount = be16(&self.sd, h + 18);
            a2 += 20;
            let pi = number & (NUM_PATCHES - 1);
            if number < NUM_PATCHES {
                let p = &mut self.patches[pi];
                p.tune = tune as i16;
                p.volume = volume;
                p.attack = Some(a2);
                p.attack_count = acount;
            }
            a2 += acount as usize * 4;
            // the driver gets memory for the envelopes and the sample structures in this order
            if acount != 0 {
                m.alloc(acount as u32 * 4);
            }
            if number < NUM_PATCHES {
                let p = &mut self.patches[pi];
                p.release = Some(a2);
                p.release_count = rcount;
            }
            a2 += rcount as usize * 4;
            if rcount != 0 {
                m.alloc(rcount as u32 * 4);
            }
            let mut prev: Option<usize> = None;
            while octaves != 0 {
                let total = alen.checked_add(slen)?;
                if total > 0x100_0000 || self.samples.len() >= 4096 {
                    return None;
                }
                m.alloc(16);
                let wave = m.alloc(total.max(4));
                if wave == 0 {
                    return None;
                }
                let end = (a2 + total as usize).min(self.sd.len());
                if a2 < end {
                    m.bus.write_bytes(wave, &self.sd[a2..end]);
                }
                let idx = self.samples.len();
                self.samples.push(Sample { next: None, wave, attack: alen, sustain: slen });
                if let Some(p) = prev {
                    self.samples[p].next = Some(idx);
                }
                prev = Some(idx);
                if number < NUM_PATCHES && self.asample[pi].is_none() {
                    self.asample[pi] = Some(idx);
                    self.patches[pi].sample = Some(idx);
                }
                a2 = a2.wrapping_add(total as usize);
                alen = alen.wrapping_add(alen);
                slen = slen.wrapping_add(slen);
                octaves -= 1;
            }
        }
        Some(())
    }

    /// `OpenMusic`: channels, patches and voices.
    fn open_music(&mut self) {
        self.flags &= !MUSIC_PLAYING;
        for i in 0..NUM_CHANNELS {
            self.chans[i].patch = i;
            self.chans[i].rpn = 0;
            self.reset_channel(i);
        }
        for v in self.voices.iter_mut() {
            v.chan = None;
            v.status = ENV_FREE;
        }
    }

    fn reset_channel(&mut self, c: usize) {
        let ch = &mut self.chans[c];
        ch.portamento_time = 500;
        ch.pitch_bend = NO_BEND;
        ch.real_bend = 0;
        ch.range = MAX_BEND_RANGE;
        ch.volume = 128;
        if c & 1 == 0 {
            ch.flags &= !CHAN_PAN;
        } else {
            ch.flags |= CHAN_PAN;
        }
        ch.flags &= !CHAN_PORTAMENTO;
        ch.flags &= !(1 << 4);
        ch.flags |= CHAN_ALTERED;
    }

    // ---- the audio.device ----

    fn audio_dma(hw: &mut Hw, ch: usize, on: bool) {
        hw.dmacon(if on { 0x8000 } else { 0 } | 1 << ch);
    }

    /// Puts a write's sample in the channel's registers, for Paula's next reload.
    fn latch(&mut self, hw: &mut Hw, ch: usize, r: Req) {
        hw.loc(ch, r.data);
        hw.len(ch, (r.len / 2) as u16);
        self.dev[ch].latched = Some(r);
    }

    /// CMD_WRITE.
    fn dev_write(&mut self, hw: &mut Hw, ch: usize, r: Req) {
        let busy = self.dev[ch].playing.is_some() || self.dev[ch].latched.is_some();
        if !busy {
            self.latch(hw, ch, r);
            // starting DMA loads the registers and raises the interrupt
            Self::audio_dma(hw, ch, false);
            Self::audio_dma(hw, ch, true);
        } else if self.dev[ch].latched.is_none() {
            self.latch(hw, ch, r);
        } else {
            self.dev[ch].queue.push_back(r);
        }
    }

    /// ADCMD_PERVOL.
    fn dev_pervol(hw: &mut Hw, ch: usize, per: u16, vol: u16) {
        hw.per(ch, per);
        hw.vol(ch, vol);
    }

    /// CMD_FLUSH (`stop_audio`).
    fn dev_flush(&mut self, hw: &mut Hw, ch: usize) {
        self.dev[ch] = DevChan::default();
        Self::audio_dma(hw, ch, false);
    }

    fn new_req(&mut self, data: u32, len: u32, cycles: u16, pervol: bool, per: u16) -> Req {
        self.next_id += 1;
        Req { id: self.next_id, data, len, cycles, pervol, per, vol: 0 }
    }

    /// Paula reloaded channel `ch`: a pass of the latched write starts, the one before it ended.
    fn service(&mut self, hw: &mut Hw, ch: usize) {
        let bit = 0x80u16 << ch;
        if hw.bus.paula.intreq & bit == 0 || (self.dev[ch].playing.is_none() && self.dev[ch].latched.is_none()) {
            return;
        }
        hw.bus.paula.write_intreq(bit);
        match self.dev[ch].latched {
            Some(l) => {
                if self.dev[ch].playing.map(|p| p.id) != Some(l.id) {
                    self.dev[ch].playing = Some(l);
                    if l.pervol {
                        hw.per(ch, l.per);
                        hw.vol(ch, l.vol);
                    }
                    self.dev[ch].passes = 0;
                }
                self.dev[ch].passes += 1;
                if l.cycles != 0 && self.dev[ch].passes >= l.cycles as u32 {
                    // its last pass: the next write goes in the registers
                    self.dev[ch].latched = None;
                    if let Some(n) = self.dev[ch].queue.pop_front() {
                        self.latch(hw, ch, n);
                    }
                }
            }
            None => {
                // the finished write's sample started over: nothing follows it
                Self::audio_dma(hw, ch, false);
                hw.bus.paula.write_intreq(bit);
                self.dev[ch].playing = None;
            }
        }
    }
}

impl MaxTrax {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// The envelope segment at `o` in the sample file: duration (ms) and volume.
    fn env(&self, o: usize) -> (u16, u16) {
        (be16(&self.sd, o), be16(&self.sd, o + 2))
    }

    /// `CalcVolume`.
    fn calc_volume(&mut self, vi: usize) -> u16 {
        let v = &mut self.voices[vi];
        let mut d0: u32 = 64; // the global volume
        if (v.note_volume as i16) < 128 {
            d0 = (d0 & 0xFFFF) * v.note_volume as u32;
            d0 = (d0 & 0xFFFF_0000) | (d0 as u16 >> 7) as u32;
        }
        let d1 = (v.base_volume >> 8) as u32; // the envelope's top byte
        if d1 < 128 {
            d0 = (d0 & 0xFFFF) * d1;
            d0 = (d0 & 0xFFFF_0000) | (d0 as u16 >> 7) as u32;
        }
        let mut d0 = d0 as u16;
        if (d0 as i16) >= 65 {
            d0 = 64;
        }
        v.last_volume = d0 as u8;
        d0
    }

    /// `CalcNote`: the voice's period (in `last_period`) and the sample to play.
    fn calc_note(&mut self, vi: usize) -> Option<usize> {
        let v = self.voices[vi];
        let chan = self.chans[v.chan.unwrap_or(0)];
        let mut d2 = chan.real_bend as i16 as i32;
        let mut d3: i32 = 0;
        self.voices[vi].last_period = 0;
        if v.flags & VOICE_PORTAMENTO != 0 {
            let d1 = v.end_note.wrapping_sub(v.base_note) as i8 as i32 as u32;
            let d0 = v.porta_ticks.wrapping_mul(d1);
            let d0 = divs_w(d0, chan.portamento_time);
            d3 = d0 as u16 as i16 as i32;
        }
        d2 = d2.wrapping_add(d3);
        d2 = d2.wrapping_add(((v.base_note as u16) << 8) as i32);
        let patch = self.patches[v.patch];
        let t = ((patch.tune as i32) << 8) as u32;
        let d0 = divs_w(t, 24);
        d2 = d2.wrapping_add(d0 as u16 as i16 as i32);
        d2 = d2.wrapping_sub(45 << 8);
        d2 = d2.wrapping_mul(4);
        let d2u = divs_w(d2 as u32, 3);
        d2 = (d2u as u16 as i16 as i32) << 4;
        d2 = d2.wrapping_neg().wrapping_add(K_VALUE);
        let mut a3 = patch.sample;
        if v.flags & VOICE_RECALC == 0 {
            self.voices[vi].period_offset = 0;
            while d2 > PREF_PERIOD {
                let Some(next) = a3.and_then(|s| self.samples[s].next) else { break };
                a3 = Some(next);
                self.voices[vi].period_offset = self.voices[vi].period_offset.wrapping_add(0x10000);
                d2 = d2.wrapping_sub(0x10000);
            }
        } else {
            d2 = d2.wrapping_sub(v.period_offset as i32);
        }
        if d2 >= PERIOD_LIMIT {
            self.voices[vi].last_period = int_alg(d2 as u32) as u16;
        }
        a3
    }

    fn min_status(&self, a: usize, b: usize) -> u8 {
        let (x, y) = (self.voices[a].status as i8, self.voices[b].status as i8);
        if x < y {
            x as u8
        } else {
            y as u8
        }
    }

    /// `pick_voice1`: chooses between two voices on the same side. Returns the voice and whether
    /// it used the left bounce byte.
    fn pick_voice1(&mut self, d0: usize) -> (usize, bool) {
        let left = (d0 as u16).wrapping_sub(1) & 2 != 0;
        let sib = d0 ^ 3;
        let d2 = self.voices[d0].status.wrapping_sub(self.voices[sib].status);
        let round = if left { self.left_round } else { self.right_round };
        let pick = if (d2 as i8) < 0 {
            true
        } else if d2 != 0 {
            false
        } else {
            (d0 as u8 & 2) != round
        };
        (if pick { d0 } else { sib }, left)
    }

    /// `pick_voice`: the voice for a new note on `side` (0 or 1) at priority `pri`.
    fn pick_voice(&mut self, mut d0: usize, pri: u16) -> Option<usize> {
        let d5 = (pri & !MUST_HAVE_SIDE) as u8;
        if pri & MUST_HAVE_SIDE == 0 {
            let d1 = d0 ^ 1;
            let near = self.min_status(d0, d0 ^ 3);
            let far = self.min_status(d1, d1 ^ 3);
            if !((near as i8) <= ENV_RELEASE as i8 || (far as i8) > ENV_RELEASE as i8) {
                d0 = d1;
            }
        }
        let (v, mut left) = self.pick_voice1(d0);
        d0 = v;
        let ok = |s: &Self, v: usize| s.voices[v].flags & (1 << 3) == 0 && (d5 as i8).wrapping_sub(s.voices[v].priority as i8) >= 0;
        let mut found = ok(self, d0);
        if !found {
            d0 ^= 3;
            found = ok(self, d0);
        }
        if !found {
            d0 ^= 1;
            let (v, l) = self.pick_voice1(d0);
            d0 = v;
            left = l;
            found = ok(self, d0);
        }
        if !found {
            d0 ^= 3;
            found = ok(self, d0);
        }
        if !found {
            return None;
        }
        let r = d0 as u8 & 2;
        if left {
            self.left_round = r;
        } else {
            self.right_round = r;
        }
        Some(d0)
    }

    /// `KillVoice`.
    fn kill_voice(&mut self, hw: &mut Hw, vi: usize) {
        if let Some(c) = self.voices[vi].chan {
            self.chans[c].voices_active = self.chans[c].voices_active.wrapping_sub(1);
        }
        let v = &mut self.voices[vi];
        v.chan = None;
        v.status = ENV_FREE;
        v.flags = 0;
        v.priority = 0;
        self.dev_flush(hw, vi);
    }

    /// `NoteOn`.
    fn note_on(&mut self, hw: &mut Hw, c: usize, note: u8, vol: u8, pri: u16) {
        if vol == 0 {
            return;
        }
        if self.patches[self.chans[c].patch].sample.is_none() {
            return;
        }
        let velocity = |s: &Self| if s.flags & MUSIC_VELOCITY != 0 { vol as u16 + 1 } else { 128 };
        let vi;
        if self.chans[c].flags & CHAN_MONO != 0 && self.chans[c].voices_active != 0 {
            let Some(i) = (0..NUM_VOICES).find(|&i| self.voices[i].chan == Some(c)) else { return };
            if self.voices[i].status >= ENV_SUSTAIN && self.chans[c].flags & CHAN_PORTAMENTO != 0 {
                let d1 = velocity(self);
                let v = &mut self.voices[i];
                v.porta_ticks = 0;
                if v.flags & VOICE_PORTAMENTO != 0 {
                    v.base_note = v.end_note;
                } else {
                    v.flags |= VOICE_PORTAMENTO;
                }
                v.end_note = note;
                v.note_volume = d1;
                self.chans[c].last_note = note;
                self.flags |= MUSIC_ADDED_NOTE;
                self.last_voice = i;
                return;
            }
            vi = i;
        } else {
            let side = if self.chans[c].flags & CHAN_PAN != 0 { 1 } else { 0 };
            let Some(i) = self.pick_voice(side, pri) else { return };
            vi = i;
        }
        if self.voices[vi].chan.is_some() {
            hw.wait(T_STEAL_PRE);
            self.kill_voice(hw, vi);
            self.voices[vi].flags = VOICE_STOLEN;
        } else {
            self.voices[vi].flags = 0;
        }
        self.voices[vi].chan = Some(c);
        self.voices[vi].patch = self.chans[c].patch;
        self.voices[vi].base_note = note;
        let sample = self.calc_note(vi);
        self.voices[vi].priority = pri as u8;
        self.voices[vi].status = ENV_START;
        let mut d1 = velocity(self);
        let cv = self.chans[c].volume;
        if cv < 128 {
            d1 = ((d1 as u32 * cv as u32) as u16) >> 7;
        }
        {
            let v = &mut self.voices[vi];
            v.note_volume = d1;
            v.base_volume = 0;
            v.last_ticks = 0;
        }
        let per = if self.voices[vi].last_period == 0 { 1000 } else { self.voices[vi].last_period };
        let Some(s) = sample else {
            self.undo_voice(vi);
            return;
        };
        let smp = self.samples[s];
        let stolen = self.voices[vi].flags & VOICE_STOLEN != 0;
        let calc = if stolen { T_CALC_STOLEN } else { T_CALC };
        let calc = calc + (self.voices[vi].period_offset >> 16) as u64 * T_OCTAVE;
        hw.wait(if self.voices[vi].last_period == 0 { calc - T_NO_PERIOD } else { calc });
        if smp.attack != 0 {
            let r = self.new_req(smp.wave, smp.attack, 1, true, per);
            self.dev_write(hw, vi, r);
        }
        hw.wait(T_DMA_WAIT);
        if smp.sustain != 0 {
            let pervol = smp.attack == 0;
            let r = self.new_req(smp.wave.wrapping_add(smp.attack), smp.sustain, 0, pervol, per);
            self.dev_write(hw, vi, r);
        }
        hw.wait(T_NOTE_END);
        self.chans[c].voices_active = self.chans[c].voices_active.wrapping_add(1);
        let ch = self.chans[c];
        if ch.flags & CHAN_MONO != 0 && ch.flags & CHAN_PORTAMENTO != 0 && (ch.last_note as i8) >= 0 {
            let v = &mut self.voices[vi];
            if ch.last_note != v.base_note {
                v.porta_ticks = 0;
                v.end_note = v.base_note;
                v.base_note = ch.last_note;
                v.flags |= VOICE_PORTAMENTO;
            }
        }
        if self.chans[c].flags & CHAN_PORTAMENTO != 0 {
            self.chans[c].last_note = note;
        }
        self.flags |= MUSIC_ADDED_NOTE;
        self.last_voice = vi;
    }

    fn undo_voice(&mut self, vi: usize) {
        let v = &mut self.voices[vi];
        v.chan = None;
        v.status = ENV_FREE;
        v.flags = 0;
        v.priority = 0;
    }

    /// `NoteOff`.
    fn note_off(&mut self, c: usize, note: u8) {
        if self.chans[c].voices_active == 0 {
            return;
        }
        let vi = self.last_voice;
        let v = &mut self.voices[vi];
        if v.chan != Some(c) || (v.status as i8) <= ENV_RELEASE as i8 {
            return;
        }
        let want = if v.flags & VOICE_PORTAMENTO != 0 { v.end_note } else { v.base_note };
        if note != want {
            return;
        }
        if self.chans[c].flags & CHAN_DAMPER != 0 {
            v.flags |= VOICE_DAMPER;
        } else {
            v.status = ENV_RELEASE;
        }
    }

    fn all_notes_off(&mut self, c: usize) {
        if self.chans[c].voices_active == 0 {
            return;
        }
        let damper = self.chans[c].flags & CHAN_DAMPER != 0;
        for v in self.voices.iter_mut().filter(|v| v.chan == Some(c)) {
            if damper {
                v.flags |= VOICE_DAMPER;
            } else {
                v.status = ENV_RELEASE;
            }
        }
    }

    fn all_sounds_off(&mut self, hw: &mut Hw, c: usize) {
        if self.chans[c].voices_active == 0 {
            return;
        }
        for i in 0..NUM_VOICES {
            if self.voices[i].chan == Some(c) {
                self.kill_voice(hw, i);
            }
        }
    }

    /// `SystemReset`.
    fn system_reset(&mut self, hw: &mut Hw) {
        for i in 0..NUM_VOICES {
            if self.voices[i].chan.is_some() {
                self.kill_voice(hw, i);
            }
        }
        for c in 0..NUM_CHANNELS {
            self.reset_channel(c);
            self.chans[c].flags &= !CHAN_MONO;
            self.chans[c].last_note = 0xff;
        }
    }

    fn set_tempo(&mut self, d0: u16) {
        self.current_tempo = d0;
        let n = (d0 >> 4) as u32 * (192 << 8);
        self.tick_unit = n / (FREQUENCY as u32 * 60);
    }

    /// `SetAudioFilter`: bit 1 of CIA-A's port A is the LED, set for "filter off".
    fn set_audio_filter(hw: &mut Hw, on: bool) {
        let x = hw.bus.read_byte(0xbfe001);
        hw.bus.write_byte(0xbfe001, if on { x & !2 } else { x | 2 });
    }

    /// `ControlCh`; `ev` is the offset of the event being processed, which the driver's
    /// portamento-off code writes into (`chan_LastNote(a2)` with the event pointer in a2).
    fn control_ch(&mut self, hw: &mut Hw, c: usize, ctl: u8, val: u8, ev: usize) {
        match ctl {
            5 => self.chans[c].portamento_time = (val as u16) << 7,
            37 => {
                let ch = &mut self.chans[c];
                ch.portamento_time = (ch.portamento_time & 0x3f80) | val as u16;
            }
            6 => {
                if self.chans[c].rpn != 0 {
                    return;
                }
                let d0 = val.min(MAX_BEND_RANGE);
                self.chans[c].range = d0;
                let d1 = self.chans[c].pitch_bend.wrapping_sub(NO_BEND);
                if d1 == 0 {
                    return;
                }
                let d1 = (d1 as u32 * d0 as u32) << 8;
                let d1 = divu_w(d1, NO_BEND);
                self.chans[c].real_bend = d1 as u16;
                self.chans[c].flags |= CHAN_ALTERED;
            }
            7 => {
                let mut d0 = val;
                if d0 != 0 {
                    d0 = d0.wrapping_add(1);
                }
                self.chans[c].volume = d0;
            }
            10 => {
                let ch = &mut self.chans[c];
                if val < 64 || (val == 64 && c & 1 == 0) {
                    ch.flags |= CHAN_PAN;
                } else {
                    ch.flags &= !CHAN_PAN;
                }
            }
            64 => {
                if val & 0x40 != 0 {
                    self.chans[c].flags |= CHAN_DAMPER;
                } else {
                    self.chans[c].flags &= !CHAN_DAMPER;
                    for v in self.voices.iter_mut().filter(|v| v.chan == Some(c)) {
                        if v.flags & VOICE_DAMPER != 0 {
                            v.flags &= !VOICE_DAMPER;
                            v.status = ENV_RELEASE;
                        }
                    }
                }
            }
            65 => {
                if val & 0x40 != 0 {
                    self.chans[c].flags |= CHAN_PORTAMENTO;
                } else {
                    self.chans[c].flags &= !CHAN_PORTAMENTO;
                    // the driver stores through its event pointer here, not the channel
                    let o = ev + 17;
                    if o < self.d.len() {
                        self.d[o] = 0xff;
                    }
                }
            }
            81 => {
                let on = match val.cmp(&64) {
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => self.filter != 0,
                    std::cmp::Ordering::Greater => true,
                };
                Self::set_audio_filter(hw, on);
            }
            100 => self.chans[c].rpn = (self.chans[c].rpn & 0xff00) | val as u16,
            101 => self.chans[c].rpn = (self.chans[c].rpn & 0x00ff) | (val as u16) << 8,
            121 => self.reset_channel(c),
            123 => self.all_notes_off(c),
            126 => {
                self.chans[c].flags |= CHAN_MONO;
                self.all_notes_off(c);
            }
            127 => {
                self.chans[c].flags &= !CHAN_MONO;
                self.all_notes_off(c);
            }
            120 => self.all_sounds_off(hw, c),
            _ => {}
        }
    }

    /// `PitchBend`.
    fn pitch_bend(&mut self, c: usize, lo: u8, hi: u8) {
        let mut d0 = (hi as u16) << 7;
        d0 = (d0 & 0xFF00) | (d0 as u8).wrapping_add(lo) as u16;
        let ch = &mut self.chans[c];
        ch.pitch_bend = d0;
        let d0 = d0.wrapping_sub(NO_BEND) as i16 as i32;
        let d1 = (ch.range as i32) << 8;
        let r = divs_w((d0 * d1) as u32, NO_BEND);
        ch.real_bend = r as u16;
        ch.flags |= CHAN_ALTERED;
    }

    /// The way `calc_incrvol` goes: no time, the division and then the early return, or the division's
    /// quotient.
    fn incr_class(delta: i32, time: u16) -> &'static str {
        if time == 0 {
            return "~0";
        }
        let d0 = delta.wrapping_abs() as u32;
        if d0 & 0xFFFF == 0 {
            return "~z";
        }
        let n = (d0 & 0xFFFF) * 1000;
        let q = n / (time as u32 * FREQUENCY as u32);
        if (d0 as u16).wrapping_sub(q as u16) & 0x8000 != 0 {
            "~e"
        } else {
            "~n"
        }
    }

    /// `calc_incrvol`.
    fn calc_incrvol(delta: i32, time: u16) -> i32 {
        if time == 0 {
            return delta;
        }
        let d2 = delta;
        let d0 = delta.wrapping_abs() as u32;
        let d3 = d0 as u16;
        let n = (d0 & 0xFFFF) * 1000;
        let t = time as u32 * FREQUENCY as u32;
        let q = n / t;
        if d3.wrapping_sub(q as u16) & 0x8000 != 0 {
            return d2;
        }
        if d2 < 0 {
            (q as i32).wrapping_neg()
        } else {
            q as i32
        }
    }

    /// `DoOneVoice`: one voice's envelope for a frame of `delta`.
    fn do_one_voice(&mut self, hw: &mut Hw, vi: usize, delta: u32) {
        self.cls.clear();
        self.do_one_voice_inner(hw, vi, delta);
    }

    fn do_one_voice_inner(&mut self, hw: &mut Hw, vi: usize, delta: u32) {
        let Some(ci) = self.voices[vi].chan else {
            self.cls.push_str("free");
            hw.wait(T_FREE);
            return;
        };
        let d2 = delta;
        self.voices[vi].last_ticks = self.voices[vi].last_ticks.wrapping_add(d2);
        // what the code does after the envelope work
        enum Next {
            Calc,
            Send(u16),
        }
        let status = self.voices[vi].status;
        let mut next = None;
        if status == ENV_SUSTAIN {
            if self.chans[ci].flags & CHAN_ALTERED != 0 || self.voices[vi].flags & VOICE_PORTAMENTO != 0 {
                self.cls.push_str("sus");
                next = Some(Next::Calc);
            } else {
                self.cls.push_str("susret");
                hw.wait(T_SUSRET);
                return;
            }
        } else if status == ENV_HALT {
            self.cls.push_str("halt");
            hw.wait(T_HALT);
            self.kill_voice(hw, vi);
            return;
        }
        if next.is_none() {
            let patch = self.patches[self.voices[vi].patch];
            let st = self.voices[vi].status;
            let mut go_env = true;
            if st == ENV_START {
                self.cls.push_str("start");
                self.voices[vi].envelope = patch.attack;
                match patch.attack {
                    None => {
                        self.cls.push_str("/noattack");
                        let v = &mut self.voices[vi];
                        v.status = ENV_SUSTAIN;
                        v.base_volume = patch.volume;
                        v.last_ticks = d2;
                        next = Some(Next::Calc);
                        go_env = false;
                    }
                    Some(e) => {
                        let (dur, vol) = self.env(e);
                        let v = &mut self.voices[vi];
                        v.envelope_left = patch.attack_count;
                        v.ticks_left = (dur as u32) << 8;
                        v.status = ENV_ATTACK;
                        v.last_ticks = d2;
                        v.incr_volume = Self::calc_incrvol(vol as i32, dur);
                        self.cls.push_str(Self::incr_class(vol as i32, dur));
                    }
                }
            } else if st == ENV_RELEASE {
                self.cls.push_str("rel");
                self.voices[vi].envelope = patch.release;
                match patch.release {
                    None => {
                        self.cls.push_str("/norel");
                        self.voices[vi].status = ENV_HALT;
                        next = Some(Next::Send(0));
                        go_env = false;
                    }
                    Some(e) => {
                        let (dur, vol) = self.env(e);
                        let v = &mut self.voices[vi];
                        v.envelope_left = patch.release_count;
                        v.ticks_left = (dur as u32) << 8;
                        v.status = ENV_DECAY;
                        v.last_ticks = d2;
                        let d0 = vol as i32 - v.base_volume as i32;
                        v.incr_volume = Self::calc_incrvol(d0, dur);
                        self.cls.push_str(Self::incr_class(d0, dur));
                    }
                }
            }
            if go_env {
                if st != ENV_START && st != ENV_RELEASE {
                    self.cls.push_str(if st == ENV_ATTACK { "att" } else { "dec" });
                }
                if self.voices[vi].status == ENV_SUSTAIN {
                    next = Some(Next::Calc);
                } else if (d2.wrapping_sub(self.voices[vi].ticks_left) as i32) >= 0 {
                    // the segment is over
                    self.cls.push_str("/segend");
                    let e = self.voices[vi].envelope.unwrap_or(0);
                    let (_, vol) = self.env(e);
                    self.voices[vi].base_volume = vol;
                    self.voices[vi].envelope_left = self.voices[vi].envelope_left.wrapping_sub(1);
                    if self.voices[vi].envelope_left == 0 {
                        self.cls.push_str("/last");
                        if self.voices[vi].status == ENV_DECAY {
                            self.voices[vi].status = ENV_HALT;
                            next = Some(Next::Send(0));
                        } else {
                            self.voices[vi].status = ENV_SUSTAIN;
                            self.voices[vi].last_ticks = d2;
                            next = Some(Next::Calc);
                        }
                    } else {
                        let e = e + 4;
                        let (dur, vol) = self.env(e);
                        let v = &mut self.voices[vi];
                        v.envelope = Some(e);
                        v.ticks_left = (dur as u32) << 8;
                        let d0 = vol as i32 - v.base_volume as i32;
                        v.incr_volume = Self::calc_incrvol(d0, dur);
                        self.cls.push_str(Self::incr_class(d0, dur));
                        next = Some(Next::Calc);
                    }
                } else {
                    self.cls.push_str("/prog");
                    let v = &mut self.voices[vi];
                    let d0 = v.base_volume as i32 + v.incr_volume;
                    let d0 = if d0 < 0 {
                        0
                    } else if d0 >= 0x8000 {
                        0x8000
                    } else {
                        d0
                    };
                    v.base_volume = d0 as u16;
                    v.ticks_left = v.ticks_left.wrapping_sub(d2);
                    next = Some(Next::Calc);
                }
            }
        }
        // .18: the volume, then the note when it changes
        let vol = match next {
            Some(Next::Send(v)) => {
                self.cls.push_str("+send");
                v
            }
            _ => {
                let d4 = self.calc_volume(vi);
                if self.voices[vi].flags & VOICE_PORTAMENTO != 0 {
                    self.cls.push_str("+porta");
                    let v = &mut self.voices[vi];
                    v.porta_ticks = v.porta_ticks.wrapping_add(d2);
                    let d0 = (v.porta_ticks >> 8) as u16;
                    if (d0.wrapping_sub(self.chans[ci].portamento_time) as i16) >= 0 {
                        v.flags &= !VOICE_PORTAMENTO;
                        v.base_note = v.end_note;
                    }
                    self.voices[vi].flags |= VOICE_RECALC;
                    self.calc_note(vi);
                } else if self.chans[ci].flags & CHAN_ALTERED != 0 {
                    self.cls.push_str("+alt");
                    self.voices[vi].flags |= VOICE_RECALC;
                    self.calc_note(vi);
                }
                d4
            }
        };
        // .17: ADCMD_PERVOL
        let nv = self.voices[vi].note_volume;
        let bv = self.voices[vi].base_volume >> 8;
        let cost = Self::voice_cost(&self.cls, nv, bv);
        hw.wait(self.voice_adjust(cost, nv, bv));
        let lp = self.voices[vi].last_period;
        if lp != 0 {
            Self::dev_pervol(hw, vi, lp, vol);
        } else {
            Self::dev_pervol(hw, vi, 1000, 0);
        }
    }

    /// The start of an envelope whose first segment `calc_incrvol` returns early for (the segment is
    /// shorter than the change of volume asks for) takes `T_EARLY` more than the path's time in the
    /// reference's runs.
    ///
    /// A decay step at a note volume of 128 or more (one multiplication skipped) whose envelope level
    /// is 4 or 5 (the volume comes to 2) takes `T_LOW_VOL` less: the reference's runs show it in
    /// every such step of one module (Contraption Zack's march) and in none of another volume.
    fn voice_adjust(&self, cost: u64, note_volume: u16, env_top: u16) -> u64 {
        if self.cls.starts_with("start~e") {
            cost + T_EARLY
        } else if self.cls == "dec/prog" && note_volume >= 128 && (env_top == 4 || env_top == 5) {
            cost.saturating_sub(T_LOW_VOL)
        } else {
            cost
        }
    }

    /// The time of one voice's turn in the envelope manager up to its period and volume write, by the
    /// path taken (the name `path` lists the steps) and the two multiplications of the volume
    /// calculation, each skipped when its factor is 128 or more.
    fn voice_cost(path: &str, note_volume: u16, env_top: u16) -> u64 {
        let alt = path.ends_with("+alt") || path.ends_with("+porta");
        let path = path.trim_end_matches("+alt").trim_end_matches("+porta");
        let mut plain = String::new();
        let mut skip = false;
        for ch in path.chars() {
            if ch == '~' {
                skip = true;
            } else if skip && (ch == '/' || ch == '+') {
                skip = false;
            }
            if !skip {
                plain.push(ch);
            }
        }
        let path = plain.as_str();
        let (base, muls) = match path {
            "att/prog" | "dec/prog" => (467, true),
            "att/segend/last" => (480, true),
            "dec/segend" => (875, true),
            "dec/segend/last+send" => (299, false),
            "rel/prog" => (925, true),
            "start/prog" => (900, true),
            "start/segend" => (1010, true),
            "start/segend/last" => (615, true),
            "sus" => (357, true),
            // the path steps not seen in the reference's runs: the nearest known cost
            p if p.ends_with("+send") => (299, false),
            p if p.starts_with("rel") => (925, true),
            p if p.starts_with("start") => (1010, true),
            p if p.starts_with("sus") => (357, true),
            _ => (T_VOICE, true),
        };
        let mut c = base;
        if alt {
            c += T_CALC_NOTE;
        }
        if muls {
            if note_volume >= 128 {
                c -= T_MUL;
            }
            if env_top >= 128 {
                c -= T_MUL;
            }
        }
        c
    }

    /// The time the sequencer takes over one event, besides what its handler does to the hardware.
    fn event_cost(cmd: u8, _stop: u16) -> u64 {
        match cmd {
            COMMAND_BEND => T_EV_BEND,
            _ => 0,
        }
    }

    /// `EnvelopeManager`.
    fn envelope_manager(&mut self, hw: &mut Hw, delta: u32) {
        for vi in 0..NUM_VOICES {
            self.do_one_voice(hw, vi, delta);
        }
        for c in self.chans.iter_mut() {
            c.flags &= !CHAN_ALTERED;
        }
    }

    /// `MusicServer`: one frame.
    fn music_server(&mut self, hw: &mut Hw) {
        hw.wait(T_FRAME);
        self.ticks = self.ticks.wrapping_add(self.tick_unit);
        let d4 = self.ticks >> 8;
        for v in 0..NUM_VOICES {
            self.last_voice = v;
            if self.voices[v].chan.is_none() {
                continue;
            }
            hw.wait(T_LOOP_VOICE);
            let ne = self.note_off[v];
            if (ne.command as i8) < 0 {
                continue;
            }
            hw.wait(T_LOOP_PENDING);
            let st = ne.stop_time.wrapping_sub(self.tick_unit as i32);
            self.note_off[v].stop_time = st;
            if st > 0 {
                continue;
            }
            hw.wait(T_NOTE_OFF);
            self.note_off(ne.data as usize & 15, ne.command);
            self.note_off[v].command = 0xff;
        }
        if self.flags & MUSIC_PLAYING != 0 {
            let mut a2 = self.current;
            let mut guard = 0u32;
            loop {
                guard += 1;
                if a2 + EV > self.d.len() || guard > 100_000 {
                    // past the score: as if it ended
                    a2 = self.end_of_score(a2);
                    break;
                }
                let start = be16(&self.d, a2 + 2) as u32;
                let d0 = start.wrapping_add(self.current_time);
                if (d4.wrapping_sub(d0) as i32) < 0 {
                    break;
                }
                self.current_time = d0;
                let cmd = self.rb(a2);
                let data = self.rb(a2 + 1);
                let stop = be16(&self.d, a2 + 4);
                hw.wait(Self::event_cost(cmd, stop));
                if (cmd as i8) >= 0 {
                    self.flags &= !MUSIC_ADDED_NOTE;
                    let vol = (data >> 1) & 0x78;
                    let c = (data & 15) as usize;
                    self.note_on(hw, c, cmd, vol, MUSIC_PRI_SCORE);
                    if self.flags & MUSIC_ADDED_NOTE != 0 {
                        let lv = self.last_voice;
                        let d0 = (stop as u32).wrapping_add(self.current_time).wrapping_sub(d4);
                        self.note_off[lv] = NoteOffEv { command: cmd, data: data & 15, stop_time: (d0 << 8) as i32 };
                    }
                } else if cmd == COMMAND_TEMPO {
                    if (((self.tick_unit >> 8) as u16).wrapping_sub(stop) as i16) < 0 {
                        // continuous
                        self.start_tempo = self.current_tempo;
                        let d1 = ((data as u16) << 4).wrapping_sub(self.current_tempo);
                        self.delta_tempo = d1;
                        self.tempo_time = (stop as u32) << 8;
                        self.tempo_ticks = 0;
                    } else {
                        self.set_tempo((data as u16) << 4);
                        self.tempo_time = 0;
                    }
                } else if cmd == COMMAND_END {
                    self.songend = true;
                    if self.flags & MUSIC_LOOP != 0 {
                        a2 = self.scores[self.current_score].0;
                        self.ticks = 0;
                        self.current_time = 0;
                    } else {
                        self.flags &= !MUSIC_PLAYING;
                    }
                    break;
                } else if cmd == COMMAND_BEND {
                    self.pitch_bend((data & 15) as usize, stop as u8 & 0x7f, (stop >> 8) as u8 & 0x7f);
                } else if cmd == COMMAND_CONTROL {
                    self.control_ch(hw, (data & 15) as usize, (stop >> 8) as u8, stop as u8, a2);
                } else if cmd == COMMAND_PROGRAM {
                    self.chans[(data & 15) as usize].patch = (stop as u8 as usize) & (NUM_PATCHES - 1);
                } else if cmd == COMMAND_SPECIAL {
                    let (d0, d1) = ((stop >> 8) as u8, stop as u8);
                    if d0 == SPECIAL_BEGINREP {
                        let n = self.repeat_total as usize;
                        if n != NUM_REPEATS {
                            self.repeat_total += 1;
                            self.repeat_count[n] = d1;
                            self.repeat_point[n] = a2 + EV;
                        }
                    } else if d0 == SPECIAL_ENDREP && self.repeat_total != 0 {
                        let n = self.repeat_total as usize - 1;
                        if self.repeat_count[n] != 0 {
                            self.repeat_count[n] -= 1;
                            a2 = self.repeat_point[n];
                            self.ticks = 0;
                            self.current_time = 0;
                            break;
                        }
                        self.repeat_total = n as u8;
                    }
                }
                a2 += EV;
            }
            self.current = a2;
            if self.tempo_time != 0 {
                self.tempo_ticks = self.tempo_ticks.wrapping_add(self.tick_unit);
                if (self.tempo_ticks.wrapping_sub(self.tempo_time) as i32) < 0 {
                    let d0 = (self.delta_tempo as u32).wrapping_mul(self.tempo_ticks);
                    let q = divs_l(d0, self.tempo_time);
                    self.set_tempo((q as u16).wrapping_add(self.start_tempo));
                } else {
                    self.set_tempo(self.start_tempo.wrapping_add(self.delta_tempo));
                    self.tempo_time = 0;
                }
            }
        }
        self.envelope_manager(hw, FRAME_UNIT);
        // the audio interrupts wait for the end of the server
        hw.wait(T_TICK_END);
    }

    /// The score's END position when the event pointer ran off the data.
    fn end_of_score(&self, a2: usize) -> usize {
        let (s, n) = self.scores[self.current_score];
        let end = s + n as usize * EV;
        if a2 >= end {
            end.saturating_sub(EV)
        } else {
            a2
        }
    }

    // ---- starting and stopping ----

    /// `PlaySong`.
    fn play_song(&mut self, hw: &mut Hw, which: u16) {
        self.flags &= !(MUSIC_PLAYING | MUSIC_LOOP);
        self.system_reset(hw);
        self.current = self.scores[self.current_score].0;
        self.advance_song(which);
        for n in self.note_off.iter_mut() {
            n.command = 0xff;
        }
        self.set_tempo(self.tempo << 4);
        Self::set_audio_filter(hw, self.filter != 0);
        self.repeat_total = 0;
        self.current_time = 0;
        self.tempo_time = 0;
        self.ticks = 0;
        self.flags |= MUSIC_PLAYING;
    }

    /// `advance_song`: forward to the `which`th mark.
    fn advance_song(&mut self, which: u16) {
        let (start, n) = self.scores[self.current_score];
        let end = start + n as usize * EV;
        let mut a0 = self.current;
        let mut a1 = a0;
        let mut d0 = which as i16;
        let mut guard = 0;
        loop {
            // dbra
            d0 = d0.wrapping_sub(1);
            if d0 == -1 {
                break;
            }
            loop {
                guard += 1;
                if a1 + EV > end.max(self.d.len()) || guard > 1_000_000 {
                    self.current = a0;
                    return;
                }
                let c = self.rb(a1);
                if c == COMMAND_END {
                    a0 = start;
                    a1 = start;
                    break;
                }
                if c == COMMAND_SPECIAL && self.rb(a1 + 4) == SPECIAL_MARK {
                    a1 += EV;
                    a0 = a1;
                    break;
                }
                a1 += EV;
            }
        }
        self.current = a0;
    }

    /// `StopSong`.
    fn stop_song(&mut self, hw: &mut Hw) {
        self.flags &= !MUSIC_PLAYING;
        for i in 0..NUM_VOICES {
            if self.voices[i].chan.is_some() {
                self.kill_voice(hw, i);
            }
        }
    }

    /// `InitSound`: score `n`, looped. (The 2007 source plays a score of fewer than 16 events once;
    /// the Eagleplayer's runs of such scores (Dune II's, Kyrandia's, the INTRO1A files) loop.)
    fn init_sound(&mut self, hw: &mut Hw, n: u16) {
        self.right_round = 0;
        self.left_round = 0;
        if (n as usize) < self.scores.len() {
            // SelectScore
            self.flags &= !MUSIC_PLAYING;
            self.current_score = n as usize;
            self.system_reset(hw);
        }
        self.play_song(hw, 0);
        // the sequencer's time counts from zero, the first frame included
        self.ticks = 0;
        self.flags |= MUSIC_LOOP;
        self.sub = self.current_score as u16;
    }
}

impl Replayer for MaxTrax {
    fn format(&self) -> &'static str {
        "MaxTrax"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.scores.len() as u16 - 1, self.sub)
    }

    /// The reference's timer interrupts come every 71 051 colour clocks.
    fn tick_clocks(&self) -> u64 {
        TICK_CLOCKS
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.stop_song(hw);
        self.dev = Default::default();
        self.intena_set = false;
        self.songend = false;
        self.init_sound(hw, n);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.intena_set {
            // the audio.device's interrupt handlers: the four channels' interrupts on
            hw.bus.paula.write_intena(0x8000 | 0xF << 7);
            self.intena_set = true;
        }
        self.music_server(hw);
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        self.service(hw, ch);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Backend, Engine, Playing};
    use std::path::Path;

    fn p16(d: &mut Vec<u8>, v: u16) {
        d.extend(v.to_be_bytes());
    }

    fn p32(d: &mut Vec<u8>, v: u32) {
        d.extend(v.to_be_bytes());
    }

    /// One event: command, data, start and stop times.
    fn ev(d: &mut Vec<u8>, cmd: u8, data: u8, start: u16, stop: u16) {
        d.extend([cmd, data]);
        p16(d, start);
        p16(d, stop);
    }

    /// A module of one score (note 60 on channel 0 for 30 ms, the end 40 ms later; with `two`
    /// a second score of the end alone) and one patch: an attack of 16 bytes and a sustain of
    /// 32 (one octave), a 20 ms ramp to full volume as its attack envelope.
    fn module(two: bool) -> Vec<u8> {
        let mut d = b"MXTX".to_vec();
        p16(&mut d, 120);
        p16(&mut d, 0);
        p16(&mut d, if two { 2 } else { 1 });
        p32(&mut d, 2);
        ev(&mut d, 60, 0xF0, 0, 30);
        ev(&mut d, COMMAND_END, 0, 40, 0);
        if two {
            p32(&mut d, 1);
            ev(&mut d, COMMAND_END, 0, 10, 0);
        }
        p16(&mut d, 1); // samples
        p16(&mut d, 0); // patch number
        p16(&mut d, 0); // tune
        p16(&mut d, 64); // volume
        p16(&mut d, 1); // octaves
        p32(&mut d, 16);
        p32(&mut d, 32);
        p16(&mut d, 1); // attack segments
        p16(&mut d, 0); // release segments
        p16(&mut d, 20);
        p16(&mut d, 0x7FFF);
        d.extend((0..48u8).map(|i| i.wrapping_mul(5).wrapping_sub(100)));
        d
    }

    fn play(data: Vec<u8>, secs: usize) -> (Engine, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/mxtx-test/mxtx.test"), data).unwrap();
        let mut out = vec![0f32; 2 * 48000 * secs];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        (e, log)
    }

    #[test]
    fn recognition() {
        let d = module(false);
        assert!(detect(&d));
        assert!(detect(&module(true)));
        assert_eq!(scan(&d), Some(8 + 2 + 4 + 12));
        let mut x = d.clone();
        x[0] = b'Y';
        assert!(!detect(&x));
        let mut x = d.clone();
        x[4] = 0; // tempo 0
        x[5] = 0;
        assert!(!detect(&x));
        let mut x = d.clone();
        x[8] = 0; // no scores
        x[9] = 0;
        assert!(!detect(&x));
        let mut x = d.clone();
        x[13] = 99; // more events than the file has
        assert!(!detect(&x));
        let mut x = d.clone();
        let so = scan(&d).unwrap();
        x[so] = 0; // more than 64 samples
        x[so + 1] = 65;
        assert!(!detect(&x));
        assert!(!detect(&d[..20]));
        assert!(!detect(b"MXTX"));
    }

    #[test]
    fn plays_a_note_and_ends() {
        let (e, log) = play(module(false), 3);
        assert_eq!(e.playing.as_ref().unwrap().format, "MaxTrax");
        assert!(e.songend());
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the voice the note went to (the driver rotates through them)
        let ch = (0..4u32).find(|c| !reg(0xDFF0A4 + 16 * c).is_empty()).expect("a voice played");
        let b = 0xDFF0A0 + 16 * ch;
        // the attack (8 words) goes first, the sustain (16 words) after it
        let len = reg(b + 4);
        assert!(len.starts_with(&[8, 16]), "LEN {len:?}");
        let (hi, lo) = (reg(b), reg(b + 2));
        assert_eq!(hi.len(), lo.len());
        let at = |i: usize| (hi[i] as u32) << 16 | lo[i] as u32;
        assert_eq!(at(1) - at(0), 16, "the sustain follows the attack's 16 bytes");
        // a period in range, and a volume that rises from 0 to the patch's level and falls again
        let per = reg(b + 6);
        assert!(!per.is_empty() && per.iter().all(|&p| (113..0x4000).contains(&p)), "PER {per:?}");
        let vol = reg(b + 8);
        let peak = *vol.iter().max().unwrap();
        assert!((1..=64).contains(&peak), "VOL {vol:?}");
        assert!(vol.windows(2).any(|w| w[0] < w[1]) && vol.windows(2).any(|w| w[0] > w[1]));
        // its DMA on, and off again
        let dma = reg(0xDFF096);
        assert!(dma.contains(&(0x8000 | 1 << ch)) && dma.contains(&(1 << ch)));
        // the same module gives the same writes
        let (_, again) = play(module(false), 3);
        assert_eq!(log, again);
    }

    #[test]
    fn subsongs() {
        let (e, _) = play(module(true), 1);
        let Some(Playing { backend: Backend::Replay(r), .. }) = &e.playing else { panic!("not a replayer") };
        assert_eq!(r.subsongs(), (0, 1, 0));
    }

    #[test]
    fn module_without_samples_needs_a_companion() {
        let mut d = module(false);
        let so = scan(&d).unwrap();
        d.truncate(so);
        p16(&mut d, 0); // no samples of its own
        assert!(detect(&d));
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        assert!(e.play_data(Path::new("/mxtx-test2/mxtx.test"), d.clone()).is_err());
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.os.mem_files.insert(Path::new("/mxtx-test2/SMPL.set").to_path_buf(), module(false));
        e.play_data(Path::new("/mxtx-test2/mxtx.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "MaxTrax");
    }
}
