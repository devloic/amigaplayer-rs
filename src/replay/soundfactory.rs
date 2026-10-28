//! Sound Factory (Profiteam), ported from NostalgicPlayer's Sound Factory player
//! (Copyright (c) 2024 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the Eagleplayer "Profiteam-Soundfactory" ("Profiteam-
//! Soundfactory Player Module V1.0", adapted by marley/INFECT), which has the
//! replayer in its own code hunk and writes Paula directly. A module is four
//! lists of opcodes and notes a subsong, with the instruments and their samples
//! defined in the lists. Like the original this plays from the module in guest
//! memory: the effect opcodes change the instrument there, for good (a later
//! subsong finds them changed). The player's variables are in guest memory in
//! the layout of the original's code hunk, with its per-voice stacks (80 bytes,
//! a byte index), its 32 instrument pointers (an instrument number shifted at a
//! byte) and its empty default instrument, and its BSS hunk holds the voices'
//! phasing and filter buffers, so the original's reads and writes past its
//! tables come out the same.
//!
//! Where NostalgicPlayer differs from the original, this follows the original:
//! * a note turns its voice's DMA off, writes its location (the phasing and the
//!   filter buffer each again), its length and its volume and period, waits for
//!   five lines and turns the DMA on; a note of length 0 has its DMA turned off
//!   and goes on with the list;
//! * the loop goes out on the next tick: a sustain part (either offset word not
//!   0) from the sustain offset to the release offset, a one-shot sample the
//!   player's empty word (length 1); the release of a one-shot sample with a
//!   sustain part writes the length to its end, and the tick after that the
//!   empty word;
//! * every tick writes every playing voice's volume and period, a voice without
//!   a note too; End writes volume 0 and turns the voice off for good;
//! * the envelope has no decay: the attack adds the sustain level every tick
//!   and a step of the volume per attack time, until the volume is the sustain
//!   level; the release starts after attack + decay ticks (half the note's
//!   length when decay is 0) and steps the volume down per release time;
//! * the default instrument is all zeros (NostalgicPlayer's has an attack, a
//!   sustain and a sample); the opcodes past $9B are not the original's (it
//!   jumps into its own code for them): $9C transposes as in NostalgicPlayer
//!   and the PlayerStore DeliPlayer, the others do nothing;
//! * the phasing's position is an unsigned byte and its mix halves with a
//!   shift (NostalgicPlayer: signed, and a division), reading the bytes past
//!   the sample; the filter copies (length / 2 + 1) longs; the multi-octave
//!   period's division keeps the dividend's low word when it overflows;
//! * a fade out that reaches 0 runs EndSound (all voices off, volume 0, the
//!   BSS end as location, length 1); there is no song end;
//! * the subsongs are the module's slots with a channel byte that is not 0,
//!   numbered from 0; InitSound plays the first tick at once, and does not reset
//!   the voices' effects' variables.
//!
//! The writes come at the original's times: each code path counts the cycles
//! of the original's instructions on it (as the host's 68000 counts them, a
//! fixed number an instruction), from the start of the call (the interrupt,
//! InitSound, EndSound), at two a colour clock; a note's line waits read
//! VHPOSR at the times its loop does.

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Sound Factory", detect, new };

/// The multipliers of a sampling period for the 12 notes of an octave.
const MULTIPLY: [u16; 12] = [32768, 30929, 29193, 27555, 26008, 24549, 23171, 21870, 20643, 19484, 18391, 17359];
/// The periods of the 12 notes of a multi-octave sample's lowest octave.
const SAMPLES: [u16; 12] = [54728, 51656, 48757, 46020, 43437, 40999, 38698, 36526, 34476, 32541, 30715, 28964];

// The hunks: the BSS hunk's block below the code hunk's, as the loader leaves
// them from the top of memory (sizes rounded up to 8, with 8 bytes of header).
const BSS_SIZE: u32 = 1028;
const CODE_SIZE: u32 = 4468;
const BSS_BLOCK: u32 = (BSS_SIZE + 8 + 7) & !7;
const CODE_BLOCK: u32 = (CODE_SIZE + 8 + 7) & !7;

// Offsets in the original's code hunk: its variables are around A6 = $376.
const A6: u32 = 0x376;
const REQUEST: u32 = A6 - 0x46;
/// The playing voices (bits 0-3).
const VOICES_ON: u32 = A6 - 0x45;
const START: u32 = A6 - 0x44; // longs
const POS: u32 = A6 - 0x34; // longs
const DURATION: u32 = A6 - 0x24; // words
const INSTRUMENT: u32 = A6 - 0x1c;
const VOLUME: u32 = A6 - 0x18;
const FINETUNE: u32 = A6 - 0x14;
const PERIOD: u32 = A6 - 0x10; // words
const NOTE: u32 = A6 - 0x08;
const SP: u32 = A6 - 0x04;
const TREM_COUNT: u32 = A6;
const TREM_STEP: u32 = A6 + 0x04;
const TREM_VOL: u32 = A6 + 0x08;
const PORTA_PERIOD: u32 = A6 + 0x0c; // words
const PORTA_COUNT: u32 = A6 + 0x14;
const ARP_COUNT: u32 = A6 + 0x18;
const ARP_FLAG: u32 = A6 + 0x1c;
const VIB_DELAY: u32 = A6 + 0x20;
const VIB_COUNT: u32 = A6 + 0x24;
const VIB_REL: u32 = A6 + 0x28; // words
const VIB_STEP: u32 = A6 + 0x30;
const VIB_COUNT2: u32 = A6 + 0x34;
/// 0 attack, 1 sustain, 2 release.
const ENV_STATE: u32 = A6 + 0x38;
const ENV_COUNT: u32 = A6 + 0x3c;
const RELEASE_AT: u32 = A6 + 0x40; // words
const ENV_VOL: u32 = A6 + 0x48;
const PH_COUNT: u32 = A6 + 0x4c;
const PH_STEP: u32 = A6 + 0x50;
const PH_POS: u32 = A6 + 0x54;
const FADE_OUT: u32 = A6 + 0x58;
const FADE_COUNT: u32 = A6 + 0x59;
const FADE_VOL: u32 = A6 + 0x5a;
const FADE_SPEED: u32 = A6 + 0x5b;
const FADE_IN: u32 = A6 + 0x5c;
const FI_COUNT: u32 = A6 + 0x5e;
const FI_POS: u32 = A6 + 0x62;
const FI_STEP: u32 = A6 + 0x66;
/// 1 after a note, 2 after a one-shot release: the loop goes out next tick.
const LOOP_FLAG: u32 = A6 + 0x6a;
/// The 32 instruments (longs), then the voices' stacks.
const INSTRUMENTS: u32 = 0x3e4;
const STACKS: u32 = 0x464;
const STACK_SIZE: u32 = 80;
const DEFAULT_INSTRUMENT: u32 = 0x5a4;
/// The empty word a one-shot sample loops on.
const EMPTY: u32 = 0xfdc;

