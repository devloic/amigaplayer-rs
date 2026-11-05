//! Thomas Hermann (1989 game driver; Eagleplayer "Thomas Hermann", module prefix `thm.`, sample
//! companion `smp.`; games such as Beam, Blue Angel 69 and Nightdawn).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player and from black-box runs of it; no original code and no bytes of it.
//! A module is plain data: four pattern pointer lists of 256 positions, four transposition tables,
//! a table of 48-byte instrument records (the samples are in the companion file, or one file per
//! sample) and patterns of rows of three bytes (note, instrument, flags).
//!
//! Two periodic interrupts run: the Play tick (period from the module's timer word) that counts
//! the speed, reads a row (stopping the voices that start a note), then starts the notes with the
//! odd 16-bit instrument-index pollution the specification describes, and the slide/envelope tick
//! (a fixed 137240 colour clocks) that runs portamento slides and, every fourth call, the
//! envelopes. Four audio interrupts, served by one handler that serves the highest pending
//! channel, reload the loop of a looping sample or end a one-shot. Every write is placed at the
//! clock of the original's code, from the specification's timing figures.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): bytes read
//! outside the file or the guest memory read as zero; a pattern pointer outside the file reads
//! zero rows; a record address outside the memory is an all-zero record; the sample
//! files that cannot be found leave their samples as silence (zero bytes).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Thomas Hermann", detect, new };

/// Colour clocks of the slide/envelope tick.
const TB_PERIOD: u64 = 137240;
/// Colour clocks between two sound initialisations (a change of subsong).
const INIT_STEP: u64 = 1896;
/// Phase of the first Play tick after the second timer's start.
const PHASE_A: u64 = 155;
/// The phase for a module whose timer word is not the default one, and the colour clocks the
/// handler's writes come earlier than for the default.
const PHASE_B: u64 = 190;
const SHIFT_B: u64 = 35;

/// Base periods of the twelve notes.
const BASE: [u16; 12] = [0x1AC, 0x194, 0x17C, 0x168, 0x154, 0x140, 0x12E, 0x11E, 0x10E, 0xFE, 0xF0, 0xE2];

/// Gaps of a note's writes after the location writes, by voice: to the length, to the period,
/// to the volume.
const G0: [u64; 4] = [80, 92, 105, 109];
const G1: [u64; 4] = [128, 141, 153, 157];
const G2: [u64; 4] = [251, 284, 313, 335];

const LINE: u64 = 227;
/// Colour clocks a voice's row costs when it writes nothing (and for each voice after the last
/// writing one).
const SKIP: u64 = 60;
/// Colour clocks from the host calling the audio handler to its first write.
const ENTRY: u64 = 59;
/// Colour clocks from the audio handler's last write to its return.
const TAIL: u64 = 54;

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// The module's fixed header layout, when the file has it.
struct Layout {
    origin: u32,
    /// Offset of the pattern pointer list of voice 0 (the lists follow each other).
    songsize: usize,
}

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() <= 6848 {
        return None;
    }
    let origin = be32(d, 46);
    if origin == 0 || origin & 0x8000_0000 != 0 || origin & 1 != 0 {
        return None;
    }
    if be32(d, 0).wrapping_sub(origin) != 64 {
        return None;
    }
    for i in 1..5 {
        if be32(d, 4 * i) != be32(d, 4 * i - 4).wrapping_add(1024) {
            return None;
        }
    }
    for i in 5..8 {
        if be32(d, 4 * i) != be32(d, 4 * i - 4).wrapping_add(256) {
            return None;
        }
    }
    let mut max = 0usize;
    for i in 0..1024 {
        let p = be32(d, 64 + 4 * i).wrapping_sub(origin) as usize;
        if p >= d.len() {
            return None;
        }
        max = max.max(p);
    }
    let songsize = max + 64;
    if songsize > d.len() {
        return None;
    }
    Some(Layout { origin, songsize })
}

/// A subsong's overrides: loop position, last position, first position, speed.
type Sub = (u8, u8, u8, u8);

