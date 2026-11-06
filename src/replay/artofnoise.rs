//! Art of Noise, ported from NostalgicPlayer's Art of Noise player (Copyright
//! (c) 2024 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the Art of Noise Eagleplayer ("Art of Noise player module
//! V1.01", Bastian Spiegel, adapted by Buggs of DEFECT), which plays the 4-voice
//! "AON4" and the 8-voice "AON8" modules with the same code. For 8 voices it
//! asks for the Eagleplayer amplifier (8 AudioStructs, EPAMB_Direct) and writes
//! every voice through ENPP_Poke* and ENPP_DMAMask, which the host's amplifier
//! plays as virtual voices; the port pokes the same amplifier (`Hw::voice_*`).
//! Like the original it plays from the module in guest memory,
//! finding the chunks by the original's search, and keeps its variables in guest
//! memory in the layout of the original's code hunk: the fine-tune offsets, the
//! periods and the vibrato tables, then the player's and the voices' variables.
//! So the original's reads past its tables come out the same: a fine-tune past
//! 15 reads its offset from the periods, an arpeggio note past the 60 reads on
//! into the vibrato tables and the variables, an instrument past the 61 of the
//! table reads the pointers after it, an arpeggio of more than 8 notes writes
//! over the voice's next variables.
//!
//! The original runs at three times the tempo's rate (its timer is $9052D /
//! tempo, $1279 at 125) and does one thing on each interrupt: the tick proper
//! (rows, effects, the period, volume and a new note's location and length,
//! then the new notes' DMA off), then the DMA on, then every voice's loop.
//!
//! Where NostalgicPlayer differs from the original, this follows the original:
//! * the period, the volume, the DMA off (0 too), the DMA on ($8000 too) and the
//!   loop's location and length are written on every tick, for every voice;
//! * the volume is ((volume * (envelope / 2)) >> 6) * track volume >> 6 in words,
//!   and C, K, R and T do not clamp their volume;
//! * the arpeggio steps only when its counter reaches the arpeggio speed, an
//!   arpeggio note is not clamped to the table, and a professional arpeggio reads
//!   its bytes on (NostalgicPlayer reads its second byte again);
//! * every row clears the note-cut/delay counter; a row's D can go past row 63;
//! * the synth envelope's sums wrap at a byte, its attack ends when the byte
//!   goes negative; the envelope runs for a sample voice too, and for a synth
//!   voice only with a waveform;
//! * a backwards wave loop restarts from the loop's end ($40), not the wave's;
//! * F00 sets speed 0 (the rows stop) and tempo 125, with no song end; the song
//!   ends when the position passes the last one, and goes on at the restart
//!   position, unchecked;
//! * the fine-tune 0 table starts with 3424 (NostalgicPlayer: 3434);
//! * the byte and word arithmetic is kept and nothing is range-checked.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN4: Open = Open { name: "Art of Noise 4V", detect: detect4, new: new4 };
pub const OPEN8: Open = Open { name: "Art of Noise 8V", detect: detect8, new: new8 };

include!("artofnoise_tables.rs");

// Offsets in the original's code hunk (its base is $10000 in the listings).
const CODE_SIZE: u32 = 0x26e4;
/// The three external event bytes (effect X).
const EVENTS: u32 = 0x1986;
/// The 16 fine-tunes' offsets in the period table (words).
const FINE_TUNES: u32 = 0x198a;
const PERIOD_TABLE: u32 = 0x19aa;
const VIB_TABLES: u32 = 0x212a;
const G: u32 = 0x218a;
const VOICES: u32 = 0x23d0;
const VOICE_SIZE: u32 = 0x62;
/// The data hunk: 256 bytes of zeros (a sample without a loop loops one word of
/// it), then "@@".
const DATA_SIZE: u32 = 0x104;

/// $9052D / tempo is the timer; $1279 at tempo 125.
const TIMER: u16 = 0x1279;

// The player's variables (from G).
const G_SPEED: u32 = 0x20;
const G_FRAME: u32 = 0x21;
/// $FF: a position jump or pattern break is due (byte), then the row * 4.
const G_BREAK: u32 = 0x22;
const G_ROW: u32 = 0x23;
const G_LOOP_ROW: u32 = 0x24;
const G_LOOP_COUNT: u32 = 0x25;
const G_LOOP: u32 = 0x26;
const G_POSITION: u32 = 0x27;
const G_INFO: u32 = 0x28;
const G_ARPG: u32 = 0x2c;
const G_PLST: u32 = 0x30;
const G_PATT: u32 = 0x34;
/// The pattern delay's count, $FF when none.
const G_PATTERN_DELAY: u32 = 0x39;
/// The 64 waveforms' addresses, from WLEN.
const G_WAVES: u32 = 0x3a;
/// The 61 instruments' addresses.
const G_INSTRUMENTS: u32 = 0x13a;
const G_MODULE: u32 = 0x23a;
const G_TEMPO: u32 = 0x23f;
const G_NOISE_AVOID: u32 = 0x240;
const G_OVERSIZE: u32 = 0x241;
const G_PATTERN: u32 = 0x242;
/// Which interrupt of the three comes next (0: the tick, 1: DMA on, 2: loops).
const G_PHASE: u32 = 0x243;
const G_DMA: u32 = 0x244;
const G_SIZE: u32 = 0x246;

// A voice's variables.
/// 1: a new loop, 3: a new note (bit 1: DMA off, location and length).
const V_FLAG: u32 = 0x00;
const V_NOTE: u32 = 0x01;
const V_WAVE: u32 = 0x02;
const V_LENGTH: u32 = 0x06;
const V_OLD_LENGTH: u32 = 0x08;
const V_REPEAT: u32 = 0x0a;
const V_REPEAT_LENGTH: u32 = 0x0e;
const V_INSTRUMENT: u32 = 0x10;
const V_VOLUME: u32 = 0x14;
/// Note cut, delay and retrig counter.
const V_STEP: u32 = 0x15;
/// 1: a synth sound.
const V_SYNTH: u32 = 0x16;
const V_VIB_SPEED: u32 = 0x17;
/// Bits 0-3 the depth, 5-6 the waveform, 7 the sign.
const V_VIB_AMPL: u32 = 0x18;
const V_VIB_POS: u32 = 0x19;
const V_VIB_DELAY: u32 = 0x1a;
const V_PERIOD: u32 = 0x1c;
const V_SLIDE: u32 = 0x1e;
/// The arpeggio's position in bytes (word; the low byte steps).
const V_ARP_POS: u32 = 0x20;
/// The arpeggio: offsets in the period table (words), $FFFF at the end.
const V_ARP: u32 = 0x22;
const V_ARP_SPEED: u32 = 0x32;
const V_ARP_COUNT: u32 = 0x33;
const V_WAVE_ACT: u32 = 0x34;
const V_WAVE_END: u32 = 0x38;
const V_WAVE_REP: u32 = 0x3c;
const V_WAVE_REP_END: u32 = 0x40;
const V_WAVE_ADD: u32 = 0x44;
const V_WAVE_COUNT: u32 = 0x48;
const V_WAVE_SPEED: u32 = 0x49;
const V_WAVE_CTRL: u32 = 0x4a;
const V_WAVE_CONT: u32 = 0x4b;
const V_ENV_ADD: u32 = 0x4c;
const V_ENV_SUB: u32 = 0x4d;
const V_ENV_END: u32 = 0x4e;
/// 0: done, 1: attack, 2: decay.
const V_ENV: u32 = 0x4f;
const V_ENV_VOL: u32 = 0x50;
/// 1: vibrato on, $21: off.
const V_VIB_ON: u32 = 0x51;
const V_WAVE_STOP: u32 = 0x52;
const V_VIB_DONE: u32 = 0x53;
const V_VIB_CONT: u32 = 0x54;
const V_FX: u32 = 0x56;
const V_ARG: u32 = 0x57;
const V_OFFSET: u32 = 0x58;
const V_GLISS: u32 = 0x5c;
const V_SLIDING: u32 = 0x5e;
const V_WAVE_NR: u32 = 0x5f;
const V_TRACK_VOL: u32 = 0x60;

// An instrument in the module (32 bytes).
const I_TYPE: u32 = 0x00;
const I_VOLUME: u32 = 0x01;
const I_FINE_TUNE: u32 = 0x02;
const I_WAVE: u32 = 0x03;
const I_START: u32 = 0x04;
const I_LENGTH: u32 = 0x08;
const I_LOOP: u32 = 0x0c;
const I_LOOP_LENGTH: u32 = 0x10;
const I_SYNTH_LENGTH: u32 = 0x04;
const I_VIB_PARAM: u32 = 0x0a;
const I_VIB_DELAY: u32 = 0x0b;
const I_VIB_WAVE: u32 = 0x0c;
const I_WAVE_SPEED: u32 = 0x0d;
const I_WAVE_LENGTH: u32 = 0x0e;
const I_WAVE_LOOP: u32 = 0x0f;
const I_WAVE_LOOP_LENGTH: u32 = 0x10;
const I_WAVE_CTRL: u32 = 0x11;
const I_ENV_START: u32 = 0x1c;
const I_ENV_ADD: u32 = 0x1d;
const I_ENV_END: u32 = 0x1e;
const I_ENV_SUB: u32 = 0x1f;

