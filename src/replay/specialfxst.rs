//! Special FX ST (Eagleplayer "Special-FX_ST", module prefix `doda.`): the Atari ST music of
//! Matthew Cannon and Jonathan Dunn (Special FX, 1989-92).
//!
//! Clean-room, written from a behaviour specification drawn up from the register writes and
//! timing histograms of the 68k reference player; no original code, nothing embedded. A file is
//! an Atari ST memory image: the game's sound driver followed by its music tables. The replay
//! never runs that driver. It finds the music tables by searching the image for fixed byte
//! patterns, interprets the three voice sequences itself, keeps a YM2149 register image and
//! turns it, once a tick, into Paula writes (a pulse wave, a noise block and a silent word on
//! three channels, the noise mix on a fourth), restarting a channel the way the original does:
//! waits for the beam, INTREQ and DMACON writes and a poll of this port's own Paula's audio
//! interrupt flag. Every write is placed at the colour clock the specification's cycle ledger
//! gives.
//!
//! Choices where the specification is silent or its data is not embedded (all safe):
//! * a note index outside the 102-entry period table reads the player's own memory in the
//!   original. Indices 102 to 121 give the table addresses as the specification describes, the
//!   other out-of-range indices (below zero, 122 to 127) give a period of 0 here;
//! * reads outside the image give zero; a voice that reads more than a bounded number of events
//!   in one tick, or a sequence that never produces a pattern, ends (the voice stops);
//! * the song-end restart after a tick that finds no active voice is done without extra timing
//!   (the specification never timed it);
//! * only emulation type 0 ("Mad Max") exists, as the reference host never loads a
//!   configuration.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;
use std::sync::atomic::{AtomicU64, Ordering};

pub const OPEN: Open = Open { name: "Special-FX ST", detect, new };

/// The phase (colour clocks modulo the raster line of 227) of the tick calls; the reference
/// host's differs per run. `u64::MAX` selects the default. A test and measurement knob.
pub static PHASE: AtomicU64 = AtomicU64::new(u64::MAX);

const LINE: u64 = 227;
const TICK: u64 = 70940;
const DEFAULT_PHASE: u64 = 196;
/// Events one voice may read in one tick before it is ended (bad data).
const EVENT_CAP: u32 = 20_000;

/// The driver's period table: YM tone periods by note.
const PERIODS: [u16; 102] = [
    0xEF8, 0xE10, 0xD60, 0xC80, 0xBD8, 0xB28, 0xA88, 0x9F0, 0x960, 0x8E0, 0x858, 0x7E0, 0x77C, 0x708, 0x6B0, 0x640, 0x5EC, 0x594, 0x544,
    0x4F8, 0x4B0, 0x470, 0x42C, 0x3F0, 0x3BE, 0x384, 0x358, 0x320, 0x2F6, 0x2CA, 0x2A2, 0x27C, 0x258, 0x238, 0x216, 0x1F8, 0x1DF, 0x1C2,
    0x1AC, 0x190, 0x17B, 0x165, 0x151, 0x13E, 0x12C, 0x11C, 0x10B, 0xFC, 0xEF, 0xE1, 0xD6, 0xC8, 0xBD, 0xB2, 0xA8, 0x9F, 0x96, 0x8E, 0x85,
    0x7E, 0x77, 0x70, 0x6B, 0x64, 0x5E, 0x59, 0x54, 0x4F, 0x4B, 0x47, 0x42, 0x3F, 0x3B, 0x38, 0x35, 0x32, 0x2F, 0x2C, 0x2A, 0x27, 0x25,
    0x23, 0x21, 0x1F, 0x1D, 0x1C, 0x1A, 0x19, 0x17, 0x16, 0x15, 0x13, 0x12, 0x11, 0x10, 0xF, 0xE, 0xD, 0xC, 0xB, 0xA, 0x9,
];

/// Paula volume by the YM volume register.
const VT: [u16; 16] = [0, 1, 2, 3, 4, 5, 7, 9, 12, 16, 21, 27, 34, 42, 52, 64];

/// The voice masks: tone bit and noise bit of the YM mixer.
const VM: [u8; 3] = [0x09, 0x12, 0x24];
/// The Paula channel of voices A, B, C and of the noise mix.
const CH: [usize; 4] = [0, 3, 2, 1];

// ---------------------------------------------------------------------------------
// Recognition and table search

fn detect(d: &[u8]) -> bool {
    if d.len() <= 2300 {
        return false;
    }
    let l = be32(d, 0);
    if !(l == 0 || l == 0x0000_0101 || be16(d, 0) == 0x0101 || be16(d, 0) == 0x6000) {
        return false;
    }
    let Some(m) = (0..140).step_by(2).find(|&m| be32(d, m) == 0x0009_0800) else { return false };
    let tails = [(74, 0x0112_0900), (148, 0x0224_0A00), (222, 0x0009_0800), (296, 0x0112_0900), (370, 0x0224_0A00)];
    if !tails.iter().all(|&(o, v)| be32(d, m + o) == v) {
        return false;
    }
    let mut p = 2000;
    while p + 4 < d.len() {
        if be32(d, p) == 0x0EF8_0E10 {
            return true;
        }
        p += 2;
    }
    false
}

