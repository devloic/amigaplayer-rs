//! AMOS Music Bank, ported from NostalgicPlayer's AMOS Music Bank player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against DeliTracker's own AmosMusicBank DeliPlayer (PlayerStore,
//! "AmosMusicBank V1.5 player module V0.8", Francois Lionet's replayer adapted
//! by Delirium), which plays the bank with AMOS's own replayer, as NostalgicPlayer
//! does. (UADE's "Amos Music Bank" Eagleplayer is no reference for this: it
//! converts the bank to a ProTracker module and plays that.) Like the original
//! this plays from the bank in guest memory: the sample table, the songs'
//! position lists and the tracks are read where they lie, through the pointers
//! the original keeps, nothing range-checked.
//!
//! Where NostalgicPlayer differs from the original, this follows the original:
//! * a note writes its sample's start and its length (the sample table's words,
//!   not doubled, nor cut to the loop) and its period, and no volume; the notes
//!   of a step turn their DMA off at its end, the next tick turns it on, and the
//!   tick after writes the loop (its start and length from the voice's
//!   instrument then; a newer note on the voice cancels it); a voice without an
//!   instrument reads its sample's entry at address 0;
//! * set instrument takes any number, past the sample table too;
//! * the arpeggio command sets only the low byte of the effect value (its
//!   counter, the high byte, goes on from what was there); the vibrato's table
//!   position is its unsigned byte shifted (NostalgicPlayer divides the signed
//!   one); a slide past 856 stops there, one that reaches 856 goes on;
//! * the position jump goes on reading the new track in the same step; a
//!   pattern number up to the pattern count (one past the last) is taken, and a
//!   track offset of 0 stops the voice, as the list's end (-1) does; the list's
//!   loop (-2) is no song end: the song ends when no voice counts any more, and
//!   the playing stops (all DMA off, each voice's length 2 and volume 0);
//! * a bank is recognised as the original's check does: "AmBk" with the chip
//!   flag ($80) in the length's top byte, or AMOS's memory form ($C0 and a 24-bit
//!   length), the length within the file, and "Music   ".
//! Subsongs are numbered from 1, as the original's. Not as the original: the
//! commands past $1F, for which it jumps past its table into its own code, do
//! nothing (none is played in the corpus); a track with no delay, or a list of
//! loops only, on which it hangs, stops its voice.
//!
//! The writes come at the original's times: each code path counts the cycles
//! of the original's instructions on it (as the host's 68000 counts them, a
//! fixed number an instruction), from the start of the call, two a colour clock;
//! the host's times around them are measured.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "AMOS Music Bank", detect, new };

/// The arpeggio's periods. The original's table is followed by two zero words
/// and the end of its hunk; an arpeggio that goes more than two steps past its
/// last period (113) reads on past it, into what the host loaded next (in
/// amigaplayer-rs the next DeliPlayer's hunk header and code), where this reads
/// zeros, as NostalgicPlayer.
const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269,
    254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// The vibrato's sine (half a period).
const SINUS: [u8; 32] = [
    0x00, 0x18, 0x31, 0x4a, 0x61, 0x78, 0x8d, 0xa1, 0xb4, 0xc5, 0xd4, 0xe0, 0xeb, 0xf4, 0xfa, 0xfd, 0xff, 0xfd,
    0xfa, 0xf4, 0xeb, 0xe0, 0xd4, 0xc5, 0xb4, 0xa1, 0x8d, 0x78, 0x61, 0x4a, 0x31, 0x18,
];

/// The original's tempo base (PAL).
const TEMPO_BASE: u16 = 100;

/// Colour clocks of the host's (measured on the reference's write logs): from
/// the replayer's start to InitSound's call (with the original's check and
/// InitPlayer, for a bank with "AmBk"); from InitSound's last write to the end
/// of the start, where the output and the timer begin (InitSound's own tail is
/// 1237); from EndSound's return to InitSound's call at a subsong change.
const START_TIME: u64 = 759;
const INIT_END: u64 = 1217;
const END_TO_INIT: u64 = 232;

