//! Ben Daglish, ported from NostalgicPlayer's Ben Daglish player (Copyright (c)
//! 2024 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! A Ben Daglish module is its game's replayer followed by the song, and the
//! replayer comes in several versions. NostalgicPlayer finds the song's tables
//! and the version's features in the code, and so does this port. The original
//! player, the Benn Daglish Eagleplayer (V1.0, Mr.Larmer/Wanted Team), runs the
//! module's own code, patched where it would take over the machine, with its own
//! audio interrupt handler. So this port follows each version's code, as it
//! works on the module in guest memory (its sample mapping, master volume and
//! flags), with the voices' state kept here. The versions it knows, by the code
//! they have (all the ones of the 55 modules checked):
//! * the oldest (no portamento, volume fade or channel volume): DMA on and the
//!   volume written per voice every tick, or one DMACON for all the voices, the
//!   volume only by a note, and an INTREQ clear on the first tick of a one-shot
//!   sample;
//! * the "C0" ones (position commands $80-$BF loop a track, $C0-$C7 map a sample):
//!   the final volume slide, DMA on every tick after the period, volumes clamped
//!   to 64 every tick but not by a note;
//! * the "F0" ones (tracks below $C8, $C8-$EF loops, $F0-$FC maps, $FD the master
//!   volume fade): DMA on in the sample handlers, volumes clamped to the master
//!   volume, a note's own too; some with the final volume slide and no master
//!   volume, some with the sample counter;
//! * the "9x" ones (Motörhead): track effects $9B-$A1, 10 mappings per voice,
//!   a byte fade, and INTENA saved and restored around a note's writes.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * a one-shot sample is stopped by the audio interrupt: on its second tick
//!   the player clears and enables its interrupt, and the handler turns its
//!   DMA off when Paula starts the sample again; on the next tick the player
//!   sees the DMA off, writes AUDxDAT 0 and DMA off, and stops the voice;
//! * a note writes INTENA (its interrupt off), DMA off, location, length,
//!   volume 0 and the note's volume unclamped (in the versions without master
//!   volume) - 255 for a full channel volume; its DMA comes on the next tick;
//! * a sample's loop is written on its second tick whatever its length (0 too),
//!   and the period (and volume) every tick, DMA on every tick in the versions
//!   that write it after the period;
//! * the song ends when the first voice's position list ends (the Eagleplayer
//!   tests only that voice), and starts again at once with the InitSong writes;
//! * no range checks: a note's period index can read past the period table, a
//!   mapping past the voice's own, a track number any offset (sign-extended);
//!   the original's byte and word arithmetic is kept (a track's wait of 0 after
//!   its end in the versions without the tick check is 256 ticks, a note's
//!   extra-tick length of 0 plays as -1);
//! * the Eagleplayer's patches are kept where they change what is played: its
//!   call in place of a note's volume writes leaves the voice's register base
//!   in d4, which the versions with the final volume slide then store as the
//!   slide's step (so a slide set by effect $C6 takes a note's volume below 0
//!   on its first step, and the voice stops);
//! * StopSound's writes are read from the module's code (their order and
//!   values vary by version);
//! * every write lands at the original's colour clock: the tick's start (the
//!   timer) plus half the 68000 cycles of the code paths taken since (enum `K`,
//!   one label per branch, per voice where the voices' code or the
//!   Eagleplayer's calls for them take their own time), and the audio
//!   interrupt's writes at its start plus its own paths. The cycles are a
//!   table per build of the replayer (`BUILDS`, keyed by the hash of the
//!   structure `layout` reads, else the most similar build), fitted exactly on
//!   the Eagleplayer's write logs (two runs, the second with the clock started
//!   one cycle late, give every write's cycle count). The interrupt ends where
//!   the Eagleplayer's ends, and the audio interrupts that came in meanwhile
//!   are taken then. The song starts at once (the Eagleplayer's start-up, its
//!   module check and InitPlayer, 65,000 to 141,000 colour clocks that vary
//!   with the module's code, is not modelled), so where an audio interrupt
//!   falls with a vertical blank the engine takes it a few colour clocks off;
//! * the audio interrupt handler takes the interrupt bits as they stood at the
//!   player's last register write, as amigaplayer-rs passes them to 68k handlers.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Ben Daglish", detect, new };

/// NostalgicPlayer reads the first $3000 bytes to find the replayer's parts.
const SEARCH: usize = 0x3000;

fn w(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

fn b(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The target of a pc-relative instruction at `o` (displacement at `o + 2`).
fn pcrel(d: &[u8], o: usize) -> usize {
    (o as i64 + 2 + w(d, o + 2) as i16 as i64) as usize
}

/// Is `pat` (words, `None` for any) at `o`?
fn at_words(d: &[u8], o: usize, pat: &[Option<u16>]) -> bool {
    pat.iter().enumerate().all(|(i, p)| p.is_none_or(|p| w(d, o + 2 * i) == p))
}

/// The first `o` from `from` (by words, below `to`) where `f` holds.
fn find(d: &[u8], from: usize, to: usize, f: impl Fn(usize) -> bool) -> Option<usize> {
    (from..to.min(d.len())).step_by(2).find(|&o| f(o))
}

/// Where the voices' DMA is turned on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Dma {
    /// After the period, every tick.
    Tick,
    /// In the sample handlers, on the sample's first ticks.
    Handlers,
    /// One DMACON for all the voices at the end of the effects.
    Global,
}

/// What a voice's volume is clamped to every tick.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Clamp {
    None,
    To64,
    Master,
}

/// A variable's place: in the module, or an absolute address (some versions'
/// code was assembled for one, and the Eagleplayer does not relocate it).
#[derive(Clone, Copy, Debug)]
enum Addr {
    Module(u32),
    Abs(u32),
}

impl Addr {
    fn at(self, base: u32) -> u32 {
        match self {
            Addr::Module(o) => base.wrapping_add(o),
            Addr::Abs(a) => a,
        }
    }
}

/// The master volume fade: its variables.
#[derive(Clone, Debug)]
struct Fade {
    /// A byte speed and counter (the 9x version) or words.
    bytes: bool,
    speed: Addr,
    counter: Addr,
    volume: Addr,
    /// Cleared at the fade's end, and whether a word (the play flag, the speed).
    clears: Vec<(Addr, bool)>,
}

/// The replayer's parts and version, as offsets in the module.
#[derive(Clone, Debug)]
struct Layout {
    subsongs: usize,
    samples: usize,
    track_offsets: usize,
    tracks: usize,
    /// The period table's note 0 (the table starts 48 notes before).
    periods: usize,
    mapping: usize,
    /// Mappings per voice (8, or 10 in the 9x version).
    mappings: u8,
    voices: [usize; 4],
    playback: [usize; 4],
    /// The 9x version's track effects ($9B-$A1 for $C0-$C6, mappings $80-$8A).
    nine: bool,
    /// Without portamento, volume fade and channel volume.
    old: bool,
    /// The position lists' commands from $C8 (else from $80).
    f0: bool,
    dma: Dma,
    clamp: Clamp,
    final_slide: bool,
    /// Effect $C7 clears the slide speed's word and sets the channel volume
    /// (else it clears the speed's high byte only).
    c7_word: bool,
    extra_tick: bool,
    check_ticks: bool,
    intena_save: bool,
    fade: Option<Fade>,
    /// The sample counter: the flag byte and the counter word.
    counter: Option<(usize, usize)>,
    /// StopSound's register writes, in its order (it varies), and the instructions before
    /// each since the one before.
    stop: Vec<(u32, u16, StopOps)>,
    /// The Eagleplayer replaces SetupSample's volume writes with a call that
    /// leaves the voice's register base in d4, which the final volume slide's
    /// setup stores after it as its step.
    d4_patched: bool,
    /// SetupSample clears the voice's portamento word itself (`clr.w $a(a1)`
    /// after the period), which takes its time.
    setup_clr: bool,
    /// The code points the search found, the build's layout of its code: from the init
    /// routine, the play routine, the track parser and its effects; from the voices'
    /// effects (which are some way before, by the module), their call of the sample
    /// handler and its return, StopSound, SetupSample's period and the master volume fade.
    marks: [i64; 8],
    /// Where the search found the parts (for the start-up's model): the init routine, the
    /// play routine, the voices' effects, the track parser, the tables.
    at: [usize; 10],
}

/// The instructions of StopSound up to one of its writes: `moveq`s, `nop`s, and whether
/// the write is an immediate's.
#[derive(Clone, Copy, Debug, Default)]
struct StopOps {
    moveq: u8,
    nop: u8,
    imm: bool,
}

/// StopSound's register writes (`moveq #n,d0`, `move.w d0,$x(a6)` and
/// `move.w #n,$x(a6)`, up to the playback structs' lea).
fn stop_writes(d: &[u8], mut o: usize) -> Vec<(u32, u16, StopOps)> {
    let (mut d0, mut out, mut ops) = (0u16, vec![], StopOps::default());
    for _ in 0..40 {
        match w(d, o) {
            0x4df9 => o += 6,
            0x4e71 => {
                ops.nop += 1;
                o += 2;
            }
            x if x & 0xff00 == 0x7000 => {
                d0 = x as u8 as i8 as u16;
                ops.moveq += 1;
                o += 2;
            }
            0x3d40 => {
                out.push((0xdff000 + w(d, o + 2) as u32, d0, ops));
                ops = StopOps::default();
                o += 4;
            }
            0x3d7c => {
                out.push((0xdff000 + w(d, o + 4) as u32, w(d, o + 2), StopOps { imm: true, ..ops }));
                ops = StopOps::default();
                o += 6;
            }
            _ => break,
        }
    }
    out
}

/// Does SetupSample (from its period's two `mulu`, to its return) clear a
/// word at `$a(a1)`?
fn setup_clr(d: &[u8]) -> bool {
    let Some(o) = find(d, 0, d.len().saturating_sub(4), |o| w(d, o) == 0xc2c0 && w(d, o + 2) == 0xc0d3) else {
        return false;
    };
    (o..(o + 0x100).min(d.len().saturating_sub(4)))
        .step_by(2)
        .take_while(|&o| !matches!(w(d, o), 0x4e73 | 0x4e75))
        .any(|o| w(d, o) == 0x4269 && w(d, o + 2) == 0x000a)
}