/// The tables of a module, as addresses in the module's memory (0 = absent).
#[derive(Clone, Copy, Debug, Default)]
struct Tables {
    subsong: i64,
    pattern: i64,
    extra: i64,
    drum: i64,
    trigger: i64,
    arp: i64,
    slide: i64,
    env: i64,
    slist: i64,
    digital: i64,
    count: u16,
}

struct Scan<'a> {
    d: &'a [u8],
    q: usize,
    counter: i64,
}

impl Scan<'_> {
    fn w(&self, o: usize) -> u16 {
        be16(self.d, o)
    }
    fn l(&self, o: usize) -> u32 {
        be32(self.d, o)
    }
    /// The signed word at `o` plus `o`.
    fn disp(&self, o: usize) -> Option<usize> {
        let t = o as i64 + be16(self.d, o) as i16 as i64;
        usize::try_from(t).ok()
    }
    /// One forward step of two bytes; false when the counter runs out or the file does.
    fn step(&mut self) -> bool {
        self.q += 2;
        self.counter -= 2;
        self.counter >= 0 && self.q <= self.d.len()
    }
    /// Walks until `f` holds at `q`.
    fn find(&mut self, f: impl Fn(&Self, usize) -> bool) -> bool {
        loop {
            if self.q > self.d.len() {
                return false;
            }
            if f(self, self.q) {
                return true;
            }
            if !self.step() {
                return false;
            }
        }
    }
}

fn locate(d: &[u8], base: i64) -> Option<Tables> {
    let mut s = Scan { d, q: 0, counter: d.len() as i64 };
    let mut t = Tables::default();
    let m = |o: usize| base + o as i64;
    // 1. the subsong table
    if !s.find(|s, q| s.w(q) == 0xE740) {
        return None;
    }
    let q = s.q;
    if q < 10 {
        return None;
    }
    t.subsong = m(s.disp(q - 2)?);
    let provisional = s.w(q - 10);
    s.q = q + 2;
    // 2. the extra pattern table, then the pattern table
    loop {
        if s.q > d.len() {
            return None;
        }
        let q = s.q;
        if s.l(q) == 0x101A_234A {
            t.extra = m(s.disp(q + 8)?);
            s.counter -= 10;
            s.q += 10;
        } else if s.l(q) == 0x4880_D040 {
            if q < 2 {
                return None;
            }
            t.pattern = m(s.disp(q - 2)?);
            break;
        } else if !s.step() {
            return None;
        }
        if s.counter < 0 {
            return None;
        }
    }
    // 3. the drum table: the load before the bit test
    if !s.find(|s, q| s.l(q) == 0x08C7_0007) {
        return None;
    }
    let mut p = s.q;
    loop {
        if s.w(p) == 0x41FA {
            break;
        }
        if p < 2 {
            return None;
        }
        p -= 2;
    }
    t.drum = m(s.disp(p + 2)?);
    // 4. the trigger table
    if !s.find(|s, q| s.w(q) == 0x41F9 || s.w(q) == 0x41FA) {
        return None;
    }
    if s.w(s.q) == 0x41F9 {
        t.trigger = s.l(s.q + 2) as i64;
    } else {
        s.q += 2;
        t.trigger = m(s.disp(s.q)?);
    }
    // 5. the arpeggio table
    if !s.find(|s, q| s.l(q) == 0x0887_0002) {
        return None;
    }
    let q = s.q;
    let mut image = None;
    if s.w(q + 4) == 0x41F9 {
        t.arp = s.l(q + 6) as i64;
        image = Some(t.arp);
    } else {
        t.arp = m(s.disp(q + 6)?);
    }
    // 6. the slide table
    if !s.find(|s, q| s.l(q) == 0x08C7_0002) {
        return None;
    }
    let q = s.q;
    if s.w(q + 8) == 0x41F9 {
        t.slide = s.l(q + 10) as i64;
    } else {
        t.slide = m(s.disp(q + 10)?);
    }
    s.counter -= 10;
    s.q = q + 10;
    if s.counter < 0 {
        return None;
    }
    // 7. the volume envelope table
    if !s.find(|s, q| s.w(q) == 0x41FA) {
        return None;
    }
    s.q += 2;
    t.env = m(s.disp(s.q)?);
    // 8. the slide list table (command 0x8C); the period table's start ends the search
    let mut ended = false;
    loop {
        if s.q > d.len() {
            return None;
        }
        let q = s.q;
        if s.l(q) == 0x0EF8_0E10 {
            ended = true;
            break;
        }
        if s.l(q) == 0x7000_101A {
            if q < 2 {
                return None;
            }
            t.slist = m(s.disp(q - 2)?);
            break;
        }
        if !s.step() {
            return None;
        }
    }
    // 9. the digital sample table (unused by the replay)
    let mut digital_at = None;
    if !ended {
        loop {
            if s.q > d.len() {
                return None;
            }
            let q = s.q;
            if s.l(q) == 0x0EF8_0E10 {
                break;
            }
            if s.w(q) == 0x41FA {
                let o = s.disp(q + 2)?;
                if o > d.len() {
                    return None;
                }
                t.digital = m(o);
                digital_at = Some(q + 2);
                break;
            }
            if !s.step() {
                return None;
            }
        }
    }
    // 10. with an image base: rebase the absolute addresses
    if let (Some(a), Some(from)) = (image, digital_at) {
        s.q = from;
        loop {
            if s.q > d.len() {
                return None;
            }
            let q = s.q;
            if s.l(q) == 0x0011_0010 {
                let e = base + q as i64 + 4;
                t.arp = t.arp - a + e;
                t.slide = t.slide - a + e;
                t.trigger = t.trigger - a + e;
                break;
            }
            if !s.step() {
                return None;
            }
        }
    }
    // the subsong count
    let diff = t.pattern - t.subsong;
    if diff < 0 {
        return None;
    }
    t.count = if diff / 8 != 0 { (diff / 8).min(255) as u16 } else { provisional };
    (t.count >= 1).then_some(t)
}

