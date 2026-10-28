//! Game Music Creator, ported from NostalgicPlayer's GameMusicCreator player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Where the original player's register writes differ from what NostalgicPlayer
//! asks of its mixer, this follows the original (checked against UADE's G.M.C
//! Eagleplayer, "Game Music Creator player module V1.6 (9 May 93)", Andreas
//! Tadic's replayer adapted by Delirium and Buggs). Like the original it plays
//! from the module in guest memory: its InitSound puts the sample addresses into
//! the module's sample table, and the rows are read through the pattern and
//! position pointers as it keeps them.
//!
//! What the original does and NostalgicPlayer does not:
//! * a row writes a new note's volume 0, address, length and period at once,
//!   then switches the DMA of the voices with a note off (a DMACON of 0 when
//!   there is none); the next tick switches them on, and the tick after writes
//!   their loop: always the sample's loop start and length as stored (a loop of
//!   length 0 plays 65536 words), and a loop of 1 word ($0002) plays from the
//!   player's 8 bytes of silence, where InitSound sets it
//! * effect 3 sets the volume unchecked (up to 255); effects 6 and 7 do nothing
//! * a position word of $8000 or more is played as a pattern offset like any
//!   other; the song ends (and starts again) past its last position
//! * effect 5 counts the position one lower than the one it reads, so the song
//!   ends a position later; with argument 0 it reads its positions from 64 KB
//!   past the position table; and it leaves the row pointer on the position
//!   table, so the voices after it in its row read their notes from there
//!
//! The writes come at the original's times: within a tick, half the 68000
//! cycles of its code paths as the emulator counts them; the start (the host's
//! check and module info calls, its two calls of the volume function, InitSound,
//! the timer) is fitted on the reference's write logs, linear in the module's
//! positions and samples. A module with no positions is refused (the original
//! loops over 65536 of them and calls SongEnd forever).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "GameMusicCreator", detect, new };

const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254,
    240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// The module's parts: 15 sample records of 16 bytes (address, length in words,
/// volume, loop address, loop length in words), the position count (a long),
/// the positions (pattern offsets, words) and the patterns (1 KB each: 64 rows
/// of 4 voices, a period word, then sample << 12 | effect << 8 | argument).
const POS_COUNT: u32 = 0xf0;
const POSITIONS: u32 = 0xf4;
const PATTERNS: u32 = 0x1bc;

/// 68000 cycles of the original's code paths, as the emulator counts them (see
/// m68k-rc's cycles.rs: a branch takes the same time taken or not): a write
/// comes half the cycles after the start of the interrupt, in colour clocks.
mod cyc {
    /// The interrupt: its start (saving the registers, the host's flags) to the
    /// first period write, and between two of them.
    pub const ENTRY: u64 = 340;
    pub const PER: u64 = 48;
    /// A voice whose loop is not due; one whose loop is: to its address, its
    /// length, and on to the next voice; after the last voice to DMA on, and
    /// more for each voice switched on.
    pub const LOOP_SKIP: u64 = 27;
    pub const LOOP_LOC: [u64; 4] = [87, 91, 91, 91];
    pub const LOOP_LEN: u64 = 20;
    pub const LOOP_POST: u64 = 72;
    pub const DMA_ON: u64 = 152;
    pub const DMA_BIT: u64 = 32;
    /// DMA on to the first volume, and between the volumes.
    pub const VOL_FIRST: u64 = 142;
    pub const VOL: u64 = 158;
    /// The last volume to a row's first voice (with a new position, more).
    pub const ROW: u64 = 150 + 90;
    pub const NEW_POS: u64 = 229;
    /// The song's end: the host's SongEnd call and the position counted again
    /// (measured).
    pub const WRAP: u64 = 274;
    /// A voice without a note; with one, to its volume, address, length and
    /// period, and on to its effect.
    pub const NO_NOTE: u64 = 47;
    pub const NOTE_VOL: [u64; 4] = [168, 187, 206, 206];
    pub const NOTE_LOC: [u64; 4] = [36, 40, 40, 40];
    pub const NOTE_LEN: [u64; 4] = [44, 20, 20, 20];
    pub const NOTE_PER: [u64; 4] = [32, 12, 12, 12];
    pub const NOTE_POST: [u64; 4] = [108, 152, 156, 152];
    /// The effect, by its number (with its call), and on to the next voice or,
    /// after the last one, to the DMA off.
    pub const FX: [u64; 16] = [55, 62, 77, 96, 166, 279, 188, 207, 185, 207, 207, 207, 207, 207, 207, 207];
    /// Effects 1 to 3 search for the voice.
    pub const FX_VOICE: [u64; 4] = [47, 66, 85, 85];
    pub const FX_CALL: u64 = 18;
    pub const NEXT: u64 = 42;
    pub const DMA_OFF: u64 = 16;
}

