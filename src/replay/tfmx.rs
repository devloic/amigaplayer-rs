//! TFMX (Chris Hülsbeck), ported from NostalgicPlayer's TFMX player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer),
//! whose engine is NostalgicPlayer's port of libTfmxAudioDecoder (GPL-2.0-or-later),
//! so this file is GPL-2.0-or-later, like the rest of amigaplayer-rs (see LICENSE).
//!
//! A TFMX song is two files: `mdat.NAME` (the song: tracks, patterns, macros)
//! and `smpl.NAME` (the samples). NostalgicPlayer plays all three kinds (1.5,
//! Professional, 7 voices) with one engine and a mixer of its own; the
//! originals are three different 68k replayers. This port follows the
//! Eagleplayer TFMX_Pro ("TFMX_Pro player module V1.4"), whose replayer it
//! transliterates: its per-voice, track and global blocks are kept byte for
//! byte in the original's layout (`Blk`), so the original's word and byte
//! overlaps (a word write over two byte fields, a byte test of a word) come out
//! the same.
//!
//! Where NostalgicPlayer differs from the original, the port follows the
//! original:
//! * the macros write Paula themselves: DMA off writes DMACON at once, or (with
//!   a non-zero second byte) at the start of the next tick, with the period 9;
//!   SetBegin, AddBegin and the SID SetBegin write the location (after waiting
//!   9 lines, as the original does), SetLen and AddLen the length, SampleLoop
//!   and OneShot both; DMA on and the periods of a tick go out at its end, after
//!   a 9-line wait, with the DMACON of the tick's DMA ons (written even when 0);
//! * the volume is written every tick (`(volume * master) >> 6`, masked to 7
//!   bits), not mixed in;
//! * the 52 macro commands of the original (NostalgicPlayer knows 34): random
//!   play, the SID synthesis (which writes its waveform into the sample memory,
//!   where the original keeps it), the memory pokes and macro self-modification;
//! * Wait's bit 0 waits for the random player, not a note; a note in a pattern
//!   is $7F..$BF with a wait (NostalgicPlayer's $80..$BF); `$F5` adds the
//!   pattern's transpose to its second byte; RoPt restores the first pattern's
//!   return point, whichever pattern it is in; command 3 of a track step (time
//!   share) is a fade, and steps over 4 stop the song; AddNote's finetune is
//!   the whole second word; no volume clamp at $40;
//! * the subsongs are the original's list (`subsong_list`), and the song ends
//!   where the original calls dtg_SongEnd;
//! * wait on DMA counts the voice's audio interrupts as the original's handler
//!   does (`audio_irq_ch`): it clears the request, counts down, and when the
//!   count goes negative lets the macro run and disables the interrupt, with
//!   the 68000's time for its instructions (no guest code runs);
//! * the random numbers come from the beam position (`VHPOSR`), as the
//!   original's do, so a random play macro matches only where the writes are
//!   timed to the colour clock.
//!
//! TFMX 1.5 (`V15`) follows the Eagleplayer TFMX_1.5 ("TFMX 1.6 player module
//! V1.91") the same way: an older replayer with the tables at $400/$600/$800,
//! 26 macro commands, additive vibrato and portamento, the period written as a
//! macro or effect sets it, the volume every tick of a running voice, and DMA
//! on once at the end of the tick through ENPP_DMAMask (whose host waits for
//! the DMA after it). Notes past its 64 periods read on into its variables; the
//! port gives the DMA word that follows the table, then 0. It calls dtg_SongEnd
//! at a pattern end on the song's last step and at a track step `EFFE 0001`.
//! Its writes go through the host's calls (ENPP_Poke*, ENPP_DMAMask), each landing
//! half the 68000 cycles its code ran since the last call returned (or since the
//! interrupt) later: the tick charges per-path costs (`COSTS15`, with per-voice
//! and per-track deviations `COSTS15_CTX`) fitted exactly to the reference's
//! gaps, and a tick that runs past the next interrupt delays it (`TICK_EXIT`).
//!
//! TFMX 7V (`Seven`) follows the Eagleplayer TFMX_7V ("TFMX 7V Eagleplayer
//! V2.35"): the Pro replayer grown to eight voices, 0-2 on Paula's channels
//! 0-2 and 4-7 mixed in software into a buffer that channel 3 plays (voice 3
//! plays on channel 3 while the mixer is off). A track step `$EFFE 0003` sets
//! the mixing rate (in kHz, up to 22) and the buffer's length (rate * 20 bytes,
//! adjusted in percent); channel 3's audio interrupt hands Paula the buffer
//! mixed in the last one, mixes the next (each voice through a volume table,
//! the four summed and clipped to a byte), then runs the replayer's tick: the
//! buffer's length is the tempo, and there is no timer interrupt. The mixer's
//! voices take the registers Paula's would (DMA on and off at once, the
//! period and volume as the tick writes them), step through their sample by
//! the period's 16.16 fraction of the mixer's, and loop as Paula does.
//!
//! An early `TFMX-SONG ` module of the fixed tables (nothing at $1D0) with no
//! `smpl.NAME` beside it but one `smpl.set` is played by the TFMX 1.5 engine
//! (`new_early`), as UADE's TFMX player does; so is one whose text starts right after
//! the header, at $0A, which the Pro check refuses (`detect_early_text`; its samples
//! `smpl.NAME`, `NAME.sam` and the rest, else `smpl.set`). UADE's player differs from the
//! Eagleplayer's in three ways found by comparing its write log: the song starts
//! at master volume $40 (no fade to $3F), SetNote adds the note's detune, and
//! DMA on goes out at the macro's command. It also reports the song end, at the
//! start of every tick while no track has a pattern and at a track step `EFFE 0000`.
//!
//! All three space their writes as the originals do: each code path costs the
//! cycles fitted to the Eagleplayers' write logs (`COSTS`, `COSTS15`; 7V and the
//! early modules colour clocks by least squares, `COST7`, `COST15`), and the
//! originals' waits for the DMA are kept.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

include!("tfmx_tables.rs");

pub const OPEN_PRO: Open = Open { name: "TFMX Pro", detect: detect_pro, new: new_pro };

/// A block of the original's variables, byte for byte.
#[derive(Clone)]
struct Blk<const N: usize>([u8; N]);

impl<const N: usize> Blk<N> {
    fn b(&self, o: usize) -> u8 {
        self.0[o]
    }
    fn w(&self, o: usize) -> u16 {
        u16::from_be_bytes([self.0[o], self.0[o + 1]])
    }
    fn l(&self, o: usize) -> u32 {
        u32::from_be_bytes([self.0[o], self.0[o + 1], self.0[o + 2], self.0[o + 3]])
    }
    fn sb(&mut self, o: usize, v: u8) {
        self.0[o] = v;
    }
    fn sw(&mut self, o: usize, v: u16) {
        self.0[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn sl(&mut self, o: usize, v: u32) {
        self.0[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
}

// The voice block (the original's at $12da6, $92 bytes each).
const V_RUN: usize = 0x00; // macro running
const V_EFX: usize = 0x01; // effects: <0 off, 0 from the next tick
const V_AB_TIME: usize = 0x03; // AddBegin counter
const V_PREV: usize = 0x04; // previous note
const V_NOTE: usize = 0x05;
const V_VEL: usize = 0x08; // word; the velocity is its low byte
const V_FINE: usize = 0x0a; // finetune, a word
const V_MACRO: usize = 0x0c;
const V_STEP: usize = 0x10;
const V_WAIT: usize = 0x12;
const V_DMA_ON: usize = 0x14;
const V_DMA_OFF: usize = 0x16;
const V_VOL: usize = 0x18;
const V_LOOP: usize = 0x1a;
const V_AB_RESET: usize = 0x1b;
const V_ENV: usize = 0x1c; // envelope speed, counter, end volume, rate
const V_VIB_WIDTH: usize = 0x20;
const V_PORTA_RESET: usize = 0x22;
const V_PORTA_TIME: usize = 0x23;
const V_VIB_OFS: usize = 0x24;
const V_VIB_RESET: usize = 0x26;
const V_VIB_TIME: usize = 0x27;
const V_PER: usize = 0x28;
const V_ADDR: usize = 0x2c;
const V_PORTA_RATE: usize = 0x30;
const V_PORTA_PER: usize = 0x32;
const V_LEN: usize = 0x34;
const V_KEY: usize = 0x36; // 1 while the key is down
const V_RND_MASK: usize = 0x37;
const V_RET: usize = 0x38;
const V_SFX: usize = 0x3c;
const V_PRIO: usize = 0x3d;
const V_LOCK: usize = 0x3e;
const V_RET_STEP: usize = 0x40;
const V_SFX_NUM: usize = 0x42;
const V_SID_LAST: usize = 0x43;
const V_INT_ON: usize = 0x44;
const V_INT_OFF: usize = 0x46;
const V_RND_SPEED: usize = 0x48;
const V_RND_MODE: usize = 0x49;
const V_RND_COUNT: usize = 0x4a;
const V_RND: usize = 0x4b;
const V_RND_PTR: usize = 0x4c;
const V_RND_STEP: usize = 0x50;
const V_RND_MACRO: usize = 0x52;
const V_RND_WAIT: usize = 0x53;
const V_AB_DELTA: usize = 0x5c;
const V_SID_BUF: usize = 0x60;
const V_SID_SRC: usize = 0x64;
const V_SID_MASK: usize = 0x68;
const V_SID: usize = 0x6a;
const V_PEND: usize = 0x8a; // a sound effect's note, for the next tick
const V_PER_OUT: usize = 0x8e; // the period, written at the end of the tick
const V_OFF_DONE: usize = 0x90; // 0 after a DMA off left to the next tick
const V_F91: usize = 0x91;
const VOICE_SIZE: usize = 0x92;

/// The voice blocks' initial contents (the original's data).
fn voice_init(ch: usize) -> Blk<VOICE_SIZE> {
    let mut v = Blk([0; VOICE_SIZE]);
    v.sw(V_DMA_ON, 0x8200 | 1 << ch);
    v.sw(V_DMA_OFF, 1 << ch);
    v.sw(V_INT_ON, 0x8000 | 0x80 << ch);
    v.sw(V_INT_OFF, 0x80 << ch);
    v.sl(V_SID_BUF, 4 + 0x100 * ch as u32);
    v.sb(V_OFF_DONE, 0xff);
    v
}

// The global block ($12d1a).
const G_MDAT: usize = 0x00;
const G_SMPL: usize = 0x04;
const G_SYNTH: usize = 0x08;
const G_RESTART: usize = 0x0d;
const G_SONG: usize = 0x10;
const G_FADE_DIR: usize = 0x11;
const G_SEED: usize = 0x12;
const G_CMD: usize = 0x18; // the command at hand, a long
const G_SPEED_CNT: usize = 0x1c;
const G_ON: usize = 0x1e;
const G_CIA: usize = 0x24;
const G_START: usize = 0x2c; // flags; the song to start is the next byte
const G_SFX7: usize = 0x2e;
const G_MASTER: usize = 0x30;
const G_FADE_END: usize = 0x31;
const G_FADE_CNT: usize = 0x32;
const G_FADE_RESET: usize = 0x33;
const G_TRACK_LOOP: usize = 0x36;
const G_STEPS: usize = 0x38;
const G_PATTERNS: usize = 0x3c;
const G_MACROS: usize = 0x40;
const G_SFX_TAB: usize = 0x44;
const G_DMA_ON: usize = 0x48;
const G_DMA_OFF: usize = 0x4a;

// The track block ($12fee): positions, speed, and eight patterns 4 bytes apart.
const P_FIRST: usize = 0x00;
const P_LAST: usize = 0x02;
const P_POS: usize = 0x04;
const P_SPEED: usize = 0x06;
const P_MUTE: usize = 0x08;
const P_PTR: usize = 0x28;
const P_NUM: usize = 0x48;
const P_XPOSE: usize = 0x49;
const P_LOOP: usize = 0x4a;
const P_STEP: usize = 0x68;
const P_WAIT: usize = 0x6a;
const P_RET: usize = 0x88;
const P_RET_STEP: usize = 0xa8;

/// The original's empty pattern (`$F4000000 $F0000000`), where every pattern
/// starts: a guest address is needed, so the port allocates it.
const EMPTY_PATTERN: [u8; 8] = [0xf4, 0, 0, 0, 0xf0, 0, 0, 0];

/// The time the original takes from its last write, by code path. Its waits for
/// the audio DMA (`dma_wait`) are to a line boundary, and are not here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
enum At {
    Tick,
    OffStart,
    OffPer,
    VoiceStart,
    Pending,
    MacroCmd,
    Effects,
    AddBegin,
    Vibrato,
    Porta,
    Env,
    Random,
    Fade,
    Volume,
    SidStart,
    SidByte,
    Tracks,
    Row,
    PatternCmd,
    Note,
    TrackStep,
    PerOut,
    /// The places the plain player waits for the DMA (nothing there in the TFHD player's
    /// code, which has its own, much shorter, costs): DMA on at a macro's command, SID
    /// start, SetBegin, and the end of the tick's DMA on.
    WaitOn,
    WaitSid,
    WaitBegin,
    WaitEnd,
    Count,
}

/// Fitted by least squares to the reference's write times (51 modules, 570000
/// gaps between writes); Pending and Random had no samples and are estimates.
const COST: [u64; At::Count as usize] = [
    300, 0, 25, 119, 160, 163, 0, 93, 124, 178, 36, 150, 34, 228, 191, 43, 166, 76, 172, 464, 456, 17, 0, 0, 0, 0,
];

/// The TFMX-Pro-TFHD player's costs (to fit): the same code paths as `COST`, but no waits
/// for the audio DMA.
/// The macro commands' names, for `COSTS`.
/// The labels of the code after a LOC/LEN write, by the command that made it (and the AddBegin effect).
const TAILS: [&str; 0x35] = ["T_00", "T_01", "T_02", "T_03", "T_04", "T_05", "T_06", "T_07", "T_08", "T_09", "T_0A", "T_0B", "T_0C", "T_0D", "T_0E", "T_0F", "T_10", "T_11", "T_12", "T_13", "T_14", "T_15", "T_16", "T_17", "T_18", "T_19", "T_1A", "T_1B", "T_1C", "T_1D", "T_1E", "T_1F", "T_20", "T_21", "T_22", "T_23", "T_24", "T_25", "T_26", "T_27", "T_28", "T_29", "T_2A", "T_2B", "T_2C", "T_2D", "T_2E", "T_2F", "T_30", "T_31", "T_32", "T_33", "T_AB"];

const MACRO_NAMES: [&str; 0x34] = [
    "M_00", "M_01", "M_02", "M_03", "M_04", "M_05", "M_06", "M_07", "M_08", "M_09", "M_0A", "M_0B", "M_0C", "M_0D",
    "M_0E", "M_0F", "M_10", "M_11", "M_12", "M_13", "M_14", "M_15", "M_16", "M_17", "M_18", "M_19", "M_1A", "M_1B",
    "M_1C", "M_1D", "M_1E", "M_1F", "M_20", "M_21", "M_22", "M_23", "M_24", "M_25", "M_26", "M_27", "M_28", "M_29",
    "M_2A", "M_2B", "M_2C", "M_2D", "M_2E", "M_2F", "M_30", "M_31", "M_32", "M_33",
];

/// `At`'s names, for `COSTS`.
const AT_NAMES: [&str; At::Count as usize] = [
    "TICK", "OFF_START", "OFF_PER", "VOICE", "PENDING", "MACRO", "EFFECTS", "ADD_BEGIN", "VIBRATO", "PORTA",
    "ENV", "RANDOM", "FADE", "VOLUME", "SID_START", "SID_BYTE", "TRACKS", "ROW", "PATTERN_CMD", "NOTE",
    "TRACK_STEP", "PER_OUT", "WAIT_ON", "WAIT_SID", "WAIT_BEGIN", "WAIT_END",
];

/// 68000 cycles of the original's tick, by code path (the names `c` takes, sorted; a name not
/// listed costs 0). A write lands at the tick's start plus half the cycles counted since the tick
/// began, rounded down; a wait for the DMA polls the beam (`dma_wait`).
const COSTS: &[(&str, i64)] = &[
    ("AB_FLIP", 44),
    ("ADD_BEGIN", 28),
    ("CO_DMA", 23),
    ("CO_DMA_POST_1", 74),
    ("CO_DMA_POST_2", 74),
    ("CO_DMA_POST_3", 74),
    ("END_DMA", 86),
    ("ENV", 47),
    ("ENV_DONE", 36),
    ("ENV_DOWN", 95),
    ("ENV_DOWN_NEG", 74),
    ("ENV_UP", 76),
    ("ENV_UP_DONE", 10),
    ("FADE_DIR", 65),
    ("FADE_STEP", 81),
    ("LEN_POST_1", 31),
    ("LEN_POST_2", 62),
    ("LEN_POST_3", 70),
    ("LOCK_NEG", 6),
    ("LOC_POST_1", 31),
    ("LOC_POST_2", 62),
    ("LOC_POST_3", 70),
    ("LOOP_DEC", 7),
    ("LOOP_END", 134),
    ("M00_DMA_POST_0", -40),
    ("M00_DMA_POST_1", -40),
    ("M00_DMA_POST_2", -40),
    ("M00_DMA_POST_3", -40),
    ("M00_LATER", 1),
    ("M01_INTREQ", 23),
    ("M04_RND_GO", 153),
    ("M10_UP", 86),
    ("M14_UP", 86),
    ("M18_LEN", 12),
    ("MACRO", 65),
    ("MV_FULL", 19),
    ("MV_SCALE", 63),
    ("M_00", 245),
    ("M_01", 103),
    ("M_04", 32),
    ("M_05", 25),
    ("M_06", 122),
    ("M_09", 2),
    ("M_0B", 343),
    ("M_0C", 418),
    ("M_0D", 188),
    ("M_0E", 156),
    ("M_0F", 191),
    ("M_10", 48),
    ("M_11", 67),
    ("M_12", 152),
    ("M_14", 48),
    ("M_18", 69),
    ("M_19", 73),
    ("M_1F", 10),
    ("NOTE", 223),
    ("NOTE_WAIT", 36),
    ("NO_SID", 106),
    ("NPER_OUT", 321),
    ("NP_INTREQ", 23),
    ("NP_INTREQ_POST_0", 174),
    ("NP_INTREQ_POST_1", 174),
    ("NP_INTREQ_POST_2", 174),
    ("NP_INTREQ_POST_3", 174),
    ("NP_NEWMAC", 16),
    ("NP_NOTE", 563),
    ("N_EFX", 137),
    ("N_PAUSE", 206),
    ("N_PAUSE_OFF", 201),
    ("N_READ", 136),
    ("N_STEP", 5),
    ("OFF_PER", 3),
    ("OFF_SKIP", 20),
    ("OFF_START", 31),
    ("OV_B3", 84),
    ("PATTERN_CMD", 151),
    ("PER_POST_0", -3),
    ("PER_POST_1", -2),
    ("PER_POST_2", -1),
    ("PER_PRE_0", 35),
    ("PER_PRE_1", 34),
    ("PER_PRE_2", 33),
    ("PER_PRE_3", 32),
    ("PORTA", 410),
    ("PORTA_REACHED", 28),
    ("PORTA_UP", 10),
    ("PORTA_UP_REACHED", -10),
    ("P_F0_LAST", 164),
    ("P_F1", 55),
    ("P_F1_DEC", 5),
    ("P_F1_END", -4),
    ("P_F3", 20),
    ("P_F5", 473),
    ("P_F6", 516),
    ("P_F7", 489),
    ("P_FC", 384),
    ("ROW", 133),
    ("ROW_FE", 115),
    ("ROW_WAIT", 16),
    ("SE_ALL", 64),
    ("SE_TRACK", 33),
    ("TICK", 486),
    ("TRACKS", 221),
    ("TR_PASS", 132),
    ("TR_RESTART", 351),
    ("TS_PAT", 104),
    ("T_02", 120),
    ("T_03", 164),
    ("T_11", 1),
    ("T_12", 156),
    ("T_19", 31),
    ("T_AB", 106),
    ("VIB_FLIP", 56),
    ("VIB_MUL", 100),
    ("VIB_OUT", 158),
    ("VOICE", 223),
    ("VOICE_1", 41),
    ("VOICE_2", 41),
    ("VOICE_3", 23),
    ("VOLUME", 271),
    ("VOL_POST_0", 45),
    ("VOL_POST_1", 76),
    ("VOL_POST_2", 107),
    ("VOL_POST_3", 115),
    ("VOL_PRE_1", 17),
    ("VOL_PRE_2", 24),
    ("VOL_PRE_3", 51),
    ("WAIT_BEGIN", 146),
    ("WAIT_DEC", 47),
    ("WAIT_ON", 36),
];
/// The cycles a name costs more (or less) in one context: 0-3 voice `i`'s code, 10-17 track `k - 10`'s.
const COSTS_CTX: &[(&str, u8, i64)] = &[
    ("M_0C", 1, 31),
    ("M_0C", 2, 62),
    ("M_0C", 3, 70),
    ("NPER_OUT", 0, -62),
    ("NPER_OUT", 1, -31),
    ("NPER_OUT", 3, 8),
    ("ROW_NONE", 10, -20),
    ("ROW_NONE", 11, -20),
    ("ROW_NONE", 12, -20),
    ("ROW_NONE", 13, -20),
    ("ROW_NONE", 14, -20),
    ("T_18", 0, 31),
    ("T_18", 2, -31),
    ("T_18", 3, -39),
    ("T_19", 1, -31),
    ("T_19", 2, -62),
    ("T_19", 3, -70),
];
/// The cycles up to a write landing (a word, a LOC's pair) and to a read of the beam line.
const EV_W: i64 = 1;
const EV_L: i64 = 75;
const EV_A: i64 = 62;

const COST_TFHD: [u64; At::Count as usize] = [
    300, 0, 27, 244, 160, 134, 0, 78, 109, 84, 56, 150, 263, 0, 191, 43, 50, 71, 127, 442, 304, 29, 0, 0, 5, 20,
];

// The TFMX-Pro-TFHD player's start-up, in colour clocks (SPEC 5.6; the parts that vary
// between runs are set to one run's).
const TFHD_PRO_LOAD_GAP: u64 = 30_000;
const TFHD_PRO_GROUP_GAP: u64 = 1_700;
const TFHD_PRO_DMA_GAP: u64 = 4_800;
const TFHD_PRO_INTENA_GAP: u64 = 264;
const TFHD_PRO_FILTER_GAP: u64 = 795;
const TFHD_PRO_VOL_GAP: u64 = 429;
const TFHD_PRO_TIMER_GAP: u64 = 1_750;
/// From the timer's start to the first tick's first write.
const TFHD_PRO_FIRST_TICK: u64 = 79_037;
/// From a tick's start to its first write (the first tick's).
const TFHD_PRO_FIRST_WRITE: u64 = 681;

pub struct Pro {
    /// Played from a TFHD file by the TFMX-Pro-TFHD player (`tfhd.rs`).
    tfhd: bool,
    v: [Blk<VOICE_SIZE>; 4],
    g: Blk<0x4c>,
    p: Blk<0xc8>,
    /// $13176: the fade lock word, the "a pattern plays" byte ($15) and the cues.
    fade_lock: u16,
    cues: [u16; 4],
    mdat: u32,
    mdat_len: u32,
    smpl: u32,
    empty: u32,
    /// Wait on DMA, per voice: the audio interrupt count (the original's
    /// handler counts a word down) and the flag it sets when it goes negative.
    dma_count: [u16; 4],
    dma_flag: [bool; 4],
    /// The subsongs (song numbers), as InitPlayer lists them.
    songs: Vec<u8>,
    sub: u16,
    /// The default timer InitSound saved ($1027a).
    timer0: u16,
    /// The module's byte $B (bit 1: a 60 Hz song, its timer 5/6 of the default).
    mdat_flag: u8,
    /// A module of sound effects only (no patterns): its subsongs are effects.
    sfx_module: bool,
    songend: bool,
    spent: u64,
    counts: [u32; At::Count as usize],
    trace: Option<std::fs::File>,
    /// In the plain player's tick: the tick's start and the 68000 cycles counted since (`COSTS`).
    in_tick: bool,
    seg: u64,
    cyc: i64,
    names: Vec<(&'static str, u32, u8)>,
    /// Whose code runs: 0-3 a voice's, 10-17 a track's, 9 neither (the costs differ).
    ctx: u8,
    site: u8,
}

/// Check2: a TFMX-SONG (or tfmxsong) module that is not 7 voices, with the
/// original's tests for odd headers.
fn detect_pro(d: &[u8]) -> bool {
    let w = |o: usize| be16(d, o);
    let l = |o: usize| be32(d, o);
    if l(0) == 0x74666d78 {
        return l(4) == 0x736f6e67 && w(8) == 0;
    }
    if l(0) != 0x54464d58 || l(4) != 0x2d534f4e || w(8) != 0x4720 {
        return false;
    }
    let mut d1 = l(0x1d0);
    if d1 != 0 {
        if d1 & 0xffff == 0 {
            // little-endian offsets
            return ((d1 >> 16) as u8 as i8) > 1;
        }
    } else {
        if w(0xc) != 0 || w(0x10) == 0x2020 || w(0x10) == 0x303d {
            return false;
        }
        // The original takes a module whose text is the editor's "(Empty) "
        // only if it is one of four it knows, leaving the others to TFMX_1.5,
        // which refuses them too (they are TFMX-SONG): the port takes them all,
        // as the original plays every one of them (with the text changed) and
        // TFMX_1.5 crashes on a third of them. The text is not read again.
        d1 = 0x800;
    }
    if w(0xe) == 0x8b0 && l(0x204) == 0x01f4ff00 {
        return false;
    }
    for s in 0..32 {
        let st = w(0x100 + 2 * s) as u32;
        if st == 0x1ff {
            continue;
        }
        let mut a = (d1 + (st << 4)) as usize;
        while w(a) == 0xeffe {
            if w(a + 2) == 3 && !(d[a + 4..].first() == Some(&0) && d.get(a + 7).is_some_and(|&b| b != 0)) {
                return false;
            }
            a += 16;
        }
    }
    let (first, end) = if l(0x1d4) != 0 {
        (l(l(0x1d8) as usize), l(0x1d4))
    } else {
        let (f, e) = (l(0x600), l(0x7fc));
        if f == 0 || f & 0x8000_0001 != 0 || e == 0 || e & 0x8000_0001 != 0 {
            return false;
        }
        (f, e)
    };
    let (mut a, end) = (first as usize, end as usize);
    while a != end {
        if a >= d.len() || d[a] > 0x3f {
            return false;
        }
        a += 4;
    }
    true
}

/// The colour clock the plain player's song starts on (its InitPlayer and InitSound have run): the
/// original's start-up measured as a black box, gap by gap between the host's calls and traps (in
/// 68000 cycles, each gap's time halved and rounded down), and modelled on the module's counts.
/// `dir_len` and `name_len` are the lengths of the module's directory (with its last "/") and file
/// name, which the companion file's load costs per character.
fn pro_song_start(d: &[u8], dir_len: usize, name_len: usize) -> u64 {
    let w = |o: usize| if o + 2 <= d.len() { be16(d, o) } else { 0 };
    let l = |o: usize| if o + 4 <= d.len() { be32(d, o) } else { 0 };
    let packed = l(0x1d0) != 0;
    let flag2 = d[0xb] == 2;
    // a module of sound effects only (its pattern table is its macro table)
    let sfx = packed && l(0x1d4) == l(0x1d8);
    let ts = if packed { l(0x1d0) as usize } else { 0x800 };
    let (first, end) =
        if packed { (l(l(0x1d8) as usize) as usize, l(0x1d4) as usize) } else { (l(0x600) as usize, l(0x7fc) as usize) };
    // the check: the macro region's steps, the songs' starts and the $EFFE steps they begin with
    let a = if flag2 {
        137
    } else {
        let (mut k, mut m) = (0i64, 0i64);
        for s in 0..32 {
            let mut p = w(0x100 + 2 * s) as usize;
            if p == 0x1ff {
                continue;
            }
            m += 1;
            while k < 0x10000 && w(ts + 16 * p) == 0xeffe {
                k += 1;
                p += 1;
            }
        }
        (if packed { 1578 } else { 1749 }) + 44 * (end.saturating_sub(first) / 4) as i64 + 68 * k + 57 * m
    };
    let b9 = 472 + 17 * dir_len as i64 + 21 * name_len as i64;
    // InitPlayer: the walk back from the pattern table to the last macro's start (packed), the patterns
    // 8 bytes long (unpacked), and the scan of the macros for the samples they play
    let (mut back, mut pat8) = (0i64, 0i64);
    let (mut count, table) = if packed {
        let mut a3 = l(0x1d4) as usize;
        let mut g = 0;
        loop {
            a3 = a3.wrapping_sub(4);
            g += 1;
            if l(a3) == 0x0700_0000 || g > 100_000 {
                break;
            }
        }
        let d4a = a3;
        loop {
            back += 1;
            if l(a3) == 0x2020_2020 {
                a3 = d4a;
                while a3 < d.len() && d[a3] < 0x80 && g < 200_000 {
                    a3 = a3.wrapping_sub(4);
                    g += 1;
                }
                break;
            }
            a3 = a3.wrapping_sub(4);
            g += 1;
            if l(a3) == 0x0700_0000 || g > 200_000 {
                break;
            }
        }
        let last = a3.wrapping_add(4) as u32;
        let mt = l(0x1d8) as usize;
        let (mut a1, mut n) = (mt, 0u32);
        loop {
            n += 1;
            let x = l(a1);
            a1 += 4;
            if x == 0 || x >= 0x8000_0000 || x == last || a1 >= d.len() {
                break;
            }
        }
        (n, mt)
    } else {
        let (mut a1, mut k) = (0x400usize, 0x7fu32);
        'p: while k != 0 {
            k -= 1;
            loop {
                let d1 = l(a1);
                a1 += 4;
                if l(a1).wrapping_sub(d1) as u16 == 8 {
                    pat8 += 1;
                    continue 'p;
                }
                k = (k & 0xffff_0000) | (k as u16).wrapping_sub(1) as u32;
                if k as u16 == 0xffff {
                    break 'p;
                }
            }
        }
        (0x7f, 0x600)
    };
    let (mut adv, mut f22, mut ends, mut upd, mut visited) = (0i64, 0i64, 0i64, 0i64, 0i64);
    let (mut mx, mut a4, mut last_end, mut guard) = (0u32, table, false, 0u32);
    'm: while count != 0 {
        count -= 1;
        loop {
            let d1 = l(a4) as usize;
            a4 += 4;
            if !packed && (l(a4) as usize).wrapping_sub(d1) as u16 == 8 {
                last_end = false;
                continue 'm;
            }
            visited += 1;
            let mut a1 = d1;
            let mut found = None;
            loop {
                guard += 1;
                if guard > 0x40_0000 {
                    break 'm;
                }
                let b = d.get(a1).copied().unwrap_or(0);
                if b == 2 || b == 0x22 {
                    found = Some(b);
                    break;
                }
                let x = l(a1);
                a1 += 4;
                if x == 0x0700_0000 {
                    break;
                }
                adv += 1;
            }
            let Some(b) = found else {
                ends += 1;
                last_end = true;
                continue 'm;
            };
            last_end = false;
            f22 += i64::from(b == 0x22);
            let e = (l(a1) & 0xff_ffff).wrapping_add(2 * w(a1 + 6) as u32);
            if e >= mx {
                mx = e;
                upd += 1;
            }
            count = (count & 0xffff_0000) | (count as u16).wrapping_sub(1) as u32;
            if count as u16 == 0xffff {
                break 'm;
            }
        }
    }
    let mut c = 81 * adv + 31 * f22 - 28 * ends + 4 * upd - 8 * i64::from(last_end);
    c += if packed { 12349 + 237 * visited + 60 * back + 82 * i64::from(sfx) } else { 33321 + 107 * visited + 3 * pat8 };
    // the test for the known module whose song end InitPlayer fixes
    if w(0xe) == 0xa40 {
        c += if w(0x112) == 0x26 { 106 } else { 25 };
    }
    // the subsong list's tests: each song taken, and some of the ways to a test
    let mut zero = false;
    for i in 0..31 {
        let a1 = 2 * i;
        let (d2, d3) = (w(a1 + 0x102), w(a1 + 0x142));
        let last = a1 >= 0x3c;
        let mut p = String::new();
        p.push(if d3 == 0 { 'Z' } else { 'z' });
        if d3 == 0 {
            p.push(if zero { 'S' } else { 's' });
            if zero {
                continue;
            }
        }
        if d2 == 0x1ff {
            continue;
        }
        p.push('u');
        if last {
            if d3 == w(a1 + 0x140) {
                continue;
            }
            p.push('l');
        }
        if d3 == w(a1 + 0x144) {
            p.push('A');
            if d2 == w(a1 + 0x104) {
                p.push('B');
                if !zero {
                    zero = true;
                    if d3 == w(a1 + 0x140) {
                        c -= 17;
                        continue;
                    }
                    p.push('q');
                }
                if d3 == d2 || !last || d3 == w(a1 + 0x146) {
                    continue;
                }
                p.push('r');
            } else {
                p.push('b');
            }
        } else {
            p.push('a');
        }
        c += 44;
        match p.as_str() {
            "Zsua" => c += 17,
            "zuAb" | "zula" => c += 21,
            _ => {}
        }
    }
    let dd = if flag2 { 1910 } else if packed { 1724 } else { 1670 };
    // InitSound: song 0's first step (its $EFFE commands, then the tracks it starts)
    let mut f = 1702 + 164 * i64::from(w(0x180) >= 16) + 186 * i64::from(flag2);
    let mut o = ts + 16 * w(0x100) as usize;
    let mut stop = false;
    for _ in 0..16 {
        if w(o) != 0xeffe {
            break;
        }
        let (cmd, w2, w3) = (w(o + 2), w(o + 4), w(o + 6));
        f += match cmd {
            0 => {
                stop = true;
                -6
            }
            2 if w3 & 0x8000 != 0 => 227,
            2 if w3 == 0 => 248,
            2 => 524,
            4 => 342 - 2 * i64::from(w2 != 0),
            _ => 227,
        };
        if stop {
            break;
        }
        o += 16;
    }
    if !stop {
        f += 104 * (0..8).filter(|j| w(o + 2 * j) >> 8 < 0x80).count() as i64;
    }
    let g = 233 + 624 * i64::from(sfx);
    let half = |x: i64| (x.max(0) / 2) as u64;
    2789 + half(a) + half(b9) + half(c) + half(dd) + half(f) + half(g)
}

/// A divu.w: the quotient, or the dividend's low word when it overflows.
fn divu(n: u32, d: u16) -> u16 {
    match n.checked_div(d as u32) {
        Some(q) if q <= 0xffff => q as u16,
        _ => n as u16,
    }
}

/// The names the samples of the module `file` are looked for under, in order: the
/// original's ExtLoad, `smpl.NAME` for `mdat.NAME` (the prefix in any case), else `smpl.FILE`;
/// then, as NostalgicPlayer looks for them, `NAME.smpl` and `NAME.sam` for a module named
/// `NAME.EXT` (`NAME.tfx` / `NAME.sam` in the NostalgicPlayer collections). A `.xz` or `.gz`
/// suffix of the module's name (a wrapper the engine undid) is left out of the later names.
fn smpl_names(file: &str) -> Vec<String> {
    fn prefixed(f: &str) -> Option<&str> {
        match f.get(..5) {
            Some(p) if p.eq_ignore_ascii_case("mdat.") => Some(&f[5..]),
            _ => None,
        }
    }
    let mut v = vec![format!("smpl.{}", prefixed(file).unwrap_or(file))];
    let lower = file.to_ascii_lowercase();
    let bare = if lower.ends_with(".xz") || lower.ends_with(".gz") { &file[..file.len() - 3] } else { file };
    match prefixed(bare) {
        Some(rest) => {
            v.push(format!("smpl.{rest}"));
            // `.pp` (PowerPacker) is a storage suffix, and rippers did not always put it on
            // both halves: aminet's tfmxtunes8 has `MDAT.BC-Kid_Title.pp` beside a plain
            // `SMPL.BC-Kid_Title`. Tried last, so a `smpl.NAME.pp` that is there wins.
            if let Some(n) = rest.len().checked_sub(3).filter(|&n| n > 0 && rest[n..].eq_ignore_ascii_case(".pp")) {
                v.push(format!("smpl.{}", &rest[..n]));
            }
        }
        None => {
            if let Some((stem, _)) = bare.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()) {
                v.push(format!("{stem}.smpl"));
                v.push(format!("{stem}.sam"));
            }
        }
    }
    v.dedup();
    v
}

/// The module's samples: the first of `smpl_names` there is.
fn read_smpl(m: &Machine) -> Option<Vec<u8>> {
    let file = m.bus.read_cstr(m.deli.file_buf);
    smpl_names(&file).iter().find_map(|n| crate::deli::read_companion(m, n).1.ok())
}

fn new_pro(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_pro(d) {
        return None;
    }
    let smpl_data = match read_smpl(m) {
        Some(s) => s,
        // an early song of the fixed tables, whose samples are `smpl.set`
        None => return new_early(d, base, m),
    };
    let smpl = m.alloc((smpl_data.len() as u32).max(4));
    if smpl == 0 {
        return None;
    }
    m.bus.write_bytes(smpl, &smpl_data);
    make_pro(d, base, smpl, m, None)
}

/// The engines for a module whose song part `d` (in guest memory at `base`) and samples come
/// from elsewhere than `mdat.NAME` and `smpl.NAME`, the samples in guest memory at `smpl`
/// (TFMX-MOD files: `tfmxmod.rs`): the replayer the plain pair would play, with the subsong
/// whose song-table entry is `song` (or the first after it) to start, and the format's name.
pub(super) fn new_with_samples(d: &[u8], base: u32, smpl: u32, song: u8, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (mut r, sub) = if detect_pro(d) {
        let r = make_pro(d, base, smpl, m, None)?;
        let sub = subsong_list(&m.bus, base).iter().position(|&s| s >= song).unwrap_or(0);
        (r, sub as u16)
    } else if detect_15(d) {
        let r = make_15(d, base, smpl, false)?;
        let sub = (song as u16).min(r.subsongs().1);
        (r, sub)
    } else if detect_7v(d) {
        let r = make_7v(d, base, smpl, m)?;
        let sub = (song as u16).min(r.subsongs().1);
        (r, sub)
    } else {
        return None;
    };
    if sub != r.subsongs().2 {
        let mut hw = Hw { bus: &mut m.bus };
        r.set_subsong(sub, &mut hw);
    }
    Some(r)
}

/// Whether one of the engines takes the song part `d` (see `new_with_samples`).
pub(super) fn detect_any(d: &[u8]) -> bool {
    detect_pro(d) || detect_15(d) || detect_7v(d)
}

/// The TFMX-Pro-TFHD player's engine: the song part `d` is in guest memory at `base`, its
/// samples are at `smpl` (in the same file, no copy). `songs` is the number of subsongs the
/// player lists (its compacted song table, see `tfhd.rs`).
pub(super) fn new_pro_tfhd(d: &[u8], base: u32, smpl: u32, songs: u8, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    make_pro(d, base, smpl, m, Some(songs))
}

fn make_pro(d: &[u8], base: u32, smpl: u32, m: &mut Machine, tfhd: Option<u8>) -> Option<Box<dyn Replayer>> {
    let empty = m.alloc(EMPTY_PATTERN.len() as u32);
    if empty == 0 {
        return None;
    }
    m.bus.write_bytes(empty, &EMPTY_PATTERN);
    let mut p = Pro {
        tfhd: tfhd.is_some(),
        v: std::array::from_fn(voice_init),
        g: Blk([0; 0x4c]),
        p: Blk([0; 0xc8]),
        fade_lock: 0,
        cues: [0; 4],
        mdat: base,
        mdat_len: d.len() as u32,
        smpl,
        empty,
        dma_count: [0; 4],
        dma_flag: [false; 4],
        songs: Vec::new(),
        sub: 0,
        timer0: crate::deli::DEFAULT_TIMER,
        mdat_flag: d[0xb],
        sfx_module: false,
        songend: false,
        spent: 0,
        counts: [0; At::Count as usize],
        trace: std::env::var_os("TFMX_TRACE").and_then(|p| std::fs::File::create(p).ok()),
        in_tick: false,
        seg: 0,
        cyc: 0,
        names: Vec::new(),
        ctx: 9,
        site: 0xff,
    };
    p.g.sw(G_CIA, 0x7fff);
    p.g.sw(G_MASTER, 0x4040);
    p.g.sw(G_TRACK_LOOP, 0xffff);
    p.p.sw(P_SPEED, 6);
    let mut hw = Hw { bus: &mut m.bus };
    if let Some(n) = tfhd {
        // the TFHD player does not refuse a module for what the plain player's tests find: what
        // it takes from InitPlayer is the song table (done by the caller) and a subsong list
        let _ = p.init_player(hw.bus);
        p.songs = (0..n.max(1)).collect();
        hw.bus.w8(base + 0xff, 0);
        p.tfhd_load(&mut hw);
    } else {
        p.init_player(hw.bus)?;
    }
    p.init_sound(0, &mut hw);
    if tfhd.is_some() {
        p.tfhd_started(&mut hw);
    }
    if tfhd.is_none() {
        let dir = m.deli.module_dir.to_string_lossy();
        let dir_len = dir.len() + usize::from(!dir.ends_with('/'));
        let file = m.bus.read_cstr(m.deli.file_buf);
        let name_len = file.rsplit('/').next().unwrap_or(&file).len();
        let start = pro_song_start(d, dir_len, name_len);
        m.bus.clock = m.bus.clock.max(start);
    }
    Some(Box::new(p))
}

impl Pro {
    fn at(&mut self, a: At) {
        if self.in_tick {
            return self.c(AT_NAMES[a as usize]);
        }
        self.spent += if self.tfhd { COST_TFHD[a as usize] } else { COST[a as usize] };
        self.counts[a as usize] += 1;
    }

    /// The original's code for `name` runs once (in the plain player's tick).
    fn c(&mut self, name: &'static str) {
        if !self.in_tick {
            return;
        }
        if let Ok(k) = COSTS.binary_search_by(|e| e.0.cmp(name)) {
            self.cyc += COSTS[k].1;
        }
        if let Some(e) = COSTS_CTX.iter().find(|e| e.0 == name && e.1 == self.ctx) {
            self.cyc += e.2;
        }
        if self.trace.is_some() {
            let v = self.ctx;
            match self.names.iter_mut().find(|e| e.0 == name && e.2 == v) {
                Some(e) => e.1 += 1,
                None => self.names.push((name, 1, v)),
            }
        }
    }

    /// The tick's code has reached its cycle count: the clock it falls on.
    fn now(&self) -> u64 {
        self.seg + (self.cyc.max(0) / 2) as u64
    }

    fn trace_cyc(&mut self, hw: &Hw, what: &str) {
        if let Some(f) = &mut self.trace {
            use std::io::Write;
            let c: Vec<String> = self
                .names
                .iter()
                .map(|(n, k, v)| if *v < 4 { format!("{n}#{v}:{k}") } else if (10..18).contains(v) { format!("{n}@{}:{k}", v - 10) } else { format!("{n}:{k}") })
                .collect();
            let _ = writeln!(f, "{} {} {} {}", hw.bus.clock, what, self.cyc, c.join(","));
        }
        self.names.clear();
    }

    /// Lets the time pass the original took since its last write.
    fn flush(&mut self, hw: &mut Hw, what: &str) {
        if self.in_tick {
            self.cyc += match what {
                "w" => EV_W,
                "L" => EV_L,
                "A" => EV_A,
                _ => 0,
            };
            let t = self.now();
            if t > hw.bus.clock {
                hw.bus.clock = t;
            }
            return self.trace_cyc(hw, what);
        }
        hw.wait(self.spent);
        self.spent = 0;
        if let Some(f) = &mut self.trace {
            use std::io::Write;
            let c: Vec<String> =
                self.counts.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| format!("{i}:{n}")).collect();
            let _ = writeln!(f, "{} {} {}", hw.bus.clock, what, c.join(","));
        }
        self.counts = [0; At::Count as usize];
    }

    fn reg(&mut self, hw: &mut Hw, a: u32, v: u16) {
        self.flush(hw, "w");
        hw.bus.write_word(a, v);
    }

    /// A write made by the audio interrupt handler (not by the tick): only the trace sees it.
    fn trace_write(&mut self, hw: &mut Hw) {
        if let Some(f) = &mut self.trace {
            use std::io::Write;
            let _ = writeln!(f, "{} w", hw.bus.clock);
        }
    }

    fn dmacon(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff096, v);
    }

