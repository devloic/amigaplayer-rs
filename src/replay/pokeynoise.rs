//! Pokeynoise (Atari 8-bit SAP files of type B, M and C; Eagleplayer "POKEYNoise"): the music is
//! Atari 6502 code plus data. The player loads it into 64 KB of emulated Atari memory, runs its
//! init and play routines on a 6502 interpreter once per tick, then reads the four POKEY channel
//! register pairs back from that memory and turns each into one Paula voice that plays a waveform
//! of a fixed bank.
//!
//! Clean-room, written from a behavioural specification (header and loader, the interpreter with
//! its deviations, the POKEY to Paula conversion, the Paula write sequence and its timing). The
//! interpreter is in `pokeynoise_cpu.rs`.
//!
//! The waveform bank `pokeynoise_waves.bin` (48,908 bytes of 8-bit samples) is the player's own
//! data, taken from the original player on the project owner's decision; the specification gives
//! offsets and lengths into it and nothing that could generate it.
//!
//! Timing. The writes of a tick follow the specification's measured gaps. The start of the writes
//! after the 6502 run and the conversion is an estimate (the emulated instruction cost and a fixed
//! conversion time), as is VCOUNT; so the absolute times of a tick's writes are approximate while
//! the values and their order are exact.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a call that runs
//! more than a few million instructions fails (the original has no limit); a SAP segment header
//! cut off by the end of the file ends the loading; a song whose start call failed produces no
//! writes; the song-stop writes of the original host are not made (no hook for them); raw
//! binary-load files with an icon are not recognised (the detection sees no icon).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

#[path = "pokeynoise_cpu.rs"]
mod cpu;
use cpu::{Cpu, End};

pub const OPEN: Open = Open { name: "Pokeynoise", detect, new };

/// The waveform bank (section 7); taken from the original player on the project owner's decision.
static WAVES: &[u8] = include_bytes!("pokeynoise_waves.bin");

const LINE: u64 = 227;
/// Colour clocks from the tick's start to the first write beyond the 6502 run itself (the
/// conversion of the four channels, estimated).
const CONV: u64 = 3750;

// ------------------------------------------------------------------ the SAP file

struct Sap {
    kind: u8,
    init: u16,
    play: u16,
    music: u16,
    songs: u32,
    defsong: u32,
    /// Where the binary part begins.
    body: usize,
}

fn find(h: &[u8], key: &[u8]) -> Option<usize> {
    h.windows(key.len()).position(|w| w == key).map(|p| p + key.len())
}

/// The start of a keyword's value: spaces and tabs skipped.
fn value(h: &[u8], key: &[u8]) -> Option<usize> {
    let mut p = find(h, key)?;
    while p < h.len() && (h[p] == b' ' || h[p] == b'\t') {
        p += 1;
    }
    Some(p)
}

/// A hexadecimal number as the player reads it (section 1.2), up to a comma, NUL, LF or CR.
fn number(h: &[u8], mut p: usize) -> u32 {
    let mut n: u32 = 0;
    while p < h.len() && !matches!(h[p], b',' | 0 | b'\n' | b'\r') {
        let mut d = h[p].wrapping_sub(0x30);
        if d >= 0x31 {
            d -= 0x27;
        } else if d >= 0x11 {
            d -= 7;
        }
        n = (n << 4) | d as u32;
        p += 1;
    }
    n
}

fn parse(d: &[u8]) -> Option<Sap> {
    if d.len() < 3 || &d[..3] != b"SAP" || d[0] == 0xFF {
        return None;
    }
    let body = d.windows(2).position(|w| w == [0xFF, 0xFF])?;
    let h = &d[..body];
    let kind = *h.get(value(h, b"TYPE")?)?;
    let get = |k: &[u8]| value(h, k).map(|p| number(h, p));
    let (init, play, music) = match kind {
        b'B' | b'M' => (get(b"INIT")? as u16, get(b"PLAYER")? as u16, 0),
        b'C' => {
            let p = get(b"PLAYER")? as u16;
            (p.wrapping_add(3), p.wrapping_add(6), get(b"MUSIC")? as u16)
        }
        _ => return None,
    };
    Some(Sap {
        kind,
        init,
        play,
        music,
        songs: get(b"SONGS").unwrap_or(1),
        defsong: get(b"DEFSONG").unwrap_or(0),
        body,
    })
}

