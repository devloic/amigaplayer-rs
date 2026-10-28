//! InStereo! 1.0, ported from NostalgicPlayer's InStereo! 1.0 player
//! (Copyright (c) 2024 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the Eagleplayer "In Stereo 1.0" (Hans Bergstedt, adapted by
//! Eagleeye of DEFECT), which plays the "ISM!V1.2" modules with a replayer of
//! its own through ENPP_Poke* and ENPP_DMAMask. Like the original, the port
//! plays from the module in guest memory, and keeps the replayer's variables,
//! tables, voices and synth buffers in guest memory in the layout of its code
//! hunk (from `$D30` on), so that what it reads past a table, and the voice
//! state it leaves from one InitSound to the next, are the original's.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * a row's notes write their period, length and volume (and a sample's or a
//!   synth's first buffer location) at once, then DMA off and on for the voices
//!   that got one; a sample's loop (or 2 words of an empty buffer) is written
//!   right after the DMA on, and a note `$7F` writes volume 0 straight to Paula
//!   and the empty buffer;
//! * every voice writes its period every tick unless its vibrato runs with no
//!   instrument; the ADSR volume is `(table + 1) * volume >> 7`, unclamped (the
//!   host masks it to 7 bits); the envelope generator counter writes the
//!   buffer it fills, before it fills it, and restarts after its last step
//!   (one more than NostalgicPlayer's);
//! * the vibrato test reads the period table where it means the instrument
//!   (always true), the volume from effect 7 takes sample 7's row argument, the
//!   speed effect sets a copy of the speed the counter does not use, and the
//!   filter effect sets and clears the LED bit itself;
//! * byte and word arithmetic as in the original, nothing range-checked, and
//!   the fourth voice starts with the vibrato delay the Eagleplayer's file holds
//!   ($F9: InitSound clears only 64 bytes of the 80);
//! * the writes come at the original's times, counted on its code (`cy`).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "InStereo! 1.0", detect, new };

/// The first offset of the original's hunk that is mirrored.
const BASE: u32 = 0xd30;
// The replayer's variables (hunk offsets).
const CNT: u32 = 0xd30;
const ROW: u32 = 0xd31;
const SPEED_SHOWN: u32 = 0xd32;
const VOICE_ON: u32 = 0xd33;
const NOTE_SHOWN: u32 = 0xd37;
const SONGPOS: u32 = 0xd3c;
/// The subsong's 14 bytes: 4 unused, speed, rows, first, last and restart.
const SUB: u32 = 0xd3e;
const SPEED: u32 = 0xd42;
const ROWS: u32 = 0xd43;
const LAST: u32 = 0xd46;
const RESTART: u32 = 0xd48;
/// The four voices' position entries (start row, sound and note transpose).
const POS: u32 = 0xd4c;
/// The four voices' rows (note, instrument, arpeggio/effect, argument).
const TRK: u32 = 0xd5c;
// InitSound's pointers into the module.
const P_SAMPLES: u32 = 0xd70;
const P_LENGTHS: u32 = 0xd74;
const P_EGC: u32 = 0xd78;
const P_ADSR: u32 = 0xd7c;
const P_INSTR: u32 = 0xd80;
const P_ARP: u32 = 0xd84;
const P_SUB: u32 = 0xd88;
const P_WAVES: u32 = 0xd8c;
const P_POSITIONS: u32 = 0xd90;
const P_TRACKS: u32 = 0xd94;
const P_DATA: u32 = 0xd98;
const P_MODULE: u32 = 0xda0;
// The pointers to its own buffers, then the buffers.
const P_BUF1: u32 = 0xda4;
const P_BUF2: u32 = 0xda8;
const P_FLAGS: u32 = 0xdac;
const P_VOICES: u32 = 0xdb0;
const P_PERIODS: u32 = 0xdc0;
const VIBRATO: u32 = 0xdc4;
const BUF1: u32 = 0xec4;
const BUF2: u32 = 0x12c4;
/// 16 bytes of zeros: a voice that stops plays them.
const EMPTY: u32 = 0x16c4;
/// The synth voices' buffer flags.
const FLAGS: u32 = 0x16d4;
const VOICES: u32 = 0x16d8;
const PERIODS_AT: u32 = 0x1728;
/// The hunk ends with the period table; reads past it get zeros.
const END: u32 = 0x1804 + 0x200;

// A voice's 20 bytes.
const V_NOTE: u32 = 0x0;
const V_PREV: u32 = 0x1;
const V_SLIDE_SPEED: u32 = 0x2;
const V_INSTR: u32 = 0x3;
const V_VOLUME: u32 = 0x4;
const V_ADSR_ON: u32 = 0x5;
const V_ADSR_POS: u32 = 0x6;
const V_VIB_DELAY: u32 = 0x8;
const V_VIB_POS: u32 = 0x9;
const V_EGC_MODE: u32 = 0xb;
const V_EGC_POS: u32 = 0xc;
const V_SLIDE: u32 = 0xe;
const V_PORT_ON: u32 = 0x10;
const V_PORT_SPEED: u32 = 0x11;
const V_PORT_COUNT: u32 = 0x12;

// An instrument's 28 bytes.
const I_WAVE: u32 = 0x0;
const I_SYNTH: u32 = 0x1;
const I_LENGTH: u32 = 0x2;
const I_REPEAT: u32 = 0x4;
const I_VOLUME: u32 = 0x6;
const I_PORT_SPEED: u32 = 0x7;
const I_ADSR_ON: u32 = 0x8;
const I_ADSR_TABLE: u32 = 0x9;
const I_ADSR_LEN: u32 = 0xa;
const I_PORT_ON: u32 = 0xe;
const I_VIB_DELAY: u32 = 0x14;
const I_VIB_SPEED: u32 = 0x15;
const I_VIB_LEVEL: u32 = 0x16;
const I_EGC_OFFSET: u32 = 0x17;
const I_EGC_MODE: u32 = 0x18;
const I_EGC_TABLE: u32 = 0x19;
const I_EGC_LEN: u32 = 0x1a;

/// The period table (NostalgicPlayer's Tables.cs, the original's to the word).
const PERIODS: [u16; 109] = [
    0, 13696, 12928, 12192, 11520, 10848, 10240, 9664, 9120, 8608, 8128, 7680, 7248, 6848, 6464, 6096,
    5760, 5424, 5120, 4832, 4560, 4304, 4064, 3840, 3624, 3424, 3232, 3048, 2880, 2712, 2560, 2416,
    2280, 2152, 2032, 1920, 1812, 1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960,
    906, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320,
    302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 107,
    101, 95, 90, 85, 80, 75, 71, 67, 63, 60, 56, 53, 50, 47, 45, 42, 40, 37, 35, 33, 31, 30, 28,
];

/// Colour clocks from the open replayer's start to the reference's InitSound:
/// the Eagleplayer's Check, InitPlayer, SubSongRange, module info and amplifier
/// (measured), and the module info's sum of the sample lengths (24 cycles a
/// sample).
const INIT_WAIT: u64 = 5067;
const INIT_WAIT_SAMPLE: u64 = 12;