    fn intena(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff09a, v);
    }

    fn intreq(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff09c, v);
    }

    fn loc(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.c(["LOC_PRE_0", "LOC_PRE_1", "LOC_PRE_2", "LOC_PRE_3"][ch]);
        self.flush(hw, "L");
        hw.loc(ch, a);
        self.c(["LOC_POST_0", "LOC_POST_1", "LOC_POST_2", "LOC_POST_3"][ch]);
        self.tail();
    }

    fn len(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.c(["LEN_PRE_0", "LEN_PRE_1", "LEN_PRE_2", "LEN_PRE_3"][ch]);
        self.reg(hw, 0xdff0a4 + 16 * ch as u32, v);
        self.c(["LEN_POST_0", "LEN_POST_1", "LEN_POST_2", "LEN_POST_3"][ch]);
        self.tail();
    }

    fn tail(&mut self) {
        if (self.site as usize) < TAILS.len() {
            self.c(TAILS[self.site as usize]);
        }
    }

    fn per(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.c(["PER_PRE_0", "PER_PRE_1", "PER_PRE_2", "PER_PRE_3"][ch]);
        self.reg(hw, 0xdff0a6 + 16 * ch as u32, v);
        self.c(["PER_POST_0", "PER_POST_1", "PER_POST_2", "PER_POST_3"][ch]);
    }

    /// $1134c: the volume, through the Eagleplayer's volume (64: unchanged).
    fn vol(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.c(["VOL_PRE_0", "VOL_PRE_1", "VOL_PRE_2", "VOL_PRE_3"][ch]);
        self.reg(hw, 0xdff0a8 + 16 * ch as u32, v & 0x7f);
        self.c(["VOL_POST_0", "VOL_POST_1", "VOL_POST_2", "VOL_POST_3"][ch]);
    }

    /// $107ee: waits for the beam to start 9 new lines.
    fn dma_wait(&mut self, hw: &mut Hw, place: At) {
        if self.tfhd {
            // the TFHD player has none of these waits, only its code's own time there
            self.at(place);
            return;
        }
        if self.in_tick {
            // nine times: a read of the line, then reads every 25 cycles until it changes
            self.at(place);
            self.flush(hw, "A");
            let line = |c: i64| (self.seg + (c / 2) as u64) / LINE;
            let mut c = self.cyc;
            for k in 0..9 {
                let l = line(c);
                c += 16;
                while line(c) == l {
                    c += 25;
                }
                if k < 8 {
                    c += 35;
                }
            }
            self.cyc = c;
            return self.flush(hw, "X");
        }
        self.flush(hw, "d");
        let now = hw.bus.clock;
        hw.wait((now / LINE + 9) * LINE - now);
    }

    /// $127d0: the random number, from the beam position.
    fn random(&mut self, hw: &mut Hw) {
        self.flush(hw, "r");
        let (line, h) = hw.bus.beam();
        let d7 = ((line as u16 & 0xff) << 8) | h as u16;
        let s = self.g.w(G_SEED) ^ d7;
        self.g.sw(G_SEED, s.wrapping_add(0x4335));
    }

    /// The address of macro `n` (any number: the table is not range checked).
    fn macro_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.g.l(G_MACROS).wrapping_add(n * 4)))
    }

    fn pattern_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.g.l(G_PATTERNS).wrapping_add(n * 4)))
    }

    /// The interrupt handler's flag: its count went negative, the macro runs.
    fn irq_flag(&mut self, i: usize) {
        if std::mem::take(&mut self.dma_flag[i]) {
            self.v[i].sb(V_RUN, 0xff);
        }
    }

    fn set_dma_count(&mut self, i: usize, n: u16) {
        self.dma_count[i] = n;
        self.dma_flag[i] = false;
    }

    /// InitPlayer: the module's fixes, the subsong list, and the tests the
    /// original fails on (it refuses the module).
    fn init_player(&mut self, bus: &mut crate::bus::Bus) -> Option<()> {
        let a0 = self.mdat;
        let end = a0 + self.mdat_len;
        // a known module's song end
        if bus.r16(a0 + 0xe) == 0xa40 && bus.r16(a0 + 0x112) == 0x26 && bus.r16(a0 + 0x152) == 0 {
            bus.w16(a0 + 0x152, 0x26);
        }
        self.songs = subsong_list(bus, a0);
        let mut sfx_count = None;
        let data_end;
        let mut d4: u32;
        let table;
        let off = bus.r32(a0 + 0x1d0);
        if off == 0 {
            // the end of the last macro: the song data's size
            let mut a3 = a0.wrapping_add(bus.r32(a0 + 0x7fc));
            loop {
                let x = bus.r32(a3);
                a3 = a3.wrapping_add(4);
                if x == 0x0700_0000 {
                    break;
                }
                if (end as i32).wrapping_sub(a3 as i32) <= 0 {
                    return None;
                }
            }
            data_end = a3.wrapping_sub(a0);
            // the patterns that are not 8 bytes long
            let (mut a1, mut n, mut k) = (a0 + 0x400, 0u32, 0x7fu32);
            'p: loop {
                if k == 0 {
                    break;
                }
                k -= 1;
                loop {
                    let d1 = bus.r32(a1);
                    a1 += 4;
                    if bus.r32(a1).wrapping_sub(d1) as u16 == 8 {
                        continue 'p;
                    }
                    n += 1;
                    k = (k & 0xffff_0000) | (k as u16).wrapping_sub(1) as u32;
                    if k as u16 == 0xffff {
                        break 'p;
                    }
                }
            }
            if n == 0 {
                self.sfx_module = true;
            }
            d4 = 0x7f;
            table = a0 + 0x600;
        } else {
            let mut d2 = off;
            if d2 & 0xffff == 0 {
                // little-endian offsets
                for o in [0x1d0, 0x1d4, 0x1d8] {
                    let v = bus.r32(a0 + o);
                    bus.w32(a0 + o, v.swap_bytes());
                }
                d2 = bus.r32(a0 + 0x1d0);
            }
            let d1 = bus.r32(a0 + 0x1d4);
            let mut a3 = a0.wrapping_add(d1);
            if d1 == bus.r32(a0 + 0x1d8) {
                sfx_count = Some(d2.wrapping_sub(0x200) >> 3);
                self.sfx_module = true;
            }
            let first_pattern = bus.r32(a3);
            let mut guard = 0u32;
            loop {
                a3 = a3.wrapping_sub(4);
                guard += 1;
                if bus.r32(a3) == 0x0700_0000 || guard > 0x40_0000 {
                    break;
                }
            }
            let d4a = a3;
            // back to the start of the last macro
            loop {
                if bus.r32(a3) == 0x2020_2020 {
                    a3 = d4a;
                    while (bus.r8(a3) as i8) >= 0 && guard < 0x80_0000 {
                        a3 = a3.wrapping_sub(4);
                        guard += 1;
                    }
                    break;
                }
                a3 = a3.wrapping_sub(4);
                guard += 1;
                if bus.r32(a3) == 0x0700_0000 || guard > 0x80_0000 {
                    break;
                }
            }
            let last = a3.wrapping_add(4).wrapping_sub(a0);
            let d3 = bus.r32(a0 + 0x1d8);
            let mut a1 = a0.wrapping_add(d3);
            d4 = 0;
            loop {
                d4 += 1;
                let d1 = bus.r32(a1);
                a1 = a1.wrapping_add(4);
                if d1 == 0 || (d1 as i32) < 0 {
                    return None;
                }
                if d1 == last {
                    break;
                }
                if (end as i32).wrapping_sub(a1 as i32) <= 0 {
                    return None;
                }
            }
            let a1o = a1.wrapping_sub(a0);
            data_end = a1o;
            if a1o == 0x32e0 && bus.r32(a0 + 0x2f24) == 0x0400_0027 {
                bus.w32(a0 + 0x2f24, 0x0400_0026);
            }
            let _ = first_pattern;
            table = a0.wrapping_add(d3);
        }
        // the macros that play a sample: the subsongs of a sound effects module
        let (mut d3, mut a4) = (0u32, table);
        let mut guard = 0u32;
        'm: loop {
            if d4 == 0 {
                break;
            }
            d4 = d4.wrapping_sub(1);
            loop {
                let d1 = bus.r32(a4);
                a4 = a4.wrapping_add(4);
                if off == 0 && bus.r32(a4).wrapping_sub(d1) as u16 == 8 {
                    continue 'm;
                }
                let mut a1 = a0.wrapping_add(d1);
                loop {
                    guard += 1;
                    if guard > 0x40_0000 {
                        break 'm;
                    }
                    let b = bus.r8(a1);
                    if b == 2 || b == 0x22 {
                        if b == 2 {
                            d3 += 1;
                        }
                        break;
                    }
                    let x = bus.r32(a1);
                    a1 = a1.wrapping_add(4);
                    if x == 0x0700_0000 {
                        continue 'm;
                    }
                }
                d4 = (d4 & 0xffff_0000) | (d4 as u16).wrapping_sub(1) as u32;
                if d4 as u16 == 0xffff {
                    break 'm;
                }
            }
        }
        if off == 0 && self.sfx_module {
            sfx_count = Some(d3);
        }
        if let Some(n) = sfx_count {
            self.songs.resize(n as usize, 0);
        }
        if data_end >= 0x9c40 && bus.r16(a0 + 0x108) == 0x28 {
            bus.w16(a0 + 0x108, 0x27);
        }
        Some(())
    }
}

/// InitPlayer's subsong list: song 0, then each later song unless its start is
/// $1FF, it repeats the next song, or it is an empty song after the first.
fn subsong_list(bus: &crate::bus::Bus, a0: u32) -> Vec<u8> {
    let w = |o: u32| bus.r16(a0 + o);
    let mut songs = vec![0u8];
    let mut zero_seen = false;
    for i in 0..31u32 {
        let a1 = 2 * i;
        let d1 = i as u8 + 1;
        let (d2, d3) = (w(a1 + 0x102), w(a1 + 0x142));
        let last = a1 >= 0x3c;
        if d3 == 0 && zero_seen {
            continue;
        }
        if d2 == 0x1ff {
            continue;
        }
        if last && d3 == w(a1 + 0x140) {
            continue;
        }
        if d3 == w(a1 + 0x144) && d2 == w(a1 + 0x104) {
            // the next song is the same
            if !zero_seen {
                zero_seen = true;
                if d3 == w(a1 + 0x140) {
                    continue;
                }
            }
            if d3 == d2 || !last || d3 == w(a1 + 0x146) {
                continue;
            }
        }
        songs.push(d1);
    }
    songs
}

/// How a macro command goes on.
enum Next {
    /// Read the next step (the command set the step itself).
    Read,
    /// Step on and read the next.
    Step,
    /// $11bf4: step on and end the macro for this tick, unless a DMA off was
    /// left to this tick (then go on reading).
    Pause,
    /// To the effects.
    Effects,
}

impl Pro {
    /// InitSound: the player's init, the subsong's song, and its sound effect
    /// in a sound effects module.
    fn init_sound(&mut self, sub: u16, hw: &mut Hw) {
        self.sub = sub;
        self.songend = false;
        self.g.sw(G_CIA, self.timer());
        // $12c4a
        self.g.sl(G_MASTER, 0x4040_0000);
        self.g.sb(G_FADE_DIR, 0);
        self.g.sl(G_MDAT, self.mdat);
        self.g.sl(G_SMPL, self.smpl);
        hw.bus.w32(self.smpl, 0);
        self.g.sl(G_SYNTH, self.smpl);
        let a = self.mdat;
        if hw.bus.r32(a + 0x1d0) != 0 {
            self.g.sl(G_STEPS, a.wrapping_add(hw.bus.r32(a + 0x1d0)));
            self.g.sl(G_PATTERNS, a.wrapping_add(hw.bus.r32(a + 0x1d4)));
            self.g.sl(G_MACROS, a.wrapping_add(hw.bus.r32(a + 0x1d8)));
            self.g.sl(G_SFX_TAB, a + 0x200);
        } else {
            self.g.sl(G_STEPS, a + 0x800);
            self.g.sl(G_PATTERNS, a + 0x400);
            self.g.sl(G_MACROS, a + 0x600);
        }
        self.p.sw(P_SPEED, 5);
        let song = self.song_of(sub);
        self.start_song(song, hw);
        if self.sfx_module {
            self.play_sfx(sub, hw);
        }
    }

    /// The default timer (14187), as 50/60 of it for a module whose flag says so.
    fn timer(&self) -> u16 {
        // InitSound reads the module's byte $B
        let flag = self.mdat_flag;
        let t = self.timer0;
        if flag & 2 != 0 {
            divu(t as u32 * 50, 60)
        } else {
            t
        }
    }

    /// $11260: the subsong's song, from InitPlayer's list (and the bytes after
    /// it, for a sound effects module with more effects than the list holds).
    fn song_of(&self, sub: u16) -> u8 {
        self.songs.get(sub as usize).copied().unwrap_or(0)
    }

    /// $12a84: all voices off (EndSound).
    fn all_off(&mut self, hw: &mut Hw) {
        self.g.sb(G_ON, 0);
        self.g.sw(G_DMA_ON, 0);
        for v in &mut self.v {
            v.sb(V_RUN, 0);
            v.sb(V_F91, 0);
            v.sl(V_SFX, 0);
            v.sl(V_PEND, 0);
            v.sw(V_SID, 0);
            v.sb(V_RND, 0);
        }
        if self.tfhd {
            // the TFHD player: two groups of volume zeros, then the interrupts off, then
            // the volumes again, at the spacing its own code has (measured, a few hundred
            // colour clocks of it vary from run to run)
            for g in 0..2 {
                for (ch, gap) in [0u64, 139, 150, 143].into_iter().enumerate() {
                    self.spent += gap;
                    self.reg(hw, 0xdff0a8 + 16 * ch as u32, 0);
                }
                if g == 0 {
                    self.spent += TFHD_PRO_GROUP_GAP;
                }
            }
            self.spent += TFHD_PRO_DMA_GAP;
            self.dmacon(hw, 0x000f);
            self.spent += 12;
            self.intreq(hw, 0x0780);
            self.spent += 6;
            self.intena(hw, 0x0780);
            self.spent += 4;
            self.intreq(hw, 0x0780);
            for ch in 0..4 {
                self.spent += 8;
                self.reg(hw, 0xdff0a8 + 16 * ch, 0);
            }
            return;
        }
        for ch in 0..4 {
            self.reg(hw, 0xdff0a8 + 16 * ch, 0);
        }
        self.dmacon(hw, 0x000f);
        self.intreq(hw, 0x0780);
        self.intena(hw, 0x0780);
        self.intreq(hw, 0x0780);
    }

    /// The TFHD player at load: the audio interrupts off, then a wait before InitSound.
    fn tfhd_load(&mut self, hw: &mut Hw) {
        self.intena(hw, 0x0780);
        self.spent += TFHD_PRO_LOAD_GAP;
    }

    /// The end of the TFHD player's start (after `init_sound`): the timer starts.
    fn tfhd_started(&mut self, hw: &mut Hw) {
        self.spent += TFHD_PRO_TIMER_GAP;
        self.flush(hw, "s");
    }

    /// $12b22: starts song `song`.
    fn start_song(&mut self, song: u8, hw: &mut Hw) {
        self.g.sb(G_START + 1, song);
        self.all_off(hw);
        self.g.sb(G_ON, 0);
        self.g.sw(G_SFX7, 0);
        let a4 = self.mdat + 2 * (song as u32 & 0x1f);
        self.g.sw(G_CIA, self.timer());
        let first = hw.bus.r16(a4 + 0x100);
        self.p.sw(P_POS, first);
        self.p.sw(P_FIRST, first);
        self.p.sw(P_LAST, hw.bus.r16(a4 + 0x140));
        let mut d2 = hw.bus.r16(a4 + 0x180);
        if d2 > 0xf {
            self.g.sw(G_CIA, divu(0x1b51f8, d2));
            d2 = self.p.w(P_SPEED);
        }
        for k in 0..8 {
            self.p.sl(P_PTR + 4 * k, self.empty);
            self.p.sw(P_NUM + 4 * k, 0xff00);
            self.p.sl(P_STEP + 4 * k, 0);
        }
        self.p.sw(P_SPEED, d2);
        if (song as i8) >= 0 {
            self.track_step(hw);
        }
        self.g.sb(G_RESTART, 0);
        self.g.sw(G_SPEED_CNT, 0);
        self.g.sb(G_TRACK_LOOP, 0xff);
        self.g.sb(G_SONG, song);
        self.g.sb(G_START, 0);
        self.g.sw(G_DMA_ON, 0);
        self.fade_lock = 0;
        if self.tfhd {
            // the TFHD player sets the master volume to $3F (and stops any fade the
            // first track steps began) after it has read them, then switches the audio
            // interrupt line on, the filter off and the volumes to 0
            self.g.sb(G_MASTER, 0x3f);
            self.g.sb(G_FADE_END, 0x3f);
            self.g.sb(G_FADE_DIR, 0);
            self.spent += TFHD_PRO_INTENA_GAP;
            self.intena(hw, 0xa000);
            self.spent += TFHD_PRO_FILTER_GAP;
        }
        // the filter off, no modulation
        hw.bus.ciaa.pra |= 2;
        if self.tfhd {
            self.spent += 10;
        }
        self.reg(hw, 0xdff09e, 0x00ff);
        if self.tfhd {
            self.spent += TFHD_PRO_VOL_GAP;
            for (ch, gap) in [0u64, 139, 150, 143].into_iter().enumerate() {
                self.spent += gap;
                self.reg(hw, 0xdff0a8 + 16 * ch as u32, 0);
            }
        }
        self.g.sb(G_ON, 1);
    }

    /// $129c8: starts sound effect `n` of the module's list.
    fn play_sfx(&mut self, n: u16, hw: &mut Hw) {
        let a5 = if hw.bus.r32(self.mdat + 0x1d0) == 0 {
            self.mdat.wrapping_add(hw.bus.r32(self.mdat + 0x5fc))
        } else {
            self.g.l(G_SFX_TAB)
        };
        let d2 = (n << 3) as u32;
        let e = a5.wrapping_add(d2 as u16 as i16 as u32);
        if hw.bus.r8(e) == 0xfb {
            // a pattern on the eighth track
            let d0 = hw.bus.r16(e + 2);
            self.g.sw(G_SFX7, 1);
            self.p.sw(P_NUM + 28, d0);
            let pa = self.pattern_addr(hw, ((d0 & 0xff00) >> 8) as u32);
            self.p.sl(P_PTR + 28, pa);
            self.p.sl(P_STEP + 28, 0);
            self.p.sb(P_LOOP + 28, 0);
            return;
        }
        let mut d3 = hw.bus.r8(e + 2);
        if (self.g.b(G_SONG) as i8) < 0 {
            d3 = hw.bus.r8(e + 4);
        }
        let i = (d3 & 3) as usize;
        let d1 = hw.bus.r8(e + 5) & 0x7f;
        let v = &self.v[i];
        let locked = (v.w(V_LOCK) as i16) >= 0;
        if d1 < v.b(V_PRIO) && locked {
            return;
        }
        if d2 as u8 == v.b(V_SFX_NUM) && locked && hw.bus.r8(e + 5) & 0x80 != 0 {
            return;
        }
        let d0 = (hw.bus.r32(e) & 0xffff_f0ff) | ((d3 as u32 & 0xf) << 8);
        let v = &mut self.v[i];
        v.sl(V_PEND, d0);
        v.sb(V_PRIO, d1);
        v.sw(V_LOCK, hw.bus.r16(e + 6));
        v.sb(V_SFX_NUM, d2 as u8);
    }

