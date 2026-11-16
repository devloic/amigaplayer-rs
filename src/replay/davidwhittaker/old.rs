//! David Whittaker's old replayer (Q-Ball), ported from NostalgicPlayer's David
//! Whittaker player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer), its `oldPlayer` paths. The original 68k player
//! crashes on this replayer, so NostalgicPlayer is the only reference: this
//! port does what its C# does and nothing the Eagleplayer's scans would add.
//! Checked 2026-11-16 against NostalgicPlayer's own DavidWhittaker.dll, run tick
//! by tick with a recording channel: dw.Q-Ball's 4 subsongs, 2950 ticks each,
//! 3608 channel events, 0 differences. No original-player reference exists, so
//! this is the weakest verification of the open replayers (refcmp row 2099
//! compares against a fallback reference).
//!
//! What differs from the other builds, as NostalgicPlayer has it:
//! * the module is found by the same byte patterns as the other builds' (the
//!   init routine, the play routine), but the sample code is not a routine of
//!   its own, the position lists and tracks hold 32-bit offsets, and the
//!   channels' volumes, the sample data and the subsong list are read through
//!   `lea x(a3)` references;
//! * a sample is a note's octave: note `n` plays sample `n / 12` at the period
//!   `PERIODS[n % 12]`, whole and once, at the channel's volume (the samples are
//!   stored in the module as a long length, a word frequency and the signed
//!   bytes, one after another); note 0 mutes the channel;
//! * a track byte from $e0 is a duration, and of the other bytes with bit 7 set
//!   only the effects 0 (end of track), 1 (stop the song, which starts it over)
//!   and 2 (no action) exist; no transposes, envelopes, arpeggios, slides or
//!   vibrato, so no frame stuff.
//!
//! Where NostalgicPlayer asks its mixer to play a sample once, this port starts
//! the voice and points its loop at an empty word on the next tick (the
//! player's way in the other builds), and the voice's DMA is switched off a
//! while before it is started again, as Paula stops a voice only at the end of
//! its word. A module that needs more than this port has (the square waveform,
//! envelopes, arpeggios, a voice count other than 1 to 4) is refused, and so is
//! one the C# loader would fail on.

use super::Rd;
use crate::machine::Machine;
use crate::replay::{Hw, Replayer};
use std::collections::HashMap;

/// NostalgicPlayer's `Tables.Periods1`, "used by a real old player which is only
/// used by QBall".
const PERIODS: [u16; 12] = [256, 242, 228, 215, 203, 192, 181, 171, 161, 152, 144, 136];

/// The C# reads the first 16 KiB of the module to identify it.
const SEARCH: i64 = 16384;

/// Colour clocks from a voice's DMA off to its start again (more than a word
/// of the lowest period, 2 * 256), and from the end of a tick's writes.
const T_RESTART: u64 = 700;

/// What the identification finds (offsets in the module).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    start: i64,
    sub_list: i64,
    sample_data: i64,
    chan_vol: i64,
    samples: usize,
    channels: usize,
    delay: bool,
    delay_mul: bool,
    extra: bool,
}

fn sbuf(d: &[u8]) -> Vec<u8> {
    let mut v = d[..d.len().min(SEARCH as usize)].to_vec();
    v.resize(SEARCH as usize, 0);
    v
}

/// The first even index from `from` that satisfies `f`, or `SEARCH`.
fn find(from: i64, to: i64, f: impl Fn(i64) -> bool) -> i64 {
    let mut i = from;
    while i < to {
        if f(i) {
            break;
        }
        i += 2;
    }
    i
}