fn detect4(d: &[u8]) -> bool {
    // the original's check is the mark alone
    d.len() >= 4 && &d[..4] == b"AON4"
}

fn new4(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect4(d) {
        return None;
    }
    new(d, base, m, 4)
}

fn detect8(d: &[u8]) -> bool {
    d.len() >= 4 && &d[..4] == b"AON8"
}

/// The 8-voice modules. The original plays them with the same code, its
/// voice count the mark less "AON0", and asks for the Eagleplayer amplifier
/// with 8 channels, poking all of them; so does the port, on the host's
/// amplifier.
fn new8(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect8(d) {
        return None;
    }
    let r = new(d, base, m, 8)?;
    super::start_amplifier(m, 8).then_some(r)
}

/// The code paths whose time is counted (`Art::spent`), by the original's
/// addresses.
#[derive(Clone, Copy)]
#[repr(usize)]
enum C {
    /// $10AF4: the tick's interrupt, to the voices' effects.
    Tick,
    /// $10C00: a row, to the voices.
    Row,
    /// $10BF8: a pattern delay's tick.
    Delay,
    /// $10C6C: after the row's voices.
    RowEnd,
    /// $10C84: the next position.
    NextPos,
    /// $10CDC: a voice's cell, to its instrument.
    Cell,
    /// One of the cell's G/EC/EE/E6 tests taken.
    CellFx,
    /// $10D8C: no instrument, the voice's.
    NoInstr,
    /// $10DD6: an instrument without a note.
    RepOnly,
    /// $10E00: an instrument with a note.
    NewInstr,
    /// $10E42: the instrument starts, its envelope.
    Start,
    /// $11244: an envelope without attack.
    EnvNone,
    /// $10E52: a synth sound.
    Synth,
    /// $10EB4: its waveform.
    SynthWave,
    /// $10EE4: effect 9's offset.
    SynthOffset,
    /// $10F08: the waveform's loop and steps.
    SynthSetup,
    /// $10F7A: its vibrato.
    SynthVib,
    /// $10FC0: a sample.
    Sample,
    /// $10FF8: its loop and start.
    SampleSet,
    /// $1102E: no loop.
    SampleNoLoop,
    /// $1105E: a loop that does not start at 0 (or oversize).
    SampleLoop,
    /// $11096: effect 9.
    SampleOffset,
    /// $110C4: the instrument's volume.
    Volume,
    /// $110CE: the note.
    Note,
    /// $11152: a tone portamento's target.
    NoteSlide,
    /// $11196: ProTracker's arpeggio.
    ArpPt,
    /// $111BA: the professional arpeggio.
    ArpPro,
    /// One more of its notes.
    ArpByte,
    /// $111FC: its single note.
    ArpNone,
    /// $1136A: a voice's effects.
    Fx,
    /// $11384: the arpeggio's step.
    ArpStep,
    /// One compare of the effects' chains.
    Cmp,
    Slide,
    Tone,
    VibParam,
    /// $11518: the vibrato.
    Vib,
    VolSlide,
    FineVol,
    SetVol,
    Jump,
    Break,
    /// $1167C: an E effect, with its own chain (`Cmp`).
    Extra,
    Speed,
    Tempo,
    /// The other effects.
    Misc,
    /// $116D4: E0's write to the CIA.
    Filter,
    /// $11250: the synth sound's tick.
    SynthTick,
    /// $11274: a synth waveform's counter.
    WaveStep,
    /// $11284: the waveform moves.
    WaveMove,
    /// $112AA: its loop restarts.
    WaveReset,
    /// $11304: the envelope.
    Env,
    /// $1133E: the vibrato's delay.
    VibDelay,
    /// $118D8: a voice's writes.
    Write,
    /// A new note's DMA bit and location.
    WriteNew,
    Per,
    Vol,
    Adr,
    Len,
    DmaOff,
    /// $10BB0: the DMA on's interrupt.
    DmaOn,
    /// $10BBE: the loops' interrupt.
    Loops,
    /// $11962: a voice's loop.
    Loop,
    /// A poke for a channel past 3 when the host dropped them (8 voices, before
    /// the amplifier): kept for the fitted table's layout, no longer counted.
    #[allow(dead_code)]
    Drop,
    Count,
}

/// Colour clocks for each of `C`, fitted by least squares to the Eagleplayer's
/// write logs of 33 AON4 and 12 AON8 modules (rms 6.0 colour clocks over 818163
/// writes): with 4 voices alone a tick's and its voices' costs cannot be told
/// apart. Tags counted together on every path share their sum arbitrarily.
const COST: [i64; C::Count as usize] = [
    105, 325, 40, 325, 67, 121, 5, 65, 42, 14, 164, -9, 115, 171, 0, 171, 115, 49, 156, 5, 34, 64,
    56, 210, 94, 69, 41, 29, 99, 107, 44, 9, 33, 56, 52, 180, 68, 28, 18, 0, 60, 16, 43, 139, 17,
    36, 107, 48, 76, 46, 38, 15, 68, 11, 130, 177, 102, 97, 135, 164, 50, 69, 0,
];

pub struct Art {
    /// The code and data hunks' images.
    code: u32,
    data: u32,
    voices: u32,
    size: u32,
    timer: u16,
    title: String,
    songend: bool,
    /// Colour clocks the original spends since its last write.
    spent: i64,
    /// AON_FIT: the code paths taken since the last write are printed with it.
    fit: Option<[u32; C::Count as usize]>,
    /// The chunk searches' steps so far, for the start's time.
    searched: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine, voices: u32) -> Option<Box<dyn Replayer>> {
    let code = m.alloc(CODE_SIZE);
    let data = m.alloc(DATA_SIZE);
    if code == 0 || data == 0 {
        return None;
    }
    let bus = &mut m.bus;
    bus.w16(data + 0x100, 0x4040);
    for k in 0..16u32 {
        bus.w16(code + FINE_TUNES + 2 * k, (120 * k) as u16);
    }
    for (k, &p) in PERIODS.iter().enumerate() {
        bus.w16(code + PERIOD_TABLE + 2 * k as u32, p);
    }
    for (t, tab) in [VIB_SINE, VIB_RAMP_DOWN, VIB_SQUARE].iter().enumerate() {
        bus.write_bytes(code + VIB_TABLES + 32 * t as u32, tab);
    }
    let mut a = Art {
        code,
        data,
        voices,
        size: d.len() as u32,
        timer: TIMER,
        title: String::new(),
        songend: false,
        spent: 0,
        fit: std::env::var_os("AON_FIT").map(|_| [0; C::Count as usize]),
        searched: 0,
    };
    let start = a.module_info(m, base);
    a.init_sound(m, base);
    // The timer starts when it does after the original's EP_NewModuleInfo and
    // InitSound, whose chunk searches go through the module by words (29 colour
    // clocks a step) and which copy the names and the remark: Paula's DMA runs in
    // slots of each line, so where in a line the ticks come changes the sound.
    // The times were fitted to the reference's start on 33 modules (rms 2 colour
    // clocks; the first writes within 5 of the reference's, 8 voices too).
    let t = start + 29.0 * a.searched as f64;
    Hw { bus: &mut m.bus }.wait(t.round() as u64);
    Some(Box::new(a))
}

impl Replayer for Art {
    fn format(&self) -> &'static str {
        if self.voices == 8 {
            "Art of Noise 8V"
        } else {
            "Art of Noise 4V"
        }
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.spent = 0;
        // paths counted after the last interrupt's last write belong to no write
        if let Some(f) = &mut self.fit {
            *f = [0; C::Count as usize];
        }
        let g = self.code + G;
        match hw.bus.r8(g + G_PHASE) {
            0 => self.play(hw),
            1 => {
                hw.bus.w8(g + G_PHASE, 2);
                let v = hw.bus.r16(g + G_DMA);
                self.dma_mask(hw, v, C::DmaOn);
            }
            _ => {
                hw.bus.w8(g + G_PHASE, 0);
                self.cost(C::Loops);
                for i in 0..self.voices {
                    self.cost(C::Loop);
                    let v = self.voice(i);
                    if hw.bus.r16(v + V_FX) & 0xff0 == 0xed0 {
                        continue;
                    }
                    self.poke_adr(hw, i, hw.bus.r32(v + V_REPEAT));
                    self.poke_len(hw, i, hw.bus.r16(v + V_REPEAT_LENGTH));
                    hw.bus.w8(v + V_FLAG, 0);
                }
            }
        }
    }
}

/// A byte's sign, as the 68k's N flag.
fn neg8(v: u8) -> bool {
    v & 0x80 != 0
}

impl Art {
    fn voice(&self, i: u32) -> u32 {
        self.code + VOICES + VOICE_SIZE * i
    }

    fn cost(&mut self, c: C) {
        self.costs(c, 1);
    }

    fn costs(&mut self, c: C, n: u32) {
        self.spent += COST[c as usize] * n as i64;
        if let Some(f) = &mut self.fit {
            f[c as usize] += n;
        }
    }


