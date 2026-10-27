//! Digital Mugician, ported from NostalgicPlayer's Digital Mugician player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the Digital Mugician Eagleplayer ("Digital Mugician player
//! module V1.3", adapted by DEFECT), which takes both kinds of module, the
//! 4-voice " MUGICIAN/SOFTEYES 1990 " and the 7-voice " MUGICIAN2/SOFTEYES
//! 1990", and plays both with one 4-voice replayer of its own, through
//! ENPP_Poke* and ENPP_DMAMask. It does no software mixing: a 7-voice module
//! plays the four tracks of the subsong's own slot on the four voices, as a
//! 4-voice module does (NostalgicPlayer plays three of them and mixes the four
//! of the next slot onto the fourth voice). Like the original, the port plays
//! from the module in guest memory, where the instrument effects rewrite the
//! waveforms and an arpeggio effect rewrites its instrument.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * a note writes its waveform's location and length before it turns the
//!   voice's DMA off, and every tick ends with DMA on for all four voices;
//!   a sample's loop (or 4 words of zeros) goes out at the start of the next
//!   tick;
//! * every voice writes its period every tick, whatever its instrument, and its
//!   volume when the volume envelope steps; the first 13 volumes after the
//!   player is loaded are written as 0 (its volume routine counts them);
//! * the no-wander effect writes the waveform's location and length too (with
//!   no DMA off), the no-DMA effect turns a sample's DMA off all the same, and a
//!   note copies the instrument's first source waveform for its effect in both
//!   versions; the filter effects do nothing;
//! * the finetune is masked to 4 bits, the volume loop is bit 1, the note index
//!   wraps at a byte with the arpeggio and at a word with the transpose (as an
//!   unsigned offset into the period table), and nothing is range-checked; the
//!   instrument of a note is taken before it is masked to 6 bits;
//! * the waveform effects shift where NostalgicPlayer divides, and the morphing
//!   effect blends waveform 0 with the original's own memory (its pointer table
//!   and periods), as the original's code does; its period table has 3830 and
//!   853 where NostalgicPlayer's has 3820 and 854;
//! * the subsongs are the leading slots that have a name (at most 7); the end of
//!   a song that does not loop turns all DMA off and clears the voices;
//! * the writes come when the original's do (`Cost`).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN1: Open = Open { name: "Digital Mugician 1", detect: detect1, new: new1 };
pub const OPEN2: Open = Open { name: "Digital Mugician 2", detect: detect2, new: new2 };

include!("digitalmugician_tables.rs");

const MARK1: &[u8; 24] = b" MUGICIAN/SOFTEYES 1990 ";
const MARK2: &[u8; 24] = b" MUGICIAN2/SOFTEYES 1990";

// The original's variables from $1128 of its hunk on, mirrored in guest memory:
// the morphing effect reads the pointer table and the periods as a waveform.
/// 8 bytes of zeros, the loop of a sample that does not loop.
const EMPTY: u32 = 0x000;
const POINTERS: u32 = 0x0dc;
const PERIOD_TABLE: u32 = 0x0fc;
const MIRROR_SIZE: u32 = 0x900;
// The pointer table, as InitSound computes it.
const P_SUBSONG: u32 = 0x00;
const P_SEQUENCES: u32 = 0x04;
const P_INSTRUMENTS: u32 = 0x08;
const P_ARPEGGIOS: u32 = 0x0c;
const P_WAVEFORMS: u32 = 0x10;
const P_TRACKS: u32 = 0x14;
const P_SAMPLES: u32 = 0x18;
const P_SAMPLE_DATA: u32 = 0x1c;

// An instrument (16 bytes).
const I_WAVEFORM: u32 = 0x0;
const I_LENGTH: u32 = 0x1;
const I_VOLUME: u32 = 0x2;
const I_VOLUME_SPEED: u32 = 0x3;
const I_ARPEGGIO: u32 = 0x4;
const I_PITCH: u32 = 0x5;
const I_EFFECT_INDEX: u32 = 0x6;
const I_DELAY: u32 = 0x7;
const I_FINETUNE: u32 = 0x8;
const I_PITCH_LOOP: u32 = 0x9;
const I_EFFECT: u32 = 0xb;
const I_SOURCE1: u32 = 0xc;
const I_SOURCE2: u32 = 0xd;
const I_EFFECT_SPEED: u32 = 0xe;
const I_VOLUME_LOOP: u32 = 0xf;

// The voice's effects (row byte 2 - $3E, or 1 below $40).
const FX_PITCH_BEND: u8 = 1;
const FX_NO_EFFECT: u8 = 2;
const FX_NO_VOLUME: u8 = 3;
const FX_NO_EFFECT_VOLUME: u8 = 4;
const FX_PATTERN_LENGTH: u8 = 5;
const FX_SPEED: u8 = 6;
const FX_NO_DMA: u8 = 0xa;
const FX_ARPEGGIO: u8 = 0xb;
const FX_NO_WANDER: u8 = 0xc;
const FX_SHUFFLE: u8 = 0xd;

fn detect1(d: &[u8]) -> bool {
    d.get(..24) == Some(&MARK1[..])
}

fn detect2(d: &[u8]) -> bool {
    d.get(..24) == Some(&MARK2[..])
}

fn new1(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    new(d, base, m, false)
}

fn new2(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    new(d, base, m, true)
}

/// A voice's variables (the original's 48-byte block; the word and byte
/// aliasing it relies on is noted where it matters).
#[derive(Clone, Copy, Default)]
struct Voice {
    track: u16,
    /// The instrument (byte 5 of the original's block), masked to 6 bits after
    /// the row that sets it.
    instrument: u16,
    note: u16,
    /// The sequence's transpose, a signed byte.
    transpose: u8,
    /// The row's effect byte (its pitch bend end note, or no-wander's note).
    target: u8,
    /// The row's effect argument (signed for the pitch bend).
    param: u8,
    effect: u8,
    period: u16,
    finetune: u8,
    /// A sample started: its loop goes out at the start of the next tick.
    sample_pending: bool,
    vol_index: u16,
    vol_counter: u16,
    arp_index: u16,
    delay: u8,
    pitch_index: u16,
    /// The sample's 32-byte entry.
    sample: u32,
    fx_counter: u8,
    bend_end: u16,
    bend: u16,
}

