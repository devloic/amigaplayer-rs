//! Jesper Olsen (`jo.*`), the music of Lollypop, Georg Glaxo, Guldkorn Expressen and
//! Kalas Puffs Expressen.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Jesper Olsen player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! The format has three engines. Engines A (Georg Glaxo) and B (Lollypop) are data
//! only: the module is a memory image of four voice structures, instrument records,
//! scripts, tracks, phrases and samples, and the replay routine is a separate file
//! in the original (`WantedTeam.bin`), whose length says which engine the game
//! has. Both engines are written here, and the one the companion's length names
//! plays. Engine C (Guldkorn and Kalas Puffs Expressen) is an Amiga executable image
//! whose code is not run: its tables are located in it structurally, and its two
//! code versions (the title music and the in-game music) are written here from
//! their behaviour. All three play at 50 Hz, four voices, one Paula channel each.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Jesper Olsen", detect, new };

/// Paula periods of the notes of engines A and B (index 95 is a terminator).
const PERIODS: [u16; 96] = [
    27360, 25824, 24384, 23008, 21712, 20496, 19344, 18256, 17232, 16272, 15360, 14496, 13680, 12912, 12192, 11504,
    10856, 10248, 9672, 9128, 8616, 8136, 7680, 7248, 6840, 6456, 6096, 5752, 5428, 5124, 4836, 4564, 4308, 4068, 3840,
    3624, 3420, 3228, 3048, 2876, 2714, 2562, 2418, 2282, 2154, 2034, 1920, 1812, 1710, 1614, 1524, 1438, 1357, 1281,
    1209, 1141, 1077, 1017, 960, 906, 855, 807, 762, 719, 679, 641, 605, 571, 539, 508, 480, 453, 428, 404, 381, 360,
    339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 0,
];

/// Loops over streams that never end (a jump to itself) give up after this many steps.
const GUARD: u32 = 4096;

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn rw(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

// ---------------------------------------------------------------------------------
// Recognition

/// The offsets of the song lists of a data module (engines A and B): the table at
/// the start is `w0/2` words, the first of which, `w0`, is the offset of the first list.
fn song_lists(d: &[u8]) -> Option<Vec<usize>> {
    if d.len() < 0x60 {
        return None;
    }
    let w0 = rw(d, 0) as usize;
    if w0 % 2 != 0 || !(4..=0x200).contains(&w0) {
        return None;
    }
    let mut v = vec![w0];
    for i in 1..w0 / 2 {
        let p = rw(d, 2 * i) as usize;
        if p == 0 || p % 2 != 0 || p >= 0x8000 || p < w0 || p + 2 > d.len() || rw(d, p - 2) != 0x7FFF {
            return None;
        }
        v.push(p);
    }
    Some(v)
}

/// A list is 10-byte records and the word 0x7FFF; every record names a voice
/// structure that fits, a channel, and an instrument table inside the file.
fn list_ok(d: &[u8], mut p: usize, w0: usize) -> Option<usize> {
    let mut n = 0;
    loop {
        if p + 2 > d.len() {
            return None;
        }
        if rw(d, p) == 0x7FFF {
            return Some(n);
        }
        if p + 10 > d.len() || n >= 8 {
            return None;
        }
        let voice = rw(d, p) as usize;
        let ins = rw(d, p + 2) as usize;
        if voice % 2 != 0 || voice < w0 || voice + 0x44 > d.len() || d[p + 6] > 3 || ins % 2 != 0 || ins < w0 || ins >= d.len() {
            return None;
        }
        n += 1;
        p += 10;
    }
}

fn detect_data(d: &[u8]) -> bool {
    let Some(lists) = song_lists(d) else { return false };
    let w0 = lists[0];
    match list_ok(d, w0, w0) {
        Some(n) if n >= 1 => {}
        _ => return false,
    }
    lists[1..].iter().all(|&p| list_ok(d, p, w0).is_some())
}

/// The engine of an executable-image module: the layout constants of the two
/// code versions in the corpus.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CKind {
    /// Title music: song-table stride 14.
    T,
    /// In-game music: song-table stride 16.
    G,
}

/// What the structural scan of an engine-C module finds.
#[derive(Clone, Copy, Debug)]
struct CLayout {
    kind: CKind,
    song_table: usize,
    pattern_base: usize,
    silent: usize,
    period_table: usize,
    /// T: the instrument-select map. G: unused.
    insmap: usize,
    /// The instrument table (the record of instrument word 0 for G).
    ins_table: usize,
}

/// A `LEA d16(PC),An` at `o` (opcode `0x41FA | n << 9`): the register and the target.
fn lea_at(d: &[u8], o: usize) -> Option<(usize, i64)> {
    let w = rw(d, o);
    if o + 4 > d.len() || w & 0xF1FF != 0x41FA {
        return None;
    }
    let disp = be16(d, o + 2) as i16 as i64;
    Some((((w >> 9) & 7) as usize, o as i64 + 2 + disp))
}

fn scan_c(d: &[u8]) -> Option<CLayout> {
    if d.len() < 0x400 || rw(d, 0) != 0x6000 || rw(d, 4) != 0x6000 || rw(d, 8) != 0x6000 {
        return None;
    }
    for o in [2usize, 6, 10] {
        let disp = rw(d, o) as usize;
        if disp == 0 || disp >= 0x8000 || disp % 2 != 0 || o + disp >= d.len() {
            return None;
        }
    }
    // the init routine: target of the second branch
    let init = 6 + rw(d, 6) as usize;
    // an AND.L #255,D0 then a MULU.W #stride,D0 within the first 48 bytes
    let mut mulu = None;
    for o in (init..(init + 48).min(d.len().saturating_sub(10))).step_by(2) {
        if d[o..o + 6] == [0x02, 0x80, 0x00, 0x00, 0x00, 0xFF] && rw(d, o + 6) == 0xC0FC {
            mulu = Some((o, rw(d, o + 8) as usize));
            break;
        }
    }
    let (and_at, stride) = mulu?;
    let kind = match stride {
        14 => CKind::T,
        16 => CKind::G,
        _ => return None,
    };
    // the period table: 27360, 25824 at an even offset 800..1700 bytes after the MULU
    let mul = and_at + 6;
    let mut period = None;
    for o in ((mul + 800)..(mul + 1700).min(d.len().saturating_sub(4))).step_by(2) {
        if be32(d, o) == 0x6AE0_64E0 {
            period = Some(o);
            break;
        }
    }
    let period_table = period?;
    // the song table: the LEA into A1 just before the AND
    let (reg, target) = lea_at(d, and_at.checked_sub(4)?)?;
    if reg != 1 || target < 0 {
        return None;
    }
    let song_table = target as usize;
    // pattern base: the only LEA into A4 that is neither 0 nor the period table
    let mut pattern = None;
    let mut a3: Vec<i64> = Vec::new();
    for o in (0..period_table).step_by(2) {
        if let Some((reg, t)) = lea_at(d, o) {
            if reg == 4 && t != 0 && t != period_table as i64 {
                pattern = Some(t);
            }
            if reg == 3 && pattern.is_some() {
                a3.push(t);
            }
        }
    }
    let pattern_base = usize::try_from(pattern?).ok()?;
    let (silent, insmap, ins_table) = match kind {
        CKind::T => {
            // the A3 targets after the pattern-base LEA, in code order: the silent
            // buffer, the instrument-select map, the instrument table
            if a3.len() < 3 {
                return None;
            }
            (usize::try_from(a3[0]).ok()?, usize::try_from(a3[1]).ok()?, usize::try_from(a3[2]).ok()?)
        }
        CKind::G => {
            let pos = a3.iter().copied().find(|&t| t > 0)?;
            let neg = a3.iter().copied().find(|&t| t < 0)?;
            (usize::try_from(pos).ok()?, 0, usize::try_from(neg + 0x6000).ok()?)
        }
    };
    for t in [song_table, pattern_base, silent, ins_table] {
        if t >= d.len() {
            return None;
        }
    }
    Some(CLayout { kind, song_table, pattern_base, silent, period_table, insmap, ins_table })
}

