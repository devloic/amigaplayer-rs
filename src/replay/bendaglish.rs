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
//! * the writes are spaced as the original's code spaces them (`At::cost`,
//!   fitted to the Eagleplayer's write logs), from the timer to the end of the
//!   Eagleplayer's interrupt, where it takes the audio interrupts that came in
//!   the meantime. The Eagleplayer's calls in place of the location, length,
//!   volume and period writes take longer the higher the voice (the `*_CH`
//!   tables), a note's setup varies with the sample (looped or not, its
//!   portamento, the volume clamp), and the "9x" version has its own costs
//!   (`At::cost9`); all fitted to the timings seen across the modules;
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
    /// StopSound's register writes, in its order (it varies).
    stop: Vec<(u32, u16)>,
    /// The Eagleplayer replaces SetupSample's volume writes with a call that
    /// leaves the voice's register base in d4, which the final volume slide's
    /// setup stores after it as its step.
    d4_patched: bool,
    /// SetupSample clears the voice's portamento word itself (`clr.w $a(a1)`
    /// after the period), which takes its time.
    setup_clr: bool,
}

/// StopSound's register writes (`moveq #n,d0`, `move.w d0,$x(a6)` and
/// `move.w #n,$x(a6)`, up to the playback structs' lea).
fn stop_writes(d: &[u8], mut o: usize) -> Vec<(u32, u16)> {
    let (mut d0, mut out) = (0u16, vec![]);
    for _ in 0..40 {
        match w(d, o) {
            0x4df9 => o += 6,
            0x4e71 => o += 2,
            x if x & 0xff00 == 0x7000 => {
                d0 = x as u8 as i8 as u16;
                o += 2;
            }
            0x3d40 => {
                out.push((0xdff000 + w(d, o + 2) as u32, d0));
                o += 4;
            }
            0x3d7c => {
                out.push((0xdff000 + w(d, o + 4) as u32, w(d, o + 2)));
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
    })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// Code points of the original where it spends the time between its writes:
/// each costs the colour clocks it takes (`cost`), and a write waits the costs
/// run since the one before, and its own kind's. The costs are a least-squares
/// fit to the gaps between the Eagleplayer's writes on the 55 modules (median
/// error 7 colour clocks, 90% within 19), but `Tick`, `TickEnd` and `Irq`, which
/// come from the Eagleplayer's code (its interrupt before and after the play
/// routine's call, its audio interrupt handler up to its first write), and a
/// few the logs do not show.
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
enum At {
    /// From the tick to the voices' effects (the Eagleplayer's interrupt and
    /// the play routine's call), and the master volume fade.
    Tick,
    Fade,
    FadeStep,
    /// A voice's effects: idle, or playing up to its sample handler.
    FxIdle,
    Fx,
    SamplePorta,
    SampleVib,
    SampleVibTurn,
    Slide,
    SlideStep,
    /// The sample handlers: looping (before the loop, the decrement, the end
    /// of the loop's wait, the fade), one-shot (the same, the interrupt's).
    LoopWait,
    LoopFade,
    LoopDec,
    LoopEnd,
    FadeOut,
    OnceWait,
    OnceDec,
    OnceEnd,
    /// After the handler: the voice stops, or writes volume and period.
    Stop,
    Play,
    /// The one DMACON of the oldest version, after the voices.
    DmaAll,
    /// From the effects to the voices, and the voices.
    Voices,
    VoiceOff,
    Voice,
    Porta,
    PortaCalc,
    PortaStep,
    VolFade,
    VolFadeStep,
    /// The position list: the loop counter, each command, a new track.
    Position,
    PositionCmd,
    Track,
    /// The track: waiting (and an effect then), each effect of a row, a pause,
    /// a note.
    Wait,
    Effect,
    Pause,
    Note,
    NotePorta,
    NoteFade,
    NotePorta2,
    /// A note's writes: from the parser, the sample handler set up after them.
    Setup,
    SetupEnd,
    /// Each kind of write, from the code before it.
    WIntena,
    WIntreq,
    WDmaOff,
    WDmaOn,
    WLoc,
    /// A note's location and length (the Eagleplayer's calls in their place
    /// take longer the higher the voice: `SLOC_CH`, `SLEN_CH`).
    SLoc,
    SLen,
    WStop0,
    WStop,
    WVol,
    WVol0,
    WVol2,
    WPer,
    WDat,
    /// After the tick's last write, to the end of the interrupt.
    TickEnd,
    /// The audio interrupt's writes, from Paula raising it.
    Irq,
    IrqIntena,
    IrqIntreq,
    IrqDma,
}

impl At {
    /// The cost in the 9x version (Motörhead's), fitted to its own logs: its
    /// code differs from the others' in many places.
    fn cost9(self) -> Option<i64> {
        use At::*;
        Some(match self {
            Tick => 219,
            Fade => 44,
            FxIdle => 30,
            Fx => 123,
            SamplePorta => 27,
            SampleVib => 17,
            LoopWait => 18,
            LoopDec => 13,
            LoopEnd => 0,
            FadeOut => 16,
            OnceWait => 33,
            OnceDec => 12,
            OnceEnd => 12,
            Stop => -26,
            Play => 30,
            Voices => 30,
            PortaStep => 32,
            VolFadeStep => 96,
            Position => 38,
            PositionCmd => 63,
            Track => -33,
            Wait => 57,
            Effect => 113,
            Pause => 78,
            Note => 245,
            NotePorta => 53,
            NoteFade => 46,
            Setup => 24,
            SetupEnd => 28,
            WIntreq => 16,
            WDmaOn => 10,
            WLoc => 84,
            SLoc => 79,
            WDat => 66,
            _ => return None,
        })
    }

    fn cost_v(self, nine: bool) -> i64 {
        match nine.then(|| self.cost9()).flatten() {
            Some(c) => c,
            None => self.cost(),
        }
    }

    fn cost(self) -> i64 {
        use At::*;
        match self {
            Tick => 215,
            Fade => 40,
            FadeStep => 60,
            FxIdle => 34,
            Fx => 132,
            SamplePorta => 32,
            SampleVib => 25,
            SampleVibTurn => 24,
            Slide => 24,
            SlideStep => 46,
            LoopWait => 22,
            LoopFade => 12,
            LoopDec => 20,
            LoopEnd => -6,
            FadeOut => 11,
            OnceWait => 34,
            OnceDec => 8,
            OnceEnd => 6,
            Stop => -34,
            Play => 26,
            DmaAll => 31,
            Voices => 26,
            VoiceOff => 20,
            Voice => -33,
            Porta => 58,
            PortaCalc => 300,
            PortaStep => 33,
            VolFade => 58,
            VolFadeStep => 79,
            Position => 40,
            PositionCmd => 58,
            Track => -31,
            Wait => 64,
            Effect => 96,
            Pause => 94,
            Note => 228,
            NotePorta => 63,
            NoteFade => 66,
            NotePorta2 => 80,
            Setup => 33,
            SetupEnd => 25,
            WIntena => 10,
            WIntreq => 18,
            WDmaOff => 8,
            WDmaOn => 13,
            WLoc => 78,
            SLoc => 80,
            SLen => 48,
            WVol => 111,
            WVol0 => 16,
            WStop0 => 931,
            WStop => 6,
            WVol2 => 242,
            WPer => 73,
            WDat => 54,
            TickEnd => 165,
            Irq => 16,
            IrqIntena => 0,
            IrqIntreq => 6,
            IrqDma => 16,
        }
    }
}

/// A note's extra colour clocks by voice: to its location's write, to its
/// length's (the Eagleplayer's call in place of each), and from its volume 0 to
/// its volume (by version), fitted to the Eagleplayer's write logs.
const SLOC_CH: [i64; 4] = [0, 13, 26, 38];
const VOL_CH: [i64; 4] = [0, 7, 20, 37];
const FX_CH: [i64; 4] = [0, -11, -13, -22];
const HANDLER_CH: [i64; 4] = [12, 10, 11, 10];
const FX9_CH: [i64; 4] = [-2, -2, -2, -11];
/// In the 9x version: an idle voice's effects, a voice's track, by voice (the
/// last voice's code is reached without a call), and the extra costs of the
/// portamento's and the volume fade's paths that do nothing.
const FXIDLE9_CH: [i64; 4] = [2, 1, 1, -7];
const VOICE9_CH: [i64; 4] = [1, 2, 2, -7];
const HANDLER9_CH: [i64; 4] = [9, 9, 9, 9];
const PER_CH: [i64; 4] = [0, 12, 25, 37];
const SLEN_CH: [i64; 4] = [0, 12, 24, 37];
const VOL2_CH: [i64; 4] = [0, 8, 20, 38];
const VOL2_MASTER_CH: [i64; 4] = [0, 7, 20, 37];
const VOL2_OLD_CH: [i64; 4] = [6, 20, 32, 44];

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
    /// Colour clocks the original's code has taken since its last write.
    pend: i64,
    /// The writes so far (to tell the paths that write from those that do not).
    writes: u32,
    /// The voice whose effects run, and the writes when they started (until
    /// its first write).
    fx_writes: Option<(usize, u32)>,
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
        pend: 0,
        writes: 0,
        fx_writes: None,
    };
    r.init_song(0, &mut Hw { bus });
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
        self.end_sound(hw);
        self.init_song(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.pend = 0;
        self.at(At::Tick);
        self.play(hw);
        // the Eagleplayer's interrupt: when the first voice has ended, the song
        // starts again, and that is its end
        if self.v[0].enabled == 0 {
            self.init_song(self.sub, hw);
            self.songend = true;
        }
        // the rest of the play routine and of the interrupt: audio interrupts
        // wait for its end
        self.at(At::TickEnd);
        hw.wait(self.pend.max(0) as u64);
        self.pend = 0;
    }

    fn audio_irq(&mut self, hw: &mut Hw) -> bool {
        // the Eagleplayer's handler: the requested audio interrupts' DMA off
        self.pend = 0;
        self.at(At::Irq);
        hw.wait(self.pend.max(0) as u64);
        self.pend = 0;
        // the handler's interrupt bits as they stood at the 68k's last register
        // access, as amigaplayer-rs passes them to 68k handlers (not synced to now)
        let bits = hw.bus.paula.intreq & hw.bus.paula.intena & 0x0780;
        if bits == 0 {
            // taken with an earlier one's
            return true;
        }
        self.write(hw, At::IrqIntena, 0xdff09a, bits);
        self.write(hw, At::IrqIntreq, 0xdff09c, bits);
        self.write(hw, At::IrqDma, 0xdff096, bits >> 7);
        true
    }
}