#[derive(Clone, Copy, PartialEq)]
enum Effect {
    None,
    Slide,
    Arpeggio,
    Portamento,
    Vibrato,
    VolumeSlide,
}

#[derive(Clone, Copy)]
struct Voice {
    /// The track position, the repeat's start (0: none), the instrument's entry
    /// in the sample table (0: none), the position list's start and position.
    track: u32,
    deb: u32,
    inst: u32,
    pat_start: u32,
    pat: u32,
    /// The step counter (the low byte of the original's word).
    cpt: u8,
    rep: u16,
    note: u16,
    vol: u16,
    effect: Effect,
    value: u16,
    pto: u16,
    ptone: u8,
    vib: u8,
}

pub struct AmosMusicBank {
    /// The guest addresses of the sample table, the song table and the track
    /// table (each at its count word).
    samples: u32,
    songs: u32,
    tracks: u32,
    /// The player's empty track (one "end of pattern" word).
    empty: u32,
    nsongs: u16,
    sub: u16,
    playing: bool,
    mucpt: u16,
    tempo: u16,
    /// Voices whose DMA goes on at the next tick, and whose loop goes out then.
    dma_new: u16,
    dma_loop: u16,
    voices: [Voice; 4],
    songend: bool,
    epoch: u64,
    t: u64,
}

/// The bank's data (after its "Music   "), as the original's check finds it.
fn data_start(d: &[u8]) -> Option<usize> {
    let l0 = super::be32(d, 0);
    let (len, at) = if d.first() == Some(&0xC0) {
        (l0 & 0xFF_FFFF, 8)
    } else if d.starts_with(b"AmBk") && d.get(8) == Some(&0x80) {
        (super::be32(d, 8) & 0xFF_FFFF, 12)
    } else {
        return None;
    };
    (len as usize <= d.len() && d.get(at..at + 8) == Some(b"Music   ")).then_some(at + 8)
}

fn detect(d: &[u8]) -> bool {
    data_start(d).is_some_and(|o| o + 12 <= d.len())
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let o = data_start(d)?;
    let a0 = base + o as u32;
    let bus = &mut m.bus;
    let samples = a0.wrapping_add(bus.r32(a0));
    let songs = a0.wrapping_add(bus.r32(a0 + 4));
    let tracks = a0.wrapping_add(bus.r32(a0 + 8));
    let nsongs = bus.r16(songs);
    if nsongs == 0 {
        return None;
    }
    let empty = m.alloc(2);
    if empty == 0 {
        return None;
    }
    m.bus.w16(empty, 0x8000);
    let v = Voice {
        track: 0,
        deb: 0,
        inst: 0,
        pat_start: 0,
        pat: 0,
        cpt: 0,
        rep: 0,
        note: 0,
        vol: 0,
        effect: Effect::None,
        value: 0,
        pto: 0,
        ptone: 0,
        vib: 0,
    };
    let mut p = AmosMusicBank {
        samples,
        songs,
        tracks,
        empty,
        nsongs,
        sub: 1,
        playing: false,
        mucpt: 0,
        tempo: 0,
        dma_new: 0,
        dma_loop: 0,
        voices: [v; 4],
        songend: false,
        epoch: 0,
        t: 0,
    };
    let mut hw = Hw { bus: &mut m.bus };
    // the host's check and InitPlayer read the memory form's header in 112
    // cycles less each
    hw.wait(START_TIME - if d[0] == 0xC0 { 112 } else { 0 });
    p.init_sound(1, &mut hw);
    hw.wait(INIT_END);
    Some(Box::new(p))
}