    /// $1188c: reads the track step at the current position: commands, then
    /// the eight patterns.
    fn track_step(&mut self, hw: &mut Hw) {
        for _ in 0..0x10000 {
            self.at(At::TrackStep);
            let a0 = self.g.l(G_STEPS).wrapping_add((self.p.w(P_POS) << 4) as i16 as u32);
            let d0 = hw.bus.r16(a0);
            if d0 == 0xeffe {
                let c = hw.bus.r16(a0 + 2).wrapping_mul(4);
                let a0 = a0 + 4;
                self.c(["TS_C0", "TS_C1", "TS_C2", "TS_C3", "TS_C4"][if c < 0x14 { c / 4 } else { 0 } as usize]);
                match if c < 0x14 { c / 4 } else { 0 } {
                    0 => {
                        self.g.sb(G_ON, 0);
                        self.songend = true;
                        return;
                    }
                    1 => {
                        let c = self.g.w(G_TRACK_LOOP);
                        if c == 0 {
                            self.c("TS_LOOP_END");
                            self.g.sw(G_TRACK_LOOP, 0xffff);
                            self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                            continue;
                        }
                        if (c as i16) < 0 {
                            self.c("TS_LOOP_SET");
                            let n = hw.bus.r16(a0 + 2);
                            // the TFHD player signals the song end at the first pass
                            if (n as i16) <= 0 || self.tfhd {
                                self.songend = true;
                            }
                            self.g.sw(G_TRACK_LOOP, n.wrapping_sub(1));
                        } else {
                            self.g.sw(G_TRACK_LOOP, c - 1);
                        }
                        self.p.sw(P_POS, hw.bus.r16(a0));
                    }
                    2 => {
                        let s = hw.bus.r16(a0);
                        self.p.sw(P_SPEED, s);
                        self.g.sw(G_SPEED_CNT, s);
                        let t = hw.bus.r16(a0 + 2);
                        if (t as i16) >= 0 && t & 0x1ff != 0 {
                            self.c("TS_CIA");
                            self.g.sw(G_CIA, divu(0x1b51f8, t & 0x1ff));
                        }
                        self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                    }
                    _ => {
                        self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                        if self.fade_lock == 0 {
                            self.c("TS_FADE");
                            self.fade_lock = 1;
                            let (b1, b3) = (hw.bus.r8(a0 + 1), hw.bus.r8(a0 + 3));
                            self.fade(b1, b3);
                        }
                    }
                }
                continue;
            }
            for k in 0..8 {
                let w = hw.bus.r16(a0 + 2 * k as u32);
                if k == 7 && self.g.w(G_SFX7) != 0 {
                    self.c("TS_SFX7");
                    break;
                }
                let o = 4 * k;
                self.p.sw(P_NUM + o, w);
                if (w as i16) >= 0 {
                    self.c("TS_PAT");
                    let pa = self.pattern_addr(hw, (w >> 8) as u32);
                    self.p.sl(P_PTR + o, pa);
                    self.p.sl(P_STEP + o, 0);
                    self.p.sb(P_LOOP + o, 0);
                }
            }
            return;
        }
    }

    /// A fade to `end` every `speed` ticks (at once for 0).
    fn fade(&mut self, speed: u8, end: u8) {
        self.g.sb(G_FADE_END, end);
        self.g.sb(G_FADE_CNT, speed);
        self.g.sb(G_FADE_RESET, speed);
        self.c("FD");
        if speed == 0 {
            self.c("FD_NOW");
            self.g.sb(G_MASTER, end);
        } else {
            self.g.sb(G_FADE_DIR, 1);
            let m = self.g.b(G_MASTER);
            if m != end {
                self.c("FD_GO");
                if m > end {
                    self.c("FD_DOWN");
                    self.g.sb(G_FADE_DIR, 0xff);
                }
                return;
            }
        }
        self.g.sb(G_FADE_DIR, 0);
        self.fade_lock = 0;
    }

    /// $11588: the patterns, when the speed counter is up.
    fn tracks(&mut self, hw: &mut Hw) {
        self.at(At::Tracks);
        let c = self.g.w(G_SPEED_CNT).wrapping_sub(1);
        self.g.sw(G_SPEED_CNT, c);
        if (c as i16) >= 0 {
            self.c("TR_WAIT");
            return;
        }
        self.c("TR_ROWS");
        self.g.sw(G_SPEED_CNT, self.p.w(P_SPEED));
        for k in 0..8 {
            // the TFHD player has no track mute table
            let w = if self.tfhd { 0 } else { hw.bus.r16(self.mdat + 0x1c0 + 2 * k as u32) };
            self.p.sw(P_MUTE + 4 * k, w);
        }
        'again: for _ in 0..256 {
            self.c("TR_PASS");
            self.g.sb(G_RESTART, 0);
            for k in 0..8 {
                self.ctx = 10 + k as u8;
                self.pattern(hw, 4 * k);
                self.ctx = 9;
                if self.g.b(G_RESTART) != 0 {
                    self.c("TR_RESTART");
                    continue 'again;
                }
            }
            break;
        }
    }

    /// $11616: pattern `o / 4`'s row.
    fn pattern(&mut self, hw: &mut Hw, o: usize) {
        self.at(At::Row);
        let num = self.p.b(P_NUM + o);
        if num >= 0x90 {
            if num == 0xfe {
                self.c("ROW_FE");
                self.p.sb(P_NUM + o, 0xff);
                if self.p.w(P_MUTE + o) == 0 {
                    self.channel_off(hw, self.p.b(P_XPOSE + o));
                }
            } else {
                self.c("ROW_NONE");
                if num != 0xff {
                    self.c("ROW_NONE_X");
                }
            }
            return;
        }
        let w = self.p.b(P_WAIT + o);
        if w != 0 {
            self.c("ROW_WAIT");
            self.p.sb(P_WAIT + o, w - 1);
            return;
        }
        for _ in 0..0x10000 {
            let step = self.p.w(P_STEP + o);
            let a = self.p.l(P_PTR + o).wrapping_add(step.wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a);
            self.g.sl(G_CMD, cmd);
            let b0 = (cmd >> 24) as u8;
            let step_on = |s: &mut Self| s.p.sw(P_STEP + o, s.p.w(P_STEP + o).wrapping_add(1));
            if b0 < 0xf0 {
                self.at(At::Note);
                self.c(if (0x7f..0xc0).contains(&b0) {
                    "NOTE_WAIT"
                } else if b0 < 0xc0 {
                    "NOTE_PLAIN"
                } else {
                    "NOTE_PORTA"
                });
                if (0x7f..0xc0).contains(&b0) {
                    self.p.sb(P_WAIT + o, cmd as u8);
                    self.g.sb(G_CMD + 3, 0);
                }
                let mut d0 = b0.wrapping_add(self.p.b(P_XPOSE + o));
                if b0 < 0xc0 {
                    d0 &= 0x3f;
                }
                self.g.sb(G_CMD, d0);
                if self.p.w(P_MUTE + o) == 0 {
                    self.note_port(hw, self.g.l(G_CMD));
                } else {
                    self.c("NOTE_MUTED");
                }
                step_on(self);
                if (0x7f..0xc0).contains(&b0) {
                    return;
                }
                continue;
            }
            self.at(At::PatternCmd);
            const PC: [&str; 16] = [
                "P_F0", "P_F1", "P_F2", "P_F3", "P_F4", "P_F5", "P_F6", "P_F7", "P_F8", "P_F9", "P_FA", "P_FB", "P_FC",
                "P_FD", "P_FE", "P_FF",
            ];
            self.c(PC[(b0 & 0xf) as usize]);
            let (b1, b2, b3) = ((cmd >> 16) as u8, (cmd >> 8) as u8, cmd as u8);
            let w1 = cmd as u16;
            match b0 & 0xf {
                0 => {
                    self.p.sb(P_NUM + o, 0xff);
                    if self.p.w(P_POS) == self.p.w(P_LAST) {
                        self.c("P_F0_LAST");
                        self.p.sw(P_POS, self.p.w(P_FIRST));
                        self.songend = true;
                    } else {
                        self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                    }
                    self.track_step(hw);
                    self.g.sb(G_RESTART, 0xff);
                    return;
                }
                1 => {
                    let c = self.p.b(P_LOOP + o);
                    if c != 0 {
                        self.c("P_F1_DEC");
                        self.p.sb(P_LOOP + o, c - 1);
                        if c == 1 {
                            self.c("P_F1_END");
                            step_on(self);
                            continue;
                        }
                    } else {
                        self.c("P_F1_SET");
                        self.p.sb(P_LOOP + o, b1);
                    }
                    self.p.sw(P_STEP + o, w1);
                }
                2 | 8 => {
                    if b0 & 0xf == 8 {
                        self.p.sl(P_RET + o, self.p.l(P_PTR + o));
                        self.p.sw(P_RET_STEP + o, step);
                    }
                    self.p.sb(P_NUM + o, b1);
                    let pa = self.pattern_addr(hw, b1 as u32);
                    self.p.sl(P_PTR + o, pa);
                    self.p.sw(P_STEP + o, w1);
                }
                3 => {
                    self.p.sb(P_WAIT + o, b1);
                    step_on(self);
                    return;
                }
                4 | 14 => {
                    if b0 & 0xf == 14 {
                        self.g.sw(G_SFX7, 0);
                    }
                    self.p.sb(P_NUM + o, 0xff);
                    return;
                }
                5..=7 | 12 => {
                    if b0 & 0xf == 5 {
                        self.g.sb(G_CMD + 1, b1.wrapping_add(self.p.b(P_XPOSE + o)));
                    }
                    if self.p.w(P_MUTE + o) == 0 {
                        self.note_port(hw, self.g.l(G_CMD));
                    }
                    step_on(self);
                }
                9 => {
                    // the first pattern's return point, whichever pattern this is
                    self.p.sl(P_PTR, self.p.l(P_RET));
                    self.p.sw(P_STEP, self.p.w(P_RET_STEP));
                    step_on(self);
                }
                10 => {
                    if self.fade_lock == 0 {
                        self.fade_lock = 1;
                        self.fade(b1, b3);
                    }
                    step_on(self);
                }
                11 => {
                    let d1 = 4 * (b2 & 7) as usize;
                    self.p.sb(P_NUM + d1, b1);
                    self.p.sb(P_XPOSE + d1, b3);
                    let pa = self.pattern_addr(hw, (b1 & 0x7f) as u32);
                    self.p.sl(P_PTR + d1, pa);
                    self.p.sl(P_STEP + d1, 0);
                    self.p.sb(P_LOOP + d1, 0);
                    step_on(self);
                }
                13 => {
                    self.cues[(b1 & 3) as usize] = w1;
                    step_on(self);
                }
                _ => step_on(self),
            }
        }
    }

    /// $12956: a voice off ($FE pattern), unless a sound effect holds it.
    fn channel_off(&mut self, hw: &mut Hw, d0: u8) {
        let i = (d0 & 3) as usize;
        if self.v[i].b(V_SFX) == 0 {
            let (int_off, dma_off) = (self.v[i].w(V_INT_OFF), self.v[i].w(V_DMA_OFF));
            self.c("CO_INTENA");
            self.c(["CO_INTENA_PRE_0", "CO_INTENA_PRE_1", "CO_INTENA_PRE_2", "CO_INTENA_PRE_3"][i]);
            self.intena(hw, int_off);
            self.c(["CO_INTENA_POST_0", "CO_INTENA_POST_1", "CO_INTENA_POST_2", "CO_INTENA_POST_3"][i]);
            self.c("CO_DMA");
            self.c(["CO_DMA_PRE_0", "CO_DMA_PRE_1", "CO_DMA_PRE_2", "CO_DMA_PRE_3"][i]);
            self.dmacon(hw, dma_off);
            self.c(["CO_DMA_POST_0", "CO_DMA_POST_1", "CO_DMA_POST_2", "CO_DMA_POST_3"][i]);
            let v = &mut self.v[i];
            v.sb(V_RUN, 0);
            v.sw(V_SID, 0);
            v.sb(V_RND, 0);
        }
    }

    /// $127ea: a note, or a note command, for the voice its third byte names.
    fn note_port(&mut self, hw: &mut Hw, d0: u32) {
        let saved = self.g.l(G_CMD);
        self.g.sl(G_CMD, d0);
        let [b0, b1, b2, b3] = d0.to_be_bytes();
        let i = (b2 & 3) as usize;
        if b0 == 0xfc {
            self.c("NP_FC");
            self.v[i].sb(V_SFX, b1);
            self.v[i].sw(V_LOCK, b3 as u16);
        } else if self.v[i].b(V_SFX) != 0 {
            self.c("NP_SFX");
        } else {
            self.c(match b0 {
                0xf7 => "NP_F7",
                0xf6 => "NP_F6",
                0xf5 => "NP_F5",
                0xbf.. => "NP_PORTA",
                _ => "NP_NOTE",
            });
            if b0 == 0xf7 {
                let v = &mut self.v[i];
                v.sb(V_ENV + 3, b1);
                let t = (b2 >> 4) + 1;
                v.sb(V_ENV + 1, t);
                v.sb(V_ENV, t);
                v.sb(V_ENV + 2, b3);
            } else if b0 == 0xf6 {
                let v = &mut self.v[i];
                let d = b1 & 0xfe;
                v.sb(V_VIB_RESET, d);
                v.sb(V_VIB_TIME, d >> 1);
                v.sb(V_VIB_WIDTH, b3);
                v.sb(V_VIB_WIDTH + 1, 1);
                v.sw(V_VIB_OFS, 0);
            } else if b0 == 0xf5 {
                self.v[i].sb(V_KEY, 0);
            } else if b0 >= 0xbf {
                // portamento to the note
                let v = &mut self.v[i];
                v.sb(V_PORTA_RESET, b1);
                v.sb(V_PORTA_TIME, 1);
                if v.w(V_PORTA_RATE) == 0 {
                    v.sw(V_PORTA_PER, v.w(V_PER));
                    self.c("NP_PORTA_START");
                }
                let v = &mut self.v[i];
                v.sw(V_PORTA_RATE, b3 as u16);
                let n = b0 & 0x3f;
                v.sb(V_NOTE, n);
                v.sw(V_PER, PERIODS[n as usize]);
            } else {
                let ma = self.macro_addr(hw, b1 as u32);
                if ma != self.v[i].l(V_MACRO) {
                    self.c("NP_NEWMAC");
                }
                let v = &mut self.v[i];
                v.sw(V_FINE, b3 as i8 as u16);
                v.sb(V_VEL + 1, (b2 >> 4) & 0xf);
                v.sb(V_PREV, v.b(V_NOTE));
                v.sb(V_NOTE, b0);
                if ma != v.l(V_MACRO) {
                    v.sb(V_F91, 0);
                }
                v.sl(V_MACRO, ma);
                v.sw(V_STEP, 0);
                v.sw(V_WAIT, 0);
                v.sb(V_EFX, 0);
                v.sb(V_LOOP, 0);
                v.sb(V_RUN, 0xff);
                self.set_dma_count(i, 0);
                let m = self.v[i].w(V_INT_OFF);
                self.c("NP_INTENA");
                self.c(["NP_INTENA_PRE_0", "NP_INTENA_PRE_1", "NP_INTENA_PRE_2", "NP_INTENA_PRE_3"][i]);
                self.intena(hw, m);
                self.c(["NP_INTENA_POST_0", "NP_INTENA_POST_1", "NP_INTENA_POST_2", "NP_INTENA_POST_3"][i]);
                self.c("NP_INTREQ");
                self.c(["NP_INTREQ_PRE_0", "NP_INTREQ_PRE_1", "NP_INTREQ_PRE_2", "NP_INTREQ_PRE_3"][i]);
                self.intreq(hw, m);
                self.c(["NP_INTREQ_POST_0", "NP_INTREQ_POST_1", "NP_INTREQ_POST_2", "NP_INTREQ_POST_3"][i]);
                self.v[i].sb(V_KEY, 1);
            }
        }
        self.g.sl(G_CMD, saved);
    }

    /// $11ab0: voice `i`'s tick: a sound effect's note, the macro, the effects.
    fn voice(&mut self, hw: &mut Hw, i: usize) {
        self.ctx = i as u8;
        self.at(At::VoiceStart);
        self.c(["VOICE_0", "VOICE_1", "VOICE_2", "VOICE_3"][i]);
        if self.dma_flag[i] {
            self.c("IRQ_FLAG");
        }
        self.irq_flag(i);
        let lock = self.v[i].w(V_LOCK);
        if (lock as i16) >= 0 {
            self.c("LOCK_DEC");
            self.v[i].sw(V_LOCK, lock.wrapping_sub(1));
        } else {
            self.c("LOCK_NEG");
            self.v[i].sb(V_SFX, 0);
            self.v[i].sb(V_PRIO, 0);
        }
        let d0 = self.v[i].l(V_PEND);
        if d0 != 0 {
            self.at(At::Pending);
            self.v[i].sl(V_PEND, 0);
            self.v[i].sb(V_SFX, 0);
            self.note_port(hw, d0);
            self.v[i].sb(V_SFX, self.v[i].b(V_PRIO));
        }
        if self.v[i].b(V_RUN) != 0 {
            let w = self.v[i].w(V_WAIT);
            if w != 0 {
                self.c("WAIT_DEC");
                self.v[i].sw(V_WAIT, w - 1);
            } else {
                self.run_macro(hw, i);
            }
        } else {
            self.c("NOT_RUN");
        }
        self.effects(hw, i);
    }

    fn step_on(&mut self, i: usize) {
        let s = self.v[i].w(V_STEP).wrapping_add(1);
        self.v[i].sw(V_STEP, s);
    }

    /// $1204c: a note's period (`note` + the command's note, the finetune plus
    /// the command's word), out unless a portamento runs.
    fn note_period(&mut self, i: usize, note: u8) {
        let n = self.g.b(G_CMD + 1).wrapping_add(note) & 0x3f;
        let mut d0 = PERIODS[n as usize] as u32;
        let d1 = self.v[i].w(V_FINE).wrapping_add(self.g.w(G_CMD + 2));
        if d1 != 0 {
            self.c("NPER_FINE");
            d0 = (d0 * d1.wrapping_add(0x100) as u32) >> 8;
        }
        self.v[i].sw(V_PER, d0 as u16);
        if self.v[i].w(V_PORTA_RATE) == 0 {
            self.c("NPER_OUT");
            self.v[i].sw(V_PER_OUT, d0 as u16);
        }
    }

    /// $11c52: DMA off's volume: the third byte, or the velocity * 3 + the
    /// fourth, when either is not 0.
    fn off_volume(&mut self, hw: &mut Hw, i: usize) {
        let (b2, b3) = (self.g.b(G_CMD + 2), self.g.b(G_CMD + 3));
        let d0 = if b2 != 0 {
            self.c("OV_B2");
            b3 as u16
        } else if b3 != 0 {
            self.c("OV_B3");
            self.v[i].w(V_VEL).wrapping_mul(3).wrapping_add(self.g.w(G_CMD + 2))
        } else {
            self.c("OV_NONE");
            return;
        };
        self.v[i].sb(V_VOL, d0 as u8);
        self.master_volume(hw, i, false);
    }

    /// The volume out: scaled by the master volume when it is under 64 (and, at
    /// the end of a tick, when no sound effect holds the voice).
    fn master_volume(&mut self, hw: &mut Hw, i: usize, lock_test: bool) {
        let d1 = self.g.b(G_MASTER) as u16;
        let mut d0 = self.v[i].b(V_VOL) as u16;
        if lock_test && (self.v[i].w(V_LOCK) as i16) >= 0 {
            self.c("MV_LOCKED");
        } else if d1 & 0x40 == 0 {
            self.c(if lock_test { "MV_SCALE" } else { "MV_SCALE_OFF" });
            d0 = (d0.wrapping_mul(4) as u32 * d1 as u32) as u16 >> 8;
        } else {
            self.c(if lock_test { "MV_FULL" } else { "MV_FULL_OFF" });
        }
        self.at(At::Volume);
        self.c(["VOLUME_0", "VOLUME_1", "VOLUME_2", "VOLUME_3"][i]);
        self.vol(hw, i, d0);
    }

    /// The macro, from its step, until a command ends it for this tick.
    fn run_macro(&mut self, hw: &mut Hw, i: usize) {
        for _ in 0..0x10000 {
            self.at(At::MacroCmd);
            let a0 = self.v[i].l(V_MACRO).wrapping_add(self.v[i].w(V_STEP).wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a0);
            self.g.sl(G_CMD, cmd & 0x00ff_ffff);
            let op = (cmd >> 24) as u8;
            self.c(MACRO_NAMES.get(op as usize).copied().unwrap_or("M_BAD"));
            self.site = if op < 0x34 { op } else { 0xff };
            let next = self.macro_cmd(hw, i, op, a0);
            self.site = 0xff;
            match next {
                Next::Read => self.c("N_READ"),
                Next::Step => {
                    self.c("N_STEP");
                    self.step_on(i)
                }
                Next::Pause => {
                    self.step_on(i);
                    if self.v[i].b(V_OFF_DONE) != 0 {
                        self.c("N_PAUSE");
                        return;
                    }
                    self.c("N_PAUSE_OFF");
                    self.v[i].sb(V_OFF_DONE, 0xff);
                }
                Next::Effects => {
                    self.c("N_EFX");
                    return;
                }
            }
        }
    }

    fn macro_cmd(&mut self, hw: &mut Hw, i: usize, c: u8, a0: u32) -> Next {
        let cmd = self.g.l(G_CMD);
        let (b1, b2, b3) = ((cmd >> 16) as u8, (cmd >> 8) as u8, cmd as u8);
        let w1 = cmd as u16;
        let ch = i;
        match c {
            0x00 | 0x13 => {
                if c == 0 {
                    let v = &mut self.v[i];
                    v.sb(V_ENV, 0);
                    v.sb(V_VIB_RESET, 0);
                    v.sw(V_PORTA_RATE, 0);
                    v.sb(V_RND, 0);
                    v.sw(V_SID, 0);
                }
                self.step_on(i);
                if b1 != 0 {
                    self.c("M00_LATER");
                    // left to the start of the next tick
                    let m = self.g.w(G_DMA_OFF) | self.v[i].w(V_DMA_OFF);
                    self.g.sw(G_DMA_OFF, m);
                    self.v[i].sb(V_OFF_DONE, 0);
                    self.off_volume(hw, i);
                    return Next::Effects;
                }
                let m = self.v[i].w(V_DMA_OFF);
                self.c("M00_DMA");
                self.c(["M00_DMA_PRE_0", "M00_DMA_PRE_1", "M00_DMA_PRE_2", "M00_DMA_PRE_3"][i]);
                self.dmacon(hw, m);
                self.c(["M00_DMA_POST_0", "M00_DMA_POST_1", "M00_DMA_POST_2", "M00_DMA_POST_3"][i]);
                self.off_volume(hw, i);
                Next::Read
            }
            0x01 => {
                let m = self.v[i].w(V_INT_OFF);
                self.c("M01_INTENA");
                self.c(["M01_INTENA_PRE_0", "M01_INTENA_PRE_1", "M01_INTENA_PRE_2", "M01_INTENA_PRE_3"][i]);
                self.intena(hw, m);
                self.c(["M01_INTENA_POST_0", "M01_INTENA_POST_1", "M01_INTENA_POST_2", "M01_INTENA_POST_3"][i]);
                self.c("M01_INTREQ");
                self.c(["M01_INTREQ_PRE_0", "M01_INTREQ_PRE_1", "M01_INTREQ_PRE_2", "M01_INTREQ_PRE_3"][i]);
                self.intreq(hw, m);
                self.c(["M01_INTREQ_POST_0", "M01_INTREQ_POST_1", "M01_INTREQ_POST_2", "M01_INTREQ_POST_3"][i]);
                self.v[i].sb(V_EFX, b1);
                self.step_on(i);
                self.dma_wait(hw, At::WaitOn);
                let m = self.g.w(G_DMA_ON) | self.v[i].w(V_DMA_ON);
                self.g.sw(G_DMA_ON, m);
                Next::Read
            }
            0x02 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = cmd.wrapping_add(self.g.l(G_SMPL));
                self.set_begin(hw, i, a);
                Next::Read
            }
            0x11 => {
                self.v[i].sb(V_AB_TIME, b1);
                self.v[i].sb(V_AB_RESET, b1);
                let d1 = w1 as i16 as i32 as u32;
                self.v[i].sl(V_AB_DELTA, d1);
                let a = self.v[i].l(V_ADDR).wrapping_add(d1);
                if self.v[i].w(V_SID) == 0 {
                    self.set_begin(hw, i, a);
                } else {
                    self.c("M11_SID");
                    self.v[i].sl(V_ADDR, a);
                    self.v[i].sl(V_SID_SRC, a);
                    self.step_on(i);
                }
                Next::Read
            }
            0x12 => {
                let d1 = self.v[i].w(V_LEN).wrapping_add(w1);
                self.v[i].sw(V_LEN, d1);
                if self.v[i].w(V_SID) != 0 {
                    self.c("M12_SID");
                    self.v[i].sw(V_SID_MASK, d1);
                } else {
                    self.len(hw, ch, d1);
                }
                Next::Step
            }
            0x03 => {
                self.v[i].sw(V_LEN, w1);
                self.len(hw, ch, w1);
                Next::Step
            }
            0x04 => {
                if b1 & 1 != 0 {
                    if self.v[i].b(V_RND_WAIT) != 0 {
                        self.c("M04_RND_WAIT");
                        return Next::Effects;
                    }
                    self.c("M04_RND_GO");
                    self.v[i].sb(V_RND_WAIT, 1);
                    return Next::Step;
                }
                self.v[i].sw(V_WAIT, w1);
                Next::Pause
            }
            0x05 | 0x10 => {
                if c == 0x10 && self.v[i].b(V_KEY) == 0 {
                    self.c("M10_UP");
                    return Next::Step;
                }
                let l = self.v[i].b(V_LOOP);
                if l != 0 {
                    self.v[i].sb(V_LOOP, l - 1);
                    if l == 1 {
                        self.c("LOOP_END");
                        return Next::Step;
                    }
                    self.c("LOOP_DEC");
                } else {
                    self.c("LOOP_SET");
                    self.v[i].sb(V_LOOP, b1);
                }
                self.v[i].sw(V_STEP, w1);
                Next::Read
            }
            0x06 | 0x15 => {
                if c == 0x15 {
                    self.v[i].sl(V_RET, self.v[i].l(V_MACRO));
                    self.v[i].sw(V_RET_STEP, self.v[i].w(V_STEP));
                }
                let ma = self.macro_addr(hw, (b1 & 0x7f) as u32);
                let v = &mut self.v[i];
                v.sl(V_MACRO, ma);
                v.sw(V_STEP, w1);
                v.sb(V_LOOP, 0);
                v.sb(V_F91, 0);
                Next::Read
            }
            0x07 => {
                self.v[i].sb(V_RUN, 0);
                Next::Effects
            }
            0x08 | 0x09 | 0x1f => {
                let note = match c {
                    0x08 => self.v[i].b(V_NOTE),
                    0x09 => 0,
                    _ => self.v[i].b(V_PREV),
                };
                self.note_period(i, note);
                Next::Pause
            }
            0x0a => {
                let v = &mut self.v[i];
                v.sb(V_RND, 0);
                v.sw(V_SID, 0);
                v.sb(V_AB_TIME, 0);
                v.sb(V_ENV, 0);
                v.sb(V_VIB_RESET, 0);
                v.sw(V_PORTA_RATE, 0);
                Next::Step
            }
            0x0b => {
                let v = &mut self.v[i];
                v.sb(V_PORTA_RESET, b1);
                v.sb(V_PORTA_TIME, 1);
                if v.w(V_PORTA_RATE) == 0 {
                    v.sw(V_PORTA_PER, v.w(V_PER));
                    self.c("M0B_START");
                }
                self.v[i].sw(V_PORTA_RATE, w1);
                Next::Step
            }
            0x0c => {
                let v = &mut self.v[i];
                v.sb(V_VIB_RESET, b1);
                v.sb(V_VIB_TIME, b1 >> 1);
                v.sb(V_VIB_WIDTH, b3);
                v.sb(V_VIB_WIDTH + 1, 1);
                if v.w(V_PORTA_RATE) != 0 {
                    self.c("M0C_PORTA");
                    return Next::Step;
                }
                v.sw(V_PER_OUT, v.w(V_PER));
                v.sw(V_VIB_OFS, 0);
                Next::Step
            }
            0x0d | 0x0e => {
                if b2 == 0xfe {
                    self.c("M0D_NOTE");
                    // a note first, the command's second byte from the note
                    self.g.sw(G_CMD + 2, 0);
                    let n = self.v[i].b(V_NOTE);
                    self.note_period(i, n);
                    self.g.sb(G_CMD + 3, b3);
                }
                let v = if c == 0x0d {
                    self.v[i].w(V_VEL).wrapping_mul(3).wrapping_add(self.g.w(G_CMD + 2)) as u8
                } else {
                    self.g.b(G_CMD + 3)
                };
                self.v[i].sb(V_VOL, v);
                Next::Step
            }
            0x0f => {
                let v = &mut self.v[i];
                v.sb(V_ENV, b2);
                v.sb(V_ENV + 3, b1);
                v.sb(V_ENV + 1, b2);
                v.sb(V_ENV + 2, b3);
                Next::Step
            }
            0x14 => {
                if self.v[i].b(V_KEY) == 0 {
                    self.c("M14_UP");
                    return Next::Step;
                }
                let l = self.v[i].b(V_LOOP);
                if l != 0 {
                    self.v[i].sb(V_LOOP, l - 1);
                    if l == 1 {
                        self.c("LOOP_END");
                        return Next::Step;
                    }
                    self.c("LOOP_DEC");
                } else {
                    self.c("LOOP_SET");
                    self.v[i].sb(V_LOOP, b3);
                }
                Next::Effects
            }
            0x16 => {
                self.v[i].sl(V_MACRO, self.v[i].l(V_RET));
                self.v[i].sw(V_STEP, self.v[i].w(V_RET_STEP));
                Next::Step
            }
            0x17 => {
                self.v[i].sw(V_PER, w1);
                if self.v[i].w(V_PORTA_RATE) == 0 {
                    self.c("M17_OUT");
                    self.v[i].sw(V_PER_OUT, w1);
                }
                Next::Step
            }
            0x18 => {
                let a = self.v[i].l(V_ADDR).wrapping_add(cmd);
                self.v[i].sl(V_ADDR, a);
                self.loc(hw, ch, a);
                let l = self.v[i].w(V_LEN).wrapping_sub((cmd as u16) >> 1);
                self.v[i].sw(V_LEN, l);
                self.c("M18_LEN");
                self.len(hw, ch, l);
                Next::Step
            }
            0x19 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = self.g.l(G_SMPL);
                self.v[i].sl(V_ADDR, a);
                self.loc(hw, ch, a);
                self.v[i].sw(V_LEN, 1);
                self.c("M19_LEN");
                self.len(hw, ch, 1);
                Next::Step
            }
            0x1a => {
                self.set_dma_count(i, w1);
                self.v[i].sb(V_RUN, 0);
                let m = self.v[i].w(V_INT_ON);
                self.c(["M1A_INTENA_PRE_0", "M1A_INTENA_PRE_1", "M1A_INTENA_PRE_2", "M1A_INTENA_PRE_3"][i]);
                self.intena(hw, m);
                self.c(["M1A_INTENA_POST_0", "M1A_INTENA_POST_1", "M1A_INTENA_POST_2", "M1A_INTENA_POST_3"][i]);
                Next::Pause
            }
            0x1b => {
                self.v[i].sb(V_RND_MACRO, b1);
                self.v[i].sw(V_RND_SPEED, w1);
                self.v[i].sw(V_RND_COUNT, 0x101);
                self.random_play(hw, i);
                self.v[i].sb(V_RND_WAIT, 1);
                Next::Step
            }
            0x1c | 0x1d => {
                let x = self.v[i].b(if c == 0x1c { V_NOTE } else { V_VOL });
                if b1 >= x {
                    self.c("M1C_NO");
                    return Next::Step;
                }
                self.v[i].sw(V_STEP, w1);
                Next::Read
            }
            0x1e => {
                self.v[i].sb(V_RND_MASK, b1);
                Next::Step
            }
            0x20 => {
                self.cues[(b1 & 3) as usize] = w1;
                Next::Step
            }
            0x21 => {
                let d = ((self.v[i].b(V_NOTE) as u32) << 24)
                    | (cmd & 0x00ff_0000)
                    | (((b2 | (self.v[i].b(V_VEL + 1) << 4)) as u32) << 8)
                    | b3 as u32;
                self.g.sl(G_CMD, d);
                self.note_port(hw, d);
                Next::Step
            }
            0x22 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = cmd.wrapping_add(self.g.l(G_SMPL));
                self.v[i].sl(V_SID_SRC, a);
                self.v[i].sl(V_ADDR, a);
                let buf = self.g.l(G_SMPL).wrapping_add(self.v[i].l(V_SID_BUF));
                self.dma_wait(hw, At::WaitSid);
                self.loc(hw, ch, buf);
                Next::Step
            }
            0x23 => {
                let n = (cmd >> 16) as u16;
                let l = if n == 0 { 0x100 } else { n } >> 1;
                self.len(hw, ch, l);
                let v = &mut self.v[i];
                v.sw(V_SID, n.wrapping_sub(1) & 0xff);
                v.sw(V_SID_MASK, w1);
                v.sw(V_LEN, w1);
                Next::Step
            }
            0x24 => {
                self.v[i].sl(0x6c, cmd << 8);
                Next::Step
            }
            0x25 => {
                let v = &mut self.v[i];
                v.sw(0x70, (cmd >> 16) as u16);
                v.sw(0x72, (cmd >> 16) as u16);
                v.sw(0x74, w1);
                Next::Step
            }
            0x26 => {
                self.v[i].sl(0x78, cmd);
                Next::Step
            }
            0x27 => {
                let v = &mut self.v[i];
                v.sw(0x84, (cmd >> 16) as u16);
                v.sw(0x86, (cmd >> 16) as u16);
                v.sw(0x82, w1);
                Next::Step
            }
            0x28 => {
                let v = &mut self.v[i];
                v.sb(0x76, b3);
                v.sw(0x80, ((b2 as i8 as i16) << 4) as u16);
                v.sw(0x7c, (cmd >> 16) as u16);
                v.sw(0x7e, (cmd >> 16) as u16);
                Next::Step
            }
            0x29 => {
                self.step_on(i);
                let v = &mut self.v[i];
                v.sw(V_SID, 0);
                if b1 != 0 {
                    self.c("M29_CLEAR");
                    let v = &mut self.v[i];
                    for o in [0x6c, 0x78] {
                        v.sl(o, 0);
                    }
                    for o in [0x70, 0x72, 0x74, 0x84, 0x86, 0x82, 0x80, 0x7c, 0x7e] {
                        v.sw(o, 0);
                    }
                    v.sb(0x76, 0);
                    v.sb(0x88, b3);
                }
                Next::Read
            }
            0x2c => {
                let a = (0x80000u32.wrapping_add(w1 as i16 as u32)) & 0x7ffff;
                hw.bus.w8(a, b1);
                Next::Step
            }
            0x2d => {
                let a = (0x80000u32.wrapping_add(w1 as i16 as u32)) & 0x7ffff;
                if hw.bus.r8(a) != b1 {
                    self.c("M2D_NO");
                    return Next::Step;
                }
                let s = self.v[i].w(V_STEP).wrapping_add(2);
                self.v[i].sw(V_STEP, s);
                Next::Read
            }
            0x2e => {
                self.reg(hw, 0xdff000 + 2 * b1 as u32, w1);
                Next::Step
            }
            0x2f => {
                let a1 = self.macro_addr(hw, (b1 & 0x7f) as u32);
                let x = hw.bus.r32(a0 + 4);
                hw.bus.w32(a1.wrapping_add(w1.wrapping_mul(4) as i16 as u32), x);
                let s = self.v[i].w(V_STEP).wrapping_add(2);
                self.v[i].sw(V_STEP, s);
                Next::Read
            }
            0x30 => {
                if self.v[i].b(V_F91) == 0 {
                    self.c("M30_FIRST");
                    self.v[i].sb(V_F91, 0xff);
                    return Next::Step;
                }
                self.v[i].sw(V_STEP, w1);
                Next::Read
            }
            0x31 => {
                let d = (0xf5 << 24) | cmd;
                self.g.sl(G_CMD, d);
                self.note_port(hw, d);
                Next::Step
            }
            0x32 | 0x33 => {
                let a = a0.wrapping_add(((cmd >> 16) as u16).wrapping_mul(2) as i16 as u32);
                let x = hw.bus.r16(a);
                hw.bus.w16(a, if c == 0x32 { x.wrapping_add(w1) } else { x & w1 });
                Next::Step
            }
            0x2a | 0x2b => Next::Step,
            _ => Next::Pause,
        }
    }

    /// SetBegin's location, out after the original's wait for the DMA.
    fn set_begin(&mut self, hw: &mut Hw, i: usize, a: u32) {
        self.v[i].sl(V_ADDR, a);
        self.dma_wait(hw, At::WaitBegin);
        self.loc(hw, i, a);
        self.step_on(i);
    }
}