// ---------------------------------------------------------------------------------
// State

/// Voice flag bits.
const F_ACTIVE: u16 = 0x8000;
const F_KICK: u16 = 1;
const F_ARP: u16 = 2;
const F_SLIDE: u16 = 4;
const F_ENV: u16 = 8;
const F_TRIG: u16 = 0x10;
const F_NOISEPER: u16 = 0x20;
const F_FIXED: u16 = 0x40;
const F_DRUM: u16 = 0x80;

#[derive(Clone, Copy, Default)]
struct Voice {
    flags: u16,
    pat: i64,
    seq_start: i64,
    seq: i64,
    rep: u8,
    trans: u8,
    mix: u8,
    dur: u8,
    fixed: u8,
    n0: u8,
    n: u8,
    per: u16,
    vol: u8,
    kick_off: u8,
    kick_len: u8,
    kick_cnt: u8,
    drum: i64,
    trig_tick: u8,
    trig_env: i64,
    arp_start: i64,
    arp_pos: i64,
    arp_len: u8,
    arp_rem: u8,
    arp_speed: u8,
    arp_timer: u8,
    sl_start: i64,
    sl_pos: i64,
    sl_cnt: u16,
    sl_delta: u16,
    env_start: i64,
    env_pos: i64,
    env_cnt: u8,
}

/// One emulator record (a Paula channel).
#[derive(Clone, Copy)]
struct Rec {
    vol: u16,
    per: u16,
    mode: u8,
}

impl Default for Rec {
    fn default() -> Self {
        Rec { vol: 0, per: 0x100, mode: 0 }
    }
}

pub struct SpecialFxSt {
    d: Vec<u8>,
    base: i64,
    tb: Tables,
    sub: u16,
    v: [Voice; 3],
    /// The YM shadow R0..R10 the replay writes.
    r: [u8; 11],
    /// The registers the emulator works from.
    er: [u8; 11],
    /// Song end bytes.
    se: [u8; 4],
    rec: [Rec; 4],
    nc: u8,
    length: i64,
    noise: u32,
    pulse: u32,
    silent: u32,
    songend: bool,
    // timing
    call: u64,
    t: u64,
    next_at: Option<u64>,
}

fn phase() -> u64 {
    match PHASE.load(Ordering::Relaxed) {
        u64::MAX => DEFAULT_PHASE,
        p => p % LINE,
    }
}

/// The noise block: a generator run for 1024 bytes.
fn noise_bytes() -> Vec<u8> {
    let mut x: u32 = 0x4849_5050;
    (0..1024)
        .map(|_| {
            let y = x.wrapping_mul(3677);
            let z = x.wrapping_add(0xE90);
            let z = (z & 0xFFFF_0000) | ((z & 0xFFFF) << 4 & 0xFFFF);
            x = (z.wrapping_add(y) & 0x7FFF_FFFF).wrapping_sub(1);
            (x >> 8) as u8
        })
        .collect()
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let tb = locate(d, base as i64)?;
    let blk = m.alloc(0x4408 + 16);
    if blk == 0 {
        return None;
    }
    m.bus.write_bytes(blk, &noise_bytes());
    m.bus.write_bytes(blk + 0x4400, &[0x70, 0x90, 0xB2, 0x4D, 0, 0]);
    let mut s = SpecialFxSt {
        d: d.to_vec(),
        base: base as i64,
        tb,
        sub: 1,
        v: [Voice::default(); 3],
        r: [0; 11],
        er: [0; 11],
        se: [0xFF, 0, 0xFF, 0xFF],
        rec: [Rec::default(); 4],
        nc: 0,
        length: 1,
        noise: blk,
        pulse: blk + 0x4402,
        silent: blk + 0x4404,
        songend: false,
        call: 0,
        t: 0,
        next_at: None,
    };
    s.start_subsong(1);
    let due = m.bus.clock + TICK;
    s.next_at = Some(due + (phase() + LINE - due % LINE) % LINE);
    Some(Box::new(s))
}

