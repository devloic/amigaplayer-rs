//! PowerTracker (Laxity / Kefrens; `*.pt`, the music of two Kefrens intros by Merge).
//!
//! Clean-room, written from the behaviour specification `docs/specs/powertracker/SPEC.md`; no
//! original code, and no byte of any player. The specification was drawn up from black-box
//! observation only: the Paula register writes (with their colour clocks) of the DeliTracker
//! DeliPlayer `PowerTracker` (PlayerStore, "PowerTracker v1.20") on this project's 68000
//! engine, its calls of the host, the memory it plays its waveforms from (read after each of its
//! calls), and how all of that changes when module bytes are changed (built modules). Nothing of
//! the player was disassembled or read; no source of it was used.
//!
//! A module is plain data: a header (offsets, song length, restart position), instrument
//! records of 24 bytes (a name, then four words), four position lists of track numbers, and
//! tracks of 64 rows of four bytes (note, volume, instrument and effect, parameter) for one
//! channel each. Four voices, one Paula channel each, 50 ticks a second. An instrument is
//! either a pulse wave the player shapes in a 32-byte buffer of the voice (its width swept
//! between two bounds) or a sample in the module.
//!
//! The values written are the specification's. The write times are the original's: each step
//! of the code below adds the CPU cycles measured for the same step of the original (the table
//! `K`, SPEC section 10); the host's dtg_WaitAudioDMA is the engine's.
//!
//! Choices where the specification is silent (none can panic or hang): see SPEC section 11.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "PowerTracker", detect, new };

/// The top octave of the period table; the notes 1 to 60 are it shifted (SPEC section 5).
const TOP: [u16; 12] = [214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113];

/// The period of note `n` (1 to 60); 0 for any other.
fn period(n: u8) -> u16 {
    if !(1..=60).contains(&n) {
        return 0;
    }
    let i = n as usize - 1;
    TOP[i % 12] << (4 - i / 12)
}

/// The detune step per note 1..60: an instrument's detune (record byte 6, signed) adds `b6 · DETUNE[n - 1]`
/// to the note's period (measured, SPEC section 5).
const DETUNE: [u8; 60] = [
    24, 24, 20, 20, 20, 18, 16, 16, 16, 14, 14, 12, 12, 12, 10, 10, 10, 9, 8, 8, 8, 7, 7, 6, 6, 6, 5, 5, 5, 4, 4, 4, 4, 3, 3,
    3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];

/// The period offset of detune `d` at note `n` (the triggered note's, kept for the arpeggio's notes).
fn detune_off(n: u8, d: i8) -> u16 {
    let step = if (1..=60).contains(&n) { DETUNE[n as usize - 1] as i16 } else { 0 };
    (d as i16).wrapping_mul(step) as u16
}

/// The first eight values of the usual vibrato sine (SPEC section 6.4).
const SINE: [u16; 8] = [0, 24, 49, 74, 97, 120, 141, 161];

/// Rows of a track.
const ROWS: usize = 64;

/// Logs the steps counted before each event (target `pt_cost`), to fit the table `K`.
const COST_LOG: bool = false;

// ---- the cost table: CPU cycles of each step (SPEC section 10)
macro_rules! costs {
    ($($name:ident = $v:expr),* $(,)?) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms, clippy::enum_variant_names, dead_code)]
        #[derive(Clone, Copy)]
        #[repr(usize)]
        enum C { $($name),* , N }
        const K: [u64; C::N as usize] = [$($v),*];
        #[allow(dead_code)]
        const NAMES: [&str; C::N as usize] = [$(stringify!($name)),*];
    };
}