/// NostalgicPlayer's identification (its feature search), and the rest of what
/// the original's code does differently by version.
fn layout(d: &[u8]) -> Option<Layout> {
    let n = d.len().min(SEARCH);
    let d = &d[..n];
    if d.len() < 0x1600 || w(d, 0) != 0x6000 || w(d, 4) != 0x6000 || w(d, 10) != 0x6000 {
        return None;
    }
    let init = pcrel(d, 0);
    if init + 14 >= n
        || !at_words(d, init, &[Some(0x3f00), Some(0x6100), None, Some(0x3d7c), None, None, Some(0x41fa)])
    {
        return None;
    }
    let play = pcrel(d, 4);
    if play + 16 >= n {
        return None;
    }
    // the tables
    let o = find(d, init, n - 6, |o| w(d, o) == 0x41fa && w(d, o + 4) == 0x2208)?;
    let subsongs = pcrel(d, o);
    let o = find(d, o + 4, n - 6, |o| w(d, o) == 0x41fa && w(d, o + 4) == 0x2348)?;
    let samples = pcrel(d, o);
    let o = find(d, play, n - 6, |o| w(d, o) == 0x47fa && matches!(w(d, o + 4), 0x4880 | 0xd040))?;
    let track_offsets = pcrel(d, o);
    let o = find(d, o + 4, n - 6, |o| w(d, o) == 0x47fa && w(d, o + 4) == 0xd6c0)?;
    let tracks = pcrel(d, o);

    // the effects in the play routine: portamento and volume fade
    let mut porta = false;
    let o = find(d, play, n - 2, |o| w(d, o) == 0x532c)?;
    let mut i = o as i64;
    while i >= play as i64 {
        let o = i as usize;
        if w(d, o) == 0x49fa {
            break;
        }
        if w(d, o) == 0x6100 {
            let m = pcrel(d, o);
            if m + 14 >= n {
                return None;
            }
            if b(d, m) == 0x4a && b(d, m + 1) == 0x2c && b(d, m + 4) == 0x67 && b(d, m + 6) == 0x6a && w(d, m + 8) == 0x3029 {
                porta = true;
            } else if !(w(d, m) == 0x4a2c && b(d, m + 4) == 0x67 && w(d, m + 6) == 0x4a2c && b(d, m + 10) == 0x67) {
                return None;
            }
        }
        i -= 2;
    }
    // the position list's commands
    let o = find(d, play, n - 6, |o| w(d, o) == 0x101b)?;
    let max_track = if matches!(w(d, o + 2), 0xb03c | 0x0c00) { b(d, o + 5) } else { 0x80 };
    let o = find(d, o + 4, n - 6, |o| matches!(w(d, o), 0xb03c | 0x0c00) && b(d, o + 4) == 0x6c)?;
    let f0 = w(d, o + 2) == 0x00f0;
    if !f0 && w(d, o + 2) != 0x00c0 || (max_track == 0x80) == f0 {
        return None;
    }

    // the voices' effects
    if w(d, play) != 0x6100 {
        return None;
    }
    let effects = pcrel(d, play);
    let cb = find(d, effects, n - 2, |o| w(d, o) == 0x4e90)?;
    let mut final_slide = false;
    let mut i = cb as i64;
    while i >= effects as i64 {
        let o = i as usize;
        if w(d, o) == 0x4e75 {
            break;
        }
        if w(d, o) == 0x6100 {
            let m = pcrel(d, o);
            if m + 14 >= n || w(d, m) != 0x302b || b(d, m + 4) != 0x67 {
                return None;
            }
            if w(d, m + 6) == 0x536b {
                final_slide = true;
            } else if !(matches!(w(d, m + 6), 0xb07c | 0x0c40) && w(d, m + 8) == 0xffff) {
                return None;
            }
        }
        i -= 2;
    }
    if !matches!(b(d, cb + 6), 0x6e | 0x66) {
        return None;
    }
    let o = cb + 8 + b(d, cb + 7) as usize;
    let rts = (o..n - 1).find(|&o| w(d, o) == 0x4e75)?;
    let dma_after_period = w(d, rts - 2) == 0x0096;
    let old = !porta;
    // (after a lea of the custom chips' base)
    let global = |o| at_words(d, o, &[Some(0x41fa), None, Some(0x30bc), Some(0x8000)]);
    let dma = if global(effects) || w(d, effects) == 0x4df9 && global(effects + 6) {
        Dma::Global
    } else if dma_after_period {
        Dma::Tick
    } else {
        Dma::Handlers
    };

    // the master volume fade, the first call of the voices' effects
    let mut fade = None;
    if w(d, effects) == 0x6100 {
        let m = pcrel(d, effects);
        if m + 24 >= n {
            return None;
        }
        let bytes = match w(d, m) {
            0x303a | 0x3039 => false,
            0x103a => true,
            _ => return None,
        };
        // the addresses the routine takes, in order: speed, counter, volume, and
        // what it clears at the end
        let mut addrs = Vec::new();
        let mut o = m;
        while o < m + 0x40 && w(d, o) != 0x6000 {
            match w(d, o) {
                0x303a | 0x103a | 0x41fa => {
                    addrs.push((Addr::Module(pcrel(d, o) as u32), false));
                    o += 4;
                }
                0x3039 | 0x41f9 => {
                    addrs.push((Addr::Abs(be32(d, o + 2)), false));
                    o += 6;
                }
                0x4250 => {
                    addrs.last_mut()?.1 = true;
                    o += 2;
                }
                _ => o += 2,
            }
        }
        if addrs.len() < 4 {
            return None;
        }
        fade = Some(Fade {
            bytes,
            speed: addrs[0].0,
            counter: addrs[1].0,
            volume: addrs[2].0,
            clears: addrs[3..].to_vec(),
        });
    }

    // the track parser
    let mut fade_at = 0;
    if w(d, effects) == 0x6100 {
        fade_at = pcrel(d, effects);
    }
    let o = find(d, play, n - 4, |o| w(d, o) == 0x6000)?;
    let parse = pcrel(d, o);
    let o = find(d, parse, n - 8, |o| w(d, o) == 0x4a2c && b(d, o + 4) == 0x67)?;
    let check_ticks = w(d, o + 6) == 0x4a2c;
    let o = find(d, o + 8, n - 6, |o| w(d, o) == 0x7200 && w(d, o + 2) == 0x121b)?;
    let extra_tick = b(d, o + 4) == 0x66;
    // its effects
    let o = find(d, parse, n - 4, |o| w(d, o) == 0x6100)?;
    let m = pcrel(d, o);
    if !matches!(w(d, m + 2), 0xb03c | 0x0c00) || w(d, m + 8) != 0x0240 || b(d, m + 10) != 0 {
        return None;
    }
    let nine = b(d, m + 11) == 0xff;
    let effm = m;

    // what NostalgicPlayer does not look for
    let periods = find(d, 0, effects, |o| w(d, o) == 0x47fa && w(d, o + 4) == 0xd6c0).map(|o| pcrel(d, o))?;
    let o = find(d, init, n - 8, |o| w(d, o) == 0x41fa && w(d, o + 4) == 0x7003 && w(d, o + 6) == 0x7204)?;
    let mapping = pcrel(d, o);
    let o = find(d, o, o + 0x20, |o| matches!(w(d, o), 0xb43c | 0x0c02))?;
    let mappings = b(d, o + 3) / 4;
    let o = find(d, init, n - 16, |o| {
        at_words(d, o, &[Some(0x43fa), None, Some(0x45fa), None, Some(0x47fa), None, Some(0x49fa)])
    })?;
    let voices = [pcrel(d, o), pcrel(d, o + 4), pcrel(d, o + 8), pcrel(d, o + 12)];
    let mut playback = [0; 4];
    let mut o = effects;
    for p in &mut playback {
        o = find(d, o, effects + 0x40, |o| w(d, o) == 0x47fa)?;
        *p = pcrel(d, o);
        o += 4;
    }
    let intena_save = find(d, 0, effects, |o| at_words(d, o, &[Some(0x3f2e), Some(0x001c), Some(0x3d7c), Some(0x7fff)])).is_some();
    let c7_word = find(d, parse, n - 8, |o| matches!(w(d, o), 0xb03c | 0x0c00) && w(d, o + 2) == 0x00c7)
        .is_some_and(|o| w(d, o + 6) == 0x426c);
    let counter = if w(d, play + 4) == 0x103a && b(d, play + 8) == 0x67 && w(d, play + 10) == 0x41fa {
        Some((pcrel(d, play + 4), pcrel(d, play + 10)))
    } else {
        None
    };
    let clamp = if fade.is_some() {
        Clamp::Master
    } else if old {
        Clamp::None
    } else {
        Clamp::To64
    };
    Some(Layout {
        subsongs,
        samples,
        track_offsets,
        tracks,
        periods,
        mapping,
        mappings,
        voices,
        playback,
        nine,
        old,
        f0,
        dma,
        clamp,
        final_slide,
        c7_word,
        extra_tick,
        check_ticks,
        intena_save,
        fade,
        counter,
        stop: stop_writes(d, pcrel(d, init + 2)),
        // the Eagleplayer's search: from the start, $281 words
        d4_patched: find(d, 0, 0x504, |o| at_words(d, o, &[Some(0x3342), Some(0x000c), Some(0x3542), Some(0x0008)])).is_some(),
        setup_clr: setup_clr(d),
        marks: {
            let mul = find(d, 0, d.len().saturating_sub(4), |o| w(d, o) == 0xc2c0 && w(d, o + 2) == 0xc0d3).unwrap_or(init);
            let fade_at = if fade_at == 0 { effects } else { fade_at };
            let (i, e) = (init as i64, effects as i64);
            [play as i64 - i, parse as i64 - i, effm as i64 - i, cb as i64 - e, rts as i64 - e, pcrel(d, init + 2) as i64 - e, mul as i64 - e, fade_at as i64 - e]
        },
        at: [init, play, effects, parse, subsongs, samples, track_offsets, tracks, periods, mapping],
    })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// The original's code paths, each taking its integer number of 68000 cycles (`Build::c`):
/// a tick's writes land at the tick's start plus half the cycles of the paths run since it
/// began (the Eagleplayer's interrupt, the module's play routine and the Eagleplayer's
/// routines in place of some of its writes are all 68k code, with no host call between), and
/// the audio interrupt's at its start plus its own. A name ending in a digit is one voice's
/// path (the voices' code, or the Eagleplayer's calls for them, take their own time).
macro_rules! labels {
    ($($n:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[repr(usize)]
        #[allow(non_camel_case_types, dead_code)]
        enum K { $($n),* }
        const NAMES: &[&str] = &[$(stringify!($n)),*];
    };
}

labels! {
    // the Eagleplayer's interrupt to the play routine, the song's restart, the rest of the
    // interrupt after the play routine
    Tick, SongEnd, TickEnd,
    // the master volume fade: off, counting, a step, the end
    Fade, FadeOff, FadeCount, FadeStep, FadeStepOn, FadeEnd,
    // a voice's effects: idle, playing; its ticks counter
    FxIdle0, FxIdle1, FxIdle2, FxIdle3, Fx0, Fx1, Fx2, Fx3, FxTicks, FxNoTicks,
    // the sample's portamento (none, $FFFF, running, done) and vibrato (running, turning,
    // ending)
    SpNone, SpFF, SpRun, SpDone, VibRun, VibTurn, VibZero,
    // the final volume slide: no speed, counting, a step
    Slide, SlideZero, SlideOn, SlideStep,
    // the looping sample's handler: waiting (then the fade), DMA on, the decrement, the loop
    LoopWait, LoopWaitOn, LoopToFade, LoopDmaOn, LoopDec, LoopRun, LoopEnd, LLoc0, LLoc1, LLoc2, LLoc3, LLen0, LLen1, LLen2, LLen3, LoopTail,
    // the fade handler (its clamp in the oldest version)
    FadeOut, FadeOutClamp, FadeOutNoClamp,
    // the one-shot handler: waiting (its last tick, the sample played), the decrement, the
    // interrupt's set-up, DMA on, the one INTREQ of the global-DMA version
    OnceWait, OnceLast, OnceNotLast, OnceEnd, OnceOn, OnceDec, OnceRun, OnceIntreq, OnceIntena, OnceIrqTail,
    OnceDmaOn, OnceDmaTail, OnceGIntreq, OnceGTail,
    // after the handler: the voice stops, or its volume (clamped), period and DMA on
    Stop, StopDat, StopDma, StopTail, FxClamp, FxNoClamp, FxVol0, FxVol1, FxVol2, FxVol3, FxPer0, FxPer1, FxPer2, FxPer3, FxDmaOn, FxTail,
    // the oldest version's one DMACON
    DmaAll,
    // the play routine after the effects: the sample counter (off, counting, at 0), the play
    // flag off, all voices ended, the voices
    CntOff, CntOn, CntZero, CntNz, Play, PlayOff, PlayAllOff, Voices,
    // a voice's track: ended, playing
    VoiceOff0, VoiceOff1, VoiceOff2, VoiceOff3, Voice0, Voice1, Voice2, Voice3,
    // the track portamento: off, on, its step's calculation, its delay, done, a step
    PortaOff, PortaOn, PortaCalc, PortaNoCalc, PortaDelay, PortaDone, PortaStep,
    // the track's volume fade: off, done, counting, a step (below 0, clamped to 64)
    VfOff, VfDone, VfCount, VfStep, VfNeg, VfClamp, VfNoClamp,
    // the position list: the loop counter (again, the next position), the track as it was
    Position, PosLoop, PosNext, PosSame,
    // the position list's commands: a track, a transpose, the end, the fade speed, a
    // mapping, a loop count
    PcTrack, PcTransp, PcEnd, PcFade, PcMap, PcLoop,
    // the track: waiting (and an effect then), a row's effects (and a new position after
    // one), the row's note or pause
    Wait, WaitFx, WaitNoFx, Row, RowFx, RowPos,
    // a track effect: a mapping, the end, a control value, the oldest version's, the others
    EffMap, EffEnd, EffCtl, EffOld, EffOldC2, Eff0, Eff1, Eff2, Eff3, Eff4, Eff5, Eff6, Eff7, EffBad,
    // a pause, a note (its portamento, its fade, its extra tick's length, the portamento to
    // it and from the note before)
    Pause, Note, NotePorta, NoteNoPorta, NoteFade, NoteNoFade, NoteExtra, NoteNoExtra, NotePorta2, NoteNoPorta2,
    NoteP2Prev, NoteP2New,
    // SetupSample: INTENA saved, the interrupt and DMA off, location, length, volume 0; the
    // vibrato's setup (an odd depth), the loop handler, the master volume's compare (taken),
    // the volume; the end, INTENA restored
    Setup, SIntSave, SInt, SDmaOff, SLoc0, SLoc1, SLoc2, SLoc3, SLen0, SLen1, SLen2, SLen3, SVol0,
    SVibSet, SVibOdd, SVibEven, SVibNone, SLoopH, SOnceH, SMaster, SClampTake, SClampNo, SVol2_0, SVol2_1, SVol2_2, SVol2_3, SetupEnd, SIntRestore,
    // InitSong (StopSound's first write and the others, AUDxDAT, INTREQ, INTENA, the rest),
    // EndSound
    InitSong, StopW0, StopMoveq, StopNop, StopW, StopWImm, InitDat, InitIntreq, InitIntena, InitTail, EndSound,
    // the audio interrupt's handler: to its first write (none to take), its writes
    Irq, IrqNone, IrqIntena, IrqIntreq, IrqDma, IrqB0, IrqB1, IrqB2, IrqB3, IrqEnd,
}

const NK: usize = NAMES.len();

/// A build of the module's replayer (with the Eagleplayer's patches): its signature's hash
/// (`signature`, the replayer's structure as `layout` reads it) and its paths' cycles,
/// fitted to the Eagleplayer's write logs (every write's exact cycle count from a second run
/// with the clock started one cycle late); a path no module of the build takes has a
/// related build's.
struct Build {
    sig: u64,
    name: &'static str,
    c: [u16; NK],
}

/// The build's signature: the version's features, as the layout reads them.
fn signature(l: &Layout) -> String {
    let b = |x: bool| x as u8;
    let fade = match &l.fade {
        None => "-".to_string(),
        Some(f) => {
            let a = |a: &Addr| if matches!(a, Addr::Abs(_)) { 'a' } else { 'm' };
            let c: String = f.clears.iter().map(|(x, w)| format!("{}{}", a(x), b(*w))).collect();
            format!("{}{}{}{}{}", b(f.bytes), a(&f.speed), a(&f.counter), a(&f.volume), c)
        }
    };
    format!(
        "{}{}{}{}{}{}{}{}{}{}{}{} {:?} {:?} {} {} {} {:?}",
        b(l.nine),
        b(l.old),
        b(l.f0),
        b(l.final_slide),
        b(l.c7_word),
        b(l.extra_tick),
        b(l.check_ticks),
        b(l.intena_save),
        b(l.counter.is_some()),
        b(l.d4_patched),
        b(l.setup_clr),
        l.mappings,
        l.dma,
        l.clamp,
        fade,
        l.stop.len(),
        l.stop.first().map_or(0, |s| s.0 & 0xff),
        l.marks,
    )
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3))
}

