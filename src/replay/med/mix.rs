//! OctaMED SoundStudio (MMD3) and OctaMED's 5-8 channel and mix mode songs
//! (MMD0-MMD2 with the 8-channel or mix flag), ported from NostalgicPlayer's
//! OctaMED player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer): the module loader (MMD0-MMD3, the old modules'
//! conversion to mix mode and the 5-8 channel tempo conversion), the player's
//! tick (rows, hold and decay, the effect commands, synth and hybrid sounds)
//! and its frequency table.
//!
//! # SoundStudio's songs (MMD3): the mixer
//!
//! The original (UADE's Octa-MED-Ss) mixes in software and plays the result on Paula, and so
//! does this port (`Soft`). What it does was measured on the reference's Paula data (the bytes
//! its DMA fetched) with built modules, one feature a module:
//!
//! - Paula: the four channels at period 149 and volume 64, each 16384 words long; channels 0
//!   and 3 play one buffer, 1 and 2 the other, two halves of 16384 samples each. Paula's
//!   interrupt at a half's start acknowledges, points the channels at the other half and mixes
//!   the song's next 16384 samples into it, so the first half played is silent. The original
//!   sets Paula up in its start, before the song's time runs.
//! - The song's ticks are counted in mixed samples: 24023 / Hz of them a tick (a fraction
//!   kept), a tick at the sample that count reaches. What the first tick's notes play before
//!   the second tick is not heard (the samples advance).
//! - A voice steps freq * 65536 / 24063 (rounded up) a sample, a 16.16 number, and plays the
//!   sample's byte there (no interpolation). The original's step differs by a few units from
//!   that for most notes; for the notes of the frequency table it is taken from the steps
//!   measured on the reference (`STEPS`).
//! - The levels: the volume V (0-64) and the pan (-16..16, w = 16 + pan) give side 0 (Paula's
//!   channels 0 and 3) the level L(w) and side 1 L(32 - w), L(0) = 0, L(32) = V, else
//!   1 + (V - 1) * w / 32. An 8-bit sample's term is s * L * voladj * 256 / (6400 * channels),
//!   cut towards zero; L * voladj is a signed 16-bit product (it wraps). Without the stereo
//!   flag both sides have L = V; with it but without free panning the channels count as half
//!   as many (rounded down, one at least; rounded up for the 16-bit samples' shift).
//! - A 16-bit sample is shifted, not multiplied: its level is L taken down to a power of two
//!   (a mono one's with the pan in the middle is V / 2, rounded down), and the volume adjust
//!   (down to a power of two of 100) over the channels (up to one) never raises it. A stereo
//!   sample's left channel plays on side 1, its right on side 0.
//! - A synth or hybrid sound's note is a period (`SYN_PERIODS`, measured; a slid frequency's
//!   is the colour clock over it), stepped as the whole frequency the colour clock gives over
//!   it; its instrument's volume is used, the notes above the fifth octave are not taken
//!   down, and a synth sound's note on a voice that plays one goes on where the voice is.
//! - An instrument whose output device byte (expansion data) is not 0 plays a semitone up,
//!   and command 0FFF (note off) does not stop it.
//! - A new note starts at the sample's first byte with the fraction of a byte the voice's
//!   position had.
//! - The sides' sums are 16-bit words (they wrap), the high byte is played. The echo adds
//!   what the side (crossed: the other side) had floor(ms * 24023 / 1000) samples ago,
//!   shifted down by the depth, to the word. Then the stereo separation adds the other
//!   side's word, shifted down by 5 - |sep|, negated for a positive one (the echo's memory
//!   is without it).
//!
//! Not measured, so guessed: an odd volume after the track's and the master volume (halved,
//! rounded down), and the step of a frequency that is not a table note's (a slide, a vibrato,
//! an arpeggio).
//!
//! The reference's CPU is not a 68000 for these songs (`needs_fast_reference_cpu`): the mixer
//! takes longer than a buffer with a few tracks on one, and the original then plays halves
//! twice.
//!
//! # The older songs: the amplifier
//!
//! OctaMED's 5-8 channel and mix mode songs go to the host's amplifier (`Hw::voice_*`, as
//! HivelyTracker's and the multichannel MODs' do): one voice a track, from the Amiga's LRRL
//! sides, at most 32. A note is the voice's DMA off, the sample's start and length, its period
//! (from the note's frequency in Hz), the volume, DMA on, and then the loop. What the amplifier
//! has no counterpart of is approximated or left out; see the README.
//!
//! - 16-bit and stereo samples are converted to 8-bit mono when the module is
//!   loaded (a 16-bit sample keeps its high byte, a stereo one its two channels'
//!   mean), in the module's own memory where that does not touch another sample.
//!   (SoundStudio's mixer plays their own data.)
//! - A ping-pong loop is unrolled into a loop of twice the length.
//! - A synth or hybrid sound's waveforms are the voice's blocks: a new waveform
//!   is the next block, taking effect where the current one ends, as on Paula;
//!   the sound's own envelope and vibrato programs run as in the original.
//! - The echo (the song's, or its one effect group's) is a copy of each track's
//!   voice, a delay later (to the nearest tick), on a voice of the same side (the
//!   other for the cross echo) at the volume of one repeat (shifted down by the
//!   depth). The repeats of the repeats are not played, nor are groups of
//!   several tracks' echoes, and a song whose tracks and echoes need more than
//!   32 voices has none.
//! - Not played: stereo separation, the track pans (a voice has its Amiga side
//!   only), "sample backwards" (command 20), MIDI instruments, the filter sweeps,
//!   and the volume adjust.
//!   The mix mode songs' voices mix at 31 kHz; a 5-8 channel song's at the amplifier's own
//!   rate, as the OctaMED Eagleplayer's do.
//! - The timing is UADE's: a tick of 3579545 / Hz colour clocks, and periods
//!   from that clock, as the 68k player counts them on a PAL Amiga.

use crate::machine::Machine;
use crate::paula::PAULA_CLOCK_PAL;
use crate::replay::{Hw, Open, Replayer, be16, be32, start_amplifier};
use m68k::AddressBus;
use std::collections::VecDeque;
use std::sync::Arc;

/// 5-8 channel and mix mode OctaMED songs before SoundStudio.
pub const OPEN_MIX: Open = Open {
    name: "OctaMED",
    detect: |d| kind_of(d) == Some(Kind::Old),
    new: |d, b, m| new(d, b, m, Kind::Old),
};
/// OctaMED SoundStudio's MMD3 songs.
pub const OPEN_OSS: Open = Open {
    name: "OctaMED SoundStudio",
    detect: |d| kind_of(d) == Some(Kind::Oss),
    new: |d, b, m| new(d, b, m, Kind::Oss),
};

/// One poke of a voice (see `OctaMed::vp`).
#[derive(Clone, Copy)]
enum Op {
    Dma(bool),
    Loc(u32),
    LenB(u32),
    Len(u16),
    Per(u16),
    Vol(u16),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Old,
    Oss,
}

const MAX_TRACKS: usize = 64;
const MAX_INSTR: usize = 64;
const MAX_VOICES: usize = 32;
/// The amplifier's mix rate as a Paula period (colour clocks a sample).
const MIX_PERIOD: u16 = 114;

const NOTE_DEF: u8 = 0x81;
const NOTE_11K: u8 = 0x82;
const NOTE_22K: u8 = 0x83;
const NOTE_44K: u8 = 0x84;

const FLAG_VOLHEX: u8 = 0x10;
const FLAG_STSLIDE: u8 = 0x20;
const FLAG_8CH: u8 = 0x40;
const FLAG2_BPM: u8 = 0x20;
const FLAG2_MIX: u8 = 0x80;

const I_LOOP: u8 = 1;
const I_DISABLED: u8 = 4;
const I_PINGPONG: u8 = 8;

const PS_STOP: u8 = 1;
const PS_POSJUMP: u8 = 2;

/// The vibrato sine (NostalgicPlayer's sineTable).
const SINE: [i8; 32] = [
    0, 25, 49, 71, 90, 106, 117, 125, 127, 125, 117, 106, 90, 71, 49, 25, 0, -25, -49, -71, -90,
    -106, -117, -125, -127, -125, -117, -106, -90, -71, -49, -25,
];
/// Tempos 1-10 of the old timing (NostalgicPlayer's bpmCompVals).
const BPM_COMP: [u16; 10] = [195, 97, 65, 49, 39, 32, 28, 24, 22, 20];
const BPM_VALS: [u8; 9] = [179, 164, 152, 141, 131, 123, 116, 110, 104];

const OCT_COUNT: [usize; 7] = [1, 5, 3, 2, 4, 6, 7];
const OCT_BUF: [[usize; 6]; 6] = [
    [4, 4, 3, 2, 2, 1],
    [2, 2, 2, 2, 2, 1],
    [1, 1, 0, 0, 0, 0],
    [3, 3, 2, 2, 2, 1],
    [5, 5, 5, 4, 3, 2],
    [6, 6, 6, 5, 4, 3],
];
const OCT_START: [[i32; 6]; 6] = [
    [12, 12, 0, -12, -12, -24],
    [0, 0, 0, 0, 0, -12],
    [0, 0, -12, -12, -12, -12],
    [0, 0, -12, -12, -12, -24],
    [0, 0, 0, -12, -24, -36],
    [0, 0, 0, -12, -24, -36],
];

/// NostalgicPlayer's frequency table: 16 finetunes of 72 notes, C-1 at
/// 1046.502261 Hz (finetune 0), 7.778532836 Hz apart per finetune step.
pub(super) fn freq_table() -> Vec<[u16; 72]> {
    (0..16)
        .map(|c| {
            let mut t = [0u16; 72];
            let mut v = 1046.502261 + (c as f64 - 8.0) * 7.778532836;
            for e in t.iter_mut() {
                *e = v as u16;
                v *= 1.059463094;
            }
            t
        })
        .collect()
}

// ---------------------------------------------------------------- the module

#[derive(Clone, Copy, Default)]
struct Cmd {
    cmd: u8,
    d0: u8,
    d1: u8,
}

impl Cmd {
    fn data(&self) -> u16 {
        self.d0 as u16 * 256 + self.d1 as u16
    }
}

struct Block {
    lines: usize,
    tracks: usize,
    pages: usize,
    /// Note and instrument, `line * tracks + track`.
    notes: Vec<[u8; 2]>,
    /// By page, then as the notes.
    cmds: Vec<Vec<Cmd>>,
}

impl Block {
    fn new(lines: usize, tracks: usize) -> Block {
        Block {
            lines,
            tracks,
            pages: 1,
            notes: vec![[0; 2]; lines * tracks],
            cmds: vec![vec![Cmd::default(); lines * tracks]],
        }
    }
    fn note(&self, line: usize, t: usize) -> [u8; 2] {
        self.notes[line * self.tracks + t]
    }
    fn cmd(&self, line: usize, t: usize, page: usize) -> Cmd {
        self.cmds[page][line * self.tracks + t]
    }
}

#[derive(Clone, Copy)]
struct PEnt {
    value: u32,
    cmd: u8,
}

#[derive(Clone, Copy)]
struct Tempo {
    tempo: u16,
    tpl: u16,
    lpb: u16,
    bpm: bool,
    /// A 5-8 channel song: its tempo 1-10 (held as the BPM value the load made of it)
    /// sets the original's CIA timer from a table, not a BPM rate.
    eight: bool,
}

impl Tempo {
    /// The original's timer period in colour clocks for a 5-8 channel song: the Eagleplayer
    /// loads the CIA latch with 90 * (100 + 10 t) for the song's tempo t (1-10, more is 10),
    /// and the timer runs latch + 1 E-clock ticks of 5 colour clocks.
    fn eight_clocks(&self) -> Option<f64> {
        if !self.eight {
            return None;
        }
        let t = if self.tempo == 99 { 10 } else { BPM_VALS.iter().position(|&v| v as u16 == self.tempo).map_or(10, |i| i + 1) };
        Some(5.0 * (90.0 * (100.0 + 10.0 * t as f64) + 1.0))
    }

    /// The player's rate in Hz (NostalgicPlayer's SetMixTempo).
    fn hz(&self) -> f64 {
        if self.bpm {
            (self.tempo as f32 * self.lpb as f32 / 10.0) as f64
        } else {
            let t = if self.tempo <= 10 { BPM_COMP[(self.tempo.max(1) - 1) as usize] } else { self.tempo };
            (1.0f32 / (0.474326f32 / t as f32 * 1.3968255f32)) as f64
        }
    }
}

struct SubSong {
    blocks: Vec<Block>,
    sections: Vec<u16>,
    pseqs: Vec<Vec<PEnt>>,
    tempo: Tempo,
    channels: usize,
    play_transp: i16,
    slide1st: bool,
    /// The LED filter the player's start sets: only SoundStudio's player sets it, and
    /// only for a song in mix mode (flags2 bit 7); then on when the filter flag is set.
    led: Option<bool>,
    track_vol: [u8; MAX_TRACKS],
    track_pan: [i8; MAX_TRACKS],
    master_vol: i32,
    /// The global echo: crossed, depth (the shift of each repeat's level) and
    /// length in ms.
    echo: Option<(bool, u8, u32)>,
    /// SoundStudio's mixer settings: the volume adjust (percent), flags3 (bit 0 stereo,
    /// bit 1 free panning) and the stereo separation (-4..4).
    vol_adj: u16,
    flags3: u8,
    sep: i8,
    name: String,
}

#[derive(Clone, Default)]
struct Instr {
    flags: u8,
    init_vol: u32,
    rep: u32,
    replen: u32,
    transp: i16,
    fine_tune: i16,
    hold: u16,
    decay: u16,
    midi_ch: u32,
    def_freq: u32,
    /// The expansion data's output device byte: SoundStudio's mixer plays an instrument whose
    /// byte is not 0 a semitone up (measured with 1 and 2 on seven notes: the next note's step).
    outdev: u8,
}

impl Instr {
    fn kill_loop(&mut self) {
        self.rep = 0;
        self.replen = 0;
        self.flags &= I_LOOP;
    }
}

/// One waveform of a synth sound: its bytes in guest memory and as read.
struct Wf {
    addr: u32,
    data: Vec<i8>,
}

struct SynSnd {
    vol_speed: u32,
    wf_speed: u32,
    vol_tbl: [u8; 128],
    wf_tbl: [u8; 128],
    wfs: Vec<Wf>,
}

/// A sample (or a hybrid sound's), converted to 8-bit mono in guest memory.
#[derive(Default)]
struct Smp {
    frames: u32,
    stype: usize,
    /// Per octave buffer: address and frames.
    octs: Vec<(u32, u32)>,
    /// Per octave buffer, the ping-pong form of the instrument's loop: the
    /// buffer, its frames and the loop's start and length.
    pp: Vec<Option<(u32, u32, u32, u32)>>,
    syn: Option<Box<SynSnd>>,
    /// A 16-bit or stereo sample's own data, per octave buffer (SoundStudio's mixer plays it).
    wide: Vec<WideData>,
}

/// The data of a 16-bit or stereo sample as the file has it: 16-bit or not, the left (or only)
/// channel and the right.
type WideData = (bool, Vec<i16>, Option<Vec<i16>>);

impl Smp {
    fn multi(&self) -> bool {
        OCT_COUNT[self.stype] > 1
    }
    fn octave_len(&self) -> u32 {
        let n = OCT_COUNT[self.stype];
        if n <= 1 { self.frames } else { self.frames / ((1u32 << n) - 1) }
    }
    fn note_diff(&self, note: u8) -> i32 {
        if !self.multi() {
            return 0;
        }
        OCT_START[self.stype - 1][((note / 12) as usize).min(5)]
    }
}

struct Song {
    instr: Vec<Instr>,
    samp: Vec<Option<Smp>>,
    subs: Vec<SubSong>,
    title: String,
    /// A 5-8 channel song (the song flag), which the OctaMED Eagleplayer plays on the amplifier.
    eight: bool,
}

fn kind_of(d: &[u8]) -> Option<Kind> {
    if d.len() < 840 || &d[0..3] != b"MMD" {
        return None;
    }
    let song = be32(d, 8) as usize;
    if song == 0 || song + 0x314 > d.len() {
        return None;
    }
    match d[3] {
        b'3' => Some(Kind::Oss),
        b'0'..=b'2' if d[song + 0x2ff] & FLAG_8CH != 0 || d[song + 0x300] & FLAG2_MIX != 0 => Some(Kind::Old),
        _ => None,
    }
}

/// An MMD sample's header at `pos`, decoded to the octave buffers' bytes.
struct Dec {
    frames: u32,
    stype: usize,
    bufs: Vec<Vec<i8>>,
    /// The file bytes the sample takes from its data, and whether it is the
    /// bytes in the file unchanged.
    raw_len: usize,
    plain: bool,
    wide: Vec<WideData>,
}

