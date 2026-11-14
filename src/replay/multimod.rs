//! Multi-channel MODs, ported from NostalgicPlayer's ModTracker player (MIT,
//! Copyright (c) Thomas Neumann: see LICENSE-NostalgicPlayer): the playback of
//! `ModTrackerWorker` for its ProTracker module type (the 31-instrument loader, the
//! period tables, every normal and E effect, speed and tempo, the pattern loop and
//! delay, the position visits that end a song) generalised from four channels to up
//! to 32, and its StarTrekker 8 type (FLT8).
//!
//! Which files, and what the source does not cover. NostalgicPlayer's identifier
//! knows the multi-channel tag "FLT8" only: its worker plays eight channels for it,
//! two four-channel patterns to a pattern (`positions / 2`). The FastTracker and
//! TakeTracker tags (`6CHN`, `8CHN`, `xxCH`) and Octalyser's (`CD61`, `CD81`, `OCTA`)
//! and Digital Tracker's (`FA06`, `FA08`) are not in it, and no other NostalgicPlayer
//! player reads them (its OpenMPT agent only hands the file to libopenmpt). They are
//! the same layout as a four-channel module with more cells to a row, which the
//! generalised worker plays as it plays a ProTracker module. No code from any other
//! source is used; the tags and channel counts are the documented ones. `4CHN` and the
//! four-channel tags stay with the built-in ProTracker.
//!
//! Where this differs from NostalgicPlayer's worker (each a plain bug there or a
//! thing the amplifier cannot do):
//! - a slide that would take the period below zero stops at the limit (the worker's
//!   16-bit period wraps and slips past its check);
//! - a tremolo changes the volume of the ticks it runs on only (the worker leaves the
//!   tremolo's volume in a field that outlives the effect);
//! - FLT8 keeps the vibrato's speed and depth, which the worker's noise-tracker path
//!   never stores;
//! - E8x (Karplus-Strong, an in-place edit of the loop) does nothing, and
//!   StarTrekker's AM instruments (a `.nt` file beside the module) are not played;
//! - samples play from the module in guest memory, and a sample that runs past the
//!   end of a truncated file is cut where the file ends.
//!
//! More than four voices cannot go one to one onto Paula, so the voices are the host's
//! Eagleplayer amplifier's (`Hw::voice_*`), poked as Paula's channels are, with the
//! amplifier's own side for each (Amiga LRRL, repeating). A note is a DMA off, LOC,
//! LEN, period, DMA on and then the loop's LOC and LEN, as ProTracker writes them.
//!
//! The FastTracker and TakeTracker tags play as PS3M plays them on the same amplifier (EP206's
//! DeliPlayer, measured as a black box from its AudioStructs and its timer): every sample's first
//! four bytes are cleared, and the first tick comes where PS3M's falls among the amplifier's
//! buffers (`timer_delay`).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Multichannel MOD", detect: |d| kind_of(d).is_some_and(|k| k.1 == Kind::Pt), new: |d, b, m| new(d, b, m, Kind::Pt) };
pub const OPEN_FLT8: Open = Open { name: "StarTrekker 8", detect: |d| kind_of(d).is_some_and(|k| k.1 == Kind::Flt8), new: |d, b, m| new(d, b, m, Kind::Flt8) };

include!("multimod_tables.rs");

/// When PS3M's first tick comes among the amplifier's buffers (EP206's PS3M, which mixes in its
/// AudioStructs a buffer at a time, so that a tick's changes are heard from the buffer mixed
/// after it): measured on 10 FST modules of 6 and 8 voices and a one-pattern probe, its first
/// tick comes 82516 colour clocks plus 3.28 per pattern and channel (the time its set-up takes
/// over the patterns, within 250) after the buffer before its first buffer interrupt, a buffer
/// earlier than this port's first one; this port's first tick, without a delay, comes 70713
/// after that.
const PS3M_FIRST_TICK: u64 = 82516;
const PS3M_PER_PATTERN_CHANNEL: u64 = 328;
const PORT_FIRST_TICK: u64 = 70713;

const NOTES: usize = 36;
const MIN_PERIOD: i32 = 113;
const MAX_PERIOD: i32 = 856;

/// The vibrato and tremolo sine.
const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141,
    120, 97, 74, 49, 24,
];
const FUNK: [u8; 16] = [0, 5, 6, 7, 8, 10, 11, 13, 16, 19, 22, 26, 32, 43, 64, 128];

/// ProTracker's command set, or StarTrekker 8's (NoiseTracker's: no fine tune, sample
/// offset or E commands but the filter, and a speed to 31).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Pt,
    Flt8,
}

/// The channel count and kind of the module's tag at 1080, if it is one of the
/// multi-channel tags (the four-channel ones are not).
pub fn kind_of(d: &[u8]) -> Option<(usize, Kind)> {
    if d.len() < 1084 {
        return None;
    }
    let (n, kind) = match &d[1080..1084] {
        b"FLT8" => (8, Kind::Flt8),
        b"CD81" | b"OCTA" | b"FA08" => (8, Kind::Pt),
        b"CD61" | b"FA06" => (6, Kind::Pt),
        [c @ b'1'..=b'9', b'C', b'H', b'N'] if *c != b'4' => ((c - b'0') as usize, Kind::Pt),
        [a @ b'1'..=b'3', c @ b'0'..=b'9', b'C', b'H'] => ((a - b'0') as usize * 10 + (c - b'0') as usize, Kind::Pt),
        _ => return None,
    };
    if !(1..=32).contains(&n) || d[950] == 0 || d.len() < 1084 + 256 * n {
        return None;
    }
    Some((n, kind))
}

