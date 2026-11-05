//! MIDI - Loriciel (Amiga; the music of Loriciel's games such as "Entity", 1993; Eagleplayer
//! "MIDI - Loriciel", song file `MIDI.NAME` or `NAME.mid`, sample bank `SMPL.NAME` or `NAME.BSP`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player; no original code, and nothing is embedded (the period table and
//! the velocity curve are values of the specification). The song is a Standard MIDI File (no
//! running status, no controllers); the bank holds programs of key regions that point at 8-bit
//! one-shot samples. Four voices; a note starts a sample on the first free voice (or steals one),
//! and every tick each owned voice is pointed at a four-byte silence block, so the sample plays
//! once and the voice then loops on silence. Every write is placed at the clock of the
//! specification's cycle ledger, including the nine-line raster waits after a DMA write (modelled
//! from the beam phase) and the start-up calls' cost for the first tick.
//!
//! No real module exists (only the specification's six synthetic song and bank pairs), so the
//! replayer is registered last.
//!
//! Choices where the specification is silent or undefined (all safe, none can panic or hang):
//! reads past the end of the song file give zero and a track that runs off the end of the file
//! ends at that point; a program change to a program that does not exist is ignored; a bank
//! with a program of no regions, a reference outside the bank or a non-negative (already
//! converted) reference is refused; a note above every region's upper key uses the last
//! region; a note outside the period table's range uses its nearest entry; a tempo that gives
//! a zero divisor is ignored; a tempo meta event or a channel prefix whose bytes are cut off
//! reads zeros; the host's volume, balance and voice mutes are not modelled (the volume is the
//! module's volume); the lower-case and `.mid` spellings of the second-file load cost the same
//! start-up time as the upper-case one.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "MIDI - Loriciel", detect, new };

const LINE: u64 = 227;

/// Period by the note's distance from the region's base note, -29..=30.
const PERIODS: [u16; 60] = [
    1991, 1882, 1772, 1678, 1584, 1491, 1408, 1329, 1254, 1184, 1117, 1055, 995, 940, 887, 837, 790, 746, 704, 664, 627,
    592, 559, 527, 498, 470, 443, 419, 395, 373, 352, 332, 314, 296, 279, 264, 249, 235, 222, 209, 198, 186, 176, 166,
    157, 148, 140, 132, 124, 117, 111, 105, 99, 93, 88, 83, 78, 74, 70, 66,
];

/// The cost before the voice select, by voice (spec 7.5).
const SEL: [u64; 4] = [43, 74, 105, 113];

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// Spec 1.1: the song file's header.
fn detect(d: &[u8]) -> bool {
    d.len() >= 18
        && &d[0..4] == b"MThd"
        && be32(d, 4) == 6
        && be16(d, 8) <= 1
        && be16(d, 10) != 0
        && be16(d, 12) != 0
        && &d[14..18] == b"MTrk"
}

/// One key region of a program.
#[derive(Clone, Copy)]
struct Region {
    /// The offset of the sample header in the bank.
    hdr: usize,
    base: i8,
    upper: i8,
}

/// A parsed sample bank.
struct Bank {
    programs: Vec<Vec<Region>>,
    /// (data offset, length) of each header, by header offset.
    headers: Vec<(usize, usize, u32)>,
    /// The conversion's cost in cycles (spec 7.11).
    cost: u64,
}

