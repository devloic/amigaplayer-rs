//! Desire (`dsr.*`), the music driver of the Power Drive build (the files `dsr.GMUSIC`
//! and `dsr.MUSIC`).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Desire player and the structure
//! of the corpus files), and this player was written from it alone and checked
//! against those writes.
//!
//! A `dsr.*` file is a raw memory image of the replay routine, its tables and the
//! music and samples. The code is not run. The module image is also the replayer's
//! working memory (its records overlap, as the original's do), so the port keeps a
//! mutable copy of the image and the engine works on it with the original's field
//! offsets. The table offsets (the global block, the period, vibrato, order,
//! pattern, sample tables) are the PC-relative operands of the play routine's
//! address loads, found with the 68000 decoder: decoded `lea d16(pc),An`
//! instructions in code order, not byte patterns.
//!
//! Decisions where the specification leaves the choice to the port:
//! * The number of subsongs is the number of distinct entries of the first channel's
//!   order list from entry 1 on, up to its first zero (6 and 10 in the two files).
//! * The host's song end is the stop path of each of the four channel records (0, 16,
//!   32, 48) having been taken; the idle fifth record's does not count. The host's
//!   end writes (DMACON 15, then the four volumes zero) are made once, after the
//!   tick that completes the set.
//! * The Batman Returns build (play routine at another offset, another layout) is
//!   not claimed.
//! * Sample addresses are guest addresses (the load address plus the file's
//!   offsets); the writes' moments within a tick are fitted to the reference.

use super::m68kdec::{decode, Ea, K};
use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Desire", detect, new };

/// The table offsets of the play routine's operands.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Params {
    g: usize,
    ord: usize,
    ptr: usize,
    flg: usize,
    len: usize,
    adr: usize,
    vib: usize,
    per: usize,
    recs: [usize; 4],
}

/// Where the play routine starts: from offset 240, the first `lea d16(pc),An`.
fn entry(d: &[u8]) -> Option<usize> {
    (240..d.len().min(1024)).step_by(2).find(|&o| {
        matches!(decode(d, o), Some(i) if i.k == K::Lea && matches!(i.src, Ea::Pc(_)) && matches!(i.dst, Ea::A(_)))
    })
}

/// The PC-relative address loads of the play routine, in code order.
fn operands(d: &[u8], start: usize) -> Vec<usize> {
    let mut v = Vec::new();
    let mut at = start;
    let mut end = d.len();
    while at < end {
        let Some(i) = decode(d, at) else { break };
        if i.k == K::Lea {
            if let (Ea::Pc(t), Ea::A(_)) = (i.src, i.dst) {
                if v.is_empty() {
                    // the first load is the global block; the code lies before it
                    end = end.min(t);
                }
                v.push(t);
            }
        }
        at = i.next();
    }
    v
}

fn params(d: &[u8]) -> Option<Params> {
    if d.len() <= 2500 {
        return None;
    }
    let o = operands(d, entry(d)?);
    if o.len() < 24 {
        return None;
    }
    let p = Params {
        g: o[0],
        recs: [o[1], o[2], o[3], o[4]],
        ord: o[10],
        ptr: o[11],
        flg: o[13],
        len: o[14],
        adr: o[15],
        vib: o[21],
        per: o[23],
    };
    if p.recs != [0, 16, 32, 48] || p.g + 4 > d.len() {
        return None;
    }
    // the tables must lie in the file and the period table must fall
    if [p.ord + 64, p.ptr + 64, p.flg + 2, p.len + 2, p.adr + 2, p.vib + 96, p.per + 192].iter().any(|&e| e > d.len()) {
        return None;
    }
    let per: Vec<u16> = (0..96).map(|k| be16(d, p.per + 2 * k)).collect();
    if per[0] == 0 || per.windows(2).any(|w| w[1] > w[0]) || per[0] > 3 * per[95] || per[0] > 0x7FFF {
        return None;
    }
    Some(p)
}

fn detect(d: &[u8]) -> bool {
    params(d).is_some()
}

struct Desire {
    p: Params,
    pristine: Vec<u8>,
    img: Vec<u8>,
    base: u32,
    sub: u16,
    subs: u16,
    started: bool,
    songend: bool,
    /// The stop paths taken, per channel record.
    stopped: [bool; 4],
    last: (Wk, usize),
}

