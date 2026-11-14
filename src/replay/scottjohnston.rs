//! Scott Johnston ("SoundPlayer", Eagleplayer "Scott Johnston", module prefix `SJ.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes and
//! cycle costs of the 68k reference player; no original code. A module is the 16-byte text
//! `Scott Johnston 1`, a 12-byte name, 16 instrument slots (one byte each, an index into the
//! player's table of 95 instrument file names), a little-endian timer word, a voice mask and rows of
//! four 3-byte cells (note, instrument, command). The sounds are IFF 8SVX files that are loaded from
//! the song's `Instruments` folder; only the one-shot and repeat lengths of VHDR and the start of
//! BODY are used. A row is read every six ticks; loops, row delays, fades, volume steps and
//! DMA-off commands are per voice or per track. Every register write is placed at the clock of the
//! original's code, from the specification's cycle ledger.
//!
//! The table of instrument file names (`scottjohnston_names.bin`, 95 entries of 12 bytes) is taken
//! from the original player on the project owner's decision; the specification gives it as data.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a module whose
//! slot byte names no entry of the table (above 95), whose first slot is 0, or one of whose files
//! is missing is not played; a note above 39 turns its voice's DMA off and starts nothing; an
//! instrument number above 38 reads the empty record; a VHDR or BODY not found in its search window
//! gives an empty record; cells beyond the end of the file read as zero (a song with no loop or stop
//! command thus ends up silent, repeating the empty cell); the song never ends.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Scott Johnston", detect, new };

const MAGIC: &[u8; 16] = b"Scott Johnston 1";
/// The player's table of 95 instrument file names, 12 bytes each.
const NAMES: &[u8] = include_bytes!("scottjohnston_names.bin");
/// The empty row's ledger and the periods of notes 1 to 39.
const PERIODS: [u16; 39] = [
    1076, 1016, 960, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302,
    285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];
/// Host helper costs (cycles before and after the write).
const H_LC: (u64, u64) = (84, 60);
const H_LEN: (u64, u64) = (116, 68);
const H_VOL: (u64, u64) = (84, 60);
const H_PER: (u64, u64) = (92, 60);
const H_DMA: (u64, u64) = (94, 68);
const H_FILTER: (u64, u64) = (74, 68);
/// Colour clock, from the run's origin, at which the player's initialisation writes its first DMACON
/// (the reference's depends on the size of the instrument files, 4 300 to 5 750).
const INIT_DMA_AT: u64 = 4_800;
/// Colour clock, from the run's origin, at which the song start call ends.
const START_END_AT: u64 = 38_800;
/// Colour clock in its raster line at which the reference's song start call ends (184 in four of
/// the five reference runs, 160 in the fifth); the DMA waits count lines from there.
const START_LINE_PHASE: u64 = 184;

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 0x2F && &d[..16] == MAGIC
}

/// One instrument record: where the sample starts, its length in words, the loop's start and length.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Inst {
    pub start: u32,
    pub len: u16,
    pub loop_at: u32,
    pub loop_len: u16,
}

/// The record made from 8SVX file `f` loaded at guest address `at`.
pub fn record_of(f: &[u8], at: u32) -> Inst {
    let find = |tag: &[u8; 4], limit: usize| (0..limit).find(|&p| f.get(p..p + 4) == Some(&tag[..]));
    let be32 = |o: usize| -> u32 {
        let g = |i: usize| byte(f, o + i) as u32;
        g(0) << 24 | g(1) << 16 | g(2) << 8 | g(3)
    };
    let (Some(vh), Some(body)) = (find(b"VHDR", 0x400), find(b"BODY", 0x800)) else {
        return Inst::default();
    };
    let one = be32(vh + 8);
    let rep = be32(vh + 12);
    let data = at.wrapping_add(body as u32 + 8);
    Inst {
        start: data,
        len: (one >> 1) as u16,
        loop_at: data.wrapping_add(one as u16 as i16 as i32 as u32),
        loop_len: (rep >> 1) as u16,
    }
}

/// A track: its row (the row pointer starts one before the first row), its loop and its delay.
#[derive(Clone, Copy)]
struct Track {
    row: i64,
    loop_save: i64,
    loop_count: u8,
    delay: u8,
}

impl Track {
    const START: Track = Track { row: -1, loop_save: -1, loop_count: 0xFF, delay: 0 };
}

#[derive(Clone, Copy)]
struct Voice {
    start: u32,
    pending: u16,
    period: u16,
    volume: u16,
    loop_at: u32,
    loop_len: u16,
    song1: bool,
    track: Track,
    note: u8,
    instr: u8,
    cmd: u8,
    hold: bool,
    fade_down: bool,
    fade_speed: u8,
    fade_count: u8,
    lock: bool,
}

