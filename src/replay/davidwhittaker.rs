//! David Whittaker, ported from NostalgicPlayer's David Whittaker player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! A David Whittaker module is a game's own replayer with its music, and each
//! game's replayer is a little different. NostalgicPlayer recognises the
//! replayer by its code and reads its tables through the code's pc-relative
//! references; so does the original player, the DavidWhittaker Eagleplayer
//! (V4.13), which then runs the module's own code with its writes to the audio
//! registers redirected to a copy of them: a DMACON write sends the copy to Paula
//! (through ENPP_PokeAdr/Len/Per/Vol and ENPP_DMAMask, the fourth voice first)
//! and so does the end of each interrupt. This port does the same: its
//! `Shadow` is the Eagleplayer's copy, and the replayer is played from the
//! module in guest memory, its global variables where the module keeps them.
//!
//! The Eagleplayer finds the replayer's routines and counts the subsongs with
//! its own scans of the code (`ep_check`, `count_subsongs`), which this port
//! follows to the instruction: it plays what the Eagleplayer plays, with the
//! subsongs it reports.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * a voice is turned off the tick before its next row (unless the row ties the
//!   note: `$83`, or in later replayers a duration followed by `$83`), and its
//!   note turns it on again; the loop (or the replayer's empty sample) is
//!   written on the tick after the note;
//! * the mute effect plays the empty sample; the stop effect, and a global fade
//!   that reaches 0, stop the replayer for good;
//! * nothing is range-checked: notes, transposes and arpeggios index the period
//!   table as a byte, and read on past it in the module;
//! * the position list ends at a 0 and starts again (or a jump effect selects
//!   another list), with no song end, as the Eagleplayer reports none;
//! * the samples' pointers, lengths and finetunes are computed into the module's
//!   table as the replayer's InitSound does (as the Eagleplayer patches it: an
//!   unplayable sample becomes 2 bytes at frequency $2000), and the square
//!   waveform is changed in the sample data;
//! * volumes are the replayer's byte arithmetic, and the Eagleplayer's
//!   flush sends the low byte, at most 64; a period of 0 is not sent;
//! * the variants NostalgicPlayer does not tell apart are read from the code
//!   too: three-voice loops, either loop direction, a transpose kept in byte
//!   $2f, half volume and a volume fade, a delay counter with a factor, a
//!   per-voice stop routine, a subsong the init routine cycles, values the
//!   sample routine stores before and after its loop (and its own flag test:
//!   when the Eagleplayer's scan misses that flag, the module's pre-computed
//!   sample pointers stay, as they do in the original), the LED filter the
//!   init routine turns off, the voices' state the module file holds;
//! * not ported: the old replayer (Q-Ball) and the one with $1e-byte voices
//!   (Leviathan), which this port refuses; an effect that starts the game's
//!   sound effect routine (Super League Manager, Emlyn Hughes International
//!   Soccer); and the Eagleplayer's relocation reaching only part of the code
//!   (its scan length comes from a false match of the sample loop in Jaws, whose
//!   voice code then writes Paula directly), where this port always flushes.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "David Whittaker", detect, new };

/// Reads the module as the Eagleplayer's scans do; outside it, zeros.
#[derive(Clone, Copy)]
struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn b(&self, o: i64) -> u8 {
        if o < 0 { 0 } else { self.0.get(o as usize).copied().unwrap_or(0) }
    }
    fn w(&self, o: i64) -> u16 {
        (self.b(o) as u16) << 8 | self.b(o + 1) as u16
    }
    fn sw(&self, o: i64) -> i64 {
        self.w(o) as i16 as i64
    }
    fn l(&self, o: i64) -> u32 {
        (self.w(o) as u32) << 16 | self.w(o + 2) as u32
    }
    /// The target of a pc-relative displacement at `o`.
    fn rel(&self, o: i64) -> i64 {
        o + self.sw(o)
    }
    /// Whether the words at `o` match `pat`: `ANY` matches any word, `hi(b)` a
    /// word with that high byte.
    fn at(&self, o: i64, pat: &[u32]) -> bool {
        pat.iter().enumerate().all(|(i, &p)| {
            let w = self.w(o + 2 * i as i64) as u32;
            match p >> 16 {
                0 => w == p,
                1 => true,
                _ => w >> 8 == p & 0xff,
            }
        })
    }
    fn find(&self, from: i64, to: i64, pat: &[u32]) -> Option<i64> {
        (from..to).step_by(2).find(|&o| self.at(o, pat))
    }
}

const ANY: u32 = 0x1_0000;
const fn hi(b: u8) -> u32 {
    0x2_0000 | b as u32
}

/// What the Eagleplayer's check finds (offsets in the module).
#[derive(Clone, Copy, Debug)]
struct Ep {
    /// The replayer's variables (its `lea 0(pc),a3`).
    base: i64,
    stop: i64,
    init: i64,
    play: i64,
    /// The sample table's "initialised" flag, cleared before each InitSound.
    init_flag: Option<i64>,
    /// The sample info table, when the check found it.
    sample_info: Option<i64>,
    last_subsong: u16,
}

/// The Eagleplayer's DTP_Check2 (and the scans of its InitPlayer), to the
/// instruction: it finds the stop routine by its `move.w #$ff,$dff09e`, the
/// sample table by the replayer's sample loop, the init routine by the call to
/// the stop routine, and the play routine by the extra counter's reset or by its
/// first test of a variable.
fn ep_check(d: &[u8]) -> Option<Ep> {
    let r = Rd(d);
    let lim = (d.len().max(0x2a0a) as u16).wrapping_sub(10) >> 1;
    let mut a0 = 0i64;
    let mut n = lim as i32;
    while r.l(a0) != 0x33fc_00ff {
        a0 += 2;
        n -= 1;
        if n < 0 {
            return None;
        }
    }
    if r.l(a0 + 4) != 0x00df_f09e {
        return None;
    }
    let dmacon = (0..18).map(|k| a0 - 4 - 2 * k).find(|&a| r.l(a) == 0x00df_f096)
        .or_else(|| (0..18).map(|k| a0 + 12 + 2 * k).find(|&a| r.l(a) == 0x00df_f096))?;
    if r.w(dmacon - 2) & 0xfff0 != 0 {
        return None;
    }
    let flag = match (1..=32).map(|k| a0 - 2 * k).find(|&a| r.w(a) == 0x4e75) {
        Some(rts) => r.w(rts - 8) != 0x50eb,
        None => true,
    };
    let lea = match (1..=24).map(|k| a0 - 2 * k).find(|&a| r.w(a) & 0xf1ff == 0x41fa) {
        Some(a) => a,
        None => {
            let mut a1 = a0 + 8;
            let mut found = None;
            for _ in 0..32 {
                let x = r.w(a1);
                if x == 0x4e75 {
                    return None;
                }
                if x & 0xf1ff == 0x41fa {
                    found = Some(a1);
                    break;
                }
                a1 += 2;
            }
            found?
        }
    };
    let base = r.rel(lea + 2);
    let mut stop = None;
    for k in 1..=24 {
        let a = lea - 2 * k;
        let x = r.w(a);
        if x == 0x48e7 || x & 0xfff0 == 0x2f00 {
            stop = Some(a);
            break;
        }
        if x == 0x4e75 {
            stop = Some(a + 2);
            break;
        }
    }
    let stop = stop?;
    let mut code = base;
    if !flag {
        // the replayer's table of entry points: movem, bsr, movem, rts, movem
        if let Some(a) = (0..0x800).map(|k| base + 2 * k).find(|&a| {
            r.at(a, &[0x48e7]) && r.at(a + 8, &[0x4cdf, 0x7f8f, 0x4e75, 0x48e7, 0xf1fe])
        }) {
            code = a;
        }
    }
    // the sample loop: move.l (a0)+,d2 / move.w (a0)+,d3
    let mut init_flag = None;
    let mut sample_info = None;
    let mut a1 = code;
    let mut n = lim as i32;
    let mut lim = lim;
    let mut alt = true;
    while n >= 0 {
        if r.l(a1) == 0x2418_3618 {
            let Some(mq) = (1..=16).map(|k| a1 - 2 * k).find(|&a| r.b(a) == 0x72) else { break };
            let count = 1 + r.b(mq + 1) as u32;
            let mut a2 = a1 - 2;
            for _ in 0..22 {
                a2 -= 2;
                if r.w(a2) | 3 == 0x4a2b && r.b(a2 + 4) == 0x66 {
                    init_flag = Some(base + r.sw(a2 + 2));
                    break;
                }
            }
            let Some(l) = (1..=16).map(|k| a1 - 2 - 2 * k).find(|&a| r.w(a) == 0x41fa) else { break };
            if r.w(l + 4) == 0x4bfa {
                sample_info = Some(r.rel(l + 6));
            }
            let mut a2 = r.rel(l + 2);
            lim = ((a2 - code) as u16) >> 1;
            let mut ok = true;
            for _ in 0..count {
                let len = r.l(a2);
                a2 += 4;
                if len as i32 <= -1 || len > 0x20000 {
                    ok = false;
                    break;
                }
                a2 += len as i64 + 2;
            }
            if !ok {
                break;
            }
            alt = false;
            break;
        }
        a1 += 2;
        n -= 1;
    }
    if alt {
        // no sample loop: the check fails unless a table it then looks for is empty
        let mut a1 = code;
        let mut found = None;
        for _ in 0..0x800 {
            let x = r.w(a1);
            a1 += 2;
            if x == 0xc2fc && r.w(a1) == 0x000a && r.w(a1 + 2) == 0x45fa {
                found = Some(a1 + 4);
                break;
            }
        }
        if let Some(a) = found {
            let disp = r.w(a);
            if disp & 1 == 0 {
                let t = a + disp as i16 as i64;
                let mut best = 0u32;
                for k in 0..50 {
                    let v = r.l(t + 12 + 10 * k);
                    if best < v {
                        if v > 0x493e0 {
                            break;
                        }
                        best = v;
                    }
                }
                if best != 0 {
                    return None;
                }
            }
        }
    }
    // the init routine: the one that calls the stop routine (through its wrapper)
    let mut target = stop;
    let mut wrapped = false;
    let init = 'search: loop {
        let mut a1 = code;
        let mut d2 = (target - code) as u16;
        let mut k = lim as i32;
        loop {
            d2 = d2.wrapping_sub(2);
            let x = r.w(a1);
            a1 += 2;
            if x == 0x6100 && r.w(a1) == d2 {
                break;
            }
            k -= 1;
            if k < 0 {
                return None;
            }
        }
        if !wrapped && r.w(a1 + 2) == 0x4cdf && r.w(a1 - 6) == 0x48e7 {
            target = a1 - 6;
            wrapped = true;
            continue;
        }
        let mut a1 = a1 - 2;
        loop {
            a1 -= 2;
            let x = r.w(a1);
            if x == 0x4e75 {
                break 'search a1 + 2;
            }
            if x | 0x600 == 0x47fa && r.rel(a1 + 2) == base {
                let at_lea = a1;
                for _ in 0..16 {
                    if a1 == base {
                        break 'search a1;
                    }
                    a1 -= 2;
                    match r.w(a1) {
                        0x4e75 => break 'search a1 + 2,
                        0x48e7 => break 'search a1,
                        _ => {}
                    }
                }
                break 'search at_lea;
            }
            if a1 == base {
                break 'search a1;
            }
            if a1 < base {
                break 'search base;
            }
        }
    };
    let last_subsong = count_subsongs(r, init, base);
    // the play routine: by the extra counter's reset, move.b #6,x(a3)
    let mut play = None;
    let mut a1 = base;
    let mut k = lim as i32;
    while k >= 0 {
        if r.l(a1) | 0x0600_0000 == 0x177c_0006 {
            let d1 = r.w(a1 + 4);
            let Some(a2) = (1..=16).map(|j| a1 - 2 * j).find(|&a| r.w(a) == d1) else {
                // the Eagleplayer tries the same place again, for ever
                return None;
            };
            play = (1..=32).map(|j| a2 - 2 * j)
                .find(|&a| r.w(a) | 0x600 == 0x47fa && r.rel(a + 2) == base);
            break;
        }
        a1 += 2;
        k -= 1;
    }
    if play.is_none() {
        // the first lea 0(pc) away from the init and stop routines, with a tst
        // and a beq after it
        let mut a1 = base;
        let mut k = lim as i32;
        while k >= 0 {
            let x = r.w(a1);
            a1 += 2;
            if x | 0x600 == 0x47fa && r.rel(a1) == base {
                let far = |o: i64| {
                    let d = (a1 - o) as u16;
                    let d = if (a1 - o) < 0 { d.wrapping_neg() } else { d };
                    d > 0x14
                };
                if far(init) && far(stop) {
                    if let Some(t) = (1..=16).map(|j| a1 + 2 * j).find(|&a| r.b(a) == 0x4a) {
                        if r.b(t + 4) == 0x67 {
                            play = Some(a1 - 2);
                            break;
                        }
                    }
                }
            }
            k -= 1;
        }
    }
    Some(Ep { base, stop, init, play: play?, init_flag, sample_info, last_subsong })
}