/// NostalgicPlayer's `ExtractInfoFromInitFunction` (its old player branch) and
/// `ExtractInfoFromPlayFunction`.
fn analyse(d: &[u8]) -> Option<Layout> {
    if d.len() < 2048 || d.starts_with(b"SC68") {
        return None;
    }
    let buf = sbuf(d);
    let r = Rd(&buf);
    let b = |o: i64| r.b(o);
    let sl = SEARCH;

    // the init function
    let mut i = find(0, sl, |i| b(i) == 0x47 && b(i + 1) == 0xfa && b(i + 2) & 0xf0 == 0xf0);
    if i >= sl - 6 {
        return None;
    }
    let start = r.sw(i + 2) + i + 2;
    i = find(i, sl, |i| b(i) == 0x61 && b(i + 1) == 0x00);
    if i >= sl - 8 {
        return None;
    }
    let init = i;
    let mut sample_init = i;
    if b(i + 4) == 0x61 && b(i + 5) == 0x00 {
        sample_init = r.sw(i + 6) + i + 6;
    }
    i = find(sample_init, sl, |i| b(i) == 0x4a && b(i + 1) == 0x2b);
    if i >= sl - 36 {
        return None;
    }
    // a `bne` after the flag test: the other builds
    if b(i + 4) == 0x66 {
        return None;
    }
    // the sample initialisation is in the init routine itself
    i = find(init, sl, |i| b(i) == 0x41 && b(i + 1) == 0xeb);
    if i >= sl - 36 {
        return None;
    }
    let sample_data = r.sw(i + 2) + start;
    i += 4;
    if b(i + 4) != 0x72 {
        return None;
    }
    let samples = b(i + 5) as usize + 1;
    i = find(i, sl - 4, |i| b(i) == 0x41 && b(i + 1) == 0xeb && b(i + 4) == 0xe3 && b(i + 5) == 0x4f);
    if i >= sl - 4 {
        return None;
    }
    let chan_vol = r.sw(i + 2) + start;
    i = find(init, sl, |i| b(i) == 0x41 && b(i + 1) == 0xeb && b(i + 4) == 0x17);
    if i >= sl - 4 {
        return None;
    }
    let sub_list = r.sw(i + 2) + start;

    // the play function
    let mut i = 0;
    while i < sl {
        if b(i) == 0x47 && b(i + 1) == 0xfa {
            if i >= sl - 10 {
                return None;
            }
            if b(i + 4) == 0x4a && b(i + 5) == 0x2b && b(i + 8) == 0x67 {
                let w = (b(i + 10), b(i + 11));
                if w == (0x33, 0xfc) || w == (0x17, 0x7c) || w == (0x08, 0xb9) {
                    i += 2;
                    continue;
                }
                break;
            }
        }
        i += 2;
    }
    if i >= sl {
        return None;
    }
    let play = i;

    // the delay counter
    let mut delay = false;
    let mut delay_mul = false;
    let mut j = play;
    while j < play + 100 {
        if b(j) == 0x10 && b(j + 1) == 0x3a {
            delay = true;
            delay_mul = b(j + 6) == 0xc0 && b(j + 7) == 0xfc;
            break;
        }
        j += 2;
    }
    // the extra counter
    let mut extra = false;
    let mut j = play;
    while j < play + 100 {
        if b(j) == 0x53 && b(j + 1) == 0x2b && b(j + 4) == 0x66 {
            if b(j + 6) == 0x17 {
                let off = (b(j - 4) as i64) << 8 | b(j - 3) as i64;
                extra = b(off + start) != 0;
            }
            break;
        }
        j += 2;
    }
    // the square waveform: not ported (the C# computes its sample from the
    // sample info table, which this build has not)
    let mut j = play;
    while j < play + 100 {
        if b(j) == 0x20 && b(j + 1) == 0x7a && b(j + 4) == 0x30 && b(j + 5) == 0x3a {
            return None;
        }
        j += 2;
    }
    // the number of voices
    let mut channels = 0usize;
    let mut j = play;
    while j < play + 200 {
        if b(j) == 0x7e {
            channels = b(j + 1) as usize;
            if channels == 0 {
                let mut k = j;
                while k < play + 500 {
                    if b(k) == 0xbe && (b(k + 1) == 0x7c || b(k + 1) == 0x3c) {
                        channels = b(k + 3) as usize;
                        break;
                    }
                    k += 2;
                }
            } else {
                channels += 1;
            }
            break;
        }
        j += 2;
    }
    if channels == 0 || channels > 4 {
        return None;
    }
    // the track command reader
    let rt = find(play, sl, |i| b(i) == 0x70 && b(i + 1) == 0x00);
    if rt >= sl || b(rt + 2) != 0x10 {
        return None;
    }
    let bm = find(rt, sl, |i| b(i) == 0x6b && b(i + 1) == 0x00);
    if bm >= sl - 6 {
        return None;
    }
    let cmds = ((b(bm + 2) as i64) << 8 | b(bm + 3) as i64) + bm + 2;
    // the command ranges: an old build has none of arpeggios and envelopes
    let mut j = cmds;
    while j < sl - 28 {
        if b(j) == 0x4e && (b(j + 1) == 0xd2 || b(j + 1) == 0xf3) {
            break;
        }
        if (b(j) == 0xb0 && b(j + 1) == 0x3c) || (b(j) == 0x0c && b(j + 1) == 0x00) {
            if b(j + 2) == 0x00
                && (b(j + 4) == 0x65 || b(j + 4) == 0x6d)
                && b(j + 10) == 0xd0
                && b(j + 11) == 0x40
                && b(j + 12) == 0x45
                && b(j + 13) == 0xfa
                && b(j + 22) == 0x21
                && b(j + 23) == 0x4a
                && ((b(j + 26) == 0x21 && b(j + 27) == 0x4a) || (b(j + 26) == 0x11 && b(j + 27) == 0x6a))
            {
                return None;
            }
        }
        j += 2;
    }
    // the effect jump table
    let ok = (b(j - 10) == 0x45 && b(j - 9) == 0xfa) || (b(j - 8) == 0x45 && b(j - 7) == 0xfa) || (b(j - 10) == 0x45 && b(j - 9) == 0xeb);
    if !ok {
        return None;
    }
    Some(Layout { start, sub_list, sample_data, chan_vol, samples, channels, delay, delay_mul, extra })
}

