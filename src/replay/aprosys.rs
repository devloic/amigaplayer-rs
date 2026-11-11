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

/// 68000 cycles (half colour clocks) the start-up call returns sooner after its row's last write
/// than a tick's call does (`TAIL_*`): measured on the reference's call ends, the same for a row
/// that ends with a key tail, with a sample or synth start, or with nothing but its start.
const START_RETURN_H: u64 = 111;
/// The start-up call's first DMACON write and the CIA write of a filter command (in half colour
/// clocks from the run's origin), before the sum of the costs of the sequence entries met until
/// them (the first entry's included).
const START_PLAIN_H: u64 = 7967;
const START_CIA_H: u64 = 5207;
/// Half colour clocks from the audio interrupt being taken (when the call running returns, or at
/// an output sample) to the handler's first write.
const START_IRQ_H: u64 = 260;
/// PER to VOL, per voice.
const PER_VOL: [u64; 4] = [444, 471, 498, 497];
/// All the in-tick costs below are counts of 68000 cycles (half colour clocks) from a tick start on a
/// whole colour clock; they were fitted on the reference's EXACT cycle count of each write (a logged
/// write time is the floor of half the running count), so the parities are the reference's. Many of
/// them are odd (the gap between two writes then depends on the parity of the cycles run since the
/// tick start).
/// The work before a voice's PER write, in half colour clocks (fitted jointly on the reference
/// logs): by what the previous voice wrote (a sample start or synth LOC per voice, a plain VOL, a
/// silent voice's VOL), and per arpeggio byte read, arpeggio jump, envelope step, envelope point
/// loaded (other than envelope 1's), envelope jump (other than envelope 1's), envelope 1's point
/// loaded and envelope 1's jump (its duration scale is worked out again for every point tried).
const PRE_LOC_H: [u64; 4] = [1765, 1677, 1708, 1739];
const PRE_VOL_H: u64 = 1765;
const PRE_MUTED_H: [u64; 4] = [1486, 1486, 1486, 1486];
const PRE_FEAT_H: [u64; 7] = [81, 73, 69, 435, 222, 549, 208];
/// The portamento step: the period is moved to the target by a step (up, up and reached, down, down
/// and reached) or already there; an arpeggio voice adds a little.
const PRE_PM_H: [u64; 7] = [0, 0, 123, 120, 123, 173, 56];
const PRE_ARP_H: u64 = 9;
/// A voice whose update aborts makes no write: its work until an envelope ran out of jumps (envelope
/// 0; 29 more for each envelope after it), besides the portamento, arpeggio and envelope work counted
/// as for the PER write; measured on the reference's call ends and the next voice's write (envelopes
/// 0, 1 and 2). The next voice follows as after a plain VOL.
const SKIP_ENV_H: i64 = 500;
const SKIP_ENV_NEXT_H: i64 = 29;
/// ... and until the arpeggio ran out of jumps (the rest of the tick's voices are left out).
const SKIP_ARP_H: i64 = 78;
/// A silent voice's VOL write (half clocks), per voice, when the previous voice ended with a plain
/// VOL (or the tick starts); after another kind of end the lead differs as it does for the PER write.
const MUTED_VOL_H: [i64; 4] = [672, 695, 718, 708];
/// A row's first write (DMACON 0x8200) comes this much later than the call would have returned
/// without the row (`tail_h` after the last write: of a voice's turn, or of the row before when that
/// took no time), in half clocks; costs of the advance in between add to it.
const ROW_LEAD_H: u64 = 420;
/// Sequence entry costs (half clocks; exact on the reference's gaps: a loop entry costs 76 more the
/// first time it is met than when repeated, and a finished loop 59 less than a repeated one), and
/// the advance's block-end, block-start and move-to-the-next-entry costs.
const SQ_NOP: u64 = 305;
const SQ_SPEED: u64 = 338;
const SQ_TRANSPOSE: u64 = 367;
const SQ_FILTER_OFF: u64 = 388;
const SQ_FILTER_ON: u64 = 398;
const SQ_JUMP: u64 = 293;
/// A loop entry: met for the first time and not repeated, repeated (taken), finished after repeats,
/// met for the first time and taken.
const SQ_LOOP_ONCE: u64 = 341;
const SQ_LOOP_TAKEN: u64 = 324;
const SQ_LOOP_DONE: u64 = 265;
const SQ_LOOP_FIRST: u64 = 400;
/// (half colour clocks) A block end alone costs 1198, with the next block's start 2397.
const ADV_END_BLOCK: u64 = 1198;
const ADV_START_BLOCK: u64 = 1199;
const ADV_NEXT_ENTRY: u64 = 784;
/// The work of a row, in 68000 cycles (half colour clocks). Each voice costs the same whatever its
/// number and whatever follows it: an idle voice, a note, a release (plus the instrument change, plus
/// the instrument-relative record bit, less a skipped division), the arpeggio bit, the two note flags
/// (bits 7 and 6 of the row's third byte), a retrigger (more on a block's first row, more again when
/// the block retriggers the fourth envelope), a voice the block mutes while its row has a note, and
/// the channel-volume update (set, up, up and clamped, down, down and clamped, same).
const ROW_IDLE: i64 = 433;
const ROW_NOTE: i64 = 1454;
const ROW_REL: i64 = 1341;
const ROW_REL_ID: i64 = 545;
const ROW_REL_CNT0: i64 = 244;
const ROW_ARP: i64 = 68;
const ROW_F80: i64 = 10;
const ROW_F40: i64 = 14;
const ROW_RETRIG: i64 = 905;
const ROW_RETRIG_ROW0: i64 = 115;
const ROW_RETRIG_E3: i64 = 174;
const ROW_BLOCK_MUTE: i64 = 39;
const ROW_CV: [i64; 6] = [0, 119, 116, 119, 167, 54];
/// A write of the row comes this long after the previous one, plus the work of the voices since: after
/// the row start's DMACON and after any later write; a sample or synth start's LEN write (by event
/// type 3, 4, 5) comes this much sooner than a key-off's DMACON.
const ROW_AFTER: [i64; 2] = [70, 86];
const ROW_LEN: [i64; 3] = [-95, -76, -49];
/// Per voice: LEN to LOC and LOC to INTENA of a synth voice's row start, and of a moving sample's
/// (half colour clocks).
const ROW_SYN_H: [(u64, u64); 4] = [(249, 325), (280, 356), (311, 387), (319, 395)];
const ROW_SMP_H: [(u64, u64); 4] = [(267, 201), (298, 232), (329, 263), (337, 271)];
/// The tail of a tick's call after its last write, in 68000 cycles (half colour clocks; the call ends
/// on a whole colour clock, an odd cycle is dropped): after the AUDxDAT write of a key tail (by
/// channel), a voice's LOC (2 less for a moving sample's), a plain VOL, a silent voice's VOL, a
/// sample or synth start's DMACON in a row (plus the work of the voices after it), the row start's
/// DMACON when the row writes nothing else (16 less: the difference of `ROW_AFTER`), and the song
/// end's DMACON. Measured on the reference's call ends (every tick of the 21 modules).
const TAIL_DAT_H: [u64; 4] = [637, 567, 497, 427];
const TAIL_LOC_H: u64 = 744;
const TAIL_VOL_H: u64 = 762;
const TAIL_MUTED_H: u64 = 483;
const TAIL_START_H: u64 = 766;
const TAIL_ROW_H: u64 = 750;
const TAIL_END_H: u64 = 232;
/// The tail of an interrupt handler after its last write, and of one that only disabled its interrupt.
const TAIL_HANDLER_H: u64 = 176;
const TAIL_HANDLER_NONE_H: u64 = 418;
/// The tick's code before the work of the voices (half clocks). The tick is called one timer period
/// after the song start and every period after that (the engine's default), or when the call that
/// was running then returns, as the reference's (measured on its guest calls).
const TICK_ENTRY_H: u64 = 116;
/// Handler gaps per channel in half clocks (the same for both handler shapes): LOC to LEN, and LEN to
/// DMACON (the loop shape's LEN to INTENA is 10 more).
const H_LEN_H: [u64; 4] = [219, 250, 281, 289];
const H_DMA_H: [u64; 4] = [187, 218, 249, 257];

