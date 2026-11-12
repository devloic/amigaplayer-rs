//! Infogrames (Amiga, `NAME.dum` song file plus `NAME.ins` instrument bank; Eagleplayer
//! "Infogrames"; games such as Bob Morane, Captain Blood, Bivouac, Stir Crazy featuring Bobo).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A song file holds a table of songs, each with a
//! speed, 13-byte volume and pitch envelope tables, four tracks of pattern numbers and the
//! patterns (byte streams of notes, durations, sample, envelope and transpose commands). The
//! instrument bank is loaded beside the song file (its name is derived from the song's) and only
//! its sample table is read; sample data go to Paula by address. Four voices; the last song of the
//! file is played, as the reference does. Every register write is placed at the clock of the
//! original's code from the specification's cycle ledger, including the audio DMA waits.
//!
//! The two small tables (the 104 periods and the list of known songs with their tick timers) are
//! embedded: taken from the original player on the project owner's decision.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a period index
//! of 104 or more after transposition gives period 0; bytes read outside the song file count as
//! 0xFF in a pattern or a track (the pattern or the voice ends); event bytes 0xE4..0xFE with low
//! three bits 4..7 end the voice; a sample slot outside the bank reads as zeros. The song is
//! reported as ended when all four voices have stopped (the reference keeps writing silence).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Infogrames", detect, new };

const PERIODS: &[u8; 208] = include_bytes!("infogrames_periods.bin");
const TEMPO: &[u8; 144] = include_bytes!("infogrames_tempo.bin");

