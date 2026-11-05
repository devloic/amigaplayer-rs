//! Dynamic Synthesizer (Chris Huelsbeck, 1987-88; Eagleplayer "Dynamic Synthesizer", module prefix
//! `dns.`, sample file `smp.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A module is the game's own position-dependent
//! driver followed by its data tables; the driver is never run here, the tables are interpreted.
//! Six tracks share four voices (a rotating allocator with ages picks the victim), each note has a
//! volume envelope (attack, decay to a sustain level, release) and every voice runs a three-step
//! DMA hand-over (off, on, loop). Every register write is placed at the clock of the original's
//! code, from the specification's cycle table, which is established only for the note-start
//! combinations of the two known modules.
//!
//! The data tables sit at per-module offsets. Two layouts are known; a module is accepted only if
//! the period table's signature is found at one layout's offset and the other tables of that
//! layout are consistent with the module's own data. Any other module is left to the 68k player.
//!
//! Choices where the specification is silent: a speed of 0 plays as 1; an unknown pattern
//! reference leaves the track empty and a pattern read past the file's end gives a rest and then
//! the end marker; the song-end flag, once raised, stays raised.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Dynamic Synthesizer", detect, new };

/// The address the module is linked for.
const LINK: i64 = 0x1F400;
/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// Twice the colour clocks the reference's start-up takes before its first period, less the
/// path-length term (measured from its write logs).
const START_UP_X2: u64 = 1_083_151;
/// The song-end tick's extra cost in cycles when the offset before it is even (one more when odd), so that the sum is odd.
const WRAP_EXTRA: u64 = 221;

/// Where the tables are in one known module.
#[derive(Clone, Copy, Debug)]
struct Lay {
    per: usize,
    seq: usize,
    pat: usize,
    sub: usize,
    ins: usize,
    shift: u32,
    /// Extra cycles of every note start (the in-game module's code is four cycles longer).
    start_extra: u64,
}

const LAYOUTS: [Lay; 2] = [
    Lay { per: 0x0D64, seq: 0x0DC4, pat: 0x1324, sub: 0x1244, ins: 0x1FD4, shift: 0, start_extra: 0 },
    Lay { per: 0x1950, seq: 0x19B0, pat: 0x28B8, sub: 0x27D8, ins: 0x39C0, shift: 2, start_extra: 4 },
];

const PERIODS: [u16; 48] = [
    1710, 1614, 1524, 1438, 1357, 1281, 1209, 1141, 1077, 1017, 960, 908, 856, 810, 764, 720, 680, 642, 606, 571, 539,
    509, 480, 454, 428, 404, 381, 360, 340, 320, 303, 286, 270, 254, 240, 227, 214, 202, 191, 180, 170, 160, 151, 143,
    135, 127, 120, 113,
];

// ---------------------------------------------------------------------------------
// Recognition

/// The five entry slots and the check of the fifth slot's target (spec 1.1).
fn detect_driver(d: &[u8]) -> bool {
    for i in 0..5 {
        let o = 4 * i;
        if be16(d, o) != 0x6000 || be16(d, o + 2) & 0x8001 != 0 {
            return false;
        }
    }
    let t = 18 + be16(d, 18) as usize;
    t + 8 <= d.len() && be16(d, t) == 0x23C8 && be16(d, t + 6) == 0x4E75
}

fn subsong_count(d: &[u8], sub: usize) -> usize {
    for n in 1..32 {
        if be16(d, sub + 2 * n) == be16(d, sub + 2 * n + 2) {
            return n;
        }
    }
    32
}

