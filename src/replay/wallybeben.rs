//! Wally Beben (`wb.*`), the music system of Ariston and Hewson games (Elite, Colossus
//! Chess X, Dark Side, Superman, Circus Games, Hammerfist, Hawk Eye, Total Eclipse, SDI,
//! Time Machine ...): about twenty builds of one engine.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Wally Beben player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A module is a raw 68000 memory image: a jump table, the game's own replay routine,
//! its tables and variables, the songs, the patterns and the samples. The routine is
//! never run. The tables (songs, patterns, samples, instruments, periods, the voice
//! block) are located in the image by short instruction signatures, the build's
//! differences are read from the image the same way, and the engine is written here
//! from its behaviour: four voices, one Paula channel each, 50 Hz, a tempo divider,
//! order lists of patterns, notes with a duration, a sample and an instrument
//! (attack and release envelope, vibrato, arpeggio), portamento and slides.

use crate::paula::LINE;
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Wally Beben", detect, new };

/// The words the signatures are made of are the encodings of the image's own
/// instructions; the replayer reads them, it does not run them.
const GUARD: u32 = 1024;

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// First position at or after `from` where `pat` matches (negative = any byte).
fn find(d: &[u8], from: usize, pat: &[i16]) -> Option<usize> {
    if d.len() < pat.len() {
        return None;
    }
    (from..=d.len() - pat.len()).find(|&i| pat.iter().enumerate().all(|(k, &p)| p < 0 || d[i + k] as i16 == p))
}

/// What the build is, read from the image.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Flags {
    /// The new image layout (word at 0x14 is the register-load opcode).
    newl: bool,
    /// The older engine of Foundation's Waste and Phantasm.
    oldest: bool,
    /// The sustain test divides instead of shifting.
    sdiv: bool,
    /// A volume that reaches exactly zero in the release also stops the voice.
    relz: bool,
    /// The slide window is compared against the order cursor field (Elite).
    slide_cursor: bool,
    /// Init writes DMACON 0x000F first.
    init_dma: bool,
    /// The sample command is `0xE0 number`.
    e0cmd: bool,
    /// Sample flag bit 1.
    sflag2: bool,
    /// The division register is cleared before the sustain division.
    d5clean: bool,
}

/// Where the tables of the image are (image offsets).
#[derive(Clone, Debug)]
struct Lay {
    origin: u32,
    /// The subsong count plus one.
    imm: u32,
    songs: usize,
    v: usize,
    patterns: usize,
    sinfo: usize,
    stab: usize,
    instr: usize,
    intervals: usize,
    arpbuf: usize,
    periods: usize,
    pc: usize,
    /// The table whose first long is the empty-sample pointer (new layout).
    empty: Option<usize>,
    flags: Flags,
    /// Engine variables that period lookups past the table can read.
    var_enable: Option<usize>,
    var_counter: Option<usize>,
    var_dm: Option<usize>,
    var_tc: Option<usize>,
    var_tr: Option<usize>,
    var_bytes: Option<usize>,
    var_chan: Option<usize>,
}

// ---------------------------------------------------------------------------------
// Recognition

/// The structure the player tests: a branch to the stop routine, which calls a
/// routine and clears two bytes; the play routine saving all registers and calling
/// the engine, whose first instruction loads the request byte (old layout) or
/// reloads the registers (new layout).
fn recognise(d: &[u8]) -> bool {
    if d.len() < 0x40 || be16(d, 0) != 0x6000 {
        return false;
    }
    let disp = be16(d, 2) as usize;
    if disp == 0 || disp & 0x8000 != 0 || disp & 1 != 0 {
        return false;
    }
    let stop = 2 + disp;
    if rb(d, stop) != 0x61 {
        return false;
    }
    let p = stop + if rb(d, stop + 1) == 0 { 4 } else { 2 };
    if be16(d, p) != 0x4239 || be16(d, p + 6) != 0x4239 || be16(d, p + 12) != 0x4E75 {
        return false;
    }
    if be32(d, 4) != 0x48E7_FFFE || be16(d, 8) != 0x6100 {
        return false;
    }
    let t = (10 + be16(d, 10) as i16 as i64) as usize;
    be32(d, t) == 0x4CF9_00FF || be16(d, t + 4) == 0x1039
}

fn abs_at(d: &[u8], o: usize) -> u32 {
    be32(d, o)
}

