//! Soundtracker Pro II (`STP3`), playing on Paula
//! as the Eagleplayer "SoundTracker Pro II" does.
//!
//! The file layout is that of libopenmpt's `soundlib/Load_stp.cpp` (BSD-3-Clause, Copyright (c)
//! 2004-2026 OpenMPT Project Developers and Contributors, Copyright (c) 1997-2003 Olivier
//! Lapicque; Devin Acker and the OpenMPT devs wrote the loader): the 204-byte header, the sample
//! chunks of versions 0, 1 and 2, the patterns of 4-byte cells, the scripts and the drum pad
//! that follow them, and the sample data. See `LICENSE-openmpt`, which has to travel with this
//! file. The command numbers and what OpenMPT makes of them (its `Load_stp.cpp` switch, with the
//! "auto" slides of its `Snd_fx.cpp`) are the vocabulary of the replay below; OpenMPT mixes the
//! module in software and converts the STP commands to its own, so what the voices write to Paula
//! is not in it. That is taken from the Eagleplayer's register writes (`DELI_WRITE_LOG`) and from
//! black-box runs of it on small modules built for the purpose: no original code or tables are
//! used.
//!
//! What the Eagleplayer does, as the runs showed it (each point checked against it):
//! - The header's speed, fraction and CIA timer are not read: the song starts at speed 6 and the
//!   default 50 Hz timer, and the first row plays at the sixth interrupt (the five before it only
//!   write volume). `0Fxy` sets the speed to x and the fraction to y/4 tick (the timer of every
//!   tick is then scaled by (4x+y)/4x), `4Fxx` is the speed for xx below 0x20 and a tempo
//!   (`1773447 / xx`, the CIA timer value) from there on. The 12-bit CIA tempo commands (`Fxxx`
//!   in OpenMPT's reading) do nothing.
//! - Four channels, Paula direct. A note is: the old period, DMA off, length, address, the
//!   new period, volume; the sample's loop is written after the DMA on, ten raster lines later
//!   (address plus loop start, length the loop's words, or 0 when it is under 2 words).
//! - The period table is its own (83 notes, 3232 down to 28), and the commands the player knows
//!   are `00` (arpeggio), `01`/`02` (slides), `0B` (volume slide each row), `0C` (volume),
//!   `0F`, `10` (vibrato), `12`, `13` (tone portamento), `14`, `4E` (fine slides), `4F`. The
//!   others OpenMPT converts (`03`..`09`, `0A`, `0D`, `0E`, `11`, `16`..`22`, `49`) do nothing
//!   in it.
//! - Only version 2 files are accepted by the Eagleplayer; versions 0 and 1 (and their loop
//!   lists, whose commands 16..19 it does not know either) are read as OpenMPT reads them and
//!   played the same way.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Soundtracker Pro II", detect: |d| Song::parse(d).is_some(), new };

/// Periods of notes 1 to 83 (the table's own values; note 84 and up read past it).
const PERIODS: [u16; 83] = [
    3232, 3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1812, 1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016,
    960, 904, 856, 808, 760, 720, 680, 640, 604, 572, 540, 508, 480, 452, 428, 404, 380, 360, 340, 320, 302, 286, 270, 254, 240,
    226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 107, 101, 95, 90, 84, 80, 75, 71, 67, 63, 60, 56, 53, 50,
    47, 45, 42, 40, 37, 35, 33, 31, 30, 28,
];

/// The vibrato's sine (ProTracker's).
const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141,
    120, 97, 74, 49, 24,
];

const DEFAULT_TIMER: u32 = crate::deli::DEFAULT_TIMER as u32;
/// The CIA timer value of a tempo: this over the tempo (709379 Hz * 2.5).
const TEMPO_CONST: u32 = 1_773_447;
const MIN_PERIOD: i32 = 14;
const MAX_PERIOD: i32 = 3424;

#[derive(Clone, Copy, Default)]
struct Cell {
    inst: u8,
    note: u8,
    cmd: u8,
    param: u8,
}

struct Pattern {
    rows: usize,
    cells: Vec<Cell>,
}

#[derive(Clone, Copy, Default)]
struct Smp {
    /// Offset of the data in the file.
    off: u32,
    len: u32,
    vol: u8,
    loop_start: u32,
    loop_len: u32,
}

