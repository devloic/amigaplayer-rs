//! Mark Cooksey (`mc.*`, and the `mcr.*` + `mcs.*` pair), the music driver of Mark Cooksey's
//! and Richard Frankish's 1988-1990 game soundtracks (Commando, Paperboy, Ghosts 'n Goblins,
//! Grand National, Gremlins 2, Mighty Bomb Jack, World Championship Soccer, Aquablast,
//! Overlander, A Question of Sport, ...).
//!
//! Clean-room, written from a behavioural specification (drawn up from the Paula register
//! writes and beam-read traces of UADE's Mark Cooksey player and from the structure of the
//! corpus files) and checked against those writes. No original code is used or run.
//!
//! Three engines. N-A and N-B ("new", 1989-1990) are one file `mc.NAME` that carries its own
//! driver, tables, song data and samples: the tables are found from the position-independent
//! address loads of the play routine (the specification's table-location rule) and read as
//! data. O ("old", 1988) is the pair `mcr.NAME` (tables) and `mcs.NAME` (song data and
//! samples), the second found beside the first.
//!
//! Timing. The original waits for the raster beam and for audio interrupts between the
//! writes of a note start (N: three beam waits and two interrupt-flag waits, O: two waits
//! of nine raster lines). The waits are modelled as the specification describes them
//! (beam reads at fixed intervals, polls of Paula's interrupt flag), the code between
//! them as colour-clock costs per path, fitted to the reference's write logs.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Mark Cooksey", detect, new };

const LINE2: u64 = 454; // a raster line in half colour clocks

// ------------------------------------------------------------------ recognition

fn detect_n(d: &[u8]) -> Option<bool> {
    if d.len() < 160 || be32(d, 0) != 0xD040_D040 || be16(d, 4) != 0x4EFB {
        return None;
    }
    if [8, 12, 16, 20].iter().any(|&o| be16(d, o) != 0x6000) {
        return None;
    }
    if be16(d, 40) == 0x43FA {
        return Some(false);
    }
    (be16(d, 24) == 0x6000 && be16(d, 150) == 0x43FA).then_some(true)
}

fn detect_o(d: &[u8]) -> bool {
    if d.len() < 64 || be16(d, 0) != 0x601A {
        return false;
    }
    let l2 = be32(d, 2);
    if l2 >= 0x8000_0000 || l2 & 1 != 0 || be16(d, 6) != 0 {
        return false;
    }
    if (8..28).step_by(4).any(|o| be32(d, o) != 0) {
        return false;
    }
    let mut first = 0;
    for (i, o) in [28usize, 32, 36, 40].iter().enumerate() {
        if be16(d, *o) != 0x6000 {
            return false;
        }
        let disp = be16(d, o + 2);
        if disp & 0x8001 != 0 {
            return false;
        }
        if i == 0 {
            first = o + 2 + disp as usize;
        }
    }
    d.get(first..first + 4).is_some_and(|b| b == [0x48, 0xE7, 0x80, 0xF0])
}

fn detect(d: &[u8]) -> bool {
    detect_n(d).is_some() || detect_o(d)
}

// ------------------------------------------------------------------ engine N: layout

#[derive(Clone, Copy)]
struct NLay {
    b: bool,
    sample: usize,
    pat: usize,
    inst: usize,
    per: usize,
    sub: usize,
    nsub: usize,
}

/// The target of the position-independent address load (`lea d16(pc),An`) whose opcode word is at `o`.
fn lea_target(d: &[u8], o: usize) -> Option<usize> {
    let w = be16(d, o);
    if !matches!(w, 0x41FA | 0x43FA | 0x45FA | 0x47FA) {
        return None;
    }
    let disp = be16(d, o + 2) as i16 as isize;
    let t = o as isize + 2 + disp;
    (t >= 0 && (t as usize) < d.len()).then_some(t as usize)
}

fn locate_n(d: &[u8], b: bool) -> Option<NLay> {
    // the play routine is behind the first branch slot (entry 0)
    let slot: isize = if b { 12 } else { 8 };
    let play = slot + 2 + be16(d, slot as usize + 2) as i16 as isize;
    if play < 0 || play as usize >= d.len() {
        return None;
    }
    let mut occ = Vec::new();
    let mut o = play as usize & !1;
    while o + 4 <= d.len() && occ.len() < 6 {
        if let Some(t) = lea_target(d, o) {
            occ.push(t);
        }
        o += 2;
    }
    if occ.len() < 6 {
        return None;
    }
    let (state, sample, pat, inst, per) = if b {
        (occ[0], occ[3], occ[1], occ[2], occ[5])
    } else {
        (occ[0], occ[4], occ[2], occ[3], occ[5])
    };
    if !b && occ[1] != sample {
        return None;
    }
    // layout checks of the specification
    let follow = if b { 768 + 256 } else { 768 };
    if per + follow != state || pat >= per || sample + 64 > d.len() || d[sample..sample + 64].iter().any(|&x| x != 0) {
        return None;
    }
    // the subsong table: the first `lea` (opcode 43FA) from offset 2; the next one's table follows it
    let mut it = (2..d.len().saturating_sub(4)).step_by(2).filter(|&o| be16(d, o) == 0x43FA).filter_map(|o| lea_target(d, o));
    let sub = it.next()?;
    let next = it.next()?;
    if next <= sub || (next - sub) % 16 != 0 {
        return None;
    }
    let nsub = (next - sub) / 16;
    if nsub == 0 || nsub > 200 {
        return None;
    }
    Some(NLay { b, sample, pat, inst, per, sub, nsub })
}

// ------------------------------------------------------------------ shared timing helpers

/// Indexes of the cost items (kind * 4 + voice), in half colour clocks.
const NK: usize = 40;

/// Every write with its time and the cost items passed since the previous one (the fitting test).
#[cfg(test)]
static TRACE: std::sync::Mutex<Vec<(u32, u16, u64, bool, [u32; NK * 4])>> = std::sync::Mutex::new(Vec::new());

struct Clock {
    /// Time in half colour clocks.
    t2: u64,
    /// Item counts since the last write (for the fitting tests).
    counts: [u32; NK * 4],
    fixed: bool,
    /// No write has been made yet in this tick (its own cost table applies).
    first: bool,
}

