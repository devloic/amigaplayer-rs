//! The 15-instrument SoundTrackers, ported from NostalgicPlayer's ModTracker player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer):
//! its identification of the variant (`ModTrackerIdentifier.Check15SamplesModule`,
//! with its sanity checks on the sample names, sizes, positions and pattern data)
//! and the per-variant playback of Ultimate SoundTracker, D.O.C. SoundTracker II,
//! VI and IX, Master SoundTracker and SoundTracker 2.x.
//!
//! The 31-instrument formats (NoiseTracker, ProTracker and the rest of NostalgicPlayer's
//! agent) stay with the built-in ProTracker; a module with a four-character tag at
//! offset 1080 is not claimed here.
//!
//! Where the original players' register writes differ from what NostalgicPlayer asks
//! of its mixer, this follows the originals (see `tools/replay-batch.py`): the
//! write logs were compared with UADE's "Ultimate Soundtracker" player for the
//! Ultimate SoundTracker variants and its "Soundtracker IV" player for the others.
//! The module is read where it lies in guest memory, and its samples are played from
//! there.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

const NAMES: [&str; 7] = [
    "Ultimate SoundTracker 1.0",
    "Ultimate SoundTracker 1.8",
    "SoundTracker II",
    "SoundTracker VI",
    "SoundTracker IX",
    "Master SoundTracker",
    "SoundTracker 2.x",
];

pub const OPEN_UST10: Open = Open { name: NAMES[0], detect: |d| variant(d) == Some(Var::Ust10), new: |d, b, m| new(d, b, m, Var::Ust10) };
pub const OPEN_UST18: Open = Open { name: NAMES[1], detect: |d| variant(d) == Some(Var::Ust18), new: |d, b, m| new(d, b, m, Var::Ust18) };
pub const OPEN_ST2: Open = Open { name: NAMES[2], detect: |d| variant(d) == Some(Var::StII), new: |d, b, m| new(d, b, m, Var::StII) };
pub const OPEN_ST6: Open = Open { name: NAMES[3], detect: |d| variant(d) == Some(Var::StVI), new: |d, b, m| new(d, b, m, Var::StVI) };
pub const OPEN_ST9: Open = Open { name: NAMES[4], detect: |d| variant(d) == Some(Var::StIX), new: |d, b, m| new(d, b, m, Var::StIX) };
pub const OPEN_MST: Open = Open { name: NAMES[5], detect: |d| variant(d) == Some(Var::Mst10), new: |d, b, m| new(d, b, m, Var::Mst10) };
pub const OPEN_ST2X: Open = Open { name: NAMES[6], detect: |d| variant(d) == Some(Var::St2x), new: |d, b, m| new(d, b, m, Var::St2x) };

/// The variants, in NostalgicPlayer's order (a later one is a newer tracker).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Var {
    Ust10 = 1,
    Ust18,
    StII,
    StVI,
    StIX,
    Mst10,
    St2x,
}

impl Var {
    fn name(self) -> &'static str {
        NAMES[self as usize - 1]
    }
}

const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214,
    202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];
/// The vibrato table of the trackers' sine waveform.
const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141,
    120, 97, 74, 49, 24,
];
const MIN_PERIOD: u16 = 113;
const MAX_PERIOD: u16 = 856;

