//! Plays what `load.rs` converts an X-Tracker module to: rows of OpenMPT-style commands, with the
//! meaning OpenMPT's player gives them in a DMF module (libopenmpt 0.8.7 `soundlib/Snd_fx.cpp`,
//! `Sndmix.cpp`, `Fastmix.cpp`, `ModChannel.cpp`, BSD-3-Clause; see `mod.rs`). OpenMPT gives a
//! DMF module the playback behaviours of its best save format, IT (`InitializeGlobals`: all of
//! IT's but the initial note memory), with the DMF module type where the code tests the type,
//! linear slides, "compatible Gxx", sample offsets without a note and no instruments: the paths
//! taken here are those (IT vibrato, tremolo, panbrello, tremor, arpeggio, retrigger and offset
//! handling, per-channel first ticks with note delays, slide memories set on the first tick,
//! the volume column's own memories), and only for the commands the loader writes.
//!
//! The mixer is this file's own: one voice per channel, linear interpolation, OpenMPT's
//! "compatible" mix levels (pre-amp 48, no soft panning), its volume ramps (363 us up, 952 us
//! down, a whole tick between two non-zero volumes) and a fading copy of the old voice when a
//! new note cuts it (OpenMPT's NNA channel). OpenMPT's end-of-sample click removal is not done.

use super::load::{Cell, Cmd, Data, Module, Sample, VolCmd, NOTE_MAX, NOTE_MIN_SPECIAL, NOTE_NONE, NOTE_NOTECUT, ORDER_END};
use crate::mptmath::{freq_slide, Tables};

/// `ITSinusTable` (libopenmpt `Tables.cpp`)
const IT_SINE: [i8; 256] = [
    0, 2, 3, 5, 6, 8, 9, 11, 12, 14, 16, 17, 19, 20, 22, 23, 24, 26, 27, 29, 30, 32, 33, 34, 36, 37, 38, 39, 41, 42, 43, 44, 45, 46, 47,
    48, 49, 50, 51, 52, 53, 54, 55, 56, 56, 57, 58, 59, 59, 60, 60, 61, 61, 62, 62, 62, 63, 63, 63, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 63, 63, 63, 62, 62, 62, 61, 61, 60, 60, 59, 59, 58, 57, 56, 56, 55, 54, 53, 52, 51, 50, 49, 48, 47, 46, 45, 44, 43, 42, 41,
    39, 38, 37, 36, 34, 33, 32, 30, 29, 27, 26, 24, 23, 22, 20, 19, 17, 16, 14, 12, 11, 9, 8, 6, 5, 3, 2, 0, -2, -3, -5, -6, -8, -9, -11,
    -12, -14, -16, -17, -19, -20, -22, -23, -24, -26, -27, -29, -30, -32, -33, -34, -36, -37, -38, -39, -41, -42, -43, -44, -45, -46,
    -47, -48, -49, -50, -51, -52, -53, -54, -55, -56, -56, -57, -58, -59, -59, -60, -60, -61, -61, -62, -62, -62, -63, -63, -63, -64,
    -64, -64, -64, -64, -64, -64, -64, -64, -64, -64, -63, -63, -63, -62, -62, -62, -61, -61, -60, -60, -59, -59, -58, -57, -56, -56,
    -55, -54, -53, -52, -51, -50, -49, -48, -47, -46, -45, -44, -43, -42, -41, -39, -38, -37, -36, -34, -33, -32, -30, -29, -27, -26,
    -24, -23, -22, -20, -19, -17, -16, -14, -12, -11, -9, -8, -6, -5, -3, -2,
];

