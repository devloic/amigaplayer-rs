//! Richard Joseph Player (`rjp.*` with a sample companion `smp.*`), the music of
//! James Pond 2, The Chaos Engine, Cannon Fodder, Brutal Football and other Amiga
//! games by Richard Joseph.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour, and this player was written from it alone and checked against the
//! Paula register writes of UADE's Richard Joseph player.
//!
//! A song file (`RJP?SMOD`) is seven length-prefixed blocks: the instrument records,
//! the envelopes, the subsong table, and the offset tables of the position lists and
//! of the patterns, then those lists and patterns themselves. A subsong gives each of
//! the four voices a position list; a voice walks its list of pattern numbers and
//! plays the notes and commands in them. A note starts a sample of the companion
//! file, with an envelope, an optional vibrato (pitch) table and tremolo (volume)
//! table, and a pitch slide; all sound is Paula DMA, started in three steps over
//! three ticks (stop, enable, install the loop). The tick is 50 Hz.

use super::{be16, be32, Hw, Open, Replayer, WriteGaps};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Richard Joseph Player", detect, new };

/// Paula periods of the notes (the note byte is twice the index).
const PERIODS: [u16; 36] = [
    453, 480, 508, 538, 570, 604, 640, 678, 720, 762, 808, 856, 226, 240, 254, 269, 285, 302, 320, 339, 360, 381, 404,
    428, 113, 120, 127, 135, 143, 151, 160, 170, 180, 190, 202, 214,
];

/// Row parsing and list walks that find nothing to play stop the voice after this
/// many steps rather than hang.
const GUARD: u32 = 20000;

/// Recognition by content: `RJP`, any version byte, `SMOD`, and a zero long where
/// the first instrument record (the empty instrument) starts.
fn detect(d: &[u8]) -> bool {
    d.len() >= 16 && d[..3] == *b"RJP" && d[4..8] == *b"SMOD" && be32(d, 12) == 0
}

/// The seven blocks of a song as (offset of the data, length), if they all fit.
fn blocks(d: &[u8]) -> Option<[(usize, usize); 7]> {
    let mut p = 8usize;
    let mut b = [(0, 0); 7];
    for e in b.iter_mut() {
        let len = d.get(p..p + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))? as usize;
        *e = (p + 4, len);
        p = p.checked_add(4)?.checked_add(len)?;
    }
    (p <= d.len()).then_some(b)
}

#[derive(Clone, Copy, Default)]
struct Voice {
    /// The pattern read pointer, if the voice has one.
    pat: Option<usize>,
    /// The position list read pointer.
    lst: usize,
    speed: u8,
    repeat: u8,
    cnt: u8,
    rep: u8,
    active: bool,
    /// The current instrument number and the record it selected, if any.
    ins: u16,
    rec: Option<usize>,
    sample: u32,
    loop_start: u16,
    loop_len: u16,
    vib: u32,
    trem: u32,
    vib_restart: u16,
    vib_len: u16,
    trem_restart: u16,
    trem_len: u16,
    ivol: u16,
    base_per: u16,
    per: u16,
    ppos: u16,
    tpos: u16,
    work: i16,
    // the envelope: phase 0 none, 4 attack, 2 decay, 255 release
    phase: u8,
    end: u8,
    delta: i8,
    c: u8,
    d: u8,
    level: u8,
    env: Option<usize>,
    acc: u32,
    rate: u32,
    slide: u8,
    keep: bool,
    dma: bool,
    install: bool,
}

pub struct Rjp {
    d: Vec<u8>,
    blk: [(usize, usize); 7],
    title: String,
    subsongs: u16,
    sub: u16,
    /// The sample area as loaded, and the one being played (the player clears words
    /// in it), mirrored in guest memory at `base`.
    pristine: Vec<u8>,
    area: Vec<u8>,
    base: u32,
    v: [Voice; 4],
    /// Voices that have not yet read their list's end since the last song end.
    flags: u8,
    flags0: u8,
    songend: bool,
    /// The next tick is the first of the song (the original's start-up routine runs in it).
    first: bool,
}