fn b(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The variant of a 15-instrument module, as NostalgicPlayer's
/// `ModTrackerIdentifier.TestModule` finds it, or None for any other file.
pub fn variant(d: &[u8]) -> Option<Var> {
    if d.len() < 1468 {
        return None;
    }
    if &d[0..4] == b"FORM" && &d[8..12] == b"MODL" {
        return None;
    }
    if &d[1464..1468] == b"MTN\0" || &d[1464..1468] == b"IT10" {
        return None;
    }
    // a tag of four printable characters: the 31-instrument check (not ours); the
    // tag is read with its trailing zeros trimmed, so one that is shorter is none
    let mark = &d[1080..1084];
    let len = mark.iter().rposition(|&c| c != 0).map_or(0, |p| p + 1);
    if len == 4 && mark.iter().all(|&c| (32..=127).contains(&c)) {
        return None;
    }
    check15(d)
}

fn check15(d: &[u8]) -> Option<Var> {
    let n = d.len();
    if d.starts_with(b"FamiTracker ") {
        return None;
    }
    let mut mv = Var::Ust10;
    let up = |mv: &mut Var, v: Var| *mv = (*mv).max(v);
    let mut disk = 0;
    let mut big = false;
    let mut slen = 0usize;
    for i in 0..15 {
        let o = 20 + i * 30;
        if i < 7 {
            for j in 2..10 {
                let c = d[o + j];
                if c == 0 {
                    break;
                }
                if !(32..=127).contains(&c) {
                    return None;
                }
            }
        }
        if matches!(d[o], b'S' | b's') && matches!(d[o + 1], b'T' | b't') && d[o + 2] == b'-' && d[o + 5] == b':' {
            disk += 1;
        }
        let ln = u16::from_be_bytes([d[o + 22], d[o + 23]]) as usize * 2;
        if ln > 9999 {
            up(&mut mv, Var::Mst10);
            big = true;
        }
        slen += ln;
    }
    if disk > 0 {
        up(&mut mv, if disk == 15 { Var::Ust18 } else { Var::StII });
    }
    let (song_len, init) = (d[470], d[471]);
    if song_len == 0 || song_len > 128 {
        return None;
    }
    let pos = &d[472..600];
    let mut used = [false; 256];
    for &p in pos {
        used[p as usize] = true;
    }
    if used[64..].iter().any(|&u| u) {
        return None;
    }
    let max = pos.iter().copied().max().unwrap() as usize;
    let calc = (max + 1) * 1024 + 0x258 + slen;
    if calc + 0x640 < n || calc > n + 0x200 {
        return None;
    }
    let (mut old_arp, mut new_arp, mut vslide, mut d00, mut speed, mut filt, mut empty) = (false, false, false, false, false, false, false);
    let mut fp = 600;
    for p in 0..64 {
        if !used[p] {
            if p < max {
                fp += 1024;
            }
            continue;
        }
        let (mut dch, mut dc, mut dc2) = (-1i32, 0, 0);
        let (mut ech, mut ec) = (-1i32, 0);
        for j in 0..256usize {
            let (a, bb, c, dd) = (b(d, fp), b(d, fp + 1), b(d, fp + 2), b(d, fp + 3));
            fp += 4;
            if a == 0 && bb == 0 && c == 0 && dd == 0 {
                empty = true;
            }
            let eff = c & 15;
            let ch = (j % 4) as i32;
            if ch == dch {
                if a == 0 && bb == 0 && ((c == 0 && dd == 0) || eff == 13) {
                    dc += 1;
                    if dc >= 5 && dc2 == 0 {
                        mv = Var::St2x;
                        vslide = false;
                    }
                } else if eff != 13 && eff != 12 {
                    dch = -1;
                    if !vslide {
                        mv = Var::St2x;
                    }
                }
            }
            if ch == ech {
                ec += 1;
            }
            match eff {
                0 => {
                    if dd != 0 {
                        if !old_arp {
                            up(&mut mv, Var::StII);
                            new_arp = true;
                        }
                    } else if ch == ech && ec == 1 && !vslide {
                        up(&mut mv, Var::StIX);
                        filt = true;
                    }
                }
                1 | 2 => {
                    if dd > 0x1f && !big && !new_arp {
                        mv = if disk > 0 { Var::Ust18 } else { Var::Ust10 };
                        old_arp = true;
                    } else if eff == 1 && dd < 3 {
                        up(&mut mv, Var::StII);
                    }
                }
                0xb => mv = Var::St2x,
                0xc => up(&mut mv, Var::StII),
                0xd => {
                    if dch == -1 {
                        if mv >= Var::Mst10 {
                            if dd == 0 {
                                mv = Var::St2x;
                                d00 = true;
                            } else if !d00 {
                                if !filt {
                                    vslide = true;
                                    mv = if speed { Var::StVI } else { Var::StII };
                                } else {
                                    mv = Var::St2x;
                                }
                            }
                        } else {
                            up(&mut mv, Var::StII);
                            if dd == 0 && !vslide {
                                dch = ch;
                                dc = 0;
                                dc2 = 0;
                            } else {
                                vslide = true;
                                dch = ch;
                                dc = 0;
                                if mv > Var::StVI {
                                    mv = if speed { Var::StVI } else { Var::StII };
                                }
                            }
                        }
                    } else if ch == dch {
                        dc2 += 1;
                        if dc2 > 2 {
                            vslide = true;
                            dch = -1;
                            dc = 0;
                        }
                    }
                }
                0xe => {
                    up(&mut mv, Var::StII);
                    if dd == 1 {
                        ech = ch;
                        ec = 0;
                    }
                }
                0xf => {
                    up(&mut mv, Var::StVI);
                    speed = true;
                }
                _ => {}
            }
        }
    }
    if !empty {
        return None;
    }
    if init != 0 && init != 0x78 && disk == 15 {
        // jjk55 and Cut it run too fast to be upgraded
        if (song_len != 0x4a || init != 0xb3) && (song_len != 0x1e || init != 0xa0) {
            if mv > Var::Ust18 {
                up(&mut mv, Var::StIX);
            } else {
                up(&mut mv, Var::Ust18);
            }
        }
    }
    if init != 0 && init < 60 && matches!(mv, Var::Ust10 | Var::StII | Var::StVI | Var::Mst10 | Var::St2x) {
        return None;
    }
    // NostalgicPlayer refuses to load a module that misses more than 512 bytes of a sample
    let mut at = 600 + (max + 1) * 1024;
    for i in 0..15 {
        let ln = u16::from_be_bytes([d[20 + i * 30 + 22], d[20 + i * 30 + 23]]) as usize * 2;
        if ln != 0 {
            if n.saturating_sub(at) + 512 < ln {
                return None;
            }
            at += ln;
        }
    }
    Some(mv)
}

#[derive(Clone, Copy, Default)]
struct Smp {
    addr: u32,
    len: u16,
    vol: u8,
    /// Loop start in bytes and length in words, as NostalgicPlayer's loader fixes them.
    rep: u16,
    replen: u16,
}

#[derive(Clone, Copy, Default)]
struct Chan {
    note: u8,
    sample: Option<usize>,
    eff: u8,
    arg: u8,
    /// What the next note plays: address, length in words, loop (bytes into it, words).
    addr: u32,
    len: u16,
    loop_off: u32,
    loop_len: u16,
    looped: bool,
    /// Where the voice goes on after its note (the D.O.C. players write it back every row).
    lp_addr: u32,
    lp_len: u16,
    has_lp: bool,
    period: u16,
    /// The period the bend of Ultimate SoundTracker works on (see `play_voice`).
    bend: u16,
    vol: u8,
    auto_slide: bool,
    auto_arg: u8,
    tremolo_cmd: u8,
    tremolo_pos: i8,
}

pub struct SoundTracker {
    var: Var,
    base: u32,
    title: String,
    samples: [Smp; 15],
    song_len: usize,
    positions: [u8; 128],
    /// CIA timer latch of a tick, and whether the module's tempo byte sets it.
    timer: u32,
    speed: u8,
    counter: u8,
    song_pos: usize,
    pat_pos: usize,
    break_pos: usize,
    pos_jump: bool,
    low_mask: u8,
    ch: [Chan; 4],
    /// The song has wrapped or come to a position it played before.
    songend: bool,
    visited: [bool; 128],
}

const DEFAULT_TIMER: u32 = crate::deli::DEFAULT_TIMER as u32;

fn new(d: &[u8], base: u32, m: &mut Machine, var: Var) -> Option<Box<dyn Replayer>> {
    if variant(d) != Some(var) {
        return None;
    }
    let mut positions = [0u8; 128];
    positions.copy_from_slice(&d[472..600]);
    let npat = *positions.iter().max().unwrap() as usize + 1;
    let mut samples = [Smp::default(); 15];
    let mut addr = base + (600 + npat * 1024) as u32;
    for (i, s) in samples.iter_mut().enumerate() {
        let o = 20 + i * 30;
        let len = u16::from_be_bytes([d[o + 22], d[o + 23]]);
        // the loop start is in bytes; the originals use it as it is (an odd one too)
        let mut rep = u16::from_be_bytes([d[o + 26], d[o + 27]]);
        let mut replen = u16::from_be_bytes([d[o + 28], d[o + 29]]);
        // the D.O.C. players take a loop length of one word for no loop, and any other (a
        // zero too) for a loop
        if rep / 2 > len {
            rep = 0;
            replen = 1;
        }
        *s = Smp { addr, len, vol: d[o + 25], rep, replen };
        addr += len as u32 * 2;
    }
    let title = d[..20].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect::<String>().trim().to_string();
    let tempo = d[471];
    let timer = if var <= Var::Ust18 {
        // the variable tempo of Ultimate SoundTracker 1.8 (the reference player takes it
        // for every Ultimate SoundTracker, even 0): a CIA timer of (240 - tempo) * 122
        // E-clock ticks, the byte wrapping, and the default for 240
        match 240u8.wrapping_sub(tempo) {
            0 => DEFAULT_TIMER,
            n => n as u32 * 122,
        }
    } else if var == Var::StIX && tempo >= 100 && tempo != 0x78 && tempo < 240 {
        // the same in D.O.C. SoundTracker IX, which NostalgicPlayer plays at 50 Hz for
        // the usual 0x78
        (240 - tempo as u32) * 122
    } else {
        DEFAULT_TIMER
    };
    let ust = var <= Var::Ust18;
    let mut st = SoundTracker {
        var,
        base,
        title,
        samples,
        song_len: d[470] as usize,
        positions,
        timer,
        speed: 6,
        counter: if ust { 1 } else { 0 },
        song_pos: 0,
        pat_pos: 0,
        break_pos: 0,
        pos_jump: false,
        low_mask: 0xff,
        ch: [Chan::default(); 4],
        songend: false,
        visited: [false; 128],
    };
    st.visited[0] = true;
    // the player starts with every voice's volume at 0
    let mut hw = Hw { bus: &mut m.bus };
    for i in 0..4 {
        hw.vol(i, 0);
        hw.wait(10);
    }
    Some(Box::new(st))
}

impl Replayer for SoundTracker {
    fn format(&self) -> &'static str {
        self.var.name()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.song_pos == 0 && self.pat_pos == 0 && self.counter <= 1 && !self.ch.iter().any(|c| c.note != 0) {
            // the first interrupt silences the voices
        }
        self.counter = self.counter.wrapping_add(1);
        // a speed of 0 (F00, or a parameter with a zero low nibble) is one the counter
        // never reaches: the originals go on without a next row
        if self.speed != 0 && self.counter >= self.speed {
            self.counter = 0;
            self.row(hw);
            self.pat_pos += 1;
            if self.pat_pos >= 64 {
                self.next_position();
            }
        } else {
            self.effects(hw);
        }
        if self.pos_jump {
            self.next_position();
        }
    }
}

