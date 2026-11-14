//! DigiBooster 1.x (`DIGI Booster module`), ported from NostalgicPlayer's
//! DigiBooster player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer). DigiBooster Pro (`DBM0`) is another format and not
//! played here.
//!
//! The module has 8 channels, which the original mixes in software; the
//! Eagleplayer hands them to the host's Eagleplayer amplifier (8 virtual voices,
//! poked as Paula's channels are: `EPAMB_Direct`), left, left, right, right,
//! right, right, left, left. This port pokes the same voices (`Hw::voice_*`) and
//! the host's mixer plays them; it does not mix.
//!
//! Where the Eagleplayer's pokes differ from what NostalgicPlayer asks of its
//! mixer this follows the Eagleplayer (checked by its virtual voices' write log):
//! a sample plays up to the end of its loop first, the loop is set after DMA is
//! on, a period is stretched by 2^(-finetune/96), every voice's volume goes out
//! every tick and its period and loop on the ticks the 68k player writes them.
//! Where it does not know the original's writes (which effects write the period
//! on which tick) it writes what the sound needs, so the log differs there while
//! the sound does not.
//!
//! Not done: the robot effect (8xx) plays the sample plain, and E3x (backward
//! play) forwards. DigiBooster Pro (`DBM0`) is another format, left out.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Digibooster", detect, new };

include!("digibooster_tables.rs");

const ARPEGGIO: [u8; 32] = [
    0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1,
];

const VIBRATO_SIN: [u8; 32] = [
    0x00, 0x18, 0x31, 0x4a, 0x61, 0x78, 0x8d, 0xa1, 0xb4, 0xc5, 0xd4, 0xe0, 0xeb, 0xf4, 0xfa, 0xfd, 0xff, 0xfd, 0xfa,
    0xf4, 0xeb, 0xe0, 0xd4, 0xc5, 0xb4, 0xa1, 0x8d, 0x78, 0x61, 0x4a, 0x31, 0x18,
];

/// Decimal to the pattern break's row.
fn hex(i: u8) -> i8 {
    let i = i as usize;
    ((i / 16) * 10 + (i % 16)).min(63) as i8
}

/// The Eagleplayer's code paths between two of its host calls (ENPP_Poke*, ENPP_DMAMask), for
/// the times of its writes: each call lands at the 68000 cycles its code took since the previous
/// call (or the timer interrupt's start), halved and rounded down. The effect classes are the
/// effect number, and $E0 + x for Exy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum C {
    /// The interrupt's start, and the row's work on a tick that takes one.
    Tick,
    Row,
    /// Each voice, on a tick after a row and on the others.
    VoiceRow,
    Voice,
    /// The tick after a row: the period again after an empty row, a note's DMA off, length,
    /// address and period, the effect's period and the volume.
    RowA,
    /// A row's sample number, a note's DMA off, and more for a sample whose loop starts
    /// past its start.
    Sample,
    NoteDma,
    NoteLoopStart,
    NoteOffset,
    NoteNine,
    NoteLen,
    NoteLoc,
    NotePer,
    NoNote,
    RowPer(u8),
    RowVol(u8),
    /// The other ticks: the effect's period and the volume.
    Per(u8),
    Vol(u8),
    DmaOn,
    Clamp,
    GlissTarget(u8),
    GlissDown,
    Gliss(u8),
    OffsetNear,
    /// A row's Fxx of 32 or more: the timer's new period.
    Tempo,
    SlideDown,
    /// To the wait for the beam after a row, and each voice's loop.
    WaitStart,
    LoopLoc,
    LoopLen,
    LoopLenLoop,
    LoopLenEnd,
}

impl C {
    /// 68000 cycles of each path, fitted as integers to the Eagleplayer's write log.
    fn cycles(self) -> u64 {
        match self {
            C::Clamp => 16,
            C::DmaOn => 298,
            C::GlissDown => 16,
            C::LoopLen => 209,
            C::LoopLenEnd => 209,
            C::LoopLenLoop => 205,
            C::LoopLoc => 208,
            C::NoNote => 68,
            C::NoteDma => 664,
            C::NoteLen => 295,
            C::NoteLoc => 182,
            C::NoteLoopStart => 64,
            C::NoteNine => 4,
            C::NoteOffset => 237,
            C::NotePer => 211,
            C::OffsetNear => 34,
            C::Per(1) => 245,
            C::Per(10) => 258,
            C::Per(12) => 258,
            C::Per(13) => 258,
            C::Per(15) => 258,
            C::Per(2) => 264,
            C::Per(3) => 533,
            C::Per(4) => 516,
            C::Per(6) => 457,
            C::Per(9) => 258,
            C::Row => 193,
            C::RowA => 196,
            C::RowPer(0) => 378,
            C::RowPer(1) => 378,
            C::RowPer(10) => 378,
            C::RowPer(2) => 378,
            C::RowPer(225) => 566,
            C::RowPer(226) => 585,
            C::RowPer(3) => 378,
            C::RowPer(4) => 378,
            C::RowPer(5) => 378,
            C::RowPer(6) => 378,
            C::RowVol(0) => 383,
            C::RowVol(1) => 383,
            C::RowVol(10) => 383,
            C::RowVol(12) => 634,
            C::RowVol(13) => 695,
            C::RowVol(15) => 660,
            C::RowVol(2) => 383,
            C::RowVol(224) => 666,
            C::RowVol(225) => 383,
            C::RowVol(226) => 383,
            C::RowVol(227) => 813,
            C::RowVol(228) => 813,
            C::RowVol(230) => 745,
            C::RowVol(231) => 750,
            C::RowVol(234) => 855,
            C::RowVol(235) => 858,
            C::RowVol(3) => 383,
            C::RowVol(4) => 383,
            C::RowVol(5) => 383,
            C::RowVol(6) => 383,
            C::RowVol(9) => 616,
            C::Sample => 496,
            C::SlideDown => 8,
            C::Tempo => 497,
            C::Tick => 123,
            C::Voice => 410,
            C::VoiceRow => 181,
            C::Vol(0) => 189,
            C::Vol(1) => 275,
            C::Vol(10) => 429,
            C::Vol(12) => 333,
            C::Vol(13) => 333,
            C::Vol(15) => 333,
            C::Vol(2) => 275,
            C::Vol(224) => 474,
            C::Vol(225) => 462,
            C::Vol(226) => 481,
            C::Vol(227) => 621,
            C::Vol(228) => 621,
            C::Vol(230) => 500,
            C::Vol(231) => 558,
            C::Vol(234) => 576,
            C::Vol(235) => 595,
            C::Vol(3) => 327,
            C::Vol(4) => 349,
            C::Vol(5) => 484,
            C::Vol(6) => 471,
            C::Vol(9) => 333,
            C::WaitStart => 0,
            C::GlissTarget(i) => 466 + 44 * i as u64,
            C::Gliss(g) => g as u64,
            _ => 0,
        }
    }
}

