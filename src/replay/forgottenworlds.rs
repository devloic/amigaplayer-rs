//! Forgotten Worlds (`fw.*`, module header `FWMP`): the music routine of the Amiga game.
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A module holds only data: the four channels' event
//! streams, a pattern table, an instrument table and a waveform directory with the 8-bit samples.
//! Four channels, one Paula channel each, 50 Hz. A tick sets the DMA bits, then serves the channels
//! in order: a running channel steps its volume envelope (one AUDxVOL write), a channel whose
//! duration has run out reads events (rest, call, return, restart, instrument select, note) and a
//! note restarts the Paula channel through the sequence of the specification (DMA off, period 1,
//! two manual data words each waiting for the channel's interrupt flag, sample, DMA on, wait for
//! the flag, loop part).
//!
//! The waits poll Paula's interrupt flag on the engine's Paula model, at the polling instants of
//! the specification, so they last as long as the model makes them; each poll is bounded (the
//! reference hangs where its Paula never raises the flag: a note on channel 0 in the first tick of
//! three of the five tunes), after which the sequence goes on.
//!
//! Every register write is placed at the cycle of the specification's cost model, counted from the
//! tick's DMACON write; the tick schedule is the specification's (interrupt every 70,940 colour
//! clocks, 185 before the tick's first write, a late tick runs as soon as the previous one ends).
//!
//! Choices where the specification leaves the behaviour open: command 0x80 ends the tick at once and
//! the channel (its envelope runs on for the 255 ticks its counter wraps through, then it is silent);
//! the song end is reported when every channel has ended; a note with no instrument selected writes
//! nothing; a stage "invalid" (channel 1 before its first note) does nothing.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Forgotten Worlds", detect, new };

const LINE: u64 = 227;
const TICK: u64 = 70940;
/// Colour clocks from the interrupt to the tick's DMACON write.
const ENTRY: u64 = 185;
/// Colour clocks from the start phase (the clock `new` is called at: Paula just reset, the raster
/// line at the engine's start phase) to the first song-initialisation write of the reference run.
const START_DELAY: u64 = 680;
/// The song initialisation (spec 7.1) takes this long from the first to the last write.
const INIT_SPAN: u64 = 182;
/// The first tick's DMACON write follows the first initialisation write by this much.
const FIRST_TICK: u64 = 71249;

/// Notes 0..45 in colour clocks.
const PERIODS: [u16; 46] = [
    1712, 1616, 1520, 1440, 1360, 1280, 1208, 1144, 1080, 1016, 960, 904, 856, 808, 760, 720, 680, 640, 604, 572, 540, 508,
    480, 452, 428, 404, 380, 360, 340, 320, 302, 286, 270, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127,
];

// Cycle costs (spec 7.4); two cycles make a colour clock.
const C_PRO: u64 = 33;
const C_EPI: [u64; 4] = [42, 42, 42, 46];
const C_RUN_PLAIN: u64 = 74;
const C_RUN_ENV: u64 = 105;
const C_VOL_TAIL: u64 = 144;
const C_AFTER_VOL: u64 = 10;
const C_HEAD: u64 = 48;
const C_NOTE: u64 = 134;
const C_NOTE_END: u64 = 40;
const C_RESTART: u64 = 83;
const C_INSTR: u64 = 271;
const C_REST: u64 = 92;
const C_CALL: u64 = 130;
const C_RETURN: u64 = 86;
/// Longest wait of the three short polls (the old period is at most 1712) and slack, in colour clocks.
const WAIT_CAP: u64 = 4000;

fn detect(d: &[u8]) -> bool {
    if d.len() < 20 || &d[..4] != b"FWMP" {
        return false;
    }
    let (a, b, c) = (be32(d, 4) as usize, be32(d, 8) as usize, be32(d, 12) as usize);
    let n = d.len();
    16 + 8 <= n && 16usize.checked_add(a).map_or(false, |x| x <= n) && 16usize.checked_add(b).map_or(false, |x| x <= n)
        && 16usize.checked_add(c).map_or(false, |x| x <= n) && a <= b && b <= c
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Attack,
    Decay,
    Hold,
    Release,
    Stopped,
    Invalid,
}

#[derive(Clone)]
struct Chan {
    start: usize,
    ptr: usize,
    cnt: u8,
    ret: usize,
    /// Offset of the current instrument's table entry.
    ins: Option<usize>,
    wave: usize,
    wlen: u16,
    loop_at: usize,
    llen: u16,
    level: u8,
    stage: Stage,
    hold: u8,
    ended: bool,
    /// The wrapped counter has run through; nothing further is read.
    dead: bool,
}