/// Instructions budget guards.
const SEQ_GUARD: usize = 4096;
const ADV_GUARD: usize = 100_000;
/// The song-end INTENA write comes this much later than the call would have returned without the end
/// (`tail_h` after the last write, as for a row; half clocks) plus the sequence entries met on the way
/// to the end entry; colour clocks from it to the DMACON write.
const END_LEAD_H: u64 = 857;
const END_TO_DMACON: u64 = 10;
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
    /// The last LOC was a moving sample's (type 4 or 5): the work after it is two cycles shorter than
    /// after a synth's.
    last_mov: bool,
    /// Work counters of the voice update in progress, for the cost of the call: arpeggio bytes
    /// read, arpeggio jumps, envelope steps, envelope points loaded, envelope jumps, envelope 1's
    /// points loaded and envelope 1's jumps (counted in the two before as well).
    feat: [u32; 7],
    pm: u8,
    /// A carried half colour clock (the synthesis costs are in half clocks).
    half: u64,
    /// The start-up call is running: sequencer costs add up in `sq_acc` instead of passing.
    start_origin: u64,
    starting: bool,
    sq_acc: u64,
    first_block_seen: bool,
    /// The tail of the tick's call after the last write made (half clocks).
    tail_h: u64,
    /// The sequence entry costs a tick's advance has paid since the last row (half clocks), and the
    /// time (clock, half clock) that advance began at.
    sq_paid: u64,
    adv_at: (u64, u64),
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

    /// Lets `c` whole colour clocks pass.
    fn wc(&mut self, hw: &mut Hw, c: u64) {
        self.wh(hw, 2 * c);
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
                    if self.loopc == 0xFF && n == 1 {
                        SQ_LOOP_ONCE
                    } else if self.loopc == 1 {
                        SQ_LOOP_DONE
                    } else if self.loopc == 0xFF {
                        SQ_LOOP_FIRST
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
                        let h = 2 * self.start_origin + START_CIA_H + self.sq_acc - cost;
                        hw.bus.clock = h / 2;
                        self.half = h % 2;
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

    /// Charges a sequence entry (half colour clocks): in the start-up call the sum is kept, later
    /// the time passes.
    fn sq_cost(&mut self, hw: &mut Hw, cost: u64) {
        if self.starting {
            self.sq_acc += cost;
        } else {
            self.sq_paid += cost;
            self.wh(hw, cost);
        }
    }

    /// Charges an advance cost: in the start-up call it adds to the start-up time, later it passes.
    fn adv_cost(&mut self, hw: &mut Hw, cost: u64) {
        if self.starting {
            self.sq_acc += cost;
        } else {
            self.wh(hw, cost);
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
                self.adv_cost(hw, ADV_END_BLOCK);
                self.blk = self.blk.wrapping_add(26);
                self.rep = self.rep.wrapping_add(1);
            } else {
                self.rep = 0;
                skip = false;
            }
            if self.rep < self.entry(hw).0 & 0x1F {
                // the first block start of the song is part of the base start-up time
                if !self.first_block_seen {
                    self.first_block_seen = true;
                } else {
                    self.adv_cost(hw, ADV_START_BLOCK);
                }
                self.start_block(hw);
                continue;
            }
            self.adv_cost(hw, ADV_NEXT_ENTRY);
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
            self.adv_at = (hw.bus.clock, self.half);
            self.sq_paid = 0;
            if self.step2(hw, false)? {
                return Ok(());
            }
        }
        Err(End)
    }

    // ---------------------------------------------------------------- rows

    fn process_row(&mut self, hw: &mut Hw) {
        self.wh(hw, self.tail_h + ROW_LEAD_H);
        hw.dmacon(0x8200);
        let blk = self.blk;
        let mut keys = [false; 4];
        // the work of the voices since the last write (half clocks), and whether that was the row start
        let mut after = 0usize;
        let mut work = 0i64;
        for v in 0..4usize {
            let be = blk.wrapping_add(2 + 6 * v as u32);
            let (b0, b1, b2, b3) = (rd8(hw, be), rd8(hw, be + 1), rd8(hw, be + 2), rd8(hw, be + 3));
            let r = self.c[2].wrapping_add(4 * self.v[v].trow as u32);
            let (r0, r1, r2, r3) = (rd8(hw, r), rd8(hw, r + 1), rd8(hw, r + 2), rd8(hw, r + 3));
            // channel volume
            let cvc = {
                let t = 4 * (b2 & 63);
                let s = b2 >> 6;
                let vo = &mut self.v[v];
                let cvc = if s == 0 {
                    0
                } else if vo.chvol < t {
                    if vo.chvol.wrapping_add(s) >= t { 2 } else { 1 }
                } else if vo.chvol > t {
                    if vo.chvol.wrapping_sub(s) <= t { 4 } else { 3 }
                } else {
                    5
                };
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
                cvc
            };
            let arp = ((r0 >> 6) & 1) as i64;
            let quiet = r0 & 63 == 63 || b0 & 63 == 63;
            if r0 & 0x40 != 0 {
                self.v[v].arp = self.c[3].wrapping_add(r3 as u32).wrapping_add(b3 as u32);
            } else {
                self.v[v].port = r3;
            }
            if quiet {
                let cur = self.v[v].inst;
                let instdiff = (rd8(hw, cur) != rd8(hw, cur.wrapping_add(1))) as i64;
                let active = self.v[v].active;
                let mut cnt0 = false;
                if active {
                    self.v[v].active = false;
                    cnt0 = self.release(hw, v, b1, r1, r2);
                }
                work += ROW_CV[cvc] + ROW_ARP * arp
                    + if r0 & 63 != 63 { ROW_BLOCK_MUTE } else { 0 }
                    + if active {
                        ROW_REL + ROW_REL_ID * instdiff + ROW_F80 * instdiff * (r2 >> 7) as i64 - if cnt0 { ROW_REL_CNT0 } else { 0 }
                    } else {
                        ROW_IDLE
                    };
                continue;
            }
            work += ROW_NOTE + ROW_CV[cvc] + ROW_ARP * arp + ROW_F80 * (r2 >> 7) as i64 + ROW_F40 * ((r2 >> 6) & 1) as i64;
            if r0 & 0x80 != 0 {
                work += ROW_RETRIG;
                if self.row == 0 {
                    work += ROW_RETRIG_ROW0 + if b0 & 0x80 != 0 { ROW_RETRIG_E3 } else { 0 };
                }
            }
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
            let ty = e >> 28;
            match ty {
                3 | 4 | 5 => {
                    let h = ROW_AFTER[after] + work + ROW_LEN[(ty - 3) as usize];
                    work = 0;
                    after = 1;
                    self.wh(hw, h.max(0) as u64);
                }
                _ => {
                    if r0 & 0x80 != 0 {
                        let h = ROW_AFTER[after] + work;
                        work = 0;
                        after = 1;
                        self.wh(hw, h.max(0) as u64);
                    }
                }
            }
            match ty {
                4 | 5 => {
                    hw.len(v, w & 0xFF);
                    self.wh(hw, ROW_SMP_H[v].0);
                    hw.loc(v, self.sample_addr(e, 0));
                    self.wh(hw, ROW_SMP_H[v].1);
                    self.intena(hw, 0x0080 << v);
                    self.wc(hw, 25);
                    hw.dmacon(0x8000 | 1 << v);
                    self.tail_h = TAIL_START_H;
                }
                3 => {
                    hw.len(v, (e & 0xFF) as u16);
                    self.wh(hw, ROW_SYN_H[v].0);
                    hw.loc(v, self.v[v].front);
                    self.wh(hw, ROW_SYN_H[v].1);
                    self.intena(hw, 0x0080 << v);
                    self.wc(hw, 25);
                    hw.dmacon(0x8000 | 1 << v);
                    self.tail_h = TAIL_START_H;
                }
                _ => {
                    if r0 & 0x80 != 0 {
                        keys[v] = true;
                        hw.dmacon(1 << v);
                    }
                }
            }
        }

        if after == 0 {
            self.tail_h = TAIL_ROW_H;
        }
        let mut first = true;
        for v in 0..4 {
            if !keys[v] {
                if !first && keys[v..].iter().any(|&k| k) {
                    self.wc(hw, 35);
                }
                continue;
            }
            {
                if first {
                    // the work of the voices after the row's last write comes before the key tail
                    let h = 193 + 70 * v as i64 + work;
                    work = 0;
                    self.wh(hw, h.max(0) as u64);
                } else {
                    self.wc(hw, 67);
                }
                first = false;
                self.intena(hw, 0x8000 | 0x0080 << v);
                self.wc(hw, 31);
                wr16(hw, 0xDFF0AA + 16 * v as u32, 0);
                self.tail_h = TAIL_DAT_H[v];
            }
        }
        // the work of the last voices, when no write follows
        if work > 0 {
            self.wh(hw, work as u64);
        }
    }

    fn release(&mut self, hw: &mut Hw, v: usize, b1: u8, r1: u8, r2: u8) -> bool {
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
        e.cnt == 0
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
        self.last_mov = false;
        for v in 0..4 {
            match self.voice(hw, v) {
                Ok(()) => {}
                Err(Skip::Voice) => {}
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
            // the song-end stop falls a fixed time after the last voice's turn or the last row (by what
            // that wrote last), plus the sequence entries met on the way to the end entry, whatever the
            // rest of the advance cost the port charged (measured: INTENA 1601 half clocks after a LOC,
            // 1619 after a VOL, 1340 after a silent voice's VOL; the DMACON 20 later)
            hw.bus.clock = self.adv_at.0;
            self.half = self.adv_at.1;
            self.wh(hw, self.tail_h + END_LEAD_H + self.sq_paid);
            self.intena(hw, 0x0780);
            self.wc(hw, END_TO_DMACON);
            hw.dmacon(0x000F);
            self.tail_h = TAIL_END_H;
        }
    }

    /// The cost of the arpeggio and envelope work counted in `feat` (half clocks).
    fn feat_cost(&self) -> u64 {
        let f = self.feat;
        PRE_FEAT_H[0] * f[0] as u64
            + PRE_FEAT_H[1] * f[1] as u64
            + PRE_FEAT_H[2] * f[2] as u64
            + PRE_FEAT_H[3] * (f[3] - f[5]) as u64
            + PRE_FEAT_H[4] * (f[4] - f[6]) as u64
            + PRE_FEAT_H[5] * f[5] as u64
            + PRE_FEAT_H[6] * f[6] as u64
    }

    /// The work of a voice whose update aborts passes (`h` half clocks and the work counted in
    /// `feat`); what follows comes as after a plain VOL.
    fn skipped(&mut self, hw: &mut Hw, h: i64) {
        let h = h + self.feat_cost() as i64;
        self.wh(hw, h.max(0) as u64);
        self.last_kind = 1;
        self.last_mov = false;
        self.tail_h = TAIL_VOL_H;
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
        if k == 1 {
            self.feat[5] += 1;
        }
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
            if e1 {
                self.feat[6] += 1;
            }
            e.pos = e.pos.wrapping_add((lev * 2) as u32);
            kk = kk.wrapping_add(1);
        }
        Err(Skip::Voice)
    }

    fn voice(&mut self, hw: &mut Hw, v: usize) -> Result<(), Skip> {
        self.feat = [0; 7];
        self.pm = 0;
        let inst = self.v[v].inst;
        // what the previous voice's end leaves before this voice's work, against a plain VOL
        let lead = match self.last_kind {
            0 => PRE_LOC_H[v] - if self.last_mov { 2 } else { 0 },
            1 => PRE_VOL_H,
            _ => PRE_MUTED_H[v],
        } as i64
            - PRE_VOL_H as i64;
        if self.v[v].chvol == 0 {
            self.wh(hw, (lead + MUTED_VOL_H[v]).max(0) as u64);
            hw.vol(v, 0);
            self.tail_h = TAIL_MUTED_H;
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
                    self.skipped(hw, lead + SKIP_ARP_H);
                    return Err(Skip::All);
                }
                let off = (((b & 0x7F) << 1) as i8 >> 1) as i32;
                a = a.wrapping_add(off as u32);
            }
            if !found {
                self.skipped(hw, lead + SKIP_ARP_H);
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
                self.pm = 1;
            } else if vo.per < target {
                vo.per = vo.per.wrapping_add(sp);
                self.pm = 2;
                if vo.per >= target {
                    vo.per = target;
                    self.pm = 3;
                }
            } else if vo.per > target {
                vo.per = vo.per.wrapping_sub(sp);
                self.pm = 4;
                if vo.per <= target {
                    vo.per = target;
                    self.pm = 5;
                }
            } else {
                self.pm = 6;
            }
        }
        // envelopes
        let scales = [rd8(hw, inst.wrapping_add(3)), rd8(hw, inst.wrapping_add(16)), rd8(hw, inst.wrapping_add(4)), rd8(hw, inst.wrapping_add(5))];
        for (k, &sc) in scales.iter().enumerate() {
            if sc != 0 && self.env_step(hw, v, k, sc as u32, k == 1).is_err() {
                let h = lead
                    + SKIP_ENV_H
                    + SKIP_ENV_NEXT_H * k as i64
                    + PRE_PM_H[self.pm as usize] as i64
                    + if r0 & 0x40 != 0 { PRE_ARP_H as i64 } else { 0 };
                self.skipped(hw, h);
                return Err(Skip::Voice);
            }
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
        let h = lead
            + (PRE_VOL_H + PRE_PM_H[self.pm as usize] + if r0 & 0x40 != 0 { PRE_ARP_H } else { 0 } + self.feat_cost()) as i64;
        self.wh(hw, h.max(0) as u64);
        hw.per(v, per);
        self.wc(hw, PER_VOL[v]);
        hw.vol(v, vol);
        self.tail_h = TAIL_VOL_H;
        // the event
        let (e, w) = self.evrec(hw, self.v[v].ev);
        self.last_mov = false;
        match e >> 28 {
            4 | 5 => {
                let f = self.moving(v, e, w, e >> 28 == 4);
                self.wh(hw, if e >> 28 == 4 { 984 } else { 765 });
                self.last_mov = true;
                hw.loc(v, self.sample_addr(e, f));
                self.tail_h = TAIL_LOC_H - 2;
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
                self.tail_h = TAIL_LOC_H;
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
        synth_cost_h(&SynthFeat { mode, op, n: n as u64, len0: e & 63 == 0, comb: comb_on, fvar, fill, skip, t1, t2, q22, g1nz, g2nz, fresh })
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
        // the handler's code starts on a whole colour clock, where the engine takes the interrupt (the
        // end of the previous call, or later when the request comes after it)
        self.half = 0;
        let lat = START_IRQ_H;
        self.wh(hw, lat);
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
                self.wh(hw, if t == 1 { 507 } else { 468 });
                hw.loc(ch, self.sample_addr(e, 0));
                self.wh(hw, H_LEN_H[ch]);
                hw.len(ch, w);
                self.wh(hw, H_DMA_H[ch]);
                hw.dmacon(0x8000 | 1 << ch);
                self.wc(hw, 32);
                self.intena(hw, 0x8000 | bit);
            }
            0 => {
                self.wc(hw, 192);
                hw.loc(ch, self.sample_addr(e, 0));
                self.wh(hw, H_LEN_H[ch]);
                hw.len(ch, w);
                self.wh(hw, H_DMA_H[ch] + 10);
                self.intena(hw, bit);
                self.wc(hw, 21);
                hw.dmacon(0x8000 | 1 << ch);
            }
            13 => {
                self.wh(hw, 231);
                hw.loc(ch, self.silence);
                self.wc(hw, 31);
                hw.len(ch, 1);
                self.wc(hw, 26);
                self.intena(hw, bit);
            }
            _ => {}
        }
        // the code returns a little after its last write (the one that only disabled its interrupt
        // does more first); the call ends on a whole colour clock
        self.wh(hw, if matches!(e >> 28, 0 | 1 | 2 | 13) { TAIL_HANDLER_H } else { TAIL_HANDLER_NONE_H });
        self.half = 0;
    }
}

