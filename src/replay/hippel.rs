//! Jochen Hippel's players, ported from NostalgicPlayer's Hippel player
//! (Copyright (c) 2021 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! NostalgicPlayer plays three kinds of Hippel modules: the "TFMX" song data
//! after a game's own replayer ("Hippel"), the "COSO" packed song data with the
//! samples at its end or in a `.samp` file ("Hippel COSO"), and the 7-voice
//! modules. It reads the replayer's code to find which of its versions' features
//! a module uses. The originals this port follows are the Eagleplayers:
//!
//! * "Hippel-COSO" (V2.12, "w/o own replaycode", Defect Softworks) plays COSO
//!   modules with a replayer of its own, the one version of it whatever the
//!   module (NostalgicPlayer's last: position commands, volume fade and
//!   variation, the third vibrato and second portamento). It plays from the
//!   module in guest memory, as this port does, and writes Paula through the
//!   host's ENPP_ calls.
//! * "Hippel" (the Eagleplayer, 2.06) calls the replayer a "TFMX" module carries
//!   itself, one of the 13 builds of it listed in `BUILDS` and `A94` (told apart
//!   by a hash of their code). They share the song data and differ in the effects
//!   their code has, as NostalgicPlayer reads them from it, and in how they order
//!   their Paula writes (see `Build`). The port is checked against the
//!   Eagleplayer's register writes tick by tick; the builds' timing (the cost of
//!   each code path, between `GENERATED-COSTS` markers) was fitted to those logs
//!   by non-negative least squares, the line-poll constants by a search.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, the port follows the original (see `Coso`).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

pub const OPEN_COSO: Open = Open { name: "Hippel COSO", detect: detect_coso, new: new_coso };

/// The Eagleplayer's Check: "COSO" and, 32 bytes on, "TFMX".
fn detect_coso(d: &[u8]) -> bool {
    d.get(0..4) == Some(b"COSO") && d.get(0x20..0x24) == Some(b"TFMX")
}

/// Code points of the original's interrupt, where it spends the time between its
/// register writes: Paula stops a playing voice only at the end of its word, so
/// the time from a note's DMA off to the DMA on decides whether the note starts.
#[derive(Clone, Copy, Debug)]
enum At {
    /// From the timer interrupt to the first voice's effects.
    Start,
    /// The stopped player's writes.
    StopPer,
    StopVol,
    /// A voice's effects: entry, and its frequency table wait.
    Fx,
    FreqWait,
    /// One byte of the frequency table read, and what it is.
    FreqRead,
    FreqNote,
    FreqStop,
    FreqLoop,
    FreqVib,
    FreqDelay,
    FreqVar,
    FreqSlideSet,
    /// The commands that play a sample: up to the DMA off, between the writes,
    /// and after the last.
    E2Off,
    E2Adr,
    E2Len,
    E2Per,
    E2End,
    E7Same,
    E7Off,
    E7Adr,
    E7Len,
    E7Per,
    E7End,
    E9Off,
    E9Adr,
    E9Len,
    E9Per,
    E9End,
    E5Off,
    E5Per,
    E5End,
    E4Adr,
    E4Len,
    E4End,
    /// The sample slide: off, waiting, its step, its writes.
    SlideOff,
    SlideWait,
    SlideFirst,
    SlideStep,
    SlideDone,
    SlideAdr,
    SlideLen,
    SlideEnd,
    /// The volume envelope.
    EnvSustain,
    EnvWait,
    EnvRead,
    EnvVol,
    EnvLoop,
    EnvHold,
    EnvStop,
    /// The period: the note, the vibrato's delay or its step (by direction and
    /// turn), the portamento.
    Period,
    VibDelay,
    VibDown,
    VibDownTurn,
    VibUp,
    VibUpTurn,
    Porta,
    PortaUp,
    PortaDown,
    /// The volume, its variation, a fade below 0.
    Volume,
    VarRandom,
    VarSub,
    VolClamp,
    /// After the voices: no DMA on (each voice's volume and period, last voice
    /// first), or the DMA on and each voice's volume, period, length, location.
    EndVol,
    EndPer,
    DmaOn,
    DmaDone,
    OnVol,
    OnPer,
    OnLen,
    OnAdr,
}

impl At {
    fn cost(self) -> u64 {
        use At::*;
        match self {
            Start => 98,
            StopPer => 40,
            StopVol => 20,
            Fx => 61,
            FreqWait => 29,
            FreqRead => 64,
            FreqNote => 0,
            FreqStop => 6,
            FreqLoop => 0,
            FreqVib => 0,
            FreqDelay => 17,
            FreqVar => 20,
            FreqSlideSet => 0,
            E2Off => 7,
            E2Adr => 141,
            E2Len => 39,
            E2Per => 19,
            E2End => 143,
            E7Same => 24,
            E7Off => 20,
            E7Adr => 135,
            E7Len => 39,
            E7Per => 19,
            E7End => 148,
            E9Off => 7,
            E9Adr => 283,
            E9Len => 35,
            E9Per => 19,
            E9End => 152,
            E5Off => 0,
            E5Per => 84,
            E5End => 228,
            E4Adr => 41,
            E4Len => 39,
            E4End => 114,
            SlideOff => 0,
            SlideWait => 26,
            SlideFirst => 25,
            SlideStep => 0,
            SlideDone => 10,
            SlideAdr => 141,
            SlideLen => 35,
            SlideEnd => 106,
            EnvSustain => 0,
            EnvWait => 0,
            EnvRead => 53,
            EnvVol => 0,
            EnvLoop => 19,
            EnvHold => 51,
            EnvStop => 10,
            Period => 238,
            VibDelay => 0,
            VibDown => 91,
            VibDownTurn => 105,
            VibUp => 94,
            VibUpTurn => 103,
            Porta => 0,
            PortaUp => 65,
            PortaDown => 64,
            Volume => 0,
            VarRandom => 120,
            VarSub => 10,
            VolClamp => 5,
            EndVol => 27,
            EndPer => 19,
            DmaOn => 124,
            DmaDone => 68,
            OnVol => 27,
            OnPer => 19,
            OnLen => 38,
            OnAdr => 19,
        }
    }
}

/// The trace of code points for fitting their costs (HIPPEL_TRACE=file).
#[derive(Default)]
struct Trace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    pend: Vec<At>,
}

impl Trace {
    fn open() -> Self {
        let out = std::env::var_os("HIPPEL_TRACE")
            .and_then(|p| std::fs::File::create(p).ok())
            .map(std::io::BufWriter::new);
        Trace { out, pend: Vec::new() }
    }

    fn at(&mut self, hw: &mut Hw, e: At) {
        hw.wait(e.cost());
        if self.out.is_some() {
            self.pend.push(e);
        }
    }

    /// A register write is about to be made.
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

/// The host's ENPP_DMAMask with voices on: it waits for the DMA as
/// dtg_WaitAudioDMA does, to 10 lines past the current one.
fn dma_on(hw: &mut Hw, mask: u16) {
    hw.dmacon(0x8000 | mask);
    let now = hw.bus.clock;
    hw.wait((now / LINE + 10) * LINE - now);
}

/// ENPP_PokeLen as the original calls it: 0 for a length of 0 or 1.
fn len_word(l: u16) -> u16 {
    if l > 1 {
        l
    } else {
        0
    }
}

/// The original's voice structure ($56 bytes), the fields it uses.
#[derive(Default, Clone, Copy)]
struct Voice {
    /// The position list (this voice's column) at the song's start and end.
    pos: u32,
    pos_end: u32,
    /// The byte offset of the next position.
    pos_off: u16,
    /// The sample's loop, written after the DMA on.
    loop_addr: u32,
    loop_len: u16,
    /// The volume envelope and the frequency table, and the positions in them.
    env: u32,
    env_pos: u16,
    freq: u32,
    freq_pos: u16,
    /// The next track byte.
    track: u32,
    porta: u16,
    /// The sample slide: the sample (offset in the sample data) and its end,
    /// position and length in words, step, its delay.
    slide_start: u32,
    slide_end: u32,
    slide_pos: u16,
    slide_delta: u16,
    slide_len: u16,
    slide_moved: bool,
    slide_counter: u8,
    slide_speed: u8,
    slide_on: bool,
    slide_done: bool,
    /// The row counter and its reload.
    counter: u8,
    speed: u8,
    /// The voice's DMA bit.
    dma: u16,
    vib_delta: u8,
    note: u8,
    info: u8,
    env_transpose: u8,
    env_counter: u8,
    env_speed: u8,
    sustain: u8,
    freq_wait: u8,
    vib_speed: u8,
    vib_depth: u8,
    vib_delay: u8,
    prev_info: u8,
    /// The sample last started by E7 ($FF: none).
    sample: u8,
    transpose: u8,
    track_transpose: u8,
    volume: u8,
    vib_flag: u8,
    fade: u8,
    var_depth: u8,
    variation: u8,
}

/// Hippel COSO, as the Hippel-COSO Eagleplayer plays it. Where NostalgicPlayer
/// differs from it, this follows the original:
/// * a tick runs the four voices' effects, then writes the registers, then reads
///   the rows: a command that plays a sample turns the voice's DMA off and
///   writes its start, length and period 0 at once; after the voices, if any
///   DMA was turned off, DMA on (the host waits 10 lines) and every voice's
///   volume, period, loop length and loop location, else the volume and period
///   of voices 3 to 0;
/// * E4 writes the sample's loop as its location with the whole length; E9
///   plays a sub-sample with a loop of 1 word (length 0 to the host); a length
///   of 1 word is written as 0;
/// * the frequency and envelope tables run on in memory (no table numbers), E0
///   loops the table without masking its position;
/// * position command 8 stops the player for good (DMA off, period and volume
///   0 every tick, no song end), and the song end comes as a voice wraps;
/// * the subsongs are all the entries of the table, numbered from 1, and the
///   voices keep the state InitSound does not reset (the row speed, the random
///   seed, the loops);
/// * periods past the table read on in the original's code hunk and past it
///   (see `note_period`);
/// * the writes are spaced by the original's code-path costs (`At::cost`),
///   fitted by least squares to the reference's write times (residual 2.4
///   colour clocks RMS over 575640 writes of 132 modules), and InitSound comes
///   when it does in the reference (`init_time`).
///
/// A loop of one word or none is written as length 0, as the original does:
/// Paula then plays 65536 words from the loop's start, through whatever memory
/// follows the sample, which differs between the reference and the port, so
/// such renders differ while the writes are the same.
pub struct Coso {
    module: u32,
    samples: u32,
    freqs: u32,
    envs: u32,
    tracks: u32,
    positions: u32,
    subsongs_at: u32,
    sample_info: u32,
    subsong_count: u16,
    sub: u16,
    counter: u16,
    speed: u16,
    stopped: bool,
    random: u16,
    /// The DMA bits turned off this tick.
    dma_acc: u16,
    voices: [Voice; 4],
    songend: bool,
    trace: Trace,
}

fn new_coso(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_coso(d) {
        return None;
    }
    // ExtLoad: the samples from NAME.samp (the name up to its last dot), if any
    let mut samples = 0;
    let name = m.bus.read_cstr(m.deli.file_buf);
    if let Some(dot) = name.rfind('.') {
        let (_, data) = crate::deli::read_companion(m, &format!("{}samp", &name[..=dot]));
        if let Ok(s) = data {
            let a = m.alloc(s.len() as u32 + 1024);
            if a != 0 {
                m.bus.write_bytes(a, &s);
                samples = a;
            }
        }
    }
    let off = |k: usize| base.wrapping_add(be32(d, 4 * k));
    let subsongs_at = off(5);
    let sample_info = off(6);
    let mut p = Coso {
        module: base,
        samples: if samples != 0 { samples } else { off(7) },
        freqs: off(1),
        envs: off(2),
        tracks: off(3),
        positions: off(4),
        subsongs_at,
        sample_info,
        // InitPlayer: the entries between the subsongs and the samples, less one
        subsong_count: ((be32(d, 0x18).wrapping_sub(be32(d, 0x14)) / 6) as u16).wrapping_sub(1),
        sub: 1,
        counter: 0,
        speed: 0,
        stopped: false,
        random: 0,
        dma_acc: 0,
        voices: Default::default(),
        songend: false,
        trace: Trace::open(),
    };
    let path = m.bus.read_cstr(m.deli.dir_buf) + &name;
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(init_time(&path) + if samples != 0 { SAMP_LOAD } else { 0 });
    p.init_sound(1, &mut hw);
    Some(Box::new(p))
}

/// From the module's start to InitSound's first write, as the reference takes
/// it (measured): ExtLoad counts the path's characters (12.5 colour clocks
/// each) and looks back from its end for the last dot (20 each), then opens
/// NAME.samp only if there is a dot.
fn init_time(path: &str) -> u64 {
    let l = path.len() as u64;
    match path.rfind('.') {
        Some(p) => (10937 + 25 * l + 40 * (l - p as u64)) / 2,
        None => (10799 + 65 * l) / 2,
    }
}

/// What loading NAME.samp adds to that (measured).
const SAMP_LOAD: u64 = 35;

include!("hippel_tables.rs");

/// A note's period. The original's table (PERIODS) ends its code hunk with one
/// word after it, $2443; notes past that (up to 127) read whatever memory
/// follows the hunk. In deliplayer that is the next Eagleplayer loaded, so
/// the reference writes periods such as $0001 and $456c there (26 of the
/// corpus's modules); on an Amiga it is any memory, and the port reads 0.
fn note_period(n: u8) -> u16 {
    match n as usize {
        i if i < PERIODS.len() => PERIODS[i],
        72 => 0x2443,
        _ => 0,
    }
}

impl Replayer for Coso {
    fn format(&self) -> &'static str {
        "Hippel COSO"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.subsong_count, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // EndSound (InitSound with song 0 stops the player), then InitSound
        self.stopped = true;
        self.init_sound(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.trace.write(hw, "tick");
        self.trace.at(hw, At::Start);
        if self.stopped {
            self.trace.write(hw, "dma");
            hw.dmacon(0x000f);
            for ch in 0..4 {
                self.trace.at(hw, At::StopPer);
                self.trace.write(hw, "per");
                hw.per(ch, 0);
                self.trace.at(hw, At::StopVol);
                self.trace.write(hw, "vol");
                hw.vol(ch, 0);
            }
            return;
        }
        self.dma_acc = 0;
        let mut out = [(0u16, 0u16); 4];
        for (ch, o) in out.iter_mut().enumerate() {
            *o = self.effects(ch, hw);
        }
        if self.dma_acc != 0 {
            self.trace.at(hw, At::DmaOn);
            self.trace.write(hw, "dma");
            dma_on(hw, self.dma_acc);
            self.trace.at(hw, At::DmaDone);
            for (ch, &(per, vol)) in out.iter().enumerate() {
                let v = self.voices[ch];
                self.trace.at(hw, At::OnVol);
                self.trace.write(hw, "vol");
                hw.vol(ch, vol);
                self.trace.at(hw, At::OnPer);
                self.trace.write(hw, "per");
                hw.per(ch, per);
                self.trace.at(hw, At::OnLen);
                self.trace.write(hw, "len");
                hw.len(ch, len_word(v.loop_len));
                self.trace.at(hw, At::OnAdr);
                self.trace.write(hw, "adr");
                hw.loc(ch, v.loop_addr);
            }
        } else {
            for ch in (0..4).rev() {
                let (per, vol) = out[ch];
                self.trace.at(hw, At::EndVol);
                self.trace.write(hw, "vol");
                hw.vol(ch, vol);
                self.trace.at(hw, At::EndPer);
                self.trace.write(hw, "per");
                hw.per(ch, per);
            }
        }
        self.trace.write(hw, "end");
        self.counter = self.counter.wrapping_sub(1);
        if self.counter == 0 {
            self.counter = self.speed;
            for ch in 0..4 {
                self.row(ch, hw);
                if self.stopped {
                    break;
                }
            }
        }
    }
}

impl Coso {
    /// InitSound for subsong `n` (from 1).
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        let e = self.subsongs_at.wrapping_add(n.wrapping_sub(1).wrapping_mul(6) as i16 as i32 as u32);
        let (start, end) = (hw.bus.r16(e) as u32, hw.bus.r16(e + 2) as u32);
        self.speed = hw.bus.r16(e + 4);
        self.counter = 1;
        self.stopped = false;
        self.trace.write(hw, "dma");
        hw.dmacon(0x000f);
        hw.wait(48);
        hw.bus.write_word(0xdff09a, 0x0780);
        hw.wait(447);
        for ch in 0..4 {
            let v = &mut self.voices[ch];
            v.env = 0;
            v.freq = 0;
            v.env_counter = 1;
            v.env_speed = 1;
            v.sustain = 0;
            v.env_pos = 0;
            v.freq_wait = 0;
            v.vib_speed = 0;
            v.vib_depth = 0;
            v.slide_on = false;
            v.var_depth = 0;
            v.vib_delta = 0;
            v.vib_delay = 0;
            v.prev_info = 0;
            v.fade = 100;
            v.sample = 0xff;
            v.counter = 0xff;
            v.transpose = 0;
            v.volume = 0;
            v.freq_pos = 0;
            v.porta = 0;
            v.loop_len = 0;
            v.dma = 1 << ch;
            let col = 3 * ch as u32;
            v.pos = self.positions.wrapping_add(start * 12 + col);
            v.pos_end = self.positions.wrapping_add((end + 1) * 12 + col);
            v.pos_off = 0;
            // a track of $FF: the first row reads the first position
            v.track = 0;
            hw.loc(ch, self.samples);
            hw.wait(19);
            hw.len(ch, 0);
            hw.wait(19);
            hw.per(ch, 0);
            hw.wait(19);
            hw.vol(ch, 0);
            hw.wait(504 - 57);
        }
    }

