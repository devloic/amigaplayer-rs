//! Beathoven Synthesizer (`bss.*`), the music system of Thomas Lopatic (1987-91) used in
//! German and British Amiga games (Zoom, Ganymed, Great Giana Sisters, Katakis, Denaris,
//! Phalanx II, Cosmic Bouncer, Rock Challenge ...) and a few demo-scene intros.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no original
//! code. The specification was drawn up from the observable behaviour of the Paula
//! register writes of UADE's Beathoven Synthesizer player and the structure of the corpus
//! files, and this player was written from it alone and checked against those writes.
//!
//! The files are hunk executables of one hunk that carry the game's own replay routine
//! with its music and samples. The routine is not run: its data tables are read where the
//! specification says they are, and what differs between the builds (three generations,
//! "100", "109" and "NEW", plus an older simple variant of 109) is a table
//! (`beathoven_tables.rs`: the voices' list pointers, tempo and note offsets, period table,
//! silent word and the sample-pointer fix-ups the games' start-up routines make, keyed by
//! the hunk size and the header's instrument-table offset) and a few flags. Four voices,
//! one Paula channel each, walk a list of sequences; a sequence is a stream of words, a
//! command (0x80 instrument, 0x81 portamento, 0x82 hold, 0x83 rest, 0x84 arpeggio, 0x85
//! transposition in one build) or a note with its duration. The Paula loop registers are
//! re-written every tick. There is no song end: the lists loop for ever.

use super::beathoven_tables::{Row, ROWS};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Beathoven Synthesizer", detect, new };

const HUNK_HEADER: u32 = 0x3F3;
const STUB: u32 = 0x70FF_4E75;
/// A list or sequence walk that finds no word to execute gives up after this many steps.
const GUARD: u32 = 64;

// ---------------------------------------------------------------- recognition

/// The generation named by the header string, and the hunk size in longs.
fn header(d: &[u8]) -> Option<(u8, usize)> {
    if d.len() < 32 + 0x4C || [0, 4, 8, 12, 16].iter().zip([HUNK_HEADER, 0, 1, 0, 0]).any(|(&o, v)| be32(d, o) != v) {
        return None;
    }
    let size = be32(d, 20);
    let n = (size & 0x3FFF_FFFF) as usize;
    if size >> 30 != 1 || be32(d, 24) != 0x3E9 || be32(d, 28) as usize != n || be32(d, 32) != STUB || d.len() < 32 + 4 * n {
        return None;
    }
    if d.get(36..44) != Some(b"BEATHOVE") {
        return None;
    }
    let generation = match d.get(44..48)? {
        b"N100" => 0,
        b"N109" => 1,
        b"NNEW" => 3,
        _ => return None,
    };
    let h = |o: usize| be32(d, 32 + o) as usize;
    if h(0x10) == 0 || h(0x14) == 0 || h(0x10) >= 4 * n || h(0x14) >= 4 * n || h(0x18) == 0 {
        return None;
    }
    Some((generation, n))
}

/// The subsong rows of the build, if the tables know it.
fn rows_of(d: &[u8]) -> Vec<&'static Row> {
    let Some((generation, n)) = header(d) else { return Vec::new() };
    let ins = be32(d, 32 + 0x30);
    let mut v: Vec<&Row> = ROWS.iter().filter(|r| r.longs as usize == n && r.ins == ins && (r.eng == 0) == (generation == 0)).collect();
    v.sort_by_key(|r| r.sub);
    v
}

fn detect(d: &[u8]) -> bool {
    let rows = rows_of(d);
    !rows.is_empty() && rows.len() == be32(d, 32 + 0x18) as usize
}

// -------------------------------------------------------------------- state

/// A voice of the register-direct engines (109, NEW and the simple variant).
#[derive(Clone, Default)]
struct V {
    list: u32,
    lp: u32,
    seq: u32,
    trans: u16,
    /// bit0 portamento, bit1 hold, bit2 two-part sample, bit3 after a rest, bit4 arpeggio.
    flags: u8,
    cnt: u16,
    tgt: u16,
    cur: u16,
    step: u16,
    /// The sample's absolute address (0 until an instrument is set), first-pass length in
    /// words, and the record's loop start, loop end and end fields.
    base: u32,
    len1: u16,
    a24: u16,
    a26: u16,
    a28: u16,
    acnt: u16,
    aptr: u32,
    aidx: u16,
    x32: u16,
}

