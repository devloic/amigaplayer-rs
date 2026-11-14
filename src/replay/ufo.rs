//! UFO (MicroProse, 1994: the music of "UFO: Enemy Unknown", Eagleplayer "UFO", prefixes
//! `mus.` / `ufo.`): a small MIDI-like sequencer of up to 16 tracks on four Paula voices.
//!
//! Clean-room, written from a behavioural specification (drawn up from the register-write
//! logs of the 68k player and from black-box runs of synthetic modules) and checked against
//! those logs. No original code or data is used. The specification had no genuine game
//! module: the chunk layouts come from synthetic ones, so the parsing here is tolerant and
//! bounded (an unknown chunk ends the walk; a stream that runs off the data, a call to a
//! missing block or a runaway chain of zero-delay events ends the song; nothing panics).
//!
//! The module is a chunked file (`FORM`/`DDAT`/`BODY`, first chunk `CHAN`) and a sample bank
//! beside it: `NAME.BANK` for a song named `NAME.mus`, else `SMP.set` in the same directory.
//!
//! Choices where the reference is undefined or its data is not format data:
//! - note numbers above 107 (past the 84-entry period table) get the period 0 (silence), and
//!   the pitch-bend slopes of such notes are the table's difference with zero words;
//! - the song ends at the first end signal (the end event, or the restart program change);
//!   the state after the restart event is rewound as the specification describes but the
//!   host does not play on;
//! - an instrument number of 40 or more is an instrument without a sample.
//!
//! Timing. The host calls the player every 59 115 colour clocks; an interrupt held while the
//! previous call ran past the grid is taken 500 cycles after that call's end. Every write is at
//! the original's colour clock: whole 68000 cycles per code path (enum `K`), a write at the
//! interrupt's start plus half the cycles since, rounded down. A note start waits twice for
//! the raster beam (nine line changes each, read in a loop at fixed spacings) on the
//! original's beam, which runs ahead of the song by the original's start-up (`start_up`: the
//! bank's and the song's walk, the file name's length and the directory's).

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "UFO", detect, new };

/// Colour clocks between two interrupts of the host's timer for this player.
const TICK_CLOCKS: u64 = 59_115;

const PERIODS: [u16; 84] = [
    0x0FBE, 0x0EEE, 0x0DF7, 0x0D09, 0x0C8C, 0x0BDC, 0x0B1A, 0x0A8F, 0x09F4, 0x0969, 0x08D6, 0x0853, //
    0x07DF, 0x0766, 0x070A, 0x069C, 0x063A, 0x05E4, 0x058D, 0x053F, 0x04F2, 0x04AE, 0x046B, 0x0429, //
    0x03EF, 0x03B7, 0x0381, 0x034E, 0x0320, 0x02F2, 0x02C8, 0x029F, 0x027B, 0x0257, 0x0235, 0x0216, //
    0x01F7, 0x01DB, 0x01C0, 0x01A7, 0x0190, 0x0179, 0x0164, 0x0150, 0x013D, 0x012B, 0x011A, 0x010A, //
    0x00FB, 0x00ED, 0x00E0, 0x00D3, 0x00C7, 0x00BC, 0x00B2, 0x00A8, 0x009E, 0x0095, 0x008D, 0x0086, //
    0x00FB, 0x00ED, 0x00E0, 0x00D3, 0x00C7, 0x00BC, 0x00B2, 0x00A8, 0x009E, 0x0095, 0x008D, 0x0086, //
    0x00FB, 0x00ED, 0x00E0, 0x00D3, 0x00C7, 0x00BC, 0x00B2, 0x00A8, 0x009E, 0x0095, 0x008D, 0x0086,
];

/// The table's period at index `i`; words outside the table read as 0 (see the module comment).
fn table(i: i32) -> i32 {
    if (0..84).contains(&i) {
        PERIODS[i as usize] as i32
    } else {
        0
    }
}

// ------------------------------------------------------------------ recognition

fn detect(d: &[u8]) -> bool {
    d.len() >= 24
        && be32(d, 0) == 0x464F_524D
        && be32(d, 8) == 0x4444_4154
        && be32(d, 12) == 0x424F_4459
        && be32(d, 20) == 0x4348_414E
}

// ------------------------------------------------------------------ module