impl Voice {
    const IDLE: Voice = Voice {
        start: 0,
        pending: 0,
        period: 0,
        volume: 0,
        loop_at: 0,
        loop_len: 0,
        song1: false,
        track: Track::START,
        note: 0,
        instr: 0,
        cmd: 0,
        hold: false,
        fade_down: false,
        fade_speed: 0,
        fade_count: 0,
        lock: false,
    };
}

/// The cycle clock: `b` is the base colour clock, `t` the cycles since it; `g` puts the raster line
/// grid of the reference host on it (a line starts where `(clock + g) % LINE` is 0).
struct Clk {
    b: u64,
    t: u64,
    g: u64,
}

impl Clk {
    fn stamp(&self) -> u64 {
        self.b + self.t / 2
    }

    /// A write through a host helper: its before-cost, the write at that moment, then the clock is
    /// re-based there and the after-cost added. A DMA-on write also waits for the audio DMA.
    fn helper(&mut self, hw: &mut Hw, cost: (u64, u64), wait: bool, f: impl FnOnce(&mut Hw)) {
        self.t += cost.0;
        let s = self.stamp();
        hw.bus.clock = hw.bus.clock.max(s);
        f(hw);
        self.b = s;
        self.t = 0;
        if wait {
            self.b = ((s + self.g) / LINE + 10) * LINE - self.g;
        }
        self.t += cost.1;
    }
}

pub struct ScottJohnston {
    d: Vec<u8>,
    insts: [Inst; 39],
    empty: u32,
    timer: u16,
    mask: u8,
    counter: u8,
    v: [Voice; 4],
    /// The `g` of the clocks (see `Clk`).
    grid: u64,
}

impl ScottJohnston {
    /// `insts[1..=16]` are the records of the 16 slots (index 0 stays empty).
    pub fn build(d: &[u8], insts: [Inst; 39], empty: u32) -> Option<Self> {
        if !detect(d) {
            return None;
        }
        let timer = if byte(d, 0x2C) == 0xFF { 0xFFFF } else { byte(d, 0x2C) as u16 | (byte(d, 0x2D) as u16) << 8 };
        Some(ScottJohnston {
            d: d.to_vec(),
            insts,
            empty,
            timer,
            mask: byte(d, 0x2E),
            counter: 0,
            v: [Voice::IDLE; 4],
            grid: 0,
        })
    }

    /// Cell `v` of the row of voice `v`'s track.
    fn cell(&self, v: usize) -> [u8; 3] {
        let t = &self.v[v];
        if t.track.row < 0 {
            return [0; 3];
        }
        if !t.song1 {
            // the built-in song: an empty row, then a row of song-loop commands
            return if t.track.row == 1 { [0, 0, 0xDE] } else { [0; 3] };
        }
        let o = 0x2Fusize + 12 * t.track.row as usize + 3 * v;
        [byte(&self.d, o), byte(&self.d, o + 1), byte(&self.d, o + 2)]
    }

    /// Player initialisation (spec 4.1): the DMA-off writes, one tick's loop refresh and DMA on.
    fn init(&mut self, hw: &mut Hw, origin: u64) {
        let start = origin + START_END_AT;
        self.grid = (START_LINE_PHASE + LINE - start % LINE) % LINE;
        let first = origin + INIT_DMA_AT;
        for i in 0..4u64 {
            hw.bus.clock = first + 129 * i;
            hw.dmacon(1 << i);
        }
        let mut c = Clk { b: first + 3 * 129 + 999 - 221, t: 0, g: self.grid };
        c.t += 246;
        for v in 0..4 {
            self.refresh(hw, &mut c, v);
        }
        c.b += 433;
        c.t = 0;
        hw.bus.clock = c.b;
        hw.dmacon(0x800F);
        self.counter = 5;
        hw.bus.clock = hw.bus.clock.max(origin + START_END_AT);
    }

    /// Song start (spec 4.2): no register is written.
    fn start_song(&mut self) {
        for v in 0..4 {
            if self.mask & 1 << v != 0 {
                self.v[v].song1 = true;
                self.v[v].track = Track::START;
                self.v[v].volume = 0;
                self.v[v].lock = false;
            }
        }
    }

