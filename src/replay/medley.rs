//! Medley (Paul van der Valk, 1990-1991 game music; Eagleplayer/UADE player "Medley", module
//! prefix `mso.`, magic `MSOB`).
//!
//! Clean-room, written from a behavioural specification (file layout, per-tick state machine,
//! register writes and a 68000 cycle ledger) and black-box runs of the reference player on
//! synthetic modules; no original code. There is no real module of the format in the archive, so
//! the synthetic ones are the only reference.
//!
//! Two small player tables are embedded (`medley_periods.bin`, the 110-entry period table, and
//! `medley_detune.bin`, the detune patterns): they are player tables, embedded on the project
//! owner's decision; nothing else is embedded. The module holds only data (songs, sequences,
//! instruments, waveforms); four voices with rows of notes and commands, an envelope, four
//! modulators, three kinds of waveform (looped, windowed sample, two-oscillator synth) and random
//! triggered voices.
//!
//! Choices where the specification is silent or says "undefined" (all safe: every read outside the
//! module gives zero, every loop is bounded, nothing can panic):
//! - a selector byte other than 0, 1, 2 counts as "flag not set";
//! - a note whose period index lies beyond the 154 values the tables hold ends the voice (silent,
//!   DMA off) instead of reading unrelated memory;
//! - a random-list count (command 4's m) of 0 does nothing (the voice stays idle / no jump);
//! - the random delay and index masks are taken as bytes;
//! - a row that does not end within 100,000 elements (an endless chain of zero durations, or a
//!   sequence running into zero bytes) ends its voice;
//! - the tempo word, the second song table, song restore and locked channels are not provided
//!   (the specification says the host never uses them);
//! - the host's volume, balance and mute are not used (the player never reads them).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Medley", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;

static PERIODS: &[u8; 220] = include_bytes!("medley_periods.bin");
static DETUNE: &[u8; 88] = include_bytes!("medley_detune.bin");

/// A row that does not end in this many elements ends its voice.
const MAX_ELEMENTS: u32 = 100_000;

fn detect(d: &[u8]) -> bool {
    d.len() >= 20 && &d[..4] == b"MSOB"
}

/// Period table entry: 0..109 the period table, 110..153 the detune table's words.
fn period_at(i: u8) -> Option<u16> {
    let i = i as usize;
    if i < 110 {
        Some(u16::from_be_bytes([PERIODS[2 * i], PERIODS[2 * i + 1]]))
    } else if i < 154 {
        let j = 2 * (i - 110);
        Some(u16::from_be_bytes([DETUNE[j], DETUNE[j + 1]]))
    } else {
        None
    }
}

fn det_b(i: usize) -> u8 {
    DETUNE.get(i).copied().unwrap_or(0)
}

/// The 64-entry volume table of value `v` (specification 2.5).
fn vol_table(v: u8) -> [u8; 64] {
    let v = (v & 63) as u32;
    let mut t = [0u8; 64];
    for i in 1..64u32 {
        t[i as usize] = if v == 63 { i as u8 } else { (1 + i * v / 63) as u8 };
    }
    t
}

#[derive(Clone, Copy, Default)]
struct Modul {
    x: u16,
    delay: u8,
    falling: bool,
}

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    note: u8,
    gate: bool,
    trig: bool,
    env: u16,
    /// Envelope phase: false = attack (the state at load), true = decay.
    decay: bool,
    dur: u8,
    pos: u32,
    track: u32,
    newrow: bool,
    speed_ctr: u8,
    inst: u32,
    octave: u8,
    wave: u32,
    base_period: u16,
    period: u16,
    level: u16,
    win: u16,
    phase_off: u16,
    step_off: u8,
    m: [Modul; 4],
    synth_valid: bool,
    last_phase: u16,
    synth_words: u16,
    synth_shift: u8,
    detune: i16,
    vtab: [u8; 64],
    transpose: u8,
    rbase: u8,
    rrange: u8,
    rctr: u8,
    ncalls: usize,
    calls: [u32; 8],
    nloops: usize,
    loops: [(u32, u8); 8],
    right: bool,
    upper: bool,
}

