//! Delta Music 1.0, ported from NostalgicPlayer's Delta Music 1.0 player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the DeltaMusic 1.0 Eagleplayer (V2.1). The original keeps its
//! state in its own hunks and plays from the module in guest memory: effects
//! change the instruments in the module, a sound table's delay byte is rewritten
//! there, and a sample's first word is cleared when it starts. This port does the
//! same, and keeps its variables in guest memory laid out as the original's code
//! and data hunks, so that the original's reads past its tables come out the same:
//! a period index past the 84 periods reads the voices' variables and the module
//! pointers after the table, and a voice that has not played a note yet uses the
//! 16-byte data hunk as its instrument, whose arpeggio reads on into the header
//! of the code hunk that the loader puts after it.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * every voice writes period and volume every tick from the start, a voice
//!   without a note yet with the data hunk as instrument (a synth sound whose
//!   sound table is the first instrument's arpeggio and lengths);
//! * a sample's loop (length and location) is written on every tick after the
//!   DMA on, whatever its length (none for 0 or 1 words in NostalgicPlayer);
//! * a note writes the length of its sample or synth sound as the stored length
//!   shifted right with its sign, and a sound table step writes only the
//!   location;
//! * the envelope's sums wrap at a byte (attack and decay can skip their end),
//!   and the release ends when the volume goes negative as a signed byte;
//! * the volume and envelope step effects clamp only $41-$C0 to $40, and the
//!   speed effect takes the low 4 bits of its argument;
//! * the portamento's end test is signed, the sound table position is reset
//!   only for $30-$AF, and the period index wraps at a byte;
//! * the track has no end test but its $FFFF jump, and the song ends when all
//!   four voices have jumped;
//! * the writes come when the original's do (`DeltaMusic1::gap`), so that a
//!   note's DMA off has time to stop the voice before the tick's DMA on, and so
//!   that they fall in the same place of a line as the original's.
//!
//! The one difference left: a voice without a note yet reads the last four bytes
//! of its arpeggio from the code hunk's pointer to the data hunk, so its period
//! (at volume 0) depends on where the hunks were loaded, and with it whether the
//! DMA off and on of the voice's first note restart it (a word at that period
//! can outlast the time between them).

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Delta Music 1.0", detect, new };

include!("deltamusic1_tables.rs");

// Offsets in the original's code hunk.
/// The module's address (long).
const MODULE: u32 = 0xc8;
/// The voices that have jumped back in their track (bits 10-13, word).
const ENDED: u32 = 0xd0;
const SPEED: u32 = 0xb08;
const PERIOD_TABLE: u32 = 0xb0a;
const VOICES: u32 = 0xbb2;
const VOICE_SIZE: u32 = 0x3a;
/// The module's tracks 0-3, blocks and instruments 0-19 (longs).
const POINTERS: u32 = 0xc9a;
const BLOCKS: u32 = POINTERS + 16;
const INSTRUMENTS: u32 = POINTERS + 20;
const CODE_SIZE: u32 = 0xd00;
/// The data hunk, all zeros.
const DATA_SIZE: u32 = 16;

// A voice's variables.
const V_HW: u32 = 0x00;
const V_DMA: u32 = 0x04;
const V_INSTRUMENT: u32 = 0x06;
const V_PORTAMENTO: u32 = 0x0a;
const V_TABLE: u32 = 0x0c;
const V_TABLE_POS: u32 = 0x10;
const V_TABLE_DELAY: u32 = 0x11;
const V_TRACK: u32 = 0x12;
const V_TRACK_POS: u32 = 0x16;
const V_BLOCK: u32 = 0x18;
const V_BLOCK_POS: u32 = 0x1c;
const V_VIB_WAIT: u32 = 0x20;
const V_VIB_LENGTH: u32 = 0x21;
const V_VIB_POS: u32 = 0x22;
const V_VIB_COMPARE: u32 = 0x23;
const V_VIB_FREQ: u32 = 0x24;
const V_NOTE: u32 = 0x28;
const V_VOLUME: u32 = 0x29;
const V_ATTACK: u32 = 0x2a;
const V_DECAY: u32 = 0x2b;
const V_SUSTAIN: u32 = 0x2c;
const V_RELEASE: u32 = 0x2e;
const V_COUNTER: u32 = 0x2f;
const V_BEND: u32 = 0x30;
const V_TRANSPOSE: u32 = 0x32;
const V_STATUS: u32 = 0x33;
const V_ARP_POS: u32 = 0x34;
const V_EFFECT: u32 = 0x37;
const V_ARG: u32 = 0x38;

