//! Music Assembler, ported from NostalgicPlayer's Music Assembler player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! A Music Assembler module is the editor's replayer with its song, its
//! variables and its tables, as one block of code. The original player, the
//! Music Assembler Eagleplayer (V2.11, adapted by DEFECT), does not run that
//! code: it has its own copy of the replayer, with the writes to the audio
//! registers turned into calls of ENPP_PokeAdr/Len/Per/Vol and ENPP_DMAMask,
//! and plays on the module's variables and tables, at fixed offsets from its
//! start ($4B0 the subsongs' position lists, $500 their speeds, $50A the DMA
//! word, $50C the volume, $50E the voice mask, $510 the speed and its
//! counter, $512 the four voices, $5B2-$5BA the tracks', samples' and
//! instruments' offsets, $5BE the periods, $61E the empty sample). This port
//! does the same in guest memory, so the module's saved state (the voices'
//! variables the file holds, which InitSound clears only in part, and their
//! hardware addresses, which give the voices their channels) plays as it does
//! in the original. It takes only the modules whose replayer has that layout,
//! as NostalgicPlayer's search of its code finds it.
//!
//! Where NostalgicPlayer differs from the original's writes, this follows the
//! original:
//! * DTP_SubSongRange gives 10 subsongs (0-9), the empty ones too, but
//!   InitSound loads d0 with dtg_SndNum and then with the module's data offset
//!   (the `lea` at $20), which it takes for the subsong: past 9, so every
//!   subsong plays subsong 0 from its start;
//! * there is no song end: a position list's end ($FE) only stops its voice
//!   reading rows, and the lists start again with no report;
//! * a note's DMA goes off at the start of the next tick and on at its end
//!   (through ENPP_DMAMask, whose 10-line wait the host does), and every tick
//!   starts with a DMA off, of no voice but after a note's row;
//! * an arpeggio of 0 leaves the replayer's d5 as the code before had it, and
//!   the arpeggio's step adds 3 to that; the comparisons are the code's
//!   (unsigned: the arpeggio counter resets only past its speed, the attack
//!   ends only past its sustain point, the vibrato level is unsigned);
//! * the vibrato's offset is added to the period on every tick, during its
//!   delay and with a portamento too; nothing is range-checked but the note
//!   (47), and the position lists and tracks are read on in the module.
//!
//! The writes come at the original's times as the emulator runs it: half the
//! 68000 cycles since its last call of the host, each instruction's cycles
//! counted on the Eagleplayer's code (`Ma::c` adds a straight run of them),
//! plus the host's stub after each call; the start and InitSound's call were
//! measured on the reference's write logs. A masked voice (its DMA bit not in
//! the voice mask, which InitSound sets to all four) passes the host a channel
//! computed from an address in the Eagleplayer's code, so from where it was
//! loaded; here it writes nothing.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Music Assembler", detect, new };

// Offsets in the module, as the Eagleplayer has them.
const POSITION_LISTS: u32 = 0x4b0;
const SPEEDS: u32 = 0x500;
const DMA: u32 = 0x50a;
const VOLUME: u32 = 0x50c;
const MASK: u32 = 0x50e;
const SPEED: u32 = 0x510;
const COUNTER: u32 = 0x511;
const VOICES: u32 = 0x512;
const VOICE_SIZE: u32 = 0x26;
const TRACKS: u32 = 0x5b2;
const SAMPLES: u32 = 0x5b6;
const INSTRUMENTS: u32 = 0x5ba;
const PERIODS: u32 = 0x5be;
const EMPTY: u32 = 0x61e;

// A voice's variables.
const V_HW: u32 = 0x00;
const V_INSTR: u32 = 0x04;
const V_LIST: u32 = 0x06;
const V_FLAGS: u32 = 0x08;
const V_VIB_DELAY: u32 = 0x09;
const V_POS: u32 = 0x0a;
const V_ROW: u32 = 0x0c;
const V_WAIT: u32 = 0x0e;
const V_ARP_COUNT: u32 = 0x0f;
const V_NOTE: u32 = 0x10;
const V_PORTA: u32 = 0x11;
const V_TRANSPOSE: u32 = 0x12;
const V_REPEAT: u32 = 0x13;
const V_ARP: u32 = 0x16;
const V_VOLUME: u32 = 0x17;
const V_PORTA_ADD: u32 = 0x18;
const V_VIB_DIR: u32 = 0x1a;
const V_VIB_COUNT: u32 = 0x1b;
const V_VIB_ADD: u32 = 0x1c;
const V_HALF: u32 = 0x1e;
const V_BIT: u32 = 0x1f;
const V_WAVE: u32 = 0x20;
const V_WAVE_DIR: u32 = 0x22;
const V_WAVE_COUNT: u32 = 0x23;
const V_HOLD: u32 = 0x24;