impl SoundTracker {
    fn ust(&self) -> bool {
        self.var <= Var::Ust18
    }

    fn next_position(&mut self) {
        self.song_pos += 1;
        if self.song_pos >= self.song_len {
            self.song_pos = 0;
            self.songend = true;
        } else if self.visited[self.song_pos] {
            self.songend = true;
        }
        self.visited[self.song_pos] = true;
        self.pat_pos = self.break_pos;
        self.break_pos = 0;
        self.pos_jump = false;
    }

    fn row(&mut self, hw: &mut Hw) {
        let pat = self.positions[self.song_pos] as u32;
        let row = self.base + 600 + pat * 1024 + self.pat_pos as u32 * 16;
        let doc = !self.ust();
        let mut mask = 0u16;
        let mut touched = [false; 4];
        for i in 0..4 {
            let a = row + 4 * i as u32;
            let (b0, b1, b2, b3) = (hw.bus.r8(a), hw.bus.r8(a + 1), hw.bus.r8(a + 2), hw.bus.r8(a + 3));
            let period = ((b0 as u16 & 15) << 8) | b1 as u16;
            let smp = (b0 & 0xf0) | (b2 >> 4);
            if doc && period != 0 && i > 0 {
                hw.wait(129);
            }
            let (started, set) = self.play_voice(i, period, smp, b2 & 15, b3, hw);
            if started {
                mask |= 1 << i;
            }
            // (Ultimate SoundTracker does not put an empty sample that has no note to it on its
            // list of voices to restore)
            touched[i] = started || (set && (doc || self.samples[smp as usize - 1].len != 0));
            if doc {
                // the modulation of the voice is switched off
                hw.bus.write_word(0xDFF09E, 0x11 << i);
                if matches!(self.ch[i].eff, 0xc) {
                    self.row_effect(i, hw);
                } else {
                    hw.wait(247);
                    self.row_effect(i, hw);
                }
            } else {
                self.row_effect(i, hw);
            }
        }
        if mask != 0 || doc || touched.iter().any(|&t| t) {
            let now = hw.bus.clock;
            hw.wait((now / LINE + 11) * LINE + 96 - now);
        }
        hw.dmacon(0x8000 | mask);
        if doc {
            hw.wait(34);
            for i in (0..4).rev() {
                let c = self.ch[i];
                if c.has_lp && !c.looped {
                    hw.loc(i, c.lp_addr);
                    hw.wait(10);
                    hw.len(i, c.lp_len);
                    hw.wait(35);
                }
            }
        } else if touched.iter().any(|&t| t) {
            hw.wait(34);
            for i in 0..4 {
                // a sample without a loop plays one word after it
                if touched[i] && !self.ch[i].looped {
                    hw.len(i, 1);
                    hw.wait(35);
                }
            }
        }
    }