fn detect(d: &[u8]) -> bool {
    if rw(d, 0) == 0x6000 {
        return scan_c(d).is_some();
    }
    detect_data(d)
}

// ---------------------------------------------------------------------------------
// The host's side: companion, subsong count, choice of engine

/// Which of the two data engines plays.
#[derive(Clone, Copy, PartialEq, Debug)]
#[allow(dead_code)]
enum Eng {
    A,
    B,
}

/// The length of the companion `WantedTeam.bin` beside the module, if there is one
/// (its length only: it holds the original's replay code).
fn companion_len(m: &Machine) -> Option<u64> {
    let p = m.os.host_path("WantedTeam.bin", Some(&m.deli.module_dir));
    let want = p.file_name()?.to_string_lossy().to_lowercase();
    if let Some((_, d)) = m.os.mem_files.iter().find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(&p.to_string_lossy())) {
        return Some(d.len() as u64);
    }
    if let Ok(md) = std::fs::metadata(&p) {
        return Some(md.len());
    }
    for e in std::fs::read_dir(p.parent()?).ok()?.flatten() {
        if e.file_name().to_string_lossy().to_lowercase() == want {
            return e.metadata().ok().map(|m| m.len());
        }
    }
    None
}

/// The engine: the reference plays every data module with the fractional-speed
/// scheme (engine B), whatever the companion's length says, so the companion is not
/// consulted. Engine A stays in the code but is not selected.
fn choose_engine(_d: &[u8], _companion: Option<u64>) -> Eng {
    Eng::B
}

/// The number of subsongs the host offers: one per list after the first, less a
/// dummy last list when the first voice's structure says so.
fn count_songs(d: &[u8]) -> u16 {
    let w0 = rw(d, 0) as usize;
    let mut n = w0 / 2 - 1;
    if n != 1 {
        let s = rw(d, w0) as usize;
        if s >= 6 && (rw(d, s - 6) == 0 || rw(d, s - 4) == 0x7F00) {
            n -= 1;
        }
    }
    n.max(1) as u16
}

// ---------------------------------------------------------------------------------
// Engines A and B

struct Ab {
    eng: Eng,
    pristine: Vec<u8>,
    img: Vec<u8>,
    base: u32,
    title: String,
    nsongs: u16,
    sub: u16,
    /// The voice structure of each channel.
    chan: [Option<usize>; 4],
    master: u8,
    pend_off: u8,
    pend_on: u8,
    alive: u8,
    alive0: u8,
    songend: bool,
    /// The song is started by the first tick or the first change of subsong, whichever
    /// comes first (the host may change subsong before it plays).
    started: bool,
}

impl Ab {
    fn w8(&mut self, o: usize, v: u8) {
        if let Some(b) = self.img.get_mut(o) {
            *b = v;
        }
    }

    fn w16(&mut self, o: usize, v: u16) {
        self.w8(o, (v >> 8) as u8);
        self.w8(o + 1, v as u8);
    }

    fn r8(&self, o: usize) -> u8 {
        rb(&self.img, o)
    }

    fn r16(&self, o: usize) -> u16 {
        rw(&self.img, o)
    }

    /// Stores the parameter words at `p` (first byte 0x80 or more), returns the new position.
    fn params(&mut self, s: usize, mut p: usize) -> usize {
        let mut guard = 0;
        while self.r8(p) >= 0x80 && guard < GUARD {
            let k = (self.r8(p) & 0x7F) as usize;
            let (a, b) = (self.r8(p), self.r8(p + 1));
            self.w8(s + k, a);
            self.w8(s + k + 1, b);
            p += 2;
            guard += 1;
        }
        p
    }

    /// Command "start song n".
    fn start_song(&mut self, n: usize, hw: &mut Hw) {
        self.master = 63;
        self.pend_off = 0;
        self.alive = 0;
        let mut p = self.r16(2 * n) as usize;
        if n == 0 {
            p = self.r16(0) as usize;
        }
        let mut guard = 0;
        while self.r16(p) != 0x7FFF && guard < 64 {
            guard += 1;
            let s = self.r16(p) as usize;
            let ins = self.r16(p + 2);
            let track = self.r16(p + 4);
            let ch = (self.r8(p + 6) & 3) as usize;
            let (speed, prio, vol) = (self.r8(p + 7), self.r8(p + 8), self.r8(p + 9));
            p += 10;
            if prio < self.r8(s + 0x40) {
                continue;
            }
            self.chan[ch] = Some(s);
            self.w16(s + 0x2E, 0);
            self.w16(s + 0x30, ins);
            self.w16(s + 0x34, track);
            self.w16(s + 0x3A, 1 << ch);
            self.w8(s + 0x03, speed);
            self.w8(s + 0x40, 0);
            if track != 0 {
                self.w8(s + 0x40, prio);
            }
            self.w8(s + 0x17, vol);
            self.w8(s + 0x14, 0);
            self.w8(s + 0x15, 63);
            self.w8(s + 0x12, 0);
            self.w8(s + 0x13, 63);
            self.w8(s + 0x43, n as u8);
            for o in [0x36, 0x06, 0x08, 0x0A, 0x20, 0x0C] {
                self.w16(s + o, 0);
            }
            if self.eng == Eng::A {
                self.w16(s + 0x32, 0x0001);
                self.w8(s + 0x41, 0);
            } else {
                self.w16(s + 0x32, 0x7F01);
                self.w8(s + 0x41, 0xFF);
                for o in [0x10, 0x18, 0x1A, 0x1C] {
                    self.w16(s + o, 0);
                }
            }
            hw.dmacon(1 << ch);
            if track != 0 {
                self.alive |= 1 << ch;
            }
        }
        self.alive0 = self.alive;
    }

    fn voice_end(&mut self, ch: usize) {
        self.alive &= !(1 << ch);
        if self.alive == 0 && self.alive0 != 0 {
            self.songend = true;
            self.alive = self.alive0;
        }
    }

    fn tick_ab(&mut self, hw: &mut Hw) {
        hw.dmacon(self.pend_off as u16);
        self.pend_off = 0;
        self.pend_on = 0;
        hw.wait(1000);
        for ch in 0..4 {
            if let Some(s) = self.chan[ch] {
                if self.r16(s + 0x34) != 0 {
                    self.voice(ch, s, hw);
                    hw.wait(400);
                }
            }
        }
        if self.eng == Eng::A {
            hw.wait(2270);
        }
        hw.dmacon(0x8000 | self.pend_on as u16);
    }