/// The kinds of write, for the gaps between them.
#[derive(Clone, Copy, PartialEq)]
enum Wk {
    Adk,
    Dma,
    Lc,
    Len,
    Per,
    Vol,
    /// A volume write of a voice start or stop.
    Vst,
}

/// The fade counter's start (the fade request, 5.2).
const FADE: u16 = 61440;

impl Desire {
    fn rb(&self, o: usize) -> u8 {
        self.img.get(o).copied().unwrap_or(0)
    }
    fn rw(&self, o: usize) -> u16 {
        be16(&self.img, o)
    }
    fn wb(&mut self, o: usize, v: u8) {
        if let Some(b) = self.img.get_mut(o) {
            *b = v;
        }
    }
    fn ww(&mut self, o: usize, v: u16) {
        self.wb(o, (v >> 8) as u8);
        self.wb(o + 1, v as u8);
    }
    fn wl(&mut self, o: usize, v: u32) {
        self.ww(o, (v >> 16) as u16);
        self.ww(o + 2, v as u16);
    }
    fn rl(&self, o: usize) -> u32 {
        (self.rw(o) as u32) << 16 | self.rw(o + 2) as u32
    }

    fn start(&mut self) {
        self.img = self.pristine.clone();
        for c in 0..4 {
            self.wb(16 * c, 64 + self.sub as u8);
        }
        self.stopped = [false; 4];
        self.songend = false;
    }

    // ------------------------------------------------------------ writes

    fn dmacon(&mut self, hw: &mut Hw, ch: Option<usize>, v: u16) {
        if ch.is_some() {
            self.gap(hw, Wk::Dma, 0);
            hw.dmacon(v);
        }
    }

    /// Lets the time pass that the original's code takes between its previous write and
    /// this one (medians of the reference's gaps for the pair; the code's own timing
    /// depends on its data, which is not modelled).
    fn gap(&mut self, hw: &mut Hw, k: Wk, c: usize) {
        let (pk, pc) = self.last;
        let g = match (pk, pc, k, c) {
            (Wk::Adk, _, Wk::Vst, 0) => 204,
            (Wk::Vst, 0, Wk::Vst, 1) | (Wk::Vst, 1, Wk::Vst, 2) => 206,
            (Wk::Vst, 2, Wk::Vst, 3) => 316,
            (Wk::Adk, _, Wk::Vol, 0) => 376,
            (Wk::Vol, a, Wk::Per, b) if a == b => if b < 3 { 308 } else { 266 },
            (Wk::Per, 0, Wk::Vol, 1) => 432,
            (Wk::Per, 1, Wk::Vol, 2) => 460,
            (Wk::Per, 2, Wk::Vol, 3) => 522,
            (Wk::Adk, _, Wk::Dma, _) => 144,
            (Wk::Per, 0, Wk::Dma, _) => 86,
            (Wk::Per, 1, Wk::Dma, _) => 98,
            (Wk::Per, 2, Wk::Dma, _) => 148,
            (Wk::Dma, _, Wk::Vol, 0) => 372,
            (Wk::Dma, _, Wk::Vol, 1) => 390,
            (Wk::Dma, _, Wk::Vol, 2) => 406,
            (Wk::Dma, _, Wk::Vol, 3) => 418,
            (Wk::Dma, _, Wk::Dma, _) => 186,
            (Wk::Dma, _, Wk::Len, 3) => 266,
            (Wk::Dma, _, Wk::Len, _) => 228,
            (Wk::Adk, _, Wk::Len, 0) => 186,
            (Wk::Adk, _, Wk::Len, 3) => 614,
            (Wk::Adk, _, Wk::Len, _) => 316,
            (Wk::Len, 0, Wk::Lc, _) => 80,
            (Wk::Len, 1, Wk::Lc, _) => 92,
            (Wk::Len, 2, Wk::Lc, _) => 104,
            (Wk::Len, 3, Wk::Lc, _) => 108,
            (Wk::Lc, 0, Wk::Dma, _) => 132,
            (Wk::Lc, 1, Wk::Dma, _) => 144,
            (Wk::Lc, 2, Wk::Dma, _) => 156,
            (Wk::Lc, 3, Wk::Dma, _) => 160,
            (Wk::Per, 2, Wk::Lc, 3) => 134,
            (Wk::Lc, 3, Wk::Len, 3) => 14,
            (Wk::Len, 3, Wk::Vol, 3) => 408,
            (Wk::Vol, _, Wk::Vol, 3) => 316,
            (Wk::Vol, _, Wk::Vol, _) => 206,
            _ => 300,
        };
        hw.wait(g);
        self.last = (k, c);
    }