impl Clock {
    fn new() -> Clock {
        Clock {
            t2: 0,
            counts: [0; NK * 4],
            fixed: false,
            first: true,
        }
    }
    fn start(&mut self, hw: &Hw) {
        self.first = true;
        self.t2 = self.t2.max(hw.bus.clock * 2);
    }
    /// Adds `n` times the cost of item `kind` for voice `v`.
    fn item(&mut self, costs: &[i32], kind: usize, v: usize, n: u32) {
        let i = kind * 4 + v;
        self.counts[i] += n;
        let c = costs.get(i + if self.first { NK * 4 } else { 0 }).copied().unwrap_or(0);
        self.t2 += (c as i64 * n as i64).max(0) as u64;
    }
    /// Lets `cc` colour clocks (`half` additional half) pass as a fixed gap of the specification.
    fn adv(&mut self, half_cc: u64) {
        self.t2 += half_cc;
        self.fixed = true;
    }
    fn sync(&mut self, hw: &mut Hw) {
        let c = self.t2 / 2;
        if c > hw.bus.clock {
            hw.bus.clock = c;
        }
    }
    #[allow(unused_variables)]
    fn mark(&mut self, reg: u32, val: u16) {
        #[cfg(test)]
        {
            TRACE.lock().unwrap().push((reg, val, self.t2, self.fixed, self.counts));
        }
        self.counts = [0; NK * 4];
        self.fixed = false;
        self.first = false;
    }
    fn vol(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.sync(hw);
        hw.vol(ch, v);
        self.mark(0xA8 + 16 * ch as u32, v);
    }
    fn per(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.sync(hw);
        hw.per(ch, v);
        self.mark(0xA6 + 16 * ch as u32, v);
    }
    fn len(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.sync(hw);
        hw.len(ch, v);
        self.mark(0xA4 + 16 * ch as u32, v);
    }
    fn loc(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.sync(hw);
        hw.loc(ch, a);
        self.mark(0xA0 + 16 * ch as u32, 0);
    }
    fn dmacon(&mut self, hw: &mut Hw, v: u16) {
        self.sync(hw);
        hw.dmacon(v);
        self.mark(0x96, v);
    }
    fn intreq(&mut self, hw: &mut Hw, v: u16) {
        self.sync(hw);
        hw.bus.write_word(0xDFF09C, v);
        self.mark(0x9C, v);
    }
    /// Whether the channel's audio DMA is on (DMACONR) and its interrupt flag (INTREQR) at the
    /// current time.
    fn dma_flag(&mut self, hw: &mut Hw, ch: usize) -> (bool, bool) {
        self.sync(hw);
        let t = hw.bus.clock;
        hw.bus.paula.sync(t, &hw.bus.ram);
        (hw.bus.paula.dmacon & (1 << ch) != 0, hw.bus.paula.intreq & (0x80 << ch) != 0)
    }
    /// The N engine's raster wait: waits for the start of a line, then for the beam to be past
    /// colour clock 22. `t2` is the first beam read; on return it is the last one.
    fn wait_w(&mut self) {
        let t0 = self.t2;
        let line0 = t0 / LINE2;
        let mut t = t0 + 12;
        while t / LINE2 == line0 {
            t += 21;
        }
        t += 24;
        while (t % LINE2) / 2 < 22 {
            t += 25;
        }
        self.t2 = t;
        self.fixed = true;
    }
    /// The O engine's wait of nine raster lines; `t2` is the first beam read, on return the
    /// last (the ninth exit read).
    fn wait_d(&mut self) {
        let mut t = self.t2;
        for it in 0..9 {
            let line0 = t / LINE2;
            t += 16;
            while t / LINE2 == line0 {
                t += 21;
            }
            if it < 8 {
                t += 36;
            }
        }
        self.t2 = t;
        self.fixed = true;
    }
}

// ------------------------------------------------------------------ engine N: player

/// Item kinds of engine N (see `COST_N`).
mod k {
    pub const PRE: usize = 0;
    pub const SFX: usize = 1;
    pub const VOICE: usize = 2;
    pub const NOTRACK: usize = 3;
    pub const EVENT: usize = 4;
    pub const CMD_TVOL: usize = 5;
    pub const CMD_CALL: usize = 6;
    pub const CMD_CALLR: usize = 7;
    pub const CMD_END: usize = 8;
    pub const CMD_JUMP: usize = 9;
    pub const CMD_PORT: usize = 10;
    pub const CMD_PORTOFF: usize = 11;
    pub const NOTE: usize = 12;
    pub const REST: usize = 13;
    pub const STOP: usize = 14;
    pub const NOTE_TAIL: usize = 15;
    pub const ENV_NONE: usize = 16;
    pub const ENV_WAIT: usize = 17;
    pub const ENV_STEP: usize = 18;
    pub const ENV_END: usize = 19;
    pub const ENV_TAIL: usize = 20;
    pub const SLIDE_NONE: usize = 21;
    pub const SLIDE_ACT: usize = 22;
    pub const PIT_NONE: usize = 23;
    pub const PIT_WAIT: usize = 24;
    pub const PIT_STEP: usize = 25;
    pub const PIT_END: usize = 26;
    pub const PLAIN_NO: usize = 27;
    pub const PLAIN_WR: usize = 28;
    pub const VEND: usize = 29;
    pub const PIT_TAIL: usize = 30;
}

const fn mk(e: &[(usize, i32)]) -> [i32; NK * 8] {
    let mut t = [0; NK * 8];
    let mut i = 0;
    while i < e.len() {
        t[e[i].0] = e[i].1;
        i += 1;
    }
    t
}

/// Where in the raster line the tick's code starts, in half colour clocks: the reference's timer
/// interrupt is not at a line start, and the waits of a note start depend on the line phase.
const PHASE_N_A: u64 = 308;
const PHASE_N_B: u64 = 266;
const PHASE_O: u64 = 44;

/// Costs of engine N's code paths in half colour clocks, indexed `kind * 4 + voice` (fitted to the
/// reference's write logs), variant A, then B.
static COST_N_A: [i32; NK * 8] = mk(&[
    (9, 194), (18, 449), (20, 293), (21, 305), (22, 319), (24, 417), (25, 422), (26, 406),
    (28, 406), (29, 405), (32, 355), (33, 349), (34, 315), (48, 1195), (49, 1029), (50, 794),
    (52, 453), (53, 258), (56, 655), (57, 410), (58, 211), (60, 93), (61, 300), (62, 146),
    (67, 138), (72, 584), (73, 425), (74, 617), (81, 252), (82, 104), (84, 456), (94, 160),
    (170, 19), (172, 63), (176, 2809), (177, 1984), (178, 1021), (183, 606), (184, 436),
    (187, 94), (192, 356), (195, 606), (209, 590), (211, 859), (215, 808), (219, 70),
    (220, 119), (233, 640), (234, 375), (240, 81), (241, 23), (260, 587),
]);

static COST_N_B: [i32; NK * 8] = mk(&[
    (8, 62), (9, 7), (10, 315), (17, 468), (18, 153), (20, 208), (21, 205), (22, 212),
    (24, 329), (25, 328), (26, 328), (29, 333), (30, 347), (33, 232), (34, 235), (48, 1181),
    (49, 790), (50, 800), (52, 409), (56, 520), (57, 135), (58, 180), (61, 15), (62, 347),
    (72, 636), (73, 710), (74, 419), (75, 802), (80, 553), (82, 333), (83, 231), (95, 101),
    (97, 28), (98, 26), (101, 584), (102, 281), (109, 301), (121, 115), (122, 144), (169, 158),
    (170, 1065), (176, 496), (177, 527), (178, 546), (183, 3124), (187, 409), (191, 608),
    (195, 244), (211, 1270), (229, 355), (230, 16), (240, 2022), (252, 1971), (253, 142),
    (280, 1972),
]);

