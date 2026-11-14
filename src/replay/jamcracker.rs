//! JamCracker, ported from NostalgicPlayer's JamCracker player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Where the original player's register writes differ from what NostalgicPlayer
//! asks of its mixer, this follows the original (checked against the JamCracker
//! Eagleplayer, "JamCracker player module V1.33"). Like the original it plays
//! from the module in guest memory: it patches the pattern and sample addresses
//! into the module's tables, and its AM phase effect rewrites the waveform inside
//! the sample. A note's sample pointer and length go to Paula before its DMA goes
//! on, and the loop (or, for a one-shot sample, one word of silence) right after;
//! both DMACON masks are written on every row, even when no note starts.
//!
//! Every write is at the original's colour clock: whole 68000 cycles per code path
//! (`K`, per voice where the original's differ), a write at the previous one's
//! clock plus half the cycles since, rounded down, and the song's end a call into
//! the host; fitted exactly on the write gaps of the Eagleplayer's runs of 48
//! archive modules and 120 one-change copies (every effect on every voice), and
//! exact on 28 more copies not in the fit (effects combined). The
//! song starts on the clock the original's InitPlayer reaches (`start_clocks`).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "JamCracker", detect, new };

/// Instrument flags.
const LOOP: u8 = 1;
const AM: u8 = 2;

/// Pattern row byte 2.
const SPEED_PORTA_TO_NOTE: u8 = 0x40;
const SPEED_SET_VOLUME: u8 = 0x80;

#[derive(Default)]
struct Voice {
    /// Where an AM instrument's waveform lies in its sample (0x80 + 0x40 * voice).
    wave_offset: u16,
    dmacon: u16,
    /// The sample (length in words, guest address) Paula plays next.
    len: u16,
    addr: u32,
    /// Index of the note in PERIODS.
    per_index: usize,
    /// The arpeggio's periods, rotated every tick.
    pers: [u16; 3],
    por: i16,
    delta_por: i16,
    por_level: i16,
    vib: i16,
    delta_vib: i16,
    vol: i16,
    delta_vol: i16,
    vol_level: u16,
    phase: u16,
    delta_phase: i16,
    vib_cnt: u8,
    vib_max: u8,
    flags: u8,
}

macro_rules! costs {
    ($($name:ident = $c:expr,)*) => {
        /// The original's code paths, one label per branch, each with the 68000 cycles it takes:
        /// fitted to the gaps between the writes of the Eagleplayer's write logs (the cycles from
        /// a run and a second one with the count started a cycle late). The replayer writes Paula
        /// directly: a write comes at the last one's clock plus half the cycles since, rounded down.
        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        enum K { $($name,)* }
        const COSTS: &[[i64; 5]] = &[$($c,)*];
        #[allow(dead_code)]
        const K_NAMES: &[&str] = &[$(stringify!($name),)*];
    };
}

