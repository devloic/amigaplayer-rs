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
//! write that asked for them are set when its first pass starts. The reference's device does not
//! run on the interrupt itself: it looks at the channels' requests at the end of every output
//! sample and after a frame's server, and so does this one (`audio_irq_ch`). The moments of the
//! writes within a tick are the 68000's: the code's cycles along the path taken, as the reference
//! (the Eagleplayer on the engine's 68000, which counts a fixed number of cycles per instruction)
//! spends them, with the half colour clock every host call drops. The first tick falls where the
//! reference's does and the ticks come every 71 051 colour clocks. On 59 modules every register
//! write of the first 60 seconds falls on the reference's colour clock, and on the 19 scores whose
//! samples are in a companion (A-Train, Kyrandia; the reference given it as `MXSM.<name>`) every one
//! of the first 20 seconds. Every score loops,
//! as the Eagleplayer's does, though the 2007 source plays one of fewer than 16 events once; a
//! score without an end event runs on into what the reference has in memory after it
//! (`heap_image`).
//!
//! Format (all words and longs big-endian): `MXTX`, tempo, flags (bit 0: audio filter, bit 1:
//! note velocities count, bit 15: a 256-byte microtonal table follows), the number of scores,
//! each with its event count and 6-byte events (command, data, start time, stop time), then the
//! number of samples and for each one a 20-byte header, its attack and release envelopes (4 bytes
//! per segment) and its octaves' waveforms (each octave twice as long as the one before). A file
//! with no samples takes them from `SMPL.<name>`, `SMPL.set` or `MXTX.main` beside it.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paulaout::{Filter, OutputStage, COLOUR_CLOCKS_PAL};
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

// The time the driver's code takes: the reference counts a fixed number of 68000 cycles for each
// instruction (two cycles to the colour clock), so the code below adds up the cycles of the
// instructions along the path it takes (`cy`), read off the reference's instruction traces, and
// every call of the audio.device (a host call) truncates a half colour clock, which `wh` keeps
// track of. Checked against the reference's write logs of 59 modules. The paths none of them takes
// (mono channels, portamento, most special events, the tempo ramp, controllers 81, 121 and 64) are
// counted from the listing alone, unchecked.

/// The reference's timer interrupts come every 71 051 colour clocks.
const TICK_CLOCKS: u64 = 71_051;
/// Colour clocks from the start of the reference run to its first tick: a constant of the
/// reference's run for nearly every module (module size and start-up time do not move it).
const FIRST_TICK_ABS: u64 = 71_051;

// The reference's song start is the end of the player's start-up. What the reference's trace shows
// of it, in colour clocks: the run starts at 100, the check takes 63, the init call that follows
// it (`InitPlayer`) takes the module-dependent time below, and the four guest calls after that
// take 20, 32, 2554 and 7817 for every module (`SETUP_FIXED` is the sum of all but the init call).
// The init call costs, in 68000 cycles (the reference's cycle table, a whole number per
// instruction; every host call drops an odd half colour clock, which the division by two takes care
// of), a sum of loop passes: the score loop over the scores that
// fit (the driver keeps `MAX_SCORES`; the rest are skipped by a host call each), the sample loop
// over the sample headers, the octave loop over every octave's waveform, the search for the first
// patch with a sample (a pass per patch up to it) and one test of the velocity flag.
const SETUP_FIXED: u64 = 10_586;
const MAX_SCORES: usize = 24;
const SETUP_CYCLES: u64 = 31_828;
const SETUP_PER_SCORE: u64 = 700;
const SETUP_PER_SKIPPED_SCORE: u64 = 506;
const SETUP_PER_SAMPLE: u64 = 772;
const SETUP_PER_OCTAVE: u64 = 686;
const SETUP_PER_PATCH_SEARCH: u64 = 39;
const SETUP_VELOCITY: u64 = 48;
/// Bytes between the module's end (on an 8-byte boundary) and the reference's copy of the first score:
/// what the Eagleplayer allocates before it.
const HEAP_GAP: u32 = 2352;
/// `comdivide` with a divisor of 16 bits (two DIVU.W), in 68000 cycles, and what is left of the cycles
/// between the end of the interrupt server and the device's look at the interrupt requests.
const DIV_FAST: u64 = 41 + 336;
const T_TAIL: u64 = 16;
/// Colour clocks a change of subsong takes in the reference, with no voice playing: the Eagleplayer's
/// EndSound (165 and 32), InitSound (2514) and StartInt (3219) as its trace shows them.
const SUBSONG_CLOCKS: u64 = 165 + 32 + 2514 + 3219;
/// Colour clocks from StartInt's write of the audio filter to its end (273 cycles).
const FILTER_TO_START: u64 = 136;

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
    /// The last `calc_note` ended its walk down the octaves at the end of the sample chain.
    octave_end: bool,
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
    /// The half colour clock the events' times carry over.
    event_half: u64,
    title: String,
    /// The module, and once it is loaded what the driver's sequencer reads (`heap_image`).
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
    /// The 68000 cycles the code has run since the last event (a write or a host call).
    cy: u64,
    sub: u16,
    intena_set: bool,
    /// The Eagleplayer's flag for new settings, which the next frame takes.
    settings_new: bool,
    /// The absolute time of the next frame.
    tick_due: u64,
    /// The output samples' grid as last seen: the end of a sample, and the output stage there.
    grid: Option<(u64, OutputStage)>,
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
/// What a score whose samples are in a companion adds to the reference's set-up, in 68000 cycles:
/// the Eagleplayer builds the companion's name (`MXSM` and the score's name after its four-letter
/// prefix) from the module's whole path, 17 cycles a character of the path and 4 more a character of
/// the name, then loads it and reads its header (`COMPANION_CYCLES`). Measured on EP206's MaxTrax with
/// the companion named as it asks (`MXSM.<name>`, found at the first try): the 17 A-Train scores and
/// two of Kyrandia, and one score under 30 paths of 81 to 97 characters (names of 1 to 10).
const COMPANION_CYCLES: u64 = 32_824;

