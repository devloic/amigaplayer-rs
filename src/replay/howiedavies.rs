//! Howie Davies (`hd.*`: Blazing Thunder, Future Bike Simulator, Yogi's Great Escape; Gary Antcliffe
//! and Howie Davies, 1990-91).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A module is an AmigaDOS hunk executable that carries
//! the game's own sound driver, followed by the song data and the samples. The driver is never run
//! here: the replay is a small step sequencer (four voices driven directly with raw sample
//! pointers) that is rebuilt from the module's DATA tables, which are located by the rules of the
//! specification (the period table, the table of channel DMA bits, the sample table, the subsong
//! table, the order lists and the pattern pointer table), and the three driver variants (A, B, C)
//! are told apart by the distance between two of those tables. A layout that does not verify against
//! the module's own data is not recognised.
//!
//! One interrupt of 70940 colour clocks runs the wrapper, a routine that switches the voices on
//! and points them at their loop (or the silent word), and the tick routine (a step every S ticks
//! reads the channels' pattern and order lists; the envelope tick writes the volumes). Every
//! register write is placed at the clock of the original's code: static 68000 cycle costs,
//! accumulated in cycles and halved when a clock is needed, with the two raster-line waits of the
//! DMA routine taken from the absolute colour clock of the call.
//!
//! Choices where the specification leaves the behaviour open: a pattern or order command number
//! that the variant does not have, a pattern index beyond the pattern pointer table, a jump back
//! before the start of the image and a channel that runs more than 100 000 events in one step stop
//! the channel for good (no Paula write, no song end mark). The song-end extra cost of variant A
//! (83 cycles) was never observed and is taken equal to variant C's.

use super::{Hw, Open, Replayer, be16, be32};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Howie Davies", detect, new };

/// Colour clocks between two interrupts.
const FRAME: u64 = 70940;
/// Colour clocks of one raster line.
const LINE: u64 = 227;
/// Lines of the beam position counter.
const LINES: u64 = 313;
/// The silent first period table word the table is found by.
const PERIOD_KEY: u32 = 0x3580_3280;
const EVENT_CAP: usize = 100_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Var {
    A,
    B,
    C,
}

/// Where the tables of a module are (offsets inside the code image).
#[derive(Clone, Copy, Debug)]
struct Lay {
    var: Var,
    per: usize,
    samp: usize,
    nsamp: usize,
    sub: usize,
    nsub: usize,
    pat: usize,
    npat: usize,
    tick: usize,
    init: usize,
}

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The code image of a file: the one code hunk behind the hunk headers.
fn image(d: &[u8]) -> Option<&[u8]> {
    if d.len() < 0x80 || be32(d, 0) != 0x3F3 || d[0x14] == 0 || be32(d, 0x18) != 0x3E9 {
        return None;
    }
    if be32(d, 0x20) != 0x70FF_4E75 || d.get(0x24..0x2C)? != b"H.DAVIES" {
        return None;
    }
    if (0x2C..0x38).step_by(4).any(|o| be32(d, o) == 0) {
        return None;
    }
    let words = (be32(d, 0x1C) as usize).checked_mul(4)?;
    if words != (be32(d, 0x14) as usize & 0x00FF_FFFF) * 4 {
        return None;
    }
    d.get(0x20..0x20usize.checked_add(words)?)
}

/// The end of an order list that starts at `p`: behind its terminating command.
fn order_end(img: &[u8], mut p: usize) -> Option<usize> {
    for _ in 0..img.len() {
        let b = *img.get(p)?;
        if b < 0x80 {
            p += 2;
        } else if b == 0xFF || b == 0xFD {
            return Some(p + 1);
        } else if b == 0xFE {
            return Some(p + 2);
        } else {
            return None;
        }
    }
    None
}