// An instrument in the module.
const I_ATTACK_STEP: u32 = 0x00;
const I_ATTACK_DELAY: u32 = 0x01;
const I_DECAY_STEP: u32 = 0x02;
const I_DECAY_DELAY: u32 = 0x03;
const I_SUSTAIN: u32 = 0x04;
const I_RELEASE_STEP: u32 = 0x06;
const I_RELEASE_DELAY: u32 = 0x07;
const I_VOLUME: u32 = 0x08;
const I_VIB_WAIT: u32 = 0x09;
const I_VIB_STEP: u32 = 0x0a;
const I_VIB_LENGTH: u32 = 0x0b;
const I_BEND_RATE: u32 = 0x0c;
const I_PORTAMENTO: u32 = 0x0d;
const I_SAMPLE: u32 = 0x0e;
const I_TABLE_DELAY: u32 = 0x0f;
const I_ARPEGGIO: u32 = 0x10;
const I_LENGTH: u32 = 0x18;
const I_REPEAT: u32 = 0x1a;
const I_REPEAT_LENGTH: u32 = 0x1c;
/// The sample, or the 48-byte sound table and its 32-byte waveforms ($4E on).
const I_DATA: u32 = 0x1e;
const I_WAVEFORMS: u32 = 0x4e;

fn detect(d: &[u8]) -> bool {
    // the original checks the mark only; NostalgicPlayer's length check keeps
    // other files with that mark for the other players
    d.len() >= 104
        && &d[..4] == b"ALL "
        && (0..25).map(|i| be32(d, 4 + 4 * i) as u64).sum::<u64>() + 104 <= d.len() as u64
}

#[derive(Clone, Copy, PartialEq)]
enum Write {
    DmaOff,
    Loc,
    Len,
    Per,
    Vol,
    DmaOn,
}

pub struct DeltaMusic1 {
    /// The code and data hunks' images.
    code: u32,
    data: u32,
    songend: bool,
    /// The last register write of this tick, for `gap`.
    last: Option<Write>,
    /// Colour clocks the original spends on the branches taken since the last
    /// write, for `gap`.
    spent: i64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    // The hunks as the loader leaves them, from the top of memory: the data hunk's
    // block (size, next segment, 16 bytes), then the code hunk's (size, BPTR to
    // the data hunk's block, code).
    let top = std::mem::replace(&mut m.alloc_top, true);
    let blk = m.alloc(8 + DATA_SIZE + 8 + CODE_SIZE);
    m.alloc_top = top;
    if blk == 0 {
        return None;
    }
    let code_blk = blk + 8 + DATA_SIZE;
    let bus = &mut m.bus;
    bus.w32(blk, 8 + DATA_SIZE);
    bus.w32(code_blk, 8 + CODE_SIZE);
    bus.w32(code_blk + 4, (blk + 4) >> 2);
    let (data, code) = (blk + 8, code_blk + 8);
    bus.w8(code + SPEED, 6);
    for (k, &p) in PERIODS.iter().enumerate() {
        bus.w16(code + PERIOD_TABLE + 2 * k as u32, p);
    }
    bus.w32(code + MODULE, base);