    /// The original's chunk search: from the module's start, by words, for the
    /// chunk's name, up to the module's length; the chunk's data, or 0.
    fn chunk(&mut self, m: &Machine, base: u32, id: [u8; 4]) -> u32 {
        let id = u32::from_be_bytes(id);
        let end = base.wrapping_add(self.size);
        let mut a = base;
        loop {
            self.searched += 1;
            if m.bus.r32(a) == id {
                return a + 8;
            }
            a += 2;
            if end < a {
                return 0;
            }
        }
    }

    /// The song name, and the time EP_NewModuleInfo ($106A4) takes without its
    /// chunk searches: it looks for INFO, PLST, INST, WAVE, AUTH, NAME and RMRK,
    /// takes the highest pattern of the position list, copies the author and the
    /// name (100 bytes at most) and the remark (2000), with its tabs expanded.
    fn module_info(&mut self, m: &Machine, base: u32) -> f64 {
        self.chunk(m, base, *b"INFO");
        let plst = self.chunk(m, base, *b"PLST");
        self.chunk(m, base, *b"INST");
        self.chunk(m, base, *b"WAVE");
        let auth = self.chunk(m, base, *b"AUTH");
        let name = self.chunk(m, base, *b"NAME");
        let rmrk = self.chunk(m, base, *b"RMRK");
        let len = |a: u32| if a == 0 { 0 } else { m.bus.r32(a - 4) };
        let text = |a: u32| (len(a) as i16).clamp(0, 100) as u32;
        // the position list's highest pattern: 28 cycles an entry, 12 more for a
        // new highest
        let mut cycles = 0u64;
        let mut top = 0u8;
        for k in 0..len(plst) {
            let b = m.bus.r8(plst.wrapping_add(k));
            cycles += 28;
            if b > top {
                cycles += 12;
                top = b;
            }
        }
        // 8 voices take 16 colour clocks more (measured on 12 modules)
        let eight = if self.voices == 8 { 16.0 } else { 0.0 };
        let mut t = 20046.0 + eight + 0.969 * cycles as f64 / 2.0 + 11.035 * (text(auth) + text(name)) as f64;
        // the remark: 48 cycles a character, 82 a control character, 94 a line
        // feed, 104 + 18 a space for a tab
        let n = len(rmrk).min(2000);
        if n != 0 {
            let (mut cycles, mut col, mut extra) = (0u64, 0u32, 43.2);
            for k in 0..n {
                match m.bus.r8(rmrk + k) {
                    0x20.. => {
                        cycles += 48;
                        col += 1;
                    }
                    9 => {
                        let s = ((col + 8) & !7) - col;
                        cycles += 104 + 18 * s as u64;
                        col += s;
                        extra += 2.0;
                    }
                    10 => {
                        cycles += 94;
                        col = 0;
                        extra += 1.58;
                    }
                    _ => {
                        cycles += 82;
                        col += 1;
                        extra += 0.93;
                    }
                }
            }
            t += 0.979 * cycles as f64 / 2.0 + extra;
        }
        // the song name (EP_NewModuleInfo's)
        if name != 0 {
            let s: Vec<u8> = (0..text(name)).map(|k| m.bus.r8(name + k)).take_while(|&c| c != 0).collect();
            self.title = s.iter().map(|&c| c as char).collect::<String>().trim().to_string();
        }
        t
    }

    /// InitSound.
    fn init_sound(&mut self, m: &mut Machine, base: u32) {
        let g = self.code + G;
        for o in 0..G_SIZE {
            m.bus.w8(g + o, 0);
        }
        m.bus.w8(g + G_SPEED, 6);
        m.bus.w8(g + G_POSITION, 0);
        m.bus.w32(g + G_MODULE, base);
        let info = self.chunk(m, base, *b"INFO");
        m.bus.w32(g + G_INFO, info);
        let arpg = self.chunk(m, base, *b"ARPG");
        m.bus.w32(g + G_ARPG, arpg);
        let plst = self.chunk(m, base, *b"PLST");
        m.bus.w32(g + G_PLST, plst);
        let patt = self.chunk(m, base, *b"PATT");
        m.bus.w32(g + G_PATT, patt);
        let mut inst = self.chunk(m, base, *b"INST");
        for k in 0..61 {
            m.bus.w32(g + G_INSTRUMENTS + 4 * k, inst);
            inst = inst.wrapping_add(0x20);
        }
        let mut wlen = self.chunk(m, base, *b"WLEN");
        let mut wave = self.chunk(m, base, *b"WAVE");
        for k in 0..64 {
            m.bus.w32(g + G_WAVES + 4 * k, wave);
            wave = wave.wrapping_add(m.bus.r32(wlen));
            wlen = wlen.wrapping_add(4);
        }
        for o in 0..8 * VOICE_SIZE {
            m.bus.w8(self.code + VOICES + o, 0);
        }
        for i in 0..8 {
            m.bus.w16(self.voice(i) + V_TRACK_VOL, 0x40);
        }
        m.bus.w8(g + G_TEMPO, 0x7d);
        self.timer = TIMER;
    }

    // The host's ENPP_Poke* and ENPP_DMAMask, as the original calls them: Paula's
    // channels for 4 voices, the amplifier's virtual voices for 8.
    fn wait(&mut self, hw: &mut Hw, c: C) {
        self.cost(c);
        hw.wait(self.spent.max(0) as u64);
        self.spent = 0;
        if let Some(f) = &mut self.fit {
            let s: Vec<String> =
                f.iter().enumerate().filter(|x| *x.1 != 0).map(|(k, n)| format!("{k}:{n}")).collect();
            eprintln!("FIT {} {}", hw.bus.clock, s.join(" "));
            *f = [0; C::Count as usize];
        }
    }

    fn poke_per(&mut self, hw: &mut Hw, i: u32, v: u16) {
        self.wait(hw, C::Per);
        if self.voices == 8 {
            hw.voice_per(i as usize, v);
        } else {
            hw.per(i as usize, v);
        }
    }

    fn poke_vol(&mut self, hw: &mut Hw, i: u32, v: u16) {
        self.wait(hw, C::Vol);
        if self.voices == 8 {
            hw.voice_vol(i as usize, v);
        } else {
            // The reference's volume word carries the byte twice (the low byte copied into the
            // high byte: 0x37 is written as 0x3737). Paula only looks at the low 7 bits, so the
            // sound is the same; the register word is what the write log shows. (The 8-voice
            // variant's amplifier writes have no reference to check this against.)
            hw.vol(i as usize, if v < 0x100 { v | v << 8 } else { v });
        }
    }

    fn poke_adr(&mut self, hw: &mut Hw, i: u32, a: u32) {
        self.wait(hw, C::Adr);
        if self.voices == 8 {
            hw.voice_loc(i as usize, a);
        } else {
            hw.loc(i as usize, a);
        }
    }

    fn poke_len(&mut self, hw: &mut Hw, i: u32, v: u16) {
        self.wait(hw, C::Len);
        if self.voices == 8 {
            hw.voice_len(i as usize, v);
        } else {
            hw.len(i as usize, v);
        }
    }

    /// ENPP_DMAMask with the mask in bits 0-7 and bit 15 for on.
    fn dma_mask(&mut self, hw: &mut Hw, v: u16, c: C) {
        self.wait(hw, c);
        if self.voices == 8 {
            hw.voice_dma((v & 0xff) as u32, v & 0x8000 != 0);
        } else {
            hw.dmacon(v & 0x800f);
        }
    }

    /// The tick proper: the rows, every voice's effects, then their writes.
    fn play(&mut self, hw: &mut Hw) {
        let g = self.code + G;
        self.cost(C::Tick);
        let f = hw.bus.r8(g + G_FRAME).wrapping_add(1);
        hw.bus.w8(g + G_FRAME, f);
        let speed = hw.bus.r8(g + G_SPEED);
        if speed != 0 && speed <= f {
            hw.bus.w8(g + G_FRAME, 0);
            self.new_step(hw);
        }
        let pos = hw.bus.r8(g + G_POSITION) as u32;
        let pat = hw.bus.r8(hw.bus.r32(g + G_PLST).wrapping_add(pos));
        hw.bus.w8(g + G_PATTERN, pat);
        for i in 0..self.voices {
            self.effects(hw, i);
        }
        let mut mask = 0u16;
        for i in 0..self.voices {
            self.write(hw, i, &mut mask);
        }
        self.dma_mask(hw, mask & 0xff, C::DmaOff);
        hw.bus.w16(g + G_DMA, mask | 0x8000);
        hw.bus.w8(g + G_PHASE, 1);
    }