costs! {
    // the init's length, in colour clocks (the song starts after it): INIT0 + INIT_POS per
    // position + INIT_REC per record + INIT_MAX per new maximum of the list bytes
    INIT0 = 497, INIT_POS = 54, INIT_REC = 29, INIT_MAX = 6,
    // to the first channel
    TICK_ROW = 479, TICK_FX = 428,
    // row tick, a channel whose list holds FF
    RE_PRE = 0, RE_POST = 244, RE_SKIP = 118,
    // row tick, a row: its base, the note, the instrument nibble, the volume byte
    RR = 227,
    RN_ZERO = 0, RN_NOTE = 0, RN_PORTA = 0, RN_OFF80 = 377, RN_OFF40 = 378,
    RI_NONE = 0, RI_SET = 378, RI_PULSE = 147, RI_SAMPLE = 0, RI_SAMPLE0 = 0, RI_LOOP = 10, RI_KEEP = 1,
    RV_SET = 33, RV_KEEP = 0,
    // after the volume: no trigger, trigger without instrument, trigger (LC, LEN, PER, after)
    RT_NONE = 113, RT_PORTA = 310, RT_BARE = 184, RT_DETUNE = 72, RT_LC = 206, RT_LEN = 20, RT_PER = 20, RT_AFTER = 134,
    // the volume byte's check (row tick)
    RVC_LT80 = 0, RVC_FF = 19, RVC_FE = 54, RVC_UP = 97, RVC_DN = 96, RVC_CLAMP_UP = 26, RVC_CLAMP_DN = 16,
    RVC_POST = 47, RVC_OTHER = 90,
    // the row effects
    RF0 = 0, RF1 = 105, RF2 = 105, RF3 = 105, RF4 = 105, RF5 = 35, RF6 = 105, RF7 = 128, RF8 = 105, RF9 = 105, RFA = 105,
    RFB = 97, RFC = 138, RFC_POST = 0, RFD = 137, RFE = 114, RFE_MID = 20, RFE_POST = 0, RFF = 129,
    R_POST = 134,
    // the next row
    ADV_ROW = 125, ADV_POS = 0, ADV_WRAP = 0, ADV_BRK = 0, ADV_JMP = 125,
    // effect tick: the volume byte's check
    FVC_LT80 = 0, FVC_FF = 19, FVC_FE = 54, FVC_UP = 66, FVC_DN = 65, FVC_CLAMP_UP = 26, FVC_CLAMP_DN = 16,
    FVC_POST = 78, FVC_OTHER = 90, FADV_BRK = 0,
    // effect tick: the effects
    F0_NONE = 0, F0_P0 = 41, F0_P1 = 104, F0_P2 = 118, F0_POST = 16,
    F1 = 49, F2 = 53, F12_POST = 16,
    F3_ON = 133, F3_OFF = 43, F3_POST = 16,
    F4 = 231, F4_P0 = 0, F4_NZ = 64, F4_HI0 = 34, F4_LO0 = 5, F4_NEG = 0, F4_POS = 10, F4_POST = 62,
    F5_STEP = 199, F5_UP = 0, F5_DN = 4, F5_WAIT = 81, F5_CLAMP = 0, F5_POST = 16,
    F6 = 251, F6_P0 = 92,
    FP = 69, FP_POST = 31,
    FA = 69, FA_MID = 123, FA_UP = 0, FA_DN = 4, FA_CLAMP = 26, FA_POST = 16,
    F_POST = 155,
    // the pulse waves, per voice
    P_OFF = 0, P_WAIT = 25, P_UP = 0, P_DN = 0, P_HOLD_MAX = 28, P_HOLD_MIN = 17, P_FILL = 4, P_STEP = 823,
    // to the wait
    E_MASK = 0, E_DMA_POST = 196, E_NOMASK = 156, E_NEWPOS = 93, E_BRK = 25, E_BRK_WRAP = 25, E_WRAP = 74, E_SONGEND_POST = 110,
    // after the first wait, after the second
    S2_DMA = 40, S2_WAIT = 52,
    S3_LC = 60, S3_LEN = 21, S3_NEXT = 27,
}

/// The module's layout (SPEC section 2).
#[derive(Clone, Debug)]
struct Lay {
    /// The tracks' file offset (also the base of the sample data).
    tracks: usize,
    /// Instrument records.
    nrec: usize,
    /// Positions a list.
    len: usize,
    restart: usize,
    /// The first list's file offset.
    lists: usize,
}

fn layout(d: &[u8]) -> Option<Lay> {
    if d.len() < 10 {
        return None;
    }
    let (p, recs, len, restart) = (be16(d, 0) as usize, be16(d, 2) as usize, be16(d, 4) as usize, be16(d, 6) as usize);
    if recs == 0 || recs % 24 != 0 || !(1..=255).contains(&len) || restart >= len || p != 8 + recs + 2 + 4 * len {
        return None;
    }
    // the two bytes before the lists: 01 00 in every module known; the original also plays others,
    // with a layout not worked out (SPEC section 2), which the port leaves alone
    if be16(d, 8 + recs) != 0x0100 {
        return None;
    }
    Some(Lay { tracks: p, nrec: recs / 24, len, restart, lists: 8 + recs + 2 })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some_and(|l| d.len() >= l.tracks)
}