// The flags.
const F_LOOP: u8 = 0x02;
const F_SYNTH: u8 = 0x04;
const F_PORTA: u8 = 0x20;
const F_RELEASE: u8 = 0x40;
const F_RETRIG: u8 = 0x80;

// An instrument (16 bytes).
const I_SAMPLE: u32 = 0x0;
const I_ATTACK: u32 = 0x1;
const I_DECAY_SUSTAIN: u32 = 0x2;
const I_VIB_DELAY: u32 = 0x3;
const I_RELEASE: u32 = 0x4;
const I_VIB_SPEED: u32 = 0x5;
const I_VIB_LEVEL: u32 = 0x6;
const I_ARPEGGIO: u32 = 0x7;
const I_FX_ARP: u32 = 0x8;
const I_HOLD: u32 = 0x9;
const I_KEY_WAVE: u32 = 0xa;
const I_WAVE: u32 = 0xb;

/// The host's stub around each call (its `rts`), in cycles.
const STUB: i64 = 16;
/// From the timer to the interrupt's code (cycles; measured: none).
const ENTRY: i64 = 0;
/// The Eagleplayer's start up to InitSound, and from InitSound's end to the
/// timer's start (colour clocks; measured on the reference's write logs).
const START_WAIT: u64 = 4623;
const INIT_WAIT: u64 = 189;
/// From the host's call of InitSound to its check of the module (cycles: the
/// moveq and jsr to ENPP_GetListData, and its stub).
const INIT_ENTRY: i64 = 38;

fn be16(d: &[u8], o: usize) -> u16 {
    super::be16(d, o)
}

/// NostalgicPlayer's search of the replayer for its tables: the offsets of the
/// speeds, the position lists, the module's data and the instruments',
/// samples' and tracks' offsets.
fn search(d: &[u8]) -> Option<[usize; 6]> {
    let b = &d[..d.len().min(0x700)];
    let n = b.len();
    let sw = |o: usize| be16(b, o) as i16 as isize;
    let find = |from: usize, end: usize, hi: u8, lo: u8| (from..end).step_by(2).find(|&i| b[i] == hi && b[i + 1] == lo);
    // the init routine
    let init = (sw(2) + 2) as usize;
    if init >= n {
        return None;
    }
    let i = find(init, n - 4, 0xb0, 0x7c)?;
    let i = find(i + 4, n - 4, 0x49, 0xfa)?;
    let speeds = (sw(i + 2) + i as isize + 2) as usize;
    let i = find(i + 4, n - 4, 0x49, 0xfb)?;
    let lists = (b[i + 3] as i8 as isize + i as isize + 2) as usize;
    // the play routine
    let i = find(0x0c, n - 4, 0x43, 0xfa)?;
    let data = (sw(i + 2) + i as isize + 2) as usize;
    let i = find(i + 4, n - 8, 0xd3, 0xfa)?;
    let instruments = (sw(i + 2) + i as isize + 2) as usize;
    if b[i + 4] != 0xd5 || b[i + 5] != 0xfa {
        return None;
    }
    let samples = (sw(i + 6) + i as isize + 6) as usize;
    let i = (i + 8..n - 2).step_by(2).find(|&i| b[i] == 0x61)?;
    let i = (b[i + 1] as i8 as isize + i as isize + 2) as usize;
    if i >= n {
        return None;
    }
    let i = find(i, n - 4, 0xdb, 0xfa)?;
    let tracks = (sw(i + 2) + i as isize + 2) as usize;
    Some([speeds, lists, data, instruments, samples, tracks])
}

fn detect(d: &[u8]) -> bool {
    // the Eagleplayer's check, and InitSound's (a `lea xx(pc),a1` to the data
    // at $20); then NostalgicPlayer's search, which must find the tables where
    // the Eagleplayer reads them
    d.len() >= 0x700
        && be16(d, 0) == 0x6000
        && be16(d, 4) == 0x6000
        && be16(d, 8) == 0x6000
        && be16(d, 0xc) == 0x48e7
        && be16(d, 0x20) == 0x43fa
        && search(d).is_some_and(|s| {
            s == [
                SPEEDS as usize,
                POSITION_LISTS as usize,
                0x22 + be16(d, 0x22) as i16 as isize as usize,
                INSTRUMENTS as usize,
                SAMPLES as usize,
                TRACKS as usize,
            ]
        })
}

pub struct Ma {
    /// The module in guest memory, and its data (the `lea` at $20).
    base: u32,
    data: u32,
    sub: u16,
    /// Cycles run since the last call of the host.
    pend: i64,
    /// The replayer's d5 (an arpeggio of 0 leaves it to the next step).
    d5: u32,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let data = base.wrapping_add((0x22 + be16(d, 0x22) as i16 as i32) as u32);
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(START_WAIT);
    let mut ma = Ma { base, data, sub: 0, pend: 0, d5: 0 };
    ma.init_sound(0, &mut hw);
    hw.wait(INIT_WAIT);
    Some(Box::new(ma))
}