#[derive(Clone, Default)]
struct Frame {
    ret: usize,
    start: usize,
    rep: i16,
}

#[derive(Clone, Default)]
struct NVoice {
    tp: Option<usize>,
    cnt: u16,
    frames: Vec<Frame>,
    trans: i16,
    tvol: u8,
    port: bool,
    plen: u8,
    // instrument
    attack: u32,
    loop_off: u32,
    env: Option<usize>,
    envcnt: u8,
    env_int: u8,
    pit_tab: Option<usize>,
    pitcnt: u8,
    pit_int: u8,
    ivol: u16,
    alen: u16,
    llen: u16,
    ascale: u16,
    lscale: u16,
    pitch: u16,
    target: u16,
    start: u16,
    slide: i16,
    loopmode: bool,
}

struct NPlayer {
    d: Vec<u8>,
    base: u32,
    lay: NLay,
    sub: u16,
    v: [NVoice; 4],
    flags: [bool; 4],
    songend: bool,
    clk: Clock,
    costs: &'static [i32],
}

// Timing constants of a note start with an attack sample, in colour clocks, by channel 0..3
// (the specification's section 7.2). Variant A, then B.
const VOL_TO_READ: [[u64; 4]; 2] = [[158, 174, 189, 193], [195, 210, 226, 230]];
const LC_TO_LEN: [[u64; 4]; 2] = [[60, 76, 92, 95], [61, 76, 92, 95]];
const LEN_TO_PER: [[u64; 4]; 2] = [[123, 138, 153, 158], [94, 110, 125, 130]];
const PER_TO_READ2: [[u64; 4]; 2] = [[68, 84, 100, 103], [69, 84, 100, 103]];
const LEN_TO_LC_LOOP: [[u64; 4]; 2] = [[68, 84, 100, 103], [68, 84, 100, 103]];