struct Song {
    orders: Vec<u8>,
    pat_len: usize,
    patterns: Vec<Option<Pattern>>,
    /// Indexed by sample number (0 is unused).
    samples: Vec<Smp>,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let v = u16::from_be_bytes(self.d.get(self.p..self.p + 2)?.try_into().ok()?);
        self.p += 2;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_be_bytes(self.d.get(self.p..self.p + 4)?.try_into().ok()?);
        self.p += 4;
        Some(v)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.p = self.p.checked_add(n)?;
        (self.p <= self.d.len()).then_some(())
    }
}

impl Song {
    /// Reads a module as `ReadSTP` does, for the four-channel ones the Amiga plays.
    fn parse(d: &[u8]) -> Option<Song> {
        if d.len() < 204 || &d[..4] != b"STP3" {
            return None;
        }
        let mut r = Reader { d, p: 4 };
        let version = r.u16()?;
        let num_orders = r.u8()? as usize;
        let pat_len = r.u8()? as usize;
        let order_list = d.get(8..136)?;
        r.p = 136;
        let (_speed, _frac, timer, _flags, _res) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?, r.u32()?);
        let midi_count = r.u16()?;
        r.skip(50)?;
        let num_samples = r.u16()? as usize;
        let struct_size = r.u16()? as usize;
        if version > 2 || num_orders > 128 || num_orders == 0 || num_samples >= 4000 || timer == 0 || midi_count != 50 {
            return None;
        }
        let mut samples = vec![Smp::default()];
        for _ in 0..num_samples {
            let idx = r.u16()? as usize;
            if idx == 0 || idx >= 4000 {
                return None;
            }
            let size = if version == 2 { (r.u32()? as usize).checked_sub(2)? } else { struct_size };
            let chunk = d.get(r.p..r.p.checked_add(size)?)?;
            r.p += size;
            // path, flags, name (and for version 2 padding to an even offset), then 20 bytes
            let at = if version < 2 {
                62
            } else {
                let path_end = chunk.iter().take(257).position(|&b| b == 0)?;
                let name_start = path_end + 2;
                let name_end = name_start + chunk.get(name_start..)?.iter().position(|&b| b == 0)?;
                let mut q = name_end + 1;
                if q % 2 == 1 {
                    q += 1;
                }
                q
            };
            let h = chunk.get(at..).unwrap_or(&[]);
            let mut sr = Reader { d: h, p: 0 };
            let len = sr.u32().unwrap_or(0);
            let vol = sr.u8().unwrap_or(0);
            sr.skip(1);
            let loop_start = sr.u32().unwrap_or(0);
            let loop_len = sr.u32().unwrap_or(0);
            if samples.len() <= idx {
                samples.resize(idx + 1, Smp::default());
            }
            samples[idx] = Smp { off: 0, len, vol: vol.min(64), loop_start, loop_len };
            if version >= 1 {
                // the loop list (OpenMPT cues and concatenates them for its mixer; Paula's
                // loop is the header's)
                let n = r.u16()? as usize;
                r.skip(n.checked_mul(8)?)?;
            }
        }
        let mut num_patterns = 128usize;
        if version == 0 {
            num_patterns = r.u16()? as usize;
        }
        let mut patterns: Vec<Option<Pattern>> = Vec::new();
        let mut len = pat_len;
        for pat in 0..num_patterns {
            let mut index = pat;
            let mut channels = 4;
            if version > 0 {
                index = r.u16()? as usize;
                if index == 0xFFFF {
                    break;
                }
                len = r.u16()? as usize;
                channels = r.u16()? as usize;
            }
            if channels != 4 || index >= 4000 {
                return None;
            }
            let bytes = len.checked_mul(channels)?.checked_mul(4)?;
            let Some(data) = d.get(r.p..r.p + bytes) else { break };
            r.p += bytes;
            if patterns.len() <= index {
                patterns.resize_with(index + 1, || None);
            }
            let cells = data.chunks_exact(4).map(|c| Cell { inst: c[0], note: c[1], cmd: c[2], param: c[3] }).collect();
            patterns[index] = Some(Pattern { rows: len, cells });
        }
        if version > 0 {
            // scripts, then the drum pad
            while r.p + 2 <= d.len() {
                let num = r.u16()?;
                if num == 0xFFFF {
                    break;
                }
                r.skip(2)?;
                let n = r.u32()? as usize;
                r.skip(n)?;
            }
            r.p = (r.p + 34).min(d.len());
        }
        // the sample data, in sample order
        let mut off = r.p as u32;
        for s in samples.iter_mut().skip(1) {
            if s.len > 0 {
                s.off = off;
                off = off.saturating_add(s.len);
            }
        }
        Some(Song { orders: order_list[..num_orders].to_vec(), pat_len, patterns, samples })
    }

    fn pattern(&self, order: usize) -> Option<&Pattern> {
        self.patterns.get(*self.orders.get(order)? as usize)?.as_ref()
    }
}

