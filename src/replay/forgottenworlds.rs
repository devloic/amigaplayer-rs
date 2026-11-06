//! Forgotten Worlds game music (module header `FWMP`, files `fw.*`).
//!
//! Clean-room, written from a behaviour specification and black-box runs of the reference
//! player's register-write logs; no original code, nothing embedded (the 46-entry period
//! table is the specification's). The module holds only data: four channel event streams,
//! a pattern table, an instrument table and a waveform directory with 8-bit samples.
//!
//! Four voices at 50 Hz: notes, rests, pattern call and return, restart, instrument select
//! and an attack / decay / hold / release envelope that writes only the volume register.
//! A note start restarts the Paula channel through the specified sequence (DMA off, period
//! 1, two manual data words, sample, DMA on, loop part), each wait polling the interrupt
//! request flag of this engine's own Paula at the specification's polling instants. Every
//! write is placed at the cycle the specification's cost model gives, counted from the
//! tick's DMACON write, and ticks follow the specified schedule (a late tick runs as soon
//! as the previous one is over).
//!
//! Choices where the specification leaves it open (all safe):
//! * the polls are bounded: a wait whose flag never rises (the reference hangs in the first
//!   tick of three of the tunes, its Paula never raising the flag after a stop following one
//!   word) is given up after a bounded time and the sequence goes on;
//! * command 0x80 ends the tick; its counter stays 0, so the envelope runs on through the
//!   wrapped counter (255 ticks), and the channel then reads the same command again (the
//!   reference's logs show exactly that: the specification says the pointer moves past the
//!   command, but the data after it is never read). The song end is reported when all four
//!   channels have read 0x80;
//! * commands above 0x8A end the channel for good (no more reads) but let the tick go on;
//! * a note with no instrument selected sets its duration and writes nothing;
//! * the invalid envelope stage of channel 1 (never exercised) behaves as the stopped stage;
//! * reads outside the module give zero; a channel that reads more than a bounded number of
//!   events in one turn ends.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Forgotten Worlds", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// Colour clocks from the interrupt to the tick's DMACON write.
const IRQ_LEAD: u64 = 185;
/// Colour clocks from the first initialisation write to the first tick's DMACON write.
const FIRST_TICK: u64 = 71249;
/// Colour clocks from the (fixed-phase) start of a run to the first initialisation write: the
/// reference's own start-up takes this long. This puts the writes on the reference's raster
/// line phase and on its output sample grid.
const START_DELAY: u64 = 680;
/// Colour clocks from the last initialisation write to the song's start (`Paula::align_to_start`).
const ORIGIN_LAG: u64 = 22;
/// Events one channel may read in one turn before it is ended (bad data).
const EVENT_CAP: u32 = 5000;
/// A wait on the audio interrupt flag of the register setup gives up after this many colour clocks.
const SETUP_WAIT_CAP: u64 = 4000;
/// The wait after the DMA start gives up after this many colour clocks (a sample's first pass
/// is at most 65535 words at period 65535: far longer; the cap only guards a hang).
const START_WAIT_CAP: u64 = 12_000_000;

const PERIODS: [u16; 46] = [
    1712, 1616, 1520, 1440, 1360, 1280, 1208, 1144, 1080, 1016, 960, 904, 856, 808, 760, 720, 680, 640, 604, 572, 540, 508, 480, 452, 428, 404,
    380, 360, 340, 320, 302, 286, 270, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127,
];

pub fn detect(d: &[u8]) -> bool {
    d.len() >= 16 && &d[..4] == b"FWMP"
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Attack,
    Decay,
    Hold,
    Release,
    Stopped,
}

#[derive(Clone, Copy)]
struct Chan {
    ptr: usize,
    start: usize,
    cnt: u8,
    ret: usize,
    /// Offset of the current instrument entry (module offset), if one was selected.
    inst: Option<usize>,
    sample: u32,
    slen: u16,
    loop_at: u32,
    llen: u16,
    stage: Stage,
    level: u8,
    hold: u8,
    ended: bool,
    /// The channel read something undefined and reads no more.
    dead: bool,
    /// Whether the channel has called a pattern before (the first call costs 3 cycles more).
    called: bool,
}

impl Chan {
    fn new(level: u8, stage: Stage, hold: u8) -> Chan {
        Chan { ptr: 0, start: 0, cnt: 1, ret: 0, inst: None, sample: 0, slen: 0, loop_at: 0, llen: 0, stage, level, hold, ended: false, dead: false, called: false }
    }
}

