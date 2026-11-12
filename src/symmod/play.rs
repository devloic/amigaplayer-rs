//! Plays what `load.rs` converts a Symphonie module to: rows of OpenMPT-style commands, with
//! the meaning OpenMPT gives them in an MPTM module with Impulse Tracker play behaviour, linear
//! slides and frequencies as periods (libopenmpt 0.8.7 `soundlib/Snd_fx.cpp`, `Sndmix.cpp`,
//! `Snd_flt.cpp`, `plugins/SymMODEcho.cpp`, BSD-3-Clause; see `mod.rs`). Only the commands the
//! loader writes are implemented, and only along the paths their IT behaviour takes.
//!
//! The mixer is this file's own, not OpenMPT's: one mono voice per channel (hard left for even
//! channels, hard right for odd ones, as the loader pans them), linear interpolation, OpenMPT's
//! volume ramps (363 us up, 952 us down, a whole tick between two non-zero volumes), a fading
//! copy of the old voice when a note cuts it (OpenMPT's NNA "note cut" channel), the IT resonant
//! filter per channel, and the Symphonie echo on the channels whose instrument does not bypass
//! it.

use super::load::{Cell, Cmd, Data, Macro, Module, VolCmd, NOTE_NONE, NO_CUE};

/// `ITSinusTable` (libopenmpt `Tables.cpp`)
const IT_SINE: [i8; 256] = [
    0, 2, 3, 5, 6, 8, 9, 11, 12, 14, 16, 17, 19, 20, 22, 23, 24, 26, 27, 29, 30, 32, 33, 34, 36, 37, 38, 39, 41, 42, 43, 44, 45, 46,
    47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 56, 57, 58, 59, 59, 60, 60, 61, 61, 62, 62, 62, 63, 63, 63, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 63, 63, 63, 62, 62, 62, 61, 61, 60, 60, 59, 59, 58, 57, 56, 56, 55, 54, 53, 52, 51, 50, 49, 48, 47, 46, 45, 44,
    43, 42, 41, 39, 38, 37, 36, 34, 33, 32, 30, 29, 27, 26, 24, 23, 22, 20, 19, 17, 16, 14, 12, 11, 9, 8, 6, 5, 3, 2, 0, -2, -3,
    -5, -6, -8, -9, -11, -12, -14, -16, -17, -19, -20, -22, -23, -24, -26, -27, -29, -30, -32, -33, -34, -36, -37, -38, -39, -41,
    -42, -43, -44, -45, -46, -47, -48, -49, -50, -51, -52, -53, -54, -55, -56, -56, -57, -58, -59, -59, -60, -60, -61, -61, -62,
    -62, -62, -63, -63, -63, -64, -64, -64, -64, -64, -64, -64, -64, -64, -64, -64, -63, -63, -63, -62, -62, -62, -61, -61, -60,
    -60, -59, -59, -58, -57, -56, -56, -55, -54, -53, -52, -51, -50, -49, -48, -47, -46, -45, -44, -43, -42, -41, -39, -38, -37,
    -36, -34, -33, -32, -30, -29, -27, -26, -24, -23, -22, -20, -19, -17, -16, -14, -12, -11, -9, -8, -6, -5, -3, -2,
];