/// What a cell's command does on the row tick and on the ticks after it. The player handles
/// some commands at once on the row (and writes the channel's period only on the later ticks);
/// all others, the ones it does not know too, write the period on every tick.
fn row_class(cmd: u8) -> bool {
    matches!(cmd, 0x0B | 0x0C | 0x0F | 0x12 | 0x14 | 0x21 | 0x49 | 0x4E | 0x4F)
}

#[derive(Clone, Copy, Default)]
struct Chan {
    smp: usize,
    /// The base period (what slides change), 0 before the first note.
    period: i32,
    note: u8,
    vol: i32,
    /// Where the sample's loop starts (address) and how many words it has (0 = a length of 0).
    loop_loc: u32,
    loop_words: u16,
    cmd: u8,
    param: u8,
    fresh: bool,
    /// Volume change each row, from `0B`.
    auto_vol: i32,
    vib_speed: u8,
    vib_depth: u8,
    vib_pos: u8,
    tp_active: bool,
    tp_target: i32,
    tp_speed: i32,
}

struct Stp3 {
    song: Song,
    base: u32,
    speed: u32,
    /// Quarter ticks added to a row's speed (0 to 15).
    frac: u32,
    /// The CIA timer value of the tempo (before the fraction scales it).
    tempo_timer: u32,
    counter: u32,
    pos: usize,
    row: usize,
    break_row: Option<usize>,
    jump: Option<usize>,
    songend: bool,
    visited: Vec<bool>,
    ch: [Chan; 4],
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let song = Song::parse(d)?;
    // the filter off (the LED), as the player's init does
    let pra = m.bus.read_byte(0xbfe001);
    m.bus.write_byte(0xbfe001, pra | 2);
    // every volume to 0
    {
        let mut hw = Hw { bus: &mut m.bus };
        // (the raster line phase the player's start-up leaves it at)
        hw.wait(71);
        for i in 0..4 {
            hw.vol(i, 0);
            hw.wait(6);
        }
    }
    let mut visited = vec![false; song.orders.len()];
    visited[0] = true;
    Some(Box::new(Stp3 {
        song,
        base,
        speed: 6,
        frac: 0,
        tempo_timer: DEFAULT_TIMER,
        // (the first row plays at the sixth interrupt)
        counter: 1,
        pos: 0,
        row: 0,
        break_row: None,
        jump: None,
        songend: false,
        visited,
        ch: [Chan::default(); 4],
    }))
}

impl Replayer for Stp3 {
    fn format(&self) -> &'static str {
        "Soundtracker Pro II"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer() as u64 + 1) * 5
    }

    /// The first interrupt comes 661 colour clocks later than the timer's period.
    fn timer_delay(&self) -> u64 {
        661
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.counter += 1;
        // a speed of 0 is one the counter never reaches again
        if self.speed != 0 && self.counter > self.speed {
            self.counter = 1;
            self.row_tick(hw);
        } else {
            self.later_tick(hw);
        }
    }
}

impl Stp3 {
    /// The CIA timer value of a tick: the tempo's, scaled by the speed's fraction.
    fn timer(&self) -> u32 {
        if self.frac == 0 || self.speed == 0 {
            return self.tempo_timer;
        }
        (self.tempo_timer as u64 * (4 * self.speed + self.frac) as u64 / (4 * self.speed) as u64) as u32
    }

    fn sample(&self, n: usize) -> Smp {
        self.song.samples.get(n).copied().unwrap_or_default()
    }

