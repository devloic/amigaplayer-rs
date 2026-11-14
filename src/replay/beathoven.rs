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
    /// Engine 109: the writes are timed by the original's code paths (`BASE` and the build's `BUILDS` entry); the
    /// half-colour-clock cursor and the voice the paths are on.
    timed: bool,
    cy: i64,
    vk: usize,
    costs: [[i32; 4]; NK],
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
            if row.eng == 1 {
                // the original's InitSound: the volumes off twice, every voice on the silent word
                // with a length and period of 1, the volumes off again; then the DMA (the waits
                // are measured in the reference runs)
                for (c, w) in [0u64, 154, 166, 178].into_iter().enumerate() {
                    hw.wait(w);
                    hw.vol(c, 0);
                }
                hw.wait(266);
                for (c, w) in [0u64, 154, 166, 178].into_iter().enumerate() {
                    hw.wait(w);
                    hw.vol(c, 0);
                }
                hw.wait(609);
                for c in 0..4 {
                    hw.loc(c, self.silent());
                    hw.wait(8);
                }
                hw.wait(6);
                for c in 0..4 {
                    hw.len(c, 1);
                    hw.per(c, 1);
                    hw.wait(8);
                }
                hw.wait(241);
                for (c, w) in [0u64, 129, 129, 120].into_iter().enumerate() {
                    hw.wait(w);
                    hw.vol(c, 0);
                }
                hw.wait(48);
            } else {
                for c in 0..4 {
                    hw.vol(c, 0);
                }
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

    /// A path of the original's code taken (engine 109): its cycles, on the current voice.
    fn k(&mut self, k: usize) {
        if self.timed {
            self.cy += self.costs[k][self.vk] as i64;
        }
    }
    /// The colour clock the original is at: the clock of the half-colour-clock cursor.
    fn sync(&mut self, hw: &mut Hw) {
        if self.timed {
            let t = self.cy.div_euclid(2) as u64;
            if hw.bus.clock < t {
                hw.bus.clock = t;
            }
        }
    }
    /// A Paula write of the path `k`.
    fn at(&mut self, k: usize, hw: &mut Hw) {
        self.k(k);
        self.sync(hw);
    }
    /// The wait for the beam before the DMA is switched on (engine 109): the line is read
    /// `K_BREAD` cycles in, then the beam is polled every `K_BPOLL` cycles from `K_BFIRST` on,
    /// until it is `K_BLINES` lines on, `K_BPOS` half colour clocks into that line.
    fn beam_wait(&mut self, b: [usize; 6], hw: &mut Hw) {
        if !self.timed {
            hw.wait_lines(10, 64);
            return;
        }
        let s = self.cy;
        let [k_read, k_lines, k_pos, k_first, k_poll, k_delay] = b;
        let c = |k: usize| self.costs[k][0] as i64;
        if c(k_poll) == 0 {
            // a build that waits in a delay loop
            self.cy = s + c(k_delay);
            return;
        }
        let l0 = (s + c(k_read)).div_euclid(2) as u64 / crate::paula::LINE;
        let target = ((l0 + c(k_lines) as u64) * crate::paula::LINE * 2) as i64 + c(k_pos);
        let mut p = s + c(k_first);
        while p < target {
            p += c(k_poll);
        }
        self.cy = p;
    }

    fn voice(&mut self, ch: usize, hw: &mut Hw) {
        let row = self.row();
        let simple = row.eng == 2;
        let bit = 1u16 << ch;
        let m = row.m;
        self.vk = ch;
        self.k(K_V);
        let mut v = std::mem::take(&mut self.v[ch]);
        // A. the loop buffer
        let two_part = if simple { v.flags & L != 0 } else { v.flags & (L | R) != 0 };
        if !two_part {
            self.at(K_ALOC, hw);
            hw.loc(ch, v.base.wrapping_add(sext(v.a24) as u32));
            self.at(K_ALEN, hw);
            hw.len(ch, ((sext(v.a26) - sext(v.a24)) >> 1) as u16);
            self.k(K_ALENX);
        } else {
            self.at(K_A2LOC, hw);
            hw.loc(ch, self.silent());
            self.at(K_A2LEN, hw);
            hw.len(ch, 1);
            self.k(K_A2LENX);
        }
        let mut d2 = false;
        v.cnt = v.cnt.wrapping_sub(1);
        if simple && v.cnt == 0 && v.flags & (P | H) == 0 {
            hw.dmacon(bit);
        }
        if v.cnt == 0 {
            self.k(K_CZ);
            let mut guard = 0;
            loop {
                v.flags &= !R;
                guard += 1;
                if guard > GUARD {
                    break;
                }
                self.k(K_IT);
                if self.r16(v.seq) == 0 {
                    self.k(K_SEND);
                    v.list = v.list.wrapping_add(8);
                    if self.r32(v.list) == 0 {
                        self.k(K_LWRAP);
                        v.list = v.lp;
                    }
                    v.seq = self.r32(v.list);
                    v.trans = self.r16(v.list + 6);
                    continue;
                }
                let c = self.r16(v.seq);
                if !simple && c != 0x83 {
                    v.flags &= !A;
                    self.at(K_OFF, hw);
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
                            self.k(K_IL);
                            v.flags |= L;
                        } else {
                            self.k(K_INL);
                            v.flags &= !L;
                        }
                        self.at(K_IVOL, hw);
                        hw.vol(ch, self.r16(rec));
                        let w = if v.flags & L != 0 { self.r16(rec + 2) } else { self.r16(rec + 4) } >> 1;
                        self.k(if v.flags & L != 0 { K_ILENL } else { K_ILENNL });
                        self.at(K_ILEN, hw);
                        hw.len(ch, w);
                        v.len1 = w;
                        let sb = if row.eng == 3 { self.h14 } else { 0 };
                        v.base = self.abs(self.r32(rec + 8).wrapping_add(sb));
                        self.at(K_ILOC, hw);
                        hw.loc(ch, v.base);
                        v.a24 = self.r16(rec + 4);
                        v.a26 = self.r16(rec + 6);
                        v.a28 = self.r16(rec + 2);
                        v.seq += 6;
                        self.k(K_IEND);
                    }
                    0x81 => {
                        self.k(K_81);
                        v.step = op;
                        if op != 0 {
                            v.flags |= P;
                            self.at(K_81ON, hw);
                            hw.dmacon(0x8000 | bit);
                        } else {
                            self.k(K_81OFF);
                            v.flags &= !P;
                        }
                        v.seq += 4;
                        self.k(K_81END);
                    }
                    0x82 => {
                        self.k(K_82);
                        if op != 0 {
                            v.flags |= H;
                            self.at(K_82ON, hw);
                            hw.dmacon(0x8000 | bit);
                        } else {
                            self.k(K_82OFF);
                            v.flags &= !H;
                        }
                        v.seq += 4;
                        self.k(K_82END);
                    }
                    0x83 if !simple => {
                        self.k(K_83);
                        v.flags |= R;
                        if v.flags & L == 0 {
                            self.at(K_83OFF, hw);
                            hw.dmacon(bit);
                            d2 = true;
                            self.at(K_83LOC, hw);
                            hw.loc(ch, v.base.wrapping_add(sext(v.a26) as u32));
                            self.at(K_83LEN, hw);
                            hw.len(ch, (v.a28.wrapping_sub(v.a26)) >> 1);
                        } else {
                            self.k(K_83L);
                        }
                        v.cnt = op.wrapping_mul(m);
                        v.seq += 4;
                        self.k(K_83END);
                        break;
                    }
                    0x84 if !simple => {
                        self.k(K_84);
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
                        self.k(K_85);
                        v.x32 = op;
                        v.seq += 4;
                    }
                    _ => {
                        self.k(K_NOTE);
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
        } else {
            self.k(K_CNZ);
        }
        // E. the period
        let n_off = row.n.wrapping_add(if row.cmd85 { v.x32 } else { 0 });
        if v.flags & A != 0 {
            self.k(K_EA);
            let n = self.r16(v.aptr + 2 * v.aidx as u32);
            let idx = n.wrapping_sub(1).wrapping_add(v.trans).wrapping_add(n_off);
            v.tgt = self.table_word(row.table as i64 + sext(idx << 1) as i64);
            v.aidx = v.aidx.wrapping_add(1);
            if v.aidx == v.acnt {
                self.k(K_EAW);
                v.aidx = 0;
            }
            v.cur = v.tgt;
        } else if v.flags & P != 0 {
            self.k(K_EP);
            let lt = |a: u16, b: u16| if row.signed { (a as i16) < (b as i16) } else { a < b };
            if v.cur > v.tgt {
                self.k(K_EPDN);
                let d = v.cur.wrapping_sub(v.step);
                let stop = if simple { d <= v.tgt } else { lt(d, v.tgt) };
                if stop {
                    self.k(K_EPDNS);
                }
                v.cur = if stop { v.tgt } else { d };
            } else if v.cur < v.tgt {
                self.k(K_EPUP);
                let d = v.cur.wrapping_add(v.step);
                let go = if simple { d <= v.tgt } else { lt(d, v.tgt) };
                if !go {
                    self.k(K_EPUPS);
                }
                v.cur = if go { d } else { v.tgt };
            } else {
                self.k(K_EPEQ);
            }
        } else {
            self.k(K_EN);
            v.cur = v.tgt;
        }
        self.at(K_PER, hw);
        hw.per(ch, v.cur);
        // F. DMA on
        let on = if simple { v.cur & 0xFF != 0 || v.flags & (P | H) == 0 } else { d2 || v.flags & (P | H) == 0 };
        if !simple {
            self.k(if d2 { K_FD2 } else { K_FFL });
        }
        if on {
            // the routine's wait before the DMA is switched on (measured: about 1900 colour clocks)
            if simple {
                hw.wait(1500);
            } else {
                self.beam_wait(B1, hw);
            }
            self.at(K_ON, hw);
            hw.dmacon(0x8000 | bit);
        } else {
            self.k(K_NOON);
        }
        self.v[ch] = v;
    }

    /// D. After a note or an arpeggio: the sample start unless a portamento or hold is on.
    fn retrigger(&mut self, v: &mut V, ch: usize, hw: &mut Hw) {
        if v.flags & (P | H) == 0 {
            self.at(K_RLOC, hw);
            hw.loc(ch, v.base);
            self.at(K_RLEN, hw);
            hw.len(ch, v.len1);
        } else {
            self.k(K_RNO);
        }
    }

    // --------------------------------------------------------------- engine 100

    fn off(&mut self, ch: usize, hw: &mut Hw) {
        self.at(K0_OFF, hw);
        hw.dmacon(1 << ch);
        if self.timed {
            // the original clears the channel's data register, then waits for the beam
            self.beam_wait(B0, hw);
        } else {
            // the original clears the channel's data register in a delay loop (measured: 2300 colour clocks)
            hw.wait(2300);
        }
        self.k(K0_OFFX);
    }

    fn start100(&mut self, ch: usize, hw: &mut Hw) {
        let row = self.row();
        let rec = self.w[ch].inst;
        self.at(K0_SLEN, hw);
        hw.len(ch, (self.r32(rec + 2) >> 1) as u16);
        self.at(K0_SLOC, hw);
        hw.loc(ch, self.abs(self.r32(rec + 6)));
        self.at(K0_SON, hw);
        hw.dmacon(0x8000 | 1 << ch);
        if row.post_reset {
            if self.timed {
                self.k(K0_PDELAY);
            } else {
                hw.wait(2100);
            }
            self.at(K0_PLEN, hw);
            hw.len(ch, 1);
            self.at(K0_PLOC, hw);
            hw.loc(ch, self.silent());
        } else {
            self.k(K0_NOPR);
        }
    }

    fn pv(&mut self, ch: usize, period: u16, hw: &mut Hw) {
        self.at(K0_PVOL, hw);
        hw.vol(ch, self.r16(self.w[ch].inst));
        self.at(K0_PPER, hw);
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
        self.vk = ch;
        self.k(K0_V);
        self.w[ch].cnt = self.w[ch].cnt.wrapping_sub(1);
        if self.w[ch].cnt == 0 {
            self.k(K0_CZ);
            self.w[ch].arp = 0;
            for _ in 0..GUARD {
                self.k(K0_IT);
                let seq = self.w[ch].seq;
                if self.r16(seq) == 0 {
                    self.k(K0_SEND);
                    let mut l = self.w[ch].list.wrapping_add(8);
                    let mut g = 0;
                    while self.r32(l) == 0 && g < 2 {
                        self.k(K0_LWRAP);
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
                        self.k(K0_80);
                        self.w[ch].inst = self.r32(seq + 2);
                        self.w[ch].seq += 6;
                    }
                    0x81 => {
                        self.k(K0_81);
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
                        self.k(K0_82);
                        self.w[ch].cnt = dur(self.r16(seq + 2));
                        self.w[ch].seq += 4;
                        self.off(ch, hw);
                        self.k(K0_82X);
                        break;
                    }
                    _ => {
                        self.k(K0_NOTE);
                        self.off(ch, hw);
                        self.w[ch].cnt = dur(self.r16(seq + 2));
                        let per = self.period100(ch, c as i64);
                        self.start100(ch, hw);
                        self.pv(ch, per, hw);
                        self.w[ch].seq += 4;
                        self.k(K0_NOTEX);
                        break;
                    }
                }
            }
        } else {
            self.k(K0_CNZ);
        }
        if self.w[ch].arp != 0 {
            self.k(K0_ARP);
            let w = &self.w[ch];
            let n = self.r16(w.aptr + 2 * w.aidx as u32);
            let per = self.period100(ch, n as i64);
            self.pv(ch, per, hw);
            let w = &mut self.w[ch];
            w.aidx = w.aidx.wrapping_add(1);
            if w.aidx == w.acnt {
                w.aidx = 0;
                self.k(K0_ARPW);
            }
        } else {
            self.k(K0_NOARP);
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

    fn timer_delay(&self) -> u64 {
        if self.row().eng == 1 { TIMER_DELAY - TICK_LEAD } else { TIMER_DELAY }
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
        self.timed = row.eng == 1;
        if self.timed {
            self.cy = 2 * (hw.bus.clock + TICK_LEAD) as i64;
            self.vk = 0;
            self.k(K_TICK);
        }
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
                    self.vk = ch;
                    self.at(K0_PRLEN, hw);
                    hw.len(ch, 1);
                    self.at(K0_PRLOC, hw);
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

/// Colour clocks from the host's start to the original's start-up writes, and to the end of its init
/// (measured in the reference runs: its InitSound sets the voices up, the timer runs from its end).
const INIT_FIRST_WRITE: u64 = 1444;
const INIT_END: u64 = 8092;
/// Engine 109: colour clocks into a raster line at which the host starts the player in the
/// reference runs (the original's beam waits see the line it is on).
const HOST_PHASE: u64 = 222;
/// Colour clocks from the song's start to the host's timer start, beyond the first period.
const TIMER_DELAY: u64 = 195;
/// Engines 100 and 109: the host's timer runs this many colour clocks ahead of the original's, whose
/// tick starts `K_TICK` cycles after it (the paths before the first write can take less than that).
const TICK_LEAD: u64 = 50;

// The original's code paths (engine 109), for the writes' times: cycles per voice.
const K_TICK: usize = 0;
const K_V: usize = 1;
const K_ALOC: usize = 2;
const K_ALEN: usize = 3;
const K_A2LOC: usize = 4;
const K_A2LEN: usize = 5;
const K_CZ: usize = 6;
const K_CNZ: usize = 7;
const K_IT: usize = 8;
const K_SEND: usize = 9;
const K_LWRAP: usize = 10;
const K_OFF: usize = 11;
const K_IL: usize = 12;
const K_INL: usize = 13;
const K_IVOL: usize = 14;
const K_ILENL: usize = 15;
const K_ILENNL: usize = 16;
const K_ILEN: usize = 17;
const K_ILOC: usize = 18;
const K_IEND: usize = 19;
const K_81: usize = 20;
const K_81ON: usize = 21;
const K_81OFF: usize = 22;
const K_81END: usize = 23;
const K_82: usize = 24;
const K_82ON: usize = 25;
const K_82OFF: usize = 26;
const K_82END: usize = 27;
const K_83: usize = 28;
const K_83OFF: usize = 29;
const K_83LOC: usize = 30;
const K_83LEN: usize = 31;
const K_83L: usize = 32;
const K_83END: usize = 33;
const K_84: usize = 34;
const K_85: usize = 35;
const K_NOTE: usize = 36;
const K_RLOC: usize = 37;
const K_RLEN: usize = 38;
const K_RNO: usize = 39;
const K_EA: usize = 40;
const K_EAW: usize = 41;
const K_EP: usize = 42;
const K_EPDN: usize = 43;
const K_EPDNS: usize = 44;
const K_EPUP: usize = 45;
const K_EPUPS: usize = 46;
const K_EPEQ: usize = 47;
const K_EN: usize = 48;
const K_PER: usize = 49;
const K_FD2: usize = 50;
const K_FFL: usize = 51;
const K_ON: usize = 52;
const K_NOON: usize = 53;
const K_BREAD: usize = 54;
const K_BLINES: usize = 55;
const K_BPOS: usize = 56;
const K_BFIRST: usize = 57;
const K_BPOLL: usize = 58;
const K_ALENX: usize = 59;
const K_A2LENX: usize = 60;
const K_BDELAY: usize = 61;
const B1: [usize; 6] = [K_BREAD, K_BLINES, K_BPOS, K_BFIRST, K_BPOLL, K_BDELAY];
// engine 100
const K0_V: usize = 70;
const K0_CZ: usize = 71;
const K0_CNZ: usize = 72;
const K0_IT: usize = 73;
const K0_SEND: usize = 74;
const K0_LWRAP: usize = 75;
const K0_80: usize = 76;
const K0_81: usize = 77;
const K0_82: usize = 78;
const K0_82X: usize = 79;
const K0_NOTE: usize = 80;
const K0_NOTEX: usize = 81;
const K0_OFF: usize = 82;
const K0_OFFX: usize = 83;
const K0_SLEN: usize = 84;
const K0_SLOC: usize = 85;
const K0_SON: usize = 86;
const K0_PDELAY: usize = 87;
const K0_PLEN: usize = 88;
const K0_PLOC: usize = 89;
const K0_NOPR: usize = 90;
const K0_PVOL: usize = 91;
const K0_PPER: usize = 92;
const K0_ARP: usize = 93;
const K0_ARPW: usize = 94;
const K0_NOARP: usize = 95;
const K0_PRLEN: usize = 96;
const K0_PRLOC: usize = 97;
const K0_BREAD: usize = 98;
const K0_BLINES: usize = 99;
const K0_BPOS: usize = 100;
const K0_BFIRST: usize = 101;
const K0_BPOLL: usize = 102;
const K0_BDELAY: usize = 103;
const B0: [usize; 6] = [K0_BREAD, K0_BLINES, K0_BPOS, K0_BFIRST, K0_BPOLL, K0_BDELAY];
const NK: usize = 104;

/// The cycles of the original's code paths (engine 109), by path and voice: the build Ringside and
/// funny_funk share; the other builds differ in `BUILDS`. Engine 100 is not timed by paths (its
/// rows are 0). The portamento paths (0x81, the period's slide) were measured on copies of Zoom
/// with shorter notes, which reach them within the first 20 s.
static BASE: [[i16; 4]; NK] = {
    let mut c = [[0; 4]; NK];
    c[K_TICK] = [44, 44, 44, 44];
    c[K_V] = [44, 88, 88, 88];
    c[K_ALOC] = [35, 57, 57, 57];
    c[K_ALEN] = [64, 64, 64, 64];
    c[K_A2LOC] = [9, 31, 31, 31];
    c[K_A2LEN] = [16, 16, 16, 16];
    c[K_CZ] = [-3, -3, -3, -3];
    c[K_CNZ] = [23, 23, 23, 23];
    c[K_IT] = [116, 116, 116, 116];
    c[K_SEND] = [60, 60, 60, 60];
    c[K_LWRAP] = [248, 281, 310, 329];
    c[K_OFF] = [56, 56, 56, 56];
    c[K_IL] = [170, 181, 122, 130];
    c[K_INL] = [127, 134, 142, 150];
    c[K_IVOL] = [170, 182, 264, 279];
    c[K_ILENL] = [178, 194, 136, 136];
    c[K_ILENNL] = [125, 136, 146, 146];
    c[K_ILEN] = [179, 194, 283, 281];
    c[K_ILOC] = [44, 44, 44, 44];
    c[K_IEND] = [60, 60, 60, 60];
    c[K_81ON] = [127, 127, 127, 127];
    c[K_81END] = [-3, -3, -3, -3];
    c[K_EP] = [58, 58, 58, 58];
    c[K_EPEQ] = [-64, -64, -64, -64];
    c[K_EPUPS] = [22, 22, 22, 22];
    c[K_EPDNS] = [22, 22, 22, 22];
    c[K_81OFF] = [81, 81, 81, 81];
    c[K_82] = [75, 75, 75, 75];
    c[K_82ON] = [61, 61, 61, 61];
    c[K_82OFF] = [15, 15, 15, 15];
    c[K_82END] = [-3, -3, -3, -3];
    c[K_84] = [150, 150, 150, 150];
    c[K_85] = [78, 78, 78, 78];
    c[K_NOTE] = [94, 94, 94, 94];
    c[K_RLOC] = [244, 244, 244, 244];
    c[K_RLEN] = [20, 20, 20, 20];
    c[K_RNO] = [151, 151, 151, 151];
    c[K_EA] = [141, 141, 141, 141];
    c[K_EAW] = [4, 4, 4, 4];
    c[K_EN] = [-15, -15, -15, -15];
    c[K_PER] = [125, 125, 125, 125];
    c[K_FD2] = [-29, -29, -29, -29];
    c[K_NOON] = [42, 42, 42, 42];
    c[K_BREAD] = [132, 132, 132, 132];
    c[K_BLINES] = [9, 9, 9, 9];
    c[K_BPOS] = [115, 115, 115, 115];
    c[K_BFIRST] = [21, 21, 21, 21];
    c[K_BPOLL] = [25, 25, 25, 25];
    c[K_ALENX] = [14, 14, 14, 14];
    c[K_A2LENX] = [4, 4, 4, 4];
    c
};

/// The builds of engine 109 measured in the reference runs: the code's hash (`code_build`), the
/// colour clocks from the host's start of the player to the song's start (the build's start-up
/// path: the same whatever samples, sequences, header fields, subsong count, hunk length or
/// relocation count the file has), and the paths that take other cycles than the base table's
/// (path, voice, cycles).
static BUILDS: &[Build] = &[
    // Cosmic_Bouncer
    Build { code: 0x2288edd6, start: 12094, over: &[(K_CZ as u8, 0, -15), (K_CZ as u8, 1, -15), (K_CZ as u8, 2, -15), (K_CZ as u8, 3, -15), (K_CNZ as u8, 0, 27), (K_CNZ as u8, 1, 27), (K_CNZ as u8, 2, 27), (K_CNZ as u8, 3, 27), (K_IT as u8, 0, 111), (K_IT as u8, 1, 111), (K_IT as u8, 2, 111), (K_IT as u8, 3, 111), (K_SEND as u8, 0, 57), (K_SEND as u8, 1, 57), (K_SEND as u8, 2, 57), (K_SEND as u8, 3, 57), (K_OFF as u8, 0, 53), (K_OFF as u8, 1, 53), (K_OFF as u8, 2, 53), (K_OFF as u8, 3, 53), (K_IL as u8, 0, 149), (K_INL as u8, 0, 169), (K_IVOL as u8, 0, 191), (K_ILENL as u8, 0, 157), (K_ILENNL as u8, 0, 167), (K_ILEN as u8, 0, 200), (K_IEND as u8, 0, 68), (K_IEND as u8, 1, 68), (K_IEND as u8, 2, 68), (K_IEND as u8, 3, 68), (K_NOTE as u8, 0, 100), (K_NOTE as u8, 1, 100), (K_NOTE as u8, 2, 100), (K_NOTE as u8, 3, 100), (K_RLOC as u8, 0, 250), (K_RLOC as u8, 1, 250), (K_RLOC as u8, 2, 250), (K_RLOC as u8, 3, 250), (K_ALENX as u8, 0, 10), (K_ALENX as u8, 1, 10), (K_ALENX as u8, 2, 10), (K_ALENX as u8, 3, 10), (K_A2LENX as u8, 0, 0), (K_A2LENX as u8, 1, 0), (K_A2LENX as u8, 2, 0), (K_A2LENX as u8, 3, 0)] },
    // Ganymed
    Build { code: 0x46475265, start: 9178, over: &[(K_INL as u8, 1, 167), (K_IVOL as u8, 1, 216), (K_ILENNL as u8, 1, 170), (K_ILEN as u8, 1, 228)] },
    // Great_Giana_Sisters ingame
    Build { code: 0x5be88c5d, start: 8751, over: &[(K_TICK as u8, 0, 5), (K_V as u8, 0, 4), (K_V as u8, 1, 89), (K_V as u8, 2, 89), (K_V as u8, 3, 107), (K_ALOC as u8, 0, 13), (K_ALOC as u8, 1, 56), (K_ALOC as u8, 2, 56), (K_ALOC as u8, 3, 65), (K_A2LOC as u8, 0, -9), (K_A2LOC as u8, 1, 34), (K_A2LOC as u8, 2, 34), (K_A2LOC as u8, 3, 43), (K_CZ as u8, 0, -6), (K_CZ as u8, 1, -6), (K_CZ as u8, 2, -6), (K_CZ as u8, 3, -6), (K_CNZ as u8, 0, 24), (K_CNZ as u8, 1, 24), (K_CNZ as u8, 2, 24), (K_CNZ as u8, 3, 24), (K_IT as u8, 0, 120), (K_IT as u8, 1, 120), (K_IT as u8, 2, 120), (K_IT as u8, 3, 120), (K_SEND as u8, 0, 56), (K_SEND as u8, 1, 56), (K_SEND as u8, 2, 56), (K_SEND as u8, 3, 56), (K_OFF as u8, 0, 65), (K_OFF as u8, 1, 65), (K_OFF as u8, 2, 65), (K_OFF as u8, 3, 65), (K_IEND as u8, 0, 47), (K_IEND as u8, 1, 47), (K_IEND as u8, 2, 47), (K_IEND as u8, 3, 47), (K_85 as u8, 0, 101), (K_85 as u8, 1, 101), (K_85 as u8, 2, 101), (K_85 as u8, 3, 101), (K_NOTE as u8, 0, 108), (K_NOTE as u8, 1, 108), (K_NOTE as u8, 2, 108), (K_NOTE as u8, 3, 108), (K_RLOC as u8, 0, 259), (K_RLOC as u8, 1, 259), (K_RLOC as u8, 2, 259), (K_RLOC as u8, 3, 259), (K_ALENX as u8, 0, 13), (K_ALENX as u8, 1, 13), (K_ALENX as u8, 2, 13), (K_ALENX as u8, 3, 13), (K_A2LENX as u8, 0, 3), (K_A2LENX as u8, 1, 3), (K_A2LENX as u8, 2, 3), (K_A2LENX as u8, 3, 3), (K_EA as u8, 0, 145), (K_EA as u8, 1, 145), (K_EA as u8, 2, 145), (K_EA as u8, 3, 145), (K_84 as u8, 0, 143), (K_84 as u8, 1, 143), (K_84 as u8, 2, 143), (K_84 as u8, 3, 143), (K_INL as u8, 1, 201), (K_ILENNL as u8, 1, 204)] },
    // Katakis
    Build { code: 0xbe94162e, start: 9550, over: &[(K_TICK as u8, 0, -7), (K_V as u8, 0, -7), (K_V as u8, 3, 83), (K_ALOC as u8, 0, 9), (K_ALOC as u8, 3, 54), (K_A2LOC as u8, 0, -17), (K_A2LOC as u8, 3, 28), (K_IL as u8, 0, 149), (K_IL as u8, 1, 159), (K_INL as u8, 0, 169), (K_INL as u8, 1, 179), (K_IVOL as u8, 0, 191), (K_IVOL as u8, 1, 204), (K_ILENL as u8, 0, 157), (K_ILENL as u8, 1, 171), (K_ILENNL as u8, 0, 167), (K_ILENNL as u8, 1, 181), (K_ILEN as u8, 0, 200), (K_ILEN as u8, 1, 217)] },
    // Ringside
    Build { code: 0x8e8080ce, start: 6923, over: &[] },
    // Rock_Challenge
    Build { code: 0x8b9abb0d, start: 17159, over: &[(K_TICK as u8, 0, 41), (K_V as u8, 0, 41), (K_V as u8, 1, 83), (K_V as u8, 2, 83), (K_V as u8, 3, 83), (K_ALOC as u8, 0, 33), (K_ALOC as u8, 1, 54), (K_ALOC as u8, 2, 54), (K_ALOC as u8, 3, 54), (K_A2LOC as u8, 0, 7), (K_A2LOC as u8, 1, 28), (K_A2LOC as u8, 2, 28), (K_A2LOC as u8, 3, 28), (K_CZ as u8, 0, -15), (K_CZ as u8, 1, -15), (K_CZ as u8, 2, -15), (K_CZ as u8, 3, -15), (K_CNZ as u8, 0, 27), (K_CNZ as u8, 1, 27), (K_CNZ as u8, 2, 27), (K_CNZ as u8, 3, 27), (K_IT as u8, 0, 111), (K_IT as u8, 1, 111), (K_IT as u8, 2, 111), (K_IT as u8, 3, 111), (K_SEND as u8, 0, 57), (K_SEND as u8, 1, 57), (K_SEND as u8, 2, 57), (K_SEND as u8, 3, 57), (K_OFF as u8, 0, 53), (K_OFF as u8, 1, 53), (K_OFF as u8, 2, 53), (K_OFF as u8, 3, 53), (K_IL as u8, 0, 168), (K_IL as u8, 1, 179), (K_IL as u8, 2, 120), (K_IL as u8, 3, 128), (K_IVOL as u8, 0, 168), (K_IVOL as u8, 1, 180), (K_IVOL as u8, 2, 262), (K_IVOL as u8, 3, 277), (K_ILOC as u8, 0, 40), (K_ILOC as u8, 1, 40), (K_ILOC as u8, 2, 40), (K_ILOC as u8, 3, 40), (K_IEND as u8, 0, 68), (K_IEND as u8, 1, 68), (K_IEND as u8, 2, 68), (K_IEND as u8, 3, 68), (K_NOTE as u8, 0, 96), (K_NOTE as u8, 1, 96), (K_NOTE as u8, 2, 96), (K_NOTE as u8, 3, 96), (K_RLOC as u8, 0, 246), (K_RLOC as u8, 1, 246), (K_RLOC as u8, 2, 246), (K_RLOC as u8, 3, 246), (K_EN as u8, 0, -17), (K_EN as u8, 1, -17), (K_EN as u8, 2, -17), (K_EN as u8, 3, -17), (K_PER as u8, 0, 123), (K_PER as u8, 1, 123), (K_PER as u8, 2, 123), (K_PER as u8, 3, 123), (K_BPOLL as u8, 0, 0), (K_BPOLL as u8, 1, 0), (K_BPOLL as u8, 2, 0), (K_BPOLL as u8, 3, 0), (K_ALENX as u8, 0, 10), (K_ALENX as u8, 1, 10), (K_ALENX as u8, 2, 10), (K_ALENX as u8, 3, 10), (K_A2LENX as u8, 0, 0), (K_A2LENX as u8, 1, 0), (K_A2LENX as u8, 2, 0), (K_A2LENX as u8, 3, 0), (K_BDELAY as u8, 0, 2016), (K_BDELAY as u8, 1, 2016), (K_BDELAY as u8, 2, 2016), (K_BDELAY as u8, 3, 2016)] },
    // Zoom
    Build { code: 0x74bab24f, start: 9326, over: &[(K_TICK as u8, 0, 51), (K_V as u8, 0, 51), (K_V as u8, 1, 85), (K_V as u8, 2, 85), (K_V as u8, 3, 85), (K_ALOC as u8, 0, 42), (K_ALOC as u8, 1, 56), (K_ALOC as u8, 2, 56), (K_ALOC as u8, 3, 56), (K_A2LOC as u8, 1, 30), (K_A2LOC as u8, 2, 30), (K_A2LOC as u8, 3, 30), (K_CZ as u8, 0, -4), (K_CZ as u8, 1, -4), (K_CZ as u8, 2, -4), (K_CZ as u8, 3, -4), (K_IT as u8, 0, 112), (K_IT as u8, 1, 112), (K_IT as u8, 2, 112), (K_IT as u8, 3, 112), (K_SEND as u8, 0, 56), (K_SEND as u8, 1, 56), (K_SEND as u8, 2, 56), (K_SEND as u8, 3, 56), (K_OFF as u8, 0, 57), (K_OFF as u8, 1, 57), (K_OFF as u8, 2, 57), (K_OFF as u8, 3, 57), (K_IL as u8, 1, 159), (K_INL as u8, 0, 158), (K_INL as u8, 1, 179), (K_IVOL as u8, 0, 202), (K_IVOL as u8, 1, 204), (K_ILENL as u8, 1, 171), (K_ILENNL as u8, 0, 156), (K_ILENNL as u8, 1, 181), (K_ILEN as u8, 0, 211), (K_ILEN as u8, 1, 217), (K_IEND as u8, 0, 63), (K_IEND as u8, 1, 63), (K_IEND as u8, 2, 63), (K_IEND as u8, 3, 63), (K_84 as u8, 0, 161), (K_84 as u8, 1, 161), (K_84 as u8, 2, 161), (K_84 as u8, 3, 161), (K_NOTE as u8, 0, 109), (K_NOTE as u8, 1, 109), (K_NOTE as u8, 2, 109), (K_NOTE as u8, 3, 109), (K_RLOC as u8, 0, 233), (K_RLOC as u8, 1, 233), (K_RLOC as u8, 2, 233), (K_RLOC as u8, 3, 233), (K_RNO as u8, 0, 189), (K_RNO as u8, 1, 189), (K_RNO as u8, 2, 189), (K_RNO as u8, 3, 189)] },
    // funny_funk
    Build { code: 0x3763398c, start: 9876, over: &[] },
    // smash_intro
    Build { code: 0xf7e3367c, start: 11854, over: &[(K_INL as u8, 1, 167), (K_IVOL as u8, 1, 216), (K_ILENNL as u8, 1, 170), (K_ILEN as u8, 1, 228), (K_84 as u8, 0, 149), (K_84 as u8, 1, 149), (K_84 as u8, 2, 149), (K_84 as u8, 3, 149), (K_NOTE as u8, 0, 93), (K_NOTE as u8, 1, 93), (K_NOTE as u8, 2, 93), (K_NOTE as u8, 3, 93), (K_RLOC as u8, 0, 241), (K_RLOC as u8, 1, 241), (K_RLOC as u8, 2, 241), (K_RLOC as u8, 3, 241)] },
];

struct Build {
    code: u32,
    start: u32,
    over: &'static [(u8, u8, i16)],
}

/// The build of the replay code in the hunk `img`: a hash (FNV-1a) of the 0x400 bytes from the
/// header's entry at 0x14 on (the replay routines, shared by files of other games), with the
/// relocated longs zeroed, so it does not depend on where the data lies.
fn code_build(img: &[u8], relocs: &[u32]) -> u32 {
    let s = be32(img, 0x14) as usize;
    let e = (s + 0x400).min(img.len());
    let mut h: u32 = 0x811C_9DC5;
    for (i, &b) in img.get(s..e).unwrap_or(&[]).iter().enumerate() {
        let o = (s + i) as u32;
        let rel = (0..4).any(|k| o >= k && relocs.binary_search(&(o - k)).is_ok());
        h = (h ^ if rel { 0 } else { b as u32 }).wrapping_mul(0x0100_0193);
    }
    h
}

/// The cycles of the code paths of the build (the base table with the build's own cycles where
/// the reference runs showed others) and its song start, if the build was measured.
fn build_costs(build: Option<&Build>) -> [[i32; 4]; NK] {
    let mut c = [[0i32; 4]; NK];
    for (k, row) in BASE.iter().enumerate() {
        for v in 0..4 {
            c[k][v] = row[v] as i32;
        }
    }
    if let Some(b) = build {
        for &(k, v, n) in b.over {
            c[k as usize][v as usize] = n as i32;
        }
    }
    c
}

/// The IFF 8SVX headers in the hunk: from each FORM to the start of its BODY chunk's data.
fn iff_headers(img: &[u8]) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut s = 0;
    while s + 12 <= img.len() {
        if &img[s..s + 4] != b"FORM" || &img[s + 8..s + 12] != b"8SVX" {
            s += 2;
            continue;
        }
        let mut p = s + 12;
        while p + 8 <= img.len() {
            if &img[p..p + 4] == b"BODY" {
                v.push((s, p + 8));
                break;
            }
            p += 8 + ((be32(img, p + 4) as usize + 1) & !1);
        }
        s += 12;
    }
    v
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let (_, n) = header(d)?;
    let rows = rows_of(d);
    let img = d[32..32 + 4 * n].to_vec();
    let name = be32(&img, 0x24) as usize;
    let title = img.get(name..).map_or(String::new(), |s| s.iter().take_while(|&&b| b != 0).take(80).map(|&b| b as char).collect());
    let mut b = Beathoven {
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
        timed: false,
        cy: 0,
        vk: 0,
        costs: [[0; 4]; NK],
    };
    let build = BUILDS.iter().find(|x| x.code == code_build(&b.pristine, &b.relocs));
    b.costs = build_costs(build);
    if b.row().eng != 0 {
        // the start-up routine runs in the original's InitSound, before its timer: the first
        // tick is already the first pass over the voices
        let mut t0 = m.bus.clock;
        let mut end = INIT_END;
        if b.row().eng == 1 {
            // a sample kept as an IFF 8SVX file plays its header as silence: the chunks before
            // the BODY's data read as zero (Katakis; measured with copies whose header bytes and
            // first sample bytes were cleared one length at a time)
            for (s, e) in iff_headers(&b.pristine) {
                m.bus.write_bytes(b.mem + s as u32, &vec![0; e - s]);
            }
            // the original polls the beam: its start lies on the reference's line phase
            t0 += (HOST_PHASE + crate::paula::LINE - t0 % crate::paula::LINE) % crate::paula::LINE;
            m.bus.clock = t0;
            end = build.map_or(INIT_END, |x| x.start as u64);
        }
        let mut hw = Hw { bus: &mut m.bus };
        hw.wait(INIT_FIRST_WRITE);
        b.start(&mut hw);
        m.bus.clock = m.bus.clock.max(t0 + end);
    }
    Some(Box::new(b))
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
        let b = Beathoven { pristine: Vec::new(), img: d[32..end].to_vec(), mem: 0x2_0000 + 32, relocs: vec![0x700], rows: rows_of(&d), title: String::new(), h14: 0, sub: 0, started: true, v: Default::default(), w: Default::default(), timed: false, cy: 0, vk: 0, costs: [[0; 4]; NK] };
        assert_eq!(b.table_word(0x700), 2);
        assert_eq!(b.table_word(-4), 0);
        assert_eq!(b.table_word(0x5fe), 500);
        // a load address below 0x10000 reads 1, the value of the one observed case
        let low = Beathoven { mem: 0x8000, ..b };
        assert_eq!(low.table_word(0x700), 1);
    }
}