impl Pro {
    /// $12384: the voice's effects, then the random player, the fade and the
    /// volume.
    fn effects(&mut self, hw: &mut Hw, i: usize) {
        self.at(At::Effects);
        let e = self.v[i].b(V_EFX);
        if e & 0x80 != 0 {
            self.c("EFX_OFF");
            return self.fade_volume(hw, i);
        }
        if e == 0 {
            self.c("EFX_FIRST");
            self.v[i].sb(V_EFX, 1);
            return self.fade_volume(hw, i);
        }
        // AddBegin
        if self.v[i].b(V_AB_TIME) != 0 {
            self.at(At::AddBegin);
            let a = self.v[i].l(V_ADDR).wrapping_add(self.v[i].l(V_AB_DELTA));
            self.v[i].sl(V_ADDR, a);
            if self.v[i].w(V_SID) != 0 {
                self.c("AB_SID");
                self.v[i].sl(V_SID_SRC, a);
            } else {
                self.site = 0x34;
                self.loc(hw, i, a);
                self.site = 0xff;
            }
            let t = self.v[i].b(V_AB_TIME).wrapping_sub(1);
            self.v[i].sb(V_AB_TIME, t);
            if t == 0 {
                self.c("AB_FLIP");
                let v = &mut self.v[i];
                v.sb(V_AB_TIME, v.b(V_AB_RESET));
                v.sl(V_AB_DELTA, v.l(V_AB_DELTA).wrapping_neg());
            }
        }
        if self.v[i].w(V_SID) != 0 {
            self.sid(hw, i);
        } else {
            self.c("NO_SID");
        }
        // vibrato
        if self.v[i].b(V_VIB_RESET) != 0 {
            self.at(At::Vibrato);
            let v = &self.v[i];
            let o = v.w(V_VIB_OFS).wrapping_add(v.b(V_VIB_WIDTH) as i8 as u16);
            let (rate, t) = (v.w(V_PORTA_RATE), v.b(V_VIB_TIME).wrapping_sub(1));
            self.c(if o != 0 { "VIB_MUL" } else { "VIB_ZERO" });
            self.c(if rate == 0 { "VIB_OUT" } else { "VIB_PORTA" });
            if t == 0 {
                self.c("VIB_FLIP");
            }
            let v = &mut self.v[i];
            v.sw(V_VIB_OFS, o);
            let mut d0 = v.w(V_PER) as u32;
            if o != 0 {
                d0 = ((d0 * o.wrapping_add(0x800) as u32) << 5) >> 16;
            }
            if v.w(V_PORTA_RATE) == 0 {
                v.sw(V_PER_OUT, d0 as u16);
            }
            let t = v.b(V_VIB_TIME).wrapping_sub(1);
            v.sb(V_VIB_TIME, t);
            if t == 0 {
                v.sb(V_VIB_TIME, v.b(V_VIB_RESET));
                v.sb(V_VIB_WIDTH, (v.b(V_VIB_WIDTH) ^ 0xff).wrapping_add(1));
            }
        } else {
            self.c("NO_VIB");
        }
        // portamento
        if self.v[i].w(V_PORTA_RATE) != 0 {
            let v = &mut self.v[i];
            let t = v.b(V_PORTA_TIME).wrapping_sub(1);
            v.sb(V_PORTA_TIME, t);
            if t == 0 {
                self.at(At::Porta);
                let v = &self.v[i];
                let (d1, p0, rate) = (v.w(V_PER), v.w(V_PORTA_PER), v.w(V_PORTA_RATE));
                let mut d0 = p0 as u32;
                let (kind, reached) = if p0 == d1 {
                    ("PORTA_EQ", true)
                } else if p0 < d1 {
                    d0 = (d0 * rate.wrapping_add(0x100) as u32) >> 8;
                    ("PORTA_UP", d0 as u16 >= d1)
                } else {
                    d0 = (d0 * 0x100u16.wrapping_sub(rate) as u32) >> 8;
                    ("PORTA_DOWN", d0 as u16 <= d1)
                };
                self.c(kind);
                if reached {
                    self.c("PORTA_REACHED");
                    if kind == "PORTA_UP" {
                        self.c("PORTA_UP_REACHED");
                    }
                    if d0 as u16 == d1 {
                        self.c("PORTA_EXACT");
                    }
                }
                let v = &mut self.v[i];
                v.sb(V_PORTA_TIME, v.b(V_PORTA_RESET));
                if reached {
                    v.sw(V_PORTA_RATE, 0);
                    d0 = v.w(V_PER) as u32;
                }
                let d0 = d0 as u16 & 0x7ff;
                v.sw(V_PORTA_PER, d0);
                v.sw(V_PER_OUT, d0);
            } else {
                self.c("PORTA_WAIT");
            }
        } else {
            self.c("NO_PORTA");
        }
        // envelope
        if self.v[i].b(V_ENV) != 0 {
            self.at(At::Env);
            let c = self.v[i].b(V_ENV + 1);
            if c != 0 {
                self.c("ENV_WAIT");
                self.v[i].sb(V_ENV + 1, c - 1);
            } else {
                let v = &mut self.v[i];
                v.sb(V_ENV + 1, v.b(V_ENV));
                let end = v.b(V_ENV + 2) as i8;
                let rate = v.b(V_ENV + 3);
                let vol = v.b(V_VOL) as i8;
                let (kind, done, exact) = if end > vol {
                    let n = (vol as u8).wrapping_add(rate) as i8;
                    v.sb(V_VOL, n as u8);
                    ("ENV_UP", end <= n, end == n)
                } else {
                    let n = (vol as u8).wrapping_sub(rate) as i8;
                    v.sb(V_VOL, n as u8);
                    (if n < 0 { "ENV_DOWN_NEG" } else { "ENV_DOWN" }, n < 0 || end >= n, end == n)
                };
                if exact {
                    self.c("ENV_EXACT");
                }
                let v = &mut self.v[i];
                if done {
                    v.sb(V_VOL, end as u8);
                    v.sb(V_ENV, 0);
                }
                self.c(kind);
                if done {
                    self.c("ENV_DONE");
                    if kind == "ENV_UP" {
                        self.c("ENV_UP_DONE");
                    }
                }
            }
        } else {
            self.c("NO_ENV");
        }
        self.random_play(hw, i);
    }

    /// $123d2: the SID synthesis, into the voice's buffer in the sample memory.
    fn sid(&mut self, hw: &mut Hw, i: usize) {
        self.at(At::SidStart);
        let v = &self.v[i];
        let src = v.l(V_SID_SRC);
        let mut d4 = v.l(0x6c);
        let a3 = v.l(0x78);
        let mut a1 = v.l(V_SID_BUF).wrapping_add(self.g.l(G_SYNTH));
        let mut a2 = a1.wrapping_add(0x100);
        let n = v.w(V_SID) as u32 + 1;
        let mask = v.w(V_SID_MASK);
        let d3 = v.b(0x76);
        let d5 = v.b(0x88);
        let mut d0: u32 = 0;
        let mut d1 = v.b(V_SID_LAST);
        for _ in 0..n {
            self.at(At::SidByte);
            d4 = d4.wrapping_add(a3);
            let s = d0.rotate_left(16);
            let (sum, x) = s.overflowing_add(d4);
            d0 = sum.rotate_left(16);
            d0 = (d0 & 0xffff_0000) | (d0 as u16 & mask) as u32;
            let d2 = hw.bus.r8(src.wrapping_add(d0 as u16 as i16 as u32)) as i8;
            let out = if d3 == 0 {
                d2
            } else {
                let c = d1 as i8;
                if d2 == c {
                    d2
                } else if d2 > c {
                    let r = (c as i16) + (d3 as i8 as i16) + x as i16;
                    if !(-128..=127).contains(&r) {
                        d2
                    } else if d2 > r as i8 {
                        r as i8
                    } else {
                        d2
                    }
                } else {
                    let r = (c as i16) - (d3 as i8 as i16) - x as i16;
                    if !(-128..=127).contains(&r) || d2 >= r as i8 {
                        d2
                    } else {
                        r as i8
                    }
                }
            };
            d1 = out as u8;
            hw.bus.w8(a1, out as u8);
            a1 = a1.wrapping_add(1);
            if d5 != 0 {
                hw.bus.w8(a2, (out as u8).wrapping_neg());
                a2 = a2.wrapping_add(1);
            }
        }
        let v = &mut self.v[i];
        v.sb(V_SID_LAST, d1);
        if d3 != 0 {
            v.sw(0x76, v.w(0x76).wrapping_add(v.w(0x80)));
            let c = v.w(0x7c).wrapping_sub(1);
            v.sw(0x7c, c);
            if c == 0 {
                v.sw(0x7c, v.w(0x7e));
                v.sw(0x80, v.w(0x80).wrapping_neg());
            }
        }
        for (acc, delta, cnt, reset) in [(0x6c, 0x74, 0x70, 0x72), (0x78, 0x82, 0x84, 0x86)] {
            v.sl(acc, v.l(acc).wrapping_add(v.w(delta) as i16 as u32));
            let c = v.w(cnt).wrapping_sub(1);
            v.sw(cnt, c);
            if c == 0 {
                v.sw(cnt, v.w(reset));
                if v.w(reset) != 0 {
                    v.sw(delta, v.w(delta).wrapping_neg());
                }
            }
        }
    }

    /// The period of the random player's note, out unless a portamento runs.
    fn random_period(&mut self, i: usize, d0: u16) {
        self.v[i].sw(V_PER, d0);
        if self.v[i].w(V_PORTA_RATE) == 0 {
            self.v[i].sw(V_PER_OUT, d0);
        }
    }

    /// $126f6: a random position in the random player's macro, then the fade
    /// and the volume (the original reaches them through its tail).
    fn random_pos(&mut self, hw: &mut Hw, i: usize) {
        self.random(hw);
        let d1 = self.g.b(G_SEED + 1) & self.v[i].b(V_RND_MASK);
        self.v[i].sw(V_RND_STEP, d1 as u16);
        self.fade_volume(hw, i);
    }

    /// $125ba: the random player, then the fade and the volume.
    fn random_play(&mut self, hw: &mut Hw, i: usize) {
        let r = self.v[i].b(V_RND);
        if r == 0 {
            self.c("NO_RND");
            return self.fade_volume(hw, i);
        }
        self.at(At::Random);
        if r & 0x80 == 0 {
            let n = (self.v[i].b(V_RND_MACRO) & 0x7f) as u32;
            let a = self.macro_addr(hw, n);
            self.v[i].sl(V_RND_PTR, a);
            self.v[i].sw(V_RND_STEP, 0);
            self.v[i].sb(V_RND, 0xff);
            if self.v[i].b(V_RND_MODE) & 1 != 0 {
                self.random_pos(hw, i);
            }
        }
        let c = self.v[i].b(V_RND_COUNT).wrapping_sub(1);
        self.v[i].sb(V_RND_COUNT, c);
        if c != 0 {
            // $1270c: pass the period and a lower volume on to the next voice
            if self.v[i].b(V_RND_MODE) & 2 != 0 {
                let d0 = (self.v[i].b(V_RND_SPEED) as u16 * 3) >> 3;
                if d0 as u8 == self.v[i].b(V_RND_COUNT) {
                    let per = self.v[i].w(V_PER);
                    let vol = ((self.v[i].b(V_VOL) as u16 * 5) >> 3) as u8;
                    let j = (i + 1) % 4;
                    self.v[j].sb(V_VOL, vol);
                    if per != self.v[j].w(V_PER) {
                        self.v[j].sw(V_PER, per);
                        self.v[j].sw(V_PER_OUT, per);
                        if self.g.b(G_CMD) & 0x80 != 0 {
                            self.v[j].sb(V_RND_WAIT, 0);
                        }
                    }
                }
            }
            return self.fade_volume(hw, i);
        }
        self.v[i].sb(V_RND_COUNT, self.v[i].b(V_RND_SPEED));
        let mut d0;
        loop {
            let s = self.v[i].w(V_RND_STEP);
            d0 = hw.bus.r8(self.v[i].l(V_RND_PTR).wrapping_add(s as i16 as u32));
            self.g.sb(G_CMD, d0);
            if d0 != 0 {
                break;
            }
            if s == 0 {
                return self.fade_volume(hw, i);
            }
            self.v[i].sw(V_RND_STEP, 0);
        }
        let n = d0.wrapping_add(self.v[i].b(V_NOTE)) & 0x3f;
        if n == 0 {
            return self.random_pos(hw, i);
        }
        let mut per = PERIODS[n as usize] as u32;
        let f = self.v[i].w(V_FINE);
        if f != 0 {
            per = (per * f.wrapping_add(0x100) as u32) >> 8;
        }
        let per = per as u16;
        if self.v[i].b(V_RND_MODE) & 1 == 0 {
            self.v[i].sw(V_PER, per);
            if self.v[i].w(V_PORTA_RATE) != 0 {
                return self.fade_volume(hw, i);
            }
            self.v[i].sw(V_PER_OUT, per);
            if self.g.b(G_CMD) & 0x80 != 0 {
                self.v[i].sb(V_RND_WAIT, 0);
            }
            let s = self.v[i].w(V_RND_STEP).wrapping_add(1);
            self.v[i].sw(V_RND_STEP, s);
            return self.fade_volume(hw, i);
        }
        self.random(hw);
        let skip = self.v[i].b(V_RND_MODE) & 4 == 0
            && self.v[i].w(V_RND_STEP) & 3 == 0
            && 0x10 >= self.g.b(G_SEED + 1);
        if !skip {
            if self.g.b(G_CMD) & 0x80 != 0 {
                self.v[i].sb(V_RND_WAIT, 0);
            }
            self.random_period(i, per);
        }
        let s = self.v[i].w(V_RND_STEP).wrapping_add(1);
        self.v[i].sw(V_RND_STEP, s);
        if self.g.b(G_CMD) & 0x40 == 0 {
            return self.fade_volume(hw, i);
        }
        self.random(hw);
        if 6 >= self.g.b(G_SEED) {
            return self.fade_volume(hw, i);
        }
        self.random_pos(hw, i)
    }

    /// $12776: the fade's step, then the voice's volume.
    fn fade_volume(&mut self, hw: &mut Hw, i: usize) {
        self.at(At::Fade);
        let dir = self.g.b(G_FADE_DIR);
        if dir != 0 {
            self.c("FADE_DIR");
            let c = self.g.b(G_FADE_CNT).wrapping_sub(1);
            self.g.sb(G_FADE_CNT, c);
            if c == 0 {
                self.c("FADE_STEP");
                self.g.sb(G_FADE_CNT, self.g.b(G_FADE_RESET));
                let m = self.g.b(G_MASTER).wrapping_add(dir);
                self.g.sb(G_MASTER, m);
                if self.g.b(G_FADE_END) == m {
                    self.c("FADE_END");
                    self.g.sb(G_FADE_DIR, 0);
                    self.fade_lock = 0;
                }
            }
        }
        self.master_volume(hw, i, true);
    }

    /// $114c8 (with the Eagleplayer's song end test before it).
    fn interrupt(&mut self, hw: &mut Hw) {
        self.flush(hw, "t");
        if !self.tfhd {
            self.in_tick = true;
            self.seg = hw.bus.clock;
            self.cyc = 0;
        }
        self.at(At::Tick);
        if !self.tfhd && !self.sfx_module {
            let mut all = true;
            for k in 0..8 {
                self.c("SE_TRACK");
                if self.p.b(P_NUM + 4 * k) != 0xff {
                    all = false;
                    break;
                }
            }
            if all {
                self.c("SE_ALL");
                self.songend = true;
            }
        }
        let saved = self.g.l(G_CMD);
        let off = self.g.w(G_DMA_OFF);
        if off != 0 {
            self.at(At::OffStart);
            self.dmacon(hw, off);
            for ch in 0..4 {
                if off & 1 << ch != 0 {
                    self.at(At::OffPer);
                    self.per(hw, ch, 9);
                } else {
                    self.c("OFF_SKIP");
                }
            }
            self.g.sw(G_DMA_OFF, 0);
        } else {
            self.c("NO_OFF");
        }
        if self.g.b(G_ON) != 0 {
            for i in 0..4 {
                self.voice(hw, i);
            }
            self.ctx = 9;
            if (self.g.b(G_SONG) as i8) >= 0 {
                self.tracks(hw);
            } else {
                self.c("NO_TRACKS");
            }
            for ch in 0..4 {
                self.at(At::PerOut);
                let p = self.v[ch].w(V_PER_OUT);
                self.per(hw, ch, p);
            }
            self.dma_wait(hw, At::WaitEnd);
            let m = self.g.w(G_DMA_ON);
            self.c("END_DMA");
            self.dmacon(hw, m);
            self.g.sw(G_DMA_ON, 0);
        }
        self.g.sl(G_CMD, saved);
        self.flush(hw, "e");
        self.in_tick = false;
    }
}

impl Replayer for Pro {
    fn format(&self) -> &'static str {
        "TFMX Pro"
    }

    /// Wait on DMA's audio interrupt, which the original's per-voice handler
    /// takes: it clears the request, counts the voice's word down, and when the
    /// count goes negative sets the voice's flag (the macro runs at the next
    /// tick) and disables the interrupt. The handler's 68000 instructions take
    /// their time (20 cycles for each move to a custom register, 12 for the
    /// subq, 10 or 8 for the branch, 16 for the st and the rts), 2 per colour
    /// clock, and the two writes are made when their instructions end.
    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        let bit = 0x80u16 << ch;
        hw.wait(10);
        self.trace_write(hw);
        hw.bus.write_word(0xdff09c, bit);
        hw.wait(6);
        self.dma_count[ch] = self.dma_count[ch].wrapping_sub(1);
        if (self.dma_count[ch] as i16) >= 0 {
            hw.wait(5 + 8);
        } else {
            hw.wait(4 + 8);
            self.dma_flag[ch] = true;
            hw.wait(10);
            self.trace_write(hw);
            hw.bus.write_word(0xdff09a, bit);
            hw.wait(8);
        }
        true
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.interrupt(hw);
    }

    fn tick_clocks(&self) -> u64 {
        (self.g.w(G_CIA) as u64 + 1) * 5
    }

    /// The TFHD player's first tick comes 79 037 colour clocks after its timer starts.
    fn timer_delay(&self) -> u64 {
        if self.tfhd {
            TFHD_PRO_FIRST_TICK.saturating_sub(self.tick_clocks() + TFHD_PRO_FIRST_WRITE)
        } else {
            0
        }
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, (self.songs.len() as u16).saturating_sub(1), self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.all_off(hw);
        self.init_sound(n, hw);
    }
}

// ---------------------------------------------------------------------------
// TFMX 1.5: the Eagleplayer TFMX_1.5 ("TFMX 1.6 player module V1.91"), an
// older replayer of its own: fixed tables at $400/$600/$800, 26 macro
// commands, and its Paula writes through the ENPP_Poke functions.

pub const OPEN_15: Open = Open { name: "TFMX 1.5", detect: |d| detect_15(d) || detect_early_text(d), new: new_15 };

/// CHECK5: "TFMX" not followed by "SONG" (the TFHD single file, which it also
/// takes, is `tfhd.rs`'s). A raw Jochen Hippel ST module also starts with "TFMX", then the
/// count of its pitch programs as a word below $200 (the ST replayer's own check); a TFMX
/// song has a separator and "SONG" there, a word of $2000 and up. Those files are not TFMX.
pub(super) fn detect_15(d: &[u8]) -> bool {
    d.len() >= 0x800 && be32(d, 0) == 0x54464d58 && &d[5..9] != b"SONG" && be16(d, 4) >= 0x200
}

// The global block ($11614).
const H_MDAT: usize = 0x00;
const H_SMPL: usize = 0x04;
const H_ON: usize = 0x08;
const H_RESTART: usize = 0x0a;
const H_SONG: usize = 0x0c;
const H_MASTER: usize = 0x18;
const H_FADE_END: usize = 0x19;
const H_FADE_CNT: usize = 0x1a;
const H_FADE_RESET: usize = 0x1b;
const H_FADE_DIR: usize = 0x1c;
const H_CMD: usize = 0x28;
const H_SPEED_CNT: usize = 0x2c;
const H_PLAYING: usize = 0x2e;
const H_START: usize = 0x3c; // the song to start, or negative
const H_HALF: usize = 0x3e; // a song of tempo over 15: every other row
const H_SFX7: usize = 0x40;
const H_NOTE_WAIT: usize = 0x44;

// The voice area ($1165a): the voices' fields interleaved, 4 bytes apart.
const W_STATE: usize = 0x02; // 0 off, >0 a new note, <0 the macro runs
const W_NOTE: usize = 0x11;
const W_MACRO_NUM: usize = 0x13;
const W_VEL: usize = 0x20;
const W_FINE: usize = 0x22; // the finetune is its low byte
const W_MACRO: usize = 0x30;
const W_STEP: usize = 0x40;
const W_WAIT: usize = 0x42;
const W_DMA_ON: usize = 0x50;
const W_DMA_OFF: usize = 0x52;
const W_VOL: usize = 0x60;
const W_LOOP: usize = 0x62;
const W_ENV: usize = 0x70; // speed, counter, end volume, rate
const W_VIB_WIDTH: usize = 0x80;
const W_PORTA_RESET: usize = 0x82;
const W_PORTA_TIME: usize = 0x83;
const W_VIB_OFS: usize = 0x90;
const W_VIB_RESET: usize = 0x92;
const W_VIB_TIME: usize = 0x93;
const W_PER: usize = 0xa0;
const W_ADDR: usize = 0xb0;
const W_PORTA_RATE: usize = 0xc0;
const W_PORTA_PER: usize = 0xc2;
const W_LEN: usize = 0xd0;
const W_KEY: usize = 0xd2;
const W_RET: usize = 0xe0;
const W_RET_STEP: usize = 0xf0;
const W_LOCK: usize = 0x100;

/// The original's empty pattern pointer: it loads the pattern's first long
/// ($F4000000), not its address.
const EMPTY_15: u32 = 0xf400_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
enum B {
    Tick,
    Start,
    Tracks,
    Row,
    PatternCmd,
    Note,
    TrackStep,
    Voice,
    Effects,
    Vibrato,
    Porta,
    Env,
    Fade,
    Volume,
    NewNote,
    MacroCmd,
    DmaOn,
    Count,
}

/// An early module's colour clocks per code path (the Eagleplayer's tick is `COSTS15`):
/// fitted by least squares to the Eagleplayer's write times (18 modules, 120000
/// gaps); the time from the interrupt to the first row or voice (Tick, with
/// Tracks) cannot be told from the reference's timer phase and is estimated.
const COST15: [u64; B::Count as usize] = [150, 0, 0, 62, 134, 377, 375, 58, 0, 142, 187, 80, 277, 0, 63, 205, 106];

/// The macro commands' names, for `COSTS15` (the commands past $19 do nothing but end the tick).
const M15_NAMES: [&str; 0x1b] = ["M_00", "M_01", "M_02", "M_03", "M_04", "M_05", "M_06", "M_07", "M_08", "M_09", "M_0A", "M_0B", "M_0C", "M_0D", "M_0E", "M_0F", "M_10", "M_11", "M_12", "M_13", "M_14", "M_15", "M_16", "M_17", "M_18", "M_19", "M_XX"];
/// The pattern commands' names, for `COSTS15`.
const P_NAMES: [&str; 16] = ["P_F0", "P_F1", "P_F2", "P_F3", "P_F4", "P_F5", "P_F6", "P_F7", "P_F8", "P_F9", "P_FA", "P_FB", "P_FC", "P_FD", "P_FE", "P_FF"];

/// Colour clocks from a tick's end to the next tick's start when that interrupt is already
/// due (measured: a modified copy whose first rows run past the next interrupt).
const TICK_EXIT: u64 = 109;

/// 68000 cycles of the Eagleplayer's tick, by code path (sorted; a name not listed costs 0). Each
/// host call (ENPP_Poke*, ENPP_DMAMask) lands half the cycles counted since the last one returned
/// (or since the interrupt) later, rounded down.
const COSTS15: &[(&str, i64)] = &[
    ("CALL_ADR", 202),
    ("CALL_DMA", 149),
    ("CALL_LEN", 278),
    ("CALL_PER", 174),
    ("CALL_SONGEND", 25),
    ("ENV", 123),
    ("ENV_DONE", 36),
    ("ENV_DOWN", 11),
    ("ENV_NEG", -21),
    ("ENV_WAIT", 41),
    ("FADE", 113),
    ("FADE_DONE", 31),
    ("FADE_DOWN", 26),
    ("FADE_NOW", -29),
    ("LOOP_DEC", 33),
    ("LOOP_FF", 35),
    ("M14_DEC", 103),
    ("M14_FF", 105),
    ("MACRO", 221),
    ("M_00", 79),
    ("M_01", 44),
    ("M_02", 22),
    ("M_04", 90),
    ("M_05", 17),
    ("M_06_GO", 137),
    ("M_07", 60),
    ("M_08", 189),
    ("M_08_OUT", 16),
    ("M_08_POST", 90),
    ("M_09", 157),
    ("M_09_POST", 90),
    ("M_0A", 28),
    ("M_0B", 56),
    ("M_0C", 81),
    ("M_0C_OUT", 118),
    ("M_0C_POST", 16),
    ("M_0D", 36),
    ("M_0F", 60),
    ("M_10", 40),
    ("M_11", 16),
    ("M_12", 12),
    ("M_13", 31),
    ("M_14", 40),
    ("M_15", 48),
    ("M_16", 28),
    ("M_17_OUT", 131),
    ("M_18", 52),
    ("M_19_MID", 8),
    ("M_WAIT", 116),
    ("NP", 387),
    ("NP_NOTE", 242),
    ("NP_PORTA", 171),
    ("PORTA", 186),
    ("PORTA_DOWN", 39),
    ("PORTA_HIT", 19),
    ("PORTA_OFF", 100),
    ("PORTA_REACHED", 28),
    ("PORTA_UP", 49),
    ("PORTA_UP_REACHED", 18),
    ("PORTA_WAIT", 129),
    ("P_CMD", 199),
    ("P_F0_SONGEND", 61),
    ("P_F1", 47),
    ("P_F1_DEC", 29),
    ("P_F1_FF", 31),
    ("P_F2", 100),
    ("P_F3", 30),
    ("P_F5_ON", 28),
    ("P_F6", 63),
    ("P_F7", 6),
    ("P_FA", 31),
    ("P_FA_FADE", 32),
    ("P_FE", 16),
    ("RET_ADR", 88),
    ("RET_DMA", 20),
    ("RET_LEN", 12),
    ("ROW", 99),
    ("ROW_FE_DMA", 9),
    ("ROW_FE_POST", 179),
    ("ROW_OFF", 14),
    ("ROW_WAIT", 30),
    ("TRACKS", 432),
    ("TR_HALF", 23),
    ("TR_HALF_SKIP", 94),
    ("TR_PASS", 35),
    ("TR_RESTART", 176),
    ("TS", 222),
    ("TS_E1", 20),
    ("TS_E1_SONGEND", 11),
    ("TS_E2", 16),
    ("TS_EBIG", 4),
    ("TS_FADE_GO", 33),
    ("TS_K", 15),
    ("TS_K_SET", 108),
    ("VIB", 106),
    ("VIB_FLIP", 60),
    ("VOICE", 62),
    ("V_FADE_END", 36),
    ("V_FADE_ON", 25),
    ("V_FADE_STEP", 81),
    ("V_FX", 9),
    ("V_LOCK_DEC", 14),
    ("V_NOFX", 45),
    ("V_OFF", 70),
    ("V_VOL", 363),
    ("V_VOL_POST", 15),
    ("V_VOL_SCALE", 94),
];
/// The cycles a name costs more (or less) in one context: 0-3 voice `i`'s code, 10-17 track `k - 10`'s.
const COSTS15_CTX: &[(&str, u8, i64)] = &[
    ("CALL_LEN", 0, -23),
    ("CALL_LEN", 2, 23),
    ("CALL_PER", 3, 13),
    ("CALL_VOL", 2, 23),
    ("M_08_OUT", 0, -23),
    ("M_08_OUT", 2, 23),
    ("M_0C", 2, 23),
    ("M_0C_OUT", 0, -23),
    ("M_17", 2, 23),
    ("M_17_OUT", 0, -23),
    ("RET_ADR", 0, -23),
    ("RET_ADR", 2, 23),
    ("RET_ADR", 3, 26),
    ("VIB_OUT", 3, -13),
    ("V_FADE", 0, -23),
    ("V_VOL", 3, 13),
];