/// What the cost of one synthesis depends on.
struct SynthFeat {
    mode: u32,
    op: u32,
    n: u64,
    /// The record's length code is 0 (128 bytes).
    len0: bool,
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

/// The half colour clocks the synthesis of one buffer takes, from VOL to LOC. Every stage is a constant
/// plus a per-sample term, and the stages add up (the cost of an operation grows with its number in
/// steps of 27 for mode 2): an exact linear fit of 2500 combinations of the reference's costs. A
/// length code of 0 (128 bytes) costs 8 cycles more, whatever the stages (21 probes).
fn synth_cost_h(f: &SynthFeat) -> u64 {
    let n = f.n;
    let mut c: u64 = match f.mode {
        0 => match f.op {
            0 => {
                if f.fill {
                    1319 + 22 * n
                } else {
                    1261
                }
            }
            1 => 1695 + 144 * (n >> 3),
            2 => 1328 + 76 * n,
            _ => 1262,
        },
        1 => {
            if f.skip {
                810
            } else {
                let extra: i64 = match f.t1 {
                    4 => 633,
                    5 => 414,
                    1 => -7,
                    _ => 0,
                };
                let b: i64 = match f.op {
                    0 => 988 + 22 * n as i64,
                    1 => 1013 + 42 * n as i64,
                    2 => 1047 + 46 * n as i64,
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
            let extra: u64 = [f.t1, f.t2]
                .iter()
                .map(|t| match t {
                    4 => 621,
                    5 => 402,
                    _ => 0,
                })
                .sum();
            let (b, k): (u64, u64) = match f.op {
                1 => (1173, 54),
                2 => (1200, 116),
                3 => (1227, 38),
                4 => (1254, 38),
                5 => (1281, 38),
                6 => (1308, 40),
                7 => (1351, 52),
                9 => (1389, 70),
                _ => (3000, 0),
            };
            b + k * n + extra
        }
        _ => 689,
    };
    if f.len0 {
        c += 8;
    }
    if f.comb {
        c += 924 + 417 * n;
    }
    if f.fvar != 0 {
        if f.q22 {
            c += 43 + if f.fresh { 52 } else { 0 };
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
            c += 611;
            if f.g1nz {
                c += 327;
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
    // the two chip buffers lie where the reference's do (measured on its AUDxLOC writes: the 2-byte
    // silence buffer first, the eight synthesis buffers 8 bytes after it), so that a sample start
    // that an envelope moves out of the module reads the same memory; two more bytes for the byte
    // the comb stage writes past the last buffer
    // (for the same reason the module's path is left in the host's path buffer, as the host of the
    // 68k player leaves it before the player starts: it lies below the module, where such a sample
    // start can point; measured on the audio DMA fetches of r2mix and r3sample)
    if m.deli.path_buf != 0 {
        let full = m.bus.read_cstr(m.deli.dir_buf) + &m.bus.read_cstr(m.deli.file_buf);
        m.bus.write_cstr(m.deli.path_buf, &full);
    }
    let silence = m.alloc(2);
    let area = m.alloc(1024 + 2);
    if silence == 0 || area == 0 {
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
        silence,
        last_kind: 1,
        last_mov: false,
        feat: [0; 7],
        pm: 0,
        half: 0,
        tail_h: TAIL_VOL_H,
        start_origin: 0,
        starting: true,
        sq_acc: 0,
        first_block_seen: false,
        sq_paid: 0,
        adv_at: (0, 0),
    };
    for v in 0..4 {
        r.v[v].front = area + 128 * v as u32;
        r.v[v].back = area + 128 * (v as u32 + 4);
    }
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut hw = Hw { bus: &mut m.bus };
    r.start_origin = origin;
    r.last_kind = 0;
    let started = r.sequencer(&mut hw);
    // the rest of the start-up call (blocks of no rows, further entries) passes in time
    r.starting = false;
    if started.is_ok() {
        r.tail_h = TAIL_LOC_H;
        let h = 2 * origin + START_PLAIN_H + r.sq_acc - (TAIL_LOC_H + ROW_LEAD_H);
        hw.bus.clock = h / 2;
        r.half = h % 2;
        // the first row is processed without moving the pointer
        if r.step2(&mut hw, true).is_err() {
            r.ended = true;
        }
    } else {
        r.ended = true;
    }
    r.last_kind = 1;
    // the song starts (Paula's output and line grids restart, the timer's first period begins) where
    // the start-up call returns: after its row as a tick's call after a row, a little sooner, and on
    // a whole colour clock (an odd cycle is dropped, as at the end of every call)
    let end_h = 2 * hw.bus.clock + r.half + r.tail_h - START_RETURN_H;
    hw.bus.clock = end_h / 2;
    r.half = 0;
    Some(Box::new(r))
}

impl Replayer for Aps {
    fn format(&self) -> &'static str {
        "AProSys"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.ended
    }

    fn tick(&mut self, hw: &mut Hw) {
        // a tick is a guest call: it starts on a whole colour clock, whatever the half clock the
        // previous call ended on (the clock only counts whole colour clocks)
        self.half = 0;
        self.wh(hw, TICK_ENTRY_H);
        self.tick_voices(hw);
        // the call returns a while after its last write
        self.wh(hw, self.tail_h);
        self.half = 0;
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
