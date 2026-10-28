//! InStereo! 2.0, ported from NostalgicPlayer's InStereo! 2.0 player
//! (Copyright (c) 2024 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the Eagleplayer "In Stereo 2.0" ("InStereo 2.0 Amplifier
//! module V2.11", Branko Mikic & Carsten Herbst, adapted by Eagleeye of
//! DEFECT), which plays the "IS20DF10" modules with a replayer of its own
//! through ENPP_Poke* and ENPP_DMAMask. Like the original, the port
//! plays from the module in guest memory, and keeps the replayer's variables,
//! tables and voices in guest memory in the layout of its code hunk (from
//! `$12D8` on), so that what it reads past a table, and the voice state it
//! leaves from one InitSound to the next, are the original's.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * a row turns off the DMA of each voice that gets a note and an instrument
//!   (one DMAMask each), writes the notes' locations, lengths and sample
//!   volumes, then DMA on for those that got one (DMAMask is called even with
//!   none); a sample's loop is written at the start of the next tick, from
//!   location 0 (the original passes a register it never set) or, for a repeat
//!   of 1 word, from an empty buffer of its own;
//! * a note `$7F` (or a sample with no data) writes volume 0 straight to Paula;
//! * clearing a voice's instrument when its DMA goes off clears it for the first
//!   voice only: for the others the original clears bytes of other voices
//!   (the third voice's envelope direction among them), as here;
//! * the sample table starts from the last sample: sample number 0 has no data
//!   and is muted;
//! * a synth's portamento takes its speed from the instrument's 10th byte, and
//!   a restored synth note with effect 7 takes its volume from its 6th, as the
//!   original reads them;
//! * every voice writes its period every tick, the ADSR volume is
//!   `table * volume >> 10`, and the speed and tempo are as the original keeps
//!   them (the tempo sets the timer: `tempo * timer / 50`);
//! * the filter effect sets and clears the LED bit through ENPP_PokeCommand;
//! * byte and word arithmetic as in the original, nothing range-checked;
//! * the writes come at the original's times, counted on its code (`cy`).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "InStereo! 2.0", detect, new };

/// The first offset of the original's hunk that is mirrored.
const BASE: u32 = 0x12d8;
/// The row's effect byte, as a word.
const FXW: u32 = 0x12d8;
const VOICES: u32 = 0x12e0;
/// The sound effect's voice (-1: none, which is all this player has).
const SFX: u32 = 0x1374;
/// The four voices' position entries (start row, sound and note transpose).
const POSN: u32 = 0x1378;
/// The four voices' rows (note, instrument, effect, argument).
const TRK: u32 = 0x1388;
/// The four voices' new sample note flags (the loop is written next tick).
const NEWNOTE: u32 = 0x1398;
const VIBTAB: u32 = 0x139c;
const PERIODS_AT: u32 = 0x149c;
const VOICE_ON: u32 = 0x1578;
const CNT: u32 = 0x157c;
const ROW: u32 = 0x157d;
const SONGPOS: u32 = 0x157e;
/// The subsong's 10 bytes: speed, rows, first, last, restart and tempo.
const SPEED: u32 = 0x1580;
const ROWS: u32 = 0x1581;
const FIRST: u32 = 0x1582;
const LAST: u32 = 0x1584;
const RESTART: u32 = 0x1586;
const TEMPO: u32 = 0x1588;
// InitPlayer's pointers into the module.
const P_OVTB: u32 = 0x158a;
const P_NTBL: u32 = 0x158e;
const P_SYNT: u32 = 0x1592;
const P_SAMP: u32 = 0x1596;
const P_STBL: u32 = 0x159a;
/// The samples' addresses (from the last sample on), then their lengths.
const ADDRS: u32 = 0x159e;
const LENS: u32 = 0x169a;
/// The hunk's end.
const END: u32 = 0x1798;

// A voice's 36 bytes.
const V_NOTE: u32 = 0x0;
const V_PREV: u32 = 0x1;
const V_SLIDE_SPEED: u32 = 0x2;
const V_INSTR: u32 = 0x3;
const V_SYNTH: u32 = 0x4;
const V_VOLUME: u32 = 0x5;
const V_ADSR_POS: u32 = 0x6;
const V_VIB_POS: u32 = 0x8;
const V_LFO_POS: u32 = 0xa;
const V_PORT_COUNT: u32 = 0xc;
const V_PORT_SPEED: u32 = 0xe;
const V_SLIDE: u32 = 0x10;
const V_VIB_DELAY: u32 = 0x12;
const V_VIB_SPEED: u32 = 0x13;
const V_VIB_LEVEL: u32 = 0x14;
const V_EG_DIR: u32 = 0x15;
const V_EG_POS: u32 = 0x16;
const V_ARP_POS: u32 = 0x18;
const V_SUSTAIN: u32 = 0x1a;
const V_ARP_TOGGLE: u32 = 0x1c;

/// The vibrato table of the original's hunk.
const VIBRATO: [u8; 256] = [
    0x00, 0x03, 0x06, 0x09, 0x0c, 0x10, 0x13, 0x16, 0x19, 0x1c, 0x1f, 0x22, 0x25, 0x28, 0x2b, 0x2e,
    0x31, 0x34, 0x36, 0x39, 0x3c, 0x3f, 0x42, 0x44, 0x47, 0x49, 0x4c, 0x4e, 0x51, 0x53, 0x56, 0x58,
    0x5a, 0x5c, 0x5e, 0x60, 0x62, 0x64, 0x66, 0x68, 0x6a, 0x6c, 0x6d, 0x6f, 0x70, 0x72, 0x73, 0x74,
    0x76, 0x77, 0x78, 0x79, 0x7a, 0x7b, 0x7b, 0x7c, 0x7d, 0x7d, 0x7e, 0x7e, 0x7e, 0x7f, 0x7f, 0x7f,
    0x7f, 0x7f, 0x7f, 0x7f, 0x7e, 0x7e, 0x7d, 0x7d, 0x7c, 0x7c, 0x7b, 0x7a, 0x79, 0x78, 0x77, 0x76,
    0x75, 0x74, 0x72, 0x71, 0x70, 0x6e, 0x6c, 0x6b, 0x69, 0x67, 0x65, 0x63, 0x61, 0x5f, 0x5d, 0x5b,
    0x59, 0x57, 0x54, 0x52, 0x50, 0x4d, 0x4b, 0x48, 0x45, 0x43, 0x40, 0x3d, 0x3b, 0x38, 0x35, 0x32,
    0x2f, 0x2c, 0x29, 0x27, 0x24, 0x20, 0x1d, 0x1a, 0x17, 0x14, 0x11, 0x0e, 0x0b, 0x08, 0x05, 0x02,
    0xff, 0xfc, 0xf9, 0xf6, 0xf2, 0xef, 0xec, 0xe9, 0xe6, 0xe3, 0xe0, 0xdd, 0xda, 0xd7, 0xd4, 0xd1,
    0xce, 0xcb, 0xc9, 0xc6, 0xc3, 0xc0, 0xbe, 0xbb, 0xb8, 0xb6, 0xb3, 0xb1, 0xae, 0xac, 0xaa, 0xa8,
    0xa5, 0xa3, 0xa1, 0x9f, 0x9d, 0x9b, 0x99, 0x98, 0x96, 0x94, 0x93, 0x91, 0x90, 0x8e, 0x8d, 0x8c,
    0x8a, 0x89, 0x88, 0x87, 0x86, 0x86, 0x85, 0x84, 0x84, 0x83, 0x83, 0x82, 0x82, 0x82, 0x82, 0x82,
    0x82, 0x82, 0x82, 0x82, 0x82, 0x83, 0x83, 0x84, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b,
    0x8c, 0x8d, 0x8f, 0x90, 0x92, 0x93, 0x95, 0x97, 0x98, 0x9a, 0x9c, 0x9e, 0xa0, 0xa2, 0xa4, 0xa6,
    0xa9, 0xab, 0xad, 0xb0, 0xb2, 0xb5, 0xb7, 0xba, 0xbc, 0xbf, 0xc2, 0xc4, 0xc7, 0xca, 0xcd, 0xd0,
    0xd3, 0xd6, 0xd9, 0xdb, 0xde, 0xe2, 0xe5, 0xe8, 0xeb, 0xee, 0xf1, 0xf4, 0xf7, 0xfa, 0xfd, 0x00,
];

/// The period table (the original's, as InStereo! 1.0's, then `$FFFF`).
const PERIODS: [u16; 110] = [
    0, 13696, 12928, 12192, 11520, 10848, 10240, 9664, 9120, 8608, 8128, 7680, 7248, 6848, 6464, 6096,
    5760, 5424, 5120, 4832, 4560, 4304, 4064, 3840, 3624, 3424, 3232, 3048, 2880, 2712, 2560, 2416,
    2280, 2152, 2032, 1920, 1812, 1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960,
    906, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320,
    302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 107,
    101, 95, 90, 85, 80, 75, 71, 67, 63, 60, 56, 53, 50, 47, 45, 42, 40, 37, 35, 33, 31, 30, 28,
    0xffff,
];

/// DeliTracker's timer when InitPlayer keeps it (the tempo scales it).
const TIMER: u32 = crate::deli::DEFAULT_TIMER as u32;

/// Colour clocks from the open replayer's start to the reference's InitPlayer
/// past GetListData, and from InitPlayer's end to InitSound (the Eagleplayer's
/// Check, SubSongRange, module info and amplifier; measured).
const INIT_WAIT: u64 = 4645;

fn detect(d: &[u8]) -> bool {
    d.len() >= 12 && &d[..8] == b"IS20DF10" && &d[8..12] == b"STBL"
}

pub struct InStereo2 {
    base: u32,
    /// The original's hunk from `BASE` on, in guest memory.
    mem: u32,
    /// The data hunk's 8 bytes of zeros: a repeat of 1 word plays them.
    empty: u32,
    /// The positions' count less one (InitPlayer's module info).
    npos1: u16,
    timer: u16,
    sub: u16,
    last_sub: u16,
    songend: bool,
    /// The bus clock and the 68000 cycles counted since the last host call, as
    /// the emulator times guest code (`cy`).
    epoch: u64,
    cycles: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if d.len() < 0x20 {
        return None;
    }
    let size = END - BASE;
    let top = std::mem::replace(&mut m.alloc_top, true);
    let mem = m.alloc(size);
    let empty = m.alloc(8);
    m.alloc_top = top;
    if mem == 0 || empty == 0 {
        return None;
    }
    let mut r = InStereo2 {
        base,
        mem,
        empty,
        npos1: 0,
        timer: TIMER as u16,
        sub: 0,
        last_sub: (super::be32(d, 0xc) as u16).saturating_sub(1),
        songend: false,
        epoch: 0,
        cycles: 0,
    };
    let b = &mut m.bus;
    for k in 0..size {
        b.w8(mem + k, 0);
    }
    for k in 0..8 {
        b.w8(empty + k, 0);
    }
    // what the hunk holds: the tables, no sound effect, every voice on
    for (k, &x) in VIBRATO.iter().enumerate() {
        b.w8(r.a(VIBTAB) + k as u32, x);
    }
    for (k, &p) in PERIODS.iter().enumerate() {
        b.w16(r.a(PERIODS_AT) + 2 * k as u32, p);
    }
    b.w32(r.a(SFX), 0xffff_ffff);
    for v in 0..4 {
        b.w8(r.a(VOICE_ON + v), 1);
    }
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(INIT_WAIT);
    r.init_player(&mut hw)?;
    r.init_sound(0, &mut hw);
    Some(Box::new(r))
}