costs! {
    TickStart = [187, 187, 187, 187, 187],
    NoRow = [0, 0, 0, 0, 0],
    Row = [1, 1, 1, 1, 1],
    PatternEnd = [217, 217, 217, 217, 217],
    SongWrap = [120, 120, 120, 120, 120],
    NwVoice = [0, 0, 0, 0, 0],
    NoteNone = [0, 0, 0, 0, 0],
    NotePorta = [198, 198, 198, 198, 198],
    NotePortaIdle = [91, 91, 91, 91, 91],
    Note = [364, 364, 364, 364, 364],
    InstZero = [0, 0, 0, 0, 0],
    InstAm = [63, 63, 63, 63, 63],
    InstPlain = [79, 79, 79, 79, 79],
    SpeedNone = [0, 0, 0, 0, 0],
    Speed = [16, 16, 16, 16, 16],
    ArpNone = [439, 439, 439, 439, 439],
    ArpOff = [514, 514, 514, 514, 514],
    Arp = [566, 566, 566, 566, 566],
    VibNone = [0, 0, 0, 0, 0],
    VibOff = [67, 67, 67, 67, 67],
    Vib = [115, 115, 115, 115, 115],
    PhaseNone = [0, 0, 0, 0, 0],
    PhaseOff = [59, 59, 59, 59, 59],
    Phase = [57, 57, 57, 57, 57],
    VolOff = [18, 18, 18, 18, 18],
    VolSet = [67, 67, 67, 67, 67],
    VolSlide = [56, 56, 56, 56, 56],
    VolSlideDown = [60, 60, 60, 60, 60],
    VolSetZero = [60, 60, 60, 60, 60],
    VolNone = [0, 0, 0, 0, 0],
    PortaNone = [0, 0, 0, 0, 0],
    PortaOff = [51, 51, 51, 51, 51],
    PortaToUp = [0, 0, 0, 0, 0],
    PortaToDown = [14, 14, 14, 14, 14],
    PortaDown = [127, 127, 127, 127, 127],
    PortaUp = [113, 113, 113, 113, 113],
    DmaOffPre = [0, 0, 0, 0, 0],
    DmaNoNote = [0, 0, 0, 0, 0],
    DmaNote = [261, 336, 411, 486, 0],
    DmaNoteLen = [190, 190, 190, 190, 0],
    DmaNotePer = [211, 211, 211, 211, 0],
    OneShot = [350, 337, 324, 265, 0],
    Looped = [185, 141, 97, 30, 0],
    DmaOnPre = [516, 516, 516, 516, 516],
    LoopLoc = [198, 190, 190, 190, 0],
    LoopLen = [190, 190, 190, 190, 0],
    Chan = [381, 18, 18, 0, 0],
    PerRot = [129, 129, 129, 129, 129],
    PorNeg = [30, 30, 30, 30, 30],
    PorNegMax = [52, 52, 52, 52, 52],
    PorPos = [30, 30, 30, 30, 30],
    PorPosMin = [42, 42, 42, 42, 42],
    PorZero = [0, 0, 0, 0, 0],
    PerLo = [1, 1, 1, 1, 1],
    PerHi = [8, 8, 8, 8, 8],
    PerIn = [0, 0, 0, 0, 0],
    PorAddLo = [1, 1, 1, 1, 1],
    PorAddHi = [16, 16, 16, 16, 16],
    PorAddIn = [0, 0, 0, 0, 0],
    VibIdle = [596, 596, 596, 596, 596],
    VibStep = [649, 649, 649, 649, 649],
    VibTurn = [36, 36, 36, 36, 36],
    VolWrite = [0, 23, 46, 46, 0],
    VolCap = [4, 4, 4, 4, 4],
    VolLo = [1, 1, 1, 1, 1],
    VolHi = [16, 16, 16, 16, 16],
    VolIn = [0, 0, 0, 0, 0],
    NoAm = [0, 0, 0, 0, 0],
    AmNoPhase = [21, 21, 21, 21, 21],
    AmPhase = [3617, 3617, 3617, 3617, 3617],
    AmPhaseNeg = [16, 16, 16, 16, 16],
    AmPhaseWrap = [20, 20, 20, 20, 20],
    ChanEnd = [506, 506, 506, 506, 506],
}

/// The original's time: 68000 cycles since its last write, and that write's clock.
struct Timing {
    acc: i64,
    t0: u64,
    fit: Option<std::io::BufWriter<std::fs::File>>,
    counts: Vec<(u16, u32)>,
    /// The voice the code works on (4: none); the original's code differs per voice.
    v: usize,
}

impl Timing {
    /// Lets label `k`'s cycles of the original's code pass.
    fn c(&mut self, k: K) {
        let k = k as usize;
        self.acc += COSTS[k][self.v];
        if self.fit.is_some() {
            let key = (k * 5 + self.v) as u16;
            match self.counts.iter_mut().find(|e| e.0 == key) {
                Some(e) => e.1 += 1,
                None => self.counts.push((key, 1)),
            }
        }
    }

    /// A call into the host: the time is the last write's (or call's) plus half the cycles since,
    /// rounded down, and counts on from there.
    fn call(&mut self) {
        self.t0 += (self.acc.max(0) / 2) as u64;
        self.acc = 0;
    }

