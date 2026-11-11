//! Synthesis, ported from NostalgicPlayer's Synthesis player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the Synth 4.0 Eagleplayer ("Synthesis Replayer V4.0 player
//! module V1.21", adapted by DEFECT), which plays both kinds of module with one
//! replayer of its own, through ENPP_Poke* and ENPP_DMAMask: a "Synth4.x" file,
//! and a "Synth.Pc" file (a game's replayer with a Synth4.2 module in it), whose
//! module it finds by the replayer's `lea` to it. Like the original, the port
//! keeps its variables, the voices' synth buffers and its tables in guest
//! memory in the layout of the original's data hunk, and plays from the module
//! in guest memory: what the original reads past its tables, or from a voice's
//! variables with the wrong register, it reads too.
//!
//! Embedded data: `synthesis_hunk.bin` (540 bytes) is taken from the data hunk
//! of the Synth 4.0 Eagleplayer file, on the project owner's decision (data
//! only, no player code): the non-zero bytes of the voices' synth buffers, which
//! are what an earlier session left there (an effect that reads past what a
//! note copied into its buffer works on them: `Human Strength`'s Alising,
//! `Labyrinth Theme`), and of the hunk after the pointers, which a voice plays
//! from InitSound's location before its first note (`Digital Man`, `Super
//! Module 2`, `Yohl`). They lie in guest memory where the original has them
//! (`HUNK_SEGMENTS`). The rest of the hunk starts as zeros (the voices on, their
//! buffer flags 1), not with the leftovers in the original's variables; and a
//! voice that plays on past the hunk's end plays what follows the port's
//! allocation, not what follows the original's hunk.
//!
//! Where NostalgicPlayer differs from what the original writes to Paula, this
//! follows the original:
//! * a row writes a note's period, and for a new instrument its sample's (or
//!   the synth buffer's) location, length and volume; then DMA off and on for
//!   the voices with a new instrument, and a sample's loop (or the empty
//!   sample); an envelope-generator instrument writes no location at its note,
//!   and its buffers' locations but no length on the next ticks; note `$7F`
//!   restarts the voice on the empty sample at volume 0;
//! * a voice without an instrument or a note writes volume 0 every tick, and the
//!   end of its ADSR table takes its instrument away;
//! * the row's effect 8 (speed) also leaves the note untransposed, 9 the
//!   instrument, and B the portamento off; 5, 6 and 7 do nothing;
//! * the envelope generator counts to its table's length inclusive and copies
//!   whole 16-byte blocks; the synth effects work on the buffer from its start,
//!   as the original's loops run (Rotate 1 to the end position, Negator wrapping
//!   at end - start, Shaker 2 negating the byte after the one it matches), and
//!   the effect and argument overrides of the row apply to the instrument's
//!   effect only if it has one; there is no FM drum effect; the slow-motion
//!   counter is the voice's own byte;
//! * a row's arpeggio reads the voice's vibrato and synth effect from the
//!   period table (the original's register then points there); the voice-1
//!   to 3 buffer flags a synth note sets are 256 bytes apart (in the period
//!   table and past it), so only voice 0's is reset;
//! * the song ends past its last position and goes on at its restart
//!   position; nothing is range-checked, byte and word arithmetic and signed
//!   compares as in the original (a division that overflows leaves its
//!   dividend);
//! * the writes come when the original's do (`Cost`).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Synthesis", detect, new };

include!("synthesis_tables.rs");

// The original's data hunk ($F4C bytes), mirrored in guest memory.
const HUNK_SIZE: u32 = 0xf4c;
/// The non-zero bytes of the original's data hunk in the voices' synth buffers
/// ($148 to $948) and after the pointers ($AE6 to the end), 540 bytes, and where
/// each run of them lies (offset, length). See the file header.
static HUNK_BYTES: &[u8; 540] = include_bytes!("synthesis_hunk.bin");
const HUNK_SEGMENTS: [(u32, usize); 10] = [
    (0x148, 0x10),
    (0x248, 0x40),
    (0x348, 0x40),
    (0x448, 0x40),
    (0x548, 0x10),
    (0x648, 0x40),
    (0x748, 0x40),
    (0x848, 0x40),
    (0xae8, 0x7a),
    (0xf4a, 2),
];
/// Each voice's synth effect position (a byte each).
const SYN_POS: u32 = 0x000;
const COUNTER: u32 = 0x005;
const ROW: u32 = 0x006;
const SPEED_COPY: u32 = 0x007;
/// A voice is played only if its byte here is not 0 (all are $FF).
const VOICE_ON: u32 = 0x008;
/// Set to 8 for a voice's new note (for the Eagleplayer's display).
const NOTE_FLAG: u32 = 0x00c;
const POSITION: u32 = 0x010;
/// The subsong's 14 bytes, as InitSound copies them.
const SONG: u32 = 0x016;
const SPEED: u32 = 0x01a;
const ROWS: u32 = 0x01b;
const LAST: u32 = 0x01e;
const RESTART: u32 = 0x020;
/// The position's 4 longs: start row, sound transpose, note transpose.
const POS_ROW: u32 = 0x024;
/// The track rows' 4 longs: note, instrument, arpeggio and effect, argument.
const TRACK_ROW: u32 = 0x034;
const VIBRATO: u32 = 0x048;
/// Each voice's two 256-byte synth buffers.
const BUF1: u32 = 0x148;
const BUF2: u32 = 0x548;
/// 4 bytes of zeros, a sample's loop of 2 words.
const EMPTY: u32 = 0x948;
/// Which synth buffer each voice plays (a byte each).
const USE_BUF: u32 = 0x958;
/// The voices' 32-byte variables.
const VOICES: u32 = 0x95c;
const PERIOD_TABLE: u32 = 0x9dc;
/// The pointers to the module's parts, as InitSound computes them.
const POINTERS: u32 = 0xab6;
/// What InitSound points the voices at.
const INIT_LOC: u32 = 0xae2;

const P_LENGTHS: u32 = 0x04;
const P_EGC: u32 = 0x08;
const P_ADSR: u32 = 0x0c;
const P_INSTRUMENTS: u32 = 0x10;
const P_ARPEGGIOS: u32 = 0x14;
const P_SUBSONGS: u32 = 0x18;
const P_WAVEFORMS: u32 = 0x1c;
const P_POSITIONS: u32 = 0x20;
const P_TRACKS: u32 = 0x24;
const P_SAMPLES: u32 = 0x28;

// A voice's variables.
const V_NOTE: u32 = 0x00;
const V_PREV_NOTE: u32 = 0x01;
const V_SLIDE_SPEED: u32 = 0x02;
const V_INSTRUMENT: u32 = 0x03;
const V_VOLUME: u32 = 0x04;
const V_ADSR_ON: u32 = 0x05;
const V_ADSR_POS: u32 = 0x06;
const V_VIB_DELAY: u32 = 0x08;
const V_VIB_POS: u32 = 0x09;
const V_EGC_MODE: u32 = 0x0b;
const V_EGC_POS: u32 = 0x0c;
const V_SLIDE: u32 = 0x0e;
const V_ARP_LEN: u32 = 0x10;
const V_PORTA_SPEED: u32 = 0x11;
const V_PORTA: u32 = 0x12;
const V_ARP_POS: u32 = 0x14;
const V_FX_OFF: u32 = 0x16;
const V_FX: u32 = 0x17;
const V_ARG1: u32 = 0x18;
const V_ARG2: u32 = 0x19;
const V_ARG3: u32 = 0x1a;
const V_EGC_OFF: u32 = 0x1b;
const V_NEW_VOLUME: u32 = 0x1c;
const V_SLOW: u32 = 0x1d;

// An instrument (28 bytes).
const I_WAVEFORM: u32 = 0x00;
const I_SYNTH: u32 = 0x01;
const I_LENGTH: u32 = 0x02;
const I_REPEAT: u32 = 0x04;
const I_VOLUME: u32 = 0x06;
const I_PORTA: u32 = 0x07;
const I_ADSR: u32 = 0x08;
const I_ADSR_TABLE: u32 = 0x09;
const I_ADSR_LEN: u32 = 0x0a;
const I_ARP_START: u32 = 0x0d;
const I_ARP_LEN: u32 = 0x0e;
const I_ARP_REPEAT: u32 = 0x0f;
const I_EFFECT: u32 = 0x10;
const I_ARG1: u32 = 0x11;
const I_ARG2: u32 = 0x12;
const I_ARG3: u32 = 0x13;
const I_VIB_DELAY: u32 = 0x14;
const I_VIB_SPEED: u32 = 0x15;
const I_VIB_LEVEL: u32 = 0x16;
const I_EGC_OFFSET: u32 = 0x17;
const I_EGC_MODE: u32 = 0x18;
const I_EGC_TABLE: u32 = 0x19;
const I_EGC_LEN: u32 = 0x1a;

