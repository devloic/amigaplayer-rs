//! NESA (`.nsa`): the NES APU write logs that the Amiga NES emulator A/NES PRO saves.
//!
//! Clean-room, written from the behaviour specification `docs/specs/nesa/SPEC.md` and black-box
//! runs of the reference (DeliTracker's DeliPlayer `NESA_note`, "NESA v1.23", playing through
//! DeliTracker's default 4Voices-NotePlayer genie on this project's 68000 host). Nothing of the
//! player, of the genie or of A/NES was disassembled or read.
//!
//! A file is a header and a stream of records: a delay in ticks, then (value, register) pairs that
//! are written to a copy of the APU registers `$4000`-`$401F`. The player turns the copy into four
//! NotePlayer voices (pulse 1, pulse 2, triangle, noise), each playing a short looped waveform
//! (SPEC sections 3-6), on DeliTracker's default 50 Hz timer. The song ends one tick after the last
//! record and starts again.
//!
//! The reference plays through the NotePlayer genie, so this port plays as the genie does (SPEC
//! section 8): each Paula channel plays two 96-byte buffers in turn, and its audio interrupt fills
//! the next with the next 96 bytes of the voice's loop (zeros before the voice's first waveform).
//! A period reaches Paula as `3546895 / round(3546895 / period)`, a volume as it is.
//!
//! The write times are a model fitted to the reference's (they are what the reference's 68000 code
//! takes in this project's host, not a property of the format): the player's time before it hands
//! the voices to the genie is a sum of costs of what the tick did (counters, records, register
//! writes, updates), the genie's time per voice depends on its change flags, and an audio
//! interrupt's time on what it copies.
//!
//! Embedded data: `nesa_noise.bin` (512 bytes) is the noise waveform the original player builds in
//! its memory, as SPEC section 6 gives it (the player's output data, observed in guest memory; no
//! rule for it was recognised). Embedding it is the project owner's decision (2026-11-16); it is
//! data, no code of the player is in this port. The duty and triangle waveforms are the
//! specification's tables.
//!
//! Choices where the specification is silent (none can panic or hang):
//! * NESA2 files (byte 4 `$22`) are not taken (open question 10.1: the reference plays nothing);
//! * a length beyond the file stops at the end of the file (SPEC 3.3);
//! * a pair cut off by the end of the data is not applied;
//! * the costs of the register writes that the test files never make (`$4009`, `$400D`, `$4010`
//!   up) are a guess.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "NESA", detect, new };

/// Colour clocks a second (PAL), the constant of the genie's period conversion.
const CLOCK: u32 = 3_546_895;
/// The genie's buffer, in bytes.
const BUF: usize = 96;

/// The NES length table (SPEC section 4), index = value >> 3.
const LEN: [i32; 32] = [
    10, 254, 20, 2, 38, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28,
    32, 30,
];

/// The waveforms (SPEC section 6): duty 0 to 3, the triangle.
const DUTY: [[u8; 8]; 4] = [
    [0x7F, 0x7F, 0x7F, 0x7F, 0x7F, 0x7F, 0x7F, 0x80],
    [0x7F, 0x7F, 0x7F, 0x7F, 0x7F, 0x7F, 0x80, 0x80],
    [0x7F, 0x7F, 0x7F, 0x7F, 0x80, 0x80, 0x80, 0x80],
    [0x7F, 0x7F, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80],
];
const TRIANGLE: [u8; 16] = [0x00, 0x1F, 0x3F, 0x5F, 0x7F, 0x5F, 0x3F, 0x1F, 0x00, 0xE0, 0xC0, 0xA0, 0x80, 0xA0, 0xE0, 0x00];
/// The noise waveform the original player builds (SPEC section 6); see the module comment.
static NOISE: &[u8; 512] = include_bytes!("nesa_noise.bin");

const W_TRIANGLE: u8 = 4;
const W_NOISE: u8 = 5;

fn wave(w: u8) -> &'static [u8] {
    match w {
        0..=3 => &DUTY[w as usize],
        W_TRIANGLE => &TRIANGLE,
        _ => NOISE,
    }
}

/// Change flags of a voice (SPEC section 5).
const F_VOL: u8 = 0x10;
const F_FREQ: u8 = 0x08;
const F_REPEAT: u8 = 0x04;