pub struct V15 {
    /// An early module (`new_early`): no fade to $3F at the start, as UADE's player.
    early: bool,
    g: Blk<0x46>,
    w: Blk<0x120>,
    p: Blk<0xc8>,
    fade_lock: u16,
    dma_on: u16,
    mdat: u32,
    smpl: u32,
    subs: u16,
    sub: u16,
    songend: bool,
    spent: u64,
    counts: [u32; B::Count as usize],
    trace: Option<std::fs::File>,
    /// In the Eagleplayer's tick (not an early module's): the clock of the last host call (or
    /// the interrupt) and the 68000 cycles its code has run since (`COSTS15`).
    in_tick: bool,
    seg: u64,
    cyc: i64,
    names: Vec<(&'static str, u32, u8)>,
    /// Whose code runs: 0-3 a voice's, 10-17 a track's, 9 neither.
    ctx: u8,
    /// The earliest clock the next tick's code can start: a tick that runs past the next
    /// interrupt delays it until the host has left the interrupt (TICK_EXIT colour clocks).
    next_tick: u64,
}

/// An early `TFMX-SONG ` module of the fixed tables (nothing at $1D0) whose text starts right
/// after the header, at $0A (`TFMX-SONG by ERN0/SB ...`): the Pro check refuses it (a word at $0C)
/// and CHECK5 any "SONG"; UADE's TFMX player plays it as TFMX 1.5 (`new_early`'s engine).
fn detect_early_text(d: &[u8]) -> bool {
    d.len() >= 0x800 && d.starts_with(b"TFMX-SONG ") && be32(d, 0x1d0) == 0 && be16(d, 0xc) != 0 && !detect_7v(d)
}

fn new_15(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_15(d) {
        if !detect_early_text(d) {
            return None;
        }
        // its samples by the module's name (`NAME.sam` in the NostalgicPlayer collections), else the game's set
        let smpl_data = match read_smpl(m) {
            Some(s) => s,
            None => crate::deli::read_companion(m, "smpl.set").1.ok()?,
        };
        return build_15(d, base, m, &smpl_data, true);
    }
    let smpl_data = read_smpl(m)?;
    build_15(d, base, m, &smpl_data, false)
}

/// A TFHD file of type 1: the song part `d` is in guest memory at `base`, its samples are at
/// `smpl` (in the same file, no copy). The Eagleplayer TFMX_1.5 plays these exactly as an
/// `mdat` with its `smpl` (the reference's writes are the same), so the engine is the same.
pub(super) fn new_15_tfhd(d: &[u8], base: u32, smpl: u32, _m: &mut Machine) -> Option<Box<dyn Replayer>> {
    make_15(d, base, smpl, false)
}

/// An early `TFMX-SONG ` module (the fixed tables of TFMX 1.5, no offsets at
/// $1D0), which the Pro replayer declined for want of `smpl.NAME`: its samples
/// are one `smpl.set` for the whole game. UADE's TFMX 1.5 player plays these.
fn new_early(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if be32(d, 0x1d0) != 0 || d.len() < 0x800 {
        return None;
    }
    let smpl_data = crate::deli::read_companion(m, "smpl.set").1.ok()?;
    build_15(d, base, m, &smpl_data, true)
}

fn build_15(d: &[u8], base: u32, m: &mut Machine, smpl_data: &[u8], early: bool) -> Option<Box<dyn Replayer>> {
    let smpl = m.alloc((smpl_data.len() as u32).max(4));
    if smpl == 0 {
        return None;
    }
    m.bus.write_bytes(smpl, smpl_data);
    make_15(d, base, smpl, early)
}

fn make_15(d: &[u8], base: u32, smpl: u32, early: bool) -> Option<Box<dyn Replayer>> {
    // InitPlayer: the last subsong is before the second song starting at 0
    let (mut n, mut zeros) = (29i32, 2);
    for k in 0..31 {
        if be16(d, 0x100 + 2 * k as usize) == 0 {
            zeros -= 1;
            if zeros == 0 {
                n = k - 1;
                break;
            }
        }
    }
    let mut p = V15 {
        early,
        g: Blk([0; 0x46]),
        w: Blk([0; 0x120]),
        p: Blk([0; 0xc8]),
        fade_lock: 0,
        dma_on: 0,
        mdat: base,
        smpl,
        subs: n.max(0) as u16,
        sub: 0,
        songend: false,
        spent: 0,
        counts: [0; B::Count as usize],
        trace: std::env::var_os("TFMX_TRACE").and_then(|p| std::fs::File::create(p).ok()),
        in_tick: false,
        next_tick: 0,
        seg: 0,
        cyc: 0,
        names: Vec::new(),
        ctx: 9,
    };
    p.g.sb(H_MASTER, 0x40);
    p.g.sb(H_MASTER + 1, 0x40);
    p.g.sw(0x34, 0x78);
    p.g.sw(H_START, 0xffff);
    p.g.sw(H_NOTE_WAIT, 1);
    for ch in 0..4 {
        p.w.sw(4 * ch + W_DMA_ON, 0x8200 | 1 << ch);
        p.w.sw(4 * ch + W_DMA_OFF, 1 << ch);
    }
    p.init_sound(0);
    Some(Box::new(p))
}

impl V15 {
    fn at(&mut self, a: B) {
        if self.in_tick {
            return;
        }
        self.spent += COST15[a as usize];
        self.counts[a as usize] += 1;
    }

    /// The original's code for `name` runs once (in the Eagleplayer's tick).
    fn c(&mut self, name: &'static str) {
        if !self.in_tick {
            return;
        }
        if let Ok(k) = COSTS15.binary_search_by(|e| e.0.cmp(name)) {
            self.cyc += COSTS15[k].1;
        }
        if let Some(e) = COSTS15_CTX.iter().find(|e| e.0 == name && e.1 == self.ctx) {
            self.cyc += e.2;
        }
        if self.trace.is_some() {
            let v = self.ctx;
            match self.names.iter_mut().find(|e| e.0 == name && e.2 == v) {
                Some(e) => e.1 += 1,
                None => self.names.push((name, 1, v)),
            }
        }
    }

    /// A host call returns: the next one's time counts from here.
    fn host_done(&mut self, hw: &Hw) {
        if self.in_tick {
            self.seg = hw.bus.clock;
            self.cyc = 0;
        }
    }

    fn flush(&mut self, hw: &mut Hw, what: &str) {
        if self.in_tick {
            let t = self.seg + (self.cyc.max(0) / 2) as u64;
            if t > hw.bus.clock {
                hw.bus.clock = t;
            }
            if let Some(f) = &mut self.trace {
                use std::io::Write;
                let c: Vec<String> = self
                    .names
                    .iter()
                    .map(|(n, k, v)| if *v < 4 { format!("{n}#{v}:{k}") } else if (10..18).contains(v) { format!("{n}@{}:{k}", v - 10) } else { format!("{n}:{k}") })
                    .collect();
                let _ = writeln!(f, "{} {} {} {}", hw.bus.clock, what, self.cyc, c.join(","));
            }
            self.names.clear();
            return;
        }
        hw.wait(self.spent);
        self.spent = 0;
        if let Some(f) = &mut self.trace {
            use std::io::Write;
            let c: Vec<String> =
                self.counts.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| format!("{i}:{n}")).collect();
            let _ = writeln!(f, "{} {} {}", hw.bus.clock, what, c.join(","));
        }
        self.counts = [0; B::Count as usize];
    }

    fn reg(&mut self, hw: &mut Hw, a: u32, v: u16) {
        let (pre, post, ch) = match a {
            0xdff096 => ("CALL_DMA", "RET_DMA", 9),
            _ => {
                let ch = ((a.wrapping_sub(0xdff0a0)) / 16) as u8;
                match (a.wrapping_sub(0xdff0a0)) % 16 {
                    4 => ("CALL_LEN", "RET_LEN", ch),
                    6 => ("CALL_PER", "RET_PER", ch),
                    _ => ("CALL_VOL", "RET_VOL", ch),
                }
            }
        };
        self.call(pre, ch);
        self.flush(hw, "w");
        hw.bus.write_word(a, v);
        self.host_done(hw);
        self.call(post, ch);
    }

    /// The code around a host call for channel `ch` (9: none) runs.
    fn call(&mut self, name: &'static str, ch: u8) {
        let k = self.ctx;
        self.ctx = ch;
        self.c(name);
        self.ctx = k;
    }

    /// ENPP_DMAMask (the host waits for the DMA after switching voices on).
    fn dma_mask(&mut self, hw: &mut Hw, m: u16) {
        let v = (m & 0x8000) | (m & 0xf);
        self.call("CALL_DMA", 9);
        self.flush(hw, "w");
        hw.bus.write_word(0xdff096, v);
        if v & 0x8000 != 0 {
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
        }
        self.host_done(hw);
        self.call("RET_DMA", 9);
    }

    fn poke_adr(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.call("CALL_ADR", ch as u8);
        self.flush(hw, "w");
        hw.loc(ch, a);
        self.host_done(hw);
        self.call("RET_ADR", ch as u8);
    }

    fn poke_len(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a4 + 16 * ch as u32, v);
    }

    fn poke_per(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a6 + 16 * ch as u32, v);
    }

    fn poke_vol(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a8 + 16 * ch as u32, v);
    }

    /// The period table ($118F2), which notes past its 64 entries read on
    /// from: the DMA mask word after it, then (here) zeros.
    fn period(&self, n: usize) -> u16 {
        match n {
            0..=63 => PERIODS[n],
            64 => self.dma_on,
            _ => 0,
        }
    }

    fn macro_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.mdat.wrapping_add(0x600 + 4 * n)))
    }

    fn pattern_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.mdat.wrapping_add(0x400 + 4 * n)))
    }

    /// InitSound: the subsong is started by the next interrupt, and a fade to
    /// $3F with speed 0 set up (as the original does).
    fn init_sound(&mut self, sub: u16) {
        self.sub = sub;
        self.songend = false;
        self.g.sl(H_MDAT, self.mdat);
        self.g.sl(H_SMPL, self.smpl);
        self.g.sw(H_ON, 1);
        self.g.sw(H_START, sub);
        if self.early {
            self.g.sb(H_FADE_END, 0x40);
            self.g.sb(H_FADE_DIR, 0);
            self.fade_lock = 0;
            return;
        }
        // $11476
        self.fade_lock = 1;
        self.g.sb(H_FADE_END, 0x3f);
        self.g.sb(H_FADE_CNT, 0);
        self.g.sb(H_FADE_RESET, 0);
        self.g.sb(H_FADE_DIR, 1);
        let m = self.g.b(H_MASTER);
        if m == 0x3f {
            self.g.sb(H_FADE_DIR, 0);
            self.fade_lock = 0;
        } else if m > 0x3f {
            self.g.sb(H_FADE_DIR, 0xff);
        }
    }

    /// $11348: the pending song's start.
    fn start_song(&mut self, hw: &mut Hw) {
        self.at(B::Start);
        // $11588
        self.g.sw(H_PLAYING, 0);
        for ch in 0..4 {
            self.w.sw(4 * ch + W_STATE, 0);
        }
        self.g.sw(H_SFX7, 0);
        self.g.sw(H_NOTE_WAIT, 1);
        hw.bus.w32(self.g.l(H_SMPL), 0);
        let a4 = self.mdat + 2 * (self.g.w(H_START) as u32 & 0x1f);
        self.g.sw(H_HALF, 0);
        let first = hw.bus.r16(a4 + 0x100);
        self.p.sw(4, first);
        self.p.sw(0, first);
        self.p.sw(2, hw.bus.r16(a4 + 0x140));
        self.p.sw(6, hw.bus.r16(a4 + 0x180));
        self.c("START");
        if self.p.w(6) > 0xf {
            self.c("ST_HALF");
            self.g.sw(H_HALF, 1);
            self.p.sw(6, self.p.w(6).wrapping_sub(0x10));
            self.g.sw(H_NOTE_WAIT, 2);
        }
        for k in 0..8 {
            self.p.sl(0x28 + 4 * k, EMPTY_15);
            self.p.sw(0x48 + 4 * k, 0xff00);
            self.p.sl(0x68 + 4 * k, 0);
        }
        if self.p.w(0) != 0x1ff {
            self.c("ST_STEP");
            self.track_step(hw);
        }
        self.g.sw(H_RESTART, 0);
        self.g.sw(H_SPEED_CNT, 0);
        self.g.sw(H_SONG, self.g.w(H_START));
        self.g.sw(H_START, 0xffff);
        self.g.sb(H_MASTER, self.g.b(H_FADE_END));
        self.g.sb(H_FADE_DIR, 0);
        self.fade_lock = 0;
        for ch in 0..4 {
            self.w.sw(4 * ch + W_LOCK, 0);
            self.w.sb(4 * ch + W_LOCK + 3, 0);
        }
        self.g.sw(H_PLAYING, 1);
    }

    /// $10AAA: the track step at the current position.
    fn track_step(&mut self, hw: &mut Hw) {
        for _ in 0..0x10000 {
            self.at(B::TrackStep);
            self.c("TS");
            let a0 = self.mdat.wrapping_add(((self.p.w(4) as u32) << 4) & 0xffff).wrapping_add(0x800);
            let d0 = hw.bus.r16(a0);
            if d0 == 0xeffe {
                let c = hw.bus.r16(a0 + 2);
                let a0 = a0 + 4;
                if c >= 5 {
                    self.c("TS_EBIG");
                }
                let c = if c < 5 { c } else { 0 };
                self.c(["TS_E0", "TS_E1", "TS_E2", "TS_E3", "TS_E4"][c as usize]);
                match c {
                    0 => {
                        // stops without telling DeliTracker; UADE's player (an early
                        // module) reports the song end here
                        self.g.sw(H_PLAYING, 0);
                        if self.early {
                            self.songend = true;
                        }
                        return;
                    }
                    1 => {
                        self.p.sw(4, hw.bus.r16(a0));
                        self.songend = true;
                        if self.in_tick {
                            // the Eagleplayer calls dtg_SongEnd here
                            self.call("CALL_SONGEND", 9);
                            self.flush(hw, "s");
                            self.host_done(hw);
                            self.c("TS_E1_SONGEND");
                        }
                    }
                    2 => {
                        let s = hw.bus.r16(a0);
                        self.p.sw(6, s);
                        self.g.sw(H_SPEED_CNT, s);
                        self.p.sw(4, self.p.w(4).wrapping_add(1));
                    }
                    3 => {
                        self.g.sw(H_HALF, hw.bus.r16(a0));
                        self.p.sw(4, self.p.w(4).wrapping_add(1));
                    }
                    _ => {
                        self.p.sw(4, self.p.w(4).wrapping_add(1));
                        if self.fade_lock == 0 {
                            self.c("TS_FADE");
                            let (b1, b3) = (hw.bus.r8(a0 + 1), hw.bus.r8(a0 + 3));
                            self.fade(b1, b3, true);
                        }
                    }
                }
                continue;
            }
            self.c("TS_PAT");
            for k in 0..8 {
                if k == 7 && self.g.w(H_SFX7) != 0 {
                    self.c("TS_SFX7");
                    break;
                }
                self.c("TS_K");
                let w = hw.bus.r16(a0 + 2 * k as u32);
                let o = 4 * k;
                self.p.sw(0x48 + o, w);
                if (w as i16) >= 0 {
                    self.c("TS_K_SET");
                    let pa = self.pattern_addr(hw, ((w >> 8) & 0x7f) as u32);
                    self.p.sl(0x28 + o, pa);
                    self.p.sl(0x68 + o, 0);
                    self.p.sw(0x4a + o, 0xffff);
                }
            }
            return;
        }
    }

    /// `ts`: a track step's fade, whose paths cost differently from a pattern's.
    fn fade(&mut self, speed: u8, end: u8, ts: bool) {
        self.fade_lock = 1;
        self.g.sb(H_FADE_END, end);
        self.g.sb(H_FADE_CNT, speed);
        self.g.sb(H_FADE_RESET, speed);
        self.c("FADE");
        if speed == 0 {
            self.c(if ts { "TS_FADE_NOW" } else { "FADE_NOW" });
            self.g.sb(H_MASTER, end);
        } else {
            self.g.sb(H_FADE_DIR, 1);
            let m = self.g.b(H_MASTER);
            if m != end {
                self.c(if ts { "TS_FADE_GO" } else { "FADE_GO" });
                if m > end {
                    self.c("FADE_DOWN");
                    self.g.sb(H_FADE_DIR, 0xff);
                }
                return;
            }
        }
        self.c("FADE_DONE");
        self.g.sb(H_FADE_DIR, 0);
        self.fade_lock = 0;
    }

    /// $107FC: the patterns, when the speed counter is up.
    fn tracks(&mut self, hw: &mut Hw) {
        self.at(B::Tracks);
        self.c("TRACKS");
        let c = self.g.w(H_SPEED_CNT);
        if c != 0 {
            self.c("TR_WAIT");
            self.g.sw(H_SPEED_CNT, c - 1);
            return;
        }
        self.g.sw(H_SPEED_CNT, self.p.w(6));
        'again: for _ in 0..256 {
            if self.g.w(H_HALF) != 0 && self.g.w(H_RESTART) != 0 {
                self.c("TR_HALF_SKIP");
                self.g.sw(H_SPEED_CNT, 0);
                self.g.sw(H_RESTART, 0);
                self.g.sw(H_NOTE_WAIT, 1);
                return;
            }
            if self.g.w(H_HALF) != 0 {
                self.c("TR_HALF");
            }
            self.c("TR_PASS");
            self.g.sb(H_RESTART, 0);
            for k in 0..8 {
                self.ctx = 10 + k as u8;
                self.pattern(hw, 4 * k);
                self.ctx = 9;
                if self.g.b(H_RESTART) != 0 {
                    self.c("TR_RESTART");
                    continue 'again;
                }
            }
            break;
        }
    }

    /// $1088E: pattern `o / 4`'s row.
    fn pattern(&mut self, hw: &mut Hw, o: usize) {
        self.at(B::Row);
        self.c("ROW");
        let num = self.p.b(0x48 + o);
        if num >= 0x90 {
            self.c("ROW_OFF");
            if num == 0xfe {
                self.c("ROW_FE");
                self.p.sb(0x48 + o, 0xff);
                if self.p.w(0x08 + o) == 0 {
                    // $11322
                    self.c("ROW_FE_DMA");
                    let i = (self.p.b(0x49 + o) & 3) as usize;
                    let m = self.w.w(4 * i + W_DMA_OFF);
                    self.dma_mask(hw, m);
                    self.c("ROW_FE_POST");
                    self.w.sw(4 * i + W_STATE, 0);
                }
            }
            return;
        }
        let wt = self.p.w(0x6a + o);
        if wt != 0 {
            self.c("ROW_WAIT");
            self.p.sw(0x6a + o, wt - 1);
            return;
        }
        self.c("ROW_READ");
        for _ in 0..0x10000 {
            self.c("P_CMD");
            let step = self.p.w(0x68 + o);
            let a = self.p.l(0x28 + o).wrapping_add(step.wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a);
            self.g.sl(H_CMD, cmd);
            let b0 = (cmd >> 24) as u8;
            let step_on = |s: &mut Self| s.p.sw(0x68 + o, s.p.w(0x68 + o).wrapping_add(1));
            if b0 < 0xf0 {
                self.at(B::Note);
                self.c("P_NOTE");
                self.g.sb(H_CMD, b0.wrapping_add(self.p.b(0x49 + o)));
                if self.p.w(0x08 + o) == 0 {
                    self.c("P_NOTE_ON");
                    self.note_port(hw, self.g.l(H_CMD));
                }
                step_on(self);
                continue;
            }
            self.at(B::PatternCmd);
            self.c(P_NAMES[(b0 & 0xf) as usize]);
            let (b1, b3) = ((cmd >> 16) as u8, cmd as u8);
            let w1 = cmd as u16;
            match b0 & 0xf {
                0 => {
                    if self.p.b(0x48 + o) == 0x81 {
                        self.c("P_F0_81");
                        self.p.sb(0x48 + o, 0xff);
                        return;
                    }
                    if self.p.w(4) == self.p.w(2) {
                        self.c("P_F0_LAST");
                        self.p.sw(4, self.p.w(0));
                        self.songend = true;
                        if self.in_tick {
                            // the Eagleplayer calls dtg_SongEnd here
                            self.call("CALL_SONGEND", 9);
                            self.flush(hw, "s");
                            self.host_done(hw);
                            self.c("P_F0_SONGEND");
                        }
                    } else {
                        self.c("P_F0_NEXT");
                        self.p.sw(4, self.p.w(4).wrapping_add(1));
                    }
                    let k = self.ctx;
                    self.ctx = 9;
                    self.track_step(hw);
                    self.ctx = k;
                    self.g.sw(H_RESTART, 0xffff);
                    return;
                }
                1 => {
                    let c = self.p.b(0x4a + o);
                    if c == 0 {
                        self.c("P_F1_ZERO");
                        self.p.sb(0x4a + o, 0xff);
                        step_on(self);
                        continue;
                    }
                    if c == 0xff {
                        self.c("P_F1_FF");
                        self.p.sb(0x4a + o, b1.wrapping_sub(1));
                    } else {
                        self.c("P_F1_DEC");
                        self.p.sb(0x4a + o, c - 1);
                    }
                    self.p.sw(0x68 + o, w1);
                }
                2 | 8 => {
                    if b0 & 0xf == 8 {
                        self.p.sl(0x88 + o, self.p.l(0x28 + o));
                        self.p.sw(0xa8 + o, step);
                    }
                    let n = b1 & 0x7f;
                    self.p.sb(0x48 + o, n);
                    let pa = self.pattern_addr(hw, n as u32);
                    self.p.sl(0x28 + o, pa);
                    self.p.sw(0x68 + o, w1);
                }
                3 => {
                    self.p.sb(0x6b + o, b1);
                    step_on(self);
                    return;
                }
                4 => {
                    self.p.sb(0x48 + o, 0xff);
                    return;
                }
                14 => {
                    self.g.sw(H_SFX7, 0);
                    self.p.sb(0x48 + o, 0xff);
                    return;
                }
                5..=7 => {
                    if self.p.w(0x08 + o) == 0 {
                        self.c("P_F5_ON");
                        self.note_port(hw, self.g.l(H_CMD));
                    }
                    step_on(self);
                }
                9 => {
                    self.p.sl(0x28, self.p.l(0x88));
                    self.p.sw(0x68, self.p.w(0xa8));
                    step_on(self);
                }
                10 => {
                    if self.fade_lock == 0 {
                        self.c("P_FA_FADE");
                        self.fade(b1, b3, false);
                    }
                    step_on(self);
                }
                11..=13 => {
                    step_on(self);
                    return;
                }
                _ => step_on(self),
            }
        }
    }

    /// $111FE: a note, or a note command, for the voice its third byte names.
    fn note_port(&mut self, hw: &mut Hw, d0: u32) {
        let saved = self.g.l(H_CMD);
        self.g.sl(H_CMD, d0);
        let [b0, b1, b2, b3] = d0.to_be_bytes();
        let v = 4 * (b2 & 3) as usize;
        self.c("NP");
        if self.w.b(v + W_LOCK + 3) != 0 {
            self.c("NP_LOCKED");
        } else {
            self.c(if b0 == 0xf7 {
                "NP_F7"
            } else if b0 == 0xf6 {
                "NP_F6"
            } else if b0 == 0xf5 {
                "NP_F5"
            } else if b0 >= 0xbf {
                "NP_PORTA"
            } else if b0 < 0x40 {
                "NP_NOTE"
            } else {
                "NP_NONE"
            });
            if b0 == 0xf7 {
                self.w.sb(v + W_ENV, 1);
                self.w.sb(v + W_ENV + 3, b1);
                self.w.sb(v + W_ENV + 1, 1);
                self.w.sb(v + W_ENV + 2, b3);
            } else if b0 == 0xf6 {
                let d = b1 & 0xfe;
                self.w.sb(v + W_VIB_RESET, d);
                self.w.sb(v + W_VIB_TIME, d >> 1);
                self.w.sb(v + W_VIB_WIDTH, b3);
                self.w.sb(v + W_VIB_WIDTH + 1, 1);
                self.w.sw(v + W_VIB_OFS, 0);
            } else if b0 == 0xf5 {
                self.w.sb(v + W_KEY, 0);
            } else if b0 >= 0xbf {
                self.w.sb(v + W_PORTA_RESET, b1);
                self.w.sb(v + W_PORTA_TIME, 1);
                self.w.sw(v + W_PORTA_RATE, b3 as u16);
                self.w.sw(v + W_PORTA_PER, self.w.w(v + W_PER));
                let n = (b0 & 0x3f) as usize;
                self.w.sb(v + W_NOTE, n as u8);
                self.w.sw(v + W_PER, self.period(n));
            } else if b0 < 0x40 {
                self.w.sb(v + W_FINE + 1, b3);
                self.w.sb(v + W_VEL + 1, (b2 >> 4) & 0xf);
                self.w.sb(v + W_MACRO_NUM, b1);
                self.w.sb(v + W_NOTE, b0);
                let ma = self.macro_addr(hw, b1 as u32);
                self.w.sl(v + W_MACRO, ma);
                self.w.sw(v, 1);
                self.w.sw(v + W_STATE, 1);
                self.w.sb(v + W_KEY, 1);
            }
        }
        self.g.sl(H_CMD, saved);
    }

    /// $10CF2: voice `ch`'s tick.
    fn voice(&mut self, hw: &mut Hw, ch: usize) {
        self.at(B::Voice);
        self.c("VOICE");
        let v = 4 * ch;
        let st = self.w.w(v + W_STATE);
        if (st as i16) <= 0 {
            self.at(B::Effects);
            self.c(if st == 0 { "V_FX_OFF" } else { "V_FX" });
            let l = self.w.w(v + W_LOCK);
            if (l as i16) >= 0 {
                self.c("V_LOCK_DEC");
                self.w.sw(v + W_LOCK, l.wrapping_sub(1));
            } else {
                self.c("V_LOCK_CLR");
                self.w.sb(v + W_LOCK + 3, 0);
            }
            self.vibrato(hw, ch);
            self.porta(hw, ch);
            self.envelope(ch);
            self.at(B::Fade);
            self.c("V_FADE");
            let dir = self.g.b(H_FADE_DIR);
            if dir != 0 {
                self.c("V_FADE_ON");
                let c = self.g.b(H_FADE_CNT).wrapping_sub(1);
                self.g.sb(H_FADE_CNT, c);
                if c == 0 {
                    self.c("V_FADE_STEP");
                    self.g.sb(H_FADE_CNT, self.g.b(H_FADE_RESET));
                    let m = self.g.b(H_MASTER).wrapping_add(dir);
                    self.g.sb(H_MASTER, m);
                    if self.g.b(H_FADE_END) == m {
                        self.c("V_FADE_END");
                        self.g.sb(H_FADE_DIR, 0);
                        self.fade_lock = 0;
                    }
                }
            }
            self.at(B::Volume);
            self.c("V_VOL");
            let d1 = self.g.b(H_MASTER) as u16;
            let mut d0 = self.w.b(v + W_VOL) as u16;
            if d1 != 0x40 {
                self.c("V_VOL_SCALE");
                d0 = ((((d0 << 2) as u32 * d1 as u32) as u16) >> 8) & 0x7f;
            }
            self.poke_vol(hw, ch, d0);
            self.c("V_VOL_POST");
        } else {
            self.c("V_NOFX");
        }
        let st = self.w.w(v + W_STATE);
        if st == 0 {
            self.c("V_OFF");
            return;
        }
        if (st as i16) > 0 {
            self.c("V_NEW");
            self.at(B::NewNote);
            self.w.sw(v + W_STEP, 0);
            self.w.sw(v + W_WAIT, self.g.w(H_NOTE_WAIT));
            self.w.sw(v + W_STATE, 0xffff);
            self.w.sw(v + W_LOOP, 0xffff);
        }
        self.run_macro(hw, ch);
    }

    fn vibrato(&mut self, hw: &mut Hw, ch: usize) {
        let v = 4 * ch;
        if self.w.b(v + W_VIB_RESET) == 0 {
            self.c("VIB_OFF");
            return;
        }
        self.at(B::Vibrato);
        self.c("VIB");
        let o = self.w.w(v + W_VIB_OFS).wrapping_add(self.w.b(v + W_VIB_WIDTH) as i8 as u16);
        self.w.sw(v + W_VIB_OFS, o);
        let d0 = o.wrapping_add(self.w.w(v + W_PER));
        if self.w.w(v + W_PORTA_RATE) == 0 {
            self.c("VIB_OUT");
            self.poke_per(hw, ch, d0);
            self.c("VIB_POST");
        }
        let t = self.w.b(v + W_VIB_TIME).wrapping_sub(1);
        self.w.sb(v + W_VIB_TIME, t);
        if t == 0 {
            self.c("VIB_FLIP");
            self.w.sb(v + W_VIB_TIME, self.w.b(v + W_VIB_RESET));
            self.w.sb(v + W_VIB_WIDTH, (self.w.b(v + W_VIB_WIDTH) ^ 0xff).wrapping_add(1));
        }
    }

    fn porta(&mut self, hw: &mut Hw, ch: usize) {
        let v = 4 * ch;
        let rate = self.w.w(v + W_PORTA_RATE);
        if rate == 0 {
            self.c("PORTA_OFF");
            return;
        }
        let t = self.w.b(v + W_PORTA_TIME).wrapping_sub(1);
        self.w.sb(v + W_PORTA_TIME, t);
        if t != 0 {
            self.c("PORTA_WAIT");
            return;
        }
        self.at(B::Porta);
        self.c("PORTA");
        self.w.sb(v + W_PORTA_TIME, self.w.b(v + W_PORTA_RESET));
        let d1 = self.w.w(v + W_PER);
        let mut d0 = self.w.w(v + W_PORTA_PER);
        if d0 == d1 {
            self.c("PORTA_EQ");
            return;
        }
        let up = d0 < d1;
        let reached = if up {
            self.c("PORTA_UP");
            d0 = d0.wrapping_add(rate);
            d0 >= d1
        } else {
            self.c("PORTA_DOWN");
            d0 = d0.wrapping_sub(rate);
            d0 <= d1
        };
        if reached {
            self.c(if d0 == d1 { "PORTA_HIT" } else if up { "PORTA_UP_REACHED" } else { "PORTA_REACHED" });
            self.w.sw(v + W_PORTA_RATE, 0);
            d0 = self.w.w(v + W_PER);
        }
        let d0 = d0 & 0x7ff;
        self.w.sw(v + W_PORTA_PER, d0);
        self.poke_per(hw, ch, d0);
        self.c("PORTA_POST");
    }

    fn envelope(&mut self, ch: usize) {
        let v = 4 * ch;
        if self.w.b(v + W_ENV) == 0 {
            self.c("ENV_OFF");
            return;
        }
        let c = self.w.b(v + W_ENV + 1);
        if c != 0 {
            self.c("ENV_WAIT");
            self.w.sb(v + W_ENV + 1, c - 1);
            return;
        }
        self.at(B::Env);
        self.c("ENV");
        self.w.sb(v + W_ENV + 1, self.w.b(v + W_ENV));
        let end = self.w.b(v + W_ENV + 2) as i8;
        let rate = self.w.b(v + W_ENV + 3);
        let vol = self.w.b(v + W_VOL);
        let done = if end > vol as i8 {
            self.c("ENV_UP");
            let n = vol.wrapping_add(rate) as i8;
            self.w.sb(v + W_VOL, n as u8);
            end <= n
        } else {
            self.c("ENV_DOWN");
            let n = vol.wrapping_sub(rate) as i8;
            self.w.sb(v + W_VOL, n as u8);
            if n < 0 {
                self.c("ENV_NEG");
            }
            n < 0 || end >= n
        };
        if done {
            self.c("ENV_DONE");
            self.w.sb(v + W_VOL, end as u8);
            self.w.sb(v + W_ENV, 0);
        }
    }

    /// $10D84: the macro, from its step, until a command ends it for this tick.
    fn run_macro(&mut self, hw: &mut Hw, ch: usize) {
        let v = 4 * ch;
        let wt = self.w.w(v + W_WAIT);
        if wt != 0 {
            self.c("M_WAIT");
            self.w.sw(v + W_WAIT, wt - 1);
            return;
        }
        self.c("M_RUN");
        let step_on = |s: &mut Self| s.w.sw(v + W_STEP, s.w.w(v + W_STEP).wrapping_add(1));
        for _ in 0..0x10000 {
            self.at(B::MacroCmd);
            self.c("MACRO");
            let a = self.w.l(v + W_MACRO).wrapping_add(self.w.w(v + W_STEP).wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a);
            self.g.sl(H_CMD, cmd);
            let [c, b1, b2, b3] = cmd.to_be_bytes();
            let w1 = cmd as u16;
            self.c(M15_NAMES[(c as usize).min(0x1a)]);
            match c {
                0x00 | 0x0a | 0x13 => {
                    if c != 0x13 {
                        self.w.sb(v + W_ENV, 0);
                        self.w.sb(v + W_VIB_RESET, 0);
                        self.w.sw(v + W_PORTA_RATE, 0);
                    }
                    if c != 0x0a {
                        let m = self.w.w(v + W_DMA_OFF);
                        self.dma_mask(hw, m);
                        self.c(if c == 0 { "M_00_POST" } else { "M_13_POST" });
                    }
                    step_on(self);
                }
                0x01 => {
                    if self.early {
                        // UADE's player switches the voice on at once
                        let m = self.w.w(v + W_DMA_ON);
                        self.dma_mask(hw, m);
                    } else {
                        self.dma_on |= self.w.w(v + W_DMA_ON);
                    }
                    step_on(self);
                }
                0x02 => {
                    let a = (cmd & 0xff_ffff).wrapping_add(self.g.l(H_SMPL));
                    self.w.sl(v + W_ADDR, a);
                    self.poke_adr(hw, ch, a);
                    self.c("M_02_POST");
                    step_on(self);
                }
                0x03 => {
                    self.w.sw(v + W_LEN, w1);
                    self.poke_len(hw, ch, w1);
                    self.c("M_03_POST");
                    step_on(self);
                }
                0x04 => {
                    self.w.sw(v + W_WAIT, w1);
                    step_on(self);
                    return;
                }
                0x05 | 0x10 => {
                    if c == 0x10 && self.w.b(v + W_KEY) == 0 {
                        self.c("M_10_NOKEY");
                        step_on(self);
                        continue;
                    }
                    let l = self.w.b(v + W_LOOP);
                    if l == 0 {
                        self.c("LOOP_END");
                        self.w.sb(v + W_LOOP, 0xff);
                        step_on(self);
                        continue;
                    }
                    if l == 0xff {
                        self.c("LOOP_FF");
                        self.w.sb(v + W_LOOP, b1.wrapping_sub(1));
                    } else {
                        self.c("LOOP_DEC");
                        self.w.sb(v + W_LOOP, l - 1);
                    }
                    self.w.sw(v + W_STEP, w1);
                }
                0x06 | 0x15 => {
                    if c == 0x15 {
                        self.w.sl(v + W_RET, self.w.l(v + W_MACRO));
                        self.w.sw(v + W_RET_STEP, self.w.w(v + W_STEP));
                    }
                    let ma = self.macro_addr(hw, (b1 & 0x7f) as u32);
                    self.w.sl(v + W_MACRO, ma);
                    self.w.sw(v + W_STEP, w1);
                    // through $10D66: the voice's state and wait again
                    let st = self.w.w(v + W_STATE);
                    if st == 0 {
                        self.c("M_06_OFF");
                        return;
                    }
                    if (st as i16) > 0 {
                        self.c("M_06_NEW");
                        self.w.sw(v + W_STEP, 0);
                        self.w.sw(v + W_WAIT, self.g.w(H_NOTE_WAIT));
                        self.w.sw(v + W_STATE, 0xffff);
                        self.w.sw(v + W_LOOP, 0xffff);
                    }
                    let wt = self.w.w(v + W_WAIT);
                    if wt != 0 {
                        self.c("M_06_WAIT");
                        self.w.sw(v + W_WAIT, wt - 1);
                        return;
                    }
                    self.c("M_06_GO");
                }
                0x07 => {
                    self.w.sw(v + W_STATE, 0);
                    return;
                }
                0x08 | 0x09 => {
                    let per = if c == 0x08 {
                        let n = b1.wrapping_add(self.w.b(v + W_NOTE)) as usize;
                        self.period(n).wrapping_add(self.w.b(v + W_FINE + 1) as i8 as u16)
                    } else if self.early {
                        // UADE's player adds the note's detune to SetNote too
                        self.period(b1 as usize).wrapping_add(self.w.b(v + W_FINE + 1) as i8 as u16)
                    } else {
                        self.period(b1 as usize)
                    }
                    .wrapping_add(w1);
                    self.w.sw(v + W_PER, per);
                    if self.w.w(v + W_PORTA_RATE) == 0 {
                        self.c("M_08_OUT");
                        self.poke_per(hw, ch, per);
                        self.c(if c == 8 { "M_08_POST" } else { "M_09_POST" });
                    }
                    step_on(self);
                    return;
                }
                0x0b => {
                    self.w.sb(v + W_PORTA_RESET, b1);
                    self.w.sb(v + W_PORTA_TIME, 1);
                    self.w.sw(v + W_PORTA_RATE, w1);
                    self.w.sw(v + W_PORTA_PER, self.w.w(v + W_PER));
                    step_on(self);
                }
                0x0c => {
                    let d = b1 & 0xfe;
                    self.w.sb(v + W_VIB_RESET, d);
                    self.w.sb(v + W_VIB_TIME, d >> 1);
                    self.w.sb(v + W_VIB_WIDTH, b3);
                    self.w.sb(v + W_VIB_WIDTH + 1, 1);
                    if self.w.w(v + W_PORTA_RATE) == 0 {
                        self.c("M_0C_OUT");
                        let p = self.w.w(v + W_PER);
                        self.poke_per(hw, ch, p);
                        self.c("M_0C_POST");
                        self.w.sw(v + W_VIB_OFS, 0);
                    }
                    step_on(self);
                }
                0x0d => {
                    let x = self.w.w(v + W_VEL).wrapping_mul(3).wrapping_add(w1);
                    self.w.sb(v + W_VOL, x as u8);
                    step_on(self);
                }
                0x0e => {
                    self.w.sb(v + W_VOL, b3);
                    step_on(self);
                }
                0x0f => {
                    self.w.sb(v + W_ENV, b2);
                    self.w.sb(v + W_ENV + 3, b1);
                    self.w.sb(v + W_ENV + 1, b2);
                    self.w.sb(v + W_ENV + 2, b3);
                    step_on(self);
                }
                0x11 => {
                    let a = self.w.l(v + W_ADDR).wrapping_add(w1 as i16 as u32);
                    self.w.sl(v + W_ADDR, a);
                    self.poke_adr(hw, ch, a);
                    self.c("M_11_POST");
                    step_on(self);
                }
                0x12 => {
                    let l = self.w.w(v + W_LEN).wrapping_add(w1);
                    self.w.sw(v + W_LEN, l);
                    self.poke_len(hw, ch, l);
                    self.c("M_12_POST");
                    step_on(self);
                }
                0x14 => {
                    if self.w.b(v + W_KEY) == 0 {
                        self.c("M_14_NOKEY");
                        step_on(self);
                        continue;
                    }
                    let l = self.w.b(v + W_LOOP);
                    if l == 0 {
                        self.c("M14_END");
                        self.w.sb(v + W_LOOP, 0xff);
                        step_on(self);
                        continue;
                    }
                    if l == 0xff {
                        self.c("M14_FF");
                        self.w.sb(v + W_LOOP, b3.wrapping_sub(1));
                    } else {
                        self.c("M14_DEC");
                        self.w.sb(v + W_LOOP, l - 1);
                    }
                    return;
                }
                0x16 => {
                    self.w.sl(v + W_MACRO, self.w.l(v + W_RET));
                    self.w.sw(v + W_STEP, self.w.w(v + W_RET_STEP));
                    step_on(self);
                }
                0x17 => {
                    self.w.sw(v + W_PER, w1);
                    if self.w.w(v + W_PORTA_RATE) == 0 {
                        self.c("M_17_OUT");
                        self.poke_per(hw, ch, w1);
                        self.c("M_17_POST");
                    }
                    step_on(self);
                }
                0x18 => {
                    let d0 = cmd & 0xffff;
                    let a = self.w.l(v + W_ADDR).wrapping_add(d0);
                    self.w.sl(v + W_ADDR, a);
                    let l = self.w.w(v + W_LEN).wrapping_sub((d0 as u16) >> 1);
                    self.w.sw(v + W_LEN, l);
                    self.poke_adr(hw, ch, a);
                    self.c("M_18_MID");
                    self.poke_len(hw, ch, l);
                    self.c("M_18_POST");
                    step_on(self);
                }
                0x19 => {
                    let a = self.g.l(H_SMPL);
                    self.w.sl(v + W_ADDR, a);
                    self.poke_adr(hw, ch, a);
                    self.c("M_19_MID");
                    self.w.sw(v + W_LEN, 1);
                    self.poke_len(hw, ch, 1);
                    self.c("M_19_POST");
                    step_on(self);
                }
                _ => {
                    step_on(self);
                    return;
                }
            }
        }
    }

    /// $1079E.
    fn interrupt(&mut self, hw: &mut Hw) {
        self.flush(hw, "t");
        if !self.early {
            if hw.bus.clock < self.next_tick {
                hw.bus.clock = self.next_tick;
            }
            self.in_tick = true;
            self.seg = hw.bus.clock;
            self.cyc = 0;
        }
        self.at(B::Tick);
        self.c("TICK");
        // UADE's player (an early module) reports the song end at the start of every tick
        // while all eight tracks have no pattern (a step of FE/FF tracks), the Eagleplayer
        // never does: seen in the reference's song-end calls, the tick after the stop
        if self.early && (self.g.w(H_START) as i16) < 0 && (0..8).all(|k| self.p.b(0x48 + 4 * k) == 0xff) {
            self.songend = true;
        }
        if (self.g.w(H_START) as i16) >= 0 {
            self.c("I_START");
            self.start_song(hw);
        } else if self.g.w(H_ON) == 0 || self.g.w(H_PLAYING) == 0 {
            self.c("I_IDLE");
            self.fade_lock = 0;
        } else {
            self.c("I_PLAY");
            self.tracks(hw);
            self.dma_on = 0;
            for ch in 0..4 {
                self.ctx = ch as u8;
                self.voice(hw, ch);
            }
            self.ctx = 9;
            if self.dma_on != 0 {
                self.at(B::DmaOn);
                self.c("DMA_ON");
                let m = self.dma_on;
                self.dma_mask(hw, m);
            } else {
                self.c("NO_DMA_ON");
            }
        }
        self.flush(hw, "e");
        self.in_tick = false;
        self.next_tick = hw.bus.clock + TICK_EXIT;
    }
}