impl Replayer for Ma {
    fn format(&self) -> &'static str {
        "Music Assembler"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        false
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, 9, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.init_sound(n, hw);
        hw.wait(INIT_WAIT);
    }

    fn tick(&mut self, hw: &mut Hw) {
        let g = self.base;
        self.d5 = 0;
        self.pend = ENTRY;
        // the DMA off of the last tick's notes
        self.c(264);
        self.dma_mask(hw, hw.bus.r16(g + DMA));
        self.c(52 + 167);
        let d3 = hw.bus.r16(g + MASK);
        let count = hw.bus.r8(g + COUNTER).wrapping_sub(1);
        hw.bus.w8(g + COUNTER, count);
        let mut d4 = 0u16;
        if count != 0 {
            for i in 0..4 {
                self.c(if i == 0 { 18 } else { 26 });
                self.effects(hw, i, d3);
            }
            self.c(14);
        } else {
            self.c(20);
            hw.bus.w8(g + COUNTER, hw.bus.r8(g + SPEED));
            for i in 0..4 {
                self.c(if i == 0 { 18 } else { 26 });
                self.row(hw, i, d3, &mut d4);
            }
        }
        // the DMA on of the last tick's notes (the low byte), and this tick's
        // notes for the next
        self.c(70 + 94);
        hw.bus.w8(g + DMA, 0x82);
        self.dma_mask(hw, hw.bus.r16(g + DMA));
        hw.bus.w16(g + DMA, d4 & d3);
    }
}

/// A word sign-extended, as `adda.w` adds it.
fn sx(w: u16) -> u32 {
    w as i16 as i32 as u32
}

impl Ma {
    /// Cycles of the original's code.
    fn c(&mut self, n: i64) {
        self.pend += n;
    }

    fn voice(&self, i: usize) -> u32 {
        self.base + VOICES + VOICE_SIZE * i as u32
    }

    /// A call of the host: half the cycles since the last, then its stub.
    fn host(&mut self, hw: &mut Hw) {
        hw.wait((self.pend.max(0) / 2) as u64);
        self.pend = STUB;
    }

    /// ENPP_DMAMask: the channels of the low 4 bits, on if bit 15 is set; the
    /// host waits for the DMA then, to 10 lines past the current one.
    fn dma_mask(&mut self, hw: &mut Hw, d0: u16) {
        self.host(hw);
        let v = d0 & 0x800f;
        hw.dmacon(v);
        if v & 0x8000 != 0 {
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
        }
    }

    /// The channel the Eagleplayer passes for a voice's hardware address
    /// (`None`: a masked voice, whose address is in its own code); the length
    /// and period calls mask it to 4 bits, the others do not, and the host
    /// drops a channel past 3.
    fn channel(a3: Option<u32>, masked: bool) -> Option<usize> {
        let ch = (a3? as u16).wrapping_sub(0xf0a0) >> 4;
        let ch = if masked { ch & 0xf } else { ch };
        (ch < 4).then_some(ch as usize)
    }

    fn poke_adr(&mut self, hw: &mut Hw, a3: Option<u32>, a: u32) {
        self.host(hw);
        if let Some(ch) = Self::channel(a3, false) {
            hw.loc(ch, a);
        }
    }

    fn poke_len(&mut self, hw: &mut Hw, a3: Option<u32>, n: u16) {
        self.host(hw);
        if let Some(ch) = Self::channel(a3, true) {
            hw.len(ch, n);
        }
    }

    fn poke_per(&mut self, hw: &mut Hw, a3: Option<u32>, p: u16) {
        self.host(hw);
        if let Some(ch) = Self::channel(a3, true) {
            hw.per(ch, p);
        }
    }

    fn poke_vol(&mut self, hw: &mut Hw, a3: Option<u32>, v: u8) {
        self.host(hw);
        if let Some(ch) = Self::channel(a3, false) {
            hw.vol(ch, v as u16);
        }
    }