    /// One voice's period, volume and new note.
    fn write(&mut self, hw: &mut Hw, i: u32, mask: &mut u16) {
        let g = self.code + G;
        let v = self.voice(i);
        self.cost(C::Write);
        if hw.bus.r16(v + V_FX) & 0xff0 == 0xed0 {
            return;
        }
        let new = hw.bus.r8(v + V_FLAG) & 2 != 0;
        if new {
            self.cost(C::WriteNew);
            let old = hw.bus.r16(v + V_OLD_LENGTH);
            let len = hw.bus.r16(v + V_LENGTH);
            // noise avoid: a short sample after a short one is not restarted
            let keep = hw.bus.r8(g + G_NOISE_AVOID) != 0 && old <= 0xff && old != 0 && len as i16 <= 0xff;
            if !keep {
                *mask |= 1 << i;
            }
        }
        let mut per = hw.bus.r16(v + V_PERIOD).wrapping_add(hw.bus.r16(v + V_SLIDE));
        if per < 0x67 {
            per = 0x67;
        }
        self.poke_per(hw, i, per);
        let x = (hw.bus.r8(v + V_VOLUME) as u32 * (hw.bus.r8(v + V_ENV_VOL) >> 1) as u32) >> 6;
        let y = x * hw.bus.r16(v + V_TRACK_VOL) as u32;
        self.poke_vol(hw, i, (y as u16) >> 6);
        if new {
            self.poke_adr(hw, i, hw.bus.r32(v + V_WAVE));
            self.poke_len(hw, i, hw.bus.r16(v + V_LENGTH));
        }
    }

    /// The next row ($10BE6): a position jump or pattern break first.
    fn new_step(&mut self, hw: &mut Hw) {
        let g = self.code + G;
        if hw.bus.r8(g + G_BREAK) == 0xff && !self.next_position(hw) {
            return;
        }
        self.row(hw);
    }

    /// $10BF0: the row, or a pattern delay's tick.
    fn row(&mut self, hw: &mut Hw) {
        let g = self.code + G;
        // a pattern break plays the next position's row at once: at most once,
        // but the original could loop for ever on a jump in every row
        for _ in 0..256 {
            let delay = hw.bus.r8(g + G_PATTERN_DELAY);
            if !neg8(delay) && delay != 0 {
                self.cost(C::Delay);
                hw.bus.w8(g + G_PATTERN_DELAY, delay - 1);
                return;
            }
            self.cost(C::Row);
            hw.bus.w8(g + G_PATTERN_DELAY, 0xff);
            let n = self.voices;
            let pat = hw.bus.r8(g + G_PATTERN) as u32;
            let row = hw.bus.r16(g + G_BREAK) as u32;
            let mut cell = hw.bus.r32(g + G_PATT).wrapping_add((pat << 8) * n).wrapping_add(row * n);
            for i in 0..n {
                self.channel(hw, i, cell);
                cell = cell.wrapping_add(4);
            }
            self.cost(C::RowEnd);
            if hw.bus.r8(g + G_LOOP) != 0 {
                hw.bus.w8(g + G_LOOP, 0);
                hw.bus.w8(g + G_ROW, hw.bus.r8(g + G_LOOP_ROW));
                return;
            }
            let r = hw.bus.r8(g + G_ROW).wrapping_add(4);
            hw.bus.w8(g + G_ROW, r);
            if r != 0 {
                return;
            }
            if !self.next_position(hw) {
                return;
            }
        }
    }

    /// $10C84: the next position; true when a break's row is to be played now.
    fn next_position(&mut self, hw: &mut Hw) -> bool {
        let g = self.code + G;
        self.cost(C::NextPos);
        hw.bus.w8(g + G_PATTERN_DELAY, 0);
        hw.bus.w16(g + G_LOOP_ROW, 0);
        let pos = hw.bus.r8(g + G_POSITION).wrapping_add(1);
        hw.bus.w8(g + G_POSITION, pos);
        let info = hw.bus.r32(g + G_INFO);
        if hw.bus.r8(info + 1) <= pos {
            hw.bus.w8(g + G_POSITION, hw.bus.r8(info + 2));
            self.songend = true;
        }
        if hw.bus.r8(g + G_BREAK) == 0 {
            return false;
        }
        hw.bus.w8(g + G_BREAK, 0);
        let pos = hw.bus.r8(g + G_POSITION) as u32;
        let pat = hw.bus.r8(hw.bus.r32(g + G_PLST).wrapping_add(pos));
        hw.bus.w8(g + G_PATTERN, pat);
        true
    }
}