pub struct DigitalMugician {
    v2: bool,
    base: u32,
    /// The original's variables from $1128 on, in guest memory.
    mem: u32,
    voices: [Voice; 4],
    speed: u16,
    new_pattern: bool,
    new_row: bool,
    position: u16,
    row: u16,
    current_speed: u16,
    song_length: u16,
    pattern_length: u16,
    /// What the original's volume routine counted (it writes 0 for the first 13).
    vol_calls: u16,
    /// InitSound ran: the song's end clears the voices once.
    started: bool,
    /// The instruments whose effect ran this tick (+1), at most 4.
    ch_tab: [u8; 4],
    ch_len: usize,
    sub: u16,
    last_sub: u16,
    songend: bool,
    /// The first slot's 12-byte name, which InitPlayer copies.
    title: String,
    /// Colour clocks the original spends since its last write (`Cost`).
    spent: f64,
    fit: Option<std::io::BufWriter<std::fs::File>>,
    counts: Vec<(u8, u32)>,
}

fn new(d: &[u8], base: u32, m: &mut Machine, v2: bool) -> Option<Box<dyn Replayer>> {
    let top = std::mem::replace(&mut m.alloc_top, true);
    let mem = m.alloc(MIRROR_SIZE);
    m.alloc_top = top;
    if mem == 0 {
        return None;
    }
    for (k, &p) in PERIODS.iter().enumerate() {
        m.bus.w16(mem + PERIOD_TABLE + 2 * k as u32, p);
    }
    m.bus.w32(mem + PERIOD_TABLE + 2 * PERIODS.len() as u32, 8);
    // SubSongRange: the slots up to the first without a name, at most 7
    let mut n = 0u16;
    while n < 7 && m.bus.r16(base + 0x50 + 16 * n as u32) != 0x2020 {
        n += 1;
    }
    let fit = std::env::var_os("DMU_FIT")
        .and_then(|p| std::fs::File::create(p).ok())
        .map(std::io::BufWriter::new);
    let mut r = DigitalMugician {
        v2,
        base,
        mem,
        voices: [Voice::default(); 4],
        speed: 0,
        new_pattern: false,
        new_row: false,
        position: 0,
        row: 0,
        current_speed: 0,
        song_length: 0,
        pattern_length: 0,
        vol_calls: 0,
        started: false,
        ch_tab: [0x80; 4],
        ch_len: 0,
        sub: 0,
        last_sub: n.saturating_sub(1),
        songend: false,
        title: d.get(0x50..0x5c).map_or(String::new(), |t| {
            t.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
        }),
        spent: 0.0,
        fit,
        counts: Vec::new(),
    };
    // InitPlayer works the pointers out once (and clears the arpeggios of a
    // module without them), then DeliTracker runs InitSound
    r.pointers(0, &mut m.bus);
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(INIT_START[n as usize] + if v2 { CHECK2 } else { 0 });
    r.init_sound(0, &mut hw);
    Some(Box::new(r))
}

/// Colour clocks from the start of the open replayer to InitSound's first write,
/// by the number of named slots that SubSongRange counts (measured on the
/// Eagleplayer's write logs).
const INIT_START: [u64; 8] = [10337, 10356, 10380, 10403, 10427, 10450, 10474, 10487];
/// The check's time for the second mark, after it failed on the first.
const CHECK2: u64 = 188;
/// Colour clocks from the end of InitSound to the next one's first write when
/// DeliTracker steps the subsong (EndSound, then InitSound again).
const SUBSONG_START: u64 = 4569;

impl Replayer for DigitalMugician {
    fn format(&self) -> &'static str {
        if self.v2 {
            "Digital Mugician 2"
        } else {
            "Digital Mugician 1"
        }
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
    }

    fn tick(&mut self, hw: &mut Hw) {
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let _ = writeln!(f, "tick {}", hw.bus.clock);
        }
        self.spent = 0.0;
        self.counts.clear();
        self.ch_tab = [0x80; 4];
        self.ch_len = 0;
        self.t(Cost::TickStart);
        self.sample_loops(hw);
        self.t(Cost::AfterLoops);
        for i in 0..4 {
            self.t(Cost::NoteCall);
            self.play_note(i, hw);
        }
        self.t(Cost::AfterNotes);
        for i in 0..4 {
            self.t(Cost::EffectsCall);
            self.effects(i, hw);
        }
        self.t(Cost::EndTick);
        self.new_pattern = false;
        self.new_row = false;
        self.speed = self.speed.wrapping_sub(1);
        if self.speed == 0 {
            self.t(Cost::EndSpeed);
            let s = self.current_speed;
            self.speed = s & 0xf;
            self.current_speed = (s & 0xf) << 4 | (s & 0xf0) >> 4;
            self.new_row = true;
            self.row = self.row.wrapping_add(1);
            if self.row != 0x40 {
                self.t(Cost::EndRowCmp);
            }
            if self.row == 0x40 || self.row == self.pattern_length {
                self.t(Cost::EndPattern);
                self.row = 0;
                self.new_pattern = true;
                self.position = self.position.wrapping_add(1);
                if self.position == self.song_length {
                    self.t(Cost::EndSong);
                    self.songend = true;
                    let h = self.ptr(hw, P_SUBSONG);
                    if hw.bus.r8(h) != 0 {
                        self.position = hw.bus.r8(h + 1) as u16;
                    } else {
                        // the song stops: DMA off, the voices cleared, no DMA on
                        self.write(hw, W::DmaOff);
                        hw.dmacon(0x000f);
                        if self.started {
                            self.started = false;
                            self.voices = [Voice::default(); 4];
                        }
                        return;
                    }
                }
            }
        }
        self.t(Cost::DmaOnCall);
        self.write(hw, W::DmaOn);
        hw.dmacon(0x800f);
        // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does
        let now = hw.bus.clock;
        hw.wait((now / LINE + 10) * LINE - now);
    }
}

/// The original's register writes, for the costs of its calls to the host.
#[derive(Clone, Copy, PartialEq)]
enum W {
    Loc,
    Len,
    Per,
    Vol,
    DmaOff,
    DmaOn,
}

macro_rules! costs {
    ($($name:ident = $c:expr,)*) => {
        /// The original's code paths, each with the colour clocks it takes: fitted
        /// (least squares) to the gaps between the writes of the Eagleplayer's
        /// write logs. A write waits what the paths since the last one took.
        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        enum Cost { $($name,)* }
        const COSTS: &[f64] = &[$($c as f64,)*];
        #[allow(dead_code)]
        const COST_NAMES: &[&str] = &[$(stringify!($name),)*];
    };
}