/// The layout whose period table is at its offset and whose other tables fit the module, and the
/// subsong count.
fn locate(d: &[u8]) -> Option<(Lay, usize)> {
    if !detect_driver(d) {
        return None;
    }
    // the signature: the first words of the period table, wherever they are
    let sig = |o: usize| (0..8).all(|i| be16(d, o + 2 * i) == PERIODS[i]);
    let found = (0..d.len().saturating_sub(16)).step_by(2).find(|&o| sig(o))?;
    let lay = LAYOUTS.iter().copied().find(|l| l.per == found)?;
    if !(0..48).all(|i| be16(d, lay.per + 2 * i) == PERIODS[i]) {
        return None;
    }
    // table order and extents
    if !(lay.per < lay.seq && lay.seq < lay.sub && lay.sub < lay.pat && lay.pat < lay.ins) {
        return None;
    }
    if lay.ins + 0x60 > d.len() || lay.pat + 4 > d.len() || lay.sub + 0xA0 > d.len() {
        return None;
    }
    if (lay.sub - lay.seq) % 24 != 0 {
        return None;
    }
    let lines = (lay.sub - lay.seq) / 24;
    let n = subsong_count(d, lay.sub);
    // each array holds 16 words
    if n > 16 {
        return None;
    }
    for s in 0..n {
        let w = |a: usize| be16(d, lay.sub + 0x20 * a + 2 * s) as usize;
        let (start, lp, end) = (w(0), w(1), w(2));
        if start >= lines || lp >= lines || end >= lines || start > end || lp > end {
            return None;
        }
    }
    // the first pattern reference must land inside the module
    let p = be32(d, lay.pat) as i64 - LINK;
    if p < 0 || p as usize >= d.len() {
        return None;
    }
    Some((lay, n))
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

// ---------------------------------------------------------------------------------
// State

/// Release cost in cycles by voice, and the period write's extra delay by voice.
const REL: [u64; 4] = [534, 553, 572, 582];
const VCOST: [u64; 4] = [0, 93, 186, 210];

const P_IDLE: u8 = 0;
const P_RELEASE: u8 = 1;
const P_DECAY: u8 = 2;
const P_ATTACK: u8 = 3;

#[derive(Clone, Copy, Default)]
struct Voice {
    age: u16,
    vol: i16,
    phase: u8,
    cnt: u16,
    reload: u16,
    delta: i16,
    target: i16,
    held: bool,
    jump: u16,
    sustain: i16,
    dec_int: u16,
    dec_delta: i16,
    rel_int: u16,
    rel_delta: i16,
    dma: u8,
    loop_lc: u32,
    loop_len: u16,
}

#[derive(Clone, Copy, Default)]
struct Track {
    owner: Option<usize>,
    playing: bool,
}

#[derive(Clone, Copy, Default)]
struct Entry {
    pat: u8,
    trans: u8,
    ins: u8,
}

pub struct DynamicSynthesizer {
    d: Vec<u8>,
    lay: Lay,
    subsongs: usize,
    smp: u32,
    sub: usize,
    // sequencer
    speed: u16,
    counter: u16,
    line: u16,
    loop_line: u16,
    end: u16,
    mute: u8,
    row: u16,
    last: bool,
    entries: [Entry; 6],
    // players
    tracks: [Track; 6],
    voices: [Voice; 4],
    rot: usize,
    active: bool,
    songend: bool,
    wrapped: bool,
    /// The next tick is the first of the subsong (11 cycles ahead of the grid).
    first: bool,
    next_at: Option<u64>,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (lay, subsongs) = locate(d)?;
    // sample file: `dns.NAME` -> `smp.NAME` (any case of the prefix)
    let file = m.bus.read_cstr(m.deli.file_buf);
    let rest = match file.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("dns.") => &file[4..],
        _ => return None,
    };
    let smp = crate::deli::read_companion(m, &format!("smp.{rest}")).1.ok()?;
    let a = m.alloc((smp.len() as u32 + 4).max(8));
    if a == 0 {
        return None;
    }
    m.bus.write_bytes(a, &smp);
    // the reference's DMA reads the sample's first long word as 0xFFFFFFFF whatever the file holds
    // (seen in its DMA log on both corpus files), so it is stamped over at load
    m.bus.write_bytes(a, &[0xFF; 4]);
    let mut s = DynamicSynthesizer::build(d.to_vec(), lay, subsongs, a);
    // the reference's first tick falls START_UP colour clocks after the start phase's first
    // period; the song's audio counts from one period before it (the clock left after `new`)
    // (the host copies the module's directory and file name once per character, 17 cycles each:
    // the start-up time grows by 8.5 colour clocks per character of the path, from the logs of
    // runs from directories of different name lengths)
    let plen = (m.bus.read_cstr(m.deli.dir_buf).len() + file.len()) as u64;
    let t = m.bus.clock + TICK + (START_UP_X2 + 17 * plen) / 2;
    s.next_at = Some(t);
    m.bus.clock = t - TICK;
    Some(Box::new(s))
}

impl DynamicSynthesizer {
    fn build(d: Vec<u8>, lay: Lay, subsongs: usize, smp: u32) -> Self {
        let mut s = DynamicSynthesizer {
            d,
            lay,
            subsongs,
            smp,
            sub: 0,
            speed: 1,
            counter: 1,
            line: 0,
            loop_line: 0,
            end: 0,
            mute: 0,
            row: 0,
            last: false,
            entries: [Entry::default(); 6],
            tracks: [Track::default(); 6],
            voices: [Voice::default(); 4],
            rot: 0,
            active: false,
            songend: false,
            wrapped: false,
            first: true,
            next_at: None,
        };
        s.init(0);
        s
    }