    fn r8(&self, hw: &Hw, a: u32) -> u8 {
        // the empty tables (address 0 here) are the original's own data
        if a < 8 {
            return EMPTY_TABLE[a as usize];
        }
        hw.bus.r8(a)
    }

    /// The track byte at `a` (0: the original's $FF before the first row).
    fn track_byte(&self, hw: &Hw, a: u32) -> u8 {
        if a < 2 {
            return 0xff;
        }
        hw.bus.r8(a)
    }

    fn word_at(&self, hw: &Hw, a: u32) -> u16 {
        (self.r8(hw, a) as u16) << 8 | self.r8(hw, a + 1) as u16
    }

    /// The sample info entry of sample `n` (10 bytes: offset, length, loop
    /// start, loop length).
    fn info(&self, n: u8) -> u32 {
        self.sample_info.wrapping_add(n as u32 * 10)
    }

    /// ENPP_DMAMask turning the voice off, and the bit kept for the DMA on.
    fn dma_off(&mut self, ch: usize, hw: &mut Hw) {
        let b = self.voices[ch].dma;
        self.dma_acc |= b;
        self.trace.write(hw, "dma");
        hw.dmacon(b & 0x0f);
    }

    fn adr(&mut self, ch: usize, hw: &mut Hw, a: u32) {
        self.trace.write(hw, "adr");
        hw.loc(ch, a);
    }

    fn len(&mut self, ch: usize, hw: &mut Hw, l: u16) {
        self.trace.write(hw, "len");
        hw.len(ch, len_word(l));
    }

    fn per(&mut self, ch: usize, hw: &mut Hw, p: u16) {
        self.trace.write(hw, "per");
        hw.per(ch, p);
    }

    /// A voice's effects: the frequency table, the slide, the envelope, the
    /// period and the volume (which the end of the tick writes).
    fn effects(&mut self, ch: usize, hw: &mut Hw) -> (u16, u16) {
        self.trace.at(hw, At::Fx);
        // the frequency table
        loop {
            if self.voices[ch].freq_wait != 0 {
                self.voices[ch].freq_wait -= 1;
                self.trace.at(hw, At::FreqWait);
                break;
            }
            let e8 = self.freq_table(ch, hw);
            if !e8 {
                break;
            }
        }
        self.slide(ch, hw);
        self.envelope(ch, hw);
        let per = self.period(ch, hw);
        let vol = self.volume(ch, hw);
        (per, vol)
    }

    /// Reads the frequency table up to a note offset or a stop; true after E8
    /// (a wait, which is tested again).
    fn freq_table(&mut self, ch: usize, hw: &mut Hw) -> bool {
        loop {
            self.trace.at(hw, At::FreqRead);
            let v = self.voices[ch];
            let a = v.freq.wrapping_add(v.freq_pos as i16 as i32 as u32);
            let c = self.r8(hw, a);
            let a = a.wrapping_add(1);
            if c < 0xe0 {
                self.trace.at(hw, At::FreqNote);
                let v = &mut self.voices[ch];
                v.transpose = c;
                v.freq_pos = v.freq_pos.wrapping_add(1);
                return false;
            }
            match c {
                0xe0 => {
                    self.trace.at(hw, At::FreqLoop);
                    self.voices[ch].freq_pos = self.r8(hw, a) as u16;
                }
                0xe2 => {
                    self.trace.at(hw, At::E2Off);
                    let v = &mut self.voices[ch];
                    v.var_depth = 0;
                    v.sample = 0xff;
                    self.dma_off(ch, hw);
                    let info = self.info(self.r8(hw, a));
                    self.play(ch, hw, info, [At::E2Adr, At::E2Len, At::E2Per]);
                    self.trace.at(hw, At::E2End);
                    let v = &mut self.voices[ch];
                    v.env_pos = 0;
                    v.env_counter = 1;
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                    v.slide_on = false;
                }
                0xe3 => {
                    self.trace.at(hw, At::FreqVib);
                    let (s, dp) = (self.r8(hw, a), self.r8(hw, a + 1));
                    let v = &mut self.voices[ch];
                    v.freq_pos = v.freq_pos.wrapping_add(3);
                    v.vib_speed = s;
                    v.vib_depth = dp;
                }
                0xe4 => {
                    let info = self.info(self.r8(hw, a));
                    let bus = &*hw.bus;
                    let s = bus.r32(info).wrapping_add(self.samples);
                    let la = s.wrapping_add(bus.r16(info + 6) as u32);
                    let (len, ll) = (bus.r16(info + 4), bus.r16(info + 8));
                    self.voices[ch].loop_addr = la;
                    self.trace.at(hw, At::E4Adr);
                    self.adr(ch, hw, la);
                    self.trace.at(hw, At::E4Len);
                    self.len(ch, hw, len);
                    self.trace.at(hw, At::E4End);
                    let v = &mut self.voices[ch];
                    v.loop_len = ll;
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                    v.slide_on = false;
                }
                0xe5 => {
                    self.trace.at(hw, At::E5Off);
                    self.voices[ch].var_depth = 0;
                    self.dma_off(ch, hw);
                    self.trace.at(hw, At::E5Per);
                    self.per(ch, hw, 0);
                    self.trace.at(hw, At::E5End);
                    let info = self.info(self.r8(hw, a));
                    let start = hw.bus.r32(info);
                    let len = hw.bus.r16(info + 4);
                    let mut pos = self.word_at(hw, a + 1);
                    if pos == 0xffff {
                        pos = len;
                    }
                    let (sl, sd, ss) = (self.word_at(hw, a + 3), self.word_at(hw, a + 5), self.r8(hw, a + 7));
                    let v = &mut self.voices[ch];
                    v.slide_start = start;
                    v.slide_end = start.wrapping_add(2 * len as u32);
                    v.slide_pos = pos;
                    v.slide_len = sl;
                    v.slide_delta = sd;
                    v.slide_counter = 0;
                    v.slide_speed = ss;
                    v.slide_done = false;
                    v.slide_moved = false;
                    v.slide_on = true;
                    v.env_pos = 0;
                    v.env_counter = 1;
                    v.freq_pos = v.freq_pos.wrapping_add(9);
                }
                0xe6 => {
                    self.trace.at(hw, At::FreqSlideSet);
                    let (sl, sd, ss) = (self.word_at(hw, a), self.word_at(hw, a + 2), self.r8(hw, a + 4));
                    let v = &mut self.voices[ch];
                    v.slide_len = sl;
                    v.slide_delta = sd;
                    v.slide_counter = 0;
                    v.slide_speed = ss;
                    v.slide_moved = false;
                    v.slide_done = false;
                    v.freq_pos = v.freq_pos.wrapping_add(6);
                }
                0xe7 => {
                    let n = self.r8(hw, a);
                    if n == self.voices[ch].sample {
                        self.trace.at(hw, At::E7Same);
                    } else {
                        self.trace.at(hw, At::E7Off);
                        let v = &mut self.voices[ch];
                        v.var_depth = 0;
                        v.sample = n;
                        self.dma_off(ch, hw);
                        let info = self.info(n);
                        self.play(ch, hw, info, [At::E7Adr, At::E7Len, At::E7Per]);
                        self.trace.at(hw, At::E7End);
                    }
                    let v = &mut self.voices[ch];
                    v.env_pos = 0;
                    v.env_counter = 1;
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                    v.slide_on = false;
                }
                0xe8 => {
                    self.trace.at(hw, At::FreqDelay);
                    let w = self.r8(hw, a);
                    let v = &mut self.voices[ch];
                    v.freq_wait = w;
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                    return true;
                }
                0xe9 => {
                    self.trace.at(hw, At::E9Off);
                    let v = &mut self.voices[ch];
                    v.var_depth = 0;
                    v.sample = 0xff;
                    self.dma_off(ch, hw);
                    self.sub_sample(ch, hw, a);
                    self.trace.at(hw, At::E9End);
                    let v = &mut self.voices[ch];
                    v.env_pos = 0;
                    v.env_counter = 1;
                    v.freq_pos = v.freq_pos.wrapping_add(3);
                    v.slide_on = false;
                }
                0xea => {
                    self.trace.at(hw, At::FreqVar);
                    let dp = self.r8(hw, a);
                    let v = &mut self.voices[ch];
                    v.var_depth = dp;
                    v.variation = 0;
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                }
                // E1 stops the table (and $EB-$FF jump through the original's
                // code past its jump table; here they stop it too)
                _ => {
                    self.trace.at(hw, At::FreqStop);
                    return false;
                }
            }
        }
    }

    /// E2/E7: the sample's start, length and period 0, and its loop kept.
    fn play(&mut self, ch: usize, hw: &mut Hw, info: u32, at: [At; 3]) {
        let bus = &*hw.bus;
        let s = bus.r32(info).wrapping_add(self.samples);
        let (len, ls, ll) = (bus.r16(info + 4), bus.r16(info + 6), bus.r16(info + 8));
        self.trace.at(hw, at[0]);
        self.adr(ch, hw, s);
        self.trace.at(hw, at[1]);
        self.len(ch, hw, len);
        self.trace.at(hw, at[2]);
        self.per(ch, hw, 0);
        let v = &mut self.voices[ch];
        v.loop_addr = s.wrapping_add(ls as u32);
        v.loop_len = ll;
    }

    /// E9: sub-sample `n` of an "SSMP" sample, with a loop of one word (its
    /// first byte doubled).
    fn sub_sample(&mut self, ch: usize, hw: &mut Hw, a: u32) {
        let info = self.info(self.r8(hw, a));
        let bus = &mut *hw.bus;
        let s = bus.r32(info).wrapping_add(self.samples);
        let count = bus.r16(s + 4) as u32;
        let skip = bus.r16(s + 6).wrapping_shl(2) as i16 as i32 as u32;
        let headers = s.wrapping_add(8);
        let data = headers.wrapping_add(count * 24).wrapping_add(skip);
        let h = headers.wrapping_add(bus.r8(a + 1) as u32 * 24);
        let start = bus.r32(h) & !1;
        let end = bus.r32(h + 4) & !1;
        let len = (end.wrapping_sub(start) >> 1) as u16;
        let at = start.wrapping_add(data);
        let b = bus.r8(at);
        bus.w8(at.wrapping_add(1), b);
        self.trace.at(hw, At::E9Adr);
        self.adr(ch, hw, at);
        self.trace.at(hw, At::E9Len);
        self.len(ch, hw, len);
        self.trace.at(hw, At::E9Per);
        self.per(ch, hw, 0);
        let v = &mut self.voices[ch];
        v.loop_addr = at;
        v.loop_len = 1;
    }