/// `ImpulseTrackerPortaVolCmd`
const IT_PORTA_VOLCMD: [u16; 16] = [0x00, 0x01, 0x04, 0x08, 0x10, 0x20, 0x40, 0x60, 0x80, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

use crate::mptmath::{freq_slide, Tables};

/// The playing part of a channel: what the mixer reads.
#[derive(Clone, Default)]
struct Voice {
    sample: usize,
    pos: f64,
    inc: f64,
    /// `nLength`: the end of the playing part (the loop end for a looped sample); 0 = stopped
    length: u32,
    loop_on: bool,
    loop_start: u32,
    loop_end: u32,
    /// gain now, its ramp step and the samples of ramp left
    gain: f32,
    step: f32,
    ramp: u32,
    target: f32,
    wet: bool,
    right: bool,
    filter: Filter,
}

#[derive(Clone, Copy, Default)]
struct Filter {
    on: bool,
    a0: f32,
    b0: f32,
    b1: f32,
    hp: f32,
    y1: f32,
    y2: f32,
}

#[derive(Clone, Default)]
struct Channel {
    row: Cell,
    v: Voice,
    /// a cut note fading out (OpenMPT's NNA channel)
    tail: Option<Voice>,
    instr: usize,
    old_ins: u8,
    new_ins: u8,
    note: u8,
    new_note: u8,
    c5: u32,
    period: i64,
    porta_dest: i64,
    porta_slide: u16,
    old_porta_up: u8,
    old_porta_down: u8,
    volume: i32,
    global_vol: i32,
    ins_vol: i32,
    old_chn_vol_slide: u8,
    vib_speed: u8,
    vib_depth: u8,
    vib_pos: u8,
    vib_on: bool,
    trem_speed: u8,
    trem_depth: u8,
    trem_pos: u8,
    trem_on: bool,
    retrig_param: u8,
    retrig_count: u8,
    paused: bool,
    keyoff: bool,
    notefade: bool,
    fade_vol: i32,
    old_offset: u32,
    cutoff: u8,
    reso: u8,
    highpass: bool,
    fast_ramp: bool,
    no_dsp: bool,
    trigger: bool,
}

/// The Symphonie echo DSP (`SymMODEcho`).
struct Echo {
    kind: u8,
    delay: u8,
    feedback: f32,
    line: Vec<f32>,
    write: usize,
}

impl Echo {
    fn new(spt: usize) -> Echo {
        // OpenMPT clears 127 ticks of delay line; only the delay in use is ever read or written,
        // so it grows (zeroed) to that on demand, which sounds the same and allocates less
        let _ = spt;
        let mut e = Echo { kind: 0, delay: 4, feedback: 0.5, line: Vec::new(), write: 0 };
        e.set(0, 4, 1);
        e
    }
    fn set(&mut self, kind: u8, delay: u8, feedback: u8) {
        self.kind = if kind >= 5 { 0 } else { kind };
        self.delay = delay.min(127);
        let fb = feedback.min(127) as f32;
        self.feedback = if self.kind == 3 { 1.0 - 2f32.powf(-(fb + 1.0)) } else { 2f32.powf(-fb) };
    }
    fn process(&mut self, buf: &mut [f32], spt: usize) {
        let delay = spt * self.delay as usize;
        if self.line.len() < delay * 2 {
            self.line.resize(delay * 2, 0.0);
        }
        if self.kind == 0 {
            return; // off: the delay line is kept as it is
        }
        for f in buf.chunks_exact_mut(2) {
            if self.write >= delay {
                self.write = 0;
            }
            // write - delay, wrapped into the delay: the oldest frame, the one written next
            let read = self.write;
            let (ld, rd) = (f[0], f[1]);
            let (le, re) = (self.line[read * 2], self.line[read * 2 + 1]);
            f[0] = ld + le;
            f[1] = rd + re;
            let (mut lo, mut ro) = match self.kind {
                1 => ((le + ld) * self.feedback, (re + rd) * self.feedback),
                2 | 3 => ((re + rd) * self.feedback, (le + ld) * self.feedback),
                _ => {
                    let l = (le + (ld + rd) * 0.5) * self.feedback;
                    (l, l)
                }
            };
            if lo.abs() < 1e-24 {
                lo = 0.0;
            }
            if ro.abs() < 1e-24 {
                ro = 0.0;
            }
            self.line[self.write * 2] = lo;
            self.line[self.write * 2 + 1] = ro;
            self.write += 1;
        }
    }
}

pub struct Player {
    m: Module,
    t: Tables,
    rate: u32,
    nearest: bool,
    ramp_up: u32,
    ramp_down: u32,
    chn: Vec<Channel>,
    echo: Echo,
    ord: usize,
    row: usize,
    tick: u32,
    speed: u32,
    spt: usize,
    gain: f32,
    started: bool,
    ended: bool,
    /// interleaved stereo of the tick being played out
    buf: Vec<f32>,
    wet: Vec<f32>,
    at: usize,
}

impl Player {
    pub fn new(m: Module, rate: u32) -> Player {
        let rate = rate.clamp(1000, 384_000);
        let tempo_raw = (m.tempo * 10000.0).round().max(1.0) as u64;
        let spt = ((rate as u64 * 5 * 10000) / (tempo_raw * 2)).clamp(1, 1 << 20) as usize;
        let n = m.channels;
        // the mix level: the sample pre-amp of OpenMPT (512 / channels, out of 128), which
        // matches openmpt123's render at 0 dB to the RMS (acoustic modeling demo: 0.00 dB)
        let gain = m.preamp as f32 / 128.0;
        let mut p = Player {
            t: Tables::new(),
            rate,
            nearest: false,
            ramp_up: ((363 * rate as u64 + 500_000) / 1_000_000).max(1) as u32,
            ramp_down: ((952 * rate as u64 + 500_000) / 1_000_000).max(1) as u32,
            chn: Vec::new(),
            echo: Echo::new(spt),
            ord: 0,
            row: 0,
            tick: 0,
            speed: 6,
            spt,
            gain,
            started: false,
            ended: false,
            buf: Vec::new(),
            wet: Vec::new(),
            at: 0,
            m,
        };
        p.chn = vec![Channel::default(); n];
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
            .map(|i| Channel {
                global_vol: 64,
                volume: 256,
                fade_vol: 65536,
                cutoff: 0x7F,
                c5: 8363,
                v: Voice { right: i & 1 == 1, ..Voice::default() },
                ..Channel::default()
            })
            .collect();
        self.echo = Echo::new(self.spt);
        self.ord = 0;
        self.row = 0;
        self.tick = 0;
        self.speed = 6;
        self.started = false;
        self.ended = !self.seek_order();
        self.buf.clear();
        self.at = 0;
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    /// (order, pattern, row)
    pub fn position(&self) -> (i32, i32, i32) {
        let pat = self.m.order.get(self.ord).copied().flatten().map_or(-1, |p| p as i32);
        (self.ord as i32, pat, self.row as i32)
    }

    /// Moves `ord` on to the next order entry that is a pattern; false at the end.
    fn seek_order(&mut self) -> bool {
        while self.ord < self.m.order.len() {
            if let Some(p) = self.m.order[self.ord] {
                if p < self.m.patterns.len() {
                    return true;
                }
            }
            self.ord += 1;
        }
        false
    }

    /// The song's length in milliseconds (rows and speed changes, as played).
    pub fn duration_ms(&self) -> i32 {
        let tempo_raw = (self.m.tempo * 10000.0).round().max(1.0) as u64;
        let spt = (48000u64 * 5 * 10000) / (tempo_raw * 2);
        // each pattern once: the rows before its first speed command (played at the speed it is
        // entered with), the ticks of the rest, and the speed it leaves with; then the order list
        // in one pass (a long order list of big patterns stays cheap)
        let ch = self.m.channels;
        let summary: Vec<(u64, u64, Option<u64>)> = self
            .m
            .patterns
            .iter()
            .map(|pat| {
                let (mut before, mut fixed, mut speed) = (0u64, 0u64, None);
                for row in 0..pat.rows {
                    for c in &pat.cells[row * ch..(row + 1) * ch] {
                        if c.cmd == Cmd::Speed && c.param > 0 {
                            speed = Some(c.param as u64);
                        }
                    }
                    match speed {
                        Some(s) => fixed += s,
                        None => before += 1,
                    }
                }
                (before, fixed, speed)
            })
            .collect();
        let mut speed = 6u64;
        let mut ticks = 0u64;
        for p in self.m.order.iter().flatten() {
            let Some(&(before, fixed, exit)) = summary.get(*p) else { continue };
            ticks = ticks.saturating_add(before * speed + fixed);
            speed = exit.unwrap_or(speed);
        }
        let ticks = ticks.min(u64::MAX / (spt.max(1) * 1000));
        (ticks * spt * 1000 / 48000).min(i32::MAX as u64) as i32
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

    /// Plays one tick into `buf`; false at the end of the song.
    fn next_tick(&mut self) -> bool {
        if self.started {
            self.tick += 1;
            if self.tick >= self.speed {
                self.tick = 0;
                self.row += 1;
                let rows = self.m.order.get(self.ord).copied().flatten().map_or(0, |p| self.m.patterns[p].rows);
                if self.row >= rows {
                    self.row = 0;
                    self.ord += 1;
                    if !self.seek_order() {
                        return false;
                    }
                }
            }
        }
        self.started = true;
        if self.tick == 0 {
            let p = self.m.order[self.ord].unwrap_or(0);
            let ch = self.m.channels;
            for c in 0..ch {
                let cell = self.m.patterns[p].cells[self.row * ch + c];
                self.chn[c].row = cell;
            }
            // the speed is a global command: set before the channels (OpenMPT runs all channels'
            // effects of the row on its first tick, so the order does not matter there)
        }
        for c in 0..self.m.channels {
            self.process_channel(c);
        }
        self.mix_tick();
        true
    }

    /// `GetPeriodFromNote` for frequencies: c5 * 2^((note - 61) / 12).
    fn note_period(&self, note: u8, c5: u32) -> i64 {
        crate::mptmath::note_freq(&self.t, note, c5)
    }

    fn is_playing(c: &Channel) -> bool {
        c.v.length != 0 && c.period != 0
    }

    /// `InstrumentChange`
    fn instrument_change(&mut self, ci: usize, instr: u8, porta: bool) {
        let (sample, gv, no_dsp) = match self.m.instruments.get(instr as usize).and_then(|i| i.as_ref()) {
            Some(i) => (i.sample, i.gv as i32, i.no_dsp),
            None => (0, 0, false),
        };
        let c = &mut self.chn[ci];
        let changed = c.instr != instr as usize;
        c.instr = instr as usize;
        c.no_dsp = no_dsp;
        if no_dsp {
            // SetCutoff(127) / SetResonance(0) of the instrument
            c.cutoff = 0x7F;
            c.reso = 0;
        }
        let Some(s) = self.m.samples.get(sample).filter(|_| sample != 0) else {
            c.v.sample = 0;
            c.ins_vol = 0;
            return;
        };
        c.ins_vol = (s.gv as i32 * gv) >> 6;
        c.volume = 256;
        c.fast_ramp = true;
        if porta && c.v.sample == sample && c.v.length != 0 {
            c.keyoff = false;
            c.notefade = false;
            return;
        }
        let _ = changed;
        c.keyoff = false;
        c.notefade = false;
        c.v.sample = sample;
        c.c5 = s.c5;
        let len = s.data.len() as u32;
        c.v.length = len;
        c.v.loop_on = s.loop_on && s.loop_end > s.loop_start && s.loop_end <= len;
        c.v.loop_start = s.loop_start;
        c.v.loop_end = s.loop_end;
        if c.v.loop_on {
            c.v.length = c.v.length.min(s.loop_end);
        }
        if c.v.pos >= c.v.length as f64 {
            c.v.pos = 0.0;
        }
    }

    /// The old note's voice fades out (NNA note cut).
    fn cut_to_tail(&mut self, ci: usize) {
        let c = &mut self.chn[ci];
        if c.v.length != 0 && c.v.gain > 0.0 && c.v.sample != 0 {
            let mut t = c.v.clone();
            t.target = 0.0;
            t.ramp = self.ramp_down;
            t.step = -t.gain / self.ramp_down as f32;
            c.tail = Some(t);
        }
    }

    /// `NoteChange`
    fn note_change(&mut self, ci: usize, note: u8, mut porta: bool, reset_env: bool) {
        if note == NOTE_NONE {
            return;
        }
        if note as i32 > 128 {
            // a note-off: the note fades (instrument fade-out 256, no volume envelope)
            let c = &mut self.chn[ci];
            c.keyoff = true;
            if c.instr != 0 {
                c.notefade = true;
            }
            c.note = NOTE_NONE;
            c.new_note = NOTE_NONE;
            return;
        }
        let sample = self.m.instruments.get(self.chn[ci].instr).and_then(|i| i.as_ref()).map_or(0, |i| i.sample);
        let smp = self.m.samples.get(sample).filter(|_| sample != 0).map(|s| (s.c5, s.data.len() as u32, s.loop_on, s.loop_start, s.loop_end));
        if !porta {
            if let Some((c5, ..)) = smp {
                self.chn[ci].c5 = c5;
            }
        }
        if porta && !Self::is_playing(&self.chn[ci]) {
            porta = false;
        }
        let period = self.note_period(note, self.chn[ci].c5);
        let c = &mut self.chn[ci];
        c.note = note;
        c.paused = false;
        c.new_ins = 0;
        let Some((_, len, loop_on, ls, le)) = smp else {
            c.vib_on = false;
            c.trem_on = false;
            return;
        };
        if period != 0 {
            if !porta || c.period == 0 {
                c.period = period;
            }
            if porta {
                c.porta_dest = period;
            }
            if !porta || c.v.length == 0 {
                c.v.sample = sample;
                c.v.length = len;
                c.v.loop_on = loop_on && le > ls && le <= len;
                c.v.loop_start = ls;
                c.v.loop_end = le;
                if c.v.loop_on {
                    c.v.length = c.v.length.min(le);
                }
                c.v.pos = 0.0;
                c.vib_pos = 0;
            }
            if c.v.pos >= c.v.length as f64 {
                c.v.pos = c.v.loop_start as f64;
            }
        } else {
            porta = false;
        }
        if !porta || (c.notefade && c.fade_vol == 0) {
            c.notefade = false;
            c.fade_vol = 65536;
        }
        c.keyoff = false;
        if !porta {
            c.trigger = true;
            c.fast_ramp = true;
            c.v.gain = 0.0;
            c.v.ramp = 0;
            c.v.filter.y1 = 0.0;
            c.v.filter.y2 = 0.0;
            let _ = reset_env;
        }
        // NoteChange resets the auto slides (vibrato, tremolo)
        c.vib_on = false;
        c.trem_on = false;
    }

    /// `SampleOffset` (IT): past the end, the note starts at the beginning.
    fn sample_offset(&mut self, ci: usize, offset: u32) {
        let c = &mut self.chn[ci];
        let note = if c.row.instr != 0 { c.new_note } else { c.row.note };
        if !(1..=128).contains(&note) {
            return;
        }
        let off = offset.min(NO_CUE);
        c.v.pos = off as f64;
        if off >= c.v.length || (c.v.loop_on && off >= c.v.loop_end) {
            c.v.pos = 0.0;
        }
    }

    fn porta_up(&mut self, ci: usize, param: u8, fine_as_regular: bool) {
        let first = self.tick == 0;
        let c = &mut self.chn[ci];
        let param = if param != 0 {
            c.old_porta_down = param;
            c.old_porta_up = param;
            param
        } else {
            c.old_porta_up
        };
        self.slide(ci, param, fine_as_regular, first, 1);
    }

    fn porta_down(&mut self, ci: usize, param: u8, fine_as_regular: bool) {
        let first = self.tick == 0;
        let c = &mut self.chn[ci];
        let param = if param != 0 {
            c.old_porta_up = param;
            c.old_porta_down = param;
            param
        } else {
            c.old_porta_down
        };
        self.slide(ci, param, fine_as_regular, first, -1);
    }

    fn slide(&mut self, ci: usize, param: u8, fine_as_regular: bool, first: bool, dir: i32) {
        let c = &mut self.chn[ci];
        if !fine_as_regular && param >= 0xE0 {
            if param & 0x0F != 0 {
                let x = (param & 0x0F) as i32;
                if param & 0xF0 == 0xF0 {
                    if first && c.period != 0 {
                        freq_slide(&self.t, &mut c.period, dir * x * 4);
                    }
                } else if first && c.period != 0 {
                    freq_slide(&self.t, &mut c.period, dir * x);
                }
            }
            return;
        }
        if !first {
            freq_slide(&self.t, &mut c.period, dir * param as i32 * 4);
        }
    }

    /// `InitTonePortamento`, with the effect memory shared with the portamentos.
    fn init_tone_porta(&mut self, ci: usize, param: u16) {
        let c = &mut self.chn[ci];
        let param = if param == 0 { c.old_porta_up as u16 } else { param };
        c.old_porta_up = param as u8;
        c.old_porta_down = param as u8;
        if param != 0 {
            c.porta_slide = param;
        }
    }

    fn tone_porta(&mut self, ci: usize) {
        let first = self.tick == 0;
        let c = &mut self.chn[ci];
        let delta = c.porta_slide as i32 * 4;
        if c.period != 0 && c.porta_dest != 0 && !first {
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

    /// `RetrigNote` (IT): restarts the note from the beginning every `param & 15` ticks.
    fn retrig(&mut self, ci: usize, param: u8) {
        let c = &mut self.chn[ci];
        let speed = param & 0x0F;
        let mut doit = false;
        if self.tick == 0 && c.row.note != NOTE_NONE {
            c.retrig_count = speed;
        } else if c.retrig_count == 0 || {
            c.retrig_count -= 1;
            c.retrig_count == 0
        } {
            c.retrig_count = speed;
            doit = true;
        }
        if !doit {
            return;
        }
        let note = c.new_note;
        let old_period = c.period;
        let vol = c.volume;
        if (1..=128).contains(&note) && c.v.length != 0 {
            self.cut_to_tail(ci);
        }
        self.note_change(ci, note, true, false);
        let c = &mut self.chn[ci];
        c.volume = vol;
        if c.row.note == NOTE_NONE && old_period != 0 {
            c.period = old_period;
        }
        c.v.pos = 0.0;
    }

    /// `SetupChannelFilter` with the IT filter behaviour and the extended range.
    fn setup_filter(&mut self, ci: usize, reset: bool) {
        let rate = self.rate as f32;
        let c = &mut self.chn[ci];
        let (cutoff, reso) = (c.cutoff.min(127) as i32, c.reso.min(127) as i32);
        let f = &mut c.v.filter;
        if reso == 0 && cutoff * 2 >= 254 {
            if c.trigger {
                f.on = false;
            }
            return;
        }
        f.on = true;
        let dmpfac = 10f32.powf(-(reso as f32) * ((24.0 / 128.0) / 20.0));
        let freq = (110.0 * 2f32.powf(0.25 + cutoff as f32 * 512.0 / (20.0 * 512.0))).clamp(120.0, 20000.0).min(rate * 0.5);
        let fc = freq * 2.0 * std::f32::consts::PI;
        let r = fc / rate;
        let mut d = (1.0 - 2.0 * dmpfac) * r;
        d = d.min(2.0);
        d = (2.0 * dmpfac - d) / r;
        let e = 1.0 / (r * r);
        let fg = 1.0 / (1.0 + d + e);
        let fb0 = (d + e + e) / (1.0 + d + e);
        let fb1 = -e / (1.0 + d + e);
        if c.highpass {
            f.a0 = 1.0 - fg;
            f.hp = 1.0;
        } else {
            f.a0 = fg;
            f.hp = 0.0;
        }
        f.b0 = fb0;
        f.b1 = fb1;
        if reset {
            f.y1 = 0.0;
            f.y2 = 0.0;
        }
    }

    fn run_macro(&mut self, ci: usize, param: u8) {
        if param < 0x80 {
            return; // SFx macros: never written by the loader
        }
        match self.m.zxx[(param & 0x7F) as usize] {
            Macro::Unused => {}
            Macro::Filter { cutoff, reso, highpass } => {
                let c = &mut self.chn[ci];
                let reset = !c.v.filter.on;
                c.cutoff = cutoff;
                c.reso = reso;
                c.highpass = highpass;
                self.setup_filter(ci, reset);
            }
            Macro::FilterOff => {
                let c = &mut self.chn[ci];
                let reset = !c.v.filter.on;
                c.cutoff = 0x7F;
                c.reso = 0;
                self.setup_filter(ci, reset);
            }
            Macro::Echo { kind, delay, feedback } => self.echo.set(kind, delay, feedback),
        }
    }

    /// One channel's share of `ProcessEffects` for this tick.
    fn process_channel(&mut self, ci: usize) {
        let first = self.tick == 0;
        let row = self.chn[ci].row;
        let mut porta = row.is_tone_porta();
        let (cmd, param, volcmd, vol) = (row.cmd, row.param, row.volcmd, row.vol);
        if first {
            self.chn[ci].trigger = false;
            let mut note = row.note;
            let mut instr = row.instr;
            if instr != 0 {
                self.chn[ci].new_ins = instr;
            }
            // IT: an invalid instrument number does nothing (and its note does not play)
            if note == NOTE_NONE || note <= 128 {
                let check = if instr != 0 { instr } else { self.chn[ci].old_ins };
                if check != 0 && self.m.instruments.get(check as usize).is_none_or(|i| i.is_none()) {
                    note = NOTE_NONE;
                    instr = 0;
                }
            }
            let retrig_env = note == NOTE_NONE && instr != 0;
            if retrig_env {
                let c = &mut self.chn[ci];
                if c.v.sample != 0 {
                    c.volume = 256;
                    c.fast_ramp = true;
                }
                // IT: an instrument number alone retriggers when it is another instrument
                // (or the sample has ended)
                if (instr as usize) < self.m.instruments.len() && (c.instr != instr as usize || !Self::is_playing(c)) {
                    note = c.note;
                }
            }
            if note >= 0xFB {
                if instr != 0 {
                    self.chn[ci].volume = 256;
                }
                instr = 0;
            }
            if (1..=128).contains(&note) {
                let c = &mut self.chn[ci];
                c.new_note = note;
                if !porta {
                    self.cut_to_tail(ci);
                }
            }
            if instr != 0 {
                let old = self.chn[ci].v.sample;
                self.instrument_change(ci, instr, porta);
                self.chn[ci].new_ins = 0;
                if (1..=128).contains(&note) && old != self.chn[ci].v.sample {
                    // IT: a portamento that changes the sample restarts it
                    self.chn[ci].v.pos = 0.0;
                }
            }
            if note != NOTE_NONE {
                self.note_change(ci, note, porta, true);
                let c = &mut self.chn[ci];
                if c.trigger {
                    let reset = true;
                    let _ = c;
                    self.setup_filter(ci, reset);
                }
            }
            // the effect memories, in IT's order when both columns have a portamento
            if cmd == Cmd::TonePorta {
                self.init_tone_porta(ci, param as u16);
            }
            if volcmd == VolCmd::TonePorta {
                self.init_tone_porta(ci, IT_PORTA_VOLCMD[(vol & 0x0F) as usize]);
            }
            if vol != 0 && matches!(volcmd, VolCmd::PortaUp | VolCmd::PortaDown) {
                let c = &mut self.chn[ci];
                c.old_porta_up = vol << 2;
                c.old_porta_down = vol << 2;
                if cmd != Cmd::TonePorta {
                    c.porta_slide = (vol << 2) as u16;
                }
            }
            if param != 0 && matches!(cmd, Cmd::PortaUp | Cmd::PortaDown) {
                let c = &mut self.chn[ci];
                c.old_porta_up = param;
                c.old_porta_down = param;
                c.porta_slide = param as u16;
            }
        }
        let _ = &mut porta;
        // the volume column
        match volcmd {
            VolCmd::TonePorta => self.tone_porta(ci),
            VolCmd::PortaUp => self.porta_up(ci, vol << 2, true),
            VolCmd::PortaDown => self.porta_down(ci, vol << 2, true),
            VolCmd::Offset => {
                if first && self.chn[ci].v.sample != 0 && vol <= 9 {
                    let c = &mut self.chn[ci];
                    let cue = self.m.samples[c.v.sample].cue;
                    let off = if vol == 0 {
                        c.old_offset
                    } else {
                        // only the first cue point exists; the others are "none" too
                        let o = if vol == 1 { cue } else { NO_CUE };
                        c.old_offset = o;
                        o
                    };
                    self.sample_offset(ci, off);
                }
            }
            VolCmd::PlayControl => {
                if first {
                    let c = &mut self.chn[ci];
                    match vol {
                        0 => c.paused = true,
                        1 => c.paused = false,
                        _ => {}
                    }
                }
            }
            VolCmd::None => {}
        }
        match cmd {
            Cmd::None => {}
            Cmd::PortaUp => self.porta_up(ci, param, false),
            Cmd::PortaDown => self.porta_down(ci, param, false),
            Cmd::TonePorta => self.tone_porta(ci),
            Cmd::Vibrato => {
                let c = &mut self.chn[ci];
                if param & 0x0F != 0 {
                    c.vib_depth = (param & 0x0F) * 4;
                }
                if param & 0xF0 != 0 {
                    c.vib_speed = param >> 4;
                }
                c.vib_on = param != 0;
            }
            Cmd::Tremolo => {
                let c = &mut self.chn[ci];
                if param & 0x0F != 0 {
                    c.trem_depth = (param & 0x0F) << 2;
                }
                if param & 0xF0 != 0 {
                    c.trem_speed = param >> 4;
                }
                c.trem_on = param & 0x0F != 0;
            }
            Cmd::Speed => {
                if first && param > 0 {
                    self.speed = param as u32;
                }
            }
            Cmd::OffsetPercentage => {
                if first {
                    let len = self.chn[ci].v.length;
                    self.sample_offset(ci, (len as u64 * param as u64 / 256) as u32);
                }
            }
            Cmd::Retrig => {
                if param != 0 {
                    self.chn[ci].retrig_param = param;
                }
                let p = self.chn[ci].retrig_param;
                self.retrig(ci, p);
            }
            Cmd::ChannelVolume => {
                if first && param <= 64 {
                    let c = &mut self.chn[ci];
                    c.global_vol = param as i32;
                    c.fast_ramp = true;
                }
            }
            Cmd::ChannelVolSlide => {
                let c = &mut self.chn[ci];
                let p = if param != 0 {
                    c.old_chn_vol_slide = param;
                    param
                } else {
                    c.old_chn_vol_slide
                };
                let mut d = 0;
                if p & 0x0F == 0x0F && p & 0xF0 != 0 {
                    if first {
                        d = (p >> 4) as i32;
                    }
                } else if p & 0xF0 == 0xF0 && p & 0x0F != 0 {
                    if first {
                        d = -((p & 0x0F) as i32);
                    }
                } else if !first {
                    if p & 0x0F != 0 {
                        if p & 0xF0 == 0 {
                            d = -((p & 0x0F) as i32);
                        }
                    } else {
                        d = (p >> 4) as i32;
                    }
                }
                if d != 0 {
                    c.global_vol = (c.global_vol + d).clamp(0, 64);
                }
            }
            Cmd::Midi => {
                if first {
                    self.run_macro(ci, param);
                }
            }
            Cmd::TonePortaDuration => {
                // param 0 (all the loader writes): straight to the new note, without retrigger
                if first && (1..=128).contains(&row.note) && param == 0 {
                    let c = &mut self.chn[ci];
                    c.period = c.porta_dest;
                }
            }
        }
        if first && row.instr != 0 {
            self.chn[ci].old_ins = row.instr;
        }
    }

    /// Volumes, pitches and the mix of one tick.
    fn mix_tick(&mut self) {
        let spt = self.spt;
        self.buf.clear();
        self.buf.resize(spt * 2, 0.0);
        self.wet.clear();
        self.wet.resize(spt * 2, 0.0);
        let first = self.tick == 0;
        for ci in 0..self.chn.len() {
            let c = &mut self.chn[ci];
            // volume: tremolo, fade-out, pause
            let mut vol = c.volume;
            if c.trem_on {
                let delta = IT_SINE[c.trem_pos as usize] as i32;
                vol += delta * c.trem_depth as i32 / 32;
                c.trem_pos = c.trem_pos.wrapping_add(4 * c.trem_speed);
            }
            vol = vol.clamp(0, 256);
            if c.notefade {
                c.fade_vol = (c.fade_vol - 512).max(0);
                vol = vol * c.fade_vol / 65536;
            }
            if c.paused {
                vol = 0;
            }
            let real = vol as f32 / 256.0 * c.global_vol as f32 / 64.0 * c.ins_vol as f32 / 64.0;
            // pitch: vibrato
            let mut period = c.period;
            if c.vib_on {
                c.vib_pos = c.vib_pos.wrapping_add(4 * c.vib_speed);
                let vd = IT_SINE[c.vib_pos as usize] as i32;
                let vdelta = vd * c.vib_depth as i32 / 64;
                freq_slide(&self.t, &mut period, vdelta);
            }
            let _ = first;
            let playing = c.v.length != 0 && c.period != 0 && c.v.sample != 0;
            c.v.inc = if playing { period as f64 / self.rate as f64 } else { 0.0 };
            c.v.wet = self.m.use_dsp && !c.no_dsp;
            let target = if playing { real * self.gain } else { 0.0 };
            if target != c.v.target || (c.v.ramp == 0 && c.v.gain != target) {
                let len = if c.v.gain != 0.0 && target != 0.0 && !c.fast_ramp {
                    (spt as u32).clamp(self.ramp_up.min(self.ramp_down), 2048)
                } else if target > c.v.gain {
                    self.ramp_up
                } else {
                    self.ramp_down
                };
                c.v.target = target;
                c.v.ramp = len.max(1);
                c.v.step = (target - c.v.gain) / c.v.ramp as f32;
            }
            c.fast_ramp = false;
            let paused = c.paused;
            let (buf, wet) = (&mut self.buf, &mut self.wet);
            if !paused || c.v.gain != 0.0 {
                let out = if c.v.wet { &mut *wet } else { &mut *buf };
                mix_voice(&mut c.v, &self.m.samples, out, self.nearest, paused);
            }
            if let Some(t) = &mut c.tail {
                t.inc = t.inc.max(0.0);
                let out = if t.wet { &mut *wet } else { &mut *buf };
                mix_voice(t, &self.m.samples, out, self.nearest, false);
                if t.ramp == 0 || t.length == 0 {
                    c.tail = None;
                }
            }
        }
        self.echo.process(&mut self.wet, spt);
        for (o, w) in self.buf.iter_mut().zip(&self.wet) {
            *o += *w;
        }
        self.at = 0;
    }
}

/// Mixes `spt` frames of a voice into `out` (its side only), advancing it.
fn mix_voice(v: &mut Voice, samples: &[super::load::Sample], out: &mut [f32], nearest: bool, paused: bool) {
    let Some(s) = samples.get(v.sample) else { return };
    let side = v.right as usize;
    let frames = out.len() / 2;
    let len = (v.length as usize).min(s.data.len());
    let get = |i: usize| -> f32 {
        match &s.data {
            Data::I16(d) => d.get(i).map_or(0.0, |&x| x as f32 / 32768.0),
            Data::I8(d) => d.get(i).map_or(0.0, |&x| x as f32 / 128.0),
            Data::Empty => 0.0,
        }
    };
    for f in 0..frames {
        if v.ramp > 0 {
            v.gain += v.step;
            v.ramp -= 1;
            if v.ramp == 0 {
                v.gain = v.target;
            }
        }
        if len == 0 || v.length == 0 || paused {
            continue;
        }
        let i = v.pos as usize;
        if i >= len {
            v.length = 0;
            continue;
        }
        let mut x = if nearest {
            get(i)
        } else {
            let frac = (v.pos - i as f64) as f32;
            let j = if v.loop_on && i + 1 >= v.loop_end as usize { v.loop_start as usize } else { i + 1 };
            let b = if j < len || v.loop_on { get(j) } else { 0.0 };
            let a = get(i);
            a + (b - a) * frac
        };
        if v.filter.on {
            let fl = &mut v.filter;
            let y = fl.a0 * x + fl.b0 * fl.y1.clamp(-2.0, 2.0) + fl.b1 * fl.y2.clamp(-2.0, 2.0);
            fl.y2 = fl.y1;
            fl.y1 = y - x * fl.hp;
            x = y;
        }
        out[f * 2 + side] += x * v.gain;
        v.pos += v.inc;
        if v.loop_on {
            let (ls, le) = (v.loop_start as f64, v.loop_end as f64);
            if v.pos >= le {
                let l = le - ls;
                v.pos = ls + (v.pos - le) % l;
            }
        } else if v.pos >= len as f64 {
            v.length = 0;
        }
    }
}
