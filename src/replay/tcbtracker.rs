//! TCB Tracker (`TCB.*`), by Anders "AN Cool" Nilsson (Atari ST, 1990, ported to the
//! Amiga; used for instance by Richard Joseph on Cannon Fodder).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's TCB Tracker player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A four-voice tracker with its own engine: an order list of 64-row patterns,
//! unsigned 8-bit samples (converted to signed when loaded), a pitch table of
//! 32-bit values from which the period is computed on every tick, and "save" and
//! "restore" effects that cut a voice and later restart it from the point it had
//! reached. Every tick writes every voice's period and volume, and a new note
//! restarts its channel with the DMA switched off and on. 50 Hz, one subsong, the
//! speed is fixed by the file. Two flavours (`AN COOL!` and `AN COOL.`) differ in
//! the period base and in the pitch-slide effects, which only the second has.
//!
//! Every write is at the original's colour clock: the tick's start plus half the 68000 cycles
//! of the code paths taken (`COSTS`, fitted to the reference's write times), with the restart's
//! raster-line and audio-interrupt waits polled as the reference's writes show them.

use crate::paula::LINE;
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "TCB Tracker", detect, new };

/// The pitch of each of the 36 notes, as a 32-bit value (octave 1 first).
const PTAB: [u32; 36] = [
    0x8000, 0x879C, 0x8FAC, 0x9837, 0xA145, 0xAADC, 0xB504, 0xBFC8, 0xCB2F, 0xD744, 0xE411, 0xF1A1, 0x10000, 0x10F38,
    0x11F59, 0x1306F, 0x1428A, 0x155B8, 0x16A09, 0x17F91, 0x1965F, 0x1AE89, 0x1C823, 0x1E343, 0x20000, 0x21E71, 0x23EB3,
    0x260DF, 0x28514, 0x2AB70, 0x2D413, 0x2FF22, 0x32CBF, 0x35D13, 0x39047, 0x3C686,
];

/// The pitch table with its sentinels: the entry before the first and the one after the last.
fn pitch(i: i32) -> i64 {
    if i < 0 {
        0x78D0
    } else if i > 35 {
        0x40000
    } else {
        PTAB[i as usize] as i64
    }
}

// ---------------------------------------------------------------------------------
// Recognition

/// What the header says: flavour A (`!`), the offsets of the patterns and of the
/// instrument block, and the total size of the latter with its samples.
struct Head {
    a: bool,
    np: usize,
    p0: usize,
    s0: usize,
    total: usize,
}

fn head(d: &[u8]) -> Option<Head> {
    if d.len() < 0x132 || d[..7] != *b"AN COOL" {
        return None;
    }
    let a = match d[7] {
        b'!' => true,
        b'.' => false,
        _ => return None,
    };
    let np = be32(d, 8) as usize;
    let l = d[0x8E];
    if np > 127 || d[12] > 15 || d[13] != 0 || l == 0 || l > 127 {
        return None;
    }
    let p0 = if a { 0x110 } else { 0x132 };
    let s0 = p0 + np * 0x200;
    if d.len() <= s0 + 0xD4 || be32(d, s0 + 0xCC) != 0xFFFF_FFFF || be32(d, s0 + 0xD0) != 0 || be32(d, s0 + 0x44) != 0xD4 {
        return None;
    }
    let total = be32(d, s0) as usize;
    if total < 0xD4 || s0 + total > d.len() {
        return None;
    }
    Some(Head { a, np, p0, s0, total })
}

fn detect(d: &[u8]) -> bool {
    head(d).is_some()
}

// ---------------------------------------------------------------------------------
// The player

struct Voice {
    adr: i64,
    len: i32,
    period: u32,
    volume: i32,
    rep: i64,
    replen: i32,
    flag: u8,
    idx: i32,
    cnt: u32,
    saved_adr: i64,
    saved_len: i32,
    /// The address and length last put in the registers, the period last computed.
    reg_adr: i64,
    reg_len: i32,
    hw_per: u16,
}