impl NPlayer {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }
    fn rw(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }
    fn rl(&self, o: usize) -> u32 {
        be32(&self.d, o)
    }
    fn var(&self) -> usize {
        self.lay.b as usize
    }

    fn start_sub(&mut self, n: u16) {
        self.sub = n;
        self.v = Default::default();
        let t = self.lay.sub + 16 * n as usize;
        for v in 0..4 {
            let at = t + 4 * v;
            let off = self.rl(at) as i32;
            self.v[v].tp = if off == 0 { None } else { Some((at as i64 + off as i64) as usize) };
        }
        self.flags = [true; 4];
        self.songend = false;
    }

    fn table_entry(&self, tab: usize, n: u8) -> usize {
        let at = tab + 2 * n as usize;
        (at as i64 + self.rw(at) as i16 as i64) as usize
    }

    fn period(&self, vo: &NVoice, idx: i32) -> u16 {
        let t = self.rw((self.lay.per as i64 + 2 * (idx as i16 as i64)).max(0) as usize) as u32;
        let scale = if vo.loopmode { vo.lscale } else { vo.ascale } as u32;
        (t.wrapping_mul(scale) >> 16) as u16
    }

    fn vol_of(level: u32) -> u16 {
        (level >> 1).min(64) as u16
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.clk.start(hw);
        self.clk.t2 += if self.lay.b { PHASE_N_B } else { PHASE_N_A };
        let c = self.costs;
        self.clk.item(c, k::PRE, 0, 1);
        self.clk.item(c, k::SFX, 0, 1);
        for v in (0..4).rev() {
            self.voice(hw, v);
        }
    }

    fn voice(&mut self, hw: &mut Hw, v: usize) {
        let c = self.costs;
        self.clk.item(c, k::VOICE, v, 1);
        let before = self.v[v].cnt;
        self.v[v].cnt = before.wrapping_sub(1);
        if before <= 1 {
            if self.v[v].tp.is_none() {
                self.clk.item(c, k::NOTRACK, v, 1);
                return;
            }
            self.clk.item(c, k::EVENT, v, 1);
            if !self.events(hw, v) {
                return;
            }
        }
        self.maintenance(hw, v);
        self.clk.item(c, k::VEND, v, 1);
    }

    fn song_flag(&mut self, v: usize) {
        self.flags[v] = false;
        if self.flags.iter().all(|f| !f) {
            self.songend = true;
            self.flags = [true; 4];
        }
    }

    /// Reads the track commands of an event step; false when the step ended with a rest or a stop.
    fn events(&mut self, hw: &mut Hw, v: usize) -> bool {
        let c = self.costs;
        for _ in 0..4096 {
            let Some(tp) = self.v[v].tp else { return false };
            let b = self.rb(tp);
            let cmd = b & 0x7F;
            if cmd < 0x60 {
                self.note(hw, v, tp, cmd);
                return true;
            }
            match cmd - 0x60 {
                0 => {
                    // rest
                    let dur = self.rw(tp + 1);
                    self.v[v].cnt = dur;
                    self.v[v].tp = Some(tp + 4);
                    self.clk.item(c, k::REST, v, 1);
                    self.clk.vol(hw, v, 0);
                    return false;
                }
                3 => {
                    self.v[v].tvol = self.rb(tp + 1);
                    self.v[v].tp = Some(tp + 2);
                    self.clk.item(c, k::CMD_TVOL, v, 1);
                }
                4 | 5 => {
                    let p = self.rb(tp + 1);
                    let t = self.rb(tp + 2) as i8 as i16;
                    let (rep, len) = if cmd == 0x65 { (self.rb(tp + 3) as i16, 4) } else { (0, 3) };
                    let start = self.table_entry(self.lay.pat, p);
                    let vo = &mut self.v[v];
                    vo.trans = t;
                    if vo.frames.len() < 3 {
                        vo.frames.push(Frame { ret: tp + len, start, rep });
                    } else {
                        // a deeper nesting would overwrite other fields in the original; it is not
                        // modelled: the call replaces the innermost frame
                        let n = vo.frames.len() - 1;
                        vo.frames[n] = Frame { ret: tp + len, start, rep };
                    }
                    vo.tp = Some(start);
                    self.clk.item(c, if cmd == 0x65 { k::CMD_CALLR } else { k::CMD_CALL }, v, 1);
                }
                6 => {
                    let vo = &mut self.v[v];
                    self.clk.item(c, k::CMD_END, v, 1);
                    let vo2 = &mut *vo;
                    if let Some(f) = vo2.frames.last_mut() {
                        f.rep = f.rep.wrapping_sub(1);
                        if f.rep >= 0 {
                            vo2.tp = Some(f.start);
                        } else {
                            vo2.tp = Some(f.ret);
                            vo2.frames.pop();
                            vo2.trans = 0;
                        }
                    } else {
                        vo2.tp = None;
                    }
                }
                7 => {
                    self.v[v].tp = None;
                    self.clk.item(c, k::STOP, v, 1);
                    self.song_flag(v);
                    let (ch, sample) = (v, self.base + self.lay.sample as u32);
                    self.clk.loc(hw, ch, sample);
                    self.clk.adv(16);
                    self.clk.len(hw, ch, 0x20);
                    self.clk.adv(16);
                    self.clk.vol(hw, ch, 0);
                    return false;
                }
                8 => {
                    let d = self.rw(tp + 1) as i16 as i64;
                    self.v[v].tp = Some((tp as i64 + d).max(0) as usize);
                    self.clk.item(c, k::CMD_JUMP, v, 1);
                    self.song_flag(v);
                }
                9 => {
                    self.v[v].port = true;
                    self.v[v].plen = self.rb(tp + 1);
                    self.v[v].tp = Some(tp + 2);
                    self.clk.item(c, k::CMD_PORT, v, 1);
                }
                11 => {
                    self.v[v].port = false;
                    self.v[v].tp = Some(tp + 1);
                    self.clk.item(c, k::CMD_PORTOFF, v, 1);
                }
                _ => {
                    // unknown: the original loops for ever; the voice stops here
                    self.v[v].tp = None;
                    return false;
                }
            }
        }
        self.v[v].tp = None;
        false
    }

    fn note(&mut self, hw: &mut Hw, v: usize, tp: usize, b: u8) {
        let c = self.costs;
        let i = self.rb(tp + 1);
        let dur = self.rb(tp + 2);
        self.v[v].tp = Some(tp + 4);
        let n = (b as i32 + self.v[v].trans as i32) as u16;
        let idx = n.wrapping_mul(4);
        let rec = self.table_entry(self.lay.inst, i);
        let attack = self.rl(rec);
        let loop_off = self.rl(rec + 4);
        let env = self.rl(rec + 8) as i32;
        let pit = self.rl(rec + 12) as i32;
        let (alen, llen, ascale, lscale) = (self.rw(rec + 16), self.rw(rec + 18), self.rw(rec + 20), self.rw(rec + 22));
        let (env_int, pit_int, ivol) = (self.rb(rec + 24), self.rb(rec + 25), self.rw(rec + 26));
        let vo = &mut self.v[v];
        if vo.port {
            vo.start = vo.target;
            vo.pitch = vo.target;
            vo.target = idx;
            let diff = idx as i32 - vo.start as i32;
            vo.slide = if vo.plen == 0 { 0 } else { (diff * 16 / vo.plen as i32) as i16 };
        } else {
            vo.start = idx;
            vo.pitch = idx;
            vo.target = idx;
            vo.slide = 0;
        }
        vo.cnt = dur as u16;
        vo.attack = attack;
        vo.loop_off = loop_off;
        vo.env = (env != 0).then(|| (rec as i64 + 8 + env as i64).max(0) as usize);
        vo.pit_tab = (pit != 0).then(|| (rec as i64 + 12 + pit as i64).max(0) as usize);
        vo.envcnt = 0;
        vo.pitcnt = 0;
        vo.alen = alen;
        vo.llen = llen;
        vo.ascale = ascale;
        vo.lscale = lscale;
        vo.env_int = env_int;
        vo.pit_int = pit_int;
        vo.ivol = ivol;
        vo.loopmode = attack == 0;
        let l0 = (((ivol as u32 * vo.tvol as u32) & 0xFFFF) >> 8) as u32;
        let vo = self.v[v].clone();
        let p = self.period(&vo, idx as i32);
        self.clk.item(c, k::NOTE, v, 1);
        self.note_hw(hw, v, &vo, l0, p);
        self.clk.item(c, k::NOTE_TAIL, v, 1);
    }

    fn note_hw(&mut self, hw: &mut Hw, ch: usize, vo: &NVoice, l0: u32, p: u16) {
        let var = self.var();
        let base = self.base;
        self.clk.vol(hw, ch, Self::vol_of(l0));
        if vo.loopmode {
            self.clk.adv(2 * 30);
            self.clk.loc(hw, ch, base + self.lay.sample as u32 + vo.loop_off);
            self.clk.adv(2 * 30);
            self.clk.len(hw, ch, vo.llen);
            self.clk.adv(2 * 30);
            self.clk.per(hw, ch, p);
            return;
        }
        // a. wait for a raster line start, then past colour clock 22
        self.clk.adv(2 * VOL_TO_READ[var][ch]);
        self.clk.wait_w();
        // b. period 1
        self.clk.adv(24);
        self.clk.per(hw, ch, 1);
        // c. stop the channel and wait for its interrupt flag
        self.clk.adv(52);
        let (running, _) = self.clk.dma_flag(hw, ch);
        let bit = 0x80u16 << ch;
        self.clk.intreq(hw, if running { bit } else { 0 });
        self.clk.adv(12);
        self.clk.dmacon(hw, if running { 1 << ch } else { 0 });
        let tdma = self.clk.t2;
        if running {
            let mut j = 0;
            loop {
                self.clk.t2 = tdma + 2 * 150 - 29 + 29 * j;
                let (_, flag) = self.clk.dma_flag(hw, ch);
                if flag || j > 4000 {
                    break;
                }
                j += 1;
            }
            self.clk.t2 = tdma + 2 * 150 + 29 * j.min(4000);
        } else {
            self.clk.t2 = tdma + 2 * 150;
        }
        self.clk.fixed = true;
        // d. new sample
        self.clk.loc(hw, ch, base + self.lay.sample as u32 + vo.attack);
        self.clk.adv(2 * LC_TO_LEN[var][ch]);
        self.clk.len(hw, ch, vo.alen);
        self.clk.adv(2 * LEN_TO_PER[var][ch]);
        self.clk.per(hw, ch, p);
        // e. wait again, then start the channel
        self.clk.adv(2 * PER_TO_READ2[var][ch]);
        self.clk.wait_w();
        self.clk.adv(2 * 37);
        self.clk.intreq(hw, bit);
        self.clk.adv(12);
        self.clk.dmacon(hw, 0x8000 | (1 << ch));
        // g. wait for the interrupt flag, then the beam once more
        self.clk.adv(2 * if ch >= 2 { 165 } else { 180 });
        self.clk.wait_w();
        // h. loop part
        if vo.loop_off != 0 {
            self.clk.adv(2 * 75);
            self.clk.len(hw, ch, vo.llen);
            self.clk.adv(2 * LEN_TO_LC_LOOP[var][ch]);
            self.clk.loc(hw, ch, base + self.lay.sample as u32 + vo.loop_off);
        } else {
            self.clk.adv(2 * 63);
            self.clk.len(hw, ch, 0x20);
            self.clk.adv(2 * 14);
            self.clk.loc(hw, ch, base + self.lay.sample as u32);
        }
    }

    fn maintenance(&mut self, hw: &mut Hw, v: usize) {
        let c = self.costs;
        // a. envelope
        if self.v[v].env.is_some() {
            let vo = &mut self.v[v];
            let before = vo.envcnt;
            vo.envcnt = before.wrapping_sub(1);
            if before <= 1 {
                vo.envcnt = vo.env_int;
                let p = vo.env.unwrap();
                let e = self.d.get(p).copied().unwrap_or(0);
                if e == 0 {
                    vo.env = None;
                    self.clk.item(c, k::ENV_END, v, 1);
                } else {
                    vo.env = Some(p + 1);
                    let level = ((((e as u32 + 1) * vo.tvol as u32) & 0xFFFF) * vo.ivol as u32) >> 16;
                    self.clk.item(c, k::ENV_STEP, v, 1);
                    self.clk.vol(hw, v, Self::vol_of(level & 0xFFFF));
                    self.clk.item(c, k::ENV_TAIL, v, 1);
                }
            } else {
                self.clk.item(c, k::ENV_WAIT, v, 1);
            }
        } else {
            self.clk.item(c, k::ENV_NONE, v, 1);
        }
        // b. slide
        let vo = &mut self.v[v];
        if vo.slide != 0 {
            let mut x = ((vo.pitch << 4) as u32 + vo.slide as u16 as u32) as u16;
            if x & 0x8000 != 0 {
                x = 0;
            }
            let idx = x >> 4;
            if vo.slide > 0 {
                if idx >= vo.target {
                    vo.pitch = vo.target;
                    vo.slide = 0;
                } else {
                    vo.pitch = idx;
                }
            } else if idx <= vo.target {
                vo.pitch = vo.target;
                vo.slide = 0;
            } else {
                vo.pitch = idx;
            }
            self.clk.item(c, k::SLIDE_ACT, v, 1);
        } else {
            self.clk.item(c, k::SLIDE_NONE, v, 1);
        }
        // c. pitch table, period
        let mut done = false;
        if let Some(p) = self.v[v].pit_tab {
            let vo = &mut self.v[v];
            let before = vo.pitcnt;
            vo.pitcnt = before.wrapping_sub(1);
            if before <= 1 {
                vo.pitcnt = vo.pit_int;
                let b = self.d.get(p).copied().unwrap_or(0x80) as i8;
                if b == -128 {
                    vo.pit_tab = None;
                    self.clk.item(c, k::PIT_END, v, 1);
                } else {
                    vo.pit_tab = Some(p + 1);
                    let idx = vo.pitch as i16 as i32 + b as i32;
                    let vv = self.v[v].clone();
                    let per = self.period(&vv, idx);
                    self.clk.item(c, k::PIT_STEP, v, 1);
                    self.clk.per(hw, v, per);
                    self.clk.item(c, k::PIT_TAIL, v, 1);
                    done = true;
                }
            } else {
                self.clk.item(c, k::PIT_WAIT, v, 1);
            }
        } else {
            self.clk.item(c, k::PIT_NONE, v, 1);
        }
        if !done {
            let vo = &self.v[v];
            if vo.start != vo.target {
                let vv = vo.clone();
                let per = self.period(&vv, vv.pitch as i16 as i32);
                self.clk.item(c, k::PLAIN_WR, v, 1);
                self.clk.per(hw, v, per);
            } else {
                self.clk.item(c, k::PLAIN_NO, v, 1);
            }
        }
    }
}

