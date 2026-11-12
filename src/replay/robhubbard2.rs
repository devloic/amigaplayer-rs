//! Rob Hubbard 2 (`rh2.*` with the sample bank `rh.ins` beside it; Stir Crazy featuring
//! Bobo, Captain Blood), a family separate from `rh.*` (`robhubbard.rs`) and from Rob Hubbard ST.
//!
//! Clean-room, written from the behaviour specification `docs/specs/robhubbard2/SPEC.md` and
//! black-box runs; no original code, and no byte of any player. The specification was drawn up
//! from observation only: the Paula register writes of the reference (the DeliTracker DeliPlayer
//! `RobHubbard_2` playing through DeliTracker's 4-voice NotePlayer genie on this project's 68000
//! engine), which bytes of the module and of the bank it reads and when, and how the output
//! changes when module bytes are changed (built modules). Nothing of the player or of the genie
//! was disassembled or read.
//!
//! A module is plain data: a header of offsets, four track lists (pattern numbers), the patterns
//! (byte streams of commands and notes), and two tables of 32 envelope records (volume, period).
//! The samples are in the bank file. Four voices, one Paula channel each, a tick 99.84 times a
//! second. The values written to Paula are the specification's: the period table and the length
//! table are the values the reference was seen to play (SPEC sections 6 and 4), written out there.
//!
//! What is the reference's and not the format's (SPEC section 9): the reference is a NotePlayer
//! player, so a period reaches Paula as `3546895 / round(3546895 / period)` (never below 122), and
//! the genie plays a sample from its own buffers, 96 bytes at a time, prepared one buffer ahead.
//! This port writes the period the same way and plays the samples from the bank in guest memory
//! with Paula's own pointer and length registers, which gives the same bytes at the same rate for
//! every sample longer than two of the genie's buffers; a shorter one (one in the Stir Crazy bank)
//! is played from buffers as the genie does (the sample, then silence until the loop is set, the
//! loop from the next buffer on), with Paula's audio interrupt. A loop of one word plays silence,
//! as in the reference.
//!
//! The write times are a model: the order of a tick's writes and the gaps between them are the
//! reference's typical ones; the time from the tick's start to its first write is a weighted sum of
//! what the tick did, the weights fitted to the reference's times (SPEC section 10). What the genie's
//! own audio interrupts and the host's other interrupts add to the reference's times is not modelled.
//!
//! Choices where the specification is silent (none can panic or hang):
//! * a length code 8C to 8F, a command byte E2 to FE, a note past the period table (index above
//!   101) are not played by any module; the original runs into its own memory there. Here the
//!   length codes play 8, 1, 1 and 1 steps, a command E2 to FE ends the voice, and a note past the
//!   table has the period 0;
//! * a sample number past the bank's table reads the table's place in the bank file as the
//!   original does (sample data), and what it describes is played if it lies within the bank;
//!   otherwise the voice plays silence;
//! * an offset outside the file reads zeros; a voice reads at most 4096 bytes in one tick, and
//!   one that reaches the bound ends;
//! * the song ends when all four track lists have ended (the reference goes on writing period 0
//!   and volume 0 for ever and never reports an end).

use super::{Hw, Open, Replayer, be16, be32};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Rob Hubbard 2", detect, new };

/// Colour clocks a second (PAL), the constant of the reference's period conversion.
const CLOCK: u32 = 3_546_895;
/// Colour clocks between two ticks (the timer value 7104 the reference runs on).
const TICK: u64 = 35525;
/// The genie's buffer, in bytes.
const BUF: usize = 96;