const SUBS3: [Sub; 3] = [(0x00, 0x6F, 0xFE, 3), (0x80, 0x87, 0x7F, 3), (0x89, 0x89, 0x88, 3)];
const SUBS5: [Sub; 5] =
    [(0x11, 0x5A, 0xFF, 3), (0x60, 0x85, 0x5F, 5), (0x90, 0xB5, 0x8F, 5), (0xC0, 0xC7, 0xBF, 3), (0xD5, 0xD6, 0xCF, 3)];

/// What is read from an instrument record.
#[derive(Clone, Copy, Default)]
struct Rec {
    len: u32,
    vol: u8,
    mode: u8,
    addr: u32,
    lstart: u16,
    llen: u16,
    alt: u16,
    flag: u8,
    atk: u8,
    t1: u8,
    t2: u8,
    t3: u8,
    l1: u8,
    l2: u8,
    l3: u8,
    slide: u8,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    /// The record's flag byte as copied at the note start; 0 is inactive.
    active: u8,
    /// The same, as the audio interrupt sees it (not cleared by a stop).
    rflag: u8,
    release: bool,
    irq_stage: u8,
    level: u16,
    rate: i16,
    estage: u8,
    maxvol: u8,
    slide_on: bool,
    cur: u16,
    tgt: u16,
    loop_addr: u32,
    loop_len: u16,
    rec: Rec,
    row: [u8; 3],
}

pub struct ThomasHermann {
    d: Vec<u8>,
    base: u32,
    origin: u32,
    /// Offsets in the module of the pattern pointer list and the transposition table of a voice.
    lists: [usize; 4],
    trans: [usize; 4],
    /// Address in guest memory of the instrument pointer table.
    table: u32,
    rows_last: u8,
    p_period: u64,
    disable: [u8; 4],
    subs: Vec<Sub>,
    sub: usize,
    // the subsong in use
    loop_pos: u8,
    last_pos: u8,
    speed: u8,
    counter: u8,
    pos: u8,
    row: u8,
    v: [Voice; 4],
    left: u32,
    songend: bool,
    // scheduling
    ts0: u64,
    play_due: u64,
    tb_due: u64,
    tb_calls: u64,
    phase: u64,
    first_due: u64,
    /// The writes of an on-time Play tick come this many colour clocks before the handler's start.
    shift: u64,
}

fn read_rec(hw: &Hw, a: u32) -> Rec {
    let b = &hw.bus;
    Rec {
        len: ((b.r16(a.wrapping_sub(2)) as u32) << 16) | b.r16(a) as u32,
        vol: b.r8(a + 2),
        mode: b.r8(a + 3),
        addr: b.r32(a + 4),
        lstart: b.r16(a + 8),
        llen: b.r16(a + 10),
        alt: b.r16(a + 12),
        flag: b.r8(a + 0x23),
        atk: b.r8(a + 0x24),
        t1: b.r8(a + 0x25),
        t2: b.r8(a + 0x26),
        t3: b.r8(a + 0x27),
        l1: b.r8(a + 0x28),
        l2: b.r8(a + 0x29),
        l3: b.r8(a + 0x2A),
        slide: b.r8(a + 0x2D),
    }
}

fn w16(hw: &mut Hw, a: u32, v: u16) {
    m68k::AddressBus::write_word(hw.bus, a, v);
}

fn at(hw: &mut Hw, t: u64) {
    hw.bus.clock = t;
}

/// The volume written for an envelope level byte.
fn scaled(maxvol: u8, level: u8) -> u16 {
    (((maxvol as u32) * (level as u32)) >> 6) as u16 & 0x7F
}

/// A signed 16-bit division of a 32-bit value; when the quotient does not fit, the low 16 bits of
/// the dividend.
fn div16(n: i32, d: i32) -> i16 {
    let q = n / d.max(1);
    if q >= -32768 && q <= 32767 {
        q as i16
    } else {
        n as i16
    }
}