impl Voice {
    fn new() -> Voice {
        Voice {
            adr: 0,
            len: -1,
            period: 0,
            volume: -1,
            rep: 0,
            replen: -1,
            flag: 0,
            idx: 0,
            cnt: 0,
            saved_adr: 0,
            saved_len: 0,
            reg_adr: 0,
            reg_len: 0,
            hw_per: 0,
        }
    }
}

struct Tcb {
    /// The file with its samples converted to signed.
    img: Vec<u8>,
    base: u32,
    title: String,
    a: bool,
    p0: usize,
    s0: usize,
    /// The period base constant.
    pbase: u32,
    songlen: u8,
    sp: u8,
    c: u8,
    pos: u8,
    r: i32,
    pat: usize,
    v: [Voice; 4],
    songend: bool,
    /// The tick's start (colour clock), the original's 68000 cycles since then and the voice the
    /// paths are on (4: none), for the writes' times.
    t0: u64,
    cy: i64,
    vk: usize,
}

impl Tcb {
    /// A path of the original's code taken: its cycles, on the current voice.
    fn k(&mut self, k: usize) {
        self.cy += COSTS[k][self.vk] as i64;
    }
    /// `n` times the path `k`.
    fn kn(&mut self, k: usize, n: u32) {
        self.cy += COSTS[k][self.vk] as i64 * n as i64;
    }
    /// The colour clock the original is at now.
    fn now(&self) -> u64 {
        (self.t0 as i64 + self.cy.div_euclid(2)) as u64
    }
    fn sync(&mut self, hw: &mut Hw) {
        let t = self.now();
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
    }
    /// A Paula write of kind `k` (the path to it): at the tick's start plus half the cycles so far.
    fn at(&mut self, k: usize, hw: &mut Hw) {
        self.k(k);
        self.sync(hw);
    }
    /// An INTREQ write (not traced: its paths go with the next audio write).
    fn intreq(&mut self, k: usize, v: u16, hw: &mut Hw) {
        self.k(k);
        self.sync(hw);
        hw.bus.write_word(0xDFF09C, v);
    }
    /// A wait for the next raster line: the line read after the path `w` and `K_WP`'s [3]
    /// cycles, then again every [0] cycles until it has changed, then the position in the line
    /// read every [1] cycles until it is [2] colour clocks or more, then the path `x`.
    fn wait_line(&mut self, w: usize, x: usize) {
        self.k(w);
        let l0 = self.now() / LINE;
        let [p1, p2, th, a, _] = COSTS[K_WP];
        self.cy += a as i64;
        while self.now() / LINE == l0 {
            self.cy += p1 as i64;
        }
        loop {
            self.cy += p2 as i64;
            if self.now() % LINE >= th as u64 {
                break;
            }
        }
        self.k(x);
    }
    /// A wait for Paula's interrupt request bits `m`: a first read after the path `w`, then one
    /// every `w + 1`'s last column of cycles until they are set, then the path `x`.
    fn wait_irq(&mut self, w: usize, x: usize, m: u16, hw: &mut Hw) {
        self.k(w);
        // the colour clock Paula raises the request at (it stays set: only a write clears it)
        let mut t = self.now();
        let lim = t + 4 * 65536;
        loop {
            hw.bus.paula.sync(t, &hw.bus.ram);
            if hw.bus.paula.intreq & m == m || t > lim {
                break;
            }
            t += 1;
        }
        let poll = COSTS[w + 1][4] as i64;
        while self.now() < t {
            self.cy += poll;
        }
        self.k(x);
    }

    fn rb(&self, o: i64) -> u8 {
        if o < 0 {
            return 0;
        }
        self.img.get(o as usize).copied().unwrap_or(0)
    }

    fn rl(&self, o: i64) -> u32 {
        if o < 0 {
            return 0;
        }
        be32(&self.img, o as usize)
    }

    /// The hardware period of a pitch value.
    fn hw_period(&self, period: u32) -> u16 {
        let d = (period >> 3) & 0xFFFF;
        if d == 0 {
            return 1;
        }
        ((self.pbase / d) & 0xFFFF) as u16 + 1
    }