/// The Eagleplayer's check: "Synth4." and any version at the start, or a
/// "Synth.Pc" replayer whose module is at the `lea` before its init code (the
/// offset from the file's start).
fn module_offset(d: &[u8]) -> Option<usize> {
    let be32 = |o: usize| super::be32(d, o);
    if be32(0) != 0x5379_6e74 {
        return None;
    }
    if be32(4) >> 8 == 0x0068_342e {
        return Some(0);
    }
    if be32(4) != 0x682e_5063 {
        return None;
    }
    // the init code (adda.l #$cc,a0; move.l a0,(a1)+; moveq #0,d1; move.b ...)
    // within the first 101 longs, a `lea d16(pc),a0` before it
    let at = (0..101)
        .map(|k| 4 * k)
        .find(|&o| be32(o) == 0xd1fc_0000 && be32(o + 4) == 0x00cc_22c8 && be32(o + 8) == 0x7200_123a)?;
    if at < 4 || super::be16(d, at - 4) != 0x41fa {
        return None;
    }
    Some(at - 2 + super::be16(d, at - 2) as usize)
}

/// How many longs the check of a "Synth.Pc" file scans before it finds the
/// init code.
fn scan_steps(d: &[u8]) -> u64 {
    (0..101u64)
        .find(|&k| {
            let o = 4 * k as usize;
            super::be32(d, o) == 0xd1fc_0000 && super::be32(d, o + 4) == 0x00cc_22c8
        })
        .unwrap_or(101)
}

fn detect(d: &[u8]) -> bool {
    module_offset(d).is_some_and(|o| o + 0xcc <= d.len())
}

pub struct Synthesis {
    /// The module (its "Synth4.x" header) in guest memory.
    base: u32,
    /// The original's data hunk, in guest memory.
    mem: u32,
    sub: u16,
    last_sub: u16,
    songend: bool,
    title: String,
    /// 68000 cycles the original spends since its last host call (`Cost`).
    spent: f64,
    fit: Option<std::io::BufWriter<std::fs::File>>,
    counts: Vec<(u16, u32)>,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let off = module_offset(d)?;
    // at the top, where the Eagleplayer's hunks lie: low memory stays as the
    // reference has it for a sample whose address is the module's offset alone
    // (`Super Module 2`)
    let top = std::mem::replace(&mut m.alloc_top, true);
    let mem = m.alloc(HUNK_SIZE);
    m.alloc_top = top;
    if mem == 0 {
        return None;
    }
    // the synth buffers and what follows the pointers start as the original's
    // file has them (`HUNK_BYTES`); the rest starts as zeros (as
    // NostalgicPlayer's variables do), not with what an earlier session left
    // in the original's variables. The voices' buffer flags start at 1, as a
    // synth note sets them: only voice 0's is ever set again (see
    // `voice_row`), so the others keep their start value.
    let mut at = 0;
    for &(o, n) in &HUNK_SEGMENTS {
        for (k, &b) in HUNK_BYTES[at..at + n].iter().enumerate() {
            m.bus.w8(mem + o + k as u32, b);
        }
        at += n;
    }
    for v in 0..4 {
        m.bus.w8(mem + VOICE_ON + v, 0xff);
        m.bus.w8(mem + USE_BUF + v, 1);
    }
    for (k, &p) in PERIODS.iter().enumerate() {
        m.bus.w16(mem + PERIOD_TABLE + 2 * k as u32, p);
    }
    let h = &d[off..];
    let fit = std::env::var_os("SYN_FIT")
        .and_then(|p| std::fs::File::create(p).ok())
        .map(std::io::BufWriter::new);
    let samples = h.get(0x10).copied().unwrap_or(0) as u64;
    let mut r = Synthesis {
        base: base + off as u32,
        mem,
        sub: 0,
        // SubSongRange: 0 to the count less one
        last_sub: (h.get(0x13).copied().unwrap_or(0) as u16).wrapping_sub(1) & 0xff,
        songend: false,
        title: h.get(0x24..0x40).map_or(String::new(), |t| {
            t.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
        }),
        spent: 0.0,
        fit,
        counts: Vec::new(),
    };
    let mut hw = Hw { bus: &mut m.bus };
    let scan = if off != 0 { INIT_PC + INIT_SCAN * scan_steps(d) } else { 0 };
    hw.wait(INIT_START + INIT_SAMPLE * samples + scan);
    r.init_sound(0, &mut hw);
    hw.wait(INIT_END);
    Some(Box::new(r))
}

/// Colour clocks from the start of the open replayer to InitSound's first write
/// (measured on the Eagleplayer's write logs): EP_NewModuleInfo adds up the
/// sample lengths, the check scans a "Synth.Pc" file by longs (24 colour clocks
/// a long, counted on its code, and 76 more on that path).
const INIT_START: u64 = 8712;
const INIT_SAMPLE: u64 = 12;
const INIT_PC: u64 = 76;
const INIT_SCAN: u64 = 24;
/// Colour clocks from InitSound's last write to the end of the Eagleplayer's
/// start (1677 on every module of the corpus), where the output and the timer
/// begin.
const INIT_END: u64 = 1677;
/// Colour clocks from the end of the start (`INIT_END`) to the next InitSound's
/// first write when DeliTracker steps the subsong (EndSound, then InitSound
/// again), measured on the Eagleplayer: 5727 between the two first writes on
/// every module; after it, `INIT_END` again to where the output restarts.
const SUBSONG_START: u64 = 5727 - 395 - INIT_END;
/// Colour clocks between InitSound's writes of the voices' locations.
const INIT_LOC_GAP: [u64; 4] = [120, 132, 143, 0];

impl Replayer for Synthesis {
    fn format(&self) -> &'static str {
        "Synthesis"
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
        hw.wait(SUBSONG_START);
        self.init_sound(n, hw);
        hw.wait(INIT_END);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let _ = writeln!(f, "tick {}", hw.bus.clock);
        }
        self.spent = 0.0;
        self.counts.clear();
        self.t(Cost::TickStart);
        let c = self.rb(hw, COUNTER).wrapping_add(1);
        self.wb(hw, COUNTER, c);
        if c as i8 >= self.rb(hw, SPEED) as i8 {
            self.t(Cost::RowCall);
            self.row(hw);
        }
        self.t(Cost::FxStart);
        for v in 0..4 {
            self.tv(Cost::FxCall0, v);
            self.voice_fx(v, hw);
        }
    }
}

/// The original's register writes, for the costs of its calls to the host.
#[derive(Clone, Copy, PartialEq)]
enum W {
    Loc,
    Len,
    Per,
    Vol,
    Dma,
}

macro_rules! costs {
    ($($name:ident = $c:expr,)*) => {
        /// The original's code paths, each with the 68000 cycles it takes (as the
        /// emulator counts them). A write waits half the cycles of the paths since
        /// the last host call, as the emulator's clock follows the CPU. The figures
        /// are a whole-number solution of the Eagleplayer's gaps between host calls
        /// (half-cycle write logs): exact on every gap of 53 modules over 120 s
        /// (2.2 million) and on the 20 later subsongs held out of the solve; only
        /// sums along a path mean something (a path's cost can sit in a neighbour's).
        /// A path the corpus never takes keeps an estimate. Where the original has
        /// its code once for each voice, voice 0's entry is followed by the others'.
        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        enum Cost { $($name,)* }
        const COSTS: &[f64] = &[$($c as f64,)*];
        #[allow(dead_code)]
        const COST_NAMES: &[&str] = &[$(stringify!($name),)*];
    };
}