// ---------------------------------------------------------------------------------
// Memory and table helpers

impl SpecialFxSt {
    fn rb(&self, a: i64) -> u8 {
        usize::try_from(a - self.base).ok().and_then(|o| self.d.get(o).copied()).unwrap_or(0)
    }

    fn rw(&self, a: i64) -> u16 {
        (self.rb(a) as u16) << 8 | self.rb(a + 1) as u16
    }

    fn sw(&self, a: i64) -> i64 {
        self.rw(a) as i16 as i64
    }

    /// Entry `n` of a table of word displacements, each relative to its own address.
    fn entry(&self, table: i64, n: i64) -> i64 {
        let a = table + 2 * n;
        a + self.sw(a)
    }

    /// The tone period of note `i` (a signed index; the player's memory outside the table).
    fn period(&self, i: i8) -> u16 {
        let i = i as i32;
        match i {
            0..=101 => PERIODS[i as usize],
            102..=121 => {
                let k = (i - 102) as usize;
                let t = &self.tb;
                let list = [t.arp, t.slide, t.env, t.trigger, t.drum, t.subsong, t.pattern, t.extra, t.slist, t.digital];
                let a = list[k / 2] as u32;
                if k % 2 == 0 {
                    (a >> 16) as u16
                } else {
                    a as u16
                }
            }
            // below zero and 122..127: the player's own bytes, not embedded
            _ => 0,
        }
    }

    // ----- subsong start (spec 3.2)

    fn start_subsong(&mut self, n: u16) {
        self.sub = n;
        self.v = [Voice::default(); 3];
        self.r = [0; 11];
        self.er = [0; 11];
        self.se = [0xFF, 0, 0xFF, 0xFF];
        self.rec = [Rec::default(); 4];
        self.nc = 0;
        let row = self.tb.subsong + 8 * (n as i64 - 1);
        let mut starts = [0i64; 3];
        let mut used = 0;
        for i in 0..3 {
            let a = row + 2 * i as i64;
            let dsp = self.sw(a);
            if dsp == 0 {
                continue;
            }
            used += 1;
            starts[i] = a + dsp;
            let v = &mut self.v[i];
            v.flags = F_ACTIVE;
            v.dur = 1;
            v.rep = 1;
            v.trans = 0;
            v.seq_start = a + dsp;
            v.seq = a + dsp;
            self.read_sequence(i);
        }
        self.length = if used == 1 {
            1
        } else if starts[1] - starts[0] >= 0 {
            starts[1] - starts[0]
        } else {
            let mut p = starts[0];
            let mut n = 1;
            while n < 100_000 {
                let b = self.rb(p);
                if b == 0x80 || b == 0x81 {
                    break;
                }
                p += 1;
                n += 1;
            }
            n
        };
    }

    /// Reads the sequence of voice `i` from its position, untimed (the start of a subsong).
    fn read_sequence(&mut self, i: usize) {
        for _ in 0..EVENT_CAP {
            let at = self.v[i].seq;
            let b = self.rb(at);
            self.v[i].seq = at + 1;
            match b {
                0..=0x7F => {
                    self.v[i].pat = self.entry(self.tb.pattern, b as i64);
                    return;
                }
                0x80 => {
                    self.v[i].flags = 0;
                    return;
                }
                0xFF => {
                    let x = self.rb(at + 1) as i8 as i64;
                    self.v[i].seq = at + 2;
                    self.v[i].pat = self.entry(self.tb.extra, x);
                    return;
                }
                0x81 => {
                    self.v[i].seq = at + 1 + self.rb(at + 1) as i8 as i64;
                }
                0x82 => {
                    self.v[i].rep = self.rb(at + 1);
                    self.v[i].seq = at + 2;
                }
                0x83 => {
                    self.v[i].trans = self.rb(at + 1);
                    self.v[i].seq = at + 2;
                }
                _ => {}
            }
        }
        self.v[i].flags = 0;
    }
}

// ---------------------------------------------------------------------------------
// The replay (spec 3)

impl SpecialFxSt {
    fn duration(&mut self, i: usize) {
        if self.v[i].flags & F_FIXED != 0 {
            self.v[i].dur = self.v[i].fixed;
            self.t += 45;
        } else {
            self.v[i].dur = self.rb(self.v[i].pat);
            self.v[i].pat += 1;
            self.t += 45 + 8;
        }
    }

    /// The song-end test of a sequence jump of voice `i` (spec 3.6).
    fn song_end_test(&mut self, i: usize) {
        self.t += 32;
        match i {
            0 => {
                self.se[0] = 0;
                self.t += 47;
            }
            2 => {
                self.se[2] = 0;
                self.t += 76;
            }
            _ => {
                self.se[3] = 0;
                self.t += 112;
                if self.v[2].flags & F_ACTIVE == 0 {
                    self.se[2] = 0;
                    self.t += 16;
                }
            }
        }
        self.t += 21;
        if self.se == [0; 4] {
            self.t += 68;
            self.se = [0xFF, 0, 0xFF, 0xFF];
            self.songend = true;
            // the host's song-end function
            self.t = (self.t & !1) + 16;
        }
        self.t += 44;
    }