    fn sub_word(&self, array: usize, s: usize) -> u16 {
        be16(&self.d, self.lay.sub + 0x20 * array + 2 * s)
    }

    /// Fresh state for subsong `s` (spec 3, 3.2, 3.6).
    fn init(&mut self, s: usize) {
        self.sub = s;
        self.line = self.sub_word(0, s);
        self.loop_line = self.sub_word(1, s);
        self.end = self.sub_word(2, s);
        // a speed of 0 is undefined in the specification: play it as 1
        self.speed = self.sub_word(3, s).max(1);
        self.mute = self.sub_word(4, s) as u8;
        self.counter = 1;
        self.tracks = [Track { owner: Some(0), playing: false }; 6];
        self.voices = [Voice { age: 0xFFFF, ..Voice::default() }; 4];
        self.rot = 0;
        self.active = false;
        self.songend = false;
        self.wrapped = false;
        self.first = true;
        self.load_line();
        self.wrapped = false;
    }

    /// Spec 3.6: the entries of the current line, then the next line index.
    fn load_line(&mut self) {
        self.row = 0;
        self.last = false;
        let base = self.lay.seq + 24 * self.line as usize;
        for t in 0..6 {
            let o = base + 4 * t;
            let g = |i: usize| self.d.get(o + i).copied().unwrap_or(0);
            self.entries[t] = Entry { pat: g(0), trans: g(1), ins: g(2) };
        }
        self.line = self.line.wrapping_add(1);
        if self.line > self.end {
            self.line = self.loop_line;
            self.songend = true;
            self.wrapped = true;
        }
    }

    /// The row word of pattern `pat` at `row + ahead`; None for an unusable pattern reference.
    /// Past the file's end the row itself reads as a rest and the next one as the end marker.
    fn row_word(&self, pat: u8, row: u16, ahead: usize) -> Option<u16> {
        let p = be32(&self.d, self.lay.pat + 4 * (pat as usize - 1)) as i64 - LINK;
        if p < 0 {
            return None;
        }
        let o = p as usize + 2 * (row as usize + ahead);
        if o + 2 > self.d.len() {
            return Some(if ahead == 0 { 0 } else { 0xFFFF });
        }
        Some(be16(&self.d, o))
    }

    // --- voices

    fn release(&mut self, v: usize) {
        let vo = &mut self.voices[v];
        vo.held = false;
        vo.age |= 0x8000;
        self.begin_release(v);
    }

    fn begin_release(&mut self, v: usize) {
        let vo = &mut self.voices[v];
        if vo.jump == 1 {
            vo.vol = vo.sustain;
        }
        vo.target = 0;
        vo.cnt = vo.rel_int;
        vo.reload = vo.rel_int;
        vo.delta = vo.rel_delta;
        vo.phase = P_RELEASE;
    }

    /// Chooses the voice of a new note (spec 3.2): the voice and the number of replacements.
    fn choose(&mut self) -> (usize, u64) {
        let mut best = 0u16;
        let mut pick = self.rot;
        let mut n = 0;
        for k in 0..4 {
            let v = (self.rot + 4 - k) % 4;
            if self.voices[v].age > best {
                best = self.voices[v].age;
                pick = v;
                n += 1;
            }
        }
        self.rot = (self.rot + 2) % 4;
        self.voices[pick].age = 0;
        (pick, n)
    }

    // --- timing: a write at `s` cycles into the tick that starts at `c` cycles

    fn put(hw: &mut Hw, c: u64, s: u64) {
        hw.bus.clock = (c + s) / 2;
    }

    // --- a row tick; returns the offset after the six tracks

