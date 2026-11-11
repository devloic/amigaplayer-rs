//! Face The Music (`FTMN`, `.ftm`), ported from NostalgicPlayer's FaceTheMusic player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Face The Music has eight tracks, with sound effect scripts and LFOs on them. The
//! Eagleplayer ("Face The Music") plays them on Paula's four channels as pairs of tracks
//! (tracks 0 and 1 on channel 0, and so on: left left right right right right left left),
//! which it mixes itself: NostalgicPlayer has a mixer of its own, so the song is
//! NostalgicPlayer's and the mixing is the Eagleplayer's, taken from its register writes
//! and from the bytes Paula fetched in its runs (docs/specs/facethemusic/SPEC.md).
//! The samples play from the module in guest memory (modules with external samples are
//! not played).

use super::facethemusic_tables::{LFO_SHAPES, PERIODS};
use super::{be16, be32, Hw, Open, Replayer};
use crate::bus::Bus;
use m68k::AddressBus;
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Face The Music", detect, new };

// the track effects
const SEL_EFFECT: u8 = 11;
const PORTAMENTO: u8 = 12;
const VOLUME_DOWN: u8 = 13;
const PATTERN_LOOP: u8 = 14;
const SKIP_EMPTY_ROWS: u8 = 15;

/// The volume of each track volume effect, as the Eagleplayer plays them: its third is silent
/// and its ninth 56 (measured on the channel volumes of 20 modules: under the global volume 63
/// the channels never show the 13 and 56 of the even steps 14 and 57, but 0 and 55).
const EFFECT_VOLUME: [u8; 10] = [0, 7, 0, 21, 28, 36, 43, 50, 56, 64];

/// Bytes of a mixing buffer: 100 words a buffer.
const BUF: u32 = 200;

/// The sample played when none is: index in `samples`.
const QUIET: usize = 64;

#[derive(Clone, Copy, Default)]
struct TLine {
    effect: u8,
    arg: u16,
    note: u8,
}

struct Track {
    spacing: u16,
    lines: Vec<TLine>,
}

#[derive(Clone, Copy, Default)]
struct SelLine {
    effect: u8,
    a1: u8,
    a2: u16,
}

#[derive(Clone, Default)]
struct Samp {
    named: bool,
    /// Words.
    oneshot: u16,
    /// Bytes.
    loop_start: u32,
    /// Words.
    loop_len: u16,
    /// Bytes.
    total: u32,
    /// Guest address of the data.
    addr: u32,
}

#[derive(Clone, Default)]
struct Sel {
    script: Option<usize>,
    pos: i32,
    wait: u16,
    loop_counter: u16,
    /// The voice the script works on.
    voice: usize,
    new_pitch: u16,
    new_volume: u16,
    new_sample: u16,
    release: u16,
    portamento: u16,
    volume_down: u16,
    interrupt: u16,
}

#[derive(Clone, Copy, Default)]
struct Lfo {
    target: u8,
    loop_mod: bool,
    shape: usize,
    shape_pos: i32,
    speed: u16,
    depth: u16,
    value: i16,
}

#[derive(Clone)]
struct Voice {
    chan: usize,
    cur_sample: usize,
    sample_number: i16,
    /// Guest address of the sample the voice plays.
    data: u32,
    start_offset: u32,
    calc_offset: u32,
    oneshot: u16,
    loop_start: u32,
    loop_len: u16,
    total: u32,
    volume: u16,
    note_index: u16,
    retrig: bool,
    detune_index: usize,
    track: usize,
    track_pos: u32,
    rows_left: i16,
    port_ticks: i16,
    port_note: u32,
    port_end: u16,
    vd_volume: u32,
    vd_speed: u16,
    sel: Sel,
    lfo: [Lfo; 4],
}

#[derive(Clone)]
struct LoopInfo {
    track_pos: u32,
    start: u16,
    count: i16,
    original: i16,
}

pub struct FaceTheMusic {
    title: String,
    rows_per_measure: u8,
    mute_status: u8,
    samples: Vec<Samp>,
    scripts: Vec<Option<Vec<SelLine>>>,
    tracks: Vec<Track>,
    quiet: u32,
    // global playing info
    start_row: u16,
    end_row: u16,
    gvol: u8,
    speed: u16,
    loop_stack: Vec<LoopInfo>,
    loop_stop_row: u16,
    loop_start_row: u16,
    do_loop: bool,
    speed_counter: u16,
    row: u16,
    detune: [i16; 4],
    voices: Vec<Voice>,
    cia: u16,
    end_reached: bool,
    songend: bool,
    visited: Vec<bool>,
    // the mixer
    /// Each track's period and volume as the last tick left them.
    per: [u16; 8],
    vol: [u16; 8],
    /// Where each track's sample plays.
    play: [Play; 8],
    /// Each channel's two buffers and which of them is mixed next.
    bufs: [[u32; 2]; 4],
    flip: [usize; 4],
    /// The period and volume of each channel's buffer mixed last: written at the next
    /// audio interrupt, of any channel.
    ch_per: [u16; 4],
    ch_vol: [u16; 4],
}

/// A track's sample in the mixer: the byte played next, the end of the block, the fraction of
/// a byte passed (16 bits), and the loop (0 bytes: none).
#[derive(Clone, Copy, Default)]
struct Play {
    cur: u32,
    end: u32,
    frac: u32,
    loop_start: u32,
    loop_bytes: u32,
    on: bool,
    /// Of the buffer being mixed: the bytes it gave and the times it went back to its loop.
    bytes: u32,
    wraps: u32,
}

