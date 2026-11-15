//! Jochen Hippel's "Astaroth" replayer (`astaroth.hip`): an early Hippel replayer
//! that reads COSO song data, as the Eagleplayer "Hippel" (2.06) runs it.
//!
//! Clean-room: written from `docs/specs/hippelastaroth/SPEC.md` only (a behaviour
//! specification), not from the replayer's code. Every register write is the
//! replayer's, in its order, at the colour clock the reference gives it: the
//! cost of each code path (SPEC section 9) is added to a cycle accumulator, and a
//! write lets `acc / 2` colour clocks pass.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Hippel Astaroth", detect, new };

/// FNV-1a of the code, `[0, EMPTY)`.
const CODE_HASH: u32 = 0x123E_2CDA;
const EMPTY: usize = 0x7EA;
const VOICES: usize = 0x7F2;
const PERIODS: usize = 0x94E;
const SONG_A: usize = 0x9F4;
const SONG_B: usize = 0x20D2;
const SAMPLE_BLOCK: usize = 0x33B8;
const SAMPLE_BYTES: usize = 0x3888;

/// Colour clocks between ticks (a CIA timer).
const TICK: u64 = 70940;

fn fnv(d: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for &b in d {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}

/// A unit test's module, which has a marker in place of the replayer's code.
#[cfg(test)]
fn synthetic(d: &[u8]) -> bool {
    d.get(2..10) == Some(b"SYNTHAST")
}

#[cfg(not(test))]
fn synthetic(_: &[u8]) -> bool {
    false
}

fn detect(d: &[u8]) -> bool {
    if synthetic(d) {
        return d.len() >= SAMPLE_BYTES && d.get(0..2) == Some(&[0x60, 0x1e]);
    }
    d.len() >= SAMPLE_BYTES
        && d.get(0..4) == Some(&[0x60, 0x1e, 0x60, 0x0e])
        && fnv(&d[..EMPTY]) == CODE_HASH
        && [SONG_A, SONG_B, SAMPLE_BLOCK].iter().all(|&s| {
            d.get(s..s + 4) == Some(b"COSO") && d.get(s + 0x20..s + 0x24) == Some(b"TFMX")
        })
}

/// The host's DMA-on call: DMACON, then a wait to the 10th line after the current one.
fn dma_on(hw: &mut Hw, mask: u16) {
    hw.dmacon(0x8000 | mask);
    let now = hw.bus.clock;
    hw.wait((now / LINE + 10) * LINE - now);
}

#[derive(Default, Clone, Copy)]
struct Voice {
    pos0: i64,
    posoff: i64,
    posend: i64,
    note: u8,
    info: u8,
    vseq: i64,
    vpos: i64,
    fseq: i64,
    fpos: i64,
    vtr: u8,
    vcnt: u8,
    vspd: u8,
    vsus: u8,
    fsus: u8,
    vibspd: u8,
    vibdep: u8,
    vibcur: u8,
    vibdel: u8,
    arg: u8,
    patp: i64,
    rowcnt: u8,
    rowspd: u8,
    fval: u8,
    ttr: u8,
    volume: u8,
    vibflag: u8,
    porta: u32,
    loop_addr: u32,
    loop_len: u16,
}

pub struct Astaroth {
    d: Vec<u8>,
    base: u32,
    sub: u16,
    counter: u8,
    speed: u16,
    stopped: bool,
    voices: [Voice; 4],
    /// The CPU cycles run since the last host call.
    acc: u64,
    /// Block of the song playing.
    song: usize,
    next_at: Option<u64>,
}

/// The colour clock InitSound is first called at, from the module's start (SPEC 9.3: a
/// constant of the file, measured for two: 428474 for the real one of 28924 bytes and
/// 268916 for the synthetic one of 14496; other sizes by the line through them).
fn load_time(len: usize) -> u64 {
    let (l0, c0, l1, c1) = (14496i64, 268916i64, 28924i64, 428474i64);
    (c0 + (len as i64 - l0) * (c1 - c0) / (l1 - l0)) as u64
}

/// The clock the machine is at when a replayer is created, in the reference's terms:
/// subtracted from `load_time` (measured).
const CLOCK_AT_NEW: u64 = 100;

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let mut p = Astaroth {
        d: d.to_vec(),
        base,
        sub: 1,
        counter: 0,
        speed: 0,
        stopped: false,
        voices: Default::default(),
        acc: 0,
        song: SONG_B,
        next_at: None,
    };
    // the voices' fields InitSound leaves alone, as saved in the file
    for (v, vo) in p.voices.iter_mut().enumerate() {
        let r = VOICES + 0x48 * v;
        vo.vibflag = p.d[r + 0x2E];
        vo.loop_len = be16(&p.d, r + 0x42);
        vo.loop_addr = be32(&p.d, r + 0x44);
    }
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(load_time(d.len()).saturating_sub(CLOCK_AT_NEW));
    p.init_sound(1, &mut hw);
    Some(Box::new(p))
}

