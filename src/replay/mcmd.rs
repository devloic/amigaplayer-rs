//! MCMD (Amiga; Eagleplayer "MCMD", "Testversion by Eagleeye of DEFECT", 1993; UADE prefix `mcmd_org.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player; no original code. Three small tables of the player are values of
//! the specification and coded here as constants: the 73-entry period table, the 8-byte default
//! sequence and the start values of two voices' note and loop-length variables.
//!
//! A module is an 18-byte header (magic, counts), then effect sequences, instruments, patterns,
//! positions, subsong entries, sample records and the 8-bit sample data. Four voices with their
//! own pattern lists; a note selects an instrument, which selects a volume sequence and an
//! effect (arpeggio / sample) sequence; samples are started from the effect sequence. Every
//! register write is placed at the clock of the specification's cycle ledger, including the
//! raster waits of the DMA phase (modelled from the beam phase).
//!
//! No real module exists; only the specification's 22 synthetic ones. Detection is the magic only
//! (plus a minimum size), so the replayer is registered after the others.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): bytes asked for
//! beyond the end of the module read as zero; effect and volume commands above 0xE8 end the step
//! like a hold; a sequence that loops without a value is cut after a bounded number of reads;
//! the host's volume, balance and voice masks are not modelled (full gain). At the wrap of the
//! position list the song end is reported and the voice goes on with the restart position's own
//! entry (the specification's "evident intent"; the reference reads three bytes of host memory
//! there, whose values cannot be reproduced), so the writes of that one tick can differ from the
//! reference in a few values. A subsong word 2 of 0 (speed) lets the counter wrap around, as an
//! unsigned 16-bit counter does.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "MCMD", detect, new };

/// Colour clocks between two ticks (the host's 50 Hz timer).
const TICK: u64 = 70_940;
/// Clock of the first play call relative to the run's origin (the host's phase).
const FIRST_CALL: u64 = 73_889;
const LINE: u64 = 227;
const FRAME: u64 = 313 * LINE;

/// The period table (73 entries; indices 73..127 read as 0). Values of the specification.
const PERIODS: [u16; 73] = [
    1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570, 538,
    508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143,
    135, 127, 120, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 3424, 3232, 3048, 2880, 2712, 2560,
    2416, 2280, 2152, 2032, 1920, 1812, 598,
];
/// The sequence both sequence pointers have before the first note (values of the specification).
const DEFAULT_SEQ: [u8; 8] = [1, 0, 0, 0, 0, 0, 0, 0xE1];
/// Start values of the never-initialised note variables, per voice (values of the specification).
const START_NOTE: [u8; 4] = [75, 0, 12, 0];
/// Start values of the loop-length variables, per voice (values of the specification).
const START_LL: [u16; 4] = [0x0010, 0, 0x0001, 0];

/// Costs (cycles) of a sample set before the length write and after it, by channel.
const SS1: [u64; 4] = [53, 76, 99, 89];
const SS2: [u64; 4] = [49, 72, 95, 85];

/// Bound on the reads of one sequence step (a sequence that never reaches a value hangs the
/// reference; here the step is cut).
const GUARD: u32 = 2048;

fn byte(d: &[u8], o: i64) -> u8 {
    if o < 0 {
        0
    } else {
        d.get(o as usize).copied().unwrap_or(0)
    }
}

fn word(d: &[u8], o: i64) -> u16 {
    ((byte(d, o) as u16) << 8) | byte(d, o + 1) as u16
}

fn long(d: &[u8], o: i64) -> u32 {
    ((word(d, o) as u32) << 16) | word(d, o + 2) as u32
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 18 && &d[0..4] == b"MCMD"
}

/// The section length: a 16-bit product added as a signed 16-bit value (spec 1).
fn sect(n: u16, size: u16) -> i64 {
    n.wrapping_mul(size) as i16 as i64
}