#[derive(Clone, Copy, Default)]
struct Cell {
    /// The cell's period when it is below the table's lowest (113). PS3M, the original of the
    /// FastTracker tags, plays such a note as the table's second entry (seen: period 95 with
    /// finetune 3 sounds at 791).
    raw: u16,
    note: u8,
    smp: u8,
    eff: u8,
    arg: u8,
}

#[derive(Clone, Copy, Default)]
struct Smp {
    addr: u32,
    /// Words, as far as the file has them.
    len: u16,
    vol: u8,
    fine: u8,
    /// Words.
    loop_start: u16,
    loop_len: u16,
}

#[derive(Clone, Copy)]
struct Ch {
    cell: Cell,
    sample: usize,
    addr: u32,
    has_data: bool,
    /// Words.
    length: u16,
    start_offset: u32,
    loop_start: u32,
    loop_len: u16,
    wave_start: u32,
    period: i32,
    fine: u8,
    volume: i32,
    /// The tremolo's volume, on the ticks it runs.
    tremolo: Option<i32>,
    tone_dir: bool,
    tone_speed: u8,
    wanted: i32,
    vib_cmd: u8,
    vib_pos: i8,
    tre_cmd: u8,
    tre_pos: i8,
    wave_ctl: u8,
    gliss_funk: u8,
    sample_offset: u8,
    loop_patt_pos: u8,
    do_loop_patt_pos: i16,
    loop_count: u8,
    funk_offset: u8,
    /// What the voice has been given.
    last_per: i32,
    last_vol: i32,
}

impl Default for Ch {
    fn default() -> Self {
        Ch {
            cell: Cell::default(),
            sample: 0,
            addr: 0,
            has_data: false,
            length: 0,
            start_offset: 0,
            loop_start: 0,
            loop_len: 0,
            wave_start: 0,
            period: 0,
            fine: 0,
            volume: 0,
            tremolo: None,
            tone_dir: false,
            tone_speed: 0,
            wanted: 0,
            vib_cmd: 0,
            vib_pos: 0,
            tre_cmd: 0,
            tre_pos: 0,
            wave_ctl: 0,
            gliss_funk: 0,
            sample_offset: 0,
            loop_patt_pos: 0,
            do_loop_patt_pos: -1,
            loop_count: 0,
            funk_offset: 0,
            last_per: -1,
            last_vol: -1,
        }
    }
}

pub struct MultiMod {
    kind: Kind,
    /// A FastTracker/TakeTracker tag (`xCHN`, `xxCH`): music made on a PC.
    pc: bool,
    title: String,
    n: usize,
    samples: Vec<Smp>,
    song_len: usize,
    restart: usize,
    positions: [u8; 128],
    /// `patterns[pattern][row * n + channel]`.
    patterns: Vec<Vec<Cell>>,
    /// A word of silence in chip memory, where a sample without a loop goes on.
    silence: u32,
    ch: Vec<Ch>,
    first_tick: bool,
    speed: u8,
    counter: u8,
    bpm: u32,
    /// A tempo command has run (PS3M then times ticks from the NTSC CIA clock).
    bpm_set: bool,
    song_pos: usize,
    old_song_pos: usize,
    pat_pos: usize,
    break_pos: usize,
    pos_jump: bool,
    break_flag: bool,
    got_break: bool,
    got_pos_jump: bool,
    last_break_arg: i32,
    last_jump_arg: i32,
    low_mask: u8,
    patt_delay: u8,
    patt_delay2: u8,
    visited: [bool; 128],
    end_reached: bool,
    restart_song: bool,
    songend: bool,
}