impl Play {
    /// The byte played now; then `step` (16.16 bytes) further.
    fn next(&mut self, bus: &Bus, step: u32) -> i32 {
        if !self.on {
            return 0;
        }
        let s = bus.r8(self.cur) as i8 as i32;
        self.frac += step;
        self.cur += self.frac >> 16;
        self.frac &= 0xffff;
        self.bytes += 1;
        if self.cur >= self.end {
            if self.loop_bytes == 0 {
                self.on = false;
            } else {
                self.cur = self.loop_start + (self.cur - self.end) % self.loop_bytes;
                self.end = self.loop_start + self.loop_bytes;
                self.wraps += 1;
            }
        }
        s
    }
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 82 && &d[0..3] == b"FTM" && d[4] == 3
}

fn text(d: &[u8], o: usize, n: usize) -> String {
    let s = d.get(o..o + n).unwrap_or(&[]);
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    s[..end].iter().map(|&c| c as char).collect::<String>().trim_end().to_string()
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    Some(Box::new(build(d, base, m)?))
}

fn build(d: &[u8], base: u32, m: &mut Machine) -> Option<FaceTheMusic> {
    if !detect(d) {
        return None;
    }
    let num_samples = d[5] as usize;
    let measures = be16(d, 6);
    let start_cia = be16(d, 8);
    let mute_status = d[11];
    let global_volume = d[12];
    let flag = d[13];
    let start_speed = d[14];
    let rows_per_measure = d[15];
    if start_speed == 0 || rows_per_measure as u32 != 96 / start_speed as u32 || rows_per_measure == 0 {
        return None;
    }
    // the samples are in the module; external ones are not read
    if flag & 1 == 0 || num_samples > 64 {
        return None;
    }
    let title = text(d, 16, 32);
    let num_sels = d[80] as usize;

    let mut p = 82;
    let mut samples = vec![Samp::default(); 65];
    for s in samples.iter_mut().take(num_samples) {
        let e = d.get(p..p + 32)?;
        s.named = !text(e, 0, 29).is_empty();
        p += 32;
    }

    let mut scripts: Vec<Option<Vec<SelLine>>> = vec![None; 64];
    for _ in 0..num_sels {
        let n = be16(d, p) as usize;
        let idx = be16(d, p + 2) as usize;
        p += 4;
        let b = d.get(p..p + 4 * n)?;
        if idx >= 64 {
            return None;
        }
        scripts[idx] = Some(b.chunks(4).map(|c| SelLine { effect: c[0], a1: c[1], a2: u16::from_be_bytes([c[2], c[3]]) }).collect());
        p += 4 * n;
    }

    let mut tracks = Vec::new();
    for _ in 0..8 {
        let spacing = be16(d, p);
        let len = be32(d, p + 2) as usize;
        p += 6;
        let b = d.get(p..p + len / 2 * 2)?;
        let lines = b
            .chunks(2)
            .map(|c| {
                let effect = c[0] >> 4;
                if effect == SKIP_EMPTY_ROWS {
                    TLine { effect, arg: (((c[0] & 0x0f) as u16) << 8) | c[1] as u16, note: 0 }
                } else {
                    TLine { effect, arg: (((c[0] & 0x0f) as u16) << 2) | (c[1] >> 6) as u16, note: c[1] & 0x3f }
                }
            })
            .collect::<Vec<_>>();
        if lines.is_empty() {
            return None;
        }
        tracks.push(Track { spacing, lines });
        p += len / 2 * 2;
    }

    for s in samples.iter_mut().take(64) {
        if !s.named {
            continue;
        }
        if p + 4 > d.len() {
            return None;
        }
        s.oneshot = be16(d, p);
        s.loop_len = be16(d, p + 2);
        s.loop_start = s.oneshot as u32 * 2;
        s.total = (s.oneshot as u32 + s.loop_len as u32) * 2;
        p += 4;
        if p + s.total as usize > d.len() {
            return None;
        }
        s.addr = base + p as u32;
        p += s.total as usize;
    }

    // the quiet sample, and the mixer's buffers: four, four more, and one that stays empty
    let quiet = m.alloc(16);
    let mixbuf = m.alloc(9 * BUF + 24);
    if quiet == 0 || mixbuf == 0 {
        return None;
    }
    samples[QUIET] = Samp { named: false, oneshot: 2, loop_start: 0, loop_len: 0, total: 4, addr: quiet };
    let bufs = std::array::from_fn(|c| [mixbuf + c as u32 * BUF, mixbuf + (4 + c as u32) * BUF]);
    let idle = mixbuf + 8 * BUF + 24;

    let voices = (0..8)
        .map(|i| Voice {
            chan: i,
            cur_sample: QUIET,
            sample_number: -1,
            data: quiet,
            start_offset: 0,
            calc_offset: 0,
            oneshot: 2,
            loop_start: 0,
            loop_len: 0,
            total: 4,
            volume: 64,
            note_index: 0,
            retrig: false,
            detune_index: i / 2,
            track: i,
            track_pos: 0,
            rows_left: 0,
            port_ticks: 0,
            port_note: 0,
            port_end: 0,
            vd_volume: 0,
            vd_speed: 0,
            sel: Sel { pos: -1, voice: i, ..Default::default() },
            lfo: [Lfo::default(); 4],
        })
        .collect();

    let mut f = FaceTheMusic {
        title,
        rows_per_measure,
        mute_status,
        samples,
        scripts,
        tracks,
        quiet,
        start_row: 0,
        end_row: measures.wrapping_mul(rows_per_measure as u16),
        gvol: global_volume,
        speed: start_speed as u16,
        loop_stack: Vec::new(),
        loop_stop_row: 0,
        loop_start_row: 0,
        do_loop: false,
        speed_counter: 1,
        row: 0,
        detune: [0; 4],
        voices,
        cia: start_cia,
        end_reached: false,
        songend: false,
        visited: vec![false; measures as usize + 1],
        per: [PERIODS[0]; 8],
        vol: [0; 8],
        play: [Play::default(); 8],
        bufs,
        flip: [0; 4],
        ch_per: [214; 4],
        ch_vol: [64; 4],
    };
    for i in 0..8 {
        f.adjust_track_indexes(i);
        f.init_voice_with_latest(i);
    }
    f.visited[0] = true;
    // the init routine sets the LED bit of CIA-A's port: the filter is off
    m.bus.ciaa.pra |= 2;
    let mut hw = Hw { bus: &mut m.bus };
    f.init_hardware(&mut hw, idle);
    // the first tick is played before the timer runs
    f.step();
    Some(f)
}