/// The Eagleplayer's subsong count: the subsong table the init routine indexes
/// (`ext.w d0` or `andi.w #$ff,d0`, then `mulu.w #size,d0`), with entries that
/// start with a speed of 2 to 15, or (speed bytes) with position lists, up to the
/// first position list.
fn count_subsongs(r: Rd, init: i64, base: i64) -> u16 {
    let mut a1 = init;
    let mut found = false;
    for _ in 0..100 {
        let x = r.w(a1);
        a1 += 2;
        if x == 0x4880 || (x == 0x0240 && {
            let y = r.w(a1);
            a1 += 2;
            y == 0x00ff
        }) {
            // the mulu
            let mut ok = false;
            for _ in 0..16 {
                let y = r.w(a1);
                a1 += 2;
                if y == 0xc0fc {
                    ok = true;
                    break;
                }
            }
            if ok {
                found = true;
                break;
            }
        }
    }
    if !found {
        return 0;
    }
    let size = r.w(a1) as i16 as i64;
    a1 += 2;
    let lea_before = |a: i64| (1..=32).map(|k| a - 2 * k).find(|&a| r.w(a) == 0x41fa).map(|a| r.rel(a + 2));
    for _ in 0..16 {
        if r.l(a1) == 0x3770_0000 {
            let Some(mut t) = lea_before(a1) else { return 0 };
            let mut n = 0u16;
            for _ in 0..100 {
                t += size;
                let s = r.w(t);
                if !(2..=15).contains(&s) {
                    return n;
                }
                n += 1;
            }
            return 0;
        }
        if r.l(a1) == 0x1230_0000 {
            let Some(mut t) = lea_before(a1) else { return 0 };
            let mut d7: i32 = -1;
            let d5 = ((size as u16) >> 1).wrapping_sub(3) as i16;
            let mut min: u16 = 0xffff;
            let mut end = None;
            'entries: for _ in 0..100 {
                let mut a2 = t;
                t += size;
                if r.w(a2) == 0 {
                    end = Some(t);
                    break;
                }
                a2 += 2;
                let mut d4 = d5;
                while d4 >= 0 {
                    let d3 = r.w(a2);
                    a2 += 2;
                    if d3 == 0 {
                        end = Some(t);
                        break 'entries;
                    }
                    let mut a3 = a2;
                    let mut d0 = d4;
                    while d0 >= 0 {
                        let mut d1 = r.w(a3);
                        a3 += 2;
                        if d1 == 0 {
                            end = Some(t);
                            break 'entries;
                        }
                        if d1 > d3 {
                            d1 = d3;
                        }
                        if min > d1 {
                            min = d1;
                        }
                        d0 -= 1;
                    }
                    d4 -= 1;
                }
                d7 += 1;
            }
            let Some(t) = end else { return 0 };
            if d7 < 0 {
                return 0;
            }
            let rel = (t - base) as u32;
            let m = min as i16 as i32 as u32;
            if m >= rel {
                return d7 as u16;
            }
            let q = ((rel - m) / (size as u32 & 0xffff).max(1)) as u16;
            let q = q.wrapping_sub(1) as i16;
            if q <= 0 {
                return d7 as u16;
            }
            return (d7 as u16).checked_sub(q as u16).unwrap_or(0);
        }
        a1 += 2;
    }
    0
}

fn detect(d: &[u8]) -> bool {
    ep_check(d).is_some_and(|ep| analyse(Rd(d), &ep).is_some())
}

/// How the row before a note turns the voice off.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Off {
    Always,
    /// Unless the next byte is $83 (a tie).
    Not83,
    /// Unless the next byte is $83, or a duration followed by $83.
    Not83Dur,
}

/// A step of a note's index into the period table.
#[derive(Clone, Copy, Debug)]
enum NoteOp {
    /// The sample's transpose.
    SampleTranspose,
    /// The note is kept (for the frames).
    Keep,
    GlobalTranspose(u32),
    Transpose,
}

/// What a track byte with bit 7 set does, by range.
#[derive(Clone, Copy, Debug)]
enum Cmd {
    /// Duration: (byte - $df) times the speed variable.
    Duration { from: u8, speed: u32 },
    Sample { from: u8, table: u32, size: u32 },
    Envelope { from: u8, table: u32 },
    Arpeggio { from: u8, table: u32 },
}

/// The effects, as their handlers' code does them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fx {
    NextPosition,
    Slide,
    Mute,
    /// The rest of the row with the voice off (with `done`, and no loop write).
    Off { done: bool },
    /// The rest of the row: keep the note (and with `dma`, turn the voice on).
    Wait { dma: bool },
    /// Stop the replayer (with `vols`, as the stop routine: volumes 0 too).
    Stop { vols: bool },
    /// The stop routine.
    StopRoutine,
    /// st.b|sf.b $3(a0): the voice's half-volume flag (where byte 3 is that).
    Half(bool),
    /// A flag cleared, then the stop routine.
    ClearStop(u32),
    /// The next byte into a global byte.
    SetByte(u32),
    /// The next byte, sign-extended, into a global word.
    SetWord(u32),
    /// A global fade: its speed and counter.
    Fade(u32, u32),
    Vibrato,
    VibratoOff,
    Transpose,
    PositionList,
    /// Not known: does nothing.
    Nop,
}

/// A write of the init routine (or of the sample routine after its loop) to
/// the replayer's variables.
#[derive(Clone, Copy, Debug)]
enum GOp {
    Byte(u32, u8),
    Word(u32, u16),
    /// addi.w|subi.w #n,x(a3): a finetune adjusted after the sample loop.
    AddWord(u32, u16),
    /// move.l a0,x(a3): the address after the samples (the empty sample).
    StoreA0(u32),
    AddA0(u32),
    /// movea.l x(pc),a0: the square waveform.
    LoadWave(u32),
    /// move.w #n,d1 / move.w|b #v,(a0)+ / dbra d1
    Fill { word: bool, val: u16, count: u16 },
}

fn exec(b: &mut crate::bus::Bus, ops: &[GOp], a0: &mut u32) {
    let mut wave = 0u32;
    for op in ops {
        match *op {
            GOp::Byte(a, x) => b.w8(a, x),
            GOp::Word(a, x) => b.w16(a, x),
            GOp::AddWord(a, x) => b.w16(a, b.r16(a).wrapping_add(x)),
            GOp::StoreA0(a) => b.w32(a, *a0),
            GOp::AddA0(n) => *a0 = a0.wrapping_add(n),
            GOp::LoadWave(a) => wave = b.r32(a),
            GOp::Fill { word, val, count } => {
                for _ in 0..=count {
                    if word {
                        b.w16(wave, val);
                        wave = wave.wrapping_add(2);
                    } else {
                        b.w8(wave, val as u8);
                        wave = wave.wrapping_add(1);
                    }
                }
            }
        }
    }
}

/// Decodes the writes to variables of the straight code at `o` (until `end` or
/// an instruction it does not know, with `stop_unknown`).
fn gops(r: Rd, mut o: i64, end: i64, base: i64, stop_unknown: bool) -> Vec<GOp> {
    let g = |o: i64| (base + r.sw(o)) as u32;
    let mut ops = Vec::new();
    let mut d1 = 0u16;
    while o < end {
        match r.w(o) {
            0x4e75 => break,
            0x51eb => {
                ops.push(GOp::Byte(g(o + 2), 0));
                o += 4;
            }
            0x50eb => {
                ops.push(GOp::Byte(g(o + 2), 0xff));
                o += 4;
            }
            0x377c => {
                ops.push(GOp::Word(g(o + 4), r.w(o + 2)));
                o += 6;
            }
            op @ (0x066b | 0x046b) => {
                let n = r.w(o + 2);
                ops.push(GOp::AddWord(g(o + 4), if op == 0x066b { n } else { n.wrapping_neg() }));
                o += 6;
            }
            0x426b => {
                ops.push(GOp::Word(g(o + 2), 0));
                o += 4;
            }
            0x2748 => {
                ops.push(GOp::StoreA0(g(o + 2)));
                o += 4;
            }
            0xd0fc => {
                ops.push(GOp::AddA0(r.sw(o + 2) as u32));
                o += 4;
            }
            0xd1fc => {
                ops.push(GOp::AddA0(r.l(o + 2)));
                o += 6;
            }
            0x207a => {
                ops.push(GOp::LoadWave(r.rel(o + 2) as u32));
                o += 4;
            }
            0x323c => {
                d1 = r.w(o + 2);
                o += 4;
            }
            op @ (0x30fc | 0x10fc) if r.w(o + 4) == 0x51c9 => {
                ops.push(GOp::Fill { word: op == 0x30fc, val: r.w(o + 2), count: d1 });
                o += 8;
            }
            _ if stop_unknown => break,
            _ => o += 2,
        }
    }
    ops
}