fn new(d: &[u8], base: u32, m: &mut Machine, want: Kind) -> Option<Box<dyn Replayer>> {
    let (n, kind) = kind_of(d)?;
    if kind != want {
        return None;
    }
    let mut samples = Vec::with_capacity(31);
    let song_len = (d[950] as usize).min(128);
    let mut positions = [0u8; 128];
    positions.copy_from_slice(&d[952..1080]);
    if kind == Kind::Flt8 {
        for p in positions.iter_mut() {
            *p /= 2;
        }
    }
    let npat = positions.iter().copied().max().unwrap_or(0) as usize + 1;
    let pat_bytes = 256 * n;
    let mut at = 1084 + npat * pat_bytes;
    for i in 0..31 {
        let o = 20 + i * 30;
        let w = |o: usize| u16::from_be_bytes([d[o], d[o + 1]]);
        let mut len = w(o + 22);
        let mut ls = w(o + 26);
        let mut ll = w(o + 28);
        if ll == 1 {
            ll = 0;
        }
        if ls > len {
            ls = 0;
            ll = 0;
        }
        if ls as u32 + ll as u32 > len as u32 {
            ll = len - ls;
        }
        // a truncated file ends the sample where it ends
        let have = (d.len().saturating_sub(at) / 2).min(len as usize) as u16;
        if have < len {
            len = have;
            if ls as u32 + ll as u32 > len as u32 {
                ls = ls.min(len);
                ll = len - ls;
                if ll <= 1 {
                    ll = 0;
                }
            }
        }
        let fine = if kind == Kind::Flt8 { 0 } else { d[o + 24] & 15 };
        samples.push(Smp { addr: base + at as u32, len, vol: d[o + 25].min(64), fine, loop_start: ls, loop_len: ll });
        at += w(o + 22) as usize * 2;
    }
    let patterns = (0..npat)
        .map(|p| {
            let mut cells = vec![Cell::default(); 64 * n];
            for row in 0..64 {
                for c in 0..n {
                    let o = if kind == Kind::Flt8 {
                        1084 + p * pat_bytes + (c / 4) * 1024 + row * 16 + (c % 4) * 4
                    } else {
                        1084 + p * pat_bytes + (row * n + c) * 4
                    };
                    if let Some(b) = d.get(o..o + 4) {
                        let period = (((b[0] & 15) as u16) << 8) | b[1] as u16;
                        let note = if period == 0 {
                            0
                        } else {
                            match PERIODS[0].iter().position(|&x| period >= x) {
                                Some(k) => k as u8 + 1,
                                None => NOTES as u8,
                            }
                        };
                        cells[row * n + c] = Cell { raw: if period != 0 && period < MIN_PERIOD as u16 { period } else { 0 }, note, smp: (b[0] & 0xf0) | (b[2] >> 4), eff: b[2] & 15, arg: b[3] };
                    }
                }
            }
            cells
        })
        .collect();
    let restart = if kind == Kind::Flt8 { (d[951] & 0x7f) as usize } else { 0 };
    let restart = if restart >= song_len { 0 } else { restart };
    let silence = m.alloc(4);
    if silence == 0 || !super::start_amplifier(m, n) {
        return None;
    }
    let title = d[..20].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect::<String>().trim().to_string();
    let pc = matches!(&d[1080..1084], [b'1'..=b'9', b'C', b'H', b'N'] | [b'1'..=b'3', b'0'..=b'9', b'C', b'H']);
    if pc {
        // PS3M plays every sample with its first four bytes cleared (the bytes its AudioStructs'
        // parts start with, 12 FST modules: silent at each turn of a loop from the sample's start)
        for smp in samples.iter().filter(|s| s.len != 0) {
            let a = smp.addr as usize;
            let k = (2 * smp.len as usize).min(4);
            if let Some(w) = m.bus.ram.get_mut(a..a + k) {
                w.fill(0);
            }
        }
    }
    let mut s = MultiMod {
        kind,
        pc,
        title,
        n,
        samples,
        song_len,
        restart,
        positions,
        patterns,
        silence,
        ch: vec![Ch::default(); n],
        first_tick: true,
        speed: 6,
        counter: 0,
        bpm: 125,
        bpm_set: false,
        song_pos: 0,
        old_song_pos: 0,
        pat_pos: 0,
        break_pos: 0,
        pos_jump: false,
        break_flag: false,
        got_break: false,
        got_pos_jump: false,
        last_break_arg: -1,
        last_jump_arg: -1,
        low_mask: 0xff,
        patt_delay: 0,
        patt_delay2: 0,
        visited: [false; 128],
        end_reached: false,
        restart_song: false,
        songend: false,
    };
    s.visited[0] = true;
    Some(Box::new(s))
}

impl Replayer for MultiMod {
    fn format(&self) -> &'static str {
        match self.kind {
            Kind::Pt => "Multichannel MOD",
            Kind::Flt8 => "StarTrekker 8",
        }
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn pc_music(&self) -> bool {
        self.pc
    }

    fn songend(&self) -> bool {
        self.songend
    }

    /// The CIA timer of ProTracker's tempo: 1773447 / BPM, plus one, E-clock ticks of
    /// 5 colour clocks.
    fn tick_clocks(&self) -> u64 {
        // PS3M (the original of the FastTracker and TakeTracker tags) sets a tempo from the NTSC
        // CIA clock (1789773 / BPM, plus one: seen in its timer writes), the PAL default until
        // the first tempo command
        let clock = if self.pc && self.bpm_set { 1_789_773 } else { 1_773_447 };
        (clock / self.bpm.max(1) as u64 + 1) * 5
    }