    /// The sample slide (E5): every `speed + 1` ticks the loop moves by its step
    /// within the sample, and is written.
    fn slide(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.voices[ch];
        if !v.slide_on || v.slide_done {
            self.trace.at(hw, At::SlideOff);
            return;
        }
        v.slide_counter = v.slide_counter.wrapping_sub(1);
        if (v.slide_counter as i8) >= 0 {
            self.trace.at(hw, At::SlideWait);
            return;
        }
        v.slide_counter = v.slide_speed;
        let mut pos = v.slide_pos as i32;
        let delta = v.slide_delta as i16 as i32;
        let len = v.slide_len;
        let at = if !v.slide_moved {
            v.slide_moved = true;
            At::SlideFirst
        } else {
            pos += delta;
            if pos < 0 {
                v.slide_done = true;
                pos -= delta;
                At::SlideDone
            } else {
                let end = v.slide_start.wrapping_add(2 * pos as u32).wrapping_add(2 * len as u32);
                if end as i32 > v.slide_end as i32 {
                    v.slide_done = true;
                    pos -= delta;
                    At::SlideDone
                } else {
                    At::SlideStep
                }
            }
        };
        v.slide_pos = pos as u16;
        let a = v.slide_start.wrapping_add(self.samples).wrapping_add(2 * pos as u32);
        v.loop_len = len;
        v.loop_addr = a;
        self.trace.at(hw, at);
        self.trace.at(hw, At::SlideAdr);
        self.adr(ch, hw, a);
        self.trace.at(hw, At::SlideLen);
        self.len(ch, hw, len);
        self.trace.at(hw, At::SlideEnd);
    }

    /// The volume envelope.
    fn envelope(&mut self, ch: usize, hw: &mut Hw) {
        loop {
            let v = &mut self.voices[ch];
            if v.sustain != 0 {
                v.sustain -= 1;
                self.trace.at(hw, At::EnvSustain);
                return;
            }
            v.env_counter = v.env_counter.wrapping_sub(1);
            if v.env_counter != 0 {
                self.trace.at(hw, At::EnvWait);
                return;
            }
            v.env_counter = v.env_speed;
            loop {
                self.trace.at(hw, At::EnvRead);
                let v = self.voices[ch];
                let a = v.env.wrapping_add(v.env_pos as i16 as i32 as u32);
                let c = self.r8(hw, a);
                match c {
                    0xe0 => {
                        self.trace.at(hw, At::EnvLoop);
                        let p = (self.r8(hw, a + 1) as u16).wrapping_sub(5);
                        self.voices[ch].env_pos = p;
                    }
                    0xe8 => {
                        self.trace.at(hw, At::EnvHold);
                        let s = self.r8(hw, a + 1);
                        let v = &mut self.voices[ch];
                        v.sustain = s;
                        v.env_pos = v.env_pos.wrapping_add(2);
                        break;
                    }
                    0xe1..=0xff => {
                        self.trace.at(hw, At::EnvStop);
                        return;
                    }
                    _ => {
                        self.trace.at(hw, At::EnvVol);
                        let v = &mut self.voices[ch];
                        v.volume = c;
                        v.env_pos = v.env_pos.wrapping_add(1);
                        return;
                    }
                }
            }
        }
    }

    /// The period: the note, vibrato and portamento.
    fn period(&mut self, ch: usize, hw: &mut Hw) -> u16 {
        self.trace.at(hw, At::Period);
        let v = &mut self.voices[ch];
        let mut n = v.transpose;
        if (n as i8) >= 0 {
            n = n.wrapping_add(v.note).wrapping_add(v.track_transpose);
        }
        let mut per = note_period(n & 0x7f);
        if v.vib_delay != 0 {
            v.vib_delay -= 1;
            self.trace.at(hw, At::VibDelay);
        } else {
            let mut d1 = v.vib_delta as i16;
            let depth = v.vib_depth as i16;
            let speed = v.vib_speed as i16;
            let mut flag = v.vib_flag;
            let at = if flag & 0x20 == 0 {
                d1 -= speed;
                if d1 < 0 {
                    flag |= 0x20;
                    d1 = 0;
                    At::VibDownTurn
                } else {
                    At::VibDown
                }
            } else {
                d1 += speed;
                if d1 > depth {
                    flag &= !0x20;
                    d1 = depth;
                    At::VibUpTurn
                } else {
                    At::VibUp
                }
            };
            v.vib_delta = d1 as u8;
            v.vib_flag = flag;
            let d1 = d1.wrapping_sub(depth >> 1);
            per = per.wrapping_add(((d1 as i32 * per as i16 as i32) >> 10) as u16);
            self.trace.at(hw, at);
        }
        let v = &mut self.voices[ch];
        if v.info & 0x20 != 0 {
            let b = v.prev_info;
            if (b as i8) >= 0 {
                v.porta = v.porta.wrapping_add(b as u16);
                per = per.wrapping_sub(((v.porta as u32 * per as u32) >> 10) as u16);
                self.trace.at(hw, At::PortaUp);
            } else {
                v.porta = v.porta.wrapping_add(b.wrapping_neg() as u16);
                per = per.wrapping_add(((v.porta as u32 * per as u32) >> 10) as u16);
                self.trace.at(hw, At::PortaDown);
            }
        } else {
            self.trace.at(hw, At::Porta);
        }
        per
    }

    /// The volume: the envelope's, faded, less a random variation (once).
    fn volume(&mut self, ch: usize, hw: &mut Hw) -> u16 {
        self.trace.at(hw, At::Volume);
        let mut d1 = self.voices[ch].fade as i16;
        if self.voices[ch].var_depth != 0 {
            let mut d4 = self.voices[ch].variation as u16;
            if d4 == 0 {
                let depth = self.voices[ch].var_depth as u32;
                self.voices[ch].var_depth = 0;
                let r = self.random() as u32 & 0xff;
                let q = (depth * r / 0xff) as u16;
                self.voices[ch].variation = q as u8;
                d4 = q;
                self.trace.at(hw, At::VarRandom);
            } else {
                self.trace.at(hw, At::VarSub);
            }
            d1 = d1.wrapping_sub(d4 as i16);
        }
        if d1 < 0 {
            d1 = 0;
            self.trace.at(hw, At::VolClamp);
        }
        (self.voices[ch].volume as u32 * d1 as u32 / 100) as u16
    }

    fn random(&mut self) -> u16 {
        let v = self.random.wrapping_add(0x4793);
        self.random = v.rotate_right(6) ^ v;
        self.random
    }

