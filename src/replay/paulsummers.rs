//! Paul Summers (`snk.*`), the Amiga sound driver of Paul Summers and Mike Chilton (Fighting
//! Soccer, Postman Pat, the Everton FC Intelligensia jingles, P47 Thunderbolt).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! and execution traces of the 68k reference player; no original code. A module is a
//! position-independent image: the driver's code first (never run here), then the period table,
//! song table, instrument tables, note streams and samples. The three things the replay needs
//! (song table, period table, subsong count) are found by a bounded scan of the image's code for
//! the address loads, as the specification gives the rule.
//!
//! Four voices, one Paula channel each. A tick steps every voice: a note stream with notes,
//! rests, jumps, calls, loops, an instrument-table select, a volume and a pitch effect. Every
//! register write is placed at the clock of the original's code (static cycle costs; the raster
//! waits of a note start are modelled with the absolute colour clock of the tick's call).
//!
//! Choices where the specification leaves the behaviour open: a positive speed value, stream
//! commands 10 to 13, a zero pitch-effect divisor, a stream or stack that leaves its bounds and
//! a voice that runs more than 100 000 events in one tick end the voice (without a song-end
//! mark), and a positive speed value plays one tick and ends the song.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use std::sync::atomic::{AtomicU64, Ordering};

pub const OPEN: Open = Open { name: "Paul Summers", detect, new };

/// The phase (colour clocks modulo the raster line of 227) of the first tick call; the reference
/// host's differs per module. `u64::MAX` selects the default. A test and measurement knob.
pub static PHASE: AtomicU64 = AtomicU64::new(u64::MAX);

const DEFAULT_PHASE: u64 = 169;
const LINE: u64 = 227;
const DEFAULT_TIMER: u64 = 14187;

// ---------------------------------------------------------------------------------
// Recognition

/// The offset of the 16-bit word `w` at or after `from` (even steps), if any.
fn find_word(d: &[u8], from: usize, w: u16) -> Option<usize> {
    let mut p = from;
    while p + 2 <= d.len() {
        if be16(d, p) == w {
            return Some(p);
        }
        p += 2;
    }
    None
}

fn s16(d: &[u8], o: usize) -> i64 {
    be16(d, o) as i16 as i64
}

/// The pointer stored at `a`: the signed word there plus `a`.
fn ptr_at(d: &[u8], a: usize) -> i64 {
    a as i64 + s16(d, a)
}

/// The driver check of the specification (1.2).
fn detect_code(d: &[u8]) -> bool {
    if d.len() <= 3000 {
        return false;
    }
    let mut p = 650;
    let mut form = 0;
    for _ in 0..10 {
        if be32(d, p) == 0x46FC_2700 {
            form = 1;
            break;
        }
        if be16(d, p) == 0x41FA {
            form = 2;
            break;
        }
        p += 2;
    }
    let mut pos;
    match form {
        1 => {
            pos = p;
            loop {
                if pos + 4 > d.len() || be32(d, pos) == 0 {
                    return false;
                }
                let w = be16(d, pos);
                pos += 2;
                if w != 0x4E73 {
                    continue;
                }
                let w = be16(d, pos);
                pos += 2;
                if w == 0x41FA {
                    break;
                }
            }
        }
        2 => {
            if [4, 10, 16, 22].iter().any(|&o| be32(d, p + o) != 0x117C_0088) {
                return false;
            }
            pos = p + 28;
        }
        _ => return false,
    }
    let Some(q) = find_word(d, pos, 0x41FA) else { return false };
    ptr_at(d, q + 2) == 0
}

/// The tables of a module: song table, period table and the subsong count.
#[derive(Clone, Copy, Debug)]
struct Lay {
    song: usize,
    per: usize,
    subsongs: usize,
}

fn locate(d: &[u8]) -> Option<Lay> {
    if !detect_code(d) {
        return None;
    }
    let p1 = find_word(d, 650, 0x45FA)?;
    let a1 = p1 + 2;
    let target1 = ptr_at(d, a1);
    let pc = find_word(d, a1, 0xC0FC)?;
    let e = pc + 2;
    let mut n0 = be16(d, e.checked_sub(8)?) as i64;
    let song = ptr_at(d, e + 4);
    if target1 >= song {
        n0 = (target1 - song) / 20;
    }
    let pf = find_word(d, e + 4, 0x4480)?;
    let f = pf + 2;
    let per = ptr_at(d, f + 2);
    if song <= 0 || per <= 0 || song as usize >= d.len() || per as usize >= d.len() {
        return None;
    }
    let subsongs = (n0 - 1).clamp(0, 15) as usize;
    Some(Lay { song: song as usize, per: per as usize, subsongs })
}

