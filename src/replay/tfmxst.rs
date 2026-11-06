//! TFMX ST (Eagleplayer "TFMX ST", module prefix `mdst.`, samples in `smpl.mdst.*`).
//!
//! Clean-room, written from a behaviour specification drawn up from the register writes of the
//! 68k reference player; no original code. The Atari ST variant of Chris Huelsbeck's TFMX shares
//! the Amiga TFMX container and sequencer (the repo's `tfmx` port) but its macros drive a YM2149
//! register image: three tone/noise voices and a field for the noise and envelope periods. Once a
//! tick the image is turned into Paula writes (a pulse wave, a noise buffer or silence on three
//! channels, plus one digital sample channel), restarting the channels with the original's
//! sequence of INTREQ, DMACON, AUDxLC and AUDxLEN writes and its waits for the beam and for
//! Paula's audio interrupt flag (which poll this port's own Paula).
//!
//! The sequencer is rewritten here over the module image rather than shared with `tfmx.rs`,
//! whose code is built around the Amiga player's memory blocks; the behaviour the specification
//! calls shared (container, track steps, pattern rows) is the same.
//!
//! Choices where the specification is silent (all safe, none can panic or hang):
//! * a macro return (or pattern return) without a call ends the voice (the macro, or the track),
//!   where the original reads low memory of the host;
//! * the riff modes that read the beam position (mode bit 0, a note sum of 0) are played as the
//!   plain riff;
//! * reads outside the module image give zero, loops that the module could make endless are
//!   cut after a bounded number of steps per tick;
//! * the cost of rows and commands that the specification's fit did not cover is estimated.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "TFMX ST", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;

/// The song starts (Paula's output and DMA line grids restart there: `Paula::align_to_start`) one
/// colour clock after the clock the tick schedule is fitted to: in the reference the first write
/// falls 73 304 (module `mdst.h2`; 73 209 to 74 903 over the set-up costs) clocks after the song
/// start where the absolute-clock fit had it at 73 305.
const ORIGIN_SHIFT: u64 = 1;
/// CPU cycles from the run's origin to the first tick of a module with one track step of one
/// started track, seven stopped, a seven-step macro range (one step above $3F) and no sample.
const E0H_BASE: i64 = 460391;
/// Entry corrections (CPU cycles) of the first two ticks: the first comes early (it is entered
/// by the set-up routine, not by the timer); the second needs none. The first is the one value
/// at which the writes of all 50 modules whose set-up cost the model has right stay equal (the
/// line wait makes the first writes insensitive to a shift within a range, and the ranges of the
/// modules only meet at this value; found by scanning the shift in single cycles).
const FIRST_TICKS: [i64; 2] = [-248, 0];
/// The cycles the explicit-offset table layout takes less than the fixed one, before the two
/// per-entry terms below.
const EXPLICIT_LAYOUT: i64 = 8303;
/// The explicit layout's set-up walks the macro table from its second entry up to the first one
/// that equals the last entry, this many cycles per entry.
const MACRO_WALK: i64 = 68;
/// The explicit layout's extra cycles per 4-byte step between the end of the last macro and the
/// pattern table (the step is also scanned by its class), two cycles fewer when their number is odd
/// (measured for one to four steps: 29, 62, 91, 124).
const GAP_STEP: i64 = 31;
/// The companion sample file's load, before the path and the file name (cycles).
const SAMPLE_LOAD: i64 = 689;

const PSG: [u16; 64] = [
    2703, 2551, 2407, 2273, 2145, 2025, 1911, 1804, 1703, 1607, 1517, 1432, 1351, 1275, 1203, 1136, 1072, 1012, 955, 902, 851, 803, 758, 716,
    675, 637, 601, 568, 536, 506, 477, 451, 425, 401, 379, 358, 337, 318, 300, 284, 268, 253, 238, 225, 212, 200, 189, 179, 168, 159, 150,
    142, 134, 126, 119, 112, 106, 100, 94, 89, 84, 79, 75, 71,
];

const AMIGA: [u16; 64] = [
    1710, 1614, 1524, 1438, 1357, 1281, 1209, 1141, 1077, 1017, 960, 908, 856, 810, 764, 720, 680, 642, 606, 571, 539, 509, 480, 454, 428, 404,
    381, 360, 340, 320, 303, 286, 270, 254, 240, 227, 214, 202, 191, 180, 170, 160, 151, 143, 135, 127, 120, 113, 214, 202, 191, 180, 170, 160,
    151, 143, 135, 127, 120, 113, 214, 202, 191, 180,
];

/// Paula volume by the YM volume register (low five bits).
const VOLTAB: [u16; 32] = [
    0, 1, 2, 3, 4, 6, 8, 10, 13, 16, 20, 24, 30, 38, 48, 64, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32,
];

/// The four-bit sample file's byte conversion.
const NIB: [u8; 16] = [0x00, 0x93, 0xA7, 0xBF, 0xCF, 0xDF, 0xEF, 0xFF, 0x0F, 0x1F, 0x2F, 0x3F, 0x4F, 0x5F, 0x6F, 0x7F];

/// Per field: period registers (low, high), volume register, tone-on AND, tone-off OR, noise-on
/// AND, noise-off OR.
const FIELD: [(usize, usize, usize, u8, u8, u8, u8); 4] = [
    (0, 1, 8, 0xFE, 0x01, 0xF7, 0x08),
    (2, 3, 9, 0xFD, 0x02, 0xEF, 0x10),
    (4, 5, 10, 0xFB, 0x04, 0xDF, 0x20),
    (11, 12, 6, 0xFF, 0x00, 0xFF, 0x00),
];

/// The Paula channel of each conversion slot (YM voice A, B, C, then the digital voice).
const SLOT_CH: [usize; 4] = [0, 3, 2, 1];

fn rb(d: &[u8], a: i64) -> u8 {
    if a < 0 {
        0
    } else {
        d.get(a as usize).copied().unwrap_or(0)
    }
}

fn rw(d: &[u8], a: i64) -> u16 {
    (rb(d, a) as u16) << 8 | rb(d, a + 1) as u16
}

fn rl(d: &[u8], a: i64) -> u32 {
    (rw(d, a) as u32) << 16 | rw(d, a + 2) as u32
}

/// The table layout: (track steps, patterns, macros).
fn tables(d: &[u8]) -> (i64, i64, i64) {
    if be32(d, 0x1d0) != 0 {
        (be32(d, 0x1d0) as i64, be32(d, 0x1d4) as i64, be32(d, 0x1d8) as i64)
    } else {
        (0x800, 0x400, 0x600)
    }
}

/// Detection (spec 2): the macro steps of the scan range by command-byte class (below $40, $40
/// and above except $48, and $48 = digital sample) of a module, if it is one.
fn scan_stats(d: &[u8]) -> Option<(usize, usize, usize)> {
    if d.len() < 0x1dc || &d[..10] != b"TFMX-SONG " {
        return None;
    }
    if be32(d, 464) == 0 && be16(d, 12) != 0 {
        return None;
    }
    let (first, end) = if be32(d, 468) != 0 {
        (be32(d, be32(d, 472) as usize) as usize, be32(d, 468) as usize)
    } else {
        (be32(d, 1536) as usize, be32(d, 2044) as usize)
    };
    let ok = |x: usize| x != 0 && x & 0x8000_0000 == 0 && x & 1 == 0;
    if !ok(first) || !ok(end) || first >= end || end > d.len() {
        return None;
    }
    let (mut lo, mut hi, mut samples) = (0, 0, 0);
    for a in (first..end).step_by(4) {
        match d[a] {
            0..=63 => lo += 1,
            0x48 => samples += 1,
            _ => hi += 1,
        }
    }
    // an ST command is a macro command byte above 63 ($48 is one too)
    let big = (first..end).step_by(4).any(|a| d[a] > 63);
    big.then_some((lo, hi, samples))
}