fn locate(d: &[u8]) -> Option<Lay> {
    if !recognise(d) {
        return None;
    }
    let newl = be16(d, 0x14) == 0x4CF9;
    // the first RTS, at an even offset; the request byte follows it
    let rts = (0..d.len().min(0x100) / 2).map(|i| 2 * i).find(|&o| be16(d, o) == 0x4E75)?;
    let r0 = rts + 2;
    let p = find(d, r0, &[0x10, 0x39, -1, -1, -1, -1])?;
    let origin = abs_at(d, p + 2).wrapping_sub(r0 as u32);
    let off = |a: u32| -> Option<usize> {
        let o = a.wrapping_sub(origin) as usize;
        if o < d.len() {
            Some(o)
        } else {
            None
        }
    };
    let q = find(d, p, &[0x22, 0x3C, -1, -1, -1, -1])?;
    let imm = abs_at(d, q + 2);
    if !(2..=64).contains(&imm) {
        return None;
    }
    let sl = find(d, q, &[0x41, 0xF9, -1, -1, -1, -1])?;
    let songs = off(abs_at(d, sl + 2))?;
    let vp = find(d, sl + 1, &[0x41, 0xF9, -1, -1, -1, -1, 0xD1, 0xC1])?;
    let v = off(abs_at(d, vp + 2))?;
    if v < 8 || v + 0x98 > d.len() {
        return None;
    }
    let a = find(d, 0, &[0xE5, 0x80, 0x43, 0xF9, -1, -1, -1, -1]);
    let b = find(d, 0, &[0xE5, 0x00, 0x43, 0xF9, -1, -1, -1, -1]);
    let pp = match (a, b) {
        (Some(x), Some(y)) => x.min(y),
        (Some(x), None) | (None, Some(x)) => x,
        _ => return None,
    };
    let patterns = off(abs_at(d, pp + 4))?;
    let after = |key: &[i16], op: u8| -> Option<usize> {
        let k = find(d, 0, key)?;
        let l = find(d, k + key.len(), &[op as i16, 0xF9, -1, -1, -1, -1])?;
        if l > k + 40 {
            return None;
        }
        off(abs_at(d, l + 2))
    };
    let sinfo = after(&[0x10, 0x28, 0x00, 0x4C], 0x43)?;
    let instr = after(&[0x10, 0x28, 0x00, 0x50], 0x45)?;
    // a lea directly before a shift / add: the operand of that lea
    let lea_before = |key: &[i16]| -> Option<usize> {
        let mut from = 0;
        while let Some(i) = find(d, from, key) {
            if i >= 6 && d[i - 6] == 0x43 && d[i - 5] == 0xF9 {
                return off(abs_at(d, i - 4));
            }
            from = i + 1;
        }
        None
    };
    let stab = lea_before(&[0xE5, 0x84])?;
    let intervals = lea_before(&[0xD3, 0xC5])?;
    let ab = find(d, 0, &[0xD3, 0xC5, 0x10, 0x11, 0x43, 0xF9, -1, -1, -1, -1, 0xD3, 0xC4, 0x12, 0x80])?;
    let arpbuf = off(abs_at(d, ab + 6))?;
    let k1 = find(d, 0, &[0x10, 0x28, 0x00, 0x08, 0xE3, 0x00, 0x43, 0xF9, -1, -1, -1, -1]);
    let k2 = find(d, 0, &[0x10, 0x28, 0x00, 0x08, 0xD0, 0x00, 0x43, 0xF9, -1, -1, -1, -1]);
    let pk = match (k1, k2) {
        (Some(x), Some(y)) => x.min(y),
        (Some(x), None) | (None, Some(x)) => x,
        _ => return None,
    };
    let periods = off(abs_at(d, pk + 8))?;
    let pcl = find(d, pk + 12, &[0x43, 0xF9, -1, -1, -1, -1])?;
    if pcl > pk + 12 + 12 {
        return None;
    }
    let pc = off(abs_at(d, pcl + 2))?;
    let empty = if newl {
        find(d, 0, &[0x3C, 0x36, 0x30, 0x00]).and_then(|m| off(abs_at(d, m + 6)))
    } else {
        None
    };
    let flags = Flags {
        newl,
        oldest: find(d, 0, &[0x1A, 0x3C, 0x00, 0xEF]).is_some(),
        sdiv: find(d, 0, &[0x1A, 0x28, 0x00, 0x10, 0x8A, 0xC4]).is_some(),
        relz: find(d, 0, &[0x18, 0x28, 0x00, 0x5C, 0x90, 0x04, 0x4A, 0x00, 0x67, 0x00]).is_some(),
        slide_cursor: find(d, 0, &[0x06, 0x00, 0x00, 0x00, 0xB0, 0x28, 0x00, 0x0C]).is_some(),
        init_dma: find(d, 0, &[0x3A, 0x3C, 0x00, 0x0F, 0x33, 0xC5]).is_some(),
        e0cmd: find(d, 0, &[0xB0, 0x3C, 0x00, 0xE0]).is_some(),
        sflag2: find(d, 0, &[0x08, 0x28, 0x00, 0x01, 0x00, 0x78]).is_some(),
        d5clean: {
            let m = find(d, 0, &[0x4A, 0x28, 0x00, 0x14, 0x67, 0x00, -1, -1, 0x4A, 0x28, 0x00, 0x6C]);
            m.is_some_and(|m| {
                (m.saturating_sub(8)..m).any(|i| {
                    let w = be16(d, i);
                    i + 2 <= m && (w == 0x7A00 || w == 0x4285)
                })
            })
        },
    };
    // engine variables
    let var_ptr = |m: Option<usize>, at: usize| m.and_then(|m| off(abs_at(d, m + at)));
    let m_en = find(d, 0, &[0x4A, 0x39, -1, -1, -1, -1, 0x67, 0x00, -1, -1, 0x13, 0xFC, 0x00, 0x03, -1, -1, -1, -1]);
    let var_enable = var_ptr(m_en, 2);
    let var_counter = var_ptr(m_en, 14);
    let var_dm = var_ptr(find(d, 0, &[0x03, 0xB9, -1, -1, -1, -1]), 2);
    let m_tc = find(d, 0, &[0x53, 0x39, -1, -1, -1, -1, 0x6A, 0x00, -1, -1, 0x13, 0xF9, -1, -1, -1, -1, -1, -1, -1, -1]);
    let var_tc = var_ptr(m_tc, 2);
    let var_tr = var_ptr(m_tc, 12);
    let var_bytes = var_ptr(find(d, 0, &[0x33, 0xC5, 0x00, 0xDF, 0xF0, 0x96, 0x13, 0xC4, -1, -1, -1, -1]), 8);
    let var_chan = [
        find(d, 0, &[0x13, 0xE8, 0x00, 0x00, -1, -1, -1, -1, 0x4A, 0x39]).map(|m| m + 4),
        find(d, 0, &[0x13, 0xD0, -1, -1, -1, -1, 0x4A, 0x39]).map(|m| m + 2),
    ]
    .into_iter()
    .flatten()
    .min()
    .and_then(|o| off(abs_at(d, o)));
    Some(Lay {
        origin,
        imm,
        songs,
        v,
        patterns,
        sinfo,
        stab,
        instr,
        intervals,
        arpbuf,
        periods,
        pc,
        empty,
        flags,
        var_enable,
        var_counter,
        var_dm,
        var_tc,
        var_tr,
        var_bytes,
        var_chan,
    })
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

// ---------------------------------------------------------------------------------
// The engine

/// What an event decoded to.
enum Ev {
    Stop,
    Jump(u8),
    Note(u8, usize),
}

struct Wb {
    lay: Lay,
    base: u32,
    pristine: Vec<u8>,
    mem: Vec<u8>,
    title: String,
    nsongs: u16,
    sub: u16,
    started: bool,
    /// A note was started in this tick.
    evented: bool,
    enable: bool,
    tr: u8,
    tc: u8,
    dm: u8,
    d5: u32,
    /// Pattern pointers and order-list starts, image offsets.
    pp: [usize; 4],
    ol: [usize; 4],
    loop_lc: [u32; 4],
    loop_off: [u16; 4],
    loop_len: [u16; 4],
    slide_dir: [bool; 4],
    slide_delta: [u16; 4],
    slide_tgt: [u16; 4],
    /// Voices that have not wrapped their order list yet, for the song-end report.
    armed: u8,
    songend: bool,
    /// The last pattern loaded during the current event (oldest builds).
    loaded: Option<usize>,
    /// Bytes consumed by the last event decoded (an engine variable).
    bytes: u8,
}

impl Wb {
    fn new(d: &[u8], base: u32, lay: Lay, title: String) -> Wb {
        let mut mem = d.to_vec();
        let nsongs = (lay.imm - 1) as u16;
        // Circus Games' six-subsong build carries two values 80 too high
        if nsongs == 6 && be32(&mem, 3090) == 0x9298 && be16(&mem, 3128) == 0x0FA0 {
            put32(&mut mem, 3090, 0x9298 - 0x50);
            put16(&mut mem, 3128, 0x0FA0 - 0x50);
        }
        // sample start addresses must be even: if the lowest is odd, all move up one
        let mut n = 0;
        while n < 64 && rb(&mem, lay.sinfo + 8 * n) == n as u8 && be16(&mem, lay.sinfo + 8 * n + 2) != 0 {
            n += 1;
        }
        let m = (0..n).map(|k| be32(&mem, lay.stab + 4 * k).wrapping_sub(lay.origin)).min();
        if m.is_some_and(|m| m & 1 != 0) {
            for k in 0..n {
                let o = lay.stab + 4 * k;
                let w = be32(&mem, o).wrapping_add(1);
                put32(&mut mem, o, w);
            }
        }
        Wb {
            lay,
            base,
            pristine: mem.clone(),
            mem,
            title,
            nsongs,
            sub: 0,
            started: false,
            evented: false,
            enable: false,
            tr: 0,
            tc: 0,
            dm: 0,
            d5: 0,
            pp: [0; 4],
            ol: [0; 4],
            loop_lc: [0; 4],
            loop_off: [0; 4],
            loop_len: [0; 4],
            slide_dir: [false; 4],
            slide_delta: [0; 4],
            slide_tgt: [0; 4],
            armed: 0x0F,
            songend: false,
            loaded: None,
            bytes: 0,
        }
    }

    // ---- memory ----

    fn rb(&self, o: usize) -> u8 {
        rb(&self.mem, o)
    }

    fn wb(&mut self, o: usize, v: u8) {
        if let Some(b) = self.mem.get_mut(o) {
            *b = v;
        }
    }

    fn rw(&self, o: usize) -> u16 {
        be16(&self.mem, o)
    }

    fn ww(&mut self, o: usize, v: u16) {
        if o + 2 <= self.mem.len() {
            put16(&mut self.mem, o, v);
        }
    }

    fn rl(&self, o: usize) -> u32 {
        be32(&self.mem, o)
    }

    /// The voice-block field `f` of voice `v`: the block is a flat byte array and
    /// voice `v` addresses it from `V + v`, so fields alias across voices.
    fn g(&self, f: usize, v: usize) -> u8 {
        self.rb(self.lay.v + f + v)
    }

    fn s(&mut self, f: usize, v: usize, x: u8) {
        let o = self.lay.v + f + v;
        self.wb(o, x);
    }

    /// Period table lookup, in the running memory.
    fn p(&self, i: u8) -> u16 {
        self.rw(self.lay.periods + ((2 * i as usize) & 0xFF))
    }

    fn p1(&self, i: u8) -> u16 {
        self.rw(self.lay.periods + ((2 * i as usize) & 0xFF) + 2)
    }

    fn outper(&self, v: usize) -> u16 {
        self.rw(self.lay.v - 8 + 2 * v)
    }

    fn set_outper(&mut self, v: usize, x: u16) {
        let o = self.lay.v - 8 + 2 * v;
        self.ww(o, x);
    }

    fn basper(&self, v: usize) -> u16 {
        self.rw(self.lay.pc + 2 * v)
    }

    fn set_basper(&mut self, v: usize, x: u16) {
        let o = self.lay.pc + 2 * v;
        self.ww(o, x);
    }

    fn setperiod(&mut self, v: usize) {
        let p = self.p(self.g(0x08, v));
        self.set_basper(v, p);
        self.set_outper(v, p);
    }

    /// Mirrors the engine's own variables into the image, where a period lookup
    /// past the table's end can read them.
    fn sync_vars(&mut self, voice: usize, bytes: usize) {
        let l = self.lay.clone();
        let vals = [
            (l.var_enable, self.enable as u8),
            (l.var_counter, voice as u8),
            (l.var_dm, self.dm),
            (l.var_tc, self.tc),
            (l.var_tr, self.tr),
            (l.var_bytes, bytes as u8),
            (l.var_chan, (0x10 * voice) as u8),
        ];
        for (a, x) in vals {
            if let Some(a) = a {
                self.wb(a, x);
            }
        }
    }

    // ---- start / stop ----

    fn silence(&mut self, hw: &mut Hw) {
        for v in 0..4 {
            hw.vol(v, 0);
        }
    }

    fn stop(&mut self, hw: &mut Hw) {
        self.enable = false;
        self.silence(hw);
        self.songend = true;
    }

    fn init(&mut self, r: u16, hw: &mut Hw) {
        let l = self.lay.clone();
        if l.flags.init_dma {
            hw.dmacon(0x000F);
        }
        let row = l.songs + 16 * (r as usize - 1);
        for v in 0..4 {
            self.ol[v] = self.rl(row + 4 * v).wrapping_sub(l.origin) as usize;
        }
        for v in (0..4).rev() {
            self.s(0x14, v, 0);
            self.s(0x18, v, 0);
            self.s(0x0C, v, 0);
            self.load_pattern(v);
        }
        self.silence(hw);
        for v in 0..4 {
            self.s(0x60, v, 0);
        }
        self.tc = 0;
        self.enable = true;
        self.dm = 0x0F;
        self.armed = 0x0F;
        self.songend = false;
    }

    /// Reads the next pattern number from the voice's order list.
    fn load_pattern(&mut self, v: usize) {
        let oldest = self.lay.flags.oldest;
        for _ in 0..GUARD {
            let cur = self.g(0x0C, v);
            let x = self.rb(self.ol[v] + cur as usize);
            self.s(0x0C, v, cur.wrapping_add(1));
            let pat = if oldest {
                if x < 0x80 {
                    Some(((x as usize) * 4) & 0xFF)
                } else {
                    None
                }
            } else if x <= 0xC0 {
                Some(4 * x as usize)
            } else {
                None
            };
            if let Some(i) = pat {
                self.pp[v] = self.rl(self.lay.patterns + i).wrapping_sub(self.lay.origin) as usize;
                self.loaded = Some(self.lay.patterns + i);
                return;
            }
            if x == 0xFF {
                self.s(0x0C, v, 0);
                self.wrap(v);
            } else if oldest {
                self.s(0x20, v, x.wrapping_add(0x40));
            } else {
                self.s(0x20, v, x.wrapping_add(0x20));
            }
        }
        self.pp[v] = 0;
    }

    fn wrap(&mut self, v: usize) {
        self.armed &= !(1 << v);
        if self.armed == 0 {
            self.songend = true;
            self.armed = 0x0F;
        }
    }

    // ---- events ----

    fn build_arp(&mut self, v: usize, mask: u8) {
        let mut i = self.g(0x04, v) as usize + 1;
        for k in 0..8 {
            if mask & (1 << k) != 0 {
                let x = self.rb(self.lay.intervals + k);
                self.wb(self.lay.arpbuf + i, x);
                i += 1;
            }
        }
        self.wb(self.lay.arpbuf + i, 0xFF);
        self.s(0x24, v, 0);
    }

    #[allow(unused_assignments)]
    fn decode(&mut self, v: usize) -> Ev {
        let fl = self.lay.flags;
        let mut cnt = 0usize;
        let mut guard = 0;
        macro_rules! rd {
            () => {{
                let x = self.rb(self.pp[v] + cnt);
                cnt += 1;
                x
            }};
        }
        'event: loop {
            guard += 1;
            if guard > GUARD {
                return Ev::Stop;
            }
            let mut x = rd!();
            if x == 0xFF {
                self.load_pattern(v);
                cnt = 0;
                continue 'event;
            }
            if x == 0xFE {
                return Ev::Stop;
            }
            if x == 0xFC && !fl.oldest {
                {
                    let y = rd!();
                    return Ev::Jump(y);
                }
            }
            if x > 0xEF {
                self.tr = x & 0x0F;
                x = rd!();
            }
            // sample
            if fl.e0cmd {
                if x == 0xE0 {
                    let n = rd!();
                    self.s(0x4C, v, n.wrapping_mul(8));
                    x = rd!();
                    if x == 0xFF {
                        self.load_pattern(v);
                        cnt = 0;
                        continue 'event;
                    }
                }
            } else if x > 0xDF {
                self.s(0x4C, v, (x & 15) * 8);
                x = rd!();
                if x == 0xFF {
                    self.load_pattern(v);
                    cnt = 0;
                    continue 'event;
                }
            }
            // instrument
            if x > 0xBF {
                self.s(0x50, v, (x & 0x1F) * 8);
                x = rd!();
                if x == 0xFF {
                    self.load_pattern(v);
                    cnt = 0;
                    continue 'event;
                }
            }
            // duration
            if x > 0x7F {
                self.s(0x10, v, x & 0x3F);
                x = rd!();
            }
            let note = match x {
                0..=0x5F => x,
                0x7F => {
                    self.s(0x2C, v, 1);
                    self.s(0x30, v, 0);
                    self.s(0x18, v, 1);
                    rd!()
                }
                0x7E => {
                    self.s(0x30, v, 1);
                    self.s(0x2C, v, 0);
                    self.s(0x18, v, 1);
                    rd!()
                }
                0x7D | 0x7C => {
                    self.s(0x18, v, if x == 0x7D { 1 } else { 2 });
                    let b = rd!();
                    self.s(0x1C, v, b);
                    self.build_arp(v, b);
                    rd!()
                }
                0x7B => {
                    self.s(0x1C, v, 0);
                    self.s(0x18, v, 1);
                    let b = rd!();
                    let t = b.wrapping_add(self.g(0x20, v));
                    self.s(0x24, v, t);
                    let sp = rd!();
                    self.s(0x28, v, sp);
                    rd!()
                }
                0x7A => {
                    let b = rd!();
                    self.s(0x44, v, b);
                    rd!()
                }
                _ => x,
            };
            return Ev::Note(note, cnt);
        }
    }

    /// Starts a new event on voice `v`. False if the song stopped.
    fn new_event(&mut self, v: usize, hw: &mut Hw) -> bool {
        if self.g(0x18, v) != 2 {
            self.s(0x18, v, 0);
            self.s(0x1C, v, 0);
        }
        for f in [0x24, 0x2C, 0x30, 0x6C] {
            self.s(f, v, 0);
        }
        self.loaded = None;
        let (note, cnt) = match self.decode(v) {
            Ev::Stop => {
                self.stop(hw);
                return false;
            }
            Ev::Jump(y) => {
                if (1..=self.lay.imm as u16).contains(&(y as u16)) {
                    self.init(y as u16, hw);
                } else {
                    self.stop(hw);
                }
                return false;
            }
            Ev::Note(n, c) => (n, c),
        };
        self.bytes = cnt as u8;
        self.sync_vars(v, cnt);
        let bit = 1u8 << v;
        self.dm &= !bit;
        if self.evented {
            hw.wait(WAIT_NOTE);
        }
        hw.dmacon(bit as u16);
        self.evented = true;
        hw.wait(WAIT_OFF_LEN);
        self.d5 = bit as u32;
        self.sample_setup(v, hw);
        self.instrument_setup(v);
        let t = note.wrapping_add(self.g(0x20, v));
        self.s(0x08, v, t);
        if self.lay.flags.oldest {
            let flags = if self.g(0x1C, v) == 0 {
                self.rb(self.lay.instr + self.g(0x50, v) as usize + 7)
            } else {
                self.stale_flags(v)
            };
            if flags & 1 != 0 {
                self.s(0x34, v, t);
                self.s(0x08, v, t.wrapping_add(1));
            }
        }
        self.setperiod(v);
        self.pp[v] += cnt;
        self.s(0x14, v, self.g(0x10, v));
        self.s(0x48, v, self.g(0x44, v));
        self.s(0x38, v, self.g(0x04, v).wrapping_add(1));
        true
    }

    /// The instrument-flags byte the oldest builds read through a register that
    /// was not reloaded while an instrument arpeggio is active.
    fn stale_flags(&self, v: usize) -> u8 {
        let o = self.lay.origin as usize;
        match self.loaded {
            Some(at) => (self.rl(at + 4).wrapping_sub(o as u32)) as u8,
            None => {
                if v == 3 {
                    (self.ol[0]) as u8
                } else {
                    (self.pp[v + 1]) as u8
                }
            }
        }
    }

    fn sample_setup(&mut self, v: usize, hw: &mut Hw) {
        let l = self.lay.clone();
        let r = l.sinfo + self.g(0x4C, v) as usize;
        let number = self.rb(r) as usize;
        let flag = self.rb(r + 1);
        self.s(0x78, v, flag);
        let (w2, w4, w6) = (self.rw(r + 2), self.rw(r + 4), self.rw(r + 6));
        let len = if !l.flags.oldest && flag != 0 { w4.wrapping_add(w6) } else { w2 };
        self.loop_off[v] = w4;
        self.loop_len[v] = w6;
        hw.len(v, len >> 1);
        hw.wait(WAIT_LEN_ADDR);
        if l.flags.sflag2 && flag & 2 != 0 {
            let t0 = self.rl(l.stab).wrapping_add(4 * number as u32).wrapping_add(w4 as u32);
            put32(&mut self.mem, l.stab, t0);
            let a = t0.wrapping_sub(l.origin).wrapping_add(self.base);
            // the original stores the address through the LEN and PER registers
            hw.bus.write_word(0xDFF0A4 + 16 * v as u32, (a >> 16) as u16);
            hw.bus.write_word(0xDFF0A6 + 16 * v as u32, a as u16);
            self.loop_lc[v] = self.rl(l.stab + 4 * number).wrapping_sub(l.origin);
        } else {
            let start = self.rl(l.stab + 4 * number).wrapping_sub(l.origin);
            hw.loc(v, self.base.wrapping_add(start));
            self.loop_lc[v] = start;
        }
    }

    fn instrument_setup(&mut self, v: usize) {
        let a = self.lay.instr + self.g(0x50, v) as usize;
        let ins: Vec<u8> = (0..8).map(|k| self.rb(a + k)).collect();
        self.s(0x60, v, ins[0]);
        self.s(0x54, v, ins[1]);
        self.s(0x58, v, ins[2]);
        self.s(0x5C, v, ins[3]);
        self.s(0x70, v, ins[4]);
        self.s(0x68, v, ins[5]);
        if ins[6] != 0 {
            self.s(0x1C, v, ins[6]);
            self.s(0x18, v, if self.lay.flags.oldest { ins[6] } else { 0x7C });
            self.build_arp(v, ins[6]);
        }
        if ins[7] != 0 && ins[7] & 8 != 0 {
            self.s(0x27, v, 1);
        }
    }

    // ---- effects ----

    fn effects(&mut self, v: usize) {
        if self.g(0x08, v) == 0 {
            return;
        }
        let oldest = self.lay.flags.oldest;
        if self.g(0x18, v) == 0 {
            if self.g(0x68, v) != 0 {
                self.vibrato(v);
                return;
            }
            if !oldest {
                self.s(0x80, v, self.g(0x4E, v));
            }
        }
        self.arp_or_slide(v);
    }

    fn arp_or_slide(&mut self, v: usize) {
        let l = self.lay.clone();
        if self.g(0x1C, v) != 0 {
            let mut i = self.g(0x38, v) as usize;
            let mut x = self.rb(l.arpbuf + i);
            for _ in 0..2 {
                if x != 0xFF {
                    break;
                }
                i = self.g(0x04, v) as usize;
                self.s(0x38, v, i as u8);
                x = self.rb(l.arpbuf + i);
            }
            let p = self.p(x.wrapping_add(self.g(0x08, v)));
            self.set_outper(v, p);
            self.s(0x38, v, (i as u8).wrapping_add(1));
            return;
        }
        if self.g(0x24, v) == 0 {
            return;
        }
        let speed = self.g(0x28, v) & 15;
        let delay = self.g(0x28, v) >> 4;
        let d0 = delay.wrapping_add(self.g(0x14, v));
        let f10 = self.g(0x10, v);
        if !l.flags.oldest {
            if (d0 as i8) <= (f10 as i8) {
                let d0 = d0.wrapping_add(speed);
                let gcmp = if l.flags.slide_cursor { self.g(0x0C, v) } else { f10 };
                if d0 <= gcmp {
                    let t = self.slide_tgt[v];
                    self.set_basper(v, t);
                } else {
                    self.slide_init(v, speed);
                    self.slide_step(v);
                }
            }
            if self.g(0x80, v) != 0 {
                self.vibrato(v);
            }
        } else if f10 >= d0 {
            let d0 = d0.wrapping_add(speed);
            if f10 < d0 {
                self.slide_init(v, speed);
                self.slide_step(v);
            }
        }
    }

    fn slide_init(&mut self, v: usize, speed: u8) {
        let cur = self.g(0x08, v);
        let tgt = self.g(0x24, v);
        let (pc, pt) = (self.p(cur), self.p(tgt));
        let mut delta;
        if !self.lay.flags.oldest {
            if cur < tgt {
                self.slide_dir[v] = true;
                delta = pc.wrapping_sub(pt);
            } else {
                self.slide_dir[v] = false;
                delta = pt.wrapping_sub(pc);
            }
            self.slide_tgt[v] = pt;
            let mut prod = 0u8;
            for _ in 0..=self.tr {
                prod = prod.wrapping_add(speed);
            }
            delta = delta.checked_div(prod as u16).unwrap_or(0);
        } else {
            if (tgt as i8) < (cur as i8) {
                self.slide_dir[v] = false;
                delta = pt.wrapping_sub(pc);
            } else {
                self.slide_dir[v] = true;
                delta = pc.wrapping_sub(pt);
            }
            self.slide_tgt[v] = pt;
            delta = delta.rotate_left(13);
        }
        self.slide_delta[v] = delta;
    }

    fn slide_step(&mut self, v: usize) {
        let b = self.basper(v);
        let d = self.slide_delta[v];
        let n = if self.slide_dir[v] { b.wrapping_sub(d) } else { b.wrapping_add(d) };
        self.set_basper(v, n);
        self.set_outper(v, n);
    }

    fn vibrato(&mut self, v: usize) {
        let par = self.g(0x68, v);
        let n = par & 15;
        let mx = par >> 4;
        let note = self.g(0x08, v);
        let mut delta = self.p(note).wrapping_sub(self.p1(note));
        for _ in 0..n {
            delta = (delta >> 1) | 1;
        }
        let up = |s: &mut Wb| {
            s.s(0x40, v, s.g(0x40, v).wrapping_add(1));
            let over = if s.lay.flags.oldest { !(s.g(0x40, v) < s.tc) } else { s.g(0x40, v) > mx };
            if over {
                s.s(0x3C, v, s.g(0x3C, v).wrapping_sub(1));
            }
        };
        if (self.g(0x3C, v) as i8) < 0 {
            self.s(0x40, v, self.g(0x40, v).wrapping_sub(1));
            if self.g(0x40, v) == 0 {
                self.s(0x3C, v, self.g(0x3C, v).wrapping_add(1));
                if (self.g(0x3C, v) as i8) < 0 {
                    up(self);
                }
            }
        } else {
            up(self);
        }
        if self.g(0x48, v) != 0 {
            self.s(0x48, v, self.g(0x48, v) - 1);
            return;
        }
        let mut p = self.basper(v);
        for _ in 0..mx >> 1 {
            p = p.wrapping_add(delta);
        }
        for _ in 0..self.g(0x40, v) {
            p = p.wrapping_sub(delta);
        }
        self.set_outper(v, p);
    }

    // ---- volume ----

    fn release(&mut self, v: usize) {
        self.s(0x6C, v, 1);
        let x = self.g(0x60, v).wrapping_sub(self.g(0x5C, v));
        if (x as i8) < 0 || (self.lay.flags.relz && x == 0) {
            self.s(0x60, v, 0);
            self.dm |= 1 << v;
        } else {
            self.s(0x60, v, x);
        }
    }

    fn volume(&mut self, v: usize) {
        if self.g(0x14, v) == 0 {
            return;
        }
        if self.g(0x6C, v) != 0 {
            self.release(v);
            return;
        }
        let d4 = self.g(0x70, v);
        if d4 == 0 {
            self.s(0x6C, v, 1);
            return;
        }
        if d4 == 0xFF {
            return;
        }
        let f10 = self.g(0x10, v);
        let hit = if !self.lay.flags.sdiv {
            let sh = d4 & 63;
            let q = if sh >= 8 { 0 } else { f10 >> sh };
            q == self.g(0x14, v)
        } else {
            if self.lay.flags.d5clean {
                self.d5 = 0;
            }
            self.d5 = (self.d5 & 0xFFFF_FF00) | f10 as u32;
            let q = self.d5 / d4 as u32;
            let r = self.d5 % d4 as u32;
            if q <= 0xFFFF {
                self.d5 = (r << 16) | q;
            }
            self.g(0x14, v) == (self.d5 & 0xFF) as u8
        };
        if hit {
            self.release(v);
            return;
        }
        if self.g(0x60, v) != self.g(0x54, v) {
            self.s(0x60, v, self.g(0x60, v).wrapping_add(self.g(0x58, v)));
        }
    }

    // ---- the tick ----

    fn play(&mut self, hw: &mut Hw) {
        if !self.enable {
            return;
        }
        self.d5 = 0;
        self.evented = false;
        for v in (0..4).rev() {
            self.sync_vars(v, self.bytes as usize);
            if self.tc != 0 {
                if self.lay.flags.oldest && self.g(0x34, v) != 0 {
                    self.s(0x34, v, 0);
                    self.s(0x08, v, self.g(0x08, v).wrapping_sub(1));
                    self.setperiod(v);
                    self.volume(v);
                    continue;
                }
                self.effects(v);
            } else {
                self.s(0x14, v, self.g(0x14, v).wrapping_sub(1));
                let mut skip = false;
                if self.g(0x70, v) == 0 && self.g(0x14, v) == 0 {
                    self.dm |= 1 << v;
                    skip = true;
                }
                if !skip && (self.g(0x14, v) as i8) < 0 {
                    if !self.new_event(v, hw) {
                        return;
                    }
                    self.volume(v);
                    continue;
                }
                if self.g(0x2C, v) != 0 {
                    self.s(0x08, v, self.g(0x08, v).wrapping_sub(1));
                    self.setperiod(v);
                    self.volume(v);
                    continue;
                }
                if self.g(0x30, v) != 0 {
                    self.s(0x08, v, self.g(0x08, v).wrapping_add(1));
                    self.setperiod(v);
                    self.volume(v);
                    continue;
                }
                self.effects(v);
            }
            self.volume(v);
            hw.wait(WAIT_VOICE);
        }
        self.hardware(hw);
        self.tc = if self.tc == 0 { self.tr } else { self.tc - 1 };
    }

    fn hardware(&mut self, hw: &mut Hw) {
        hw.wait(if self.evented { WAIT_HARDWARE_AFTER_NOTE } else { WAIT_HARDWARE });
        for v in 0..4 {
            if v > 0 {
                hw.wait(WAIT_PER);
            }
            hw.per(v, self.outper(v));
        }
        let on = !self.dm & 0x0F;
        wait_lines(hw, LINES_DMA, PHASE_DMA);
        hw.dmacon(0x8200 | on as u16);
        hw.wait(WAIT_DMA_OFF);
        hw.dmacon((0x8200 | on as u16) ^ 0x820F);
        wait_lines(hw, LINES_VOL, PHASE_VOL);
        for v in 0..4 {
            if v > 0 {
                hw.wait(WAIT_VOL_NEXT);
            }
            hw.vol(v, self.g(0x60, v) as u16);
        }
        hw.wait(WAIT_LEN);
        for v in (0..4).rev() {
            if v < 3 {
                hw.wait(WAIT_LEN_NEXT);
            }
            hw.len(v, self.loop_len[v] >> 1);
            hw.wait(WAIT_LOC);
            let restart = if self.lay.flags.newl && self.lay.empty.is_some() && self.g(0x78, v) == 0 {
                let t = self.lay.empty.unwrap_or(0);
                self.rl(t).wrapping_sub(self.lay.origin)
            } else {
                self.loop_lc[v].wrapping_add(self.loop_off[v] as u32)
            };
            hw.loc(v, self.base.wrapping_add(restart));
        }
    }
}

