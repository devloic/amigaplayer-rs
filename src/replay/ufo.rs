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
//! Timing. The host calls the player every 59 115 colour clocks. A note start waits twice for
//! the raster beam (nine line changes each, read in a loop at fixed spacings); the code
//! between writes is modelled as per-step costs fitted to the reference's write logs.

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
    /// Walked: CHAN, BLCK, INST chunks and the extra bytes of CHAN initial delays (the start-up of the original takes time for each).
    counts: [u64; 4],
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
    let mut p = Parsed { tracks: [Track { scale: 0x7F, ..Default::default() }; 16], blocks: Vec::new(), insts: *insts, bpm: 0, counts: [0; 4] };
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
                            p.counts[3] += (q - pos - 11) as u64;
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
                p.bpm = d.get(pos + 5).copied().unwrap_or(0);
                Some(pos + 6)
            }
            ID_DRUM => Some(pos + 88),
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

/// Cycle costs (68000 cycles; one colour clock is two). Fitted to the reference's logs.
mod cost {
    pub const COSTS: [u64; super::NF] = [
        100, 105, 107, 87, 118, 573, 503, 796, 14, 22, 152, 56, 100, 100, 869, 828, 900, 0, 0, 0, 0, 0, 0, 0,
    ];
    pub const AFTER_WAIT1: u64 = 494;
    pub const PER: u64 = 484;
    pub const LEN: u64 = 158;
    pub const VOL: u64 = 328;
    pub const DMA_ON: u64 = 202;
    pub const AFTER_WAIT2: [u64; 4] = [104, 104, 154, 154];
    pub const LOC2: u64 = 24;
    pub const BEND_MUL: u64 = 98;
    pub const EX_PER: [u64; 4] = [0, 32, 62, 70];
    pub const EX_LEN: [u64; 4] = [0, 30, 62, 70];
    pub const EX_VOL: [u64; 4] = [0, 48, 86, 120];
    pub const EX_ON: [u64; 4] = [0, 32, 62, 72];
    /// Colour clocks from a volume write to the next voice's (voices 0-1, 1-2, 2-3).
    pub const VOL_GAP_CC: [u64; 3] = [266, 286, 314];
    pub const VOL_GAP_STOPPED_CC: u64 = 104;
    /// The original's start-up (file loading and parsing, about 11 000 to 16 000 colour clocks
    /// before its first write) is not part of the audio timeline of the reference renders: with
    /// the delay modelled the port's sound came 150 to 220 sample frames late; with none it is
    /// within 3 frames. So the port starts its timer at once (the write logs, compared per tick,
    /// are unchanged).
    pub const TIMER_DELAY_CC: u64 = 0;
    pub const START_PER_CHUNK_CC: [u64; 4] = [0, 0, 0, 0];
}

const NF: usize = 24;
const F_ENTRY: usize = 0;
const F_STEP: usize = 1;
const F_ROW: usize = 2;
const F_TRK_IDLE: usize = 3;
const F_TRK_SLEEP: usize = 4;
const F_TRK_RUN: usize = 5;
const F_DISPATCH: usize = 6;
const F_NOTE_OFF: usize = 7;
const F_NOTE_OFF_REC: usize = 8;
const F_NOTE_OFF_HIT: usize = 9;
const F_CONTROLLER: usize = 10;
const F_PROGRAM: usize = 11;
const F_CALL: usize = 12;
const F_RETURN: usize = 13;
const F_BEND: usize = 14;
const F_NOTE_PRE: usize = 15;
const F_VOLENTRY: usize = 16;

pub struct Ufo {
    start_delay: u64,
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
    /// Absolute time in 68000 cycles (twice the colour clock).
    t: u64,
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
        t: 0,
        start_delay: cost::TIMER_DELAY_CC + (0..4).map(|k| cost::START_PER_CHUNK_CC[k] * p.counts[k]).sum::<u64>(),
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
    fn charge(&mut self, k: usize, n: u64) {
        self.t += cost::COSTS[k] * n;
    }

    fn sync(&mut self, hw: &mut Hw) {
        let c = self.t / 2;
        if c > hw.bus.clock {
            hw.bus.clock = c;
        }
    }