/// One voice's state (spec 4).
#[derive(Clone, Copy)]
struct Voice {
    pos_base: i64,
    pos_end: i64,
    pos_off: u16,
    pat: i64,
    row_off: u16,
    transpose: u8,
    ins_off: u8,
    n: u8,
    b1: u8,
    pb: u8,
    cs: u8,
    /// The effect sequence number (None: the default sequence).
    eff: Option<u8>,
    ep: u16,
    ew: u8,
    a: u8,
    /// The instrument record of the volume sequence (None: the default sequence).
    vrec: Option<i64>,
    vp: u16,
    vc: u8,
    vr: u8,
    vw: u8,
    v: u8,
    pct: u32,
    vib_speed: u8,
    vib_depth: u8,
    vib_cur: u8,
    vib_delay: u8,
    vib_up: bool,
    acc: u32,
    ls: u32,
    ll: u16,
}

pub struct Mcmd {
    d: Vec<u8>,
    /// Guest address of the module.
    base: u32,
    eff_base: i64,
    ins_base: i64,
    pat_base: i64,
    rec_base: i64,
    smp_base: i64,
    ps: u16,
    tick_ctr: u16,
    speed: u16,
    restart_off: u16,
    stop: bool,
    mask: u8,
    v: [Voice; 4],
    origin: u64,
    songend: bool,
}

/// Where a tick's writes go: the cycle count `t` from the tick's start gives the colour clock.
struct Clk {
    t0: u64,
    /// The tick's start relative to the run's origin (the beam's reference).
    rel: u64,
    t: u64,
}

impl Clk {
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.t / 2;
    }

    /// The beam line (low 8 bits) at the current count.
    fn line(&self) -> u64 {
        (((self.rel + self.t / 2) % FRAME) / LINE) & 0xFF
    }

    /// Wait for five line changes (spec 11.3).
    fn w5(&mut self) {
        self.t += 28;
        for _ in 0..5 {
            self.t += 16;
            let l0 = self.line();
            // bounded: the beam moves on within a line
            for _ in 0..4096 {
                self.t += 16;
                let r = self.line();
                self.t += 11;
                if r != l0 {
                    break;
                }
            }
            self.t += 10;
        }
        self.t += 44;
    }
}

impl Mcmd {
    fn rd(&self, o: i64) -> u8 {
        byte(&self.d, o)
    }

    fn eff_byte(&self, v: &Voice, cur: u16) -> u8 {
        match v.eff {
            None => DEFAULT_SEQ.get(cur as usize).copied().unwrap_or(0xE1),
            Some(n) => self.rd(self.eff_base + 64 * n as i64 + cur as i64),
        }
    }

    fn vol_byte(&self, v: &Voice, p: u16) -> u8 {
        match v.vrec {
            None => DEFAULT_SEQ.get(p as usize).copied().unwrap_or(0xE1),
            Some(r) => self.rd(r + 5 + p as i64),
        }
    }

    fn set_sample(&mut self, hw: &mut Hw, clk: &mut Clk, ch: usize, s: u8) {
        let rec = self.rec_base + 28 * s as i64;
        let start = self
            .base
            .wrapping_add(self.smp_base as u32)
            .wrapping_add(long(&self.d, rec + 18));
        clk.t += 12;
        clk.at(hw);
        hw.loc(ch, start);
        clk.t += SS1[ch] + 16 + 58;
        clk.t += 24;
        clk.at(hw);
        hw.len(ch, word(&self.d, rec + 22));
        clk.t += SS2[ch] + 16;
        let v = &mut self.v[ch];
        v.ls = start.wrapping_add(word(&self.d, rec + 24) as u32);
        v.ll = word(&self.d, rec + 26);
    }

    fn dma_off(&self, hw: &mut Hw, clk: &Clk, ch: usize) {
        clk.at(hw);
        hw.dmacon(1 << ch);
    }