    fn w_vol(&mut self, hw: &mut Hw, c: usize, v: u16, k: Wk) {
        self.gap(hw, k, c);
        hw.vol(c, v);
    }

    // ------------------------------------------------------------ a channel

    fn update(&mut self, hw: &mut Hw, r: usize, ch: Option<usize>) {
        let g = self.p.g;
        // 5.1 the pending DMA
        let pend = self.rw(r + 250);
        if pend != 0 {
            self.dmacon(hw, ch, pend | 0x8200);
            self.ww(r + 250, 0);
            if self.rb(r + 175) == 0 {
                self.wb(r + 80, self.rb(r + 81));
                self.wb(r + 4, 0);
                self.wb(r + 90, 0);
                self.ww(r + 172, 0);
                self.ww(r + 162, 1);
            }
        }
        let st = self.rb(r);
        if st == 128 {
        } else if st & 0x80 != 0 {
            self.process(hw, r, ch);
        } else if st == 64 {
            self.ww(g + 2, FADE);
            self.wb(r, 192);
        } else if st != 0 {
            self.wb(r + 1, st);
            self.wb(r, st.wrapping_add(128));
            self.voice_start(hw, r, ch);
        } else {
            if let Some(c) = ch {
                self.w_vol(hw, c, 0, Wk::Vst);
            }
            self.wb(r, 128);
            if r % 16 == 0 && r < 64 {
                self.stopped[r / 16] = true;
            }
        }
    }

    fn voice_start(&mut self, hw: &mut Hw, r: usize, ch: Option<usize>) {
        let g = self.p.g;
        self.wb(r + 90, 0);
        self.ww(r + 92, 0);
        self.wb(r + 4, 0);
        self.ww(r + 6, 0);
        self.wb(r + 316, 0);
        self.ww(r + 250, 0);
        self.wb(r + 175, 0);
        self.ww(r + 172, 0);
        self.ww(r + 162, 0);
        self.ww(r + 160, 255);
        self.ww(g, 255);
        self.ww(g + 2, 0);
        self.wb(r + 10, 1);
        self.wb(r + 81, 1);
        self.wb(r + 5, 4);
        self.ww(r + 246, 0);
        self.ww(r + 240, 0);
        self.wb(r + 174, 0);
        if let Some(c) = ch {
            self.w_vol(hw, c, 0, Wk::Vst);
        }
    }

    fn process(&mut self, hw: &mut Hw, r: usize, ch: Option<usize>) {
        if self.sequence(hw, r, ch) {
            return;
        }
        self.volume_mode(r);
        self.volume_out(hw, r, ch);
        self.portamento(r);
        self.vibrato(r);
        self.period_out(hw, r, ch);
    }

    /// Stage A of the sequencer (also the pattern jump): the pattern for the order value `v`.
    fn new_order(&mut self, r: usize, v: u8) {
        let pat = if v & 64 != 0 {
            let x = ((v & 63) as u16).wrapping_add(self.rw(r + 254));
            self.rb(self.p.ord + x as usize) as u16
        } else {
            v as u16
        };
        self.ww(r + 252, pat.wrapping_mul(2));
        self.next_pattern(r);
    }

    /// The pointer becomes the pattern pointer of word 252, which moves on.
    fn next_pattern(&mut self, r: usize) {
        let e = self.rw(self.p.ptr + self.rw(r + 252) as usize);
        self.ww(r + 14, e);
        self.ww(r + 252, self.rw(r + 252).wrapping_add(2));
    }