    /// The Eagleplayer's InitSound: a subsong's speed, and each voice's
    /// position list from its start, its DMA off. It reads dtg_SndNum into d0
    /// and then the module's data offset (the `lea` at $20) into d0 as well,
    /// and takes d0 for the subsong: past 9, it plays subsong 0 whatever
    /// subsong `n` is.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        let g = self.base;
        self.sub = n;
        self.pend = INIT_ENTRY;
        // the module's check and the patching of the Eagleplayer's 12 pointers
        self.c(93 + 12 * 71 + 17 + 12 + 113);
        let d0 = hw.bus.r16(g + 0x22);
        let n = if d0 >= 10 {
            self.c(4);
            0
        } else {
            d0 as u32
        };
        self.c(106);
        hw.bus.w16(g + DMA, 0);
        hw.bus.w8(g + SPEED, hw.bus.r8(g + SPEEDS + n));
        hw.bus.w8(g + COUNTER, 1);
        let lists = g + POSITION_LISTS + 8 * n;
        for i in 0..4 {
            let v = self.voice(i);
            self.c(18 + 200 + 18 + 94);
            hw.bus.w8(v + V_WAIT, 0);
            let list = hw.bus.r16(lists + 2 * i as u32);
            hw.bus.w16(v + V_LIST, list);
            let b = hw.bus.r8(self.data.wrapping_add(sx(list)).wrapping_add(1));
            hw.bus.w16(v + V_TRANSPOSE, transpose(b));
            hw.bus.w32(v + V_POS, 0);
            hw.bus.w8(v + V_FLAGS, 0);
            hw.bus.w8(v + V_VOLUME, 0);
            self.dma_mask(hw, hw.bus.r16(g + MASK));
            hw.bus.w16(g + MASK, 0);
            self.c(52 + 52);
        }
        // the volume and the voice mask
        hw.bus.w16(g + VOLUME, 64);
        hw.bus.w16(g + MASK, 0xf);
    }

    /// A voice's row, when its wait is over; else its effects.
    fn row(&mut self, hw: &mut Hw, i: usize, d3: u16, d4: &mut u16) {
        let v = self.voice(i);
        let ms = self.data;
        let r8 = |hw: &Hw, a: u32| hw.bus.r8(a);
        let r16 = |hw: &Hw, a: u32| hw.bus.r16(a);
        self.c(27);
        let wait = r8(hw, v + V_WAIT).wrapping_sub(1);
        hw.bus.w8(v + V_WAIT, wait);
        if (wait as i8) >= 0 {
            return self.effects(hw, i, d3);
        }
        self.c(79);
        let a3 = ms.wrapping_add(sx(r16(hw, v + V_LIST))).wrapping_add(sx(r16(hw, v + V_POS)));
        let track = r8(hw, a3).wrapping_add(2);
        if track == 0 {
            // $FE: the end of the list
            return self.effects(hw, i, d3);
        }
        self.c(125);
        let tracks = ms.wrapping_add(hw.bus.r32(self.base + TRACKS));
        let start = tracks.wrapping_add(sx(r16(hw, ms.wrapping_sub(4).wrapping_add(2 * track as u32))));
        let mut p = start.wrapping_add(sx(r16(hw, v + V_ROW)));
        let mut next = |hw: &Hw| {
            let b = hw.bus.r8(p);
            p = p.wrapping_add(1);
            b
        };
        let mut d7 = next(hw);
        // 0: a note, 1: an instrument's note, 2: a note that keeps the voice
        let mut kind = 0;
        if d7 & 0x80 != 0 {
            self.c(19);
            if d7 & 0x40 == 0 {
                // the release
                self.c(54);
                d7 &= 0x3f;
                hw.bus.w8(v + V_FLAGS, r8(hw, v + V_FLAGS) | F_RELEASE);
                hw.bus.w8(v + V_HOLD, 0);
                kind = 3;
            } else {
                self.c(61);
                hw.bus.w16(v + V_INSTR, ((d7 & 0x3f) as u16) << 4);
                d7 = next(hw);
                if d7 & 0x40 != 0 {
                    kind = 1;
                } else {
                    self.c(24);
                    hw.bus.w32(v + V_WAVE, 1);
                }
            }
        }
        if kind == 0 {
            self.c(19);
            if d7 & 0x40 != 0 {
                kind = 2;
            } else {
                // a new note: the voice's state from its instrument
                self.c(150);
                hw.bus.w8(v + V_HALF, 0);
                hw.bus.w16(v + V_ARP, 0);
                hw.bus.w32(v + V_VIB_DIR, 0);
                let off = r16(hw, v + V_INSTR);
                self.d5 = self.d5 & 0xffff_0000 | off as u32;
                let ins = self.instrument(hw, off);
                hw.bus.w8(v + V_VIB_DELAY, r8(hw, ins + I_VIB_DELAY));
                hw.bus.w8(v + V_ARP_COUNT, r8(hw, ins + I_FX_ARP));
                hw.bus.w8(v + V_HOLD, r8(hw, ins + I_HOLD));
                hw.bus.w8(v + V_FLAGS, 0);
            }
        }
        if kind < 2 {
            // the retrigger, and the voice's DMA off next tick
            self.c(52);
            hw.bus.w8(v + V_FLAGS, r8(hw, v + V_FLAGS) & F_RELEASE | F_RETRIG);
            *d4 |= r8(hw, v + V_BIT) as u16;
        }
        if kind < 3 {
            self.c(65);
            hw.bus.w16(v + V_PORTA_ADD, 0);
            hw.bus.w8(v + V_NOTE, (d7 & 0x3f).wrapping_add(r8(hw, v + V_TRANSPOSE)));
            d7 = next(hw);
            if d7 & 0x80 == 0 {
                self.c(26);
                hw.bus.w8(v + V_PORTA, 0);
            } else {
                self.c(24);
                d7 &= 0x7f;
                let b = next(hw);
                hw.bus.w8(v + V_PORTA, b);
            }
        }
        self.c(33);
        hw.bus.w8(v + V_WAIT, d7);
        let mut start = start;
        if r8(hw, p) == 0xff {
            // the track's end: again, or the next position
            self.c(25);
            let repeat = r8(hw, v + V_REPEAT).wrapping_sub(8);
            hw.bus.w8(v + V_REPEAT, repeat);
            if (repeat as i8) < 0 {
                self.c(41);
                let pos = r16(hw, v + V_POS).wrapping_add(2);
                hw.bus.w16(v + V_POS, pos);
                let mut a3 = a3;
                if r8(hw, a3.wrapping_add(2)) == 0xff {
                    self.c(32);
                    a3 = a3.wrapping_sub(sx(pos));
                    hw.bus.w16(v + V_POS, 0);
                }
                self.c(46);
                hw.bus.w16(v + V_TRANSPOSE, transpose(r8(hw, a3.wrapping_add(3))));
            }
            self.c(4);
            start = p;
        }
        self.c(36);
        hw.bus.w16(v + V_ROW, p.wrapping_sub(start) as u16);
    }

    fn instrument(&self, hw: &Hw, off: u16) -> u32 {
        self.data.wrapping_add(hw.bus.r32(self.base + INSTRUMENTS)).wrapping_add(sx(off))
    }

    /// A voice's sample, effects, period and volume.
    fn effects(&mut self, hw: &mut Hw, i: usize, d3: u16) {
        let v = self.voice(i);
        let r8 = |hw: &Hw, a: u32| hw.bus.r8(a);
        let r16 = |hw: &Hw, a: u32| hw.bus.r16(a);
        let r32 = |hw: &Hw, a: u32| hw.bus.r32(a);
        let w8 = |hw: &mut Hw, a: u32, b: u8| hw.bus.w8(a, b);
        let w16 = |hw: &mut Hw, a: u32, w: u16| hw.bus.w16(a, w);
        self.c(41);
        let mut a3 = Some(r32(hw, v + V_HW));
        if r8(hw, v + V_BIT) as u16 & d3 == 0 {
            self.c(8);
            a3 = None;
        }
        self.c(81);
        let ins = self.instrument(hw, r16(hw, v + V_INSTR));
        let a2 = self.data.wrapping_add(r32(hw, self.base + SAMPLES));
        let smp = a2 + 24 * r8(hw, ins + I_SAMPLE) as u32;
        let sample = |hw: &Hw| a2.wrapping_add(r32(hw, smp));
        let flags = r8(hw, v + V_FLAGS);
        // the sample: the note's, then its loop, or the synth sound's wave
        let mut wave = false;
        if flags & F_LOOP != 0 {
            self.c(43);
            w8(hw, v + V_FLAGS, flags & !F_LOOP);
            let loop_len = r16(hw, smp + 6);
            self.d5 = self.d5 & 0xffff_0000 | loop_len as u32;
            if loop_len == 0 {
                // a one-shot sample: the empty sample after it
                self.c(34 + 92);
                self.poke_adr(hw, a3, self.base + EMPTY);
                self.c(52 + 22 + 108);
                self.poke_len(hw, a3, 2);
                self.c(44 + 32);
                w8(hw, v + V_FLAGS, r8(hw, v + V_FLAGS) & !F_SYNTH);
                self.c(25);
            } else {
                self.c(34 + 108);
                self.poke_len(hw, a3, loop_len);
                self.c(44 + 69);
                let len = r16(hw, smp + 4);
                if len < 0x81 {
                    wave = true;
                } else {
                    self.c(66 + 92);
                    let a = sample(hw).wrapping_add(sx(len).wrapping_mul(2)).wrapping_sub(sx(loop_len).wrapping_mul(2));
                    self.poke_adr(hw, a3, a);
                    self.c(52 + 22);
                }
            }
        } else {
            self.c(27);
            if flags & F_RETRIG != 0 {
                self.c(127);
                w8(hw, v + V_FLAGS, flags & F_RELEASE | F_LOOP);
                let mut a5 = sample(hw);
                let mut len = r16(hw, smp + 4);
                let loop_len = r16(hw, smp + 6);
                self.d5 = self.d5 & 0xffff_0000 | loop_len as u32;
                if len < 0x81 {
                    self.c(15);
                    if a5 != self.base + EMPTY {
                        // a synth sound: its loop from the wave's start
                        self.c(45);
                        w8(hw, v + V_FLAGS, r8(hw, v + V_FLAGS) | F_SYNTH);
                        a5 = a5.wrapping_add(r8(hw, ins + I_KEY_WAVE) as u32);
                        len = loop_len;
                        if len == 0 {
                            self.c(4);
                            len = 1;
                        }
                    }
                }
                self.c(34 + 92);
                self.poke_adr(hw, a3, a5);
                self.c(52 + 22 + 108);
                self.poke_len(hw, a3, len);
                self.c(44 + 56);
            } else {
                self.c(25);
                wave = r8(hw, v + V_FLAGS) & F_SYNTH != 0;
            }
        }
        if wave {
            // the synth sound's wave moves through its sample
            self.c(51);
            let w = r8(hw, ins + I_WAVE);
            let mut step = (w >> 4) as u16;
            let speed = w & 0xf;
            if speed != 0 {
                self.c(45);
                let count = r8(hw, v + V_WAVE_COUNT).wrapping_add(1);
                w8(hw, v + V_WAVE_COUNT, count);
                if speed * 2 < count {
                    self.c(32);
                    w8(hw, v + V_WAVE_DIR, !r8(hw, v + V_WAVE_DIR));
                    w8(hw, v + V_WAVE_COUNT, 0);
                }
                self.c(21);
                if r8(hw, v + V_WAVE_DIR) != 0 {
                    self.c(4);
                    step = step.wrapping_neg();
                }
            }
            self.c(106 + 34 + 92);
            let pos = r16(hw, v + V_WAVE).wrapping_add(step);
            w16(hw, v + V_WAVE, pos);
            let d6 = pos >> 2;
            let d6 = d6 & 0xff00 | (d6 as u8).wrapping_add(r8(hw, ins + I_KEY_WAVE)) as u16;
            let mask = r16(hw, smp + 4).wrapping_sub(1);
            self.d5 = mask as u32;
            let a = sample(hw).wrapping_add(sx(d6 & mask));
            self.poke_adr(hw, a3, a);
            self.c(64);
        }

        // the arpeggio: the instrument's two notes, by the voice's step
        self.c(25);
        let mut note = r8(hw, ins + I_ARPEGGIO);
        if note != 0 {
            self.c(25);
            let step = r8(hw, v + V_ARP).wrapping_sub(2);
            self.d5 = self.d5 & !0xff | step as u32;
            if step == 0 {
                self.c(14);
                note >>= 4;
            } else if (step as i8) > 0 {
                self.c(9 + 18);
                note &= 0xf;
            } else {
                self.c(9 + 14);
                note = 0;
            }
        }
        self.c(21);
        let fx = r8(hw, ins + I_FX_ARP);
        if fx != 0 {
            self.c(41);
            let count = r8(hw, v + V_ARP_COUNT).wrapping_add(0x10);
            w8(hw, v + V_ARP_COUNT, count);
            if fx < count {
                self.c(33);
                w8(hw, v + V_ARP_COUNT, 0);
                let mut d6 = fx as u32 & 3;
                if d6 == 0 {
                    self.c(20);
                    w8(hw, v + V_HALF, !r8(hw, v + V_HALF));
                    d6 = 1;
                }
                self.c(21);
                let step = (self.d5 as u8).wrapping_add(3);
                self.d5 = self.d5 & !0xff | step as u32;
                if step >= 4 {
                    self.c(4);
                    self.d5 = self.d5 & 0xffff_0000 | d6;
                }
                self.c(12);
                w8(hw, v + V_ARP, self.d5 as u8);
            }
        }
        self.c(37);
        note = note.wrapping_add(r8(hw, v + V_NOTE));
        if r8(hw, v + V_FLAGS) & F_SYNTH == 0 {
            self.c(12);
            note = note.wrapping_add(r8(hw, ins + I_KEY_WAVE));
        }
        self.c(17);
        if note >= 0x30 {
            self.c(4);
            note = 0x2f;
        }
        self.c(47);
        let mut period = r16(hw, self.base + PERIODS + 2 * note as u32);

        // the portamento, every other tick for an odd speed
        let porta = r8(hw, v + V_PORTA);
        if porta != 0 {
            self.c(29);
            let flags = r8(hw, v + V_FLAGS);
            w8(hw, v + V_FLAGS, flags ^ F_PORTA);
            let mut step = true;
            if flags & F_PORTA != 0 {
                self.c(19);
                step = porta & 1 == 0;
            }
            if step {
                self.c(41);
                let add = r16(hw, v + V_PORTA_ADD);
                period = (period << 1).wrapping_add(add) >> 1;
                if (porta as i8) < 0 {
                    self.c(42);
                    w16(hw, v + V_PORTA_ADD, add.wrapping_add(((porta.wrapping_neg() as u16) + 1) >> 1));
                } else {
                    self.c(28);
                    w16(hw, v + V_PORTA_ADD, add.wrapping_sub((porta as u16 + 1) >> 1));
                }
            }
        }

        // the vibrato, after its delay and with no portamento
        self.c(21);
        let delay = r8(hw, v + V_VIB_DELAY);
        if delay != 0 {
            self.c(26);
            w8(hw, v + V_VIB_DELAY, delay - 1);
        } else {
            self.c(21);
            if r8(hw, v + V_PORTA) == 0 {
                self.c(25);
                let level = r8(hw, ins + I_VIB_LEVEL) as u16;
                self.d5 = level as u32;
                if level != 0 {
                    self.c(29);
                    let dir = r8(hw, v + V_VIB_DIR) & 3;
                    let mut add = level;
                    if dir != 0 {
                        self.c(17);
                        if dir != 3 {
                            self.c(4);
                            add = add.wrapping_neg();
                        }
                    }
                    self.d5 = add as u32;
                    self.c(65);
                    w16(hw, v + V_VIB_ADD, r16(hw, v + V_VIB_ADD).wrapping_add(add));
                    let count = r8(hw, v + V_VIB_COUNT).wrapping_add(1);
                    w8(hw, v + V_VIB_COUNT, count);
                    if count == r8(hw, ins + I_VIB_SPEED) {
                        self.c(32);
                        w8(hw, v + V_VIB_COUNT, 0);
                        w8(hw, v + V_VIB_DIR, r8(hw, v + V_VIB_DIR).wrapping_add(1));
                    }
                }
            }
        }

        // the envelope: attack to the sustain level, then the hold and the
        // release
        self.c(29);
        if r8(hw, v + V_FLAGS) & F_RELEASE == 0 {
            self.c(85);
            let vol = r8(hw, v + V_VOLUME) as u16 + r8(hw, ins + I_ATTACK) as u16;
            w8(hw, v + V_VOLUME, vol as u8);
            let ds = r8(hw, ins + I_DECAY_SUSTAIN);
            self.d5 = vol as u32;
            if (ds as u16 & 0xf0 | 0xf) < vol {
                self.c(68);
                w8(hw, v + V_FLAGS, r8(hw, v + V_FLAGS) | F_RELEASE);
                w8(hw, v + V_VOLUME, ds << 4);
            }
        } else {
            self.c(21);
            let hold = r8(hw, v + V_HOLD);
            if hold != 0 {
                self.c(26);
                w8(hw, v + V_HOLD, hold - 1);
            } else {
                self.c(37);
                let (vol, under) = r8(hw, v + V_VOLUME).overflowing_sub(r8(hw, ins + I_RELEASE));
                w8(hw, v + V_VOLUME, vol);
                if under {
                    self.c(16);
                    w8(hw, v + V_VOLUME, 0);
                }
            }
        }

        // the volume (a quarter for the arpeggio's every other step), the
        // period with the vibrato
        self.c(157);
        let master = r16(hw, self.base + VOLUME);
        self.d5 = self.d5 & 0xffff_0000 | master as u32;
        let mut vol = ((r8(hw, v + V_VOLUME) as u32 * master as u32) & 0xffff) >> 8;
        if r8(hw, v + V_VIB_DELAY) == 0 {
            self.c(21);
            if r8(hw, v + V_HALF) != 0 {
                self.c(10);
                vol >>= 2;
            }
        }
        self.c(45);
        let mut period = (period << 1).wrapping_add(r16(hw, v + V_VIB_ADD)) >> 1;
        if period < 0x7f {
            self.c(4);
            period = 0x7f;
        }
        self.c(34 + 92);
        self.poke_per(hw, a3, period);
        self.c(44 + 26 + 84);
        self.poke_vol(hw, a3, vol as u8);
        self.c(44 + 44);
    }
}