fn decode(d: &[u8], pos: usize) -> Option<Dec> {
    let nbytes = be32(d, pos);
    let ty = be16(d, pos + 4) as i16;
    if ty < 0 || ty & 0xf > 7 || ty & 0x80 != 0 {
        return None;
    }
    let b16 = ty & 0x10 != 0;
    let stereo = ty & 0x20 != 0;
    let delta = ty & 0x40 != 0;
    let frames = if b16 { nbytes / 2 } else { nbytes };
    let stype = if ty > 6 { 0 } else { (ty & 0xf) as usize };
    let noct = OCT_COUNT[stype];
    let dl = if b16 { 2u32 } else { 1 };
    let total = dl as u64 * frames as u64;
    if frames == 0 {
        return Some(Dec { frames, stype, bufs: Vec::new(), raw_len: 0, plain: true, wide: Vec::new() });
    }
    let sizes: Vec<u64> = if noct == 1 {
        vec![total]
    } else {
        let base = total / ((1u64 << noct) - 1);
        (0..noct).map(|i| base << i).collect()
    };
    let chans = if stereo { 2 } else { 1 };
    let raw_len = chans * sizes.iter().sum::<u64>() as usize;
    if raw_len > d.len() {
        return None;
    }
    let mut p = pos + 6;
    let mut acc: Vec<Vec<i32>> = sizes.iter().map(|s| vec![0i32; (*s / dl as u64) as usize]).collect();
    let mut wide: Vec<WideData> = Vec::new();
    for c in 0..chans {
        for (o, s) in sizes.iter().enumerate() {
            let fr = (*s / dl as u64) as usize;
            if b16 || stereo {
                let v: Vec<i16> = if b16 {
                    let mut prev = 0i16;
                    (0..fr)
                        .map(|i| {
                            let v = be16(d, p + 2 * i) as i16;
                            prev = if delta && i > 0 { prev.wrapping_add(v) } else { v };
                            prev
                        })
                        .collect()
                } else {
                    let mut prev = 0i8;
                    (0..fr)
                        .map(|i| {
                            let v = d.get(p + i).copied().unwrap_or(0) as i8;
                            prev = if delta && i > 0 { prev.wrapping_add(v) } else { v };
                            prev as i16
                        })
                        .collect()
                };
                if c == 0 {
                    wide.push((b16, v, None));
                } else if let Some(w) = wide.get_mut(o) {
                    w.2 = Some(v);
                }
            }
            let buf = &mut acc[o];
            if b16 {
                let mut prev = 0i16;
                for i in 0..fr {
                    let v = be16(d, p) as i16;
                    p += 2;
                    prev = if delta && i > 0 { prev.wrapping_add(v) } else { v };
                    buf[i] += (prev >> 8) as i32;
                }
            } else {
                let mut prev = 0i8;
                for i in 0..fr {
                    let v = d.get(p).copied().unwrap_or(0) as i8;
                    p += 1;
                    prev = if delta && i > 0 { prev.wrapping_add(v) } else { v };
                    buf[i] += prev as i32;
                }
            }
        }
    }
    let bufs = acc.into_iter().map(|b| b.into_iter().map(|v| (v / chans as i32) as i8).collect()).collect();
    Some(Dec { frames, stype, bufs, raw_len, plain: !b16 && !stereo && !delta, wide })
}