impl Replayer for Astaroth {
    fn format(&self) -> &'static str {
        "Hippel Astaroth"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        false
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, 4, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.end_sound(hw);
        self.init_sound(n, hw);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.stopped {
            return;
        }
        self.acc = 308;
        self.counter = self.counter.wrapping_sub(1);
        if self.counter == 0 {
            self.counter = self.speed as u8;
            self.acc += 16;
            for v in 0..4 {
                self.acc += 26;
                self.row(v, hw);
            }
        }
        self.acc += 20;
        let mut mask = 0u16;
        for v in 0..4 {
            self.acc += 26;
            let per = self.effects(v, hw, &mut mask);
            self.acc += 20 + 230;
            self.sync(hw);
            hw.per(v, per);
            self.acc = 166;
            self.sync(hw);
            hw.vol(v, self.voices[v].volume as u16);
            self.acc = 20 + 4;
        }
        // after voice 3: to the DMACON write
        self.acc += 160 + 248;
        self.sync(hw);
        dma_on(hw, mask);
        self.acc = 186;
        for v in 0..4 {
            self.sync(hw);
            let vo = self.voices[v];
            hw.loc(v, vo.loop_addr);
            self.acc = if v == 0 { 210 } else { 198 };
            self.sync(hw);
            hw.len(v, vo.loop_len);
            self.acc = 182;
        }
        self.acc = 0;
    }
}

impl Astaroth {
    fn b(&self, o: i64) -> u8 {
        if o < 0 {
            return 0;
        }
        self.d.get(o as usize).copied().unwrap_or(0)
    }

    fn w(&self, o: i64) -> u16 {
        (self.b(o) as u16) << 8 | self.b(o + 1) as u16
    }

    fn l(&self, o: i64) -> u32 {
        (self.w(o) as u32) << 16 | self.w(o + 2) as u32
    }

    /// Entry `n` of an offset table of the current song block (`field`: its offset in the
    /// block's header): the block's start plus the word, signed.
    fn table(&self, field: i64, n: u8) -> i64 {
        let s = self.song as i64;
        let t = s + self.l(s + field) as i64;
        s + self.w(t + 2 * n as i64) as i16 as i64
    }

    fn sync(&mut self, hw: &mut Hw) {
        hw.wait(self.acc / 2);
        self.acc = 0;
    }