impl Voice {
    fn new(ch: usize) -> Self {
        Voice {
            active: false,
            note: 0,
            gate: false,
            trig: false,
            env: 0,
            decay: false,
            dur: 0,
            pos: 0,
            track: 0,
            newrow: false,
            speed_ctr: 0,
            inst: 0,
            octave: 0,
            wave: 0,
            base_period: 0,
            period: 0,
            level: 0,
            win: 0,
            phase_off: 0,
            step_off: 0,
            m: [Modul::default(); 4],
            synth_valid: false,
            last_phase: 0,
            synth_words: 0,
            synth_shift: 0,
            detune: 0,
            vtab: [0; 64],
            transpose: 0,
            rbase: 0,
            rrange: 0,
            rctr: 0,
            ncalls: 0,
            calls: [0; 8],
            nloops: 0,
            loops: [(0, 0); 8],
            right: ch == 1 || ch == 2,
            upper: ch >= 2,
        }
    }
}

pub struct Medley {
    d: Vec<u8>,
    /// Guest address of the module.
    base: u32,
    /// Guest address of the silence block (4 bytes) followed by the four synth buffers.
    mem: u32,
    song_tab: u32,
    seq_tab: u32,
    inst_tab: u32,
    wave_tab: u32,
    nsongs: u8,
    sub: u16,
    v: [Voice; 4],
    ticks: u16,
    seed: u16,
    master: [u8; 64],
    speed: u8,
    gtranspose: u8,
    det_start: usize,
    det_ptr: usize,
    det_interval: u8,
    det_ctr: u8,
    songend: bool,
    /// Clock of the tick's start and cycles since.
    t0: u64,
    cy: u64,
    /// Clock of the first tick after the current start.
    first_at: u64,
    /// Clock of the latest host-side write.
    host_t: u64,
}

impl Medley {
    fn b(&self, a: u32) -> u8 {
        self.d.get(a as usize).copied().unwrap_or(0)
    }
    fn w(&self, a: u32) -> u16 {
        ((self.b(a) as u16) << 8) | self.b(a.wrapping_add(1)) as u16
    }
    fn l(&self, a: u32) -> u32 {
        ((self.w(a) as u32) << 16) | self.w(a.wrapping_add(2)) as u32
    }
    /// Object `i` of the table at `tab`: the entry's address plus the entry's value.
    fn obj(&self, tab: u32, i: u8) -> u32 {
        let e = tab.wrapping_add(4 * i as u32);
        e.wrapping_add(self.l(e))
    }
    /// Whether the table's entry `i` is 0 (no object).
    fn entry0(&self, tab: u32, i: u8) -> bool {
        self.l(tab.wrapping_add(4 * i as u32)) == 0
    }

    fn c(&mut self, n: u64) {
        self.cy += n;
    }
    /// Places the next write at the cycle count so far.
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    fn rnd(&mut self) -> u16 {
        self.seed = self.seed.wrapping_add(self.ticks);
        self.seed >> 4
    }

    fn flag(&self, v: usize, sel: u8) -> bool {
        match sel {
            1 => self.v[v].right,
            2 => self.v[v].upper,
            _ => false,
        }
    }

    // ---------------------------------------------------------------- start

    /// The song object of the current subsong when it is valid (specification 1.6).
    fn song(&self, n: u16) -> Option<u32> {
        if n == 0 || n > self.nsongs as u16 || n > 255 || self.entry0(self.song_tab, n as u8) {
            return None;
        }
        Some(self.obj(self.song_tab, n as u8))
    }