// The player's time, in half colour clocks (68000 cycles), before it calls the genie.
const C_BASE: u32 = 901;
const C_FIRST_TICK: u32 = 49;
const C_FIRST_RECORD: u32 = 50;
const C_DUE: u32 = 134;
const C_SONG_END: u32 = 1657;
/// A register write, by register; the ones the test files never write are a guess (600).
const C_WRITE: [u32; 32] = [
    637, 0, 652, 692, 638, 0, 653, 692, 594, 600, 677, 767, 682, 600, 668, 0, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600,
];
/// A write of the value the register holds already, on top (only these three were seen to cost).
fn c_same(r: usize) -> u32 {
    match r {
        3 => 6,
        0xF => 1780,
        _ => 0,
    }
}
const C_PULSE_AUDIBLE: [u32; 2] = [294, 292];
const C_PULSE_NO_T: [u32; 2] = [0, 143];
const C_PULSE_SKIP3: [u32; 2] = [7, 0];
const C_TRI_AUDIBLE: u32 = 174;
const C_NOISE_AUDIBLE: u32 = 165;
const C_NOISE_NO_LEN: u32 = 25;
const C_PULSE_DEC: u32 = 4;
const C_PULSE_RAN_OUT: u32 = 25;
const C_NOISE_DEC: u32 = 30;
const C_NOISE_RAN_OUT: u32 = 43;

// The genie (colour clocks): from the player's call to the first voice, per voice by its flags.
const G_START: u64 = 505;
/// The time of a voice (from its start to the next voice's) with a frequency change, a volume
/// change only, no change; +2 for a voice whose period is 0.
const G_VOICE: [u64; 3] = [369, 321, 192];
const G_PER: u64 = 104;
const G_VOL_FREQ: u64 = 246;
const G_VOL_ONLY: u64 = 197;
/// From the last voice's volume write (frequency and volume change, volume only; or its start,
/// without change) to the end of the tick.
const G_END_FREQ: u64 = 160;
const G_END_WRITE: u64 = 161;
const G_END_NONE: u64 = 231;

// The genie's audio interrupt (colour clocks): to the LOC write, then the copy by what it copies.
const H_LOC: u64 = 63;
const H_ZERO: u64 = 367;
const H_PULSE: u64 = 1263;
const H_TRIANGLE: u64 = 846;
const H_EXIT: u64 = 30;

/// The reference's start (colour clocks from `new`): its init writes and the first tick.
const T_OFF: u64 = 2606;
const T_VOL: [u64; 8] = [11369, 11608, 11848, 12087, 12630, 12869, 13109, 13348];
const T_BUFFERS: u64 = 18231;
const T_ON: u64 = 18729;
/// The end of the reference's start-up (the song's start: Paula's line grid restarts there).
const T_STARTED: u64 = 18932;
const T_FIRST_TICK: u64 = 89872;

fn detect(d: &[u8]) -> bool {
    d.len() >= 16 && &d[..4] == b"NESA" && d[4] == 0x23
}

/// The period as the genie writes it (SPEC section 8).
fn written_period(p: u32) -> u16 {
    if p == 0 {
        return 0;
    }
    let f = ((2 * CLOCK + p) / (2 * p)).max(1);
    (CLOCK / f).clamp(122, 0xFFFF) as u16
}

/// A NotePlayer voice as the player fills it.
#[derive(Clone, Copy, Default)]
struct Voice {
    flags: u8,
    wave: Option<u8>,
    per: u32,
    vol: u16,
}

/// The sequencer and the APU copy (SPEC sections 3-5).
struct Seq {
    data: Vec<u8>,
    reg: [u8; 32],
    plen: [i32; 2],
    reload: [i32; 2],
    flevel: [i32; 2],
    fon: [bool; 2],
    tlen: i32,
    level: i32,
    linear: i32,
    nlen: i32,
    nper: u32,
    tri_fade: bool,
    p: usize,
    c: u8,
    end_next: bool,
    v: [Voice; 4],
    /// The player's time this tick, half colour clocks.
    cost: u32,
    /// Records applied since the start.
    records: u64,
}