impl Art {
    /// $10CDC: one voice's cell of the row.
    fn channel(&mut self, hw: &mut Hw, i: u32, cell: u32) {
        let g = self.code + G;
        let v = self.voice(i);
        let b = |hw: &Hw, k: u32| hw.bus.r8(cell + k);
        self.cost(C::Cell);
        let fx = b(hw, 2) & 0x3f;
        let arg = b(hw, 3);
        hw.bus.w8(v + V_FX, fx);
        hw.bus.w8(v + V_ARG, arg);
        if fx == 0xd && hw.bus.r8(g + G_ROW) == 0xfc {
            hw.bus.w8(g + G_ROW, 0xf8);
        }
        // the effect and the argument's high nibble, as a word
        let ex = (fx as u16) << 8 | (arg & 0xf0) as u16;
        let lo = arg & 0x0f;
        hw.bus.w8(v + V_STEP, 0);
        let taken = (fx == 0x10) as u32 * 1
            + (fx != 0x10 && ex == 0xec0) as u32 * 2
            + (ex == 0xee0) as u32 * 3
            + (ex == 0xe60) as u32 * 4;
        self.costs(C::CellFx, taken);
        if fx == 0x10 {
            hw.bus.w8(v + V_STEP, lo);
        } else if ex == 0xec0 {
            hw.bus.w8(v + V_STEP, lo);
        } else if ex == 0xee0 {
            if neg8(hw.bus.r8(g + G_PATTERN_DELAY)) {
                hw.bus.w8(g + G_PATTERN_DELAY, lo);
            }
        } else if ex == 0xe60 {
            let c = hw.bus.r8(g + G_LOOP_COUNT);
            if c != 0xf0 && lo != 0 {
                if c == 0 {
                    hw.bus.w8(g + G_LOOP_COUNT, lo);
                }
                let c = hw.bus.r8(g + G_LOOP_COUNT).wrapping_sub(1);
                hw.bus.w8(g + G_LOOP_COUNT, if c == 0 { 0xf0 } else { c });
                hw.bus.w8(g + G_LOOP, 0xff);
            }
        }
        let porta = matches!(fx, 3 | 5 | 0x1b | 0x1c);
        let note = b(hw, 0) & 0x3f;
        let ins = (b(hw, 1) & 0x3f).wrapping_sub(1);
        // whether the instrument's volume is not taken (an old instrument's note)
        let mut old = false;
        let a2;
        'instr: {
            if neg8(ins) {
                // the voice's instrument
                self.cost(C::NoInstr);
                a2 = hw.bus.r32(v + V_INSTRUMENT);
                if a2 == 0 {
                    return;
                }
                if note == 0 {
                    break 'instr;
                }
                old = true;
                if porta {
                    break 'instr;
                }
            } else if note == 0 {
                self.cost(C::RepOnly);
                a2 = hw.bus.r32(g + G_INSTRUMENTS + ((ins << 2) as u32));
                if hw.bus.r8(a2 + I_TYPE) != 0 || a2 == hw.bus.r32(v + V_INSTRUMENT) {
                    self.take_volume(hw, v, a2, old);
                    break 'instr;
                }
                hw.bus.w32(v + V_INSTRUMENT, a2);
                hw.bus.w8(v + V_FLAG, 1);
                self.sample(hw, v, a2, note);
                self.take_volume(hw, v, a2, old);
                break 'instr;
            } else {
                self.cost(C::NewInstr);
                hw.bus.w32(v + V_OFFSET, 0);
                a2 = hw.bus.r32(g + G_INSTRUMENTS + ((ins << 2) as u32));
                if a2 == hw.bus.r32(v + V_INSTRUMENT) && porta {
                    self.take_volume(hw, v, a2, old);
                    break 'instr;
                }
                hw.bus.w32(v + V_INSTRUMENT, a2);
            }
            // $10E42: the instrument starts
            self.cost(C::Start);
            hw.bus.w8(v + V_VIB_CONT, 0);
            self.init_envelope(hw, v, a2);
            if hw.bus.r8(a2 + I_TYPE) != 0 {
                self.synth(hw, v, a2, note);
            } else {
                self.cost(C::Sample);
                hw.bus.w8(v + V_VIB_ON, 0x21);
                hw.bus.w8(v + V_SYNTH, 0);
                let ex = hw.bus.r16(v + V_FX) & 0xfff0;
                let lo = hw.bus.r8(v + V_ARG) & 0xf;
                if ex == 0xed0 {
                    hw.bus.w8(v + V_STEP, lo);
                } else {
                    if ex == 0xe90 {
                        hw.bus.w8(v + V_STEP, lo);
                    }
                    hw.bus.w8(v + V_FLAG, 3);
                }
                self.sample(hw, v, a2, note);
            }
            self.take_volume(hw, v, a2, old);
        }
        self.note(hw, v, a2, cell, note);
    }

    /// $110C4: a new instrument's volume.
    fn take_volume(&mut self, hw: &mut Hw, v: u32, a2: u32, old: bool) {
        if !old {
            self.cost(C::Volume);
            hw.bus.w8(v + V_VOLUME, hw.bus.r8(a2 + I_VOLUME));
        }
    }

    /// $1120E: the envelope, unless H's bit 0 keeps it.
    fn init_envelope(&mut self, hw: &mut Hw, v: u32, a2: u32) {
        if hw.bus.r8(v + V_FX) == 0x11 && hw.bus.r8(v + V_ARG) & 1 != 0 {
            return;
        }
        hw.bus.w8(v + V_ENV_VOL, hw.bus.r8(a2 + I_ENV_START));
        let add = hw.bus.r8(a2 + I_ENV_ADD);
        if add == 0 {
            self.cost(C::EnvNone);
            hw.bus.w8(v + V_ENV_VOL, 0x7f);
            hw.bus.w8(v + V_ENV, 0);
            return;
        }
        hw.bus.w8(v + V_ENV_ADD, add);
        hw.bus.w8(v + V_ENV_SUB, hw.bus.r8(a2 + I_ENV_SUB));
        hw.bus.w8(v + V_ENV_END, hw.bus.r8(a2 + I_ENV_END));
        hw.bus.w8(v + V_ENV, 1);
    }

    /// $10E52: a synth sound starts.
    fn synth(&mut self, hw: &mut Hw, v: u32, a2: u32, note: u8) {
        let g = self.code + G;
        self.cost(C::Synth);
        hw.bus.w8(v + V_SYNTH, 1);
        let ex = hw.bus.r16(v + V_FX) & 0xfff0;
        let arg = hw.bus.r8(v + V_ARG);
        if ex == 0xe90 {
            hw.bus.w8(v + V_STEP, arg & 0xf);
        }
        if note == 0 {
            return;
        }
        hw.bus.w16(v + V_SLIDE, 0);
        let h = if hw.bus.r8(v + V_FX) == 0x11 { arg } else { 0 };
        'wave: {
            if h & 0x10 != 0 {
                break 'wave;
            }
            if ex == 0xed0 {
                hw.bus.w8(v + V_STEP, arg & 0xf);
            } else {
                hw.bus.w8(v + V_FLAG, 3);
            }
            let nr = hw.bus.r8(a2 + I_WAVE) as u32;
            hw.bus.w8(v + V_WAVE_NR, nr as u8);
            let mut w = hw.bus.r32(g + G_WAVES + 4 * nr);
            if w == hw.bus.r32(v + V_WAVE) {
                hw.bus.w8(v + V_FLAG, 0);
                if hw.bus.r8(v + V_WAVE_CONT) != 0 {
                    break 'wave;
                }
            }
            self.cost(C::SynthWave);
            hw.bus.w32(v + V_WAVE, w);
            let len = hw.bus.r8(a2 + I_SYNTH_LENGTH) as u32;
            let stop = hw.bus.r8(v + V_WAVE_STOP) != 0;
            if hw.bus.r8(v + V_FX) == 9 {
                self.cost(C::SynthOffset);
                w = (hw.bus.r8(v + V_ARG) as u32 * len).wrapping_add(w);
                if stop {
                    hw.bus.w32(v + V_WAVE_ACT, w);
                    hw.bus.w32(v + V_REPEAT, w);
                    break 'wave;
                }
            }
            if stop {
                break 'wave;
            }
            self.cost(C::SynthSetup);
            hw.bus.w32(v + V_WAVE_ACT, w);
            hw.bus.w16(v + V_OLD_LENGTH, hw.bus.r16(v + V_LENGTH));
            hw.bus.w16(v + V_LENGTH, len as u16);
            hw.bus.w16(v + V_REPEAT_LENGTH, len as u16);
            let bytes = len * 2;
            hw.bus.w32(v + V_WAVE_ADD, bytes);
            let at = |k: u32| (k * bytes).wrapping_add(w);
            let lp = hw.bus.r8(a2 + I_WAVE_LOOP) as u32;
            hw.bus.w32(v + V_REPEAT, at(lp));
            hw.bus.w32(v + V_WAVE_END, at(hw.bus.r8(a2 + I_WAVE_LENGTH) as u32));
            hw.bus.w32(v + V_WAVE_REP, at(lp));
            let rep_end = hw.bus.r8(a2 + I_WAVE_LOOP_LENGTH).wrapping_add(lp as u8);
            hw.bus.w32(v + V_WAVE_REP_END, at(rep_end as u32));
            let spd = hw.bus.r8(a2 + I_WAVE_SPEED);
            hw.bus.w8(v + V_WAVE_COUNT, spd);
            hw.bus.w8(v + V_WAVE_SPEED, spd);
            hw.bus.w8(v + V_WAVE_CTRL, hw.bus.r8(a2 + I_WAVE_CTRL));
        }
        // $10F6E: the vibrato
        hw.bus.w8(v + V_VIB_ON, 0);
        if hw.bus.r8(a2 + I_VIB_WAVE) == 3 {
            hw.bus.w8(v + V_VIB_ON, 0x21);
            return;
        }
        hw.bus.w16(v + V_VIB_DELAY, hw.bus.r8(a2 + I_VIB_DELAY) as u16);
        self.cost(C::SynthVib);
        let p = hw.bus.r8(a2 + I_VIB_PARAM);
        if p == 0 {
            hw.bus.w16(v + V_VIB_DELAY, 0xfffe);
            return;
        }
        self.vibrato_param(hw, v, p);
        let w = hw.bus.r8(a2 + I_VIB_WAVE).rotate_right(3);
        let a = hw.bus.r8(v + V_VIB_AMPL) & 0x9f | w;
        hw.bus.w8(v + V_VIB_AMPL, a);
        hw.bus.w8(v + V_VIB_CONT, 1);
    }

    /// $10FF8: a sample's start and loop.
    fn sample(&mut self, hw: &mut Hw, v: u32, a2: u32, note: u8) {
        let g = self.code + G;
        self.cost(C::SampleSet);
        if note != 0 {
            hw.bus.w16(v + V_SLIDE, 0);
        }
        let nr = hw.bus.r8(a2 + I_WAVE) as u32;
        hw.bus.w8(v + V_WAVE_NR, nr as u8);
        let w = hw.bus.r32(g + G_WAVES + 4 * nr);
        let len = hw.bus.r32(a2 + I_LENGTH);
        hw.bus.w16(v + V_OLD_LENGTH, hw.bus.r16(v + V_LENGTH));
        hw.bus.w16(v + V_LENGTH, len as u16);
        let oversize = hw.bus.r8(g + G_OVERSIZE) != 0;
        if hw.bus.r32(a2 + I_LOOP_LENGTH) == 0 {
            self.cost(C::SampleNoLoop);
            hw.bus.w32(v + V_REPEAT, self.data);
            hw.bus.w16(v + V_REPEAT_LENGTH, 1);
        } else {
            let lp = hw.bus.r32(a2 + I_LOOP);
            let ll = hw.bus.r16(a2 + I_LOOP_LENGTH + 2);
            if !oversize && lp == 0 {
                hw.bus.w32(v + V_REPEAT, w);
                hw.bus.w16(v + V_REPEAT_LENGTH, ll);
            } else {
                self.cost(C::SampleLoop);
                hw.bus.w32(v + V_REPEAT, lp.wrapping_mul(2).wrapping_add(w));
                hw.bus.w16(v + V_REPEAT_LENGTH, ll);
                if !oversize {
                    hw.bus.w16(v + V_LENGTH, ll.wrapping_add(lp as u16));
                }
            }
        }
        let start = hw.bus.r32(a2 + I_START).wrapping_mul(2);
        let off = hw.bus.r32(v + V_OFFSET);
        let l = hw.bus.r16(v + V_LENGTH).wrapping_sub((off >> 1) as u16);
        hw.bus.w16(v + V_LENGTH, l);
        if hw.bus.r8(v + V_FX) == 9 {
            self.cost(C::SampleOffset);
            let d3 = (hw.bus.r8(v + V_ARG) as u32) << 8;
            let l = l.wrapping_sub((d3 >> 1) as u16);
            hw.bus.w16(v + V_LENGTH, l);
            if l as i16 >= 0 {
                hw.bus.w32(v + V_OFFSET, off.wrapping_add(d3));
            } else {
                hw.bus.w32(v + V_WAVE, hw.bus.r32(v + V_REPEAT));
                hw.bus.w16(v + V_LENGTH, hw.bus.r16(v + V_REPEAT_LENGTH));
                return;
            }
        }
        let off = hw.bus.r32(v + V_OFFSET);
        hw.bus.w32(v + V_WAVE, w.wrapping_add(start).wrapping_add(off));
    }

    /// $110CE: the note, its portamento and arpeggio.
    fn note(&mut self, hw: &mut Hw, v: u32, a2: u32, cell: u32, note: u8) {
        let g = self.code + G;
        self.cost(C::Note);
        let mut d0 = note;
        if d0 == 0 {
            d0 = hw.bus.r8(v + V_NOTE);
            if d0 == 0 || d0 > 60 {
                return;
            }
        } else {
            hw.bus.w8(v + V_SLIDING, 0);
            hw.bus.w8(v + V_NOTE, d0);
            if d0 > 60 {
                return;
            }
        }
        let b1 = hw.bus.r8(cell + 1) & 0xc0;
        let b2 = hw.bus.r8(cell + 2) & 0xc0;
        let mut arp = hw.bus.r32(g + G_ARPG).wrapping_add((b2.rotate_left(6) | b1.rotate_left(4)) as u32);
        let fine = hw.bus.r8(a2 + I_FINE_TUNE) as u32;
        let d2 = hw.bus.r16(self.code + FINE_TUNES + 2 * fine).wrapping_add((d0 as u16 - 1) * 2);
        let fx = hw.bus.r8(v + V_FX);
        if matches!(fx, 0x1b | 0x1c | 5 | 3) && note != 0 {
            self.cost(C::NoteSlide);
            hw.bus.w8(v + V_SLIDING, 1);
            let p = hw.bus.r16(self.code + PERIOD_TABLE + d2 as u32);
            let s = hw.bus.r16(v + V_PERIOD).wrapping_add(hw.bus.r16(v + V_SLIDE)).wrapping_sub(p);
            hw.bus.w16(v + V_SLIDE, s);
        }
        let mut t = v + V_ARP;
        if hw.bus.r16(t + 2) == 0xffff {
            hw.bus.w16(v + V_ARP_POS, 0);
            hw.bus.w8(v + V_ARP_COUNT, 0);
        }
        let mut put = |hw: &mut Hw, w: u16| {
            hw.bus.w16(t, w);
            t += 2;
        };
        let arg = hw.bus.r8(v + V_ARG);
        if fx == 0 && arg != 0 {
            // ProTracker's arpeggio
            self.cost(C::ArpPt);
            put(hw, d2);
            put(hw, d2.wrapping_add((arg >> 4) as u16 * 2));
            put(hw, d2.wrapping_add((arg & 0xf) as u16 * 2));
        } else {
            let b = hw.bus.r8(arp);
            arp = arp.wrapping_add(1);
            let mut n = b >> 4;
            self.cost(if n == 0 { C::ArpNone } else { C::ArpPro });
            self.costs(C::ArpByte, n.saturating_sub(1) as u32);
            if n == 0 {
                hw.bus.w16(v + V_ARP_POS, 0);
                hw.bus.w8(v + V_ARP_COUNT, hw.bus.r8(v + V_ARP_SPEED).wrapping_sub(1));
                put(hw, d2);
            } else {
                put(hw, d2.wrapping_add((b & 0xf) as u16 * 2));
                n -= 1;
                while n != 0 {
                    let b = hw.bus.r8(arp);
                    arp = arp.wrapping_add(1);
                    put(hw, d2.wrapping_add((b >> 4) as u16 * 2));
                    n -= 1;
                    if n == 0 {
                        break;
                    }
                    put(hw, d2.wrapping_add((b & 0xf) as u16 * 2));
                    n -= 1;
                }
            }
        }
        put(hw, 0xffff);
    }
}