    fn timer_delay(&self) -> u64 {
        if !self.pc {
            return 0;
        }
        let set_up = (self.patterns.len() * self.n) as u64 * PS3M_PER_PATTERN_CHANNEL / 100;
        (PS3M_FIRST_TICK + set_up).saturating_sub(PORT_FIRST_TICK)
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.first_tick {
            self.first_tick = false;
            hw.voice_dma(u32::MAX >> (32 - self.n), false);
            for v in 0..self.n {
                hw.voice_loc(v, self.silence);
                hw.voice_len(v, 1);
                hw.voice_per(v, 0x71);
                hw.voice_vol(v, 0);
                self.ch[v].last_vol = 0;
            }
        }
        for c in self.ch.iter_mut() {
            c.tremolo = None;
        }
        if self.speed != 0 {
            self.counter += 1;
            if self.counter >= self.speed {
                self.counter = 0;
                if self.patt_delay2 != 0 {
                    self.no_new_all(hw);
                } else {
                    self.get_new_note(hw);
                }
                self.pat_pos += 1;
                if self.patt_delay != 0 {
                    self.patt_delay2 = self.patt_delay;
                    self.patt_delay = 0;
                }
                if self.patt_delay2 != 0 {
                    self.patt_delay2 -= 1;
                    if self.patt_delay2 != 0 {
                        self.pat_pos -= 1;
                    }
                }
                if self.break_flag {
                    self.break_flag = false;
                    self.pat_pos = self.break_pos;
                    self.break_pos = 0;
                }
                if self.pat_pos >= 64 {
                    self.next_position();
                }
            } else {
                self.no_new_all(hw);
            }
        } else {
            self.no_new_all(hw);
        }
        if self.pos_jump {
            self.next_position();
        }
        for v in 0..self.n {
            let c = &mut self.ch[v];
            let vol = c.tremolo.unwrap_or(c.volume).clamp(0, 64);
            if vol != c.last_vol {
                c.last_vol = vol;
                hw.voice_vol(v, vol as u16);
            }
        }
        if self.end_reached {
            self.end_reached = false;
            self.songend = true;
            if self.restart_song {
                self.restart_song = false;
                self.song_pos = self.restart;
                self.pat_pos = 0;
                self.visited = [false; 128];
            }
            self.visited[self.song_pos] = true;
        }
    }
}

impl MultiMod {
    fn pt(&self) -> bool {
        self.kind == Kind::Pt
    }

    fn next_position(&mut self) {
        self.song_pos += 1;
        if self.song_pos >= self.song_len {
            self.song_pos = self.restart;
            self.last_break_arg = -1;
            self.last_jump_arg = -1;
            self.end_reached = true;
        } else if self.visited[self.song_pos] {
            if !self.got_break || self.song_pos != self.old_song_pos {
                self.end_reached = true;
            } else if self.got_break && self.got_pos_jump && self.song_pos as i32 == self.last_jump_arg && self.break_pos as i32 == self.last_break_arg {
                self.end_reached = true;
                self.restart_song = true;
            }
        }
        self.visited[self.song_pos] = true;
        if self.got_break {
            self.last_break_arg = self.break_pos as i32;
        }
        if self.got_pos_jump {
            self.last_jump_arg = self.song_pos as i32;
        }
        self.pat_pos = self.break_pos;
        self.break_pos = 0;
        self.pos_jump = false;
        self.got_break = false;
        self.got_pos_jump = false;
    }

    fn set_position(&mut self, pos: usize) {
        self.old_song_pos = self.song_pos;
        self.song_pos = pos.wrapping_sub(1);
        self.break_pos = 0;
        self.pos_jump = true;
        self.got_pos_jump = true;
    }

    fn no_new_all(&mut self, hw: &mut Hw) {
        for i in 0..self.n {
            self.check_effects(i, hw);
        }
    }

    fn get_new_note(&mut self, hw: &mut Hw) {
        let pat = self.positions[self.song_pos.min(127)] as usize;
        for i in 0..self.n {
            let cell = self.patterns.get(pat).map_or(Cell::default(), |p| p[self.pat_pos.min(63) * self.n + i]);
            self.play_voice(i, cell, hw);
        }
    }

    // The channel, as NostalgicPlayer's IChannel: a period, a sample started at an
    // offset for a length with its loop, and silence.

    fn set_period(&mut self, i: usize, period: i32, hw: &mut Hw) {
        let c = &mut self.ch[i];
        if period > 0 && period != c.last_per {
            c.last_per = period;
            hw.voice_per(i, period.min(0xffff) as u16);
        }
    }

    /// Starts the channel's sample from `offset` bytes for `words`, then its loop.
    fn play_sample(&mut self, i: usize, offset: u32, words: u16, hw: &mut Hw) {
        let c = self.ch[i];
        hw.voice_dma(1 << i, false);
        hw.voice_loc(i, c.addr + offset);
        hw.voice_len(i, words);
        self.ch[i].last_per = -1;
        self.set_period(i, c.period, hw);
        hw.voice_dma(1 << i, true);
        hw.wait(40);
        self.set_loop(i, hw);
    }

    fn set_loop(&mut self, i: usize, hw: &mut Hw) {
        let c = self.ch[i];
        if c.loop_len > 0 {
            hw.voice_loc(i, c.addr + c.loop_start);
            hw.voice_len(i, c.loop_len);
        } else {
            // a sample without a loop plays on into a word of silence
            hw.voice_loc(i, self.silence);
            hw.voice_len(i, 1);
        }
        hw.wait(40);
    }

    fn mute(&mut self, i: usize, hw: &mut Hw) {
        hw.voice_dma(1 << i, false);
    }

