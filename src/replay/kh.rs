//! Kris Hatlelid (`kh.*`), the music driver of Fiendish Freddy's Big Top O' Fun,
//! Wings of Fury, The Cycles, Grand Prix Circuit, Test Drive II, Hardball II,
//! Super C and Teenage Mutant Ninja Turtles.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Kris Hatlelid Eagleplayer and
//! the structure of the corpus files), and this player was written from it alone
//! and checked against those writes.
//!
//! A module is an AmigaDOS load file of three hunks, loaded and relocated as
//! LoadSeg does. A two-file tune holds the music only (a stub returning the songs
//! table); its replay routine is the `songplay` load file beside it, which is only
//! looked at, never run: its code tells the build (raw or table durations, the
//! gen2 note rules, the TMNT DE scaling and DA stop). A one-file tune (Grand Prix
//! Circuit, Test Drive II) carries the routine and one song together. The
//! frequency and duration tables are the same musical values in every build and
//! are written out here.
//!
//! A song is four voice lists of (pattern, transpose) entries; a pattern is byte
//! pairs: notes with a duration index, and commands (next/first list entry,
//! instrument, the CIA timer bytes, volume, legato, voice end). Instruments are
//! records with an IFF 8SVX sample (one octave after another), a vibrato and an
//! arpeggio whose counters live in the record and are shared by the voices.
//!
//! One-shot ends and loops are the Eagleplayer's audio interrupt handler, which
//! does not run when Paula raises a request: it waits for the next raster line
//! (after the host's dispatch) and reads the requests then, serves every channel
//! pending, and acknowledges all four. The engine calls `audio_irq_ch` at request
//! time; this player keeps the deferred moment itself (and a handler due just
//! before a tick is held until the tick has started, as the host's tick masks
//! it), so the engine's dispatch is unchanged for every other player.
//!
//! Decided where the specification is silent: a duration index past the table
//! reads (0, 0); a frequency index past the table takes its nearest entry; a
//! zero octave divisor gives octave quotient 0; a zero period divisor writes the
//! frequency's low word (as a divu overflow); a handler re-entry is taken from the
//! later of the request and 40 colour clocks after the last read; a request whose
//! read falls at or after the next tick's start is read then, before the tick's
//! writes; each subsong's instrument pass runs when it starts, on the records as
//! the earlier subsongs left them.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Kris Hatlelid", detect, new };

/// The frequency table; entry 60 is note 0x1B.
const FREQ: [u32; 132] = [
    437824, 413248, 390056, 368164, 347500, 327996, 309588, 292212, 275812, 260332, 245720, 231928, 218912, 206624,
    195028, 184082, 173750, 163998, 154794, 146106, 137906, 130166, 122860, 115964, 109456, 103312, 97514, 92041,
    86875, 81999, 77397, 73053, 68953, 65083, 61430, 57982, 54728, 51656, 48757, 46020, 43437, 40999, 38698, 36526,
    34476, 32541, 30715, 28991, 27364, 25828, 24378, 23010, 21719, 20500, 19349, 18263, 17238, 16271, 15357, 14496,
    13682, 12914, 12189, 11505, 10859, 10250, 9675, 9132, 8619, 8135, 7679, 7248, 6841, 6457, 6095, 5753, 5430,
    5125, 4837, 4566, 4310, 4068, 3839, 3624, 3420, 3229, 3047, 2876, 2715, 2562, 2419, 2283, 2155, 2034, 1920,
    1812, 1710, 1614, 1524, 1438, 1357, 1281, 1209, 1141, 1077, 1017, 960, 906, 855, 807, 762, 719, 679, 641, 605,
    571, 539, 508, 480, 453, 427, 403, 381, 359, 339, 320, 302, 285, 269, 254, 240, 226,
];

/// The duration table of the table builds: (ticks to the next pair, ticks sounding).
const DUR: [(u8, u8); 22] = [
    (96, 86),
    (48, 43),
    (32, 28),
    (64, 58),
    (24, 21),
    (72, 64),
    (12, 10),
    (84, 75),
    (60, 54),
    (36, 32),
    (6, 5),
    (90, 81),
    (78, 70),
    (66, 59),
    (54, 48),
    (42, 38),
    (30, 27),
    (18, 16),
    (8, 7),
    (16, 14),
    (4, 3),
    (3, 2),
];

/// The Eagleplayer's default timer.
const TIMER0: u16 = 0x3700;
/// The host's exec dispatch before the handler's raster wait, the horizontal
/// position it then waits for, the time before it can be entered again, and the
/// span before a tick in which the tick masks it (it then runs this long after
/// the tick's start).
const T_DISPATCH: u64 = 60;
const T_HPOS: u64 = 22;
const T_REENTER: u64 = 40;
const T_MASK: u64 = 300;
const T_HELD: u64 = 17;
/// Pairs read in one parse before it gives up (a pattern with no note).
const PARSE_GUARD: u32 = 4096;

fn long_at(d: &[u8], o: usize) -> u32 {
    be32(d, o)
}

/// Recognition, as the Eagleplayer's check: a three-hunk load file whose code
/// hunk is a songs-table stub (two-file) or the one-file routine.
fn shape(d: &[u8]) -> Option<bool> {
    if d.len() < 0x48 {
        return None;
    }
    if [0x3F3, 0, 3, 0, 2].iter().enumerate().any(|(i, &v)| long_at(d, 4 * i) != v) {
        return None;
    }
    let s0 = long_at(d, 20) & !0x4000_0000;
    if d[0x18] != 0x40 || long_at(d, 0x1C) != 1 || long_at(d, 0x20) != 0x3E9 || long_at(d, 0x24) != s0 {
        return None;
    }
    if long_at(d, 0x28) == 0x6000_0016 && long_at(d, 0x2C) == 0x0000_ABCD {
        if long_at(d, 0x40) == 0xB07C_0000 {
            return Some(true);
        }
        if long_at(d, 0x40) == 0x41F9_0000 && long_at(d, 0x44) == 0x0000_4E75 {
            return Some(false);
        }
        return None;
    }
    if long_at(d, 0x28) == 0x41F9_0000 && long_at(d, 0x2C) == 0x0000_4E75 {
        return Some(false);
    }
    None
}

fn detect(d: &[u8]) -> bool {
    shape(d).is_some()
}

/// The build, from the code of `songplay` or of a one-file module.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Build {
    gen2: bool,
    raw: bool,
    descale: bool,
    da_stop: bool,
}

/// The word offsets (even) of `pat` in `code`.
fn find<'a>(code: &'a [u8], pat: &[u16]) -> impl Iterator<Item = usize> + 'a {
    let pat = pat.to_vec();
    (0..code.len().saturating_sub(2 * pat.len() - 1))
        .step_by(2)
        .filter(move |&o| pat.iter().enumerate().all(|(i, &w)| be16(code, o + 2 * i) == w))
}

/// The build flags of a code hunk; `None` without the frequency table's shape.
fn scan_build(code: &[u8]) -> Option<Build> {
    find(code, &[0x927C, 0x001B, 0xE549]).chain(find(code, &[0x0441, 0x001B, 0xE549])).next()?;
    let da_stop = find(code, &[0xB27C, 0x00DA]).any(|o| {
        let b = be16(code, o + 4);
        b >> 8 == 0x67 && b & 0xFF != 0 && {
            let t = (o as i64 + 6 + (b as u8 as i8) as i64) as usize;
            be16(code, t) == 0x4EBA
        }
    });
    Some(Build {
        gen2: find(code, &[0x0A42, 0x00FF]).next().is_some(),
        raw: find(code, &[0x4243, 0xE34A]).next().is_none(),
        descale: find(code, &[0xC4FC, 0x0090, 0x84FC, 0x00A0]).next().is_some(),
        da_stop,
    })
}