impl Replayer for AmosMusicBank {
    fn format(&self) -> &'static str {
        "AMOS Music Bank"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.nsongs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // EndSound ($226): the volumes 0 and the DMA off
        self.begin(hw);
        self.c(16);
        for ch in 0..4 {
            self.vol(hw, 12, ch, 0);
        }
        self.dma(hw, 16, 0xf);
        self.c(16);
        self.at(hw, 0);
        hw.wait(END_TO_INIT);
        self.init_sound(n, hw);
        hw.wait(INIT_END);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.begin(hw);
        // the call ($136): movem, bsr; the player ($35C)
        self.c(96 + 18 + 35);
        if !self.playing {
            return;
        }
        self.c(22);
        self.dma_irq(hw);
        // the tempo counter
        self.c(63);
        self.mucpt = self.mucpt.wrapping_add(self.tempo);
        if self.mucpt < TEMPO_BASE {
            self.c(18);
            self.effects(hw);
            return;
        }
        self.mucpt -= TEMPO_BASE;
        self.c(28);
        let mut counting = 0;
        let mut dma = 0u16;
        for i in 0..4 {
            self.c(if i == 0 { 21 } else { 29 });
            if self.voices[i].cpt != 0 {
                counting += 1;
                self.c(29);
                self.voices[i].cpt -= 1;
                if self.voices[i].cpt == 0 {
                    self.c(38);
                    self.mu_step(i, &mut dma, hw);
                }
            }
        }
        self.dma(hw, 28, dma & 0xf);
        self.c(13);
        if counting == 0 {
            // the song is over: the playing stops ($410)
            self.playing = false;
            self.songend = true;
            self.c(42);
            self.stop_dma(hw);
        }
    }
}

impl AmosMusicBank {
    fn begin(&mut self, hw: &Hw) {
        self.epoch = hw.bus.clock;
        self.t = 0;
    }
    fn c(&mut self, n: u64) {
        self.t += n;
    }
    /// The instructions of `n` cycles to the end of a write run.
    fn at(&mut self, hw: &mut Hw, n: u64) {
        self.t += n;
        let at = self.epoch + self.t / 2;
        if at > hw.bus.clock {
            hw.wait(at - hw.bus.clock);
        }
    }
    fn dma(&mut self, hw: &mut Hw, n: u64, v: u16) {
        self.at(hw, n);
        hw.dmacon(v);
    }
    fn loc(&mut self, hw: &mut Hw, n: u64, ch: usize, a: u32) {
        self.at(hw, n);
        hw.loc(ch, a);
    }
    fn len(&mut self, hw: &mut Hw, n: u64, ch: usize, l: u16) {
        self.at(hw, n);
        hw.len(ch, l);
    }
    fn per(&mut self, hw: &mut Hw, n: u64, ch: usize, p: u16) {
        self.at(hw, n);
        hw.per(ch, p);
    }
    fn vol(&mut self, hw: &mut Hw, n: u64, ch: usize, v: u16) {
        self.at(hw, n);
        hw.vol(ch, v);
    }