    fn voice(&mut self, ch: usize, s: usize, hw: &mut Hw) {
        let b3 = self.r8(s + 3);
        let fetch_tick = match self.eng {
            Eng::A => {
                let cnt = self.r8(s + 0x41);
                let phase = b3 & 0x80 != 0;
                let cnt = cnt.wrapping_sub(if phase { 0x10 } else { 1 });
                if (cnt as i8) >= 0 {
                    self.w8(s + 0x41, cnt);
                    false
                } else {
                    self.w8(s + 0x41, if phase { b3 & 7 } else { b3 & 0x70 });
                    self.w8(s + 3, b3 ^ 0x80);
                    true
                }
            }
            Eng::B => {
                let (sum, carry) = self.r8(s + 0x41).overflowing_add(b3);
                self.w8(s + 0x41, sum);
                carry
            }
        };
        if fetch_tick {
            let step = (self.r8(s + 0x33) & 0x7F).wrapping_sub(1);
            self.w8(s + 0x33, step);
            if step == 0 {
                self.fetch(ch, s, hw);
                return;
            }
        }
        self.effects(ch, s, hw);
    }

    fn fetch(&mut self, ch: usize, s: usize, hw: &mut Hw) {
        let bit = 1u8 << ch;
        // the track
        let mut t = self.r16(s + 0x34) as usize;
        let mut guard = 0;
        loop {
            match self.r16(t) {
                0x7FFF => {
                    self.w16(s + 0x34, 0);
                    hw.vol(ch, 0);
                    self.w8(s + 0x40, 0);
                    self.voice_end(ch);
                    return;
                }
                0x7FFE => {
                    t = self.r16(t + 2) as usize;
                    self.w16(s + 0x34, t as u16);
                    self.voice_end(ch);
                    guard += 1;
                    if guard > 8 {
                        return;
                    }
                }
                _ => break,
            }
        }
        let t = self.params(s, t);
        self.w16(s + 0x34, t as u16);
        let phrase = self.r16(t) as usize;
        let ppos = self.r16(s + 0x36) as usize;
        let start = phrase + ppos;
        self.w16(s + 0x0C, 0);
        let item = self.params(s, start);
        let (first, second) = (self.r8(item), self.r8(item + 1));
        if first == 0x7E || first == 0x7F {
            self.w8(s + 4, first);
            self.w8(s + 5, second);
            self.w8(s + 0x33, second);
            if first == 0x7F {
                self.w8(s + 0x32, 0x7F);
                self.pend_off |= bit;
            } else {
                self.w8(s + 4, 0);
                self.w8(s + 5, second & 0x80);
            }
        } else {
            if second & 0x80 == 0 {
                self.trigger(ch, s, hw);
            }
            self.w8(s + 0x32, first);
            self.w8(s + 0x33, second);
            if self.eng == Eng::B {
                let p = first.wrapping_add(self.r8(s + 0x0B));
                self.w8(s + 0x32, p);
                self.w16(s + 0x20, 0);
            }
        }
        // advance
        let next = item + 2;
        self.w16(s + 0x36, (next - phrase) as u16);
        if self.r16(next) == 0 {
            self.w16(s + 0x36, 0);
            let rep = self.r8(s + 7).wrapping_sub(1);
            self.w8(s + 7, rep);
            if rep & 0x80 != 0 {
                self.w8(s + 7, 0);
                let t = self.r16(s + 0x34);
                self.w16(s + 0x34, t.wrapping_add(2));
            }
        }
    }

    /// The instrument record of the voice's current instrument.
    fn record(&self, s: usize) -> usize {
        let tbl = self.r16(s + 0x30) as usize;
        let n = self.r8(s + 1) as usize;
        self.r16(tbl + 2 * n) as usize
    }

    fn trigger(&mut self, ch: usize, s: usize, hw: &mut Hw) {
        let bit = 1u8 << ch;
        self.w8(s + 0x42, 0xFF);
        let rec = self.record(s);
        for i in 0..3 {
            let a = self.r16(rec + 4 * i);
            let c = self.r8(rec + 4 * i + 2);
            let r = self.r8(rec + 4 * i + 3);
            self.w16(s + 0x22 + 4 * i, a);
            self.w8(s + 0x24 + 4 * i, c);
            self.w8(s + 0x25 + 4 * i, r);
        }
        self.w16(s + 0x20, 0);
        let flags = self.r8(rec + 13);
        match self.eng {
            Eng::A => {
                if flags & 1 != 0 {
                    hw.dmacon(bit as u16);
                } else {
                    self.pend_off |= bit;
                }
            }
            Eng::B => {
                if flags & 4 != 0 {
                    if self.r8(s + 0x1C) != 0 {
                        return;
                    }
                    self.w8(s + 0x1C, 0xFF);
                } else {
                    self.w8(s + 0x1C, 0);
                }
                self.pend_off |= bit;
                if flags & 1 == 0 {
                    hw.dmacon(bit as u16);
                }
            }
        }
    }

    fn effects(&mut self, ch: usize, s: usize, hw: &mut Hw) {
        if self.r8(s + 0x32) == 0x7F {
            return;
        }
        self.pend_on |= 1 << ch;
        let eng = self.eng;
        if eng == Eng::A {
            self.w16(s + 0x0E, 0);
            self.w16(s + 0x08, 0);
        }
        let rec = self.record(s);
        let flags = self.r8(rec + 13);
        let wave = self.r8(s + 0x1F) as u32;
        let base = self.base;
        let first = self.r8(s + 0x42) != 0;
        let start = be32(&self.img, rec + 14);
        let alen = self.r16(rec + 18);
        let lstart = be32(&self.img, rec + 20);
        let llen = self.r16(rec + 24);
        let iff = eng == Eng::B && start & 0x8000_0000 != 0;
        if first {
            self.w8(s + 0x42, 0);
            self.w8(s + 5, 0xFF);
            if eng == Eng::B {
                if flags & 2 != 0 {
                    self.w8(s + 0x1F, 0);
                }
                self.w16(s + 0x0E, 0);
                self.w16(s + 0x08, 0);
            }
            let wave = if eng == Eng::B && flags & 2 != 0 { 0 } else { wave };
            if iff {
                let data = start & 0x7FFF_FFFF;
                let hdr = data as usize - 0x68.min(data as usize);
                let one = self.r16(hdr + 0x16);
                let rep = self.r16(hdr + 0x1A);
                hw.loc(ch, base.wrapping_add(data));
                hw.len(ch, one.wrapping_add(rep) >> 1);
            } else {
                let mut a = start;
                if eng == Eng::B && flags & 0x80 != 0 {
                    a = a.wrapping_add(wave.wrapping_mul(alen as u32).wrapping_mul(2));
                }
                hw.loc(ch, base.wrapping_add(a));
                hw.len(ch, alen);
            }
        } else if iff {
            let data = start & 0x7FFF_FFFF;
            let hdr = data as usize - 0x68.min(data as usize);
            let one = self.r16(hdr + 0x16);
            let rep = self.r16(hdr + 0x1A);
            hw.len(ch, rep >> 1);
            hw.loc(ch, base.wrapping_add(data).wrapping_add(one as u32));
        } else {
            let mut a = lstart;
            if flags & 0x80 != 0 {
                let unit = if eng == Eng::A { llen } else { alen } as u32;
                a = a.wrapping_add(wave.wrapping_mul(unit).wrapping_mul(2));
            }
            hw.loc(ch, base.wrapping_add(a));
            hw.len(ch, llen);
        }
        self.scripts(s);
        // the slide
        let slide = self.r16(s + 0x20).wrapping_add(self.r8(s + 0x0D) as i8 as i16 as u16);
        self.w16(s + 0x20, slide);
        // volume
        let mut v = self.r8(s + 0x17) as u32;
        v = v * self.r8(s + 0x15) as u32 / 63;
        v = v * self.r8(s + 0x13) as u32 / 63;
        v = v * self.master as u32 / 63;
        v = v * self.r8(rec + 12) as u32 / 63;
        hw.vol(ch, v as u16);
        // period
        let mut n = self.r8(s + 0x09);
        if n & 0x80 == 0 {
            n = n.wrapping_add(self.r8(s + 0x32));
            if eng == Eng::A {
                n = n.wrapping_add(self.r8(s + 0x0B));
            }
        }
        let mut per = PERIODS[(n & 0x7F) as usize] as i32 - self.r8(s + 0x11) as i8 as i32;
        if self.r8(s + 0x19) & 1 == 0 {
            per -= self.r8(s + 0x0F) as i8 as i32;
        }
        per -= slide as i32;
        hw.per(ch, per as u16);
    }