/// The explicit layout's macro table walk: the number of entries from the second up to and
/// including the first that equals the table's last (128th) entry.
fn explicit_walk(d: &[u8]) -> i64 {
    let mt = be32(d, 472) as usize;
    let at = |i: usize| if mt + 4 * i + 4 <= d.len() { be32(d, mt + 4 * i) } else { 0 };
    let last = at(127);
    (1..128).find(|&i| at(i) == last).unwrap_or(127) as i64
}

/// The explicit layout's 4-byte steps between the end of the last macro (the step after the one the
/// last macro table entry names) and the pattern table.
fn explicit_gap(d: &[u8]) -> i64 {
    let mt = be32(d, 472) as usize;
    let last = if mt + 512 <= d.len() { be32(d, mt + 508) as i64 } else { 0 };
    ((be32(d, 468) as i64 - (last + 4)) / 4).max(0)
}

/// The sample count (macro steps of command $48) of a module, if it is one.
fn scan(d: &[u8]) -> Option<usize> {
    scan_stats(d).map(|s| s.2)
}

fn detect(d: &[u8]) -> bool {
    scan(d).is_some()
}

#[derive(Clone, Copy, Default)]
struct Track {
    /// File offset of the pattern's rows.
    addr: i64,
    pat: u8,
    tr: u8,
    pos: u16,
    wait: u8,
    cnt: u8,
    saved: Option<(i64, u16)>,
}

#[derive(Clone, Copy, Default)]
struct Field {
    stopped: bool,
    mac: i64,
    step: i32,
    wait: u16,
    cnt: u16,
    saved: Option<(i64, i32)>,
    skip: bool,
    key: bool,
    first: bool,
    note: u8,
    prev: u8,
    detune: i16,
    basevol: u8,
    vol: u8,
    envbit: u8,
    base: u16,
    gper: u16,
    gtime: u16,
    gspeed: u16,
    gcnt: u16,
    vsize: u8,
    vhalf: u8,
    vrate: i8,
    voff: i16,
    elen: u8,
    espeed: u8,
    ecnt: u8,
    etarget: u8,
    riff: bool,
    rmac: u8,
    rspeed: u8,
    rmode: u8,
    rcnt: u8,
    rpos: u16,
    raddr: i64,
    rstarted: bool,
    trig: bool,
    sample: bool,
}

#[derive(Clone, Copy, Default)]
struct Fade {
    vol: u8,
    target: u8,
    c1: u16,
    c2: u16,
    step: u8,
    active: bool,
}

/// How a fade start went (it decides the code path's cost).
#[derive(Clone, Copy, PartialEq)]
enum FadeStart {
    Busy,
    Immediate,
    Equal,
    Up,
    Down,
}

#[derive(Clone, Copy, Default)]
struct Digital {
    state: u8,
    rate: u16,
    offset: u32,
}

pub struct TfmxSt {
    /// The module image, followed by the empty pattern.
    mem: Vec<u8>,
    empty: i64,
    steps_t: i64,
    pat_t: i64,
    mac_t: i64,
    songs: Vec<u8>,
    sub: u16,
    playing: bool,
    song: u8,
    cur: u16,
    first: u16,
    last: u16,
    speed: i16,
    counter: i16,
    loopc: i16,
    tracks: [Track; 8],
    mute: [bool; 8],
    fl: [Field; 4],
    regs: [u8; 16],
    mixer: u8,
    fade: Fade,
    dig: Digital,
    songend: bool,
    /// CPU cycles spent so far in this tick.
    cy: i64,
    /// `cy` at the last call to the host in this tick (0 at the tick's start).
    trap_cy: i64,
    /// Ticks run so far (the first two are timed off the regular spacing).
    ticks_run: u32,
    /// The colour clock the last tick started at.
    tick_t0: u64,
    /// Half colour clock of the tick phase (the start clock is a whole number of cycles).
    half: u64,
    noise: u32,
    pulse: u32,
    silent: u32,
    smpl: u32,
    conv: Conv,
}

/// The conversion's channel bookkeeping.
#[derive(Clone, Default)]
struct Conv {
    state: [u8; 3],
    running: [bool; 4],
}

#[cfg(test)]
fn song_list(d: &[u8]) -> Vec<u8> {
    song_list_cost(d).0
}

/// The subsong list and the CPU cycles its building takes beyond skipping an unused entry (spec 4).
/// The costs are measured: taking a song 44; a song equal to the next one 142 the first time,
/// 106 after; the skips by an unused first step or a zero last step cost nothing extra.
fn song_list_cost(d: &[u8]) -> (Vec<u8>, i64) {
    let f = |k: usize| be16(d, 256 + 2 * k);
    let l = |k: usize| be16(d, 320 + 2 * k);
    let mut v = vec![0u8];
    let mut cost = 0i64;
    let mut flag = false;
    for k in 1..32usize {
        if l(k) == 0 && flag {
            continue;
        }
        if f(k) == 0x1FF {
            continue;
        }
        if k == 31 && l(31) == l(30) {
            continue;
        }
        if f(k) == f(k + 1) && l(k) == l(k + 1) {
            cost += if flag { 106 } else { 142 };
            if !flag {
                flag = true;
                if l(k) == l(k - 1) {
                    continue;
                }
            }
            if l(k) == f(k) {
                continue;
            }
            if k <= 30 {
                continue;
            }
            if l(k) == l(k + 2) {
                continue;
            }
            cost += 44;
            v.push(k as u8);
            continue;
        }
        cost += 44;
        v.push(k as u8);
    }
    (v, cost)
}

impl TfmxSt {
    fn rb(&self, a: i64) -> u8 {
        rb(&self.mem, a)
    }
    fn rw(&self, a: i64) -> u16 {
        rw(&self.mem, a)
    }
    fn rl(&self, a: i64) -> u32 {
        rl(&self.mem, a)
    }

    fn pattern_addr(&self, i: u8) -> i64 {
        self.rl(self.pat_t + 4 * i as i64) as i64
    }

    fn macro_addr(&self, i: u8) -> i64 {
        self.rl(self.mac_t + 4 * i as i64) as i64
    }

    /// The song-end signal is a call to the host, which takes the elapsed cycles down to a whole
    /// colour clock: a cycle is lost when the count at the call is odd. `odd` is the parity of
    /// the cycles between the call and the cost counted so far (measured per call site).
    fn signal_end(&mut self, odd: i64) {
        self.songend = true;
        if (self.cy - self.trap_cy + odd) & 1 != 0 {
            self.cy -= 1;
        }
        self.cy += 164;
        self.trap_cy = self.cy;
    }

    // ------------------------------------------------------------------ YM image

    fn set_mixer(&mut self, v: u8) {
        self.mixer = v;
        self.regs[7] = v;
    }

    fn tone(&mut self, f: usize, on: bool) {
        let m = FIELD[f];
        let v = if on { self.mixer & m.3 } else { self.mixer | m.4 };
        self.set_mixer(v);
    }

