//! Paul Shields (`ps.*`), the music system Paul Shields wrote for Airball, Goldrunner
//! II, Jug, Amiga Soccer and Amnios (1987-91).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Paul Shields player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A `ps.*` file is pure data: sample headers, a song header (four voices' initial
//! volumes and track lists, and the envelope or instrument tables), track lists,
//! patterns, envelopes and the sample data. Three layouts (`FF`, `01`, `00`) are told
//! apart by their headers; two engines play them. Engine A (layouts `FF` and `01`)
//! has seven host-provided synthetic waves as instruments 16-22, absolute-period
//! notes and envelope tables; engine B (layout `00`) has an instrument table and
//! writes PER before VOL. Both play at the host's 50 Hz, one parse of a voice's
//! pattern bytes whenever its duration runs out, with volume and pitch envelopes,
//! slides, portamento and a release; a note's sample starts with the next tick's
//! DMA-on and its loop is written right after it.
//!
//! Every write is at the reference's colour clock: whole 68000 cycles per code path (`K`, per
//! voice where the original's differ, one table per engine), a write at the tick's start plus half
//! the cycles since, rounded down; fitted exactly on all in-tick write gaps of the reference's runs
//! of the 8 refcmp rows and 3 Jug files (engine A) and the 2 Amnios files (engine B), from a run and a
//! second one with the cycle count started a cycle late. After the voices the original polls the
//! beam for 9 line changes before it writes the loops (`Timing::beam_wait`, the polling measured on
//! the reference's VHPOSR reads). The beam's phase there is the port's own: the reference's depends on
//! how long its load took, which the port does not model; it moves only the loop writes, by up to a line.
//!
//! Decisions where the specification leaves the choice to the port:
//! * Engine A can index its period table below the start for notes below the table
//!   (the original then reads its own code). The port extends the table downwards by
//!   octaves instead (the period doubled per octave, at most 0xFFFF).
//! * Instrument 22 (the original plays 400 bytes of its own code) is a silent wave
//!   of the same length here. No corpus file uses it.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Paul Shields", detect, new };

/// The 96-entry pitch table: 8 steps per semitone, the lowest octave (largest periods).
const PITCH: [u16; 96] = [
    27364, 27167, 26972, 26778, 26585, 26394, 26204, 26015, 25828, 25642, 25458, 25275, 25093, 24912, 24733, 24555,
    24378, 24203, 24029, 23856, 23684, 23514, 23345, 23177, 23010, 22845, 22680, 22517, 22355, 22194, 22035, 21876,
    21719, 21562, 21407, 21253, 21100, 20949, 20798, 20648, 20500, 20352, 20206, 20060, 19916, 19773, 19631, 19489,
    19349, 19210, 19072, 18935, 18798, 18663, 18529, 18396, 18263, 18132, 18001, 17872, 17743, 17616, 17489, 17363,
    17238, 17114, 16991, 16869, 16747, 16627, 16507, 16389, 16271, 16154, 16037, 15922, 15807, 15694, 15581, 15469,
    15357, 15247, 15137, 15028, 14920, 14813, 14706, 14601, 14496, 14391, 14288, 14185, 14083, 13982, 13881, 13781,
];

/// Engine A's synthetic waves, instruments 16-21 (signed bytes); 22 is 400 bytes of silence.
const WAVES: [&[i8]; 6] = [
    &[127, 127, -127, -127],
    &[127, 127, 127, 127, -127, -127, -127, -127],
    &[0, 127, 0, -127],
    &[0, 89, 127, 89, 0, -89, -127, -89],
    &[127, 42, -43, -128],
    &[127, 90, 54, 17, -18, -55, -91, -128],
];
const WAVE22_LEN: u32 = 400;

/// Host bytes the voices point at before the music sets them: the "next track"
/// command and an envelope's hold marker.
const HOST_NEXT: u32 = 0xFFFF_0000;
const HOST_HOLD: u32 = 0xFFFF_0002;

/// The master volume.
const MASTER: i16 = 64;

/// From the host's first write (the interrupts off) to the song start, and from a timer call to
/// the first write of its tick, in the reference's runs.
const START: u64 = 990;
const ENTRY: u64 = 163;

macro_rules! costs {
    ($($name:ident = $a:expr, $b:expr;)*) => {
        /// The original's code paths, one label per branch, each with the 68000 cycles it takes
        /// (per voice 0..3, then for no voice): fitted to the gaps between the writes of the
        /// reference's write logs (exact cycles from a run and a second one with the count
        /// started a cycle late). The player writes Paula directly: a write comes at the tick's
        /// start plus half the cycles since, rounded down.
        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        enum K { $($name,)* }
        const COSTS: &[[[i64; 5]; 2]] = &[$([$a, $b],)*];
        #[allow(dead_code)]
        const K_NAMES: &[&str] = &[$(stringify!($name),)*];
    };
}

