//! Darius Zendeh sound system ("old Mark II"; Eagleplayer "Darius Zendeh", prefixes `dz` and
//! `mkiio`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player; no original code. A module is a plain 68k program image that
//! carries its own tiny driver with its data. This replayer never runs that code: it finds the
//! module's tables from marker words in the image (record size, sample table, sample data, period
//! table, record table, song table) and interprets them as the specification describes. Three
//! variants exist: A (older, 4-byte sample entries), B (8-byte entries) and C (B plus a period
//! slide feature and 32 steps per row). One song of four voices; a row tick (every second 50 Hz
//! call) writes all four channels and starts DMA, the tick in between stops the voices marked as
//! retriggering. Register writes are placed at the clocks the specification measured; the +-1
//! (A, B) and +-2 (C) colour clock jitter of the original is not modelled (C's fractional gap
//! means are kept in half colour clocks).
//!
//! The Mark II driver (prefixes `mk2`, `mkii`) is a different driver and is not covered. The
//! fade and sound-effect commands of the driver are not used by the player and are not
//! implemented.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): reads outside
//! the file give zero; the table indexes (sample number, period index) are not range checked
//! beyond that; a module whose tables lie outside the file, whose record 0 or song table has no
//! end marker inside the file is not recognised; the song restart costs 61 colour clocks in all (the row change's 22 included, as measured for C) for
//! every variant (only C was seen restarting); the sub-song kind (word 0x4A44 at offset
//! 12) plays song 0 like the others.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Darius Zendeh", detect, new };

/// Colour clocks between two timer calls.
const TICK: u64 = 70940;
/// Where the run's detection scan starts, in the reference's log clock.
const DETECT_AT: u64 = 100;
/// The string the detection of B and C scans for (offset 0 to 0x240).
const SCAN: [u8; 12] = [0x70, 0x00, 0x33, 0xFC, 0x00, 0x0F, 0x00, 0xDF, 0xF0, 0x96, 0x41, 0xFA];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Variant {
    A,
    B,
    C,
}

/// The located tables of a module (offsets in the file).
#[derive(Clone, Copy, Debug)]
struct Layout {
    var: Variant,
    rs: usize,
    esz: usize,
    samples: usize,
    data: usize,
    periods: usize,
    records: usize,
    song: usize,
    steps: usize,
    /// Offset of the 12-byte detection string (B and C).
    scan: usize,
}

