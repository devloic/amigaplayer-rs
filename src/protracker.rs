//! Native ProTracker/NoiseTracker/SoundTracker replayer, standing in for
//! DeliTracker's internal player.
//!
//! A translation of ProTracker 2.3D's replay routine as 8bitbubsy's pt2-clone
//! (`pt2_replayer.c`, its C port of the 2.3D replayer) and his ProTracker 2.3F
//! re-source of 2.3D (`PT2.3F_replay_cia.s`) give it, both Copyright (c)
//! 2010-2026 / 2020-2025 Olav Sørensen, BSD 3-Clause licence: see
//! LICENSE-pt2-clone. The period table (all 16 finetunes, and what PT reads past
//! its end) and the vibrato and funk tables are pt2-clone's.
//!
//! It drives the emulated Paula as the 68k routine does, write for write and in
//! its order: on a row each voice's period (when the last note long was empty),
//! DMA off, length, pointer and period of a new note, the period or volume of
//! its effect, then its volume; after the four voices, PT's DMA wait, DMA on for
//! the new notes, the wait again and the repeat pointer and length of all four
//! voices, last voice first. Between ticks only the effects' writes and each
//! voice's volume. The writes are spaced as the 68000 takes to get from one to
//! the next (cycle counts of the 2.3 code paths, roughly), and the two DMA waits
//! are 2.3D's: a DBRA loop of 300 (its DMAWait), 1515 colour clocks on a 7 MHz
//! 68000. Its quirks are kept: the first row comes on the sixth interrupt, a new
//! tempo on the next, 9xx is added twice, B and D on one row, E6x and EEx on
//! delayed rows, the tremolo ramp's vibrato position, portamentos' 12-bit wrap,
//! arpeggios read past the table, EFx inverts the sample in memory. E8x does
//! nothing (as in pt2-clone by default and 2.3F).

use crate::bus::Bus;
use m68k::AddressBus;
use std::collections::HashSet;

/// Periods of the 36 notes (and a 0) for finetunes 0..7, -8..-1, 37 each.
#[rustfmt::skip]
pub(crate) const PERIODS: [u16; 607] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 0,
    850, 802, 757, 715, 674, 637, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318, 300, 284, 268, 253, 239, 225, 213, 201, 189, 179, 169, 159, 150, 142, 134, 126, 119, 113, 0,
    844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316, 298, 282, 266, 251, 237, 224, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 112, 0,
    838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118, 111, 0,
    832, 785, 741, 699, 660, 623, 588, 555, 524, 495, 467, 441, 416, 392, 370, 350, 330, 312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 124, 117, 110, 0,
    826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116, 109, 0,
    820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307, 290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115, 109, 0,
    814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305, 288, 272, 256, 242, 228, 216, 204, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114, 108, 0,
    907, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 0,
    900, 850, 802, 757, 715, 675, 636, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318, 300, 284, 268, 253, 238, 225, 212, 200, 189, 179, 169, 159, 150, 142, 134, 126, 119, 0,
    894, 844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316, 298, 282, 266, 251, 237, 223, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 0,
    887, 838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118, 0,
    881, 832, 785, 741, 699, 660, 623, 588, 555, 524, 494, 467, 441, 416, 392, 370, 350, 330, 312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 123, 117, 0,
    875, 826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116, 0,
    868, 820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307, 290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115, 0,
    862, 814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305, 288, 272, 256, 242, 228, 216, 203, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114, 0,
    // read past the end by arpeggios on finetune -1 (what follows the table in PT)
    774, 1800, 2314, 3087, 4113, 4627, 5400, 6426, 6940, 7713, 8739, 9253, 24625, 12851, 13365,
];

const VIBRATO: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24,
];

const FUNK: [u8; 16] = [0, 5, 6, 7, 8, 10, 11, 13, 16, 19, 22, 26, 32, 43, 64, 128];

/// PT 2.3D's DMA wait (MOVE.W DMAWait,D0 and DBRA D0 300 times), colour clocks.
pub const DMA_WAIT: u64 = 1515;

// Colour clocks the 68000 takes between writes (half its cycles), roughly, for
// the paths of the 2.3 replay code.
/// A voice's dispatch in an effects tick: LEAs, BSRs, the funk check, the
/// command test, the jump table.
const C_FX_CHAN: u64 = 111;
/// A voice with no effect in an effects tick, to its volume write.
const C_FX_NONE: u64 = 102;
/// From an effect's last write back to the volume write.
const C_FX_VOL: u64 = 22;
/// A row's voice: reading the note long and testing the previous one.
const C_ROW_CHAN: u64 = 60;
/// Setting up a voice from its instrument.
const C_ROW_INSTR: u64 = 100;
/// The note tests before the period lookup.
const C_ROW_TESTS: u64 = 30;
/// A step of a period table scan (CMP.W (A0)+, BHS, DBRA).
const C_SCAN: u64 = 13;
/// A write of a register from the voice's variables.
const C_WRITE: u64 = 10;
/// The second word of a MOVE.L to a register pair.
const C_LONG: u64 = 2;
/// Effect computations before their write.
const C_PORTA: u64 = 45;
const C_TONE: u64 = 40;
const C_VIB: u64 = 75;
const C_TREM: u64 = 85;
const C_SLIDE: u64 = 20;
const C_ECMD: u64 = 25;
/// The command dispatch of CheckMoreEffects.
const C_MORE: u64 = 32;
/// The voice loop after SetDMA's second wait.
const C_LOOPS: u64 = 10;