    /// Spec 5.2: advance the position of voice `ch` (the time is charged here too).
    fn position_advance(&mut self, hw: &mut Hw, clk: &mut Clk, ch: usize) {
        let _ = hw;
        clk.t += 63;
        let v = self.v[ch];
        let mut addr = v.pos_base + v.pos_off as i64;
        if addr == v.pos_end {
            clk.t += 92;
            self.v[ch].pos_off = self.restart_off;
            self.songend = true;
            clk.t -= clk.t % 2;
            clk.t += 16;
            // the restart position's own entry stands in for the host's three bytes
            addr = v.pos_base + self.restart_off as i64;
        }
        clk.t += 57;
        let (p, t, x) = (self.rd(addr), self.rd(addr + 1), self.rd(addr + 2));
        let ps = self.ps as i64;
        let v = &mut self.v[ch];
        v.transpose = t;
        if x < 0x80 {
            v.ins_off = x;
            clk.t += 12;
        } else {
            clk.t += 51;
            let hi = x >> 4;
            let lo = (x & 15) as u32;
            if hi == 0xF {
                v.pct = if lo == 0 { 100 } else { (16 - lo) * 6 };
                clk.t += 17 + if lo == 0 { 22 } else { 50 };
            } else {
                clk.t += 17;
                if hi == 8 {
                    self.stop = true;
                    clk.t += 30;
                }
            }
        }
        let v = &mut self.v[ch];
        v.row_off = 0;
        v.pat = self.pat_base + p as i64 * ps;
        v.pos_off = v.pos_off.wrapping_add(12);
        clk.t += 130;
    }

    /// Spec 5.1: the row step of voice `ch`.
    fn row_step(&mut self, hw: &mut Hw, clk: &mut Clk, ch: usize) {
        clk.t += 77;
        let v = self.v[ch];
        let b0 = self.rd(v.pat + v.row_off as i64);
        let brk = b0 & 0x7F == 1;
        if !brk {
            clk.t += 23;
        }
        if brk || v.row_off == self.ps {
            self.position_advance(hw, clk, ch);
        }
        let v = self.v[ch];
        let (b0, b1) = (self.rd(v.pat + v.row_off as i64), self.rd(v.pat + v.row_off as i64 + 1));
        clk.t += 35;
        if b0 & 0x7F != 0 {
            let row0 = v.row_off == 0;
            clk.t += 53 + if row0 { 8 } else { 0 } + 37;
            {
                let v = &mut self.v[ch];
                v.n = b0 & 0x7F;
                v.acc = 0;
                v.b1 = b1;
            }
            if b1 & 0xC0 != 0 {
                clk.t += 20;
                let prev = if row0 {
                    v.pat + self.ps as i64 - 2 + 1
                } else {
                    v.pat + v.row_off as i64 - 2 + 1
                };
                self.v[ch].pb = self.rd(prev);
            }
            clk.t += 13;
            if b0 & 0x80 == 0 {
                clk.t += 21;
                if self.v[ch].cs & 0x80 != 0 {
                    clk.t += 36;
                    self.dma_off(hw, clk, ch);
                }
                clk.t += 267;
                let inum = ((b1 & 0x3F).wrapping_add(self.v[ch].ins_off)) as i64;
                let rec = self.ins_base + 64 * inum;
                let (sp, es, vs, vd, vdl) = (
                    self.rd(rec),
                    self.rd(rec + 1),
                    self.rd(rec + 2),
                    self.rd(rec + 3),
                    self.rd(rec + 4),
                );
                let pb = self.v[ch].pb;
                let v = &mut self.v[ch];
                v.vc = sp;
                v.vr = sp;
                v.vp = 0;
                v.vib_speed = vs;
                v.vib_depth = vd;
                v.vib_cur = vd;
                v.vib_delay = vdl;
                v.vib_up = false;
                v.vrec = Some(rec);
                v.eff = Some(if b1 & 0x80 != 0 { pb } else { es });
                v.ep = 0;
                v.ew = 0;
                v.vw = 0;
                if b1 & 0x80 != 0 {
                    clk.t += 12;
                }
                clk.t += 82;
            }
        }
        let v = &mut self.v[ch];
        v.row_off = v.row_off.wrapping_add(2);
        clk.t += 32;
    }