#[derive(Clone, Copy, Debug)]
struct Square {
    /// The waveform's pointer, its position and direction words.
    wave: u32,
    pos: u32,
    dir: u32,
    bytes: bool,
    up: (u16, u8, u16),
    down: (u16, u8, u16),
}

/// The replayer's code, as far as this port needs it: guest addresses of its
/// variables and tables, and the features its code has.
#[derive(Clone, Debug)]
struct Var {
    /// The module, and the replayer's variables (its a3).
    module: u32,
    base: u32,
    voices: u32,
    stride: u32,
    down: bool,
    /// How many voices the loop plays (3 in some replayers).
    nch: usize,
    playing: u32,
    extra: Option<(u32, u32)>,
    /// The delay speed (times a factor in some), its counter.
    delay: Option<(u32, u32, u16)>,
    fade: Option<(u32, u32, u32)>,
    square: Option<Square>,
    loop_len_long: bool,
    empty: u32,
    empty_len: u16,
    off: Off,
    /// The note's transposes and where it is kept, in the code's order.
    note_ops: Vec<NoteOp>,
    /// The note's volume: from the envelope (else the sample's), times the
    /// global volume / 64.
    env: bool,
    gvol: Option<u32>,
    periods: u32,
    frame_gtransp: Option<u32>,
    frame_ctransp: bool,
    arp: bool,
    slide: bool,
    /// The vibrato's speed, value and limit fields.
    vib: Option<(u32, u32, u32)>,
    cmds: Vec<Cmd>,
    fx: Vec<Fx>,
    fx_mask: u8,
    // InitSound
    subsongs: u32,
    sub_size: u32,
    /// The subsong's speed: a byte (and the delay speed's after it) or a word.
    sub_speed: u32,
    sub_byte: bool,
    sub_delay: Option<u32>,
    default_arp: u32,
    clear_transpose: bool,
    sample_data: u32,
    sample_info: u32,
    sample_count: u32,
    sample_size: u32,
    init_flag: Option<u32>,
    /// The flag the sample routine tests (the one the Eagleplayer clears, when
    /// its scan found it).
    sample_flag: Option<u32>,
    sample_ops: Vec<GOp>,
    /// The voice's transpose byte.
    tr: u32,
    /// The envelope's volume halved when the voice's byte 3 is set, then less a
    /// fade (not below 0).
    vol_half: bool,
    vol_fade: Option<u32>,
    vol_sub: Option<u32>,
    /// What the sample routine stores before its loop (a0 from the data on).
    pre_ops: Vec<GOp>,
    init_ops: Vec<GOp>,
    patch_voices: bool,
    /// A stop routine that turns each voice off on its own, unless the flag
    /// (a sound effect's) at its address is set.
    stop_voices: Option<[u32; 4]>,
    filter_off: bool,
    /// The stop routine's DMACON write, and the voices whose volume it zeroes.
    stop_mask: u16,
    stop_vols: [bool; 4],
    /// A subsong the init routine replaces by the next of a cycle: its number,
    /// the cycle's counter and length.
    sub_cycle: Option<(u8, u32, u8)>,
}