impl Bank {
    fn parse(d: &[u8]) -> Option<Bank> {
        if d.len() < 8 || &d[0..4] != b"BNKS" {
            return None;
        }
        let p = be32(d, 4) as usize;
        if p > d.len() / 4 || 8 + 4 * p > d.len() {
            return None;
        }
        let mut programs = Vec::new();
        let mut headers: Vec<(usize, usize, u32)> = Vec::new();
        let mut cost = 60u64;
        let mut last: Option<(usize, u32)> = None;
        for i in 0..p {
            let o = be32(d, 8 + 4 * i) as usize;
            if o.checked_add(2)? > d.len() {
                return None;
            }
            let r = be16(d, o) as usize;
            if r == 0 || o + 2 + 8 * r > d.len() {
                return None;
            }
            cost += 54;
            let mut regs = Vec::new();
            for k in 0..r {
                let ro = o + 2 + 8 * k;
                let href = be32(d, ro) as i32;
                if href >= 0 {
                    return None;
                }
                let hdr = href.checked_neg()? as usize;
                if hdr.checked_add(8)? > d.len() {
                    return None;
                }
                cost += 39 + 51;
                if !headers.iter().any(|h| h.0 == hdr) {
                    let dref = be32(d, hdr) as i32;
                    if dref >= 0 {
                        return None;
                    }
                    let data = dref.checked_neg()? as usize;
                    let len = be32(d, hdr + 4);
                    if data > d.len() {
                        return None;
                    }
                    cost += 30;
                    headers.push((hdr, data, len));
                    last = Some((data, len));
                }
                regs.push(Region { hdr, base: byte(d, ro + 4) as i8, upper: byte(d, ro + 6) as i8 });
            }
            programs.push(regs);
        }
        if programs.is_empty() {
            return None;
        }
        // the sample area, up to the last converted header's data end, must be in the file
        if let Some((data, len)) = last {
            if data as u64 + len as u64 > d.len() as u64 {
                return None;
            }
        }
        Some(Bank { programs, headers, cost })
    }

    fn header(&self, hdr: usize) -> (usize, usize) {
        self.headers.iter().find(|h| h.0 == hdr).map_or((0, 0), |h| (h.1, h.2 as usize))
    }
}

#[derive(Clone, Copy)]
struct Track {
    pos: usize,
    active: bool,
    /// The tick counter; `None` until the track's first tick.
    count: Option<i32>,
}

pub struct MidiLoriciel {
    d: Vec<u8>,
    bank: Bank,
    bank_addr: u32,
    /// Where each track's events start.
    starts: Vec<usize>,
    tbase: u32,
    tracks: Vec<Track>,
    remaining: usize,
    program: [usize; 16],
    link: [Option<usize>; 16],
    owner: [Option<u8>; 4],
    note: [u8; 4],
    /// The timer latch.
    latch: u32,
    origin: u64,
    songend: bool,
}

/// The cycle count of one call of the reference (spec 7.1): `t` cycles from the call's start
/// clock `t0`; `rel` is the call's start relative to the run's origin (the beam's reference).
struct Clk {
    t0: u64,
    rel: u64,
    t: u64,
}

impl Clk {
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.t / 2;
    }

    fn line(&self) -> u64 {
        (self.rel + self.t / 2) / LINE
    }

    /// A host service call.
    fn service(&mut self) {
        self.t -= self.t % 2;
        self.t += 16;
    }

    /// The nine-line wait (spec 7.3).
    fn wait(&mut self) {
        self.t += 28;
        for _ in 0..9 {
            self.t += 16;
            let l0 = self.line();
            // bounded: the beam moves on within a line
            for _ in 0..4096 {
                self.t += 16;
                if self.line() != l0 {
                    break;
                }
                self.t += 9;
            }
            self.t += 19;
        }
        self.t += 44;
    }
}

/// The cost of a variable-length quantity of `b` bytes.
fn vcost(b: u64) -> u64 {
    53 * b + 44 * (b - 1) + 12
}

/// The cost of matching `v` against a list (spec 7.4).
fn mcost(list: &[u8], v: u8) -> u64 {
    let mut c = 0;
    for &x in list {
        c += 58;
        if x == v {
            return c + 8;
        }
        c += 18;
    }
    c + 21
}