impl Replayer for InStereo2 {
    fn format(&self) -> &'static str {
        "InStereo! 2.0"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        let t = if self.timer == 0 { TIMER as u64 } else { self.timer as u64 };
        (t + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.last_sub, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.songend = false;
        self.init_sound(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.epoch = hw.bus.clock;
        self.cycles = 0;
        // the interrupt: registers saved, the UPS structure, then the loops of
        // last row's samples
        self.c(0x5ec, 0x614);
        self.loops(hw);
        self.c(0x614, 0x624);
        let cnt = self.r8(hw, CNT).wrapping_add(1);
        self.w8(hw, CNT, cnt);
        if cnt as i8 >= self.r8(hw, SPEED) as i8 {
            self.c(0x624, 0x62a);
            self.w8(hw, CNT, 0);
            self.row(hw);
        }
        self.c(0x62a, 0x62e);
        self.effects(hw);
        self.c(0x62e, 0x63c);
        self.sync(hw);
    }
}

impl InStereo2 {
    /// The guest address of hunk offset `h`.
    fn a(&self, h: u32) -> u32 {
        self.mem + h - BASE
    }

    fn r8(&self, hw: &Hw, h: u32) -> u8 {
        hw.bus.r8(self.a(h))
    }

    fn r16(&self, hw: &Hw, h: u32) -> u16 {
        hw.bus.r16(self.a(h))
    }

    fn r32(&self, hw: &Hw, h: u32) -> u32 {
        hw.bus.r32(self.a(h))
    }

    fn w8(&self, hw: &mut Hw, h: u32, v: u8) {
        hw.bus.w8(self.a(h), v);
    }

    fn w16(&self, hw: &mut Hw, h: u32, v: u16) {
        hw.bus.w16(self.a(h), v);
    }

    fn w32(&self, hw: &mut Hw, h: u32, v: u32) {
        hw.bus.w32(self.a(h), v);
    }

    /// The original's instructions from hunk offset `from` up to `to` ran once.
    fn c(&mut self, from: u16, to: u16) {
        self.cycles += cy(from, to);
    }

    /// `n` times.
    fn cn(&mut self, from: u16, to: u16, n: u32) {
        self.cycles += cy(from, to) * n as u64;
    }

    /// The clock reaches the instruction that writes (its cycles counted).
    fn sync(&self, hw: &mut Hw) {
        let t = self.epoch + self.cycles / 2;
        if t > hw.bus.clock {
            hw.bus.clock = t;
        }
    }

    /// A host call: the emulator counts the cycles again from its return.
    fn host(&mut self, hw: &mut Hw) {
        self.sync(hw);
        self.epoch = hw.bus.clock;
        self.cycles = 0;
        // the stub's RTS
        self.cycles += 16;
    }

    fn poke_adr(&mut self, hw: &mut Hw, v: u32, a: u32) {
        self.c(0x3f2, 0x408);
        self.sync(hw);
        hw.loc(v as usize, a);
        self.host(hw);
        self.c(0x408, 0x40e);
    }

    fn poke_len(&mut self, hw: &mut Hw, v: u32, n: u32) {
        self.c(0x40e, 0x42a);
        self.sync(hw);
        hw.len(v as usize, n as u16);
        self.host(hw);
        self.c(0x42a, 0x430);
    }

    fn poke_per(&mut self, hw: &mut Hw, v: u32, p: u32) {
        self.c(0x430, 0x44a);
        self.sync(hw);
        hw.per(v as usize, p as u16);
        self.host(hw);
        self.c(0x44a, 0x450);
    }

    /// The volume (the routine at $3A0): the UPS structure's copy, then Paula.
    fn poke_vol(&mut self, hw: &mut Hw, v: u32, x: u32) {
        self.c(0x3a0, 0x3b8);
        match v {
            0 => self.c(0x3b8, 0x3be),
            1 => {
                self.c(0x3be, 0x3c6);
                self.c(0x3c6, 0x3cc);
            }
            2 => {
                self.c(0x3be, 0x3c6);
                self.c(0x3cc, 0x3d4);
                self.c(0x3d4, 0x3da);
            }
            _ => {
                self.c(0x3be, 0x3c6);
                self.c(0x3cc, 0x3d4);
                self.c(0x3da, 0x3de);
            }
        }
        self.c(0x3de, 0x3ec);
        self.sync(hw);
        hw.vol(v as usize, x as u16 & 0x7f);
        self.host(hw);
        self.c(0x3ec, 0x3f2);
    }

    /// ENPP_DMAMask with d0: its sign bit switches the voices on (and the host
    /// waits for the DMA as dtg_WaitAudioDMA does).
    fn dma_mask(&mut self, hw: &mut Hw, d0: u16) {
        self.c(0x450, 0x468);
        self.sync(hw);
        let v = (d0 & 0xf) | (d0 & 0x8000);
        hw.dmacon(v);
        if v & 0x8000 != 0 {
            let now = hw.bus.clock;
            hw.bus.clock = (now / LINE + 10) * LINE;
        }
        self.host(hw);
        self.c(0x468, 0x46e);
    }

    /// InitPlayer past GetListData: the pointers into the module, the samples'
    /// lengths and addresses (from the last one on), and the synth instruments
    /// past the "SYNT" mark; then the time to InitSound.
    fn init_player(&mut self, hw: &mut Hw) -> Option<()> {
        self.epoch = hw.bus.clock;
        self.cycles = 16;
        self.c(0x37c, 0x39c);
        self.c(0x46e, 0x47a);
        self.cn(0x47a, 0x480, 0x21a);
        for k in CNT..CNT + 0x21a {
            self.w8(hw, k, 0);
        }
        self.c(0x480, 0x4de);
        let m = self.base;
        let rl = |hw: &Hw, a: u32| hw.bus.r32(a);
        let mut a0 = m.wrapping_add(0x10);
        self.w32(hw, P_STBL, a0);
        a0 = a0.wrapping_add((rl(hw, m + 0xc) & 0xffff) * 10);
        let npos = rl(hw, a0.wrapping_add(4));
        self.npos1 = npos.wrapping_sub(1) as u16;
        a0 = a0.wrapping_add(8);
        self.w32(hw, P_OVTB, a0);
        a0 = a0.wrapping_add((npos & 0xffff) << 4);
        let nrows = rl(hw, a0.wrapping_add(4));
        a0 = a0.wrapping_add(8);
        self.w32(hw, P_NTBL, a0);
        a0 = a0.wrapping_add((nrows & 0xffff) << 2);
        self.w32(hw, P_SAMP, 0);
        let n = rl(hw, a0.wrapping_add(4));
        if n == 0 {
            self.c(0x532, 0x534);
            a0 = a0.wrapping_add(2);
        } else {
            self.c(0x4de, 0x50c);
            a0 = a0.wrapping_add(8);
            self.w32(hw, P_SAMP, a0);
            let n16 = n & 0xffff;
            a0 = a0.wrapping_add(n16 * 16).wrapping_add(n16 * 20).wrapping_add(n16 * 8);
            let cnt = (n as u16).wrapping_sub(1) as u32 + 1;
            for k in 0..cnt {
                let x = rl(hw, a0);
                self.w32(hw, LENS + 4 * k, x);
                a0 = a0.wrapping_add(4);
            }
            self.cn(0x50c, 0x512, cnt);
            self.c(0x512, 0x528);
            let d3 = ((n << 2) as u16 as i16 as i32) as u32;
            let mut a1 = self.a(ADDRS).wrapping_add(4).wrapping_add(d3);
            let mut a2 = self.a(LENS).wrapping_add(4).wrapping_add(d3);
            let cnt = (n as u16).wrapping_sub(1) as u32 + 1;
            for _ in 0..cnt {
                a1 = a1.wrapping_sub(4);
                hw.bus.w32(a1, a0);
                a2 = a2.wrapping_sub(4);
                a0 = a0.wrapping_add(hw.bus.r32(a2));
            }
            self.cn(0x528, 0x530, cnt);
            self.c(0x530, 0x532);
        }
        // the "SYNT" mark, by words
        let mut steps = 0u32;
        loop {
            self.c(0x534, 0x53c);
            if rl(hw, a0) == 0x5359_4e54 {
                break;
            }
            self.c(0x532, 0x534);
            a0 = a0.wrapping_add(2);
            steps += 1;
            if steps > 0x100_0000 {
                return None;
            }
        }
        self.c(0x53c, 0x552);
        self.w32(hw, P_SYNT, a0.wrapping_add(8));
        self.c(0x39c, 0x3a0);
        self.sync(hw);
        Some(())
    }

    /// InitSound: DMA off, the voices and the song's variables cleared, subsong
    /// `n`'s speed, positions and tempo, and the timer from the tempo.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.epoch = hw.bus.clock;
        self.cycles = 0;
        self.c(0x552, 0x55c);
        self.dma_mask(hw, 0xf);
        self.c(0x55c, 0x564);
        self.cn(0x564, 0x56a, 0xc4);
        for k in 0..0xc4 {
            self.w8(hw, FXW + k, 0);
        }
        self.c(0x56a, 0x578);
        self.w32(hw, SFX, 0xffff_ffff);
        self.cn(0x578, 0x57e, 14);
        for k in 0..14 {
            self.w8(hw, CNT + k, 0);
        }
        self.c(0x57e, 0x594);
        let s = self.r32(hw, P_STBL).wrapping_add(n as u32 * 10);
        self.cn(0x594, 0x59a, 10);
        for k in 0..10 {
            let x = hw.bus.r8(s + k);
            self.w8(hw, SPEED + k, x);
        }
        self.c(0x59a, 0x5d0);
        for v in 0..4 {
            self.w8(hw, VOICES + 0x24 * v + V_INSTR, 0);
        }
        let first = self.r16(hw, FIRST).wrapping_sub(1);
        self.w16(hw, SONGPOS, first);
        let speed = self.r8(hw, SPEED);
        self.w8(hw, CNT, speed);
        let rows = self.r8(hw, ROWS);
        self.w8(hw, ROW, rows);
        let mut tempo = self.r16(hw, TEMPO) as u32;
        if tempo == 0 {
            self.c(0x5d0, 0x5d4);
            tempo = 50;
        }
        self.c(0x5d4, 0x5e6);
        // mulu.w, then divu.w #50 (the product kept if it overflows)
        let p = tempo * TIMER;
        let q = p / 50;
        self.timer = if q > 0xffff { p as u16 } else { q as u16 };
        // dtg_SetTimer
        self.host(hw);
        self.c(0x5e6, 0x5ec);
        self.sync(hw);
    }