    fn scripts(&mut self, s: usize) {
        for i in 0..3 {
            let (pp, cp, rp) = (s + 0x22 + 4 * i, s + 0x24 + 4 * i, s + 0x25 + 4 * i);
            let pos = self.r16(pp) as usize;
            if pos == 0 {
                continue;
            }
            let c = self.r8(cp).wrapping_sub(1);
            self.w8(cp, c);
            if (c as i8) >= 0 {
                continue;
            }
            self.w8(cp, 0);
            let mut pos = pos;
            let mut w = self.r16(pos);
            if w == 0x7FFE {
                pos = self.r16(pos + 2) as usize;
                self.w16(pp, pos as u16);
                w = self.r16(pos);
            }
            let (k, v) = ((w >> 8) as u8, w as u8);
            if k == 0x7E {
                if v < 0x80 {
                    self.w8(rp, v);
                } else if self.r8(s + 5) & 0x80 != 0 {
                    self.w8(cp, 0);
                    continue;
                }
                self.w16(pp, (pos + 2) as u16);
            } else if w == 0x7FFF {
                continue;
            } else {
                self.w8(s + (k & 0x7F) as usize + 1, v);
                self.w16(pp, (pos + 2) as u16);
            }
            let r = self.r8(rp);
            self.w8(cp, r);
        }
    }

    fn start(&mut self, sub: u16, hw: &mut Hw) {
        self.img = self.pristine.clone();
        self.chan = [None; 4];
        self.songend = false;
        self.started = true;
        self.sub = sub;
        self.start_song(0, hw);
        self.start_song(sub as usize + 1, hw);
    }
}

impl Replayer for Ab {
    fn format(&self) -> &'static str {
        "Jesper Olsen"
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
        if !self.started {
            self.start(self.sub, hw);
        }
        self.tick_ab(hw);
    }
}

// ---------------------------------------------------------------------------------

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let title = m.bus.read_cstr(m.deli.file_buf);
    if rw(d, 0) == 0x6000 {
        let lay = scan_c(d)?;
        // The reference leaves the LED filter off before the first sound (observed in its
        // host calls: the LED command goes on and off again within a few colour clocks of
        // each song start; the title music also writes CIA-A port A itself, in its log).
        if lay.kind == CKind::T {
            let pra = m.bus.ciaa.pra | 2;
            m.bus.write_byte(0xBFE001, pra);
        } else {
            m.bus.ciaa.pra |= 2;
        }
        return Some(Box::new(C::new(d, base, lay, title)));
    }
    let comp = companion_len(m);
    let eng = choose_engine(d, comp);
    let a = Ab {
        eng,
        pristine: d.to_vec(),
        img: d.to_vec(),
        base,
        title,
        nsongs: count_songs(d),
        sub: 0,
        chan: [None; 4],
        master: 63,
        pend_off: 0,
        pend_on: 0,
        alive: 0,
        alive0: 0,
        songend: false,
        started: false,
    };
    Some(Box::new(a))
}


// ---------------------------------------------------------------------------------
// Engine C: the module carries its code, which is not run; its tables are used

/// A byte counter fires when its decrement goes negative as a signed byte.
fn fires(c: &mut u8) -> bool {
    *c = c.wrapping_sub(1);
    *c >= 0x80
}

#[derive(Clone, Default)]
struct Cv {
    trk: usize,
    trans: u8,
    trans1: u8,
    cnt: u16,
    len: u16,
    ins: u16,
    cursor: u16,
    rep: u8,
    rest: bool,
    base: u8,
    note0: u8,
    note: u8,
    ivol: u8,
    vvol: u8,
    age: u16,
    smp_ptr: u16,
    note_ptr: u16,
    vol_ptr: u16,
    rel_ptr: u16,
    c_smp: u8,
    c_note: u8,
    c_vol: u8,
    c_rel: u8,
    mod_smp: u16,
    mod_note: u16,
    mod_vol: u16,
    pflags: u8,
    ptarget: u8,
    step: u16,
    pos: u16,
    lastper: u16,
    flagsw: u16,
    insmap: u16,
    /// Engine G: the sample the voice's registers hold.
    lc: u32,
    ln: u16,
}

struct C {
    d: Vec<u8>,
    base: u32,
    lay: CLayout,
    title: String,
    nsongs: u16,
    sub: u16,
    started: bool,
    songend: bool,
    // global state
    master: u16,
    tempo_hi: u8,
    tempo_lo: u8,
    tphase: u8,
    tcount: u8,
    active: u8,
    dma_on: u8,
    dma_off: u8,
    trans2: u8,
    /// Retune-only subsong of the in-game music: nothing plays.
    retune: bool,
    alive: u8,
    alive0: u8,
    v: [Cv; 4],
}

impl C {
    fn new(d: &[u8], base: u32, lay: CLayout, title: String) -> C {
        let stride = if lay.kind == CKind::T { 14 } else { 16 };
        let first = rw(d, lay.song_table) as usize;
        let n = (first.saturating_sub(lay.song_table) / stride).max(1) as u16;
        C {
            d: d.to_vec(),
            base,
            lay,
            title,
            nsongs: n,
            sub: 0,
            started: false,
            songend: false,
            master: 0,
            tempo_hi: 0,
            tempo_lo: 0,
            tphase: 0,
            tcount: 0,
            active: 0,
            dma_on: 0,
            dma_off: 0,
            trans2: 0,
            retune: false,
            alive: 0,
            alive0: 0,
            v: Default::default(),
        }
    }

    fn rb(&self, o: usize) -> u8 {
        rb(&self.d, o)
    }

    fn rw(&self, o: usize) -> u16 {
        rw(&self.d, o)
    }

    fn per_of(&self, n: u8) -> u16 {
        self.rw(self.lay.period_table + 2 * (n & 0x7F) as usize)
    }

    fn start(&mut self, hw: &mut Hw) {
        self.started = true;
        self.songend = false;
        let g = self.lay.kind == CKind::G;
        // Every song start leaves the LED filter off, as the reference does (see `new`).
        hw.bus.ciaa.pra |= 2;
        let stride = if g { 16 } else { 14 };
        let e = self.lay.song_table + stride * self.sub as usize;
        let (b8, b9, b10) = (self.rb(e + 8), self.rb(e + 9), self.rb(e + 10));
        self.v = Default::default();
        self.tcount = 0;
        self.tphase = 0;
        self.dma_on = 0;
        self.dma_off = 0;
        self.trans2 = 0;
        self.retune = false;
        self.alive = 0;
        if g {
            self.tempo_lo = b8 & 15;
            self.master = (b10 & 15) as u16;
            if b10 & 0x80 != 0 {
                self.retune = true;
                self.active = 0;
            } else {
                self.active = b9 & 15;
            }
        } else {
            self.tempo_lo = b8 & 15;
            self.tempo_hi = b8 >> 4;
            self.master = b10 as u16;
            self.active = b9 & 15;
        }
        if !self.retune {
            for i in 0..4 {
                let trk = self.rw(e + 2 * i) as usize;
                let v = &mut self.v[i];
                v.trk = trk;
                if g {
                    v.cnt = 1;
                    v.ins = 0x6000;
                    v.ln = 1;
                    v.lc = self.lay.silent as u32;
                } else {
                    v.trans = 0x0B;
                }
            }
            // a voice that starts is alive for the song end
            for i in 0..4 {
                let live = if g { self.active & (1 << i) != 0 } else { self.active & (1 << i) != 0 && self.v[i].trk != 0 };
                if live {
                    self.alive |= 1 << i;
                }
            }
        }
        self.alive0 = self.alive;
        if g {
            // the voices not in the song are stopped, then those in it
            hw.dmacon(!(b9 as u16) & 15);
            if !self.retune {
                hw.dmacon(self.active as u16);
            }
        }
    }

