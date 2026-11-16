//! Native ProTracker/NoiseTracker/SoundTracker replayer, standing in for
//! DeliTracker's internal player.
//!
//! A translation of ProTracker 2.3D's replay routine as 8bitbubsy's pt2-clone
//! (`pt2_replayer.c`, its C port of the 2.3D replayer) and his ProTracker 2.3F
//! re-source of 2.3D (`PT2.3F_replay_cia.s`) give it, both Copyright (c)
//! 2010-2026 / 2020-2025 Olav Sørensen, BSD 3-Clause licence: see
//! LICENSE-pt2-clone. The period table (all 16 finetunes, and what PT reads past
//! its end) and the vibrato and funk tables are pt2-clone's.
//!
//! It drives the emulated Paula as the 68k routine does, write for write and in
//! its order: on a row each voice's period (when the last note long was empty),
//! DMA off, length, pointer and period of a new note, the period or volume of
//! its effect, then its volume; after the four voices, PT's DMA wait, DMA on for
//! the new notes, the wait again and the repeat pointer and length of all four
//! voices, last voice first. Between ticks only the effects' writes and each
//! voice's volume. The writes are spaced as the 68000 takes to get from one to
//! the next (cycle counts of the 2.3 code paths, roughly), and the two DMA waits
//! are 2.3D's: a DBRA loop of 300 (its DMAWait), 1515 colour clocks on a 7 MHz
//! 68000. Its quirks are kept: the first row comes on the sixth interrupt, a new
//! tempo on the next, 9xx is added twice, B and D on one row, E6x and EEx on
//! delayed rows, the tremolo ramp's vibrato position, portamentos' 12-bit wrap,
//! arpeggios read past the table, EFx inverts the sample in memory. E8x does
//! nothing (as in pt2-clone by default and 2.3F).
//!
//! A ProWizard conversion is played as its original, UADE's PTK-Prowiz, writes it
//! (`Build::Ptk`): the period of a voice without an effect in every effects tick too,
//! the repeats first voice first, the DMA waits on the beam (to 110 colour clocks
//! into the tenth raster line on, then 100 into the tenth after that), and every
//! write at the interrupt's start plus half the 68000 cycles of the code paths taken
//! (`PTK_COSTS`), fitted to the reference's write logs (black box; see README).

use crate::bus::Bus;
use crate::paula::LINE;
use m68k::AddressBus;
use std::collections::HashSet;

/// Periods of the 36 notes (and a 0) for finetunes 0..7, -8..-1, 37 each.
#[rustfmt::skip]
pub(crate) const PERIODS: [u16; 607] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 0,
    850, 802, 757, 715, 674, 637, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318, 300, 284, 268, 253, 239, 225, 213, 201, 189, 179, 169, 159, 150, 142, 134, 126, 119, 113, 0,
    844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316, 298, 282, 266, 251, 237, 224, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 112, 0,
    838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118, 111, 0,
    832, 785, 741, 699, 660, 623, 588, 555, 524, 495, 467, 441, 416, 392, 370, 350, 330, 312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 124, 117, 110, 0,
    826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116, 109, 0,
    820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307, 290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115, 109, 0,
    814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305, 288, 272, 256, 242, 228, 216, 204, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114, 108, 0,
    907, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 0,
    900, 850, 802, 757, 715, 675, 636, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318, 300, 284, 268, 253, 238, 225, 212, 200, 189, 179, 169, 159, 150, 142, 134, 126, 119, 0,
    894, 844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316, 298, 282, 266, 251, 237, 223, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 0,
    887, 838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118, 0,
    881, 832, 785, 741, 699, 660, 623, 588, 555, 524, 494, 467, 441, 416, 392, 370, 350, 330, 312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 123, 117, 0,
    875, 826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116, 0,
    868, 820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307, 290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115, 0,
    862, 814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305, 288, 272, 256, 242, 228, 216, 203, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114, 0,
    // read past the end by arpeggios on finetune -1 (what follows the table in PT)
    774, 1800, 2314, 3087, 4113, 4627, 5400, 6426, 6940, 7713, 8739, 9253, 24625, 12851, 13365,
];

pub(crate) const VIBRATO: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24,
];

pub(crate) const FUNK: [u8; 16] = [0, 5, 6, 7, 8, 10, 11, 13, 16, 19, 22, 26, 32, 43, 64, 128];

/// PT 2.3D's DMA wait (MOVE.W DMAWait,D0 and DBRA D0 300 times), colour clocks.
pub const DMA_WAIT: u64 = 1515;

// Colour clocks the 68000 takes between writes (half its cycles), roughly, for
// the paths of the 2.3 replay code.
/// A voice's dispatch in an effects tick: LEAs, BSRs, the funk check, the
/// command test, the jump table.
const C_FX_CHAN: u64 = 111;
/// A voice with no effect in an effects tick, to its volume write.
const C_FX_NONE: u64 = 102;
/// From an effect's last write back to the volume write.
const C_FX_VOL: u64 = 22;
/// A row's voice: reading the note long and testing the previous one.
const C_ROW_CHAN: u64 = 60;
/// Setting up a voice from its instrument.
const C_ROW_INSTR: u64 = 100;
/// The note tests before the period lookup.
const C_ROW_TESTS: u64 = 30;
/// A step of a period table scan (CMP.W (A0)+, BHS, DBRA).
const C_SCAN: u64 = 13;
/// A write of a register from the voice's variables.
const C_WRITE: u64 = 10;
/// The second word of a MOVE.L to a register pair.
const C_LONG: u64 = 2;
/// Effect computations before their write.
const C_PORTA: u64 = 45;
const C_TONE: u64 = 40;
const C_VIB: u64 = 75;
const C_TREM: u64 = 85;
const C_SLIDE: u64 = 20;
const C_ECMD: u64 = 25;
/// The command dispatch of CheckMoreEffects.
const C_MORE: u64 = 32;
/// The voice loop after SetDMA's second wait.
const C_LOOPS: u64 = 10;

/// PTK-Prowiz's DMA waits are on the beam: on to the tenth start of a raster line, then
/// this many colour clocks into it (to the DMA on, and to the first repeat pointer).
const PTK_WAIT_LINES: u64 = 10;
const PTK_DMA_PHASE: u64 = 110;
const PTK_LOOP_PHASE: u64 = 100;

/// The code paths of PTK-Prowiz's replay whose 68000 cycles place its writes: indices into
/// `PTK_COSTS`.
pub(crate) mod ptk {
    pub const FXT: usize = 0;
    pub const ROWT: usize = 1;
    pub const PDEL: usize = 2;
    pub const FXV1: usize = 3; // ..5
    pub const ROWV1: usize = 6; // ..8
    pub const ROW_END: usize = 9;
    pub const WAIT2: usize = 10;
    pub const LOOP_NEXT: usize = 11;
    pub const LOOP_LEN: usize = 12;
    pub const ROW_PERNOP: usize = 13;
    pub const ROW_READ: usize = 14;
    pub const INS: usize = 15;
    pub const INS_REP: usize = 16;
    pub const NONOTE: usize = 17;
    pub const ROW_E5: usize = 18;
    pub const ROW_TONE: usize = 19;
    pub const ROW_9: usize = 20;
    pub const ROW_NOTE: usize = 21;
    pub const SCAN: usize = 22;
    pub const SETPER: usize = 23;
    pub const SP_ED: usize = 24;
    pub const SP_DMA: usize = 25;
    pub const SP_LEN: usize = 26;
    pub const SP_LC: usize = 27;
    pub const SP_NULL: usize = 28;
    pub const SP_PER: usize = 29;
    pub const CME: usize = 30;
    pub const CME_9: usize = 31;
    pub const CME_B: usize = 32;
    pub const CME_C: usize = 33;
    pub const CME_D: usize = 34;
    pub const CME_E: usize = 35;
    pub const CME_F0: usize = 36;
    pub const CME_FT: usize = 37;
    pub const CME_F: usize = 38;
    pub const CME_PN: usize = 39;
    pub const CME_PN_VOL: usize = 40;
    pub const NONE: usize = 41;
    pub const NONE_VOL: usize = 42;
    pub const DISP: usize = 43;
    pub const ARP_VOL: usize = 44;
    pub const PORTA_VOL: usize = 45;
    pub const TP_SET: usize = 46;
    pub const VIB_LO: usize = 47;
    pub const VIB_HI: usize = 48;
    pub const FX5: usize = 49;
    pub const FX6: usize = 50;
    pub const FX_E: usize = 51;
    pub const FX_E_VOL: usize = 52;
    pub const OTHER: usize = 53;
    pub const OTHER_VOL: usize = 54;
    pub const FUNK: usize = 55;
    pub const ARP0: usize = 56;
    pub const ARP1: usize = 57;
    pub const ARP2: usize = 58;
    pub const ARP_W: usize = 59;
    pub const PORTA: usize = 60;
    pub const PORTA_CLAMP: usize = 61;
    pub const TP_DONE: usize = 62;
    pub const TP_UP: usize = 63;
    pub const TP_DOWN: usize = 64;
    pub const TP_REACH: usize = 65;
    pub const GLISS: usize = 66;
    pub const TP_VOL: usize = 67;
    pub const VIB: usize = 68; // ..71 by waveform
    pub const VIB_NEG: usize = 72;
    pub const VIB_VOL: usize = 73;
    pub const TREM: usize = 74; // ..77 by waveform
    pub const TREM_NEG: usize = 78;
    pub const TREM_CLAMP: usize = 79;
    pub const SLIDE_UP: usize = 80;
    pub const SLIDE_DOWN: usize = 81;
    pub const SLIDE_CLAMP: usize = 82;
    pub const E_ROW: usize = 83; // ..98 by E command
    pub const E_FX: usize = 99; // ..114
    pub const RT_DMA: usize = 115;
    pub const RT_LC: usize = 116;
    pub const RT_LEN: usize = 117;
    pub const RT_PER: usize = 118;
    pub const RT_END: usize = 119;
    pub const RT_BACK: usize = 120;
    pub const VIB_POS: usize = 121;
    pub const TREM_POS: usize = 122;
    pub const INS_NOREP: usize = 123;
    pub const SLIDE_NOCLAMP: usize = 124;
    pub const ARP_SCAN: usize = 125;
    pub const TONE_SCAN: usize = 126;
    pub const GLISS_SCAN: usize = 127;
    pub const TONE_FT: usize = 128;
    pub const TONE_EQ: usize = 129;
    pub const TONE_GT: usize = 130;
    pub const TONE_LT: usize = 131;
    pub const PORTA_DN: usize = 132;
    pub const FX6_LO: usize = 133;
    pub const FX6_HI: usize = 134;
    pub const FX5_LO: usize = 135;
    pub const FX5_HI: usize = 136;
    pub const N: usize = 137;
}