/// A cursor over the module as NostalgicPlayer's `ModuleStream`: a read that
/// comes up short sets `eos`, and so does a seek past the end.
struct St<'a> {
    d: &'a [u8],
    pos: i64,
    eos: bool,
}

impl St<'_> {
    fn seek(&mut self, p: i64) {
        self.pos = p;
        self.eos = p > self.d.len() as i64;
    }
    fn read(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        let mut got = 0;
        for k in 0..n {
            let p = self.pos + k as i64;
            if p >= 0 && (p as usize) < self.d.len() {
                v = v << 8 | self.d[p as usize] as u32;
                got += 1;
            } else {
                v <<= 8;
            }
        }
        self.eos = got < n;
        self.pos += got as i64;
        v
    }
    fn u8(&mut self) -> u8 {
        self.read(1) as u8
    }
    fn u16(&mut self) -> u16 {
        self.read(2) as u16
    }
    fn u32(&mut self) -> u32 {
        self.read(4)
    }
}

/// A sample: where its signed bytes are in the module, and how many.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    off: u32,
    len: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct PositionList {
    /// Indices of tracks.
    tracks: Vec<usize>,
    restart: u16,
}

#[derive(Clone, Debug, PartialEq)]
struct SongInfo {
    speed: u16,
    delay_speed: u8,
    lists: Vec<PositionList>,
}

/// What NostalgicPlayer's loader reads.
#[derive(Debug, PartialEq)]
struct Module {
    lay: Layout,
    songs: Vec<SongInfo>,
    tracks: Vec<Vec<u8>>,
    volumes: Vec<u16>,
    samples: Vec<Sample>,
}

/// `FindEffectByteCount` for the old player: -1 stops, and an effect it does not
/// know makes the C# loader throw.
fn effect_bytes(effect: u8) -> Option<i32> {
    match effect {
        0 | 1 => Some(-1),
        2 => Some(1),
        _ => None,
    }
}

/// `LoadTrack`.
fn load_track(st: &mut St, off: u32, start: i64) -> Option<Vec<u8>> {
    st.seek(off as i64 + start);
    let mut t = Vec::new();
    loop {
        let byt = st.u8();
        if st.eos {
            return None;
        }
        t.push(byt);
        if byt & 0x80 != 0 {
            if byt >= 0xe0 {
                continue;
            }
            let n = effect_bytes(byt & 0x7f)?;
            if n == -1 {
                break;
            }
            for _ in 0..n {
                let v = st.u8();
                t.push(v);
            }
        }
    }
    Some(t)
}