    /// The state part of a subsong start (specification 3.2); the host's DMACON writes are made
    /// by the caller.
    fn start_state(&mut self, n: u16, hw: &mut Hw, reset_at: &mut u64) {
        self.songend = false;
        let song = self.song(n);
        for ch in 0..4usize {
            {
                let x = &mut self.v[ch];
                x.active = false;
                x.gate = false;
                x.trig = false;
                x.speed_ctr = 0;
                x.rctr = 0;
                x.transpose = 0;
                x.dur = 1;
                x.ncalls = 0;
                x.nloops = 0;
                x.detune = 0;
                x.env = 0;
                x.m = [Modul::default(); 4];
                x.vtab = vol_table(63);
            }
            let Some(s) = song else { continue };
            if ch > 0 {
                *reset_at += 5069;
            }
            hw.bus.clock = *reset_at;
            hw.dmacon(1 << ch);
            let track = self.b(s + 0x10 + ch as u32);
            if track == 0 {
                continue;
            }
            let seq = self.obj(self.seq_tab, track);
            let mut ins = self.b(s + 0x1A + ch as u32);
            if ins == 0 {
                ins = self.b(s + 0x1E);
            }
            if ins == 0 {
                ins = 1;
            }
            let inst = self.obj(self.inst_tab, ins);
            let rbase = self.b(s + 0x26 + ch as u32);
            let rrange = self.b(s + 0x2A + ch as u32);
            let vv = self.b(s + 0x2E + ch as u32);
            self.master = vol_table(self.b(s + 0x24));
            self.speed = self.b(s + 0x20);
            self.gtranspose = self.b(s + 0x1F);
            let mut p = self.b(s + 0x22);
            if p > det_b(1) {
                p = 0;
            }
            let wpos = 2 + 2 * p as usize;
            let off = i16::from_be_bytes([det_b(wpos), det_b(wpos + 1)]) as isize;
            self.det_start = (wpos as isize + off).max(0) as usize;
            self.det_ptr = self.det_start;
            self.det_interval = self.b(s + 0x23);
            for k in 0..4 {
                self.v[k].detune = det_b(self.det_start + k) as i8 as i16;
            }
            self.det_ctr = 1;
            let x = &mut self.v[ch];
            x.pos = seq.wrapping_add(0x10);
            x.track = x.pos;
            x.inst = inst;
            x.rbase = rbase;
            x.rrange = rrange;
            x.active = rbase == 0;
            x.vtab = vol_table(vv);
        }
    }

    // ----------------------------------------------------------------- tick

    fn run(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 0;
        self.c(344);
        self.ticks = self.ticks.wrapping_add(1);
        self.c(21);
        if self.det_interval != 0 {
            self.c(29);
            self.det_ctr = self.det_ctr.wrapping_sub(1);
            if self.det_ctr == 0 {
                self.det_ctr = self.det_interval;
                self.c(319);
                if det_b(self.det_ptr) == 0x80 {
                    self.det_ptr = self.det_start;
                    self.c(16);
                }
                for k in 0..4 {
                    self.v[k].detune = det_b(self.det_ptr + k) as i8 as i16;
                }
                self.det_ptr += 4;
            }
        }
        for ch in 0..4 {
            self.voice(hw, ch);
        }
    }

    fn voice(&mut self, hw: &mut Hw, ch: usize) {
        self.c(26);
        if !self.voice_body(hw, ch) {
            self.c(16);
        }
    }

    /// Returns true when a random voice ended (no return cost).
    fn voice_body(&mut self, hw: &mut Hw, ch: usize) -> bool {
        if !self.v[ch].active {
            self.c(44);
            if self.v[ch].rbase == 0 {
                return false;
            }
            self.c(29);
            if self.ticks & 3 != 0 {
                return false;
            }
            self.c(21);
            if self.v[ch].rctr == 0 {
                self.c(88 + 44);
                let r = self.rnd();
                let range = self.v[ch].rrange.wrapping_sub(1);
                self.v[ch].rctr = ((r & range as u16) as u8).wrapping_add(self.v[ch].rbase);
            }
            self.c(25);
            self.v[ch].rctr = self.v[ch].rctr.wrapping_sub(1);
            if self.v[ch].rctr != 0 {
                return false;
            }
            self.c(205);
            let t = self.v[ch].track;
            let m = self.b(t.wrapping_add(1));
            let r = self.rnd();
            if m == 0 {
                return false;
            }
            let idx = (r & m.wrapping_sub(1) as u16) as u32;
            let s = self.b(t.wrapping_add(2 + 2 * idx + 1));
            if self.entry0(self.seq_tab, s) {
                return false;
            }
            self.c(68);
            let seq = self.obj(self.seq_tab, s);
            let x = &mut self.v[ch];
            x.pos = seq.wrapping_add(0x10);
            x.dur = 1;
            x.active = true;
            return false;
        }
        self.c(85);
        self.v[ch].dur = self.v[ch].dur.wrapping_sub(1);
        if self.v[ch].dur == 0 {
            self.c(48);
            self.v[ch].newrow = true;
            self.v[ch].speed_ctr = 0;
            if self.read_row(hw, ch) {
                return true;
            }
            if !self.v[ch].active {
                return false;
            }
        }
        self.c(27);
        let sc = self.v[ch].speed_ctr.wrapping_sub(1);
        self.v[ch].speed_ctr = sc;
        if sc & 0x80 == 0 {
            return false;
        }
        self.v[ch].speed_ctr = self.speed;
        self.c(43);
        self.output(hw, ch);
        false
    }