costs! {
    TickStart = -239,
    RowCall = 313,
    RowNewPos = 121,
    RowSongEnd = 63,
    RowPosCopy = 120,
    RowTracks = 313,
    VoiceOn0 = 126,
    VoiceOn1 = 125,
    VoiceOn2 = 125,
    VoiceOn3 = 125,
    VoiceOff0 = 20,
    VoiceOff1 = 20,
    VoiceOff2 = 20,
    VoiceOff3 = 20,
    VrEntry = 125,
    VrEffect = 384,
    VrFx1 = 12,
    VrFx2 = 44,
    VrFx3 = 20,
    VrFx4Set = 8,
    VrFx4 = 12,
    VrFx5 = 8,
    VrFx6 = 8,
    VrFx8Set = 17,
    VrFx8 = 19,
    VrFx9 = 12,
    VrFxA = 8,
    VrFxB = 12,
    VrFxC = 12,
    VrFxD = 8,
    VrFxE = 8,
    VrFxF = 12,
    VrNote0 = -111,
    VrNote = 227,
    VrNoteNoTr = 22,
    VrCut = 10,
    VrCutAfter = -35,
    VrCutLoc = -35,
    VrCutEnd = 0,
    VrNoInstr = -321,
    VrBitLoop = 31,
    VrInstr = 312,
    VrInstrNoTr = -4,
    VrInstrB = 24,
    VrAdsrOn = 16,
    VrSampleNoLen = -194,
    VrSample = 122,
    VrSampleSum = 41,
    VrSampleLen = -88,
    VrSampleVolNew = 161,
    VrSampleVolNewAfter = 4,
    VrSampleVol = 151,
    VrSample7 = 10,
    VrSampleEndNew = -117,
    VrSampleEnd = -133,
    VrSynth = 190,
    VrSynthFx = 92,
    VrSynthEgcOff = -14,
    VrSynthLen0 = 4,
    VrSynthCopy = 30,
    VrSynthNegSetup = 4,
    VrSynthNeg = 22,
    VrSynthEgc = 205,
    VrSynthLen = 20,
    VrSynthVolNewAfter = -9,
    VrSynthVolNew = 161,
    VrSynthVolAfter = 1,
    VrSynthVol = 151,
    VrSynthEnd = -162,
    VoiceNext0 = 144,
    VoiceNext1 = 146,
    VoiceNext2 = 146,
    VoiceNext3 = 146,
    RowDma = 163,
    RowNoDma = 171,
    RowEnd = -238,
    LoopVoice0 = 44,
    LoopVoice1 = 44,
    LoopVoice2 = 44,
    LoopVoice3 = 44,
    LoopCall0 = 240,
    LoopCall1 = 240,
    LoopCall2 = 240,
    LoopCall3 = 240,
    LoopSynth = -51,
    LoopNoRepeat = -30,
    LoopEmpty = 118,
    LoopEmptyEnd = 0,
    EmptyLen = -120,
    LoopSample = 199,
    LoopSampleSum = 20,
    LoopLen = -100,
    LoopReturn = -44,
    FxStart = 238,
    FxCall0 = 120,
    FxCall1 = 120,
    FxCall2 = 120,
    FxCall3 = 120,
    FxNoInstr = -87,
    FxNoInstrEnd = 1,
    FxInstrArp = 83,
    FxNoNote = 10,
    FxNoNoteEnd = 0,
    FxArpStep = 92,
    FxArpWrap = 43,
    FxArpInc = 49,
    FxPeriods = 83,
    FxTrackArp = 122,
    FxSynthNone = 26,
    FxSynthEgcOn = 72,
    FxSynthEgcDisabled = 10,
    FxSynthEgcOff = 111,
    FxSynthOff = 10,
    FxSynthCall = 111,
    SfxEntry = 111,
    SfxCmp = 25,
    SfxUnknown = 10,
    SfxFxOverride = 4,
    SfxNone = 10,
    SfxOverride = 12,
    SfxSlow = 20,
    SfxSet = 65,
    SfxRotate1Run = 25,
    SfxRotate1 = 25,
    SfxRotate1Iter = 22,
    SfxRotate2Run = 22,
    SfxRotate2Go = 0,
    SfxRotate2Slow = 13,
    SfxRotate2 = 21,
    SfxRotate2Iter = 29,
    SfxAlienGo = 17,
    SfxAlienSlow = 0,
    SfxAlien = 40,
    SfxAlienIter = 30,
    SfxNegatorRun = 45,
    SfxNegatorGo = 0,
    SfxNegatorSlow = 0,
    SfxNegator = 18,
    SfxNegatorWrap = 28,
    SfxPolyNegRun = 87,
    SfxPolyNegZero = 10,
    SfxPolyNegGo = 0,
    SfxPolyNegSlow = -1,
    SfxPolyNeg = 33,
    SfxPolyNegWrap = 28,
    SfxShaker1Go = 76,
    SfxShaker1Slow = -14,
    SfxShaker1 = 40,
    SfxShaker1Iter = 22,
    SfxShaker2Go = 0,
    SfxShaker2Slow = -12,
    SfxShaker2 = 40,
    SfxShaker2Iter = 35,
    SfxShakerWrap = 8,
    SfxShaker2Neg = 98,
    SfxAmfLfoWrap = 8,
    SfxAmfLfoGo = 0,
    SfxAmfLfoSlow = 0,
    SfxAmfLfo = 40,
    SfxLaserGo = 0,
    SfxLaserSlow = -32,
    SfxLaser = 48,
    SfxLaserWrap = 16,
    SfxOctFx1Hold = 8,
    SfxOctFx1 = -23,
    SfxOctFx1Iter = 52,
    SfxOctFx2Go = 0,
    SfxOctFx2Slow = 0,
    SfxOctFx2 = 40,
    SfxOctFx2Iter = 24,
    SfxAlisingGo = -15,
    SfxAlisingSlow = 18,
    SfxAlising = -44,
    SfxAlisingUp = 61,
    SfxAlisingDown = 64,
    SfxAlisingIter = 62,
    SfxEgFx1Wrap = 18,
    SfxEgFx1Go = 98,
    SfxEgFx1Slow = 0,
    SfxEgFx1 = -8,
    SfxEgFx1Up = 88,
    SfxEgFx1Down = 91,
    SfxEgFx1Iter = 47,
    SfxEgFx2Go = 0,
    SfxEgFx2Slow = 0,
    SfxEgFx2 = 40,
    SfxEgFx2Up = 88,
    SfxEgFx2Down = 91,
    SfxEgFx2Iter = 47,
    SfxChangerGo = 0,
    SfxChangerSlow = 0,
    SfxChanger = 40,
    SfxChangerUp = 74,
    SfxChangerDown = 77,
    SfxChangerIter = 47,
    SfxExit = 111,
    FxPortaSame = 13,
    FxPorta = 138,
    FxPortaDiv = 138,
    FxVib = 118,
    FxVibDelay = -71,
    FxVibNeg = -6,
    FxVibRun = 189,
    FxPer = 207,
    FxAfterPer = -34,
    FxAdsr = 63,
    FxAdsrEnd = -103,
    FxAdsrEndEnd = -1,
    FxAdsrNewVol = -10,
    FxAdsrStep = 166,
    FxEgcDisabled = 10,
    FxEgcBuf2 = 10,
    FxEgcOnce = 51,
    FxEgcRepeat = 41,
    FxEgc = 40,
    FxEgcRun = 175,
    FxEgcCopy = 90,
    FxEgcNegSetup = 4,
    FxEgcNeg = 22,
    FxEgcEnd = -42,
    // the host calls, before and after the write, and the Eagleplayer's
    // compares of the voice (before the volume's call, after the others')
    LocPre = 565,
    LocPost = 59,
    LenPre = 306,
    LenPost = -399,
    PerPre = 434,
    PerPost = -39,
    VolPre = 608,
    VolPost = -251,
    DmaPre = 382,
    DmaPost = -190,
    LocCh1 = 23,
    LocCh2 = 46,
    LocCh3 = 46,
    LenCh1 = 23,
    LenCh2 = 46,
    LenCh3 = 46,
    PerCh1 = 23,
    PerCh2 = 46,
    PerCh3 = 46,
    VolCh1 = 23,
    VolCh2 = 30,
    VolCh3 = 36,
    VolClamp = 4,
}

/// A signed byte, sign-extended to a word.
fn ext(b: u8) -> u16 {
    b as i8 as u16
}

impl Synthesis {
    fn t(&mut self, c: Cost) {
        self.tn(c, 1);
    }

    /// The cost of voice `v`'s copy of a path the original has once for each voice (`c` is voice 0's).
    fn tv(&mut self, c: Cost, v: usize) {
        self.tk(c as usize + v, 1);
    }

    fn tn(&mut self, c: Cost, n: u32) {
        self.tk(c as usize, n);
    }

    fn tk(&mut self, k: usize, n: u32) {
        self.spent += COSTS[k] * n as f64;
        if self.fit.is_some() && n != 0 {
            match self.counts.iter_mut().find(|e| e.0 as usize == k) {
                Some(e) => e.1 += n,
                None => self.counts.push((k as u16, n)),
            }
        }
    }

    /// The time the Eagleplayer's compares of the voice take (its chains of
    /// `cmpa.l #$dff0a0,a5`...).
    fn ch_cost(&mut self, ch: usize, w: W) {
        const CH: [[Cost; 3]; 4] = [
            [Cost::LocCh1, Cost::LocCh2, Cost::LocCh3],
            [Cost::LenCh1, Cost::LenCh2, Cost::LenCh3],
            [Cost::PerCh1, Cost::PerCh2, Cost::PerCh3],
            [Cost::VolCh1, Cost::VolCh2, Cost::VolCh3],
        ];
        if ch > 0 {
            self.t(CH[w as usize][ch - 1]);
        }
    }