costs! {
    TickStart = 153.48,
    LoopVoice = 23.48,
    LoopPending = 70.53,
    LoopLenLoc = -14.14,
    LoopLocAfter = 6.15,
    LoopEmpty = 46.01,
    LoopLocLen = -0.24,
    LoopLenAfter = -2.19,
    AfterLoops = 40.90,
    NoteCall = 16.43,
    NoteEntry = 16.43,
    NotePattern = 44.09,
    NoteRowTest = 16.43,
    NoteRow = 48.99,
    NoteRowEmpty = 5.52,
    NoteNote = 43.47,
    NoteStore = 3.27,
    NoteStoreInstrument = 17.74,
    NoteInstrument = 43.47,
    NoteFx40 = 8.62,
    NoteFx1 = 34.85,
    NoteParam = 43.47,
    NoteNotNoWander = 3.27,
    NoteLookupNoWander = 40.19,
    NoteLookupBend = 34.85,
    NoteArpCheck = 43.47,
    NoteArpSet = 20.12,
    NoteWave = 43.47,
    NoteWaveCheck = 3.27,
    NoteWaveLoc = -3.77,
    NoteLocLen = -0.24,
    NoteLenCheck = 13.37,
    NoteDmaOff = 21.66,
    NoteDmaOffAfter = 10.44,
    NoteCopyTest = 13.49,
    NoteCopyCmp = 12.43,
    NoteCopy = 575.88,
    NoteTailCmp = 12.88,
    NoteVolReset = 15.38,
    NoteTail = 31.60,
    NoteSample = 47.23,
    NoteSampleLocLen = 17.76,
    NoteSampleLenDma = 23.41,
    NoteSampleAfter = 18.11,
    FxCmp = 13.99,
    FxNone = -1.74,
    FxLength = 16.60,
    FxLengthSet = 16.60,
    FxSpeed = 28.34,
    FxSpeedSet = 28.34,
    FxShuffle = 18.95,
    FxShuffleTest = 18.95,
    FxShuffleSet = 18.95,
    NoteReturn = 4.56,
    AfterNotes = 29.03,
    EffectsCall = 62.56,
    EffectsEntry = 62.56,
    EffectsWaveCheck = 10.88,
    ChTab = 21.30,
    ChTabCmp = 8.81,
    ChTabAdd = 55.44,
    FxDec = -19.89,
    FxRun = 75.33,
    FxFilter = -23.31,
    FxFilterIter = 28.58,
    FxMixing = 35.54,
    FxMixingIter = 36.00,
    FxScrollLeft = -66.07,
    FxScrollIter = 12.98,
    FxScrollRight = 66.17,
    FxUpsample = 30,
    FxUpsampleIter = 13.18,
    FxDownsample = 0.26,
    FxDownsampleIter = 16.96,
    FxNegate = -33.83,
    FxMadMix = 8.31,
    FxMadMixIter = 13.00,
    FxAddition = 0.18,
    FxAdditionIter = 22.78,
    FxFilter2 = 0.30,
    FxFilter2Iter = 37.44,
    FxMorph = 54.33,
    FxMorphHigh = 6.07,
    FxMorphBit = -0.19,
    FxIndexWrap = 7.69,
    FxMorphIter = 87.16,
    FxMorphF = 16.35,
    FxFilter3 = 0.24,
    FxFilter3Iter = 30.44,
    FxPolygate = -6.67,
    FxColgate = 23.54,
    FxColgateUp = 0.10,
    FxReturn = 75.33,
    EffectsRestore = 62.56,
    VolDec = 18.52,
    VolStep = 68.65,
    VolEnd = 14.04,
    VolLoopTest = 10,
    VolCalc = 54.61,
    VolForced = 14.01,
    VolAfter = 26.54,
    PerStart = 34.49,
    PerArp = 59.04,
    PerCalc = 34.49,
    PerBendTest = 12.33,
    PerBend = 46.16,
    PerBendCheck = 20.14,
    PerBendUp = -0.31,
    PerBendClamp = 25.76,
    PitchTest = 34.49,
    PitchDelay = 23.64,
    Pitch = 101.97,
    PitchLoop = 9.96,
    PerWrite = 34.49,
    PerAfter = 35.24,
    EndTick = 1.72,
    EndSpeed = 92.00,
    EndRowCmp = 11.00,
    EndPattern = 41.00,
    EndSong = 67.00,
    DmaOnCall = 13.00,
    // the host calls, before and after the write
    LocPre = 75.34,
    LocPost = 23.43,
    LenPre = 87.81,
    LenPost = 31.80,
    PerPre = 34.49,
    PerPost = 35.24,
    VolPre = 54.61,
    VolPost = 26.54,
    DmaPre = 46.79,
    DmaPost = 28.55,
}

/// A signed byte, sign-extended to a word.
fn ext(b: u8) -> u16 {
    b as i8 as u16
}

impl DigitalMugician {
    fn t(&mut self, c: Cost) {
        self.tn(c, 1);
    }

    fn tn(&mut self, c: Cost, n: u32) {
        let k = c as usize;
        self.spent += COSTS[k] * n as f64;
        if self.fit.is_some() && n != 0 {
            match self.counts.iter_mut().find(|e| e.0 as usize == k) {
                Some(e) => e.1 += n,
                None => self.counts.push((k as u8, n)),
            }
        }
    }