// An instrument: the DefineInstrument opcode ($84, number, length in words) and
// its fields.
const I_LENGTH: u32 = 0x04;
const I_SAMPLING_PERIOD: u32 = 0x06;
const I_FLAGS: u32 = 0x08;
const I_TREM_SPEED: u32 = 0x09;
const I_TREM_STEP: u32 = 0x0a;
const I_TREM_RANGE: u32 = 0x0b;
const I_PORTA_STEP: u32 = 0x0c;
const I_PORTA_SPEED: u32 = 0x0e;
const I_ARP_SPEED: u32 = 0x0f;
const I_VIB_DELAY: u32 = 0x10;
const I_VIB_SPEED: u32 = 0x11;
const I_VIB_STEP: u32 = 0x12;
const I_VIB_AMOUNT: u32 = 0x13;
const I_ATTACK: u32 = 0x14;
const I_DECAY: u32 = 0x15;
const I_SUSTAIN: u32 = 0x16;
const I_RELEASE: u32 = 0x17;
const I_PH_START: u32 = 0x18;
const I_PH_END: u32 = 0x19;
const I_PH_SPEED: u32 = 0x1a;
const I_PH_STEP: u32 = 0x1b;
const I_WAVES: u32 = 0x1c;
const I_OCTAVE: u32 = 0x1d;
const I_FI_START: u32 = 0x1e;
const I_FI_END: u32 = 0x1f;
const I_FI_SPEED: u32 = 0x20;
/// The sustain and release offsets (words; tested together as a long).
const I_SUSTAIN_AT: u32 = 0x22;
const I_RELEASE_AT: u32 = 0x24;
const I_DATA: u32 = 0x26;

// The instrument flags.
const F_ONESHOT: u8 = 0x01;
const F_VIBRATO: u8 = 0x02;
const F_ARPEGGIO: u8 = 0x04;
const F_PHASING: u8 = 0x08;
const F_PORTAMENTO: u8 = 0x10;
const F_NO_RELEASE: u8 = 0x20;
const F_TREMOLO: u8 = 0x40;
const F_FILTER: u8 = 0x80;

fn detect(d: &[u8]) -> bool {
    // the original's check: a length of at most 1000000, an opcode ($80-$8F) at
    // 276 and a subsong; and NostalgicPlayer's, as that one passes most files:
    // the length within the file, channel bytes of at most 15 and the lists'
    // offsets within the file, the lowest at 276
    if d.len() < 278 || be32(d, 0) > 1_000_000 || d[276] >> 4 != 8 || d[4..20].iter().all(|&b| b == 0) {
        return false;
    }
    let len = d.len() as u32;
    be32(d, 0) <= len
        && d[4..20].iter().all(|&b| b <= 15)
        && (0..64).map(|i| be32(d, 20 + 4 * i)).all(|o| o <= len)
        && (0..64).map(|i| be32(d, 20 + 4 * i)).min() == Some(276)
}

pub struct SoundFactory {
    base: u32,
    /// The original's code and BSS hunks' images.
    code: u32,
    bss: u32,
    /// The subsongs' slots.
    slots: Vec<u8>,
    sub: u16,
    /// $9C's transposes (not the original's).
    transpose: [u8; 4],
    /// The clock the running call of the original's code started at, and the
    /// 68000 cycles of the instructions it has run since.
    epoch: u64,
    t: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let top = std::mem::replace(&mut m.alloc_top, true);
    let blk = m.alloc(BSS_BLOCK + CODE_BLOCK);
    m.alloc_top = top;
    if blk == 0 {
        return None;
    }
    let bus = &mut m.bus;
    bus.w32(blk, BSS_SIZE + 8);
    bus.w32(blk + BSS_BLOCK, CODE_SIZE + 8);
    bus.w32(blk + BSS_BLOCK + 4, (blk + 4) >> 2);
    let (bss, code) = (blk + 8, blk + BSS_BLOCK + 8);
    // Check2: the subsongs are the slots with a channel byte
    let slots: Vec<u8> = (0..16).filter(|&i| d[4 + i as usize] != 0).collect();
    let mut s = SoundFactory { base, code, bss, slots, sub: 0, transpose: [0; 4], epoch: 0, t: 0 };
    let mut hw = Hw { bus: &mut m.bus };
    // to InitSound: the host's time and Check2's
    hw.wait(START_TIME + check2(d) / 2);
    s.init_sound(0, &mut hw);
    Some(Box::new(s))
}

/// The 68000 cycles of Check2 ($21A): its search for 276 among the lists'
/// offsets, and its count of the slots with a channel byte.
fn check2(d: &[u8]) -> u64 {
    let search = match (0..64).position(|i| be32(d, 20 + 4 * i) == 276) {
        Some(i) => 23 * (i as u64 + 1) + 10 * i as u64,
        None => 33 * 64,
    };
    let slots = d[4..20].iter().filter(|&&b| b != 0).count() as u64;
    232 + 12 + search + 20 + 29 * 16 + 12 * slots + 237
}

/// Colour clocks of the host's: from the replayer's start to InitSound's call
/// (less Check2's time), from InitSound's return to the timer's start, and from
/// EndSound's return to InitSound's call at a subsong change.
const START_TIME: u64 = 350;
const INIT_TO_TIMER: u64 = 0;
const END_TO_INIT: u64 = 264;

impl Replayer for SoundFactory {
    fn format(&self) -> &'static str {
        "Sound Factory"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        false
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.slots.len() as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // DeliTracker's EndSound ($2F8), then InitSound
        self.begin(hw);
        self.c(146);
        self.end_sound(hw);
        self.c(16 + 152);
        self.at(hw, 0);
        hw.wait(END_TO_INIT);
        self.init_sound(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        // the interrupt ($308), to the call of the player
        self.begin(hw);
        self.c(162);
        self.play(hw);
    }
}

impl SoundFactory {
    fn r8(&self, hw: &Hw, o: u32) -> u8 {
        hw.bus.r8(self.code + o)
    }
    fn r16(&self, hw: &Hw, o: u32) -> u16 {
        hw.bus.r16(self.code + o)
    }
    fn r32(&self, hw: &Hw, o: u32) -> u32 {
        hw.bus.r32(self.code + o)
    }
    fn w8(&self, hw: &mut Hw, o: u32, v: u8) {
        hw.bus.w8(self.code + o, v);
    }
    fn w16(&self, hw: &mut Hw, o: u32, v: u16) {
        hw.bus.w16(self.code + o, v);
    }
    fn w32(&self, hw: &mut Hw, o: u32, v: u32) {
        hw.bus.w32(self.code + o, v);
    }

    /// A call of the original's code starts.
    fn begin(&mut self, hw: &Hw) {
        self.epoch = hw.bus.clock;
        self.t = 0;
    }

    /// The original's instructions of `n` cycles run.
    fn c(&mut self, n: u64) {
        self.t += n;
    }

