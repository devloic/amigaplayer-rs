//! AProSys (`aps.*`; Petter A. Urkedal, 1989-1990; DeliTracker/UADE player "AProSys").
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player; no original code. A module is pure data: it starts with `ADRVPACK`
//! and holds nine length-prefixed chunks (sequence, blocks, track rows, arpeggio bytes, instruments,
//! envelope points, event records, sample data and one the replayer never reads). Four voices; every
//! tick runs, per voice, the arpeggio or portamento, four envelopes, the pitch and volume
//! routings and the voice's event: a sample whose start moves with an envelope, or a synthetic
//! waveform built into one of two 128-byte chip buffers that are handed to Paula alternately
//! (generators, mixers of one or two source samples, a comb and two filter variants). Sample
//! chains (loop, one-shot, chain, stop) are played by the voice's audio interrupt handler.
//!
//! The player has no data of its own besides the 48-entry period table, which is coded here as
//! constants from the specification's text; nothing is embedded. Every module known when this port
//! was written is synthetic (no real AProSys file was available), built from the specification.
//!
//! Choices where the specification is silent or the original is host dependent (all safe, none can
//! panic or hang): period table indices 48, 49 and above 53 give period 0; an envelope number past
//! voice 3 reads as 0; mode 2 operations 0, 8 and 10-15 leave the buffer as it is; a comb stage
//! whose length is not a multiple of 4 is skipped (the original never leaves it); a running sum
//! (mode 2 operation 7) is a 16-bit word; a voice that has not played a note yet reads instrument
//! 0 and event record 0; a sequence that executes more than 4096 entries without reaching a play
//! entry, or an advance that loops more than 100000 times, ends the song; after the song end the
//! voice updates go on every tick (as the original's do) but the sequencer never runs again.
//!
//! Register writes are placed at colour clocks from the measured gaps in the specification.

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "AProSys", detect, new };