/// The build of a layout: its signature's, else the one whose signature has the most in
/// common with it.
fn build(l: &Layout) -> &'static Build {
    let s = signature(l);
    let h = fnv(&s);
    if let Some(b) = BUILDS.iter().find(|b| b.sig == h) {
        return b;
    }
    // the features one by one (the flags, then each word), as many in common as can be
    fn toks(s: &str) -> Vec<String> {
        let mut w = s.split([' ', ',', '[', ']']).filter(|x| !x.is_empty());
        let flags = w.next().unwrap_or("");
        flags.chars().map(String::from).chain(w.map(String::from)).collect()
    }
    let own = toks(&s);
    let score = |b: &Build| toks(b.name).iter().zip(&own).filter(|(x, y)| x == y).count();
    BUILDS.iter().max_by_key(|b| score(b)).unwrap_or(&BUILDS[0])
}

/// BD_TRACE=file: each write's clock and the paths since the one before (for fitting the
/// paths' cycles to the original's log).
struct Trace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    pend: Vec<usize>,
}

impl Trace {
    fn open() -> Self {
        let out = std::env::var_os("BD_TRACE").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new);
        Trace { out, pend: Vec::new() }
    }
    fn on(&self) -> bool {
        self.out.is_some()
    }
    fn line(&mut self, s: std::fmt::Arguments) {
        if let Some(o) = &mut self.out {
            use std::io::Write;
            let _ = write!(o, "{s}");
            for e in self.pend.drain(..) {
                let _ = write!(o, " {}", NAMES[e]);
            }
            let _ = writeln!(o);
        }
    }
}