impl Art {
    /// $1136A: one voice's arpeggio, effect and synth sound.
    fn effects(&mut self, hw: &mut Hw, i: u32) {
        let g = self.code + G;
        let v = self.voice(i);
        self.cost(C::Fx);
        if hw.bus.r8(v + V_VIB_CONT) == 0 {
            hw.bus.w8(v + V_VIB_ON, 0x21);
        }
        let c = hw.bus.r8(v + V_ARP_COUNT).wrapping_add(1);
        hw.bus.w8(v + V_ARP_COUNT, c);
        if (hw.bus.r8(v + V_ARP_SPEED) as i8) <= c as i8 {
            hw.bus.w8(v + V_ARP_COUNT, 0);
            // an arpeggio of end marks alone would loop for ever
            for _ in 0..2 {
                self.cost(C::ArpStep);
                let pos = hw.bus.r16(v + V_ARP_POS) as i16 as i32;
                let d7 = hw.bus.r16((v + V_ARP).wrapping_add(pos as u32));
                if (d7 as i16) < 0 {
                    hw.bus.w16(v + V_ARP_POS, 0);
                    continue;
                }
                let p = hw.bus.r16(self.code + PERIOD_TABLE + d7 as u32);
                hw.bus.w16(v + V_PERIOD, p);
                let lo = hw.bus.r8(v + V_ARP_POS + 1).wrapping_add(2) & 0xf;
                hw.bus.w8(v + V_ARP_POS + 1, lo);
                break;
            }
        }
        let d1 = hw.bus.r8(v + V_ARG);
        let fx = hw.bus.r8(v + V_FX);
        if fx != 0 {
            let frame = hw.bus.r8(g + G_FRAME);
            const FIRST: [u8; 7] = [1, 2, 3, 4, 5, 6, 0xa];
            const EVERY: [u8; 20] = [
                0xb, 0xc, 0xd, 0xe, 0xf, 0x10, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
                0x1d, 0x1e, 0x21,
            ];
            let chain = |t: &[u8], n: usize| t[..n].iter().position(|&x| x == fx).map_or(n, |k| k + 1) as u32;
            if frame != 0 {
                self.costs(C::Cmp, chain(&FIRST, 7));
            }
            if frame == 0 || !self.slide_effect(hw, v, fx, d1) {
                self.costs(C::Cmp, chain(&EVERY, 20));
                self.effect(hw, v, fx, d1);
            }
        }
        self.synth_tick(hw, v);
    }

    /// Effects 1-6 and A, not on a row's first tick; false for the others.
    fn slide_effect(&mut self, hw: &mut Hw, v: u32, fx: u8, d1: u8) -> bool {
        match fx {
            1 => self.slide(hw, v, -(d1 as i32)),
            2 => self.slide(hw, v, d1 as i32),
            3 => {
                if d1 != 0 {
                    hw.bus.w8(v + V_GLISS, d1);
                }
                self.tone_slide(hw, v);
            }
            4 => {
                hw.bus.w8(v + V_VIB_ON, 1);
                self.vibrato_param(hw, v, d1);
            }
            5 => {
                self.tone_slide(hw, v);
                self.volume_slide(hw, v, d1);
            }
            6 => {
                self.vibrato(hw, v);
                self.volume_slide(hw, v, d1);
            }
            0xa => self.volume_slide(hw, v, d1),
            _ => return false,
        }
        true
    }

    fn slide(&mut self, hw: &mut Hw, v: u32, d: i32) {
        self.cost(C::Slide);
        let s = hw.bus.r16(v + V_SLIDE).wrapping_add(d as u16);
        hw.bus.w16(v + V_SLIDE, s);
    }

    /// $114BE: the tone portamento toward the note.
    fn tone_slide(&mut self, hw: &mut Hw, v: u32) {
        self.cost(C::Tone);
        if hw.bus.r8(v + V_SLIDING) == 0 {
            return;
        }
        let d = hw.bus.r8(v + V_GLISS) as u16;
        let s = hw.bus.r16(v + V_SLIDE);
        if s == 0 {
            return;
        }
        let n = if (s as i16) < 0 {
            let n = s.wrapping_add(d);
            if (n as i16) < 0 { n } else { 0 }
        } else {
            let n = s.wrapping_sub(d);
            if (n as i16) >= 0 { n } else { 0 }
        };
        hw.bus.w16(v + V_SLIDE, n);
    }

    /// $114EE: the vibrato's speed and depth.
    fn vibrato_param(&mut self, hw: &mut Hw, v: u32, d1: u8) {
        self.cost(C::VibParam);
        if d1 == 0 {
            return;
        }
        if d1 >> 4 != 0 {
            hw.bus.w8(v + V_VIB_SPEED, d1 >> 4);
        }
        if d1 & 0xf != 0 {
            let a = hw.bus.r8(v + V_VIB_AMPL) & 0xf0 | d1 & 0xf;
            hw.bus.w8(v + V_VIB_AMPL, a);
        }
    }

    /// $11512: the vibrato, once a tick.
    fn vibrato(&mut self, hw: &mut Hw, v: u32) {
        if hw.bus.r8(v + V_VIB_DONE) != 0 {
            return;
        }
        hw.bus.w8(v + V_VIB_DONE, 1);
        self.cost(C::Vib);
        let a = hw.bus.r8(v + V_VIB_AMPL);
        let tab = match a & 0x60 {
            0 => 0,
            0x20 => 1,
            _ => 2,
        };
        let pos = hw.bus.r8(v + V_VIB_POS) as u32;
        let x = hw.bus.r8(self.code + VIB_TABLES + 32 * tab + pos) as u16;
        let d = (x * (a & 0xf) as u16) >> 7;
        let p = hw.bus.r16(v + V_PERIOD);
        let p = if a & 0x80 != 0 { p.wrapping_sub(d) } else { p.wrapping_add(d) };
        hw.bus.w16(v + V_PERIOD, p);
        let pos = (pos as u8).wrapping_add(hw.bus.r8(v + V_VIB_SPEED));
        hw.bus.w8(v + V_VIB_POS, pos);
        if pos & 0x20 != 0 {
            hw.bus.w8(v + V_VIB_POS, pos & 0x1f);
            hw.bus.w8(v + V_VIB_AMPL, hw.bus.r8(v + V_VIB_AMPL) ^ 0x80);
        }
    }

