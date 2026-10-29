//! Synth Dream (`sdr.*`, with the shared sample bank `smp.set`; Laurens Tummers and John
//! Tonnard, 1991, from the game Monster Business).
//!
//! Clean-room, written from a behavioural specification and black-box runs of UADE's
//! player (its Paula write logs); no original code. A module holds an instrument table,
//! two envelope switch tables, one track list per voice, patterns of row commands and the
//! envelope data; the samples are in the companion bank. Four voices, one Paula channel
//! each, 50 Hz. A voice with a wave envelope plays a 16-byte synthesised buffer that the
//! player rewrites every tick in guest memory (a row of the wave table, then one byte
//! lowered by the envelope's depth), the others play the bank.
//!
//! Decisions where the specification leaves the choice to the port:
//! * An unknown row command (odd, or above 34) is read as row command 0; an unknown
//!   modifier (above 46 or odd) has no arguments and no effect.
//! * A slide set-up with a zero divisor uses index 0; a wave row above 15 reads on into
//!   the bytes after the wave table (zero past the end) and its depth byte is not
//!   subtracted; table indexes past the flat tables read zero.
//! * A voice that passes 256 pattern ends without reading a row (empty patterns) is
//!   switched off, so that a tick cannot loop for ever.
//! * A reload (`set_subsong`) zeroes all voice state, as UADE's fresh load does.
//! * Song end is sticky, set in the tick of the last voice's first track-list end marker;
//!   the player carries on from the start of the lists.
//! * The time of each write within a tick is rough (the specification gives ranges only);
//!   the order and the values are exact.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Synth Dream", detect, new };

const PERIODS: [u16; 144] = [
    53421, 50423, 47593, 44921, 42400, 40020, 37774, 35654, 33653, 31764, 29981, 28299, 26710, 25211, 23796, 22461, 21200,
    20010, 18887, 17827, 16826, 15882, 14991, 14149, 13355, 12606, 11898, 11230, 10600, 10005, 9444, 8914, 8413, 7941,
    7495, 7075, 6678, 6303, 5949, 5615, 5300, 5003, 4722, 4457, 4207, 3971, 3748, 3537, 3339, 3151, 2975, 2808, 2650,
    2501, 2361, 2228, 2103, 1985, 1874, 1769, 1669, 1576, 1487, 1404, 1325, 1251, 1180, 1114, 1052, 993, 937, 884, 835,
    788, 744, 702, 663, 625, 590, 557, 526, 496, 468, 442, 417, 394, 372, 351, 331, 313, 295, 279, 263, 248, 234, 221,
    209, 197, 186, 175, 166, 156, 148, 139, 131, 124, 117, 111, 104, 98, 93, 88, 83, 78, 74, 70, 66, 62, 59, 55, 52, 49,
    46, 44, 41, 39, 37, 35, 33, 31, 29, 28, 26, 25, 23, 22, 21, 20, 18, 17, 16, 16, 15, 14,
];

/// Flat word index of the tables, as the player has them back to back.
const DOWN_AT: usize = 144;
const UP_AT: usize = 544;
/// Byte index of the wave table in the flat tables.
const WAVES_AT: usize = 2 * 944;

/// The tables as one flat byte array: periods, down factors, up factors, wave rows.
fn tables() -> Vec<u8> {
    let mut t = Vec::new();
    for p in PERIODS {
        t.extend_from_slice(&p.to_be_bytes());
    }
    for i in 0..400 {
        t.extend_from_slice(&((8192.0 * (-(i as f64) / 4800.0).exp2()).round() as u16).to_be_bytes());
    }
    for i in 0..400 {
        t.extend_from_slice(&((8192.0 * (i as f64 / 4800.0).exp2()).round() as u16).to_be_bytes());
    }
    for r in 0..16 {
        for b in 0..16 {
            t.push(if b < r { 0x81 } else { 0x7F });
        }
    }
    t
}

/// The player's audio and buffer events of one tick, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    Dma(u16),
    Loc(usize, u32),
    Len(usize, u16),
    Per(usize, u16),
    Vol(usize, u16),
    /// The synth buffer of a voice is filled with a wave row ...
    Wave(usize, [u8; 16]),
    /// ... and then byte `.1` is set to `.2`.
    Sub(usize, usize, u8),
}