    /// The next row of a voice: positions, the row speed, a note and its
    /// envelope and frequency table.
    fn row(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.voices[ch];
        v.counter = v.counter.wrapping_sub(1);
        if (v.counter as i8) >= 0 {
            return;
        }
        v.counter = v.speed;
        let mut a = v.track;
        loop {
            let c = self.track_byte(hw, a);
            a = a.wrapping_add(1);
            match c {
                0xff => {
                    let v = &mut self.voices[ch];
                    let mut p = v.pos.wrapping_add(v.pos_off as i16 as i32 as u32);
                    if p == v.pos_end {
                        v.pos_off = 0;
                        p = v.pos;
                        self.songend = true;
                    }
                    let bus = &*hw.bus;
                    let (t, tt, e) = (bus.r8(p), bus.r8(p + 1), bus.r8(p + 2));
                    let v = &mut self.voices[ch];
                    v.track_transpose = tt;
                    if e > 0x7f {
                        let (cmd, arg) = (e >> 4, e & 0x0f);
                        match cmd {
                            0xf => v.fade = if arg == 0 { 100 } else { (16 - arg) * 6 },
                            0x8 => self.stopped = true,
                            0xe => self.speed = arg as u16,
                            _ => {}
                        }
                    } else {
                        v.env_transpose = e;
                    }
                    let off = hw.bus.r16(self.tracks.wrapping_add(2 * t as u32));
                    let v = &mut self.voices[ch];
                    v.track = self.module.wrapping_add(off as i16 as i32 as u32);
                    v.pos_off = v.pos_off.wrapping_add(12);
                    a = v.track;
                }
                0xfe => {
                    let s = hw.bus.r8(a);
                    a = a.wrapping_add(1);
                    let v = &mut self.voices[ch];
                    v.speed = s;
                    v.counter = s;
                }
                0xfd => {
                    let s = hw.bus.r8(a);
                    let v = &mut self.voices[ch];
                    v.speed = s;
                    v.counter = s;
                    v.track = a.wrapping_add(1);
                    return;
                }
                _ => {
                    let info = hw.bus.r8(a);
                    a = a.wrapping_add(1);
                    let v = &mut self.voices[ch];
                    v.note = c;
                    v.info = info;
                    if info & 0xe0 != 0 {
                        v.prev_info = hw.bus.r8(a);
                        a = a.wrapping_add(1);
                    }
                    v.track = a;
                    v.porta = 0;
                    if (c as i8) < 0 {
                        return;
                    }
                    let n = (info & 0x1f).wrapping_add(v.env_transpose) as u32;
                    let off = hw.bus.r16(self.envs.wrapping_add(2 * n));
                    let e = self.module.wrapping_add(off as i16 as i32 as u32);
                    let bus = &*hw.bus;
                    let (sp, f, vs, dp, dl) = (bus.r8(e), bus.r8(e + 1), bus.r8(e + 2), bus.r8(e + 3), bus.r8(e + 4));
                    let v = &mut self.voices[ch];
                    v.env_pos = 0;
                    v.env_counter = sp;
                    v.env_speed = sp;
                    v.vib_speed = vs;
                    v.vib_flag = 0x40;
                    v.vib_depth = dp;
                    v.vib_delta = dp;
                    v.vib_delay = dl;
                    v.env = e.wrapping_add(5);
                    v.sustain = 0;
                    if f == 0x80 {
                        return;
                    }
                    let f = if info & 0x40 != 0 { v.prev_info } else { f };
                    let off = hw.bus.r16(self.freqs.wrapping_add(2 * f as u32));
                    let v = &mut self.voices[ch];
                    v.freq = self.module.wrapping_add(off as i16 as i32 as u32);
                    v.freq_pos = 0;
                    v.freq_wait = 0;
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hippel: the "TFMX" song data after a game's own replayer
// ---------------------------------------------------------------------------

pub const OPEN: Open = Open { name: "Hippel TFMX", detect: detect_tfmx, new: new_tfmx };

/// Which set of code-path costs a build's interrupt takes (`Tp::cost`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Timing {
    A94,
    /// Song data at $962, $98E and $9A6.
    E962,
    E98e,
    E9a6,
    E8d6,
    E8e6,
    Ea4c,
    Ea50,
    /// $A2C (the two versions of its code), $A58, $B02.
    Middle,
    Ma2cb,
    Ma58,
    Mb02,
}

/// How a replayer build writes Paula and which effects its code has (the
/// effect set as NostalgicPlayer reads it from the code). The builds the port
/// plays are told apart by a hash of their code (`fnv`, with the bytes that
/// differ between modules of one build, the saved variables, left out); other
/// builds are refused.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Build {
    /// The code-path costs.
    timing: Timing,
    /// The replayer keeps a word six bytes before the song data that is not 0 in
    /// a module saved while a song played (it holds that song's number). The
    /// Eagleplayer's run of such a module never starts a song: InitSound sets the
    /// voices up, but reads no positions and leaves the speed counter at 0.
    song_var: bool,
    /// The Eagleplayer's start: colour clocks from the module's load to the
    /// replayer's LED write, from that to its first DMA off, and (some builds
    /// turn the DMA off once more first) to InitSound's own, and the wait at
    /// InitSound's end that puts the interrupts' phase right.
    init_at: u64,
    led_gap: u64,
    init_stop: bool,
    /// From the LED write to InitSound's DMA off, less what depends on the song
    /// data (9 colour clocks per sample and 58.4 per subsong: InitPlayer's loops),
    /// 0 where it is `led_gap` alone.
    init_k: u64,
    init_tail: u64,
    /// The subsong table's step in bytes (the A94 build reads 6-byte entries by 4).
    sub_step: u32,
    /// Line waits before the DMA on and after it (interleaved builds), and the
    /// polling's first read and delay to the DMA on in this build.
    waits: u8,
    waits_after: u8,
    poll_from: u64,
    poll_to: i64,
    /// A new note with an envelope turns the voice's DMA off.
    mute: bool,
    /// A row's info bit 6 takes the frequency table from the row before.
    freq_prev: bool,
    /// E5: 0 not there, 1 plays a sample from an offset, 2 the sample slide.
    e5: u8,
    /// E6 changes the slide.
    e6: bool,
    /// E7: 1 switches the frequency table, 2 plays a sample.
    e7: u8,
    /// E9 plays a sub-sample.
    e9: bool,
    /// E9 makes the sub-sample's bounds even and copies its first byte over
    /// the second (else the bounds are used as they are, and the sample must
    /// begin with SSMP).
    e9_even: bool,
    /// After a sample or vibrato command the table goes on with the next
    /// command, else the next byte is the transpose.
    effloop: bool,
    /// The vibrato's version (NostalgicPlayer's 1 or 2).
    vib: u8,
    /// A frequency table wait skips the sample slide too.
    wait_skips_slide: bool,
    /// Where the writes go: per voice the effects, then its period and volume
    /// (false), or the effects of all voices, the DMA on and the loops, then
    /// every voice's period and volume (true).
    split: bool,
    /// Voices whose period and volume are written, and whose loops.
    voices: usize,
    /// The voice after those gets a period and volume of 0 every tick.
    silent_last: bool,
    loop_voices: usize,
    /// The DMA on is written every tick, not only after a DMA off.
    dma_always: bool,
    /// The voices' fields InitSound leaves alone are read from the replayer's
    /// variables ($52 bytes each after the empty table), as the A94 build keeps them.
    saved_vars: bool,
    /// The loops left pending in the early builds' variables ($4A bytes per
    /// voice from 6A after the empty table: loop length, address, pending).
    early_vars: bool,
    /// The speed counter starts at the subsong's speed (else at 1).
    counter_from_speed: bool,
    /// Voices InitSound sets up.
    init_voices: usize,
    /// InitSound points the voices at this offset from the empty table (else at
    /// their own register), with this length, period and volume.
    init_loc: Option<u32>,
    init_len: u16,
    init_per: u16,
    init_vol: u16,
}

/// The replayers with the song data at $A94 (Leaving Teramis, Wings of
/// Death and others): rows first, then the voices' effects, each voice's
/// period and volume in one long write; E2/E7/E9 turn the DMA off and
/// write the location and length; after the voices, if any DMA was turned
/// off, three line waits, the DMA on, a line wait, every voice's loop.
const A94: Build = Build {
    timing: Timing::A94,
    song_var: false,
    init_at: 58358,
    led_gap: 5409,
    init_stop: false,
    init_k: 0,
    init_tail: 0,
    sub_step: 4,
    waits: 3,
    waits_after: 1,
    poll_from: 168,
    poll_to: 18,
    mute: false,
    freq_prev: true,
    e5: 2,
    e6: true,
    e7: 2,
    e9: true,
    e9_even: true,
    effloop: true,
    vib: 2,
    wait_skips_slide: false,
    split: false,
    voices: 4,
    silent_last: false,
    loop_voices: 4,
    dma_always: false,
    counter_from_speed: false,
    saved_vars: true,
    early_vars: false,
    init_voices: 4,
    init_loc: None,
    init_len: 1,
    init_per: 0,
    init_vol: 0,
};

fn fnv(d: &[u8], skip: &[std::ops::Range<usize>]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for (i, &b) in d.iter().enumerate() {
        if !skip.iter().any(|r| r.contains(&i)) {
            h = (h ^ b as u32).wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// The early builds (song data at $8D6 to $A50): a new note turns its voice's
/// DMA off, E7 switches the frequency table, no sample slide or E9, the older
/// vibrato; per tick the effects of every voice, the DMA on, the loops of the
/// voices that started a sample the tick before, then every voice's period
/// and volume.
const EARLY: Build = Build {
    timing: Timing::E962,
    song_var: true,
    init_at: 51887,
    led_gap: 0,
    init_stop: false,
    init_k: 8563,
    init_tail: 0,
    sub_step: 6,
    waits: 0,
    waits_after: 0,
    poll_from: 0,
    poll_to: 0,
    mute: true,
    freq_prev: false,
    e5: 0,
    e6: false,
    e7: 1,
    e9: false,
    e9_even: false,
    effloop: false,
    vib: 1,
    wait_skips_slide: false,
    split: true,
    voices: 4,
    silent_last: false,
    loop_voices: 4,
    dma_always: true,
    counter_from_speed: true,
    saved_vars: false,
    early_vars: true,
    init_voices: 4,
    init_loc: Some(0x18),
    init_len: 0x100,
    init_per: 0,
    init_vol: 0,
};

/// The middle builds ($A2C to $B02): no mute, E5 plays a sample from an offset,
/// E7 a sample, the older vibrato; per voice its effects and its period and
/// volume, then every tick the DMA on and all loops.
const MIDDLE: Build = Build {
    timing: Timing::Middle,
    song_var: true,
    init_at: 55862,
    led_gap: 346,
    init_stop: true,
    init_k: 8289,
    init_tail: 0,
    sub_step: 6,
    waits: 1,
    waits_after: 1,
    poll_from: 219,
    poll_to: 18,
    mute: false,
    freq_prev: true,
    e5: 1,
    e6: false,
    e7: 2,
    e9: true,
    e9_even: false,
    effloop: false,
    vib: 1,
    wait_skips_slide: false,
    split: false,
    voices: 4,
    silent_last: false,
    loop_voices: 4,
    dma_always: true,
    counter_from_speed: false,
    saved_vars: false,
    early_vars: false,
    init_voices: 4,
    init_loc: None,
    init_len: 1,
    init_per: 0,
    init_vol: 0,
};

/// The builds the port plays by the hash of their code up to the empty table
/// (see `fnv`).
const BUILDS: &[(u32, Build)] = &[
    // song data at $8D6: three voices
    (0x8e22_6fe7, Build { timing: Timing::E8d6, init_at: 48865, led_gap: 345, init_stop: true, init_k: 8295, voices: 3, loop_voices: 3, init_voices: 3, ..EARLY }),
    // $8E6: two voices
    (0x300e_934a, Build { timing: Timing::E8e6, init_at: 49252, led_gap: 345, init_stop: true, init_k: 8295, voices: 2, loop_voices: 2, ..EARLY }),
    // $962, $98E, $9A6
    (0xc7c7_ab3c, EARLY),
    (0x9ab5_f1f4, Build { timing: Timing::E98e, init_at: 52817, led_gap: 345, init_stop: true, init_k: 8294, ..EARLY }),
    (0x88cc_aff4, Build { timing: Timing::E9a6, init_at: 53301, led_gap: 345, init_stop: true, init_k: 8109, ..EARLY }),
    // $A4C, $A50
    (0x0e4a_0a89, Build { timing: Timing::Ea4c, init_at: 56909, led_gap: 346, init_stop: true, init_k: 8303, ..EARLY }),
    (0xa3a1_0c0a, Build { timing: Timing::Ea50, init_at: 56975, led_gap: 346, init_stop: true, init_k: 8303, voices: 3, silent_last: true, ..EARLY }),
    // $A2C (two versions); the $A3A build, whose volumes differ, is not played
    (0x688a_532a, MIDDLE),
    (0x7d97_d238, Build { timing: Timing::Ma2cb, init_at: 55872, waits: 2, poll_from: 195, poll_to: 13, ..MIDDLE }),
    // $B02: three voices
    (0xaf7d_cbe8, Build { timing: Timing::Mb02, init_at: 60566, led_gap: 345, init_stop: true, init_k: 8298, voices: 3, waits: 3, waits_after: 3, poll_from: 273, poll_to: -1, ..MIDDLE }),
    // $A58
    (0x91ba_7e51, Build { timing: Timing::Ma58, init_at: 56454, led_gap: 348, init_stop: true, init_k: 8291, dma_always: false, waits: 3, waits_after: 3, poll_from: 144, poll_to: 40, init_loc: Some(0), init_per: 0xc0, init_vol: 0x40, ..MIDDLE }),
];

/// The build of the module's replayer, and where the song data starts.
fn tfmx_version(d: &[u8]) -> Option<(Build, usize)> {
    let t = (0..d.len().saturating_sub(4)).step_by(2).find(|&i| &d[i..i + 4] == b"TFMX")?;
    if t == 0xa94 && d.get(0..2) == Some(&[0x60, 0x00]) {
        // the code, to the replayer's variables at $866 (the empty table, the voices)
        if fnv(&d[..0x866], &[0x406..0x40a]) != A94_CODE && !synthetic(d) {
            return None;
        }
        let wait_skips_slide = be32(d, 0x406) == 0x6000_0336;
        if !wait_skips_slide && be32(d, 0x406) != 0x6000_02b2 {
            return None;
        }
        return Some((Build { wait_skips_slide, ..A94 }, t));
    }
    if synthetic_early(d) {
        return Some((EARLY, t));
    }
    let empty = (0..t).find(|&i| d[i..i + 8] == [1, 0, 0, 0, 0, 0, 0, 0xe1])?;
    let h = fnv(&d[..empty], &[]);
    BUILDS.iter().find(|b| b.0 == h).map(|b| (b.1, t))
}

/// The hash of the $A94 version's code (see `fnv`).
const A94_CODE: u32 = 0x8d42_32fa;

/// A unit test's module, which has a marker in place of the replayer's code.
#[cfg(test)]
fn synthetic(d: &[u8]) -> bool {
    d.get(2..10) == Some(b"SYNTHTST")
}

#[cfg(not(test))]
fn synthetic(_: &[u8]) -> bool {
    false
}

/// The same for a unit test of an early build.
#[cfg(test)]
fn synthetic_early(d: &[u8]) -> bool {
    d.get(2..10) == Some(b"SYNTHEAR")
}

#[cfg(not(test))]
fn synthetic_early(_: &[u8]) -> bool {
    false
}

fn detect_tfmx(d: &[u8]) -> bool {
    tfmx_version(d).is_some()
}

/// Code points of the TFMX replayers' interrupt (see `At`).
#[derive(Clone, Copy, Debug)]
enum Tp {
    Start,
    Stopped,
    StopVol,
    Rows,
    Row,
    RowNext,
    RowStop,
    RowTrack,
    RowEmpty,
    RowNote,
    RowEnv,
    RowAlt,
    RowEnd,
    Fx,
    FreqWait,
    FreqRead,
    FreqNote,
    FreqLoop,
    FreqStop,
    E2Off,
    E2Loc,
    E2Len,
    E2End,
    E3,
    E4Loc,
    E4Len,
    E4End,
    E5Off,
    E5End,
    E6,
    E7Same,
    E7Off,
    E7Loc,
    E7Len,
    E7End,
    E8,
    E9Off,
    E9Loc,
    E9Len,
    E9End,
    SlideOff,
    SlideWait,
    SlideFirst,
    SlideStep,
    SlideDone,
    SlideLoc,
    SlideLen,
    EnvSustain,
    EnvWait,
    EnvRead,
    EnvVol,
    EnvLoop,
    EnvHold,
    EnvStop,
    Period,
    VibDelay,
    VibSkip,
    VibDown,
    VibDownTurn,
    VibUp,
    VibUpTurn,
    Porta,
    PortaUp,
    PortaDown,
    Write,
    NoDma,
    Loops,
    LineOn,
    OnLoc,
    OnLen,
    PerStart,
    PerStart0,
    WriteVol,
}

impl Tp {
    fn cost(self, t: Timing) -> u64 {
        match t {
            Timing::A94 => self.cost_a94(),
            Timing::E962 => self.cost_e962(),
            Timing::E98e => self.cost_e98e(),
            Timing::E9a6 => self.cost_e9a6(),
            Timing::E8d6 => self.cost_e8d6(),
            Timing::E8e6 => self.cost_e8e6(),
            Timing::Ea4c => self.cost_ea4c(),
            Timing::Ea50 => self.cost_ea50(),
            Timing::Middle => self.cost_middle(),
            Timing::Ma2cb => self.cost_ma2cb(),
            Timing::Ma58 => self.cost_ma58(),
            Timing::Mb02 => self.cost_mb02(),
        }
    }

    fn cost_a94(self) -> u64 {
        use Tp::*;
        match self {
            Start => 23,
            Stopped => 40,
            StopVol => 10,
            Rows => 0,
            Row => 97,
            RowNext => 135,
            RowStop => 10,
            RowTrack => 0,
            RowEmpty => 0,
            RowNote => 52,
            RowEnv => 166,
            RowAlt => 5,
            RowEnd => 0,
            Fx => 7,
            FreqWait => 40,
            FreqRead => 65,
            FreqNote => 8,
            FreqLoop => 0,
            FreqStop => 14,
            E2Off => 31,
            E2Loc => 63,
            E2Len => 30,
            E2End => 9,
            E3 => 30,
            E4Loc => 66,
            E4Len => 30,
            E4End => 0,
            E5Off => 23,
            E5End => 0,
            E6 => 80,
            E7Same => 22,
            E7Off => 47,
            E7Loc => 57,
            E7Len => 30,
            E7End => 14,
            E8 => 18,
            E9Off => 60,
            E9Loc => 200,
            E9Len => 20,
            E9End => 60,
            SlideOff => 0,
            SlideWait => 20,
            SlideFirst => 150,
            SlideStep => 0,
            SlideDone => 12,
            SlideLoc => 175,
            SlideLen => 4,
            EnvSustain => 52,
            EnvWait => 51,
            EnvRead => 104,
            EnvVol => 0,
            EnvLoop => 30,
            EnvHold => 0,
            EnvStop => 12,
            Period => 74,
            VibDelay => 0,
            VibSkip => 59,
            VibDown => 65,
            VibDownTurn => 78,
            VibUp => 67,
            VibUpTurn => 76,
            Porta => 0,
            PortaUp => 54,
            PortaDown => 55,
            Write => 0,
            NoDma => 20,
            Loops => 0,
            LineOn => 0,
            OnLoc => 8,
            OnLen => 6,
            PerStart => 0,
            PerStart0 => 0,
            WriteVol => 0,
        }
    }

    // GENERATED-COSTS-BEGIN
    fn cost_e962(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 56,
            E2Len => 18,
            E2Loc => 86,
            E2Off => 0,
            E3 => 19,
            EnvRead => 0,
            EnvStop => 67,
            EnvVol => 89,
            EnvWait => 12,
            FreqLoop => 0,
            FreqNote => 104,
            FreqRead => 31,
            FreqStop => 0,
            Fx => 154,
            LineOn => 9,
            Loops => 209,
            OnLen => 8,
            OnLoc => 55,
            PerStart => 26,
            PerStart0 => 59,
            Period => 0,
            Porta => 0,
            Row => 0,
            RowEmpty => 0,
            RowEnd => 104,
            RowEnv => 0,
            RowNext => 161,
            RowNote => 243,
            RowTrack => 0,
            Rows => 10,
            SlideOff => 0,
            VibDelay => 0,
            VibDown => 85,
            VibDownTurn => 99,
            VibUp => 84,
            VibUpTurn => 93,
            Write => 8,
            WriteVol => 8,
            _ => self.cost_a94(),
        }
    }

    fn cost_e98e(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 16,
            E2Len => 18,
            E2Loc => 86,
            E2Off => 43,
            E3 => 38,
            E4End => 0,
            E4Len => 18,
            E4Loc => 109,
            E8 => 89,
            EnvLoop => 28,
            EnvRead => 48,
            EnvStop => 0,
            EnvVol => 26,
            EnvWait => 0,
            FreqLoop => 0,
            FreqNote => 104,
            FreqRead => 31,
            FreqStop => 0,
            FreqWait => 23,
            Fx => 124,
            LineOn => 7,
            Loops => 219,
            OnLen => 8,
            OnLoc => 59,
            PerStart => 31,
            PerStart0 => 61,
            Period => 0,
            Porta => 0,
            PortaUp => 41,
            Row => 0,
            RowEmpty => 0,
            RowEnd => 103,
            RowEnv => 201,
            RowNext => 159,
            RowNote => 37,
            RowTrack => 0,
            Rows => 15,
            SlideOff => 45,
            VibDelay => 0,
            VibDown => 74,
            VibDownTurn => 97,
            VibUp => 81,
            VibUpTurn => 91,
            Write => 8,
            WriteVol => 8,
            _ => self.cost_a94(),
        }
    }

    fn cost_e9a6(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 45,
            E2Len => 18,
            E2Loc => 86,
            E2Off => 15,
            E4End => 0,
            E4Len => 18,
            E4Loc => 110,
            E8 => 0,
            EnvRead => 0,
            EnvStop => 0,
            EnvVol => 20,
            FreqNote => 187,
            FreqRead => 106,
            FreqStop => 79,
            Fx => 73,
            LineOn => 0,
            Loops => 205,
            OnLen => 8,
            OnLoc => 55,
            PerStart => 6,
            PerStart0 => 68,
            Period => 0,
            Porta => 0,
            Row => 0,
            RowEmpty => 0,
            RowEnd => 98,
            RowEnv => 0,
            RowNext => 158,
            RowNote => 252,
            RowTrack => 0,
            Rows => 35,
            SlideOff => 0,
            VibDelay => 0,
            VibDown => 76,
            VibDownTurn => 92,
            VibUp => 78,
            VibUpTurn => 86,
            Write => 8,
            WriteVol => 8,
            _ => self.cost_a94(),
        }
    }

    fn cost_e8d6(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 18,
            E2Len => 18,
            E2Loc => 86,
            E2Off => 33,
            E4End => 0,
            E4Len => 18,
            E4Loc => 100,
            EnvRead => 49,
            EnvStop => 0,
            EnvVol => 27,
            EnvWait => 0,
            FreqLoop => 0,
            FreqNote => 136,
            FreqRead => 40,
            FreqStop => 25,
            Fx => 117,
            LineOn => 0,
            Loops => 724,
            OnLen => 8,
            OnLoc => 39,
            PerStart => 36,
            PerStart0 => 52,
            Period => 0,
            Porta => 0,
            Row => 0,
            RowEmpty => 0,
            RowEnd => 104,
            RowEnv => 0,
            RowNext => 158,
            RowNote => 241,
            RowTrack => 0,
            Rows => 10,
            SlideOff => 0,
            VibDelay => 0,
            VibDown => 97,
            VibDownTurn => 111,
            VibUp => 96,
            VibUpTurn => 106,
            Write => 8,
            WriteVol => 8,
            _ => self.cost_a94(),
        }
    }

    fn cost_e8e6(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 15,
            E2Len => 18,
            E2Loc => 86,
            E2Off => 40,
            E4End => 0,
            E4Len => 18,
            E4Loc => 111,
            EnvRead => 0,
            EnvStop => 0,
            EnvVol => 35,
            FreqLoop => 0,
            FreqNote => 106,
            FreqRead => 42,
            FreqStop => 0,
            Fx => 105,
            LineOn => 2,
            Loops => 158,
            OnLen => 8,
            OnLoc => 55,
            PerStart => 10,
            PerStart0 => 35,
            Period => 88,
            Porta => 0,
            Row => 0,
            RowEmpty => 0,
            RowEnd => 104,
            RowEnv => 0,
            RowNext => 162,
            RowNote => 242,
            RowTrack => 0,
            Rows => 10,
            SlideOff => 0,
            VibDelay => 0,
            VibDown => 68,
            VibDownTurn => 84,
            VibUp => 77,
            VibUpTurn => 85,
            Write => 8,
            WriteVol => 8,
            _ => self.cost_a94(),
        }
    }

    fn cost_ea4c(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 47,
            E2Len => 18,
            E2Loc => 86,
            E2Off => 0,
            E4End => 36,
            E4Len => 18,
            E4Loc => 82,
            EnvRead => 0,
            EnvStop => 43,
            EnvVol => 68,
            EnvWait => 0,
            FreqLoop => 0,
            FreqNote => 118,
            FreqRead => 45,
            FreqStop => 0,
            Fx => 102,
            LineOn => 0,
            Loops => 242,
            OnLen => 8,
            OnLoc => 56,
            PerStart => 30,
            PerStart0 => 68,
            Period => 51,
            Porta => 0,
            PortaUp => 60,
            Row => 0,
            RowEmpty => 0,
            RowEnd => 79,
            RowEnv => 221,
            RowNext => 129,
            RowNote => 53,
            RowTrack => 0,
            Rows => 0,
            SlideOff => 0,
            VibDelay => 0,
            VibDown => 83,
            VibDownTurn => 88,
            VibUp => 76,
            VibUpTurn => 90,
            Write => 8,
            WriteVol => 8,
            _ => self.cost_a94(),
        }
    }