#[derive(Clone, Copy, Default)]
struct Pwm {
    on: bool,
    /// The record's word 0, its low byte: the counter's reload.
    delay: u8,
    min: i32,
    max: i32,
    w: i32,
    down: bool,
    /// Counted down each tick (a byte); the wave steps when it goes below 0 (signed) and
    /// reloads the delay, so 0 to 7F step every delay + 1 ticks, 80 every 129, 81 to FF every tick.
    count: u8,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    vol: u8,
    /// The note's period, and the period last written.
    base: u16,
    per: u16,
    note: u8,
    ins: u8,
    fx: u8,
    param: u8,
    /// The row's volume byte.
    vb: u8,
    vib_pos: u8,
    vib_speed: u8,
    vib_depth: u8,
    porta_target: u16,
    porta_down: bool,
    /// The instrument's detune: its record's byte 6 (signed), see `DETUNE`.
    detune: i8,
    /// The period offset of the detune at the last triggered note.
    doff: u16,
    /// Effect 5's tick counter.
    vs_count: u8,
    loop_ptr: u32,
    loop_len: u16,
    pwm: Pwm,
}

pub struct PowerTracker {
    d: Vec<u8>,
    lay: Lay,
    /// Guest address of the module and of the silent word (the four 32-byte buffers follow it).
    base: u32,
    silent: u32,
    /// The samples' file offset (SPEC section 3).
    samples: usize,
    v: [Voice; 4],
    speed: u8,
    count: u8,
    pos: usize,
    row: usize,
    /// The DMACON value written after the wait (0 until a note has started one).
    dma_on: u16,
    songend: bool,
    // the cycle count since the last epoch (the call's start, a wait's end)
    epoch: u64,
    t: u64,
    /// SongEnd is called at the tick's end (a volume byte FE, the list's end).
    songend_call: bool,
    /// A volume byte FE was seen this tick.
    fe_brk: bool,
    /// The row tick moved to another position.
    newpos: bool,
    /// ... past the list's end, back to the restart position.
    wrapped: bool,
    /// ... by effect D.
    broke: bool,
    // the steps counted since the epoch (the cost log, `COST_LOG`)
    n: [u32; C::N as usize],
}