    fn play_voice(&mut self, i: usize, cell: Cell, hw: &mut Hw) {
        let pt = self.pt();
        {
            let c = &self.ch[i];
            if c.cell.note == 0 && c.cell.smp == 0 && c.cell.eff == 0 && c.cell.arg == 0 {
                let p = c.period;
                self.set_period(i, p, hw);
            }
        }
        self.ch[i].cell = cell;
        let samp = cell.smp as usize;
        if samp != 0 && samp <= 31 {
            let s = self.samples[samp - 1];
            let c = &mut self.ch[i];
            c.sample = samp - 1;
            c.addr = s.addr;
            c.has_data = s.len != 0;
            c.length = s.len;
            c.start_offset = 0;
            c.fine = s.fine;
            c.volume = s.vol as i32;
            if s.loop_start != 0 && s.loop_len > 1 {
                c.loop_start = s.loop_start as u32 * 2;
                c.wave_start = c.loop_start;
                c.length = s.loop_start + s.loop_len;
                c.loop_len = s.loop_len;
            } else {
                c.loop_start = s.loop_start as u32 * 2;
                c.wave_start = c.loop_start;
                c.loop_len = s.loop_len;
            }
        }
        if cell.note != 0 {
            let cmd = cell.eff;
            let mut skip_to_end = false;
            if pt && cmd == 0xe && cell.arg & 0xf0 == 0x50 {
                self.set_fine_tune(i);
            } else {
                match cmd {
                    3 | 5 => {
                        self.set_tone_porta(i);
                        self.check_more_effects(i, hw);
                        return;
                    }
                    9 => self.check_more_effects(i, hw),
                    _ => {}
                }
            }
            let _ = &mut skip_to_end;
            let c = &mut self.ch[i];
            c.period = if cell.raw != 0 && self.pc { PERIODS[c.fine as usize][1] as i32 } else { PERIODS[c.fine as usize][cell.note as usize - 1] as i32 };
            if !pt || cmd != 0xe || cell.arg & 0xf0 != 0xd0 {
                if c.wave_ctl & 4 == 0 {
                    c.vib_pos = 0;
                }
                if c.wave_ctl & 64 == 0 {
                    c.tre_pos = 0;
                }
                let c = self.ch[i];
                if c.length > 0 && c.has_data {
                    self.play_sample(i, c.start_offset, c.length, hw);
                } else {
                    self.mute(i, hw);
                }
            }
        }
        self.check_more_effects(i, hw);
    }

    fn check_effects(&mut self, i: usize, hw: &mut Hw) {
        self.update_funk(i, hw);
        let (cmd, arg) = (self.ch[i].cell.eff, self.ch[i].cell.arg);
        if cmd == 0 && arg == 0 {
            return;
        }
        match cmd {
            0 => self.arpeggio(i, hw),
            1 => self.porta_up(i, hw),
            2 => self.porta_down(i, hw),
            3 => self.tone_portamento(i, false, hw),
            4 => self.vibrato(i, false, hw),
            5 => {
                self.tone_portamento(i, true, hw);
                self.volume_slide(i, arg);
            }
            6 => {
                self.vibrato(i, true, hw);
                self.volume_slide(i, arg);
            }
            0xe => self.e_commands(i, hw),
            _ => {
                let p = self.ch[i].period;
                self.set_period(i, p, hw);
                if cmd == 7 {
                    self.tremolo(i);
                } else if cmd == 0xa {
                    self.volume_slide(i, arg);
                }
            }
        }
    }

    fn check_more_effects(&mut self, i: usize, hw: &mut Hw) {
        let (cmd, arg) = (self.ch[i].cell.eff, self.ch[i].cell.arg);
        match cmd {
            9 => {
                if self.pt() {
                    self.sample_offset(i);
                }
            }
            0xb => self.set_position(arg as usize),
            0xc => self.ch[i].volume = arg.min(64) as i32,
            0xd => {
                let a = if self.pt() { arg } else { 0 };
                self.break_pos = ((a >> 4) & 15) as usize * 10 + (a & 15) as usize;
                if self.break_pos > 63 {
                    self.break_pos = 0;
                }
                self.pos_jump = true;
                self.got_break = true;
            }
            0xe => self.e_commands(i, hw),
            0xf => self.set_speed(arg),
            _ => {
                let p = self.ch[i].period;
                self.set_period(i, p, hw);
            }
        }
    }

    fn set_speed(&mut self, arg: u8) {
        if !self.pt() {
            if arg != 0 {
                self.speed = arg.min(31);
            }
        } else if arg > 0 {
            if arg >= 32 {
                self.bpm = arg as u32;
                self.bpm_set = true;
            } else {
                self.speed = arg;
                self.counter = 0;
            }
        } else {
            // a speed of 0 ends the song, which starts over
            self.end_reached = true;
            self.restart_song = true;
            self.set_position(self.restart);
        }
    }