/// Reads the replayer's code into a `Var` (offsets in the module, made guest
/// addresses with `base`).
fn analyse(r: Rd, ep: &Ep) -> Option<Var> {
    let p = ep.play;
    let base = ep.base;
    // play: lea 0(pc),a3 / tst.b x(a3) / beq
    if !r.at(p, &[hi(0x47), ANY, 0x4a2b, ANY, hi(0x67)]) {
        return None;
    }
    // the playing flag: the first test (some test the extra counter's before it)
    let playing = if r.at(p + 4, &[0x4a2b, ANY, hi(0x67), 0x532b, ANY, hi(0x66), 0x177c, 6]) && r.at(p + 22, &[hi(0x60), 0x4a2b, ANY, hi(0x67)]) {
        base + r.sw(p + 26)
    } else {
        base + r.sw(p + 6)
    };

    let extra = r.find(p, p + 100, &[0x4a2b, ANY, hi(0x67), 0x532b, ANY, hi(0x66), 0x177c, 6])
        .map(|o| (base + r.sw(o + 2), base + r.sw(o + 8)));
    let delay = r.find(p, p + 100, &[0x103a, ANY, 0xd12b, ANY, hi(0x65)])
        .map(|o| (r.rel(o + 2), base + r.sw(o + 6), 1))
        // ext.w d0 / mulu.w #n,d0 before the add
        .or_else(|| r.find(p, p + 100, &[0x103a, ANY, 0x4880, 0xc0fc, ANY, 0xd12b, ANY, hi(0x65)])
            .map(|o| (r.rel(o + 2), base + r.sw(o + 12), r.w(o + 8))));
    let fade = r.find(p, p + 100, &[0x103a, ANY, hi(0x67), 0x4a6b, ANY, 0x6700, ANY, 0x532b, ANY, hi(0x66), 0x536b])
        .map(|o| (r.rel(o + 2), base + r.sw(o + 16), base + r.sw(o + 8)));
    let square = r.find(p, p + 100, &[0x207a, ANY, 0x303a, ANY, 0x4a6b, ANY, hi(0x6b)]).and_then(|o| {
        let wave = r.rel(o + 2);
        let pos = r.rel(o + 6);
        let dir = base + r.sw(o + 10);
        let half = |a: i64| -> Option<((u16, u8, u16), bool, i64)> {
            // move.w/b #v,(a0,d0.w) / addq|subq.w #n,pos(a3) / cmpi.w #lim,pos(a3)
            let op = r.w(a);
            if op != 0x31bc && op != 0x11bc {
                return None;
            }
            let v = r.w(a + 2);
            let q = r.w(a + 6);
            if q & 0xf0ff != 0x506b {
                return None;
            }
            let n = ((q >> 9) & 7) as u8;
            let n = if n == 0 { 8 } else { n };
            if r.w(a + 10) != 0x0c6b {
                return None;
            }
            Some(((v, n, r.w(a + 12)), op == 0x11bc, a + 16))
        };
        let (up, bytes, a) = half(o + 14)?;
        // bne / not.w / bra
        let a = r.find(a, a + 12, &[0x31bc]).or_else(|| r.find(a, a + 12, &[0x11bc]))?;
        let (down, _, _) = half(a)?;
        Some(Square { wave: wave as u32, pos: pos as u32, dir: dir as u32, bytes, up, down })
    });
    // the voice loop
    // moveq #0,d7 counts up; moveq #n,d7 (n = 1..3) counts down, n+1 voices
    let lp = (0..0x100).map(|k| p + 2 * k)
        .find(|&a| r.w(a) & 0xfffc == 0x7e00 && r.find(a, a + 40, &[0x303c, ANY, 0xc0c7]).is_some())?;
    let down = r.w(lp) != 0x7e00;
    let (stride, voices) = {
        let o = r.find(lp, lp + 40, &[0x303c, ANY, 0xc0c7])?;
        let l = r.find(o, o + 12, &[0x41fa])?;
        (r.w(o + 2) as u32, r.rel(l + 2))
    };
    // addq.w #1,d7 / cmp.w #n,d7 ends a loop counting up
    let nch = if down { (r.w(lp) & 3) as usize + 1 } else { r.find(lp, lp + 0x800, &[0x5247, 0xbe7c, ANY]).map_or(4, |o| r.w(o + 4) as usize).clamp(1, 4) };
    // the loop write: move.l 4(a5),d0 / bmi ... lsr.l|lsr.w #1,d0
    let lw = r.find(lp, lp + 0x60, &[0x202d, 0x0004, hi(0x6b)])?;
    let loop_len_long = r.find(lw, lw + 16, &[0xe288]).is_some();
    let e = r.find(lw, lw + 0x50, &[0x2d7a, ANY, 0x00a0])?;
    let empty = r.rel(e + 2);
    let el = r.find(lw, lw + 0x50, &[0x3d7c, ANY, 0x00a4])?;
    let empty_len = r.w(el + 2);
    // the counter: subq.w #1,$1e(a0) / beq read / cmpi.w #1,$1e(a0) / bne frame
    let c = r.find(lw, lw + 0x60, &[0x5368, 0x001e, hi(0x67)])?;
    let read = c + 6 + (r.b(c + 5) as i8 as i64);
    if !r.at(c + 6, &[0x0c68, 1, 0x001e]) {
        return None;
    }
    let frame = if r.at(c + 12, &[0x6600]) { r.rel(c + 14) } else if r.b(c + 12) == 0x66 { c + 14 + r.b(c + 13) as i8 as i64 } else { return None };
    let doff = r.find(c + 12, c + 60, &[0x33c1, 0x00df, 0xf096])?;
    let off = if r.find(c + 12, doff, &[0x0c11, 0x00e0]).is_some() && r.find(c + 12, doff, &[0x0c29, 0x0083, 0x0001]).is_some() {
        Off::Not83Dur
    } else if r.find(c + 12, doff, &[0x0c11, 0x0083]).is_some() {
        Off::Not83
    } else {
        Off::Always
    };
    // the row: clr.b 0(a0) / moveq #0,d0 / move.b (a1)+,d0 / bmi.w commands
    if !r.at(read, &[0x4228, 0x0000, 0x7000, 0x1019, 0x6b00]) {
        return None;
    }
    let cmd = r.rel(read + 10);
    let per = r.find(read, read + 0x80, &[0x45fa, ANY, 0x322d, 0x000a])?;
    let periods = r.rel(per + 2);
    let note_code = read + 12;
    let store = r.find(note_code, per, &[0x1140, 0x0002])?;
    let mut note_ops = vec![(store, NoteOp::Keep)];
    if let Some(o) = r.find(note_code, per, &[0xd02d, 0x000e]) {
        note_ops.push((o, NoteOp::SampleTranspose));
    }
    if let Some(o) = r.find(note_code, per, &[0xd03a]) {
        note_ops.push((o, NoteOp::GlobalTranspose(r.rel(o + 2) as u32)));
    }
    // the voice's transpose: byte 3, or $2f in some
    let tr: u32 = if r.find(note_code, per, &[0xd028, 0x002f]).is_some() { 0x2f } else { 3 };
    if let Some(o) = r.find(note_code, per, &[0xd028, tr]) {
        note_ops.push((o, NoteOp::Transpose));
    }
    note_ops.sort_by_key(|x| x.0);
    let note_ops: Vec<NoteOp> = note_ops.into_iter().map(|x| x.1).collect();
    let env = r.find(note_code, per, &[0x2468, 0x0022, 0x121a]).is_some();
    let gvol = r.find(note_code, per, &[0x343a, ANY, 0xc2c2, 0xec49]).map(|o| r.rel(o + 2));
    // tst.b $3(a0) / beq / lsr.w #1,d1: half the volume; sub.w f(a3),d1 / bcc / moveq #0,d1
    let vol_half = r.find(note_code, per, &[0x4a28, 0x0003, 0x6702, 0xe249]).is_some();
    let vol_fade = r.find(note_code, per, &[0x926b, ANY, 0x6402, 0x7200]).map(|o| (base + r.sw(o + 2)) as u32)
        .or_else(|| r.find(note_code, per, &[0x927a, ANY, 0x6402, 0x7200]).map(|o| r.rel(o + 2) as u32));
    // move.w $c(a5),d1 / sub.w fade(pc),d1: the sample's volume less a fade
    let vol_sub = r.find(note_code, per, &[0x322d, 0x000c, 0x927a]).map(|o| r.rel(o + 6) as u32);
    if !env && vol_sub.is_none() && r.find(note_code, per, &[0x3d6d, 0x000c, 0x00a8]).is_none() {
        return None;
    }
    // the effects of a frame
    if !r.at(frame, &[0x1028, 0x0002]) {
        return None;
    }
    let fper = r.find(frame, frame + 0x40, &[0x45fa, ANY, 0x322d, 0x000a])?;
    let frame_gtransp = r.find(frame, fper, &[0xd03a]).map(|o| r.rel(o + 2));
    let frame_ctransp = r.find(frame, fper, &[0xd028, tr]).is_some();
    let arp = r.find(frame, fper, &[0x2268, 0x0010, 0x1219, 0x0881, 0x0007]).is_some();
    let slide = r.find(fper, fper + 0x30, &[0x0828, 0x0001, 0x0000]).is_some();
    let vib = r.find(fper, fper + 0x60, &[0x4a28, 0x0001, hi(0x67), hi(0x6a), 0x1228]).map(|o| {
        let v = r.w(o + 10) as u32;
        let s = r.w(o + 14) as u32;
        let m = r.w(o + 22) as u32;
        (s, v, m)
    });
    // the commands
    let mut cmds = Vec::new();
    let mut a = cmd;
    let fx_at;
    loop {
        if (r.w(a) == 0xb03c || r.w(a) == 0x0c00) && r.b(a + 2) == 0 && matches!(r.b(a + 4), 0x6d | 0x65) {
            let from = r.b(a + 3);
            let body = a + if r.b(a + 5) == 0 { 8 } else { 6 };
            if let Some(o) = r.find(body, body + 8, &[0xc0fa]) {
                cmds.push(Cmd::Duration { from, speed: r.rel(o + 2) as u32 });
            } else if let Some(o) = r.find(body, body + 8, &[0x4bfa, ANY, 0xc0fc]) {
                cmds.push(Cmd::Sample { from, table: r.rel(o + 2) as u32, size: r.w(o + 6) as u32 });
            } else if let Some(o) = r.find(body, body + 10, &[0x45fa]) {
                if r.find(o, o + 16, &[0x214a, 0x0022]).is_some() {
                    cmds.push(Cmd::Envelope { from, table: r.rel(o + 2) as u32 });
                } else if r.find(o, o + 16, &[0x214a, 0x000c, 0x214a, 0x0010]).is_some() {
                    cmds.push(Cmd::Arpeggio { from, table: r.rel(o + 2) as u32 });
                } else {
                    return None;
                }
            } else {
                return None;
            }
            // to the next compare
            a = r.find(body, body + 0x30, &[0xb03c]).or_else(|| r.find(body, body + 0x30, &[0x0c00]))
                .filter(|&n| r.find(body, n, &[0xd000]).is_none() && r.find(body, n, &[0x0240, 0x001f]).is_none())
                .unwrap_or_else(|| r.find(body, body + 0x30, &[0xd000]).or_else(|| r.find(body, body + 0x30, &[0x0240, 0x001f])).unwrap_or(-1));
            if a < 0 {
                return None;
            }
            continue;
        }
        fx_at = a;
        break;
    }
    let (fx_mask, jt, rel_base) = if r.at(fx_at, &[0xd000, 0x45fa, ANY, 0x3472, 0x0000, 0x4ef3, 0xa000]) {
        (0x7f, r.rel(fx_at + 4), base)
    } else if r.at(fx_at, &[0x0240, 0x001f, 0xd040, 0x45fa, ANY, 0x3472, 0x0000, 0xd5cb, 0x4ed2]) {
        (0x1f, r.rel(fx_at + 8), base)
    } else {
        return None;
    };
    let mut fx = Vec::new();
    for k in 0..16 {
        let t = rel_base + r.w(jt + 2 * k) as i64;
        // handlers lie in the play code, some before the dispatch
        if t < ep.play || t >= jt {
            break;
        }
        fx.push(classify(r, t, base, ep.stop, tr));
    }
    // InitSound: the subsong table
    let i = ep.init;
    // ext.w d0 / mulu.w #size,d0 / lea table(pc),a0; or (one subsong) moveq #0,d0
    let (sub_size, l) = match r.find(i, i + 0x100, &[0x4880, 0xc0fc]) {
        Some(m) => (r.w(m + 4) as u32, r.find(m, m + 12, &[0x41fa])?),
        None if r.at(i, &[0x7000]) => (0, r.find(i, i + 0x20, &[0x41fa, ANY, 0x1230, 0x0000])?),
        None => return None,
    };
    let subsongs = r.rel(l + 2) as u32;
    let (sub_speed, sub_byte, sub_delay) = if r.at(l + 4, &[0x1230, 0x0000, 0x4881, 0x3741]) {
        let d = r.at(l + 14, &[0x1770, 0x0001]).then(|| (base + r.sw(l + 18)) as u32);
        ((base + r.sw(l + 12)) as u32, true, d)
    } else if r.at(l + 4, &[0x3770, 0x0000]) {
        ((base + r.sw(l + 8)) as u32, false, None)
    } else {
        return None;
    };
    let vl = r.find(l, l + 0x40, &[0x337c, 0x0001, 0x001e])?;
    let da = r.find(vl, vl + 0x30, &[0x41fa, ANY, 0x2348, 0x000c])?;
    let default_arp = r.rel(da + 2) as u32;
    let clear_transpose = r.find(vl, da, &[0x51e9, tr]).is_some();
    if r.find(da, da + 0x20, &[0x3070, 0x0002]).is_none() {
        return None;
    }
    // the init routine's own writes to variables
    // (without the sample routine's own, when its loop is inline)
    let mut init_ops = gops(r, i, i + 0x100, base, false);
    init_ops.retain(|o| !matches!(o, GOp::StoreA0(_) | GOp::AddA0(_)));
    // the sample loop
    let sl = r.find(0, 0x4000, &[0x2418, 0x3618, 0x2b48, 0x0000])?;
    // the nearest lea before it: the sample data (and what is stored before)
    let sd = (1..=24).map(|k| sl - 2 * k).find(|&a| r.w(a) == 0x41fa)?;
    let sample_data = r.rel(sd + 2) as u32;
    // the routine's own test of its flag: tst.b f(a3) / bne.w
    let sample_flag = r.find(sd - 8, sd, &[0x4a2b, ANY, hi(0x66)]).map(|o| (base + r.sw(o + 2)) as u32);
    let si = r.find(sd, sl, &[0x4bfa])?;
    let sample_info = r.rel(si + 2) as u32;
    let mq = r.find(si, sl, &[hi(0x72)])?;
    let sample_count = r.b(mq + 1) as u32 + 1;
    let ss = r.find(sl, sl + 0x30, &[0xdafc])?;
    let sample_size = r.w(ss + 2) as u32;
    let post = r.find(ss, ss + 8, &[hi(0x51)])? + 4;
    let sample_ops = gops(r, post, post + 0x60, base, true);
    let pre_ops = gops(r, sd + 4, si, base, true);
    let patch_voices = r.find(0, 0x4000, &[0x303c, 0x0030, 0xc0c7, 0x41fa]).is_some();
    // tst.b f(a3) / bne / sf.b f(a3) / move.w #1<<ch,$dff096 / move.w #0,$dff0a8+16*ch
    let stop_voices = (|| {
        let mut f = [0i64; 4];
        let mut a = ep.stop;
        for ch in 0..4u32 {
            let o = r.find(a, a + 0x40, &[0x4a2b, ANY, 0x6614, 0x51eb])?;
            if !r.at(o + 10, &[0x33fc, 1 << ch, 0x00df, 0xf096, 0x33fc, 0, 0x00df, 0xf0a8 + 16 * ch]) {
                return None;
            }
            f[ch as usize] = base + r.sw(o + 2);
            a = o + 26;
        }
        Some(f)
    })();
    // the stop routine: move.w #mask,$dff096 / move.w #0,$dff0a8+16*ch
    let st = ep.stop;
    let st_end = r.find(st, st + 0x80, &[0x4e75]).unwrap_or(st + 0x80);
    let stop_mask = r.find(st, st_end, &[0x33fc, ANY, 0x00df, 0xf096]).map_or(0xf, |o| r.w(o + 2));
    let stop_vols = [0, 1, 2, 3].map(|ch: u32| r.find(st, st_end, &[0x33fc, 0, 0x00df, 0xf0a8 + 16 * ch]).is_some());
    // cmp.b #n,d0 / bne / add.b c(pc),d0 / addq.b #1,c(a3) / cmpi.b #m,c(a3) / bne / clr.b c(a3)
    let sub_cycle = r.find(ep.init, ep.init + 0x20, &[0xb03c, ANY, hi(0x66), 0xd03a, ANY, 0x522b, ANY, 0x0c2b, ANY, ANY, 0x6604, 0x422b])
        .map(|o| (r.b(o + 3), r.rel(o + 8) as u32, r.b(o + 17)));
    // bset.b #1,$bfe001: the init routine turns the LED filter off
    let filter_off = r.find(ep.init, ep.init + 8, &[0x08f9, 0x0001, 0x00bf, 0xe001]).is_some();
    let u = |v: i64| v as u32;
    Some(Var {
        filter_off,
        stop_mask,
        stop_vols,
        sub_cycle,
        module: 0,
        base: u(base),
        voices: u(voices),
        stride,
        down,
        nch,
        playing: u(playing),
        extra: extra.map(|(a, b)| (u(a), u(b))),
        delay: delay.map(|(a, b, m)| (u(a), u(b), m)),
        fade: fade.map(|(a, b, c)| (u(a), u(b), u(c))),
        square,
        loop_len_long,
        empty: u(empty),
        empty_len,
        off,
        note_ops,
        env,
        gvol: gvol.map(u),
        periods: u(periods),
        frame_gtransp: frame_gtransp.map(u),
        frame_ctransp,
        arp,
        slide,
        vib,
        cmds,
        fx,
        fx_mask,
        subsongs,
        sub_size,
        sub_speed,
        sub_byte,
        sub_delay,
        default_arp,
        clear_transpose,
        sample_data,
        sample_info,
        sample_count,
        sample_size,
        init_flag: ep.init_flag.map(u),
        sample_flag,
        sample_ops,
        tr,
        vol_half,
        vol_fade,
        vol_sub,
        pre_ops,
        init_ops,
        patch_voices,
        stop_voices: stop_voices.map(|f| f.map(u)),
    })
}