/// The time of the interrupt-enable write after the volume write at `v`: the driver waits for
/// the raster line to change nine times, polling every 12.5 colour clocks.
fn wait_time(v: u64) -> u64 {
    let b = ((v + 53) / LINE + 9) * LINE;
    let need = 2 * (b.saturating_sub(v + 5));
    let n = need.div_ceil(25);
    (2 * v + 92 + 25 * n + 1) / 2
}

impl ThomasHermann {
    fn byte(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn set_sub(&mut self, n: usize) {
        let n = n.min(self.subs.len() - 1);
        self.sub = n;
        let (lp, last, first, speed) = self.subs[n];
        self.loop_pos = lp;
        self.last_pos = last;
        self.speed = speed;
        self.counter = speed;
        self.pos = first;
        self.row = 0;
        self.v = [Voice::default(); 4];
        self.left = 0;
        self.songend = false;
        self.tb_calls = 0;
    }

    /// Starts the timers of the sound initialisation at subsong `n`.
    fn start_timers(&mut self, n: usize) {
        let ts = self.ts0 + INIT_STEP * n as u64;
        self.play_due = ts + self.phase + self.p_period;
        self.tb_due = ts / 5 * 5 + TB_PERIOD;
        self.first_due = self.play_due;
    }

    // ---- the Play tick ----

    fn play(&mut self, hw: &mut Hw, on_time: bool) {
        let e = hw.bus.clock - if on_time { self.shift.min(hw.bus.clock) } else { 0 };
        if self.counter == 0 {
            self.counter = 0xFF;
            self.load_tick(hw, e);
        } else if self.counter & 0x80 != 0 {
            self.counter = self.speed;
            self.start_tick(hw, e);
            self.advance();
        } else {
            self.counter -= 1;
            at(hw, e + 150);
        }
    }

    /// Reads the row of every voice (with the transposition) and stops the voices that start a
    /// note in it.
    fn load_tick(&mut self, hw: &mut Hw, e: u64) {
        let mut stop = [false; 4];
        // the extra colour clocks a voice's row takes: the transposition of a note costs 35,
        // and 3 more when it falls below the first note, 13 when it passes the last
        let mut extra = [0u64; 4];
        for v in 0..4 {
            let p = be32(&self.d, self.lists[v] + 4 * self.pos as usize).wrapping_sub(self.origin) as usize;
            let a = p.wrapping_add(3 * self.row as usize);
            let (mut b0, b1, b2) = (self.byte(a), self.byte(a + 1), self.byte(a + 2));
            if b0 < 0xC0 {
                let tr = self.byte(self.trans[v] + self.pos as usize) as i8;
                let note = (b0 >> 4) as i32 + tr as i32;
                extra[v] = 35 + if note < 0 { 3 } else if note >= 12 { 13 } else { 0 };
                b0 = transpose(b0, tr);
            }
            self.v[v].row = [b0, b1, b2];
            stop[v] = b2 & 2 == 0 && (b0 < 0xC0 || b0 == 0xCF);
        }
        if !stop.iter().any(|&s| s) {
            at(hw, e + 249);
            return;
        }
        // `cur` is where voice `v`'s code would reach its stop for a row without extra costs
        let mut cur = e + 499;
        let mut last = (0u64, 0usize);
        for v in 0..4 {
            if !stop[v] {
                cur += extra[v] + 239;
                continue;
            }
            let t = cur + extra[v];
            at(hw, t);
            hw.dmacon(1 << v);
            at(hw, t + 15);
            w16(hw, 0xDFF09A, 0x80 << v);
            self.v[v].active = 0;
            cur = t + 15 + 270;
            last = (t + 15, v);
        }
        at(hw, last.0 + 220 + SKIP * (3 - last.1) as u64);
    }

    /// The colour clocks the start tick spends on voices `from..to` that write nothing: 60 each,
    /// 11 for a voice the module disables.
    fn skip_cost(&self, from: usize, to: usize) -> u64 {
        (from..to).map(|u| if self.disable[u] != 0 { 11 } else { SKIP }).sum()
    }

    fn start_tick(&mut self, hw: &mut Hw, e: u64) {
        self.left = 0;
        // the writing voice before, the time its next voice's note start would come, and how the
        // handler ends: the time of the last write, its voice and whether it was a stop
        let mut prev: Option<usize> = None;
        let mut last: Option<(u64, usize, bool)> = None;
        for v in 0..4 {
            if self.disable[v] != 0 {
                continue;
            }
            let [b0, b1, b2] = self.v[v].row;
            self.left = (self.left & !0xFF) | b1 as u32;
            match b0 {
                0xFF => {}
                0xDF => self.v[v].release = true,
                0xEF => {
                    let t = match last {
                        // the first writer: a voice that writes nothing before it costs 76 and 60 each
                        None => e + 385 + self.skip_cost(0, v),
                        Some((t, p, stop)) => t + if stop { 129 } else { 116 } + self.skip_cost(p + 1, v),
                    };
                    at(hw, t);
                    hw.dmacon(1 << v);
                    at(hw, t + 15);
                    w16(hw, 0xDFF09A, 0x80 << v);
                    self.v[v].active = 0;
                    prev = Some(v);
                    last = Some((t + 15, v, true));
                }
                0xC0..=0xFE if b0 != 0xCF => {
                    self.left = (self.left & 0xFFFF_0000) | ((self.left & 0xFFFF) * 4 & 0xFFFF);
                }
                _ => {
                    let cost = period_cost(b0);
                    let c = match last {
                        None => e + 568 + self.skip_cost(0, v),
                        Some((t, p, stop)) => t + if stop { 175 } else { 299 } + self.skip_cost(p + 1, v),
                    };
                    if let Some(dma_t) = self.note(hw, v, b0, b2, (c as i64 + cost) as u64) {
                        prev = Some(v);
                        last = Some((dma_t, v, false));
                    }
                }
            }
        }
        let _ = prev;
        match last {
            None => at(hw, e + 150),
            Some((t, v, stop)) => at(hw, t + if stop { 220 } else { 206 } + self.skip_cost(v + 1, 4)),
        }
    }

    /// A note start at `c` (the ADKCON write). Returns the time of the last write when it was
    /// accepted.
    fn note(&mut self, hw: &mut Hw, v: usize, b0: u8, b2: u8, c: u64) -> Option<u64> {
        let rn = self.left & 0xFFFF;
        let hi = self.left >> 16;
        let off = (hi << 16).wrapping_add((rn * 4) & 0xFFFF);
        let ta = self.table.wrapping_add(off);
        let mut ra = hw.bus.r32(ta);
        if off < 128 {
            ra = ra.wrapping_sub(self.origin).wrapping_add(self.base);
        }
        let rec = read_rec(hw, ra);
        let nib = (b0 >> 4) as usize;
        let oct = b0 & 15;
        let base = BASE.get(nib).copied().unwrap_or(0) as u32;
        let mut per: u32;
        let mut addr = rec.addr;
        let mut len = rec.len;
        let mut ls = rec.lstart as u32;
        let mut ll = rec.llen as u32;
        let mut hi_update: Option<u32> = None;
        // rejection leaves the high word and sets the low word
        let reject = |this: &mut Self, low: u32| {
            this.left = (this.left & 0xFFFF_0000) | (low & 0xFFFF);
            None::<u64>
        };
        if b0 == 0xCF {
            per = rec.alt as u32;
            if rec.mode != 1 {
                addr = addr.wrapping_add(len);
                len = len.wrapping_mul(2);
                ls = ls.wrapping_mul(2);
                ll = ll.wrapping_mul(2);
                hi_update = Some(ls >> 16);
            }
        } else if rec.mode == 1 {
            match oct {
                1 => per = base,
                2 => {
                    per = base / 2;
                    if per <= 0x82 {
                        return reject(self, per);
                    }
                }
                0 => per = base * 2,
                15 => per = base * 4,
                _ => return reject(self, base),
            }
        } else {
            match oct {
                3 => {
                    per = base / 2;
                    if per <= 0x82 {
                        return reject(self, per);
                    }
                }
                2 => per = base,
                1 => {
                    per = base;
                    addr = addr.wrapping_add(len);
                    len = len.wrapping_mul(2);
                    ls = ls.wrapping_mul(2);
                    ll = ll.wrapping_mul(2);
                    hi_update = Some(ls >> 16);
                }
                0 | 15 => {
                    per = if oct == 15 { base * 2 } else { base };
                    addr = addr.wrapping_add(len.wrapping_mul(3));
                    len = len.wrapping_mul(4);
                    ls = ls.wrapping_mul(4);
                    ll = ll.wrapping_mul(4);
                    hi_update = Some(ls >> 16);
                }
                _ => return reject(self, base),
            }
        }
        per &= 0xFFFF;
        let porta = b2 & 1 != 0;
        // the register writes
        at(hw, c);
        w16(hw, 0xDFF09E, 0x00FF);
        let t_loc = c + 48;
        at(hw, t_loc);
        hw.loc(v, addr);
        let t_len = t_loc + G0[v];
        at(hw, t_len);
        hw.len(v, (len >> 1) as u16);
        let mut t = t_len + G1[v] + if b0 == 0xCF { 6 } else { 0 };
        if !porta {
            at(hw, t);
            hw.per(v, per as u16);
        }
        t += G2[v] + if rec.flag != 0 { 125 } else { 0 };
        // the voice's state and the volume
        let vs = &mut self.v[v];
        vs.rec = rec;
        vs.rflag = rec.flag;
        vs.active = rec.flag;
        let mut low = 0u32;
        let vol: u16;
        if rec.flag == 0 {
            vol = (rec.vol & 0x7F) as u16;
            low = vol as u32;
        } else {
            vs.loop_addr = addr.wrapping_add(ls);
            vs.loop_len = (ll >> 1) as u16;
            vs.estage = 0;
            vs.maxvol = rec.vol;
            if rec.flag == 1 {
                vs.level = 0x4000;
                vol = (rec.vol & 0x7F) as u16;
                low = vol as u32;
            } else if rec.atk == 0 {
                vs.estage = 1;
                vs.level = (rec.l1 as u16) << 8;
                vol = scaled(rec.vol, rec.l1);
            } else {
                vol = 0;
            }
        }
        vs.irq_stage = 0;
        vs.release = false;
        if porta {
            vs.tgt = per as u16;
            vs.slide_on = true;
        } else {
            vs.slide_on = false;
            vs.cur = per as u16;
        }
        let _ = oct;
        at(hw, t);
        hw.vol(v, vol);
        let ti = wait_time(t);
        at(hw, ti);
        w16(hw, 0xDFF09A, 0x8000 | 0x80 << v);
        let td = ti + 21;
        at(hw, td);
        hw.dmacon(0x8200 | 1 << v);
        let hi_word = match hi_update {
            Some(h) => h,
            None => self.left >> 16,
        };
        self.left = (hi_word << 16) | (low & 0xFFFF);
        Some(td)
    }

    /// The row counter after a start tick.
    fn advance(&mut self) {
        if self.row < self.rows_last {
            self.row += 1;
            return;
        }
        self.row = 0;
        if self.pos == self.last_pos {
            self.pos = self.loop_pos;
            self.songend = true;
        } else {
            self.pos = self.pos.wrapping_add(1);
        }
    }

    // ---- the slide/envelope tick ----

    fn tb(&mut self, hw: &mut Hw) {
        let e = hw.bus.clock;
        self.tb_calls += 1;
        let mut tau = 2 * e + 333; // half colour clocks
        for v in 0..4 {
            let vs = &mut self.v[v];
            if !vs.slide_on {
                tau += 75;
                continue;
            }
            let step = vs.rec.slide as u16;
            let done;
            if vs.cur < vs.tgt {
                vs.cur = vs.cur.wrapping_add(step);
                done = vs.cur >= vs.tgt;
            } else if vs.cur > vs.tgt {
                vs.cur = vs.cur.wrapping_sub(step);
                done = vs.cur <= vs.tgt || (vs.cur as u32 + step as u32) < step as u32;
            } else {
                done = true;
            }
            if done {
                vs.cur = vs.tgt;
                vs.slide_on = false;
            }
            let p = vs.cur;
            at(hw, (tau + 1) / 2);
            hw.per(v, p);
            tau += 224;
        }
        if self.tb_calls % 4 == 0 {
            for v in 0..4 {
                match self.envelope(v) {
                    None => tau += 112,
                    Some(vol) => {
                        at(hw, (tau + 523 + 1) / 2);
                        hw.vol(v, vol);
                        tau += 912;
                    }
                }
            }
        }
        at(hw, (tau + 1) / 2 + 100);
    }

    /// One envelope step of voice `v`; the volume to write, if any.
    fn envelope(&mut self, v: usize) -> Option<u16> {
        let vs = &mut self.v[v];
        if vs.active == 0 {
            return None;
        }
        let r = vs.rec;
        if vs.release {
            if vs.estage != 4 && vs.estage != 5 {
                vs.estage = 4;
                let d = r.t3.max(1) as i32;
                vs.rate = div16(-(vs.level as i32), d);
            }
            // the generic step with target 0, next 0, time 1
            vs.level = vs.level.wrapping_add(vs.rate as u16);
            let lb = (vs.level >> 8) as u8;
            let reached = if vs.rate < 0 { (lb as i8) <= 0 } else { true };
            let out;
            if reached {
                vs.level = 0;
                out = scaled(vs.maxvol, 0);
                vs.rate = 0;
                vs.estage = 5;
                vs.active = 0;
            } else {
                out = scaled(vs.maxvol, lb);
            }
            return Some(out);
        }
        if vs.rflag != 0xFF && vs.active != 0xFF {
            return None;
        }
        match vs.estage {
            0 => {
                let n = r.atk.max(1) as i32;
                vs.rate = div16((r.l1 as i32) * 256, n);
                vs.level = vs.rate as u16;
                vs.estage = 1;
                Some(scaled(vs.maxvol, (vs.level >> 8) as u8))
            }
            1..=3 => {
                let (t, n, d) = match vs.estage {
                    1 => (r.l1, r.l2, r.t1),
                    2 => (r.l2, r.l3, r.t2),
                    _ => (r.l3, 0, r.t3),
                };
                let d = d.max(1) as i32;
                vs.level = vs.level.wrapping_add(vs.rate as u16);
                let lb = (vs.level >> 8) as u8;
                let reached = if vs.rate < 0 { (lb as i8) <= t as i8 } else { lb >= t };
                if reached {
                    vs.level = (t as u16) << 8;
                    let out = scaled(vs.maxvol, t);
                    vs.rate = div16(((n as i32) - (t as i32)) << 8, d);
                    vs.estage += 1;
                    Some(out)
                } else {
                    Some(scaled(vs.maxvol, lb))
                }
            }
            _ => None,
        }
    }

    // ---- the audio interrupt ----

    fn audio(&mut self, ch: usize, hw: &mut Hw) {
        let bit = 0x80u16 << ch;
        let p = &hw.bus.paula;
        if p.intreq & bit == 0 {
            // an earlier handler of the round already served it
            return;
        }
        let active = p.intena & p.intreq & (0xF << 7);
        if active == 0 {
            return;
        }
        // the handler serves the highest pending channel
        let hc = (0..4).rev().find(|&c| active & (0x80 << c) != 0).unwrap_or(ch);
        let t = hw.bus.clock + ENTRY;
        let end = self.serve(hc, hw, t);
        at(hw, end);
    }

    /// Serves the interrupt of channel `hc` from time `e`; the time the handler ends.
    fn serve(&mut self, hc: usize, hw: &mut Hw, e: u64) -> u64 {
        let hb = 0x80u16 << hc;
        at(hw, e);
        w16(hw, 0xDFF09C, hb);
        let vs = &mut self.v[hc];
        let mut last = e;
        if vs.rflag != 0 {
            if vs.irq_stage == 0 {
                vs.irq_stage = 1;
            } else {
                let (la, ll) = (vs.loop_addr, vs.loop_len);
                vs.irq_stage = 2;
                let t = e + 70;
                at(hw, t);
                hw.loc(hc, la);
                hw.len(hc, ll);
                at(hw, t + 10);
                w16(hw, 0xDFF09A, hb);
                last = t + 10;
            }
        } else if vs.irq_stage < 2 {
            vs.irq_stage += 1;
        } else {
            vs.irq_stage = 0;
            at(hw, e + 10);
            w16(hw, 0xDFF09A, hb);
            at(hw, e + 20);
            hw.dmacon(0x0001 << hc);
            last = e + 20;
        }
        last + TAIL
    }
}

/// The extra colour clocks a note's period calculation takes before its first write, by the
/// octave code (the fixed-pitch row skips the calculation).
fn period_cost(b0: u8) -> i64 {
    if b0 == 0xCF {
        return -76;
    }
    match b0 & 15 {
        0 | 2 => 23,
        15 => 33,
        _ => 0,
    }
}

/// The note byte after the transposition by `tr` half-steps.
fn transpose(b0: u8, tr: i8) -> u8 {
    let note = (b0 >> 4) as i32 + tr as i32;
    let mut nib = note & 15;
    let mut oct = (b0 & 15) as i32;
    if note < 0 {
        nib -= 4;
        oct -= 1;
    } else if note >= 12 {
        nib += 4;
        oct += 1;
    }
    (((nib as u8) << 4) | (oct as u8 & 15)) as u8
}

impl Replayer for ThomasHermann {
    fn format(&self) -> &'static str {
        "Thomas Hermann"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        self.p_period
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subs.len() as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.set_sub(n as usize);
        self.start_timers(self.sub);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        Some(self.first_due.min(self.tb_due))
    }