const P: u8 = 1;
const H: u8 = 2;
const L: u8 = 4;
const R: u8 = 8;
const A: u8 = 16;

/// A voice of the request-block engine (100).
#[derive(Clone, Default)]
struct V100 {
    list: u32,
    lp: u32,
    seq: u32,
    trans: i64,
    cnt: u16,
    inst: u32,
    arp: u32,
    acnt: u16,
    aptr: u32,
    aidx: u16,
}

pub struct Beathoven {
    /// The pristine hunk image, and the image with the subsong's fix-ups applied.
    pristine: Vec<u8>,
    img: Vec<u8>,
    /// Where `img[0]` lies in guest memory.
    mem: u32,
    /// Hunk offsets of the longs the relocation hunk adjusts.
    relocs: Vec<u32>,
    rows: Vec<&'static Row>,
    title: String,
    /// The header's long at hunk offset 0x48: the tick timer (100, 109) or the sample base (NEW).
    h14: u32,
    sub: u16,
    started: bool,
    v: [V; 4],
    w: [V100; 4],
}

fn sext(v: u16) -> i32 {
    v as i16 as i32
}

impl Beathoven {
    fn row(&self) -> &'static Row {
        self.rows[self.sub as usize]
    }

    fn r16(&self, o: u32) -> u16 {
        be16(&self.img, o as usize)
    }

    fn r32(&self, o: u32) -> u32 {
        be32(&self.img, o as usize)
    }

    /// A word of the period table at hunk offset `p` (negative or beyond the image: 0). A word
    /// that is the high half of a relocated long reads as the high half of the load address,
    /// as the original's image holds it (the one case in the corpus: Ganymed, subsong 0).
    fn table_word(&self, p: i64) -> u16 {
        if p < 0 {
            return 0;
        }
        let p = p as u32;
        if self.relocs.binary_search(&p).is_ok() {
            return ((self.mem >> 16) as u16).max(1);
        }
        if p >= 2 && self.relocs.binary_search(&(p - 2)).is_ok() {
            return (self.mem.wrapping_add(self.r32(p - 2)) & 0xFFFF) as u16;
        }
        self.r16(p)
    }

    fn start(&mut self, hw: &mut Hw) {
        let row = self.row();
        let mut img = self.pristine.clone();
        for &(o, b) in row.patch {
            if let Some(dst) = img.get_mut(o as usize..o as usize + b.len()) {
                dst.copy_from_slice(b);
            }
        }
        self.img = img;
        self.started = true;
        if row.eng == 0 {
            for c in 0..row.voices as usize {
                let list = row.lists[2 * c];
                self.w[c] = V100 {
                    list,
                    lp: row.lists[2 * c + 1],
                    seq: self.r32(list),
                    trans: self.r32(list + 4) as i32 as i64,
                    cnt: 1,
                    ..Default::default()
                };
            }
        } else {
            for c in 0..4 {
                let list = row.lists[2 * c];
                self.v[c] = V {
                    list,
                    lp: row.lists[2 * c + 1],
                    seq: self.r32(list),
                    trans: self.r16(list + 6),
                    cnt: 1,
                    ..Default::default()
                };
            }
            for c in 0..4 {
                hw.vol(c, 0);
            }
            if row.init_dma {
                hw.dmacon(if row.eng == 3 && row.voices == 3 { 0x8007 } else { 0x800F });
            }
        }
    }

    /// A pointer long of the image as an address: the load address plus the offset, except the
    /// literal 0, which the relocation hunk leaves alone.
    fn abs(&self, off: u32) -> u32 {
        if off == 0 {
            0
        } else {
            self.mem.wrapping_add(off)
        }
    }

    /// The silent word's address.
    fn silent(&self) -> u32 {
        let r = self.row();
        if r.silent_abs {
            r.silent
        } else {
            self.mem.wrapping_add(r.silent)
        }
    }

    // ---------------------------------------------------- engines 109, NEW, simple

    fn voice(&mut self, ch: usize, hw: &mut Hw) {
        let row = self.row();
        let simple = row.eng == 2;
        let bit = 1u16 << ch;
        let m = row.m;
        let mut v = std::mem::take(&mut self.v[ch]);
        // A. the loop buffer
        let two_part = if simple { v.flags & L != 0 } else { v.flags & (L | R) != 0 };
        if !two_part {
            hw.loc(ch, v.base.wrapping_add(sext(v.a24) as u32));
            hw.len(ch, ((sext(v.a26) - sext(v.a24)) >> 1) as u16);
        } else {
            hw.loc(ch, self.silent());
            hw.len(ch, 1);
        }
        let mut d2 = false;
        v.cnt = v.cnt.wrapping_sub(1);
        if simple && v.cnt == 0 && v.flags & (P | H) == 0 {
            hw.dmacon(bit);
        }
        if v.cnt == 0 {
            let mut guard = 0;
            loop {
                v.flags &= !R;
                guard += 1;
                if guard > GUARD {
                    break;
                }
                if self.r16(v.seq) == 0 {
                    v.list = v.list.wrapping_add(8);
                    if self.r32(v.list) == 0 {
                        v.list = v.lp;
                    }
                    v.seq = self.r32(v.list);
                    v.trans = self.r16(v.list + 6);
                    continue;
                }
                let c = self.r16(v.seq);
                if !simple && c != 0x83 {
                    v.flags &= !A;
                    hw.dmacon(bit);
                    d2 = true;
                }
                let op = self.r16(v.seq + 2);
                match c {
                    0x80 => {
                        if simple {
                            hw.dmacon(bit);
                            d2 = true;
                        }
                        let rec = self.r32(v.seq + 2);
                        if self.r16(rec + 0xC) != 0 {
                            v.flags |= L;
                        } else {
                            v.flags &= !L;
                        }
                        hw.vol(ch, self.r16(rec));
                        let w = if v.flags & L != 0 { self.r16(rec + 2) } else { self.r16(rec + 4) } >> 1;
                        hw.len(ch, w);
                        v.len1 = w;
                        let sb = if row.eng == 3 { self.h14 } else { 0 };
                        v.base = self.abs(self.r32(rec + 8).wrapping_add(sb));
                        hw.loc(ch, v.base);
                        v.a24 = self.r16(rec + 4);
                        v.a26 = self.r16(rec + 6);
                        v.a28 = self.r16(rec + 2);
                        v.seq += 6;
                    }
                    0x81 => {
                        v.step = op;
                        if op != 0 {
                            v.flags |= P;
                            hw.dmacon(0x8000 | bit);
                        } else {
                            v.flags &= !P;
                        }
                        v.seq += 4;
                    }
                    0x82 => {
                        if op != 0 {
                            v.flags |= H;
                            hw.dmacon(0x8000 | bit);
                        } else {
                            v.flags &= !H;
                        }
                        v.seq += 4;
                    }
                    0x83 if !simple => {
                        v.flags |= R;
                        if v.flags & L == 0 {
                            hw.dmacon(bit);
                            d2 = true;
                            hw.loc(ch, v.base.wrapping_add(sext(v.a26) as u32));
                            hw.len(ch, (v.a28.wrapping_sub(v.a26)) >> 1);
                        }
                        v.cnt = op.wrapping_mul(m);
                        v.seq += 4;
                        break;
                    }
                    0x84 if !simple => {
                        let n = self.r16(v.seq + 4);
                        v.flags |= A;
                        v.cnt = op.wrapping_mul(m);
                        v.acnt = n;
                        v.aptr = v.seq + 6;
                        v.aidx = 0;
                        v.seq = v.seq.wrapping_add(6 + 2 * n as u32);
                        self.retrigger(&mut v, ch, hw);
                        break;
                    }
                    0x85 if row.cmd85 => {
                        v.x32 = op;
                        v.seq += 4;
                    }
                    _ => {
                        let n_off = row.n.wrapping_add(if row.cmd85 { v.x32 } else { 0 });
                        let idx = c.wrapping_sub(1).wrapping_add(v.trans).wrapping_add(if simple { 0 } else { n_off });
                        v.tgt = self.table_word(row.table as i64 + sext(idx << 1) as i64);
                        v.cnt = op.wrapping_mul(m);
                        v.seq += 4;
                        self.retrigger(&mut v, ch, hw);
                        break;
                    }
                }
            }
        }
        // E. the period
        let n_off = row.n.wrapping_add(if row.cmd85 { v.x32 } else { 0 });
        if v.flags & A != 0 {
            let n = self.r16(v.aptr + 2 * v.aidx as u32);
            let idx = n.wrapping_sub(1).wrapping_add(v.trans).wrapping_add(n_off);
            v.tgt = self.table_word(row.table as i64 + sext(idx << 1) as i64);
            v.aidx = v.aidx.wrapping_add(1);
            if v.aidx == v.acnt {
                v.aidx = 0;
            }
            v.cur = v.tgt;
        } else if v.flags & P != 0 {
            let lt = |a: u16, b: u16| if row.signed { (a as i16) < (b as i16) } else { a < b };
            if v.cur > v.tgt {
                let d = v.cur.wrapping_sub(v.step);
                let stop = if simple { d <= v.tgt } else { lt(d, v.tgt) };
                v.cur = if stop { v.tgt } else { d };
            } else if v.cur < v.tgt {
                let d = v.cur.wrapping_add(v.step);
                let go = if simple { d <= v.tgt } else { lt(d, v.tgt) };
                v.cur = if go { d } else { v.tgt };
            }
        } else {
            v.cur = v.tgt;
        }
        hw.per(ch, v.cur);
        // F. DMA on
        let on = if simple { v.cur & 0xFF != 0 || v.flags & (P | H) == 0 } else { d2 || v.flags & (P | H) == 0 };
        if on {
            // the routine's wait before the DMA is switched on (measured: about 1900 colour clocks)
            hw.wait(if simple { 1500 } else { 1900 });
            hw.dmacon(0x8000 | bit);
        }
        self.v[ch] = v;
    }

    /// D. After a note or an arpeggio: the sample start unless a portamento or hold is on.
    fn retrigger(&self, v: &mut V, ch: usize, hw: &mut Hw) {
        if v.flags & (P | H) == 0 {
            hw.loc(ch, v.base);
            hw.len(ch, v.len1);
        }
    }

    // --------------------------------------------------------------- engine 100

    fn off(&self, ch: usize, hw: &mut Hw) {
        hw.dmacon(1 << ch);
        // the original clears the channel's data register in a delay loop (measured: 2300 colour clocks)
        hw.wait(2300);
    }

    fn start100(&self, ch: usize, hw: &mut Hw) {
        let row = self.row();
        let rec = self.w[ch].inst;
        hw.len(ch, (self.r32(rec + 2) >> 1) as u16);
        hw.loc(ch, self.abs(self.r32(rec + 6)));
        hw.dmacon(0x8000 | 1 << ch);
        if row.post_reset {
            hw.wait(2100);
            hw.len(ch, 1);
            hw.loc(ch, self.silent());
        }
    }

    fn pv(&self, ch: usize, period: u16, hw: &mut Hw) {
        hw.vol(ch, self.r16(self.w[ch].inst));
        hw.per(ch, period);
    }

    fn period100(&self, ch: usize, n: i64) -> u16 {
        let row = self.row();
        let sum = n + self.w[ch].trans + row.d7[ch] as i64 + row.n as i64 - 1;
        self.table_word(row.table as i64 + 2 * sum)
    }

    fn voice100(&mut self, ch: usize, hw: &mut Hw) {
        let row = self.row();
        let dur = |d: u16| if row.k5 { (d >> 2).wrapping_mul(5) } else { d };
        self.w[ch].cnt = self.w[ch].cnt.wrapping_sub(1);
        if self.w[ch].cnt == 0 {
            self.w[ch].arp = 0;
            for _ in 0..GUARD {
                let seq = self.w[ch].seq;
                if self.r16(seq) == 0 {
                    let mut l = self.w[ch].list.wrapping_add(8);
                    let mut g = 0;
                    while self.r32(l) == 0 && g < 2 {
                        l = self.w[ch].lp;
                        g += 1;
                    }
                    let (sq, tr) = (self.r32(l), self.r32(l + 4) as i32 as i64);
                    let w = &mut self.w[ch];
                    w.list = l;
                    w.seq = sq;
                    w.trans = tr;
                    continue;
                }
                let c = self.r16(seq);
                match c {
                    0x80 => {
                        self.w[ch].inst = self.r32(seq + 2);
                        self.w[ch].seq += 6;
                    }
                    0x81 => {
                        let n = self.r16(seq + 4);
                        let d = dur(self.r16(seq + 2));
                        let w = &mut self.w[ch];
                        w.arp += 1;
                        w.acnt = n;
                        w.cnt = d;
                        w.aptr = seq + 6;
                        w.seq = seq.wrapping_add(2 * (n as u32 + 3));
                        self.off(ch, hw);
                        self.start100(ch, hw);
                        break;
                    }
                    0x82 => {
                        self.w[ch].cnt = dur(self.r16(seq + 2));
                        self.w[ch].seq += 4;
                        self.off(ch, hw);
                        break;
                    }
                    _ => {
                        self.off(ch, hw);
                        self.w[ch].cnt = dur(self.r16(seq + 2));
                        let per = self.period100(ch, c as i64);
                        self.start100(ch, hw);
                        self.pv(ch, per, hw);
                        self.w[ch].seq += 4;
                        break;
                    }
                }
            }
        }
        if self.w[ch].arp != 0 {
            let w = &self.w[ch];
            let n = self.r16(w.aptr + 2 * w.aidx as u32);
            let per = self.period100(ch, n as i64);
            self.pv(ch, per, hw);
            let w = &mut self.w[ch];
            w.aidx = w.aidx.wrapping_add(1);
            if w.aidx == w.acnt {
                w.aidx = 0;
            }
        }
    }
}