    /// Lets the time pass that the original's code takes up to a write of kind
    /// `w`, and counts the host call's own time after it for the next write.
    fn wait_for(&mut self, hw: &mut Hw, w: W) {
        let pre = match w {
            W::Loc => Cost::LocPre,
            W::Len => Cost::LenPre,
            W::Per => Cost::PerPre,
            W::Vol => Cost::VolPre,
            W::Dma => Cost::DmaPre,
        };
        self.t(pre);
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let s: Vec<String> = self.counts.iter().map(|(k, n)| format!("{}:{n}", COST_NAMES[*k as usize])).collect();
            let _ = writeln!(f, "w {} {}", hw.bus.clock + (self.spent.max(0.0) / 2.0) as u64, s.join(" "));
        }
        // a host call starts the count again: an odd cycle left over is lost
        hw.wait((self.spent.max(0.0) / 2.0) as u64);
        self.spent = 0.0;
        self.counts.clear();
        self.t(match w {
            W::Loc => Cost::LocPost,
            W::Len => Cost::LenPost,
            W::Per => Cost::PerPost,
            W::Vol => Cost::VolPost,
            W::Dma => Cost::DmaPost,
        });
    }

    /// ENPP_PokeVol through the Eagleplayer's routine, which makes it at most 64.
    fn poke_vol(&mut self, hw: &mut Hw, ch: usize, v: u32) {
        let v = if v as u16 >= 0x41 {
            self.t(Cost::VolClamp);
            0x40
        } else {
            v as u16
        };
        self.ch_cost(ch, W::Vol);
        self.wait_for(hw, W::Vol);
        hw.vol(ch, v);
    }

    fn poke_loc(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.wait_for(hw, W::Loc);
        hw.loc(ch, a);
        self.ch_cost(ch, W::Loc);
    }

    fn poke_len(&mut self, hw: &mut Hw, ch: usize, n: u16) {
        self.wait_for(hw, W::Len);
        hw.len(ch, n);
        self.ch_cost(ch, W::Len);
    }

    fn poke_per(&mut self, hw: &mut Hw, ch: usize, p: u16) {
        self.wait_for(hw, W::Per);
        hw.per(ch, p);
        self.ch_cost(ch, W::Per);
    }

    fn rb(&self, hw: &Hw, o: u32) -> u8 {
        hw.bus.r8(self.mem + o)
    }

    fn wb(&self, hw: &mut Hw, o: u32, v: u8) {
        hw.bus.w8(self.mem + o, v)
    }

    fn rw(&self, hw: &Hw, o: u32) -> u16 {
        hw.bus.r16(self.mem + o)
    }

    fn ww(&self, hw: &mut Hw, o: u32, v: u16) {
        hw.bus.w16(self.mem + o, v)
    }

    fn ptr(&self, hw: &Hw, p: u32) -> u32 {
        hw.bus.r32(self.mem + POINTERS + p)
    }

    /// A voice's variable.
    fn vb(&self, hw: &Hw, v: usize, o: u32) -> u8 {
        self.rb(hw, VOICES + 32 * v as u32 + o)
    }

    fn vwb(&self, hw: &mut Hw, v: usize, o: u32, x: u8) {
        self.wb(hw, VOICES + 32 * v as u32 + o, x)
    }

    fn vw(&self, hw: &Hw, v: usize, o: u32) -> u16 {
        self.rw(hw, VOICES + 32 * v as u32 + o)
    }

    fn vww(&self, hw: &mut Hw, v: usize, o: u32, x: u16) {
        self.ww(hw, VOICES + 32 * v as u32 + o, x)
    }

    /// Instrument `n` (1-based, as a byte less one, times 28 as a word).
    fn instrument(&self, hw: &Hw, n: u8) -> u32 {
        self.ptr(hw, P_INSTRUMENTS).wrapping_add(n.wrapping_sub(1) as u32 * 0x1c)
    }

    /// The start of sample `n`: the lengths of the ones before it added up.
    fn sample(&mut self, hw: &Hw, n: u8, each: Cost) -> u32 {
        let (mut a, l) = (self.ptr(hw, P_SAMPLES), self.ptr(hw, P_LENGTHS));
        for k in 0..n as u32 {
            a = a.wrapping_add(hw.bus.r32(l + 4 * k));
        }
        self.tn(each, n as u32);
        a
    }

    /// InitSound: the pointers to the module's parts, the vibrato table, the
    /// voices cleared and pointed at the empty location, and the subsong's
    /// speed, track length and first position.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        let mut a = 0u8;
        for k in 0..64 {
            self.wb(hw, VIBRATO + k, a);
            a = a.wrapping_add(2);
        }
        a = a.wrapping_add(1);
        for k in 0..128 {
            a = a.wrapping_sub(2);
            self.wb(hw, VIBRATO + 64 + k, a);
        }
        a = a.wrapping_sub(1);
        for k in 0..64 {
            self.wb(hw, VIBRATO + 192 + k, a);
            a = a.wrapping_add(2);
        }
        let b = self.base;
        let r8 = |hw: &Hw, o: u32| hw.bus.r8(b + o) as u32;
        let r16 = |hw: &Hw, o: u32| hw.bus.r16(b + o) as u32;
        let mut p = b.wrapping_add(0xcc);
        let parts = [
            r8(hw, 0x10) * 0x1c,
            r8(hw, 0x10) << 2,
            r8(hw, 0x14) << 7,
            r8(hw, 0x15) << 8,
            r8(hw, 0x12) * 0x1c,
            0x100,
            (r8(hw, 0x13) + 1) * 0xe,
            r8(hw, 0x11) << 8,
            r16(hw, 0x08) << 4,
            (r16(hw, 0x0a) + 0x40) << 2,
        ];
        for (k, &len) in parts.iter().enumerate() {
            hw.bus.w32(self.mem + POINTERS + 4 * k as u32, p);
            p = p.wrapping_add(len);
        }
        hw.bus.w32(self.mem + POINTERS + 4 * parts.len() as u32, p);
        for v in 0..4 {
            for o in V_FX_OFF..=V_SLOW {
                self.vwb(hw, v, o, 0);
            }
        }
        for ch in 0..4 {
            hw.loc(ch, self.mem + INIT_LOC);
            hw.wait(INIT_LOC_GAP[ch]);
        }
        for o in 0..0x78 {
            self.wb(hw, VOICES + o, 0);
        }
        let s = self.ptr(hw, P_SUBSONGS).wrapping_add(n as u32 * 0xe);
        for k in 0..14 {
            let x = hw.bus.r8(s + k);
            self.wb(hw, SONG + k, x);
        }
        let speed = self.rb(hw, SPEED);
        self.wb(hw, SPEED_COPY, speed);
        let first = self.rw(hw, SPEED + 2);
        self.ww(hw, POSITION, first);
        self.wb(hw, COUNTER, speed);
        let rows = self.rb(hw, ROWS);
        self.wb(hw, ROW, rows);
    }

    /// A new row (the original's routine at $862).
    fn row(&mut self, hw: &mut Hw) {
        self.wb(hw, COUNTER, 0);
        let mut d0 = self.rb(hw, ROW) as u32;
        if self.rb(hw, ROW) as i8 >= self.rb(hw, ROWS) as i8 {
            self.t(Cost::RowNewPos);
            self.wb(hw, ROW, 0);
            if self.rw(hw, POSITION) as i16 > self.rw(hw, LAST) as i16 {
                self.t(Cost::RowSongEnd);
                let r = self.rw(hw, RESTART);
                self.ww(hw, POSITION, r);
                self.songend = true;
            }
            self.t(Cost::RowPosCopy);
            let pos = self.rw(hw, POSITION).wrapping_add(1);
            self.ww(hw, POSITION, pos);
            d0 = ((pos as i16 as i32 - 1) << 4) as u32;
            let a = self.ptr(hw, P_POSITIONS).wrapping_add(d0);
            for k in 0..16 {
                let x = hw.bus.r8(a.wrapping_add(k));
                self.wb(hw, POS_ROW + k, x);
            }
        }
        self.t(Cost::RowTracks);
        let tracks = self.ptr(hw, P_TRACKS);
        let d1 = (self.rb(hw, ROW) as u32) << 2;
        for v in 0..4 {
            d0 = ((d0 & 0xffff_0000) | self.rw(hw, POS_ROW + 4 * v) as u32) << 2;
            let x = hw.bus.r32(tracks.wrapping_add(d0).wrapping_add(d1));
            hw.bus.w32(self.mem + TRACK_ROW + 4 * v, x);
        }
        let mut d5 = 0u16;
        for v in 0..4 {
            if self.rb(hw, VOICE_ON + v as u32) != 0 {
                self.tv(Cost::VoiceOn0, v);
                let d0 = hw.bus.r32(self.mem + TRACK_ROW + 4 * v as u32);
                let d3 = hw.bus.r32(self.mem + POS_ROW + 4 * v as u32);
                self.voice_row(v, d0, d3, &mut d5, hw);
            } else {
                self.tv(Cost::VoiceOff0, v);
            }
            self.tv(Cost::VoiceNext0, v);
        }
        if d5 != 0 {
            self.t(Cost::RowDma);
            self.wait_for(hw, W::Dma);
            hw.dmacon(d5 & 0xf);
            self.wait_for(hw, W::Dma);
            hw.dmacon(0x8000 | (d5 & 0xf));
            // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
        } else {
            self.t(Cost::RowNoDma);
        }
        let row = self.rb(hw, ROW).wrapping_add(1);
        self.wb(hw, ROW, row);
        self.t(Cost::RowEnd);
        for v in 0..4 {
            self.tv(Cost::LoopVoice0, v);
            if self.rb(hw, VOICE_ON + v as u32) != 0 && d5 & 1 << v != 0 {
                self.tv(Cost::LoopCall0, v);
                self.sample_loop(v, hw);
            }
        }
    }

    /// One voice's row (the original's routine at $9C8): d0 is the track row,
    /// d3 the position's long; the voices with a new instrument are set in d5.
    fn voice_row(&mut self, v: usize, mut d0: u32, d3: u32, d5: &mut u16, hw: &mut Hw) {
        self.t(Cost::VrEntry);
        let d6 = 1u16 << v;
        let fx = ((d0 & 0xfff) >> 8) as u8;
        let arg = d0 as u8;
        self.vwb(hw, v, V_VOLUME, 0);
        self.vwb(hw, v, V_SLIDE_SPEED, 0);
        if fx != 0 {
            self.t(Cost::VrEffect);
            for o in V_FX_OFF..=V_NEW_VOLUME {
                self.vwb(hw, v, o, 0);
            }
            match fx {
                1 => {
                    self.t(Cost::VrFx1);
                    self.vwb(hw, v, V_SLIDE_SPEED, arg);
                }
                2 => {
                    self.t(Cost::VrFx2);
                    self.vww(hw, v, V_ADSR_POS, arg as u16);
                    self.vwb(hw, v, V_ADSR_ON, 1);
                }
                3 => {
                    self.t(Cost::VrFx3);
                    self.vww(hw, v, V_EGC_POS, arg as u16);
                    self.vwb(hw, v, V_EGC_MODE, 1);
                }
                4 => {
                    self.t(Cost::VrFx4);
                    if arg as i8 > 0 && arg <= 0x40 {
                        self.t(Cost::VrFx4Set);
                        self.wb(hw, ROWS, arg);
                    }
                }
                // they set a register the note then overwrites
                5 => self.t(Cost::VrFx5),
                6 => self.t(Cost::VrFx6),
                8 => {
                    self.t(Cost::VrFx8);
                    if arg as i8 > 0 && arg <= 0x10 {
                        self.t(Cost::VrFx8Set);
                        self.wb(hw, SPEED_COPY, arg);
                        self.wb(hw, SPEED, arg);
                    }
                }
                9 => {
                    self.t(Cost::VrFx9);
                    self.vwb(hw, v, V_FX_OFF, arg);
                }
                0xa => {
                    self.t(Cost::VrFxA);
                    self.vwb(hw, v, V_FX, arg);
                }
                0xb => {
                    self.t(Cost::VrFxB);
                    self.vwb(hw, v, V_ARG1, arg);
                }
                0xc => {
                    self.t(Cost::VrFxC);
                    self.vwb(hw, v, V_ARG2, arg);
                }
                0xd => {
                    self.t(Cost::VrFxD);
                    self.vwb(hw, v, V_ARG3, arg);
                }
                0xe => {
                    self.t(Cost::VrFxE);
                    self.vwb(hw, v, V_EGC_OFF, arg);
                }
                0xf => {
                    self.t(Cost::VrFxF);
                    self.vwb(hw, v, V_NEW_VOLUME, arg);
                }
                _ => {}
            }
        }
        let mut d1 = (d0 >> 24) as u8;
        if d1 == 0 {
            self.t(Cost::VrNote0);
            return;
        }
        if d1 == 0x7f {
            self.t(Cost::VrCut);
            self.poke_vol(hw, v, 0);
            self.t(Cost::VrCutAfter);
            self.vwb(hw, v, V_VOLUME, 0);
            *d5 |= d6;
            self.empty_loop(v, hw);
            self.t(Cost::VrCutEnd);
            return;
        }
        self.t(Cost::VrNote);
        if fx != 8 {
            d1 = d1.wrapping_add(d3 as u8);
        } else {
            self.t(Cost::VrNoteNoTr);
        }
        let n = self.vb(hw, v, V_NOTE);
        self.vwb(hw, v, V_PREV_NOTE, n);
        self.vwb(hw, v, V_NOTE, d1);
        let per = self.rw(hw, PERIOD_TABLE + 2 * d1 as u32);
        self.poke_per(hw, v, per);
        let mut ins = (d0 >> 16) as u8;
        if ins == 0 {
            self.t(Cost::VrNoInstr);
            self.tn(Cost::VrBitLoop, v as u32 + 1);
            self.wb(hw, NOTE_FLAG + v as u32, 8);
            return;
        }
        self.t(Cost::VrInstr);
        if fx != 9 {
            ins = ins.wrapping_add((d3 >> 8) as u8);
        } else {
            self.t(Cost::VrInstrNoTr);
        }
        self.vwb(hw, v, V_INSTRUMENT, ins);
        // subq.l and mulu.w: instrument 0 is $FFFF instruments on
        let a1 = self.ptr(hw, P_INSTRUMENTS).wrapping_add((ins as u16).wrapping_sub(1) as u32 * 0x1c);
        self.tn(Cost::VrBitLoop, v as u32 + 1);
        self.wb(hw, NOTE_FLAG + v as u32, 8);
        self.vwb(hw, v, V_ADSR_ON, 0);
        self.vww(hw, v, V_ADSR_POS, 0);
        for o in 8..0x10 {
            self.vwb(hw, v, o, 0);
        }
        self.vwb(hw, v, V_ARP_POS, 0);
        let ib = |hw: &Hw, o: u32| hw.bus.r8(a1.wrapping_add(o));
        let porta = ib(hw, I_PORTA);
        self.vwb(hw, v, V_PORTA_SPEED, porta);
        let al = ib(hw, I_ARP_LEN);
        self.vwb(hw, v, V_ARP_LEN, al);
        self.vww(hw, v, V_PORTA, ext(porta));
        if fx == 0xb {
            self.t(Cost::VrInstrB);
            for o in 0x10..0x14 {
                self.vwb(hw, v, o, 0);
            }
        }
        let vd = ib(hw, I_VIB_DELAY);
        self.vwb(hw, v, V_VIB_DELAY, vd);
        if ib(hw, I_ADSR) != 0 {
            self.t(Cost::VrAdsrOn);
            self.vwb(hw, v, V_ADSR_ON, 1);
        }
        let length = hw.bus.r16(a1.wrapping_add(I_LENGTH));
        let new_vol = self.vb(hw, v, V_NEW_VOLUME);
        let vol = ib(hw, I_VOLUME);
        if ib(hw, I_SYNTH) != 0 {
            self.t(Cost::VrSynth);
            let wave = self.ptr(hw, P_WAVEFORMS).wrapping_add((ib(hw, I_WAVEFORM) as u32) << 8);
            if ib(hw, I_EFFECT) != 0 {
                self.t(Cost::VrSynthFx);
                self.vwb(hw, v, V_SLOW, 0);
                self.wb(hw, SYN_POS + v as u32, 0);
            }
            // the voice number shifted by 8 where it is used as a byte offset
            self.wb(hw, USE_BUF + 256 * v as u32, 1);
            let mode = ib(hw, I_EGC_MODE);
            self.vwb(hw, v, V_EGC_MODE, mode);
            if mode == 0 {
                self.t(Cost::VrSynthEgcOff);
                self.vwb(hw, v, V_SLOW, 0);
                let buf = self.mem + BUF1 + 256 * v as u32;
                self.poke_loc(hw, v, buf);
                let mut n = length >> 2;
                if n == 0 {
                    self.t(Cost::VrSynthLen0);
                    n = 1;
                }
                let n = (n - 1) & 0x3f;
                for k in 0..=n as u32 {
                    let x = hw.bus.r32(wave.wrapping_add(4 * k));
                    hw.bus.w32(buf + 4 * k, x);
                }
                self.tn(Cost::VrSynthCopy, n as u32 + 1);
                let neg = ib(hw, I_EGC_OFFSET);
                if neg != 0 {
                    self.t(Cost::VrSynthNegSetup);
                    self.negate(hw, buf, neg as u32);
                    self.tn(Cost::VrSynthNeg, neg as u32);
                }
            } else {
                self.t(Cost::VrSynthEgc);
            }
            self.poke_len(hw, v, length >> 1);
            if new_vol == 0 {
                self.t(Cost::VrSynthVol);
                self.poke_vol(hw, v, vol as u32);
                self.t(Cost::VrSynthVolAfter);
                self.vwb(hw, v, V_VOLUME, vol);
            } else {
                self.t(Cost::VrSynthVolNew);
                self.poke_vol(hw, v, new_vol as u32);
                self.t(Cost::VrSynthVolNewAfter);
                self.vwb(hw, v, V_VOLUME, new_vol);
            }
            self.t(Cost::VrSynthEnd);
            *d5 |= d6;
            return;
        }
        if length == 0 {
            self.t(Cost::VrSampleNoLen);
            return;
        }
        self.t(Cost::VrSample);
        self.vwb(hw, v, V_SLOW, 0);
        self.wb(hw, SYN_POS + v as u32, 0);
        let sn = ib(hw, I_WAVEFORM) & 0x3f;
        let a = self.sample(hw, sn, Cost::VrSampleSum);
        self.poke_loc(hw, v, a);
        self.t(Cost::VrSampleLen);
        self.poke_len(hw, v, length >> 1);
        if new_vol != 0 {
            self.t(Cost::VrSampleVolNew);
            self.poke_vol(hw, v, new_vol as u32);
            self.t(Cost::VrSampleVolNewAfter);
        } else {
            self.t(Cost::VrSampleVol);
            self.poke_vol(hw, v, vol as u32);
        }
        if sn == 7 {
            // the sample number where NostalgicPlayer has it, not the effect
            self.t(Cost::VrSample7);
            d0 &= 0x3f;
            self.vwb(hw, v, V_VOLUME, d0 as u8);
        } else {
            self.t(if new_vol != 0 { Cost::VrSampleEndNew } else { Cost::VrSampleEnd });
            self.vwb(hw, v, V_VOLUME, if new_vol != 0 { new_vol } else { vol });
        }
        *d5 |= d6;
    }

    fn negate(&self, hw: &mut Hw, a: u32, n: u32) {
        for k in 0..n {
            let x = hw.bus.r8(a + k);
            hw.bus.w8(a + k, x.wrapping_neg());
        }
    }

    /// The empty sample: 2 words of zeros.
    fn empty_loop(&mut self, v: usize, hw: &mut Hw) {
        self.t(Cost::VrCutLoc);
        self.poke_loc(hw, v, self.mem + EMPTY);
        self.t(Cost::EmptyLen);
        self.poke_len(hw, v, 2);
    }

    /// The loop of a voice's new sample, after its DMA on (the original's
    /// routine at $CD4).
    fn sample_loop(&mut self, v: usize, hw: &mut Hw) {
        let a0 = self.instrument(hw, self.vb(hw, v, V_INSTRUMENT));
        if hw.bus.r8(a0.wrapping_add(I_SYNTH)) != 0 {
            self.t(Cost::LoopSynth);
            return;
        }
        let rep = hw.bus.r16(a0.wrapping_add(I_REPEAT));
        if rep == 0 {
            self.t(Cost::LoopNoRepeat);
            return;
        }
        if rep == 2 {
            self.t(Cost::LoopEmpty);
            self.poke_loc(hw, v, self.mem + EMPTY);
            self.t(Cost::EmptyLen);
            self.poke_len(hw, v, 2);
            self.t(Cost::LoopEmptyEnd);
            return;
        }
        self.t(Cost::LoopSample);
        let sn = hw.bus.r8(a0) & 0x3f;
        let a = self.sample(hw, sn, Cost::LoopSampleSum);
        let a = a.wrapping_add(hw.bus.r16(a0.wrapping_add(I_LENGTH)) as u32);
        self.poke_loc(hw, v, a);
        self.t(Cost::LoopLen);
        self.poke_len(hw, v, rep >> 1);
        self.t(Cost::LoopReturn);
    }

    /// One voice's tick (the original's routine at $E82).
    fn voice_fx(&mut self, v: usize, hw: &mut Hw) {
        let d0 = hw.bus.r32(self.mem + TRACK_ROW + 4 * v as u32);
        let ins = self.vb(hw, v, V_INSTRUMENT);
        if ins == 0 {
            self.t(Cost::FxNoInstr);
            self.poke_vol(hw, v, 0);
            self.t(Cost::FxNoInstrEnd);
            return;
        }
        let arp = (d0 >> 12) as u8 & 0xf;
        let mut d1 = self.vb(hw, v, V_NOTE);
        let mut d7 = self.vb(hw, v, V_PREV_NOTE);
        // what the original's a0 points at for the effects and the vibrato
        let a0;
        if arp == 0 {
            self.t(Cost::FxInstrArp);
            a0 = self.instrument(hw, ins);
            if d1 == 0 {
                self.t(Cost::FxNoNote);
                self.poke_vol(hw, v, 0);
                self.t(Cost::FxNoNoteEnd);
                return;
            }
            let ib = |hw: &Hw, o: u32| hw.bus.r8(a0.wrapping_add(o));
            let len = ib(hw, I_ARP_LEN).wrapping_add(ib(hw, I_ARP_REPEAT));
            if len != 0 {
                self.t(Cost::FxArpStep);
                let pos = self.vb(hw, v, V_ARP_POS);
                let i = ib(hw, I_ARP_START).wrapping_add(pos);
                let d4 = hw.bus.r8(self.ptr(hw, P_ARPEGGIOS).wrapping_add(i as u32));
                if len == pos {
                    self.t(Cost::FxArpWrap);
                    let l = ib(hw, I_ARP_LEN);
                    self.vwb(hw, v, V_ARP_POS, l);
                } else {
                    self.t(Cost::FxArpInc);
                    self.vwb(hw, v, V_ARP_POS, pos.wrapping_add(1));
                }
                d1 = d1.wrapping_add(d4);
                d7 = d7.wrapping_add(d4);
            }
            self.t(Cost::FxPeriods);
        } else {
            self.t(Cost::FxTrackArp);
            let i = (arp << 4).wrapping_add(self.rb(hw, COUNTER));
            let d4 = hw.bus.r8(self.ptr(hw, P_ARPEGGIOS).wrapping_add(i as u32));
            d1 = d1.wrapping_add(d4);
            d7 = d7.wrapping_add(d4);
            a0 = self.mem + PERIOD_TABLE;
        }
        let mut d2 = self.rw(hw, PERIOD_TABLE + 2 * d1 as u32);
        let d4 = self.rw(hw, PERIOD_TABLE + 2 * d7 as u32);
        let ib = |hw: &Hw, o: u32| hw.bus.r8(a0.wrapping_add(o));
        if ib(hw, I_EFFECT) == 0 {
            self.t(Cost::FxSynthNone);
        } else if ib(hw, I_EGC_MODE) != 0 && self.vb(hw, v, V_EGC_OFF) != 1 {
            self.t(Cost::FxSynthEgcOn);
        } else {
            self.t(if ib(hw, I_EGC_MODE) != 0 { Cost::FxSynthEgcDisabled } else { Cost::FxSynthEgcOff });
            if self.vb(hw, v, V_FX_OFF) == 1 {
                self.t(Cost::FxSynthOff);
            } else {
                self.t(Cost::FxSynthCall);
                self.synth_fx(v, a0, hw);
            }
        }
        // portamento
        let pc = self.vw(hw, v, V_PORTA);
        if pc != 0 && d2 == d4 {
            self.t(Cost::FxPortaSame);
        }
        if pc != 0 && d2 != d4 {
            self.t(Cost::FxPorta);
            let pc = pc.wrapping_sub(1);
            self.vww(hw, v, V_PORTA, pc);
            let (prev, cur) = (d4, d2);
            let prod = (prev.wrapping_sub(cur) as i16 as i32).wrapping_mul(pc as i16 as i32);
            let speed = ext(self.vb(hw, v, V_PORTA_SPEED)) as i16;
            let mut q = prod as u32;
            if speed != 0 {
                self.t(Cost::FxPortaDiv);
                let quot = prod.wrapping_div(speed as i32);
                // divs.w: an overflowing quotient leaves the dividend
                if (-0x8000..=0x7fff).contains(&quot) {
                    q = quot as u32;
                }
            }
            d2 = (q as u16).wrapping_add(cur);
        }
        // vibrato
        let level = ib(hw, I_VIB_LEVEL);
        if level != 0 {
            self.t(Cost::FxVib);
            let delay = self.vb(hw, v, V_VIB_DELAY);
            if delay != 0 {
                self.t(Cost::FxVibDelay);
                self.vwb(hw, v, V_VIB_DELAY, delay - 1);
            } else {
                self.t(Cost::FxVibRun);
                let pos = self.vb(hw, v, V_VIB_POS);
                let x = self.rb(hw, VIBRATO + pos as u32);
                if (x as i8) < 0 {
                    self.t(Cost::FxVibNeg);
                    let d = ((x.wrapping_neg() as u32) << 2) / level as u32;
                    d2 = d2.wrapping_sub(d as u16);
                } else {
                    let d = ((x as u32) << 2) / level as u32;
                    d2 = d2.wrapping_add(d as u16);
                }
                let sp = ib(hw, I_VIB_SPEED);
                self.vwb(hw, v, V_VIB_POS, pos.wrapping_add(sp));
            }
        }
        self.t(Cost::FxPer);
        let slide = self.vw(hw, v, V_SLIDE);
        d2 = d2.wrapping_add(slide);
        self.poke_per(hw, v, d2);
        let ss = ext(self.vb(hw, v, V_SLIDE_SPEED));
        self.vww(hw, v, V_SLIDE, slide.wrapping_sub(ss));
        self.t(Cost::FxAfterPer);
        // the ADSR and the envelope generator, of the voice's instrument
        let ins = self.vb(hw, v, V_INSTRUMENT);
        let a0 = self.instrument(hw, ins);
        let ib = |hw: &Hw, o: u32| hw.bus.r8(a0.wrapping_add(o));
        if self.vb(hw, v, V_ADSR_ON) != 0 {
            self.t(Cost::FxAdsr);
            let len = hw.bus.r16(a0.wrapping_add(I_ADSR_LEN));
            let pos = self.vw(hw, v, V_ADSR_POS);
            if pos as i16 >= len as i16 {
                self.t(Cost::FxAdsrEnd);
                self.vwb(hw, v, V_INSTRUMENT, 0);
                self.poke_vol(hw, v, 0);
                self.t(Cost::FxAdsrEndEnd);
                return;
            }
            self.t(Cost::FxAdsrStep);
            let t = self.ptr(hw, P_ADSR).wrapping_add((ib(hw, I_ADSR_TABLE) as u32) << 8).wrapping_add(pos as u32);
            let x = hw.bus.r8(t) as u32 + 1;
            let nv = self.vb(hw, v, V_NEW_VOLUME);
            if nv != 0 {
                self.t(Cost::FxAdsrNewVol);
            }
            let vol = if nv != 0 { nv } else { ib(hw, I_VOLUME) } as u32;
            self.vww(hw, v, V_ADSR_POS, pos.wrapping_add(1));
            self.poke_vol(hw, v, (x * vol) >> 7);
        }
        if self.vb(hw, v, V_EGC_MODE) == 0 {
            self.t(Cost::FxEgc);
            return;
        }
        if self.vb(hw, v, V_EGC_OFF) == 1 {
            self.t(Cost::FxEgcDisabled);
            return;
        }
        self.t(Cost::FxEgcRun);
        let wave = self.ptr(hw, P_WAVEFORMS).wrapping_add((ib(hw, I_WAVEFORM) as u32) << 8);
        let flag = self.rb(hw, USE_BUF + v as u32) ^ 1;
        self.wb(hw, USE_BUF + v as u32, flag);
        if flag == 0 {
            self.t(Cost::FxEgcBuf2);
        }
        let buf = self.mem + if flag != 0 { BUF1 } else { BUF2 } + 256 * v as u32;
        self.poke_loc(hw, v, buf);
        let n = ((hw.bus.r16(a0.wrapping_add(I_LENGTH)).wrapping_sub(1) & 0xff) >> 4) as u32;
        for k in 0..(n + 1) * 4 {
            let x = hw.bus.r32(wave.wrapping_add(4 * k));
            hw.bus.w32(buf + 4 * k, x);
        }
        self.tn(Cost::FxEgcCopy, n + 1);
        let t = self.ptr(hw, P_EGC).wrapping_add((ib(hw, I_EGC_TABLE) as u32) << 7);
        let pos = self.vw(hw, v, V_EGC_POS);
        let neg = hw.bus.r8(t.wrapping_add(pos as i16 as u32)).wrapping_add(ib(hw, I_EGC_OFFSET));
        if neg != 0 {
            self.t(Cost::FxEgcNegSetup);
            self.negate(hw, buf, neg as u32);
            self.tn(Cost::FxEgcNeg, neg as u32);
        }
        let pos = pos.wrapping_add(1);
        self.vww(hw, v, V_EGC_POS, pos);
        if pos as i16 > hw.bus.r16(a0.wrapping_add(I_EGC_LEN)) as i16 {
            if self.vb(hw, v, V_EGC_MODE) == 1 {
                self.t(Cost::FxEgcOnce);
                self.vwb(hw, v, V_EGC_MODE, 0);
            } else {
                self.t(Cost::FxEgcRepeat);
                self.vww(hw, v, V_EGC_POS, 0);
            }
        }
        self.t(Cost::FxEgcEnd);
    }

    /// The synth effects on the voice's first buffer (the original's routine at
    /// $1172), with `a0` the instrument (or what the original takes for it).
    fn synth_fx(&mut self, v: usize, a0: u32, hw: &mut Hw) {
        self.t(Cost::SfxEntry);
        let ib = |hw: &Hw, o: u32| hw.bus.r8(a0.wrapping_add(o));
        if ib(hw, I_EFFECT) == 0 {
            self.t(Cost::SfxNone);
            return self.t(Cost::SfxExit);
        }
        let buf = self.mem + BUF1 + 256 * v as u32;
        let fx = match self.vb(hw, v, V_FX) {
            0 => ib(hw, I_EFFECT),
            f => {
                self.t(Cost::SfxFxOverride);
                f
            }
        };
        self.tn(Cost::SfxCmp, (fx as u32).clamp(1, 15));
        if !(1..=15).contains(&fx) {
            self.t(Cost::SfxUnknown);
            return self.t(Cost::SfxExit);
        }
        // the arguments, each replaced by the row's if it set one
        let mut args = [ib(hw, I_ARG1), ib(hw, I_ARG2), ib(hw, I_ARG3)];
        for (k, a) in args.iter_mut().enumerate() {
            let o = self.vb(hw, v, V_ARG1 + k as u32);
            if o != 0 {
                self.t(Cost::SfxOverride);
                *a = o;
            }
        }
        let [a1, a2, a3] = args;
        let rd = |hw: &Hw, a: u32| hw.bus.r8(a);
        let wr = |hw: &mut Hw, a: u32, x: u8| hw.bus.w8(a, x);
        let pos_at = self.mem + SYN_POS + v as u32;
        let slow = self.vb(hw, v, V_SLOW);
        // the slow-motion counter: counts down, then the effect runs once and it
        // starts again from `level` (Rotate 1 and the effects that take no level
        // do not use it)
        let slowed = |me: &mut Self, hw: &mut Hw, level: u8| -> bool {
            if slow != 0 {
                me.t(Cost::SfxSlow);
                me.vwb(hw, v, V_SLOW, slow - 1);
                true
            } else {
                me.t(Cost::SfxSet);
                me.vwb(hw, v, V_SLOW, level);
                false
            }
        };
        let waves = self.ptr(hw, P_WAVEFORMS);
        match fx {
            1 => {
                self.t(Cost::SfxRotate1);
                let (s, e, add) = (a1 as u32, a2 as u32, a3);
                if e >= s {
                    self.t(Cost::SfxRotate1Run);
                    for k in s..=e {
                        let x = rd(hw, buf + k);
                        wr(hw, buf + k, x.wrapping_add(add));
                    }
                    self.tn(Cost::SfxRotate1Iter, e - s + 1);
                }
            }
            2 => {
                self.t(Cost::SfxRotate2);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxRotate2Slow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxRotate2Go);
                let (s, e) = (a1 as u32, a2 as u32);
                if e != 0 && e >= s {
                    self.t(Cost::SfxRotate2Run);
                    let mut p = buf + s;
                    let first = rd(hw, p);
                    let mut n = (e - s) as i16;
                    loop {
                        let x = rd(hw, p + 1);
                        wr(hw, p, x);
                        p += 1;
                        n -= 1;
                        self.t(Cost::SfxRotate2Iter);
                        if n <= 0 {
                            break;
                        }
                    }
                    wr(hw, p, first);
                }
            }
            3 => {
                self.t(Cost::SfxAlien);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxAlienSlow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxAlienGo);
                let w = waves.wrapping_add((a1 as u32) << 8);
                for k in 0..=a2 as u32 {
                    let x = rd(hw, buf + k).wrapping_add(rd(hw, w.wrapping_add(k)));
                    wr(hw, buf + k, x);
                }
                self.tn(Cost::SfxAlienIter, a2 as u32 + 1);
            }
            4 => {
                self.t(Cost::SfxNegator);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxNegatorSlow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxNegatorGo);
                if a2 >= a1 {
                    self.t(Cost::SfxNegatorRun);
                    let count = a2 - a1;
                    let p = rd(hw, pos_at);
                    let at = buf + a1 as u32 + p as u32;
                    let x = rd(hw, at);
                    wr(hw, at, x.wrapping_neg());
                    wr(hw, pos_at, p.wrapping_add(1));
                    if p == count {
                        self.t(Cost::SfxNegatorWrap);
                        wr(hw, pos_at, 0);
                    }
                }
            }
            5 => {
                self.t(Cost::SfxPolyNeg);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxPolyNegSlow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxPolyNegGo);
                if a2 >= a1 {
                    self.t(Cost::SfxPolyNegRun);
                    let count = a2 - a1;
                    let s = buf + a1 as u32;
                    let p = rd(hw, pos_at);
                    if p == 0 {
                        self.t(Cost::SfxPolyNegZero);
                    }
                    let back = if p == 0 { count } else { p - 1 };
                    for at in [s + back as u32, s + p as u32] {
                        let x = rd(hw, at);
                        wr(hw, at, x.wrapping_neg());
                    }
                    wr(hw, pos_at, p.wrapping_add(1));
                    if p == count {
                        self.t(Cost::SfxPolyNegWrap);
                        wr(hw, pos_at, 0);
                    }
                }
            }
            6 | 7 => {
                self.t(if fx == 6 { Cost::SfxShaker1 } else { Cost::SfxShaker2 });
                if slowed(self, hw, a3) {
                    self.t(if fx == 6 { Cost::SfxShaker1Slow } else { Cost::SfxShaker2Slow });
                    return self.t(Cost::SfxExit);
                }
                self.t(if fx == 6 { Cost::SfxShaker1Go } else { Cost::SfxShaker2Go });
                let w = waves.wrapping_add((a1 as u32) << 8);
                let p = rd(hw, pos_at);
                let mix = rd(hw, w.wrapping_add(p as u32));
                for k in 0..=a2 as u32 {
                    let x = rd(hw, buf + k);
                    wr(hw, buf + k, x.wrapping_add(mix));
                    // Shaker 2 negates the next byte when the loop's counter
                    // (from the level down) is the position
                    if fx == 7 && (a2 as u32 - k) as u8 == p {
                        self.t(Cost::SfxShaker2Neg);
                        let x = rd(hw, buf + k + 1);
                        wr(hw, buf + k + 1, x.wrapping_neg());
                    }
                }
                self.tn(if fx == 6 { Cost::SfxShaker1Iter } else { Cost::SfxShaker2Iter }, a2 as u32 + 1);
                wr(hw, pos_at, p.wrapping_add(1));
                if p == a2 {
                    self.t(Cost::SfxShakerWrap);
                    wr(hw, pos_at, 0);
                }
            }
            8 => {
                self.t(Cost::SfxAmfLfo);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxAmfLfoSlow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxAmfLfoGo);
                let w = waves.wrapping_add((a1 as u32) << 8);
                let p = rd(hw, pos_at);
                let x = rd(hw, w.wrapping_add(p as u32)).wrapping_neg();
                self.vww(hw, v, V_SLIDE, ext(x));
                wr(hw, pos_at, p.wrapping_add(1));
                if p == a2 {
                    self.t(Cost::SfxAmfLfoWrap);
                    wr(hw, pos_at, 0);
                }
            }
            9 => {
                self.t(Cost::SfxLaser);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxLaserSlow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxLaserGo);
                let p = rd(hw, pos_at);
                let s = self.vw(hw, v, V_SLIDE).wrapping_add(ext(a1.wrapping_neg()));
                self.vww(hw, v, V_SLIDE, s);
                if p == a2 {
                    self.t(Cost::SfxLaserWrap);
                    wr(hw, pos_at, 0);
                    self.vww(hw, v, V_SLIDE, 0);
                } else {
                    wr(hw, pos_at, p.wrapping_add(1));
                }
            }
            0xa => {
                self.t(Cost::SfxOctFx1);
                if slow != 0 {
                    // a level of 0 holds the counter where it is
                    if a1 != 0 {
                        self.t(Cost::SfxSlow);
                        self.vwb(hw, v, V_SLOW, slow - 1);
                    } else {
                        self.t(Cost::SfxOctFx1Hold);
                    }
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxSet);
                self.vwb(hw, v, V_SLOW, a3);
                let n = (a1 >> 1) as u32;
                let mut d = buf;
                for k in 0..=n {
                    let x = rd(hw, buf + 2 * k);
                    wr(hw, d, x);
                    d += 1;
                }
                for k in 0..=n {
                    let x = rd(hw, buf + k);
                    wr(hw, d, x);
                    d += 1;
                }
                self.tn(Cost::SfxOctFx1Iter, n + 1);
            }
            0xb => {
                self.t(Cost::SfxOctFx2);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxOctFx2Slow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxOctFx2Go);
                let mut d = buf + a1 as u32;
                for k in (0..=(a1 >> 1) as u32).rev() {
                    let x = rd(hw, buf + k);
                    d -= 1;
                    wr(hw, d, x);
                    d -= 1;
                    wr(hw, d, x);
                }
                self.tn(Cost::SfxOctFx2Iter, (a1 >> 1) as u32 + 1);
            }
            0xc => {
                self.t(Cost::SfxAlising);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxAlisingSlow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxAlisingGo);
                let mut d = buf;
                let mut n = [0u32; 3];
                for k in 0..=a1 as u32 {
                    let (x, next) = (rd(hw, d) as i8, rd(hw, buf + k + 1) as i8);
                    if next > x {
                        wr(hw, d, (x as u8).wrapping_add(a2));
                        d += 1;
                        n[1] += 1;
                    } else if next < x {
                        wr(hw, d, (x as u8).wrapping_sub(a2));
                        d += 1;
                        n[2] += 1;
                    } else {
                        n[0] += 1;
                    }
                }
                self.tn(Cost::SfxAlisingIter, n[0]);
                self.tn(Cost::SfxAlisingUp, n[1]);
                self.tn(Cost::SfxAlisingDown, n[2]);
            }
            0xd | 0xe => {
                self.t(if fx == 0xd { Cost::SfxEgFx1 } else { Cost::SfxEgFx2 });
                if slowed(self, hw, a3) {
                    self.t(if fx == 0xd { Cost::SfxEgFx1Slow } else { Cost::SfxEgFx2Slow });
                    return self.t(Cost::SfxExit);
                }
                self.t(if fx == 0xd { Cost::SfxEgFx1Go } else { Cost::SfxEgFx2Go });
                let t = self.ptr(hw, P_EGC).wrapping_add((a2 as u32) << 7);
                let p = rd(hw, pos_at);
                let mut d = buf;
                let mut n = [0u32; 3];
                for k in 0..=a1 as u32 {
                    let (x, next) = (rd(hw, d) as i8, rd(hw, buf + k + 1) as i8);
                    n[(next > x) as usize + 2 * (next < x) as usize] += 1;
                    // EgFx 1 steps by the table at its position, 2 by the table
                    // at the loop's counter (from the level down)
                    let i = if fx == 0xd { p as u32 } else { a1 as u32 - k };
                    if next > x {
                        wr(hw, d, (x as u8).wrapping_add(rd(hw, t.wrapping_add(i))));
                    } else if next < x {
                        wr(hw, d, (x as u8).wrapping_sub(rd(hw, t.wrapping_add(i))));
                    }
                    d += 1;
                }
                if fx == 0xd {
                    self.tn(Cost::SfxEgFx1Iter, n[0]);
                    self.tn(Cost::SfxEgFx1Up, n[1]);
                    self.tn(Cost::SfxEgFx1Down, n[2]);
                    wr(hw, pos_at, p.wrapping_add(1));
                    if p == a1 {
                        self.t(Cost::SfxEgFx1Wrap);
                        wr(hw, pos_at, 0);
                    }
                } else {
                    self.tn(Cost::SfxEgFx2Iter, n[0]);
                    self.tn(Cost::SfxEgFx2Up, n[1]);
                    self.tn(Cost::SfxEgFx2Down, n[2]);
                }
            }
            _ => {
                self.t(Cost::SfxChanger);
                if slowed(self, hw, a3) {
                    self.t(Cost::SfxChangerSlow);
                    return self.t(Cost::SfxExit);
                }
                self.t(Cost::SfxChangerGo);
                let w = waves.wrapping_add((a1 as u32) << 8);
                let mut n = [0u32; 3];
                for k in 0..=a2 as u32 {
                    let (x, to) = (rd(hw, buf + k), rd(hw, w.wrapping_add(k)));
                    if to > x {
                        wr(hw, buf + k, x.wrapping_add(1));
                        n[1] += 1;
                    } else if to < x {
                        wr(hw, buf + k, x.wrapping_sub(1));
                        n[2] += 1;
                    } else {
                        n[0] += 1;
                    }
                }
                self.tn(Cost::SfxChangerIter, n[0]);
                self.tn(Cost::SfxChangerUp, n[1]);
                self.tn(Cost::SfxChangerDown, n[2]);
            }
        }
        self.t(Cost::SfxExit);
    }
}

