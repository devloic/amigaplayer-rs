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
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, the port follows the original (see `Coso`).

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

pub const OPEN_COSO: Open = Open { name: "Hippel-COSO", detect: detect_coso, new: new_coso };

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
        "Hippel-COSO"
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
        assert_eq!(p.format, "Hippel-COSO");
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
}