pub struct ForgottenWorlds {
    d: Vec<u8>,
    base: u32,
    pt: usize,
    it: usize,
    wd: usize,
    ch: [Chan; 4],
    scale: [u32; 4],
    silent: u32,
    playing: bool,
    /// The clock of the next tick's DMACON write, and the grid it is on time for.
    due: u64,
    grid: u64,
    // per tick
    call: u64,
    /// Cycles (half colour clocks) since the tick's DMACON write.
    t: u64,
    songend: bool,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let silent = m.alloc(8);
    if silent == 0 {
        return None;
    }
    let off32 = |o: usize| -> usize { super::be32(d, o) as usize };
    let pt = 16usize.saturating_add(off32(4));
    let it = 16usize.saturating_add(off32(8));
    let wd = 16usize.saturating_add(off32(12));
    let mut s = ForgottenWorlds {
        d: d.to_vec(),
        base,
        pt,
        it,
        wd,
        ch: [Chan::new(0x30, Stage::Hold, 45), Chan::new(0x40, Stage::Stopped, 50), Chan::new(0x28, Stage::Attack, 0), Chan::new(0x28, Stage::Attack, 0)],
        scale: [64; 4],
        silent,
        playing: false,
        due: 0,
        grid: 0,
        call: 0,
        t: 0,
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    hw.bus.clock += START_DELAY;
    s.start_song(&mut hw);
    // the song starts (Paula's grids restart) this long after the last initialisation write in the
    // reference; the first tick keeps its time (`due` is absolute)
    hw.bus.clock += ORIGIN_LAG;
    Some(Box::new(s))
}

impl ForgottenWorlds {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn rw(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// The song initialisation and the stop sequence's hardware writes (spec 7.1).
    fn hw_silence(&mut self, hw: &mut Hw) {
        for ch in 0..4 {
            hw.loc(ch, self.silent);
            hw.wait(8);
            hw.len(ch, 2);
            hw.wait(8);
            hw.per(ch, 0x7C);
            hw.wait(8);
            hw.vol(ch, 0);
            if ch < 3 {
                hw.wait(15);
            }
        }
        hw.wait(17);
        hw.bus.write_word(0xDFF17E, 0x00FF);
        hw.wait(8);
        hw.bus.write_word(0xDFF176, 0x000F);
        hw.wait(8);
        hw.bus.write_word(0xDFF17C, 0x0780);
        hw.wait(8);
        hw.bus.write_word(0xDFF17A, 0x0780);
    }

    /// Starts (or restarts) the song at the current clock. The envelope state, the current
    /// instrument and the return pointers survive a restart.
    fn start_song(&mut self, hw: &mut Hw) {
        for c in 0..4 {
            let w = self.rw(16 + 2 * c) as usize;
            let ch = &mut self.ch[c];
            ch.start = 16 + w;
            ch.ptr = 16 + w;
            ch.cnt = 1;
            ch.ended = false;
            ch.dead = false;
        }
        self.playing = true;
        self.songend = false;
        let s = hw.bus.clock;
        self.hw_silence(hw);
        self.grid = s + FIRST_TICK;
        self.due = self.grid;
        hw.bus.clock = hw.bus.clock.max(s);
    }

    fn clk(&self) -> u64 {
        self.call + self.t / 2
    }

    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = hw.bus.clock.max(self.clk());
    }

    /// Polls the audio interrupt flag `mask` the way the routine does: the k-th read samples
    /// 13 + 25 k cycles after the write that preceded (cycle `t0`); returns k when the flag
    /// was seen. A flag that never rises ends the wait after `cap` colour clocks.
    fn poll(&mut self, hw: &mut Hw, mask: u16, t0: u64, cap: u64) -> u64 {
        let mut k = 0u64;
        loop {
            let ts = t0 + 13 + 25 * k;
            let clock = self.call + (ts - 1) / 2;
            hw.bus.clock = hw.bus.clock.max(clock);
            let r = hw.bus.read_word(0xDFF01E);
            if r & mask != 0 {
                return k;
            }
            if clock.saturating_sub(self.call + t0 / 2) > cap {
                return k;
            }
            k += 1;
        }
    }

    fn period(note: u8) -> u16 {
        PERIODS.get(note as usize).copied().unwrap_or(0)
    }

    fn pattern_start(&self, p: i8) -> usize {
        let w = self.rw((self.pt as isize + 2 * p as isize).max(0) as usize) as i16;
        (self.pt as isize + w as isize).max(0) as usize
    }

