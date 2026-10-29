//! MED Packer (`mmdc.*`), the packed MED modules of Captain Planet, Knightmare and
//! other Mindscape games (Antony Crowther, 1991).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's MED Packer player and the structure
//! of the corpus files), and this player was written from it alone and checked
//! against those writes.
//!
//! The module is an MMD0-family MED song (`MMDC`) of four tracks whose blocks are
//! run-length packed. Every row lasts six ticks; the tempo only sets the interval of
//! the timer that calls the player. A note stops the channel, sets up its sample, and
//! at the end of the tick the channel is restarted and the loop part of the sample
//! installed, the DMA being given time to settle in between.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "MED Packer", detect, new };

/// The note periods: three octaves, the last repeated to fill 72 entries.
const PERIODS: [u16; 72] = {
    const OCT: [u16; 36] = [
        856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
        226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
    ];
    let mut t = [0u16; 72];
    let mut i = 0;
    while i < 72 {
        t[i] = if i < 36 { OCT[i] } else { OCT[24 + (i - 36) % 12] };
        i += 1;
    }
    t
};

/// The vibrato's sine.
const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24,
];

/// Timer values of the tempi 1 to 10.
const SLOW_TIMERS: [u16; 10] = [0x971, 0x12E1, 0x1C52, 0x25C2, 0x2F33, 0x38A4, 0x4214, 0x4B84, 0x53BC, 0x5E63];

/// Tracks kept; any beyond are read and ignored.
const TRACKS: usize = 16;

/// Recognition by content: `MMDC`, a block array at an even offset below 32 KiB
/// whose first word (the high half of the first block pointer) is zero, and the
/// declared length no more than the file.
fn detect(d: &[u8]) -> bool {
    if d.len() < 0x40 || d[..4] != *b"MMDC" || be16(d, 16) != 0 {
        return false;
    }
    let b = be16(d, 18) as usize;
    b != 0 && b & 0x8001 == 0 && d.len() >= b + 2 && be16(d, b) == 0 && be32(d, 4) as usize <= d.len()
}

#[derive(Clone, Copy, Default)]
struct Track {
    /// The current sample (0-based), if one was ever chosen.
    sample: Option<usize>,
    vol: i32,
    /// The current note.
    note: u8,
    cmd: u8,
    data: u8,
    /// The period of the slides.
    cur: u16,
    target: u16,
    speed: u8,
    vib: u8,
    phase: u8,
    /// The loop to install after the restart.
    rep_addr: u32,
    rep_len: u16,
}

pub struct Mmdc {
    d: Vec<u8>,
    base: u32,
    /// A zero word to loop on.
    empty: u32,
    title: String,
    /// Offsets of the song structure, the block pointers and the sample pointers.
    song: usize,
    blocks: usize,
    samples: usize,
    tr: [Track; TRACKS],
    pos: usize,
    block: usize,
    line: i32,
    tick: u8,
    flag: u8,
    /// The stream of the block's packed rows: the read offset and what is left of
    /// the run in progress.
    sp: usize,
    lit: usize,
    zeros: usize,
    /// Channels to restart at the end of the tick.
    mask: u8,
    stop: bool,
    timer: u16,
    songend: bool,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let song = be32(d, 8) as usize;
    if song + 0x313 > d.len() {
        return None;
    }
    let empty = m.alloc(4);
    if empty == 0 {
        return None;
    }
    let title = m.bus.read_cstr(m.deli.file_buf);
    let mut r = Mmdc {
        d: d.to_vec(),
        base,
        empty,
        title,
        song,
        blocks: be32(d, 0x10) as usize,
        samples: be32(d, 0x18) as usize,
        tr: Default::default(),
        pos: 0,
        block: 0,
        line: -1,
        tick: 5,
        flag: 0,
        sp: 0,
        lit: 0,
        zeros: 0,
        mask: 0,
        stop: false,
        timer: 0,
        songend: false,
    };
    r.reset();
    // the init routine sets the LED bit of CIA-A's port: the filter is off
    m.bus.ciaa.pra |= 2;
    Some(Box::new(r))
}