    /// The loops of last row's sample notes (the routine at $860).
    fn loops(&mut self, hw: &mut Hw) {
        self.c(0x860, 0x868);
        let tests: [(u16, u16, u16); 4] = [(0x868, 0x86c, 0x878), (0x878, 0x87e, 0x88a), (0x88a, 0x890, 0x89c), (0x89c, 0x8a2, 0x8ae)];
        for v in 0..4u32 {
            let (t, b, e) = tests[v as usize];
            self.c(t, b);
            if self.r8(hw, NEWNOTE + v) != 0 {
                self.c(b, e);
                self.loop_voice(hw, v);
            }
        }
        self.c(0x8ae, 0x8b6);
        self.w32(hw, NEWNOTE, 0);
    }

    /// A sample's loop (the routine at $8B6).
    fn loop_voice(&mut self, hw: &mut Hw, v: u32) {
        let a2 = VOICES + 0x24 * v;
        self.c(0x8b6, 0x8c0);
        if self.r8(hw, a2 + V_SYNTH) != 0 {
            self.c(0x92c, 0x932);
            return;
        }
        self.c(0x8c0, 0x8d8);
        let i = (self.r8(hw, a2 + V_INSTR) as i8 as i16).wrapping_sub(1) as u16 as u32;
        let a3 = self.r32(hw, P_SAMP).wrapping_add(i * 16);
        let rep = hw.bus.r16(a3.wrapping_add(2));
        if rep == 0 {
            self.c(0x92c, 0x932);
            return;
        }
        self.c(0x8d8, 0x8e0);
        if rep == 1 {
            self.c(0x8e0, 0x8f0);
            let e = self.empty;
            self.poke_adr(hw, v, e);
            self.c(0x8f0, 0x8f6);
            self.poke_len(hw, v, 1);
            self.c(0x8f6, 0x8fc);
        } else {
            // the loop's address is computed, then d4 (0) is passed
            self.c(0x8fc, 0x922);
            self.poke_adr(hw, v, 0);
            self.c(0x922, 0x928);
            self.poke_len(hw, v, rep as u32);
            self.c(0x928, 0x92c);
        }
        self.c(0x92c, 0x932);
    }

    /// A row (the routine at $932).
    fn row(&mut self, hw: &mut Hw) {
        self.c(0x932, 0x940);
        let row = self.r8(hw, ROW).wrapping_add(1);
        self.w8(hw, ROW, row);
        if row as i8 >= self.r8(hw, ROWS) as i8 {
            self.c(0x940, 0x94a);
            self.w8(hw, ROW, 0);
            self.next_pos(hw);
        }
        self.c(0x94a, 0x94e);
        self.fetch(hw);
        // no sound effect to put in, nor to take out
        self.c(0x94e, 0x950);
        self.c(0x976, 0x982);
        self.c(0x9a6, 0x9ac);
        self.c(0x950, 0x954);
        self.dma_off(hw);
        self.c(0x954, 0x958);
        self.play(hw);
        self.c(0x958, 0x95a);
        self.c(0x9ac, 0x9b8);
        self.c(0x9d6, 0x9dc);
        self.c(0x95a, 0x95c);
    }

    /// The next position (the routine at $63C): past the last, the restart (or
    /// the first subsong's first position) and the song's end.
    fn next_pos(&mut self, hw: &mut Hw) {
        self.c(0x63c, 0x650);
        let pos = self.r16(hw, SONGPOS).wrapping_add(1);
        self.w16(hw, SONGPOS, pos);
        if pos as i16 > self.r16(hw, LAST) as i16 {
            self.c(0x650, 0x65a);
            let mut d0 = self.r16(hw, RESTART);
            if d0 == self.npos1 {
                self.c(0x65a, 0x662);
                d0 = hw.bus.r16(self.r32(hw, P_STBL).wrapping_add(2));
            }
            self.c(0x662, 0x66e);
            self.w16(hw, SONGPOS, d0);
            // dtg_SongEnd
            self.songend = true;
            self.host(hw);
        }
        self.c(0x66e, 0x68c);
        let d0 = (self.r16(hw, SONGPOS) << 4) as i16 as i32 as u32;
        let p = self.r32(hw, P_OVTB).wrapping_add(d0);
        for k in 0..16 {
            let x = hw.bus.r8(p.wrapping_add(k));
            self.w8(hw, POSN + k, x);
        }
    }

    /// The voices' rows (the routine at $68C).
    fn fetch(&mut self, hw: &mut Hw) {
        self.c(0x68c, 0x6ae);
        let d0 = (((self.r8(hw, ROW) as i8 as i16) << 2) as i32) as u32;
        let a0 = self.r32(hw, P_NTBL).wrapping_add(d0);
        let r: [(u16, u16, u16); 4] = [(0x6aa, 0x6ae, 0x6b8), (0x6b8, 0x6bc, 0x6ca), (0x6ca, 0x6ce, 0x6dc), (0x6dc, 0x6e0, 0x6ee)];
        for v in 0..4u32 {
            let (t, b, e) = r[v as usize];
            if v > 0 {
                self.c(t, b);
            }
            if self.r8(hw, VOICE_ON + v) != 0 {
                self.c(b, e);
                let d = ((self.r16(hw, POSN + 4 * v) as i16 as i32) << 2) as u32;
                let x = hw.bus.r32(a0.wrapping_add(d));
                self.w32(hw, TRK + 4 * v, x);
            }
        }
        self.c(0x6ee, 0x6f4);
    }

    /// DMA off for each voice with a note and an instrument (the routine at
    /// $6F4), and a byte cleared at `$112E0` + 3, $21, $3F or $5D.
    fn dma_off(&mut self, hw: &mut Hw) {
        self.c(0x6f4, 0x712);
        // (test, flag test, note, instrument, DMA off, after)
        let r: [(u16, u16, u16, u16, u16, u16, u16); 4] = [
            (0x712, 0x716, 0x71a, 0x720, 0x728, 0x72e, 0x03),
            (0x72e, 0x73a, 0x740, 0x746, 0x74e, 0x754, 0x21),
            (0x754, 0x760, 0x766, 0x76c, 0x774, 0x77a, 0x3f),
            (0x77a, 0x786, 0x78c, 0x792, 0x79a, 0x7a0, 0x5d),
        ];
        for v in 0..4u32 {
            let (t, n, i, d, a, e, clr) = r[v as usize];
            self.c(t, n);
            let off = if self.r8(hw, VOICE_ON + v) == 0 {
                true
            } else {
                self.c(n, i);
                if self.r8(hw, TRK + 4 * v) == 0 {
                    false
                } else {
                    self.c(i, d);
                    self.r8(hw, TRK + 4 * v + 1) != 0
                }
            };
            if off {
                self.c(d, a);
                self.dma_mask(hw, 1 << v);
                self.c(a, e);
                self.w8(hw, VOICES + clr as u32, 0);
            }
        }
        self.c(0x7a0, 0x7a6);
    }

    /// The voices' notes (the routine at $7A6), then DMA on.
    fn play(&mut self, hw: &mut Hw) {
        self.c(0x7a6, 0x7b8);
        let r: [(u16, u16, u16); 4] = [(0x7b8, 0x7bc, 0x7d8), (0x7d8, 0x7e4, 0x800), (0x800, 0x80c, 0x828), (0x828, 0x834, 0x850)];
        let mut d7 = 0u16;
        for v in 0..4u32 {
            let (t, b, e) = r[v as usize];
            self.c(t, b);
            if self.r8(hw, VOICE_ON + v) != 0 {
                self.c(b, e);
                self.note(hw, v, &mut d7);
            }
        }
        self.c(0x850, 0x85a);
        self.dma_mask(hw, d7 | 0x8000);
        self.c(0x85a, 0x860);
    }

    /// The row's effect (the routine at $9DC).
    fn row_fx(&mut self, hw: &mut Hw, v: u32, d0: u32) {
        let a2 = VOICES + 0x24 * v;
        let fx = (d0 >> 8) as u8;
        let arg = d0 as u8;
        self.c(0x9dc, 0x9f8);
        self.w16(hw, FXW, fx as u16);
        let n = fx & 0xf;
        if n == 0 {
            self.c(0xb5c, 0xb62);
            return;
        }
        let chain: [(u8, u16, u16); 12] = [
            (1, 0x9f8, 0x9fe),
            (2, 0xa06, 0xa0c),
            (7, 0xa18, 0xa1e),
            (0xa, 0xa32, 0xa38),
            (0xb, 0xa70, 0xa78),
            (0xd, 0xa8e, 0xa94),
            (0xc, 0xaa0, 0xaa6),
            (9, 0xac8, 0xace),
            (0xe, 0xae4, 0xaea),
            (0xf, 0xb1a, 0xb20),
            (4, 0xb32, 0xb38),
            (0xff, 0, 0),
        ];
        for &(e, from, to) in chain.iter() {
            if e == 0xff {
                break;
            }
            self.c(from, to);
            if e == n {
                self.fx(hw, a2, n, arg);
                break;
            }
        }
        self.c(0xb5c, 0xb62);
    }