    /// Reads pattern events of voice `i` until one ends the step. False when the voice stopped.
    fn events(&mut self, i: usize) -> bool {
        for _ in 0..EVENT_CAP {
            let at = self.v[i].pat;
            let b = self.rb(at);
            self.v[i].pat = at + 1;
            self.t += 19;
            if b < 0x80 {
                self.t += 293;
                let v = &mut self.v[i];
                v.n0 = b.wrapping_add(v.trans);
                if v.flags & F_KICK != 0 {
                    v.n = v.n0.wrapping_add(v.kick_off);
                    v.kick_cnt = v.kick_len;
                    self.t += 32;
                } else {
                    v.n = v.n0;
                }
                let per = self.period(self.v[i].n as i8);
                let v = &mut self.v[i];
                v.per = per;
                v.arp_pos = v.arp_start;
                v.arp_rem = v.arp_len;
                v.arp_timer = 1;
                v.sl_pos = v.sl_start;
                v.sl_cnt = 1;
                v.env_pos = v.env_start;
                v.env_cnt = 1;
                v.vol = 0;
                v.flags |= F_ENV;
                v.flags &= !F_DRUM;
                self.duration(i);
                return true;
            }
            self.t += 19;
            if b < 0x90 {
                self.t += if b >= 0x8E { 51 } else { 65 };
                match b {
                    0x80 => {
                        self.t += 22;
                        self.v[i].flags = 0;
                        return false;
                    }
                    0x81 => {
                        self.t += 30;
                        self.v[i].pat = at + 1 + self.rb(at + 1) as i8 as i64;
                    }
                    0x82 => {
                        if !self.end_of_pattern(i) {
                            return false;
                        }
                    }
                    0x83..=0x85 => {
                        let v = &mut self.v[i];
                        match b {
                            0x83 => {
                                v.mix = VM[i] & 0x07;
                                v.flags &= !F_NOISEPER;
                                self.t += 52;
                            }
                            0x84 => {
                                v.mix = VM[i] & 0x38;
                                v.flags |= F_NOISEPER;
                                self.t += 50;
                            }
                            _ => {
                                v.mix = VM[i] & 0x3F;
                                v.flags |= F_NOISEPER;
                                self.t += 50;
                            }
                        }
                    }
                    0x86 => {
                        self.t += 10;
                        self.duration(i);
                        return true;
                    }
                    0x87 => {
                        self.t += 40;
                        let v = &mut self.v[i];
                        v.flags &= !F_ENV;
                        v.vol = 0;
                        self.duration(i);
                        return true;
                    }
                    0x88 => {
                        self.t += 38;
                        let f = self.rb(at + 1);
                        let v = &mut self.v[i];
                        v.fixed = f;
                        v.pat += 1;
                        v.flags |= F_FIXED;
                    }
                    0x89 => {
                        self.t += 24;
                        self.v[i].flags &= !F_FIXED;
                    }
                    0x8A => {
                        self.t += 68;
                        let (o, l) = (self.rb(at + 1), self.rb(at + 2));
                        let v = &mut self.v[i];
                        v.flags |= F_KICK;
                        v.flags &= !F_ARP;
                        v.kick_off = o;
                        v.kick_len = l;
                        v.pat += 2;
                    }
                    0x8B => {
                        self.t += 24;
                        self.v[i].flags &= !F_KICK;
                    }
                    0x8C => {
                        self.t += 104;
                        let k = self.rb(at + 1) as i64;
                        let s = self.entry(self.tb.slist, k);
                        let v = &mut self.v[i];
                        v.pat += 1;
                        v.flags |= F_SLIDE;
                        v.flags &= !F_ARP;
                        v.sl_start = s;
                    }
                    0x8D => {
                        self.t += 72;
                        let v = &mut self.v[i];
                        v.flags &= !(F_DRUM | F_ENV);
                        v.pat += 3;
                        v.vol = 0;
                        self.duration(i);
                        return true;
                    }
                    _ => {}
                }
                continue;
            }
            match b {
                0x90..=0x97 => {
                    self.t += 177;
                    let d = self.entry(self.tb.drum, (b - 0x90) as i64);
                    self.v[i].drum = d;
                    self.v[i].flags |= F_DRUM;
                    self.duration(i);
                    return true;
                }
                0x98..=0x9F => {
                    self.t += 87;
                    let k = (b - 0x98) as i64;
                    if k == 0 {
                        self.t += 24;
                        self.v[i].flags &= !F_TRIG;
                    } else {
                        self.t += 98;
                        let e = self.entry(self.tb.trigger, k);
                        let (tick, env) = (self.rb(e), e + 1);
                        let v = &mut self.v[i];
                        v.flags |= F_TRIG;
                        v.trig_tick = tick;
                        v.trig_env = env;
                    }
                }
                0xA0..=0xBF => {
                    self.t += 68;
                    let k = (b - 0xA0) as i64;
                    if k == 0 {
                        self.t += 24;
                        self.v[i].flags &= !F_ARP;
                    } else {
                        self.t += 144;
                        let e = self.entry(self.tb.arp, k);
                        let (sp, len) = (self.rb(e), self.rb(e + 1));
                        let v = &mut self.v[i];
                        v.flags |= F_ARP;
                        v.flags &= !F_SLIDE;
                        v.arp_speed = sp;
                        v.arp_len = len;
                        v.arp_start = e + 2;
                        v.arp_timer = 1;
                    }
                }
                0xC0..=0xDF => {
                    self.t += 49;
                    let k = (b - 0xC0) as i64;
                    if k == 0 {
                        self.t += 24;
                        self.v[i].flags &= !F_SLIDE;
                    } else {
                        self.t += 96;
                        let s = self.entry(self.tb.slide, k);
                        let v = &mut self.v[i];
                        v.flags |= F_SLIDE;
                        v.flags &= !F_ARP;
                        v.sl_start = s;
                    }
                }
                _ => {
                    self.t += 89;
                    let e = self.entry(self.tb.env, (b - 0xE0) as i64);
                    self.v[i].env_start = e;
                }
            }
        }
        // a pattern that never ends a step: end the voice
        self.v[i].flags = 0;
        false
    }