    /// The slide of a note's pitch by effect `eff` (flavour B), once per row.
    fn slide(&mut self, v: usize, eff: usize) {
        let a = be16(&self.img, 0x112 + 2 * eff) as i16 as i32;
        let mut period = self.v[v].period as i64;
        let mut idx = self.v[v].idx;
        // a signed 16-bit product divided by 800; if the quotient does not fit 16 bits
        // the low half of the product stands in for it
        let q = |d: i64| -> (i64, bool) {
            let d16 = d as i16 as i32;
            let prod = a.wrapping_mul(d16);
            let qq = prod / 800;
            if qq as i16 as i32 == qq {
                (qq as i64, false)
            } else {
                (prod as i16 as i64, true)
            }
        };
        let (i0, cl, ov);
        if a >= 0 {
            let (d, o) = q(pitch(idx) - pitch(idx - 1));
            ov = o;
            period -= d;
            i0 = !(pitch(idx - 1) < period);
            if i0 {
                idx -= 1;
            }
            cl = !(period > 0x8000);
            if cl {
                period = 0x8000;
                idx = 0;
            }
        } else {
            self.k(K_SLNEG);
            let (d, o) = q(pitch(idx + 1) - pitch(idx));
            ov = o;
            period -= d;
            i0 = !(pitch(idx + 1) > period);
            if i0 {
                idx += 1;
            }
            cl = !(period < 0x3C686);
            if cl {
                period = 0x3C686;
                idx = 35;
            }
        }
        if ov {
            self.k(K_SLOV);
        }
        if i0 {
            self.k(K_SLIDX);
        }
        if cl {
            self.k(K_SLCL);
        }
        let voice = &mut self.v[v];
        voice.period = period as u32;
        voice.idx = idx;
    }

    fn row_voice(&mut self, v: usize, hw: &mut Hw) {
        self.vk = v;
        self.k(K_RV);
        let cell = (self.p0 + self.pat * 0x200 + self.r as usize * 8 + v * 2) as i64;
        let (note, b1) = (self.rb(cell), self.rb(cell + 1));
        let eff = b1 & 15;
        let bit = 1u16 << v;
        if note != 0 {
            self.k(K_RN);
            let (oct, semi) = (note >> 4, note & 15);
            let ok = (1..=3).contains(&oct) && semi < 12;
            if !ok {
                self.k(K_RNBAD);
            }
            let k = if ok { (oct as i32 - 1) * 12 + semi as i32 } else { 0 };
            let ins = (b1 >> 4) as i64;
            let s0 = self.s0 as i64;
            let rec = |o: i64| s0 + o;
            let vb = self.rb(rec(4 + 4 * ins));
            let rep = (((self.rb(rec(4 + 4 * ins + 2)) as u32) << 8) | self.rb(rec(4 + 4 * ins + 3)) as u32) as i64;
            let start = self.rl(rec(0x44 + 8 * ins)) as i64;
            let len = self.rl(rec(0x44 + 8 * ins + 4)) as i32;
            // an instrument of length 1 is empty: the original crashes on it, here the note is dropped
            if len == 1 {
                self.k(K_RL1);
            } else {
                let adr = s0 + start;
                let end = adr + len as i64;
                {
                    let voice = &mut self.v[v];
                    voice.flag = 0;
                    voice.cnt = 0;
                    voice.idx = k;
                    voice.period = PTAB[k as usize];
                    voice.adr = adr;
                    voice.len = len;
                    voice.replen = rep as i32;
                }
                if rep == 0 {
                    self.k(K_RR0);
                    let e = end & !1;
                    let mut n = 0;
                    let mut pad = true;
                    for o in [e - 4, e - 204, e - 404, e - 604] {
                        n += 1;
                        if !(o >= 0 && self.rl(o) == 0xFFFF_FFFF) {
                            pad = false;
                            break;
                        }
                    }
                    self.kn(K_RPADC, n);
                    if pad {
                        self.k(K_RPADY);
                        self.v[v].len -= 700;
                        if self.a {
                            self.v[v].len -= 212;
                        }
                    }
                } else {
                    self.k(K_RR);
                    let e = end - 700 - if self.a { 212 } else { 0 };
                    let r0 = e - rep;
                    let mut n = 0;
                    let mut same = true;
                    for i in 0..6 {
                        n += 1;
                        if self.rb(r0 + i) != self.rb(e + i) {
                            same = false;
                            break;
                        }
                    }
                    self.kn(K_RSAMEC, n);
                    if same {
                        self.k(K_RSAME);
                    }
                    self.v[v].rep = if same { r0 } else { r0 + 700 };
                }
                if vb == 0 {
                    self.k(K_RVB0);
                }
                self.v[v].volume = if vb == 0 { 0 } else { (vb as i32 + 1) >> 1 };
            }
        }
        self.k(K_REFF + eff as usize);
        match eff {
            1..=10 if !self.a => self.slide(v, eff as usize),
            11 => {
                let dv = (self.v[v].period >> 3) & 0xFFFF;
                if dv != 0 {
                    let q = (self.pbase / dv) & 0xFFFF;
                    let ap = (q + 1) & 0xFFFF;
                    if ap == 0 {
                        self.k(K_E11AP0);
                    }
                    let n = if ap == 0 { 0 } else { ((70938 / ap) + 1) & !1 & 0xFFFF };
                    let sp = self.sp as u32;
                    let voice = &mut self.v[v];
                    let adv = n as i64 * ((sp * voice.cnt) & 0xFFFF) as i64;
                    voice.saved_adr = voice.adr + adv;
                    voice.saved_len = (voice.len as i64 - adv) as i32;
                } else {
                    self.k(K_E11DV0);
                }
                self.v[v].cnt = 0;
                let w = be16(&self.img, 0x128);
                if self.a || w != 2 {
                    self.k(K_E11CUT);
                    self.at(K_WE11, hw);
                    hw.dmacon(bit);
                    self.v[v].flag = 0xFF;
                }
            }
            12 => {
                let voice = &mut self.v[v];
                voice.adr = voice.saved_adr;
                voice.len = voice.saved_len;
                voice.flag = 0;
            }
            13 => self.r = 0x3F,
            _ => {}
        }
        self.v[v].cnt += 1;
    }