    fn cost_ea50(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 0,
            E2Len => 18,
            E2Loc => 86,
            E2Off => 48,
            EnvRead => 67,
            EnvStop => 0,
            EnvVol => 13,
            EnvWait => 0,
            FreqNote => 115,
            FreqRead => 0,
            FreqStop => 0,
            Fx => 164,
            LineOn => 0,
            Loops => 267,
            OnLen => 8,
            OnLoc => 55,
            PerStart => 50,
            PerStart0 => 68,
            Period => 0,
            Porta => 0,
            Row => 0,
            RowEmpty => 0,
            RowEnd => 106,
            RowEnv => 0,
            RowNext => 159,
            RowNote => 241,
            RowTrack => 0,
            Rows => 1,
            SlideOff => 21,
            VibDelay => 0,
            VibDown => 79,
            VibDownTurn => 92,
            VibUp => 77,
            VibUpTurn => 86,
            Write => 8,
            WriteVol => 8,
            _ => self.cost_a94(),
        }
    }

    fn cost_middle(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 5,
            E2Len => 30,
            E2Loc => 65,
            E2Off => 0,
            E4End => 0,
            E4Len => 30,
            E4Loc => 62,
            E7End => 22,
            E7Len => 30,
            E7Loc => 57,
            E7Off => 59,
            E7Same => 33,
            EnvLoop => 0,
            EnvRead => 71,
            EnvStop => 0,
            EnvVol => 23,
            EnvWait => 17,
            FreqLoop => 37,
            FreqNote => 197,
            FreqRead => 0,
            FreqStop => 83,
            Fx => 102,
            OnLen => 6,
            OnLoc => 8,
            Period => 0,
            Porta => 0,
            PortaUp => 54,
            Row => 99,
            RowAlt => 12,
            RowEmpty => 0,
            RowEnd => 0,
            RowEnv => 174,
            RowNext => 141,
            RowNote => 53,
            RowTrack => 0,
            Rows => 0,
            SlideOff => 0,
            VibDelay => 0,
            VibDown => 70,
            VibDownTurn => 92,
            VibUp => 77,
            VibUpTurn => 86,
            Write => 0,
            _ => self.cost_a94(),
        }
    }

    fn cost_ma2cb(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 13,
            E2Len => 30,
            E2Loc => 65,
            E2Off => 0,
            E4End => 0,
            E4Len => 30,
            E4Loc => 59,
            E7End => 17,
            E7Len => 30,
            E7Loc => 57,
            E7Off => 56,
            E7Same => 33,
            E9End => 0,
            E9Len => 6,
            E9Loc => 207,
            E9Off => 9,
            EnvLoop => 0,
            EnvRead => 72,
            EnvStop => 0,
            EnvVol => 23,
            EnvWait => 20,
            FreqLoop => 0,
            FreqNote => 117,
            FreqRead => 27,
            FreqStop => 0,
            Fx => 79,
            OnLen => 6,
            OnLoc => 8,
            Period => 0,
            Porta => 0,
            Row => 100,
            RowAlt => 5,
            RowEmpty => 0,
            RowEnd => 0,
            RowEnv => 170,
            RowNext => 142,
            RowNote => 53,
            RowTrack => 0,
            Rows => 0,
            SlideOff => 77,
            VibDelay => 0,
            VibDown => 77,
            VibDownTurn => 94,
            VibUp => 79,
            VibUpTurn => 88,
            Write => 0,
            _ => self.cost_a94(),
        }
    }

    fn cost_ma58(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 7,
            E2Len => 30,
            E2Loc => 65,
            E2Off => 0,
            E4End => 0,
            E4Len => 30,
            E4Loc => 61,
            E7End => 24,
            E7Len => 30,
            E7Loc => 57,
            E7Off => 59,
            E7Same => 35,
            EnvLoop => 0,
            EnvRead => 73,
            EnvStop => 0,
            EnvVol => 19,
            EnvWait => 17,
            FreqLoop => 0,
            FreqNote => 114,
            FreqRead => 38,
            FreqStop => 0,
            Fx => 64,
            OnLen => 6,
            OnLoc => 8,
            Period => 92,
            Porta => 0,
            PortaUp => 54,
            Row => 99,
            RowAlt => 9,
            RowEmpty => 0,
            RowEnd => 0,
            RowEnv => 177,
            RowNext => 142,
            RowNote => 52,
            RowTrack => 0,
            Rows => 0,
            SlideOff => 0,
            VibDelay => 0,
            VibDown => 71,
            VibDownTurn => 93,
            VibUp => 77,
            VibUpTurn => 88,
            Write => 0,
            _ => self.cost_a94(),
        }
    }

    fn cost_mb02(self) -> u64 {
        use Tp::*;
        match self {
            E2End => 5,
            E2Len => 30,
            E2Loc => 65,
            E2Off => 19,
            E7End => 8,
            E7Len => 30,
            E7Loc => 57,
            E7Off => 71,
            E7Same => 41,
            E9End => 0,
            E9Len => 6,
            E9Loc => 207,
            E9Off => 33,
            EnvRead => 0,
            EnvStop => 0,
            EnvVol => 16,
            FreqLoop => 0,
            FreqNote => 106,
            FreqRead => 37,
            FreqStop => 0,
            Fx => 54,
            OnLen => 6,
            OnLoc => 8,
            Period => 0,
            Porta => 0,
            Row => 0,
            RowAlt => 6,
            RowEmpty => 0,
            RowEnd => 101,
            RowEnv => 226,
            RowNext => 143,
            RowNote => 0,
            RowTrack => 0,
            Rows => 0,
            SlideOff => 175,
            VibDelay => 0,
            VibDown => 76,
            VibDownTurn => 85,
            VibUp => 72,
            VibUpTurn => 78,
            Write => 0,
            _ => self.cost_a94(),
        }
    }
    // GENERATED-COSTS-END
}

/// A TFMX replayer's voice structure ($52 bytes), the fields it uses.
#[derive(Default, Clone, Copy)]
struct TVoice {
    /// The position list (this voice's column) at the song's start and end,
    /// and the next position's offset.
    pos: u32,
    pos_end: u32,
    pos_off: u16,
    /// The portamento's sum (16.16).
    porta: u32,
    /// The loop, written after the DMA on.
    loop_addr: u32,
    loop_len: u16,
    /// The envelope and the frequency table, and the positions in them.
    env: u32,
    env_pos: u16,
    freq: u32,
    /// The row's table, which E0 returns to when E7 switched tables.
    freq_orig: u32,
    freq_pos: u16,
    /// The track and the row's offset in it.
    track: u32,
    row: u16,
    /// The sample slide (E5): the sample's offset and end, position, step and
    /// length in words, whether it moved, its delay.
    slide_start: u32,
    slide_end: u32,
    slide_pos: u16,
    slide_delta: u16,
    slide_len: u16,
    slide_moved: bool,
    slide_counter: u8,
    slide_speed: u8,
    slide_on: bool,
    slide_done: bool,
    dma: u16,
    vib_delta: u16,
    note: u8,
    info: u8,
    env_transpose: u8,
    env_counter: u8,
    env_speed: u8,
    sustain: u8,
    freq_wait: u8,
    vib_speed: u8,
    vib_depth: u8,
    vib_delay: u8,
    prev_info: u8,
    sample: u8,
    transpose: u8,
    track_transpose: u8,
    volume: u8,
    vib_flag: u8,
}

/// Hippel's TFMX modules, played as their own replayer does (the Eagleplayer
/// calls the module's code: its first entry with the subsong, its second at
/// every tick). The port follows that code, not NostalgicPlayer, where they
/// differ:
/// * (the $A94 build) the rows are read before the effects, and a voice's
///   period and volume are written together; the DMA on comes after three line
///   waits, the loops after one more; other builds differ (see `Build`);
/// * the tables are read from the module (the periods from the replayer's code,
///   so a note past them reads the song data after them), and the empty table
///   is the replayer's own;
/// * the voices' InitSound writes point each voice at its own register, with a
///   length of 1 word;
/// * position byte 3 of $80 stops the player (DMA off and volume 0 every tick);
///   the positions wrap with no song end (the original has none);
/// * frequency table commands past E9, and envelope commands past E8, jump
///   through data in the original; the port stops the table there.
pub struct Tfmx {
    ver: Build,
    /// The counts InitPlayer loops over.
    subsongs_n: u64,
    samples_n: u64,
    /// The module was saved with a song playing (see `Build::song_var`): played as
    /// UADE's player plays it, the song started as for any module, without the
    /// Eagleplayer's first stop, and a subsong change silencing the four voices.
    dead: bool,
    periods: u32,
    empty: u32,
    freqs: u32,
    envs: u32,
    tracks: u32,
    track_size: u16,
    positions: u32,
    subsongs_at: u32,
    sample_info: u32,
    samples: u32,
    subsong_count: u16,
    sub: u16,
    counter: u16,
    speed: u16,
    stopped: bool,
    led_done: bool,
    dma_acc: u16,
    /// Loops to write after the next tick's DMA on, and those set this tick.
    pend: [bool; 4],
    pend_now: [bool; 4],
    out: [(u16, u16); 4],
    voices: [TVoice; 4],
    trace: TTrace,
}

/// The trace of code points, as `Trace` (HIPPEL_TRACE=file).
#[derive(Default)]
struct TTrace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    pend: Vec<Tp>,
    timing: Option<Timing>,
}

impl TTrace {
    fn open() -> Self {
        let out = std::env::var_os("HIPPEL_TRACE")
            .and_then(|p| std::fs::File::create(p).ok())
            .map(std::io::BufWriter::new);
        TTrace { out, pend: Vec::new(), timing: None }
    }

