//! Mark Cooksey, old player (`mco.*`; Eagleplayer "Mark_Cooksey_Old"): the 1988 song and sample
//! data of Mark Cooksey's soundtracks played by the older, simpler player (Aquablast, Live and
//! Let Die, Overlander, A Question of Sport, Mike Read's Computer Pop Quiz). A module is one
//! file, byte for byte the `mcs.*` data file of the newer player's engine O.
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! and beam-read traces of the 68k reference player; no original code is used or run, and
//! nothing is embedded: the 40-word period table is a list of values of the specification.
//!
//! Four voices run a byte-coded track language (note, period, instrument, volume, rest, note
//! off, stop, call, return, loops, marks). Every register write is placed at the clock of the
//! original's code, from the specification's cycle ledger: per-visit and per-command costs,
//! and the two waits of a note start (for a number of changes of the raster line, modelled from
//! the beam phase). Volumes are written raw (four times the byte, unmasked and unscaled).
//!
//! Known gap (a decision of the project owner): the original's period table sits behind 16
//! words of the player's own code, which a module made for another period table (Overlander's
//! two first subsongs, with a transposition of -24) reads. Those words are not embedded; an
//! index before the table uses the table's first entry instead, so these few period writes
//! differ from the reference.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): an index
//! after the table uses the last entry; an odd index offset rounds down; a note before any
//! instrument command plays one word of the silence buffer; an invalid command byte, a stack
//! overflow past 40 bytes, a return, loop end or loop to mark on an empty stack or on an entry
//! of the wrong kind, or more than 2^20 commands in one visit make the voice dead (a loop end
//! on a pointer entry just goes on); bytes asked for beyond the end of the file read as zero.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Mark Cooksey Old", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70_940;
const LINE: u64 = 227;
/// The first tick's entry for subsong 0, and what each next-song call adds, in colour clocks
/// from the run's origin (host constants of the reference run).
const FIRST_TICK: u64 = 71_761;
const PER_SUBSONG: u64 = 621;
/// The header the data skips.
const D: usize = 28;

/// The player's period table.
const PERIODS: [u16; 40] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254,
    240, 226, 214, 202, 190, 180, 169, 160, 151, 142, 134, 127, 120, 113, 107, 101, 95, 90,
];
/// Table index of the centre (offset 0).
const CENTRE: i32 = 2;
/// Volume words of the voices before any volume command.
const INIT_VOL: [u16; 4] = [0x14, 0x14, 0x1C, 0x30];

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

// ------------------------------------------------------------------ recognition

/// The number of subsongs, if the file passes the format's checks: the specification's weak
/// test (first byte 0x60, the long at 2 agreeing with the length in its upper 24 bits, byte 32
/// zero) plus consistency of the subsong table and of the offsets in it, so that the weak test
/// does not take other files (a tightening).
fn header(d: &[u8]) -> Option<u16> {
    if d.len() < D + 24 || d[0] != 0x60 || d[D + 4] != 0 {
        return None;
    }
    if (be32(d, 2) & 0xFFFF_FF00) != (d.len() as u32 & 0xFFFF_FF00) {
        return None;
    }
    let body = d.len() - D;
    let pt = be32(d, D) as usize;
    let it = be32(d, D + 4) as usize;
    let l = be32(d, D + 8) as usize;
    if pt >= body || it >= body || l < 24 || l >= body || (l - 8) % 16 != 0 {
        return None;
    }
    let n = (l - 8) / 16;
    if n > 255 {
        return None;
    }
    // every voice's first track byte lies in the file
    (0..n * 4).all(|i| (be32(d, D + 8 + 4 * i) as usize) < body).then_some(n as u16)
}

fn detect(d: &[u8]) -> bool {
    header(d).is_some()
}

// ------------------------------------------------------------------ player state

/// One stack entry: a pointer (call return, mark, 4 bytes) or a loop (pointer and count, 6).
#[derive(Clone, Copy)]
enum Entry {
    Ptr(usize),
    Loop(usize, u16),
}