fn detect(d: &[u8]) -> bool {
    parse(d).is_some()
}

/// Loads the segments of the binary part (section 1.3).
fn load(mem: &mut [u8; 65536], d: &[u8], mut p: usize) {
    loop {
        if p + 2 <= d.len() && d[p] == 0xFF && d[p + 1] == 0xFF {
            p += 2;
        }
        if p + 4 > d.len() {
            return;
        }
        let start = d[p] as u16 | (d[p + 1] as u16) << 8;
        let end = d[p + 2] as u16 | (d[p + 3] as u16) << 8;
        p += 4;
        let n = if end == 0xFFFF {
            usize::MAX
        } else if end < start {
            1
        } else {
            (end - start) as usize + 1
        };
        let mut a = start;
        let mut done = 0;
        while done < n && p < d.len() {
            mem[a as usize] = d[p];
            a = a.wrapping_add(1);
            p += 1;
            done += 1;
        }
        if p >= d.len() {
            return;
        }
    }
}

// ------------------------------------------------------------------ conversion

/// What one channel becomes: waveform offset, length in words, period, volume (0..15).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Tuple {
    off: u32,
    len: u16,
    per: u16,
    vol: u16,
}

const SILENCE: Tuple = Tuple { off: 0, len: 4, per: 0x100, vol: 0 };

/// floor(x * k / 256); zero if the product reaches 2^32.
fn p(x: u64, k: u64) -> u64 {
    let m = x * k;
    if m >= 1 << 32 {
        0
    } else {
        m >> 8
    }
}

fn hi(x: u64) -> u64 {
    x.min(65535)
}