    fn at(&mut self, hw: &mut Hw, e: Tp) {
        hw.wait(e.cost(self.timing.unwrap_or(Timing::A94)));
        if self.out.is_some() {
            self.pend.push(e);
        }
    }

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

/// The replayer's wait for the next line: it reads VHPOSR's line byte, then
/// reads it again every 21 CPU cycles until it changes. In CPU cycles (half
/// colour clocks), from the first read to the read that sees the new line.
fn poll_line(first: u64) -> u64 {
    const P: u64 = 21;
    let next = (first / (2 * LINE) + 1) * 2 * LINE;
    first + (next - first).div_ceil(P) * P
}

/// The polling's times, in CPU cycles from the last voice's write to the first
/// wait's first read (`Build::poll_from`) and from one wait's end to the next
/// one's first read; in colour clocks from the last wait's end to the DMA on
/// (`Build::poll_to`); in CPU cycles from the DMA on to the first wait's first
/// read, and in colour clocks from the last wait's end to the first loop's
/// write. Fitted to the reference's writes: in the $A94 build 74% of 2603 DMA
/// ons come at the same colour clock, 23% one later.
const POLL_AGAIN: u64 = 52;
const POLL_FROM_DMA: u64 = 44;
const POLL_TO_LOOP: u64 = 20;

fn new_tfmx(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (ver, t) = tfmx_version(d)?;
    let h = |o: usize| be16(d, t + o) as u32;
    // InitPlayer: the parts' addresses from the counts in the header
    let freqs = base + t as u32 + 0x20;
    let envs = freqs + ((h(4) + 1) << 6);
    let tracks = envs + ((h(6) + 1) << 6);
    let track_size = h(0xc) as u16;
    let positions = tracks + (h(8) + 1) * h(0xc);
    let subsongs_at = positions + (h(0xa) + 1) * 12;
    let sample_info = subsongs_at + (h(0x10) + 1) * 6;
    let samples = sample_info + h(0x12) * 30;
    let find = |pat: &[u8]| (0..t.saturating_sub(pat.len())).find(|&i| &d[i..i + pat.len()] == pat);
    let periods = base + find(&[0x06, 0xb0, 0x06, 0x50, 0x05, 0xf4])? as u32;
    let empty = base + find(&[1, 0, 0, 0, 0, 0, 0, 0xe1])? as u32;
    let mut p = Tfmx {
        ver,
        subsongs_n: h(0x10) as u64 + 1,
        samples_n: h(0x12) as u64,
        dead: ver.song_var && be16(d, t - 6) != 0,
        periods,
        empty,
        freqs,
        envs,
        tracks,
        track_size,
        positions,
        subsongs_at,
        sample_info,
        samples,
        subsong_count: h(0x10) as u16,
        sub: 1,
        counter: 0,
        speed: 0,
        stopped: false,
        led_done: false,
        dma_acc: 0,
        pend: [false; 4],
        pend_now: [false; 4],
        out: [(0, 0); 4],
        voices: Default::default(),
        trace: TTrace::open(),
    };
    // the voices' fields InitSound leaves as they are, from the replayer's
    // variables (a module saved from a game's memory has them set)
    for (ch, v) in p.voices.iter_mut().enumerate().filter(|_| ver.saved_vars) {
        let a = empty + 0x24 + 0x52 * ch as u32;
        let bus = &m.bus;
        v.loop_addr = bus.r32(a + 0x10);
        v.slide_start = bus.r32(a + 0x20);
        v.slide_end = bus.r32(a + 0x24);
        v.slide_pos = bus.r16(a + 0x28);
        v.note = bus.r8(a + 0x42);
        v.info = bus.r8(a + 0x43);
        v.vib_flag = bus.r8(a + 0x51);
    }
    if ver.early_vars {
        for ch in 0..ver.voices {
            let a = empty + 0x6a + 0x4a * ch as u32;
            let bus = &m.bus;
            p.voices[ch].loop_len = bus.r16(a);
            p.voices[ch].loop_addr = bus.r32(a + 2);
            p.pend[ch] = bus.r16(a + 6) != 0;
        }
    }
    p.trace.timing = Some(ver.timing);
    let mut hw = Hw { bus: &mut m.bus };
    hw.wait(ver.init_at);
    p.init(1, &mut hw);
    Some(Box::new(p))
}


impl Replayer for Tfmx {
    fn format(&self) -> &'static str {
        "Hippel TFMX"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        false
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.subsong_count, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if self.dead {
            // UADE's player stops the DMA and silences all four voices
            // (measured from its logs)
            self.trace.write(hw, "dma");
            hw.dmacon(0x000f);
            hw.wait(8);
            for ch in 0..4 {
                self.trace.write(hw, "vol");
                hw.vol(ch, 0);
                hw.wait(6);
            }
            hw.wait(1160);
        } else if self.ver.init_stop {
            // the stop the Eagleplayer asks for first
            self.trace.write(hw, "dma");
            hw.dmacon(0x000f);
            hw.wait(451);
        }
        self.init(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.trace.write(hw, "tick");
        self.trace.at(hw, Tp::Start);
        if self.stopped && self.ver.split {
            // stopped: the DMA off, each voice's last period and volume 0
            self.trace.at(hw, Tp::Stopped);
            self.trace.write(hw, "dma");
            hw.dmacon(0x000f);
            for ch in 0..self.ver.voices {
                self.trace.at(hw, Tp::Write);
                self.trace.write(hw, "per");
                hw.per(ch, self.out[ch].0);
                self.trace.at(hw, Tp::WriteVol);
                self.trace.write(hw, "vol");
                hw.vol(ch, 0);
            }
            if self.ver.silent_last {
                self.trace.at(hw, Tp::Write);
                self.trace.write(hw, "per");
                hw.per(self.ver.voices, 0);
                self.trace.at(hw, Tp::WriteVol);
                self.trace.write(hw, "vol");
                hw.vol(self.ver.voices, 0);
            }
            self.trace.write(hw, "end");
            return;
        }
        if self.stopped {
            self.trace.at(hw, Tp::Stopped);
            self.trace.write(hw, "dma");
            hw.dmacon(0x000f);
            for ch in 0..4 {
                self.trace.at(hw, Tp::StopVol);
                self.trace.write(hw, "vol");
                hw.vol(ch, 0);
            }
            self.trace.write(hw, "end");
            return;
        }
        self.dma_acc = 0;
        self.counter = self.counter.wrapping_sub(1);
        if self.counter == 0 {
            self.counter = self.speed;
            self.trace.at(hw, Tp::Rows);
            for ch in 0..self.ver.voices {
                self.row(ch, hw);
            }
        }
        if self.ver.split {
            self.tick_split(hw);
            return;
        }
        for ch in 0..self.ver.voices {
            let (per, vol) = self.effects(ch, hw);
            self.trace.at(hw, Tp::Write);
            self.trace.write(hw, "per");
            hw.per(ch, per);
            self.trace.write(hw, "vol");
            hw.vol(ch, vol);
        }
        if self.dma_acc == 0 && !self.ver.dma_always {
            self.trace.at(hw, Tp::NoDma);
            self.trace.write(hw, "end");
            return;
        }
        // the loops into registers, three line waits, the DMA on, one more
        self.trace.at(hw, Tp::Loops);
        let c = hw.bus.clock;
        let mut t = poll_line(2 * c + self.ver.poll_from);
        for _ in 1..self.ver.waits {
            t = poll_line(t + POLL_AGAIN);
        }
        hw.wait(((t / 2) as i64 + self.ver.poll_to - c as i64).max(0) as u64);
        self.trace.write(hw, "dma");
        hw.dmacon(0x8000 | self.dma_acc);
        let c = hw.bus.clock;
        let mut t = poll_line(2 * c + POLL_FROM_DMA);
        for _ in 1..self.ver.waits_after {
            t = poll_line(t + POLL_AGAIN);
        }
        hw.wait(t / 2 + POLL_TO_LOOP - c);
        self.trace.at(hw, Tp::LineOn);
        for ch in 0..self.ver.loop_voices {
            let v = self.voices[ch];
            if ch > 0 {
                self.trace.at(hw, Tp::OnLoc);
            }
            self.trace.write(hw, "adr");
            hw.loc(ch, v.loop_addr);
            self.trace.at(hw, Tp::OnLen);
            self.trace.write(hw, "len");
            hw.len(ch, v.loop_len);
        }
        self.trace.write(hw, "end");
    }
}

impl Tfmx {
    /// A tick of the builds that write the DMA on and the loops after all the
    /// voices' effects, and the periods and volumes after those.
    fn tick_split(&mut self, hw: &mut Hw) {
        let n = self.ver.voices;
        for ch in 0..n {
            let (per, vol) = self.effects(ch, hw);
            self.out[ch] = (per, vol);
        }
        self.trace.at(hw, Tp::Loops);
        self.trace.write(hw, "dma");
        hw.dmacon(0x8000 | self.dma_acc);
        self.trace.at(hw, Tp::LineOn);
        let mut looped = false;
        for ch in 0..n {
            // a voice that restarted its DMA or set a sample this tick has its loop
            // written now, and so has one the saved variables left pending
            let (restarted, touched) = (self.dma_acc & (1 << ch) != 0, self.pend_now[ch]);
            if restarted || touched || self.pend[ch] {
                self.pend[ch] = false;
                self.pend_now[ch] = false;
                looped = true;
                let v = self.voices[ch];
                self.trace.at(hw, Tp::OnLoc);
                self.trace.write(hw, "adr");
                hw.loc(ch, v.loop_addr);
                self.trace.at(hw, Tp::OnLen);
                self.trace.write(hw, "len");
                hw.len(ch, v.loop_len);
            }
        }
        self.pend_now = [false; 4];
        self.trace.at(hw, if looped { Tp::PerStart } else { Tp::PerStart0 });
        for ch in 0..n {
            let (per, vol) = self.out[ch];
            self.trace.at(hw, Tp::Write);
            self.trace.write(hw, "per");
            hw.per(ch, per);
            self.trace.at(hw, Tp::WriteVol);
            self.trace.write(hw, "vol");
            hw.vol(ch, vol);
        }
        if self.ver.silent_last {
            self.trace.at(hw, Tp::Write);
            self.trace.write(hw, "per");
            hw.per(n, 0);
            self.trace.at(hw, Tp::WriteVol);
            self.trace.write(hw, "vol");
            hw.vol(n, 0);
        }
        self.trace.write(hw, "end");
    }

    /// The module's first entry: InitPlayer once (the LED off), then with
    /// subsong `n` the subsong's speed and InitSound, or with 0 the stop.
    fn init(&mut self, n: u16, hw: &mut Hw) {
        let first = !self.led_done;
        if !self.led_done {
            self.led_done = true;
            use m68k::AddressBus;
            let v = hw.bus.read_byte(0xbfe001);
            hw.bus.write_byte(0xbfe001, v | 2);
            if self.ver.init_k == 0 {
                hw.wait(self.ver.led_gap);
            } else {
                // InitPlayer's loops over the subsongs and samples
                let total = self.ver.init_k + 9 * self.samples_n + (584 * self.subsongs_n + 5) / 10;
                if self.ver.init_stop && !self.dead {
                    // the stop the Eagleplayer asks for first
                    hw.wait(self.ver.led_gap);
                    self.trace.write(hw, "dma");
                    hw.dmacon(0x000f);
                    hw.wait(total - self.ver.led_gap);
                } else {
                    hw.wait(total);
                }
            }
        }
        if n == 0 {
            self.stopped = true;
            return;
        }
        self.sub = n;
        // the subsong's start, end and speed: the replayer steps through its
        // table of 6-byte entries by 4 bytes, so a subsong past the first reads
        // the words of two entries
        let e = self.subsongs_at.wrapping_add((n as u32 - 1) * self.ver.sub_step);
        let (start, end) = (hw.bus.r16(e) as u32, hw.bus.r16(e + 2) as u32);
        self.speed = hw.bus.r16(e + 4);
        self.trace.write(hw, "dma");
        hw.dmacon(0x000f);
        hw.wait(8);
        hw.bus.write_word(0xdff09a, 0x0780);
        hw.wait(377);
        for ch in 0..self.ver.init_voices {
            let v = &mut self.voices[ch];
            v.freq = self.empty;
            v.freq_orig = self.empty;
            v.env = self.empty;
            v.env_counter = 1;
            v.env_speed = 1;
            v.sustain = 0;
            v.env_pos = 0;
            v.freq_wait = 0;
            v.vib_speed = 0;
            v.vib_depth = 0;
            v.slide_on = false;
            v.vib_delta = 0;
            v.vib_delay = 0;
            v.prev_info = 0;
            v.sample = 0xff;
            v.row = 0;
            v.transpose = 0;
            v.volume = 0;
            v.freq_pos = 0;
            v.porta = 0;
            if !self.ver.early_vars {
                v.loop_len = 0;
            }
            v.dma = 1 << ch;
            let col = 3 * ch as u32;
            v.pos = self.positions + start * 12 + col;
            v.pos_end = self.positions + (end + 1) * 12 + col;
            v.pos_off = 12;
            let p = v.pos;
            // each voice at its own register, 1 word, or at the empty table
            let (r, l) = match self.ver.init_loc {
                Some(o) => (self.empty + o, self.ver.init_len),
                None => (0xdff0a0 + 16 * ch as u32, self.ver.init_len),
            };
            hw.loc(ch, r);
            hw.wait(6);
            hw.len(ch, l);
            hw.wait(4);
            hw.per(ch, self.ver.init_per);
            hw.wait(4);
            hw.vol(ch, self.ver.init_vol);
            let bus = &*hw.bus;
            let (t, tt, et) = (bus.r8(p), bus.r8(p + 1), bus.r8(p + 2));
            let v = &mut self.voices[ch];
            v.track = self.tracks + t as u32 * self.track_size as u32;
            v.track_transpose = tt;
            v.env_transpose = et;
            hw.wait(495 - 14);
        }
        if first {
            hw.wait(self.ver.init_tail);
        }
        self.counter = if self.ver.counter_from_speed { self.speed } else { 1 };
        self.stopped = false;
    }

    fn dma_off(&mut self, ch: usize, hw: &mut Hw) {
        let b = self.voices[ch].dma;
        self.dma_acc |= b;
        self.trace.write(hw, "dma");
        hw.dmacon(b);
    }