/// Colour clocks from an audio interrupt to its handler's first write.
const T_ENTRY: u64 = 60;
/// From the handler's last write to its end.
const T_EXIT: u64 = 60;
/// A tick's time.
const T_TICK: u64 = 2000;

impl Replayer for FaceTheMusic {
    fn format(&self) -> &'static str {
        "Face The Music"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        (self.cia as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.step();
        hw.wait(T_TICK);
    }

    /// The handler of a channel's audio interrupt: the channel's buffer has started. It writes
    /// every channel's period and volume as the buffers mixed last have them, gives the channel
    /// its other buffer and mixes it.
    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if hw.bus.read_word(0xdff01e) & (0x80 << ch) == 0 {
            // taken already: the request came again while its handler waited
            return true;
        }
        hw.wait(T_ENTRY);
        hw.bus.write_word(0xdff09c, 0x80 << ch);
        for c in 0..4 {
            hw.wait([93, 38, 38, 38][c]);
            hw.per(c, self.ch_per[c]);
            hw.wait([115, 127, 139, 133][c]);
            hw.vol(c, self.ch_vol[c]);
        }
        hw.wait([51, 66, 82, 97][ch]);
        hw.bus.write_word(0xdff09a, 0x2000);
        hw.wait(154);
        let buf = self.bufs[ch][self.flip[ch]];
        self.flip[ch] ^= 1;
        hw.loc(ch, buf);
        let time = self.mix(ch, hw.bus, buf);
        hw.wait(time);
        hw.bus.write_word(0xdff09a, 0xa000);
        hw.wait(T_EXIT);
        true
    }
}

impl FaceTheMusic {
    /// The init and start routines' register writes, as the Eagleplayer's logs have them.
    fn init_hardware(&mut self, hw: &mut Hw, idle: u32) {
        hw.bus.write_word(0xdff09a, 0xa000);
        hw.wait(22);
        hw.bus.write_word(0xdff09c, 0x0780);
        hw.wait(278);
        hw.bus.write_word(0xdff09a, 0x8780);
        hw.wait(29);
        hw.dmacon(0x000f);
        hw.wait(8);
        hw.bus.write_word(0xdff09e, 0x00ff);
        for c in 0..4 {
            hw.wait([8, 6, 6, 6][c]);
            hw.bus.write_word(0xdff0aa + 16 * c as u32, 0);
        }
        for c in 0..4 {
            hw.wait(6);
            hw.vol(c, 0);
        }
        let channels = |hw: &mut Hw, first: u64, vol: u16| {
            for c in 0..4 {
                hw.wait(if c == 0 { first } else { 51 });
                hw.loc(c, idle);
                hw.wait(8);
                hw.len(c, (BUF / 2) as u16);
                hw.wait(8);
                hw.per(c, 428);
                hw.wait([235, 246, 258, 253][c]);
                hw.vol(c, vol);
            }
        };
        channels(hw, 2134, 1);
        // the start routine
        hw.wait(249474);
        for k in 0..8 {
            hw.wait([0, 156, 168, 162, 206, 156, 168, 162][k]);
            hw.vol(k & 3, 64);
        }
        channels(hw, 2348, 64);
        hw.wait(5193);
        hw.dmacon(0x800f);
    }

    /// One tick of the song.
    fn step(&mut self) {
        self.speed_counter = self.speed_counter.wrapping_sub(1);
        if self.speed_counter == 0 {
            self.speed_counter = self.speed;
            self.take_next_row();
        }
        for i in 0..8 {
            self.run_effects(i);
        }
        self.setup_voices();
        if self.end_reached {
            self.end_reached = false;
            self.songend = true;
            self.mark_visited();
        }
    }

    fn rpm(&self) -> u16 {
        self.rows_per_measure as u16
    }

    fn mark_visited(&mut self) {
        let p = (self.row / self.rpm()) as usize;
        if let Some(v) = self.visited.get_mut(p) {
            *v = true;
        }
    }

    fn has_visited(&self, p: usize) -> bool {
        self.visited.get(p).copied().unwrap_or(false)
    }

    fn line(&self, track: usize, pos: u32) -> Option<TLine> {
        self.tracks[track].lines.get(pos as usize).copied()
    }

    /// The previous volume and instrument of the voice's track, as the first row's.
    fn init_voice_with_latest(&mut self, vi: usize) {
        let track = self.voices[vi].track;
        let old = self.voices[vi].track_pos;
        let mut prev_vol = 0u8;
        let mut prev_instr = 0u16;
        let mut pos = old as i64 - 1;
        while pos >= 0 {
            let l = self.tracks[track].lines[pos as usize];
            if l.effect < SEL_EFFECT {
                prev_vol = l.effect;
                if prev_vol != 0 {
                    break;
                }
            }
            pos -= 1;
        }
        pos = old as i64 - 1;
        while pos >= 0 {
            let l = self.tracks[track].lines[pos as usize];
            if l.effect < SEL_EFFECT {
                prev_instr = l.arg;
                if prev_instr != 0 {
                    break;
                }
            }
            pos -= 1;
        }
        self.parse_track_effect(vi, TLine { effect: prev_vol, arg: prev_instr, note: 0 }, old);
    }