impl BenDaglish {
    fn at(&mut self, e: At) {
        self.pend += e.cost_v(self.l.nine);
    }

    fn write(&mut self, hw: &mut Hw, e: At, a: u32, v: u16) {
        // a voice's first write in its sample handler
        if let Some((ch, w)) = self.fx_writes.take()
            && w == self.writes
        {
            self.pend += if self.l.nine { HANDLER9_CH[ch] } else { HANDLER_CH[ch] };
        }
        self.writes = self.writes.wrapping_add(1);
        self.at(e);
        hw.wait(self.pend.max(0) as u64);
        self.pend = 0;
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
            let (a, v) = self.l.stop[i];
            self.write(hw, if i == 0 { At::WStop0 } else { At::WStop }, a, v);
        }
        for p in &mut self.pb {
            p.delay = 0;
        }
    }

    /// EndSound (the module's third entry): the play flag cleared, the sound
    /// stopped.
    fn end_sound(&mut self, hw: &mut Hw) {
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
        self.pend = 0;
        self.stop_sound(hw);
        self.write(hw, At::WDat, 0xdff09e, 0x00ff);
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
        self.write(hw, At::WIntreq, 0xdff09c, 0x0780);
        self.write(hw, At::WIntena, 0xdff09a, 0x0780);
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
    }

    /// The module's play routine (its second entry).
    fn play(&mut self, hw: &mut Hw) {
        self.effects(hw);
        let b = &mut *hw.bus;
        if let Some((flag, counter)) = self.l.counter
            && b.r8(self.m(flag)) != 0
        {
            let c = b.r16(self.m(counter)).wrapping_sub(1);
            b.w16(self.m(counter), c);
            if c == 0 {
                b.w16(self.m(counter), 6);
                return;
            }
        }
        self.at(At::Play);
        if b.r8(self.base + 8) == 0 {
            return;
        }
        let on = self.v.iter().fold(0, |a, v| a | v.enabled);
        b.w8(self.base + 8, on);
        if on == 0 {
            return;
        }
        self.at(At::Voices);
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
            self.write(hw, At::DmaAll, 0xdff096, self.dma_all);
        }
    }

    fn master_fade(&mut self, f: &Fade, hw: &mut Hw) {
        self.at(At::Fade);
        let b = &mut *hw.bus;
        let base = self.base;
        let speed = if f.bytes { b.r8(f.speed.at(base)) as u16 } else { b.r16(f.speed.at(base)) };
        if speed == 0 {
            return;
        }
        let c = f.counter.at(base);
        if f.bytes {
            let n = b.r8(c).wrapping_sub(1);
            b.w8(c, n);
            if (n as i8) >= 0 {
                return;
            }
            b.w8(c, (speed as u8).wrapping_sub(1));
        } else {
            let n = b.r16(c).wrapping_sub(1);
            b.w16(c, n);
            if n != 0 {
                return;
            }
            b.w16(c, speed);
        }
        self.at(At::FadeStep);
        let vol = f.volume.at(base);
        let n = b.r16(vol).wrapping_sub(1);
        b.w16(vol, n);
        if (n as i16) >= 0 {
            return;
        }
        for &(a, word) in &f.clears {
            if word {
                b.w16(a.at(base), 0);
            } else {
                b.w8(a.at(base), 0);
            }
        }
        self.stop_sound(hw);
    }

    fn voice_effects(&mut self, ch: usize, hw: &mut Hw) {
        if self.pb[ch].delay == 0 {
            self.at(At::FxIdle);
            self.pend += if self.l.nine { FXIDLE9_CH[ch] } else { 0 };
            return;
        }
        self.at(At::Fx);
        let fx = if self.l.nine { FX9_CH[ch] } else { FX_CH[ch] };
        self.pend += fx;
        self.fx_writes = Some((ch, self.writes));
        let p = &mut self.pb[ch];
        if (p.ticks as i16) > 0 {
            p.ticks -= 1;
            self.pend += 7;
        }
        self.sample_effects(ch, hw);
        if self.l.final_slide {
            self.at(At::Slide);
            let p = &mut self.pb[ch];
            if p.slide_speed != 0 {
                p.slide_count = p.slide_count.wrapping_sub(1);
                if p.slide_count == 0 {
                    p.slide_count = p.slide_speed;
                    p.volume = p.volume.wrapping_add(p.slide_add);
                    self.at(At::SlideStep);
                }
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
            self.at(At::Stop);
            if self.fx_writes.take().is_some_and(|(_, w)| w == self.writes) {
                self.pend += -fx;
            }
            self.write(hw, At::WDat, Self::reg(ch, 0x0a), 0);
            self.write(hw, At::WDmaOff, 0xdff096, p.bit);
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
            if self.l.clamp == Clamp::Master {
                let abs = matches!(self.l.fade, Some(Fade { volume: Addr::Abs(_), .. }));
                self.pend += if abs { 8 } else { 6 };
            }
        }
        if self.l.dma != Dma::Global {
            self.pend += VOL_CH[ch];
            self.fx_writes = None;
            self.write(hw, At::WVol, Self::reg(ch, 8), vol);
        }
        let per = p.period.wrapping_add(p.period_add).wrapping_add(p.porta);
        self.pend += PER_CH[ch];
        self.write(hw, At::WPer, Self::reg(ch, 6), per);
        match self.l.dma {
            Dma::Tick => self.write(hw, At::WDmaOn, 0xdff096, 0x8000 | p.bit),
            Dma::Global => self.dma_all |= p.bit,
            Dma::Handlers => {}
        }
    }

    /// The sample's portamento, then its vibrato.
    fn sample_effects(&mut self, ch: usize, hw: &mut Hw) {
        let p = &mut self.pb[ch];
        if p.sporta != 0 {
            if p.sporta == 0xffff {
                return;
            }
            self.pend += At::SamplePorta.cost_v(self.l.nine);
            p.period_add = p.period_add.wrapping_add(p.sporta_add);
            p.sporta -= 1;
            if p.sporta != 0 {
                return;
            }
        }
        self.pend += At::SampleVib.cost_v(self.l.nine);
        p.period_add = p.period_add.wrapping_add(p.vib_add);
        p.vib_depth = p.vib_depth.wrapping_sub(1);
        if p.vib_depth != 0 {
            return;
        }
        let depth = hw.bus.r16(p.sample + 0x14);
        if depth != 0 {
            self.pend += At::SampleVibTurn.cost_v(self.l.nine);
            p.vib_depth = depth;
            p.vib_add = p.vib_add.wrapping_neg();
        }
    }

    fn handle_loop(&mut self, ch: usize, hw: &mut Hw) {
        let p = self.pb[ch];
        if (p.delay as i16) < 0 {
            self.at(At::LoopWait);
            let ticks = p.ticks as i16;
            let next = if self.l.old { ticks != 1 } else { ticks > 1 };
            if !next {
                self.at(At::LoopFade);
                self.pb[ch].handler = Handler::Fade;
            }
            return;
        }
        if self.l.dma == Dma::Handlers {
            self.write(hw, At::WDmaOn, 0xdff096, 0x8000 | p.bit);
        }
        self.at(At::LoopDec);
        let d = p.delay - 1;
        self.pb[ch].delay = d;
        if d != 0 {
            return;
        }
        self.pb[ch].delay = !d;
        self.at(At::LoopEnd);
        let s = p.sample;
        let a = self.m(self.l.samples).wrapping_add(hw.bus.r32(s + 4));
        let len = hw.bus.r16(s + 0x0a);
        self.pend += SLOC_CH[ch];
        self.write(hw, At::WLoc, Self::reg(ch, 0), (a >> 16) as u16);
        hw.bus.write_word(Self::reg(ch, 2), a as u16);
        self.pend += SLEN_CH[ch];
        self.write(hw, At::SLen, Self::reg(ch, 4), len);
    }

    fn handle_fade(&mut self, ch: usize, hw: &mut Hw) {
        self.at(At::FadeOut);
        let p = &mut self.pb[ch];
        p.delay = 0x8000;
        p.volume = p.volume.wrapping_add(hw.bus.r16(p.sample + 0x0e));
        if self.l.old && (p.volume as i16) < 0 {
            p.volume = 0;
        }
    }

    fn handle_once(&mut self, ch: usize, hw: &mut Hw) {
        let p = self.pb[ch];
        if (p.delay as i16) < 0 {
            self.at(At::OnceWait);
            if p.ticks == 1 {
                self.pb[ch].delay = 0x8000;
            }
            // the interrupt has turned the DMA off: the sample has played
            if p.bit & hw.bus.paula.dmacon == 0 {
                self.at(At::OnceEnd);
                self.pb[ch].volume = 0;
            }
            return;
        }
        self.at(At::OnceDec);
        let d = p.delay - 1;
        self.pb[ch].delay = d;
        if d == 0 {
            self.pb[ch].delay = !d;
            self.write(hw, At::WIntreq, 0xdff09c, p.int);
            self.write(hw, At::WIntena, 0xdff09a, 0x8000 | p.int);
        } else if self.l.dma == Dma::Handlers {
            self.write(hw, At::WDmaOn, 0xdff096, 0x8000 | p.bit);
        } else if self.l.dma == Dma::Global {
            self.write(hw, At::WIntreq, 0xdff09c, p.int);
        }
    }

    /// A voice's track: its portamento and volume fade, its position list and
    /// track when it is time, and the note.
    fn voice(&mut self, ch: usize, hw: &mut Hw) {
        if self.v[ch].enabled == 0 {
            self.at(At::VoiceOff);
            return;
        }
        self.at(At::Voice);
        self.pend += if self.l.nine { VOICE9_CH[ch] } else { 0 };
        if !self.l.old {
            self.portamento(ch);
            self.volume_fade(ch, hw);
        }
        let mut a3;
        'position: loop {
            if self.v[ch].next_position != 0 {
                self.at(At::Position);
                let v = &mut self.v[ch];
                v.loops = v.loops.wrapping_sub(1);
                if v.loops != 0 {
                    v.track = v.track_start;
                    v.next_position = 0;
                    a3 = v.track;
                } else {
                    v.loops = 1;
                    match self.positions(ch, hw) {
                        Some(t) => a3 = t,
                        None => return,
                    }
                }
            } else {
                a3 = self.v[ch].track;
            }
            let v = &mut self.v[ch];
            v.wait = v.wait.wrapping_sub(1);
            if v.wait != 0 {
                self.at(At::Wait);
                if hw.bus.r8(a3) & 0x80 != 0 {
                }
                if hw.bus.r8(a3) & 0x80 != 0 {
                    self.effect(ch, &mut a3, hw);
                }
                self.v[ch].track = a3;
                return;
            }
            loop {
                if hw.bus.r8(a3) & 0x80 == 0 {
                    break;
                }
                self.effect(ch, &mut a3, hw);
                if self.v[ch].next_position != 0 {
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
            self.at(At::PositionCmd);
            let b = &mut *hw.bus;
            let cmd = b.r8(a3);
            a3 = a3.wrapping_add(1);
            let track = if self.l.f0 { cmd < 0xc8 } else { cmd < 0x80 };
            if track {
                self.at(At::Track);
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
                    self.v[ch].transpose = b.r8(a3);
                    a3 = a3.wrapping_add(1);
                }
                0xff => {
                    let d = self.pb[ch].delay;
                    if d == 0 || d == 0x8000 {
                        self.v[ch].enabled = 0;
                    }
                    return None;
                }
                0xfd if self.l.f0 && self.l.fade.is_some() => {
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
                    } else if self.l.f0 {
                        v.loops = cmd.wrapping_sub(0xc8);
                    } else {
                        v.loops = cmd & 0x1f;
                    }
                }
            }
        }
    }

    /// A track effect (a byte with bit 7 set) at `a3`.
    fn effect(&mut self, ch: usize, a3: &mut u32, hw: &mut Hw) {
        self.at(At::Effect);
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
            return;
        }
        if cmd == 0xff {
            self.v[ch].next_position = 0xff;
            return;
        }
        if (cmd as i8) < (first as i8) {
            // the "control" value, which the player keeps but does not use
            return;
        }
        let v = &mut self.v[ch];
        if self.l.old {
            if cmd != 0xc2 {
                *a3 = a3.wrapping_add(3);
            }
            return;
        }
        match cmd - first {
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
            self.at(At::Pause);
            self.v[ch].wait = b.r8(a3);
            self.v[ch].track = a3.wrapping_add(1);
            return;
        }
        self.at(At::Note);
        if !self.l.old {
            self.pb[ch].porta = 0;
        }
        let mut d0 = note.wrapping_add(self.v[ch].transpose);
        self.v[ch].note = d0;
        if !self.l.old {
            let v = self.v[ch];
            self.v[ch].porta_on = v.porta1;
            if v.porta1 != 0 {
                self.at(At::NotePorta);
                let step = self.period_long(hw, (v.porta_delta as i8 as i16).wrapping_neg().wrapping_mul(4));
                let v = &mut self.v[ch];
                v.porta_delay_count = v.porta_delay;
                v.porta_dur_count = v.porta_dur;
                v.porta_step = step;
            }
            let v = &mut self.v[ch];
            v.fading = v.fade_on;
            if v.fade_on != 0 {
                self.pend += At::NoteFade.cost_v(self.l.nine);
                v.fade_speed2 = v.fade_speed;
                v.fade_count = v.fade_speed;
                v.fade_dur_count = v.fade_dur;
                v.fade_step = v.fade_add;
                v.fade_value = 0;
            }
        }
        let b = &mut *hw.bus;
        let t = b.r8(a3);
        a3 = a3.wrapping_add(1);
        let ticks = if self.l.extra_tick && t == 0 {
            self.v[ch].wait = b.r8(a3);
            a3 = a3.wrapping_add(1);
            0xffff
        } else {
            self.v[ch].wait = t;
            t as u16
        };
        if !self.l.old {
            if self.v[ch].porta2 != 0 {
                self.at(At::NotePorta2);
                let v = &mut self.v[ch];
                v.porta_on = 0xff;
                v.porta_delay_count = 0;
                v.porta_dur_count = v.porta_dur;
                let d2 = d0;
                if v.new_note == 0 {
                    d0 = v.prev_note;
                }
                v.note = d0;
                let i = (d2.wrapping_sub(d0) as i8 as i16).wrapping_neg().wrapping_mul(4);
                self.v[ch].porta_step = self.period_long(hw, i);
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
        self.at(At::Porta);
        let v = &mut self.v[ch];
        if v.porta_on == 0 {
            return;
        }
        self.pend += if self.l.nine { 15 } else { 0 };
        if v.porta_on & 0x80 != 0 {
            self.pend += At::PortaCalc.cost_v(self.l.nine);
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
        }
        if v.porta_delay_count != 0 {
            v.porta_delay_count -= 1;
            self.pend += if self.l.nine { 8 } else { 0 };
            return;
        }
        if v.porta_dur_count == 0 {
            return;
        }
        self.pend += At::PortaStep.cost_v(self.l.nine);
        v.porta_dur_count -= 1;
        let p = &mut self.pb[ch];
        p.porta = p.porta.wrapping_add((v.porta_step >> 16) as u16);
    }

    /// The track's volume fade: a note again at each step.
    fn volume_fade(&mut self, ch: usize, hw: &mut Hw) {
        self.at(At::VolFade);
        let v = &mut self.v[ch];
        if v.fading == 0 || v.fade_dur_count == 0 {
            if v.fading != 0 {
                self.pend += if self.l.nine { 10 } else { 0 };
            }
            return;
        }
        v.fade_count = v.fade_count.wrapping_sub(1);
        if v.fade_count != 0 {
            self.pend += if self.l.nine { 23 } else { 0 };
            return;
        }
        self.pend += At::VolFadeStep.cost_v(self.l.nine);
        v.fade_dur_count -= 1;
        v.fade_count = v.fade_speed2;
        let d2 = v.fade_value.wrapping_add(v.fade_step);
        v.fade_value = d2;
        let s = v.sample2;
        let d2 = d2.wrapping_add(hw.bus.r16(s + 0x0c));
        if (d2 as i16) < 0 {
            self.v[ch].fade_dur_count = 0;
            return;
        }
        if (d2 as i16) > 0x40 {
        }
        let d2 = if (d2 as i16) > 0x40 { 0x40 } else { d2 };
        let (note, ticks) = (v.note, v.fade_speed2 as u16);
        self.setup(ch, s, note, ticks, d2, hw);
    }

    /// The original's SetupSample: a note of sample `s` on voice `ch`.
    fn setup(&mut self, ch: usize, s: u32, note: u8, ticks: u16, vol: u16, hw: &mut Hw) {
        self.at(At::Setup);
        let saved = hw.bus.paula.intena;
        if self.l.intena_save {
            self.write(hw, At::WIntena, 0xdff09a, 0x7fff);
        }
        let p = self.pb[ch];
        self.write(hw, At::WIntena, 0xdff09a, p.int);
        self.write(hw, At::WDmaOff, 0xdff096, p.bit);
        let b = &mut *hw.bus;
        let a = self.m(self.l.samples).wrapping_add(b.r32(s));
        let len = b.r16(s + 8);
        self.pend += SLOC_CH[ch];
        self.write(hw, At::SLoc, Self::reg(ch, 0), (a >> 16) as u16);
        hw.bus.write_word(Self::reg(ch, 2), a as u16);
        self.pend += SLEN_CH[ch];
        self.write(hw, At::SLen, Self::reg(ch, 4), len);
        self.write(hw, At::WVol0, Self::reg(ch, 8), 0);
        let b = &mut *hw.bus;
        // the period: the table's long at the note (from the sample's transpose)
        // times the sample's fine tune, as words
        let i = (b.r16(s + 0x18).wrapping_sub((note & 0x7f) as u16)).wrapping_mul(4) as i16;
        let t = self.m(self.l.periods).wrapping_add(i as u32);
        let ft = b.r16(s + 0x1a) as u32;
        let period = (((b.r16(t + 2) as u32 * ft) >> 16) as u16).wrapping_add((b.r16(t) as u32 * ft) as u16);
        let sporta = b.r16(s + 0x10);
        let pm = &mut self.pb[ch];
        pm.sample = s;
        pm.ticks = ticks;
        pm.period = period;
        pm.sporta = sporta;
        if (sporta as i16) >= 0 {
            let depth = b.r16(s + 0x14);
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
        // from the volume 0 to the note's volume: the period, the vibrato's
        // setup, the handler, the clamp (`At::WVol2` is the fastest path)
        let mut x = if self.l.old { VOL2_OLD_CH[ch] } else if self.l.clamp == Clamp::Master { VOL2_MASTER_CH[ch] } else { VOL2_CH[ch] };
        if (sporta as i16) >= 0 {
            x += 113 + (b.r16(s + 0x14) & 1) as i64;
        }
        if pm.handler == Handler::Loop {
            x -= 4;
        }
        if self.l.setup_clr {
            x += 8;
        }
        let mut vol = vol;
        if self.l.clamp == Clamp::Master {
            // the master volume's compare (and move), slower from an absolute address
            let abs = matches!(self.l.fade, Some(Fade { volume: Addr::Abs(_), .. }));
            x += 11 + if abs { 2 } else { 0 };
            let m = self.master(hw);
            if (vol as i16) > (m as i16) {
                vol = m;
                x += 6 + if abs { 2 } else { 0 };
            }
        }
        self.pend += x;
        self.pb[ch].volume = vol;
        self.write(hw, At::WVol2, Self::reg(ch, 8), vol);
        let pm = &mut self.pb[ch];
        if self.l.final_slide {
            pm.slide_speed = self.d3;
            pm.slide_count = self.d3;
            pm.slide_add = if self.l.d4_patched { 0xf0a0 + 0x10 * ch as u16 } else { self.d4 };
        }
        pm.delay = 2;
        self.at(At::SetupEnd);
        if self.l.intena_save {
            self.write(hw, At::WIntena, 0xdff09a, saved | 0x8000);
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