pub struct ForgottenWorlds {
    d: Vec<u8>,
    base: u32,
    pt: usize,
    it: usize,
    wd: usize,
    silent: u32,
    ch: [Chan; 4],
    songend: bool,
    /// The interrupt time of the next tick (colour clocks, absolute).
    due: u64,
    /// The clock of the first tick call.
    first_n: u64,
    started: bool,
    /// Tick-local: the colour clock of the DMACON write and the cycle count since.
    td: u64,
    cy: u64,
    /// The end of the last tick, in cycles (2 per colour clock).
    end_cy: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    m.bus.write_bytes(silent, &[0; 16]);
    let (a, b, c) = (be32(d, 4) as usize, be32(d, 8) as usize, be32(d, 12) as usize);
    let mut chs: Vec<Chan> = Vec::new();
    for i in 0..4 {
        let start = 16 + be16(d, 16 + 2 * i) as usize;
        let (level, stage, hold) = match i {
            0 => (0x30, Stage::Hold, 45),
            1 => (0x40, Stage::Invalid, 50),
            _ => (0x28, Stage::Attack, 0),
        };
        chs.push(Chan {
            start,
            ptr: start,
            cnt: 1,
            ret: 0,
            ins: None,
            wave: 0,
            wlen: 0,
            loop_at: 0,
            llen: 0,
            level,
            stage,
            hold,
            ended: false,
            dead: false,
        });
    }
    let ch: [Chan; 4] = chs.try_into().ok()?;
    // the host's start: the reference's first write follows Paula's reset by a fixed time
    let s = m.bus.clock + START_DELAY;
    let mut r = Replay {
        d: d.to_vec(),
        base,
        pt: 16 + a,
        it: 16 + b,
        wd: 16 + c,
        silent,
        ch,
        songend: false,
        due: s + FIRST_TICK - ENTRY,
        first_n: 0,
        started: false,
        td: 0,
        cy: 0,
        end_cy: 0,
    };
    let mut hw = Hw { bus: &mut m.bus };
    r.init_writes(&mut hw, s);
    Some(Box::new(r))
}

type Replay = ForgottenWorlds;

impl Replay {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// Song initialisation (spec 7.1): the silence buffer on every channel, then the stray writes.
    fn init_writes(&mut self, hw: &mut Hw, s: u64) {
        for ch in 0..4usize {
            let t = s + 39 * ch as u64;
            hw.bus.clock = t;
            hw.loc(ch, self.silent);
            hw.bus.clock = t + 8;
            hw.len(ch, 2);
            hw.bus.clock = t + 16;
            hw.per(ch, 0x7C);
            hw.bus.clock = t + 24;
            hw.vol(ch, 0);
        }
        for (i, (a, v)) in [(0xDFF17Eu32, 0x00FFu16), (0xDFF176, 0x000F), (0xDFF17C, 0x0780), (0xDFF17A, 0x0780)].iter().enumerate() {
            hw.bus.clock = s + 158 + 8 * i as u64;
            hw.bus.write_word(*a, *v);
        }
        hw.bus.clock = s + INIT_SPAN;
        // the engine starts the timer at the clock after `new`
        let n = hw.bus.clock;
        self.first_n = self.due.saturating_sub(n);
    }

