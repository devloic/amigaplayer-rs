//! MultiMedia Sound (the replay routine of mms.library 6.4; Eagleplayer "MultiMedia Sound",
//! file name prefixes `mms` and `sfx20`), written from the behaviour specification
//! (register-write logs of the reference player), not from NostalgicPlayer.
//!
//! The module layout is the SoundFX 2.0 one (mark `SO31`, 31 samples), but the replay engine is
//! another: AUDxVOL is written for every voice on every row, command F sets the speed, 4 is a
//! vibrato, C sets the volume, steps stop at their target, `BRK` takes effect after the row,
//! `PIC` clears the effect, and orders separated by empty patterns are subsongs.
//!
//! Routing: both this player and the SoundFX 2.0 replayer accept the same files, so this one
//! takes a module only when the file name says so (prefix `mms.` or `sfx20.`, or suffix `.mms`
//! or `.sfx20`, in any case), as UADE sends those prefixes to MultiMedia Sound. The engine
//! and the library tell `detect` the name (see `adpcmmono::set_name`); without a name the
//! SoundFX 2.0 replayer, which comes after this one, takes the file.
//!
//! The writes are timed in 68000 cycles (2 cycles = 1 colour clock) from the tick's start, with
//! two waits for the beam (9 line changes each) around the DMA-on write, so where the tick lies
//! against the 227-colour-clock line grid decides the DMA off to on time and the loop writes.
//! The period and vibrato tables are the player's own values, typed in from the specification.

use super::{be16, be32, Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "MultiMedia Sound", detect, new };

/// The period table: 20 words of 1076 in front (an index up to 20 below the first entry reads
/// them), 66 entries (40 notes, then 113 repeated), and the terminator.
const PAD: usize = 20;
const NOTES: [u16; 40] = [
    1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302,
    285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];
const ENTRIES: usize = 66;
const fn period_table() -> [u16; PAD + ENTRIES + 1] {
    let mut t = [1076u16; PAD + ENTRIES + 1];
    let mut i = 0;
    while i < ENTRIES {
        t[PAD + i] = if i < 40 { NOTES[i] } else { 113 };
        i += 1;
    }
    t[PAD + ENTRIES] = 0xFFFF;
    t
}
pub(crate) const PERIODS: [u16; PAD + ENTRIES + 1] = period_table();

/// The vibrato sine.
pub(crate) const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180,
    161, 141, 120, 97, 74, 49, 24,
];

/// The word `k` entries from the first table entry (k below 0 reaches the pad); past the
/// terminator it reads the terminator.
fn tab(k: i32) -> u16 {
    let i = (k + PAD as i32).clamp(0, (PAD + ENTRIES) as i32);
    PERIODS[i as usize]
}

const ORDERS: u32 = 0x434;
const PATTERNS: u32 = 0x4B8;

/// Whether a file name is one this player takes: prefix `mms.` or `sfx20.`, or suffix `.mms`
/// or `.sfx20`, in any case.
pub fn name_ok(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("mms.") || n.starts_with("sfx20.") || n.ends_with(".mms") || n.ends_with(".sfx20")
}

fn content_ok(d: &[u8]) -> bool {
    d.len() >= 0x84
        && d[0x7C..0x80] == *b"SO31"
        && be16(d, 0x80) != 0
        && (0..31).all(|i| {
            let l = be32(d, 4 * i);
            l & 1 == 0 && l <= 0x20000
        })
}

fn detect(d: &[u8]) -> bool {
    name_ok(&super::adpcmmono::current_name()) && content_ok(d)
}

/// The module's geometry as the player's init finds it: the song length, where the samples
/// start (after the patterns up to the highest one used) and the module size it needs.
struct Layout {
    songlen: u32,
    samples: u32,
    total: u64,
}