/// The name the sample companion has: `smp.NAME` for `rjp.NAME` (the prefix in any
/// case), `NAME.ins` for `NAME.sng`.
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    match file.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("rjp.") => v.push(format!("smp.{}", &file[4..])),
        _ => {
            if let Some(s) = file.strip_suffix(".sng").or_else(|| file.strip_suffix(".SNG")) {
                v.push(format!("{s}.ins"));
            }
        }
    }
    v.push("smp.set".to_string());
    v
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let blk = blocks(d)?;
    let mut d = d.to_vec();
    // the two known-bad files are repaired, as the reference player does, by the
    // size of the song
    let size = blk[6].0 + blk[6].1;
    if size == 9506 && be16(&d, 2650) == 2 {
        d[2650..2652].fill(0);
    }
    if size == 2989 && be32(&d, 1192) == 0x0805_0706 {
        d[1192..1196].fill(0);
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut sample = None;
    for name in companion(&file) {
        if let Ok(s) = crate::deli::read_companion(m, &name).1 {
            sample = Some(s);
            break;
        }
    }
    let sample = sample?;
    // the sample area starts after the first four bytes of the file
    let mut pristine = sample.get(4..).unwrap_or(&[]).to_vec();
    // records may reach past the file's end: such reads give zero
    let mut reach = pristine.len();
    for i in 0..blk[0].1 / 32 {
        let r = blk[0].0 + 32 * i;
        let o = be32(&d, r) as usize;
        let words = be16(&d, r + 0x10).max(be16(&d, r + 0x14)) as usize + be16(&d, r + 0x12).max(be16(&d, r + 0x16)) as usize;
        reach = reach.max((o + 2 * words).min(pristine.len() + 0x40000));
    }
    pristine.resize(reach, 0);
    let base = m.alloc((pristine.len() as u32).max(4));
    if base == 0 {
        return None;
    }
    m.bus.write_bytes(base, &pristine);
    let subsongs = ((blk[2].1 / 4) as u16).max(1);
    let mut r = Rjp {
        d,
        blk,
        title: String::new(),
        subsongs,
        sub: 0,
        area: pristine.clone(),
        pristine,
        base,
        v: Default::default(),
        flags: 0,
        flags0: 0,
        songend: false,
        first: false,
    };
    r.title = file;
    let mut hw = Hw { bus: &mut m.bus };
    hw.dmacon(0x000F);
    r.start(0, &mut hw);
    Some(Box::new(r))
}

impl Rjp {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// The offset of record `n` of instrument block, or of the empty one if `n` is
    /// past the block.
    fn record(&self, n: u16) -> usize {
        let (o, len) = self.blk[0];
        if 32 * n as usize >= len {
            o
        } else {
            o + 32 * n as usize
        }
    }

    /// The offset of position list `n`.
    fn list(&self, n: u8) -> usize {
        self.blk[5].0.wrapping_add(be32(&self.d, self.blk[3].0 + 4 * n as usize) as usize)
    }

    fn pattern(&self, n: u8) -> usize {
        self.blk[6].0.wrapping_add(be32(&self.d, self.blk[4].0 + 4 * n as usize) as usize)
    }

    /// Stop, then the start of subsong `n`.
    fn start(&mut self, n: u16, hw: &mut Hw) {
        hw.dmacon(0x000F);
        self.sub = n;
        self.songend = false;
        self.first = true;
        // a new start finds the samples as loaded
        self.area.copy_from_slice(&self.pristine);
        hw.bus.write_bytes(self.base, &self.pristine);
        self.v = Default::default();
        self.flags = 0;
        let row = self.blk[2].0 + 4 * (n & 63) as usize;
        for ch in 0..4 {
            let l = self.rb(row + ch);
            if l == 0 {
                continue;
            }
            self.flags |= 1 << ch;
            let p = self.list(l);
            let first = self.rb(p);
            self.v[ch] = Voice {
                pat: Some(self.pattern(first)),
                lst: p + 1,
                speed: 6,
                cnt: 1,
                rep: 1,
                active: true,
                ..Default::default()
            };
        }
        self.flags0 = self.flags;
    }

    /// A voice reached the end of its list.
    fn list_end(&mut self, ch: usize) {
        self.flags &= !(1 << ch);
        if self.flags == 0 {
            self.songend = true;
            self.flags = self.flags0;
        }
    }