    fn fx(&mut self, hw: &mut Hw, a2: u32, n: u8, arg: u8) {
        match n {
            1 => {
                self.c(0x9fe, 0xa06);
                self.w8(hw, a2 + V_SLIDE_SPEED, arg);
            }
            2 => {
                self.c(0xa0c, 0xa18);
                self.w8(hw, a2 + V_ADSR_POS, 0);
                self.w8(hw, a2 + V_ADSR_POS + 1, arg);
            }
            7 => {
                self.c(0xa1e, 0xa32);
                self.w16(hw, a2 + V_PORT_COUNT, arg as u16);
                self.w16(hw, a2 + V_PORT_SPEED, arg as u16);
            }
            0xa => {
                self.c(0xa38, 0xa48);
                let mut d4 = (self.r8(hw, a2 + V_VOLUME) as u16).wrapping_add(arg as i8 as u16);
                if (d4 as i16) < 0 {
                    self.c(0xa48, 0xa4c);
                    d4 = 0;
                } else {
                    self.c(0xa4c, 0xa52);
                    if self.r8(hw, a2 + V_SYNTH) == 0 {
                        self.c(0xa52, 0xa58);
                        if d4 > 0x40 {
                            self.c(0xa58, 0xa5e);
                            d4 = 0x40;
                        }
                    } else {
                        self.c(0xa5e, 0xa64);
                        if d4 > 0xff {
                            self.c(0xa64, 0xa68);
                            d4 = 0xff;
                        }
                    }
                }
                self.c(0xa68, 0xa70);
                self.w8(hw, a2 + V_VOLUME, d4 as u8);
            }
            0xb => {
                self.c(0xa78, 0xa8e);
                self.w16(hw, SONGPOS, arg as u16);
                let rows = self.r8(hw, ROWS);
                self.w8(hw, ROW, rows);
            }
            0xd => {
                self.c(0xa94, 0xaa0);
                let rows = self.r8(hw, ROWS);
                self.w8(hw, ROW, rows);
            }
            0xc => {
                self.c(0xaa6, 0xaae);
                let mut d3 = arg;
                if self.r8(hw, a2 + V_SYNTH) == 0 {
                    self.c(0xaae, 0xab4);
                    if d3 > 0x40 {
                        self.c(0xab4, 0xab8);
                        d3 = 0x40;
                    }
                    self.c(0xab8, 0xac0);
                } else {
                    self.c(0xac0, 0xac8);
                }
                self.w8(hw, a2 + V_VOLUME, d3);
            }
            9 => {
                self.c(0xace, 0xad4);
                if arg as i8 > 0 {
                    self.c(0xad4, 0xadc);
                    if arg <= 0x40 {
                        self.c(0xadc, 0xae4);
                        self.w8(hw, ROWS, arg);
                    }
                }
            }
            0xe => {
                // ENPP_PokeCommand 1: the LED filter, on with argument 0
                self.c(0xaea, 0xaee);
                let led = hw.bus.read_byte(0xbfe001);
                if arg == 0 {
                    self.c(0xaee, 0xafe);
                    self.sync(hw);
                    hw.bus.write_byte(0xbfe001, led | 2);
                    self.host(hw);
                    self.c(0xafe, 0xb04);
                } else {
                    self.c(0xb04, 0xb14);
                    self.sync(hw);
                    hw.bus.write_byte(0xbfe001, led & !2);
                    self.host(hw);
                    self.c(0xb14, 0xb1a);
                }
            }
            0xf => {
                self.c(0xb20, 0xb24);
                if arg as i8 > 0 {
                    self.c(0xb24, 0xb2a);
                    if arg as i8 <= 0x10 {
                        self.c(0xb2a, 0xb32);
                        self.w8(hw, SPEED, arg);
                    }
                }
            }
            _ => {
                // 4: vibrato
                self.c(0xb38, 0xb5c);
                self.w8(hw, a2 + V_VIB_DELAY, 0);
                self.w8(hw, a2 + V_VIB_SPEED, (((arg as i8) >> 4) as u8 & 0xf) << 1);
                self.w8(hw, a2 + V_VIB_LEVEL, 0xa0u8.wrapping_sub((arg & 0xf) << 4));
            }
        }
    }

    /// A voice's note (the routine at $B62).
    fn note(&mut self, hw: &mut Hw, v: u32, d7: &mut u16) {
        let a2 = VOICES + 0x24 * v;
        let d0 = self.r32(hw, TRK + 4 * v);
        let d1 = self.r32(hw, POSN + 4 * v);
        self.c(0xb62, 0xb6a);
        self.row_fx(hw, v, d0);
        self.c(0xb6a, 0xb80);
        let mut d3 = (d0 >> 24) as u8;
        let mut d4 = (d0 >> 16) as u8 as u16;
        if d3 == 0 {
            self.c(0xb80, 0xb84);
            if d4 != 0 {
                self.c(0xb84, 0xb88);
                self.restore(hw, v, d4 as u8);
                self.c(0xb88, 0xb8a);
            }
            self.c(0xbc6, 0xbcc);
            return;
        }
        self.c(0xb8a, 0xb90);
        if d3 == 0x80 {
            self.c(0xbc6, 0xbcc);
            return;
        }
        self.c(0xb90, 0xb96);
        if d3 == 0x7f {
            self.c(0xb96, 0xb9a);
            self.quiet(hw, v);
            self.c(0xb9a, 0xb9c);
            self.c(0xbc6, 0xbcc);
            return;
        }
        // the transposes (the routine at $CA6), but for effect bits 6 or 7
        self.c(0xb9c, 0xba0);
        self.c(0xca6, 0xcbe);
        let d6 = (self.r16(hw, FXW) & 0xc0) >> 4;
        if d6 != 4 {
            self.c(0xcbe, 0xcc0);
            d3 = d3.wrapping_add(d1 as u8);
        }
        self.c(0xcc0, 0xcc6);
        if d6 != 8 {
            self.c(0xcc6, 0xcd4);
            d4 = (d4 as u8).wrapping_add((d1 >> 8) as u8) as u16;
        }
        self.c(0xcd4, 0xcda);
        self.c(0xba0, 0xbac);
        let prev = self.r8(hw, a2 + V_NOTE);
        self.w8(hw, a2 + V_PREV, prev);
        self.w8(hw, a2 + V_NOTE, d3);
        let ins = d4 as u8 as i8;
        if ins > 0 {
            self.c(0xbac, 0xbb2);
            self.c(0xbb2, 0xbb6);
            self.reset(hw, a2);
            self.c(0xbb6, 0xbbc);
            if ins as u8 & 0x40 != 0 {
                self.c(0xbbc, 0xbc0);
                self.sample(hw, v, ins as u8, d7);
                self.c(0xbc0, 0xbc2);
            } else {
                self.c(0xbc2, 0xbc6);
                self.synth(hw, v, ins as u8, d7);
            }
        }
        self.c(0xbc6, 0xbcc);
    }

    /// Volume 0 straight to Paula, the voice's volume and instrument cleared
    /// (the routine at $CDA).
    fn quiet(&mut self, hw: &mut Hw, v: u32) {
        let a2 = VOICES + 0x24 * v;
        self.c(0xcda, 0xcde);
        self.sync(hw);
        hw.vol(v as usize, 0);
        self.c(0xcde, 0xcea);
        self.w8(hw, a2 + V_VOLUME, 0);
        self.w8(hw, a2 + V_INSTR, 0);
        self.w8(hw, NEWNOTE + v, 0);
    }

    /// A new note's voice state (the routine at $CEA).
    fn reset(&mut self, hw: &mut Hw, a2: u32) {
        self.c(0xcea, 0xd0a);
        self.w8(hw, a2 + V_SLIDE_SPEED, 0);
        self.w8(hw, a2 + V_INSTR, 0);
        self.w16(hw, a2 + V_ADSR_POS, 0);
        self.w16(hw, a2 + V_VIB_POS, 0);
        self.w16(hw, a2 + V_LFO_POS, 0);
        let fx = self.r16(hw, FXW);
        if fx != 7 {
            self.c(0xd0a, 0xd12);
            self.w16(hw, a2 + V_PORT_COUNT, 0);
            self.w16(hw, a2 + V_PORT_SPEED, 0);
        }
        self.c(0xd12, 0xd20);
        self.w16(hw, a2 + V_SLIDE, 0);
        if fx != 4 {
            self.c(0xd20, 0xd2c);
            self.w8(hw, a2 + V_VIB_DELAY, 0);
            self.w8(hw, a2 + V_VIB_SPEED, 0);
            self.w8(hw, a2 + V_VIB_LEVEL, 0);
        }
        self.c(0xd2c, 0xd40);
        self.w8(hw, a2 + V_EG_DIR, 0);
        self.w16(hw, a2 + V_EG_POS, 0);
        self.w16(hw, a2 + V_ARP_POS, 0);
        self.w16(hw, a2 + V_SUSTAIN, 0);
    }

    /// The instrument's address: `(n & $3F) - 1` instruments of `size` in.
    fn instr(&self, hw: &Hw, p: u32, n: u8, size: u32) -> u32 {
        let i = ((n & 0x3f) as u32).wrapping_sub(1) as u16 as u32;
        self.r32(hw, p).wrapping_add(i * size)
    }

    /// The vibrato and portamento of an instrument (the code at $C00..$C2A or
    /// $D70..$D9A, or a synth's at $C60..$C8A or $E22..$E4C).
    fn vib_port(&mut self, hw: &mut Hw, a2: u32, a5: u32, d5: u8, synth: bool, at: [u16; 6]) -> bool {
        let (vib, port_v, port_s) = if synth { (0x1b, 0x1e, 9) } else { (6, 9, 9) };
        if d5 != 4 {
            self.c(at[0], at[1]);
            for k in 0..3 {
                let x = hw.bus.r8(a5 + vib + k);
                self.w8(hw, a2 + V_VIB_DELAY + k, x);
            }
        }
        self.c(at[1], at[2]);
        if d5 == 8 {
            return true;
        }
        self.c(at[3], at[4]);
        if d5 == 7 {
            return false;
        }
        self.c(at[4], at[5]);
        let x = hw.bus.r8(a5 + port_v);
        self.w8(hw, a2 + V_PORT_COUNT + 1, x);
        let x = hw.bus.r8(a5 + port_s);
        self.w8(hw, a2 + V_PORT_SPEED + 1, x);
        true
    }

    /// An instrument with no note (the routine at $BCC): its vibrato,
    /// portamento and volume, but for the row's effect.
    fn restore(&mut self, hw: &mut Hw, v: u32, d4: u8) {
        let a2 = VOICES + 0x24 * v;
        let d5 = (self.r16(hw, FXW) & 0xf) as u8;
        self.c(0xbcc, 0xbe4);
        let sample_vol = |s: &mut Self, hw: &mut Hw, a5: u32, off: u32| {
            s.c(0xc2a, 0xc32);
            if d5 != 0xc {
                s.c(0xc32, 0xc3a);
                if d5 != 0xa {
                    s.c(0xc3a, 0xc40);
                    let x = hw.bus.r8(a5 + off);
                    s.w8(hw, a2 + V_VOLUME, x);
                }
            }
            s.c(0xc40, 0xc44);
        };
        if self.r8(hw, a2 + V_SYNTH) == 0 {
            self.c(0xbe4, 0xc00);
            let a5 = self.instr(hw, P_SAMP, d4, 16);
            self.w8(hw, a2 + V_INSTR, d4 & 0x3f);
            self.vib_port(hw, a2, a5, d5, false, [0xc00, 0xc12, 0xc18, 0xc18, 0xc1e, 0xc2a]);
            sample_vol(self, hw, a5, 5);
        } else {
            self.c(0xc44, 0xc60);
            let a5 = self.instr(hw, P_SYNT, d4, 0x3f2);
            self.w8(hw, a2 + V_INSTR, d4 & 0x3f);
            if !self.vib_port(hw, a2, a5, d5, true, [0xc60, 0xc72, 0xc78, 0xc78, 0xc7e, 0xc8a]) {
                // effect 7: into the sample code, with the synth's 6th byte
                sample_vol(self, hw, a5, 5);
            } else {
                self.c(0xc8a, 0xc92);
                if d5 != 0xc {
                    self.c(0xc92, 0xc9a);
                    if d5 != 0xa {
                        self.c(0xc9a, 0xca0);
                        let x = hw.bus.r8(a5 + 0x1a);
                        self.w8(hw, a2 + V_VOLUME, x);
                    }
                }
            }
        }
        self.c(0xca0, 0xca6);
    }