/// The trace of code paths for fitting their cycles (DIGI_TRACE=file): each host call's
/// clock, what it writes, and the paths since the previous one.
#[derive(Default)]
struct Trace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    pend: Vec<C>,
}

/// Raster lines the Eagleplayer waits for after a row's DMA on, and the colour clock of the
/// line where its next write comes (measured: always 130).
const WAIT_LINES: u64 = 10;
const GLISS_FIRST: u64 = 28;
const GLISS_REACH_DOWN: u64 = 36;
const GLISS_REACH_UP: u64 = 46;
/// The vibrato's above its middle, and its row giving a speed (measured).
const VIB_UP: u64 = 10;
const VIB_MOVE: u64 = 12;
const WAIT_PHASE: u64 = 26;

const E_ARPEGGIO: u8 = 0;
const E_PORTAMENTO_UP: u8 = 1;
const E_PORTAMENTO_DOWN: u8 = 2;
const E_GLISSANDO: u8 = 3;
const E_VIBRATO: u8 = 4;
const E_GLISSANDO_VOLSLIDE: u8 = 5;
const E_VIBRATO_VOLSLIDE: u8 = 6;
const E_ROBOT: u8 = 8;
const E_SAMPLE_OFFSET: u8 = 9;
const E_VOLUME_SLIDE: u8 = 10;
const E_SONG_REPEAT: u8 = 11;
const E_SET_VOLUME: u8 = 12;
const E_PATTERN_BREAK: u8 = 13;
const E_EXTRA: u8 = 14;
const E_SET_SPEED: u8 = 15;

const X_FILTER: u8 = 0x00;
const X_FINE_SLIDE_UP: u8 = 0x10;
const X_FINE_SLIDE_DOWN: u8 = 0x20;
const X_BACKWARD: u8 = 0x30;
const X_CHANNEL_ON_OFF: u8 = 0x50;
const X_LOOP: u8 = 0x60;
const X_SAMPLE_OFFSET: u8 = 0x80;
const X_RETRACE: u8 = 0x90;
const X_FINE_VOL_UP: u8 = 0xa0;
const X_FINE_VOL_DOWN: u8 = 0xb0;
const X_CUT: u8 = 0xc0;
const X_PATTERN_DELAY: u8 = 0xe0;

fn detect(d: &[u8]) -> bool {
    d.len() >= 1572 && d.starts_with(b"DIGI Booster module\0")
}

#[derive(Clone, Copy, Default)]
struct TrackLine {
    period: u16,
    sample: u8,
    effect: u8,
    arg: u8,
}

/// A period stretched by 12 cents per step of the fine tune, 2^(-ft/100), and rounded down
/// (measured: 285 at +2 is 281, 170 at -4 is 174): not by NostalgicPlayer's tables, which
/// round to ProTracker's.
fn tune(period: u16, ft: i8) -> u16 {
    if ft == 0 {
        return period;
    }
    (period as f64 * 2f64.powf(-(ft as f64) / 100.0)) as u16
}

fn track_line(data: u32) -> TrackLine {
    TrackLine {
        period: ((data >> 16) & 0x0fff) as u16,
        sample: (((data >> 24) & 0xf0) | ((data >> 12) & 0x0f)) as u8,
        effect: ((data >> 8) & 0x0f) as u8,
        arg: data as u8,
    }
}

#[derive(Clone, Default)]
struct Sample {
    length: u32,
    loop_start: u32,
    loop_len: u32,
    /// The loop's start as the module gives it (where a sample without a loop goes on).
    raw_loop_start: u32,
    raw_loop_len: u32,
    volume: u8,
    fine_tune: i8,
    /// Offset of its data in the module.
    data: Option<usize>,
}

type Pattern = Vec<[TrackLine; 8]>;

struct Song {
    channels: usize,
    length: u8,
    orders: [u8; 128],
    name: String,
    samples: Vec<Sample>,
    patterns: Vec<Pattern>,
}

fn load(d: &[u8]) -> Option<Song> {
    if !detect(d) {
        return None;
    }
    let (channels, packed) = (d[25] as usize, d[26] != 0);
    let patterns_n = d[46].wrapping_add(1) as usize;
    let length = d[47].wrapping_add(1);
    let mut orders = [0u8; 128];
    orders.copy_from_slice(&d[48..176]);
    let mut samples = Vec::with_capacity(31);
    for i in 0..31 {
        let mut s = Sample {
            length: be32(d, 176 + 4 * i),
            ..Default::default()
        };
        let (ls, ll) = (be32(d, 300 + 4 * i), be32(d, 424 + 4 * i));
        s.volume = d[548 + i];
        // the Eagleplayer tunes the samples of every version (NostalgicPlayer leaves those of
        // 1.0 to 1.3 untuned)
        s.fine_tune = d[579 + i] as i8;
        (s.raw_loop_start, s.raw_loop_len) = (ls, ll);
        let (mut ls, mut ll) = (ls, ll);
        if ls > s.length || ll == 0 {
            ls = 0;
            ll = 0;
        } else if ls.wrapping_add(ll) >= s.length {
            ll = s.length - ls;
        }
        (s.loop_start, s.loop_len) = (ls, ll);
        samples.push(s);
    }
    let name = d[610..642].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect::<String>().trim().to_string();
    let mut patterns = Vec::with_capacity(patterns_n);
    let mut p = 1572;
    for _ in 0..patterns_n {
        let mut pat: Pattern = vec![[TrackLine::default(); 8]; 64];
        if packed {
            let mut length = be16(d, p) as i32;
            p += 2;
            if p + 64 > d.len() {
                return None;
            }
            let masks = &d[p..p + 64];
            p += 64;
            length -= 64;
            for j in 0..64 {
                for (m, k) in (0..8).zip([0x80u8, 0x40, 0x20, 0x10, 8, 4, 2, 1]) {
                    let mut data = 0;
                    if masks[j] & k != 0 {
                        if p + 4 > d.len() {
                            return None;
                        }
                        data = be32(d, p);
                        p += 4;
                        length -= 4;
                    }
                    pat[j][m] = track_line(data);
                }
            }
            if length != 0 {
                return None;
            }
        } else {
            for j in 0..8 {
                for k in 0..64 {
                    if p + 4 > d.len() {
                        return None;
                    }
                    pat[k][j] = track_line(be32(d, p));
                    p += 4;
                }
            }
        }
        patterns.push(pat);
    }
    for s in samples.iter_mut() {
        // a file cut short plays on into whatever follows it in memory, as the original's;
        // a sample of no bytes is played too, from where the next one starts (measured)
        s.data = Some(p);
        p += s.length as usize;
    }
    Some(Song { channels, length, orders, name, samples, patterns })
}

#[derive(Clone, Copy, Default)]
struct LastRound {
    period: u16,
    sample: u8,
    effect: u8,
    arg: u8,
}