    fn noise_on(&mut self, f: usize, on: bool) {
        let m = FIELD[f];
        let v = if on { self.mixer & m.5 } else { self.mixer | m.6 };
        self.set_mixer(v);
    }

    fn put_period(&mut self, f: usize, p: u16) {
        let m = FIELD[f];
        self.regs[m.0] = p as u8;
        self.regs[m.1] = (p >> 8) as u8;
    }

    // ------------------------------------------------------------------ song start

    fn song_start(&mut self, s: u8) {
        self.playing = false;
        self.mixer = 0xFF;
        for f in &mut self.fl {
            f.stopped = true;
            f.skip = false;
            f.riff = false;
            f.sample = false;
        }
        let k = (s & 31) as usize;
        self.song = s;
        self.cur = self.rw(256 + 2 * k as i64);
        self.first = self.cur;
        self.last = self.rw(320 + 2 * k as i64);
        self.speed = self.rw(384 + 2 * k as i64) as i16;
        for t in &mut self.tracks {
            *t = Track { addr: self.empty, pat: 0xFF, ..Track::default() };
        }
        if s & 0x80 == 0 {
            self.read_step();
        }
        self.counter = 0;
        self.loopc = 0xFF00u16 as i16;
        self.playing = true;
    }

    /// Spec 6.4.
    fn fade_start(&mut self, speed: u8, target: u8) -> FadeStart {
        let fd = &mut self.fade;
        if fd.active {
            return FadeStart::Busy;
        }
        fd.active = true;
        fd.target = target;
        fd.c1 = speed as u16;
        fd.c2 = speed as u16;
        if speed == 0 {
            fd.vol = target;
            fd.step = 0;
            fd.active = false;
            return FadeStart::Immediate;
        }
        if fd.vol < target {
            fd.step = 1;
            FadeStart::Up
        } else if fd.vol > target {
            fd.step = 0xFF;
            FadeStart::Down
        } else {
            fd.step = 0;
            fd.active = false;
            FadeStart::Equal
        }
    }

    /// Spec 6.1.
    fn read_step(&mut self) {
        for _ in 0..512 {
            let a = self.steps_t + ((self.cur as u32 * 16) & 0xFFFF) as i64;
            if self.rw(a) == 0xEFFE {
                let c = self.rw(a + 2);
                let c = if c >= 5 { 0 } else { c };
                match c {
                    0 => {
                        self.playing = false;
                        self.signal_end(0);
                        self.cy += 564;
                        return;
                    }
                    1 => {
                        let l = self.loopc;
                        if l == 0 {
                            self.loopc = -1;
                            self.cur = self.cur.wrapping_add(1);
                            self.cy += 241;
                        } else if l < 0 {
                            let n = self.rw(a + 6) as i16;
                            if n <= 0 {
                                self.signal_end(1);
                                self.cy -= 14;
                            }
                            self.loopc = n.wrapping_sub(1);
                            self.cur = self.rw(a + 4);
                            self.cy += 241;
                        } else {
                            self.loopc = l - 1;
                            self.cur = self.rw(a + 4);
                            self.cy += 230;
                        }
                    }
                    2 => {
                        let v = self.rw(a + 4) as i16;
                        self.speed = v;
                        self.counter = v;
                        self.cur = self.cur.wrapping_add(1);
                        self.cy += 206;
                    }
                    _ => {
                        self.cur = self.cur.wrapping_add(1);
                        let r = self.fade_start(self.rw(a + 4) as u8, self.rw(a + 6) as u8);
                        if c == 3 {
                            self.cy += 112;
                        }
                        self.cy += match r {
                            FadeStart::Busy => 194,
                            FadeStart::Immediate => 342,
                            FadeStart::Equal => 371,
                            FadeStart::Up => 340,
                            FadeStart::Down => 366,
                        };
                    }
                }
                continue;
            }
            for t in 0..8 {
                let w = self.rw(a + 2 * t as i64);
                let addr = if w < 0x8000 { Some(self.pattern_addr((w >> 8) as u8)) } else { None };
                let tk = &mut self.tracks[t];
                tk.pat = (w >> 8) as u8;
                tk.tr = w as u8;
                if let Some(ad) = addr {
                    tk.addr = ad;
                    tk.pos = 0;
                    tk.wait = 0;
                    tk.cnt = 0;
                }
                if t > 0 {
                    self.cy += if addr.is_some() { 225 } else { 121 };
                }
            }
            return;
        }
        // an endless run of command steps: stop the song
        self.playing = false;
        self.signal_end(0);
    }

    // ------------------------------------------------------------------ sequencer

    fn sequencer(&mut self) {
        self.counter = self.counter.wrapping_sub(1);
        if self.counter >= 0 {
            return;
        }
        self.counter = self.speed;
        self.cy += 307;
        for t in 0..8 {
            self.mute[t] = self.rw(448 + 2 * t as i64) != 0;
        }
        for _ in 0..64 {
            let mut again = false;
            for t in 0..8 {
                if t > 0 {
                    self.cy += 113;
                }
                if self.track(t) {
                    // the restart from the last track costs two cycles more (measured)
                    if t == 7 {
                        self.cy += 2;
                    }
                    again = true;
                    break;
                }
            }
            if !again {
                break;
            }
        }
    }

    /// Channel off for transpose byte `x` (spec 6.3).
    fn channel_off(&mut self, x: u8) {
        let f = (x & 3) as usize;
        if x & 15 >= 6 {
            self.tone(f, false);
            self.noise_on(f, false);
            self.fl[f].envbit = 0;
        }
        self.fl[f].stopped = true;
        self.fl[f].riff = false;
        self.cy += 166;
    }