    fn select(&mut self, c: usize, i: i8) {
        let it = (self.it as isize + 8 * i as isize).max(0) as usize;
        let hi = (((i as i16 * 8) >> 8) & 255) as usize;
        let e = self.wd + self.rb(it) as usize + 256 * hi;
        let (s, ls, lo, ll) = (self.rw(e) as u32, self.rw(e + 2), self.rw(e + 4) as u32, self.rw(e + 6));
        let ch = &mut self.ch[c];
        ch.inst = Some(it);
        ch.sample = self.base.wrapping_add(self.wd as u32).wrapping_add(s);
        ch.slen = ls;
        ch.loop_at = ch.sample.wrapping_add(lo);
        ch.llen = ll;
    }

    fn vol_out(&self, c: usize, level: u8) -> u16 {
        ((self.scale[c] * level as u32) >> 6) as u16
    }

    /// The hardware sequence of a note start (spec 5). The cycle counter is at the first write.
    fn note_start(&mut self, hw: &mut Hw, c: usize, note: u8) {
        let ch = self.ch[c];
        let Some(inst) = ch.inst else { return };
        let b2 = self.rb(inst + 2);
        let b = 1u16 << c;
        let irq = 0x80u16 << c;
        let r = 0xDFF0A0 + 0x10 * c as u32;
        let per = Self::period(note);
        // 1
        self.at(hw);
        hw.bus.write_word(0xDFF09C, irq);
        // 2
        self.t += 12;
        self.at(hw);
        hw.dmacon(b);
        // 3
        self.t += 16;
        self.at(hw);
        hw.per(c, 1);
        // 4
        self.t += 16;
        self.at(hw);
        hw.vol(c, 0);
        // 5 W1
        let t0 = self.t;
        let k = self.poll(hw, irq, t0, SETUP_WAIT_CAP);
        self.t = t0 + 25 * (k + 1) + 12;
        // 6
        self.at(hw);
        hw.bus.write_word(0xDFF09C, irq);
        // 7
        self.t += 16;
        self.at(hw);
        hw.bus.write_word(r + 0xA, 0);
        // 8 W2
        let t0 = self.t;
        let k = self.poll(hw, irq, t0, SETUP_WAIT_CAP);
        self.t = t0 + 25 * (k + 1) + 16;
        // 9
        self.at(hw);
        hw.bus.write_word(r + 0xA, 0);
        // 10 W3
        let t0 = self.t;
        let k = self.poll(hw, irq, t0, SETUP_WAIT_CAP);
        self.t = t0 + 25 * (k + 1) + 28;
        // 11
        self.at(hw);
        hw.loc(c, ch.sample);
        self.t += 20;
        self.at(hw);
        hw.len(c, ch.slen);
        self.t += 20;
        self.at(hw);
        hw.per(c, per);
        self.t += 168;
        self.at(hw);
        let v = self.vol_out(c, b2);
        hw.vol(c, v);
        // 12
        self.t += 24;
        self.at(hw);
        hw.dmacon(0x8000 | b);
        // 13
        self.t += 26;
        self.at(hw);
        hw.bus.write_word(0xDFF09C, irq);
        // 14 W4
        let t0 = self.t;
        let k = self.poll(hw, irq, t0, START_WAIT_CAP);
        self.t = t0 + 25 * (k + 1) + 12;
        // 15
        self.at(hw);
        hw.bus.write_word(0xDFF09C, irq);
        // 16
        self.t += 28;
        self.at(hw);
        hw.loc(c, ch.loop_at);
        self.t += 20;
        self.at(hw);
        hw.len(c, ch.llen);
        self.t += 40;
        let chn = &mut self.ch[c];
        chn.level = b2;
        chn.stage = Stage::Attack;
    }

    /// One envelope step of a running channel, then the volume write (spec 4.2).
    fn envelope(&mut self, hw: &mut Hw, c: usize, inst: usize) {
        let (sus, att, dec, hold, rel) = (self.rb(inst + 1), self.rb(inst + 2), self.rb(inst + 3), self.rb(inst + 4), self.rb(inst + 5));
        let ch = &mut self.ch[c];
        self.t += 105;
        match ch.stage {
            Stage::Attack => {
                ch.level = ch.level.wrapping_add(att);
                if ch.level >= 0x40 {
                    ch.level = 0x40;
                    ch.stage = Stage::Decay;
                    self.t += 78;
                } else {
                    self.t += 46;
                }
            }
            Stage::Decay => {
                ch.level = ch.level.wrapping_add(dec);
                if ch.level <= sus {
                    ch.level = sus;
                    ch.hold = hold;
                    ch.stage = Stage::Hold;
                    self.t += 106;
                } else {
                    self.t += 50;
                }
            }
            Stage::Hold => {
                ch.hold = ch.hold.wrapping_sub(1);
                if ch.hold == 0 {
                    ch.stage = Stage::Release;
                    self.t += 66;
                } else {
                    self.t += 42;
                }
            }
            Stage::Release => {
                ch.level = ch.level.wrapping_add(rel);
                if ch.level & 0x80 != 0 {
                    ch.level = 0;
                    ch.stage = Stage::Stopped;
                    self.t += 66;
                } else {
                    self.t += 38;
                }
            }
            Stage::Stopped => {
                self.t += 17;
            }
        }
        self.t += 144;
        let level = self.ch[c].level;
        self.at(hw);
        let v = self.vol_out(c, level);
        hw.vol(c, v);
        self.t += 10;
    }