const ID_CHAN: u32 = 0x4348_414E;
const ID_BLCK: u32 = 0x424C_434B;
const ID_INST: u32 = 0x494E_5354;
const ID_BPM: u32 = 0x4250_4D20;
const ID_DRUM: u32 = 0x4452_554D;

const MAX_BLOCKS: usize = 80;
/// Zero-delay events one run of a track may execute before the song is ended (runaway guard).
const MAX_EVENTS_PER_RUN: u32 = 200_000;

#[derive(Clone, Copy, Default)]
struct Inst {
    start: u32,
    end: u32,
    loop_start: u32,
    looped: bool,
    vol: u8,
}

#[derive(Clone, Copy, Default)]
struct Track {
    active: bool,
    init_ptr: usize,
    init_wait: u16,
    ptr: usize,
    wait: u16,
    status: u8,
    inst: u8,
    scale: u8,
    bend: i8,
    base: i32,
    ret: usize,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    track: usize,
    note: i32,
    up: i16,
    down: i16,
    sample_vol: u8,
    max: i8,
    cur: i8,
    rate: i8,
}

struct Parsed {
    tracks: [Track; 16],
    blocks: Vec<usize>,
    insts: [Inst; 40],
    bpm: u8,
    /// Walked: CHAN, BLCK, INST, BPM and DRUM chunks, INST chunks with a loop, and the bytes of
    /// the CHAN initial delays past the first (the original's start-up takes time for each).
    counts: [u64; 7],
}

fn parse_bank(bank: &[u8], ba: u32, insts: &mut [Inst; 40]) {
    let mut ptr = 504u32;
    for (i, ins) in insts.iter_mut().enumerate() {
        let o = 20 + 10 * i;
        let len = be32(bank, o);
        let lp = be32(bank, o + 4);
        let vol = bank.get(o + 9).copied().unwrap_or(0);
        *ins = Inst { start: 0, end: 0, loop_start: 0, looped: lp != 0, vol };
        if len != 0 {
            ins.start = ba.wrapping_add(ptr);
            ins.end = ins.start.wrapping_add(len);
            ins.loop_start = ins.start.wrapping_add(lp);
            ptr = ptr.wrapping_add(len);
        }
    }
}

