//! Mike Davies (`md.*`), the sound driver Mike Davies wrote for the US Gold / Tiertex
//! conversions of 1989-91 (Strider, Strider II, Mercs, Alien Storm, Die Hard 2, Dynasty
//! Wars, Indiana Jones 3 Action, Italy 1990 Winners Edition, World Class Soccer).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Mike Davies player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A `md.*` file is a memory image cut from the game: the game's driver code, its
//! variables, the song, instrument and period tables, the tracks and the samples,
//! with absolute pointers to the game's load address. The code is not run. It is
//! read only to find the data, by generic instruction signposts (the driver's
//! "playing" flag store gives the game address, the address loads and compares
//! before and after the play routine give the song, effect and instrument tables
//! and the period table), and to tell the four builds apart (track encoding, rest
//! flag, low-period clamp or skip, transposition of note 0, passes per tick). The
//! data is then prepared as the reference host does (the pointers converted to the
//! image's address, the instrument tables of Italy 1990 / World Class Soccer and of
//! Mercs repaired) and the engine (four voices, a main track and a called pattern
//! each, loops, instruments, 50 Hz) is written from its behaviour. Every write lands
//! on the reference's colour clock: the host's start-up and change of subsong are
//! modelled from the file, and each tick's writes from the 68000 cycles of the paths
//! the driver takes (measured on the reference's write logs, black box).

use super::{be16, be32, Hw, Open, Replayer, DT_TICK};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Mike Davies", detect, new };

// The prepared data holds absolute pointers to the image's guest address, and the one period read
// past the period table (Alien Storm song 5) reads such a pointer: the port prepares the image for
// the address the host loaded it at.

/// Track elements (the first unit).
const END_PATTERN: u16 = 0x94;
const CALL: u16 = 0x95;
const LOOP_END: u16 = 0x9D;
const LOOP_START: u16 = 0x9E;
const RESTART: u16 = 0xFF;
const STOP: u16 = 0xFE;

/// The "end words" the image-extent search of the preparation stops at.
const END_WORDS: [u16; 5] = [0x0094, 0x00FE, 0x00FF, 0x01FE, 0x01FF];

// The tick's timing: every write at the original's colour clock, the tick's start plus half the
// 68000 cycles since, rounded down (measured on the reference's write logs and VHPOSR reads of
// the 9 corpus files, 20 s each, a run and a second one with the cycle count started a cycle
// late). All costs below are in cycles.

/// The sequencer, from the tick's start to the output's first voice: four voices that only count
/// down (dmacon_rec builds 4 more); a voice that is off (its track pointer negative) 27 less; a
/// voice that reads its next note 289 more (byte-coded builds 12 more), each element read from
/// the main track rather than a pattern (but a call) 8 less, a note 0 12 less for each
/// transposition it skips (two in zero_both builds); an instrument element 571 (16 more in the
/// rest builds, 4 more in the byte-coded ones, 8 more when its number is past the table), a call
/// 226, a pattern end 160 and 34 more when its repeat count runs out, a loop start 240, a loop end
/// 208 and 69 more when it loops back (odd, but the 10 ticks with loops leave no other value).
/// Fitted exactly on the first DMA-off write of every tick of the 9 corpus files (1004 ticks, a
/// run and a second one with the cycle count started a cycle late); the loops and the instrument
/// past the table on their other subsongs.
const SEQ_START: i64 = 861;
const SEQ_REC: i64 = 4;
const SEQ_OFF: i64 = -27;
const SEQ_FETCH: i64 = 289;
const SEQ_FETCH_BYTES: i64 = 12;
const SEQ_MAIN: i64 = -8;
const SEQ_ZERO: i64 = -12;
const SEQ_INS: i64 = 571;
const SEQ_INS_REST: i64 = 16;
const SEQ_INS_BYTES: i64 = 4;
const SEQ_INS_CLAMP: i64 = 8;
const SEQ_CALL: i64 = 226;
const SEQ_END_PATTERN: i64 = 160;
const SEQ_LAST_REPEAT: i64 = 34;
const SEQ_LOOP_END: i64 = 208;
const SEQ_LOOP_BACK: i64 = 69;
const SEQ_LOOP_START: i64 = 240;
/// A restart or stop element: its first volume clear from the pass's start, with every voice
/// before it 72 (an idle one) more and the sequencer's costs above for what they read, as for the
/// elements the voice read before; the A builds 20 more, the byte-coded ones 4; a restart 178
/// more (A builds 197). The clears 20 apart, the DMA stop 40 after the last, the ADKCON write of
/// the restart 230 after it (A builds 231). Fitted on the 32 stops and restarts of every subsong
/// of the 9 corpus files (the run's colour clocks only: the run with the cycle count started a
/// cycle late is off here by a few clocks per number of the song).
const END_START: i64 = 1478;
const END_A: i64 = 20;
const END_BYTES: i64 = 4;
const END_RESTART: i64 = 178;
const END_RESTART_A: i64 = 19;
const END_IDLE: i64 = 72;
const END_VOL: i64 = 20;
const END_DMA: i64 = 40;
const END_ADK: i64 = 230;
const END_ADK_A: i64 = 1;
/// From a restart's ADKCON write, the half cycle dropped, to the start of the next pass (its
/// sequencer's `SEQ_START`).
const END_NEXT: i64 = 54;
/// Path counters (indices into `tl`).
const C_OFF: usize = 0;
const C_IDLE: usize = 1;
const C_FETCH: usize = 2;
const C_END_PATTERN: usize = 3;
const C_LAST_REPEAT: usize = 4;
const C_CALL: usize = 5;
const C_LOOP_END: usize = 6;
const C_LOOP_BACK: usize = 7;
const C_LOOP_START: usize = 8;
const C_INS: usize = 9;
const C_ZERO: usize = 10;
const C_MAIN: usize = 11;
const C_INS_CLAMP: usize = 12;
const C_COUNT: usize = 13;

/// The output routine: a voice that does not start; from the DMA-off write to the location
/// write, to the length, to the period; from the period to the volume (builds A: 27 more).
const OUT_IDLE: i64 = 41;
const OUT_LOC: i64 = 76;
const OUT_LEN: i64 = 20;
const OUT_PER: i64 = 20;
const OUT_VOL: [i64; 4] = [468, 491, 514, 504];
const OUT_VOL_A: i64 = 27;
/// From a voice's volume write to the next voice's DMA-off write (31 more for each channel
/// number of the first); with the DMA write from the voice record (`dmacon_rec`), 4 more.
const OUT_NEXT: i64 = 237;
const OUT_NEXT_CH: i64 = 31;
/// A voice skipped for a period below the threshold: 73 more than one that does not start; one
/// whose period is raised to it (the `clamp` builds), 40 more before its DMA-off write.
const OUT_LOW: i64 = 73;
const OUT_CLAMP: i64 = 40;
const OUT_REC: i64 = 4;
/// From the last voice's volume write to the first read of the beam, by its channel.
const OUT_WAIT: [i64; 4] = [369, 359, 349, 316];
/// The beam wait's polling (as UFO's): the second read 16 after the first, then one every 25; a
/// read on another line ends a round, the next round's first read 35 after it; 9 rounds.
const POLL_SECOND: i64 = 16;
const POLL: i64 = 25;
const POLL_NEXT: i64 = 35;
const WAIT_LINES: u32 = 9;
/// The DMA-on loop, from the first wait's last read: its start; per voice, one that starts (with
/// the rest flag, 21 more; with `dmacon_rec`, 4 more), one with the rest note (rest flag only),
/// one not written; from its end to the second wait's first read.
const ON_START: i64 = 65;
const ON_VOICE: i64 = 73;
const ON_REST_FLAG: i64 = 21;
const ON_REST: i64 = 78;
const ON_END: i64 = 82;
/// The loop writes, from the second wait's last read: to the first voice's location write; per
/// voice not written; a looped voice's more before its location write; from the location to the
/// length write (looped 12, else 16); from the length write to the next voice's location write, 10
/// more after a looped voice.
const LOOP_FIRST: i64 = 201;
const LOOP_LOOPED: i64 = 60;
const LOOP_LEN: i64 = 16;
const LOOP_LEN_LOOPED: i64 = 12;
const LOOP_NEXT: i64 = 136;
const LOOP_AFTER_LOOPED: i64 = 10;