// The N engine's Replayer wrapper (the host groups of the specification's 3.3).
struct N {
    p: NPlayer,
    first: u16,
}

impl N {
    /// The stop group the host makes before a subsong other than the first one played.
    fn host_stop(&mut self, hw: &mut Hw) {
        hw.dmacon(0x000F);
        for ch in 0..4 {
            hw.wait(if ch == 0 { 8 } else { 6 });
            hw.vol(ch, 0);
        }
    }
}

impl Replayer for N {
    fn format(&self) -> &'static str {
        "Mark Cooksey"
    }
    fn title(&self) -> String {
        String::new()
    }
    fn tick(&mut self, hw: &mut Hw) {
        self.p.tick(hw);
    }
    fn songend(&self) -> bool {
        self.p.songend
    }
    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.p.lay.nsub as u16 - 1, self.p.sub)
    }
    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if n != self.first {
            self.host_stop(hw);
        }
        self.p.start_sub(n);
    }
}

fn new_n(d: &[u8], base: u32, m: &mut Machine, b: bool) -> Option<Box<dyn Replayer>> {
    let lay = locate_n(d, b)?;
    let mut p = NPlayer {
        d: d.to_vec(),
        base,
        lay,
        sub: 0,
        v: Default::default(),
        flags: [true; 4],
        songend: false,
        clk: Clock::new(),
        costs: if b { &COST_N_B } else { &COST_N_A },
    };
    p.start_sub(0);
    if b {
        // hardware initialisation, once at load
        let mut hw = Hw { bus: &mut m.bus };
        for ch in 0..4 {
            hw.len(ch, 0x7F);
            hw.wait(if ch == 0 { 8 } else { 6 });
        }
        for ch in 0..4 {
            hw.per(ch, 1);
            hw.wait(6);
        }
        for ch in 0..4 {
            hw.vol(ch, 0);
            hw.wait(6);
        }
        hw.bus.write_word(0xDFF09E, 0x00FF);
        hw.wait(8);
        hw.dmacon(0x820F);
    }
    Some(Box::new(N { p, first: 0 }))
}

// ------------------------------------------------------------------ engine O: player

mod ko {
    pub const PRE: usize = 0;
    pub const VOICE: usize = 1;
    pub const WAIT: usize = 2;
    pub const NOTE: usize = 3;
    pub const PERIOD: usize = 4;
    pub const INSTR: usize = 5;
    pub const VOLUME: usize = 6;
    pub const REST: usize = 7;
    pub const OFF: usize = 8;
    pub const STOP: usize = 9;
    pub const CALL: usize = 10;
    pub const RET: usize = 11;
    pub const LOOPB: usize = 12;
    pub const LOOPE: usize = 13;
    pub const CALLT: usize = 14;
    pub const MARK: usize = 15;
    pub const LOOPM: usize = 16;
    pub const VEND: usize = 17;
}

/// The same for engine O.
static COST_O: [i32; NK * 8] = mk(&[
    (9, 55), (10, 58), (12, 300), (13, 301), (14, 338), (15, 321), (20, 416), (21, 131),
    (22, 189), (23, 126), (24, 380), (25, 464), (26, 483), (27, 440), (32, 244), (33, 302),
    (34, 252), (35, 223), (41, 183), (42, 167), (43, 204), (45, 109), (46, 70), (47, 90),
    (49, 131), (50, 125), (51, 122), (53, 82), (54, 53), (55, 95), (57, 188), (58, 147),
    (61, 90), (62, 79), (63, 148), (171, 1596), (172, 5422), (173, 5044), (174, 4620),
    (175, 4103), (182, 14796), (183, 3178), (184, 5001), (192, 6161), (193, 3134), (194, 5362),
    (195, 2016), (201, 262), (205, 114), (208, 1305), (212, 1212), (214, 100), (220, 2957),
]);