/// What an effect's handler does, from its code.
fn classify(r: Rd, t: i64, base: i64, stop: i64, tr: u32) -> Fx {
    let g = |o: i64| (base + r.sw(o)) as u32;
    if r.at(t, &[0x3028, 0x000a, 0x3468, 0x0008]) {
        return Fx::NextPosition;
    }
    if r.at(t, &[0x4268, 0x0020, 0x1159, 0x0014, 0x1159, 0x0015, 0x08e8, 0x0001]) {
        return Fx::Slide;
    }
    let rest = r.at(t, &[0x3168, 0x001c, 0x001e, 0x2149, 0x0004]) || r.at(t, &[0x2149, 0x0004, 0x3168, 0x001c, 0x001e]);
    if rest && r.find(t, t + 30, &[0x2d7a]).is_none() {
        if let Some(o) = r.find(t, t + 26, &[0x4241, 0x0fc1, 0x33c1]) {
            return Fx::Off { done: r.find(t, o, &[0x50e8, 0x0016]).is_some() };
        }
    }
    if r.at(t, &[0x3168, 0x001c, 0x001e, 0x2149, 0x0004]) && r.find(t, t + 30, &[0x2d7a]).is_some() {
        return Fx::Mute;
    }
    if r.at(t, &[0x1759, ANY, 0x177a]) {
        return Fx::Fade(g(t + 2), g(t + 8));
    }
    if r.at(t, &[0x1759, ANY, 0x6000]) {
        return Fx::SetByte(g(t + 2));
    }
    if r.at(t, &[0x1019, 0x4880, 0x3740, ANY, 0x6000]) {
        return Fx::SetWord(g(t + 6));
    }
    if r.at(t, &[0x1019, 0x1740, ANY, 0x6000]) {
        return Fx::SetByte(g(t + 4));
    }
    if r.at(t, &[0x50e8, 0x0001, 0x1159]) {
        return Fx::Vibrato;
    }
    if r.at(t, &[0x51e8, 0x0001, 0x6000]) {
        return Fx::VibratoOff;
    }
    if r.at(t, &[0x50e8, 0x0003, 0x6000]) || r.at(t, &[0x51e8, 0x0003, 0x6000]) {
        return Fx::Half(r.w(t) == 0x50e8);
    }
    if r.at(t, &[0x1159, tr, 0x6000]) {
        return Fx::Transpose;
    }
    if r.at(t, &[0x1159, 0x0008, 0x1159, 0x0009, 0x317c, 0x0000, 0x000a]) {
        return Fx::PositionList;
    }
    // a stop: clears its flags and turns all voices off, or goes to the stop routine
    let stop_code = |a: i64| {
        r.find(a, a + 60, &[0x33fc, 0x000f, 0x00df, 0xf096])
            .filter(|&o| r.find(a, o, &[0x6000]).is_none() && r.find(a, o, &[hi(0x60)]).is_none())
            .map(|o| Fx::Stop { vols: r.find(o, o + 24, &[0x33fc, 0x0000, 0x00df, 0xf0a8]).is_some() })
    };
    if let Some(f) = stop_code(t) {
        return f;
    }
    let wait = |x: i64| r.at(x, &[0x2149, 0x0004, 0x3168, 0x001c, 0x001e])
        .then(|| Fx::Wait { dma: r.at(x + 10, &[0x323c, 0x8200, 0x0fc1, 0x33c1]) });
    if let Some(f) = wait(t) {
        return f;
    }
    // the stop routine, or its wrapper: movem / bsr stop
    let is_stop = |x: i64| x == stop || (r.at(x, &[0x48e7]) && r.at(x + 4, &[0x6100]) && r.rel(x + 6) == stop);
    if r.at(t, &[0x51eb, ANY, 0x6000]) && is_stop(r.rel(t + 6)) {
        return Fx::ClearStop(g(t + 2));
    }
    if r.at(t, &[0x6000]) {
        let x = r.rel(t + 2);
        if let Some(f) = stop_code(x) {
            return f;
        }
        if is_stop(x) {
            return Fx::StopRoutine;
        }
        if let Some(f) = wait(x) {
            return f;
        }
        if r.at(x, &[0x6000]) {
            if let Some(f) = wait(r.rel(x + 2)) {
                return f;
            }
        }
    }
    Fx::Nop
}

/// The Eagleplayer's copy of the audio registers: what the replayer wrote since
/// the last flush. Cleared, a location is 0, a length $FFFF, a period 0 and a
/// volume's low byte $FF: those are not sent.
struct Shadow {
    dma: u16,
    loc: [u32; 4],
    len: [u16; 4],
    per: [u16; 4],
    vol: [u16; 4],
}

impl Shadow {
    fn new() -> Self {
        let mut s = Shadow { dma: 0, loc: [0; 4], len: [0; 4], per: [0; 4], vol: [0; 4] };
        s.clear();
        s
    }

    fn clear(&mut self) {
        self.dma = 0;
        for ch in 0..4 {
            self.loc[ch] = 0;
            self.len[ch] = 0xffff;
            self.per[ch] = 0;
            self.vol[ch] |= 0xff;
        }
    }

    /// Sends the copy to Paula, the fourth voice first, then DMACON.
    fn flush(&mut self, hw: &mut Hw) {
        hw.wait(T_FLUSH);
        for ch in (0..4).rev() {
            hw.wait(T_FLUSH_VOICE);
            if self.loc[ch] != 0 {
                hw.wait(T_POKE);
                hw.loc(ch, self.loc[ch]);
            } else {
                hw.wait(T_SKIP);
            }
            if self.len[ch] != 0xffff {
                hw.wait(T_POKE);
                hw.len(ch, self.len[ch]);
            } else {
                hw.wait(T_SKIP);
            }
            if self.per[ch] != 0 {
                hw.wait(T_POKE_PER);
                hw.per(ch, self.per[ch]);
            } else {
                hw.wait(T_SKIP);
            }
            if self.vol[ch] & 0xff != 0xff {
                hw.wait(T_POKE);
                hw.vol(ch, (self.vol[ch] & 0xff).min(64));
            } else {
                hw.wait(T_SKIP_VOL);
            }
        }
        hw.wait(T_FLUSH_END);
        if self.dma != 0 {
            let v = (self.dma & 0x8000) | (self.dma & 0xf);
            hw.dmacon(v);
            if v & 0x8000 != 0 {
                // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does
                let now = hw.bus.clock;
                hw.wait((now / LINE + 10) * LINE - now);
            }
        }
    }

    /// A DMACON write: the Eagleplayer's handler flushes at once.
    fn dmacon(&mut self, v: u16, hw: &mut Hw) {
        hw.wait(T_DMA_CALL);
        self.dma = v;
        self.flush(hw);
        self.clear();
        hw.wait(T_DMA_RET);
    }
}

// The flush's costs, in colour clocks, measured on the reference's write log:
// a poke (the period's scaling costs more), a register it skips, each voice,
// and from the last voice to the DMACON call.
const T_FLUSH: u64 = 40;
const T_FLUSH_VOICE: u64 = 9;
const T_POKE: u64 = 45;
const T_POKE_PER: u64 = 62;
const T_SKIP: u64 = 17;
const T_SKIP_VOL: u64 = 13;
const T_FLUSH_END: u64 = 62;
const T_DMA_CALL: u64 = 100;
const T_DMA_RET: u64 = 100;

#[derive(Clone, Copy, Default)]
struct Voice {
    flags: u8,
    vib: u8,
    note: u8,
    transpose: u8,
    track: u32,
    list: u16,
    pos: u16,
    arp_start: u32,
    arp: u32,
    slide_speed: u8,
    slide_delay: u8,
    loop_done: bool,
    sample: u32,
    speed: u16,
    counter: u16,
    slide: u16,
    env_start: u32,
    env: u32,
    env_speed: u8,
    env_count: u8,
    vib_speed: u8,
    vib_value: u8,
    vib_max: u8,
}

pub struct Whittaker {
    v: Var,
    voices: [Voice; 4],
    sh: Shadow,
    sub: u16,
    last_subsong: u16,
    init_flag: Option<u32>,
    /// A [$80, 0, 0, 0] in player memory: the Eagleplayer's envelope for voices
    /// that have none yet; and its stand-in for an unplayable sample.
    no_env: u32,
    bad_sample: u32,
}