impl Replayer for Beathoven {
    fn format(&self) -> &'static str {
        "Beathoven Synthesizer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        if self.row().eng != 3 && self.h14 != 0 {
            (self.h14 as u64 + 1) * 5
        } else {
            super::DT_TICK
        }
    }

    fn songend(&self) -> bool {
        false
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.rows.len() as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.sub = n.min(self.rows.len() as u16 - 1);
        self.started = false;
    }

    fn tick(&mut self, hw: &mut Hw) {
        let row = self.row();
        if !self.started {
            self.start(hw);
            if row.eng != 0 {
                // the start-up routine runs on its own, one tick before the first pass over the voices
                return;
            }
        }
        if row.eng == 0 {
            if row.prologue {
                for ch in 0..4 {
                    hw.len(ch, 1);
                    hw.loc(ch, self.silent());
                }
            }
            for ch in 0..row.voices as usize {
                self.voice100(ch, hw);
            }
        } else {
            for ch in 0..row.voices as usize {
                if ch == 3 && !row.voice3 {
                    continue;
                }
                self.voice(ch, hw);
            }
        }
    }
}

/// The offsets of the longs the relocation hunk of the file adjusts (sorted).
fn relocations(d: &[u8], n: usize) -> Vec<u32> {
    let mut v = Vec::new();
    let mut p = 32 + 4 * n;
    if be32(d, p) != 0x3EC {
        return v;
    }
    p += 4;
    loop {
        let count = be32(d, p) as usize;
        if count == 0 || p + 8 + 4 * count > d.len() {
            break;
        }
        v.extend((0..count).map(|i| be32(d, p + 8 + 4 * i)));
        p += 8 + 4 * count;
    }
    v.sort_unstable();
    v
}