#[derive(Clone, Default)]
struct OVoice {
    cnt: u16,
    tp: usize,
    stack: Vec<usize>,
    ptab: isize,
    vol: u16,
    rec: usize,
}

struct OPlayer {
    mcr: Vec<u8>,
    /// The `mcs` file in memory, and its address in the guest.
    mcs: Vec<u8>,
    mcs_addr: u32,
    silence: u32,
    centre: usize,
    sub: u16,
    nsub: u16,
    v: [OVoice; 4],
    flags: [bool; 4],
    songend: bool,
    clk: Clock,
    costs: &'static [i32],
}

const O_HDR: usize = 28;

// Timing constants of engine O's note start in colour clocks, by channel.
const O_STOP_TO_LC: u64 = 18;
const O_LC_TO_LEN: [u64; 4] = [57, 72, 88, 92];
const O_LEN_TO_VOL: [u64; 4] = [141, 165, 184, 202];
const O_VOL_TO_PER: [u64; 4] = [73, 88, 104, 108];
const O_PER_TO_READ: [u64; 4] = [79, 95, 110, 114];

impl OPlayer {
    fn mb(&self, o: usize) -> u8 {
        self.mcs.get(O_HDR + o).copied().unwrap_or(0)
    }
    fn mw(&self, o: usize) -> u16 {
        be16(&self.mcs, O_HDR + o)
    }
    fn ml(&self, o: usize) -> u32 {
        be32(&self.mcs, O_HDR + o)
    }
    fn pword(&self, p: isize) -> u16 {
        if p < 0 { 0 } else { be16(&self.mcr, p as usize) }
    }

    fn start_sub(&mut self, n: u16) {
        self.sub = n;
        self.v = Default::default();
        for ch in 0..4 {
            let at = 8 + 16 * n as usize + 4 * ch;
            let v = &mut self.v[ch];
            v.cnt = 1;
            v.tp = self.mcs.get(O_HDR..).map_or(0, |_| be32(&self.mcs, O_HDR + at) as usize);
            v.ptab = self.centre as isize;
        }
        self.flags = [true; 4];
        self.songend = false;
    }

    fn song_flag(&mut self, v: usize) {
        self.flags[v] = false;
        if self.flags.iter().all(|f| !f) {
            self.songend = true;
            self.flags = [true; 4];
        }
    }

    fn pat_addr(&self, x: u8) -> usize {
        let pt = self.ml(0) as usize;
        (pt as i64 + self.mw(pt + x as usize) as i16 as i64).max(0) as usize
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.clk.start(hw);
        self.clk.t2 += PHASE_O;
        let c = self.costs;
        self.clk.item(c, ko::PRE, 0, 1);
        for ch in 0..4 {
            self.voice(hw, ch);
            self.clk.item(c, ko::VEND, ch, 1);
        }
    }

    fn voice(&mut self, hw: &mut Hw, ch: usize) {
        let c = self.costs;
        self.clk.item(c, ko::VOICE, ch, 1);
        let vo = &mut self.v[ch];
        if vo.cnt & 0x8000 != 0 {
            return;
        }
        vo.cnt = vo.cnt.wrapping_sub(1);
        if vo.cnt != 0 {
            self.clk.item(c, ko::WAIT, ch, 1);
            return;
        }
        let bit = 1u16 << ch;
        for _ in 0..4096 {
            let tp = self.v[ch].tp;
            let cmd = self.mb(tp);
            match cmd {
                0x00 => {
                    let x = self.mb(tp + 1);
                    let dur = self.mb(tp + 2);
                    self.v[ch].tp = tp + 3;
                    let per = self.pword(self.v[ch].ptab + x as isize);
                    self.v[ch].cnt = dur as u16;
                    self.clk.item(c, ko::NOTE, ch, 1);
                    self.note_hw(hw, ch, per);
                    return;
                }
                0x04 => {
                    let x = self.mb(tp + 1);
                    let cnt = self.mb(tp + 2);
                    self.v[ch].tp = tp + 3;
                    let per = self.pword(self.v[ch].ptab + x as isize);
                    self.v[ch].cnt = cnt as u16;
                    self.clk.item(c, ko::PERIOD, ch, 1);
                    self.clk.per(hw, ch, per);
                    return;
                }
                0x08 => {
                    let x = self.mb(tp + 1) as usize;
                    self.v[ch].tp = tp + 2;
                    let it = self.ml(4) as usize;
                    self.v[ch].rec = it.wrapping_add(self.ml(it + x) as usize);
                    self.clk.item(c, ko::INSTR, ch, 1);
                }
                0x0C => {
                    let v = self.mb(tp + 1) as u16;
                    self.v[ch].tp = tp + 2;
                    self.v[ch].vol = v.wrapping_mul(4);
                    self.clk.item(c, ko::VOLUME, ch, 1);
                    self.clk.vol(hw, ch, (v.wrapping_mul(4)) & 0x7F);
                }
                0x10 => {
                    let n = self.mb(tp + 1) as i8 as i16 as u16;
                    self.v[ch].tp = tp + 2;
                    self.v[ch].cnt = n;
                    self.clk.item(c, ko::REST, ch, 1);
                    return;
                }
                0x14 => {
                    let n = self.mb(tp + 1);
                    self.v[ch].tp = tp + 2;
                    self.v[ch].cnt = n as u16;
                    self.clk.item(c, ko::OFF, ch, 1);
                    self.clk.dmacon(hw, bit);
                    return;
                }
                0x18 => {
                    self.v[ch].tp = tp + 1;
                    self.v[ch].cnt = 0xFFFF;
                    self.clk.item(c, ko::STOP, ch, 1);
                    self.clk.dmacon(hw, bit);
                    self.song_flag(ch);
                    return;
                }
                0x1C => {
                    let x = self.mb(tp + 1);
                    self.v[ch].stack.push(tp + 2);
                    self.v[ch].tp = self.pat_addr(x);
                    self.clk.item(c, ko::CALL, ch, 1);
                }
                0x20 => {
                    let vo = &mut self.v[ch];
                    if let Some(r) = vo.stack.pop() {
                        vo.tp = r;
                    } else {
                        vo.tp = tp + 1;
                    }
                    vo.ptab = self.centre as isize;
                    self.clk.item(c, ko::RET, ch, 1);
                }
                0x24 => {
                    let n = self.mb(tp + 1) as usize;
                    let vo = &mut self.v[ch];
                    vo.tp = tp + 2;
                    vo.stack.push(tp + 2);
                    vo.stack.push(n);
                    self.clk.item(c, ko::LOOPB, ch, 1);
                }
                0x28 => {
                    let vo = &mut self.v[ch];
                    let n = vo.stack.pop().unwrap_or(1);
                    let n = n.wrapping_sub(1) & 0xFFFF;
                    self.clk.item(c, ko::LOOPE, ch, 1);
                    if n == 0 {
                        vo.stack.pop();
                        vo.tp = tp + 1;
                    } else {
                        vo.tp = vo.stack.last().copied().unwrap_or(tp + 1);
                        vo.stack.push(n);
                    }
                }
                0x2C => {
                    let t = self.mb(tp + 1) as i8 as isize;
                    let x = self.mb(tp + 2);
                    self.v[ch].stack.push(tp + 3);
                    self.v[ch].tp = self.pat_addr(x);
                    self.v[ch].ptab = self.centre as isize + t;
                    self.clk.item(c, ko::CALLT, ch, 1);
                }
                0x30 => {
                    let vo = &mut self.v[ch];
                    vo.tp = tp + 1;
                    vo.stack.push(tp + 1);
                    self.clk.item(c, ko::MARK, ch, 1);
                }
                0x34 => {
                    let vo = &mut self.v[ch];
                    vo.tp = vo.stack.last().copied().unwrap_or(tp + 1);
                    self.clk.item(c, ko::LOOPM, ch, 1);
                    self.song_flag(ch);
                }
                _ => {
                    // unknown command: the voice stops
                    self.v[ch].cnt = 0xFFFF;
                    return;
                }
            }
        }
        self.v[ch].cnt = 0xFFFF;
    }