impl Entry {
    fn size(&self) -> usize {
        match self {
            Entry::Ptr(_) => 4,
            Entry::Loop(..) => 6,
        }
    }
}

#[derive(Clone, Default)]
struct Voice {
    cnt: u16,
    tp: usize,
    stack: Vec<Entry>,
    /// Period-table offset in bytes from the centre.
    ptab: i32,
    vol: u16,
    /// The current sample record (offset after the header), if an instrument was chosen.
    rec: Option<usize>,
}

/// Where a tick's writes go: the cycle count `t` from the tick's entry gives the colour clock.
struct Clk {
    t0: u64,
    /// The tick's entry relative to the run's origin (the beam's reference).
    rel: u64,
    t: u64,
}

impl Clk {
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.t / 2;
    }

    fn line(&self) -> u64 {
        (self.rel + self.t / 2) / LINE
    }

    /// The wait for `n` changes of the raster line with lead-in `lead` (spec 5.3).
    fn wait(&mut self, n: u32, lead: u64) {
        self.t += lead;
        for _ in 0..n {
            self.t += 16;
            let l0 = self.line();
            // bounded: the beam moves on within a line
            for _ in 0..4096 {
                self.t += 16;
                let changed = self.line() != l0;
                self.t += 9;
                if changed {
                    break;
                }
            }
            self.t += 10;
        }
        self.t += 44;
    }
}

pub struct MarkCookseyOld {
    d: Vec<u8>,
    /// The guest address of the file.
    base: u32,
    silence: u32,
    nsub: u16,
    sub: u16,
    v: [Voice; 4],
    /// The run's origin (colour clock of the reference's clock 0).
    origin: u64,
    ticked: bool,
    tick_at: Option<u64>,
    songend: bool,
}

impl MarkCookseyOld {
    fn ml(&self, o: usize) -> u32 {
        be32(&self.d, D + o)
    }

    fn start_sub(&mut self, n: u16) {
        self.sub = n;
        for ch in 0..4 {
            let v = &mut self.v[ch];
            v.cnt = 1;
            v.tp = be32(&self.d, D + 8 + 16 * n as usize + 4 * ch) as usize;
            v.stack.clear();
            v.ptab = 0;
            // volume and sample record are not reset
        }
        self.songend = false;
    }

    /// The period of table offset `t + x` (spec 3.3); `next` takes the entry after it.
    fn period(&self, ch: usize, x: u8, next: bool) -> u16 {
        let off = self.v[ch].ptab + x as i32;
        let i = CENTRE + off.div_euclid(2) + next as i32;
        PERIODS[i.clamp(0, 39) as usize]
    }

    fn pattern(&self, x: u8) -> usize {
        let pt = self.ml(0) as usize;
        (pt as i64 + be16(&self.d, D + pt + x as usize) as i16 as i64).max(0) as usize
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticked = true;
        let t0 = hw.bus.clock;
        let mut clk = Clk { t0, rel: t0.saturating_sub(self.origin), t: 247 };
        for ch in 0..4 {
            if ch > 0 {
                clk.t += 16;
            }
            self.voice(hw, &mut clk, ch);
        }
        if self.v.iter().all(|v| v.cnt & 0x8000 != 0) {
            self.songend = true;
        }
    }