    fn e_commands(&mut self, i: usize, hw: &mut Hw) {
        let arg = self.ch[i].cell.arg;
        if !self.pt() {
            self.filter(arg, hw);
            return;
        }
        let x = arg & 15;
        let first = self.counter == 0;
        match arg & 0xf0 {
            0x00 => self.filter(arg, hw),
            0x10 => {
                if first {
                    self.low_mask = 0x0f;
                    self.porta_up(i, hw);
                }
            }
            0x20 => {
                if first {
                    self.low_mask = 0x0f;
                    self.porta_down(i, hw);
                }
            }
            0x30 => self.ch[i].gliss_funk = (self.ch[i].gliss_funk & 0xf0) | x,
            0x40 => self.ch[i].wave_ctl = (self.ch[i].wave_ctl & 0xf0) | x,
            0x50 => self.set_fine_tune(i),
            0x60 => {
                if first {
                    let pat_pos = self.pat_pos;
                    let c = &mut self.ch[i];
                    if x != 0 {
                        if c.do_loop_patt_pos == -1 {
                            c.do_loop_patt_pos = pat_pos as i16;
                        }
                        // another E6x without a new E60 is ignored, to prevent an endless loop
                        if pat_pos as i16 != c.do_loop_patt_pos {
                            return;
                        }
                        if c.loop_count == 0 {
                            c.loop_count = x;
                        } else {
                            c.loop_count -= 1;
                        }
                        if c.loop_count != 0 {
                            self.break_pos = c.loop_patt_pos as usize;
                            self.break_flag = true;
                        }
                    } else {
                        c.loop_patt_pos = pat_pos as u8;
                        c.do_loop_patt_pos = -1;
                    }
                }
            }
            0x70 => self.ch[i].wave_ctl = (self.ch[i].wave_ctl & 0x0f) | (x << 4),
            // 0x80: Karplus-Strong in the source, not played
            0x90 => {
                if x != 0 && self.counter != 0 && self.ch[i].cell.note == 0 && self.counter % x == 0 {
                    let c = self.ch[i];
                    if c.length != 0 && c.has_data {
                        self.play_sample(i, 0, c.length, hw);
                    } else {
                        self.mute(i, hw);
                    }
                }
            }
            0xa0 => {
                if first {
                    let c = &mut self.ch[i];
                    c.volume = (c.volume + x as i32).min(64);
                }
            }
            0xb0 => {
                if first {
                    let c = &mut self.ch[i];
                    c.volume = (c.volume - x as i32).max(0);
                }
            }
            0xc0 => {
                if x == self.counter {
                    self.ch[i].volume = 0;
                }
            }
            0xd0 => {
                if x == self.counter && self.ch[i].cell.note != 0 {
                    let c = self.ch[i];
                    if c.length != 0 && c.has_data {
                        self.play_sample(i, 0, c.length, hw);
                    } else {
                        self.mute(i, hw);
                    }
                }
            }
            0xe0 => {
                if first && self.patt_delay2 == 0 {
                    self.patt_delay = x + 1;
                }
            }
            0xf0 => {
                if first {
                    let c = &mut self.ch[i];
                    c.gliss_funk = (c.gliss_funk & 0x0f) | (x << 4);
                    if x != 0 {
                        self.update_funk(i, hw);
                    }
                }
            }
            _ => {}
        }
    }

    /// The LED filter, through CIA-A's port A (bit 1 clear: filter on).
    fn filter(&mut self, arg: u8, hw: &mut Hw) {
        let x = hw.bus.read_byte(0xbfe001);
        hw.bus.write_byte(0xbfe001, if arg & 1 == 0 { x & !2 } else { x | 2 });
    }

    fn set_fine_tune(&mut self, i: usize) {
        self.ch[i].fine = self.ch[i].cell.arg & 15;
    }

    fn set_tone_porta(&mut self, i: usize) {
        let c = &mut self.ch[i];
        let period = PERIODS[0][c.cell.note as usize - 1];
        let mut k = PERIODS[c.fine as usize].iter().position(|&p| p <= period).map_or(NOTES - 1, |k| k);
        if c.fine > 7 && k != 0 {
            k -= 1;
        }
        let period = PERIODS[c.fine as usize][k] as i32;
        c.wanted = period;
        c.tone_dir = false;
        if c.period == period {
            c.wanted = 0;
        } else if c.period > period {
            c.tone_dir = true;
        }
    }

    /// Inverts a byte of the loop, as the funk repeat does.
    fn update_funk(&mut self, i: usize, hw: &mut Hw) {
        let c = &mut self.ch[i];
        let gf = c.gliss_funk >> 4;
        if gf == 0 {
            return;
        }
        c.funk_offset = c.funk_offset.wrapping_add(FUNK[gf as usize]);
        if c.funk_offset >= 128 {
            c.funk_offset = 0;
            let mut w = c.wave_start + 1;
            if w >= c.loop_start + c.loop_len as u32 * 2 {
                w = c.loop_start;
            }
            c.wave_start = w;
            if c.has_data {
                let a = c.addr + w;
                let v = hw.bus.r8(a);
                hw.bus.w8(a, !v);
            }
        }
    }

    fn arpeggio(&mut self, i: usize, hw: &mut Hw) {
        let c = &self.ch[i];
        let period = if c.cell.arg != 0 {
            let arp = match self.counter % 3 {
                1 => c.cell.arg >> 4,
                2 => c.cell.arg & 15,
                _ => 0,
            } as usize;
            let k = PERIODS[c.fine as usize].iter().position(|&p| p as i32 <= c.period).unwrap_or(NOTES);
            PERIODS[c.fine as usize][(k + arp).min(NOTES - 1)] as i32
        } else {
            c.period
        };
        self.set_period(i, period, hw);
    }

    fn porta_up(&mut self, i: usize, hw: &mut Hw) {
        let c = &mut self.ch[i];
        c.period = (c.period - (c.cell.arg & self.low_mask) as i32).max(MIN_PERIOD);
        self.low_mask = 0xff;
        let p = c.period;
        self.set_period(i, p, hw);
    }

    fn porta_down(&mut self, i: usize, hw: &mut Hw) {
        let c = &mut self.ch[i];
        c.period = (c.period + (c.cell.arg & self.low_mask) as i32).min(MAX_PERIOD);
        self.low_mask = 0xff;
        let p = c.period;
        self.set_period(i, p, hw);
    }