    /// The wait after a DMACON write at the current time: nine line changes, read in a loop.
    fn beam_wait(&mut self) {
        let mut now = self.t as i64;
        const LINE: i64 = 454; // a raster line in cycles
        now += 62; // first read of the first iteration
        // the loop's reads are 12 and 13 colour clocks apart alternately, the restart after an exit
        // read 17 and 18 (in cycles: 24/26 and 34/36)
        let (mut tg, mut tx) = (false, false);
        for iter in 0..9 {
            let line = now.div_euclid(LINE);
            now += 16; // second read
            let mut guard = 0;
            while now.div_euclid(LINE) == line && guard < 100 {
                now += if tg { 26 } else { 24 };
                tg = !tg;
                guard += 1;
            }
            if iter < 8 {
                now += if tx { 36 } else { 34 };
                tx = !tx;
            }
        }
        self.t = now as u64;
    }

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

    fn read(&self, pos: &mut usize) -> Option<u8> {
        let b = *self.d.get(*pos)?;
        *pos += 1;
        Some(b)
    }

    fn note_index(k: u8) -> i32 {
        let mut n = k as i32 - 24;
        while n < 0 {
            n += 12;
        }
        n
    }

    fn note_off(&mut self, t: usize, k: u8) {
        let n = Self::note_index(k);
        let mut hits = 0u64;
        for v in self.voices.iter_mut() {
            if v.track == t && v.note == n {
                v.rate = (v.sample_vol as i8).wrapping_neg();
                v.max = 0;
                hits += 1;
            }
        }
        let recs = self.voices.iter().filter(|v| v.track == t).count() as u64;
        self.charge(F_NOTE_OFF, 1);
        self.charge(F_NOTE_OFF_REC, recs);
        self.charge(F_NOTE_OFF_HIT, hits);
    }