    /// $115A2: A's volume slide.
    fn volume_slide(&mut self, hw: &mut Hw, v: u32, d1: u8) {
        self.cost(C::VolSlide);
        let (hi, lo) = (d1 >> 4, d1 & 0xf);
        let mut vol = hw.bus.r8(v + V_VOLUME);
        if hi == 0 {
            vol = vol.wrapping_sub(lo);
            if neg8(vol) {
                vol = 0;
            }
        }
        vol = vol.wrapping_add(hi);
        if vol > 0x40 {
            vol = 0x40;
        }
        hw.bus.w8(v + V_VOLUME, vol);
    }

    /// $1174C: EA's fine volume slide up, on a row's first tick.
    fn fine_volume_up(&mut self, hw: &mut Hw, v: u32, d1: u8) {
        self.cost(C::FineVol);
        if hw.bus.r8(self.code + G + G_FRAME) != 0 {
            return;
        }
        let vol = hw.bus.r8(v + V_VOLUME).wrapping_add(d1);
        hw.bus.w8(v + V_VOLUME, if vol as i8 > 0x40 { 0x40 } else { vol });
    }

    /// $11766: EB's fine volume slide down, on a row's first tick.
    fn fine_volume_down(&mut self, hw: &mut Hw, v: u32, d1: u8) {
        self.cost(C::FineVol);
        if hw.bus.r8(self.code + G + G_FRAME) != 0 {
            return;
        }
        let vol = hw.bus.r8(v + V_VOLUME).wrapping_sub(d1);
        hw.bus.w8(v + V_VOLUME, if neg8(vol) { 0 } else { vol });
    }

    /// $1184E: P and S's fine volume slide, up by x or down by y.
    fn fine_volume(&mut self, hw: &mut Hw, v: u32, d1: u8) {
        if d1 >> 4 != 0 {
            self.fine_volume_up(hw, v, d1 >> 4);
        } else {
            self.fine_volume_down(hw, v, d1 & 0xf);
        }
    }

    /// L and M: a volume step from the nibble table, and a portamento.
    fn nibble_slide(&mut self, hw: &mut Hw, v: u32, d1: u8, up: bool) {
        let n = NIBBLES[(d1 >> 4) as usize];
        if n < 0 {
            self.fine_volume_down(hw, v, n.unsigned_abs());
        } else {
            self.fine_volume_up(hw, v, n as u8);
        }
        if hw.bus.r8(self.code + G + G_FRAME) != 0 {
            let lo = (d1 & 0xf) as i32;
            self.slide(hw, v, if up { -lo } else { lo });
        }
    }

    /// The effects of every tick.
    fn effect(&mut self, hw: &mut Hw, v: u32, fx: u8, d1: u8) {
        let g = self.code + G;
        let frame = hw.bus.r8(g + G_FRAME);
        self.cost(match fx {
            0xb => C::Jump,
            0xc | 0x14 | 0x1b => C::SetVol,
            0xd => C::Break,
            0xe => C::Extra,
            0xf => C::Speed,
            _ => C::Misc,
        });
        match fx {
            0xb => {
                hw.bus.w8(g + G_POSITION, d1.wrapping_sub(1));
                hw.bus.w16(g + G_BREAK, 0xff00);
            }
            0xc => hw.bus.w8(v + V_VOLUME, d1),
            0xd => {
                // the row in decimal, times 4, in a byte
                let hi = (d1 & 0xf0) >> 1;
                let r = hi.wrapping_add(hi >> 3).wrapping_add(hi >> 3).wrapping_add(d1 & 0xf);
                let r = r.wrapping_mul(4);
                hw.bus.w16(g + G_BREAK, 0xff00 | r as u16);
            }
            0xe => self.extra(hw, v, d1),
            0xf => {
                if d1 == 0 {
                    hw.bus.w8(g + G_SPEED, 0);
                    hw.bus.w8(g + G_TEMPO, 0x7d);
                    self.timer = TIMER;
                } else if d1 <= 0x20 {
                    hw.bus.w8(g + G_SPEED, d1);
                } else if d1 <= 0xc8 {
                    self.cost(C::Tempo);
                    hw.bus.w8(g + G_TEMPO, d1);
                    self.timer = (0x9052d / d1 as u32) as u16;
                }
            }
            0x10 => {
                let s = hw.bus.r8(v + V_STEP);
                if s == 0 {
                    hw.bus.w8(v + V_VOLUME, (d1 >> 4).wrapping_mul(4).wrapping_add(4));
                } else {
                    hw.bus.w8(v + V_STEP, s - 1);
                }
            }
            0x12 => hw.bus.w8(v + V_WAVE_SPEED, d1 >> 4),
            0x13 => {
                if d1 & 0xf != 0 {
                    hw.bus.w8(v + V_ARP_SPEED, d1 & 0xf);
                }
            }
            0x14 => {
                self.vibrato(hw, v);
                hw.bus.w8(v + V_VOLUME, d1);
            }
            0x15 => self.nibble_slide(hw, v, d1, true),
            0x16 => self.nibble_slide(hw, v, d1, false),
            0x17 => hw.bus.w8(g + G_NOISE_AVOID, d1),
            0x18 => hw.bus.w8(g + G_OVERSIZE, d1),
            0x19 => {
                self.vibrato(hw, v);
                self.fine_volume(hw, v, d1);
            }
            0x1a => {
                self.slide(hw, v, ((d1 >> 4) as i32) << 3);
                self.volume_slide(hw, v, d1 & 0xf);
            }
            0x1b => {
                self.tone_slide(hw, v);
                hw.bus.w8(v + V_VOLUME, d1);
            }
            0x1c => {
                self.tone_slide(hw, v);
                self.fine_volume(hw, v, d1);
            }
            0x1d => hw.bus.w16(v + V_TRACK_VOL, d1 as u16),
            0x1e => {
                hw.bus.w8(v + V_WAVE_CONT, d1 & 0xf);
                hw.bus.w8(v + V_WAVE_STOP, d1 >> 4);
            }
            0x21 => {
                if frame == 0 {
                    hw.bus.w8(self.code + EVENTS, d1);
                }
            }
            _ => {}
        }
    }

    /// $1167C: the E effects.
    fn extra(&mut self, hw: &mut Hw, v: u32, d1: u8) {
        let g = self.code + G;
        let frame = hw.bus.r8(g + G_FRAME);
        let lo = d1 & 0xf;
        const CHAIN: [u8; 10] = [0x10, 0x20, 0x40, 0x50, 0x60, 0x90, 0xa0, 0xb0, 0xc0, 0xd0];
        if d1 & 0xf0 != 0 {
            let k = CHAIN.iter().position(|&x| x == d1 & 0xf0).map_or(10, |k| k + 1);
            self.costs(C::Cmp, k as u32);
        }
        match d1 & 0xf0 {
            0x00 => {
                // the LED filter, with a BSET or BCLR on the CIA's port
                use m68k::AddressBus;
                self.cost(C::Filter);
                let p = hw.bus.read_byte(0xbfe001);
                hw.bus.write_byte(0xbfe001, if lo != 0 { p | 2 } else { p & !2 });
            }
            0x10 => {
                if frame == 0 {
                    self.slide(hw, v, -(lo as i32));
                }
            }
            0x20 => {
                if frame == 0 {
                    self.slide(hw, v, lo as i32);
                }
            }
            0x40 => {
                let a = hw.bus.r8(v + V_VIB_AMPL) & 0x9f | (lo & 3).rotate_right(3);
                hw.bus.w8(v + V_VIB_AMPL, a);
            }
            0x50 => self.set_loop(hw),
            0x60 => {
                if lo == 0 {
                    self.set_loop(hw);
                }
            }
            0x90 | 0xd0 => {
                let s = hw.bus.r8(v + V_STEP);
                if s == 0 {
                    hw.bus.w8(v + V_FLAG, 3);
                    hw.bus.w8(v + V_FX, 0xef);
                } else {
                    hw.bus.w8(v + V_STEP, s - 1);
                }
            }
            0xa0 => self.fine_volume_up(hw, v, lo),
            0xb0 => self.fine_volume_down(hw, v, lo),
            0xc0 => {
                let s = hw.bus.r8(v + V_STEP);
                if s == 0 {
                    hw.bus.w8(v + V_VOLUME, 0);
                } else {
                    hw.bus.w8(v + V_STEP, s - 1);
                }
            }
            _ => {}
        }
    }

    /// $11716: E5's loop point, the row before the next.
    fn set_loop(&mut self, hw: &mut Hw) {
        let g = self.code + G;
        let r = hw.bus.r8(g + G_ROW).wrapping_sub(4);
        if r != hw.bus.r8(g + G_LOOP_ROW) {
            hw.bus.w8(g + G_LOOP_ROW, r);
            hw.bus.w8(g + G_LOOP_COUNT, 0);
        }
    }