    /// One track's turn; true if a new track step was read (the pass starts again).
    fn track(&mut self, t: usize) -> bool {
        let pat = self.tracks[t].pat;
        if pat >= 0x90 {
            if pat == 0xFE {
                self.tracks[t].pat = 0xFF;
                if !self.mute[t] {
                    let x = self.tracks[t].tr;
                    self.channel_off(x);
                }
            }
            return false;
        }
        if self.tracks[t].wait != 0 {
            self.tracks[t].wait -= 1;
            self.cy += 36;
            return false;
        }
        for _ in 0..1024 {
            let tk = self.tracks[t];
            let a = tk.addr + (tk.pos as i16 as i64) * 4;
            let (b0, b1, b2, b3) = (self.rb(a), self.rb(a + 1), self.rb(a + 2), self.rb(a + 3));
            let w = (b2 as u16) << 8 | b3 as u16;
            let muted = self.mute[t];
            if b0 < 0xF0 {
                let wait_note = (0x7F..0xC0).contains(&b0);
                let mut b3n = b3;
                if wait_note {
                    self.tracks[t].wait = b3;
                    b3n = 0;
                }
                let mut n = b0.wrapping_add(tk.tr);
                if b0 < 0xC0 {
                    n &= 0x3F;
                }
                self.cy += if wait_note {
                    731
                } else if n >= 0xBF {
                    633
                } else {
                    675
                };
                if muted {
                    self.cy -= 392;
                } else {
                    self.dispatch(n, b1, b2, b3n);
                }
                self.tracks[t].pos = tk.pos.wrapping_add(1);
                if wait_note {
                    return false;
                }
                continue;
            }
            let next = tk.pos.wrapping_add(1);
            match b0 & 15 {
                0 => {
                    self.tracks[t].pat = 0xFF;
                    self.cy += 3;
                    if self.cur == self.last {
                        self.cur = self.first;
                        self.signal_end(0);
                    } else {
                        self.cur = self.cur.wrapping_add(1);
                    }
                    self.read_step();
                    return true;
                }
                1 => {
                    let k = &mut self.tracks[t];
                    if k.cnt == 0 {
                        k.cnt = b1;
                        k.pos = w;
                        self.cy += 206;
                    } else {
                        k.cnt -= 1;
                        if k.cnt == 0 {
                            k.pos = next;
                            self.cy += 209;
                        } else {
                            k.pos = w;
                            self.cy += 213;
                        }
                    }
                }
                2 => {
                    let ad = self.pattern_addr(b1 & 0x7F);
                    let k = &mut self.tracks[t];
                    k.pat = b1;
                    k.addr = ad;
                    k.pos = w;
                    self.cy += 213;
                }
                3 => {
                    let k = &mut self.tracks[t];
                    k.wait = b1;
                    k.pos = next;
                    self.cy += 191;
                    return false;
                }
                4 | 14 => {
                    self.tracks[t].pat = 0xFF;
                    self.cy += 171;
                    return false;
                }
                c @ (5 | 6 | 7 | 12) => {
                    let b1n = if c == 5 { b1.wrapping_add(tk.tr) } else { b1 };
                    self.cy += if c == 5 { 628 } else { 600 };
                    if muted {
                        self.cy -= 392;
                    } else {
                        self.dispatch(b0, b1n, b2, b3);
                    }
                    self.tracks[t].pos = next;
                }
                8 => {
                    let ad = self.pattern_addr(b1 & 0x7F);
                    let k = &mut self.tracks[t];
                    k.saved = Some((k.addr, k.pos));
                    k.pat = b1;
                    k.addr = ad;
                    k.pos = w;
                    self.cy += 303;
                }
                9 => {
                    // the original restores track 0's saved place into track 0 whichever track
                    // runs the return, then steps the running track
                    match self.tracks[0].saved {
                        Some((ad, ps)) => {
                            self.tracks[0].addr = ad;
                            self.tracks[0].pos = ps;
                            let k = &mut self.tracks[t];
                            k.pos = k.pos.wrapping_add(1);
                            self.cy += 219;
                        }
                        None => {
                            // no call: the original reads low memory; end the track instead
                            self.tracks[t].pat = 0xFF;
                            return false;
                        }
                    }
                }
                10 => {
                    let r = self.fade_start(b1, b3);
                    self.cy += match r {
                        FadeStart::Busy => 192,
                        FadeStart::Immediate => 339,
                        FadeStart::Equal => 368,
                        FadeStart::Up => 337,
                        FadeStart::Down => 363,
                    };
                    self.tracks[t].pos = next;
                }
                11 => {
                    let ad = self.pattern_addr(b1 & 0x7F);
                    let k = &mut self.tracks[(b2 & 7) as usize];
                    k.addr = ad;
                    k.pat = b1;
                    k.tr = b3;
                    k.pos = 0;
                    k.wait = 0;
                    k.cnt = 0;
                    let p = self.tracks[t].pos;
                    self.tracks[t].pos = p.wrapping_add(1);
                    self.cy += 367;
                }
                c => {
                    self.tracks[t].pos = next;
                    self.cy += if c == 13 { 225 } else { 151 };
                }
            }
        }
        // a pattern that never ends a tick's work: stop the track
        self.tracks[t].pat = 0xFF;
        false
    }
}

impl TfmxSt {
    // ------------------------------------------------------------------ note dispatch (8.1)

    fn dispatch(&mut self, b0: u8, b1: u8, b2: u8, b3: u8) {
        let f = (b2 & 3) as usize;
        match b0 {
            0xF7 => {
                let fl = &mut self.fl[f];
                fl.espeed = b1;
                fl.elen = (b2 >> 4).wrapping_add(1);
                fl.ecnt = fl.elen;
                fl.etarget = b3;
            }
            0xF6 => {
                let fl = &mut self.fl[f];
                fl.vsize = b1 & 0xFE;
                fl.vhalf = fl.vsize >> 1;
                fl.vrate = b3 as i8;
                fl.voff = 0;
                self.cy += 27;
            }
            0xF5 => {
                self.fl[f].key = false;
                self.cy -= 44;
            }
            b if b >= 0xBF => {
                let fl = &mut self.fl[f];
                fl.gspeed = b1 as u16;
                fl.gcnt = 1;
                self.cy += 116;
                if fl.gtime == 0 {
                    fl.gper = fl.base;
                    self.cy += 20;
                }
                fl.gtime = b3 as u16;
                fl.note = b & 0x3F;
                fl.base = AMIGA[(b & 0x3F) as usize];
            }
            _ => {
                let newmac = self.macro_addr(b1);
                let fl = &mut self.fl[f];
                fl.detune = b3 as i8 as i16;
                fl.basevol = b2 >> 4;
                fl.prev = fl.note;
                fl.note = b0;
                self.cy += 206;
                if newmac != fl.mac {
                    fl.skip = false;
                    self.cy += 16;
                }
                fl.mac = newmac;
                fl.step = 0;
                fl.wait = 0;
                fl.first = true;
                fl.cnt = 0;
                fl.stopped = false;
                fl.key = true;
            }
        }
    }

    // ------------------------------------------------------------------ fields (8)

    /// Spec 8.2: the period of note `n` with the extra detune `extra`, as the base period of field
    /// `f` and, unless a glide runs, in the registers of field `to` (cost `wcost` for the write).
    fn note_period(&mut self, f: usize, n: u8, extra: u16, to: usize, wcost: i64) {
        let fl = &mut self.fl[f];
        let mut p = PSG[(n & 0x3F) as usize] as u32;
        let d = (fl.detune as u16).wrapping_add(extra);
        if d != 0 {
            p = (p * d.wrapping_add(256) as u32) >> 8;
            self.cy += 86;
        }
        let p = p as u16;
        fl.base = p;
        if fl.gtime == 0 {
            self.put_period(to, p);
            self.cy += wcost;
        }
    }

    fn field(&mut self, f: usize) {
        if !self.fl[f].stopped {
            if self.fl[f].wait != 0 {
                self.fl[f].wait -= 1;
                self.cy += 47;
            } else {
                self.cy += 178;
                self.run_macro(f);
            }
        }
        if self.fl[f].first {
            self.fl[f].first = false;
            self.cy -= 62;
        } else {
            self.vibrato(f);
            self.glide(f);
            self.envelope(f);
            self.riff_part(f);
        }
        self.fader(f);
    }