    fn voice_end(&mut self, i: usize) {
        self.alive &= !(1 << i);
        if self.alive == 0 && self.alive0 != 0 {
            self.songend = true;
            self.alive = self.alive0;
        }
    }

    fn lc_write(&self, hw: &mut Hw, i: usize, off: u32) {
        hw.loc(i, self.base.wrapping_add(off));
    }

    /// The item of a list at `*ptr`: 0x7FFF reads nothing, 0x7FFE jumps.
    fn item(&self, ptr: &mut u16) -> Option<usize> {
        let mut p = *ptr as usize;
        match self.rw(p) {
            0x7FFF => return None,
            0x7FFE => {
                p = self.rw(p + 2) as usize;
                *ptr = p as u16;
            }
            _ => {}
        }
        Some(p)
    }

    fn tick_c(&mut self, hw: &mut Hw) {
        if self.retune {
            if self.tcount == 1 {
                hw.dmacon(0);
            }
            if fires(&mut self.tcount) {
                self.tcount = self.tempo_lo;
            }
            hw.dmacon(0);
            return;
        }
        let g = self.lay.kind == CKind::G;
        if self.tcount == 1 {
            hw.dmacon(self.dma_off as u16);
            hw.wait(500);
        }
        if fires(&mut self.tcount) {
            if g {
                self.tcount = self.tempo_lo;
            } else if self.tphase == 0 {
                self.tcount = self.tempo_hi;
                self.tphase = 1;
            } else {
                self.tcount = self.tempo_lo;
                self.tphase = 0;
            }
        }
        self.dma_on = 0;
        self.dma_off = 0;
        for i in 0..4 {
            if self.active & (1 << i) == 0 {
                continue;
            }
            let bit = 1u8 << i;
            if self.tcount != 0 {
                let v = &self.v[i];
                let early = if g { v.cnt == 1 } else { v.cnt == 0 };
                if v.len >> 8 < 0x40 && early {
                    self.dma_off |= bit;
                }
                if !self.v[i].rest {
                    if g {
                        self.process_g(i, hw);
                    } else {
                        self.process_t(i, hw);
                    }
                }
            } else {
                let fire = if g {
                    self.v[i].cnt = self.v[i].cnt.wrapping_sub(1);
                    self.v[i].cnt == 0
                } else {
                    self.v[i].cnt = self.v[i].cnt.wrapping_sub(1);
                    self.v[i].cnt & 0x8000 != 0
                };
                if fire {
                    if g {
                        self.fetch_g(i, hw);
                    } else {
                        self.fetch_t(i, hw);
                    }
                } else if !self.v[i].rest {
                    if g {
                        self.process_g(i, hw);
                    } else {
                        self.process_t(i, hw);
                    }
                }
            }
            hw.wait(500);
        }
        hw.dmacon(0x8000 | self.dma_on as u16);
    }

    // ---- title version ----

    fn out_t(&mut self, i: usize, slide: u16, vib: u16, hw: &mut Hw) {
        let raw = self.per_of(self.v[i].note);
        let last = raw.wrapping_add(slide);
        self.v[i].lastper = last;
        hw.per(i, last.wrapping_sub(vib));
        let v = &self.v[i];
        let a = (v.ivol as u32 * v.vvol as u32) >> 4;
        let vol = (a * self.master as u32) >> 4;
        hw.vol(i, (vol & 0xFF) as u16);
    }

    fn fetch_t(&mut self, i: usize, hw: &mut Hw) {
        let bit = 1u8 << i;
        let mut guard = 0;
        loop {
            match self.rw(self.v[i].trk) {
                0x7FFF => {
                    self.active &= !bit;
                    self.voice_end(i);
                    return;
                }
                0x7FFE => {
                    self.v[i].trk = self.rw(self.v[i].trk + 2) as usize;
                    self.voice_end(i);
                    guard += 1;
                    if guard > 8 {
                        return;
                    }
                }
                _ => break,
            }
        }
        // prefix commands, each once, in this order
        let mut p = self.v[i].trk;
        let mut fetch_slide = 0u8;
        if self.rb(p) == 0xFF {
            self.v[i].rep = self.rb(p + 1);
            p += 2;
        }
        if self.rb(p) == 0xFE {
            let t = self.rb(p + 1).wrapping_add(0x0B);
            self.v[i].trans = t;
            fetch_slide = t;
            p += 2;
        }
        if self.rb(p) == 0xFD {
            self.master = (self.master & 0xFF00) | self.rb(p + 1) as u16;
            p += 2;
        }
        if self.rb(p) == 0xFC {
            let a = self.rb(p + 1);
            self.tempo_lo = a & 15;
            self.tempo_hi = a >> 4;
            p += 2;
        }
        if self.rw(p) >= 0xE000 {
            self.v[i].insmap = self.rw(p);
            p += 2;
        }
        self.v[i].trk = p;
        let r = self.rw(p) as i16 as isize;
        let mut q = (self.lay.pattern_base as isize + r + self.v[i].cursor as isize) as usize;
        let start = q;
        self.v[i].rest = false;
        let x = self.rw(q);
        if x >= 0x7001 {
            q += 2;
            self.v[i].cnt = x;
            self.lc_write(hw, i, self.lay.silent as u32);
            hw.len(i, 1);
            self.v[i].rest = true;
        } else {
            if x >= 0x6000 {
                let idx = (x.wrapping_add(self.v[i].insmap) & 0x0FFF) as usize;
                self.v[i].ins = self.rw(self.lay.insmap + 2 * idx);
                q += 2;
            }
            let rec = self.lay.ins_table + self.v[i].ins as usize;
            if self.v[i].len >> 8 < 0x40 {
                let a = be32(&self.d, rec);
                self.lc_write(hw, i, a);
                hw.len(i, self.rw(rec + 4));
                self.v[i].ivol = self.rb(rec + 18);
                self.v[i].vvol = self.rb(rec + 19);
                self.v[i].age = 0;
            }
            let w = self.rw(q);
            if w == 0x5003 {
                self.v[i].mod_smp = self.rw(q + 2);
                q += 4;
            }
            let w = self.rw(q);
            if w == 0x5002 {
                self.v[i].mod_vol = self.rw(q + 2);
                q += 4;
            }
            let w = self.rw(q);
            if w == 0x5001 {
                self.v[i].mod_note = self.rw(q + 2);
                q += 4;
            }
            let w = self.rw(q);
            if w >= 0x4001 {
                self.v[i].len = w;
                q += 2;
            }
            let w = self.rw(q);
            if w >= 0x3001 {
                self.v[i].len = w;
                q += 2;
            }
            if self.v[i].pflags & 2 == 0 {
                self.v[i].step = 0;
            }
            if self.v[i].pflags & 4 == 0 {
                self.v[i].pos = 0;
            }
            let w = self.rw(q);
            if w >= 0x2000 {
                self.v[i].pflags = (w >> 8) as u8;
                self.v[i].ptarget = w as u8;
                self.v[i].step = self.rw(q + 2);
                q += 4;
            }
            let w = self.rw(q);
            if w >= 0x1000 {
                self.v[i].flagsw = w;
                q += 2;
            }
            match self.rb(q) {
                0x02 => {
                    self.master = (self.master & 0xFF00) | self.rb(q + 1) as u16;
                    q += 2;
                }
                _ => {}
            }
            if self.rb(q) == 0x01 {
                self.v[i].ivol = self.rb(q + 1);
                q += 2;
            }
            let mut n = self.rb(q + 1);
            if n < 0x80 {
                n = n.wrapping_add(self.v[i].trans);
            }
            self.v[i].base = n & 0x7F;
            self.v[i].note = n & 0x7F;
            q += 2;
            self.v[i].cnt = self.v[i].len;
        }
        self.v[i].cnt = (self.v[i].cnt & 0x0FFF).wrapping_sub(1);
        self.dma_on |= bit;
        self.v[i].cursor = self.v[i].cursor.wrapping_add((q - start) as u16);
        // pattern end
        if self.rb(q + 1) == 0x7F {
            self.v[i].cursor = 0;
            let rep = self.v[i].rep.wrapping_sub(1);
            self.v[i].rep = rep;
            if rep & 0x80 != 0 {
                self.v[i].rep = 0;
                self.v[i].trk += 2;
            }
        }
        self.out_t(i, fetch_slide as u16, 0, hw);
    }