pub struct GameMusicCreator {
    module: u32,
    /// The voices whose loop is due on the next tick.
    loop_due: u8,
    slide: [u16; 4],
    /// The sample record a note started, until its loop is written.
    sample: [u32; 4],
    period: [u16; 4],
    volume: [u16; 4],
    speed_cnt: u16,
    speed: u16,
    row: u16,
    row_ptr: u32,
    pos_ptr: u32,
    pos: u32,
    /// The clock at the start of the tick and the cycles spent since.
    t0: u64,
    cycles: u64,
    songend: bool,
}

/// NostalgicPlayer's test, but for the note it wants in the first pattern.
fn detect(d: &[u8]) -> bool {
    if d.len() < 444 {
        return false;
    }
    let positions = be32(d, 240);
    if positions > 100 {
        return false;
    }
    if (be32(d, 244) | be32(d, 248) | be32(d, 252) | be32(d, 256)) & 0x03ff03ff != 0 {
        return false;
    }
    let mut sample_size = 0u32;
    for i in 0..15 {
        let o = 16 * i;
        let start = be32(d, o);
        let len = be16(d, o + 4);
        if start > 0xf8000 || len >= 0x8000 || (len != 0 && start == 0) {
            return false;
        }
        sample_size += 2 * len as u32;
        if be16(d, o + 6) > 64 {
            return false;
        }
        let loop_start = be32(d, o + 8);
        if loop_start != 0 && (loop_start > 0xf8000 || start == 0 || len == 0) {
            return false;
        }
    }
    let mut max = 0;
    for i in 0..positions as usize {
        let w = be16(d, 244 + 2 * i);
        if !w.is_multiple_of(1024) {
            return false;
        }
        if w < 0x8000 && w > max {
            max = w;
        }
    }
    let patterns = max as usize / 1024 + 1;
    if 444 + patterns * 1024 + sample_size as usize > d.len() + 256 {
        return false;
    }
    // the first pattern's periods must be notes; NostalgicPlayer also wants one
    // there, here one in any pattern will do (the original plays modules whose
    // first pattern is empty)
    let period = |i: usize| be32(d, 444 + 4 * i) >> 16 & 0x0fff;
    let note = |p: u32| p != 0 && p != 0xffe;
    if (0..256).any(|i| note(period(i)) && !PERIODS.contains(&(period(i) as u16))) {
        return false;
    }
    (0..256 * patterns).any(|i| note(period(i)))
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let positions = be32(d, POS_COUNT as usize);
    if positions == 0 {
        return None;
    }
    // the original's data hunk: 8 bytes of silence
    let silence = m.alloc(8);
    if silence == 0 {
        return None;
    }
    // as the original's InitSound: the samples follow the highest pattern of the
    // positions (a signed word, at least 0); their records get their addresses,
    // and a 1-word loop the silence
    let b = &mut m.bus;
    let mut max = 0i16;
    let mut raised = 0u64;
    for i in 0..positions {
        let w = b.r16(base + POSITIONS + 2 * i) as i16;
        if w > max {
            max = w;
            raised += 1;
        }
    }
    let mut a = base + PATTERNS + max as u32 + 0x400;
    let (mut empty, mut short_loops) = (0u64, 0u64);
    for i in 0..15 {
        let r = base + 16 * i;
        short_loops += (b.r16(r + 0xc) == 2) as u64;
        if b.r32(r) == 0 {
            empty += 1;
            continue;
        }
        let rel = b.r32(r + 8).wrapping_sub(b.r32(r));
        b.w32(r, a);
        b.w32(r + 8, a.wrapping_add(rel));
        if b.r16(r + 0xc) == 2 {
            b.w32(r + 8, silence);
        }
        a = a.wrapping_add(2 * b.r16(r + 4) as u32);
    }

    // The start as the reference's (colour clocks, fitted on its write logs, the
    // same for all 90 modules): the host calls the volume function twice, each
    // writing the (still 0) volumes, then InitSound clears them and the DMA; the
    // timer starts after the rest of InitSound (its loops: 33 cycles a position,
    // 4 for a new highest one, 24 more for a 1-word loop, 131 less for an empty
    // sample) and the host's start, the interrupt's entry before the first write.
    let (n, raised, short_loops, empty) = (positions as i64, raised as i64, short_loops as i64, empty as i64);
    let mut hw = Hw { bus: &mut m.bus };
    // the host's check (its loops: 50 cycles a position, 4 more for a new
    // highest one, 47 less for a sample with a 1-word loop) and module info call
    // (29 a position, 4 for a new highest one, 38 less for an empty sample), each
    // counted in colour clocks from its own start
    let check = (1 + 50 * n + 4 * raised - 47 * short_loops).div_euclid(2);
    let info = (29 * n + 4 * raised - 38 * empty).div_euclid(2);
    let at = |hw: &mut Hw, t: u64| {
        if t > hw.bus.clock {
            hw.wait(t - hw.bus.clock);
        }
    };
    let mut t = (2235 + check + info) as u64;
    for (i, gap) in [0, 156, 168, 162, 206, 156, 168, 162, 127, 10, 10, 10].into_iter().enumerate() {
        t += gap;
        at(&mut hw, t);
        hw.vol(i % 4, 0);
    }
    t += 10;
    at(&mut hw, t);
    hw.dmacon(0x000f);
    let c2 = 145366 + 33 * n + 4 * raised + 24 * short_loops - 131 * empty;
    at(&mut hw, t + (c2 / 2) as u64 - super::DT_TICK - cyc::ENTRY / 2);

    Some(Box::new(GameMusicCreator {
        module: base,
        loop_due: 0,
        slide: [0; 4],
        sample: [0; 4],
        period: [0; 4],
        volume: [0; 4],
        speed_cnt: 0,
        speed: 6,
        row: 0x40,
        row_ptr: base + PATTERNS,
        pos_ptr: base + 0xf2,
        pos: 0,
        t0: 0,
        cycles: 0,
        songend: false,
    }))
}