    /// Returns whether a note started on the voice, and whether it got a sample.
    fn play_voice(&mut self, i: usize, period: u16, smp: u8, eff: u8, arg: u8, hw: &mut Hw) -> (bool, bool) {
        let var = self.var;
        let c = &mut self.ch[i];
        // the originals play the period as the pattern has it
        let note = u8::from(period != 0);
        c.note = note;
        c.eff = eff;
        c.arg = arg;
        let mut set = false;
        if smp != 0 && smp <= 15 {
            set = true;
            let s = self.samples[smp as usize - 1];
            c.sample = Some(smp as usize - 1);
            c.addr = s.addr;
            c.len = s.len;
            c.vol = s.vol;
            // (Ultimate SoundTracker takes only a length of two words or more for a loop)
            if s.replen > 1 || (s.replen == 0 && self.var > Var::Ust18) {
                // only the loop part is played
                c.addr = s.addr + s.rep as u32;
                // the D.O.C. players start a loop that begins at 0 from the sample's own length
                c.len = if self.var > Var::Ust18 && s.rep == 0 { s.len } else { s.replen };
                c.loop_off = s.rep as u32;
                c.loop_len = s.replen;
                c.looped = true;
            } else {
                c.loop_off = 0;
                c.loop_len = 0;
                c.looped = false;
            }
            // any sample arms the voice's restore, an empty one too
            c.has_lp = true;
            c.lp_addr = s.addr + c.loop_off;
            c.lp_len = if c.looped { c.loop_len } else { 1 };
            hw.vol(i, c.vol.min(64) as u16);
        }
        let mut started = false;
        if note != 0 {
            c.period = period;
            c.bend = period;
            c.tremolo_pos = 0;
            if var == Var::StII || var == Var::StVI {
                if eff == 0xe {
                    c.auto_slide = true;
                    c.auto_arg = arg;
                } else if arg == 0 {
                    c.auto_slide = false;
                    c.auto_arg = 0;
                }
            }
            {
                let t = if var <= Var::Ust18 { [75, 35, 10, 10] } else { [28, 12, 10, 8] };
                hw.wait(t[0]);
                hw.dmacon(1 << i);
                hw.wait(t[1]);
                hw.loc(i, c.addr);
                hw.wait(t[2]);
                hw.len(i, c.len);
                hw.wait(t[3]);
                hw.per(i, c.period);
                hw.wait(if var <= Var::Ust18 { 185 } else { 43 });
                started = true;
            }
        } else if var <= Var::Ust18 {
            // the bend works on the period of the row's own cell, none when there is no note
            c.bend = 0;
            hw.wait(79);
        }
        (started, set)
    }