    fn process_t(&mut self, i: usize, hw: &mut Hw) {
        let rec = self.lay.ins_table + self.v[i].ins as usize;
        let mut slide = 0u16;
        let mut vib = 0u16;
        if self.v[i].age == 0 {
            let s = ((self.rw(rec + 6) as u32) << 16 | self.rw(rec + 8) as u32)
                .wrapping_add((self.v[i].mod_smp as u32) << 16 | self.v[i].mod_note as u32);
            let r = ((self.rw(rec + 10) as u32) << 16 | self.rw(rec + 12) as u32).wrapping_add((self.v[i].mod_vol as u32) << 16);
            let v = &mut self.v[i];
            v.smp_ptr = (s >> 16) as u16;
            v.note_ptr = s as u16;
            v.vol_ptr = (r >> 16) as u16;
            v.rel_ptr = r as u16;
            v.c_smp = 0;
            v.c_note = 0;
            v.c_vol = 0;
            v.c_rel = 0;
        } else {
            let mut reached = false;
            if self.v[i].ptarget & 0x80 == 0 {
                let t = self.v[i].ptarget.wrapping_add(self.v[i].trans);
                let tp = self.rw(self.lay.period_table + 2 * t as usize);
                let neg = t.wrapping_sub(self.v[i].base) >= 0x80;
                let lt = tp < self.v[i].lastper;
                if (neg && lt) || (!neg && !lt) {
                    reached = true;
                    let v = &mut self.v[i];
                    v.step = 0;
                    v.pos = 0;
                    v.base = t;
                    v.note = t;
                    v.pflags = 0;
                    v.ptarget = 0xFF;
                }
            }
            if !reached {
                let v = &mut self.v[i];
                v.pos = v.pos.wrapping_sub(v.step);
                slide = v.pos;
                let v = &self.v[i];
                let fl = v.flagsw as u8;
                if fl & 2 == 0 && v.pflags & 1 == 0 && (fl & 1 != 0 || v.age > self.rb(rec + 14) as u16) {
                    let mut rp = self.v[i].rel_ptr;
                    if let Some(p) = self.item(&mut rp) {
                        vib = self.rw(p);
                        let n = self.rb(rec + 16) & 15;
                        let v = &mut self.v[i];
                        v.rel_ptr = rp;
                        if v.c_rel == n {
                            v.c_rel = 0;
                            v.rel_ptr = v.rel_ptr.wrapping_add(2);
                        } else {
                            v.c_rel += 1;
                        }
                    } else {
                        self.v[i].rel_ptr = rp;
                    }
                }
            }
        }
        // volume list
        {
            if fires(&mut self.v[i].c_vol) {
                self.v[i].c_vol = self.rb(rec + 16) >> 4;
                let mut vp = self.v[i].vol_ptr;
                if let Some(p) = self.item(&mut vp) {
                    self.v[i].vol_ptr = vp;
                    self.v[i].vvol = self.rb(p + 1);
                    let hold = self.rb(p) & 1 != 0 && (self.rb(rec + 17) as u16) < self.v[i].cnt;
                    if !hold {
                        self.v[i].vol_ptr = vp.wrapping_add(2);
                    }
                } else {
                    self.v[i].vol_ptr = vp;
                }
            }
            if fires(&mut self.v[i].c_note) {
                self.v[i].c_note = self.rb(rec + 15) & 15;
                let mut np = self.v[i].note_ptr;
                if let Some(p) = self.item(&mut np) {
                    self.v[i].note = self.v[i].base.wrapping_add(self.rb(p + 1));
                    np = np.wrapping_add(2);
                }
                self.v[i].note_ptr = np;
            }
            if fires(&mut self.v[i].c_smp) {
                self.v[i].c_smp = self.rb(rec + 15) >> 4;
                let mut sp = self.v[i].smp_ptr;
                if let Some(p) = self.item(&mut sp) {
                    self.lc_write(hw, i, be32(&self.d, p));
                    hw.len(i, self.rw(p + 4));
                    sp = sp.wrapping_add(6);
                }
                self.v[i].smp_ptr = sp;
            }
        }
        self.v[i].age = self.v[i].age.wrapping_add(1);
        self.out_t(i, slide, vib, hw);
    }

    // ---- in-game version ----

    fn out_g(&mut self, i: usize, slide: u16, vib: u16, hw: &mut Hw) {
        let raw = self.per_of(self.v[i].note);
        let last = raw.wrapping_add(slide);
        self.v[i].lastper = last;
        hw.per(i, last.wrapping_sub(vib));
        let v = &self.v[i];
        let a = (v.ivol as u32 * v.vvol as u32) >> 4;
        let vol = (a * self.master as u32) >> 4;
        hw.vol(i, (vol & 0xFF) as u16);
        self.lc_write(hw, i, self.v[i].lc);
        hw.len(i, self.v[i].ln);
    }