    fn take_next_row(&mut self) {
        if self.do_loop {
            self.do_loop = false;
            self.row = self.loop_start_row;
        } else {
            for i in 0..8 {
                if self.mute_status & (1 << i) != 0 {
                    self.handle_row(i);
                }
            }
            self.row = self.row.wrapping_add(1);
            let rpm = self.rpm();
            if self.row == self.loop_stop_row {
                self.row = self.loop_start_row;
            } else if self.row == self.end_row {
                self.row = self.start_row;
                self.end_reached = true;
            } else if self.row % rpm == 0 && self.loop_stack.is_empty() && self.has_visited((self.row / rpm) as usize) {
                self.end_reached = true;
            }
            self.mark_visited();
        }
        for i in 0..8 {
            self.adjust_track_indexes(i);
        }
    }

    fn adjust_track_indexes(&mut self, vi: usize) {
        let track = self.voices[vi].track;
        let spacing = self.tracks[track].spacing as i32;
        let mut pos = 0u32;
        let mut row = 0i32;
        let cur = self.row as i32;
        while row < cur {
            let Some(line) = self.line(track, pos) else { break };
            if line.effect == SKIP_EMPTY_ROWS {
                row += line.arg as i32;
                pos += 1;
            } else {
                loop {
                    row += 1;
                    pos += 1;
                    let Some(l) = self.line(track, pos) else { break };
                    if l.effect == SKIP_EMPTY_ROWS {
                        break;
                    }
                    row += spacing;
                    if row >= cur {
                        break;
                    }
                }
                if self.line(track, pos).is_none() {
                    break;
                }
            }
        }
        let v = &mut self.voices[vi];
        v.rows_left = (row - cur) as i16;
        v.track_pos = pos;
    }

    fn handle_row(&mut self, vi: usize) {
        self.voices[vi].rows_left = self.voices[vi].rows_left.wrapping_sub(1);
        if self.voices[vi].rows_left >= 0 {
            return;
        }
        let track = self.voices[vi].track;
        let mut pos = self.voices[vi].track_pos;
        let Some(line) = self.line(track, pos) else { return };
        if line.effect == SKIP_EMPTY_ROWS {
            if line.arg == 0 && line.note == 0 {
                pos += 1;
            } else {
                self.voices[vi].rows_left = line.arg as i16 - 1;
                self.voices[vi].track_pos += 1;
                return;
            }
        }
        if let Some(l) = self.line(track, pos) {
            self.parse_track_effect(vi, l, pos);
        }
        pos += 1;
        let spacing = self.tracks[track].spacing as i16;
        match self.line(track, pos) {
            Some(l) if l.effect == SKIP_EMPTY_ROWS => {
                self.voices[vi].rows_left = l.arg as i16;
                self.voices[vi].track_pos = pos + 1;
            }
            _ => {
                self.voices[vi].rows_left = spacing;
                self.voices[vi].track_pos = pos;
            }
        }
    }

    fn parse_track_effect(&mut self, vi: usize, line: TLine, pos: u32) {
        if line.effect == 0 && line.arg == 0 && line.note == 0 {
            return;
        }
        let mut new_sample = false;
        match line.effect {
            PATTERN_LOOP => self.parse_pattern_loop(line, pos),
            VOLUME_DOWN => self.parse_volume_down(vi, line),
            PORTAMENTO => {
                self.parse_portamento(vi, line);
                return;
            }
            SEL_EFFECT => self.parse_sel(vi, line),
            _ => {
                new_sample = line.arg != 0;
                self.parse_set_volume(vi, line);
            }
        }
        self.setup_note_and_sample(vi, line, new_sample);
    }

    fn parse_pattern_loop(&mut self, line: TLine, pos: u32) {
        let rpm = self.rpm();
        if line.arg == 0 {
            if !self.loop_stack.is_empty() && self.loop_stop_row == 0 {
                let top = self.loop_stack.len() - 1;
                if self.loop_stack[top].original == 63 {
                    // the special case of Jonahara.ftm and Slow-Singslow.ftm
                    self.end_reached = true;
                }
                self.loop_stack[top].count -= 1;
                if self.loop_stack[top].count < 0 {
                    self.loop_stack.pop();
                    self.loop_stop_row = 0;
                } else {
                    self.loop_stop_row = ((self.row / rpm) + 1).wrapping_mul(rpm);
                    self.loop_start_row = self.loop_stack[top].start;
                }
            }
        } else if self.loop_stop_row == 0 {
            // at most 16 pattern loops
            if self.loop_stack.len() < 16 {
                let start = (self.row / rpm).wrapping_mul(rpm);
                self.loop_stack.push(LoopInfo { track_pos: pos, start, count: line.arg as i16, original: line.arg as i16 });
            }
        } else if let Some(top) = self.loop_stack.last() {
            if pos == top.track_pos {
                self.loop_stop_row = 0;
            }
        }
    }

    fn parse_volume_down(&mut self, vi: usize, line: TLine) {
        let speed = self.speed as u32;
        let v = &mut self.voices[vi];
        if line.arg == 0 {
            v.volume = 0;
        } else {
            let ticks = line.arg as u32 * speed;
            v.vd_volume = v.volume as u32 * 256;
            v.vd_speed = if ticks == 0 { 0 } else { (v.vd_volume / ticks) as u16 };
            if v.sel.volume_down != 0 {
                v.sel.interrupt = v.sel.volume_down;
            }
        }
    }