    /// The effects that act on the row itself, after the voice's note.
    fn row_effect(&mut self, i: usize, hw: &mut Hw) {
        let var = self.var;
        let (eff, arg) = (self.ch[i].eff, self.ch[i].arg);
        match eff {
            0xc if var > Var::Ust18 => {
                self.ch[i].vol = arg.min(64);
                hw.wait(149);
                // written as the byte repeated, unchecked
                hw.vol(i, arg as u16 * 0x101);
                hw.wait(98);
            }
            3..=0xb if var == Var::StII || var == Var::StVI => {
                // the modulation of the next voice by this one: volume (3, 6, 9), period (4, 7, a) or both
                let bits = match (eff - 3) % 3 {
                    0 => 1u16 << i,
                    1 => 0x10 << i,
                    _ => 0x11 << i,
                };
                hw.wait(94);
                hw.bus.write_word(0xDFF09E, 0x8000 | bits);
                hw.wait(108);
            }
            0xe if var >= Var::StIX => self.filter(hw, arg),
            0xf if var > Var::Ust18 => {
                self.speed = arg & 15;
            }
            0xb if var == Var::St2x => {
                self.song_pos = (arg as usize).wrapping_sub(1);
                self.break_pos = 0;
                self.pos_jump = true;
            }
            0xd if var == Var::St2x => {
                self.break_pos = 0;
                self.pos_jump = true;
            }
            _ => {}
        }
    }