    /// Reads a row (specification 2.3). Returns true when a random voice ended.
    fn read_row(&mut self, hw: &mut Hw, ch: usize) -> bool {
        for _ in 0..MAX_ELEMENTS {
            self.c(21);
            let pos = self.v[ch].pos;
            let n = self.b(pos);
            let a = self.b(pos.wrapping_add(1));
            let after = pos.wrapping_add(2);
            self.v[ch].pos = after;
            if n >= 0x80 {
                self.c(64);
                if self.command(hw, ch, n & 0x0F, a, after) {
                    return true;
                }
                self.c(10);
                continue;
            }
            let x = &mut self.v[ch];
            if n > 0 {
                x.note = n;
                x.gate = true;
                x.trig = true;
                self.c(63);
            } else {
                x.gate = false;
                x.trig = false;
                self.c(41);
            }
            self.c(25);
            if a == 0 {
                continue;
            }
            self.c(35);
            let x = &mut self.v[ch];
            if a & 0x80 != 0 {
                x.trig = false;
                self.c(36);
            }
            self.v[ch].dur = a & 0x7F;
            return false;
        }
        // an endless row: end the voice (safe choice)
        self.v[ch].active = false;
        false
    }

    /// One command; `after` is the position after its argument. True when a random voice ended.
    fn command(&mut self, hw: &mut Hw, ch: usize, cmd: u8, a: u8, after: u32) -> bool {
        match cmd {
            0 => {
                if self.v[ch].ncalls > 0 {
                    self.v[ch].ncalls -= 1;
                    self.v[ch].pos = self.v[ch].calls[self.v[ch].ncalls];
                    self.c(79);
                } else if self.v[ch].rbase == 0 {
                    self.c(67 + 218);
                    self.cy &= !1;
                    self.c(156);
                    self.v[ch].pos = self.v[ch].track;
                    self.songend = true;
                } else {
                    self.c(67 + 53 + 24);
                    self.at(hw);
                    hw.dmacon(1 << ch);
                    self.c(28);
                    self.v[ch].active = false;
                    return true;
                }
            }
            2 => {
                let v = (a >> 1) & 63;
                self.v[ch].vtab = vol_table(v);
                let cc = if v == 63 { 64 } else { 1 + (64 * v as u64) / 63 };
                self.c(118 + 35 * 64 + 8 * cc + 80);
            }
            3 => {
                if self.entry0(self.inst_tab, a) {
                    self.c(79);
                } else {
                    self.v[ch].inst = self.obj(self.inst_tab, a);
                    self.c(119);
                }
            }
            4 => {
                if a == 0 {
                    self.c(33);
                } else {
                    let r = self.rnd();
                    let idx = (r & a.wrapping_sub(1) as u16) as u32;
                    let s = self.b(after.wrapping_add(2 * idx + 1));
                    if self.entry0(self.seq_tab, s) {
                        self.c(192 + 16);
                    } else {
                        self.c(192 + 28);
                        self.v[ch].pos = self.seq_tab.wrapping_add(self.l(self.seq_tab.wrapping_add(4 * s as u32)));
                    }
                }
            }
            5 => {
                if self.entry0(self.seq_tab, a) {
                    self.c(79);
                } else if self.v[ch].ncalls >= 8 {
                    self.c(128);
                } else {
                    let n = self.v[ch].ncalls;
                    self.v[ch].calls[n] = after;
                    self.v[ch].ncalls += 1;
                    self.v[ch].pos = self.obj(self.seq_tab, a).wrapping_add(0x10);
                    self.c(174);
                }
            }
            6 => {
                if self.v[ch].nloops < 8 {
                    let n = self.v[ch].nloops;
                    self.v[ch].loops[n] = (after, a);
                    self.v[ch].nloops += 1;
                    self.c(109);
                } else {
                    self.c(45);
                }
            }
            7 => {
                if self.v[ch].nloops == 0 {
                    self.c(37);
                } else {
                    let n = self.v[ch].nloops - 1;
                    let cnt = self.v[ch].loops[n].1.wrapping_sub(1);
                    self.v[ch].loops[n].1 = cnt;
                    if cnt == 0 {
                        self.v[ch].nloops -= 1;
                        self.c(88);
                    } else {
                        self.v[ch].pos = self.v[ch].loops[n].0;
                        self.c(98);
                    }
                }
            }
            8 => {
                self.v[ch].transpose = a;
                self.c(28);
            }
            _ => self.c(16),
        }
        false
    }