    fn voice(&mut self, hw: &mut Hw, clk: &mut Clk, ch: usize) {
        if self.v[ch].cnt & 0x8000 != 0 {
            clk.t += 57;
            return;
        }
        clk.t += 84;
        self.v[ch].cnt = self.v[ch].cnt.wrapping_sub(1);
        if self.v[ch].cnt != 0 {
            return;
        }
        let bit = 1u16 << ch;
        for _ in 0..(1 << 20) {
            let tp = self.v[ch].tp;
            let b = |o: usize| byte(&self.d, D + tp + o);
            clk.t += 36;
            match b(0) {
                0x00 => {
                    let (x, dur) = (b(1), b(2));
                    self.v[ch].tp = tp + 3;
                    let per = self.period(ch, x, false);
                    self.v[ch].cnt = dur as u16;
                    clk.t += 103;
                    self.note_hw(hw, clk, ch, per);
                    return;
                }
                0x04 => {
                    let (x, c) = (b(1), b(2));
                    self.v[ch].tp = tp + 3;
                    let per = self.period(ch, x, false);
                    let vol = self.period(ch, x, true);
                    self.v[ch].cnt = c as u16;
                    clk.t += 60;
                    clk.at(hw);
                    hw.per(ch, per);
                    hw.vol(ch, vol);
                    clk.t += 66;
                    return;
                }
                0x08 => {
                    let x = b(1) as usize;
                    self.v[ch].tp = tp + 2;
                    let it = self.ml(4) as usize;
                    self.v[ch].rec = Some(it.wrapping_add(be32(&self.d, D + it.wrapping_add(x)) as usize));
                    clk.t += 74;
                }
                0x0C => {
                    let v = (b(1) as u16) * 4;
                    self.v[ch].tp = tp + 2;
                    self.v[ch].vol = v;
                    clk.t += 32;
                    clk.at(hw);
                    hw.vol(ch, v);
                    clk.t += 22;
                }
                0x10 => {
                    self.v[ch].tp = tp + 2;
                    self.v[ch].cnt = b(1) as i8 as i16 as u16;
                    clk.t += 66;
                    return;
                }
                0x14 => {
                    self.v[ch].tp = tp + 2;
                    self.v[ch].cnt = b(1) as u16;
                    clk.t += 44;
                    clk.at(hw);
                    hw.dmacon(bit);
                    clk.t += 42;
                    return;
                }
                0x18 => {
                    self.v[ch].tp = tp + 1;
                    self.v[ch].cnt = 0xFFFF;
                    clk.t += 40;
                    clk.at(hw);
                    hw.dmacon(bit);
                    clk.t += 80;
                    return;
                }
                0x1C => {
                    let x = b(1);
                    if !self.push(ch, Entry::Ptr(tp + 2)) {
                        break;
                    }
                    self.v[ch].tp = self.pattern(x);
                    clk.t += 100;
                }
                0x20 => {
                    match self.v[ch].stack.pop() {
                        Some(Entry::Ptr(p)) | Some(Entry::Loop(p, _)) => self.v[ch].tp = p,
                        None => break,
                    }
                    self.v[ch].ptab = 0;
                    clk.t += 78;
                }
                0x24 => {
                    let n = b(1) as u16;
                    self.v[ch].tp = tp + 2;
                    if !self.push(ch, Entry::Loop(tp + 2, n)) {
                        break;
                    }
                    clk.t += 74;
                }
                0x28 => {
                    self.v[ch].tp = tp + 1;
                    match self.v[ch].stack.last().copied() {
                        Some(Entry::Loop(p, c)) => {
                            let c = c.wrapping_sub(1);
                            if c == 0 {
                                self.v[ch].stack.pop();
                                clk.t += 71;
                            } else {
                                *self.v[ch].stack.last_mut().unwrap() = Entry::Loop(p, c);
                                self.v[ch].tp = p;
                                clk.t += 63;
                            }
                        }
                        // not a loop entry: nothing to count, go on
                        _ => clk.t += 71,
                    }
                }
                0x2C => {
                    let (t, x) = (b(1) as i8 as i32, b(2));
                    if !self.push(ch, Entry::Ptr(tp + 3)) {
                        break;
                    }
                    self.v[ch].tp = self.pattern(x);
                    self.v[ch].ptab = t;
                    clk.t += 152;
                }
                0x30 => {
                    self.v[ch].tp = tp + 1;
                    if !self.push(ch, Entry::Ptr(tp + 1)) {
                        break;
                    }
                    clk.t += 54;
                }
                0x34 => {
                    match self.v[ch].stack.last() {
                        Some(Entry::Ptr(p)) | Some(Entry::Loop(p, _)) => self.v[ch].tp = *p,
                        None => break,
                    }
                    clk.t += 38;
                }
                _ => break,
            }
        }
        // an invalid command or state: the voice is dead
        self.v[ch].cnt = 0xFFFF;
    }