    fn note_hw(&mut self, hw: &mut Hw, ch: usize, per: u16) {
        let bit = 1u16 << ch;
        self.clk.dmacon(hw, bit);
        self.clk.adv(2 * O_STOP_TO_LC);
        let rec = self.v[ch].rec;
        let a = self.mcs_addr + (O_HDR + rec) as u32 + 2;
        let len = self.mw(rec);
        self.clk.loc(hw, ch, a);
        self.clk.adv(2 * O_LC_TO_LEN[ch]);
        self.clk.len(hw, ch, len);
        self.clk.adv(2 * O_LEN_TO_VOL[ch]);
        let vol = self.v[ch].vol & 0x7F;
        self.clk.vol(hw, ch, vol);
        self.clk.adv(2 * O_VOL_TO_PER[ch]);
        self.clk.per(hw, ch, per);
        self.clk.adv(2 * O_PER_TO_READ[ch]);
        self.clk.wait_d();
        self.clk.adv(2 * 47);
        self.clk.dmacon(hw, 0x8000 | bit);
        self.clk.adv(2 * 31);
        self.clk.wait_d();
        self.clk.adv(2 * 44);
        self.clk.loc(hw, ch, self.silence);
        self.clk.adv(2 * 8);
        self.clk.len(hw, ch, 2);
    }
}

struct O {
    p: OPlayer,
}

impl Replayer for O {
    fn format(&self) -> &'static str {
        "Mark Cooksey"
    }
    fn title(&self) -> String {
        String::new()
    }
    fn tick(&mut self, hw: &mut Hw) {
        self.p.tick(hw);
    }
    fn songend(&self) -> bool {
        self.p.songend
    }
    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.p.nsub - 1, self.p.sub)
    }
    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.p.start_sub(n);
    }
}