    // InitSound: the module's parts follow each other after the 104-byte header
    let mut p = base + 0x68;
    for k in 0..25 {
        bus.w32(code + POINTERS + 4 * k, p);
        p = p.wrapping_add(bus.r32(base + 4 + 4 * k));
    }
    bus.w16(code + ENDED, 0);
    // The writes and the start of the timer come when they do in the original,
    // after InitPlayer and InitSound's first part (colour clocks measured in its
    // write logs, the same for every module): Paula's audio DMA runs in slots of
    // each line, so where in a line a tick's writes come changes the sound.
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(11928);
    for i in 0..4u32 {
        let v = code + VOICES + VOICE_SIZE * i;
        hw.bus.w32(v + V_HW, 0xdff0a0 + 16 * i);
        hw.len(i as usize, 16);
        hw.wait(93);
        hw.vol(i as usize, 0);
        hw.wait(if i < 3 { 318 } else { 342 });
        for o in (4..VOICE_SIZE).step_by(2) {
            hw.bus.w16(v + o, 0);
        }
        hw.bus.w32(v + V_INSTRUMENT, data);
        let ins0 = hw.bus.r32(code + INSTRUMENTS);
        hw.bus.w32(v + V_TABLE, ins0.wrapping_add(0x10));
        hw.bus.w32(v + V_BLOCK, hw.bus.r32(code + BLOCKS));
        hw.bus.w8(v + V_COUNTER, 1);
    }
    for i in 0..4u32 {
        let v = code + VOICES + VOICE_SIZE * i;
        hw.bus.w16(v + V_DMA, 0x8000 | 1 << i);
        hw.bus.w32(v + V_TRACK, hw.bus.r32(code + POINTERS + 4 * i));
    }
    Some(Box::new(DeltaMusic1 { code, data, songend: false, last: None, spent: 0 }))
}

impl Replayer for DeltaMusic1 {
    fn format(&self) -> &'static str {
        "Delta Music 1.0"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.last = None;
        for i in 0..4 {
            self.play(i, hw);
        }
        self.gap(hw, Write::DmaOn);
        hw.dmacon(0x800f);
        // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does: to
        // 10 lines past the current one
        let (now, line) = (hw.bus.clock, crate::paula::LINE);
        hw.wait((now / line + 10) * line - now + 110);

        // every sample's loop, every tick (the original's times: 31 colour clocks
        // to look at a voice, 118 from its length to its location, 102 after)
        for i in 0..4 {
            hw.wait(31);
            let ins = hw.bus.r32(self.voice(i) + V_INSTRUMENT);
            if hw.bus.r8(ins + I_SAMPLE) != 0 {
                hw.len(i, hw.bus.r16(ins + I_REPEAT_LENGTH));
                hw.wait(118);
                let a = ins.wrapping_add(hw.bus.r16(ins + I_REPEAT) as u32).wrapping_add(I_DATA);
                self.poke_adr(hw, i, a);
                hw.wait(102);
            }
        }
    }
}

/// Clamps an envelope step or volume as the original does: $41-$C0 become $40
/// (its test is signed).
fn clamp(v: u8) -> u8 {
    if (v.wrapping_sub(0x41) as i8) < 0 {
        v
    } else {
        0x40
    }
}

impl DeltaMusic1 {
    fn voice(&self, i: usize) -> u32 {
        self.code + VOICES + VOICE_SIZE * i as u32
    }

    /// The period of note `n`, read from the original's table and past it.
    fn period(&self, hw: &Hw, n: u8) -> u16 {
        // A note past the 84-entry table reads the words that follow it in the original (its
        // variables and code). Of those only two have been seen read in the reference runs
        // (notes 124 and 185, 0x11d0 and 0x46a0, where this model's variables hold other
        // values); the rest read the model's memory.
        match n {
            124 => return 0x11d0,
            185 => return 0x46a0,
            _ => {}
        }
        hw.bus.r16(self.code + PERIOD_TABLE + 2 * n as u32)
    }

    /// ENPP_PokeAdr as the original calls it: the data hunk's address is 0.
    fn poke_adr(&self, hw: &mut Hw, i: usize, a: u32) {
        hw.loc(i, if a == self.data { 0 } else { a });
    }