    /// Lets the time pass that the original's code takes up to a write of kind
    /// `w`, and counts the host call's own time after it for the next write.
    fn write(&mut self, hw: &mut Hw, w: W) {
        let (pre, post) = match w {
            W::Loc => (Cost::LocPre, Cost::LocPost),
            W::Len => (Cost::LenPre, Cost::LenPost),
            W::Per => (Cost::PerPre, Cost::PerPost),
            W::Vol => (Cost::VolPre, Cost::VolPost),
            W::DmaOff | W::DmaOn => (Cost::DmaPre, Cost::DmaPost),
        };
        self.t(pre);
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let s: Vec<String> = self.counts.iter().map(|(k, n)| format!("{}:{n}", COST_NAMES[*k as usize])).collect();
            let _ = writeln!(f, "w {} {}", hw.bus.clock + self.spent.max(0.0).round() as u64, s.join(" "));
        }
        // the fraction of a colour clock left over is carried to the next write
        let c = self.spent.max(0.0).round();
        hw.wait(c as u64);
        self.spent -= c;
        self.counts.clear();
        self.t(post);
    }

    fn ptr(&self, hw: &Hw, p: u32) -> u32 {
        hw.bus.r32(self.mem + POINTERS + p)
    }

    /// The original's pointers to the module's parts for subsong `n` (its
    /// routine at $574); a module without arpeggios gets 256 bytes of zeros
    /// where they would be, past its samples.
    fn pointers(&self, n: u16, b: &mut crate::bus::Bus) {
        let (base, p) = (self.base, self.mem + POINTERS);
        let mut a = base + 0x4c;
        b.w32(p + P_SUBSONG, a.wrapping_add((n as u32) << 4));
        a = a.wrapping_add(0x80);
        for k in 0..8u32 {
            let len = b.r32(base + 0x1c + 4 * k) << 3;
            if k == n as u32 {
                b.w32(p + P_SEQUENCES, a);
            }
            a = a.wrapping_add(len);
        }
        b.w32(p + P_INSTRUMENTS, a);
        a = a.wrapping_add(b.r32(base + 0x3c) << 4);
        b.w32(p + P_WAVEFORMS, a);
        a = a.wrapping_add(b.r32(base + 0x40) << 7);
        b.w32(p + P_SAMPLES, a);
        a = a.wrapping_add(b.r32(base + 0x44) << 5);
        b.w32(p + P_TRACKS, a);
        a = a.wrapping_add((b.r16(base + 0x1a) as u32) << 8);
        b.w32(p + P_SAMPLE_DATA, a);
        a = a.wrapping_add(b.r32(base + 0x48));
        b.w32(p + P_ARPEGGIOS, a);
        if b.r16(base + 0x18) == 0 {
            for k in 0..256 {
                b.w8(a.wrapping_add(k), 0);
            }
        }
    }

    /// InitSound: subsong `n`'s pointers, the voices cleared, all DMA off and
    /// each voice pointed at address 0 (volume 0, length and period 2), then
    /// DMA off once more.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.pointers(n, hw.bus);
        self.voices = [Voice::default(); 4];
        self.new_pattern = false;
        self.new_row = false;
        self.row = 0;
        // InitSound calls the host directly here; the times are the original's
        hw.dmacon(0x000f);
        hw.wait(55);
        for ch in (0..4).rev() {
            hw.vol(ch, 0);
            hw.wait(17);
            hw.loc(ch, 0);
            hw.wait(19);
            hw.len(ch, 2);
            hw.wait(23);
            hw.per(ch, 2);
            hw.wait(if ch > 0 { 24 } else { 648 });
        }
        let h = self.ptr(hw, P_SUBSONG);
        self.song_length = hw.bus.r8(h + 3) as u16;
        self.pattern_length = 0x40;
        let s = hw.bus.r8(h + 2) as u16;
        self.speed = s;
        self.current_speed = ((s & 0xf) << 4 | (s & 0xf)) & 0xff;
        self.position = 0;
        self.new_pattern = true;
        self.new_row = true;
        hw.dmacon(0x000f);
        self.started = true;
    }

    /// The start of a tick: the loop of each sample started on the last one.
    fn sample_loops(&mut self, hw: &mut Hw) {
        let data = self.ptr(hw, P_SAMPLE_DATA);
        for ch in 0..4 {
            if !self.voices[ch].sample_pending {
                self.t(Cost::LoopVoice);
                continue;
            }
            self.voices[ch].sample_pending = false;
            let s = self.voices[ch].sample;
            let loop_start = hw.bus.r32(s + 8);
            if loop_start != 0 {
                self.t(Cost::LoopPending);
                let len = (hw.bus.r32(s + 4).wrapping_sub(loop_start) as i32 >> 1) as u16;
                self.write(hw, W::Len);
                hw.len(ch, len);
                self.t(Cost::LoopLenLoc);
                self.write(hw, W::Loc);
                hw.loc(ch, data.wrapping_add(loop_start));
                self.t(Cost::LoopLocAfter);
            } else {
                self.t(Cost::LoopEmpty);
                self.write(hw, W::Loc);
                hw.loc(ch, self.mem + EMPTY);
                self.t(Cost::LoopLocLen);
                self.write(hw, W::Len);
                hw.len(ch, 4);
                self.t(Cost::LoopLenAfter);
            }
        }
    }

    /// The period of note `n` (a word) at finetune `ft`, from the original's
    /// table: the note is an unsigned offset into it.
    fn period(&self, hw: &Hw, n: u16, ft: u8) -> u16 {
        let off = ((ft as u32 & 0xf) << 7) + (n.wrapping_mul(2) as u32);
        hw.bus.r16(self.mem + PERIOD_TABLE + 2 * PERIOD_START + off)
    }

    fn instrument(&self, hw: &Hw, n: u16) -> u32 {
        self.ptr(hw, P_INSTRUMENTS).wrapping_add(((n << 4) as u32) & 0xffff)
    }

    fn waveform(&self, hw: &Hw, n: u8) -> u32 {
        self.ptr(hw, P_WAVEFORMS).wrapping_add((n as u32) << 7)
    }

    /// The original's routine at $866 for voice `ch`: a new track, a new row's
    /// note, and the effects that act on the song.
    fn play_note(&mut self, ch: usize, hw: &mut Hw) {
        self.t(Cost::NoteEntry);
        if self.new_pattern {
            self.t(Cost::NotePattern);
            let s = self.ptr(hw, P_SEQUENCES).wrapping_add((self.position.wrapping_shl(3) as u32 + 2 * ch as u32) & 0xffff);
            self.voices[ch].track = hw.bus.r8(s) as u16;
            self.voices[ch].transpose = hw.bus.r8(s + 1);
        }
        self.t(Cost::NoteRowTest);
        if self.new_row {
            self.t(Cost::NoteRow);
            let v = self.voices[ch];
            let row = self.ptr(hw, P_TRACKS).wrapping_add((v.track as u32) << 8).wrapping_add((self.row as u32 * 4) & 0xffff);
            if hw.bus.r8(row) == 0 {
                self.t(Cost::NoteRowEmpty);
            } else {
                self.note(ch, row, hw);
            }
        }
        self.song_effects(ch);
        self.t(Cost::NoteReturn);
    }

    fn note(&mut self, ch: usize, row: u32, hw: &mut Hw) {
        let rd = |a: u32| hw.bus.r8(a);
        let (r0, r1, r2, r3) = (rd(row), rd(row + 1), rd(row + 2), rd(row + 3));
        self.t(Cost::NoteNote);
        let mut v = self.voices[ch];
        if r2 != 0x4a {
            self.t(Cost::NoteStore);
            v.note = r0 as u16;
            if r1 != 0 {
                self.t(Cost::NoteStoreInstrument);
                v.instrument = r1.wrapping_sub(1) as u16;
            }
        }
        self.t(Cost::NoteInstrument);
        // the instrument before its mask
        let ins = self.instrument(hw, v.instrument);
        v.finetune = hw.bus.r8(ins + I_FINETUNE);
        v.instrument &= 0x3f;
        if r2 >= 0x40 {
            self.t(Cost::NoteFx40);
            v.effect = r2.wrapping_sub(0x3e);
        } else {
            self.t(Cost::NoteFx1);
            v.effect = FX_PITCH_BEND;
        }
        v.param = r3;
        self.t(Cost::NoteParam);
        if v.effect == FX_NO_WANDER {
            self.t(Cost::NoteLookupNoWander);
            v.target = r0;
            v.bend_end = self.period(hw, (v.target as u16).wrapping_add(ext(v.transpose)), v.finetune);
        } else {
            self.t(Cost::NoteNotNoWander);
            v.target = r2;
            if v.effect == FX_PITCH_BEND {
                self.t(Cost::NoteLookupBend);
                v.bend_end = self.period(hw, (v.target as u16).wrapping_add(ext(v.transpose)), v.finetune);
            }
        }
        self.t(Cost::NoteArpCheck);
        if v.effect == FX_ARPEGGIO {
            self.t(Cost::NoteArpSet);
            hw.bus.w8(ins + I_ARPEGGIO, v.param & 7);
        }
        self.t(Cost::NoteWave);
        let wave = hw.bus.r8(ins + I_WAVEFORM);
        let fx = v.effect;
        self.voices[ch] = v;
        if fx != FX_NO_WANDER {
            self.t(Cost::NoteWaveCheck);
        }
        if fx != FX_NO_WANDER && wave >= 0x20 {
            self.sample_note(ch, wave, hw);
        } else {
            self.t(Cost::NoteWaveLoc);
            let a = self.waveform(hw, wave);
            self.write(hw, W::Loc);
            hw.loc(ch, a);
            self.t(Cost::NoteLocLen);
            self.write(hw, W::Len);
            hw.len(ch, hw.bus.r8(ins + I_LENGTH) as u16);
            self.t(Cost::NoteLenCheck);
            if fx != FX_NO_WANDER {
                self.t(Cost::NoteLenCheck);
            }
            if fx != FX_NO_WANDER && fx != FX_NO_DMA {
                self.t(Cost::NoteDmaOff);
                self.write(hw, W::DmaOff);
                hw.dmacon(1 << ch);
                self.t(Cost::NoteDmaOffAfter);
            }
            // the instrument's effect starts from its first source waveform
            self.t(Cost::NoteCopyTest);
            let effect = hw.bus.r8(ins + I_EFFECT);
            if effect != 0 {
                let n = match fx {
                    FX_NO_EFFECT => 1,
                    FX_NO_EFFECT_VOLUME => 2,
                    FX_NO_WANDER => 3,
                    _ => 3,
                };
                self.tn(Cost::NoteCopyCmp, n);
            }
            if effect != 0 && fx != FX_NO_EFFECT && fx != FX_NO_EFFECT_VOLUME && fx != FX_NO_WANDER {
                self.t(Cost::NoteCopy);
                let src = self.waveform(hw, hw.bus.r8(ins + I_SOURCE1));
                let dst = self.waveform(hw, wave);
                hw.bus.w8(ins + I_EFFECT_INDEX, 0);
                for k in 0..32 {
                    let l = hw.bus.r32(src.wrapping_add(4 * k));
                    hw.bus.w32(dst.wrapping_add(4 * k), l);
                }
                self.voices[ch].fx_counter = hw.bus.r8(ins + I_EFFECT_SPEED);
            }
        }
        self.note_tail(ch, ins, hw);
    }

    /// A sample note (the original's routine at $1062): the sample's location
    /// and length, the voice's DMA off; its loop follows on the next tick.
    fn sample_note(&mut self, ch: usize, wave: u8, hw: &mut Hw) {
        self.t(Cost::NoteSample);
        let s = self.ptr(hw, P_SAMPLES).wrapping_add(((wave as u32).wrapping_sub(0x20) << 5) & 0xffff);
        self.voices[ch].sample = s;
        self.voices[ch].sample_pending = true;
        let start = hw.bus.r32(s);
        let a = self.ptr(hw, P_SAMPLE_DATA).wrapping_add(start);
        self.write(hw, W::Loc);
        hw.loc(ch, a);
        self.t(Cost::NoteSampleLocLen);
        let len = (hw.bus.r32(s + 4).wrapping_sub(start) as i32 >> 1) as u16;
        self.write(hw, W::Len);
        hw.len(ch, len);
        self.t(Cost::NoteSampleLenDma);
        self.write(hw, W::DmaOff);
        hw.dmacon(1 << ch);
        self.t(Cost::NoteSampleAfter);
    }

    fn note_tail(&mut self, ch: usize, ins: u32, hw: &mut Hw) {
        let v = &mut self.voices[ch];
        let n = match v.effect {
            FX_NO_VOLUME => 1,
            FX_NO_EFFECT_VOLUME => 2,
            _ => 3,
        };
        let reset = !matches!(v.effect, FX_NO_VOLUME | FX_NO_EFFECT_VOLUME | FX_NO_WANDER);
        if reset {
            v.vol_counter = 1;
            v.vol_index = 0;
        }
        v.bend = 0;
        v.delay = hw.bus.r8(ins + I_DELAY);
        v.pitch_index = 0;
        v.arp_index = 0;
        self.tn(Cost::NoteTailCmp, n);
        if reset {
            self.t(Cost::NoteVolReset);
        }
        self.t(Cost::NoteTail);
    }

    /// The effects that act on the song, every tick until the voice's next row.
    fn song_effects(&mut self, ch: usize) {
        let (fx, p) = (self.voices[ch].effect, self.voices[ch].param);
        let cmps = match fx {
            FX_PATTERN_LENGTH => 1,
            FX_SPEED => 2,
            7 => 3,
            8 => 4,
            _ => 5,
        };
        self.tn(Cost::FxCmp, cmps);
        match fx {
            FX_PATTERN_LENGTH => {
                self.t(Cost::FxLength);
                if p != 0 && p <= 0x40 {
                    self.t(Cost::FxLengthSet);
                    self.pattern_length = p as u16;
                }
            }
            FX_SPEED => {
                self.t(Cost::FxSpeed);
                let d1 = p & 0xf;
                if d1 != 0 {
                    self.t(Cost::FxSpeedSet);
                    self.current_speed = (d1 << 4 | d1) as u16;
                }
            }
            // the filter effects do nothing
            7 | 8 => {}
            FX_SHUFFLE => {
                self.t(Cost::FxShuffle);
                self.voices[ch].effect = 0;
                if p & 0xf != 0 {
                    self.t(Cost::FxShuffleTest);
                }
                if p & 0xf != 0 && p & 0xf0 != 0 {
                    self.t(Cost::FxShuffleSet);
                    self.current_speed = p as u16;
                }
            }
            _ => self.t(Cost::FxNone),
        }
    }

    /// The original's routine at $B14 for voice `ch`: the instrument's waveform
    /// effect, the volume envelope, arpeggio, pitch bend and pitch waveform, and
    /// the period.
    fn effects(&mut self, ch: usize, hw: &mut Hw) {
        self.t(Cost::EffectsEntry);
        let ins = self.instrument(hw, self.voices[ch].instrument);
        let r8 = |hw: &Hw, o: u32| hw.bus.r8(ins + o);
        if r8(hw, I_EFFECT) != 0 {
            self.t(Cost::EffectsWaveCheck);
            if r8(hw, I_WAVEFORM) < 0x20 {
                self.t(Cost::ChTab);
                let id = (self.voices[ch].instrument as u8).wrapping_add(1);
                match self.ch_tab.iter().position(|&c| c == id) {
                    Some(k) => self.tn(Cost::ChTabCmp, k as u32 + 1),
                    None => {
                        self.tn(Cost::ChTabCmp, 4);
                        self.t(Cost::ChTabAdd);
                        if self.ch_len < 4 {
                            self.ch_tab[self.ch_len] = id;
                        }
                        self.ch_len += 1;
                        let c = self.voices[ch].fx_counter;
                        if c != 0 {
                            self.t(Cost::FxDec);
                            self.voices[ch].fx_counter = c - 1;
                        } else {
                            self.t(Cost::FxRun);
                            self.voices[ch].fx_counter = r8(hw, I_EFFECT_SPEED);
                            let dst = self.waveform(hw, r8(hw, I_WAVEFORM));
                            self.wave_effect(r8(hw, I_EFFECT), ins, dst, hw);
                            self.t(Cost::FxReturn);
                        }
                    }
                }
            }
        }
        self.t(Cost::EffectsRestore);

        // the volume envelope
        let mut v = self.voices[ch];
        if v.vol_counter != 0 {
            v.vol_counter -= 1;
            if v.vol_counter != 0 {
                self.t(Cost::VolDec);
            } else {
                v.vol_counter = r8(hw, I_VOLUME_SPEED) as u16;
                v.vol_index = (v.vol_index + 1) & 0x7f;
                self.t(Cost::VolStep);
                if v.vol_index == 0 && r8(hw, I_VOLUME_LOOP) & 2 == 0 {
                    v.vol_counter = 0;
                    self.t(Cost::VolEnd);
                } else {
                    if v.vol_index == 0 {
                        self.t(Cost::VolLoopTest);
                    }
                    let a = self
                        .ptr(hw, P_WAVEFORMS)
                        .wrapping_add((((r8(hw, I_VOLUME) as u16) << 7).wrapping_add(v.vol_index)) as u32);
                    let vol = (hw.bus.r8(a).wrapping_add(0x81).wrapping_neg() >> 2) as u16;
                    self.t(Cost::VolCalc);
                    // the original's volume routine writes 0 for its first 13 calls
                    let vol = if self.vol_calls <= 0xc {
                        self.vol_calls += 1;
                        self.t(Cost::VolForced);
                        0
                    } else {
                        vol
                    };
                    self.write(hw, W::Vol);
                    hw.vol(ch, vol);
                    self.t(Cost::VolAfter);
                }
            }
        }

        // the period: arpeggio, transpose, finetune
        self.t(Cost::PerStart);
        let mut n = v.note;
        let arp = r8(hw, I_ARPEGGIO);
        if arp != 0 {
            self.t(Cost::PerArp);
            let a = self.ptr(hw, P_ARPEGGIOS).wrapping_add((arp as u32) << 5).wrapping_add(v.arp_index as u32);
            n = (n & 0xff00) | (n as u8).wrapping_add(hw.bus.r8(a)) as u16;
            v.arp_index = (v.arp_index + 1) & 0x1f;
        }
        self.t(Cost::PerCalc);
        n = n.wrapping_add(ext(v.transpose));
        let base = self.period(hw, n, v.finetune);
        v.period = base;
        if v.effect != FX_NO_WANDER {
            self.t(Cost::PerBendTest);
        }
        if v.effect == FX_NO_WANDER || v.effect == FX_PITCH_BEND {
            self.t(Cost::PerBend);
            let d0 = ext(v.param).wrapping_neg();
            v.bend = v.bend.wrapping_add(d0);
            let p = v.period.wrapping_add(v.bend);
            v.period = p;
            if v.param != 0 {
                self.t(Cost::PerBendCheck);
                if d0 & 0x8000 == 0 {
                    self.t(Cost::PerBendUp);
                }
                let reached = if d0 & 0x8000 != 0 { p <= v.bend_end } else { p >= v.bend_end };
                if reached {
                    self.t(Cost::PerBendClamp);
                    v.bend = v.bend_end.wrapping_sub(base);
                    v.param = 0;
                }
            }
        }

        // the pitch waveform
        self.t(Cost::PitchTest);
        let pitch = r8(hw, I_PITCH);
        if pitch != 0 {
            if v.delay != 0 {
                self.t(Cost::PitchDelay);
                v.delay -= 1;
            } else {
                self.t(Cost::Pitch);
                let a = self.waveform(hw, pitch).wrapping_add(v.pitch_index as u32);
                v.pitch_index = (v.pitch_index + 1) & 0x7f;
                if v.pitch_index == 0 {
                    self.t(Cost::PitchLoop);
                    v.pitch_index = r8(hw, I_PITCH_LOOP) as u16;
                }
                v.period = v.period.wrapping_sub(ext(hw.bus.r8(a)));
            }
        }
        self.voices[ch] = v;
        self.t(Cost::PerWrite);
        self.write(hw, W::Per);
        hw.per(ch, v.period);
        self.t(Cost::PerAfter);
    }

    /// The instrument's waveform effect `e` on waveform `w` (its own waveform),
    /// as the original's routines do it, byte for byte.
    fn wave_effect(&mut self, e: u8, ins: u32, w: u32, hw: &mut Hw) {
        let b = &mut *hw.bus;
        let rd = |b: &crate::bus::Bus, a: u32| b.r8(a);
        let sx = |x: u8| x as i8 as i16;
        let len2 = b.r8(ins + I_LENGTH).wrapping_mul(2);
        // a dbra count of the doubled length, a byte, less one (0 loops 65536 times)
        let count = if len2 == 0 { 0x10000 } else { len2 as u32 };
        let index = |b: &crate::bus::Bus| b.r8(ins + I_EFFECT_INDEX);
        let set_index = |b: &mut crate::bus::Bus, x: u8| b.w8(ins + I_EFFECT_INDEX, x);
        let waves = b.r32(self.mem + POINTERS + P_WAVEFORMS);
        let wave = |n: u8| waves.wrapping_add((n as u32) << 7);
        match e {
            1 => {
                self.t(Cost::FxFilter);
                self.tn(Cost::FxFilterIter, 127);
                filter(b, w);
            }
            2 => {
                self.t(Cost::FxMixing);
                self.tn(Cost::FxMixingIter, count);
                let s1 = wave(rd(b, ins + I_SOURCE1));
                let s2 = wave(rd(b, ins + I_SOURCE2));
                let mut k2 = index(b);
                set_index(b, k2.wrapping_add(1) & 0x7f);
                for k in 0..count {
                    let x = (sx(rd(b, s1.wrapping_add(k))) + sx(rd(b, s2.wrapping_add(k2 as u32)))) >> 1;
                    b.w8(w.wrapping_add(k), x as u8);
                    k2 = k2.wrapping_add(1) & 0x7f;
                }
            }
            3 => {
                self.t(Cost::FxScrollLeft);
                self.tn(Cost::FxScrollIter, 127);
                let first = rd(b, w);
                for k in 0..127 {
                    let x = rd(b, w + k + 1);
                    b.w8(w + k, x);
                }
                b.w8(w + 127, first);
            }
            4 => {
                self.t(Cost::FxScrollRight);
                self.tn(Cost::FxScrollIter, 127);
                let last = rd(b, w + 127);
                for k in (0..127).rev() {
                    let x = rd(b, w + k);
                    b.w8(w + k + 1, x);
                }
                b.w8(w, last);
            }
            5 => {
                self.t(Cost::FxUpsample);
                self.tn(Cost::FxUpsampleIter, 128);
                upsample(b, w);
            }
            6 => {
                self.t(Cost::FxDownsample);
                self.tn(Cost::FxDownsampleIter, 64);
                for k in (0..64).rev() {
                    let x = rd(b, w + k);
                    b.w8(w + 2 * k + 1, x);
                    b.w8(w + 2 * k, x);
                }
            }
            7 => {
                self.t(Cost::FxNegate);
                let k = index(b);
                let a = w.wrapping_add(k as u32);
                let x = rd(b, a).wrapping_neg();
                b.w8(a, x);
                let k = k.wrapping_add(1);
                if len2 <= k {
                    self.t(Cost::FxIndexWrap);
                }
                set_index(b, if len2 > k { k } else { 0 });
            }
            8 => {
                self.t(Cost::FxMadMix);
                self.tn(Cost::FxMadMixIter, count);
                let k = index(b).wrapping_add(1) & 0x7f;
                set_index(b, k);
                let step = rd(b, wave(rd(b, ins + I_SOURCE2)).wrapping_add(k as u32));
                let mut add = 3u8;
                for k in 0..count {
                    let a = w.wrapping_add(k);
                    let x = rd(b, a).wrapping_add(add);
                    b.w8(a, x);
                    add = add.wrapping_add(step);
                }
            }
            9 => {
                self.t(Cost::FxAddition);
                self.tn(Cost::FxAdditionIter, count);
                let s2 = wave(rd(b, ins + I_SOURCE2));
                for k in 0..count {
                    let a = w.wrapping_add(k);
                    let x = rd(b, s2.wrapping_add(k)).wrapping_add(rd(b, a));
                    b.w8(a, x);
                }
            }
            0xa => {
                self.t(Cost::FxFilter2);
                self.tn(Cost::FxFilter2Iter, 126);
                for k in 0..126 {
                    let x = (sx(rd(b, w + k)) * 3 + sx(rd(b, w + k + 2))) >> 2;
                    b.w8(w + k + 1, x as u8);
                }
            }
            0xb | 0xc => {
                // morphing blends waveform 0 (the original loads the second
                // source's pointer into the wrong register) with what lies at
                // the original's pointer to the waveforms plus 128 times the
                // second source: its pointer table and periods
                let (mask, half, shift) = if e == 0xb { (0x7f, 0x40, 6) } else { (0x1f, 0x10, 4) };
                let (s1, s2) = if e == 0xb {
                    (waves, (self.mem + POINTERS + P_WAVEFORMS).wrapping_add((rd(b, ins + I_SOURCE2) as u32) << 7))
                } else {
                    (wave(rd(b, ins + I_SOURCE1)), wave(rd(b, ins + I_SOURCE2)))
                };
                self.t(if e == 0xb { Cost::FxMorph } else { Cost::FxMorphF });
                self.tn(Cost::FxMorphIter, count);
                let k = index(b).wrapping_add(1) & mask;
                set_index(b, k);
                let m1 = if k < half { k } else { mask - k } as u16;
                let m2 = (m1 ^ 0xff) & (half as u16 - 1);
                if k >= half {
                    self.t(Cost::FxMorphHigh);
                }
                // a mulu takes a cycle pair per bit set in its multiplier
                self.tn(Cost::FxMorphBit, count * (m1.count_ones() + m2.count_ones()));
                for k in 0..count {
                    let x1 = (sx(rd(b, s1.wrapping_add(k))) as u16).wrapping_mul(m1);
                    let x2 = (sx(rd(b, s2.wrapping_add(k))) as u16).wrapping_mul(m2);
                    b.w8(w.wrapping_add(k), ((x1.wrapping_add(x2) as i16) >> shift) as u8);
                }
            }
            0xd => {
                self.t(Cost::FxFilter3);
                self.tn(Cost::FxFilter3Iter, 126);
                for k in 0..126 {
                    let x = (sx(rd(b, w + k)) + sx(rd(b, w + k + 2))) >> 1;
                    b.w8(w + k + 1, x as u8);
                }
            }
            0xe => {
                self.t(Cost::FxPolygate);
                let k = index(b);
                let a = w.wrapping_add(k as u32);
                let x = rd(b, a).wrapping_neg();
                b.w8(a, x);
                let k2 = k.wrapping_add(rd(b, ins + I_SOURCE2)) & len2.wrapping_sub(1);
                let a = w.wrapping_add(k2 as u32);
                let x = rd(b, a).wrapping_neg();
                b.w8(a, x);
                let k = k.wrapping_add(1);
                if len2 <= k {
                    self.t(Cost::FxIndexWrap);
                }
                set_index(b, if len2 > k { k } else { 0 });
            }
            0xf => {
                self.t(Cost::FxColgate);
                self.tn(Cost::FxFilterIter, 127);
                filter(b, w);
                let k = index(b).wrapping_add(1);
                set_index(b, k);
                if k == rd(b, ins + I_SOURCE2) {
                    set_index(b, 0);
                    self.t(Cost::FxColgateUp);
                    self.tn(Cost::FxUpsampleIter, 128);
                    upsample(b, w);
                }
            }
            // the jump table's entries 0 and $10-$1F return at once (those past
            // it, which would jump into the code, are taken as these)
            _ => {}
        }
    }
}

