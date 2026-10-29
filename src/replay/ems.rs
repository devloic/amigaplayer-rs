//! EMS, Editeur Musical Sequentiel (Thomas Pimmel; `ems.*`; replayer 1.24).
//!
//! Clean-room, written from a behavioural specification and black-box runs of UADE's
//! player (its Paula write logs); no original code. A file holds the header, the song
//! sequence, 15 sample entries, the patterns and the sample data. Nothing is mixed: the
//! player sets period, volume, start and length of the four Paula voices and DMA reads the
//! samples from the module.
//!
//! One call per tick at 50 Hz. A call is a row tick (the tick counter has reached the
//! speed, so a row lasts speed + 1 ticks) or an in-between tick (loop restart on the first,
//! then the volume slide, period slide and vibrato effects).
//!
//! Decisions where the specification leaves the choice to the port:
//! * Command 9 with argument 0 is ignored, and a break to row 64 or more becomes a break
//!   to row 0 (the original crashes, or is undefined, there).
//! * The host's start-up writes (the four volume clears before the first tick) are not made.
//! * The time of each write within a tick is fitted to the specification's measured
//!   offsets from the frame start; only the order of the writes is exact.
//! * The sample start-address rule is applied to the module image in guest memory at init
//!   (one word cleared per sample), so the cleared word also changes what Paula reads.
//! * Song end is a sticky flag, set in the row tick in which the position wraps; the
//!   player carries on from position 0.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "EMS", detect, new };

const PERIODS: [u16; 48] = [
    1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508,
    480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127,
    120, 113,
];

const HEAD: usize = 314;
const PAT: usize = 1024;
/// Colour clocks from the frame start to the first write of a row tick / of an in-between tick.
const T_ROW: u64 = 310;
const T_BETWEEN: u64 = 190;
/// The beam wait of a row tick (about nine lines), and a write's own cost.
const T_BEAM: u64 = 2500;
const T_WRITE: u64 = 8;

fn detect(d: &[u8]) -> bool {
    if d.len() <= 1338 || d[0] != 1 || d[1] > 37 {
        return false;
    }
    let l = be16(d, 2);
    let h = be16(d, 4);
    if !(1..=128).contains(&l) || h > 63 {
        return false;
    }
    if d[6..134].iter().any(|&s| u16::from(s) > h) {
        return false;
    }
    let mut any = false;
    for c in 0..256 {
        let p = be16(d, HEAD + 4 * c);
        if p & 0x8000 != 0 || (p != 0 && !PERIODS.contains(&p)) {
            return false;
        }
        any |= p != 0;
    }
    any && d.len() >= need(d)
}

/// The size the init wants: the header, the patterns and the sample data.
fn need(d: &[u8]) -> usize {
    let h = be16(d, 4) as usize;
    let words: usize = (0..15).map(|i| be16(d, 134 + 12 * i + 4) as usize).sum();
    HEAD + (h + 1) * PAT + 2 * words
}

#[derive(Clone, Copy, Default)]
struct Voice {
    addr: u32,
    len: u16,
    rs: u16,
    rl: u16,
    vol: u16,
    per: u16,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum W {
    Dma(u16),
    Loc(usize, u32),
    Len(usize, u16),
    Per(usize, u16),
    Vol(usize, u16),
    Led(bool),
    Wait(u64),
}

struct Ems {
    d: Vec<u8>,
    len: u16,
    seq: [u8; 128],
    /// Per sample entry 1..15: play address (guest), length, repeat start, repeat length, volume.
    smp: [(u32, u16, u16, u16, u16); 16],
    speed: u16,
    tick: u16,
    pos: u16,
    row: u16,
    brk: u16,
    pat: u8,
    v: [Voice; 4],
    eff: [(u8, u8); 4],
    songend: bool,
}

impl Ems {
    /// The sample play addresses (the specification's 3.3). Also returns the offsets whose word is cleared.
    fn new(d: &[u8], base: u32) -> (Ems, Vec<u32>) {
        let h = be16(d, 4) as usize;
        let mut seq = [0u8; 128];
        seq.copy_from_slice(&d[6..134]);
        let mut smp = [(0, 0, 0, 0, 0); 16];
        let mut run = 1336 + h * PAT;
        let mut clear = Vec::new();
        for (i, s) in smp.iter_mut().enumerate().skip(1) {
            let e = 134 + 12 * (i - 1);
            let n = be16(d, e + 4);
            *s = (0, n, be16(d, e + 6), be16(d, e + 8), be16(d, e + 10));
            if n != 0 {
                s.0 = base + run as u32;
                clear.push(run as u32);
                run += 2 * n as usize;
            }
        }
        let e = Ems {
            d: d.to_vec(),
            len: be16(d, 2),
            seq,
            smp,
            speed: 6,
            tick: 0,
            pos: 0,
            row: 0,
            brk: 0,
            pat: seq[0],
            v: [Voice::default(); 4],
            eff: [(0, 0); 4],
            songend: false,
        };
        (e, clear)
    }

    fn cell(&self, ch: usize) -> (u16, u8, u8) {
        let o = HEAD + self.pat as usize * PAT + self.row as usize * 16 + ch * 4;
        match self.d.get(o..o + 4) {
            Some(b) => (u16::from_be_bytes([b[0], b[1]]), b[2], b[3]),
            None => (0, 0, 0),
        }
    }