    // --------------------------------------------------------------- output

    fn output(&mut self, hw: &mut Hw, ch: usize) {
        if self.v[ch].newrow {
            self.v[ch].newrow = false;
            if !self.note_setup(hw, ch) {
                return;
            }
        }
        self.v[ch].period = self.v[ch].base_period;
        self.envelope(ch);
        self.modulators(ch);
        let (addr, len) = self.waveform(hw, ch);
        // register writes
        self.c(49);
        self.c(28);
        self.at(hw);
        hw.loc(ch, addr);
        self.c(12);
        self.at(hw);
        hw.len(ch, len);
        self.c(36);
        self.at(hw);
        let per = self.v[ch].period.wrapping_add(self.v[ch].detune as u16);
        hw.per(ch, per);
        self.c(70);
        self.at(hw);
        let vi = (self.v[ch].level >> 10) as usize & 63;
        let vol = self.master[(self.v[ch].vtab[vi] & 63) as usize] as u16;
        hw.vol(ch, vol * 0x0101);
        self.c(24);
        self.at(hw);
        hw.dmacon(0x8000 | 1 << ch);
        self.v[ch].trig = false;
        self.c(16);
    }

    /// Specification 2.4.1. Returns false when the voice ended (index beyond the tables).
    fn note_setup(&mut self, hw: &mut Hw, ch: usize) -> bool {
        let inst = self.v[ch].inst;
        let ty = self.b(inst + 0x10);
        let x = self.v[ch]
            .note
            .wrapping_add(self.gtranspose)
            .wrapping_add(self.v[ch].transpose)
            .wrapping_add(self.b(inst + 0x13));
        self.c(89);
        if ty == 2 {
            let oct = x / 12;
            let semi = x % 12;
            self.c(8 + 17 * (oct as u64 + 1) + 212);
            let s0 = self.b(inst + 0x14);
            let wave = self.obj(self.wave_tab, s0);
            let v = &mut self.v[ch];
            v.octave = oct;
            v.base_period = period_at(12 + semi).unwrap_or(0);
            v.wave = wave;
            v.synth_valid = false;
            return true;
        }
        let k = [0x1Au8, 0x26, 0x32, 0x3E, 0x4A, 0x56, 0x62].iter().filter(|&&t| x >= t).count() as u8;
        self.v[ch].octave = k;
        let mut j = k;
        while j > 0 && self.b(inst + 0x14 + j as u32) == 0 {
            j -= 1;
        }
        if j > 0 {
            self.c(19 * (k - j + 1) as u64);
        } else {
            self.c(19 * k as u64 + 10);
        }
        let w = self.b(inst + 0x14 + j as u32);
        let wave = self.obj(self.wave_tab, w);
        self.v[ch].wave = wave;
        let o = self.b(wave + 0x14);
        let mut x = x.wrapping_sub(o.wrapping_mul(12));
        if ty == 1 {
            x = x.wrapping_sub(12);
        }
        x = x.wrapping_add(0x30);
        self.c(89 + 183 + 78 + if ty == 1 { 8 } else { 0 });
        match period_at(x) {
            Some(p) => {
                self.v[ch].base_period = p;
                true
            }
            None => {
                // undefined in the specification: end the voice, DMA off
                self.v[ch].active = false;
                self.at(hw);
                hw.dmacon(1 << ch);
                false
            }
        }
    }