impl PowerTracker {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }
    fn rw(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    fn buffer(&self, k: usize) -> u32 {
        self.silent + 2 + 32 * k as u32
    }

    // ---- timing: CPU cycles since the epoch, two a colour clock
    fn c(&mut self, k: C) {
        self.t += K[k as usize];
        self.n[k as usize] += 1;
    }
    fn c_n(&mut self, k: C, n: u32) {
        self.t += K[k as usize] * n as u64;
        self.n[k as usize] += n;
    }
    fn epoch(&mut self, hw: &Hw) {
        self.epoch = hw.bus.clock;
        self.t = 0;
        self.n = [0; C::N as usize];
    }
    /// The clock to the current cycle count (an event's moment).
    fn at(&mut self, hw: &mut Hw, _ev: &str) {
        let at = self.epoch + self.t / 2;
        if at > hw.bus.clock {
            hw.wait(at - hw.bus.clock);
        }
        if COST_LOG {
            let s: Vec<String> =
                self.n.iter().enumerate().filter(|(_, x)| **x != 0).map(|(i, x)| format!("{}:{x}", NAMES[i])).collect();
            log::trace!(target: "pt_cost", "{_ev} {}", s.join(","));
        }
    }
    fn vol(&mut self, hw: &mut Hw, k: usize, v: u16) {
        self.at(hw, "VOL");
        hw.vol(k, v);
    }
    fn per(&mut self, hw: &mut Hw, k: usize, v: u16) {
        self.at(hw, "PER");
        hw.per(k, v);
    }
    fn loc(&mut self, hw: &mut Hw, k: usize, a: u32) {
        self.at(hw, "LC");
        hw.loc(k, a);
    }
    fn len(&mut self, hw: &mut Hw, k: usize, v: u16) {
        self.at(hw, "LEN");
        hw.len(k, v);
    }
    fn dmacon(&mut self, hw: &mut Hw, v: u16) {
        self.at(hw, "DMA");
        hw.dmacon(v);
    }
    /// The host's dtg_WaitAudioDMA (`deli::wait_audio_dma`): to the start of the tenth raster
    /// line from now.
    fn wait_dma(&mut self, hw: &mut Hw) {
        self.at(hw, "WAIT");
        let line = crate::paula::LINE;
        hw.bus.clock = (hw.bus.clock / line + 10) * line;
        self.epoch(hw);
    }

    /// The instrument record of instrument `i` (1-15): its file offset (it may lie past the
    /// records, in the module's other data, as the original reads it).
    fn rec(&self, i: u8) -> usize {
        8 + 24 * (i as usize - 1) + 16
    }

    /// The record of instrument `i`: its four words (the last as its low byte, the pulse flag).
    fn record(&self, i: u8) -> (u16, u16, u16, u8) {
        let r = self.rec(i);
        (self.rw(r), self.rw(r + 2), self.rw(r + 4), self.rb(r + 7))
    }

    /// The loop voice `k` repeats with instrument `i`: its buffer, or the sample's loop (the silent
    /// word when the loop's length is 0).
    fn loop_of(&self, k: usize, i: u8) -> (u32, u16) {
        let (_, w1, w2, flag) = self.record(i);
        if flag != 0 {
            (self.buffer(k), 16)
        } else if w2 == 0 {
            (self.silent, 1)
        } else {
            (self.base + self.sample_off(i) as u32 + 2 * w1 as u32, w2)
        }
    }

    /// The sample's file offset: the samples' start plus the lengths of the records before it.
    fn sample_off(&self, i: u8) -> usize {
        let words: usize = (1..i).map(|j| self.rw(self.rec(j)) as usize).sum();
        self.samples + 2 * words
    }

    /// The pulse wave of width `w` (0 to 32) in voice `k`'s buffer (SPEC section 7).
    fn shape(&self, hw: &mut Hw, k: usize, w: i32) {
        let left = 16 - w.div_euclid(2);
        let right = 16 + (w - 1).div_euclid(2);
        let mut b = [0u8; 32];
        for (i, x) in b.iter_mut().enumerate() {
            let i = i as i32;
            *x = if i < left {
                0x80
            } else if i <= right {
                0
            } else {
                0x7f
            };
        }
        hw.bus.write_bytes(self.buffer(k), &b);
    }

    fn pwm_step(&mut self, hw: &mut Hw, k: usize) {
        let p = self.v[k].pwm;
        if !p.on {
            self.c(C::P_OFF);
            return;
        }
        let c = p.count.wrapping_sub(1);
        if c as i8 >= 0 {
            self.v[k].pwm.count = c;
            self.c(C::P_WAIT);
            return;
        }
        self.c(C::P_STEP);
        let mut p = p;
        p.count = p.delay;
        if !p.down {
            // at a bound the width is set to it (it may start past it: a minimum above the maximum)
            if p.w >= p.max {
                p.w = p.max;
                p.down = true;
                self.c(C::P_HOLD_MAX);
            } else {
                p.w += 1;
                self.c(C::P_UP);
            }
        } else if p.w <= p.min {
            p.w = p.min;
            p.down = false;
            self.c(C::P_HOLD_MIN);
        } else {
            p.w -= 1;
            self.c(C::P_DN);
        }
        self.v[k].pwm = p;
        let w = p.w.clamp(0, 32);
        self.c_n(C::P_FILL, w as u32);
        self.shape(hw, k, w);
    }

    /// The track row of voice `k` now (Err: the list's byte, FE or FF, which reads no row).
    fn row_bytes(&self, k: usize) -> Result<[u8; 4], u8> {
        let t = self.rb(self.lay.lists + k * self.lay.len + self.pos);
        if t >= 0xfe {
            return Err(t);
        }
        // the track number is signed: 80 to FD read before the tracks (SPEC section 4); before the
        // file the port reads zeros
        let o = self.lay.tracks as isize + 256 * (t as i8 as isize) + 4 * self.row as isize;
        let b = |i: isize| if o + i < 0 { 0 } else { self.rb((o + i) as usize) };
        Ok([b(0), b(1), b(2), b(3)])
    }

    /// The volume byte's check, each tick after the row's writes (SPEC section 6.1).
    fn vol_check(&mut self, hw: &mut Hw, k: usize, row: bool) {
        let vb = self.v[k].vb;
        let (lt80, ff, fe, up, dn, cu, cd, post, other) = if row {
            (C::RVC_LT80, C::RVC_FF, C::RVC_FE, C::RVC_UP, C::RVC_DN, C::RVC_CLAMP_UP, C::RVC_CLAMP_DN, C::RVC_POST, C::RVC_OTHER)
        } else {
            (C::FVC_LT80, C::FVC_FF, C::FVC_FE, C::FVC_UP, C::FVC_DN, C::FVC_CLAMP_UP, C::FVC_CLAMP_DN, C::FVC_POST, C::FVC_OTHER)
        };
        match vb {
            0..=0x7f => self.c(lt80),
            0xff => self.c(ff),
            0xfe => {
                // a break (as effect D) on every tick of the row (SPEC section 6.1)
                self.c(fe);
                self.fe_brk = true;
            }
            0x80..=0x9f => {
                let x = (vb & 15) as i32;
                let v = self.v[k].vol as i32;
                let nv = if vb < 0x90 {
                    self.c(up);
                    if v + x > 0x40 {
                        self.c(cu);
                        0x40
                    } else {
                        v + x
                    }
                } else {
                    self.c(dn);
                    if v - x < 0 {
                        self.c(cd);
                        0
                    } else {
                        v - x
                    }
                };
                self.v[k].vol = nv as u8;
                self.vol(hw, k, nv as u16);
                self.c(post);
            }
            _ => self.c(other),
        }
    }

    fn row_tick(&mut self, hw: &mut Hw) -> u16 {
        let mut mask = 0u16;
        let mut brk = false;
        let mut jump = None;
        for k in 0..4 {
            let [n, vb, ie, p] = match self.row_bytes(k) {
                Ok(r) => r,
                Err(0xfe) => {
                    // FE: the channel is left as it is, without a write (SPEC section 4)
                    self.c(C::RE_SKIP);
                    continue;
                }
                Err(_) => {
                    // FF: the row is not read: the voice's state goes on, but its pulse width
                    // turns to narrowing (SPEC section 4)
                    self.v[k].pwm.down = true;
                    self.c(C::RE_PRE);
                    self.vol(hw, k, 0);
                    self.c(C::RE_POST);
                    continue;
                }
            };
            self.c(C::RR);
            let ins = ie >> 4;
            let fx = ie & 15;
            {
                let v = &mut self.v[k];
                v.fx = fx;
                v.param = p;
                v.vb = vb;
            }
            if fx == 4 {
                let v = &mut self.v[k];
                if p >> 4 != 0 {
                    v.vib_speed = p >> 4;
                }
                if p & 15 != 0 {
                    v.vib_depth = p & 15;
                }
            }
            if fx == 5 {
                self.v[k].vs_count = 0;
            }
            // the note: its period is taken when an instrument plays it (SPEC section 5)
            let mut trig = false;
            let mut pending = None;
            let off40 = n & 0xc0 == 0x40;
            if n == 0 {
                self.c(C::RN_ZERO);
            } else if fx == 3 {
                self.c(C::RN_PORTA);
                let v = &mut self.v[k];
                v.porta_target = period(n & 0x3f);
                // the slide runs down from now to the end of the effect, past the target (SPEC section 6.3)
                v.porta_down = v.porta_target != 0 && v.porta_target < v.per;
            } else if n & 0x80 != 0 {
                self.c(C::RN_OFF80);
                trig = n & 0x3f != 0;
                let v = &mut self.v[k];
                v.ins = 0;
                v.loop_ptr = self.silent;
                v.loop_len = 1;
                // the pulse wave turns to narrowing (SPEC section 7)
                v.pwm.down = true;
            } else if off40 {
                self.c(C::RN_OFF40);
                trig = n & 0x3f != 0;
                pending = Some(n & 0x3f);
                let v = &mut self.v[k];
                v.ins = 0;
                v.loop_ptr = self.silent;
                v.loop_len = 1;
                // the pulse wave turns to narrowing (SPEC section 7)
                v.pwm.down = true;
            } else {
                self.c(C::RN_NOTE);
                trig = true;
                pending = Some(n);
            }
            // the instrument nibble (not after a note byte with bit 7)
            if ins == 0 || n & 0x80 != 0 {
                self.c(if ins == 0 { C::RI_NONE } else { C::RI_KEEP });
            } else {
                self.c(C::RI_SET);
                let (w0, w1, w2, flag) = self.record(ins);
                let lp = self.loop_of(k, ins);
                let detune = self.rb(self.rec(ins) + 6) as i8;
                let v = &mut self.v[k];
                v.ins = ins;
                v.detune = detune;
                // the loop the voice repeats is the instrument's from now, with or without a note (not
                // after a note byte with bit 6)
                if !off40 {
                    (v.loop_ptr, v.loop_len) = lp;
                }
                // (a sample instrument leaves the pulse wave going on in the buffer, unheard, but
                // turns it to narrowing, as a list's FF does)
                if flag != 0 {
                    v.pwm = Pwm { on: true, delay: w0 as u8, min: w1 as i32, max: w2 as i32, w: w1 as i32, down: false, count: 0 };
                } else {
                    v.pwm.down = true;
                }
                let kind = if flag != 0 {
                    C::RI_PULSE
                } else if w0 != 0 {
                    C::RI_SAMPLE
                } else {
                    C::RI_SAMPLE0
                };
                self.c(kind);
                if flag == 0 && w2 != 0 {
                    self.c(C::RI_LOOP);
                }
            }
            // the volume byte
            if vb < 0x80 {
                self.c(C::RV_SET);
                self.v[k].vol = vb;
            } else {
                self.c(C::RV_KEEP);
            }
            let vol = self.v[k].vol as u16;
            self.vol(hw, k, vol);
            if trig {
                mask |= 1 << k;
                let i = self.v[k].ins;
                let mut wrote = false;
                if i != 0 {
                    let (w0, _, _, flag) = self.record(i);
                    if flag != 0 || w0 != 0 {
                        if let Some(nn) = pending {
                            let v = &mut self.v[k];
                            v.note = nn;
                            v.doff = detune_off(nn, v.detune);
                            v.base = period(nn).wrapping_add(v.doff);
                            v.per = v.base;
                            v.vib_pos = 0;
                            if v.detune != 0 {
                                self.c(C::RT_DETUNE);
                            }
                        }
                        let per = self.v[k].per;
                        let (start, len) =
                            if flag != 0 { (self.buffer(k), 16) } else { (self.base + self.sample_off(i) as u32, w0) };
                        self.c(C::RT_LC);
                        self.loc(hw, k, start);
                        self.c(C::RT_LEN);
                        self.len(hw, k, len);
                        self.c(C::RT_PER);
                        self.per(hw, k, per);
                        if !off40 {
                            let lp = self.loop_of(k, i);
                            (self.v[k].loop_ptr, self.v[k].loop_len) = lp;
                        }
                        wrote = true;
                    }
                }
                self.c(if wrote { C::RT_AFTER } else { C::RT_BARE });
            } else if n != 0 && fx == 3 {
                self.c(C::RT_PORTA);
            } else {
                self.c(C::RT_NONE);
            }
            self.vol_check(hw, k, true);
            match fx {
                0 => self.c(C::RF0),
                1 => self.c(C::RF1),
                2 => self.c(C::RF2),
                3 => self.c(C::RF3),
                4 => self.c(C::RF4),
                5 => self.c(C::RF5),
                6 => self.c(C::RF6),
                7 => self.c(C::RF7),
                8 => self.c(C::RF8),
                9 => self.c(C::RF9),
                0xa => self.c(C::RFA),
                0xb => {
                    self.c(C::RFB);
                    jump = Some(p as usize);
                }
                0xc => {
                    self.c(C::RFC);
                    self.vol(hw, k, p as u16);
                    self.c(C::RFC_POST);
                }
                0xd => {
                    self.c(C::RFD);
                    brk = true;
                }
                0xe => {
                    self.c(C::RFE);
                    self.at(hw, "CIA");
                    hw.bus.write_byte(0xBFE001, 0);
                    self.c(C::RFE_MID);
                    self.at(hw, "CIA");
                    hw.bus.write_byte(0xBFE001, 0);
                    self.c(C::RFE_POST);
                }
                _ => {
                    self.c(C::RFF);
                    self.speed = p;
                }
            }
            self.c(C::R_POST);
        }
        // the next row
        if let Some(j) = jump {
            self.c(C::ADV_JMP);
            // B xx sets the position's low byte to xx - 1 and then steps on, so B 00 goes 256
            // positions on (SPEC section 6.3); the position is kept to 16 bits (a guess)
            self.pos = (((self.pos & 0xff00) | (j.wrapping_sub(1) & 0xff)) + 1) & 0xffff;
            self.row = 0;
            self.newpos = true;
        } else {
            self.c(C::ADV_ROW);
            self.row += 1;
            let brk = brk || self.fe_brk;
            if brk || self.row == ROWS {
                self.c(if brk { C::ADV_BRK } else { C::ADV_POS });
                self.broke = brk;
                self.row = 0;
                self.pos = (self.pos + 1) & 0xffff;
                self.newpos = true;
            }
        }
        self.wrap_check();
        mask
    }

    /// Only the list's end itself wraps (B can jump past it): back to the restart position, and
    /// SongEnd is called.
    fn wrap_check(&mut self) {
        if self.pos == self.lay.len {
            self.c(C::ADV_WRAP);
            self.wrapped = true;
            self.pos = self.lay.restart;
            self.songend_call = true;
        }
    }

    fn fx_tick(&mut self, hw: &mut Hw) {
        let in_row = self.count;
        for k in 0..4 {
            self.vol_check(hw, k, false);
            let (fx, p) = (self.v[k].fx, self.v[k].param);
            match fx {
                0 if p == 0 => self.c(C::F0_NONE),
                0 => {
                    let (add, c) = match in_row % 3 {
                        1 => (p & 15, C::F0_P1),
                        2 => (p >> 4, C::F0_P2),
                        _ => (0, C::F0_P0),
                    };
                    self.c(c);
                    let per = period(self.v[k].note.wrapping_add(add)).wrapping_add(self.v[k].doff);
                    self.v[k].per = per;
                    self.per(hw, k, per);
                    self.c(C::F0_POST);
                }
                1 | 2 => {
                    let v = &mut self.v[k];
                    v.per = if fx == 1 { v.per.wrapping_sub(p as u16) } else { v.per.wrapping_add(p as u16) };
                    let per = v.per;
                    self.c(if fx == 1 { C::F1 } else { C::F2 });
                    self.per(hw, k, per);
                    self.c(C::F12_POST);
                }
                3 => {
                    let v = &mut self.v[k];
                    if v.porta_down {
                        v.per = v.per.wrapping_sub(p as u16);
                        let per = v.per;
                        self.c(C::F3_ON);
                        self.per(hw, k, per);
                        self.c(C::F3_POST);
                    } else {
                        self.c(C::F3_OFF);
                    }
                }
                4 => {
                    let v = &mut self.v[k];
                    let pos = v.vib_pos;
                    let delta = (SINE[(pos & 7) as usize] * v.vib_depth as u16) >> 7;
                    let per = if pos & 0x20 != 0 { v.base.wrapping_sub(delta) } else { v.base.wrapping_add(delta) };
                    v.vib_pos = pos.wrapping_add(v.vib_speed);
                    v.per = per;
                    self.c(C::F4);
                    if p == 0 {
                        self.c(C::F4_P0);
                    } else {
                        self.c(C::F4_NZ);
                        if p >> 4 == 0 {
                            self.c(C::F4_HI0);
                        }
                        if p & 15 == 0 {
                            self.c(C::F4_LO0);
                        }
                    }
                    self.c(if pos & 0x20 != 0 { C::F4_NEG } else { C::F4_POS });
                    self.per(hw, k, per);
                    self.c(C::F4_POST);
                }
                5 => {
                    let v = &mut self.v[k];
                    if v.vs_count == 0 {
                        let (up, down) = ((p >> 4) as i32, (p & 15) as i32);
                        let (step, every) = if up != 0 { (1, up) } else { (-1, down) };
                        v.vs_count = (every - 1) as u8;
                        let nv = v.vol as i32 + step;
                        self.c(C::F5_STEP);
                        self.c(if up != 0 { C::F5_UP } else { C::F5_DN });
                        let nv = if !(0..=0x40).contains(&nv) {
                            self.c(C::F5_CLAMP);
                            nv.clamp(0, 0x40)
                        } else {
                            nv
                        };
                        self.v[k].vol = nv as u8;
                        self.vol(hw, k, nv as u16);
                        self.c(C::F5_POST);
                    } else {
                        v.vs_count -= 1;
                        self.c(C::F5_WAIT);
                    }
                }
                // effect 6 writes nothing (it only takes time, less with parameter 0)
                6 => self.c(if p == 0 { C::F6_P0 } else { C::F6 }),
                0xa => {
                    let per = self.v[k].per;
                    self.c(C::FA);
                    self.per(hw, k, per);
                    let v = &mut self.v[k];
                    let (up, down) = ((p >> 4) as i32, (p & 15) as i32);
                    let nv = v.vol as i32 + if up != 0 { up } else { -down };
                    self.c(C::FA_MID);
                    self.c(if up != 0 { C::FA_UP } else { C::FA_DN });
                    let nv = if !(0..=0x40).contains(&nv) {
                        self.c(C::FA_CLAMP);
                        nv.clamp(0, 0x40)
                    } else {
                        nv
                    };
                    self.v[k].vol = nv as u8;
                    self.vol(hw, k, nv as u16);
                    self.c(C::FA_POST);
                }
                _ => {
                    let per = self.v[k].per;
                    self.c(C::FP);
                    self.per(hw, k, per);
                    self.c(C::FP_POST);
                }
            }
            self.c(C::F_POST);
        }
    }
}