    fn parse_portamento(&mut self, vi: usize, line: TLine) {
        if line.note == 0 {
            return;
        }
        let speed = self.speed as i32;
        let v = &mut self.voices[vi];
        v.port_end = (line.note as u16 - 1) * 16;
        v.port_note = v.note_index as u32 * 256;
        let mut ticks = (v.port_end as u32 * 256).wrapping_sub(v.port_note) as i32;
        if ticks != 0 {
            if line.arg == 0 {
                v.note_index = v.port_end;
                v.port_ticks = 0;
            } else {
                let div = line.arg as i32 * speed;
                if div != 0 {
                    ticks /= div;
                }
                if v.sel.portamento != 0 {
                    v.sel.interrupt = v.sel.portamento;
                }
            }
        }
        v.port_ticks = ticks as i16;
    }

    fn parse_sel(&mut self, vi: usize, line: TLine) {
        let script = self.scripts.get(line.arg as usize).map(|s| s.is_some()).unwrap_or(false);
        let v = &mut self.voices[vi];
        v.sel = Sel {
            script: if script { Some(line.arg as usize) } else { None },
            pos: if script { 0 } else { -1 },
            voice: vi,
            ..Default::default()
        };
    }

    fn parse_set_volume(&mut self, vi: usize, line: TLine) {
        let v = &mut self.voices[vi];
        if line.arg != 0 {
            v.sample_number = line.arg as i16 - 1;
            v.cur_sample = line.arg as usize - 1;
            if v.sel.new_sample != 0 {
                v.sel.interrupt = v.sel.new_sample;
            }
        }
        if line.effect != 0 {
            v.volume = EFFECT_VOLUME.get(line.effect as usize - 1).copied().unwrap_or(0) as u16;
            v.vd_speed = 0;
            if v.sel.new_volume != 0 {
                v.sel.interrupt = v.sel.new_volume;
            }
        }
    }

    fn load_sample(&mut self, vi: usize, s: usize) {
        let smp = self.samples[s].clone();
        let v = &mut self.voices[vi];
        v.data = smp.addr;
        v.start_offset = 0;
        v.calc_offset = 0;
        v.oneshot = smp.oneshot;
        v.loop_start = smp.loop_start;
        v.loop_len = smp.loop_len;
        v.total = smp.total;
    }

    fn setup_note_and_sample(&mut self, vi: usize, line: TLine, new_sample: bool) {
        let note = line.note;
        if note == 35 {
            let v = &mut self.voices[vi];
            if v.sel.release != 0 {
                v.sel.interrupt = v.sel.release;
            }
        } else if note > 0 && note < 35 {
            let v = &mut self.voices[vi];
            v.note_index = (note as u16 - 1) * 16;
            if v.sel.new_pitch != 0 {
                v.sel.interrupt = v.sel.new_pitch;
            }
            v.port_ticks = 0;
            let cs = v.cur_sample;
            let loop_len = self.samples[cs].loop_len;
            if new_sample || self.voices[vi].data == self.quiet || loop_len == 0 {
                self.voices[vi].retrig = true;
                self.load_sample(vi, cs);
            }
        }
    }

    fn run_effects(&mut self, vi: usize) {
        self.run_volume_down(vi);
        self.run_portamento(vi);
        self.run_sel(vi);
        self.run_lfo(vi);
    }

    fn run_volume_down(&mut self, vi: usize) {
        let v = &mut self.voices[vi];
        if v.vd_speed != 0 {
            let mut nv = v.vd_volume as i64 - v.vd_speed as i64;
            if nv < 0 {
                v.vd_speed = 0;
                nv = 0;
            }
            v.vd_volume = nv as u32;
            v.volume = (nv / 256) as u16;
        }
    }

    fn run_portamento(&mut self, vi: usize) {
        let v = &mut self.voices[vi];
        if v.port_ticks != 0 {
            v.port_note = (v.port_note as i64 + v.port_ticks as i64) as u32;
            v.note_index = ((v.port_note / 256) & 0xfffe) as u16;
            if v.port_ticks < 0 {
                if v.note_index <= v.port_end {
                    v.note_index = v.port_end;
                    v.port_ticks = 0;
                }
            } else if v.note_index >= v.port_end {
                v.note_index = v.port_end;
                v.port_ticks = 0;
            }
        }
    }

    fn script_len(&self, vi: usize) -> usize {
        self.voices[vi].sel.script.and_then(|s| self.scripts[s].as_ref()).map_or(0, |s| s.len())
    }

    fn run_sel(&mut self, vi: usize) {
        if self.voices[vi].sel.pos < 0 {
            return;
        }
        if self.voices[vi].sel.interrupt != 0 {
            let goto = self.voices[vi].sel.interrupt;
            let s = &mut self.voices[vi].sel;
            s.interrupt = 0;
            s.wait = 0;
            s.loop_counter = 0;
            if !self.sel_goto_line(vi, goto) {
                return;
            }
        } else if self.voices[vi].sel.wait != 0 {
            let s = &mut self.voices[vi].sel;
            if s.wait != 0xffff {
                s.wait -= 1;
            }
            return;
        }
        loop {
            let (script, pos) = (self.voices[vi].sel.script, self.voices[vi].sel.pos);
            let Some(line) = script.and_then(|s| self.scripts[s].as_ref()).and_then(|s| s.get(pos.max(0) as usize)).copied() else {
                self.voices[vi].sel.pos = -1;
                break;
            };
            if line.effect != 0 && !self.run_specific_sel(vi, line) {
                break;
            }
            self.voices[vi].sel.pos += 1;
            if self.voices[vi].sel.pos as usize >= self.script_len(vi) {
                self.voices[vi].sel.pos = -1;
                break;
            }
        }
    }

    fn sel_goto_line(&mut self, vi: usize, line_number: u16) -> bool {
        if (line_number as usize) < self.script_len(vi) {
            self.voices[vi].sel.pos = line_number as i32 - 1;
            true
        } else {
            self.voices[vi].sel.pos = -1;
            false
        }
    }