fn layout(d: &[u8]) -> Option<Layout> {
    let n = *d.get(0x432)? as u32;
    if n == 0 {
        // (the original scans 65536 order bytes; no module has it)
        return None;
    }
    let max = (0..n).map(|i| d.get((ORDERS + i) as usize).copied().unwrap_or(0)).max()? as u64;
    let s = PATTERNS as u64 + 1024 * (max + 1);
    let total = s + (0..31).map(|i| be32(d, 4 * i) as u64).sum::<u64>();
    (total <= d.len() as u64).then_some(Layout { songlen: n, samples: s as u32, total })
}

/// Whether the init looks for subsongs (section 3.3 of the specification).
fn splits(l: &Layout, tempo: u16) -> bool {
    !(l.total == 95_688 || tempo <= 0x1BB6 || l.songlen == 1)
}

/// The 68000 cycles from the machine's start to the host's first volume write, the original's
/// init (measured on the reference's runs of the 10 refcmp rows and 30 one-change copies, exact
/// on all): 28 a sample with data, 31 an order and 4 more where it is at least the highest
/// before it; when it looks for subsongs, 98, then 133 a position whose pattern has a note, 65
/// an entry 0 and 62 an entry 0xFFFE0000 before it, for an empty-or-stopped pattern 83 at the
/// last position, otherwise 205 (213 from the second on) and 36 a long of the next pattern up
/// to its first one not 0 (57 less when all 256 are 0: the abort).
fn init_cycles(d: &[u8], l: &Layout, tempo: u16) -> u64 {
    let n = l.songlen as usize;
    let order = |p: usize| d.get(ORDERS as usize + p).copied().unwrap_or(0) as usize;
    let long = |p: usize, k: usize| {
        let o = PATTERNS as usize + 1024 * order(p) + 4 * k;
        d.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut c = 5955 + 28 * (0..31).filter(|&i| be32(d, 4 * i) != 0).count() as u64 + 31 * n as u64;
    let mut max = 0;
    for p in 0..n {
        if order(p) >= max {
            max = order(p);
            c += 4;
        }
    }
    if !splits(l, tempo) {
        return c;
    }
    c += 98;
    let (mut p, mut split) = (1, 0);
    while p < n {
        let mut quiet = true;
        for k in 0..256 {
            match long(p, k) {
                0 => c += 65,
                0xFFFE_0000 => c += 62,
                _ => {
                    quiet = false;
                    break;
                }
            }
        }
        if !quiet {
            c += 133;
            p += 1;
            continue;
        }
        if p + 1 == n {
            c += 83;
            break;
        }
        let k = (0..256).find(|&k| long(p + 1, k) != 0);
        c += 205 + if split > 0 { 8 } else { 0 } + k.map_or(36 * 256 - 57, |k| 36 * (k as u64 + 1));
        split += 1;
        if k.is_none() {
            break;
        }
        p += 2;
    }
    c
}

/// The host's volume writes of the song init, colour clocks after the previous one (the first after
/// the init); and from the last of them to the song's start, by the filter byte (0, 0xFF, other).
const HOST_GAPS: [u64; 8] = [0, 150, 162, 174, 262, 150, 162, 174];
const HOST_START: [u64; 3] = [1724, 1733, 1746];

/// The (start, end) position pairs. `end` is exclusive; 0 marks the one subsong an aborted split
/// leaves without an end.
fn subsongs(d: &[u8], l: &Layout, tempo: u16) -> Vec<(u16, u16)> {
    let n = l.songlen as u16;
    let mut v = vec![(0u16, n)];
    if !splits(l, tempo) {
        return v;
    }
    let pat = |p: u16| -> &[u8] {
        let o = d.get(ORDERS as usize + p as usize).copied().unwrap_or(0) as usize;
        d.get(PATTERNS as usize + 1024 * o..PATTERNS as usize + 1024 * (o + 1)).unwrap_or(&[])
    };
    let quiet = |p: &[u8]| p.len() == 1024 && p.chunks(4).all(|e| e == [0; 4] || e == [0xFF, 0xFE, 0, 0]);
    let zero = |p: &[u8]| p.len() == 1024 && p.iter().all(|&b| b == 0);
    let mut p = 1u16;
    while p < n {
        if !quiet(pat(p)) {
            p += 1;
        } else if p + 1 == n {
            break;
        } else if zero(pat(p + 1)) {
            // aborted: the first subsong runs to the end, the splits so far stay, the last has no end
            v[0].1 = n;
            if v.len() > 1 {
                v.last_mut().unwrap().1 = 0;
            }
            return v;
        } else {
            v.last_mut().unwrap().1 = p;
            v.push((p + 1, n));
            p += 2;
        }
    }
    v
}

#[derive(Clone, Copy, Default)]
struct Voice {
    w: u16,
    b2: u8,
    b3: u8,
    start: u32,
    len: u16,
    period: u16,
    volume: u16,
    loop_addr: u32,
    loop_len: u16,
    step: i16,
    stepcur: u16,
    target: u16,
    vibparam: u8,
    vibpos: u8,
}

pub struct MultiMediaSound {
    base: u32,
    samples: [u32; 31],
    tempo: u16,
    subs: Vec<(u16, u16)>,
    cur: u16,
    v: [Voice; 4],
    speed: u16,
    timer: u16,
    pos: u16,
    start: u16,
    end: u16,
    row: u32,
    brk: bool,
    songend: bool,
    /// The absolute time of the next first tick, when it is known.
    next_at: Option<u64>,
}

#[cfg(test)]
thread_local! {
    /// Test hook: the absolute time of the first tick after the start or a change of subsong.
    pub(crate) static FIRST_TICK: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn first_tick_hook() -> Option<u64> {
    #[cfg(test)]
    return FIRST_TICK.with(|c| c.get());
    #[cfg(not(test))]
    None
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !content_ok(d) {
        return None;
    }
    let l = layout(d)?;
    let tempo = be16(d, 0x80);
    // sample addresses; the first long of each sample that has data is cleared, for good
    let mut samples = [0u32; 31];
    let mut a = base.wrapping_add(l.samples);
    for (i, s) in samples.iter_mut().enumerate() {
        *s = a;
        let len = be32(d, 4 * i);
        if len != 0 {
            m.bus.w32(a, 0);
        }
        a = a.wrapping_add(len);
    }
    let subs = subsongs(d, &l, tempo);
    // the song starts on the clock the original's init and the host's start reach
    m.bus.clock = m.bus.clock.max(init_cycles(d, &l, tempo) / 2);
    let filter = d[0x433];
    let mut s = MultiMediaSound {
        base,
        samples,
        tempo,
        subs,
        cur: 0,
        v: [Voice::default(); 4],
        speed: 6,
        timer: 0,
        pos: 0,
        start: 0,
        end: 0,
        row: 0,
        brk: false,
        songend: false,
        next_at: first_tick_hook(),
    };
    s.init_song(0, &mut m.bus);
    m.bus.clock += HOST_START[match filter {
        0 => 0,
        0xFF => 1,
        _ => 2,
    }];
    Some(Box::new(s))
}

/// The raster wait: nine changes of the beam line, polled; `u` is the cycle count in the tick
/// that started at colour clock `e`. Returns the cycle count after it.
fn raster_wait(e: u64, mut u: u64) -> u64 {
    let line = |u: u64| (e + u / 2) / crate::paula::LINE;
    u += 28;
    for _ in 0..9 {
        u += 16;
        let l = line(u);
        loop {
            u += 16;
            if line(u) != l {
                break;
            }
            u += 9;
        }
        u += 19;
    }
    u + 44
}

/// Lets the clock reach cycle `u` of the tick that started at `e`.
fn stamp(hw: &mut Hw, e: u64, u: u64) {
    let t = e + u / 2;
    if t > hw.bus.clock {
        hw.bus.clock = t;
    }
}

impl MultiMediaSound {
    /// Song init (each start of a subsong): the host's volume writes and the filter, the voices
    /// cleared, speed 6.
    fn init_song(&mut self, n: u16, b: &mut Bus) {
        self.cur = n.min(self.subs.len() as u16 - 1);
        for (k, g) in HOST_GAPS.iter().enumerate() {
            b.clock += g;
            b.write_word(0xDFF0A8 + 16 * (k as u32 & 3), 0);
        }
        // the filter byte: 0 is on, 0xFF off, anything else is cleared in the module and is on
        match b.r8(self.base + 0x433) {
            0xFF => b.ciaa.pra |= 2,
            0 => b.ciaa.pra &= !2,
            _ => {
                b.w8(self.base + 0x433, 0);
                b.ciaa.pra &= !2;
            }
        }
        self.v = [Voice::default(); 4];
        self.speed = 6;
        self.timer = 0;
        let (s, e) = self.subs[self.cur as usize];
        // (a subsong without an end, left by an aborted split, has end 0: its position runs on past
        // the order list, reading what lies there, until the 16-bit position wraps to 0)
        (self.start, self.end, self.pos, self.row, self.brk) = (s, e, s, 0, false);
    }

    fn row_tick(&mut self, hw: &mut Hw, e: u64, u: &mut u64) {
        let b = self.base;
        *u += 194;
        let order = hw.bus.r8(b.wrapping_add(ORDERS).wrapping_add(self.pos as u32)) as u32;
        let rowat = b.wrapping_add(PATTERNS).wrapping_add(order << 10).wrapping_add(self.row);
        let mut mask = 0u16;
        for i in 0..4 {
            *u += 18;
            self.row_voice(i, rowat.wrapping_add(4 * i as u32), hw, e, u, &mut mask);
            *u += 34;
        }
        *u += 30;
        *u = raster_wait(e, *u);
        *u += 24;
        stamp(hw, e, *u);
        hw.dmacon(0x8000 | mask);
        *u += 18;
        *u = raster_wait(e, *u);
        *u += 24;
        for i in (0..4).rev() {
            *u += 24;
            stamp(hw, e, *u);
            hw.loc(i, self.v[i].loop_addr);
            *u += 20;
            stamp(hw, e, *u);
            hw.len(i, self.v[i].loop_len);
            *u += 34;
        }
        if self.brk {
            self.row = 0x3F0;
            self.brk = false;
        }
        self.row += 16;
        if self.row >= 1024 {
            self.row = 0;
            self.pos = self.pos.wrapping_add(1);
            if self.pos == self.end {
                self.pos = self.start;
                self.songend = true;
            }
        }
    }

    fn row_voice(&mut self, i: usize, at: u32, hw: &mut Hw, e: u64, u: &mut u64, mask: &mut u16) {
        let bit = 1u16 << i;
        let base = self.base;
        let (w, b2, b3) = (hw.bus.r16(at), hw.bus.r8(at.wrapping_add(2)), hw.bus.r8(at.wrapping_add(3)));
        *u += 98;
        {
            let v = &mut self.v[i];
            (v.w, v.b2, v.b3) = (w, b2, b3);
        }
        if w != 0xFFFD {
            *u += 19;
        }
        if w != 0xFFFD && w & 0x8000 == 0 {
            *u += 71 + if w & 0x1000 != 0 { 8 } else { 0 } + 27;
            self.v[i].vibpos = 0;
            let mut vol = self.v[i].volume;
            let s = (b2 >> 4) as usize + if w & 0x1000 != 0 { 16 } else { 0 };
            if s != 0 {
                let rec = base.wrapping_add(0x90 + 30 * (s as u32 - 1));
                let start = self.samples[s - 1];
                let v = &mut self.v[i];
                v.start = start;
                v.len = hw.bus.r16(rec.wrapping_add(0x16));
                v.volume = hw.bus.r16(rec.wrapping_add(0x18));
                let ls = hw.bus.r16(rec.wrapping_add(0x1A));
                v.loop_addr = start.wrapping_add(ls as u32);
                v.loop_len = hw.bus.r16(rec.wrapping_add(0x1C));
                vol = v.volume;
                *u += 176 + if ls != 0 { 97 } else { 87 };
            }
            match b2 & 15 {
                0xC => {
                    vol = b3 as u16;
                    *u += 55;
                }
                5 => {
                    vol = vol.wrapping_add(b3 as u16);
                    *u += 93;
                    if vol as i16 > 64 {
                        vol = 64;
                        *u += 14;
                    }
                }
                6 => {
                    vol = vol.wrapping_sub(b3 as u16);
                    *u += 106;
                    if (vol as i16) < 0 {
                        vol = 0;
                        *u += 14;
                    }
                }
                _ => *u += 73,
            }
            *u += 46 + 23 * (i as u64 + 1) + 133;
            stamp(hw, e, *u);
            // the volume stage: masked, times the channel gain (64), shifted down by 6
            let out = vol & 0x7F;
            hw.vol(i, out);
            self.v[i].volume = out;
            *u += if i == 3 { 40 } else { 50 };
            *u += 12;
        }
        match w {
            0xFFFD => {
                *u += 79;
                self.v[i].b2 = 0;
                self.v[i].b3 = 0;
            }
            0xFFFB => {
                *u += 80;
                self.v[i].w = 0;
            }
            0xFFFC => {
                *u += 121;
                self.brk = true;
                self.v[i].w = 0;
            }
            0 => *u += 112,
            _ => {
                self.v[i].step = 0;
                self.v[i].period = w & 0xEFFF;
                *u += 156;
                stamp(hw, e, *u);
                hw.dmacon(bit);
                if w == 0xFFFE {
                    *u += 53;
                    stamp(hw, e, *u);
                    hw.vol(i, 0);
                    self.v[i].volume = 0;
                    *u += 10;
                } else {
                    let v = self.v[i];
                    *u += 45;
                    stamp(hw, e, *u);
                    hw.loc(i, v.start);
                    *u += 20;
                    stamp(hw, e, *u);
                    hw.len(i, v.len);
                    *u += 28;
                    stamp(hw, e, *u);
                    hw.per(i, w & 0xEFFF);
                    *u += [145, 176, 207, 215][i];
                }
                *u += 68;
                *mask |= bit;
            }
        }
    }

    fn effect_tick(&mut self, hw: &mut Hw, e: u64, u: &mut u64) {
        *u += 24;
        for i in 0..4 {
            *u += 18;
            self.effect_voice(i, hw, e, u);
            *u += 34;
        }
    }

    fn effect_voice(&mut self, i: usize, hw: &mut Hw, e: u64, u: &mut u64) {
        let timer = self.timer;
        let v = &mut self.v[i];
        *u += 21;
        if v.step != 0 {
            *u += 74;
            v.stepcur = v.stepcur.wrapping_add(v.step as u16);
            v.period = v.stepcur;
            let go = if v.step > 0 { v.target > v.period } else { (v.target as i16) < (v.period as i16) };
            if !go {
                v.step = 0;
                v.period = v.target;
                *u += 32;
            }
            *u += 12;
            stamp(hw, e, *u);
            hw.per(i, v.period);
            v.stepcur = v.period;
            *u += 28;
            return;
        }
        let (cmd, b3) = (v.b2 & 15, v.b3);
        match cmd {
            0 => *u += 49,
            3 | 5 | 6 | 9..=0xE => *u += 98,
            _ => {
                *u += 102;
                match cmd {
                    1 => {
                        *u += 43 + if timer <= 4 { 40 } else { 0 };
                        if timer == 3 {
                            *u += 24;
                            stamp(hw, e, *u);
                            hw.per(i, v.period);
                        } else {
                            let n = if timer == 2 || timer == 4 {
                                *u += 34;
                                (b3 & 15) as i32
                            } else {
                                *u += 40;
                                (b3 >> 4) as i32
                            };
                            *u += 24;
                            for k in 0..(PAD + ENTRIES + 1) as i32 {
                                *u += 31;
                                let t = tab(k);
                                if t == 0xFFFF {
                                    break;
                                }
                                *u += 17;
                                if t == v.period {
                                    *u += 12;
                                    stamp(hw, e, *u);
                                    hw.per(i, tab(k + n));
                                    break;
                                }
                                *u += 18;
                            }
                        }
                        // (the arpeggio's own 16, besides the one every command ends with)
                        *u += 16;
                    }
                    2 => {
                        *u += 43;
                        let (hi, lo) = ((b3 >> 4) as u16, (b3 & 15) as u16);
                        let d = if hi != 0 {
                            *u += 80;
                            Some(hi)
                        } else if lo != 0 {
                            *u += 117;
                            Some(lo.wrapping_neg())
                        } else {
                            *u += 53;
                            None
                        };
                        if let Some(d) = d {
                            let p = v.period;
                            let q = (p & 0xEFFF).wrapping_add(d);
                            stamp(hw, e, *u);
                            hw.per(i, q);
                            v.period = q | (p & 0x1000);
                            *u += 32;
                        }
                    }
                    4 => {
                        *u += 25;
                        if b3 != 0 {
                            *u += 12;
                            v.vibparam = b3;
                        }
                        *u += 175;
                        let x = SINE[((v.vibpos >> 2) & 31) as usize] as u16;
                        let delta = (x * (v.vibparam & 15) as u16) >> 7;
                        let per = if v.vibpos >= 0x80 {
                            *u += 4;
                            v.period.wrapping_sub(delta)
                        } else {
                            *u += 14;
                            v.period.wrapping_add(delta)
                        };
                        *u += 12;
                        stamp(hw, e, *u);
                        hw.per(i, per);
                        v.vibpos = v.vibpos.wrapping_add((v.vibparam >> 4).wrapping_mul(4));
                        *u += 62;
                    }
                    7 | 8 => {
                        let down = cmd == 8;
                        *u += if down { 15 } else { 4 };
                        *u += 69 + if down { 4 } else { 0 } + 62;
                        v.stepcur = v.period;
                        let s = (b3 & 15) as i16;
                        v.step = if down { -s } else { s };
                        let k = (b3 >> 4) as i32;
                        v.target = v.period;
                        for j in 0..(PAD + ENTRIES + 1) as i32 {
                            *u += 27;
                            let t = tab(j);
                            if t == 0xFFFF {
                                *u += 28;
                                break;
                            }
                            *u += 13;
                            if t == v.period {
                                v.target = tab(if down { j + k } else { j - k });
                                *u += 17 + if down { 0 } else { 4 } + 42;
                                break;
                            }
                            *u += 18;
                        }
                    }
                    _ => {
                        // F: the speed; 0 becomes 1
                        *u += 33;
                        let mut s = (b3 & 0x1F) as u16;
                        if s == 0 {
                            s = 1;
                            *u += 4;
                        }
                        self.speed = s;
                        *u += 32;
                    }
                }
                *u += 16;
            }
        }
    }
}

impl Replayer for MultiMediaSound {
    fn format(&self) -> &'static str {
        "MultiMedia Sound"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    /// The module's tempo word is the CIA timer value.
    fn tick_clocks(&self) -> u64 {
        (self.tempo as u64 + 1) * 5
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subs.len() as u16 - 1, self.cur)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.init_song(n, hw.bus);
        self.songend = false;
        self.next_at = first_tick_hook().or(Some(hw.bus.clock + self.tick_clocks()));
    }

    fn tick(&mut self, hw: &mut Hw) {
        let e = hw.bus.clock;
        let mut u = 415u64;
        self.timer = self.timer.wrapping_add(1);
        if self.timer == self.speed {
            self.timer = 0;
            self.row_tick(hw, e, &mut u);
        } else {
            self.effect_tick(hw, e, &mut u);
        }
        stamp(hw, e, u);
    }
}

#[cfg(test)]
#[path = "multimediasound_tests.rs"]
mod tests;