    /// Brings the clock to the original's next write (of kind `kind`): the last write's (or the
    /// tick's start) plus half the cycles since, rounded down; the time counts on from there.
    fn w(&mut self, hw: &mut Hw, kind: &str) {
        let at = self.t0 + (self.acc.max(0) / 2) as u64;
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let names: Vec<String> = self
                .counts
                .iter()
                .map(|&(k, n)| match k % 5 {
                    4 => format!("{}:{n}", K_NAMES[k as usize / 5]),
                    v => format!("{}.{v}:{n}", K_NAMES[k as usize / 5]),
                })
                .collect();
            let _ = writeln!(f, "{at} {kind} {}", names.join(" "));
        }
        self.counts.clear();
        if hw.bus.clock < at {
            hw.wait(at - hw.bus.clock);
        }
        self.t0 = at;
        self.acc = 0;
    }
}

pub struct JamCracker {
    t: Timing,
    /// Guest addresses of the instrument table (40 bytes an instrument), the
    /// pattern table (6 bytes a pattern) and the song table (one word a position).
    inst_table: u32,
    patt_table: u32,
    song_table: u32,
    song_len: u16,
    /// The player's word of silence, played after a sample that does not loop.
    silence: u32,
    /// The next row, the rows left in its pattern, and the song position.
    row: u32,
    note_cnt: u8,
    song_pos: u32,
    positions_left: u16,
    wait: u8,
    wait_cnt: u8,
    tmp_dmacon: u16,
    voices: [Voice; 4],
    songend: bool,
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 6 && d.starts_with(b"BeEp")
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let num_inst = be16(d, 4) as usize;
    let inst_off = 6;
    let patt_count_off = inst_off + 40 * num_inst;
    let num_patt = be16(d, patt_count_off) as usize;
    let patt_off = patt_count_off + 2;
    let song_len_off = patt_off + 6 * num_patt;
    let song_len = be16(d, song_len_off);
    let song_off = song_len_off + 2;
    let data_off = song_off + 2 * song_len as usize;
    if data_off > d.len() || num_patt == 0 || song_len == 0 {
        return None;
    }

    // as the original's InitSound: the patterns follow the song table, then the
    // samples, and their addresses go into the tables (a sample of size 0 gets
    // the address of the next one)
    let mut a = base + data_off as u32;
    for i in 0..num_patt {
        let e = base + (patt_off + 6 * i) as u32;
        m.bus.w32(e + 2, a);
        // adda.w: the pattern's size in bytes is taken as a signed word
        a = a.wrapping_add((be16(d, patt_off + 6 * i) as u32 * 32) as u16 as i16 as u32);
    }
    for i in 0..num_inst {
        let e = base + (inst_off + 40 * i) as u32;
        m.bus.w32(e + 0x24, a);
        a = a.wrapping_add(be32(d, inst_off + 40 * i + 0x20));
    }

    m.bus.clock += start_clocks(d, num_inst, num_patt, patt_off, song_off, song_len, data_off, a.wrapping_sub(base) as usize);

    // a one-shot AM instrument makes the phase effect rewrite 64 bytes below and
    // at the silence (the original: its own 4-byte data hunk and what follows it);
    // here they stay in this allocation
    let mem = m.alloc(0x80 + 4);
    if mem == 0 {
        return None;
    }
    let silence = mem + 0x80;

    let mut voices: [Voice; 4] = Default::default();
    for (i, v) in voices.iter_mut().enumerate() {
        v.wave_offset = 0x80 + 0x40 * i as u16;
        v.dmacon = 1 << i;
        v.pers = [PERIODS[0], 0, 0];
        v.vol_level = 0x40;
    }

    let mut p = JamCracker {
        t: Timing {
            acc: 0,
            t0: 0,
            fit: std::env::var_os("JAM_FIT").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new),
            counts: Vec::new(),
            v: 4,
        },
        inst_table: base + inst_off as u32,
        patt_table: base + patt_off as u32,
        song_table: base + song_off as u32,
        song_len,
        silence,
        row: 0,
        note_cnt: 0,
        song_pos: 0,
        positions_left: 0,
        wait: 6,
        wait_cnt: 1,
        tmp_dmacon: 0,
        voices,
        songend: false,
    };
    p.song_pos = p.song_table;
    p.positions_left = song_len;
    p.load_pattern(&m.bus);
    Some(Box::new(p))
}