/// The hunks of a load file; a trailing HUNK_BREAK (some rips end with one, and
/// its zero padding) is left out.
fn parse_hunks(d: &[u8]) -> anyhow::Result<crate::hunk::HunkFile> {
    crate::hunk::parse(d).or_else(|e| {
        let n = d.len() & !3;
        let cut = (n.saturating_sub(16)..n).step_by(4).find(|&o| be32(d, o) == 0x3F5 && d[o + 4..].iter().all(|&b| b == 0));
        match cut {
            Some(o) => crate::hunk::parse(&d[..o]),
            None => Err(e),
        }
    })
}

/// A load file laid out in one block from `base` (each hunk 8-aligned) and
/// relocated: the image and each hunk's offset in it.
fn load(d: &[u8], base: u32) -> Option<(Vec<u8>, Vec<usize>)> {
    let hf = parse_hunks(d).ok()?;
    let mut offs = Vec::new();
    let mut size = 0usize;
    for h in &hf.hunks {
        offs.push(size);
        size = (size + h.data.len() + 7) & !7;
    }
    let mut img = vec![0u8; size.max(8)];
    for (h, &o) in hf.hunks.iter().zip(&offs) {
        img[o..o + h.data.len()].copy_from_slice(&h.data);
    }
    for (i, h) in hf.hunks.iter().enumerate() {
        for &(at, target) in &h.relocs {
            let p = offs[i] + at as usize;
            let t = *offs.get(target as usize)?;
            if p + 4 > offs[i] + h.data.len() {
                return None;
            }
            let v = be32(&img, p).wrapping_add(base + t as u32);
            img[p..p + 4].copy_from_slice(&v.to_be_bytes());
        }
    }
    Some((img, offs))
}

/// The handler's state for a voice.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Irq {
    Idle,
    Triggered,
    Finished,
}

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    list: u32,
    pat: u32,
    lofs: u32,
    dur: u16,
    rel: u16,
    vol: u16,
    irq: Irq,
    legato: u16,
    tie: u16,
    inst: u32,
    vhdr: u32,
    body: u32,
    div: u16,
    note: u16,
    cur: u16,
    base: u16,
    step: u16,
    loop_start: u32,
    loop_len: u16,
}

impl Voice {
    fn new(inst: u32) -> Self {
        Voice {
            active: true,
            list: 0,
            pat: 0,
            lofs: 0,
            dur: 0,
            rel: 0,
            vol: 0x20,
            irq: Irq::Idle,
            legato: 0,
            tie: 0,
            inst,
            vhdr: 0,
            body: 0,
            div: 0,
            note: 0,
            cur: 0,
            base: 0,
            step: 0,
            loop_start: 0,
            loop_len: 0,
        }
    }
}

/// The points of the play routine and the handler whose time the moments of the
/// writes are built from (the costs in `COST`, colour clocks, per channel).
#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
enum Ev {
    /// tick start to the play routine's state switch
    Play,
    /// the first tick's voice set-up, before its DMACON
    Init,
    /// a voice that is off
    Off,
    /// a voice entry with a note sounding
    Sustain,
    /// an arpeggio step
    Arp,
    /// the release point, skipped (legato or tie)
    RelSkip,
    /// the release point, before its DMACON
    Release,
    /// no vibrato
    VibOff,
    /// vibrato counting down
    VibWait,
    /// a vibrato step, before its PER
    VibStep,
    /// an arpeggio period, before its PER
    ArpPer,
    /// a voice entry that parses
    Parse,
    D9,
    Da,
    Db,
    Dc,
    Dd,
    De,
    /// DF, before its VOL
    Df,
    E0,
    Ign,
    /// a note pair, up to the rest/mark/play decision
    Note,
    Rest,
    Mark,
    /// a played note, before its PER
    NotePer,
    /// each octave the sample start steps over
    Oct,
    /// the next writes of a played note
    NoteLc,
    NoteLen,
    NoteVol,
    NoteAdk,
    NoteDma,
    NoteInt,
    /// a release's VOL and PER
    RelVol,
    RelPer,
    /// the voices done, the marks test
    End,
    /// the handler: the read; per channel, not pending, pending with nothing
    /// to do, a one-shot marked (each with its acknowledge), the stop's writes and
    /// acknowledge, the loop's writes and acknowledge; the return to what it
    /// interrupted
    HRead,
    HSkip,
    HIdle,
    HMark,
    HStopVol,
    HStopPer,
    HStopDma,
    HStopAck,
    HLoopLen,
    HLoopLc,
    HLoopInt,
    HLoopAck,
    HExit,
}

const N_MAIN: usize = Ev::HRead as usize;
const N_HND: usize = Ev::HExit as usize + 1 - N_MAIN;