impl Seq {
    /// The sequencer at the start of file `d` (SPEC sections 2 and 3.1).
    fn new(d: &[u8]) -> Seq {
        let start = 16 + 32 * (d[6] & 0x70).count_ones() as usize;
        let len = u32::from_be_bytes([d[12], d[13], d[14], d[15]]) as usize;
        let data = d.get(start..).map_or(&[][..], |r| &r[..len.min(r.len())]).to_vec();
        let mut s = Seq {
            data,
            reg: [0; 32],
            plen: [0; 2],
            reload: [0; 2],
            flevel: [0; 2],
            fon: [false; 2],
            tlen: 0,
            level: 0,
            linear: 0,
            nlen: 0,
            nper: 0,
            tri_fade: false,
            p: 0,
            c: 0,
            end_next: false,
            v: [Voice::default(); 4],
            cost: 0,
            records: 0,
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        self.reg = [0; 32];
        self.plen = [0; 2];
        self.reload = [0; 2];
        self.flevel = [0; 2];
        self.fon = [false; 2];
        self.tlen = 0;
        self.level = 0;
        self.linear = 0;
        self.nlen = 0;
        self.nper = 0;
        self.end_next = false;
        self.p = 0;
        if self.data.is_empty() {
            self.end_next = true;
            self.c = 1;
        } else {
            self.c = self.data[0];
            self.p = 1;
        }
    }

    fn audible(&mut self, k: usize, wave: u8, per: u32, vol: i32) {
        let v = &mut self.v[k];
        v.flags |= F_VOL | F_FREQ | F_REPEAT;
        v.wave = Some(wave);
        v.per = per;
        v.vol = vol as u16;
    }

    fn set_vol(&mut self, k: usize, vol: i32) {
        self.v[k].flags |= F_VOL;
        self.v[k].vol = vol as u16;
    }

    fn pulse(&mut self, i: usize) {
        let r = &self.reg[4 * i..4 * i + 4];
        let n = (r[0] & 15) as i32;
        let t = ((r[3] & 7) as u32) << 8 | r[2] as u32;
        let duty = r[0] >> 6;
        if n != 0 && t != 0 {
            self.cost += C_PULSE_AUDIBLE[i];
            self.audible(i, duty, 4 * t, n + 23);
            self.plen[i] = self.reload[i];
        } else {
            if n != 0 {
                self.cost += C_PULSE_NO_T[i];
            }
            self.set_vol(i, 0);
        }
        self.flevel[i] = if n != 0 { n + 23 } else { 0 };
        self.fon[i] = false;
    }

    fn triangle(&mut self) {
        let t = ((self.reg[0xB] & 7) as u32) << 8 | self.reg[0xA] as u32;
        if t != 0 && self.tlen > 0 {
            self.cost += C_TRI_AUDIBLE;
            self.audible(2, W_TRIANGLE, 4 * t, self.level);
        } else {
            self.set_vol(2, 0);
        }
    }

    fn noise(&mut self) {
        let n = (self.reg[0xC] & 15) as i32;
        if n != 0 && self.nlen > 0 {
            self.cost += C_NOISE_AUDIBLE;
            self.audible(3, W_NOISE, self.nper, 4 * n);
        } else {
            if n != 0 {
                self.cost += C_NOISE_NO_LEN;
            }
            self.set_vol(3, 0);
        }
    }

    /// A write to `$4000 + r` (SPEC section 4).
    fn write(&mut self, r: usize, v: u8) {
        if self.reg[r] == v {
            self.cost += c_same(r);
        }
        self.cost += C_WRITE[r];
        self.reg[r] = v;
        match r {
            0 | 4 => self.pulse(r / 4),
            2 | 6 => {
                if self.reg[r - 2] & 15 != 0 {
                    self.pulse(r / 4)
                }
            }
            3 | 7 => {
                self.reload[r / 4] = if v >> 3 == 1 { 2 } else { LEN[(v >> 3) as usize] };
                if self.reg[r - 3] & 15 != 0 {
                    self.pulse(r / 4)
                } else {
                    self.cost += C_PULSE_SKIP3[r / 4];
                }
            }
            8 => self.linear = (v & 0x7F) as i32,
            0xA => {
                self.tlen = LEN[(self.reg[0xB] >> 3) as usize];
                self.triangle();
            }
            0xB => {
                self.level = 46;
                self.tlen = LEN[(v >> 3) as usize];
                self.triangle();
            }
            0xC => self.noise(),
            0xE => {
                self.nper = 500;
                self.noise();
            }
            0xF => {
                self.nlen = LEN[(v >> 3) as usize];
                self.noise();
            }
            _ => {}
        }
    }

    /// The per-tick counters (SPEC section 5.5).
    fn counters(&mut self) {
        for i in 0..2 {
            let halt = self.reg[4 * i] & 0x20 != 0;
            if self.plen[i] > 0 {
                self.cost += C_PULSE_DEC;
                self.plen[i] -= 2;
                if self.plen[i] <= 0 {
                    self.cost += C_PULSE_RAN_OUT;
                }
            }
            if self.plen[i] <= 0 && !halt {
                self.fon[i] = true;
            }
            if self.fon[i] && self.flevel[i] > 0 {
                self.flevel[i] -= 1;
                self.set_vol(i, self.flevel[i]);
            }
        }
        if self.nlen > 0 {
            self.cost += C_NOISE_DEC;
            self.nlen -= 2;
            if self.nlen == 0 {
                self.cost += C_NOISE_RAN_OUT;
                self.set_vol(3, 0);
            }
        }
        let halt = self.reg[8] & 0x80 != 0;
        if self.tlen > 0 && !halt {
            self.tlen -= 2;
            if self.tlen == 0 && self.level > 0 {
                self.set_vol(2, 0);
            }
        }
        if self.tri_fade && self.level > 0 {
            self.level -= 1;
            self.set_vol(2, self.level);
        }
        if !halt && self.linear >= 4 {
            self.linear -= 4;
            if self.linear < 4 && !self.tri_fade {
                self.tri_fade = true;
                self.set_vol(2, 0);
            }
        }
    }

    /// One tick (SPEC section 3.2); true at the song's end.
    fn tick(&mut self) -> bool {
        for v in &mut self.v {
            v.flags = 0;
        }
        self.counters();
        self.c = self.c.wrapping_sub(1);
        while self.c == 0 {
            self.cost += C_DUE;
            if self.end_next {
                for k in 0..4 {
                    self.set_vol(k, 0);
                }
                self.cost += C_SONG_END;
                self.reset();
                return true;
            }
            // the record's pairs, the last without bit 5
            self.records += 1;
            let len = self.data.len();
            while self.p < len {
                if self.p + 1 >= len {
                    self.p = len;
                    break;
                }
                let (val, code) = (self.data[self.p], self.data[self.p + 1]);
                self.p += 2;
                self.write((code & 31) as usize, val);
                if code & 0x20 == 0 {
                    break;
                }
            }
            if self.p >= len {
                self.end_next = true;
                self.c = 1;
            } else {
                self.c = self.data[self.p];
                self.p += 1;
            }
        }
        false
    }
}

/// The genie's side of a Paula channel.
#[derive(Clone, Copy, Default)]
struct Chan {
    /// The repeat (waveform) the genie plays, from the voice's last repeat change.
    wave: Option<u8>,
    /// The read position in the loop (bytes copied since the first waveform).
    pos: usize,
    /// The buffer the next interrupt fills (0 or 1).
    next: usize,
}

struct Nesa {
    seq: Seq,
    ch: [Chan; 4],
    buffers: u32,
    first_tick: Option<u64>,
    ticks: u64,
    first_record: bool,
    songend: bool,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let seq = Seq::new(d);
    let buffers = m.alloc((8 * BUF) as u32);
    if buffers == 0 {
        return None;
    }
    let mut r = Nesa {
        seq,
        ch: [Chan { next: 1, ..Chan::default() }; 4],
        buffers,
        first_tick: None,
        ticks: 0,
        first_record: true,
        songend: false,
    };
    // the genie's start (SPEC section 8)
    let t0 = m.bus.clock;
    let mut hw = Hw { bus: &mut m.bus };
    let at = |hw: &mut Hw, t: u64| hw.bus.clock = hw.bus.clock.max(t0 + t);
    at(&mut hw, T_OFF);
    for (i, (a, v)) in [(0xDFF096, 0x000F), (0xDFF09E, 0x00FF), (0xDFF09C, 0x0780), (0xDFF09A, 0x0780)].into_iter().enumerate()
    {
        at(&mut hw, T_OFF + 8 * i as u64);
        hw.bus.write_word(a, v);
    }
    for (i, &t) in T_VOL.iter().enumerate() {
        at(&mut hw, t);
        hw.vol(i & 3, 0);
    }
    for k in 0..4 {
        at(&mut hw, T_BUFFERS + 58 * k as u64);
        hw.loc(k, r.buffer(k, 0));
        hw.wait(9);
        hw.len(k, (BUF / 2) as u16);
        hw.wait(9);
        hw.per(k, 0x80);
    }
    for (i, (a, v)) in [(0xDFF096, 0x800F), (0xDFF09E, 0x00FF), (0xDFF09C, 0x0780), (0xDFF09A, 0x8780)].into_iter().enumerate()
    {
        at(&mut hw, T_ON + 8 * i as u64);
        hw.bus.write_word(a, v);
    }
    at(&mut hw, T_STARTED);
    r.first_tick = Some(t0 + T_FIRST_TICK);
    Some(Box::new(r))
}

impl Nesa {
    fn buffer(&self, k: usize, n: usize) -> u32 {
        self.buffers + ((2 * k + n) * BUF) as u32
    }
}

impl Replayer for Nesa {
    fn format(&self) -> &'static str {
        "NESA"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.first_tick.take()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        let start = hw.bus.clock;
        let s = &mut self.seq;
        s.cost = C_BASE;
        if self.ticks == 0 {
            s.cost += C_FIRST_TICK;
        }
        if s.tick() {
            self.songend = true;
        }
        if self.first_record && s.records != 0 {
            s.cost += C_FIRST_RECORD;
            self.first_record = false;
        }
        self.ticks += 1;
        // the genie: each voice in turn, its writes by its flags
        let mut t = start + (s.cost / 2) as u64 + G_START;
        let mut end = t;
        for k in 0..4 {
            let v = s.v[k];
            let z = (v.per == 0) as u64 * 2;
            if v.flags & F_REPEAT != 0 {
                self.ch[k].wave = v.wave;
            }
            if v.flags & F_FREQ != 0 {
                hw.bus.clock = hw.bus.clock.max(t + G_PER + z);
                hw.per(k, written_period(v.per));
                hw.bus.clock = hw.bus.clock.max(t + G_VOL_FREQ + z);
                hw.vol(k, v.vol);
                end = hw.bus.clock + G_END_FREQ;
                t += G_VOICE[0] + z;
            } else if v.flags & F_VOL != 0 {
                hw.bus.clock = hw.bus.clock.max(t + G_VOL_ONLY + z);
                hw.vol(k, v.vol);
                end = hw.bus.clock + G_END_WRITE;
                t += G_VOICE[1] + z;
            } else {
                end = t + G_END_NONE;
                t += G_VOICE[2] + z;
            }
        }
        hw.bus.clock = hw.bus.clock.max(end);
    }