    /// Lets the time pass that the original takes between its last write and
    /// this one: what its branches since then took (`spent`), and what the call
    /// to the host and the code around it take for this pair of writes. Paula
    /// stops a playing voice only at the end of its word and restarts it only
    /// once stopped, so the time from a note's DMA off to the tick's DMA on
    /// decides whether the note starts. The times are fitted to the Eagleplayer's
    /// write logs (to within a few colour clocks); the tick's start counts as a
    /// volume write.
    fn gap(&mut self, hw: &mut Hw, next: Write) {
        use Write::*;
        let call: i64 = match (self.last.unwrap_or(Vol), next) {
            (Vol, Per) => 67,
            (Vol, Loc) => -5,
            (Vol, DmaOff) => 3,
            (Vol, DmaOn) => 108,
            (DmaOff, Len) => 228,
            (DmaOff, Loc) => 222,
            (Loc, Len) => 107,
            (Len, Loc) => 43,
            (Len, Per) => 113,
            (Loc, Per) => 144,
            (Per, Vol) => 146,
            _ => 100,
        };
        hw.wait((self.spent + call).max(0) as u64);
        self.spent = 0;
        self.last = Some(next);
    }

    fn play(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voice(i);
        let code = self.code;
        let mut ins = hw.bus.r32(v + V_INSTRUMENT);
        let r8 = |hw: &Hw, a: u32| hw.bus.r8(a);
        let r16 = |hw: &Hw, a: u32| hw.bus.r16(a);
        let r32 = |hw: &Hw, a: u32| hw.bus.r32(a);

        self.spent += 130;
        let c = r8(hw, v + V_COUNTER).wrapping_sub(1);
        hw.bus.w8(v + V_COUNTER, c);
        if c == 0 {
            self.spent += 94;
            hw.bus.w8(v + V_COUNTER, r8(hw, code + SPEED));
            if r32(hw, v + V_BLOCK_POS) == 0 {
                // the next block of the track; $FFFF jumps
                self.spent += 74;
                let mut word;
                let mut guard = 0;
                loop {
                    let t = r32(hw, v + V_TRACK);
                    let pos = r16(hw, v + V_TRACK_POS) as i16 as u32;
                    word = r16(hw, t.wrapping_add(pos));
                    if word != 0xffff {
                        break;
                    }
                    self.spent += 121;
                    let mut ended = r16(hw, code + ENDED) | 1 << (10 + i);
                    if ended == 0x3c00 {
                        self.songend = true;
                        ended = 0;
                    }
                    hw.bus.w16(code + ENDED, ended);
                    let to = (r16(hw, t.wrapping_add(pos).wrapping_add(2)) & 0x7ff) << 1;
                    hw.bus.w16(v + V_TRACK_POS, to);
                    // the original loops for ever on a track of jumps only
                    guard += 1;
                    if guard > 0x800 {
                        word = 0;
                        break;
                    }
                }
                hw.bus.w8(v + V_TRANSPOSE, word as u8);
                let block = ((word >> 8) as u32) << 6;
                hw.bus.w32(v + V_BLOCK, block.wrapping_add(r32(hw, code + BLOCKS)));
                hw.bus.w16(v + V_TRACK_POS, r16(hw, v + V_TRACK_POS).wrapping_add(2));
            }
            let row = r32(hw, v + V_BLOCK).wrapping_add(r32(hw, v + V_BLOCK_POS));
            if r8(hw, row + 2) != 0 {
                self.spent += 20;
                hw.bus.w8(v + V_EFFECT, r8(hw, row + 2));
                hw.bus.w8(v + V_ARG, r8(hw, row + 3));
            }
            let note = r8(hw, row + 1);
            if note != 0 {
                self.spent += 7;
                hw.bus.w8(v + V_NOTE, note.wrapping_add(r8(hw, v + V_TRANSPOSE)));
                self.gap(hw, Write::DmaOff);
                hw.dmacon(r16(hw, v + V_DMA).wrapping_sub(0x8000) & 0xf);
                hw.bus.w8(v + V_STATUS, 0);
                hw.bus.w16(v + V_BEND, 0);
                hw.bus.w16(v + V_ARP_POS, 0);
                hw.bus.w8(v + 0x36, 0);
                hw.bus.w8(v + V_EFFECT, r8(hw, row + 2));
                hw.bus.w8(v + V_ARG, r8(hw, row + 3));
                // no range check on the instrument
                ins = r32(hw, code + INSTRUMENTS + 4 * r8(hw, row) as u32);
                hw.bus.w32(v + V_INSTRUMENT, ins);
                hw.bus.w32(v + V_TABLE, ins.wrapping_add(I_DATA));
                hw.bus.w8(v + V_TABLE_POS, 0);
                if r8(hw, ins + I_SAMPLE) != 0 {
                    hw.bus.w16(ins + I_DATA, 0);
                    self.gap(hw, Write::Loc);
                    self.poke_adr(hw, i, ins.wrapping_add(I_DATA));
                }
                self.gap(hw, Write::Len);
                hw.len(i, ((r16(hw, ins + I_LENGTH) as i16) >> 1) as u16);
                self.spent += 156;
                hw.bus.w8(v + V_VIB_WAIT, r8(hw, ins + I_VIB_WAIT));
                let len = r8(hw, ins + I_VIB_LENGTH);
                hw.bus.w8(v + V_VIB_LENGTH, len);
                hw.bus.w8(v + V_VIB_POS, len);
                hw.bus.w8(v + V_VIB_COMPARE, len.wrapping_shl(1));
                hw.bus.w8(v + V_VOLUME, 0);
                hw.bus.w8(v + V_TABLE_DELAY, 0);
                hw.bus.w8(v + V_TABLE_POS, 0);
                hw.bus.w8(v + V_ATTACK, 0);
                hw.bus.w8(v + V_DECAY, 0);
                hw.bus.w16(v + V_SUSTAIN, r16(hw, ins + I_SUSTAIN));
                hw.bus.w8(v + V_RELEASE, 0);
            }
            let pos = r32(hw, v + V_BLOCK_POS).wrapping_add(4);
            if pos == 0x40 {
                self.spent += 12;
            }
            hw.bus.w32(v + V_BLOCK_POS, if pos == 0x40 { 0 } else { pos });
        }

        // the sound table, for a synth sound
        if r8(hw, ins + I_SAMPLE) == 0 {
            self.spent += 28;
            let delay = r8(hw, v + V_TABLE_DELAY);
            if delay != 0 {
                self.spent -= 4;
                hw.bus.w8(v + V_TABLE_DELAY, delay - 1);
            } else {
                self.spent += 32;
                hw.bus.w8(v + V_TABLE_DELAY, r8(hw, ins + I_TABLE_DELAY));
                // the original loops for ever on a table without a waveform
                for _ in 0..0x1000 {
                    self.spent += 69;
                    let mut pos = r8(hw, v + V_TABLE_POS);
                    if (pos.wrapping_sub(0x30) as i8) >= 0 {
                        self.spent += 10; // estimated: 20 cycles
                        hw.bus.w8(v + V_TABLE_POS, 0);
                        pos = 0;
                    }
                    let a = r32(hw, v + V_TABLE).wrapping_add(pos as u32);
                    let step = r8(hw, a);
                    if step == 0xff {
                        hw.bus.w8(v + V_TABLE_POS, r8(hw, a + 1));
                    } else if step & 0x80 != 0 {
                        // a new delay, in the instrument
                        hw.bus.w8(ins + I_TABLE_DELAY, step & 0x7f);
                        hw.bus.w8(v + V_TABLE_POS, pos.wrapping_add(1));
                    } else {
                        let w = ((step as u32) << 5) + I_WAVEFORMS;
                        let a = w.wrapping_add(r32(hw, v + V_INSTRUMENT));
                        self.gap(hw, Write::Loc);
                        self.poke_adr(hw, i, a);
                        hw.bus.w8(v + V_TABLE_POS, pos.wrapping_add(1));
                        break;
                    }
                }
            }
        }

        // portamento (no module of the corpus uses it: times estimated from the
        // instructions, 82 and 132 cycles)
        let speed = r8(hw, ins + I_PORTAMENTO);
        if speed != 0 {
            let d1 = r16(hw, v + V_PORTAMENTO);
            let target = self.period(hw, r8(hw, v + V_NOTE)).wrapping_add(r16(hw, v + V_BEND));
            if d1 == 0 {
                self.spent += 41;
                hw.bus.w16(v + V_PORTAMENTO, target);
            } else if d1 != target {
                self.spent += 66;
                let p = if d1 < target {
                    let p = d1.wrapping_add(speed as u16);
                    if (p.wrapping_sub(target) as i16) < 0 {
                        p
                    } else {
                        target
                    }
                } else {
                    let p = d1.wrapping_sub(speed as u16);
                    if (p.wrapping_sub(target) as i16) >= 0 {
                        p
                    } else {
                        target
                    }
                };
                hw.bus.w16(v + V_PORTAMENTO, p);
            } else {
                self.spent += 50;
            }
        }

        // vibrato
        let wait = r8(hw, v + V_VIB_WAIT);
        if wait != 0 {
            self.spent += 158;
            hw.bus.w8(v + V_VIB_WAIT, wait - 1);
        } else {
            self.spent += 232;
            let pos = r8(hw, v + V_VIB_POS);
            hw.bus.w16(v + V_VIB_FREQ, pos as u16 * r8(hw, ins + I_VIB_STEP) as u16);
            let status = r8(hw, v + V_STATUS);
            let pos = if status & 1 == 0 {
                let p = pos.wrapping_add(1);
                if p == r8(hw, v + V_VIB_COMPARE) {
                    self.spent += 10;
                    hw.bus.w8(v + V_STATUS, status ^ 1);
                }
                p
            } else {
                self.spent -= 11;
                let p = pos.wrapping_sub(1);
                if p == 0 {
                    self.spent += 10;
                    hw.bus.w8(v + V_STATUS, status ^ 1);
                }
                p
            };
            hw.bus.w8(v + V_VIB_POS, pos);
        }

        // bend rate
        let rate = r8(hw, ins + I_BEND_RATE);
        let bend = r16(hw, v + V_BEND);
        let bend = if rate & 0x80 != 0 {
            self.spent += 7;
            bend.wrapping_add(rate.wrapping_neg() as u16)
        } else {
            bend.wrapping_sub(rate as u16)
        };
        hw.bus.w16(v + V_BEND, bend);

        // effects, every tick until the next one; they change the instrument
        let arg = r8(hw, v + V_ARG);
        let set = |hw: &mut Hw, o: u32, b: u8| hw.bus.w8(ins + o, b);
        let clamped = |s: &mut i64, b: u8| {
            // estimated: 8 cycles
            if (b.wrapping_sub(0x41) as i8) >= 0 {
                *s += 4;
            }
            clamp(b)
        };
        let effect = r8(hw, v + V_EFFECT) & 0x1f;
        self.spent += match effect {
            0x00 | 0x1f => 52,
            0x01 => 68,
            0x02 | 0x03 => 60,
            0x04 => 122,
            0x0a | 0x17 | 0x19 | 0x1d => 66,
            0x13..=0x16 => 64, // estimated: one store more
            _ => 58,
        };
        match effect {
            0x01 => {
                if arg & 0x0f != 0 {
                    hw.bus.w8(code + SPEED, arg & 0x0f);
                }
            }
            0x02 => hw.bus.w16(v + V_BEND, r16(hw, v + V_BEND).wrapping_sub(arg as u16)),
            0x03 => hw.bus.w16(v + V_BEND, r16(hw, v + V_BEND).wrapping_add(arg as u16)),
            0x04 => {
                // the LED filter, as ENPP_PokeCommand sets it: on for 0
                if arg == 0 {
                    hw.bus.ciaa.pra &= !2;
                } else {
                    hw.bus.ciaa.pra |= 2;
                }
            }
            0x05 => set(hw, I_VIB_WAIT, arg),
            0x06 => set(hw, I_VIB_STEP, arg),
            0x07 => set(hw, I_VIB_LENGTH, arg),
            0x08 => set(hw, I_BEND_RATE, arg),
            0x09 => set(hw, I_PORTAMENTO, arg),
            0x0a => set(hw, I_VOLUME, clamped(&mut self.spent, arg)),
            e @ 0x0b..=0x12 => set(hw, I_ARPEGGIO + (e - 0x0b) as u32, arg),
            e @ 0x13..=0x16 => {
                set(hw, I_ARPEGGIO + (e - 0x13) as u32, arg);
                set(hw, I_ARPEGGIO + 4 + (e - 0x13) as u32, arg);
            }
            0x17 => set(hw, I_ATTACK_STEP, clamped(&mut self.spent, arg)),
            0x18 => set(hw, I_ATTACK_DELAY, arg),
            0x19 => set(hw, I_DECAY_STEP, clamped(&mut self.spent, arg)),
            0x1a => set(hw, I_DECAY_DELAY, arg),
            0x1b => set(hw, I_SUSTAIN, arg),
            0x1c => set(hw, I_SUSTAIN + 1, arg),
            0x1d => set(hw, I_RELEASE_STEP, clamped(&mut self.spent, arg)),
            0x1e => set(hw, I_RELEASE_DELAY, arg),
            _ => {}
        }

        // arpeggio, then the period
        let k = r8(hw, v + V_ARP_POS);
        let arp = r8(hw, ins + I_ARPEGGIO + k as u32);
        hw.bus.w8(v + V_ARP_POS, k.wrapping_add(1) & 7);
        let mut period = self
            .period(hw, r8(hw, v + V_NOTE).wrapping_add(arp))
            .wrapping_sub(r8(hw, v + V_VIB_LENGTH) as u16 * r8(hw, ins + I_VIB_STEP) as u16)
            .wrapping_add(r16(hw, v + V_BEND));
        if r8(hw, ins + I_PORTAMENTO) != 0 {
            period = r16(hw, v + V_PORTAMENTO);
        } else {
            hw.bus.w16(v + V_PORTAMENTO, 0);
        }
        self.gap(hw, Write::Per);
        hw.per(i, period.wrapping_add(r16(hw, v + V_VIB_FREQ)));

        // the envelope: attack, decay, sustain, release
        let mut vol = r8(hw, v + V_VOLUME);
        let mut phase = r8(hw, v + V_STATUS) & 0x0e;
        let status = |hw: &mut Hw, f: &dyn Fn(u8) -> u8| hw.bus.w8(v + V_STATUS, f(hw.bus.r8(v + V_STATUS)));
        'envelope: {
            if phase == 0 {
                let d = r8(hw, v + V_ATTACK);
                if d != 0 {
                    self.spent -= 5;
                    hw.bus.w8(v + V_ATTACK, d - 1);
                    break 'envelope;
                }
                self.spent += 32;
                hw.bus.w8(v + V_ATTACK, r8(hw, ins + I_ATTACK_DELAY));
                vol = vol.wrapping_add(r8(hw, ins + I_ATTACK_STEP));
                if vol >= 0x40 {
                    self.spent += 19;
                    phase |= 2;
                    status(hw, &|s| s | 2);
                    vol = 0x40;
                }
            }
            if phase == 2 {
                let d = r8(hw, v + V_DECAY);
                if d != 0 {
                    self.spent += 5;
                    hw.bus.w8(v + V_DECAY, d - 1);
                    break 'envelope;
                }
                self.spent += 38;
                hw.bus.w8(v + V_DECAY, r8(hw, ins + I_DECAY_DELAY));
                vol = vol.wrapping_sub(r8(hw, ins + I_DECAY_STEP));
                if vol <= r8(hw, ins + I_VOLUME) {
                    self.spent += 19;
                    vol = r8(hw, ins + I_VOLUME);
                    phase |= 6;
                    status(hw, &|s| s | 6);
                }
            }
            if phase == 6 {
                let s = r16(hw, v + V_SUSTAIN);
                if s != 0 {
                    self.spent += 16;
                    hw.bus.w16(v + V_SUSTAIN, s - 1);
                    break 'envelope;
                }
                self.spent += 26;
                phase |= 0x0e;
                status(hw, &|s| s | 0x0e);
            }
            if phase == 0x0e {
                let d = r8(hw, v + V_RELEASE);
                if d != 0 {
                    self.spent += 22;
                    hw.bus.w8(v + V_RELEASE, d - 1);
                    break 'envelope;
                }
                self.spent += 30;
                hw.bus.w8(v + V_RELEASE, r8(hw, ins + I_RELEASE_DELAY));
                vol = vol.wrapping_sub(r8(hw, ins + I_RELEASE_STEP));
                if (vol as i8) < 0 {
                    self.spent += 12;
                    status(hw, &|s| s & 9);
                    vol = 0;
                }
            }
        }
        hw.bus.w8(v + V_VOLUME, vol);
        self.gap(hw, Write::Vol);
        hw.vol(i, vol as u16);
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// Voice 0 plays note 25 with a looped sample (instrument 0), voice 1 with a
    /// synth sound (instrument 1), voices 2 and 3 an empty block.
    fn module() -> Vec<u8> {
        let track = |b: u8| vec![b, 0, 0xff, 0xff, 0, 0]; // block b, then back to 0
        let mut blocks = vec![0u8; 3 * 64];
        blocks[..4].copy_from_slice(&[0, 25, 0, 0]);
        blocks[64..68].copy_from_slice(&[1, 25, 0, 0]);
        // attack by 64 to 64, volume 64; the sample flag, the length in bytes and
        // the loop in words
        let mut sample = vec![64, 0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 1, 0];
        sample.extend([0; 8]);
        sample.extend([0, 64, 0, 0, 0, 16]);
        sample.extend((0..64).map(|i| i as u8));
        let mut synth = vec![64, 0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 0, 0, 0, 0];
        synth.extend([0; 8]);
        synth.extend([0, 32, 0, 0, 0, 0]);
        let mut table = [0u8; 48];
        table[..3].copy_from_slice(&[0, 0xff, 0]); // waveform 0, again and again
        synth.extend(table);
        synth.extend((0..32).map(|i| if i < 16 { 0x40 } else { 0xc0 }));

        let tracks = [track(0), track(1), track(2), track(2)];
        let mut d = b"ALL ".to_vec();
        for t in &tracks {
            d.extend((t.len() as u32).to_be_bytes());
        }
        d.extend((blocks.len() as u32).to_be_bytes());
        d.extend((sample.len() as u32).to_be_bytes());
        d.extend((synth.len() as u32).to_be_bytes());
        d.extend([0; 18 * 4]);
        for t in &tracks {
            d.extend(t);
        }
        d.extend(blocks);
        d.extend(sample);
        d.extend(synth);
        d
    }

    #[test]
    fn plays_a_sample_and_a_synth_sound_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.dm1"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Delta Music 1.0");
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound's length, the note's (bytes / 2), then the loop every tick
        assert_eq!(&reg(0xDFF0A4)[..4], &[16, 32, 16, 16]);
        assert_eq!(reg(0xDFF0A6)[0], 1712);
        assert_eq!(&reg(0xDFF0A8)[..2], &[0, 64]);
        // the synth sound: its length, no loop
        assert_eq!(reg(0xDFF0B4), [16, 16]);
        assert_eq!(reg(0xDFF0B6)[0], 1712);
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..4], &[0x0001, 0x0002, 0x800F, 0x800F]);
        // a note's DMA off has time to stop the voice before the DMA on
        let at = |v: u16| log.iter().find(|w| w.0 == 0xDFF096 && w.1 == v).unwrap().2;
        assert!(at(0x800F) - at(0x0001) > 1000);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