/// The periods of the notes 0 to 101 (SPEC section 6), as played by the reference.
const PERIODS: [u16; 102] = [
    27340, 25805, 24357, 22990, 21699, 20483, 19334, 18247, 17222, 16267, 15347, 14482, 13672, 12905, 12179, 11498,
    10854, 10241, 9574, 9125, 8623, 8132, 7678, 7246, 6844, 6454, 6092, 5750, 5427, 5121, 4836, 4565, 4308, 4067, 3838,
    3622, 3419, 3227, 3045, 2875, 2715, 2562, 2418, 2281, 2153, 2033, 1919, 1811, 1709, 1613, 1522, 1437, 1357, 1280,
    1208, 1141, 1077, 1016, 959, 906, 854, 806, 761, 719, 678, 640, 604, 570, 538, 508, 480, 453, 427, 403, 381, 359,
    339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 112, 96, 80, 64, 48,
    32, 16,
];

/// The note lengths in steps for the codes 80 to 8B (SPEC section 4); the last four are this
/// port's choice (no module has them).
const STEPS: [u8; 16] = [2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 8, 1, 1, 1];

/// The layout of a module: file offsets.
#[derive(Clone, Debug)]
struct Lay {
    speed: u8,
    vol_tab: usize,
    per_tab: usize,
    tracks: [usize; 4],
    patterns: Vec<usize>,
}