costs! {
    TickStart = [1, 1, 1, 1, 1], [1, 1, 1, 1, 1];
    VoiceOff = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    Voice = [0, 0, 0, 22, 0], [0, 0, 0, 0, 0];
    SlideNo = [0, 0, 0, 0, 0], [371, 371, 371, 371, 371];
    SlideEq = [33, 33, 33, 33, 33], [0, 0, 0, 0, 0];
    SlideUp = [78, 78, 78, 78, 78], [0, 0, 0, 0, 0];
    SlideDown = [88, 88, 88, 88, 88], [0, 0, 0, 0, 0];
    NoParse = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    Parse = [271, 271, 271, 271, 271], [66, 66, 66, 66, 66];
    PByte = [62, 62, 62, 62, 62], [81, 81, 81, 81, 81];
    PNote = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    PNoteAbs = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    PInst = [130, 130, 130, 130, 130], [0, 0, 0, 0, 0];
    PVolOfs = [0, 0, 0, 0, 0], [27, 27, 27, 27, 27];
    PTrack = [27, 27, 27, 27, 27], [0, 0, 0, 0, 0];
    PRest = [231, 231, 231, 231, 231], [0, 0, 0, 0, 0];
    PPEnv = [20, 20, 20, 20, 20], [0, 0, 0, 0, 0];
    PVEnv = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    PPorta = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    PFilter = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    PInstrB = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    PSkip = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    DurByte = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    DurWord = [38, 38, 38, 38, 38], [0, 0, 0, 0, 0];
    Inst = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    InstSilent = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    InstNone = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    SetEnv = [110, 110, 110, 110, 110], [174, 174, 174, 174, 174];
    Porta = [329, 329, 329, 329, 329], [0, 0, 0, 0, 0];
    PortaAbs = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    PortaNeg = [28, 28, 28, 28, 28], [379, 379, 379, 379, 379];
    PortaDur0 = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    NoteStart = [0, 0, 0, 0, 0], [0, 31, 74, 0, 0];
    NoteVolOfs = [157, 157, 157, 157, 157], [84, 84, 84, 84, 84];
    NoteDefVol = [169, 169, 169, 169, 169], [84, 84, 84, 84, 84];
    NoteLoc = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    NoteLen = [20, 20, 20, 20, 20], [20, 20, 20, 20, 20];
    NoteEnd = [132, 163, 194, 202, 132], [0, 0, 0, 70, 0];
    TsRep = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    TsWord = [75, 75, 75, 75, 75], [77, 77, 77, 77, 77];
    TsPat = [45, 45, 45, 45, 45], [45, 45, 45, 45, 45];
    TsPatRep = [16, 16, 16, 16, 16], [16, 16, 16, 16, 16];
    TsVolTgt = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    TsDetune = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    TsLoop = [23, 23, 23, 23, 23], [0, 0, 0, 0, 0];
    TsLoopEnd = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    TsStop = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    TsVolAll = [200, 200, 200, 200, 200], [0, 0, 0, 0, 0];
    TsTrans = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    RelDec = [429, 429, 429, 429, 429], [22, 22, 22, 22, 22];
    RelRestDec = [26, 26, 26, 26, 26], [0, 0, 0, 0, 0];
    RelRestNeg = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    RelNeg = [403, 403, 403, 403, 403], [0, 0, 0, 0, 0];
    EnvCnt = [407, 407, 407, 407, 407], [0, 0, 0, 0, 0];
    EnvByte = [68, 68, 68, 68, 68], [41, 41, 41, 41, 41];
    EnvHold = [380, 380, 380, 380, 380], [0, 0, 0, 0, 0];
    EnvLoop = [0, 0, 0, 0, 0], [27, 27, 27, 27, 27];
    EnvVal = [453, 453, 453, 453, 453], [73, 73, 73, 73, 73];
    PEnvCnt = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    PEnvByte = [41, 41, 41, 41, 41], [0, 0, 0, 0, 0];
    PEnvHold = [0, 0, 0, 0, 0], [409, 409, 409, 409, 409];
    PEnvLoop = [27, 27, 27, 27, 27], [0, 0, 0, 0, 0];
    PEnvVal = [73, 73, 73, 73, 73], [0, 0, 0, 0, 0];
    VolLo = [0, 17, 24, 51, 0], [0, 0, 0, 0, 0];
    VolHi = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    VolIn = [3, 20, 27, 54, 3], [247, 264, 271, 298, 247];
    PerStep = [0, 31, 62, 70, 0], [0, 31, 62, 0, 0];
    PerAbs = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    Conv = [347, 347, 347, 347, 347], [39, 39, 39, 39, 39];
    ConvIter = [19, 19, 19, 19, 19], [19, 19, 19, 19, 19];
    ConvBit = [2, 2, 2, 2, 2], [2, 2, 2, 2, 2];
    ConvBig = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    ConvBelow = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    ConvNeg = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    VoiceEnd = [0, 0, 0, 0, 0], [0, 138, 138, 115, 0];
    VoiceStop = [0, 0, 0, 0, 0], [0, 0, 0, 0, 0];
    WaitPre = [112, 112, 112, 112, 112], [237, 237, 237, 237, 237];
    WaitExit = [112, 112, 112, 112, 112], [112, 112, 112, 112, 112];
    LoopSkip = [19, 19, 19, 0, 19], [19, 19, 19, 0, 19];
    LoopLoc = [55, 55, 55, 36, 55], [55, 55, 55, 36, 55];
    LoopLen = [20, 20, 20, 20, 20], [20, 20, 20, 20, 20];
}

/// The beam wait after the voices (measured on the reference's reads of VHPOSR): a first read of
/// the line, then reads `POLL_FIRST` cycles later and every `POLL` cycles until the line has
/// changed; a change read, the next round starts `POLL_NEXT` cycles later; `WAIT_LINES` changes.
const POLL_FIRST: i64 = 16;
const POLL: i64 = 25;
const POLL_NEXT: i64 = 35;
const WAIT_LINES: u32 = 9;

/// The original's time: 68000 cycles since the tick's start.
struct Timing {
    acc: i64,
    t: u64,
    fit: Option<std::io::BufWriter<std::fs::File>>,
    counts: Vec<(u16, u32)>,
    /// The voice the code works on (4: none).
    v: usize,
    /// The engine: 0 (A) or 1 (B), whose code and costs differ.
    e: usize,
}

impl Timing {
    fn n(&mut self, k: K, n: u32) {
        if n == 0 {
            return;
        }
        let k = k as usize;
        self.acc += COSTS[k][self.e][self.v] * n as i64;
        if self.fit.is_some() {
            let key = (k * 5 + self.v) as u16;
            match self.counts.iter_mut().find(|e| e.0 == key) {
                Some(e) => e.1 += n,
                None => self.counts.push((key, n)),
            }
        }
    }

    fn c(&mut self, k: K) {
        self.n(k, 1);
    }