/// The cost of each point of the play routine, per channel, by family of builds
/// (gen1 two-file, one-file, gen2 raw, gen2 table, TMNT), fitted on the
/// reference runs' write times.
const COST: [[[u16; 4]; N_MAIN]; 5] = [
    // gen1 two-file
    [
        [196, 0, 0, 0], // Play
        [294, 0, 0, 0], // Init
        [60, 0, 0, 0], // Off
        [99, 105, 96, 78], // Sustain
        [0, 0, 0, 0], // Arp
        [25, 26, 25, 0], // RelSkip
        [49, 54, 62, 86], // Release
        [25, 30, 29, 0], // VibOff
        [0, 0, 0, 0], // VibWait
        [0, 103, 0, 0], // VibStep
        [0, 0, 0, 0], // ArpPer
        [116, 28, 17, 0], // Parse
        [64, 94, 47, 59], // D9
        [0, 9, 27, 21], // Da
        [130, 114, 157, 223], // Db
        [124, 124, 122, 90], // Dc
        [226, 123, 0, 0], // Dd
        [177, 0, 257, 248], // De
        [276, 172, 313, 308], // Df
        [169, 172, 85, 0], // E0
        [0, 0, 0, 0], // Ign
        [116, 209, 210, 243], // Note
        [0, 0, 0, 0], // Rest
        [0, 0, 9, 0], // Mark
        [222, 227, 238, 243], // NotePer
        [24, 24, 24, 24], // Oct
        [122, 134, 146, 150], // NoteLc
        [110, 122, 134, 138], // NoteLen
        [170, 190, 210, 212], // NoteVol
        [52, 64, 76, 80], // NoteAdk
        [10, 10, 10, 10], // NoteDma
        [14, 14, 14, 14], // NoteInt
        [112, 120, 128, 126], // RelVol
        [56, 68, 80, 84], // RelPer
        [0, 0, 0, 0], // End
    ],
    // one-file
    [
        [199, 0, 0, 0], // Play
        [291, 0, 0, 0], // Init
        [60, 0, 0, 0], // Off
        [57, 133, 96, 78], // Sustain
        [0, 0, 0, 0], // Arp
        [25, 0, 25, 0], // RelSkip
        [88, 25, 59, 78], // Release
        [66, 4, 37, 0], // VibOff
        [0, 0, 0, 0], // VibWait
        [0, 103, 0, 0], // VibStep
        [0, 0, 0, 0], // ArpPer
        [116, 35, 12, 30], // Parse
        [64, 106, 76, 44], // D9
        [0, 9, 27, 21], // Da
        [130, 114, 157, 223], // Db
        [120, 69, 107, 140], // Dc
        [292, 123, 0, 0], // Dd
        [177, 0, 257, 248], // De
        [276, 172, 313, 308], // Df
        [169, 172, 85, 0], // E0
        [0, 0, 0, 0], // Ign
        [116, 217, 229, 216], // Note
        [0, 0, 0, 0], // Rest
        [0, 0, 9, 0], // Mark
        [219, 245, 219, 216], // NotePer
        [24, 24, 24, 24], // Oct
        [122, 134, 146, 150], // NoteLc
        [110, 122, 134, 138], // NoteLen
        [170, 190, 210, 212], // NoteVol
        [52, 64, 76, 80], // NoteAdk
        [10, 10, 10, 10], // NoteDma
        [14, 14, 14, 14], // NoteInt
        [112, 120, 128, 126], // RelVol
        [56, 68, 80, 84], // RelPer
        [0, 0, 0, 0], // End
    ],
    // gen2 raw
    [
        [239, 0, 0, 0], // Play
        [303, 0, 0, 0], // Init
        [60, 0, 0, 0], // Off
        [106, 106, 105, 78], // Sustain
        [0, 0, 0, 0], // Arp
        [25, 26, 25, 0], // RelSkip
        [51, 52, 51, 78], // Release
        [29, 30, 29, 0], // VibOff
        [0, 0, 0, 0], // VibWait
        [0, 103, 0, 0], // VibStep
        [0, 0, 0, 0], // ArpPer
        [123, 157, 12, 168], // Parse
        [71, 82, 81, 67], // D9
        [0, 9, 27, 21], // Da
        [130, 114, 157, 223], // Db
        [132, 165, 123, 132], // Dc
        [226, 123, 0, 0], // Dd
        [240, 0, 257, 248], // De
        [276, 172, 315, 308], // Df
        [169, 172, 85, 0], // E0
        [0, 0, 0, 0], // Ign
        [123, 157, 218, 168], // Note
        [0, 0, 0, 0], // Rest
        [42, 0, 9, 0], // Mark
        [259, 232, 274, 168], // NotePer
        [24, 24, 24, 24], // Oct
        [122, 134, 146, 150], // NoteLc
        [110, 122, 134, 138], // NoteLen
        [170, 190, 210, 212], // NoteVol
        [52, 64, 76, 80], // NoteAdk
        [10, 10, 10, 10], // NoteDma
        [14, 14, 14, 14], // NoteInt
        [112, 120, 128, 126], // RelVol
        [56, 68, 80, 84], // RelPer
        [0, 0, 0, 0], // End
    ],
    // gen2 table
    [
        [248, 0, 0, 0], // Play
        [294, 0, 0, 0], // Init
        [60, 0, 0, 0], // Off
        [99, 107, 100, 73], // Sustain
        [0, 0, 0, 0], // Arp
        [23, 25, 25, 0], // RelSkip
        [49, 51, 56, 73], // Release
        [27, 29, 44, 0], // VibOff
        [0, 0, 0, 0], // VibWait
        [0, 103, 0, 0], // VibStep
        [0, 0, 0, 0], // ArpPer
        [28, 15, 0, 0], // Parse
        [70, 70, 74, 44], // D9
        [0, 9, 27, 21], // Da
        [136, 175, 176, 176], // Db
        [132, 178, 131, 157], // Dc
        [226, 123, 0, 0], // Dd
        [250, 194, 257, 248], // De
        [276, 0, 263, 265], // Df
        [169, 172, 85, 0], // E0
        [0, 0, 0, 0], // Ign
        [229, 262, 240, 229], // Note
        [0, 20, 0, 0], // Rest
        [0, 0, 9, 0], // Mark
        [251, 239, 263, 228], // NotePer
        [24, 24, 24, 24], // Oct
        [122, 134, 146, 150], // NoteLc
        [110, 122, 134, 138], // NoteLen
        [170, 191, 210, 214], // NoteVol
        [52, 64, 76, 80], // NoteAdk
        [10, 10, 10, 10], // NoteDma
        [14, 14, 14, 14], // NoteInt
        [112, 120, 128, 126], // RelVol
        [56, 68, 80, 84], // RelPer
        [0, 0, 0, 0], // End
    ],
    // TMNT
    [
        [354, 0, 0, 0], // Play
        [188, 0, 0, 0], // Init
        [60, 0, 0, 0], // Off
        [16, 107, 105, 78], // Sustain
        [0, 0, 0, 0], // Arp
        [0, 25, 25, 0], // RelSkip
        [26, 51, 51, 78], // Release
        [4, 29, 29, 0], // VibOff
        [0, 0, 0, 0], // VibWait
        [0, 103, 0, 0], // VibStep
        [0, 0, 0, 0], // ArpPer
        [64, 179, 193, 186], // Parse
        [70, 70, 36, 49], // D9
        [0, 9, 27, 21], // Da
        [30, 152, 0, 244], // Db
        [132, 132, 133, 127], // Dc
        [226, 123, 0, 0], // Dd
        [464, 0, 257, 248], // De
        [276, 172, 313, 308], // Df
        [169, 172, 85, 0], // E0
        [0, 0, 0, 0], // Ign
        [117, 166, 167, 165], // Note
        [0, 0, 0, 0], // Rest
        [0, 0, 9, 0], // Mark
        [221, 166, 169, 165], // NotePer
        [24, 24, 24, 24], // Oct
        [122, 134, 146, 150], // NoteLc
        [110, 122, 134, 138], // NoteLen
        [170, 190, 210, 212], // NoteVol
        [52, 64, 76, 80], // NoteAdk
        [10, 10, 10, 10], // NoteDma
        [14, 14, 14, 14], // NoteInt
        [112, 120, 128, 126], // RelVol
        [56, 68, 80, 84], // RelPer
        [0, 0, 0, 0], // End
    ],
];

/// The handler's return to the play routine it interrupted, by family.
const EXIT: [u16; 5] = [71, 54, 57, 115, 76];

/// The same for the handler (one for all builds).
const HCOST: [[u16; 4]; N_HND] = [
    [23, 0, 0, 0],     // HRead
    [34, 34, 34, 34],  // HSkip
    [50, 50, 50, 50],  // HIdle
    [68, 68, 68, 68],  // HMark
    [75, 72, 72, 72],  // HStopVol
    [12, 12, 12, 12],  // HStopPer
    [6, 6, 6, 6],      // HStopDma
    [30, 30, 30, 30],  // HStopAck
    [71, 68, 68, 68],  // HLoopLen
    [22, 22, 22, 22],  // HLoopLc
    [20, 20, 20, 20],  // HLoopInt
    [14, 14, 14, 14],  // HLoopAck
    [0, 0, 0, 0],      // HExit
];

struct Kh {
    title: String,
    /// The loaded module (records are written here), at `base` in guest memory.
    img: Vec<u8>,
    base: u32,
    one_file: bool,
    build: Build,
    songs: Vec<u32>,
    sub: u16,
    started: bool,
    timer: u16,
    state: u8,
    marks: u8,
    /// Voices that have not yet gone back to their list start.
    not_looped: u8,
    v: [Voice; 4],
    songend: bool,
    next_tick: Option<u64>,
    /// When the handler reads the requests, once a request has come.
    due: Option<u64>,
    /// When the handler can be entered again.
    reenter: u64,
    /// The play routine is running (the handler may come between its writes).
    in_tick: bool,
}