fn bytes(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The target of an address constant whose opcode word sits at `o`.
fn target(d: &[u8], o: usize) -> usize {
    (o as i64 + 2 + be16(d, o + 2) as i16 as i64).max(0) as usize
}

/// The first even offset from `from` below `to` where the word `w` stands.
fn find_word(d: &[u8], from: usize, to: usize, w: u16) -> Option<usize> {
    let to = to.min(d.len().saturating_sub(1));
    (from..to).step_by(2).find(|&o| be16(d, o) == w)
}

fn locate(d: &[u8]) -> Option<Layout> {
    if d.len() < 0x40 || d.len() > 0x100_0000 {
        return None;
    }
    let first = be32(d, 0);
    let mut var;
    let mut scan = 0;
    if first == 0x48E7_78F0 {
        if be16(d, 4) != 0x41FA || be32(d, 8) != 0x4CD8_0600 || be32(d, 12) != 0x0C00_00FF {
            return None;
        }
        var = Variant::A;
    } else if first == 0x48E7_00F0 {
        if be32(d, 8) != 0x4CD8_0600 || !matches!(be16(d, 12), 0x4A44 | 0x4A00 | 0x0C00) {
            return None;
        }
        scan = (0..289).map(|i| 2 * i).find(|&o| d.get(o..o + 12) == Some(&SCAN[..]))?;
        var = Variant::B;
    } else {
        return None;
    }
    // record size marker
    let mk = find_word(d, 0x10, d.len(), 0xC0FC)?;
    let rs = be16(d, mk + 2) as usize;
    if rs == 0 || rs > 0x1000 {
        return None;
    }
    let after = mk + 4;
    let kind = (after..(after + 0x80).min(d.len())).step_by(2).map(|o| be16(d, o)).find(|&w| w == 0xE742 || w == 0xE542)?;
    let esz = if kind == 0xE742 { 8 } else { 4 };
    if (var == Variant::A) != (esz == 4) {
        return None;
    }
    // the first three address constants after the marker
    let lim = (after + 0x400).min(d.len());
    let mut consts = [0usize; 3];
    let mut o = after;
    for c in consts.iter_mut() {
        o = find_word(d, o, lim, 0x41FA)?;
        *c = target(d, o);
        o += 4;
    }
    // the record table: the nearest 45FA before the marker
    let rt = (0x10..mk).rev().find(|&o| o % 2 == 0 && be16(d, o) == 0x45FA)?;
    let records = target(d, rt);
    let song;
    if var == Variant::A {
        let st = (0x10..mk).rev().find(|&o| o % 2 == 0 && be16(d, o) == 0x43FA)?;
        song = target(d, st);
    } else {
        let ff = (0x10..d.len().saturating_sub(4)).step_by(2).find(|&o| be32(d, o) == 0x0C00_00FF)?;
        let c = find_word(d, ff, d.len(), 0x41FA)?;
        song = target(d, c);
        // C has the slide feature near the record size marker
        if (mk..mk + 0x60).step_by(2).any(|o| be32(d, o) == 0x0883_0007) {
            var = Variant::C;
        }
    }
    // the record 0 list and the song table must end inside the file
    let mut steps = 0;
    loop {
        let o = records + 4 * steps;
        if o >= d.len() {
            return None;
        }
        if d[o] == 0xFF {
            break;
        }
        steps += 1;
        if steps > 4096 {
            return None;
        }
    }
    if steps == 0 {
        return None;
    }
    let mut r = 0;
    loop {
        let o = song + 8 * r;
        if o >= d.len() {
            return None;
        }
        if d[o] == 0xFF {
            break;
        }
        r += 1;
    }
    if r == 0 || consts.iter().any(|&c| c >= d.len()) {
        return None;
    }
    Some(Layout { var, rs, esz, samples: consts[0], data: consts[1], periods: consts[2], records, song, steps, scan })
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

pub struct DariusZendeh {
    d: Vec<u8>,
    base: u32,
    l: Layout,
    row: usize,
    step: usize,
    prev: [u16; 4],
    /// DMA stop mask for the next (gate) tick.
    mask: u16,
    gate: bool,
}

/// Gaps between writes, in half colour clocks.
struct Gaps {
    len_lc: u64,
    lc_per: u64,
    per_vol: u64,
    vol_next: u64,
    last: u64,
}

impl DariusZendeh {
    fn rec_entry(&self, rec: u8, k: usize) -> [u8; 4] {
        let o = self.l.records + rec as usize * self.l.rs + 4 * k;
        [bytes(&self.d, o), bytes(&self.d, o + 1), bytes(&self.d, o + 2), bytes(&self.d, o + 3)]
    }

    fn song_byte(&self, row: usize, i: usize) -> u8 {
        bytes(&self.d, self.l.song + 8 * row + i)
    }

    fn gaps(&self) -> Gaps {
        match self.l.var {
            Variant::A => Gaps { len_lc: 42, lc_per: 42, per_vol: 74, vol_next: 232, last: 106 },
            Variant::B => Gaps { len_lc: 46, lc_per: 42, per_vol: 72, vol_next: 236, last: 116 },
            Variant::C => Gaps { len_lc: 46, lc_per: 121, per_vol: 81, vol_next: 237, last: 117 },
        }
    }

    /// Writes the four channels of the current row and the closing DMACON, the first LEN write
    /// at half-clock offset `p` from `t0`. Returns the offset after the DMACON write.
    fn write_row(&mut self, hw: &mut Hw, t0: u64, mut p: u64) -> u64 {
        let g = self.gaps();
        let at = |hw: &mut Hw, p: u64| hw.bus.clock = t0 + p / 2;
        let c_var = self.l.var == Variant::C;
        let mut mask = 0u16;
        for c in 0..4 {
            let rec = self.song_byte(self.row, 2 * c);
            let note = self.song_byte(self.row, 2 * c + 1) as usize;
            let [s, base, vol, flag] = self.rec_entry(rec, self.step);
            let st = self.l.samples + self.l.esz * s as usize;
            let (off, len) = if self.l.esz == 4 { (be16(&self.d, st) as u32, be16(&self.d, st + 2)) } else { (be32(&self.d, st), be16(&self.d, st + 6)) };
            let ptr = self.base.wrapping_add(self.l.data as u32).wrapping_add(off);
            let mut slide_gap = g.lc_per;
            let per = if c_var && base & 0x80 != 0 {
                let amount = (base & 0x7F) as u16;
                if flag & 0x80 != 0 {
                    slide_gap = 200;
                    self.prev[c].wrapping_add(amount)
                } else {
                    slide_gap = 196;
                    self.prev[c].wrapping_sub(amount)
                }
            } else {
                be16(&self.d, self.l.periods + 2 * (base as usize + note))
            };
            if c_var {
                self.prev[c] = per;
            }
            let v = if vol > 0 { vol as u16 } else { 1 };
            let retrig = if c_var { flag & 1 == 0 } else { flag == 0 };
            if retrig {
                mask |= 1 << c;
            }
            at(hw, p);
            hw.len(c, len);
            p += g.len_lc;
            at(hw, p);
            hw.loc(c, ptr);
            p += slide_gap;
            at(hw, p);
            hw.per(c, per);
            p += g.per_vol + if vol == 0 { 14 } else { 0 };
            at(hw, p);
            hw.vol(c, v);
            let r = if retrig { 12 } else { 0 };
            p += if c == 3 { g.last } else { g.vol_next } + r;
        }
        at(hw, p);
        hw.dmacon(0x820F);
        self.mask = mask;
        p
    }

    /// Moves to the next step; returns the extra half clocks of a row change and a restart.
    fn advance(&mut self) -> u64 {
        let mut extra = 0;
        self.step += 1;
        if self.rec_entry(0, self.step)[0] == 0xFF || self.step >= self.l.steps {
            self.step = 0;
            self.row += 1;
            extra += if self.l.var == Variant::A { 38 } else { 44 };
            if self.song_byte(self.row, 0) == 0xFF {
                self.row = 0;
                extra += 78; // a restart costs 61 in all, 39 more than a row change
            }
        }
        extra
    }
}

impl Replayer for DariusZendeh {
    fn format(&self) -> &'static str {
        "Darius Zendeh"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        false
    }

    fn tick(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        if self.gate {
            self.gate = false;
            hw.bus.clock = t0 + if self.l.var == Variant::A { 238 } else { 253 };
            hw.dmacon(self.mask);
        } else {
            self.gate = true;
            let extra = self.advance();
            let first = if self.l.var == Variant::A { 341 } else { 366 };
            self.write_row(hw, t0, 2 * first + extra);
        }
    }
}

/// The start-up call's start in the reference's log clock: the host's detection scan costs
/// 23.5 colour clocks per word of the string's offset (spec 6).
fn call_start(l: &Layout) -> u64 {
    if l.var == Variant::A {
        268
    } else {
        DETECT_AT + 203 + 47 * (l.scan as u64 / 2) / 2 + 71
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = locate(d)?;
    let mut r = DariusZendeh { d: d.to_vec(), base, l, row: 0, step: 0, prev: [0; 4], mask: 0, gate: true };
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let s = origin + call_start(&l);
    let mut hw = Hw { bus: &mut m.bus };
    let end;
    if l.var == Variant::A {
        let p = r.write_row(&mut hw, s, 2 * 342);
        end = s + p / 2 + 164;
    } else {
        let first = if l.var == Variant::C { 177 } else { 167 };
        hw.bus.clock = s + first;
        hw.dmacon(0x000F);
        let t1 = s + first;
        let p = r.write_row(&mut hw, t1, 2 * 212);
        let mut t = t1 + p / 2 + 101;
        for c in 0..4 {
            hw.bus.clock = t;
            hw.vol(c, 0);
            t += 6;
        }
        t += 2;
        hw.bus.clock = t;
        hw.dmacon(0x000F);
        end = t + 18;
    }
    // the host's timer starts when the start-up call returns; the first tick follows one period later
    m.bus.clock = end.max(m.bus.clock);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "dariuszendeh_tests.rs"]
mod tests;