/// Colour clocks from the host's call of the original's InitPlayer to the song's start, beyond
/// the walk of the song (measured in the reference runs).
const INIT_CLOCKS: u64 = 6443;

/// The time the original takes from InitPlayer to the song's start (measured on the Eagleplayer's
/// runs of 48 archive modules and 33 one-change copies, exact on all): a walk of the song, 422
/// 68000 cycles a row and 27 a row per tick of the speed then (a speed set on a row counts from
/// the next row), 4 a speed set; 108 an instrument, 308 a pattern, 414 a song position; and 34
/// when the file runs on past its samples.
fn start_clocks(d: &[u8], num_inst: usize, num_patt: usize, patt_off: usize, song_off: usize, song_len: u16, data_off: usize, end: usize) -> u64 {
    let rows: Vec<usize> = (0..num_patt).map(|i| be16(d, patt_off + 6 * i) as usize).collect();
    let mut start = Vec::with_capacity(num_patt);
    let mut a = data_off;
    for r in &rows {
        start.push(a);
        a += 32 * r;
    }
    let mut c: u64 = 108 * num_inst as u64 + 308 * num_patt as u64 + 414 * song_len as u64;
    let mut wait = 6u64;
    for k in 0..song_len as usize {
        let p = be16(d, song_off + 2 * k) as usize;
        if p >= num_patt {
            continue;
        }
        for r in 0..rows[p] {
            c += 422 + 27 * wait;
            for v in 0..4 {
                let s = d.get(start[p] + 32 * r + 8 * v + 2).copied().unwrap_or(0) & 15;
                if s != 0 {
                    wait = s as u64;
                    c += 4;
                }
            }
        }
    }
    if d.len() > end {
        c += 34;
    }
    INIT_CLOCKS + (c + 1) / 2
}

impl Replayer for JamCracker {
    fn format(&self) -> &'static str {
        "JamCracker"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t.t0 = hw.bus.clock;
        self.t.acc = 0;
        self.t.counts.clear();
        if let Some(f) = &mut self.t.fit {
            use std::io::Write;
            let _ = writeln!(f, "{} tick", hw.bus.clock);
        }
        self.t.v = 4;
        self.t.c(K::TickStart);
        self.wait_cnt = self.wait_cnt.wrapping_sub(1);
        if self.wait_cnt == 0 {
            self.t.c(K::Row);
            self.new_note(hw);
            self.wait_cnt = self.wait;
        } else {
            self.t.c(K::NoRow);
        }
        for i in 0..4 {
            self.set_channel(i, hw);
        }
    }
}

/// A note's period. The original's table is followed by another one and then by
/// its variables, which a note past the table would read; no module seen does.
fn period(index: usize) -> u16 {
    PERIODS.get(index).copied().unwrap_or(135)
}

impl JamCracker {
    /// The pattern at the song position: its row count (a byte, so 256 rows are 0)
    /// and first row.
    fn load_pattern(&mut self, bus: &crate::bus::Bus) {
        let e = self.patt_table.wrapping_add(bus.r16(self.song_pos) as u32 * 6);
        self.note_cnt = bus.r8(e + 1);
        self.row = bus.r32(e + 2);
    }