#[derive(Clone, Default)]
struct Chan {
    volume: u8,
    slide_volume_old: u8,
    off_enable: bool,
    sample_offset: u8,
    retrace_count: u8,
    old_sample: u8,
    main_period: i16,
    main_volume: u8,
    play_pointer: bool,
    last: LastRound,
    loop_pattern_pos: i8,
    loop_song_pos: u8,
    loop_how_many: u8,
    backward: u8,
    /// The portamento's value, one for up and down.
    porta_old: u8,
    vibrato_period: u16,
    vibrato_value: i8,
    vibrato_old: u8,
    glissando_old_value: u8,
    glissando_enable: bool,
    glissando_old_period: u16,
    glissando_new_period: u16,
    on_off: bool,
    original_period: u16,
    old_volume: u8,
    is_active: bool,
    /// The sample whose data the channel plays (index), if it has any.
    sample_data: Option<usize>,
    start_offset: u32,
    row: (u16, u8, u8, u8),
    /// The row before was empty (no note, no effect), or there was none.
    prev_empty: bool,
    /// The period written last.
    last_per: u16,
    /// The glissando moved the period on its last step (the Eagleplayer writes it only then).
    gliss_moved: bool,
    /// ... and the period it wrote then.
    gliss_out: u16,
    /// A volume slide met its end (0 or 64) since the last volume write: 16 cycles more.
    clamped: bool,
    /// The glissando's and vibrato's extra cycles since the last write: the glissando's first
    /// tick in a row and reaching its target down or up, the vibrato's above its middle and
    /// moving (measured).
    gliss_cost: u64,
    /// The last sample offset went past half its sample.
    offset_far: bool,
    /// ... and slid down (8 cycles more than up).
    slide_down: bool,
    /// ... or slid at all.
    slid: bool,
    /// The colour clock at which a sample with no loop has played out.
    ends_at: u64,
    looped: bool,
}

pub struct DigiBooster {
    song: Song,
    base: u32,
    cia_tempo: u16,
    /// The tempo the timer runs at: the original sets the CIA timer a tick after the port's
    /// row parse (the new period counts from the second underflow after the parse).
    cia_out: u16,
    tempo: u8,
    count: u8,
    song_pos: u8,
    pattern_pos: i8,
    row: [TrackLine; 8],
    pause_vbl: u16,
    pause_enabled: bool,
    chans: Vec<Chan>,
    visited: [bool; 256],
    songend: bool,
    row_out: bool,
    /// This tick takes a row.
    parse_tick: bool,
    title: String,
    /// The clock of the last host call (or the interrupt's start), and the 68000 cycles the
    /// original spent since.
    seg: u64,
    cyc: u64,
    trace: Trace,
}

/// The colour clocks the Eagleplayer takes from loading to starting its amplifier, and from
/// there to the song's first tick (it starts the amplifier again then). Before the song it
/// walks the positions but the last, unpacking each pattern and going through its rows as
/// they play (loops, breaks, speeds); the time is a fit of the file's counts to the
/// reference's start in 238 modules (black box): the header's 2175 + 21 a pattern is exact,
/// the walk is within 4600 colour clocks on average (the loops and some commands are not
/// walked as the original does), the time to the first tick within 33.
fn start_clocks(s: &Song, packed: bool) -> (u64, u64) {
    let npat = s.patterns.len() as f64;
    let nsmp = s.samples.iter().filter(|x| x.length != 0).count() as f64;
    let mut f = [0f64; 12];
    let (pos, rows, ticks, cell, fx, ecmd, fspeed, fbpm, f0, loops, stop, delay) = (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11);
    let mut speed = 6u8;
    for k in 0..(s.length as usize).saturating_sub(1) {
        let Some(pat) = s.patterns.get(s.orders[k] as usize) else { continue };
        f[pos] += 1.0;
        let (mut rw, mut guard) = (0usize, 0);
        let (mut lp, mut cnt) = ([0usize; 8], [0u8; 8]);
        while rw < 64 && guard < 5000 {
            guard += 1;
            f[rows] += 1.0;
            let (mut nxt, mut brk) = (rw + 1, false);
            for (ch, c) in pat[rw].iter().enumerate() {
                let (e, a) = (c.effect, c.arg);
                if c.period != 0 || c.sample != 0 || e != 0 || a != 0 {
                    f[cell] += 1.0;
                }
                if e != 0 || a != 0 {
                    f[fx] += 1.0;
                    match e {
                        E_EXTRA => {
                            f[ecmd] += 1.0;
                            if a & 0xf0 == X_LOOP {
                                f[loops] += 1.0;
                            }
                        }
                        E_SET_SPEED if a == 0 => f[f0] += 1.0,
                        E_SET_SPEED if a < 32 => f[fspeed] += 1.0,
                        E_SET_SPEED => f[fbpm] += 1.0,
                        _ => {}
                    }
                }
                if e == E_PATTERN_BREAK || e == E_SONG_REPEAT {
                    brk = true;
                }
                if e == E_EXTRA && a & 0xf0 == X_LOOP {
                    match (a & 15, cnt[ch]) {
                        (0, 0) => lp[ch] = rw,
                        (0, _) => {}
                        (_, 0) => {
                            cnt[ch] = a & 15;
                            nxt = lp[ch];
                        }
                        _ => {
                            cnt[ch] -= 1;
                            if cnt[ch] != 0 {
                                nxt = lp[ch];
                            }
                        }
                    }
                }
            }
            for c in &pat[rw] {
                if c.effect == E_SET_SPEED && c.arg != 0 && c.arg < 32 {
                    speed = c.arg;
                }
            }
            for c in &pat[rw] {
                if c.effect == E_EXTRA && c.arg & 0xf0 == X_PATTERN_DELAY {
                    f[delay] += speed as f64 * (c.arg & 15) as f64;
                }
            }
            f[ticks] += speed as f64;
            if brk {
                f[stop] += 1.0;
                break;
            }
            rw = nxt;
        }
    }
    const W: [f64; 12] = [13445.046, 647.649, 13.772, 0.506, 33.951, 27.672, 16.957, 39.694, 1967.99, -298.774, 380.002, -0.688];
    let walk: f64 = f.iter().zip(W).map(|(x, w)| x * w).sum();
    let amp = 2175.0 + 21.0 * npat + 5560.538 + 32.464 * npat + 187.331 * nsmp + walk;
    // to the first tick: exact for unpacked modules, within 33 colour clocks for packed ones
    let tail = if packed { 16832.0 + 35.772 * npat + 3.45 * nsmp } else { 11190.0 + 15.0 * npat + 2.0 * nsmp };
    (amp.max(0.0).round() as u64, tail.round() as u64)
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let song = load(d)?;
    // the time to the amplifier's start and to the song's: the port's first tick comes
    // 100 colour clocks after its start
    let (amp, tail) = start_clocks(&song, d[26] != 0);
    m.bus.clock += amp.saturating_sub(100);
    // left, left, right, right, right, right, left, left; the original's AudioStructs are
    // its own, so the amplifier's buffers come right after the module (what a sample that
    // plays past the module's end plays, measured)
    use crate::eagle::epamb;
    if !crate::amplifier::start_panned(m, 8, epamb::DIRECT | epamb::BIT8 | epamb::CHIP_RAM, &[1, 1, -1, -1, -1, -1, 1, 1], 100) {
        return None;
    }
    m.bus.clock += tail;
    crate::amplifier::restart(&mut m.bus);
    let channels = song.channels.clamp(1, 8);
    let title = song.name.clone();
    let mut o = DigiBooster {
        base,
        cia_tempo: 125,
        cia_out: 125,
        tempo: 6,
        count: 2,
        song_pos: 0,
        pattern_pos: 0,
        row: [TrackLine::default(); 8],
        pause_vbl: 0,
        pause_enabled: false,
        chans: vec![Chan { prev_empty: true, ..Chan::default() }; channels],
        visited: [false; 256],
        songend: false,
        row_out: false,
        parse_tick: false,
        title,
        seg: 0,
        cyc: 0,
        trace: Trace {
            out: std::env::var_os("DIGI_TRACE").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new),
            pend: Vec::new(),
        },
        song,
    };
    o.visited[0] = true;
    Some(Box::new(o))
}