/// The instrument record and the instrument count's step.
const REC: usize = 0x18;

// ------------------------------------------------------- the original's start-up
//
// The song starts on the clock the original's start-up reaches, which its beam waits read as
// an absolute raster position. Measured on the reference's runs (a run and a second one with
// the cycle count started a cycle late give each stretch's exact 68000 cycles) of the 9 corpus
// files and of about 2 500 one-change copies of them: most of it is the Eagleplayer's
// InitPlayer, which finds the tables, converts the pointers (`prepare`) and converts the
// absolute operands of the driver code (`reloc_cycles`).

/// Per word of the driver code that is not one of the converted instructions.
const C_WORD: i64 = 317;
/// The converted instructions (opcode, operand words, extra cycles): matched in this order, the
/// k-th at 100 + 21 k cycles, its operand words skipped; 584 more when the (last) absolute
/// address is a custom register's (0x00DFxxxx).
const RELOC: [(u16, usize, i64); 14] = [
    (0x3039, 2, 0),
    (0x33C0, 2, 0),
    (0x33C1, 2, 0),
    (0x33EE, 3, 8),
    (0x33F9, 4, 63),
    (0x33FC, 3, 8),
    (0x3C39, 2, 0),
    (0x41F9, 2, 0),
    (0x4279, 2, 0),
    (0x4A79, 2, 0),
    (0x4BF9, 2, 0),
    (0x4DF9, 2, 0),
    (0x5379, 2, 0),
    (0xB1FC, 2, 0),
];
const C_CUSTOM: i64 = 584;
/// A move between two absolute addresses whose source is a custom register.
const C_CUSTOM_SRC: i64 = -22;
/// A step of the searches for one instruction word, and of those for two (the entry, the playing
/// flag, the play routine).
const C_FIND: i64 = 21;
const C_FIND2: i64 = 47;
/// InitPlayer's fixed part; with the tracks below the samples; with no effect table.
const C_INIT: i64 = 3672;
const C_LOW_TRACKS: i64 = -3849;
const C_NO_FX: i64 = -136;
/// The instrument count one record short; the Italy 1990 move; the Mercs repair.
const C_COUNT_FIX: i64 = 192;
const C_FTD: i64 = 152;
const C_MERCS: i64 = 176;
/// A sample pointer; a new lowest one; a new end of the samples.
const C_REC: i64 = 104;
const C_LOWER: i64 = 12;
const C_END: i64 = 48;
/// A song entry; an unused one (0xFFFFFFFF); a new highest pointer.
const C_ENTRY: i64 = 104;
const C_UNUSED: i64 = 41;
const C_HIGHER: i64 = 12;
/// The call-pattern scan: a word, a call (with the tracks below the samples: 88).
const C_SCAN: i64 = 40;
const C_HIT: i64 = 111;
const C_HIT_LOW: i64 = 88;
/// The search for an end word: per word, 21 more for each end word before the one found; the
/// second search.
const C_SEARCH: i64 = 123;
const C_SEARCH_END: i64 = 21;
const C_SEARCH_AGAIN: i64 = -56;

/// The cycles of the driver-code conversion: the words from the driver's entry to the period table.
fn reloc_cycles(d: &[u8], from: usize, to: usize) -> i64 {
    let mut o = from;
    let mut c = 0;
    while o < to {
        let v = w(d, o);
        match RELOC.iter().position(|r| r.0 == v) {
            Some(k) => {
                let (_, n, x) = RELOC[k];
                c += 100 + 21 * k as i64 + x;
                if w(d, o + 2 * n - 2) == 0x00DF {
                    c += C_CUSTOM;
                }
                if v == 0x33F9 && w(d, o + 2) == 0x00DF {
                    c += C_CUSTOM_SRC;
                }
                o += 2 * (n + 1);
            }
            None => {
                c += C_WORD;
                o += 2;
            }
        }
    }
    c
}

/// The first even offset at or after `from` where `pat` stands, counting the positions looked at.
fn find_n(d: &[u8], from: usize, pat: &[u16], n: &mut i64) -> Option<usize> {
    let o = find(d, from, pat)?;
    *n += ((o - from) / 2 + 1) as i64;
    Some(o)
}

/// The 68000 cycles from InitPlayer's start to its end (between the host's Deli(0) and Eagle(35)
/// traps): its searches for the tables, the code conversion and the preparation (`prep`).
fn stretch(d: &[u8], lay: &Lay, prep: i64) -> Option<i64> {
    let (mut n1, mut n2) = (0, 0);
    let e = find_n(d, 0, &[0x48E7, 0xFFFE], &mut n2)?;
    let a = find_n(d, e + 4, &[0x33FC, 0x0001], &mut n2)?;
    find_n(d, a, &[0x4DF9], &mut n1)?;
    let s = find_n(d, a, &[0x41F9], &mut n1)?;
    let se = find_n(d, s + 6, &[0xB1FC], &mut n1)?;
    let x = find_n(d, se + 6, &[0x41F9], &mut n1)?;
    let xe = find_n(d, x + 6, &[0xB1FC], &mut n1)?;
    let delay = find_n(d, xe + 6, &[0x303C], &mut n1)?;
    let mut p = delay + 4;
    let p = loop {
        p = find_n(d, p, &[0x48E7, 0xFFFE], &mut n2)?;
        if w(d, p + 8) == 0x6100 {
            break p;
        }
        p += 2;
    };
    let i = find_n(d, p + 4, &[0x41F9], &mut n1)?;
    find_n(d, i + 6, &[0xB1FC], &mut n1)?;
    find_n(d, p + 4, &[0x0C40], &mut n1)?;
    Some(C_INIT + C_FIND * n1 + C_FIND2 * n2 + reloc_cycles(d, e, lay.periods) + prep)
}