    fn period_of(note: u8) -> Option<i32> {
        PERIODS.get((note as usize).checked_sub(1)?).map(|&p| p as i32)
    }

    /// Sets the loop registers' values of channel `i` from its sample.
    fn set_loop(&mut self, i: usize) {
        let s = self.sample(self.ch[i].smp);
        let words = s.loop_len / 2;
        let c = &mut self.ch[i];
        c.loop_loc = self.base.wrapping_add(s.off).wrapping_add(s.loop_start);
        c.loop_words = if words > 1 { words.min(0xFFFF) as u16 } else { 0 };
    }

    fn row_tick(&mut self, hw: &mut Hw) {
        let pos = self.pos;
        let row = self.row;
        let cells: [Cell; 4] = match self.song.pattern(pos) {
            Some(p) if row < p.rows => std::array::from_fn(|i| p.cells.get(row * 4 + i).copied().unwrap_or_default()),
            _ => [Cell::default(); 4],
        };
        let mut mask = 0u16;
        for (i, cell) in cells.iter().enumerate() {
            // the volume slide of the earlier rows comes first
            let c = &mut self.ch[i];
            if c.auto_vol != 0 {
                c.vol = (c.vol + c.auto_vol).clamp(0, 64);
            }
            // (whether the channel started a note or had a command on the row before)
            let prev_fresh = c.fresh;
            let new_idle = cell.cmd == 0 && cell.param == 0;
            let has_cmd = !new_idle;
            let immediate = row_class(cell.cmd);
            // the sample first
            if cell.inst != 0 {
                let s = self.sample(cell.inst as usize);
                let c = &mut self.ch[i];
                c.smp = cell.inst as usize;
                c.vol = s.vol as i32;
                c.auto_vol = 0;
                self.set_loop(i);
            }
            let tone = cell.cmd == 0x13;
            let note_period = Self::period_of(cell.note);
            let starts = !tone && note_period.is_some();
            // the period as it was is written first unless the channel did something the row before
            // (the waits are the time the player's code takes, as the reference's runs show it)
            if !prev_fresh {
                if i > 0 {
                    hw.wait(206);
                }
                hw.per(i, self.ch[i].period as u16);
            } else if i > 0 && !starts {
                hw.wait(367);
            }
            if starts {
                let np = note_period.unwrap();
                let s = self.sample(self.ch[i].smp);
                let c = &mut self.ch[i];
                hw.wait(if !prev_fresh {
                    if i == 0 { 475 } else { 477 }
                } else if i == 0 {
                    475
                } else {
                    585
                });
                hw.dmacon(1 << i);
                hw.wait(147);
                let words = if s.loop_start == 0 { s.len / 2 } else { (s.loop_start + s.loop_len) / 2 };
                hw.len(i, words.min(0xFFFF) as u16);
                hw.wait(91);
                hw.loc(i, self.base.wrapping_add(s.off));
                hw.wait(105);
                c.period = np;
                c.note = cell.note;
                c.vib_pos = 0;
                c.tp_active = false;
                hw.per(i, np as u16);
                mask |= 1 << i;
                self.set_loop(i);
                if !immediate {
                    hw.wait(235);
                    hw.per(i, np as u16);
                }
            } else if !immediate {
                // (the final period)
                if !prev_fresh {
                    hw.wait(259);
                }
                // (a command the player does not know costs its dispatch)
                if has_cmd && !matches!(cell.cmd, 0x00..=0x02 | 0x10 | 0x13) {
                    hw.wait(232);
                }
                hw.per(i, self.ch[i].period as u16);
            }
            // what the command's handler costs
            hw.wait(match cell.cmd {
                0x0C if starts => 159,
                0x0C if !prev_fresh => 186,
                0x0C => 168,
                0x4E => 242,
                0x4F => 393,
                _ => 0,
            });
            // the command
            {
                let c = &mut self.ch[i];
                c.cmd = cell.cmd;
                c.param = cell.param;
                c.fresh = starts || has_cmd;
            }
            self.row_command(i, cell, note_period, tone);
            let c = &self.ch[i];
            hw.wait([219, 231, 242, 242][i]);
            hw.vol(i, c.vol.clamp(0, 64) as u16);
        }
        hw.wait(158);
        hw.dmacon(0x8000 | mask);
        hw.wait_lines(10, 105);
        for i in 0..4 {
            let c = self.ch[i];
            hw.loc(i, c.loop_loc);
            hw.wait(102);
            hw.len(i, c.loop_words);
            hw.wait(100);
        }
        self.advance();
    }