    /// Loop refresh and fade of voice `v` (5.1).
    fn refresh(&mut self, hw: &mut Hw, c: &mut Clk, v: usize) {
        c.t += if v == 0 { 18 } else { 34 };
        c.t += 39;
        let (hold, loop_at, loop_len) = (self.v[v].hold, self.v[v].loop_at, self.v[v].loop_len);
        if hold {
            c.t += 16;
        } else if loop_len != 0 {
            c.t += 21 + 46;
            c.helper(hw, H_LC, false, |h| h.loc(v, loop_at));
            c.t += 30;
            c.helper(hw, H_LEN, false, |h| h.len(v, loop_len));
            c.t += 28;
        } else {
            c.t += 21 + 34;
            let e = self.empty;
            c.helper(hw, H_LC, false, |h| h.loc(v, e));
            c.t += 22;
            c.helper(hw, H_LEN, false, |h| h.len(v, 1));
            c.t += 28;
        }
        c.t += 21;
        let s = &mut self.v[v];
        if s.fade_speed == 0 {
            c.t += 16;
            return;
        }
        s.fade_count = s.fade_count.wrapping_sub(1);
        if s.fade_count != 0 {
            c.t += 37 + 16;
            return;
        }
        c.t += 37 + 41;
        s.fade_count = s.fade_speed;
        let limit;
        if s.fade_down {
            c.t += 21;
            limit = s.volume == 0;
            if !limit {
                s.volume -= 1;
            }
        } else {
            c.t += 25;
            limit = s.volume >= 63;
            if !limit {
                s.volume += 1;
            }
        }
        if limit {
            c.t += 32;
            s.fade_speed = 0;
        } else {
            c.t += 58;
            let vol = s.volume;
            c.helper(hw, H_VOL, false, |h| h.vol(v, vol));
            c.t += 38;
        }
    }

    /// Note start of voice `v` if one is pending (5.2).
    fn start_note(&mut self, hw: &mut Hw, c: &mut Clk, v: usize) {
        c.t += if v == 0 { 42 } else { 34 };
        c.t += 21;
        let s = self.v[v];
        if s.pending == 0 {
            c.t += 24;
            return;
        }
        c.t += 62;
        c.helper(hw, H_LC, false, |h| h.loc(v, s.start));
        c.t += 30;
        c.helper(hw, H_LEN, false, |h| h.len(v, s.pending));
        c.t += 30;
        c.helper(hw, H_VOL, false, |h| h.vol(v, s.volume));
        c.t += 30;
        c.helper(hw, H_PER, false, |h| h.per(v, s.period));
        c.t += 46;
        c.helper(hw, H_DMA, true, |h| h.dmacon(0x8000 | 1 << v));
        c.t += 72;
        self.v[v].pending = 0;
    }

    /// The row (5.4).
    fn row(&mut self, hw: &mut Hw, c: &mut Clk) {
        c.t += 18;
        for v in 0..4 {
            c.t += 63;
            let t = &mut self.v[v].track;
            let advance = if t.delay != 0 {
                t.delay -= 1;
                c.t += 37;
                if t.delay != 0 {
                    c.t += 16;
                    false
                } else {
                    c.t += 48;
                    true
                }
            } else {
                c.t += 48;
                true
            };
            if advance {
                t.row += 1;
            }
        }
        c.t += 34;
        for v in 0..4 {
            c.t += 67;
            if self.v[v].track.delay == 0 {
                c.t += 82;
                let [n, i, k] = self.cell(v);
                let s = &mut self.v[v];
                s.note = n;
                s.instr = i;
                s.cmd = k;
            }
            c.t += 16;
        }
        c.t += 66;
        for v in 0..4 {
            c.t += 47;
            let note = self.v[v].note;
            if note == 0 {
                c.t += 24;
                continue;
            }
            c.t += 30;
            c.helper(hw, H_DMA, false, |h| h.dmacon(1 << v));
            c.t += 248;
            let inst = self.insts.get(self.v[v].instr as usize).copied().unwrap_or_default();
            let s = &mut self.v[v];
            match PERIODS.get(note as usize - 1) {
                Some(&p) => {
                    s.period = p;
                    s.start = inst.start;
                    s.pending = inst.len;
                    s.loop_at = inst.loop_at;
                    s.loop_len = inst.loop_len;
                }
                // a note past the table (unspecified): nothing starts
                None => s.pending = 0,
            }
            s.note = 0;
        }
        c.t += 46;
        for v in 0..4 {
            c.t += 114;
            self.command(hw, c, v);
            self.v[v].cmd = 0;
        }
    }