    fn new_note(&mut self, hw: &mut Hw) {
        let row = self.row;
        self.row = self.row.wrapping_add(32);
        self.note_cnt = self.note_cnt.wrapping_sub(1);
        if self.note_cnt == 0 {
            self.t.c(K::PatternEnd);
            self.song_pos = self.song_pos.wrapping_add(2);
            self.positions_left = self.positions_left.wrapping_sub(1);
            if self.positions_left == 0 {
                self.t.c(K::SongWrap);
                // the original tells the host of the song end: a call, and the time counts on from it
                self.t.call();
                self.songend = true;
                self.song_pos = self.song_table;
                self.positions_left = self.song_len;
            }
            self.load_pattern(hw.bus);
        }

        self.tmp_dmacon = 0;
        for i in 0..4 {
            self.nw_note(row + 8 * i as u32, i, hw);
        }
        self.t.v = 4;
        self.t.c(K::DmaOffPre);
        self.t.w(hw, "off");
        hw.dmacon(self.tmp_dmacon);
        // Paula stops a playing voice only at the end of its word, and restarts it
        // only once stopped: the time the original takes between DMA off and on
        // decides which notes retrigger
        for i in 0..4 {
            self.t.v = i;
            let v = &mut self.voices[i];
            if self.tmp_dmacon & v.dmacon != 0 {
                self.t.c(K::DmaNote);
                self.t.w(hw, "loc");
                hw.loc(i, v.addr);
                self.t.c(K::DmaNoteLen);
                self.t.w(hw, "len");
                hw.len(i, v.len);
                self.t.c(K::DmaNotePer);
                self.t.w(hw, "per");
                hw.per(i, v.pers[0]);
                if v.flags & LOOP == 0 {
                    self.t.c(K::OneShot);
                    v.addr = self.silence;
                    v.len = 1;
                } else {
                    self.t.c(K::Looped);
                }
            } else {
                self.t.c(K::DmaNoNote);
            }
        }
        self.t.v = 4;
        self.t.c(K::DmaOnPre);
        self.t.w(hw, "on");
        hw.dmacon(0x8000 | self.tmp_dmacon);
        // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does: to
        // 10 lines past the current one
        let now = hw.bus.clock;
        hw.wait((now / LINE + 10) * LINE - now);
        self.t.t0 = hw.bus.clock;
        // every voice's loop, whether a note started on it or not
        for i in 0..4 {
            self.t.v = i;
            let (a, l) = (self.voices[i].addr, self.voices[i].len);
            self.t.c(K::LoopLoc);
            self.t.w(hw, "loc");
            hw.loc(i, a);
            self.t.c(K::LoopLen);
            self.t.w(hw, "len");
            hw.len(i, l);
        }
    }