/// A position's second byte: the transpose (high nibble) and the repeat count
/// times 8 (low nibble), as the replayer splits it.
fn transpose(b: u8) -> u16 {
    ((b as u16) << 4) & 0xff00 | ((b << 4) >> 1) as u16
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// NostalgicPlayer's period table (Tables.cs).
    const PERIODS: [u16; 48] = [
        1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570,
        538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160,
        151, 143, 135, 127, 120, 113,
    ];

    /// A module with the opcodes NostalgicPlayer's search looks for (and no
    /// code) and the tables where the Eagleplayer reads them: voice 0 plays
    /// note 24 with a looped sample (instrument 0), voice 1 with a synth sound
    /// (instrument 1), voices 2 and 3 have an empty position list.
    fn module() -> Vec<u8> {
        let mut d = vec![0u8; 0x622 + 0x340];
        let put = |d: &mut Vec<u8>, o: usize, b: &[u8]| d[o..o + b.len()].copy_from_slice(b);
        let w = |d: &mut Vec<u8>, o: usize, v: u16| d[o..o + 2].copy_from_slice(&v.to_be_bytes());
        let l = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_be_bytes());
        // the entries, the init routine's references at $442, the play
        // routine's from $20
        put(&mut d, 0, &[0x60, 0, 0x04, 0x34, 0x60, 0, 0, 0, 0x60, 0, 0, 0, 0x48, 0xe7]);
        put(&mut d, 0x20, &[0x43, 0xfa, 0x06, 0x00]);
        put(&mut d, 0x2a, &[0xd3, 0xfa, 0x05, 0x8e, 0xd5, 0xfa, 0x05, 0x86]);
        put(&mut d, 0x40, &[0x61, 0x0e]);
        put(&mut d, 0x50, &[0xdb, 0xfa, 0x05, 0x60]);
        put(&mut d, 0x442, &[0xb0, 0x7c, 0x00, 0x0a]);
        put(&mut d, 0x44c, &[0x49, 0xfa, 0x00, 0xb2]);
        put(&mut d, 0x45a, &[0x49, 0xfb, 0x70, 0x54]);
        // subsong 0's position lists and speed, the volume and voice mask
        for (i, o) in [0x60u16, 0x64, 0x68, 0x68].into_iter().enumerate() {
            w(&mut d, 0x4b0 + 2 * i, o);
        }
        d[0x500] = 3;
        w(&mut d, 0x50c, 0x40);
        w(&mut d, 0x50e, 0xf);
        // the voices' hardware addresses and DMA bits
        for (i, (hw, bit)) in [(0xdff0a0u32, 1u8), (0xdff0d0, 8), (0xdff0b0, 2), (0xdff0c0, 4)].into_iter().enumerate()
        {
            let v = 0x512 + 0x26 * i;
            l(&mut d, v, hw);
            d[v + 0x1f] = bit;
        }
        l(&mut d, 0x5b2, 0x80);
        l(&mut d, 0x5b6, 0x30);
        l(&mut d, 0x5ba, 0x10);
        for (i, &p) in PERIODS.iter().enumerate() {
            w(&mut d, 0x5be + 2 * i, p);
        }
        let ms = 0x622;
        // the tracks' offsets; track 0: note 24, wait 7; track 1: instrument 1,
        // note 24, wait 7
        w(&mut d, ms + 2, 8);
        put(&mut d, ms + 0x80, &[0x18, 0x07, 0xff]);
        put(&mut d, ms + 0x88, &[0xc1, 0x18, 0x07, 0xff]);
        // instruments: attack $40 up to $FF
        put(&mut d, ms + 0x10, &[0, 0x40, 0xf0]);
        put(&mut d, ms + 0x20, &[1, 0x40, 0xf0]);
        // samples: $100 words looping the last $80; a $20-word synth sound
        // looping $10
        put(&mut d, ms + 0x30, &[0, 0, 0, 0xd0, 0x01, 0x00, 0x00, 0x80]);
        put(&mut d, ms + 0x48, &[0, 0, 0x02, 0xd0, 0x00, 0x20, 0x00, 0x10]);
        for i in 0..0x200 {
            d[ms + 0x100 + i] = i as u8;
        }
        for i in 0..0x40 {
            d[ms + 0x300 + i] = if i < 0x20 { 0x40 } else { 0xc0 };
        }
        // position lists: track 0, track 1, the end
        put(&mut d, ms + 0x60, &[0, 0, 0xff, 0, 1, 0, 0xff, 0, 0xfe, 0]);
        d
    }

    #[test]
    fn plays_a_sample_and_a_synth_sound_on_paula() {
        assert!(super::detect(&module()));
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.ma"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Music Assembler");
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        let loc = |ch: u32| {
            let (hi, lo) = (reg(0xdff0a0 + 16 * ch), reg(0xdff0a2 + 16 * ch));
            hi.iter().zip(lo).map(|(&h, l)| (h as u32) << 16 | l as u32).collect::<Vec<_>>()
        };
        // InitSound's DMA off (the voice mask, then 0), the first tick's rows
        // (no writes), the notes' DMA off and on
        assert_eq!(&reg(0xdff096)[..8], &[0x000f, 0, 0, 0, 0, 0x8000, 0x0009, 0x8009]);
        // voice 0 on channel 0: the sample, then its loop, once
        assert_eq!(reg(0xdff0a4), [0x100, 0x80]);
        let l0 = loc(0);
        assert_eq!(l0[1] - l0[0], 0x100);
        assert_eq!(reg(0xdff0a6)[0], 428);
        assert_eq!(reg(0xdff0a8)[0], 16);
        // voice 1 on channel 3: the synth sound's loop from its wave, whose
        // start is written every tick
        assert_eq!(reg(0xdff0d4), [0x10, 0x10]);
        let l3 = loc(3);
        assert_eq!(l3[0] - l0[0], 0x200);
        assert!(l3.len() > 2 && l3[1..].iter().all(|&a| a == l3[0]));
        // voices 2 and 3 run their effects only: period and volume
        assert!(reg(0xdff0b4).is_empty() && !reg(0xdff0b6).is_empty());
        // the DMA off has time to stop the voices before the DMA on
        let at = |v: u16| log.iter().find(|w| w.0 == 0xdff096 && w.1 == v).unwrap().2;
        assert!(at(0x8009) - at(0x0009) > 1000);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