impl MidiLoriciel {
    /// A variable-length quantity at the cursor: (value, bytes used).
    fn vlq(&self, pos: &mut usize) -> (u32, u64) {
        let mut v = 0u32;
        let mut n = 0;
        // bounded: four bytes at most, as the delta range of the specification
        for _ in 0..4 {
            let b = byte(&self.d, *pos);
            *pos += 1;
            n += 1;
            v = (v << 7) | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                break;
            }
        }
        (v, n)
    }

    /// Spec 7.9 and 3.5: parse again; the cost.
    fn parse(&mut self, clk: &mut Clk) {
        let n = self.starts.len() as u64;
        clk.t += 97 + if be16(&self.d, 12) & 0x8000 == 0 { 88 } else { 28 } + 28 + 293 * n + 20;
        self.tracks = self.starts.iter().map(|&p| Track { pos: p, active: true, count: None }).collect();
        self.remaining = self.starts.len();
    }

    /// Spec 6.1 and 7.7: set the tempo from microseconds per quarter note.
    fn set_tempo(&mut self, clk: &mut Clk, us: u32) {
        clk.t += 558;
        clk.service();
        clk.t += 60;
        let mut q = us / 100;
        if q > 0xFFFF {
            q = us & 0xFFFF;
        }
        if q == 0 {
            return; // the original does not handle it: ignored
        }
        let mut r = self.tbase / q;
        if r > 0xFFFF {
            r = self.tbase & 0xFFFF;
        }
        let f = r >> 2;
        if f == 0 {
            return; // likewise
        }
        let mut l = 716_000 / f;
        if l > 0xFFFF {
            l = 716_000 & 0xFFFF;
        }
        self.latch = l;
    }

    /// Spec 5.2 and 7.10.
    fn reset(&mut self, hw: &mut Hw, clk: &mut Clk) {
        clk.t += 176;
        for v in 0..4 {
            clk.t += 16;
            clk.at(hw);
            hw.vol(v, 0);
        }
        clk.t += 16;
        clk.at(hw);
        hw.dmacon(0x000F);
        clk.t += 16;
        clk.at(hw);
        hw.bus.write_word(0xDFF09E, 0x00FF);
        clk.t += 24 + 4 * 54 + 70 + 16 * 62 + 194;
        self.owner = [None; 4];
        self.link = [None; 16];
        self.program = [0; 16];
        self.set_tempo(clk, 500_000);
        clk.t += 16;
    }

    /// Spec 3.4 and 7.2: the tail.
    fn tail(&mut self, hw: &mut Hw, clk: &mut Clk) {
        clk.t += 42;
        for v in 0..4 {
            clk.t += 21;
            if self.owner[v].is_some() {
                clk.t += 24;
                clk.at(hw);
                hw.loc(v, self.bank_addr);
                clk.t += 16;
                clk.at(hw);
                hw.len(v, 2);
            }
            clk.t += 26;
        }
    }

    /// Spec 4.5: the volume of velocity `v`.
    fn volume(v: u8) -> u16 {
        let i = (v >> 1) as u16;
        let k = if i < 16 {
            i
        } else if i < 32 {
            16 + (i - 16) / 2
        } else {
            24 + (i - 32) / 4
        };
        (2 * k) & 0x7F
    }

    /// Spec 4.3 and 7.5.
    fn note_on(&mut self, hw: &mut Hw, clk: &mut Clk, c: u8, n: u8, v: u8) {
        clk.t += 18 + 24;
        let mut sel = None;
        for i in 0..4 {
            clk.t += 17;
            if self.owner[i].is_none() {
                sel = Some(i);
                break;
            }
            clk.t += 31;
            if self.owner[i] == Some(c) {
                clk.t += 4;
            }
        }
        let x = match sel {
            Some(i) => i,
            None => {
                clk.t += 4;
                (0..4).rev().find(|&i| self.owner[i] == Some(c)).unwrap_or(0)
            }
        };
        clk.t += 118;
        self.owner[x] = Some(c);
        self.note[x] = n;
        self.link[c as usize] = Some(x);
        // the region
        clk.t += 58;
        let regs = &self.bank.programs[self.program[c as usize]];
        let mut reg = regs[regs.len() - 1];
        for r in regs {
            clk.t += 21;
            if (n as i8) <= r.upper {
                reg = *r;
                break;
            }
            clk.t += 18;
        }
        let (data, len) = self.bank.header(reg.hdr);
        let words = ((len & 0xFFFF) / 2) as u16;
        let d = (n as i8 as i32 - reg.base as i32 + 29).clamp(0, 59) as usize;
        let per = PERIODS[d];
        clk.t += 70;
        clk.at(hw);
        hw.loc(x, self.bank_addr.wrapping_add(data as u32));
        clk.t += 32;
        clk.at(hw);
        hw.len(x, words);
        clk.t += SEL[x] + 78 + 50;
        clk.at(hw);
        hw.per(x, per);
        clk.t += SEL[x] + 64 + 16;
        clk.t += 82 + 31 + 23 * (x.min(2) as u64) + 103 + if x < 3 { 10 } else { 0 } + 30;
        clk.at(hw);
        hw.vol(x, Self::volume(v));
        clk.t += 16 + 18 + SEL[x] + 36 + 16;
        clk.t += 74;
        clk.at(hw);
        hw.dmacon(0x8000 | 1 << x);
        clk.t += 18;
        clk.wait();
        clk.t += 16;
    }

    /// Spec 4.4 and 7.6.
    fn note_off(&mut self, hw: &mut Hw, clk: &mut Clk, c: u8, n: u8) {
        clk.t += 18 + 65;
        let mut freed = None;
        if let Some(x) = self.link[c as usize] {
            clk.t += 21;
            if self.owner[x] == Some(c) {
                clk.t += 29;
                if self.note[x] == n {
                    freed = Some(x);
                }
            }
        }
        match freed {
            None => clk.t += 20 + 13 + 16,
            Some(x) => {
                self.owner[x] = None;
                self.link[c as usize] = None;
                clk.t += 72 + 13 + 70;
                clk.at(hw);
                hw.dmacon(1 << x);
                clk.t += 18;
                clk.wait();
                clk.t += 16;
            }
        }
    }

    /// Spec 4 and 7.4: one event of track `ti`; whether the track ended.
    fn event(&mut self, hw: &mut Hw, clk: &mut Clk, ti: usize) -> bool {
        let mut pos = self.tracks[ti].pos;
        let s = byte(&self.d, pos);
        pos += 1;
        clk.t += 25;
        let mut ended = false;
        if s >= 0xF0 {
            clk.t += 8;
            clk.t += mcost(&[0xF0, 0xFF], s);
            if s == 0xF0 {
                let (l, b) = self.vlq(&mut pos);
                pos = pos.saturating_add(l as usize);
                clk.t += 4 + vcost(b) + 24;
            } else if s == 0xFF {
                let ty = byte(&self.d, pos);
                pos += 1;
                clk.t += 16;
                clk.t += mcost(&[0, 1, 2, 3, 4, 5, 6, 7, 0x20, 0x2F, 0x51, 0x54, 0x58, 0x59, 0x7F], ty);
                match ty {
                    0x2F => {
                        pos += 1;
                        ended = true;
                        clk.t += 56;
                    }
                    0x51 => {
                        pos += 1;
                        let us = (byte(&self.d, pos) as u32) << 16
                            | (byte(&self.d, pos + 1) as u32) << 8
                            | byte(&self.d, pos + 2) as u32;
                        pos += 3;
                        clk.t += 80;
                        self.set_tempo(clk, us);
                        clk.t += 16;
                    }
                    0x20 => {
                        pos += 2;
                        clk.t += 48;
                    }
                    _ => {
                        let (l, b) = self.vlq(&mut pos);
                        pos = pos.saturating_add(l as usize);
                        clk.t += 4 + vcost(b) + 24;
                    }
                }
            } else {
                clk.t += 24;
                pos += 2;
            }
        } else {
            clk.t += 36;
            let c = s & 15;
            let kind = s & 0xF0;
            clk.t += mcost(&[0x90, 0x80, 0xC0], kind);
            match kind {
                0x90 | 0x80 => {
                    if kind == 0x90 {
                        clk.t += 21;
                    }
                    let n = byte(&self.d, pos);
                    let v = byte(&self.d, pos + 1);
                    pos += 2;
                    if kind == 0x90 && v != 0 {
                        clk.t += 78 + 186;
                        self.note_on(hw, clk, c, n, v);
                    } else {
                        clk.t += 78 + 186;
                        self.note_off(hw, clk, c, n);
                    }
                    clk.t += 148 + 16;
                }
                0xC0 => {
                    let p = byte(&self.d, pos) as usize;
                    pos += 1;
                    // a program that does not exist: ignored (undefined in the original)
                    if p < self.bank.programs.len() {
                        self.program[c as usize] = p;
                    }
                    clk.t += 66 + 176 + 138 + 148 + 16;
                }
                _ => {
                    clk.t += 24;
                    pos += 2;
                }
            }
        }
        self.tracks[ti].pos = pos;
        if ended {
            self.tracks[ti].active = false;
            self.remaining -= 1;
        }
        ended
    }

    /// Spec 3 and 7.2: one track's turn.
    fn track(&mut self, hw: &mut Hw, clk: &mut Clk, ti: usize) {
        let mut need_delta;
        match self.tracks[ti].count {
            None => {
                self.tracks[ti].count = Some(0);
                clk.t += 24;
                need_delta = true;
            }
            Some(c) => {
                clk.t += 33;
                let c = c.wrapping_sub(4);
                self.tracks[ti].count = Some(c);
                if c != 0 {
                    clk.t += 9;
                }
                if c > 0 {
                    clk.t += 10;
                    clk.t += 18;
                    return;
                }
                need_delta = false;
            }
        }
        // bounded: every event consumes a byte and the track ends at the file's end
        loop {
            if self.tracks[ti].pos >= self.d.len() {
                // runs off the end of the file: the track ends (safe choice)
                self.tracks[ti].active = false;
                self.remaining -= 1;
                break;
            }
            if need_delta {
                clk.t += 4;
                let mut pos = self.tracks[ti].pos;
                let (d, b) = self.vlq(&mut pos);
                self.tracks[ti].pos = pos;
                clk.t += vcost(b) + 13;
                if d != 0 {
                    clk.t += 50;
                    let c = self.tracks[ti].count.unwrap_or(0);
                    self.tracks[ti].count = Some(c.wrapping_add(d as i32));
                    break;
                }
                if pos >= self.d.len() {
                    continue;
                }
            }
            clk.t += 26;
            let ended = self.event(hw, clk, ti);
            clk.t += 17;
            if ended {
                break;
            }
            clk.t += 10;
            need_delta = true;
        }
        clk.t += 18;
    }

    /// Spec 3.5 and 7.8.
    fn song_end(&mut self, hw: &mut Hw, clk: &mut Clk) {
        clk.t += 90;
        clk.service();
        self.songend = true;
        clk.t += 78;
        self.parse(clk);
        clk.t += 18;
        self.reset(hw, clk);
        clk.t += 10;
        self.tail(hw, clk);
    }
}