    /// One voice's part of the row at `a`.
    fn nw_note(&mut self, a: u32, i: usize, hw: &mut Hw) {
        let bus = &*hw.bus;
        let b: [u8; 8] = std::array::from_fn(|k| bus.r8(a + k as u32));
        let [note, instr, speed, arp, vib, phase, vol, porta] = b;
        let v = &mut self.voices[i];
        let t = &mut self.t;
        t.v = i;
        t.c(K::NwVoice);

        if note == 0 {
            t.c(K::NoteNone);
        } else {
            let index = note as usize - 1;
            if speed & SPEED_PORTA_TO_NOTE != 0 {
                t.c(if porta == 0 { K::NotePortaIdle } else { K::NotePorta });
                v.por_level = period(index) as i16;
            } else {
                t.c(K::Note);
                self.tmp_dmacon |= v.dmacon;
                v.per_index = index;
                v.pers = [period(index); 3];
                v.por = 0;
                // the instrument's record, wherever its number points (as the
                // original, which does not check it against the instrument count)
                let rec = self.inst_table.wrapping_add((instr as i8 as i16 as u16 as u32) * 40);
                let addr = bus.r32(rec + 0x24);
                if addr == 0 {
                    t.c(K::InstZero);
                    v.addr = self.silence;
                    v.len = 1;
                    v.flags = 0;
                } else {
                    let flags = bus.r8(rec + 0x1f);
                    if flags & AM == 0 {
                        t.c(K::InstPlain);
                        v.addr = addr;
                        v.len = (bus.r32(rec + 0x20) >> 1) as u16;
                    } else {
                        t.c(K::InstAm);
                        // NostalgicPlayer copies the waveform to a buffer; the
                        // original plays (and phases) it in place
                        v.addr = addr.wrapping_add(v.wave_offset as u32);
                        v.len = 0x20;
                    }
                    v.flags = flags;
                    v.vol = v.vol_level as i16;
                }
            }
        }

        if speed & 15 != 0 {
            t.c(K::Speed);
            self.wait = speed & 15;
        } else {
            t.c(K::SpeedNone);
        }

        let pi = v.per_index;
        if arp == 0 {
            t.c(K::ArpNone);
        } else {
            if arp == 255 {
                t.c(K::ArpOff);
                v.pers = [period(pi); 3];
            } else {
                t.c(K::Arp);
                v.pers[2] = period(pi + (arp & 15) as usize);
                v.pers[1] = period(pi + (arp >> 4) as usize);
                v.pers[0] = period(pi);
            }
        }

        if vib == 0 {
            t.c(K::VibNone);
        } else {
            if vib == 255 {
                t.c(K::VibOff);
                v.vib = 0;
                v.delta_vib = 0;
                v.vib_cnt = 0;
            } else {
                t.c(K::Vib);
                v.vib = 0;
                v.delta_vib = (vib & 15) as i16;
                v.vib_max = vib >> 4;
                v.vib_cnt = vib >> 5;
            }
        }

        if phase == 0 {
            t.c(K::PhaseNone);
        } else {
            t.c(if phase == 255 { K::PhaseOff } else { K::Phase });
            v.phase = 0;
            v.delta_phase = if phase == 255 { -1 } else { (phase & 15) as i16 };
        }

        // the original sets only the low bytes of the volume words
        if vol == 255 {
            t.c(K::VolOff);
            v.delta_vol = 0;
        } else if speed & SPEED_SET_VOLUME != 0 {
            t.c(if vol == 0 { K::VolSetZero } else { K::VolSet });
            v.vol = (v.vol & !0xff) | vol as i16;
            v.vol_level = (v.vol_level & !0xff) | vol as u16;
            v.delta_vol = 0;
        } else if vol != 0 {
            t.c(if vol & 0x80 != 0 { K::VolSlideDown } else { K::VolSlide });
            let d = (vol & 0x7f) as i16;
            v.delta_vol = if vol & 0x80 != 0 { -d } else { d };
        } else {
            t.c(K::VolNone);
        }

        if porta == 0 {
            t.c(K::PortaNone);
        } else {
            if porta == 255 {
                t.c(K::PortaOff);
                v.por = 0;
                v.delta_por = 0;
            } else {
                v.por = 0;
                // byte arithmetic, as the original: a slide of $80 or more towards
                // a note is negative (NostalgicPlayer keeps it positive)
                let d = if speed & SPEED_PORTA_TO_NOTE != 0 {
                    if v.por_level > v.pers[0] as i16 {
                        t.c(K::PortaToUp);
                        porta
                    } else {
                        t.c(K::PortaToDown);
                        porta.wrapping_neg()
                    }
                } else if porta & 0x80 == 0 {
                    t.c(K::PortaDown);
                    v.por_level = 135;
                    porta.wrapping_neg()
                } else {
                    t.c(K::PortaUp);
                    v.por_level = 1019;
                    porta & 0x7f
                };
                v.delta_por = d as i8 as i16;
            }
        }
    }