#[rustfmt::skip]
const C_C6149FD2B9440E14: [u16; NK] = [428, 0, 0, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 16, 0, 34, 0, 53, 106, 0, 49, 0, 0, 353, 0, 49, 4, 0, 24, 0, 4, 0, 140, 0, 0, 25, 50, 95, 120, 145, 170, 58, 0, 0, 7, 33, 16, 0, 16, 0, 0, 2, 0, 20, 42, 0, 5, 0, 0, 0, 11, 16, 16, 179, 175, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 467, 0, 73, 0, 55, 0, 18, 18, 0, 0, 195, 358, 0, 0, 0, 49, 254, 21, 46, 0, 193, 0, 0, 79, 0, 0, 0, 83, 60, 0, 0, 120, 98, 8, 0, 0, 51, 38, 0, 141, 88, 25, 0, 0, 94, 47, 120, 81, 0, 0, 180, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 46, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 702, 4, 0, 477, 0, 8, 581, 12, 0, 0, 15, 40, 75, 433, 0, 0, 351, 4, 0, 12, 16, 88, 96, 16, 2009, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_ECCF2909C8EAC5E0: [u16; NK] = [428, 0, 109, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 16, 0, 0, 0, 53, 72, 34, 83, 0, 353, 0, 0, 49, 4, 0, 24, 0, 4, 0, 115, 0, 25, 50, 50, 95, 120, 145, 170, 58, 0, 0, 7, 33, 16, 0, 0, 0, 0, 2, 0, 20, 42, 0, 5, 0, 0, 0, 27, 16, 16, 179, 175, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 358, 0, 73, 0, 55, 0, 18, 18, 0, 149, 195, 358, 0, 0, 0, 49, 0, 21, 46, 0, 193, 0, 0, 79, 0, 83, 0, 0, 60, 0, 0, 120, 98, 113, 88, 0, 156, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 180, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 164, 0, 0, 0, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 702, 4, 0, 477, 0, 8, 581, 12, 0, 0, 15, 40, 75, 324, 0, 0, 351, 4, 0, 12, 16, 88, 96, 16, 2009, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_0549DD7ABBB3A329: [u16; NK] = [519, 0, 397, 0, 0, 0, 0, 0, 0, 0, 90, 36, 10, 0, 18, 0, 0, 302, 0, 0, 0, 19, 72, 0, 49, 0, 0, 316, 0, 49, 4, 0, 24, 0, 32, 0, 202, 0, 25, 0, 75, 95, 120, 145, 170, 21, 0, 0, 7, 3, 16, 0, 0, 0, 0, 2, 12, 20, 0, 0, 0, 0, 0, 0, 69, 16, 16, 216, 221, 0, 15, 35, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 116, 0, 73, 0, 55, 0, 0, 36, 0, 149, 195, 358, 0, 0, 0, 49, 0, 21, 0, 0, 0, 0, 0, 192, 0, 91, 113, 0, 72, 0, 0, 149, 127, 0, 88, 0, 43, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 186, 0, 0, 242, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 150, 0, 0, 0, 0, 0, 581, 12, 0, 0, 15, 36, 0, 0, 0, 0, 351, 4, 0, 12, 16, 88, 96, 16, 2009, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_B93133FB7CA68619: [u16; NK] = [617, 0, 397, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 178, 162, 34, 0, 53, 106, 0, 49, 0, 0, 316, 0, 49, 4, 0, 24, 0, 32, 0, 202, 0, 25, 0, 75, 95, 120, 145, 170, 21, 0, 0, 7, 3, 16, 0, 0, 0, 0, 2, 8, 20, 0, 0, 0, 0, 0, 0, 69, 20, 144, 216, 225, 0, 15, 40, 75, 145, 170, 195, 220, 28, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 73, 0, 55, 0, 18, 18, 0, 262, 195, 358, 0, 0, 0, 49, 0, 21, 46, 193, 0, 0, 0, 79, 0, 91, 0, 0, 0, 0, 0, 153, 127, 0, 88, 0, 43, 126, 0, 57, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 428, 0, 0, 20, 165, 190, 215, 240, 95, 120, 145, 170, 32, 229, 0, 0, 0, 0, 510, 0, 0, 0, 0, 15, 40, 75, 0, 0, 0, 351, 4, 0, 12, 16, 88, 96, 16, 2009, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_475A666E60B07114: [u16; NK] = [485, 0, 243, 0, 0, 0, 0, 0, 0, 218, 0, 126, 205, 0, 18, 18, 0, 0, 0, 0, 302, 19, 72, 0, 49, 0, 0, 316, 0, 49, 4, 0, 24, 0, 32, 0, 202, 0, 25, 0, 75, 95, 120, 145, 170, 21, 0, 0, 7, 3, 16, 0, 0, 0, 0, 2, 12, 20, 0, 0, 0, 0, 0, 0, 69, 16, 16, 216, 221, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 224, 0, 73, 0, 55, 0, 18, 18, 0, 0, 195, 358, 0, 0, 0, 49, 262, 21, 0, 0, 0, 0, 0, 86, 0, 0, 0, 0, 0, 0, 0, 149, 127, 0, 88, 0, 317, 0, 0, 2, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 150, 0, 0, 518, 0, 0, 0, 0, 0, 0, 15, 40, 75, 154, 0, 0, 351, 4, 0, 12, 16, 88, 96, 16, 2009, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_0C23BF0AEC71A447: [u16; NK] = [485, 0, 401, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 16, 0, 0, 286, 53, 72, 34, 83, 0, 0, 316, 0, 49, 4, 0, 24, 5, 27, 0, 207, 0, 25, 0, 75, 95, 120, 145, 170, 16, 0, 0, 7, 3, 16, 0, 0, 0, 0, 2, 12, 20, 0, 0, 0, 0, 0, 0, 69, 16, 16, 216, 221, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 87, 0, 73, 0, 55, 0, 18, 18, 0, 0, 195, 358, 0, 0, 0, 49, 262, 21, 46, 0, 193, 0, 0, 170, 0, 0, 0, 0, 72, 0, 0, 149, 127, 0, 88, 0, 43, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 432, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 225, 4, 0, 518, 0, 0, 0, 0, 0, 0, 15, 40, 75, 0, 0, 0, 454, 4, 0, 12, 16, 88, 96, 16, 1855, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_7AC0F2B85A3EF381: [u16; NK] = [485, 0, 0, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 16, 0, 0, 256, 53, 72, 34, 83, 0, 0, 316, 0, 49, 4, 0, 24, 5, 27, 0, 207, 0, 25, 0, 75, 95, 120, 145, 170, 16, 0, 0, 7, 33, 16, 0, 0, 0, 30, 2, 12, 20, 0, 0, 0, 0, 0, 0, 69, 16, 16, 216, 221, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 488, 0, 73, 0, 55, 0, 18, 18, 0, 0, 195, 358, 0, 0, 0, 49, 262, 21, 46, 0, 193, 0, 0, 79, 0, 7, 0, 84, 72, 0, 0, 149, 127, 0, 88, 0, 43, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 31, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 225, 4, 0, 510, 0, 8, 0, 0, 0, 0, 15, 40, 75, 397, 0, 0, 109, 0, 0, 12, 16, 120, 96, 16, 2252, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_32C82C28C4C081AC: [u16; NK] = [485, 0, 397, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 0, 0, 0, 302, 53, 72, 34, 83, 0, 0, 316, 0, 49, 4, 0, 24, 5, 27, 0, 207, 0, 25, 0, 75, 95, 120, 145, 170, 16, 0, 0, 7, 3, 16, 0, 0, 0, 0, 2, 12, 20, 0, 0, 0, 0, 0, 0, 69, 16, 16, 216, 221, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 91, 0, 73, 0, 55, 0, 18, 18, 0, 149, 195, 358, 0, 0, 0, 49, 0, 21, 46, 0, 193, 0, 0, 108, 0, 91, 29, 0, 72, 0, 0, 149, 127, 84, 88, 0, 127, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 140, 0, 0, 288, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 225, 4, 0, 506, 0, 0, 0, 12, 0, 0, 15, 40, 75, 0, 0, 0, 454, 4, 0, 12, 16, 88, 96, 16, 1855, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_5CBEAC6E17B636EB: [u16; NK] = [771, 0, 0, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 16, 0, 0, 0, 53, 72, 34, 83, 0, 0, 316, 0, 49, 4, 0, 24, 5, 27, 0, 207, 0, 25, 0, 75, 95, 120, 145, 170, 16, 0, 0, 7, 3, 16, 0, 0, 0, 0, 2, 12, 20, 0, 0, 0, 0, 0, 0, 69, 16, 318, 216, 221, 0, 15, 40, 75, 145, 170, 195, 220, 28, 286, 0, 0, 0, 0, 0, 0, 0, 0, 202, 0, 73, 0, 55, 0, 18, 18, 0, 0, 195, 358, 0, 0, 0, 49, 262, 21, 46, 0, 193, 0, 0, 79, 0, 91, 0, 0, 72, 0, 0, 149, 127, 0, 88, 0, 43, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 44, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 225, 4, 0, 0, 0, 506, 0, 12, 0, 0, 15, 40, 75, 397, 0, 0, 489, 4, 0, 12, 16, 136, 96, 16, 1855, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_0E4AF888BA6DF82D: [u16; NK] = [467, 0, 397, 0, 0, 0, 0, 0, 0, 63, 63, 45, 63, 0, 0, 18, 0, 0, 0, 0, 320, 19, 72, 0, 49, 0, 0, 316, 0, 49, 4, 0, 24, 5, 27, 0, 207, 0, 25, 0, 75, 95, 120, 145, 170, 16, 0, 0, 7, 3, 16, 0, 0, 0, 0, 2, 12, 20, 0, 0, 0, 0, 0, 0, 69, 16, 16, 216, 221, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 82, 0, 73, 0, 55, 0, 9, 0, 0, 0, 195, 358, 0, 0, 0, 49, 271, 21, 46, 0, 193, 0, 0, 79, 0, 91, 0, 0, 72, 0, 0, 149, 127, 0, 88, 0, 43, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 428, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 150, 0, 0, 518, 0, 0, 0, 0, 0, 0, 15, 40, 75, 0, 0, 0, 454, 4, 0, 12, 16, 88, 96, 16, 1855, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_947A4714873A681E: [u16; NK] = [485, 0, 397, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 16, 0, 34, 0, 53, 106, 0, 49, 0, 0, 316, 0, 49, 4, 0, 24, 5, 27, 0, 207, 0, 25, 0, 75, 95, 120, 145, 170, 16, 0, 0, 7, 289, 16, 0, 16, 0, 286, 2, 12, 20, 0, 0, 0, 0, 0, 0, 53, 16, 32, 216, 221, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 91, 0, 73, 0, 55, 0, 18, 18, 0, 149, 195, 358, 0, 0, 0, 49, 0, 21, 46, 0, 193, 0, 0, 108, 0, 91, 29, 0, 72, 0, 0, 149, 127, 84, 88, 0, 127, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 130, 0, 0, 311, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 225, 4, 0, 0, 0, 522, 0, 12, 0, 0, 15, 40, 75, 0, 0, 0, 489, 4, 0, 12, 16, 136, 96, 16, 1855, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_AFAD72EA4D02C78C: [u16; NK] = [428, 0, 433, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 16, 0, 0, 0, 53, 72, 34, 83, 0, 0, 316, 0, 49, 4, 0, 24, 0, 32, 0, 152, 0, 25, 50, 75, 95, 120, 145, 170, 21, 0, 0, 7, 33, 16, 0, 16, 0, 25, 2, 12, 20, 5, 0, 5, 0, 0, 0, 48, 16, 16, 216, 212, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 34, 0, 73, 0, 55, 0, 18, 18, 0, 0, 195, 358, 0, 0, 0, 49, 262, 21, 46, 0, 193, 0, 0, 79, 0, 4, 0, 87, 72, 0, 0, 132, 110, 0, 88, 0, 43, 126, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 180, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 488, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 706, 0, 0, 477, 0, 8, 581, 12, 0, 0, 15, 40, 75, 0, 0, 0, 351, 4, 0, 12, 16, 88, 96, 16, 2009, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_15F54DF7465885E6: [u16; NK] = [466, 0, 215, 0, 0, 0, 0, 0, 0, 63, 63, 63, 63, 0, 0, 0, 0, 258, 242, 0, 0, 53, 72, 34, 83, 0, 0, 353, 0, 49, 0, 0, 24, 0, 0, 0, 157, 0, 25, 25, 75, 95, 120, 145, 170, 16, 0, 0, 7, 29, 16, 0, 0, 0, 18, 0, 20, 20, 0, 0, 5, 0, 0, 0, 69, 16, 52, 179, 166, 0, 0, 25, 60, 0, 25, 50, 75, 28, 64, 0, 0, 0, 0, 0, 0, 0, 0, 252, 0, 73, 0, 55, 0, 18, 18, 0, 0, 195, 358, 0, 0, 0, 49, 254, 21, 46, 0, 193, 0, 0, 223, 0, 0, 144, 83, 60, 0, 0, 120, 98, 8, 0, 0, 49, 38, 0, 141, 88, 25, 0, 0, 94, 47, 120, 81, 0, 0, 180, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 229, 0, 0, 0, 489, 497, 581, 12, 0, 0, 27, 52, 75, 186, 0, 0, 351, 4, 0, 12, 16, 88, 96, 16, 2009, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_D6C9C91850402100: [u16; NK] = [491, 0, 397, 0, 0, 0, 0, 0, 0, 0, 45, 63, 45, 0, 0, 18, 0, 16, 0, 0, 298, 53, 72, 34, 83, 0, 0, 353, 0, 49, 0, 0, 24, 0, 0, 0, 157, 0, 25, 25, 75, 95, 120, 145, 170, 16, 0, 0, 7, 31, 16, 0, 0, 0, 0, 0, 0, 20, 40, 0, 5, 0, 0, 0, 29, 16, 16, 179, 175, 0, 0, 25, 60, 0, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 88, 0, 73, 0, 55, 0, 0, 18, 0, 0, 195, 358, 0, 0, 0, 49, 254, 21, 46, 0, 193, 0, 0, 231, 0, 181, 152, 0, 0, 0, 0, 120, 0, 0, 88, 0, 41, 186, 0, 53, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 180, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 0, 0, 0, 182, 0, 0, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 229, 0, 0, 524, 489, 0, 581, 12, 0, 0, 0, 25, 48, 0, 0, 0, 351, 4, 0, 12, 16, 88, 96, 16, 2009, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];
#[rustfmt::skip]
const C_04BF5386AA8D4ACE: [u16; NK] = [485, 0, 349, 0, 0, 0, 0, 0, 0, 45, 63, 63, 45, 0, 18, 18, 0, 272, 256, 34, 0, 53, 106, 0, 49, 0, 0, 316, 0, 49, 4, 0, 24, 5, 27, 0, 182, 0, 0, 25, 50, 95, 120, 145, 170, 16, 0, 0, 7, 33, 16, 0, 16, 0, 30, 0, 12, 20, 0, 0, 0, 0, 0, 0, 53, 16, 32, 216, 221, 0, 15, 40, 75, 145, 170, 195, 220, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0, 139, 0, 73, 0, 55, 0, 18, 18, 0, 149, 195, 358, 0, 0, 0, 49, 0, 21, 46, 0, 193, 0, 0, 79, 0, 91, 0, 0, 72, 0, 0, 149, 127, 113, 88, 0, 156, 126, 0, 61, 0, 25, 0, 0, 94, 47, 120, 81, 0, 0, 132, 0, 0, 0, 0, 106, 0, 92, 0, 18, 0, 0, 142, 0, 0, 261, 0, 20, 16, 161, 186, 211, 236, 95, 120, 145, 170, 32, 225, 4, 0, 0, 0, 8, 498, 12, 0, 0, 15, 40, 75, 76, 0, 0, 109, 0, 0, 12, 16, 120, 96, 16, 2252, 0, 0, 0, 32, 12, 32, 0, 0, 0, 0, 224];

/// The builds known, by their signature's hash and signature (an unknown build takes the
/// one whose signature has the most in common with it).
static BUILDS: &[Build] = &[
    Build { sig: 0xc6149fd2b9440e14, name: "000100000108 Tick To64 - 9 150 [376, 620, 894, 76, 136, -68, -180, 0]", c: C_C6149FD2B9440E14 },
    Build { sig: 0xeccf2909c8eac5e0, name: "000111000108 Tick To64 - 9 150 [376, 620, 904, 76, 136, -68, -180, 0]", c: C_ECCF2909C8EAC5E0 },
    Build { sig: 0x0549dd7abbb3a329, name: "001001000108 Handlers Master 0mmmm1 9 150 [374, 640, 916, 76, 128, -68, -178, 394]", c: C_0549DD7ABBB3A329 },
    Build { sig: 0xb93133fb7ca68619, name: "001001100108 Handlers Master 0aaaa0 9 150 [378, 674, 964, 76, 134, -68, -182, 406]", c: C_B93133FB7CA68619 },
    Build { sig: 0x475a666e60b07114, name: "001001100108 Handlers Master 0mmmm1 9 150 [374, 640, 930, 76, 128, -68, -178, 394]", c: C_475A666E60B07114 },
    Build { sig: 0x0c23bf0aec71a447, name: "001001101108 Handlers Master 0mmma0 9 150 [374, 662, 952, 76, 128, -68, -180, 394]", c: C_0C23BF0AEC71A447 },
    Build { sig: 0x7ac0f2b85a3ef381, name: "001001101108 Handlers Master 0mmmm0 5 168 [374, 662, 952, 76, 128, -68, -178, 394]", c: C_7AC0F2B85A3EF381 },
    Build { sig: 0x32c82c28c4c081ac, name: "001001101108 Handlers Master 0mmmm0 9 150 [374, 662, 952, 76, 128, -68, -178, 394]", c: C_32C82C28C4C081AC },
    Build { sig: 0x5cbeac6e17b636eb, name: "001001101108 Handlers Master 0mmmm0 9 150 [374, 662, 958, 76, 132, -84, -194, 398]", c: C_5CBEAC6E17B636EB },
    Build { sig: 0x0e4af888ba6df82d, name: "001001101108 Handlers Master 0mmmm1 9 150 [374, 662, 952, 76, 128, -68, -178, 394]", c: C_0E4AF888BA6DF82D },
    Build { sig: 0x947a4714873a681e, name: "001001101118 Handlers Master 0mmmm0 9 150 [374, 662, 958, 76, 132, -84, -198, 398]", c: C_947A4714873A681E },
    Build { sig: 0xafad72ea4d02c78c, name: "001111000108 Handlers To64 - 9 150 [376, 624, 908, 76, 126, -68, -180, 0]", c: C_AFAD72EA4D02C78C },
    Build { sig: 0x15f54df7465885e6, name: "010000000008 Global None - 9 150 [280, 516, 616, 114, 160, -70, -176, 0]", c: C_15F54DF7465885E6 },
    Build { sig: 0xd6c9c91850402100, name: "010000000008 Tick None - 9 150 [280, 516, 616, 72, 124, -68, -172, 0]", c: C_D6C9C91850402100 },
    Build { sig: 0x04bf5386aa8d4ace, name: "1010011110010 Handlers Master 1mmmm0m0 9 150 [374, 658, 956, 76, 132, -84, -204, 396]", c: C_04BF5386AA8D4ACE },
];

/// What a playing voice runs every tick after its note (its `$20` pointer).
#[derive(Clone, Copy, PartialEq, Default, Debug)]
enum Handler {
    #[default]
    Once,
    Loop,
    Fade,
}

/// A voice's playing state (the original's `$22`-`$28` byte structures).
#[derive(Clone, Copy, Default)]
struct Pb {
    /// Its DMA and interrupt bits.
    bit: u16,
    int: u16,
    /// The ticks to the loop: 2 after a note, then $FFFF, $8000 when done.
    delay: u16,
    period: u16,
    porta: u16,
    volume: u16,
    slide_speed: u16,
    slide_add: u16,
    slide_count: u16,
    ticks: u16,
    sporta: u16,
    sporta_add: u16,
    vib_depth: u16,
    period_add: u16,
    vib_add: u16,
    handler: Handler,
    sample: u32,
}

/// A voice's track state (the original's `$44`-`$48` byte structures).
#[derive(Clone, Copy, Default)]
struct Voice {
    /// The offset of its sample mapping (the structure's first word).
    map: u16,
    positions: u32,
    track_start: u32,
    track: u32,
    sample: u32,
    loops: u8,
    transpose: u8,
    next_position: u8,
    wait: u8,
    enabled: u8,
    note: u8,
    new_note: u8,
    sample2: u32,
    prev_note: u8,
    porta1: u8,
    porta_delay: u8,
    porta_dur: u8,
    porta_delta: u8,
    porta_on: u8,
    porta_delay_count: u8,
    porta_dur_count: u8,
    /// A long from the period table; its high word becomes the step.
    porta_step: u32,
    porta2: u8,
    fade_on: u8,
    fade_speed: u8,
    fade_dur: u8,
    fade_add: u16,
    fading: u8,
    fade_speed2: u8,
    fade_count: u8,
    fade_dur_count: u8,
    fade_step: u16,
    fade_value: u16,
    chan_volume: u16,
    slide_speed: u16,
    slide_add: u16,
}

pub struct BenDaglish {
    base: u32,
    l: Layout,
    pb: [Pb; 4],
    v: [Voice; 4],
    sub: u16,
    subsong_count: u16,
    songend: bool,
    /// The oldest version's DMACON word, built by the voices' effects.
    dma_all: u16,
    /// d3 and d4 of the original's play routine: a note's slide, which a
    /// volume fade step after it in the same tick passes on.
    d3: u16,
    d4: u16,
    /// The build's cycles per path.
    c: &'static [u16; NK],
    /// The clock the cycles count from (the tick's or the audio interrupt's start), and the
    /// cycles since.
    clk: u64,
    cyc: u64,
    trace: Trace,
    tickno: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = layout(d)?;
    let bus = &mut m.bus;
    let mut pb = [Pb::default(); 4];
    let mut v = [Voice::default(); 4];
    for ch in 0..4 {
        let p = base + l.playback[ch] as u32;
        pb[ch].bit = bus.r16(p);
        pb[ch].int = bus.r16(p + 2);
        v[ch].map = bus.r16(base + l.voices[ch] as u32);
    }
    // the Eagleplayer's subsong count: the entries whose four offsets are below $400
    let mut subsong_count = 0u16;
    while (0..4).all(|i| be16(d, l.subsongs + 8 * subsong_count as usize + 2 * i) & 0xfc00 == 0)
        && (subsong_count as usize) < d.len() / 8
    {
        subsong_count += 1;
    }
    let trace = Trace::open();
    let sig = signature(&l);
    let c = &build(&l).c;
    let mut r = BenDaglish {
        base,
        l,
        pb,
        v,
        sub: 0,
        subsong_count: subsong_count.max(1),
        songend: false,
        dma_all: 0,
        d3: 0,
        d4: 0,
        c,
        clk: 0,
        cyc: 0,
        trace,
        tickno: 0,
    };
    r.trace.line(format_args!("S {} {:016x}", sig.replace(' ', "_"), fnv(&sig)));
    r.trace.line(format_args!("L {} {:?}", d.len(), r.l.at));
    let mut hw = Hw { bus };
    r.start(&mut hw, 'B');
    r.init_song(0, &mut hw);
    Some(Box::new(r))
}

impl Replayer for BenDaglish {
    fn format(&self) -> &'static str {
        "Ben Daglish"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsong_count - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // DeliTracker calls EndSound, then InitSound
        self.start(hw, 'B');
        self.end_sound(hw);
        self.init_song(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tickno += 1;
        self.start(hw, 'T');
        self.k(K::Tick);
        self.play(hw);
        // the Eagleplayer's interrupt: when the first voice has ended, the song
        // starts again, and that is its end
        if self.v[0].enabled == 0 {
            self.k(K::SongEnd);
            self.init_song(self.sub, hw);
            self.songend = true;
        }
        // the rest of the play routine and of the interrupt: audio interrupts
        // wait for its end
        self.k(K::TickEnd);
        self.settle(hw);
    }

    fn audio_irq(&mut self, hw: &mut Hw) -> bool {
        // the Eagleplayer's handler: the requested audio interrupts' DMA off
        self.start(hw, 'I');
        self.k(K::Irq);
        // the handler's interrupt bits as they stood at the 68k's last register
        // access, as amigaplayer-rs passes them to 68k handlers (not synced to now)
        let bits = hw.bus.paula.intreq & hw.bus.paula.intena & 0x0780;
        if bits == 0 {
            // taken with an earlier one's
            self.k(K::IrqNone);
            self.settle(hw);
            return true;
        }
        self.wr(hw, K::IrqIntena, 0xdff09a, bits);
        self.wr(hw, K::IrqIntreq, 0xdff09c, bits);
        self.wr(hw, K::IrqDma, 0xdff096, bits >> 7);
        // the rest of the handler (by the voices it took): a later interrupt waits for its end
        for ch in 0..4 {
            if bits & 0x80 << ch != 0 {
                self.kc(K::IrqB0, ch);
            }
        }
        self.k(K::IrqEnd);
        self.settle(hw);
        true
    }
}

impl BenDaglish {
    /// The original's code takes path `e`.
    fn k(&mut self, e: K) {
        self.ki(e as usize);
    }

    /// Voice `ch`'s path `e` (the first voice's of four in a row).
    fn kc(&mut self, e: K, ch: usize) {
        self.ki(e as usize + ch);
    }

    fn ki(&mut self, i: usize) {
        self.cyc += self.c[i] as u64;
        if self.trace.on() {
            self.trace.pend.push(i);
        }
    }

    /// The cycles count from now (`kind`: B the start-up or a subsong change, T a tick, I an
    /// audio interrupt).
    fn start(&mut self, hw: &mut Hw, kind: char) {
        self.clk = hw.bus.clock;
        self.cyc = 0;
        self.trace.pend.clear();
        self.trace.line(format_args!("{kind} {} {}", self.tickno, self.clk));
    }

    /// The clock reaches the code's time.
    fn settle(&mut self, hw: &mut Hw) {
        let t = self.clk + self.cyc / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
        self.trace.line(format_args!("E {} {}", self.tickno, hw.bus.clock));
    }

    /// A register write after path `e`.
    fn wr(&mut self, hw: &mut Hw, e: K, a: u32, v: u16) {
        self.k(e);
        self.put(hw, a, v);
    }

    fn put(&mut self, hw: &mut Hw, a: u32, v: u16) {
        let t = self.clk + self.cyc / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
        self.trace.line(format_args!("W {} {:06x} {:04x} {}", self.tickno, a, v, hw.bus.clock));
        hw.bus.write_word(a, v);
    }

    fn reg(ch: usize, r: u32) -> u32 {
        0xdff0a0 + 16 * ch as u32 + r
    }

    fn m(&self, o: usize) -> u32 {
        self.base + o as u32
    }

    /// The master volume (64 in the versions without).
    fn master(&self, hw: &Hw) -> u16 {
        match &self.l.fade {
            Some(f) => hw.bus.r16(f.volume.at(self.base)),
            None => 0x40,
        }
    }

    /// StopSound: all DMA off, periods $F, volumes 0, the voices idle.
    fn stop_sound(&mut self, hw: &mut Hw) {
        for i in 0..self.l.stop.len() {
            let (a, v, ops) = self.l.stop[i];
            if i == 0 {
                self.k(K::StopW0);
            }
            for _ in 0..ops.moveq {
                self.k(K::StopMoveq);
            }
            for _ in 0..ops.nop {
                self.k(K::StopNop);
            }
            self.wr(hw, if ops.imm { K::StopWImm } else { K::StopW }, a, v);
        }
        for p in &mut self.pb {
            p.delay = 0;
        }
    }

    /// EndSound (the module's third entry): the play flag cleared, the sound
    /// stopped.
    fn end_sound(&mut self, hw: &mut Hw) {
        self.k(K::EndSound);
        let b = &mut *hw.bus;
        let mut o = self.base + 0x0e;
        if b.r16(o) == 0x6000 {
            o = o.wrapping_add(2).wrapping_add(b.r16(o + 2) as i16 as u32);
        }
        if b.r16(o) == 0x41fa {
            let a = o.wrapping_add(2).wrapping_add(b.r16(o + 2) as i16 as u32);
            if b.r16(o + 4) == 0x4250 {
                b.w16(a, 0);
            } else {
                b.w8(a, 0);
            }
        }
        self.stop_sound(hw);
    }

    /// InitSong (the module's first entry) for subsong `n`.
    fn init_song(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.k(K::InitSong);
        self.stop_sound(hw);
        self.wr(hw, K::InitDat, 0xdff09e, 0x00ff);
        let b = &mut *hw.bus;
        if let Some(f) = &self.l.fade {
            b.w16(f.volume.at(self.base), 0x40);
            if f.bytes {
                b.w8(f.speed.at(self.base), 0);
            } else {
                b.w16(f.speed.at(self.base), 0);
            }
        }
        // the filter off (the LED)
        let pra = b.read_byte(0xbfe001);
        b.write_byte(0xbfe001, pra | 2);
        self.wr(hw, K::InitIntreq, 0xdff09c, 0x0780);
        self.wr(hw, K::InitIntena, 0xdff09a, 0x0780);
        let b = &mut *hw.bus;
        let sub = (n & 0xff) as u32;
        let table = self.m(self.l.subsongs);
        for ch in 0..4 {
            let v = &mut self.v[ch];
            v.positions = table + b.r16(table + 8 * sub + 2 * ch as u32) as u32;
            v.transpose = 0;
            v.porta1 = 0;
            v.fade_on = 0;
            v.porta2 = 0;
            if self.l.final_slide {
                v.slide_speed &= 0x00ff;
            }
            v.next_position = 1;
            v.new_note = 1;
            v.loops = 1;
            v.wait = 1;
            v.sample = self.base + self.l.samples as u32;
            v.enabled = 0xff;
            v.chan_volume = 0xffff;
        }
        let map = self.m(self.l.mapping);
        let n = self.l.mappings as u32;
        for i in 0..4 * n {
            b.w8(map + i, 4 * (i % n) as u8);
        }
        b.w8(self.base + 8, 0xff);
        self.k(K::InitTail);
    }

    /// The module's play routine (its second entry).
    fn play(&mut self, hw: &mut Hw) {
        self.effects(hw);
        let b = &mut *hw.bus;
        if let Some((flag, counter)) = self.l.counter {
            if b.r8(self.m(flag)) != 0 {
                self.k(K::CntOn);
                let c = b.r16(self.m(counter)).wrapping_sub(1);
                b.w16(self.m(counter), c);
                if c == 0 {
                    self.k(K::CntZero);
                    b.w16(self.m(counter), 6);
                    return;
                }
                self.k(K::CntNz);
            } else {
                self.k(K::CntOff);
            }
        }
        self.k(K::Play);
        if b.r8(self.base + 8) == 0 {
            self.k(K::PlayOff);
            return;
        }
        let on = self.v.iter().fold(0, |a, v| a | v.enabled);
        b.w8(self.base + 8, on);
        if on == 0 {
            self.k(K::PlayAllOff);
            return;
        }
        self.k(K::Voices);
        (self.d3, self.d4) = (0, 0);
        for ch in 0..4 {
            self.voice(ch, hw);
        }
    }

    /// The voices' effects and sample handlers, and their volume, period and
    /// DMA writes.
    fn effects(&mut self, hw: &mut Hw) {
        if let Some(f) = self.l.fade.clone() {
            self.master_fade(&f, hw);
        }
        self.dma_all = 0x8000;
        for ch in 0..4 {
            self.voice_effects(ch, hw);
        }
        if self.l.dma == Dma::Global {
            self.wr(hw, K::DmaAll, 0xdff096, self.dma_all);
        }
    }

    fn master_fade(&mut self, f: &Fade, hw: &mut Hw) {
        self.k(K::Fade);
        let b = &mut *hw.bus;
        let base = self.base;
        let speed = if f.bytes { b.r8(f.speed.at(base)) as u16 } else { b.r16(f.speed.at(base)) };
        if speed == 0 {
            self.k(K::FadeOff);
            return;
        }
        let c = f.counter.at(base);
        if f.bytes {
            let n = b.r8(c).wrapping_sub(1);
            b.w8(c, n);
            if (n as i8) >= 0 {
                self.k(K::FadeCount);
                return;
            }
            b.w8(c, (speed as u8).wrapping_sub(1));
        } else {
            let n = b.r16(c).wrapping_sub(1);
            b.w16(c, n);
            if n != 0 {
                self.k(K::FadeCount);
                return;
            }
            b.w16(c, speed);
        }
        self.k(K::FadeStep);
        let vol = f.volume.at(base);
        let n = b.r16(vol).wrapping_sub(1);
        b.w16(vol, n);
        if (n as i16) >= 0 {
            self.k(K::FadeStepOn);
            return;
        }
        for &(a, word) in &f.clears {
            if word {
                b.w16(a.at(base), 0);
            } else {
                b.w8(a.at(base), 0);
            }
        }
        self.k(K::FadeEnd);
        self.stop_sound(hw);
    }

    fn voice_effects(&mut self, ch: usize, hw: &mut Hw) {
        if self.pb[ch].delay == 0 {
            self.kc(K::FxIdle0, ch);
            return;
        }
        self.kc(K::Fx0, ch);
        let p = &mut self.pb[ch];
        if (p.ticks as i16) > 0 {
            p.ticks -= 1;
            self.k(K::FxTicks);
        } else {
            self.k(K::FxNoTicks);
        }
        self.sample_effects(ch, hw);
        if self.l.final_slide {
            self.k(K::Slide);
            let p = &mut self.pb[ch];
            if p.slide_speed != 0 {
                p.slide_count = p.slide_count.wrapping_sub(1);
                let step = p.slide_count == 0;
                if step {
                    p.slide_count = p.slide_speed;
                    p.volume = p.volume.wrapping_add(p.slide_add);
                }
                self.k(K::SlideOn);
                if step {
                    self.k(K::SlideStep);
                }
            } else {
                self.k(K::SlideZero);
            }
        }
        match self.pb[ch].handler {
            Handler::Loop => self.handle_loop(ch, hw),
            Handler::Fade => self.handle_fade(ch, hw),
            Handler::Once => self.handle_once(ch, hw),
        }
        let p = self.pb[ch];
        let stop = if self.l.old { p.volume == 0 } else { (p.volume as i16) <= 0 };
        if stop {
            self.k(K::Stop);
            self.wr(hw, K::StopDat, Self::reg(ch, 0x0a), 0);
            self.wr(hw, K::StopDma, 0xdff096, p.bit);
            self.k(K::StopTail);
            self.pb[ch].delay = 0;
            return;
        }
        let mut vol = p.volume;
        let max = match self.l.clamp {
            Clamp::None => None,
            Clamp::To64 => Some(0x40),
            Clamp::Master => Some(self.master(hw)),
        };
        if let Some(m) = max
            && (vol as i16) > (m as i16)
        {
            vol = m;
            self.k(K::FxClamp);
        } else {
            self.k(K::FxNoClamp);
        }
        if self.l.dma != Dma::Global {
            self.kc(K::FxVol0, ch);
            self.put(hw, Self::reg(ch, 8), vol);
        }
        let per = p.period.wrapping_add(p.period_add).wrapping_add(p.porta);
        self.kc(K::FxPer0, ch);
        self.put(hw, Self::reg(ch, 6), per);
        match self.l.dma {
            Dma::Tick => self.wr(hw, K::FxDmaOn, 0xdff096, 0x8000 | p.bit),
            Dma::Global => self.dma_all |= p.bit,
            Dma::Handlers => {}
        }
        self.k(K::FxTail);
    }

    /// The sample's portamento, then its vibrato.
    fn sample_effects(&mut self, ch: usize, hw: &mut Hw) {
        let p = self.pb[ch];
        if p.sporta != 0 {
            if p.sporta == 0xffff {
                self.k(K::SpFF);
                return;
            }
            let p = &mut self.pb[ch];
            p.period_add = p.period_add.wrapping_add(p.sporta_add);
            p.sporta -= 1;
            if p.sporta != 0 {
                self.k(K::SpRun);
                return;
            }
            self.k(K::SpDone);
        } else {
            self.k(K::SpNone);
        }
        let p = &mut self.pb[ch];
        p.period_add = p.period_add.wrapping_add(p.vib_add);
        p.vib_depth = p.vib_depth.wrapping_sub(1);
        if p.vib_depth != 0 {
            self.k(K::VibRun);
            return;
        }
        let depth = hw.bus.r16(p.sample + 0x14);
        if depth != 0 {
            p.vib_depth = depth;
            p.vib_add = p.vib_add.wrapping_neg();
            self.k(K::VibTurn);
        } else {
            self.k(K::VibZero);
        }
    }

    fn handle_loop(&mut self, ch: usize, hw: &mut Hw) {
        let p = self.pb[ch];
        if (p.delay as i16) < 0 {
            self.k(K::LoopWait);
            let ticks = p.ticks as i16;
            let next = if self.l.old { ticks != 1 } else { ticks > 1 };
            if !next {
                self.k(K::LoopToFade);
                self.pb[ch].handler = Handler::Fade;
            } else {
                self.k(K::LoopWaitOn);
            }
            return;
        }
        if self.l.dma == Dma::Handlers {
            self.wr(hw, K::LoopDmaOn, 0xdff096, 0x8000 | p.bit);
        }
        self.k(K::LoopDec);
        let d = p.delay - 1;
        self.pb[ch].delay = d;
        if d != 0 {
            self.k(K::LoopRun);
            return;
        }
        self.pb[ch].delay = !d;
        self.k(K::LoopEnd);
        let s = p.sample;
        let a = self.m(self.l.samples).wrapping_add(hw.bus.r32(s + 4));
        let len = hw.bus.r16(s + 0x0a);
        self.kc(K::LLoc0, ch);
        self.put(hw, Self::reg(ch, 0), (a >> 16) as u16);
        hw.bus.write_word(Self::reg(ch, 2), a as u16);
        self.kc(K::LLen0, ch);
        self.put(hw, Self::reg(ch, 4), len);
        self.k(K::LoopTail);
    }

    fn handle_fade(&mut self, ch: usize, hw: &mut Hw) {
        self.k(K::FadeOut);
        let p = &mut self.pb[ch];
        p.delay = 0x8000;
        p.volume = p.volume.wrapping_add(hw.bus.r16(p.sample + 0x0e));
        if self.l.old && (p.volume as i16) < 0 {
            p.volume = 0;
            self.k(K::FadeOutClamp);
        } else if self.l.old {
            self.k(K::FadeOutNoClamp);
        }
    }

    fn handle_once(&mut self, ch: usize, hw: &mut Hw) {
        let p = self.pb[ch];
        if (p.delay as i16) < 0 {
            self.k(K::OnceWait);
            if p.ticks == 1 {
                self.k(K::OnceLast);
                self.pb[ch].delay = 0x8000;
            } else {
                self.k(K::OnceNotLast);
            }
            // the interrupt has turned the DMA off: the sample has played
            if p.bit & hw.bus.paula.dmacon == 0 {
                self.k(K::OnceEnd);
                self.pb[ch].volume = 0;
            } else {
                self.k(K::OnceOn);
            }
            return;
        }
        self.k(K::OnceDec);
        let d = p.delay - 1;
        self.pb[ch].delay = d;
        if d == 0 {
            self.pb[ch].delay = !d;
            self.wr(hw, K::OnceIntreq, 0xdff09c, p.int);
            self.wr(hw, K::OnceIntena, 0xdff09a, 0x8000 | p.int);
            self.k(K::OnceIrqTail);
        } else if self.l.dma == Dma::Handlers {
            self.wr(hw, K::OnceDmaOn, 0xdff096, 0x8000 | p.bit);
            self.k(K::OnceDmaTail);
        } else if self.l.dma == Dma::Global {
            self.wr(hw, K::OnceGIntreq, 0xdff09c, p.int);
            self.k(K::OnceGTail);
        } else {
            self.k(K::OnceRun);
        }
    }

    /// A voice's track: its portamento and volume fade, its position list and
    /// track when it is time, and the note.
    fn voice(&mut self, ch: usize, hw: &mut Hw) {
        if self.v[ch].enabled == 0 {
            self.kc(K::VoiceOff0, ch);
            return;
        }
        self.kc(K::Voice0, ch);
        if !self.l.old {
            self.portamento(ch);
            self.volume_fade(ch, hw);
        }
        let mut a3;
        'position: loop {
            if self.v[ch].next_position != 0 {
                self.k(K::Position);
                let v = &mut self.v[ch];
                v.loops = v.loops.wrapping_sub(1);
                if v.loops != 0 {
                    v.track = v.track_start;
                    v.next_position = 0;
                    a3 = v.track;
                    self.k(K::PosLoop);
                } else {
                    v.loops = 1;
                    self.k(K::PosNext);
                    match self.positions(ch, hw) {
                        Some(t) => a3 = t,
                        None => return,
                    }
                }
            } else {
                self.k(K::PosSame);
                a3 = self.v[ch].track;
            }
            let v = &mut self.v[ch];
            v.wait = v.wait.wrapping_sub(1);
            if v.wait != 0 {
                self.k(K::Wait);
                if hw.bus.r8(a3) & 0x80 != 0 {
                    self.k(K::WaitFx);
                    self.effect(ch, &mut a3, hw);
                } else {
                    self.k(K::WaitNoFx);
                }
                self.v[ch].track = a3;
                return;
            }
            self.k(K::Row);
            loop {
                if hw.bus.r8(a3) & 0x80 == 0 {
                    break;
                }
                self.k(K::RowFx);
                self.effect(ch, &mut a3, hw);
                if self.v[ch].next_position != 0 {
                    self.k(K::RowPos);
                    let v = &mut self.v[ch];
                    if self.l.check_ticks && v.wait == 0 {
                        v.wait = 1;
                    }
                    continue 'position;
                }
            }
            break;
        }
        self.note(ch, a3, hw);
    }

    /// The position list's commands up to a track: its first byte, or None when
    /// the list has ended.
    fn positions(&mut self, ch: usize, hw: &mut Hw) -> Option<u32> {
        let mut a3 = self.v[ch].positions;
        loop {
            let b = &mut *hw.bus;
            let cmd = b.r8(a3);
            a3 = a3.wrapping_add(1);
            let track = if self.l.f0 { cmd < 0xc8 } else { cmd < 0x80 };
            if track {
                self.k(K::PcTrack);
                let off = b.r16(self.m(self.l.track_offsets) + 2 * cmd as u32) as i16;
                let t = self.m(self.l.tracks).wrapping_add(off as u32);
                let v = &mut self.v[ch];
                v.next_position = 0;
                v.positions = a3;
                v.track_start = t;
                return Some(t);
            }
            match cmd {
                0xfe => {
                    self.k(K::PcTransp);
                    self.v[ch].transpose = b.r8(a3);
                    a3 = a3.wrapping_add(1);
                }
                0xff => {
                    self.k(K::PcEnd);
                    let d = self.pb[ch].delay;
                    if d == 0 || d == 0x8000 {
                        self.v[ch].enabled = 0;
                    }
                    return None;
                }
                0xfd if self.l.f0 && self.l.fade.is_some() => {
                    self.k(K::PcFade);
                    let f = self.l.fade.as_ref().unwrap();
                    let s = b.r8(a3);
                    a3 = a3.wrapping_add(1);
                    if f.bytes {
                        b.w8(f.speed.at(self.base), s);
                    } else {
                        b.w16(f.speed.at(self.base), s as i8 as u16);
                    }
                }
                _ => {
                    let map = self.m(self.l.mapping);
                    let v = &mut self.v[ch];
                    let (mapping, i) = if self.l.f0 {
                        ((cmd as i8) >= (0xf0u8 as i8), cmd.wrapping_sub(0xf0) as u16)
                    } else {
                        ((cmd as i8) >= (0xc0u8 as i8), (cmd & 7) as u16)
                    };
                    if mapping {
                        let a = map.wrapping_add(i.wrapping_add(v.map) as i16 as u32);
                        b.w8(a, b.r8(a3));
                        a3 = a3.wrapping_add(1);
                        self.k(K::PcMap);
                    } else {
                        if self.l.f0 {
                            v.loops = cmd.wrapping_sub(0xc8);
                        } else {
                            v.loops = cmd & 0x1f;
                        }
                        self.k(K::PcLoop);
                    }
                }
            }
        }
    }

    /// A track effect (a byte with bit 7 set) at `a3`.
    fn effect(&mut self, ch: usize, a3: &mut u32, hw: &mut Hw) {
        let b = &mut *hw.bus;
        let mut next = || {
            let v = b.r8(*a3);
            *a3 = a3.wrapping_add(1);
            v
        };
        let cmd = next();
        let (last_map, first) = if self.l.nine { (0x8a, 0x9b) } else { (0x88, 0xc0) };
        if (cmd as i8) <= (last_map as i8) {
            let i = if self.l.nine { (cmd as u16).wrapping_sub(0x80) } else { (cmd & 7) as u16 };
            let i = i.wrapping_add(self.v[ch].map) as i16 as u32;
            let s = b.r8(self.m(self.l.mapping).wrapping_add(i)) as u32;
            let table = self.m(self.l.samples);
            self.v[ch].sample = table.wrapping_add(b.r32(table + s));
            self.k(K::EffMap);
            return;
        }
        if cmd == 0xff {
            self.v[ch].next_position = 0xff;
            self.k(K::EffEnd);
            return;
        }
        if (cmd as i8) < (first as i8) {
            // the "control" value, which the player keeps but does not use
            self.k(K::EffCtl);
            return;
        }
        let v = &mut self.v[ch];
        if self.l.old {
            if cmd != 0xc2 {
                *a3 = a3.wrapping_add(3);
                self.k(K::EffOld);
            } else {
                self.k(K::EffOldC2);
            }
            return;
        }
        let e = cmd - first;
        match e {
            0 => {
                v.porta1 = 0xff;
                v.porta2 = 0;
                v.porta_delay = next();
                v.porta_dur = next();
                v.porta_delta = next();
            }
            1 => v.porta1 = 0,
            2 => {
                v.fade_on = 0xff;
                v.fade_speed = next();
                v.fade_dur = next();
                v.fade_add = next() as i8 as u16;
            }
            3 => v.fade_on = 0,
            4 => {
                v.porta2 = 0xff;
                v.porta1 = 0;
                v.porta_dur = next();
            }
            5 => v.porta2 = 0,
            6 => {
                v.chan_volume = (next() as u16) << 8 | (v.chan_volume & 0xff);
                if self.l.final_slide {
                    v.slide_speed = next() as u16;
                    v.slide_add = next() as i8 as u16;
                }
            }
            7 if self.l.final_slide => {
                if self.l.c7_word {
                    v.slide_speed = 0;
                    v.chan_volume = 0xffff;
                } else {
                    v.slide_speed &= 0x00ff;
                }
            }
            // an illegal instruction in the original
            _ => {}
        }
        self.ki(if e <= 7 { K::Eff0 as usize + e as usize } else { K::EffBad as usize });
    }

    /// A long of the period table, `i` bytes from its note 0.
    fn period_long(&self, hw: &Hw, i: i16) -> u32 {
        hw.bus.r32(self.m(self.l.periods).wrapping_add(i as u32))
    }

    /// A row's note (or pause) at `a3`.
    fn note(&mut self, ch: usize, mut a3: u32, hw: &mut Hw) {
        let b = &mut *hw.bus;
        let note = b.r8(a3);
        a3 = a3.wrapping_add(1);
        if note == 0x7f {
            self.k(K::Pause);
            self.v[ch].wait = b.r8(a3);
            self.v[ch].track = a3.wrapping_add(1);
            return;
        }
        self.k(K::Note);
        if !self.l.old {
            self.pb[ch].porta = 0;
        }
        let mut d0 = note.wrapping_add(self.v[ch].transpose);
        self.v[ch].note = d0;
        if !self.l.old {
            let v = self.v[ch];
            self.v[ch].porta_on = v.porta1;
            if v.porta1 != 0 {
                self.k(K::NotePorta);
                let step = self.period_long(hw, (v.porta_delta as i8 as i16).wrapping_neg().wrapping_mul(4));
                let v = &mut self.v[ch];
                v.porta_delay_count = v.porta_delay;
                v.porta_dur_count = v.porta_dur;
                v.porta_step = step;
            } else {
                self.k(K::NoteNoPorta);
            }
            let v = &mut self.v[ch];
            v.fading = v.fade_on;
            if v.fade_on != 0 {
                v.fade_speed2 = v.fade_speed;
                v.fade_count = v.fade_speed;
                v.fade_dur_count = v.fade_dur;
                v.fade_step = v.fade_add;
                v.fade_value = 0;
                self.k(K::NoteFade);
            } else {
                self.k(K::NoteNoFade);
            }
        }
        let b = &mut *hw.bus;
        let t = b.r8(a3);
        a3 = a3.wrapping_add(1);
        let ticks = if self.l.extra_tick && t == 0 {
            self.v[ch].wait = b.r8(a3);
            a3 = a3.wrapping_add(1);
            self.k(K::NoteExtra);
            0xffff
        } else {
            self.v[ch].wait = t;
            if self.l.extra_tick {
                self.k(K::NoteNoExtra);
            }
            t as u16
        };
        if !self.l.old {
            if self.v[ch].porta2 != 0 {
                self.k(K::NotePorta2);
                let v = &mut self.v[ch];
                v.porta_on = 0xff;
                v.porta_delay_count = 0;
                v.porta_dur_count = v.porta_dur;
                let d2 = d0;
                if v.new_note == 0 {
                    d0 = v.prev_note;
                    self.k(K::NoteP2Prev);
                } else {
                    self.k(K::NoteP2New);
                }
                let v = &mut self.v[ch];
                v.note = d0;
                let i = (d2.wrapping_sub(d0) as i8 as i16).wrapping_neg().wrapping_mul(4);
                self.v[ch].porta_step = self.period_long(hw, i);
            } else {
                self.k(K::NoteNoPorta2);
            }
            let v = &mut self.v[ch];
            v.prev_note = v.note;
        }
        self.v[ch].track = a3;
        let s = self.v[ch].sample;
        let vol = if self.l.old {
            hw.bus.r16(s + 0x0c)
        } else {
            let v = &mut self.v[ch];
            v.sample2 = s;
            v.new_note = 0;
            if self.l.final_slide {
                (self.d3, self.d4) = (v.slide_speed, v.slide_add);
            }
            ((hw.bus.r16(s + 0x0c) as u32 * v.chan_volume as u32) >> 14) as u16
        };
        self.setup(ch, s, d0, ticks, vol, hw);
    }

    /// The track portamento (note to note).
    fn portamento(&mut self, ch: usize) {
        let v = &mut self.v[ch];
        if v.porta_on == 0 {
            self.k(K::PortaOff);
            return;
        }
        self.k(K::PortaOn);
        let v = &mut self.v[ch];
        if v.porta_on & 0x80 != 0 {
            let p = self.pb[ch].period as u32;
            let s = v.porta_step;
            let d0 = ((p * (s & 0xffff)) >> 16) as u16;
            let d0 = d0.wrapping_add((p * (s >> 16)) as u16).wrapping_sub(p as u16);
            let n = d0 as i16 as i32;
            let d = v.porta_dur_count as i32;
            // divs.w: the quotient, the dividend kept when it overflows (and no
            // step for a duration of 0, which traps in the original)
            if d != 0 {
                let q = n / d;
                let q = if q < i16::MIN as i32 || q > i16::MAX as i32 { n as u16 } else { q as u16 };
                v.porta_step = (q as u32) << 16 | (s & 0xffff);
            }
            v.porta_on &= 0x7f;
            self.k(K::PortaCalc);
        } else {
            self.k(K::PortaNoCalc);
        }
        let v = &mut self.v[ch];
        if v.porta_delay_count != 0 {
            v.porta_delay_count -= 1;
            self.k(K::PortaDelay);
            return;
        }
        if v.porta_dur_count == 0 {
            self.k(K::PortaDone);
            return;
        }
        v.porta_dur_count -= 1;
        let p = &mut self.pb[ch];
        p.porta = p.porta.wrapping_add((v.porta_step >> 16) as u16);
        self.k(K::PortaStep);
    }

    /// The track's volume fade: a note again at each step.
    fn volume_fade(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.v[ch];
        if v.fading == 0 {
            self.k(K::VfOff);
            return;
        }
        if v.fade_dur_count == 0 {
            self.k(K::VfDone);
            return;
        }
        v.fade_count = v.fade_count.wrapping_sub(1);
        if v.fade_count != 0 {
            self.k(K::VfCount);
            return;
        }
        self.k(K::VfStep);
        let v = &mut self.v[ch];
        v.fade_dur_count -= 1;
        v.fade_count = v.fade_speed2;
        let d2 = v.fade_value.wrapping_add(v.fade_step);
        v.fade_value = d2;
        let s = v.sample2;
        let d2 = d2.wrapping_add(hw.bus.r16(s + 0x0c));
        if (d2 as i16) < 0 {
            self.v[ch].fade_dur_count = 0;
            self.k(K::VfNeg);
            return;
        }
        let d2 = if (d2 as i16) > 0x40 {
            self.k(K::VfClamp);
            0x40
        } else {
            self.k(K::VfNoClamp);
            d2
        };
        let v = self.v[ch];
        let (note, ticks) = (v.note, v.fade_speed2 as u16);
        self.setup(ch, s, note, ticks, d2, hw);
    }

    /// The original's SetupSample: a note of sample `s` on voice `ch`.
    fn setup(&mut self, ch: usize, s: u32, note: u8, ticks: u16, vol: u16, hw: &mut Hw) {
        self.k(K::Setup);
        let saved = hw.bus.paula.intena;
        if self.l.intena_save {
            self.wr(hw, K::SIntSave, 0xdff09a, 0x7fff);
        }
        let p = self.pb[ch];
        self.wr(hw, K::SInt, 0xdff09a, p.int);
        self.wr(hw, K::SDmaOff, 0xdff096, p.bit);
        let b = &mut *hw.bus;
        let a = self.m(self.l.samples).wrapping_add(b.r32(s));
        let len = b.r16(s + 8);
        self.kc(K::SLoc0, ch);
        self.put(hw, Self::reg(ch, 0), (a >> 16) as u16);
        hw.bus.write_word(Self::reg(ch, 2), a as u16);
        self.kc(K::SLen0, ch);
        self.put(hw, Self::reg(ch, 4), len);
        self.wr(hw, K::SVol0, Self::reg(ch, 8), 0);
        let b = &mut *hw.bus;
        // the period: the table's long at the note (from the sample's transpose)
        // times the sample's fine tune, as words
        let i = (b.r16(s + 0x18).wrapping_sub((note & 0x7f) as u16)).wrapping_mul(4) as i16;
        let t = self.m(self.l.periods).wrapping_add(i as u32);
        let ft = b.r16(s + 0x1a) as u32;
        let period = (((b.r16(t + 2) as u32 * ft) >> 16) as u16).wrapping_add((b.r16(t) as u32 * ft) as u16);
        let sporta = b.r16(s + 0x10);
        let depth = b.r16(s + 0x14);
        let pm = &mut self.pb[ch];
        pm.sample = s;
        pm.ticks = ticks;
        pm.period = period;
        pm.sporta = sporta;
        if (sporta as i16) >= 0 {
            pm.vib_depth = (depth >> 1) + (depth & 1);
            let scaled = |o: u32| ((b.r16(s + o) as i16 as i32 * period as i16 as i32) >> 15) as u16;
            pm.sporta_add = scaled(0x12);
            pm.vib_add = scaled(0x16);
        }
        pm.period_add = 0;
        if !self.l.old && self.l.f0 {
            pm.porta = 0;
        }
        pm.handler = if b.r16(s + 0x0e) != 0 { Handler::Loop } else { Handler::Once };
        let looped = pm.handler == Handler::Loop;
        if (sporta as i16) >= 0 {
            self.k(K::SVibSet);
            if depth & 1 != 0 {
                self.k(K::SVibOdd);
            } else {
                self.k(K::SVibEven);
            }
        } else {
            self.k(K::SVibNone);
        }
        if looped {
            self.k(K::SLoopH);
        } else {
            self.k(K::SOnceH);
        }
        let mut vol = vol;
        if self.l.clamp == Clamp::Master {
            // the master volume's compare (and move)
            self.k(K::SMaster);
            let m = self.master(hw);
            if (vol as i16) > (m as i16) {
                vol = m;
                self.k(K::SClampTake);
            } else {
                self.k(K::SClampNo);
            }
        }
        self.pb[ch].volume = vol;
        self.kc(K::SVol2_0, ch);
        self.put(hw, Self::reg(ch, 8), vol);
        let pm = &mut self.pb[ch];
        if self.l.final_slide {
            pm.slide_speed = self.d3;
            pm.slide_count = self.d3;
            pm.slide_add = if self.l.d4_patched { 0xf0a0 + 0x10 * ch as u16 } else { self.d4 };
        }
        pm.delay = 2;
        self.k(K::SetupEnd);
        if self.l.intena_save {
            self.wr(hw, K::SIntRestore, 0xdff09a, saved | 0x8000);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    // a module of the oldest version: its replayer's code, as far as the search
    // for its parts looks at it, then its song
    const STOP: usize = 0x20;
    const SETUP: usize = 0x100;
    const EFFECTS: usize = 0x200;
    const FX: usize = 0x240;
    const SFX: usize = 0x280;
    const PLAY: usize = 0x300;
    const PARSE: usize = 0x400;
    const EFFECT: usize = 0x480;
    const INIT: usize = 0x500;
    const PB: usize = 0x600;
    const VOICES: usize = 0x700;
    const SUBSONGS: usize = 0x1000;
    const TRACK_OFFSETS: usize = 0x1100;
    const TRACKS: usize = 0x1200;
    const SAMPLES: usize = 0x1300;
    const DATA: usize = SAMPLES + 0x100;
    const PERIODS: usize = 0x1500;
    const MAPPING: usize = 0x1580;

    fn put(d: &mut [u8], at: usize, words: &[u16]) {
        for (i, w) in words.iter().enumerate() {
            d[at + 2 * i..at + 2 * i + 2].copy_from_slice(&w.to_be_bytes());
        }
    }

    /// The displacement of a pc-relative instruction at `at` to `to`.
    fn rel(at: usize, to: usize) -> u16 {
        (to as i64 - at as i64 - 2) as u16
    }

    fn module() -> Vec<u8> {
        let mut d = vec![0u8; 0x1600];
        put(&mut d, 0, &[0x6000, rel(0, INIT), 0x6000, rel(4, PLAY), 0, 0x6000, 0]);
        // EndSound, StopSound
        put(&mut d, 0x0e, &[0x41fa, 0xfff8, 0x4250, 0x6000, rel(0x14, STOP)]);
        let mut stop = vec![0x4df9, 0x00df, 0xf000, 0x700f];
        for r in [0x96, 0xa6, 0xb6, 0xc6, 0xd6] {
            stop.extend([0x3d40, r]);
        }
        stop.push(0x7000);
        for r in [0xa8, 0xb8, 0xc8, 0xd8] {
            stop.extend([0x3d40, r]);
        }
        stop.extend([0x41fa, 0, 0x4e75]);
        put(&mut d, STOP, &stop);
        put(&mut d, SETUP, &[0x47fa, rel(SETUP, PERIODS), 0xd6c0, 0x4e75]);
        // the voices' effects: a voice, its sample's effects, its handler, the
        // volume and period, DMA on
        let mut fx = vec![0x4df9, 0x00df, 0xf000];
        for ch in 0..4 {
            let at = EFFECTS + 2 * fx.len();
            fx.extend([0x47fa, rel(at, PB + 0x20 * ch), if ch < 3 { 0x6100 } else { 0x6000 }, rel(at + 4, FX)]);
        }
        put(&mut d, EFFECTS, &fx);
        put(&mut d, FX, &[0x4a6b, 6, 0x6602, 0x4e75, 0x6100, rel(FX + 8, SFX), 0x206b, 0x20, 0x302b, 4, 0x4e90]);
        put(&mut d, FX + 0x16, &[0x302b, 0x0c, 0x6e04, 0x4269, 0x0a, 0x3d40, 0x96, 0x4e75]);
        put(&mut d, SFX, &[0x302b, 0x16, 0x6716, 0xb07c, 0xffff, 0x4e75]);
        // the play routine: the effects, then the voices' position lists and
        // tracks
        put(&mut d, PLAY, &[0x6100, rel(PLAY, EFFECTS), 0x49fa, rel(PLAY + 4, VOICES), 0x532c, 0x15]);
        put(&mut d, PLAY + 12, &[0x101b, 0xb03c, 0x80, 0x6d02, 0xb03c, 0xc0, 0x6c02]);
        let o = PLAY + 26;
        put(&mut d, o, &[0x47fa, rel(o, TRACK_OFFSETS), 0x4880, 0x47fa, rel(o + 6, TRACKS), 0xd6c0, 0x6000, rel(o + 12, PARSE)]);
        put(&mut d, PARSE, &[0x4a2c, 0x14, 0x6702, 0x4e71, 0x7200, 0x121b, 0x6002, 0x6100, rel(PARSE + 14, EFFECT), 0x4e75]);
        put(&mut d, EFFECT, &[0x101b, 0xb03c, 0x88, 0x6e00, 0x0240, 7, 0x4e75]);
        // InitSong: StopSound, the tables, 8 mappings per voice, the voices
        let mut init = vec![0x3f00, 0x6100, rel(INIT + 2, STOP), 0x3d7c, 0xff, 0x9e];
        for (to, op) in [(SUBSONGS, 0x2208), (SAMPLES, 0x2348), (MAPPING, 0x7003)] {
            let at = INIT + 2 * init.len();
            init.extend([0x41fa, rel(at, to), op]);
        }
        init.extend([0x7204, 0xb43c, 0x20]);
        for (ch, op) in [0x43fa, 0x45fa, 0x47fa, 0x49fa].into_iter().enumerate() {
            let at = INIT + 2 * init.len();
            init.extend([op, rel(at, VOICES + 0x40 * ch)]);
        }
        init.push(0x4e75);
        put(&mut d, INIT, &init);
        for ch in 0..4 {
            put(&mut d, PB + 0x20 * ch, &[1 << ch, 0x80 << ch]);
            put(&mut d, VOICES + 0x40 * ch, &[8 * ch as u16]);
        }
        // two subsongs: the first voice plays track 0 (in both), the others
        // end at once
        put(&mut d, SUBSONGS, &[0x10, 0x14, 0x14, 0x14, 0x12, 0x14, 0x14, 0x14, 0x00ff, 0x00ff, 0xffff]);
        // track 0: sample 0, note $18 for 16 ticks, the end
        d[TRACKS..TRACKS + 4].copy_from_slice(&[0x80, 0x18, 0x10, 0xff]);
        // sample 0: 64 words played once at volume 64, no effects, at the
        // period table's note 0 with no fine tune ($1AC)
        put(&mut d, SAMPLES, &[0, 0x10]);
        put(&mut d, SAMPLES + 0x10, &[0, 0x100, 0, 0x100, 0x40, 0x40, 0x40, 0, 0, 0, 0, 0, 0x18, 1]);
        for i in 0..128 {
            d[DATA + i] = if i < 64 { 0x40 } else { 0xc0 };
        }
        put(&mut d, PERIODS, &[0x1ac, 0]);
        d
    }

    fn start() -> Engine {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.bd"), module()).unwrap();
        e
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = start();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Ben Daglish");
        assert_eq!((p.sub_min, p.sub_max, p.sub), (0, 1, 0));
        let base = e.m.deli.list.first().unwrap().0;
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let w: Vec<(u32, u16)> = log.iter().map(|w| (w.0, w.1)).filter(|w| w.0 >= 0xdff096).collect();
        // InitSong: StopSound, the filter, the audio interrupts off
        let init = [
            (0xdff096, 0x000f),
            (0xdff0a6, 0x000f),
            (0xdff0b6, 0x000f),
            (0xdff0c6, 0x000f),
            (0xdff0d6, 0x000f),
            (0xdff0a8, 0),
            (0xdff0b8, 0),
            (0xdff0c8, 0),
            (0xdff0d8, 0),
            (0xdff09e, 0x00ff),
            (0xdff09c, 0x0780),
            (0xdff09a, 0x0780),
        ];
        assert_eq!(&w[..12], &init);
        // the note: its interrupt and DMA off, the sample, volume 0 then 64; on
        // the next tick the volume, the period and DMA on
        let data = base + DATA as u32;
        let note = [
            (0xdff09a, 0x0080),
            (0xdff096, 0x0001),
            (0xdff0a0, (data >> 16) as u16),
            (0xdff0a2, data as u16),
            (0xdff0a4, 0x40),
            (0xdff0a8, 0),
            (0xdff0a8, 0x40),
            (0xdff0a8, 0x40),
            (0xdff0a6, 0x1ac),
            (0xdff096, 0x8001),
        ];
        assert_eq!(&w[12..22], &note);
        // its second tick enables its interrupt; the interrupt turns its DMA
        // off when Paula starts it again, and the next tick stops the voice
        let on = w.iter().position(|&x| x == (0xdff09a, 0x8080)).expect("interrupt enabled");
        assert_eq!(w[on - 1], (0xdff09c, 0x0080));
        let irq = on + w[on..].iter().position(|&x| x == (0xdff09a, 0x0080)).expect("audio interrupt");
        assert_eq!(&w[irq..irq + 3], &[(0xdff09a, 0x0080), (0xdff09c, 0x0080), (0xdff096, 0x0001)]);
        assert!(w[irq + 3..].windows(2).any(|p| p == [(0xdff0aa, 0), (0xdff096, 0x0001)]));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