    /// A sample note (the routine at $D40).
    fn sample(&mut self, hw: &mut Hw, v: u32, d4: u8, d7: &mut u16) {
        let a2 = VOICES + 0x24 * v;
        self.c(0xd40, 0xd70);
        self.w8(hw, a2 + V_SYNTH, 0);
        let d5 = (self.r16(hw, FXW) & 0xf) as u8;
        let a5 = self.instr(hw, P_SAMP, d4, 16);
        self.w8(hw, a2 + V_INSTR, d4 & 0x3f);
        self.vib_port(hw, a2, a5, d5, false, [0xd70, 0xd82, 0xd88, 0xd88, 0xd8e, 0xd9a]);
        self.c(0xd9a, 0xda6);
        let num = hw.bus.r8(a5 + 4) as i8 as i16;
        let addr = if num < 0 {
            0
        } else {
            self.c(0xda6, 0xdae);
            hw.bus.r32(self.a(ADDRS).wrapping_add(((num << 2) as i32) as u32))
        };
        if addr == 0 {
            self.c(0xdae, 0xdb2);
            self.quiet(hw, v);
            self.c(0xdb2, 0xdb4);
            self.c(0xdec, 0xdf2);
            return;
        }
        self.c(0xdb4, 0xdbc);
        self.poke_adr(hw, v, addr);
        self.c(0xdbc, 0xdc8);
        let len = hw.bus.r16(a5).wrapping_add(hw.bus.r16(a5 + 2));
        self.poke_len(hw, v, len as u32);
        self.c(0xdc8, 0xdce);
        if d5 != 0xc {
            self.c(0xdce, 0xdd4);
            if d5 != 0xa {
                self.c(0xdd4, 0xde0);
                let x = hw.bus.r8(a5 + 5);
                self.poke_vol(hw, v, x as u32);
                self.c(0xde0, 0xde8);
                self.w8(hw, a2 + V_VOLUME, x);
            }
        }
        self.c(0xde8, 0xdec);
        *d7 |= 1 << v;
        self.w8(hw, NEWNOTE + v, 0xff);
        self.c(0xdec, 0xdf2);
    }

    /// A synth note (the routine at $DF2): its waveform (from the envelope
    /// generator's start), length and volume.
    fn synth(&mut self, hw: &mut Hw, v: u32, d4: u8, d7: &mut u16) {
        let a2 = VOICES + 0x24 * v;
        self.c(0xdf2, 0xe22);
        self.w8(hw, a2 + V_SYNTH, 0xff);
        let d5 = (self.r16(hw, FXW) & 0xf) as u8;
        let a5 = self.instr(hw, P_SYNT, d4, 0x3f2);
        self.w8(hw, a2 + V_INSTR, d4 & 0x3f);
        self.vib_port(hw, a2, a5, d5, true, [0xe22, 0xe34, 0xe3a, 0xe3a, 0xe40, 0xe4c]);
        self.c(0xe4c, 0xe52);
        let b = |o: u32| hw.bus.r8(a5 + o);
        let (eg_on, eg_free, eg_a, eg_b, eg_first) = (b(0x2a), b(0x29), b(0x2b), b(0x2c), b(0x172));
        let mut wave: u32 = 0;
        let mut off = eg_on == 0;
        if !off {
            self.c(0xe5a, 0xe60);
            if eg_free != 0 {
                self.c(0xe60, 0xe72);
                self.w16(hw, a2 + V_EG_POS, 0);
                if eg_a.wrapping_add(eg_b) == 0 {
                    off = true;
                } else {
                    self.c(0xe72, 0xe7a);
                    wave = eg_first as u32;
                }
            } else {
                self.c(0xe7a, 0xe90);
                self.w16(hw, a2 + V_EG_POS, (eg_a as u16) << 8);
                self.w8(hw, a2 + V_EG_DIR, 1);
                wave = eg_a as u32;
            }
        }
        if off {
            self.c(0xe52, 0xe5a);
            self.w8(hw, a2 + V_EG_DIR, 0);
            wave = 0;
        }
        self.c(0xe90, 0xe98);
        if wave & 1 != 0 {
            self.c(0xe98, 0xeaa);
            self.poke_adr(hw, v, a5.wrapping_add(0x2f2).wrapping_add(wave & !1));
            self.c(0xeaa, 0xeac);
        } else {
            self.c(0xeac, 0xeb8);
            self.poke_adr(hw, v, a5.wrapping_add(0x1f2).wrapping_add(wave));
        }
        self.c(0xeb8, 0xec2);
        let len = hw.bus.r16(a5 + 0x18) >> 1;
        self.poke_len(hw, v, len as u32);
        self.c(0xec2, 0xec8);
        if d5 != 0xc {
            self.c(0xec8, 0xece);
            if d5 != 0xa {
                self.c(0xece, 0xed4);
                let x = hw.bus.r8(a5 + 0x1a);
                self.w8(hw, a2 + V_VOLUME, x);
            }
        }
        self.c(0xed4, 0xede);
        *d7 |= 1 << v;
        self.w8(hw, NEWNOTE + v, 0);
    }

    /// The voices' effects every tick (the routine at $EDE).
    fn effects(&mut self, hw: &mut Hw) {
        self.c(0xede, 0xefa);
        self.fx_voice(hw, 0);
        self.c(0xefa, 0xf0a);
        self.fx_voice(hw, 1);
        self.c(0xf0a, 0xf1a);
        self.fx_voice(hw, 2);
        self.c(0xf1a, 0xf2a);
        self.fx_voice(hw, 3);
        self.c(0xf2a, 0xf30);
    }

    /// A voice's effects (the routine at $F30): arpeggio, portamento, vibrato
    /// (and a synth's LFO) into the period, the volume (a synth's through its
    /// ADSR), a synth's envelope generator, then the slide.
    fn fx_voice(&mut self, hw: &mut Hw, v: u32) {
        let a2 = VOICES + 0x24 * v;
        let d0 = self.r32(hw, TRK + 4 * v);
        self.c(0xf30, 0xf36);
        let ins = self.r8(hw, a2 + V_INSTR);
        if ins == 0 {
            self.c(0xf36, 0xf3e);
            self.poke_vol(hw, v, 0);
            self.c(0xf3e, 0xf42);
            return;
        }
        self.c(0xf42, 0xf4c);
        let i = (ins as i8 as i16).wrapping_sub(1) as u16 as u32;
        if self.r8(hw, a2 + V_SYNTH) == 0 {
            self.c(0xf4c, 0xf60);
            let _a4 = self.r32(hw, P_SAMP).wrapping_add(i * 16);
            let (mut d2, d3) = self.sample_arp(hw, a2, d0);
            self.c(0xf60, 0xf64);
            d2 = self.porta(hw, a2, d2, d3);
            self.c(0xf64, 0xf68);
            d2 = self.vibrato(hw, a2, d2);
            self.c(0xf68, 0xf72);
            self.poke_per(hw, v, d2 & 0xffff);
            self.c(0xf72, 0xf7c);
            let x = self.r8(hw, a2 + V_VOLUME);
            self.poke_vol(hw, v, x as u32);
            self.c(0xf7c, 0xf80);
        } else {
            self.c(0xf80, 0xf96);
            let a4 = self.r32(hw, P_SYNT).wrapping_add(i * 0x3f2);
            let (mut d2, d3) = self.synth_arp(hw, a2, a4, d0);
            self.c(0xf96, 0xf9a);
            d2 = self.porta(hw, a2, d2, d3);
            self.c(0xf9a, 0xf9e);
            d2 = self.vibrato(hw, a2, d2);
            self.c(0xf9e, 0xfa2);
            d2 = self.lfo(hw, a2, a4, d2);
            self.c(0xfa2, 0xfac);
            self.poke_per(hw, v, d2 & 0xffff);
            self.c(0xfac, 0xfb2);
            self.adsr(hw, v, a4, d0);
            self.c(0xfb2, 0xfb6);
            self.eg(hw, v, a4);
        }
        self.c(0xfb6, 0xfc6);
        let s = self.r16(hw, a2 + V_SLIDE).wrapping_sub(self.r8(hw, a2 + V_SLIDE_SPEED) as i8 as u16);
        self.w16(hw, a2 + V_SLIDE, s);
    }

    /// A sample's arpeggio (the routine at $FC6): with no effect, the
    /// argument's nibbles by turns; the note's and the previous note's periods.
    fn sample_arp(&mut self, hw: &mut Hw, a2: u32, d0: u32) -> (u32, u32) {
        self.c(0xfc6, 0xfde);
        let mut d2 = self.r8(hw, a2 + V_NOTE) as u32;
        let mut d3 = self.r8(hw, a2 + V_PREV) as u32;
        if d0 & 0xf00 == 0 {
            self.c(0xfde, 0xfe6);
            let mut d4 = d0 as u8;
            if self.r16(hw, a2 + V_ARP_TOGGLE) == 0 {
                self.c(0xfe6, 0xfee);
                d4 >>= 4;
                self.w8(hw, a2 + V_ARP_TOGGLE, 0xff);
            } else {
                self.c(0xfee, 0xff2);
                self.w8(hw, a2 + V_ARP_TOGGLE, 0);
            }
            self.c(0xff2, 0xffa);
            d2 += (d4 & 0xf) as u32;
            d3 += (d4 & 0xf) as u32;
        }
        self.c(0xffa, 0x1010);
        let p2 = self.r16(hw, PERIODS_AT + ((d2 << 1) as u16 as i16 as i32 as u32)) as u32;
        let p3 = self.r16(hw, PERIODS_AT + ((d3 << 1) as u16 as i16 as i32 as u32)) as u32;
        (p2, p3)
    }

