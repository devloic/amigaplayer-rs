//! Ron Klaren, ported from NostalgicPlayer's Ron Klaren player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! A Ron Klaren module is an executable: a game's replayer with its music. The
//! original player, the RonKlaren Eagleplayer (V1.13), LoadSegs it, patches its
//! writes to the audio registers into calls of ENPP_PokeAdr/Len/Per/Vol and
//! ENPP_DMAMask, keeps a copy of the module that each InitSound copies back,
//! calls the replayer's subsong routine and then, every tick, its interrupt
//! routine. This port does the same with the module LoadSegged into guest memory:
//! it works on the replayer's own variables there (its voices, globals and the
//! instruments' phase bytes), and finds them, the period, arpeggio and
//! instrument tables and the voice routine through the replayer's pc-relative
//! references, as NostalgicPlayer finds its tables.
//!
//! The replayers differ from game to game; the two kinds here are the older one
//! (Battle Squadron, Synsations), and the newer one with a voice mute, a pause,
//! a backup of the voices that song end ($83) restores, so that the song starts
//! again, and an instrument and portamento that reset the envelope. Some
//! newer replayers also clear a voice's volume when it starts; an older one has
//! a second stop command ($85). Each is read from the code.
//!
//! Where NostalgicPlayer differs from the original's writes, this follows the
//! original:
//! * a voice's DMA comes on at the start of its routine every tick, through
//!   ENPP_DMAMask, whose wait for the DMA (10 lines) the host does each time; a
//!   note turns it off and writes its sample's start and length, and a one-shot
//!   sample's length 1 comes on the next tick;
//! * the period of a note is read with the high byte of the replayer's d0, as
//!   it is: after a wait-0 note, an instrument or a new track it can index far
//!   from the table, and nothing is range-checked: the table is read on in the
//!   module (and the arpeggio's and vibrato's tables too, the vibrato one word
//!   past its end where it starts again);
//! * the envelope's comparisons are the replayer's byte comparisons (its sign
//!   flag), and a step that reaches its point exactly goes on to the next only
//!   when it comes from above;
//! * song end ($83) stops the older replayers for good and makes the newer
//!   ones restore their voices as they were before the subsong started (from the
//!   module's own saved state); the song end the Eagleplayer reports is a voice's
//!   track list starting again;
//! * the game's routines (sound effects, fades, pause, mute) are not called by
//!   the Eagleplayer, so their flags stay as the module has them.
//!
//! The writes come at the original's times as the emulator runs it, whose clock
//! is half the cycles counted since its last call of the host (each patched
//! write, the DMA wait): the Eagleplayer's start (the time to its first write
//! fitted to its logs from the module's header size, its subsong range and the
//! kind of replayer, within 250 colour clocks; the copies of the module,
//! DTP_SubSongRange, InitSound), the interrupt's cycles to its first voice
//! (counted on the code), and between writes the costs of the code points the
//! replayer passes (`At`), fitted to the reference's write log for the older
//! and newer replayers and for a newer one whose instrument sample the
//! Eagleplayer patches as two calls. No code or data of the Eagleplayer is
//! kept: the timer's value and the interrupt routine are found as
//! NostalgicPlayer finds them (`timer_and_irq`), and the two forms of the
//! replayer's code whose writes the Eagleplayer patches in another way (an
//! InitVoice that writes directly, an instrument command split in two) are told
//! by the replayer's own instructions (`code_forms`).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Ron Klaren", detect, new };

/// The module's code hunk, as a byte slice that reads zeros outside it.
#[derive(Clone, Copy)]
struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn b(&self, o: usize) -> u8 {
        self.0.get(o).copied().unwrap_or(0)
    }
    fn w(&self, o: usize) -> u16 {
        (self.b(o) as u16) << 8 | self.b(o + 1) as u16
    }
    fn l(&self, o: usize) -> u32 {
        (self.w(o) as u32) << 16 | self.w(o + 2) as u32
    }
    /// The target of a pc-relative displacement at `o`.
    fn rel(&self, o: usize) -> usize {
        (o as i64 + self.w(o) as i16 as i64) as usize
    }
    /// Whether the words at `o` match `pat` (`ANY` matches any word).
    fn at(&self, o: usize, pat: &[u32]) -> bool {
        pat.iter().enumerate().all(|(i, &p)| p == ANY || self.w(o + 2 * i) as u32 == p)
    }
    fn find(&self, from: usize, pat: &[u32]) -> Option<usize> {
        (from..self.0.len().saturating_sub(2 * pat.len())).step_by(2).find(|&o| self.at(o, pat))
    }
}

const ANY: u32 = 0x1_0000;