    /// The instructions of `n` cycles to the end of a write or read run: the
    /// clock goes on to the time they end, as the host counts them (two
    /// cycles a colour clock, from the call's start).
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
    fn vol(&mut self, hw: &mut Hw, n: u64, ch: usize, v: u16) {
        self.at(hw, n);
        hw.vol(ch, v);
    }
    fn per(&mut self, hw: &mut Hw, n: u64, ch: usize, p: u16) {
        self.at(hw, n);
        hw.per(ch, p);
    }

    /// A read of VHPOSR's line byte by an instruction of `n` cycles.
    fn line(&mut self, hw: &mut Hw, n: u64) -> u32 {
        self.at(hw, n);
        hw.bus.beam().0 & 0xff
    }

    /// InitSound ($2CE) for subsong `n`, which plays the first tick.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.begin(hw);
        self.c(160);
        // the n-th slot with a channel byte
        let mask: u16 = self.slots.iter().fold(0, |m, &s| m | 1 << s);
        let mut slot = 0;
        let mut k = 0;
        for b in 0..16 {
            self.c(19);
            if mask & 1 << b != 0 {
                self.c(10);
                if k == n {
                    slot = b;
                    break;
                }
                k += 1;
            }
        }
        // $5CC
        self.c(34 + 560 + 25 * 32);
        let base = self.base;
        self.w8(hw, VOICES_ON, hw.bus.r8(base + 4 + slot));
        for v in 0..4 {
            let a = base.wrapping_add(hw.bus.r32(base + 0x14 + 16 * slot + 4 * v));
            self.w32(hw, START + 4 * v, a);
            self.w32(hw, POS + 4 * v, a);
            self.w16(hw, DURATION + 2 * v, 1);
            for o in [FINETUNE, SP, INSTRUMENT, VOLUME] {
                self.w8(hw, o + v, 0);
            }
        }
        for o in [FADE_OUT, FADE_IN, FADE_VOL, FADE_COUNT, REQUEST] {
            self.w8(hw, o, 0);
        }
        for i in 0..32 {
            self.w32(hw, INSTRUMENTS + 4 * i, self.code + DEFAULT_INSTRUMENT);
        }
        self.transpose = [0; 4];
        self.at(hw, 16);
        hw.bus.write_word(0xDFF09E, 0x00ff);
        self.dma(hw, 16, 0x000f);
        self.c(8);
        self.play(hw);
        // the return to the host, which starts the timer
        self.c(152);
        self.at(hw, 0);
        hw.wait(INIT_TO_TIMER);
    }

    /// EndSound ($6C8): all voices off.
    fn end_sound(&mut self, hw: &mut Hw) {
        for o in [VOICES_ON, FADE_OUT, FADE_IN] {
            self.w8(hw, o, 0);
        }
        self.dma(hw, 76, 0x000f);
        for v in (0..4).rev() {
            self.vol(hw, if v == 3 { 34 } else { 14 }, v, 0);
            self.loc(hw, 18, v, self.bss + 0x400);
            self.len(hw, 14, v, 1);
            self.c(17);
        }
    }

    /// The player ($678): the fade, then the voices from the fourth down.
    fn play(&mut self, hw: &mut Hw) {
        self.c(41);
        let fade_in = self.r8(hw, FADE_IN) != 0;
        if fade_in || {
            self.c(21);
            self.r8(hw, FADE_OUT) != 0
        } {
            self.c(91);
            let d0 = self.r8(hw, FADE_COUNT).wrapping_add(self.r8(hw, FADE_SPEED));
            self.w8(hw, FADE_COUNT, d0 & 3);
            let d1 = d0 >> 2;
            let v = self.r8(hw, FADE_VOL);
            self.c(13);
            if fade_in {
                match v.checked_add(d1) {
                    Some(s) => {
                        self.c(12);
                        self.w8(hw, FADE_VOL, s);
                    }
                    None => {
                        self.c(42);
                        self.w8(hw, FADE_IN, 0);
                        self.w8(hw, FADE_VOL, 0);
                    }
                }
            } else if v == 0 {
                self.c(14 + 12);
                self.w8(hw, FADE_VOL, v.wrapping_sub(d1));
            } else {
                self.c(13);
                let (s, borrow) = v.overflowing_sub(d1);
                if s != 0 {
                    self.c(9);
                }
                if s != 0 && !borrow {
                    self.c(12);
                    self.w8(hw, FADE_VOL, s);
                } else {
                    self.end_sound(hw);
                    self.c(16);
                    return;
                }
            }
        }
        self.c(16);
        for v in (0..4).rev() {
            self.c(37);
            if self.r8(hw, VOICES_ON) & 1 << v != 0 {
                self.c(18);
                self.voice(v, hw);
            }
            self.c(37);
        }
        self.c(16);
    }

    fn instrument(&self, hw: &Hw, v: usize) -> u32 {
        let i = self.r8(hw, INSTRUMENT + v as u32) << 2;
        self.r32(hw, INSTRUMENTS + i as u32)
    }

    fn fetch(&self, hw: &mut Hw, v: usize) -> u8 {
        let p = self.r32(hw, POS + 4 * v as u32);
        self.w32(hw, POS + 4 * v as u32, p.wrapping_add(1));
        hw.bus.r8(p)
    }

    fn fetch_word(&self, hw: &mut Hw, v: usize) -> u16 {
        let h = self.fetch(hw, v) as u16;
        h << 8 | self.fetch(hw, v) as u16
    }

    /// A voice's stack: 80 bytes, indexed by a byte that a byte pushes by 2
    /// and a long by 4.
    fn stack(&self, v: usize, i: u8) -> u32 {
        self.code + STACKS + STACK_SIZE * v as u32 + i as u32
    }

    fn push(&self, hw: &mut Hw, v: usize, val: u32, long: bool) {
        let sp = SP + v as u32;
        let i = self.r8(hw, sp);
        let a = self.stack(v, i);
        if long {
            hw.bus.w32(a, val);
        } else {
            hw.bus.w8(a, val as u8);
        }
        self.w8(hw, sp, i.wrapping_add(if long { 4 } else { 2 }));
    }

    fn pull(&self, hw: &mut Hw, v: usize, long: bool) -> u32 {
        let sp = SP + v as u32;
        let i = self.r8(hw, sp).wrapping_sub(if long { 4 } else { 2 });
        self.w8(hw, sp, i);
        let a = self.stack(v, i);
        if long {
            hw.bus.r32(a)
        } else {
            hw.bus.r8(a) as u32
        }
    }

    /// A voice ($72E): its note goes on, or the list goes on to the next note or
    /// pause.
    fn voice(&mut self, v: usize, hw: &mut Hw) {
        let bit = 1u16 << v;
        let vo = v as u32;
        let mut a4 = self.instrument(hw, v);
        self.c(18 + 70 + 41);
        let d = self.r16(hw, DURATION + 2 * vo).wrapping_sub(1);
        self.w16(hw, DURATION + 2 * vo, d);
        if d != 0 {
            return self.modulate(v, a4, hw);
        }
        loop {
            // the fetch and the test for a note
            self.c(90 + 21);
            let op = self.fetch(hw, v);
            if op & 0x80 == 0 {
                if self.note(v, a4, op, hw) {
                    return;
                }
                continue;
            }
            let k = op & 0x7f;
            // the dispatch; then each opcode's cycles, to its RTS
            self.c(68);
            let flag = |s: &mut Self, hw: &mut Hw, f: u8| -> bool {
                let on = s.fetch(hw, v) != 0;
                let b = hw.bus.r8(a4 + I_FLAGS);
                hw.bus.w8(a4 + I_FLAGS, if on { b | f } else { b & !f });
                on
            };
            let set = |s: &mut Self, hw: &mut Hw, o: u32| {
                let b = s.fetch(hw, v);
                hw.bus.w8(a4 + o, b);
            };
            let mut go_on = k != 0;
            match k {
                0 => {
                    self.c(278);
                    let w = self.fetch_word(hw, v);
                    self.w16(hw, DURATION + 2 * vo, w);
                }
                1 => {
                    self.c(120);
                    let b = self.fetch(hw, v);
                    self.w8(hw, VOLUME + vo, b);
                }
                2 => {
                    self.c(120);
                    let b = self.fetch(hw, v);
                    self.w8(hw, FINETUNE + vo, b);
                }
                3 => {
                    self.c(208);
                    let b = self.fetch(hw, v);
                    self.w8(hw, INSTRUMENT + vo, b);
                    a4 = self.instrument(hw, v);
                }
                4 => {
                    // the instrument is the opcode's address
                    self.c(452);
                    let n = self.fetch(hw, v) << 2;
                    let at = self.r32(hw, POS + 4 * vo).wrapping_sub(2);
                    self.w32(hw, INSTRUMENTS + n as u32, at);
                    let l = self.fetch_word(hw, v) as u32;
                    self.w32(hw, POS + 4 * vo, at.wrapping_add(l << 1));
                }
                5 => {
                    self.c(232);
                    let p = self.pull(hw, v, true);
                    self.w32(hw, POS + 4 * vo, p);
                }
                6 | 7 => {
                    self.c(if k == 6 { 800 } else { 580 });
                    let h = self.fetch_word(hw, v) as u32;
                    let off = h << 16 | self.fetch_word(hw, v) as u32;
                    let p = self.r32(hw, POS + 4 * vo);
                    if k == 6 {
                        self.push(hw, v, p, true);
                    }
                    self.w32(hw, POS + 4 * vo, p.wrapping_add(off));
                }
                8 => {
                    self.c(516);
                    let b = self.fetch(hw, v);
                    self.push(hw, v, b as u32, false);
                    let p = self.r32(hw, POS + 4 * vo);
                    self.push(hw, v, p, true);
                }
                9 => {
                    let a3 = self.pull(hw, v, true);
                    let c = (self.pull(hw, v, false) as u8).wrapping_sub(1);
                    if c != 0 {
                        self.c(839);
                        self.w32(hw, POS + 4 * vo, a3);
                        self.push(hw, v, c as u32, false);
                        self.push(hw, v, a3, true);
                    } else {
                        self.c(425);
                    }
                }
                10 => {
                    self.c(174);
                    let b = self.fetch(hw, v);
                    self.w8(hw, FADE_SPEED, b);
                    self.w8(hw, FADE_IN, 0);
                    self.w8(hw, FADE_COUNT, 0);
                    self.w8(hw, FADE_OUT, 1);
                }
                11 => self.c(16),
                12 => {
                    self.c(32);
                    let r = self.r8(hw, REQUEST).wrapping_add(1);
                    self.w8(hw, REQUEST, r);
                }
                13 => {
                    self.c(48);
                    let s = self.r32(hw, START + 4 * vo);
                    self.w32(hw, POS + 4 * vo, s);
                }
                14 => {
                    self.dma(hw, 36, bit);
                    let on = self.r8(hw, VOICES_ON) & (bit as u8 ^ 0x0f);
                    self.w8(hw, VOICES_ON, on);
                    self.vol(hw, 70, v, 0);
                    self.c(44);
                    go_on = false;
                }
                15 => {
                    self.c(187);
                    let b = self.fetch(hw, v);
                    self.w8(hw, FADE_SPEED, b);
                    self.w8(hw, FADE_IN, 1);
                    if self.r8(hw, FADE_VOL) == 0 {
                        self.c(16);
                        self.w8(hw, FADE_VOL, 1);
                    }
                    self.w8(hw, FADE_COUNT, 0);
                    self.w8(hw, FADE_OUT, 0);
                }
                16 => {
                    set(self, hw, I_ATTACK);
                    set(self, hw, I_DECAY);
                    set(self, hw, I_SUSTAIN);
                    // a release flag of 0 sets the no-release flag
                    let on = self.fetch(hw, v) != 0;
                    let b = hw.bus.r8(a4 + I_FLAGS);
                    if on {
                        self.c(557);
                        hw.bus.w8(a4 + I_FLAGS, b & !F_NO_RELEASE);
                        set(self, hw, I_RELEASE);
                    } else {
                        self.c(445);
                        hw.bus.w8(a4 + I_FLAGS, b | F_NO_RELEASE);
                    }
                }
                17 | 18 => {
                    self.c(36);
                    let b = hw.bus.r8(a4 + I_FLAGS);
                    hw.bus.w8(a4 + I_FLAGS, if k == 17 { b | F_ONESHOT } else { b & !F_ONESHOT });
                }
                19..=24 => {
                    let (f, fields, cycles): (u8, &[u32], u64) = match k {
                        19 => (F_VIBRATO, &[I_VIB_DELAY, I_VIB_SPEED, I_VIB_STEP, I_VIB_AMOUNT], 547),
                        20 => (F_ARPEGGIO, &[I_ARP_SPEED], 241),
                        21 => (F_PHASING, &[I_PH_START, I_PH_END, I_PH_SPEED, I_PH_STEP], 547),
                        22 => (F_PORTAMENTO, &[I_PORTA_SPEED], 501),
                        23 => (F_TREMOLO, &[I_TREM_SPEED, I_TREM_STEP, I_TREM_RANGE], 445),
                        _ => (F_FILTER, &[I_FI_START, I_FI_END, I_FI_SPEED], 445),
                    };
                    if flag(self, hw, f) {
                        self.c(cycles);
                        for &o in fields {
                            set(self, hw, o);
                        }
                        if k == 22 {
                            let w = self.fetch_word(hw, v);
                            hw.bus.w16(a4 + I_PORTA_STEP, w);
                        }
                    } else {
                        self.c(139);
                    }
                }
                25 => {
                    let w = self.fetch_word(hw, v);
                    self.w16(hw, DURATION + 2 * vo, w);
                    self.dma(hw, 298, bit);
                    self.c(32);
                    go_on = false;
                }
                26 => {
                    // the LED (the audio filter), as BCLR/BSET on CIA-A's PRA
                    self.c(143);
                    let on = self.fetch(hw, v) != 0;
                    let pra = hw.bus.read_byte(0xBFE001);
                    hw.bus.write_byte(0xBFE001, if on { pra & !2 } else { pra | 2 });
                }
                27 => {
                    let b = self.fetch(hw, v);
                    if b != self.r8(hw, REQUEST) {
                        self.c(187);
                        let p = self.r32(hw, POS + 4 * vo).wrapping_sub(2);
                        self.w32(hw, POS + 4 * vo, p);
                        self.w16(hw, DURATION + 2 * vo, 1);
                        go_on = false;
                    } else {
                        self.c(127);
                    }
                }
                // not the original's: as a fetch and an RTS
                28 => {
                    self.c(90 + 16);
                    self.transpose[v] = self.fetch(hw, v);
                }
                _ => self.c(16),
            }
            self.c(21);
            if !go_on {
                self.c(16);
                return;
            }
        }
    }

    /// A note ($B18). Whether the list stops here (a length of 0 goes on).
    fn note(&mut self, v: usize, a4: u32, note: u8, hw: &mut Hw) -> bool {
        let bit = 1u16 << v;
        let vo = v as u32;
        let note = note.wrapping_add(self.transpose[v]) & 0x7f;
        self.dma(hw, 36, bit);
        self.c(45);
        self.w8(hw, NOTE + vo, note);
        let d3 = hw.bus.r8(a4 + I_FLAGS);
        if d3 & F_PORTAMENTO != 0 {
            self.c(42);
            let p = self.r16(hw, PERIOD + 2 * vo);
            self.w16(hw, PORTA_PERIOD + 2 * vo, p);
            self.w8(hw, PORTA_COUNT + vo, 1);
        }
        self.c(18);
        let p = self.period(hw, v, a4);
        self.c(14);
        self.w16(hw, PERIOD + 2 * vo, p);
        self.c(18 + 86 + 2 * 72 + 27);
        let raw = self.fetch_word(hw, v);
        let dur = raw & 0x7fff;
        if dur == 0 {
            self.c(18);
            return false;
        }
        self.c(39);
        self.w16(hw, DURATION + 2 * vo, dur);
        let decay = hw.bus.r8(a4 + I_DECAY);
        let at = if decay != 0 {
            self.c(26);
            decay.wrapping_add(hw.bus.r8(a4 + I_ATTACK)) as u16
        } else {
            self.c(8);
            dur >> 1
        };
        self.c(51);
        self.w16(hw, RELEASE_AT + 2 * vo, at);
        self.w8(hw, LOOP_FLAG + vo, 1);
        if d3 & F_ARPEGGIO != 0 {
            self.c(40);
            self.w8(hw, ARP_FLAG + vo, 0);
            self.w8(hw, ARP_COUNT + vo, hw.bus.r8(a4 + I_ARP_SPEED));
        }
        self.c(19);
        if d3 & F_VIBRATO != 0 {
            self.c(31);
            let d = hw.bus.r8(a4 + I_VIB_DELAY);
            self.w8(hw, VIB_DELAY + vo, d);
            if d == 0 {
                self.c(84);
                self.w8(hw, VIB_STEP + vo, hw.bus.r8(a4 + I_VIB_STEP));
                self.w16(hw, VIB_REL + 2 * vo, 0);
                self.w8(hw, VIB_COUNT + vo, hw.bus.r8(a4 + I_VIB_SPEED));
                self.w8(hw, VIB_COUNT2 + vo, hw.bus.r8(a4 + I_VIB_AMOUNT));
            }
        }
        self.c(19);
        if d3 & F_TREMOLO != 0 {
            self.c(66);
            self.w8(hw, TREM_COUNT + vo, 1);
            self.w8(hw, TREM_STEP + vo, hw.bus.r8(a4 + I_TREM_STEP).wrapping_neg());
            self.w8(hw, TREM_VOL + vo, 0);
        }
        self.c(17);
        if raw & 0x8000 == 0 {
            self.c(21);
            if hw.bus.r8(a4 + I_ATTACK) != 0 {
                self.c(56);
                for o in [ENV_VOL, ENV_STATE, ENV_COUNT] {
                    self.w8(hw, o + vo, 0);
                }
            } else {
                self.c(40);
                self.w8(hw, ENV_STATE + vo, 1);
                self.w8(hw, ENV_VOL + vo, hw.bus.r8(a4 + I_SUSTAIN));
            }
        }
        self.c(19);
        if d3 & F_PHASING == 0 {
            self.loc(hw, 38, v, a4 + I_DATA);
            self.c(22);
        } else {
            self.c(84);
            self.w8(hw, PH_COUNT + vo, hw.bus.r8(a4 + I_PH_SPEED));
            self.w8(hw, PH_STEP + vo, hw.bus.r8(a4 + I_PH_STEP));
            self.w8(hw, PH_POS + vo, hw.bus.r8(a4 + I_PH_START));
            let a1 = self.mix(hw, v, a4);
            self.loc(hw, 18, v, a1);
            self.c(12);
        }
        self.c(19);
        if d3 & F_FILTER != 0 {
            self.c(80);
            self.w8(hw, FI_COUNT + vo, hw.bus.r8(a4 + I_FI_SPEED));
            self.w8(hw, FI_POS + vo, hw.bus.r8(a4 + I_FI_START));
            self.w8(hw, FI_STEP + vo, 1);
            let a1 = self.filter(hw, v, a4);
            self.loc(hw, 18, v, a1);
            self.c(12);
        }
        self.c(25);
        if hw.bus.r32(a4 + I_SUSTAIN_AT) != 0 {
            self.len(hw, 26, v, hw.bus.r16(a4 + I_RELEASE_AT));
            self.c(22);
        } else {
            self.len(hw, 26, v, hw.bus.r16(a4 + I_LENGTH));
            self.c(12);
        }
        self.c(18);
        self.out(hw, v, d3);
        // five line waits on VHPOSR, then the DMA on
        self.c(4);
        for _ in 0..5 {
            let l = self.line(hw, 12);
            loop {
                let m = self.line(hw, 12);
                self.c(9);
                if m != l {
                    break;
                }
            }
            self.c(10);
        }
        self.dma(hw, 40, 0x8000 | bit);
        self.c(16);
        true
    }

    /// A note's period ($FDE).
    fn period(&mut self, hw: &mut Hw, v: usize, a4: u32) -> u16 {
        self.c(179);
        let note = self.r8(hw, NOTE + v as u32);
        let (oct, n) = (note / 12, (note % 12) as usize);
        let sp = hw.bus.r16(a4 + I_SAMPLING_PERIOD);
        if sp == 0 {
            // a sample of several octaves' waveforms
            self.c(55);
            let mut d0 = SAMPLES[n] as u32;
            let waves = hw.bus.r8(a4 + I_WAVES);
            if waves != 1 {
                self.c(70);
                d0 = (d0 & 0xffff) * waves as u32;
            }
            self.c(25);
            let len = hw.bus.r16(a4 + I_LENGTH);
            if len != 1 {
                self.c(148);
                if len == 0 {
                    // the original's division by zero traps
                    return 0;
                }
                if d0 / len as u32 <= 0xffff {
                    d0 /= len as u32;
                }
                d0 &= 0xffff;
            }
            self.c(if oct == 0 { 13 } else { 36 * oct as u64 } + 24);
            (d0 >> oct << 1) as u16
        } else {
            self.c(154);
            let m = MULTIPLY[n];
            let mut d0 = (((sp as u32 * m as u32) << 1) >> 16) as u16;
            let mut o = hw.bus.r8(a4 + I_OCTAVE);
            loop {
                self.c(13);
                if o == oct {
                    break;
                }
                self.c(9 + 22);
                if o < oct {
                    d0 >>= 1;
                    o += 1;
                } else {
                    d0 <<= 1;
                    o -= 1;
                }
            }
            self.c(56);
            d0
        }
    }

    /// The effects of a tick without a new note ($C86).
    fn modulate(&mut self, v: usize, a4: u32, hw: &mut Hw) {
        let vo = v as u32;
        self.c(35);
        let d3 = hw.bus.r8(a4 + I_FLAGS);
        let lf = self.r8(hw, LOOP_FLAG + vo);
        if lf != 0 {
            self.c(27);
            let sustain = hw.bus.r32(a4 + I_SUSTAIN_AT) != 0;
            let empty = lf == 2 || {
                self.c(25);
                !sustain && {
                    self.c(19);
                    d3 & F_ONESHOT != 0
                }
            };
            if empty {
                self.len(hw, 18, v, 1);
                self.loc(hw, 42, v, self.code + EMPTY);
                self.c(22);
            } else if sustain {
                let s = hw.bus.r16(a4 + I_SUSTAIN_AT);
                self.loc(hw, 72, v, (a4 + I_DATA).wrapping_add((s as u32) << 1));
                let l = hw.bus.r16(a4 + I_RELEASE_AT).wrapping_sub(s);
                self.len(hw, 50, v, l);
                self.c(12);
            }
        }
        self.w8(hw, LOOP_FLAG + vo, 0);
        self.c(37);
        if d3 & F_TREMOLO != 0 {
            self.c(27);
            let c = self.r8(hw, TREM_COUNT + vo).wrapping_sub(1);
            self.w8(hw, TREM_COUNT + vo, c);
            if c == 0 {
                self.c(89);
                self.w8(hw, TREM_COUNT + vo, hw.bus.r8(a4 + I_TREM_SPEED));
                let step = self.r8(hw, TREM_STEP + vo);
                let tv = self.r8(hw, TREM_VOL + vo).wrapping_add(step);
                self.w8(hw, TREM_VOL + vo, tv);
                if hw.bus.r8(a4 + I_TREM_RANGE) >= tv {
                    self.c(18);
                    self.w8(hw, TREM_STEP + vo, step.wrapping_neg());
                }
            }
        }
        self.c(19);
        if d3 & F_PORTAMENTO != 0 {
            self.c(37);
            let p = self.r16(hw, PERIOD + 2 * vo);
            let act = self.r16(hw, PORTA_PERIOD + 2 * vo);
            if p != act {
                self.c(27);
                let c = self.r8(hw, PORTA_COUNT + vo).wrapping_sub(1);
                self.w8(hw, PORTA_COUNT + vo, c);
                if c == 0 {
                    self.c(57 + 41);
                    self.w8(hw, PORTA_COUNT + vo, hw.bus.r8(a4 + I_PORTA_SPEED));
                    let step = hw.bus.r16(a4 + I_PORTA_STEP);
                    let mut a = act;
                    if p < act {
                        a = a.wrapping_sub(step);
                        if p >= a {
                            self.c(14);
                            a = p;
                        }
                    } else {
                        a = a.wrapping_add(step);
                        if p < a {
                            self.c(24);
                            a = p;
                        }
                    }
                    self.w16(hw, PORTA_PERIOD + 2 * vo, a);
                }
            }
        }
        self.c(19);
        if d3 & F_ARPEGGIO != 0 {
            self.c(27);
            let c = self.r8(hw, ARP_COUNT + vo).wrapping_sub(1);
            self.w8(hw, ARP_COUNT + vo, c);
            if c == 0 {
                self.c(44);
                self.w8(hw, ARP_COUNT + vo, hw.bus.r8(a4 + I_ARP_SPEED));
                let f = self.r8(hw, ARP_FLAG + vo) ^ 1;
                self.w8(hw, ARP_FLAG + vo, f);
            }
        }
        self.c(19);
        if d3 & F_VIBRATO != 0 {
            self.c(23 + 27);
            let d = self.r8(hw, VIB_DELAY + vo);
            if d != 0 {
                self.w8(hw, VIB_DELAY + vo, d - 1);
                if d == 1 {
                    self.c(90);
                    self.w8(hw, VIB_COUNT + vo, 1);
                    self.w16(hw, VIB_REL + 2 * vo, 0);
                    self.w8(hw, VIB_STEP + vo, hw.bus.r8(a4 + I_VIB_STEP));
                    self.w8(hw, VIB_COUNT2 + vo, hw.bus.r8(a4 + I_VIB_AMOUNT));
                }
            } else {
                let c = self.r8(hw, VIB_COUNT + vo).wrapping_sub(1);
                self.w8(hw, VIB_COUNT + vo, c);
                if c == 0 {
                    self.c(89);
                    self.w8(hw, VIB_COUNT + vo, hw.bus.r8(a4 + I_VIB_SPEED));
                    let step = self.r8(hw, VIB_STEP + vo);
                    let r = self.r16(hw, VIB_REL + 2 * vo).wrapping_add(step as i8 as u16);
                    self.w16(hw, VIB_REL + 2 * vo, r);
                    let c2 = self.r8(hw, VIB_COUNT2 + vo).wrapping_sub(1);
                    self.w8(hw, VIB_COUNT2 + vo, c2);
                    if c2 == 0 {
                        self.c(52);
                        self.w8(hw, VIB_COUNT2 + vo, hw.bus.r8(a4 + I_VIB_AMOUNT) << 1);
                        self.w8(hw, VIB_STEP + vo, step.wrapping_neg());
                    }
                }
            }
        }
        self.envelope(v, a4, d3, hw);
        let mut mix = false;
        self.c(23);
        if d3 & F_PHASING != 0 {
            self.c(27);
            let c = self.r8(hw, PH_COUNT + vo).wrapping_sub(1);
            self.w8(hw, PH_COUNT + vo, c);
            if c == 0 {
                self.c(89);
                self.w8(hw, PH_COUNT + vo, hw.bus.r8(a4 + I_PH_SPEED));
                let step = self.r8(hw, PH_STEP + vo);
                let d0 = self.r8(hw, PH_POS + vo) as i16 + step as i8 as i16;
                self.w8(hw, PH_POS + vo, d0 as u8);
                let turn = d0 < 0 || {
                    self.c(29);
                    d0 >= hw.bus.r8(a4 + I_PH_END) as i16 || {
                        self.c(25);
                        d0 <= hw.bus.r8(a4 + I_PH_START) as i16
                    }
                };
                if turn {
                    self.c(18);
                    self.w8(hw, PH_STEP + vo, step.wrapping_neg());
                }
                self.c(4);
                mix = true;
            }
        }
        self.c(19);
        if d3 & F_FILTER != 0 {
            self.c(27);
            let c = self.r8(hw, FI_COUNT + vo).wrapping_sub(1);
            self.w8(hw, FI_COUNT + vo, c);
            if c == 0 {
                self.c(89);
                self.w8(hw, FI_COUNT + vo, hw.bus.r8(a4 + I_FI_SPEED));
                let step = self.r8(hw, FI_STEP + vo);
                let f = self.r8(hw, FI_POS + vo).wrapping_add(step);
                self.w8(hw, FI_POS + vo, f);
                let turn = f == hw.bus.r8(a4 + I_FI_START) || {
                    self.c(21);
                    f == hw.bus.r8(a4 + I_FI_END)
                };
                if turn {
                    self.c(18);
                    self.w8(hw, FI_STEP + vo, step.wrapping_neg());
                }
                self.c(4);
                mix = true;
            }
        }
        self.c(13);
        if mix {
            self.c(19);
            if d3 & F_PHASING != 0 {
                self.c(18);
                self.mix(hw, v, a4);
            }
            self.c(19);
            if d3 & F_FILTER != 0 {
                self.c(18);
                self.filter(hw, v, a4);
            }
        }
        self.out(hw, v, d3);
    }

    /// The envelope ($DC4).
    fn envelope(&mut self, v: usize, a4: u32, d3: u8, hw: &mut Hw) {
        let vo = v as u32;
        let sustain = hw.bus.r8(a4 + I_SUSTAIN);
        self.c(23);
        if self.r8(hw, ENV_STATE + vo) == 0 {
            // the attack: the sustain level a tick, a step per attack time
            self.c(30);
            let attack = hw.bus.r8(a4 + I_ATTACK);
            let mut c = self.r8(hw, ENV_COUNT + vo).wrapping_add(sustain);
            loop {
                self.c(35);
                if c < attack {
                    break;
                }
                self.c(21);
                if attack == 0 {
                    break;
                }
                self.c(46);
                c -= attack;
                let vol = self.r8(hw, ENV_VOL + vo).wrapping_add(1);
                self.w8(hw, ENV_VOL + vo, vol);
            }
            self.w8(hw, ENV_COUNT + vo, c);
        }
        self.c(23);
        let r = self.r16(hw, RELEASE_AT + 2 * vo);
        if r != 0 {
            self.c(27);
            self.w16(hw, RELEASE_AT + 2 * vo, r - 1);
            if r == 1 {
                self.c(19);
                if d3 & F_NO_RELEASE == 0 {
                    self.c(55);
                    self.w8(hw, ENV_STATE + vo, 2);
                    self.w8(hw, ENV_COUNT + vo, 0);
                    if d3 & F_ONESHOT != 0 {
                        self.c(25);
                        if hw.bus.r32(a4 + I_SUSTAIN_AT) != 0 {
                            let l = hw.bus.r16(a4 + I_LENGTH).wrapping_sub(hw.bus.r16(a4 + I_SUSTAIN_AT));
                            self.len(hw, 38, v, l);
                            self.c(30);
                            self.w8(hw, LOOP_FLAG + vo, 2);
                        }
                    }
                }
            }
        }
        self.c(35);
        let vol = self.r8(hw, ENV_VOL + vo);
        if vol == sustain {
            self.c(23);
            if self.r8(hw, ENV_STATE + vo) == 0 {
                self.c(18);
                self.w8(hw, ENV_STATE + vo, 1);
            }
        }
        self.c(13);
        if vol != 0 {
            self.c(27);
            if self.r8(hw, ENV_STATE + vo) == 2 {
                // the release: a step down per release time
                self.c(30);
                let release = hw.bus.r8(a4 + I_RELEASE);
                let mut c = self.r8(hw, ENV_COUNT + vo).wrapping_add(sustain);
                let mut vol = vol;
                loop {
                    self.c(35);
                    if c < release {
                        break;
                    }
                    self.c(57);
                    c -= release;
                    vol -= 1;
                    if vol == 0 {
                        break;
                    }
                }
                self.w8(hw, ENV_COUNT + vo, c);
                self.w8(hw, ENV_VOL + vo, vol);
            }
        }
    }

    /// The volume and period ($EF8), to its RTS.
    fn out(&mut self, hw: &mut Hw, v: usize, d3: u8) {
        let vo = v as u32;
        self.c(41);
        let mut d0 = self.r8(hw, ENV_VOL + vo) as u16;
        if d3 & F_TREMOLO != 0 {
            self.c(23);
            let t = self.r8(hw, TREM_VOL + vo);
            if t != 0 {
                self.c(76);
                d0 = d0 * t as u16 >> 8;
            }
        }
        self.c(23);
        let vol = self.r8(hw, VOLUME + vo);
        if vol != 0 {
            self.c(76);
            d0 = d0 * vol as u16 >> 8;
        }
        self.c(21);
        if self.r8(hw, FADE_OUT) != 0 || {
            self.c(21);
            self.r8(hw, FADE_IN) != 0
        } {
            self.c(21);
            let f = self.r8(hw, FADE_VOL);
            if f != 0 {
                self.c(76);
                d0 = d0 * f as u16 >> 8;
            }
        }
        // DTP_Volume's table: 64 * 64 >> 6
        self.vol(hw, 137, v, d0);
        self.c(33);
        let mut p = self.r16(hw, PERIOD + 2 * vo);
        if d3 & F_PORTAMENTO != 0 {
            self.c(14);
            p = self.r16(hw, PORTA_PERIOD + 2 * vo);
        }
        self.c(19);
        if d3 & F_VIBRATO != 0 {
            self.c(23);
            if self.r8(hw, VIB_DELAY + vo) == 0 {
                self.c(14);
                p = p.wrapping_add(self.r16(hw, VIB_REL + 2 * vo));
            }
        }
        self.c(19);
        if d3 & F_ARPEGGIO != 0 {
            self.c(23);
            if self.r8(hw, ARP_FLAG + vo) != 0 {
                self.c(8);
                p >>= 1;
            }
        }
        p = p.wrapping_add(self.r8(hw, FINETUNE + vo) as u16);
        self.per(hw, 32, v, p);
        self.c(28);
    }

    /// The phasing ($F84): the sample mixed with itself shifted, into the voice's
    /// buffer, whose address it returns.
    fn mix(&mut self, hw: &mut Hw, v: usize, a4: u32) -> u32 {
        self.c(152);
        let d4 = hw.bus.r16(a4 + I_LENGTH) << 1;
        let a0 = a4 + I_DATA;
        let a1 = self.bss + 256 * v as u32;
        let at = |a: u32, i: u16| a.wrapping_add(i as i16 as u32);
        let mut d3 = d4.wrapping_sub(self.r8(hw, PH_POS + v as u32) as u16);
        let mut d2: u16 = 0;
        loop {
            self.c(79 + 17);
            let s = hw.bus.r8(at(a0, d3)) as i8 as i16 + hw.bus.r8(at(a0, d2)) as i8 as i16;
            hw.bus.w8(at(a1, d2), ((s as u16) >> 1) as u8);
            d3 = d3.wrapping_add(1);
            if d3 == d4 {
                self.c(4);
                d3 = 0;
            }
            d2 = d2.wrapping_add(1);
            if d2 == d4 {
                break;
            }
        }
        self.c(44);
        a1
    }

    /// The filter ($109C): the sample (or the phasing's buffer) smoothed into
    /// the voice's buffer, whose address it returns.
    fn filter(&mut self, hw: &mut Hw, v: usize, a4: u32) -> u32 {
        self.c(173);
        let d2 = hw.bus.r16(a4 + I_LENGTH) << 1;
        let d1 = self.r8(hw, FI_POS + v as u32) as u16;
        let a1 = self.bss + 256 * v as u32;
        let a0 = if hw.bus.r8(a4 + I_FLAGS) & F_PHASING != 0 {
            self.c(4);
            a1
        } else {
            a4 + I_DATA
        };
        self.c(17);
        let at = |a: u32, i: u16| a.wrapping_add(i as i16 as u32);
        if d1 == 1 {
            // a copy, of (length / 2 + 1) longs (64 for 128 words)
            self.c(15);
            if a0 == a1 {
                self.c(76);
                return a1;
            }
            self.c(39);
            let mut n = (d2 >> 2) as u8;
            if n != 0x40 {
                self.c(4);
                n = n.wrapping_add(1);
            }
            let mut o = 0;
            loop {
                self.c(33);
                let l = hw.bus.r32(a0 + o);
                hw.bus.w32(a1 + o, l);
                o += 4;
                n = n.wrapping_sub(1);
                if n == 0 {
                    break;
                }
            }
            self.c(22 + 76);
            return a1;
        }
        if d1 == 0 || d2 == 0 {
            // the original's division by zero traps
            return a1;
        }
        self.c(42);
        let mut d3 = d1 >> 1;
        let mut done = false;
        let mut d0 = self.average(hw, a0, 0, d1, d2);
        while !done {
            self.c(33);
            let mut d4 = d0;
            let mut i = (d1 >> 1).wrapping_add(d3);
            if i >= d2 {
                self.c(4);
                i = i.wrapping_sub(d2);
            }
            self.c(18);
            d0 = self.average(hw, a0, i, d1, d2);
            self.c(17);
            let mut d7 = d0.wrapping_sub(d4);
            let d5: u16 = if (d7 as i16) < 0 {
                self.c(12);
                d7 = d7.wrapping_neg();
                0xffff
            } else {
                self.c(18);
                1
            };
            self.c(16);
            let mut d6: u16 = 0;
            let mut n = d1 as u8;
            loop {
                self.c(31);
                hw.bus.w8(at(a1, d3), d4 as u8);
                d6 = d6.wrapping_add(d7);
                while d1 <= d6 {
                    self.c(18 + 13);
                    d6 -= d1;
                    d4 = d4.wrapping_add(d5);
                }
                self.c(17);
                d3 = d3.wrapping_add(1);
                if d3 >= d2 {
                    self.c(12);
                    d3 = d3.wrapping_sub(d2);
                    done = true;
                }
                self.c(13);
                n = n.wrapping_sub(1);
                if n == 0 {
                    break;
                }
            }
            self.c(27);
        }
        self.c(76);
        a1
    }

    /// The average of `n` bytes from `i` on, wrapping at `len` ($11148).
    fn average(&mut self, hw: &mut Hw, a0: u32, mut i: u16, n: u16, len: u16) -> u16 {
        self.c(40);
        let mut sum: i16 = 0;
        let mut k = n as u8;
        loop {
            self.c(39 + 13);
            sum = sum.wrapping_add(hw.bus.r8(a0.wrapping_add(i as i16 as u32)) as i8 as i16);
            i = i.wrapping_add(1);
            if i == len {
                self.c(4);
                i = 0;
            }
            k = k.wrapping_sub(1);
            if k == 0 {
                break;
            }
        }
        self.c(202);
        (sum as i32 / n as i16 as i32) as u16
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One subsong: voice 0 defines instrument 0 (a looped 8-word sample, no
    /// attack, sustain 64), plays note 24 for 3 ticks and pauses; the other
    /// voices pause. Voice 1 is enabled, voices 2 and 3 are not.
    fn module() -> Vec<u8> {
        let mut list = vec![0x84, 0, 0, 27]; // DefineInstrument 0, 27 words
        let mut ins = vec![0u8; 0x26 - 4];
        ins[0..2].copy_from_slice(&8u16.to_be_bytes()); // 8 words
        ins[2..4].copy_from_slice(&428u16.to_be_bytes()); // sampling period
        ins[0x16 - 4] = 64; // sustain
        ins[0x17 - 4] = 1; // release time
        ins[0x1c - 4] = 1; // waves
        ins[0x1d - 4] = 2; // octave
        list.extend(ins);
        list.extend((0..16).map(|i| (i * 8) as u8));
        list.extend([0x83, 0, 24, 0, 3, 0x80, 0x10, 0x00]);
        let pause = [0x80u8, 0x10, 0x00];
        let mut d = vec![0u8; 276];
        d[4] = 0x03;
        let o0 = 276u32;
        let o1 = o0 + list.len() as u32;
        for k in 0..16 {
            for v in 0..4 {
                let o = if v == 0 && k == 0 { o0 } else { o1 };
                d[20 + 16 * k + 4 * v..24 + 16 * k + 4 * v].copy_from_slice(&o.to_be_bytes());
            }
        }
        d.extend(list);
        d.extend(pause);
        let len = d.len() as u32;
        d[..4].copy_from_slice(&len.to_be_bytes());
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.psf"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Sound Factory");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // ADKCON and DMA off, then the note: DMA off, location, length, volume,
        // period, DMA on
        assert_eq!(reg(0xDFF09E), [0x00ff]);
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..3], &[0x000f, 0x0001, 0x8001]);
        assert_eq!(reg(0xDFF0A4)[0], 8);
        // the sampling period of octave 2 at note 24 (octave 2, C)
        assert_eq!(reg(0xDFF0A6)[0], 428);
        assert_eq!(reg(0xDFF0A8)[0], 64);
        // voice 1 writes volume 0 and period 0 from the second tick on; voices 2
        // and 3 nothing
        assert!(reg(0xDFF0B8).iter().all(|&v| v == 0) && !reg(0xDFF0B8).is_empty());
        assert!(reg(0xDFF0C8).is_empty() && reg(0xDFF0D8).is_empty());
        // the DMA on comes five lines after the DMA off
        let at = |v: u16| log.iter().find(|w| w.0 == 0xDFF096 && w.1 == v).unwrap().2;
        assert!(at(0x8001) - at(0x0001) > 4 * crate::paula::LINE);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