const DURATION: [u8; 16] = [2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 8, 0, 0, 0];
/// CIA timer value of a song that is not in the known list.
const DEFAULT_TIMER: u64 = 0x1A00;
const LINE: u64 = 227;

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The header offset of the first song if the file is a song file (spec 1.3).
fn locate(d: &[u8]) -> Option<usize> {
    let h = be16(d, 0) as usize;
    if d.len() < 4 || h == 0 || h % 2 != 0 || h >= d.len() {
        return None;
    }
    let a = h + be16(d, h + 2) as usize;
    (a + 1 < d.len() && d[a] == 0x00 && d[a + 1] == 0x0F).then_some(h)
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

/// The tick timer value (spec 1.6) and the index of the list entry it came from, if any.
fn timer_value(d: &[u8]) -> (u64, Option<usize>) {
    let words = d.len() / 4;
    let mut sum = (0..words).fold(0u32, |s, i| s.wrapping_add(be32(d, 4 * i)));
    let r = d.len() % 4;
    if r != 0 {
        let mut w = [0u8; 4];
        w[..r].copy_from_slice(&d[4 * words..]);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
    }
    for (i, e) in TEMPO.chunks(12).enumerate() {
        if be32(e, 0) == sum && be32(e, 4) == d.len() as u32 {
            return (be32(e, 8) as u64, Some(i));
        }
    }
    (DEFAULT_TIMER, None)
}

#[derive(Clone, Copy, Default)]
struct Env {
    data: [u8; 12],
    seg: usize,
    cur: u16,
    base: u16,
    step: i8,
    looping: bool,
    pending: bool,
    done: bool,
    tick: u8,
    rep: u8,
}

impl Env {
    fn select(&mut self, data: [u8; 12], looping: bool, step: i8) {
        self.data = data;
        self.looping = looping;
        self.step = step;
        self.restart();
    }

    fn restart(&mut self) {
        self.seg = 0;
        self.cur = 0;
        self.base = 0;
        self.tick = 0;
        self.rep = 0;
        self.pending = true;
        self.done = false;
    }

    /// Spec 4.3: returns the value and the cost in cycles.
    fn step(&mut self, x: u16) -> (u16, u64) {
        let (count, delta, ticks) = (self.data[self.seg], self.data[self.seg + 1] as i8, self.data[self.seg + 2]);
        let mut c = 73;
        if self.pending {
            self.pending = false;
            self.cur = self.cur.wrapping_add(delta as i16 as u16);
            c += 32;
        }
        c += 43;
        let mut v = x.wrapping_add(self.cur).wrapping_sub(self.base);
        if v & 0x8000 != 0 {
            v = 0;
            c += 4;
        }
        c += 25;
        if self.done {
            return (v, c + 16);
        }
        self.tick = self.tick.wrapping_add(1);
        c += 49;
        if self.tick != ticks {
            return (v, c + 16);
        }
        self.tick = 0;
        self.rep = self.rep.wrapping_add(1);
        c += 65;
        if self.rep != count {
            self.pending = true;
            return (v, c + 36);
        }
        self.rep = 0;
        c += 49;
        if self.seg < 9 {
            self.seg += 3;
            self.pending = true;
            return (v, c + 48);
        }
        c += 21;
        if !self.looping {
            self.done = true;
            return (v, c + 36);
        }
        self.base = self.base.wrapping_add(self.step as i16 as u16);
        self.seg = 3;
        self.pending = true;
        (v, c + 56 + 48)
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    track_pos: usize,
    pat_pos: usize,
    rows: u8,
    dur: u8,
    transpose: u8,
    period: u16,
    stopped: bool,
    start: bool,
    a: Env,
    b: Env,
}

pub struct Infogrames {
    d: Vec<u8>,
    h: usize,
    /// Guest address of the bank's sample base (file offset 4), and the bank itself.
    bank: Vec<u8>,
    bank_base: u32,
    speed: u8,
    rowctr: u8,
    tick_cc: u64,
    v: [Voice; 4],
    songend: bool,
}

impl Infogrames {
    fn env_entry(&self, table: usize, i: usize) -> [u8; 13] {
        let o = self.h + table + 13 * i;
        let mut e = [0u8; 13];
        for (k, b) in e.iter_mut().enumerate() {
            *b = byte(&self.d, o + k);
        }
        e
    }

    /// Spec 3.4. Returns false when the track ended (the voice stops).
    fn fetch(&mut self, v: usize, cost: &mut u64) -> bool {
        *cost += 41;
        let p = self.v[v].track_pos;
        let e = if p < self.d.len() { self.d[p] } else { 0xFF };
        if e == 0xFF {
            self.v[v].stopped = true;
            *cost += 25 + 107 + 16;
            return false;
        }
        self.v[v].track_pos += 1;
        self.v[v].pat_pos = self.h + be16(&self.d, self.h + 16 + 2 * e as usize) as usize;
        *cost += 122;
        true
    }

    /// Spec 3.3. Returns whether the voice is still running.
    fn read_events(&mut self, v: usize, smp: &mut Option<u8>, cost: &mut u64) -> bool {
        loop {
            let p = self.v[v].pat_pos;
            let b = if p < self.d.len() { self.d[p] } else { 0xFF };
            self.v[v].pat_pos = p.saturating_add(1);
            *cost += 41;
            let arg = |s: &Self| byte(&s.d, s.v[v].pat_pos);
            match b {
                0x00..=0x7F => {
                    let vs = &mut self.v[v];
                    let idx = if b == 0 { 0 } else { b.wrapping_add(vs.transpose) as usize };
                    vs.period = if idx < 104 { be16(PERIODS, 2 * idx) } else { 0 };
                    vs.a.restart();
                    vs.b.restart();
                    *cost += 51 + 25 + if b != 0 { 12 } else { 0 } + 52 + 282;
                    return true;
                }
                0x80..=0x9F => {
                    let vs = &mut self.v[v];
                    vs.dur = DURATION[(b & 15) as usize];
                    vs.rows = vs.dur;
                    *cost += 51 + 19 + 19 + 74;
                }
                0xA0..=0xBF => {
                    *smp = Some(b & 31);
                    *cost += 51 + 19 + 19 + 30;
                }
                0xC0..=0xDF => {
                    let e = self.env_entry(self.vol_table(), (b & 31) as usize);
                    let mut data = [0u8; 12];
                    data.copy_from_slice(&e[1..]);
                    self.v[v].a.select(data, e[0] & 0x80 != 0, (e[0] & 0x7F) as i8);
                    *cost += 51 + 19 + 19 + 292;
                }
                0xE0 | 0xE8 | 0xF0 | 0xF8 => {
                    let a = arg(self);
                    self.v[v].pat_pos += 1;
                    self.v[v].transpose = a;
                    *cost += 51 + 19 + 19 + 72 + 58;
                }
                0xE1 | 0xE9 | 0xF1 | 0xF9 | 0xE2 | 0xEA | 0xF2 | 0xFA => {
                    let a = arg(self);
                    self.v[v].pat_pos += 1;
                    let e = self.env_entry(self.pitch_table(), a as usize);
                    let mut data = [0u8; 12];
                    data.copy_from_slice(&e[1..]);
                    self.v[v].b.select(data, b & 7 == 1, 0);
                    *cost += 51 + 19 + 19 + 72 + 316;
                }
                0xE3 | 0xEB | 0xF3 | 0xFB => *cost += 51 + 19 + 19 + 72 + 10,
                0xFF => {
                    if !self.fetch(v, cost) {
                        return false;
                    }
                }
                // undefined in the original (it jumps into data): the voice ends
                _ => {
                    self.v[v].stopped = true;
                    return false;
                }
            }
        }
    }

    fn vol_table(&self) -> usize {
        be16(&self.d, self.h + 2) as usize
    }

    fn pitch_table(&self) -> usize {
        be16(&self.d, self.h + 4) as usize
    }

    /// Spec 3.2 and 4.3: one voice's step. Returns the volume, the period, the sample to start,
    /// and the cycles the step took.
    fn step(&mut self, v: usize, row_tick: bool) -> (u16, u16, Option<u8>, u64) {
        let mut smp = None;
        if self.v[v].stopped {
            return (0, 0, None, 43);
        }
        let mut cost = 121;
        let read;
        if self.v[v].start {
            self.v[v].start = false;
            cost += 54;
            self.v[v].track_pos = be16(&self.d, self.h + 6 + 2 * v) as usize + self.h;
            read = self.fetch(v, &mut cost);
            if !read {
                return (0, 0, None, cost);
            }
        } else {
            cost += 21;
            if row_tick {
                self.v[v].rows = self.v[v].rows.wrapping_sub(1);
                cost += 16;
            }
            cost += 23;
            read = self.v[v].rows == 0;
            if read {
                cost += 20;
                self.v[v].rows = self.v[v].dur;
            }
        }
        if read && !self.read_events(v, &mut smp, &mut cost) {
            return (0, 0, smp, cost);
        }
        cost += 38;
        let (a, ca) = self.v[v].a.step(0);
        cost += ca + 37 + 21 + 78;
        let per_in = self.v[v].period;
        let (b, cb) = self.v[v].b.step(per_in);
        cost += cb + 40;
        (a & 0xFF, b, smp, cost)
    }

    fn tick_run(&mut self, hw: &mut Hw) {
        self.rowctr = self.rowctr.wrapping_sub(1);
        let row_tick = self.rowctr == 0;
        let mut c0 = hw.bus.clock;
        let mut t: u64 = 359;
        for v in 0..4 {
            t += 46;
            let (vol, per, smp, cost) = self.step(v, row_tick);
            t += cost + 149;
            hw.bus.clock = c0 + t / 2;
            hw.vol(v, vol);
            t += 22;
            hw.bus.clock = c0 + t / 2;
            hw.per(v, per);
            if let Some(s) = smp {
                let e = 4 + 16 * s as usize;
                let (start, rep) = (be32(&self.bank, e) as u64, be32(&self.bank, e + 4) as u64);
                let (len, rlen) = (be16(&self.bank, e + 12), be16(&self.bank, e + 14));
                let at = |o: u64| self.bank_base.wrapping_add(o as u32);
                t += 25 + 162;
                hw.bus.clock = c0 + t / 2;
                hw.loc(v, at(start));
                t += 24;
                hw.bus.clock = c0 + t / 2;
                hw.len(v, len);
                t += 16;
                hw.bus.clock = c0 + t / 2;
                hw.dmacon(1 << v);
                t += 198;
                c0 = ((c0 + t / 2) / LINE + 10) * LINE;
                t = 188;
                hw.bus.clock = c0 + t / 2;
                hw.dmacon(0x8200 | 1 << v);
                t += 198;
                c0 = ((c0 + t / 2) / LINE + 10) * LINE;
                t = 208;
                hw.bus.clock = c0 + t / 2;
                hw.loc(v, at(rep));
                t += 24;
                hw.bus.clock = c0 + t / 2;
                hw.len(v, rlen);
                t += 107;
            } else {
                t += 25 + 107;
            }
        }
        if row_tick {
            self.rowctr = self.speed;
        }
        if self.v.iter().all(|v| v.stopped) {
            self.songend = true;
        }
    }
}

impl Replayer for Infogrames {
    fn format(&self) -> &'static str {
        "Infogrames"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        self.tick_cc
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_run(hw);
    }
}

/// The companion bank's names (spec 1.4): `.dum` replaced by `.ins` (or `.ins` appended), then
/// the same with the character before the suffix dropped.
fn bank_names(file: &str) -> Vec<String> {
    let stem = match file.len().checked_sub(4) {
        Some(n) if file.is_char_boundary(n) && file[n..].eq_ignore_ascii_case(".dum") => &file[..n],
        _ => file,
    };
    let mut v = vec![format!("{stem}.ins")];
    if let Some((i, _)) = stem.char_indices().last() {
        v.push(format!("{}.ins", &stem[..i]));
    }
    v
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    locate(d)?;
    let n = be16(d, 0) as usize / 2;
    let h = be16(d, 2 * (n - 1)) as usize;
    if h + 16 > d.len() {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    // no bank of the module's name: the Rob Hubbard 2 modules (Stir Crazy featuring Bobo, Captain
    // Blood) are this format with the bank `rh.ins` beside them, played by the DeliPlayer
    // `RobHubbard_2`, whose periods differ by one in places; nothing in the module tells them apart
    let Some((bank, bank_try)) = bank_names(&file)
        .iter()
        .enumerate()
        .find_map(|(i, n)| crate::deli::read_companion(m, n).1.ok().map(|b| (b, i as u64)))
    else {
        return super::robhubbard2::new(d, _base, m);
    };
    let addr = m.alloc(bank.len() as u32 + 16);
    if addr == 0 {
        return None;
    }
    m.bus.write_bytes(addr, &bank);
    let bank_base = addr + 4;
    let words = d.len() as u64 / 4;
    // the reference's start-up (the checksum) costs 2259 cc and 27.5 cc per word; the first tick
    // follows one period after it
    let (timer, hit) = timer_value(d);
    let mut init = 2259 + (words * 55) / 2;
    // measured: a song found in the list ends the search early (11 entries skipped at the first
    // one saves 333 cc; the saving per entry is interpolated, only the first entry was seen)
    if let Some(i) = hit {
        init -= (11 - i as u64) * 333 / 11;
    }
    // measured (half cycles, floor to a colour clock): the reference's run origin lies 644.5 cc
    // plus 8.5 cc per character of the song's full path (directory and name alike) before the
    // player's start-up; 12.5 cc more for every lower-case letter of a ".dum" extension (its
    // case-folding), and 81 cc more when the first bank name (NAME.ins) was not found and the
    // second (the name less its last character) was
    let path_len = m.bus.read_cstr(m.deli.dir_buf).len() + file.len();
    let ext_lower = match file.len().checked_sub(4) {
        Some(n) if file.is_char_boundary(n) && file[n..].eq_ignore_ascii_case(".dum") => {
            file[n + 1..].bytes().filter(u8::is_ascii_lowercase).count() as u64
        }
        _ => 0,
    };
    let host = (1289 + 17 * path_len as u64 + 25 * ext_lower + 162 * bank_try) / 2;
    let t = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    m.bus.clock = t + host + init;
    let speed = byte(d, h + 1);
    let mut v = [Voice::default(); 4];
    for vs in v.iter_mut() {
        vs.start = true;
    }
    Some(Box::new(Infogrames {
        d: d.to_vec(),
        h,
        bank,
        bank_base,
        speed,
        rowctr: speed,
        tick_cc: (timer + 1) * 5,
        v,
        songend: false,
    }))
}

#[cfg(test)]
#[path = "infogrames_tests.rs"]
mod tests;