/// The 68000 cycles of the host's check call (DTP_Check: the test of `detect`).
fn check_cycles(d: &[u8]) -> i64 {
    let (a, base) = if w(d, 0xF2) == 0x0C40 { (0xF2, 261) } else { (0xEC, 290) };
    let b = find(d, a - 12, &[0x6000]).unwrap_or(a - 12);
    base + C_FIND * ((b - (a - 12)) / 2) as i64
}

// ---------------------------------------------------------------- recognition

fn w(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

/// The first even offset at or after `from` where the words `pat` stand.
fn find(d: &[u8], from: usize, pat: &[u16]) -> Option<usize> {
    let mut o = from;
    while o + 2 * pat.len() <= d.len() {
        if pat.iter().enumerate().all(|(i, &p)| w(d, o + 2 * i) == p) {
            return Some(o);
        }
        o += 2;
    }
    None
}

/// The Eagleplayer's test (specification, section 10).
fn detect(d: &[u8]) -> bool {
    let a = if w(d, 0xF2) == 0x0C40 {
        0xF2
    } else if w(d, 0xEC) == 0x33C0 {
        0xEC
    } else {
        return false;
    };
    if be32(d, a - 4) != 0x7FFF_4E75 || be32(d, a - 8) != 0x6106_4CDF || be32(d, a - 12) != 0x48E7_FFFE {
        return false;
    }
    let Some(b) = find(d, a - 12, &[0x6000]) else { return false };
    let disp = w(d, b + 2) as i16;
    disp > 0 && disp % 2 == 0 && w(d, b + 2 + disp as usize) == 0x2D58
}

// ------------------------------------------------------------------ the layout

/// What the image's driver code says (file offsets).
#[derive(Clone, Debug, PartialEq)]
struct Lay {
    g: u32,
    songs: usize,
    songs_end: usize,
    fx: usize,
    fx_end: usize,
    ins: usize,
    /// The instrument table's end as the driver has it (before the count fix).
    ins_end: usize,
    periods: usize,
    call: u16,
    /// Byte-coded tracks (A2) rather than word-coded.
    bytes: bool,
    /// The instrument loader sets the note to 1, and note 0 is a rest (A1, A2).
    rest: bool,
    /// A period below `thr` is raised to it (A1, A2), else the voice is skipped.
    clamp: bool,
    thr: u16,
    /// Note 0 is left untransposed by the instrument as well (A1, A2).
    zero_both: bool,
    passes: usize,
    /// The A builds' slower volume path (the signpost at 0xF2, Strider, Strider II, Mercs, Alien Storm).
    a_build: bool,
    /// The output routine writes DMACON from the voice record (a `move.w d16(a6),$DFF096`).
    dmacon_rec: bool,
}

fn operand(d: &[u8], at: usize, g: u32) -> Option<usize> {
    let p = be32(d, at + 2).wrapping_sub(g) as usize;
    (p < d.len()).then_some(p)
}

/// Finds the tables and the build (specification, sections 2 and 8).
fn layout(d: &[u8]) -> Option<Lay> {
    let e = find(d, 0, &[0x48E7, 0xFFFE])?;
    let a = find(d, e + 4, &[0x33FC, 0x0001])?;
    let g = be32(d, a + 4);
    let vb = find(d, a, &[0x4DF9])?;
    operand(d, vb, g)?;
    let s = find(d, a, &[0x41F9])?;
    let se = find(d, s + 6, &[0xB1FC])?;
    let x = find(d, se + 6, &[0x41F9])?;
    let xe = find(d, x + 6, &[0xB1FC])?;
    let (songs, songs_end, fx, fx_end) = (operand(d, s, g)?, operand(d, se, g)?, operand(d, x, g)?, operand(d, xe, g)?);
    let delay = find(d, xe + 6, &[0x303C])?;
    let mut p = delay + 4;
    let p = loop {
        p = find(d, p, &[0x48E7, 0xFFFE])?;
        if w(d, p + 8) == 0x6100 {
            break p;
        }
        p += 2;
    };
    // the play routine's list of word-sized calls
    let mut targets = Vec::new();
    let mut o = p + 4;
    while w(d, o) == 0x6100 {
        targets.push((o + 2).wrapping_add(w(d, o + 2) as i16 as isize as usize));
        o += 4;
    }
    let seq = *targets.first()?;
    let passes = targets.iter().filter(|&&t| t == seq).count();
    let i = find(d, p + 4, &[0x41F9])?;
    let ie = find(d, i + 6, &[0xB1FC])?;
    let t = find(d, ie + 6, &[0x4BF9])?;
    let (ins, ins_end, periods) = (operand(d, i, g)?, operand(d, ie, g)?, operand(d, t, g)?);
    let c = find(d, p + 4, &[0x0C40])?;
    let call = w(d, c + 2 + 0x16);
    // build properties
    let tf = find(d, e, &[0xD0ED, 0x001C])?;
    let bytes = w(d, tf + 4) == 0x7000;
    let mut rest = false;
    let mut r = e;
    while let Some(o) = find(d, r, &[0x3D7C, 0x0001, 0x0008]) {
        if o >= 4 && w(d, o - 2) == 0x000C && w(d, o - 4) & 0xFFF8 == 0x3D40 {
            rest = [0x3D7C, 0x0001, 0x0034].iter().enumerate().all(|(k, &v)| w(d, o + 6 + 2 * k) == v);
            break;
        }
        r = o + 2;
    }
    let mut lp = e;
    let (thr, clamp) = loop {
        let o = find(d, lp, &[0x0C6E])?;
        if w(d, o + 4) == 0x0004 {
            match w(d, o + 10) {
                0x33EE => break (w(d, o + 2), true),
                0x426E => break (w(d, o + 2), false),
                _ => {}
            }
        }
        lp = o + 2;
    };
    let n0 = find(d, seq, &[0x4A40])?;
    let zero_both = match (w(d, n0 + 2), w(d, n0 + 4)) {
        (0x6700, 10) | (0x670A, _) => true,
        (0x6700, 6) | (0x6706, _) => false,
        _ => return None,
    };
    if songs_end < songs || fx_end < fx || ins_end <= ins || periods <= e {
        return None;
    }
    let a_build = w(d, 0xF2) == 0x0C40;
    let dmacon_rec = (e..periods).step_by(2).any(|o| o + 8 <= d.len() && w(d, o) == 0x33EE && be32(d, o + 4) == 0x00DF_F096);
    Some(Lay { g, songs, songs_end, fx, fx_end, ins, ins_end, periods, call, bytes, rest, clamp, thr, zero_both, passes, a_build, dmacon_rec })
}

// ----------------------------------------------------------------- preparation

fn put32(d: &mut [u8], o: usize, v: u32) -> Option<()> {
    d.get_mut(o..o + 4)?.copy_from_slice(&v.to_be_bytes());
    Some(())
}

/// Prepares a copy of the image for address `virt` (specification, section 3):
/// the image, the instrument count and the 68000 cycles the original's preparation takes
/// (`InitPlayer`'s, see `stretch`) for its loops and branches.
fn prepare(d: &[u8], lay: &Lay, virt: u32) -> Option<(Vec<u8>, usize, i64)> {
    let mut img = d.to_vec();
    let mut cy = 0i64;
    let conv = |p: u32| p.wrapping_sub(lay.g).wrapping_add(virt);
    // 1. the instrument count
    let n = lay.ins_end - lay.ins;
    let count = if n % REC == 0 {
        n / REC
    } else if (n + 4) % REC == 0 {
        cy += C_COUNT_FIX;
        if img.get(0x7D8..0x7DC) == Some(b"FTD<") {
            // the missing loop field of the next-to-last record (Italy 1990, World Class Soccer)
            if img.len() < 0x774 {
                return None;
            }
            img.copy_within(0x764..0x770, 0x768);
            put32(&mut img, 0x764, 0)?;
            cy += C_FTD;
        }
        (n + 4) / REC
    } else {
        return None;
    };
    if lay.fx_end == lay.fx {
        cy += C_NO_FX;
    }
    // 2. the sample pointers
    let recs = (0..count).map(|k| lay.ins + REC * k).chain((0..(lay.fx_end - lay.fx) / REC).map(|k| lay.fx + REC * k));
    let (mut d2, mut d5) = (i32::MAX, None::<i32>);
    for o in recs.collect::<Vec<_>>() {
        if o + 8 > img.len() {
            return None;
        }
        let p = conv(be32(&img, o));
        put32(&mut img, o, p)?;
        cy += C_REC;
        if p as i32 <= d2 {
            cy += C_LOWER;
        }
        d2 = d2.min(p as i32);
        let end = p.wrapping_add(be32(&img, o + 4).wrapping_mul(2)) as i32;
        match d5 {
            Some(e) if (p as i32) < e => {}
            _ => {
                d5 = Some(end);
                cy += C_END;
            }
        }
    }
    let d5 = d5?;
    // 3. the song table, whole songs of four entries
    let mut d3 = i32::MIN;
    let mut o = lay.songs;
    while o < lay.songs + (lay.songs_end - lay.songs) / 16 * 16 {
        let v = be32(&img, o);
        if v != 0xFFFF_FFFF {
            let c = conv(v);
            put32(&mut img, o, c)?;
            cy += C_ENTRY;
            if c as i32 >= d3 {
                cy += C_HIGHER;
            }
            d3 = d3.max(c as i32);
        } else {
            cy += C_UNUSED;
        }
        o += 4;
    }
    // 4. the call-pattern pointers in the tracks
    let off = |a: i32| (a as u32).wrapping_sub(virt) as usize;
    let scan = |img: &mut Vec<u8>, from: usize, to: usize, d3: Option<&mut i32>, cy: &mut i64| -> Option<()> {
        let mut o = from;
        let mut hi = d3;
        while o < to {
            if o + 2 > img.len() {
                return None;
            }
            if w(img, o) == lay.call {
                let c = conv(be32(img, o + 6));
                put32(img, o + 6, c)?;
                match hi.as_deref_mut() {
                    Some(h) => {
                        *cy += C_HIT;
                        if c as i32 >= *h {
                            *cy += C_HIGHER;
                        }
                        *h = (*h).max(c as i32);
                    }
                    None => *cy += C_HIT_LOW,
                }
                o += 8;
            } else {
                *cy += C_SCAN;
                o += 2;
            }
        }
        Some(())
    };
    let after_end_word = |img: &[u8], from: usize, cy: &mut i64| -> Option<usize> {
        let mut o = from;
        while o + 2 <= img.len() {
            *cy += C_SEARCH;
            if let Some(k) = END_WORDS.iter().position(|&e| e == w(img, o)) {
                *cy += C_SEARCH_END * k as i64;
                return Some(o + 2);
            }
            o += 2;
        }
        None
    };
    let size = if d3 < d5 {
        cy += C_LOW_TRACKS;
        scan(&mut img, lay.fx_end, off(d2), None, &mut cy)?;
        off(d5)
    } else {
        let mut a2 = after_end_word(&img, off(d3), &mut cy)?;
        let mut hi = d3;
        scan(&mut img, off(d5), a2, Some(&mut hi), &mut cy)?;
        if (virt as usize + a2) as i32 <= hi {
            a2 = after_end_word(&img, off(hi), &mut cy)?;
            cy += C_SEARCH_AGAIN;
        }
        a2
    };
    if size > d.len() {
        return None;
    }
    // 6. Mercs: two stop commands, and the last instrument's missing period field
    if size == 0x77AE {
        put32(&mut img, 0x7318, 0x00FE_00FE)?;
        if img.len() < 0x874 {
            return None;
        }
        img.copy_within(0x864..0x870, 0x868);
        put32(&mut img, 0x864, 488)?;
        cy += C_MERCS;
    }
    Some((img, count, cy))
}

// -------------------------------------------------------------------- engine

#[derive(Clone, Copy, Default)]
struct Ctx {
    ptr: u32,
    idx: u16,
    lcnt: u16,
    lret: u16,
    tr: u16,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    per: u16,
    vol: u16,
    len: u16,
    lp: u16,
    smp: u32,
    trig: bool,
    dur: u16,
    prep: u16,
    itr: u16,
    note: u16,
    main: Ctx,
    pat: Ctx,
}

/// What reading a voice's next note ended with.
enum Next {
    Note(u16, u16),
    Restart,
}

pub struct MikeDavies {
    lay: Lay,
    img: Vec<u8>,
    /// Where file offset 0 lies in guest memory.
    base: u32,
    songs: u16,
    sub: u16,
    /// The first tick after a change of subsong.
    next_at: Option<u64>,
    songend: bool,
    playing: bool,
    song: u16,
    v: [Voice; 4],
    tl: [u32; C_COUNT],
    /// The tick's start (colour clocks) and the cycles since.
    t: u64,
    acc: i64,
    /// `acc` at the start of the sequencer's pass.
    pass_acc: i64,
}

impl MikeDavies {
    fn t(&mut self, id: usize) {
        self.tl[id] += 1;
    }

    /// The prepared image's byte at guest address `a`; 0 outside it.
    fn b(&self, a: u32) -> u8 {
        let o = a.wrapping_sub(self.base) as usize;
        self.img.get(o).copied().unwrap_or(0)
    }

    fn w(&self, a: u32) -> u16 {
        (self.b(a) as u16) << 8 | self.b(a.wrapping_add(1)) as u16
    }

    fn l(&self, a: u32) -> u32 {
        (self.w(a) as u32) << 16 | self.w(a.wrapping_add(2)) as u32
    }

    /// Unit `k` of the element at `a`.
    fn unit(&self, a: u32, k: u32) -> u16 {
        if self.lay.bytes {
            self.b(a.wrapping_add(k)) as u16
        } else {
            self.w(a.wrapping_add(2 * k))
        }
    }

    /// An element's size in the word and the byte encoding.
    fn size(&self, word: u16, byte: u16) -> u16 {
        if self.lay.bytes {
            byte
        } else {
            word
        }
    }

    fn stop(&mut self, hw: &mut Hw) {
        self.reset();
        for ch in 0..4 {
            hw.vol(ch, 0);
        }
        hw.dmacon(0x000F);
    }

    /// The stop's state change (specification, 4.1).
    fn reset(&mut self) {
        for v in &mut self.v {
            v.main.ptr = 0xFFFF_FFFF;
            v.trig = false;
            v.main.idx = 0;
            v.main.lcnt = 0;
            v.pat.ptr = 0;
            v.pat.idx = 0;
            v.pat.lcnt = 0;
            v.main.tr = 0;
        }
        self.playing = false;
    }

    /// The start of song `n` after the stop and the ADKCON write: false when there is no such song.
    fn begin(&mut self, n: u16) -> bool {
        self.song = n;
        self.playing = true;
        let e = self.lay.songs + 16 * (n as usize - 1);
        if e >= self.lay.songs_end {
            return false;
        }
        for (k, v) in self.v.iter_mut().enumerate() {
            v.main.ptr = be32(&self.img, e + 4 * k);
            v.dur = 1;
        }
        true
    }

    /// Reads voice `vi`'s next note (specification, 5.1): the note and its duration.
    fn next(&mut self, vi: usize, hw: &mut Hw) -> Next {
        for _ in 0..100_000 {
            let in_pat = self.v[vi].pat.ptr != 0;
            let c = if in_pat { self.v[vi].pat } else { self.v[vi].main };
            let a = c.ptr.wrapping_add(c.idx as i16 as i32 as u32);
            let x = self.unit(a, 0);
            let mut c2 = c;
            if !in_pat && x != CALL {
                self.t(C_MAIN);
            }
            match x {
                0..=0x7F => {
                    if x == 0 {
                        self.t(C_ZERO);
                    }
                    let dur = self.unit(a, 1);
                    c2.idx = c.idx.wrapping_add(self.size(4, 2));
                    self.set_ctx(vi, in_pat, c2);
                    return Next::Note(x, dur);
                }
                END_PATTERN => {
                    self.t(C_END_PATTERN);
                    c2.idx = 0;
                    let v = &mut self.v[vi];
                    v.prep = v.prep.wrapping_sub(1);
                    if v.prep == 0 {
                        c2.ptr = 0;
                        self.t(C_LAST_REPEAT);
                    }
                    self.set_ctx(vi, in_pat, c2);
                }
                CALL => {
                    self.t(C_CALL);
                    let (count, tr, ptr) = (self.w(a.wrapping_add(2)), self.w(a.wrapping_add(4)), self.l(a.wrapping_add(6)));
                    let sz = self.size(10, 5);
                    let v = &mut self.v[vi];
                    v.prep = count;
                    v.pat.tr = tr;
                    v.pat.ptr = ptr;
                    v.main.idx = v.main.idx.wrapping_add(sz);
                }
                LOOP_END => {
                    self.t(C_LOOP_END);
                    c2.lcnt = c.lcnt.wrapping_sub(1);
                    if c2.lcnt != 0 {
                        self.t(C_LOOP_BACK);
                    }
                    c2.idx = if c2.lcnt != 0 { c.lret } else { c.idx.wrapping_add(self.size(2, 1)) };
                    self.set_ctx(vi, in_pat, c2);
                }
                LOOP_START => {
                    self.t(C_LOOP_START);
                    let n = self.unit(a, 1);
                    c2.idx = c.idx.wrapping_add(self.size(4, 2));
                    c2.lret = c2.idx;
                    c2.lcnt = if self.lay.bytes { c.lcnt & 0xFF00 | n } else { n };
                    self.set_ctx(vi, in_pat, c2);
                }
                RESTART | STOP => {
                    self.songend = true;
                    self.end_of_song(x == RESTART, hw);
                    return Next::Restart;
                }
                _ => {
                    self.t(C_INS);
                    let mut r = self.lay.ins + REC * (x & 0x7F) as usize;
                    if r >= self.lay.ins_end {
                        self.t(C_INS_CLAMP);
                        r = self.lay.ins;
                    }
                    let f = |k: usize| be16(&self.img, r + 4 * k + 2);
                    let smp = be32(&self.img, r);
                    let (len, per, vol, itr, lp) = (f(1), f(2), f(3), f(4), f(5));
                    let rest = self.lay.rest;
                    let v = &mut self.v[vi];
                    (v.smp, v.len, v.per, v.vol, v.itr, v.lp) = (smp, len, per, vol, itr, lp);
                    v.trig = true;
                    if rest {
                        v.note = 1;
                    }
                    c2.idx = c.idx.wrapping_add(self.size(2, 1));
                    self.set_ctx(vi, in_pat, c2);
                }
            }
        }
        // a track of commands that never reaches a note: the voice stops reading
        self.v[vi].main.ptr = 0xFFFF_FFFF;
        Next::Note(0, 0)
    }

    fn set_ctx(&mut self, vi: usize, in_pat: bool, c: Ctx) {
        if in_pat {
            self.v[vi].pat = c;
        } else {
            self.v[vi].main = c;
        }
    }

    /// One pass of the sequencer; true when a restart (0xFF or 0xFE) abandoned it.
    fn sequencer(&mut self, hw: &mut Hw) -> bool {
        if !self.playing {
            return false;
        }
        for vi in 0..4 {
            if self.v[vi].main.ptr & 0x8000_0000 != 0 {
                self.t(C_OFF);
                continue;
            }
            self.v[vi].dur = self.v[vi].dur.wrapping_sub(1);
            if self.v[vi].dur != 0 {
                self.t(C_IDLE);
                continue;
            }
            self.t(C_FETCH);
            let (mut x, dur) = match self.next(vi, hw) {
                Next::Restart => return true,
                Next::Note(x, d) => (x, d),
            };
            let v = &self.v[vi];
            let tr = if v.pat.ptr != 0 { v.pat.tr } else { v.main.tr };
            if self.lay.zero_both {
                if x != 0 {
                    x = x.wrapping_add(tr).wrapping_add(v.itr);
                }
            } else {
                if x != 0 {
                    x = x.wrapping_add(tr);
                }
                x = x.wrapping_add(v.itr);
            }
            let per = self.w((self.base + self.lay.periods as u32).wrapping_add(x.wrapping_mul(2) as i16 as i32 as u32));
            let v = &mut self.v[vi];
            v.note = x;
            v.per = per;
            v.dur = dur;
            v.trig = true;
        }
        false
    }

    /// Brings the clock to the original's next write (`acc` cycles into the tick).
    fn at(&self, hw: &mut Hw) {
        let at = self.t + (self.acc.max(0) / 2) as u64;
        hw.bus.clock = hw.bus.clock.max(at);
    }

    /// The beam wait: `WAIT_LINES` line changes seen by the polling reads, the first at `acc`;
    /// `acc` ends at the last read.
    fn beam_wait(&mut self) {
        use crate::paula::LINE;
        let line = |s: &Self, a: i64| (s.t + (a.max(0) / 2) as u64) / LINE;
        let mut a = self.acc;
        for n in 1..=WAIT_LINES {
            let cur = line(self, a);
            a += POLL_SECOND;
            while line(self, a) == cur {
                a += POLL;
            }
            if n < WAIT_LINES {
                a += POLL_NEXT;
            }
        }
        self.acc = a;
    }

    /// A restart (0xFF) or stop (0xFE) element: the stop's volume clears and DMA stop at the
    /// original's cycles, and for a restart its ADKCON write and the song's start.
    fn end_of_song(&mut self, restart: bool, hw: &mut Hw) {
        let (t, l) = (&self.tl, &self.lay);
        let n = |i: usize| t[i] as i64;
        let flag = |b: bool, c: i64| if b { c } else { 0 };
        self.acc = self.pass_acc
            + END_START
            + flag(l.a_build, END_A)
            + flag(l.bytes, END_BYTES)
            + flag(restart, END_RESTART + flag(l.a_build, END_RESTART_A))
            + END_IDLE * n(C_IDLE)
            + (END_IDLE + SEQ_OFF) * n(C_OFF)
            + (END_IDLE + SEQ_FETCH + flag(l.bytes, SEQ_FETCH_BYTES)) * (n(C_FETCH) - 1)
            + SEQ_MAIN * n(C_MAIN)
            + SEQ_ZERO * flag(l.zero_both, 2).max(1) * n(C_ZERO)
            + (SEQ_INS + flag(l.rest, SEQ_INS_REST) + flag(l.bytes, SEQ_INS_BYTES)) * n(C_INS)
            + SEQ_INS_CLAMP * n(C_INS_CLAMP)
            + SEQ_CALL * n(C_CALL)
            + SEQ_END_PATTERN * n(C_END_PATTERN)
            + SEQ_LAST_REPEAT * n(C_LAST_REPEAT)
            + SEQ_LOOP_END * n(C_LOOP_END)
            + SEQ_LOOP_BACK * n(C_LOOP_BACK)
            + SEQ_LOOP_START * n(C_LOOP_START);
        self.reset();
        for ch in 0..4 {
            self.at(hw);
            hw.vol(ch, 0);
            self.acc += END_VOL;
        }
        self.acc += END_DMA - END_VOL;
        self.at(hw);
        hw.dmacon(0x000F);
        if !restart {
            return;
        }
        self.acc += END_ADK + if self.lay.a_build { END_ADK_A } else { 0 };
        self.at(hw);
        hw.bus.write_word(0xDFF09E, 0x00FF);
        if !self.begin(self.song) {
            self.stop(hw);
        }
        self.acc = (self.acc & !1) + END_NEXT;
    }

    /// The sequencer's cycles of this pass, from its path counters.
    fn seq_cost(&self) -> i64 {
        let (t, l) = (&self.tl, &self.lay);
        let n = |i: usize| t[i] as i64;
        let flag = |b: bool, c: i64| if b { c } else { 0 };
        SEQ_START
            + flag(l.dmacon_rec, SEQ_REC)
            + SEQ_OFF * n(C_OFF)
            + (SEQ_FETCH + flag(l.bytes, SEQ_FETCH_BYTES)) * n(C_FETCH)
            + SEQ_MAIN * n(C_MAIN)
            + SEQ_ZERO * flag(l.zero_both, 2).max(1) * n(C_ZERO)
            + (SEQ_INS + flag(l.rest, SEQ_INS_REST) + flag(l.bytes, SEQ_INS_BYTES)) * n(C_INS)
            + SEQ_INS_CLAMP * n(C_INS_CLAMP)
            + SEQ_CALL * n(C_CALL)
            + SEQ_END_PATTERN * n(C_END_PATTERN)
            + SEQ_LAST_REPEAT * n(C_LAST_REPEAT)
            + SEQ_LOOP_END * n(C_LOOP_END)
            + SEQ_LOOP_BACK * n(C_LOOP_BACK)
            + SEQ_LOOP_START * n(C_LOOP_START)
    }

    /// The output routine (specification, 6.2), after the sequencer's work of this pass.
    fn output(&mut self, hw: &mut Hw) {
        self.acc += self.seq_cost();
        self.tl = [0; C_COUNT];
        let a = if self.lay.a_build { OUT_VOL_A } else { 0 };
        let rec = if self.lay.dmacon_rec { OUT_REC } else { 0 };
        let mut wrote = false;
        let mut last = 0;
        for ch in 0..4 {
            if !self.v[ch].trig {
                self.acc += OUT_IDLE;
                continue;
            }
            if self.v[ch].per < self.lay.thr {
                if self.lay.clamp {
                    self.v[ch].per = self.lay.thr;
                    self.acc += OUT_CLAMP;
                } else {
                    self.v[ch].trig = false;
                    self.acc += OUT_IDLE + OUT_LOW;
                    continue;
                }
            }
            let v = self.v[ch];
            if wrote {
                self.acc += OUT_NEXT + OUT_NEXT_CH * last as i64 + rec;
            }
            last = ch;
            self.at(hw);
            hw.dmacon(1 << ch);
            self.acc += OUT_LOC;
            self.at(hw);
            hw.loc(ch, v.smp);
            self.acc += OUT_LEN;
            self.at(hw);
            hw.len(ch, v.len);
            self.acc += OUT_PER;
            self.at(hw);
            hw.per(ch, v.per);
            self.acc += OUT_VOL[ch] + a;
            self.at(hw);
            hw.vol(ch, v.vol & 0x7F);
            wrote = true;
        }
        if !wrote {
            return;
        }
        // the voices after the last one written were counted as not starting: the wait's first read
        // comes OUT_WAIT after the last volume write
        self.acc += OUT_WAIT[last] - OUT_IDLE * (3 - last) as i64;
        self.beam_wait();
        self.acc += ON_START;
        for ch in 0..4 {
            let v = &self.v[ch];
            if !v.trig {
                self.acc += OUT_IDLE;
            } else if self.lay.rest && v.note == 0 {
                self.acc += ON_REST;
            } else {
                self.acc += ON_VOICE + rec + if self.lay.rest { ON_REST_FLAG } else { 0 };
                self.at(hw);
                hw.dmacon(0x8000 | 1 << ch);
            }
        }
        self.acc += ON_END;
        self.beam_wait();
        self.acc += LOOP_FIRST - LOOP_NEXT;
        let mut after_loop = false;
        for ch in 0..4 {
            if !self.v[ch].trig {
                self.acc += OUT_IDLE;
                continue;
            }
            self.v[ch].trig = false;
            let v = self.v[ch];
            self.acc += LOOP_NEXT + if after_loop { LOOP_AFTER_LOOPED } else { 0 };
            after_loop = v.lp != 0;
            if v.lp != 0 {
                self.acc += LOOP_LOOPED;
                self.at(hw);
                hw.loc(ch, v.smp.wrapping_add(v.lp as i16 as i32 as u32));
                self.acc += LOOP_LEN_LOOPED;
                self.at(hw);
                hw.len(ch, (v.len.wrapping_mul(2).wrapping_sub(v.lp)) >> 1);
            } else {
                self.at(hw);
                hw.loc(ch, v.smp);
                self.acc += LOOP_LEN;
                self.at(hw);
                hw.len(ch, 2);
            }
        }
    }

    /// The play routine `passes` times; a restart begins again with the first pass.
    fn play(&mut self, hw: &mut Hw) {
        let mut restarts = 0;
        'tick: loop {
            for _ in 0..self.lay.passes {
                self.pass_acc = self.acc;
                if self.sequencer(hw) {
                    self.tl = [0; C_COUNT];
                    restarts += 1;
                    if restarts > 8 {
                        // a song that restarts without ever reaching a note
                        self.stop(hw);
                    }
                    continue 'tick;
                }
                self.output(hw);
            }
            return;
        }
    }
}