    fn effects(&mut self, hw: &mut Hw) {
        for i in 0..4 {
            let (eff, arg) = (self.ch[i].eff, self.ch[i].arg);
            // the originals do nothing for an effect without a parameter
            if arg == 0 {
                continue;
            }
            let var = self.var;
            if self.ust() {
                match eff {
                    1 => self.arpeggio(i, hw),
                    2 => self.pitch_bend(i, hw),
                    _ => {}
                }
            } else if var == Var::StII || var == Var::StVI {
                if self.ch[i].auto_slide {
                    self.volume_slide(i, self.ch[i].auto_arg, hw);
                }
                match eff {
                    0 | 3 | 4 | 5 => self.arpeggio(i, hw),
                    1 | 6 | 7 | 8 => self.porta(i, true, hw),
                    2 | 9 | 0xa | 0xb => self.porta(i, false, hw),
                    0xd => self.volume_slide(i, arg, hw),
                    _ => {}
                }
            } else {
                match eff {
                    0 => self.arpeggio(i, hw),
                    1 => self.porta(i, true, hw),
                    2 => self.porta(i, false, hw),
                    // NostalgicPlayer's vibrato has no depth or speed for these trackers: the
                    // period goes to Paula unchanged
                    4 | 6 => {
                        if eff == 6 {
                            self.volume_slide(i, arg, hw);
                        }
                        hw.per(i, self.ch[i].period);
                        hw.wait(80);
                    }
                    5 => self.volume_slide(i, arg, hw),
                    7 => self.tremolo(i, arg, hw),
                    0xa => self.volume_slide(i, arg, hw),
                    0xe => self.filter(hw, arg),
                    _ => {}
                }
            }
        }
    }

    /// The LED filter, through CIA-A's port A (bit 1 clear: filter on).
    fn filter(&mut self, hw: &mut Hw, arg: u8) {
        let x = hw.bus.read_byte(0xbfe001);
        hw.bus.write_byte(0xbfe001, if arg & 1 == 0 { x & !2 } else { x | 2 });
    }

    fn tremolo(&mut self, i: usize, arg: u8, hw: &mut Hw) {
        let c = &mut self.ch[i];
        let mut cmd = c.tremolo_cmd;
        if arg & 0x0f != 0 {
            cmd = (cmd & 0xf0) | (arg & 0x0f);
        }
        if arg & 0xf0 != 0 {
            cmd = (cmd & 0x0f) | (arg & 0xf0);
        }
        c.tremolo_cmd = cmd;
        let add = (SINE[((c.tremolo_pos as i32 / 4) & 0x1f) as usize] as u32 * (cmd & 0x0f) as u32 / 64) as i16;
        let vol = c.vol as i16;
        let vol = if c.tremolo_pos < 0 { (vol - add).max(0) } else { (vol + add).min(64) };
        c.tremolo_pos = c.tremolo_pos.wrapping_add(((cmd / 4) & 0x3c) as i8);
        hw.vol(i, vol as u16);
        hw.wait(80);
    }

    fn arpeggio(&mut self, i: usize, hw: &mut Hw) {
        // the originals' arpeggio only runs for the first six ticks of a row
        if self.counter >= 6 {
            return;
        }
        let c = &self.ch[i];
        let arp = match self.counter % 3 {
            1 => c.arg >> 4,
            2 => c.arg & 15,
            _ => 0,
        } as usize;
        // the originals search their table for the period itself, and read on past
        // its end for the note: zeros there for a few entries, then data of their own
        // (the port writes 0 for those, and for a period that is not in the table)
        let period = PERIODS.iter().position(|&p| p == c.period).map_or(0, |k| PERIODS.get(k + arp).copied().unwrap_or(0));
        hw.per(i, period);
        hw.wait(80);
    }