    fn select(&mut self, ch: usize, n: u8) {
        if self.v[ch].ins == n as u16 && self.v[ch].rec.is_some() {
            return;
        }
        let r = self.record(n as u16);
        let d = &self.d;
        let w = |o: usize| be16(d, r + o);
        let l = |o: usize| be32(d, r + o);
        let v = &mut self.v[ch];
        v.ins = n as u16;
        v.rec = Some(r);
        v.sample = l(0);
        v.vib = l(4);
        v.trem = l(8);
        v.ivol = w(0x0E);
        v.loop_start = w(0x14);
        v.loop_len = w(0x16);
        v.vib_restart = w(0x18);
        v.vib_len = w(0x1A);
        v.trem_restart = w(0x1C);
        v.trem_len = w(0x1E);
        v.ppos = 0;
        v.tpos = 0;
    }

    /// A note of period `p`: stop the channel and set up its first segment.
    fn note(&mut self, ch: usize, p: u16, hw: &mut Hw) {
        let Some(r) = self.v[ch].rec else { return };
        let env = self.blk[1].0 + be16(&self.d, r + 0x0C) as usize;
        let e: [u8; 5] = std::array::from_fn(|i| self.rb(env + i));
        let start = (be16(&self.d, r + 0x10).wrapping_mul(2)) as u32;
        let len = be16(&self.d, r + 0x12);
        let v = &mut self.v[ch];
        v.base_per = p;
        v.per = p;
        if !v.keep {
            v.acc = 0;
            v.slide = 0;
        }
        v.env = Some(env);
        v.end = e[1];
        v.delta = e[1].wrapping_sub(e[0]) as i8;
        v.c = e[2];
        v.d = e[2];
        v.phase = 4;
        let s = v.sample.wrapping_add(start);
        hw.dmacon(1 << ch);
        hw.bus.write_word(0xDFF0AA + 16 * ch as u32, 0);
        if (s as usize) + 2 <= self.area.len() {
            self.area[s as usize..s as usize + 2].fill(0);
            hw.bus.write_bytes(self.base + s, &[0, 0]);
        }
        hw.loc(ch, self.base.wrapping_add(s));
        hw.len(ch, len);
        self.v[ch].dma = true;
    }

    /// The next entry of the voice's position list: a pattern to read from, or the
    /// voice's end.
    fn next_pattern(&mut self, ch: usize) -> Option<usize> {
        for _ in 0..GUARD {
            let p = self.v[ch].lst;
            let n = self.rb(p);
            self.v[ch].lst = p + 1;
            if n != 0 {
                return Some(self.pattern(n));
            }
            self.list_end(ch);
            let c = self.rb(p + 1);
            match c {
                0 => return None,
                1..=0x7F => self.v[ch].lst = (p + 1).wrapping_sub(c as usize),
                _ => {
                    let l = self.rb(p + 2);
                    self.v[ch].lst = self.list(l);
                }
            }
        }
        None
    }

    /// Reads one row of the voice's pattern.
    fn row(&mut self, ch: usize, hw: &mut Hw) {
        for _ in 0..GUARD {
            let Some(p) = self.v[ch].pat else {
                self.v[ch].active = false;
                return;
            };
            let b = self.rb(p);
            self.v[ch].pat = Some(p + 1);
            match b {
                0..=0x7F => {
                    let per = PERIODS.get(b as usize / 2).copied().unwrap_or(0);
                    self.note(ch, per, hw);
                    return;
                }
                0x80 => {
                    self.v[ch].repeat = 1;
                    match self.next_pattern(ch) {
                        Some(q) => self.v[ch].pat = Some(q),
                        None => {
                            self.v[ch].pat = None;
                            self.v[ch].active = false;
                            return;
                        }
                    }
                }
                0x81 => {
                    let rel = self.v[ch].env.map(|e| self.rb(e + 5));
                    if let Some(rel) = rel {
                        let v = &mut self.v[ch];
                        v.end = 0;
                        v.delta = 0u8.wrapping_sub(v.level) as i8;
                        v.c = rel;
                        v.d = rel;
                        v.phase = 255;
                    }
                    return;
                }
                0x82 => {
                    self.v[ch].speed = self.rb(p + 1);
                    self.v[ch].pat = Some(p + 2);
                }
                0x83 => {
                    self.v[ch].repeat = self.rb(p + 1);
                    self.v[ch].pat = Some(p + 2);
                }
                0x84 => {
                    let n = self.rb(p + 1);
                    self.v[ch].pat = Some(p + 2);
                    if n != 0 {
                        self.select(ch, n);
                    }
                }
                0x85 => {
                    self.v[ch].ivol = self.rb(p + 1) as u16;
                    self.v[ch].pat = Some(p + 3);
                }
                0x86 => {
                    let v = &mut self.v[ch];
                    v.slide = self.d.get(p + 1).copied().unwrap_or(0);
                    v.rate = be32(&self.d, p + 2);
                    v.acc = 0;
                    v.keep = true;
                    v.pat = Some(p + 6);
                }
                0x87 => return,
                // undefined commands: skipped
                _ => {}
            }
        }
        self.v[ch].active = false;
    }