    /// The clock `acc` cycles into the tick.
    fn clock(&self, acc: i64) -> u64 {
        self.t + (acc.max(0) / 2) as u64
    }

    /// A trace entry (for the fit) at the current time, `fix` cycles of it known.
    fn mark(&mut self, kind: &str, fix: i64) {
        let at = self.clock(self.acc);
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let b = if self.e == 1 { "b_" } else { "" };
            let mut names: Vec<String> = self
                .counts
                .iter()
                .map(|&(k, n)| match k % 5 {
                    4 => format!("{b}{}:{n}", K_NAMES[k as usize / 5]),
                    v => format!("{b}{}.{v}:{n}", K_NAMES[k as usize / 5]),
                })
                .collect();
            if fix != 0 {
                names.push(format!("FIX:{fix}"));
            }
            let _ = writeln!(f, "{at} {kind} {}", names.join(" "));
        }
        self.counts.clear();
    }

    /// Brings the clock to the original's next write (of kind `kind`).
    fn w(&mut self, hw: &mut Hw, kind: &str) {
        self.mark(kind, 0);
        let at = self.clock(self.acc);
        if hw.bus.clock < at {
            hw.wait(at - hw.bus.clock);
        }
    }

    /// The beam wait: `WAIT_LINES` line changes seen by the polling reads.
    fn beam_wait(&mut self) {
        use crate::paula::LINE;
        let line = |s: &Self, a: i64| s.clock(a) / LINE;
        let a0 = self.acc;
        let mut a = a0;
        let mut cur = line(self, a);
        a += POLL_FIRST;
        let mut n = 0;
        loop {
            if line(self, a) != cur {
                n += 1;
                if n == WAIT_LINES {
                    break;
                }
                a += POLL_NEXT;
                cur = line(self, a);
                a += POLL_FIRST;
            } else {
                a += POLL;
            }
        }
        self.acc = a;
        self.mark("wx", a - a0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Layout {
    /// 32-byte sample headers, the song header at 0x200, engine A.
    FF,
    /// 10-byte sample headers, the song header at 0xA0, engine A.
    L01,
    /// 32-byte sample headers, the song header at 0x200, engine B.
    L00,
}

impl Layout {
    fn header(self) -> usize {
        if self == Layout::L01 {
            0xA0
        } else {
            0x200
        }
    }
    fn engine_a(self) -> bool {
        self != Layout::L00
    }
}

// ---------------------------------------------------------------- recognition

fn w(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

fn equal4(d: &[u8], o: usize) -> bool {
    (1..4).all(|k| w(d, o + 4 * k) == w(d, o))
}

fn pointer(d: &[u8], at: usize) -> Option<usize> {
    let o = w(d, at);
    (o != 0 && o < 0x8000 && o % 2 == 0).then_some(o as usize)
}

/// The layout, by the Eagleplayer's test (specification, section 2).
fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 530 || be32(d, 0) != 0 || be32(d, 4) != 0 || w(d, 8) != 0 {
        return None;
    }
    // the first rule that holds as a whole (the volume words of the first are 0 in
    // the 32-byte layouts' unused sample headers)
    let long_at = |at: usize, v: u32| pointer(d, at).is_some_and(|o| o + 4 <= d.len() && be32(d, o) == v);
    if equal4(d, 164) && long_at(160, 0x00B4_00B6) {
        return Some(Layout::L01);
    }
    if equal4(d, 516) && long_at(512, 0x0214_0216) {
        return Some(Layout::FF);
    }
    if equal4(d, 514) && pointer(d, 516).is_some_and(|o| o + 2 <= d.len() && matches!(w(d, o - 2), 0xFFEC | 0xFFE8)) {
        return Some(Layout::L00);
    }
    None
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sample {
    /// Offsets into the module (or host addresses when `host`).
    lc: u32,
    len: u16,
    llc: u32,
    llen: u16,
    fine: u16,
    defvol: u16,
    /// The loop is the host's silence buffer.
    silent_loop: bool,
    host: bool,
}

/// The sample data start (section 2): after the end command of the list before the
/// last-starting one, searched from that list's start.
fn sample_start(d: &[u8], lay: Layout) -> Option<usize> {
    let h = lay.header();
    let first = if lay.engine_a() { h + 6 } else { h + 4 };
    let mut m: i16 = 0;
    for k in 0..4 {
        m = m.max(w(d, first + 4 * k) as i16);
    }
    if m < 2 {
        return None;
    }
    let m = m as usize;
    let t = w(d, m - 2);
    let mut o = m;
    while o + 2 <= d.len() {
        if w(d, o) == t {
            return Some(o + 2);
        }
        o += 2;
    }
    None
}

/// The 16 module samples (offsets into the module).
fn samples(d: &[u8], lay: Layout) -> Option<Vec<Sample>> {
    let mut at = sample_start(d, lay)? as u32;
    let mut out = Vec::new();
    for k in 0..16 {
        let (len, ls, ll, fine, defvol) = if lay == Layout::L01 {
            let o = 10 * k;
            (w(d, o + 2), w(d, o + 4), w(d, o + 6), w(d, o + 8), 0)
        } else {
            let o = 32 * k;
            (w(d, o + 0x16), w(d, o + 0x1A), w(d, o + 0x1C), w(d, o + 0x1E), w(d, o + 0x18))
        };
        let silent = ll < 2;
        out.push(Sample {
            lc: at,
            len,
            llc: at + ls as u32,
            llen: if silent { 2 } else { ll },
            fine,
            defvol,
            silent_loop: silent,
            host: false,
        });
        at += 2 * len as u32;
    }
    (at as usize <= d.len()).then_some(out)
}

fn detect(d: &[u8]) -> bool {
    layout(d).and_then(|l| samples(d, l)).is_some()
}

// ------------------------------------------------------------------- the engine

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    active: bool,
    rel: bool,
    abs: bool,
    vol: i16,
    vtgt: i16,
    vspd: u16,
    vcnt: u16,
    volofs: u16,
    defvol: u16,
    env: i16,
    per: u16,
    frac: u16,
    slide: u32,
    det: u16,
    tr: u16,
    fine: u16,
    vr: u8,
    vc: u8,
    venvs: u32,
    venv: u32,
    pr: u8,
    pc: u8,
    penvs: u32,
    penv: u32,
    lc: u32,
    len: u16,
    llc: u32,
    llen: u16,
    lst: u32,
    lpos: u32,
    patstart: u32,
    pat: u32,
    rep: u16,
    dur: u16,
}

/// How a voice's parse ended.
#[derive(Clone, Copy, PartialEq)]
enum Event {
    Note,
    Rest,
    Stop,
}

struct PaulShields {
    t: Timing,
    d: Vec<u8>,
    lay: Layout,
    base: u32,
    /// Instruments: the 16 samples, then (engine A) the 7 synthetic waves.
    smp: Vec<Sample>,
    silence: u32,
    /// The first table of the song header (pitch envelopes or instruments) and the
    /// volume-envelope table (engine A).
    pt: u32,
    vt: u32,
    v: [Voice; 4],
    dacc: u16,
    iacc: u16,
    songend: bool,
}

impl PaulShields {
    fn b(&self, a: u32) -> u8 {
        match a {
            HOST_NEXT => {
                if self.lay.engine_a() {
                    0x0C
                } else {
                    0x90
                }
            }
            HOST_HOLD => 0x80,
            _ => self.d.get(a as usize).copied().unwrap_or(0),
        }
    }

    fn w(&self, a: u32) -> u16 {
        (self.b(a) as u16) << 8 | self.b(a.wrapping_add(1)) as u16
    }

    /// The guest address of a sample address.
    fn guest(&self, s: &Sample, a: u32) -> u32 {
        if s.host {
            a
        } else {
            self.base.wrapping_add(a)
        }
    }

    fn init(&mut self, hw: &mut Hw) {
        hw.bus.write_word(0xDFF09A, 0x0780);
        hw.wait(8);
        hw.dmacon(0x000F);
        hw.wait(8);
        hw.bus.write_word(0xDFF09E, 0x00FF);
        if !self.lay.engine_a() {
            self.filter(hw, true);
        }
        self.dacc = 0;
        self.iacc = 0;
        let h = self.lay.header() as u32;
        let pairs = if self.lay.engine_a() {
            self.pt = self.w(h) as u32;
            self.vt = self.w(h + 2) as u32;
            h + 4
        } else {
            self.pt = self.w(h) as u32;
            h + 2
        };
        let (lc, len) = if self.lay.engine_a() { (self.smp[16].lc, 1) } else { (self.silence, 1) };
        for k in 0..4 {
            let ch = 3 - k;
            let vol = self.w(pairs + 4 * k as u32) as i16;
            let list = self.w(pairs + 4 * k as u32 + 2) as u32;
            self.v[ch] = Voice {
                active: true,
                vol,
                vtgt: vol,
                lst: list,
                lpos: list,
                rep: 1,
                dur: 1,
                pat: HOST_NEXT,
                venvs: HOST_HOLD,
                venv: HOST_HOLD,
                penvs: HOST_HOLD,
                penv: HOST_HOLD,
                lc,
                len,
                llc: lc,
                llen: len,
                ..Voice::default()
            };
        }
    }

    /// CIA-A PRA bit 1: the LED filter on (bit clear) or off.
    fn filter(&self, hw: &mut Hw, on: bool) {
        let x = hw.bus.read_byte(0xBFE001);
        hw.bus.write_byte(0xBFE001, if on { x & !2 } else { x | 2 });
    }

    /// conv(n) (section 5.5).
    fn conv(&mut self, ch: usize, n: u16) -> u16 {
        self.t.c(K::Conv);
        let v = &self.v[ch];
        let mut x = n.wrapping_add(v.tr).wrapping_add(v.fine) as i16;
        if !self.lay.engine_a() && x < 0 {
            self.t.c(K::ConvNeg);
            return 0xFFFF;
        }
        let mut o = -1i32;
        loop {
            o += 1;
            x = x.wrapping_sub(96);
            if x < 0 {
                break;
            }
        }
        self.t.n(K::ConvIter, o as u32 + 1);
        let v = &self.v[ch];
        let i = 96 + x as i32;
        let p = if i >= 0 {
            let p = PITCH[i as usize];
            if o >= 16 {
                self.t.c(K::ConvBig);
                0
            } else {
                self.t.n(K::ConvBit, o as u32);
                p >> o
            }
        } else {
            self.t.c(K::ConvBelow);
            // below the table: the table extended down by octaves
            let oct = (-i + 95) / 96;
            let p = PITCH[(i + 96 * oct) as usize] as u32;
            if oct >= 16 {
                0xFFFF
            } else {
                (p << oct).min(0xFFFF) as u16
            }
        };
        p.wrapping_add(v.det)
    }

    /// A duration byte (a 0 byte is followed by a word).
    fn duration(&mut self, ch: usize) -> u16 {
        let p = self.v[ch].pat;
        let b = self.b(p);
        if b == 0 {
            self.t.c(K::DurWord);
            self.v[ch].pat = p.wrapping_add(3);
            self.w(p.wrapping_add(1))
        } else {
            self.t.c(K::DurByte);
            self.v[ch].pat = p.wrapping_add(1);
            b as u16
        }
    }

    fn next_byte(&mut self, ch: usize) -> u8 {
        let p = self.v[ch].pat;
        self.v[ch].pat = p.wrapping_add(1);
        self.b(p)
    }

    fn instrument(&mut self, ch: usize, i: usize) {
        let Some(s) = self.smp.get(i).copied() else {
            self.t.c(K::InstNone);
            return;
        };
        self.t.c(K::Inst);
        let lc = self.guest(&s, s.lc);
        let llc = if s.silent_loop {
            self.t.c(K::InstSilent);
            self.silence
        } else {
            self.guest(&s, s.llc)
        };
        let v = &mut self.v[ch];
        v.lc = lc;
        v.len = s.len;
        v.llc = llc;
        v.llen = s.llen;
        v.fine = s.fine;
        v.defvol = s.defvol;
    }

    /// An envelope at module offset `e`: its reload byte, then the bytes.
    fn set_env(&mut self, ch: usize, vol: bool, e: u32) {
        self.t.c(K::SetEnv);
        let r = self.b(e);
        let v = &mut self.v[ch];
        if vol {
            v.vr = r;
            v.vc = r;
            v.venvs = e.wrapping_add(1);
            v.venv = e.wrapping_add(1);
        } else {
            v.pr = r;
            v.pc = r;
            v.penvs = e.wrapping_add(1);
            v.penv = e.wrapping_add(1);
        }
    }

    fn portamento(&mut self, ch: usize, n1: u8, n2: u8) {
        self.t.c(K::Porta);
        let (mut a, mut b) = (n1 as u16 * 8, n2 as u16 * 8);
        if self.lay.engine_a() && self.v[ch].abs {
            self.t.c(K::PortaAbs);
            a = self.conv(ch, a);
            b = self.conv(ch, b);
        }
        self.v[ch].per = a;
        let d = b.wrapping_sub(a) as i16;
        let neg = d < 0;
        let d = d.unsigned_abs() as u32;
        let dur = self.duration(ch);
        self.v[ch].dur = dur;
        let mut s = if dur == 0 {
            self.t.c(K::PortaDur0);
            0
        } else {
            let dur = dur as u32;
            ((d / dur) << 16) + ((((d % dur) << 16) / dur) & 0xFFFF)
        };
        if neg {
            self.t.c(K::PortaNeg);
            s = s.wrapping_neg();
        }
        self.v[ch].slide = s;
    }

    /// The note start (section 5.3).
    fn note_start(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.v[ch];
        v.vc = v.vr;
        v.venv = v.venvs;
        v.pc = v.pr;
        v.penv = v.penvs;
        v.rel = false;
        let (lc, len) = (v.lc, v.len);
        self.t.c(K::NoteStart);
        self.t.w(hw, "off");
        hw.dmacon(1 << ch);
        let v = &mut self.v[ch];
        if v.volofs != 0 {
            self.t.c(K::NoteVolOfs);
            v.env = v.volofs as i16;
        } else {
            self.t.c(K::NoteDefVol);
            v.env = v.defvol as i16;
        }
        self.t.c(K::NoteLoc);
        self.t.w(hw, "loc");
        hw.loc(ch, lc);
        self.t.c(K::NoteLen);
        self.t.w(hw, "len");
        hw.len(ch, len);
        self.t.c(K::NoteEnd);
        self.iacc |= 1 << (7 + ch);
        self.dacc |= 0x8200 | 1 << ch;
    }

    /// The track step at a pattern end (section 5.7); `None` when the voice stopped.
    fn track_step(&mut self, ch: usize) -> Option<()> {
        let a = self.lay.engine_a();
        self.v[ch].rep = self.v[ch].rep.wrapping_sub(1);
        if self.v[ch].rep != 0 {
            self.t.c(K::TsRep);
            self.v[ch].pat = self.v[ch].patstart;
            return Some(());
        }
        // a list of commands only, with no pattern, would never end
        for _ in 0..0x10000 {
            self.t.c(K::TsWord);
            let p = self.v[ch].lpos;
            let wd = self.w(p);
            self.v[ch].lpos = p.wrapping_add(2);
            if wd < 0xFFE8 {
                self.t.c(K::TsPat);
                self.v[ch].rep = 1;
                let mut wd = wd;
                if wd & 0x8000 != 0 {
                    self.t.c(K::TsPatRep);
                    wd &= 0x7FFF;
                    let r = self.w(self.v[ch].lpos);
                    let v = &mut self.v[ch];
                    v.rep = r;
                    v.lpos = v.lpos.wrapping_add(2);
                }
                let v = &mut self.v[ch];
                v.patstart = wd as u32;
                v.pat = wd as u32;
                return Some(());
            }
            let w1 = self.w(self.v[ch].lpos);
            let w2 = self.w(self.v[ch].lpos.wrapping_add(2));
            let take = |n: u32, s: &mut Self| s.v[ch].lpos = s.v[ch].lpos.wrapping_add(2 * n);
            let cmd = match (a, wd) {
                (true, 0xFFE8) | (false, 0xFFF0) => 0,
                (true, 0xFFEC) | (false, 0xFFF8) => 1,
                (true, 0xFFF0) | (false, 0xFFEC) => 2,
                (true, 0xFFF4) | (false, 0xFFE8) => 3,
                (true, 0xFFF8) | (false, 0xFFF4) => 4,
                _ => 5,
            };
            self.t.c([K::TsVolTgt, K::TsDetune, K::TsLoop, K::TsStop, K::TsVolAll, K::TsTrans][cmd]);
            match cmd {
                0 => {
                    take(2, self);
                    let v = &mut self.v[ch];
                    v.vtgt = w1 as i16;
                    v.vspd = w2;
                    v.vcnt = w2;
                }
                1 => {
                    take(1, self);
                    self.v[ch].det = w1;
                }
                2 => {
                    let v = &mut self.v[ch];
                    v.lpos = v.lst;
                    if ch == if a { 1 } else { 0 } {
                        self.t.c(K::TsLoopEnd);
                        self.songend = true;
                    }
                }
                3 => {
                    let v = &mut self.v[ch];
                    v.active = false;
                    if a {
                        v.abs = false;
                        v.rel = false;
                    }
                    self.songend = true;
                    return None;
                }
                4 => {
                    take(2, self);
                    for v in &mut self.v {
                        v.vtgt = w1 as i16;
                        v.vspd = w2;
                        v.vcnt = w2;
                    }
                }
                _ => {
                    take(1, self);
                    self.v[ch].tr = w1;
                }
            }
        }
        self.v[ch].active = false;
        None
    }

    /// Parses pattern bytes up to a note, a portamento or a rest (section 5.2).
    fn parse(&mut self, ch: usize, hw: &mut Hw) -> Event {
        let a = self.lay.engine_a();
        for _ in 0..0x10000 {
            self.t.c(K::PByte);
            let c = self.next_byte(ch);
            if a {
                self.t.c(match c {
                    0x00 => K::PNote,
                    0x04 => K::PInst,
                    0x08 => K::PVolOfs,
                    0x0C => K::PTrack,
                    0x10 => K::PRest,
                    0x14 | 0x1C => K::PPEnv,
                    0x18 => K::PVEnv,
                    0x20 => K::PPorta,
                    _ => K::PSkip,
                });
            } else {
                self.t.c(match c {
                    0x00..=0x7F => K::PNote,
                    0x80 => K::PRest,
                    0x84 => K::PPorta,
                    0x88 => K::PVolOfs,
                    0x8C => K::PInstrB,
                    0x90 => K::PTrack,
                    0x94 | 0x98 => K::PFilter,
                    _ => K::PSkip,
                });
            }
            if a {
                match c {
                    0x00 => {
                        let n = self.next_byte(ch) as u16 * 8;
                        let n = if self.v[ch].abs {
                            self.t.c(K::PNoteAbs);
                            self.conv(ch, n)
                        } else {
                            n
                        };
                        self.v[ch].per = n;
                        self.v[ch].dur = self.duration(ch);
                        self.v[ch].slide = 0;
                        self.note_start(ch, hw);
                        return Event::Note;
                    }
                    0x04 => {
                        let i = self.next_byte(ch) as usize;
                        self.instrument(ch, i);
                    }
                    0x08 => {
                        let b = self.next_byte(ch);
                        let v = &mut self.v[ch];
                        v.volofs = v.volofs & 0xFF00 | b as u16;
                    }
                    0x0C => {
                        if self.track_step(ch).is_none() {
                            return Event::Stop;
                        }
                    }
                    0x10 => {
                        self.v[ch].rel = true;
                        self.v[ch].dur = self.duration(ch);
                        return Event::Rest;
                    }
                    0x14 | 0x1C => {
                        let i = self.next_byte(ch) as u32;
                        self.v[ch].abs = c == 0x14;
                        let e = self.w(self.pt.wrapping_add(i)) as u32;
                        self.set_env(ch, false, e);
                    }
                    0x18 => {
                        let i = self.next_byte(ch) as u32;
                        let e = self.w(self.vt.wrapping_add(i)) as u32;
                        self.set_env(ch, true, e);
                    }
                    0x20 => {
                        let n1 = self.next_byte(ch);
                        let n2 = self.next_byte(ch);
                        self.portamento(ch, n1, n2);
                        self.note_start(ch, hw);
                        return Event::Note;
                    }
                    // not a command of the engine: skipped
                    _ => {}
                }
            } else {
                match c {
                    0x00..=0x7F => {
                        self.v[ch].per = c as u16 * 8;
                        self.v[ch].dur = self.duration(ch);
                        self.v[ch].slide = 0;
                        self.note_start(ch, hw);
                        return Event::Note;
                    }
                    0x80 => {
                        self.v[ch].rel = true;
                        self.v[ch].dur = self.duration(ch);
                        return Event::Rest;
                    }
                    0x84 => {
                        let n1 = self.next_byte(ch);
                        let n2 = self.next_byte(ch);
                        self.portamento(ch, n1, n2);
                        self.note_start(ch, hw);
                        return Event::Note;
                    }
                    0x88 => {
                        let b = self.next_byte(ch) as u16;
                        let v = &mut self.v[ch];
                        v.volofs = v.volofs & 0xFF00 | b;
                        v.env = (v.env as u16 & 0xFF00 | b) as i16;
                    }
                    0x8C => {
                        let i = self.next_byte(ch) as u32;
                        let r = self.pt.wrapping_add(i);
                        let (s, ve, pe) = (self.w(r), self.w(r + 2), self.w(r + 4));
                        self.instrument(ch, (s / 16) as usize);
                        self.set_env(ch, true, ve as u32);
                        self.set_env(ch, false, pe as u32);
                    }
                    0x90 => {
                        if self.track_step(ch).is_none() {
                            return Event::Stop;
                        }
                    }
                    0x94 => self.filter(hw, true),
                    0x98 => self.filter(hw, false),
                    _ => {}
                }
            }
        }
        self.v[ch].active = false;
        Event::Stop
    }

    /// One envelope step (section 5.4); returns the delta to add, if any.
    fn env_step(&mut self, ch: usize, vol: bool) -> Option<i16> {
        let v = &mut self.v[ch];
        let (cnt, cur, start, reload) = if vol {
            (&mut v.vc, &mut v.venv, v.venvs, v.vr)
        } else {
            (&mut v.pc, &mut v.penv, v.penvs, v.pr)
        };
        let [k_cnt, k_byte, k_hold, k_loop, k_val] = if vol {
            [K::EnvCnt, K::EnvByte, K::EnvHold, K::EnvLoop, K::EnvVal]
        } else {
            [K::PEnvCnt, K::PEnvByte, K::PEnvHold, K::PEnvLoop, K::PEnvVal]
        };
        *cnt = cnt.wrapping_sub(1);
        if *cnt != 0 {
            self.t.c(k_cnt);
            return None;
        }
        let mut p = *cur;
        for _ in 0..0x100 {
            self.t.c(k_byte);
            let c = match p {
                HOST_HOLD => 0x80,
                _ => self.d.get(p as usize).copied().unwrap_or(0),
            };
            p = p.wrapping_add(1);
            match c {
                0x80 => {
                    self.t.c(k_hold);
                    return None;
                }
                0x81 => {
                    self.t.c(k_loop);
                    p = start
                }
                _ => {
                    self.t.c(k_val);
                    let v = &mut self.v[ch];
                    if vol {
                        v.vc = reload;
                        v.venv = p;
                    } else {
                        v.pc = reload;
                        v.penv = p;
                    }
                    return Some(c as i8 as i16);
                }
            }
        }
        None
    }

    /// The release (`rest`: in the tick of the rest command, another path) or the volume envelope.
    fn release_or_env(&mut self, ch: usize, rest: bool) {
        if self.v[ch].rel {
            if self.v[ch].env >= 0 {
                self.t.c(if rest { K::RelRestDec } else { K::RelDec });
                self.v[ch].env = self.v[ch].env.wrapping_sub(2);
            } else {
                self.t.c(if rest { K::RelRestNeg } else { K::RelNeg });
            }
        } else if let Some(d) = self.env_step(ch, true) {
            self.v[ch].env = self.v[ch].env.wrapping_add(d);
        }
    }

    fn write_vol(&mut self, ch: usize, hw: &mut Hw) {
        let v = &self.v[ch];
        let x = v.vol.wrapping_add(v.env).wrapping_sub(MASTER);
        self.t.c(if x < 0 {
            K::VolLo
        } else if x > 64 {
            K::VolHi
        } else {
            K::VolIn
        });
        self.t.w(hw, "vol");
        hw.vol(ch, x.clamp(0, 64) as u16);
    }

    fn write_per(&mut self, ch: usize, hw: &mut Hw) {
        self.t.c(K::PerStep);
        let v = &mut self.v[ch];
        let pf = ((v.per as u32) << 16 | v.frac as u32).wrapping_add(v.slide);
        v.per = (pf >> 16) as u16;
        v.frac = pf as u16;
        if let Some(d) = self.env_step(ch, false) {
            self.v[ch].per = self.v[ch].per.wrapping_add(d as u16);
        }
        let v = self.v[ch];
        let p = if self.lay.engine_a() && v.abs {
            self.t.c(K::PerAbs);
            v.per
        } else {
            self.conv(ch, v.per)
        };
        self.t.w(hw, "per");
        hw.per(ch, p);
    }

    /// A voice's step (section 5.1).
    fn voice(&mut self, ch: usize, hw: &mut Hw) {
        self.t.v = ch;
        if !self.v[ch].active {
            self.t.c(K::VoiceOff);
            return;
        }
        self.t.c(K::Voice);
        let v = &mut self.v[ch];
        v.vcnt = v.vcnt.wrapping_sub(1);
        if v.vcnt != 0 {
            self.t.c(K::SlideNo);
        } else if v.vol == v.vtgt {
            self.t.c(K::SlideEq);
        } else {
            self.t.c(if v.vol < v.vtgt { K::SlideUp } else { K::SlideDown });
            v.vol += if v.vol < v.vtgt { 1 } else { -1 };
            v.vcnt = v.vspd;
        }
        let v = &mut self.v[ch];
        v.dur = v.dur.wrapping_sub(1);
        let ev = if v.dur == 0 {
            self.t.c(K::Parse);
            Some(self.parse(ch, hw))
        } else {
            self.t.c(K::NoParse);
            None
        };
        if ev == Some(Event::Stop) {
            self.t.c(K::VoiceStop);
            return;
        }
        if self.lay.engine_a() {
            self.release_or_env(ch, ev == Some(Event::Rest));
            self.write_vol(ch, hw);
            self.write_per(ch, hw);
        } else {
            match ev {
                Some(Event::Note) => {
                    if let Some(d) = self.env_step(ch, true) {
                        self.v[ch].env = self.v[ch].env.wrapping_add(d);
                    }
                }
                Some(Event::Rest) => {}
                _ => self.release_or_env(ch, false),
            }
            self.write_per(ch, hw);
            self.write_vol(ch, hw);
        }
        self.t.c(K::VoiceEnd);
    }
}

impl Replayer for PaulShields {
    fn format(&self) -> &'static str {
        "Paul Shields"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    /// The first tick's first write follows its timer call by this much (the interrupt's entry).
    fn timer_delay(&self) -> u64 {
        ENTRY
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t.t = hw.bus.clock;
        self.t.acc = 0;
        self.t.counts.clear();
        if let Some(f) = &mut self.t.fit {
            use std::io::Write;
            let _ = writeln!(f, "{} tick", hw.bus.clock);
        }
        self.t.v = 4;
        let itmp = self.iacc;
        self.t.c(K::TickStart);
        self.t.w(hw, if self.dacc & 0x8000 != 0 { "on" } else { "off" });
        hw.dmacon(self.dacc);
        self.dacc = 0;
        self.iacc = 0;
        for ch in (0..4).rev() {
            self.voice(ch, hw);
        }
        self.t.v = 4;
        self.t.c(K::WaitPre);
        self.t.mark("wa", 0);
        self.t.beam_wait();
        self.t.c(K::WaitExit);
        for ch in (0..4).rev() {
            self.t.v = ch;
            if itmp & 1 << (7 + ch) != 0 {
                let v = self.v[ch];
                self.t.c(K::LoopLoc);
                self.t.w(hw, "loc");
                hw.loc(ch, v.llc);
                self.t.c(K::LoopLen);
                self.t.w(hw, "len");
                hw.len(ch, v.llen);
            } else {
                self.t.c(K::LoopSkip);
            }
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    let mut smp = samples(d, lay)?;
    let silence = m.alloc(4);
    if silence == 0 {
        return None;
    }
    if lay.engine_a() {
        let mut size = WAVE22_LEN;
        for wv in WAVES {
            size += wv.len() as u32;
        }
        let mut a = m.alloc(size);
        if a == 0 {
            return None;
        }
        for k in 0..7 {
            let n = if k < 6 { WAVES[k].len() as u32 } else { WAVE22_LEN };
            if k < 6 {
                for (i, &b) in WAVES[k].iter().enumerate() {
                    m.bus.ram[a as usize + i] = b as u8;
                }
            }
            smp.push(Sample { lc: a, len: (n / 2) as u16, llc: a, llen: (n / 2) as u16, host: true, ..Sample::default() });
            a += n;
        }
    }
    let mut s = PaulShields {
        t: Timing {
            acc: 0,
            t: 0,
            fit: std::env::var_os("PS_FIT").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new),
            counts: Vec::new(),
            v: 4,
            e: if lay.engine_a() { 0 } else { 1 },
        },
        d: d.to_vec(),
        lay,
        base,
        smp,
        silence,
        pt: 0,
        vt: 0,
        v: Default::default(),
        dacc: 0,
        iacc: 0,
        songend: false,
    };
    // the host's start (interrupts and DMA off, the audio modulation off) comes before the song
    // start, not with the first tick
    let t0 = m.bus.clock;
    s.init(&mut Hw { bus: &mut m.bus });
    m.bus.clock = t0 + START;
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

    fn words(d: &mut [u8], o: usize, ws: &[u16]) {
        for (i, &v) in ws.iter().enumerate() {
            p16(d, o + 2 * i, v);
        }
    }

    /// A synthetic layout-FF module: voices 3-1 rest in a loop, voice 0 plays
    /// instrument 0 (8 bytes at 0x292, default volume 10, no loop) with the pattern
    /// bytes `pat` (at 0x280), then stops.
    fn module(pat: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x29A];
        words(&mut d, 0x16, &[4, 10, 0, 0, 0]);
        // PT, VT, then (vol, list) for channels 3, 2, 1, 0
        words(&mut d, 0x200, &[0x214, 0x218, 64, 0x250, 64, 0x250, 64, 0x250, 64, 0x254]);
        words(&mut d, 0x214, &[0x0214, 0x0216, 0x0240]);
        d[0x240] = 1;
        d[0x241] = 0x80;
        words(&mut d, 0x250, &[0x0260, 0xFFF0, 0x0280, 0xFFF4]);
        d[0x260..0x265].copy_from_slice(&[0x10, 0, 0x10, 0, 0x0C]);
        d[0x280..0x280 + pat.len()].copy_from_slice(pat);
        p16(&mut d, 0x290, 0xFFF0);
        d[0x292..0x29A].copy_from_slice(&[0, 40, 80, 120, 0, 0x80, 0xC0, 0xF0]);
        d
    }

    fn play(d: Vec<u8>, ticks: usize) -> (bool, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("ps.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Paul Shields");
        let mut out = vec![0f32; 2 * (960 * ticks + 100)];
        e.render(&mut out);
        let end = e.songend();
        (end, e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_the_three_layouts() {
        let d = module(&[0x04, 0, 0x00, 12, 3, 0x0C]);
        assert_eq!(layout(&d), Some(Layout::FF));
        assert_eq!(sample_start(&d, Layout::FF), Some(0x292));
        assert!(detect(&d));
        // the sample data must fit
        assert!(!detect(&d[..0x298]));
        let mut e = d.clone();
        e[3] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        p16(&mut e, 0x214, 0x0215);
        assert!(!detect(&e));
        // layout 00: the first list preceded by a track command
        let mut b = d.clone();
        words(&mut b, 0x200, &[0x300, 64, 0x252, 64, 0x252, 64, 0x252, 64, 0x252]);
        p16(&mut b, 0x250, 0xFFEC);
        assert_eq!(layout(&b), Some(Layout::L00));
        // layout 01: the header at 0xA0
        let mut c = vec![0u8; 0x220];
        words(&mut c, 0xA0, &[0xB4, 0xB6, 30, 0x100, 30, 0x100, 30, 0x100, 30, 0x100]);
        words(&mut c, 0xB4, &[0x00B4, 0x00B6]);
        assert_eq!(layout(&c), Some(Layout::L01));
        assert!(!detect(&c[..0x200]));
    }

    #[test]
    fn plays_a_note_then_its_loop_and_stops() {
        let (end, log) = play(module(&[0x04, 0, 0x00, 12, 3, 0x0C]), 6);
        assert!(end);
        // init, then each tick's DMACON: off-mask first, the note's DMA on the next tick
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..5], [0x000F, 0x0000, 0x0001, 0x8201, 0x0000]);
        // note 12 (per 96): one octave up from the table's first entry
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per[0], PITCH[0] >> 1);
        // the resting voices: release below 0, VOL 0, the table's lowest period
        assert_eq!(reg(&log, 0xDFF0D6)[0], PITCH[0]);
        assert_eq!(reg(&log, 0xDFF0D8)[0], 0);
        // the default volume, the sample, then the silence loop of 2 words
        assert_eq!(reg(&log, 0xDFF0A8)[0], 10);
        assert_eq!(reg(&log, 0xDFF0A4), [4, 2]);
        // voice 0 stops in tick 4: three ticks of writes
        assert_eq!(per.len(), 3);
    }

    #[test]
    fn portamento_slides_by_the_divided_distance() {
        // from note 0 to note 1 (8 steps) over 3 ticks: 2 + 0xAAAA/65536 a tick, the
        // first step in the note's own tick
        let (_, log) = play(module(&[0x04, 0, 0x20, 0, 1, 3, 0x0C]), 6);
        assert_eq!(reg(&log, 0xDFF0A6), [PITCH[2], PITCH[5], PITCH[7]]);
    }

    #[test]
    fn notes_below_the_table_extend_it_by_octaves() {
        // note 0 with a fine tune of -8: one octave below entry 88
        let mut d = module(&[0x04, 0, 0x00, 0, 3, 0x0C]);
        p16(&mut d, 0x1E, 0xFFF8);
        let (_, log) = play(d, 3);
        assert_eq!(reg(&log, 0xDFF0A6)[0], PITCH[88] * 2);
    }
}