/// What a voice's row did.
enum Next {
    Voice,
    Stop,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let ep = ep_check(d)?;
    let mut v = analyse(Rd(d), &ep)?;
    // offsets in the module to guest addresses
    let g = |o: u32| base.wrapping_add(o);
    v.module = base;
    v.base = g(v.base);
    v.voices = g(v.voices);
    v.playing = g(v.playing);
    v.extra = v.extra.map(|(a, b)| (g(a), g(b)));
    v.delay = v.delay.map(|(a, b, m)| (g(a), g(b), m));
    v.fade = v.fade.map(|(a, b, c)| (g(a), g(b), g(c)));
    if let Some(s) = &mut v.square {
        s.wave = g(s.wave);
        s.pos = g(s.pos);
        s.dir = g(s.dir);
    }
    v.empty = g(v.empty);
    for o in &mut v.note_ops {
        if let NoteOp::GlobalTranspose(a) = o {
            *a = g(*a);
        }
    }
    v.gvol = v.gvol.map(g);
    v.periods = g(v.periods);
    v.frame_gtransp = v.frame_gtransp.map(g);
    for c in &mut v.cmds {
        match c {
            Cmd::Duration { speed, .. } => *speed = g(*speed),
            Cmd::Sample { table, .. } | Cmd::Envelope { table, .. } | Cmd::Arpeggio { table, .. } => *table = g(*table),
        }
    }
    for f in &mut v.fx {
        match f {
            Fx::SetByte(a) | Fx::SetWord(a) | Fx::ClearStop(a) => *a = g(*a),
            Fx::Fade(a, b) => {
                *a = g(*a);
                *b = g(*b);
            }
            _ => {}
        }
    }
    for o in v.init_ops.iter_mut().chain(v.sample_ops.iter_mut()).chain(v.pre_ops.iter_mut()) {
        match o {
            GOp::Byte(a, _) | GOp::Word(a, _) | GOp::AddWord(a, _) | GOp::StoreA0(a) | GOp::LoadWave(a) => *a = g(*a),
            _ => {}
        }
    }
    v.subsongs = g(v.subsongs);
    v.sub_speed = g(v.sub_speed);
    v.sub_delay = v.sub_delay.map(g);
    v.vol_sub = v.vol_sub.map(g);
    v.vol_fade = v.vol_fade.map(g);
    v.stop_voices = v.stop_voices.map(|f| f.map(g));
    v.sub_cycle = v.sub_cycle.map(|(n, c, m)| (n, g(c), m));
    v.default_arp = g(v.default_arp);
    v.sample_data = g(v.sample_data);
    v.sample_info = g(v.sample_info);
    let init_flag = v.init_flag.map(g);
    v.sample_flag = v.sample_flag.map(g);
    let no_env = m.alloc(16);
    if no_env == 0 {
        return None;
    }
    m.bus.w8(no_env, 0x80);
    let bad_sample = no_env + 8;
    let b = &m.bus;
    let mut voices = [Voice::default(); 4];
    for (ch, vo) in voices.iter_mut().enumerate() {
        let a = v.voices + v.stride * ch as u32;
        // what the module's voices hold before the init routine runs, and
        // that it does not set
        vo.sample = b.r32(a + 0x18);
        vo.note = b.r8(a + 2);
        vo.transpose = b.r8(a + v.tr);
        vo.slide_speed = b.r8(a + 0x14);
        vo.slide_delay = b.r8(a + 0x15);
        vo.speed = b.r16(a + 0x1c);
        vo.slide = b.r16(a + 0x20);
        if let Some((s, x, m)) = v.vib {
            vo.vib_speed = b.r8(a + s);
            vo.vib_value = b.r8(a + x);
            vo.vib_max = b.r8(a + m);
        }
        if v.env {
            vo.env_start = b.r32(a + 0x22);
            vo.env = b.r32(a + 0x26);
            vo.env_speed = b.r8(a + 0x2a);
            vo.env_count = b.r8(a + 0x2b);
        }
        // the Eagleplayer's InitPlayer points a replayer with $30-byte voices at
        // its empty envelope and the first sample
        if v.patch_voices {
            vo.env = no_env;
            if ep.sample_info.is_some() {
                vo.sample = v.sample_info;
            }
        }
    }
    let mut w = Whittaker { v, voices, sh: Shadow::new(), sub: 0, last_subsong: ep.last_subsong, init_flag, no_env, bad_sample };
    let _ = w.no_env;
    w.init_sound(0, &mut Hw { bus: &mut m.bus });
    Some(Box::new(w))
}

impl Replayer for Whittaker {
    fn format(&self) -> &'static str {
        "David Whittaker"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        false
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.last_subsong, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // EndSound: the stop routine, whose DMACON write is flushed
        self.sh.clear();
        self.stop(hw);
        self.init_sound(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        hw.wait(T_TICK);
        self.sh.clear();
        self.play(hw);
        self.sh.flush(hw);
    }
}

// InitSound's time before its flush (its sample loop, per sample), measured on
// the reference's writes of Emlyn Hughes Arcade Quiz, APB and Army Moves; the
// play routine's: a voice, a row with its note, a frame, fitted to the gaps
// between the reference's writes and then to the audio (specdist) of 13 modules.
const T_INIT: u64 = 1350;
const T_INIT_SAMPLE: u64 = 152;
const T_TICK: u64 = 212;
const T_VOICE: u64 = 100;
const T_ROW: u64 = 468;
const T_FRAME: u64 = 300;

impl Whittaker {
    /// The stop routine: clears its flags, all voices off, volumes 0.
    fn stop(&mut self, hw: &mut Hw) {
        let r = &self.v;
        hw.bus.w8(r.playing, 0);
        if let Some(f) = r.stop_voices {
            for ch in 0..4 {
                if hw.bus.r8(f[ch]) == 0 {
                    self.sh.dmacon(1 << ch, hw);
                    self.sh.vol[ch] = 0;
                }
            }
            return;
        }
        self.sh.dmacon(r.stop_mask, hw);
        for ch in 0..4 {
            if r.stop_vols[ch] {
                self.sh.vol[ch] = 0;
            }
        }
    }