/// The module's layout, if the file is one (SPEC section 2): the first word 2, the offsets in
/// order, every track list and pattern closed by FF, the two tables of 416 bytes at the end.
fn layout(d: &[u8]) -> Option<Lay> {
    if d.len() < 18 + 2 + 4 + 1 + 832 || be16(d, 0) != 2 {
        return None;
    }
    let speed = be16(d, 2);
    if speed == 0 || speed > 255 {
        return None;
    }
    let off = |o: usize| be16(d, o) as usize + 2;
    let (a, b, end) = (off(4), off(6), off(16));
    if end != d.len() || b + 416 != end || a + 416 != b {
        return None;
    }
    let tracks = [off(8), off(10), off(12), off(14)];
    let first = tracks[0];
    if first < 20 || (first - 18) % 2 != 0 {
        return None;
    }
    let patterns: Vec<usize> = (18..first).step_by(2).map(off).collect();
    // the track lists follow the pattern table, the patterns follow them, the tables the patterns
    let mut bounds = tracks.to_vec();
    bounds.extend_from_slice(&patterns);
    bounds.push(a);
    if bounds.windows(2).any(|w| w[0] >= w[1]) {
        return None;
    }
    // every list and every pattern ends with FF
    if bounds[1..].iter().any(|&e| d[e - 1] != 0xFF) {
        return None;
    }
    // a track list holds pattern numbers
    for (i, &t) in tracks.iter().enumerate() {
        if d[t..bounds[i + 1] - 1].iter().any(|&p| p as usize >= patterns.len()) {
            return None;
        }
    }
    Some(Lay { speed: speed as u8, vol_tab: a, per_tab: b, tracks, patterns })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// The bank (SPEC section 3): its length less 4, then one 16-byte entry a sample.
fn bank_ok(b: &[u8]) -> bool {
    if b.len() < 4 + 16 + 2 || be32(b, 0) as usize + 4 > b.len() {
        return false;
    }
    let first = be32(b, 4) as usize;
    first >= 16 && first % 16 == 0 && first + 4 < b.len()
}

/// The names the bank is looked for under, in order: the one the DeliPlayer asks for (`rh.ins`,
/// the name the archive has), then the Eagleplayer's (`rh2.ins`, the module's name with `.ins` for
/// its extension, the same less its last letter; SPEC section 1).
fn bank_names(path: &str) -> Vec<String> {
    let file = path.rsplit(['/', ':']).next().unwrap_or(path);
    let mut v = vec!["rh.ins".to_string(), "rh2.ins".to_string()];
    if let Some(dot) = file.rfind('.').filter(|&p| p > 0 && file.is_char_boundary(p)) {
        let stem = &file[..dot];
        v.push(format!("{stem}.ins"));
        if let Some((i, _)) = stem.char_indices().last() {
            v.push(format!("{}.ins", &stem[..i]));
        }
    }
    v
}

/// One of the two envelopes of a voice (SPEC section 5).
#[derive(Clone, Copy, Default)]
struct Env {
    /// The record's file offset.
    rec: usize,
    /// The step (0 to 3), or 4 when the envelope has ended.
    step: u8,
    ticks: u8,
    count: u8,
    value: i16,
}

impl Env {
    fn restart(&mut self) {
        self.step = 0;
        self.ticks = 0;
        self.count = 0;
        self.value = 0;
    }

    /// One tick. `always_loops`: the period envelope goes back to its second step after the
    /// fourth, the volume envelope only with bit 7 of the record's first byte.
    fn tick(&mut self, d: &[u8], always_loops: bool) -> EnvTick {
        if self.step > 3 {
            return EnvTick { add: false, what: EnvStep::Ended };
        }
        let rb = |o: usize| d.get(o).copied().unwrap_or(0);
        let s = self.rec + 1 + 3 * self.step as usize;
        let add = self.ticks == 0;
        if add {
            self.value = self.value.wrapping_add(rb(s + 1) as i8 as i16);
        }
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks != rb(s + 2) {
            return EnvTick { add, what: EnvStep::Same };
        }
        self.ticks = 0;
        self.count = self.count.wrapping_add(1);
        if self.count != rb(s) {
            return EnvTick { add, what: EnvStep::Repeat };
        }
        self.count = 0;
        self.step += 1;
        let mut what = EnvStep::Next;
        if self.step > 3 && (always_loops || rb(self.rec) & 0x80 != 0) {
            self.step = 1;
            what = EnvStep::Loop;
        }
        EnvTick { add, what }
    }
}

/// What an envelope did in a tick.
#[derive(Clone, Copy, PartialEq)]
enum EnvStep {
    /// It had ended before.
    Ended,
    /// The step's tick count is not reached.
    Same,
    /// The step's tick count is reached and the step repeats.
    Repeat,
    /// The step's repeats are done: the next step (or the end).
    Next,
    /// After the fourth step, back to the second.
    Loop,
}

#[derive(Clone, Copy)]
struct EnvTick {
    /// The step's value was added.
    add: bool,
    what: EnvStep,
}

/// What a tick did, counted for its time: the colour clocks from the tick's start to its first
/// write are a weighted sum of these (SPEC section 10; weights fitted to the reference's times).
#[derive(Clone, Copy, Default)]
struct Work {
    step: u32,
    reads: u32,
    track: u32,
    pattern_end: u32,
    note: u32,
    length: u32,
    sample: u32,
    vol_select: u32,
    transpose: u32,
    per_select: u32,
    other: u32,
    starts: u32,
    live: u32,
    ended: u32,
    vol_add: u32,
    vol_repeat: u32,
    vol_next: u32,
    vol_ended: u32,
    per_add: u32,
    per_repeat: u32,
    per_next: u32,
    per_loop: u32,
    loops: u32,
}

impl Work {
    fn envelopes(&mut self, vol: EnvTick, per: EnvTick) {
        self.live += 1;
        self.vol_add += vol.add as u32;
        self.per_add += per.add as u32;
        match vol.what {
            EnvStep::Ended => self.vol_ended += 1,
            EnvStep::Same => {}
            EnvStep::Repeat => self.vol_repeat += 1,
            EnvStep::Next | EnvStep::Loop => self.vol_next += 1,
        }
        match per.what {
            EnvStep::Ended | EnvStep::Same => {}
            EnvStep::Repeat => self.per_repeat += 1,
            EnvStep::Next => self.per_next += 1,
            EnvStep::Loop => self.per_loop += 1,
        }
    }

    /// Colour clocks from the tick's start to its first write.
    fn clocks(&self) -> u64 {
        let w = [
            (1, 280),
            (self.step, 50),
            (self.reads, 87),
            (self.track, 65),
            (self.pattern_end, 41),
            (self.note, 151),
            (self.length, 100),
            (self.sample, 83),
            (self.vol_select, 208),
            (self.transpose, 129),
            (self.per_select, 258),
            (self.other, 100),
            (self.starts, 83),
            (self.live, 753),
            (self.ended, 365),
            (self.vol_add, 19),
            (self.vol_repeat, 45),
            (self.vol_next, 76),
            (self.per_add, 17),
            (self.per_repeat, 42),
            (self.per_next, 73),
            (self.per_loop, 110),
            (self.loops, 104),
        ];
        let sum: i64 = w.iter().map(|&(n, c)| n as i64 * c).sum::<i64>() - 25 * self.vol_ended as i64;
        sum.max(0) as u64
    }
}

/// How a voice's sample reaches Paula.
#[derive(Clone, Copy, PartialEq)]
enum Feed {
    /// Paula's pointer and length registers on the bank.
    Direct,
    /// From the voice's two buffers, as the genie: `at` bytes of the sample are in them.
    Buffers { at: usize, next: usize },
}

#[derive(Clone)]
struct Voice {
    track: usize,
    pos: usize,
    in_pattern: bool,
    ended: bool,
    /// Steps left of the note.
    left: u8,
    steps: u8,
    transpose: u8,
    sample: u8,
    /// A sample command came since the last note: the next note starts the sample.
    armed: bool,
    period: u16,
    vol: Env,
    per: Env,
    /// Ticks until the sample's loop is set (0: none pending).
    loop_in: u8,
    /// The loop of the sample that plays (address, words), once set.
    looped: Option<(u32, u16)>,
    /// The sample started (bank offset, bytes).
    cur: (usize, usize),
    feed: Feed,
}

pub struct RobHubbard2 {
    d: Vec<u8>,
    lay: Lay,
    bank: Vec<u8>,
    bank_addr: u32,
    /// 192 zero bytes, then two buffers of 96 bytes a voice.
    scratch: u32,
    v: [Voice; 4],
    speed_count: u8,
    /// The audio interrupts this player has enabled (INTENA bits).
    irq: u16,
    first: bool,
    /// What the current tick did, for its time (SPEC section 10).
    work: Work,
    songend: bool,
}

/// The period as the reference writes it (SPEC section 9.1).
fn written_period(p: i32) -> u16 {
    if p <= 0 {
        return 0;
    }
    let p = p as u32;
    let f = ((2 * CLOCK + p) / (2 * p)).max(1);
    (CLOCK / f).clamp(122, 0xFFFF) as u16
}

impl RobHubbard2 {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// Reads on from the voice's place to its next note (SPEC section 4). Returns whether the
    /// note starts its sample.
    fn next_note(&mut self, k: usize) -> bool {
        for _ in 0..4096 {
            let v = &self.v[k];
            if !v.in_pattern {
                let p = self.rb(v.track);
                self.work.track += 1;
                let v = &mut self.v[k];
                if p == 0xFF {
                    v.ended = true;
                    return false;
                }
                v.track += 1;
                match self.lay.patterns.get(p as usize) {
                    Some(&o) => {
                        v.pos = o;
                        v.in_pattern = true;
                    }
                    None => {
                        v.ended = true;
                        return false;
                    }
                }
                continue;
            }
            let c = self.rb(v.pos);
            let arg = self.rb(v.pos + 1);
            let (vol_tab, per_tab) = (self.lay.vol_tab, self.lay.per_tab);
            let w = &mut self.work;
            *match c {
                0xFF => &mut w.pattern_end,
                0xE0 => &mut w.transpose,
                0xE1 => &mut w.per_select,
                0xE2..=0xFE => &mut w.other,
                0xC0..=0xDF => &mut w.vol_select,
                0xA0..=0xBF => &mut w.sample,
                0x80..=0x9F => &mut w.length,
                _ => &mut w.note,
            } += 1;
            let v = &mut self.v[k];
            v.pos += 1;
            match c {
                0xFF => v.in_pattern = false,
                0xE0 => {
                    v.transpose = arg;
                    v.pos += 1;
                }
                0xE1 => {
                    v.per.rec = per_tab + 13 * arg as usize;
                    v.pos += 1;
                }
                0xE2..=0xFE => {
                    v.ended = true;
                    return false;
                }
                0xC0..=0xDF => v.vol.rec = vol_tab + 13 * (c & 0x1F) as usize,
                0xA0..=0xBF => {
                    v.sample = c & 0x1F;
                    v.armed = true;
                }
                0x80..=0x9F => v.steps = STEPS[(c & 0x0F) as usize],
                _ => {
                    let i = if c == 0 { 0 } else { c.wrapping_add(v.transpose) as usize };
                    v.period = PERIODS.get(i).copied().unwrap_or(0);
                    v.left = v.steps;
                    v.vol.restart();
                    v.per.restart();
                    return std::mem::take(&mut v.armed);
                }
            }
        }
        self.v[k].ended = true;
        false
    }

    /// The bank entry's block: `field` 0 the sample (start, words), 4 its loop. `None` when it
    /// does not lie within the bank or is empty.
    fn block(&self, sample: u8, field: usize) -> Option<(usize, u16)> {
        let e = 4 + 16 * sample as usize;
        let start = (be32(&self.bank, e + field) as usize).checked_add(4)?;
        let words = be16(&self.bank, e + 12 + field / 2);
        (words != 0 && start + 2 * words as usize <= self.bank.len()).then_some((start, words))
    }

    fn silence(&self) -> u32 {
        self.scratch
    }

    fn buffer(&self, k: usize, n: usize) -> u32 {
        self.scratch + (2 * BUF + (2 * k + n) * BUF) as u32
    }

    /// Fills buffer `n` of voice `k` with the next 96 bytes of its sample, zeros after its end.
    fn fill(&mut self, k: usize, n: usize, hw: &mut Hw) {
        let Feed::Buffers { at, .. } = self.v[k].feed else { return };
        let (start, len) = self.v[k].cur;
        let mut b = [0u8; BUF];
        let have = len.saturating_sub(at).min(BUF);
        if let Some(s) = self.bank.get(start + at..start + at + have) {
            b[..have].copy_from_slice(s);
        }
        hw.bus.write_bytes(self.buffer(k, n), &b);
        self.v[k].feed = Feed::Buffers { at: at + have, next: n ^ 1 };
    }

    /// The writes that start voice `k`'s sample (between the DMA off and on of the tick).
    fn start_sample(&mut self, k: usize, hw: &mut Hw) {
        let sample = self.v[k].sample;
        self.v[k].looped = None;
        self.v[k].loop_in = 2;
        match self.block(sample, 0) {
            Some((start, words)) if 2 * words as usize > 2 * BUF => {
                self.v[k].feed = Feed::Direct;
                self.v[k].cur = (start, 2 * words as usize);
                hw.loc(k, self.bank_addr + start as u32);
                hw.len(k, words);
            }
            Some((start, words)) => {
                // a short one: from the buffers, the first filled now
                self.v[k].cur = (start, 2 * words as usize);
                self.v[k].feed = Feed::Buffers { at: 0, next: 0 };
                self.fill(k, 0, hw);
                hw.loc(k, self.buffer(k, 0));
                hw.len(k, (BUF / 2) as u16);
            }
            None => {
                self.v[k].feed = Feed::Direct;
                self.v[k].cur = (0, 0);
                hw.loc(k, self.silence());
                hw.len(k, 1);
            }
        }
    }

    /// The loop of voice `k`'s sample, two ticks after its start.
    fn set_loop(&mut self, k: usize, hw: &mut Hw) {
        let l = match self.block(self.v[k].sample, 4) {
            // a loop of one word plays silence (SPEC section 3)
            Some((start, words)) if words > 1 && self.v[k].cur.1 != 0 => (self.bank_addr + start as u32, words),
            _ => (self.silence(), 1),
        };
        self.v[k].looped = Some(l);
        if self.v[k].feed == Feed::Direct {
            hw.loc(k, l.0);
            hw.len(k, l.1);
        }
    }
}

impl Replayer for RobHubbard2 {
    fn format(&self) -> &'static str {
        "Rob Hubbard 2"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn timer_delay(&self) -> u64 {
        START_DELAY
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        // the music (SPEC section 7)
        let mut step = false;
        self.speed_count = self.speed_count.wrapping_sub(1);
        if self.speed_count == 0 {
            self.speed_count = self.lay.speed;
            step = true;
        }
        let first = std::mem::take(&mut self.first);
        self.work = Work { step: step as u32, ..Work::default() };
        let mut starts = 0u16;
        let mut loops = 0u16;
        for k in 0..4 {
            if !self.v[k].ended {
                let mut read = first;
                if !first && step {
                    self.v[k].left = self.v[k].left.wrapping_sub(1);
                    read = self.v[k].left == 0;
                }
                if read {
                    self.work.reads += 1;
                    if self.next_note(k) {
                        starts |= 1 << k;
                    }
                }
            }
            if !self.v[k].ended {
                let d = std::mem::take(&mut self.d);
                let vol = self.v[k].vol.tick(&d, false);
                let per = self.v[k].per.tick(&d, true);
                self.d = d;
                self.work.envelopes(vol, per);
            } else {
                self.work.ended += 1;
            }
            if self.v[k].loop_in != 0 {
                self.v[k].loop_in -= 1;
                if self.v[k].loop_in == 0 && starts & 1 << k == 0 {
                    self.work.loops += 1;
                    loops |= 1 << k;
                }
            }
        }
        let out: [(u16, u16); 4] = std::array::from_fn(|k| {
            let v = &self.v[k];
            if v.ended {
                (0, 0)
            } else {
                (written_period(v.period as i32 + v.per.value as i32), v.vol.value.max(0) as u16)
            }
        });

        self.work.starts = starts.count_ones();
        // the first write of the tick at the time the cost model gives (SPEC section 10)
        let first_gap = if starts != 0 {
            216
        } else if out[0].0 == 0 {
            189
        } else {
            187
        };
        let cost = self.work.clocks().saturating_sub(first_gap);
        // the writes, in the order and at the typical gaps of the reference's (SPEC section 8)
        hw.wait(cost);
        if starts != 0 {
            hw.wait(first_gap);
            for k in (0..4).filter(|k| starts & 1 << k != 0) {
                hw.per(k, 1);
                hw.wait(18);
            }
            hw.wait(14 + 9 * (4 - starts.count_ones() as u64));
            // a voice that played from the buffers has its interrupt off again
            let off = self.irq & starts << 7;
            if off != 0 {
                hw.bus.write_word(0xDFF09A, off);
                self.irq &= !off;
            }
            hw.dmacon(starts);
        }
        // 0: the tick's first voice, 1: after a voice that starts its sample, 2: after another
        let mut prev = 0;
        for k in 0..4 {
            let (per, vol) = out[k];
            let plain = if per == 0 { 189 } else { 187 };
            if starts & 1 << k != 0 {
                hw.wait([300, 594, 230][prev]);
                hw.per(k, per);
                hw.wait(141);
                hw.vol(k, vol);
                hw.wait(58);
                self.start_sample(k, hw);
                hw.wait(7);
                hw.per(k, per);
                prev = 1;
            } else {
                hw.wait(if starts == 0 { plain } else { [300, 556, plain][prev] });
                hw.per(k, per);
                hw.wait(141);
                hw.vol(k, vol);
                prev = 2;
            }
        }
        if starts != 0 {
            hw.wait(if prev == 1 { 668 } else { 333 });
            // the buffers' interrupts on before the DMA starts
            let buffered: u16 =
                (0..4).filter(|&k| starts & 1 << k != 0 && self.v[k].feed != Feed::Direct).map(|k| 0x80 << k).sum();
            if buffered != 0 {
                hw.bus.write_word(0xDFF09C, buffered);
                hw.bus.write_word(0xDFF09A, 0xC000 | buffered);
                self.irq |= buffered;
            }
            hw.dmacon(0x8000 | starts);
        }
        for k in (0..4).filter(|k| loops & 1 << k != 0) {
            self.set_loop(k, hw);
        }
        if self.v.iter().all(|v| v.ended) {
            self.songend = true;
        }
    }

    /// A buffer of a short sample started: the next one (SPEC section 9.2).
    fn audio_irq_ch(&mut self, k: usize, hw: &mut Hw) -> bool {
        let bit = 0x80u16 << k;
        let Feed::Buffers { at, next } = self.v[k].feed else {
            hw.bus.write_word(0xDFF09A, bit);
            hw.bus.write_word(0xDFF09C, bit);
            self.irq &= !bit;
            return true;
        };
        hw.wait(125);
        if at < self.v[k].cur.1 {
            // more of the sample
            self.fill(k, next, hw);
            hw.loc(k, self.buffer(k, next));
            hw.len(k, (BUF / 2) as u16);
        } else if let Some((a, words)) = self.v[k].looped {
            // the loop from the next buffer on; no further interrupt
            hw.loc(k, a);
            hw.len(k, words);
            self.v[k].feed = Feed::Direct;
            hw.bus.write_word(0xDFF09A, bit);
            self.irq &= !bit;
        } else {
            hw.loc(k, self.silence());
            hw.len(k, (BUF / 2) as u16);
        }
        hw.wait(480);
        hw.bus.write_word(0xDFF09C, bit);
        true
    }
}

/// Colour clocks from the song's start to the timer's start (the reference's start-up).
const START_DELAY: u64 = 13200;

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    layout(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let bank =
        bank_names(&file).iter().find_map(|n| crate::deli::read_companion(m, n).1.ok().filter(|b| bank_ok(b)))?;
    let bank_addr = m.alloc(bank.len() as u32 + 2);
    let scratch = m.alloc((2 * BUF + 8 * BUF) as u32);
    if bank_addr == 0 || scratch == 0 {
        return None;
    }
    m.bus.write_bytes(bank_addr, &bank);
    Some(Box::new(RobHubbard2::build(d, bank, bank_addr, scratch)?))
}

impl RobHubbard2 {
    /// The player for module `d` with its bank, which is in guest memory at `bank_addr`; `scratch`
    /// is 960 zero bytes (the silent area and the voices' buffers).
    fn build(d: &[u8], bank: Vec<u8>, bank_addr: u32, scratch: u32) -> Option<Self> {
        let lay = layout(d)?;
        let voice = |k: usize| Voice {
            track: lay.tracks[k],
            pos: 0,
            in_pattern: false,
            ended: false,
            left: 0,
            steps: STEPS[0],
            transpose: 0,
            sample: 0,
            armed: false,
            period: 0,
            vol: Env { rec: lay.vol_tab, ..Env::default() },
            per: Env { rec: lay.per_tab, ..Env::default() },
            loop_in: 0,
            looped: None,
            cur: (0, 0),
            feed: Feed::Direct,
        };
        let v = [voice(0), voice(1), voice(2), voice(3)];
        Some(RobHubbard2 {
            d: d.to_vec(),
            speed_count: lay.speed,
            lay,
            bank,
            bank_addr,
            scratch,
            v,
            irq: 0,
            first: true,
            work: Work::default(),
            songend: false,
        })
    }
}

#[cfg(test)]
#[path = "robhubbard2_tests.rs"]
mod tests;
