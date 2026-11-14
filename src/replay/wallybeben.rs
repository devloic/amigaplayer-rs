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
use std::collections::HashMap;

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
    /// The tick's start (colour clocks) and the 68000 cycles its code has taken so far.
    t0: u64,
    cy: i64,
    /// The voice being processed, for the per-voice paths' costs.
    cv: usize,
    /// Cycles per code path (`COSTS`, with its build's `OLD` and `BUILDS` deltas).
    costs: HashMap<String, i64>,
}

impl Wb {
    fn new(d: &[u8], base: u32, lay: Lay, title: String) -> Wb {
        let costs = costs(lay.flags);
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
            t0: 0,
            cy: 0,
            cv: 0,
            costs,
        }
    }

    /// Charges the 68000 cycles of the code path `l` (of the current voice for the per-voice paths).
    fn k(&mut self, l: &str) {
        let per_voice = VOICE_PATHS.iter().any(|p| l.strip_prefix(p).is_some_and(|r| r.is_empty() || r.starts_with('.')));
        let c = if per_voice { self.costs.get(&format!("{l}.{}", self.cv)) } else { self.costs.get(l) };
        self.cy += c.copied().unwrap_or(0);
    }

    fn at(&mut self, hw: &mut Hw) {
        let t = self.t0 + self.cy.max(0) as u64 / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
    }

    /// The original's wait for `n` changes of the beam's line: a read, a read `p.f4` cycles later,
    /// then every `p.loop` cycles until the line differs, `p.next` to the next change's first read.
    fn poll(&mut self, n: u32) {
        let (f4, lp, nx) = (POLL_FIRST, POLL_LOOP, POLL_NEXT);
        let line = |s: &Self, c: i64| (s.t0 + c.max(0) as u64 / 2) / LINE;
        for i in 0..n {
            let l = line(self, self.cy);
            self.cy += f4;
            while line(self, self.cy) == l {
                self.cy += lp;
            }
            if i + 1 < n {
                self.cy += nx;
            }
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
            self.k(["sil0", "sil1", "sil2", "sil3"][v]);
            self.at(hw);
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
        self.k("init");
        if l.flags.init_dma {
            self.k("init.dma");
            self.at(hw);
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
        self.k("lp");
        for _ in 0..GUARD {
            self.k("lp.rd");
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
                self.k("lp.pat");
                self.pp[v] = self.rl(self.lay.patterns + i).wrapping_sub(self.lay.origin) as usize;
                self.loaded = Some(self.lay.patterns + i);
                return;
            }
            if x == 0xFF {
                self.k("lp.ff");
                self.s(0x0C, v, 0);
                self.wrap(v);
            } else if oldest {
                self.k("lp.tr");
                self.s(0x20, v, x.wrapping_add(0x40));
            } else {
                self.k("lp.tr");
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
        self.k("ba");
        let mut i = self.g(0x04, v) as usize + 1;
        for k in 0..8 {
            self.k(if mask & (1 << k) != 0 { "ba.1" } else { "ba.0" });
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
            self.k("dc");
            let mut x = rd!();
            if x == 0xFF {
                self.k("dc.ff");
                self.load_pattern(v);
                cnt = 0;
                continue 'event;
            }
            if x == 0xFE {
                self.k("dc.fe");
                return Ev::Stop;
            }
            if x == 0xFC && !fl.oldest {
                {
                    self.k("dc.fc");
                    let y = rd!();
                    return Ev::Jump(y);
                }
            }
            if x > 0xEF {
                self.k("dc.tr");
                self.tr = x & 0x0F;
                x = rd!();
            }
            // sample
            if fl.e0cmd {
                if x == 0xE0 {
                    self.k("dc.e0");
                    let n = rd!();
                    self.s(0x4C, v, n.wrapping_mul(8));
                    x = rd!();
                    if x == 0xFF {
                        self.k("dc.ff2");
                        self.load_pattern(v);
                        cnt = 0;
                        continue 'event;
                    }
                }
            } else if x > 0xDF {
                self.k("dc.smp");
                if x & 15 == 0 {
                    self.k("dc.s0");
                }
                self.s(0x4C, v, (x & 15) * 8);
                x = rd!();
                if x == 0xFF {
                    self.k("dc.ff2");
                    self.load_pattern(v);
                    cnt = 0;
                    continue 'event;
                }
            }
            // instrument
            if x > 0xBF {
                self.k("dc.ins");
                if x & 0x1F == 0 {
                    self.k("dc.i0");
                }
                self.s(0x50, v, (x & 0x1F) * 8);
                x = rd!();
                if x == 0xFF {
                    self.k("dc.ff3");
                    self.load_pattern(v);
                    cnt = 0;
                    continue 'event;
                }
            }
            // duration
            if x > 0x7F {
                self.k("dc.dur");
                self.s(0x10, v, x & 0x3F);
                x = rd!();
            }
            let lab = match x {
                0..=0x5F => "dc.n",
                0x7F => "dc.7f",
                0x7E => "dc.7e",
                0x7D | 0x7C => "dc.7d",
                0x7B => "dc.7b",
                0x7A => "dc.7a",
                _ => "dc.nx",
            };
            self.k(lab);
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
        self.k("ne");
        if self.g(0x18, v) != 2 {
            self.k("ne.clr");
            self.s(0x18, v, 0);
            self.s(0x1C, v, 0);
        }
        for f in [0x24, 0x2C, 0x30, 0x6C] {
            self.s(f, v, 0);
        }
        self.loaded = None;
        let (note, cnt) = match self.decode(v) {
            Ev::Stop => {
                self.k("ne.stop");
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
        if self.dm & bit != 0 {
            self.k("ne.own");
        }
        self.dm &= !bit;
        self.k("ne.off");
        self.at(hw);
        hw.dmacon(bit as u16);
        self.evented = true;
        self.d5 = bit as u32;
        self.sample_setup(v, hw);
        self.instrument_setup(v);
        let t = note.wrapping_add(self.g(0x20, v));
        self.s(0x08, v, t);
        self.k("ne.post");
        if note == 0 {
            self.k("ne.n0");
        }
        if self.lay.flags.oldest {
            self.k("ne.o");
            let flags = if self.g(0x1C, v) == 0 {
                self.rb(self.lay.instr + self.g(0x50, v) as usize + 7)
            } else {
                self.stale_flags(v)
            };
            if flags & 1 != 0 {
                self.k("ne.o1");
                self.s(0x34, v, t);
                self.s(0x08, v, t.wrapping_add(1));
            }
        }
        self.setperiod(v);
        self.k("ne.end");
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
        self.k(if !l.flags.oldest && flag != 0 { "ss.len.f" } else { "ss.len" });
        self.at(hw);
        hw.len(v, len >> 1);
        if flag != 0 {
            self.k("ss.flag");
        }
        if l.flags.sflag2 && flag & 2 != 0 {
            self.k("ss.f2");
            self.at(hw);
            let t0 = self.rl(l.stab).wrapping_add(4 * number as u32).wrapping_add(w4 as u32);
            put32(&mut self.mem, l.stab, t0);
            let a = t0.wrapping_sub(l.origin).wrapping_add(self.base);
            // the original stores the address through the LEN and PER registers
            hw.bus.write_word(0xDFF0A4 + 16 * v as u32, (a >> 16) as u16);
            hw.bus.write_word(0xDFF0A6 + 16 * v as u32, a as u16);
            self.k("ss.f2.end");
            self.loop_lc[v] = self.rl(l.stab + 4 * number).wrapping_sub(l.origin);
        } else {
            let start = self.rl(l.stab + 4 * number).wrapping_sub(l.origin);
            self.k("ss.loc");
            self.at(hw);
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
        self.k("is");
        if ins[..6].iter().all(|&x| x == 0) {
            self.k("is.z");
        }
        if ins[6] != 0 {
            self.k("is.arp");
            self.s(0x1C, v, ins[6]);
            self.s(0x18, v, if self.lay.flags.oldest { ins[6] } else { 0x7C });
            self.build_arp(v, ins[6]);
        }
        if ins[7] != 0 {
            self.k("is.7");
        }
        if ins[7] != 0 && ins[7] & 8 != 0 {
            self.k("is.27");
            self.s(0x27, v, 1);
        }
    }

    // ---- effects ----

    fn effects(&mut self, v: usize) {
        self.k("eff");
        if self.g(0x08, v) == 0 {
            self.k("eff.z");
            return;
        }
        let oldest = self.lay.flags.oldest;
        if self.g(0x18, v) == 0 {
            if self.g(0x68, v) != 0 {
                self.k("eff.vib");
                self.vibrato(v);
                return;
            }
            self.k("eff.18z");
            if !oldest {
                self.s(0x80, v, self.g(0x4E, v));
            }
        }
        self.arp_or_slide(v);
    }

    fn arp_or_slide(&mut self, v: usize) {
        let l = self.lay.clone();
        self.k("aos");
        if self.g(0x1C, v) != 0 {
            self.k("arp");
            let mut i = self.g(0x38, v) as usize;
            let mut x = self.rb(l.arpbuf + i);
            for _ in 0..2 {
                if x != 0xFF {
                    break;
                }
                self.k("arp.wrap");
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
            self.k("aos.none");
            return;
        }
        self.k("sl");
        let speed = self.g(0x28, v) & 15;
        let delay = self.g(0x28, v) >> 4;
        let d0 = delay.wrapping_add(self.g(0x14, v));
        let f10 = self.g(0x10, v);
        if !l.flags.oldest {
            if (d0 as i8) <= (f10 as i8) {
                self.k("sl.in");
                let d0 = d0.wrapping_add(speed);
                let gcmp = if l.flags.slide_cursor { self.g(0x0C, v) } else { f10 };
                if d0 <= gcmp {
                    self.k("sl.tgt");
                    let t = self.slide_tgt[v];
                    self.set_basper(v, t);
                } else {
                    self.k("sl.go");
                    self.slide_init(v, speed);
                    self.slide_step(v);
                }
            } else {
                self.k("sl.out");
            }
            if self.g(0x80, v) != 0 {
                self.k("sl.vib");
                self.vibrato(v);
            }
        } else if f10 >= d0 {
            self.k("slo.in");
            let d0 = d0.wrapping_add(speed);
            if f10 < d0 {
                self.k("slo.go");
                self.slide_init(v, speed);
                self.slide_step(v);
            }
        } else {
            self.k("slo.out");
        }
    }

    fn slide_init(&mut self, v: usize, speed: u8) {
        let cur = self.g(0x08, v);
        let tgt = self.g(0x24, v);
        let (pc, pt) = (self.p(cur), self.p(tgt));
        let mut delta;
        if !self.lay.flags.oldest {
            self.k("si");
            if cur < tgt {
                self.k("si.up");
                self.slide_dir[v] = true;
                delta = pc.wrapping_sub(pt);
            } else {
                self.slide_dir[v] = false;
                delta = pt.wrapping_sub(pc);
            }
            self.slide_tgt[v] = pt;
            let mut prod = 0u8;
            for _ in 0..=self.tr {
                self.k("si.mul");
                prod = prod.wrapping_add(speed);
            }
            delta = delta.checked_div(prod as u16).unwrap_or(0);
        } else {
            self.k("sio");
            if (tgt as i8) < (cur as i8) {
                self.k("sio.dn");
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
        self.k(if self.slide_dir[v] { "st.up" } else { "st.dn" });
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
        self.k("vib");
        let mut delta = self.p(note).wrapping_sub(self.p1(note));
        if n == 0 {
            self.k("vib.sh0");
        }
        for _ in 0..n {
            self.k("vib.sh");
            delta = (delta >> 1) | 1;
        }
        let up = |s: &mut Wb| {
            s.k("vib.up");
            s.s(0x40, v, s.g(0x40, v).wrapping_add(1));
            let over = if s.lay.flags.oldest { !(s.g(0x40, v) < s.tc) } else { s.g(0x40, v) > mx };
            if over {
                s.k("vib.over");
                s.s(0x3C, v, s.g(0x3C, v).wrapping_sub(1));
            }
        };
        if (self.g(0x3C, v) as i8) < 0 {
            self.k("vib.dn");
            self.s(0x40, v, self.g(0x40, v).wrapping_sub(1));
            if self.g(0x40, v) == 0 {
                self.k("vib.z");
                self.s(0x3C, v, self.g(0x3C, v).wrapping_add(1));
                if (self.g(0x3C, v) as i8) < 0 {
                    up(self);
                }
            }
        } else {
            up(self);
        }
        if self.g(0x48, v) != 0 {
            self.k("vib.delay");
            self.s(0x48, v, self.g(0x48, v) - 1);
            return;
        }
        self.k("vib.out");
        let mut p = self.basper(v);
        if mx >> 1 == 0 {
            self.k("vib.add0");
        }
        for _ in 0..mx >> 1 {
            self.k("vib.add");
            p = p.wrapping_add(delta);
        }
        if self.g(0x40, v) == 0 {
            self.k("vib.sub0");
        }
        for _ in 0..self.g(0x40, v) {
            self.k("vib.sub");
            p = p.wrapping_sub(delta);
        }
        self.set_outper(v, p);
    }

    // ---- volume ----

    fn release(&mut self, v: usize) {
        self.k("rel");
        self.s(0x6C, v, 1);
        let x = self.g(0x60, v).wrapping_sub(self.g(0x5C, v));
        if (x as i8) < 0 || (self.lay.flags.relz && x == 0) {
            self.k("rel.end");
            self.s(0x60, v, 0);
            self.dm |= 1 << v;
        } else {
            self.s(0x60, v, x);
        }
    }

    fn volume(&mut self, v: usize) {
        self.k("vol");
        if self.g(0x14, v) == 0 {
            self.k("vol.z");
            return;
        }
        if self.g(0x6C, v) != 0 {
            self.k(["vol.rel", "vol.rel1", "vol.rel2", "vol.rel3"][v]);
            self.release(v);
            return;
        }
        let d4 = self.g(0x70, v);
        if d4 == 0 {
            self.k("vol.d0");
            self.s(0x6C, v, 1);
            return;
        }
        if d4 == 0xFF {
            self.k("vol.ff");
            return;
        }
        let f10 = self.g(0x10, v);
        let hit = if !self.lay.flags.sdiv {
            let sh = d4 & 63;
            self.k("vol.sh");
            if sh >= 8 {
                self.k("vol.sh8");
            }
            let q = if sh >= 8 { 0 } else { f10 >> sh };
            q == self.g(0x14, v)
        } else {
            if self.lay.flags.d5clean {
                self.d5 = 0;
            }
            self.d5 = (self.d5 & 0xFFFF_FF00) | f10 as u32;
            self.k("vol.div");
            let q = self.d5 / d4 as u32;
            let r = self.d5 % d4 as u32;
            if q <= 0xFFFF {
                self.d5 = (r << 16) | q;
            } else {
                self.k("vol.ovf");
            }
            self.g(0x14, v) == (self.d5 & 0xFF) as u8
        };
        if hit {
            self.k(["vol.hit", "vol.hit1", "vol.hit2", "vol.hit3"][v]);
            self.release(v);
            return;
        }
        self.k("vol.att");
        if self.g(0x60, v) != self.g(0x54, v) {
            self.k("vol.add");
            self.s(0x60, v, self.g(0x60, v).wrapping_add(self.g(0x58, v)));
        }
    }

    // ---- the tick ----

    fn play(&mut self, hw: &mut Hw) {
        self.k("tick");
        if !self.enable {
            self.k("noenable");
            return;
        }
        self.d5 = 0;
        self.evented = false;
        for v in (0..4).rev() {
            self.cv = v;
            self.k("v");
            if self.dm & (1 << v) != 0 {
                self.k("v.dmb");
            }
            self.sync_vars(v, self.bytes as usize);
            if self.tc != 0 {
                self.k("v.tc");
                if self.lay.flags.oldest && self.g(0x34, v) != 0 {
                    self.k("v.o34");
                    self.s(0x34, v, 0);
                    self.s(0x08, v, self.g(0x08, v).wrapping_sub(1));
                    self.setperiod(v);
                    self.volume(v);
                    self.k("v.o34.end");
                    continue;
                }
                self.effects(v);
            } else {
                self.k("v.row");
                self.s(0x14, v, self.g(0x14, v).wrapping_sub(1));
                let mut skip = false;
                if self.g(0x70, v) == 0 && self.g(0x14, v) == 0 {
                    self.k("v.dm");
                    self.dm |= 1 << v;
                    skip = true;
                }
                if !skip && (self.g(0x14, v) as i8) < 0 {
                    if !self.new_event(v, hw) {
                        return;
                    }
                    self.volume(v);
                    self.k("v.ev.end");
                    continue;
                }
                if self.g(0x2C, v) != 0 {
                    self.k("v.2c");
                    self.s(0x08, v, self.g(0x08, v).wrapping_sub(1));
                    self.setperiod(v);
                    self.volume(v);
                    self.k("v.2c.end");
                    continue;
                }
                if self.g(0x30, v) != 0 {
                    self.k("v.30");
                    self.s(0x08, v, self.g(0x08, v).wrapping_add(1));
                    self.setperiod(v);
                    self.volume(v);
                    self.k("v.30.end");
                    continue;
                }
                self.k("v.row.eff");
                self.effects(v);
            }
            self.volume(v);
            self.k("v.end");
        }
        self.hardware(hw);
        self.tc = if self.tc == 0 { self.tr } else { self.tc - 1 };
    }

    fn hardware(&mut self, hw: &mut Hw) {
        self.k("hw");
        for v in 0..4 {
            self.k(if v == 0 { "hw.per0" } else { "hw.per" });
            self.at(hw);
            hw.per(v, self.outper(v));
        }
        let on = !self.dm & 0x0F;
        self.k("w1");
        self.poll(LINES_DMA as u32);
        self.k("w1.dma");
        self.at(hw);
        hw.dmacon(0x8200 | on as u16);
        self.k("hw.off");
        self.at(hw);
        hw.dmacon((0x8200 | on as u16) ^ 0x820F);
        self.k("w2");
        self.poll(LINES_VOL as u32);
        for v in 0..4 {
            self.cv = v;
            self.k(if v == 0 { "w2.vol" } else { "hw.vol" });
            self.at(hw);
            hw.vol(v, self.g(0x60, v) as u16);
        }
        for v in (0..4).rev() {
            self.cv = v;
            self.k(if v == 3 { "hw.len0" } else { "hw.len" });
            self.at(hw);
            hw.len(v, self.loop_len[v] >> 1);
            self.k("hw.loc");
            let restart = if self.lay.flags.newl && self.lay.empty.is_some() && self.g(0x78, v) == 0 {
                self.k("hw.empty");
                let t = self.lay.empty.unwrap_or(0);
                self.rl(t).wrapping_sub(self.lay.origin)
            } else {
                self.loop_lc[v].wrapping_add(self.loop_off[v] as u32)
            };
            self.at(hw);
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

// The original waits for the beam before the DMA enable and before the volumes, LINES_* changes of
// its line each: it reads the line, again POLL_FIRST cycles later and then every POLL_LOOP cycles
// until it differs, and reads it for the next change POLL_NEXT cycles after (68000 cycles, measured
// in the reference runs: the beam reads of its wait loop).
const LINES_DMA: u64 = 9;
const LINES_VOL: u64 = 9;
const POLL_FIRST: i64 = 16;
const POLL_LOOP: i64 = 25;
const POLL_NEXT: i64 = 35;
/// The code paths whose cost depends on the voice: their labels get the voice's number.
const VOICE_PATHS: &[&str] = &["rel", "eff", "vol", "aos", "v", "hw.vol", "hw.len", "hw.loc"];

impl Replayer for Wb {
    fn format(&self) -> &'static str {
        "Wally Beben"
    }

    fn title(&self) -> String {
        self.title.clone()
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
        self.t0 = hw.bus.clock;
        self.cy = 0;
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
    // the original's init routine runs before the host starts its timer (about eight frames)
    m.bus.clock += start(d, &lay);
    Some(Box::new(Wb::new(d, base, lay, title)))
}

// The 68000 cycles of each code path the port charges (`Wb::k`): fitted exactly to the reference runs'
// write times (each write at its tick's start plus half the cycles its code took, rounded down),
// the per-voice paths with the voice's number. The builds differ in some paths (`OLD`, `BUILDS`).
const COSTS: &[(&str, i16)] = &[
    ("aos.0", 88), ("aos.1", 78), ("aos.2", 88), ("aos.3", 105), ("aos.none.0", 12), ("aos.none.1", 12),
    ("aos.none.2", 0), ("aos.none.3", 12), ("arp", 407), ("arp.wrap", 121), ("ba", 157), ("ba.0", 157),
    ("ba.1", 157), ("dc", 0), ("dc.7a", 136), ("dc.dur", 32), ("dc.e0", 63), ("dc.ff", -136),
    ("dc.ff3", -29), ("dc.i0", 0), ("dc.ins", 63), ("dc.n", 0), ("dc.s0", 0), ("dc.smp", 63), ("dc.tr", 36),
    ("eff.0", 0), ("eff.1", 0), ("eff.18z.0", 0), ("eff.18z.1", 0), ("eff.18z.2", 0), ("eff.18z.3", 0),
    ("eff.2", 0), ("eff.3", 0), ("eff.vib.0", 0), ("eff.vib.1", 0), ("eff.vib.2", 0), ("eff.vib.3", -2),
    ("eff.z.0", 0), ("eff.z.1", 22), ("eff.z.2", -11), ("eff.z.3", 0), ("hw", 760), ("hw.empty", 6),
    ("hw.len.0", 108), ("hw.len.1", 108), ("hw.len.2", 108), ("hw.len0", 386), ("hw.loc.0", 133),
    ("hw.loc.1", 133), ("hw.loc.2", 133), ("hw.loc.3", 133), ("hw.off", 24), ("hw.per", 16), ("hw.per0", 0),
    ("hw.vol.1", 168), ("hw.vol.2", 158), ("hw.vol.3", 168), ("init", 0), ("is", 0), ("is.arp", -2),
    ("is.z", -106), ("lp", 0), ("lp.ff", 0), ("lp.pat", 275), ("lp.rd", 350), ("lp.tr", -117), ("ne", 0),
    ("ne.clr", 0), ("ne.end", 0), ("ne.n0", 12), ("ne.o", -261), ("ne.off", 1231), ("ne.own", 33),
    ("ne.post", 311), ("rel.0", 32), ("rel.1", 32), ("rel.2", 32), ("rel.3", 32), ("rel.end.1", 8),
    ("rel.end.2", 3), ("rel.end.3", 3), ("sil0", 0), ("sil1", 32), ("sil2", 12), ("sil3", 77),
    ("ss.f2", 135), ("ss.f2.end", -177), ("ss.flag", 0), ("ss.len", 727), ("ss.len.f", 729),
    ("ss.loc", 115), ("tick", 0), ("v.0", 0), ("v.1", 0), ("v.2", 0), ("v.3", 0), ("v.dm.0", 14),
    ("v.dm.1", 0), ("v.dm.2", 13), ("v.dm.3", 13), ("v.dmb.0", 0), ("v.dmb.1", 0), ("v.dmb.2", 0),
    ("v.dmb.3", 0), ("v.end.0", 0), ("v.end.1", 2), ("v.end.2", 0), ("v.end.3", 0), ("v.ev.end.0", 0),
    ("v.ev.end.1", 0), ("v.ev.end.2", 0), ("v.ev.end.3", 0), ("v.row.0", 79), ("v.row.1", 79),
    ("v.row.2", 79), ("v.row.3", 79), ("v.row.eff.0", 17), ("v.row.eff.1", 25), ("v.row.eff.2", 29),
    ("v.row.eff.3", 0), ("v.tc.0", 0), ("v.tc.1", 0), ("v.tc.2", 0), ("v.tc.3", 0), ("vib", 0),
    ("vib.add", 77), ("vib.add0", 203), ("vib.delay", 40), ("vib.dn", 0), ("vib.out", 0), ("vib.over", 16),
    ("vib.sh", 304), ("vib.sub", 81), ("vib.sub0", 0), ("vib.up", 32), ("vib.z", 27), ("vol.0", 393),
    ("vol.1", 393), ("vol.2", 393), ("vol.3", 393), ("vol.add.0", 34), ("vol.add.1", 34), ("vol.add.2", 34),
    ("vol.add.3", 34), ("vol.att.0", 76), ("vol.att.1", 76), ("vol.att.2", 76), ("vol.att.3", 76),
    ("vol.d0.0", 0), ("vol.d0.1", 1), ("vol.d0.2", 0), ("vol.d0.3", -1), ("vol.div.0", 120),
    ("vol.div.1", 120), ("vol.div.2", 120), ("vol.div.3", 120), ("vol.ff.0", -6), ("vol.ff.1", 0),
    ("vol.ff.2", 2), ("vol.ff.3", 0), ("vol.hit.0", 101), ("vol.hit1.1", 101), ("vol.hit2.2", 101),
    ("vol.hit3.3", 101), ("vol.rel.0", 0), ("vol.rel1.1", 0), ("vol.rel2.2", 0), ("vol.rel3.3", 0),
    ("vol.sh.0", 2), ("vol.sh.1", 2), ("vol.sh.2", 2), ("vol.sh.3", 0), ("vol.z.0", -87), ("vol.z.1", -87),
    ("vol.z.2", -87), ("vol.z.3", -87), ("w1", 118), ("w1.dma", 79), ("w2", 62), ("w2.vol", 333),
];

/// The builds whose costs differ from `COSTS`, told apart by their image's `Flags`. Each table holds the
/// paths every reference file it covers needs a delta for, at their mean: `OLD` for the old layout
/// (Circus Games, Elite, Wicked, Phantasm), `MID` on top for its builds other than the oldest (Circus
/// Games, Elite, Wicked), then each build's own (`BUILDS`). A delta only some files of a build need is
/// left out, and so are the files' differences from the mean: those files are SIMILAR, not IDENTICAL.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Build {
    /// The new layout without the 0xE0 sample command or sample flag bit 1 (Colossus Chess X,
    /// Futuresport, Hawk Eye, SDI, Pool of Radiance, Total Eclipse).
    New,
    /// The new layout with sample flag bit 1 (Lancaster, Mr. Do! Run Run).
    NewFlag2,
    /// The new layout with the 0xE0 sample command (The Ball Game, Hammerfist).
    NewE0,
    /// The old layout without the build options (Circus Games, Wicked).
    Plain,
    /// The old layout comparing the slide window against the order cursor (Elite).
    Cursor,
    /// The older engine (Phantasm, Foundation's Waste).
    Oldest,
    /// Any other old-layout build (Dark Side, Hyperdome, Superman): no reference measured, `OLD` only.
    Other,
}

fn build(f: Flags) -> Build {
    match f {
        _ if f.newl && f.e0cmd => Build::NewE0,
        _ if f.newl && f.sflag2 => Build::NewFlag2,
        _ if f.newl => Build::New,
        _ if f.oldest => Build::Oldest,
        _ if f.slide_cursor => Build::Cursor,
        _ if !f.sdiv && !f.init_dma => Build::Plain,
        _ => Build::Other,
    }
}

const OLD: &[(&str, i16)] = &[
    ("dc.ff", 2), ("hw.len.0", 16), ("hw.len.1", 16), ("hw.len.2", 16), ("hw.len0", -56), ("hw.loc.0", 13),
    ("hw.loc.1", 13), ("hw.loc.2", 13), ("hw.loc.3", 13), ("hw.off", 80), ("sil0", -1), ("ss.loc", 179),
    ("w1", 4), ("w1.dma", 8), ("w2.vol", -20),
];

const MID: &[(&str, i16)] = &[
    ("dc.ff", 1), ("hw.len.0", -6), ("hw.len.1", -6), ("hw.len.2", -6), ("hw.len0", -2), ("ne.post", -215),
    ("sil0", -1), ("ss.len.f", 8), ("v.dm.2", -3), ("vol.z.3", 10),
];

const BUILDS: &[(Build, &[(&str, i16)])] = &[
    (Build::New, &[
        ("ss.loc", -27),
    ]),
    (Build::NewFlag2, &[
        ("dc.ff", -33), ("dc.ins", 7), ("ne.off", 1), ("ne.own", -13), ("rel.end.3", 5), ("v.dm.3", 16),
        ("v.dmb.0", -20), ("v.dmb.1", -20), ("v.dmb.2", 11), ("v.dmb.3", 6), ("v.end.2", 4), ("v.row.2", -4),
        ("v.row.eff.3", -3), ("vol.0", -25), ("vol.2", 29), ("vol.z.1", -17),
    ]),
    (Build::NewE0, &[
        ("eff.18z.3", 8), ("eff.z.2", -29), ("eff.z.3", -3), ("ne.own", -16), ("v.dm.2", 17), ("v.dm.3", -16),
        ("v.dmb.0", 16), ("v.dmb.2", 16), ("v.dmb.3", 16), ("v.row.3", 1), ("v.row.eff.3", 25),
        ("vol.d0.2", 13), ("vol.d0.3", -2), ("vol.rel2.2", 33),
    ]),
    (Build::Plain, &[
        ("ne.off", -24), ("ne.post", 4), ("v.dm.1", 14), ("v.dm.2", 8), ("v.dm.3", 6), ("vol.att.2", 20),
        ("vol.z.3", 7),
    ]),
    (Build::Cursor, &[
        ("eff.18z.0", -4), ("eff.18z.1", -4), ("eff.18z.2", -5), ("eff.z.3", 6), ("lp.rd", -1),
        ("ne.own", 17), ("ne.post", -9), ("rel.0", -11), ("rel.1", -11), ("rel.2", -11), ("ss.len", -12),
        ("v.3", -17), ("v.dm.2", -16), ("v.dmb.0", -17), ("v.dmb.1", -17), ("v.dmb.2", -17), ("v.dmb.3", -17),
        ("v.row.eff.3", 13), ("v.tc.3", -12), ("vol.0", -2), ("vol.1", -2), ("vol.2", -1), ("vol.d0.0", 1),
        ("vol.hit3.3", -27), ("vol.rel3.3", 2), ("vol.sh.2", -1), ("vol.z.3", -13),
    ]),
    (Build::Oldest, &[
        ("dc", -8), ("dc.ff", -3), ("eff.18z.1", -6), ("eff.z.3", -5), ("hw.len.0", 16), ("hw.len.1", 16),
        ("hw.len.2", 16), ("hw.len0", 4), ("lp.rd", -7), ("ne.n0", 96), ("sil0", 3), ("ss.len", -57),
        ("v.3", -48), ("v.dm.3", 47), ("v.end.0", -7), ("v.end.1", -1), ("v.end.3", -7), ("vol.0", -12),
        ("vol.1", -59), ("vol.3", -59), ("vol.ff.1", -6), ("vol.ff.3", -6), ("vol.rel3.3", -1),
        ("vol.z.2", -59),
    ]),
];

fn costs(f: Flags) -> HashMap<String, i64> {
    let mut c: HashMap<String, i64> = COSTS.iter().map(|&(l, v)| (l.to_string(), v as i64)).collect();
    let b = build(f);
    let old = if f.newl { &[][..] } else { OLD };
    let mid = if matches!(b, Build::Plain | Build::Cursor | Build::Other) { MID } else { &[][..] };
    let own = BUILDS.iter().filter(|x| x.0 == b).flat_map(|x| x.1.iter());
    for &(l, v) in old.iter().chain(mid.iter()).chain(own) {
        *c.entry(l.to_string()).or_insert(0) += v as i64;
    }
    c
}

/// The colour clocks the original's init routine takes before the song starts (about eight frames):
/// it grows with the size of the code (the offset of its first table) and with the absolute addresses
/// in it, 229 and 133 colour clocks each, a least-squares fit to the 21 reference runs; their starts
/// are 4.2k colour clocks off it (root mean square), 8.2k at most, part of it the song's data (the
/// three Hammerfist files share their code and are 2.8k apart).
fn start(d: &[u8], lay: &Lay) -> u64 {
    let ce = [lay.songs, lay.patterns, lay.sinfo, lay.stab, lay.instr, lay.intervals, lay.arpbuf, lay.periods, lay.pc, lay.v - 8]
        .into_iter()
        .min()
        .unwrap()
        .min(d.len());
    let nabs = (0..ce.saturating_sub(3))
        .step_by(2)
        .filter(|&i| be32(d, i).wrapping_sub(lay.origin) < d.len() as u32)
        .count() as u64;
    4058 + (22878 * ce as u64 + 13283 * nabs) / 100
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