fn companion_cycles(dir: &str, file: &str) -> u64 {
    let leaf = file.rsplit(['/', ':']).next().unwrap_or(file);
    COMPANION_CYCLES + 17 * (dir.len() + file.len()) as u64 + 4 * leaf.len().saturating_sub(5) as u64
}

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

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let so = scan(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let nscores = be16(d, if be16(d, 6) & 0x8000 != 0 { 8 + 256 } else { 8 }) as usize;
    // the samples: in the module, or in a companion file
    let companion = be16(d, so) == 0;
    let (sd, so2) = if !companion {
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
        event_half: 0,
        cy: 0,
        sub: 0,
        intena_set: false,
        settings_new: false,
        tick_due: 0,
        grid: None,
    };
    mx.load_perf(nscores);
    mx.sd = sd;
    // the Eagleplayer's own memory and the driver's copies of the scores come first in the reference's
    // heap, `HEAP_GAP` bytes after the module: as much taken here keeps the waveforms at the
    // reference's addresses
    let want = (base + d.len() as u32).div_ceil(8) * 8 + HEAP_GAP;
    let mut heap = m.alloc(8) + 8;
    if heap < want {
        m.alloc(want - heap);
        heap = want;
    }
    for &(_, c) in mx.scores.iter().take(MAX_SCORES) {
        if c != 0 {
            m.alloc(c.saturating_mul(EV as u32));
        }
    }
    mx.install_samples(m, so2)?;
    mx.heap_image(heap, so2);
    mx.open_music();
    // The reference's song start (where Paula's grids restart) is the end of the driver's set-up (the
    // first tick is at an absolute clock whatever the set-up took).
    let first_patch = mx.patches.iter().position(|p| p.sample.is_some()).unwrap_or(NUM_PATCHES - 1);
    let (fit, skipped) = (nscores.min(MAX_SCORES) as u64, nscores.saturating_sub(MAX_SCORES) as u64);
    let cycles = SETUP_CYCLES
        + SETUP_PER_SCORE * fit
        + SETUP_PER_SKIPPED_SCORE * skipped
        + SETUP_PER_SAMPLE * be16(&mx.sd, so2) as u64
        + SETUP_PER_OCTAVE * mx.samples.len() as u64
        + SETUP_PER_PATCH_SEARCH * (first_patch as u64 + 1)
        + if mx.flags & MUSIC_VELOCITY != 0 { SETUP_VELOCITY } else { 0 }
        // (the 2 cycles the skipping of scores drops in the two modules that have any)
        - if skipped > 0 { 2 } else { 0 }
        + if companion { companion_cycles(&m.bus.read_cstr(m.deli.dir_buf), &file) } else { 0 };
    let est = SETUP_FIXED + cycles / 2;
    let start = m.bus.clock.max(est.min(FIRST_TICK_ABS - 2_000));
    // the filter is set `FILTER_TO_START` before the start
    m.bus.clock = m.bus.clock.max(start.saturating_sub(FILTER_TO_START));
    let mut hw = Hw { bus: &mut m.bus };
    mx.init_sound(&mut hw, 0);
    mx.resume = Some(FIRST_TICK_ABS);
    m.bus.clock = start;
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

    /// Replaces the module by what the reference's sequencer reads: the driver copies each score to
    /// memory of its own and then gets, for each sample, memory for its attack envelope, its release
    /// envelope and, for each octave, a 16-byte record (the next octave's record, the waveform, the
    /// attack's and the sustain's lengths) and the waveform. The reference's allocations follow each
    /// other on 8-byte boundaries in cleared memory, the first score at `heap`, so a score
    /// with no end event runs on into the next score, the sample data and then zeroes (events that
    /// are notes of no volume). The image matches the reference's memory for the 59 modules it was
    /// compared with (all carry their samples; with a companion file the addresses in the records
    /// are a guess). `o` is the sample count's offset in the sample file.
    fn heap_image(&mut self, heap: u32, o: usize) {
        fn block(img: &mut Vec<u8>, b: &[u8]) -> usize {
            img.resize(img.len().div_ceil(8) * 8, 0);
            img.extend_from_slice(b);
            img.len() - b.len()
        }
        let part = |d: &[u8], o: usize, n: usize| d.get(o..).map_or(&[][..], |t| &t[..n.min(t.len())]).to_vec();
        let h0 = heap as usize;
        let mut img = Vec::new();
        let n = self.scores.len().min(MAX_SCORES);
        for i in 0..n {
            let (so, c) = self.scores[i];
            self.scores[i].0 = block(&mut img, &part(&self.d, so, c as usize * EV));
        }
        let mut a2 = o + 2;
        for _ in 0..be16(&self.sd, o) {
            let mut octaves = be16(&self.sd, a2 + 6);
            let (mut alen, mut slen) = (be32(&self.sd, a2 + 8), be32(&self.sd, a2 + 12));
            let (ac, rc) = (be16(&self.sd, a2 + 16) as usize * 4, be16(&self.sd, a2 + 18) as usize * 4);
            a2 += 20;
            for n in [ac, rc] {
                if n != 0 {
                    block(&mut img, &part(&self.sd, a2, n));
                }
                a2 += n;
            }
            let mut prev = None;
            while octaves != 0 && img.len() < 0x200_0000 {
                let total = alen.wrapping_add(slen) as usize;
                let rec = block(&mut img, &[0; 16]);
                let wave = block(&mut img, &part(&self.sd, a2, total));
                img[rec + 4..rec + 8].copy_from_slice(&((h0 + wave) as u32).to_be_bytes());
                img[rec + 8..rec + 12].copy_from_slice(&alen.to_be_bytes());
                img[rec + 12..rec + 16].copy_from_slice(&slen.to_be_bytes());
                if let Some(p) = prev {
                    img[p..p + 4].copy_from_slice(&((h0 + rec) as u32).to_be_bytes());
                }
                prev = Some(rec);
                a2 = a2.wrapping_add(total);
                alen = alen.wrapping_add(alen);
                slen = slen.wrapping_add(slen);
                octaves -= 1;
            }
        }
        // (the scores the driver has no room for: the reference cannot play them)
        for i in n..self.scores.len() {
            let (so, c) = self.scores[i];
            self.scores[i].0 = block(&mut img, &part(&self.d, so, c as usize * EV));
        }
        self.d = img;
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
            // (the engine's device drops the interrupt request an idle channel left set)
            hw.bus.paula.write_intreq(0x80 << ch);
            Self::audio_dma(hw, ch, true);
        } else if self.dev[ch].latched.is_none() {
            self.latch(hw, ch, r);
        } else {
            self.dev[ch].queue.push_back(r);
        }
        self.event_half = 0;
    }

    /// ADCMD_PERVOL.
    fn dev_pervol(&mut self, hw: &mut Hw, ch: usize, per: u16, vol: u16) {
        hw.per(ch, per);
        hw.vol(ch, vol);
        self.event_half = 0;
    }

    /// CMD_FLUSH (`stop_audio`).
    fn dev_flush(&mut self, hw: &mut Hw, ch: usize) {
        self.dev[ch] = DevChan::default();
        Self::audio_dma(hw, ch, false);
        self.event_half = 0;
    }

    fn new_req(&mut self, data: u32, len: u32, cycles: u16, pervol: bool, per: u16) -> Req {
        self.next_id += 1;
        Req { id: self.next_id, data, len, cycles, pervol, per, vol: 0 }
    }

    /// Whether an output sample ends now. The output stage only tells the time to the end of the
    /// running sample, which right at an end is the length of the next one and can be one colour
    /// clock less than the longest: the grid seen at an earlier end, run forward, decides then.
    fn at_sample_end(&mut self, hw: &Hw) -> bool {
        let now = hw.bus.clock;
        let left = hw.bus.paula.out.until_sample() as u64;
        if let Some((end, g)) = &mut self.grid {
            while *end < now {
                let n = g.until_sample();
                g.step([0; 4], n);
                *end += n as u64;
            }
            if *end == now && g.until_sample() as u64 == left {
                return true;
            }
            if *end == now + left {
                return false;
            }
            // the grid was restarted since
            self.grid = None;
        }
        self.see_grid(hw);
        // a sample that has run for a colour clock or more has less left than this
        left as f32 > COLOUR_CLOCKS_PAL as f32 / hw.bus.paula.out.rate() as f32 - 0.5
    }

    /// Takes the output samples' grid from the output stage: the end of the running sample.
    fn see_grid(&mut self, hw: &Hw) {
        let mut g = hw.bus.paula.out.clone();
        g.set_filter(Filter::None);
        let n = g.until_sample();
        g.step([0; 4], n);
        self.grid = Some((hw.bus.clock + n as u64, g));
    }

    /// The device's look at its four channels' interrupt requests.
    fn service_all(&mut self, hw: &mut Hw) {
        for ch in 0..NUM_VOICES {
            self.service(hw, ch);
        }
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
    /// Lets `h` cycles of the 68000 pass (two to the colour clock), carrying the odd cycle.
    fn wh(&mut self, hw: &mut Hw, h: u64) {
        let t = self.event_half + h + std::mem::take(&mut self.cy);
        self.event_half = t & 1;
        hw.wait(t >> 1);
    }

    /// Lets the cycles run so far pass.
    fn flush(&mut self, hw: &mut Hw) {
        self.wh(hw, 0);
    }

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
        // lea, move.w, move.w, cmpi, bpl, then (a multiplication and a shift unless skipped)
        let mut c: u64 = 8 + 12 + 12 + 8 + 9;
        if (v.note_volume as i16) < 128 {
            c += 54 + 20;
            d0 = (d0 & 0xFFFF) * v.note_volume as u32;
            d0 = (d0 & 0xFFFF_0000) | (d0 as u16 >> 7) as u32;
        }
        let d1 = (v.base_volume >> 8) as u32; // the envelope's top byte
        // moveq, move.b, cmpi, bpl
        c += 4 + 12 + 8 + 9;
        if d1 < 128 {
            c += 54 + 20;
            d0 = (d0 & 0xFFFF) * d1;
            d0 = (d0 & 0xFFFF_0000) | (d0 as u16 >> 7) as u32;
        }
        let mut d0 = d0 as u16;
        // cmpi, bmi, (moveq), move.b, rts
        c += 8 + 9 + 12 + 16;
        if (d0 as i16) >= 65 {
            d0 = 64;
            c += 4;
        }
        v.last_volume = d0 as u8;
        self.cy += c;
        d0
    }

    /// `CalcNote`: the voice's period (in `last_period`) and the sample to play.
    fn calc_note(&mut self, vi: usize) -> Option<usize> {
        let v = self.voices[vi];
        let chan = self.chans[v.chan.unwrap_or(0)];
        let mut d2 = chan.real_bend as i16 as i32;
        let mut d3: i32 = 0;
        self.voices[vi].last_period = 0;
        // the entry (movem, movea, move.w, moveq, clr.w, the test of the portamento flag), and the long
        // run of the common arithmetic up to the test of the recalc flag
        let mut c: u64 = 109 + 533;
        if v.flags & VOICE_PORTAMENTO != 0 {
            // the set-up, the multiplication routine, the DIVS
            c += 244 + 278;
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
            self.voices[vi].octave_end = false;
            // the clr.l of the offset
            c += 24;
            loop {
                // cmpi, ble
                c += 14 + 9;
                if d2 <= PREF_PERIOD {
                    break;
                }
                // move.l, beq
                c += 12 + 9;
                let Some(next) = a3.and_then(|s| self.samples[s].next) else {
                    self.voices[vi].octave_end = true;
                    break;
                };
                // movea, addi, subi, bra
                c += 4 + 32 + 16 + 10;
                a3 = Some(next);
                self.voices[vi].period_offset = self.voices[vi].period_offset.wrapping_add(0x10000);
                d2 = d2.wrapping_sub(0x10000);
            }
        } else {
            // sub.l
            c += 18;
            d2 = d2.wrapping_sub(v.period_offset as i32);
        }
        // cmpi, blt
        c += 14 + 9;
        if d2 >= PERIOD_LIMIT {
            // movea, move.l, bsr, IntAlg (a failing shift ends it early), move.w
            c += 4 + 4 + 18 + 217 + 12;
            c += if ((d2 as u32) >> 16) as u16 >= 32 { 32 } else { 68 };
            self.voices[vi].last_period = int_alg(d2 as u32) as u16;
        }
        // move.l, movem, rts
        c += 4 + 44 + 16;
        self.cy += c;
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
    /// it used the left bounce byte. Costs (68000 cycles): the set-up, the two statuses'
    /// subtraction and the exits.
    fn pick_voice1(&mut self, d0: usize) -> (usize, bool) {
        let left = (d0 as u16).wrapping_sub(1) & 2 != 0;
        let sib = d0 ^ 3;
        let d2 = self.voices[d0].status.wrapping_sub(self.voices[sib].status);
        let round = if left { self.left_round } else { self.right_round };
        // lea, move.w, subq, btst, beq, [addq], the two statuses' fetches and the subtraction, bmi, rts
        self.cy += 8 + 4 + 4 + 10 + 9 + 4 + 58 + 14 + 4 + 8 + 4 + 58 + 14 + 9 + 16;
        if left {
            self.cy += 8;
        }
        let pick = if (d2 as i8) < 0 {
            true
        } else if d2 != 0 {
            // bne, the move of the sibling
            self.cy += 9 + 4;
            false
        } else {
            // bne, move.b, andi, cmp.b, bne and the sibling's move
            self.cy += 9 + 4 + 8 + 8 + 9;
            let p = (d0 as u8 & 2) != round;
            if !p {
                self.cy += 4;
            }
            p
        };
        (if pick { d0 } else { sib }, left)
    }

    /// `pick_voice`: the voice for a new note on `side` (0 or 1) at priority `pri`.
    fn pick_voice(&mut self, mut d0: usize, pri: u16) -> Option<usize> {
        let d5 = (pri & !MUST_HAVE_SIDE) as u8;
        // movem, lea, move.w, the two bclr, bne
        self.cy += 48 + 8 + 4 + 14 + 14 + 9;
        if pri & MUST_HAVE_SIDE == 0 {
            let d1 = d0 ^ 1;
            let near = self.min_status(d0, d0 ^ 3);
            let far = self.min_status(d1, d1 ^ 3);
            // the statuses of the side's two voices: two multiplications each, a compare, a branch
            self.cy += 4 + 8 + 4 + 58 + 14 + 4 + 8 + 58 + 14 + 4 + 9 + 4;
            if !((self.voices[d0].status as i8) < (self.voices[d0 ^ 3].status as i8)) {
                self.cy += 4;
            }
            self.cy += 4 + 58 + 14 + 4 + 8 + 58 + 14 + 4 + 9;
            if !((self.voices[d1].status as i8) < (self.voices[d1 ^ 3].status as i8)) {
                self.cy += 4;
            }
            // cmpi, ble
            self.cy += 8 + 9;
            if !((near as i8) <= ENV_RELEASE as i8) {
                // cmpi, bgt
                self.cy += 8 + 9;
                if !((far as i8) > ENV_RELEASE as i8) {
                    self.cy += 4;
                    d0 = d1;
                }
            }
        }
        // bsr
        self.cy += 18;
        let (v, mut left) = self.pick_voice1(d0);
        d0 = v;
        // the check of a voice: move.l, mulu, lea, btst, bne, and cmp.b, bpl unless the damper bit is set
        let ok = |s: &mut Self, v: usize, first: bool| {
            s.cy += if first { 4 + 58 + 12 + 16 + 9 } else { 8 + 4 + 58 + 12 + 16 + 9 };
            let free = s.voices[v].flags & (1 << 3) == 0;
            if free {
                s.cy += 12 + 9;
            }
            free && (d5 as i8).wrapping_sub(s.voices[v].priority as i8) >= 0
        };
        let mut found = ok(self, d0, true);
        if !found {
            d0 ^= 3;
            // (eori is part of the check's cost)
            found = ok(self, d0, false);
        }
        if !found {
            d0 ^= 1;
            // eori, bsr
            self.cy += 8 + 18;
            let (v, l) = self.pick_voice1(d0);
            d0 = v;
            left = l;
            found = ok(self, d0, true);
        }
        if !found {
            d0 ^= 3;
            found = ok(self, d0, false);
        }
        if !found {
            // moveq, bra, movem, rts
            self.cy += 4 + 10 + 52 + 16;
            return None;
        }
        // andi, move.b, move.l, movem, rts
        self.cy += 8 + 8 + 4 + 52 + 16;
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
        self.kill_voice_cost(hw, vi, false);
    }

    /// `KillVoice`, with its cycles when `charge` (the envelope manager's halt path).
    fn kill_voice_cost(&mut self, hw: &mut Hw, vi: usize, charge: bool) {
        if let Some(c) = self.voices[vi].chan {
            self.chans[c].voices_active = self.chans[c].voices_active.wrapping_sub(1);
        }
        let v = &mut self.voices[vi];
        v.chan = None;
        v.status = ENV_FREE;
        v.flags = 0;
        v.priority = 0;
        // the rest of KillVoice and the call of stop_audio up to its host call
        if charge {
            self.cy += 195 + 134;
            self.flush(hw);
        }
        self.dev_flush(hw, vi);
        if charge {
            // the device's stub's rts, the rest of stop_audio: the version check and its return
            self.cy += 16 + 16 + 9 + 28 + 16;
        }
    }

    /// `NoteOn`, with the 68000 cycles of its code between the host calls it makes (the allocation
    /// and the `CMD_WRITE` requests of the attack and the sustain).
    fn note_on(&mut self, hw: &mut Hw, c: usize, note: u8, vol: u8, pri: u16) {
        // the entry, the test of the volume (a zero volume leaves at once: the exit is a movem and an rts)
        self.cy += 107;
        if vol == 0 {
            self.cy += 92;
            return;
        }
        // the channel's patch's sample
        self.cy += 35;
        if self.patches[self.chans[c].patch].sample.is_none() {
            self.cy += 92;
            return;
        }
        let velocity = |s: &Self| if s.flags & MUSIC_VELOCITY != 0 { vol as u16 + 1 } else { 128 };
        let vi;
        // btst, beq: mono mode or not
        self.cy += 16 + 9;
        if self.chans[c].flags & CHAN_MONO != 0 && self.chans[c].voices_active != 0 {
            // tst.b, beq, the search for the channel's voice
            self.cy += 12 + 9 + 8 + 4;
            let Some(i) = (0..NUM_VOICES).find(|&i| self.voices[i].chan == Some(c)) else {
                self.cy += 92;
                return;
            };
            self.cy += (i as u64 + 1) * (14 + 9 + 8 + 10) - 8 - 10 + 4 + 11;
            // cmpi, bmi, btst, beq
            self.cy += 16 + 9;
            if self.voices[i].status >= ENV_SUSTAIN {
                self.cy += 16 + 9;
            }
            if self.voices[i].status >= ENV_SUSTAIN && self.chans[c].flags & CHAN_PORTAMENTO != 0 {
                let d1 = velocity(self);
                // the portamento's set-up
                self.cy += 24 + 20 + 9 + 16 + 16 + 8 + 20 + 9 + 4 + 12 + 4 + 12 + 24 + 24 + 10 + 92;
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
            if self.chans[c].flags & CHAN_MONO != 0 {
                // tst.b, beq
                self.cy += 12 + 9;
            }
            let side = if self.chans[c].flags & CHAN_PAN != 0 { 1 } else { 0 };
            // moveq, btst, beq, (moveq), move.w, bsr
            self.cy += 4 + 16 + 9 + 4 + 18;
            if side == 1 {
                self.cy += 4;
            }
            let p = self.pick_voice(side, pri);
            // tst.l, beq
            self.cy += 4 + 11;
            let Some(i) = p else {
                self.cy += 92;
                return;
            };
            // movea
            self.cy += 4;
            vi = i;
        }
        // tst.l (a5), beq
        self.cy += 12 + 9;
        if self.voices[vi].chan.is_some() {
            // movea, bsr, KillVoice (with the stop request), move.b, bra
            self.cy += 4 + 18;
            self.kill_voice_cost(hw, vi, true);
            self.cy += 16 + 10;
            self.voices[vi].flags = VOICE_STOLEN;
        } else {
            // clr.b
            self.cy += 16;
            self.voices[vi].flags = 0;
        }
        // move.l, move.l, move.b, movea, bsr
        self.cy += 12 + 24 + 16 + 4 + 18;
        self.voices[vi].chan = Some(c);
        self.voices[vi].patch = self.chans[c].patch;
        self.voices[vi].base_note = note;
        let sample = self.calc_note(vi);
        self.voices[vi].priority = pri as u8;
        self.voices[vi].status = ENV_START;
        // move.l, move.b, move.b, move.w, btst, beq
        self.cy += 4 + 12 + 16 + 8 + 20 + 9;
        let mut d1 = velocity(self);
        if self.flags & MUSIC_VELOCITY != 0 {
            self.cy += 4 + 12 + 4;
        }
        // moveq, move.b, bmi
        self.cy += 4 + 12 + 9;
        let cv = self.chans[c].volume;
        if cv < 128 {
            self.cy += 54 + 20;
            d1 = ((d1 as u32 * cv as u32) as u16) >> 7;
        }
        // move.w, clr.w, clr.l, move.w, bne
        self.cy += 12 + 16 + 24 + 12 + 9;
        {
            let v = &mut self.voices[vi];
            v.note_volume = d1;
            v.base_volume = 0;
            v.last_ticks = 0;
        }
        let per = if self.voices[vi].last_period == 0 { 1000 } else { self.voices[vi].last_period };
        if self.voices[vi].last_period == 0 {
            self.cy += 8;
        }
        // movea, tst.l, beq
        self.cy += 4 + 16 + 9;
        let Some(s) = sample else {
            self.undo_voice(vi);
            return;
        };
        let smp = self.samples[s];
        if smp.attack != 0 {
            // the request's allocation (two movea, jsr), the host call, the set-up of the request up to the
            // jsr to the device, and its host call
            self.cy += 16 + 16 + 18;
            self.flush(hw);
            self.event_half = 0;
            self.cy += 16 + 261;
            self.flush(hw);
            let r = self.new_req(smp.wave, smp.attack, 1, true, per);
            self.dev_write(hw, vi, r);
            // the stub's rts, the test of the request's error
            self.cy += 16 + 25;
        }
        // movea, tst.l, beq
        self.cy += 4 + 16 + 11;
        if smp.sustain != 0 {
            self.cy += 50;
            self.flush(hw);
            self.event_half = 0;
            self.cy += 16 + if smp.attack != 0 { 280 } else { 324 };
            self.flush(hw);
            let pervol = smp.attack == 0;
            let r = self.new_req(smp.wave.wrapping_add(smp.attack), smp.sustain, 0, pervol, per);
            self.dev_write(hw, vi, r);
            self.cy += 16 + 25;
        }
        // the epilogue: the channel's number, the count of voices, the mono test
        self.cy += 86;
        self.chans[c].voices_active = self.chans[c].voices_active.wrapping_add(1);
        let ch = self.chans[c];
        if ch.flags & CHAN_MONO != 0 {
            // btst, beq, and with portamento the last note's test
            self.cy += 16 + 9;
            if ch.flags & CHAN_PORTAMENTO != 0 {
                self.cy += 12 + 9;
                if (ch.last_note as i8) >= 0 {
                    self.cy += 12 + 9;
                    if ch.last_note != self.voices[vi].base_note {
                        self.cy += 24 + 20 + 12 + 20;
                    }
                }
            }
        }
        {
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
        }
        // btst, beq, (move.b), the flag and the voice number to the globals, the exit
        self.cy += 16 + 9 + 24 + 24 + 92;
        if self.chans[c].flags & CHAN_PORTAMENTO != 0 {
            self.cy += 16;
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

    /// `NoteOff`, with its 68000 cycles.
    fn note_off(&mut self, c: usize, note: u8) {
        // move.l, tst.b, beq (and at the exits a movea and an rts)
        self.cy += 12 + 12 + 9 + 12 + 16;
        if self.chans[c].voices_active == 0 {
            return;
        }
        // lea, moveq, move.b, mulu, adda, cmpa, bne
        self.cy += 8 + 4 + 12 + 58 + 8 + 14 + 9;
        let vi = self.last_voice;
        let v = &mut self.voices[vi];
        if v.chan != Some(c) {
            return;
        }
        // cmpi, ble
        self.cy += 16 + 9;
        if (v.status as i8) <= ENV_RELEASE as i8 {
            return;
        }
        // move.b, btst, beq, and the compare with the note, bne
        self.cy += 8 + 16 + 9 + 12 + 9;
        let want = if v.flags & VOICE_PORTAMENTO != 0 { v.end_note } else { v.base_note };
        if note != want {
            return;
        }
        // btst, bne, and the damper's bset or the release's move.b and bra
        self.cy += 16 + 9;
        if self.chans[c].flags & CHAN_DAMPER != 0 {
            v.flags |= VOICE_DAMPER;
            self.cy += 20;
        } else {
            v.status = ENV_RELEASE;
            self.cy += 16 + 10;
        }
    }

    /// The voices of the channel `c` all end (`AllNotesOff`'s loop), in 68000 cycles.
    fn all_notes_off(&mut self, c: usize) {
        // tst.b, beq, rts
        self.cy += 12 + 9 + 16;
        if self.chans[c].voices_active == 0 {
            return;
        }
        // lea, moveq
        self.cy += 8 + 4;
        let damper = self.chans[c].flags & CHAN_DAMPER != 0;
        for v in self.voices.iter_mut() {
            // cmpa, bne, lea, dbf
            self.cy += 14 + 9 + 8 + 10;
            if v.chan == Some(c) {
                // btst, beq, and bset, bra or move.b
                self.cy += 16 + 9;
                if damper {
                    v.flags |= VOICE_DAMPER;
                    self.cy += 20 + 10;
                } else {
                    v.status = ENV_RELEASE;
                    self.cy += 16;
                }
            }
        }
    }

    fn all_sounds_off(&mut self, hw: &mut Hw, c: usize) {
        // tst.b, beq, rts
        self.cy += 12 + 9 + 16;
        if self.chans[c].voices_active == 0 {
            return;
        }
        // movem, movea, lea, moveq, and at the end a movem
        self.cy += 32 + 4 + 12 + 4 + 36;
        for i in 0..NUM_VOICES {
            // cmpa, bne, lea, dbf
            self.cy += 14 + 9 + 8 + 10;
            if self.voices[i].chan == Some(c) {
                // movea, bsr
                self.cy += 4 + 18;
                self.kill_voice_cost(hw, i, true);
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
    /// portamento-off code writes into (`chan_LastNote(a2)` with the event pointer in a2). The cycles
    /// are those of the code's chain of compares (17 for each, the handler's own and a final rts).
    fn control_ch(&mut self, hw: &mut Hw, c: usize, ctl: u8, val: u8, ev: usize) {
        // the controller numbers in the order of the code's compares
        const ORDER: [u8; 15] = [5, 37, 6, 7, 10, 64, 65, 81, 100, 101, 121, 123, 126, 127, 120];
        let k = ORDER.iter().position(|&o| o == ctl).map_or(ORDER.len(), |i| i + 1) as u64;
        // move.b, the compares, and the rts
        self.cy += 8 + 17 * k + 16;
        match ctl {
            5 => {
                self.cy += 4 + 12 + 20 + 12 + 10;
                self.chans[c].portamento_time = (val as u16) << 7
            }
            37 => {
                self.cy += 12 + 8 + 12 + 12 + 10;
                let ch = &mut self.chans[c];
                ch.portamento_time = (ch.portamento_time & 0x3f80) | val as u16;
            }
            6 => {
                // tst.w, bne
                self.cy += 12 + 11;
                if self.chans[c].rpn != 0 {
                    return;
                }
                // moveq, move.b, cmpi, ble, (move.b), move.b, move.w, subi, beq
                self.cy += 4 + 12 + 8 + 9 + 12 + 12 + 8 + 11;
                let d0 = val.min(MAX_BEND_RANGE);
                if val > MAX_BEND_RANGE {
                    self.cy += 8;
                }
                self.chans[c].range = d0;
                let d1 = self.chans[c].pitch_bend.wrapping_sub(NO_BEND);
                if d1 == 0 {
                    return;
                }
                // mulu, lsl, divu, move.w, bset, bra
                self.cy += 54 + 24 + 128 + 12 + 20 + 10;
                let d1 = (d1 as u32 * d0 as u32) << 8;
                let d1 = divu_w(d1, NO_BEND);
                self.chans[c].real_bend = d1 as u16;
                self.chans[c].flags |= CHAN_ALTERED;
            }
            7 => {
                // move.b, beq, (addq), move.b, bra
                self.cy += 12 + 9 + 12 + 10;
                let mut d0 = val;
                if d0 != 0 {
                    self.cy += 4;
                    d0 = d0.wrapping_add(1);
                }
                self.chans[c].volume = d0;
            }
            10 => {
                // move.b, cmpi, bmi, and for 64 and over: bne, and for 64 the test of the number's bit
                self.cy += 12 + 8 + 9;
                let ch = &mut self.chans[c];
                if val < 64 {
                    self.cy += 20 + 10;
                    ch.flags |= CHAN_PAN;
                } else if val == 64 {
                    self.cy += 9 + 16 + 9;
                    if c & 1 == 0 {
                        self.cy += 20 + 10;
                        ch.flags |= CHAN_PAN;
                    } else {
                        self.cy += 20 + 10;
                        ch.flags &= !CHAN_PAN;
                    }
                } else {
                    self.cy += 9 + 20 + 10;
                    ch.flags &= !CHAN_PAN;
                }
            }
            64 => {
                // btst, beq
                self.cy += 16 + 9;
                if val & 0x40 != 0 {
                    self.cy += 20 + 10;
                    self.chans[c].flags |= CHAN_DAMPER;
                } else {
                    // bclr, lea, moveq, the loop over the voices, bra
                    self.cy += 20 + 8 + 4 + 10;
                    self.chans[c].flags &= !CHAN_DAMPER;
                    for v in self.voices.iter_mut() {
                        // cmpa, bne, lea, dbf
                        self.cy += 14 + 9 + 8 + 10;
                        if v.chan == Some(c) {
                            // bclr, beq
                            self.cy += 20 + 9;
                            if v.flags & VOICE_DAMPER != 0 {
                                v.flags &= !VOICE_DAMPER;
                                v.status = ENV_RELEASE;
                                self.cy += 16;
                            }
                        }
                    }
                }
            }
            65 => {
                // btst, beq
                self.cy += 16 + 9;
                if val & 0x40 != 0 {
                    self.cy += 20 + 10;
                    self.chans[c].flags |= CHAN_PORTAMENTO;
                } else {
                    self.cy += 20 + 16 + 10;
                    self.chans[c].flags &= !CHAN_PORTAMENTO;
                    // the driver stores through its event pointer here, not the channel
                    let o = ev + 17;
                    if o < self.d.len() {
                        self.d[o] = 0xff;
                    }
                }
            }
            81 => {
                // moveq, cmpi, beq, bmi, (moveq, bra | move.w), bsr, and the filter's routine: the read of the
                // port, a test, a bset or bclr and the rts
                self.cy += 4 + 16 + 9 + 9 + 18 + 4 + 18 + (16 + 10 + 9 + 4 + 16) + 4 + 9 + 24 + 16 + 10;
                let on = match val.cmp(&64) {
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => {
                        self.cy += 12;
                        self.filter != 0
                    }
                    std::cmp::Ordering::Greater => {
                        self.cy += 4 + 10;
                        true
                    }
                };
                Self::set_audio_filter(hw, on);
            }
            100 => {
                self.cy += 20 + 10;
                self.chans[c].rpn = (self.chans[c].rpn & 0xff00) | val as u16
            }
            101 => {
                self.cy += 20 + 10;
                self.chans[c].rpn = (self.chans[c].rpn & 0x00ff) | (val as u16) << 8
            }
            121 => {
                // bsr, the channel's reset (move.w x2, clr.w, move.b x2, btst, bne, bclr or bset, bra, 3 flag
                // operations, rts), bra
                self.cy += 18 + 16 * 5 + 16 + 9 + 20 + 20 + 20 + 16 + 10;
                self.cy += if c & 1 == 0 { 20 + 10 } else { 20 };
                self.reset_channel(c)
            }
            123 => {
                self.cy += 18 + 10;
                self.all_notes_off(c)
            }
            126 => {
                self.cy += 20 + 18 + 10;
                self.chans[c].flags |= CHAN_MONO;
                self.all_notes_off(c);
            }
            127 => {
                self.cy += 20 + 18 + 10;
                self.chans[c].flags &= !CHAN_MONO;
                self.all_notes_off(c);
            }
            120 => {
                self.cy += 18;
                self.all_sounds_off(hw, c)
            }
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

    /// The 68000 cycles of `comdivide`, the driver's long division, of `n` by `d`: the short form (two
    /// DIVU.W) when the divisor has no high word, else the shift-and-subtract loop, which costs 12
    /// more for each quotient bit set.
    fn comdivide_cycles(n: u32, d: u32) -> u64 {
        if d >> 16 == 0 {
            return 41 + 336;
        }
        let mut d1 = n >> 16;
        let mut d0 = n << 16;
        let mut k = 0u64;
        for _ in 0..16 {
            let x = d0 >> 31;
            d0 <<= 1;
            d1 = d1.wrapping_add(d1).wrapping_add(x);
            if d <= d1 {
                d1 = d1.wrapping_sub(d);
                d0 = d0.wrapping_add(1);
                k += 1;
            }
        }
        41 + 32 + 16 * 41 + 12 * k + 44
    }

    /// The 68000 cycles of the driver's `divs` (the long signed division) of `d0` by `d1`.
    fn divs_l_cycles(d0: u32, d1: u32) -> u64 {
        // move.l, moveq, tst, bpl, tst, bpl, jsr, tst.w, beq, move.l, rts
        let mut c = 12 + 4 + 4 + 9 + 4 + 9 + 20 + 4 + 9 + 12 + 16;
        let (a, b) = (d0 as i32, d1 as i32);
        if a < 0 {
            c += 14;
        }
        if b < 0 {
            c += 14;
        }
        if (a < 0) != (b < 0) {
            c += 6;
        }
        c + Self::comdivide_cycles(a.unsigned_abs(), b.unsigned_abs())
    }

    /// `calc_incrvol`, and the cycles it takes (added to the running count).
    fn calc_incrvol(&mut self, delta: i32, time: u16) -> i32 {
        if time == 0 {
            self.cy += 24 + 4 + 9 + 28 + 16;
            return delta;
        }
        let d2 = delta;
        let d0 = delta.wrapping_abs() as u32;
        let d3 = d0 as u16;
        let n = (d0 & 0xFFFF) * 1000;
        let t = time as u32 * FREQUENCY as u32;
        let q = n / t;
        let neg = delta < 0;
        // movem, tst, beq, move, bpl, [neg], move, two mulu, bsr, the division (a bsr and an rts around
        // `comdivide`), cmp, bmi
        let mut c = 24 + 4 + 9 + 4 + 9 + 4 + 58 + 66 + 18 + 34 + Self::comdivide_cycles(n, t) + 4 + 9;
        if neg {
            c += 6;
        }
        let r = if d3.wrapping_sub(q as u16) & 0x8000 != 0 {
            // time too short: the delta itself (a move and a bra)
            c += 4 + 10;
            d2
        } else {
            c += 4 + 9;
            if neg {
                c += 6;
            }
            if neg {
                (q as i32).wrapping_neg()
            } else {
                q as i32
            }
        };
        self.cy += c + 28 + 16;
        r
    }

    /// `DoOneVoice`: one voice's envelope for a frame of `delta`.
    fn do_one_voice(&mut self, hw: &mut Hw, vi: usize, delta: u32) {
        // the loop's bsr, the channel test (a free voice returns at once), and the loop's lea and dbra
        let Some(ci) = self.voices[vi].chan else {
            self.cy += 18 + 12 + 11 + 16 + 8 + 10;
            return;
        };
        // the bsr, the channel test, the add to the voice's ticks, the test of the status for sustain
        self.cy += 18 + 12 + 11 + 4 + 24 + 16 + 9;
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
            self.cy += 16 + 11;
            if self.chans[ci].flags & CHAN_ALTERED != 0 {
                next = Some(Next::Calc);
            } else {
                self.cy += 16 + 11;
                if self.voices[vi].flags & VOICE_PORTAMENTO != 0 {
                    next = Some(Next::Calc);
                } else {
                    // bra, rts, and the loop's lea and dbra
                    self.cy += 10 + 16 + 8 + 10;
                    return;
                }
            }
        } else {
            self.cy += 16 + 9;
            if status == ENV_HALT {
                // the movea and the bsr to KillVoice, then the bra, the rts and the loop's lea and dbra
                self.cy += 4 + 18;
                self.kill_voice_cost(hw, vi, true);
                self.cy += 10 + 16 + 8 + 10;
                return;
            }
            // the movea of the patch, the compare for start, the bne
            self.cy += 16 + 16 + 9;
            let patch = self.patches[self.voices[vi].patch];
            let st = self.voices[vi].status;
            let mut go_env = true;
            if st == ENV_START {
                self.voices[vi].envelope = patch.attack;
                self.cy += 16 + 16 + 9;
                match patch.attack {
                    None => {
                        // move.b, move.w, move.l, bra
                        self.cy += 16 + 20 + 16 + 10;
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
                        // the set-up (move.w, moveq, move.w, lsl, move.l, move.b, move.l, moveq, move.w, move.w),
                        // the bsr, and after the call a move.l and a bra
                        self.cy += 20 + 4 + 8 + 24 + 16 + 16 + 16 + 4 + 12 + 8 + 18;
                        let iv = self.calc_incrvol(vol as i32, dur);
                        self.voices[vi].incr_volume = iv;
                        self.cy += 16 + 10;
                    }
                }
            } else {
                // the compare for release and its bne
                self.cy += 16 + 9;
                if st == ENV_RELEASE {
                    self.voices[vi].envelope = patch.release;
                    self.cy += 16 + 16 + 9;
                    match patch.release {
                        None => {
                            // move.b, moveq, bra
                            self.cy += 16 + 4 + 10;
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
                            // the set-up as for the attack with the subtraction of the base volume, and the bsr
                            self.cy += 20 + 4 + 8 + 24 + 16 + 16 + 16 + 4 + 12 + 4 + 12 + 8 + 8 + 18;
                            let iv = self.calc_incrvol(d0, dur);
                            self.voices[vi].incr_volume = iv;
                            self.cy += 16;
                        }
                    }
                }
            }
            if go_env {
                // the compare for sustain and its beq
                self.cy += 16 + 11;
                if self.voices[vi].status == ENV_SUSTAIN {
                    next = Some(Next::Calc);
                } else {
                    // the compare of the delta with the time left and its bmi
                    self.cy += 18 + 9;
                    if (d2.wrapping_sub(self.voices[vi].ticks_left) as i32) >= 0 {
                        // the segment is over: movea, move.w, subq, bne
                        self.cy += 16 + 20 + 16 + 9;
                        let e = self.voices[vi].envelope.unwrap_or(0);
                        let (_, vol) = self.env(e);
                        self.voices[vi].base_volume = vol;
                        self.voices[vi].envelope_left = self.voices[vi].envelope_left.wrapping_sub(1);
                        if self.voices[vi].envelope_left == 0 {
                            // the compare for decay and its bne
                            self.cy += 16 + 9;
                            if self.voices[vi].status == ENV_DECAY {
                                // move.b, moveq, bra
                                self.cy += 16 + 4 + 10;
                                self.voices[vi].status = ENV_HALT;
                                next = Some(Next::Send(0));
                            } else {
                                // move.b, move.l, bra
                                self.cy += 16 + 16 + 10;
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
                            // adda, move.l, moveq, move.w, lsl, move.l, moveq, move.w, moveq, move.w, sub, move.w, bsr
                            self.cy += 8 + 16 + 4 + 8 + 24 + 16 + 4 + 12 + 4 + 12 + 8 + 8 + 18;
                            let iv = self.calc_incrvol(d0, dur);
                            self.voices[vi].incr_volume = iv;
                            self.cy += 16 + 10;
                            next = Some(Next::Calc);
                        }
                    } else {
                        // moveq, move.w, add.l, bmi
                        self.cy += 4 + 12 + 18 + 9;
                        let d0 = self.voices[vi].base_volume as i32 + self.voices[vi].incr_volume;
                        let d0 = if d0 < 0 {
                            self.cy += 4;
                            0
                        } else {
                            // cmpi, bmi
                            self.cy += 14 + 9;
                            if d0 >= 0x8000 {
                                self.cy += 8 + 10;
                                0x8000
                            } else {
                                d0
                            }
                        };
                        // move.w, sub.l
                        self.cy += 12 + 24;
                        let v = &mut self.voices[vi];
                        v.base_volume = d0 as u16;
                        v.ticks_left = v.ticks_left.wrapping_sub(d2);
                        next = Some(Next::Calc);
                    }
                }
            }
        }
        // .18: the volume, then the note when it changes
        let vol = match next {
            Some(Next::Send(v)) => v,
            _ => {
                // movea, bsr, the volume, move.w, the test of the portamento flag and its beq
                self.cy += 4 + 18;
                let d4 = self.calc_volume(vi);
                self.cy += 4 + 16 + 9;
                if self.voices[vi].flags & VOICE_PORTAMENTO != 0 {
                    // add.l, move.l, lsr, cmp.w, bmi
                    self.cy += 24 + 16 + 24 + 12 + 9;
                    let ptime = self.chans[ci].portamento_time;
                    let v = &mut self.voices[vi];
                    v.porta_ticks = v.porta_ticks.wrapping_add(d2);
                    let d0 = (v.porta_ticks >> 8) as u16;
                    if (d0.wrapping_sub(ptime) as i16) >= 0 {
                        v.flags &= !VOICE_PORTAMENTO;
                        v.base_note = v.end_note;
                        // bclr, move.b, bra
                        self.cy += 20 + 16 + 10;
                    }
                    self.cy += 20 + 4 + 18;
                    self.voices[vi].flags |= VOICE_RECALC;
                    self.calc_note(vi);
                } else {
                    // the test of the channel's altered flag and its beq
                    self.cy += 16 + 9;
                    if self.chans[ci].flags & CHAN_ALTERED != 0 {
                        // bset, movea, bsr
                        self.cy += 20 + 4 + 18;
                        self.voices[vi].flags |= VOICE_RECALC;
                        self.calc_note(vi);
                    }
                }
                d4
            }
        };
        // .17: ADCMD_PERVOL: the set-up of the request up to the jsr, 2 less for no period
        let lp = self.voices[vi].last_period;
        self.cy += 16 + 16 + 16 + 4 + 12 + 4 + 16 + 16 + 12 + 9 + 20 + 18;
        self.cy += if lp != 0 { 12 + 12 + 10 } else { 16 + 16 };
        self.flush(hw);
        if lp != 0 {
            self.dev_pervol(hw, vi, lp, vol);
        } else {
            self.dev_pervol(hw, vi, 1000, 0);
        }
        // the rts of the device's stub, the rts, and the loop's lea and dbra
        self.cy += 16 + 16 + 8 + 10;
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

    /// `MusicServer`: one frame, with the 68000 cycles of its code (the numbers are the sums of the
    /// instructions' times along the path taken).
    fn music_server(&mut self, hw: &mut Hw) {
        // the interrupt server's wrapper (a tst.l, a bne, a movea, a lea and the jsr to the host), whose
        // host call starts the server on a fresh colour clock
        self.event_half = 0;
        self.cy = 71;
        self.flush(hw);
        self.event_half = 0;
        // the server's entry, the test of its flags, and the clock's advance
        self.cy += 233;
        if std::mem::take(&mut self.settings_new) {
            // the Eagleplayer's settings changed (always so at a song's first frame): it copies the
            // volume and the velocity flag and marks the channels of the voices in use
            self.cy += 268;
            for v in 0..NUM_VOICES {
                if let Some(c) = self.voices[v].chan {
                    self.chans[c].flags |= CHAN_ALTERED;
                    self.cy += 24;
                }
            }
        }
        self.ticks = self.ticks.wrapping_add(self.tick_unit);
        let d4 = self.ticks >> 8;
        for v in 0..NUM_VOICES {
            self.last_voice = v;
            // move.b, tst.l, beq
            self.cy += 33;
            if self.voices[v].chan.is_none() {
                // the loop's tail
                self.cy += 30;
                continue;
            }
            // move.b, bmi
            self.cy += 17;
            let ne = self.note_off[v];
            if (ne.command as i8) < 0 {
                self.cy += 30;
                continue;
            }
            // the channel's test, the subtraction of the frame's time, bgt
            self.cy += 33 + 49;
            let st = ne.stop_time.wrapping_sub(self.tick_unit as i32);
            self.note_off[v].stop_time = st;
            if st > 0 {
                self.cy += 30;
                continue;
            }
            // the call of NoteOff, and what follows it
            self.cy += 120;
            self.note_off(ne.data as usize & 15, ne.command);
            self.note_off[v].command = 0xff;
            self.cy += 22 + 30;
        }
        // btst, beq
        self.cy += 27;
        if self.flags & MUSIC_PLAYING != 0 {
            // movea
            self.cy += 16;
            let mut a2 = self.current;
            let mut guard = 0u32;
            loop {
                guard += 1;
                if guard > 100_000 {
                    break;
                }
                // the read of the beam's position, which stops the events once the beam is in the
                // second half of the 256 lines
                self.cy += 16;
                self.flush(hw);
                self.cy += 11;
                if hw.bus.beam().0 & 0x80 != 0 {
                    break;
                }
                let start = be16(&self.d, a2 + 2) as u32;
                let d0 = start.wrapping_add(self.current_time);
                // moveq, move.w, add.l, cmp.l, bmi
                self.cy += 4 + 12 + 18 + 6 + 11;
                if (d4.wrapping_sub(d0) as i32) < 0 {
                    break;
                }
                self.current_time = d0;
                let cmd = self.rb(a2);
                let data = self.rb(a2 + 1);
                let stop = be16(&self.d, a2 + 4);
                // move.l, move.b, bmi
                self.cy += 16 + 8 + 9;
                if (cmd as i8) >= 0 {
                    self.flags &= !MUSIC_ADDED_NOTE;
                    let vol = (data >> 1) & 0x78;
                    let c = (data & 15) as usize;
                    // the set-up of the call
                    self.cy += 184;
                    self.note_on(hw, c, cmd, vol, MUSIC_PRI_SCORE);
                    // btst, beq
                    self.cy += 16 + 11;
                    if self.flags & MUSIC_ADDED_NOTE != 0 {
                        // the stop event's entry
                        self.cy += 226;
                        let lv = self.last_voice;
                        let d0 = (stop as u32).wrapping_add(self.current_time).wrapping_sub(d4);
                        self.note_off[lv] = NoteOffEv { command: cmd, data: data & 15, stop_time: (d0 << 8) as i32 };
                    }
                } else if cmd == COMMAND_TEMPO {
                    // cmpi, bne, the compare of the tempo with the frame's
                    self.cy += 17 + 61;
                    if (((self.tick_unit >> 8) as u16).wrapping_sub(stop) as i16) < 0 {
                        // continuous
                        self.cy += 160;
                        self.start_tempo = self.current_tempo;
                        let d1 = ((data as u16) << 4).wrapping_sub(self.current_tempo);
                        self.delta_tempo = d1;
                        self.tempo_time = (stop as u32) << 8;
                        self.tempo_ticks = 0;
                    } else {
                        // the call of SetTempo (with its division) and the clearing of the ramp
                        self.cy += 82 + 236 + 34 + DIV_FAST;
                        self.set_tempo((data as u16) << 4);
                        self.tempo_time = 0;
                    }
                } else if cmd == COMMAND_END {
                    // two compares, btst, beq
                    self.cy += 17 + 17 + 25;
                    self.songend = true;
                    if self.flags & MUSIC_LOOP != 0 {
                        // up to the call of the host
                        self.cy += 122;
                        self.flush(hw);
                        self.event_half = 0;
                        // the stub's rts, movea, bra
                        self.cy += 16 + 12 + 10;
                        a2 = self.scores[self.current_score].0;
                        self.ticks = 0;
                        self.current_time = 0;
                    } else {
                        // bclr, bra
                        self.cy += 20 + 10;
                        self.flags &= !MUSIC_PLAYING;
                    }
                    break;
                } else if cmd == COMMAND_BEND {
                    self.cy += 17 * 3 + 176 + 350 + 10;
                    self.pitch_bend((data & 15) as usize, stop as u8 & 0x7f, (stop >> 8) as u8 & 0x7f);
                } else if cmd == COMMAND_CONTROL {
                    self.cy += 17 * 4 + 152 + 10;
                    self.control_ch(hw, (data & 15) as usize, (stop >> 8) as u8, stop as u8, a2);
                } else if cmd == COMMAND_PROGRAM {
                    self.cy += 17 * 5 + 132 + 122 + 10;
                    self.chans[(data & 15) as usize].patch = (stop as u8 as usize) & (NUM_PATCHES - 1);
                } else if cmd == COMMAND_SPECIAL {
                    let (d0, d1) = ((stop >> 8) as u8, stop as u8);
                    // five compares, the sub-command's moveq, two moves, cmpi, bne
                    self.cy += 17 * 5 + 19 + 45;
                    if d0 == SPECIAL_BEGINREP {
                        // cmpi, bne, the test of the table's size, and the entry
                        self.cy += 17 + 33;
                        let n = self.repeat_total as usize;
                        if n != NUM_REPEATS {
                            self.cy += 92;
                            self.repeat_total += 1;
                            self.repeat_count[n] = d1;
                            self.repeat_point[n] = a2 + EV;
                        }
                    } else if d0 == SPECIAL_ENDREP {
                        self.cy += 17 + 17 + 25;
                        if self.repeat_total != 0 {
                            // subi, lea, tst.b, beq
                            self.cy += 35;
                            let n = self.repeat_total as usize - 1;
                            if self.repeat_count[n] != 0 {
                                self.cy += 112;
                                self.repeat_count[n] -= 1;
                                a2 = self.repeat_point[n];
                                self.ticks = 0;
                                self.current_time = 0;
                                break;
                            }
                            self.cy += 12 + 10;
                            self.repeat_total = n as u8;
                        }
                    } else {
                        self.cy += 17 + 17;
                    }
                } else {
                    // the compares of the last kind, and the unknown command's exit
                    self.cy += 17 * 5 + 19;
                }
                // adda, bra
                self.cy += 18;
                a2 += EV;
            }
            self.current = a2;
            // the store of the event pointer, the test of the tempo ramp
            self.cy += 41;
            if self.tempo_time != 0 {
                self.cy += 65;
                self.tempo_ticks = self.tempo_ticks.wrapping_add(self.tick_unit);
                if (self.tempo_ticks.wrapping_sub(self.tempo_time) as i32) < 0 {
                    // the ramp: the multiplication routine, the long division, the call of SetTempo
                    let d0 = (self.delta_tempo as u32).wrapping_mul(self.tempo_ticks);
                    self.cy += 4 + 12 + 18 + 278 + 16 + 18 + Self::divs_l_cycles(d0, self.tempo_time) + 12 + 18;
                    self.cy += 236 + 34 + DIV_FAST;
                    let q = divs_l(d0, self.tempo_time);
                    self.set_tempo((q as u16).wrapping_add(self.start_tempo));
                } else {
                    // move.w, add.w, the call of SetTempo, clr.l, bra
                    self.cy += 12 + 12 + 18 + 236 + 34 + DIV_FAST + 24 + 10;
                    self.set_tempo(self.start_tempo.wrapping_add(self.delta_tempo));
                    self.tempo_time = 0;
                }
            }
        }
        // the call of the envelope manager (move.l, bsr) and its entry
        self.cy += 34 + 80;
        self.envelope_manager(hw, FRAME_UNIT);
        // after the last voice: the loop over the channels that clears the altered flags and the exit
        self.cy += 732;
        // bset, lea, moveq, the loop over the voices (a free voice is a few cycles shorter), the flags
        // and the exit, then the wrapper's end
        self.cy += 32;
        for v in 0..NUM_VOICES {
            self.cy += if self.voices[v].chan.is_some() { 69 } else { 64 };
        }
        self.cy += 168 + 20;
        // the audio interrupts wait for the end of the server
        self.wh(hw, T_TAIL);
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
        self.settings_new = true;
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
        self.tick_due = 0;
        self.grid = None;
        self.songend = false;
        hw.wait(SUBSONG_CLOCKS - FILTER_TO_START - 1);
        self.init_sound(hw, n);
        hw.wait(FILTER_TO_START + 1);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.intena_set {
            // the audio interrupts on, so that the engine tells of them (`audio_irq_ch`)
            hw.bus.paula.write_intena(0x8000 | 0xF << 7);
            self.intena_set = true;
        }
        self.tick_due = hw.bus.clock + TICK_CLOCKS;
        if self.grid.is_none() {
            self.see_grid(hw);
        }
        self.music_server(hw);
        // the device takes the interrupts requested while the server ran
        self.service_all(hw);
    }

    /// The reference's audio.device does not run on Paula's interrupts: it looks at the requests
    /// of its channels whenever the machine has run to the next thing that happens, which is the
    /// end of an output sample or a frame's server. So a request waits for the end of the sample
    /// it falls in, and one that falls in the last sample before a frame for the end of the server.
    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        let bit = 0x80u16 << ch;
        if hw.bus.paula.intreq & bit == 0 || (self.dev[ch].playing.is_none() && self.dev[ch].latched.is_none()) {
            return true;
        }
        if !self.at_sample_end(hw) {
            let end = hw.bus.clock + hw.bus.paula.out.until_sample() as u64;
            if self.tick_due <= end {
                return true;
            }
            hw.bus.clock = end;
            hw.bus.paula.sync(end, &hw.bus.ram);
        }
        self.service_all(hw);
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

    /// A score without an end event runs on into the reference's memory after it: the next score's
    /// events on the next 8-byte boundary (in the file its 4-byte event count comes first).
    #[test]
    fn score_without_end_runs_into_the_next() {
        let mut d = b"MXTX".to_vec();
        p16(&mut d, 120);
        p16(&mut d, 0);
        p16(&mut d, 2);
        // 24 bytes, so that the second score follows at once
        p32(&mut d, 4);
        ev(&mut d, 60, 0xF0, 0, 30);
        for _ in 0..3 {
            ev(&mut d, COMMAND_PROGRAM, 0, 0, 0);
        }
        p32(&mut d, 2);
        ev(&mut d, 64, 0xF0, 200, 30);
        ev(&mut d, COMMAND_END, 0, 100, 0);
        let m = module(false);
        d.extend(&m[scan(&m).unwrap()..]);
        assert!(detect(&d));
        let (e, log) = play(d, 2);
        assert!(e.songend(), "the second score's end was reached");
        let mut pers: Vec<u16> = log.iter().filter(|w| (0xDFF0A0..0xDFF0E0).contains(&w.0) && w.0 & 0xF == 6).map(|w| w.1).collect();
        pers.sort();
        pers.dedup();
        assert_eq!(pers.len(), 2, "both scores' notes play: {pers:?}");
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

    /// Playing over and over on one Engine must not use up the guest heap: a module whose
    /// samples cannot be installed (what the replayer allocated before giving up), and one that
    /// plays (what it allocated, until stop), each 30 times.
    #[test]
    fn repeated_plays_free_the_guest_heap() {
        let mut huge = module(false);
        let at = huge.len() - 64;
        huge[at..at + 4].copy_from_slice(&1_000_000u32.to_be_bytes());
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.os.mem_files.insert(Path::new("/mxtx-test2/SMPL.set").to_path_buf(), module(false));
        let free = e.m.heap.available();
        let first = e.play_data(Path::new("/mxtx-test2/mxtx.test"), huge.clone()).is_ok();
        e.stop();
        for _ in 0..30 {
            let ok = e.play_data(Path::new("/mxtx-test2/mxtx.test"), huge.clone()).is_ok();
            assert_eq!(ok, first, "play result changed: the heap leaks");
            e.stop();
        }
        for _ in 0..30 {
            e.play_data(Path::new("/mxtx-test2/mxtx.test"), module(false)).expect("play");
            e.stop();
        }
        assert_eq!(e.m.heap.available(), free);
    }
}