    /// A line's 12-bit argument pair: the value of arguments 1 and the high nibble of 2.
    fn arg12(l: SelLine) -> i32 {
        ((l.a1 as i32) << 4) | ((l.a2 as i32 & 0xf000) >> 12)
    }

    fn run_specific_sel(&mut self, vi: usize, l: SelLine) -> bool {
        let sv = self.voices[vi].sel.voice;
        match l.effect {
            1 => {
                // wait
                let wait = l.a2.wrapping_sub(1);
                self.voices[vi].sel.wait = wait;
                if wait != 0xffff {
                    self.voices[vi].sel.pos += 1;
                    if self.voices[vi].sel.pos as usize == self.script_len(vi) {
                        self.voices[vi].sel.pos = -1;
                    }
                }
                return false;
            }
            2 => return self.sel_goto_line(vi, l.a2 & 0x0fff),
            3 => {
                // loop
                let s = &mut self.voices[vi].sel;
                if s.loop_counter == 0 {
                    s.loop_counter = ((l.a1 as u16) << 4) | ((l.a2 & 0xf000) >> 12);
                } else {
                    s.loop_counter -= 1;
                }
                if s.loop_counter == 0 {
                    return true;
                }
                return self.sel_goto_line(vi, l.a2 & 0x0fff);
            }
            4 => {
                // goto script
                let n = Self::arg12(l) as usize;
                if self.scripts.get(n).map_or(true, |s| s.is_none()) {
                    self.voices[vi].sel.pos = -1;
                    return false;
                }
                self.voices[vi].sel.script = Some(n);
                return self.sel_goto_line(vi, l.a2 & 0x0fff);
            }
            5 => {
                self.voices[vi].sel.pos = -1;
                return false;
            }
            6..=11 => {
                let (pitch, vol) = (self.voices[sv].note_index as i32, self.voices[sv].volume as i32);
                let x = Self::arg12(l);
                let cond = match l.effect {
                    6 => pitch == x,
                    7 => pitch < x,
                    8 => pitch > x,
                    9 => vol == x,
                    10 => vol < x,
                    _ => vol > x,
                };
                return if cond { self.sel_goto_line(vi, l.a2 & 0x0fff) } else { true };
            }
            12..=17 => {
                let g = (l.a2 & 0x0fff) + 1;
                let s = &mut self.voices[vi].sel;
                match l.effect {
                    12 => s.new_pitch = g,
                    13 => s.new_volume = g,
                    14 => s.new_sample = g,
                    15 => s.release = g,
                    16 => s.portamento = g,
                    _ => s.volume_down = g,
                }
            }
            18 => {
                let cs = self.voices[sv].cur_sample;
                self.sel_play_sample(sv, cs);
            }
            19 => self.sel_play_sample(sv, QUIET),
            20 => {
                let off = (((l.a1 & 0x0f) as u32) << 16) + l.a2 as u32;
                let v = &mut self.voices[sv];
                v.start_offset = v.calc_offset.wrapping_add(off.wrapping_mul(2));
                v.retrig = true;
            }
            21 => {
                let off = (((l.a1 & 0x0f) as u32) << 16) + l.a2 as u32;
                let v = &mut self.voices[sv];
                v.start_offset = v.start_offset.wrapping_add(off);
                v.retrig = true;
            }
            22 => {
                let off = (((l.a1 & 0x0f) as u32) << 16) + l.a2 as u32;
                let v = &mut self.voices[sv];
                v.start_offset = v.start_offset.wrapping_sub(off);
                v.retrig = true;
            }
            23 => self.voices[sv].note_index = ((l.a2 & 0x0fff) * 2).min(542),
            24 => self.detune[self.voices[sv].detune_index] = (l.a2 & 0x0fff) as i16,
            25 => {
                let di = self.voices[sv].detune_index;
                self.detune[di] = self.detune[di].wrapping_add(Self::arg12(l) as i16);
                let ni = (((l.a2 & 0x0fff) * 2) as u32 + self.voices[sv].note_index as u32) as u16;
                self.voices[sv].note_index = ni.min(542);
            }
            26 => {
                let di = self.voices[sv].detune_index;
                self.detune[di] = self.detune[di].wrapping_sub(Self::arg12(l) as i16);
                let ni = self.voices[sv].note_index as i32 - (l.a2 & 0x0fff) as i32 * 2;
                self.voices[sv].note_index = ni.max(0) as u16;
            }
            27 => self.voices[sv].volume = (l.a2 & 0xff).min(64),
            28 => self.voices[sv].volume = (self.voices[sv].volume + (l.a2 & 0xff)).min(64),
            29 => self.voices[sv].volume = (self.voices[sv].volume as i32 - (l.a2 & 0xff) as i32).max(0) as u16,
            30 => {
                let n = (l.a2 & 0xff) as usize;
                self.voices[sv].sample_number = n as i16;
                self.voices[sv].cur_sample = if n < 64 { n } else { QUIET };
            }
            31..=33 => {
                let off = (((l.a1 & 0x0f) as i64) << 16) + l.a2 as i64;
                let v = &mut self.voices[sv];
                let n = match l.effect {
                    31 => off,
                    32 => off + v.calc_offset as i64,
                    _ => (v.calc_offset as i64 - off).max(0),
                };
                v.calc_offset = if l.effect == 33 { n as u32 } else { n.min(v.total as i64) as u32 };
            }
            34..=36 => {
                let v = &self.voices[sv];
                let n = match l.effect {
                    34 => l.a2,
                    35 => (v.oneshot as u32 + l.a2 as u32).min(0xffff) as u16,
                    _ => (v.oneshot as i32 - l.a2 as i32).max(0) as u16,
                };
                let v = &mut self.voices[sv];
                v.oneshot = n;
                v.loop_start = v.calc_offset.wrapping_add(n as u32 * 2);
                v.total = (v.loop_len as u32 * 2).wrapping_add(v.loop_start);
            }
            37..=39 => {
                let v = &self.voices[sv];
                let n = match l.effect {
                    37 => l.a2,
                    38 => (v.loop_len as u32 + l.a2 as u32).min(0xffff) as u16,
                    _ => (v.loop_len as i32 - l.a2 as i32).max(0) as u16,
                };
                let v = &mut self.voices[sv];
                v.loop_len = n;
                v.total = v.calc_offset.wrapping_add((v.oneshot as u32 + n as u32) * 2);
            }
            40..=42 => {
                let t = (l.a2 & 7) as usize;
                match l.effect {
                    40 => self.voices[sv].note_index = self.voices[t].note_index,
                    41 => self.voices[sv].volume = self.voices[t].volume,
                    _ => self.voices[sv].cur_sample = self.voices[t].cur_sample,
                }
            }
            43 => {
                // clone track
                let t = (l.a2 & 7) as usize;
                let mut c = self.voices[t].clone();
                c.chan = self.voices[sv].chan;
                self.voices[sv] = c;
            }
            44..=55 => {
                let n = ((l.effect - 44) / 3) as usize;
                let lfo = &mut self.voices[vi].lfo[n];
                match (l.effect - 44) % 3 {
                    0 => {
                        let target = (l.a1 & 0xf0) >> 4;
                        let shape = (l.a1 & 0x0f) as usize;
                        let mut speed = (l.a2 & 0xff00) >> 8;
                        let depth = l.a2 & 0x00ff;
                        lfo.target = target;
                        lfo.loop_mod = shape & 8 == 0;
                        lfo.shape = shape & 7;
                        lfo.shape_pos = 0;
                        if speed > 192 {
                            speed = 191;
                        }
                        lfo.speed = speed;
                        lfo.depth = depth & 0x7f;
                        lfo.value = 0;
                    }
                    1 => {
                        let mut speed = lfo.speed.wrapping_add((l.a2 & 0xff00) >> 8);
                        if speed > 192 {
                            speed = 191;
                        }
                        lfo.speed = speed;
                        lfo.depth = (lfo.depth + (l.a2 & 0xff)).min(127);
                    }
                    _ => {
                        lfo.speed = (lfo.speed as i32 - ((l.a2 & 0xff00) >> 8) as i32).max(0) as u16;
                        lfo.depth = (lfo.depth as i32 - (l.a2 & 0xff) as i32).max(0) as u16;
                    }
                }
            }
            56 => {
                let t = (l.a2 & 7) as usize;
                self.voices[vi].sel.voice = t;
            }
            57 => {
                let mut t = (l.a2 & 7) as usize + self.voices[sv].chan;
                if t >= 8 {
                    t -= 8;
                }
                self.voices[vi].sel.voice = t;
            }
            58 => self.gvol = ((l.a2 & 0xff).min(64)) as u8,
            59 => self.cia = l.a2.max(0x1000),
            60 => self.speed = l.a2 & 0xff,
            61 => {
                self.loop_start_row = l.a2;
                self.do_loop = true;
            }
            _ => {}
        }
        true
    }