    /// A synth's arpeggio (the routine at $1010): one of its three tables, by
    /// the effect's bits 4 and 5.
    fn synth_arp(&mut self, hw: &mut Hw, a2: u32, a4: u32, d0: u32) -> (u32, u32) {
        self.c(0x1010, 0x102c);
        let mut d2 = self.r8(hw, a2 + V_NOTE) as u32;
        let mut d3 = self.r8(hw, a2 + V_PREV) as u32;
        let t = ((d0 as u16 >> 8) & 0x30) >> 4;
        if t != 0 {
            self.c(0x102c, 0x1052);
            let a5 = a4.wrapping_add((((t - 1) * 16) as i16 as i32) as u32).wrapping_add(0x142);
            let pos = self.r16(hw, a2 + V_ARP_POS) as i16 as i32 as u32;
            let x = hw.bus.r8(a5.wrapping_add(2).wrapping_add(pos));
            d2 = (d2 as u8).wrapping_add(x) as u32;
            d3 = (d3 as u8).wrapping_add(x) as u32;
            let end = hw.bus.r8(a5).wrapping_add(hw.bus.r8(a5 + 1));
            if end != self.r8(hw, a2 + V_ARP_POS + 1) {
                self.c(0x1052, 0x1058);
                let p = self.r8(hw, a2 + V_ARP_POS + 1).wrapping_add(1);
                self.w8(hw, a2 + V_ARP_POS + 1, p);
            } else {
                self.c(0x1058, 0x105c);
                let s = hw.bus.r8(a5);
                self.w8(hw, a2 + V_ARP_POS + 1, s);
            }
        }
        self.c(0x105c, 0x1072);
        let p2 = self.r16(hw, PERIODS_AT + ((d2 << 1) as u16 as i16 as i32 as u32)) as u32;
        let p3 = self.r16(hw, PERIODS_AT + ((d3 << 1) as u16 as i16 as i32 as u32)) as u32;
        (p2, p3)
    }

    /// The vibrato (the routine at $1072), then the slide added.
    fn vibrato(&mut self, hw: &mut Hw, a2: u32, mut d2: u32) -> u32 {
        self.c(0x1072, 0x107e);
        let delay = self.r8(hw, a2 + V_VIB_DELAY);
        if delay != 0xff {
            self.c(0x107e, 0x1084);
            if delay != 0 {
                self.c(0x1084, 0x108a);
                self.w8(hw, a2 + V_VIB_DELAY, delay - 1);
            } else {
                self.c(0x108a, 0x10a2);
                let pos = self.r16(hw, a2 + V_VIB_POS) as i16 as i32 as u32;
                let x = self.r8(hw, VIBTAB + pos);
                let level = self.r8(hw, a2 + V_VIB_LEVEL) as u32;
                if x as i8 >= 0 {
                    self.c(0x10a2, 0x10a6);
                    if level != 0 {
                        self.c(0x10a6, 0x10ae);
                        let q = ((x as u32) << 2) / level;
                        d2 = (d2 & 0xffff_0000) | (d2 as u16).wrapping_add(q as u16) as u32;
                    }
                } else {
                    self.c(0x10ae, 0x10b4);
                    if level != 0 {
                        self.c(0x10b4, 0x10ba);
                        let q = ((x.wrapping_neg() as u32) << 2) / level;
                        d2 = (d2 & 0xffff_0000) | (d2 as u16).wrapping_sub(q as u16) as u32;
                    }
                }
                self.c(0x10ba, 0x10c2);
                let p = self.r8(hw, a2 + V_VIB_POS + 1).wrapping_add(self.r8(hw, a2 + V_VIB_SPEED));
                self.w8(hw, a2 + V_VIB_POS + 1, p);
            }
        }
        self.c(0x10c2, 0x10cc);
        (d2 & 0xffff_0000) | (d2 as u16).wrapping_add(self.r16(hw, a2 + V_SLIDE)) as u32
    }

    /// The portamento (the routine at $10CC), from the previous note's period.
    fn porta(&mut self, hw: &mut Hw, a2: u32, mut d2: u32, d3: u32) -> u32 {
        self.c(0x10cc, 0x10d4);
        if self.r16(hw, a2 + V_PORT_SPEED) != 0 {
            self.c(0x10d4, 0x10da);
            if self.r16(hw, a2 + V_PORT_COUNT) != 0 {
                self.c(0x10da, 0x10de);
                if d2 as u16 != d3 as u16 {
                    self.c(0x10de, 0x10f2);
                    let cnt = self.r16(hw, a2 + V_PORT_COUNT).wrapping_sub(1);
                    self.w16(hw, a2 + V_PORT_COUNT, cnt);
                    // exg: the previous period less the current one, times the counter
                    let cur = d2;
                    let diff = (d3 as u16).wrapping_sub(cur as u16) as i16 as i32;
                    d2 = diff.wrapping_mul(cnt as i16 as i32) as u32;
                    let sp = self.r16(hw, a2 + V_PORT_SPEED) as i16;
                    if sp != 0 {
                        self.c(0x10f2, 0x10f4);
                        d2 = divs(d2, sp);
                    }
                    self.c(0x10f4, 0x10f6);
                    d2 = (d2 & 0xffff_0000) | (d2 as u16).wrapping_add(cur as u16) as u32;
                }
            }
        }
        self.c(0x10f6, 0x10fe);
        d2
    }

    /// A synth's LFO (the routine at $1296), off the period.
    fn lfo(&mut self, hw: &mut Hw, a2: u32, a4: u32, mut d2: u32) -> u32 {
        self.c(0x1296, 0x12a8);
        let end = hw.bus.r8(a4 + 0x27).wrapping_add(hw.bus.r8(a4 + 0x28));
        if end != 0 {
            self.c(0x12a8, 0x12c6);
            let pos = self.r16(hw, a2 + V_LFO_POS).wrapping_add(0xc2) as i16 as i32 as u32;
            let x = hw.bus.r8(a4.wrapping_add(pos)) as i8 as u16;
            d2 = (d2 & 0xffff_0000) | (d2 as u16).wrapping_sub(x) as u32;
            if end != self.r8(hw, a2 + V_LFO_POS + 1) {
                self.c(0x12c6, 0x12cc);
                let p = self.r8(hw, a2 + V_LFO_POS + 1).wrapping_add(1);
                self.w8(hw, a2 + V_LFO_POS + 1, p);
            } else {
                self.c(0x12cc, 0x12d2);
                let s = hw.bus.r8(a4 + 0x27);
                self.w8(hw, a2 + V_LFO_POS + 1, s);
            }
        }
        self.c(0x12d2, 0x12d8);
        d2
    }

    /// A synth's ADSR volume (the routine at $10FE), with its sustain on note
    /// `$80`.
    fn adsr(&mut self, hw: &mut Hw, v: u32, a4: u32, d0: u32) {
        let a2 = VOICES + 0x24 * v;
        self.c(0x10fe, 0x1110);
        let end = hw.bus.r8(a4 + 0x1f).wrapping_add(hw.bus.r8(a4 + 0x20));
        if end == 0 {
            self.c(0x1110, 0x111c);
            let x = self.r8(hw, a2 + V_VOLUME) >> 2;
            self.poke_vol(hw, v, x as u32);
            self.c(0x111c, 0x1120);
            self.c(0x11a2, 0x11a8);
            return;
        }
        self.c(0x1120, 0x113a);
        let pos = self.r16(hw, a2 + V_ADSR_POS);
        let t = hw.bus.r8(a4.wrapping_add(0x42).wrapping_add(pos as i16 as i32 as u32)) as u32;
        let x = (self.r8(hw, a2 + V_VOLUME) as u32 * t) >> 10;
        self.poke_vol(hw, v, x);
        self.c(0x113a, 0x114e);
        if end as i16 <= pos as i16 {
            self.c(0x114e, 0x115a);
            self.w16(hw, a2 + V_ADSR_POS, 0);
            let s = hw.bus.r8(a4 + 0x1f);
            self.w8(hw, a2 + V_ADSR_POS + 1, s);
            self.c(0x11a2, 0x11a8);
            return;
        }
        self.c(0x115a, 0x116c);
        let mut next = true;
        if d0 >> 24 == 0x80 {
            self.c(0x1172, 0x117a);
            let sus = hw.bus.r8(a4 + 0x26);
            if sus != 1 {
                self.c(0x117a, 0x1184);
                if self.r8(hw, a2 + V_ADSR_POS + 1) as i8 >= hw.bus.r8(a4 + 0x25) as i8 {
                    self.c(0x1184, 0x118a);
                    if sus == 0 {
                        next = false;
                    } else {
                        self.c(0x118a, 0x1190);
                        let n = self.r16(hw, a2 + V_SUSTAIN);
                        if n != 0 {
                            self.c(0x1190, 0x1196);
                            self.w16(hw, a2 + V_SUSTAIN, n - 1);
                            next = false;
                        } else {
                            self.c(0x1196, 0x11a2);
                            self.w16(hw, a2 + V_SUSTAIN, sus as i8 as u16);
                        }
                    }
                }
            }
        }
        if next {
            self.c(0x116c, 0x1172);
            let p = self.r16(hw, a2 + V_ADSR_POS).wrapping_add(1);
            self.w16(hw, a2 + V_ADSR_POS, p);
        }
        self.c(0x11a2, 0x11a8);
    }

    /// A synth's envelope generator (the routine at $11A8): the waveform's
    /// location moves through the instrument's table (free) or between its
    /// two bounds (calculated).
    fn eg(&mut self, hw: &mut Hw, v: u32, a4: u32) {
        let a2 = VOICES + 0x24 * v;
        self.c(0x11a8, 0x11b4);
        let b = |o: u32| hw.bus.r8(a4 + o);
        let (on, free, lo, hi, down, up) = (b(0x2a), b(0x29), b(0x2b), b(0x2c), b(0x2e), b(0x2d));
        if on == 0 {
            self.c(0x1290, 0x1296);
            return;
        }
        self.c(0x11b4, 0x11ba);
        let d4: u32;
        if free != 0 {
            self.c(0x11ba, 0x11ca);
            let end = lo.wrapping_add(hi) as u16;
            if end == 0 {
                self.c(0x1290, 0x1296);
                return;
            }
            self.c(0x11ca, 0x11d0);
            let pos = self.r16(hw, a2 + V_EG_POS);
            if end as i16 >= pos as i16 {
                self.c(0x11dc, 0x11e0);
                self.w16(hw, a2 + V_EG_POS, pos.wrapping_add(1));
            } else {
                self.c(0x11d0, 0x11dc);
                self.w16(hw, a2 + V_EG_POS, lo as u16);
            }
            self.c(0x11e0, 0x11f6);
            let pos = self.r16(hw, a2 + V_EG_POS).wrapping_add(0x172) as i16 as i32 as u32;
            d4 = hw.bus.r8(a4.wrapping_add(pos)) as u32;
        } else {
            self.c(0x11f6, 0x11fe);
            let dir = self.r8(hw, a2 + V_EG_DIR);
            if dir == 0 {
                self.c(0x1290, 0x1296);
                return;
            }
            self.c(0x11fe, 0x120a);
            let pos = self.r16(hw, a2 + V_EG_POS);
            if dir as i8 > 0 {
                self.c(0x1236, 0x124a);
                let d5 = pos.wrapping_add((up as u16) << 5);
                if (d5 >> 8) as u8 as i8 >= hi as i8 {
                    self.c(0x1250, 0x1260);
                    self.w16(hw, a2 + V_EG_POS, (hi as u16) << 8);
                    self.w8(hw, a2 + V_EG_DIR, 0xff);
                } else {
                    self.c(0x124a, 0x1250);
                    self.w16(hw, a2 + V_EG_POS, d5);
                }
            } else {
                self.c(0x120a, 0x121e);
                let d5 = pos.wrapping_sub((down as u16) << 5);
                if (d5 >> 8) as u8 as i8 <= lo as i8 {
                    self.c(0x1224, 0x1236);
                    self.w16(hw, a2 + V_EG_POS, (lo as u16) << 8);
                    self.w8(hw, a2 + V_EG_DIR, 1);
                } else {
                    self.c(0x121e, 0x1224);
                    self.w16(hw, a2 + V_EG_POS, d5);
                }
            }
            self.c(0x1260, 0x1266);
            d4 = self.r8(hw, a2 + V_EG_POS) as u32;
        }
        self.c(0x1266, 0x126e);
        let a5 = if d4 & 1 != 0 {
            self.c(0x126e, 0x127e);
            a4.wrapping_add(0x2f2).wrapping_add(d4 & !1)
        } else {
            self.c(0x127e, 0x1286);
            a4.wrapping_add(0x1f2).wrapping_add(d4)
        };
        self.c(0x1286, 0x128e);
        self.poke_adr(hw, v, a5);
        self.c(0x128e, 0x1296);
    }
}