    fn run_macro(&mut self, f: usize) {
        for _ in 0..4096 {
            let a = self.fl[f].mac + self.fl[f].step as i64 * 4;
            let (c, b1, b2, b3) = (self.rb(a), self.rb(a + 1), self.rb(a + 2), self.rb(a + 3));
            let w = (b2 as u16) << 8 | b3 as u16;
            let wi = w as i16 as i32;
            let fl = self.fl[f];
            let next = (fl.step + 1) as i16 as i32;
            let mut end = false;
            match c {
                0x04 => {
                    self.cy += 79;
                    if b1 & 1 != 0 {
                        if fl.trig {
                            return;
                        }
                        self.fl[f].trig = true;
                        self.fl[f].step = next;
                    } else {
                        self.fl[f].wait = w;
                        self.fl[f].step = next;
                        end = true;
                    }
                }
                0x05 | 0x10 => {
                    if c == 0x10 && !fl.key {
                        self.cy += 204;
                        self.fl[f].step = next;
                        continue;
                    }
                    let (s, j, x) = if c == 0x05 { (226, 233, 229) } else { (249, 256, 252) };
                    let m = &mut self.fl[f];
                    if m.cnt == 0 {
                        m.cnt = b1 as u16;
                        m.step = wi;
                        self.cy += s;
                    } else {
                        m.cnt -= 1;
                        if m.cnt == 0 {
                            m.step = next;
                            self.cy += x;
                        } else {
                            m.step = wi;
                            self.cy += j;
                        }
                    }
                }
                0x06 | 0x15 => {
                    let ad = self.macro_addr(b1 & 0x7F);
                    let m = &mut self.fl[f];
                    if c == 0x15 {
                        m.saved = Some((m.mac, m.step));
                    }
                    m.mac = ad;
                    m.step = wi;
                    m.cnt = 0;
                    m.skip = false;
                    self.cy += if c == 0x06 { 323 } else { 371 };
                }
                0x16 => {
                    self.cy += 239;
                    match fl.saved {
                        Some((ad, s)) => {
                            self.fl[f].mac = ad;
                            self.fl[f].step = (s + 1) as i16 as i32;
                        }
                        None => {
                            // a return without a call: the original reads low memory; end the voice
                            self.fl[f].stopped = true;
                            return;
                        }
                    }
                }
                0x07 => {
                    self.cy += 24;
                    self.fl[f].stopped = true;
                    return;
                }
                0x08 | 0x09 | 0x1F => {
                    let n = match c {
                        0x08 => b1.wrapping_add(fl.note),
                        0x09 => b1,
                        _ => b1.wrapping_add(fl.prev),
                    };
                    self.cy += match c {
                        0x08 => 195,
                        0x09 => 197,
                        _ => 205,
                    };
                    self.note_period(f, n, w, f, 87);
                    self.fl[f].step = next;
                    end = true;
                }
                0x0D | 0x0E => {
                    let fe = b2 == 0xFE;
                    self.cy += match (c, fe) {
                        (0x0D, false) => 258,
                        (0x0D, true) => 479,
                        (_, false) => 226,
                        _ => 447,
                    };
                    if fe {
                        self.note_period(f, b1.wrapping_add(fl.note), 0, f, 87);
                    }
                    let m = &mut self.fl[f];
                    m.vol = if c == 0x0D { m.basevol.wrapping_mul(3).wrapping_add(b3) } else { b3 };
                    m.step = next;
                }
                0x0A | 0x40 => {
                    let m = &mut self.fl[f];
                    if c == 0x0A {
                        m.riff = false;
                    }
                    m.elen = 0;
                    m.vsize = 0;
                    m.gtime = 0;
                    m.envbit = 0;
                    m.step = next;
                    self.cy += if c == 0x0A { 271 } else { 255 };
                }
                0x0B => {
                    let m = &mut self.fl[f];
                    m.gspeed = b1 as u16;
                    m.gcnt = 1;
                    if m.gtime == 0 {
                        m.gper = m.base;
                        self.cy += 288;
                    } else {
                        self.cy += 268;
                    }
                    m.gtime = w;
                    m.step = next;
                }
                0x0C => {
                    let m = &mut self.fl[f];
                    m.vsize = b1;
                    m.vhalf = b1 >> 1;
                    m.vrate = b3 as i8;
                    if m.gtime == 0 {
                        m.voff = 0;
                        self.cy += 300;
                    } else {
                        self.cy += 284;
                    }
                    m.step = next;
                }
                0x0F => {
                    let m = &mut self.fl[f];
                    m.elen = b2;
                    m.espeed = b1;
                    m.ecnt = b2;
                    m.etarget = b3;
                    m.step = next;
                    self.cy += 261;
                }
                0x14 => {
                    let m = &mut self.fl[f];
                    if !m.key {
                        m.step = next;
                        self.cy += 204;
                    } else if m.cnt == 0 {
                        m.cnt = b3 as u16;
                        self.cy += 72;
                        return;
                    } else {
                        m.cnt -= 1;
                        if m.cnt == 0 {
                            m.step = next;
                            self.cy += 252;
                        } else {
                            self.cy += 79;
                            return;
                        }
                    }
                }
                0x17 => {
                    self.fl[f].base = w;
                    self.fl[f].step = next;
                    self.cy += 211;
                }
                0x1B => {
                    let m = &mut self.fl[f];
                    m.rmac = b1;
                    m.rspeed = b2;
                    m.rmode = b3;
                    m.rcnt = 1;
                    m.riff = true;
                    m.rstarted = false;
                    self.cy += 670;
                    self.riff_part(f);
                    self.fader(f);
                    self.fl[f].trig = true;
                    self.fl[f].step = next;
                }
                0x1C | 0x1D => {
                    let v = if c == 0x1C { fl.note } else { fl.vol };
                    if b1 >= v {
                        self.fl[f].step = next;
                        self.cy += 216;
                    } else {
                        self.fl[f].step = wi;
                        self.cy += 220;
                    }
                }
                0x1E => {
                    self.fl[f].step = next;
                    self.cy += 211;
                }
                0x21 => {
                    self.cy += 661;
                    self.dispatch(fl.note, b1, b2 | (fl.basevol << 4), b3);
                    // "next" counts from where the dispatch left this field's step
                    self.fl[f].step = (self.fl[f].step + 1) as i16 as i32;
                }
                0x31 => {
                    self.cy += 615;
                    self.dispatch(0xF5, b1, b2, b3);
                    self.fl[f].step = next;
                }
                0x32 | 0x33 => {
                    let at = a + 2 * b1 as i64;
                    if at >= 0 && (at as usize) + 2 <= self.mem.len() {
                        let o = at as usize;
                        let v = u16::from_be_bytes([self.mem[o], self.mem[o + 1]]);
                        let n = if c == 0x32 { v.wrapping_add(w) } else { v & w };
                        self.mem[o..o + 2].copy_from_slice(&n.to_be_bytes());
                    }
                    self.fl[f].step = next;
                    self.cy += 237;
                }
                0x30 => {
                    if fl.skip {
                        self.fl[f].step = wi;
                        self.cy += 206;
                    } else {
                        self.fl[f].skip = true;
                        self.fl[f].step = next;
                        self.cy += 228;
                    }
                }
                0x41 => {
                    self.tone(f, b1 != 0);
                    self.noise_on(f, b3 != 0);
                    self.fl[f].step = next;
                    self.cy += 329 + if b1 != 0 { 10 } else { 0 };
                }
                0x42 => {
                    self.regs[11] = b3;
                    self.regs[12] = b2;
                    self.regs[13] = b1;
                    self.fl[f].envbit = 0x10;
                    self.fl[f].step = next;
                    self.cy += 267;
                }
                0x43 => {
                    self.cy += 329;
                    self.note_period(f, b1.wrapping_add(fl.note), 0, 3, 46);
                    self.fl[f].step = next;
                }
                0x44 => {
                    self.fl[3].vol = b1;
                    self.fl[f].step = next;
                    self.cy += 219;
                }
                0x45 => {
                    self.put_period(f, w);
                    self.fl[f].base = w;
                    self.fl[f].step = next;
                    self.cy += 283;
                }
                0x47 => {
                    self.set_mixer((self.mixer & b1) | b3);
                    self.fl[f].step = next;
                    self.cy += 251;
                }
                0x48 => {
                    self.tone(f, true);
                    self.noise_on(f, false);
                    self.put_period(f, 0);
                    self.fl[f].base = 0;
                    self.dig.offset = self.rl(a) & 0xF_FFFF;
                    self.fl[f].sample = true;
                    self.dig.state = 1;
                    self.fl[f].step = next;
                    self.cy += 473;
                }
                0x49 => {
                    self.dig.rate = w;
                    self.fl[f].step = next;
                    self.cy += 215;
                }
                0x4A => {
                    self.fl[f].sample = false;
                    self.dig.state = 0;
                    self.fl[f].step = next;
                    self.cy += 227;
                }
                0x20 => {
                    self.fl[f].step = next;
                    self.cy += 245;
                }
                c if c < 0x50 => {
                    self.fl[f].step = next;
                    self.cy += 181;
                }
                _ => {
                    self.fl[f].step = next;
                    end = true;
                }
            }
            if end {
                return;
            }
        }
        // a macro that never waits: stop it
        self.fl[f].stopped = true;
    }