/// The filter effect (the original's routine at $1004): each byte the mean of
/// itself and the next, shifted.
fn filter(b: &mut crate::bus::Bus, w: u32) {
    for k in 0..127 {
        let x = (b.r8(w + k) as i8 as i16 + b.r8(w + k + 1) as i8 as i16) >> 1;
        b.w8(w + k, x as u8);
    }
}

/// The upsample effect: every second byte, twice.
fn upsample(b: &mut crate::bus::Bus, w: u32) {
    for k in 0..64 {
        let x = b.r8(w + 2 * k);
        b.w8(w + k, x);
    }
    for k in 0..64 {
        let x = b.r8(w + k);
        b.w8(w + 64 + k, x);
    }
}

impl Drop for DigitalMugician {
    fn drop(&mut self) {
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let _ = f.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PERIODS, PERIOD_START};
    use crate::engine::Engine;
    use std::path::Path;

    /// Subsong 0 ("TEST", speed 3, one position): voice 0 plays note 24 with
    /// instrument 1, a 32-byte waveform whose volume envelope (waveform 1) is
    /// all full volume; the other voices play the empty track 0. Subsong 1
    /// ("TWO") is named too.
    fn module(mark: &[u8; 24]) -> Vec<u8> {
        let mut d = mark.to_vec();
        d.extend(0u16.to_be_bytes()); // no arpeggios
        d.extend(2u16.to_be_bytes()); // tracks
        for k in 0..8u32 {
            d.extend((if k < 2 { 1u32 } else { 0 }).to_be_bytes());
        }
        for n in [1u32, 2, 0, 0] {
            d.extend(n.to_be_bytes()); // instruments, waveforms, samples, sample bytes
        }
        for k in 0..8 {
            let name: &[u8; 12] = match k {
                0 => b"TEST        ",
                1 => b"TWO         ",
                _ => b"            ",
            };
            d.extend([1, 0, 3, if k < 2 { 1 } else { 0 }]);
            d.extend(name);
        }
        d.extend([1, 0, 0, 0, 0, 0, 0, 0]); // subsong 0: voice 0 track 1
        d.extend([1, 0, 0, 0, 0, 0, 0, 0]); // subsong 1 alike
        // waveform 0, 16 words; volume waveform 1 at speed 1, looping
        d.extend([0, 16, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
        d.extend((0..128).map(|i| if i < 16 { 0x40 } else { 0xc0u8 }));
        d.extend([0x80u8; 128]);
        d.extend([0u8; 256]); // track 0
        let mut t = [0u8; 256];
        t[..4].copy_from_slice(&[24, 1, 0, 0]);
        d.extend(t);
        d
    }

    fn run(mark: &[u8; 24]) -> (Engine, Vec<(u32, u16, u64)>, Vec<f32>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("dmu.test"), module(mark)).unwrap();
        let mut out = vec![0f32; 2 * 24000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (e, log, out)
    }

    #[test]
    fn plays_a_waveform_on_paula() {
        let (e, log, out) = run(super::MARK1);
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Digital Mugician 1");
        assert_eq!((p.sub_min, p.sub_max), (0, 1));
        assert_eq!(p.module_name, "TEST        ");
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound's length 2, then the note's 16 words
        assert_eq!(&reg(0xDFF0A4)[..2], &[2, 16]);
        assert_eq!(reg(0xDFF0B4), [2]);
        // the note's period, every tick; the other voices' note 0
        let per = PERIODS[(PERIOD_START + 24) as usize];
        assert_eq!(&reg(0xDFF0A6)[1..4], &[per; 3]);
        assert_eq!(reg(0xDFF0B6)[1], PERIODS[PERIOD_START as usize]);
        // InitSound's volume, the first 13 of the envelope as 0, then 63
        let vol = reg(0xDFF0A8);
        assert!(vol[..14].iter().all(|&v| v == 0));
        assert_eq!(vol[14], 63);
        // the location and length go out before the note's DMA off; every
        // tick ends with DMA on
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..4], &[0x000F, 0x000F, 0x0001, 0x800F]);
        let at = |a: u32, v: u16| log.iter().position(|w| w.0 == a && w.1 == v).unwrap();
        assert!(at(0xDFF0A4, 16) < at(0xDFF096, 0x0001));
        let t = |a: u32, v: u16| log.iter().find(|w| w.0 == a && w.1 == v).unwrap().2;
        assert!(t(0xDFF096, 0x800F) - t(0xDFF096, 0x0001) > 1000);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }

    #[test]
    fn takes_seven_voice_modules_as_four() {
        let (e, log, _) = run(super::MARK2);
        assert_eq!(e.playing.as_ref().unwrap().format, "Digital Mugician 2");
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        assert_eq!(&reg(0xDFF0A4)[..2], &[2, 16]);
    }
}