impl Replayer for MikeDavies {
    fn format(&self) -> &'static str {
        "Mike Davies"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.songs, self.sub)
    }

    /// The host's change of subsong: EndSound's DMA stop and volume clears, then the start-up's
    /// calls from the volume clears on (`host_start`); the timer starts again at its end.
    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if !(1..=self.songs).contains(&n) {
            return;
        }
        self.sub = n;
        let b = &mut *hw.bus;
        let c = b.clock;
        b.clock = c + 14;
        b.write_word(0xDFF096, 0x000F);
        for ch in 0..4u32 {
            b.clock = c + 24 + 8 * ch as u64;
            b.write_word(0xDFF0A8 + 16 * ch, 0);
        }
        let end = host_start(b, c + 56, &self.img, &self.lay, n as usize - 1);
        self.next_at = Some(end + DT_TICK);
        self.songend = false;
        self.reset();
        if !self.begin(n) {
            self.reset();
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t = hw.bus.clock;
        self.acc = 0;
        self.tl = [0; C_COUNT];
        self.play(hw);
    }
}

/// The host's start-up before the song's first tick, from its first call (the machine's clock at
/// `new`): the check, InitPlayer (`stretch`), the host's short calls, two calls that clear the
/// volumes (writes 440, 705, 993 and 1271 cycles into each), and InitSound: its volume clears, the
/// DMA stop and the ADKCON write of the song's start (a song's unused voice saves 8 cycles; the A
/// builds take 39 more). Each call's cycles are halved and rounded down on their own.
fn start_up(m: &mut Machine, d: &[u8], lay: &Lay, stretch: i64, song: usize) {
    let half = |c: i64| (c.max(0) / 2) as u64;
    let b = &mut m.bus;
    let mut c = b.clock;
    for cy in [check_cycles(d), 36, stretch, 80, 16, 24, 36] {
        c += half(cy);
    }
    host_start(b, c, d, lay, song);
}