    fn pitch_bend(&mut self, i: usize, hw: &mut Hw) {
        let c = &mut self.ch[i];
        if c.arg & 0xf0 == 0 {
            c.bend = c.bend.saturating_sub((c.arg & 15) as u16).max(MIN_PERIOD);
        } else {
            c.bend = (c.bend + (c.arg >> 4) as u16).min(MAX_PERIOD);
        }
        hw.per(i, c.bend);
        hw.wait(80);
    }

    fn porta(&mut self, i: usize, up: bool, hw: &mut Hw) {
        let c = &mut self.ch[i];
        if up {
            c.period = c.period.saturating_sub((c.arg & self.low_mask) as u16).max(MIN_PERIOD);
        } else {
            c.period = (c.period + (c.arg & self.low_mask) as u16).min(MAX_PERIOD);
        }
        self.low_mask = 0xff;
        hw.per(i, c.period);
        hw.wait(80);
    }

    fn volume_slide(&mut self, i: usize, arg: u8, hw: &mut Hw) {
        let c = &mut self.ch[i];
        let spd = arg >> 4;
        if spd != 0 {
            c.vol = (c.vol + spd).min(64);
        } else {
            c.vol = c.vol.saturating_sub(arg & 15);
        }
        hw.vol(i, c.vol as u16);
        hw.wait(80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A module of two 64-byte samples (the second looped from byte 16, 16 words long), one
    /// position, and a pattern with `cells` at (row, voice).
    fn module(tempo: u8, cells: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
        let mut d = vec![0u8; 600];
        d[..8].copy_from_slice(b"test mod");
        for i in 0..15 {
            let o = 20 + i * 30;
            d[o..o + 5].copy_from_slice(format!("st-{:02}", i + 1).as_bytes());
            d[o + 28..o + 30].copy_from_slice(&[0, 1]);
        }
        for (i, (vol, rep, replen)) in [(48u8, 0u16, 1u16), (40, 16, 16)].into_iter().enumerate() {
            let o = 20 + i * 30;
            d[o + 22..o + 24].copy_from_slice(&32u16.to_be_bytes());
            d[o + 25] = vol;
            d[o + 26..o + 28].copy_from_slice(&rep.to_be_bytes());
            d[o + 28..o + 30].copy_from_slice(&replen.to_be_bytes());
        }
        d[470] = 1;
        d[471] = tempo;
        let mut pat = vec![0u8; 1024];
        for &(row, voice, c) in cells {
            let o = row * 16 + voice * 4;
            pat[o..o + 4].copy_from_slice(&c);
        }
        d.extend(pat);
        d.extend((0..128).map(|i| i as u8));
        d
    }

    /// Period `p`, sample `s`, effect `e`, parameter `a`.
    fn cell(p: u16, s: u8, e: u8, a: u8) -> [u8; 4] {
        [(s & 0xf0) | (p >> 8) as u8, p as u8, (s << 4) | e, a]
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.mod"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, log.iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn identifies_the_variants() {
        let ust = module(0x78, &[(0, 0, cell(0x1ac, 1, 1, 0x37))]);
        assert_eq!(variant(&ust), Some(Var::Ust10));
        // effect C is not an Ultimate SoundTracker effect
        let doc = module(0x78, &[(0, 0, cell(0x1ac, 1, 0xc, 0x20))]);
        assert_eq!(variant(&doc), Some(Var::StII));
        let st2 = module(0x78, &[(0, 0, cell(0x1ac, 1, 0xb, 0))]);
        assert_eq!(variant(&st2), Some(Var::St2x));
        // a tag of four printable characters makes it a 31-instrument module
        let mut mk = module(0x78, &[]);
        mk.resize(1100, 0);
        mk[1080..1084].copy_from_slice(b"M.K.");
        assert_eq!(variant(&mk), None);
        // too short, a position past the 64 patterns, song length 0, a sample name with a
        // control character
        assert_eq!(variant(&ust[..1200]), None);
        let mut bad = ust.clone();
        bad[472] = 64;
        assert_eq!(variant(&bad), None);
        bad = ust.clone();
        bad[470] = 0;
        assert_eq!(variant(&bad), None);
        bad = ust.clone();
        bad[20 + 3] = 7;
        assert_eq!(variant(&bad), None);
    }

    #[test]
    fn ultimate_soundtracker_plays_a_note_and_arpeggio() {
        let d = module(0x78, &[(0, 0, cell(0x1ac, 1, 0, 0)), (0, 1, cell(0x17d, 2, 0, 0)), (1, 0, cell(0, 0, 1, 0x37))]);
        let (format, log) = play(d, 40);
        assert_eq!(format, "Ultimate SoundTracker 1.0");
        // the player starts with every volume at 0
        assert_eq!(&reg(&log, 0xDFF0A8)[..1], &[0]);
        // DMA off for each note, then on for both together
        assert_eq!(&reg(&log, 0xDFF096)[..3], &[0x0001, 0x0002, 0x8003]);
        // voice 0 plays its sample once and then one word; voice 1 only its loop
        assert_eq!(&reg(&log, 0xDFF0A4)[..2], &[32, 1]);
        assert_eq!(&reg(&log, 0xDFF0B4)[..1], &[16]);
        assert_eq!(reg(&log, 0xDFF0B4).len(), 1);
        assert_eq!(&reg(&log, 0xDFF0A6)[..1], &[0x1ac]);
        // the arpeggio of the next row: +3 and +7 semitones from 428, then the note again
        assert_eq!(&reg(&log, 0xDFF0A6)[1..5], &[0x168, 0x11d, 0x1ac, 0x168]);
    }

    #[test]
    fn doc_soundtracker_writes_volume_and_modulation() {
        let d = module(0x78, &[(0, 0, cell(0x1ac, 1, 0xc, 0x20)), (0, 1, cell(0x17d, 2, 4, 0x24))]);
        let (format, log) = play(d, 30);
        assert_eq!(format, "SoundTracker II");
        // set volume writes its parameter in both bytes, unchecked
        assert!(reg(&log, 0xDFF0A8).contains(&0x2020));
        // each voice's modulation is cleared; voice 1's period modulation is set (effect 4)
        let adk = reg(&log, 0xDFF09E);
        assert_eq!(&adk[..3], &[0x11, 0x22, 0x8020]);
        // the arpeggio of effect 4 with 2 and 4 semitones, on voice 1
        let per = reg(&log, 0xDFF0B6);
        assert_eq!(&per[..3], &[0x17d, 0x153, 0x12e]);
    }
    #[test]
    fn the_web_path_claims_it_by_variant() {
        let ust = module(0x78, &[(0, 0, cell(0x1ac, 1, 1, 0x37))]);
        assert_eq!(crate::open::detect(&ust).as_deref(), Some("Ultimate SoundTracker 1.0"));
        let mut mk = module(0x78, &[]);
        mk.resize(1100, 0);
        mk[1080..1084].copy_from_slice(b"M.K.");
        assert_ne!(crate::open::detect(&mk).as_deref(), Some("SoundTracker"));
    }

    #[test]
    fn speed_zero_stops_the_song() {
        // F00 on the first row: no second row ever comes
        let d = module(0x78, &[(0, 0, cell(0x1ac, 1, 0xf, 0)), (3, 1, cell(0x17d, 2, 0, 0))]);
        let (format, log) = play(d, 120);
        assert_eq!(format, "SoundTracker VI");
        assert_eq!(reg(&log, 0xDFF0B6).len(), 0);
        assert_eq!(reg(&log, 0xDFF0A6), vec![0x1ac]);
    }

    #[test]
    fn a_doc_loop_of_zero_words_is_a_loop() {
        // sample 1 with a loop length of 0 words: the D.O.C. players take it for a loop,
        // so nothing is written after the note to stop it after one pass
        let mut d = module(0x78, &[(0, 0, cell(0x1ac, 1, 0xc, 0x20))]);
        d[20 + 28..20 + 30].copy_from_slice(&[0, 0]);
        let (_, log) = play(d, 30);
        assert_eq!(reg(&log, 0xDFF0A4), vec![32]);
    }
}