impl Replayer for GameMusicCreator {
    fn format(&self) -> &'static str {
        "Game Music Creator"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cycles = 0;
        for i in 0..4 {
            self.spend(hw, if i == 0 { cyc::ENTRY } else { cyc::PER });
            self.period[i] = self.period[i].wrapping_add(self.slide[i]);
            hw.per(i, self.period[i]);
        }
        // the loops of the notes switched on in the last tick
        for i in 0..4 {
            if self.loop_due & 1 << i == 0 {
                self.spend(hw, cyc::LOOP_SKIP);
                continue;
            }
            self.loop_due &= !(1 << i);
            let r = self.sample[i];
            self.spend(hw, cyc::LOOP_LOC[i]);
            hw.loc(i, hw.bus.r32(r + 8));
            self.spend(hw, cyc::LOOP_LEN);
            hw.len(i, hw.bus.r16(r + 0xc));
            self.sample[i] = 0;
            self.spend(hw, cyc::LOOP_POST);
        }
        // the notes of the last row
        let mut dma = 0x8000;
        for i in 0..4 {
            if self.sample[i] != 0 {
                self.loop_due |= 1 << i;
                dma |= 1 << i;
                self.spend(hw, cyc::DMA_BIT);
            }
        }
        self.spend(hw, cyc::DMA_ON);
        hw.dmacon(dma);
        // the volume function scales by the host's voice volumes (64: no change)
        for i in 0..4 {
            self.spend(hw, if i == 0 { cyc::VOL_FIRST } else { cyc::VOL });
            hw.vol(i, ((self.volume[i] as u32 * 64) as u16) >> 6);
        }