/// The start-up from the host's two volume-clearing calls on, from `c`: those calls and InitSound
/// for song `song` (from 0); returns the clock at InitSound's end.
fn host_start(b: &mut crate::bus::Bus, mut c: u64, d: &[u8], lay: &Lay, song: usize) -> u64 {
    let half = |c: i64| (c.max(0) / 2) as u64;
    for _ in 0..2 {
        for (ch, cy) in [440, 705, 993, 1271].into_iter().enumerate() {
            b.clock = c + half(cy);
            b.write_word(0xDFF0A8 + 16 * ch as u32, 0);
        }
        c += half(1329);
    }
    let e = lay.songs + 16 * song;
    let unused = (0..4).filter(|k| be32(d, e + 4 * k) == 0xFFFF_FFFF).count() as i64;
    let g = if lay.a_build { 1321 } else { 1282 } - 8 * unused;
    for ch in 0..4u32 {
        b.clock = c + half(g + 20 * ch as i64);
        b.write_word(0xDFF0A8 + 16 * ch, 0);
    }
    b.clock = c + half(g + 100);
    b.write_word(0xDFF096, 0x000F);
    b.clock = c + half(g + 216);
    b.write_word(0xDFF09E, 0x00FF);
    b.clock = c + half(g + 671);
    b.clock
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let lay = layout(d)?;
    let songs = (lay.songs_end - lay.songs) / 16;
    if songs == 0 {
        return None;
    }
    let (img, _, prep) = prepare(d, &lay, base)?;
    let stretch = stretch(d, &lay, prep)?;
    start_up(m, d, &lay, stretch, 0);
    let mut s = MikeDavies {
        lay,
        img,
        base,
        songs: songs as u16,
        sub: 1,
        next_at: None,
        songend: false,
        playing: false,
        song: 0,
        v: Default::default(),
        tl: [0; C_COUNT],
        t: 0,
        acc: 0,
        pass_acc: 0,
    };
    s.reset();
    if !s.begin(1) {
        s.reset();
    }
    Some(Box::new(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn p16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn p32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    fn words(d: &mut [u8], o: usize, ws: &[u16]) {
        for (i, &v) in ws.iter().enumerate() {
            p16(d, o + 2 * i, v);
        }
    }

    const G: u32 = 0x2_0000;

    /// A synthetic image: the signposts the scan looks for (an A1-like build unless
    /// `b` says otherwise), one song with voice 0's track `track` at 0x340 and a
    /// pattern `pat` at 0x380, one instrument (period 0x150, volume 40, a 16-byte
    /// sample at 0x400 looping from byte 4), a period table at 0x200 of 0x100 + 2n, but 4 for note 0.
    fn module(track: &[u16], pat: &[u16], bytes: bool, rest: bool, clamp: bool, zero_both: bool) -> Vec<u8> {
        let mut d = vec![0u8; 0x420];
        words(&mut d, 0xE6, &[0x48E7, 0xFFFE, 0x6106, 0x4CDF, 0x7FFF, 0x4E75, 0x0C40, 0x0000, 0x6000, 0x0010]);
        p16(&mut d, 0x108, 0x2D58);
        words(&mut d, 0x100, &[0x33FC, 0x0001]);
        p32(&mut d, 0x104, G);
        let op = |d: &mut Vec<u8>, at: usize, code: u16, off: u32| {
            p16(d, at, code);
            p32(d, at + 2, G + off);
        };
        op(&mut d, 0x10C, 0x4DF9, 0x10);
        op(&mut d, 0x112, 0x41F9, 0x300);
        op(&mut d, 0x118, 0xB1FC, 0x310);
        op(&mut d, 0x11E, 0x41F9, 0x310);
        op(&mut d, 0x124, 0xB1FC, 0x310);
        words(&mut d, 0x12A, &[0x303C, 0x0000]);
        // the play routine: the sequencer at 0x16C, the output at 0x1A0
        words(&mut d, 0x12E, &[0x48E7, 0xFFFE, 0x6100, 0x16C - 0x134, 0x6100, 0x1A0 - 0x138, 0x4CDF, 0x7FFF, 0x4E75]);
        op(&mut d, 0x140, 0x41F9, 0x310);
        op(&mut d, 0x146, 0xB1FC, 0x328);
        op(&mut d, 0x14C, 0x4BF9, 0x200);
        words(&mut d, 0x152, &[0x0C40, 0x0000]);
        p16(&mut d, 0x16A, 0x0095);
        words(&mut d, 0x16C, &[0x4A40, 0x6700, if zero_both { 10 } else { 6 }]);
        words(&mut d, 0x172, &[0xD0ED, 0x001C, if bytes { 0x7000 } else { 0x3010 }]);
        words(&mut d, 0x178, &[0x3D40, 0x000C, 0x3D7C, 0x0001, 0x0008]);
        if rest {
            words(&mut d, 0x182, &[0x3D7C, 0x0001, 0x0034]);
        }
        words(&mut d, 0x18A, &[0x0C6E, 0x007C, 0x0004]);
        p16(&mut d, 0x194, if clamp { 0x33EE } else { 0x426E });
        p16(&mut d, 0x1A0, 0x4E75);
        for n in 1..0x40 {
            p16(&mut d, 0x200 + 2 * n, 0x100 + 2 * n as u16);
        }
        p16(&mut d, 0x200, 4);
        // the song, the instrument, the track, the pattern
        p32(&mut d, 0x300, G + 0x340);
        for k in 1..4 {
            p32(&mut d, 0x300 + 4 * k, 0xFFFF_FFFF);
        }
        let ins = [G + 0x400, 8, 0x150, 40, 0, 4];
        for (k, &v) in ins.iter().enumerate() {
            p32(&mut d, 0x310 + 4 * k, v);
        }
        let put = |d: &mut Vec<u8>, at: usize, ws: &[u16]| {
            let mut o = at;
            let mut i = 0;
            while i < ws.len() {
                if ws[i] == CALL {
                    // call: count, transpose, the pattern's game address
                    let (n, t) = (ws[i + 1], ws[i + 2]);
                    if bytes {
                        d[o] = CALL as u8;
                    } else {
                        p16(d, o, CALL);
                    }
                    p16(d, o + 2, n);
                    p16(d, o + 4, t);
                    p32(d, o + 6, G + 0x380);
                    o += if bytes { 5 } else { 10 };
                    i += 3;
                } else {
                    if bytes {
                        d[o] = ws[i] as u8;
                        o += 1;
                    } else {
                        p16(d, o, ws[i]);
                        o += 2;
                    }
                    i += 1;
                }
            }
        };
        put(&mut d, 0x340, track);
        put(&mut d, 0x380, pat);
        d
    }

    fn a1(track: &[u16], pat: &[u16]) -> Vec<u8> {
        module(track, pat, false, true, true, true)
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, bool, Vec<(u32, u16, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("md.test"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * (960 * ticks + 100)];
        e.render(&mut out);
        let end = e.songend();
        (format, end, e.m.bus.write_log.take().unwrap())
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_and_reads_the_layout() {
        let d = a1(&[0x80, 0x10, 3, 0xFF], &[0x94]);
        assert!(detect(&d));
        let lay = layout(&d).unwrap();
        assert_eq!((lay.g, lay.songs, lay.songs_end, lay.ins, lay.ins_end, lay.periods, lay.call), (G, 0x300, 0x310, 0x310, 0x328, 0x200, 0x95));
        assert!(!lay.bytes && lay.rest && lay.clamp && lay.zero_both && lay.thr == 0x7C && lay.passes == 1);
        let lay = layout(&module(&[0xFF], &[0x94], true, false, false, false)).unwrap();
        assert!(lay.bytes && !lay.rest && !lay.clamp && !lay.zero_both);
        // the branch's target must be the expected instruction
        let mut e = d.clone();
        p16(&mut e, 0x108, 0x2D59);
        assert!(!detect(&e));
        let mut e = d.clone();
        p32(&mut e, 0xEA, 0x6106_4CDE);
        assert!(!detect(&e));
        assert!(!detect(&d[..0x100]));
        assert!(!detect(b"FORM\0\0\0\0EMODEMIC"));
    }

    #[test]
    fn prepares_the_pointers_for_the_image_address() {
        let d = a1(&[0x80, CALL, 2, 1, 0, 0x10, 3, 0xFF], &[0x05, 2, 0x94]);
        let lay = layout(&d).unwrap();
        const AT: u32 = 0xF000;
        let (img, count, _) = prepare(&d, &lay, AT).unwrap();
        assert_eq!(count, 1);
        assert_eq!(be32(&img, 0x310), AT + 0x400);
        assert_eq!(be32(&img, 0x300), AT + 0x340);
        assert_eq!(be32(&img, 0x304), 0xFFFF_FFFF);
        assert_eq!(be32(&img, 0x342 + 6), AT + 0x380);
        // a table one long short of a whole record gets one more
        let mut e = d.clone();
        p32(&mut e, 0x148, G + 0x324);
        let lay = layout(&e).unwrap();
        assert_eq!(prepare(&e, &lay, AT).unwrap().1, 1);
        p32(&mut e, 0x148, G + 0x322);
        let lay = layout(&e).unwrap();
        assert!(prepare(&e, &lay, AT).is_none());
    }

    #[test]
    fn plays_instrument_pattern_calls_and_restarts() {
        // instrument 0, a pattern played twice transposed by 1, a note, the restart
        let d = a1(&[0x80, CALL, 2, 1, 0x10, 3, 0xFF], &[0x05, 2, 0x94]);
        let (format, end, log) = play(d, 9);
        assert_eq!(format, "Mike Davies");
        assert!(end);
        // start-up: the host's two calls clearing the volumes, InitSound's clears, DMACON 000F,
        // ADKCON 00FF
        let audio: Vec<u32> = log.iter().map(|w| w.0).filter(|&a| a == 0xDFF096 || a == 0xDFF09E || (0xDFF0A0..0xDFF0E0).contains(&a)).collect();
        let vols = [0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8];
        assert_eq!(audio[..12], [vols, vols, vols].concat());
        assert_eq!(audio[12..14], [0xDFF096, 0xDFF09E]);
        // the pattern's note 5 transposed by 1, twice, then the track's note, then again after the restart
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per[..4], [0x100 + 2 * 6, 0x100 + 2 * 6, 0x100 + 2 * 0x10, 0x100 + 2 * 6]);
        // the sample, then its loop from byte 4 (6 words)
        let len = reg(&log, 0xDFF0A4);
        assert_eq!(len[..2], [8, 6]);
        let lc: Vec<u16> = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1].wrapping_sub(lc[0]), 4);
        assert_eq!(reg(&log, 0xDFF0A8)[..4], [0, 0, 0, 40]);
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..3], [0x000F, 0x0001, 0x8001]);
    }

    #[test]
    fn low_periods_clamp_or_skip_and_note_zero_rests() {
        // note 0 with the rest flag: written, but its DMA stays off
        let (_, _, log) = play(a1(&[0x80, 0x00, 50, 0xFE], &[0x94]), 3);
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001) && !dma.contains(&0x8001));
        // period 4 below the threshold: clamped to 0x7C ...
        assert_eq!(reg(&log, 0xDFF0A6), [0x7C]);
        // ... or the voice skipped, with no writes at all (the A3/B builds)
        let (_, _, log) = play(module(&[0x80, 0x00, 50, 0xFE], &[0x94], false, false, false, false), 3);
        assert!(reg(&log, 0xDFF0A6).is_empty() && !reg(&log, 0xDFF096).contains(&0x0001));
    }

    #[test]
    fn byte_tracks_and_loops() {
        // a loop of 3 around one note, then the stop
        let d = module(&[0x80, LOOP_START, 3, 0x08, 1, LOOP_END, 0xFE], &[0x94], true, true, true, true);
        let (_, end, log) = play(d, 8);
        assert!(end);
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per, [0x100 + 2 * 8; 3]);
    }
}