/// The loaded image, the build and the songs, for the module at `base`.
fn layout(d: &[u8], base: u32, m: &Machine) -> Option<(Vec<u8>, bool, Build, Vec<u32>)> {
    let one_file = shape(d)?;
    let (img, offs) = load(d, base)?;
    if offs.len() < 2 {
        return None;
    }
    let code_end = offs[1];
    let build = if one_file {
        scan_build(&img[..code_end])?
    } else {
        let sp = crate::deli::read_companion(m, "songplay").1.ok()?;
        let hf = parse_hunks(&sp).ok()?;
        scan_build(&hf.hunks.first()?.data)?
    };
    let songs = if one_file {
        let code = &img[..code_end];
        let o = find(code, &[0xB07C, 0x0001, 0x6700]).next()?;
        let t = usize::try_from(o as i64 + 6 + be16(code, o + 6) as i16 as i64).ok()?;
        let l = find(code.get(t..)?, &[0x41F9]).next()? + t;
        vec![be32(code, l + 2).wrapping_sub(16)]
    } else {
        let at = if be32(&img, 0) == 0x6000_0016 { 0x1A } else { 2 };
        let table = be32(&img, at);
        let rd = |a: u32| a.checked_sub(base).map_or(0, |o| be32(&img, o as usize));
        let first = rd(table);
        let mut v = Vec::new();
        let mut a = table;
        while v.len() < 256 && a != first {
            let s = rd(a);
            if s == 0 {
                break;
            }
            v.push(s);
            a += 4;
        }
        v
    };
    if songs.is_empty() || songs.iter().any(|&s| s.checked_sub(base).is_none_or(|o| o as usize >= img.len())) {
        return None;
    }
    Some((img, one_file, build, songs))
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    // the checks at a trial address first, so that a refused module takes no memory
    layout(d, 0x10_0000, m)?;
    let size = parse_hunks(d).ok()?.hunks.iter().map(|h| (h.data.len() + 7) & !7).sum::<usize>().max(8) as u32;
    let mut base = m.alloc(size);
    if base != 0 && base < 0x1_0000 {
        // a sample address with a zero high word would read as no sample (gen2):
        // the module goes above 64 KiB, as it is in the Amiga's memory
        m.heap.free(base);
        base = m.alloc(size + 0x1_0000).max(0x1_0000);
    }
    if base == 0 {
        return None;
    }
    let (img, one_file, build, songs) = layout(d, base, m)?;
    m.bus.write_bytes(base, &img);
    // the Eagleplayer's handler for the four audio interrupts
    for v in [0x8080, 0x8100, 0x8200, 0x8400] {
        m.bus.write_word(0xDFF09A, v);
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut k = Kh {
        title: file.rsplit(['/', ':']).next().unwrap_or(&file).to_string(),
        img,
        base,
        one_file,
        build,
        songs,
        sub: 0,
        started: false,
        timer: TIMER0,
        state: 0,
        marks: 0,
        not_looped: 0xF,
        v: [Voice::new(0); 4],
        songend: false,
        next_tick: None,
        due: None,
        reenter: 0,
        in_tick: false,
    };
    k.timer = k.initial_timer();
    Some(Box::new(k))
}

impl Kh {
    fn off(&self, a: u32) -> Option<usize> {
        a.checked_sub(self.base).map(|o| o as usize).filter(|&o| o < self.img.len())
    }

    fn r8(&self, a: u32) -> u8 {
        self.off(a).map_or(0, |o| self.img[o])
    }

    fn r16(&self, a: u32) -> u16 {
        self.off(a).map_or(0, |o| be16(&self.img, o))
    }

    fn r32(&self, a: u32) -> u32 {
        self.off(a).map_or(0, |o| be32(&self.img, o))
    }

    fn w16(&mut self, a: u32, v: u16) {
        if let Some(o) = self.off(a).filter(|&o| o + 2 <= self.img.len()) {
            self.img[o..o + 2].copy_from_slice(&v.to_be_bytes());
        }
    }

    fn w32(&mut self, a: u32, v: u32) {
        if let Some(o) = self.off(a).filter(|&o| o + 4 <= self.img.len()) {
            self.img[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
    }

    /// The family of builds whose play routine times are the same.
    fn family(&self) -> usize {
        let b = self.build;
        if self.one_file {
            1
        } else if !b.gen2 {
            0
        } else if b.raw {
            2
        } else if b.descale || b.da_stop {
            4
        } else {
            3
        }
    }

    fn song(&self) -> u32 {
        self.songs[self.sub as usize]
    }

    fn itab(&self) -> u32 {
        self.song().wrapping_add(16)
    }

    /// TIMER after a DE with `x`, by the build's rule.
    fn de(&self, timer: u16, x: u8) -> u16 {
        if !self.build.gen2 {
            return timer & 0xFF00 | x as u16;
        }
        let d = (x ^ 0xFF) as u32;
        let d = if self.build.descale { d * 0x90 / 0xA0 } else { d };
        timer & 0xFF | (((d >> 1) & 0xFF) as u16) << 8
    }

    /// The Eagleplayer's initial speed, from voice 0's first pattern.
    fn initial_timer(&self) -> u16 {
        let mut p = self.r32(self.r32(self.song()));
        if self.r8(p) == 0xDE {
            return self.de(TIMER0, self.r8(p + 1));
        }
        if self.r8(p) == 0xDC {
            p += 2;
        }
        let mut t = TIMER0;
        if self.r8(p) == 0xDD {
            t = (self.r8(p + 1) as u16) << 8;
            if self.r8(p + 2) == 0xDE {
                t = t & 0xFF00 | self.r8(p + 3) as u16;
            }
        }
        t
    }

    /// Searches word by word from `a` for the four characters `id`.
    fn search(&self, a: u32, id: &[u8; 4]) -> Option<u32> {
        let o = self.off(a)?;
        (o..self.img.len().saturating_sub(3)).step_by(2).find(|&i| &self.img[i..i + 4] == id).map(|i| self.base + i as u32)
    }

    /// ReadInstruments: each record's VHDR and BODY data and octave divisor.
    fn read_instruments(&mut self) {
        let it = self.itab();
        let mut k = if self.build.gen2 { 0 } else { 1 };
        while k < 4096 {
            let r = self.r32(it + 4 * k);
            if r == 0 {
                break;
            }
            k += 1;
            let form = self.r32(r + 0x0A);
            if form == 0 {
                continue;
            }
            let Some(vh) = self.search(form, b"VHDR") else { continue };
            let vh = vh + 8;
            self.w32(r, vh);
            let oct = self.r8(vh + 0x0E) as u16;
            self.w16(r + 8, 0x53u16.checked_div(oct).unwrap_or(0));
            if let Some(b) = self.search(vh, b"BODY") {
                self.w32(r + 4, b + 8);
            }
        }
    }

    /// InitSound for the subsong: the instrument pass, PlaySong, the tracks'
    /// presets.
    fn start(&mut self) {
        self.read_instruments();
        self.state = 1;
        self.marks = 0;
        self.not_looped = 0xF;
        let inst = if self.one_file { self.itab() } else { self.r32(self.itab()) };
        self.v = [Voice::new(inst); 4];
    }

    // ---- time and the handler

    /// When the handler reads, for a request at `t`: after the host's dispatch,
    /// at the next raster line's horizontal position 0x16.
    fn read_time(&self, t: u64) -> u64 {
        let entry = t.max(self.reenter);
        ((entry + T_DISPATCH) / LINE + 1) * LINE + T_HPOS
    }

    /// Notes a request that is pending (requested and enabled) with no handler due.
    fn note_request(&mut self, hw: &mut Hw) {
        if self.due.is_none() && hw.bus.paula.pending_audio_irqs() != 0 {
            self.due = Some(self.read_time(hw.bus.clock));
        }
    }

    /// One point of the play routine: its cost passes (the handler comes when it
    /// is due, and the routine goes on after it), and the test runs log it.
    fn ev(&mut self, hw: &mut Hw, e: Ev, ch: usize) {
        let mut left = COST[self.family()][e as usize][ch] as u64;
        for _ in 0..10_000 {
            let now = hw.bus.clock;
            hw.bus.paula.sync(now, &hw.bus.ram);
            self.note_request(hw);
            // the handler is entered at once and waits for its read (the
            // routine it interrupts goes on after it)
            if let Some(r) = self.due.filter(|_| self.in_tick) {
                self.run_to(hw, r);
                self.serve(hw);
                continue;
            }
            let target = now + left;
            let r = hw.bus.paula.sync_to_irq(target, &hw.bus.ram).max(now);
            hw.bus.clock = r;
            left -= r - now;
            if r >= target {
                break;
            }
        }
        #[cfg(test)]
        if let Some(l) = &mut hw.bus.write_log {
            l.push((EV_MARK, (e as u16) << 2 | ch as u16, hw.bus.clock));
        }
    }

    /// A point of the handler: its cost passes (Paula runs on).
    fn hev(&mut self, hw: &mut Hw, e: Ev, ch: usize) {
        hw.bus.clock += HCOST[e as usize - N_MAIN][ch] as u64;
        let now = hw.bus.clock;
        hw.bus.paula.sync(now, &hw.bus.ram);
        #[cfg(test)]
        if let Some(l) = &mut hw.bus.write_log {
            l.push((EV_MARK, (e as u16) << 2 | ch as u16, now));
        }
    }

    /// Lets time run to `t` (Paula with it), outside the play routine.
    fn run_to(&mut self, hw: &mut Hw, t: u64) {
        if t > hw.bus.clock {
            hw.bus.clock = t;
        }
        let now = hw.bus.clock;
        hw.bus.paula.sync(now, &hw.bus.ram);
    }

    /// The handler's read: the channels requested and enabled are served in
    /// order, each channel's request acknowledged after it (served or not).
    fn serve(&mut self, hw: &mut Hw) {
        let read = hw.bus.clock;
        hw.bus.paula.sync(read, &hw.bus.ram);
        let p = hw.bus.paula.pending_audio_irqs();
        self.hev(hw, Ev::HRead, 0);
        for ch in 0..4 {
            if p & 0x80 << ch != 0 {
                self.serve_ch(hw, ch);
            } else {
                self.hev(hw, Ev::HSkip, ch);
            }
            hw.bus.write_word(0xDFF09C, 0x80 << ch);
        }
        self.hev(hw, Ev::HExit, 0);
        hw.bus.clock += EXIT[self.family()] as u64;
        self.reenter = (read + T_REENTER).max(hw.bus.clock);
        self.due = None;
        self.note_request(hw);
    }

    fn serve_ch(&mut self, hw: &mut Hw, ch: usize) {
        let v = self.v[ch];
        match v.irq {
            Irq::Idle => self.hev(hw, Ev::HIdle, ch),
            Irq::Finished => {
                self.v[ch].irq = Irq::Idle;
                self.hev(hw, Ev::HStopVol, ch);
                hw.vol(ch, 0);
                self.hev(hw, Ev::HStopPer, ch);
                hw.per(ch, 0x7C);
                self.hev(hw, Ev::HStopDma, ch);
                hw.dmacon(1 << ch);
                self.hev(hw, Ev::HStopAck, ch);
            }
            Irq::Triggered if v.loop_len != 0 => {
                self.v[ch].irq = Irq::Idle;
                self.hev(hw, Ev::HLoopLen, ch);
                hw.len(ch, v.loop_len);
                self.hev(hw, Ev::HLoopLc, ch);
                hw.loc(ch, v.loop_start);
                self.hev(hw, Ev::HLoopInt, ch);
                hw.bus.write_word(0xDFF09A, 0x80 << ch);
                self.hev(hw, Ev::HLoopAck, ch);
            }
            Irq::Triggered => {
                self.v[ch].irq = Irq::Finished;
                self.hev(hw, Ev::HMark, ch);
            }
        }
    }

    // ---- the tick

    fn vol(&self, hw: &mut Hw, ch: usize, x: u16) {
        hw.vol(ch, (2 * x).min(64) & 0x7F);
    }

    fn signal(&mut self, hw: &mut Hw) {
        if !self.songend {
            self.songend = true;
            #[cfg(test)]
            if let Some(l) = &mut hw.bus.write_log {
                l.push((SONGEND_MARK, 1, hw.bus.clock));
            }
        }
        let _ = hw;
    }

    fn play(&mut self, hw: &mut Hw) {
        if self.v.iter().all(|v| !v.active) {
            self.signal(hw);
        }
        self.ev(hw, Ev::Play, 0);
        match self.state {
            0 => return,
            3 => {
                self.state = 0;
                self.signal(hw);
                hw.dmacon(0x000F);
                return;
            }
            1 => {
                let song = self.song();
                for i in 0..4 {
                    let list = self.r32(song + 4 * i as u32);
                    let pat = self.r32(list);
                    let v = &mut self.v[i];
                    (v.active, v.list, v.pat, v.lofs, v.dur, v.vol, v.irq, v.legato) =
                        (true, list, pat, 0, 0, 0x20, Irq::Idle, 0);
                }
                self.ev(hw, Ev::Init, 0);
                hw.dmacon(0x000F);
                self.state = 2;
            }
            _ => {}
        }
        for ch in 0..4 {
            self.voice(hw, ch);
        }
        self.ev(hw, Ev::End, 0);
        if self.marks == 0 {
            self.state = 0;
            self.signal(hw);
            hw.dmacon(0x000F);
        }
    }

    fn voice(&mut self, hw: &mut Hw, ch: usize) {
        if !self.v[ch].active {
            self.ev(hw, Ev::Off, ch);
            return;
        }
        if self.v[ch].dur == 0 {
            self.ev(hw, Ev::Parse, ch);
            self.parse(hw, ch);
            return;
        }
        self.ev(hw, Ev::Sustain, ch);
        let inst = self.v[ch].inst;
        let arp = self.r16(inst + 0x22) != 0;
        let mut d3 = 0u16;
        if arp {
            let c = self.r16(inst + 0x2C);
            self.w16(inst + 0x2C, c.wrapping_add(1));
            d3 = self.r16(inst + 0x24 + 2 * (c & 3) as u32);
            self.ev(hw, Ev::Arp, ch);
        }
        let v = &mut self.v[ch];
        v.dur = v.dur.wrapping_sub(1);
        if v.dur == v.rel {
            if v.legato == 1 || v.tie != 0 {
                self.ev(hw, Ev::RelSkip, ch);
                return;
            }
            v.irq = Irq::Idle;
            // atomic: the handler does not come between the release's writes
            self.in_tick = false;
            self.ev(hw, Ev::Release, ch);
            hw.dmacon(1 << ch);
            self.ev(hw, Ev::RelVol, ch);
            hw.vol(ch, 0);
            self.ev(hw, Ev::RelPer, ch);
            hw.per(ch, 0x7C);
            self.in_tick = true;
            return;
        }
        if arp {
            let n = v.note.wrapping_add(d3);
            self.ev(hw, Ev::ArpPer, ch);
            self.period(hw, ch, n);
            return;
        }
        self.vibrato(hw, ch);
    }

    fn vibrato(&mut self, hw: &mut Hw, ch: usize) {
        let inst = self.v[ch].inst;
        if self.r16(inst + 0x1E) == 0 {
            self.ev(hw, Ev::VibOff, ch);
            return;
        }
        let c = (self.r16(inst + 0x1C) as i16).wrapping_sub(1);
        self.w16(inst + 0x1C, c as u16);
        if c > 0 {
            self.ev(hw, Ev::VibWait, ch);
            return;
        }
        let reload = self.r16(inst + 0x18);
        self.w16(inst + 0x1C, reload);
        let (up, down) = (self.r16(inst + 0x12), self.r16(inst + 0x14));
        let v = &mut self.v[ch];
        let d = v.cur.wrapping_sub(v.base);
        if (v.step as i16) >= 0 {
            if d > up {
                v.step = v.step.wrapping_neg();
            }
        } else if d <= down {
            v.step = v.step.wrapping_neg();
        }
        v.cur = v.cur.wrapping_add(v.step);
        let cur = v.cur;
        self.ev(hw, Ev::VibStep, ch);
        hw.per(ch, cur);
    }

    /// The octave a note plays in.
    fn octave(&self, ch: usize, n: u16) -> u32 {
        let v = &self.v[ch];
        let q = n.checked_div(v.div).unwrap_or(0) as i32;
        let o = self.r8(v.vhdr + 0x0E) as i32 - q - 1;
        o.max(0) as u32
    }

    /// Writes the period of note `n` and restarts the vibrato.
    fn period(&mut self, hw: &mut Hw, ch: usize, n: u16) {
        let o = self.octave(ch, n);
        let i = (60 + (n.wrapping_sub(0x1B) as i16) as i32).clamp(0, FREQ.len() as i32 - 1);
        let f = FREQ[i as usize];
        let d = (self.r32(self.v[ch].vhdr + 8) << o) as u16;
        let per = match f.checked_div(d as u32) {
            Some(q) if q <= 0xFFFF => q as u16,
            _ => f as u16,
        };
        hw.per(ch, per);
        let inst = self.v[ch].inst;
        let delay = self.r16(inst + 0x1A);
        self.w16(inst + 0x1C, delay);
        let step = self.r16(inst + 0x16);
        let v = &mut self.v[ch];
        (v.base, v.cur, v.step) = (per, per, step);
    }

    fn duration(&mut self, ch: usize, x: u8) {
        let v = &mut self.v[ch];
        if self.build.raw {
            v.dur = (x as u16).wrapping_sub(1);
            v.rel = (x as u16 >> 4) + 1;
        } else {
            let (a, b) = DUR.get(x as usize).copied().unwrap_or((0, 0));
            v.dur = (a as u16).wrapping_sub(1);
            v.rel = v.dur & 0xFF00 | (v.dur.wrapping_sub(b as u16) & 0xFF);
        }
    }

    fn parse(&mut self, hw: &mut Hw, ch: usize) {
        for _ in 0..PARSE_GUARD {
            let p = self.v[ch].pat;
            let (c, x) = (self.r8(p), self.r8(p + 1));
            match c {
                0xD9 => {
                    self.ev(hw, Ev::D9, ch);
                    let v = &mut self.v[ch];
                    v.lofs += 6;
                    let a = v.list + v.lofs;
                    self.v[ch].pat = self.r32(a);
                }
                0xDA => {
                    self.ev(hw, Ev::Da, ch);
                    if self.build.da_stop {
                        if self.state != 0 {
                            hw.dmacon(0x000F);
                            self.state = 3;
                        }
                    } else {
                        self.v[ch].active = false;
                    }
                    return;
                }
                0xDB => {
                    self.ev(hw, Ev::Db, ch);
                    self.v[ch].lofs = 0;
                    self.v[ch].pat = self.r32(self.v[ch].list);
                    self.not_looped &= !(1 << ch);
                    if self.not_looped == 0 {
                        self.signal(hw);
                        self.not_looped = 0xF;
                    }
                }
                0xDC => {
                    self.ev(hw, Ev::Dc, ch);
                    let r = self.r32(self.itab() + 4 * x as u32);
                    let (vh, body, div) = (self.r32(r), self.r32(r + 4), self.r16(r + 8));
                    let v = &mut self.v[ch];
                    (v.inst, v.vhdr, v.body, v.div) = (r, vh, body, div);
                    v.pat += 2;
                }
                0xDD => {
                    self.ev(hw, Ev::Dd, ch);
                    self.timer = self.timer & 0xFF | (x as u16) << 8;
                    self.v[ch].pat += 2;
                }
                0xDE => {
                    self.ev(hw, Ev::De, ch);
                    self.timer = self.de(self.timer, x);
                    self.v[ch].pat += 2;
                }
                0xDF => {
                    self.ev(hw, Ev::Df, ch);
                    self.vol(hw, ch, x as u16);
                    self.v[ch].vol = x as u16;
                    self.v[ch].pat += 2;
                }
                0xE0 => {
                    self.ev(hw, Ev::E0, ch);
                    self.v[ch].legato = x as u16;
                    self.v[ch].pat += 2;
                }
                0xE1.. => {
                    self.ev(hw, Ev::Ign, ch);
                    self.v[ch].pat += 2;
                }
                _ => {
                    self.note(hw, ch, c, x);
                    return;
                }
            }
        }
    }

    fn note(&mut self, hw: &mut Hw, ch: usize, c: u8, x: u8) {
        let gen2 = self.build.gen2;
        let tied = c & 0x80 != 0 || gen2 && x & 0x80 != 0;
        let x = if gen2 { x & 0x7F } else { x };
        let v = &mut self.v[ch];
        if tied {
            if v.tie == 0 {
                v.tie = 2;
            }
        } else {
            v.tie = 0;
        }
        let at = v.list + v.lofs + 4;
        let tr = self.r16(at);
        let note = ((c & 0x7F) as u16).wrapping_add(tr);
        let inst = self.v[ch].inst;
        let played = if gen2 { note.wrapping_add(self.r16(inst + 0x2E)) } else { note };
        self.v[ch].note = note;
        self.v[ch].pat += 2;
        self.duration(ch, x);
        self.marks |= 1 << ch;
        self.ev(hw, Ev::Note, ch);
        if gen2 && c & 0x7F == 0 {
            self.ev(hw, Ev::Rest, ch);
            return;
        }
        let v = self.v[ch];
        if v.vhdr == 0 || gen2 && v.body >> 16 == 0 || v.tie == 1 {
            self.ev(hw, Ev::Mark, ch);
            return;
        }
        if v.tie == 2 {
            self.v[ch].tie = 1;
        }
        let o = self.octave(ch, played);
        self.ev(hw, Ev::NotePer, ch);
        self.period(hw, ch, played);
        let (one, rep) = (self.r32(v.vhdr), self.r32(v.vhdr + 4));
        let start = (0..o).fold(v.body, |s, i| s.wrapping_add(one.wrapping_add(rep) << i));
        for _ in 0..o {
            self.ev(hw, Ev::Oct, ch);
        }
        self.ev(hw, Ev::NoteLc, ch);
        hw.loc(ch, start);
        let vv = &mut self.v[ch];
        vv.irq = Irq::Triggered;
        vv.loop_start = start.wrapping_add(one << o);
        vv.loop_len = ((rep << o) >> 1) as u16;
        let vol = vv.vol;
        self.ev(hw, Ev::NoteLen, ch);
        hw.len(ch, ((one.wrapping_add(rep) << o) >> 1) as u16);
        self.ev(hw, Ev::NoteVol, ch);
        self.vol(hw, ch, vol);
        self.ev(hw, Ev::NoteAdk, ch);
        hw.bus.write_word(0xDFF09E, 0x00FF);
        self.ev(hw, Ev::NoteDma, ch);
        hw.dmacon(0x8000 | 1 << ch);
        self.ev(hw, Ev::NoteInt, ch);
        hw.bus.write_word(0xDFF09A, 0x8000 | 0x80 << ch);
    }
}

/// Log-only entries the test runs mark the song end and the points with.
#[cfg(test)]
const SONGEND_MARK: u32 = 0xFFFFF0;
#[cfg(test)]
const EV_MARK: u32 = 0xFFFF00;

impl Replayer for Kh {
    fn format(&self) -> &'static str {
        "Kris Hatlelid"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        5 * (self.timer as u64 + 1)
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.songs.len() as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.sub = n;
        self.started = false;
        self.songend = false;
        self.due = None;
        self.state = 0;
        for v in &mut self.v {
            v.irq = Irq::Idle;
        }
        self.timer = self.initial_timer();
    }

    fn tick(&mut self, hw: &mut Hw) {
        let t = hw.bus.clock;
        self.next_tick = Some(t + self.tick_clocks());
        // the tick's interrupt has a lower level than the audio one: a handler
        // that the tick held, or that is waiting for its read, runs first
        if let Some(r) = self.due.take() {
            self.run_to(hw, r.max(t + T_HELD));
            self.serve(hw);
        }
        // the host's tick acknowledge, as the reference logs mark a tick (log only)
        if let Some(l) = &mut hw.bus.write_log {
            l.push((0xDFF09C, 0x0008, hw.bus.clock));
        }
        if !self.started {
            self.started = true;
            self.start();
        }
        self.in_tick = true;
        self.play(hw);
        self.in_tick = false;
        if let Some(r) = self.due.take() {
            self.run_to(hw, r);
            self.serve(hw);
        }
    }

    /// A request: the handler reads after the next raster line, or once the
    /// next tick has started when the tick masks it.
    fn audio_irq_ch(&mut self, _ch: usize, hw: &mut Hw) -> bool {
        let now = hw.bus.clock;
        hw.bus.paula.sync(now, &hw.bus.ram);
        for _ in 0..64 {
            self.note_request(hw);
            let Some(r) = self.due else { break };
            if self.next_tick.is_some_and(|t| r + T_MASK > t) {
                break;
            }
            self.due = None;
            self.run_to(hw, r);
            self.serve(hw);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn put32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// A load file of three hunks (code, data in chip memory, a 4-byte BSS);
    /// `relocs` are (hunk, offset, target hunk).
    fn hunk_file(h0: &[u8], h1: &[u8], relocs: &[(usize, u32, u32)]) -> Vec<u8> {
        let mut d = Vec::new();
        let l = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_be_bytes());
        for v in [0x3F3, 0, 3, 0, 2] {
            l(&mut d, v);
        }
        l(&mut d, (h0.len() / 4) as u32);
        l(&mut d, 0x4000_0000 | (h1.len() / 4) as u32);
        l(&mut d, 1);
        for (i, (t, h)) in [(0x3E9u32, h0), (0x3EA, h1)].into_iter().enumerate() {
            l(&mut d, t);
            l(&mut d, (h.len() / 4) as u32);
            d.extend_from_slice(h);
            let rs: Vec<_> = relocs.iter().filter(|r| r.0 == i).collect();
            if !rs.is_empty() {
                l(&mut d, 0x3EC);
                for target in [0u32, 1] {
                    let v: Vec<_> = rs.iter().filter(|r| r.2 == target).collect();
                    if !v.is_empty() {
                        l(&mut d, v.len() as u32);
                        l(&mut d, target);
                        for r in v {
                            l(&mut d, r.1);
                        }
                    }
                }
                l(&mut d, 0);
            }
            l(&mut d, 0x3F2);
        }
        for v in [0x3EB, 1, 0x3F2] {
            l(&mut d, v);
        }
        d
    }

    /// The music hunk: the song at 0 (instrument table at 16: a dummy record,
    /// the sample's record), voice 0's list and pattern (DC 1, `notes`, DA),
    /// voices 1-3 at once DA; the sample's 8SVX FORM (64 one-shot bytes, `rep`
    /// repeat bytes, ctOctave 1, 32 samples per cycle).
    fn music(notes: &[u8], rep: u32) -> (Vec<u8>, Vec<(u32, u32)>) {
        let mut d = vec![0u8; 0x200];
        let mut rel = Vec::new();
        let mut ptr = |d: &mut [u8], at: usize, to: u32| {
            put32(d, at, to);
            rel.push((at as u32, 1));
        };
        ptr(&mut d, 0, 0x40);
        for v in 1..4 {
            ptr(&mut d, 4 * v, 0x48);
        }
        ptr(&mut d, 16, 0x60);
        ptr(&mut d, 20, 0x90);
        ptr(&mut d, 0x40, 0xC0);
        ptr(&mut d, 0x48, 0xD0);
        let mut p = vec![0xDC, 1];
        p.extend_from_slice(notes);
        p.extend_from_slice(&[0xDA, 0]);
        d[0xC0..0xC0 + p.len()].copy_from_slice(&p);
        d[0xD0] = 0xDA;
        ptr(&mut d, 0x90 + 0x0A, 0x100);
        d[0x100..0x104].copy_from_slice(b"FORM");
        d[0x108..0x10C].copy_from_slice(b"8SVX");
        d[0x10C..0x110].copy_from_slice(b"VHDR");
        put32(&mut d, 0x110, 20);
        put32(&mut d, 0x114, 64);
        put32(&mut d, 0x118, rep);
        put32(&mut d, 0x11C, 32);
        d[0x122] = 1;
        d[0x128..0x12C].copy_from_slice(b"BODY");
        put32(&mut d, 0x12C, 64 + rep);
        (d, rel)
    }

    /// A one-file module: the routine's signature words (the one-file entry, the
    /// instrument table's lea, the frequency and duration shapes) and the music.
    fn one_file(notes: &[u8], rep: u32) -> Vec<u8> {
        let mut c = vec![0u8; 0x40];
        put32(&mut c, 0, 0x6000_0016);
        put32(&mut c, 4, 0x0000_ABCD);
        let ws = [0xB07C, 0x0000, 0x4E71, 0xB07C, 0x0001, 0x6700, 0x0002, 0x41F9, 0, 16, 0x927C, 0x001B, 0xE549, 0x4243, 0xE34A];
        for (i, w) in ws.iter().enumerate() {
            put16(&mut c, 0x18 + 2 * i, *w);
        }
        let (m, rel) = music(notes, rep);
        let mut relocs = vec![(0, 0x18 + 16, 1)];
        relocs.extend(rel.iter().map(|&(o, t)| (1, o, t)));
        hunk_file(&c, &m, &relocs)
    }

    fn run(d: Vec<u8>, path: &str, files: &[(&str, Vec<u8>)], chunks: usize) -> (Engine, Vec<(u32, u16, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        for (n, f) in files {
            e.m.os.mem_files.insert(Path::new(n).to_path_buf(), f.clone());
        }
        e.play_data(Path::new(path), d).unwrap();
        let mut out = vec![0f32; 2 * 960];
        for _ in 0..chunks {
            e.render(&mut out);
        }
        let log = e.m.bus.write_log.take().unwrap();
        (e, log)
    }

    /// The register writes of each tick (the log's tick marks cut it).
    fn ticks(log: &[(u32, u16, u64)]) -> Vec<Vec<(u32, u16)>> {
        let mut t: Vec<Vec<(u32, u16)>> = Vec::new();
        for &(a, v, _) in log {
            if (a, v) == (0xDFF09C, 0x0008) {
                t.push(Vec::new());
            } else if let Some(last) = t.last_mut() {
                if a >> 12 == 0xDFF {
                    last.push((a, v));
                }
            }
        }
        t
    }

    #[test]
    fn recognises_the_three_shapes() {
        let d = one_file(&[0x30, 10], 0);
        assert_eq!(shape(&d), Some(true));
        let mut p = d.clone();
        put32(&mut p, 0x28, 0x41F9_0000);
        put32(&mut p, 0x2C, 0x0000_4E75);
        assert_eq!(shape(&p), Some(false));
        let mut b = d.clone();
        put32(&mut b, 0x40, 0x41F9_0000);
        put32(&mut b, 0x44, 0x0000_4E75);
        assert_eq!(shape(&b), Some(false));
        // another code shape, hunk 1 without the memory bit, two hunks
        let mut x = d.clone();
        put32(&mut x, 0x40, 0x4E75_4E75);
        assert_eq!(shape(&x), None);
        let mut x = d.clone();
        x[0x18] = 0;
        assert_eq!(shape(&x), None);
        let mut x = d;
        put32(&mut x, 8, 2);
        assert_eq!(shape(&x), None);
    }

    #[test]
    fn the_build_comes_from_the_code() {
        let code = |ws: &[u16]| ws.iter().flat_map(|w| w.to_be_bytes()).collect::<Vec<u8>>();
        assert_eq!(scan_build(&code(&[0x4E71, 0x4E75])), None);
        assert_eq!(scan_build(&code(&[0x927C, 0x001B, 0xE549, 0x4243, 0xE34A])), Some(Build::default()));
        let b = scan_build(&code(&[0x0441, 0x001B, 0xE549, 0x0A42, 0x00FF])).unwrap();
        assert!(b.gen2 && b.raw && !b.descale && !b.da_stop);
        let b = scan_build(&code(&[
            0x927C, 0x001B, 0xE549, 0x4243, 0xE34A, 0x0A42, 0x00FF, 0xC4FC, 0x0090, 0x84FC, 0x00A0, 0xB27C, 0x00DA, 0x6702, 0x4E71,
            0x4EBA, 0x0000,
        ]))
        .unwrap();
        assert!(b.gen2 && !b.raw && b.descale && b.da_stop);
    }

    #[test]
    fn plays_a_note_releases_it_and_ends() {
        // note 0x30, duration index 10 (6/5): released 5 ticks after its own
        // tick, the next pair (DA) 6 ticks after
        let (e, log) = run(one_file(&[0x30, 10], 0), "/kh-test/kh.one", &[], 12);
        assert_eq!(e.playing.as_ref().unwrap().format, "Kris Hatlelid");
        let t = ticks(&log);
        let main: Vec<_> = t[0].iter().filter(|(a, _)| *a != 0xDFF09C).copied().collect();
        assert_eq!(main[0], (0xDFF096, 0x000F));
        let regs: Vec<u32> = main[1..10].iter().map(|w| w.0).collect();
        assert_eq!(regs, [0xDFF0A6, 0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF0A8, 0xDFF09E, 0xDFF096, 0xDFF09A, 0xDFF0A8]);
        // 4068 / 32 = 127; 64 bytes; volume 0x20 doubled
        assert_eq!(main[1].1, 127);
        assert_eq!(main[4].1, 32);
        assert_eq!((main[5].1, main[6].1, main[7].1, main[8].1), (0x40, 0x00FF, 0x8001, 0x8080));
        // the one-shot: marked by the handler, then stopped at its end
        let all: Vec<_> = t.iter().flatten().copied().collect();
        assert!(all.windows(3).any(|w| w == [(0xDFF0A8, 0), (0xDFF0A6, 0x7C), (0xDFF096, 1)]));
        // the release in tick 5
        assert!(t[5].windows(3).any(|w| w == [(0xDFF096, 1), (0xDFF0A8, 0), (0xDFF0A6, 0x7C)]), "{:x?}", t[5]);
        // DA in tick 6: the song ends at the start of tick 7 (the 8th)
        let end = log.iter().position(|w| w.0 == SONGEND_MARK).unwrap();
        assert_eq!(log[..end].iter().filter(|w| (w.0, w.1) == (0xDFF09C, 0x0008)).count(), 8);
        assert!(e.songend());
    }

    #[test]
    fn a_looped_sample_gets_its_loop_from_the_handler() {
        let (_, log) = run(one_file(&[0x30, 0], 32), "/kh-test/kh.loop", &[], 3);
        let w: Vec<_> = ticks(&log).into_iter().flatten().collect();
        let i = w.iter().position(|x| *x == (0xDFF09A, 0x8080)).unwrap();
        // LEN (32 repeat bytes: 16 words), LC past the one-shot part, the loop
        // fix's INTENA, then the acknowledges
        let j = i + w[i..].iter().position(|x| x.0 == 0xDFF0A4).unwrap();
        assert_eq!(w[j].1, 16);
        assert_eq!(w[j + 1].0, 0xDFF0A0);
        assert_eq!(w[j + 3], (0xDFF09A, 0x0080));
        assert_eq!(&w[j + 4..j + 8], &[(0xDFF09C, 0x80), (0xDFF09C, 0x100), (0xDFF09C, 0x200), (0xDFF09C, 0x400)]);
        let la = (w[j + 1].1 as u32) << 16 | w[j + 2].1 as u32;
        let i0 = w.iter().position(|x| x.0 == 0xDFF0A0).unwrap();
        assert_eq!(la, ((w[i0].1 as u32) << 16 | w[i0 + 1].1 as u32) + 64);
    }

    #[test]
    fn durations_and_timer() {
        let mut k = Kh {
            title: String::new(),
            img: vec![0; 8],
            base: 0x10000,
            one_file: true,
            build: Build::default(),
            songs: vec![0x10000],
            sub: 0,
            started: false,
            timer: TIMER0,
            state: 0,
            marks: 0,
            not_looped: 0xF,
            v: [Voice::new(0); 4],
            songend: false,
            next_tick: None,
            due: None,
            reenter: 0,
            in_tick: false,
        };
        k.duration(0, 0);
        assert_eq!((k.v[0].dur, k.v[0].rel), (95, 9));
        k.build.raw = true;
        k.duration(0, 40);
        assert_eq!((k.v[0].dur, k.v[0].rel), (39, 3));
        assert_eq!(k.de(0x3700, 0x12), 0x3712);
        k.build.gen2 = true;
        assert_eq!(k.de(0x3700, 0x12), 0x7600);
        k.build.descale = true;
        assert_eq!(k.de(0x3700, 0x12), 0x6A00);
        assert_eq!(k.tick_clocks(), 5 * (0x3700 + 1));
    }

    #[test]
    fn a_two_file_tune_needs_its_songplay() {
        // the plain stub returning the songs table
        let mut h0 = vec![0u8; 8];
        put16(&mut h0, 0, 0x41F9);
        put16(&mut h0, 6, 0x4E75);
        let (m0, rel) = music(&[0x30, 10], 0);
        // the songs table at 0 (both songs are the one song that follows it)
        let mut m = vec![0u8; 16];
        m.extend_from_slice(&m0);
        put32(&mut m, 0, 16);
        put32(&mut m, 4, 16);
        let mut relocs = vec![(0, 2, 1), (1, 0, 1), (1, 4, 1)];
        for &(o, t) in &rel {
            let at = o as usize + 16;
            let v = u32::from_be_bytes(m[at..at + 4].try_into().unwrap()) + 16;
            put32(&mut m, at, v);
            relocs.push((1, o + 16, t));
        }
        let d = hunk_file(&h0, &m, &relocs);
        assert_eq!(shape(&d), Some(false));
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        let r = e.play_data(Path::new("/kh-test2/kh.two"), d.clone());
        assert!(r.is_err() || e.playing.as_ref().unwrap().format != "Kris Hatlelid");
        // with a songplay beside it (its code: the dispatch and the build's shapes)
        let ws: [u16; 8] = [0xB07C, 0x0000, 0x927C, 0x001B, 0xE549, 0x4243, 0xE34A, 0x4E75];
        let sp_code: Vec<u8> = ws.iter().flat_map(|w| w.to_be_bytes()).collect();
        let sp = hunk_file(&sp_code, &[0; 4], &[]);
        let (e, log) = run(d, "/kh-test3/kh.two", &[("/kh-test3/songplay", sp)], 3);
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Kris Hatlelid");
        assert_eq!((p.sub_min, p.sub_max), (0, 1));
        assert!(ticks(&log)[0].contains(&(0xDFF0A6, 127)));
    }
}