fn lo(x: u64) -> u64 {
    x.max(124)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Clk {
    A,
    B,
    C,
    D,
}

/// The clock mode and the joined flag of channel `k` for an AUDCTL value (section 4.3).
fn clock_mode(k: usize, ctl: u8) -> (Clk, bool) {
    let bit = |n: u8| ctl & (1 << n) != 0;
    let base = if bit(0) { Clk::A } else { Clk::B };
    let (mut m, joined) = match k {
        0 => (if bit(6) { if bit(4) { Clk::C } else { Clk::D } } else { base }, bit(7)),
        1 => (if bit(6) { Clk::C } else { base }, bit(4) || bit(7)),
        2 => (if bit(5) { if bit(3) { Clk::C } else { Clk::D } } else { base }, bit(7)),
        _ => (if bit(5) { Clk::C } else { base }, bit(3) || bit(7)),
    };
    if bit(7) {
        m = match m {
            Clk::A => Clk::C,
            Clk::B => Clk::D,
            o => o,
        };
    }
    (m, joined)
}

fn tuple(off: u32, len: u16, per: u64, vol: u16) -> Tuple {
    Tuple { off, len, per: per as u16, vol }
}

// ------------------------------------------------------------------ the replayer

#[derive(Clone, Copy, Default)]
struct Voice {
    off: u32,
    len: u16,
    per: u16,
    vol: u16,
    per_changed: bool,
    restart: bool,
}

pub struct Pokeynoise {
    cpu: Cpu,
    sap: Sap,
    bank: u32,
    v: [Voice; 4],
    started: bool,
    cur: u16,
}

impl Pokeynoise {
    /// A step of the generator, as the random silencing and RANDOM reads take it.
    fn rnd(&mut self) -> u32 {
        self.cpu.rng.step()
    }

    /// Starts subsong `n` (section 2): the init call(s) and the voice records.
    fn start(&mut self, n: u16, t: u64) {
        self.cur = n;
        let s = &self.sap;
        let ok = if s.kind == b'C' {
            let (m, init) = (s.music, s.init);
            self.cpu.call(init, 0x70, m as u8, (m >> 8) as u8, t) == End::Ok
                && self.cpu.call(init, 0, n as u8, 0, t) == End::Ok
        } else {
            self.cpu.call(s.init, n as u8, 0, 0, t) == End::Ok
        };
        self.started = ok;
        for v in self.v.iter_mut() {
            v.off = 0;
            v.len = 4;
        }
    }

    /// Converts channel `k` (section 4).
    fn convert(&mut self, k: usize) -> Tuple {
        let audf = self.cpu.r8(0xD200 + 2 * k as u16) as u64;
        let audc = self.cpu.r8(0xD201 + 2 * k as u16);
        let ctl = self.cpu.r8(0xD208);
        if audc & 0x10 != 0 {
            return SILENCE;
        }
        let vol = (audc & 15) as u16;
        let dist = audc >> 5;
        let (mode, joined) = clock_mode(k, ctl);
        let prev = self.cpu.r8((0xD200 + 2 * k as u16).wrapping_sub(2)) as u64;
        let m = if joined { audf * 256 + prev } else { audf };
        // f and c
        let (f26, c, f_other) = match mode {
            Clk::A => ((m + 1) * 29184 / 28, (m + 1) * 114, (m + 1) * 29184 / 28),
            Clk::B => ((m + 1) * 256, (m + 1) * 28, (m + 1) * 256),
            Clk::C => ((m + 7) * 256 / 28, m + 7, (m * 256 + 7) / 28),
            Clk::D => ((m + 4) * 256 / 28, m + 4, (m * 256 + 4) / 28),
        };
        let f = if dist == 2 || dist == 6 { f26 } else { f_other };
        // history
        let tag: u8 = match dist {
            0 => 0,
            1 | 3 => 1,
            2 => 2,
            4 => 4,
            5 | 7 => 5,
            _ => 6,
        };
        let (ta, fa, va) = (0xD260 + 2 * k as u16, 0xD240 + 2 * k as u16, 0xD250 + 2 * k as u16);
        let f16 = (f & 0xFFFF) as u16;
        let old_f = (self.cpu.r8(fa) as u16) << 8 | self.cpu.r8(fa + 1) as u16;
        let changed = self.cpu.r8(ta) != tag || old_f != f16;
        self.cpu.w8(ta, tag);
        self.cpu.w8(fa, (f16 >> 8) as u8);
        self.cpu.w8(fa + 1, f16 as u8);
        self.cpu.w8(va, 0);
        self.cpu.w8(va + 1, vol as u8);

        // the random silencing test (only where a case asks for it)
        let rsilent = |s: &mut Pokeynoise| -> bool { changed && (s.rnd() & 0xFF) < 0x50 };
        let mut t = match dist {
            0 => tuple(0x277E, 10247, lo(hi(p(f, 0x89))), vol),
            4 => tuple(0x778C, 9151, lo(hi(p(f, 0x84))), vol),
            5 | 7 => {
                if f <= 0x500 {
                    tuple(0x0008, 1, p(f, 0x38), vol)
                } else if f <= 0xC00 {
                    tuple(0x000A, 2, p(f / 5, 0x8C), vol)
                } else if f <= 0xC000 {
                    tuple(0x000E, 4, p(f / 10, 0x8C), vol)
                } else {
                    tuple(0x0016, 13, p(hi(2 * f / 65), 0x8C), vol)
                }
            }
            1 | 3 => {
                if f <= 0x280 {
                    tuple(0x01A4, 11, lo(p(f, 0x9E)), vol)
                } else if f <= 0xA00 {
                    tuple(0x0030, 31, p(2 * f / 5, 0x8C), vol)
                } else if f <= 0x2000 {
                    tuple(0x006E, 62, p(f / 5, 0x8C), vol)
                } else {
                    tuple(0x00EA, 93, p(hi(2 * f / 13), 0x8C), vol)
                }
            }
            6 => {
                let r = c % 15;
                match r {
                    0 => SILENCE,
                    5 | 10 => {
                        if rsilent(self) {
                            SILENCE
                        } else if f <= 0xA00 {
                            tuple(0x030A, 3, lo(p(2 * f / 5, 0x8C)), vol)
                        } else if f <= 0x2000 {
                            tuple(0x0310, 3, p(f / 5, 0x8C), vol)
                        } else {
                            tuple(0x0316, 6, hi(p(f / 10, 0x8C)), vol)
                        }
                    }
                    3 | 6 | 9 | 12 => {
                        if f <= 0xA00 {
                            tuple(0x0322, 5, lo(p(2 * f / 5, 0x8C)), vol)
                        } else if f <= 0x2000 {
                            tuple(0x032C, 5, p(f / 5, 0x8C), vol)
                        } else {
                            tuple(0x0336, 10, hi(p(f / 10, 0x8C)), vol)
                        }
                    }
                    _ => {
                        if f <= 0x280 {
                            tuple(0x01BA, 3, lo(p(f, 0x8C)), vol)
                        } else if f <= 0xA00 {
                            tuple(0x01C0, 15, p(2 * f / 5, 0x8C), vol)
                        } else if f <= 0x1800 {
                            tuple(0x01DE, 15, p(f / 5, 0x8C), vol)
                        } else if f <= 0x3800 {
                            tuple(0x01FC, 30, p(f / 10, 0x8C), vol)
                        } else {
                            tuple(0x0238, 105, hi(p(f / 35, 0x8C)), vol)
                        }
                    }
                }
            }
            _ => {
                // dist 2
                let r = c % 15;
                if c % 31 == 0 {
                    if rsilent(self) {
                        SILENCE
                    } else if f <= 0x280 {
                        tuple(0x1136, 5, lo(p(f, 0x8C)), vol)
                    } else {
                        tuple(0x1140, 10, hi(p(f / 10, 0x8C)), vol)
                    }
                } else if r == 0 {
                    SILENCE
                } else if r == 5 || r == 10 {
                    if rsilent(self) {
                        SILENCE
                    } else if f <= 0xA00 {
                        tuple(0x0B8E, 93, lo(p(2 * f / 5, 0x8C)), vol)
                    } else if f <= 0x2000 {
                        tuple(0x0C48, 93, p(f / 5, 0x8C), vol)
                    } else {
                        tuple(0x0D02, 186, hi(p(f / 10, 0x8C)), vol)
                    }
                } else if r % 3 == 0 {
                    if f <= 0x280 {
                        tuple(0x0E76, 42, lo(p(5 * f / 7, 0x90)), vol)
                    } else if f <= 0xA00 {
                        tuple(0x0ECA, 155, lo(p(2 * f / 5, 0x8C)), vol)
                    } else {
                        tuple(0x1000, 155, hi(p(f / 5, 0x8C)), vol)
                    }
                } else if f <= 0x280 {
                    tuple(0x0390, 93, lo(p(f, 0x8C)), vol)
                } else if f <= 0xA00 {
                    tuple(0x044A, 465, p(2 * f / 5, 0x8C), vol)
                } else {
                    tuple(0x07EC, 465, hi(p(f / 5, 0x8C)), vol)
                }
            }
        };
        // the high-tone mute (always on for SAP files)
        if matches!(dist, 4 | 5 | 7) && f < 0x2F0 {
            t.vol = 0;
        }
        t
    }

    fn paula_vol(v: u16) -> u16 {
        (v as u32 * 1093 / 256) as u16
    }
}

// ------------------------------------------------------------------ Paula writes

/// The writing of a tick: `t` is the colour clock of the next write.
struct Out<'a, 'b> {
    hw: &'a mut Hw<'b>,
    t: u64,
}

impl Out<'_, '_> {
    fn now(&mut self) {
        if self.t > self.hw.bus.clock {
            self.hw.bus.clock = self.t;
        }
    }
    fn per(&mut self, ch: usize, v: u16) {
        self.now();
        self.hw.per(ch, v);
    }
    fn vol(&mut self, ch: usize, v: u16) {
        self.now();
        self.hw.vol(ch, v);
    }
    fn len(&mut self, ch: usize, v: u16) {
        self.now();
        self.hw.len(ch, v);
    }
    fn loc(&mut self, ch: usize, a: u32) {
        self.now();
        self.hw.loc(ch, a);
    }
    fn dmacon(&mut self, v: u16) {
        self.now();
        self.hw.dmacon(v);
    }
    fn intreq(&mut self, v: u16) {
        self.now();
        self.hw.bus.write_word(0xDFF09C, v);
    }
    /// Waits for a new raster line and then past `pos` colour clocks of it (the polling of the
    /// original lands a little later; `pos` is the typical landing position).
    fn line_sync(&mut self, pos: u64) {
        self.t = (self.t / LINE + 1) * LINE + pos;
    }
    fn dma_on(&mut self, ch: usize) -> bool {
        self.now();
        let t = self.hw.bus.clock;
        self.hw.bus.paula.sync(t, &self.hw.bus.ram);
        self.hw.bus.paula.dmacon & (1 << ch) != 0
    }
    /// Polls the channel's interrupt flag, from the current time on; leaves `t` where it was
    /// seen (bounded).
    fn wait_flag(&mut self, ch: usize) {
        for _ in 0..8192 {
            self.now();
            let t = self.hw.bus.clock;
            self.hw.bus.paula.sync(t, &self.hw.bus.ram);
            if self.hw.bus.paula.intreq & (0x80 << ch) != 0 {
                return;
            }
            self.t += 15;
        }
    }
}

impl Pokeynoise {
    fn write_tick(&mut self, hw: &mut Hw, t0: u64) {
        let mut o = Out { hw, t: t0 };
        let bank = self.bank;
        for ch in 0..4 {
            let v = self.v[ch];
            let bit = 1u16 << ch;
            let vol = Self::paula_vol(v.vol);
            if v.restart {
                o.line_sync(40);
                o.per(ch, 1);
                o.t += 26;
                if o.dma_on(ch) {
                    o.intreq(bit << 7);
                    o.t += 6;
                    o.dmacon(bit);
                    o.wait_flag(ch);
                    o.t += 15;
                } else {
                    o.intreq(0);
                    o.t += 6;
                    o.dmacon(0);
                    o.t += 20;
                }
                o.loc(ch, bank + v.off);
                o.t += 6;
                o.len(ch, v.len);
                o.t += 12;
                o.per(ch, v.per);
                o.t += 10;
                o.vol(ch, vol);
                o.line_sync(64);
                o.intreq(bit << 7);
                o.t += 6;
                o.dmacon(0x8000 | bit);
                o.wait_flag(ch);
                o.line_sync(37);
                o.loc(ch, bank + v.off);
                o.t += 6;
                o.len(ch, v.len);
                o.t += 28;
                o.per(ch, v.per);
                o.t += 18;
                o.vol(ch, vol);
                self.v[ch].per_changed = false;
            } else {
                if v.per_changed {
                    o.per(ch, v.per);
                    o.t += 18;
                    self.v[ch].per_changed = false;
                }
                o.vol(ch, vol);
            }
            o.t += 41;
        }
    }
}

impl Replayer for Pokeynoise {
    fn format(&self) -> &'static str {
        "Pokeynoise"
    }
    fn title(&self) -> String {
        String::new()
    }
    fn tick(&mut self, hw: &mut Hw) {
        // the real-time clock of the Atari
        let m = &mut self.cpu.mem;
        m[0x14] = m[0x14].wrapping_add(1);
        if m[0x14] == 0 {
            m[0x13] = m[0x13].wrapping_add(1);
            if m[0x13] == 0 {
                m[0x12] = m[0x12].wrapping_add(1);
            }
        }
        if !self.started {
            return;
        }
        let t0 = hw.bus.clock;
        if self.cpu.call(self.sap.play, 0, 0, 0, t0) != End::Ok {
            return;
        }
        let start = t0 + self.cpu.cycles + CONV;
        for ch in 0..4 {
            let n = self.convert(ch);
            let v = &mut self.v[ch];
            v.restart = false;
            if n.off != v.off {
                v.restart = true;
                v.per_changed = true;
                v.off = n.off;
                v.len = n.len;
            }
            if n.per != v.per {
                v.per = n.per;
                v.per_changed = true;
            }
            v.vol = n.vol;
        }
        self.write_tick(hw, start);
    }
    fn songend(&self) -> bool {
        false
    }
    fn subsongs(&self) -> (u16, u16, u16) {
        let max = self.sap.songs.saturating_sub(1).min(65535) as u16;
        (0, max, self.cur)
    }
    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let t = hw.bus.clock;
        self.start(n, t);
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let sap = parse(d)?;
    let bank = m.alloc(WAVES.len() as u32);
    if bank == 0 {
        return None;
    }
    m.bus.ram[bank as usize..bank as usize + WAVES.len()].copy_from_slice(WAVES);
    let mut cpu = Cpu::new();
    load(&mut cpu.mem, d, sap.body);
    let def = sap.defsong.min(65535) as u16;
    let mut r = Pokeynoise {
        cpu,
        sap,
        bank,
        v: [Voice { len: 4, ..Voice::default() }; 4],
        started: false,
        cur: def,
    };
    let t = m.bus.clock;
    r.start(def, t);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "pokeynoise_tests.rs"]
mod tests;