    /// The next row of a voice: at the track's end (or a note of 1) the next
    /// position, then a note with its envelope and frequency table.
    fn row(&mut self, ch: usize, hw: &mut Hw) {
        self.trace.at(hw, Tp::Row);
        let size = self.track_size;
        let v = self.voices[ch];
        let c = hw.bus.r8(v.track + v.row as u32) & 0x7f;
        if c == 1 || v.row == size {
            self.trace.at(hw, Tp::RowNext);
            let v = &mut self.voices[ch];
            let mut p = v.pos + v.pos_off as u32;
            if p == v.pos_end {
                v.pos_off = 0;
                p = v.pos;
            }
            let bus = &*hw.bus;
            let (t, tt, et) = (bus.r8(p), bus.r8(p + 1), bus.r8(p + 2));
            v.track_transpose = tt;
            v.env_transpose = et;
            if et == 0x80 {
                self.stopped = true;
                self.trace.at(hw, Tp::RowStop);
            }
            let v = &mut self.voices[ch];
            v.row = 0;
            v.track = self.tracks + t as u32 * size as u32;
            v.pos_off = v.pos_off.wrapping_add(12);
            self.trace.at(hw, Tp::RowTrack);
        }
        let v = self.voices[ch];
        let a = v.track + v.row as u32;
        let c = hw.bus.r8(a);
        let n = c & 0x7f;
        if n == 0 {
            self.trace.at(hw, Tp::RowEmpty);
        } else {
            self.trace.at(hw, Tp::RowNote);
            // the info byte of the row before (of the track's last row at its start)
            let before = if v.row == 0 { a + 1 + size as u32 } else { a + 1 } - 2;
            let prev = hw.bus.r8(before);
            let info = hw.bus.r8(a + 1);
            let v = &mut self.voices[ch];
            v.porta = 0;
            v.note = n;
            v.prev_info = prev;
            v.info = info;
            if c & 0x80 == 0 {
                self.trace.at(hw, Tp::RowEnv);
                let et = self.voices[ch].env_transpose;
                if self.ver.mute {
                    self.dma_off(ch, hw);
                }
                let e = self.envs + ((((info & 0x1f).wrapping_add(et)) as u32 & 0xffff) << 6);
                let bus = &*hw.bus;
                let (sp, f, vs, dp, dl) = (bus.r8(e), bus.r8(e + 1), bus.r8(e + 2), bus.r8(e + 3), bus.r8(e + 4));
                let v = &mut self.voices[ch];
                v.env_pos = 0;
                v.env_counter = sp;
                v.env_speed = sp;
                v.vib_speed = vs;
                v.vib_flag = 0x40;
                v.vib_depth = dp;
                v.vib_delta = dp as u16;
                v.vib_delay = dl;
                v.env = e + 5;
                let f = if self.ver.freq_prev && info & 0x40 != 0 {
                    self.trace.at(hw, Tp::RowAlt);
                    prev
                } else {
                    f
                };
                let v = &mut self.voices[ch];
                v.freq = self.freqs + ((f as u32) << 6);
                v.freq_orig = v.freq;
                v.freq_pos = 0;
                v.freq_wait = 0;
                v.sustain = 0;
            }
        }
        self.trace.at(hw, Tp::RowEnd);
        let v = &mut self.voices[ch];
        v.row = v.row.wrapping_add(2);
    }

    /// A voice's effects: the frequency table, the slide, the envelope; the
    /// period and volume to write.
    fn effects(&mut self, ch: usize, hw: &mut Hw) -> (u16, u16) {
        self.trace.at(hw, Tp::Fx);
        let wait_skips_slide = self.ver.wait_skips_slide;
        let mut slide = true;
        loop {
            if self.voices[ch].freq_wait != 0 {
                self.voices[ch].freq_wait -= 1;
                self.trace.at(hw, Tp::FreqWait);
                slide = !wait_skips_slide;
                break;
            }
            if !self.freq_table(ch, hw) {
                break;
            }
        }
        if slide {
            self.slide(ch, hw);
        }
        self.envelope(ch, hw);
        let per = self.period(ch, hw);
        (per, self.voices[ch].volume as u16)
    }

    /// A sample's info (30 bytes: the sample's offset, length in words, loop
    /// start in bytes and loop length).
    fn info(&self, n: u8) -> u32 {
        self.sample_info + n as u32 * 30
    }

    /// E2/E4/E7: the sample's start and length, its loop kept.
    fn sample(&mut self, ch: usize, hw: &mut Hw, n: u8, at: [Tp; 2]) {
        let i = self.info(n);
        let bus = &*hw.bus;
        let s = bus.r32(i + 0x12).wrapping_add(self.samples);
        let (len, ls, ll) = (bus.r16(i + 0x16), bus.r16(i + 0x1a), bus.r16(i + 0x1c));
        self.trace.at(hw, at[0]);
        self.trace.write(hw, "adr");
        hw.loc(ch, s);
        self.trace.at(hw, at[1]);
        self.trace.write(hw, "len");
        hw.len(ch, len);
        let v = &mut self.voices[ch];
        v.loop_addr = s.wrapping_add(ls as u32);
        v.loop_len = ll;
        self.pend_now[ch] = true;
    }

    /// The byte at the table's position is the transpose.
    fn transpose_next(&mut self, ch: usize, hw: &mut Hw) -> bool {
        self.trace.at(hw, Tp::FreqNote);
        let v = self.voices[ch];
        let c = hw.bus.r8(v.freq.wrapping_add(v.freq_pos as u32));
        let v = &mut self.voices[ch];
        v.transpose = c;
        v.freq_pos = v.freq_pos.wrapping_add(1);
        false
    }

    /// Reads the frequency table to a transpose or a stop; true after E8.
    fn freq_table(&mut self, ch: usize, hw: &mut Hw) -> bool {
        loop {
            self.trace.at(hw, Tp::FreqRead);
            let v = self.voices[ch];
            let a = v.freq.wrapping_add(v.freq_pos as u32);
            let c = hw.bus.r8(a);
            let b = hw.bus.r8(a + 1);
            if c < 0xe0 {
                self.trace.at(hw, Tp::FreqNote);
                let v = &mut self.voices[ch];
                v.transpose = c;
                v.freq_pos = v.freq_pos.wrapping_add(1);
                return false;
            }
            let enabled = match c {
                0xe5 => self.ver.e5 != 0,
                0xe6 => self.ver.e6,
                0xe7 => self.ver.e7 != 0,
                0xe9 => self.ver.e9,
                _ => true,
            };
            if !enabled {
                return self.transpose_next(ch, hw);
            }
            match c {
                0xe0 => {
                    self.trace.at(hw, Tp::FreqLoop);
                    let v = &mut self.voices[ch];
                    if self.ver.e7 == 1 {
                        // back to the row's table, whatever E7 switched to
                        v.freq = v.freq_orig;
                        v.freq_pos = (b & 0x3f) as u16;
                    } else {
                        v.freq_pos = b as u16;
                    }
                }
                0xe2 => {
                    self.voices[ch].sample = 0xff;
                    self.trace.at(hw, Tp::E2Off);
                    self.dma_off(ch, hw);
                    self.sample(ch, hw, b, [Tp::E2Loc, Tp::E2Len]);
                    self.trace.at(hw, Tp::E2End);
                    let v = &mut self.voices[ch];
                    v.env_pos = 0;
                    v.env_counter = 1;
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                    v.slide_on = false;
                    if !self.ver.effloop {
                        return self.transpose_next(ch, hw);
                    }
                }
                0xe3 => {
                    self.trace.at(hw, Tp::E3);
                    let d = hw.bus.r8(a + 2);
                    let v = &mut self.voices[ch];
                    v.freq_pos = v.freq_pos.wrapping_add(3);
                    v.vib_speed = b;
                    v.vib_depth = d;
                    if !self.ver.effloop {
                        return self.transpose_next(ch, hw);
                    }
                }
                0xe4 => {
                    self.sample(ch, hw, b, [Tp::E4Loc, Tp::E4Len]);
                    self.trace.at(hw, Tp::E4End);
                    let v = &mut self.voices[ch];
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                    v.slide_on = false;
                    if !self.ver.effloop {
                        return self.transpose_next(ch, hw);
                    }
                }
                0xe5 if self.ver.e5 == 1 => {
                    // plays the sample's loop part from an offset
                    self.trace.at(hw, Tp::E2Off);
                    self.dma_off(ch, hw);
                    let i = self.info(b);
                    let bus = &*hw.bus;
                    let idx = bus.r8(a + 2) as u32;
                    let s = bus.r32(i + 0x12).wrapping_add(self.samples);
                    let (ls, ll) = (bus.r16(i + 0x1a) as u32, bus.r16(i + 0x1c) as u32);
                    let at = s.wrapping_add(ls).wrapping_add(idx * ll * 2);
                    self.trace.at(hw, Tp::E2Loc);
                    self.trace.write(hw, "adr");
                    hw.loc(ch, at);
                    self.trace.at(hw, Tp::E2Len);
                    self.trace.write(hw, "len");
                    hw.len(ch, ll as u16);
                    self.trace.at(hw, Tp::E2End);
                    let v = &mut self.voices[ch];
                    v.loop_addr = at;
                    v.loop_len = ll as u16;
                    self.pend_now[ch] = true;
                    let v = &mut self.voices[ch];
                    v.env_pos = 0;
                    v.env_counter = 1;
                    v.freq_pos = v.freq_pos.wrapping_add(3);
                    return self.transpose_next(ch, hw);
                }
                0xe5 => {
                    self.trace.at(hw, Tp::E5Off);
                    self.dma_off(ch, hw);
                    self.trace.at(hw, Tp::E5End);
                    let i = self.info(b);
                    let bus = &*hw.bus;
                    let start = bus.r32(i + 0x12);
                    let len = bus.r16(i + 0x16);
                    let w = |k: u32| (bus.r8(a + k) as u16) << 8 | bus.r8(a + k + 1) as u16;
                    let mut pos = w(2);
                    if pos == 0xffff {
                        pos = len;
                    }
                    let (sl, sd, ss) = (w(4), w(6), bus.r8(a + 8));
                    let v = &mut self.voices[ch];
                    v.slide_start = start;
                    v.slide_end = start.wrapping_add(2 * len as u32);
                    v.slide_pos = pos;
                    v.slide_len = sl;
                    v.slide_delta = sd;
                    v.slide_counter = 0;
                    v.slide_speed = ss;
                    v.slide_done = false;
                    v.slide_moved = false;
                    v.slide_on = true;
                    v.env_pos = 0;
                    v.env_counter = 1;
                    v.freq_pos = v.freq_pos.wrapping_add(9);
                }
                0xe6 => {
                    self.trace.at(hw, Tp::E6);
                    let bus = &*hw.bus;
                    let w = |k: u32| (bus.r8(a + k) as u16) << 8 | bus.r8(a + k + 1) as u16;
                    let (sl, sd, ss) = (w(1), w(3), bus.r8(a + 5));
                    let v = &mut self.voices[ch];
                    v.slide_len = sl;
                    v.slide_delta = sd;
                    v.slide_counter = 0;
                    v.slide_speed = ss;
                    v.slide_moved = false;
                    v.slide_done = false;
                    v.freq_pos = v.freq_pos.wrapping_add(6);
                }
                0xe7 if self.ver.e7 == 1 => {
                    // switches to another frequency table
                    self.trace.at(hw, Tp::E8);
                    let v = &mut self.voices[ch];
                    v.freq = self.freqs + ((b as u32) << 6);
                    v.freq_orig = v.freq;
                    v.freq_pos = 0;
                }
                0xe7 => {
                    if b == self.voices[ch].sample {
                        self.trace.at(hw, Tp::E7Same);
                    } else {
                        self.voices[ch].sample = b;
                        self.trace.at(hw, Tp::E7Off);
                        self.dma_off(ch, hw);
                        self.sample(ch, hw, b, [Tp::E7Loc, Tp::E7Len]);
                        self.trace.at(hw, Tp::E7End);
                    }
                    let v = &mut self.voices[ch];
                    v.env_pos = 0;
                    v.env_counter = 1;
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                    v.slide_on = false;
                    if !self.ver.effloop {
                        return self.transpose_next(ch, hw);
                    }
                }
                0xe8 => {
                    self.trace.at(hw, Tp::E8);
                    let v = &mut self.voices[ch];
                    v.freq_wait = b;
                    v.freq_pos = v.freq_pos.wrapping_add(2);
                    return true;
                }
                0xe9 => {
                    self.voices[ch].sample = 0xff;
                    let i = self.info(b);
                    let ssmp = self.ver.e9_even || hw.bus.r32(hw.bus.r32(i + 0x12).wrapping_add(self.samples)) == 0x5353_4d50;
                    if ssmp {
                        self.trace.at(hw, Tp::E9Off);
                        self.dma_off(ch, hw);
                        self.sub_sample(ch, hw, a);
                        self.trace.at(hw, Tp::E9End);
                    }
                    let v = &mut self.voices[ch];
                    if ssmp {
                        v.env_pos = 0;
                        v.env_counter = 1;
                        v.slide_on = false;
                    }
                    v.freq_pos = v.freq_pos.wrapping_add(3);
                    if !self.ver.effloop {
                        return self.transpose_next(ch, hw);
                    }
                }
                // E1 stops the table; past E9 the original jumps through data
                _ => {
                    self.trace.at(hw, Tp::FreqStop);
                    return false;
                }
            }
        }
    }

    /// E9: sub-sample `n` of an "SSMP" sample, with a loop of one word (its
    /// first byte doubled).
    fn sub_sample(&mut self, ch: usize, hw: &mut Hw, a: u32) {
        let i = self.info(hw.bus.r8(a + 1));
        let bus = &mut *hw.bus;
        let s = bus.r32(i + 0x12).wrapping_add(self.samples);
        let count = bus.r16(s + 4) as u32;
        let skip = (bus.r16(s + 6) << 2) as i16 as i32 as u32;
        let headers = s + 8;
        let data = headers.wrapping_add(count * 0x18).wrapping_add(skip);
        let h = headers.wrapping_add(bus.r8(a + 2) as u32 * 0x18);
        let mask = if self.ver.e9_even { !1 } else { !0 };
        let start = bus.r32(h) & mask;
        let end = bus.r32(h + 4) & mask;
        let len = (end.wrapping_sub(start) >> 1) as u16;
        let at = start.wrapping_add(data);
        if self.ver.e9_even {
            let b = bus.r8(at);
            bus.w8(at.wrapping_add(1), b);
        }
        self.trace.at(hw, Tp::E9Loc);
        self.trace.write(hw, "adr");
        hw.loc(ch, at);
        self.trace.at(hw, Tp::E9Len);
        self.trace.write(hw, "len");
        hw.len(ch, len);
        let v = &mut self.voices[ch];
        v.loop_addr = at;
        v.loop_len = 1;
    }