fn detect(d: &[u8]) -> bool {
    locate(d).map_or(false, |l| l.subsongs >= 1)
}

// ---------------------------------------------------------------------------------
// State

#[derive(Clone, Copy)]
enum Rec {
    Call(usize),
    Loop(u16, usize),
}

impl Rec {
    fn bytes(&self) -> usize {
        match self {
            Rec::Call(_) => 4,
            Rec::Loop(..) => 8,
        }
    }
}

#[derive(Clone, Default)]
struct Voice {
    ptr: usize,
    stack: Vec<Rec>,
    dur: u16,
    flags: u8,
    hi: u16,
    ins: Option<usize>,
    n: u8,
    div: u8,
    idx: i32,
    step: u8,
    edel: u16,
    env: usize,
}

impl Voice {
    fn stack_bytes(&self) -> usize {
        self.stack.iter().map(Rec::bytes).sum()
    }
}

pub struct PaulSummers {
    d: Vec<u8>,
    base: u32,
    silent: u32,
    lay: Lay,
    sub: usize,
    v: [Voice; 4],
    speed: i8,
    speedcnt: u8,
    songword: u16,
    marks: [bool; 4],
    carry: u32,
    timer: u64,
    songend: bool,
    stopped: bool,
    // timing
    call: u64,
    t: u64,
    next_at: Option<u64>,
    /// Colour clocks added to the first period to reach the wanted raster phase.
    first_due: u64,
}

const F_PITCH: u8 = 1;
const F_REST: u8 = 2;
const F_RUN: u8 = 0x20;
const F_ENDSENT: u8 = 0x40;
const F_ENDED: u8 = 0x80;

const SETALL: [u64; 4] = [107, 138, 169, 177];
const VOLB: [u64; 4] = [169, 192, 215, 205];
const CHAIN: [u64; 4] = [45, 72, 95, 108];
const EVENT_CAP: usize = 100_000;
const STACK_CAP: usize = 128;

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = locate(d)?;
    if lay.subsongs < 1 {
        return None;
    }
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    let mut img = d.to_vec();
    if img.len() > 14000 && be16(&img, 13836) == 0xF87F {
        img[13836] = 0;
        img[13837] = 0;
        m.bus.write_bytes(base + 13836, &[0, 0]);
    }
    let mut s = PaulSummers {
        d: img,
        base,
        silent,
        lay,
        sub: 0,
        v: Default::default(),
        speed: 0,
        speedcnt: 0,
        songword: 0,
        marks: [true; 4],
        carry: 0,
        timer: DEFAULT_TIMER,
        songend: false,
        stopped: false,
        call: 0,
        t: 0,
        next_at: None,
        first_due: 0,
    };
    let mut hw = Hw { bus: &mut m.bus };
    s.init(0);
    s.start_sequence(&mut hw);
    Some(Box::new(s))
}

fn phase() -> u64 {
    match PHASE.load(Ordering::Relaxed) {
        u64::MAX => DEFAULT_PHASE,
        p => p % LINE,
    }
}

impl PaulSummers {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn rl(&self, o: usize) -> u32 {
        be32(&self.d, o)
    }

    fn period_clocks(&self) -> u64 {
        5 * (self.timer + 1)
    }

    /// The host's start: the volumes twice, then DMA off (spec 7.1), at the offsets from the first write.
    fn start_sequence(&mut self, hw: &mut Hw) {
        let ts = hw.bus.clock;
        for (i, off) in [0u64, 138, 288, 432, 686, 824, 974, 1118].iter().enumerate() {
            hw.bus.clock = ts + off;
            hw.vol(i & 3, 0);
        }
        hw.bus.clock = ts + 6240;
        hw.dmacon(0x000F);
        // the first tick: one period after this write, plus what puts it on the wanted raster phase
        // (the engine starts the timer at the clock after `new`)
        let due = hw.bus.clock + self.period_clocks();
        self.first_due = (phase() + LINE - due % LINE) % LINE;
    }