impl Mmdc {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// The start of the song (and again after a stop): position 0, before the first
    /// row, the song's current tempo.
    fn reset(&mut self) {
        self.pos = 0;
        self.line = -1;
        self.tick = 5;
        self.flag = 0;
        self.mask = 0;
        self.stop = false;
        self.tr = Default::default();
        let first = self.rb(self.song + 0x1FC);
        self.block = self.clamp_block(first);
        let t = self.rb(self.song + 0x2FD);
        self.set_tempo(t);
    }

    fn clamp_block(&self, n: u8) -> usize {
        let count = self.rb(self.song + 0x1F9);
        n.min(count.saturating_sub(1)) as usize
    }

    fn set_tempo(&mut self, t: u8) {
        let s = self.song;
        self.d[s + 0x2FD] = t;
        match t {
            0 => {}
            1..=10 => {
                self.timer = SLOW_TIMERS[t as usize - 1];
                self.d[s + 0x301] = t - 1;
            }
            _ => self.timer = (470000 / t as u32) as u16,
        }
    }

    fn song_len(&self) -> usize {
        be16(&self.d, self.song + 0x1FA) as usize
    }

    /// The next byte of the block's packed rows.
    fn stream(&mut self) -> u8 {
        loop {
            if self.lit > 0 {
                self.lit -= 1;
                let b = self.rb(self.sp);
                self.sp += 1;
                return b;
            }
            if self.zeros > 0 {
                self.zeros -= 1;
                return 0;
            }
            let h = self.rb(self.sp);
            self.sp += 1;
            if h & 0x80 == 0 {
                self.lit = h as usize + 1;
            } else {
                self.zeros = (!h) as usize + 1;
            }
        }
    }

    fn sample_ptr(&self, n: usize) -> usize {
        if n >= 63 {
            return 0;
        }
        be32(&self.d, self.samples + 4 * n) as usize
    }

    /// The transpose of sample `n`, as a byte.
    fn sample_transpose(&self, n: Option<usize>) -> u8 {
        n.map_or(0, |n| self.rb(self.song + 8 * n + 7))
    }

    /// The volume of `v` scaled by the track and master volumes.
    fn scale(&self, t: usize, v: i32) -> i32 {
        let tv = if t < 16 { self.rb(self.song + 0x302 + t) as i32 } else { 64 };
        (v * tv * self.rb(self.song + 0x312) as i32) >> 12
    }

    fn trigger(&mut self, t: usize, note: u8, hw: &mut Hw) {
        let Some(s) = self.tr[t].sample else { return };
        let ptr = self.sample_ptr(s);
        if ptr == 0 {
            return;
        }
        let mut n = note.wrapping_add(self.rb(self.song + 0x2FE)).wrapping_add(self.sample_transpose(Some(s)));
        if t > 3 {
            return;
        }
        hw.dmacon(1 << t);
        while n >= 0x80 {
            n = n.wrapping_add(12);
        }
        if n > 0x3F {
            n -= 12;
        }
        self.mask |= 1 << t;
        // an index before the table's start is taken as its first entry
        let period = PERIODS[(n as usize).saturating_sub(1).min(71)];
        let rec = self.song + 8 * s;
        let rep = be16(&self.d, rec) as u32;
        let replen = be16(&self.d, rec + 2);
        let len = be32(&self.d, ptr);
        let data = self.base.wrapping_add(ptr as u32 + 6);
        hw.loc(t, data);
        let tr = &mut self.tr[t];
        if replen <= 1 {
            hw.len(t, (len >> 1) as u16);
            tr.rep_addr = self.empty;
            tr.rep_len = 1;
        } else {
            hw.len(t, if rep != 0 { rep as u16 } else { replen });
            tr.rep_addr = data.wrapping_add(2 * rep);
            tr.rep_len = replen;
        }
        tr.cur = period;
        hw.per(t, period);
        hw.vol(t, (tr.vol & 0x7F) as u16);
    }