    /// The command of voice `v` (6).
    fn command(&mut self, hw: &mut Hw, c: &mut Clk, v: usize) {
        let k = self.v[v].cmd;
        let s = &mut self.v[v];
        match k {
            0x01 | 0x02 => {
                c.t += 28;
                c.helper(hw, H_FILTER, false, |h| {
                    let x = h.bus.read_byte(0xBFE001);
                    // bit 1 set = LED (and filter) off
                    h.bus.write_byte(0xBFE001, if k == 1 { x | 2 } else { x & !2 });
                });
                c.t += 16;
            }
            0x03..=0x42 => {
                c.t += 31;
                if s.lock {
                    c.t += 16;
                } else {
                    c.t += 28;
                    s.volume = (k - 3) as u16;
                    hw.bus.clock = hw.bus.clock.max(c.stamp());
                    hw.vol(v, s.volume);
                    c.t += 16;
                }
            }
            0x43 => {
                c.t += 10;
                for j in 0..4 {
                    c.t += 23;
                    if j == v {
                        c.t += 34;
                        let val = if v == 0 { 0x0002 } else { 1 << v };
                        c.helper(hw, H_DMA, false, |h| h.dmacon(val));
                        c.t += 12;
                    }
                }
                c.t += 16;
            }
            0x57..=0x88 => {
                c.t += 39;
                if s.track.delay == 0 {
                    s.track.delay = k - 0x56;
                    c.t += 12;
                }
                c.t += 16;
            }
            0xA7..=0xBA => {
                c.t += 74;
                s.fade_down = k >= 0xB1;
                s.fade_speed = if s.fade_down { k - 0xB0 } else { k - 0xA6 };
                s.fade_count = s.fade_speed;
            }
            0xBB..=0xCE | 0xE5..=0xF8 => c.t += 68,
            0xCF | 0xD0 => {
                c.t += 42;
                s.hold = k == 0xCF;
            }
            0xD1 => c.t += 192,
            0xD2..=0xDB => {
                c.t += 35;
                if s.track.loop_count == 0xFF {
                    c.t += 48;
                    s.track.loop_save = s.track.row;
                    s.track.loop_count = k - 0xD1;
                }
                c.t += 16;
            }
            0xDC => {
                c.t += 31;
                if s.track.loop_count == 0 {
                    c.t += 32;
                    s.track.loop_count = 0xFF;
                } else {
                    c.t += 60;
                    s.track.loop_count = s.track.loop_count.wrapping_sub(1);
                    s.track.row = s.track.loop_save;
                }
            }
            0xDD => {
                c.t += 58;
                s.track.row -= 1;
            }
            0xDE => {
                c.t += 54;
                s.track.row = -1;
            }
            0xDF..=0xE4 | 0xF9..=0xFD => c.t += 26,
            _ => c.t += 16,
        }
    }
}

impl Replayer for ScottJohnston {
    fn format(&self) -> &'static str {
        "Scott Johnston"
    }

    fn title(&self) -> String {
        let name = self.d.get(0x10..0x1C).unwrap_or(&[]);
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        name[..end].iter().map(|&b| if (32..127).contains(&b) { b as char } else { '?' }).collect()
    }

    fn tick_clocks(&self) -> u64 {
        5 * (self.timer as u64 + 1)
    }

    fn songend(&self) -> bool {
        false
    }

    fn tick(&mut self, hw: &mut Hw) {
        let mut c = Clk { b: hw.bus.clock, t: 246, g: self.grid };
        for v in 0..4 {
            self.refresh(hw, &mut c, v);
        }
        c.t += 34;
        for v in 0..4 {
            self.start_note(hw, &mut c, v);
        }
        c.t += 108;
        if self.counter == 0 {
            self.counter = 5;
        } else {
            self.counter -= 1;
            if self.counter == 0 {
                self.row(hw, &mut c);
            }
        }
        hw.bus.clock = hw.bus.clock.max(c.stamp());
    }
}

/// The instrument records of the 16 slots, their files loaded into guest memory in slot order.
fn load_instruments(d: &[u8], m: &mut Machine) -> Option<[Inst; 39]> {
    let mut insts = [Inst::default(); 39];
    let mut last = Inst::default();
    for s in 0..16 {
        let k = byte(d, 0x1C + s) as usize;
        if k == 0 {
            if s == 0 {
                return None;
            }
        } else {
            let e = NAMES.get(12 * (k - 1)..12 * k)?;
            let end = e.iter().position(|&b| b == 0).unwrap_or(e.len());
            let name = std::str::from_utf8(&e[..end]).ok()?;
            let p = m.os.host_path(&format!("Instruments:{name}"), None);
            let f = m.os.read_file(&p).ok()?;
            log::debug!("Scott Johnston: slot {} {name} -> {} ({} bytes)", s + 1, p.display(), f.len());
            let at = m.alloc(f.len() as u32 + 16);
            if at == 0 {
                return None;
            }
            m.bus.write_bytes(at, &f);
            last = record_of(&f, at);
        }
        insts[s + 1] = last;
    }
    Some(insts)
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let insts = load_instruments(d, m)?;
    let empty = m.alloc(8);
    if empty == 0 {
        return None;
    }
    let mut r = ScottJohnston::build(d, insts, empty)?;
    // the reference's start-up counts from the run's origin, START_PHASE colour clocks before the
    // engine's aligned start; the first tick follows one period after the song start call
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    {
        let mut hw = Hw { bus: &mut m.bus };
        r.init(&mut hw, origin);
    }
    r.start_song();
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "scottjohnston_tests.rs"]
mod tests;