    /// The sequencer; true when a note or a rest ended the tick's work.
    fn sequence(&mut self, hw: &mut Hw, r: usize, ch: Option<usize>) -> bool {
        let v = self.rb(r + 1);
        if v != 0 {
            self.wb(r + 1, 0);
            self.new_order(r, v);
        }
        let c = self.rb(r + 10).wrapping_sub(1);
        self.wb(r + 10, c);
        if c != 0 {
            return false;
        }
        // the event fetch; every event consumes at least a byte, so the loop is
        // bounded by the pointer wrapping round the image
        for _ in 0..0x20000 {
            let ptr = self.rw(r + 14) as usize;
            let e = self.rb(ptr);
            self.ww(r + 14, (ptr + 1) as u16);
            if e < 128 {
                if e != 0 {
                    let t = (e as u16 & 127).wrapping_add(self.rb(r + 174) as u16).wrapping_add(self.rb(r + 248) as u16) & 255;
                    self.ww(r + 244, t << 4);
                    let m = self.rw(r + 86);
                    self.dmacon(hw, ch, m);
                    self.ww(r + 250, m);
                }
                self.wb(r + 10, self.rb(r + 9));
                return true;
            }
            let p = ptr + 1;
            match e & 112 {
                16 => {
                    self.ww(r + 92, ((e & 15) as u16) << 3);
                    self.wb(r + 91, 8);
                }
                32 => self.wb(r + 175, e & 15),
                48 => self.ww(r + 6, ((e & 15) as u16) << 3),
                64 => self.ww(r + 6, self.rw(r + 6).wrapping_add((e & 15) as u16)),
                0 => {
                    let a = |k: usize| self.rb(p + k);
                    let (a0, a1, a2, a3, a4) = (a(0), a(1), a(2), a(3), a(4));
                    let to = |s: &mut Self, n: usize| s.ww(r + 14, (p + n) as u16);
                    match e {
                        128 => {
                            self.wb(r + 9, a0);
                            to(self, 1);
                        }
                        129 => {
                            self.wb(r + 11, a0);
                            self.ww(r + 12, (p + 1) as u16);
                            to(self, 1);
                        }
                        130 => {
                            let lp = self.rw(r + 12);
                            if self.rb(r + 11) == 0 {
                                self.ww(r + 14, lp);
                            } else {
                                let n = self.rb(r + 11) - 1;
                                self.wb(r + 11, n);
                                if n != 0 {
                                    self.ww(r + 14, lp);
                                }
                            }
                        }
                        131 => self.next_pattern(r),
                        132 => {
                            self.wb(r + 174, a0);
                            to(self, 1);
                        }
                        133 => {
                            self.wb(r + 165, a0);
                            self.wb(r + 94, a1);
                            self.wb(r + 167, a2);
                            self.wb(r + 169, a3);
                            self.wb(r + 171, a4);
                            to(self, 5);
                        }
                        134 => {
                            self.wb(r + 241, a0);
                            to(self, 1);
                        }
                        135 => {
                            self.wb(r + 93, a0);
                            self.wb(r + 91, a1);
                            to(self, 2);
                        }
                        136 => {
                            to(self, 1);
                            self.sample(hw, r, ch, a0 as usize);
                        }
                        137 => self.ww(r + 162, 0),
                        138 => {
                            self.ww(r + 246, a0 as i8 as i16 as u16);
                            to(self, 1);
                        }
                        139 => {
                            self.wb(r + 161, a0);
                            to(self, 1);
                        }
                        140 => self.new_order(r, a0),
                        141 => self.wb(r, 0),
                        142 => self.wb(r, 64),
                        143 => {
                            self.wb(r + 81, a0);
                            self.wb(r + 5, a1);
                            to(self, 2);
                        }
                        _ => {}
                    }
                }
                // classes 80, 96, 112: no effect, the fetch ends
                _ => return false,
            }
        }
        false
    }

    /// 136: the sample's length and address registers, kept in the record.
    fn sample(&mut self, hw: &mut Hw, r: usize, ch: Option<usize>, i: usize) {
        self.wb(r + 248, self.rb(self.p.flg + i));
        let d = 2 * i;
        let l = self.rw(self.p.len + d).wrapping_sub(128);
        let a = self.base.wrapping_add(8 * self.rw(self.p.adr + d) as u32).wrapping_add(128);
        if let Some(c) = ch {
            self.gap(hw, Wk::Len, c);
            hw.len(c, l);
            self.gap(hw, Wk::Lc, c);
            hw.loc(c, a);
        }
        self.ww(r + 324, l);
        self.wl(r + 320, a);
    }