    /// Spec 6: the effect sequence step.
    fn eff_step(&mut self, hw: &mut Hw, clk: &mut Clk, ch: usize) {
        let mut guard = 0;
        loop {
            if self.v[ch].ew > 0 {
                clk.t += 47;
                self.v[ch].ew -= 1;
                return;
            }
            clk.t += 53;
            let mut cur = self.v[ch].ep;
            loop {
                guard += 1;
                if guard > GUARD {
                    return;
                }
                let v = self.v[ch];
                let b = self.eff_byte(&v, cur);
                cur = cur.wrapping_add(1);
                clk.t += 31;
                if b <= 0xDF {
                    clk.t += 28;
                    let v = &mut self.v[ch];
                    v.a = b;
                    v.ep = v.ep.wrapping_add(1);
                    return;
                }
                clk.t += 40;
                let arg = self.eff_byte(&v, cur);
                match b {
                    0xE0 => {
                        clk.t += 58;
                        self.v[ch].ep = arg as u16;
                        cur = arg as u16;
                    }
                    0xE1 => return,
                    0xE2 => {
                        cur = cur.wrapping_add(1);
                        self.v[ch].cs = 0xFF;
                        self.mask |= 1 << ch;
                        clk.t += 52;
                        self.dma_off(hw, clk, ch);
                        clk.t += 162;
                        self.set_sample(hw, clk, ch, arg);
                        let v = &mut self.v[ch];
                        v.vp = 0;
                        v.vc = 1;
                        v.ep = v.ep.wrapping_add(2);
                        clk.t += 82;
                    }
                    0xE3 => {
                        let a2 = self.eff_byte(&v, cur.wrapping_add(1));
                        cur = cur.wrapping_add(2);
                        let v = &mut self.v[ch];
                        v.vib_speed = arg;
                        v.vib_depth = a2;
                        v.ep = v.ep.wrapping_add(3);
                        clk.t += 58;
                    }
                    0xE4 => {
                        cur = cur.wrapping_add(1);
                        clk.t += 162;
                        self.set_sample(hw, clk, ch, arg);
                        self.v[ch].ep = self.v[ch].ep.wrapping_add(2);
                        clk.t += 54;
                    }
                    0xE5 => {
                        cur = cur.wrapping_add(1);
                        clk.t += 33;
                        if arg == v.cs {
                            let v = &mut self.v[ch];
                            v.vp = 0;
                            v.vc = 1;
                            clk.t += 54;
                        } else {
                            self.v[ch].cs = arg;
                            self.mask |= 1 << ch;
                            clk.t += 48;
                            self.dma_off(hw, clk, ch);
                            clk.t += 150;
                            self.set_sample(hw, clk, ch, arg);
                            let v = &mut self.v[ch];
                            v.vp = 0;
                            v.vc = 1;
                            clk.t += 28 + 54;
                        }
                        self.v[ch].ep = self.v[ch].ep.wrapping_add(2);
                    }
                    0xE6 => clk.t += 10,
                    0xE7 => {
                        clk.t += 124;
                        let v = &mut self.v[ch];
                        v.eff = Some(arg);
                        v.ep = 0;
                        cur = 0;
                    }
                    0xE8 => {
                        clk.t += 42;
                        let v = &mut self.v[ch];
                        v.ew = arg;
                        v.ep = v.ep.wrapping_add(2);
                        break;
                    }
                    // not supported by the reference: ends the step (safe choice)
                    _ => return,
                }
            }
        }
    }

    /// Spec 7: the volume sequence step.
    fn vol_step(&mut self, ch: usize, clk: &mut Clk) {
        let mut guard = 0;
        loop {
            if self.v[ch].vw > 0 {
                clk.t += 47;
                self.v[ch].vw -= 1;
                return;
            }
            clk.t += 46;
            let v = &mut self.v[ch];
            v.vc = v.vc.wrapping_sub(1);
            if v.vc != 0 {
                return;
            }
            v.vc = v.vr;
            clk.t += 52;
            loop {
                guard += 1;
                if guard > GUARD {
                    return;
                }
                let v = self.v[ch];
                let b = self.vol_byte(&v, v.vp);
                clk.t += 29;
                if b <= 0xDF {
                    clk.t += 28;
                    let v = &mut self.v[ch];
                    v.v = b;
                    v.vp = v.vp.wrapping_add(1);
                    return;
                }
                clk.t += 40;
                let arg = self.vol_byte(&v, v.vp.wrapping_add(1));
                match b {
                    0xE0 => {
                        clk.t += 70;
                        self.v[ch].vp = (arg as u16).wrapping_sub(5);
                    }
                    0xE8 => {
                        clk.t += 42;
                        let v = &mut self.v[ch];
                        v.vw = arg;
                        v.vp = v.vp.wrapping_add(2);
                        break;
                    }
                    // holds (0xE1..0xE7) end the step; the unsupported ones are treated alike
                    _ => return,
                }
            }
        }
    }