#[derive(Clone, Copy, Default)]
struct Env {
    pos: usize,
    steps: u8,
    reps: u8,
    seg: u8,
}

#[derive(Clone, Default)]
struct Voice {
    lstart: usize,
    lpos: usize,
    pstart: usize,
    ppos: usize,
    /// Track entry bytes 4..11.
    ent: [u8; 8],
    cnt: u16,
    dur: u16,
    note: u16,
    red: u16,
    inst: [u8; 32],
    env: [Env; 4],
    sfactor: u16,
    sstep: u16,
    sdelay: u16,
    noteoff: u8,
    rest: bool,
    legato: bool,
    off: bool,
    pend: [(u32, u16); 2],
}

pub struct Sdr {
    d: Vec<u8>,
    tb: Vec<u8>,
    sub: u16,
    subsongs: u16,
    bank: u32,
    buf: u32,
    v: [Voice; 4],
    flags: [bool; 4],
    start_flags: [bool; 4],
    pub songend: bool,
}

fn byte(d: &[u8], p: usize) -> u8 {
    d.get(p).copied().unwrap_or(0)
}

fn rd(d: &[u8], p: &mut usize) -> u16 {
    let x = be16(d, *p);
    *p += 2;
    x
}

impl Sdr {
    pub fn new(d: &[u8], bank: u32, buf: u32) -> Sdr {
        let subsongs = (((be32(d, 4) as i32).wrapping_sub(12)) >> 4).clamp(1, 255) as u16;
        let mut s = Sdr {
            d: d.to_vec(),
            tb: tables(),
            sub: 0,
            subsongs,
            bank,
            buf,
            v: Default::default(),
            flags: [true; 4],
            start_flags: [true; 4],
            songend: false,
        };
        s.init(0);
        s
    }

    /// The word `i` of the flat tables (zero past the end).
    fn tw(&self, i: usize) -> u16 {
        be16(&self.tb, 2 * i)
    }

    pub fn init(&mut self, sub: u16) {
        self.sub = sub;
        self.songend = false;
        self.v = Default::default();
        for k in 0..4 {
            let start = be32(&self.d, 12 + 16 * sub as usize + 4 * k) as usize;
            self.v[k].lstart = start;
            if byte(&self.d, start) == 255 {
                self.v[k].off = true;
                self.flags[k] = false;
            } else {
                self.flags[k] = true;
                self.take(k, start);
            }
        }
        self.start_flags = self.flags;
    }

    /// Takes the track entry at `a`.
    fn take(&mut self, k: usize, a: usize) {
        let p = be32(&self.d, a) as usize;
        let v = &mut self.v[k];
        v.pstart = p;
        v.ppos = p;
        for i in 0..8 {
            v.ent[i] = byte(&self.d, a + 4 + i);
        }
        v.lpos = a + 12;
    }

    fn end_test(&mut self, k: usize) {
        self.flags[k] = false;
        if self.flags.iter().all(|&f| !f) {
            self.songend = true;
            self.flags = self.start_flags;
        }
    }

    /// The end of a pattern (5.3); false if the voice has ended.
    fn pattern_end(&mut self, k: usize) -> bool {
        if self.v[k].ent[1] != 0 {
            let v = &mut self.v[k];
            v.ent[1] -= 1;
            v.ppos = v.pstart;
            return true;
        }
        self.v[k].pend = [(0, 0); 2];
        let e = self.v[k].lpos;
        match byte(&self.d, e) {
            0..=253 => self.take(k, e),
            254 => {
                self.end_test(k);
                let s = self.v[k].lstart;
                self.take(k, s);
            }
            _ => {
                self.v[k].off = true;
                self.end_test(k);
                return false;
            }
        }
        true
    }

    fn reset_env(&mut self, k: usize) {
        let v = &mut self.v[k];
        for i in 0..4 {
            v.env[i].pos = be32(&v.inst, 4 + 4 * i) as usize;
        }
    }

    fn instr(&mut self, k: usize, i: u16) {
        let mut i = i;
        if i as u8 == self.v[k].ent[2] {
            i = i & 0xFF00 | self.v[k].ent[3] as u16;
        }
        let a = be32(&self.d, 0) as usize + ((i as usize * 32) & 0xFFFF);
        for j in 0..32 {
            self.v[k].inst[j] = byte(&self.d, a + j);
        }
        self.reset_env(k);
    }