    fn end_sound(&mut self, hw: &mut Hw) {
        hw.wait(275);
        hw.dmacon(0x000f);
        hw.wait(97);
        for v in 0..4 {
            if v > 0 {
                hw.wait(89);
            }
            hw.per(v, 0);
            hw.wait(81);
            hw.vol(v, 0);
        }
        hw.wait(192);
        self.stopped = true;
    }

    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        use m68k::AddressBus;
        self.sub = n;
        let (first, last, speed);
        if n == 0 {
            return;
        } else if n == 1 {
            self.song = SONG_B;
            (first, last, speed) = (0i64, 204i64, 3u16);
        } else {
            self.song = SONG_A;
            let s = self.song as i64;
            let e = s + self.l(s + 0x14) as i64 + 6 * (n as i64 - 2);
            (first, last, speed) = (self.w(e) as i64, self.w(e + 2) as i64, self.w(e + 4));
        }
        // the filter off (a host call)
        let v = hw.bus.read_byte(0xbfe001);
        hw.bus.write_byte(0xbfe001, v | 2);
        hw.wait(if n == 1 { 535 } else { 579 });
        hw.dmacon(0x000f);
        let s = self.song as i64;
        let plist = s + self.l(s + 0x10) as i64;
        for v in 0..4usize {
            let col = 3 * v as i64;
            let vo = &mut self.voices[v];
            let (loop_addr, loop_len, vibflag) = (vo.loop_addr, vo.loop_len, vo.vibflag);
            *vo = Voice {
                vseq: EMPTY as i64,
                fseq: EMPTY as i64,
                vcnt: 1,
                vspd: 1,
                loop_addr,
                loop_len,
                vibflag,
                ..Default::default()
            };
            vo.pos0 = plist + 12 * first + col;
            vo.posend = plist + 12 * (last + 1) + col;
            vo.posoff = 12;
            let p0 = vo.pos0;
            let (b0, b1, b2) = (self.b(p0), self.b(p0 + 1), self.b(p0 + 2));
            let patp = self.table(0x0C, b0);
            let vo = &mut self.voices[v];
            vo.ttr = b1;
            vo.vtr = b2;
            vo.patp = patp;
            // the writes: AUDxLC, AUDxLEN, AUDxPER, AUDxVOL
            hw.wait(if v == 0 { 434 } else { 562 });
            hw.loc(v, self.base + EMPTY as u32);
            hw.wait(99);
            hw.len(v, 0x10);
            hw.wait(91);
            hw.per(v, 0);
            hw.wait(81);
            hw.vol(v, 0);
        }
        hw.wait(406);
        self.counter = 1;
        self.speed = speed;
        self.stopped = false;
        self.acc = 0;
        self.next_at = Some(hw.bus.clock + TICK);
    }

    /// The pattern rows of voice `v`.
    fn row(&mut self, v: usize, hw: &mut Hw) {
        self.acc += 25;
        let vo = &mut self.voices[v];
        vo.rowcnt = vo.rowcnt.wrapping_sub(1);
        if (vo.rowcnt as i8) >= 0 {
            self.acc += 16;
            return;
        }
        vo.rowcnt = vo.rowspd;
        self.acc += 36 + 25;
        loop {
            let patp = self.voices[v].patp;
            let c = self.b(patp);
            match c {
                0xFF => {
                    self.acc += 61;
                    let vo = &mut self.voices[v];
                    let mut p = vo.pos0 + vo.posoff;
                    if p == vo.posend {
                        vo.posoff = 0;
                        p = vo.pos0;
                        self.acc += 28;
                    }
                    let (b0, b1, b2) = (self.b(p), self.b(p + 1), self.b(p + 2));
                    let patp = self.table(0x0C, b0);
                    let vo = &mut self.voices[v];
                    vo.ttr = b1;
                    vo.vtr = b2;
                    vo.patp = patp;
                    vo.posoff += 12;
                    self.acc += 180 + 25;
                }
                0xFE => {
                    let n = self.b(patp + 1);
                    let vo = &mut self.voices[v];
                    vo.rowspd = n;
                    vo.rowcnt = n;
                    vo.patp += 2;
                    self.acc += 59 + 25;
                }
                0xFD => {
                    let n = self.b(patp + 1);
                    let vo = &mut self.voices[v];
                    vo.rowspd = n;
                    vo.rowcnt = n;
                    vo.patp += 2;
                    self.acc += 98;
                    return;
                }
                _ => {
                    let info = self.b(patp + 1);
                    let third = info & 0xE0 != 0;
                    let arg = self.b(patp + 2);
                    let vo = &mut self.voices[v];
                    vo.note = c;
                    vo.info = info;
                    if third {
                        vo.arg = arg;
                    }
                    vo.patp += if third { 3 } else { 2 };
                    vo.porta = 0;
                    self.acc += 83 + if third { 16 } else { 0 } + 47;
                    if c & 0x80 != 0 {
                        self.acc += 16;
                        return;
                    }
                    self.acc += 36 + 220;
                    self.sync(hw);
                    hw.dmacon(1 << v);
                    self.acc = 12 + 263 + 126 + if info & 0x40 != 0 { 12 } else { 0 };
                    let vo = self.voices[v];
                    let i = (info & 0x1F).wrapping_add(vo.vtr);
                    let vs = self.table(0x08, i);
                    let fnum = if info & 0x40 != 0 { vo.arg } else { self.b(vs + 1) };
                    let fseq = self.table(0x04, fnum);
                    let (v0, v2, v3, v4) = (self.b(vs), self.b(vs + 2), self.b(vs + 3), self.b(vs + 4));
                    let vo = &mut self.voices[v];
                    vo.vpos = 0;
                    vo.vcnt = v0;
                    vo.vspd = v0;
                    vo.vibspd = v2;
                    vo.vibflag = 0x40;
                    vo.vibdep = v3;
                    vo.vibcur = v3;
                    vo.vibdel = v4;
                    vo.vseq = vs + 5;
                    vo.fseq = fseq;
                    vo.fpos = 0;
                    vo.fsus = 0;
                    vo.vsus = 0;
                    return;
                }
            }
        }
    }

    /// A sample's address and loop of entry `s` of the sample table; writes AUDxLC and
    /// AUDxLEN through the accumulator's costs.
    fn start_sample(&mut self, v: usize, s: u8, hw: &mut Hw) {
        let blk = SAMPLE_BLOCK as i64;
        let tab = blk + self.l(blk + 0x18) as i64 + 10 * s as i64;
        let start = self.l(tab);
        let len = self.w(tab + 4);
        let lstart = self.w(tab + 6);
        let llen = self.w(tab + 8);
        let addr = self.base.wrapping_add(SAMPLE_BYTES as u32).wrapping_add(start);
        self.acc += 164 + 234;
        self.sync(hw);
        hw.loc(v, addr);
        self.acc = 202;
        self.sync(hw);
        hw.len(v, len);
        let vo = &mut self.voices[v];
        vo.loop_addr = addr.wrapping_add(lstart as u32);
        vo.loop_len = llen;
    }

    /// The voice's frequency sequence, envelope, period and portamento; returns the period.
    fn effects(&mut self, v: usize, hw: &mut Hw, mask: &mut u16) -> u16 {
        self.acc += 4;
        self.freq(v, hw, mask);
        self.envelope(v);
        self.period(v)
    }

    fn freq(&mut self, v: usize, hw: &mut Hw, mask: &mut u16) {
        'sustain: loop {
            self.acc += 21;
            if self.voices[v].fsus != 0 {
                self.voices[v].fsus -= 1;
                self.acc += 26;
                return;
            }
            self.acc += 34;
            let vo = self.voices[v];
            let mut c = self.b(vo.fseq + vo.fpos);
            loop {
                {
                    // the E1 and E0 tests
                    self.acc += 23;
                    if c == 0xE1 {
                        return;
                    }
                    self.acc += 21;
                    if c == 0xE0 {
                        self.acc += 68;
                        let vo = self.voices[v];
                        let x = self.b(vo.fseq + vo.fpos + 1);
                        let fpos = (x & 0x3F) as i64;
                        self.voices[v].fpos = fpos;
                        c = self.b(vo.fseq + fpos);
                    }
                }
                let vo = self.voices[v];
                let at = vo.fseq + vo.fpos;
                self.acc += 21; // E2 test (after E0: its first test)
                if c == 0xE2 {
                    self.acc += 48 + 208;
                    self.sync(hw);
                    hw.dmacon(1 << v);
                    *mask |= 1 << v;
                    let s = self.b(at + 1);
                    self.start_sample(v, s, hw);
                    self.acc += 32 + 138;
                    let vo = &mut self.voices[v];
                    vo.vpos = 0;
                    vo.vcnt = 1;
                    vo.fpos += 2;
                    break;
                }
                self.acc += 21; // E4 test
                if c == 0xE4 {
                    let s = self.b(at + 1);
                    self.start_sample(v, s, hw);
                    self.acc += 32 + 98;
                    self.voices[v].fpos += 2;
                    break;
                }
                self.acc += 21; // E7 test
                if c == 0xE7 {
                    self.acc += 112;
                    let t = self.b(at + 1);
                    let fseq = self.table(0x04, t);
                    let vo = &mut self.voices[v];
                    vo.fseq = fseq;
                    vo.fpos = 0;
                    c = self.b(fseq);
                    continue;
                }
                self.acc += 21; // E8 test
                if c == 0xE8 {
                    self.acc += 54;
                    let n = self.b(at + 1);
                    let vo = &mut self.voices[v];
                    vo.fsus = n;
                    vo.fpos += 2;
                    continue 'sustain;
                }
                self.acc += 21; // E3 test
                if c == 0xE3 {
                    self.acc += 68;
                    let (a, bb) = (self.b(at + 1), self.b(at + 2));
                    let vo = &mut self.voices[v];
                    vo.vibspd = a;
                    vo.vibdep = bb;
                    vo.fpos += 3;
                }
                break;
            }
            // the value step
            self.acc += 74;
            let vo = &mut self.voices[v];
            let val = self.d.get((vo.fseq + vo.fpos).max(0) as usize).copied().unwrap_or(0);
            vo.fval = val;
            vo.fpos += 1;
            return;
        }
    }

    fn envelope(&mut self, v: usize) {
        'sustain: loop {
            self.acc += 21;
            let vo = &mut self.voices[v];
            if vo.vsus != 0 {
                vo.vsus -= 1;
                self.acc += 26;
                return;
            }
            loop {
                self.acc += 25;
                let vo = &mut self.voices[v];
                vo.vcnt = vo.vcnt.wrapping_sub(1);
                if vo.vcnt != 0 {
                    return;
                }
                vo.vcnt = vo.vspd;
                self.acc += 20;
                loop {
                    self.acc += 61;
                    let vo = self.voices[v];
                    let at = vo.vseq + vo.vpos;
                    let c = self.b(at);
                    if c == 0xE8 {
                        self.acc += 54;
                        let n = self.b(at + 1);
                        let vo = &mut self.voices[v];
                        vo.vpos += 2;
                        vo.vsus = n;
                        continue 'sustain;
                    }
                    self.acc += 17;
                    if c == 0xE1 {
                        return;
                    }
                    self.acc += 17;
                    if c == 0xE0 {
                        self.acc += 62;
                        let x = self.b(at + 1);
                        self.voices[v].vpos = (x & 0x3F) as i64 - 5;
                        continue;
                    }
                    self.acc += 40;
                    let vo = &mut self.voices[v];
                    vo.volume = c;
                    vo.vpos += 1;
                    return;
                }
            }
        }
    }

    fn period(&mut self, v: usize) -> u16 {
        let vo = &mut self.voices[v];
        self.acc += 21 + 73;
        let n = if vo.fval & 0x80 != 0 {
            vo.fval & 0x7F
        } else {
            self.acc += 24;
            vo.fval.wrapping_add(vo.note).wrapping_add(vo.ttr) & 0x7F
        };
        let mut p = be16(&self.d, PERIODS + 2 * n as usize);
        if vo.vibdel != 0 {
            vo.vibdel -= 1;
            self.acc += 26;
        } else {
            self.acc += 45;
            let d4 = vo.vibdep.wrapping_mul(2) as i32;
            let mut d1 = vo.vibcur as i32;
            let held = vo.vibflag & 0x80 != 0 && vo.vibflag & 1 != 0;
            if vo.vibflag & 0x80 != 0 {
                self.acc += 19;
            }
            if !held {
                self.acc += 19;
                if vo.vibflag & 0x20 == 0 {
                    self.acc += 21;
                    d1 -= vo.vibspd as i32;
                    if d1 < 0 {
                        vo.vibflag |= 0x20;
                        d1 = 0;
                        self.acc += 26;
                    }
                } else {
                    self.acc += 25;
                    d1 = (d1 + vo.vibspd as i32) & 0xFF;
                    if d1 >= d4 {
                        vo.vibflag &= !0x20;
                        d1 = d4;
                        self.acc += 18;
                    }
                }
                vo.vibcur = d1 as u8;
                self.acc += 12;
            }
            let mut o = d1 - (d4 >> 1);
            self.acc += 21;
            if o < 0 {
                self.acc += 8;
            }
            let mut k = 2 * n as i32 + 160;
            self.acc += 17;
            while k < 256 {
                o *= 2;
                k += 24;
                self.acc += 21;
            }
            self.acc += 4;
            p = p.wrapping_add(o as u16);
        }
        vo.vibflag ^= 1;
        self.acc += 47;
        if vo.info & 0x20 != 0 {
            self.acc += 35;
            let b = vo.arg;
            if b < 0x80 {
                vo.porta = vo.porta.wrapping_add(b as u32 * 2048);
                p = p.wrapping_sub((vo.porta >> 16) as u16);
                self.acc += 78;
            } else {
                vo.porta = vo.porta.wrapping_add((256 - b as u32) * 2048);
                p = p.wrapping_add((vo.porta >> 16) as u16);
                self.acc += 72;
            }
        }
        self.acc += 32;
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    const PERIOD_TABLE: [u16; 83] = [
        1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570,
        538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160,
        151, 143, 135, 127, 120, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 3424, 3232, 3048, 2880,
        2712, 2560, 2416, 2280, 2152, 2032, 1920, 1812, 113, 113, 113, 113, 113, 113, 113, 113, 10344, 113, 113,
    ];

    /// The synthetic module of SPEC 11.1: a marker in place of the code, song B with two
    /// frequency sequences, two volume sequences and two patterns, two samples.
    fn module() -> Vec<u8> {
        let mut d = vec![0u8; 0x38A0];
        d[0] = 0x60;
        d[1] = 0x1E;
        d[2..10].copy_from_slice(b"SYNTHAST");
        d[EMPTY..EMPTY + 8].copy_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0xE1]);
        for (i, p) in PERIOD_TABLE.iter().enumerate() {
            d[PERIODS + 2 * i..PERIODS + 2 * i + 2].copy_from_slice(&p.to_be_bytes());
        }
        let mut put = |at: usize, bytes: &[u8]| d[at..at + bytes.len()].copy_from_slice(bytes);
        let s = SONG_B;
        put(s, b"COSO");
        for (o, v) in [(4, 0x40u32), (8, 0x44), (0xC, 0x48), (0x10, 0x7A), (0x14, 0xA16)] {
            put(s + o, &v.to_be_bytes());
        }
        put(s + 0x20, b"TFMX");
        put(s + 0x40, &[0, 0x4C, 0, 0x53, 0, 0x57, 0, 0x63, 0, 0x6B, 0, 0x75]);
        put(s + 0x4C, &[0xE2, 0, 0, 0x0C, 0, 0xE0, 3]);
        put(s + 0x53, &[0xE2, 1, 0, 0xE1]);
        put(s + 0x57, &[1, 0, 0, 0, 0, 0x3F, 0x30, 0x20, 0xE8, 4, 0x10, 0xE1]);
        put(s + 0x63, &[2, 1, 2, 3, 1, 0x28, 0xE0, 5]);
        put(s + 0x6B, &[0x18, 0, 0xFE, 1, 0x1B, 0, 0x1F, 0x20, 3, 0xFF]);
        put(s + 0x75, &[0xFD, 5, 0x24, 1, 0xFF]);
        for i in 0..205 {
            put(s + 0x7A + 12 * i, &[0, 0, 0, 1, 0, 0, 1, 5, 0, 1, 0xF4, 0]);
        }
        put(s + 0x7A + 12 * 205, &[0, 0, 0, 0xCC, 0, 3]);
        let b = SAMPLE_BLOCK;
        put(b, b"COSO");
        put(b + 4, &0x40u32.to_be_bytes());
        put(b + 8, &0x40u32.to_be_bytes());
        put(b + 0x18, &0x386u32.to_be_bytes());
        put(b + 0x20, b"TFMX");
        put(0x373E, &[0, 0, 0, 0, 0, 8, 0, 0, 0, 8]);
        put(0x373E + 10, &[0, 0, 0, 0x10, 0, 4, 0, 4, 0, 2]);
        put(SAMPLE_BYTES, &[0x40; 8]);
        put(SAMPLE_BYTES + 8, &[0xC0; 8]);
        put(SAMPLE_BYTES + 16, &[0, 0x30, 0x60, 0x30, 0, 0xD0, 0xA0, 0xD0]);
        d
    }

    fn play(seconds: usize) -> (Vec<(u32, u16, u64)>, u32) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("astaroth.hip"), module()).unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Hippel Astaroth");
        assert_eq!((p.sub_min, p.sub_max), (1, 4));
        let base = e.m.deli.list.first().unwrap().0;
        let mut out = vec![0f32; 2 * 48000 * seconds];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        if let Some(p) = std::env::var_os("ASTAROTH_DUMP") {
            let t: String = log.iter().map(|w| format!("{:x} {:04x} {}\n", w.0, w.1, w.2)).collect();
            std::fs::write(p, t).unwrap();
        }
        (log, base)
    }

    /// The first tick as the reference writes it (SPEC 11.1), times relative to the
    /// first interrupt.
    #[test]
    fn synthetic_first_tick() {
        let (log, base) = play(2);
        let w: Vec<_> = log.iter().filter(|w| (0xDFF096..0xDFF0E0).contains(&w.0)).collect();
        // InitSound: DMACON, then per voice LC (high, low), LEN, PER, VOL
        assert_eq!((w[0].0, w[0].1), (0xDFF096, 0x000F));
        let init = w[0].2;
        assert_eq!((w[1].0, w[1].1, w[2].1), (0xDFF0A0, 0, (base + EMPTY as u32) as u16));
        assert_eq!((w[3].0, w[3].1), (0xDFF0A4, 0x10));
        // InitSound returns 4145 colour clocks after its call: the interrupt 70940 later
        let i = init - 535 + 4145 + 70940;
        let t: Vec<_> = w.iter().filter(|x| x.2 >= i).collect();
        let rel = |x: &(u32, u16, u64)| (x.0, x.1, x.2 - i);
        let sample = (base + SAMPLE_BYTES as u32) as u16;
        assert_eq!(rel(t[0]), (0xDFF096, 0x0001, 411));
        assert_eq!(rel(t[1]), (0xDFF096, 0x0001, 1139));
        assert_eq!(rel(t[2]), (0xDFF0A0, 0, 1338));
        assert_eq!(rel(t[3]), (0xDFF0A2, sample, 1338));
        assert_eq!(rel(t[4]), (0xDFF0A4, 8, 1439));
        assert_eq!(rel(t[5]), (0xDFF0A6, 0x01AC, 1975));
        assert_eq!(rel(t[6]), (0xDFF0A8, 0x003F, 2058));
        // voices 1..3 on the empty table: volume 1; voice 3's note is past the table
        let per_vol: Vec<_> = t.iter().skip(7).take(6).map(|x| (x.0, x.1)).collect();
        assert_eq!(
            per_vol,
            [(0xDFF0B6, 0x0650), (0xDFF0B8, 1), (0xDFF0C6, 0x04B8), (0xDFF0C8, 1), (0xDFF0D6, 0), (0xDFF0D8, 1)]
        );
        // then DMACON 8001 and the loops: voice 0 the sample's, the others 0
        assert_eq!((t[13].0, t[13].1), (0xDFF096, 0x8001));
        assert_eq!((t[14].0, t[14].1, t[15].1, t[16].1), (0xDFF0A0, 0, sample, 8));
    }

    #[test]
    fn synthetic_following_ticks() {
        let (log, _) = play(2);
        let pers = |reg: u32| log.iter().filter(|x| x.0 == reg).map(|x| x.1).collect::<Vec<_>>();
        // voice 0 per tick (after InitSound's 0): 01AC/3F, 00D6/30, 01AC/20 (SPEC 11.1)
        let (per, vol) = (pers(0xDFF0A6), pers(0xDFF0A8));
        assert_eq!(&per[1..4], &[0x01AC, 0x00D6, 0x01AC]);
        assert_eq!(&vol[1..4], &[0x3F, 0x30, 0x20]);
        // tick 4: the next row (note $1B) restarts the voice
        assert_eq!(per[4], 0x0168);
        assert_eq!(vol[4], 0x3F);
    }

    #[test]
    fn detects_only_its_build() {
        assert!(detect(&module()));
        let mut d = module();
        d[0] = 0;
        assert!(!detect(&d));
        assert!(!detect(&[0u8; 100]));
    }
}