    fn play_voice(&mut self, v: usize, hw: &mut Hw) {
        self.vk = v;
        self.k(K_PV);
        let empty = (self.s0 + 0xD0) as i64;
        let hw_per = if self.v[v].period != 0 { Some(self.hw_period(self.v[v].period)) } else { None };
        let base = self.base as i64;
        if self.v[v].len >= 0 {
            self.k(K_PLEN);
            let voice = &mut self.v[v];
            if voice.len == 0 {
                voice.reg_adr = empty;
                voice.reg_len = 2;
                self.k(K_PLEN0);
            } else {
                voice.reg_adr = voice.adr;
                voice.reg_len = voice.len;
            }
        }
        if let Some(p) = hw_per {
            self.v[v].hw_per = p;
            if (self.v[v].period >> 3) & 0xFFFF == 0 {
                self.k(K_PDIV0);
            }
            self.at(K_WPER, hw);
            hw.per(v, p);
        } else {
            self.k(K_PNOPER);
        }
        if self.v[v].volume >= 0 {
            let vol = self.v[v].volume as u16;
            self.at(K_WVOL, hw);
            hw.vol(v, vol);
        } else {
            self.k(K_PNOVOL);
        }
        if self.v[v].replen >= 0 {
            self.k(K_PREP);
            let replen = self.v[v].replen;
            let (la, ll) = if replen == 0 { (empty, 2) } else { (self.v[v].rep, replen) };
            if replen == 0 {
                self.k(K_PREP0);
            }
            if self.v[v].flag == 0 {
                let bit = 1u16 << v;
                let run = hw.bus.paula.dmacon & bit != 0;
                let m = if run { bit } else { 0 };
                self.wait_line(K_W1, K_W1X);
                self.at(K_WPER1, hw);
                hw.per(v, 1);
                if run {
                    self.k(K_RUN);
                }
                self.intreq(K_WIRQ1, m << 7, hw);
                self.at(K_WOFF, hw);
                hw.dmacon(m);
                if run {
                    self.wait_irq(K_Q1, K_Q1X, m << 7, hw);
                }
                let (ra, rl, hp) = (self.v[v].reg_adr, self.v[v].reg_len, self.v[v].hw_per);
                self.at(K_WLOC, hw);
                hw.loc(v, (base + ra) as u32);
                self.at(K_WLEN, hw);
                hw.len(v, (rl >> 1) as u16);
                self.at(K_WPER2, hw);
                hw.per(v, hp);
                self.wait_line(K_W2, K_W2X);
                self.intreq(K_WIRQ2, bit << 7, hw);
                self.at(K_WON, hw);
                hw.dmacon(0x8000 | bit);
                self.wait_irq(K_Q2, K_Q2X, bit << 7, hw);
                self.wait_line(K_W3, K_W3X);
                self.at(K_WLOC2, hw);
                hw.loc(v, (base + la) as u32);
                self.at(K_WLEN2, hw);
                hw.len(v, (ll >> 1) as u16);
                self.v[v].flag = 0xFF;
            }
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 0;
        self.vk = 4;
        self.k(K_TICK);
        self.c = self.c.wrapping_sub(1);
        if self.c == 0 {
            self.k(K_ROW);
            self.r = (self.r + 1) & 0x3F;
            if self.r == 0 {
                self.k(K_RNEXT);
                self.pos = self.pos.wrapping_add(1);
                if self.pos == self.songlen {
                    self.k(K_SEND);
                    self.songend = true;
                    self.pos = 0;
                }
                self.pat = self.img.get(14 + self.pos as usize).copied().unwrap_or(0) as usize;
            }
            for v in 0..4 {
                self.row_voice(v, hw);
            }
            self.vk = 4;
            self.k(K_ROWEND);
            self.c = self.sp;
        } else {
            self.k(K_NOROW);
        }
        for v in 0..4 {
            self.play_voice(v, hw);
        }
    }
}

// The original's code paths, for the writes' times (cycles per voice; column 4: no voice).
const K_TICK: usize = 0;
const K_ROW: usize = 1;
const K_NOROW: usize = 2;
const K_RNEXT: usize = 3;
const K_SEND: usize = 4;
const K_ROWEND: usize = 5;
const K_RV: usize = 10;
const K_RN: usize = 11;
const K_RNBAD: usize = 12;
const K_RL1: usize = 13;
const K_RR0: usize = 14;
const K_RPADC: usize = 15;
const K_RPADY: usize = 16;
const K_RR: usize = 17;
const K_RSAMEC: usize = 18;
const K_RSAME: usize = 19;
const K_RVB0: usize = 20;
/// 21..36: effect 0..15
const K_REFF: usize = 21;
const K_SLNEG: usize = 37;
const K_SLIDX: usize = 38;
const K_SLCL: usize = 39;
const K_SLOV: usize = 40;
const K_E11DV0: usize = 41;
const K_E11AP0: usize = 42;
const K_E11CUT: usize = 43;
const K_WE11: usize = 44;
const K_PV: usize = 50;
const K_PLEN: usize = 51;
const K_PLEN0: usize = 52;
const K_PDIV0: usize = 53;
const K_WPER: usize = 54;
const K_PNOPER: usize = 55;
const K_WVOL: usize = 56;
const K_PNOVOL: usize = 57;
const K_PREP: usize = 58;
const K_PREP0: usize = 59;
/// 60, 61: the line wait before the restart (path, poll)
const K_W1: usize = 60;
const K_WPER1: usize = 62;
const K_RUN: usize = 63;
const K_WIRQ1: usize = 64;
const K_WOFF: usize = 65;
/// 66, 67: the wait for the channel's interrupt after its DMA is off
const K_Q1: usize = 66;
const K_WLOC: usize = 68;
const K_WLEN: usize = 69;
const K_WPER2: usize = 70;
/// 71, 72: the line wait before the DMA on
const K_W2: usize = 71;
const K_WIRQ2: usize = 73;
const K_WON: usize = 74;
/// 75, 76: the wait for the channel's interrupt after its DMA is on
const K_Q2: usize = 75;
const K_WLOC2: usize = 77;
const K_WLEN2: usize = 78;
const K_W1X: usize = 80;
const K_Q1X: usize = 81;
const K_W2X: usize = 82;
const K_Q2X: usize = 83;
const K_W3: usize = 84;
const K_W3X: usize = 86;
/// The line waits' polls: the line read every [0] cycles, then the position every [1] until [2].
const K_WP: usize = 87;
const NK: usize = 90;
/// Each path's 68000 cycles, per voice (the last column: no voice, and the polls of the two
/// interrupt waits, rows 67 and 76). Integer costs fitted exactly to the reference's write
/// times; row `K_WP` holds the line waits' polls. Some paths share their cycles with others
/// the fit cannot tell apart, which puts them on one voice.
const COSTS: [[i32; 5]; NK] = [
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 31],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [147, 147, 147, 147, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [97, 97, 97, 97, 0],
    [2, 2, 2, 2, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 713, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [117, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [667, 134, 134, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [231, 231, 231, 365, 0],
    [0, 0, 0, 0, 0],
    [307, 355, 393, 428, 0],
    [0, 0, 0, 0, 0],
    [35, 35, 35, 0, 0],
    [20, 20, 20, -22, 0],
    [70, 70, 70, 147, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [64, 64, 64, 64, 0],
    [12, 12, 12, 12, 0],
    [0, 0, 0, 0, 29],
    [41, 41, 41, 41, 0],
    [22, 22, 22, 22, 0],
    [12, 12, 12, 12, 0],
    [137, 168, 199, 207, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [12, 12, 12, 12, 0],
    [0, 0, 0, 0, 29],
    [0, 0, 0, 0, 0],
    [22, 22, 22, 22, 0],
    [0, 0, 0, 0, 0],
    [25, 25, 25, 25, 0],
    [-12, -12, -12, -12, 0],
    [85, 85, 85, 85, 0],
    [29, 29, 29, 29, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
    [21, 21, 21, 21, 0],
    [21, 25, 22, 12, 0],
    [0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0],
];

impl Replayer for Tcb {
    fn format(&self) -> &'static str {
        "TCB Tracker"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        Tcb::tick(self, hw);
    }
}

/// The player on module `d` loaded at `base`; the converted samples are returned
/// with it for the caller to put in guest memory.
fn start(d: &[u8], base: u32, title: String) -> Option<Tcb> {
    let h = head(d)?;
    let mut img = d.to_vec();
    // a repair the original makes to one known module
    if h.s0 + h.total == 51825 && be32(&img, 4494) == 0x1289 {
        let a = be32(&img, 4494).wrapping_add(8);
        img[4494..4498].copy_from_slice(&a.to_be_bytes());
        let b = be32(&img, 4498).wrapping_sub(8);
        img[4498..4502].copy_from_slice(&b.to_be_bytes());
    }
    for b in &mut img[h.s0 + 0xD4..h.s0 + h.total] {
        *b ^= 0x80;
    }
    let _ = h.np;
    Some(Tcb {
        base,
        title,
        a: h.a,
        p0: h.p0,
        s0: h.s0,
        pbase: if h.a || img[0x91] != 0 { 3_460_228 } else { 2_867_490 },
        songlen: img[0x8E],
        sp: 16 - img[12],
        c: 1,
        pos: 0xFF,
        r: -1,
        pat: 0,
        v: std::array::from_fn(|_| Voice::new()),
        songend: false,
        t0: 0,
        cy: 0,
        vk: 4,
        img,
    })
}

/// Colour clocks from the module's load to the song's start: the original's check and init, which
/// converts the samples to signed four bytes at a time (43 cycles) and the last one to three bytes
/// one at a time (31 cycles each), and takes 8 cycles less for every instrument of length 1
/// (measured on the 15 corpus modules and on copies cut short by one to a thousand bytes).
fn start_time(d: &[u8], h: &Head) -> u64 {
    let n = (h.total - 0xD4) as u64;
    let len1 = (0..16).filter(|&i| be32(d, h.s0 + 0x48 + 8 * i) == 1).count() as u64;
    (START_CYCLES + 43 * (n / 4) + 31 * (n % 4) - 8 * len1) / 2
}
const START_CYCLES: u64 = 4514;

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let title = m.bus.read_cstr(m.deli.file_buf);
    let t = start(d, base, title)?;
    let h = head(d)?;
    // the samples, converted to signed, over the module in memory
    m.bus.write_bytes(base + (h.s0 + 0xD4) as u32, &t.img[h.s0 + 0xD4..h.s0 + h.total]);
    m.bus.clock += start_time(d, &h);
    Some(Box::new(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// A one-pattern song (flavour B, speed 3 ticks per row, length 1) whose pattern
    /// has a note C-1 with instrument 0 on voice 0 in row 0 and `effect` on row 2.
    /// The instrument is a sample of 0x2FC bytes with a loop of 0x20 (the last 700 bytes
    /// are the padding the loop point is counted back over).
    fn module(effect: u8) -> Vec<u8> {
        let p0 = 0x132;
        let s0 = p0 + 0x200;
        let total = 0xD4 + 0x2FC;
        let mut d = vec![0u8; s0 + total];
        d[..8].copy_from_slice(b"AN COOL.");
        put32(&mut d, 8, 1);
        d[12] = 13;
        d[14] = 0;
        d[0x8E] = 1;
        d[p0] = 0x10;
        d[p0 + 1] = 0x00;
        d[p0 + 2 * 8] = 0;
        d[p0 + 2 * 8 + 1] = effect;
        put32(&mut d, s0, total as u32);
        // instrument 0: volume 0x7F, loop of 0x20 bytes
        d[s0 + 4] = 0x7F;
        d[s0 + 6] = 0;
        d[s0 + 7] = 0x20;
        put32(&mut d, s0 + 0x44, 0xD4);
        put32(&mut d, s0 + 0x48, 0x2FC);
        put32(&mut d, s0 + 0xCC, 0xFFFF_FFFF);
        put32(&mut d, s0 + 0xD0, 0);
        for b in &mut d[s0 + 0xD4..] {
            *b = 0x90;
        }
        d
    }

    #[test]
    fn recognises_modules() {
        let d = module(0);
        assert!(detect(&d));
        let mut e = d.clone();
        e[7] = b'?';
        assert!(!detect(&e));
        let mut e = d.clone();
        e[13] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[0x8E] = 0;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[12] = 16;
        assert!(!detect(&e));
        let mut e = d.clone();
        put32(&mut e, 0x132 + 0x200 + 0xCC, 0);
        assert!(!detect(&e));
        // shorter than the instrument block says
        assert!(!detect(&d[..d.len() - 1]));
        assert!(!detect(&[0u8; 0x400]));
    }

    fn run(data: Vec<u8>, ticks: usize) -> (Vec<(u32, u16)>, bool) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/tcb-test/TCB.test"), data).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "TCB Tracker");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let end = e.songend();
        (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), end)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn a_note_restarts_the_channel_and_sets_the_loop() {
        let (log, _) = run(module(0), 4);
        // period 2867490 / (0x8000 >> 3) + 1 = 701, volume (0x7F + 1) >> 1 = 64
        assert_eq!(reg(&log, 0xDFF0A6)[..3], [701, 1, 701]);
        assert_eq!(reg(&log, 0xDFF0A8)[0], 64);
        // sample start and length, then the loop (not the whole sample: 0x20 bytes)
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [0x17E, 0x10]);
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..2], [0x0000, 0x8001]);
        // the loop starts 0x20 bytes into the sample
        let lo = reg(&log, 0xDFF0A2);
        assert_eq!(lo[1].wrapping_sub(lo[0]), 0x20);
    }

    #[test]
    fn save_and_cut_stops_the_channel_and_restore_restarts_it() {
        // effect B (11) on row 2 of voice 0
        let (log, _) = run(module(11), 14);
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001), "{dma:?}");
    }

    #[test]
    fn song_end_is_announced_when_the_position_wraps() {
        let (_, end) = run(module(0), 64 * 3 - 4);
        assert!(!end);
        let (_, end) = run(module(0), 64 * 3 + 4);
        assert!(end);
    }

    #[test]
    fn pitch_table_has_sentinels() {
        assert_eq!(pitch(-1), 0x78D0);
        assert_eq!(pitch(0), 0x8000);
        assert_eq!(pitch(35), 0x3C686);
        assert_eq!(pitch(36), 0x40000);
    }
}