    /// A buffer started: the genie fills the other one with the next 96 bytes of the loop.
    fn audio_irq_ch(&mut self, k: usize, hw: &mut Hw) -> bool {
        // one raised before the start's INTREQ clear is gone
        if hw.bus.paula.intreq & 0x80 << k == 0 {
            return true;
        }
        let n = self.ch[k].next;
        self.ch[k].next ^= 1;
        hw.wait(H_LOC);
        hw.loc(k, self.buffer(k, n));
        let mut b = [0u8; BUF];
        let cost = match self.ch[k].wave {
            None => H_ZERO,
            Some(w) => {
                let src = wave(w);
                let pos = self.ch[k].pos;
                for (i, x) in b.iter_mut().enumerate() {
                    *x = src[(pos + i) % src.len()];
                }
                self.ch[k].pos = (pos + BUF) % 512;
                match w {
                    0..=3 => H_PULSE,
                    W_TRIANGLE => H_TRIANGLE,
                    _ => match pos % 512 {
                        0 => 501,
                        416 => 478,
                        448 | 480 => 550,
                        _ => 480,
                    },
                }
            }
        };
        hw.bus.write_bytes(self.buffer(k, n), &b);
        hw.wait(cost);
        hw.bus.write_word(0xDFF09C, 0x80 << k);
        hw.wait(H_EXIT);
        true
    }
}

#[cfg(test)]
#[path = "nesa_tests.rs"]
mod tests;