    fn sequence(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.v[ch];
        v.cnt = v.cnt.wrapping_sub(1);
        if v.cnt != 0 {
            return;
        }
        v.rep = v.rep.wrapping_sub(1);
        if v.rep != 0 {
            v.cnt = v.speed;
            return;
        }
        self.row(ch, hw);
        let v = &mut self.v[ch];
        v.rep = v.repeat;
        v.cnt = v.speed;
    }

    /// The envelope, tremolo and scaling; the volume to write.
    fn volume(&mut self, ch: usize) -> u16 {
        let v = &mut self.v[ch];
        let mut refresh = true;
        if v.phase != 0 {
            let q = if v.d == 0 { 0 } else { v.delta as i32 * v.c as i32 / v.d as i32 };
            v.level = (v.end as i32).wrapping_sub(q) as u8;
            v.c = v.c.wrapping_sub(1);
            if v.c == 255 {
                match v.phase {
                    4 => {
                        let env = v.env.unwrap_or(0);
                        let e: [u8; 5] = std::array::from_fn(|i| self.d.get(env + i).copied().unwrap_or(0));
                        let v = &mut self.v[ch];
                        v.end = e[3];
                        v.delta = e[3].wrapping_sub(e[1]) as i8;
                        v.c = e[4];
                        v.d = e[4];
                        v.phase = 2;
                    }
                    2 => v.phase = 0,
                    _ => {
                        v.phase = 0;
                        refresh = false;
                    }
                }
            }
        }
        let v = &mut self.v[ch];
        if refresh {
            v.work = v.level as i16;
        }
        if v.trem != 0 {
            let t = self.area.get(v.trem as usize + v.tpos as usize).copied().unwrap_or(0) as i8 as i32;
            let w = v.work as i32;
            v.work = (w + ((t * w) >> 7)) as i16;
            v.tpos = v.tpos.wrapping_add(1);
            if v.tpos == v.trem_len.wrapping_mul(2) {
                v.tpos = v.trem_restart.wrapping_mul(2);
            }
        }
        let w = ((v.work as i32 * v.ivol as i32) as i16) >> 6;
        let w = ((w as i32 * 64) as i16) >> 6;
        v.work = w;
        w.clamp(0, 64) as u16
    }

    /// The period to write: vibrato, then the slide.
    fn period(&mut self, ch: usize) -> u16 {
        let v = &mut self.v[ch];
        if v.vib != 0 {
            let t = self.area.get(v.vib as usize + v.ppos as usize).copied().unwrap_or(0) as i8 as i32;
            let x = (t * v.base_per as i32) >> 7;
            let mut y = (-x) as i16;
            if y < 0 {
                y >>= 1;
            }
            v.per = v.base_per.wrapping_add(y as u16);
            v.ppos = v.ppos.wrapping_add(1);
            if v.ppos == v.vib_len.wrapping_mul(2) {
                v.ppos = v.vib_restart.wrapping_mul(2);
            }
        }
        if v.slide != 0 {
            v.acc = v.acc.wrapping_add(v.rate);
            v.slide -= 1;
        }
        ((v.acc >> 16) as u16).wrapping_add(v.per)
    }