    /// The init routine (as the Eagleplayer calls it: the sample table's flag
    /// cleared first).
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.sh.clear();
        if self.v.filter_off {
            hw.bus.ciaa.pra |= 2;
        }
        let mut n = n;
        if let Some((k, c, m)) = self.v.sub_cycle {
            if n as u8 == k {
                n = (n as u8).wrapping_add(hw.bus.r8(c)) as u16;
                let x = hw.bus.r8(c).wrapping_add(1);
                hw.bus.w8(c, if x == m { 0 } else { x });
            }
        }
        if let Some(f) = self.init_flag {
            hw.bus.w8(f, 0);
        }
        self.stop(hw);
        self.init_samples(hw);
        let v = self.v.clone();
        let b = &mut *hw.bus;
        // the variables the init routine sets
        let mut a0 = 0;
        exec(b, &v.init_ops, &mut a0);
        let e = v.subsongs.wrapping_add((n as i16 as i32 * v.sub_size as i32) as u32);
        if v.sub_byte {
            b.w16(v.sub_speed, b.r8(e) as i8 as i16 as u16);
            if let Some(d) = v.sub_delay {
                b.w8(d, b.r8(e + 1));
            }
        } else {
            b.w16(v.sub_speed, b.r16(e));
        }
        for ch in 0..v.nch {
            let vo = &mut self.voices[ch];
            vo.counter = 1;
            vo.flags = 0;
            vo.vib = 0;
            if v.clear_transpose {
                vo.transpose = 0;
            }
            vo.loop_done = true;
            vo.arp_start = v.default_arp;
            vo.arp = v.default_arp;
            vo.list = b.r16(e + 2 + 2 * ch as u32);
            vo.pos = 2;
            vo.track = v.base.wrapping_add(b.r16(v.base.wrapping_add(vo.list as i16 as i32 as u32)) as i16 as i32 as u32);
        }
        // the init routine's time, mostly its sample loop
        hw.wait(T_INIT + T_INIT_SAMPLE * v.sample_count as u64);
        self.sh.flush(hw);
    }

    /// The sample loop of the init routine: each sample's pointer, length and
    /// finetune into the table, then what the routine sets after it.
    fn init_samples(&mut self, hw: &mut Hw) {
        let v = &self.v;
        let b = &mut *hw.bus;
        if let Some(f) = v.sample_flag {
            if b.r8(f) != 0 {
                return;
            }
        }
        let mut a0 = v.sample_data;
        exec(b, &v.pre_ops, &mut a0);
        let mut a5 = v.sample_info;
        for _ in 0..v.sample_count {
            let mut len = b.r32(a0);
            let mut freq = b.r16(a0 + 4);
            a0 += 6;
            if freq == 0 || len >= 0x20000 {
                // the Eagleplayer's patch
                a0 = self.bad_sample + 4;
                freq = 0x2000;
                len = 2;
            }
            b.w32(a5, a0);
            a0 = a0.wrapping_add(len);
            b.w16(a5 + 8, (len >> 1) as u16);
            let q = 0x369e99 / freq as u32;
            if q <= 0xffff {
                b.w16(a5 + 10, q as u16);
            } else {
                b.w16(a5 + 10, 0x9e99);
            }
            a5 += v.sample_size;
        }
        // after the loop: the empty sample, the flag, the square waveform
        exec(b, &v.sample_ops, &mut a0);
    }

    fn play(&mut self, hw: &mut Hw) {
        let v = self.v.clone();
        if hw.bus.r8(v.playing) == 0 {
            return;
        }
        if let Some((flag, cnt)) = v.extra {
            if hw.bus.r8(flag) != 0 {
                let c = hw.bus.r8(cnt).wrapping_sub(1);
                hw.bus.w8(cnt, c);
                if c == 0 {
                    hw.bus.w8(cnt, 6);
                    return;
                }
            }
        }
        if let Some((spd, cnt, mul)) = v.delay {
            let d = (hw.bus.r8(spd) as i8 as i16 as u16).wrapping_mul(mul) as u8;
            let (c, carry) = hw.bus.r8(cnt).overflowing_add(d);
            hw.bus.w8(cnt, c);
            if carry {
                return;
            }
        }
        if let Some((spd, cnt, vol)) = v.fade {
            let s = hw.bus.r8(spd);
            if s != 0 {
                if hw.bus.r16(vol) == 0 {
                    return self.stop_song(false, hw);
                }
                let c = hw.bus.r8(cnt).wrapping_sub(1);
                hw.bus.w8(cnt, c);
                if c == 0 {
                    let g = hw.bus.r16(vol).wrapping_sub(1);
                    hw.bus.w16(vol, g);
                    if g == 0 {
                        return self.stop_song(false, hw);
                    }
                    hw.bus.w8(cnt, s);
                }
            }
        }
        if let Some(s) = v.square {
            let b = &mut *hw.bus;
            let wave = b.r32(s.wave);
            let pos = b.r16(s.pos);
            let (val, n, lim) = if b.r16(s.dir) as i16 >= 0 { s.up } else { s.down };
            let a = wave.wrapping_add(pos as i16 as i32 as u32);
            if s.bytes {
                b.w8(a, val as u8);
            } else {
                b.w16(a, val);
            }
            let np = if b.r16(s.dir) as i16 >= 0 { pos.wrapping_add(n as u16) } else { pos.wrapping_sub(n as u16) };
            b.w16(s.pos, np);
            if np == lim {
                b.w16(s.dir, !b.r16(s.dir));
            }
        }
        let order: [usize; 4] = if v.down { [3, 2, 1, 0] } else { [0, 1, 2, 3] };
        let order = if v.down { &order[4 - v.nch..] } else { &order[..v.nch] };
        for &ch in order {
            if let Next::Stop = self.voice(ch, hw) {
                return;
            }
        }
    }

    /// The stop effect: the replayer's flags cleared and all voices off (and
    /// with `vols` their volumes 0).
    fn stop_song(&mut self, vols: bool, hw: &mut Hw) {
        hw.bus.w8(self.v.playing, 0);
        self.sh.dmacon(0x000f, hw);
        if vols {
            self.sh.vol = [0; 4];
        }
    }

    fn voice(&mut self, ch: usize, hw: &mut Hw) -> Next {
        hw.wait(T_VOICE);
        let v = &self.v;
        let b = &*hw.bus;
        let vo = &mut self.voices[ch];
        let s = vo.sample;
        if !vo.loop_done {
            vo.loop_done = true;
            let lp = b.r32(s.wrapping_add(4));
            if lp as i32 >= 0 {
                let d0 = if v.loop_len_long { (lp >> 1) as u16 } else { (lp as u16) >> 1 };
                self.sh.loc[ch] = b.r32(s).wrapping_add(lp);
                self.sh.len[ch] = b.r16(s.wrapping_add(8)).wrapping_sub(d0);
            } else {
                self.sh.loc[ch] = b.r32(v.empty);
                self.sh.len[ch] = v.empty_len;
            }
        }
        vo.counter = vo.counter.wrapping_sub(1);
        if vo.counter == 0 {
            return self.row(ch, hw);
        }
        if vo.counter == 1 {
            let next = b.r8(vo.track);
            let off = match v.off {
                Off::Always => true,
                Off::Not83 => next != 0x83,
                Off::Not83Dur => next != 0x83 && !((next as i8) >= (0xe0u8 as i8) && b.r8(vo.track.wrapping_add(1)) == 0x83),
            };
            if off {
                self.sh.dmacon(1 << ch, hw);
            }
            return Next::Voice;
        }
        self.frame(ch, hw);
        Next::Voice
    }

    /// The voice's next byte in its track.
    fn next(&mut self, ch: usize, hw: &Hw) -> u8 {
        let vo = &mut self.voices[ch];
        let x = hw.bus.r8(vo.track);
        vo.track = vo.track.wrapping_add(1);
        x
    }

    fn row(&mut self, ch: usize, hw: &mut Hw) -> Next {
        hw.wait(T_ROW);
        let v = self.v.clone();
        let base = v.base;
        let sx = |w: u16| base.wrapping_add(w as i16 as i32 as u32);
        self.voices[ch].flags = 0;
        for _ in 0..4096 {
            let byte = self.next(ch, hw);
            if byte < 0x80 {
                self.note(ch, byte, hw);
                return Next::Voice;
            }
            let from = |c: &Cmd| match c {
                Cmd::Duration { from, .. } | Cmd::Sample { from, .. } | Cmd::Envelope { from, .. } | Cmd::Arpeggio { from, .. } => *from,
            };
            if let Some(&c) = v.cmds.iter().find(|c| byte >= from(c)) {
                let b = &*hw.bus;
                let vo = &mut self.voices[ch];
                match c {
                    Cmd::Duration { speed, .. } => {
                        let k = byte.wrapping_sub(0xdf) as u32;
                        vo.speed = (k * b.r16(speed) as u32) as u16;
                    }
                    Cmd::Sample { from, table, size } => {
                        vo.sample = table.wrapping_add(((byte - from) as u32 * size) as u16 as i16 as i32 as u32);
                    }
                    Cmd::Envelope { from, table } => {
                        let e = sx(b.r16(table + 2 * (byte - from) as u32));
                        vo.env_start = e;
                        vo.env_speed = b.r8(e.wrapping_sub(1));
                    }
                    Cmd::Arpeggio { from, table } => {
                        let a = sx(b.r16(table + 2 * (byte - from) as u32));
                        vo.arp_start = a;
                        vo.arp = a;
                    }
                }
                continue;
            }
            let fx = v.fx.get((byte & v.fx_mask) as usize).copied().unwrap_or(Fx::Nop);
            match fx {
                Fx::NextPosition => {
                    let b = &*hw.bus;
                    let vo = &mut self.voices[ch];
                    let mut at = vo.list.wrapping_add(vo.pos);
                    let mut pos = vo.pos.wrapping_add(2);
                    if b.r16(sx(at)) == 0 {
                        at = vo.list;
                        pos = 2;
                    }
                    vo.track = sx(b.r16(sx(at)));
                    vo.pos = pos;
                }
                Fx::Slide => {
                    let s = self.next(ch, hw);
                    let d = self.next(ch, hw);
                    let vo = &mut self.voices[ch];
                    vo.slide = 0;
                    vo.slide_speed = s;
                    vo.slide_delay = d;
                    vo.flags |= 2;
                }
                Fx::Mute => {
                    let vo = &mut self.voices[ch];
                    vo.counter = vo.speed;
                    self.sh.loc[ch] = hw.bus.r32(v.empty);
                    self.sh.len[ch] = v.empty_len;
                    return Next::Voice;
                }
                Fx::Off { done } => {
                    let vo = &mut self.voices[ch];
                    vo.counter = vo.speed;
                    if done {
                        vo.loop_done = true;
                    }
                    self.sh.dmacon(1 << ch, hw);
                    return Next::Voice;
                }
                Fx::Wait { dma } => {
                    let vo = &mut self.voices[ch];
                    vo.counter = vo.speed;
                    if dma {
                        self.sh.dmacon(0x8000 | 1 << ch, hw);
                    }
                    return Next::Voice;
                }
                Fx::Half(on) => self.voices[ch].transpose = if on { 0xff } else { 0 },
                Fx::StopRoutine => {
                    self.stop(hw);
                    return Next::Stop;
                }
                Fx::ClearStop(f) => {
                    hw.bus.w8(f, 0);
                    self.stop(hw);
                    return Next::Stop;
                }
                Fx::Stop { vols } => {
                    self.stop_song(vols, hw);
                    return Next::Stop;
                }
                Fx::SetByte(a) => {
                    let x = self.next(ch, hw);
                    hw.bus.w8(a, x);
                }
                Fx::SetWord(a) => {
                    let x = self.next(ch, hw);
                    hw.bus.w16(a, x as i8 as i16 as u16);
                }
                Fx::Fade(s, c) => {
                    let x = self.next(ch, hw);
                    hw.bus.w8(s, x);
                    hw.bus.w8(c, x);
                }
                Fx::Vibrato => {
                    let s = self.next(ch, hw);
                    let m = self.next(ch, hw);
                    let vo = &mut self.voices[ch];
                    vo.vib = 0xff;
                    vo.vib_speed = s;
                    vo.vib_max = m;
                    vo.vib_value = 0;
                }
                Fx::VibratoOff => self.voices[ch].vib = 0,
                Fx::Transpose => self.voices[ch].transpose = self.next(ch, hw),
                Fx::PositionList => {
                    let h = self.next(ch, hw);
                    let l = self.next(ch, hw);
                    let vo = &mut self.voices[ch];
                    vo.list = (h as u16) << 8 | l as u16;
                    vo.pos = 0;
                }
                Fx::Nop => {}
            }
        }
        Next::Voice
    }

    fn period(&self, b: &crate::bus::Bus, s: u32, note: u8) -> u32 {
        let p = b.r16(self.v.periods + 2 * note as u32) as u32;
        (p * b.r16(s.wrapping_add(10)) as u32) >> 10
    }

    fn scale(&self, b: &crate::bus::Bus, vol: u16) -> u16 {
        match self.v.gvol {
            Some(g) => ((vol as u32 * b.r16(g) as u32) as u16) >> 6,
            None => vol,
        }
    }

    /// An envelope volume as the voice writes it.
    fn env_vol(&self, b: &crate::bus::Bus, ch: usize, vol: u16) -> u16 {
        let mut d1 = self.scale(b, vol);
        if self.v.vol_half && self.voices[ch].transpose != 0 {
            d1 >>= 1;
        }
        if let Some(f) = self.v.vol_fade {
            d1 = d1.checked_sub(b.r16(f)).unwrap_or(0);
        }
        d1
    }

    fn note(&mut self, ch: usize, byte: u8, hw: &mut Hw) {
        let v = &self.v;
        let b = &*hw.bus;
        let s = self.voices[ch].sample;
        let mut d0 = byte;
        for op in &v.note_ops {
            match *op {
                NoteOp::SampleTranspose => d0 = d0.wrapping_add(b.r8(s.wrapping_add(14))),
                NoteOp::Keep => self.voices[ch].note = d0,
                NoteOp::GlobalTranspose(g) => d0 = d0.wrapping_add(b.r8(g)),
                NoteOp::Transpose => d0 = d0.wrapping_add(self.voices[ch].transpose),
            }
        }
        let vol = if v.env {
            let vo = &mut self.voices[ch];
            let e = vo.env_start;
            let d1 = b.r8(e) as i8 as i16 as u16;
            vo.env = e.wrapping_add(1);
            vo.env_count = vo.env_speed;
            self.env_vol(b, ch, d1)
        } else if let Some(f) = v.vol_sub {
            b.r16(s.wrapping_add(12)).wrapping_sub(b.r16(f))
        } else {
            self.scale(b, b.r16(s.wrapping_add(12)))
        };
        self.sh.loc[ch] = b.r32(s);
        self.sh.len[ch] = b.r16(s.wrapping_add(8));
        self.sh.vol[ch] = vol;
        self.sh.per[ch] = self.period(b, s, d0) as u16;
        let vo = &mut self.voices[ch];
        vo.loop_done = false;
        vo.counter = vo.speed;
        self.sh.dmacon(0x8000 | 1 << ch, hw);
    }

    fn frame(&mut self, ch: usize, hw: &mut Hw) {
        hw.wait(T_FRAME);
        let v = &self.v;
        let b = &*hw.bus;
        let vo = &mut self.voices[ch];
        let s = vo.sample;
        let mut d0 = vo.note;
        if let Some(g) = v.frame_gtransp {
            d0 = d0.wrapping_add(b.r8(g));
        }
        if v.frame_ctransp {
            d0 = d0.wrapping_add(vo.transpose);
        }
        if v.arp {
            let mut a1 = vo.arp;
            let mut d1 = b.r8(a1);
            a1 = a1.wrapping_add(1);
            if d1 & 0x80 != 0 {
                d1 &= 0x7f;
                a1 = vo.arp_start;
            }
            vo.arp = a1;
            d0 = d0.wrapping_add(d1);
        }
        let p = b.r16(v.periods + 2 * d0 as u32) as u32;
        let mut per = ((p * b.r16(s.wrapping_add(10)) as u32) >> 10) as u16;
        if v.slide && vo.flags & 2 != 0 {
            if vo.slide_delay != 0 {
                vo.slide_delay -= 1;
            } else {
                vo.slide = vo.slide.wrapping_add(vo.slide_speed as i8 as i16 as u16);
                per = per.wrapping_sub(vo.slide);
            }
        }
        if v.vib.is_some() && vo.vib != 0 {
            let d1 = if vo.vib & 0x80 != 0 {
                let d1 = vo.vib_value.wrapping_add(vo.vib_speed);
                vo.vib_value = d1;
                if d1 == vo.vib_max {
                    vo.vib ^= 0x80;
                }
                d1
            } else {
                let d1 = vo.vib_value.wrapping_sub(vo.vib_speed);
                vo.vib_value = d1;
                if d1 == 0 {
                    vo.vib ^= 0x80;
                }
                d1
            };
            if vo.vib_value == 0 {
                vo.vib ^= 1;
            }
            let d = d1 as i8 as i16 as u16;
            per = if vo.vib & 1 != 0 { per.wrapping_add(d) } else { per.wrapping_sub(d) };
        }
        self.sh.per[ch] = per;
        if v.env {
            let (c, borrow) = vo.env_count.overflowing_sub(1);
            vo.env_count = c;
            if borrow {
                vo.env_count = vo.env_speed;
                let mut d1 = b.r8(vo.env);
                if d1 & 0x80 == 0 {
                    vo.env = vo.env.wrapping_add(1);
                }
                d1 &= 0x7f;
                self.sh.vol[ch] = self.env_vol(b, ch, d1 as u16);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::collections::HashMap;
    use std::path::Path;

    /// A little assembler: words, labels, and references to labels (resolved
    /// at the end).
    #[derive(Default)]
    struct Asm {
        d: Vec<u8>,
        labels: HashMap<&'static str, usize>,
        /// (where, label, kind): 0 pc-relative word, 1 short branch, 2 offset.
        fix: Vec<(usize, &'static str, u8)>,
    }

    impl Asm {
        fn w(&mut self, ws: &[u16]) -> &mut Self {
            for &x in ws {
                self.d.extend(x.to_be_bytes());
            }
            self
        }
        fn l(&mut self, name: &'static str) -> &mut Self {
            self.labels.insert(name, self.d.len());
            self
        }
        /// A pc-relative displacement to `name`.
        fn r(&mut self, name: &'static str) -> &mut Self {
            self.fix.push((self.d.len(), name, 0));
            self.w(&[0])
        }
        /// A short branch (its high byte `op`) to `name`.
        fn s(&mut self, op: u8, name: &'static str) -> &mut Self {
            self.fix.push((self.d.len(), name, 1));
            self.w(&[(op as u16) << 8])
        }
        /// `name`'s offset in the module (the replayer's variables are at 0).
        fn a(&mut self, name: &'static str) -> &mut Self {
            self.fix.push((self.d.len(), name, 2));
            self.w(&[0])
        }
        fn bytes(&mut self) -> Vec<u8> {
            for &(at, name, kind) in &self.fix {
                let t = self.labels[name] as i64;
                match kind {
                    0 => self.d[at..at + 2].copy_from_slice(&((t - at as i64) as u16).to_be_bytes()),
                    1 => self.d[at + 1] = (t - at as i64 - 2) as u8,
                    _ => self.d[at..at + 2].copy_from_slice(&(t as u16).to_be_bytes()),
                }
            }
            self.d.clone()
        }
    }

    /// A replayer as the games have it, with one subsong whose four voices
    /// play the same track: a duration, a note, and the jump to the next
    /// position (the same one again); one sample of 32 bytes with no loop.
    fn module() -> Vec<u8> {
        let mut a = Asm::default();
        // InitSound: lea 0(pc),a3 / bsr stop / the sample loop / the subsong
        a.l("base").w(&[0x47fa]).r("base").w(&[0x6100]).r("stop");
        a.w(&[0x4a2b]).a("flag").s(0x66, "sdone");
        a.w(&[0x41fa]).r("sdata").w(&[0x4bfa]).r("sinfo").w(&[0x7200]);
        a.l("sloop").w(&[0x2418, 0x3618, 0x2b48, 0x0000, 0xd1c2, 0xdafc, 0x0010, 0x51c9]).r("sloop");
        a.w(&[0x2748]).a("empty");
        a.l("sdone").w(&[0x4880, 0xc0fc, 0x000a, 0x41fa]).r("subs").w(&[0x3770, 0x0000]).a("speed");
        a.w(&[0x43fa]).r("voices").w(&[0x7e00]);
        a.l("vinit").w(&[0x337c, 0x0001, 0x001e, 0x50e9, 0x0016, 0x41fa]).r("arp");
        a.w(&[0x2348, 0x000c, 0x2348, 0x0010, 0x41fa]).r("subs");
        a.w(&[0x3070, 0x0002, 0x3348, 0x0008, 0xd2fc, 0x0030, 0x5440, 0x5247, 0xbe7c, 0x0004]).s(0x66, "vinit");
        a.w(&[0x50eb]).a("playing").w(&[0x4e75]);
        // the stop routine: DMA off, the filter, volumes 0
        a.l("stop").w(&[0x47fa]).r("base").w(&[0x51eb]).a("playing");
        a.w(&[0x33fc, 0x000f, 0x00df, 0xf096, 0x33fc, 0x00ff, 0x00df, 0xf09e]);
        for ch in 0..4 {
            a.w(&[0x33fc, 0x0000, 0x00df, 0xf0a8 + 16 * ch]);
        }
        a.w(&[0x4e75]);
        // the play routine and its voice loop
        a.l("play").w(&[0x47fa]).r("base").w(&[0x4a2b]).a("playing").s(0x67, "rts").w(&[0x7e00]);
        a.l("voice").w(&[0x4df9, 0x00df, 0xf000, 0x303c, 0x0030, 0xc0c7, 0x41fa]).r("voices");
        // the loop (none: the empty sample) on the tick after the note
        a.w(&[0x202d, 0x0004]).s(0x6b, "noloop");
        a.l("noloop").w(&[0x2d7a]).r("empty").w(&[0x00a0, 0x3d7c, 0x0002, 0x00a4]);
        // the counter: the row at 0, the voice off at 1, else the frame
        a.w(&[0x5368, 0x001e]).s(0x67, "read").w(&[0x0c68, 0x0001, 0x001e, 0x6600]).r("frame");
        a.w(&[0x4241, 0x0fc1, 0x33c1, 0x00df, 0xf096, 0x6000]).r("next");
        a.l("read").w(&[0x4228, 0x0000, 0x7000, 0x1019, 0x6b00]).r("cmd");
        a.w(&[0x1140, 0x0002, 0x3d6d, 0x000c, 0x00a8, 0x0240, 0x00ff, 0x45fa]).r("periods").w(&[0x322d, 0x000a]);
        a.w(&[0x3d40, 0x00a6, 0x323c, 0x8200, 0x0fc1, 0x33c1, 0x00df, 0xf096, 0x6000]).r("next");
        a.l("frame").w(&[0x1028, 0x0002, 0x45fa]).r("periods").w(&[0x322d, 0x000a, 0x3d41, 0x00a6]);
        a.l("next").w(&[0x5247, 0xbe7c, 0x0004, 0x6600]).r("voice");
        a.l("rts").w(&[0x4e75]);
        // the commands: a duration from $e0, then the effects' jump table
        a.l("cmd").w(&[0xb03c, 0x00e0]).s(0x6d, "fx").w(&[0x0400, 0x00df, 0xc0fa]).r("speed");
        a.w(&[0x3140, 0x001c, 0x6000]).r("read");
        a.l("fx").w(&[0xd000, 0x45fa]).r("jt").w(&[0x3472, 0x0000, 0x4ef3, 0xa000]);
        a.l("fx0").w(&[0x3028, 0x000a, 0x3468, 0x0008, 0x6000]).r("read");
        a.l("jt").a("fx0").w(&[0]);
        // the variables, the song, the tables
        a.l("playing").w(&[0]);
        a.l("flag").w(&[0]);
        a.l("speed").w(&[0]);
        a.l("empty").w(&[0, 0]);
        a.l("list").a("track").w(&[0]);
        a.l("track").w(&[0xe012, 0x8000]);
        a.l("subs").w(&[6]).a("list").a("list").a("list").a("list").w(&[0; 5]);
        a.l("arp").w(&[0x0080]);
        a.l("voices").w(&[0; 4 * 0x18]);
        a.l("periods").w(&[0; 0x12]).w(&[0x400]);
        // the sample's table entry: no loop, volume 64
        a.l("sinfo").w(&[0, 0, 0xffff, 0xffff, 0, 0, 0x40, 0]);
        // the sample: 32 bytes at 8287 Hz
        a.l("sdata").w(&[0, 0x20, 8287]).w(&[0x4000; 16]);
        a.bytes()
    }

    #[test]
    fn plays_a_note_on_paula() {
        let d = module();
        let len = d.len() as u32;
        assert!(super::detect(&d));
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.dw"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "David Whittaker");
        let base = e.m.deli.list.first().unwrap().0;
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound's stop; each voice's note; each voice off the tick before
        // its next row; the notes again
        assert_eq!(&reg(0xDFF096)[..10], &[0x000F, 0x8001, 0x8002, 0x8004, 0x8008, 1, 2, 4, 8, 0x8001]);
        // the note: the sample (its data after the length and frequency, at the
        // module's end), 16 words, its volume, period $400 * 3579545 / 8287 >> 10;
        // on the next tick the empty sample after it, 2 words
        let loc = (base + len - 32) as u16;
        assert_eq!(&reg(0xDFF0A2)[..2], &[loc, loc + 32]);
        assert_eq!(&reg(0xDFF0A4)[..2], &[16, 2]);
        assert_eq!(&reg(0xDFF0A8)[..2], &[0, 64]);
        assert_eq!(reg(0xDFF0A6)[0], 0x1AF);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