    /// Pushes onto the voice's 40-byte stack; false if it would overflow.
    fn push(&mut self, ch: usize, e: Entry) -> bool {
        let used: usize = self.v[ch].stack.iter().map(Entry::size).sum();
        if used + e.size() > 40 {
            return false;
        }
        self.v[ch].stack.push(e);
        true
    }

    /// The hardware sequence of a note start (spec 4.2).
    fn note_hw(&mut self, hw: &mut Hw, clk: &mut Clk, ch: usize, per: u16) {
        let bit = 1u16 << ch;
        clk.at(hw);
        let t = hw.bus.clock;
        hw.bus.paula.sync(t, &hw.bus.ram);
        if hw.bus.paula.dmacon & bit != 0 {
            clk.t += 20;
            clk.at(hw);
            hw.dmacon(bit);
            clk.t += 16;
            clk.at(hw);
            hw.per(ch, 2);
            clk.t += 16;
            clk.at(hw);
            hw.vol(ch, 0);
            clk.wait(10, 46);
            clk.t += 16;
            clk.at(hw);
            hw.bus.write_word(0xDFF0AA + 16 * ch as u32, 0);
        }
        clk.t += 40;
        clk.at(hw);
        match self.v[ch].rec {
            Some(r) => {
                hw.loc(ch, self.base + (D + r) as u32 + 2);
                clk.t += 12;
                clk.at(hw);
                hw.len(ch, be16(&self.d, D + r));
            }
            None => {
                hw.loc(ch, self.silence);
                clk.t += 12;
                clk.at(hw);
                hw.len(ch, 1);
            }
        }
        clk.t += 20;
        clk.at(hw);
        hw.vol(ch, self.v[ch].vol);
        clk.t += 16;
        clk.at(hw);
        hw.per(ch, per);
        clk.t += 32;
        clk.at(hw);
        hw.dmacon(0x8000 | bit);
        clk.wait(5, 56);
        clk.t += 28;
        clk.at(hw);
        hw.loc(ch, self.silence);
        clk.t += 16;
        clk.at(hw);
        hw.len(ch, 2);
        clk.t += 42;
    }
}

impl Replayer for MarkCookseyOld {
    fn format(&self) -> &'static str {
        "Mark Cooksey Old"
    }
    fn title(&self) -> String {
        String::new()
    }
    fn tick(&mut self, hw: &mut Hw) {
        MarkCookseyOld::tick(self, hw);
    }
    fn tick_clocks(&self) -> u64 {
        TICK
    }
    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.tick_at.take()
    }
    fn songend(&self) -> bool {
        self.songend
    }
    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsub - 1, self.sub)
    }
    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        let n = n.min(self.nsub - 1);
        self.start_sub(n);
        // before the first tick each next-song call of the reference's start delays it
        if !self.ticked {
            self.tick_at = Some(self.origin + FIRST_TICK + PER_SUBSONG * n as u64);
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let nsub = header(d)?;
    let silence = m.alloc(8);
    if silence == 0 {
        return None;
    }
    // the reference's clock 0 is START_PHASE colour clocks before the engine's aligned start
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut p = MarkCookseyOld {
        d: d.to_vec(),
        base,
        silence,
        nsub,
        sub: 0,
        v: Default::default(),
        origin,
        ticked: false,
        tick_at: Some(origin + FIRST_TICK),
        songend: false,
    };
    for (v, vol) in p.v.iter_mut().zip(INIT_VOL) {
        v.vol = vol;
    }
    p.start_sub(0);
    Some(Box::new(p))
}

#[cfg(test)]
#[path = "markcookseyold_tests.rs"]
mod tests;