/// `LoadPositionList`: the old player's lists have no restart position.
fn load_position_list(st: &mut St, start: u32, base: i64, tracks: &mut Vec<Vec<u8>>, index: &mut HashMap<u32, usize>) -> Option<PositionList> {
    if start == 0 {
        return None;
    }
    st.seek(start as i64 + base);
    let mut list = Vec::new();
    loop {
        let off = st.u32();
        if off == 0 || off as i64 >= st.d.len() as i64 || off & 0x8000 != 0 {
            break;
        }
        if st.eos {
            return None;
        }
        let cur = st.pos;
        let ix = match index.get(&off) {
            Some(&i) => i,
            None => {
                let t = load_track(st, off, base)?;
                tracks.push(t);
                index.insert(off, tracks.len() - 1);
                tracks.len() - 1
            }
        };
        list.push(ix);
        st.seek(cur);
    }
    Some(PositionList { tracks: list, restart: 0 })
}

/// NostalgicPlayer's loader: the subsong list with its position lists and
/// tracks, the channel volumes, and the sample table.
fn load(d: &[u8]) -> Option<Module> {
    let lay = analyse(d)?;
    let mut st = St { d, pos: 0, eos: false };
    let mut tracks = Vec::new();
    let mut index = HashMap::new();
    let mut songs: Vec<SongInfo> = Vec::new();
    st.seek(lay.sub_list);
    let mut min_pos = i64::MAX;
    loop {
        if st.pos + 8 >= min_pos {
            break;
        }
        let (speed, delay_speed) = if lay.delay {
            let s = st.u8() as u16;
            (s, st.u8())
        } else {
            (st.u16(), 0)
        };
        if speed > 255 {
            break;
        }
        let mut offs = Vec::new();
        for _ in 0..lay.channels {
            let o = st.u32();
            min_pos = min_pos.min(o as i64 + lay.start);
            offs.push(o);
        }
        if st.eos {
            return None;
        }
        let cur = st.pos;
        let mut lists = Vec::new();
        for &o in &offs {
            lists.push(load_position_list(&mut st, o, lay.start, &mut tracks, &mut index)?);
        }
        songs.push(SongInfo { speed, delay_speed, lists });
        st.seek(cur);
    }
    if songs.is_empty() {
        return None;
    }
    // the channel volumes
    st.seek(lay.chan_vol);
    let volumes: Vec<u16> = (0..lay.channels).map(|_| st.u16()).collect();
    if st.eos {
        return None;
    }
    // the samples: a long length, a word frequency, the data
    st.seek(lay.sample_data);
    let mut samples = Vec::new();
    for _ in 0..lay.samples {
        let len = st.u32();
        let freq = st.u16();
        if freq == 0 {
            // the C# divides by it
            return None;
        }
        let off = st.pos;
        if off + len as i64 > d.len() as i64 {
            return None;
        }
        samples.push(Sample { off: off as u32, len });
        st.seek(off + len as i64);
    }
    Some(Module { lay, songs, tracks, volumes, samples })
}

pub fn detect(d: &[u8]) -> bool {
    load(d).is_some()
}

/// `ChannelInfo` (the fields the old player uses).
#[derive(Clone, Debug)]
struct Chan {
    list: usize,
    pos: u16,
    restart: u16,
    track: Option<usize>,
    tpos: usize,
    speed: u16,
    counter: u16,
    note: u8,
}

/// What a tick asked of a channel, as the IChannel calls say it.
#[derive(Clone, Copy, Default)]
struct Want {
    /// A sample to play (its number).
    play: Option<usize>,
    mute: bool,
    period: Option<u16>,
    vol: Option<u16>,
}