/// DIVS.W: the quotient in the low word and the remainder in the high one, or
/// the dividend unchanged when the quotient does not fit.
fn divs(d: u32, s: i16) -> u32 {
    let (n, s) = (d as i32, s as i32);
    let q = n.wrapping_div(s);
    if !(-0x8000..0x8000).contains(&q) {
        return d;
    }
    let r = n.wrapping_rem(s);
    ((r as u32) << 16) | (q as u32 & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::PERIODS;
    use crate::engine::Engine;
    use std::path::Path;

    /// Two subsongs (speed 3, 4 rows, position 0; the second at tempo 25), one
    /// position: voice 0 plays note 24 with synth 1 (32 bytes of a square, at
    /// volume $30, no ADSR, envelope generator or vibrato); the other voices
    /// play empty rows. No samples.
    fn module() -> Vec<u8> {
        let mut d = b"IS20DF10STBL".to_vec();
        d.extend(2u32.to_be_bytes());
        d.extend([3, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
        d.extend([3, 4, 0, 0, 0, 0, 0, 0, 0, 25]);
        d.extend(b"OVTB");
        d.extend(1u32.to_be_bytes());
        for v in 0..4u16 {
            d.extend((if v == 0 { 0u16 } else { 4 }).to_be_bytes());
            d.extend([0, 0]);
        }
        d.extend(b"NTBL");
        d.extend(8u32.to_be_bytes());
        let mut t = vec![0u8; 8 * 4];
        t[..4].copy_from_slice(&[24, 1, 0, 0]);
        d.extend(t);
        d.extend(b"SAMP");
        d.extend(0u32.to_be_bytes());
        d.extend(b"SYNT");
        d.extend(1u32.to_be_bytes());
        let mut s = vec![0u8; 0x3f2];
        s[..4].copy_from_slice(b"IS20");
        s[0x18..0x1a].copy_from_slice(&32u16.to_be_bytes());
        s[0x1a] = 0x30;
        s[0x1b] = 0xff; // no vibrato
        for k in 0..32 {
            s[0x1f2 + k] = if k < 16 { 0x60 } else { 0xa0 };
        }
        d.extend(s);
        d
    }

    fn run(sub: Option<u16>) -> (Engine, Vec<(u32, u16, u64)>, Vec<f32>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new("is20.test"), module()).unwrap();
        if let Some(n) = sub {
            e.change_subsong(n as i32).unwrap();
        }
        e.m.bus.write_log = Some(Vec::new());
        let mut out = vec![0f32; 2 * 24000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (e, log, out)
    }

    #[test]
    fn plays_a_synth_on_paula() {
        let (e, log, out) = run(None);
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "InStereo! 2.0");
        assert_eq!((p.sub_min, p.sub_max), (0, 1));
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the waveform's length in words; its volume through the ADSR (none:
        // a quarter), the other voices' 0
        let len = reg(0xDFF0A4);
        assert!(!len.is_empty() && len.iter().all(|&x| x == 16));
        assert!(reg(0xDFF0A8).iter().all(|&v| v == 0x0c));
        assert!(reg(0xDFF0B8).iter().all(|&v| v == 0));
        let per = reg(0xDFF0A6);
        assert!(per.len() > 10 && per.iter().all(|&x| x == PERIODS[24]));
        // DMA off for the note, then on every row (with no voice but the first)
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..3], &[0x0001, 0x8001, 0x8000]);
        let t = |a: u32, v: u16| log.iter().find(|w| w.0 == a && w.1 == v).unwrap().2;
        assert!(t(0xDFF096, 0x0001) < t(0xDFF0A4, 16));
        assert!(t(0xDFF0A4, 16) < t(0xDFF096, 0x8001));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }

    #[test]
    fn the_tempo_sets_the_timer() {
        let ticks = |log: &[(u32, u16, u64)]| {
            let t: Vec<u64> = log.iter().filter(|w| w.0 == 0xDFF0A6).map(|w| w.2).collect();
            t[2] - t[1]
        };
        let (_, log, _) = run(None);
        assert_eq!(ticks(&log), (14187 + 1) * 5);
        let (e, log, _) = run(Some(1));
        assert_eq!(e.playing.as_ref().unwrap().sub, 1);
        assert_eq!(ticks(&log), (25 * 14187 / 50 + 1) * 5);
    }
}