    fn fetch_g(&mut self, i: usize, hw: &mut Hw) {
        let bit = 1u8 << i;
        let mut guard = 0;
        loop {
            match self.rw(self.v[i].trk) {
                0x7FFF => {
                    self.active &= !bit;
                    hw.vol(i, 0);
                    self.voice_end(i);
                    return;
                }
                0x7FFE => {
                    self.v[i].trk = self.rw(self.v[i].trk + 2) as usize;
                    self.voice_end(i);
                    guard += 1;
                    if guard > 8 {
                        return;
                    }
                }
                _ => break,
            }
        }
        let mut p = self.v[i].trk;
        if self.rb(p) == 0xFE {
            self.trans2 = self.rb(p + 1);
            p += 2;
        }
        if self.rb(p) == 0xFD {
            self.v[i].trans1 = self.rb(p + 1);
            p += 2;
        }
        if self.rb(p) == 0xFC {
            self.master = self.rb(p + 1) as u16;
            p += 2;
        }
        if self.rb(p) == 0xFB {
            self.tempo_lo = self.rb(p + 1) & 15;
            p += 2;
        }
        self.v[i].trk = p;
        let r = self.rw(p) as usize;
        let mut q = self.lay.pattern_base + r + self.v[i].cursor as usize;
        let start = q;
        self.v[i].rest = false;
        let x = self.rw(q);
        if x >= 0x7001 {
            q += 2;
            self.v[i].cnt = x;
            self.v[i].lc = self.lay.silent as u32;
            self.v[i].ln = 1;
            self.v[i].rest = true;
        } else {
            if x >= 0x6000 {
                self.v[i].ins = x;
                q += 2;
            }
            let rec = (self.lay.ins_table as isize + self.v[i].ins as isize - 0x6000) as usize;
            if self.v[i].len >> 8 < 0x40 {
                self.v[i].lc = be32(&self.d, rec);
                self.v[i].ln = self.rw(rec + 4);
                self.v[i].ivol = self.rb(rec + 18);
                self.v[i].vvol = self.rb(rec + 19) & 15;
                self.v[i].age = 0;
            }
            let w = self.rw(q);
            if w >= 0x4001 {
                self.v[i].len = w;
                q += 2;
            }
            let w = self.rw(q);
            if w >= 0x3001 {
                self.v[i].len = w;
                q += 2;
            }
            if self.v[i].pflags & 1 == 0 {
                self.v[i].step = 0;
            }
            if self.v[i].pflags & 2 == 0 {
                self.v[i].pos = 0;
                self.v[i].ptarget = 0;
            }
            let w = self.rw(q);
            if w >= 0x2000 {
                self.v[i].pflags = (w >> 8) as u8;
                self.v[i].ptarget = w as u8;
                self.v[i].step = self.rw(q + 2);
                q += 4;
            }
            let w = self.rw(q);
            if w >= 0x1000 {
                self.v[i].flagsw = w;
                q += 2;
            }
            if self.rb(q) == 0x01 {
                self.v[i].ivol = self.rb(q + 1);
                q += 2;
            }
            let mut n = self.rb(q + 1);
            if n < 0x80 {
                n = n.wrapping_add(self.v[i].trans1).wrapping_add(self.trans2);
            }
            n = n.wrapping_add(self.rb(rec + 17)) & 0x7F;
            self.v[i].note0 = n;
            self.v[i].note = n;
            q += 2;
            self.v[i].cnt = self.v[i].len;
        }
        self.v[i].cnt &= 0x0FFF;
        self.dma_on |= bit;
        self.v[i].cursor = self.v[i].cursor.wrapping_add((q - start) as u16);
        if self.rb(q + 1) == 0x7F {
            self.v[i].cursor = 0;
            self.v[i].trk += 2;
        }
        if self.v[i].rest {
            self.lc_write(hw, i, self.v[i].lc);
            hw.len(i, self.v[i].ln);
        } else {
            self.out_g(i, 0, 0, hw);
        }
    }

    fn process_g(&mut self, i: usize, hw: &mut Hw) {
        let rec = (self.lay.ins_table as isize + self.v[i].ins as isize - 0x6000) as usize;
        let mut slide = 0u16;
        let mut vib = 0u16;
        if self.v[i].age == 0 {
            let (s, n, vo, rl) = (self.rw(rec + 6), self.rw(rec + 8), self.rw(rec + 10), self.rw(rec + 12));
            let v = &mut self.v[i];
            v.smp_ptr = s;
            v.note_ptr = n;
            v.vol_ptr = vo;
            v.rel_ptr = rl;
            v.c_smp = 0;
            v.c_note = 0;
            v.c_vol = 0;
            v.c_rel = 0;
        } else {
            let b = self.v[i].ptarget;
            let mut reached = false;
            if b != 0 {
                let x = b.wrapping_add(self.v[i].note0);
                let tp = self.per_of(x);
                let d = tp.wrapping_sub(self.v[i].lastper) as i16;
                if (b < 0x80 && d >= 0) || (b >= 0x80 && d < 0) {
                    reached = true;
                    let v = &mut self.v[i];
                    v.note0 = x & 0x7F;
                    v.note = x & 0x7F;
                    v.pflags = 0;
                    v.ptarget = 0;
                    v.pos = 0;
                    v.step = 0;
                }
            }
            if !reached {
                let v = &mut self.v[i];
                v.pos = v.pos.wrapping_sub(v.step);
                slide = v.pos;
                let v = &self.v[i];
                let fl = v.flagsw as u8;
                if fl & 2 == 0 && v.pflags & 1 == 0 && (fl & 1 != 0 || v.age > self.rb(rec + 14) as u16) {
                    let mut rp = self.v[i].rel_ptr;
                    if let Some(p) = self.item(&mut rp) {
                        vib = self.rw(p);
                        let n = self.rb(rec + 16) & 15;
                        let v = &mut self.v[i];
                        v.rel_ptr = rp;
                        if v.c_rel == n {
                            v.c_rel = 0;
                            v.rel_ptr = v.rel_ptr.wrapping_add(2);
                        } else {
                            v.c_rel += 1;
                        }
                    } else {
                        self.v[i].rel_ptr = rp;
                    }
                }
            }
        }
        if fires(&mut self.v[i].c_vol) {
            self.v[i].c_vol = self.rb(rec + 16) >> 4;
            let mut vp = self.v[i].vol_ptr;
            if let Some(p) = self.item(&mut vp) {
                self.v[i].vvol = self.rb(p + 1);
                let first = self.rb(p);
                let adv = first & 0x80 == 0 || (self.v[i].len < 0x4000 && (first & 0x7F) as u16 >= self.v[i].cnt);
                if adv {
                    vp = vp.wrapping_add(2);
                }
            }
            self.v[i].vol_ptr = vp;
        }
        if fires(&mut self.v[i].c_note) {
            self.v[i].c_note = self.rb(rec + 15) & 15;
            let mut np = self.v[i].note_ptr;
            if let Some(p) = self.item(&mut np) {
                self.v[i].note = self.v[i].note0.wrapping_add(self.rb(p + 1));
                np = np.wrapping_add(2);
            }
            self.v[i].note_ptr = np;
        }
        if fires(&mut self.v[i].c_smp) {
            self.v[i].c_smp = self.rb(rec + 15) >> 4;
            let mut sp = self.v[i].smp_ptr;
            if let Some(p) = self.item(&mut sp) {
                self.v[i].lc = be32(&self.d, p);
                self.v[i].ln = self.rw(p + 4);
                sp = sp.wrapping_add(6);
            }
            self.v[i].smp_ptr = sp;
        }
        self.v[i].age = self.v[i].age.wrapping_add(1);
        self.out_g(i, slide, vib, hw);
    }
}