    fn volume_mode(&mut self, r: usize) {
        let vol = self.rw(r + 172) as i16;
        match self.rw(r + 162) {
            0 => {
                let n = vol.wrapping_sub(self.rw(r + 170) as i16);
                self.ww(r + 172, n.max(0) as u16);
            }
            1 => {
                let n = vol.wrapping_add(self.rw(r + 164) as i16);
                if n > 255 {
                    self.ww(r + 162, 2);
                    self.ww(r + 172, 255);
                    self.wb(r + 95, 0);
                } else {
                    self.ww(r + 172, n as u16);
                }
            }
            2 => {
                if self.rb(r + 95) == self.rb(r + 94) {
                    self.ww(r + 162, 3);
                } else {
                    self.wb(r + 95, self.rb(r + 95).wrapping_add(1));
                }
            }
            3 => {
                let n = vol.wrapping_sub(self.rw(r + 166) as i16);
                let floor = self.rw(r + 168) as i16;
                if n < floor {
                    self.ww(r + 162, 4);
                    self.ww(r + 172, floor as u16);
                } else {
                    self.ww(r + 172, n as u16);
                }
            }
            4 => self.ww(r + 172, self.rw(r + 168)),
            _ => {}
        }
    }

    fn volume_out(&mut self, hw: &mut Hw, r: usize, ch: Option<usize>) {
        let g = self.p.g;
        let a = ((self.rw(r + 172) & 255) as u32 * self.rw(r + 160) as u32) & 0xFFFF;
        let v = (a * self.rw(g) as u32) >> 18 & 255;
        if let Some(c) = ch {
            self.w_vol(hw, c, v as u16, Wk::Vol);
        }
        let f = self.rw(g + 2);
        if f != 0 {
            self.wb(g + 1, self.rb(g + 2));
            let n = f.wrapping_sub(64);
            self.ww(g + 2, n);
            if n == 0 {
                for k in [0, 16, 32, 48, 64] {
                    self.ww(k, 0);
                }
            }
        }
    }

    fn portamento(&mut self, r: usize) {
        let s = self.rw(r + 240) as i16;
        let mut c = self.rw(r + 242) as i16;
        let t = self.rw(r + 244) as i16;
        if s == 0 {
            c = t;
        } else if c > t {
            c = c.wrapping_sub(s);
            if c < t {
                c = t;
            }
        } else {
            c = c.wrapping_add(s);
            if c > t {
                c = t;
            }
        }
        self.ww(r + 242, c as u16);
    }

    fn vibrato(&mut self, r: usize) {
        for _ in 0..512 {
            let x = (self.rb(r + 90) as u16).wrapping_add(self.rw(r + 92)) as usize;
            let v = self.rb(self.p.vib + x);
            if v == 127 {
                self.wb(r + 90, self.rb(r + 90).wrapping_sub(1));
                continue;
            }
            self.ww(r + 88, v as i8 as i16 as u16);
            let n = self.rb(r + 90).wrapping_add(1);
            self.wb(r + 90, if n == self.rb(r + 91) { 0 } else { n });
            return;
        }
    }

    fn period_out(&mut self, hw: &mut Hw, r: usize, ch: Option<usize>) {
        let d = (self.rw(r + 242) >> 1)
            .wrapping_add(self.rw(r + 2))
            .wrapping_add(self.rw(r + 88))
            .wrapping_add(self.rw(r + 246))
            .wrapping_add(16);
        let (q, m) = (d / 96, d % 96);
        let mut p = self.rw(self.p.per + 2 * m as usize);
        if q & 255 != 0 {
            let s = q & 63;
            p = if s >= 16 { 0 } else { p >> s };
        }
        if let Some(c) = ch {
            self.gap(hw, Wk::Per, c);
            hw.per(c, p >> 1);
        }
    }