    /// Command 0x82, the end of a pattern. False when the voice stopped.
    fn end_of_pattern(&mut self, i: usize) -> bool {
        self.t += 55;
        let v = &mut self.v[i];
        v.rep = v.rep.wrapping_sub(1);
        if v.rep != 0 {
            self.t += 70;
            let n = self.rb(self.v[i].seq - 1) as i8 as i64;
            self.v[i].pat = self.entry(self.tb.pattern, n);
            return true;
        }
        v.rep = 1;
        v.trans = 0;
        self.t += 32;
        for _ in 0..EVENT_CAP {
            let at = self.v[i].seq;
            let b = self.rb(at);
            self.v[i].seq = at + 1;
            self.t += 19;
            if b < 0x80 {
                self.t += 70;
                self.v[i].pat = self.entry(self.tb.pattern, b as i64);
                return true;
            }
            self.t += 17;
            if b == 0x80 {
                self.t += 22;
                self.v[i].flags = 0;
                return false;
            }
            self.t += 17;
            if b == 0xFF {
                self.t += 78;
                let x = self.rb(at + 1) as i8 as i64;
                self.v[i].seq = at + 2;
                self.v[i].pat = self.entry(self.tb.extra, x);
                return true;
            }
            self.t += 15;
            if b == 0x81 {
                self.t += 48;
                self.v[i].seq = at + 1 + self.rb(at + 1) as i8 as i64;
                self.song_end_test(i);
                continue;
            }
            self.t += 15;
            if b == 0x82 {
                self.t += 26;
                self.v[i].rep = self.rb(at + 1);
                self.v[i].seq = at + 2;
                continue;
            }
            self.t += 13;
            if b == 0x83 {
                self.t += 26;
                self.v[i].trans = self.rb(at + 1);
                self.v[i].seq = at + 2;
            }
        }
        self.v[i].flags = 0;
        false
    }