impl Replayer for C {
    fn format(&self) -> &'static str {
        "Jesper Olsen"
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
        if !self.started {
            self.start(hw);
        }
        self.tick_c(hw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn put32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// A data module (engines A and B): one voice on channel 0 that plays a single
    /// note (pitch 36, four steps) with a sample of 8 words and a loop of 4.
    fn data_module(speed: u8) -> Vec<u8> {
        let mut d = vec![0u8; 0x120];
        put16(&mut d, 0, 4);
        put16(&mut d, 2, 16);
        for (l, end) in [(4usize, 14usize), (16, 26)] {
            put16(&mut d, l, 0x40);
            put16(&mut d, l + 2, 0x90);
            put16(&mut d, l + 4, 0xD0);
            d[l + 6] = 0;
            d[l + 7] = speed;
            d[l + 8] = 1;
            d[l + 9] = 63;
            put16(&mut d, end, 0x7FFF);
        }
        put16(&mut d, 0x90, 0xA0);
        d[0xA0 + 12] = 63;
        put32(&mut d, 0xA0 + 14, 0x100);
        put16(&mut d, 0xA0 + 18, 8);
        put32(&mut d, 0xA0 + 20, 0x110);
        put16(&mut d, 0xA0 + 24, 4);
        put16(&mut d, 0xD0, 0xE0);
        put16(&mut d, 0xD2, 0x7FFF);
        d[0xE0] = 36;
        d[0xE1] = 4;
        d
    }

    #[test]
    fn recognises_data_modules() {
        let d = data_module(0xFF);
        assert!(detect(&d));
        assert_eq!(count_songs(&d), 1);
        // a channel beyond 3, an odd table size, a list that does not end, a song
        // pointer not preceded by the list terminator, a short file
        let mut e = d.clone();
        e[4 + 6] = 4;
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 0, 5);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 14, 0);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 2, 18);
        assert!(!detect(&e));
        assert!(!detect(&d[..0x50]));
        assert!(!detect(&[0u8; 0x200]));
    }

    #[test]
    fn engine_is_always_b() {
        let d = data_module(0xFF);
        assert_eq!(choose_engine(&d, Some(5860)), Eng::B);
        assert_eq!(choose_engine(&d, Some(6074)), Eng::B);
        assert_eq!(choose_engine(&data_module(0x22), None), Eng::B);
    }

    fn run(path: &Path, data: Vec<u8>, companion: Option<usize>, ticks: usize) -> Vec<(u32, u16)> {
        let mut e = Engine::new(48000.0);
        if let Some(n) = companion {
            e.m.os.mem_files.insert(path.parent().unwrap().join("WantedTeam.bin"), vec![0; n]);
        }
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(path, data).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Jesper Olsen");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect()
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn engine_b_plays_a_note() {
        let log = run(Path::new("/jo-test/jo.b"), data_module(0xFF), Some(6074), 6);
        // the note is fetched in the first tick, the sample installed in the second
        // and the loop in the third; the period is the table's for pitch 36
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [8, 4]);
        assert_eq!(reg(&log, 0xDFF0A6)[0], PERIODS[36]);
        assert_eq!(reg(&log, 0xDFF0A8)[0], 63);
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1] - lc[0], 0x10);
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x8001) && dma.contains(&0x0001));
    }

    #[test]
    fn engine_a_steps_by_its_two_phase_clock() {
        let log = run(Path::new("/jo-test/jo.a"), data_module(0x22), Some(5860), 8);
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [8, 4]);
        assert_eq!(reg(&log, 0xDFF0A6)[0], PERIODS[36]);
    }

    #[test]
    fn song_end_is_announced_when_the_track_ends() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new("/jo-test/jo.end"), data_module(0xFF)).unwrap();
        let mut out = vec![0f32; 2 * 960 * 12];
        assert!(!e.songend());
        e.render(&mut out);
        // the note lasts four steps, then the track's end word ends the voice
        assert!(e.songend());
    }

    /// An in-game-version module image (code version with stride 16): the branch
    /// table, an init routine with the signature instructions, a period table, a
    /// song table, one voice with a pattern of a single note.
    fn code_module() -> Vec<u8> {
        let mut d = vec![0u8; 0x800];
        for k in 0..8 {
            put16(&mut d, 4 * k, 0x6000);
            put16(&mut d, 4 * k + 2, 0x40);
        }
        put16(&mut d, 6, 0x5A);
        // init at 0x60: LEA song_table(PC),A1; ANDI.L #255,D0; MULU.W #16,D0
        put16(&mut d, 0x60, 0x43FA);
        put16(&mut d, 0x62, (0x500 - 0x62) as u16);
        d[0x64..0x6A].copy_from_slice(&[0x02, 0x80, 0x00, 0x00, 0x00, 0xFF]);
        put16(&mut d, 0x6A, 0xC0FC);
        put16(&mut d, 0x6C, 16);
        // LEA pattern(PC),A4; LEA silent(PC),A3; LEA (instruments - 0x6000)(PC),A3
        put16(&mut d, 0x70, 0x49FA);
        put16(&mut d, 0x72, (0x540 - 0x72) as u16);
        put16(&mut d, 0x74, 0x47FA);
        put16(&mut d, 0x76, (0x580 - 0x76) as u16);
        put16(&mut d, 0x78, 0x47FA);
        put16(&mut d, 0x7A, ((0x300i32 - 0x6000) - 0x7A) as u16);
        for (i, p) in PERIODS.iter().take(95).enumerate() {
            put16(&mut d, 0x400 + 2 * i, *p);
        }
        // song table: two songs; track lists follow it
        put16(&mut d, 0x500, 0x520);
        d[0x508] = 0; // tempo: a step per tick
        d[0x509] = 0x01; // voice 0
        d[0x50A] = 15; // master volume
        put16(&mut d, 0x510, 0x520);
        d[0x519] = 0x01;
        d[0x51A] = 15;
        put16(&mut d, 0x520, 0x0000);
        put16(&mut d, 0x522, 0x7FFF);
        // pattern: instrument 0, length 2, note 48, end marker
        put16(&mut d, 0x540, 0x6000);
        put16(&mut d, 0x542, 0x3002);
        put16(&mut d, 0x544, 0x0030);
        put16(&mut d, 0x546, 0x007F);
        // instrument 0: sample at 0x600 of 4 words, no lists, volume 0x30, vv 15
        put32(&mut d, 0x300, 0x600);
        put16(&mut d, 0x304, 4);
        for o in [6, 8, 10, 12] {
            put16(&mut d, 0x300 + o, 0x590);
        }
        d[0x300 + 18] = 0x30;
        d[0x300 + 19] = 0x0F;
        put16(&mut d, 0x590, 0x7FFF);
        d
    }

    #[test]
    fn locates_the_tables_of_a_code_module() {
        let d = code_module();
        assert!(detect(&d));
        let l = scan_c(&d).unwrap();
        assert_eq!(l.kind, CKind::G);
        assert_eq!((l.song_table, l.pattern_base, l.silent, l.ins_table, l.period_table), (0x500, 0x540, 0x580, 0x300, 0x400));
        // the multiplier, the period marker and the branches are checked
        let mut e = d.clone();
        put16(&mut e, 0x6C, 15);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 0x400, 0);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 4, 0x4E75);
        assert!(!detect(&e));
    }

    #[test]
    fn code_module_plays_a_note_and_ends() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/jo-test/jo.code"), code_module()).unwrap();
        let mut out = vec![0f32; 2 * 960 * 8];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        // the note: period of 48, volume (48 * 15 >> 4) * 15 >> 4, its sample of 4 words
        assert_eq!(reg(&log, 0xDFF0A6)[0], PERIODS[48]);
        assert_eq!(reg(&log, 0xDFF0A8)[0], 42);
        assert_eq!(reg(&log, 0xDFF0A4)[0], 4);
        // the voices not in the song are stopped, then the song's start
        assert_eq!(reg(&log, 0xDFF096)[..2], [0x000E, 0x0001]);
        assert!(reg(&log, 0xDFF096).contains(&0x8001));
        assert!(e.songend());
    }
}