fn locate(d: &[u8]) -> Option<Lay> {
    let img = image(d)?;
    let n = img.len();
    let ptr = |o: usize| -> Option<usize> {
        let v = be32(img, o) as usize;
        (v != 0 && v < n).then_some(v)
    };
    let (tick, dmaon, init) = (ptr(0x0C)?, ptr(0x10)?, ptr(0x14)?);
    let _ = dmaon;
    let (samp, sampend, sub) = (ptr(0x18)?, ptr(0x1C)?, ptr(0x3C)?);
    // the period table and the DMA bit table behind it
    let per = (0..n.saturating_sub(4)).step_by(2).find(|&o| be32(img, o) == PERIOD_KEY)?;
    let dma = per + 172;
    if img.get(dma..dma + 4)? != [8, 4, 2, 1] {
        return None;
    }
    // the variant, from the distance to the envelope list
    if samp < 6 {
        return None;
    }
    let envl = samp - 6;
    if be32(img, envl) as usize != samp - 2 || img.get(samp - 2..samp)? != [0x00, 0xFF] {
        return None;
    }
    let code_ptrs = |from: usize, count: usize| (0..count).all(|i| ptr(from + 4 * i).is_some());
    let var = match envl.checked_sub(dma)? {
        32 => {
            if !code_ptrs(dma + 4, 5) || !code_ptrs(dma + 24, 2) {
                return None;
            }
            Var::A
        }
        36 if img.get(dma + 4..dma + 8)? == [0xF7, 0xFB, 0xFD, 0xFE] => {
            if !code_ptrs(dma + 8, 4) || !code_ptrs(dma + 24, 3) {
                return None;
            }
            Var::B
        }
        36 => {
            if !code_ptrs(dma + 4, 5) || !code_ptrs(dma + 24, 3) {
                return None;
            }
            Var::C
        }
        _ => return None,
    };
    // the sample table
    if sampend <= samp || (sampend - samp) % 16 != 0 {
        return None;
    }
    let nsamp = (sampend - samp) / 16;
    if nsamp > 256 || (var != Var::A && sub != sampend) || sub < sampend {
        return None;
    }
    for i in 0..nsamp {
        let e = samp + 16 * i;
        let (start, len) = (be32(img, e) as usize, be16(img, e + 4) as usize);
        let (lp, lplen) = (be32(img, e + 6) as usize, be16(img, e + 10) as usize);
        if i == 0 && start != 0 {
            return None;
        }
        if i > 0 && (start >= n || start + 2 * len > n) {
            return None;
        }
        if lp != 0 && (lp >= n || lp + 2 * lplen > n) {
            return None;
        }
        if be16(img, e + 12) > 0x40 || be16(img, e + 14) != 0 {
            return None;
        }
    }
    // the subsong table and the order lists
    let first = ptr(sub)?;
    if first <= sub || (first - sub) % 16 != 0 {
        return None;
    }
    let nsub = (first - sub) / 16;
    if nsub > 64 {
        return None;
    }
    let mut last = 0;
    for i in 0..4 * nsub {
        let o = ptr(sub + 4 * i)?;
        if o < first {
            return None;
        }
        last = last.max(o);
    }
    let end = order_end(img, last)?;
    // the pattern pointer table follows the last order list
    let pat = [end, (end + 1) & !1].into_iter().find(|&p| {
        let f = be32(img, p) as usize;
        f > p && f <= n && (f - p) % 4 == 0 && (f - p) / 4 <= 128
    })?;
    let f = be32(img, pat) as usize;
    let npat = (f - pat) / 4;
    for i in 0..npat {
        let v = be32(img, pat + 4 * i) as usize;
        if v < f || v >= n {
            return None;
        }
    }
    // every order list ends and names existing patterns
    for i in 0..4 * nsub {
        let mut p = be32(img, sub + 4 * i) as usize;
        loop {
            let b = rb(img, p);
            if b < 0x80 {
                if p + 1 >= n || b as usize >= npat {
                    return None;
                }
                p += 2;
            } else if (b >= 0xFD || b == 0xFE) && p + 1 < n {
                break;
            } else {
                return None;
            }
        }
    }
    Some(Lay { var, per, samp, nsamp, sub, nsub, pat, npat, tick, init })
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

/// The cycles the wrapper spends scanning the loaded image before the first init (specification
/// 11.1); `None` when the scan leaves the image.
fn scan_cycles(img: &[u8], tick: usize, init: usize) -> Option<u64> {
    let mut c = 44u64;
    for i in 0..401 {
        c += 47;
        if be32(img, tick + 2 * i) == 0x3D40_00A8 {
            c += 48;
        }
    }
    c += 16;
    let mut p = init;
    for _ in 0..img.len() {
        if p + 8 > img.len() {
            return None;
        }
        c += 29;
        if be32(img, p) == PERIOD_KEY {
            return Some(c);
        }
        c += 21;
        if be16(img, p) == 0x5407 {
            c += 12;
            p += 2;
        }
        c += 29;
        if be32(img, p) == 0x4ED0_2B6D {
            c += 64;
            p += 8;
        }
        c += 21;
        let stepped = be16(img, p);
        p += 2;
        if stepped == 0x00DF {
            c += 26;
            p += 2;
        }
    }
    None
}

// ---------------------------------------------------------------------------------
// Costs (68000 cycles), specification 10

/// Per channel: where the note start hook writes its pointer, and its total.
const NOTE_TOTAL: [u64; 4] = [229, 260, 291, 299];
/// The volume hook: write offset and total after VOLWRITE's 30.
const VOL_AT: [u64; 4] = [173, 190, 197, 224];
const VOL_TOTAL: [u64; 4] = [314, 362, 400, 435];
/// The note volume hook of variant B.
const NVOL_AT: [u64; 4] = [197, 214, 221, 248];
const NVOL_TOTAL: [u64; 4] = [350, 398, 436, 471];
/// The loop / jump / stop reporting hook.
const LOOP_TOTAL: [u64; 4] = [204, 231, 254, 267];

#[derive(Clone, Copy, Default)]
struct Ch {
    smp: u32,
    per: u16,
    len: u16,
    vol: u16,
    ivol: u16,
    maxv: u16,
    lp: u32,
    lplen: u16,
    envp: usize,
    envs: usize,
    delta: u16,
    count: u16,
    pat: usize,
    pats: usize,
    rep: u16,
    ord: usize,
    ords: usize,
    cnt: u8,
    dead: bool,
}

pub struct HowieDavies {
    d: Vec<u8>,
    base: u32,
    silent: u32,
    lay: Lay,
    title: String,
    sub: usize,
    ch: [Ch; 4],
    s: u16,
    c: u16,
    mask: u16,
    flags: [bool; 4],
    songend: bool,
    ticks: u64,
    resume: Option<u64>,
    // the clock of the call being modelled, and the cycles into it
    t0: u64,
    cy: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = locate(d)?;
    let img = image(d)?;
    let scan = scan_cycles(img, lay.tick, lay.init)?;
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    let title = {
        let o = be32(img, 0x20) as usize;
        img.get(o..).map_or(String::new(), |s| {
            s.iter().take_while(|&&b| (0x20..0x7F).contains(&b)).take(80).map(|&b| b as char).collect()
        })
    };
    let mut s = HowieDavies {
        d: img.to_vec(),
        base: base + 0x20,
        silent,
        lay,
        title,
        sub: 0,
        ch: [Ch::default(); 4],
        s: 6,
        c: 6,
        mask: 0xF,
        flags: [true; 4],
        songend: false,
        ticks: 0,
        resume: None,
        t0: 0,
        cy: 0,
    };
    // the first InitSound call falls at the time the start-up scan gives
    m.bus.clock = m.bus.clock.max(957 + scan / 2);
    let mut hw = Hw { bus: &mut m.bus };
    s.init(&mut hw, 0);
    Some(Box::new(s))
}

impl HowieDavies {
    fn w(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    fn line(&self, extra: u64) -> u64 {
        ((self.t0 + (self.cy + extra) / 2) / LINE % LINES) & 0xFF
    }

    fn addr(&self, off: u32) -> u32 {
        self.base.wrapping_add(off)
    }

    // --- InitSound and EndSound (specification 5, 8)

    fn init(&mut self, hw: &mut Hw, sub: usize) {
        self.sub = sub;
        self.songend = false;
        self.t0 = hw.bus.clock;
        self.cy = 1168;
        let var = self.lay.var;
        self.s = 6;
        self.c = 6;
        self.mask = 0xF;
        self.flags = [true; 4];
        let silent = self.silent;
        let env = self.env_data();
        for c in 0..4 {
            let o = self.lay.sub + 16 * sub + 4 * c;
            let ord = be32(&self.d, o) as usize;
            let pat0 = self.pattern(0);
            let ch = &mut self.ch[c];
            ch.dead = false;
            ch.ord = ord;
            ch.ords = ord;
            ch.pat = pat0;
            ch.pats = pat0;
            ch.rep = 0;
            ch.cnt = 0;
            ch.smp = silent;
            ch.len = 1;
            ch.vol = 0x40;
            ch.envp = env;
            ch.count = 1;
            ch.delta = 0;
            if var != Var::B {
                ch.maxv = 0x40;
                ch.lp = silent;
                ch.lplen = 1;
                ch.ivol = 0;
                ch.envs = env;
            }
        }
        match var {
            Var::A => {
                self.cy += 24;
                self.w(hw);
                hw.bus.write_byte(0xBFE001, 0x02);
                self.cy += 20;
                self.w(hw);
                hw.dmacon(0x000F);
                self.cy += 114;
                for c in 0..4 {
                    self.cy += 30 + 432;
                    self.w(hw);
                    hw.loc(c, silent);
                    self.cy += 16;
                    self.w(hw);
                    hw.len(c, 1);
                    self.cy += 48;
                }
                self.cy += 52;
            }
            Var::B => {
                self.cy += 40;
                for (c, add) in [(0, 12 + 16), (1, 16), (2, 4 + 16 + 16), (3, 16)] {
                    self.cy += add;
                    self.w(hw);
                    hw.loc(c, silent);
                }
                for c in 0..4 {
                    self.cy += 12;
                    self.w(hw);
                    hw.len(c, 1);
                }
                self.cy += 72 + 34 + 4 * (26 + 344) + 16;
            }
            Var::C => {
                self.cy += 106 + 4 * (30 + 404) + 16;
            }
        }
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    fn end_sound(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 28;
        self.w(hw);
        hw.dmacon(0x000F);
        self.cy += 4;
        for c in 0..4 {
            self.cy += 12;
            self.w(hw);
            hw.vol(c, 0);
        }
        self.cy += 16;
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    /// The offset of the envelope data (the list entry 0).
    fn env_data(&self) -> usize {
        be32(&self.d, self.lay.samp - 6) as usize
    }

    /// The offset of pattern `i`, if the table has it.
    fn pattern(&self, i: usize) -> usize {
        if i < self.lay.npat {
            be32(&self.d, self.lay.pat + 4 * i) as usize
        } else {
            self.d.len()
        }
    }

    // --- one interrupt (specification 6)

    fn interrupt(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 256;
        self.dma_on(hw);
        self.cy += 32;
        self.tick_routine(hw);
        self.cy += 168;
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    fn dma_on(&mut self, hw: &mut Hw) {
        let var = self.lay.var;
        self.cy += if var == Var::B { 44 } else { 40 };
        self.w(hw);
        hw.dmacon(0x8000 | self.mask);
        if var == Var::B {
            self.cy += 12;
            let target = (self.line(0) + 8) & 0xFF;
            self.cy += 4;
            for _ in 0..10_000 {
                self.cy += 12;
                let l = self.line(0);
                self.cy += 9;
                if l == target {
                    break;
                }
            }
            self.cy += 40;
        } else {
            self.cy += 66;
            for _ in 0..9 {
                self.cy += 16;
                let reference = self.line(0);
                for _ in 0..1000 {
                    self.cy += 16;
                    let l = self.line(0);
                    self.cy += 9;
                    if l != reference {
                        break;
                    }
                }
                self.cy += 10;
            }
            self.cy += 60 + 4 + 60;
        }
        for c in 0..4 {
            self.cy += 31;
            if self.mask & (1 << c) != 0 {
                let ch = self.ch[c];
                if var == Var::B {
                    self.cy += 20 + 24;
                    self.w(hw);
                    hw.loc(c, self.silent);
                    self.cy += 16;
                    self.w(hw);
                    hw.len(c, 1);
                    self.cy += 24;
                } else {
                    self.cy += 61;
                    let lp = if ch.lp == 0 {
                        self.cy += 12;
                        self.silent
                    } else {
                        ch.lp
                    };
                    self.w(hw);
                    hw.loc(c, lp);
                    self.cy += 20;
                    self.w(hw);
                    hw.len(c, ch.lplen);
                    self.cy += 28;
                }
            }
            self.cy += 26;
        }
        self.cy += 16;
    }

    fn tick_routine(&mut self, hw: &mut Hw) {
        let var = self.lay.var;
        self.cy += if var == Var::B { 39 } else { 47 };
        self.c = self.c.wrapping_add(1);
        if self.c >= self.s {
            self.cy += 18;
            self.step(hw);
            self.cy += 4;
            self.c = 0;
        }
        self.cy += if var == Var::B { 68 } else { 76 };
        for c in 0..4 {
            self.cy += 31;
            if self.mask & (1 << c) != 0 {
                self.cy += 18;
                self.env(hw, c);
                if var != Var::A {
                    self.cy += 23;
                }
            }
            self.cy += 26;
        }
        self.cy += 16;
    }

    fn step(&mut self, hw: &mut Hw) {
        self.cy += if self.lay.var == Var::B { 52 } else { 60 };
        for c in 0..4 {
            self.cy += 31;
            if self.mask & (1 << c) != 0 {
                self.cy += 18;
                self.chan_step(hw, c);
            }
            self.cy += 26;
        }
        self.cy += 16;
    }

    fn chan_step(&mut self, hw: &mut Hw, c: usize) {
        self.cy += 27;
        if self.ch[c].dead {
            self.cy += 16;
            return;
        }
        self.ch[c].cnt = self.ch[c].cnt.wrapping_sub(1);
        if self.ch[c].cnt & 0x80 == 0 {
            self.cy += 16;
            return;
        }
        self.events(hw, c);
    }

    // --- volumes

    /// The wrapper's volume write: the module's value masked to seven bits.
    fn volwrite(&mut self, hw: &mut Hw, c: usize, v: u16) {
        self.cy += 30;
        let e = self.cy;
        self.cy = e + VOL_AT[c];
        self.w(hw);
        hw.vol(c, v & 0x7F);
        self.cy = e + VOL_TOTAL[c];
    }

    /// The output volume of A and C (specification 6.4 step 2); adds the clamp's cycles.
    fn clamped(&mut self, c: usize) -> u16 {
        let ch = self.ch[c];
        let mut out = ch.vol;
        if ch.vol >= ch.maxv {
            self.cy += 12;
            out = ch.maxv;
        }
        if self.lay.var == Var::C {
            self.cy += 27;
            if out >= 0x40 {
                self.cy += 16;
                out = 0x40;
            }
        }
        out
    }

    /// Reads the next envelope pair; true when it was the end marker.
    fn env_pair(&mut self, c: usize) -> bool {
        let p = self.ch[c].envp;
        let (delta, count) = (rb(&self.d, p), rb(&self.d, p + 1));
        let ch = &mut self.ch[c];
        ch.envp = p + 2;
        ch.delta = delta as i8 as i16 as u16;
        if count == 0xFF {
            ch.count = 0xFFFF;
            true
        } else {
            ch.count = count as u16;
            false
        }
    }

    fn env(&mut self, hw: &mut Hw, c: usize) {
        self.cy += 27;
        if self.lay.var == Var::B {
            if self.ch[c].count != 0xFFFF {
                self.cy += 55;
                let ch = &mut self.ch[c];
                ch.vol = ch.vol.wrapping_add(ch.delta);
                ch.count = ch.count.wrapping_sub(1);
                if ch.count == 0 {
                    self.cy += 67;
                    if self.env_pair(c) {
                        self.cy += 48;
                        return;
                    }
                    self.cy += 46;
                }
            }
            self.cy += 12;
            let v = self.ch[c].vol;
            self.volwrite(hw, c, v);
            self.cy += 16;
            return;
        }
        if self.ch[c].count == 0xFFFF {
            self.cy += 16;
            return;
        }
        self.cy += 63;
        let ch = &mut self.ch[c];
        ch.vol = ch.vol.wrapping_add(ch.delta);
        let out = self.clamped(c);
        self.volwrite(hw, c, out);
        self.cy += 27;
        let ch = &mut self.ch[c];
        ch.count = ch.count.wrapping_sub(1);
        if ch.count != 0 {
            self.cy += 16;
            return;
        }
        self.cy += 67;
        self.cy += if self.env_pair(c) { 48 } else { 52 };
    }

    // --- events (specification 7)

    fn park(&mut self, c: usize) {
        self.ch[c].dead = true;
    }

    fn events(&mut self, hw: &mut Hw, c: usize) {
        let var = self.lay.var;
        for _ in 0..EVENT_CAP {
            self.cy += 35;
            let p = self.ch[c].pat;
            let b = rb(&self.d, p);
            if b < 0x80 {
                self.note(hw, c, b, rb(&self.d, p + 1));
                return;
            }
            self.ch[c].pat = p + 1;
            self.cy += if var == Var::B { 60 } else { 64 };
            let operand = rb(&self.d, p + 1);
            match 0xFF - b {
                0 => {
                    self.cy += 27;
                    let ch = &mut self.ch[c];
                    ch.rep = ch.rep.wrapping_sub(1);
                    if ch.rep & 0x8000 == 0 {
                        self.cy += 38;
                        ch.pat = ch.pats;
                    } else if !self.order(hw, c) {
                        return;
                    }
                }
                1 => {
                    self.cy += if var == Var::B { 280 } else { 288 };
                    self.instrument(c, operand as usize);
                    self.ch[c].pat = p + 2;
                }
                2 => {
                    self.cy += 46;
                    self.s = (self.s & 0xFF00) | operand as u16;
                    self.ch[c].pat = p + 2;
                }
                3 => {
                    self.cy += 48;
                    self.ch[c].cnt = operand;
                    self.ch[c].pat = p + 2;
                    return;
                }
                4 if var != Var::B => {
                    self.cy += 42;
                    let ch = &mut self.ch[c];
                    ch.maxv = (ch.maxv & 0xFF00) | operand as u16;
                    ch.pat = p + 2;
                }
                _ => {
                    self.park(c);
                    return;
                }
            }
        }
        self.park(c);
    }

    fn instrument(&mut self, c: usize, n: usize) {
        let e = self.lay.samp + 16 * n;
        let (start, len) = (be32(&self.d, e), be16(&self.d, e + 4));
        let (lp, lplen) = (be32(&self.d, e + 6), be16(&self.d, e + 10));
        let vol = be16(&self.d, e + 12);
        let envn = be16(&self.d, e + 14) as usize;
        let env = if envn == 0 { self.env_data() } else { self.d.len() };
        let smp = if n == 0 || n >= self.lay.nsamp {
            self.silent
        } else {
            self.addr(start)
        };
        let lpa = if lp == 0 { 0 } else { self.addr(lp) };
        let ch = &mut self.ch[c];
        ch.smp = smp;
        ch.len = if n >= self.lay.nsamp { 1 } else { len };
        ch.lp = lpa;
        ch.lplen = if n >= self.lay.nsamp { 1 } else { lplen };
        ch.vol = vol;
        ch.ivol = vol;
        ch.envp = env;
        ch.envs = env;
    }

    fn hook(&mut self, c: usize) {
        self.cy += 20 + LOOP_TOTAL[c];
        self.flags[c] = false;
        if self.flags.iter().all(|&f| !f) {
            self.flags = [true; 4];
            self.cy += if self.lay.var == Var::B { 84 } else { 83 };
            self.songend = true;
        }
    }

    /// Fetches the next order list entry; false when the channel's events end.
    fn order(&mut self, hw: &mut Hw, c: usize) -> bool {
        let _ = hw;
        let var = self.lay.var;
        for _ in 0..EVENT_CAP {
            self.cy += 35;
            let o = self.ch[c].ord;
            let b = rb(&self.d, o);
            if b < 0x80 {
                if b as usize >= self.lay.npat {
                    self.park(c);
                    return false;
                }
                self.cy += 142;
                let p = self.pattern(b as usize);
                let ch = &mut self.ch[c];
                ch.pat = p;
                ch.pats = p;
                ch.rep = rb(&self.d, o + 1) as u16;
                ch.ord = o + 2;
                return true;
            }
            self.cy += if var == Var::B { 60 } else { 64 };
            match 0xFF - b {
                0 => {
                    self.ch[c].ord = self.ch[c].ords;
                    self.hook(c);
                    self.cy += 10;
                }
                1 => {
                    let q = o + 1;
                    let n = rb(&self.d, q) as usize;
                    let back = if var == Var::A { n } else { n + 1 };
                    if back > q {
                        self.park(c);
                        return false;
                    }
                    self.ch[c].ord = q - back;
                    self.hook(c);
                    self.cy += if var == Var::A { 42 } else { 46 };
                }
                2 if var == Var::B => {
                    self.ch[c].ord = self.ch[c].ords;
                    self.hook(c);
                    self.cy += 64;
                    self.mask &= !(1 << c);
                }
                2 if var == Var::C => {
                    // the dead end: the order pointer stays, the pattern pointer goes back onto the
                    // end-of-pattern command and the counter is left as it was
                    self.cy += 16;
                    let ch = &mut self.ch[c];
                    ch.pat = ch.pat.saturating_sub(1);
                    return false;
                }
                _ => {
                    self.park(c);
                    return false;
                }
            }
        }
        self.park(c);
        false
    }

    /// A note start (specification 7.1): note byte `b`, duration `dur`.
    fn note(&mut self, hw: &mut Hw, c: usize, b: u8, dur: u8) {
        let var = self.lay.var;
        let period = be16(&self.d, self.lay.per + 2 * b as usize);
        {
            let ch = &mut self.ch[c];
            ch.per = period;
            ch.cnt = dur;
            ch.pat += 2;
        }
        self.cy += match var {
            Var::A => 132,
            Var::B => 147,
            Var::C => 186,
        };
        self.w(hw);
        hw.dmacon(1 << c);
        self.cy += 80;
        {
            let ch = &mut self.ch[c];
            ch.envp = ch.envs;
            ch.vol = ch.ivol;
            ch.count = 1;
            ch.delta = 0;
        }
        self.cy += 20;
        let e = self.cy;
        let ch = self.ch[c];
        self.cy = e + 24;
        self.w(hw);
        hw.loc(c, ch.smp);
        self.cy = e + 44;
        self.w(hw);
        hw.per(c, ch.per);
        self.cy = e + 64;
        self.w(hw);
        hw.len(c, ch.len);
        self.cy = e + NOTE_TOTAL[c];
        self.cy += 24;
        if var == Var::B {
            self.cy += 20;
            let e = self.cy;
            self.cy = e + NVOL_AT[c];
            self.w(hw);
            hw.vol(c, ch.vol & 0x7F);
            self.cy = e + NVOL_TOTAL[c];
            self.cy += 16;
        } else {
            self.cy += 35;
            let out = self.clamped(c);
            self.volwrite(hw, c, out);
            self.cy += 16;
        }
    }
}

impl Replayer for HowieDavies {
    fn format(&self) -> &'static str {
        "Howie Davies"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        FRAME
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.nsub as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.lay.nsub - 1);
        self.end_sound(hw);
        hw.bus.clock += 236;
        self.init(hw, n);
        if self.ticks == 0 {
            self.resume = Some(hw.bus.clock + FRAME);
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticks += 1;
        self.interrupt(hw);
    }
}

#[cfg(test)]
#[path = "howiedavies_tests.rs"]
mod tests;