    /// Parses one row at the pattern position (5.2).
    fn row(&mut self, k: usize) {
        let mut p = self.v[k].ppos;
        let c = rd(&self.d, &mut p);
        // (duration, note, slide, volume, instrument)
        let (hd, hn, hs, hv, hi) = match c {
            0 => (false, false, false, false, false),
            2 | 30 => (true, false, false, false, false),
            4 => (false, true, false, false, false),
            6 => (false, false, false, true, false),
            8 => (false, false, false, false, true),
            10 => (true, true, false, false, false),
            12 => (true, false, false, true, false),
            14 => (true, false, false, false, true),
            16 => (false, true, false, true, false),
            18 => (false, true, false, false, true),
            20 => (true, true, false, true, false),
            22 => (true, true, false, false, true),
            24 => (true, true, true, false, false),
            26 => (false, true, false, true, true),
            28 => (true, true, false, true, true),
            32 | 34 => (true, true, true, false, true),
            _ => (false, false, false, false, false),
        };
        let d = if hd { rd(&self.d, &mut p) } else { 0 };
        let n = if hn { rd(&self.d, &mut p) } else { 0 };
        let (f, a, b) = if hs {
            let f = rd(&self.d, &mut p);
            let a = rd(&self.d, &mut p);
            (f, a, rd(&self.d, &mut p))
        } else {
            (0, 0, 0)
        };
        let vr = if hv { rd(&self.d, &mut p) } else { 0 };
        let i = if hi { rd(&self.d, &mut p) } else { 0 };
        {
            let v = &mut self.v[k];
            if hd {
                v.dur = d;
                v.cnt = d;
            } else {
                v.cnt = v.dur;
            }
            if hn {
                v.note = n;
            }
            if hv {
                v.red = vr;
            }
            if c == 30 {
                v.rest = true;
            }
        }
        if hs {
            let q = a.checked_div(d.wrapping_sub(b)).unwrap_or(0) as usize;
            let step = if f == 64768 { self.tw(UP_AT + q) } else { self.tw(DOWN_AT + q) };
            let v = &mut self.v[k];
            v.sdelay = b;
            v.sstep = step;
            v.sfactor = 8192;
        }
        if hi {
            self.instr(k, i);
        } else {
            self.reset_env(k);
        }
        // modifiers
        loop {
            let w = be16(&self.d, p);
            if !(36..32768).contains(&w) {
                break;
            }
            p += 2;
            match w {
                36 | 46 => {
                    let t = rd(&self.d, &mut p);
                    let kk = rd(&self.d, &mut p) as usize;
                    let table = if w == 36 { be32(&self.d, 4) } else { be32(&self.d, 8) } as usize;
                    let target = be32(&self.d, table.wrapping_add(kk));
                    self.v[k].pend[usize::from(w == 46)] = (target, t);
                }
                38 => self.v[k].legato = false,
                40 => self.v[k].legato = true,
                42 | 44 => {
                    let t = rd(&self.d, &mut p);
                    let kk = rd(&self.d, &mut p) as usize;
                    let step = self.tw(if w == 42 { DOWN_AT } else { UP_AT } + kk / 2);
                    let v = &mut self.v[k];
                    v.sdelay = t;
                    v.sstep = step;
                    v.sfactor = 8192;
                }
                _ => {}
            }
        }
        self.v[k].ppos = p;
    }

    /// The row fetch of a voice whose countdown is 0 (5.1); false if the voice ended.
    fn fetch(&mut self, k: usize) -> bool {
        for _ in 0..256 {
            let v = &mut self.v[k];
            v.sfactor = 0;
            v.noteoff = 0;
            v.rest = false;
            for e in v.env.iter_mut() {
                e.steps = 0;
                e.reps = 0;
            }
            if byte(&self.d, self.v[k].ppos) == 255 {
                if !self.pattern_end(k) {
                    return false;
                }
                continue;
            }
            self.row(k);
            return true;
        }
        self.v[k].off = true;
        false
    }