fn new_o(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    // the code behind the first branch slot loads the period table's centre
    let first = 30 + be16(d, 30) as usize;
    let mut o = first & !1;
    let mut centre = None;
    while o + 4 <= d.len() {
        if be16(d, o) == 0x45FA {
            let t = o as isize + 2 + be16(d, o + 2) as i16 as isize;
            if t >= 0 && (t as usize) + 2 <= d.len() {
                centre = Some(t as usize);
            }
            break;
        }
        o += 2;
    }
    let centre = centre?;
    // `mcs.NAME` beside `mcr.NAME`
    let file = m.bus.read_cstr(m.deli.file_buf);
    if file.len() < 4 || !file[..4].eq_ignore_ascii_case("mcr.") {
        return None;
    }
    let mcs = crate::deli::read_companion(m, &format!("mcs.{}", &file[4..])).1.ok()?;
    if mcs.len() < O_HDR + 16 || be16(&mcs, 0) != 0x601A {
        return None;
    }
    let l = be32(&mcs, O_HDR + 8) as usize;
    if l < 24 || (l - 8) % 16 != 0 || l >= mcs.len() {
        return None;
    }
    let nsub = ((l - 8) / 16) as u16;
    let mcs_addr = m.alloc(mcs.len() as u32 + 16);
    let silence = m.alloc(8);
    if mcs_addr == 0 || silence == 0 {
        return None;
    }
    m.bus.write_bytes(mcs_addr, &mcs);
    let mut p = OPlayer {
        mcr: d.to_vec(),
        mcs,
        mcs_addr,
        silence,
        centre,
        sub: 0,
        nsub,
        v: Default::default(),
        flags: [true; 4],
        songend: false,
        clk: Clock::new(),
        costs: &COST_O,
    };
    p.start_sub(0);
    {
        let hw = Hw { bus: &mut m.bus };
        hw.bus.write_word(0xDFF09E, 0x00FF);
    }
    Some(Box::new(O { p }))
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if let Some(b) = detect_n(d) {
        return new_n(d, base, m, b);
    }
    if detect_o(d) {
        return new_o(d, base, m);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn p16(d: &mut [u8], o: usize, x: u16) {
        d[o..o + 2].copy_from_slice(&x.to_be_bytes());
    }
    fn p32(d: &mut [u8], o: usize, x: u32) {
        d[o..o + 4].copy_from_slice(&x.to_be_bytes());
    }
    /// A position-independent address load at `o` of `target`.
    fn lea(d: &mut [u8], o: usize, op: u16, target: usize) {
        p16(d, o, op);
        p16(d, o + 2, (target as isize - (o as isize + 2)) as u16);
    }

    /// A small variant-A module: two subsongs, one instrument with an attack sample.
    fn module_a() -> Vec<u8> {
        let mut d = vec![0u8; 0x1000];
        p32(&mut d, 0, 0xD040_D040);
        p16(&mut d, 4, 0x4EFB);
        p16(&mut d, 6, 2);
        for o in [8usize, 12, 16, 20] {
            p16(&mut d, o, 0x6000);
            p16(&mut d, o + 2, (0x100 - (o + 2)) as u16);
        }
        let (pt, inst, per, state, sample, t1, t2, tr) = (0x300, 0x340, 0x400, 0x700, 0xA00, 0x800, 0x820, 0x900);
        lea(&mut d, 40, 0x43FA, t1);
        for (k, t) in [state, sample, pt, inst, sample, per].iter().enumerate() {
            lea(&mut d, 0x100 + 4 * k, 0x41FA, *t);
        }
        lea(&mut d, 0x120, 0x43FA, t2);
        // period table: four entries per semitone
        for i in 0..384 {
            let v = (65536.0 * 2f64.powf(-((i + 12) as f64) / 48.0)) as u16;
            p16(&mut d, per + 2 * i, v);
        }
        // instrument 0 at 0x360
        p16(&mut d, inst, (0x360 - inst) as u16);
        p32(&mut d, 0x360, 0x40);
        p16(&mut d, 0x360 + 16, 4);
        p16(&mut d, 0x360 + 20, 0x8000);
        p16(&mut d, 0x360 + 26, 0x100);
        for i in 0..8 {
            d[sample + 0x40 + i] = (i as u8).wrapping_mul(30);
        }
        // subsong 0: voice 0 plays two notes and a rest, then jumps back; subsong 1 stops
        p32(&mut d, t1, (tr - t1) as u32);
        p32(&mut d, t1 + 16, (0x940 - (t1 + 16)) as u32);
        let t = [0x63, 0x80, 0x30, 0, 4, 0, 0x34, 0, 3, 0, 0x60, 0, 2, 0, 0x68, 0xFF, 0xF2];
        d[tr..tr + t.len()].copy_from_slice(&t);
        d[0x940..0x943].copy_from_slice(&[0x63, 0x40, 0x67]);
        d
    }

    fn module_o() -> (Vec<u8>, Vec<u8>) {
        let mut r = vec![0u8; 0x300];
        p16(&mut r, 0, 0x601A);
        p32(&mut r, 2, 0x40);
        for o in [28usize, 32, 36, 40] {
            p16(&mut r, o, 0x6000);
            p16(&mut r, o + 2, 226);
        }
        r[0x100..0x104].copy_from_slice(&[0x48, 0xE7, 0x80, 0xF0]);
        lea(&mut r, 0x110, 0x45FA, 0x200);
        for (i, v) in [856u16, 808, 762, 720, 678, 640, 604, 570].iter().enumerate() {
            p16(&mut r, 0x1FC + 2 * i, *v);
        }
        let mut s = vec![0u8; 28 + 0x80];
        p16(&mut s, 0, 0x601A);
        let d = 28;
        p32(&mut s, d, 0x40);
        p32(&mut s, d + 4, 0x50);
        for (v, o) in [0x18u32, 0x20, 0x20, 0x20].iter().enumerate() {
            p32(&mut s, d + 8 + 4 * v, *o);
        }
        s[d + 0x18..d + 0x20].copy_from_slice(&[0x08, 0, 0x0C, 0x20, 0, 0, 4, 0x18]);
        s[d + 0x20] = 0x18;
        p32(&mut s, d + 0x50, 0x10);
        p16(&mut s, d + 0x60, 4);
        for i in 0..8 {
            s[d + 0x62 + i] = (i as u8) * 20;
        }
        (r, s)
    }

    fn play(path: &str, d: Vec<u8>, comp: Option<(&str, Vec<u8>)>, ticks: usize) -> Vec<(u32, u16)> {
        let dir = Path::new("/mc-test");
        let mut e = Engine::new(48000.0);
        if let Some((n, c)) = comp {
            e.m.os.mem_files.insert(dir.join(n), c);
        }
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&dir.join(path), d).unwrap();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect()
    }

    #[test]
    fn detects() {
        let d = module_a();
        assert_eq!(detect_n(&d), Some(false));
        assert!(detect(&d));
        assert!(!detect(&d[..100]));
        let mut e = d.clone();
        e[4] = 0;
        assert!(!detect(&e));
        // variant B: the load moves to offset 150 behind a fifth branch slot
        let mut b = d.clone();
        p16(&mut b, 24, 0x6000);
        p16(&mut b, 40, 0);
        p16(&mut b, 150, 0x43FA);
        assert_eq!(detect_n(&b), Some(true));
        let (r, s) = module_o();
        assert!(detect(&r));
        assert!(!detect(&s));
        assert!(!detect(&[0u8; 400]));
    }

    #[test]
    fn parses_the_layout() {
        let d = module_a();
        let lay = locate_n(&d, false).unwrap();
        assert_eq!((lay.pat, lay.inst, lay.per, lay.sample, lay.sub, lay.nsub), (0x300, 0x340, 0x400, 0xA00, 0x800, 2));
        // a table that does not follow the period table fails the layout checks
        let mut bad = d.clone();
        p16(&mut bad, 0x102, 0x600);
        assert!(locate_n(&bad, false).is_none());
    }

    #[test]
    fn plays_variant_a_deterministically() {
        let a = play("mc.test", module_a(), None, 60);
        let b = play("mc.test", module_a(), None, 60);
        assert_eq!(a, b);
        // a note start: volume, period 1, DMA stop and start, the loop part
        let dma: Vec<u16> = a.iter().filter(|w| w.0 == 0xDFF096).map(|w| w.1).collect();
        assert!(dma.contains(&0x8001), "{dma:x?}");
        assert!(a.iter().any(|w| w.0 == 0xDFF0A4 && w.1 == 4));
        // the rest writes volume 0 and the jump loops the track: more than one note start
        assert!(dma.iter().filter(|&&v| v == 0x8001).count() >= 3);
    }

    #[test]
    fn plays_the_pair_with_its_companion() {
        let (r, s) = module_o();
        let a = play("mcr.test", r.clone(), Some(("mcs.test", s.clone())), 20);
        let b = play("mcr.test", r.clone(), Some(("mcs.test", s)), 20);
        assert_eq!(a, b);
        assert!(a.iter().any(|w| w.0 == 0xDFF096 && w.1 == 0x8001), "{a:x?}");
        // volume 0x20 * 4 = 0x80 written masked to 0
        assert!(a.iter().any(|w| w.0 == 0xDFF0A8));
        // without the companion no replayer takes the module
        let dir = Path::new("/mc-test");
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        assert!(e.play_data(&dir.join("mcr.test"), r).is_err() || e.playing.is_none() || e.playing.as_ref().is_some_and(|p| p.format != "Mark Cooksey"));
    }

    /// `PK_MODULE=file PK_TICKS=n PK_OUT=out [PK_SUBSONG=k] cargo test --release trace_dump -- --ignored`:
    /// plays the module and writes, for every audio write, "reg val t2 fixed counts..." for the cost fitting.
    #[test]
    #[ignore]
    fn trace_dump() {
        let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("PK_MODULE"), std::env::var("PK_TICKS"), std::env::var("PK_OUT")) else {
            return;
        };
        let ticks: usize = ticks.parse().unwrap();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
        let sub: u32 = std::env::var("PK_SUBSONG").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        TRACE.lock().unwrap().clear();
        for _ in 0..sub {
            e.change_subsong(1).unwrap();
        }
        let mut buf = vec![0f32; 2 * 960];
        for _ in 0..ticks {
            e.render(&mut buf);
        }
        let t = TRACE.lock().unwrap();
        let mut text = String::new();
        for (reg, val, t2, fixed, counts) in t.iter() {
            text.push_str(&format!("{reg:x} {val:x} {t2} {}", *fixed as u8));
            for c in counts.iter() {
                text.push_str(&format!(" {c}"));
            }
            text.push('\n');
        }
        std::fs::write(out, text).unwrap();
    }
}