fn new(d: &[u8], base: u32, _m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let (_, n) = header(d)?;
    let rows = rows_of(d);
    let img = d[32..32 + 4 * n].to_vec();
    let name = be32(&img, 0x24) as usize;
    let title = img.get(name..).map_or(String::new(), |s| s.iter().take_while(|&&b| b != 0).take(80).map(|&b| b as char).collect());
    Some(Box::new(Beathoven {
        h14: be32(&img, 0x48),
        pristine: img.clone(),
        img,
        mem: base + 32,
        relocs: relocations(d, n),
        rows,
        title,
        sub: 0,
        started: false,
        v: Default::default(),
        w: Default::default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn p16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn p32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// Words at hunk offset `o` (the hunk starts at file offset 32).
    fn words(d: &mut [u8], o: usize, w: &[u16]) {
        for (i, &v) in w.iter().enumerate() {
            p16(d, 32 + o + 2 * i, v);
        }
    }

    /// A file of the build the table row for a hunk of `longs` longs names, with the
    /// header string `hdr` ("N109"); the hunk holds the header and nothing else.
    fn file(longs: u32, hdr: &[u8; 4]) -> Vec<u8> {
        let row = ROWS.iter().find(|r| r.longs == longs).unwrap();
        let mut d = vec![0u8; 32 + 4 * longs as usize + 4];
        for (i, v) in [HUNK_HEADER, 0, 1, 0, 0, 0x4000_0000 | longs, 0x3E9, longs].into_iter().enumerate() {
            p32(&mut d, 4 * i, v);
        }
        p32(&mut d, 32, STUB);
        d[36..44].copy_from_slice(b"BEATHOVE");
        d[44..48].copy_from_slice(hdr);
        p32(&mut d, 32 + 0x10, 0x100);
        p32(&mut d, 32 + 0x14, 0x200);
        p32(&mut d, 32 + 0x18, 1);
        p32(&mut d, 32 + 0x30, row.ins);
        let end = 32 + 4 * longs as usize;
        p32(&mut d, end, 0x3F2);
        d
    }

    /// Denaris' build (109): voice 0 plays an instrument, a note of 4 steps with transposition
    /// 2, and a rest; the other voices rest.
    fn module109() -> Vec<u8> {
        let mut d = file(65323, b"N109");
        words(&mut d, 0x5fe, &[500, 480, 460, 440, 420, 400, 380, 360, 340, 320, 300, 280, 260, 240]);
        // lists: voice 0 (list 0x7f4, loop 0x884), the others
        for l in [0x7f4, 0x884] {
            p32(&mut d, 32 + l, 0x1000);
            p16(&mut d, 32 + l + 6, 2);
        }
        for l in [0xb6c, 0xb8c, 0xbe4, 0xbfc, 0xc24, 0xc2c] {
            p32(&mut d, 32 + l, 0x1100);
        }
        // instrument 0x900, note 5 for 4 steps, rest for 2, end of sequence
        words(&mut d, 0x1000, &[0x80, 0, 0x900, 5, 4, 0x83, 2, 0]);
        words(&mut d, 0x1100, &[0x83, 100, 0]);
        // the record: volume, end, loop start, loop end, sample address, one-part sample
        words(&mut d, 0x900, &[0x20, 0x100, 0x20, 0x80, 0, 0x2000, 0, 0]);
        d
    }

    /// Detonator title's build (100), voice 0: instrument, a note, an arpeggio over two notes.
    fn module100() -> Vec<u8> {
        let mut d = file(5466, b"N100");
        words(&mut d, 0x2e4, &[400, 380, 360, 340, 320, 300]);
        for (l, s, t) in [(0x678, 0x1000, 1), (0x680, 0x1000, 1), (0x690, 0x1100, 0), (0x698, 0x1100, 0), (0x7a0, 0x1100, 0), (0x7a8, 0x1100, 0), (0x7b8, 0x1100, 0)] {
            p32(&mut d, 32 + l, s);
            p32(&mut d, 32 + l + 4, t);
        }
        // instrument 0x900, note 3 for 2 ticks, arpeggio of 2 ticks over notes 1 and 2
        words(&mut d, 0x1000, &[0x80, 0, 0x900, 3, 2, 0x81, 2, 2, 1, 2, 0]);
        words(&mut d, 0x1100, &[0x82, 1000, 0]);
        // the record: volume, length in bytes (long), sample address (long)
        words(&mut d, 0x900, &[0x30, 0, 0x40, 0, 0, 0, 0x3000]);
        d
    }

    fn run(data: Vec<u8>, ticks: usize) -> Vec<(u32, u16)> {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/bss-test/bss.test"), data).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Beathoven Synthesizer");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect()
    }

    /// Channel 0's writes: (register, value), LC as one long, DMACON as written.
    fn ch0(log: &[(u32, u16)]) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        let mut hi = 0u32;
        for &(a, v) in log {
            match a {
                0xDFF0A0 => hi = v as u32,
                0xDFF0A2 => out.push((0xA0, hi << 16 | v as u32)),
                0xDFF0A4 | 0xDFF0A6 | 0xDFF0A8 => out.push((a - 0xDFF000, v as u32)),
                0xDFF096 if v & 0x7FFF == 1 => out.push((0x96, v as u32)),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn recognises_by_content() {
        assert!(detect(&module109()));
        assert!(detect(&module100()));
        // the header string must be the build's generation
        let mut d = module109();
        d[44..48].copy_from_slice(b"N100");
        assert!(!detect(&d));
        let mut d = module109();
        d[36] = b'X';
        assert!(!detect(&d));
        // a build the tables do not know, a second hunk, a short file
        let mut d = module109();
        p32(&mut d, 32 + 0x30, 0x1234);
        assert!(!detect(&d));
        let mut d = module109();
        p32(&mut d, 8, 2);
        assert!(!detect(&d));
        assert!(!detect(&module109()[..40]));
        assert!(!detect(b"FORM\0\0\0\0EMODEMIC"));
    }

    #[test]
    fn rows_are_unique_and_complete() {
        for r in ROWS {
            let mut subs: Vec<u8> = ROWS.iter().filter(|o| o.longs == r.longs && o.ins == r.ins).map(|o| o.sub).collect();
            subs.sort_unstable();
            assert_eq!(subs, (0..subs.len() as u8).collect::<Vec<_>>(), "{}", r.longs);
        }
    }

    #[test]
    fn engine_109_plays_a_note_and_rests() {
        let w = ch0(&run(module109(), 20));
        // the start-up routine: volume 0 on channel 0
        assert_eq!(w[0], (0xA8, 0));
        // the instrument's volume, length and address
        let i = w.iter().position(|&(r, v)| r == 0xA8 && v == 0x20).unwrap();
        assert_eq!(w[i + 1], (0xA4, 0x10));
        assert_eq!(w[i + 2].0, 0xA0);
        let base = w[i + 2].1;
        // the note: index 5 - 1 + 2 + 7 = 13
        assert!(w.contains(&(0xA6, 240)));
        // the next tick re-programs the loop region: base + 0x20, 0x30 words
        let j = w.iter().position(|&x| x == (0xA0, base + 0x20)).unwrap();
        assert_eq!(w[j + 1], (0xA4, 0x30));
        // the rest: DMA off, then the tail [0x80, 0x100)
        let k = w.iter().position(|&x| x == (0xA0, base + 0x80)).unwrap();
        assert_eq!(w[k + 1], (0xA4, 0x40));
        assert_eq!(w[k - 1], (0x96, 1));
    }

    #[test]
    fn engine_100_plays_a_note_with_post_reset_and_arpeggio() {
        let w = ch0(&run(module100(), 8));
        // off, length 0x20 words, address, on, then the post-reset, volume, period (index 3)
        assert_eq!(w[0], (0x96, 1));
        assert_eq!(w[1], (0xA4, 0x20));
        let a = w[2].1;
        assert_eq!(w[3], (0x96, 0x8001));
        assert_eq!(w[4], (0xA4, 1));
        assert_ne!(w[5].1, a);
        assert_eq!(w[6], (0xA8, 0x30));
        assert_eq!(w[7], (0xA6, 340));
        // the note lasts 2 ticks; the arpeggio then steps through the periods of notes 1 and 2,
        // and the list loops to the note
        let pers: Vec<u32> = w.iter().filter(|x| x.0 == 0xA6).map(|x| x.1).collect();
        assert_eq!(pers[..4], [340, 380, 360, 340]);
    }

    #[test]
    fn tick_rate_follows_the_timer_header() {
        let mut m = Engine::new(48000.0).m;
        let mut d = module100();
        assert_eq!(new(&d, 0x10000, &mut m).unwrap().tick_clocks(), super::super::DT_TICK);
        p32(&mut d, 32 + 0x48, 0x3A00);
        assert_eq!(new(&d, 0x10000, &mut m).unwrap().tick_clocks(), 0x3A01 * 5);
    }

    #[test]
    fn an_index_in_a_relocated_pointer_reads_the_load_address() {
        let mut d = module109();
        let n = 65323usize;
        // a relocation hunk with one entry: the long at hunk offset 0x700
        let end = 32 + 4 * n;
        d.truncate(end);
        for v in [0x3ECu32, 1, 0, 0x700, 0, 0x3F2] {
            d.extend_from_slice(&v.to_be_bytes());
        }
        assert_eq!(relocations(&d, n), vec![0x700]);
        let b = Beathoven { pristine: Vec::new(), img: d[32..end].to_vec(), mem: 0x2_0000 + 32, relocs: vec![0x700], rows: rows_of(&d), title: String::new(), h14: 0, sub: 0, started: true, v: Default::default(), w: Default::default() };
        assert_eq!(b.table_word(0x700), 2);
        assert_eq!(b.table_word(-4), 0);
        assert_eq!(b.table_word(0x5fe), 500);
        // a load address below 0x10000 reads 1, the value of the one observed case
        let low = Beathoven { mem: 0x8000, ..b };
        assert_eq!(low.table_word(0x700), 1);
    }
}