    fn set_channel(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        let t = &mut self.t;
        t.v = i;
        t.c(K::Chan);
        // (all three periods are never 0: the first one is a note's)
        for _ in 0..3 {
            if v.pers[0] != 0 {
                break;
            }
            t.c(K::PerRot);
            v.pers.rotate_left(1);
        }

        let mut per = (v.pers[0] as i16).wrapping_add(v.por);
        if v.por < 0 {
            t.c(if per < v.por_level { K::PorNegMax } else { K::PorNeg });
            per = per.max(v.por_level);
        } else if v.por != 0 {
            t.c(if per > v.por_level { K::PorPosMin } else { K::PorPos });
            per = per.min(v.por_level);
        } else {
            t.c(K::PorZero);
        }
        per = per.wrapping_add(v.vib);
        t.c(if per < 135 { K::PerLo } else if per > 1019 { K::PerHi } else { K::PerIn });
        per = per.clamp(135, 1019);
        t.w(hw, "per");
        hw.per(i, per as u16);
        v.pers.rotate_left(1);

        let por = v.por.wrapping_add(v.delta_por);
        t.c(if por < -1019 { K::PorAddLo } else if por > 1019 { K::PorAddHi } else { K::PorAddIn });
        v.por = por.clamp(-1019, 1019);

        if v.vib_cnt != 0 {
            t.c(K::VibStep);
            v.vib = v.vib.wrapping_add(v.delta_vib);
            v.vib_cnt -= 1;
            if v.vib_cnt == 0 {
                t.c(K::VibTurn);
                v.delta_vib = v.delta_vib.wrapping_neg();
                v.vib_cnt = v.vib_max;
            }
        } else {
            t.c(K::VibIdle);
        }

        // the original's volume routine masks the volume to 7 bits and caps it at 64
        t.c(K::VolWrite);
        if (v.vol as u16) & 0x7f >= 0x40 {
            t.c(K::VolCap);
        }
        t.w(hw, "vol");
        hw.vol(i, ((v.vol as u16) & 0x7f).min(0x40));
        let vol = v.vol.wrapping_add(v.delta_vol);
        t.c(if vol < 0 { K::VolLo } else if vol > 64 { K::VolHi } else { K::VolIn });
        v.vol = vol.clamp(0, 64);

        // NostalgicPlayer tests the loop flag here; the original the AM flag
        if v.flags & AM == 0 {
            t.c(K::NoAm);
        } else if v.delta_phase == 0 {
            t.c(K::AmNoPhase);
        } else {
            t.c(K::AmPhase);
            if v.delta_phase < 0 {
                t.c(K::AmPhaseNeg);
                v.delta_phase = 0;
            }
            // the waveform at the voice's offset in the sample becomes the mean
            // of the sample's start and the start shifted by phase / 4
            let out = v.addr;
            let src = out.wrapping_sub(v.wave_offset as u32);
            let shifted = src.wrapping_add(v.phase as u32 / 4);
            for k in 0..64 {
                let x = hw.bus.r8(src + k) as i8 as i16 + hw.bus.r8(shifted + k) as i8 as i16;
                m68k::AddressBus::write_byte(&mut *hw.bus, out + k, (x >> 1) as u8);
            }
            v.phase = v.phase.wrapping_add(v.delta_phase as u16);
            if v.phase as i16 >= 0x100 {
                t.c(K::AmPhaseWrap);
                v.phase -= 0x100;
            }
        }
        t.c(K::ChanEnd);
        t.v = 4;
    }
}

include!("jamcracker_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// Two instruments (a looped sample and a one-shot one), one pattern of two
    /// rows, one position.
    fn module() -> Vec<u8> {
        let mut d = b"BeEp".to_vec();
        d.extend([0, 2]);
        for (flags, size) in [(1u8, 64u32), (0, 32)] {
            let mut inst = vec![0u8; 31];
            inst.push(flags);
            inst.extend(size.to_be_bytes());
            inst.extend([0; 4]);
            d.extend(inst);
        }
        d.extend([0, 1, 0, 2, 0, 0, 0, 0]); // 1 pattern of 2 rows
        d.extend([0, 1, 0, 0]); // 1 position: pattern 0
        // row 0: voice 0 note 13 with instrument 0 at volume 48, voice 1 note 1 with
        // instrument 1; row 1 empty
        d.extend([13, 0, 0x80, 0, 0, 0, 48, 0, 1, 1, 0, 0, 0, 0, 0, 0]);
        d.extend([0; 16 + 32]);
        d.extend((0..96).map(|i| i as u8));
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.jam"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "JamCracker");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // DMA off and on for the two notes, then off and on with no note
        assert_eq!(&reg(0xDFF096)[..4], &[0x0003, 0x8003, 0x0000, 0x8000]);
        // voice 0: its note's period and length, then the loop again
        assert_eq!(&reg(0xDFF0A4)[..2], &[32, 32]);
        assert_eq!(reg(0xDFF0A6)[..2], [super::PERIODS[12]; 2]);
        assert_eq!(reg(0xDFF0A8)[0], 48);
        // voice 1: the one-shot sample, then one word of silence
        assert_eq!(&reg(0xDFF0B4)[..2], &[16, 1]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