#[derive(Clone, Copy, Default)]
struct Sample {
    start: u32, // address in guest memory
    length: u16, // words
    finetune: u8,
    volume: u8,
    repeat: u16, // words
    replen: u16, // words
}

/// A voice's variables, PT's n_* (n_note and n_cmd are the pattern long).
#[derive(Clone, Copy, Default)]
struct Chan {
    note: u16,
    cmd: u16,
    start: u32,
    loopstart: u32,
    wavestart: u32,
    length: u16,
    replen: u16,
    period: u16,
    wantedperiod: u16,
    finetune: u8,
    volume: u8,
    toneportdirec: bool,
    toneportspeed: u8,
    vibratocmd: u8,
    vibratopos: u8,
    tremolocmd: u8,
    tremolopos: u8,
    wavecontrol: u8,
    glissfunk: u8,
    sampleoffset: u8,
    pattpos: u8,
    loopcount: u8,
    funkoffset: u8,
}

impl Chan {
    fn cmdlo(&self) -> u8 {
        self.cmd as u8
    }
    fn peroffset(&self) -> usize {
        (self.finetune & 0xF) as usize * 37
    }
}

pub struct ProTracker {
    pub title: String,
    samples: [Sample; 32],
    orders: [u8; 128],
    song_len: usize,
    /// Pattern data, a long per voice and row.
    patterns: Vec<u32>,
    ch: [Chan; 4],
    speed: u8,
    counter: u8,
    pub bpm: u32,
    song_pos: u8,
    row: u8,
    pbreak_pos: u8,
    pbreak_flag: bool,
    pos_jump: bool,
    patt_del_time: u8,
    patt_del_time2: u8,
    low_mask: u8,
    dmacon_temp: u16,
    visited: HashSet<(u8, u8)>,
    pub songend: bool,
    /// The LED filter (E0x): on, as the Amiga starts and ProTracker leaves it.
    pub filter: bool,
}

fn word(d: &[u8], o: usize) -> u16 {
    match d.get(o..o + 2) {
        Some(b) => u16::from_be_bytes([b[0], b[1]]),
        None => 0,
    }
}

fn spend(bus: &mut Bus, cc: u64) {
    bus.clock += cc;
}

fn dmacon(bus: &mut Bus, v: u16) {
    bus.write_word(0xDFF096, v);
}

fn reg(bus: &mut Bus, c: usize, r: u32, v: u16) {
    bus.write_word(0xDFF0A0 + 16 * c as u32 + r, v);
}

/// A MOVE.L to AUDxLC: the high word, then the low one.
fn ptr(bus: &mut Bus, c: usize, a: u32) {
    reg(bus, c, 0, (a >> 16) as u16);
    spend(bus, C_LONG);
    reg(bus, c, 2, a as u16);
}