fn r8(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// Where a decoded sample goes in guest memory: its own place in the module if
/// no other sample's data lies there, else new memory.
fn place(
    dec: &Dec,
    pos: usize,
    base: u32,
    others: &[(usize, usize)],
    m: &mut Machine,
) -> Option<Vec<(u32, u32)>> {
    let mono: usize = dec.bufs.iter().map(|b| b.len()).sum();
    let at = pos + 6;
    let mut addr;
    if dec.plain {
        addr = base + at as u32;
    } else {
        let clash = others.iter().any(|&(s, l)| s < at + dec.raw_len.max(mono) && at < s + l);
        if !clash {
            addr = base + at as u32;
        } else {
            addr = m.alloc(mono as u32 + 2);
            if addr == 0 {
                return None;
            }
        }
        let mut a = addr;
        for b in &dec.bufs {
            let bytes: Vec<u8> = b.iter().map(|&v| v as u8).collect();
            m.bus.write_bytes(a, &bytes);
            a += b.len() as u32;
        }
    }
    let mut out = Vec::new();
    for b in &dec.bufs {
        out.push((addr, b.len() as u32));
        addr += b.len() as u32;
    }
    Some(out)
}

struct Hdr {
    song: usize,
    blocks: usize,
    samples: usize,
    exp: usize,
    extra: usize,
}

fn read_hdr(d: &[u8], o: usize) -> Hdr {
    Hdr {
        song: be32(d, o + 8) as usize,
        blocks: be32(d, o + 0x10) as usize,
        samples: be32(d, o + 0x18) as usize,
        exp: be32(d, o + 0x20) as usize,
        extra: r8(d, o + 0x33) as usize,
    }
}

/// The module's tags (`TagsLoad`): (tag, value) up to the end tag.
fn tags(d: &[u8], mut p: usize) -> Vec<(u32, u32)> {
    let mut t = Vec::new();
    for _ in 0..64 {
        let tag = be32(d, p);
        if tag == 0 || p + 8 > d.len() {
            break;
        }
        t.push((tag, be32(d, p + 4)));
        p += 8;
    }
    t
}

fn load(d: &[u8], base: u32, m: &mut Machine, kind: Kind) -> Option<Song> {
    let freqs = freq_table();
    let mark_version = d[3];
    let mut instr = vec![Instr::default(); MAX_INSTR];
    let mut subs: Vec<SubSong> = Vec::new();
    let mut title = String::new();
    let mut mix_conv = false;
    let mut eight_conv = false;
    let mut num_samples;
    let mut hpos = 0usize;
    let hdr0 = read_hdr(d, 0);
    let mut extra = hdr0.extra as i32;
    let samples_off = hdr0.samples;
    loop {
        let h = if hpos == 0 { read_hdr(d, 0) } else { read_hdr(d, hpos) };
        if h.song == 0 || h.song + 0x314 > d.len() {
            return None;
        }
        let s = h.song;
        let mut ss = SubSong {
            blocks: Vec::new(),
            sections: Vec::new(),
            pseqs: Vec::new(),
            tempo: Tempo { tempo: 125, tpl: 6, lpb: 4, bpm: true, eight: false },
            channels: 4,
            play_transp: 0,
            slide1st: false,
            led: (r8(d, s + 0x300) & 0x80 != 0).then_some(r8(d, s + 0x2ff) & 1 != 0),
            track_vol: [64; MAX_TRACKS],
            track_pan: [0; MAX_TRACKS],
            master_vol: 64,
            echo: None,
            vol_adj: 100,
            flags3: 0,
            sep: 0,
            name: String::new(),
        };
        // the instruments of the song structure: the first song's (a later song's header has
        // its own copy, without the first's expansion data: its loops are the short fields',
        // an even number of frames)
        for i in 0..if subs.is_empty() { MAX_INSTR - 1 } else { 0 } {
            let o = s + 8 * i;
            let ci = &mut instr[i];
            ci.rep = (be16(d, o) as u32) << 1;
            ci.replen = (be16(d, o + 2) as u32) << 1;
            ci.transp = r8(d, o + 7) as i8 as i16;
            ci.init_vol = (r8(d, o + 6) as u32 * 2).min(128);
            ci.midi_ch = r8(d, o + 4) as u32;
        }
        let nblocks = be16(d, s + 0x1f8) as usize;
        let defs = be16(d, s + 0x2fc);
        let flags = r8(d, s + 0x2ff);
        let flags2 = r8(d, s + 0x300);
        let tempo2 = r8(d, s + 0x301);
        let mut song_tracks = 0usize;
        let uses_hex = flags & FLAG_VOLHEX != 0;
        if mark_version < b'2' {
            let songlen = be16(d, s + 0x1fa) as usize;
            if songlen > 256 {
                return None;
            }
            num_samples = (r8(d, s + 0x313) as usize).min(MAX_INSTR);
            if flags2 & FLAG2_MIX == 0 {
                mix_conv = true;
            }
            if flags & FLAG_8CH != 0 {
                eight_conv = true;
            }
            ss.slide1st = flags & FLAG_STSLIDE != 0;
            ss.tempo.tempo = defs.clamp(1, 240);
            ss.tempo.tpl = if tempo2 == 0 { 6 } else { (tempo2 as u16).min(32) };
            ss.tempo.lpb = ((flags2 & 0x1f) as u16 + 1).min(32);
            ss.tempo.bpm = flags2 & FLAG2_BPM != 0;
            ss.play_transp = r8(d, s + 0x2fe) as i8 as i16;
            ss.channels = 4;
            for t in 0..16 {
                ss.track_vol[t] = r8(d, s + 0x302 + t);
            }
            ss.master_vol = (r8(d, s + 0x312) as i8 as i32).clamp(1, 64);
            ss.sections.push(0);
            ss.pseqs.push((0..songlen).map(|i| PEnt { value: r8(d, s + 0x1fc + i) as u32, cmd: 0 }).collect());
        } else {
            num_samples = (r8(d, s + 0x313) as usize).min(MAX_INSTR);
            if flags2 & FLAG2_MIX == 0 {
                mix_conv = true;
            }
            if flags & FLAG_8CH != 0 {
                eight_conv = true;
            }
            ss.slide1st = flags & FLAG_STSLIDE != 0;
            ss.tempo.tempo = defs.clamp(1, 240);
            ss.tempo.tpl = if tempo2 == 0 { 6 } else { (tempo2 as u16).min(32) };
            ss.tempo.lpb = ((flags2 & 0x1f) as u16 + 1).min(32);
            ss.tempo.bpm = flags2 & FLAG2_BPM != 0;
            ss.play_transp = r8(d, s + 0x2fe) as i8 as i16;
            let ch = be16(d, s + 0x216) as usize;
            ss.channels = if ch != 0 { ch } else { 4 }.clamp(1, MAX_TRACKS);
            song_tracks = (be16(d, s + 0x208) as usize).min(MAX_TRACKS);
            let (tv, tp) = (be32(d, s + 0x204) as usize, be32(d, s + 0x20c) as usize);
            for t in 0..song_tracks {
                ss.track_vol[t] = r8(d, tv + t);
                ss.track_pan[t] = r8(d, tp + t) as i8;
            }
            ss.master_vol = (r8(d, s + 0x312) as i8 as i32).clamp(1, 64);
            ss.flags3 = r8(d, s + 0x213);
            ss.vol_adj = be16(d, s + 0x214);
            ss.sep = (r8(d, s + 0x21c) as i8).clamp(-4, 4);
            let (et, ed, el) = (r8(d, s + 0x218), r8(d, s + 0x219), be16(d, s + 0x21a) as u32);
            if et != 0 && (1..=6).contains(&ed) && el > 0 {
                ss.echo = Some((et == 2, ed, el));
            }
            let (sect, nsect) = (be32(d, s + 0x200) as usize, be16(d, s + 0x1fa) as usize);
            for i in 0..nsect {
                ss.sections.push(be16(d, sect + 2 * i));
            }
            let (pst, npseq) = (be32(d, s + 0x1fc) as usize, be16(d, s + 0x20a) as usize);
            for i in 0..npseq {
                let ps = be32(d, pst + 4 * i) as usize;
                let cmd_ptr = be32(d, ps + 0x20) as usize;
                let seq_len = be16(d, ps + 0x28) as usize;
                let mut seq = Vec::new();
                for k in 0..seq_len {
                    let n = be16(d, ps + 0x2a + 2 * k);
                    if n < 0x8000 {
                        seq.push(PEnt { value: n as u32, cmd: 0 });
                    }
                }
                if cmd_ptr != 0 {
                    let mut p = cmd_ptr;
                    for _ in 0..4096 {
                        let offs = be16(d, p);
                        let cn = r8(d, p + 2);
                        let eb = r8(d, p + 3) as usize;
                        p += 4;
                        if offs == 0xffff && cn == 0 && eb == 0 {
                            break;
                        }
                        let e = seq.get_mut(offs as usize);
                        match cn {
                            PS_STOP => {
                                if let Some(e) = e {
                                    *e = PEnt { value: 0, cmd: PS_STOP };
                                }
                                p += eb;
                            }
                            PS_POSJUMP => {
                                if let Some(e) = e {
                                    *e = PEnt { value: be16(d, p) as u32, cmd: PS_POSJUMP };
                                }
                                p += eb;
                            }
                            _ => p += eb,
                        }
                    }
                }
                ss.pseqs.push(seq);
            }
        }
        // the blocks
        for b in 0..nblocks.min(0x4000) {
            let bo = be32(d, h.blocks + 4 * b) as usize;
            let blk = if mark_version == b'0' {
                let tracks = r8(d, bo) as usize;
                let lines = r8(d, bo + 1) as usize + 1;
                if bo + 2 + tracks * lines * 3 > d.len() {
                    return None;
                }
                let mut blk = Block::new(lines, tracks);
                for l in 0..lines {
                    for t in 0..tracks {
                        let o = bo + 2 + (l * tracks + t) * 3;
                        let (b0, b1, b2) = (d[o], d[o + 1], d[o + 2]);
                        let inst = (b1 >> 4) | if b0 & 0x80 != 0 { 0x10 } else { 0 } | if b0 & 0x40 != 0 { 0x20 } else { 0 };
                        blk.notes[l * tracks + t] = [b0 & 0x3f, inst];
                        blk.cmds[0][l * tracks + t] = if b1 & 0xf == 0 {
                            Cmd { cmd: 0, d0: 0, d1: b2 }
                        } else {
                            Cmd { cmd: b1 & 0xf, d0: b2, d1: 0 }
                        };
                    }
                }
                blk
            } else {
                let tracks_all = be16(d, bo) as usize;
                let lines = be16(d, bo + 2) as usize + 1;
                let info = be32(d, bo + 4) as usize;
                let tracks = tracks_all.min(MAX_TRACKS);
                if bo + 8 + tracks_all * lines * 4 > d.len() {
                    return None;
                }
                let mut blk = Block::new(lines, tracks);
                for l in 0..lines {
                    for t in 0..tracks {
                        let o = bo + 8 + (l * tracks_all + t) * 4;
                        let (n, i, c, a) = (d[o], d[o + 1], d[o + 2], d[o + 3]);
                        blk.notes[l * tracks + t] = [if n <= NOTE_44K { n } else { 0 }, i];
                        blk.cmds[0][l * tracks + t] =
                            if c == 0x19 || c == 0 { Cmd { cmd: c, d0: 0, d1: a } } else { Cmd { cmd: c, d0: a, d1: 0 } };
                    }
                }
                if info != 0 {
                    let page_table = be32(d, info + 0xc) as usize;
                    let cmd_ext = be32(d, info + 0x10) as usize;
                    if page_table != 0 {
                        let np = be16(d, page_table) as usize;
                        blk.pages = np + 1;
                        for p in 0..np {
                            let po = be32(d, page_table + 4 + 4 * p) as usize;
                            let mut page = vec![Cmd::default(); lines * tracks];
                            for l in 0..lines {
                                for t in 0..tracks {
                                    let o = po + 2 * (l * tracks_all + t);
                                    let (c, a) = (r8(d, o), r8(d, o + 1));
                                    page[l * tracks + t] =
                                        if c == 0x19 || c == 0 { Cmd { cmd: c, d0: 0, d1: a } } else { Cmd { cmd: c, d0: a, d1: 0 } };
                                }
                            }
                            blk.cmds.push(page);
                        }
                    }
                    if cmd_ext != 0 {
                        for p in 0..blk.pages {
                            let po = be32(d, cmd_ext + 4 * p) as usize;
                            for l in 0..lines {
                                for t in 0..tracks {
                                    let a2 = r8(d, po + l * tracks_all + t);
                                    let c = &mut blk.cmds[p][l * tracks + t];
                                    if c.cmd == 0 || c.cmd == 0x19 {
                                        c.d0 = a2;
                                    } else {
                                        c.d1 = a2;
                                    }
                                }
                            }
                        }
                    }
                }
                // the old volume command to the new
                for page in blk.cmds.iter_mut() {
                    for c in page.iter_mut() {
                        if c.cmd == 0x0c {
                            if c.d1 != 0 {
                                c.d0 = c.d1;
                                c.d1 = 0;
                            } else if !uses_hex && c.d0 != 0 && c.d0 < 128 {
                                c.d0 = (c.d0 >> 4) * 10 + (c.d0 & 0xf);
                                c.d1 = 0;
                            }
                        }
                    }
                }
                blk
            };
            if blk.tracks > 0 {
                ss.blocks.push(blk);
            } else {
                ss.blocks.push(Block::new(blk.lines, 1));
            }
        }
        // the expansion data
        let mut has_loop = false;
        let mut next = 0usize;
        if h.exp != 0 {
            let e = h.exp;
            next = be32(d, e) as usize;
            let (it_off, it_n, it_sz) = (be32(d, e + 4) as usize, be16(d, e + 8) as usize, be16(d, e + 10) as usize);
            for i in 0..if subs.is_empty() { it_n.min(MAX_INSTR) } else { 0 } {
                let o = it_off + i * it_sz;
                let ci = &mut instr[i];
                if it_sz >= 2 {
                    ci.hold = (r8(d, o) as u16).min(128);
                    ci.decay = (r8(d, o + 1) as u16).min(128);
                }
                if it_sz >= 4 {
                    ci.fine_tune = (r8(d, o + 3) as i8 as i16).clamp(-8, 7);
                }
                if it_sz >= 8 {
                    let (dp, fl) = (r8(d, o + 4), r8(d, o + 5));
                    ci.def_freq = if dp == 0 { 0 } else { freqs[(ci.fine_tune + 8) as usize][((dp - 1) as usize).min(71)] as u32 };
                    ci.flags |= fl & 2 | fl & I_DISABLED;
                    if fl & 1 != 0 {
                        ci.flags |= I_LOOP;
                        if fl & I_PINGPONG != 0 {
                            ci.flags |= I_PINGPONG;
                        }
                    }
                    has_loop = true;
                } else if ci.replen > 2 {
                    ci.flags = I_LOOP;
                }
                if it_sz >= 9 {
                    ci.outdev = r8(d, o + 8);
                }
                if it_sz >= 18 {
                    ci.rep = be32(d, o + 10);
                    ci.replen = be32(d, o + 14);
                }
                if it_sz >= 19 {
                    ci.init_vol = (r8(d, o + 18) as u32).min(128);
                }
            }
            // the effect groups: with more than one the global echo is not mixed; with
            // one, its tags replace the song's settings
            let tg = tags(d, e + 0x4c);
            let ngroups = tg.iter().find(|t| t.0 == 1).map_or(1, |t| t.1);
            let fx = be32(d, e + 0x48) as usize;
            if mark_version >= b'2' {
                if ngroups > 1 {
                    ss.echo = None;
                } else if fx != 0 && be32(d, fx) != 0 {
                    let g = tags(d, be32(d, fx) as usize);
                    let v = |k: u32, def: u32| g.iter().find(|t| t.0 == k).map_or(def, |t| t.1);
                    let (et, el, ed) = (v(1, 0), v(2, 300), v(3, 2));
                    ss.echo = if et != 0 && el > 0 && ed > 0 { Some((et == 2, ed.min(6) as u8, el)) } else { None };
                }
            }
            let nm = be32(d, e + 0x2c) as usize;
            let nl = be32(d, e + 0x30) as usize;
            if nm != 0 && nl > 0 && nl < 256 && nm + nl <= d.len() {
                let n: Vec<u8> = d[nm..nm + nl].iter().copied().take_while(|&c| c != 0).collect();
                ss.name = String::from_utf8_lossy(&n).trim().to_string();
            }
            if eight_conv {
                for k in 0..4 {
                    if r8(d, e + 0x24 + k) == 0 {
                        ss.channels = (ss.channels + 1).min(MAX_TRACKS);
                    }
                }
            }
            let _ = song_tracks;
        }
        if !has_loop && subs.is_empty() {
            for ci in instr.iter_mut() {
                if ci.replen > 2 {
                    ci.flags = I_LOOP;
                }
            }
        }
        if title.is_empty() {
            title = ss.name.clone();
        }
        subs.push(ss);
        if extra == 0 || next == 0 {
            break;
        }
        extra -= 1;
        hpos = next;
    }
    // a song with nothing in it plays silence
    for s in subs.iter_mut() {
        if s.blocks.is_empty() {
            s.blocks.push(Block::new(64, 4));
        }
        if s.sections.is_empty() {
            s.sections.push(0);
        }
        if s.pseqs.is_empty() {
            s.pseqs.push(vec![PEnt { value: 0, cmd: 0 }]);
        }
    }
    // samples and synth sounds
    let mut samp: Vec<Option<Smp>> = (0..MAX_INSTR).map(|_| None).collect();
    if samples_off != 0 && num_samples != 0 {
        let ptrs: Vec<usize> = (0..num_samples).map(|i| be32(d, samples_off + 4 * i) as usize).collect();
        // the data regions of the samples (to see which may be converted in place)
        let mut regions: Vec<(usize, usize)> = Vec::new();
        let mut targets: Vec<(usize, usize)> = Vec::new(); // (slot, header position)
        for (i, &w) in ptrs.iter().enumerate() {
            if w == 0 || w + 6 > d.len() {
                continue;
            }
            let ty = be16(d, w + 4) as i16;
            if ty >= 0 {
                if let Some(dec) = decode(d, w) {
                    regions.push((w + 6, dec.raw_len));
                }
                targets.push((i, w));
            } else if ty == -2 {
                // a hybrid sound's sample is its first waveform
                let wf0 = w + be32(d, w + 0x116) as usize;
                if wf0 + 6 <= d.len() {
                    if let Some(dec) = decode(d, wf0) {
                        regions.push((wf0 + 6, dec.raw_len));
                    }
                }
                targets.push((i, w));
            } else if ty == -1 {
                targets.push((i, w));
            }
        }
        for (i, w) in targets {
            let ty = be16(d, w + 4) as i16;
            let mut smp = Smp::default();
            let mut ok = false;
            if ty >= 0 {
                if let Some(dec) = decode(d, w) {
                    let mut others = regions.clone();
                    if let Some(k) = others.iter().position(|&r| r == (w + 6, dec.raw_len)) {
                        others.remove(k);
                    }
                    if let Some(octs) = place(&dec, w, base, &others, m) {
                        smp.frames = dec.frames;
                        smp.stype = dec.stype;
                        smp.octs = octs;
                        smp.wide = dec.wide;
                        ok = true;
                    }
                }
            } else {
                // a synth or hybrid sound
                let (vl, wl, nw) = (be16(d, w + 6 + 8) as usize, be16(d, w + 6 + 10) as usize, be16(d, w + 6 + 14) as usize);
                if vl <= 128 && wl <= 128 && nw <= 256 {
                    let mut syn = SynSnd {
                        vol_speed: r8(d, w + 6 + 12) as u32,
                        wf_speed: r8(d, w + 6 + 13) as u32,
                        vol_tbl: [0; 128],
                        wf_tbl: [0; 128],
                        wfs: Vec::new(),
                    };
                    for k in 0..128 {
                        syn.vol_tbl[k] = r8(d, w + 0x16 + k);
                        syn.wf_tbl[k] = r8(d, w + 0x96 + k);
                    }
                    let mut good = true;
                    for k in 0..nw {
                        let p = w + be32(d, w + 0x116 + 4 * k) as usize;
                        if k == 0 && ty == -2 {
                            if let Some(dec) = decode(d, p) {
                                let mut others = regions.clone();
                                if let Some(k) = others.iter().position(|&r| r == (p + 6, dec.raw_len)) {
                                    others.remove(k);
                                }
                                match place(&dec, p, base, &others, m) {
                                    Some(octs) => {
                                        smp.frames = dec.frames;
                                        smp.stype = dec.stype;
                                        smp.octs = octs;
                                    }
                                    None => good = false,
                                }
                            }
                            syn.wfs.push(Wf { addr: 0, data: Vec::new() });
                        } else {
                            let len = be16(d, p) as usize * 2;
                            let data: Vec<i8> = (0..len).map(|j| r8(d, p + 2 + j) as i8).collect();
                            syn.wfs.push(Wf { addr: base + p as u32 + 2, data });
                        }
                    }
                    if good {
                        smp.syn = Some(Box::new(syn));
                        ok = true;
                    }
                }
            }
            if ok {
                // (SoundStudio's mixer plays a synth or hybrid sound at its instrument's volume:
                // measured, three sounds of a module at two volumes each)
                if smp.syn.is_some() && kind != Kind::Oss {
                    instr[i].init_vol = 128;
                }
                samp[i] = Some(smp);
            }
        }
    }
    // the loops, against the samples
    for i in 0..MAX_INSTR {
        if let Some(s) = &samp[i] {
            let ci = &mut instr[i];
            let len = s.octave_len();
            if len == 0 {
                ci.kill_loop();
            } else {
                if ci.rep >= len {
                    ci.rep = len - 1;
                }
                if ci.rep + ci.replen > len {
                    ci.replen = len - ci.rep;
                }
            }
        }
    }
    // the ping-pong loops, unrolled
    for i in 0..MAX_INSTR {
        let ci = instr[i].clone();
        if ci.flags & (I_LOOP | I_PINGPONG) != (I_LOOP | I_PINGPONG) || ci.replen <= 2 {
            continue;
        }
        let Some(s) = samp[i].as_mut() else { continue };
        let n = s.octs.len();
        s.pp = vec![None; n];
        for o in 0..n {
            let (addr, frames) = s.octs[o];
            let sh = if s.stype == 0 { 0 } else { o };
            // the loop of the buffer `o`, scaled as `play` scales it for the octave it serves
            let (ls, ll) = (ci.rep << sh, ci.replen << sh);
            if ll <= 2 || ls + ll > frames {
                continue;
            }
            let le = ls + ll;
            let mut data: Vec<u8> = (0..le).map(|k| m.bus.r8(addr + k)).collect();
            for k in (ls + 1..le - 1).rev() {
                data.push(m.bus.r8(addr + k));
            }
            let a = m.alloc(data.len() as u32 + 2);
            if a == 0 {
                continue;
            }
            m.bus.write_bytes(a, &data);
            s.pp[o] = Some((a, data.len() as u32, ls, 2 * ll - 2));
        }
    }
    // the old modules' conversion to mix mode, and the 5-8 channel tempo
    if mix_conv {
        convert_to_mix_mode(&mut subs, &instr, &samp, mark_version == b'0');
    }
    if eight_conv {
        for ss in subs.iter_mut() {
            for blk in ss.blocks.iter_mut() {
                for page in blk.cmds.iter_mut() {
                    for c in page.iter_mut() {
                        if c.cmd == 0x0f && c.d0 != 0 && c.d0 <= 240 {
                            c.d0 = if c.d0 >= 10 { 99 } else { BPM_VALS[c.d0 as usize - 1] };
                        }
                    }
                }
            }
            ss.tempo.bpm = true;
            ss.tempo.eight = true;
            ss.tempo.lpb = 4;
            let old = ss.tempo.tempo;
            ss.tempo.tempo = if old >= 10 { 99 } else { BPM_VALS[(old.max(1) - 1) as usize] as u16 };
        }
        // the 5-8 channel player starts every note at full volume: the instrument's own
        // volume is not used (measured: a module's instrument volumes changed to 20 and 50
        // play at 64; a note without command 0C after one with it plays at 64 again)
        for ci in instr.iter_mut() {
            ci.init_vol = 128;
        }
    }
    let _ = kind;
    Some(Song { instr, samp, subs, title, eight: eight_conv })
}

/// NostalgicPlayer's ScanSongConvertToMixMode: notes up two octaves for the
/// instruments an old song plays, and the Amiga's panning.
fn convert_to_mix_mode(subs: &mut [SubSong], instr: &[Instr], samp: &[Option<Smp>], type0: bool) {
    let mut transp = [true; MAX_INSTR + 1];
    let mut itrans = [0i32; MAX_INSTR + 1];
    let mut midi = [false; MAX_INSTR + 1];
    let mut multi = [false; MAX_INSTR + 1];
    for c in 1..=MAX_INSTR {
        if let Some(s) = &samp[c - 1] {
            transp[c] = s.syn.is_none() || s.frames != 0;
            multi[c] = s.multi();
        }
        itrans[c] = instr[c - 1].transp as i32;
        midi[c] = instr[c - 1].midi_ch != 0;
    }
    const PAN: [i8; 8] = [-16, 16, 16, -16, -16, 16, 16, -16];
    for ss in subs.iter_mut() {
        let pt = ss.play_transp as i32;
        for (t, p) in PAN.iter().enumerate() {
            ss.track_pan[t] = *p;
        }
        for blk in ss.blocks.iter_mut() {
            for t in 0..blk.tracks {
                let mut last = 0usize;
                for l in 0..blk.lines {
                    let nt = &mut blk.notes[l * blk.tracks + t];
                    if nt[1] != 0 {
                        last = (nt[1] as usize).min(MAX_INSTR);
                    }
                    let mut n = nt[0];
                    if n != 0 && n <= 0x7f - 24 && transp[last] {
                        if midi[last] {
                            if type0 {
                                n += 24;
                            }
                        } else {
                            if !multi[last] {
                                while (n as i8 as i32) + itrans[last] > 36 {
                                    n = n.wrapping_sub(12);
                                }
                            }
                            while (n as i8 as i32) + itrans[last] + pt < 1 && n <= 0x7f - 24 - 12 {
                                n += 12;
                            }
                            n = n.wrapping_add(24);
                        }
                    }
                    nt[0] = n;
                }
            }
        }
    }
}

// ----------------------------------------------------------------- the player

#[derive(Clone, Default)]
struct SynthData {
    /// 0 none, 1 synth, 2 hybrid.
    ty: u8,
    period_change: i32,
    vib_offs: u32,
    vib_speed: u32,
    vib_dep: i32,
    vib_wf_num: u32,
    arp_start: u32,
    arp_offs: u32,
    vol_cmd_pos: u32,
    wf_cmd_pos: u32,
    vol_wait: u32,
    wf_wait: u32,
    vol_chg_speed: i32,
    wf_chg_speed: i32,
    vol_x_speed: u32,
    wf_x_speed: u32,
    vol_x_cnt: i32,
    wf_x_cnt: i32,
    env_wf_num: u32,
    env_loop: bool,
    env_count: u16,
    vol: i32,
    note_number: u8,
}

const M_BACKWARDS: u8 = 1;
const M_NO_WF_RESET: u8 = 2;
const M_STOP: u8 = 4;

#[derive(Clone, Default)]
struct Track {
    prev_note: u8,
    prev_inum: usize,
    prev_vol: u8,
    misc: u8,
    note_off_cnt: i32,
    init_hold: u32,
    init_decay: u32,
    decay: u32,
    fade_speed: u32,
    stransp: i32,
    fine_tune: i32,
    arp_adjust: i32,
    vibr_adjust: i32,
    soffset: u32,
    curr_note: u8,
    no_play: bool,
    frequency: i32,
    port_target: i32,
    port_speed: u16,
    vib_shift: u8,
    vib_speed: u8,
    vib_size: u8,
    temp_vol: u8,
    vib_offs: u16,
    last_note_midi: bool,
    sy: SynthData,
}

/// A voice of the amplifier as the player drives it.
#[derive(Clone, Default)]
struct Chan {
    on: bool,
    per: u16,
    vol: u16,
    /// The block playing: its start, frames, and the loop (frames, 0: none).
    buf: (u32, u32),
    lp: (u32, u32),
    /// Frames played since the start, estimated from the periods.
    pos: f64,
    start_syn: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Brk {
    Normal,
    PatternBreak,
    Loop,
    PositionJump,
}

#[derive(Clone, Default)]
struct Pos {
    block: u32,
    line: i32,
    psq: u32,
    sec: u32,
    seq: u32,
}

fn bound(v: &mut u32, max: usize) {
    if max == 0 {
        *v = 0;
    } else if *v as usize >= max {
        *v = max as u32 - 1;
    }
}

/// The echo, done with voices: each track's pokes repeated later on a voice of
/// the same side (crossed: the other) at the level of one repeat.
struct EchoState {
    cross: bool,
    depth: u8,
    delay: u64,
    base: usize,
    pending: VecDeque<(u64, usize, Op)>,
}

fn apply(hw: &mut Hw, v: usize, op: Op) {
    match op {
        Op::Dma(on) => hw.voice_dma(1 << v, on),
        Op::Loc(a) => hw.voice_loc_byte(v, a),
        Op::LenB(b) => hw.voice_len_bytes(v, b),
        Op::Len(w) => hw.voice_len(v, w),
        Op::Per(p) => hw.voice_per(v, p),
        Op::Vol(x) => hw.voice_vol(v, x),
    }
}

/// The buffer SoundStudio's mixer plays: two halves of this many samples a side.
const SOFT_HALF: usize = 16384;
/// Its Paula period: 24023 Hz as the player counts it (3579545 / 149).
const SOFT_PERIOD: u16 = 149;
/// The mixed samples a second the player's ticks are counted in, and the same as the 16.16
/// number a BPM tempo's tick length comes from (24023 * 65536 + 320: within what eight
/// measured tempos allow, 1574371620..1574371668).
const SOFT_RATE: f64 = 24023.0;
const SOFT_TICK: u64 = 24023 * 65536 + 320;
/// The rate a frequency outside the frequency table is stepped at: f * 65536 / 24063 rounded up
/// is within a few units of the measured steps of finetune 0 (`STEPS`).
const SOFT_STEP_RATE: u64 = 24063;
/// The steps of the frequency table's notes in SoundStudio's mixer, 16.16, by finetune (-8..7)
/// and note: measured on the reference, the positions its output plays of a sample of noise
/// (30000 mixed samples a note; one step fits each). Five were not settled by that: the top
/// note of finetunes 6 and 7 (its lower bound, 2.75 exactly, is taken) and note 67 of finetune
/// 2 and notes 69 and 71 of finetune 6 (the note below, a semitone up).
#[rustfmt::skip]
const STEPS: [[u32; 72]; 16] = [
    [2684, 2842, 3011, 3191, 3382, 3581, 3794, 4018, 4258, 4512, 4779, 5063, 5366, 5685, 6023, 6380, 6760, 7161, 7589, 8039, 8516, 9024, 9561, 10129, 10732, 11370, 12047, 12761, 13522, 14325, 15178, 16079, 17033, 18046, 19120, 20255, 21458, 22735, 24085, 25518, 27034, 28641, 30344, 32150, 34062, 36086, 38233, 40506, 42914, 45468, 48171, 51036, 54069, 57286, 60690, 64300, 68124, 72176, 76467, 81014, 85832, 90936, 96343, 102072, 108142, 114572, 121387, 128603, 136252, 144354, 152937, 162032],
    [2706, 2867, 3036, 3216, 3407, 3611, 3824, 4051, 4291, 4547, 4817, 5104, 5406, 5728, 6069, 6430, 6814, 7218, 7649, 8102, 8585, 9095, 9635, 10208, 10813, 11457, 12139, 12860, 13623, 14434, 15290, 16199, 17162, 18182, 19262, 20408, 21622, 22907, 24268, 25711, 27239, 28860, 30576, 32392, 34318, 36362, 38522, 40814, 43242, 45812, 48537, 51423, 54481, 57720, 61152, 64788, 68640, 72721, 77045, 81626, 86479, 91621, 97069, 102839, 108952, 115432, 122295, 129568, 137272, 145435, 154083, 163246],
    [2725, 2889, 3060, 3240, 3431, 3636, 3852, 4081, 4323, 4583, 4855, 5142, 5450, 5772, 6116, 6481, 6866, 7275, 7706, 8165, 8650, 9166, 9711, 10287, 10901, 11547, 12235, 12963, 13733, 14551, 15416, 16332, 17304, 18332, 19423, 20577, 21802, 23098, 24473, 25927, 27468, 29102, 30832, 32665, 34608, 36665, 38844, 41155, 43602, 46194, 48941, 51852, 54934, 58202, 61664, 65331, 69216, 73330, 77692, 82310, 87204, 92388, 97882, 103704, 109869, 116403, 123324, 130660, 138429, 146660, 155381, 164621],
    [2747, 2910, 3082, 3265, 3459, 3663, 3882, 4113, 4359, 4618, 4891, 5183, 5491, 5818, 6165, 6530, 6918, 7330, 7766, 8227, 8718, 9237, 9785, 10369, 10985, 11637, 12330, 13064, 13839, 14663, 15536, 16458, 17437, 18474, 19573, 20738, 21971, 23278, 24661, 26129, 27681, 29329, 31072, 32919, 34878, 36951, 39147, 41474, 43940, 46554, 49320, 52255, 55363, 58653, 62142, 65836, 69750, 73900, 78295, 82949, 87881, 93108, 98643, 104508, 110723, 117306, 124281, 131672, 139501, 147797, 156587, 165898],
    [2768, 2932, 3107, 3292, 3489, 3696, 3914, 4146, 4392, 4654, 4932, 5224, 5535, 5865, 6211, 6580, 6970, 7384, 7824, 8290, 8784, 9305, 9859, 10445, 11067, 11725, 12423, 13160, 13943, 14772, 15650, 16581, 17565, 18610, 19718, 20888, 22132, 23447, 24841, 26320, 27883, 29539, 31296, 33156, 35126, 37216, 39428, 41774, 44257, 46889, 49677, 52632, 55761, 59078, 62589, 66313, 70255, 74432, 78857, 83546, 88514, 93776, 99352, 105261, 111520, 118149, 125176, 132618, 140505, 148861, 157711, 167090],
    [2790, 2957, 3131, 3317, 3516, 3723, 3944, 4179, 4427, 4689, 4967, 5265, 5576, 5906, 6258, 6629, 7024, 7442, 7884, 8353, 8849, 9376, 9935, 10524, 11152, 11815, 12516, 13261, 14049, 14884, 15768, 16706, 17699, 18752, 19865, 21046, 22296, 23622, 25027, 26516, 28093, 29763, 31533, 33407, 35396, 37499, 39731, 42093, 44595, 47247, 50057, 53033, 56186, 59528, 63069, 66818, 70792, 75002, 79462, 84187, 89193, 94497, 100116, 106069, 112376, 119060, 126139, 133641, 141588, 150007, 158928, 168378],
    [2809, 2979, 3156, 3344, 3541, 3753, 3974, 4209, 4460, 4724, 5005, 5303, 5619, 5955, 6307, 6683, 7082, 7502, 7949, 8421, 8920, 9452, 10014, 10609, 11242, 11910, 12617, 13367, 14164, 15007, 15899, 16842, 17844, 18905, 20032, 21221, 22484, 23821, 25237, 26737, 28328, 30014, 31798, 33688, 35691, 37813, 40064, 42445, 44969, 47642, 50477, 53478, 56658, 60028, 63596, 67380, 71384, 75629, 80128, 84894, 89941, 95288, 100954, 106956, 113315, 120053, 127192, 134757, 142769, 151259, 160253, 169783],
    [2831, 3000, 3178, 3369, 3568, 3781, 4004, 4242, 4493, 4760, 5044, 5344, 5663, 5998, 6356, 6732, 7133, 7559, 8009, 8484, 8988, 9523, 10091, 10691, 11326, 12000, 12712, 13468, 14270, 15118, 16019, 16971, 17980, 19049, 20182, 21382, 22653, 24001, 25428, 26939, 28543, 30240, 32038, 33942, 35961, 38099, 40366, 42764, 45307, 48000, 50856, 53879, 57084, 60478, 64073, 67884, 71919, 76197, 80728, 85529, 90614, 96002, 101712, 107760, 114168, 120956, 128150, 135769, 143844, 152397, 161459, 171059],
    [2853, 3022, 3202, 3393, 3595, 3808, 4034, 4272, 4525, 4793, 5079, 5382, 5701, 6039, 6400, 6779, 7180, 7608, 8061, 8538, 9048, 9586, 10156, 10759, 11400, 12077, 12797, 13558, 14363, 15217, 16122, 17080, 18097, 19172, 20313, 21521, 22801, 24156, 25594, 27116, 28729, 30436, 32245, 34163, 36195, 38348, 40628, 43043, 45602, 48313, 51186, 54230, 57455, 60871, 64491, 68326, 72388, 76693, 81255, 86086, 91204, 96627, 102372, 108461, 114911, 121744, 128982, 136650, 144777, 153384, 162507, 172170],
    [2875, 3044, 3224, 3415, 3620, 3835, 4062, 4304, 4561, 4831, 5117, 5423, 5745, 6089, 6449, 6833, 7240, 7671, 8126, 8609, 9122, 9665, 10238, 10849, 11493, 12178, 12900, 13667, 14480, 15342, 16253, 17219, 18242, 19328, 20476, 21696, 22983, 24350, 25796, 27332, 28955, 30679, 32502, 34436, 36482, 38653, 40950, 43386, 45967, 48701, 51595, 54664, 57913, 61359, 65006, 68872, 72967, 77304, 81901, 86771, 91932, 97399, 103191, 109326, 115827, 122715, 130013, 137742, 145934, 154612, 163805, 173545],
    [2897, 3069, 3251, 3445, 3650, 3865, 4094, 4337, 4594, 4869, 5158, 5464, 5788, 6135, 6498, 6885, 7294, 7728, 8189, 8675, 9190, 9739, 10317, 10931, 11580, 12270, 12999, 13771, 14592, 15459, 16379, 17353, 18384, 19478, 20634, 21862, 23163, 24541, 26001, 27545, 29184, 30919, 32758, 34706, 36771, 38956, 41272, 43727, 46327, 49083, 52002, 55093, 58369, 61842, 65519, 69415, 73542, 77915, 82548, 87458, 92658, 98168, 104004, 110188, 116738, 123681, 131035, 138827, 147082, 155829, 165096, 174911],
    [2916, 3090, 3273, 3470, 3674, 3895, 4124, 4367, 4626, 4902, 5191, 5502, 5829, 6176, 6541, 6931, 7343, 7780, 8244, 8735, 9253, 9804, 10385, 11004, 11659, 12352, 13086, 13863, 14687, 15560, 16485, 17464, 18504, 19606, 20771, 22007, 23313, 24702, 26170, 27727, 29375, 31121, 32973, 34932, 37011, 39213, 41542, 44014, 46630, 49405, 52343, 55455, 58751, 62243, 65945, 69865, 74020, 78423, 83085, 88026, 93261, 98807, 104683, 110906, 117502, 124489, 131893, 139733, 148043, 156846, 166173, 176054],
    [2938, 3112, 3298, 3494, 3701, 3922, 4154, 4400, 4662, 4940, 5232, 5546, 5873, 6225, 6593, 6986, 7401, 7843, 8309, 8803, 9327, 9881, 10467, 11092, 11749, 12450, 13190, 13973, 14802, 15683, 16616, 17604, 18651, 19759, 20935, 22179, 23499, 24896, 26377, 27946, 29607, 31367, 33233, 35208, 37303, 39521, 41870, 44360, 46998, 49792, 52755, 55892, 59215, 62737, 66466, 70419, 74606, 79042, 83743, 88724, 94000, 99590, 105510, 111784, 118432, 125474, 132935, 140841, 149216, 158087, 167488, 177448],
    [2959, 3134, 3320, 3516, 3726, 3947, 4182, 4430, 4694, 4973, 5270, 5584, 5917, 6269, 6640, 7035, 7453, 7897, 8366, 8863, 9392, 9949, 10541, 11168, 11834, 12538, 13282, 14071, 14908, 15795, 16736, 17729, 18785, 19901, 21085, 22340, 23668, 25076, 26565, 28145, 29820, 31593, 33473, 35461, 37570, 39805, 42172, 44680, 47337, 50152, 53134, 56293, 59640, 63187, 66946, 70926, 75144, 79612, 84345, 89362, 94677, 100305, 106268, 112586, 119281, 126374, 133890, 141850, 150282, 159220, 168689, 178720],
    [2981, 3159, 3347, 3546, 3756, 3980, 4217, 4465, 4733, 5014, 5311, 5627, 5963, 6318, 6694, 7092, 7513, 7960, 8435, 8934, 9466, 10031, 10625, 11258, 11927, 12636, 13389, 14185, 15028, 15920, 16867, 17868, 18932, 20056, 21248, 22511, 23851, 25267, 26767, 28360, 30046, 31833, 33726, 35732, 37857, 40107, 42492, 45021, 47697, 50534, 53538, 56721, 60096, 63667, 67451, 71464, 75711, 80215, 84984, 90039, 95391, 101063, 107073, 113440, 120187, 127334, 134904, 142928, 151427, 160431, 169971, 180224],
    [3000, 3178, 3366, 3568, 3778, 4002, 4239, 4493, 4757, 5038, 5338, 5657, 5993, 6350, 6727, 7128, 7551, 8001, 8476, 8980, 9515, 10080, 10680, 11315, 11987, 12701, 13454, 14254, 15099, 15997, 16946, 17956, 19022, 20154, 21352, 22623, 23968, 25392, 26904, 28502, 30199, 31994, 33896, 35912, 38048, 40309, 42707, 45247, 47937, 50788, 53808, 57008, 60399, 63989, 67792, 71824, 76096, 80619, 85415, 90492, 95874, 101576, 107616, 114016, 120795, 127978, 135586, 143651, 152192, 161241, 170830, 180224],
];

/// The periods of a synth or hybrid sound's notes in SoundStudio's mixer, by finetune (-8..7)
/// and note: measured on the reference with a hybrid sound of noise, one note a run. Its step
/// is that of the whole frequency a period gives (`syn_step`), so in the lowest octave, where
/// several periods give one step, the lowest that fits is listed. 0: not found (the top note of
/// finetunes 5 to 7, whose frequency is above 65535).
#[rustfmt::skip]
const SYN_PERIODS: [[u16; 72]; 16] = [
    [3635, 3432, 3240, 3057, 2885, 2725, 2572, 2429, 2292, 2163, 2042, 1928, 1819, 1717, 1621, 1530, 1444, 1363, 1286, 1214, 1146, 1082, 1021, 964, 909, 858, 810, 765, 722, 681, 643, 607, 573, 541, 510, 482, 455, 429, 405, 382, 361, 340, 321, 303, 286, 270, 255, 241, 227, 214, 202, 191, 180, 170, 160, 151, 143, 135, 127, 120, 113, 107, 101, 95, 90, 85, 80, 75, 71, 67, 63, 60],
    [3605, 3403, 3214, 3034, 2864, 2702, 2552, 2409, 2275, 2147, 2026, 1913, 1806, 1704, 1608, 1518, 1432, 1352, 1276, 1205, 1137, 1073, 1013, 956, 903, 852, 804, 759, 716, 676, 638, 602, 568, 537, 506, 478, 451, 426, 402, 379, 358, 338, 319, 301, 284, 268, 253, 239, 225, 213, 201, 189, 179, 169, 159, 150, 142, 134, 126, 119, 112, 106, 100, 94, 89, 84, 79, 75, 71, 67, 63, 59],
    [3580, 3377, 3188, 3011, 2844, 2684, 2534, 2392, 2257, 2130, 2010, 1898, 1791, 1691, 1596, 1506, 1422, 1342, 1267, 1195, 1128, 1065, 1005, 949, 895, 845, 798, 753, 711, 671, 633, 597, 564, 532, 502, 474, 447, 422, 399, 376, 355, 335, 316, 298, 282, 266, 251, 237, 223, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 111, 105, 99, 94, 88, 83, 79, 74, 70, 66, 62, 59],
    [3552, 3352, 3165, 2988, 2821, 2664, 2514, 2373, 2239, 2114, 1996, 1883, 1778, 1678, 1583, 1495, 1411, 1332, 1257, 1186, 1120, 1057, 997, 941, 888, 839, 791, 747, 705, 665, 628, 593, 560, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 279, 264, 249, 235, 222, 209, 197, 186, 176, 166, 157, 148, 140, 132, 124, 117, 111, 104, 98, 93, 88, 83, 78, 74, 70, 66, 62, 58],
    [3524, 3327, 3140, 2964, 2797, 2640, 2493, 2354, 2222, 2097, 1979, 1869, 1764, 1664, 1572, 1484, 1400, 1322, 1248, 1177, 1111, 1049, 990, 934, 882, 832, 786, 742, 700, 661, 623, 588, 555, 524, 495, 467, 441, 416, 393, 371, 350, 330, 312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 138, 131, 123, 116, 110, 104, 98, 92, 87, 82, 78, 73, 69, 65, 61, 58],
    [3496, 3300, 3116, 2942, 2775, 2621, 2474, 2335, 2205, 2082, 1965, 1854, 1751, 1653, 1560, 1473, 1390, 1312, 1238, 1169, 1103, 1041, 982, 927, 875, 826, 780, 736, 695, 656, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 275, 260, 245, 231, 218, 206, 195, 184, 173, 164, 154, 146, 137, 130, 122, 115, 109, 103, 97, 92, 86, 82, 77, 73, 68, 65, 61, 57],
    [3472, 3275, 3092, 2918, 2756, 2600, 2456, 2319, 2188, 2066, 1950, 1841, 1737, 1639, 1548, 1461, 1378, 1301, 1228, 1159, 1094, 1033, 975, 920, 868, 819, 773, 730, 689, 650, 614, 579, 547, 516, 487, 460, 434, 409, 386, 365, 344, 325, 307, 289, 273, 258, 243, 230, 217, 204, 193, 182, 172, 162, 153, 144, 136, 129, 121, 115, 108, 102, 96, 91, 86, 81, 76, 72, 68, 64, 60, 57],
    [3446, 3252, 3070, 2897, 2735, 2581, 2437, 2301, 2173, 2051, 1935, 1827, 1724, 1627, 1536, 1450, 1368, 1291, 1219, 1150, 1086, 1025, 967, 913, 862, 813, 768, 725, 684, 645, 609, 575, 543, 512, 483, 456, 431, 406, 384, 362, 342, 322, 304, 287, 271, 256, 241, 228, 215, 203, 192, 181, 171, 161, 152, 143, 135, 128, 120, 114, 107, 101, 96, 90, 85, 80, 76, 71, 67, 64, 60, 57],
    [3419, 3228, 3047, 2876, 2714, 2563, 2419, 2285, 2157, 2037, 1922, 1814, 1712, 1616, 1525, 1440, 1360, 1283, 1211, 1143, 1079, 1018, 961, 907, 856, 808, 763, 720, 679, 641, 605, 571, 539, 509, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 169, 160, 151, 142, 134, 127, 120, 113, 107, 101, 95, 90, 84, 80, 75, 71, 67, 63, 60, 56],
    [3393, 3205, 3026, 2857, 2696, 2545, 2403, 2267, 2140, 2021, 1908, 1800, 1699, 1603, 1514, 1428, 1348, 1272, 1201, 1134, 1070, 1010, 953, 900, 849, 801, 756, 714, 674, 636, 600, 567, 535, 505, 476, 450, 424, 401, 378, 357, 337, 318, 300, 283, 267, 252, 238, 225, 212, 200, 189, 178, 168, 159, 150, 141, 133, 126, 119, 112, 106, 100, 94, 89, 84, 79, 75, 70, 66, 63, 59, 56],
    [3368, 3179, 3001, 2832, 2674, 2525, 2384, 2250, 2125, 2005, 1892, 1787, 1686, 1591, 1502, 1418, 1338, 1263, 1192, 1125, 1062, 1002, 946, 893, 843, 795, 751, 709, 669, 631, 596, 562, 531, 501, 473, 446, 421, 397, 375, 354, 334, 315, 298, 281, 265, 250, 236, 223, 210, 198, 187, 177, 167, 157, 149, 140, 132, 125, 118, 111, 105, 99, 93, 88, 83, 78, 74, 70, 66, 62, 59, 55],
    [3346, 3157, 2981, 2812, 2656, 2505, 2366, 2235, 2110, 1991, 1881, 1774, 1675, 1581, 1492, 1408, 1329, 1255, 1184, 1117, 1055, 995, 940, 887, 837, 790, 746, 704, 664, 627, 592, 559, 527, 498, 470, 443, 418, 395, 373, 352, 332, 313, 296, 279, 263, 249, 235, 221, 209, 197, 186, 176, 166, 156, 148, 139, 131, 124, 117, 110, 104, 98, 93, 88, 83, 78, 74, 69, 65, 62, 58, 55],
    [3321, 3135, 2959, 2793, 2636, 2488, 2349, 2218, 2094, 1976, 1866, 1760, 1662, 1568, 1480, 1397, 1319, 1245, 1175, 1109, 1046, 988, 932, 880, 831, 784, 740, 698, 659, 622, 587, 554, 523, 494, 466, 440, 415, 392, 370, 349, 329, 311, 293, 277, 261, 247, 233, 220, 207, 196, 185, 174, 164, 155, 146, 138, 130, 123, 116, 110, 103, 98, 92, 87, 82, 77, 73, 69, 65, 61, 58, 55],
    [3297, 3113, 2939, 2775, 2619, 2473, 2334, 2203, 2079, 1963, 1852, 1748, 1650, 1557, 1470, 1387, 1310, 1236, 1167, 1101, 1039, 981, 926, 874, 825, 778, 735, 693, 654, 618, 583, 550, 519, 490, 463, 437, 412, 389, 367, 346, 327, 309, 291, 275, 259, 245, 231, 218, 206, 194, 183, 173, 163, 154, 145, 137, 129, 122, 115, 109, 103, 97, 91, 86, 81, 77, 72, 68, 64, 61, 57, 0],
    [3272, 3089, 2915, 2752, 2598, 2452, 2314, 2186, 2062, 1947, 1838, 1735, 1637, 1545, 1458, 1376, 1299, 1226, 1157, 1092, 1031, 973, 919, 867, 818, 772, 729, 688, 649, 613, 578, 546, 515, 486, 459, 433, 409, 386, 364, 344, 324, 306, 289, 273, 257, 243, 229, 216, 204, 193, 182, 172, 162, 153, 144, 136, 128, 121, 114, 108, 102, 96, 91, 86, 81, 76, 72, 68, 64, 60, 57, 0],
    [3252, 3070, 2899, 2735, 2583, 2439, 2302, 2173, 2052, 1937, 1829, 1725, 1629, 1537, 1451, 1369, 1293, 1220, 1152, 1087, 1026, 968, 914, 862, 814, 768, 725, 685, 646, 610, 576, 543, 513, 484, 457, 431, 407, 384, 362, 342, 323, 305, 288, 271, 256, 242, 228, 215, 203, 192, 181, 171, 161, 152, 144, 135, 128, 121, 114, 107, 101, 96, 90, 85, 80, 76, 72, 67, 64, 60, 57, 0],
];

/// The mixer's step for a synth or hybrid sound's period: the frequency is the colour clock
/// (3579545) over the period, a whole number, and the step that over the mix rate, a whole number of
/// 65536ths (fitted on ten measured notes; no other rounding of the two fits them).
fn syn_step(period: u16) -> u64 {
    (3_579_545 / period.max(1) as u64) * 65536 / SOFT_RATE as u64
}

/// When Paula is set up, and how long after that the buffer starts: colour clocks from the
/// song's start. The original does both in its start, before the song's time runs, so the
/// buffer's first fetch is a few colour clocks into the song.
const SOFT_SETUP_AT: u64 = 1;
const SOFT_START_AFTER: u64 = 24;

/// One track's voice of SoundStudio's mixer: Paula's manners (a block plays to its end, then
/// the one set meanwhile), at any rate.
#[derive(Clone, Default)]
struct SoftVoice {
    on: bool,
    /// Playing the silence after a sample's end: nothing, until another block is set.
    idle: bool,
    cur: (u32, u32),
    next: (u32, u32),
    /// The block lies in a 16-bit or stereo sample: which, and the offset.
    wide: Option<(usize, usize)>,
    /// The position in the block and the step a mixed sample, 32.32.
    pos: u64,
    step: u64,
    /// The volume, 0-64.
    level: u32,
}

struct Wide {
    addr: u32,
    frames: u32,
    b16: bool,
    l: Vec<i16>,
    r: Option<Vec<i16>>,
}

/// A voice's levels for a run of samples: an 8-bit sample's factor on each side, and a 16-bit
/// sample's shifts (None: silent), mono and stereo.
#[derive(Clone, Copy, Default)]
struct Lev {
    l: [i64; 2],
    sh: [Option<u32>; 2],
    shc: [Option<u32>; 2],
}

/// OctaMED SoundStudio's mixer, as its player for the Amiga (UADE's Octa-MED-Ss) mixes: see the
/// module's comment.
struct Soft {
    /// Guest memory: side 1's first half, side 0's, side 1's second half, side 0's.
    buf: u32,
    silence: u32,
    v: Vec<SoftVoice>,
    wides: Vec<Wide>,
    /// 0: Paula is to be set up, 1: the buffer is to start, 2: playing.
    stage: u8,
    next_at: Option<u64>,
    /// The half that plays.
    half: usize,
    /// Mixed samples to the next tick, 16.16.
    left: u64,
    ticks: u64,
    /// The echo: crossed, the shift, the delay in samples; what each side put out a delay ago.
    echo: Option<(bool, u8, usize)>,
    ring: [Vec<i16>; 2],
    ring_at: usize,
    vol_adj: u16,
    flags3: u8,
    sep: i8,
}

impl Soft {
    fn reset(&mut self, ss: &SubSong) {
        for v in self.v.iter_mut() {
            *v = SoftVoice::default();
        }
        self.stage = 0;
        self.next_at = None;
        self.half = 0;
        self.left = 0;
        self.ticks = 0;
        self.vol_adj = ss.vol_adj;
        self.flags3 = ss.flags3;
        self.sep = ss.sep;
        self.echo = ss.echo.map(|(cross, depth, ms)| (cross, depth, ((ms as u64 * SOFT_RATE as u64 / 1000) as usize).max(1)));
        let n = self.echo.map_or(0, |e| e.2);
        self.ring = [vec![0; n], vec![0; n]];
        self.ring_at = 0;
    }

    fn enter(&mut self, t: usize, block: (u32, u32)) {
        let v = &mut self.v[t];
        v.cur = block;
        v.idle = block.0 == self.silence || block.1 == 0;
        v.wide = None;
        if !v.idle {
            if let Some(k) = self.wides.iter().position(|w| block.0 >= w.addr && block.0 < w.addr + w.frames) {
                v.wide = Some((k, (block.0 - self.wides[k].addr) as usize));
            }
        }
    }

    fn op(&mut self, t: usize, op: Op) {
        if t >= self.v.len() {
            return;
        }
        match op {
            Op::Dma(false) => self.v[t].on = false,
            Op::Dma(true) => {
                let b = self.v[t].next;
                self.enter(t, b);
                self.v[t].on = true;
                // a new note starts at the sample's first byte, with the fraction the voice's
                // position had (the original does not clear it)
                self.v[t].pos &= 0xffff_ffff;
            }
            Op::Loc(a) => self.v[t].next.0 = a,
            Op::LenB(n) => self.v[t].next.1 = n,
            Op::Len(w) => self.v[t].next.1 = 2 * w as u32,
            Op::Per(_) | Op::Vol(_) => {}
        }
    }

    /// The levels of track `t` at `pan` (-16..16).
    fn levels(&self, t: usize, pan: i8, channels: usize) -> Lev {
        let v = self.v[t].level as i64;
        let side0 = |w: i64| {
            if w <= 0 || v == 0 {
                0
            } else if w >= 32 {
                v
            } else {
                1 + (v - 1) * w / 32
            }
        };
        let stereo = self.flags3 & 1 != 0;
        let l = if stereo {
            [side0(16 + pan as i64), side0(16 - pan as i64)]
        } else {
            [v, v]
        };
        // a 16-bit sample: every factor is taken down to a power of two, and the volume adjust
        // and the channels together never raise it
        let va = self.vol_adj as i64;
        let adj = if va >= 100 { (va / 100).ilog2() as i32 } else if va > 0 { -((1..16).find(|n| (va << n) >= 100).unwrap_or(16)) } else { -64 };
        let chs = (channels.max(1) as u64).next_power_of_two().ilog2() as i32;
        let tot = (adj - chs).min(0);
        // a level of 64 down to a power of two
        let sh = |x: i64| -> Option<u32> {
            if x <= 0 || tot < -40 {
                return None;
            }
            let n = (0..7).find(|n| (64 >> n) <= x).unwrap_or(6);
            Some((8 + n - tot) as u32)
        };
        // a mono 16-bit sample's level is the 8-bit samples' too, but half the volume (rounded
        // down) on both sides with the pan in the middle (measured: 63 gives 31 there, 3 gives
        // 1, 1 none)
        let m = |w: i64| if w == 16 { v >> 1 } else { side0(w) };
        let (w0, w1) = if stereo { (16 + pan as i64, 16 - pan as i64) } else { (32, 32) };
        Lev { l, sh: [sh(m(w0)), sh(m(w1))], shc: [sh(l[0]), sh(l[1])] }
    }

    /// Mixes `n` samples into `out` from `at`: the voices of the first `channels` tracks.
    fn render(&mut self, ram: &[u8], pans: &[i8], channels: usize, out: &mut [Vec<i8>; 2], at: usize, n: usize) {
        let nch = channels.min(self.v.len());
        let (stereo, free) = (self.flags3 & 1 != 0, self.flags3 & 2 != 0);
        // stereo without free panning: the levels are over half the channels, rounded down
        // for the 8-bit samples' division and up for the 16-bit ones' shift (measured with 1
        // to 7 and 25 channels)
        let (over, over16) = if stereo && !free { ((channels / 2).max(1), channels.div_ceil(2)) } else { (channels.max(1), channels) };
        let lev: Vec<Lev> = (0..nch).map(|t| self.levels(t, pans[t], over16)).collect();
        let muted = self.ticks <= 1;
        let va = self.vol_adj as i64;
        let den = 6400 * over as i64;
        // an 8-bit sample's term: the player's table entry, 8.8, cut towards zero. A level
        // times the volume adjust is the player's 16-bit product (it wraps: 64 * 800)
        let k = |s: i64, l: i64| s * ((l * va) as u16 as i16 as i64) * 256 / den;
        for i in at..at + n {
            // the two sides' sums: 16-bit words, 8.8 (the high byte is what Paula plays)
            let mut a = [0i64; 2];
            for t in 0..nch {
                if !self.v[t].on {
                    continue;
                }
                if self.v[t].idle {
                    if self.v[t].next == self.v[t].cur {
                        continue;
                    }
                    let b = self.v[t].next;
                    self.enter(t, b);
                    self.v[t].pos &= 0xffff_ffff;
                    if self.v[t].idle {
                        continue;
                    }
                }
                let mut guard = 0;
                while (self.v[t].pos >> 32) as u32 >= self.v[t].cur.1 {
                    let len = self.v[t].cur.1 as u64;
                    self.v[t].pos -= len << 32;
                    let b = self.v[t].next;
                    self.enter(t, b);
                    guard += 1;
                    if self.v[t].idle || guard > 64 {
                        self.v[t].pos &= 0xffff_ffff;
                        break;
                    }
                }
                let v = &mut self.v[t];
                if v.idle {
                    continue;
                }
                let idx = (v.pos >> 32) as usize;
                v.pos += v.step;
                if muted {
                    continue;
                }
                let lv = &lev[t];
                match v.wide {
                    None => {
                        let s = ram.get(v.cur.0 as usize + idx).copied().unwrap_or(0) as i8 as i64;
                        a[0] += k(s, lv.l[0]);
                        a[1] += k(s, lv.l[1]);
                    }
                    Some((wi, off)) => {
                        let wd = &self.wides[wi];
                        let l = wd.l.get(off + idx).copied().unwrap_or(0) as i64;
                        let r = wd.r.as_ref().map(|r| r.get(off + idx).copied().unwrap_or(0) as i64);
                        match (wd.b16, r) {
                            (false, None) => {
                                a[0] += k(l, lv.l[0]);
                                a[1] += k(l, lv.l[1]);
                            }
                            (false, Some(r)) => {
                                a[1] += k(l, lv.l[1]);
                                a[0] += k(r, lv.l[0]);
                            }
                            (true, None) => {
                                for s in 0..2 {
                                    if let Some(sh) = lv.sh[s] {
                                        a[s] += l >> (sh - 8).min(62);
                                    }
                                }
                            }
                            (true, Some(r)) => {
                                if let Some(sh) = lv.shc[1] {
                                    a[1] += l >> (sh - 8).min(62);
                                }
                                if let Some(sh) = lv.shc[0] {
                                    a[0] += r >> (sh - 8).min(62);
                                }
                            }
                        }
                    }
                }
            }
            // the echo first (its memory is without the separation), then the separation
            let mut o = [a[0] as i16, a[1] as i16];
            if let Some((cross, depth, d)) = self.echo {
                let e = [self.ring[0][self.ring_at], self.ring[1][self.ring_at]];
                for s in 0..2 {
                    let src = if cross { e[s ^ 1] } else { e[s] };
                    o[s] = o[s].wrapping_add(src >> depth.min(7));
                }
                self.ring[0][self.ring_at] = o[0];
                self.ring[1][self.ring_at] = o[1];
                self.ring_at = (self.ring_at + 1) % d;
            }
            if stereo && self.sep != 0 {
                let n = 5 - self.sep.unsigned_abs() as u32;
                let sg = -(self.sep.signum() as i16);
                o = [o[0].wrapping_add(sg * (o[1] >> n)), o[1].wrapping_add(sg * (o[0] >> n))];
            }
            out[0][i] = (o[0] >> 8) as i8;
            out[1][i] = (o[1] >> 8) as i8;
        }
    }
}

pub struct OctaMed {
    /// SoundStudio's mixer (an MMD3 song), in place of the amplifier's voices.
    soft: Option<Box<Soft>>,
    name: &'static str,
    /// An MMD0-MMD2 song.
    old: bool,
    /// The voices of the amplifier: the tracks', and the echo's after them.
    total: usize,
    echo: Option<EchoState>,
    song: Arc<Song>,
    sub: usize,
    freqs: Vec<[u16; 72]>,
    /// A 5-8 channel song: the period tables of the 4-channel player (`super::periods`), which
    /// its notes are played from (empty otherwise).
    periods: Vec<u16>,
    silence: u32,
    nvoices: usize,
    td: Vec<Track>,
    ch: Vec<Chan>,
    /// The instruments' current volumes (cmd 0C sets a default).
    ivol: [u32; MAX_INSTR],
    tempo: Tempo,
    track_pan: [i8; MAX_TRACKS],
    pos: Pos,
    pulse: i32,
    block_delay: u32,
    channels: usize,
    fx_line: i32,
    fx_block: u32,
    next_line: i32,
    repeat_line: i32,
    repeat_counter: u32,
    delayed_stop: bool,
    brk: Brk,
    songend: bool,
    first_tick: bool,
}

#[derive(Default)]
struct Ev {
    end: bool,
    stop: bool,
}

fn new(d: &[u8], base: u32, m: &mut Machine, kind: Kind) -> Option<Box<dyn Replayer>> {
    if kind_of(d) != Some(kind) {
        return None;
    }
    let mut song = load(d, base, m, kind)?;
    let soft_mode = kind == Kind::Oss;
    let nvoices = song.subs.iter().map(|s| s.channels).max().unwrap_or(4).clamp(1, if soft_mode { MAX_TRACKS } else { MAX_VOICES });
    let silence = m.alloc(4);
    // the echo's voices follow the tracks' (on the same sides: from a multiple
    // of 4), if they fit
    let echoes = song.subs.iter().any(|s| s.echo.is_some());
    let eb = (nvoices + 3) & !3;
    let total = if !soft_mode && echoes && eb + ((nvoices + 1) & !1) <= MAX_VOICES { eb + ((nvoices + 1) & !1) } else { nvoices };
    // a 5-8 channel song's amplifier has a voice for every track of its widest block, as the
    // OctaMED Eagleplayer's has (8 voices for a song that splits one channel only)
    let amp_voices = if song.eight {
        song.subs.iter().flat_map(|s| s.blocks.iter()).map(|b| b.tracks).max().unwrap_or(0).clamp(total, MAX_VOICES)
    } else {
        total
    };
    let mut soft = None;
    if soft_mode {
        let buf = m.alloc(4 * SOFT_HALF as u32);
        if silence == 0 || buf == 0 {
            return None;
        }
        let mut wides = Vec::new();
        for smp in song.samp.iter_mut().flatten() {
            for (k, (b16, l, r)) in std::mem::take(&mut smp.wide).into_iter().enumerate() {
                if let Some(&(addr, frames)) = smp.octs.get(k) {
                    wides.push(Wide { addr, frames, b16, l, r });
                }
            }
        }
        soft = Some(Box::new(Soft {
            buf,
            silence,
            v: vec![SoftVoice::default(); nvoices],
            wides,
            stage: 0,
            next_at: None,
            half: 0,
            left: 0,
            ticks: 0,
            echo: None,
            ring: [Vec::new(), Vec::new()],
            ring_at: 0,
            vol_adj: 100,
            flags3: 0,
            sep: 0,
        }));
    } else if silence == 0 || !start_amplifier(m, amp_voices) {
        return None;
    }
    // the mix rate: the highest the emulated Paula plays one byte a channel at
    // (its DMA reads a word a line), 31 kHz, not the 25 kHz or so the amplifier's
    // default is for this many voices (the original's rate is not known). The level
    // stays the amplifier's, which keeps every voice of a side at full volume
    // within the 8-bit stage (some of these songs come near it, and the same
    // songs at twice the level, tried first, clipped); the echo's voices, which sound
    // less, are not to make it quieter.
    // A 5-8 channel song keeps the amplifier's rate for its voices: its original is the OctaMED
    // Eagleplayer on that amplifier.
    if let Some(a) = m.bus.amp.as_mut() {
        if !song.eight {
            a.period = MIX_PERIOD;
        }
        if total > nvoices {
            a.boost = a.boost * total as u32 / nvoices as u32;
        }
    }
    if !soft_mode {
        crate::amplifier::restart(&mut m.bus);
    }
    let eight = song.eight;
    let mut p = OctaMed {
        soft,
        old: kind == Kind::Old,
        total,
        echo: None,
        name: if kind == Kind::Oss { "OctaMED SoundStudio" } else { "OctaMED" },
        song: Arc::new(song),
        sub: 0,
        freqs: freq_table(),
        periods: if eight { super::periods() } else { Vec::new() },
        silence,
        nvoices,
        td: vec![Track::default(); MAX_TRACKS],
        ch: vec![Chan::default(); MAX_TRACKS],
        ivol: [0; MAX_INSTR],
        tempo: Tempo { tempo: 125, tpl: 6, lpb: 4, bpm: true, eight: false },
        track_pan: [0; MAX_TRACKS],
        pos: Pos::default(),
        pulse: 0,
        block_delay: 0,
        channels: 4,
        fx_line: 0,
        fx_block: 0,
        next_line: 0,
        repeat_line: 0,
        repeat_counter: 0,
        delayed_stop: false,
        brk: Brk::Normal,
        songend: false,
        first_tick: true,
    };
    p.play_song(0);
    p.init_led(&mut Hw { bus: &mut m.bus });
    Some(Box::new(p))
}

impl OctaMed {
    /// The start's write of the LED bit (before the first tick, as the player's
    /// InitSound makes it).
    fn init_led(&self, hw: &mut Hw) {
        if !self.old {
            if let Some(on) = self.song.subs[self.sub].led {
                set_filter(hw, on);
            }
        }
    }

    fn play_song(&mut self, sub: usize) {
        self.sub = sub.min(self.song.subs.len() - 1);
        self.td = vec![Track::default(); MAX_TRACKS];
        for t in self.td.iter_mut() {
            t.note_off_cnt = -1;
        }
        self.ch = vec![Chan::default(); MAX_TRACKS];
        for i in 0..MAX_INSTR {
            self.ivol[i] = self.song.instr[i].init_vol;
        }
        let song = self.song.clone();
        let ss = &song.subs[self.sub];
        self.tempo = ss.tempo;
        self.track_pan = ss.track_pan;
        self.channels = ss.channels;
        self.pulse = ss.tempo.tpl as i32;
        self.block_delay = 0;
        self.fx_line = 0;
        self.fx_block = 0;
        self.next_line = 0;
        self.brk = Brk::Normal;
        self.repeat_line = 0;
        self.repeat_counter = 0;
        self.delayed_stop = false;
        self.songend = false;
        self.first_tick = true;
        log::debug!(
            "octamed: subsong {sub}: tempo {} tpl {} lpb {} bpm {}, {} channels, {} blocks",
            self.tempo.tempo,
            self.tempo.tpl,
            self.tempo.lpb,
            self.tempo.bpm,
            self.channels,
            ss.blocks.len()
        );
        self.echo = None;
        if let Some(s) = self.soft.as_mut() {
            s.reset(ss);
        }
        if self.total > self.nvoices {
            if let Some((cross, depth, ms)) = ss.echo {
                self.echo = Some(EchoState {
                    cross,
                    depth,
                    delay: (ms as f64 * PAULA_CLOCK_PAL / 1000.0) as u64,
                    base: (self.nvoices + 3) & !3,
                    pending: VecDeque::new(),
                });
            }
        }
        // the start position
        let mut pos = Pos { sec: 0, seq: 0, line: 0, ..Pos::default() };
        pos.psq = ss.sections[0] as u32;
        let mut ev = Ev::default();
        advance_song_pos(&mut pos, ss, 0, false, &mut ev);
        verify(&mut pos, ss);
        self.pos = pos;
    }

    fn note_freq(&self, note: u8, ft: i32) -> u32 {
        let n = (note as usize).min(71);
        self.freqs[(ft + 8).clamp(0, 15) as usize][n] as u32
    }

    fn instr_note_freq(&self, note: u8, inum: usize) -> u32 {
        let i = &self.song.instr[inum];
        match note {
            NOTE_DEF => {
                if i.def_freq != 0 {
                    i.def_freq
                } else {
                    22050
                }
            }
            NOTE_11K => 11025,
            NOTE_22K => 22050,
            NOTE_44K => 44100,
            _ => {
                let smp = self.song.samp[inum].as_ref();
                let ft = i.fine_tune as i32;
                if smp.is_some_and(|s| s.syn.is_some()) {
                    if note <= 0x7f {
                        let mut n = note;
                        // (SoundStudio's mixer plays the notes above as they are: measured)
                        while n > 60 && self.soft.is_none() {
                            n -= 12;
                        }
                        return self.note_freq(n.wrapping_sub(1), ft);
                    }
                    return 0;
                }
                if note <= 0x7f {
                    let mut n = note;
                    if let Some(s) = smp {
                        n = (n as i32 + s.note_diff(n.wrapping_sub(1))) as u8;
                    }
                    if self.soft.is_some() && i.outdev != 0 {
                        n = n.saturating_add(1);
                    }
                    return self.note_freq(n.wrapping_sub(1), ft);
                }
                0
            }
        }
    }

    // ---- the voices

    fn mute(&mut self, hw: &mut Hw, t: usize) {
        if t >= self.channels || t >= self.nvoices {
            return;
        }
        if self.ch[t].on {
            self.vp(hw, t, Op::Dma(false));
            self.ch[t].on = false;
        }
    }

    /// The voice's period for a frequency. The original counts 3579545 a second
    /// (the NTSC clock) for a period and plays it with PAL's, so its notes sound
    /// 0.9% lower than the frequency: what UADE's render shows.
    /// A poke of track `t`'s voice, and of its echo's, a delay later.
    fn vp(&mut self, hw: &mut Hw, t: usize, op: Op) {
        if let Some(s) = self.soft.as_mut() {
            s.op(t, op);
            return;
        }
        apply(hw, t, op);
        if let Some(e) = self.echo.as_mut() {
            if t < self.channels {
                let v = e.base + if e.cross { t ^ 1 } else { t };
                if v < self.total {
                    let op = if let Op::Vol(x) = op { Op::Vol(x >> e.depth) } else { op };
                    e.pending.push_back((hw.bus.clock + e.delay, v, op));
                }
            }
        }
    }

    /// The echo's pokes that are due (to the nearest tick).
    fn flush_echo(&mut self, hw: &mut Hw) {
        let half = self.tick_clocks_f() as u64 / 2;
        if let Some(e) = self.echo.as_mut() {
            while let Some(&(due, v, op)) = e.pending.front() {
                if due > hw.bus.clock + half {
                    break;
                }
                e.pending.pop_front();
                apply(hw, v, op);
            }
        }
    }

    fn per_of(freq: i32) -> u16 {
        ((3579545.0 / freq as f64) + 0.5).clamp(1.0, 65535.0) as u16
    }

    /// Starts the block `addr`, `frames` long from frame `from`, then the loop.
    fn start_block(&mut self, hw: &mut Hw, t: usize, addr: u32, frames: u32, from: u32, lp: (u32, u32)) {
        let c = &mut self.ch[t];
        c.buf = (addr, frames);
        c.lp = lp;
        c.pos = from as f64;
        c.on = true;
        c.per = 0;
        c.vol = 0xffff;
        let first_end = if lp.1 > 2 { frames.min(lp.0 + lp.1) } else { frames };
        let first_end = if first_end > from { first_end } else { frames };
        self.vp(hw, t, Op::Dma(false));
        self.vp(hw, t, Op::Loc(addr + from));
        self.vp(hw, t, Op::LenB(first_end - from));
        self.vp(hw, t, Op::Dma(true));
        if lp.1 > 2 {
            self.vp(hw, t, Op::Loc(addr + lp.0));
            self.vp(hw, t, Op::LenB(lp.1));
        } else {
            self.vp(hw, t, Op::Loc(self.silence));
            self.vp(hw, t, Op::Len(1));
        }
    }

    fn play(&mut self, hw: &mut Hw, t: usize, note: u8, inum: usize, start: u32, lp: (u32, u32), flags: u8) {
        if t >= self.channels || t >= self.nvoices {
            return;
        }
        let song = self.song.clone();
        let Some(smp) = song.samp[inum].as_ref() else { return };
        if smp.octs.is_empty() {
            return;
        }
        let (mut rep, mut replen) = lp;
        let buf = if smp.stype == 0 {
            0
        } else {
            let b = OCT_BUF[smp.stype - 1][((note / 12) as usize).min(5)];
            rep <<= b;
            replen <<= b;
            b
        };
        let Some(&(addr, frames)) = smp.octs.get(buf) else { return };
        let looping = flags & I_LOOP != 0 && replen > 2;
        log::debug!(
            "octamed: {:.3}s track {t} note {note} instr {} freq {} per {} frames {frames} loop {looping} {rep}+{replen} start {start}",
            hw.bus.clock as f64 / PAULA_CLOCK_PAL,
            inum + 1,
            self.td[t].frequency,
            if self.td[t].frequency > 0 { Self::per_of(self.td[t].frequency) } else { 0 }
        );
        let mut start = start;
        if start >= frames {
            start = 0;
        }
        if frames <= start {
            return;
        }
        if looping {
            if let Some(Some((pa, pf, ps, pl))) = smp.pp.get(buf) {
                self.start_block(hw, t, *pa, *pf, start.min(*pf - 1), (*ps, *pl));
                return;
            }
        }
        let lp = if looping { (rep, replen) } else { (0, 0) };
        self.start_block(hw, t, addr, frames, start, lp);
    }

    fn set_position(&mut self, hw: &mut Hw, t: usize, pos: u32) {
        let c = self.ch[t].clone();
        if t >= self.nvoices || !c.on || c.buf.1 == 0 {
            return;
        }
        let pos = if pos >= c.buf.1 { 0 } else { pos };
        let per = c.per;
        let vol = c.vol;
        self.start_block(hw, t, c.buf.0, c.buf.1, pos, c.lp);
        if per != 0 {
            self.vp(hw, t, Op::Per(per));
            self.ch[t].per = per;
        }
        if vol != 0xffff {
            self.vp(hw, t, Op::Vol(vol));
            self.ch[t].vol = vol;
        }
    }

    fn set_freq(&mut self, hw: &mut Hw, t: usize, freq: i32) {
        if t >= self.channels || t >= self.nvoices {
            return;
        }
        if freq <= 0 || freq > 65535 {
            self.mute(hw, t);
            return;
        }
        if self.soft.is_some() {
            let step = self.soft_step(t, freq);
            if let Some(v) = self.soft.as_mut().and_then(|s| s.v.get_mut(t)) {
                v.step = step << 16;
            }
        }
        let per = self.table_period(t, freq).unwrap_or_else(|| Self::per_of(freq));
        if self.ch[t].per != per {
            self.ch[t].per = per;
            self.vp(hw, t, Op::Per(per));
        }
    }

    /// The mixer's step (16.16) for a frequency: the measured one of a note of the track's
    /// finetune, between two notes a line from one's to the next's (a guess: the original's
    /// sum is not known), outside the table the rate's.
    fn soft_step(&self, t: usize, freq: i32) -> u64 {
        let ft = (self.td[t].fine_tune.clamp(-8, 7) + 8) as usize;
        let (fr, st) = (&self.freqs[ft], &STEPS[ft]);
        let f = freq.clamp(0, 65535) as u16;
        if self.td[t].sy.ty != 0 {
            // a synth or hybrid sound: the note's period; a frequency that is no note's (a
            // slide, a vibrato) has the whole period the colour clock gives over it (measured
            // on a module's slides of four synth sounds)
            return match fr.binary_search(&f) {
                Ok(n) if SYN_PERIODS[ft][n] != 0 => syn_step(SYN_PERIODS[ft][n]),
                _ => syn_step((3_579_545 / f.max(1) as u32).min(65535) as u16),
            };
        }
        if f < 65535 && fr.binary_search(&f).is_err() {
            // not a note of this finetune: one of another's (an instrument number without a
            // note changes the track's finetune and leaves the note's frequency)
            for (r, row) in self.freqs.iter().enumerate() {
                if let Ok(n) = row.binary_search(&f) {
                    return STEPS[r][n] as u64;
                }
            }
        }
        if f >= fr[0] && f < 65535 {
            let n = fr.partition_point(|&x| x <= f) - 1;
            if fr[n] == f {
                return st[n] as u64;
            }
            if n + 1 < fr.len() && fr[n + 1] > fr[n] {
                let (a, b) = (st[n] as u64, st[n + 1] as u64);
                return a + b.saturating_sub(a) * (f - fr[n]) as u64 / (fr[n + 1] - fr[n]) as u64;
            }
        }
        ((freq.max(0) as u64) << 16).div_ceil(SOFT_STEP_RATE)
    }

    /// A 5-8 channel song's period for a frequency that is a note of the track's finetune: the
    /// period table's entry (the Eagleplayer plays G-1 at 570, not at the 571 the frequency gives).
    fn table_period(&self, t: usize, freq: i32) -> Option<u16> {
        if self.periods.is_empty() {
            return None;
        }
        let ft = self.td[t].fine_tune.clamp(-8, 7);
        let n = self.freqs[(ft + 8) as usize].iter().position(|&f| f as i32 == freq)?;
        let row = if ft >= 0 { ft } else { 16 + ft } as usize;
        self.periods.get(row * super::TABLE + n).copied().filter(|&p| p != 0)
    }

    fn set_vol(&mut self, hw: &mut Hw, t: usize, vol: u32) {
        if t >= self.channels || t >= self.nvoices {
            return;
        }
        if let Some(sv) = self.soft.as_mut().and_then(|s| s.v.get_mut(t)) {
            sv.level = vol.min(128) >> 1;
        }
        let v = (vol / 2).min(64) as u16;
        if self.ch[t].vol != v {
            self.ch[t].vol = v;
            self.vp(hw, t, Op::Vol(v));
        }
    }

    fn set_synth_wf(&mut self, hw: &mut Hw, t: usize, addr: u32, bytes: u32) {
        if t >= self.channels || t >= self.nvoices || bytes == 0 {
            return;
        }
        if self.ch[t].start_syn {
            self.ch[t].start_syn = false;
            let (per, vol) = (self.ch[t].per, self.ch[t].vol);
            self.start_block(hw, t, addr, bytes, 0, (0, bytes));
            if per != 0 {
                self.ch[t].per = per;
                self.vp(hw, t, Op::Per(per));
            }
            if vol != 0xffff {
                self.ch[t].vol = vol;
                self.vp(hw, t, Op::Vol(vol));
            }
        } else {
            self.vp(hw, t, Op::Loc(addr));
            self.vp(hw, t, Op::LenB(bytes));
        }
        self.ch[t].buf = (addr, bytes);
        self.ch[t].lp = (0, bytes);
    }

    // ---- the tick

    fn callback(&mut self, hw: &mut Hw) {
        let song = self.song.clone();
        let ss = &song.subs[self.sub];
        let mut ev = Ev::default();
        let line;
        let mut blk;
        self.pulse += 1;
        if self.pulse >= self.tempo.tpl as i32 {
            blk = &ss.blocks[self.pos.block as usize];
            self.pulse = 0;
            if self.delayed_stop {
                self.songend = true;
            }
            let hold_line = if self.block_delay != 0 && {
                self.block_delay -= 1;
                self.block_delay != 0
            } {
                // no new line
                false
            } else {
                true
            };
            if hold_line {
                line = self.pos.line;
                let l = line as usize;
                for t in 0..blk.tracks {
                    let td = &mut self.td[t];
                    td.no_play = false;
                    let [n, i] = blk.note(l, t);
                    td.curr_note = if (NOTE_DEF..=NOTE_44K).contains(&n) {
                        n
                    } else if n > 0x7f {
                        0
                    } else {
                        n
                    };
                    if td.curr_note != 0 {
                        td.prev_note = td.curr_note;
                    }
                    if i != 0 && i as usize <= MAX_INSTR {
                        td.prev_inum = i as usize - 1;
                        let ci = &song.instr[td.prev_inum];
                        td.prev_vol = self.ivol[td.prev_inum] as u8;
                        td.init_hold = ci.hold as u32;
                        td.init_decay = ci.decay as u32;
                        td.stransp = ci.transp as i32;
                        td.fine_tune = ci.fine_tune as i32;
                        td.soffset = 0;
                        td.misc = 0;
                        if n != 0 && song.samp[td.prev_inum].as_ref().is_some_and(|s| s.syn.is_some()) {
                            let inum = td.prev_inum;
                            self.clear_synth(t, inum);
                        }
                    }
                    if n == 0x80 {
                        self.td[t].misc |= M_STOP;
                    }
                }
                // pre-fx
                for page in 0..blk.pages {
                    for t in 0..blk.tracks {
                        let cmd = blk.cmd(l, t, page);
                        self.pre_fx(hw, ss, t, cmd, line, &mut ev);
                    }
                }
                // play the notes
                for t in 0..blk.tracks {
                    if self.td[t].no_play {
                        continue;
                    }
                    if self.td[t].curr_note != 0 {
                        self.td[t].note_off_cnt = if self.td[t].init_hold != 0 { self.td[t].init_hold as i32 } else { -1 };
                        let (n, inum) = (self.td[t].curr_note.wrapping_sub(1), self.td[t].prev_inum);
                        self.plr_play_note(hw, ss, t, n, inum);
                    }
                }
                self.fx_line = line;
                self.fx_block = self.pos.block;
                // advance
                if !self.delayed_stop {
                    match self.brk {
                        Brk::Loop => {
                            self.pos.line = self.next_line;
                            verify(&mut self.pos, ss);
                            self.brk = Brk::Normal;
                        }
                        Brk::PatternBreak => {
                            let p = &mut self.pos;
                            let n = p.seq + 1;
                            advance_song_pos(p, ss, n, true, &mut ev);
                            p.line = self.next_line;
                            if p.line as usize >= ss.blocks[p.block as usize].lines {
                                p.line = 0;
                            }
                            self.brk = Brk::Normal;
                        }
                        Brk::PositionJump => {
                            let p = &mut self.pos;
                            let n = self.next_line as u32;
                            if n <= p.seq {
                                ev.end = true;
                            }
                            advance_song_pos(p, ss, n, true, &mut ev);
                            p.line = 0;
                            self.brk = Brk::Normal;
                        }
                        Brk::Normal => {
                            let p = &mut self.pos;
                            p.line += 1;
                            if p.line as usize >= ss.blocks[p.block as usize].lines {
                                p.line = 0;
                                let n = p.seq + 1;
                                advance_song_pos(p, ss, n, true, &mut ev);
                            }
                        }
                    }
                }
                blk = &ss.blocks[self.pos.block as usize];
            }
            // check holding
            let cl = self.pos.line as usize;
            for t in 0..blk.tracks {
                if self.td[t].note_off_cnt >= 0 {
                    let [n, i] = blk.note(cl, t);
                    if (n == 0 && i != 0) || (n != 0 && blk.cmd(cl, t, 0).cmd == 0x03) {
                        self.td[t].note_off_cnt += self.tempo.tpl as i32;
                    }
                }
            }
        }
        if ev.end {
            self.songend = true;
        }
        if ev.stop {
            self.delayed_stop = true;
        }
        // every tick
        if self.fx_block as usize >= ss.blocks.len() {
            self.fx_block = ss.blocks.len() as u32 - 1;
        }
        let blk = &ss.blocks[self.fx_block as usize];
        for t in 0..blk.tracks {
            let stop_now = {
                let td = &mut self.td[t];
                if td.misc & M_STOP != 0 || (td.note_off_cnt >= 0 && {
                    td.note_off_cnt -= 1;
                    td.note_off_cnt < 0
                }) {
                    td.misc &= !M_STOP;
                    true
                } else {
                    false
                }
            };
            if stop_now {
                let td = &mut self.td[t];
                if td.sy.ty != 0 {
                    td.sy.vol_cmd_pos = td.decay;
                    td.sy.vol_wait = 0;
                } else {
                    td.fade_speed = td.decay * 2;
                    if td.fade_speed == 0 {
                        self.mute_track(hw, t);
                    }
                }
            }
            let td = &mut self.td[t];
            if td.fade_speed != 0 {
                if td.prev_vol as u32 > td.fade_speed {
                    td.prev_vol -= td.fade_speed as u8;
                } else {
                    td.prev_vol = 0;
                    td.fade_speed = 0;
                }
            }
            td.no_play = false;
        }
        if self.fx_line as usize >= blk.lines {
            self.fx_line = blk.lines as i32 - 1;
        }
        let fl = self.fx_line as usize;
        for page in 0..blk.pages {
            for t in 0..blk.tracks {
                let cmd = blk.cmd(fl, t, page);
                if self.td[t].last_note_midi && midi_command(cmd) {
                    continue;
                }
                self.every_tick_cmd(hw, ss, t, cmd);
            }
        }
        for t in 0..blk.tracks {
            self.update_freq_vol_pan(hw, ss, t);
        }
        // the played frames
        let clocks = self.tick_clocks_f();
        for t in 0..self.nvoices {
            let c = &mut self.ch[t];
            if c.on && c.per != 0 {
                c.pos += clocks / c.per as f64;
            }
        }
    }

    /// The colour clocks of a tick. The original's timer counts 3579545 a second
    /// (the NTSC clock, as the slides do), so on a PAL Amiga its ticks are 0.9%
    /// longer than the tempo's Hz: what UADE's render shows.
    fn tick_clocks_f(&self) -> f64 {
        if let Some(c) = self.tempo.eight_clocks() {
            return c;
        }
        // the older songs' ticks are another 0.45% longer in UADE's renders (fitted
        // on 14 of them, drifting 0.02 s in 4 s; the cause is not known)
        let slow = if self.old { 1.0045 } else { 1.0 };
        3579545.0 * slow / self.tempo.hz().max(0.1)
    }

    /// SoundStudio's mixer: Paula's set-up, then the buffer's start (the two "ticks" of a song
    /// that has no timer: the song's ticks are counted in mixed samples).
    fn soft_tick(&mut self, hw: &mut Hw) {
        let Some(s) = self.soft.as_mut() else { return };
        let now = hw.bus.clock;
        match s.stage {
            0 => {
                s.stage = 1;
                s.next_at = Some(now + SOFT_START_AFTER);
                hw.bus.write_word(0xdff09a, 0x0780);
                hw.wait(1);
                hw.bus.write_word(0xdff096, 0x000f);
                hw.wait(12);
                for (reg, val) in [(0xa4, 0x2000), (0xa6, SOFT_PERIOD), (0xa8, 64)] {
                    for ch in 0..4 {
                        hw.bus.write_word(0xdff000 + reg + 16 * ch, val);
                        hw.wait(1);
                    }
                }
            }
            1 => {
                s.stage = 2;
                s.next_at = Some(now + (1 << 40));
                s.half = 0;
                let (buf, end) = (s.buf as usize, s.buf as usize + 4 * SOFT_HALF);
                hw.bus.ram[buf..end].fill(0);
                Self::soft_locs(hw, s.buf, 0);
                hw.bus.write_word(0xdff09a, 0x8080);
                hw.wait(1);
                hw.bus.write_word(0xdff09c, 0x0080);
                hw.wait(1);
                hw.bus.write_word(0xdff096, 0x800f);
            }
            _ => s.next_at = Some(now + (1 << 40)),
        }
    }

    /// Paula's pointers at half `h` of the buffer: channels 0 and 3 play side 0, 1 and 2 side 1.
    fn soft_locs(hw: &mut Hw, buf: u32, h: usize) {
        let b = buf + 2 * (h * SOFT_HALF) as u32;
        for (ch, a) in [(0, b + SOFT_HALF as u32), (1, b), (2, b), (3, b + SOFT_HALF as u32)] {
            hw.bus.write_word(0xdff0a0 + 16 * ch, (a >> 16) as u16);
            hw.bus.write_word(0xdff0a2 + 16 * ch, a as u16);
            hw.wait(1);
        }
    }

    /// Paula started a half: the pointers go to the other, and the song's next 16384 samples
    /// are mixed into it, its ticks where they fall.
    fn soft_irq(&mut self, hw: &mut Hw) {
        let Some(s) = self.soft.as_mut() else { return };
        hw.wait(1);
        hw.bus.write_word(0xdff09c, 0x0080);
        hw.wait(6);
        s.half ^= 1;
        let (buf, h) = (s.buf, s.half);
        Self::soft_locs(hw, buf, h);
        let mut out = [vec![0i8; SOFT_HALF], vec![0i8; SOFT_HALF]];
        let mut i = 0;
        while i < SOFT_HALF {
            if self.soft.as_ref().is_some_and(|s| s.left < 1 << 16) {
                self.callback(hw);
                // a tick's samples, 16.16: measured at eight tempos as ten times the whole
                // part of 24023.005 * 65536 / (tempo * lines a beat)
                let t = self.tempo;
                let len = if t.bpm {
                    10 * (SOFT_TICK / (t.tempo.max(1) as u64 * t.lpb.max(1) as u64))
                } else {
                    (SOFT_RATE * 65536.0 / t.hz().max(0.1)) as u64
                };
                if let Some(s) = self.soft.as_mut() {
                    s.ticks += 1;
                    s.left += len.max(1 << 16);
                }
                continue;
            }
            let Some(s) = self.soft.as_mut() else { return };
            let run = ((s.left >> 16) as usize).clamp(1, SOFT_HALF - i);
            s.render(&hw.bus.ram, &self.track_pan, self.channels, &mut out, i, run);
            s.left -= (run as u64) << 16;
            i += run;
        }
        let base = buf as usize + 2 * h * SOFT_HALF;
        for (side, o) in out.iter().enumerate() {
            let a = base + if side == 0 { SOFT_HALF } else { 0 };
            for (d, &v) in hw.bus.ram[a..a + SOFT_HALF].iter_mut().zip(o.iter()) {
                *d = v as u8;
            }
        }
    }

    fn mute_track(&mut self, hw: &mut Hw, t: usize) {
        self.td[t].sy.ty = 0;
        self.mute(hw, t);
    }

    fn pre_fx(&mut self, hw: &mut Hw, ss: &SubSong, t: usize, cmd: Cmd, line: i32, ev: &mut Ev) {
        let _ = ev;
        let data = cmd.d0;
        let dataw = cmd.data();
        let song = self.song.clone();
        match cmd.cmd {
            0x03 => {
                if self.td[t].curr_note != 0 {
                    let mut dest = self.td[t].curr_note;
                    if dest < 0x80 {
                        let mut dn = dest as i32 + ss.play_transp as i32 + self.td[t].stransp;
                        while dn >= 0x80 {
                            dn -= 12;
                        }
                        while dn < 1 {
                            dn += 12;
                        }
                        dest = dn as u8;
                    }
                    self.td[t].port_target = self.instr_note_freq(dest, self.td[t].prev_inum) as i32;
                }
                if dataw != 0 {
                    self.td[t].port_speed = dataw;
                }
                self.td[t].no_play = true;
            }
            0x08 => {
                if song.instr[self.td[t].prev_inum].midi_ch != 0 {
                    self.td[t].init_hold = data as u32;
                } else {
                    self.td[t].init_hold = (data & 0xf) as u32;
                    self.td[t].init_decay = (data >> 4) as u32;
                }
            }
            0x09 => {
                if data & 0x1f != 0 {
                    self.tempo.tpl = (data & 0x1f) as u16;
                }
            }
            0x0b => {
                self.brk = Brk::PositionJump;
                self.next_line = data as i32;
            }
            0x0c => {
                if data < 0x80 {
                    if data <= 64 {
                        self.td[t].prev_vol = data * 2;
                    }
                } else {
                    let inum = self.td[t].prev_inum;
                    let synth = song.samp[inum].as_ref().is_some_and(|s| s.syn.is_some());
                    if song.samp[inum].is_none() || !synth {
                        let v = data & 0x7f;
                        if v <= 64 {
                            self.td[t].prev_vol = v * 2;
                            self.ivol[inum] = (v * 2) as u32;
                        }
                    }
                }
            }
            0x0e => {
                if data < 128 {
                    self.td[t].sy.wf_cmd_pos = data as u32;
                    self.td[t].misc |= M_NO_WF_RESET;
                }
            }
            0x0f => match data {
                0x00 => {
                    self.brk = Brk::PatternBreak;
                    self.next_line = 0;
                }
                0xf2 | 0xf4 | 0xf5 => self.delay_retrig(t),
                0xf7 => {}
                0xf8 => set_filter(hw, false),
                0xf9 => set_filter(hw, true),
                0xfd => {
                    if self.td[t].curr_note != 0 {
                        let mut dest = self.td[t].curr_note;
                        if dest < 0x80 {
                            let mut dn = dest as i32 + ss.play_transp as i32 + self.td[t].stransp;
                            while dn >= 0x80 {
                                dn -= 12;
                            }
                            while dn < 1 {
                                dn += 12;
                            }
                            dest = dn as u8;
                        }
                        self.td[t].frequency = self.instr_note_freq(dest, self.td[t].prev_inum) as i32;
                    }
                }
                0xfe => {
                    self.delayed_stop = true;
                    self.songend = true;
                }
                // SoundStudio's mixer plays on an instrument with an output device (measured)
                0xff if self.soft.is_some() && song.instr[self.td[t].prev_inum].outdev != 0 => {}
                0xff => self.mute_track(hw, t),
                v => {
                    if v <= 240 {
                        self.tempo.tempo = (v as u16).max(1);
                    }
                }
            },
            0x15 => {
                if (data as i8) >= -8 && (data as i8) <= 7 {
                    self.td[t].fine_tune = data as i8 as i32;
                }
            }
            0x16 => {
                if data != 0 {
                    if self.repeat_counter == 0 {
                        self.repeat_counter = data as u32;
                    } else {
                        self.repeat_counter -= 1;
                        if self.repeat_counter == 0 {
                            return;
                        }
                    }
                    self.next_line = self.repeat_line;
                    self.brk = Brk::Loop;
                } else {
                    self.repeat_line = line;
                }
            }
            0x19 => self.td[t].soffset = (dataw as u32) << 8,
            0x1d => {
                self.brk = Brk::PatternBreak;
                self.next_line = data as i32;
            }
            0x1e => {
                if self.block_delay == 0 {
                    self.block_delay = data as u32 + 1;
                }
            }
            0x1f => self.delay_retrig(t),
            0x20 => {
                if dataw == 0 {
                    self.td[t].misc |= M_BACKWARDS;
                }
            }
            0x2f => {
                // effect settings: $Ex the echo depth (7 - x)
                if data & 0xf0 == 0xe0 {
                    let dep = 7 - (data & 0xf) as i32;
                    if let (Some(e), true) = (self.echo.as_mut(), (1..=6).contains(&dep)) {
                        e.depth = dep as u8;
                    }
                    if let (Some(e), true) = (self.soft.as_mut().and_then(|s| s.echo.as_mut()), (1..=6).contains(&dep)) {
                        e.1 = dep as u8;
                    }
                }
            }
            0x2e => {
                if (data as i8) >= -16 && (data as i8) <= 16 {
                    self.track_pan[t] = data as i8;
                }
            }
            _ => {}
        }
    }

    fn delay_retrig(&mut self, t: usize) {
        let td = &mut self.td[t];
        td.note_off_cnt = td.init_hold as i32;
        if td.note_off_cnt == 0 {
            td.note_off_cnt = -1;
        }
        td.no_play = true;
    }

    fn every_tick_cmd(&mut self, hw: &mut Hw, ss: &SubSong, t: usize, cmd: Cmd) {
        let data = cmd.d0;
        let pulse = self.pulse;
        let tpl = self.tempo.tpl as i32;
        match cmd.cmd {
            0x00 => {
                if cmd.d1 != 0 {
                    let mut bas = self.td[t].prev_note;
                    if bas > 0x80 {
                        return;
                    }
                    match pulse % 3 {
                        0 => bas = bas.wrapping_add(cmd.d1 >> 4),
                        1 => bas = bas.wrapping_add(cmd.d1 & 0xf),
                        _ => {}
                    }
                    bas = bas.wrapping_add((ss.play_transp as i32 - 1 + self.td[t].stransp) as u8);
                    let f = self.note_freq(bas, self.td[t].fine_tune) as i32;
                    self.td[t].arp_adjust = f - self.td[t].frequency;
                }
            }
            0x11 => {
                if pulse == 0 {
                    self.do_cmd1(t, cmd);
                }
            }
            0x01 => {
                if !(pulse == 0 && ss.slide1st) {
                    self.do_cmd1(t, cmd);
                }
            }
            0x12 => {
                if pulse == 0 {
                    self.do_cmd2(t, cmd);
                }
            }
            0x02 => {
                if !(pulse == 0 && ss.slide1st) {
                    self.do_cmd2(t, cmd);
                }
            }
            0x03 => {
                if !(pulse == 0 && ss.slide1st) {
                    self.do_portamento(t);
                }
            }
            0x0d | 0x0a | 0x06 | 0x05 => {
                if pulse == 0 && ss.slide1st {
                    return;
                }
                let td = &mut self.td[t];
                if data & 0xf0 != 0 {
                    td.prev_vol = td.prev_vol.wrapping_add((data >> 4) * 2);
                    if td.prev_vol > 128 {
                        td.prev_vol = 128;
                    }
                } else if ((data & 0xf) * 2) > td.prev_vol {
                    td.prev_vol = 0;
                } else {
                    td.prev_vol -= (data & 0xf) * 2;
                }
                if cmd.cmd == 0x06 {
                    self.do_vibrato(t);
                } else if cmd.cmd == 0x05 {
                    self.do_portamento(t);
                }
            }
            0x04 => {
                self.td[t].vib_shift = 5;
                self.vib_cont(t, data);
            }
            0x14 => {
                self.td[t].vib_shift = 6;
                self.vib_cont(t, data);
            }
            0x13 => {
                if pulse < 3 {
                    self.td[t].vibr_adjust = -(data as i32);
                }
            }
            0x18 => {
                if pulse == data as i32 {
                    self.td[t].prev_vol = 0;
                }
            }
            0x1a => {
                if pulse == 0 {
                    let incr = data as u32 + (cmd.d1 >= 0x80) as u32;
                    let td = &mut self.td[t];
                    td.prev_vol = if td.prev_vol as u32 + incr < 128 { (td.prev_vol as u32 + incr) as u8 } else { 128 };
                }
            }
            0x1b => {
                if pulse == 0 {
                    let decr = (data as i32 - (cmd.d1 >= 0x80) as i32) as u8;
                    let td = &mut self.td[t];
                    td.prev_vol = if td.prev_vol > decr { td.prev_vol - decr } else { 0 };
                }
            }
            0x0f => match data {
                0xf1 | 0xf2 => {
                    if pulse == 3 {
                        self.play_fx_note(hw, ss, t);
                    }
                }
                0xf3 => {
                    if pulse == 2 || pulse == 4 {
                        self.play_fx_note(hw, ss, t);
                    }
                }
                0xf4 => {
                    if tpl / 3 == pulse {
                        self.play_fx_note(hw, ss, t);
                    }
                }
                0xf5 => {
                    if (tpl * 2) / 3 == pulse {
                        self.play_fx_note(hw, ss, t);
                    }
                }
                _ => {}
            },
            0x1f => {
                if data >> 4 != 0 {
                    if pulse < (data >> 4) as i32 {
                        return;
                    }
                    if pulse == (data >> 4) as i32 {
                        self.play_fx_note(hw, ss, t);
                        return;
                    }
                }
                if data & 0xf != 0 && pulse % (data & 0xf) as i32 == 0 {
                    self.play_fx_note(hw, ss, t);
                }
            }
            0x20 => {
                if pulse == 0 && data != 0 {
                    let c = &self.ch[t];
                    let to = c.pos as i64 + cmd.data() as i64;
                    let len = c.buf.1 as i64;
                    if c.on && len > 0 {
                        let to = if to >= len { 0 } else { to };
                        self.set_position(hw, t, to as u32);
                    }
                }
            }
            0x21 => {
                let td = &mut self.td[t];
                td.frequency += (td.frequency * data as i32) >> 11;
                if td.frequency > 65535 {
                    td.frequency = 65535;
                }
            }
            0x22 => {
                let td = &mut self.td[t];
                if (((td.frequency * data as i32) >> 11) + 1) < td.frequency {
                    td.frequency -= (td.frequency * data as i32) >> 11;
                } else {
                    td.frequency = 1;
                }
            }
            0x29 => {
                if pulse == 0 {
                    let inum = self.td[t].prev_inum;
                    let song = self.song.clone();
                    let mut div = cmd.d1;
                    if div == 0 {
                        div = 0x10;
                    }
                    if let Some(s) = song.samp[inum].as_ref() {
                        if s.syn.is_none() && data < div {
                            let len = self.ch[t].buf.1 as i64;
                            self.set_position(hw, t, ((data as i64 * len) / div as i64) as u32);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn do_cmd1(&mut self, t: usize, cmd: Cmd) {
        let td = &mut self.td[t];
        if td.frequency > 0 {
            let div = 3579545 * 256 / td.frequency - cmd.data() as i32;
            if div > 0 {
                td.frequency = 3579545 * 256 / div;
            }
        }
    }

    fn do_cmd2(&mut self, t: usize, cmd: Cmd) {
        let td = &mut self.td[t];
        if td.frequency > 0 {
            let div = 3579545 * 256 / td.frequency + cmd.data() as i32;
            if div > 0 {
                td.frequency = 3579545 * 256 / div;
            }
        }
    }

    fn do_portamento(&mut self, t: usize) {
        let td = &mut self.td[t];
        if td.port_target == 0 || td.frequency <= 0 {
            return;
        }
        let mut new = td.frequency;
        let mut div = 3579545 * 256 / new;
        if td.frequency > td.port_target {
            div += td.port_speed as i32;
            if div != 0 {
                new = 3579545 * 256 / div;
                if new <= td.port_target {
                    new = td.port_target;
                    td.port_target = 0;
                }
            }
        } else {
            if div > td.port_speed as i32 {
                div -= td.port_speed as i32;
                new = 3579545 * 256 / div;
            } else {
                new = td.port_target;
            }
            if new >= td.port_target {
                new = td.port_target;
                td.port_target = 0;
            }
        }
        td.frequency = new;
    }

    fn vib_cont(&mut self, t: usize, data: u8) {
        if self.pulse == 0 && data != 0 {
            let td = &mut self.td[t];
            if data & 0xf != 0 {
                td.vib_size = data & 0xf;
            }
            if data >> 4 != 0 {
                td.vib_speed = (data >> 4) * 2;
            }
        }
        self.do_vibrato(t);
    }

    fn do_vibrato(&mut self, t: usize) {
        let td = &mut self.td[t];
        if td.frequency > 0 {
            let mut per = 3579545 / td.frequency;
            per += (SINE[((td.vib_offs >> 2) & 0x1f) as usize] as i32 * td.vib_size as i32) >> td.vib_shift;
            if per > 0 {
                td.vibr_adjust = 3579545 / per - td.frequency;
            }
        }
        td.vib_offs = td.vib_offs.wrapping_add(td.vib_speed as u16);
    }

    fn play_fx_note(&mut self, hw: &mut Hw, ss: &SubSong, t: usize) {
        if self.td[t].curr_note != 0 {
            let pulse = self.pulse;
            let td = &mut self.td[t];
            if td.note_off_cnt >= 0 {
                td.note_off_cnt += pulse;
            } else if td.init_hold != 0 {
                td.note_off_cnt = td.init_hold as i32;
            } else {
                td.note_off_cnt = -1;
            }
            let (n, inum) = (td.curr_note.wrapping_sub(1), td.prev_inum);
            self.plr_play_note(hw, ss, t, n, inum);
        }
    }

    fn plr_play_note(&mut self, hw: &mut Hw, ss: &SubSong, t: usize, mut note: u8, inum: usize) {
        let song = self.song.clone();
        let midi = song.instr[inum].midi_ch != 0;
        if song.samp[inum].is_none() && !midi {
            return;
        }
        let ci = &song.instr[inum];
        if ci.flags & I_DISABLED != 0 {
            self.mute_track(hw, t);
            return;
        }
        if note < 0x80 {
            let mut nt = note as i32 + ss.play_transp as i32 + ci.transp as i32;
            while nt >= 0x80 {
                nt -= 12;
            }
            while nt < 0 {
                nt += 12;
            }
            note = nt as u8;
        }
        if midi {
            self.td[t].last_note_midi = true;
            return;
        }
        if t > self.channels {
            return;
        }
        if note < 0x80 {
            while note > 71 {
                note -= 12;
            }
        }
        let freq = self.instr_note_freq(note.wrapping_add(1), inum) as i32;
        let td = &mut self.td[t];
        td.decay = td.init_decay;
        td.fade_speed = 0;
        td.vib_offs = 0;
        td.frequency = freq;
        td.last_note_midi = false;
        let smp = song.samp[inum].as_ref().unwrap();
        if smp.syn.is_some() {
            if t < self.ch.len() && t < self.channels {
                // (SoundStudio's mixer: a synth sound's note on a voice that plays one goes on
                // where the voice is: measured, notes of one and of two synth sounds)
                let plays = smp.frames == 0 && self.td[t].sy.ty == 1 && self.soft.as_ref().and_then(|s| s.v.get(t)).is_some_and(|v| v.on && !v.idle);
                self.ch[t].start_syn = !plays;
            }
            self.td[t].sy.ty = if smp.frames != 0 { 2 } else { 1 };
        } else {
            self.td[t].sy.ty = 0;
        }
        if self.td[t].sy.ty != 1 {
            let (soff, flags) = (self.td[t].soffset, (ci.flags & (I_LOOP | I_PINGPONG)));
            self.play(hw, t, note, inum, soff, (ci.rep, ci.replen), flags);
        }
        if self.td[t].sy.ty != 0 {
            self.clear_synth(t, inum);
            self.td[t].sy.note_number = note;
        }
    }

    fn clear_synth(&mut self, t: usize, inum: usize) {
        let song = self.song.clone();
        let td = &mut self.td[t];
        let no_reset = td.misc & M_NO_WF_RESET != 0;
        let sy = &mut td.sy;
        sy.note_number = 0;
        sy.arp_offs = 0;
        sy.arp_start = 0;
        sy.vol_x_cnt = 0;
        sy.wf_x_cnt = 0;
        sy.vol_cmd_pos = 0;
        if !no_reset {
            sy.wf_cmd_pos = 0;
        }
        sy.vol_wait = 0;
        sy.wf_wait = 0;
        sy.vib_speed = 0;
        sy.vib_dep = 0;
        sy.vib_wf_num = 0;
        sy.vib_offs = 0;
        sy.period_change = 0;
        sy.vol_chg_speed = 0;
        sy.wf_chg_speed = 0;
        if let Some(s) = song.samp[inum].as_ref().and_then(|s| s.syn.as_ref()) {
            sy.vol_x_speed = s.vol_speed;
            sy.wf_x_speed = s.wf_speed;
        }
        sy.env_wf_num = 0;
    }

    fn update_freq_vol_pan(&mut self, hw: &mut Hw, ss: &SubSong, t: usize) {
        if self.td[t].no_play {
            return;
        }
        let song = self.song.clone();
        let inum = self.td[t].prev_inum;
        let base = if self.td[t].sy.ty == 0 {
            self.td[t].frequency
        } else {
            match song.samp[inum].as_ref().and_then(|s| s.syn.as_ref()) {
                Some(snd) => self.synth_handler(hw, t, inum, snd),
                None => {
                    self.td[t].sy.ty = 0;
                    self.mute_track(hw, t);
                    0
                }
            }
        };
        let f = base + self.td[t].arp_adjust + self.td[t].vibr_adjust;
        self.set_freq(hw, t, f);
        let td = &mut self.td[t];
        let used = if td.temp_vol != 0 {
            let u = td.temp_vol - 1;
            td.temp_vol = 0;
            u
        } else {
            td.prev_vol
        };
        let vol = (used as i32 * ss.track_vol[t] as i32 * ss.master_vol) / (64 * 64);
        td.arp_adjust = 0;
        td.vibr_adjust = 0;
        self.set_vol(hw, t, vol as u32);
    }

    fn synth_handler(&mut self, hw: &mut Hw, t: usize, inum: usize, snd: &SynSnd) -> i32 {
        let frames = self.song.samp[inum].as_ref().map_or(0, |s| s.frames);
        let mut set_wf: Option<(u32, u32)> = None;
        let td = &mut self.td[t];
        let sy = &mut td.sy;
        let vtbl = |p: u32| snd.vol_tbl[(p & 0x7f) as usize];
        let wtbl = |p: u32| snd.wf_tbl[(p & 0x7f) as usize];
        let nwf = snd.wfs.len() as u32;
        let cnt = sy.vol_x_cnt;
        sy.vol_x_cnt -= 1;
        if cnt <= 1 {
            sy.vol_x_cnt = sy.vol_x_speed as i32;
            if sy.vol_chg_speed != 0 {
                sy.vol += sy.vol_chg_speed;
                sy.vol = sy.vol.clamp(0, 64);
            }
            if sy.env_wf_num != 0 && !(sy.env_wf_num == 1 && frames != 0) && sy.env_wf_num <= nwf {
                let w = &snd.wfs[(sy.env_wf_num - 1) as usize];
                let v = w.data.get(sy.env_count as usize).copied().unwrap_or(0) as i32;
                sy.vol = (v + 128) >> 2;
                sy.env_count += 1;
                if sy.env_count >= 128 {
                    sy.env_count = 0;
                    if !sy.env_loop {
                        sy.env_wf_num = 0;
                    }
                }
            }
            let run = if sy.vol_wait == 0 {
                true
            } else {
                sy.vol_wait -= 1;
                sy.vol_wait == 0
            };
            if run {
                let mut end = false;
                let mut count = 0;
                while !end {
                    count += 1;
                    if count >= 128 {
                        break;
                    }
                    let cmd = vtbl(sy.vol_cmd_pos);
                    sy.vol_cmd_pos = sy.vol_cmd_pos.wrapping_add(1);
                    if cmd < 0x80 {
                        sy.vol = (cmd as i32).min(64);
                        break;
                    }
                    match cmd {
                        0xfe => {
                            sy.vol_cmd_pos = vtbl(sy.vol_cmd_pos) as u32;
                            if sy.vol_cmd_pos > 127 {
                                sy.vol_cmd_pos = 0;
                            }
                        }
                        0xf0 => {
                            sy.vol_x_speed = vtbl(sy.vol_cmd_pos) as u32;
                            sy.vol_cmd_pos += 1;
                        }
                        0xf1 => {
                            sy.vol_wait = vtbl(sy.vol_cmd_pos) as u32;
                            sy.vol_cmd_pos += 1;
                            end = true;
                        }
                        0xf3 => {
                            sy.vol_chg_speed = vtbl(sy.vol_cmd_pos) as i32;
                            sy.vol_cmd_pos += 1;
                        }
                        0xf2 => {
                            sy.vol_chg_speed = -(vtbl(sy.vol_cmd_pos) as i32);
                            sy.vol_cmd_pos += 1;
                        }
                        0xfa => {
                            sy.wf_wait = 0;
                            sy.wf_cmd_pos = vtbl(sy.vol_cmd_pos) as u32;
                            sy.vol_cmd_pos += 1;
                        }
                        0xf4 | 0xf5 => {
                            sy.env_wf_num = vtbl(sy.vol_cmd_pos) as u32 + 1;
                            sy.vol_cmd_pos += 1;
                            sy.env_loop = cmd == 0xf5;
                            sy.env_count = 0;
                        }
                        0xf6 => sy.env_wf_num = 0,
                        0xff | 0xfb => {
                            sy.vol_cmd_pos = sy.vol_cmd_pos.wrapping_sub(1);
                            end = true;
                        }
                        _ => end = true,
                    }
                }
            }
        }
        td.temp_vol = (((sy.vol * td.prev_vol as i32) >> 6) + 1) as u8;
        let cnt = sy.wf_x_cnt;
        sy.wf_x_cnt -= 1;
        if cnt <= 1 {
            sy.wf_x_cnt = sy.wf_x_speed as i32;
            if sy.wf_chg_speed != 0 {
                sy.period_change += sy.wf_chg_speed;
            }
            let run = if sy.wf_wait == 0 {
                true
            } else {
                sy.wf_wait -= 1;
                sy.wf_wait == 0
            };
            if run {
                let mut end = false;
                let mut count = 0;
                while !end {
                    count += 1;
                    if count >= 128 {
                        break;
                    }
                    let cmd = wtbl(sy.wf_cmd_pos);
                    sy.wf_cmd_pos = sy.wf_cmd_pos.wrapping_add(1);
                    if cmd < 0x80 {
                        if (cmd as u32) < nwf {
                            let w = &snd.wfs[cmd as usize];
                            if !w.data.is_empty() {
                                set_wf = Some((w.addr, w.data.len() as u32));
                            }
                        }
                        break;
                    }
                    match cmd {
                        0xf7 => {
                            sy.vib_wf_num = wtbl(sy.wf_cmd_pos) as u32 + 1;
                            sy.wf_cmd_pos += 1;
                        }
                        0xfe => {
                            sy.wf_cmd_pos = wtbl(sy.wf_cmd_pos) as u32;
                            if sy.wf_cmd_pos > 127 {
                                sy.wf_cmd_pos = 0;
                            }
                        }
                        0xfc => {
                            sy.arp_offs = sy.wf_cmd_pos;
                            sy.arp_start = sy.wf_cmd_pos;
                            let mut guard = 0;
                            while wtbl(sy.wf_cmd_pos) < 0x80 && guard < 256 {
                                sy.wf_cmd_pos += 1;
                                guard += 1;
                            }
                        }
                        0xf0 => {
                            sy.wf_x_speed = wtbl(sy.wf_cmd_pos) as u32;
                            sy.wf_cmd_pos += 1;
                        }
                        0xf1 => {
                            sy.wf_wait = wtbl(sy.wf_cmd_pos) as u32;
                            sy.wf_cmd_pos += 1;
                            end = true;
                        }
                        0xf4 => {
                            sy.vib_dep = wtbl(sy.wf_cmd_pos) as i32;
                            sy.wf_cmd_pos += 1;
                        }
                        0xf5 => {
                            sy.vib_speed = wtbl(sy.wf_cmd_pos) as u32 + 1;
                            sy.wf_cmd_pos += 1;
                        }
                        0xf2 => {
                            sy.wf_chg_speed = wtbl(sy.wf_cmd_pos) as i32;
                            sy.wf_cmd_pos += 1;
                        }
                        0xf3 => {
                            sy.wf_chg_speed = -(wtbl(sy.wf_cmd_pos) as i32);
                            sy.wf_cmd_pos += 1;
                        }
                        0xf6 => sy.period_change = 0,
                        0xfa => {
                            sy.vol_cmd_pos = wtbl(sy.wf_cmd_pos) as u32;
                            sy.wf_cmd_pos += 1;
                            if sy.vol_cmd_pos > 127 {
                                sy.vol_cmd_pos = 0;
                            }
                            sy.vol_wait = 0;
                        }
                        0xff | 0xfb => {
                            sy.wf_cmd_pos = sy.wf_cmd_pos.wrapping_sub(1);
                            end = true;
                        }
                        _ => end = true,
                    }
                }
            }
        }
        let mut cur = td.frequency;
        let (arp_offs, arp_start, note_number, ft) = (sy.arp_offs, sy.arp_start, sy.note_number, td.fine_tune);
        let mut new_arp = None;
        if arp_offs != 0 {
            new_arp = Some(if wtbl(arp_offs + 1) >= 0x80 { arp_start } else { arp_offs + 1 });
            let n = note_number.wrapping_add(wtbl(arp_offs));
            cur = self.freqs[(ft + 8).clamp(0, 15) as usize][(n as usize).min(71)] as i32;
        }
        let sy = &mut self.td[t].sy;
        if let Some(a) = new_arp {
            sy.arp_offs = a;
        }
        let mut change = sy.period_change;
        if sy.vib_dep != 0 && !(sy.vib_wf_num == 1 && frames != 0) && sy.vib_wf_num <= nwf {
            let v = if sy.vib_wf_num != 0 && sy.vib_wf_num <= nwf {
                snd.wfs[(sy.vib_wf_num - 1) as usize].data.get(((sy.vib_offs >> 4) & 0x1f) as usize).copied().unwrap_or(0) as i32
            } else {
                SINE[((sy.vib_offs >> 4) & 0x1f) as usize] as i32
            };
            change += (v * sy.vib_dep) / 256;
            sy.vib_offs = sy.vib_offs.wrapping_add(sy.vib_speed);
        }
        if change != 0 && cur != 0 {
            let mut np = 3579545 / cur + change;
            if np < 113 {
                np = 113;
            }
            cur = 3579545 / np;
        }
        if let Some((a, n)) = set_wf {
            self.set_synth_wf(hw, t, a, n);
        }
        cur
    }
}

/// The MIDI command handler of a MIDI instrument's track: whether it took the
/// command (all of them but a few).
fn midi_command(cmd: Cmd) -> bool {
    match cmd.cmd {
        0x0f => matches!(cmd.d0, 0xfa | 0xfb),
        _ => true,
    }
}

/// The Amiga's LED filter: CIA-A PRA bit 1 set is off.
fn set_filter(hw: &mut Hw, on: bool) {
    let pra = hw.bus.read_byte(0xbfe001);
    hw.bus.write_byte(0xbfe001, if on { pra & !2 } else { pra | 2 });
}

fn verify(p: &mut Pos, ss: &SubSong) {
    bound(&mut p.sec, ss.sections.len());
    bound(&mut p.psq, ss.pseqs.len());
    let n = ss.pseqs.get(p.psq as usize).map_or(0, |s| s.len());
    bound(&mut p.seq, n);
    bound(&mut p.block, ss.blocks.len());
    let lines = ss.blocks[p.block as usize].lines;
    if p.line < 0 {
        p.line = 0;
    } else if p.line as usize >= lines {
        p.line = lines as i32 - 1;
    }
}

/// PlayPosition.AdvanceSongPosition: to play sequence position `new_seq`,
/// through the play sequence commands (`handler`: a stop command ends the
/// song, as the player's handler does).
fn advance_song_pos(p: &mut Pos, ss: &SubSong, new_seq: u32, handler: bool, ev: &mut Ev) {
    let mut jumps = 0;
    let mut roll = 0;
    p.seq = new_seq;
    for _ in 0..100_000 {
        let plen = ss.pseqs.get(p.psq as usize).map_or(0, |s| s.len());
        if p.seq as usize >= plen {
            roll += 1;
            if roll - 1 > 3 {
                p.block = 0;
                break;
            }
            p.seq = 0;
            p.sec += 1;
            if p.sec as usize >= ss.sections.len() {
                p.sec = 0;
                ev.end = true;
            }
            p.psq = ss.sections[p.sec as usize] as u32;
            continue;
        }
        let e = ss.pseqs[p.psq as usize][p.seq as usize];
        if e.cmd != 0 {
            if e.cmd == PS_POSJUMP && jumps < 10 {
                jumps += 1;
                p.seq = e.value;
                continue;
            }
            if handler && e.cmd == PS_STOP {
                ev.stop = true;
            } else {
                p.seq += 1;
                continue;
            }
        }
        p.block = e.value;
        if (p.block as usize) < ss.blocks.len() {
            break;
        }
        p.seq += 1;
    }
    if p.block as usize >= ss.blocks.len() {
        p.block = 0;
    }
}

impl Replayer for OctaMed {
    fn format(&self) -> &'static str {
        self.name
    }

    fn title(&self) -> String {
        self.song.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        if let Some(s) = &self.soft {
            return if s.stage == 0 { SOFT_SETUP_AT } else { 1 << 40 };
        }
        self.tick_clocks_f() as u64
    }

    fn next_tick_at(&self) -> Option<u64> {
        self.soft.as_ref().and_then(|s| s.next_at)
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if ch != 0 || !self.soft.as_ref().is_some_and(|s| s.stage == 2) {
            return false;
        }
        self.soft_irq(hw);
        true
    }

    /// The original's mixer does not fit a 68000's time with a few tracks (it then plays its
    /// buffer late, with gaps); its reference is UADE, a 68020 at full speed.
    fn needs_fast_reference_cpu(&self) -> bool {
        self.soft.is_some()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.song.subs.len() as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.play_song(n as usize);
        self.init_led(hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.soft.is_some() {
            self.soft_tick(hw);
            return;
        }
        if self.first_tick {
            self.first_tick = false;
            hw.voice_dma(u32::MAX >> (32 - self.total), false);
            for v in 0..self.total {
                hw.voice_loc_byte(v, self.silence);
                hw.voice_len(v, 1);
                hw.voice_per(v, 0x71);
                hw.voice_vol(v, 0);
            }
        }
        self.flush_echo(hw);
        self.callback(hw);
    }
}