    /// The voice routine (spec 3.4): returns the period and the volume.
    fn voice(&mut self, i: usize) -> (u16, u8) {
        self.t += 27;
        self.v[i].dur = self.v[i].dur.wrapping_sub(1);
        if self.v[i].dur == 0 && !self.events(i) {
            self.t += 16;
            return (0, 0);
        }
        // the effects
        self.t += 19;
        if self.v[i].flags & F_DRUM != 0 {
            self.t += 63;
            let a = self.v[i].drum;
            let m = VM[i] & self.rb(a);
            if m == 0 {
                self.t += 18 + 16;
                return (0, 0);
            }
            self.t += 78 + 16;
            self.r[7] = (self.r[7] | VM[i]) ^ m;
            let vol = self.rb(a + 1);
            let p = self.rw(a + 2);
            self.r[6] = p as u8;
            self.v[i].drum = a + 4;
            return (p, vol);
        }
        self.t += 89;
        self.r[7] = (self.r[7] | VM[i]) ^ self.v[i].mix;
        let mut p = self.v[i].per;
        let f = self.v[i].flags;
        if f & F_KICK != 0 {
            self.t += 27;
            let v = &mut self.v[i];
            v.kick_cnt = v.kick_cnt.wrapping_sub(1);
            if v.kick_cnt == 0xFF {
                self.t += 54;
                v.n = v.n0;
                p = self.period(self.v[i].n0 as i8);
            }
        }
        self.t += 21;
        if f & F_ARP != 0 {
            self.t += 27;
            let v = &mut self.v[i];
            v.arp_timer = v.arp_timer.wrapping_sub(1);
            if v.arp_timer == 0 {
                self.t += 51;
                let o = self.rb(self.v[i].arp_pos);
                let v = &mut self.v[i];
                v.arp_pos += 1;
                v.arp_rem = v.arp_rem.wrapping_sub(1);
                if v.arp_rem == 0 {
                    self.t += 36;
                    v.arp_pos = v.arp_start;
                    v.arp_rem = v.arp_len;
                }
                v.arp_timer = v.arp_speed;
                let n = v.n;
                p = self.period(o.wrapping_add(n) as i8);
                self.t += 78;
            }
        }
        self.t += 21;
        if f & F_SLIDE != 0 {
            self.t += 27;
            let v = &mut self.v[i];
            v.sl_cnt = v.sl_cnt.wrapping_sub(1);
            if v.sl_cnt == 0 {
                self.t += 43;
                let mut pos = self.v[i].sl_pos;
                if self.sw(pos) < 0 {
                    self.t += 28;
                    pos = pos + 2 + self.sw(pos + 2);
                }
                let (c, dl) = (self.rw(pos), self.rw(pos + 2));
                let v = &mut self.v[i];
                v.sl_cnt = c;
                v.sl_delta = dl;
                v.sl_pos = pos + 4;
                self.t += 32;
            }
            p = p.wrapping_add(self.v[i].sl_delta);
            self.t += 12;
        }
        self.v[i].per = p;
        self.t += 12 + 21;
        if f & F_TRIG != 0 {
            self.t += 35;
            let v = &mut self.v[i];
            if v.trig_tick == v.dur {
                self.t += 44;
                v.env_pos = v.trig_env;
                v.env_cnt = 1;
            }
        }
        self.t += 21;
        if self.v[i].flags & F_ENV != 0 {
            self.t += 27;
            let v = &mut self.v[i];
            v.env_cnt = v.env_cnt.wrapping_sub(1);
            if v.env_cnt == 0 {
                self.t += 43;
                let mut pos = self.v[i].env_pos;
                let mut c = self.rb(pos);
                if c != 0 {
                    self.t += 11;
                    if c as i8 <= 0 {
                        self.t += 36;
                        pos = pos + 1 + self.rb(pos + 1) as i8 as i64;
                        c = self.rb(pos);
                    }
                    let vol = self.rb(pos + 1);
                    let v = &mut self.v[i];
                    v.env_cnt = c;
                    v.vol = vol;
                    v.env_pos = pos + 2;
                    self.t += 32;
                }
            }
        }
        self.t += 12 + 16;
        (p, self.v[i].vol)
    }

    fn hostsongend(&mut self) {
        self.songend = true;
    }

    fn replay(&mut self) {
        self.r[7] = 0xFF;
        self.t += 20;
        for i in 0..3 {
            self.t += 27;
            if self.v[i].flags & F_ACTIVE != 0 {
                self.t += 34;
                let (p, vol) = self.voice(i);
                self.t += 115;
                let f = self.v[i].flags;
                self.r[8 + i] = vol;
                self.r[2 * i] = p as u8;
                self.r[2 * i + 1] = (p >> 8) as u8;
                if f & F_NOISEPER != 0 {
                    self.t += 16;
                    self.r[6] = (p >> 8) as u8;
                }
            }
        }
        self.t += 242;
    }
}

// ---------------------------------------------------------------------------------
// The emulator (spec 4)

const ST: [u64; 4] = [95, 126, 157, 165];
const CV: [u64; 4] = [163, 180, 187, 214];
const SV: [u64; 4] = [79, 110, 141, 149];