    fn tone_portamento(&mut self, i: usize, skip: bool, hw: &mut Hw) {
        let c = &mut self.ch[i];
        if !skip && c.cell.arg != 0 {
            c.tone_speed = c.cell.arg;
            c.cell.arg = 0;
        }
        if c.wanted != 0 {
            let mut period;
            if c.tone_dir {
                period = c.period - c.tone_speed as i32;
                if c.wanted >= period {
                    period = c.wanted;
                    c.wanted = 0;
                }
            } else {
                period = c.period + c.tone_speed as i32;
                if c.wanted <= period {
                    period = c.wanted;
                    c.wanted = 0;
                }
            }
            c.period = period;
            if c.gliss_funk & 15 != 0 {
                let k = PERIODS[c.fine as usize].iter().position(|&p| p as i32 <= period).map_or(NOTES - 1, |k| k);
                period = PERIODS[c.fine as usize][k] as i32;
            }
            self.set_period(i, period, hw);
        }
    }

    /// The vibrato's or tremolo's offset from its waveform: sine, ramp down or square.
    fn wave(pos: i8, ctl: u8) -> u32 {
        let p = ((pos as i32 / 4) & 0x1f) as usize;
        match ctl & 3 {
            0 => SINE[p] as u32,
            1 => {
                let v = (p * 8) as u32 & 0xff;
                if pos < 0 {
                    255 - v
                } else {
                    v
                }
            }
            _ => 255,
        }
    }

    fn vibrato(&mut self, i: usize, skip: bool, hw: &mut Hw) {
        let c = &mut self.ch[i];
        let arg = c.cell.arg;
        if !skip && arg != 0 {
            let mut v = c.vib_cmd;
            if arg & 15 != 0 {
                v = (v & 0xf0) | (arg & 15);
            }
            if arg & 0xf0 != 0 {
                v = (v & 0x0f) | (arg & 0xf0);
            }
            c.vib_cmd = v;
        }
        let add = (Self::wave(c.vib_pos, c.wave_ctl) * (c.vib_cmd & 15) as u32 / 128) as i32;
        let period = if c.vib_pos < 0 { c.period - add } else { c.period + add };
        c.vib_pos = c.vib_pos.wrapping_add(((c.vib_cmd / 4) & 0x3c) as i8);
        self.set_period(i, period, hw);
    }

    fn tremolo(&mut self, i: usize) {
        let c = &mut self.ch[i];
        let arg = c.cell.arg;
        if arg != 0 {
            let mut v = c.tre_cmd;
            if arg & 15 != 0 {
                v = (v & 0xf0) | (arg & 15);
            }
            if arg & 0xf0 != 0 {
                v = (v & 0x0f) | (arg & 0xf0);
            }
            c.tre_cmd = v;
        }
        let add = (Self::wave(c.tre_pos, c.wave_ctl >> 4) * (c.tre_cmd & 15) as u32 / 64) as i32;
        let vol = if c.tre_pos < 0 { (c.volume - add).max(0) } else { (c.volume + add).min(64) };
        c.tremolo = Some(vol);
        c.tre_pos = c.tre_pos.wrapping_add(((c.tre_cmd / 4) & 0x3c) as i8);
    }

    fn sample_offset(&mut self, i: usize) {
        let c = &mut self.ch[i];
        if c.cell.arg != 0 {
            c.sample_offset = c.cell.arg;
        }
        let offset = (c.sample_offset as u32 * 128) as u16;
        if offset < c.length {
            c.length -= offset;
            c.start_offset = offset as u32 * 2;
        } else {
            c.length = 1;
        }
    }

