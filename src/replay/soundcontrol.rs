//! Sound Control 3.x, 4.0 and 5.0, ported from NostalgicPlayer's SoundControl
//! player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! NostalgicPlayer has two players: 3.x (with a 3.2 variant, the transposing
//! sixth track, for two modules it knows by length) and 4.0/5.0 (4.0 for the
//! three Hot Numbers Deluxe modules it knows by length, with their own subsongs
//! and speeds). The original this port follows is the SoundControl Eagleplayer
//! ("SoundControl player module V1.01", Holger Gehrmann, adapted by Turbo/Infect
//! & DEFECT), which has one replayer for each of 3.x and 4.0/5.0 and tells them
//! apart by the sample command list's length (the long at $1C, 0 in 3.x). It
//! plays from the module in guest memory and keeps its variables in its own
//! hunks, laid out here as there (`H0` holds the original's addresses), and
//! writes Paula through the host's ENPP_ calls. It has no subsongs.
//!
//! Where NostalgicPlayer differs from the original, this follows the original:
//! * 3.x: the counter runs ProcessCounter twice every fourth tick (NostalgicPlayer:
//!   every second or third), and every module has the sixth track's transposes
//!   (each to its own voice, `dat1 & 3`, not the inverse), applied less one: a
//!   transpose of 0 plays a note one lower. A sample's note transpose is
//!   unsigned. A note plays its sample: LOC, LEN 1, PER 1, DMA off, VOL, PER,
//!   LEN, the sample's first long cleared, LOC, DMA on, then the loop, or LEN 1
//!   if the loop start is 0. InitSound points the voices at the zeros of the
//!   original's second hunk (LEN 1, PER 1), sets the first track of position 0
//!   to 1 and turns DMA on. The song ends when a position has neither its first
//!   nor its second track's pad byte at 1.
//! * 4.0 and 5.0 play alike: 5.0's speed (46 and every fourth step), the speed
//!   and positions of the module (no subsongs, from position 0), no volume
//!   clamp. Nothing is written as the rows are read; the sample commands write
//!   as they run: a play sample command writes LOC, DMA on, LEN 1, PER 1 and
//!   VOL 0 on its first tick; DMA off, PER, LEN, the sample's first long cleared
//!   and DMA on on its second; the loop (or LEN 2) on its third, and goes on to
//!   the next command. Its DMA bits are the low bits of whatever the original's
//!   d4 holds: the voice's bit, or twice the note a switch sample command left
//!   there. The address and length commands write at once. Every tick writes
//!   the four volumes, the note's times the envelope's over 256. The repeat
//!   command repeats while the value has not passed its end (NostalgicPlayer:
//!   not reached), and steps the value its set command's raw argument names.
//! * Track, command and table reads run on in memory, and nothing is
//!   range-checked; the original's word and byte arithmetic is kept.
//!
//! A module is taken when both the Eagleplayer's check and NostalgicPlayer's
//! pass. The writes come at the original's colour clocks: each is a host call
//! (ENPP_PokeAdr, ...; the song's end calls ENPP_SongEnd), which the engine runs
//! at the 68000 cycles the original's code took since the last one, halved and
//! rounded down. `At` holds the cycles of each path between them, integers
//! fitted to every host-call gap of the reference (the 12 modules the
//! Eagleplayer plays, 20 s and 90 s, and about 150 modified copies taking the
//! paths they do not: 903 409 gaps, all exact), and the start waits as long as
//! the reference's check, InitPlayer and InitSound (`Scan::start`).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "SoundControl", detect, new };

include!("soundcontrol_tables.rs");

/// The original's first hunk starts here in the addresses below; its second
/// hunk (`H1_SIZE` bytes, zeros) holds the 4.0/5.0 period table at 8.
const H0: u32 = 0x10000;
const H0_SIZE: u32 = 0x19c8;
const H1_SIZE: u32 = 0x108;

// Variables of both replayers.
/// The module (long).
const MODULE: u32 = 0x10a06;

// 3.x.
const START_POS: u32 = 0x10a0a;
/// Six track pointers (longs) and their wait counters (words).
const TRACKS3: u32 = 0x10a12;
const WAITS3: u32 = 0x10a2a;
const COUNTER2: u32 = 0x10a36;
const POS3: u32 = 0x10a3a;
const COUNTER1: u32 = 0x10a3e;
const TRACK_TABLE3: u32 = 0x10a4e;
const SAMPLES3: u32 = 0x10a56;
const POSITIONS3: u32 = 0x10a5e;
/// The voices' transposes (bytes).
const TRANSPOSE3: u32 = 0x10a62;
const PERIODS3: u32 = 0x10a72;
/// 3.x's InitSound clears its variables from START_POS to here.
const CLEAR3_END: u32 = 0x10e72;
const BASE3: u32 = 0x109e6;

// 4.0/5.0.
const SPEED_MAX: u32 = 0x10e72;
const STEPS: u32 = 0x10e74;
const STEPS4: u32 = 0x10e76;
/// The period table (long, to the second hunk).
const PERIODS4: u32 = 0x10e78;
const HALT: u32 = 0x10e7c;
const SAMPLES4: u32 = 0x10e82;
const TRACK_TABLE4: u32 = 0x10e8a;
const INSTRUMENTS4: u32 = 0x10e92;
const POSITIONS4: u32 = 0x10e96;
const MODULE4: u32 = 0x10e9a;
const START_POS4: u32 = 0x10e9e;
const END_POS4: u32 = 0x10ea0;
const SPEED4: u32 = 0x10ea4;
const SPEED_ACC: u32 = 0x10ea6;
/// Play disabled ($FFFF until InitSound).
const OFF4: u32 = 0x10eae;
/// The voice not played ($FFFF: none).
const MUTED4: u32 = 0x10eb0;
const POS4: u32 = 0x10eb2;
const GLOBAL_TRANSPOSE4: u32 = 0x10ebe;
const MASTER4: u32 = 0x10ec0;
const VOICE_MASK4: u32 = 0x10ec2;
/// The voices' transposes (words).
const TRANSPOSE4: u32 = 0x10ec4;
const BASE4: u32 = 0x10ed0;
const TRACKS4: u32 = 0x10ef0;
const WAITS4: u32 = 0x10f08;
/// The four voices' structures.
const VOICES4: [u32; 4] = [0x110ba, 0x11182, 0x111cc, 0x11216];

// A 4.0/5.0 voice's fields.
const V_CMDS: u32 = 0x00;
const V_WAIT: u32 = 0x04;
const V_INSTR: u32 = 0x06;
const V_SAMPLE_NR: u32 = 0x08;
const V_PERIOD: u32 = 0x0a;
const V_NOTE: u32 = 0x0c;
const V_SAMPLE_NOTE: u32 = 0x0e;
const V_VOLUME: u32 = 0x10;
const V_DATA: u32 = 0x12;
const V_SAMPLE: u32 = 0x16;
const V_LENGTH: u32 = 0x1a;
const V_PHASE: u32 = 0x1c;
const V_ENV: u32 = 0x1e;
const V_ENV_COUNTER: u32 = 0x20;
const V_ENV_VOLUME: u32 = 0x22;
const V_RELEASE: u32 = 0x24;
const V_STACK_PTR: u32 = 0x26;
const V_STACK: u32 = 0x2a;
const V_VALUES: u32 = 0x3a;

/// The 4.0 modules, which NostalgicPlayer knows by their length (64 plus the
/// four parts'): Hot Numbers Deluxe's intro, ongame 2 and title.
const LENGTHS40: [u32; 3] = [81906, 95960, 54544];

/// Most commands a voice runs in one tick, and rows read in one track step: the
/// original runs on for ever on a list or song that never waits.
const MAX_LOOP: u32 = 0x10000;

fn sx(w: u16) -> u32 {
    w as i16 as i32 as u32
}

