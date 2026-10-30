//! OctaMED SoundStudio (MMD3) and OctaMED's 5-8 channel and mix mode songs
//! (MMD0-MMD2 with the 8-channel or mix flag), ported from NostalgicPlayer's
//! OctaMED player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer): the module loader (MMD0-MMD3, the old modules'
//! conversion to mix mode and the 5-8 channel tempo conversion), the player's
//! tick (rows, hold and decay, the effect commands, synth and hybrid sounds)
//! and its frequency table.
//!
//! The original plays these songs by mixing in software (OctaMED SoundStudio's
//! mix mode), which no Paula channel does, so the voices go to the host's
//! amplifier (`Hw::voice_*`, as HivelyTracker's and the multichannel MODs' do):
//! one voice a track, from the Amiga's LRRL sides, at most 32. A note is the
//! voice's DMA off, the sample's start and length, its period (from the note's
//! frequency in Hz), the volume, DMA on, and then the loop. What the amplifier
//! has no counterpart of is approximated or left out; see the README.
//!
//! - 16-bit and stereo samples are converted to 8-bit mono when the module is
//!   loaded (a 16-bit sample keeps its high byte, a stereo one its two channels'
//!   mean), in the module's own memory where that does not touch another sample.
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
//!   The voices mix at the amplifier's rate, 31 kHz; the original's is not
//!   known.
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
}

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
        return Some(Dec { frames, stype, bufs: Vec::new(), raw_len: 0, plain: true });
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
    for _ in 0..chans {
        for (o, s) in sizes.iter().enumerate() {
            let fr = (*s / dl as u64) as usize;
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
    Some(Dec { frames, stype, bufs, raw_len, plain: !b16 && !stereo && !delta })
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
            name: String::new(),
        };
        // the instruments of the song structure
        for i in 0..MAX_INSTR - 1 {
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
            for i in 0..it_n.min(MAX_INSTR) {
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
        if !has_loop {
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
                if smp.syn.is_some() {
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
    }
    let _ = kind;
    Some(Song { instr, samp, subs, title })
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

pub struct OctaMed {
    name: &'static str,
    /// An MMD0-MMD2 song.
    old: bool,
    /// The voices of the amplifier: the tracks', and the echo's after them.
    total: usize,
    echo: Option<EchoState>,
    song: Arc<Song>,
    sub: usize,
    freqs: Vec<[u16; 72]>,
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
    let song = load(d, base, m, kind)?;
    let nvoices = song.subs.iter().map(|s| s.channels).max().unwrap_or(4).clamp(1, MAX_VOICES);
    let silence = m.alloc(4);
    // the echo's voices follow the tracks' (on the same sides: from a multiple
    // of 4), if they fit
    let echoes = song.subs.iter().any(|s| s.echo.is_some());
    let eb = (nvoices + 3) & !3;
    let total = if echoes && eb + ((nvoices + 1) & !1) <= MAX_VOICES { eb + ((nvoices + 1) & !1) } else { nvoices };
    if silence == 0 || !start_amplifier(m, total) {
        return None;
    }
    // the mix rate: the highest the emulated Paula plays one byte a channel at
    // (its DMA reads a word a line), 31 kHz, not the 25 kHz or so the amplifier's
    // default is for this many voices (the original's rate is not known). The level
    // stays the amplifier's, which keeps every voice of a side at full volume
    // within the 8-bit stage (some of these songs come near it, and the same
    // songs at twice the level, tried first, clipped); the echo's voices, which sound
    // less, are not to make it quieter.
    if let Some(a) = m.bus.amp.as_mut() {
        a.period = MIX_PERIOD;
        if total > nvoices {
            a.boost = a.boost * total as u32 / nvoices as u32;
        }
    }
    crate::amplifier::restart(&mut m.bus);
    let mut p = OctaMed {
        old: kind == Kind::Old,
        total,
        echo: None,
        name: if kind == Kind::Oss { "OctaMED SoundStudio" } else { "OctaMED" },
        song: Arc::new(song),
        sub: 0,
        freqs: freq_table(),
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
                        while n > 60 {
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
        let per = Self::per_of(freq);
        if self.ch[t].per != per {
            self.ch[t].per = per;
            self.vp(hw, t, Op::Per(per));
        }
    }

    fn set_vol(&mut self, hw: &mut Hw, t: usize, vol: u32) {
        if t >= self.channels || t >= self.nvoices {
            return;
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
                self.ch[t].start_syn = true;
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
        self.tick_clocks_f() as u64
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