    fn vibrato(&mut self, f: usize) {
        let fl = &mut self.fl[f];
        if fl.vsize == 0 {
            return;
        }
        self.cy += 111;
        fl.voff = fl.voff.wrapping_add(fl.vrate as i16);
        let mut p = fl.base as u32;
        if fl.voff != 0 {
            self.cy += 100;
            p = (p * (fl.voff as i32 + 2048) as u16 as u32) >> 11;
        }
        let glide = fl.gtime != 0;
        fl.vhalf = fl.vhalf.wrapping_sub(1);
        if fl.vhalf == 0 {
            fl.vhalf = fl.vsize;
            fl.vrate = fl.vrate.wrapping_neg();
            self.cy += 56;
        }
        if !glide {
            self.put_period(f, p as u16);
            self.cy += 87;
        }
    }

    fn glide(&mut self, f: usize) {
        let fl = &mut self.fl[f];
        if fl.gtime == 0 {
            return;
        }
        self.cy += 25;
        fl.gcnt = fl.gcnt.wrapping_sub(1);
        if fl.gcnt != 0 {
            return;
        }
        fl.gcnt = fl.gspeed;
        let (mut g, b) = (fl.gper, fl.base);
        let mut ended = false;
        if g == b {
            ended = true;
            self.cy += 206;
        } else if g < b {
            g = ((g as u32 * (256 + fl.gtime as u32)) >> 8) as u16;
            if g >= b {
                ended = true;
                self.cy += if g == b { 326 } else { 335 };
            } else {
                self.cy += 317;
            }
        } else {
            g = ((g as u32 * 256u16.wrapping_sub(fl.gtime) as u32) >> 8) as u16;
            if g <= b {
                ended = true;
                self.cy += if g == b { 326 } else { 335 };
            } else {
                self.cy += 307;
            }
        }
        if ended {
            fl.gtime = 0;
            g = b;
        }
        g &= 0x7FF;
        fl.gper = g;
        self.put_period(f, g);
    }

    fn envelope(&mut self, f: usize) {
        let fl = &mut self.fl[f];
        if fl.elen == 0 {
            return;
        }
        if fl.ecnt != 0 {
            fl.ecnt -= 1;
            self.cy += 47;
            return;
        }
        fl.ecnt = fl.elen;
        let (v, t) = (fl.vol as i8, fl.etarget as i8);
        if v < t {
            let n = v.wrapping_add(fl.espeed as i8);
            if n >= t {
                fl.vol = fl.etarget;
                fl.elen = 0;
                self.cy += 169;
            } else {
                fl.vol = n as u8;
                self.cy += 123;
            }
        } else {
            let n = v.wrapping_sub(fl.espeed as i8);
            if n < 0 {
                fl.vol = fl.etarget;
                fl.elen = 0;
                self.cy += 157;
            } else if n <= t {
                fl.vol = fl.etarget;
                fl.elen = 0;
                self.cy += 178;
            } else {
                fl.vol = n as u8;
                self.cy += 142;
            }
        }
    }

    /// Spec 8.4. The beam-driven random variants are not modelled.
    fn riff_part(&mut self, f: usize) {
        if !self.fl[f].riff {
            return;
        }
        if !self.fl[f].rstarted {
            let ad = self.macro_addr(self.fl[f].rmac & 0x7F);
            let fl = &mut self.fl[f];
            fl.raddr = ad;
            fl.rpos = 0;
            fl.rstarted = true;
        }
        let fl = &mut self.fl[f];
        fl.rcnt = fl.rcnt.wrapping_sub(1);
        if fl.rcnt != 0 {
            if fl.rmode & 2 != 0 {
                self.cy += 168;
                if fl.rcnt as u32 == ((fl.rspeed as u32 * 3) >> 3) & 255 {
                    fl.vol = ((fl.vol as u32 * 5) >> 3) as u8;
                    self.cy += 205;
                }
            } else {
                self.cy += 61;
            }
            return;
        }
        fl.rcnt = fl.rspeed;
        self.cy += 395;
        let (raddr, rpos) = (fl.raddr, fl.rpos);
        let mut by = rb(&self.mem, raddr + rpos as i64);
        let fl = &mut self.fl[f];
        if by == 0 {
            if rpos == 0 {
                return;
            }
            fl.rpos = 0;
            self.cy += 96;
            by = rb(&self.mem, raddr);
            if by == 0 {
                return;
            }
        }
        let n = by.wrapping_add(fl.note) & 0x3F;
        let mut p = AMIGA[n as usize] as u32;
        if fl.detune != 0 {
            p = (p * (fl.detune as u16).wrapping_add(256) as u32) >> 8;
            self.cy += 86;
        }
        fl.base = p as u16;
        if fl.gtime != 0 {
            self.cy -= 138;
            return;
        }
        let base = fl.base;
        if by & 0x80 != 0 {
            fl.trig = false;
            self.cy += 16;
        }
        fl.rpos = fl.rpos.wrapping_add(1);
        self.put_period(f, base);
    }

    /// Spec 8.5.
    fn fader(&mut self, f: usize) {
        let fd = &mut self.fade;
        if fd.step != 0 {
            self.cy += 25;
            fd.c1 = fd.c1.wrapping_sub(1);
            if fd.c1 == 0 {
                fd.c1 = fd.c2;
                fd.vol = fd.vol.wrapping_add(fd.step);
                self.cy += 81;
                if fd.vol == fd.target {
                    fd.step = 0;
                    fd.active = false;
                    self.cy += 40;
                }
            }
        }
        let fv = fd.vol;
        let fl = &mut self.fl[f];
        let mut d = fl.vol as u32;
        if fv & 0x40 == 0 {
            d = ((d * 4 * fv as u32) & 0xFFFF) >> 8;
            fl.envbit = 0;
            self.cy += 121;
        }
        if fl.sample {
            self.cy -= 71;
            return;
        }
        d >>= 2;
        let v = if d & 0x10 != 0 {
            self.cy += 8;
            0x0F
        } else {
            d as u8
        };
        self.regs[FIELD[f].2] = v | fl.envbit;
    }

    // ------------------------------------------------------------------ one tick before the conversion