/// The Eagleplayer's Check5: the first track's name (16 characters from $20 to
/// $7A) at $240, then the track's rows up to its end, and any further tracks
/// and names, up to 100 rows each.
fn ep_check(d: &[u8]) -> Option<Scan> {
    let r = |a: usize| d.get(a).copied().unwrap_or(0);
    let text = |c: u8| (0x20..=0x7a).contains(&c);
    let mut a = 0x240;
    let mut s = Scan::default();
    'name: loop {
        s.names += 1;
        for _ in 0..16 {
            a += 1;
            if !text(r(a - 1)) {
                return None;
            }
        }
        let mut left = 100;
        loop {
            left -= 1;
            if left == 0 || a > d.len() {
                return None;
            }
            let c = r(a);
            a += 4;
            if text(c) {
                s.text += 1;
                continue;
            }
            if c == 0 {
                s.zero += 1;
                continue;
            }
            if r(a) == 0xff {
                if r(a + 1) != 0xff {
                    return None;
                }
                if c == 0xff {
                    s.ff_skips += 1;
                } else {
                    s.skips += 1;
                }
                a += 4;
            } else if r(a - 2) == 0xff {
                s.short_end = true;
                return Some(s);
            } else if c != 0xff || r(a - 3) != 0xff {
                return None;
            }
            a -= 2;
            if r(a) == 0 && r(a + 1) == 0 {
                return Some(s);
            }
            continue 'name;
        }
    }
}

/// What the Eagleplayer's check read: rows whose first byte is a character, rows
/// whose first byte is 0, track names (with the row that ends each track), the
/// tracks that end instead on a row followed by an $FFFF (a row whose first byte
/// is neither, or $FF), and whether the last track ended on its row's third byte
/// rather than on a name of zeros.
#[derive(Default)]
struct Scan {
    text: u32,
    zero: u32,
    names: u32,
    skips: u32,
    ff_skips: u32,
    short_end: bool,
}

impl Scan {
    /// The colour clocks from the start of the reference's run of the
    /// Eagleplayer to the first write of 3.x's InitSound, or to the end of
    /// 4.0/5.0's. The check is one stretch of 68000 code with no host call, so
    /// it takes its cycles halved, rounded down (`check_cycles`); InitPlayer and
    /// InitSound up to there take the same time on every module (measured on
    /// the 12 modules the Eagleplayer plays).
    fn start(&self, v4: bool) -> u64 {
        self.check_cycles(v4) / 2 + if v4 { 8830 } else { 4697 }
    }

    /// The check's 68000 cycles, by what it read: integers fitted exactly to
    /// the reference's check times of the 12 modules, and of modified copies
    /// for the track ends on an $FFFF (16 of a first byte other than $FF, 1 of
    /// $FF); the short end was seen on one module (-92 fits it too), the end on
    /// an $FFFF after $FF on one copy (-24 fits it too).
    fn check_cycles(&self, v4: bool) -> u64 {
        let c = 149 + if v4 { 20 } else { 0 } + 155 * self.text as i64 + 137 * self.zero as i64
            + 1170 * self.names as i64
            - 41 * self.skips as i64
            - 25 * self.ff_skips as i64
            - if self.short_end { 93 } else { 0 };
        c.max(0) as u64
    }
}

/// NostalgicPlayer's test: the tracks' end mark, and the version word matching
/// the sample command list (none in 3.x).
fn np_check(d: &[u8]) -> bool {
    let off = be32(d, 16);
    if d.len() < 576 || off >= 0x8000 || off & 1 != 0 || off as usize + 64 >= d.len() {
        return false;
    }
    let o = off as usize + 62;
    if be16(d, o) != 0xffff || be32(d, o + 2) != 0x400 {
        return false;
    }
    let (cmds, version) = (be32(d, 28), be16(d, 32));
    (version == 2 && cmds == 0) || (version == 3 && cmds != 0)
}

fn detect(d: &[u8]) -> bool {
    np_check(d) && ep_check(d).is_some()
}

/// Code paths of the original between its host calls. Each register write is a
/// host call (ENPP_PokeAdr, ENPP_PokeLen, ...), which the engine runs at the
/// 68000 cycles the code took since the previous one (or since the tick's
/// start), halved and rounded down; the engine charges a fixed figure per
/// instruction (crates/m68k-rc/src/cycles.rs), so a path's time depends only on
/// the branches it takes, and each branch of the port is one of these. Paula
/// stops a playing voice only at the end of its word, so the time from a note's
/// DMA off to its DMA on decides whether the note starts, and the DMA slots of a
/// line make the place of a write in its line matter.
#[derive(Clone, Copy, Debug)]
enum At {
    /// The interrupt up to the replayer.
    Tick,
    // 3.x
    /// The fourth tick's second ProcessCounter; a ProcessCounter call, and one
    /// that reads the tracks.
    Count3Fourth,
    Counter3,
    Rows3,
    /// The sixth track: at its end, waiting, a row, a transposing row (to every
    /// voice: and the four).
    Six3End,
    Six3Wait,
    Six3Row,
    Six3Trsp,
    Six3All,
    /// A voice: waiting, a new row (no note), a note to its sample, each step of
    /// its transpose (down or up, and across an octave), a rest (sample $FF), a
    /// note of volume $80. UNSURE: the original does not just drop a note of
    /// volume $80 (on modified copies it takes time on later ticks and leaves
    /// out later notes); no module has one.
    Wait3,
    Row3,
    Note3,
    Trsp3Down,
    Trsp3DownWrap,
    Trsp3Up,
    Trsp3UpWrap,
    Rest3,
    Hold3,
    /// The sample: each step of its note transpose (and across an octave), to
    /// its LOC; between its writes; after the DMA on (with a loop, and between
    /// its two writes); the rest of the voice.
    STrsp3Any,
    STrsp3,
    STrsp3Wrap,
    Smp3,
    LocLen3,
    LenPer3,
    PerOff3,
    OffVol3,
    VolPer3,
    PerLen3,
    LenLoc3,
    LocOn3,
    NoLoop3,
    Loop3,
    LoopLoc3,
    LoopEnd3,
    SmpEnd3,
    /// The next position, and the song's end (to the host's SongEnd, and
    /// after it); the new position's tracks.
    Pos3,
    Pos3End,
    Pos3EndTail,
    Pos3Tracks,
    // 4.0/5.0
    /// The speed counter, each step of it, and a step that reads the tracks.
    Counter4,
    Step4,
    Step4Track,
    /// The sixth track: waiting, a row, a row going back, a transposing row (to
    /// every voice).
    Six4Wait,
    Six4Row,
    Six4Back,
    Six4Trsp,
    Six4All,
    /// A voice's track: waiting, a row without a note, a note on the muted
    /// voice, a note.
    Wait4,
    Row4,
    Row4Muted,
    Note4,
    /// The next position, from a note of $FF or an instrument of $FF; the
    /// position list's end (to the host's SongEnd, and after it); the new
    /// position's tracks.
    Pos4Note,
    Pos4Instr,
    Pos4,
    Pos4End,
    Pos4EndTail,
    Pos4Tracks,
    /// The sample commands: a voice with none, a waiting voice, one command,
    /// either argument taken from the repeat values, and each command.
    Cmds4,
    CmdsNone4,
    CmdWait4,
    Cmd4,
    CmdArg5,
    CmdArg6,
    CmdStop4,
    CmdSwitch4,
    CmdWaitSet4,
    CmdAdr3,
    CmdAdr4,
    CmdLen5,
    CmdLen6,
    CmdPer7,
    CmdPer8,
    /// After the write of each of these.
    CmdAdr3Tail,
    CmdAdr4Tail,
    CmdLen5Tail,
    CmdLen6Tail,
    CmdPer7Tail,
    CmdPer8Tail,
    /// The volume command, and its clamps below 0 and at $40.
    CmdVol9,
    CmdVolNeg,
    CmdVolMax,
    CmdSet10,
    /// The repeat command, counting down, going back or not.
    CmdRep11,
    CmdRep11Down,
    CmdRep11Again,
    CmdRep11Done,
    CmdValue12,
    CmdValue13,
    CmdNop4,
    /// The play sample command's three ticks: to the first write, between
    /// writes, and after the last; a phase past the three.
    Play0,
    Play0Dma,
    Play0Len,
    Play0Per,
    Play0Vol,
    PlayEnd0,
    Play1,
    Play1Per,
    Play1Len,
    Play1On,
    PlayEnd1,
    Play2,
    Play2NoLoop,
    Play2NoLoopEnd,
    Play2Loop,
    Play2Loc,
    Play2End,
    PlayNone,
    /// The volumes: the envelope counting, or a step of its attack (reaching
    /// the top), decay (reaching the level), sustain (released), release or
    /// end; the volume write, or a voice not written.
    EnvCount4,
    EnvAttack4,
    EnvAttackTop,
    EnvDecay4,
    EnvDecayLevel,
    EnvSustain4,
    EnvSustainNext,
    EnvRelease4,
    EnvDone4,
    Vol4,
    VolSkip4,
}