pub struct Old {
    m: Module,
    /// The module in guest memory, and the empty word voices end on.
    base: u32,
    silent: u32,
    /// Where each sample's bytes are in guest memory.
    addr: Vec<u32>,
    sub: u16,
    // GlobalPlayingInfo
    speed: u16,
    extra: u8,
    delay_speed: u8,
    delay: u16,
    ch: Vec<Chan>,
    want: [Want; 4],
    /// Voices started on the previous tick: their loop is not written yet.
    pending: u8,
    /// Whether a voice's DMA is on (for the tick's off mask).
    on: u8,
}

pub fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let module = load(d)?;
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    // Paula plays from even addresses only: an odd sample is copied
    let mut addr = Vec::new();
    for s in &module.samples {
        let a = base.wrapping_add(s.off);
        if a & 1 == 0 || s.len == 0 {
            addr.push(a);
        } else {
            let c = m.alloc(s.len + 2);
            if c == 0 {
                return None;
            }
            for k in 0..s.len {
                let v = m.bus.r8(a + k);
                m.bus.w8(c + k, v);
            }
            addr.push(c);
        }
    }
    let mut w = Old {
        m: module,
        base,
        silent,
        addr,
        sub: 0,
        speed: 0,
        extra: 0,
        delay_speed: 0,
        delay: 0,
        ch: Vec::new(),
        want: [Want::default(); 4],
        pending: 0,
        on: 0,
    };
    w.init_sound(0, &mut Hw { bus: &mut m.bus });
    Some(Box::new(w))
}

impl Replayer for Old {
    fn format(&self) -> &'static str {
        "David Whittaker"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        false
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.m.songs.len() as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.init_sound(n.min(self.m.songs.len() as u16 - 1), hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.play();
        self.flush(hw);
    }
}