fn varint(d: &[u8], pos: &mut usize) -> Option<u16> {
    let mut v: u16 = 0;
    for _ in 0..8 {
        let b = *d.get(*pos)?;
        *pos += 1;
        v = (v << 7).wrapping_add((b & 0x7F) as u16);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    Some(v)
}

fn parse_song(d: &[u8], base: u32, insts: &mut [Inst; 40]) -> Parsed {
    let mut p = Parsed { tracks: [Track { scale: 0x7F, ..Default::default() }; 16], blocks: Vec::new(), insts: *insts, bpm: 0, counts: [0; 7] };
    let mut pos = 20usize;
    // each chunk is bounded by the data; a walk step always moves forward
    for _ in 0..100_000 {
        if pos + 4 > d.len() {
            break;
        }
        let id = be32(d, pos);
        let size = be32(d, pos + 4) as usize;
        let next = match id {
            ID_CHAN => {
                p.counts[0] += 1;
                if pos + 10 > d.len() {
                    break;
                }
                let c = d[pos + 9] as usize;
                if c < 16 {
                    let mut q = pos + 10;
                    let t = &mut p.tracks[c];
                    match varint(d, &mut q) {
                        Some(w) => {
                            p.counts[6] += (q - pos - 11) as u64;
                            t.active = true;
                            t.init_ptr = q;
                            t.init_wait = w;
                            t.ptr = q;
                            t.wait = w;
                        }
                        None => t.active = false,
                    }
                }
                pos.checked_add(8).and_then(|x| x.checked_add(size))
            }
            ID_BLCK => {
                p.counts[1] += 1;
                if p.blocks.len() < MAX_BLOCKS {
                    p.blocks.push(pos + 8);
                }
                pos.checked_add(4).and_then(|x| x.checked_add(size))
            }
            ID_INST => {
                p.counts[2] += 1;
                if pos + 20 > d.len() {
                    break;
                }
                p.counts[5] += u64::from(be32(d, pos + 14) != 0);
                let i = d[pos + 9] as usize;
                if i < 40 {
                    let len = be32(d, pos + 10);
                    let lp = be32(d, pos + 14);
                    let start = base.wrapping_add(pos as u32 + 20);
                    p.insts[i] = Inst { start, end: start.wrapping_add(len), loop_start: start.wrapping_add(lp), looped: lp != 0, vol: d[pos + 19] };
                }
                pos.checked_add(4).and_then(|x| x.checked_add(size))
            }
            ID_BPM => {
                p.counts[3] += 1;
                p.bpm = d.get(pos + 5).copied().unwrap_or(0);
                Some(pos + 6)
            }
            ID_DRUM => {
                p.counts[4] += 1;
                Some(pos + 88)
            }
            _ => None,
        };
        match next {
            Some(n) if n > pos => pos = n,
            _ => break,
        }
    }
    p
}

// ------------------------------------------------------------------ timing model

macro_rules! costs {
    ($($name:ident = $c:expr;)*) => {
        /// The original's code paths, one label per branch, each with the 68000 cycles it takes
        /// (per voice 0..3, then for no voice): fitted to the gaps between the writes of the
        /// reference's write logs (exact cycles from a run and a second one with the count
        /// started a cycle late). A write comes at the interrupt's start plus half the cycles
        /// since, rounded down.
        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        enum K { $($name,)* }
        const COSTS: &[[i64; 5]] = &[$($c,)*];
    };
}

costs! {
    TickStart = [0, 0, 0, 0, 0];
    Held = [500, 500, 500, 500, 500];
    TickEnd = [0, 0, 0, 0, 0];
    Step = [42, 42, 42, 42, 42];
    StepIdle = [130, 130, 130, 130, 130];
    StepStop = [0, 0, 0, 0, 0];
    StepStopNew = [82, 82, 82, 82, 82];
    Row = [16, 16, 16, 16, 16];
    RowEnd = [162, 162, 162, 162, 162];
    RowEndStopped = [0, 0, 0, 0, 0];
    RowRestart = [2309, 2309, 2309, 2309, 2309];
    TrkIdle = [88, 88, 88, 88, 88];
    TrkSleep = [117, 117, 117, 117, 117];
    TrkRun = [435, 435, 435, 435, 435];
    Ev = [364, 364, 364, 364, 364];
    EvStatus = [70, 70, 70, 70, 70];
    EvEnd = [17, 17, 17, 17, 17];
    EvCall = [76, 76, 76, 76, 76];
    EvRet = [-5, -5, -5, -5, -5];
    EvUnk = [136, 136, 136, 136, 136];
    Var = [1, 1, 1, 1, 1];
    VarByte = [115, 115, 115, 115, 115];
    VarZero = [0, 0, 0, 0, 0];
    VarZeroJump = [5, 5, 5, 5, 5];
    VarDelay = [131, 131, 131, 131, 131];
    VarDelayJump = [131, 131, 131, 131, 131];
    Wrap = [25, 25, 25, 25, 25];
    Off = [774, 774, 774, 774, 774];
    OffVel = [241, 241, 241, 241, 241];
    OffRec = [31, 31, 31, 31, 31];
    OffHit = [44, 44, 44, 44, 44];
    Vel = [859, 859, 859, 859, 859];
    VelBit = [0, 0, 0, 0, 0];
    ScaleBit = [0, 0, 0, 0, 0];
    Cc = [199, 199, 199, 199, 199];
    CcTempo = [150, 150, 150, 150, 150];
    CcTempoBit = [0, 0, 0, 0, 0];
    CcDivu = [0, 0, 0, 0, 0];
    CcVol = [-9, -9, -9, -9, -9];
    Prog = [138, 138, 138, 138, 138];
    ProgRestart = [0, 0, 0, 0, 0];
    Bend = [785, 785, 785, 785, 785];
    BendZero = [0, 0, 0, 0, 0];
    BendUp = [2, 2, 2, 2, 99];
    BendDown = [0, 0, 0, 0, 97];
    BendBit = [0, 0, 0, 0, 0];
    BendSlopeBit = [0, 0, 0, 0, 0];
    OnPre = [0, 0, 0, 0, 0];
    WaitFirst = [62, 62, 62, 62, 0];
    OnNoSample = [144, 144, 144, 144, 0];
    OnLoc = [494, 494, 494, 494, 0];
    OnPer = [484, 515, 546, 554, 0];
    OnPerBend = [97, 97, 97, 97, 0];
    OnLen = [158, 189, 220, 228, 0];
    OnVol = [327, 375, 413, 448, 0];
    OnDma = [203, 234, 265, 273, 0];
    OnLenLoop = [153, 153, 153, 153, 0];
    OnLenOnce = [104, 104, 104, 104, 0];
    OnLoc2 = [24, 24, 24, 24, 0];
    OnEndLoop = [68, 68, 68, 68, 0];
    OnEndOnce = [58, 58, 58, 58, 0];
    VolEntry = [654, 654, 654, 654, 654];
    VolRise = [12, 12, 12, 12, 0];
    VolTop = [40, 40, 40, 40, 0];
    VolFall = [0, 0, 0, 0, 0];
    VolBottom = [38, 38, 38, 38, 0];
    VolStop = [-11, 196, 196, 196, 0];
    VolStopNew = [-11, 196, 196, 196, 0];
    VolNext = [521, 559, 617, 521, 0];
    VolNextStop = [0, 0, 0, 0, 0];
    VolNextStopNew = [0, 0, 0, 0, 0];
}

/// The beam wait's polling (cycles): the first read after the DMACON write is `WaitFirst`
/// cycles after it, the second `POLL_SECOND` after that, then one every `POLL`; a read
/// that sees another line than the first read of its round ends the round, and the next
/// round's first read comes `POLL_NEXT` after it. `WAIT_LINES` rounds (measured on the
/// reference's VHPOSR reads).
const POLL_SECOND: i64 = 16;
const POLL: i64 = 25;
const POLL_NEXT: i64 = 35;
const WAIT_LINES: u32 = 9;

/// The clock of the original's writes: cycles counted from the interrupt's start.
struct Timing {
    acc: i64,
    /// The interrupt's start (colour clocks).
    t: u64,
    /// Colour clocks the original's beam runs ahead of the port's clock: its start-up
    /// (`start_up`), which the reference's audio does not include but its beam does.
    beam: u64,
    /// The voice the code works on (4: none).
    v: usize,
}

impl Timing {
    fn n(&mut self, k: K, n: u32) {
        if n == 0 {
            return;
        }
        self.acc += COSTS[k as usize][self.v] * n as i64;
    }

    fn c(&mut self, k: K) {
        self.n(k, 1);
    }

    /// The clock `acc` cycles into the interrupt.
    fn clock(&self, acc: i64) -> u64 {
        self.t + (acc.max(0) / 2) as u64
    }

    /// Brings the clock to the original's next write.
    fn w(&mut self, hw: &mut Hw) {
        let at = self.clock(self.acc);
        if hw.bus.clock < at {
            hw.wait(at - hw.bus.clock);
        }
    }

    /// The wait after a DMACON write: `WAIT_LINES` line changes seen by the polling reads
    /// (on the original's beam).
    fn beam_wait(&mut self) {
        use crate::paula::LINE;
        let line = |s: &Self, a: i64| (s.clock(a) + s.beam) / LINE;
        self.c(K::WaitFirst);
        let a0 = self.acc;
        let mut a = a0;
        let mut n = 0;
        loop {
            let cur = line(self, a);
            a += POLL_SECOND;
            while line(self, a) == cur {
                a += POLL;
            }
            n += 1;
            if n == WAIT_LINES {
                break;
            }
            a += POLL_NEXT;
        }
        self.acc = a;
    }
}

/// Colour clocks of the original's start-up before the song starts (from the host's first
/// call): the host calls and the player's own work between them, each stretch's cycles halved
/// and rounded down (measured: the clocks of the host calls in runs and second runs with the
/// cycle count started a cycle late, on 230 files in six directories). `dir` is the length
/// of the module's directory path with its trailing slash; `fallback` when the bank is
/// `SMP.set` after the failed load of the song's own one; `records` the instruments with a
/// sample after the song's INST chunks.
fn start_up(name: &str, dir: usize, fallback: bool, bank: &[u8], counts: &[u64; 7], records: u64) -> u64 {
    let b = name.as_bytes();
    let n = b.len();
    // the song name's suffix test (".mus" in either case, upper-case letters costing more)
    let suffix: i64 = if n >= 4 && b[n - 4] == b'.' {
        if b[n - 3..].eq_ignore_ascii_case(b"mus") {
            21 * i64::from(b[n - 3] == b'M') + 25 * i64::from(b[n - 2] == b'U') + 25 * i64::from(b[n - 1] == b'S')
        } else {
            -73
        }
    } else {
        -115
    };
    let (dir, name_len) = (dir as i64, n as i64);
    let in_bank = |o: usize| if o + 4 <= bank.len() { be32(bank, o) } else { 0 };
    let bank_recs = (0..40).filter(|i| in_bank(20 + 10 * i) != 0).count() as i64;
    let bank_loops = (0..40).filter(|i| in_bank(24 + 10 * i) != 0).count() as i64;
    let c = |k: usize| counts[k] as i64;
    let load = 320 + 21 * name_len + 17 * dir + suffix;
    let bank_parse = 10528 + 66 * bank_recs + 6 * bank_loops;
    let song = 8224 + 518 * c(0) + 230 * c(1) + 432 * c(2) + 127 * c(3) + 84 * c(4) + 42 * c(5) + 115 * c(6) + 8 * records as i64;
    let calls: &[i64] = if fallback {
        &[172, 60, load, 89, 299 + 17 * dir, 32, 36, 167, bank_parse, 16, 24, 236, 236, song]
    } else {
        &[172, 60, load, 45, 36, 167, bank_parse, 16, 24, 236, 236, song]
    };
    calls.iter().map(|&x| (x.max(0) / 2) as u64).sum()
}

pub struct Ufo {
    d: Vec<u8>,
    tracks: [Track; 16],
    blocks: Vec<usize>,
    insts: [Inst; 40],
    voices: [Voice; 4],
    silent: u32,
    tempo: u8,
    counter: i32,
    stopped: bool,
    songend: bool,
    restart: bool,
    /// Stopped before this interrupt.
    stop0: bool,
    /// The clock this interrupt was due (on the timer's grid).
    due: u64,
    tm: Timing,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) || d.len() < be32(d, 4) as usize + 8 {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let name = file.rsplit('/').next().unwrap_or(&file).to_string();
    let mut bank = None;
    if let Some(stem) = bank_stem(&name) {
        bank = crate::deli::read_companion(m, &format!("{stem}BANK")).1.ok();
    }
    let fallback = bank.is_none();
    let bank = match bank.filter(|b| bank_ok(b)) {
        Some(b) => b,
        None => crate::deli::read_companion(m, "SMP.set").1.ok().filter(|b| bank_ok(b))?,
    };
    let ba = m.alloc((bank.len() as u32).max(8));
    if ba == 0 {
        return None;
    }
    m.bus.write_bytes(ba, &bank);
    // the first four bytes of the bank serve as the silent sample
    m.bus.write_bytes(ba, &[0; 4]);
    let mut insts = [Inst::default(); 40];
    parse_bank(&bank, ba, &mut insts);
    let p = parse_song(d, base, &mut insts);
    let t0 = (p.bpm as u32 * 24 / 60) as u8;
    let dir = {
        let dir = m.deli.module_dir.to_string_lossy();
        dir.len() + usize::from(!dir.ends_with('/'))
    };
    let records = p.insts.iter().filter(|i| i.end != i.start).count() as u64;
    let beam = start_up(&name, dir, fallback, &bank, &p.counts, records);
    Some(Box::new(Ufo {
        d: d.to_vec(),
        tracks: p.tracks,
        blocks: p.blocks,
        insts: p.insts,
        voices: [Voice::default(); 4],
        silent: ba,
        tempo: t0,
        counter: t0 as i32,
        stopped: false,
        songend: false,
        restart: false,
        stop0: false,
        due: 0,
        tm: Timing { acc: 0, t: 0, beam, v: 4 },
    }))
}
/// For a song named `x.mus` (the suffix in either case): `x.`, to which the bank name adds `BANK`.
fn bank_stem(name: &str) -> Option<&str> {
    let b = name.as_bytes();
    let n = b.len();
    (n > 4 && b[n - 4] == b'.' && b[n - 3..].eq_ignore_ascii_case(b"mus")).then(|| &name[..n - 3])
}

fn bank_ok(b: &[u8]) -> bool {
    b.len() >= 20 && be32(b, 0) == 0x464F_524D && (be32(b, 4) as u64 + 8) <= b.len() as u64 && be32(b, 8) == 0x4144_4154 && be32(b, 12) == 0x424F_4459
}

impl Ufo {
    fn period(&self, t: usize) -> u16 {
        let tr = &self.tracks[t];
        let v = &self.voices[t & 3];
        let p = tr.base;
        let bend = tr.bend as i32;
        let r = if bend > 0 {
            p - v.up as i32 * bend
        } else if bend < 0 {
            p + v.down as i32 * (-bend)
        } else {
            p
        };
        r as u16
    }

    /// The cycles of the period's computation (a multiplication when the bend is not zero).
    fn period_cost(&mut self, t: usize) {
        let bend = self.tracks[t].bend as i32;
        let v = self.voices[t & 3];
        if bend == 0 {
            self.tm.c(K::BendZero);
            return;
        }
        let slope = if bend > 0 { v.up } else { v.down };
        self.tm.c(if bend > 0 { K::BendUp } else { K::BendDown });
        self.tm.n(K::BendBit, bend.unsigned_abs().count_ones());
        self.tm.n(K::BendSlopeBit, (slope as u16).count_ones());
    }

    fn read(&self, pos: &mut usize) -> Option<u8> {
        let b = *self.d.get(*pos)?;
        *pos += 1;
        Some(b)
    }

    /// The note's index from the key, and the octaves added to bring it to 0 or more.
    fn note_index(k: u8) -> (i32, u32) {
        let mut n = k as i32 - 24;
        let mut w = 0;
        while n < 0 {
            n += 12;
            w += 1;
        }
        (n, w)
    }

    fn note_off(&mut self, t: usize, k: u8) {
        let (n, w) = Self::note_index(k);
        self.tm.n(K::Wrap, w);
        let mut hits = 0;
        for v in self.voices.iter_mut() {
            if v.track == t && v.note == n {
                v.rate = (v.sample_vol as i8).wrapping_neg();
                v.max = 0;
                hits += 1;
            }
        }
        let recs = self.voices.iter().filter(|v| v.track == t).count() as u32;
        self.tm.n(K::OffRec, recs);
        self.tm.n(K::OffHit, hits);
    }

    fn note_on(&mut self, hw: &mut Hw, t: usize, k: u8, vol: u8) {
        let (n, w) = Self::note_index(k);
        let j = t & 3;
        self.tm.n(K::Wrap, w);
        self.tm.v = j;
        self.tm.c(K::OnPre);
        self.tm.w(hw);
        hw.dmacon(1 << j);
        self.tm.beam_wait();
        let ins = self.insts[self.tracks[t].inst as usize % 40];
        let has = (self.tracks[t].inst as usize) < 40 && ins.start != 0;
        if !has {
            self.tm.c(K::OnNoSample);
            self.tm.v = 4;
            return;
        }
        let up = ((table(n) - table(n + 2)) >> 4) as i16;
        let down = match n {
            0 => 400,
            1 => 1016,
            _ => ((table(n - 2) - table(n)) >> 4) as i16,
        };
        self.voices[j] = Voice { track: t, note: n, up, down, sample_vol: ins.vol, max: vol as i8, cur: vol as i8, rate: 0x63 };
        self.tracks[t].base = table(n);
        self.tm.c(K::OnLoc);
        self.tm.w(hw);
        hw.loc(j, ins.start);
        self.tm.c(K::OnPer);
        if self.tracks[t].bend != 0 {
            self.tm.c(K::OnPerBend);
        }
        self.period_cost(t);
        self.tm.v = j;
        self.tm.w(hw);
        let per = self.period(t);
        hw.per(j, per);
        self.tm.c(K::OnLen);
        self.tm.w(hw);
        let mut diff = ins.end.wrapping_sub(ins.start);
        if ins.end < ins.start {
            diff = diff.wrapping_add(1);
        }
        hw.len(j, (diff >> 1) as u16);
        self.tm.c(K::OnVol);
        self.tm.w(hw);
        hw.vol(j, (vol & 0x7F) as u16);
        self.tm.c(K::OnDma);
        self.tm.w(hw);
        hw.dmacon(0x8000 | 1 << j);
        self.tm.beam_wait();
        if ins.looped {
            self.tm.c(K::OnLenLoop);
            self.tm.w(hw);
            hw.len(j, (ins.end.wrapping_sub(ins.loop_start) >> 1) as u16);
            self.tm.c(K::OnLoc2);
            self.tm.w(hw);
            hw.loc(j, ins.loop_start);
            self.tm.c(K::OnEndLoop);
        } else {
            self.tm.c(K::OnLenOnce);
            self.tm.w(hw);
            hw.len(j, 2);
            self.tm.c(K::OnLoc2);
            self.tm.w(hw);
            hw.loc(j, self.silent);
            self.tm.c(K::OnEndOnce);
        }
        self.tm.v = 4;
    }

    /// Runs track `t` until an event gives a non-zero delay; returns it (0: stopped).
    fn run_track(&mut self, hw: &mut Hw, t: usize) -> u16 {
        let mut pos = self.tracks[t].ptr;
        for _ in 0..MAX_EVENTS_PER_RUN {
            self.tm.c(K::Ev);
            let Some(b) = self.read(&mut pos) else {
                self.end_song();
                return 0;
            };
            if b == 0xFF {
                self.tm.c(K::EvEnd);
                self.end_song();
                return 0;
            }
            if b == 0xFE {
                let Some(n) = self.read(&mut pos) else {
                    self.end_song();
                    return 0;
                };
                self.tm.c(K::EvCall);
                self.tracks[t].ret = pos;
                match self.blocks.get(n as usize) {
                    Some(&bp) => pos = bp,
                    None => {
                        self.end_song();
                        return 0;
                    }
                }
            } else if b == 0xFD {
                self.tm.c(K::EvRet);
                pos = self.tracks[t].ret;
            } else {
                let data;
                if b >= 0x80 {
                    self.tm.c(K::EvStatus);
                    self.tracks[t].status = b;
                    match self.read(&mut pos) {
                        Some(x) => data = x,
                        None => {
                            self.end_song();
                            return 0;
                        }
                    }
                } else {
                    data = b;
                }
                let status = self.tracks[t].status;
                match status & 0xF0 {
                    0x80 | 0x90 => {
                        let Some(v) = self.read(&mut pos) else {
                            self.end_song();
                            return 0;
                        };
                        if status & 0xF0 == 0x80 {
                            self.tm.c(K::Off);
                            self.note_off(t, data);
                        } else {
                            let scale = self.tracks[t].scale;
                            self.tm.c(K::Vel);
                            self.tm.n(K::VelBit, v.count_ones());
                            self.tm.n(K::ScaleBit, scale.count_ones());
                            let eff = ((v as u32 * scale as u32) >> 8) as u8;
                            if eff == 0 {
                                self.tm.c(K::OffVel);
                                self.note_off(t, data);
                            } else {
                                self.note_on(hw, t, data, eff);
                            }
                        }
                    }
                    0xB0 => {
                        let Some(v) = self.read(&mut pos) else {
                            self.end_song();
                            return 0;
                        };
                        self.tm.c(K::Cc);
                        match data {
                            0 => {
                                self.tm.c(K::CcTempo);
                                self.tm.n(K::CcTempoBit, v.count_ones());
                                self.tm.n(K::CcDivu, divu_cycles(v as u32 * 48, 60));
                                self.tempo = (v as u32 * 48 / 60) as u8
                            }
                            7 => {
                                self.tm.c(K::CcVol);
                                self.tracks[t].scale = v
                            }
                            _ => {}
                        }
                    }
                    0xC0 => {
                        self.tm.c(K::Prog);
                        if data == 0x7E {
                            // song restart: signal the end, rewind; the rest of the row is skipped
                            self.tm.c(K::ProgRestart);
                            self.songend = true;
                            self.restart = true;
                            return 0;
                        }
                        self.tracks[t].inst = data;
                    }
                    0xE0 => {
                        self.tm.c(K::Bend);
                        self.tracks[t].bend = data.wrapping_sub(0x10) as i8;
                        self.period_cost(t);
                        self.tm.w(hw);
                        let per = self.period(t);
                        hw.per(t & 3, per);
                    }
                    _ => self.tm.c(K::EvUnk),
                }
            }
            let p0 = pos;
            let Some(delay) = varint(&self.d, &mut pos) else {
                self.end_song();
                return 0;
            };
            self.tm.c(K::Var);
            self.tm.n(K::VarByte, (pos - p0 - 1) as u32);
            // the delay after a call or a return is read by that path's own code
            let jump = b == 0xFE || b == 0xFD;
            if delay != 0 {
                self.tm.c(if jump { K::VarDelayJump } else { K::VarDelay });
                self.tracks[t].ptr = pos;
                return delay;
            }
            self.tm.c(if jump { K::VarZeroJump } else { K::VarZero });
        }
        self.end_song();
        0
    }

    fn end_song(&mut self) {
        self.songend = true;
        self.stopped = true;
    }

    fn row(&mut self, hw: &mut Hw) {
        self.tm.c(K::Row);
        for t in 0..16 {
            if !self.tracks[t].active {
                self.tm.c(K::TrkIdle);
                continue;
            }
            if self.tracks[t].wait == 0 {
                self.tm.c(K::TrkRun);
                let dl = self.run_track(hw, t);
                if self.restart {
                    self.tm.c(K::RowRestart);
                    self.restart = false;
                    for tr in self.tracks.iter_mut() {
                        tr.ptr = tr.init_ptr;
                        tr.wait = tr.init_wait;
                    }
                    return;
                }
                self.tracks[t].wait = dl;
            } else {
                self.tm.c(K::TrkSleep);
            }
            self.tracks[t].wait = self.tracks[t].wait.wrapping_sub(1);
        }
        self.tm.c(if self.stopped { K::RowEndStopped } else { K::RowEnd });
    }

    fn row_step(&mut self, hw: &mut Hw) {
        self.tm.c(K::Step);
        if self.stopped {
            self.tm.c(if self.stop0 { K::StepStop } else { K::StepStopNew });
            return;
        }
        self.counter -= self.tempo as i32;
        if self.counter > 0 {
            self.tm.c(K::StepIdle);
            return;
        }
        self.counter += 120;
        self.row(hw);
    }

    fn volume_stage(&mut self, hw: &mut Hw) {
        self.tm.c(K::VolEntry);
        for j in 0..4 {
            self.tm.v = j;
            let v = &mut self.voices[j];
            let new = ((2 * v.cur as i32 + v.rate as i32) >> 1) as i8;
            v.cur = new;
            let k = if v.rate >= 0 {
                if new > v.max {
                    v.cur = v.max;
                    v.rate = 0;
                    K::VolTop
                } else {
                    K::VolRise
                }
            } else if new < 0 {
                v.cur = 0;
                v.rate = 0;
                K::VolBottom
            } else {
                K::VolFall
            };
            let out = if self.stopped { 0 } else { (v.cur as u8 & 0x7F) as u16 };
            self.tm.c(k);
            match (self.stopped, self.stop0) {
                (true, true) => self.tm.c(K::VolStop),
                (true, false) => self.tm.c(K::VolStopNew),
                _ => {}
            }
            self.tm.w(hw);
            hw.vol(j, out);
            if j < 3 {
                self.tm.c(match (self.stopped, self.stop0) {
                    (true, true) => K::VolNextStop,
                    (true, false) => K::VolNextStopNew,
                    _ => K::VolNext,
                });
            }
        }
        self.tm.v = 4;
    }
}

/// The 68000's DIVU cycles (an unsigned 32 by 16 bit division without overflow) over two:
/// 38 plus one or two per quotient bit by the shift-and-subtract steps (the documented timing).
fn divu_cycles(dividend: u32, divisor: u16) -> u32 {
    if divisor == 0 || (dividend >> 16) >= divisor as u32 {
        return 5;
    }
    let mut m = 38;
    let h = (divisor as u32) << 16;
    let mut d = dividend;
    for _ in 0..15 {
        let top = d & 0x8000_0000 != 0;
        d <<= 1;
        if top {
            d = d.wrapping_sub(h);
        } else {
            m += 2;
            if d >= h {
                d -= h;
                m -= 1;
            }
        }
    }
    m
}

impl Replayer for Ufo {
    fn format(&self) -> &'static str {
        "UFO"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK_CLOCKS
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        // an interrupt due while the previous call still ran is taken when that call returns
        let end = self.tm.clock(self.tm.acc);
        self.due = if self.due == 0 { hw.bus.clock } else { self.due + TICK_CLOCKS };
        let held = end > self.due;
        self.tm.t = if held { end } else { hw.bus.clock };
        self.tm.acc = 0;
        self.tm.v = 4;
        if held {
            self.tm.c(K::Held);
        }
        self.tm.c(K::TickStart);
        self.stop0 = self.stopped;
        self.row_step(hw);
        self.row_step(hw);
        self.volume_stage(hw);
        self.tm.c(K::TickEnd);
    }
}

#[cfg(test)]
#[path = "ufo_tests.rs"]
mod tests;