impl Replayer for V15 {
    fn format(&self) -> &'static str {
        "TFMX 1.5"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.interrupt(hw);
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.init_sound(n);
    }
}

// ---------------------------------------------------------------------------
// TFMX 7V: the Eagleplayer TFMX_7V ("TFMX 7V Eagleplayer V2.35"), whose own
// replayer plays eight voices: voices 0-2 on Paula's channels 0-2, voices 4-7
// mixed in software into a buffer that channel 3 plays (voice 3 is left out
// while the mixer runs). The mixer runs in channel 3's audio interrupt, and
// the replayer's tick follows it there: the buffer's length sets the tempo.

pub const OPEN_7V: Open = Open { name: "TFMX 7V", detect: detect_7v, new: new_7v };

/// Check2: a TFMX module whose first song starts with a 7 voices track step
/// (`$EFFE 0003`) among the track commands before its first real step.
pub(super) fn detect_7v(d: &[u8]) -> bool {
    if d.len() < 0x200 || be32(d, 0) != 0x54464d58 {
        return false;
    }
    let (steps, patterns, macros) =
        if be32(d, 0x1d4) == 0 { (0x800, 0x400, 0x600) } else { (be32(d, 0x1d0), be32(d, 0x1d4), be32(d, 0x1d8)) };
    let at = |o: u32, k: u32| o.checked_add(k).map_or(0, |a| be32(d, a as usize));
    if at(patterns, 4) == 0 || at(macros, 4) == 0 {
        return false;
    }
    let mut o = be16(d, 0x100) << 4;
    for _ in 0..0x1000 {
        let a = steps.wrapping_add(o as i16 as u32) as usize;
        if be16(d, a) != 0xeffe {
            return false;
        }
        if be16(d, a + 2) == 3 {
            return true;
        }
        o = o.wrapping_add(0x10);
    }
    false
}

// The global block ($13b38).
const G7_MDAT: usize = 0x00;
const G7_SMPL: usize = 0x04;
const G7_SYNTH: usize = 0x08;
const G7_RESTART: usize = 0x0d;
const G7_SONG: usize = 0x0e; // the song playing; negative: no patterns
const G7_FADE_DIR: usize = 0x0f;
const G7_SEED: usize = 0x10;
const G7_CMD: usize = 0x12; // the command at hand, a long
const G7_SPEED_CNT: usize = 0x16;
const G7_ON: usize = 0x18;
const G7_START: usize = 0x1e; // flags; the song to start is the next byte
const G7_SFX7: usize = 0x20; // a sound effect holds track 7
const G7_MASTER: usize = 0x22;
const G7_FADE_END: usize = 0x23;
const G7_FADE_CNT: usize = 0x24;
const G7_FADE_RESET: usize = 0x25;
const G7_BUSY: usize = 0x27;
const G7_TRACK_LOOP: usize = 0x28;
const G7_STEPS: usize = 0x2a;
const G7_PATTERNS: usize = 0x2e;
const G7_MACROS: usize = 0x32;
const G7_SFX_TAB: usize = 0x36;
const G7_DMA_ON: usize = 0x3a;
const G7_DMA_OFF: usize = 0x3c; // DMA off at the start of the next tick
const G7_MIX: usize = 0x3e; // the mixer runs
const G7_MIX_IRQ: usize = 0x3f;
const G7_RATE: usize = 0x40; // the mixing rate, in kHz
const G7_TEMPO: usize = 0x42; // the buffer's length adjustment, in percent
const G7_DMA_START: usize = 0x54; // DMACON at the start of a tick

// The voice block ($13bce, $98 bytes each). The fields are TFMX Pro's (V_*)
// up to $53; then:
const S_PARTNER: usize = 0x54; // the random player's next voice
const S_PER_OUT: usize = 0x8c;
const S_OFF_DONE: usize = 0x8e;
const S_PEND: usize = 0x88;
const S_MACRO_NUM: usize = 0x07;
const S_RND_MODE: usize = 0x49;
const S_RND_COUNT: usize = 0x4a;
const VOICE7_SIZE: usize = 0x98;

// The track block ($1408e): TFMX Pro's (P_*), with the wait for another
// track's step ($E/$F pattern commands) at $140.
const P7_TRACK_WAIT: usize = 0x140;

/// The voice blocks' initial contents: the channels' masks; the notes,
/// volumes and periods start at 0. (The original's variables hold whatever was
/// left in its data hunk when it was saved, which the port does not carry: so
/// until a voice plays a note, its first ticks write volume and period 0 where
/// the original writes those leftovers, and a first SetPrevNote reads note 0.)
fn voice7_init(i: usize) -> Blk<VOICE7_SIZE> {
    let mut v = Blk([0; VOICE7_SIZE]);
    if i < 4 {
        v.sw(V_DMA_ON, 0x8200 | 1 << i);
        v.sw(V_DMA_OFF, 1 << i);
        v.sw(V_INT_ON, 0x8000 | 0x80 << i);
        v.sw(V_INT_OFF, 0x80 << i);
    }
    v.sl(S_PARTNER, if i & 3 == 3 { (-3 * VOICE7_SIZE as i32) as u32 } else { VOICE7_SIZE as u32 });
    v.sl(V_SID_BUF, 4 + 0x100 * i as u32);
    v.sw(V_LOCK, 0xffff);
    v.sb(V_LOOP, 0xff);
    v.sb(S_OFF_DONE, 0xff);
    // The voices 0 to 2 hold a volume and a period before their first note: the first tick
    // of the Eagleplayer writes them (the same on every module in its logs).
    if i < 3 {
        v.sb(V_VOL, [0x2e, 0x20, 0x20][i]);
        v.sw(S_PER_OUT, [0x17d, 0x12f, 0x168][i]);
    }
    v
}

/// The mixer's period for a rate of `khz` (3 to 22): the NTSC clock's.
fn mix_period(khz: u16) -> u16 {
    let k = khz.max(1) as u32 * 1000;
    ((3_579_545 + k / 2) / k) as u16
}

/// One of the mixer's four voices: the registers the replayer writes, as
/// Paula's (`$14c2a`), and where the mixer plays.
#[derive(Clone, Copy, Default)]
struct MixVoice {
    loc: u32,
    len: u16,
    per: u16,
    vol: u16,
    /// The loop the mixer takes at the end: its end and its length in bytes.
    loop_end: u32,
    loop_len: u16,
    /// DMA on (`$14ca6`), and the restart the next DMA on makes (`$14c6a`).
    on: bool,
    restart: bool,
    /// The step (the integer in the low word, the fraction in the high one),
    /// the volume the mixer's code was patched with, and the position: the end
    /// of the sample, and the fraction above a negative offset from it.
    step: u32,
    tab: u8,
    end: u32,
    pos: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
enum At7 {
    Irq,
    ScopeClear,
    ScopeVoice,
    MixStart,
    MixVoice,
    Sample,
    Wrap,
    Tick,
    OffStart,
    OffPer,
    VoiceStart,
    Pending,
    NewNote,
    MacroCmd,
    Effects,
    AddBegin,
    Vibrato,
    Porta,
    Env,
    Random,
    Fade,
    Volume,
    SidStart,
    SidByte,
    Tracks,
    Row,
    PatternCmd,
    Note,
    TrackStep,
    PerOut,
    MixCfg,
    IrqEnd,
    VolScale,
    MixClear,
    MixLoop,
    Reg,
    SongPlay,
    Count,
}

/// Fitted by least squares to the reference's write times.
const COST7: [u64; At7::Count as usize] = [
    798, 1616, 53, 0, 147, 141, 17, 0, 7, 3, 195, 0, 84, 124, 30, 44, 133, 185, 45, 0, 0, 106, 0, 0, 197, 72, 199, 441,
    312, 16, 349, 172, 32, 5518, 215, 24, 909,
];

enum Next7 {
    /// `$12800`: the next command at once (this one moved the step).
    Read,
    /// `$128e8`: the next step, at once.
    Step,
    /// `$128d6`: the next step, at once only the first time.
    Pause,
    /// `$12f04`: on to the effects.
    Effects,
}

pub struct Seven {
    g: Blk<0x96>,
    v: [Blk<VOICE7_SIZE>; 8],
    p: Blk<0x188>,
    /// `$14216`: the fade lock word, and `$15` set while a pattern plays.
    c: Blk<0x26>,
    /// `$14156`: each song's position, then its speed, then a word.
    song_pos: Blk<0xc0>,
    mix: [MixVoice; 4],
    /// The buffers: the one Paula gets next, the other, and the silence.
    buf_next: u32,
    buf_other: u32,
    silence: u32,
    bss: u32,
    mix_bytes: u16,
    mix_words: u16,
    mix_d3: u32,
    vol_tab: Vec<[u8; 256]>,
    mdat: u32,
    smpl: u32,
    empty: u32,
    subsongs: u16,
    sub: u16,
    /// The Eagleplayer's row counter (`$104dc`): its display is cleared at 0.
    rows: u16,
    /// The mixing rate the Eagleplayer starts with (its configuration).
    rate0: u16,
    songend: bool,
    spent: u64,
    counts: [u32; At7::Count as usize],
    trace: Option<std::fs::File>,
}

fn new_7v(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_7v(d) {
        return None;
    }
    let smpl_data = read_smpl(m)?;
    let smpl = m.alloc((smpl_data.len() as u32).max(4));
    if smpl == 0 {
        return None;
    }
    m.bus.write_bytes(smpl, &smpl_data);
    make_7v(d, base, smpl, m)
}

/// The TFMX-7V-TFHD player's engine, which is this one: the song part `d` is in guest memory
/// at `base`, its samples are at `smpl` (in the same file, no copy).
pub(super) fn new_7v_tfhd(d: &[u8], base: u32, smpl: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    make_7v(d, base, smpl, m)
}

fn make_7v(d: &[u8], base: u32, smpl: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let _ = d;
    let empty = m.alloc(EMPTY_PATTERN.len() as u32);
    let bss = m.alloc(0x5a0);
    if empty == 0 || bss == 0 {
        return None;
    }
    m.bus.write_bytes(empty, &EMPTY_PATTERN);
    // Check2 gives a module of the old layout its offsets
    if m.bus.r32(base + 0x1d4) == 0 {
        m.bus.w32(base + 0x1d0, 0x800);
        m.bus.w32(base + 0x1d4, 0x400);
        m.bus.w32(base + 0x1d8, 0x600);
    }
    // InitPlayer: the subsongs are the songs with an end, but the last
    let n = (0..32).filter(|k| m.bus.r16(base + 0x140 + 2 * k) != 0).count() as u16;
    m.bus.w8(base + 0xff, 0);
    let vol_tab = (0..64)
        .map(|vol: i32| {
            let mut t = [0u8; 256];
            for (b, e) in t.iter_mut().enumerate() {
                let p = (b as u8 as i8 as i32 * vol) as u16;
                *e = ((p >> 6) as u8) ^ 0x80;
            }
            t
        })
        .collect();
    let mut s = Seven {
        g: Blk([0; 0x96]),
        v: std::array::from_fn(voice7_init),
        p: Blk([0; 0x188]),
        c: Blk([0; 0x26]),
        song_pos: Blk([0; 0xc0]),
        mix: [MixVoice::default(); 4],
        buf_next: 0,
        buf_other: 0,
        silence: 0,
        bss,
        mix_bytes: 0,
        mix_words: 0,
        mix_d3: 0,
        vol_tab,
        mdat: base,
        smpl,
        empty,
        subsongs: if n > 0 { n - 1 } else { 0 },
        sub: 0,
        rows: 0,
        rate0: 12,
        songend: false,
        spent: 0,
        counts: [0; At7::Count as usize],
        trace: std::env::var_os("TFMX_TRACE").and_then(|p| std::fs::File::create(p).ok()),
    };
    s.g.sw(G7_TRACK_LOOP, 0xffff);
    s.g.sw(G7_MASTER, 0x4040);
    s.g.sb(G7_ON, 1);
    s.c.sb(0x15, 0xff);
    let mut hw = Hw { bus: &mut m.bus };
    // InitPlayer's INTENA, then StartInt
    s.reg(&mut hw, 0xdff09a, 0x0780);
    s.spent = 1_100;
    s.start(&mut hw);
    Some(Box::new(s))
}

impl Seven {
    fn at(&mut self, a: At7) {
        self.spent += COST7[a as usize];
        self.counts[a as usize] += 1;
    }

    fn flush(&mut self, hw: &mut Hw, what: &str) {
        hw.wait(self.spent);
        self.spent = 0;
        if let Some(f) = &mut self.trace {
            use std::io::Write;
            let c: Vec<String> =
                self.counts.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| format!("{i}:{n}")).collect();
            let _ = writeln!(f, "{} {} {}", hw.bus.clock, what, c.join(","));
        }
        self.counts = [0; At7::Count as usize];
        if what == "w" {
            self.at(At7::Reg);
        }
    }

    fn reg(&mut self, hw: &mut Hw, a: u32, v: u16) {
        self.flush(hw, "w");
        hw.bus.write_word(a, v);
    }