    fn step_env(&mut self, k: usize, e: usize, w: usize) -> u16 {
        let base = be32(&self.v[k].inst, 4 + 4 * e) as usize;
        let d = &self.d;
        let v = &mut self.v[k].env[e];
        if v.steps == 0 {
            if v.reps != 0 {
                v.pos = v.pos.wrapping_sub(v.seg as usize * w);
                v.steps = v.seg;
                v.reps -= 1;
            } else {
                if byte(d, v.pos) == 255 {
                    v.pos = base;
                }
                v.steps = byte(d, v.pos);
                v.seg = v.steps;
                v.reps = byte(d, v.pos + 1);
                v.pos += 2;
            }
        }
        let x = if w == 1 { byte(d, v.pos) as u16 } else { be16(d, v.pos) };
        v.pos += w;
        v.steps = v.steps.wrapping_sub(1);
        x
    }

    fn has(&self, k: usize, e: usize) -> bool {
        be32(&self.v[k].inst, 4 + 4 * e) != 0
    }

    /// One tick: the events in the order the player makes them.
    pub fn tick(&mut self) -> Vec<Ev> {
        let mut out = Vec::new();
        for k in 0..4 {
            if self.v[k].off {
                continue;
            }
            if self.v[k].cnt == 0 && !self.fetch(k) {
                continue;
            }
            if self.v[k].rest {
                out.push(Ev::Dma(1 << k));
                self.v[k].cnt = self.v[k].cnt.wrapping_sub(1);
                continue;
            }
            self.sound(k, &mut out);
        }
        out
    }

    fn sound(&mut self, k: usize, out: &mut Vec<Ev>) {
        // pending envelope switches (volume, pitch)
        for s in 0..2 {
            let v = &mut self.v[k];
            let (target, delay) = v.pend[s];
            if target != 0 {
                if delay == 0 {
                    v.env[s].pos = (target & 0xFFFF) as usize;
                    v.pend[s].0 = target & 0xFFFF;
                    v.env[s].steps = 0;
                    v.env[s].reps = 0;
                }
                v.pend[s].1 = delay.wrapping_sub(1);
            }
        }
        // volume
        let e = if self.has(k, 0) { self.step_env(k, 0, 1) } else { 64 };
        let v = &self.v[k];
        let r = (e as u8).wrapping_sub(v.red as u8).wrapping_sub(v.ent[5]);
        let vol = if r >= 128 { 0 } else { r };
        // pitch
        if self.has(k, 2) {
            let x = self.step_env(k, 2, 1) as u8;
            self.v[k].noteoff = x;
        }
        let pitch = if self.has(k, 1) { Some(self.step_env(k, 1, 2) as i16) } else { None };
        let v = &self.v[k];
        let mut lo = v.note as u8;
        if v.inst[1] == 0 {
            lo = lo.wrapping_add(v.ent[0]);
        }
        lo = lo.wrapping_add(v.noteoff);
        let idx = (v.note & 0xFF00) as usize | lo as usize;
        let base = self.tw(idx) as u32;
        let mut period = base;
        if let Some(p) = pitch {
            let f = if p >= 0 { self.tw(UP_AT + p as usize) } else { self.tw(DOWN_AT + (-(p as i32)) as usize) };
            period = (base * f as u32) >> 13 & 0xFFFF;
        }
        if self.v[k].sfactor != 0 {
            let v = &mut self.v[k];
            if v.sdelay != 0 {
                v.sdelay -= 1;
            } else {
                v.sfactor = ((v.sfactor as u32 * v.sstep as u32) >> 13) as u16;
                period = (period * v.sfactor as u32) >> 13 & 0xFFFF;
            }
        }
        // wave envelope
        let synth = self.has(k, 3);
        if synth {
            let w = self.step_env(k, 3, 2);
            let (row, depth) = ((w >> 8) as usize, w as u8);
            let mut b = [0u8; 16];
            for (i, x) in b.iter_mut().enumerate() {
                *x = byte(&self.tb, WAVES_AT + 16 * row + i);
            }
            out.push(Ev::Wave(k, b));
            if row < 16 {
                out.push(Ev::Sub(k, row, b[row].wrapping_sub(depth)));
            }
        }
        // fine tune
        let ft = be16(&self.v[k].inst, 2) as i16 as i32;
        if ft > 0 {
            period = (period * self.tw(UP_AT + (ft / 2) as usize) as u32) >> 13 & 0xFFFF;
        } else if ft < 0 {
            period = (period * self.tw(DOWN_AT + (-ft / 2) as usize) as u32) >> 13 & 0xFFFF;
        }
        // Paula writes
        let v = &self.v[k];
        let (start, slen, lp, llen) = if synth {
            let a = self.buf + 16 * k as u32;
            (a, 8, a, 8)
        } else {
            (
                self.bank.wrapping_add(be32(&v.inst, 20)),
                be16(&v.inst, 24),
                self.bank.wrapping_add(be32(&v.inst, 26)),
                be16(&v.inst, 30),
            )
        };
        let dd = v.dur.wrapping_sub(1);
        let c = v.cnt;
        let on = 0x8000 | 1 << k;
        if dd <= c {
            let (a, l) = if dd < c { (start, slen) } else { (lp, llen) };
            out.extend([Ev::Loc(k, a), Ev::Len(k, l), Ev::Per(k, period as u16), Ev::Vol(k, vol as u16), Ev::Dma(on)]);
        } else {
            out.extend([Ev::Per(k, period as u16), Ev::Vol(k, vol as u16)]);
            if c == 1 {
                if !v.legato {
                    out.push(Ev::Dma(1 << k));
                }
            } else {
                out.push(Ev::Dma(on));
            }
        }
        self.v[k].cnt = c.wrapping_sub(1);
    }
}