    /// Spec 8: the period and the volume of voice `ch`, written (the clock is advanced here).
    fn period_volume(&mut self, hw: &mut Hw, clk: &mut Clk, ch: usize) {
        let v = &mut self.v[ch];
        clk.t += 21;
        let idx = if v.a >= 0x80 {
            v.a & 0x7F
        } else {
            clk.t += 24;
            v.a.wrapping_add(v.n).wrapping_add(v.transpose) & 0x7F
        };
        clk.t += 59;
        let mut p = PERIODS.get(idx as usize).copied().unwrap_or(0);
        if v.vib_delay > 0 {
            clk.t += 26;
            v.vib_delay -= 1;
        } else {
            clk.t += 79;
            let (speed, depth) = (v.vib_speed as i32, v.vib_depth as i32);
            let mut c = v.vib_cur as i32;
            if !v.vib_up {
                clk.t += 13;
                c -= speed;
                if c < 0 {
                    c = 0;
                    v.vib_up = true;
                    clk.t += 26;
                }
            } else {
                clk.t += 17;
                c += speed;
                if c > depth {
                    c = depth;
                    v.vib_up = false;
                    clk.t += 18;
                }
            }
            v.vib_cur = c as u8;
            clk.t += 118;
            let add = ((c - (depth >> 1)) * (p as i16 as i32)) >> 10;
            p = p.wrapping_add(add as u16);
        }
        clk.t += 25;
        if v.b1 & 0x40 != 0 {
            clk.t += 33;
            let s = v.pb as i8;
            v.acc = v.acc.wrapping_add((s as i32).unsigned_abs() << 11);
            let q = (v.acc >> 16) as u16;
            if s >= 0 {
                clk.t += 74;
                p = p.wrapping_sub(q);
            } else {
                clk.t += 68;
                p = p.wrapping_add(q);
            }
        }
        clk.t += 242;
        let vol = (v.v as u32 * v.pct) / 100;
        let hwv = (vol * 64) >> 6;
        clk.t += if ch == 0 { 46 } else { 58 };
        clk.t += 28;
        clk.at(hw);
        hw.per(ch, p);
        clk.t += 117;
        clk.at(hw);
        hw.vol(ch, hwv as u16);
        clk.t += 40;
    }

    fn run(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        let mut clk = Clk { t0, rel: t0.saturating_sub(self.origin), t: 283 };
        if self.stop {
            clk.t += 16;
            clk.at(hw);
            hw.dmacon(0x000F);
            clk.t += 88;
            for c in 0..4 {
                clk.at(hw);
                hw.vol(c, 0);
                clk.t += 12;
            }
            return;
        }
        clk.t += 33;
        self.tick_ctr = self.tick_ctr.wrapping_sub(1);
        if self.tick_ctr == 0 {
            self.tick_ctr = self.speed;
            clk.t += 42;
            for c in 0..4 {
                if c > 0 {
                    clk.t += 26;
                }
                self.row_step(hw, &mut clk, c);
            }
        }
        self.mask = 0;
        clk.t += 34;
        for c in 0..4 {
            if c > 0 {
                clk.t += 26;
            }
            self.eff_step(hw, &mut clk, c);
            self.vol_step(c, &mut clk);
            self.period_volume(hw, &mut clk, c);
        }
        clk.t += 17;
        if self.mask != 0 {
            clk.t += 120;
            for _ in 0..3 {
                clk.t += 18;
                clk.w5();
            }
            clk.t += 12;
            clk.at(hw);
            hw.dmacon(0x8000 | self.mask as u16);
            clk.t += 18;
            clk.w5();
            clk.t += 8;
            for c in 0..4 {
                clk.t += 32;
                clk.at(hw);
                hw.loc(c, self.v[c].ls);
                clk.t += 24;
                clk.at(hw);
                hw.len(c, self.v[c].ll);
            }
        }
    }
}