    /// Reads the next row and starts its notes.
    fn row(&mut self, hw: &mut Hw) {
        self.line += 1;
        let blk = be32(&self.d, self.blocks + 4 * self.block) as usize;
        let lines = self.rb(blk + 1) as i32;
        if self.line > lines || self.flag != 0 {
            self.line = 0;
            if self.flag != 1 {
                self.pos += 1;
            }
            if self.pos >= self.song_len() {
                self.pos = 0;
                self.songend = true;
            }
            let n = self.rb(self.song + 0x1FC + self.pos.min(255));
            self.block = self.clamp_block(n);
            self.flag = 0;
        }
        self.tick = 0;
        let blk = be32(&self.d, self.blocks + 4 * self.block) as usize;
        if self.line == 0 {
            self.sp = blk + 2;
            self.lit = 0;
            self.zeros = 0;
        }
        let tracks = self.rb(blk) as usize;
        for t in 0..tracks {
            let b0 = self.stream();
            let b1 = self.stream();
            let b2 = self.stream();
            if t < TRACKS {
                self.note_on(t, b0, b1, b2, hw);
            }
        }
    }

    fn note_on(&mut self, t: usize, b0: u8, b1: u8, b2: u8, hw: &mut Hw) {
        let ins = ((b0 & 0x80) >> 3) | ((b0 & 0x40) >> 1) | (b1 >> 4);
        let mut note = b0 & 0x3F;
        if ins != 0 {
            let s = ins as usize - 1;
            self.tr[t].sample = Some(s);
            self.tr[t].vib = 0;
            let sv = self.rb(self.song + 8 * s + 6) as i32;
            self.tr[t].vol = self.scale(t, sv);
        }
        let cmd = b1 & 15;
        let d = b2;
        self.tr[t].cmd = cmd;
        self.tr[t].data = d;
        match cmd {
            3 => {
                if note == 0 {
                    return;
                }
                let idx = (note as i32 - 1)
                    + self.rb(self.song + 0x2FE) as i8 as i32
                    + self.sample_transpose(self.tr[t].sample) as i8 as i32;
                if idx < 0 {
                    return;
                }
                self.tr[t].target = PERIODS[(idx as usize).min(71)];
                self.tr[t].speed = d;
                note = 0;
            }
            0xB => {
                if d as usize <= self.song_len() {
                    self.pos = d as usize;
                    self.flag = 1;
                }
            }
            0xC => {
                let v = ((d >> 4) as i32 * 10 + (d & 15) as i32).min(64);
                self.tr[t].vol = self.scale(t, v);
            }
            0xF => match d {
                0 => self.flag = 0xFF,
                1..=0xF0 => self.set_tempo(d),
                0xF2 => {
                    if note != 0 {
                        self.tr[t].note = note;
                    }
                    note = 0;
                }
                0xFE => self.stop = true,
                0xFD => {
                    if note != 0 && t <= 3 {
                        hw.per(t, PERIODS[(note as usize - 1).min(71)]);
                    }
                    note = 0;
                }
                _ => {}
            },
            _ => {}
        }
        if note != 0 {
            self.tr[t].note = note;
            self.trigger(t, note, hw);
        }
    }