    /// Clears the state and loads song table entry `sub + 1` (spec 3.1, without the host writes).
    fn init(&mut self, sub: usize) {
        self.sub = sub;
        self.songend = false;
        self.stopped = false;
        self.v = Default::default();
        for v in self.v.iter_mut() {
            v.flags = 0x88;
        }
        self.marks = [true; 4];
        self.carry = 0;
        self.songword = 0xFF00 + sub as u16 + 1;
        let e = self.lay.song + 20 * (sub + 1);
        self.speed = self.rb(e + 16) as i8;
        self.speedcnt = self.rb(e + 17);
        let timer = be16(&self.d, e + 18) as u64;
        self.timer = if timer != 0 { timer } else { DEFAULT_TIMER };
    }

    // --- the clock

    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.call + self.t / 2;
    }

    fn line(&self) -> u64 {
        ((self.call + self.t / 2) / LINE) & 0xFF
    }

    /// The raster wait of a note start: twice nine changes of the line number (spec 5.5).
    fn raster(&mut self) {
        self.t += 28;
        for _ in 0..9 {
            self.t += 16;
            let first = self.line();
            loop {
                self.t += 16;
                let l = self.line();
                self.t += 9;
                if l != first {
                    break;
                }
            }
            self.t += 10;
        }
        self.t += 44;
    }

    fn w_dma(&mut self, hw: &mut Hw, n: u16) {
        self.t += 16;
        self.at(hw);
        hw.dmacon(n);
    }

    // --- stream helpers

    /// The period table entry at index `i`.
    fn entry(&self, i: i64) -> u32 {
        let o = self.lay.per as i64 - 4 * i;
        if o < 0 {
            0
        } else {
            self.rl(o as usize)
        }
    }

    fn end_voice_fault(&mut self, v: usize) {
        self.v[v].flags |= F_ENDED;
    }

    fn push(&mut self, v: usize, r: Rec) -> bool {
        if self.v[v].stack_bytes() + r.bytes() > STACK_CAP {
            return false;
        }
        self.v[v].stack.push(r);
        true
    }

    // --- the tick

    fn tick_inner(&mut self, hw: &mut Hw) {
        self.call = hw.bus.clock;
        self.t = 242 + 113;
        if self.speed > 0 {
            // undefined in the original (it fails after one tick): play that one tick, then stop
            if self.stopped {
                return;
            }
            self.stopped = true;
            self.songend = true;
            // the one observed tick starts its first write 32 colour clocks after the call of a
            // zero-speed tick would (the cost of the speed test, fitted)
            self.t += 64;
        } else if self.speed < 0 {
            self.t += 30;
            if self.speedcnt == 0 {
                self.speedcnt = self.speed as u8;
                self.t += 26;
                self.at(hw);
                return;
            }
            self.speedcnt = self.speedcnt.wrapping_add(1);
            self.t += 26;
        }
        self.carry = 0;
        self.t += 39;
        if self.songword & 0x8000 != 0 {
            self.song_start(hw);
        }
        self.t += 24;
        for v in 0..4 {
            self.voice(v, hw);
            self.t += 39;
        }
        self.at(hw);
    }

    fn song_start(&mut self, hw: &mut Hw) {
        self.t += 20;
        self.at(hw);
        hw.dmacon(0x000F);
        self.songword &= 0xF;
        let e = self.lay.song + 20 * self.songword as usize;
        self.t += 130;
        for v in 0..4 {
            self.v[v].flags = 0x88;
            self.v[v].stack.clear();
            let val = self.rl(e + 4 * v) as i32;
            let off = self.lay.song as i64 + val as i64;
            if val == 0 {
                self.t += 91;
            } else if val == 0x1EF8 {
                self.t += 114;
            } else if off <= 0 || off as usize >= self.d.len() {
                self.t += 169;
            } else {
                self.t += 229;
                let vo = &mut self.v[v];
                vo.ptr = off as usize;
                vo.flags = F_PITCH;
                vo.dur = 0;
                vo.hi = 0;
            }
        }
        for c in 0..4 {
            self.t += 20;
            self.at(hw);
            hw.vol(c, 0);
        }
        self.t += 20;
        self.at(hw);
        hw.dmacon(0x000F);
    }

    /// One voice's slot of the pass.
    fn voice(&mut self, v: usize, hw: &mut Hw) {
        let m = 1u16 << v;
        self.t += 25;
        if self.v[v].flags & F_ENDED != 0 {
            self.t += 31;
            if self.v[v].flags & F_ENDSENT == 0 {
                self.v[v].flags |= F_ENDSENT;
                self.w_dma(hw, m);
                self.t += 10;
            }
            return;
        }
        self.t += 27;
        if self.v[v].flags & F_RUN == 0 && !self.fetch(v, hw) {
            return;
        }
        // the duration step; a zero duration goes back to the fetch
        let mut guard = 0;
        loop {
            self.t += 23;
            if self.v[v].dur == 0 {
                guard += 1;
                if guard > EVENT_CAP || !self.fetch(v, hw) {
                    if guard > EVENT_CAP {
                        self.end_voice_fault(v);
                    }
                    return;
                }
                continue;
            }
            break;
        }
        self.t += 25;
        self.v[v].dur -= 1;
        if self.v[v].dur == 0 {
            self.t += 20;
            self.v[v].flags &= 0xDC;
            self.w_dma(hw, m);
        }
        self.t += 27;
        if self.v[v].flags & F_PITCH != 0 {
            self.pitch(v, hw);
        }
    }

    /// The fetch (spec 3.4 step 3): events until one sets a duration. False when the voice ended.
    fn fetch(&mut self, v: usize, hw: &mut Hw) -> bool {
        let m = 1u16 << v;
        self.t += 27;
        if self.v[v].flags & F_REST == 0 {
            self.w_dma(hw, m);
        }
        self.t += 60;
        self.v[v].flags &= !(F_PITCH | F_REST | F_RUN);
        for _ in 0..EVENT_CAP {
            let p = self.v[v].ptr;
            if p >= self.d.len() {
                self.end_voice_fault(v);
                return false;
            }
            let b = self.d[p];
            self.v[v].ptr = p + 1;
            self.t += 39;
            if b >= 0x80 {
                self.t += 62;
                match self.command(v, b, p, hw) {
                    Cmd::Next => continue,
                    Cmd::Duration => return true,
                    Cmd::Ended => return false,
                }
            } else if self.note(v, b, hw) {
                return true;
            }
        }
        self.end_voice_fault(v);
        false
    }

    fn set_duration(&mut self, v: usize) {
        let p = self.v[v].ptr;
        let b = self.rb(p) as u16;
        let vo = &mut self.v[v];
        vo.dur = vo.hi | b;
        vo.hi = 0;
        vo.ptr = p + 1;
        vo.flags |= F_RUN;
        self.t += 80;
    }

    fn jump_to(&mut self, v: usize, operand: usize) -> bool {
        let t = operand as i64 + self.rl(operand) as i32 as i64;
        if t < 0 || t as usize >= self.d.len() {
            self.end_voice_fault(v);
            return false;
        }
        self.v[v].ptr = t as usize;
        true
    }

    /// A command byte `b` read at `at`.
    fn command(&mut self, v: usize, b: u8, at: usize, hw: &mut Hw) -> Cmd {
        let m = 1u16 << v;
        let op = at + 1;
        match b & 15 {
            0 => {
                self.v[v].flags |= F_REST;
                self.t += 20;
                self.w_dma(hw, m);
                self.t += 10;
                self.set_duration(v);
                Cmd::Duration
            }
            1 => {
                self.t += 142;
                if self.jump_to(v, op) {
                    Cmd::Next
                } else {
                    Cmd::Ended
                }
            }
            2 => {
                self.t += 182;
                if !self.push(v, Rec::Call(op + 4)) {
                    self.end_voice_fault(v);
                    return Cmd::Ended;
                }
                if self.jump_to(v, op) {
                    Cmd::Next
                } else {
                    Cmd::Ended
                }
            }
            3 => {
                self.t += 74;
                match self.v[v].stack.pop() {
                    Some(Rec::Call(p)) => {
                        self.v[v].ptr = p;
                        Cmd::Next
                    }
                    _ => {
                        self.end_voice_fault(v);
                        Cmd::Ended
                    }
                }
            }
            4 => {
                self.t += 126;
                let count = be16(&self.d, op);
                if !self.push(v, Rec::Loop(count, op + 2)) {
                    self.end_voice_fault(v);
                    return Cmd::Ended;
                }
                self.v[v].ptr = op + 2;
                Cmd::Next
            }
            5 => match self.v[v].stack.pop() {
                Some(Rec::Loop(c, start)) => {
                    let c = c.wrapping_sub(1);
                    if c == 0 {
                        self.t += 109;
                    } else {
                        self.t += 139;
                        self.push(v, Rec::Loop(c, start));
                        self.v[v].ptr = start;
                    }
                    Cmd::Next
                }
                _ => {
                    self.end_voice_fault(v);
                    Cmd::Ended
                }
            },
            6 => {
                self.t += 158;
                let t = op as i64 + self.rl(op) as i32 as i64;
                self.v[v].ins = (t >= 0).then_some(t as usize);
                self.v[v].ptr = op + 4;
                Cmd::Next
            }
            7 => {
                self.t += 68;
                self.v[v].hi = (self.rb(op) as u16) << 8;
                self.v[v].ptr = op + 1;
                Cmd::Next
            }
            8 => {
                let vo = &mut self.v[v];
                if vo.flags & F_PITCH == 0 {
                    self.t += 131;
                    vo.flags |= F_PITCH;
                    vo.step = self.d.get(op).copied().unwrap_or(0);
                    vo.edel = vo.hi | self.d.get(op + 1).copied().unwrap_or(0) as u16;
                    vo.hi = 0;
                    vo.env = op + 2;
                } else {
                    self.t += 59;
                }
                self.v[v].ptr = op + 2;
                Cmd::Next
            }
            9 => {
                let n = self.rb(op);
                self.v[v].ptr = op + 1;
                self.v[v].n = n;
                if n == 0 {
                    self.t += 59;
                } else {
                    self.t += 203;
                    let dividend = (self.carry & 0xFFFF_0000) | 0x40;
                    let q = dividend / n as u32;
                    if q > 0xFFFF {
                        self.v[v].div = 0x40;
                        self.carry = (self.carry & 0xFFFF_0000) | 0x40;
                    } else {
                        let r = dividend % n as u32;
                        self.v[v].div = q as u8;
                        self.carry = (r << 16) | q;
                    }
                }
                Cmd::Next
            }
            14 => {
                self.t += 30 + VOLB[v];
                let vol = (self.rb(op) & 0x7F) as u16;
                self.v[v].ptr = op + 1;
                self.t += 12;
                self.at(hw);
                hw.vol(v, vol);
                self.t += 28 + 26;
                Cmd::Next
            }
            15 => {
                self.t += 18 + 32 + CHAIN[v] + 21;
                self.marks[v] = false;
                if self.marks.iter().all(|&m| !m) {
                    self.t += 68 + 16;
                    self.marks = [true; 4];
                    self.songend = true;
                }
                self.t += 44 + 20 + 10;
                self.v[v].flags |= F_ENDED;
                Cmd::Ended
            }
            _ => {
                // 10 to 13: undefined; the voice ends
                self.end_voice_fault(v);
                Cmd::Ended
            }
        }
    }

    /// A note event; true when the voice has its duration (always).
    fn note(&mut self, v: usize, note: u8, hw: &mut Hw) -> bool {
        let m = 1u16 << v;
        self.t += 39;
        let ins = self.v[v].ins;
        let mut found = None;
        if let Some(i) = ins {
            let n = be16(&self.d, i) as usize;
            if n != 0 {
                self.t += 4;
            }
            for k in 0..n {
                let e = i + 2 + 14 * k;
                let (lo, hi) = (s16(&self.d, e + 10), s16(&self.d, e + 12));
                let nt = note as i64;
                if nt < lo {
                    self.t += 41;
                } else if nt > hi {
                    self.t += 62;
                } else {
                    self.t += 44;
                    found = Some(e);
                    break;
                }
            }
        }
        let Some(e) = found else {
            self.t += 10;
            self.set_duration(v);
            return true;
        };
        let base = be16(&self.d, e + 8) as i32;
        let idx = note as i32 - base;
        self.v[v].idx = idx;
        let off = e as i64 + self.rl(e) as i32 as i64;
        let len = be16(&self.d, e + 6);
        let p = self.entry(idx as i64);
        let period = (p >> 16) as u16;
        self.t += 212;
        self.t += 12;
        self.at(hw);
        hw.loc(v, (self.base as i64 + off) as u32);
        self.t += 12;
        self.at(hw);
        hw.len(v, len);
        self.t += 12;
        self.at(hw);
        hw.per(v, period);
        self.t += 18 + SETALL[v] + 18;
        self.raster();
        self.t += 12;
        self.w_dma(hw, 0x8200 | m);
        self.t += 18;
        self.raster();
        self.t += 53;
        let one_shot = v != 3 || self.stack0_bit6_clear();
        if one_shot {
            self.t += 12 + 12;
            self.at(hw);
            hw.loc(v, self.silent);
            self.t += 16;
            self.at(hw);
            hw.len(v, 1);
        }
        self.t += 12;
        self.carry = p.rotate_left(16);
        self.set_duration(v);
        true
    }

    /// Bit 6 of the low byte of voice 0's stack pointer is clear (spec 3.10).
    fn stack0_bit6_clear(&self) -> bool {
        let sp = (0x1Eu32.wrapping_sub(self.v[0].stack_bytes() as u32)) & 0xFF;
        sp & 0x40 == 0
    }

    // --- the pitch effect (spec 3.7)

    fn pitch(&mut self, v: usize, hw: &mut Hw) {
        self.t += 21;
        if self.v[v].edel != 0 {
            self.t += 27;
            self.v[v].edel -= 1;
            if self.v[v].edel != 0 {
                return;
            }
        }
        self.t += 52;
        let (n, div) = (self.v[v].n, self.v[v].div);
        let mut t = self.v[v].idx as i64 - n as i64;
        let mut r = self.v[v].step;
        let mut guard = 0;
        loop {
            self.t += 21;
            if (r as i8) < (div as i8) {
                break;
            }
            guard += 1;
            if guard > 512 {
                // a zero divisor never ends in the original: the effect stops
                self.v[v].flags &= !F_PITCH;
                return;
            }
            self.t += 30;
            r = r.wrapping_sub(div);
            t += 1;
        }
        self.t += 32 + (n as u64 + 1) * 14 + 40 + 90 + 192;
        let w = (n as u32 * r as u32) & 0xFFFF;
        let q = self.entry(t);
        let qn = self.entry(t - 1);
        let diff = qn.wrapping_sub(q);
        let (hiw, low) = (diff >> 16, diff & 0xFFFF);
        let x = ((((hiw * w) & 0xFFFF) << 16) | ((low * w) & 0xFFFF)) >> 6;
        let z = q.wrapping_sub(x);
        self.t += 12;
        self.at(hw);
        hw.per(v, (z >> 16) as u16);
        self.t += 16;
        self.carry = z.rotate_left(16);
        loop {
            self.t += 29;
            let env = self.v[v].env;
            let b = self.rb(env);
            if b == 0xF7 {
                self.t += 52;
                self.v[v].hi = (self.rb(env + 1) as u16) << 8;
                self.v[v].env = env + 2;
                continue;
            }
            self.t += 17;
            if b == 0xF8 {
                self.t += 90;
                let vo = &mut self.v[v];
                vo.step = self.d.get(env + 1).copied().unwrap_or(0);
                vo.edel = vo.hi | self.d.get(env + 2).copied().unwrap_or(0) as u16;
                vo.hi = 0;
                vo.env = env + 3;
            } else {
                self.t += 20;
                self.v[v].flags &= !F_PITCH;
            }
            break;
        }
    }
}

enum Cmd {
    Next,
    Duration,
    Ended,
}

impl Replayer for PaulSummers {
    fn format(&self) -> &'static str {
        "Paul Summers"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        self.period_clocks()
    }

    fn timer_delay(&self) -> u64 {
        // the engine counts one period from the clock after `new`; the first call falls on `first_due`
        self.first_due
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.subsongs as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.lay.subsongs - 1);
        self.init(n);
        let due = hw.bus.clock + self.period_clocks();
        self.next_at = Some(due + (phase() + LINE - due % LINE) % LINE);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }
}

#[cfg(test)]
#[path = "paulsummers_tests.rs"]
mod tests;