    fn play(&mut self, hw: &mut Hw) {
        hw.bus.write_word(0xDFF09E, 0x00FF);
        self.last = (Wk::Adk, 0);
        for c in 0..3 {
            self.update(hw, 16 * c, Some(c));
        }
        self.ww(48 + 86, 8);
        // channel 3
        let c = self.rb(48 + 316);
        if c != 0 {
            self.wb(48 + 316, c - 1);
            self.ww(48 + 86, 0);
            if self.rb(64) == 128 {
                self.gap(hw, Wk::Lc, 3);
                hw.loc(3, self.rl(48 + 320));
                self.gap(hw, Wk::Len, 3);
                hw.len(3, self.rw(48 + 324));
            }
        }
        if self.rb(64) == 128 {
            self.update(hw, 48, Some(3));
        } else {
            self.update(hw, 48, None);
            self.update(hw, 64, Some(3));
            self.ww(48 + 86, 0);
            self.wb(48 + 316, 13);
        }
        if !self.songend && self.stopped.iter().all(|&s| s) {
            self.songend = true;
            hw.wait(300);
            hw.dmacon(0x000F);
            for c in 0..4 {
                hw.wait(60);
                hw.vol(c, 0);
            }
        }
    }
}

impl Replayer for Desire {
    fn format(&self) -> &'static str {
        "Desire"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.subs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (1..=self.subs).contains(&n) {
            self.sub = n;
            self.started = false;
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.start();
        }
        self.play(hw);
    }
}

/// The distinct entries of the first channel's order list, from entry 1 to its first zero.
fn subsong_count(d: &[u8], p: &Params) -> u16 {
    let row = p.ord + be16(d, 254) as usize;
    let mut n = 0;
    let mut last = 0;
    for k in 1..64 {
        let v = d.get(row + k).copied().unwrap_or(0);
        if v == 0 {
            break;
        }
        if v != last {
            n += 1;
            last = v;
        }
    }
    n.max(1)
}