    /// The effects of every tick.
    fn effects(&mut self, t: usize, hw: &mut Hw) {
        let tick = self.tick;
        let d = self.tr[t].data;
        let gate = self.rb(self.song + 0x2FF) & 0x20 != 0 && self.rb(self.song + 0x301) <= tick;
        let vol = (self.tr[t].vol & 0x7F) as u16;
        match self.tr[t].cmd {
            0 if d != 0 => {
                let n0 = self.tr[t].note;
                let n = match tick {
                    0 | 3 => n0.wrapping_add(d & 15),
                    1 | 4 => n0.wrapping_add(d >> 4),
                    _ => n0,
                };
                let idx = n
                    .wrapping_sub(1)
                    .wrapping_add(self.rb(self.song + 0x2FE))
                    .wrapping_add(self.sample_transpose(self.tr[t].sample));
                hw.per(t, PERIODS[((idx as usize) & 0x7F).min(71)]);
                hw.vol(t, vol);
            }
            1 if !gate => {
                let tr = &mut self.tr[t];
                tr.cur = (tr.cur as i32 - d as i32).max(0x71) as u16;
                hw.per(t, tr.cur);
                hw.vol(t, vol);
            }
            2 if !gate => {
                let tr = &mut self.tr[t];
                tr.cur = (tr.cur as i32 + d as i32).min(0x358) as u16;
                hw.per(t, tr.cur);
                hw.vol(t, vol);
            }
            3 if !gate => {
                let tr = &mut self.tr[t];
                if tr.target != 0 {
                    let (c, g, s) = (tr.cur as i32, tr.target as i32, tr.speed as i32);
                    if c > g {
                        tr.cur = (c - s).max(g) as u16;
                    } else {
                        tr.cur = (c + s).min(g) as u16;
                    }
                    if tr.cur == tr.target {
                        tr.target = 0;
                    }
                }
                hw.per(t, tr.cur);
                hw.vol(t, vol);
            }
            4 => {
                let tr = &mut self.tr[t];
                if d != 0 {
                    tr.vib = d;
                }
                let delta = ((SINE[(tr.phase as usize >> 1) & 31] as u32 * (tr.vib & 15) as u32) >> 6) as u16;
                let per = if tr.phase & 0x40 != 0 { tr.cur.wrapping_sub(delta) } else { tr.cur.wrapping_add(delta) };
                tr.phase = tr.phase.wrapping_add((tr.vib >> 4) + 1);
                hw.per(t, per);
            }
            5 => {
                let tr = &self.tr[t];
                let per = if tick >= 3 { tr.cur } else { tr.cur.wrapping_sub(d as u16) };
                hw.per(t, per);
                hw.vol(t, vol);
            }
            0xA | 0xD if !gate => {
                let tr = &mut self.tr[t];
                if d >> 4 == 0 {
                    tr.vol = (tr.vol - d as i32).max(0);
                } else {
                    tr.vol = (tr.vol + (d >> 4) as i32).min(64);
                }
                hw.per(t, tr.cur);
                hw.vol(t, (tr.vol & 0x7F) as u16);
            }
            0xC if tick == 0 => {
                hw.per(t, self.tr[t].cur);
                hw.vol(t, vol);
            }
            0xF => match d {
                0xFF => hw.dmacon(1 << t),
                0xF1 | 0xF2 if tick == 3 => self.retrigger(t, hw),
                0xF3 if tick >= 2 => self.retrigger(t, hw),
                _ => {}
            },
            _ => {}
        }
    }

    fn retrigger(&mut self, t: usize, hw: &mut Hw) {
        let n = self.tr[t].note;
        self.trigger(t, n, hw);
    }

    /// The end of a tick: the channels started are enabled and their loops installed.
    fn restart(&mut self, hw: &mut Hw) {
        if self.mask == 0 {
            return;
        }
        hw.wait(2400);
        hw.dmacon(0x8000 | self.mask as u16);
        hw.wait(2000);
        for t in 0..4 {
            if self.mask & 1 << t != 0 {
                hw.loc(t, self.tr[t].rep_addr);
                hw.len(t, self.tr[t].rep_len);
                hw.wait(30);
            }
        }
        self.mask = 0;
    }
}