impl At {
    /// 68000 cycles, fitted as integers to the reference's host-call gaps
    /// (`SOUNDCONTROL_TRACE` against the reference's DELI_TRACE): each gap is
    /// the sum along its path, halved and rounded down. The fit fixes only the
    /// sums, so where a cost could move to its neighbour one split was chosen
    /// (the nearest to an earlier fit). UNSURE: the paths no module or copy took
    /// (a muted voice or one the voice mask leaves out, a play phase past the
    /// three, the envelope's release and end: nothing sets them), and the note of
    /// volume $80.
    fn cycles(self) -> u64 {
        use At::*;
        match self {
            Tick => 226,
            Count3Fourth => 0,
            Counter3 => 103,
            Rows3 => 323,
            Six3End => 10,
            Six3Wait => 49,
            Six3Row => 114,
            Six3Trsp => 94,
            Six3All => 44,
            Wait3 => 101,
            Row3 => 183,
            Note3 => 25,
            Trsp3Down => 59,
            Trsp3DownWrap => 14,
            Trsp3Up => 59,
            Trsp3UpWrap => 14,
            Rest3 => 18,
            Hold3 => 183,
            STrsp3Any => 16,
            STrsp3 => 43,
            STrsp3Wrap => 4,
            Smp3 => 668,
            LocLen3 => 198,
            LenPer3 => 170,
            PerOff3 => 176,
            OffVol3 => 174,
            VolPer3 => 174,
            PerLen3 => 210,
            LenLoc3 => 216,
            LocOn3 => 200,
            NoLoop3 => 218,
            Loop3 => 250,
            LoopLoc3 => 224,
            LoopEnd3 => 8,
            SmpEnd3 => 422,
            Pos3 => 173,
            Pos3End => 0,
            Pos3EndTail => 155,
            Pos3Tracks => 1135,
            Counter4 => 307,
            Step4 => 157,
            Step4Track => 0,
            Six4Wait => 204,
            Six4Row => 259,
            Six4Back => 49,
            Six4Trsp => 119,
            Six4All => 28,
            Wait4 => 104,
            Row4 => 172,
            Row4Muted => 172,
            Note4 => 915,
            Pos4Note => 0,
            Pos4Instr => 83,
            Pos4 => 69,
            Pos4End => 0,
            Pos4EndTail => 74,
            Pos4Tracks => 1083,
            Cmds4 => 365,
            CmdsNone4 => 45,
            CmdWait4 => 102,
            Cmd4 => 163,
            CmdArg5 => 22,
            CmdArg6 => 22,
            CmdStop4 => 34,
            CmdSwitch4 => 198,
            CmdWaitSet4 => 22,
            CmdAdr3 => 39,
            CmdAdr4 => 81,
            CmdLen5 => 24,
            CmdLen6 => 98,
            CmdPer7 => 0,
            CmdPer8 => 58,
            CmdAdr3Tail => 232,
            CmdAdr4Tail => 248,
            CmdLen5Tail => 224,
            CmdLen6Tail => 224,
            CmdPer7Tail => 236,
            CmdPer8Tail => 224,
            CmdVol9 => 68,
            CmdVolNeg => 4,
            CmdVolMax => 4,
            CmdSet10 => 104,
            CmdRep11 => 120,
            CmdRep11Down => 0,
            CmdRep11Again => 0,
            CmdRep11Done => 20,
            CmdValue12 => 40,
            CmdValue13 => 36,
            CmdNop4 => 10,
            Play0 => 81,
            Play0Dma => 193,
            Play0Len => 214,
            Play0Per => 170,
            Play0Vol => 166,
            PlayEnd0 => 248,
            Play1 => 48,
            Play1Per => 190,
            Play1Len => 210,
            Play1On => 224,
            PlayEnd1 => 264,
            Play2 => 115,
            Play2NoLoop => 0,
            Play2NoLoopEnd => 224,
            Play2Loop => 4,
            Play2Loc => 236,
            Play2End => 232,
            PlayNone => 0,
            EnvCount4 => 168,
            EnvAttack4 => 337,
            EnvAttackTop => 32,
            EnvDecay4 => 349,
            EnvDecayLevel => 28,
            EnvSustain4 => 277,
            EnvSustainNext => 28,
            EnvRelease4 => 349,
            EnvDone4 => 277,
            Vol4 => 310,
            VolSkip4 => 0,
        }
    }
}

/// The trace of code paths for fitting their cycles (SOUNDCONTROL_TRACE=file):
/// each host call's clock and the paths since the previous one.
#[derive(Default)]
struct Trace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    pend: Vec<At>,
}

impl Trace {
    fn open() -> Self {
        let out = std::env::var_os("SOUNDCONTROL_TRACE")
            .and_then(|p| std::fs::File::create(p).ok())
            .map(std::io::BufWriter::new);
        Trace { out, pend: Vec::new() }
    }

    /// A register write is about to be made (or a tick starts).
    fn write(&mut self, hw: &Hw, what: &str) {
        if let Some(o) = &mut self.out {
            use std::io::Write;
            let _ = write!(o, "{} {}", hw.bus.clock, what);
            for e in self.pend.drain(..) {
                let _ = write!(o, " {:?}", e);
            }
            let _ = writeln!(o);
        }
    }
}

pub struct SoundControl {
    /// 4.0/5.0 (the sample command list's length is not 0).
    v4: bool,
    v40: bool,
    /// The original's two hunks in guest memory.
    h0: u32,
    h1: u32,
    songend: bool,
    /// The clock of the last host call's return (or the tick's start), and the
    /// 68000 cycles spent since.
    seg: u64,
    cyc: u64,
    trace: Trace,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let h0 = m.alloc(H0_SIZE);
    let h1 = m.alloc(H1_SIZE);
    if h0 == 0 || h1 == 0 {
        return None;
    }
    let v4 = be32(d, 0x1c) != 0;
    let total = (be32(d, 16) as u64 + be32(d, 20) as u64 + be32(d, 24) as u64 + be32(d, 28) as u64 + 64) as u32;
    let mut p = SoundControl {
        v4,
        v40: v4 && LENGTHS40.contains(&total),
        h0,
        h1,
        songend: false,
        seg: 0,
        cyc: 0,
        trace: Trace::open(),
    };
    // the initialised data of the original's first hunk
    let b = &mut m.bus;
    for (i, &w) in BASE_PERIOD.iter().enumerate() {
        b.w16(p.at(BASE3) + 2 * i as u32, w);
        b.w16(p.at(BASE4) + 2 * i as u32, w);
    }
    for (a, w) in [(SPEED_MAX, 0x32), (SPEED4, 0x6d), (OFF4, 0xffff), (MUTED4, 0xffff), (0x10eba, 0xffff),
        (0x10ebc, 0xffff), (MASTER4, 0x40), (VOICE_MASK4, 0xf)]
    {
        b.w16(p.at(a), w);
    }
    b.w32(p.at(PERIODS4), h1 + 8);
    // InitPlayer: the module
    b.w32(p.at(MODULE), base);
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(ep_check(d)?.start(v4));
    if v4 {
        p.init_sound4(&mut hw);
    } else {
        p.init_sound3(&mut hw);
    }
    Some(Box::new(p))
}