#[cfg(test)]
mod tests {
    use super::{BUF1, PERIODS};
    use crate::engine::Engine;
    use std::path::Path;

    /// "TEST", two subsongs (speed 3, 16 rows, position 0 only): voice 0 plays
    /// note 24 with instrument 1, a synth instrument on a 32-byte square
    /// waveform at volume 64; the other voices play empty rows.
    fn module() -> Vec<u8> {
        let mut d = b"Synth4.0".to_vec();
        d.extend(1u16.to_be_bytes()); // positions
        d.extend(0u16.to_be_bytes()); // track rows past the first 64
        d.extend([0; 4]);
        d.extend([0, 1, 1, 2, 0, 0]); // samples, waveforms, instruments, subsongs, EGC and ADSR tables
        d.resize(0x24, 0);
        d.extend(b"TEST");
        d.resize(0xcc, 0);
        let mut ins = [0u8; 0x1c];
        ins[1] = 1; // synth, waveform 0
        ins[2..4].copy_from_slice(&32u16.to_be_bytes());
        ins[6] = 64;
        d.extend(ins);
        d.extend([0u8; 256]); // arpeggios
        for _ in 0..3 {
            // the subsongs (and the one past them): speed 3, 16 rows, positions 0 to 0
            d.extend([0, 0, 0, 0, 3, 16, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
        d.extend((0..256).map(|i| if i < 16 { 0x7f } else if i < 32 { 0x80 } else { 0u8 }));
        // position 0: voice 0 from row 0, the others from row 32
        d.extend([0, 0, 0, 0, 0, 32, 0, 0, 0, 32, 0, 0, 0, 32, 0, 0]);
        let mut t = [0u8; 256];
        t[..4].copy_from_slice(&[24, 1, 0, 0]);
        d.extend(t);
        d
    }

    #[test]
    fn plays_a_synth_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("syn.test"), module()).unwrap();
        let mut out = vec![0f32; 2 * 24000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Synthesis");
        assert_eq!((p.sub_min, p.sub_max), (0, 1));
        assert_eq!(p.module_name, "TEST");
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        let loc = |ch: u32| {
            let (hi, lo) = (reg(0xDFF0A0 + 16 * ch), reg(0xDFF0A2 + 16 * ch));
            hi.iter().zip(lo).map(|(&h, l)| (h as u32) << 16 | l as u32).collect::<Vec<_>>()
        };
        // InitSound points every voice at one place; the note at voice 0's
        // first synth buffer, 0x148 into the data hunk the init location is in
        let init = loc(0)[0];
        assert!((1..4).all(|ch| loc(ch) == [init]));
        assert_eq!(loc(0).len(), 2);
        let buf = loc(0)[1];
        assert_eq!(buf.wrapping_sub(init), BUF1.wrapping_sub(super::INIT_LOC));
        // the waveform copied there: 32 bytes, 16 words
        assert_eq!(e.m.bus.r8(buf), 0x7f);
        assert_eq!(e.m.bus.r8(buf + 16), 0x80);
        assert_eq!(reg(0xDFF0A4), [16]);
        // the note's period at its row and every tick; the volume
        let per = reg(0xDFF0A6);
        assert!(per.len() > 10 && per.iter().all(|&p| p == PERIODS[24]));
        assert!(reg(0xDFF0A8).contains(&64));
        // the other voices: no instrument, volume 0 every tick
        assert!(reg(0xDFF0B8).len() > 10 && reg(0xDFF0B8).iter().all(|&v| v == 0));
        // DMA off and on for the new instrument, after its location and length
        assert_eq!(reg(0xDFF096), [0x0001, 0x8001]);
        let at = |a: u32, v: u16| log.iter().position(|w| w.0 == a && w.1 == v).unwrap();
        assert!(at(0xDFF0A4, 16) < at(0xDFF096, 0x0001));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