impl Replayer for DigiBooster {
    fn format(&self) -> &'static str {
        "Digibooster"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        (1_773_447 / self.cia_out.max(1) as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.seg = hw.bus.clock;
        self.cyc = 0;
        self.trace.pend.clear();
        if let Some(o) = &mut self.trace.out {
            use std::io::Write;
            let _ = writeln!(o, "T {}", hw.bus.clock);
        }
        self.play(hw);
    }
}

impl DigiBooster {
    fn play(&mut self, hw: &mut Hw) {
        self.cia_out = self.cia_tempo;
        let current = self.song_pos;
        let n = self.chans.len();
        if self.count >= self.tempo {
            if self.tempo != 0 && self.pattern_pos == 64 {
                self.pattern_pos = 0;
                self.song_pos = self.song_pos.wrapping_add(1);
            }
            if self.song_pos >= self.song.length {
                self.song_pos = 0;
                self.pattern_pos = 0;
            }
            if self.tempo == 0 {
                self.song_pos = 0;
                self.pattern_pos = 0;
                self.tempo = 6;
                for c in self.chans.iter_mut() {
                    c.loop_pattern_pos = 0;
                    c.loop_song_pos = 0;
                    c.loop_how_many = 0;
                }
            }
            let pat = self.song.orders[(self.song_pos & 0x7f) as usize] as usize;
            if let Some(p) = self.song.patterns.get(pat) {
                self.row = p[(self.pattern_pos.clamp(0, 63)) as usize];
            }
        }
        self.cost(C::Tick);
        if self.row_out {
            self.cost(C::Row);
        }
        let mut dma_on = 0u32;
        for v in 0..n {
            self.cost(if self.row_out { C::VoiceRow } else { C::Voice });
            self.play_voice(hw, v, &mut dma_on);
        }
        if dma_on != 0 {
            self.cost(C::DmaOn);
            self.call(hw, "dma");
            hw.voice_dma(dma_on, true);
        }
        if self.row_out {
            // the original waits for the beam before it writes the loops
            if dma_on == 0 {
                // the time of the DMA on goes by with no voice to start (measured: the wait
                // counts its lines from 149 colour clocks after the last volume)
                self.cost(C::DmaOn);
            }
            self.cost(C::WaitStart);
            hw.bus.clock = self.seg + self.cyc / 2;
            hw.wait_lines(WAIT_LINES, WAIT_PHASE);
            (self.seg, self.cyc) = (hw.bus.clock, 0);
            self.trace.pend.clear();
            self.write_loops(hw);
        }
        let parsed = self.tempo != 0 && self.count >= self.tempo;
        self.parse_tick = parsed;
        for v in 0..n {
            let row = self.row[v];
            self.parse_voice(v, row);
        }
        self.row_out = parsed;
        if self.pause_vbl > 0 {
            self.pause_enabled = true;
            self.pause_vbl -= 1;
        }
        if self.count >= self.tempo {
            self.count = 0;
            if self.pause_vbl == 0 {
                self.pattern_pos = self.pattern_pos.wrapping_add(1);
                self.pause_enabled = false;
            }
        }
        self.count = self.count.wrapping_add(1);
        if current != self.song_pos {
            if self.visited[self.song_pos as usize] && self.chans.iter().all(|c| c.loop_how_many == 0) {
                self.songend = true;
            }
            self.visited[self.song_pos as usize] = true;
        }
    }

    fn sample_addr(&self, idx: usize, off: u32) -> u32 {
        self.base + self.song.samples[idx].data.unwrap_or(0) as u32 + off
    }

