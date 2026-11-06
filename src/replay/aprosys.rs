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

/// The start-up call's first DMACON write and the CIA write of a filter command (in half colour
/// clocks from the run's origin), before the sum of the costs of the sequence entries met until
/// them (the first entry's included).
/// Colour clocks the song start (the engine's clock when the start-up call returns) lies from the end
/// of the start-up call, for a keyed and a plain start.
const SONG_START_KEYED: i64 = -73;
const SONG_START_PLAIN: i64 = 327;
const START_PLAIN_H: u64 = 7967;
const START_CIA_H: u64 = 5207;
/// Colour clocks from the end of the start-up call (the audio interrupts it requests come after)
/// to the first tick's call: with a keyed voice (plus the latency of the first interrupt handler
/// after the last AUDxDAT write, `lat_h`), and without.
const START_TO_TICK_KEYED_H: u64 = 141849;
const START_TO_TICK_H: u64 = 142650;
/// Half colour clocks from the last AUDxDAT write of channel 0 to the end of the start-up call
/// (70 less per channel), and from the audio interrupt being taken (at an output sample) to the
/// handler's first write.
const START_TAIL_H: u64 = 672;
const START_IRQ_H: u64 = 112;
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
/// loaded (other than envelope 1's), envelope jump and envelope 1's point loaded.
const PRE_LOC_H: [u64; 4] = [1765, 1677, 1708, 1739];
const PRE_VOL_H: u64 = 1765;
const PRE_MUTED_H: [u64; 4] = [1486, 1486, 1486, 1486];
const PRE_FEAT_H: [u64; 6] = [81, 73, 69, 435, 222, 549];
/// The portamento step: the period is moved to the target by a step (up, up and reached, down, down
/// and reached) or already there; an arpeggio voice adds a little.
const PRE_PM_H: [[u64; 7]; 2] = [[0, 0, 123, 119, 123, 174, 56], [0, 0, 123, 120, 123, 173, 56]];
const PRE_ARP_H: u64 = 9;
/// A silent voice's VOL write after the previous voice's last write (half clocks): by what that was
/// (a sample start, a plain VOL, a silent voice's VOL), per voice.
const MUTED_AFTER: [[u64; 4]; 3] = [[673, 607, 660, 681], [674, 695, 718, 708], [673, 416, 439, 429]];
/// The gap before a row's work (DMACON 0x8200).
const ROW_PRE: u64 = 580;
/// ... after a plain volume write, and after a silent voice's.
const ROW_PRE_VOL: u64 = 591;
const ROW_PRE_MUTED: u64 = 451;
/// Sequence entry costs (measured with probe modules; a loop's first-met-and-final and
/// taken/not-first costs are split evenly), and the advance's block-end, block-start and
/// move-to-the-next-entry costs.
const SQ_NOP: u64 = 305;
const SQ_SPEED: u64 = 338;
const SQ_TRANSPOSE: u64 = 367;
const SQ_FILTER_OFF: u64 = 388;
const SQ_FILTER_ON: u64 = 398;
const SQ_JUMP: u64 = 293;
/// A loop entry: met for the first time and not repeated, repeated (taken), finished after repeats.
const SQ_LOOP_ONCE: u64 = 341;
const SQ_LOOP_TAKEN: u64 = 324;
const SQ_LOOP_DONE: u64 = 342;
/// (half colour clocks) The block-end and block-start costs only matter as their sum.
const ADV_END_BLOCK: u64 = 1199;
const ADV_START_BLOCK: u64 = 1198;
const ADV_NEXT_ENTRY: u64 = 784;
/// The work of voices after the last write of a row, when no write follows (a late tick or a
/// handler comes next), in half colour clocks, from the leftovers of the reference logs: an idle
/// voice, a note without a write, a release (plus the instrument change, less a skipped division),
/// the arpeggio bit, and the channel-volume update class (set, up, up and clamped, down, down and
/// clamped, same).
const LF_IDLE: i64 = 433;
const LF_NOTE: i64 = 1454;
const LF_REL: i64 = 1342;
const LF_REL_ID: i64 = 544;
const LF_REL_CNT0: i64 = 244;
const LF_ARP: i64 = 68;
const LF_CV: [i64; 6] = [0, 119, 0, 145, 0, 57];
/// A voice the block mutes while its row has a note (half clocks, more than an idle voice).
const LF_BLOCK_MUTE: i64 = 39;
/// The cost of a row's work between two writes, in half colour clocks (fitted jointly on the
/// reference logs): a constant by the previous write (row start, key-off, other), the work of each
/// voice processed since (by voice, and by what the voice does), and a constant by the write that
/// ends it (the sample or synth start of a voice, a key-off, or the first interrupt enable).
struct RowCost {
    a: [i64; 3],
    tl_len: [[i64; 4]; 3],
    tl_key: [[i64; 4]; 4],
    note: [i64; 4],
    idle: [i64; 4],
    rel: [i64; 4],
    rel_id: [i64; 4],
    rel_id280: [i64; 4],
    r280: [i64; 4],
    r240: [i64; 4],
    retrig: [i64; 4],
    row0r: [i64; 4],
    e3: [i64; 4],
    n_arp: [i64; 4],
    q_arp: [i64; 4],
    n_cv: [[i64; 4]; 6],
    q_cv: [[i64; 4]; 6],
    rel_cnt0: i64,
    rel_div: i64,
    idle_then_note: i64,
}
const RC_MAIN: RowCost = RowCost {
    a: [1639, 86, 86],
    tl_len: [[-95, -95, -95, 1413], [-76, -76, -76, 1432], [-49, -49, -49, 1459]],
    tl_key: [[0, -1, -1, 1507], [1, 0, 0, 1508], [1, 0, 0, 1508], [1, 0, 0, 1508]],
    note: [0, 1573, 1621, 0],
    idle: [-1023, 549, 550, 0],
    rel: [-1629, -57, -56, 0],
    rel_id: [545, 545, 545, 1337],
    rel_id280: [10, 10, 10, 0],
    r280: [10, 10, 10, 10],
    r240: [14, 14, 14, 14],
    retrig: [905, 905, 905, 905],
    row0r: [115, 115, 115, 115],
    e3: [174, 174, 174, 174],
    n_arp: [68, 68, 68, 68],
    q_arp: [68, 68, 68, 466],
    n_cv: [[-116, -119, -167, -54], [3, 0, -48, 65], [0, 0, -51, 0], [3, 0, -48, 65], [51, 0, 0, 0], [-62, -65, -113, 0]],
    q_cv: [[-114, -116, -117, -1074], [5, 3, 2, 0], [2, 0, -1, 0], [5, 3, 2, 0], [0, 0, 0, 0], [-60, 0, -24, 0]],
    rel_cnt0: 1270,
    rel_div: 1514,
    idle_then_note: 0,
};
const RC_INT: RowCost = RowCost {
    a: [0, -328, -328],
    tl_len: [[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
    tl_key: [[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
    note: [0, 0, 1574, 1974],
    idle: [0, 0, 552, 1009],
    rel: [0, 0, -516, 0],
    rel_id: [0, 0, 2196, 545],
    rel_id280: [0, 0, 0, 10],
    r280: [0, 0, -111, 13],
    r240: [0, 0, 0, 0],
    retrig: [0, 0, 0, 0],
    row0r: [0, 0, 0, 0],
    e3: [0, 0, 0, 0],
    n_arp: [0, 0, 184, 70],
    q_arp: [0, 0, 68, 68],
    n_cv: [[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
    q_cv: [[0, 491, -61, -55], [0, 0, 0, 65], [0, 0, 0, 0], [0, 699, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
    rel_cnt0: 1673,
    rel_div: 1917,
    idle_then_note: 0,
};
/// Per voice: LEN to LOC and LOC to INTENA of a synth voice's row start, and of a moving sample's
/// (half colour clocks).
const ROW_SYN_H: [(u64, u64); 4] = [(249, 325), (280, 356), (311, 387), (319, 395)];
const ROW_SMP_H: [(u64, u64); 4] = [(267, 201), (298, 232), (329, 263), (337, 271)];
/// Colour clocks from the last write of a tick to the first interrupt handler write, and from the
/// end of one handler to the next one's start.
/// (the first handler's start follows the tick's last write: by what that was, in half clocks:
/// AUDxDAT of channel 0..3, a LOC, a plain VOL, a silent voice's VOL, a sample start's DMACON).
const LAT_DAT_H: [u64; 4] = [897, 827, 757, 687];
const LAT_LOC_H: u64 = 1003;
const LAT_VOL_H: u64 = 1022;
const LAT_MUTED_H: u64 = 743;
const LAT_START_H: u64 = 1026;
/// What a tick whose timer is already due adds to its start, by how the previous call ended: after
/// a LOC, a VOL, a sample start or key tail, an interrupt handler (half clocks).
const LATE_LOC_H: u64 = 860;
const LATE_VOL_H: u64 = 878;
const LATE_START_H: u64 = 882;
const LATE_HANDLER_H: u64 = 292;
/// Handler gaps per channel: LOC to LEN, and LEN to the next write.
const H_LEN: [u64; 4] = [109, 125, 140, 144];
const H_DMA: [u64; 4] = [94, 109, 125, 129];
const IRQ_NEXT: u64 = 218;

/// Instructions budget guards.
const SEQ_GUARD: usize = 4096;
const ADV_GUARD: usize = 100_000;
/// Colour clocks from the end of the last voice's turn to the song-end INTENA write, and from it to the
/// DMACON write.
const END_TO_INTENA: u64 = 670;
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
    start_keyed: bool,
    /// Work counters of the voice update in progress, for the cost of the call: arpeggio bytes
    /// read, arpeggio jumps, envelope steps, envelope points loaded, envelope jumps.
    feat: [u32; 6],
    pm: u8,
    /// A carried half colour clock (the synthesis costs are in half clocks).
    half: u64,
    /// The start-up call is running: sequencer costs add up in `sq_acc` instead of passing.
    start_origin: u64,
    starting: bool,
    sq_acc: u64,
    first_block_seen: bool,
    first_irq: bool,
    last_key: usize,
    irq_end: u64,
    lat_h: u64,
    /// The clock at the end of the last call (a tick or an interrupt handler), and what a tick that
    /// finds the timer already due adds to start (half clocks).
    act_end: u64,
    late_h: u64,
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
            // (a tick's sequencer pays a little more than the start-up call's)
            let adj: i64 = if cost == SQ_NOP {
                3
            } else if cost == SQ_LOOP_TAKEN {
                -2
            } else if cost == SQ_SPEED {
                2
            } else {
                0
            };
            self.wh(hw, (cost as i64 + adj) as u64);
        }
    }

    /// Charges an advance cost: in the start-up call it adds to the start-up time, later it passes.
    fn adv_cost(&mut self, hw: &mut Hw, cost: u64) {
        if self.starting {
            self.sq_acc += cost;
        } else {
            self.wh(hw, if cost == ADV_END_BLOCK { cost - 1 } else { cost });
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
            if self.step2(hw, false)? {
                return Ok(());
            }
        }
        Err(End)
    }

    // ---------------------------------------------------------------- rows

    fn process_row(&mut self, hw: &mut Hw) {
        self.wh(hw, match self.last_kind {
            2 => 2 * ROW_PRE_MUTED + 1,
            1 => 2 * ROW_PRE_VOL,
            _ => 2 * ROW_PRE + 4 - if self.last_mov { 2 } else { 0 },
        });
        hw.dmacon(0x8200);
        let blk = self.blk;
        let mut keys = [false; 4];
        // the work charged since the last write (with the row's cost tables, and the tail's)
        let mut prev = 0usize;
        let mut acc = [0i64; 2];
        // the work of the voices since the last write, as it takes when no write follows (half clocks)
        let mut lf = 0i64;
        let mut idle_before = false;
        let mut pending = false;
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
                for (t, rc) in [&RC_MAIN, &RC_INT].into_iter().enumerate() {
                    acc[t] += rc.q_arp[v] * arp + rc.q_cv[cvc][v];
                    if active {
                        acc[t] += rc.rel[v]
                            + rc.rel_id[v] * instdiff
                            + rc.rel_id280[v] * instdiff * (r2 >> 7) as i64
                            + if cnt0 { rc.rel_cnt0 } else { rc.rel_div };
                    } else {
                        acc[t] += rc.idle[v];
                    }
                }
                idle_before = !active;
                pending = true;
                lf += LF_CV[cvc] + LF_ARP * arp
                    + if r0 & 63 != 63 { LF_BLOCK_MUTE } else { 0 }
                    + if active { LF_REL + LF_REL_ID * instdiff - if cnt0 { LF_REL_CNT0 } else { 0 } } else { LF_IDLE };
                continue;
            }
            for (t, rc) in [&RC_MAIN, &RC_INT].into_iter().enumerate() {
                acc[t] += rc.note[v]
                    + rc.n_arp[v] * arp
                    + rc.n_cv[cvc][v]
                    + rc.r280[v] * (r2 >> 7) as i64
                    + rc.r240[v] * ((r2 >> 6) & 1) as i64;
                if idle_before {
                    acc[t] += rc.idle_then_note;
                }
                if r0 & 0x80 != 0 {
                    acc[t] += rc.retrig[v];
                    if self.row == 0 {
                        acc[t] += rc.row0r[v];
                        if b0 & 0x80 != 0 {
                            acc[t] += rc.e3[v];
                        }
                    }
                }
            }
            idle_before = false;
            pending = true;
            lf += LF_NOTE + LF_CV[cvc] + LF_ARP * arp + 10 * (r2 >> 7) as i64 + 14 * ((r2 >> 6) & 1) as i64;
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
                    let h = RC_MAIN.a[prev] + acc[0] + RC_MAIN.tl_len[(ty - 3) as usize][v] + (prev == 0) as i64;
                    acc = [0; 2];
                    lf = 0;
                    pending = false;
                    self.wh(hw, h.max(0) as u64);
                    prev = 2;
                }
                _ => {
                    if r0 & 0x80 != 0 {
                        let k = match ty {
                            1 => 1,
                            2 => 2,
                            13 => 3,
                            _ => 0,
                        };
                        let h = RC_MAIN.a[prev] + acc[0] + RC_MAIN.tl_key[k][v] + (k == 0) as i64;
                        acc = [0; 2];
                    lf = 0;
                        pending = false;
                        self.wh(hw, h.max(0) as u64);
                        prev = 1;
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
                    self.lat_h = LAT_START_H;
                }
                3 => {
                    hw.len(v, (e & 0xFF) as u16);
                    self.wh(hw, ROW_SYN_H[v].0);
                    hw.loc(v, self.v[v].front);
                    self.wh(hw, ROW_SYN_H[v].1);
                    self.intena(hw, 0x0080 << v);
                    self.wc(hw, 25);
                    hw.dmacon(0x8000 | 1 << v);
                    self.lat_h = LAT_START_H;
                }
                _ => {
                    if r0 & 0x80 != 0 {
                        keys[v] = true;
                        hw.dmacon(1 << v);
                    }
                }
            }
        }
        self.start_keyed = keys.iter().any(|&k| k);
        let mut first = true;
        for v in 0..4 {
            if !keys[v] {
                if !first && keys[v..].iter().any(|&k| k) {
                    self.wc(hw, 35);
                }
                continue;
            }
            {
                if first && pending {
                    let h = 193 + 70 * v as i64 + lf;
                    lf = 0;
                    self.wh(hw, h.max(0) as u64);
                } else if first {
                    self.wh(hw, 193 + 70 * v as u64 + (v == 3) as u64);
                } else {
                    self.wc(hw, 67);
                }
                first = false;
                self.intena(hw, 0x8000 | 0x0080 << v);
                self.wc(hw, 31);
                wr16(hw, 0xDFF0AA + 16 * v as u32, 0);
                self.lat_h = LAT_DAT_H[v];
                self.last_key = v;
            }
        }
        // the work of the last voices, when no write follows
        if lf > 0 {
            self.wh(hw, lf as u64);
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
        let (t_adv, h_adv) = (hw.bus.clock, self.half);
        if self.advance(hw).is_err() {
            self.ended = true;
            // the song-end stop falls a fixed time after the last voice's turn, whatever the
            // advance cost the port charged on the way to the end entry (measured: INTENA 670, DMACON 10 later)
            hw.bus.clock = t_adv;
            self.half = h_adv;
            self.wc(hw, END_TO_INTENA);
            self.intena(hw, 0x0780);
            self.wc(hw, END_TO_DMACON);
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
            e.pos = e.pos.wrapping_add((lev * 2) as u32);
            kk = kk.wrapping_add(1);
        }
        Err(Skip::Voice)
    }

    fn voice(&mut self, hw: &mut Hw, v: usize) -> Result<(), Skip> {
        self.feat = [0; 6];
        self.pm = 0;
        let inst = self.v[v].inst;
        if self.v[v].chvol == 0 {
            let mut h = MUTED_AFTER[self.last_kind as usize][v];
            if self.last_kind == 0 && v >= 2 {
                h = h + 1 - if self.last_mov { 2 } else { 0 };
            }
            self.wh(hw, h);
            hw.vol(v, 0);
            self.lat_h = LAT_MUTED_H;
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
        let f = self.feat;
        let base = match self.last_kind {
            0 => PRE_LOC_H[v] - if self.last_mov { 2 } else { 0 },
            1 => PRE_VOL_H,
            _ => PRE_MUTED_H[v],
        };
        let h = base
            + PRE_PM_H[(self.last_kind != 0) as usize][self.pm as usize]
            + if r0 & 0x40 != 0 { PRE_ARP_H } else { 0 }
            + PRE_FEAT_H[0] * f[0] as u64
            + PRE_FEAT_H[1] * f[1] as u64
            + PRE_FEAT_H[2] * f[2] as u64
            + PRE_FEAT_H[3] * (f[3] - f[5]) as u64
            + PRE_FEAT_H[4] * f[4] as u64
            + PRE_FEAT_H[5] * f[5] as u64;
        self.wh(hw, h);
        hw.per(v, per);
        self.wc(hw, PER_VOL[v]);
        hw.vol(v, vol);
        self.lat_h = LAT_VOL_H;
        // the event
        let (e, w) = self.evrec(hw, self.v[v].ev);
        self.last_mov = false;
        match e >> 28 {
            4 | 5 => {
                let f = self.moving(v, e, w, e >> 28 == 4);
                self.wh(hw, if e >> 28 == 4 { 984 } else { 765 });
                self.last_mov = true;
                hw.loc(v, self.sample_addr(e, f));
                self.lat_h = LAT_LOC_H;
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
                self.lat_h = LAT_LOC_H;
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
        let lat = if self.first_irq {
            // the host takes the audio interrupts of the start-up at the next output sample
            self.first_irq = false;
            self.half = 0;
            START_IRQ_H
        } else if hw.bus.clock <= self.irq_end + 2 {
            self.half = 0;
            2 * IRQ_NEXT
        } else {
            self.lat_h
        };
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
                self.wc(hw, if t == 1 { 253 } else { 234 });
                hw.loc(ch, self.sample_addr(e, 0));
                self.wh(hw, 2 * H_LEN[ch] + (ch == 0) as u64);
                hw.len(ch, w);
                self.wh(hw, 2 * H_DMA[ch] - (ch == 0) as u64);
                hw.dmacon(0x8000 | 1 << ch);
                self.wc(hw, 32);
                self.intena(hw, 0x8000 | bit);
            }
            0 => {
                self.wc(hw, 192);
                hw.loc(ch, self.sample_addr(e, 0));
                self.wh(hw, 2 * H_LEN[ch] + (ch == 3) as u64);
                hw.len(ch, w);
                self.wc(hw, H_DMA[ch] + 5);
                self.intena(hw, bit);
                self.wc(hw, 21);
                hw.dmacon(0x8000 | 1 << ch);
            }
            13 => {
                self.wc(hw, 115);
                hw.loc(ch, self.silence);
                self.wc(hw, 31);
                hw.len(ch, 1);
                self.wc(hw, 26);
                self.intena(hw, bit);
            }
            _ => {}
        }
        self.irq_end = hw.bus.clock;
        self.act_end = hw.bus.clock;
        self.late_h = LATE_HANDLER_H;
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

/// Exact corrections (half clocks) to `synth_cost_h`: reference minus the stage model, per
/// (mode, op, comb, fvar, fill, skip, t1, t2, q22, g1nz, g2nz); nonzero only.
const SYNTH_FIX: [([u8; 11], i8); 27] = [
    ([0, 0, 1, 1, 0, 0, 0, 0, 1, 1, 1], 1),
    ([0, 5, 0, 1, 1, 0, 0, 0, 1, 1, 1], 1),
    ([1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0], 1),
    ([1, 1, 0, 1, 1, 0, 1, 0, 0, 1, 1], -1),
    ([1, 1, 1, 1, 1, 0, 4, 0, 1, 1, 1], -1),
    ([1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1], 1),
    ([1, 3, 0, 0, 1, 0, 4, 0, 0, 0, 0], -1),
    ([1, 3, 0, 1, 1, 0, 5, 0, 0, 1, 1], -1),
    ([1, 4, 1, 1, 1, 0, 0, 0, 0, 1, 1], -1),
    ([1, 4, 1, 1, 1, 0, 4, 0, 1, 1, 1], -1),
    ([1, 4, 1, 1, 1, 0, 5, 0, 1, 1, 1], -1),
    ([1, 7, 1, 2, 1, 0, 0, 0, 1, 1, 1], 1),
    ([2, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0], -1),
    ([2, 2, 0, 1, 1, 0, 5, 1, 1, 1, 1], 1),
    ([2, 3, 0, 1, 1, 0, 0, 1, 1, 1, 1], -1),
    ([2, 3, 0, 2, 1, 0, 5, 1, 1, 1, 1], -1),
    ([2, 3, 0, 2, 1, 0, 5, 4, 0, 1, 1], -1),
    ([2, 3, 0, 2, 1, 0, 5, 5, 1, 1, 1], -1),
    ([2, 4, 1, 0, 1, 0, 0, 4, 0, 0, 0], 1),
    ([2, 4, 1, 1, 1, 0, 1, 0, 1, 1, 1], 1),
    ([2, 5, 0, 1, 1, 0, 1, 0, 0, 1, 1], -1),
    ([2, 5, 1, 0, 1, 0, 5, 0, 0, 0, 0], -1),
    ([2, 6, 0, 0, 1, 0, 0, 0, 0, 0, 0], -1),
    ([2, 7, 0, 2, 1, 0, 0, 4, 1, 1, 1], -1),
    ([2, 9, 0, 0, 1, 0, 0, 0, 0, 0, 0], -1),
    ([2, 9, 0, 1, 1, 0, 5, 1, 0, 1, 1], -1),
    ([3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0], -1),
];
/// ... and the exceptions by length: (mode, op, n, comb, fvar, fill, skip, t1, t2, q22, g1nz, g2nz) -> correction.
const SYNTH_FIX_N: [([u8; 12], i8); 1] = [
    ([0, 1, 8, 1, 1, 1, 0, 0, 0, 0, 1, 1], -1),
];

/// The half colour clocks the synthesis of one buffer takes, from VOL to LOC: measured on the
/// reference with probe modules (one stage at a time; the stages add up).
fn synth_cost_h(f: &SynthFeat) -> u64 {
    let k = [f.mode as u8, f.op as u8, f.comb as u8, f.fvar, f.fill as u8, f.skip as u8, f.t1 as u8, f.t2 as u8, f.q22 as u8, f.g1nz as u8, f.g2nz as u8];
    let mut fix = SYNTH_FIX.iter().find(|e| e.0 == k).map_or(0, |e| e.1 as i64);
    let kn = [k[0], k[1], f.n as u8, k[2], k[3], k[4], k[5], k[6], k[7], k[8], k[9], k[10]];
    if let Some(e) = SYNTH_FIX_N.iter().find(|e| e.0 == kn) {
        fix = e.1 as i64;
    }
    (synth_cost_base_h(f) as i64 + fix) as u64
}

fn synth_cost_base_h(f: &SynthFeat) -> u64 {
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
            1 => 1695 + 144 * (n >> 3) + ((n >> 3) & 1),
            2 => 1328 + 76 * n,
            _ => 1262,
        },
        1 => {
            if f.skip {
                810
            } else {
                let extra: i64 = match f.t1 {
                    4 => 634,
                    5 => 415,
                    1 => -7,
                    _ => 0,
                };
                let b: i64 = match f.op {
                    0 => 987 + 22 * n as i64,
                    1 => 1014 + 42 * n as i64,
                    2 => 1047 + 46 * n as i64,
                    3 => {
                        if n >= 3 {
                            1080 + 46 * n as i64
                        } else {
                            1034
                        }
                    }
                    4 => 1445 + 243 * n as i64,
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
                3 => (1229, 38),
                4 => (1254, 38),
                5 => (1282, 38),
                6 => (1309, 40),
                7 => (1354, 52),
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
        last_mov: false,
        start_keyed: false,
        feat: [0; 6],
        pm: 0,
        half: 0,
        irq_end: 0,
        lat_h: LAT_VOL_H,
        act_end: u64::MAX / 2,
        late_h: 0,
        start_origin: 0,
        starting: true,
        sq_acc: 0,
        first_block_seen: false,
        first_irq: false,
        last_key: 0,
        first_tick: None,
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
        let h = 2 * origin + START_PLAIN_H + r.sq_acc - (2 * ROW_PRE + 4);
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
    // the timer starts after the start-up call, not after its interrupts
    let keyed = r.start_keyed;
    if keyed {
        // the start-up routine returns a little after its last AUDxDAT write (less for a later
        // channel: the loop over the voices has fewer turns left)
        r.wh(&mut hw, START_TAIL_H - 70 * r.last_key as u64);
        r.first_irq = true;
    }
    // the first tick follows the end of the call to the half clock
    let ft = 2 * hw.bus.clock + r.half + if keyed { START_TO_TICK_KEYED_H } else { START_TO_TICK_H };
    r.first_tick = Some(ft.div_ceil(2));
    // the song start (where Paula's output and line grids restart) is this far from the end of the
    // start-up call in the reference (measured on the first write of each module)
    hw.bus.clock = (hw.bus.clock as i64 + if keyed { SONG_START_KEYED } else { SONG_START_PLAIN } + r.half as i64) as u64;
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
        // a tick is a guest call: it starts on a whole colour clock, whatever the half clock the
        // previous call ended on (the clock only counts whole colour clocks)
        self.half = 0;
        if hw.bus.clock <= self.act_end + 1 {
            self.wh(hw, self.late_h);
        }
        self.tick_voices(hw);
        self.act_end = hw.bus.clock;
        self.late_h = match self.lat_h {
            LAT_LOC_H => LATE_LOC_H,
            LAT_VOL_H | LAT_MUTED_H => LATE_VOL_H,
            _ => LATE_START_H,
        };
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