    /// Everything of a tick up to the conversion; leaves the CPU cycles it took in `cy`.
    fn run(&mut self) {
        self.cy = 1972;
        self.trap_cy = 0;
        if !self.playing {
            // a stopped song: the tick does only the conversion (measured, h3 after its end command)
            self.cy = 404;
            return;
        }
        match self.tracks.iter().position(|t| t.pat != 0xFF) {
            Some(k) => self.cy += 33 * k as i64,
            None => {
                self.cy += 131;
                self.signal_end(1);
            }
        }
        if self.playing {
            for f in 0..4 {
                self.field(f);
            }
            if self.song & 0x80 == 0 {
                self.sequencer();
            }
        }
    }
}

// ---------------------------------------------------------------------- Paula conversion (9)

/// Time in CPU cycles from the tick's start (one colour clock is two cycles).
struct Cur {
    t0: u64,
    ph: u64,
}

impl Cur {
    fn cc(&self, tc: u64) -> u64 {
        self.t0 + (tc + self.ph) / 2
    }

    fn at(&self, hw: &mut Hw, tc: u64) {
        hw.bus.clock = self.cc(tc);
    }

    /// The beam wait from a routine entered at `tc`: the line register is read every 21 cycles
    /// until the line changes, then the horizontal position every 25 until it is at least $16.
    /// Returns the time of the last horizontal read.
    fn beam_wait(&self, tc: u64) -> u64 {
        let mut t = tc + 21;
        let l0 = self.cc(t) / 227;
        t += 12;
        for _ in 0..4000 {
            if self.cc(t) / 227 != l0 {
                break;
            }
            t += 21;
        }
        t += 25;
        for _ in 0..4000 {
            if self.cc(t) % 227 >= 22 {
                break;
            }
            t += 25;
        }
        t
    }
}

/// One-shot LEN to the next beam wait's first line read, by Paula channel (cycles).
const LEN_TO_BEAM: [u64; 4] = [129, 160, 191, 199];

/// Restart entry offsets from the previous write (the conversion entry for voice A, the previous
/// voice's period write after), in CPU cycles: by voice A, B, C and the kind of the new state
/// (silent, noise, tone). Each is the offset of the first beam-register read (21 cycles after the
/// entry of the wait routine), taken from a trace of the reference's reads; the writes after the
/// wait are only sensitive to it at a line boundary.
const RESTART_AT: [[u64; 3]; 3] = [[353, 398, 381], [434, 479, 462], [504, 549, 532]];
/// The digital voice's restart entry after voice C's period write: silent, sample.
const DIGITAL_AT: [u64; 2] = [309, 360];
/// The volume write when no restart is needed: by voice and kind (noise, tone).
const HELD_VOL: [[u64; 2]; 3] = [[677, 606], [879, 808], [914, 843]];
/// The volume write after a restart's loop LEN: by voice and kind (silent, noise, tone).
const LEN_VOL: [[u64; 3]; 3] = [[362, 378, 324], [483, 499, 445], [448, 464, 410]];

impl TfmxSt {
    /// The channel restart (spec 9): returns the time of the loop LEN write.
    fn restart(&mut self, hw: &mut Hw, c: &Cur, tc: u64, ch: usize, one: (u32, u16), lp: (u32, u16)) -> u64 {
        let m = 1u16 << ch;
        let running = self.conv.running[ch];
        let mut t = c.beam_wait(tc) + 25;
        c.at(hw, t);
        hw.per(ch, 1);
        t += 52;
        c.at(hw, t);
        m68k::AddressBus::write_word(hw.bus, 0xDFF09C, if running { m << 7 } else { 0 });
        t += 12;
        c.at(hw, t);
        hw.dmacon(if running { m } else { 0 });
        if running {
            // the original polls the channel's audio flag every 29 cycles; the loop ends 41
            // cycles after DMACON when the first look finds it
            let first = 12;
            let mut p = t + first;
            for _ in 0..2000 {
                hw.bus.paula.sync(c.cc(p), &hw.bus.ram);
                if hw.bus.paula.intreq & (0x80 << ch) != 0 {
                    break;
                }
                p += 29;
            }
            t = p - first + 41;
        } else {
            t += 41;
        }
        c.at(hw, t);
        hw.loc(ch, one.0);
        t += 12;
        c.at(hw, t);
        hw.len(ch, one.1);
        t += LEN_TO_BEAM[ch];
        t = c.beam_wait(t - 21) + 73;
        c.at(hw, t);
        m68k::AddressBus::write_word(hw.bus, 0xDFF09C, m << 7);
        t += 12;
        c.at(hw, t);
        hw.dmacon(0x8000 | m);
        // the original polls the flag every 29 cycles from 12 cycles after DMACON, and goes on
        // 29 cycles after the look that finds it set (Paula sets it at its next line start)
        let first = 12;
        let mut p = t + first;
        for _ in 0..2000 {
            hw.bus.paula.sync(c.cc(p), &hw.bus.ram);
            if hw.bus.paula.intreq & (0x80 << ch) != 0 {
                break;
            }
            p += 29;
        }
        self.conv.running[ch] = true;
        let t = c.beam_wait(p + 29 - 21) + 21;
        c.at(hw, t);
        hw.loc(ch, lp.0);
        let t = t + 12;
        c.at(hw, t);
        hw.len(ch, lp.1);
        t
    }

    fn convert(&mut self, hw: &mut Hw, t0: u64, entry: u64) {
        let c = Cur { t0, ph: self.half };
        let mut t = entry;
        let mm = !self.regs[7] & 0x3F;
        for v in 0..3 {
            let ch = SLOT_CH[v];
            let (kind, per, vol): (usize, u16, u16);
            let f = &self.regs;
            let word = (f[2 * v] as u32 | (f[2 * v + 1] as u32) << 8) & 0xFFF;
            let vt = VOLTAB[(f[8 + v] & 31) as usize];
            if mm & (1 << v) != 0 {
                kind = 2;
                per = (word * 7 + 1) as u16;
                vol = vt;
            } else if mm & (8 << v) != 0 {
                kind = 1;
                per = 0x280 - 0x10 * (f[6] & 31) as u16;
                vol = vt;
            } else {
                kind = 0;
                per = 0x100;
                vol = 0;
            }
            let state = self.conv.state[v] as usize;
            if kind == 0 || state != kind {
                let (one, lp) = match kind {
                    2 => ((self.pulse, 2), (self.pulse, 2)),
                    1 => ((self.noise, 0x200), (self.noise, 0x200)),
                    _ => ((self.silent, 1), (self.silent, 1)),
                };
                self.conv.state[v] = kind as u8;
                let re = if v == 0 { RESTART_AT[0][kind] } else { RESTART_AT[v][kind] };
                let tl = self.restart(hw, &c, t + re, ch, one, lp);
                t = tl + LEN_VOL[v][kind];
            } else {
                t += HELD_VOL[v][kind - 1];
            }
            c.at(hw, t);
            hw.vol(ch, vol);
            t += 24;
            c.at(hw, t);
            hw.per(ch, per);
        }
        // the digital voice
        let rate = self.dig.rate;
        let (kind, one) = match self.dig.state {
            0 => (0, (self.silent, 1)),
            1 => (1, (self.smpl.wrapping_add(self.dig.offset), 0)),
            _ => return,
        };
        self.dig.state = if kind == 1 { 2 } else { 0 };
        let tl = self.restart(hw, &c, t + DIGITAL_AT[kind], 1, one, (self.silent, 1));
        let t = tl + 40;
        c.at(hw, t);
        hw.per(1, rate);
        let t = t + 478;
        c.at(hw, t);
        hw.vol(1, 0x40);
    }
}