fn put16(d: &mut [u8], o: usize, v: u16) {
    if o + 2 <= d.len() {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    if o + 4 <= d.len() {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
}

/// Colour clocks from the song's start to the host's timer start, beyond the first period (measured
/// in the reference runs).
const TIMER_DELAY: u64 = 751;

// Colour clocks between the writes of a tick, measured in the reference runs: the tick's start to
// its first period, between periods, the last period to DMACON, DMACON to the DMA-off one, to the
// first volume, between volumes, the last volume to the first length, length to address, and an
// address to the next length. The voices' own processing takes no more time before the writes.
const WAIT_VOICE: u64 = 0;
// A note start: DMA off to the length, the length to the address, the address to the next voice's
// first write.
const WAIT_OFF_LEN: u64 = 363;
const WAIT_LEN_ADDR: u64 = 58;
const WAIT_NOTE: u64 = 1074;
const WAIT_HARDWARE: u64 = 873;
const WAIT_HARDWARE_AFTER_NOTE: u64 = 729;
const WAIT_PER: u64 = 8;
const WAIT_DMA_OFF: u64 = 12;
const WAIT_VOL_NEXT: u64 = 82;
const WAIT_LEN: u64 = 193;
const WAIT_LOC: u64 = 67;
const WAIT_LEN_NEXT: u64 = 54;
// The original waits for the beam, a number of raster line changes, before the DMA enable and
// the volumes: it goes on that many colour clocks into the line.
const LINES_DMA: u64 = 9;
const PHASE_DMA: u64 = 49;
const LINES_VOL: u64 = 9;
const PHASE_VOL: u64 = 172;

/// Lets `n` raster line changes pass, then `phase` colour clocks of the line.
fn wait_lines(hw: &mut Hw, n: u64, phase: u64) {
    let now = hw.bus.clock;
    let boundary = (now / LINE + 1) * LINE;
    hw.wait(boundary + (n - 1) * LINE + phase - now);
}

impl Replayer for Wb {
    fn format(&self) -> &'static str {
        "Wally Beben"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn timer_delay(&self) -> u64 {
        TIMER_DELAY
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsongs - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.sub = n;
        self.started = false;
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.mem.copy_from_slice(&self.pristine);
            self.tr = 0;
            self.d5 = 0;
            self.slide_dir = [false; 4];
            self.slide_delta = [0; 4];
            self.slide_tgt = [0; 4];
            self.loop_lc = [0; 4];
            self.loop_off = [0; 4];
            self.loop_len = [0; 4];
            self.init(self.sub + 1, hw);
            return;
        }
        self.play(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = locate(d)?;
    let title = m.bus.read_cstr(m.deli.file_buf);
    // the original's init routine runs before the host starts its timer (about eight frames; measured
    // in the reference runs)
    m.bus.clock += 662_627;
    Some(Box::new(Wb::new(d, base, lay, title)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    const ORIGIN: u32 = 0x1000;

    fn at(a: usize) -> u32 {
        ORIGIN + a as u32
    }

    fn pk(d: &mut [u8], o: usize, bytes: &[u8]) {
        d[o..o + bytes.len()].copy_from_slice(bytes);
    }

    fn pl(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// A module image in the old layout with the standard engine: one subsong whose
    /// four voices all play `pattern` (a byte string), one sample of 16 bytes and one
    /// instrument of constant volume 40. The period of note n is 1000 - 10 n.
    fn module(pattern: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x900];
        // jump table: BRA.W to the stop routine at 0x20, the play routine's entry
        pk(&mut d, 0, &[0x60, 0x00, 0x00, 0x1E]);
        pk(&mut d, 4, &[0x48, 0xE7, 0xFF, 0xFE, 0x61, 0x00, 0x00, 0x36]);
        pk(&mut d, 0x10, &[0x4E, 0x75, 0xFF]);
        pk(&mut d, 0x14, &[0x61, 0x00]);
        // stop routine: BSR.W, CLR.B, CLR.B, RTS
        pk(&mut d, 0x20, &[0x61, 0x00, 0x00, 0x00, 0x42, 0x39, 0, 0, 0x10, 0x12, 0x42, 0x39, 0, 0, 0x10, 0x12, 0x4E, 0x75]);
        // the engine: the request byte's address, the subsong count, the tables
        pk(&mut d, 0x44, &[0x10, 0x39]);
        pl(&mut d, 0x46, at(0x12));
        pk(&mut d, 0x4A, &[0x22, 0x3C, 0, 0, 0, 2]);
        pk(&mut d, 0x50, &[0x41, 0xF9]);
        pl(&mut d, 0x52, at(0x500));
        pk(&mut d, 0x56, &[0x41, 0xF9]);
        pl(&mut d, 0x58, at(0x400));
        pk(&mut d, 0x5C, &[0xD1, 0xC1]);
        pk(&mut d, 0x60, &[0xE5, 0x80, 0x43, 0xF9]);
        pl(&mut d, 0x64, at(0x540));
        pk(&mut d, 0x68, &[0x10, 0x28, 0x00, 0x4C, 0x43, 0xF9]);
        pl(&mut d, 0x6E, at(0x600));
        pk(&mut d, 0x72, &[0x43, 0xF9]);
        pl(&mut d, 0x74, at(0x640));
        pk(&mut d, 0x78, &[0xE5, 0x84]);
        pk(&mut d, 0x7C, &[0x10, 0x28, 0x00, 0x50, 0x45, 0xF9]);
        pl(&mut d, 0x82, at(0x660));
        pk(&mut d, 0x86, &[0x43, 0xF9]);
        pl(&mut d, 0x88, at(0x700));
        pk(&mut d, 0x8C, &[0xD3, 0xC5, 0x10, 0x11, 0x43, 0xF9]);
        pl(&mut d, 0x92, at(0x680));
        pk(&mut d, 0x96, &[0xD3, 0xC4, 0x12, 0x80]);
        pk(&mut d, 0x9C, &[0x10, 0x28, 0x00, 0x08, 0xE3, 0x00, 0x43, 0xF9]);
        pl(&mut d, 0xA4, at(0x300));
        pk(&mut d, 0xA8, &[0x43, 0xF9]);
        pl(&mut d, 0xAA, at(0x3A0));
        for n in 0..128usize {
            d[0x300 + 2 * n..0x302 + 2 * n].copy_from_slice(&(1000 - 10 * n as u16).to_be_bytes());
        }
        // voice block: channel offsets, arpeggio bases, DMA bits
        for v in 0..4 {
            d[0x400 + v] = 16 * v as u8;
            d[0x404 + v] = 10 * v as u8;
            d[0x464 + v] = 1 << v;
        }
        // every voice plays order list 0 (pattern 0, then restart)
        for v in 0..4 {
            pl(&mut d, 0x500 + 4 * v, at(0x520));
        }
        pk(&mut d, 0x520, &[0, 0xFF]);
        pl(&mut d, 0x540, at(0x560));
        pk(&mut d, 0x560, pattern);
        // sample 0: 16 bytes, not looped, at 0x800; instrument 0: volume 40, constant
        pk(&mut d, 0x600, &[0, 0, 0, 16, 0, 0, 0, 0]);
        pl(&mut d, 0x640, at(0x800));
        pk(&mut d, 0x660, &[40, 40, 0, 0, 0xFF, 0, 0, 0]);
        pk(&mut d, 0x700, &[3, 4, 5, 7, 8, 9, 10, 12]);
        d
    }

    fn run(pattern: &[u8], ticks: usize) -> (Vec<(u32, u16)>, bool) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/wb-test/wb.test"), module(pattern)).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Wally Beben");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        (log, e.songend())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_the_image_structure() {
        let d = module(&[0xFE]);
        assert!(detect(&d));
        let lay = locate(&d).unwrap();
        assert_eq!((lay.origin, lay.imm, lay.songs, lay.v), (ORIGIN, 2, 0x500, 0x400));
        assert_eq!((lay.periods, lay.pc, lay.instr, lay.arpbuf), (0x300, 0x3A0, 0x660, 0x680));
        assert!(!lay.flags.newl && !lay.flags.oldest && !lay.flags.sdiv);
        // a stop routine without its clears, a play routine that does not save the
        // registers, a short file, plain data
        let mut e = d.clone();
        e[0x30] = 0x4E;
        e[0x31] = 0x71;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[4] = 0;
        assert!(!detect(&e));
        assert!(!detect(&d[..0x30]));
        assert!(!detect(&[0u8; 0x900]));
        assert!(!detect(&(0..0x900).map(|i| (i * 7) as u8).collect::<Vec<_>>()));
    }

    #[test]
    fn plays_a_note_with_its_sample_period_and_volume() {
        // sample 0, instrument 0, duration 3, note 24
        let (log, _) = run(&[0xE0, 0xC0, 0x83, 24, 0xFE], 6);
        assert_eq!(reg(&log, 0xDFF0D4)[0], 8);
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [1000 - 240, 1000 - 240]);
        assert!(reg(&log, 0xDFF0A8).contains(&40));
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0008) && dma.contains(&0x0001) && dma.contains(&0x820F));
        // the sample start and the restart address of a non-looped sample agree
        let lc = reg(&log, 0xDFF0A2);
        assert!(lc.len() >= 2 && lc.iter().all(|&l| l == lc[0]));
    }

    #[test]
    fn tempo_prefix_spaces_the_music_ticks() {
        // tempo reload 1: a music tick every second call; the note lasts 2 music ticks,
        // so it starts at tick 1 and the stop event is met at tick 5
        let (log, end) = run(&[0xF1, 0xE0, 0xC0, 0x81, 24, 0xFE], 8);
        assert_eq!(reg(&log, 0xDFF0A6).len(), 4, "hardware stage in ticks 1..4 only");
        assert!(end);
    }

    #[test]
    fn stop_event_silences_and_ends_the_song() {
        let (log, end) = run(&[0xE0, 0xC0, 0x80, 24, 0xFE], 10);
        assert!(end);
        assert_eq!(*reg(&log, 0xDFF0A8).last().unwrap(), 0);
        let n = reg(&log, 0xDFF0A6).len();
        let (log2, _) = run(&[0xE0, 0xC0, 0x80, 24, 0xFE], 20);
        assert_eq!(reg(&log2, 0xDFF0A6).len(), n, "nothing is written after the stop");
    }

    #[test]
    fn pattern_restart_reports_the_song_end_once_all_voices_wrapped() {
        let (log, end) = run(&[0xE0, 0xC0, 0x80, 24, 0xFF], 8);
        assert!(reg(&log, 0xDFF0A6).iter().all(|&p| p == 1000 - 240));
        assert!(end);
    }

    #[test]
    fn order_list_transposition_and_the_period_index_wrap() {
        let mut d = module(&[0xE0, 0xC0, 0x80, 24, 0xFE]);
        // order list: transposition +3 (0xE3), then pattern 0
        pk(&mut d, 0x520, &[0xE3, 0, 0xFF]);
        let lay = locate(&d).unwrap();
        let mut w = Wb::new(&d, 0x20000, lay, String::new());
        w.ol[0] = 0x520;
        w.load_pattern(0);
        assert_eq!(w.g(0x20, 0), 3);
        assert_eq!(w.pp[0], 0x560);
        // the period index is a byte offset: note 130 wraps to note 2
        assert_eq!(w.p(130), w.p(2));
    }

    #[test]
    fn arpeggio_list_holds_the_masked_intervals() {
        let d = module(&[0xE0, 0xC0, 0x80, 24, 0xFE]);
        let lay = locate(&d).unwrap();
        let mut w = Wb::new(&d, 0x20000, lay, String::new());
        // mask 0b101: intervals 3 and 5, then the terminator, after voice 1's base byte
        w.build_arp(1, 0b101);
        let b = w.lay.arpbuf + 10;
        assert_eq!(&w.mem[b + 1..b + 4], &[3, 5, 0xFF]);
    }
}