/// Where the replayer keeps its code and data (offsets in the code hunk), and
/// which variant it is.
#[derive(Clone, Debug)]
struct Layout {
    /// The interrupt's cycles from its start to the first voice.
    prologue: u64,
    globals: usize,
    periods: usize,
    arpeggios: usize,
    instruments: usize,
    voices: [usize; 4],
    /// The newer replayer: its subsong word at globals+8, the table at +12,
    /// the voice backup (and its restore flag at +6), mute ($3d), pause (+4).
    newer: bool,
    backup: usize,
    subsong_table: usize,
    /// An instrument clears the volume ($39); a portamento the envelope
    /// state ($28); InitVoice writes volume 0; command $85 stops.
    clear_volume: bool,
    clear_adsr_porta: bool,
    init_volume: bool,
    stop85: bool,
    /// The subsong routine the Eagleplayer calls, and how many it counts.
    subsongs: u16,
    /// The interrupt routine the Eagleplayer finds.
    irq: usize,
    /// CIA-B timer A's value (the module's own tick rate).
    timer: u16,
    /// Timing variant (the row of `COSTS`), and the writes the Eagleplayer's
    /// patterns miss: InitVoice's, and an instrument's sample after its DMA off.
    timing: Timing,
    init_direct: bool,
    instr_split: bool,
    /// The Eagleplayer's time from loading the module to starting its timer.
    init_wait: u64,
    hdr: u32,
    /// DTP_SubSongRange's time (cycles).
    ssr_cycles: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Timing {
    Older,
    Newer,
    /// The newer replayer with its instrument command's writes split.
    Split,
}

/// Finds the replayer's parts as NostalgicPlayer and the Eagleplayer find them.
fn layout(d: &[u8]) -> Option<Layout> {
    // NostalgicPlayer's identification, the Eagleplayer's check (moveq #-1,d0;
    // rts, then the name)
    if d.len() < 0xa40 || d.get(..4)? != [0, 0, 3, 0xf3] || d.get(40..63)? != b"RON_KLAREN_SOUNDMODULE!" {
        return None;
    }
    if d.get(32..36)? != [0x70, 0xff, 0x4e, 0x75] {
        return None;
    }
    let hf = crate::hunk::parse(d).ok()?;
    let code = &hf.hunks.first()?.data;
    let r = Rd(code);

    // the voice routine: movea.l 4(a0),a1; movea.l $10(a0),a2; movea.l 0(a0),a3
    let voice_routine = r.find(0, &[0x2268, 4, 0x2468, 0x10, 0x2668, 0])?;
    let newer = r.at(voice_routine + 12, &[0x4a28, 0x3d]);
    // lea G(pc),a1; add.b d2,d3; cmp.b 1(a1),d3 (the envelope's peak)
    let globals = r.rel(r.find(0, &[0x43fa, ANY, 0xd602, 0xb629, 1])? + 2);
    // lea periods(pc),a4; move.w (a4,d0.w),$24(a0)
    let periods = r.rel(r.find(0, &[0x49fa, ANY, 0x3174, 0, 0x24])? + 2);
    // lea arpeggios(pc),a4; adda.l d0,a4
    let arpeggios = r.rel(r.find(0, &[0x49fa, ANY, 0xd9c0])? + 2);
    // lsl.w #5,d0; lea instruments(pc),a4; lea (a4,d0.w),a1
    let instruments = r.rel(r.find(0, &[0xeb48, 0x49fa, ANY, 0x43f4, 0])? + 4);
    // the interrupt's lea voice(pc),a0; bsr.w voice_routine, four times
    let mut voices = [0; 4];
    let mut first = None;
    let mut o = 0;
    for v in voices.iter_mut() {
        o = (o..code.len().saturating_sub(8)).step_by(2).find(|&k| {
            r.at(k, &[0x41fa, ANY, 0x6100]) && r.rel(k + 6) == voice_routine
        })?;
        *v = r.rel(o + 2);
        first.get_or_insert(o);
        o += 8;
    }
    // move.w sub(a0),d0; andi.w #$f,d0; subq.w #1,d0; lsl.w #4,d0; lea tab(a0,d0.w),a3
    let s = r.find(0, &[0x3028, ANY, 0x0240, 0xf, 0x5340, 0xe948, 0x47f0, ANY])?;
    let subsong_table = globals + r.w(s + 14) as usize;
    if r.w(s + 2) != if newer { 8 } else { 4 } {
        return None;
    }
    // lea voice0(pc),a0; lea backup(pc),a1; move.w #$3f,d0; move.l (a0)+,(a1)+; dbra
    let backup = if newer {
        let b = r.find(0, &[0x41fa, ANY, 0x43fa, ANY, 0x303c, 0x3f, 0x22d8, 0x51c8, 0xfffc])?;
        if r.rel(b + 2) != voices[0] {
            return None;
        }
        r.rel(b + 6)
    } else {
        0
    };
    // cmpi.b #$82,(a2); bne; clr.w d0; clr.b $39(a0)
    let clear_volume = r.find(0, &[0x0c12, 0x82, ANY, 0x4240, 0x4228, 0x39]).is_some();
    // NostalgicPlayer's test: cmpi.b #$81,(a2); bne; clr.w d0; clr.w ...
    let clear_adsr_porta = r.find(0, &[0x0c12, 0x81, ANY, 0x4240, 0x4268]).is_some();
    // move.w 4(a1),4(a2); clr.w 8(a2)
    let init_volume = r.find(0, &[0x3569, 4, 4, 0x426a, 8]).is_some();
    let stop85 = r.find(0, &[0x0c12, 0x85]).is_some();
    let (timer, irq) = timer_and_irq(r)?;
    let prologue = cycles_to(code, irq, first?)?;
    let (subsongs, ssr_cycles) = ep_subsongs(r)?;
    let mut l = Layout {
        prologue,
        globals,
        periods,
        arpeggios,
        instruments,
        voices,
        newer,
        backup,
        subsong_table,
        clear_volume,
        clear_adsr_porta,
        init_volume,
        stop85,
        subsongs,
        irq,
        timer: if timer == 0 { crate::deli::DEFAULT_TIMER } else { timer },
        timing: Timing::Older,
        init_direct: false,
        instr_split: false,
        init_wait: 0,
        hdr: r.l(4),
        ssr_cycles,
    };
    l.start_up(r);
    Some(l)
}

/// How the replayer writes the voices' registers, which tells how the
/// Eagleplayer's patching leaves its code: the instrument's sample start after
/// a DMA off is written through `0(a1)` in two places or in fewer (the
/// instrument command's writes then go out as two calls), and InitVoice copies
/// the sample's start and length, the next pointer after them, in one form
/// (otherwise its writes are the replayer's own, not patched).
fn code_forms(r: Rd) -> (bool, bool) {
    // move.w d(a0),DMACON; movea.l 0(a1),a4
    let starts = (0..r.0.len()).step_by(2).filter(|&o| r.at(o, &[0x33e8, ANY, 0x00df, 0xf096, 0x2869, 0])).count();
    // move.l 0(a1),0(a2); move.w 4(a1),4(a2); move.l 8(a0),$c(a0)
    let direct = match r.find(0, &[0x3569, 4, 4]) {
        Some(o) => !(o >= 6 && r.at(o - 6, &[0x2569, 0, 0]) && r.at(o + 6, &[0x2168, 8, 0xc])),
        None => true,
    };
    (starts < 2, direct)
}

impl Layout {
    /// What the Eagleplayer's patching leaves of the code, and its time to the
    /// timer's start: InitPlayer (its scan of the code for the replayer's
    /// writes, the copy of the module), DTP_SubSongRange, InitSound (the copy
    /// back, the subsong routine). The scan's time is not counted on the code:
    /// it is about 6.54 million cycles in all of the modules, within 0.1%, and
    /// the time to the first write is fitted to the Eagleplayer's logs by the
    /// header's size (the copies, 29 cycles a byte each way, taken as one
    /// colour clock for 2 cycles), the subsong range's cycles, and a constant
    /// for each kind of replayer (the median of the corpus' modules: older,
    /// newer, newer with the instrument command split); it is within 250
    /// colour clocks of the reference's for all of them.
    fn start_up(&mut self, r: Rd) {
        (self.instr_split, self.init_direct) = code_forms(r);
        self.timing = match (self.newer, self.instr_split) {
            (false, _) => Timing::Older,
            (true, false) => Timing::Newer,
            (true, true) => Timing::Split,
        };
        let base = match self.timing {
            Timing::Older => 3_275_442,
            Timing::Newer => 3_278_459,
            Timing::Split => 3_276_983,
        };
        self.init_wait = base + 29 * self.hdr as u64 + self.ssr_cycles / 2;
    }
}

/// The timer's value and the interrupt routine, found as NostalgicPlayer finds
/// them: one of the first two jumps after the module's header goes to the
/// timer set-up (it starts with a `bsr.w` or a `move.w #n,abs`), whose code to
/// its `rts` writes CIA-B timer A's low and high bytes (`move.b #n,$bfd400`
/// and `$bfd500`) and the interrupt's address into the level 6 vector
/// (`move.l #addr,$78`). A module whose jumps go elsewhere has the set-up
/// somewhere in its code: the first `move.l #addr,$78`, with the two timer
/// writes just before it.
fn timer_and_irq(r: Rd) -> Option<(u16, usize)> {
    let init = (0..2).find_map(|i| {
        let j = 32 + 6 * i;
        let a = r.l(j + 2) as usize;
        (r.w(j) == 0x4ef9 && matches!(r.w(a), 0x6100 | 0x33fc)).then_some(a)
    });
    let (mut lo, mut hi, mut irq) = (0u8, 0u8, 0usize);
    let (from, to) = match init {
        Some(a) => (a, None),
        None => {
            let v = (0..r.0.len()).step_by(2).find(|&o| r.at(o, &[0x23fc, ANY, ANY, 0, 0x78]))?;
            (v.saturating_sub(0x60), Some(v + 10))
        }
    };
    let mut o = from;
    while o + 10 < r.0.len() && to.map_or(r.w(o) != 0x4e75, |t| o < t) {
        match r.w(o) {
            0x13fc => {
                match r.l(o + 4) {
                    0xbfd400 => lo = r.b(o + 3),
                    0xbfd500 => hi = r.b(o + 3),
                    _ => {}
                }
                o += 6;
            }
            0x23fc => {
                if r.l(o + 6) == 0x78 {
                    irq = r.l(o + 2) as usize;
                }
                o += 8;
            }
            _ => {}
        }
        o += 2;
    }
    (irq != 0 && irq < r.0.len()).then_some((u16::from(hi) << 8 | u16::from(lo), irq))
}

/// The fewest cycles (as the emulator counts them) the code takes from `from`
/// to `to`, through its branches and subroutine calls: the interrupt's path to
/// its first voice when it plays.
fn cycles_to(code: &[u8], from: usize, to: usize) -> Option<u64> {
    use m68k_rc::Op;
    use std::cmp::Reverse;
    let r = Rd(code);
    let mut best = std::collections::HashMap::new();
    let mut q = std::collections::BinaryHeap::from([Reverse((0u64, from as u32))]);
    while let Some(Reverse((t, pc))) = q.pop() {
        if pc as usize == to {
            return Some(t);
        }
        if best.len() > 4096 || pc as usize >= code.len() || best.get(&pc).is_some_and(|&b| b <= t) {
            continue;
        }
        best.insert(pc, t);
        let ins = m68k_rc::decode::decode_model(pc, &mut |a| r.w(a as usize), m68k_rc::Model::M68020);
        let t = t + m68k_rc::cycles::cycles(&ins) as u64;
        let next = pc + ins.len;
        let to: &[u32] = match ins.op {
            Op::Bra { target } | Op::Bsr { target } => &[target],
            Op::Bcc { target, .. } | Op::Dbcc { target, .. } => &[next, target],
            Op::Rts | Op::Rte | Op::Rtr | Op::Jmp { .. } | Op::Jsr { .. } | Op::Illegal => &[],
            _ => &[next],
        };
        q.extend(to.iter().map(|&n| Reverse((t, n))));
    }
    None
}

/// The Eagleplayer's DTP_SubSongRange: past the jumps after the module's name,
/// the `move.w #n,d0` entries, counted 8 bytes apart, and its InitSound calls
/// the first one's routine.
fn ep_subsongs(r: Rd) -> Option<(u16, u64)> {
    let mut a = 0x1c;
    let mut jumps = 0u16;
    loop {
        a += 4;
        let w = r.w(a);
        a += 2;
        if w != 0x4ef9 {
            break;
        }
        jumps += 1;
    }
    let bra = match r.w(a + 2) {
        0x4ef9 => false,
        0x6000 => true,
        _ => return None,
    };
    a -= 8;
    let mut n = 0u16;
    loop {
        n += 1;
        a += 6;
        let w = r.w(a);
        a += 2;
        if w != 0x303c {
            break;
        }
    }
    // the loops' cycles, and the entry's address read from a jmp or a bra
    let cycles = 24 + 29 * (jumps as u64 + 1) + 41 + if bra { 57 } else { 0 } + 32 + 37 * n as u64 + 28;
    Some((n - 1, cycles))
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// Code points of the original where it spends the time between its writes:
/// each costs the cycles it takes (`COSTS`), and a write waits half the
/// cycles run since the call of the host before it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(usize)]
enum At {
    /// From the timer to the first voice (the Eagleplayer's interrupt, the
    /// replayer's interrupt and its tests).
    Tick,
    /// A voice's routine up to its DMA on, and after the DMA wait.
    Voice,
    AfterDma,
    /// The loop's length 1, and after it.
    Len1,
    /// Waiting: the effects up to the period.
    Fx,
    Phase,
    PhaseStep,
    PhaseTurn,
    Porta,
    PortaStep,
    Arp,
    ArpWrap,
    /// Vibrato: none, its delay, a step, its wrap.
    VibNone,
    VibDelay,
    Vib,
    VibWrap,
    /// The envelope: not this tick, a step, the next state.
    AdsrWait,
    Adsr,
    AdsrAttack,
    AdsrNext,
    /// A row: the start, each command, a track's repeat, the next track, a
    /// note that only sets the period, a note.
    Parse,
    ParseVibDelay,
    Cmd80,
    Cmd81,
    Cmd82,
    Cmd84,
    Checks,
    Repeat,
    NextTrack,
    Legato,
    Note,
    NoteOnce,
    /// After the voice's last write, to the next voice.
    VoiceEnd,
    /// A new subsong: the voices' backup and DMA off, a voice's start.
    NewSong,
    InitVoice,
    InitVoiceLen,
    /// Song end's restore of the voices.
    Restore,
    /// Each kind of write, from the code before it.
    WDmaOn,
    WDmaOff,
    WLoc,
    WLen,
    WPer,
    WVol,
    /// A write the Eagleplayer does not patch: the replayer's own `move`.
    Direct,
    /// The phasing's step backwards, and its turn there.
    PhaseBack,
    PhaseBackTurn,
    /// The envelope's attack at its peak, and a next state past the last.
    AdsrPeak,
    AdsrLast,
    /// An instrument's sample the Eagleplayer patches as two calls.
    WLocSplit,
    /// The voice routine's end after a row, from the one after the effects.
    RowEnd,
    /// The Eagleplayer's routine after a write (to its return into the
    /// replayer), by the write: the portamento's, arpeggio's, vibrato's and a
    /// row's period, the envelope's volume.
    TailPorta,
    TailArp,
    TailVib,
    TailRow,
    TailVol,
    /// The envelope's attack stopping at its point (not at the peak).
    AdsrToPoint,
    /// The portamento reaching its period going up or down.
    PortaClampUp,
    PortaClampDown,
    Count,
}

const N_AT: usize = At::Count as usize;

/// The costs in cycles, by timing variant (fitted to the reference's write times).
const COSTS: [[i64; N_AT]; 3] = [
    // older
    [
        114, 95, 85, 77, 93, 48, 174, 42, 62, 62, 61, 8, -6, 41, 195, 44, 10, 155, 28, 35, 16, 20, 176, 104, 153, 32,
        186, 55, 200, 35, 81, 16, 96, -136, 496, 110, 0, 96, 234, 223, 198, 136, 155, 6, -20, 32, 6, -14, 222, 32, 115,
        117, 128, 32, 55, 11, 4, 14,
    ],
    // newer
    [
        123, 101, 84, 76, 107, 48, 174, 42, 72, 72, 71, 8, 1, 48, 196, 44, 12, 166, 29, 35, 70, 26, 176, 124, 176, 28,
        191, 41, 187, 35, 77, 16, 102, 1837, 528, 110, 0, 102, 254, 222, 198, 153, 166, 10, -20, 32, 5, -14, 222, 36,
        122, 123, 129, 35, 55, 10, 1, 11,
    ],
    // newer, split (the paths the modules did not take from the newer)
    [
        126, 100, 84, 75, 43, 48, 166, 42, 72, 72, 87, 8, 2, 48, 139, 44, -2, 155, 29, 35, 98, 26, 169, 124, 156, 28,
        159, 66, 187, 35, 90, 16, 99, 1842, 576, -14, 0, 99, 246, 222, 198, 186, 155, 20, -20, 32, 5, -14, 263, 39,
        122, 142, 151, 39, 62, 10, 1, 11,
    ],
];

pub struct RonKlaren {
    l: Layout,
    /// The code hunk in guest memory.
    code: u32,
    /// The Eagleplayer's copy of the module (from the code's $28 bytes on, as
    /// long as its header says), which InitSound copies back.
    saved: Vec<u8>,
    sub: u16,
    songend: bool,
    /// The replayer's d0 (its high byte reaches the period lookups).
    d0: u16,
    /// Cycles run since the last call of the host (a write, the DMA wait).
    pend: i64,
    /// The Eagleplayer's routine after the last write, run before the next.
    tail: Option<At>,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = layout(d)?;
    let (_seg, code) = crate::deli::load_seg(m, d).ok()?;
    let len = m.bus.r32(code + 4);
    let saved = (0..len).map(|i| m.bus.r8(code + 0x28 + i)).collect();
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(l.init_wait);
    let mut rk = RonKlaren { l, code, saved, sub: 1, songend: false, d0: 0, pend: 0, tail: None };
    rk.init_sound(1, &mut hw);
    Some(Box::new(rk))
}

impl Replayer for RonKlaren {
    fn format(&self) -> &'static str {
        "Ron Klaren"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        (self.l.timer as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.l.subsongs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.init_sound(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        // the Eagleplayer calls the interrupt with its address in d0
        self.d0 = self.a(self.l.irq) as u16;
        self.pend = 0;
        self.tail = None;
        self.pend += self.l.prologue as i64;
        self.at(At::Tick);
        if self.l.newer {
            self.play_newer(hw);
        } else {
            self.play_older(hw);
        }
    }
}

// voice variables
const V_HW: u32 = 0;
const V_INSTR: u32 = 4;
const V_LIST: u32 = 8;
const V_POS: u32 = 0xc;
const V_TRACK: u32 = 0x10;
const V_ARP: u32 = 0x14;
const V_TRANSPOSE: u32 = 0x20;
const V_REPEAT: u32 = 0x22;
const V_PERIOD: u32 = 0x24;
const V_PORTA_END: u32 = 0x26;
const V_ADSR_STATE: u32 = 0x28;
const V_VIB_POS: u32 = 0x2a;
const V_ARP_POS: u32 = 0x2c;
const V_DMA: u32 = 0x2e;
const V_NOTE: u32 = 0x30;
const V_WAIT: u32 = 0x31;
const V_PORTA_NOTE: u32 = 0x32;
const V_ADSR_SPEED: u32 = 0x33;
const V_ADSR_COUNT: u32 = 0x34;
const V_VIB_DELAY: u32 = 0x35;
const V_PHASE_COUNT: u32 = 0x36;
const V_PORTA_STEP: u32 = 0x37;
const V_VOLUME: u32 = 0x39;
const V_ONCE: u32 = 0x3a;
const V_SFX: u32 = 0x3b;
const V_SFX_COUNT: u32 = 0x3c;
const V_MUTE: u32 = 0x3d;
// instrument
const I_SAMPLE: u32 = 0;
const I_VIBRATO: u32 = 4;
const I_ONESHOT: u32 = 8;
const I_PHASE_SPEED: u32 = 9;
const I_PHASE_LEN: u32 = 0xa;
const I_VIB_SPEED: u32 = 0xb;
const I_VIB_DEPTH: u32 = 0xc;
const I_VIB_DELAY: u32 = 0xd;
const I_ADSR: u32 = 0xe;
const I_PHASE_VALUE: u32 = 0x16;
const I_PHASE_DIR: u32 = 0x17;
const I_PHASE_POS: u32 = 0x18;

impl RonKlaren {
    fn a(&self, o: usize) -> u32 {
        self.code.wrapping_add(o as u32)
    }

    fn g(&self) -> u32 {
        self.a(self.l.globals)
    }

    fn at(&mut self, e: At) {
        self.pend += COSTS[self.l.timing as usize][e as usize];
    }

    /// The time to a call of the host: the cycles since the last, two a colour
    /// clock (the emulator's count starts again at each).
    fn wait(&mut self, hw: &mut Hw, e: At) {
        if let Some(t) = self.tail.take() {
            self.at(t);
        }
        self.at(e);
        hw.wait((self.pend.max(0) / 2) as u64);
        self.pend = 0;
    }

    fn write(&mut self, hw: &mut Hw, e: At, a: u32, v: u16) {
        self.wait(hw, e);
        hw.bus.write_word(a, v);
    }

    fn loc(&mut self, hw: &mut Hw, e: At, hwb: u32, a: u32) {
        self.wait(hw, e);
        hw.bus.write_word(hwb, (a >> 16) as u16);
        hw.bus.write_word(hwb + 2, a as u16);
    }

    /// ENPP_DMAMask turning voices on: DMACON, then the host's wait for the
    /// DMA, to 10 lines past the current one.
    fn dma_on(&mut self, hw: &mut Hw, mask: u16) {
        self.write(hw, At::WDmaOn, 0xdff096, 0x8000 | mask);
        let now = hw.bus.clock;
        hw.wait((now / LINE + 10) * LINE - now);
    }

    /// The Eagleplayer's InitSound: the module as it was loaded, then the
    /// replayer's subsong routine.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        hw.bus.write_bytes(self.code + 0x28, &self.saved);
        self.sub = n;
        self.songend = false;
        let g = self.g();
        if self.l.newer {
            hw.bus.w8(g + 2, 0);
            let saved = hw.bus.r8(g + 3);
            if saved != 0 {
                hw.bus.w8(g + 1, saved);
                hw.bus.w8(g + 3, 0);
            }
            if hw.bus.r8(g + 4) == 0 && hw.bus.r16(g + 0xa) == 0 {
                hw.bus.w16(g + 8, n);
                hw.bus.w8(g, 1);
                if n != 1 {
                    hw.bus.w16(g + 0xa, 1);
                }
            }
        } else {
            hw.bus.w16(g + 4, n);
            hw.bus.w8(g, 1);
        }
    }

    fn play_newer(&mut self, hw: &mut Hw) {
        let g = self.g();
        if hw.bus.r8(g) == 0 {
            for i in 0..4 {
                self.voice(hw, self.a(self.l.voices[i]));
            }
        } else if hw.bus.r16(g + 8) != 0 {
            let v0 = self.a(self.l.voices[0]);
            let b = self.a(self.l.backup);
            for k in 0..256 {
                hw.bus.w8(b + k, hw.bus.r8(v0 + k));
            }
            self.new_song(hw, hw.bus.r16(g + 8));
            hw.bus.w8(g, 0);
            hw.bus.w16(g + 8, 0);
        } else if hw.bus.r16(g + 6) != 0 {
            let v0 = self.a(self.l.voices[0]);
            let b = self.a(self.l.backup);
            for k in 0..256 {
                hw.bus.w8(v0 + k, hw.bus.r8(b + k));
            }
            self.at(At::Restore);
            for i in 0..4 {
                let v = self.a(self.l.voices[i]);
                let hwb = hw.bus.r32(v + V_HW);
                self.write(hw, At::WDmaOff, 0xdff096, hw.bus.r16(v + V_DMA));
                self.write(hw, At::Direct, hwb + 6, hw.bus.r16(v + V_PERIOD));
                let s = hw.bus.r32(hw.bus.r32(v + V_INSTR) + I_SAMPLE);
                self.loc(hw, At::Direct, hwb, hw.bus.r32(s));
                self.write(hw, At::Direct, hwb + 4, hw.bus.r16(s + 4));
                self.d0 = hw.bus.r8(v + V_VOLUME) as u16;
                self.write(hw, At::Direct, hwb + 8, self.d0);
            }
            hw.bus.w16(g + 0xa, 0);
            hw.bus.w16(g + 6, 0);
            hw.bus.w8(g, 0);
        }
    }

    fn play_older(&mut self, hw: &mut Hw) {
        let g = self.g();
        if hw.bus.r8(g) == 0 {
            for i in 0..4 {
                self.voice(hw, self.a(self.l.voices[i]));
            }
        } else if hw.bus.r16(g + 4) != 0 {
            self.new_song(hw, hw.bus.r16(g + 4));
            hw.bus.w8(g, 0);
            hw.bus.w16(g + 4, 0);
        }
    }

    /// A subsong's start: all DMA off, each voice's track list and InitVoice.
    fn new_song(&mut self, hw: &mut Hw, n: u16) {
        self.at(At::NewSong);
        self.write(hw, At::WDmaOff, 0xdff096, 0xf);
        let d0 = ((n & 0xf).wrapping_sub(1) << 4) as i16;
        let tab = self.a(self.l.subsong_table).wrapping_add(d0 as i32 as u32);
        for i in 0..4 {
            let v = self.a(self.l.voices[i]);
            hw.bus.w32(v + V_LIST, hw.bus.r32(tab + 4 * i as u32));
            self.init_voice(hw, v);
        }
    }

    fn init_voice(&mut self, hw: &mut Hw, v: u32) {
        for o in [0x30, 0x31, 0x32, 0x33, 0x34, 0x39, 0x35, 0x37, 0x36, 0x3b, 0x3c] {
            hw.bus.w8(v + o, 0);
        }
        if self.l.newer {
            hw.bus.w8(v + V_ONCE, 1);
            hw.bus.w8(v + V_MUTE, 0);
        }
        for o in [V_ADSR_STATE, V_VIB_POS, V_ARP_POS] {
            hw.bus.w16(v + o, 0);
        }
        for o in [0x14, 0x18, 0x1c] {
            hw.bus.w32(v + o, 0);
        }
        let ins = self.a(self.l.instruments);
        hw.bus.w32(v + V_INSTR, ins);
        let s = hw.bus.r32(ins + I_SAMPLE);
        let hwb = hw.bus.r32(v + V_HW);
        self.at(At::InitVoice);
        let (wloc, wlen, wvol) =
            if self.l.init_direct { (At::Direct, At::Direct, At::Direct) } else { (At::WLoc, At::WLen, At::WVol) };
        self.loc(hw, wloc, hwb, hw.bus.r32(s));
        self.write(hw, wlen, hwb + 4, hw.bus.r16(s + 4));
        if self.l.init_volume {
            self.write(hw, wvol, hwb + 8, 0);
        }
        self.at(At::InitVoiceLen);
        let list = hw.bus.r32(v + V_LIST);
        hw.bus.w32(v + V_POS, list);
        hw.bus.w32(v + V_TRACK, hw.bus.r32(list));
        hw.bus.w16(v + V_TRANSPOSE, hw.bus.r16(list + 6));
        self.d0 = hw.bus.r16(list + 0xa).wrapping_sub(1);
        hw.bus.w16(v + V_REPEAT, self.d0);
    }

    fn period(&self, hw: &Hw, d0: u16) -> u16 {
        hw.bus.r16(self.a(self.l.periods).wrapping_add(d0 as i16 as i32 as u32))
    }

    /// The voice routine.
    fn voice(&mut self, hw: &mut Hw, v: u32) {
        let newer = self.l.newer;
        let g = self.g();
        let ins = hw.bus.r32(v + V_INSTR);
        let hwb = hw.bus.r32(v + V_HW);
        self.at(At::Voice);
        if !newer || hw.bus.r8(v + V_MUTE) == 0 {
            if hw.bus.r8(v + V_SFX) != 0 {
                // a sound effect playing (only the game starts them)
                let c = hw.bus.r8(v + V_SFX_COUNT);
                if c != 0 {
                    hw.bus.w8(v + V_SFX_COUNT, c - 1);
                } else {
                    hw.bus.w8(v + V_SFX, 0);
                    if !newer || hw.bus.r8(g + 4) == 0 {
                        if newer {
                            hw.bus.w8(v + V_ONCE, hw.bus.r8(ins + I_ONESHOT));
                        }
                        let s = hw.bus.r32(ins + I_SAMPLE);
                        self.loc(hw, At::WLoc, hwb, hw.bus.r32(s));
                        self.write(hw, At::WLen, hwb + 4, hw.bus.r16(s + 4));
                    }
                }
            }
            self.d0 = 0x8000 | hw.bus.r16(v + V_DMA);
            self.dma_on(hw, self.d0 & 0x7fff);
            self.at(At::AfterDma);
            if hw.bus.r8(v + V_ONCE) != 0 {
                hw.bus.w8(v + V_ONCE, 0);
                self.write(hw, At::WLen, hwb + 4, 1);
                self.at(At::Len1);
            }
        }
        if newer && hw.bus.r8(g + 4) != 0 {
            return;
        }
        let wait = hw.bus.r8(v + V_WAIT);
        if wait == 0 {
            self.parse(hw, v);
            self.at(At::RowEnd);
        } else {
            hw.bus.w8(v + V_WAIT, wait - 1);
            if hw.bus.r8(v + V_SFX) != 0 {
                return;
            }
            self.effects(hw, v, ins, hwb);
        }
        self.at(At::VoiceEnd);
    }

    fn per_write(&mut self, hw: &mut Hw, v: u32, hwb: u32, e: At, p: u16) {
        self.at(e);
        if !self.l.newer || hw.bus.r8(v + V_MUTE) == 0 {
            self.write(hw, At::WPer, hwb + 6, p);
            self.tail = Some(match e {
                At::PortaStep => At::TailPorta,
                At::Arp => At::TailArp,
                At::Vib => At::TailVib,
                _ => At::TailRow,
            });
        }
    }

    fn effects(&mut self, hw: &mut Hw, v: u32, ins: u32, hwb: u32) {
        let g = self.g();
        self.at(At::Fx);
        self.d0 = 0;
        // phasing: a byte of the synth waveform rewritten, back and forth
        let speed = hw.bus.r8(ins + I_PHASE_SPEED);
        if speed != 0 {
            self.at(At::Phase);
            let c = hw.bus.r8(v + V_PHASE_COUNT);
            if c != 0 {
                hw.bus.w8(v + V_PHASE_COUNT, c - 1);
            } else {
                self.at(At::PhaseStep);
                hw.bus.w8(v + V_PHASE_COUNT, speed.wrapping_sub(1));
                let s = hw.bus.r32(ins + I_SAMPLE);
                let mut d0 = hw.bus.r16(s + 6);
                d0 = d0 & 0xff00 | (d0 as u8).wrapping_sub(hw.bus.r8(ins + I_PHASE_LEN)) as u16;
                let a4 = hw.bus.r32(s).wrapping_add(d0 as i16 as i32 as u32);
                let mut pos = hw.bus.r8(ins + I_PHASE_POS);
                hw.bus.w8(a4 + pos as u32, hw.bus.r8(ins + I_PHASE_VALUE));
                let back = hw.bus.r8(ins + I_PHASE_DIR) & 0x80 != 0;
                let turn = if !back {
                    pos = pos.wrapping_add(1);
                    hw.bus.r8(ins + I_PHASE_LEN).wrapping_mul(2) == pos
                } else {
                    self.at(At::PhaseBack);
                    pos = pos.wrapping_sub(1);
                    pos == 0
                };
                if turn {
                    self.at(if back { At::PhaseBackTurn } else { At::PhaseTurn });
                    hw.bus.w8(ins + I_PHASE_DIR, !hw.bus.r8(ins + I_PHASE_DIR));
                    hw.bus.w8(ins + I_PHASE_VALUE, !hw.bus.r8(ins + I_PHASE_VALUE));
                }
                hw.bus.w8(ins + I_PHASE_POS, pos);
                self.d0 = pos as u16;
            }
        }
        let step = hw.bus.r8(v + V_PORTA_STEP);
        if step != 0 {
            // portamento towards its period
            self.at(At::Porta);
            self.d0 = step as u16;
            let end = hw.bus.r16(v + V_PORTA_END);
            let mut p = hw.bus.r16(v + V_PERIOD);
            if end.wrapping_sub(p) as i16 >= 0 {
                p = p.wrapping_add(step as u16);
                if end.wrapping_sub(p) as i16 >= 0 {
                } else {
                    self.at(At::PortaClampUp);
                    p = end;
                }
            } else {
                p = p.wrapping_sub(step as u16);
                if (end.wrapping_sub(p) as i16) < 0 {
                } else {
                    self.at(At::PortaClampDown);
                    p = end;
                }
            }
            hw.bus.w16(v + V_PERIOD, p);
            self.per_write(hw, v, hwb, At::PortaStep, p);
        } else {
            // the arpeggio, one step back each tick
            self.at(At::Arp);
            let mut d1 = hw.bus.r16(v + V_ARP_POS);
            let a = hw.bus.r8(v.wrapping_add(V_ARP).wrapping_add(d1 as i16 as i32 as u32));
            let mut d0 = a.wrapping_add(hw.bus.r8(v + V_NOTE)) as u16;
            d0 = d0.wrapping_add(hw.bus.r16(v + V_TRANSPOSE));
            d0 = d0.wrapping_add(d0);
            let p = self.period(hw, d0);
            self.d0 = p;
            hw.bus.w16(v + V_PERIOD, p);
            self.per_write(hw, v, hwb, At::Arp, p);
            d1 = d1.wrapping_sub(1);
            if (d1 as i16) < 0 {
                self.at(At::ArpWrap);
                d1 = d1.wrapping_add(12);
            }
            hw.bus.w16(v + V_ARP_POS, d1);
        }
        let vspeed = hw.bus.r8(ins + I_VIB_SPEED);
        if vspeed == 0 {
            self.at(At::VibNone);
        } else {
            let delay = hw.bus.r8(v + V_VIB_DELAY);
            if delay != 0 {
                self.at(At::VibDelay);
                hw.bus.w8(v + V_VIB_DELAY, delay - 1);
            } else {
                let pos = hw.bus.r16(v + V_VIB_POS);
                let vib = hw.bus.r32(ins + I_VIBRATO);
                let tab = hw.bus.r32(vib);
                let d2 = hw.bus.r8(tab.wrapping_add(pos as i16 as i32 as u32)) as i8 as i16;
                let depth = hw.bus.r8(ins + I_VIB_DEPTH) as i16;
                let p = (d2 as i32 * depth as i32) as u16;
                let p = p.wrapping_add(hw.bus.r16(v + V_PERIOD));
                self.per_write(hw, v, hwb, At::Vib, p);
                let np = pos.wrapping_sub(vspeed as u16);
                self.d0 = vspeed as u16;
                if (np as i16) < 0 {
                    self.at(At::VibWrap);
                    self.d0 = hw.bus.r16(vib + 4).wrapping_mul(2);
                    hw.bus.w16(v + V_VIB_POS, self.d0);
                } else {
                    hw.bus.w16(v + V_VIB_POS, np);
                }
            }
        }
        // the envelope
        let c = hw.bus.r8(v + V_ADSR_COUNT).wrapping_sub(1);
        hw.bus.w8(v + V_ADSR_COUNT, c);
        if c & 0x80 == 0 {
            self.at(At::AdsrWait);
            return;
        }
        self.at(At::Adsr);
        hw.bus.w8(v + V_ADSR_COUNT, hw.bus.r8(v + V_ADSR_SPEED));
        let mut d0 = hw.bus.r16(v + V_ADSR_STATE);
        let pt = |k: u32| ins.wrapping_add(I_ADSR).wrapping_add(d0 as i16 as i32 as u32).wrapping_add(k);
        let d1 = hw.bus.r8(pt(0));
        let d2 = hw.bus.r8(pt(4));
        let mut d3 = hw.bus.r8(v + V_VOLUME);
        let next;
        if d1.wrapping_sub(d3) & 0x80 == 0 {
            self.at(At::AdsrAttack);
            d3 = d3.wrapping_add(d2);
            let peak = hw.bus.r8(g + 1);
            if d3.wrapping_sub(peak) & 0x80 == 0 {
                self.at(At::AdsrPeak);
                d3 = peak;
                next = true;
            } else if d1.wrapping_sub(d3) & 0x80 == 0 {
                next = false;
            } else {
                self.at(At::AdsrToPoint);
                d3 = d1;
                next = true;
            }
        } else {
            d3 = d3.wrapping_sub(d2);
            if d1.wrapping_sub(d3) & 0x80 != 0 {
                next = false;
            } else {
                d3 = d1;
                next = true;
            }
        }
        if next {
            self.at(At::AdsrNext);
            if d0 != 3 {
                d0 = d0.wrapping_add(1);
            } else {
                self.at(At::AdsrLast);
            }
        }
        self.d0 = d0;
        hw.bus.w16(v + V_ADSR_STATE, d0);
        hw.bus.w8(v + V_VOLUME, d3);
        if !self.l.newer || hw.bus.r8(v + V_MUTE) == 0 {
            self.write(hw, At::WVol, hwb + 8, (d3 & 0x3f) as u16);
            self.tail = Some(At::TailVol);
        }
    }

    /// The next row of the voice's track.
    fn parse(&mut self, hw: &mut Hw, v: u32) {
        let newer = self.l.newer;
        let g = self.g();
        let hwb = hw.bus.r32(v + V_HW);
        let mut ins = hw.bus.r32(v + V_INSTR);
        let mut a2 = hw.bus.r32(v + V_TRACK);
        self.at(At::Parse);
        if newer && hw.bus.r8(g + 4) != 0 {
            return;
        }
        hw.bus.w8(v + V_PORTA_STEP, 0);
        let d = hw.bus.r8(ins + I_VIB_DELAY);
        self.d0 = self.d0 & 0xff00 | d as u16;
        if d != 0 {
            self.at(At::ParseVibDelay);
            let d = (d << 2).wrapping_sub(1);
            self.d0 = self.d0 & 0xff00 | d as u16;
            hw.bus.w8(v + V_VIB_DELAY, d);
        }
        loop {
            if hw.bus.r8(a2) == 0x80 {
                self.at(At::Cmd80);
                hw.bus.w16(v + V_ARP_POS, 0);
                let n = hw.bus.r8(a2 + 1) as u16;
                self.d0 = (n << 3).wrapping_add(n << 2);
                let a4 = self.a(self.l.arpeggios).wrapping_add(self.d0 as u32);
                for k in 0..12 {
                    hw.bus.w8(v + V_ARP + k, hw.bus.r8(a4 + k));
                }
                a2 = a2.wrapping_add(2);
            }
            if hw.bus.r8(a2) == 0x81 {
                self.at(At::Cmd81);
                if self.l.clear_adsr_porta {
                    hw.bus.w16(v + V_ADSR_STATE, 0);
                }
                let n = hw.bus.r8(a2 + 1);
                hw.bus.w8(v + V_PORTA_NOTE, n);
                let mut d0 = (n as u16).wrapping_add(hw.bus.r16(v + V_TRANSPOSE));
                d0 = d0.wrapping_add(d0);
                hw.bus.w16(v + V_PORTA_END, self.period(hw, d0));
                hw.bus.w8(v + V_PORTA_STEP, hw.bus.r8(a2 + 2));
                let w = (hw.bus.r8(a2 + 3) << 2).wrapping_sub(1);
                self.d0 = d0 & 0xff00 | w as u16;
                hw.bus.w8(v + V_WAIT, w);
                a2 = a2.wrapping_add(4);
                self.end_row(hw, v, hwb, a2);
                return;
            }
            if hw.bus.r8(a2) == 0x82 {
                self.at(At::Cmd82);
                if self.l.clear_volume {
                    hw.bus.w8(v + V_VOLUME, 0);
                }
                hw.bus.w16(v + V_ADSR_STATE, 0);
                self.d0 = (hw.bus.r8(a2 + 1) as u16) << 5;
                ins = self.a(self.l.instruments).wrapping_add(self.d0 as i16 as i32 as u32);
                hw.bus.w32(v + V_INSTR, ins);
                if (!newer || hw.bus.r8(v + V_MUTE) == 0) && hw.bus.r8(v + V_SFX) == 0 {
                    self.sample(hw, v, hwb, ins, self.l.instr_split);
                }
                a2 = a2.wrapping_add(2);
            }
            if hw.bus.r8(a2) == 0x83 {
                hw.bus.w8(g, 1);
                if newer {
                    hw.bus.w16(g + 8, 0);
                    hw.bus.w16(g + 6, 1);
                }
                return;
            }
            if hw.bus.r8(a2) == 0x84 {
                self.at(At::Cmd84);
                hw.bus.w8(v + V_ADSR_SPEED, hw.bus.r8(a2 + 1));
                a2 = a2.wrapping_add(2);
            }
            if self.l.stop85 && hw.bus.r8(a2) == 0x85 {
                hw.bus.w8(g, 1);
                return;
            }
            self.at(At::Checks);
            if hw.bus.r8(a2) & 0x80 != 0 {
                // the track's end: again, or the list's next track
                let rep = hw.bus.r16(v + V_REPEAT);
                let a4;
                if rep != 0 {
                    self.at(At::Repeat);
                    hw.bus.w16(v + V_REPEAT, rep - 1);
                    a4 = hw.bus.r32(v + V_POS);
                } else {
                    self.at(At::NextTrack);
                    let mut p = hw.bus.r32(v + V_POS).wrapping_add(12);
                    hw.bus.w32(v + V_POS, p);
                    hw.bus.w16(v + V_REPEAT, hw.bus.r16(p + 0xa).wrapping_sub(1));
                    if hw.bus.r8(p) & 0x80 != 0 {
                        p = hw.bus.r32(v + V_LIST);
                        hw.bus.w32(v + V_POS, p);
                        self.songend = true;
                    }
                    self.d0 = hw.bus.r16(p + 0xa).wrapping_sub(1);
                    hw.bus.w16(v + V_REPEAT, self.d0);
                    a4 = p;
                }
                a2 = hw.bus.r32(a4);
                hw.bus.w32(v + V_TRACK, a2);
                hw.bus.w16(v + V_TRANSPOSE, hw.bus.r16(a4 + 6));
                continue;
            }
            let wait = hw.bus.r8(a2 + 1);
            self.d0 = self.d0 & 0xff00 | wait as u16;
            if wait == 0 {
                // a note that only sets the period
                self.at(At::Legato);
                let n = hw.bus.r8(a2);
                self.d0 = self.d0 & 0xff00 | n as u16;
                hw.bus.w8(v + V_NOTE, n);
                let mut d0 = self.d0.wrapping_add(hw.bus.r16(v + V_TRANSPOSE));
                d0 = d0.wrapping_add(d0);
                self.d0 = d0;
                hw.bus.w16(v + V_PERIOD, self.period(hw, d0));
                a2 = a2.wrapping_add(2);
                continue;
            }
            self.at(At::Note);
            hw.bus.w8(v + V_WAIT, (wait << 2).wrapping_sub(1));
            hw.bus.w16(v + V_ADSR_STATE, 0);
            let n = hw.bus.r8(a2);
            hw.bus.w8(v + V_NOTE, n);
            let mut d0 = (n as u16).wrapping_add(hw.bus.r16(v + V_TRANSPOSE));
            d0 = d0.wrapping_add(d0);
            self.d0 = d0;
            hw.bus.w16(v + V_PERIOD, self.period(hw, d0));
            if (newer && hw.bus.r8(v + V_MUTE) != 0) || hw.bus.r8(v + V_SFX) != 0 {
                hw.bus.w32(v + V_TRACK, a2.wrapping_add(2));
                return;
            }
            self.sample(hw, v, hwb, ins, false);
            a2 = a2.wrapping_add(2);
            if hw.bus.r8(ins + I_ONESHOT) != 0 {
                self.at(At::NoteOnce);
                hw.bus.w8(v + V_ONCE, 1);
            }
            self.end_row(hw, v, hwb, a2);
            return;
        }
    }

    /// DMA off and the instrument's sample.
    fn sample(&mut self, hw: &mut Hw, v: u32, hwb: u32, ins: u32, split: bool) {
        self.write(hw, At::WDmaOff, 0xdff096, hw.bus.r16(v + V_DMA));
        let s = hw.bus.r32(ins + I_SAMPLE);
        self.loc(hw, if split { At::WLocSplit } else { At::WLoc }, hwb, hw.bus.r32(s));
        self.write(hw, At::WLen, hwb + 4, hw.bus.r16(s + 4));
    }

    fn end_row(&mut self, hw: &mut Hw, v: u32, hwb: u32, a2: u32) {
        hw.bus.w32(v + V_TRACK, a2);
        let p = hw.bus.r16(v + V_PERIOD);
        self.per_write(hw, v, hwb, At::Checks, p);
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
        /// (where, label, kind): 0 pc-relative word, 1 relocated long, 2 word
        /// from `globals`.
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
        fn r(&mut self, name: &'static str) -> &mut Self {
            self.fix.push((self.d.len(), name, 0));
            self.w(&[0])
        }
        fn p(&mut self, name: &'static str) -> &mut Self {
            self.fix.push((self.d.len(), name, 1));
            self.w(&[0, 0])
        }
        fn g(&mut self, name: &'static str) -> &mut Self {
            self.fix.push((self.d.len(), name, 2));
            self.w(&[0])
        }
        /// The code hunk as an executable, its pointers relocated.
        fn exe(&mut self) -> Vec<u8> {
            self.d.resize(0xa40, 0);
            let hdr = (self.d.len() - 0x28) as u32;
            self.d[4..8].copy_from_slice(&hdr.to_be_bytes());
            let mut relocs = Vec::new();
            for &(at, name, kind) in &self.fix {
                let t = self.labels[name] as i64;
                match kind {
                    0 => self.d[at..at + 2].copy_from_slice(&((t - at as i64) as u16).to_be_bytes()),
                    1 => {
                        self.d[at..at + 4].copy_from_slice(&(t as u32).to_be_bytes());
                        relocs.push(at as u32);
                    }
                    _ => self.d[at..at + 2].copy_from_slice(&((t - self.labels["globals"] as i64) as u16).to_be_bytes()),
                }
            }
            let n = self.d.len() as u32 / 4;
            let mut e = Vec::new();
            for x in [0x3f3, 0, 1, 0, 0, n, 0x3e9, n] {
                e.extend((x as u32).to_be_bytes());
            }
            e.extend(&self.d);
            for x in [0x3ec, relocs.len() as u32, 0].into_iter().chain(relocs).chain([0, 0x3f2]) {
                e.extend(x.to_be_bytes());
            }
            e
        }
    }

    /// An older replayer with one subsong, whose four voices play the same
    /// track list: a note held two rows, again and again, with one instrument
    /// (a 32-byte sample, an envelope straight to its peak). The code is only
    /// the parts the Eagleplayer and the port look for.
    fn module() -> Vec<u8> {
        module_with(false)
    }

    /// `module`, whose first jump goes to the timer set-up if `jump`, as the
    /// replayers' do; otherwise it goes nowhere and the set-up is found in the
    /// code.
    fn module_with(jump: bool) -> Vec<u8> {
        let mut a = Asm::default();
        a.w(&[0x70ff, 0x4e75, 0, 0]);
        a.d.extend(b"RON_KLAREN_SOUNDMODULE!\0");
        // a jump, then the subsong entries (move.w #1,d0; jmp)
        a.w(&[0x4ef9]);
        if jump {
            a.p("timer");
        } else {
            a.w(&[0, 0]);
        }
        a.w(&[0x303c, 1, 0x4ef9, 0, 0]);
        // the timer set-up, the interrupt at `irq`
        a.l("timer").w(&[0x33fc, 0x4000, 0x00df, 0xf09a, 0x13fc, 0x0000, 0x00bf, 0xde00, 0x13fc, 0x0037, 0x00bf, 0xd400]);
        a.w(&[0x13fc, 0x0037, 0x00bf, 0xd500, 0x13fc, 0x0081, 0x00bf, 0xdd00, 0x13fc, 0x0011, 0x00bf, 0xde00]);
        a.w(&[0x23f9, 0, 0x78, 0, 0, 0x23fc]).p("irq").w(&[0, 0x78]);
        a.w(&[0x33fc, 0xc000, 0x00df, 0xf09a, 0x33fc, 0x800f, 0x00df, 0xf096, 0x4e75]);
        // the interrupt: each voice's routine
        a.l("irq");
        for v in ["v0", "v1", "v2", "v3"] {
            a.w(&[0x41fa]).r(v).w(&[0x6100]).r("voice");
        }
        a.w(&[0x4e75]);
        a.l("voice").w(&[0x2268, 4, 0x2468, 0x10, 0x2668, 0, 0x4e75]);
        // the tables' references, the subsong routine's
        a.w(&[0x43fa]).r("globals").w(&[0xd602, 0xb629, 1]);
        a.w(&[0x49fa]).r("periods").w(&[0x3174, 0, 0x24]);
        a.w(&[0x49fa]).r("arps").w(&[0xd9c0]);
        a.w(&[0xeb48, 0x49fa]).r("instrs").w(&[0x43f4, 0]);
        a.w(&[0x3028, 4, 0x0240, 0xf, 0x5340, 0xe948, 0x47f0]).g("subsongs").w(&[0x4e75]);
        // stopped, the envelope's peak $3f, the subsong
        a.l("globals").w(&[0x003f, 0, 0, 0]);
        a.l("subsongs").p("list").p("list").p("list").p("list");
        // the track, 2 times, then the end
        a.l("list").p("track").w(&[0, 0, 0, 2]).w(&[0xffff; 6]);
        a.l("track").w(&[0x1802, 0xff00]);
        a.l("periods").w(&[0x100; 24]).w(&[428]).w(&[0x100; 24]);
        a.l("arps").w(&[0; 6]);
        // the sample, no vibrato, no phasing, the envelope
        a.l("instrs").p("sample").w(&[0; 5]).w(&[0x3f3f, 0x3f3f, 0x3f3f, 0x3f3f]).w(&[0; 5]);
        a.l("sample").p("sdata").w(&[16, 0]);
        a.l("sdata").w(&[0x4000; 16]);
        for (v, n) in [("v0", 0), ("v1", 1), ("v2", 2), ("v3", 3)] {
            let mut voice = [0u16; 0x20];
            voice[1] = 0xf0a0 + 16 * n;
            voice[0] = 0xdf;
            voice[0x17] = 1 << n;
            a.l(v).w(&voice);
        }
        a.exe()
    }

    #[test]
    fn finds_the_timer_and_interrupt_from_the_header_jump_or_the_code() {
        let by_jump = super::layout(&module_with(true)).unwrap();
        let by_code = super::layout(&module_with(false)).unwrap();
        // the module's $3737 in the timer's two bytes
        assert_eq!(by_jump.timer, 0x3737);
        assert_eq!((by_code.timer, by_code.irq), (by_jump.timer, by_jump.irq));
        assert_ne!(by_jump.irq, 0);
        // an older replayer; it has no InitVoice, whose writes are then direct
        assert_eq!(by_jump.timing, super::Timing::Older);
        assert!(by_jump.init_direct);
    }

    #[test]
    fn plays_a_note_on_paula() {
        let d = module();
        assert!(super::detect(&d));
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.rk"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Ron Klaren");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the subsong's start: all DMA off; then each voice's DMA on, and the
        // note's DMA off before its sample
        assert_eq!(&reg(0xDFF096)[..9], &[0x000F, 0x8001, 1, 0x8002, 2, 0x8004, 4, 0x8008, 8]);
        // InitVoice's sample and the note's: 16 words at the same address
        assert_eq!(&reg(0xDFF0A4)[..2], &[16, 16]);
        let loc = reg(0xDFF0A2);
        assert_eq!(loc[0], loc[1]);
        // the note's period (note 24 in the table), then the envelope's peak
        assert_eq!(&reg(0xDFF0A6)[..2], &[428, 428]);
        assert_eq!(reg(0xDFF0A8)[0], 63);
        // the DMA wait: 10 lines from a DMA on to the next write
        let at = |v: u16| log.iter().position(|w| w.0 == 0xDFF096 && w.1 == v).unwrap();
        let i = at(0x8001);
        assert!(log[i + 1].2 - log[i].2 >= 9 * 227);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