impl Replayer for PowerTracker {
    fn format(&self) -> &'static str {
        "PowerTracker"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        if COST_LOG {
            log::trace!(target: "pt_cost", "TICK {}", hw.bus.clock);
        }
        self.epoch(hw);
        self.songend_call = false;
        self.newpos = false;
        self.wrapped = false;
        self.broke = false;
        self.fe_brk = false;
        let mut mask = 0;
        if self.count == 0 {
            self.c(C::TICK_ROW);
            mask = self.row_tick(hw);
        } else {
            self.c(C::TICK_FX);
            self.fx_tick(hw);
            if self.fe_brk {
                self.c(C::FADV_BRK);
                self.broke = true;
                self.row = 0;
                self.pos = (self.pos + 1) & 0xffff;
                self.newpos = true;
                self.wrap_check();
            }
        }
        self.count += 1;
        if self.count >= self.speed.max(1) {
            self.count = 0;
        }
        for k in 0..4 {
            self.pwm_step(hw, k);
        }
        if mask != 0 {
            self.c(C::E_MASK);
            self.dmacon(hw, mask);
            self.dma_on = 0x8000 | mask;
            self.c(C::E_DMA_POST);
        } else {
            self.c(C::E_NOMASK);
        }
        if self.wrapped {
            self.c(C::E_WRAP);
            if self.broke {
                self.c(C::E_BRK_WRAP);
            }
        } else if self.newpos {
            self.c(C::E_NEWPOS);
            if self.broke {
                self.c(C::E_BRK);
            }
        }
        if self.songend_call {
            // the host's dtg_SongEnd takes no time, but the clock restarts from its call
            self.at(hw, "SONGEND");
            self.songend = true;
            self.epoch(hw);
            self.c(C::E_SONGEND_POST);
        }
        self.wait_dma(hw);
        self.c(C::S2_DMA);
        self.dmacon(hw, self.dma_on);
        self.c(C::S2_WAIT);
        self.wait_dma(hw);
        for k in 0..4 {
            let (a, l) = (self.v[k].loop_ptr, self.v[k].loop_len);
            self.c(if k == 0 { C::S3_LC } else { C::S3_NEXT });
            self.loc(hw, k, a);
            self.c(C::S3_LEN);
            self.len(hw, k, l);
        }
        if COST_LOG {
            let s: String = (0..128).map(|i| format!("{:02x}", hw.bus.read_byte(self.silent + 2 + i))).collect();
            log::trace!(target: "pt_buf", "{s}");
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    Some(Box::new(build(d, base, m)?))
}

fn build(d: &[u8], base: u32, m: &mut Machine) -> Option<PowerTracker> {
    let lay = layout(d)?;
    if d.len() < lay.tracks {
        return None;
    }
    // the silent word, then the four voices' buffers
    let silent = m.alloc(2 + 4 * 32);
    if silent == 0 {
        return None;
    }
    log::debug!("PowerTracker: silent word and buffers at {silent:#x}");
    // the init's length: per position, per record, per new maximum of the list bytes
    let mut max = 0u8;
    let mut news = 0u64;
    for &b in &d[lay.lists..lay.tracks] {
        if b > max {
            max = b;
            news += 1;
        }
    }
    let init = K[C::INIT0 as usize]
        + K[C::INIT_POS as usize] * lay.len as u64
        + K[C::INIT_REC as usize] * lay.nrec as u64
        + K[C::INIT_MAX as usize] * news;
    // the samples follow the tracks up to the highest list byte (counted in a byte: FF gives none)
    let lay_samples = lay.tracks + 256 * (max.wrapping_add(1) as usize);
    let p = PowerTracker {
        d: d.to_vec(),
        lay,
        base,
        silent,
        samples: lay_samples,
        v: [Voice::default(); 4],
        speed: 6,
        count: 0,
        pos: 0,
        row: 0,
        dma_on: 0,
        songend: false,
        songend_call: false,
        fe_brk: false,
        newpos: false,
        wrapped: false,
        broke: false,
        epoch: 0,
        t: 0,
        n: [0; C::N as usize],
    };
    // the original's init takes this long; the song (the audio's origin, the timer's start)
    // begins at its end, at the clock `new` leaves
    m.bus.clock += init;
    let mut hw = Hw { bus: &mut m.bus };
    hw.bus.write_bytes(silent, &[0, 0]);
    // the buffers as the reference holds them before any note (SPEC section 7)
    for (k, w) in [1, 1, 13, 32].into_iter().enumerate() {
        p.shape(&mut hw, k, w);
    }
    Some(p)
}

#[cfg(test)]
#[path = "powertracker_tests.rs"]
mod tests;