    fn tick(&mut self, w: &mut Vec<W>) {
        if self.tick >= self.speed {
            self.row_tick(w);
        } else {
            self.between(w);
        }
    }

    fn row_tick(&mut self, w: &mut Vec<W>) {
        w.push(W::Wait(T_ROW));
        self.tick = 0;
        self.eff = [(0, 0); 4];
        let cells: Vec<_> = (0..4).map(|c| self.cell(c)).collect();
        let mask = cells.iter().enumerate().fold(0u16, |m, (c, x)| m | u16::from(x.1 >> 4 != 0) << c);
        w.push(W::Dma(mask));
        for (c, &(per, ic, a)) in cells.iter().enumerate() {
            w.push(W::Wait(T_WRITE));
            self.v[c].addr = 0;
            let ins = (ic >> 4) as usize;
            if ins != 0 {
                let (addr, n, rs, rl, vol) = self.smp[ins];
                let v = &mut self.v[c];
                v.addr = addr;
                v.len = n;
                v.rs = rs;
                v.rl = rl;
                v.vol = vol;
                w.push(W::Vol(c, vol & 127));
            }
            if per != 0 {
                self.v[c].per = per;
                w.push(W::Per(c, per));
            }
            match ic & 15 {
                7 | 11 | 12 | 13 => self.eff[c] = (ic & 15, a),
                8 => {
                    self.brk = if a < 64 { u16::from(a) } else { 0 };
                    self.row = 63;
                }
                9 => {
                    if a != 0 {
                        self.speed = u16::from(a);
                    }
                }
                10 => w.push(W::Led(a != 0)),
                14 => {
                    self.v[c].vol = u16::from(a);
                    w.push(W::Vol(c, u16::from(a) & 127));
                }
                15 => {
                    self.v[c].rl = self.v[c].len;
                    self.v[c].rs = 0;
                }
                _ => {}
            }
        }
        self.row += 1;
        if self.row == 64 {
            self.row = self.brk;
            self.brk = 0;
            self.pos += 1;
            if self.pos > self.len - 1 {
                self.songend = true;
                self.pos = 0;
            }
            self.pat = self.seq[self.pos as usize];
        }
        w.push(W::Wait(T_BEAM));
        for c in 0..4 {
            let v = self.v[c];
            if v.addr != 0 {
                w.push(W::Loc(c, v.addr));
                w.push(W::Len(c, v.len));
                w.push(W::Wait(T_WRITE));
            }
        }
        w.push(W::Dma(0x8000 | mask));
    }

    fn between(&mut self, w: &mut Vec<W>) {
        w.push(W::Wait(T_BETWEEN));
        if self.tick == 0 {
            for c in 0..4 {
                let v = &mut self.v[c];
                if v.addr != 0 {
                    w.push(W::Loc(c, v.addr.wrapping_add(2 * u32::from(v.rs))));
                    w.push(W::Len(c, v.rl));
                    w.push(W::Wait(T_WRITE));
                    v.addr = 0;
                }
            }
        }
        for c in 0..4 {
            let (cmd, a) = self.eff[c];
            let v = &mut self.v[c];
            match cmd {
                7 => {
                    if (v.vol as i16) > i16::from(a) {
                        v.vol -= u16::from(a);
                    } else {
                        v.vol = 0;
                        self.eff[c] = (0, 0);
                    }
                    w.push(W::Vol(c, v.vol & 127));
                }
                12 => {
                    let p = (v.per as i16).wrapping_add(i16::from(a));
                    if p <= 1712 {
                        v.per = p as u16;
                        w.push(W::Per(c, p as u16));
                    }
                }
                11 => {
                    let p = (v.per as i16).wrapping_sub(i16::from(a));
                    if p >= 112 {
                        v.per = p as u16;
                        w.push(W::Per(c, p as u16));
                    }
                }
                13 => {
                    let p = if self.tick & 1 == 0 { v.per.wrapping_sub(u16::from(a)) } else { v.per.wrapping_add(u16::from(a)) };
                    w.push(W::Per(c, p));
                }
                _ => {}
            }
            w.push(W::Wait(T_WRITE));
        }
        self.tick += 1;
    }
}

struct Player {
    e: Ems,
}

impl Replayer for Player {
    fn format(&self) -> &'static str {
        "EMS"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.e.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        let mut w = Vec::new();
        self.e.tick(&mut w);
        for x in w {
            match x {
                W::Dma(v) => hw.dmacon(v),
                W::Loc(c, a) => hw.loc(c, a),
                W::Len(c, n) => hw.len(c, n),
                W::Per(c, p) => hw.per(c, p),
                W::Vol(c, v) => hw.vol(c, v),
                W::Led(on) => {
                    let x = hw.bus.read_byte(0xBFE001);
                    hw.bus.write_byte(0xBFE001, if on { x | 2 } else { x & !2 });
                }
                W::Wait(n) => hw.wait(n),
            }
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let (e, clear) = Ems::new(d, base);
    for o in clear {
        m.bus.write_bytes(base + o, &[0, 0]);
    }
    Some(Box::new(Player { e }))
}

#[cfg(test)]
#[path = "ems_tests.rs"]
mod tests;