impl Replayer for Mmdc {
    fn format(&self) -> &'static str {
        "MED Packer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn set_subsong(&mut self, _n: u16, _hw: &mut Hw) {
        self.songend = false;
        self.reset();
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.stop {
            // a stop command ends the song and starts it over at the next tick
            self.songend = true;
            self.reset();
            return;
        }
        hw.wait(900);
        self.tick += 1;
        if self.tick >= 6 {
            self.row(hw);
        }
        let tracks = self.tr.len().min(4);
        for t in 0..tracks {
            self.effects(t, hw);
        }
        self.restart(hw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// The smallest song: song struct at 0x34, one block of 4 tracks and 2 lines,
    /// one sample (length 8 bytes, loop from word 1 for 3 words, or unlooped).
    fn song(looped: bool, rows: &[u8], tempo: u8) -> Vec<u8> {
        let mut d = vec![0u8; 0x600];
        d[..4].copy_from_slice(b"MMDC");
        let s = 0x34usize;
        d[8..12].copy_from_slice(&(s as u32).to_be_bytes());
        // block array at 0x400, sample array at 0x404, block at 0x440, sample at 0x500
        d[0x10..0x14].copy_from_slice(&0x400u32.to_be_bytes());
        d[0x18..0x1C].copy_from_slice(&0x404u32.to_be_bytes());
        d[0x400..0x404].copy_from_slice(&0x440u32.to_be_bytes());
        d[0x404..0x408].copy_from_slice(&0x500u32.to_be_bytes());
        // sample 1: repeat start 1 word, length 3 words (or none), volume 64
        if looped {
            d[s..s + 2].copy_from_slice(&1u16.to_be_bytes());
            d[s + 2..s + 4].copy_from_slice(&3u16.to_be_bytes());
        }
        d[s + 6] = 64;
        d[s + 0x1F9] = 1;
        d[s + 0x1FB] = 1; // song length 1
        d[s + 0x2FD] = tempo;
        d[s + 0x302..s + 0x312].fill(64);
        d[s + 0x312] = 64;
        // sample data: length 8 bytes
        d[0x500..0x504].copy_from_slice(&8u32.to_be_bytes());
        // block: 4 tracks, 2 lines, literal runs of the rows given
        d[0x440] = 4;
        d[0x441] = 1;
        d[0x442..0x442 + rows.len()].copy_from_slice(rows);
        let len = d.len() as u32;
        d[4..8].copy_from_slice(&len.to_be_bytes());
        d
    }

    #[test]
    fn recognises_by_content() {
        let d = song(true, &[!23u8], 6);
        assert!(detect(&d));
        let mut e = d.clone();
        e[3] = b'D';
        assert!(!detect(&e));
        let mut e = d.clone();
        e[17] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[19] = 0x01;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[0x400] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[7] = 0xFF;
        assert!(!detect(&e));
        assert!(!detect(&d[..0x3F]));
    }

    #[test]
    fn periods_and_timers() {
        assert_eq!(PERIODS[0], 856);
        assert_eq!(PERIODS[35], 113);
        assert_eq!(PERIODS[36], 214);
        assert_eq!(PERIODS[71], 113);
        assert_eq!((470000 / 41u32) as u16, 11463);
        assert_eq!(SLOW_TIMERS[5], 14500);
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    fn play(d: Vec<u8>, ticks: usize) -> Vec<(u32, u16)> {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/mmdc-test/mmdc.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "MED Packer");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect()
    }

    #[test]
    fn a_looped_note_stops_starts_and_installs_its_loop() {
        // row 0 of track 0: note 13 (period 428), sample 1, no command; the rest empty.
        // A literal run of 12 bytes for row 0, then a zero run of 12 for row 1.
        let mut rows = vec![11u8, 13, 0x10, 0];
        rows.extend_from_slice(&[0; 9]);
        rows.push(!11u8);
        let d = song(true, &rows, 6);
        let log = play(d, 8);
        // stop, then the sample's first segment (the 1 word before the loop), the
        // period and the volume; the DMA set and the loop (3 words) follow
        assert_eq!(reg(&log, 0xDFF0A4), [1, 3]);
        assert_eq!(reg(&log, 0xDFF0A6), [428]);
        assert_eq!(reg(&log, 0xDFF0A8), [64]);
        let dma = reg(&log, 0xDFF096);
        let stop = dma.iter().position(|&x| x == 1).unwrap();
        let set = dma.iter().position(|&x| x == 0x8001).unwrap();
        assert!(stop < set);
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1] - lc[0], 2);
    }

    #[test]
    fn an_unlooped_note_loops_on_the_empty_word() {
        let mut rows = vec![11u8, 13, 0x10, 0];
        rows.extend_from_slice(&[0; 9]);
        rows.push(!11u8);
        let log = play(song(false, &rows, 6), 4);
        // the whole sample (4 words), then one word of the silent buffer
        assert_eq!(reg(&log, 0xDFF0A4), [4, 1]);
    }

    #[test]
    fn six_ticks_make_a_row_and_tempo_sets_the_interval() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new("/mmdc-test/mmdc.test"), song(true, &[!23u8], 41)).unwrap();
        // 41 -> 11463: 5 colour clocks a timer count
        let clocks = match &e.playing.as_ref().unwrap().backend {
            crate::engine::Backend::Replay(r) => r.tick_clocks(),
            _ => panic!(),
        };
        assert_eq!(clocks, 11464 * 5);
    }

    #[test]
    fn volume_command_is_decimal() {
        // note on track 0 with command C 0x32 = volume 32
        let mut rows = vec![11u8, 13, 0x1C, 0x32];
        rows.extend_from_slice(&[0; 9]);
        rows.push(!11u8);
        let log = play(song(true, &rows, 6), 3);
        assert_eq!(reg(&log, 0xDFF0A8)[0], 32);
    }
}