impl ProTracker {
    /// Recognises a 31-instrument (M.K. and friends) or 15-instrument module.
    pub fn detect(d: &[u8]) -> Option<&'static str> {
        if d.len() >= 1084 {
            match &d[1080..1084] {
                b"M.K." | b"M!K!" | b"M&K!" | b"N.T." => return Some("ProTracker"),
                b"FLT4" => return Some("StarTrekker"),
                b"4CHN" => return Some("FastTracker 4ch"),
                _ => {}
            }
        }
        if d.len() >= 600 + 1024 && Self::plausible_st(d) {
            return Some("SoundTracker");
        }
        None
    }

    fn plausible_st(d: &[u8]) -> bool {
        let song_len = d[470] as usize;
        if song_len == 0 || song_len > 128 {
            return false;
        }
        if d[..20].iter().any(|&c| c != 0 && !(32..127).contains(&c)) {
            return false;
        }
        let mut total = 0usize;
        for i in 0..15 {
            let o = 20 + i * 30;
            if d[o + 25] > 64 {
                return false;
            }
            if d[o..o + 22].iter().any(|&c| c != 0 && !(32..127).contains(&c)) {
                return false;
            }
            total += word(d, o + 22) as usize * 2;
        }
        let pats = d[472..600].iter().copied().max().unwrap_or(0) as usize + 1;
        if pats > 64 {
            return false;
        }
        let need = 600 + pats * 1024 + total;
        // allow truncated sample data, but the size must be in the ballpark
        need <= d.len() + 0x8000 && 600 + pats * 1024 <= d.len()
    }

    /// The module `d`, loaded at `base` in guest memory.
    pub fn new(d: &[u8], base: u32) -> Option<Self> {
        Self::new_at(d, base, None)
    }

    /// The same with the sample data at `sample_at` in guest memory instead of after
    /// the patterns (a module laid out as a ProTracker one from another format's).
    pub fn new_at(d: &[u8], base: u32, sample_at: Option<u32>) -> Option<Self> {
        let format = Self::detect(d)?;
        let nsamp = if format == "SoundTracker" { 15 } else { 31 };
        let hdr = 20 + nsamp * 30;
        let song_len = (d[hdr] as usize).clamp(1, 128);
        let mut orders = [0u8; 128];
        orders.copy_from_slice(&d[hdr + 2..hdr + 130]);
        let pat_start = hdr + 130 + if nsamp == 31 { 4 } else { 0 };
        // as mt_init: the highest pattern of all 128 orders
        let npat = orders.iter().copied().max().unwrap_or(0) as usize + 1;
        let patterns = (0..npat * 256).map(|i| {
            let o = pat_start + i * 4;
            ((word(d, o) as u32) << 16) | word(d, o + 2) as u32
        });
        let mut samples = [Sample::default(); 32];
        let mut addr = sample_at.unwrap_or(base + (pat_start + npat * 1024) as u32);
        for (i, s) in samples[1..=nsamp].iter_mut().enumerate() {
            let o = 20 + i * 30;
            let length = word(d, o + 22);
            // SoundTracker's repeat is in bytes, and it has no finetune
            let (finetune, repeat) =
                if nsamp == 15 { (0, word(d, o + 26) / 2) } else { (d[o + 24] & 0xF, word(d, o + 26)) };
            let mut replen = word(d, o + 28);
            if length != 0 && replen == 0 {
                replen = 1; // mt_init: a replen of 0 locks up
            }
            // in the 2.3D editor an empty sample has no memory: its notes take the
            // replay's null-pointer path (length 1, no repeat), not 64K words
            let start = if length == 0 { 0 } else { addr };
            *s = Sample { start, length, finetune, volume: d[o + 25].min(64), repeat, replen };
            addr += length as u32 * 2;
        }
        let title: String = d[..20].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
        let mut pt = ProTracker {
            title: title.trim().to_string(),
            samples,
            orders,
            song_len,
            patterns: patterns.collect(),
            ch: Default::default(),
            speed: 6,
            counter: 0,
            bpm: 125,
            song_pos: 0,
            row: 0,
            pbreak_pos: 0,
            pbreak_flag: false,
            pos_jump: false,
            patt_del_time: 0,
            patt_del_time2: 0,
            low_mask: 0xFF,
            dmacon_temp: 0,
            visited: HashSet::new(),
            songend: false,
            filter: true,
        };
        pt.restart_song();
        Some(pt)
    }

    pub fn restart_song(&mut self) {
        self.ch = Default::default();
        self.speed = 6;
        self.counter = 0;
        self.bpm = 125;
        self.song_pos = 0;
        self.row = 0;
        self.pbreak_pos = 0;
        self.pbreak_flag = false;
        self.pos_jump = false;
        self.patt_del_time = 0;
        self.patt_del_time2 = 0;
        self.low_mask = 0xFF;
        self.visited.clear();
        self.songend = false;
    }

    pub fn position(&self) -> (usize, usize, usize) {
        (self.song_pos as usize, self.song_len, self.row as usize)
    }

    pub fn jump(&mut self, delta: i32) {
        self.song_pos = (self.song_pos as i32 + delta).clamp(0, self.song_len as i32 - 1) as u8;
        self.row = 0;
        self.pbreak_pos = 0;
        self.pbreak_flag = false;
        self.pos_jump = false;
        self.patt_del_time = 0;
        self.patt_del_time2 = 0;
        // the next interrupt reads the row
        self.counter = self.speed.saturating_sub(1);
        self.visited.clear();
    }

    /// Colour clocks between ticks: the CIA timer latch ProTracker sets for the
    /// tempo (1773447 / BPM), plus one, E-clock ticks of 5 colour clocks.
    pub fn tick_clocks(&self) -> u64 {
        (1_773_447 / self.bpm.max(1) as u64 + 1) * 5
    }

    /// As mt_init: the first word of a sample that does not loop is cleared, so
    /// that the repeat Paula plays after it is silent.
    pub fn clear_loop_words(&self, ram: &mut [u8]) {
        for s in &self.samples[1..] {
            if s.length != 0 && s.repeat as u32 + s.replen as u32 <= 1 {
                let a = s.start as usize;
                if a + 1 < ram.len() {
                    ram[a] = 0;
                    ram[a + 1] = 0;
                }
            }
        }
    }

    /// One interrupt of the replayer (mt_music).
    pub fn tick(&mut self, bus: &mut Bus) {
        self.counter = self.counter.wrapping_add(1);
        if self.counter >= self.speed {
            self.counter = 0;
            if self.patt_del_time2 == 0 {
                self.get_new_note(bus);
            } else {
                self.no_new_all_channels(bus);
            }
            // mt_dskip
            self.row += 1;
            if self.patt_del_time != 0 {
                self.patt_del_time2 = self.patt_del_time;
                self.patt_del_time = 0;
            }
            if self.patt_del_time2 != 0 {
                self.patt_del_time2 -= 1;
                if self.patt_del_time2 != 0 {
                    self.row -= 1;
                }
            }
            if self.pbreak_flag {
                self.pbreak_flag = false;
                self.row = self.pbreak_pos;
                self.pbreak_pos = 0;
            }
            if self.row >= 64 {
                self.next_position();
            }
        } else {
            self.no_new_all_channels(bus);
        }
        if self.pos_jump {
            self.next_position();
        }
    }

    fn next_position(&mut self) {
        self.row = self.pbreak_pos;
        self.pbreak_pos = 0;
        self.pos_jump = false;
        self.song_pos = self.song_pos.wrapping_add(1) & 127;
        if self.song_pos as usize >= self.song_len {
            self.song_pos = 0;
            self.songend = true;
            self.visited.clear();
        }
    }

    fn no_new_all_channels(&mut self, bus: &mut Bus) {
        for c in 0..4 {
            self.check_effects(bus, c);
        }
    }

    fn get_new_note(&mut self, bus: &mut Bus) {
        // a row played before (by a jump, not a loop) ends the song
        if !self.visited.insert((self.song_pos, self.row)) {
            self.songend = true;
            self.visited.clear();
            self.visited.insert((self.song_pos, self.row));
        }
        self.dmacon_temp = 0;
        let pat = self.orders[self.song_pos as usize & 127] as usize;
        for c in 0..4 {
            let l = self.patterns.get((pat * 64 + self.row as usize) * 4 + c).copied().unwrap_or(0);
            self.play_voice(bus, c, l);
            let v = self.ch[c].volume as u16;
            spend(bus, C_WRITE);
            reg(bus, c, 8, v);
        }
        // mt_SetDMA
        spend(bus, DMA_WAIT);
        dmacon(bus, 0x8000 | self.dmacon_temp);
        spend(bus, DMA_WAIT);
        for c in (0..4).rev() {
            spend(bus, C_LOOPS);
            let (ls, rl) = (self.ch[c].loopstart, self.ch[c].replen);
            ptr(bus, c, ls);
            spend(bus, C_WRITE);
            reg(bus, c, 4, rl);
        }
    }

    fn per_nop(&mut self, bus: &mut Bus, c: usize) {
        spend(bus, C_WRITE);
        let p = self.ch[c].period;
        reg(bus, c, 6, p);
    }

    fn play_voice(&mut self, bus: &mut Bus, c: usize, l: u32) {
        spend(bus, C_ROW_CHAN);
        if self.ch[c].note == 0 && self.ch[c].cmd == 0 {
            self.per_nop(bus, c);
        }
        let ch = &mut self.ch[c];
        ch.note = (l >> 16) as u16;
        ch.cmd = l as u16;
        let ins = ((ch.note >> 8) & 0xF0) as usize | (ch.cmd >> 12) as usize;
        if ins != 0 {
            spend(bus, C_ROW_INSTR);
            let s = self.samples[ins];
            ch.start = s.start;
            ch.length = s.length;
            ch.finetune = s.finetune;
            ch.volume = s.volume;
            ch.replen = s.replen;
            if s.repeat != 0 {
                ch.loopstart = s.start + s.repeat as u32 * 2;
                ch.length = s.repeat.wrapping_add(s.replen);
            } else {
                ch.loopstart = s.start;
            }
            ch.wavestart = ch.loopstart;
        }
        // mt_SetRegisters
        spend(bus, C_ROW_TESTS);
        let ch = self.ch[c];
        if ch.note & 0xFFF == 0 {
            self.check_more_effects(bus, c);
        } else if ch.cmd & 0xFF0 == 0xE50 {
            self.ch[c].finetune = ch.cmdlo() & 0xF;
            self.set_period(bus, c);
        } else {
            match (ch.cmd >> 8) & 0xF {
                3 | 5 => {
                    self.set_tone_porta(bus, c);
                    self.check_more_effects(bus, c);
                }
                9 => {
                    self.check_more_effects(bus, c);
                    self.set_period(bus, c);
                }
                _ => self.set_period(bus, c),
            }
        }
    }

    fn set_period(&mut self, bus: &mut Bus, c: usize) {
        let note = self.ch[c].note & 0xFFF;
        let i = (0..37).find(|&i| note >= PERIODS[i]).unwrap_or(36);
        spend(bus, (i as u64 + 1) * C_SCAN);
        let ch = &mut self.ch[c];
        ch.period = PERIODS[ch.peroffset() + i];
        if ch.cmd & 0xFF0 == 0xED0 {
            // note delay: triggered by the effect
            return self.check_more_effects(bus, c);
        }
        spend(bus, C_WRITE);
        dmacon(bus, 1 << c);
        let ch = &mut self.ch[c];
        if ch.wavecontrol & 4 == 0 {
            ch.vibratopos = 0;
        }
        if ch.wavecontrol & 0x40 == 0 {
            ch.tremolopos = 0;
        }
        let ch = *ch;
        spend(bus, C_WRITE + 10);
        reg(bus, c, 4, ch.length);
        spend(bus, C_WRITE);
        ptr(bus, c, ch.start);
        if ch.start == 0 {
            self.ch[c].loopstart = 0;
            self.ch[c].replen = 1;
            spend(bus, C_WRITE);
            reg(bus, c, 4, 1);
        }
        spend(bus, C_WRITE);
        reg(bus, c, 6, ch.period);
        self.dmacon_temp |= 1 << c;
        self.check_more_effects(bus, c);
    }

    /// The row's effects (mt_CheckMoreEffects).
    fn check_more_effects(&mut self, bus: &mut Bus, c: usize) {
        spend(bus, C_MORE);
        let lo = self.ch[c].cmdlo();
        match (self.ch[c].cmd >> 8) & 0xF {
            9 => {
                // sample offset
                let ch = &mut self.ch[c];
                if lo != 0 {
                    ch.sampleoffset = lo;
                }
                let off = (ch.sampleoffset as u16) << 7;
                if off < ch.length {
                    ch.length -= off;
                    ch.start = ch.start.wrapping_add(off as u32 * 2);
                } else {
                    ch.length = 1;
                }
            }
            0xB => {
                self.song_pos = lo.wrapping_sub(1);
                self.pbreak_pos = 0;
                self.pos_jump = true;
            }
            0xC => self.ch[c].volume = lo.min(64),
            0xD => {
                let v = (lo >> 4) * 10 + (lo & 0xF);
                self.pbreak_pos = if v > 63 { 0 } else { v };
                self.pos_jump = true;
            }
            0xE => self.e_commands(bus, c),
            0xF => {
                if lo == 0 {
                    // F00 stops ProTracker: the song's end
                    self.songend = true;
                } else if lo >= 32 {
                    self.bpm = lo as u32;
                } else {
                    self.counter = 0;
                    self.speed = lo;
                }
            }
            _ => self.per_nop(bus, c),
        }
    }

    /// An effects tick of a voice (mt_CheckEffects).
    fn check_effects(&mut self, bus: &mut Bus, c: usize) {
        if !self.chkefx2(bus, c) {
            let v = self.ch[c].volume as u16;
            spend(bus, C_WRITE);
            reg(bus, c, 8, v);
        }
    }

    /// Returns true when the effect wrote the volume itself (tremolo).
    fn chkefx2(&mut self, bus: &mut Bus, c: usize) -> bool {
        self.update_funk(bus, c);
        let ch = self.ch[c];
        if ch.cmd & 0xFFF == 0 {
            spend(bus, C_FX_NONE);
            return false;
        }
        spend(bus, C_FX_CHAN);
        match (ch.cmd >> 8) & 0xF {
            0 => self.arpeggio(bus, c),
            1 => self.porta_up(bus, c),
            2 => self.porta_down(bus, c),
            3 => {
                if ch.cmdlo() != 0 {
                    self.ch[c].toneportspeed = ch.cmdlo();
                    self.ch[c].cmd &= 0xFF00;
                }
                self.tone_port_no_change(bus, c);
            }
            4 => {
                let lo = ch.cmdlo();
                let v = &mut self.ch[c].vibratocmd;
                if lo & 0x0F != 0 {
                    *v = (*v & 0xF0) | (lo & 0x0F);
                }
                if lo & 0xF0 != 0 {
                    *v = (lo & 0xF0) | (*v & 0x0F);
                }
                self.vibrato2(bus, c);
            }
            5 => {
                self.tone_port_no_change(bus, c);
                self.volume_slide(bus, c);
            }
            6 => {
                self.vibrato2(bus, c);
                self.volume_slide(bus, c);
            }
            0xE => self.e_commands(bus, c),
            cmd => {
                self.per_nop(bus, c);
                if cmd == 7 {
                    self.tremolo(bus, c);
                    return true;
                }
                if cmd == 0xA {
                    self.volume_slide(bus, c);
                }
            }
        }
        spend(bus, C_FX_VOL);
        false
    }

    fn update_funk(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        let speed = ch.glissfunk >> 4;
        if speed == 0 {
            return;
        }
        ch.funkoffset = ch.funkoffset.wrapping_add(FUNK[speed as usize]);
        if ch.funkoffset & 0x80 == 0 {
            return;
        }
        ch.funkoffset = 0;
        if ch.loopstart == 0 || ch.wavestart == 0 {
            return; // 2.3F and pt2-clone: no sample yet
        }
        ch.wavestart += 1;
        if ch.wavestart >= ch.loopstart + ch.replen as u32 * 2 {
            ch.wavestart = ch.loopstart;
        }
        let a = ch.wavestart;
        let b = bus.read_byte(a);
        bus.write_byte(a, !b);
    }

    fn arpeggio(&mut self, bus: &mut Bus, c: usize) {
        let ch = self.ch[c];
        let k = match self.counter % 3 {
            0 => return self.per_nop(bus, c),
            1 => ch.cmdlo() >> 4,
            _ => ch.cmdlo() & 0xF,
        } as usize;
        let t = ch.peroffset();
        let i = (0..37).find(|&i| ch.period >= PERIODS[t + i]).unwrap_or(36);
        spend(bus, (i as u64 + 1) * C_SCAN + C_WRITE);
        reg(bus, c, 6, PERIODS[t + i + k]);
    }

    fn porta_up(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        ch.period = ch.period.wrapping_sub((ch.cmdlo() & self.low_mask) as u16);
        self.low_mask = 0xFF;
        // PT compares the 12 bits only: an underflow is not clamped
        if ch.period & 0xFFF < 113 {
            ch.period = (ch.period & 0xF000) | 113;
        }
        let p = ch.period & 0xFFF;
        spend(bus, C_PORTA);
        reg(bus, c, 6, p);
    }

    fn porta_down(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        ch.period = ch.period.wrapping_add((ch.cmdlo() & self.low_mask) as u16);
        self.low_mask = 0xFF;
        if ch.period & 0xFFF > 856 {
            ch.period = (ch.period & 0xF000) | 856;
        }
        let p = ch.period & 0xFFF;
        spend(bus, C_PORTA);
        reg(bus, c, 6, p);
    }

    fn set_tone_porta(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        let note = ch.note & 0xFFF;
        let t = ch.peroffset();
        let mut i = (0..37).find(|&i| note >= PERIODS[t + i]).unwrap_or(35);
        spend(bus, (i as u64 + 1) * C_SCAN + C_TONE);
        if ch.finetune & 8 != 0 && i > 0 {
            i -= 1;
        }
        ch.wantedperiod = PERIODS[t + i];
        ch.toneportdirec = false;
        if ch.period == ch.wantedperiod {
            ch.wantedperiod = 0;
        } else if (ch.period as i16) > ch.wantedperiod as i16 {
            ch.toneportdirec = true;
        }
    }

    fn tone_port_no_change(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        if ch.wantedperiod == 0 {
            return;
        }
        let sp = ch.toneportspeed as u16;
        if ch.toneportdirec {
            ch.period = ch.period.wrapping_sub(sp);
            if ch.period as i16 <= ch.wantedperiod as i16 {
                ch.period = ch.wantedperiod;
                ch.wantedperiod = 0;
            }
        } else {
            ch.period = ch.period.wrapping_add(sp);
            if ch.period as i16 >= ch.wantedperiod as i16 {
                ch.period = ch.wantedperiod;
                ch.wantedperiod = 0;
            }
        }
        let mut p = ch.period;
        spend(bus, C_TONE);
        if ch.glissfunk & 0xF != 0 {
            // glissando: the note's period at or below
            let t = ch.peroffset();
            let i = (0..37).find(|&i| p >= PERIODS[t + i]).unwrap_or(35);
            spend(bus, (i as u64 + 1) * C_SCAN);
            p = PERIODS[t + i];
        }
        reg(bus, c, 6, p);
    }

    fn vibrato2(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        let pos = (ch.vibratopos >> 2) & 0x1F;
        let data = match ch.wavecontrol & 3 {
            0 => VIBRATO[pos as usize] as u16,
            1 if ch.vibratopos < 128 => (pos as u16) << 3,
            1 => 255 - ((pos as u16) << 3),
            _ => 255,
        };
        let data = (data * (ch.vibratocmd & 0xF) as u16) >> 7;
        let p = if ch.vibratopos < 128 { ch.period.wrapping_add(data) } else { ch.period.wrapping_sub(data) };
        spend(bus, C_VIB);
        reg(bus, c, 6, p);
        let ch = &mut self.ch[c];
        ch.vibratopos = ch.vibratopos.wrapping_add((ch.vibratocmd >> 2) & 0x3C);
    }

    fn tremolo(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        let lo = ch.cmdlo();
        if lo & 0x0F != 0 {
            ch.tremolocmd = (ch.tremolocmd & 0xF0) | (lo & 0x0F);
        }
        if lo & 0xF0 != 0 {
            ch.tremolocmd = (lo & 0xF0) | (ch.tremolocmd & 0x0F);
        }
        let pos = (ch.tremolopos >> 2) & 0x1F;
        let data = match (ch.wavecontrol >> 4) & 3 {
            0 => VIBRATO[pos as usize] as i32,
            // PT tests the vibrato's position here
            1 if ch.vibratopos < 128 => (pos as i32) << 3,
            1 => 255 - ((pos as i32) << 3),
            _ => 255,
        };
        let data = (data * (ch.tremolocmd & 0xF) as i32) >> 6;
        let v = if ch.tremolopos < 128 { ch.volume as i32 + data } else { ch.volume as i32 - data };
        spend(bus, C_TREM);
        reg(bus, c, 8, v.clamp(0, 64) as u16);
        let ch = &mut self.ch[c];
        ch.tremolopos = ch.tremolopos.wrapping_add((ch.tremolocmd >> 2) & 0x3C);
    }

    fn volume_slide(&mut self, bus: &mut Bus, c: usize) {
        spend(bus, C_SLIDE);
        let ch = &mut self.ch[c];
        let lo = ch.cmdlo();
        if lo >> 4 != 0 {
            ch.volume = (ch.volume + (lo >> 4)).min(64);
        } else {
            ch.volume = ch.volume.saturating_sub(lo & 0xF);
        }
    }

    fn e_commands(&mut self, bus: &mut Bus, c: usize) {
        spend(bus, C_ECMD);
        let lo = self.ch[c].cmdlo();
        let y = lo & 0xF;
        let tick0 = self.counter == 0;
        let has_note = self.ch[c].note & 0xFFF != 0;
        let ch = &mut self.ch[c];
        match lo >> 4 {
            0 if tick0 => self.filter = y & 1 == 0,
            1 if tick0 => {
                self.low_mask = 0xF;
                self.porta_up(bus, c);
            }
            2 if tick0 => {
                self.low_mask = 0xF;
                self.porta_down(bus, c);
            }
            3 => ch.glissfunk = (ch.glissfunk & 0xF0) | y,
            4 => ch.wavecontrol = (ch.wavecontrol & 0xF0) | y,
            5 => ch.finetune = y,
            6 if tick0 => {
                if y == 0 {
                    ch.pattpos = self.row & 63;
                    return;
                }
                if ch.loopcount == 0 {
                    ch.loopcount = y;
                } else {
                    ch.loopcount -= 1;
                    if ch.loopcount == 0 {
                        return;
                    }
                }
                let from = ch.pattpos;
                self.pbreak_pos = from;
                self.pbreak_flag = true;
                // a loop's rows are not a song end
                for r in from..=self.row {
                    self.visited.remove(&(self.song_pos, r));
                }
            }
            7 => ch.wavecontrol = (y << 4) | (ch.wavecontrol & 0xF),
            9 if y != 0 => {
                if tick0 && has_note {
                    return;
                }
                if self.counter % y == 0 {
                    self.do_retrig(bus, c);
                }
            }
            0xA if tick0 => ch.volume = (ch.volume + y).min(64),
            0xB if tick0 => ch.volume = ch.volume.saturating_sub(y),
            0xC if self.counter == y => ch.volume = 0,
            0xD if self.counter == y && has_note => self.do_retrig(bus, c),
            0xE if tick0 && self.patt_del_time2 == 0 => self.patt_del_time = y + 1,
            0xF if tick0 => {
                ch.glissfunk = (y << 4) | (ch.glissfunk & 0xF);
                if y != 0 {
                    self.update_funk(bus, c);
                }
            }
            _ => {}
        }
    }

    fn do_retrig(&mut self, bus: &mut Bus, c: usize) {
        let ch = self.ch[c];
        spend(bus, C_WRITE);
        dmacon(bus, 1 << c);
        spend(bus, C_WRITE);
        ptr(bus, c, ch.start);
        spend(bus, C_WRITE);
        reg(bus, c, 4, ch.length);
        spend(bus, C_WRITE);
        reg(bus, c, 6, ch.period);
        spend(bus, DMA_WAIT);
        dmacon(bus, 0x8000 | 1 << c);
        spend(bus, DMA_WAIT);
        ptr(bus, c, ch.loopstart);
        spend(bus, C_WRITE);
        reg(bus, c, 4, ch.replen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 31-instrument module: sample 1 of `len` words (repeat `rep`, replen
    /// `replen`, finetune `ft`, volume 64), one pattern with `rows`
    /// (row, voice, note long).
    fn module(len: u16, rep: u16, replen: u16, ft: u8, rows: &[(usize, usize, u32)]) -> Vec<u8> {
        let mut d = vec![0u8; 1084 + 1024 + len as usize * 2];
        d[42..44].copy_from_slice(&len.to_be_bytes());
        d[44] = ft;
        d[45] = 64;
        d[46..48].copy_from_slice(&rep.to_be_bytes());
        d[48..50].copy_from_slice(&replen.to_be_bytes());
        for i in 1..31 {
            d[20 + i * 30 + 29] = 1;
        }
        d[950] = 1;
        d[951] = 127;
        d[1080..1084].copy_from_slice(b"M.K.");
        for &(r, c, l) in rows {
            let o = 1084 + (r * 4 + c) * 4;
            d[o..o + 4].copy_from_slice(&l.to_be_bytes());
        }
        d
    }

    /// A note long: instrument, period, command and parameter.
    fn note(ins: u32, period: u32, cmd: u32) -> u32 {
        (ins & 0xF0) << 24 | period << 16 | (ins & 0xF) << 12 | cmd
    }

    fn bus() -> Bus {
        let mut b = Bus::new();
        b.write_log = Some(Vec::new());
        b
    }

    /// Runs `ticks` interrupts; the writes of each (address, value).
    fn run(pt: &mut ProTracker, b: &mut Bus, ticks: usize) -> Vec<Vec<(u32, u16)>> {
        (0..ticks)
            .map(|_| {
                b.write_log.as_mut().unwrap().clear();
                pt.tick(b);
                b.clock += pt.tick_clocks();
                b.write_log.as_ref().unwrap().iter().map(|w| (w.0, w.1)).collect()
            })
            .collect()
    }

    const BASE: u32 = 0x10000;

    #[test]
    fn finetuned_periods_come_from_the_table() {
        // E-1 (678) at finetune +7 is 646 in PT's table, 645 by formula
        for (ft, period, want) in [(15, 428, 431), (7, 678, 646), (8, 113, 120)] {
            let d = module(100, 0, 1, ft, &[(0, 0, note(1, period, 0))]);
            let mut pt = ProTracker::new(&d, BASE).unwrap();
            let mut b = bus();
            let w = run(&mut pt, &mut b, 6);
            assert!(w[5].contains(&(0xDFF0A6, want)), "finetune {ft}: {:x?}", w[5]);
        }
    }

    #[test]
    fn row_writes_in_pt_order() {
        let d = module(100, 10, 20, 0, &[(0, 1, note(1, 428, 0))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let mut b = bus();
        let w = run(&mut pt, &mut b, 6);
        // the first row comes on the sixth interrupt; before it, volumes only
        assert_eq!(w[0], vec![(0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0)]);
        let s = BASE + 1084 + 1024;
        let (hi, lo) = ((s >> 16) as u16, s as u16);
        let (lhi, llo) = (((s + 20) >> 16) as u16, (s + 20) as u16);
        let want = vec![
            (0xDFF0A6, 0),
            (0xDFF0A6, 0),
            (0xDFF0A8, 0),
            (0xDFF0B6, 0),
            (0xDFF096, 2),
            (0xDFF0B4, 30), // repeat + replen
            (0xDFF0B0, hi),
            (0xDFF0B2, lo),
            (0xDFF0B6, 428),
            (0xDFF0B6, 428),
            (0xDFF0B8, 64),
            (0xDFF0C6, 0),
            (0xDFF0C6, 0),
            (0xDFF0C8, 0),
            (0xDFF0D6, 0),
            (0xDFF0D6, 0),
            (0xDFF0D8, 0),
            (0xDFF096, 0x8002),
            (0xDFF0D0, 0),
            (0xDFF0D2, 0),
            (0xDFF0D4, 0),
            (0xDFF0C0, 0),
            (0xDFF0C2, 0),
            (0xDFF0C4, 0),
            (0xDFF0B0, lhi),
            (0xDFF0B2, llo),
            (0xDFF0B4, 20),
            (0xDFF0A0, 0),
            (0xDFF0A2, 0),
            (0xDFF0A4, 0),
        ];
        assert_eq!(w[5], want);
    }

    #[test]
    fn dma_waits_are_pt_s() {
        let d = module(100, 0, 1, 0, &[(0, 0, note(1, 428, 0))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let mut b = bus();
        for _ in 0..5 {
            pt.tick(&mut b);
        }
        b.write_log.as_mut().unwrap().clear();
        pt.tick(&mut b);
        let l = b.write_log.clone().unwrap();
        let t = |a: u32, v: u16| l.iter().find(|w| w.0 == a && w.1 == v).unwrap().2;
        let off = t(0xDFF096, 1);
        let on = t(0xDFF096, 0x8001);
        let last_vol = l.iter().filter(|w| w.0 == 0xDFF0D8).last().unwrap().2;
        assert_eq!(on - last_vol, DMA_WAIT);
        assert!(on - off > DMA_WAIT);
        let loops = l.iter().find(|w| w.0 == 0xDFF0D0).unwrap().2;
        assert_eq!(loops - on, DMA_WAIT + C_LOOPS);
    }

    #[test]
    fn sample_offset_is_added_twice() {
        // 901 on a note: 128 words off before the trigger, 128 more after it
        let d = module(1000, 0, 1, 0, &[(0, 0, note(1, 428, 0x901)), (1, 0, note(0, 0, 0xE91))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let mut b = bus();
        let w = run(&mut pt, &mut b, 12);
        let s = BASE + 1084 + 1024;
        assert!(w[5].contains(&(0xDFF0A2, (s + 256) as u16)));
        assert!(w[5].contains(&(0xDFF0A4, 1000 - 128)));
        // the next row's E91 retriggers where the second offset left it
        assert!(w[11].contains(&(0xDFF0A2, (s + 512) as u16)), "{:x?}", w[11]);
        assert!(w[11].contains(&(0xDFF0A4, 1000 - 256)));
    }

    #[test]
    fn tempo_and_pattern_break() {
        let d = module(100, 0, 1, 0, &[(0, 0, note(0, 0, 0xF96)), (0, 1, note(0, 0, 0xD10)), (10, 0, note(0, 0, 0xF02))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let mut b = bus();
        run(&mut pt, &mut b, 6);
        assert_eq!(pt.bpm, 150);
        assert_eq!(pt.tick_clocks(), (1_773_447 / 150 + 1) * 5);
        // D10 on a one-pattern song: back to row 10 of position 0, a song end
        assert_eq!(pt.position(), (0, 1, 10));
        assert!(pt.songend);
        run(&mut pt, &mut b, 6);
        assert_eq!(pt.speed, 2);
    }

    #[test]
    fn empty_sample_is_silent() {
        // sample 2 is empty: its note gets a null pointer and length 1
        let d = module(100, 0, 1, 0, &[(0, 0, note(2, 428, 0))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let w = run(&mut pt, &mut bus(), 6);
        let i = w[5].iter().position(|&x| x == (0xDFF096, 1)).unwrap();
        assert_eq!(&w[5][i + 1..i + 5], &[(0xDFF0A4, 0), (0xDFF0A0, 0), (0xDFF0A2, 0), (0xDFF0A4, 1)]);
        assert!(w[5].ends_with(&[(0xDFF0A0, 0), (0xDFF0A2, 0), (0xDFF0A4, 1)]));
    }

    #[test]
    fn led_filter_starts_on() {
        let d = module(100, 0, 1, 0, &[(0, 0, note(0, 0, 0xE01))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        assert!(pt.filter);
        run(&mut pt, &mut bus(), 6);
        assert!(!pt.filter);
    }
}