impl Replayer for SoundControl {
    fn format(&self) -> &'static str {
        match (self.v4, self.v40) {
            (false, _) => "SoundControl 3.x",
            (true, true) => "SoundControl 4.0",
            (true, false) => "SoundControl 5.0",
        }
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.seg = hw.bus.clock;
        self.cyc = 0;
        self.trace.write(hw, "tick");
        self.go(At::Tick);
        if self.v4 {
            self.play4(hw);
        } else {
            self.play3(hw);
        }
    }
}

impl SoundControl {
    /// The guest address of the original's variable at `a`.
    fn at(&self, a: u32) -> u32 {
        self.h0 + (a - H0)
    }

    fn r8(&self, hw: &Hw, a: u32) -> u8 {
        hw.bus.r8(self.at(a))
    }
    fn r16(&self, hw: &Hw, a: u32) -> u16 {
        hw.bus.r16(self.at(a))
    }
    fn r32(&self, hw: &Hw, a: u32) -> u32 {
        hw.bus.r32(self.at(a))
    }
    fn w8(&self, hw: &mut Hw, a: u32, v: u8) {
        hw.bus.w8(self.at(a), v)
    }
    fn w16(&self, hw: &mut Hw, a: u32, v: u16) {
        hw.bus.w16(self.at(a), v)
    }
    fn w32(&self, hw: &mut Hw, a: u32, v: u32) {
        hw.bus.w32(self.at(a), v)
    }

    /// The original's code takes path `e`.
    fn go(&mut self, e: At) {
        self.cyc += e.cycles();
        if self.trace.out.is_some() {
            self.trace.pend.push(e);
        }
    }

    /// A host call: it runs at the clock the cycles since the last one reach.
    fn host(&mut self, hw: &mut Hw, what: &str) {
        hw.bus.clock = hw.bus.clock.max(self.seg + self.cyc / 2);
        self.trace.write(hw, what);
    }

    /// The code goes on from the host call's return.
    fn rebase(&mut self, hw: &Hw) {
        self.seg = hw.bus.clock;
        self.cyc = 0;
    }

    // The host's ENPP_Poke* and ENPP_DMAMask, as the original's wrappers call
    // them: an address of 0 is the second hunk's zeros; DMACON takes bit 15 and
    // the low 4 bits of the mask; with voices on the host waits for the DMA as
    // dtg_WaitAudioDMA does, to 10 lines past the current one.
    fn poke_adr(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.host(hw, "loc");
        hw.loc(ch, if a == 0 { self.h1 } else { a });
        self.rebase(hw);
    }
    fn poke_len(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.host(hw, "len");
        hw.len(ch, v);
        self.rebase(hw);
    }
    fn poke_per(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.host(hw, "per");
        hw.per(ch, v);
        self.rebase(hw);
    }
    fn poke_vol(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.host(hw, "vol");
        hw.vol(ch, v);
        self.rebase(hw);
    }
    /// ENPP_SongEnd.
    fn song_end(&mut self, hw: &mut Hw) {
        self.host(hw, "end");
        self.songend = true;
        self.rebase(hw);
    }
    fn dma_mask(&mut self, hw: &mut Hw, v: u16) {
        self.host(hw, "dma");
        hw.dmacon(v & 0x800f);
        if v & 0x8000 != 0 {
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
        }
        self.rebase(hw);
    }

    // ---------------------------------------------------------------- 3.x

    fn init_sound3(&mut self, hw: &mut Hw) {
        for ch in 0..4 {
            if ch != 0 {
                hw.wait(113);
            }
            self.poke_adr(hw, ch, 0);
            hw.wait(85);
            self.poke_vol(hw, ch, 0);
            hw.wait(95);
            self.poke_len(hw, ch, 1);
            hw.wait(85);
            self.poke_per(hw, ch, 1);
        }
        for a in START_POS..CLEAR3_END {
            self.w8(hw, a, 0);
        }
        let m = self.r32(hw, MODULE);
        self.w32(hw, TRACK_TABLE3, m + 0x40);
        self.w32(hw, TRACK_TABLE3 + 4, m + 0x240);
        for oct in 2..10 {
            for j in 0..16 {
                let p = self.r16(hw, BASE3 + 2 * j) >> oct;
                self.w16(hw, PERIODS3 + 2 * (16 * (oct - 2) + j), p);
            }
        }
        let samples = m.wrapping_add(hw.bus.r32(m + 0x10)).wrapping_add(0x40);
        self.w32(hw, SAMPLES3, samples);
        self.w32(hw, SAMPLES3 + 4, samples.wrapping_add(0x400));
        let positions = samples.wrapping_add(hw.bus.r32(m + 0x14));
        self.w32(hw, POSITIONS3, positions);
        hw.bus.w8(positions, 1);
        let start = self.r16(hw, START_POS);
        self.w16(hw, POS3, start);
        self.set_tracks3(hw);
        // the clearing, the tables and the tracks; then the return to the host,
        // which starts the timer (measured)
        hw.wait(17830);
        self.dma_mask(hw, 0x800f);
        hw.wait(42);
    }

    fn play3(&mut self, hw: &mut Hw) {
        let c = self.r16(hw, COUNTER1).wrapping_add(1);
        self.w16(hw, COUNTER1, c);
        if c == 4 {
            self.go(At::Count3Fourth);
            self.counter3(hw);
            self.w16(hw, COUNTER1, 0);
        }
        self.counter3(hw);
    }

    fn counter3(&mut self, hw: &mut Hw) {
        self.go(At::Counter3);
        let c = self.r16(hw, COUNTER2).wrapping_add(1);
        self.w16(hw, COUNTER2, c);
        if c == 2 {
            self.go(At::Rows3);
            self.process_track3(hw);
            self.w16(hw, COUNTER2, 0);
        }
    }

    fn set_tracks3(&mut self, hw: &mut Hw) {
        for i in 0..6 {
            self.w16(hw, WAITS3 + 2 * i, 0);
        }
        let pos = self.r16(hw, POS3);
        let (pl, tt) = (self.r32(hw, POSITIONS3), self.r32(hw, TRACK_TABLE3));
        for i in 0..6u16 {
            let t = hw.bus.r8(pl.wrapping_add(sx(pos.wrapping_add(2 * i)))) as u32;
            let off = hw.bus.r16(tt.wrapping_add(2 * t)) as u32;
            self.w32(hw, TRACKS3 + 4 * i as u32, tt.wrapping_add(off).wrapping_add(16));
        }
    }

    /// A track's row: its wait counter takes the row's length (less one, as the
    /// counter is decremented now); returns the row, or None while waiting.
    fn step(&mut self, hw: &mut Hw, track: u32, wait: u32) -> Option<u32> {
        let w = self.r16(hw, wait);
        if w != 0 {
            self.w16(hw, wait, w - 1);
            return None;
        }
        let row = self.r32(hw, track);
        let len = hw.bus.r8(row.wrapping_add(1));
        self.w16(hw, wait, (len as u16).wrapping_sub(1));
        self.w32(hw, track, row.wrapping_add(4));
        Some(row)
    }