/// 68000 cycles of each of the `ptk` code paths, fitted exactly (integer program) to the
/// reference write logs of the refcmp ChipTracker, ProWizard, Promizer and ProRunner rows
/// (the times after a DMA wait counted from the reference's own DMA write). Only the
/// sums along a path are determined; the terms no measured module takes (the vibrato
/// and tremolo waveforms 1-3, tremolo, retrigger, most E commands, pattern delay) keep
/// estimates.
#[rustfmt::skip]
const PTK_COSTS: [u32; ptk::N] = [
    // FXT ROWT PDEL FXV1-3 ROWV1-3 ROW_END WAIT2 LOOP_NEXT LOOP_LEN
    562, 958, 0, 273, 273, 265, 63, 63, 63, 208, 20, 28, 20,
    // ROW_PERNOP ROW_READ INS INS_REP NONOTE ROW_E5 ROW_TONE ROW_9 ROW_NOTE SCAN SETPER
    20, 0, 477, 46, 176, 100, 552, 0, 37, 41, 0,
    // SP_ED SP_DMA SP_LEN SP_LC SP_NULL SP_PER
    20, 524, 127, 24, 20, 24,
    // CME CME_9 CME_B CME_C CME_D CME_E CME_F0 CME_FT CME_F CME_PN CME_PN_VOL
    335, 71, 0, 78, 100, 88, 14, 497, 102, 9, 44,
    // NONE NONE_VOL DISP ARP_VOL PORTA_VOL TP_SET VIB_LO VIB_HI FX5 FX6 FX_E FX_E_VOL OTHER OTHER_VOL FUNK
    0, 73, 116, 73, 73, 28, 82, 12, 0, 0, 0, 142, 0, 111, 40,
    // ARP0 ARP1 ARP2 ARP_W PORTA PORTA_CLAMP TP_DONE TP_UP TP_DOWN TP_REACH GLISS TP_VOL
    85, 0, 1, 215, 20, 40, 19, 75, 85, 46, 40, 73,
    // VIB0-3 VIB_NEG VIB_VOL TREM0-3 TREM_NEG TREM_CLAMP SLIDE_UP SLIDE_DOWN SLIDE_CLAMP
    187, 300, 300, 300, 0, 118, 300, 300, 300, 300, 10, 10, 236, 244, 16,
    // E_ROW 0-F
    12, 100, 100, 0, 100, 100, 100, 100, 100, 100, 85, 69, 100, 100, 100, 100,
    // E_FX 0-F
    43, 100, 0, 31, 100, 100, 100, 100, 100, 100, 0, 0, 100, 100, 100, 100,
    // RT_DMA RT_LC RT_LEN RT_PER RT_END RT_BACK
    40, 20, 20, 20, 200, 40,
    // VIB_POS TREM_POS INS_NOREP SLIDE_NOCLAMP ARP_SCAN TONE_SCAN GLISS_SCAN
    10, 0, 0, 0, 49, 52, 26,
    // TONE_FT TONE_EQ TONE_GT TONE_LT PORTA_DN
    17, 7, 16, 0, 39,
    // FX6_LO FX6_HI FX5_LO FX5_HI
    0, 0, 0, 0,
];

#[derive(Clone, Copy, Default)]
struct Sample {
    start: u32, // address in guest memory
    length: u16, // words
    finetune: u8,
    volume: u8,
    repeat: u16, // words
    replen: u16, // words
}

/// A voice's variables, PT's n_* (n_note and n_cmd are the pattern long).
#[derive(Clone, Copy, Default)]
struct Chan {
    note: u16,
    cmd: u16,
    start: u32,
    loopstart: u32,
    wavestart: u32,
    length: u16,
    replen: u16,
    period: u16,
    wantedperiod: u16,
    finetune: u8,
    volume: u8,
    toneportdirec: bool,
    toneportspeed: u8,
    vibratocmd: u8,
    vibratopos: u8,
    tremolocmd: u8,
    tremolopos: u8,
    wavecontrol: u8,
    glissfunk: u8,
    sampleoffset: u8,
    pattpos: u8,
    loopcount: u8,
    funkoffset: u8,
}

impl Chan {
    fn cmdlo(&self) -> u8 {
        self.cmd as u8
    }
    fn peroffset(&self) -> usize {
        (self.finetune & 0xF) as usize * 37
    }
}

pub struct ProTracker {
    pub title: String,
    samples: [Sample; 32],
    orders: [u8; 128],
    song_len: usize,
    /// Pattern data, a long per voice and row.
    patterns: Vec<u32>,
    ch: [Chan; 4],
    speed: u8,
    counter: u8,
    pub bpm: u32,
    song_pos: u8,
    row: u8,
    pbreak_pos: u8,
    pbreak_flag: bool,
    pos_jump: bool,
    patt_del_time: u8,
    patt_del_time2: u8,
    low_mask: u8,
    dmacon_temp: u16,
    visited: HashSet<(u8, u8)>,
    pub songend: bool,
    /// The LED filter (E0x): off at the start, as the engine's 68k hosting and UADE leave it for
    /// a player that never touches it (an Amiga itself powers on with the filter on).
    pub filter: bool,
    /// Whether F00 ends the song (ProTracker 2.3D's). Off for the ProWizard conversions, whose
    /// original (UADE's PTK-Prowiz, a ProTracker 3.0b replay) plays on through the pattern and
    /// ends the song at the wrap (The Player 4 modules `p4x.Col30.mod`, `p4x.ingame_7`).
    pub f00_ends: bool,
    /// The first position of each subsong (see `subsong_starts`).
    starts: Vec<u8>,
    /// The subsong playing, an index into `starts`.
    sub: usize,
    /// Whose replay code the writes are timed as.
    pub build: Build,
    /// The PTK build's clock in 68000 cycles (twice the colour clock).
    cyc: u64,
}

/// The build a module converted from the packed format `packed` (ProWizard's name) plays
/// as: the conversions are PTK-Prowiz's, a plain module ProTracker 2.3D's.
pub fn build_for(packed: Option<&str>) -> Build {
    match packed {
        Some("Mosh Player" | "Titanics Player") | None => Build::Pt23d,
        Some(_) => Build::Ptk,
    }
}

/// Whose replay code the writes of a module are timed as.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Build {
    /// ProTracker 2.3D's (the port's original for plain modules).
    #[default]
    Pt23d,
    /// UADE's PTK-Prowiz ("Protracker and family"), which plays the ProWizard conversions.
    Ptk,
}

/// Effect B in the pattern long `l`, and its parameter.
fn pos_jump_of(l: u32) -> Option<u8> {
    ((l >> 8) & 0xF == 0xB).then_some(l as u8)
}

/// The first position of each subsong, as UADE's PTK-Prowiz ("Protracker and family") counts them
/// (ProTracker 2.3D itself has none): measured black box, on synthetic modules and the subsong
/// counts uade123 reports for the corpus's ProTracker modules (7,741 of 7,750 the same; the other
/// 9 have 1 or 2 more there, from forward jumps it counts in a way not pinned down). A position of the song whose
/// pattern holds a position jump (Bxx, any row, any voice, reached or not) to itself or back
/// ends a subsong; each one that does starts a subsong at the next position, the first at
/// position 0, and the position after the last such jump starts none (what follows it is no
/// subsong: it never jumps back). Jumps forward (to a later position, or past the song's end)
/// end none. A module whose patterns (all of them, up to the highest in the order table, as
/// mt_init counts them) hold 127 position jumps or more has one subsong only.
fn subsong_starts(orders: &[u8; 128], song_len: usize, patterns: &[u32]) -> Vec<u8> {
    if patterns.iter().filter(|&&l| pos_jump_of(l).is_some()).count() >= 127 {
        return vec![0];
    }
    let mut starts = vec![0u8];
    let ends: Vec<usize> = (0..song_len.min(128))
        .filter(|&p| {
            let pat = orders[p] as usize;
            patterns.get(pat * 256..pat * 256 + 256).unwrap_or(&[]).iter().any(|&l| pos_jump_of(l).is_some_and(|t| t as usize <= p))
        })
        .collect();
    // the end of the last subsong starts nothing
    for &p in ends.iter().take(ends.len().saturating_sub(1)) {
        starts.push(p as u8 + 1);
    }
    starts
}

