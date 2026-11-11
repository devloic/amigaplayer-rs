//! Quartet (Amiga; Dan Lennard, 1990; Eagleplayer "Quartet", module prefix `qpa.`, companion
//! sample set `SMP.*`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A module is a tempo byte, the mark 0x50, rows of four
//! 2-byte channel entries and an end mark of four words 0xFFFF; the 8-bit samples come from a
//! companion set file (`SMP.NAME`, else `SMP.set`) in the module's directory. Four voices, no
//! envelope, a portamento ("slide") that runs from a note to the next note of the channel. Every
//! register write is placed at the clock of the original's code, from the specification's cycle
//! ledger, including the two nine-line raster waits of each note start (modelled from the beam
//! phase) and the start-up calls' cost for the first tick.
//!
//! The Atari ST variants (Quartet ST, Quartet PSG) are different formats and are not covered.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): bytes asked for
//! beyond the end of the module read as zero (so a slide search that runs past the end finds the
//! end mark's bytes, then zeros); the `SMP.set` fallback costs the same start-up time as a companion
//! of the module's own name; a set whose samples lie partly outside it plays what the guest memory
//! holds there; the host's volume, balance and voice masks are not modelled (volume is the
//! module's volume); a song of one or two blocks does not restart after its end (the host stops).
//! The 4-byte zero buffer of the one-shot samples is a block the replayer allocates itself.
//! The period word of note value 0 (a word of the original player that the editor never writes)
//! is the constant `NOTE0_PERIOD`; notes 13 to 15 use the values of the specification.
//!
//! The colour clock of the first tick depends on the length of the module's directory path and
//! file name (the reference builds the companion's path character by character): the
//! specification's formula is used with the real strings the engine has, and `DIR_LEN` overrides
//! the directory length (the shipped reference logs were made with 90 and 113 characters).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The length of the directory part of the module path (with its trailing slash) used in the
/// first-tick formula; 0 takes the real one. A test and measurement knob.
pub static DIR_LEN: AtomicUsize = AtomicUsize::new(0);