    fn step(&mut self, ch: usize, hw: &mut Hw) {
        self.v[ch].keep = false;
        if self.v[ch].install {
            self.v[ch].install = false;
            let v = &self.v[ch];
            let a = self.base.wrapping_add(v.sample).wrapping_add(2 * v.loop_start as u32);
            let len = v.loop_len;
            hw.loc(ch, a);
            hw.len(ch, len);
        }
        if self.v[ch].dma {
            self.v[ch].dma = false;
            hw.dmacon(0x8200 | 1 << ch);
            self.v[ch].install = true;
        }
        self.sequence(ch, hw);
        let vol = self.volume(ch);
        hw.vol(ch, vol);
        let per = self.period(ch);
        hw.per(ch, per);
    }
}

/// Where the writes of a tick fall in the original's runs, by the previous write (see `WriteGaps`):
/// the voices' work between two writes, and the tick's first write (the first write of a tick sits at its start
/// gap).
static GAPS: WriteGaps = WriteGaps::new(
    &[
        (0, 4, 10), // LOC0 -> LEN0 x628
        (1, 5, 10), // LOC1 -> LEN1 x494
        (2, 6, 10), // LOC2 -> LEN2 x602
        (3, 7, 10), // LOC3 -> LEN3 x270
        (4, 12, 575), // LEN0 -> VOL0 x628
        (5, 13, 583), // LEN1 -> VOL1 x494
        (6, 14, 660), // LEN2 -> VOL2 x602
        (7, 15, 682), // LEN3 -> VOL3 x270
        (8, 1, 204), // PER0 -> LOC1 x249
        (8, 13, 729), // PER0 -> VOL1 x3413
        (8, 16, 500), // PER0 -> DMA_OFF x241
        (8, 20, 232), // PER0 -> DMA_ON x240
        (9, 2, 220), // PER1 -> LOC2 x302
        (9, 14, 748), // PER1 -> VOL2 x2821
        (9, 16, 515), // PER1 -> DMA_OFF x297
        (9, 20, 247), // PER1 -> DMA_ON x295
        (10, 3, 236), // PER2 -> LOC3 x135
        (10, 15, 777), // PER2 -> VOL3 x2820
        (10, 16, 531), // PER2 -> DMA_OFF x132
        (10, 20, 263), // PER2 -> DMA_ON x131
        (12, 8, 137), // VOL0 -> PER0 x4439
        (13, 9, 152), // VOL1 -> PER1 x4173
        (14, 10, 167), // VOL2 -> PER2 x3724
        (15, 11, 172), // VOL3 -> PER3 x3231
        (16, 0, 44), // DMA_OFF -> LOC0 x310
        (16, 1, 44), // DMA_OFF -> LOC1 x243
        (16, 2, 44), // DMA_OFF -> LOC2 x298
        (16, 3, 44), // DMA_OFF -> LOC3 x133
        (20, 12, 512), // DMA_ON -> VOL0 x309
        (20, 13, 521), // DMA_ON -> VOL1 x242
        (20, 14, 525), // DMA_ON -> VOL2 x297
        (20, 15, 538), // DMA_ON -> VOL3 x133
    ],
    &[(0, 44), (1, 204), (2, 220), (3, 235), (4, 10), (5, 10), (6, 10), (7, 10), (8, 137), (9, 152), (10, 167), (11, 172), (12, 552), (13, 729), (14, 748), (15, 682), (16, 530), (20, 247)],
    &[(0, 0), (1, 89), (2, 177), (3, 266), (12, 516), (13, 613), (14, 705), (15, 807), (16, 576), (20, 28)],
);

/// The tick's first write falls this far into it (the start gap of a stop of DMA below).
const START_FIRST: u64 = 576;

/// Colour clocks from the song's start to the host's timer start, beyond the first period: the
/// original's init takes longer than the port's by this much, over the files at the median.
const TIMER_DELAY: u64 = 14131;