fn detect(d: &[u8]) -> bool {
    d.len() >= 8 && &d[..8] == b"ISM!V1.2"
}

pub struct InStereo1 {
    base: u32,
    /// The original's hunk from `BASE` on, in guest memory.
    mem: u32,
    sub: u16,
    last_sub: u16,
    songend: bool,
    title: String,
    /// The bus clock and the 68000 cycles counted since the last host call, as
    /// the emulator times guest code (`cy`).
    epoch: u64,
    cycles: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if d.len() < 0xcc {
        return None;
    }
    let size = END - BASE;
    let top = std::mem::replace(&mut m.alloc_top, true);
    let mem = m.alloc(size);
    m.alloc_top = top;
    if mem == 0 {
        return None;
    }
    let mut r = InStereo1 {
        base,
        mem,
        sub: 0,
        last_sub: (d[0x13] as u16).saturating_sub(1),
        songend: false,
        title: d[0x24..0x40].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect(),
        epoch: 0,
        cycles: 0,
    };
    let b = &mut m.bus;
    for k in 0..size {
        b.w8(mem + k, 0);
    }
    // what the Eagleplayer's file holds: every voice on, the buffer flags of
    // the voices, the fourth voice's vibrato delay past what InitSound clears
    for v in 0..4 {
        b.w8(r.a(VOICE_ON + v), 0xff);
        b.w8(r.a(NOTE_SHOWN + v), 8);
        b.w8(r.a(FLAGS + v), (v < 3) as u8);
    }
    b.w8(r.a(VOICES + 3 * 20 + V_VIB_DELAY), 0xf9);
    b.w32(r.a(P_BUF1), r.a(BUF1));
    b.w32(r.a(P_BUF2), r.a(BUF2));
    b.w32(r.a(P_FLAGS), r.a(FLAGS));
    for v in 0..4 {
        b.w32(r.a(P_VOICES + 4 * v), r.a(VOICES + 20 * v));
    }
    b.w32(r.a(P_PERIODS), r.a(PERIODS_AT));
    for (k, &p) in PERIODS.iter().enumerate() {
        b.w16(r.a(PERIODS_AT) + 2 * k as u32, p);
    }
    // InitPlayer keeps the module's address
    b.w32(r.a(P_MODULE), base);
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(INIT_WAIT + INIT_WAIT_SAMPLE * d[0x10] as u64);
    r.init_sound(0, &mut hw);
    Some(Box::new(r))
}

impl Replayer for InStereo1 {
    fn format(&self) -> &'static str {
        "InStereo! 1.0"
    }

    fn title(&self) -> String {
        self.title.clone()
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
        // the interrupt: registers saved and cleared, then the play routine
        self.c(0x420, 0x428);
        self.c(0x4de, 0x4fe);
        self.c(0x428, 0x42c);
        self.c(0x610, 0x61e);
        let cnt = self.r8(hw, CNT).wrapping_add(1);
        self.w8(hw, CNT, cnt);
        if cnt as i8 >= self.r8(hw, SPEED) as i8 {
            self.row(hw);
        }
        self.c(0x620, 0x624);
        self.c(0xaf8, 0xb0c);
        self.effects(hw, 0);
        self.c(0xb0c, 0xb20);
        self.effects(hw, 1);
        self.c(0xb20, 0xb34);
        self.effects(hw, 2);
        self.c(0xb34, 0xb46);
        self.effects(hw, 3);
    }
}