    fn note_on(&mut self, hw: &mut Hw, t: usize, k: u8, vol: u8) {
        let n = Self::note_index(k);
        let j = t & 3;
        self.charge(F_NOTE_PRE, 1);
        self.sync(hw);
        hw.dmacon(1 << j);
        self.t = self.t.max(hw.bus.clock * 2);
        self.beam_wait();
        let ins = self.insts[self.tracks[t].inst as usize % 40];
        let has = (self.tracks[t].inst as usize) < 40 && ins.start != 0;
        if !has {
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
        self.t += cost::AFTER_WAIT1;
        self.sync(hw);
        hw.loc(j, ins.start);
        self.t += cost::PER + cost::EX_PER[j] + if self.tracks[t].bend != 0 { cost::BEND_MUL } else { 0 };
        self.sync(hw);
        let per = self.period(t);
        hw.per(j, per);
        self.t += cost::LEN + cost::EX_LEN[j];
        self.sync(hw);
        let mut diff = ins.end.wrapping_sub(ins.start);
        if ins.end < ins.start {
            diff = diff.wrapping_add(1);
        }
        hw.len(j, (diff >> 1) as u16);
        self.t += cost::VOL + cost::EX_VOL[j];
        self.sync(hw);
        hw.vol(j, (vol & 0x7F) as u16);
        self.t += cost::DMA_ON + cost::EX_ON[j];
        self.sync(hw);
        hw.dmacon(0x8000 | 1 << j);
        self.t = self.t.max(hw.bus.clock * 2);
        self.beam_wait();
        self.t += cost::AFTER_WAIT2[j];
        self.sync(hw);
        if ins.looped {
            hw.len(j, (ins.end.wrapping_sub(ins.loop_start) >> 1) as u16);
            self.t += cost::LOC2;
            self.sync(hw);
            hw.loc(j, ins.loop_start);
        } else {
            hw.len(j, 2);
            self.t += cost::LOC2;
            self.sync(hw);
            hw.loc(j, self.silent);
        }
    }

    /// Runs track `t` until an event gives a non-zero delay; returns it (0: stopped).
    fn run_track(&mut self, hw: &mut Hw, t: usize) -> u16 {
        let mut pos = self.tracks[t].ptr;
        for _ in 0..MAX_EVENTS_PER_RUN {
            self.charge(F_DISPATCH, 1);
            let Some(b) = self.read(&mut pos) else {
                self.end_song();
                return 0;
            };
            if b == 0xFF {
                self.end_song();
                return 0;
            }
            if b == 0xFE {
                let Some(n) = self.read(&mut pos) else {
                    self.end_song();
                    return 0;
                };
                self.charge(F_CALL, 1);
                self.tracks[t].ret = pos;
                match self.blocks.get(n as usize) {
                    Some(&bp) => pos = bp,
                    None => {
                        self.end_song();
                        return 0;
                    }
                }
            } else if b == 0xFD {
                self.charge(F_RETURN, 1);
                pos = self.tracks[t].ret;
            } else {
                let data;
                if b >= 0x80 {
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
                            self.note_off(t, data);
                        } else {
                            let eff = ((v as u32 * self.tracks[t].scale as u32) >> 8) as u8;
                            if eff == 0 {
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
                        self.charge(F_CONTROLLER, 1);
                        match data {
                            0 => self.tempo = (v as u32 * 48 / 60) as u8,
                            7 => self.tracks[t].scale = v,
                            _ => {}
                        }
                    }
                    0xC0 => {
                        self.charge(F_PROGRAM, 1);
                        if data == 0x7E {
                            // song restart: signal the end, rewind; the rest of the row is skipped
                            self.songend = true;
                            self.restart = true;
                            return 0;
                        }
                        self.tracks[t].inst = data;
                    }
                    0xE0 => {
                        self.charge(F_BEND, 1);
                        self.tracks[t].bend = data.wrapping_sub(0x10) as i8;
                        self.sync(hw);
                        let per = self.period(t);
                        hw.per(t & 3, per);
                    }
                    _ => {}
                }
            }
            let Some(delay) = varint(&self.d, &mut pos) else {
                self.end_song();
                return 0;
            };
            if delay != 0 {
                self.tracks[t].ptr = pos;
                return delay;
            }
        }
        self.end_song();
        0
    }

    fn end_song(&mut self) {
        self.songend = true;
        self.stopped = true;
    }

    fn row(&mut self, hw: &mut Hw) {
        self.charge(F_ROW, 1);
        for t in 0..16 {
            if !self.tracks[t].active {
                self.charge(F_TRK_IDLE, 1);
                continue;
            }
            if self.tracks[t].wait == 0 {
                self.charge(F_TRK_RUN, 1);
                let dl = self.run_track(hw, t);
                if self.restart {
                    self.restart = false;
                    for tr in self.tracks.iter_mut() {
                        tr.ptr = tr.init_ptr;
                        tr.wait = tr.init_wait;
                    }
                    return;
                }
                self.tracks[t].wait = dl;
            }
            else {
                self.charge(F_TRK_SLEEP, 1);
            }
            self.tracks[t].wait = self.tracks[t].wait.wrapping_sub(1);
        }
    }

    fn row_step(&mut self, hw: &mut Hw) {
        if self.stopped {
            return;
        }
        self.charge(F_STEP, 1);
        self.counter -= self.tempo as i32;
        if self.counter > 0 {
            return;
        }
        self.counter += 120;
        self.row(hw);
    }

    fn volume_stage(&mut self, hw: &mut Hw) {
        self.charge(F_VOLENTRY, 1);
        for j in 0..4 {
            let v = &mut self.voices[j];
            let new = ((2 * v.cur as i32 + v.rate as i32) >> 1) as i8;
            v.cur = new;
            if v.rate >= 0 {
                if new > v.max {
                    v.cur = v.max;
                    v.rate = 0;
                }
            } else if new < 0 {
                v.cur = 0;
                v.rate = 0;
            }
            let out = if self.stopped { 0 } else { (v.cur as u8 & 0x7F) as u16 };
            self.sync(hw);
            hw.vol(j, out);
            if j < 3 {
                self.t += if self.stopped { cost::VOL_GAP_STOPPED_CC } else { cost::VOL_GAP_CC[j] } * 2;
            }
        }
    }
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

    fn timer_delay(&self) -> u64 {
        self.start_delay
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t = hw.bus.clock * 2;
        self.charge(F_ENTRY, 1);
        self.row_step(hw);
        self.row_step(hw);
        self.volume_stage(hw);
    }
}

#[cfg(test)]
#[path = "ufo_tests.rs"]
mod tests;