    fn dmacon(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff096, v);
    }

    fn intena(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff09a, v);
    }

    fn intreq(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff09c, v);
    }

    /// The voice's location, length, period or volume: Paula's for voices 0-3,
    /// the mixer's registers for 4-7.
    fn v_loc(&mut self, hw: &mut Hw, i: usize, a: u32) {
        if i < 4 {
            self.flush(hw, "w");
            hw.loc(i, a);
        } else {
            self.mix[i - 4].loc = a;
        }
    }

    fn v_len(&mut self, hw: &mut Hw, i: usize, w: u16) {
        if i < 4 {
            self.reg(hw, 0xdff0a4 + 16 * i as u32, w);
        } else {
            self.mix[i - 4].len = w;
        }
    }

    fn v_per(&mut self, hw: &mut Hw, i: usize, w: u16) {
        if i < 4 {
            self.reg(hw, 0xdff0a6 + 16 * i as u32, w);
        } else {
            self.mix[i - 4].per = w;
        }
    }

    fn v_vol(&mut self, hw: &mut Hw, i: usize, w: u16) {
        if i < 4 {
            self.reg(hw, 0xdff0a8 + 16 * i as u32, w);
        } else {
            self.mix[i - 4].vol = w;
        }
    }

    /// `$132f2`: the random number, from the beam position.
    fn random(&mut self, hw: &mut Hw) {
        self.flush(hw, "r");
        let (line, h) = hw.bus.beam();
        let d7 = ((line as u16 & 0xff) << 8) | h as u16;
        let s = self.g.w(G7_SEED) ^ d7;
        self.g.sw(G7_SEED, s.wrapping_add(0x4335));
    }

    fn macro_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.g.l(G7_MACROS).wrapping_add(n * 4)))
    }

    fn pattern_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.g.l(G7_PATTERNS).wrapping_add(n * 4)))
    }

    /// The voice of a channel number (the original's table of 16: twice the 8).
    fn voice_of(ch: u8) -> usize {
        (ch & 7) as usize
    }

    // ---- the mixer ----

    /// `$145a0`: a mixer voice's loop from its registers, and the restart of
    /// one DMA was turned on for; with DMA off it stands still.
    fn mix_loop(&mut self, k: usize) {
        self.at(At7::MixLoop);
        let silence = self.silence;
        let m = &mut self.mix[k];
        if !m.on {
            m.step = 0;
            m.restart = true;
            return;
        }
        let (mut a, mut l) = (m.loc, m.len);
        if (l as i16) < 0x20 {
            l = 0xd0;
            a = silence;
        }
        let bytes = (l as u32 & 0x3fff) * 2;
        m.loop_end = a.wrapping_add(bytes);
        m.loop_len = bytes as u16;
        if m.restart {
            m.restart = false;
            m.end = a.wrapping_add(bytes);
            m.pos = bytes.wrapping_neg();
        }
    }

    /// `$14600`: a mixer voice's step and volume from its period (left as they
    /// are for period 0).
    fn mix_rate(&mut self, k: usize) {
        let d3 = self.mix_d3;
        let m = &mut self.mix[k];
        if m.per == 0 {
            return;
        }
        m.tab = (m.vol & 0xff).min(0x3f) as u8;
        let q = divu(d3, m.per) as u32;
        m.step = (q << 5).rotate_left(16);
    }

    /// `$143d2`: the mixer off: channel 3 off, the mixer's voices silent.
    fn mix_off(&mut self, hw: &mut Hw) {
        self.g.sw(G7_DMA_START, 0);
        self.g.sb(G7_MIX, 0);
        self.g.sb(G7_MIX_IRQ, 0);
        self.channel_off(hw, 3);
        let silence = self.silence;
        for m in &mut self.mix {
            m.on = false;
            m.loop_len = 0xd0;
            m.step = 0;
            m.restart = false;
            m.pos = 0xfff0;
            m.end = silence;
            m.loop_end = silence;
        }
        for k in 0..4 {
            self.mix_loop(k);
        }
        self.at(At7::MixClear);
        for a in 0..0x5a0 {
            hw.bus.w8(self.bss + a, 0);
        }
    }

    /// `$142f4`: the mixer on at the rate and buffer length the song asks for:
    /// channel 3 starts on a buffer of 2 words, and its interrupt then.
    fn mix_on(&mut self, hw: &mut Hw) {
        self.at(At7::MixCfg);
        self.intena(hw, 0x0400);
        let rate = self.g.w(G7_RATE).min(0x16);
        if (self.g.w(G7_TEMPO) as i16) < -0x20 {
            self.g.sw(G7_TEMPO, 0xffe0);
        }
        let d3 = 100u32.wrapping_add(self.g.w(G7_TEMPO) as i16 as i32 as u32) as u16;
        let d1 = rate as u32 * 20 * d3 as u32;
        let d1 = (divu(d1, 100) as u32 + 1) as u16 & 0xfffe;
        self.mix_words = d1 >> 1;
        self.mix_bytes = d1;
        let per = mix_period(rate);
        self.mix_d3 = (per as u32) << 11;
        self.v[3].sw(S_PER_OUT, per);
        self.reg(hw, 0xdff0d6, per);
        self.g.sw(G7_DMA_OFF, self.g.w(G7_DMA_OFF) & 0xfff7);
        self.buf_other = self.bss;
        self.buf_next = self.bss + 0x1e0;
        self.silence = self.bss + 0x3c0;
        self.reg(hw, 0xdff0d8, 0);
        self.flush(hw, "w");
        hw.loc(3, self.bss);
        self.reg(hw, 0xdff0d4, 2);
        self.g.sw(G7_DMA_ON, 0x8208);
        self.intena(hw, 0xc400);
        for k in 0..4 {
            self.mix_loop(k);
        }
        if self.g.b(G7_MIX) == 0 {
            self.intreq(hw, 0x8400);
            self.g.sb(G7_MIX, 0xff);
        }
    }

    /// `$14640`: channel 3's interrupt with the mixer on: the next buffer to
    /// Paula, the mix of the four voices into it, then the replayer's tick.
    fn mix_irq(&mut self, hw: &mut Hw) {
        let (next, words) = (self.buf_next, self.mix_words);
        self.flush(hw, "w");
        hw.loc(3, next);
        self.reg(hw, 0xdff0d4, words);
        if self.g.b(G7_ON) != 0 {
            self.at(At7::MixStart);
            for k in 0..4 {
                self.at(At7::MixVoice);
                self.mix_rate(k);
            }
            for k in 0..4 {
                self.mix_loop(k);
            }
            let out = self.buf_next;
            self.buf_next = self.buf_other;
            self.buf_other = out;
            self.mix(hw, out);
            if self.g.b(G7_BUSY) == 0 {
                self.g.sb(G7_MIX_IRQ, 0xff);
                self.interrupt(hw);
            }
        }
        self.at(At7::IrqEnd);
        self.intreq(hw, 0x0400);
    }

    /// The mixing loop: each voice's sample through its volume's table, the
    /// four summed and clipped to a byte.
    fn mix(&mut self, hw: &mut Hw, out: u32) {
        // A rate of 0 gives a buffer of 0 bytes, for which the original's
        // loop (a dbra from the length less 1) mixes 65536 into memory past
        // its buffers; the port mixes none.
        let n = self.mix_bytes as u32;
        for j in 0..n {
            self.at(At7::Sample);
            let mut sum = 0u16;
            for k in 0..4 {
                let m = &self.mix[k];
                let b = hw.bus.r8(m.end.wrapping_add(m.pos as u16 as i16 as u32));
                sum += self.vol_tab[m.tab as usize][b as usize] as u16;
            }
            let o = match sum {
                0..=0x17f => 0x80,
                0x180..=0x27f => (sum as u8).wrapping_add(0x80 - 0x80),
                _ => 0x7f,
            };
            hw.bus.w8(out + j, o);
            for k in 0..4 {
                let m = &mut self.mix[k];
                let (p, x) = m.pos.overflowing_add(m.step);
                let low = (p as u16).wrapping_add(x as u16);
                m.pos = (p & 0xffff_0000) | low as u32;
                if (low as i16) >= 0 {
                    self.counts[At7::Wrap as usize] += 1;
                    self.spent += COST7[At7::Wrap as usize];
                    m.end = m.loop_end;
                    m.pos = (m.pos & 0xffff_0000) | low.wrapping_sub(m.loop_len) as u32;
                }
            }
        }
    }

    // ---- the replayer ----

    /// `$13892` and `$1449c`: the module's tables, the songs' positions, the
    /// mixer off; then `$14486`: the mixer on at the Eagleplayer's rate.
    fn init_data(&mut self, hw: &mut Hw) {
        self.g.sl(G7_MASTER, 0x4040_0000);
        self.g.sb(G7_FADE_DIR, 0);
        self.g.sl(G7_MDAT, self.mdat);
        self.g.sl(G7_SMPL, self.smpl);
        hw.bus.w32(self.smpl, 0);
        self.g.sl(G7_SYNTH, self.smpl);
        let a = self.mdat;
        self.g.sl(G7_STEPS, a.wrapping_add(hw.bus.r32(a + 0x1d0)));
        self.g.sl(G7_PATTERNS, a.wrapping_add(hw.bus.r32(a + 0x1d4)));
        self.g.sl(G7_MACROS, a.wrapping_add(hw.bus.r32(a + 0x1d8)));
        self.g.sl(G7_SFX_TAB, a + 0x200);
        self.p.sw(P_SPEED, 5);
        for k in 0..32 {
            self.song_pos.sw(2 * k, 0);
            self.song_pos.sw(0x40 + 2 * k, 5);
            self.song_pos.sw(0x80 + 2 * k, 0);
        }
        self.silence = self.bss + 0x3c0;
        self.mix_off(hw);
        self.g.sw(G7_RATE, self.rate0);
        self.mix_on(hw);
    }

    /// StartInt: the replayer's init and the subsong's song.
    fn start(&mut self, hw: &mut Hw) {
        // the mixer's tables, built anew each time (measured on the reference)
        self.spent += 914_000;
        self.songend = false;
        self.init_data(hw);
        self.g.sb(G7_START + 1, self.sub as u8);
        self.g.sb(G7_BUSY, 0);
        self.song_play(hw);
    }

    /// `$13600` for each voice, the mixer off, Paula's voices off (`$13612`).
    fn all_off(&mut self, hw: &mut Hw) {
        self.g.sb(G7_ON, 0);
        self.g.sw(G7_DMA_ON, 0);
        for v in &mut self.v {
            v.sb(V_RUN, 0);
            v.sl(V_SFX, 0);
            v.sw(V_SID, 0);
            v.sb(V_RND, 0);
        }
        self.mix_off(hw);
        for ch in 0..4 {
            self.reg(hw, 0xdff0a8 + 16 * ch, 0);
        }
        self.dmacon(hw, 0x000f);
        self.intreq(hw, 0x0780);
        self.intena(hw, 0x0780);
        self.intreq(hw, 0x0780);
        self.c.sb(0x15, 0);
    }

    /// `$137a6`: starts the song of `$1f`.
    fn song_play(&mut self, hw: &mut Hw) {
        self.at(At7::SongPlay);
        self.all_off(hw);
        self.g.sb(G7_ON, 0);
        self.g.sw(G7_SFX7, 0);
        let song = self.g.b(G7_START + 1);
        let d0 = 2 * (song as u32 & 0x1f);
        let a4 = self.mdat + d0;
        let cur = self.g.b(G7_SONG);
        if (cur as i8) >= 0 {
            let o = 2 * (cur as usize & 0x1f);
            self.song_pos.sw(o, self.p.w(P_POS));
            self.song_pos.sb(o + 0x41, self.p.b(P_SPEED + 1));
        }
        let first = hw.bus.r16(a4 + 0x100);
        self.p.sw(P_POS, first);
        self.p.sw(P_FIRST, first);
        self.p.sw(P_LAST, hw.bus.r16(a4 + 0x140));
        let mut d2 = hw.bus.r16(a4 + 0x180);
        if self.g.b(G7_START) & 1 != 0 {
            let o = d0 as usize;
            self.p.sw(P_POS, self.song_pos.w(o));
            d2 = self.song_pos.b(o + 0x41) as u16;
        }
        for k in 0..8 {
            self.p.sl(P_PTR + 4 * k, self.empty);
            self.p.sw(P_NUM + 4 * k, 0xff00);
            self.p.sl(P_STEP + 4 * k, 0);
        }
        self.p.sw(P_SPEED, d2);
        if (song as i8) >= 0 {
            self.track_step(hw);
        }
        self.g.sb(G7_RESTART, 0);
        self.g.sw(G7_SPEED_CNT, 0);
        self.g.sb(G7_TRACK_LOOP, 0xff);
        self.g.sb(G7_SONG, song);
        self.g.sb(G7_START, 0);
        self.g.sw(G7_DMA_ON, 0);
        self.c.sw(0, 0);
        self.c.sb(0x15, 0);
        self.reg(hw, 0xdff09e, 0x00ff);
        self.g.sb(G7_ON, 1);
        if self.g.b(G7_MIX) != 0 {
            self.dmacon(hw, 0x8208);
            self.intena(hw, 0xc400);
            self.intreq(hw, 0x8400);
        }
    }

    /// `$12384`: the track step at the position: its commands, then the
    /// eight tracks' patterns.
    fn track_step(&mut self, hw: &mut Hw) {
        for _ in 0..0x10000 {
            self.at(At7::TrackStep);
            let a0 = self.g.l(G7_STEPS).wrapping_add((self.p.w(P_POS) << 4) as i16 as u32);
            let d0 = hw.bus.r16(a0);
            if d0 == 0xeffe {
                let c = hw.bus.r16(a0 + 2).wrapping_mul(4) as i16;
                let a0 = a0 + 4;
                let pos_on = |s: &mut Self| s.p.sw(P_POS, s.p.w(P_POS).wrapping_add(1));
                match if c <= 0x14 { c / 4 } else { 0 } {
                    0 | 5 => {
                        self.g.sb(G7_ON, 0);
                        return;
                    }
                    1 => {
                        let c = self.g.w(G7_TRACK_LOOP);
                        if c == 0 {
                            self.g.sw(G7_TRACK_LOOP, 0xffff);
                            pos_on(self);
                            continue;
                        }
                        if (c as i16) < 0 {
                            self.g.sw(G7_TRACK_LOOP, hw.bus.r16(a0 + 2).wrapping_sub(1));
                        } else {
                            self.g.sw(G7_TRACK_LOOP, c - 1);
                        }
                        self.p.sw(P_POS, hw.bus.r16(a0));
                    }
                    2 => {
                        let s = hw.bus.r16(a0);
                        self.p.sw(P_SPEED, s);
                        self.g.sw(G7_SPEED_CNT, s);
                        pos_on(self);
                    }
                    3 => {
                        pos_on(self);
                        let r = hw.bus.r16(a0);
                        if (r as i16) >= 0 {
                            self.g.sw(G7_RATE, r);
                        }
                        let t = hw.bus.r16(a0 + 2);
                        if (t as i16) >= 0 {
                            self.g.sw(G7_TEMPO, t as u8 as i8 as u16);
                        }
                        self.mix_on(hw);
                    }
                    4 => {
                        pos_on(self);
                        if self.c.w(0) == 0 {
                            self.c.sw(0, 1);
                            let (b1, b3) = (hw.bus.r8(a0 + 1), hw.bus.r8(a0 + 3));
                            self.fade(b1, b3);
                        }
                    }
                    _ => {
                        // the filter
                        if hw.bus.r16(a0) != 0 {
                            hw.bus.ciaa.pra &= !2;
                        } else {
                            hw.bus.ciaa.pra |= 2;
                        }
                        pos_on(self);
                    }
                }
                continue;
            }
            for k in [0, 2, 4, 6, 8, 0xc, 0xe] {
                self.p.sw(P7_TRACK_WAIT + k, 0);
            }
            for k in 0..8 {
                let w = hw.bus.r16(a0 + 2 * k as u32);
                if k == 7 && self.g.w(G7_SFX7) != 0 {
                    break;
                }
                let o = 4 * k;
                self.p.sw(P_NUM + o, w);
                if (w as i16) >= 0 {
                    let pa = self.pattern_addr(hw, (w >> 8) as u32);
                    self.p.sl(P_PTR + o, pa);
                    self.p.sl(P_STEP + o, 0);
                    self.p.sb(P_LOOP + o, 0xff);
                }
            }
            return;
        }
    }

    /// A fade to `end` every `speed` ticks (at once for 0).
    fn fade(&mut self, speed: u8, end: u8) {
        self.g.sb(G7_FADE_END, end);
        self.g.sb(G7_FADE_CNT, speed);
        self.g.sb(G7_FADE_RESET, speed);
        if speed == 0 {
            self.g.sb(G7_MASTER, end);
        } else {
            self.g.sb(G7_FADE_DIR, 1);
            let m = self.g.b(G7_MASTER);
            if m != end {
                if m > end {
                    self.g.sb(G7_FADE_DIR, 0xff);
                }
                return;
            }
        }
        self.g.sb(G7_FADE_DIR, 0);
        self.c.sw(0, 0);
    }

    /// `$12078`: the patterns, when the speed counter is up.
    fn tracks(&mut self, hw: &mut Hw) {
        self.at(At7::Tracks);
        let c = self.g.w(G7_SPEED_CNT).wrapping_sub(1);
        self.g.sw(G7_SPEED_CNT, c);
        if (c as i16) >= 0 {
            return;
        }
        self.g.sw(G7_SPEED_CNT, self.p.w(P_SPEED));
        self.rows = if self.rows < 0x40 { self.rows + 1 } else { 0 };
        'again: for _ in 0..256 {
            self.g.sb(G7_RESTART, 0);
            for k in 0..8 {
                self.pattern(hw, 4 * k);
                if self.g.b(G7_RESTART) != 0 {
                    continue 'again;
                }
            }
            break;
        }
    }

    /// `$120f6`: pattern `o / 4`'s row.
    fn pattern(&mut self, hw: &mut Hw, o: usize) {
        self.at(At7::Row);
        let num = self.p.b(P_NUM + o);
        if num >= 0x90 {
            if num == 0xfe {
                self.p.sb(P_NUM + o, 0xff);
                self.channel_off(hw, self.p.b(P_XPOSE + o));
            }
            return;
        }
        self.c.sb(0x15, 0xff);
        let w = self.p.b(P_WAIT + o);
        if w != 0 {
            self.p.sb(P_WAIT + o, w - 1);
            return;
        }
        for _ in 0..0x10000 {
            let step = self.p.w(P_STEP + o);
            let a = self.p.l(P_PTR + o).wrapping_add(step.wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a);
            self.g.sl(G7_CMD, cmd);
            let b0 = (cmd >> 24) as u8;
            let step_on = |s: &mut Self| s.p.sw(P_STEP + o, s.p.w(P_STEP + o).wrapping_add(1));
            if b0 < 0xf0 {
                self.at(At7::Note);
                if (0x7f..0xc0).contains(&b0) {
                    self.p.sb(P_WAIT + o, cmd as u8);
                    self.g.sb(G7_CMD + 3, 0);
                }
                let mut d0 = b0.wrapping_add(self.p.b(P_XPOSE + o));
                if b0 < 0xc0 {
                    d0 &= 0x3f;
                }
                self.g.sb(G7_CMD, d0);
                self.note_port(hw, self.g.l(G7_CMD));
                step_on(self);
                if (0x7f..0xc0).contains(&b0) {
                    return;
                }
                continue;
            }
            self.at(At7::PatternCmd);
            let (b1, b2, b3) = ((cmd >> 16) as u8, (cmd >> 8) as u8, cmd as u8);
            let w1 = cmd as u16;
            match b0 & 0xf {
                0 => {
                    self.p.sb(P_NUM + o, 0xff);
                    if self.p.w(P_POS) == self.p.w(P_LAST) {
                        self.songend = true;
                        self.p.sw(P_POS, self.p.w(P_FIRST));
                    } else {
                        self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                    }
                    self.track_step(hw);
                    self.g.sb(G7_RESTART, 0xff);
                    return;
                }
                1 => {
                    let c = self.p.b(P_LOOP + o);
                    if c == 0 {
                        self.p.sb(P_LOOP + o, 0xff);
                        step_on(self);
                        continue;
                    }
                    self.p.sb(P_LOOP + o, if c == 0xff { b1.wrapping_sub(1) } else { c - 1 });
                    self.p.sw(P_STEP + o, w1);
                }
                2 | 8 => {
                    if b0 & 0xf == 8 {
                        self.p.sl(P_RET + o, self.p.l(P_PTR + o));
                        self.p.sw(P_RET_STEP + o, step);
                    }
                    self.p.sb(P_NUM + o, b1);
                    let pa = self.pattern_addr(hw, b1 as u32);
                    self.p.sl(P_PTR + o, pa);
                    self.p.sw(P_STEP + o, w1);
                }
                3 => {
                    self.p.sb(P_WAIT + o, b1);
                    step_on(self);
                    return;
                }
                4 => {
                    self.p.sb(P_NUM + o, 0xff);
                    return;
                }
                5..=7 | 12 => {
                    self.note_port(hw, self.g.l(G7_CMD));
                    step_on(self);
                }
                9 => {
                    // the first pattern's return point, whichever pattern this is
                    self.p.sl(P_PTR, self.p.l(P_RET));
                    self.p.sw(P_STEP, self.p.w(P_RET_STEP));
                    step_on(self);
                }
                10 => {
                    if self.c.w(0) == 0 {
                        self.c.sw(0, 1);
                        self.fade(b1, b3);
                    }
                    step_on(self);
                }
                11 => {
                    let d1 = 4 * (b2 & 7) as usize;
                    self.p.sb(P_NUM + d1, b1);
                    self.p.sb(P_XPOSE + d1, b3);
                    let pa = self.pattern_addr(hw, (b1 & 0x7f) as u32);
                    self.p.sl(P_PTR + d1, pa);
                    self.p.sl(P_STEP + d1, 0);
                    self.p.sb(P_LOOP + d1, 0xff);
                    step_on(self);
                }
                13 => step_on(self),
                _ => {
                    // wait until track b1 is at step b3
                    if self.p.w(P7_TRACK_WAIT + o) == 0 {
                        self.p.sw(P7_TRACK_WAIT + o + 2, 4 * b1 as u16);
                        self.p.sw(P7_TRACK_WAIT + o, b3 as u16);
                    }
                    let t = self.p.w(P7_TRACK_WAIT + o + 2) as i16 as isize;
                    let at = self.p.w((P_STEP as isize + t).clamp(0, 0x186) as usize);
                    if at != self.p.w(P7_TRACK_WAIT + o) {
                        return;
                    }
                    self.p.sw(P7_TRACK_WAIT + o, 0);
                    step_on(self);
                }
            }
        }
    }

    /// `$1346c`: a voice off, unless a sound effect holds it.
    fn channel_off(&mut self, hw: &mut Hw, ch: u8) {
        let i = Self::voice_of(ch);
        if self.v[i].b(V_SFX) != 0 {
            return;
        }
        let (int_off, dma_off) = (self.v[i].w(V_INT_OFF), self.v[i].w(V_DMA_OFF));
        self.intena(hw, int_off);
        self.dmacon(hw, dma_off);
        let v = &mut self.v[i];
        v.sb(V_RUN, 0);
        v.sw(V_SID, 0);
        v.sb(V_RND, 0);
        if i >= 4 {
            self.mix[i - 4].on = false;
            self.mix_loop(i - 4);
        }
    }

    /// `$1330c`: a note, or a note command, for the voice its third byte
    /// names; a note for voice 3 turns the mixer off, one for 4-7 on.
    fn note_port(&mut self, hw: &mut Hw, d0: u32) {
        let saved = self.g.l(G7_CMD);
        self.g.sl(G7_CMD, d0);
        let [b0, b1, b2, b3] = d0.to_be_bytes();
        let ch = b2 & 0xf;
        if ch == 3 {
            if self.g.b(G7_MIX) != 0 {
                self.mix_off(hw);
            }
        } else if ch > 3 && self.g.b(G7_MIX) == 0 {
            self.mix_on(hw);
        }
        let i = Self::voice_of(ch);
        if b0 == 0xfc {
            self.v[i].sb(V_SFX, b1);
            self.v[i].sw(V_LOCK, b3 as u16);
        } else if self.v[i].b(V_SFX) == 0 {
            let v = &mut self.v[i];
            if b0 == 0xf7 {
                v.sb(V_ENV + 3, b1);
                let t = (b2 >> 4) + 1;
                v.sb(V_ENV + 1, t);
                v.sb(V_ENV, t);
                v.sb(V_ENV + 2, b3);
            } else if b0 == 0xf6 {
                let d = b1 & 0xfe;
                v.sb(V_VIB_RESET, d);
                v.sb(V_VIB_TIME, d >> 1);
                v.sb(V_VIB_WIDTH, b3);
                v.sb(V_VIB_WIDTH + 1, 1);
                v.sw(V_VIB_OFS, 0);
            } else if b0 == 0xf5 {
                v.sb(V_KEY, 0);
            } else if b0 >= 0xbf {
                v.sb(V_PORTA_RESET, b1);
                v.sb(V_PORTA_TIME, 1);
                if v.w(V_PORTA_RATE) == 0 {
                    v.sw(V_PORTA_PER, v.w(V_PER));
                }
                v.sw(V_PORTA_RATE, 0);
                v.sb(V_PORTA_RATE + 1, b3);
                let n = b0 & 0x3f;
                v.sb(V_NOTE, n);
                v.sw(V_PER, PERIODS[n as usize]);
            } else {
                v.sw(V_FINE, b3 as i8 as u16);
                v.sb(V_VEL + 1, (b2 >> 4) & 0xf);
                v.sb(S_MACRO_NUM, b1);
                v.sb(V_PREV, v.b(V_NOTE));
                v.sb(V_NOTE, b0);
                let ma = self.macro_addr(hw, b1 as u32);
                let v = &mut self.v[i];
                v.sl(V_MACRO, ma);
                v.sb(V_RUN, 1);
                v.sb(V_KEY, 1);
            }
        }
        self.g.sl(G7_CMD, saved);
    }

    /// `$12756`: voice `i`'s tick: a sound effect's note, the macro, the effects.
    fn voice(&mut self, hw: &mut Hw, i: usize) {
        self.at(At7::VoiceStart);
        let lock = self.v[i].w(V_LOCK);
        if (lock as i16) >= 0 {
            self.v[i].sw(V_LOCK, lock.wrapping_sub(1));
        } else {
            self.v[i].sb(V_SFX, 0);
            self.v[i].sb(V_PRIO, 0);
        }
        let d0 = self.v[i].l(S_PEND);
        if d0 != 0 {
            self.at(At7::Pending);
            self.v[i].sl(S_PEND, 0);
            self.v[i].sb(V_SFX, 0);
            self.note_port(hw, d0);
            self.v[i].sb(V_SFX, self.v[i].b(V_PRIO));
        }
        let run = self.v[i].b(V_RUN);
        if run != 0 {
            if run & 0x80 == 0 {
                self.at(At7::NewNote);
                let v = &mut self.v[i];
                v.sw(V_STEP, 0);
                v.sw(V_WAIT, 0);
                v.sb(V_EFX, 0);
                v.sb(V_LOOP, 0xff);
                v.sb(V_RUN, 0xff);
                let m = self.v[i].w(V_INT_OFF);
                self.intena(hw, m);
                self.intreq(hw, m);
                self.run_macro(hw, i);
            } else {
                let w = self.v[i].w(V_WAIT);
                if w != 0 {
                    self.v[i].sw(V_WAIT, w - 1);
                } else {
                    self.run_macro(hw, i);
                }
            }
        }
        self.effects(hw, i);
    }

    fn step_on(&mut self, i: usize) {
        let s = self.v[i].w(V_STEP).wrapping_add(1);
        self.v[i].sw(V_STEP, s);
    }

    /// `$12c36`: a note's period (`note` + the command's note, the finetune
    /// plus the command's word), out unless a portamento runs.
    fn note_period(&mut self, i: usize, note: u8) {
        let n = self.g.b(G7_CMD + 1).wrapping_add(note) & 0x3f;
        let mut d0 = PERIODS[n as usize] as u32;
        let d1 = self.v[i].w(V_FINE).wrapping_add(self.g.w(G7_CMD + 2));
        if d1 != 0 {
            d0 = (d0 * d1.wrapping_add(0x100) as u32) >> 8;
        }
        self.v[i].sw(V_PER, d0 as u16);
        if self.v[i].w(V_PORTA_RATE) == 0 {
            self.v[i].sw(S_PER_OUT, d0 as u16);
        }
    }

    /// The macro, from its step, until a command ends it for this tick.
    fn run_macro(&mut self, hw: &mut Hw, i: usize) {
        for _ in 0..0x10000 {
            self.at(At7::MacroCmd);
            let a0 = self.v[i].l(V_MACRO).wrapping_add(self.v[i].w(V_STEP).wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a0);
            self.g.sl(G7_CMD, cmd & 0x00ff_ffff);
            let c = (cmd >> 24) as u8;
            match self.macro_cmd(hw, i, if c <= 0x27 { c } else { 0 }) {
                Next7::Read => {}
                Next7::Step => self.step_on(i),
                Next7::Pause => {
                    self.step_on(i);
                    if self.v[i].b(S_OFF_DONE) != 0 {
                        return;
                    }
                    self.v[i].sb(S_OFF_DONE, 0xff);
                }
                Next7::Effects => return,
            }
        }
    }

    /// DMA on or off for a mixer voice: its flag, and its loop at once.
    fn mix_dma(&mut self, i: usize, on: bool) {
        self.mix[i - 4].on = on;
        self.mix_loop(i - 4);
    }

    fn macro_cmd(&mut self, hw: &mut Hw, i: usize, c: u8) -> Next7 {
        let cmd = self.g.l(G7_CMD);
        let (b1, b2, b3) = ((cmd >> 16) as u8, (cmd >> 8) as u8, cmd as u8);
        let w1 = cmd as u16;
        match c {
            0x00 | 0x13 => {
                if c == 0 {
                    let v = &mut self.v[i];
                    v.sb(V_ENV, 0);
                    v.sb(V_VIB_RESET, 0);
                    v.sw(V_PORTA_RATE, 0);
                    v.sb(V_RND, 0);
                    v.sw(V_SID, 0);
                }
                self.step_on(i);
                if i >= 4 {
                    self.mix_dma(i, false);
                    self.v[i].sb(S_OFF_DONE, 0);
                    Next7::Read
                } else if b1 == 0 {
                    let m = self.v[i].w(V_DMA_OFF);
                    self.dmacon(hw, m);
                    Next7::Read
                } else {
                    let m = self.g.w(G7_DMA_OFF) | self.v[i].w(V_DMA_OFF);
                    self.g.sw(G7_DMA_OFF, m);
                    self.v[i].sb(S_OFF_DONE, 0);
                    Next7::Effects
                }
            }
            0x01 => {
                let m = self.v[i].w(V_INT_OFF);
                self.intena(hw, m);
                self.intreq(hw, m);
                self.v[i].sb(V_EFX, b1);
                self.step_on(i);
                if i >= 4 {
                    self.mix_dma(i, true);
                    Next7::Effects
                } else {
                    let m = self.g.w(G7_DMA_ON) | self.v[i].w(V_DMA_ON);
                    self.g.sw(G7_DMA_ON, m);
                    Next7::Read
                }
            }
            0x02 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = cmd.wrapping_add(self.g.l(G7_SMPL));
                self.v[i].sl(V_ADDR, a);
                self.v_loc(hw, i, a);
                self.step_on(i);
                Next7::Read
            }
            0x03 => {
                self.v[i].sw(V_LEN, w1);
                self.v_len(hw, i, w1);
                self.step_on(i);
                Next7::Read
            }
            0x04 => {
                if b1 & 1 != 0 {
                    if self.v[i].b(V_RND_WAIT) != 0 {
                        return Next7::Effects;
                    }
                    self.v[i].sb(V_RND_WAIT, 1);
                    Next7::Step
                } else {
                    self.v[i].sw(V_WAIT, w1);
                    Next7::Pause
                }
            }
            0x05 => self.macro_loop(i, b1, w1),
            0x06 | 0x15 => {
                if c == 0x15 {
                    let v = &mut self.v[i];
                    v.sl(V_RET, v.l(V_MACRO));
                    v.sw(V_RET_STEP, v.w(V_STEP));
                }
                let a = self.macro_addr(hw, (b1 & 0x7f) as u32);
                let v = &mut self.v[i];
                v.sl(V_MACRO, a);
                v.sw(V_STEP, w1);
                v.sb(V_LOOP, 0xff);
                Next7::Read
            }
            0x07 => {
                self.v[i].sb(V_RUN, 0);
                Next7::Effects
            }
            0x08 | 0x09 | 0x1f => {
                let note = match c {
                    0x08 => self.v[i].b(V_NOTE),
                    0x09 => 0,
                    _ => self.v[i].b(V_PREV),
                };
                self.note_period(i, note);
                Next7::Pause
            }
            0x0a => {
                let v = &mut self.v[i];
                v.sb(V_ENV, 0);
                v.sb(V_VIB_RESET, 0);
                v.sw(V_PORTA_RATE, 0);
                v.sb(V_RND, 0);
                v.sw(V_SID, 0);
                v.sb(V_AB_TIME, 0);
                Next7::Step
            }
            0x0b => {
                let v = &mut self.v[i];
                v.sb(V_PORTA_RESET, b1);
                v.sb(V_PORTA_TIME, 1);
                if v.w(V_PORTA_RATE) == 0 {
                    v.sw(V_PORTA_PER, v.w(V_PER));
                }
                v.sw(V_PORTA_RATE, w1);
                Next7::Step
            }
            0x0c => {
                let v = &mut self.v[i];
                v.sb(V_VIB_RESET, b1);
                v.sb(V_VIB_TIME, b1 >> 1);
                v.sb(V_VIB_WIDTH, b3);
                v.sb(V_VIB_WIDTH + 1, 1);
                if v.w(V_PORTA_RATE) != 0 {
                    return Next7::Step;
                }
                v.sw(S_PER_OUT, v.w(V_PER));
                v.sw(V_VIB_OFS, 0);
                self.step_on(i);
                Next7::Read
            }
            0x0d => {
                let v = &mut self.v[i];
                let d0 = v.w(V_VEL).wrapping_mul(3).wrapping_add(w1);
                v.sb(V_VOL, d0 as u8);
                self.step_on(i);
                Next7::Read
            }
            0x0e => {
                self.v[i].sb(V_VOL, b3);
                self.step_on(i);
                Next7::Read
            }
            0x0f => {
                let v = &mut self.v[i];
                v.sb(V_ENV, b2);
                v.sb(V_ENV + 3, b1);
                v.sb(V_ENV + 1, b2);
                v.sb(V_ENV + 2, b3);
                self.step_on(i);
                Next7::Read
            }
            0x10 => {
                if self.v[i].b(V_KEY) != 0 {
                    return self.macro_loop(i, b1, w1);
                }
                self.step_on(i);
                Next7::Read
            }
            0x11 => {
                let v = &mut self.v[i];
                v.sb(V_AB_TIME, b1);
                v.sb(V_AB_RESET, b1);
                let d1 = w1 as i16 as u32;
                v.sl(V_AB_DELTA, d1);
                let a = v.l(V_ADDR).wrapping_add(d1);
                v.sl(V_ADDR, a);
                if v.w(V_SID) == 0 {
                    self.v_loc(hw, i, a);
                } else {
                    v.sl(V_SID_SRC, a);
                }
                self.step_on(i);
                Next7::Read
            }
            0x12 => {
                let d1 = self.v[i].w(V_LEN).wrapping_add(w1);
                self.v[i].sw(V_LEN, d1);
                if self.v[i].w(V_SID) != 0 {
                    self.v[i].sw(V_SID_MASK, d1);
                } else {
                    self.v_len(hw, i, d1);
                }
                self.step_on(i);
                Next7::Read
            }
            0x14 => {
                if self.v[i].b(V_KEY) == 0 {
                    return Next7::Step;
                }
                let l = self.v[i].b(V_LOOP);
                if l == 0 {
                    self.v[i].sb(V_LOOP, 0xff);
                    return Next7::Step;
                }
                self.v[i].sb(V_LOOP, if l == 0xff { b3.wrapping_sub(1) } else { l - 1 });
                Next7::Effects
            }
            0x16 => {
                let v = &mut self.v[i];
                v.sl(V_MACRO, v.l(V_RET));
                v.sw(V_STEP, v.w(V_RET_STEP));
                Next7::Step
            }
            0x17 => {
                self.v[i].sw(V_PER, w1);
                if self.v[i].w(V_PORTA_RATE) == 0 {
                    self.v[i].sw(S_PER_OUT, w1);
                }
                Next7::Step
            }
            0x18 => {
                let a = self.v[i].l(V_ADDR).wrapping_add(cmd);
                self.v[i].sl(V_ADDR, a);
                self.v_loc(hw, i, a);
                let l = self.v[i].w(V_LEN).wrapping_sub(w1 >> 1);
                self.v[i].sw(V_LEN, l);
                self.v_len(hw, i, l);
                self.step_on(i);
                Next7::Read
            }
            0x19 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = self.g.l(G7_SMPL);
                self.v[i].sl(V_ADDR, a);
                self.v_loc(hw, i, a);
                self.v[i].sw(V_LEN, 1);
                self.v_len(hw, i, 1);
                self.step_on(i);
                Next7::Read
            }
            0x1a => {
                self.v[i].sb(V_RUN, 0);
                let m = self.v[i].w(V_INT_ON);
                self.intena(hw, m);
                Next7::Pause
            }
            0x1b => {
                let v = &mut self.v[i];
                v.sb(V_RND_MACRO, b1);
                v.sw(V_RND_SPEED, w1);
                v.sw(S_RND_COUNT, 0x0101);
                self.random_play(hw, i);
                self.v[i].sb(V_RND_WAIT, 1);
                Next7::Step
            }
            0x1c => {
                if b1 >= self.v[i].b(V_NOTE) {
                    return Next7::Step;
                }
                self.v[i].sw(V_STEP, w1);
                Next7::Read
            }
            0x1d => {
                if b1 >= self.v[i].b(V_VOL) {
                    return Next7::Step;
                }
                self.v[i].sw(V_STEP, w1);
                Next7::Read
            }
            0x1e => {
                self.v[i].sb(V_RND_MASK, b1);
                Next7::Step
            }
            0x22 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = cmd.wrapping_add(self.g.l(G7_SMPL));
                self.v[i].sl(V_SID_SRC, a);
                self.v[i].sl(V_ADDR, a);
                let b = self.g.l(G7_SMPL).wrapping_add(self.v[i].l(V_SID_BUF));
                self.v_loc(hw, i, b);
                self.step_on(i);
                Next7::Read
            }
            0x23 => {
                let hi = (cmd >> 16) as u16;
                let l = if hi == 0 { 0x100 } else { hi } >> 1;
                self.v_len(hw, i, l);
                let v = &mut self.v[i];
                v.sw(V_SID, hi.wrapping_sub(1) & 0xff);
                v.sw(V_SID_MASK, w1);
                v.sw(V_LEN, w1);
                self.step_on(i);
                Next7::Read
            }
            0x24 => {
                self.v[i].sl(0x6c, cmd << 8);
                self.step_on(i);
                Next7::Read
            }
            0x25 => {
                let v = &mut self.v[i];
                v.sw(0x70, (cmd >> 16) as u16);
                v.sw(0x72, (cmd >> 16) as u16);
                v.sw(0x74, w1);
                self.step_on(i);
                Next7::Read
            }
            0x26 => {
                self.v[i].sl(0x78, cmd);
                self.step_on(i);
                Next7::Read
            }
            0x27 => {
                let v = &mut self.v[i];
                v.sw(0x84, (cmd >> 16) as u16);
                v.sw(0x86, (cmd >> 16) as u16);
                v.sw(0x82, w1);
                self.step_on(i);
                Next7::Read
            }
            _ => Next7::Step,
        }
    }

    /// `$12baa`: the macro's loop, `b1` times to step `w1`.
    fn macro_loop(&mut self, i: usize, b1: u8, w1: u16) -> Next7 {
        let l = self.v[i].b(V_LOOP);
        if l == 0 {
            self.v[i].sb(V_LOOP, 0xff);
            self.step_on(i);
            return Next7::Read;
        }
        self.v[i].sb(V_LOOP, if l == 0xff { b1.wrapping_sub(1) } else { l - 1 });
        self.v[i].sw(V_STEP, w1);
        Next7::Read
    }

    /// `$12f04`: the voice's effects, then the random player, the fade and the
    /// volume; none of them while the DMA on's flag says so (once if positive).
    fn effects(&mut self, hw: &mut Hw, i: usize) {
        self.at(At7::Effects);
        let e = self.v[i].b(V_EFX);
        if e != 0 {
            if e & 0x80 == 0 {
                self.v[i].sb(V_EFX, 0);
            }
            return;
        }
        if self.v[i].b(V_AB_TIME) != 0 {
            self.at(At7::AddBegin);
            let a = self.v[i].l(V_ADDR).wrapping_add(self.v[i].l(V_AB_DELTA));
            self.v[i].sl(V_ADDR, a);
            if self.v[i].w(V_SID) != 0 {
                self.v[i].sl(V_SID_SRC, a);
            } else {
                self.v_loc(hw, i, a);
            }
            let t = self.v[i].b(V_AB_TIME).wrapping_sub(1);
            self.v[i].sb(V_AB_TIME, t);
            if t == 0 {
                let v = &mut self.v[i];
                v.sb(V_AB_TIME, v.b(V_AB_RESET));
                v.sl(V_AB_DELTA, v.l(V_AB_DELTA).wrapping_neg());
            }
        }
        if self.v[i].w(V_SID) != 0 {
            self.sid(hw, i);
        }
        if self.v[i].b(V_VIB_RESET) != 0 {
            self.at(At7::Vibrato);
            let v = &mut self.v[i];
            let o = v.w(V_VIB_OFS).wrapping_add(v.b(V_VIB_WIDTH) as i8 as u16);
            v.sw(V_VIB_OFS, o);
            let mut d0 = v.w(V_PER) as u32;
            if o != 0 {
                d0 = (d0 * o.wrapping_add(0x800) as u32).wrapping_shl(5) >> 16;
            }
            if v.w(V_PORTA_RATE) == 0 {
                v.sw(S_PER_OUT, d0 as u16);
            }
            let t = v.b(V_VIB_TIME).wrapping_sub(1);
            v.sb(V_VIB_TIME, t);
            if t == 0 {
                v.sb(V_VIB_TIME, v.b(V_VIB_RESET));
                v.sb(V_VIB_WIDTH, (v.b(V_VIB_WIDTH) ^ 0xff).wrapping_add(1));
            }
        }
        if self.v[i].w(V_PORTA_RATE) != 0 {
            let v = &mut self.v[i];
            let t = v.b(V_PORTA_TIME).wrapping_sub(1);
            v.sb(V_PORTA_TIME, t);
            if t == 0 {
                self.at(At7::Porta);
                let v = &mut self.v[i];
                v.sb(V_PORTA_TIME, v.b(V_PORTA_RESET));
                let d1 = v.w(V_PER);
                let mut d0 = v.w(V_PORTA_PER) as u32;
                let reached = if d0 as u16 == d1 {
                    true
                } else if (d0 as u16) < d1 {
                    d0 = (d0 * v.w(V_PORTA_RATE).wrapping_add(0x100) as u32) >> 8;
                    d0 as u16 >= d1
                } else {
                    d0 = (d0 * 0x100u16.wrapping_sub(v.w(V_PORTA_RATE)) as u32) >> 8;
                    d0 as u16 <= d1
                };
                if reached {
                    v.sw(V_PORTA_RATE, 0);
                    d0 = v.w(V_PER) as u32;
                }
                let d0 = d0 as u16 & 0x7ff;
                v.sw(V_PORTA_PER, d0);
                v.sw(S_PER_OUT, d0);
            }
        }
        if self.v[i].b(V_ENV) != 0 {
            self.at(At7::Env);
            let v = &mut self.v[i];
            let c = v.b(V_ENV + 1);
            if c != 0 {
                v.sb(V_ENV + 1, c - 1);
            } else {
                v.sb(V_ENV + 1, v.b(V_ENV));
                let end = v.b(V_ENV + 2) as i8;
                let rate = v.b(V_ENV + 3);
                let vol = v.b(V_VOL) as i8;
                let done = if end > vol {
                    let n = (vol as u8).wrapping_add(rate) as i8;
                    v.sb(V_VOL, n as u8);
                    end <= n
                } else {
                    let n = (vol as u8).wrapping_sub(rate) as i8;
                    v.sb(V_VOL, n as u8);
                    n < 0 || end >= n
                };
                if done {
                    v.sb(V_VOL, end as u8);
                    v.sb(V_ENV, 0);
                }
            }
        }
        self.random_play(hw, i);
    }

    /// `$12f48`: the SID synthesis, into the voice's buffer in the sample
    /// memory. The last byte starts from the byte at address $43 (the original
    /// reads it absolute), not the one it saves.
    fn sid(&mut self, hw: &mut Hw, i: usize) {
        self.at(At7::SidStart);
        let v = &self.v[i];
        let src = v.l(V_SID_SRC);
        let mut d4 = v.l(0x6c);
        let d5 = v.l(0x78);
        let mut a1 = v.l(V_SID_BUF).wrapping_add(self.g.l(G7_SYNTH));
        let n = v.w(V_SID) as u32 + 1;
        let mask = v.w(V_SID_MASK);
        let d3 = v.b(0x76);
        let mut d0: u32 = 0;
        let mut d1 = hw.bus.r8(0x43);
        for _ in 0..n {
            self.at(At7::SidByte);
            d4 = d4.wrapping_add(d5);
            let s = d0.rotate_left(16);
            let (sum, x) = s.overflowing_add(d4);
            d0 = sum.rotate_left(16);
            d0 = (d0 & 0xffff_0000) | (d0 as u16 & mask) as u32;
            let d2 = hw.bus.r8(src.wrapping_add(d0 as u16 as i16 as u32)) as i8;
            let out = if d3 == 0 {
                d2
            } else {
                let c = d1 as i8;
                if d2 == c {
                    d2
                } else if d2 > c {
                    let r = (c as i16) + (d3 as i8 as i16) + x as i16;
                    if !(-128..=127).contains(&r) || d2 <= r as i8 {
                        d2
                    } else {
                        r as i8
                    }
                } else {
                    let r = (c as i16) - (d3 as i8 as i16) - x as i16;
                    if !(-128..=127).contains(&r) || d2 >= r as i8 {
                        d2
                    } else {
                        r as i8
                    }
                }
            };
            if d3 != 0 {
                d1 = out as u8;
            }
            hw.bus.w8(a1, out as u8);
            a1 = a1.wrapping_add(1);
        }
        let v = &mut self.v[i];
        v.sb(V_SID_LAST, d1);
        if d3 != 0 {
            v.sw(0x76, v.w(0x76).wrapping_add(v.w(0x80)));
            let c = v.w(0x7c).wrapping_sub(1);
            v.sw(0x7c, c);
            if c == 0 {
                v.sw(0x7c, v.w(0x7e));
                v.sw(0x80, v.w(0x80).wrapping_neg());
            }
        }
        for (acc, delta, cnt, reset) in [(0x6c, 0x74, 0x70, 0x72), (0x78, 0x82, 0x84, 0x86)] {
            v.sl(acc, v.l(acc).wrapping_add(v.w(delta) as i16 as u32));
            let c = v.w(cnt).wrapping_sub(1);
            v.sw(cnt, c);
            if c == 0 {
                v.sw(cnt, v.w(reset));
                if v.w(reset) != 0 {
                    v.sw(delta, v.w(delta).wrapping_neg());
                }
            }
        }
    }

    /// `$13214`: a random position in the random player's macro, then the fade
    /// and the volume (the original reaches them through its tail).
    fn random_pos(&mut self, hw: &mut Hw, i: usize) {
        self.random(hw);
        let d1 = self.g.b(G7_SEED + 1) & self.v[i].b(V_RND_MASK);
        self.v[i].sw(V_RND_STEP, d1 as u16);
        self.fade_volume(hw, i);
    }

    /// `$130fe`: the random player, then the fade and the volume.
    fn random_play(&mut self, hw: &mut Hw, i: usize) {
        let r = self.v[i].b(V_RND);
        if r == 0 {
            return self.fade_volume(hw, i);
        }
        self.at(At7::Random);
        if r & 0x80 == 0 {
            let n = (self.v[i].b(V_RND_MACRO) & 0x7f) as u32;
            let a = self.macro_addr(hw, n);
            self.v[i].sl(V_RND_PTR, a);
            self.v[i].sw(V_RND_STEP, 0);
            self.v[i].sb(V_RND, 0xff);
            if self.v[i].b(S_RND_MODE) & 1 != 0 {
                self.random_pos(hw, i);
            }
        }
        let c = self.v[i].b(S_RND_COUNT).wrapping_sub(1);
        self.v[i].sb(S_RND_COUNT, c);
        if c != 0 {
            // `$1322a`: pass the period and a lower volume on to the next voice
            if self.v[i].b(S_RND_MODE) & 2 != 0 {
                let d0 = (self.v[i].b(V_RND_SPEED) as u16 * 3) >> 3;
                if d0 as u8 == self.v[i].b(S_RND_COUNT) {
                    let per = self.v[i].w(V_PER);
                    let vol = ((self.v[i].b(V_VOL) as u16 * 5) >> 3) as u8;
                    let j = (i & 4) | ((i + 1) & 3);
                    self.v[j].sb(V_VOL, vol);
                    if per != self.v[j].w(V_PER) {
                        self.v[j].sw(V_PER, per);
                        self.v[j].sw(S_PER_OUT, per);
                        if self.g.b(G7_CMD) & 0x80 != 0 {
                            self.v[j].sb(V_RND_WAIT, 0);
                        }
                    }
                }
            }
            return self.fade_volume(hw, i);
        }
        self.v[i].sb(S_RND_COUNT, self.v[i].b(V_RND_SPEED));
        let mut d0;
        loop {
            let s = self.v[i].w(V_RND_STEP);
            d0 = hw.bus.r8(self.v[i].l(V_RND_PTR).wrapping_add(s as i16 as u32));
            self.g.sb(G7_CMD, d0);
            if d0 != 0 {
                break;
            }
            if s == 0 {
                return self.fade_volume(hw, i);
            }
            self.v[i].sw(V_RND_STEP, 0);
        }
        let n = d0.wrapping_add(self.v[i].b(V_NOTE)) & 0x3f;
        if n == 0 {
            return self.random_pos(hw, i);
        }
        let mut per = PERIODS[n as usize] as u32;
        let f = self.v[i].w(V_FINE);
        if f != 0 {
            per = (per * f.wrapping_add(0x100) as u32) >> 8;
        }
        let per = per as u16;
        if self.v[i].b(S_RND_MODE) & 1 == 0 {
            self.v[i].sw(V_PER, per);
            if self.v[i].w(V_PORTA_RATE) != 0 {
                return self.fade_volume(hw, i);
            }
            self.v[i].sw(S_PER_OUT, per);
            if self.g.b(G7_CMD) & 0x80 != 0 {
                self.v[i].sb(V_RND_WAIT, 0);
            }
            let s = self.v[i].w(V_RND_STEP).wrapping_add(1);
            self.v[i].sw(V_RND_STEP, s);
            return self.fade_volume(hw, i);
        }
        self.random(hw);
        let skip = self.v[i].b(S_RND_MODE) & 4 == 0
            && self.v[i].w(V_RND_STEP) & 3 == 0
            && 0x10 >= self.g.b(G7_SEED + 1);
        if !skip {
            if self.g.b(G7_CMD) & 0x80 != 0 {
                self.v[i].sb(V_RND_WAIT, 0);
            }
            self.v[i].sw(V_PER, per);
            if self.v[i].w(V_PORTA_RATE) == 0 {
                self.v[i].sw(S_PER_OUT, per);
            }
        }
        let s = self.v[i].w(V_RND_STEP).wrapping_add(1);
        self.v[i].sw(V_RND_STEP, s);
        if self.g.b(G7_CMD) & 0x40 == 0 {
            return self.fade_volume(hw, i);
        }
        self.random(hw);
        if 6 >= self.g.b(G7_SEED) {
            return self.fade_volume(hw, i);
        }
        self.random_pos(hw, i)
    }

    /// `$13288`: the fade's step (once for each voice played), then the
    /// voice's volume; with the mixer on, the master volume is channel 3's and
    /// scales the voices of Paula's channels only.
    fn fade_volume(&mut self, hw: &mut Hw, i: usize) {
        self.at(At7::Fade);
        let dir = self.g.b(G7_FADE_DIR);
        if dir != 0 {
            let c = self.g.b(G7_FADE_CNT).wrapping_sub(1);
            self.g.sb(G7_FADE_CNT, c);
            if c == 0 {
                self.g.sb(G7_FADE_CNT, self.g.b(G7_FADE_RESET));
                let m = self.g.b(G7_MASTER).wrapping_add(dir);
                self.g.sb(G7_MASTER, m);
                if self.g.b(G7_FADE_END) == m {
                    self.g.sb(G7_FADE_DIR, 0);
                    self.c.sw(0, 0);
                }
            }
        }
        self.at(At7::Volume);
        let d1 = self.g.b(G7_MASTER) as u16;
        let mut d0 = self.v[i].b(V_VOL) as u16;
        let mix = self.g.b(G7_MIX) != 0;
        if mix {
            self.reg(hw, 0xdff0d8, d1);
            self.at(At7::VolScale);
        }
        if !(mix && i >= 4) && d1 & 0x40 == 0 {
            d0 = d0.wrapping_mul(4).wrapping_mul(d1) >> 6;
        }
        self.v_vol(hw, i, d0);
    }

    /// `$11f38`: the tick.
    fn interrupt(&mut self, hw: &mut Hw) {
        self.at(At7::Tick);
        if self.g.b(G7_BUSY) != 0 {
            return;
        }
        self.g.sb(G7_BUSY, 1);
        let saved = self.g.l(G7_CMD);
        let off = self.g.w(G7_DMA_OFF);
        if off != 0 {
            self.at(At7::OffStart);
            self.dmacon(hw, off);
            for ch in 0..4 {
                if off & 1 << ch != 0 {
                    self.at(At7::OffPer);
                    self.reg(hw, 0xdff0a6 + 16 * ch as u32, 9);
                }
            }
            self.g.sw(G7_DMA_OFF, 0);
        }
        let m = self.g.w(G7_DMA_START);
        self.dmacon(hw, m);
        if self.g.b(G7_ON) != 0 {
            for i in 0..3 {
                self.voice(hw, i);
            }
            if self.g.b(G7_MIX) != 0 {
                for i in 4..8 {
                    self.voice(hw, i);
                }
            } else {
                self.voice(hw, 3);
            }
            if (self.g.b(G7_SONG) as i8) >= 0 {
                self.tracks(hw);
            }
            for i in 0..8 {
                self.at(At7::PerOut);
                let p = self.v[i].w(S_PER_OUT);
                self.v_per(hw, i, p);
            }
            let m = self.g.w(G7_DMA_ON);
            self.dmacon(hw, m);
            self.g.sw(G7_DMA_ON, 0);
        }
        self.g.sb(G7_BUSY, 0);
        self.g.sl(G7_CMD, saved);
    }

    /// `$12a50`: an audio interrupt: the Eagleplayer's display first (cleared
    /// when its row counter is 0), then the mixer's, or a voice's for wait on
    /// DMA (the macro runs again).
    fn irq(&mut self, hw: &mut Hw) {
        self.flush(hw, "t");
        self.at(At7::Irq);
        for i in [0, 1, 2, 4, 5, 6, 7] {
            if self.rows == 0 {
                self.at(At7::ScopeClear);
            }
            if self.v[i].b(V_RUN) != 0 && self.v[i].b(V_EFX) == 0 {
                self.at(At7::ScopeVoice);
            }
        }
        let p = &hw.bus.paula;
        let req = p.intreq & p.intena;
        let i = if req & 0x80 != 0 {
            0
        } else if req & 0x100 != 0 {
            1
        } else if req & 0x200 != 0 {
            2
        } else if self.g.b(G7_MIX) != 0 {
            self.mix_irq(hw);
            self.flush(hw, "e");
            return;
        } else {
            3
        };
        let m = self.v[i].w(V_INT_OFF);
        self.intena(hw, m);
        self.intreq(hw, m);
        self.v[i].sb(V_RUN, 0xff);
        self.flush(hw, "e");
    }
}