fn new(d: &[u8], base: u32, _m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let p = params(d)?;
    Some(Box::new(Desire {
        p,
        pristine: d.to_vec(),
        img: d.to_vec(),
        base,
        sub: 1,
        subs: subsong_count(d, &p),
        started: false,
        songend: false,
        stopped: [false; 4],
        last: (Wk::Adk, 0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// Test-only encoder of the one instruction shape the scan matches.
    mod enc {
        /// `lea d16(pc),An` at image offset `at`, for the target offset `to`.
        pub fn lea_pc(an: u16, at: usize, to: usize) -> [u8; 4] {
            let op: u16 = 4 << 12 | an << 9 | 7 << 6 | 7 << 3 | 2;
            let d = (to as i64 - (at as i64 + 2)) as i16 as u16;
            [(op >> 8) as u8, op as u8, (d >> 8) as u8, d as u8]
        }
    }

    const G: usize = 900;
    const PER: usize = 910;
    const VIB: usize = 1110;
    const ORD: usize = 1210;
    const PTR: usize = 1300;
    const ADR: usize = 1500;
    const LEN: usize = 1550;
    const FLG: usize = 1600;
    const PAT1: usize = 2000;
    const PAT2: usize = 2100;

    fn put(d: &mut [u8], o: usize, v: u16) {
        d[o] = (v >> 8) as u8;
        d[o + 1] = v as u8;
    }

    /// A module whose play routine is a run of address loads, in the scan's operand order.
    fn module(pat1: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 3000];
        let ops = [G, 0, 16, 32, 48, 430, 64, 64, 64, 48, ORD, PTR, 0, FLG, LEN, ADR, 0, PTR, ORD, PTR, 0, VIB, VIB, PER];
        for (k, &t) in ops.iter().enumerate() {
            let at = 400 + 4 * k;
            d[at..at + 4].copy_from_slice(&enc::lea_pc((k % 7) as u16, at, t));
        }
        for k in 0..96 {
            put(&mut d, PER + 2 * k, 2000 - 10 * k as u16);
        }
        // order lists: channel 0 has two subsongs, the others the idle pattern
        for c in 0..4 {
            put(&mut d, 16 * c + 254, 16 * c as u16);
            d[ORD + 16 * c + 1] = if c == 0 { 1 } else { 2 };
        }
        d[ORD + 2] = 2;
        put(&mut d, PTR + 2, PAT1 as u16);
        put(&mut d, PTR + 4, PAT2 as u16);
        d[PAT1..PAT1 + pat1.len()].copy_from_slice(pat1);
        d[PAT2..PAT2 + 2].copy_from_slice(&[141, 0]);
        for c in 0..4 {
            put(&mut d, 16 * c + 86, 1 << c);
        }
        put(&mut d, ADR, 100);
        put(&mut d, LEN, 138);
        d
    }

    fn song() -> Vec<u8> {
        // default duration 2; envelope ramp 64; sample 0; a note; a rest; stop
        module(&[128, 2, 133, 64, 2, 1, 0, 1, 136, 0, 40, 0, 141, 0])
    }

    fn run(d: Vec<u8>, ticks: usize) -> (Engine, Vec<(u32, u16, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/dsr-test/dsr.TEST"), d).unwrap();
        let mut out = vec![0f32; 2 * 960];
        for _ in 0..ticks {
            e.render(&mut out);
        }
        let log = e.m.bus.write_log.take().unwrap();
        (e, log)
    }

    /// The register writes of each tick (the ADKCON write marks it).
    fn ticks(log: &[(u32, u16, u64)]) -> Vec<Vec<(u32, u16)>> {
        let mut t: Vec<Vec<(u32, u16)>> = Vec::new();
        for &(a, v, _) in log {
            if (a, v) == (0xDFF09E, 0x00FF) {
                t.push(Vec::new());
            }
            if let Some(last) = t.last_mut() {
                if a >> 12 == 0xDFF && a != 0xDFF09A && a != 0xDFF09C {
                    last.push((a, v));
                }
            }
        }
        t
    }

    #[test]
    fn the_scan_finds_the_tables_by_decoded_loads() {
        let d = song();
        let p = params(&d).unwrap();
        assert_eq!((p.g, p.per, p.vib, p.ord, p.ptr), (G, PER, VIB, ORD, PTR));
        assert_eq!((p.flg, p.len, p.adr, p.recs), (FLG, LEN, ADR, [0, 16, 32, 48]));
        assert!(detect(&d));
        assert_eq!(subsong_count(&d, &p), 2);
    }

    #[test]
    fn refuses_other_data() {
        assert!(!detect(&[0u8; 4000]));
        assert!(!detect(&song()[..2400]));
        let mut d = song();
        put(&mut d, PER + 20, 5000);
        assert!(!detect(&d), "a period table that rises");
        let mut d = song();
        let at = 400 + 4 * 23;
        d[at..at + 4].fill(0);
        assert!(!detect(&d), "an operand short");
        let mut d = song();
        let at = 400 + 4 * 2;
        d[at..at + 4].copy_from_slice(&enc::lea_pc(1, at, 24));
        assert!(!detect(&d), "records not at 0, 16, 32, 48");
    }

    #[test]
    fn plays_a_note_with_the_specified_writes() {
        let (e, log) = run(song(), 30);
        assert_eq!(e.playing.as_ref().unwrap().format, "Desire");
        let t = ticks(&log);
        // tick 0: every voice starts, channel 3 through the second driver
        let regs: Vec<u32> = t[0].iter().map(|w| w.0).collect();
        assert_eq!(regs, [0xDFF09E, 0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8]);
        // tick 1: the sample (length then address) and the note's DMA off
        let t1 = &t[1];
        let i = t1.iter().position(|w| w.0 == 0xDFF0A4).unwrap();
        assert_eq!(t1[i].1, 10);
        assert_eq!(t1[i + 1].0, 0xDFF0A0);
        assert_eq!(t1[i + 2].0, 0xDFF0A2);
        assert_eq!(t1[i + 3], (0xDFF096, 1));
        // tick 2: DMA on, volume 15 (ramp 64 of 255, master 255), period 95
        assert_eq!(&t[2][..4], &[(0xDFF09E, 0xFF), (0xDFF096, 0x8201), (0xDFF0A8, 15), (0xDFF0A6, 95)]);
        // the song ends when all four channels have stopped, with the host's writes
        assert!(e.songend());
        let all: Vec<_> = t.iter().flatten().copied().collect();
        let k = all.iter().position(|w| *w == (0xDFF096, 0x000F)).unwrap();
        assert_eq!(&all[k + 1..k + 5], &[(0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0)]);
    }

    #[test]
    fn subsongs_are_numbered_from_one() {
        let (mut e, _) = run(song(), 2);
        let p = e.playing.as_ref().unwrap();
        assert_eq!((p.sub_min, p.sub_max, p.sub), (1, 2, 1));
        e.change_subsong(1).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().sub, 2);
    }
}