    fn envelope(&mut self, ch: usize) {
        let inst = self.v[ch].inst;
        let peak = self.w(inst + 0x28);
        let sustain = self.w(inst + 0x2A);
        let attack = self.w(inst + 0x2C);
        let decay = self.w(inst + 0x2E);
        let release = self.w(inst + 0x30);
        self.c(53);
        let mut e = self.v[ch].env;
        if self.v[ch].trig {
            self.c(20);
            e = 0;
            self.v[ch].decay = false;
        }
        self.c(21);
        if self.v[ch].gate {
            if !self.v[ch].decay {
                let (s, carry) = e.overflowing_add(attack);
                if carry {
                    e = peak;
                    self.v[ch].decay = true;
                    self.c(80);
                } else if s >= peak {
                    e = peak;
                    self.v[ch].decay = true;
                    self.c(101);
                } else {
                    e = s;
                    self.c(63);
                }
            } else {
                let (s, borrow) = e.overflowing_sub(decay);
                if borrow {
                    e = sustain;
                    self.c(64);
                } else if s <= sustain {
                    e = sustain;
                    self.c(85);
                } else {
                    e = s;
                    self.c(63);
                }
            }
        } else {
            let (s, borrow) = e.overflowing_sub(release);
            if borrow {
                e = 0;
                self.c(25);
            } else {
                e = s;
                self.c(21);
            }
        }
        self.c(92);
        let v = &mut self.v[ch];
        v.env = e;
        v.level = e;
        v.win = 0;
        v.phase_off = 0;
        v.step_off = 0;
    }

    fn modulators(&mut self, ch: usize) {
        let inst = self.v[ch].inst;
        for k in 0..4usize {
            let m = inst + 0x32 + 18 * k as u32;
            let target = self.b(m);
            if target == 0 {
                self.c(19 + 30);
                continue;
            }
            self.c(48);
            let amp = self.w(m + 12);
            let rise = self.w(m + 14);
            let fall = self.w(m + 16);
            if self.v[ch].trig {
                self.c(41);
                let delay = self.b(m + 6);
                self.v[ch].m[k].delay = delay;
                if self.b(m + 2) == 0 {
                    self.c(45);
                    self.v[ch].m[k].falling = false;
                    self.v[ch].m[k].x = 0;
                    let top = self.b(m + 7);
                    if top != 0 {
                        self.c(23);
                        if self.flag(ch, top) {
                            self.c(28);
                            self.v[ch].m[k].falling = true;
                            self.v[ch].m[k].x = amp;
                        }
                    }
                    self.c(21);
                    let mut mid = false;
                    let mut test = true;
                    if target == 1 {
                        self.c(21);
                        if delay != 0 {
                            mid = true;
                            test = false;
                        }
                    }
                    if test {
                        self.c(21);
                        let sel = self.b(m + 8);
                        if sel != 0 {
                            self.c(23);
                            if self.flag(ch, sel) {
                                mid = true;
                            }
                        }
                    }
                    if mid {
                        self.c(20);
                        self.v[ch].m[k].x = amp >> 1;
                    }
                }
            }
            self.c(21);
            let mut st = self.v[ch].m[k];
            if st.delay != 0 {
                self.c(16);
                st.delay -= 1;
            } else {
                self.c(21);
                if !st.falling {
                    let (s, carry) = st.x.overflowing_add(rise);
                    if carry {
                        st.x = amp;
                        st.falling = true;
                        self.c(59);
                    } else if s >= amp {
                        st.x = amp;
                        st.falling = true;
                        self.c(80);
                    } else {
                        st.x = s;
                        self.c(42);
                    }
                } else {
                    let (s, borrow) = st.x.overflowing_sub(fall);
                    if !borrow && s != 0 {
                        st.x = s;
                        self.c(30);
                    } else {
                        self.c(if borrow { 34 } else { 21 });
                        st.x = 0;
                        self.c(21);
                        if self.b(m + 3) == 0 {
                            self.c(26);
                            st.falling = false;
                        }
                    }
                }
            }
            self.v[ch].m[k] = st;
            self.c(29);
            let mut o = st.x;
            if self.b(m + 1) != 0 {
                self.c(25);
                if st.falling {
                    self.c(12);
                    o = amp;
                } else {
                    o = 0;
                }
            }
            self.c(21);
            if self.b(m + 5) != 0 {
                self.c(16);
                o = amp.wrapping_sub(o);
            }
            let v = &mut self.v[ch];
            match target {
                1 => {
                    let d = ((amp >> 1).wrapping_sub(o) as i16) >> 8;
                    v.period = v.period.wrapping_add(d as u16);
                    self.cy += 83;
                }
                2 => {
                    if v.level < o {
                        v.level = 0;
                        self.cy += 81;
                    } else {
                        v.level -= o;
                        self.cy += 55;
                    }
                }
                3 => {
                    v.win = v.win.wrapping_add(o >> 8);
                    self.cy += 99;
                }
                4 => {
                    v.phase_off = v.phase_off.wrapping_add(o >> 8);
                    self.cy += 108;
                }
                5 => {
                    v.period = v.period.wrapping_add(o >> 5);
                    self.cy += 123;
                }
                6 => {
                    v.period = v.period.wrapping_sub(o >> 5);
                    self.cy += 132;
                }
                _ => {
                    v.step_off = v.step_off.wrapping_add((o >> 8) as u8);
                    self.cy += 138;
                }
            }
            self.c(30);
        }
    }