/// The 68000 cycles of each instruction of the original's code (as the emulator times
/// it), by word from hunk offset `CYC_FROM` (0 between instructions).
const CYC_FROM: u16 = 0x2e4;
const CYC: [u8; 2042] = [
    4, 16, 0, 9, 4, 16, 0, 18, 0, 10, 0, 9, 8, 0, 12, 0, 0, 16, 0, 4,
    8, 0, 12, 10, 0, 16, 0, 0, 16, 0, 0, 16, 0, 0, 16, 0, 0, 8, 0, 16,
    0, 4, 16, 4, 16, 16, 0, 4, 20, 0, 0, 9, 24, 0, 0, 0, 9, 24, 0, 0,
    0, 9, 4, 16, 4, 4, 16, 0, 16, 0, 8, 16, 4, 16, 0, 16, 8, 0, 12, 28,
    0, 0, 28, 0, 0, 20, 0, 0, 16, 0, 18, 0, 4, 16, 48, 0, 16, 0, 8, 0,
    16, 0, 14, 0, 0, 9, 12, 0, 10, 14, 0, 0, 9, 12, 0, 10, 14, 0, 0, 9,
    12, 0, 10, 12, 0, 4, 4, 8, 0, 14, 18, 0, 52, 0, 16, 32, 0, 4, 4, 8,
    0, 14, 16, 0, 18, 0, 36, 0, 16, 32, 0, 4, 4, 8, 0, 14, 16, 0, 16, 0,
    0, 18, 0, 36, 0, 16, 32, 0, 4, 4, 8, 0, 14, 8, 0, 16, 0, 18, 0, 36,
    0, 16, 32, 0, 16, 0, 4, 8, 0, 16, 0, 0, 18, 0, 36, 0, 16, 128, 0, 8,
    0, 8, 0, 12, 10, 0, 8, 0, 16, 0, 8, 0, 16, 0, 58, 0, 8, 0, 8, 0,
    12, 8, 16, 0, 16, 0, 24, 0, 58, 0, 8, 0, 8, 0, 12, 8, 16, 0, 58, 0,
    8, 0, 8, 0, 12, 8, 24, 0, 16, 0, 16, 0, 9, 4, 58, 0, 4, 58, 0, 4,
    58, 0, 4, 8, 0, 8, 0, 12, 8, 8, 8, 8, 4, 4, 8, 0, 20, 10, 0, 4,
    12, 8, 0, 12, 0, 8, 0, 12, 0, 4, 12, 16, 10, 0, 10, 8, 20, 0, 0, 9,
    8, 0, 28, 0, 0, 8, 0, 12, 132, 0, 16, 32, 0, 4, 18, 0, 8, 0, 8, 0,
    12, 10, 0, 8, 0, 4, 16, 0, 8, 0, 12, 10, 0, 16, 0, 4, 12, 0, 58, 0,
    8, 8, 0, 4, 12, 10, 0, 8, 0, 12, 16, 0, 16, 0, 16, 0, 8, 0, 12, 0,
    4, 8, 8, 0, 16, 0, 8, 0, 16, 0, 4, 12, 0, 9, 8, 0, 62, 0, 128, 0,
    12, 0, 16, 0, 16, 36, 0, 16, 128, 0, 16, 0, 4, 12, 0, 16, 0, 0, 16, 0,
    16, 0, 16, 0, 16, 0, 18, 0, 8, 0, 12, 4, 8, 12, 0, 9, 12, 18, 0, 18,
    0, 16, 0, 16, 0, 132, 0, 16, 56, 0, 8, 0, 12, 12, 0, 12, 0, 9, 12, 0,
    12, 0, 9, 16, 0, 12, 0, 8, 0, 8, 16, 0, 16, 16, 0, 12, 0, 14, 8, 44,
    0, 8, 0, 40, 0, 60, 0, 16, 48, 0, 16, 0, 12, 0, 4, 10, 8, 8, 0, 8,
    0, 8, 0, 8, 9, 8, 4, 12, 26, 0, 8, 9, 12, 0, 4, 12, 30, 0, 0, 8,
    9, 12, 0, 4, 12, 30, 0, 0, 8, 9, 12, 0, 4, 12, 30, 0, 0, 52, 0, 16,
    48, 0, 12, 0, 0, 8, 0, 8, 0, 8, 0, 8, 0, 12, 9, 8, 9, 8, 9, 12,
    0, 9, 12, 4, 18, 0, 12, 16, 0, 20, 0, 0, 9, 8, 9, 12, 0, 9, 12, 0,
    9, 12, 4, 18, 0, 12, 16, 0, 20, 0, 0, 9, 8, 9, 12, 0, 9, 12, 0, 9,
    12, 4, 18, 0, 12, 16, 0, 20, 0, 0, 9, 8, 9, 12, 0, 9, 12, 0, 9, 12,
    4, 18, 0, 12, 16, 0, 52, 0, 16, 80, 0, 4, 8, 0, 8, 0, 12, 9, 8, 9,
    16, 0, 16, 0, 4, 8, 0, 8, 0, 12, 0, 0, 18, 0, 20, 0, 0, 9, 8, 9,
    16, 0, 16, 0, 4, 8, 0, 8, 0, 12, 0, 0, 18, 0, 20, 0, 0, 9, 8, 9,
    16, 0, 16, 0, 4, 8, 0, 8, 0, 12, 0, 0, 18, 0, 20, 0, 0, 9, 8, 9,
    16, 0, 16, 0, 4, 8, 0, 8, 0, 12, 0, 0, 18, 0, 4, 8, 0, 18, 0, 84,
    0, 16, 32, 0, 8, 0, 8, 9, 12, 0, 0, 8, 0, 18, 12, 0, 9, 12, 0, 0,
    8, 0, 18, 12, 0, 9, 12, 0, 0, 8, 0, 18, 12, 0, 9, 12, 0, 0, 8, 0,
    18, 20, 36, 0, 16, 40, 0, 12, 0, 9, 16, 0, 12, 0, 4, 4, 58, 0, 8, 12,
    0, 9, 16, 0, 0, 9, 24, 0, 12, 0, 0, 4, 18, 0, 4, 18, 0, 28, 0, 10,
    8, 0, 12, 0, 4, 10, 18, 0, 20, 0, 4, 10, 8, 24, 0, 4, 4, 18, 0, 4,
    18, 0, 28, 0, 44, 0, 16, 8, 0, 12, 8, 12, 0, 9, 8, 0, 12, 18, 0, 18,
    0, 18, 18, 0, 18, 0, 18, 16, 8, 0, 20, 20, 20, 20, 8, 0, 20, 20, 20, 20,
    10, 32, 0, 8, 0, 8, 9, 12, 10, 8, 0, 8, 8, 0, 30, 0, 0, 8, 0, 8,
    0, 26, 0, 26, 0, 36, 0, 16, 24, 0, 8, 0, 8, 9, 12, 0, 8, 0, 30, 0,
    0, 8, 0, 20, 0, 0, 8, 0, 20, 28, 0, 16, 24, 0, 4, 22, 8, 0, 8, 0,
    8, 8, 0, 4, 11, 0, 8, 0, 9, 12, 0, 10, 0, 8, 0, 9, 16, 0, 12, 0,
    10, 0, 8, 0, 9, 16, 0, 16, 0, 12, 0, 12, 0, 10, 0, 8, 0, 9, 4, 4,
    4, 12, 0, 4, 4, 9, 4, 10, 12, 0, 9, 8, 0, 9, 8, 0, 10, 8, 0, 9,
    8, 0, 12, 0, 10, 0, 8, 0, 11, 0, 4, 4, 8, 0, 8, 8, 0, 16, 0, 10,
    0, 8, 0, 9, 8, 0, 16, 0, 10, 0, 8, 0, 9, 4, 12, 0, 9, 8, 0, 9,
    8, 0, 12, 0, 10, 0, 12, 0, 10, 0, 8, 0, 9, 4, 11, 0, 8, 0, 11, 0,
    8, 0, 8, 10, 8, 0, 9, 4, 9, 32, 0, 16, 0, 4, 4, 18, 0, 36, 0, 10,
    32, 0, 16, 0, 4, 4, 18, 0, 36, 0, 10, 8, 0, 9, 4, 9, 8, 0, 9, 8,
    0, 8, 10, 8, 0, 9, 16, 0, 4, 14, 8, 0, 8, 12, 0, 4, 8, 0, 14, 4,
    8, 0, 12, 0, 28, 0, 16, 24, 0, 18, 0, 4, 24, 16, 0, 0, 4, 4, 8, 0,
    4, 9, 4, 9, 18, 0, 10, 8, 0, 9, 8, 0, 9, 18, 0, 10, 18, 0, 16, 0,
    8, 4, 4, 9, 8, 0, 9, 18, 0, 10, 0, 9, 18, 0, 10, 18, 0, 28, 0, 16,
    48, 0, 8, 0, 8, 16, 0, 0, 12, 0, 11, 0, 16, 0, 16, 0, 0, 12, 0, 8,
    58, 0, 8, 8, 0, 9, 20, 0, 0, 20, 0, 0, 20, 0, 0, 8, 0, 9, 8, 0,
    9, 20, 0, 0, 20, 0, 0, 8, 0, 11, 0, 8, 0, 11, 0, 20, 0, 0, 10, 0,
    16, 0, 16, 0, 0, 12, 0, 8, 58, 0, 8, 8, 0, 9, 20, 0, 0, 20, 0, 0,
    20, 0, 0, 8, 0, 9, 8, 0, 9, 20, 0, 0, 20, 0, 0, 8, 0, 11, 0, 8,
    0, 11, 0, 20, 0, 0, 52, 0, 16, 32, 0, 8, 0, 8, 16, 0, 0, 16, 8, 0,
    9, 4, 8, 0, 9, 4, 22, 8, 0, 4, 8, 0, 36, 0, 16, 16, 0, 16, 0, 16,
    0, 12, 16, 12, 16, 0, 16, 0, 16, 0, 16, 0, 16, 0, 12, 0, 8, 0, 9, 16,
    0, 16, 0, 16, 0, 12, 0, 8, 0, 9, 16, 0, 16, 0, 16, 0, 16, 0, 16, 0,
    16, 0, 16, 0, 12, 16, 48, 0, 16, 0, 8, 0, 8, 16, 0, 0, 16, 0, 16, 0,
    0, 12, 0, 8, 58, 0, 8, 8, 0, 9, 20, 0, 0, 20, 0, 0, 20, 0, 0, 8,
    0, 9, 8, 0, 9, 20, 0, 0, 20, 0, 0, 8, 0, 12, 0, 4, 9, 10, 18, 0,
    9, 18, 0, 10, 12, 4, 18, 0, 12, 8, 12, 0, 18, 0, 8, 0, 9, 8, 0, 9,
    12, 4, 12, 0, 18, 0, 12, 20, 0, 0, 4, 12, 52, 0, 16, 48, 0, 16, 0, 8,
    0, 8, 16, 0, 0, 16, 0, 16, 0, 0, 12, 0, 8, 58, 0, 8, 8, 0, 9, 20,
    0, 0, 20, 0, 0, 20, 0, 0, 8, 0, 9, 8, 0, 9, 20, 0, 0, 20, 0, 0,
    12, 0, 9, 16, 0, 4, 10, 12, 0, 9, 16, 0, 4, 12, 0, 12, 0, 4, 9, 4,
    12, 0, 10, 16, 0, 20, 0, 0, 16, 0, 0, 4, 12, 0, 4, 10, 0, 9, 8, 0,
    0, 16, 0, 0, 8, 18, 0, 10, 8, 0, 0, 8, 18, 0, 12, 0, 8, 18, 0, 8,
    0, 9, 8, 0, 9, 20, 0, 0, 4, 12, 52, 0, 16, 56, 0, 8, 0, 8, 0, 12,
    12, 8, 0, 12, 0, 0, 18, 12, 12, 8, 0, 12, 0, 0, 18, 12, 12, 8, 0, 12,
    0, 0, 18, 12, 12, 8, 0, 12, 0, 0, 18, 60, 0, 16, 12, 0, 9, 12, 4, 18,
    0, 12, 16, 32, 0, 12, 0, 9, 16, 0, 12, 0, 4, 4, 58, 0, 8, 18, 18, 0,
    18, 0, 12, 4, 4, 18, 0, 4, 12, 0, 18, 0, 12, 10, 16, 0, 12, 0, 4, 4,
    58, 0, 8, 18, 0, 18, 0, 18, 0, 18, 0, 12, 4, 4, 18, 0, 12, 18, 0, 18,
    0, 12, 0, 4, 16, 0, 36, 0, 16, 24, 0, 4, 8, 4, 12, 0, 4, 8, 0, 4,
    9, 4, 12, 0, 9, 14, 16, 0, 10, 16, 0, 8, 0, 4, 4, 8, 0, 8, 14, 0,
    8, 14, 0, 28, 0, 16, 24, 0, 4, 8, 4, 12, 0, 4, 22, 8, 0, 14, 4, 9,
    4, 4, 58, 0, 8, 12, 0, 12, 0, 14, 0, 4, 4, 8, 12, 0, 12, 0, 9, 16,
    0, 10, 16, 0, 8, 0, 10, 14, 0, 10, 14, 0, 28, 0, 16, 40, 0, 16, 0, 0,
    9, 12, 0, 9, 16, 0, 10, 8, 0, 12, 0, 4, 14, 0, 4, 12, 0, 4, 9, 4,
    9, 12, 124, 4, 10, 4, 4, 9, 12, 124, 4, 12, 0, 16, 0, 12, 0, 44, 0, 16,
    12, 12, 0, 9, 12, 0, 9, 4, 9, 16, 0, 6, 4, 62, 0, 12, 0, 4, 9, 142,
    4, 8, 0, 12, 16, 24, 0, 4, 12, 0, 12, 0, 4, 9, 4, 12, 0, 10, 18, 0,
    10, 0, 12, 0, 4, 14, 0, 4, 12, 0, 54, 24, 12, 18, 0, 4, 4, 12, 0, 12,
    0, 12, 0, 4, 9, 16, 0, 20, 0, 0, 10, 4, 24, 16, 0, 0, 14, 0, 0, 9,
    16, 0, 10, 16, 0, 0, 9, 12, 0, 12, 0, 9, 12, 0, 9, 12, 0, 9, 16, 0,
    10, 12, 0, 4, 12, 0, 10, 28, 0, 16, 32, 0, 12, 0, 11, 0, 12, 0, 9, 4,
    12, 0, 12, 0, 4, 11, 0, 12, 0, 9, 16, 0, 20, 0, 0, 10, 16, 0, 4, 12,
    0, 8, 0, 14, 0, 16, 0, 0, 10, 12, 0, 11, 0, 4, 12, 0, 12, 0, 9, 4,
    12, 0, 16, 4, 4, 22, 12, 0, 9, 12, 0, 10, 16, 0, 20, 0, 0, 16, 0, 0,
    10, 4, 12, 0, 16, 4, 4, 22, 12, 0, 9, 12, 0, 10, 16, 0, 20, 0, 0, 16,
    0, 0, 4, 12, 0, 4, 10, 0, 9, 8, 0, 0, 16, 0, 0, 8, 10, 8, 0, 0,
    8, 12, 4, 18, 0, 12, 36, 0, 16, 24, 0, 4, 12, 0, 12, 0, 4, 9, 12, 0,
    8, 0, 14, 0, 4, 4, 12, 0, 12, 0, 12, 0, 9, 16, 0, 10, 20, 0, 0, 28,
    0, 16,
];

/// The original's cycles from hunk offset `from` up to `to`.
fn cy(from: u16, to: u16) -> u64 {
    let (a, b) = (((from - CYC_FROM) / 2) as usize, ((to - CYC_FROM) / 2) as usize);
    CYC[a..b].iter().map(|&x| x as u64).sum()
}