        self.speed_cnt = self.speed_cnt.wrapping_add(1);
        if (self.speed as i16) > self.speed_cnt as i16 {
            return;
        }
        self.speed_cnt = 0;
        self.spend(hw, cyc::ROW);
        self.row = self.row.wrapping_add(1);
        if self.row == 0x41 {
            self.spend(hw, cyc::NEW_POS);
            loop {
                self.pos = self.pos.wrapping_add(1);
                if self.pos > hw.bus.r32(self.module + POS_COUNT) {
                    self.pos = 0;
                    self.pos_ptr = self.module + 0xf2;
                    self.songend = true;
                    self.spend(hw, cyc::WRAP);
                    continue;
                }
                break;
            }
            self.row = 1;
            self.pos_ptr = self.pos_ptr.wrapping_add(2);
            let off = hw.bus.r16(self.pos_ptr) as u32;
            self.row_ptr = self.module.wrapping_add(PATTERNS).wrapping_add(off);
        }
        let mut a0 = self.row_ptr;
        self.row_ptr = self.row_ptr.wrapping_add(16);
        let mut off = 0;
        for i in 0..4 {
            if i > 0 {
                self.spend(hw, cyc::NEXT);
            }
            let d0 = hw.bus.r32(a0.wrapping_add(4 * i as u32));
            let d5 = d0 as u16;
            if d5 & 0xf000 == 0 {
                self.spend(hw, cyc::NO_NOTE);
            } else {
                let per = (d0 >> 16) as u16;
                let r = self.module.wrapping_add((((d5 >> 12) - 1) << 4) as u32);
                self.spend(hw, cyc::NOTE_VOL[i]);
                hw.vol(i, 0);
                self.sample[i] = r;
                self.spend(hw, cyc::NOTE_LOC[i]);
                hw.loc(i, hw.bus.r32(r));
                self.spend(hw, cyc::NOTE_LEN[i]);
                hw.len(i, hw.bus.r16(r + 4));
                self.spend(hw, cyc::NOTE_PER[i]);
                hw.per(i, per);
                self.period[i] = per;
                self.volume[i] = hw.bus.r16(r + 6);
                self.slide[i] = 0;
                off |= 1 << i;
                self.spend(hw, cyc::NOTE_POST[i]);
            }
            let fx = (d5 >> 8 & 15) as usize;
            let arg = d5 & 0xff;
            self.spend(hw, cyc::FX_CALL + cyc::FX[fx] + if (1..=3).contains(&fx) { cyc::FX_VOICE[i] } else { 0 });
            match fx {
                1 => self.slide[i] = arg.wrapping_neg(),
                2 => self.slide[i] = arg,
                3 => self.volume[i] = arg,
                4 => self.row = 0x40,
                5 => {
                    // the position counter one lower than the position read next;
                    // the pointer in word arithmetic, and in the row's register
                    self.pos = (arg as u32).wrapping_sub(1);
                    let d4 = ((arg << 1).wrapping_sub(2)) as u32;
                    a0 = self.module.wrapping_add(POSITIONS).wrapping_add(d4);
                    self.pos_ptr = a0;
                    self.row = 0x40;
                }
                8 => self.speed = arg,
                _ => {}
            }
        }
        self.spend(hw, cyc::DMA_OFF);
        hw.dmacon(off);
    }
}

impl GameMusicCreator {
    /// Lets `c` more of the original's cycles pass before the next write.
    fn spend(&mut self, hw: &mut Hw, c: u64) {
        self.cycles += c;
        let t = self.t0 + self.cycles / 2;
        if t > hw.bus.clock {
            hw.wait(t - hw.bus.clock);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One sample (16 words, a 4-word loop 8 bytes in), one position, one
    /// pattern: row 0 plays it on voice 0, and sets voice 1's volume to $50.
    fn module() -> Vec<u8> {
        let mut d = vec![0u8; 444 + 1024];
        // the editor's addresses: only their differences count
        d[0..14].copy_from_slice(&[0, 1, 0, 0, 0, 16, 0, 64, 0, 1, 0, 8, 0, 4]);
        d[240..244].copy_from_slice(&1u32.to_be_bytes());
        d[444..452].copy_from_slice(&[0x01, 0xac, 0x10, 0x00, 0, 0, 0x03, 0x50]);
        d.extend((0..32).map(|i| i as u8));
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("gmc.test"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Game Music Creator");
        let mut out = vec![0f32; 2 * 14400];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound's DMA off, DMA on with no voice for 6 ticks (the row comes at
        // the end of the sixth), the row's DMA off for voice 0, its DMA on on the
        // next tick
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..10], &[0x000f, 0x8000, 0x8000, 0x8000, 0x8000, 0x8000, 0x8000, 0x0001, 0x8001, 0x8000]);
        // the note's length and period, then its loop: 8 bytes in, 4 words
        assert_eq!(&reg(0xDFF0A4)[..2], &[16, 4]);
        let loc = reg(0xDFF0A2);
        assert_eq!(loc[1].wrapping_sub(loc[0]), 8);
        assert!(reg(0xDFF0A6).contains(&428));
        // the sample's volume on voice 0, the effect's (unchecked) on voice 1
        assert!(reg(0xDFF0A8).contains(&64));
        assert!(reg(0xDFF0B8).contains(&0x50));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