    fn process_track3(&mut self, hw: &mut Hw) {
        'redo: for _ in 0..MAX_LOOP {
            // the sixth track: the voices' transposes
            let row = self.r32(hw, TRACKS3 + 20);
            if hw.bus.r8(row) == 0xff {
                self.go(At::Six3End);
            } else if let Some(row) = self.step(hw, TRACKS3 + 20, WAITS3 + 10) {
                self.go(At::Six3Row);
                if hw.bus.r8(row) != 0 {
                    self.go(At::Six3Trsp);
                    let t = hw.bus.r8(row.wrapping_add(3)) & 0x3f;
                    let v = hw.bus.r8(row.wrapping_add(2)).wrapping_sub(1);
                    if v == 4 {
                        self.go(At::Six3All);
                        for i in 0..4 {
                            self.w8(hw, TRANSPOSE3 + i, t);
                        }
                    }
                    self.w8(hw, TRANSPOSE3 + (v & 3) as u32, t);
                }
            } else {
                self.go(At::Six3Wait);
            }

            for i in 0..4usize {
                let (track, wait) = (TRACKS3 + 4 * i as u32, WAITS3 + 2 * i as u32);
                let Some(row) = self.step(hw, track, wait) else {
                    self.go(At::Wait3);
                    continue;
                };
                let note = hw.bus.r8(row);
                if note == 0xff {
                    self.next_pos3(hw);
                    continue 'redo;
                }
                if note == 0 {
                    self.go(At::Row3);
                    continue;
                }
                self.go(At::Note3);
                // the transpose, less one, a semitone at a time (the low nibble
                // is the note, 0-11)
                let mut note = note as u16;
                let mut t = self.r8(hw, TRANSPOSE3 + i as u32).wrapping_add(0xff);
                while t != 0 {
                    if (t as i8) < 0 {
                        self.go(At::Trsp3Down);
                        t = t.wrapping_add(1);
                        note = note.wrapping_sub(1);
                        if note & 15 == 15 {
                            self.go(At::Trsp3DownWrap);
                            note = note.wrapping_sub(4);
                        }
                    } else {
                        self.go(At::Trsp3Up);
                        t -= 1;
                        note = note.wrapping_add(1);
                        if note & 15 == 12 {
                            self.go(At::Trsp3UpWrap);
                            note = note.wrapping_add(4);
                        }
                    }
                }
                let (mut sample, mut vol) = (hw.bus.r8(row.wrapping_add(2)) as u32, hw.bus.r8(row.wrapping_add(3)));
                if sample == 0xff {
                    self.go(At::Rest3);
                    sample = 1;
                    vol = 0;
                    self.w16(hw, wait, 0);
                }
                if vol == 0x80 {
                    // the original keeps the note in a list it never plays
                    self.go(At::Hold3);
                    continue;
                }
                let st = self.r32(hw, SAMPLES3);
                let header = st.wrapping_add(hw.bus.r32(st.wrapping_add(sample << 2)));
                self.play_sample3(hw, i, header, note, vol as u16);
            }
            return;
        }
    }

    fn next_pos3(&mut self, hw: &mut Hw) {
        self.go(At::Pos3);
        let pos = self.r16(hw, POS3).wrapping_add(12);
        self.w16(hw, POS3, pos);
        let pl = self.r32(hw, POSITIONS3).wrapping_add(sx(pos));
        if hw.bus.r8(pl.wrapping_add(1)) != 1 && hw.bus.r8(pl.wrapping_add(3)) != 1 {
            self.go(At::Pos3End);
            self.song_end(hw);
            self.go(At::Pos3EndTail);
            let start = self.r16(hw, START_POS);
            self.w16(hw, POS3, start);
        }
        self.go(At::Pos3Tracks);
        self.set_tracks3(hw);
    }

    /// Plays the sample whose header is at `h`: its name, length, loop start and
    /// end at $10, the note transpose at $2A, the data at $40.
    fn play_sample3(&mut self, hw: &mut Hw, ch: usize, h: u32, mut note: u16, vol: u16) {
        let bit = 1 << ch;
        let nt = hw.bus.r16(h.wrapping_add(0x2a));
        if nt != 0 {
            self.go(At::STrsp3Any);
        }
        for _ in 0..nt {
            self.go(At::STrsp3);
            note = note.wrapping_add(1);
            if note & 15 == 12 {
                self.go(At::STrsp3Wrap);
                note = note.wrapping_add(4);
            }
        }
        let per = self.r16(hw, PERIODS3 + 2 * (note & 0xff) as u32);
        let data = h.wrapping_add(0x40);
        self.go(At::Smp3);
        self.poke_adr(hw, ch, data);
        self.go(At::LocLen3);
        self.poke_len(hw, ch, 1);
        self.go(At::LenPer3);
        self.poke_per(hw, ch, 1);
        self.go(At::PerOff3);
        self.dma_mask(hw, bit);
        self.go(At::OffVol3);
        self.poke_vol(hw, ch, vol);
        self.go(At::VolPer3);
        self.poke_per(hw, ch, per);
        self.go(At::PerLen3);
        let len = hw.bus.r16(h.wrapping_add(0x10)) >> 1;
        self.poke_len(hw, ch, len);
        self.go(At::LenLoc3);
        hw.bus.w32(data, 0);
        self.poke_adr(hw, ch, data);
        self.go(At::LocOn3);
        self.dma_mask(hw, 0x8200 | bit);
        let start = hw.bus.r16(h.wrapping_add(0x12));
        if start == 0 {
            self.go(At::NoLoop3);
            self.poke_len(hw, ch, 1);
        } else {
            self.go(At::Loop3);
            let end = hw.bus.r16(h.wrapping_add(0x14));
            self.poke_len(hw, ch, end.wrapping_sub(start) >> 1);
            self.go(At::LoopLoc3);
            self.poke_adr(hw, ch, data.wrapping_add(sx(start)));
            self.go(At::LoopEnd3);
        }
        self.go(At::SmpEnd3);
    }

    // ---------------------------------------------------------------- 4.0/5.0

    fn init_sound4(&mut self, hw: &mut Hw) {
        let m = self.r32(hw, MODULE);
        self.w32(hw, MODULE4, m);
        if m == 0 || self.r16(hw, HALT) != 0 {
            self.w16(hw, OFF4, 0);
            return;
        }
        self.w16(hw, 0x10ea2, 4);
        let t = self.r32(hw, PERIODS4);
        for oct in 2..10 {
            for j in 0..12 {
                let p = self.r16(hw, BASE4 + 2 * j) >> oct;
                hw.bus.w16(t + 2 * (12 * (oct - 2) + j), p);
            }
        }
        for v in VOICES4 {
            self.w32(hw, v + V_CMDS, 0);
        }
        let tracks = m + 0x40;
        self.w32(hw, TRACK_TABLE4, tracks);
        self.w32(hw, TRACK_TABLE4 - 4, tracks + 0x200);
        let speed = hw.bus.r16(m + 0x22);
        self.w16(hw, SPEED4, speed);
        let samples = tracks.wrapping_add(hw.bus.r32(m + 0x10));
        self.w32(hw, SAMPLES4, samples);
        self.w32(hw, SAMPLES4 - 4, samples.wrapping_add(0x400));
        let positions = samples.wrapping_add(hw.bus.r32(m + 0x14));
        self.w32(hw, POSITIONS4, positions);
        let instruments = positions.wrapping_add(hw.bus.r32(m + 0x18));
        self.w32(hw, INSTRUMENTS4, instruments);
        self.w32(hw, INSTRUMENTS4 - 4, instruments.wrapping_add(0x200));
        self.w16(hw, START_POS4, 0);
        for i in 0..4 {
            self.w16(hw, TRANSPOSE4 + 2 * i, 0);
        }
        self.w16(hw, POS4, 0);
        // a long from $1A: the position list's length (its low word), and the
        // sample commands' high word over the 4 above
        let end = hw.bus.r32(m + 0x1a);
        self.w32(hw, END_POS4, end);
        self.set_tracks4(hw);
        self.w16(hw, OFF4, 0);
    }

    fn play4(&mut self, hw: &mut Hw) {
        if self.r16(hw, OFF4) != 0 {
            return;
        }
        self.counter4(hw);
        if self.r16(hw, HALT) != 0 {
            return;
        }
        for i in 0..4 {
            self.commands4(hw, i);
        }
        self.volumes4(hw);
    }

    fn counter4(&mut self, hw: &mut Hw) {
        self.go(At::Counter4);
        let speed = self.r16(hw, SPEED4);
        let acc = self.r16(hw, SPEED_ACC).wrapping_add(speed);
        self.w16(hw, SPEED_ACC, acc);
        for _ in 0..MAX_LOOP {
            let max = if self.r16(hw, SPEED_MAX) == 0x32 { 0x2e } else { 0x38 };
            let acc = self.r16(hw, SPEED_ACC);
            if max >= acc {
                return;
            }
            self.go(At::Step4);
            self.w16(hw, SPEED_ACC, acc - max);
            let s = self.r16(hw, STEPS).wrapping_add(1);
            self.w16(hw, STEPS, s);
            let s = self.r16(hw, STEPS4).wrapping_add(1);
            self.w16(hw, STEPS4, s);
            if s & 3 == 0 && self.r16(hw, HALT) == 0 {
                self.go(At::Step4Track);
                self.process_track4(hw);
            }
        }
    }

    fn set_tracks4(&mut self, hw: &mut Hw) {
        for i in 0..6 {
            self.w16(hw, WAITS4 + 2 * i, 0);
        }
        let pos = self.r16(hw, POS4);
        let (pl, tt) = (self.r32(hw, POSITIONS4), self.r32(hw, TRACK_TABLE4));
        for i in 0..6u16 {
            let t = hw.bus.r8(pl.wrapping_add(sx(pos.wrapping_add(2 * i)))) as u32;
            let off = hw.bus.r16(tt.wrapping_add(2 * t)) as u32;
            self.w32(hw, TRACKS4 + 4 * i as u32, tt.wrapping_add(off).wrapping_add(16));
        }
    }

    fn process_track4(&mut self, hw: &mut Hw) {
        'redo: for _ in 0..MAX_LOOP {
            self.six4(hw);
            for i in 0..4usize {
                let (track, wait) = (TRACKS4 + 4 * i as u32, WAITS4 + 2 * i as u32);
                let Some(row) = self.step(hw, track, wait) else {
                    self.go(At::Wait4);
                    continue;
                };
                let note = hw.bus.r8(row);
                let instr = hw.bus.r8(row.wrapping_add(2));
                if note == 0xff || (note != 0 && instr == 0xff) {
                    self.go(if note == 0xff { At::Pos4Note } else { At::Pos4Instr });
                    self.next_pos4(hw);
                    continue 'redo;
                }
                if note == 0 {
                    self.go(At::Row4);
                    continue;
                }
                if i as u16 == self.r16(hw, MUTED4) {
                    self.go(At::Row4Muted);
                    continue;
                }
                self.go(At::Note4);
                let vol = hw.bus.r8(row.wrapping_add(3));
                self.note4(hw, i, note as u16, instr as u16, vol as u16);
            }
            return;
        }
    }

    /// The sixth track: the voices' transposes.
    fn six4(&mut self, hw: &mut Hw) {
        let Some(row) = self.step(hw, TRACKS4 + 20, WAITS4 + 10) else {
            self.go(At::Six4Wait);
            return;
        };
        self.go(At::Six4Row);
        match hw.bus.r8(row) {
            0 => {}
            0xff => {
                self.go(At::Six4Back);
                let p = self.r32(hw, TRACKS4 + 20);
                self.w32(hw, TRACKS4 + 20, p.wrapping_sub(4));
            }
            _ => {
                self.go(At::Six4Trsp);
                let t = (hw.bus.r8(row.wrapping_add(3)) & 0x3f) as u16;
                let v = hw.bus.r8(row.wrapping_add(2)).wrapping_sub(1);
                if v == 4 {
                    self.go(At::Six4All);
                    for i in 0..4 {
                        self.w16(hw, TRANSPOSE4 + 2 * i, t);
                    }
                } else {
                    self.w16(hw, TRANSPOSE4 + 2 * (v & 3) as u32, t);
                }
            }
        }
    }

    fn next_pos4(&mut self, hw: &mut Hw) {
        self.go(At::Pos4);
        let pos = self.r16(hw, POS4).wrapping_add(12);
        self.w16(hw, POS4, pos);
        if pos == self.r16(hw, END_POS4) {
            self.go(At::Pos4End);
            self.song_end(hw);
            self.go(At::Pos4EndTail);
            let start = self.r16(hw, START_POS4);
            self.w16(hw, POS4, start);
        }
        self.go(At::Pos4Tracks);
        self.set_tracks4(hw);
    }

    /// A note: the instrument's sample commands start, and its envelope.
    fn note4(&mut self, hw: &mut Hw, i: usize, note: u16, instr: u16, vol: u16) {
        let v = VOICES4[i];
        let it = self.r32(hw, INSTRUMENTS4);
        let off = hw.bus.r16(it.wrapping_add(2 * instr as u32));
        self.w16(hw, v + V_INSTR, instr);
        self.w32(hw, v + V_CMDS, it.wrapping_add(sx(off)).wrapping_add(0x30));
        self.w16(hw, v + V_VOLUME, vol);
        self.w16(hw, v + V_PHASE, 0);
        self.w16(hw, v + V_WAIT, 0);
        self.w32(hw, v + V_STACK_PTR, self.at(v + V_STACK));
        let oct = (note & 0xf0) >> 2;
        let n = (note & 15)
            .wrapping_add(oct * 3)
            .wrapping_add(self.r16(hw, TRANSPOSE4 + 2 * i as u32))
            .wrapping_add(self.r16(hw, GLOBAL_TRANSPOSE4));
        self.w16(hw, v + V_NOTE, n);
        self.w16(hw, v + V_ENV_VOLUME, 0);
        self.w16(hw, v + V_ENV_COUNTER, 1);
        self.w16(hw, v + V_ENV, 0);
        self.w16(hw, v + V_RELEASE, 0);
    }

    /// Sample `n`'s header (its offset from the sample table, as a word index).
    fn sample4(&self, hw: &Hw, n: u16) -> u32 {
        let st = self.r32(hw, SAMPLES4);
        st.wrapping_add(hw.bus.r32(st.wrapping_add(sx(n << 2))))
    }

    /// Voice `ch`'s sample commands, up to a wait.
    fn commands4(&mut self, hw: &mut Hw, ch: usize) {
        self.go(At::Cmds4);
        let v = self.at(VOICES4[ch]);
        let b = |o: u32| v + o;
        let mut a0 = hw.bus.r32(b(V_CMDS));
        if a0 == 0 {
            self.go(At::CmdsNone4);
            return;
        }
        // the DMA mask is the original's d4: the voice's bit until a switch
        // sample command leaves twice its note there
        let mut d4: u16 = 1 << ch;
        let periods = self.r32(hw, PERIODS4);
        for _ in 0..MAX_LOOP {
            let w = hw.bus.r16(b(V_WAIT));
            if w != 0 {
                self.go(At::CmdWait4);
                hw.bus.w16(b(V_WAIT), w - 1);
                hw.bus.w32(b(V_CMDS), a0);
                return;
            }
            self.go(At::Cmd4);
            let mut a1 = a0;
            let cmd = hw.bus.r16(a0);
            let mut d5 = hw.bus.r16(a0.wrapping_add(2));
            let mut d6 = hw.bus.r16(a0.wrapping_add(4));
            let values = b(V_VALUES);
            let value = |n: u16| values.wrapping_add(sx(n.wrapping_add(n)));
            if cmd & 0x4000 != 0 {
                self.go(At::CmdArg5);
                d5 = hw.bus.r16(value(d5));
            }
            if cmd & 0x8000 != 0 {
                self.go(At::CmdArg6);
                d6 = hw.bus.r16(value(d6));
            }
            match cmd & 0x1f {
                // stop: this command again next tick
                0 => {
                    self.go(At::CmdStop4);
                    a1 = a1.wrapping_sub(6);
                    hw.bus.w16(b(V_WAIT), 1);
                }
                // switch sample: its length, note and period
                1 => {
                    self.go(At::CmdSwitch4);
                    hw.bus.w16(b(V_SAMPLE_NR), d5);
                    let s = self.sample4(hw, d5);
                    hw.bus.w32(b(V_SAMPLE), s);
                    hw.bus.w16(b(V_LENGTH), hw.bus.r16(s.wrapping_add(0x10)));
                    d4 = hw.bus.r16(b(V_NOTE)).wrapping_add(hw.bus.r16(s.wrapping_add(0x2a)));
                    hw.bus.w16(b(V_SAMPLE_NOTE), d4);
                    d4 = d4.wrapping_add(d4);
                    hw.bus.w16(b(V_PERIOD), hw.bus.r16(periods.wrapping_add(sx(d4))));
                    hw.bus.w32(b(V_DATA), s.wrapping_add(0x40));
                }
                2 => {
                    self.go(At::CmdWaitSet4);
                    hw.bus.w16(b(V_WAIT), d5);
                }
                // change address: in the sample, at once
                3 => {
                    self.go(At::CmdAdr3);
                    let a = hw.bus.r32(b(V_SAMPLE)).wrapping_add(0x40).wrapping_add(sx(d5));
                    self.poke_adr(hw, ch, a);
                    self.go(At::CmdAdr3Tail);
                }
                4 => {
                    self.go(At::CmdAdr4);
                    hw.bus.w16(b(V_SAMPLE_NR), d5);
                    let s = self.sample4(hw, d5);
                    hw.bus.w32(b(V_SAMPLE), s);
                    let a = s.wrapping_add(0x40);
                    self.poke_adr(hw, ch, a);
                    self.go(At::CmdAdr4Tail);
                    hw.bus.w32(b(V_DATA), a);
                }
                5 => {
                    self.go(At::CmdLen5);
                    hw.bus.w16(b(V_LENGTH), d5);
                    self.poke_len(hw, ch, d5 >> 1);
                    self.go(At::CmdLen5Tail);
                }
                6 => {
                    self.go(At::CmdLen6);
                    hw.bus.w16(b(V_SAMPLE_NR), d5);
                    let s = self.sample4(hw, d5);
                    hw.bus.w32(b(V_SAMPLE), s);
                    let l = hw.bus.r16(s.wrapping_add(0x10));
                    hw.bus.w16(b(V_LENGTH), l);
                    self.poke_len(hw, ch, l >> 1);
                    self.go(At::CmdLen6Tail);
                }
                7 => {
                    self.go(At::CmdPer7);
                    let p = hw.bus.r16(b(V_PERIOD)).wrapping_add(d5);
                    self.poke_per(hw, ch, p);
                    self.go(At::CmdPer7Tail);
                    hw.bus.w16(b(V_PERIOD), p);
                }
                8 => {
                    self.go(At::CmdPer8);
                    let n = hw.bus.r16(b(V_SAMPLE_NOTE)).wrapping_add(d5);
                    hw.bus.w16(b(V_SAMPLE_NOTE), n);
                    let p = hw.bus.r16(periods.wrapping_add(sx(n.wrapping_add(n))));
                    hw.bus.w16(b(V_PERIOD), p);
                    self.poke_per(hw, ch, p);
                    self.go(At::CmdPer8Tail);
                }
                9 => {
                    self.go(At::CmdVol9);
                    let mut vol = hw.bus.r16(b(V_VOLUME)).wrapping_add(d5);
                    if (vol as i16) < 0 {
                        self.go(At::CmdVolNeg);
                        vol = 0;
                    }
                    if vol >= 0x40 {
                        self.go(At::CmdVolMax);
                        vol = 0x40;
                    }
                    hw.bus.w16(b(V_VOLUME), vol);
                }
                // set repeat: push this command, set its value
                10 => {
                    self.go(At::CmdSet10);
                    let sp = hw.bus.r32(b(V_STACK_PTR)).wrapping_add(4);
                    hw.bus.w32(sp, a1);
                    hw.bus.w32(b(V_STACK_PTR), sp);
                    hw.bus.w16(value(d5), d6);
                }
                // repeat: step the set command's value (by its raw argument) and
                // go back after it until the value passes d5
                11 => {
                    self.go(At::CmdRep11);
                    let sp = hw.bus.r32(b(V_STACK_PTR));
                    let set = hw.bus.r32(sp);
                    let at = value(hw.bus.r16(set.wrapping_add(2)));
                    let n = hw.bus.r16(at).wrapping_add(d6);
                    hw.bus.w16(at, n);
                    let again = if (d6 as i16) < 0 {
                        self.go(At::CmdRep11Down);
                        d5 < n
                    } else {
                        d5 >= n
                    };
                    if again {
                        self.go(At::CmdRep11Again);
                        a1 = set;
                    } else {
                        self.go(At::CmdRep11Done);
                        hw.bus.w32(b(V_STACK_PTR), sp.wrapping_sub(4));
                    }
                }
                12 => {
                    self.go(At::CmdValue12);
                    let n = hw.bus.r16(value(d5)).wrapping_add(d6);
                    hw.bus.w16(value(d5), n);
                }
                13 => {
                    self.go(At::CmdValue13);
                    hw.bus.w16(value(d5), d6);
                }
                15 => {
                    let ph = hw.bus.r16(b(V_PHASE));
                    hw.bus.w16(b(V_PHASE), ph.wrapping_add(4));
                    match ph {
                        // LOC, DMA on, LEN 1, PER 1, VOL 0, and again next tick
                        0 => {
                            self.go(At::Play0);
                            let a = hw.bus.r32(b(V_DATA));
                            self.poke_adr(hw, ch, a);
                            self.go(At::Play0Dma);
                            self.dma_mask(hw, d4 | 0x8200);
                            d4 &= 0xf;
                            self.go(At::Play0Len);
                            self.poke_len(hw, ch, 1);
                            self.go(At::Play0Per);
                            self.poke_per(hw, ch, 1);
                            self.go(At::Play0Vol);
                            self.poke_vol(hw, ch, 0);
                            hw.bus.w16(b(V_WAIT), 1);
                            a1 = a1.wrapping_sub(6);
                            self.go(At::PlayEnd0);
                        }
                        // DMA off, PER, LEN, the first long cleared, DMA on
                        4 => {
                            self.go(At::Play1);
                            self.dma_mask(hw, d4);
                            self.go(At::Play1Per);
                            let p = hw.bus.r16(b(V_PERIOD));
                            self.poke_per(hw, ch, p);
                            self.go(At::Play1Len);
                            let l = hw.bus.r16(b(V_LENGTH)) >> 1;
                            self.poke_len(hw, ch, l);
                            let s = hw.bus.r32(b(V_SAMPLE));
                            hw.bus.w32(s.wrapping_add(0x40), 0);
                            self.go(At::Play1On);
                            self.dma_mask(hw, d4 | 0x8200);
                            d4 &= 0xf;
                            hw.bus.w16(b(V_WAIT), 1);
                            a1 = a1.wrapping_sub(6);
                            self.go(At::PlayEnd1);
                        }
                        // the loop, and on to the next command
                        8 => {
                            self.go(At::Play2);
                            hw.bus.w16(b(V_PHASE), 0);
                            let s = hw.bus.r32(b(V_SAMPLE));
                            let end = hw.bus.r16(s.wrapping_add(0x14));
                            if end == 0 {
                                self.go(At::Play2NoLoop);
                                self.poke_len(hw, ch, 2);
                                self.go(At::Play2NoLoopEnd);
                            } else {
                                self.go(At::Play2Loop);
                                let start = hw.bus.r16(s.wrapping_add(0x12));
                                self.poke_len(hw, ch, end.wrapping_sub(start) >> 1);
                                self.go(At::Play2Loc);
                                self.poke_adr(hw, ch, s.wrapping_add(0x40).wrapping_add(sx(start)));
                                self.go(At::Play2End);
                            }
                        }
                        // past the original's table of the three
                        _ => self.go(At::PlayNone),
                    }
                }
                _ => self.go(At::CmdNop4),
            }
            a0 = a1.wrapping_add(6);
        }
    }

    /// The four volumes: the note's times the envelope's over 256.
    fn volumes4(&mut self, hw: &mut Hw) {
        let it = self.r32(hw, INSTRUMENTS4);
        for ch in 0..4usize {
            let v = self.at(VOICES4[ch]);
            let b = |o: u32| v + o;
            let c = hw.bus.r16(b(V_ENV_COUNTER));
            if c != 0 {
                self.go(At::EnvCount4);
                hw.bus.w16(b(V_ENV_COUNTER), c - 1);
            } else {
                let instr = hw.bus.r16(b(V_INSTR));
                let e = it.wrapping_add(sx(hw.bus.r16(it.wrapping_add(sx(instr.wrapping_add(instr))))));
                let env = hw.bus.r16(b(V_ENV_VOLUME));
                let phase = hw.bus.r16(b(V_ENV));
                let next = |hw: &mut Hw| hw.bus.w16(b(V_ENV), phase.wrapping_add(1));
                self.go([At::EnvAttack4, At::EnvDecay4, At::EnvSustain4, At::EnvRelease4, At::EnvDone4][(phase as usize).min(4)]);
                match phase {
                    // attack to 256
                    0 => {
                        let mut n = env.wrapping_add(hw.bus.r8(e + 0x13) as u16);
                        if n as i16 >= 0x100 {
                            n = 0x100;
                            self.go(At::EnvAttackTop);
                            next(hw);
                        }
                        hw.bus.w16(b(V_ENV_VOLUME), n);
                        hw.bus.w16(b(V_ENV_COUNTER), hw.bus.r8(e + 0x12) as u16);
                    }
                    // decay to the sustain level
                    1 => {
                        let mut n = env.wrapping_sub(hw.bus.r8(e + 0x15) as u16);
                        let level = hw.bus.r8(e + 0x17) as u16;
                        if (level as i16) >= n as i16 {
                            n = level;
                            self.go(At::EnvDecayLevel);
                            next(hw);
                        }
                        hw.bus.w16(b(V_ENV_VOLUME), n);
                        hw.bus.w16(b(V_ENV_COUNTER), hw.bus.r8(e + 0x14) as u16);
                    }
                    // sustain (nothing sets the release)
                    2 => {
                        if hw.bus.r16(b(V_RELEASE)) == 0x80 {
                            self.go(At::EnvSustainNext);
                            next(hw);
                        }
                    }
                    3 => {
                        let dec = hw.bus.r8(e + 0x19) as u16;
                        hw.bus.w16(b(V_ENV_VOLUME), env.saturating_sub(dec));
                        hw.bus.w16(b(V_ENV_COUNTER), hw.bus.r8(e + 0x18) as u16);
                    }
                    _ => {}
                }
            }
            let x = hw.bus.r16(b(V_VOLUME)) as u32 * hw.bus.r16(b(V_ENV_VOLUME)) as u32;
            let vol = (x as u16) >> 8;
            if ch as u16 != self.r16(hw, MUTED4) && self.r16(hw, VOICE_MASK4) & (1 << ch) != 0 {
                let vol = (vol as u32 * self.r16(hw, MASTER4) as u32) as u16 >> 6;
                self.go(At::Vol4);
                self.poke_vol(hw, ch, vol);
            } else {
                self.go(At::VolSkip4);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// A module of two tracks (0: a rest; 1: note `note` for 8 rows, instrument
    /// or sample 0, volume $30), one position (track 1 on voice 0) and one sample
    /// of 32 bytes; 4.0/5.0 (`v5`) with instrument 0's commands: switch sample 0,
    /// play sample, stop, and an attack of $80 per tick.
    fn module(v5: bool, note: u8) -> Vec<u8> {
        let name = |s: &str| format!("{s:<16}").into_bytes();
        let mut tracks = vec![0u8; 0x200];
        tracks[0..2].copy_from_slice(&0x200u16.to_be_bytes());
        tracks.extend(name("REST"));
        tracks.extend([0, 16, 0, 0, 0xff, 0xff]);
        let t1 = tracks.len() as u16;
        tracks[2..4].copy_from_slice(&t1.to_be_bytes());
        tracks.extend(name("NOTE"));
        tracks.extend([note, 8, 0, 0x30, 0xff, 0xff]);
        let mut samples = vec![0u8; 0x400];
        samples[0..4].copy_from_slice(&0x400u32.to_be_bytes());
        let mut header = name("SAMPLE");
        header.resize(0x40, 0);
        header[0x10..0x12].copy_from_slice(&32u16.to_be_bytes());
        header[0x3c..0x40].copy_from_slice(&(0x40u32 + 32).to_be_bytes());
        samples.extend(header);
        samples.extend((0..32).map(|i| 0x40 + i as u8));
        let positions = vec![1, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1];
        let mut cmds = Vec::new();
        if v5 {
            cmds = vec![0u8; 0x200];
            cmds[0..2].copy_from_slice(&0x200u16.to_be_bytes());
            let mut instr = name("INSTRUMENT");
            instr.resize(0x30, 0);
            instr[0x10..0x12].copy_from_slice(&18u16.to_be_bytes());
            instr[0x13] = 0x80;
            cmds.extend(instr);
            for c in [[1u16, 0, 0], [15, 0, 0], [0, 0, 0]] {
                cmds.extend(c.iter().flat_map(|w| w.to_be_bytes()));
            }
        }
        let mut d = name("TEST");
        for l in [tracks.len(), samples.len(), positions.len(), cmds.len()] {
            d.extend((l as u32).to_be_bytes());
        }
        d.extend(if v5 { [0, 3, 0, 200] } else { [0, 2, 0, 0] });
        d.resize(64, 0);
        for part in [tracks, samples, positions, cmds] {
            d.extend(part);
        }
        d
    }

    fn play(d: Vec<u8>, format: &str) -> (Vec<(u32, u16, u64)>, Engine) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.sc"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, format);
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
        (e.m.bus.write_log.take().unwrap(), e)
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_3x_note() {
        let (log, e) = play(module(false, 0x24), "SoundControl 3.x");
        // InitSound: every voice at the second hunk's zeros, LEN 1 and PER 1, DMA
        // on; the note, on the second tick: LOC, LEN 1, PER 1, DMA off, VOL, PER
        // (note $24 played as $23: transpose 0 less one), LEN, LOC, DMA on, LEN 1
        // (no loop)
        assert_eq!(reg(&log, 0xDFF096), [0x800f, 0x0001, 0x8001]);
        assert_eq!(reg(&log, 0xDFF0A4), [1, 1, 16, 1]);
        assert_eq!(reg(&log, 0xDFF0A6), [1, 1, 0xb400 >> 4]);
        assert_eq!(reg(&log, 0xDFF0A8), [0, 0x30]);
        assert_eq!(reg(&log, 0xDFF0B4), [1]);
        let lo = reg(&log, 0xDFF0A2);
        let hi = reg(&log, 0xDFF0A0);
        let a = (hi[1] as u32) << 16 | lo[1] as u32;
        assert_eq!((hi[2], lo[2]), (hi[1], lo[1]));
        // the sample's first long was cleared, the rest is as loaded
        assert_eq!(e.m.bus.r32(a), 0);
        assert_eq!(e.m.bus.r8(a + 4), 0x44);
    }

    #[test]
    fn plays_a_50_note() {
        let (log, _) = play(module(true, 0x10), "SoundControl 5.0");
        // the play sample command: LOC, DMA on with the low bits of twice the
        // note (12: $8008, as the original's d4 holds), LEN 1, PER 1, VOL 0; next
        // tick DMA off, PER, LEN, DMA on; then LEN 2 (no loop) and the stop
        assert_eq!(reg(&log, 0xDFF096)[..3], [0x8008, 0x0001, 0x8001]);
        assert_eq!(reg(&log, 0xDFF0A4)[..3], [1, 16, 2]);
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [1, 0xd600 >> 3]);
        // every tick's volume: the note's, times the envelope's over 256
        assert_eq!(&reg(&log, 0xDFF0A8)[..5], &[0, 0, 0x18, 0x30, 0x30]);
        assert_eq!(reg(&log, 0xDFF0B8)[..3], [0, 0, 0]);
    }
}