    /// $11250: the synth waveform's step, the envelope and the vibrato.
    fn synth_tick(&mut self, hw: &mut Hw, v: u32) {
        self.cost(C::SynthTick);
        hw.bus.w8(v + V_VIB_DONE, 0);
        'env: {
            if hw.bus.r8(v + V_FLAG) != 0 {
                break 'env;
            }
            if hw.bus.r8(v + V_SYNTH) != 0 {
                if hw.bus.r32(v + V_WAVE) == 0 {
                    break 'env;
                }
                if hw.bus.r8(v + V_WAVE_STOP) == 0 {
                    self.wave_step(hw, v);
                }
            }
            // the envelope
            let env = hw.bus.r8(v + V_ENV);
            if env != 0 {
                self.cost(C::Env);
                let mut d0 = hw.bus.r8(v + V_ENV_VOL);
                if env == 1 {
                    d0 = d0.wrapping_add(hw.bus.r8(v + V_ENV_ADD));
                    if neg8(d0) {
                        d0 = 0x7f;
                        hw.bus.w8(v + V_ENV, 2);
                    }
                } else {
                    d0 = d0.wrapping_sub(hw.bus.r8(v + V_ENV_SUB));
                    let end = hw.bus.r8(v + V_ENV_END);
                    if d0 as i8 <= end as i8 {
                        d0 = end;
                        hw.bus.w8(v + V_ENV, 0);
                    }
                }
                hw.bus.w8(v + V_ENV_VOL, d0);
            }
            // the vibrato's delay
            if hw.bus.r8(v + V_VIB_ON) != 0x21 {
                self.cost(C::VibDelay);
                let d = hw.bus.r16(v + V_VIB_DELAY);
                if d == 0xffff {
                    hw.bus.w8(v + V_VIB_ON, 1);
                } else {
                    hw.bus.w16(v + V_VIB_DELAY, d.wrapping_sub(1));
                }
            }
        }
        if hw.bus.r8(v + V_VIB_ON) == 1 {
            self.vibrato(hw, v);
        }
    }

    /// $11274: the synth waveform moves on every wave speed's tick.
    fn wave_step(&mut self, hw: &mut Hw, v: u32) {
        self.cost(C::WaveStep);
        let c = hw.bus.r8(v + V_WAVE_COUNT).wrapping_add(1);
        hw.bus.w8(v + V_WAVE_COUNT, c);
        if hw.bus.r8(v + V_WAVE_SPEED) as i8 > c as i8 {
            return;
        }
        hw.bus.w8(v + V_WAVE_COUNT, 0);
        self.cost(C::WaveMove);
        let r32 = |hw: &Hw, o: u32| hw.bus.r32(v + o) as i32;
        let add = r32(hw, V_WAVE_ADD);
        let act = r32(hw, V_WAVE_ACT).wrapping_add(add);
        hw.bus.w32(v + V_WAVE_ACT, act as u32);
        let past = if add >= 0 { r32(hw, V_WAVE_END) <= act } else { r32(hw, V_WAVE_REP) > act };
        if past {
            self.cost(C::WaveReset);
            match hw.bus.r8(v + V_WAVE_CTRL) {
                0 => {
                    hw.bus.w32(v + V_WAVE_ACT, r32(hw, V_WAVE_REP) as u32);
                    hw.bus.w32(v + V_WAVE_END, r32(hw, V_WAVE_REP_END) as u32);
                }
                1 => {
                    // backwards, from the loop's end
                    hw.bus.w32(v + V_WAVE_ACT, r32(hw, V_WAVE_REP_END) as u32);
                    let mut d0 = r32(hw, V_WAVE_ADD);
                    if d0 >= 0 {
                        d0 = d0.wrapping_neg();
                        if hw.bus.r8(v + V_WAVE_STOP) == 0 {
                            let a = r32(hw, V_WAVE_ACT).wrapping_add(d0);
                            hw.bus.w32(v + V_WAVE_ACT, a as u32);
                            hw.bus.w32(v + V_WAVE_ADD, d0 as u32);
                        }
                    } else {
                        let a = r32(hw, V_WAVE_ACT).wrapping_add(d0);
                        hw.bus.w32(v + V_WAVE_ACT, a as u32);
                        hw.bus.w32(v + V_WAVE_ADD, d0 as u32);
                    }
                }
                _ => {
                    // ping-pong
                    hw.bus.w32(v + V_WAVE_END, r32(hw, V_WAVE_REP_END) as u32);
                    let d0 = r32(hw, V_WAVE_ADD);
                    let a = r32(hw, V_WAVE_ACT).wrapping_sub(d0);
                    hw.bus.w32(v + V_WAVE_ACT, a as u32);
                    hw.bus.w32(v + V_WAVE_ADD, d0.wrapping_neg() as u32);
                }
            }
        }
        hw.bus.w8(v + V_FLAG, 1);
        hw.bus.w32(v + V_REPEAT, hw.bus.r32(v + V_WAVE_ACT));
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;

    fn chunk(d: &mut Vec<u8>, id: &[u8], body: &[u8]) {
        d.extend(id);
        d.extend((body.len() as u32).to_be_bytes());
        d.extend(body);
    }

    /// One position of pattern 0, whose first row has note 25 of instrument
    /// 1 on the given voices: a sample of 16 words without a loop, volume 64,
    /// no envelope.
    fn module(voices: usize, notes: &[usize]) -> Vec<u8> {
        let mut d = format!("AON{voices}artofnoise by bastian spiegel (twice/lego)\0\0").into_bytes();
        chunk(&mut d, b"NAME", b"test");
        // the version, 1 position, restart at 0
        chunk(&mut d, b"INFO", &[0, 1, 0, 0]);
        chunk(&mut d, b"ARPG", &[0; 64]);
        chunk(&mut d, b"PLST", &[0, 0]);
        let mut patt = vec![0u8; 64 * voices * 4];
        for &i in notes {
            patt[4 * i..4 * i + 4].copy_from_slice(&[25, 1, 0, 0]);
        }
        chunk(&mut d, b"PATT", &patt);
        let mut inst = [0u8; 32];
        inst[1] = 64;
        inst[8..12].copy_from_slice(&16u32.to_be_bytes());
        chunk(&mut d, b"INST", &inst);
        let mut wlen = vec![0u8; 64 * 4];
        wlen[..4].copy_from_slice(&32u32.to_be_bytes());
        chunk(&mut d, b"WLEN", &wlen);
        let wave: Vec<u8> = (0..32).map(|i| (i * 8) as u8).collect();
        chunk(&mut d, b"WAVE", &wave);
        d
    }

    /// Plays the module for a second; its format, the writes, the output.
    fn play(name: &str, d: Vec<u8>) -> (String, Vec<(u32, u16)>, Vec<f32>) {
        let dir = std::env::current_exe().unwrap().parent().unwrap().join("artofnoise-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, d).unwrap();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&path, std::fs::read(&path).unwrap()).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 48000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        (format, log, out)
    }

    #[test]
    fn plays_a_sample_on_paula() {
        let (format, log, out) = play("test.aon", module(4, &[0]));
        assert_eq!(format, "Art of Noise 4V");
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // every tick turns the new notes' DMA off and then on, none at first;
        // the note's voice 0 once
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..2], &[0x0000, 0x8000]);
        let k = dma.iter().position(|&v| v == 0x0001).expect("no DMA off for the note");
        assert_eq!(dma[k + 1], 0x8001);
        assert_eq!(dma.iter().filter(|&&v| v & 0x000f != 0).count(), 2);
        // note 25 of fine-tune 0
        assert_eq!(*reg(0xDFF0A6).last().unwrap(), super::PERIODS[24]);
        // (64 * (127 / 2)) >> 6, at the track volume 64
        // (the reference writes the byte twice: 0x3f3f)
        assert_eq!(*reg(0xDFF0A8).last().unwrap(), 0x3f3f);
        // the sample's 16 words, then the empty loop's one
        let len = reg(0xDFF0A4);
        let k = len.iter().position(|&v| v == 16).expect("no sample length");
        assert_eq!(len[k + 1], 1);
        // the other voices get their period and volume, and stay silent
        assert!(!reg(0xDFF0B6).is_empty());
        assert!(reg(0xDFF0B8).iter().all(|&v| v == 0));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }

    #[test]
    fn eight_voices_poke_the_amplifier() {
        // the note on voices 1 and 5: all 8 voices go to the amplifier's
        // virtual voices, none to Paula's registers
        let (format, log, _) = play("test8.aon", module(8, &[1, 5]));
        assert_eq!(format, "Art of Noise 8V");
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        let v = |voice: u32, r: u32| reg(crate::amplifier::LOG_BASE | voice << 4 | r);
        assert!(log.iter().all(|w| !(0xDFF096..0xDFF0E0).contains(&w.0)));
        for voice in [1, 5] {
            assert_eq!(v(voice, 0xA), [0, 0x8000], "voice {voice}");
            assert_eq!(*v(voice, 6).last().unwrap(), super::PERIODS[24]);
            assert_eq!(*v(voice, 8).last().unwrap(), 63);
        }
        for voice in [0, 2, 3, 4, 6, 7] {
            assert!(v(voice, 0xA).is_empty(), "voice {voice}");
            assert!(v(voice, 8).iter().all(|&x| x == 0));
            // 8 voices of cells a row: a voice with no note has the first
            // period of the table, as in the original
            assert!(v(voice, 6).iter().all(|&x| x == super::PERIODS[0]));
        }
    }
}