impl SpecialFxSt {
    fn clk(&self) -> u64 {
        self.call + self.t / 2
    }

    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.clk();
    }

    /// The raster wait: the line register until it changes, then the horizontal position
    /// until it is at least 22.
    fn line_wait(&mut self) {
        self.t += 12;
        let first = self.clk() / LINE;
        for _ in 0..1000 {
            self.t += 12;
            let l = self.clk() / LINE;
            self.t += 9;
            if l != first {
                break;
            }
        }
        for _ in 0..1000 {
            self.t += 16;
            let h = self.clk() % LINE;
            self.t += 9;
            if h >= 22 {
                break;
            }
        }
    }

    /// Polls INTREQ until all bits of `mask` are set.
    fn poll(&mut self, hw: &mut Hw, mask: u16) {
        for _ in 0..400_000 {
            self.t += 12;
            self.at(hw);
            let r = hw.bus.read_word(0xDFF01E);
            self.t += 17;
            if r & mask == mask {
                break;
            }
        }
    }

    /// The channel restart (spec 4.3).
    fn restart(&mut self, hw: &mut Hw, ch: usize, s: u32, len: u16, per: u16) {
        let b = 1u16 << ch;
        self.t += 30;
        self.line_wait();
        self.t += 16;
        self.at(hw);
        hw.per(ch, 1);
        self.t += 12;
        self.at(hw);
        let s1 = hw.bus.read_word(0xDFF002) & b;
        self.t += 40;
        self.at(hw);
        hw.bus.write_word(0xDFF09C, s1 << 7);
        self.t += 12;
        self.at(hw);
        hw.dmacon(s1);
        self.poll(hw, s1 << 7);
        self.t += 12;
        self.at(hw);
        hw.loc(ch, s);
        self.t += 12;
        self.at(hw);
        hw.len(ch, len);
        self.t += ST[ch] + 22 + 20;
        self.at(hw);
        hw.per(ch, per);
        self.line_wait();
        self.t += 12;
        self.at(hw);
        let s2 = !hw.bus.read_word(0xDFF002) & b;
        self.t += 52;
        self.at(hw);
        hw.bus.write_word(0xDFF09C, s2 << 7);
        self.t += 12;
        self.at(hw);
        hw.dmacon(0x8000 | s2);
        self.poll(hw, s2 << 7);
        self.line_wait();
        self.t += 12;
        self.at(hw);
        hw.loc(ch, s);
        self.t += 12;
        self.at(hw);
        hw.len(ch, len);
        self.t += 16;
    }

    /// Volume and period write of record `k`.
    fn write_vp(&mut self, hw: &mut Hw, k: usize) {
        let ch = CH[k];
        self.t += 44 + CV[ch] + SV[ch] + 12;
        self.at(hw);
        hw.vol(ch, self.rec[k].vol);
        self.t += 24;
        self.at(hw);
        hw.per(ch, self.rec[k].per);
        self.t += 10 + SV[ch];
    }

    fn restart_rec(&mut self, hw: &mut Hw, k: usize, mode: u8, after: u64) {
        self.rec[k].mode = mode;
        self.t += 62;
        let (s, len) = match mode {
            1 => (self.pulse, 1),
            2 => (self.noise, 0x200),
            _ => (self.silent, 1),
        };
        let per = self.rec[k].per;
        self.restart(hw, CH[k], s, len, per);
        self.t += after;
    }

    fn disable(&mut self, hw: &mut Hw, k: usize) {
        self.t += 82 - 62;
        self.rec[k] = Rec { vol: 0, per: 0x100, mode: 0 };
        self.restart_rec(hw, k, 0, 0);
        self.write_vp(hw, k);
    }

    fn noise_path(&mut self, hw: &mut Hw, k: usize) {
        self.t += 75;
        self.rec[k].per = (self.er[6] as u16 & 0x1F) * 16 + 0x8E;
        if self.rec[k].mode != 2 {
            self.restart_rec(hw, k, 2, 10);
        }
        self.write_vp(hw, k);
    }

    fn emulate(&mut self, hw: &mut Hw) {
        self.t += 110;
        let en = !self.er[7];
        for i in 0..3 {
            if i > 0 {
                self.t += 82;
            }
            self.t += 37;
            let p = (self.er[2 * i + 1] as u16) << 8 | self.er[2 * i] as u16;
            let v = (self.er[8 + i] & 15) as usize;
            self.rec[i].vol = VT[v];
            let tone = en >> i & 1 != 0;
            let noise = en >> (i + 3) & 1 != 0;
            if tone {
                self.t += 117;
                self.rec[i].per = ((((p & 0xFFF) as u32 * 0x38CC) >> 10) + 1) as u16;
                if noise {
                    self.t += 40;
                    self.nc += 1;
                    self.rec[3].vol = self.rec[3].vol.wrapping_add(self.rec[i].vol);
                }
                self.t += 25;
                if self.rec[i].mode != 1 {
                    self.restart_rec(hw, i, 1, 10);
                }
                self.write_vp(hw, i);
            } else {
                self.t += 15;
                if noise {
                    self.noise_path(hw, i);
                } else {
                    self.disable(hw, i);
                }
            }
        }
        self.t += 37;
        match self.nc {
            0 => {
                self.t += 21;
                if self.rec[3].mode != 0 {
                    self.disable(hw, 3);
                }
            }
            k => {
                self.t += 13;
                match k {
                    2 => {
                        self.t += 25;
                        self.rec[3].vol >>= 1;
                    }
                    3 => {
                        self.t += 171;
                        self.rec[3].vol /= 3;
                    }
                    _ => {}
                }
                self.t += 18;
                self.noise_path(hw, 3);
                self.rec[3].vol = 0;
                self.nc = 0;
            }
        }
    }
}

// ---------------------------------------------------------------------------------

impl Replayer for SpecialFxSt {
    fn format(&self) -> &'static str {
        "Special-FX ST"
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
        (1, self.tb.count, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // the host's end-sound call
        hw.dmacon(0x000F);
        for ch in 0..4 {
            hw.wait(3);
            hw.vol(ch, 0);
        }
        self.songend = false;
        self.start_subsong(n.clamp(1, self.tb.count));
        let due = hw.bus.clock + TICK;
        self.next_at = Some(due + (phase() + LINE - due % LINE) % LINE);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.call = hw.bus.clock;
        self.t = 0;
        let a = |i: usize| self.v[i].flags & F_ACTIVE != 0;
        self.t += if a(0) {
            271
        } else if a(1) {
            292
        } else if a(2) {
            313
        } else {
            430
        };
        if !(a(0) || a(1) || a(2)) {
            self.hostsongend();
            if self.length > 10 {
                self.start_subsong(self.sub);
            }
        }
        self.replay();
        self.er = self.r;
        self.t += 39;
        self.emulate(hw);
        self.at(hw);
    }
}

#[cfg(test)]
#[path = "specialfxst_tests.rs"]
mod tests;