    fn row_tick(&mut self, hw: &mut Hw, c: u64) -> u64 {
        let mut s: u64 = 374;
        for t in 0..6 {
            let e = self.entries[t];
            if self.mute >> t & 1 != 0 {
                s += 95;
                continue;
            }
            if e.pat == 0 {
                s += 128;
                continue;
            }
            let Some(word) = self.row_word(e.pat, self.row, 0) else {
                s += 128;
                continue;
            };
            if self.row_word(e.pat, self.row, 1) == Some(0xFFFF) {
                self.last = true;
            }
            let l16 = if self.last { 16 } else { 0 };
            match word {
                0 => s += 276 + l16,
                0x8000 => {
                    self.tracks[t].playing = false;
                    match self.tracks[t].owner {
                        None => s += 218,
                        Some(v) => {
                            self.release(v);
                            self.tracks[t].owner = None;
                            s += 161 + REL[v] + l16;
                        }
                    }
                }
                w => {
                    let mut s1 = 1769 + l16 + self.lay.start_extra;
                    if self.tracks[t].playing {
                        match self.tracks[t].owner {
                            None => s1 += 227,
                            Some(v) => {
                                self.release(v);
                                self.tracks[t].owner = None;
                                s1 += REL[v];
                            }
                        }
                    }
                    self.tracks[t].playing = true;
                    s = self.note_start(hw, c, s, s1, t, w, e);
                }
            }
        }
        self.row = self.row.wrapping_add(1);
        s += 148;
        if self.last {
            self.load_line();
            s += 371;
        }
        s
    }

    /// Spec 3.2: a note starts on track `t`. `s` is the offset at the track's start, `s1` the cost
    /// before the first write so far. Returns the offset after the track.
    #[allow(clippy::too_many_arguments)]
    fn note_start(&mut self, hw: &mut Hw, c: u64, s: u64, mut s1: u64, t: usize, w: u16, e: Entry) -> u64 {
        let boost = (((w >> 4) & 0xF) as i32) << self.lay.shift;
        let ins = ((w & 0xF) as u8).wrapping_add(e.ins);
        let note = ((w >> 8) as u8).wrapping_add(e.trans).wrapping_add(1);
        let (v, cmp) = self.choose();
        s1 += 52 * cmp;
        for u in 0..6 {
            if self.tracks[u].owner == Some(v) {
                self.tracks[u].owner = None;
                s1 += 18;
            }
        }
        self.tracks[t].owner = Some(v);

        let b = self.lay.ins + 0x60 * ins as usize;
        let w16 = |o: usize| be16(&self.d, b + o);
        let sv = |o: usize| be16(&self.d, b + o) as i16;
        let lc = self.smp.wrapping_add(be32(&self.d, b));
        let len = w16(0x06);
        let loop_lc = self.smp.wrapping_add(be32(&self.d, b + 0x08));
        let start_vol = sv(0x1C) as i32 + boost;
        let target = sv(0x1E) as i32 + boost;
        let (c1, c2) = ((start_vol > 0x40) as u64, (target > 0x40) as u64);
        let raw = be16(&self.d, self.lay.per + 2 * (note & 0x3F) as usize).wrapping_add(w16(0x4A));
        let (per, p) = if raw < 0x71 { (0x71, 1) } else { (raw, 0) };

        let vo = &mut self.voices[v];
        vo.loop_lc = loop_lc;
        vo.loop_len = w16(0x0E);
        vo.held = true;
        vo.jump = w16(0x24);
        vo.sustain = sv(0x26);
        vo.dec_int = w16(0x20);
        vo.dec_delta = sv(0x22);
        vo.rel_int = w16(0x28);
        vo.rel_delta = sv(0x2A);
        vo.phase = P_ATTACK;
        vo.cnt = w16(0x18);
        vo.reload = vo.cnt;
        vo.delta = sv(0x1A);
        vo.vol = start_vol.min(0x40) as i16;
        vo.target = target.min(0x40) as i16;
        vo.dma = 3;
        self.active = true;

        let ptime = 1148 + VCOST[v] + 8 * (c1 + c2 + p);
        Self::put(hw, c, s + s1);
        hw.dmacon(1 << v);
        Self::put(hw, c, s + s1 + 132);
        hw.loc(v, lc);
        Self::put(hw, c, s + s1 + 152);
        hw.len(v, len);
        Self::put(hw, c, s + s1 + 152 + ptime);
        hw.per(v, per);
        s + s1 + 152 + ptime + 132
    }

    /// Spec 3.4 for voice `v`: the cost in cycles.
    fn envelope(&mut self, v: usize) -> u64 {
        let vo = &mut self.voices[v];
        if vo.vol == 0 {
            vo.age = 0xFFFF;
            return 67;
        }
        vo.age = vo.age.wrapping_add(1);
        if vo.phase != P_RELEASE {
            vo.age &= 0x7FFF;
        }
        if vo.phase == P_IDLE {
            return 118;
        }
        let rel = vo.phase == P_RELEASE;
        vo.cnt = vo.cnt.wrapping_sub(1);
        if vo.cnt != 0 {
            return if rel { 168 } else { 192 };
        }
        vo.cnt = vo.reload;
        vo.vol = vo.vol.wrapping_add(vo.delta);
        let going = if vo.delta >= 0 { vo.vol < vo.target } else { vo.vol > vo.target };
        if going {
            return if rel { 294 } else { 318 };
        }
        vo.vol = vo.target;
        match vo.phase {
            P_RELEASE => {
                vo.phase = P_IDLE;
                373
            }
            P_DECAY => {
                if !vo.held {
                    self.begin_release(v);
                }
                439
            }
            _ => {
                vo.target = vo.sustain;
                vo.cnt = vo.dec_int;
                vo.reload = vo.dec_int;
                vo.delta = vo.dec_delta;
                vo.phase = P_DECAY;
                540
            }
        }
    }