    /// The command of a row, at its first tick.
    fn row_command(&mut self, i: usize, cell: &Cell, note_period: Option<i32>, tone: bool) {
        let p = cell.param;
        let c = &mut self.ch[i];
        match cell.cmd {
            0x0B => {
                // up by the low nibble, or down by the high one
                c.auto_vol = if p & 15 != 0 { (p & 15) as i32 } else { -((p >> 4) as i32) };
            }
            0x0C => {
                c.vol = (p as i32).min(64);
                c.auto_vol = 0;
            }
            0x0F => {
                self.speed = (p >> 4) as u32;
                self.frac = (p & 15) as u32;
            }
            0x10 => {
                if p >> 4 != 0 {
                    c.vib_speed = p >> 4;
                }
                if p & 15 != 0 {
                    c.vib_depth = p & 15;
                }
            }
            0x12 => self.break_row = Some(p as usize),
            0x13 => {
                if let Some(t) = note_period {
                    c.tp_target = t;
                }
                if p != 0 {
                    c.tp_speed = p as i32;
                }
                c.tp_active = c.tp_target != 0;
            }
            0x14 => self.jump = Some(p as usize),
            0x4E => match p >> 4 {
                0x1 => c.period = (c.period - (p & 15) as i32).max(MIN_PERIOD),
                0x2 => c.period = (c.period + (p & 15) as i32).min(MAX_PERIOD),
                _ => {}
            },
            0x4F => {
                if p >= 0x20 {
                    self.tempo_timer = TEMPO_CONST / p as u32;
                } else if p != 0 {
                    self.speed = p as u32;
                    self.frac = 0;
                }
            }
            _ => {}
        }
        let _ = tone;
    }

    /// Moves to the next row, or to where a break or jump goes.
    fn advance(&mut self) {
        let jump = self.jump.take();
        let brk = self.break_row.take();
        let rows = self.song.pattern(self.pos).map_or(self.song.pat_len, |p| p.rows);
        let mut next_order = None;
        let mut next_row = self.row + 1;
        if let Some(j) = jump {
            next_order = Some(j);
            next_row = brk.unwrap_or(0);
        } else if let Some(b) = brk {
            next_order = Some(self.pos + 1);
            next_row = b;
        } else if next_row >= rows {
            next_order = Some(self.pos + 1);
            next_row = 0;
        }
        if let Some(mut o) = next_order {
            if o >= self.song.orders.len() {
                o = 0;
                self.songend = true;
            } else if self.visited[o] {
                self.songend = true;
            }
            self.visited[o] = true;
            self.pos = o;
        }
        let rows = self.song.pattern(self.pos).map_or(self.song.pat_len, |p| p.rows);
        self.row = if next_row < rows { next_row } else { 0 };
    }