    /// Reads the events of channel `c` until a note or a rest ends the turn. False when the
    /// command 0x80 ended the tick.
    fn serve(&mut self, hw: &mut Hw, c: usize) -> bool {
        self.t += 48;
        if self.ch[c].dead {
            return true;
        }
        for _ in 0..EVENT_CAP {
            let p = self.ch[c].ptr;
            let b = self.rb(p);
            self.ch[c].ptr = p + 1;
            if b < 0x80 {
                let d = self.rb(p + 1);
                self.ch[c].ptr = p + 2;
                self.ch[c].cnt = d;
                self.t += 134 + 12;
                self.note_start(hw, c, b);
                return true;
            }
            match b {
                0x80 => {
                    // the pointer stays on the command: the channel reads it again
                    // when its (wrapped) counter next runs out
                    self.ch[c].ptr = p;
                    self.ch[c].ended = true;
                    self.ch[c].cnt = 0;
                    return false;
                }
                0x82 => {
                    self.ch[c].ptr = self.ch[c].start;
                    self.t += 83;
                }
                0x84 => {
                    let i = self.rb(p + 1) as i8;
                    self.ch[c].ptr = p + 2;
                    self.select(c, i);
                    self.t += 271;
                }
                0x86 => {
                    let d = self.rb(p + 1);
                    self.ch[c].ptr = p + 2;
                    self.ch[c].cnt = d;
                    self.t += 92;
                    return true;
                }
                0x88 => {
                    let n = self.rb(p + 1) as i8;
                    self.ch[c].ptr = p + 2;
                    self.ch[c].ret = p + 2;
                    self.ch[c].ptr = self.pattern_start(n);
                    self.t += 130;
                    if !self.ch[c].called {
                        self.ch[c].called = true;
                        self.t += 3;
                    }
                }
                0x8A => {
                    self.ch[c].ptr = self.ch[c].ret;
                    self.t += 86;
                }
                _ => {
                    // undefined command: the channel ends, the tick goes on
                    self.ch[c].ended = true;
                    self.ch[c].dead = true;
                    self.ch[c].cnt = 0;
                    return true;
                }
            }
        }
        self.ch[c].ended = true;
        self.ch[c].dead = true;
        true
    }
}

impl Replayer for ForgottenWorlds {
    fn format(&self) -> &'static str {
        "Forgotten Worlds"
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

    fn set_subsong(&mut self, _n: u16, hw: &mut Hw) {
        self.start_song(hw);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        Some(self.due.saturating_sub(IRQ_LEAD))
    }

    fn next_tick_at(&self) -> Option<u64> {
        Some(self.due.saturating_sub(IRQ_LEAD))
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.playing {
            return;
        }
        self.call = hw.bus.clock.max(self.due);
        hw.bus.clock = self.call;
        self.t = 0;
        hw.dmacon(0x800F);
        self.t = 33;
        for c in 0..4 {
            self.ch[c].cnt = self.ch[c].cnt.wrapping_sub(1);
            let mut go = true;
            if self.ch[c].cnt != 0 {
                match self.ch[c].inst {
                    Some(i) if self.rb(i + 4) != 0 => self.envelope(hw, c, i),
                    _ => self.t += 74,
                }
            } else {
                go = self.serve(hw, c);
            }
            self.t += if c == 3 { 46 } else { 42 };
            if !go {
                break;
            }
        }
        if self.ch.iter().all(|c| c.ended) {
            self.songend = true;
        }
        let end = self.clk();
        hw.bus.clock = hw.bus.clock.max(end);
        self.grid += TICK;
        self.due = self.grid.max(self.call + (self.t + 370) / 2);
    }
}

#[cfg(test)]
#[path = "forgottenworlds_tests.rs"]
mod tests;