    /// The part every tick runs once a note has started (spec 3 step 2), from offset `s`.
    fn active_part(&mut self, hw: &mut Hw, c: u64, s: u64) {
        Self::put(hw, c, s);
        hw.bus.write_byte(0xBF_E001, 2);
        for (v, off) in [242u64, 551, 881, 1252].into_iter().enumerate() {
            Self::put(hw, c, s + off);
            hw.vol(v, (self.voices[v].vol as u16) & 0x7F);
        }
        let mut t = s + 1252 + 413;
        for v in 0..4 {
            t += self.envelope(v);
        }
        t += 12;
        for v in 0..4 {
            t += 26;
            let st = self.voices[v].dma;
            match st {
                2 => {
                    Self::put(hw, c, t + 137);
                    hw.dmacon(0x8200 | 1 << v);
                    self.voices[v].dma = 1;
                }
                1 => {
                    Self::put(hw, c, t + 217);
                    hw.loc(v, self.voices[v].loop_lc);
                    Self::put(hw, c, t + 237);
                    hw.len(v, self.voices[v].loop_len);
                    self.voices[v].dma = 0;
                }
                3 => self.voices[v].dma = 2,
                _ => {}
            }
            t += 8 + match st {
                0 => 39,
                3 => 78,
                2 => 185,
                _ => 265,
            };
        }
    }

    fn tick_inner(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        let mut c = 2 * t0;
        if self.first {
            // the first tick is 11 cycles ahead of the grid
            c = c.saturating_sub(11);
            self.first = false;
        }
        self.wrapped = false;
        self.counter = self.counter.wrapping_sub(1);
        let mut s;
        if self.counter == 0 {
            self.counter = self.speed;
            s = self.row_tick(hw, c);
            if self.wrapped {
                // 221 or 222 cycles: the one that makes the offset odd fits the three long runs (the
                // half cycle the logs hide); a note start on the first track of the final row
                // ran 16 cycles shorter in one run, which the specification does not explain
                s += WRAP_EXTRA + (s & 1);
            }
        } else {
            s = 401;
        }
        if self.active {
            self.active_part(hw, c, s);
        }
    }

    /// The host's init call: 21 writes (spec 4.1).
    fn init_group(&self, hw: &mut Hw, t0: u64) {
        let at = |hw: &mut Hw, off: u64| hw.bus.clock = t0 + off;
        at(hw, 0);
        hw.dmacon(0x000F);
        for (ch, off) in [8u64, 14, 20, 26].into_iter().enumerate() {
            at(hw, off);
            hw.vol(ch, 0);
        }
        for (ch, off) in [82u64, 100, 118, 136].into_iter().enumerate() {
            at(hw, off);
            hw.loc(ch, self.smp);
        }
        for (ch, off) in [146u64, 156, 166, 176].into_iter().enumerate() {
            at(hw, off);
            hw.len(ch, 1);
        }
        for (ch, off) in [186u64, 196, 206, 216].into_iter().enumerate() {
            at(hw, off);
            hw.vol(ch, 0);
        }
    }
}

impl Replayer for DynamicSynthesizer {
    fn format(&self) -> &'static str {
        "Dynamic Synthesizer"
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

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsongs as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.subsongs - 1);
        // the host calls the player's init once per step of the subsong number (spec 4.1)
        let steps = n.abs_diff(self.sub).max(1) as u64;
        self.init(n);
        let t0 = hw.bus.clock;
        for g in 0..steps {
            self.init_group(hw, t0 + 1572 * g);
        }
        hw.bus.clock = t0 + 1572 * steps;
        // the first tick: 70920 colour clocks after the first group plus one group length per
        // step; the grid of the following ticks starts 6 colour clocks later (the first tick
        // is 11 cycles ahead of it)
        self.next_at = Some(t0 + 70926 + 1572 * steps);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }
}

#[cfg(test)]
#[path = "dynamicsynthesizer_tests.rs"]
mod tests;