    /// Moves the clock to the current cycle.
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.td + self.cy / 2;
    }

    fn adv(&mut self, cycles: u64) {
        self.cy += cycles;
    }

    /// A register write `cycles` after the previous event.
    fn w(&mut self, hw: &mut Hw, cycles: u64, a: u32, v: u16) {
        self.cy += cycles;
        self.at(hw);
        hw.bus.write_word(a, v);
    }

    /// Waits for the channel's interrupt flag: polls at 13 + 25 k cycles after the last write; the
    /// flag counts if it was raised strictly before the sample. Returns the number of failed polls.
    /// Gives up (the sequence goes on) after the colour clocks `cap`.
    fn poll(&mut self, hw: &mut Hw, ch: usize, cap: u64) -> u64 {
        let bit = 0x80u16 << ch;
        let limit = self.td + (self.cy / 2) + cap;
        let mut k = 0u64;
        loop {
            let s = self.cy + 13 + 25 * k;
            let t = self.td + (s - 1) / 2;
            if t >= hw.bus.paula.now() {
                hw.bus.paula.sync(t, &hw.bus.ram);
            }
            if hw.bus.paula.intreq & bit != 0 || t > limit {
                return k;
            }
            k += 1;
        }
    }

    // ---- event streams

    fn pattern_start(&self, p: i8) -> usize {
        let w = be16(&self.d, (self.pt as i64 + 2 * p as i64).max(0) as usize) as i16 as i64;
        (self.pt as i64 + w).max(0) as usize
    }

    fn select(&mut self, c: usize, i: i8) {
        let ins = (self.it as i64 + 8 * i as i64).max(0) as usize;
        let b0 = self.rb(ins) as i32;
        let d3 = ((((i as i32) << 3) & 0xFF00) | b0) as u16 as i16 as i64;
        let e = (self.wd as i64 + d3).max(0) as usize;
        let ch = &mut self.ch[c];
        ch.ins = Some(ins);
        let wave = self.wd + be16(&self.d, e) as usize;
        let wlen = be16(&self.d, e + 2);
        let loop_at = wave + be16(&self.d, e + 4) as usize;
        let llen = be16(&self.d, e + 6);
        let ch = &mut self.ch[c];
        ch.wave = wave;
        ch.wlen = wlen;
        ch.loop_at = loop_at;
        ch.llen = llen;
    }

    // ---- the tick

    fn envelope(&mut self, c: usize) -> (u64, u8) {
        let ins = self.ch[c].ins.unwrap_or(0);
        let (sus, att, dec, hold, rel) = (self.rb(ins + 1), self.rb(ins + 2), self.rb(ins + 3), self.rb(ins + 4), self.rb(ins + 5));
        let ch = &mut self.ch[c];
        let mut l = ch.level;
        let cost;
        match ch.stage {
            Stage::Attack => {
                l = l.wrapping_add(att);
                if l >= 0x40 {
                    l = 0x40;
                    ch.stage = Stage::Decay;
                    cost = 78;
                } else {
                    cost = 46;
                }
            }
            Stage::Decay => {
                l = l.wrapping_add(dec);
                if l <= sus {
                    l = sus;
                    ch.hold = hold;
                    ch.stage = Stage::Hold;
                    cost = 106;
                } else {
                    cost = 50;
                }
            }
            Stage::Hold => {
                ch.hold = ch.hold.wrapping_sub(1);
                if ch.hold == 0 {
                    ch.stage = Stage::Release;
                    cost = 66;
                } else {
                    cost = 42;
                }
            }
            Stage::Release => {
                l = l.wrapping_add(rel);
                if l & 0x80 != 0 {
                    l = 0;
                    ch.stage = Stage::Stopped;
                    cost = 66;
                } else {
                    cost = 38;
                }
            }
            Stage::Stopped | Stage::Invalid => cost = 17,
        }
        ch.level = l;
        (cost, l)
    }

    fn turn(&mut self, c: usize, hw: &mut Hw) -> bool {
        let reg = 0xDFF0A0 + 0x10 * c as u32;
        if self.ch[c].dead {
            self.adv(C_RUN_PLAIN);
            return true;
        }
        self.ch[c].cnt = self.ch[c].cnt.wrapping_sub(1);
        if self.ch[c].cnt != 0 {
            let has_env = self.ch[c].ins.map_or(false, |i| self.rb(i + 4) != 0);
            if !has_env {
                self.adv(C_RUN_PLAIN);
            } else {
                self.adv(C_RUN_ENV);
                if self.ch[c].stage != Stage::Invalid {
                    let (cost, l) = self.envelope(c);
                    self.adv(cost);
                    self.w(hw, C_VOL_TAIL, reg + 8, l as u16);
                } else {
                    self.adv(17);
                    self.adv(C_VOL_TAIL);
                }
                self.adv(C_AFTER_VOL);
            }
            return true;
        }
        if self.ch[c].ended {
            // the wrapped counter has run through: nothing is read past the end
            self.ch[c].dead = true;
            self.adv(C_RUN_PLAIN);
            return true;
        }
        self.adv(C_HEAD);
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 100_000 {
                self.ch[c].ended = true;
                self.ch[c].dead = true;
                return true;
            }
            let b = self.rb(self.ch[c].ptr);
            self.ch[c].ptr += 1;
            if b < 0x80 {
                let dur = self.rb(self.ch[c].ptr);
                self.ch[c].ptr += 1;
                self.ch[c].cnt = dur;
                self.adv(C_NOTE);
                self.note(c, b, hw);
                self.adv(C_NOTE_END);
                return true;
            }
            match b {
                0x80 => {
                    self.ch[c].ended = true;
                    self.ch[c].cnt = 0;
                    if self.ch.iter().all(|x| x.ended) {
                        self.songend = true;
                    }
                    return false;
                }
                0x82 => {
                    self.ch[c].ptr = self.ch[c].start;
                    self.adv(C_RESTART);
                }
                0x84 => {
                    self.adv(C_INSTR);
                    let i = self.rb(self.ch[c].ptr) as i8;
                    self.ch[c].ptr += 1;
                    self.select(c, i);
                }
                0x86 => {
                    self.ch[c].cnt = self.rb(self.ch[c].ptr);
                    self.ch[c].ptr += 1;
                    self.adv(C_REST);
                    return true;
                }
                0x88 => {
                    self.adv(C_CALL);
                    if self.ch[c].ret == 0 {
                        // the channel's first call costs 3 cycles more (measured, first tick of a tune)
                        self.adv(3);
                    }
                    let p = self.rb(self.ch[c].ptr) as i8;
                    self.ch[c].ptr += 1;
                    self.ch[c].ret = self.ch[c].ptr;
                    self.ch[c].ptr = self.pattern_start(p);
                }
                0x8A => {
                    self.ch[c].ptr = self.ch[c].ret;
                    self.adv(C_RETURN);
                }
                _ => {
                    // undefined commands: the channel ends
                    self.ch[c].ended = true;
                    self.ch[c].dead = true;
                    return true;
                }
            }
        }
    }

    /// The note start (spec 5).
    fn note(&mut self, c: usize, key: u8, hw: &mut Hw) {
        let Some(ins) = self.ch[c].ins else {
            return;
        };
        let level = self.rb(ins + 2);
        self.ch[c].level = level;
        self.ch[c].stage = Stage::Attack;
        let period = PERIODS.get(key as usize).copied().unwrap_or(0);
        let reg = 0xDFF0A0 + 0x10 * c as u32;
        let (ib, bit) = (0x80u16 << c, 1u16 << c);
        let (wave, wlen, loop_at, llen) = (self.ch[c].wave, self.ch[c].wlen, self.ch[c].loop_at, self.ch[c].llen);
        self.w(hw, 12, 0xDFF09C, ib);
        self.w(hw, 12, 0xDFF096, bit);
        self.w(hw, 16, reg + 6, 1);
        self.w(hw, 16, reg + 8, 0);
        let k = self.poll(hw, c, WAIT_CAP);
        self.w(hw, 25 * (k + 1) + 12, 0xDFF09C, ib);
        self.w(hw, 16, reg + 0xA, 0);
        let k = self.poll(hw, c, WAIT_CAP);
        self.w(hw, 25 * (k + 1) + 16, reg + 0xA, 0);
        let k = self.poll(hw, c, WAIT_CAP);
        self.cy += 25 * (k + 1) + 28;
        self.at(hw);
        hw.loc(c, self.base + wave as u32);
        self.w(hw, 20, reg + 4, wlen);
        self.w(hw, 20, reg + 6, period);
        self.w(hw, 168, reg + 8, level as u16);
        self.w(hw, 24, 0xDFF096, 0x8000 | bit);
        self.w(hw, 26, 0xDFF09C, ib);
        // the flag rises at the first raster line start after the DMA-on write; if that comes
        // before the clear, the wait lasts until the end of the first pass
        let first_pass = 2 * period.max(1) as u64 * if wlen == 0 { 65536 } else { wlen as u64 };
        let cap = 2 * LINE + first_pass + 1000;
        let k = self.poll(hw, c, cap);
        self.w(hw, 25 * (k + 1) + 12, 0xDFF09C, ib);
        self.cy += 28;
        self.at(hw);
        hw.loc(c, self.base + loop_at as u32);
        self.w(hw, 20, reg + 4, llen);
    }
}

impl Replayer for Replay {
    fn format(&self) -> &'static str {
        "Forgotten Worlds"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        if self.started {
            TICK
        } else {
            self.first_n
        }
    }

    fn next_tick_at(&self) -> Option<u64> {
        Some(self.due)
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.started = true;
        self.due += TICK;
        self.td = hw.bus.clock + ENTRY;
        self.cy = 0;
        self.at(hw);
        hw.dmacon(0x800F);
        self.cy = C_PRO;
        for c in 0..4 {
            if !self.turn(c, hw) {
                break;
            }
            self.adv(C_EPI[c]);
        }
        self.end_cy = 2 * self.td + self.cy;
        hw.bus.clock = (self.td + self.cy / 2).max(hw.bus.clock);
    }
}

#[cfg(test)]
#[path = "forgottenworlds_tests.rs"]
mod tests;