    fn sel_play_sample(&mut self, vi: usize, s: usize) {
        self.load_sample(vi, s);
        self.voices[vi].retrig = true;
    }

    fn run_lfo(&mut self, vi: usize) {
        for i in 0..4 {
            let lfo = self.voices[vi].lfo[i];
            if lfo.speed == 0 || lfo.target == 0 {
                continue;
            }
            let sv = self.voices[vi].sel.voice;
            let shape = LFO_SHAPES.get(lfo.shape).map_or(0, |s| s[lfo.shape_pos.clamp(0, 191) as usize] as i32);
            let value = (shape * lfo.depth as i32) / 128;
            let add = value - lfo.value as i32;
            let t = &mut self.voices[sv];
            match lfo.target {
                1..=4 => {
                    let l = &mut t.lfo[lfo.target as usize - 1];
                    l.speed = (l.speed as i32 + add).clamp(0, 95) as u16;
                }
                5..=8 => {
                    let l = &mut t.lfo[lfo.target as usize - 5];
                    l.depth = (l.depth as i32 + add).clamp(0, 127) as u16;
                }
                10 => t.volume = (t.volume as i32 + add).clamp(0, 64) as u16,
                15 => t.note_index = (t.note_index as i32 + add * 2).clamp(0, 542) as u16,
                _ => {}
            }
            let l = &mut self.voices[vi].lfo[i];
            l.value = value as i16;
            let mut pos = lfo.shape_pos + l.speed as i32;
            if pos >= 192 {
                pos -= 192;
                if !l.loop_mod {
                    l.speed = 0;
                }
            }
            l.shape_pos = pos;
        }
    }

    /// The tick's result for the mixer: the samples started, each track's period and volume.
    fn setup_voices(&mut self) {
        for t in 0..8 {
            if self.mute_status & (1 << t) == 0 {
                continue;
            }
            let v = &mut self.voices[t];
            if v.retrig {
                v.retrig = false;
                let (data, start, total) = (v.data, v.start_offset, v.total);
                let on = start < total && (total - start) / 2 != 0;
                self.play[t] = Play {
                    cur: data.wrapping_add(start),
                    end: data.wrapping_add(total),
                    frac: 0,
                    loop_start: data.wrapping_add(v.loop_start),
                    loop_bytes: v.loop_len as u32 * 2,
                    on,
                    bytes: 0,
                    wraps: 0,
                };
            }
            let v = &self.voices[t];
            let mut period = PERIODS.get(v.note_index as usize / 2).copied().unwrap_or(0) as f64;
            if period != 0.0 {
                let detune = self.detune[v.detune_index] as i32 * -8;
                if detune != 0 {
                    // the frequency is scaled by 2 ^ (detune / (12 * 256 * 128))
                    period /= 2f64.powf(detune as f64 / (12.0 * 256.0 * 128.0));
                }
                self.per[t] = period.round().clamp(1.0, 65535.0) as u16;
            }
            self.vol[t] = (v.volume as u32 * self.gvol as u32 / 64).min(64) as u16;
        }
    }