impl Replayer for Seven {
    fn format(&self) -> &'static str {
        "TFMX 7V"
    }

    fn title(&self) -> String {
        String::new()
    }

    /// The tick comes with channel 3's interrupt, not DeliTracker's timer.
    fn tick(&mut self, _hw: &mut Hw) {}

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsongs.saturating_sub(1), self.sub)
    }

    /// StopInt, then StartInt for the new subsong (the voices off first).
    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.intena(hw, 0x0780);
        self.intreq(hw, 0x0780);
        for ch in 0..4 {
            self.reg(hw, 0xdff0a8 + 16 * ch, 0);
        }
        self.sub = n;
        self.all_off(hw);
        for ch in 0..4 {
            self.channel_off(hw, ch);
        }
        self.start(hw);
    }

    fn audio_irq(&mut self, hw: &mut Hw) -> bool {
        self.irq(hw);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;

    /// An mdat with the old fixed layout: song 0 at track step 0 (speed 5),
    /// pattern 0 at $A00 on the first track, macro 0 at $B00; `id` is its first
    /// 10 bytes, `macro` macro 0's commands.
    fn mdat(id: &[u8], macro_: &[u32]) -> Vec<u8> {
        let mut d = vec![0u8; 0xc00];
        let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_be_bytes());
        d[..id.len()].copy_from_slice(id);
        d[0x180..0x182].copy_from_slice(&5u16.to_be_bytes());
        put(&mut d, 0x400, 0xa00); // pattern 0, then 1 ($20 apart: not empty)
        put(&mut d, 0x404, 0xa20);
        put(&mut d, 0x600, 0xb00); // macro 0, the last one
        put(&mut d, 0x7fc, 0xb00);
        // track step 0: pattern 0 on the first track, the others off
        for k in 1..8 {
            d[0x800 + 2 * k] = 0xff;
        }
        // pattern 0: note 30, macro 0, velocity 15, voice 0; wait 16; end
        put(&mut d, 0xa00, 0x1e00_f000);
        put(&mut d, 0xa04, 0xf310_0000);
        put(&mut d, 0xa08, 0xf000_0000);
        put(&mut d, 0xa20, 0xf000_0000);
        for (k, &c) in macro_.iter().enumerate() {
            put(&mut d, 0xb00 + 4 * k, c);
        }
        d
    }

    /// DMA off, SetBegin 0, SetLen 16, SetVol $30, AddNote 0, DMA on, Stop.
    const MACRO: [u32; 7] = [0x0000_0000, 0x0200_0000, 0x0300_0010, 0x0e00_0030, 0x0800_0000, 0x0100_0000, 0x0700_0000];

    /// Plays `mdat` (with a 64-byte square wave as smpl.test) for 0.2 s, and
    /// returns the format, the smpl address (LOC0's first value) and the writes.
    fn play(name: &str, mdat: Vec<u8>) -> (String, Vec<(u32, u16, u64)>) {
        play_smpl(name, mdat, "smpl.test")
    }

    /// `play` with the samples in the file `smpl_name`.
    fn play_smpl(name: &str, mdat: Vec<u8>, smpl_name: &str) -> (String, Vec<(u32, u16, u64)>) {
        let dir = std::env::temp_dir().join(format!("amigaplayer-tfmx-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let smpl: Vec<u8> = (0..64).map(|i| if i < 32 { 0x40 } else { 0xc0 }).collect();
        std::fs::write(dir.join(smpl_name), smpl).unwrap();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        let r = e.play_data(&dir.join("mdat.test"), mdat);
        let _ = std::fs::remove_dir_all(&dir);
        r.unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
        (format, e.m.bus.write_log.take().unwrap())
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn pro_plays_a_note_on_paula() {
        let (format, log) = play("pro", mdat(b"TFMX-SONG ", &MACRO));
        assert_eq!(format, "TFMX Pro");
        // InitSound's all off, then the macro: DMA off at once, the sample's
        // start and length, the volume; the period and the DMA on at the ends
        // of the ticks (the first tick's DMACON is 0)
        assert_eq!(&reg(&log, 0xDFF096)[..4], &[0x000F, 0x0000, 0x0001, 0x0000]);
        assert!(reg(&log, 0xDFF096).contains(&0x8201));
        assert_eq!(&reg(&log, 0xDFF0A4)[..1], &[16]);
        assert!(reg(&log, 0xDFF0A8).contains(&0x30));
        assert!(reg(&log, 0xDFF0A6).contains(&super::PERIODS[30]));
        // the 9-line wait before the period's tick-end DMACON
        let per = log.iter().find(|w| w.0 == 0xDFF0A6 && w.1 == super::PERIODS[30]).unwrap().2;
        let on = log.iter().find(|w| w.0 == 0xDFF096 && w.1 == 0x8201).unwrap().2;
        assert!(on >= per + 8 * crate::paula::LINE);
    }

    #[test]
    fn pro_wait_on_dma_counts_audio_interrupts_in_rust() {
        // DMA on, wait on DMA with a count of 1, then SetVol $10 (the first
        // SetVol is $30): the handler clears the request each time, and on the
        // second interrupt (the count goes negative) disables the voice's
        // interrupt and lets the macro run on
        let run = |name: &str, count: u32| {
            let macro_ = [0x0000_0000, 0x0200_0000, 0x0300_0010, 0x0e00_0030, 0x0800_0000, 0x0100_0000, 0x1a00_0000 | count, 0x0e00_0010, 0x0700_0000];
            let (format, log) = play(name, mdat(b"TFMX-SONG ", &macro_));
            assert_eq!(format, "TFMX Pro");
            let count_of = |a, v| reg(&log, a).into_iter().filter(|&x| x == v).count();
            let ena = reg(&log, 0xDFF09A);
            assert!(ena.contains(&0x8080), "{ena:x?}");
            let vol = reg(&log, 0xDFF0A8);
            assert!(vol.contains(&0x10), "the macro ran on after the wait: {vol:x?}");
            (count_of(0xDFF09C, 0x0080), count_of(0xDFF09A, 0x0080))
        };
        // a count of n takes n + 1 interrupts: each clears the request, the
        // last one disables the interrupt (the same once for any count)
        let (req0, dis0) = run("pro-dma0", 0);
        let (req3, dis3) = run("pro-dma3", 3);
        assert_eq!(req3, req0 + 3);
        assert_eq!(dis3, dis0);
    }

    #[test]
    fn v15_plays_a_note_on_paula() {
        let (format, log) = play("15", mdat(b"TFMX \0\0\x01\0\0", &MACRO));
        assert_eq!(format, "TFMX 1.5");
        // DMA off, the sample, the period at once; the volume ($30 at the master
        // volume $3F) and DMA on in the next tick
        assert_eq!(&reg(&log, 0xDFF096)[..2], &[0x0001, 0x8001]);
        assert_eq!(&reg(&log, 0xDFF0A4)[..1], &[16]);
        assert_eq!(&reg(&log, 0xDFF0A6)[..1], &[super::PERIODS[30]]);
        assert!(reg(&log, 0xDFF0A8).contains(&(0x30 * 4 * 0x3f >> 8)));
    }

    #[test]
    fn early_song_without_its_own_samples_plays_as_tfmx_15_from_smpl_set() {
        // an early "(Empty)" song of the fixed tables whose samples are the
        // game's one smpl.set: UADE's TFMX 1.5 player plays it. The volume is
        // not faded to $3F, the note's detune adds to SetNote's period, and
        // DMA on goes out at the macro's command.
        let mut d = mdat(b"TFMX-SONG ", &[0x0000_0000, 0x0200_0000, 0x0300_0010, 0x0e00_0030, 0x0900_0000, 0x0100_0000, 0x0700_0000]);
        d[0x10..0x18].copy_from_slice(b"(Empty) ");
        d[0xa03] = 5; // the note's detune
        let (format, log) = play_smpl("early", d.clone(), "smpl.set");
        assert_eq!(format, "TFMX 1.5");
        assert!(reg(&log, 0xDFF0A8).contains(&0x30));
        assert!(reg(&log, 0xDFF0A6).contains(&(super::PERIODS[0] + 5)));
        assert!(reg(&log, 0xDFF096).contains(&0x8001));
        // with smpl.NAME present the Pro replayer keeps it
        let (format, _) = play_smpl("early-own", d, "smpl.test");
        assert_eq!(format, "TFMX Pro");
    }

    #[test]
    fn early_song_with_its_text_at_0a_plays_as_tfmx_15() {
        // "TFMX-SONG by ERN0/SB ..." (Erno's NAME.tfx beside NAME.sam): the text starts right
        // after the header, which the Pro check refuses (a word at $0C) and CHECK5 too ("SONG");
        // UADE's TFMX player plays it, as the early modules
        let mut d = mdat(b"TFMX-SONG ", &[0x0000_0000, 0x0200_0000, 0x0300_0010, 0x0e00_0030, 0x0900_0000, 0x0100_0000, 0x0700_0000]);
        d[0x0a..0x14].copy_from_slice(b"by ERN0/SB");
        assert!(!super::detect_pro(&d) && !super::detect_15(&d));
        assert_eq!(crate::detect(&d).as_deref(), Some("TFMX 1.5"));
        let dir = std::env::temp_dir().join(format!("amigaplayer-tfmx-test-erno-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let smpl: Vec<u8> = (0..64).map(|i| if i < 32 { 0x40 } else { 0xc0 }).collect();
        std::fs::write(dir.join("Erno.sam"), smpl).unwrap();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        let r = e.play_data(&dir.join("Erno.tfx"), d.clone());
        let _ = std::fs::remove_dir_all(&dir);
        r.unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "TFMX 1.5");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
        let log = e.m.bus.write_log.take().unwrap();
        // played as the early modules: no fade to $3F
        assert!(reg(&log, 0xDFF0A8).contains(&0x30));
        // the editor's text at $10 is still the Pro replayer's
        d[0x0a..0x10].fill(0);
        assert!(super::detect_pro(&d));
    }

    #[test]
    fn pro_takes_modules_with_the_editors_text() {
        // the original takes only four it knows; their TFMX_1.5 refuses them all
        let mut d = mdat(b"TFMX-SONG ", &MACRO);
        d[0x10..0x18].copy_from_slice(b"(Empty) ");
        let (format, log) = play("empty", d);
        assert_eq!(format, "TFMX Pro");
        assert!(reg(&log, 0xDFF0A6).contains(&super::PERIODS[30]));
    }

    /// `MACRO` with the whole 32-word sample: the mixer plays silence for
    /// lengths under 32 words, as the original's does.
    const MACRO32: [u32; 7] = [0x0000_0000, 0x0200_0000, 0x0300_0020, 0x0e00_0030, 0x0800_0000, 0x0100_0000, 0x0700_0000];

    /// `mdat` made a 7 voices song: track step 0 turns the mixer on (12 kHz,
    /// the buffer's length unchanged), step 1 is the old step 0, with the note
    /// on `voice`.
    fn mdat7(voice: u8, macro_: &[u32]) -> Vec<u8> {
        let mut d = mdat(b"TFMX-SONG ", macro_);
        let step: Vec<u8> = d[0x800..0x810].to_vec();
        d[0x810..0x820].copy_from_slice(&step);
        d[0x800..0x810].fill(0);
        d[0x800..0x808].copy_from_slice(&[0xef, 0xfe, 0, 3, 0, 12, 0, 0]);
        d[0x140..0x142].copy_from_slice(&1u16.to_be_bytes()); // the song's last step
        d[0x604..0x608].copy_from_slice(&0xb00u32.to_be_bytes()); // Check2 wants a second macro
        d[0xa02] = voice;
        d
    }

    #[test]
    fn seven_voices_plays_paula_voices() {
        let (format, log) = play("7v0", mdat7(0, &MACRO));
        assert_eq!(format, "TFMX 7V");
        // the mixer on channel 3 at 12 kHz: its period, then buffers of 240 bytes
        assert!(reg(&log, 0xDFF0D6).contains(&298));
        assert!(reg(&log, 0xDFF0D4).contains(&120));
        // the note on Paula's channel 0: DMA off, the sample, DMA on and the
        // period at the end of the tick
        assert!(reg(&log, 0xDFF096).contains(&0x0001));
        assert!(reg(&log, 0xDFF096).contains(&0x8201));
        assert_eq!(&reg(&log, 0xDFF0A4)[..1], &[16]);
        assert!(reg(&log, 0xDFF0A6).contains(&super::PERIODS[30]));
        // the master volume goes to channel 3 with the mixer on, and scales
        // only Paula's voices (64: unscaled)
        assert!(reg(&log, 0xDFF0D8).contains(&0x40));
        assert!(reg(&log, 0xDFF0A8).contains(&0x30));
    }

    #[test]
    fn seven_voices_mixes_voices_4_to_7() {
        let dir = std::env::temp_dir().join(format!("amigaplayer-tfmx-test-7v4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let smpl: Vec<u8> = (0..64).map(|i| if i < 32 { 0x40 } else { 0xc0 }).collect();
        std::fs::write(dir.join("smpl.test"), smpl).unwrap();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        let r = e.play_data(&dir.join("mdat.test"), mdat7(4, &MACRO32));
        let _ = std::fs::remove_dir_all(&dir);
        r.unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "TFMX 7V");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        // nothing of the note reaches Paula's channels 0-2
        assert!(!log.iter().any(|w| (0xDFF0A0..0xDFF0D0).contains(&w.0) && w.0 & 0xf == 4));
        // the last buffer handed to channel 3 holds the square wave at volume
        // $30 (unscaled by the master volume): (+-$40 * $30) >> 6, the silent
        // voices adding nothing; 0 where the sample's first long was, which the
        // original's init clears (for the one-shot loop)
        let hi = log.iter().rev().find(|w| w.0 == 0xDFF0D0).unwrap().1 as u32;
        let lo = log.iter().rev().find(|w| w.0 == 0xDFF0D2).unwrap().1 as u32;
        let buf = (hi << 16) | lo;
        let b: Vec<u8> = (0..240).map(|i| e.m.bus.r8(buf + i)).collect();
        assert!(b.iter().all(|&x| x == 0x30 || x == 0xd0 || x == 0), "{b:02x?}");
        assert!(b.contains(&0x30) && b.contains(&0xd0));
    }

    #[test]
    fn sample_file_names() {
        assert_eq!(super::smpl_names("mdat.Tune"), ["smpl.Tune"]);
        assert_eq!(super::smpl_names("MDAT.tune.xz"), ["smpl.tune.xz", "smpl.tune"]);
        // NostalgicPlayer's names: NAME.tfx beside NAME.sam (or NAME.smpl)
        assert_eq!(super::smpl_names("X-Out - Title.tfx"), ["smpl.X-Out - Title.tfx", "X-Out - Title.smpl", "X-Out - Title.sam"]);
        assert_eq!(super::smpl_names("Tune.mdat.gz"), ["smpl.Tune.mdat.gz", "Tune.smpl", "Tune.sam"]);
        assert_eq!(super::smpl_names("tune"), ["smpl.tune"]);
        assert_eq!(super::smpl_names(".tfx"), ["smpl..tfx"]);
        // a PowerPacked mdat beside an unpacked smpl (aminet tfmxtunes8, B.C.Kid)
        assert_eq!(super::smpl_names("MDAT.BC-Kid_Title.pp"), ["smpl.BC-Kid_Title.pp", "smpl.BC-Kid_Title"]);
        assert_eq!(super::smpl_names("mdat.tune.PP.xz"), ["smpl.tune.PP.xz", "smpl.tune.PP", "smpl.tune"]);
        assert_eq!(super::smpl_names("mdat..pp"), ["smpl..pp"]);
    }

    #[test]
    fn a_module_without_its_samples_names_them() {
        // the library: the format, and each name the samples were looked for under
        let e = crate::Song::open("mdat.lost", &mdat(b"TFMX-SONG ", &MACRO), 48000).err();
        let wanted: Vec<String> = ["smpl.lost", "smpl.set"].iter().map(|s| s.to_string()).collect();
        assert_eq!(e, Some(crate::Error::MissingCompanion { format: "TFMX Pro".into(), wanted: wanted.clone() }));
        assert_eq!(crate::detect(&mdat(b"TFMX-SONG ", &MACRO)).as_deref(), Some("TFMX Pro"));
        // given them, it plays
        let smpl = vec![0x40u8; 64];
        assert!(crate::Song::open_with("mdat.lost", &mdat(b"TFMX-SONG ", &MACRO), &[("smpl.lost", &smpl)], 48000).is_ok());
        // the engine's error, on disk (a name of NostalgicPlayer's form adds its two)
        let dir = std::env::temp_dir().join(format!("amigaplayer-tfmx-test-lost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        let r = e.play_data(&dir.join("Lost.tfx"), mdat(b"TFMX-SONG ", &MACRO));
        let _ = std::fs::remove_dir_all(&dir);
        let err = r.unwrap_err();
        let m = err.downcast_ref::<crate::engine::MissingCompanion>().unwrap();
        assert_eq!(m.format, "TFMX Pro");
        assert_eq!(m.wanted, ["smpl.Lost.tfx", "Lost.smpl", "Lost.sam", "smpl.set"]);
        assert!(err.to_string().starts_with("TFMX Pro module, but a file it needs beside it is missing (looked for: smpl.Lost.tfx, "));
        // a module nothing recognises is still that
        assert_eq!(crate::Song::open("x", b"nothing at all", 48000).err(), Some(crate::Error::NotRecognised));
    }
}