    /// A tick between rows: the effects, then the volumes.
    fn later_tick(&mut self, hw: &mut Hw) {
        let k = self.counter as i32 - 1;
        for i in 0..4 {
            let c = &mut self.ch[i];
            let mut write = c.cmd != 0 && c.cmd != 0x4E || c.cmd == 0 && c.param != 0 || c.tp_active;
            let mut out = c.period;
            match c.cmd {
                0x00 if c.param != 0 => {
                    let n = match k % 3 {
                        1 => c.param >> 4,
                        2 => c.param & 15,
                        _ => 0,
                    };
                    out = Self::period_of(c.note.saturating_add(n)).unwrap_or(c.period);
                }
                0x01 if k >= 2 => {
                    c.period = (c.period - c.param as i32).max(MIN_PERIOD);
                    out = c.period;
                }
                0x02 if k >= 2 => {
                    c.period = (c.period + c.param as i32).min(MAX_PERIOD);
                    out = c.period;
                }
                0x10 => {
                    let delta = (SINE[(c.vib_pos & 31) as usize] as i32 * c.vib_depth as i32) >> 7;
                    out = if c.vib_pos & 32 != 0 { c.period - delta } else { c.period + delta };
                    c.vib_pos = c.vib_pos.wrapping_add(c.vib_speed) & 63;
                }
                _ => {}
            }
            if c.tp_active && k >= 1 {
                if c.period > c.tp_target {
                    c.period = (c.period - c.tp_speed).max(c.tp_target);
                } else if c.period < c.tp_target {
                    c.period = (c.period + c.tp_speed).min(c.tp_target);
                }
                if c.period == c.tp_target {
                    c.tp_active = false;
                }
                out = c.period;
                write = true;
            }
            if write {
                hw.wait(if i == 0 { 0 } else { 333 });
                hw.per(i, out.clamp(0, 0xFFFF) as u16);
                hw.wait(if i == 0 { 194 } else { 215 });
            } else {
                hw.wait(if i == 0 { 0 } else if i == 1 { 363 } else { 375 });
            }
            hw.vol(i, c.vol.clamp(0, 64) as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// A version 2 module of one 8-row pattern: row 0 plays note 25 of a looped 256-byte
    /// sample (volume 48) on voice 0, row 4 sets the volume to 20 (`0C14`).
    fn module() -> Vec<u8> {
        let mut d = b"STP3".to_vec();
        d.extend(2u16.to_be_bytes());
        d.push(1); // one order
        d.push(8); // rows
        d.extend([0u8; 128]); // the order list: pattern 0
        for v in [6u16, 0, 2608, 2] {
            d.extend(v.to_be_bytes()); // speed, fraction, timer, flags
        }
        d.extend(0u32.to_be_bytes());
        d.extend(50u16.to_be_bytes()); // the MIDI list
        d.extend([0u8; 50]);
        d.extend(1u16.to_be_bytes()); // one sample
        d.extend(0u16.to_be_bytes()); // (the structure size; version 2 has chunk sizes)
        // sample 1: empty path, flags, name "s", then the 20-byte header
        let mut chunk = vec![0u8, 0, b's', 0];
        chunk.extend(256u32.to_be_bytes()); // length
        chunk.extend([48, 0]); // volume
        chunk.extend(0u32.to_be_bytes()); // loop start
        chunk.extend(256u32.to_be_bytes()); // loop length
        chunk.extend([0u8; 6]); // default command and period, fine tune
        d.extend(1u16.to_be_bytes());
        d.extend((chunk.len() as u32 + 2).to_be_bytes());
        d.extend(&chunk);
        d.extend(0u16.to_be_bytes()); // the loop list
        // the pattern: index 0, 8 rows, 4 channels
        for v in [0u16, 8, 4] {
            d.extend(v.to_be_bytes());
        }
        let mut cells = vec![[0u8; 4]; 8 * 4];
        cells[0] = [1, 25, 0, 0];
        cells[4 * 4] = [0, 0, 0x0c, 20];
        for c in &cells {
            d.extend(c);
        }
        d.extend(0xFFFFu16.to_be_bytes()); // no more patterns
        d.extend(0xFFFFu16.to_be_bytes()); // no scripts
        d.extend([0u8; 34]); // the drum pad
        d.extend((0..256).map(|i| (i as i8).wrapping_mul(3) as u8));
        d
    }

    #[test]
    fn detects_and_plays() {
        assert!(super::Song::parse(&module()).is_some());
        let mut bad = module();
        bad[0] = b'X';
        assert!(super::Song::parse(&bad).is_none());
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.stp"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Soundtracker Pro II");
        // the row at the sixth interrupt (five ticks of volume 0 before it), the volume
        // command on row 4 (tick 6 + 4 * 6)
        let mut out = vec![0f32; 2 * 48000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        let vol = reg(0xDFF0A8);
        assert!(vol[..5].iter().all(|&v| v == 0));
        assert!(vol.contains(&48) && *vol.last().unwrap() == 20);
        // note 25's period, the sample's length in words, DMA off then on for the note
        assert!(reg(0xDFF0A6).contains(&808));
        assert!(reg(0xDFF0A4).iter().all(|&l| l == 128));
        let dma = reg(0xDFF096);
        assert!(dma.windows(2).any(|w| w == [0x0001, 0x8001]));
        // the output is not silent
        assert!(out.iter().any(|&x| x != 0.0));
    }
}