impl Replayer for MidiLoriciel {
    fn format(&self) -> &'static str {
        "MIDI - Loriciel"
    }

    fn title(&self) -> String {
        String::new()
    }

    /// The latch as it stood before the tick that is running (the engine asks first), so a tempo
    /// event acts from the interval after the next one (spec 6.2).
    fn tick_clocks(&self) -> u64 {
        (self.latch as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        let mut clk = Clk { t0, rel: t0.saturating_sub(self.origin), t: 273 };
        if self.remaining == 0 {
            self.song_end(hw, &mut clk);
            return;
        }
        clk.t += 4;
        let todo = self.remaining;
        let mut handled = 0;
        for ti in 0..self.tracks.len() {
            if handled >= todo {
                break;
            }
            if !self.tracks[ti].active {
                clk.t += 37;
                continue;
            }
            handled += 1;
            clk.t += 68;
            self.track(hw, &mut clk, ti);
        }
        self.tail(hw, &mut clk);
    }
}

/// The bank's name for a song file: `SMPL.NAME` for `MIDI.NAME`, `NAME.BSP` for `NAME.mid`.
fn companion(file: &str) -> Option<String> {
    let b = file.as_bytes();
    if b.len() >= 5 && b[..5].eq_ignore_ascii_case(b"midi.") {
        return Some(format!("SMPL{}", &file[4..]));
    }
    if b.len() >= 4 && b[b.len() - 4..].eq_ignore_ascii_case(b".mid") {
        return Some(format!("{}BSP", &file[..file.len() - 3]));
    }
    None
}