    /// InitSound ($21A) for subsong `n` (from 1).
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.begin(hw);
        self.c(4 + 12 + 18 + 72 + 136);
        self.stop_dma(hw);
        let bus = &*hw.bus;
        let song = self.songs.wrapping_add(bus.r32(self.songs.wrapping_add(4 * n as u32).wrapping_sub(2)));
        self.mucpt = TEMPO_BASE;
        self.tempo = 0x11;
        self.dma_new = 0;
        self.dma_loop = 0;
        for (i, v) in self.voices.iter_mut().enumerate() {
            let list = song.wrapping_add(bus.r16(song + 2 * i as u32) as u32);
            *v = Voice {
                track: self.empty,
                deb: 0,
                inst: 0,
                pat_start: list,
                pat: list,
                cpt: 1,
                rep: 0,
                note: 0,
                vol: 0,
                effect: Effect::None,
                value: 0,
                pto: 0,
                ptone: 0,
                vib: 0,
            };
        }
        self.playing = true;
        self.songend = false;
    }

    /// $330: all DMA off, each voice's length 2 and volume 0.
    fn stop_dma(&mut self, hw: &mut Hw) {
        self.dma(hw, 12 + 12 + 9 + 12, 0xf);
        self.c(4);
        for ch in 0..4 {
            self.c(15);
            self.len(hw, 16, ch, 2);
            self.vol(hw, 16, ch, 0);
            self.c(18);
        }
        self.c(16);
    }

    /// $838: the loops of the notes of two ticks ago, and the DMA on for those
    /// of the last tick.
    fn dma_irq(&mut self, hw: &mut Hw) {
        let d0 = self.dma_loop;
        self.c(23);
        if d0 != 0 {
            self.c(16);
            for i in 0..4 {
                self.c(19);
                if d0 & 1 << i != 0 {
                    let inst = self.voices[i].inst;
                    let a = self.samples.wrapping_add(hw.bus.r32(inst.wrapping_add(4)));
                    let l = hw.bus.r16(inst.wrapping_add(10));
                    self.loc(hw, 66, i, a);
                    self.len(hw, 20, i, l);
                }
            }
        }
        let d1 = self.dma_new;
        self.dma_loop = d1;
        self.dma_new = 0;
        self.dma(hw, 68 + 16, 0x8000 | ((d0 | d1) & 0xf));
        self.c(16);
    }

    /// $6A6: every voice's effect and volume.
    fn effects(&mut self, hw: &mut Hw) {
        self.c(4);
        for i in 0..4 {
            self.c(if i == 0 { 48 } else { 56 });
            self.effect(i, hw);
            let v = self.voices[i].vol;
            // the volume times the host's (64) / 64
            self.vol(hw, 104, i, ((v as u32 * 64) >> 6) as u16);
        }
        self.c(16);
    }

    /// $744: back to no effect (with its return).
    fn no_effect(&mut self, i: usize) {
        self.voices[i].effect = Effect::None;
        self.c(40);
    }

    fn effect(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        match v.effect {
            Effect::None => {
                self.per(hw, 20, i, v.note);
                self.c(16);
            }
            Effect::Slide => {
                self.c(21);
                if v.value == 0 {
                    self.no_effect(i);
                    return;
                }
                let mut d0 = v.value.wrapping_add(v.note);
                self.c(29);
                if d0 < 0x71 {
                    d0 = 0x71;
                    self.c(22);
                    self.no_effect(i);
                }
                self.c(17);
                if d0 > 0x358 {
                    d0 = 0x358;
                    self.c(26);
                    self.no_effect(i);
                }
                self.voices[i].note = d0;
                self.per(hw, 24, i, d0);
                self.c(16);
            }
            Effect::Arpeggio => {
                let mut d0 = v.value & 0xff;
                let mut d1 = (v.value >> 8) as u8;
                self.c(45);
                if d1 >= 3 {
                    d1 = 2;
                    self.c(4);
                }
                d1 = d1.wrapping_sub(1);
                self.voices[i].value = (d1 as u16) << 8 | (v.value & 0xff);
                self.c(25);
                if d1 == 0 {
                    self.per(hw, 22 + 12, i, v.note);
                    self.c(16);
                    return;
                }
                self.c(9);
                if d1 & 0x80 != 0 {
                    d0 >>= 4;
                    self.c(24);
                } else {
                    d0 &= 0xf;
                    self.c(18);
                }
                self.c(32);
                for k in 0..37usize {
                    self.c(31);
                    let t = |x: usize| PERIODS.get(x).copied().unwrap_or(0);
                    if v.note as i16 >= t(k) as i16 {
                        self.per(hw, 12, i, t(k + d0 as usize));
                        self.c(16);
                        return;
                    }
                    self.c(18);
                }
                self.c(16);
            }
            Effect::Portamento => {
                let mut d1 = v.note;
                self.c(45);
                if d1 == v.pto {
                    self.c(58);
                    self.voices[i].effect = Effect::None;
                } else {
                    self.c(9 + 25);
                    let reached = if d1 < v.pto {
                        d1 = d1.wrapping_add(v.value);
                        d1 >= v.pto
                    } else {
                        d1 = d1.wrapping_sub(v.value);
                        d1 <= v.pto
                    };
                    if reached {
                        d1 = v.pto;
                        self.c(if v.note < v.pto { 12 } else { 22 } + 58);
                        self.voices[i].effect = Effect::None;
                    }
                }
                self.voices[i].note = d1;
                self.per(hw, 24, i, d1);
                self.c(16);
            }
            Effect::Vibrato => {
                let index = ((v.vib >> 2) & 0x1f) as usize;
                let d2 = (SINUS[index] as u16 * (v.value & 0xf)) >> 6;
                self.c(181);
                let p = if v.vib & 0x80 == 0 {
                    self.c(14);
                    v.note.wrapping_add(d2)
                } else {
                    self.c(4);
                    v.note.wrapping_sub(d2)
                };
                self.per(hw, 12, i, p);
                self.voices[i].vib = v.vib.wrapping_add(((v.value & 0xff) >> 2) as u8 & 0x3c);
                self.c(46 + 16);
            }
            Effect::VolumeSlide => {
                let mut d0 = v.vol.wrapping_add(v.value);
                self.c(33);
                if d0 & 0x8000 != 0 {
                    d0 = 0;
                    self.c(4);
                }
                self.c(17);
                if d0 >= 0x40 {
                    d0 = 0x3f;
                    self.c(4);
                }
                self.voices[i].vol = d0;
                self.c(28);
            }
        }
    }

    /// $424: one step of voice `i`'s track, to its delay; `dma` collects the
    /// voices whose DMA goes off.
    fn mu_step(&mut self, i: usize, dma: &mut u16, hw: &mut Hw) {
        self.c(24);
        let mut a2 = self.voices[i].track;
        // the original loops for ever on a track with no delay; this stops the
        // voice after so many words
        for _ in 0..0x10000 {
            let d0 = hw.bus.r16(a2);
            a2 = a2.wrapping_add(2);
            self.c(17);
            if d0 & 0x8000 != 0 {
                let arg = d0 & 0xff;
                self.c(44 + 10);
                match (d0 >> 8) & 0x7f {
                    0 => match self.fin_pattern(i, hw) {
                        Some(a) => a2 = a,
                        None => return,
                    },
                    3 => {
                        self.voices[i].vol = arg.min(0x3f);
                        self.c(47 + if arg >= 0x40 { 4 } else { 0 });
                    }
                    4 => {
                        self.voices[i].effect = Effect::None;
                        self.c(34);
                    }
                    5 => {
                        self.c(17);
                        if arg == 0 {
                            self.voices[i].deb = a2;
                            self.c(26);
                        } else if {
                            self.c(21);
                            self.voices[i].rep == 0
                        } {
                            self.voices[i].rep = arg;
                            self.c(22);
                        } else {
                            self.voices[i].rep -= 1;
                            self.c(27);
                            if self.voices[i].rep != 0 {
                                self.c(27);
                                if self.voices[i].deb != 0 {
                                    a2 = self.voices[i].deb;
                                    self.c(14);
                                }
                            }
                        }
                    }
                    6 | 7 => {
                        // the filter commands: the Eagleplayer, which converts the bank
                        // to ProTracker's effects, has none for them (and no E0x), so
                        // the LED bit stays as it is
                        self.at(hw, 24);
                        self.c(10);
                    }
                    8 => {
                        self.tempo = arg;
                        self.c(30);
                    }
                    9 => {
                        let e = self.samples.wrapping_add(2 + 32 * arg as u32);
                        let vol = hw.bus.r16(e + 12);
                        self.voices[i].inst = e;
                        self.voices[i].vol = if vol >= 0x40 { 0x3f } else { vol };
                        self.c(97 + 22 + if vol >= 0x40 { 4 } else { 0 });
                    }
                    0xa => {
                        // only the value's low byte: the high one is the counter
                        self.voices[i].value = (self.voices[i].value & 0xff00) | arg;
                        self.voices[i].effect = Effect::Arpeggio;
                        self.c(46);
                    }
                    0xb => {
                        self.voices[i].ptone = 1;
                        self.voices[i].value = arg;
                        self.voices[i].effect = Effect::Portamento;
                        self.c(34 + 46);
                    }
                    0xc => {
                        self.voices[i].value = arg;
                        self.voices[i].effect = Effect::Vibrato;
                        self.c(18 + 46);
                    }
                    0xd => {
                        let mut x = arg >> 4;
                        self.c(39);
                        if x == 0 {
                            x = (arg & 0xf).wrapping_neg();
                            self.c(16);
                        }
                        self.voices[i].value = x;
                        self.voices[i].effect = Effect::VolumeSlide;
                        self.c(46);
                    }
                    0xe => {
                        self.voices[i].value = arg.wrapping_neg();
                        self.voices[i].effect = Effect::Slide;
                        self.c(30 + 38);
                    }
                    0xf => {
                        self.voices[i].value = arg;
                        self.voices[i].effect = Effect::Slide;
                        self.c(8 + 46);
                    }
                    0x10 => {
                        self.voices[i].cpt = arg as u8;
                        self.voices[i].track = a2;
                        self.c(44);
                        return;
                    }
                    0x11 => {
                        self.voices[i].pat = self.voices[i].pat_start.wrapping_add(2 * arg as u32);
                        self.c(66);
                        match self.fin_pattern(i, hw) {
                            Some(a) => a2 = a,
                            None => return,
                        }
                    }
                    // 1, 2 (the old slides) and $12-$1F do nothing; past $1F the
                    // original jumps past its table, into its own code
                    _ => {}
                }
                continue;
            }
            self.c(19);
            let inst = self.voices[i].inst;
            let start = self.samples.wrapping_add(hw.bus.r32(inst));
            let len = hw.bus.r16(inst.wrapping_add(8));
            if d0 & 0x4000 == 0 {
                // a note
                let p = d0 & 0xfff;
                self.loc(hw, 66, i, start);
                self.len(hw, 20, i, len);
                *dma |= 1 << i;
                self.dma_loop &= !(1 << i);
                self.dma_new |= 1 << i;
                self.c(61);
                let v = &mut self.voices[i];
                if v.ptone == 0 {
                    v.note = p;
                    self.per(hw, 24, i, p);
                    self.c(10);
                } else {
                    v.ptone = 0;
                    v.pto = p;
                    v.effect = Effect::Portamento;
                    self.c(62);
                }
                continue;
            }
            // a note of the first version: its delay, then its period
            self.voices[i].cpt = d0 as u8;
            let w = hw.bus.r16(a2);
            a2 = a2.wrapping_add(2);
            self.c(29);
            if w != 0 {
                let p = w & 0xfff;
                self.voices[i].note = p;
                self.per(hw, 32, i, p);
                self.loc(hw, 58, i, start);
                self.len(hw, 20, i, len);
                *dma |= 1 << i;
                self.dma_loop &= !(1 << i);
                self.dma_new |= 1 << i;
                self.c(40);
            }
            self.voices[i].track = a2;
            self.c(32);
            return;
        }
        self.voices[i].cpt = 0;
    }

    /// $53A: the end of a pattern: the next track of the position list, or
    /// None when the voice stops (the list's end, or a pattern it cannot find).
    fn fin_pattern(&mut self, i: usize, hw: &mut Hw) -> Option<u32> {
        let v = &mut self.voices[i];
        v.cpt = 0;
        v.rep = 0;
        v.deb = 0;
        v.effect = Effect::None;
        self.c(96);
        let mut a0 = self.voices[i].pat;
        // a list of loops only would hang the original
        for _ in 0..0x10000 {
            let d0 = hw.bus.r16(a0);
            a0 = a0.wrapping_add(2);
            self.c(21);
            if d0 & 0x8000 != 0 {
                self.c(17);
                if d0 == 0xffff {
                    self.c(16);
                    return None;
                }
                a0 = self.voices[i].pat_start;
                self.c(26);
                continue;
            }
            self.voices[i].pat = a0;
            self.c(49);
            if d0 > hw.bus.r16(self.tracks) {
                self.c(16);
                return None;
            }
            let idx = ((d0 << 2).wrapping_add(i as u16) << 1) as i16 as i32;
            let off = hw.bus.r16(self.tracks.wrapping_add(2).wrapping_add(idx as u32));
            self.c(45);
            if off == 0 {
                self.c(16);
                return None;
            }
            self.c(22);
            return Some(self.tracks.wrapping_add(off as u32));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// A bank with one sample (a 32-word attack, an 8-word loop), one song whose
    /// voice 0 plays one pattern (set instrument, a note, a delay of 2, the end),
    /// the others an empty pattern list, looped.
    fn module() -> Vec<u8> {
        let mut b = Vec::new(); // the data after "Music   "
        b.extend([0u8; 12]); // the three offsets, set below
        // the sample table
        let si = b.len();
        b.extend([0, 1]);
        b.extend(0x24u32.to_be_bytes()); // start (from the table)
        b.extend(0x44u32.to_be_bytes()); // loop
        b.extend([0, 32, 0, 8, 0, 40, 0, 32]); // lengths, volume 40
        b.extend(*b"SAMPLE          ");
        assert_eq!(b.len() - si, 0x22);
        b.extend([0, 0]);
        b.extend((0..64).map(|i| (i * 4) as u8));
        // the song table: one song
        let so = b.len();
        b.extend([0, 1]);
        b.extend(6u32.to_be_bytes());
        let song = b.len();
        for v in 0..4u16 {
            b.extend((28 + 4 * v).to_be_bytes());
        }
        b.extend([0, 17, 0, 0]);
        b.extend(*b"SONG            ");
        for v in 0..4 {
            b.extend(if v == 0 { [0u8, 0, 0xff, 0xfe] } else { [0xff, 0xff, 0, 0] });
        }
        assert_eq!(b.len() - song, 44);
        // the tracks: one pattern, voice 0's track
        let tr = b.len();
        b.extend([0, 1]);
        b.extend([0, 10, 0, 0, 0, 0, 0, 0]);
        b.extend([0x89, 0x00, 0x30, 0xd6, 0x90, 0x02, 0x80, 0x00]);
        for (k, o) in [si, so, tr].into_iter().enumerate() {
            b[4 * k..4 * k + 4].copy_from_slice(&(o as u32).to_be_bytes());
        }
        let mut d = b"AmBk".to_vec();
        d.extend([0, 3, 0, 0]);
        d.extend((0x8000_0000u32 | (b.len() + 8) as u32).to_be_bytes());
        d.extend(*b"Music   ");
        d.extend(b);
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.abk"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "AMOS Music Bank");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound's DMA off; the first tick's DMA on (none) and the note's DMA
        // off; its DMA on; its loop's DMA on
        assert_eq!(&reg(0xDFF096)[..5], &[0x000f, 0x8000, 0x0001, 0x8001, 0x8001]);
        // voice 0: InitSound's length, the note's, then its loop's
        assert_eq!(&reg(0xDFF0A4)[..3], &[2, 32, 8]);
        assert_eq!(reg(0xDFF0A6)[0], 0xd6);
        // InitSound's volume 0, then the instrument's on the effect ticks
        assert_eq!(&reg(0xDFF0A8)[..2], &[0, 40]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