    /// Plays the voice from what the previous tick worked out, writing what the
    /// Eagleplayer writes (measured on its virtual voices' write log): on the tick
    /// after a row, the period once more if the row before it was empty (no note, no
    /// effect), a new note's sample and period, and the period again for the effects
    /// that work on it (0-7, A, E1x, E2x); on the other ticks the period for every
    /// effect but 0 and Ex; the volume on every tick.
    fn play_voice(&mut self, hw: &mut Hw, v: usize, dma_on: &mut u32) {
        let mut c = std::mem::take(&mut self.chans[v]);
        let row_tick = self.row_out;
        let (eff, x) = (c.row.2, c.row.3 & 0xf0);
        // E1x and E2x: the period before the slide, and after it
        let slide = eff == E_EXTRA && matches!(x, X_FINE_SLIDE_UP | X_FINE_SLIDE_DOWN);
        let again = row_tick && c.prev_empty;
        let fx_per = if row_tick {
            eff <= E_VIBRATO_VOLSLIDE || eff == 7 || eff == E_VOLUME_SLIDE || slide
        } else if matches!(eff, E_GLISSANDO | E_GLISSANDO_VOLSLIDE) {
            c.gliss_moved
        } else {
            eff != 0 && eff != E_EXTRA
        };
        if row_tick {
            c.prev_empty = c.row.0 == 0 && eff == 0;
        }
        let mut retrig = false;
        if c.main_period > 0 && c.off_enable {
            c.off_enable = false;
            retrig = !(c.play_pointer && matches!(c.last.effect, E_GLISSANDO | E_GLISSANDO_VOLSLIDE));
        }
        let per = c.main_period.max(0) as u16;
        let class = if eff == E_EXTRA { 0xe0 | x >> 4 } else { eff };
        if again {
            self.cost(C::RowA);
            self.call(hw, "per");
            hw.voice_per(v, c.last_per);
        }
        if row_tick && c.row.1 != 0 {
            self.cost(C::Sample);
        }
        if retrig {
            if let Some(i) = c.sample_data {
                // Paula plays up to the end of the loop first
                let smp = &self.song.samples[i];
                // to the loop's end as the module gives it, even past the sample's end
                // (measured), or to the sample's end when it gives none
                let end = match smp.raw_loop_start + smp.raw_loop_len {
                    0 => smp.length,
                    e => e,
                };
                let len = end.saturating_sub(c.start_offset);
                let loc = self.sample_addr(i, c.start_offset);
                c.looped = smp.loop_len > 0;
                let late_loop = smp.raw_loop_start != 0;
                self.cost(C::NoteDma);
                if late_loop {
                    self.cost(C::NoteLoopStart);
                }
                if eff == E_SAMPLE_OFFSET {
                    self.cost(C::NoteOffset);
                }
                // sample 9's notes take 4 cycles more (measured in 7 modules, every note of it)
                if c.row.1 == 9 {
                    self.cost(C::NoteNine);
                }
                self.call(hw, "dmaoff");
                c.ends_at = hw.bus.clock + len as u64 * per as u64;
                hw.voice_dma(1 << v, false);
                self.cost(C::NoteLen);
                if len / 2 == 0 {
                    self.cost(C::NoteNine);
                }
                self.call(hw, "len");
                hw.voice_len(v, (len / 2) as u16);
                self.cost(C::NoteLoc);
                self.call(hw, "loc");
                hw.voice_loc(v, loc);
                *dma_on |= 1 << v;
            }
            let p = if slide { c.original_period } else { per };
            self.cost(C::NotePer);
            self.call(hw, "per");
            hw.voice_per(v, p);
            c.last_per = p;
        } else if row_tick {
            self.cost(C::NoNote);
        }
        let g = std::mem::take(&mut c.gliss_cost);
        if g != 0 {
            self.cost(C::Gliss(g as u8));
        }
        if row_tick && eff == E_GLISSANDO && c.row.0 != 0 {
            // the target's place in the period table, searched from the top (measured: 44
            // cycles a note), 16 more sliding down
            let idx = PERIODS[8].iter().position(|&q| c.row.0 >= q).unwrap_or(36) as u8;
            self.cost(C::GlissTarget(idx));
            if c.last.period > c.row.0 {
                self.cost(C::GlissDown);
            }
        }
        if fx_per {
            let p = if !row_tick && matches!(eff, E_GLISSANDO | E_GLISSANDO_VOLSLIDE) { c.gliss_out } else { per };
            self.cost(if row_tick { C::RowPer(class) } else { C::Per(class) });
            self.call(hw, "per");
            hw.voice_per(v, p);
            c.last_per = p;
        }
        let vol = if c.main_period > 0 { c.main_volume.min(64) as u16 } else { 0 };
        self.cost(if row_tick { C::RowVol(class) } else { C::Vol(class) });
        if row_tick && eff == E_SAMPLE_OFFSET && !c.offset_far {
            self.cost(C::OffsetNear);
        }
        if row_tick && eff == E_SET_SPEED && c.row.3 >= 32 {
            self.cost(C::Tempo);
        }
        // a slide's row on a voice without a period: the original slides down from 0 (measured)
        let idle_slide = !row_tick && eff == E_VOLUME_SLIDE && !c.slid && c.main_period == 0 && c.row.3 < 0x10;
        if std::mem::take(&mut c.clamped) || idle_slide {
            self.cost(C::Clamp);
        }
        if std::mem::take(&mut c.slide_down) || idle_slide {
            self.cost(C::SlideDown);
        }
        c.slid = false;
        self.call(hw, "vol");
        hw.voice_vol(v, vol);
        if c.main_period != 0 {
            c.play_pointer = true;
        }
        if c.main_period == -1 {
            c.main_period = 0;
        }
        // whether the voice still plays: the original knows from the end of the sample's first block
        c.is_active = c.main_period > 0 && c.sample_data.is_some() && (c.looped || hw.bus.clock < c.ends_at);
        self.chans[v] = c;
    }

    /// The loops of all voices, written after a row: the sample's loop, or its start with
    /// no length; nothing for a voice with no sample.
    fn write_loops(&mut self, hw: &mut Hw) {
        for v in 0..8 {
            let (loc, len) = match self.chans.get(v).and_then(|c| c.sample_data) {
                Some(i) if self.chans[v].main_period != 0 => {
                    let s = &self.song.samples[i];
                    if s.loop_len > 0 {
                        (self.sample_addr(i, s.loop_start), (s.loop_len / 2) as u16)
                    } else {
                        // the loop's start as the module has it (often the sample's end),
                        // whatever offset the note played from
                        (self.sample_addr(i, s.raw_loop_start), 0)
                    }
                }
                _ => (0, 0),
            };
            self.cost(C::LoopLoc);
            self.call(hw, "loc");
            hw.voice_loc(v, loc);
            self.cost(if len != 0 { C::LoopLenLoop } else if loc != 0 { C::LoopLenEnd } else { C::LoopLen });
            self.call(hw, "len");
            hw.voice_len(v, len);
        }
    }

    fn cost(&mut self, c: C) {
        self.cyc += c.cycles();
        if self.trace.out.is_some() {
            self.trace.pend.push(c);
        }
    }

    /// A host call: its clock is the cycles since the previous one, halved and rounded down.
    fn call(&mut self, hw: &mut Hw, what: &str) {
        hw.bus.clock = self.seg + self.cyc / 2;
        (self.seg, self.cyc) = (hw.bus.clock, 0);
        if let Some(o) = &mut self.trace.out {
            use std::io::Write;
            let _ = write!(o, "{} {}", hw.bus.clock, what);
            for e in self.trace.pend.drain(..) {
                let _ = write!(o, " {:?}", e);
            }
            let _ = writeln!(o);
        }
    }

    fn parse_voice(&mut self, v: usize, row: TrackLine) {
        let mut c = std::mem::take(&mut self.chans[v]);
        c.volume = c.old_volume;
        if self.tempo != 0 && self.count >= self.tempo {
            // a vibrato's row has ended: its period back (it runs to the row's last tick)
            if c.vibrato_period != 0 {
                c.last.period = c.vibrato_period;
                c.vibrato_period = 0;
            }
            self.parse_row_data(&mut c, row);
            self.do_trigger_effects(&mut c);
        }
        // the slides go on to the row's last tick (measured: ticks 1 to speed-1 after the
        // row's, where NostalgicPlayer slides on the row's tick and stops a tick early)
        let clear = self.tempo.wrapping_sub(1) == self.count;
        let late = matches!(c.last.effect, E_PORTAMENTO_UP | E_PORTAMENTO_DOWN | E_VOLUME_SLIDE | E_GLISSANDO_VOLSLIDE | E_VIBRATO_VOLSLIDE);
        if clear && !late {
            let effect = c.last.effect;
            if effect == E_GLISSANDO_VOLSLIDE {
                c.last.effect = E_GLISSANDO;
                c.last.arg = 0;
            } else if effect == E_EXTRA {
                let x = c.last.arg & 0xf0;
                if x != X_CUT && x != X_RETRACE {
                    c.last.effect = 0;
                    c.last.arg = 0;
                }
            } else if !matches!(effect, E_ROBOT | E_GLISSANDO | E_VIBRATO | E_ARPEGGIO) {
                c.last.effect = 0;
                c.last.arg = 0;
            }
        }
        self.test_period(&mut c);
        self.effect_commands(&mut c);
        self.test_period(&mut c);
        if clear && late {
            // the glissando goes on without the slide
            c.last.effect = if c.last.effect == E_GLISSANDO_VOLSLIDE { E_GLISSANDO } else { 0 };
            c.last.arg = 0;
        }
        c.glissando_old_period = c.last.period;
        c.old_volume = c.volume;
        if c.last.period == 0 {
            if c.main_period != 0 {
                c.main_period = -1;
            }
        } else {
            c.main_period = c.last.period as i16;
            c.main_volume = c.volume;
            if c.backward == 0 {
                if c.last.effect == E_EXTRA && c.last.arg & 0xf0 == X_BACKWARD {
                    c.backward = if c.last.arg & 0x0f == 0 { 1 } else { 2 };
                }
            }
        }
        self.chans[v] = c;
    }