/// Walks the tracks: the start of each one's events, or `None` if the song is corrupt or short.
fn tracks_of(d: &[u8]) -> Option<Vec<usize>> {
    let n = be16(d, 10) as usize;
    let mut pos = 14usize;
    let mut starts = Vec::new();
    for _ in 0..n {
        if d.get(pos..pos + 4)? != b"MTrk" {
            return None;
        }
        let l = be32(d, pos + 4) as usize;
        starts.push(pos + 8);
        pos = pos.checked_add(8)?.checked_add(l)?;
    }
    (pos <= d.len()).then_some(starts)
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let starts = tracks_of(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let dir = m.bus.read_cstr(m.deli.dir_buf);
    let bank_file = crate::deli::read_companion(m, &companion(&file)?).1.ok()?;
    let bank = Bank::parse(&bank_file)?;
    let bank_addr = m.alloc(bank_file.len() as u32 + 16);
    if bank_addr == 0 {
        return None;
    }
    // the "BNKS" magic is replaced by four zero bytes: the silence block
    let mut img = bank_file.clone();
    img[..4].fill(0);
    m.bus.write_bytes(bank_addr, &img);

    let division = be16(d, 12);
    let tbase = if division & 0x8000 == 0 { division as u32 * 10000 } else { 192 * 10000 };
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut p = MidiLoriciel {
        d: d.to_vec(),
        bank,
        bank_addr,
        starts,
        tbase,
        tracks: Vec::new(),
        remaining: 0,
        program: [0; 16],
        link: [None; 16],
        owner: [None; 4],
        note: [0; 4],
        latch: 0,
        origin,
        songend: false,
    };

    // the start-up calls (spec 7.11), each from the clock where the previous one ended
    let mut c = crate::engine::START_PHASE;
    let mut hw = Hw { bus: &mut m.bus };
    let call = |c: &mut u64, f: &mut dyn FnMut(&mut Clk)| {
        let mut clk = Clk { t0: origin + *c, rel: *c, t: 0 };
        f(&mut clk);
        *c += clk.t / 2;
    };
    call(&mut c, &mut |k| k.t += 180);
    let (dl, fl) = (dir.len() as u64, file.len() as u64);
    call(&mut c, &mut |k| {
        k.t += 60;
        k.service();
        k.t += 62 + 17 * (dl + 1) + 21 * (fl + 1) + 235;
        k.service();
    });
    let conv = p.bank.cost;
    let ntracks = p.starts.len() as u64;
    let smpte = division & 0x8000 != 0;
    call(&mut c, &mut |k| {
        k.t += 36;
        k.service();
        k.t += 78 + 97 + if smpte { 28 } else { 88 } + 28 + 293 * ntracks + 20;
        k.t += 116;
        k.service();
        k.t += 91 + conv + 16 + 117;
        k.service();
    });
    call(&mut c, &mut |k| k.t += 24);
    for _ in 0..2 {
        call(&mut c, &mut |k| {
            k.t += 240;
            for x in 0..4usize {
                k.t += 26;
                k.t += 31 + 23 * (x.min(2) as u64) + 103 + if x < 3 { 10 } else { 0 } + 30;
                k.at(&mut hw);
                hw.vol(x, 0);
                k.t += 16 + 18;
            }
            k.t += 16;
        });
    }
    call(&mut c, &mut |k| {
        k.t += 1178;
        p.parse(k);
        k.t += 55;
        k.service();
        k.t += 10;
        p.reset(&mut hw, k);
    });
    m.bus.clock = (origin + c).max(m.bus.clock);
    Some(Box::new(p))
}

#[cfg(test)]
#[path = "midiloriciel_tests.rs"]
mod tests;