fn word(d: &[u8], o: usize) -> u16 {
    match d.get(o..o + 2) {
        Some(b) => u16::from_be_bytes([b[0], b[1]]),
        None => 0,
    }
}

impl ProTracker {
    /// PT 2.3D's time between writes (colour clocks); nothing in the PTK build.
    fn sp(&self, bus: &mut Bus, cc: u64) {
        if self.build == Build::Pt23d {
            bus.clock += cc;
        }
    }

    /// The PTK build's 68000 cycles of code path `t`; nothing in the 2.3D build.
    fn k(&mut self, bus: &mut Bus, t: usize) {
        if self.build == Build::Ptk {
            self.cyc += PTK_COSTS[t] as u64;
            bus.clock = self.cyc / 2;
        }
    }

    /// The PTK build: the time from now on counts from colour clock `cc`.
    fn anchor(&mut self, bus: &mut Bus, cc: u64) {
        if self.build == Build::Ptk {
            bus.clock = cc;
            self.cyc = cc * 2;
        }
    }

    fn write(&mut self, bus: &mut Bus, a: u32, v: u16) {
        bus.write_word(a, v);
    }

    fn dmacon(&mut self, bus: &mut Bus, v: u16) {
        self.write(bus, 0xDFF096, v);
    }

    fn reg(&mut self, bus: &mut Bus, c: usize, r: u32, v: u16) {
        self.write(bus, 0xDFF0A0 + 16 * c as u32 + r, v);
    }

    /// A MOVE.L to AUDxLC: the high word, then the low one (in the PTK build at one clock,
    /// as the engine logs a long write).
    fn ptr(&mut self, bus: &mut Bus, c: usize, a: u32) {
        self.reg(bus, c, 0, (a >> 16) as u16);
        self.sp(bus, C_LONG);
        self.reg(bus, c, 2, a as u16);
    }