    /// Specification 2.8: the sample address (guest) and length in words.
    fn waveform(&mut self, hw: &mut Hw, ch: usize) -> (u32, u16) {
        let inst = self.v[ch].inst;
        let ty = self.b(inst + 0x10);
        let wave = self.v[ch].wave;
        let len = self.w(wave + 0x10);
        let wd = wave.wrapping_add(0x18);
        self.c(67);
        match ty {
            0 => {
                self.c(22);
                (self.base.wrapping_add(wd), len / 2)
            }
            1 => {
                self.c(106);
                let mut y = (self.b(inst + 0x11) as u16).wrapping_add(self.v[ch].win);
                let sh = self.b(wave + 0x14);
                let lo = if sh >= 8 { 0 } else { (y & 0xFF) >> sh };
                y = (y & 0xFF00) | lo;
                let mut a = wd;
                if y & 1 != 0 {
                    self.c(21);
                    if self.b(wave + 0x16) != 0 {
                        self.c(8);
                        a = wd.wrapping_add(len as u32);
                    }
                }
                self.c(24);
                let mask = (len / 2).wrapping_sub(1);
                a = a.wrapping_add((y & mask & !1) as u32);
                (self.base.wrapping_add(a), len / 4)
            }
            2 => {
                self.c(79);
                let phase = (self.b(inst + 0x12) as u16).wrapping_add(self.v[ch].phase_off);
                let mut regen = true;
                if phase == self.v[ch].last_phase {
                    self.c(21);
                    if self.v[ch].synth_valid {
                        regen = false;
                    }
                }
                if regen {
                    self.v[ch].last_phase = phase;
                    self.c(117);
                    let step = self.b(inst + 0x1C).wrapping_add(self.v[ch].step_off);
                    let mut s1 = (step >> 4) as u32;
                    let mut s2 = (step & 15) as u32;
                    if s1 == 0 {
                        s1 = 16;
                        self.c(4);
                    }
                    if s2 == 0 {
                        s2 = 16;
                        self.c(4);
                    }
                    let oct = self.v[ch].octave;
                    let mut size = 32usize;
                    let shift;
                    if oct < 4 {
                        shift = oct;
                    } else {
                        shift = 4;
                        let sh = (oct - 4).min(4);
                        size >>= sh;
                        s1 <<= sh;
                        s2 <<= sh;
                        self.c(63);
                        if oct > 8 {
                            self.c(4);
                        }
                    }
                    self.c(88 + 65 * size as u64 + 16);
                    let mask = len.wrapping_sub(1);
                    let mut p1 = phase;
                    let mut p2 = 0u16;
                    let buf = self.mem + 4 + 32 * ch as u32;
                    let mut out = [0u8; 32];
                    for o in out.iter_mut().take(size) {
                        let a = self.b(wd.wrapping_add((p1 & mask) as u32));
                        let b = self.b(wd.wrapping_add((p2 & mask) as u32));
                        *o = a.wrapping_add(b);
                        p1 = p1.wrapping_add(s1 as u16);
                        p2 = p2.wrapping_add(s2 as u16);
                    }
                    hw.bus.write_bytes(buf, &out[..size]);
                    let v = &mut self.v[ch];
                    v.synth_valid = true;
                    v.synth_words = (size / 2) as u16;
                    v.synth_shift = shift;
                }
                self.c(88);
                let v = &mut self.v[ch];
                v.period >>= v.synth_shift;
                (self.mem + 4 + 32 * ch as u32, self.v[ch].synth_words)
            }
            _ => {
                self.c(51);
                if self.v[ch].trig {
                    self.c(22);
                    (self.base.wrapping_add(wd), len / 2)
                } else {
                    self.c(30);
                    (self.mem, 2)
                }
            }
        }
    }
}