/// The 48 note periods.
const PERIODS: [u16; 48] = [
    1710, 1614, 1524, 1438, 1357, 1281, 1209, 1141, 1077, 1017, 960, 906, 855, 807, 762, 719, 679,
    641, 605, 571, 539, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226,
    214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

const INTENA: u32 = 0xDFF09A;
const CIAA_PRA: u32 = 0xBFE001;

/// The first DMACON write of the start-up, from the run's origin, without and with a filter
/// command in front (the CIA write is at `START_CIA`).
/// (the first sequence entry, a speed command, is included; later entries add their costs).
const START_PLAIN: u64 = 4152;
const START_CIA: u64 = 2772;
/// Colour clocks from the last write of the start-up call (the audio interrupts it requests come
/// after) to the first tick's call: when its last write is AUDxDAT of a keyed voice, and when not.
const START_TO_TICK_KEYED: u64 = 71200;
const START_TO_TICK: u64 = 71333;
/// PER to VOL, per voice.
const PER_VOL: [u64; 4] = [444, 471, 498, 497];
/// The gap from a voice's last write to the next voice's first.
/// The work before a voice's PER write (fitted to the reference logs): a constant, and per arpeggio
/// byte read, arpeggio jump, envelope step, envelope point loaded and envelope jump.
const PRE: [u64; 6] = [883, 31, 43, 34, 234, 79];
/// A silent voice's VOL write after the previous voice's last write: by what that was (a sample
/// start, a plain VOL, a silent voice's VOL), per voice.
const MUTED_AFTER: [[u64; 4]; 3] = [[300, 303, 331, 341], [300, 348, 359, 354], [300, 219, 215, 215]];
/// The gap from a muted voice's VOL write (volume 0) to the next voice's first write.
/// The gap before a row's work (DMACON 0x8200), and each voice's first write in a row.
const ROW_PRE: u64 = 580;
/// ... after a plain volume write, and after a silent voice's.
const ROW_PRE_VOL: u64 = 591;
const ROW_PRE_MUTED: u64 = 451;
/// Sequence entry costs (measured with probe modules; a loop's first-met-and-final and
/// taken/not-first costs are split evenly), and the advance's block-end, block-start and
/// move-to-the-next-entry costs.
const SQ_NOP: u64 = 153;
const SQ_SPEED: u64 = 169;
const SQ_TRANSPOSE: u64 = 183;
const SQ_FILTER_OFF: u64 = 194;
const SQ_FILTER_ON: u64 = 199;
const SQ_JUMP: u64 = 146;
const SQ_LOOP_TAKEN: u64 = 163;
const SQ_LOOP_DONE: u64 = 171;
const ADV_END_BLOCK: u64 = 600;
const ADV_START_BLOCK: u64 = 599;
const ADV_NEXT_ENTRY: u64 = 392;
/// A note voice's work before its first write, per voice, and what a retrigger adds.
const ROW_BASE: [u64; 4] = [721, 730, 745, 728];
const ROW_RETRIG: u64 = 485;
/// The fourth envelope's restart, on a block's first row.
const ROW_E3: u64 = 130;
/// A keyed voice (DMA off, then the interrupt starts the sound) over a retriggered note's.
const ROW_KEY: u64 = 50;
/// Per voice: LEN to LOC and LOC to INTENA of a synth voice's row start, and of a moving sample's.
const ROW_SYN: [(u64, u64); 4] = [(125, 162), (140, 178), (156, 193), (159, 198)];
const ROW_SMP: [(u64, u64); 4] = [(133, 100), (149, 116), (164, 132), (168, 135)];
/// A quiet row of a voice with a note to release, per voice, and of one without.
const ROW_QUIET: [u64; 4] = [900, 760, 800, 800];
const ROW_IDLE: u64 = 230;
/// Colour clocks from the last write of a tick to the first interrupt handler write, and from the
/// end of one handler to the next one's start.
const IRQ_LAT: [u64; 4] = [413, 379, 344, 343];
/// Handler gaps per channel: LOC to LEN, and LEN to the next write.
const H_LEN: [u64; 4] = [109, 125, 140, 144];
const H_DMA: [u64; 4] = [94, 109, 125, 129];
const IRQ_NEXT: u64 = 218;

/// Instructions budget guards.
const SEQ_GUARD: usize = 4096;
const ADV_GUARD: usize = 100_000;
const STOP: u32 = u32::MAX;

/// The chunks' file offsets, when the nine of them lie inside the file.
fn parse(d: &[u8]) -> Option<[usize; 9]> {
    if d.len() < 8 + 36 || &d[0..8] != b"ADRVPACK" {
        return None;
    }
    let mut off = 8usize;
    let mut out = [0usize; 9];
    for o in out.iter_mut() {
        let len = be32(d, off) as usize;
        let start = off.checked_add(4)?;
        let end = start.checked_add(len)?;
        if end > d.len() {
            return None;
        }
        *o = start;
        off = end;
    }
    Some(out)
}

fn detect(d: &[u8]) -> bool {
    parse(d).is_some()
}

fn s8(x: u8) -> i32 {
    x as i8 as i32
}

fn s16(x: i32) -> i32 {
    x as i16 as i32
}

/// The division of the player: a signed 32-bit dividend by a signed 16-bit divisor; a quotient that
/// does not fit 16 bits leaves the low 16 bits of the dividend. (A zero divisor cannot occur where
/// it is used; it gives the dividend's low word here.)
fn q16(n: i32, d: i32) -> u16 {
    let d = s16(d);
    if d == 0 {
        return n as u16;
    }
    let q = n.wrapping_div(d);
    if (-32768..=32767).contains(&q) {
        q as u16
    } else {
        n as u16
    }
}

fn period_at(i: u32) -> u16 {
    match i {
        0..=47 => PERIODS[i as usize],
        50 => 128,
        51 => 256,
        52 => 512,
        53 => 1024,
        _ => 0,
    }
}

#[derive(Clone, Copy, Default)]
struct Env {
    pos: u32,
    cnt: u16,
    val: u16,
    delta: u16,
}

#[derive(Clone, Copy)]
struct Voice {
    chvol: u8,
    trow: u16,
    note: u8,
    inst: u32,
    nvol: u8,
    arp: u32,
    port: u8,
    ev: u32,
    per: u16,
    active: bool,
    fresh: bool,
    f1: u16,
    f2: u16,
    env: [Env; 4],
    front: u32,
    back: u32,
}

/// Why a voice update stopped early.
enum Skip {
    /// The rest of this voice's update (an envelope ran out of jumps).
    Voice,
    /// The rest of the tick's voice updates (an arpeggio ran out of jumps).
    All,
}

/// The song ended (an end entry, or a sequence jump that ran out of budget).
struct End;

pub struct Aps {
    c: [u32; 9],
    v: [Voice; 4],
    seq: u8,
    blk: u32,
    rep: u8,
    row: u8,
    timer: u8,
    tpr: u8,
    mult: u8,
    loopc: u8,
    trans: u8,
    budget: u8,
    ended: bool,
    filter_cmd: bool,
    silence: u32,
    last_kind: u8,
    start_keyed: bool,
    /// Work counters of the voice update in progress, for the cost of the call: arpeggio bytes
    /// read, arpeggio jumps, envelope steps, envelope points loaded, envelope jumps.
    feat: [u32; 5],
    /// A carried half colour clock (the synthesis costs are in half clocks).
    half: u64,
    /// The start-up call is running: sequencer costs add up in `sq_acc` instead of passing.
    start_origin: u64,
    starting: bool,
    sq_acc: u64,
    sq_seen: bool,
    irq_end: u64,
    first_tick: Option<u64>,
}

fn rd8(hw: &Hw, a: u32) -> u8 {
    hw.bus.r8(a)
}

fn rd16(hw: &Hw, a: u32) -> u16 {
    hw.bus.r16(a)
}

fn rd32(hw: &Hw, a: u32) -> u32 {
    hw.bus.r32(a)
}

fn wr16(hw: &mut Hw, a: u32, v: u16) {
    hw.bus.write_word(a, v);
}

impl Aps {
    // ---------------------------------------------------------------- small helpers

    fn env_val(&self, v: usize, k: u32) -> i32 {
        let vv = v + (k / 4) as usize;
        if vv < 4 {
            self.v[vv].env[(k % 4) as usize].val as i16 as i32
        } else {
            0
        }
    }

    fn evrec(&self, hw: &Hw, a: u32) -> (u32, u16) {
        if a == STOP {
            return (13 << 28, 0);
        }
        (rd32(hw, a), rd16(hw, a.wrapping_add(4)))
    }

    /// The moving sample start terms of a record `(e, w)` for voice `v` (type 4: `f4`, type 5: `f5`).
    fn moving(&self, v: usize, e: u32, w: u16, two: bool) -> i32 {
        let ea = (e >> 26) & 3;
        let eb = (e >> 24) & 3;
        let mut a = (self.env_val(v, ea) * (w >> 8) as i32) >> 8;
        if two {
            a = (s16(a) * self.env_val(v, eb)) >> 16;
        }
        (w & 0xFF) as i32 * s16(a)
    }

    fn sample_addr(&self, e: u32, off: i32) -> u32 {
        self.c[7].wrapping_add(2u32.wrapping_mul((e & 0xFF_FFFF).wrapping_add(off as u32)))
    }

    /// Lets `h` half colour clocks pass (a half clock left over is carried to the next).
    fn wh(&mut self, hw: &mut Hw, h: u64) {
        let t = self.half + h;
        hw.wait(t / 2);
        self.half = t % 2;
    }

    fn intena(&self, hw: &mut Hw, v: u16) {
        wr16(hw, INTENA, v);
    }

    // ---------------------------------------------------------------- the sequencer

    fn entry(&self, hw: &Hw) -> (u8, u8) {
        let a = self.c[0].wrapping_add(2 * self.seq as u32);
        (rd8(hw, a), rd8(hw, a.wrapping_add(1)))
    }

    /// Runs sequence entries from the current position to a play entry.
    fn sequencer(&mut self, hw: &mut Hw) -> Result<(), End> {
        for _ in 0..SEQ_GUARD {
            let (c, p) = self.entry(hw);
            let n = c & 0x1F;
            let cost = match c & 0xE0 {
                0x40 => {
                    if self.loopc == 0xFF && n == 1 || self.loopc == 1 {
                        SQ_LOOP_DONE
                    } else {
                        SQ_LOOP_TAKEN
                    }
                }
                0x60 => SQ_JUMP,
                0x80 => SQ_TRANSPOSE,
                0xA0 => SQ_SPEED,
                0xC0 => {
                    if p != 0 {
                        SQ_FILTER_ON
                    } else {
                        SQ_FILTER_OFF
                    }
                }
                0xE0 => SQ_NOP,
                _ => 0,
            };
            self.sq_cost(hw, cost);
            match c & 0xE0 {
                0x00 => return Err(End),
                0x20 => {
                    self.blk = self.c[1].wrapping_add(26 * p as u32);
                    return Ok(());
                }
                0x40 => {
                    if self.loopc == 0xFF {
                        self.loopc = n;
                    }
                    self.loopc = self.loopc.wrapping_sub(1);
                    if self.loopc == 0 {
                        self.loopc = 0xFF;
                        self.seq = self.seq.wrapping_add(1);
                    } else {
                        self.jump(p)?;
                    }
                }
                0x60 => self.jump(p)?,
                0x80 => {
                    self.trans = p & 63;
                    self.seq = self.seq.wrapping_add(1);
                }
                0xA0 => {
                    self.mult = p;
                    self.seq = self.seq.wrapping_add(1);
                }
                0xC0 => {
                    self.filter_cmd = true;
                    if self.starting {
                        hw.bus.clock = self.start_origin + START_CIA + self.sq_acc - cost;
                    }
                    let x = hw.bus.read_byte(CIAA_PRA);
                    hw.bus.write_byte(CIAA_PRA, if p != 0 { x & !2 } else { x | 2 });
                    self.seq = self.seq.wrapping_add(1);
                }
                _ => self.seq = self.seq.wrapping_add(1),
            }
        }
        Err(End)
    }

    /// Charges a sequence entry: in the start-up call the sum is kept (the first entry is part of
    /// the base time), later the time passes.
    fn sq_cost(&mut self, hw: &mut Hw, cost: u64) {
        if self.starting {
            if self.sq_seen {
                self.sq_acc += cost;
            }
            self.sq_seen = true;
        } else {
            hw.wait(cost);
        }
    }

    fn jump(&mut self, p: u8) -> Result<(), End> {
        self.budget = self.budget.wrapping_sub(1);
        if self.budget == 0 {
            return Err(End);
        }
        self.seq = self.seq.wrapping_add(p);
        Ok(())
    }

    // ---------------------------------------------------------------- advance

    fn start_block(&mut self, hw: &Hw) {
        self.tpr = rd8(hw, self.blk).wrapping_mul(self.mult);
        for v in 0..4 {
            self.v[v].trow = rd16(hw, self.blk.wrapping_add(2 + 6 * v as u32 + 4));
        }
        self.row = 0;
    }

    /// Steps 2 to 4 of the advance. With `skip` the row test and the move to the next block are
    /// left out (the sequence entry has just been entered).
    fn step2(&mut self, hw: &mut Hw, mut skip: bool) -> Result<bool, End> {
        for _ in 0..ADV_GUARD {
            if !skip {
                if self.row < rd8(hw, self.blk.wrapping_add(1)) {
                    self.process_row(hw);
                    self.timer = self.tpr;
                    return Ok(self.tpr != 0);
                }
                if !self.starting {
                    hw.wait(ADV_END_BLOCK);
                }
                self.blk = self.blk.wrapping_add(26);
                self.rep = self.rep.wrapping_add(1);
            } else {
                self.rep = 0;
                skip = false;
            }
            if self.rep < self.entry(hw).0 & 0x1F {
                if !self.starting {
                    hw.wait(ADV_START_BLOCK);
                }
                self.start_block(hw);
                continue;
            }
            if !self.starting {
                hw.wait(ADV_NEXT_ENTRY);
            }
            self.seq = self.seq.wrapping_add(1);
            self.sequencer(hw)?;
            skip = true;
        }
        Err(End)
    }

    fn advance(&mut self, hw: &mut Hw) -> Result<(), End> {
        for _ in 0..ADV_GUARD {
            for v in 0..4 {
                self.v[v].trow = self.v[v].trow.wrapping_add(1);
            }
            self.row = self.row.wrapping_add(1);
            if self.step2(hw, false)? {
                return Ok(());
            }
        }
        Err(End)
    }

    // ---------------------------------------------------------------- rows

    fn process_row(&mut self, hw: &mut Hw) {
        hw.wait(match self.last_kind {
            2 => ROW_PRE_MUTED,
            1 => ROW_PRE_VOL,
            _ => ROW_PRE,
        });
        hw.dmacon(0x8200);
        let blk = self.blk;
        let mut keys = [false; 4];
        for v in 0..4usize {
            let be = blk.wrapping_add(2 + 6 * v as u32);
            let (b0, b1, b2, b3) = (rd8(hw, be), rd8(hw, be + 1), rd8(hw, be + 2), rd8(hw, be + 3));
            let r = self.c[2].wrapping_add(4 * self.v[v].trow as u32);
            let (r0, r1, r2, r3) = (rd8(hw, r), rd8(hw, r + 1), rd8(hw, r + 2), rd8(hw, r + 3));
            // channel volume
            {
                let t = 4 * (b2 & 63);
                let s = b2 >> 6;
                let vo = &mut self.v[v];
                if s == 0 {
                    vo.chvol = t;
                } else if vo.chvol < t {
                    vo.chvol = vo.chvol.wrapping_add(s);
                    if vo.chvol >= t {
                        vo.chvol = t;
                    }
                } else if vo.chvol > t {
                    vo.chvol = vo.chvol.wrapping_sub(s);
                    if vo.chvol <= t {
                        vo.chvol = t;
                    }
                }
            }
            if r0 & 0x40 != 0 {
                self.v[v].arp = self.c[3].wrapping_add(r3 as u32).wrapping_add(b3 as u32);
            } else {
                self.v[v].port = r3;
            }
            if r0 & 63 == 63 || b0 & 63 == 63 {
                if self.v[v].active {
                    self.v[v].active = false;
                    self.release(hw, v, b1, r1, r2);
                    hw.wait(ROW_QUIET[v]);
                } else {
                    hw.wait(ROW_IDLE);
                }
                continue;
            }
            hw.wait(
                ROW_BASE[v]
                    + if r0 & 0x80 != 0 { ROW_RETRIG } else { 0 }
                    + if r0 & 0x80 != 0 && self.row == 0 && b0 & 0x80 != 0 { ROW_E3 } else { 0 },
            );
            self.v[v].active = true;
            self.v[v].note = (r0 & 63).wrapping_add(b0 & 63);
            let (inst, ev);
            if r2 & 0x80 != 0 {
                inst = self.c[4].wrapping_add(26 * b1 as u32);
                ev = self.c[6].wrapping_add(6 * rd8(hw, inst).wrapping_add(r1) as u32);
            } else {
                inst = self.c[4].wrapping_add(26 * b1.wrapping_add(r1) as u32);
                ev = self.c[6].wrapping_add(6 * rd8(hw, inst) as u32);
            }
            self.v[v].inst = inst;
            self.v[v].ev = ev;
            if r0 & 0x80 != 0 {
                self.retrigger(hw, v, b0);
            }
            if r2 & 0x40 != 0 {
                self.v[v].nvol = self.v[v].chvol;
                self.tpr = (r2 & 63).wrapping_mul(self.mult);
            } else {
                self.v[v].nvol = (((r2 & 63) as u32 * self.v[v].chvol as u32) >> 5) as u8;
            }
            let (e, w) = self.evrec(hw, ev);
            match e >> 28 {
                4 | 5 => {
                    hw.len(v, w & 0xFF);
                    hw.wait(ROW_SMP[v].0);
                    hw.loc(v, self.sample_addr(e, 0));
                    hw.wait(ROW_SMP[v].1);
                    self.intena(hw, 0x0080 << v);
                    hw.wait(25);
                    hw.dmacon(0x8000 | 1 << v);
                }
                3 => {
                    hw.len(v, (e & 0xFF) as u16);
                    hw.wait(ROW_SYN[v].0);
                    hw.loc(v, self.v[v].front);
                    hw.wait(ROW_SYN[v].1);
                    self.intena(hw, 0x0080 << v);
                    hw.wait(25);
                    hw.dmacon(0x8000 | 1 << v);
                }
                _ => {
                    if r0 & 0x80 != 0 {
                        keys[v] = true;
                        hw.wait(ROW_KEY);
                        hw.dmacon(1 << v);
                    }
                }
            }
        }
        self.start_keyed = keys.iter().any(|&k| k);
        let mut first = true;
        for v in 0..4 {
            if !keys[v] {
                if keys[v..].iter().any(|&k| k) {
                    hw.wait(35);
                }
                continue;
            }
            {
                hw.wait(if first { 97 } else { 67 });
                first = false;
                self.intena(hw, 0x8000 | 0x0080 << v);
                hw.wait(31);
                wr16(hw, 0xDFF0AA + 16 * v as u32, 0);
            }
        }
    }

    fn release(&mut self, hw: &mut Hw, v: usize, b1: u8, r1: u8, r2: u8) {
        let cur = self.v[v].inst;
        if rd8(hw, cur) != rd8(hw, cur.wrapping_add(1)) {
            let (inst, ev);
            if r2 & 0x80 != 0 {
                inst = self.c[4].wrapping_add(26 * b1 as u32);
                ev = self.c[6].wrapping_add(6 * rd8(hw, inst.wrapping_add(1)).wrapping_add(r1) as u32);
            } else {
                inst = self.c[4].wrapping_add(26 * b1.wrapping_add(r1) as u32);
                ev = self.c[6].wrapping_add(6 * rd8(hw, inst.wrapping_add(1)) as u32);
            }
            self.v[v].inst = inst;
            self.v[v].ev = ev;
        }
        let inst = self.v[v].inst;
        let rp = self.c[5].wrapping_add(2 * rd16(hw, inst.wrapping_add(8)) as u32);
        let dur = rd8(hw, rp) as u32;
        let lev = s8(rd8(hw, rp.wrapping_add(1)));
        let scale = rd8(hw, inst.wrapping_add(3)) as u32;
        let e = &mut self.v[v].env[0];
        e.pos = rp;
        e.cnt = (dur * scale) as u16;
        if e.cnt != 0 {
            e.delta = q16(s16(lev * 256 - e.val as i32), e.cnt as i16 as i32);
        }
    }

    fn retrigger(&mut self, hw: &Hw, v: usize, b0: u8) {
        let inst = self.v[v].inst;
        for (k, off) in [(0usize, 6u32), (1, 10), (2, 12), (3, 14)] {
            if k == 3 && !(self.row == 0 && b0 & 0x80 != 0) {
                continue;
            }
            let p = self.c[5].wrapping_add(2 * rd16(hw, inst.wrapping_add(off)) as u32);
            let e = &mut self.v[v].env[k];
            e.pos = p;
            e.val = (s8(rd8(hw, p.wrapping_add(1))) * 256) as u16;
            e.cnt = 0;
        }
        self.v[v].fresh = true;
    }

    // ---------------------------------------------------------------- the tick

    fn tick_voices(&mut self, hw: &mut Hw) {
        self.budget = 255;
        self.last_kind = 1;
        for v in 0..4 {
            match self.voice(hw, v) {
                Ok(()) | Err(Skip::Voice) => {}
                Err(Skip::All) => break,
            }
        }
        if self.ended {
            return;
        }
        self.timer = self.timer.wrapping_sub(1);
        if self.timer != 0 {
            return;
        }
        if self.advance(hw).is_err() {
            self.ended = true;
            hw.wait(100);
            self.intena(hw, 0x0780);
            hw.wait(40);
            hw.dmacon(0x000F);
        }
    }

    fn env_step(&mut self, hw: &Hw, v: usize, k: usize, scale: u32, e1: bool) -> Result<(), Skip> {
        self.feat[2] += 1;
        let e2val = self.v[v].env[2].val as u32;
        let inst = self.v[v].inst;
        let e = &mut self.v[v].env[k];
        e.val = e.val.wrapping_add(e.delta);
        if e.cnt != 0 {
            e.cnt -= 1;
            return Ok(());
        }
        self.feat[3] += 1;
        e.val = (s8(rd8(hw, e.pos.wrapping_add(1))) * 256) as u16;
        e.pos = e.pos.wrapping_add(2);
        let mut kk = 1u32;
        for _ in 0..4096 {
            let s = if e1 {
                (((rd8(hw, inst.wrapping_add(17)) as u32 * e2val) & 0xFFFF) >> 8)
                    .wrapping_add(rd8(hw, inst.wrapping_add(16)) as u32)
                    .wrapping_add(kk)
            } else {
                scale
            };
            let dur = rd8(hw, e.pos) as u32;
            let lev = s8(rd8(hw, e.pos.wrapping_add(1)));
            e.cnt = (dur.wrapping_mul(s) & 0xFFFF) as u16;
            if e.cnt != 0 {
                e.delta = q16(s16(lev * 256 - e.val as i32), e.cnt as i16 as i32);
                return Ok(());
            }
            self.budget = self.budget.wrapping_sub(1);
            if self.budget == 0 {
                return Err(Skip::Voice);
            }
            self.feat[4] += 1;
            e.pos = e.pos.wrapping_add((lev * 2) as u32);
            kk = kk.wrapping_add(1);
        }
        Err(Skip::Voice)
    }

    fn voice(&mut self, hw: &mut Hw, v: usize) -> Result<(), Skip> {
        self.feat = [0; 5];
        let inst = self.v[v].inst;
        if self.v[v].chvol == 0 {
            hw.wait(MUTED_AFTER[self.last_kind as usize][v]);
            hw.vol(v, 0);
            self.last_kind = 2;
            return Ok(());
        }
        // period
        let r0 = rd8(hw, self.c[2].wrapping_add(4 * self.v[v].trow as u32));
        if r0 & 0x40 != 0 {
            let mut a = self.v[v].arp;
            let mut x = 0u8;
            let mut found = false;
            for _ in 0..4096 {
                self.feat[0] += 1;
                let b = rd8(hw, a);
                if b & 0x80 == 0 {
                    x = b;
                    found = true;
                    break;
                }
                self.feat[1] += 1;
                self.budget = self.budget.wrapping_sub(1);
                if self.budget == 0 {
                    self.v[v].arp = a;
                    return Err(Skip::All);
                }
                let off = (((b & 0x7F) << 1) as i8 >> 1) as i32;
                a = a.wrapping_add(off as u32);
            }
            if !found {
                return Err(Skip::All);
            }
            self.v[v].arp = a.wrapping_add(1);
            let idx = (self.trans as u32) + (x & 63) as u32 + self.v[v].note as u32;
            self.v[v].per = period_at(idx);
        } else {
            let target = period_at(self.trans as u32 + self.v[v].note as u32);
            let sp = self.v[v].port as u16;
            let vo = &mut self.v[v];
            if sp == 0 {
                vo.per = target;
            } else if vo.per < target {
                vo.per = vo.per.wrapping_add(sp);
                if vo.per >= target {
                    vo.per = target;
                }
            } else if vo.per > target {
                vo.per = vo.per.wrapping_sub(sp);
                if vo.per <= target {
                    vo.per = target;
                }
            }
        }
        // envelopes
        let s0 = rd8(hw, inst.wrapping_add(3)) as u32;
        if s0 != 0 {
            self.env_step(hw, v, 0, s0, false)?;
        }
        if rd8(hw, inst.wrapping_add(16)) != 0 {
            self.env_step(hw, v, 1, 0, true)?;
        }
        let s2 = rd8(hw, inst.wrapping_add(4)) as u32;
        if s2 != 0 {
            self.env_step(hw, v, 2, s2, false)?;
        }
        let s3 = rd8(hw, inst.wrapping_add(5)) as u32;
        if s3 != 0 {
            self.env_step(hw, v, 3, s3, false)?;
        }
        // pitch
        let pr = rd8(hw, inst.wrapping_add(24)) as u32;
        let a = (self.env_val(v, pr >> 4) * s8(rd8(hw, inst.wrapping_add(19)))) >> 8;
        let b = (s16(a) * self.env_val(v, (pr >> 2) & 3)) >> 16;
        let c = (self.env_val(v, pr & 3) * s8(rd8(hw, inst.wrapping_add(20)))) >> 10;
        let det = s8(rd8(hw, inst.wrapping_add(18)));
        let per = ((self.v[v].per as i32 - det) as u16 as i32 - (b + c)) as u16;
        // volume
        let vr = rd8(hw, inst.wrapping_add(25)) as u32;
        let mut a = (self.env_val(v, vr >> 2) * s8(rd8(hw, inst.wrapping_add(22)))
            + self.env_val(v, vr & 3) * s8(rd8(hw, inst.wrapping_add(23))))
            >> 8;
        a += 16 * s8(rd8(hw, inst.wrapping_add(21)));
        let b = (s16(a) * self.v[v].nvol as i32) >> 12;
        let vol = (b & 127) as u16;
        hw.wait(PRE.iter().zip([1, self.feat[0], self.feat[1], self.feat[2], self.feat[3], self.feat[4]]).map(|(a, b)| a * b as u64).sum());
        hw.per(v, per);
        hw.wait(PER_VOL[v]);
        hw.vol(v, vol);
        // the event
        let (e, w) = self.evrec(hw, self.v[v].ev);
        match e >> 28 {
            4 | 5 => {
                let f = self.moving(v, e, w, e >> 28 == 4);
                hw.wait(if e >> 28 == 4 { 492 } else { 383 });
                hw.loc(v, self.sample_addr(e, f));
                self.last_kind = 0;
            }
            3 => {
                let cost = self.synth(hw, v);
                self.wh(hw, cost);
                self.v[v].fresh = false;
                let vo = &mut self.v[v];
                std::mem::swap(&mut vo.front, &mut vo.back);
                let a = vo.back;
                hw.loc(v, a);
                self.last_kind = 0;
            }
            _ => self.last_kind = 1,
        }
        Ok(())
    }

    // ---------------------------------------------------------------- synthesis

    /// The source address of the record at `rec` (section 8.2); `None` for a one-shot record whose
    /// note is not fresh, when `need_fresh`.
    fn source(&self, hw: &Hw, v: usize, rec: u32, need_fresh: bool) -> Option<u32> {
        let (e, w) = self.evrec(hw, rec);
        match e >> 28 {
            1 if need_fresh && !self.v[v].fresh => None,
            4 => Some(self.sample_addr(e, self.moving(v, e, w, true))),
            5 => Some(self.sample_addr(e, self.moving(v, e, w, false))),
            _ => Some(self.sample_addr(e, 0)),
        }
    }

    /// Builds the waveform of voice `v`'s synth record into its front buffer; the colour clocks the
    /// work takes.
    fn synth(&mut self, hw: &mut Hw, v: usize) -> u64 {
        let x = self.v[v].ev;
        let (e, w) = self.evrec(hw, x);
        let n = if e & 63 == 0 { 128usize } else { 2 * (e & 63) as usize };
        let mode = (e >> 20) & 3;
        let op = (e >> 16) & 15;
        let front = self.v[v].front;
        let mut buf: Vec<u8> = (0..=n as u32).map(|i| rd8(hw, front.wrapping_add(i))).collect();
        let (ea, eb) = ((e >> 26) & 3, (e >> 24) & 3);
        let (mut comb_on, mut fvar) = (false, 0u8);
        let (mut fill, mut skip, mut t1, mut t2) = (true, false, 0u32, 0u32);
        let (mut q22, mut g1nz, mut g2nz) = (false, false, false);
        let fresh = self.v[v].fresh;
        let rec = |k: u32| x.wrapping_add(6 * k);
        match mode {
            0 => {
                let a = s16((self.env_val(v, ea) * (w >> 8) as i32) >> 8);
                let bv = s16((self.env_val(v, eb) * (w & 0xFF) as i32) >> 8);
                match op {
                    0 => {
                        let d = s16(a + bv) >> 4;
                        if (d as u16 as usize) < n {
                            for (i, b) in buf.iter_mut().enumerate().take(n) {
                                *b = if i < d as usize { 100 } else { 156 };
                            }
                        } else {
                            fill = false;
                        }
                    }
                    1 => {
                        let y = bv >> 4;
                        let xx = a >> 4;
                        let z = (127 - xx - y) & 0xFF;
                        let k = n >> 3;
                        let vals = [
                            xx + z + y,
                            xx + z - y,
                            xx - z + y,
                            xx - z - y,
                            y - xx + z,
                            z - xx - y,
                            y - xx - z,
                            -xx - z - y,
                        ];
                        for (r, val) in vals.iter().enumerate() {
                            for b in buf.iter_mut().skip(r * k).take(k) {
                                *b = *val as u8;
                            }
                        }
                    }
                    2 => {
                        let d = s16(a + bv) >> 4;
                        for (i, b) in buf.iter_mut().enumerate().take(n) {
                            *b = (d * (n as i32 - 1 - i as i32)) as u8;
                        }
                    }
                    _ => {}
                }
            }
            1 => {
                t1 = self.evrec(hw, rec(1)).0 >> 28;
                let srcopt = self.source(hw, v, rec(1), true);
                skip = srcopt.is_none();
                if let Some(s) = srcopt {
                    let src = |i: usize| rd8(hw, s.wrapping_add(i as u32)) as i8;
                    match op {
                        0 => {
                            for i in 0..n {
                                buf[i] = src(i) as u8;
                            }
                        }
                        1 => {
                            for i in 0..n {
                                buf[i] = ((buf[i] as i8 >> 1) as u8).wrapping_add(src(i) as u8);
                            }
                        }
                        2 if n >= 2 => {
                            buf[0] = ((buf[n - 1] as i8 >> 1) as u8).wrapping_add(src(0) as u8);
                            for i in 1..n {
                                buf[i] = ((buf[i - 1] as i8 >> 1) as u8).wrapping_add(src(i) as u8);
                            }
                        }
                        3 if n >= 3 => {
                            buf[0] = ((buf[n - 2] as i8 >> 1) as u8).wrapping_add(src(0) as u8);
                            buf[1] = ((buf[n - 1] as i8 >> 1) as u8).wrapping_add(src(1) as u8);
                            for i in 2..n {
                                buf[i] = ((buf[i - 2] as i8 >> 1) as u8).wrapping_add(src(i) as u8);
                            }
                        }
                        4 => {
                            let input: Vec<u8> = (0..n).map(|i| src(i) as u8).collect();
                            let w1 = halve(&input);
                            let w2 = halve(&w1);
                            let o = halve(&w2);
                            buf[..n].copy_from_slice(&o);
                        }
                        _ => {}
                    }
                }
            }
            2 => {
                t1 = self.evrec(hw, rec(1)).0 >> 28;
                t2 = self.evrec(hw, rec(2)).0 >> 28;
                let s1 = self.source(hw, v, rec(1), false).unwrap_or(0);
                let s2 = self.source(hw, v, rec(2), false).unwrap_or(0);
                let a = |i: usize| rd8(hw, s1.wrapping_add(i as u32)) as i8 as i32;
                let b = |i: usize| rd8(hw, s2.wrapping_add(i as u32)) as i8 as i32;
                let mut acc: i32 = 0;
                for i in 0..n {
                    let r = match op {
                        1 => (a(i) + b(i)) >> 1,
                        2 => (a(i) * b(i)) >> 6,
                        3 => a(i) & b(i),
                        4 => a(i) | b(i),
                        5 => a(i) ^ b(i),
                        6 => rd8(hw, s2.wrapping_add(a(i) as u32)) as i32,
                        7 => {
                            acc = s16(acc + a(i));
                            rd8(hw, s2.wrapping_add(acc as u32)) as i32
                        }
                        9 => (a(i) + b(i) + buf[i] as i8 as i32) >> 1,
                        _ => break,
                    };
                    buf[i] = r as u8;
                }
            }
            _ => {}
        }
        // comb
        let k = (e >> 8) & 15;
        if k != 0 && n % 4 == 0 {
            comb_on = true;
            let (p, pw) = self.evrec(hw, rec(k));
            let g = s16(((self.env_val(v, (p >> 26) & 3) * (pw >> 12) as i32) >> 8) + ((p >> 8) & 0xFF) as i32);
            let h = s16(((self.env_val(v, (p >> 24) & 3) * (pw & 0xFF) as i32) >> 12) + (p & 0xFF) as i32);
            let (mut i, mut j, mut m) = (n, n, n);
            for pass in 0..4 {
                match pass {
                    1 => m = n,
                    2 => {
                        j = n;
                        m = n;
                    }
                    3 => m = n,
                    _ => {}
                }
                while m != 0 {
                    let t = (buf[i] as i8 as i32).wrapping_mul(g) + (buf[j] as i8 as i32) * 256
                        + (buf[m] as i8 as i32).wrapping_mul(h);
                    buf[i] = (s16(t) >> 8) as u8;
                    j = j.wrapping_sub(2);
                    i = i.wrapping_sub(1);
                    m = m.wrapping_sub(4);
                }
            }
        }
        // filter
        let f = (e >> 12) & 15;
        if f != 0 {
            let (q, qw) = self.evrec(hw, rec(f));
            fvar = if q & 0x80_0000 == 0 { 1 } else { 2 };
            if q & 0x40_0000 != 0 && self.v[v].fresh {
                self.v[v].f1 = 0;
                self.v[v].f2 = 0;
            }
            let g1 = s16(((self.env_val(v, (q >> 26) & 3) * s8((qw >> 8) as u8)) >> 10) + ((q >> 8) & 0xFF) as i32);
            let g2 = s16(((self.env_val(v, (q >> 24) & 3) * s8(qw as u8)) >> 10) + (q & 0xFF) as i32);
            q22 = q & 0x40_0000 != 0;
            g1nz = g1 != 0;
            g2nz = g2 != 0;
            let vo = &mut self.v[v];
            if q & 0x80_0000 == 0 {
                if g1 != 0 {
                    for b in buf.iter_mut().take(n) {
                        let qq = q16(s16(vo.f1 as i32 * 8), g1);
                        *b = b.wrapping_sub(qq as u8);
                        vo.f1 = vo.f1.wrapping_add(*b as i8 as i16 as u16);
                    }
                }
                if g2 != 0 {
                    for i in 0..n {
                        let qq = q16(s16(vo.f2 as i32 * 8), g2);
                        let nxt = buf[i + 1] as i8 as i32;
                        vo.f2 = vo.f2.wrapping_add((nxt - qq as i16 as i32) as u16);
                        buf[i] = qq as u8;
                    }
                }
            } else {
                let d = s16(-g1);
                if d != 0 && g2 != 0 {
                    for b in buf.iter_mut().take(n) {
                        let q1 = q16(s16(vo.f2 as i32 * 8), g2) as i16 as i32;
                        let mut xx = (*b as i8 as i32 - q1) as u16;
                        let q2 = q16(q1 * 8, d);
                        vo.f1 = vo.f1.wrapping_add(q2);
                        xx = xx.wrapping_sub(vo.f1);
                        vo.f2 = vo.f2.wrapping_add(xx);
                        *b = q2 as u8;
                    }
                }
            }
        }
        for (i, b) in buf.iter().enumerate() {
            hw.bus.w8(front.wrapping_add(i as u32), *b);
        }
        synth_cost_h(&SynthFeat { mode, op, n: n as u64, comb: comb_on, fvar, fill, skip, t1, t2, q22, g1nz, g2nz, fresh })
    }

    // ---------------------------------------------------------------- the audio interrupt

    /// The original's handler never clears the channel's INTREQ bit (SPEC section 10 lists only the
    /// INTENA disable), so the bit stays set: a later INTENA enable takes the interrupt again at
    /// once, and the AUDxDAT write of a row, finding the request already set, raises nothing. The
    /// reference runs only match with this (clearing the bit moved the interrupts of nine modules).
    fn handler(&mut self, ch: usize, hw: &mut Hw) {
        hw.bus.paula.sync(hw.bus.clock, &hw.bus.ram);
        let bit = 0x80u16 << ch;
        if hw.bus.paula.intreq & bit == 0 {
            return;
        }
        hw.wait(if hw.bus.clock <= self.irq_end + 2 { IRQ_NEXT } else { IRQ_LAT[ch] });
        self.intena(hw, bit);
        let ev = self.v[ch].ev;
        let (e, w) = self.evrec(hw, ev);
        match e >> 28 {
            t @ (1 | 2) => {
                if t == 1 {
                    self.v[ch].ev = STOP;
                } else {
                    self.v[ch].ev = ev.wrapping_add(6);
                }
                hw.wait(if t == 1 { 234 } else { 253 });
                hw.loc(ch, self.sample_addr(e, 0));
                hw.wait(H_LEN[ch]);
                hw.len(ch, w);
                hw.wait(H_DMA[ch]);
                hw.dmacon(0x8000 | 1 << ch);
                hw.wait(32);
                self.intena(hw, 0x8000 | bit);
            }
            0 => {
                hw.wait(192);
                hw.loc(ch, self.sample_addr(e, 0));
                hw.wait(H_LEN[ch]);
                hw.len(ch, w);
                hw.wait(H_DMA[ch] + 5);
                self.intena(hw, bit);
                hw.wait(21);
                hw.dmacon(0x8000 | 1 << ch);
            }
            13 => {
                hw.wait(115);
                hw.loc(ch, self.silence);
                hw.wait(31);
                hw.len(ch, 1);
                hw.wait(26);
                self.intena(hw, bit);
            }
            _ => {}
        }
        self.irq_end = hw.bus.clock;
    }
}

/// What the cost of one synthesis depends on.
struct SynthFeat {
    mode: u32,
    op: u32,
    n: u64,
    comb: bool,
    fvar: u8,
    /// Mode 0 operation 0: the pulse is written (its width is below the length).
    fill: bool,
    /// Mode 1: the one-shot source is not fresh, no generation.
    skip: bool,
    t1: u32,
    t2: u32,
    /// The filter record's bit 22 (state reset on a fresh note) and which gains are non-zero.
    q22: bool,
    g1nz: bool,
    g2nz: bool,
    fresh: bool,
}

/// The half colour clocks the synthesis of one buffer takes, from VOL to LOC: measured on the
/// reference with probe modules (one stage at a time; the stages add up).
fn synth_cost_h(f: &SynthFeat) -> u64 {
    let n = f.n;
    let mut c: u64 = match f.mode {
        0 => match f.op {
            0 => {
                if f.fill {
                    1320 + 22 * n
                } else {
                    1260
                }
            }
            1 => 1696 + 144 * (n >> 3),
            2 => 1328 + 76 * n,
            _ => 1262,
        },
        1 => {
            if f.skip {
                810
            } else {
                let extra: i64 = match f.t1 {
                    4 => 634,
                    5 => 414,
                    1 => -6,
                    _ => 0,
                };
                let b: i64 = match f.op {
                    0 => 988 + 22 * n as i64,
                    1 => 1014 + 42 * n as i64,
                    2 => 1048 + 46 * n as i64,
                    3 => {
                        if n >= 3 {
                            1080 + 46 * n as i64
                        } else {
                            1034
                        }
                    }
                    4 => 1444 + 243 * n as i64,
                    _ => 1046,
                };
                (b + extra) as u64
            }
        }
        2 => {
            let extra: u64 = [f.t1, f.t2].iter().map(|t| match t {
                4 => 620,
                5 => 402,
                _ => 0,
            }).sum();
            let (b, k): (u64, u64) = match f.op {
                1 => (1174, 54),
                2 => (1200, 116),
                3 => (1228, 38),
                4 => (1254, 38),
                5 => (1282, 38),
                6 => (1308, 40),
                7 => (1352, 52),
                9 => (1390, 70),
                _ => (3000, 0),
            };
            b + k * n + extra
        }
        _ => 690,
    };
    if f.comb {
        c += 924 + 417 * n;
    }
    if f.fvar != 0 {
        if f.q22 {
            c += 42 + if f.fresh { 52 } else { 0 };
        }
        if f.fvar == 1 {
            c += 956;
            if f.g1nz {
                c += 58 + 200 * n;
            }
            if f.g2nz {
                c += 76 + 200 * n;
            }
        } else {
            c += 610;
            if f.g1nz {
                c += 328;
                if f.g2nz {
                    c += 92 + 368 * n;
                }
            }
        }
    }
    c
}

/// The "halve" transform of mode 1 operation 4.
fn halve(i: &[u8]) -> Vec<u8> {
    let n = i.len();
    let h = n / 2;
    let sh = |x: u8, s: u32| ((x as i8) >> s) as u8;
    let mut o: Vec<u8> = i.iter().map(|&x| sh(x, 1)).collect();
    for k in 0..h {
        o[k] = o[k].wrapping_add(sh(i[2 * k], 1));
    }
    for k in 0..h {
        o[h + k] = o[h + k].wrapping_add(sh(i[2 * k], 2));
    }
    o
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let offs = parse(d)?;
    // absolute guest addresses of the nine chunks' data
    let c = offs.map(|o| base.wrapping_add(o as u32));
    let area = m.alloc(1024 + 16 + 2);
    if area == 0 {
        return None;
    }
    let mut r = Aps {
        c,
        v: [Voice {
            chvol: 0,
            trow: 0,
            note: 0,
            inst: c[4],
            nvol: 0,
            arp: c[3],
            port: 0,
            ev: c[6],
            per: 0,
            active: false,
            fresh: false,
            f1: 0,
            f2: 0,
            env: [Env::default(); 4],
            front: 0,
            back: 0,
        }; 4],
        seq: 0,
        blk: c[1],
        rep: 0,
        row: 0,
        timer: 0,
        tpr: 0,
        mult: 0,
        loopc: 0xFF,
        trans: 0,
        budget: 0,
        ended: false,
        filter_cmd: false,
        silence: area + 1040,
        last_kind: 1,
        start_keyed: false,
        feat: [0; 5],
        half: 0,
        irq_end: 0,
        start_origin: 0,
        starting: true,
        sq_acc: 0,
        sq_seen: false,
        first_tick: None,
    };
    for v in 0..4 {
        r.v[v].front = area + 128 * v as u32;
        r.v[v].back = area + 128 * (v as u32 + 4);
    }
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut hw = Hw { bus: &mut m.bus };
    r.start_origin = origin;
    hw.bus.clock = origin + START_CIA;
    r.last_kind = 0;
    let started = r.sequencer(&mut hw);
    if started.is_ok() {
        hw.bus.clock = origin + START_PLAIN + r.sq_acc - ROW_PRE;
        // the first row is processed without moving the pointer
        if r.step2(&mut hw, true).is_err() {
            r.ended = true;
        }
    } else {
        r.ended = true;
    }
    r.starting = false;
    r.last_kind = 1;
    // the timer starts after the start-up call, not after its interrupts
    let keyed = r.start_keyed;
    r.first_tick = Some(hw.bus.clock + if keyed { START_TO_TICK_KEYED } else { START_TO_TICK });
    Some(Box::new(r))
}

impl Replayer for Aps {
    fn format(&self) -> &'static str {
        "AProSys"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.first_tick.take()
    }

    fn songend(&self) -> bool {
        self.ended
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_voices(hw);
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if ch < 4 {
            self.handler(ch, hw);
            return true;
        }
        false
    }
}

#[cfg(test)]
#[path = "aprosys_tests.rs"]
mod tests;