    fn parse_row_data(&mut self, c: &mut Chan, t: TrackLine) {
        c.row = (t.period, t.sample, t.effect, t.arg);
        let mut old_period = true;
        if !self.pause_enabled && !c.on_off && t.period != 0 {
            if t.effect == E_GLISSANDO {
                c.glissando_new_period = 0;
            }
            c.vibrato_period = 0;
            // a note starts the vibrato's wave again (measured)
            c.vibrato_value = 0;
            c.off_enable = true;
            c.original_period = self.find_period(c, t);
            c.last.period = c.original_period;
            old_period = false;
        }
        let mut sample = c.last.sample;
        let mut effect = 0u8;
        let mut arg = 0u8;
        let mut finished = false;
        'body: {
            if t.sample != 0 || t.effect != 0 || t.arg != 0 {
                effect = t.effect;
                arg = t.arg;
                if t.sample != 0 {
                    let number = if sample == 0 { c.old_sample } else { sample };
                    if old_period && number != 0 {
                        // a sample without a note: its volume, and its loop for the voice
                        // (measured: the Eagleplayer's next loop write is the new sample's)
                        c.volume = self.samples_volume(t.sample);
                        c.sample_data = self.sample_with_data(t.sample);
                        c.old_sample = t.sample;
                        sample = t.sample;
                    } else {
                        sample = t.sample;
                        if effect == E_GLISSANDO {
                            if !c.is_active {
                                sample = 0;
                                effect = 0;
                                arg = 0;
                                finished = true;
                                break 'body;
                            }
                        } else {
                            c.sample_data = self.sample_with_data(sample);
                            c.start_offset = 0;
                        }
                        c.volume = self.samples_volume(sample);
                        c.old_sample = sample;
                        c.backward = 0;
                        finished = true;
                        break 'body;
                    }
                }
            }
            if t.period != 0 {
                sample = c.old_sample;
                if matches!(effect, E_GLISSANDO | E_GLISSANDO_VOLSLIDE) {
                    if !c.is_active {
                        sample = 0;
                        effect = 0;
                        arg = 0;
                    }
                } else if sample == 0 {
                    c.sample_data = None;
                    c.start_offset = 0;
                } else {
                    c.sample_data = self.sample_with_data(sample);
                    c.start_offset = 0;
                }
            }
        }
        let _ = finished;
        c.last.sample = sample;
        c.last.effect = effect;
        c.last.arg = arg;
    }

    fn samples_volume(&self, number: u8) -> u8 {
        self.song.samples.get(number as usize - 1).map_or(0, |s| s.volume)
    }

    fn sample_with_data(&self, number: u8) -> Option<usize> {
        let i = (number as usize).checked_sub(1)?;
        self.song.samples.get(i).filter(|s| s.data.is_some()).map(|_| i)
    }

    fn find_period(&self, c: &Chan, t: TrackLine) -> u16 {
        let mut number = t.sample;
        if number == 0 {
            number = c.old_sample;
        }
        if number == 0 {
            return 0;
        }
        let Some(s) = self.song.samples.get(number as usize - 1) else { return 0 };
        tune(t.period, s.fine_tune)
    }

    /// The fine tune of sample `number` (1-based), 0 for none.
    fn fine_tune(&self, number: u8) -> i8 {
        (number as usize).checked_sub(1).and_then(|i| self.song.samples.get(i)).map_or(0, |s| s.fine_tune)
    }

    fn test_period(&self, c: &mut Chan) {
        if c.last.period == 0 {
            c.last.period = 0;
            c.last.sample = 0;
            c.last.effect = 0;
            c.last.arg = 0;
        } else if c.last.period < 113 {
            c.last.period = 113;
        }
    }

    fn do_trigger_effects(&mut self, c: &mut Chan) {
        let no_data = c.last.sample == 0
            || self.song.samples.get(c.last.sample as usize - 1).map_or(true, |s| s.data.is_none());
        if no_data {
            c.last.period = 0;
            c.last.sample = 0;
        }
        self.effect_commands2(c);
        // E40 does not stop the sample in the Eagleplayer (measured: the voice plays on)
        if c.on_off {
            c.last = LastRound::default();
        }
    }

    fn effect_commands2(&mut self, c: &mut Chan) {
        let (effect, mut arg) = (c.last.effect, c.last.arg);
        if effect == 0 && arg == 0 {
            return;
        }
        match effect {
            E_SAMPLE_OFFSET => {
                c.start_offset =
                    c.start_offset.wrapping_add(c.sample_offset as u32 * 65536 + arg as u32 * 256);
                let len = c.sample_data.map_or(0, |i| self.song.samples[i].length);
                // past half the sample (and within it) the original takes 34 cycles less
                // (measured)
                c.offset_far = c.start_offset <= len && 2 * c.start_offset > len;
                if c.start_offset > len {
                    c.start_offset = len;
                }
            }
            E_SONG_REPEAT => {
                self.pattern_pos = -1;
                let a = arg.min(127);
                if a <= self.song_pos {
                    self.songend = true;
                }
                self.song_pos = a;
            }
            E_SET_VOLUME => c.volume = arg,
            E_PATTERN_BREAK => {
                let a = arg.min(0x63);
                if self.pattern_pos != -1 {
                    self.song_pos = self.song_pos.wrapping_add(1);
                    if self.song_pos >= self.song.length {
                        self.song_pos = 0;
                    }
                }
                self.pattern_pos = hex(a) - 1;
            }
            E_SET_SPEED => {
                if arg > 31 {
                    if arg as u16 != self.cia_tempo {
                        self.cia_tempo = arg as u16;
                    }
                } else {
                    self.count = arg;
                    if arg != self.tempo {
                        self.tempo = arg;
                    }
                }
            }
            E_EXTRA => {
                let x = arg & 0xf0;
                arg &= 0x0f;
                match x {
                    X_FILTER => {
                        // the LED filter: the Eagleplayer neither writes $BFE001 nor calls
                        // ENPP_PokeCommand, so the LED bit is not touched
                    }
                    X_CHANNEL_ON_OFF => {
                        if arg == 0 {
                            c.on_off = true;
                        } else if arg == 1 {
                            c.on_off = false;
                        }
                    }
                    X_FINE_SLIDE_UP => {
                        c.last.period = c.last.period.wrapping_sub(arg as u16);
                        if c.last.period < 113 {
                            c.last.period = 113;
                        }
                        c.last.effect = 0;
                        c.last.arg = 0;
                    }
                    X_FINE_SLIDE_DOWN => {
                        c.last.period = c.last.period.wrapping_add(arg as u16);
                        if c.last.period > 856 {
                            c.last.period = 856;
                        }
                        c.last.effect = 0;
                        c.last.arg = 0;
                    }
                    X_LOOP => self.do_loop(c, arg),
                    X_SAMPLE_OFFSET => c.sample_offset = arg,
                    X_FINE_VOL_UP => {
                        c.volume = c.volume.wrapping_add(arg);
                        if c.volume >= 64 {
                            c.volume = 64;
                            c.clamped = true;
                        }
                        c.last.effect = 0;
                        c.last.arg = 0;
                    }
                    X_FINE_VOL_DOWN => {
                        c.volume = c.volume.wrapping_sub(arg);
                        if (c.volume as i8) < 0 {
                            c.volume = 0;
                            c.clamped = true;
                        }
                        c.last.effect = 0;
                        c.last.arg = 0;
                    }
                    X_PATTERN_DELAY => {
                        if !self.pause_enabled && arg != 0 {
                            self.pause_vbl = self.tempo as u16 * arg as u16 + 1;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn do_loop(&mut self, c: &mut Chan, arg: u8) {
        if arg == 0 {
            if c.loop_how_many == 0 {
                c.loop_pattern_pos = self.pattern_pos.wrapping_sub(1);
                c.loop_song_pos = self.song_pos;
            }
        } else if c.loop_how_many != 0 {
            c.loop_how_many -= 1;
            if c.loop_how_many == 0 {
                c.loop_pattern_pos = 0;
                c.loop_song_pos = 0;
            } else {
                self.pattern_pos = c.loop_pattern_pos;
                self.song_pos = c.loop_song_pos;
            }
        } else {
            c.loop_how_many = arg;
            self.pattern_pos = c.loop_pattern_pos;
            self.song_pos = c.loop_song_pos;
        }
    }

    fn effect_commands(&mut self, c: &mut Chan) {
        let (effect, arg) = (c.last.effect, c.last.arg);
        if effect == 0 && arg == 0 {
            return;
        }
        match effect {
            E_ARPEGGIO => self.arpeggio(c, arg),
            // the slides by the value kept from before, then a new one kept (measured: a first
            // slide moves on the second tick after its row)
            E_PORTAMENTO_UP if self.parse_tick => {}
            E_PORTAMENTO_DOWN if self.parse_tick => {}
            E_PORTAMENTO_UP => {
                let a = c.porta_old;
                if arg != 0 {
                    c.porta_old = arg;
                }
                c.last.period = c.last.period.wrapping_sub(a as u16);
                if c.last.period < 113 {
                    c.last.period = 113;
                }
            }
            E_PORTAMENTO_DOWN => {
                let a = c.porta_old;
                if arg != 0 {
                    c.porta_old = arg;
                }
                c.last.period = c.last.period.wrapping_add(a as u16);
                if c.last.period > 856 {
                    c.last.period = 856;
                }
            }
            E_GLISSANDO => self.glissando(c, arg),
            E_VIBRATO => self.vibrato(c, arg),
            // the volume slides, not on the tick that takes the row (measured: the
            // Eagleplayer's volumes go a step behind NostalgicPlayer's)
            E_GLISSANDO_VOLSLIDE => {
                if !self.parse_tick {
                    self.volume_slide(c, arg);
                }
                self.glissando(c, 0);
            }
            E_VIBRATO_VOLSLIDE => {
                if !self.parse_tick {
                    self.volume_slide(c, arg);
                }
                self.vibrato(c, 0);
            }
            E_VOLUME_SLIDE => {
                if !self.parse_tick {
                    self.volume_slide(c, arg);
                }
            }
            E_EXTRA => {
                let x = arg & 0xf0;
                let a = arg & 0x0f;
                match x {
                    X_RETRACE => {
                        if self.count == 1 {
                            c.retrace_count = 0;
                        }
                        if c.retrace_count == a.wrapping_sub(1) {
                            c.off_enable = true;
                            c.start_offset = 0;
                            c.retrace_count = 0;
                        } else {
                            c.retrace_count = c.retrace_count.wrapping_add(1);
                        }
                    }
                    X_CUT => {
                        if self.count == a {
                            c.volume = 0;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn volume_slide(&self, c: &mut Chan, arg: u8) {
        let a = if arg == 0 { c.slide_volume_old } else { arg };
        c.slide_volume_old = a;
        c.slid = true;
        if a < 0x10 {
            c.volume = c.volume.wrapping_sub(a);
            // a voice without a period slides from 0 (measured: its time is that of the clamp)
            if (c.volume as i8) < 0 || c.main_period == 0 {
                if (c.volume as i8) < 0 {
                    c.volume = 0;
                }
                c.clamped = true;
            }
            c.slide_down = true;
        } else {
            c.volume = c.volume.wrapping_add(a >> 4);
            if c.volume >= 64 {
                c.volume = 64;
                c.clamped = true;
            }
        }
    }

    fn arpeggio(&self, c: &mut Chan, arg: u8) {
        let arp = ARPEGGIO[(self.count.wrapping_sub(1) & 31) as usize];
        if arp == 0 {
            c.last.period = c.original_period;
            return;
        }
        let mut a = if arp == 2 { arg & 0x0f } else { (arg & 0xf0) >> 4 };
        let mut period = c.original_period;
        for i in 0..36u8 {
            if period >= PERIODS[8][i as usize] {
                a = a.wrapping_add(i);
                if a >= 35 {
                    a = 35;
                }
                period = PERIODS[8][a as usize];
                break;
            }
        }
        c.last.period = period;
    }

    fn glissando(&self, c: &mut Chan, arg: u8) {
        c.gliss_moved = false;
        let mut a = if arg == 0 { c.glissando_old_value } else { arg };
        if self.count == 1 {
            c.glissando_old_value = a;
            if c.last.effect == E_GLISSANDO {
                c.gliss_cost += GLISS_FIRST;
            }
        }
        if c.glissando_old_period == 0 {
            return;
        }
        if c.glissando_new_period == 0 {
            c.glissando_new_period = c.last.period;
            c.last.period = c.glissando_old_period;
            c.glissando_enable = false;
            if c.glissando_new_period == c.last.period {
                c.glissando_new_period = 0;
                return;
            }
            if c.glissando_new_period < c.last.period {
                c.glissando_enable = true;
            }
        } else if self.parse_tick {
            // no step on the row's tick (measured: the steps come on ticks 1 to speed-1)
            c.last.period = c.glissando_old_period;
        } else {
            c.gliss_moved = true;
            if c.glissando_enable {
                // a step past the target stops at it (measured), not wrapping round
                c.glissando_old_period = c.glissando_old_period.saturating_sub(a as u16);
                if c.glissando_old_period <= c.glissando_new_period {
                    c.glissando_old_period = c.glissando_new_period;
                    c.glissando_new_period = 0;
                    c.gliss_cost += GLISS_REACH_DOWN;
                }
            } else {
                c.glissando_old_period = c.glissando_old_period.saturating_add(a as u16);
                if c.glissando_old_period >= c.glissando_new_period {
                    c.glissando_old_period = c.glissando_new_period;
                    c.glissando_new_period = 0;
                    c.gliss_cost += GLISS_REACH_UP;
                }
            }
            c.last.period = c.glissando_old_period;
            // what goes out is the period tuned once more (measured: a note of 480 at +1,
            // 476, is reached as 472, and plays at 476 after the glissando)
            c.gliss_out = tune(c.last.period, self.fine_tune(c.old_sample));
        }
        let _ = &mut a;
    }

    fn vibrato(&self, c: &mut Chan, arg: u8) {
        let mut effect_arg = arg;
        if c.vibrato_period == 0 {
            c.vibrato_period = c.last.period;
        }
        let mut period = c.vibrato_period;
        // the row's tick plays the period as it is, the vibrato runs on ticks 1 to speed-1
        // after it (measured: a tick behind NostalgicPlayer's)
        'body: {
            if self.parse_tick {
                break 'body;
            }
            if effect_arg & 0x0f == 0 {
                effect_arg |= c.vibrato_old & 0x0f;
            }
            if effect_arg & 0xf0 == 0 {
                effect_arg |= c.vibrato_old & 0xf0;
            }
            c.vibrato_old = effect_arg;
            let mut vib = VIBRATO_SIN[((c.vibrato_value >> 2) & 0x1f) as usize] as u16;
            vib = ((effect_arg as u16 & 0x0f) * vib) >> 7;
            if c.vibrato_value < 0 {
                period = period.wrapping_sub(vib);
            } else {
                period = period.wrapping_add(vib);
                c.gliss_cost += VIB_UP;
            }
            if arg & 0xf0 != 0 {
                c.gliss_cost += VIB_MOVE;
            }
            c.vibrato_value = c.vibrato_value.wrapping_add(((effect_arg >> 2) & 0x3c) as i8);
        }
        c.last.period = period;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amplifier::LOG_BASE;
    use crate::engine::Engine;
    use std::path::Path;

    /// An unpacked module of 8 channels, one position, one pattern whose row 0
    /// plays sample 1 (64 bytes, a loop from byte 16 to its end, volume 48, fine
    /// tune `ft`) at `period` on channel 0, and a volume effect (C20) on channel 1.
    fn module(period: u16, ft: i8) -> Vec<u8> {
        let mut d = vec![0u8; 1572];
        d[..20].copy_from_slice(b"DIGI Booster module\0");
        d[24] = 0x17;
        d[25] = 8;
        // 1 pattern, 1 position
        d[176..180].copy_from_slice(&64u32.to_be_bytes());
        d[300..304].copy_from_slice(&16u32.to_be_bytes());
        d[424..428].copy_from_slice(&48u32.to_be_bytes());
        d[548] = 48;
        d[579] = ft as u8;
        d[610..614].copy_from_slice(b"test");
        for ch in 0..8 {
            for row in 0..64 {
                let v: u32 = match (ch, row) {
                    (0, 0) => (period as u32) << 16 | 0x1000,
                    (1, 0) => 0x0c20,
                    _ => 0,
                };
                d.extend(v.to_be_bytes());
            }
        }
        d.extend((0..64).map(|i| (i * 2) as u8));
        d
    }

    #[test]
    fn detects_and_loads() {
        let d = module(428, 0);
        assert!(detect(&d));
        assert!(!detect(&d[..1000]));
        let mut other = d.clone();
        other[..4].copy_from_slice(b"DBM0");
        assert!(!detect(&other));
        let s = load(&d).unwrap();
        assert_eq!((s.channels, s.length, s.name.as_str()), (8, 1, "test"));
        let s0 = &s.samples[0];
        assert_eq!((s0.length, s0.loop_start, s0.loop_len, s0.volume), (64, 16, 48, 48));
        assert_eq!(s0.data, Some(1572 + 2048));
        let row = s.patterns[0][0];
        assert_eq!((row[0].period, row[0].sample, row[1].effect, row[1].arg), (428, 1, 12, 0x20));
        // a sample that is cut short is taken
        assert!(load(&d[..d.len() - 10]).is_some());
        // a loop that starts past the sample's end is none
        let mut bad = d.clone();
        bad[300..304].copy_from_slice(&99u32.to_be_bytes());
        assert_eq!((load(&bad).unwrap().samples[0].loop_start, load(&bad).unwrap().samples[0].loop_len), (0, 0));
    }

    fn render(d: Vec<u8>) -> (Vec<(u32, u16, u64)>, Vec<f32>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.digi"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Digibooster");
        let mut out = vec![0f32; 2 * 48000];
        e.render(&mut out);
        (e.m.bus.write_log.take().unwrap(), out)
    }

    /// The first row's note starts on the amplifier's voice 0: DMA off, its
    /// length up to the loop's end, address and period, then DMA on and the loop.
    #[test]
    fn plays_a_note_on_the_amplifier() {
        let (log, out) = render(module(428, 0));
        let reg = |v: u32, r: u32| log.iter().filter(|w| w.0 == LOG_BASE | v << 4 | r).map(|w| w.1).collect::<Vec<_>>();
        // 64 bytes, the loop (16 to 64) ends the sample: 32 words, then the loop's 24
        assert_eq!(&reg(0, 4)[..2], &[32, 0x18]);
        assert_eq!(reg(0, 6).iter().copied().filter(|&p| p != 0).next(), Some(428));
        // the volume of voice 0 goes out every tick, the sample's once the note has started
        assert_eq!(reg(0, 8)[0], 0);
        assert!(reg(0, 8).contains(&48));
        // DMA off, then on
        assert_eq!(&reg(0, 0xa)[..2], &[0, 0x8000]);
        // channel 1's effect C20 sets a volume, which no sample plays
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }

    /// A sample's fine tune stretches the period by 2^(-ft/96), as the
    /// Eagleplayer does, not as NostalgicPlayer's ProTracker tables do.
    #[test]
    fn fine_tune_stretches_the_period() {
        for (period, ft, want) in [(214u16, -1i8, 215u16), (254, 1, 252), (428, 0, 428)] {
            let (log, _) = render(module(period, ft));
            let per: Vec<u16> = log.iter().filter(|w| w.0 == LOG_BASE | 6).map(|w| w.1).collect();
            assert!(per.contains(&want), "{period} at {ft}: {per:?}");
        }
    }
}