/// The host's pre-tick timing (all relative to the run's origin, from the reference logs).
const LOAD_AT: u64 = 2595;
const STOP_AT: u64 = 11815;
const RESET_AFTER_STOP: u64 = 7080;
const NEXT_STOP: u64 = 7477;
/// From the last reset write to the first tick call, and from there to where the engine's audio
/// starts counting (fitted to the reference's WAV renders).
const FIRST_AFTER_RESET: u64 = 76007;
/// The same after a subsong switch (the reference varies by a few hundred between starts and the
/// spacing of its reset writes by module; the first start's value is used).
const FIRST_AFTER_SWITCH: u64 = 76007;
const ORIGIN: u64 = 0;


impl Medley {
    /// The host's start of subsong `n`: four DMACON stop writes, then the state reset with its
    /// four DMACON writes. `stop` is the clock of the first stop write. Returns the last clock.
    fn start(&mut self, n: u16, hw: &mut Hw, stop: u64) -> u64 {
        for k in 0..4u64 {
            hw.bus.clock = stop + 43 * k;
            hw.dmacon(1 << k);
        }
        let mut reset = stop + RESET_AFTER_STOP;
        self.start_state(n, hw, &mut reset);
        self.sub = n;
        self.host_t = reset;
        reset
    }
}

impl Replayer for Medley {
    fn format(&self) -> &'static str {
        "Medley"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, (self.nsongs as u16).max(1), self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = n.clamp(1, (self.nsongs as u16).max(1));
        // the host steps through subsongs at once; a switch later in a run starts at the clock
        let stop = (self.host_t + NEXT_STOP).max(hw.bus.clock);
        let end = self.start(n, hw, stop);
        self.first_at = end + FIRST_AFTER_SWITCH;
        hw.bus.clock = hw.bus.clock.max((self.first_at + ORIGIN).saturating_sub(TICK));
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        Some(self.first_at)
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.run(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let mem = m.alloc(4 + 4 * 32);
    if mem == 0 {
        return None;
    }
    let mut s = Medley {
        d: d.to_vec(),
        base,
        mem,
        song_tab: 0,
        seq_tab: 0,
        inst_tab: 0,
        wave_tab: 0,
        nsongs: 0,
        sub: 1,
        v: [Voice::new(0), Voice::new(1), Voice::new(2), Voice::new(3)],
        ticks: 0,
        seed: 0,
        master: vol_table(63),
        speed: 0,
        gtranspose: 0,
        det_start: 0,
        det_ptr: 0,
        det_interval: 0,
        det_ctr: 0,
        songend: false,
        t0: 0,
        cy: 0,
        first_at: 0,
        host_t: 0,
    };
    s.song_tab = 4u32.wrapping_add(s.l(4));
    s.seq_tab = 8u32.wrapping_add(s.l(8));
    s.inst_tab = 12u32.wrapping_add(s.l(12));
    s.wave_tab = 16u32.wrapping_add(s.l(16));
    s.nsongs = s.b(s.song_tab.wrapping_sub(1));
    // the run's origin is START_PHASE before the engine's aligned start
    let t = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut hw = Hw { bus: &mut m.bus };
    for k in 0..4u64 {
        hw.bus.clock = t + LOAD_AT + 43 * k;
        hw.dmacon(1 << k);
    }
    let end = s.start(1, &mut hw, t + STOP_AT);
    s.first_at = end + FIRST_AFTER_RESET;
    // the song's audio counts from one period before the first tick (the host starts its timer
    // then); the engine clears what Paula made before that
    hw.bus.clock = (s.first_at + ORIGIN).saturating_sub(TICK).max(end);
    Some(Box::new(s))
}

#[cfg(test)]
#[path = "medley_tests.rs"]
mod tests;