    fn next_tick_at(&self) -> Option<u64> {
        Some(self.play_due.min(self.tb_due))
    }

    fn tick(&mut self, hw: &mut Hw) {
        let now = hw.bus.clock;
        // the slide/envelope timer is served first when both are due
        if self.tb_due <= now {
            self.tb_due += TB_PERIOD;
            self.tb(hw);
        }
        if self.play_due <= now {
            let on_time = hw.bus.clock <= self.play_due + 2;
            self.play_due += self.p_period;
            self.play(hw, on_time);
        }
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        self.audio(ch, hw);
        true
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    let origin = lay.origin;
    let ninstr = (d[35] as usize).clamp(1, 31);
    let tbl_off = be32(d, 36).wrapping_sub(origin) as usize;
    let table = base.wrapping_add(tbl_off as u32);
    let lists = [be32(d, 0), be32(d, 4), be32(d, 8), be32(d, 12)].map(|p| p.wrapping_sub(origin) as usize);
    let trans = [be32(d, 16), be32(d, 20), be32(d, 24), be32(d, 28)].map(|p| p.wrapping_sub(origin) as usize);
    let rec_off = |i: usize| be32(d, tbl_off + 4 * i).wrapping_sub(origin) as usize;
    // the samples: the companion file, or one file per sample
    let file = m.bus.read_cstr(m.deli.file_buf);
    let leaf = file.rsplit(['/', ':']).next().unwrap_or(&file).to_string();
    let comp = leaf
        .get(..4)
        .filter(|p| p.eq_ignore_ascii_case("thm."))
        .and_then(|_| crate::deli::read_companion(m, &format!("smp.{}", &leaf[4..])).1.ok());
    let rec_len = |i: usize| -> u32 {
        let o = rec_off(i);
        ((be16(d, o.wrapping_sub(2)) as u32) << 16) | be16(d, o) as u32
    };
    // the sample bytes of each instrument, the header skipped and the size of the file it came from
    let mut files: Vec<(Vec<u8>, u32, u32)> = Vec::new();
    let mut cpos = 0usize;
    for i in 1..=ninstr {
        let len = rec_len(i).min(0x100000) as usize;
        let ro = rec_off(i);
        match &comp {
            Some(c) => {
                let s = c.get(cpos..).unwrap_or(&[]);
                files.push((s[..len.min(s.len())].to_vec(), 0, len as u32));
                cpos += len;
            }
            None => {
                let name: Vec<u8> = d.get(ro + 14..ro + 36).unwrap_or(&[]).iter().copied().take_while(|&b| b != 0).collect();
                let name = String::from_utf8_lossy(&name).to_string();
                let leaf = name.rsplit(['/', ':']).next().unwrap_or(&name).to_string();
                match crate::deli::read_companion(m, &format!("Instruments/{leaf}")).1.ok() {
                    Some(f) => {
                        let hdr = if f.starts_with(b"FORM") && f.len() > 104 { 104 } else { 0 };
                        let s = &f[hdr..];
                        files.push((s[..len.min(s.len())].to_vec(), hdr as u32, f.len() as u32));
                    }
                    None => files.push((Vec::new(), 0, 0)),
                }
            }
        }
    }
    // one buffer for the companion; for single files, each in a block of its own size plus the
    // allocator's 16 bytes, rounded up to 8 (as the original's loader lays them out)
    let pitch = |fs: u32| (fs.saturating_add(16) + 7) & !7;
    let total = if comp.is_some() {
        files.iter().fold(0u32, |a, f| a.saturating_add(f.2))
    } else {
        files.iter().fold(0u32, |a, f| a.saturating_add(pitch(f.2)))
    };
    let sbase = m.alloc(total.max(4) + 64);
    if sbase == 0 {
        return None;
    }
    let mut buf = sbase;
    for (i, (data, hdr, fs)) in files.iter().enumerate() {
        let a = buf.wrapping_add(*hdr);
        m.bus.write_bytes(a, data);
        // the record's sample address field
        let rec_addr = base.wrapping_add(rec_off(i + 1) as u32);
        m.bus.w32(rec_addr.wrapping_add(4), a);
        buf = buf.wrapping_add(if comp.is_some() { *fs } else { pitch(*fs) });
    }
    let timer = be16(d, 42);
    let p_period = (timer as u64 + 1) * 5;
    let subs: Vec<Sub> = match lay.songsize {
        11792 => SUBS5.to_vec(),
        10112 => SUBS3.to_vec(),
        _ => {
            let first = if d[34] & 0x80 != 0 { d[34] } else { 0xFF };
            vec![(d[32], d[33], first, d[44])]
        }
    };
    let c0 = m.bus.clock;
    // start-up head before the first host tick: data dependent in the original (measured 45069 and
    // 45134 with a companion, 65330 and 76983 with per-sample files); a typical value is used
    let head: u64 = if comp.is_some() { 45100 } else { 71000 };
    let phase: u64 = if timer == 0x4000 { PHASE_A } else { PHASE_B };
    let ts0 = c0.saturating_sub(crate::engine::START_PHASE) + head;
    let mut t = ThomasHermann {
        d: d.to_vec(),
        base,
        origin,
        lists,
        trans,
        table,
        rows_last: d[41],
        p_period,
        disable: [d[50], d[51], d[52], d[53]],
        subs,
        sub: 0,
        loop_pos: 0,
        last_pos: 0,
        speed: 0,
        counter: 0,
        pos: 0,
        row: 0,
        v: [Voice::default(); 4],
        left: 0,
        songend: false,
        ts0,
        play_due: 0,
        tb_due: 0,
        tb_calls: 0,
        phase,
        first_due: 0,
        shift: if timer == 0x4000 { 0 } else { SHIFT_B },
    };
    t.set_sub(0);
    t.start_timers(0);
    // the host starts its timer one period before the first tick
    m.bus.clock = t.play_due - p_period;
    Some(Box::new(t))
}

#[cfg(test)]
#[path = "thomashermann_tests.rs"]
mod tests;