impl InStereo1 {
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
        self.epoch = hw.bus.clock;
        self.cycles = 0;
        // the stub's RTS
        self.cycles += 16;
    }

    fn ch(v: u32) -> usize {
        v as usize
    }

    fn poke_adr(&mut self, hw: &mut Hw, v: u32, a: u32) {
        self.c(0x434, 0x44a);
        self.sync(hw);
        hw.loc(Self::ch(v), a);
        self.host(hw);
        self.c(0x44a, 0x450);
    }

    fn poke_len(&mut self, hw: &mut Hw, v: u32, n: u32) {
        self.c(0x450, 0x46c);
        self.sync(hw);
        hw.len(Self::ch(v), n as u16);
        self.host(hw);
        self.c(0x46c, 0x472);
    }

    fn poke_per(&mut self, hw: &mut Hw, v: u32, p: u32) {
        self.c(0x472, 0x48c);
        self.sync(hw);
        hw.per(Self::ch(v), p as u16);
        self.host(hw);
        self.c(0x48c, 0x492);
    }

    fn poke_vol(&mut self, hw: &mut Hw, v: u32, x: u32) {
        self.c(0x492, 0x4aa);
        self.sync(hw);
        hw.vol(Self::ch(v), x as u16 & 0x7f);
        self.host(hw);
        self.c(0x4aa, 0x4b0);
    }

    /// ENPP_DMAMask with d5: its sign bit switches the voices on (and the host
    /// waits for the DMA as dtg_WaitAudioDMA does).
    fn dma_mask(&mut self, hw: &mut Hw, d5: u16) {
        self.c(0x4b0, 0x4ca);
        self.sync(hw);
        let v = (d5 & 0xf) | (d5 & 0x8000);
        hw.dmacon(v);
        if v & 0x8000 != 0 {
            let now = hw.bus.clock;
            hw.bus.clock = (now / LINE + 10) * LINE;
        }
        self.host(hw);
        self.c(0x4ca, 0x4d0);
    }

    /// The instrument's volume (the routine at $4D0).
    fn instr_vol(&mut self, hw: &mut Hw, v: u32, a1: u32) {
        self.c(0x4d0, 0x4da);
        let x = hw.bus.r8(a1 + I_VOLUME) as u32;
        self.poke_vol(hw, v, x);
        self.c(0x4da, 0x4de);
    }

    /// InitSound: the vibrato table, the pointers into the module, the voices
    /// cleared (64 of their 80 bytes) and subsong `n`'s speed and positions.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        let mut p = self.a(VIBRATO);
        let mut x = 0u8;
        for _ in 0..64 {
            hw.bus.w8(p, x);
            p += 1;
            x = x.wrapping_add(2);
        }
        x = x.wrapping_add(1);
        for _ in 0..128 {
            x = x.wrapping_sub(2);
            hw.bus.w8(p, x);
            p += 1;
        }
        x = x.wrapping_sub(1);
        for _ in 0..64 {
            hw.bus.w8(p, x);
            p += 1;
            x = x.wrapping_add(2);
        }
        let m = self.base;
        let b = |o: u32| hw.bus.r8(m + o) as u32;
        let (nsamp, nwave, ninstr, nsub, negc, nadsr) = (b(0x10), b(0x11), b(0x12), b(0x13), b(0x14), b(0x15));
        let (npos, nrows) = (hw.bus.r16(m + 8) as u32, hw.bus.r16(m + 10) as u32);
        let mut a = m + 0xcc;
        let mut ptrs = [0u32; 11];
        ptrs[0] = a;
        a = a.wrapping_add(nsamp * 0x1c);
        ptrs[1] = a;
        a = a.wrapping_add(nsamp << 2);
        ptrs[2] = a;
        a = a.wrapping_add(negc << 7);
        ptrs[3] = a;
        a = a.wrapping_add(nadsr << 8);
        ptrs[4] = a;
        a = a.wrapping_add(ninstr * 0x1c);
        ptrs[5] = a;
        a = a.wrapping_add(0x100);
        ptrs[6] = a;
        a = a.wrapping_add((nsub + 1) * 0xe);
        ptrs[7] = a;
        a = a.wrapping_add(nwave << 8);
        ptrs[8] = a;
        a = a.wrapping_add(npos << 4);
        ptrs[9] = a;
        a = a.wrapping_add((nrows + 0x40) << 2);
        ptrs[10] = a;
        for (k, &p) in ptrs.iter().enumerate() {
            self.w32(hw, P_SAMPLES + 4 * k as u32, p);
        }
        for k in 0..64 {
            self.w8(hw, VOICES + k, 0);
        }
        let s = self.r32(hw, P_SUB).wrapping_add(n as u32 * 0xe);
        for k in 0..14 {
            let v = hw.bus.r8(s + k);
            self.w8(hw, SUB + k, v);
        }
        let speed = self.r8(hw, SPEED);
        self.w8(hw, SPEED_SHOWN, speed);
        let first = self.r16(hw, SUB + 6);
        self.w16(hw, SONGPOS, first);
        self.w8(hw, CNT, speed);
        let rows = self.r8(hw, ROWS);
        self.w8(hw, ROW, rows);
        // its time, from the registers cleared to the RTS
        let mut t = cy(0x4fe, 0x508) + cy(0x4e0, 0x4fe) + cy(0x508, 0x510) + cy(0x518, 0x51c);
        t += cy(0x510, 0x518) * 64 + cy(0x51c, 0x524) * 128 + cy(0x524, 0x528) + cy(0x528, 0x530) * 64;
        t += cy(0x530, 0x5d0) + cy(0x5d0, 0x5d6) * 64 + cy(0x5d6, 0x5e6) + cy(0x5e6, 0x5ec) * 14 + cy(0x5ec, 0x610);
        hw.wait(t / 2);
    }

    /// A row (the routine at $624): a new position after the last row, the
    /// voices' rows, their DMA off and on, then their loops.
    fn row(&mut self, hw: &mut Hw) {
        self.c(0x61e, 0x620);
        self.w8(hw, CNT, 0);
        self.c(0x624, 0x630);
        let mut d0: u32 = 0;
        if self.r8(hw, ROW) as i8 >= self.r8(hw, ROWS) as i8 {
            self.c(0x630, 0x642);
            self.w8(hw, ROW, 0);
            if self.r16(hw, SONGPOS) as i16 > self.r16(hw, LAST) as i16 {
                self.c(0x642, 0x64c);
                let r = self.r16(hw, RESTART);
                self.w16(hw, SONGPOS, r);
                // dtg_SongEnd
                self.sync(hw);
                self.songend = true;
                self.host(hw);
            }
            self.c(0x64c, 0x676);
            let pos = self.r16(hw, SONGPOS).wrapping_add(1);
            self.w16(hw, SONGPOS, pos);
            d0 = ((pos as i16 as i32 - 1) << 4) as u32;
            let p = self.r32(hw, P_POSITIONS).wrapping_add(d0);
            for k in 0..16 {
                let v = hw.bus.r8(p + k);
                self.w8(hw, POS + k, v);
            }
        }
        self.c(0x676, 0x6ce);
        let d1 = (self.r8(hw, ROW) as u32) << 2;
        for v in 0..4 {
            d0 = ((d0 & 0xffff_0000) | self.r16(hw, POS + 4 * v) as u32) << 2;
            let t = self.r32(hw, P_TRACKS).wrapping_add(d0).wrapping_add(d1 as u16 as i16 as u32);
            let x = hw.bus.r32(t);
            self.w32(hw, TRK + 4 * v, x);
        }
        let mut d5 = 0u16;
        self.c(0x6ce, 0x6e0);
        self.c(0x6e0, 0x6f0);
        self.row_voice(hw, 0, &mut d5);
        self.c(0x6f0, 0x6fe);
        self.c(0x6fe, 0x70e);
        self.row_voice(hw, 1, &mut d5);
        self.c(0x70e, 0x71c);
        self.c(0x71c, 0x72c);
        self.row_voice(hw, 2, &mut d5);
        self.c(0x72c, 0x73a);
        self.c(0x73a, 0x74a);
        self.row_voice(hw, 3, &mut d5);
        self.c(0x74a, 0x74e);
        if d5 != 0 {
            self.c(0x74e, 0x752);
            self.dma_mask(hw, d5);
            self.c(0x752, 0x756);
            self.c(0xd2e, 0xd30);
            d5 |= 0x8200;
            self.c(0x756, 0x75e);
            self.dma_mask(hw, d5);
        }
        self.c(0x75e, 0x768);
        let row = self.r8(hw, ROW).wrapping_add(1);
        self.w8(hw, ROW, row);
        self.c(0xd2e, 0xd30);
        self.c(0x768, 0x776);
        let d6 = d5 & 0xf;
        self.c(0x776, 0x77c);
        if d6 & 1 != 0 {
            self.c(0x77c, 0x78a);
            self.loop_voice(hw, 0);
        }
        self.c(0x78a, 0x792);
        self.c(0x792, 0x798);
        if d6 & 2 != 0 {
            self.c(0x798, 0x7a6);
            self.loop_voice(hw, 1);
        }
        self.c(0x7a6, 0x7ae);
        self.c(0x7ae, 0x7b4);
        if d6 & 4 != 0 {
            self.c(0x7b4, 0x7c2);
            self.loop_voice(hw, 2);
        }
        self.c(0x7c2, 0x7cc);
        self.c(0x7cc, 0x7d4);
        if d6 & 8 != 0 {
            self.c(0x7d4, 0x7e2);
            self.loop_voice(hw, 3);
        } else {
            self.c(0x9d8, 0x9da);
        }
    }

    /// Marks the voice's note for the display (the loop at $8FE or $940).
    fn shown(&mut self, hw: &mut Hw, v: u32, at: u16) {
        self.cn(at, at + 8, v);
        self.c(at, at + 4);
        self.w8(hw, NOTE_SHOWN + v, 8);
    }

    /// A voice's row (the routine at $7E2): its effect, then its note.
    fn row_voice(&mut self, hw: &mut Hw, v: u32, d5: &mut u16) {
        let a4 = VOICES + 20 * v;
        let d0 = self.r32(hw, TRK + 4 * v);
        let d3 = self.r32(hw, POS + 4 * v);
        let d6 = 1u16 << v;
        let fx = ((d0 & 0xfff) >> 8) as u8;
        let arg = d0 as u8;
        self.c(0x7e2, 0x7fa);
        self.w8(hw, a4 + V_VOLUME, 0);
        self.w8(hw, a4 + V_SLIDE_SPEED, 0);
        if fx != 0 {
            self.c(0x7fa, 0x800);
            if fx == 1 {
                self.c(0x800, 0x804);
                self.w8(hw, a4 + V_SLIDE_SPEED, arg);
            }
            self.c(0x804, 0x80a);
            if fx == 2 {
                self.c(0x80a, 0x818);
                self.w8(hw, a4 + V_ADSR_POS, 0);
                self.w8(hw, a4 + V_ADSR_POS + 1, arg);
                self.w8(hw, a4 + V_ADSR_ON, 1);
            }
            self.c(0x818, 0x81e);
            if fx == 3 {
                self.c(0x81e, 0x82c);
                self.w8(hw, a4 + V_EGC_POS, 0);
                self.w8(hw, a4 + V_EGC_POS + 1, arg);
                self.w8(hw, a4 + V_EGC_MODE, 1);
            }
            self.c(0x82c, 0x832);
            if fx == 4 {
                self.c(0x832, 0x83a);
                self.w16(hw, a4 + V_SLIDE, arg as i8 as u16);
            }
            self.c(0x83a, 0x840);
            if fx == 5 {
                self.c(0x840, 0x848);
                self.w8(hw, a4 + V_VIB_DELAY, arg);
                self.w8(hw, a4 + V_VIB_POS, 0);
            }
            self.c(0x848, 0x84e);
            if fx == 6 {
                self.c(0x84e, 0x852);
                self.w8(hw, a4 + V_VIB_POS, arg);
            }
            self.c(0x852, 0x858);
            if fx == 7 {
                self.c(0x858, 0x860);
                self.w8(hw, a4 + V_VOLUME, arg & 0x3f);
            }
            self.c(0x860, 0x866);
            if fx == 0xa {
                self.c(0x866, 0x86a);
                if arg as i8 > 0 {
                    self.c(0x86a, 0x870);
                    if arg <= 0x40 {
                        self.c(0x870, 0x876);
                        self.w8(hw, ROWS, arg);
                    }
                }
            }
            // C and D load a register nothing reads
            self.c(0x876, 0x87c);
            if fx == 0xc {
                self.c(0x87c, 0x882);
            }
            self.c(0x882, 0x888);
            if fx == 0xd {
                self.c(0x888, 0x88c);
            }
            self.c(0x88c, 0x892);
            if fx == 0xe {
                self.c(0x892, 0x896);
                // bclr/bset #1,$BFE001: the LED filter, on with argument 0
                let led = hw.bus.read_byte(0xbfe001);
                if arg == 0 {
                    self.c(0x896, 0x89e);
                    self.sync(hw);
                    hw.bus.write_byte(0xbfe001, led & !2);
                    self.c(0x89e, 0x8a0);
                } else {
                    self.c(0x8a0, 0x8a8);
                    self.sync(hw);
                    hw.bus.write_byte(0xbfe001, led | 2);
                }
            }
            self.c(0x8a8, 0x8ae);
            if fx == 0xf {
                self.c(0x8ae, 0x8b2);
                if arg as i8 > 0 {
                    self.c(0x8b2, 0x8b8);
                    if arg <= 0x10 {
                        self.c(0x8b8, 0x8be);
                        self.w8(hw, SPEED_SHOWN, arg);
                    }
                }
            }
        }
        self.c(0x8be, 0x8ce);
        let note = (d0 >> 24) as u8;
        if note == 0 {
            self.c(0x9d8, 0x9da);
            return;
        }
        self.c(0x8ce, 0x8d6);
        if note == 0x7f {
            // volume 0 straight to Paula (clr.w 8(a5)), then the empty buffer
            self.c(0x9e6, 0x9ea);
            self.sync(hw);
            hw.vol(v as usize, 0);
            self.w8(hw, a4 + V_VOLUME, 0);
            *d5 |= d6;
            self.c(0x9ea, 0x9f2);
            self.empty(hw, v);
            return;
        }
        self.c(0x8d6, 0x8dc);
        let mut n = note;
        if fx != 8 {
            self.c(0x8dc, 0x8de);
            n = n.wrapping_add(d3 as u8);
        }
        self.c(0x8de, 0x8ee);
        let prev = self.r8(hw, a4 + V_NOTE);
        self.w8(hw, a4 + V_PREV, prev);
        self.w8(hw, a4 + V_NOTE, n);
        let per = self.r16(hw, PERIODS_AT + 2 * n as u32);
        self.c(0x8ee, 0x8f2);
        self.poke_per(hw, v, per as u32);
        self.c(0x8f2, 0x8f8);
        let mut ins = (d0 >> 16) as u8;
        if ins == 0 {
            self.c(0x8f8, 0x8fe);
            self.shown(hw, v, 0x8fe);
            self.c(0x906, 0x912);
            return;
        }
        self.c(0x912, 0x926);
        if fx != 9 {
            self.c(0x926, 0x928);
            ins = ins.wrapping_add((d3 >> 8) as u8);
        }
        self.c(0x928, 0x93a);
        self.w8(hw, a4 + V_INSTR, ins);
        let a1 = self.r32(hw, P_INSTR).wrapping_add((ins as u32).wrapping_sub(1) as u16 as u32 * 0x1c);
        self.c(0x93a, 0x940);
        self.shown(hw, v, 0x940);
        self.c(0x948, 0x97e);
        self.w8(hw, a4 + V_ADSR_ON, 0);
        self.w16(hw, a4 + V_ADSR_POS, 0);
        self.w32(hw, a4 + V_VIB_DELAY, 0);
        self.w32(hw, a4 + V_EGC_POS, 0);
        let m = |o: u32| hw.bus.r8(a1 + o);
        let (port_speed, port_on) = (m(I_PORT_SPEED), m(I_PORT_ON));
        self.w8(hw, a4 + V_PORT_SPEED, port_speed);
        self.w8(hw, a4 + V_PORT_ON, port_on);
        self.w16(hw, a4 + V_PORT_COUNT, port_speed as i8 as u16);
        if fx == 0xb {
            self.c(0x97e, 0x982);
            self.w32(hw, a4 + V_PORT_ON, 0);
        }
        self.c(0x982, 0x98e);
        let vib = hw.bus.r8(a1 + I_VIB_DELAY);
        self.w8(hw, a4 + V_VIB_DELAY, vib);
        if hw.bus.r8(a1 + I_ADSR_ON) != 0 {
            self.c(0x98e, 0x994);
            self.w8(hw, a4 + V_ADSR_ON, 1);
        }
        self.c(0x994, 0x99c);
        if hw.bus.r8(a1 + I_SYNTH) != 0 {
            self.synth_note(hw, v, a1, d5);
            return;
        }
        // a sample: its data past the lengths of the ones before
        self.c(0x99c, 0x9ae);
        let s = hw.bus.r8(a1 + I_WAVE) & 0x3f;
        let mut a0 = self.r32(hw, P_DATA);
        let mut a2 = self.r32(hw, P_LENGTHS);
        for _ in 0..s {
            a0 = a0.wrapping_add(hw.bus.r32(a2));
            a2 = a2.wrapping_add(4);
        }
        self.cn(0x9ae, 0x9b8, s as u32);
        self.c(0x9ae, 0x9b2);
        self.c(0x9b8, 0x9bc);
        self.poke_adr(hw, v, a0);
        self.c(0x9bc, 0x9c6);
        let len = (hw.bus.r16(a1 + I_LENGTH) >> 1) as u32;
        self.poke_len(hw, v, len);
        self.c(0x9c6, 0x9ca);
        self.instr_vol(hw, v, a1);
        self.c(0x9ca, 0x9d0);
        if s == 7 {
            self.c(0x9da, 0x9e6);
            self.w8(hw, a4 + V_VOLUME, arg & 0x3f);
        } else {
            self.c(0x9d0, 0x9da);
            let x = hw.bus.r8(a1 + I_VOLUME);
            self.w8(hw, a4 + V_VOLUME, x);
        }
        *d5 |= d6;
    }

    /// A synth note (the code at $A70): its first buffer, unless the envelope
    /// generator counter writes it, its length and volume.
    fn synth_note(&mut self, hw: &mut Hw, v: u32, a1: u32, d5: &mut u16) {
        self.c(0xa70, 0xa9c);
        let wave = self.r32(hw, P_WAVES).wrapping_add((hw.bus.r8(a1 + I_WAVE) as u32) << 8);
        self.w8(hw, FLAGS + v, 1);
        let mode = hw.bus.r8(a1 + I_EGC_MODE);
        self.w8(hw, VOICES + 20 * v + V_EGC_MODE, mode);
        if mode == 0 {
            self.c(0xa9c, 0xab4);
            let buf = self.a(BUF1) + (v << 8);
            self.poke_adr(hw, v, buf);
            self.c(0xab4, 0xabe);
            let mut n = hw.bus.r16(a1 + I_LENGTH) >> 2;
            if n == 0 {
                self.c(0xabe, 0xac0);
                n = 1;
            }
            self.c(0xac0, 0xac8);
            let n = (n.wrapping_sub(1) & 0x3f) as u32 + 1;
            for k in 0..n {
                let x = hw.bus.r32(wave + 4 * k);
                hw.bus.w32(buf + 4 * k, x);
            }
            self.cn(0xac8, 0xace, n);
            self.c(0xace, 0xad8);
            let neg = hw.bus.r8(a1 + I_EGC_OFFSET) as u32;
            if neg != 0 {
                self.c(0xad8, 0xada);
                for k in 0..neg {
                    let x = hw.bus.r8(buf + k);
                    hw.bus.w8(buf + k, x.wrapping_neg());
                }
                self.cn(0xada, 0xae0, neg);
            }
        }
        self.c(0xae0, 0xaea);
        let len = (hw.bus.r16(a1 + I_LENGTH) >> 1) as u32;
        self.poke_len(hw, v, len);
        self.c(0xaea, 0xaee);
        self.instr_vol(hw, v, a1);
        self.c(0xaee, 0xaf8);
        let x = hw.bus.r8(a1 + I_VOLUME);
        self.w8(hw, VOICES + 20 * v + V_VOLUME, x);
        *d5 |= 1 << v;
    }

    /// The empty buffer, 2 words (the routine at $A58).
    fn empty(&mut self, hw: &mut Hw, v: u32) {
        self.c(0xa58, 0xa64);
        let e = self.a(EMPTY);
        self.poke_adr(hw, v, e);
        self.c(0xa64, 0xa6a);
        self.poke_len(hw, v, 2);
        self.c(0xa6a, 0xa70);
    }

    /// A voice's loop after its note's DMA on (the routine at $9F2): a sample's
    /// repeat, the empty buffer for a repeat of 2, nothing for 0 or a synth.
    fn loop_voice(&mut self, hw: &mut Hw, v: u32) {
        let ins = self.r8(hw, VOICES + 20 * v + V_INSTR);
        let a0 = self.r32(hw, P_INSTR).wrapping_add(ins.wrapping_sub(1) as u32 * 0x1c);
        self.c(0x9f2, 0xa0a);
        if hw.bus.r8(a0 + I_SYNTH) != 0 {
            self.c(0x9d8, 0x9da);
            return;
        }
        self.c(0xa0a, 0xa10);
        let rep = hw.bus.r16(a0 + I_REPEAT);
        if rep == 0 {
            self.c(0x9d8, 0x9da);
            return;
        }
        self.c(0xa10, 0xa18);
        if rep == 2 {
            self.empty(hw, v);
            return;
        }
        self.c(0xa18, 0xa2a);
        let s = hw.bus.r8(a0 + I_WAVE) & 0x3f;
        let mut a1 = self.r32(hw, P_DATA);
        let mut a2 = self.r32(hw, P_LENGTHS);
        for _ in 0..s {
            a1 = a1.wrapping_add(hw.bus.r32(a2));
            a2 = a2.wrapping_add(4);
        }
        self.cn(0xa2a, 0xa34, s as u32);
        self.c(0xa2a, 0xa2e);
        self.c(0xa34, 0xa44);
        a1 = a1.wrapping_add(hw.bus.r16(a0 + I_LENGTH) as u32);
        self.poke_adr(hw, v, a1);
        self.c(0xa44, 0xa54);
        self.poke_len(hw, v, (rep >> 1) as u32);
        self.c(0xa54, 0xa58);
    }

    /// A voice's effects every tick (the routine at $B46): arpeggio,
    /// portamento, vibrato and slide into the period, the ADSR volume, and the
    /// envelope generator counter's buffer.
    fn effects(&mut self, hw: &mut Hw, v: u32) {
        let a4 = VOICES + 20 * v;
        let d0 = self.r32(hw, TRK + 4 * v);
        self.c(0xb46, 0xb80);
        let arp = ((d0 >> 12) & 0xf) << 4;
        let idx = (arp as u8).wrapping_add(self.r8(hw, CNT)) as u32;
        let d4 = hw.bus.r8(self.r32(hw, P_ARP).wrapping_add(idx));
        let d1 = self.r8(hw, a4 + V_NOTE).wrapping_add(d4) as u32;
        let d7 = self.r8(hw, a4 + V_PREV).wrapping_add(d4) as u32;
        let mut d2: u32 = self.r16(hw, PERIODS_AT + 2 * d1) as u32;
        let d4w = self.r16(hw, PERIODS_AT + 2 * d7);
        if self.r8(hw, a4 + V_PORT_ON) != 0 {
            self.c(0xb80, 0xb86);
            if self.r16(hw, a4 + V_PORT_COUNT) != 0 {
                self.c(0xb86, 0xb8a);
                if d2 as u16 != d4w {
                    self.c(0xb8a, 0xba0);
                    let cnt = self.r16(hw, a4 + V_PORT_COUNT).wrapping_sub(1);
                    self.w16(hw, a4 + V_PORT_COUNT, cnt);
                    // exg: the previous period, less the current one, times the counter
                    let cur = d2 as u16;
                    let diff = d4w.wrapping_sub(cur) as i16 as i32;
                    let prod = diff.wrapping_mul(cnt as i16 as i32);
                    d2 = prod as u32;
                    let sp = self.r8(hw, a4 + V_PORT_SPEED) as i8 as i32;
                    if sp != 0 {
                        self.c(0xba0, 0xba2);
                        d2 = divs(d2, sp as i16);
                    }
                    self.c(0xba2, 0xba4);
                    d2 = (d2 & 0xffff_0000) | (d2 as u16).wrapping_add(cur) as u32;
                }
            }
        }
        // tst.b $16(a0) with a0 at the period table
        self.c(0xba4, 0xbaa);
        if self.r8(hw, PERIODS_AT + 0x16) != 0 {
            self.c(0xbaa, 0xbb0);
            let delay = self.r8(hw, a4 + V_VIB_DELAY);
            if delay != 0 {
                self.c(0xbb0, 0xbb6);
                self.w8(hw, a4 + V_VIB_DELAY, delay.wrapping_sub(1));
            } else {
                self.c(0xbb6, 0xbd8);
                let vib = self.r8(hw, VIBRATO + self.r8(hw, a4 + V_VIB_POS) as u32);
                let ins = self.r8(hw, a4 + V_INSTR);
                if ins == 0 {
                    // no instrument: no period, no ADSR
                    self.egc(hw, v, PERIODS_AT, true);
                    return;
                }
                self.c(0xbd8, 0xbf0);
                let a0 = self.r32(hw, P_INSTR).wrapping_add(ins.wrapping_sub(1) as u32 * 0x1c);
                let level = hw.bus.r8(a0 + I_VIB_LEVEL) as u32;
                if vib as i8 >= 0 {
                    self.c(0xbf0, 0xbf6);
                    if level != 0 {
                        self.c(0xbf6, 0xbfc);
                        let q = ((vib as u32) << 2) / level;
                        d2 = (d2 & 0xffff_0000) | (d2 as u16).wrapping_add(q as u16) as u32;
                    } else {
                        self.c(0xbfc, 0xc00);
                        d2 = 0;
                    }
                } else {
                    self.c(0xc00, 0xc06);
                    if level != 0 {
                        self.c(0xc06, 0xc0c);
                        let q = ((vib.wrapping_neg() as u32) << 2) / level;
                        d2 = (d2 & 0xffff_0000) | (d2 as u16).wrapping_sub(q as u16) as u32;
                    }
                }
                self.c(0xc0c, 0xc14);
                let pos = self.r8(hw, a4 + V_VIB_POS).wrapping_add(hw.bus.r8(a0 + I_VIB_SPEED));
                self.w8(hw, a4 + V_VIB_POS, pos);
            }
        }
        self.c(0xc14, 0xc1c);
        d2 = (d2 as u16).wrapping_add(self.r16(hw, a4 + V_SLIDE)) as u32;
        self.poke_per(hw, v, d2);
        self.c(0xc1c, 0xc3e);
        let slide = self.r16(hw, a4 + V_SLIDE).wrapping_sub(self.r8(hw, a4 + V_SLIDE_SPEED) as i8 as u16);
        self.w16(hw, a4 + V_SLIDE, slide);
        let ins = self.r8(hw, a4 + V_INSTR);
        let a0 = self.r32(hw, P_INSTR).wrapping_add(ins.wrapping_sub(1) as u32 * 0x1c);
        if self.r8(hw, a4 + V_ADSR_ON) != 0 {
            self.c(0xc3e, 0xc4a);
            let pos = self.r16(hw, a4 + V_ADSR_POS);
            if pos as i16 >= hw.bus.r16(a0 + I_ADSR_LEN) as i16 {
                self.c(0xc4a, 0xc4e);
                self.w8(hw, a4 + V_ADSR_ON, 0);
            }
            self.c(0xc4e, 0xc7c);
            let t = self.r32(hw, P_ADSR)
                .wrapping_add((hw.bus.r8(a0 + I_ADSR_TABLE) as u32) << 8)
                .wrapping_add(pos as u32);
            let x = (hw.bus.r8(t) as u32 + 1) * hw.bus.r8(a0 + I_VOLUME) as u32 >> 7;
            self.w16(hw, a4 + V_ADSR_POS, pos.wrapping_add(1));
            self.poke_vol(hw, v, x);
        }
        self.egc(hw, v, a0, false);
    }

    /// The envelope generator counter (the code at $C7C), with `a0` at the
    /// instrument (or where the vibrato left it): the next buffer gets the
    /// waveform, its first bytes negated by the counter's table.
    fn egc(&mut self, hw: &mut Hw, v: u32, a0: u32, a0_mirror: bool) {
        let a4 = VOICES + 20 * v;
        self.c(0xc7c, 0xc84);
        let mode = self.r8(hw, a4 + V_EGC_MODE);
        if mode == 0 {
            self.c(0xd2c, 0xd2e);
            return;
        }
        // a0 is a guest address, or a hunk offset in the mirror
        let a0 = if a0_mirror { self.a(a0) } else { a0 };
        self.c(0xc84, 0xca8);
        let wave = self.r32(hw, P_WAVES).wrapping_add((hw.bus.r8(a0) as u32) << 8);
        let f = self.r8(hw, FLAGS + v) ^ 1;
        self.w8(hw, FLAGS + v, f);
        let (fill, _) = if f == 0 {
            self.c(0xca8, 0xcb8);
            (self.a(BUF2) + (v << 8), ())
        } else {
            self.c(0xcb8, 0xcc6);
            (self.a(BUF1) + (v << 8), ())
        };
        self.c(0xcc6, 0xcce);
        self.poke_adr(hw, v, fill);
        self.c(0xcce, 0xcdc);
        let n = (hw.bus.r16(a0 + I_LENGTH).wrapping_sub(1) & 0xff) as u32 + 1;
        for k in 0..n {
            let x = hw.bus.r8(wave + k);
            hw.bus.w8(fill + k, x);
        }
        self.cn(0xcdc, 0xce2, n);
        self.c(0xce2, 0xd04);
        let pos = self.r16(hw, a4 + V_EGC_POS);
        let t = self.r32(hw, P_EGC)
            .wrapping_add((hw.bus.r8(a0 + I_EGC_TABLE) as u32) << 7)
            .wrapping_add(pos as i16 as u32);
        let neg = hw.bus.r8(t).wrapping_add(hw.bus.r8(a0 + I_EGC_OFFSET)) as u32;
        if neg != 0 {
            self.c(0xd04, 0xd06);
            for k in 0..neg {
                let x = hw.bus.r8(fill + k);
                hw.bus.w8(fill + k, x.wrapping_neg());
            }
            self.cn(0xd06, 0xd0c, neg);
        }
        self.c(0xd0c, 0xd1a);
        let pos = pos.wrapping_add(1);
        self.w16(hw, a4 + V_EGC_POS, pos);
        if pos as i16 > hw.bus.r16(a0 + I_EGC_LEN) as i16 {
            self.c(0xd1a, 0xd22);
            if self.r8(hw, a4 + V_EGC_MODE) == 1 {
                self.c(0xd22, 0xd28);
                self.w8(hw, a4 + V_EGC_MODE, 0);
            } else {
                self.c(0xd28, 0xd2c);
                self.w16(hw, a4 + V_EGC_POS, 0);
            }
        }
        self.c(0xd2c, 0xd2e);
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

    /// "TEST", one subsong (speed 3, 4 rows, position 0 only), one position:
    /// voice 0 plays note 24 with instrument 1, a synth of 32 bytes (waveform 0,
    /// a square) at volume $30; the other voices play empty rows.
    fn module() -> Vec<u8> {
        let mut d = vec![0u8; 0xcc];
        d[..8].copy_from_slice(b"ISM!V1.2");
        d[8..10].copy_from_slice(&1u16.to_be_bytes()); // positions
        d[10..12].copy_from_slice(&8u16.to_be_bytes()); // track rows
        // samples, waveforms, instruments, subsongs, EGC and ADSR tables
        d[0x10..0x16].copy_from_slice(&[0, 1, 1, 1, 0, 0]);
        d[0x24..0x28].copy_from_slice(b"TEST");
        let mut i = [0u8; 28]; // instrument 1
        i[1] = 1; // synth
        i[2..4].copy_from_slice(&32u16.to_be_bytes());
        i[6] = 0x30;
        i[0x16] = 1; // vibrato level 1 (at speed 0: no change)
        d.extend(i);
        d.extend([0u8; 0x100]); // arpeggios
        // subsong 0: speed 3, 4 rows, first, last and restart position 0
        d.extend([0, 0, 0, 0, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
        d.extend([0u8; 14]); // the one past the last
        d.extend((0..256).map(|k| if k < 16 { 0x60u8 } else { 0xa0 }));
        // position 0: voice 0 from row 0, the others from row 4
        for v in 0..4u16 {
            d.extend((if v == 0 { 0u16 } else { 4 }).to_be_bytes());
            d.extend([0, 0]);
        }
        let mut t = vec![0u8; (8 + 0x40) * 4];
        t[..4].copy_from_slice(&[24, 1, 0, 0]);
        d.extend(t);
        d
    }

    #[test]
    fn plays_a_synth_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("is10.test"), module()).unwrap();
        let mut out = vec![0f32; 2 * 24000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "InStereo! 1.0");
        assert_eq!(p.module_name, "TEST");
        assert_eq!((p.sub_min, p.sub_max), (0, 0));
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the note's length (in words) and volume (each time the song comes
        // round), its period every tick
        let len = reg(0xDFF0A4);
        assert!(!len.is_empty() && len.iter().all(|&x| x == 16));
        assert_eq!(reg(0xDFF0A8)[0], 0x30);
        let per = reg(0xDFF0A6);
        assert!(per.len() > 10 && per.iter().all(|&x| x == PERIODS[24]));
        // DMA off and on for voice 0 only
        assert_eq!(&reg(0xDFF096)[..2], &[0x0001, 0x8001]);
        let t = |a: u32, v: u16| log.iter().find(|w| w.0 == a && w.1 == v).unwrap().2;
        assert!(t(0xDFF0A4, 16) < t(0xDFF096, 0x0001));
        // the other voices: no instrument, no period
        assert!(reg(0xDFF0B6).is_empty());
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

/// The 68000 cycles of each instruction of the original's code (as the emulator times
/// it), by word from hunk offset `CYC_FROM` (0 between instructions).
const CYC_FROM: u16 = 0x2e6;
const CYC: [u8; 1317] = [
    4, 4, 16, 0, 16, 4, 9, 4, 20, 0, 0, 18, 0, 10, 0, 9, 8, 0, 12, 0,
    0, 16, 0, 4, 8, 0, 12, 10, 0, 16, 0, 0, 16, 0, 0, 16, 0, 0, 16, 0,
    0, 8, 0, 16, 0, 4, 16, 4, 16, 4, 4, 16, 0, 12, 0, 8, 16, 8, 0, 24,
    0, 24, 0, 16, 0, 11, 0, 4, 8, 0, 16, 0, 4, 8, 0, 4, 12, 0, 58, 0,
    8, 4, 12, 0, 4, 4, 4, 4, 9, 14, 10, 0, 12, 8, 4, 12, 0, 22, 8, 4,
    12, 0, 24, 8, 4, 12, 0, 58, 0, 8, 8, 0, 4, 12, 0, 8, 58, 0, 8, 4,
    12, 0, 24, 8, 4, 12, 0, 16, 8, 4, 12, 0, 16, 0, 0, 12, 8, 8, 4, 8,
    16, 0, 16, 4, 16, 0, 16, 8, 0, 12, 28, 0, 0, 16, 0, 4, 16, 120, 0, 18,
    0, 18, 0, 124, 0, 4, 16, 32, 0, 4, 4, 8, 0, 14, 16, 0, 18, 0, 36, 0,
    16, 32, 0, 4, 4, 8, 0, 14, 16, 0, 0, 16, 0, 18, 0, 36, 0, 16, 32, 0,
    4, 4, 8, 0, 14, 8, 0, 16, 0, 18, 0, 36, 0, 16, 32, 0, 8, 0, 4, 8,
    0, 14, 16, 0, 18, 0, 36, 0, 16, 32, 0, 4, 4, 8, 0, 16, 0, 0, 16, 0,
    18, 0, 36, 0, 16, 12, 4, 12, 0, 18, 12, 16, 4, 4, 4, 4, 4, 4, 4, 4,
    8, 8, 8, 8, 8, 8, 8, 16, 4, 12, 0, 12, 18, 8, 0, 4, 4, 8, 4, 10,
    0, 4, 4, 4, 8, 10, 0, 4, 4, 8, 4, 10, 0, 8, 0, 16, 0, 4, 8, 0,
    0, 12, 4, 12, 0, 58, 0, 8, 12, 4, 12, 0, 12, 8, 12, 4, 12, 0, 22, 8,
    12, 4, 12, 0, 24, 8, 12, 4, 12, 0, 58, 0, 8, 12, 8, 0, 0, 12, 4, 12,
    0, 8, 58, 0, 8, 12, 4, 12, 0, 24, 8, 12, 4, 12, 0, 16, 8, 12, 4, 12,
    0, 16, 0, 0, 12, 8, 12, 12, 0, 0, 16, 0, 4, 12, 10, 0, 16, 0, 58, 0,
    8, 8, 0, 4, 12, 10, 0, 24, 0, 0, 0, 24, 0, 0, 0, 24, 0, 0, 0, 24,
    0, 0, 0, 12, 16, 8, 0, 12, 8, 12, 0, 9, 18, 10, 0, 12, 12, 0, 12, 0,
    9, 8, 0, 12, 8, 0, 8, 12, 0, 9, 16, 0, 16, 0, 16, 12, 16, 0, 12, 0,
    4, 8, 16, 8, 28, 0, 0, 28, 0, 0, 28, 0, 0, 28, 0, 0, 16, 0, 4, 12,
    0, 12, 12, 0, 12, 8, 34, 0, 0, 0, 16, 0, 12, 0, 12, 8, 34, 0, 0, 0,
    16, 0, 12, 0, 12, 8, 34, 0, 0, 0, 16, 0, 12, 0, 12, 8, 34, 0, 0, 0,
    4, 12, 0, 0, 4, 16, 0, 0, 9, 16, 0, 16, 0, 16, 0, 18, 0, 8, 8, 8,
    16, 0, 0, 9, 16, 0, 16, 0, 16, 0, 18, 0, 8, 8, 10, 16, 0, 0, 9, 16,
    0, 16, 0, 16, 0, 18, 0, 8, 8, 10, 16, 0, 0, 9, 16, 0, 16, 0, 16, 0,
    18, 0, 4, 9, 18, 0, 18, 0, 8, 0, 18, 0, 20, 0, 0, 18, 0, 4, 8, 0,
    16, 0, 0, 9, 10, 0, 9, 12, 0, 0, 16, 0, 18, 0, 16, 0, 0, 9, 10, 0,
    9, 12, 0, 0, 16, 0, 18, 0, 16, 0, 0, 9, 10, 0, 9, 12, 0, 0, 16, 0,
    18, 0, 16, 0, 0, 11, 0, 10, 0, 11, 0, 12, 0, 0, 16, 0, 10, 0, 4, 8,
    0, 22, 4, 16, 0, 16, 0, 4, 11, 0, 8, 0, 9, 12, 0, 8, 0, 9, 16, 0,
    12, 0, 16, 0, 0, 8, 0, 9, 16, 0, 12, 0, 16, 0, 0, 8, 0, 9, 4, 4,
    12, 0, 8, 0, 9, 12, 0, 16, 0, 8, 0, 9, 12, 0, 8, 0, 9, 8, 0, 12,
    0, 8, 0, 9, 4, 9, 8, 0, 9, 16, 0, 0, 8, 0, 9, 8, 0, 4, 8, 0,
    9, 4, 4, 8, 0, 9, 4, 9, 24, 0, 0, 0, 10, 24, 0, 0, 0, 8, 0, 9,
    4, 9, 8, 0, 9, 16, 0, 0, 4, 24, 16, 0, 0, 4, 11, 0, 8, 0, 11, 0,
    8, 0, 9, 4, 16, 0, 8, 16, 0, 10, 14, 0, 18, 0, 4, 4, 9, 4, 4, 4,
    8, 9, 4, 10, 8, 0, 18, 0, 0, 16, 4, 16, 0, 4, 4, 4, 22, 8, 0, 9,
    4, 16, 0, 0, 12, 0, 8, 58, 0, 8, 4, 4, 4, 8, 9, 4, 10, 8, 0, 18,
    0, 0, 16, 0, 16, 0, 24, 0, 24, 0, 20, 0, 0, 20, 0, 0, 12, 0, 4, 12,
    0, 8, 0, 9, 24, 0, 20, 0, 0, 12, 0, 9, 16, 0, 0, 12, 0, 11, 0, 16,
    0, 16, 0, 4, 8, 8, 0, 4, 4, 9, 14, 4, 10, 18, 0, 12, 0, 8, 18, 0,
    18, 0, 8, 0, 9, 20, 0, 0, 4, 16, 8, 0, 12, 0, 4, 16, 16, 0, 16, 0,
    4, 10, 16, 0, 4, 12, 0, 4, 58, 0, 8, 12, 0, 9, 12, 0, 9, 16, 0, 0,
    9, 4, 8, 16, 0, 16, 0, 8, 0, 4, 4, 9, 14, 8, 10, 4, 12, 0, 8, 12,
    4, 18, 0, 12, 12, 0, 8, 12, 4, 18, 0, 12, 16, 24, 0, 8, 0, 18, 0, 4,
    18, 0, 28, 0, 16, 16, 0, 4, 8, 24, 8, 4, 16, 0, 0, 16, 16, 0, 8, 12,
    0, 20, 0, 0, 12, 0, 9, 4, 16, 0, 0, 16, 16, 0, 8, 12, 4, 18, 0, 12,
    12, 0, 10, 9, 4, 4, 8, 0, 4, 20, 10, 0, 4, 12, 0, 4, 9, 4, 12, 10,
    0, 12, 0, 8, 18, 0, 18, 0, 20, 0, 0, 4, 16, 12, 0, 0, 16, 0, 16, 0,
    16, 0, 18, 12, 0, 0, 16, 0, 16, 0, 16, 0, 18, 12, 0, 0, 16, 0, 16, 0,
    16, 0, 18, 12, 0, 0, 16, 0, 16, 0, 16, 0, 4, 4, 8, 12, 0, 16, 0, 4,
    14, 8, 0, 16, 12, 0, 14, 0, 4, 4, 16, 0, 10, 14, 0, 10, 14, 0, 12, 0,
    9, 12, 0, 9, 4, 9, 16, 0, 6, 4, 62, 0, 12, 0, 4, 4, 9, 142, 4, 12,
    0, 9, 12, 0, 9, 16, 0, 10, 8, 0, 12, 0, 8, 0, 14, 0, 16, 0, 0, 12,
    0, 4, 4, 11, 0, 4, 58, 0, 16, 0, 8, 12, 0, 8, 0, 4, 9, 12, 4, 9,
    124, 4, 10, 4, 10, 4, 4, 9, 12, 124, 4, 12, 0, 16, 0, 12, 0, 18, 0, 12,
    0, 4, 16, 0, 4, 16, 0, 12, 0, 4, 58, 0, 8, 12, 0, 9, 12, 0, 12, 0,
    4, 9, 16, 0, 16, 0, 4, 12, 0, 24, 8, 4, 12, 0, 8, 4, 8, 4, 4, 12,
    0, 54, 22, 16, 0, 18, 0, 12, 0, 11, 0, 16, 0, 4, 8, 24, 8, 4, 16, 0,
    0, 16, 16, 0, 8, 16, 0, 8, 9, 24, 16, 0, 16, 0, 8, 8, 10, 24, 16, 0,
    16, 0, 8, 8, 12, 4, 18, 0, 12, 12, 0, 4, 8, 0, 4, 12, 10, 0, 16, 0,
    4, 12, 0, 22, 8, 12, 0, 14, 0, 12, 0, 8, 0, 4, 9, 4, 12, 10, 0, 16,
    0, 12, 0, 12, 0, 9, 16, 0, 0, 9, 16, 0, 10, 16, 0, 16, 16,
];

/// The original's cycles from hunk offset `from` up to `to`.
fn cy(from: u16, to: u16) -> u64 {
    let (a, b) = (((from - CYC_FROM) / 2) as usize, ((to - CYC_FROM) / 2) as usize);
    CYC[a..b].iter().map(|&x| x as u64).sum()
}