    fn volume_slide(&mut self, i: usize, arg: u8) {
        let c = &mut self.ch[i];
        let up = arg >> 4;
        if up != 0 {
            c.volume = (c.volume + up as i32).min(64);
        } else {
            c.volume = (c.volume - (arg & 15) as i32).max(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A module of `n` channels with one 64-byte looped sample (loop from byte 16) and
    /// one pattern with `cells` at (row, channel).
    fn module(tag: &[u8; 4], n: usize, cells: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
        let mut d = vec![0u8; 1084];
        d[..8].copy_from_slice(b"test mod");
        d[20..26].copy_from_slice(b"sample");
        d[42..44].copy_from_slice(&32u16.to_be_bytes());
        d[45] = 48;
        d[46..48].copy_from_slice(&8u16.to_be_bytes());
        d[48..50].copy_from_slice(&24u16.to_be_bytes());
        d[950] = 1;
        d[1080..1084].copy_from_slice(tag);
        let mut pat = vec![0u8; 256 * n];
        for &(row, ch, c) in cells {
            let o = if tag == b"FLT8" { (ch / 4) * 1024 + row * 16 + (ch % 4) * 4 } else { (row * n + ch) * 4 };
            pat[o..o + 4].copy_from_slice(&c);
        }
        d.extend(pat);
        d.extend((0..64u8).map(|i| i.wrapping_mul(3)));
        d
    }

    /// Period `p`, sample `s`, effect `e`, parameter `a`.
    fn cell(p: u16, s: u8, e: u8, a: u8) -> [u8; 4] {
        [(s & 0xf0) | (p >> 8) as u8, p as u8, (s << 4) | e, a]
    }

    /// The format and the amplifier's pokes: voice, register and value.
    fn play(d: Vec<u8>, ticks: usize) -> (String, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.mod"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, log.iter().filter(|w| w.0 & 0xFF_0000 == crate::amplifier::LOG_BASE).map(|w| (w.0 & 0xFFFF, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], voice: usize, r: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == ((voice as u32) << 4 | r)).map(|w| w.1).collect()
    }

    #[test]
    fn claims_the_multichannel_tags_only() {
        for (tag, n) in [(b"6CHN", 6), (b"8CHN", 8), (b"CD81", 8), (b"10CH", 10), (b"32CH", 32), (b"FLT8", 8)] {
            let d = module(tag, n, &[]);
            assert_eq!(kind_of(&d).map(|k| k.0), Some(n), "{}", String::from_utf8_lossy(tag));
        }
        for tag in [b"M.K.", b"4CHN", b"FLT4", b"M!K!", b"33CH", b"0CHN", b"xxxx"] {
            let d = module(tag, 4, &[]);
            assert_eq!(kind_of(&d), None, "{}", String::from_utf8_lossy(tag));
        }
        // too short for one pattern, or no positions
        assert_eq!(kind_of(&module(b"8CHN", 8, &[])[..1500]), None);
        let mut d = module(b"6CHN", 6, &[]);
        d[950] = 0;
        assert_eq!(kind_of(&d), None);
    }

    #[test]
    fn plays_eight_channels_with_a_loop_and_an_arpeggio() {
        let mut cells = vec![(1, 0, cell(0, 0, 0, 0))];
        cells.push((0, 0, cell(428, 1, 0, 0)));
        cells.push((0, 7, cell(214, 1, 0, 0)));
        cells.push((1, 0, cell(0, 0, 0, 0x37)));
        let (format, log) = play(module(b"8CHN", 8, &cells), 40);
        assert_eq!(format, "Multichannel MOD");
        // the first row plays on channels 0 and 7: its sample once to the loop's end
        // (8 + 24 words... the loop is 24 words from word 8: 32), then the loop
        assert_eq!(reg(&log, 0, 4)[..2], [1, 32]);
        assert_eq!(reg(&log, 0, 4)[2], 24);
        assert_eq!(reg(&log, 7, 6)[1], 214);
        assert_eq!(reg(&log, 3, 6).len(), 1, "an idle channel only gets the initial period");
        // the arpeggio of the next row: +3 and +7 semitones from 428, then the note again
        let per = reg(&log, 0, 6);
        assert_eq!(per[2..6], [360, 285, 428, 360]);
        // the volume of the sample
        assert!(reg(&log, 0, 8).contains(&48));
    }

    #[test]
    fn effects_volume_speed_and_break() {
        let cells = [
            (0, 1, cell(428, 1, 0xc, 20)),
            (0, 2, cell(428, 1, 0xf, 3)),
            (1, 1, cell(0, 0, 0xa, 0x02)),
            (2, 1, cell(0, 0, 0xd, 0x00)),
        ];
        let (_, log) = play(module(b"6CHN", 6, &cells), 30);
        let vol = reg(&log, 1, 8);
        // set volume 20, then the slide down by 2 on each of the next two ticks
        assert_eq!(vol[1..4], [20, 18, 16]);
        // speed 3 on the second voice: rows come every 3 ticks, so the break at row 2
        // wraps the song (one position) and the first row plays again
        assert!(reg(&log, 2, 4).len() >= 4);
    }

    #[test]
    fn starrekker_8_reads_two_patterns_for_eight_channels() {
        let cells = [(0, 1, cell(428, 1, 0, 0)), (0, 5, cell(370, 1, 0, 0))];
        let (format, log) = play(module(b"FLT8", 8, &cells), 20);
        assert_eq!(format, "StarTrekker 8");
        assert_eq!(reg(&log, 1, 6)[1], 428);
        // 370 is quantised to the table's period below it
        assert_eq!(reg(&log, 5, 6)[1], 360);
    }

    #[test]
    fn four_channel_modules_stay_with_the_builtin_player() {
        for tag in [b"M.K.", b"4CHN", b"FLT4"] {
            let d = module(tag, 4, &[(0, 0, cell(428, 1, 0, 0))]);
            assert!(crate::open::detect(&d).is_some_and(|f| f != "Multichannel MOD" && f != "StarTrekker 8"));
        }
        assert_eq!(crate::open::detect(&module(b"8CHN", 8, &[])).as_deref(), Some("Multichannel MOD"));
    }

    /// Damaged modules play without a panic.
    #[test]
    fn survives_damage() {
        let mut seed = 99u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as usize
        };
        for k in 0..60 {
            let n = [6usize, 8, 12, 32][k % 4];
            let tag: &[u8; 4] = [b"6CHN", b"8CHN", b"12CH", b"32CH"][k % 4];
            let mut d = module(tag, n, &[(0, 0, cell(428, 1, 0, 0)), (1, 1, cell(300, 1, 0xe, 0x91))]);
            for _ in 0..1 + next() % 8 {
                let at = 20 + next() % (d.len() - 20);
                d[at] = next() as u8;
            }
            let (_, _) = {
                let mut e = Engine::new(48000.0);
                e.open_replayers = true;
                if e.play_data(Path::new("test.mod"), d).is_ok() {
                    let mut out = vec![0f32; 2 * 960 * 200];
                    e.render(&mut out);
                }
                (0, 0)
            };
        }
    }
}