/// `ImpulseTrackerPortaVolCmd`
const IT_PORTA_VOLCMD: [u16; 16] = [0x00, 0x01, 0x04, 0x08, 0x10, 0x20, 0x40, 0x60, 0x80, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// `retrigTable1` / `retrigTable2`: the volume change of a retrigger (`x` of `Qxy`)
const RETRIG_MUL: [i32; 16] = [0, 0, 0, 0, 0, 0, 10, 8, 0, 0, 0, 0, 0, 0, 24, 32];
const RETRIG_ADD: [i32; 16] = [0, -1, -2, -4, -8, -16, 0, 0, 0, 1, 2, 4, 8, 16, 0, 0];

/// `GetVibratoDelta` with the IT tables (the random waveform, which the loader never
/// selects, plays as the sine).
fn vib_delta(kind: u8, pos: u8) -> i32 {
    match kind & 3 {
        1 => 64 - (pos as i32 + 1) / 2,
        2 => {
            if pos < 128 {
                64
            } else {
                0
            }
        }
        _ => IT_SINE[pos as usize] as i32,
    }
}

/// The playing part of a channel: what the mixer reads.
#[derive(Clone, Default)]
struct Voice {
    sample: usize,
    pos: f64,
    inc: f64,
    /// `nLength`: the end of the playing part (the loop end while looping); 0 = stopped
    length: u32,
    loop_on: bool,
    loop_start: u32,
    loop_end: u32,
    /// `CHN_PINGPONGFLAG`: playing backwards
    back: bool,
    /// the gains now, their ramp steps, their targets and the frames of ramp left
    gl: f32,
    gr: f32,
    sl: f32,
    sr: f32,
    tl: f32,
    tr: f32,
    ramp: u32,
}

#[derive(Clone, Default)]
struct Channel {
    row: Cell,
    v: Voice,
    /// a cut note fading out (OpenMPT's NNA channel)
    tail: Option<Voice>,
    /// `pModSample`: None before the first instrument
    smp: Option<usize>,
    /// `CHN_SUSTAINLOOP` of the channel
    sustain: bool,
    note: u8,
    new_note: u8,
    /// `nNewIns`: an instrument number waiting for its note
    new_ins: u8,
    /// `isFirstTick`: the tick the row's note starts on (note delays included)
    first: bool,
    /// `IsSamplePlaying`: the last tick mixed this channel
    sample_playing: bool,
    c5: u32,
    period: i64,
    porta_dest: i64,
    porta_slide: u16,
    old_porta_up: u8,
    old_porta_down: u8,
    volume: i32,
    global_vol: i32,
    ins_vol: i32,
    pan: i32,
    real_pan: i32,
    old_vol_slide: u8,
    old_fine_vol: u8,
    old_pan_slide: u8,
    old_vol_param: u8,
    old_offset: u32,
    old_hi_offset: u8,
    micro: i32,
    vib_speed: u8,
    vib_depth: u8,
    vib_pos: u8,
    vib_type: u8,
    vib_on: bool,
    trem_speed: u8,
    trem_depth: u8,
    trem_pos: u8,
    trem_type: u8,
    trem_on: bool,
    pbr_speed: u8,
    pbr_depth: u8,
    pbr_pos: u8,
    pbr_type: u8,
    /// `nPanbrelloOffset`: the panbrello holds until a note or a panning command
    pbr_offset: i32,
    arpeggio: u8,
    /// `nCommand`: the arpeggio or tremor running on this row
    command: Cmd,
    retrig_param: u8,
    retrig_count: u8,
    tremor_param: u8,
    tremor_count: u8,
    paused: bool,
    keyoff: bool,
    notefade: bool,
    fade_vol: i32,
    fast_ramp: bool,
}

pub struct Player {
    m: Module,
    t: Tables,
    rate: u32,
    nearest: bool,
    ramp_up: u32,
    ramp_down: u32,
    chn: Vec<Channel>,
    ord: usize,
    row: usize,
    /// `m_nTickCount`: the tick in the row, row delays included
    tick: u32,
    speed: u32,
    tempo: u32,
    pat_delay: u32,
    frame_delay: u32,
    first: bool,
    jump: Option<usize>,
    /// order entries whose first row has played
    visited: Vec<bool>,
    spt: usize,
    started: bool,
    ended: bool,
    /// interleaved stereo of the tick being played out
    buf: Vec<f32>,
    at: usize,
}

/// Samples per tick at `tempo` (`GetTickDuration`, classic tempo mode).
fn tick_len(rate: u32, tempo: u32) -> usize {
    ((rate as u64 * 5) / (tempo.max(1) as u64 * 2)).max(1) as usize
}

/// What a row does to the song's timing on its first tick: (speed, tempo, row delay, frame
/// delay, position jump).
fn row_timing(cells: &[Cell], speed: u32, tempo: u32) -> (u32, u32, u32, u32, Option<usize>) {
    let (mut speed, mut tempo, mut pd, mut fd, mut jump) = (speed, tempo, 0, 0, None);
    for c in cells {
        match c.cmd {
            Cmd::Speed if c.param > 0 => speed = c.param as u32,
            Cmd::Tempo if c.param >= 32 => tempo = c.param as u32,
            Cmd::S3mCmdEx if c.param & 0xF0 == 0xE0 => pd = 1 + (c.param & 0x0F) as u32,
            Cmd::S3mCmdEx if c.param & 0xF0 == 0x60 => fd += (c.param & 0x0F) as u32,
            Cmd::PositionJump => jump = Some(c.param as usize),
            _ => {}
        }
    }
    (speed, tempo, pd, fd, jump)
}

impl Player {
    pub fn new(m: Module, rate: u32) -> Player {
        let rate = rate.clamp(1000, 384_000);
        let mut p = Player {
            t: Tables::new(),
            rate,
            nearest: false,
            ramp_up: ((363 * rate as u64 + 500_000) / 1_000_000).max(1) as u32,
            ramp_down: ((952 * rate as u64 + 500_000) / 1_000_000).max(1) as u32,
            chn: Vec::new(),
            ord: 0,
            row: 0,
            tick: 0,
            speed: 6,
            tempo: 120,
            pat_delay: 0,
            frame_delay: 0,
            first: true,
            jump: None,
            visited: Vec::new(),
            spt: tick_len(rate, 120),
            started: false,
            ended: false,
            buf: Vec::new(),
            at: 0,
            m,
        };
        p.restart();
        p
    }

    pub fn module(&self) -> &Module {
        &self.m
    }

    pub fn set_nearest(&mut self, nearest: bool) {
        self.nearest = nearest;
    }

    /// Back to the start of the song.
    pub fn restart(&mut self) {
        let n = self.m.channels;
        self.chn = (0..n)
            .map(|_| Channel {
                global_vol: 64,
                ins_vol: 64,
                pan: 128,
                real_pan: 128,
                keyoff: true,
                notefade: true,
                c5: 8363,
                retrig_param: 1,
                ..Channel::default()
            })
            .collect();
        self.ord = 0;
        self.row = 0;
        self.tick = 0;
        self.speed = 6;
        self.tempo = 120;
        self.pat_delay = 0;
        self.frame_delay = 0;
        self.jump = None;
        self.visited = vec![false; self.m.order.len()];
        self.started = false;
        self.ended = !self.seek_order();
        if !self.ended {
            self.visited[self.ord] = true;
        }
        self.buf.clear();
        self.at = 0;
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    /// (order, pattern, row)
    pub fn position(&self) -> (i32, i32, i32) {
        let pat = self.m.order.get(self.ord).map_or(-1, |&p| p as i32);
        (self.ord as i32, pat, self.row as i32)
    }

    /// Moves `ord` on to the next order entry that is a pattern, skipping the others; false at
    /// the end of the list or at an end-of-song entry.
    fn seek_order(&mut self) -> bool {
        while self.ord < self.m.order.len() {
            let p = self.m.order[self.ord];
            if p == ORDER_END {
                return false;
            }
            if (p as usize) < self.m.patterns.len() {
                return true;
            }
            self.ord += 1;
        }
        false
    }

    /// The song's length in milliseconds: the rows as they play, with their speeds, tempos,
    /// row and frame delays and the position jump, to the first order entry played again.
    pub fn duration_ms(&self) -> i32 {
        let ch = self.m.channels;
        let order = &self.m.order;
        let mut visited = vec![false; order.len()];
        let (mut speed, mut tempo) = (6u32, 120u32);
        let mut frames = 0u64;
        let mut ord = 0usize;
        let seek = |mut o: usize| -> Option<usize> {
            while o < order.len() {
                let p = order[o];
                if p == ORDER_END {
                    return None;
                }
                if (p as usize) < self.m.patterns.len() {
                    return Some(o);
                }
                o += 1;
            }
            None
        };
        'song: while let Some(o) = seek(ord) {
            if visited[o] {
                break;
            }
            visited[o] = true;
            let pat = &self.m.patterns[order[o] as usize];
            let mut next = o + 1;
            for row in 0..pat.rows {
                let (s, t, pd, fd, jump) = row_timing(&pat.cells[row * ch..(row + 1) * ch], speed, tempo);
                speed = s;
                tempo = t;
                let ticks = (speed + fd) as u64 * pd.max(1) as u64;
                frames += ticks * tick_len(48000, tempo) as u64;
                if let Some(j) = jump {
                    if j >= order.len() {
                        break 'song;
                    }
                    next = j;
                    break;
                }
            }
            ord = next;
        }
        (frames * 1000 / 48000).min(i32::MAX as u64) as i32
    }

    /// Fills `out` (interleaved stereo, -1..1) and returns the frames written: fewer at the end.
    pub fn render(&mut self, out: &mut [f32]) -> usize {
        let want = out.len() / 2;
        let mut done = 0;
        while done < want {
            if self.at >= self.buf.len() {
                if self.ended || !self.next_tick() {
                    self.ended = true;
                    break;
                }
            }
            let n = ((self.buf.len() - self.at) / 2).min(want - done);
            out[done * 2..(done + n) * 2].copy_from_slice(&self.buf[self.at..self.at + n * 2]);
            self.at += n * 2;
            done += n;
        }
        done
    }

    /// `ProcessRow`: on to the next tick, and the next row when the row's ticks are over; false
    /// at the end of the song.
    fn advance(&mut self) -> bool {
        if self.started {
            self.tick += 1;
            let ticks_on_row = (self.speed + self.frame_delay) * self.pat_delay.max(1);
            if self.tick >= ticks_on_row {
                let rows = self.m.patterns[self.m.order[self.ord] as usize].rows;
                let (ord, row) = match self.jump.take() {
                    Some(j) => (j, 0),
                    None if self.row + 1 < rows => (self.ord, self.row + 1),
                    None => (self.ord + 1, 0),
                };
                self.pat_delay = 0;
                self.frame_delay = 0;
                self.tick = 0;
                self.row = row;
                if ord != self.ord || row == 0 {
                    self.ord = ord;
                    if !self.seek_order() || self.visited[self.ord] {
                        return false;
                    }
                    self.visited[self.ord] = true;
                }
            }
        }
        if self.tick == 0 {
            let p = &self.m.patterns[self.m.order[self.ord] as usize];
            let ch = self.m.channels;
            for (c, chn) in self.chn.iter_mut().enumerate() {
                chn.row = p.cells[self.row * ch + c];
                chn.vib_on = false;
                chn.trem_on = false;
                chn.command = Cmd::None;
            }
        }
        self.started = true;
        self.first = self.tick == 0 || self.tick % (self.speed + self.frame_delay).max(1) == 0;
        true
    }

    /// Plays one tick into `buf`; false at the end of the song.
    fn next_tick(&mut self) -> bool {
        if !self.advance() {
            return false;
        }
        let mut jump = None;
        for c in 0..self.m.channels {
            self.process_channel(c, &mut jump);
        }
        if self.first && jump.is_some() {
            self.jump = jump;
        }
        self.spt = tick_len(self.rate, self.tempo);
        self.mix_tick();
        true
    }

    /// `GetPeriodFromNote` for frequencies; 0 for no note.
    fn note_period(&self, note: u32, c5: u32) -> i64 {
        if note == 0 || note >= NOTE_MIN_SPECIAL as u32 {
            return 0;
        }
        crate::mptmath::note_freq(&self.t, note.min(255) as u8, c5)
    }

    fn sample(&self, i: usize) -> &Sample {
        &self.m.samples[i.min(self.m.samples.len() - 1)]
    }

    /// The sample's play range and loop on the channel: its sustain loop while the key is on.
    fn set_range(c: &mut Channel, s: &Sample) {
        c.v.length = s.len;
        c.sustain = s.sustain;
        c.v.loop_on = false;
        c.v.loop_start = 0;
        c.v.loop_end = s.len;
        if c.sustain {
            c.v.loop_start = s.sus_start;
            c.v.loop_end = s.sus_end;
            c.v.loop_on = true;
            if c.v.length > c.v.loop_end {
                c.v.length = c.v.loop_end;
            }
        }
    }

    /// `CheckNNA` (always "note cut" here): the playing voice fades out on a copy, and the
    /// channel starts over silent.
    fn cut_to_tail(&mut self, ci: usize) {
        let c = &mut self.chn[ci];
        if c.v.length == 0 || (c.v.gl == 0.0 && c.v.gr == 0.0) {
            return;
        }
        let mut t = c.v.clone();
        t.tl = 0.0;
        t.tr = 0.0;
        t.ramp = self.ramp_down;
        t.sl = -t.gl / self.ramp_down as f32;
        t.sr = -t.gr / self.ramp_down as f32;
        c.tail = Some(t);
        c.v.length = 0;
        c.v.pos = 0.0;
        c.v.gl = 0.0;
        c.v.gr = 0.0;
        c.v.ramp = 0;
    }

    /// `InstrumentChange` without instruments: the sample of that number (0 past the last).
    fn instrument_change(&mut self, ci: usize, instr: u8, porta: bool, upd_vol: bool) {
        if self.chn[ci].new_note == NOTE_NONE {
            return; // an instrument without a note does nothing here
        }
        let mut idx = if (instr as usize) < self.m.samples.len() { instr as usize } else { 0 };
        let c = &self.chn[ci];
        let changed = c.smp.is_some_and(|o| o != idx);
        let porta_keep = changed && porta && c.sample_playing;
        if porta_keep {
            // no sample change during a portamento ("compatible Gxx")
            idx = c.smp.unwrap_or(0);
        }
        let s = self.sample(idx).head();
        let c = &mut self.chn[ci];
        if !porta || changed {
            c.micro = 0;
        }
        if upd_vol && !s.no_default_volume {
            c.volume = s.volume;
        }
        c.new_ins = 0;
        c.ins_vol = 64;
        if c.v.length == 0 || (instr != 0 && porta) {
            c.fast_ramp = true;
        }
        c.keyoff = false;
        c.notefade = false;
        if c.smp != Some(idx) {
            c.v.back = false;
        }
        // the sample's own flags go; the sustain loop comes back only without portamento
        c.v.loop_on = false;
        if porta && c.v.length == 0 {
            c.sample_playing = false;
        }
        c.smp = Some(idx);
        c.v.sample = idx;
        c.v.length = s.len;
        c.v.loop_start = 0;
        c.v.loop_end = s.len;
        c.sustain = s.sustain;
        c.c5 = s.c5;
        c.porta_dest = 0;
        if c.sustain && !porta {
            c.v.loop_start = s.sus_start;
            c.v.loop_end = s.sus_end;
            c.v.loop_on = true;
        }
        if c.v.loop_on && c.v.loop_end < c.v.length {
            c.v.length = c.v.loop_end;
        }
    }

    /// `KeyOff`: the sustain loop lets go (the samples have no other loop: they play out).
    fn key_off(&mut self, ci: usize) {
        let c = &mut self.chn[ci];
        let key_on = !c.keyoff;
        c.keyoff = true;
        if c.v.length == 0 {
            return;
        }
        if c.sustain && key_on {
            if let Some(i) = c.smp {
                let len = self.m.samples[i].len;
                c.v.loop_on = false;
                c.v.back = false;
                c.v.length = len;
            }
        }
    }

    /// `NoteChange`
    fn note_change(&mut self, ci: usize, note: u8, mut porta: bool) {
        if note == NOTE_NONE {
            return;
        }
        if note > NOTE_MAX {
            self.key_off(ci);
            let c = &mut self.chn[ci];
            if note == NOTE_NOTECUT {
                c.notefade = true;
                c.fast_ramp = true;
                c.volume = 0;
                c.sample_playing = false;
                c.fade_vol = 0;
            }
            // the note memory goes with a note cut, off or fade
            c.note = NOTE_NONE;
            c.new_note = NOTE_NONE;
            return;
        }
        let smp = self.chn[ci].smp;
        let head = smp.map(|i| self.sample(i).head());
        if !porta {
            if let Some(s) = &head {
                self.chn[ci].c5 = s.c5;
            }
        }
        if porta && !self.chn[ci].sample_playing {
            porta = false; // nothing to slide from
        }
        let note = note.clamp(1, NOTE_MAX);
        let period = self.note_period(note as u32, self.chn[ci].c5);
        let c = &mut self.chn[ci];
        c.note = note;
        c.paused = false;
        if !porta {
            c.new_ins = 0;
        }
        c.pbr_offset = 0;
        let Some(s) = head else { return };
        if period != 0 {
            if !porta || c.period == 0 {
                c.period = period;
            }
            if porta {
                c.porta_dest = period;
            }
            if !porta || c.v.length == 0 {
                Self::set_range(c, &s);
                c.v.pos = 0.0;
                c.v.back = false;
                if c.vib_type < 4 {
                    c.vib_pos = 0;
                }
            }
            if c.v.pos >= c.v.length as f64 {
                c.v.pos = c.v.loop_start as f64;
            }
        } else {
            porta = false;
        }
        if !porta || c.row.instr != 0 {
            c.notefade = false;
            c.fade_vol = 65536;
        }
        if !porta || c.row.instr != 0 {
            c.keyoff = false;
        }
        if !porta {
            c.fast_ramp = true;
            c.v.gl = 0.0;
            c.v.gr = 0.0;
            c.v.ramp = 0;
        }
    }

    /// `SampleOffset`: applies with or without a note; past the end, from the start.
    fn sample_offset(&mut self, ci: usize, offset: u32) {
        let c = &mut self.chn[ci];
        let off = offset.min(0x1000_0000);
        c.v.pos = off as f64;
        if off >= c.v.length || (c.v.loop_on && off >= c.v.loop_end) {
            c.v.pos = 0.0;
        }
    }

    /// `PortamentoUp` / `PortamentoDown`: the parameter is always the memory, which the first
    /// tick of the row set ("double portamento slides").
    fn porta(&mut self, ci: usize, fine_as_regular: bool, dir: i32) {
        let c = &mut self.chn[ci];
        let param = if dir > 0 { c.old_porta_up } else { c.old_porta_down };
        let first = c.first;
        if !fine_as_regular && param >= 0xE0 {
            let x = (param & 0x0F) as i32;
            if x != 0 && first && c.period != 0 {
                let amount = if param & 0xF0 == 0xF0 { x * 4 } else { x };
                freq_slide(&self.t, &mut c.period, dir * amount);
            }
            return;
        }
        if !first {
            freq_slide(&self.t, &mut c.period, dir * param as i32 * 4);
        }
    }

    /// `TonePortamento` (its own memory, set on the row's first tick; the target clears once
    /// reached)
    fn tone_porta(&mut self, ci: usize) {
        let c = &mut self.chn[ci];
        let delta = c.porta_slide as i32 * 4;
        if c.period != 0 && c.porta_dest != 0 && !c.first {
            if c.period < c.porta_dest {
                freq_slide(&self.t, &mut c.period, delta);
                if c.period > c.porta_dest {
                    c.period = c.porta_dest;
                }
            } else if c.period > c.porta_dest {
                freq_slide(&self.t, &mut c.period, -delta);
                if c.period < c.porta_dest {
                    c.period = c.porta_dest;
                }
            }
        }
        if c.period == c.porta_dest {
            c.porta_dest = 0;
        }
    }

    /// `FineVolumeUp` / `FineVolumeDown`; the volume column has its own memory.
    fn fine_vol(&mut self, ci: usize, param: u8, up: bool, vol_col: bool) {
        let c = &mut self.chn[ci];
        let mem = if vol_col { &mut c.old_vol_param } else { &mut c.old_fine_vol };
        let p = if param != 0 {
            *mem = param;
            param
        } else {
            *mem
        };
        if c.first {
            let d = p as i32 * 4;
            c.volume = if up { (c.volume + d).min(256) } else { (c.volume - d).max(0) };
        }
    }

    /// `VolumeSlide` (S3M-style: x0 up, 0x down, xF / Fx fine, 0F / F0 also on the first tick);
    /// the volume column's slides keep no memory here.
    fn volume_slide(&mut self, ci: usize, param: u8, vol_col: bool) {
        let c = &mut self.chn[ci];
        let p = if vol_col {
            param
        } else if param != 0 {
            c.old_vol_slide = param;
            param
        } else {
            c.old_vol_slide
        };
        let first = c.first;
        let mut v = c.volume;
        if p & 0x0F == 0x0F {
            if p & 0xF0 != 0 {
                return self.fine_vol(ci, p >> 4, true, false);
            } else if first {
                v -= 0x0F * 4;
            }
        } else if p & 0xF0 == 0xF0 {
            if p & 0x0F != 0 {
                return self.fine_vol(ci, p & 0x0F, false, false);
            } else if first {
                v += 0x0F * 4;
            }
        }
        if !first {
            if p & 0x0F != 0 {
                v -= (p & 0x0F) as i32 * 4;
            } else {
                v += ((p & 0xF0) >> 2) as i32;
            }
        }
        c.volume = v.clamp(0, 256);
    }

    /// `PanningSlide` (low nibble right, high nibble left; xF / Fx fine)
    fn pan_slide(&mut self, ci: usize, param: u8) {
        let first = self.first;
        let c = &mut self.chn[ci];
        let p = if param != 0 {
            c.old_pan_slide = param;
            param
        } else {
            c.old_pan_slide
        };
        let mut d = 0;
        if p & 0x0F == 0x0F && p & 0xF0 != 0 {
            if first {
                d = -(((p & 0xF0) / 4) as i32);
            }
        } else if p & 0xF0 == 0xF0 && p & 0x0F != 0 {
            if first {
                d = (p & 0x0F) as i32 * 4;
            }
        } else if !first {
            if p & 0x0F != 0 {
                d = (p & 0x0F) as i32 * 4;
            } else {
                d = -(((p & 0xF0) / 4) as i32);
            }
        }
        if d != 0 {
            c.pan = (c.pan + d).clamp(0, 256);
        }
    }

    /// `Panning`: also ends a held panbrello.
    fn set_pan(c: &mut Channel, pan: i32) {
        c.pan = pan;
        c.fast_ramp = true;
        c.pbr_offset = 0;
    }

    fn vibrato(c: &mut Channel, param: u8) {
        if param & 0x0F != 0 {
            c.vib_depth = (param & 0x0F) * 4;
        }
        if param & 0xF0 != 0 {
            c.vib_speed = param >> 4;
        }
        c.vib_on = true;
    }

    /// `RetrigNote`, IT style: counts down from `param & 15`, retriggers the note (as a
    /// portamento would, from the sample's start) with the volume change of `param >> 4`.
    fn retrig(&mut self, ci: usize, param: u8) {
        let tick = self.tick;
        let c = &mut self.chn[ci];
        let mut doit = false;
        if tick == 0 && c.row.note != NOTE_NONE {
            c.retrig_count = param & 0x0F;
        } else if c.retrig_count == 0 || {
            c.retrig_count -= 1;
            c.retrig_count == 0
        } {
            c.retrig_count = param & 0x0F;
            doit = true;
        }
        if c.v.length == 0 || !doit {
            return;
        }
        let dv = (param >> 4) as usize;
        let mut vol = c.volume;
        if dv != 0 {
            vol = if RETRIG_MUL[dv] != 0 { vol * RETRIG_MUL[dv] / 16 } else { vol + RETRIG_ADD[dv] * 4 };
            vol = vol.clamp(0, 256);
            c.fast_ramp = true;
        }
        let note = c.new_note;
        if (1..=NOTE_MAX).contains(&note) && c.v.length != 0 {
            self.cut_to_tail(ci);
        }
        self.note_change(ci, note, true);
        let c = &mut self.chn[ci];
        c.volume = vol;
        c.v.pos = 0.0;
        if c.row.volcmd == VolCmd::Offset && c.smp.is_some() {
            let off = c.old_offset;
            self.sample_offset(ci, off);
        }
    }

    /// `NoteCut` (`SCx`): the sample stops at tick `x`.
    fn note_cut(&mut self, ci: usize, at: u32) {
        let mut tick = self.tick;
        let row_len = self.speed + self.frame_delay;
        let c = &mut self.chn[ci];
        if c.row.is_note() && c.row.is_tone_porta() {
            if tick < row_len {
                return;
            }
            if self.pat_delay != 0 {
                tick %= row_len.max(1);
            }
        }
        if tick == at {
            c.period = 0;
            c.sample_playing = false;
            c.fade_vol = 0;
            c.notefade = true;
            c.fast_ramp = true;
        }
    }

    /// One channel's share of `ProcessEffects` for this tick.
    fn process_channel(&mut self, ci: usize, jump: &mut Option<usize>) {
        let first = self.first;
        let tick = self.tick;
        let row_len = (self.speed + self.frame_delay).max(1);
        let tick_count = tick % row_len;
        let row = self.chn[ci].row;
        let porta = row.is_tone_porta();
        let (cmd, param, volcmd, vol) = (row.cmd, row.param, row.volcmd, row.vol);
        let mut start = 0;
        if cmd == Cmd::S3mCmdEx {
            if param & 0xF0 == 0xD0 {
                start = (param & 0x0F) as u32;
                if start != 0 && start >= row_len {
                    // a delay past the row: the whole cell is ignored
                    if row.instr != 0 {
                        self.chn[ci].new_ins = row.instr;
                    }
                    return;
                }
            } else if first && param & 0xF0 == 0xE0 {
                self.pat_delay = 1 + (param & 0x0F) as u32;
            }
        }
        let trigger = tick == start || (start > 0 && tick_count == start);
        self.chn[ci].first = tick_count == start;
        if trigger {
            let mut note = row.note;
            let mut instr = row.instr;
            if instr != 0 {
                self.chn[ci].new_ins = instr;
            }
            if note == NOTE_NONE && instr != 0 {
                // an instrument alone: the sample's volume, and the last note again if it
                // names another sample or the sample stopped
                if (instr as usize) < self.m.samples.len() && !self.m.samples[instr as usize].no_default_volume {
                    let v = self.m.samples[instr as usize].volume;
                    let c = &mut self.chn[ci];
                    c.volume = v;
                    c.fast_ramp = true;
                }
                let c = &self.chn[ci];
                if c.smp != Some(instr as usize) || !c.sample_playing {
                    note = c.note;
                }
                instr = 0;
            }
            if note >= NOTE_MIN_SPECIAL {
                if instr != 0 && (instr as usize) < self.m.samples.len() && !self.m.samples[instr as usize].no_default_volume {
                    self.chn[ci].volume = self.m.samples[instr as usize].volume;
                }
                instr = 0;
            }
            if (1..=NOTE_MAX).contains(&note) {
                self.chn[ci].new_note = note;
                if !porta {
                    self.cut_to_tail(ci);
                }
            }
            if instr != 0 {
                let old = self.chn[ci].smp;
                self.instrument_change(ci, instr, porta, true);
                let c = &mut self.chn[ci];
                c.new_ins = 0;
                if (1..=NOTE_MAX).contains(&note) && old != c.smp {
                    c.v.pos = 0.0;
                }
            }
            if note != NOTE_NONE {
                let ni = self.chn[ci].new_ins;
                if instr == 0 && ni != 0 && (1..=NOTE_MAX).contains(&note) {
                    let upd = self.chn[ci].smp.is_none();
                    self.instrument_change(ci, ni, porta, upd);
                    let c = &mut self.chn[ci];
                    c.new_note = note;
                    c.new_ins = 0;
                }
                self.note_change(ci, note, porta);
            }
            let c = &mut self.chn[ci];
            match volcmd {
                VolCmd::Volume => {
                    c.volume = vol.min(64) as i32 * 4;
                    c.fast_ramp = true;
                }
                VolCmd::Panning => Self::set_pan(c, vol.min(64) as i32 * 4),
                _ => {}
            }
        }

        // the first tick sets the slide memories, the volume column's first, then the effect's
        let c = &mut self.chn[ci];
        if c.first {
            if cmd == Cmd::TonePorta && param != 0 {
                c.porta_slide = param as u16;
            }
            if volcmd == VolCmd::TonePorta {
                let p = IT_PORTA_VOLCMD[(vol & 0x0F) as usize];
                if p != 0 {
                    c.porta_slide = p;
                }
            }
            if vol != 0 && matches!(volcmd, VolCmd::PortaUp | VolCmd::PortaDown) {
                c.old_porta_up = vol << 2;
                c.old_porta_down = vol << 2;
            }
            if param != 0 && matches!(cmd, Cmd::PortaUp | Cmd::PortaDown) {
                c.old_porta_up = param;
                c.old_porta_down = param;
            }
        }
        // the volume column
        if volcmd > VolCmd::Panning && tick >= start {
            match volcmd {
                VolCmd::TonePorta => self.tone_porta(ci),
                VolCmd::VolSlideUp | VolCmd::VolSlideDown => {
                    let c = &mut self.chn[ci];
                    let v = if vol == 0 { c.old_vol_param } else { vol };
                    c.old_vol_param = v;
                    if v != 0 {
                        self.volume_slide(ci, if volcmd == VolCmd::VolSlideUp { v << 4 } else { v }, true);
                    }
                }
                VolCmd::FineVolUp | VolCmd::FineVolDown => {
                    if tick == start {
                        self.fine_vol(ci, vol, volcmd == VolCmd::FineVolUp, true);
                    }
                }
                VolCmd::VibratoSpeed => Self::vibrato(&mut self.chn[ci], vol << 4),
                VolCmd::VibratoDepth => Self::vibrato(&mut self.chn[ci], vol),
                VolCmd::PanSlideLeft => self.pan_slide(ci, vol),
                VolCmd::PanSlideRight => self.pan_slide(ci, vol << 4),
                VolCmd::PortaUp => self.porta(ci, true, 1),
                VolCmd::PortaDown => self.porta(ci, true, -1),
                VolCmd::Offset => {
                    if trigger && vol == 0 && self.chn[ci].smp.is_some() {
                        let off = self.chn[ci].old_offset;
                        self.sample_offset(ci, off);
                    }
                }
                VolCmd::PlayControl => {
                    let c = &mut self.chn[ci];
                    if c.first {
                        match vol {
                            0 => c.paused = true,
                            1 => c.paused = false,
                            2 => c.v.back = false,
                            3 => c.v.back = true,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        // the effect column
        match cmd {
            Cmd::None => {}
            Cmd::Volume => {
                if first {
                    let c = &mut self.chn[ci];
                    c.volume = if param < 64 { param as i32 * 4 } else { 256 };
                    c.fast_ramp = true;
                }
            }
            Cmd::PortaUp => self.porta(ci, false, 1),
            Cmd::PortaDown => self.porta(ci, false, -1),
            Cmd::VolumeSlide => self.volume_slide(ci, param, false),
            Cmd::TonePorta => self.tone_porta(ci),
            Cmd::Vibrato => Self::vibrato(&mut self.chn[ci], param),
            Cmd::Speed => {
                if first && param > 0 {
                    self.speed = param as u32;
                }
            }
            Cmd::Tempo => {
                if first && param >= 32 {
                    self.tempo = param as u32;
                }
            }
            Cmd::Offset => {
                if trigger {
                    let c = &mut self.chn[ci];
                    let mut off = (param as u32) << 8;
                    if off != 0 {
                        c.old_offset = off;
                    } else if volcmd != VolCmd::Offset {
                        off = c.old_offset;
                    }
                    let hi = (c.old_hi_offset as u32) << 16;
                    self.sample_offset(ci, off + hi);
                }
            }
            Cmd::Arpeggio => {
                if tick == 0 {
                    let c = &mut self.chn[ci];
                    c.command = Cmd::Arpeggio;
                    if param != 0 {
                        c.arpeggio = param;
                    }
                }
            }
            Cmd::Retrig => {
                if param != 0 {
                    self.chn[ci].retrig_param = param;
                }
                let p = self.chn[ci].retrig_param;
                self.retrig(ci, p);
            }
            Cmd::Tremor => {
                if first {
                    let c = &mut self.chn[ci];
                    if param != 0 {
                        let mut p = param;
                        if p & 0xF0 != 0 {
                            p -= 0x10;
                        }
                        if p & 0x0F != 0 {
                            p -= 0x01;
                        }
                        c.tremor_param = p;
                    }
                    c.tremor_count |= 0x80;
                    c.command = Cmd::Tremor;
                }
            }
            Cmd::Panning8 => {
                if first {
                    Self::set_pan(&mut self.chn[ci], param as i32);
                }
            }
            Cmd::PanningSlide => self.pan_slide(ci, param),
            Cmd::Tremolo => {
                let c = &mut self.chn[ci];
                if param & 0x0F != 0 {
                    c.trem_depth = (param & 0x0F) << 2;
                }
                if param & 0xF0 != 0 {
                    c.trem_speed = param >> 4;
                }
                c.trem_on = true;
            }
            Cmd::Panbrello => {
                let c = &mut self.chn[ci];
                if param & 0x0F != 0 {
                    c.pbr_depth = param & 0x0F;
                }
                if param & 0xF0 != 0 {
                    c.pbr_speed = param >> 4;
                }
            }
            Cmd::Finetune => {
                if first {
                    self.chn[ci].micro = ((param as i32) << 8) - 0x8000;
                }
            }
            Cmd::PositionJump => *jump = Some(param as usize),
            Cmd::S3mCmdEx => self.extended(ci, param),
        }
    }

    /// `ExtendedS3MCommands`, the ones the loader writes or moves (Sxx without memory here).
    fn extended(&mut self, ci: usize, param: u8) {
        let first = self.first;
        let tick = self.tick;
        let x = param & 0x0F;
        let c = &mut self.chn[ci];
        match param & 0xF0 {
            0x30 => c.vib_type = if x < 4 { x } else { 0 },
            0x40 => c.trem_type = if x < 4 { x } else { 0 },
            0x50 => {
                c.pbr_type = if x < 4 { x } else { 0 };
                c.pbr_pos = 0;
            }
            0x60 => {
                if first && tick == 0 {
                    self.frame_delay += x as u32;
                }
            }
            0x80 => {
                if first {
                    Self::set_pan(c, (x as i32 * 256 + 8) / 15);
                }
            }
            0x90 => {
                if first {
                    if x == 0x0E {
                        c.v.back = false;
                    } else if x == 0x0F {
                        if c.v.pos == 0.0 && c.v.length != 0 && (c.row.is_note() || !c.v.loop_on) {
                            c.v.pos = c.v.length as f64 - 1.0 / 4294967296.0;
                        }
                        c.v.back = true;
                    }
                }
            }
            0xA0 => {
                if first {
                    c.old_hi_offset = x; // the note is not moved
                }
            }
            0xC0 => self.note_cut(ci, x as u32),
            _ => {}
        }
    }

    /// Volumes, pitches and the mix of one tick (`ReadNote`).
    fn mix_tick(&mut self) {
        let spt = self.spt;
        self.buf.clear();
        self.buf.resize(spt * 2, 0.0);
        let first = self.first;
        let tick = self.tick;
        for ci in 0..self.chn.len() {
            let out: &mut [f32] = &mut self.buf;
            let c = &mut self.chn[ci];
            if c.notefade && c.fade_vol == 0 && c.v.gl == 0.0 && c.v.gr == 0.0 {
                c.v.length = 0;
            }
            let playing = c.period != 0 && c.v.length != 0;
            c.sample_playing = playing;
            let mut period = c.period;
            let mut real = 0i64;
            if playing {
                let mut vol = c.volume;
                // tremolo: in DMF a share of the note's volume
                if c.trem_on {
                    let delta = vib_delta(c.trem_type, c.trem_pos);
                    vol -= vol * c.trem_depth as i32 * (64 - delta) / (128 * 64);
                    if !first {
                        c.trem_pos = c.trem_pos.wrapping_add(c.trem_speed.wrapping_mul(4));
                    }
                }
                // tremor, IT style: on for x + 1 ticks, off for y + 1 (the parameter is stored
                // minus one per nibble)
                if c.command == Cmd::Tremor {
                    if c.tremor_count & 0x80 != 0 && c.v.length != 0 {
                        c.tremor_count = match c.tremor_count {
                            0x80 => (c.tremor_param >> 4) | 0xC0,
                            0xC0 => (c.tremor_param & 0x0F) | 0x80,
                            n => n.wrapping_sub(1),
                        };
                    }
                    if c.tremor_count & 0xC0 == 0x80 {
                        vol = 0;
                    }
                    c.fast_ramp = true;
                }
                let mut vol = (vol.clamp(0, 256) as i64) << 6;
                // no instrument: a fading note is cut
                if c.notefade {
                    c.fade_vol = 0;
                    vol = 0;
                }
                if c.paused {
                    vol = 0;
                }
                real = vol * 256 * (c.global_vol as i64 * c.ins_vol as i64) >> 20;
                // arpeggio, IT style: the pitch times a linear-slide ratio
                if c.command == Cmd::Arpeggio && c.arpeggio != 0 {
                    let step = match (tick % (self.speed + self.frame_delay).max(1)) % 3 {
                        1 => (c.arpeggio >> 4) as usize,
                        2 => (c.arpeggio & 0x0F) as usize,
                        _ => 0,
                    };
                    if step != 0 {
                        period = crate::mptmath::muldivr(period, self.t.up[step * 16] as i64, 65536);
                    }
                }
                // panbrello, held after its rows until a note or a panning command
                c.real_pan = c.pan;
                let mut d = c.pbr_offset;
                if c.row.cmd == Cmd::Panbrello {
                    d = vib_delta(c.pbr_type, c.pbr_pos);
                    c.pbr_pos = c.pbr_pos.wrapping_add(c.pbr_speed);
                    c.pbr_offset = d;
                }
                if d != 0 {
                    c.real_pan = (c.real_pan + (d * c.pbr_depth as i32 + 2) / 8).clamp(0, 256);
                }
            }
            // vibrato, IT style (its position moves on with or without a sample, before use)
            if c.vib_on {
                if !first {
                    c.vib_pos = c.vib_pos.wrapping_add(c.vib_speed.wrapping_mul(4));
                }
                let d = vib_delta(c.vib_type, c.vib_pos);
                freq_slide(&self.t, &mut period, d * c.vib_depth as i32 / 64);
            }
            let s = &self.m.samples[c.v.sample.min(self.m.samples.len() - 1)];
            let audible = playing && s.has_data() && c.smp.is_some();
            if audible {
                // `GetFreqFromPeriod` keeps a frequency below 2^24 Hz
                let mut freq = period.clamp(1, 0xFF_FFFF) as f64;
                if c.micro != 0 {
                    freq = (freq * 2f64.powf(c.micro as f64 / (12.0 * 256.0 * 128.0))).round();
                }
                c.v.inc = freq.min(i32::MAX as f64) / self.rate as f64;
                if c.v.back {
                    c.v.inc = -c.v.inc;
                }
                // compatible mix levels: pre-amp 48, no global pre-amp, extra attenuation 1
                let realvol = real * 48 / 128 / 2;
                let pan = c.real_pan.clamp(0, 256) as i64;
                let nl = (realvol * (256 - pan) / 256 / 2) as f32 / 4096.0;
                let nr = (realvol * pan / 256 / 2) as f32 / 4096.0;
                ramp_to(&mut c.v, nl, nr, c.fast_ramp, spt as u32, self.ramp_up, self.ramp_down);
                c.fast_ramp = false;
                mix_voice(&mut c.v, s, out, self.nearest);
            } else {
                c.v.gl = 0.0;
                c.v.gr = 0.0;
                c.v.tl = 0.0;
                c.v.tr = 0.0;
                c.v.ramp = 0;
                c.v.length = 0;
            }
            if let Some(t) = &mut c.tail {
                let ts = &self.m.samples[t.sample.min(self.m.samples.len() - 1)];
                mix_voice(t, ts, out, self.nearest);
                if t.ramp == 0 || t.length == 0 {
                    c.tail = None;
                }
            }
        }
        self.at = 0;
    }
}

impl Sample {
    /// The sample without its data (what the channel needs to know of it).
    fn head(&self) -> Sample {
        Sample {
            data: Data::Empty,
            len: self.len,
            sustain: self.sustain,
            sus_start: self.sus_start,
            sus_end: self.sus_end,
            c5: self.c5,
            volume: self.volume,
            no_default_volume: self.no_default_volume,
        }
    }
}

/// `ProcessRamping`: towards the new gains, over the up or down ramp, or over the whole tick
/// (at most 2048 frames) between two non-zero volumes.
fn ramp_to(v: &mut Voice, nl: f32, nr: f32, fast: bool, spt: u32, up: u32, down: u32) {
    if nl == v.tl && nr == v.tr && (v.ramp > 0 || (v.gl == nl && v.gr == nr)) {
        return;
    }
    if v.gl == nl && v.gr == nr {
        v.tl = nl;
        v.tr = nr;
        v.ramp = 0;
        return;
    }
    let rising = nl > v.gl || nr > v.gr;
    let global = if rising { up } else { down };
    let len = if (v.gl != 0.0 || v.gr != 0.0) && (nl != 0.0 || nr != 0.0) && !fast { spt.clamp(global, 2048.max(global)) } else { global };
    v.tl = nl;
    v.tr = nr;
    v.ramp = len.max(1);
    v.sl = (nl - v.gl) / v.ramp as f32;
    v.sr = (nr - v.gr) / v.ramp as f32;
}

/// Mixes one tick of a voice into `out`, advancing it (`Fastmix.cpp`'s loop and end handling,
/// forwards and backwards, without ping-pong loops, which DMF has none of).
fn mix_voice(v: &mut Voice, s: &Sample, out: &mut [f32], nearest: bool) {
    let frames = out.len() / 2;
    let data_len = match &s.data {
        Data::I8(d) => d.len(),
        Data::I16(d) => d.len(),
        Data::Empty => 0,
    };
    let get = |i: usize| -> f32 {
        match &s.data {
            Data::I16(d) => d.get(i).map_or(0.0, |&x| x as f32 / 32768.0),
            Data::I8(d) => d.get(i).map_or(0.0, |&x| x as f32 / 128.0),
            Data::Empty => 0.0,
        }
    };
    for f in 0..frames {
        if v.ramp > 0 {
            v.gl += v.sl;
            v.gr += v.sr;
            v.ramp -= 1;
            if v.ramp == 0 {
                v.gl = v.tl;
                v.gr = v.tr;
            }
        }
        if v.length == 0 || data_len == 0 {
            continue;
        }
        let len = v.length.min(data_len as u32) as f64;
        let ls = if v.loop_on { v.loop_start as f64 } else { 0.0 };
        if v.pos.floor() < ls {
            if v.inc < 0.0 {
                // below the loop start backwards: on from the end of the loop
                v.pos = len - 1.0;
                if !v.loop_on || v.pos >= len {
                    v.length = 0;
                    continue;
                }
            } else if v.pos < 0.0 {
                v.pos = 0.0;
            }
        } else if v.pos >= len {
            if !v.loop_on {
                v.length = 0;
                continue;
            }
            if v.inc < 0.0 {
                v.inc = -v.inc;
            }
            v.pos += ls - len;
            if v.pos < ls || v.pos >= len {
                v.pos = ls;
            }
        }
        let i = v.pos as usize;
        let x = if nearest {
            get(i)
        } else {
            let frac = (v.pos - i as f64) as f32;
            let j = i + 1;
            let b = if (j as f64) < len {
                get(j)
            } else if v.loop_on {
                get(v.loop_start as usize)
            } else {
                0.0
            };
            let a = get(i);
            a + (b - a) * frac
        };
        out[f * 2] += x * v.gl;
        out[f * 2 + 1] += x * v.gr;
        v.pos += v.inc;
    }
}