impl Old {
    /// `InitializeSound`, the voices silent.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        hw.dmacon(0x000f);
        self.on = 0;
        self.pending = 0;
        self.want = [Want::default(); 4];
        self.reset(n);
    }

    fn reset(&mut self, n: u16) {
        let song = &self.m.songs[n as usize];
        self.speed = song.speed;
        self.extra = 1;
        self.delay = 0;
        self.delay_speed = song.delay_speed;
        if self.m.lay.delay_mul {
            self.delay_speed = self.delay_speed.wrapping_mul(16);
        }
        self.init_channels(n);
    }

    /// `InitializeChannelInfo`.
    fn init_channels(&mut self, n: u16) {
        let song = &self.m.songs[n as usize];
        self.ch = song
            .lists
            .iter()
            .enumerate()
            .map(|(i, l)| Chan { list: i, pos: 1, restart: l.restart, track: l.tracks.first().copied(), tpos: 0, speed: 0, counter: 1, note: 0 })
            .collect();
    }

    /// `Play`.
    fn play(&mut self) {
        let lay = self.m.lay;
        if lay.delay {
            self.delay += self.delay_speed as u16;
            if self.delay > 255 {
                self.delay -= 256;
                return;
            }
        }
        if lay.extra {
            self.extra = self.extra.wrapping_sub(1);
            if self.extra == 0 {
                self.extra = 6;
                return;
            }
        }
        for i in 0..lay.channels {
            // the voice as the loop found it: a stop in an earlier voice
            // replaces the voices that follow, and this one is not kept
            let mut c = self.ch[i].clone();
            c.counter = c.counter.wrapping_sub(1);
            if c.counter == 0 {
                let stale = self.read_track_commands(&mut c, i);
                if !stale {
                    self.ch[i] = c;
                }
            } else {
                self.ch[i] = c;
            }
            // (frame stuff: the old player has none, its voices have no sample info)
        }
    }

    /// `ReadTrackCommands`; true when the voices were replaced by a stop.
    fn read_track_commands(&mut self, c: &mut Chan, n: usize) -> bool {
        let mut stopped = false;
        // a track of nothing but end-of-track bytes would not end in the C#
        for _ in 0..0x10000 {
            let byt = match c.track.and_then(|t| self.m.tracks[t].get(c.tpos)) {
                Some(&b) => b,
                // the empty track: end of track
                None if c.track.is_none() => 0x80,
                // read past the track: nothing in the C# does that
                None => 0x80,
            };
            c.tpos += 1;
            if byt & 0x80 != 0 {
                let (brk, st) = self.do_track_command(c, n, byt);
                stopped |= st;
                if brk {
                    break;
                }
            } else {
                // play the note
                c.note = byt;
                let sample = byt as usize / 12;
                let note = byt as usize % 12;
                if let Some(s) = self.m.samples.get(sample) {
                    let _ = s;
                    if c.note != 0 {
                        self.want[n].mute = false;
                        self.want[n].period = Some(PERIODS[note]);
                        self.want[n].play = Some(sample);
                        self.want[n].vol = Some(self.m.volumes[n]);
                    } else {
                        self.want[n] = Want { mute: true, ..Want::default() };
                    }
                }
                break;
            }
        }
        c.counter = c.speed;
        stopped
    }

    /// `DoTrackCommand` for the old player: (stop reading, the voices were replaced).
    fn do_track_command(&mut self, c: &mut Chan, n: usize, cmd: u8) -> (bool, bool) {
        if cmd >= 0xe0 {
            c.speed = (cmd as u16 - 0xdf).wrapping_mul(self.speed);
            return (false, false);
        }
        match cmd & 0x7f {
            0 => {
                self.end_of_track(c);
                (false, false)
            }
            1 => {
                self.stop_module();
                (true, true)
            }
            // 2: no action (its byte is read as the next command, as the C# does)
            _ => {
                let _ = n;
                (false, false)
            }
        }
    }

    /// `HandleEndOfTrackEffect`.
    fn end_of_track(&mut self, c: &mut Chan) {
        let len = self.m.songs[self.sub as usize].lists[c.list].tracks.len() as u16;
        if c.pos >= len {
            c.pos = c.restart.wrapping_add(1);
        } else {
            c.pos += 1;
        }
        let l = &self.m.songs[self.sub as usize].lists[c.list];
        c.track = if l.tracks.is_empty() { None } else { l.tracks.get(c.pos as usize - 1).copied() };
        c.tpos = 0;
    }

    /// `StopModule`: all voices muted, the song started over.
    fn stop_module(&mut self) {
        for w in self.want.iter_mut() {
            *w = Want { mute: true, ..Want::default() };
        }
        let n = self.sub;
        self.speed = self.m.songs[n as usize].speed;
        self.init_channels(n);
    }

    /// The tick's calls on Paula.
    fn flush(&mut self, hw: &mut Hw) {
        // the loops of the voices started on the tick before: the empty word
        for ch in 0..4 {
            if self.pending & 1 << ch != 0 {
                hw.loc(ch, self.silent);
                hw.len(ch, 1);
            }
        }
        self.pending = 0;
        let mut off = 0u16;
        let mut start = 0u16;
        for ch in 0..4 {
            let w = self.want[ch];
            if w.play.is_some() {
                start |= 1 << ch;
                if self.on & 1 << ch != 0 {
                    off |= 1 << ch;
                }
            } else if w.mute && self.on & 1 << ch != 0 {
                off |= 1 << ch;
                self.on &= !(1 << ch);
            }
        }
        if off != 0 {
            hw.dmacon(off);
            if start & off != 0 {
                hw.wait(T_RESTART);
            }
        }
        for ch in 0..4 {
            let w = self.want[ch];
            if let Some(s) = w.play {
                let len = self.m.samples[s].len;
                let (a, words) = if len < 2 { (self.silent, 1) } else { (self.addr[s], (len / 2).min(0xffff) as u16) };
                hw.loc(ch, a);
                hw.len(ch, words);
                hw.per(ch, w.period.unwrap_or(0));
                hw.vol(ch, w.vol.unwrap_or(0).min(64));
            }
        }
        if start != 0 {
            hw.dmacon(0x8000 | start);
            self.on |= start as u8;
            self.pending = start as u8;
        }
        self.want = [Want::default(); 4];
        let _ = self.base;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module of the old build, as far as the C# identifies and loads one:
    /// the init routine, play routine, track reader and effect table in the
    /// code's byte patterns, the tables behind them.
    fn synthetic() -> Vec<u8> {
        let mut d = vec![0u8; 3000];
        let mut put = |o: usize, b: &[u8]| d[o..o + b.len()].copy_from_slice(b);
        // init: lea 0(pc),a3 (the replayer's base) / bsr.w / lea sample table, moveq #4,d1 ... lea voice volumes,
        // lea subsong list
        put(0x20, &[0x47, 0xfa, 0xff, 0xde]); // start = 0x20 + 2 - 0x22 = 0
        put(0x24, &[0x61, 0x00, 0x00, 0x10, 0x4e, 0x71, 0x4e, 0x71]);
        put(0x30, &[0x4a, 0x2b, 0x00, 0x01, 0x4e, 0x71]); // tst.b 1(a3); no bne
        put(0x40, &[0x41, 0xeb, 0x04, 0x00, 0x4e, 0x71, 0x4e, 0x71, 0x72, 0x04]); // lea $400(a3),a0 ... moveq #4,d1 (5 samples)
        put(0x50, &[0x41, 0xeb, 0x03, 0x00, 0xe3, 0x4f]); // lea $300(a3),a0 ; lsl.w #1,d7
        put(0x60, &[0x41, 0xeb, 0x02, 0x00, 0x17, 0x40]); // lea $200(a3),a0 ; move.b d0,..
        // play: lea 0(pc),a3 / tst.b / beq
        put(0x100, &[0x47, 0xfa, 0xfe, 0x00, 0x4a, 0x2b, 0x00, 0x00, 0x67, 0x00, 0x4e, 0x71]);
        put(0x110, &[0x7e, 0x03]); // moveq #3,d7: 4 voices
        put(0x140, &[0x70, 0x00, 0x10, 0x19]); // moveq #0,d0 / move.b (a1)+,d0
        put(0x150, &[0x6b, 0x00, 0x00, 0x40]); // bmi.w cmds
        put(0x190, &[0x45, 0xfa, 0x00, 0x10]); // lea jump table
        put(0x198, &[0x4e, 0xd2]); // jmp (a2)
        // subsong list at $200: speed word 3, four 32-bit position list offsets
        put(0x200, &[0, 3]);
        for (i, o) in [0x240u32, 0x250, 0x260, 0x270].iter().enumerate() {
            put(0x202 + 4 * i, &o.to_be_bytes());
        }
        // a second entry that stops the list (its position lists would start before it)
        put(0x212, &[1, 0]);
        // position lists: one track each, then 0
        for (i, o) in [0x240usize, 0x250, 0x260, 0x270].iter().enumerate() {
            put(*o, &(0x280u32 + 0x10 * i as u32).to_be_bytes());
        }
        // tracks: a duration ($e1: 2 rows), a note, end of track
        for i in 0..4 {
            put(0x280 + 0x10 * i, &[0xe1, 13 + i as u8, 0x80]);
        }
        // channel volumes at $300
        for (i, v) in [64u16, 40, 40, 64].iter().enumerate() {
            put(0x300 + 2 * i, &v.to_be_bytes());
        }
        // samples at $400: 5 x (len 8, freq 8363, 8 bytes)
        let mut o = 0x400;
        for i in 0..5 {
            put(o, &8u32.to_be_bytes());
            put(o + 4, &8363u16.to_be_bytes());
            put(o + 6, &[(i * 3) as u8; 8]);
            o += 14;
        }
        d
    }

    #[test]
    fn identifies_the_old_replayer() {
        let d = synthetic();
        let lay = analyse(&d).expect("analyse");
        assert_eq!(lay.channels, 4);
        assert_eq!(lay.samples, 5);
        assert_eq!(lay.sub_list, 0x200);
        assert_eq!(lay.sample_data, 0x400);
        assert_eq!(lay.chan_vol, 0x300);
        let m = load(&d).expect("load");
        assert_eq!(m.songs.len(), 1);
        assert_eq!(m.songs[0].speed, 3);
        assert_eq!(m.tracks.len(), 4);
        assert_eq!(m.volumes, vec![64, 40, 40, 64]);
        assert_eq!(m.samples.len(), 5);
        assert_eq!(m.samples[1], Sample { off: 0x400 + 14 + 6, len: 8 });
        // the other builds' code is not claimed
        assert!(analyse(&[0u8; 4096]).is_none());
    }

    #[test]
    fn plays_notes_as_octave_samples() {
        let d = synthetic();
        let m = load(&d).unwrap();
        let mut p = Old {
            m,
            base: 0,
            silent: 0x1000,
            addr: Vec::new(),
            sub: 0,
            speed: 0,
            extra: 0,
            delay_speed: 0,
            delay: 0,
            ch: Vec::new(),
            want: [Want::default(); 4],
            pending: 0,
            on: 0,
        };
        p.reset(0);
        p.play();
        // note 13 = sample 1, note 1: period 242, the channel's volume
        for ch in 0..4 {
            let w = p.want[ch];
            assert_eq!(w.play, Some(1), "ch {ch}");
            assert_eq!(w.period, Some(PERIODS[1 + ch]));
        }
        assert_eq!(p.want[1].vol, Some(40));
        // the row is 2 speeds long: nothing until it is over
        p.want = [Want::default(); 4];
        for _ in 0..5 {
            p.play();
            assert!(p.want.iter().all(|w| w.play.is_none() && !w.mute));
        }
    }

    /// The synthetic module in guest memory, ticked: the notes reach Paula as
    /// the C# asks for them (a sample, its period and volume, DMA on), and
    /// the voice's loop points at the empty word on the tick after.
    #[test]
    fn renders_notes_to_paula() {
        use m68k::CpuType;
        let d = synthetic();
        let mut m = Machine::new(CpuType::M68000);
        let base = m.alloc(d.len() as u32 + 16);
        assert!(base != 0);
        for (k, &b) in d.iter().enumerate() {
            m.bus.w8(base + k as u32, b);
        }
        let mut r = new(&d, base, &mut m).expect("new");
        assert_eq!(r.subsongs(), (0, 0, 0));
        m.bus.write_log = Some(Vec::new());
        r.tick(&mut Hw { bus: &mut m.bus });
        let l = m.bus.write_log.take().unwrap();
        let w = |a: u32| l.iter().filter(|e| e.0 == a).map(|e| e.1).collect::<Vec<_>>();
        // voice 1 (note 14 = sample 1, period 228, volume 40), the sample's bytes after its header
        assert_eq!(w(0xDFF0B6), vec![228]);
        assert_eq!(w(0xDFF0B8), vec![40]);
        assert_eq!(w(0xDFF0B4), vec![4]);
        let loc = (w(0xDFF0B0)[0] as u32) << 16 | w(0xDFF0B2)[0] as u32;
        assert_eq!(loc, base + 0x400 + 14 + 6);
        assert_eq!(w(0xDFF096).last(), Some(&0x800f));
        // the next tick: the loop is the empty word
        m.bus.write_log = Some(Vec::new());
        r.tick(&mut Hw { bus: &mut m.bus });
        let l = m.bus.write_log.take().unwrap();
        let w = |a: u32| l.iter().filter(|e| e.0 == a).map(|e| e.1).collect::<Vec<_>>();
        let loc = (w(0xDFF0B0)[0] as u32) << 16 | w(0xDFF0B2)[0] as u32;
        assert!(loc != base + 0x400 + 14 + 6 && loc != 0);
        assert_eq!(w(0xDFF0B4), vec![1]);
    }

    /// `DW_QBALL=/path/to/dw.Q-Ball cargo test old_real_module`: the real file
    /// (not in the repository) loads as NostalgicPlayer loads it.
    #[test]
    fn old_real_module() {
        let Ok(p) = std::env::var("DW_QBALL") else { return };
        let d = std::fs::read(p).unwrap();
        let m = load(&d).expect("load");
        assert_eq!((m.songs.len(), m.samples.len(), m.lay.channels), (4, 5, 4));
        assert_eq!(m.samples.iter().map(|s| s.len).collect::<Vec<_>>(), vec![7436, 6350, 6998, 8502, m.samples[4].len]);
        assert_eq!(m.volumes, vec![64, 40, 40, 64]);
    }
}