    /// Recognises a 31-instrument (M.K. and friends) or 15-instrument module.
    pub fn detect(d: &[u8]) -> Option<&'static str> {
        if d.len() >= 1084 {
            if Self::modplug_adpcm(d) {
                // ModPlug's 4-bit ADPCM samples: the libxmp port's MOD loader decodes them
                return None;
            }
            match &d[1080..1084] {
                b"M.K." | b"M!K!" | b"M&K!" | b"N.T." => return Some("ProTracker"),
                b"FLT4" => return Some("StarTrekker"),
                b"4CHN" => return Some("FastTracker 4ch"),
                _ => {}
            }
        }
        if d.len() >= 600 + 1024 && Self::plausible_st(d) {
            return Some("SoundTracker");
        }
        None
    }

    /// A 31-instrument module whose sample data ModPlug packed: a sample stored as `ADPCM`, a
    /// 16-byte delta table and (length + 1) / 2 nibble bytes. Per sample (a module can mix
    /// packed and raw ones), walked as libxmp's MOD loader reads them; this replayer plays
    /// sample bytes as PCM, so such a module is left to that loader.
    pub(crate) fn modplug_adpcm(d: &[u8]) -> bool {
        if d.len() < 1084 {
            return false;
        }
        let pats = d[952..1080].iter().copied().max().unwrap_or(0) as usize + 1;
        let mut pos = 1084 + pats * 1024;
        for i in 0..31 {
            let len = word(d, 20 + i * 30 + 22) as usize * 2;
            if len == 0 {
                continue;
            }
            if pos + 5 > d.len() {
                return false;
            }
            if &d[pos..pos + 5] == b"ADPCM" {
                return true;
            }
            pos += len;
        }
        false
    }

    fn plausible_st(d: &[u8]) -> bool {
        let song_len = d[470] as usize;
        if song_len == 0 || song_len > 128 {
            return false;
        }
        if d[..20].iter().any(|&c| c != 0 && !(32..127).contains(&c)) {
            return false;
        }
        let mut total = 0usize;
        for i in 0..15 {
            let o = 20 + i * 30;
            if d[o + 25] > 64 {
                return false;
            }
            if d[o..o + 22].iter().any(|&c| c != 0 && !(32..127).contains(&c)) {
                return false;
            }
            total += word(d, o + 22) as usize * 2;
        }
        let pats = d[472..600].iter().copied().max().unwrap_or(0) as usize + 1;
        if pats > 64 {
            return false;
        }
        let need = 600 + pats * 1024 + total;
        // allow truncated sample data, but the size must be in the ballpark
        need <= d.len() + 0x8000 && 600 + pats * 1024 <= d.len()
    }

    /// The module `d`, loaded at `base` in guest memory.
    pub fn new(d: &[u8], base: u32) -> Option<Self> {
        Self::new_at(d, base, None)
    }

    /// The same with the sample data at `sample_at` in guest memory instead of after
    /// the patterns (a module laid out as a ProTracker one from another format's).
    pub fn new_at(d: &[u8], base: u32, sample_at: Option<u32>) -> Option<Self> {
        let format = Self::detect(d)?;
        let nsamp = if format == "SoundTracker" { 15 } else { 31 };
        let hdr = 20 + nsamp * 30;
        let song_len = (d[hdr] as usize).clamp(1, 128);
        let mut orders = [0u8; 128];
        orders.copy_from_slice(&d[hdr + 2..hdr + 130]);
        let pat_start = hdr + 130 + if nsamp == 31 { 4 } else { 0 };
        // as mt_init: the highest pattern of all 128 orders
        let npat = orders.iter().copied().max().unwrap_or(0) as usize + 1;
        let patterns = (0..npat * 256).map(|i| {
            let o = pat_start + i * 4;
            ((word(d, o) as u32) << 16) | word(d, o + 2) as u32
        });
        let mut samples = [Sample::default(); 32];
        let mut addr = sample_at.unwrap_or(base + (pat_start + npat * 1024) as u32);
        for (i, s) in samples[1..=nsamp].iter_mut().enumerate() {
            let o = 20 + i * 30;
            let length = word(d, o + 22);
            // SoundTracker's repeat is in bytes, and it has no finetune
            let (finetune, repeat) =
                if nsamp == 15 { (0, word(d, o + 26) / 2) } else { (d[o + 24] & 0xF, word(d, o + 26)) };
            let mut replen = word(d, o + 28);
            if length != 0 && replen == 0 {
                replen = 1; // mt_init: a replen of 0 locks up
            }
            // in the 2.3D editor an empty sample has no memory: its notes take the
            // replay's null-pointer path (length 1, no repeat), not 64K words
            let start = if length == 0 { 0 } else { addr };
            *s = Sample { start, length, finetune, volume: d[o + 25].min(64), repeat, replen };
            addr += length as u32 * 2;
        }
        let title: String = d[..20].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
        let patterns: Vec<u32> = patterns.collect();
        let starts = subsong_starts(&orders, song_len, &patterns);
        let mut pt = ProTracker {
            title: title.trim().to_string(),
            samples,
            orders,
            song_len,
            patterns,
            ch: Default::default(),
            speed: 6,
            counter: 0,
            bpm: 125,
            song_pos: 0,
            row: 0,
            pbreak_pos: 0,
            pbreak_flag: false,
            pos_jump: false,
            patt_del_time: 0,
            patt_del_time2: 0,
            low_mask: 0xFF,
            dmacon_temp: 0,
            visited: HashSet::new(),
            songend: false,
            filter: false,
            f00_ends: true,
            starts,
            sub: 0,
            build: Build::Pt23d,
            cyc: 0,
        };
        pt.restart_song();
        Some(pt)
    }

    pub fn restart_song(&mut self) {
        self.ch = Default::default();
        self.speed = 6;
        self.counter = 0;
        self.bpm = 125;
        self.song_pos = self.starts[self.sub];
        self.row = 0;
        self.pbreak_pos = 0;
        self.pbreak_flag = false;
        self.pos_jump = false;
        self.patt_del_time = 0;
        self.patt_del_time2 = 0;
        self.low_mask = 0xFF;
        self.visited.clear();
        self.songend = false;
    }

    /// How many subsongs the module has (`subsong_starts`).
    pub fn subsongs(&self) -> usize {
        self.starts.len()
    }

    /// Restarts at subsong `n` (from 0), if there is one.
    pub fn start_subsong(&mut self, n: usize) {
        if n < self.starts.len() {
            self.sub = n;
            self.restart_song();
        }
    }

    pub fn position(&self) -> (usize, usize, usize) {
        (self.song_pos as usize, self.song_len, self.row as usize)
    }

    pub fn jump(&mut self, delta: i32) {
        self.song_pos = (self.song_pos as i32 + delta).clamp(0, self.song_len as i32 - 1) as u8;
        self.row = 0;
        self.pbreak_pos = 0;
        self.pbreak_flag = false;
        self.pos_jump = false;
        self.patt_del_time = 0;
        self.patt_del_time2 = 0;
        // the next interrupt reads the row
        self.counter = self.speed.saturating_sub(1);
        self.visited.clear();
    }

    /// Colour clocks between ticks: the CIA timer latch ProTracker sets for the
    /// tempo (1773447 / BPM), plus one, E-clock ticks of 5 colour clocks.
    pub fn tick_clocks(&self) -> u64 {
        (1_773_447 / self.bpm.max(1) as u64 + 1) * 5
    }

    /// As mt_init: the first word of a sample that does not loop is cleared, so
    /// that the repeat Paula plays after it is silent.
    pub fn clear_loop_words(&self, ram: &mut [u8]) {
        for s in &self.samples[1..] {
            if s.length != 0 && s.repeat as u32 + s.replen as u32 <= 1 {
                let a = s.start as usize;
                if a + 1 < ram.len() {
                    ram[a] = 0;
                    ram[a + 1] = 0;
                }
            }
        }
    }

    /// For a replayer that leaves the repeat length of a sample without a loop at 0
    /// (64K words in Paula, which replays from the repeat pointer on) instead of
    /// ProTracker's 1: set it for every sample of the module that has data and no loop.
    pub fn set_unlooped_replen(&mut self, replen: u16) {
        for s in &mut self.samples[1..] {
            if s.length != 0 && s.repeat == 0 && s.replen <= 1 {
                s.replen = replen;
            }
        }
    }

    /// One interrupt of the replayer (mt_music).
    pub fn tick(&mut self, bus: &mut Bus) {
        let t0 = bus.clock;
        self.anchor(bus, t0);
        self.counter = self.counter.wrapping_add(1);
        if self.counter >= self.speed {
            self.counter = 0;
            self.k(bus, ptk::ROWT);
            if self.patt_del_time2 == 0 {
                self.get_new_note(bus);
            } else {
                self.k(bus, ptk::PDEL);
                self.no_new_all_channels(bus);
            }
            // mt_dskip
            self.row += 1;
            if self.patt_del_time != 0 {
                self.patt_del_time2 = self.patt_del_time;
                self.patt_del_time = 0;
            }
            if self.patt_del_time2 != 0 {
                self.patt_del_time2 -= 1;
                if self.patt_del_time2 != 0 {
                    self.row -= 1;
                }
            }
            if self.pbreak_flag {
                self.pbreak_flag = false;
                self.row = self.pbreak_pos;
                self.pbreak_pos = 0;
            }
            if self.row >= 64 {
                self.next_position();
            }
        } else {
            self.k(bus, ptk::FXT);
            self.no_new_all_channels(bus);
        }
        if self.pos_jump {
            self.next_position();
        }
    }

    fn next_position(&mut self) {
        self.row = self.pbreak_pos;
        self.pbreak_pos = 0;
        self.pos_jump = false;
        self.song_pos = self.song_pos.wrapping_add(1) & 127;
        if self.song_pos as usize >= self.song_len {
            self.song_pos = 0;
            // a later subsong plays on past the wrap, to the first row it plays again (as
            // PTK-Prowiz's subsongs do: measured on synthetic modules in uade123)
            if self.starts[self.sub] == 0 {
                self.songend = true;
                self.visited.clear();
            }
        }
    }

    fn no_new_all_channels(&mut self, bus: &mut Bus) {
        for c in 0..4 {
            if c > 0 {
                self.k(bus, ptk::FXV1 + c - 1);
            }
            self.check_effects(bus, c);
        }
    }

    /// The PTK build's DMA wait: on to the `lines`-th start of a raster line from now, and
    /// `phase` colour clocks into that line.
    fn beam_wait(&mut self, bus: &mut Bus, lines: u64, phase: u64) {
        let t = (bus.clock / LINE + lines) * LINE + phase;
        self.anchor(bus, t);
    }

    fn get_new_note(&mut self, bus: &mut Bus) {
        // a row played before (by a jump, not a loop) ends the song
        if !self.visited.insert((self.song_pos, self.row)) {
            self.songend = true;
            self.visited.clear();
            self.visited.insert((self.song_pos, self.row));
        }
        self.dmacon_temp = 0;
        let pat = self.orders[self.song_pos as usize & 127] as usize;
        for c in 0..4 {
            if c > 0 {
                self.k(bus, ptk::ROWV1 + c - 1);
            }
            let l = self.patterns.get((pat * 64 + self.row as usize) * 4 + c).copied().unwrap_or(0);
            self.play_voice(bus, c, l);
            let v = self.ch[c].volume as u16;
            self.sp(bus, C_WRITE);
            self.reg(bus, c, 8, v);
        }
        // mt_SetDMA
        if self.build == Build::Ptk {
            self.k(bus, ptk::ROW_END);
            self.beam_wait(bus, PTK_WAIT_LINES, PTK_DMA_PHASE);
            self.dmacon(bus, 0x8000 | self.dmacon_temp);
            self.k(bus, ptk::WAIT2);
            self.beam_wait(bus, PTK_WAIT_LINES, PTK_LOOP_PHASE);
            for c in 0..4 {
                if c > 0 {
                    self.k(bus, ptk::LOOP_NEXT);
                }
                let (ls, rl) = (self.ch[c].loopstart, self.ch[c].replen);
                self.ptr(bus, c, ls);
                self.k(bus, ptk::LOOP_LEN);
                self.reg(bus, c, 4, rl);
            }
            return;
        }
        self.sp(bus, DMA_WAIT);
        self.dmacon(bus, 0x8000 | self.dmacon_temp);
        self.sp(bus, DMA_WAIT);
        for c in (0..4).rev() {
            self.sp(bus, C_LOOPS);
            let (ls, rl) = (self.ch[c].loopstart, self.ch[c].replen);
            self.ptr(bus, c, ls);
            self.sp(bus, C_WRITE);
            self.reg(bus, c, 4, rl);
        }
    }

    fn per_nop(&mut self, bus: &mut Bus, c: usize) {
        self.sp(bus, C_WRITE);
        let p = self.ch[c].period;
        self.reg(bus, c, 6, p);
    }

    fn play_voice(&mut self, bus: &mut Bus, c: usize, l: u32) {
        self.sp(bus, C_ROW_CHAN);
        if self.ch[c].note == 0 && self.ch[c].cmd == 0 {
            self.k(bus, ptk::ROW_PERNOP);
            self.per_nop(bus, c);
        }
        self.k(bus, ptk::ROW_READ);
        let ch = &mut self.ch[c];
        ch.note = (l >> 16) as u16;
        ch.cmd = l as u16;
        let ins = ((ch.note >> 8) & 0xF0) as usize | (ch.cmd >> 12) as usize;
        // (instrument numbers above 31 name no sample: ignored)
        if let (true, Some(&s)) = (ins != 0, self.samples.get(ins)) {
            self.sp(bus, C_ROW_INSTR);
            self.k(bus, ptk::INS);
            let ch = &mut self.ch[c];
            ch.start = s.start;
            ch.length = s.length;
            ch.finetune = s.finetune;
            ch.volume = s.volume;
            ch.replen = s.replen;
            if s.repeat != 0 {
                ch.loopstart = s.start + s.repeat as u32 * 2;
                ch.length = s.repeat.wrapping_add(s.replen);
                self.k(bus, ptk::INS_REP);
            } else {
                ch.loopstart = s.start;
                self.k(bus, ptk::INS_NOREP);
            }
            self.ch[c].wavestart = self.ch[c].loopstart;
        }
        // mt_SetRegisters
        self.sp(bus, C_ROW_TESTS);
        let ch = self.ch[c];
        if ch.note & 0xFFF == 0 {
            self.k(bus, ptk::NONOTE);
            self.check_more_effects(bus, c);
        } else if ch.cmd & 0xFF0 == 0xE50 {
            self.k(bus, ptk::ROW_E5);
            self.ch[c].finetune = ch.cmdlo() & 0xF;
            self.set_period(bus, c);
        } else {
            match (ch.cmd >> 8) & 0xF {
                3 | 5 => {
                    self.k(bus, ptk::ROW_TONE);
                    self.set_tone_porta(bus, c);
                    self.check_more_effects(bus, c);
                }
                9 => {
                    self.k(bus, ptk::ROW_9);
                    self.check_more_effects(bus, c);
                    self.set_period(bus, c);
                }
                _ => {
                    self.k(bus, ptk::ROW_NOTE);
                    self.set_period(bus, c)
                }
            }
        }
    }

    /// Steps of a period table scan for `p` from table offset `t` (the index found).
    fn scan(&mut self, bus: &mut Bus, t: usize, p: u16, none: usize, term: usize) -> usize {
        let i = (0..37).find(|&i| p >= PERIODS[t + i]).unwrap_or(none);
        for _ in 0..=i {
            self.k(bus, term);
        }
        i
    }

    fn set_period(&mut self, bus: &mut Bus, c: usize) {
        let note = self.ch[c].note & 0xFFF;
        self.k(bus, ptk::SETPER);
        let i = self.scan(bus, 0, note, 36, ptk::SCAN);
        self.sp(bus, (i as u64 + 1) * C_SCAN);
        let ch = &mut self.ch[c];
        ch.period = PERIODS[ch.peroffset() + i];
        if ch.cmd & 0xFF0 == 0xED0 {
            // note delay: triggered by the effect
            self.k(bus, ptk::SP_ED);
            return self.check_more_effects(bus, c);
        }
        self.sp(bus, C_WRITE);
        self.k(bus, ptk::SP_DMA);
        self.dmacon(bus, 1 << c);
        let ch = &mut self.ch[c];
        if ch.wavecontrol & 4 == 0 {
            ch.vibratopos = 0;
        }
        if ch.wavecontrol & 0x40 == 0 {
            ch.tremolopos = 0;
        }
        let ch = *ch;
        self.sp(bus, C_WRITE + 10);
        self.k(bus, ptk::SP_LEN);
        self.reg(bus, c, 4, ch.length);
        self.sp(bus, C_WRITE);
        self.k(bus, ptk::SP_LC);
        self.ptr(bus, c, ch.start);
        if ch.start == 0 {
            self.ch[c].loopstart = 0;
            self.ch[c].replen = 1;
            self.sp(bus, C_WRITE);
            self.k(bus, ptk::SP_NULL);
            self.reg(bus, c, 4, 1);
        }
        self.sp(bus, C_WRITE);
        self.k(bus, ptk::SP_PER);
        self.reg(bus, c, 6, ch.period);
        self.dmacon_temp |= 1 << c;
        self.check_more_effects(bus, c);
    }

    /// The row's effects (mt_CheckMoreEffects).
    fn check_more_effects(&mut self, bus: &mut Bus, c: usize) {
        self.sp(bus, C_MORE);
        self.k(bus, ptk::CME);
        let lo = self.ch[c].cmdlo();
        match (self.ch[c].cmd >> 8) & 0xF {
            9 => {
                self.k(bus, ptk::CME_9);
                // sample offset
                let ch = &mut self.ch[c];
                if lo != 0 {
                    ch.sampleoffset = lo;
                }
                let off = (ch.sampleoffset as u16) << 7;
                if off < ch.length {
                    ch.length -= off;
                    ch.start = ch.start.wrapping_add(off as u32 * 2);
                } else {
                    ch.length = 1;
                }
            }
            0xB => {
                self.k(bus, ptk::CME_B);
                self.song_pos = lo.wrapping_sub(1);
                self.pbreak_pos = 0;
                self.pos_jump = true;
            }
            0xC => {
                self.k(bus, ptk::CME_C);
                self.ch[c].volume = lo.min(64)
            }
            0xD => {
                self.k(bus, ptk::CME_D);
                let v = (lo >> 4) * 10 + (lo & 0xF);
                self.pbreak_pos = if v > 63 { 0 } else { v };
                self.pos_jump = true;
            }
            0xE => {
                self.k(bus, ptk::CME_E);
                self.e_commands(bus, c)
            }
            0xF => {
                if lo == 0 {
                    self.k(bus, ptk::CME_F0);
                    // F00 stops ProTracker: the song's end
                    if self.f00_ends {
                        self.songend = true;
                    }
                } else if lo >= 32 {
                    self.k(bus, ptk::CME_FT);
                    self.bpm = lo as u32;
                } else {
                    self.k(bus, ptk::CME_F);
                    self.counter = 0;
                    self.speed = lo;
                }
            }
            _ => {
                self.k(bus, ptk::CME_PN);
                self.per_nop(bus, c);
                self.k(bus, ptk::CME_PN_VOL);
            }
        }
    }

    /// An effects tick of a voice (mt_CheckEffects).
    fn check_effects(&mut self, bus: &mut Bus, c: usize) {
        if !self.chkefx2(bus, c) {
            let v = self.ch[c].volume as u16;
            self.sp(bus, C_WRITE);
            self.reg(bus, c, 8, v);
        }
    }

    /// Returns true when the effect wrote the volume itself (tremolo).
    fn chkefx2(&mut self, bus: &mut Bus, c: usize) -> bool {
        self.update_funk(bus, c);
        let ch = self.ch[c];
        if ch.cmd & 0xFFF == 0 {
            self.sp(bus, C_FX_NONE);
            if self.build == Build::Ptk {
                // PTK-Prowiz writes the period of a voice without an effect too
                self.k(bus, ptk::NONE);
                self.per_nop(bus, c);
                self.k(bus, ptk::NONE_VOL);
            }
            return false;
        }
        self.sp(bus, C_FX_CHAN);
        self.k(bus, ptk::DISP);
        match (ch.cmd >> 8) & 0xF {
            0 => {
                self.arpeggio(bus, c);
                self.k(bus, ptk::ARP_VOL);
            }
            1 => {
                self.porta_up(bus, c);
                self.k(bus, ptk::PORTA_VOL);
            }
            2 => {
                self.porta_down(bus, c);
                self.k(bus, ptk::PORTA_VOL);
            }
            3 => {
                if ch.cmdlo() != 0 {
                    self.k(bus, ptk::TP_SET);
                    self.ch[c].toneportspeed = ch.cmdlo();
                    self.ch[c].cmd &= 0xFF00;
                }
                self.tone_port_no_change(bus, c);
            }
            4 => {
                let lo = ch.cmdlo();
                let v = &mut self.ch[c].vibratocmd;
                if lo & 0x0F != 0 {
                    *v = (*v & 0xF0) | (lo & 0x0F);
                }
                if lo & 0xF0 != 0 {
                    *v = (lo & 0xF0) | (*v & 0x0F);
                }
                if lo & 0x0F != 0 {
                    self.k(bus, ptk::VIB_LO);
                }
                if lo & 0xF0 != 0 {
                    self.k(bus, ptk::VIB_HI);
                }
                self.vibrato2(bus, c);
            }
            5 => {
                self.k(bus, ptk::FX5);
                if ch.cmdlo() & 0xF != 0 {
                    self.k(bus, ptk::FX5_LO);
                }
                if ch.cmdlo() & 0xF0 != 0 {
                    self.k(bus, ptk::FX5_HI);
                }
                self.tone_port_no_change(bus, c);
                self.volume_slide(bus, c);
            }
            6 => {
                self.k(bus, ptk::FX6);
                if ch.cmdlo() & 0xF != 0 {
                    self.k(bus, ptk::FX6_LO);
                }
                if ch.cmdlo() & 0xF0 != 0 {
                    self.k(bus, ptk::FX6_HI);
                }
                self.vibrato2(bus, c);
                self.volume_slide(bus, c);
            }
            0xE => {
                self.k(bus, ptk::FX_E);
                self.e_commands(bus, c);
                self.k(bus, ptk::FX_E_VOL);
            }
            cmd => {
                self.k(bus, ptk::OTHER);
                self.per_nop(bus, c);
                if cmd == 7 {
                    self.tremolo(bus, c);
                    return true;
                }
                if cmd == 0xA {
                    self.volume_slide(bus, c);
                } else {
                    self.k(bus, ptk::OTHER_VOL);
                }
            }
        }
        self.sp(bus, C_FX_VOL);
        false
    }

    fn update_funk(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        let speed = ch.glissfunk >> 4;
        if speed == 0 {
            return;
        }
        self.k(bus, ptk::FUNK);
        let ch = &mut self.ch[c];
        ch.funkoffset = ch.funkoffset.wrapping_add(FUNK[speed as usize]);
        if ch.funkoffset & 0x80 == 0 {
            return;
        }
        ch.funkoffset = 0;
        if ch.loopstart == 0 || ch.wavestart == 0 {
            return; // 2.3F and pt2-clone: no sample yet
        }
        ch.wavestart += 1;
        if ch.wavestart >= ch.loopstart + ch.replen as u32 * 2 {
            ch.wavestart = ch.loopstart;
        }
        let a = ch.wavestart;
        let b = bus.read_byte(a);
        bus.write_byte(a, !b);
    }

    fn arpeggio(&mut self, bus: &mut Bus, c: usize) {
        let ch = self.ch[c];
        let k = match self.counter % 3 {
            0 => {
                self.k(bus, ptk::ARP0);
                return self.per_nop(bus, c);
            }
            1 => ch.cmdlo() >> 4,
            _ => ch.cmdlo() & 0xF,
        } as usize;
        let t = ch.peroffset();
        self.k(bus, if self.counter % 3 == 1 { ptk::ARP1 } else { ptk::ARP2 });
        let i = self.scan(bus, t, ch.period, 36, ptk::ARP_SCAN);
        self.sp(bus, (i as u64 + 1) * C_SCAN + C_WRITE);
        self.k(bus, ptk::ARP_W);
        self.reg(bus, c, 6, PERIODS[t + i + k]);
    }

    fn porta_up(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        ch.period = ch.period.wrapping_sub((ch.cmdlo() & self.low_mask) as u16);
        self.low_mask = 0xFF;
        // PT compares the 12 bits only: an underflow is not clamped
        let clamp = ch.period & 0xFFF < 113;
        if clamp {
            ch.period = (ch.period & 0xF000) | 113;
        }
        let p = ch.period & 0xFFF;
        self.sp(bus, C_PORTA);
        self.k(bus, ptk::PORTA);
        if clamp {
            self.k(bus, ptk::PORTA_CLAMP);
        }
        self.reg(bus, c, 6, p);
    }

    fn porta_down(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        ch.period = ch.period.wrapping_add((ch.cmdlo() & self.low_mask) as u16);
        self.low_mask = 0xFF;
        let clamp = ch.period & 0xFFF > 856;
        if clamp {
            ch.period = (ch.period & 0xF000) | 856;
        }
        let p = ch.period & 0xFFF;
        self.sp(bus, C_PORTA);
        self.k(bus, ptk::PORTA_DN);
        if clamp {
            self.k(bus, ptk::PORTA_CLAMP);
        }
        self.reg(bus, c, 6, p);
    }

    fn set_tone_porta(&mut self, bus: &mut Bus, c: usize) {
        let note = self.ch[c].note & 0xFFF;
        let t = self.ch[c].peroffset();
        let mut i = self.scan(bus, t, note, 35, ptk::TONE_SCAN);
        self.sp(bus, (i as u64 + 1) * C_SCAN + C_TONE);
        let ch = &mut self.ch[c];
        let ft = ch.finetune & 8 != 0 && i > 0;
        if ft {
            i -= 1;
        }
        ch.wantedperiod = PERIODS[t + i];
        ch.toneportdirec = false;
        let term = if ch.period == ch.wantedperiod {
            ch.wantedperiod = 0;
            ptk::TONE_EQ
        } else if (ch.period as i16) > ch.wantedperiod as i16 {
            ch.toneportdirec = true;
            ptk::TONE_GT
        } else {
            ptk::TONE_LT
        };
        if ft {
            self.k(bus, ptk::TONE_FT);
        }
        self.k(bus, term);
    }

    fn tone_port_no_change(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        if ch.wantedperiod == 0 {
            self.k(bus, ptk::TP_DONE);
            return;
        }
        let sp = ch.toneportspeed as u16;
        let mut reach = false;
        if ch.toneportdirec {
            ch.period = ch.period.wrapping_sub(sp);
            if ch.period as i16 <= ch.wantedperiod as i16 {
                ch.period = ch.wantedperiod;
                ch.wantedperiod = 0;
                reach = true;
            }
        } else {
            ch.period = ch.period.wrapping_add(sp);
            if ch.period as i16 >= ch.wantedperiod as i16 {
                ch.period = ch.wantedperiod;
                ch.wantedperiod = 0;
                reach = true;
            }
        }
        let mut p = ch.period;
        let (direc, gliss) = (ch.toneportdirec, ch.glissfunk & 0xF != 0);
        self.sp(bus, C_TONE);
        self.k(bus, if direc { ptk::TP_UP } else { ptk::TP_DOWN });
        if reach {
            self.k(bus, ptk::TP_REACH);
        }
        if gliss {
            // glissando: the note's period at or below
            let t = self.ch[c].peroffset();
            self.k(bus, ptk::GLISS);
            let i = self.scan(bus, t, p, 35, ptk::GLISS_SCAN);
            self.sp(bus, (i as u64 + 1) * C_SCAN);
            p = PERIODS[t + i];
        }
        self.reg(bus, c, 6, p);
        self.k(bus, ptk::TP_VOL);
    }

    fn vibrato2(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        let pos = (ch.vibratopos >> 2) & 0x1F;
        let wave = ch.wavecontrol & 3;
        let data = match wave {
            0 => VIBRATO[pos as usize] as u16,
            1 if ch.vibratopos < 128 => (pos as u16) << 3,
            1 => 255 - ((pos as u16) << 3),
            _ => 255,
        };
        let ch = &mut self.ch[c];
        let data = (data * (ch.vibratocmd & 0xF) as u16) >> 7;
        let neg = ch.vibratopos >= 128;
        let p = if !neg { ch.period.wrapping_add(data) } else { ch.period.wrapping_sub(data) };
        self.sp(bus, C_VIB);
        self.k(bus, ptk::VIB + wave as usize);
        self.k(bus, if neg { ptk::VIB_NEG } else { ptk::VIB_POS });
        self.reg(bus, c, 6, p);
        self.k(bus, ptk::VIB_VOL);
        let ch = &mut self.ch[c];
        ch.vibratopos = ch.vibratopos.wrapping_add((ch.vibratocmd >> 2) & 0x3C);
    }

    fn tremolo(&mut self, bus: &mut Bus, c: usize) {
        let ch = &mut self.ch[c];
        let lo = ch.cmdlo();
        if lo & 0x0F != 0 {
            ch.tremolocmd = (ch.tremolocmd & 0xF0) | (lo & 0x0F);
        }
        if lo & 0xF0 != 0 {
            ch.tremolocmd = (lo & 0xF0) | (ch.tremolocmd & 0x0F);
        }
        let pos = (ch.tremolopos >> 2) & 0x1F;
        let wave = (ch.wavecontrol >> 4) & 3;
        let data = match wave {
            0 => VIBRATO[pos as usize] as i32,
            // PT tests the vibrato's position here
            1 if ch.vibratopos < 128 => (pos as i32) << 3,
            1 => 255 - ((pos as i32) << 3),
            _ => 255,
        };
        let data = (data * (ch.tremolocmd & 0xF) as i32) >> 6;
        let neg = ch.tremolopos >= 128;
        let v = if !neg { ch.volume as i32 + data } else { ch.volume as i32 - data };
        let clamp = !(0..=64).contains(&v);
        self.sp(bus, C_TREM);
        self.k(bus, ptk::TREM + wave as usize);
        self.k(bus, if neg { ptk::TREM_NEG } else { ptk::TREM_POS });
        if clamp {
            self.k(bus, ptk::TREM_CLAMP);
        }
        self.reg(bus, c, 8, v.clamp(0, 64) as u16);
        let ch = &mut self.ch[c];
        ch.tremolopos = ch.tremolopos.wrapping_add((ch.tremolocmd >> 2) & 0x3C);
    }

    fn volume_slide(&mut self, bus: &mut Bus, c: usize) {
        self.sp(bus, C_SLIDE);
        let ch = &mut self.ch[c];
        let lo = ch.cmdlo();
        let (t, clamp) = if lo >> 4 != 0 {
            let clamp = ch.volume + (lo >> 4) >= 64;
            ch.volume = (ch.volume + (lo >> 4)).min(64);
            (ptk::SLIDE_UP, clamp)
        } else {
            let clamp = ch.volume < lo & 0xF;
            ch.volume = ch.volume.saturating_sub(lo & 0xF);
            (ptk::SLIDE_DOWN, clamp)
        };
        self.k(bus, t);
        self.k(bus, if clamp { ptk::SLIDE_CLAMP } else { ptk::SLIDE_NOCLAMP });
    }

    fn e_commands(&mut self, bus: &mut Bus, c: usize) {
        self.sp(bus, C_ECMD);
        let lo = self.ch[c].cmdlo();
        let y = lo & 0xF;
        let tick0 = self.counter == 0;
        self.k(bus, if tick0 { ptk::E_ROW } else { ptk::E_FX } + (lo >> 4) as usize);
        let has_note = self.ch[c].note & 0xFFF != 0;
        let ch = &mut self.ch[c];
        match lo >> 4 {
            0 if tick0 => self.filter = y & 1 == 0,
            1 if tick0 => {
                self.low_mask = 0xF;
                self.porta_up(bus, c);
            }
            2 if tick0 => {
                self.low_mask = 0xF;
                self.porta_down(bus, c);
            }
            3 => ch.glissfunk = (ch.glissfunk & 0xF0) | y,
            4 => ch.wavecontrol = (ch.wavecontrol & 0xF0) | y,
            5 => ch.finetune = y,
            6 if tick0 => {
                if y == 0 {
                    ch.pattpos = self.row & 63;
                    return;
                }
                if ch.loopcount == 0 {
                    ch.loopcount = y;
                } else {
                    ch.loopcount -= 1;
                    if ch.loopcount == 0 {
                        return;
                    }
                }
                let from = ch.pattpos;
                self.pbreak_pos = from;
                self.pbreak_flag = true;
                // a loop's rows are not a song end
                for r in from..=self.row {
                    self.visited.remove(&(self.song_pos, r));
                }
            }
            7 => ch.wavecontrol = (y << 4) | (ch.wavecontrol & 0xF),
            9 if y != 0 => {
                if tick0 && has_note {
                    return;
                }
                if self.counter % y == 0 {
                    self.do_retrig(bus, c);
                }
            }
            0xA if tick0 => ch.volume = (ch.volume + y).min(64),
            0xB if tick0 => ch.volume = ch.volume.saturating_sub(y),
            0xC if self.counter == y => ch.volume = 0,
            0xD if self.counter == y && has_note => self.do_retrig(bus, c),
            0xE if tick0 && self.patt_del_time2 == 0 => self.patt_del_time = y + 1,
            0xF if tick0 => {
                ch.glissfunk = (y << 4) | (ch.glissfunk & 0xF);
                if y != 0 {
                    self.update_funk(bus, c);
                }
            }
            _ => {}
        }
    }

    fn do_retrig(&mut self, bus: &mut Bus, c: usize) {
        let ch = self.ch[c];
        self.sp(bus, C_WRITE);
        self.k(bus, ptk::RT_DMA);
        self.dmacon(bus, 1 << c);
        self.sp(bus, C_WRITE);
        self.k(bus, ptk::RT_LC);
        self.ptr(bus, c, ch.start);
        self.sp(bus, C_WRITE);
        self.k(bus, ptk::RT_LEN);
        self.reg(bus, c, 4, ch.length);
        self.sp(bus, C_WRITE);
        self.k(bus, ptk::RT_PER);
        self.reg(bus, c, 6, ch.period);
        self.sp(bus, DMA_WAIT);
        if self.build == Build::Ptk {
            self.k(bus, ptk::RT_END);
            self.beam_wait(bus, PTK_WAIT_LINES, PTK_DMA_PHASE);
        }
        self.dmacon(bus, 0x8000 | 1 << c);
        self.sp(bus, DMA_WAIT);
        if self.build == Build::Ptk {
            self.k(bus, ptk::WAIT2);
            self.beam_wait(bus, PTK_WAIT_LINES, PTK_LOOP_PHASE);
        }
        self.ptr(bus, c, ch.loopstart);
        self.sp(bus, C_WRITE);
        self.k(bus, ptk::LOOP_LEN);
        self.reg(bus, c, 4, ch.replen);
        self.k(bus, ptk::RT_BACK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 31-instrument module: sample 1 of `len` words (repeat `rep`, replen
    /// `replen`, finetune `ft`, volume 64), one pattern with `rows`
    /// (row, voice, note long).
    fn module(len: u16, rep: u16, replen: u16, ft: u8, rows: &[(usize, usize, u32)]) -> Vec<u8> {
        let mut d = vec![0u8; 1084 + 1024 + len as usize * 2];
        d[42..44].copy_from_slice(&len.to_be_bytes());
        d[44] = ft;
        d[45] = 64;
        d[46..48].copy_from_slice(&rep.to_be_bytes());
        d[48..50].copy_from_slice(&replen.to_be_bytes());
        for i in 1..31 {
            d[20 + i * 30 + 29] = 1;
        }
        d[950] = 1;
        d[951] = 127;
        d[1080..1084].copy_from_slice(b"M.K.");
        for &(r, c, l) in rows {
            let o = 1084 + (r * 4 + c) * 4;
            d[o..o + 4].copy_from_slice(&l.to_be_bytes());
        }
        d
    }

    /// A MOD whose sample ModPlug packed as ADPCM is left to the libxmp port's MOD loader,
    /// also when a raw sample comes first; a plain MOD stays here.
    #[test]
    fn modplug_adpcm_mod_goes_to_libxmp() {
        let plain = module(8, 0, 1, 0, &[(0, 0, 0x0D60_1000)]);
        assert_eq!(ProTracker::detect(&plain), Some("ProTracker"));
        assert!(crate::xmp::detect_open(&plain).is_none());
        // sample 1: 16 bytes, packed to "ADPCM" + 16-byte table + 8 nibble bytes
        let mut d = plain[..1084 + 1024].to_vec();
        d.extend_from_slice(b"ADPCM");
        d.extend_from_slice(&[0, 1, 2, 4, 8, 16, 32, 64, 0xff, 0xfe, 0xfc, 0xf8, 0xf0, 0xe0, 0xd0, 0xc0]);
        d.extend_from_slice(&[0x11; 8]);
        assert_eq!(ProTracker::detect(&d), None);
        assert_eq!(crate::xmp::detect_open(&d).map(|l| l.name), Some("ModPlug ADPCM MOD"));
        // a raw 4-word sample 1, then the packed one as sample 2
        let mut m = d[..1084 + 1024].to_vec();
        m[42..44].copy_from_slice(&4u16.to_be_bytes());
        m[72..74].copy_from_slice(&8u16.to_be_bytes());
        m.extend_from_slice(&[0u8; 8]);
        m.extend_from_slice(&d[1084 + 1024..]);
        assert_eq!(ProTracker::detect(&m), None);
        // "ADPCM" past a raw sample's end is not the next sample's start: plain MOD
        let mut r = plain.clone();
        r[1084 + 1024 + 2..1084 + 1024 + 7].copy_from_slice(b"ADPCM");
        assert_eq!(ProTracker::detect(&r), Some("ProTracker"));
    }

    /// Instrument numbers above 31 name no sample: ignored, no panic.
    #[test]
    fn instrument_past_31_is_ignored() {
        let d = module(4, 0, 1, 0, &[(0, 0, note(0xF0, 428, 0))]);
        let mut e = crate::engine::Engine::new(48000.0);
        e.open_replayers = true;
        let _ = e.play_data(std::path::Path::new("t.mod"), d);
        let mut out = vec![0f32; 2 * 960];
        e.render(&mut out);
    }

    /// A note long: instrument, period, command and parameter.
    fn note(ins: u32, period: u32, cmd: u32) -> u32 {
        (ins & 0xF0) << 24 | period << 16 | (ins & 0xF) << 12 | cmd
    }

    fn bus() -> Bus {
        let mut b = Bus::new();
        b.write_log = Some(Vec::new());
        b
    }

    /// Runs `ticks` interrupts; the writes of each (address, value).
    fn run(pt: &mut ProTracker, b: &mut Bus, ticks: usize) -> Vec<Vec<(u32, u16)>> {
        (0..ticks)
            .map(|_| {
                b.write_log.as_mut().unwrap().clear();
                pt.tick(b);
                b.clock += pt.tick_clocks();
                b.write_log.as_ref().unwrap().iter().map(|w| (w.0, w.1)).collect()
            })
            .collect()
    }

    const BASE: u32 = 0x10000;

    #[test]
    fn finetuned_periods_come_from_the_table() {
        // E-1 (678) at finetune +7 is 646 in PT's table, 645 by formula
        for (ft, period, want) in [(15, 428, 431), (7, 678, 646), (8, 113, 120)] {
            let d = module(100, 0, 1, ft, &[(0, 0, note(1, period, 0))]);
            let mut pt = ProTracker::new(&d, BASE).unwrap();
            let mut b = bus();
            let w = run(&mut pt, &mut b, 6);
            assert!(w[5].contains(&(0xDFF0A6, want)), "finetune {ft}: {:x?}", w[5]);
        }
    }

    #[test]
    fn row_writes_in_pt_order() {
        let d = module(100, 10, 20, 0, &[(0, 1, note(1, 428, 0))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let mut b = bus();
        let w = run(&mut pt, &mut b, 6);
        // the first row comes on the sixth interrupt; before it, volumes only
        assert_eq!(w[0], vec![(0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0)]);
        let s = BASE + 1084 + 1024;
        let (hi, lo) = ((s >> 16) as u16, s as u16);
        let (lhi, llo) = (((s + 20) >> 16) as u16, (s + 20) as u16);
        let want = vec![
            (0xDFF0A6, 0),
            (0xDFF0A6, 0),
            (0xDFF0A8, 0),
            (0xDFF0B6, 0),
            (0xDFF096, 2),
            (0xDFF0B4, 30), // repeat + replen
            (0xDFF0B0, hi),
            (0xDFF0B2, lo),
            (0xDFF0B6, 428),
            (0xDFF0B6, 428),
            (0xDFF0B8, 64),
            (0xDFF0C6, 0),
            (0xDFF0C6, 0),
            (0xDFF0C8, 0),
            (0xDFF0D6, 0),
            (0xDFF0D6, 0),
            (0xDFF0D8, 0),
            (0xDFF096, 0x8002),
            (0xDFF0D0, 0),
            (0xDFF0D2, 0),
            (0xDFF0D4, 0),
            (0xDFF0C0, 0),
            (0xDFF0C2, 0),
            (0xDFF0C4, 0),
            (0xDFF0B0, lhi),
            (0xDFF0B2, llo),
            (0xDFF0B4, 20),
            (0xDFF0A0, 0),
            (0xDFF0A2, 0),
            (0xDFF0A4, 0),
        ];
        assert_eq!(w[5], want);
    }

    #[test]
    fn dma_waits_are_pt_s() {
        let d = module(100, 0, 1, 0, &[(0, 0, note(1, 428, 0))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let mut b = bus();
        for _ in 0..5 {
            pt.tick(&mut b);
        }
        b.write_log.as_mut().unwrap().clear();
        pt.tick(&mut b);
        let l = b.write_log.clone().unwrap();
        let t = |a: u32, v: u16| l.iter().find(|w| w.0 == a && w.1 == v).unwrap().2;
        let off = t(0xDFF096, 1);
        let on = t(0xDFF096, 0x8001);
        let last_vol = l.iter().filter(|w| w.0 == 0xDFF0D8).last().unwrap().2;
        assert_eq!(on - last_vol, DMA_WAIT);
        assert!(on - off > DMA_WAIT);
        let loops = l.iter().find(|w| w.0 == 0xDFF0D0).unwrap().2;
        assert_eq!(loops - on, DMA_WAIT + C_LOOPS);
    }

    #[test]
    fn sample_offset_is_added_twice() {
        // 901 on a note: 128 words off before the trigger, 128 more after it
        let d = module(1000, 0, 1, 0, &[(0, 0, note(1, 428, 0x901)), (1, 0, note(0, 0, 0xE91))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let mut b = bus();
        let w = run(&mut pt, &mut b, 12);
        let s = BASE + 1084 + 1024;
        assert!(w[5].contains(&(0xDFF0A2, (s + 256) as u16)));
        assert!(w[5].contains(&(0xDFF0A4, 1000 - 128)));
        // the next row's E91 retriggers where the second offset left it
        assert!(w[11].contains(&(0xDFF0A2, (s + 512) as u16)), "{:x?}", w[11]);
        assert!(w[11].contains(&(0xDFF0A4, 1000 - 256)));
    }

    #[test]
    fn tempo_and_pattern_break() {
        let d = module(100, 0, 1, 0, &[(0, 0, note(0, 0, 0xF96)), (0, 1, note(0, 0, 0xD10)), (10, 0, note(0, 0, 0xF02))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let mut b = bus();
        run(&mut pt, &mut b, 6);
        assert_eq!(pt.bpm, 150);
        assert_eq!(pt.tick_clocks(), (1_773_447 / 150 + 1) * 5);
        // D10 on a one-pattern song: back to row 10 of position 0, a song end
        assert_eq!(pt.position(), (0, 1, 10));
        assert!(pt.songend);
        run(&mut pt, &mut b, 6);
        assert_eq!(pt.speed, 2);
    }

    #[test]
    fn empty_sample_is_silent() {
        // sample 2 is empty: its note gets a null pointer and length 1
        let d = module(100, 0, 1, 0, &[(0, 0, note(2, 428, 0))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        let w = run(&mut pt, &mut bus(), 6);
        let i = w[5].iter().position(|&x| x == (0xDFF096, 1)).unwrap();
        assert_eq!(&w[5][i + 1..i + 5], &[(0xDFF0A4, 0), (0xDFF0A0, 0), (0xDFF0A2, 0), (0xDFF0A4, 1)]);
        assert!(w[5].ends_with(&[(0xDFF0A0, 0), (0xDFF0A2, 0), (0xDFF0A4, 1)]));
    }

    /// Subsong starts of a song of `orders` whose patterns hold the B commands `jumps`
    /// ((pattern, row, parameter)), as uade123 reported them for the same synthetic modules.
    fn starts_of(orders: &[u8], jumps: &[(usize, usize, u8)]) -> Vec<u8> {
        let mut o = [0u8; 128];
        o[..orders.len()].copy_from_slice(orders);
        let npat = *orders.iter().max().unwrap() as usize + 1;
        let mut pats = vec![0u32; npat * 256];
        for (k, &(p, r, prm)) in jumps.iter().enumerate() {
            // on voice 1, or 2 when two share a row
            let v = 1 + jumps[..k].iter().filter(|j| j.0 == p && j.1 == r).count();
            pats[p * 256 + r * 4 + v] = 0xB00 | prm as u32;
        }
        subsong_starts(&o, orders.len(), &pats)
    }

    #[test]
    fn subsongs_start_after_each_jump_back_but_the_last() {
        // no jump; one back at position 1 (what follows never jumps back: no subsong)
        assert_eq!(starts_of(&[0, 1, 2], &[]), [0]);
        assert_eq!(starts_of(&[0, 1, 2, 3], &[(1, 10, 0)]), [0]);
        // back at 1 and 3; to itself counts; three of them
        assert_eq!(starts_of(&[0, 1, 2, 3], &[(1, 10, 0), (3, 5, 2)]), [0, 2]);
        assert_eq!(starts_of(&[0, 1, 2, 3], &[(1, 10, 1), (3, 10, 0)]), [0, 2]);
        assert_eq!(starts_of(&[0, 1, 2, 3, 4, 5], &[(1, 10, 0), (3, 10, 2), (5, 10, 4)]), [0, 2, 4]);
        // forward jumps (also past the song's end) end nothing
        assert_eq!(starts_of(&[0, 1, 2, 3, 4], &[(1, 10, 0), (2, 10, 4), (4, 10, 0)]), [0, 2]);
        assert_eq!(starts_of(&[0, 1, 2, 3], &[(1, 10, 0x7f), (3, 10, 0)]), [0]);
        // a jump nothing reaches still counts; a pattern counts at each position that plays it
        assert_eq!(starts_of(&[0, 1, 2, 3], &[(0, 20, 0), (3, 10, 2)]), [0, 1]);
        assert_eq!(starts_of(&[0, 1, 0, 2], &[(0, 10, 0)]), [0, 1]);
        assert_eq!(starts_of(&[0, 1, 2, 1, 3], &[(1, 10, 0), (3, 0, 0)]), [0, 2, 4]);
        // 126 B commands in the patterns are counted, 127 give up
        let many = |n3: usize| -> Vec<(usize, usize, u8)> { (0..64).map(|r| (1, r, 0)).chain((0..n3).map(|r| (3, r, 0))).collect() };
        assert_eq!(starts_of(&[0, 1, 2, 3], &many(62)), [0, 2]);
        assert_eq!(starts_of(&[0, 1, 2, 3], &many(63)), [0]);
    }

    #[test]
    fn a_later_subsong_plays_past_the_wrap_to_a_row_played_again() {
        // positions 0..5, row 10 of 1: B00, of 2: B05, of 3: B02 (uade123: subsong 2 plays 2.0-10,
        // 5.0-63, 0.0-63, 1.0-10 and ends on 0.0)
        let mut d = module(100, 0, 1, 0, &[]);
        d.truncate(1084);
        d[950] = 6;
        d[952..958].copy_from_slice(&[0, 1, 2, 3, 4, 5]);
        let mut pats = vec![0u8; 6 * 1024];
        for (p, b) in [(1usize, 0xB00), (2, 0xB05), (3, 0xB02)] {
            pats[p * 1024 + 10 * 16 + 4..p * 1024 + 10 * 16 + 8].copy_from_slice(&note(0, 0, b).to_be_bytes());
        }
        d.extend(pats);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        assert_eq!(pt.subsongs(), 2);
        pt.start_subsong(1);
        assert_eq!(pt.position().0, 2);
        let mut b = bus();
        // 11 + 64 rows to the wrap, which is no end
        run(&mut pt, &mut b, 6 * 75);
        assert!(!pt.songend);
        assert_eq!(pt.position(), (0, 6, 0));
        // 64 + 11 more, and the jump's row 0.0 played again
        run(&mut pt, &mut b, 6 * 75);
        assert!(!pt.songend);
        run(&mut pt, &mut b, 6);
        assert!(pt.songend);
        // subsong 1 ends at the wrap as before
        pt.start_subsong(0);
        assert_eq!(pt.position().0, 0);
    }

    #[test]
    fn f00_ends_the_song_unless_the_module_is_a_prowizard_conversion() {
        let d = module(100, 0, 1, 0, &[(0, 0, note(0, 0, 0xF00))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        run(&mut pt, &mut bus(), 6);
        assert!(pt.songend);
        // PTK-Prowiz plays on to the wrap (refcmp rows 867, 870)
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        pt.f00_ends = false;
        run(&mut pt, &mut bus(), 6 * 63);
        assert!(!pt.songend);
        assert_eq!(pt.speed, 6);
        run(&mut pt, &mut bus(), 6);
        assert!(pt.songend);
    }

    #[test]
    fn ptk_build_writes_as_ptk_prowiz() {
        let d = module(100, 10, 20, 0, &[(0, 1, note(1, 428, 0))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        pt.build = Build::Ptk;
        let mut b = bus();
        let w = run(&mut pt, &mut b, 6);
        // an effects tick: the period and the volume of every voice
        assert_eq!(w[0].iter().map(|x| x.0).collect::<Vec<_>>(), [0xDFF0A6, 0xDFF0A8, 0xDFF0B6, 0xDFF0B8, 0xDFF0C6, 0xDFF0C8, 0xDFF0D6, 0xDFF0D8]);
        // the row: the repeats first voice first, after the DMA on
        let on = w[5].iter().position(|x| *x == (0xDFF096, 0x8002)).unwrap();
        assert_eq!(w[5][on + 1..].iter().step_by(3).map(|x| x.0).collect::<Vec<_>>(), [0xDFF0A0, 0xDFF0B0, 0xDFF0C0, 0xDFF0D0]);
        // the DMA waits end on the beam: 110 and 100 colour clocks into a line
        b.write_log.as_mut().unwrap().clear();
        b.clock = 12_345 * LINE + 17;
        for _ in 0..6 {
            pt.tick(&mut b);
            b.clock += pt.tick_clocks();
        }
        let l = b.write_log.clone().unwrap();
        let on = l.iter().find(|w| w.0 == 0xDFF096 && w.1 & 0x8000 != 0).unwrap().2;
        let rep = l.iter().find(|w| w.0 == 0xDFF0A0 && w.2 > on).unwrap().2;
        assert_eq!((on % LINE, rep % LINE, (rep - on) / LINE), (PTK_DMA_PHASE, PTK_LOOP_PHASE, 9));
        // and the first write of an effects tick comes 281 colour clocks into it
        b.write_log.as_mut().unwrap().clear();
        let t = b.clock;
        pt.tick(&mut b);
        assert_eq!(b.write_log.as_ref().unwrap()[0].2 - t, 281);
    }

    #[test]
    fn led_filter_starts_off() {
        let d = module(100, 0, 1, 0, &[(0, 0, note(0, 0, 0xE00))]);
        let mut pt = ProTracker::new(&d, BASE).unwrap();
        assert!(!pt.filter);
        run(&mut pt, &mut bus(), 6);
        assert!(pt.filter);
    }
}