    /// The sample slide (E5), written at once.
    fn slide(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.voices[ch];
        if !v.slide_on || v.slide_done {
            self.trace.at(hw, Tp::SlideOff);
            return;
        }
        v.slide_counter = v.slide_counter.wrapping_sub(1);
        if (v.slide_counter as i8) >= 0 {
            self.trace.at(hw, Tp::SlideWait);
            return;
        }
        v.slide_counter = v.slide_speed;
        let mut pos = v.slide_pos as u32;
        let delta = v.slide_delta as i16 as i32 as u32;
        let len = v.slide_len;
        let at = if !v.slide_moved {
            v.slide_moved = true;
            Tp::SlideFirst
        } else {
            pos = pos.wrapping_add(delta);
            if (pos as i32) < 0 {
                v.slide_done = true;
                pos = pos.wrapping_sub(delta);
                Tp::SlideDone
            } else {
                let end = v.slide_start.wrapping_add(2 * pos).wrapping_add(2 * len as u32);
                if end as i32 > v.slide_end as i32 {
                    v.slide_done = true;
                    pos = pos.wrapping_sub(delta);
                    Tp::SlideDone
                } else {
                    Tp::SlideStep
                }
            }
        };
        v.slide_pos = pos as u16;
        let a = v.slide_start.wrapping_add(self.samples).wrapping_add(2 * pos);
        v.loop_len = len;
        v.loop_addr = a;
        self.trace.at(hw, at);
        self.trace.at(hw, Tp::SlideLoc);
        self.trace.write(hw, "adr");
        hw.loc(ch, a);
        self.trace.at(hw, Tp::SlideLen);
        self.trace.write(hw, "len");
        hw.len(ch, len);
    }

    /// The volume envelope.
    fn envelope(&mut self, ch: usize, hw: &mut Hw) {
        loop {
            let v = &mut self.voices[ch];
            if v.sustain != 0 {
                v.sustain -= 1;
                self.trace.at(hw, Tp::EnvSustain);
                return;
            }
            v.env_counter = v.env_counter.wrapping_sub(1);
            if v.env_counter != 0 {
                self.trace.at(hw, Tp::EnvWait);
                return;
            }
            v.env_counter = v.env_speed;
            loop {
                self.trace.at(hw, Tp::EnvRead);
                let v = self.voices[ch];
                let a = v.env.wrapping_add(v.env_pos as u32);
                let c = hw.bus.r8(a);
                match c {
                    0xe0 => {
                        self.trace.at(hw, Tp::EnvLoop);
                        self.voices[ch].env_pos = (hw.bus.r8(a + 1) as u16).wrapping_sub(5);
                    }
                    0xe8 => {
                        self.trace.at(hw, Tp::EnvHold);
                        let s = hw.bus.r8(a + 1);
                        let v = &mut self.voices[ch];
                        v.sustain = s;
                        v.env_pos = v.env_pos.wrapping_add(2);
                        break;
                    }
                    0xe1..=0xff => {
                        self.trace.at(hw, Tp::EnvStop);
                        return;
                    }
                    _ => {
                        self.trace.at(hw, Tp::EnvVol);
                        let v = &mut self.voices[ch];
                        v.volume = c;
                        v.env_pos = v.env_pos.wrapping_add(1);
                        return;
                    }
                }
            }
        }
    }

    /// The period: the note from the replayer's table, the vibrato (every
    /// other tick when its speed has bit 7 set) and the portamento.
    fn period(&mut self, ch: usize, hw: &mut Hw) -> u16 {
        self.trace.at(hw, Tp::Period);
        let v = self.voices[ch];
        let mut n = v.transpose;
        if (n as i8) >= 0 {
            n = n.wrapping_add(v.note).wrapping_add(v.track_transpose);
        }
        let mut per = hw.bus.r16(self.periods + 2 * (n & 0x7f) as u32);
        let v = &mut self.voices[ch];
        if self.ver.vib == 1 {
            // the older vibrato: a step in the note's scale, every other tick
            let mut flag = v.vib_flag;
            let mut at = Tp::VibDelay;
            if v.vib_delay == 0 {
                let depth = v.vib_depth as i32;
                let depth2 = 2 * depth;
                let mut delta = v.vib_delta as u8 as i32;
                at = Tp::VibSkip;
                if flag & 0x80 == 0 || flag & 1 == 0 {
                    if flag & 0x20 == 0 {
                        delta -= v.vib_speed as i32;
                        if delta < 0 {
                            flag |= 0x20;
                            delta = 0;
                            at = Tp::VibDownTurn;
                        } else {
                            at = Tp::VibDown;
                        }
                    } else {
                        delta += v.vib_speed as i32;
                        if delta >= depth2 {
                            flag &= !0x20;
                            delta = depth2;
                            at = Tp::VibUpTurn;
                        } else {
                            at = Tp::VibUp;
                        }
                    }
                    v.vib_delta = delta as u8 as u16;
                }
                let mut d = delta - depth;
                if d != 0 {
                    let mut value = (n & 0x7f) as i32 * 2 + 160;
                    while value < 256 {
                        d += d;
                        value += 24;
                    }
                    per = per.wrapping_add(d as u16);
                }
            } else {
                v.vib_delay -= 1;
            }
            v.vib_flag = flag ^ 1;
            self.trace.at(hw, at);
        } else if v.vib_delay != 0 {
            v.vib_delay -= 1;
            self.trace.at(hw, Tp::VibDelay);
        } else {
            let mut flag = v.vib_flag;
            let mut speed = v.vib_speed as u16;
            if speed & 0x80 != 0 {
                speed &= 0x7f;
                flag ^= 1;
            }
            let depth = v.vib_depth as u16;
            let mut d1 = v.vib_delta;
            let at = if flag & 1 != 0 {
                Tp::VibSkip
            } else if flag & 0x20 == 0 {
                d1 = d1.wrapping_sub(speed);
                if (d1 as i16) < 0 {
                    flag |= 0x20;
                    d1 = 0;
                    Tp::VibDownTurn
                } else {
                    Tp::VibDown
                }
            } else {
                d1 = d1.wrapping_add(speed);
                if (d1 as i16) > (2 * depth) as i16 {
                    flag &= !0x20;
                    d1 = 2 * depth;
                    Tp::VibUpTurn
                } else {
                    Tp::VibUp
                }
            };
            v.vib_delta = d1;
            v.vib_flag = flag;
            per = per.wrapping_add(d1.wrapping_sub(depth));
            self.trace.at(hw, at);
        }
        let v = &mut self.voices[ch];
        if v.info & 0x20 != 0 {
            let b = v.prev_info;
            if (b as i8) >= 0 {
                v.porta = v.porta.wrapping_add((b as u32) << 11);
                per = per.wrapping_sub((v.porta >> 16) as u16);
                self.trace.at(hw, Tp::PortaUp);
            } else {
                v.porta = v.porta.wrapping_add((b.wrapping_neg() as u32) << 11);
                per = per.wrapping_add((v.porta >> 16) as u16);
                self.trace.at(hw, Tp::PortaDown);
            }
        } else {
            self.trace.at(hw, Tp::Porta);
        }
        per
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// Where the module's parts are: the tables of frequency tables, envelopes
    /// and tracks (one each), one position, two subsong entries (so one
    /// subsong), one sample's info, then the parts and the sample's data.
    const FREQS: usize = 0x40;
    const ENVS: usize = 0x42;
    const TRACKS: usize = 0x44;
    const POS: usize = 0x48;
    const SUBS: usize = POS + 12;
    const INFO: usize = SUBS + 12;
    const FREQ0: usize = INFO + 10;
    const ENV0: usize = FREQ0 + 4;
    const TRACK0: usize = ENV0 + 7;
    const SAMPLES: usize = TRACK0 + 3;

    fn module() -> Vec<u8> {
        let mut d = b"COSO".to_vec();
        for at in [FREQS, ENVS, TRACKS, POS, SUBS, INFO, SAMPLES] {
            d.extend((at as u32).to_be_bytes());
        }
        d.extend(b"TFMX");
        d.resize(FREQS, 0);
        for at in [FREQ0, ENV0, TRACK0] {
            d.extend((at as u16).to_be_bytes());
        }
        d.resize(POS, 0);
        d.extend([0; 12]); // every voice on track 0, no transposes
        d.extend([0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 3]); // positions 0 to 0, speed 3
        d.extend([0, 0, 0, 0, 0, 32, 0, 0, 0, 32]); // sample 0: 32 words, looped whole
        d.extend([0xe2, 0, 0, 0xe1]); // play sample 0, no transpose, stop
        d.extend([1, 0, 0, 0, 0, 0x40, 0xe1]); // speed 1, frequency table 0; volume 64
        d.extend([24, 0, 0xff]); // note 24 with envelope 0, then the next position
        d.extend((0..64).map(|i| if i < 32 { 0x40 } else { 0xc0 }));
        assert_eq!(d.len(), SAMPLES + 64);
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.hipc"), module()).unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Hippel COSO");
        assert_eq!((p.sub_min, p.sub_max), (1, 1));
        let base = e.m.deli.list.first().unwrap().0;
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound: DMA off, every voice at the samples
        assert_eq!(reg(0xDFF096)[0], 0x000F);
        assert_eq!(reg(0xDFF0A2)[0], (base + SAMPLES as u32) as u16);
        // the note: each voice's DMA off, then on together, the sample, the
        // note's period, the envelope's volume
        let dma = reg(0xDFF096);
        assert!(dma.contains(&0x0001) && dma.contains(&0x0008) && dma.contains(&0x800F));
        assert!(reg(0xDFF0A4).contains(&32));
        assert!(reg(0xDFF0A6).contains(&super::PERIODS[24]));
        assert!(reg(0xDFF0A8).contains(&64));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }

    /// A TFMX module with the song data at $A94: in place of the replayer's
    /// code a marker, the period table and the empty table; then one
    /// frequency table, envelope, track (one row) and position, one subsong
    /// and one sample.
    const T: usize = 0xa94;
    const TFREQ: usize = T + 0x20;
    const TENV: usize = TFREQ + 64;
    const TTRACK: usize = TENV + 64;
    const TPOS: usize = TTRACK + 2;
    const TSUBS: usize = TPOS + 12;
    const TINFO: usize = TSUBS + 12;
    const TSAMPLES: usize = TINFO + 30;

    fn tfmx_module() -> Vec<u8> {
        let mut d = vec![0x60, 0x00];
        d.extend(b"SYNTHTST");
        d.resize(0x100, 0);
        d.extend(super::PERIODS.iter().flat_map(|p| p.to_be_bytes()));
        d.resize(0x406, 0);
        d.extend([0x60, 0x00, 0x02, 0xb2]); // a frequency table wait slides
        d.resize(0x866, 0);
        d.extend([1, 0, 0, 0, 0, 0, 0, 0xe1]); // the empty table, then the voices
        d.resize(T, 0);
        d.extend(b"TFMX");
        // one frequency table, envelope, track, position; rows of 2 bytes; one
        // subsong; one sample
        d.extend([0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 1, 0, 1]);
        d.resize(TFREQ, 0);
        d.extend([0xe2, 0, 0, 0xe1]); // play sample 0, no transpose, stop
        d.resize(TENV, 0);
        d.extend([1, 0, 0, 0, 0, 0x40, 0xe1]); // speed 1, frequency table 0; volume 64
        d.resize(TTRACK, 0);
        d.extend([24, 0]); // note 24 with envelope 0
        d.extend([0; 12]); // every voice on track 0, no transposes
        d.extend([0, 0, 0, 0, 0, 3]); // positions 0 to 0, speed 3
        d.resize(TINFO, 0);
        d.resize(TINFO + 0x16, 0); // the sample at the samples' start
        d.extend([0, 32, 0, 0, 0, 0, 0, 32]); // 32 words, looped whole
        d.resize(TSAMPLES, 0);
        d.extend((0..64).map(|i| if i < 32 { 0x40 } else { 0xc0 }));
        d
    }

    /// The same song as an early build's module: the replayer's code is a
    /// marker, the period table and the empty table.
    fn early_module() -> Vec<u8> {
        let mut d = tfmx_module();
        d[0..2].copy_from_slice(&[0x60, 0x02]);
        d[2..10].copy_from_slice(b"SYNTHEAR");
        d[0x406..0x40a].fill(0);
        d[0x866..0x86e].fill(0);
        d[0x700..0x708].copy_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0xe1]);
        d
    }

    #[test]
    fn plays_an_early_build_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.hip"), early_module()).unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Hippel TFMX");
        let base = e.m.deli.list.first().unwrap().0;
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound points every voice at the empty table, 256 words
        assert_eq!(reg(0xDFF096)[0], 0x000F);
        assert_eq!(reg(0xDFF0A2)[0], (base + 0x700 + 0x18) as u16);
        assert_eq!(reg(0xDFF0A4)[0], 0x100);
        // the first row comes after the song's speed (3) ticks: the new note
        // turns each voice's DMA off, the sample starts, the DMA goes on, then
        // the periods and volumes follow
        let dma = reg(0xDFF096);
        assert!(dma.contains(&0x0001) && dma.contains(&0x0008) && dma.contains(&0x800F));
        assert!(reg(0xDFF0A2).contains(&((base + TSAMPLES as u32) as u16)));
        assert!(reg(0xDFF0A6).contains(&super::PERIODS[24]));
        assert!(reg(0xDFF0A8).contains(&64));
        // the loop follows one tick after the DMA on
        let on = log.iter().position(|w| w.0 == 0xDFF096 && w.1 == 0x800F).unwrap();
        let loops = log.iter().skip(on).filter(|w| w.0 == 0xDFF0A4 && w.1 == 32).count();
        assert!(loops >= 1, "no loop written after the DMA on");
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }

    #[test]
    fn plays_a_tfmx_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.hip"), tfmx_module()).unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Hippel TFMX");
        assert_eq!((p.sub_min, p.sub_max), (1, 1));
        let base = e.m.deli.list.first().unwrap().0;
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound: DMA off, every voice at its own register, 1 word
        assert_eq!(reg(0xDFF096)[0], 0x000F);
        assert_eq!(reg(0xDFF0A2)[0], 0xF0A0);
        // the note: the sample's start and length, the note's period, the
        // envelope's volume, the DMA on, the loop
        assert!(reg(0xDFF0A2).contains(&((base + TSAMPLES as u32) as u16)));
        assert!(reg(0xDFF0A4).contains(&32));
        assert!(reg(0xDFF0A6).contains(&super::PERIODS[24]));
        assert!(reg(0xDFF0A8).contains(&64));
        assert!(reg(0xDFF096).contains(&0x800F));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