impl Replayer for Mcmd {
    fn format(&self) -> &'static str {
        "MCMD"
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

    fn tick(&mut self, hw: &mut Hw) {
        self.run(hw);
    }
}

/// Builds the player state from a module image (`base`: its guest address).
fn build(d: &[u8], base: u32, origin: u64) -> Mcmd {
    let h = |o: usize| word(d, o as i64);
    let (ne, ni, np, no, ps, ns, nw) = (h(4), h(6), h(8), h(10), h(12), h(14), h(16));
    let eff_base = 18i64;
    let ins_base = eff_base + sect(ne, 64);
    let pat_base = ins_base + sect(ni, 64);
    let pos_base = pat_base + sect(np, ps);
    let sub_base = pos_base + sect(no, 12);
    let rec_base = sub_base + sect(ns, 8);
    let smp_base = rec_base + sect(nw, 28);
    let (first, last, speed, restart) = (
        word(d, sub_base) as i64,
        word(d, sub_base + 2) as i64,
        word(d, sub_base + 4),
        word(d, sub_base + 6),
    );
    let restart_off = (restart.wrapping_sub(first as u16)).wrapping_mul(12);
    let mut m = Mcmd {
        d: d.to_vec(),
        base,
        eff_base,
        ins_base,
        pat_base,
        rec_base,
        smp_base,
        ps,
        tick_ctr: 1,
        speed,
        restart_off,
        stop: false,
        mask: 0,
        v: [Voice {
            pos_base: 0,
            pos_end: 0,
            pos_off: 12,
            pat: 0,
            row_off: 0,
            transpose: 0,
            ins_off: 0,
            n: 0,
            b1: 0,
            pb: 0,
            cs: 0xFF,
            eff: None,
            ep: 0,
            ew: 0,
            a: 0,
            vrec: None,
            vp: 0,
            vc: 1,
            vr: 1,
            vw: 0,
            v: 0,
            pct: 100,
            vib_speed: 0,
            vib_depth: 0,
            vib_cur: 0,
            vib_delay: 0,
            vib_up: false,
            acc: 0,
            ls: 0,
            ll: 0,
        }; 4],
        origin,
        songend: false,
    };
    for c in 0..4 {
        let pb = pos_base + first * 12 + 3 * c as i64;
        let e = pb;
        let (p, t, x) = (byte(d, e), byte(d, e + 1), byte(d, e + 2));
        let v = &mut m.v[c];
        v.pos_base = pb;
        v.pos_end = pos_base + (last + 1) * 12 + 3 * c as i64;
        v.pat = pat_base + p as i64 * ps as i64;
        v.transpose = t;
        v.ins_off = x;
        v.n = START_NOTE[c];
        v.ll = START_LL[c];
    }
    m
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    // the reference's start-up counts from the run's origin, START_PHASE colour clocks before the
    // engine's aligned start; the first tick follows one period after the clock left here
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    {
        let mut hw = Hw { bus: &mut m.bus };
        let put = |hw: &mut Hw, t: u64| hw.bus.clock = (origin + t).max(hw.bus.clock);
        put(&mut hw, 501);
        hw.bus.write_byte(0xBFE001, 0x02);
        put(&mut hw, 857);
        hw.dmacon(0x000F);
        put(&mut hw, 865);
        hw.bus.write_word(0xDFF09A, 0x0780);
        for c in 0..4u64 {
            let t = 1238 + 495 * c;
            put(&mut hw, t);
            hw.loc(c as usize, 0x00DF_F0A0 + 16 * c as u32);
            put(&mut hw, t + 6);
            hw.len(c as usize, 1);
            put(&mut hw, t + 10);
            hw.per(c as usize, 0);
            put(&mut hw, t + 14);
            hw.vol(c as usize, 0);
        }
    }
    m.bus.clock = (origin + FIRST_CALL - TICK).max(m.bus.clock);
    Some(Box::new(build(d, base, origin)))
}

#[cfg(test)]
#[path = "mcmd_tests.rs"]
mod tests;