impl Replayer for Rjp {
    fn format(&self) -> &'static str {
        "Richard Joseph Player"
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
        (0, self.subsongs - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.start(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.flags0 == 0 {
            // a subsong without voices ends at every tick
            self.songend = true;
            return;
        }
        if self.first {
            // the start-up routine of the first tick: all voices off, the volumes cleared, the
            // DMA stopped again (measured in the reference runs: the first voice's stop comes some
            // 8,000 colour clocks into the tick, which fixes how long the channels stay in the
            // state the data-register write leaves them in)
            self.first = false;
            hw.wait(START_FIRST);
            hw.dmacon(0x000F);
            for ch in 0..4 {
                hw.vol(ch, 0);
                hw.wait(6);
            }
            hw.wait(4648);
            hw.dmacon(0x000F);
            hw.wait(3351 - START_FIRST);
        }
        hw.gaps(&GAPS);
        for ch in 0..4 {
            if self.v[ch].active {
                self.step(ch, hw);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn block(d: &mut Vec<u8>, b: &[u8]) {
        d.extend_from_slice(&(b.len() as u32).to_be_bytes());
        d.extend_from_slice(b);
    }

    /// One voice (list 1: pattern 1 then stop): instrument 1 with envelope 0, a
    /// note, a rest, a note off; the sample area is 64 bytes of 0x40.
    fn song() -> Vec<u8> {
        let mut d = b"RJP1SMOD".to_vec();
        let mut ins = vec![0u8; 64];
        // record 0: the empty instrument
        ins[0x12 + 1] = 1;
        ins[0x16 + 1] = 1;
        ins[0x1A + 1] = 1;
        ins[0x1E + 1] = 1;
        // record 1: sample at 0, volume 64, first segment 8 words, loop at 4 of 4 words
        let r = 32;
        ins[r + 0x0E + 1] = 64;
        ins[r + 0x12 + 1] = 8;
        ins[r + 0x14 + 1] = 4;
        ins[r + 0x16 + 1] = 4;
        block(&mut d, &ins);
        block(&mut d, &[64, 64, 254, 64, 254, 254]);
        block(&mut d, &[1, 0, 0, 0]);
        block(&mut d, &[0, 0, 0, 0, 0, 0, 0, 0]);
        block(&mut d, &[0, 0, 0, 0, 0, 0, 0, 0]);
        block(&mut d, &[1, 0, 0, 0]);
        // pattern 1: instrument 1, speed 1, repeat 1, note index 19 (period 339),
        // rest, note off, end of pattern
        block(&mut d, &[0x84, 1, 0x82, 1, 0x83, 1, 38, 0x87, 0x81, 0x80]);
        // fix up the tables: list 1 at block 6 offset 0, pattern 1 at offset 0
        let b = blocks(&d).unwrap();
        d[b[3].0 + 4..b[3].0 + 8].copy_from_slice(&0u32.to_be_bytes());
        d[b[4].0 + 4..b[4].0 + 8].copy_from_slice(&0u32.to_be_bytes());
        d
    }

    #[test]
    fn recognises_by_content() {
        let d = song();
        assert!(detect(&d));
        assert!(blocks(&d).is_some());
        let mut e = d.clone();
        e[3] = b'3';
        assert!(detect(&e));
        e[4] = b'X';
        assert!(!detect(&e));
        let mut e = d.clone();
        e[15] = 1;
        assert!(!detect(&e));
        assert!(!detect(&d[..15]));
        assert!(blocks(&d[..d.len() - 1]).is_none());
        assert_eq!(companion("RJP.title"), ["smp.title", "smp.set"]);
        assert_eq!(companion("a.sng"), ["a.ins", "smp.set"]);
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note_in_three_ticks() {
        let dir = Path::new("/rjp-test");
        let mut smp = b"RJP1".to_vec();
        smp.extend_from_slice(&[0x40; 64]);
        let path = dir.join("rjp.test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("SMP.test"), smp);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&path, song()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Richard Joseph Player");
        let mut out = vec![0f32; 2 * 800 * 8];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        // tick 0: DMA off, first segment of 8 words, period 339
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [8, 4]);
        assert_eq!(reg(&log, 0xDFF0AA)[0], 0);
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [339, 339]);
        // tick 1 enables, tick 2 installs the loop of 4 words at word 4
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001) && dma.contains(&0x8201));
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1] - lc[0], 8);
        // the start-up routine clears the volume first; the envelope is flat at 64
        assert_eq!(reg(&log, 0xDFF0A8)[..2], [0, 64]);
    }
}