// ---------------------------------------------------------------------- construction

impl TfmxSt {
    fn init(&mut self) {
        self.regs = [0; 16];
        self.regs[7] = 0x3B;
        self.regs[10] = 0x10;
        self.regs[5] = 4;
        self.mixer = 0x3B;
        self.dig = Digital { state: 0, rate: 0x240, offset: 0 };
        self.conv.state = [0; 3];
        self.fade = Fade { vol: 0x40, target: 0x40, ..Fade::default() };
        self.speed = 5;
        self.fl = [Field::default(); 4];
        for f in &mut self.fl {
            f.stopped = true;
        }
        self.songend = false;
        let s = self.songs.get(self.sub as usize).copied().unwrap_or(0);
        self.song_start(s);
    }
}

impl Replayer for TfmxSt {
    fn format(&self) -> &'static str {
        "TFMX ST"
    }

    fn title(&self) -> String {
        let t: Vec<String> = (0..6)
            .map(|i| String::from_utf8_lossy(&self.mem[16 + 40 * i..16 + 40 * i + 40]).trim_matches(|c: char| c == '\0' || c == ' ').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        t.first().cloned().unwrap_or_default()
    }

    fn tick_clocks(&self) -> u64 {
        // the first period starts `ORIGIN_SHIFT` later (see there): the first tick keeps its time
        if self.ticks_run == 0 {
            TICK - ORIGIN_SHIFT
        } else {
            TICK
        }
    }

    fn next_tick_at(&self) -> Option<u64> {
        Some(self.tick_t0 + TICK)
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.songs.len().saturating_sub(1) as u16, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        hw.dmacon(0x000F);
        for ch in 0..4 {
            hw.wait(2);
            hw.vol(ch, 0);
        }
        self.sub = n.min(self.songs.len().saturating_sub(1) as u16);
        self.init();
    }

    fn tick(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        self.tick_t0 = t0;
        self.run();
        // the first two ticks come off the regular spacing (measured; see FIRST_TICKS)
        let adj = FIRST_TICKS.get(self.ticks_run as usize).copied().unwrap_or(0);
        self.ticks_run = self.ticks_run.saturating_add(1);
        let entry = (self.cy.max(0) + 1 + adj).max(0) as u64;
        self.convert(hw, t0, entry);
    }
}

/// The noise buffer (spec 9): a generator run for 1024 bytes.
fn noise_bytes() -> Vec<u8> {
    let mut s: u32 = 0x4849_5050;
    (0..1024)
        .map(|_| {
            let b = s.wrapping_mul(3677);
            let a = s.wrapping_add(0xE90);
            let a = (a & 0xFFFF_0000) | ((a & 0xFFFF) << 4 & 0xFFFF);
            s = (b.wrapping_add(a) & 0x7FFF_FFFF).wrapping_sub(1);
            (s >> 8) as u8
        })
        .collect()
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let _ = base;
    let (n_lo, n_hi, samples) = scan_stats(d)?;
    let (steps_t, pat_t, mac_t) = tables(d);
    let mut mem = d.to_vec();
    let empty = mem.len() as i64;
    mem.extend_from_slice(&[0xF4, 0, 0, 0, 0xF0, 0, 0, 0]);
    let blk = m.alloc(1024 + 16);
    if blk == 0 {
        return None;
    }
    m.bus.write_bytes(blk, &noise_bytes());
    let pulse = blk + 1024;
    m.bus.write_bytes(pulse, &[0xB2, 0xB2, 0x4D, 0x4D, 0, 0, 0, 0]);
    let mut smp: Vec<u8> = Vec::new();
    let mut load_cost = 0i64;
    if samples > 0 {
        let file = m.bus.read_cstr(m.deli.file_buf);
        let rest = match file.get(..5) {
            Some(p) if p.eq_ignore_ascii_case("mdat.") => &file[5..],
            _ => &file[..],
        };
        if let Ok(s) = crate::deli::read_companion(m, &format!("smpl.{rest}")).1 {
            smp = s;
        }
        if be32(&smp, 4) & 0xF0F0_F0F0 == 0 {
            for b in &mut smp {
                *b = NIB[(*b & 15) as usize];
            }
            load_cost += 54 + 49 * smp.len() as i64;
        }
        if !smp.is_empty() {
            let dir = m.deli.module_dir.to_string_lossy();
            let dir_len = dir.len() + usize::from(!dir.ends_with('/'));
            let name_len = file.rsplit('/').next().unwrap_or(&file).len();
            // one cycle more when the path (directory with its last "/" plus file name) has an even length
            // (measured on 80 modules in four directories; the cost per sample step is 91)
            load_cost += SAMPLE_LOAD + 17 * dir_len as i64 + 21 * name_len as i64 + i64::from((dir_len + name_len) % 2 == 0);
        }
    }
    let sa = m.alloc((smp.len() as u32).max(4) + 4);
    if sa == 0 {
        return None;
    }
    m.bus.write_bytes(sa, &smp);
    let (songs, song_cost) = song_list_cost(d);
    let mut p = TfmxSt {
        mem,
        empty,
        steps_t,
        pat_t,
        mac_t,
        songs,
        sub: 0,
        playing: false,
        song: 0,
        cur: 0,
        first: 0,
        last: 0,
        speed: 5,
        counter: 0,
        loopc: 0,
        tracks: [Track::default(); 8],
        mute: [false; 8],
        fl: [Field::default(); 4],
        regs: [0; 16],
        mixer: 0,
        fade: Fade::default(),
        dig: Digital::default(),
        songend: false,
        cy: 0,
        trap_cy: 0,
        ticks_run: 0,
        tick_t0: 0,
        half: 0,
        noise: blk,
        pulse,
        silent: pulse + 4,
        smpl: sa,
        conv: Conv::default(),
    };
    p.init();
    // The first tick comes after the original's set-up: its fixed part, the scan of the macro
    // steps (by class), the companion sample's load and conversion, the building of the subsong
    // list and the first track step (the cost `init` left in `cy`); all in CPU cycles, the sum a
    // whole number of cycles, so the tick phase may be half a colour clock.
    let setup = E0H_BASE
        + 65 * n_lo as i64
        + 83 * n_hi as i64
        + 91 * samples as i64
        + load_cost
        + song_cost
        + p.cy
        - if be32(d, 468) != 0 {
            EXPLICIT_LAYOUT - MACRO_WALK * (explicit_walk(d) - 1) - GAP_STEP * explicit_gap(d) + 2 * (explicit_gap(d) & 1)
        } else {
            0
        };
    // the run's origin is `START_PHASE` before the engine's aligned start; the engine adds one period
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    // The tick is entered at a whole colour clock plus one cycle: of the cycles the set-up
    // takes, the largest odd number not above them counts (measured: the reference's first
    // write time is exact for it on every module, and a cycle off for the even sums).
    let e0 = (setup.max(1) as u64 - 1) / 2;
    p.half = 1;
    m.bus.clock = (origin + e0).saturating_sub(TICK).max(m.bus.clock) + ORIGIN_SHIFT;
    Some(Box::new(p))
}

#[cfg(test)]
#[path = "tfmxst_tests.rs"]
mod tests;