pub const OPEN: Open = Open { name: "Quartet", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70_940;
const LINE: u64 = 227;
/// The period word the original uses for note value 0: it lies in the player, not in the module
/// (taken from the specification's observation of the reference's writes).
const NOTE0_PERIOD: u16 = 0x33C3;
/// Period table by note 1..15 (notes 13 to 15 are what the original reads past its table).
const PERIODS: [u16; 15] = [204, 216, 229, 242, 257, 272, 288, 305, 323, 343, 363, 384, 0x666C, 0x7279, 0x8088];
/// The sample set's bytes before the first sample.
const SET_HEAD: usize = 354;
/// Rows a slide search looks ahead, after the row of the note (rows R+2 ..= R+36).
const SEARCH: usize = 35;

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The period of note `n` (0..15).
fn table(n: usize) -> u16 {
    if n == 0 {
        NOTE0_PERIOD
    } else {
        PERIODS[n - 1]
    }
}

/// The number of zero words at the end of the file after the end mark, if the file is a module.
fn trailer(d: &[u8]) -> Option<usize> {
    let t = d.first().copied()?;
    if !(1..=30).contains(&t) || 3000 % t as u32 != 0 || d.get(1) != Some(&0x50) {
        return None;
    }
    let n = d.len() & !1;
    let mut z = 0;
    while z <= 15 && n >= 2 + 2 * z + 2 && be16(d, n - 2 - 2 * z) == 0 {
        z += 1;
    }
    // z zero words, then a word 0xFFFF and three more before it
    let end = n.checked_sub(2 * z + 8)?;
    if end < 2 || z > 15 {
        return None;
    }
    (0..4).all(|i| be16(d, end + 2 * i) == 0xFFFF).then_some(z)
}

fn detect(d: &[u8]) -> bool {
    trailer(d).is_some()
}

/// The sample set (spec 1.3): per sample the length in words, start offset and special length,
/// and the loop word.
struct Set {
    len: [u16; 16],
    start: [u16; 16],
    special: [u16; 16],
    loops: u16,
}

fn parse_set(z: &[u8]) -> Option<Set> {
    let n = z.len();
    if n <= 450 || n % 2 != 0 || &z[4..8] != b"lock" {
        return None;
    }
    let mut s = Set { len: [0; 16], start: [0; 16], special: [0; 16], loops: be16(z, 336) };
    let mut sum = 0usize;
    for i in 0..16 {
        s.len[i] = be16(z, n - 96 + 2 * i);
        s.start[i] = be16(z, n - 64 + 4 * i);
        s.special[i] = be16(z, n - 62 + 4 * i);
        if s.len[i] != 0 {
            sum += s.len[i] as usize + 2;
        }
    }
    (n + 2 == 450 + 2 * sum).then_some(s)
}

#[derive(Clone, Copy, Default)]
struct Chan {
    p: u16,
    slide: bool,
    base: i32,
    rem: i32,
    sec: i32,
    acc: i32,
    n: i32,
}

pub struct Quartet {
    d: Vec<u8>,
    tempo: u8,
    counter: u8,
    /// Offset of the current row.
    rp: usize,
    /// The song is longer than two blocks (it restarts at its end).
    long: bool,
    set: Set,
    /// Guest address of the first sample of the set, and of the zero buffer.
    samples: u32,
    zero: u32,
    /// Start of each sample relative to `samples`, and its length in words.
    addr: [u32; 16],
    slen: [u16; 16],
    ch: [Chan; 4],
    origin: u64,
    songend: bool,
}

/// Where a tick's writes go: the cycle count T from the tick's start gives the colour clock.
struct Clk {
    t0: u64,
    /// The tick's start relative to the run's start (the beam's reference).
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

    /// The nine-line wait (spec 5.3).
    fn wait(&mut self) {
        self.t += 46;
        for _ in 0..9 {
            self.t += 16;
            let l0 = self.line();
            // bounded: the beam moves on within a line
            for _ in 0..4096 {
                self.t += 16;
                let l = self.line();
                self.t += 9;
                if l != l0 {
                    break;
                }
            }
            self.t += 10;
        }
        self.t += 44;
    }
}

impl Quartet {
    fn word(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// Spec 4.1 step 3, after the end check: the row at `rp`, from the cost 24 on.
    fn play_row(&mut self, hw: &mut Hw, clk: &mut Clk) {
        clk.t += 24;
        for c in 0..4 {
            clk.t += if c == 0 { 19 } else { 23 };
            let b0 = byte(&self.d, self.rp + 2 * c);
            let b1 = byte(&self.d, self.rp + 2 * c + 1);
            if b0 != 0 {
                clk.t += 25;
                if b1 != 0xFE {
                    self.note(hw, clk, c, b0, b1);
                    clk.t += 25;
                    if byte(&self.d, self.rp + 8 + 2 * c + 1) == 0xFE {
                        clk.t += 18;
                        self.slide_setup(clk, c);
                    }
                }
            }
        }
        clk.t += 44;
        self.rp += 8;
    }

    /// Spec 4.2: the note start of channel `c`.
    fn note(&mut self, hw: &mut Hw, clk: &mut Clk, c: usize, b0: u8, b1: u8) {
        let v = (b0 & 0x3F) as u16 + 1;
        let o = b0 >> 6;
        let s = (b1 >> 4) as usize;
        let n = (b1 & 15) as usize;
        let looping = self.set.loops >> s & 1 != 0;
        let special = self.set.special[s];
        let addr = self.samples.wrapping_add(self.addr[s]).wrapping_add(self.set.start[s] as u32);
        let len = if special != 0 { special } else { self.slen[s] };
        let t = table(n);
        let p = match o {
            2 | 3 => t >> 1,
            0 => t << 1,
            _ => t,
        };
        self.ch[c].slide = false;
        self.ch[c].p = p;
        clk.t += 20;
        clk.at(hw);
        hw.dmacon(1 << c);
        clk.t += 36 + 320 + if c > 0 { 16 } else { 0 } + if o == 0 || o >= 2 { 8 } else { 0 } + 149;
        clk.t += if special != 0 { 16 } else { 0 } + 65 + if looping { 72 } else { 64 };
        clk.t += 32;
        clk.at(hw);
        hw.loc(c, addr);
        clk.t += 24;
        clk.at(hw);
        hw.len(c, len);
        clk.t += 24;
        clk.at(hw);
        hw.per(c, p);
        clk.t += 120;
        clk.at(hw);
        hw.vol(c, v);
        clk.t += 80;
        clk.wait();
        clk.t += 20;
        clk.at(hw);
        hw.dmacon(0x8000 | 1 << c);
        clk.wait();
        let (la, ll) = if looping { (addr, len) } else { (self.zero, 1) };
        clk.t += 32;
        clk.at(hw);
        hw.loc(c, la);
        clk.t += 24;
        clk.at(hw);
        hw.len(c, ll);
    }

    /// Spec 4.4 and 5.4: the slide set-up of channel `c`; the note's row is at `rp`.
    fn slide_setup(&mut self, clk: &mut Clk, c: usize) {
        clk.t += 34;
        let mut found = None;
        for k in 2..2 + SEARCH {
            clk.t += 37;
            let o = self.rp + 8 * k + 2 * c;
            if byte(&self.d, o) & 0x3F != 0 {
                found = Some((k, byte(&self.d, o), byte(&self.d, o + 1)));
                break;
            }
            clk.t += 17;
        }
        let end = if c == 3 { 11 } else { 9 };
        let Some((k, b0, b1)) = found else {
            clk.t += 24 + 4 + end + 16;
            return;
        };
        clk.t += 99;
        let o = b0 >> 6;
        let t = table((b1 & 15) as usize);
        let target = match o {
            2 => {
                clk.t += 19 + 18;
                t >> 1
            }
            0 => {
                clk.t += 19 + 8;
                t << 1
            }
            _ => t,
        };
        clk.t += 82 + 4 + end + 65;
        let ch = &mut self.ch[c];
        let n = k as i32 * self.tempo as i32;
        let d = target.wrapping_sub(ch.p) as i16 as i32;
        let base = d / n;
        let r = d - base * n;
        let sec = if (target as i16) > (ch.p as i16) { 1 } else { -1 };
        if sec < 0 {
            clk.t += 20;
        }
        clk.t += 195;
        if r < 0 {
            clk.t += 20;
        }
        clk.t += 52;
        *ch = Chan { slide: true, base, rem: r.abs(), sec, acc: 0, n, ..*ch };
    }

    fn run(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        let mut clk = Clk { t0, rel: t0.saturating_sub(self.origin), t: 250 };
        // slides (spec 4.4)
        clk.t += 25;
        if self.ch.iter().any(|c| c.slide) {
            clk.t += 18;
            for c in 0..4 {
                clk.t += 29;
                if self.ch[c].slide {
                    clk.t += 18 + 101;
                    let ch = &mut self.ch[c];
                    ch.p = ch.p.wrapping_add(ch.base as u16);
                    ch.acc += ch.rem;
                    if ch.acc as i16 >= ch.n as i16 {
                        clk.t += 44;
                        ch.p = ch.p.wrapping_add(ch.sec as u16);
                        ch.acc -= ch.n;
                    }
                    clk.t += 40;
                    clk.at(hw);
                    hw.per(c, ch.p);
                    clk.t += 64;
                }
            }
            clk.t += 16;
        }
        clk.t += 29;
        self.counter = self.counter.wrapping_sub(1);
        if self.counter != 0 {
            return;
        }
        self.counter = self.tempo;
        clk.t += 16;
        for i in 0..4 {
            clk.t += if i == 0 { 23 } else { 27 };
            if self.word(self.rp + 2 * i) == 0xFFFF {
                self.song_end(hw, &mut clk);
                return;
            }
        }
        self.play_row(hw, &mut clk);
    }

    /// Spec 4.5.
    fn song_end(&mut self, _hw: &mut Hw, clk: &mut Clk) {
        self.songend = true;
        clk.t += 90;
        clk.t -= clk.t % 2;
        clk.t += 89;
        if self.long {
            clk.t += 58;
            self.rp = 2;
            self.counter = self.tempo;
            self.play_row(_hw, clk);
        } else {
            // the counter stays 0: the next check is 256 ticks away
            self.counter = 0;
            clk.t += 16;
        }
    }
}

impl Replayer for Quartet {
    fn format(&self) -> &'static str {
        "Quartet"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.run(hw);
    }
}

/// The song's size scan (spec 1.4): the longs scanned (with the end one) and the song size.
fn song_scan(d: &[u8]) -> (u64, usize) {
    let mut o = 2;
    let mut l = 0;
    // bounded by the file: the detection guarantees an end mark
    while o + 4 <= d.len() {
        l += 1;
        if be32(d, o) == 0xFFFF_FFFF {
            return (l, o + 4 + 4);
        }
        o += 4;
    }
    (l, o + 4)
}

/// Cycles of the first-tick calls, each halved and rounded down as the host does (spec 5.5).
fn start_clocks(d: &[u8], z: usize, set: &Set, l: u64, dir: usize, name: &str, fallback: bool) -> u64 {
    let b = name.as_bytes();
    // the cost of each lower-case letter of the prefix (an upper-case one costs nothing here)
    let x = [(b'q', 21u64), (b'p', 25), (b'a', 25)]
        .iter()
        .enumerate()
        .map(|(i, &(ch, cost))| match b.get(i) {
            Some(&c) if c == ch => cost,
            _ => 0,
        })
        .sum::<u64>();
    let _ = d;
    let zero = set.len.iter().filter(|&&n| n == 0).count() as u64;
    let nonzero = 16 - zero;
    let short = set.len.iter().filter(|&&n| n <= 50).count() as u64;
    let long = 16 - short;
    let f = name.len() as u64;
    let dd = dir as u64;
    let calls = [
        388 + 29 * z as u64,
        60,
        16 + 18 + 16 + 17 * (dd + 1) + 28 + 21 * (f + 1) + 96 + x + 78 + 32,
        45,
        36,
        574 + 23 * l,
        394 + 27 * zero + 51 * nonzero + 86 * short + 88 * long,
        16,
        236,
        236,
        1338,
    ];
    // (measured: a bank found only as SMP.set, after the failed open of SMP.<name>, costs one
    // more host call, which scans the directory path again: 17 cycles per character)
    let fb = if fallback { (17 * dd + 351) / 2 } else { 0 };
    fb + calls.iter().map(|c| c / 2).sum::<u64>()
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let z = trailer(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let name = file.rsplit('/').next().unwrap_or(&file).to_string();
    let nb = name.as_bytes();
    if nb.len() < 4 || !nb[..3].eq_ignore_ascii_case(b"qpa") || nb[3] != b'.' {
        return None;
    }
    // the companion: the first three characters become SMP, else SMP.set (spec 1.3)
    let rest = &name[3..];
    let cands = [format!("SMP{rest}"), format!("smp{rest}"), "SMP.set".to_string(), "smp.set".to_string()];
    let (zdata, set, cand) = cands.iter().enumerate().find_map(|(i, n)| {
        let z = crate::deli::read_companion(m, n).1.ok()?;
        let s = parse_set(&z)?;
        Some((z, s, i))
    })?;
    let set_addr = m.alloc((zdata.len() as u32 + 8).max(8));
    let zero = m.alloc(16);
    if set_addr == 0 || zero == 0 {
        return None;
    }
    m.bus.write_bytes(set_addr, &zdata);
    // sample addresses (spec 1.3)
    let mut addr = [0u32; 16];
    let mut slen = [0u16; 16];
    let mut a = 0u32;
    for i in 0..16 {
        addr[i] = a;
        if set.len[i] > 50 {
            slen[i] = set.len[i];
            a = a.wrapping_add((set.len[i] as u32 + 2) * 2);
        } else {
            slen[i] = 50;
        }
    }
    let (l, size) = song_scan(d);
    let blocks = (size.saturating_sub(2) >> 8) + 1;
    let dir = match DIR_LEN.load(Ordering::Relaxed) {
        0 => {
            let dir = m.deli.module_dir.to_string_lossy();
            dir.len() + usize::from(!dir.ends_with('/'))
        }
        n => n,
    };
    let sum = start_clocks(d, z, &set, l, dir, &name, cand >= 2);
    // the reference's start-up counts from the run's origin, START_PHASE colour clocks before the
    // engine's aligned start; the first tick follows one period after the clock left here
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    m.bus.clock = (origin + 100 + sum).max(m.bus.clock);
    let tempo = d[0];
    Some(Box::new(Quartet {
        d: d.to_vec(),
        tempo,
        counter: tempo,
        rp: 2,
        long: blocks > 2,
        set,
        samples: set_addr + SET_HEAD as u32,
        zero,
        addr,
        slen,
        ch: [Chan { p: 288, ..Chan::default() }; 4],
        origin,
        songend: false,
    }))
}

#[cfg(test)]
#[path = "quartet_tests.rs"]
mod tests;