/// The module layout test (1.2 of the specification).
fn detect(d: &[u8]) -> bool {
    let l = |o| be32(d, o);
    let l0 = l(0) as usize;
    if l0 == 0 || l0 >= 1 << 31 || l0 % 2 != 0 || d.len() <= l0 || be16(d, l0 - 2) != 65535 {
        return false;
    }
    for o in [4, 8] {
        let x = l(o);
        if x >> 16 != 0 || x == 0 || x >= 32768 || x % 4 != 0 {
            return false;
        }
    }
    for o in [12, 16, 20, 24] {
        let x = l(o);
        if x >> 16 != 0 || x == 0 || x >= 32768 || x % 2 != 0 {
            return false;
        }
        let w = be16(d, x as usize - 2);
        if x != 40 && w != 65279 && w != 65535 {
            return false;
        }
    }
    let t = l(l(4) as usize + 4) as i32;
    (d.len() as i32) > t
}

/// The companion bank: `SMP.<rest>` for a name starting `SDR.` (any case), else `smp.set`.
fn bank(m: &Machine, file: &str) -> Option<Vec<u8>> {
    let name = file.rsplit('/').next().unwrap_or(file);
    if name.len() > 4 && name.is_char_boundary(4) && name[..4].eq_ignore_ascii_case("sdr.") {
        if let Ok(s) = crate::deli::read_companion(m, &format!("SMP.{}", &name[4..])).1 {
            return Some(s);
        }
    }
    crate::deli::read_companion(m, "smp.set").1.ok()
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let smp = bank(m, &file)?;
    let ba = m.alloc((smp.len() as u32).max(4));
    let buf = m.alloc(64);
    if ba == 0 || buf == 0 {
        return None;
    }
    m.bus.write_bytes(ba, &smp);
    Some(Box::new(Player { e: Sdr::new(d, ba, buf), title: file, buf }))
}

struct Player {
    e: Sdr,
    title: String,
    buf: u32,
}

impl Replayer for Player {
    fn format(&self) -> &'static str {
        "Synth Dream"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.e.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.e.subsongs - 1, self.e.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        hw.dmacon(0x000F);
        self.e.init(n.min(self.e.subsongs - 1));
    }

    fn tick(&mut self, hw: &mut Hw) {
        hw.wait(350);
        let mut gap = true;
        for ev in self.e.tick() {
            if gap {
                hw.wait(250);
                gap = false;
            }
            match ev {
                Ev::Loc(c, a) => hw.loc(c, a),
                Ev::Len(c, l) => {
                    hw.wait(6);
                    hw.len(c, l)
                }
                Ev::Per(c, p) => {
                    hw.wait(7);
                    hw.per(c, p)
                }
                Ev::Vol(c, v) => {
                    hw.wait(180);
                    hw.vol(c, v)
                }
                Ev::Dma(v) => {
                    hw.wait(20);
                    hw.dmacon(v);
                    gap = true;
                }
                Ev::Wave(c, b) => hw.bus.write_bytes(self.buf + 16 * c as u32, &b),
                Ev::Sub(c, i, x) => {
                    hw.wait(30);
                    hw.bus.write_bytes(self.buf + 16 * c as u32 + i as u32, &[x]);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "synthdream_tests.rs"]
mod tests;