    /// Mixes the two tracks of channel `ch` into the buffer at `dst`: the channel plays at the
    /// shorter of their periods and the higher of their volumes, the other track is stepped
    /// and scaled to them. Returns the time it takes.
    fn mix(&mut self, ch: usize, bus: &mut Bus, dst: u32) -> u64 {
        let t = [2 * ch, 2 * ch + 1];
        let per = self.per[t[0]].min(self.per[t[1]]);
        let vol = self.vol[t[0]].max(self.vol[t[1]]);
        let mut gain = [64i32; 2];
        let mut step = [0x10000u32; 2];
        for k in 0..2 {
            let (p, v) = (self.per[t[k]] as u32, self.vol[t[k]] as u32);
            if v != vol as u32 {
                gain[k] = ((v * 64 + vol as u32 / 2) / vol as u32) as i32;
            }
            if p != per as u32 {
                step[k] = ((per as u32) << 16) / p;
            }
        }
        for k in 0..2 {
            self.play[t[k]].bytes = 0;
            self.play[t[k]].wraps = 0;
        }
        for i in 0..BUF {
            let mut s = 0;
            for k in 0..2 {
                s += (self.play[t[k]].next(bus, step[k]) * gain[k]) >> 6;
            }
            bus.w8(dst + i, s.clamp(-128, 127) as u8);
        }
        self.ch_per[ch] = per;
        self.ch_vol[ch] = vol;
        3347
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A module of one measure of 16 rows, speed 6, CIA timer 0x3781, tracks 0 and 1 in use,
    /// and one sample of 4 words, played once. Track 0 plays note 25 with the sample at
    /// volume 9 (64) in row 0 and note 27 in row 8; track 1 is empty.
    fn module() -> Vec<u8> {
        let mut d = b"FTMN\x03\x01".to_vec();
        d.extend([0, 1, 0x37, 0x81, 0, 0x03, 63, 1, 6, 16]);
        d.extend(b"test song\0".iter().chain([0u8; 22].iter()));
        d.extend(b"nobody\0".iter().chain([0u8; 25].iter()));
        d.extend([0, 0]);
        let mut name = b"smp".to_vec();
        name.resize(32, 0);
        d.extend(name);
        // the tracks: note lines have an effect (10: volume 9) and an argument (the sample), then
        // the rows to skip
        let track0 = [0xa0u8, 0x40 | 25, 0xf0, 7, 0xa0, 27, 0xf0, 7];
        for t in 0..8 {
            d.extend(0u16.to_be_bytes());
            let lines: &[u8] = if t == 0 { &track0 } else { &[0xf0, 16] };
            d.extend((lines.len() as u32).to_be_bytes());
            d.extend(lines);
        }
        d.extend([0, 4, 0, 0]);
        d.extend([0x20; 8]);
        d
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.ftm"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, log.iter().map(|w| (w.0, w.1)).collect())
    }

    #[test]
    fn detects_only_ftm() {
        let d = module();
        assert!(detect(&d));
        let mut bad = d.clone();
        bad[4] = 2;
        assert!(!detect(&bad));
        assert!(!detect(&d[..60]));
        assert!(!detect(b"FTX\0\x03"));
    }

    #[test]
    fn plays_two_notes_on_paula() {
        let (format, log) = play(module(), 60);
        assert_eq!(format, "Face The Music");
        // tracks 0 and 1 are channel 0: volume 64 of the global 63, the note's period (track 1
        // stays at the lowest note's, which is longer)
        let reg = |r: u32| log.iter().filter(|w| w.0 == r).map(|w| w.1).collect::<Vec<_>>();
        assert_eq!(reg(0xdff0a8).last().copied(), Some(63));
        let per = reg(0xdff0a6);
        assert!(per.contains(&PERIODS[24 * 8]), "{per:x?}");
        assert!(per.contains(&PERIODS[26 * 8]), "{per:x?}");
        // the other channels are silent
        assert_eq!(reg(0xdff0b8).last().copied(), Some(0));
        // every buffer is 100 words
        assert!(reg(0xdff0a4).iter().all(|&l| l == 100));
    }

    #[test]
    fn mixes_two_tracks_to_the_shorter_period_and_higher_volume() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        let m = &mut e.m;
        let base = m.alloc(64);
        for i in 0..16 {
            m.bus.w8(base + i, (i as u8) * 8);
        }
        let d = module();
        let at = m.alloc(d.len() as u32);
        m.bus.write_bytes(at, &d);
        let mut f = build(&d, at, m).unwrap();
        let on = Play { cur: base, end: base + 16, frac: 0, loop_start: base, loop_bytes: 16, on: true, bytes: 0, wraps: 0 };
        f.play[0] = on;
        f.play[1] = on;
        f.per[0] = 200;
        f.per[1] = 400;
        f.vol[0] = 32;
        f.vol[1] = 64;
        let dst = m.alloc(BUF);
        f.mix(0, &mut m.bus, dst);
        assert_eq!((f.ch_per[0], f.ch_vol[0]), (200, 64));
        // track 0 at half the volume, a byte each; track 1 in full, a byte every two
        let got: Vec<i8> = (0..6).map(|i| m.bus.r8(dst + i) as i8).collect();
        assert_eq!(got, [0, 4, 8 + 8, 12 + 8, 16 + 16, 20 + 16]);
    }
}
