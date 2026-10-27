//! Sonic Arranger, ported from NostalgicPlayer's Sonic Arranger player and its
//! converter for the "final" modules (Copyright (c) 2023 Thomas Neumann, MIT
//! licence: see LICENSE-NostalgicPlayer).
//!
//! It plays the three layouts the original player (the SonicArranger
//! Eagleplayer V4.11, "adapted by Eagleeye of DEFECT") takes: the editor's
//! "SOARV1.0" files, and the "final" ones, eight offsets to the song's parts,
//! alone or after a replayer that starts with six JMPs. Like the original it
//! plays the song from the module in guest memory, at the addresses its
//! InitPlayer computes, and keeps each voice's state in the layout of its BSS
//! hunk: the voice's 128-byte synth waveform, then its variables. The lh.library
//! packed modules ("@OARV1.0"), which NostalgicPlayer decrunches, the original
//! does not take.
//!
//! The original is an older player than the one NostalgicPlayer follows, and
//! where they differ this follows the original:
//! * a note turns its voice's DMA off and writes a length of 0 first; a sample's
//!   loop goes to Paula on the next tick, the player's empty word for a one-shot
//!   sample and nothing for a sample that loops whole; a synth sound is played
//!   from the voice's buffer without a loop write; there is no check of the
//!   sample length or of the instrument number;
//! * a silent voice gets volume 0 every tick, and volumes are written masked to
//!   7 bits; the master volume starts at 63 and effect 6 takes the low 6 bits
//!   (64 is 0); an envelope's volume is (table * master >> 6) * volume >> 6,
//!   with no clamp, and its end silences the voice when that product's low byte
//!   is 0;
//! * the portamento is the original's: from the previous note's period to the
//!   new one in as many ticks as the instrument (or effect 7) says, not a slide
//!   by that much per tick;
//! * effect 0's arpeggio takes the low nibble on the first tick of three and the
//!   high one on the second; an instrument's arpeggio runs to position
//!   length + repeat - 1 (NostalgicPlayer: one more), without the cap at 13;
//! * the period is clamped to 113 as a signed word; effect 9 does not take 0;
//!   effect F takes 1-16 as a signed byte; effect E turns the filter off for 0;
//! * a new note resets the sustain counter; a note-off ($7F) clears only the
//!   high word of the voice's instrument pointer, so the voice plays on with
//!   whatever lies at the low word's address, as the original does;
//! * the synth effects work on the buffer bytes as the original's code does,
//!   longword steps included (its wave alias carries between neighbouring
//!   bytes), their position runs from the start to the stop position inclusive,
//!   metamorph stops once its target is reached (NostalgicPlayer tests the wrong
//!   flag), the frequency drum restarts from the whole fine tuning word, the
//!   second low-pass filter reads its thresholds on past the waveform, and the
//!   noise comes from the beam position, as the original's;
//! * the song end is signalled on entering the last position (or on a jump
//!   back); the tempo gives the timer value tempo * 14187 / 50, as the
//!   original's InitSound computes it;
//! * the writes are spaced as the original's: each part of its code costs the
//!   time fitted to the gaps of its write logs (see `t`), so that a note's DMA
//!   off stops the voice for as long before the tick's DMA on. The noise
//!   generators read the beam where the port's timing puts it, which is not
//!   the original's to the colour clock, so their noise differs.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Sonic Arranger", detect, new };

/// A voice's variables in the original's BSS hunk (after the 128-byte waveform).
const NOTE: u32 = 0x80;
const PREV_NOTE: u32 = 0x82;
const SLIDE: u32 = 0x84;
const SLIDE_SPEED: u32 = 0x86;
const INSTR_NUM: u32 = 0x88;
const INSTR: u32 = 0x8a;
const VOL: u32 = 0x8e;
const VOL_SLIDE: u32 = 0x90;
const VIB_POS: u32 = 0x92;
const VIB_DELAY: u32 = 0x94;
const VIB_SPEED: u32 = 0x96;
const VIB_LEVEL: u32 = 0x98;
const PORTA_CNT: u32 = 0x9a;
const PORTA_LEN: u32 = 0x9c;
const ADSR_POS: u32 = 0x9e;
const SUSTAIN_CNT: u32 = 0xa0;
const ADSR_DELAY: u32 = 0xa2;
const AMF_POS: u32 = 0xa4;
const AMF_DELAY: u32 = 0xa6;
const FX_POS: u32 = 0xa8;
const FX_WAVE_POS: u32 = 0xaa;
const FX_DELAY: u32 = 0xac;
const ARP_POS: u32 = 0xae;
/// The volume times 4, for the original's scopes.
const SCOPE_VOL: u32 = 0xb2;
/// A byte: bit 0 silences the voice, bits 1-3 are the synth effects' flags.
const FLAGS: u32 = 0xb5;
const LEN: u32 = 0xb6;
const TYPE: u32 = 0xb8;
const SAMPLE: u32 = 0xba;
const VOICE_SIZE: u32 = 0xcc;
/// After the four voices: the empty word a one-shot sample loops on.
const EMPTY: u32 = 4 * VOICE_SIZE;
const BSS_SIZE: u32 = 0x338;

/// An instrument's words (152 bytes an instrument).
const I_WAVE: u32 = 0x02;
const I_LEN: u32 = 0x04;
const I_REPEAT: u32 = 0x06;
const I_VOL: u32 = 0x10;
const I_FINE: u32 = 0x12;
const I_PORTA: u32 = 0x14;
const I_VIB_DELAY: u32 = 0x16;
const I_VIB_SPEED: u32 = 0x18;
const I_VIB_LEVEL: u32 = 0x1a;
const I_AMF_NUM: u32 = 0x1c;
const I_AMF_DELAY: u32 = 0x1e;
const I_AMF_LEN: u32 = 0x20;
const I_AMF_REPEAT: u32 = 0x22;
const I_ADSR_NUM: u32 = 0x24;
const I_ADSR_DELAY: u32 = 0x26;
const I_ADSR_LEN: u32 = 0x28;
const I_ADSR_REPEAT: u32 = 0x2a;
const I_SUSTAIN_POINT: u32 = 0x2c;
const I_SUSTAIN_DELAY: u32 = 0x2e;
const I_ARG1: u32 = 0x40;
const I_EFFECT: u32 = 0x42;
const I_ARG2: u32 = 0x44;
const I_ARG3: u32 = 0x46;
const I_FX_DELAY: u32 = 0x48;
const I_ARPEGGIOS: u32 = 0x4a;

/// The original's InitPlayer keeps the default timer to scale it by the tempo.
const TIMER: u32 = crate::deli::DEFAULT_TIMER as u32;

/// Colour clocks the original takes between its writes: what each part of its
/// code costs, fitted (least squares) to the gaps between the writes of its
/// write logs on 80 modules. The code adds the parts it runs to `pend`; a
/// write waits them, plus its own kind's.
mod t {
    /// To each kind of write, from the code before it.
    pub const PER: i64 = 401;
    pub const VOL: i64 = 227;
    pub const LEN: i64 = 105;
    pub const LOC: i64 = 99;
    pub const OFF: i64 = 183;
    pub const ON: i64 = 168;
    /// From the tick to the first voice (not measured: the logs have no tick).
    pub const TICK: i64 = 200;
    /// A row, a new position (not measured either).
    pub const ROW: i64 = 200;
    pub const POSITION: i64 = 100;
    /// A voice's row: without a note (and with an instrument), quiet, a note,
    /// its instrument (or none), a sample, a synth sound and its copy for each
    /// 16 bytes.
    pub const EMPTY: i64 = 131;
    pub const EMPTY_INSTRUMENT: i64 = 279;
    pub const QUIET: i64 = 114;
    pub const NOTE: i64 = 183;
    pub const INSTRUMENT: i64 = 352;
    pub const NO_INSTRUMENT: i64 = 235;
    pub const SAMPLE: i64 = -27;
    pub const SYNTH_SOUND: i64 = 5;
    pub const COPY16: i64 = 45;
    /// The row's effect, by effect (the ones the logs do not have at about
    /// their neighbours').
    pub const ROW_EFFECT: [i64; 16] = [84, 89, 100, 100, 145, 100, 97, 97, 100, 108, 90, 100, 110, 98, 151, 110];
    /// A sample's loop, on the next tick.
    pub const LOOP: i64 = 97;
    /// A voice's tick: silent; arpeggio (the instrument's, the effect's);
    /// portamento; vibrato (its delay, it, its level); AMF and its step; slide.
    pub const MUTED: i64 = 30;
    pub const ARPEGGIO: i64 = 31;
    pub const EFFECT_ARPEGGIO: i64 = 95;
    pub const PORTAMENTO: i64 = 141;
    pub const VIBRATO_DELAY: i64 = 27;
    pub const VIBRATO: i64 = 80;
    pub const VIBRATO_LEVEL: i64 = 80;
    pub const AMF: i64 = 62;
    pub const AMF_STEP: i64 = 22;
    pub const SLIDE: i64 = 15;
    /// The envelope (or none), its sustain, its delay and its step.
    pub const ADSR: i64 = 84;
    pub const NO_ADSR: i64 = -1;
    pub const SUSTAIN: i64 = 53;
    pub const ADSR_ADVANCE: i64 = 36;
    pub const ADSR_STEP: i64 = 37;
    /// The synth effect's delay; each effect (0 for none) and for each byte it
    /// changes. Effects 5, 8, 9, 15 and 17 are not in the logs.
    pub const SYNTH: i64 = 47;
    pub const EFFECT: [i64; 18] = [22, 104, 75, 140, 117, 140, 158, 203, 243, 140, 134, 199, 110, 5, 3, 140, 19, 120];
    pub const EFFECT_BYTE: [i64; 18] = [0, 0, 0, 20, 15, 20, 0, 19, 19, 25, 0, 40, 0, 74, 81, 25, 19, 0];
    /// The free negator: its copy and its negation, for each longword, and
    /// an odd byte.
    pub const NEGATOR: i64 = 192;
    pub const NEGATOR_COPY: i64 = 24;
    pub const NEGATOR_NEGATE: i64 = 74;
    pub const NEGATOR_ODD: i64 = 25;
}

pub struct SonicArranger {
    subsong_table: u32,
    subsong_count: u16,
    positions: u32,
    tracks: u32,
    instruments: u32,
    waveforms: u32,
    adsr: u32,
    amf: u32,
    /// The samples' addresses (the original's table has room for 64).
    samples: [u32; 64],
    /// The original's BSS hunk: four voices and the empty word.
    bss: u32,
    sub: u16,
    // the original's variables
    master: u16,
    dma_mask: u16,
    /// A sample's loop goes to Paula on the next tick.
    loop_pending: [bool; 4],
    speed: u16,
    rows: u16,
    last: u16,
    restart: u16,
    timer: u16,
    counter: u16,
    row: u16,
    pos: u16,
    playing: bool,
    /// The position's four (track start, sound transpose, note transpose) and the
    /// voices' current rows.
    position: [u32; 4],
    row_data: [u32; 4],
    songend: bool,
    /// Colour clocks the original's code has taken since its last write.
    pend: i64,
}

/// The module data's start (where the eight offsets are) and its layout: 0 for
/// "SOARV1.0", 1 for the offsets alone, 2 for a replayer in front. As the
/// original's check, with NostalgicPlayer's test of the replayer's init routine.
fn layout(d: &[u8]) -> Option<(usize, u8)> {
    if d.len() >= 8 && &d[..8] == b"SOARV1.0" {
        return Some((0, 0));
    }
    let l = |o: usize| be32(d, o);
    let offsets = || -> bool {
        if d.len() < 32 || be16(d, 0) != 0 || be16(d, 4) != 0 || [8, 12, 16, 20].iter().any(|&o| d[o] != 0) {
            return false;
        }
        let (l0, l1, l2, l3, l4, l5, l6) = (l(0), l(4), l(8), l(12), l(16), l(20), l(24));
        if l0 == 0 || l0 & !7 != l0 || l1 == 0 {
            return false;
        }
        let n = l1.wrapping_sub(l0);
        // the low byte may have only bits 2 and 3; then divu #12, remainder 0
        // and at most 32 subsongs
        if n & 0xf3 != 0 || n % 12 != 0 || n / 12 > 32 {
            return false;
        }
        if l1 < l0 || l2 == 0 || l1 > l2 || (l2 - l1) & 0x0f != 0 {
            return false;
        }
        l3 != 0 && l3 >= l2 && l4 != 0 && l3 <= l4 && l5 != 0 && l4 <= l5 && l6 != 0 && l5 < l6
    };
    if offsets() {
        return Some((0, 1));
    }
    if d.len() < 0x1630 || (0..6).any(|i| be16(d, 4 * i) != 0x4efa) {
        return None;
    }
    let init = (2 + be16(d, 2) as i16 as isize) as usize;
    if d.get(init..init + 6)? != [0x48, 0xe7, 0xff, 0xfe, 0x41, 0xfa] {
        return None;
    }
    let md = (init as isize + 6 + be16(d, init + 6) as i16 as isize) as usize;
    (md + 32 <= d.len()).then_some((md, 2))
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (md, kind) = layout(d)?;
    let bss = m.alloc(BSS_SIZE);
    if bss == 0 {
        return None;
    }
    let bus = &m.bus;
    let a0 = base + md as u32;
    let mut p = SonicArranger {
        subsong_table: 0,
        subsong_count: 0,
        positions: 0,
        tracks: 0,
        instruments: 0,
        waveforms: 0,
        adsr: 0,
        amf: 0,
        samples: [0; 64],
        bss,
        sub: 0,
        master: 0,
        dma_mask: 0,
        loop_pending: [false; 4],
        speed: 0,
        rows: 0,
        last: 0,
        restart: 0,
        timer: TIMER as u16,
        counter: 0,
        row: 0,
        pos: 0,
        playing: false,
        position: [0; 4],
        row_data: [0; 4],
        songend: false,
        pend: 0,
    };
    // as the original's InitPlayer: the parts' addresses, and the samples'
    if kind == 0 {
        // the chunks one after the other, each with its count after the mark
        let mut a = a0 + 8;
        let count = bus.r32(a + 4);
        p.subsong_count = count as u16;
        p.subsong_table = a + 8;
        a = p.subsong_table.wrapping_add(count.wrapping_mul(12));
        p.positions = a + 8;
        a = p.positions.wrapping_add(bus.r32(a + 4).wrapping_mul(16));
        p.tracks = a + 8;
        a = p.tracks.wrapping_add(bus.r32(a + 4).wrapping_mul(4));
        p.instruments = a + 8;
        a = p.instruments.wrapping_add(bus.r32(a + 4).wrapping_mul(152));
        // SD8B: the count, its one-shot and repeat lengths, names, sample lengths
        let n = bus.r32(a + 4);
        a += 8;
        if n as u16 != 0 {
            a = a.wrapping_add(n * 8 + n * 30);
            let lengths = a;
            a = a.wrapping_add(n * 4);
            for i in 0..n {
                if (i as usize) < p.samples.len() {
                    p.samples[i as usize] = a & !1;
                }
                a = a.wrapping_add(bus.r32(lengths + 4 * i));
            }
        }
        let n = bus.r32(a + 4);
        p.waveforms = a + 8;
        a = p.waveforms.wrapping_add(n.wrapping_mul(128));
        let n = bus.r32(a + 4);
        p.adsr = a + 8;
        a = p.adsr.wrapping_add(n.wrapping_mul(128));
        p.amf = a + 8;
    } else {
        let o = |i: u32| a0.wrapping_add(bus.r32(a0 + 4 * i));
        let (l0, l1) = (bus.r32(a0), bus.r32(a0 + 4));
        // divu.w #12: the quotient's word
        p.subsong_count = (l1.wrapping_sub(l0) / 12) as u16;
        p.subsong_table = o(0);
        p.positions = o(1);
        p.tracks = o(2);
        p.instruments = o(3);
        p.waveforms = o(4);
        p.adsr = o(5);
        p.amf = o(6);
        let mut a = o(7);
        let n = bus.r32(a);
        a += 4;
        if n != 0 {
            let lengths = a;
            a = a.wrapping_add(n << 2);
            for i in 0..n {
                if (i as usize) < p.samples.len() {
                    p.samples[i as usize] = a;
                }
                a = a.wrapping_add(bus.r32(lengths + 4 * i));
            }
        }
    }
    // the original's InitPlayer ends in its InitSound, which DeliTracker then
    // calls again
    let mut hw = Hw { bus: &mut m.bus };
    p.init_sound(0, &mut hw);
    p.init_sound(0, &mut hw);
    Some(Box::new(p))
}

impl Replayer for SonicArranger {
    fn format(&self) -> &'static str {
        "Sonic Arranger"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        // as DeliTracker takes dtg_Timer (0 is its default)
        let t = if self.timer == 0 { TIMER as u64 } else { self.timer as u64 };
        (t + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsong_count.wrapping_sub(1), self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // DeliTracker calls EndSound, then InitSound
        self.end_sound(hw);
        self.init_sound(n, hw);
    }

    /// The original's interrupt: the loops of last tick's samples, the row when
    /// its speed counter is up, the voices' effects, and the DMA of the notes.
    fn tick(&mut self, hw: &mut Hw) {
        self.pend = t::TICK;
        if self.loop_pending.iter().any(|&f| f) {
            for v in 0..4 {
                if self.loop_pending[v] {
                    self.set_loop(v, hw);
                }
            }
            self.loop_pending = [false; 4];
        }
        self.counter = self.counter.wrapping_add(1);
        if self.counter as i16 >= self.speed as i16 {
            self.counter = 0;
            if self.playing {
                self.next_row(hw);
            }
        }
        for v in 0..4 {
            self.update_voice(v, hw);
        }
        if self.dma_mask & 0xff != 0 {
            self.write_wait(hw, t::ON);
            hw.dmacon(0x8000 | (self.dma_mask & 0xf));
            // the host's ENPP_DMAMask waits for the DMA, as dtg_WaitAudioDMA does:
            // to 10 lines past the current one
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
            self.dma_mask = 0;
        }
    }
}

/// Lets time pass, and Paula play up to then from the waveforms as they are.
fn wait(hw: &mut Hw, clocks: u64) {
    hw.wait(clocks);
    let b = &mut *hw.bus;
    b.paula.sync(b.clock, &b.ram);
}

/// The original's `divs.w`: the quotient's word, or the dividend's if it
/// overflows.
fn divs(n: i32, d: i16) -> u16 {
    let q = n / d as i32;
    if q < i16::MIN as i32 || q > i16::MAX as i32 { n as u16 } else { q as u16 }
}

/// A waveform's offset from a word: `asl.w #7` as a signed word index.
fn wave_w(n: u16) -> u32 {
    (n << 7) as i16 as i32 as u32
}

impl SonicArranger {
    fn va(&self, v: usize) -> u32 {
        self.bss + VOICE_SIZE * v as u32
    }

    /// Lets the time of the code run since the last write pass.
    fn spend(&mut self, hw: &mut Hw) {
        wait(hw, self.pend.max(0) as u64);
        self.pend = 0;
    }

    /// Before a write: that time, and the write's own.
    fn write_wait(&mut self, hw: &mut Hw, own: i64) {
        self.pend += own;
        self.spend(hw);
    }

    fn signal_songend(&mut self) {
        self.songend = true;
    }

    fn end_sound(&mut self, hw: &mut Hw) {
        self.playing = false;
        self.loop_pending = [false; 4];
        self.silence_all(hw);
        for v in 0..4 {
            self.write_wait(hw, t::VOL);
            hw.vol(v, 0);
        }
        self.write_wait(hw, t::OFF);
        hw.dmacon(0x000f);
    }

    /// All voices silent and their DMA off.
    fn silence_all(&mut self, hw: &mut Hw) {
        for v in 0..4 {
            let a = self.va(v) + FLAGS;
            hw.bus.w8(a, hw.bus.r8(a) | 1);
        }
        self.write_wait(hw, t::OFF);
        hw.dmacon(0x000f);
    }

    /// The original's InitSound for subsong `n` (the first valid one from it).
    fn init_sound(&mut self, mut n: u16, hw: &mut Hw) {
        self.master = 0x3f;
        loop {
            let e = self.subsong_table.wrapping_add(n as u32 * 12);
            if hw.bus.r16(e + 6) != 0xffff && hw.bus.r16(e + 8) != 0xffff {
                break;
            }
            n = n.wrapping_add(1);
            if (n.wrapping_sub(self.subsong_count) as i16) >= 0 {
                return;
            }
        }
        self.sub = n;
        let e = self.subsong_table.wrapping_add(n as u32 * 12);
        let w = |i: u32| hw.bus.r16(e + 2 * i);
        let (speed, rows, first, last, restart, tempo) = (w(0), w(1), w(2), w(3), w(4), w(5));
        for i in 0..BSS_SIZE {
            hw.bus.w8(self.bss + i, 0);
        }
        // timer = tempo * the default timer / 50, as mulu and divu compute it
        let prod = if tempo == 0 { 50 } else { tempo as u32 } * TIMER;
        self.timer = if prod / 50 > 0xffff { prod as u16 } else { (prod / 50) as u16 };
        self.speed = speed;
        self.rows = rows;
        self.last = last;
        self.restart = restart;
        self.counter = speed;
        self.row = rows;
        self.pos = first.wrapping_sub(1);
        self.playing = true;
        self.silence_all(hw);
    }

    /// The loop of the sample a note started on the last tick.
    fn set_loop(&mut self, v: usize, hw: &mut Hw) {
        let va = self.va(v);
        let b = &*hw.bus;
        let inst = b.r32(va + INSTR);
        let repeat = b.r16(inst + I_REPEAT);
        self.pend += t::LOOP;
        if repeat == 0 {
            return;
        }
        let sample = b.r32(va + SAMPLE);
        if repeat == 1 || sample == 0 {
            self.write_wait(hw, t::LOC);
            hw.loc(v, self.bss + EMPTY);
            self.write_wait(hw, t::LEN);
            hw.len(v, 1);
        } else {
            // movem.w: the one-shot length sign-extended
            let one_shot = hw.bus.r16(inst + I_LEN) as i16 as i32 as u32;
            self.write_wait(hw, t::LOC);
            hw.loc(v, sample.wrapping_add(one_shot << 1));
            self.write_wait(hw, t::LEN);
            hw.len(v, repeat);
        }
    }

    fn next_row(&mut self, hw: &mut Hw) {
        self.pend += t::ROW;
        self.row = self.row.wrapping_add(1);
        if !(self.rows as i16 > self.row as i16) {
            self.row = 0;
            self.pos = self.pos.wrapping_add(1);
            self.pend += t::POSITION;
            if self.last as i16 >= self.pos as i16 {
                if self.pos.wrapping_add(1) > self.last {
                    self.signal_songend();
                }
            } else {
                self.pos = self.restart;
            }
            let e = self.positions.wrapping_add((self.pos as u32) << 4);
            for v in 0..4 {
                self.position[v] = hw.bus.r32(e + 4 * v as u32);
            }
        }
        let rows = self.tracks.wrapping_add(((self.row << 2) as i16) as i32 as u32);
        let mut dma = 0;
        for v in 0..4 {
            let start = self.position[v] >> 16;
            let d7 = hw.bus.r32(rows.wrapping_add(start << 2));
            self.row_data[v] = d7;
            self.play_voice(v, d7, self.position[v], &mut dma, hw);
        }
        self.dma_mask = dma | 0x8000;
    }

    /// One voice's row `d7` (note, instrument, flags/arpeggio/effect, argument)
    /// with the position's transposes in `d6`.
    fn play_voice(&mut self, v: usize, d7: u32, d6: u32, dma: &mut u16, hw: &mut Hw) {
        let va = self.va(v);
        let note = (d7 >> 24) as u16;
        let mut instr = (d7 >> 16) & 0xff;
        if note == 0 || note == 0x80 {
            self.pend += t::EMPTY;
            if note == 0 && instr != 0 {
                self.pend += t::EMPTY_INSTRUMENT;
                self.set_instrument(v, instr, hw);
            }
        } else if note == 0x7f {
            self.pend += t::QUIET;
            self.quiet(v, hw);
        } else {
            let flags = (d7 >> 12) & 0xc;
            self.pend += t::NOTE;
            let mut n = note;
            if flags & 4 == 0 {
                n = n.wrapping_add(d6 as u8 as i8 as u16);
            }
            if instr != 0 && flags & 8 == 0 {
                instr = (instr as u16).wrapping_add((d6 >> 8) as u8 as i8 as u16) as u32 & 0xff;
            }
            let b = &mut *hw.bus;
            b.w16(va + PREV_NOTE, b.r16(va + NOTE));
            b.w16(va + NOTE, n);
            if b.r16(va + PREV_NOTE) == 0 {
                b.w16(va + PREV_NOTE, n);
            }
            self.write_wait(hw, t::OFF);
            hw.dmacon(1 << v);
            self.write_wait(hw, t::LEN);
            hw.len(v, 0);
            let inst = if instr == 0 {
                self.pend += t::NO_INSTRUMENT;
                let inst = hw.bus.r32(va + INSTR);
                if inst == 0 {
                    // straight to the original's quiet routine, which returns from
                    // the row: no effect
                    self.quiet(v, hw);
                    return;
                }
                self.set_synth(v, inst, hw);
                inst
            } else {
                self.pend += t::INSTRUMENT;
                self.set_instrument(v, instr, hw)
            };
            let kind = hw.bus.r16(inst);
            hw.bus.w16(va + TYPE, kind);
            if kind == 0 {
                self.play_sample(v, inst, dma, hw);
            } else {
                self.play_synth(v, inst, dma, hw);
            }
        }
        self.effect(v, d7, hw);
    }

    fn play_sample(&mut self, v: usize, inst: u32, dma: &mut u16, hw: &mut Hw) {
        let va = self.va(v);
        let n = hw.bus.r16(inst + I_WAVE);
        // lsl.w #2: a signed word index into the original's table of 64
        let addr = self.samples.get(((n << 2) as i16 / 4) as usize).copied().unwrap_or(0);
        self.pend += t::SAMPLE;
        if addr == 0 {
            hw.bus.w8(va + FLAGS, hw.bus.r8(va + FLAGS) | 1);
            self.quiet(v, hw);
            return;
        }
        self.write_wait(hw, t::LOC);
        hw.loc(v, addr);
        hw.bus.w32(va + SAMPLE, addr);
        let repeat = hw.bus.r16(inst + I_REPEAT);
        let len = hw.bus.r16(inst + I_LEN).wrapping_add(if repeat == 1 { 0 } else { repeat });
        self.write_wait(hw, t::LEN);
        hw.len(v, len);
        hw.bus.w16(va + LEN, len);
        *dma |= 1 << v;
        self.loop_pending[v] = true;
    }

    /// A synth sound plays from the voice's buffer, which gets the instrument's
    /// waveform.
    fn play_synth(&mut self, v: usize, inst: u32, dma: &mut u16, hw: &mut Hw) {
        let va = self.va(v);
        let src = self.waveforms.wrapping_add((hw.bus.r16(inst + I_WAVE) as u32) << 7);
        hw.bus.w32(va + SAMPLE, src);
        hw.bus.w32(va + 0xbe, 0);
        self.write_wait(hw, t::LOC);
        hw.loc(v, va);
        let len = hw.bus.r16(inst + I_LEN);
        self.write_wait(hw, t::LEN);
        hw.len(v, len);
        hw.bus.w16(va + LEN, len);
        // dbra over (len - 1) >> 3 + 1 blocks of 16 bytes
        let blocks = (((len.wrapping_sub(1) as i16) >> 3) as u16 as u32) + 1;
        self.pend += t::SYNTH_SOUND + t::COPY16 * blocks as i64;
        for i in 0..blocks * 16 {
            let x = hw.bus.r8(src.wrapping_add(i));
            hw.bus.w8(va.wrapping_add(i), x);
        }
        *dma |= 1 << v;
        self.loop_pending[v] = false;
    }

    /// Instrument `n` (from 1) on voice `v`; returns its address.
    fn set_instrument(&mut self, v: usize, n: u32, hw: &mut Hw) -> u32 {
        let va = self.va(v);
        let inst = self.instruments.wrapping_add((n.wrapping_sub(1) & 0xffff) * 152);
        let b = &mut *hw.bus;
        b.w16(va + INSTR_NUM, n as u16);
        b.w32(va + INSTR, inst);
        b.w16(va + VOL, b.r16(inst + I_VOL));
        b.w16(va + PORTA_CNT, b.r16(inst + I_PORTA));
        b.w16(va + PORTA_LEN, b.r16(inst + I_PORTA));
        b.w16(va + VIB_POS, 0);
        b.w16(va + VIB_DELAY, b.r16(inst + I_VIB_DELAY));
        b.w16(va + VIB_SPEED, b.r16(inst + I_VIB_SPEED));
        b.w16(va + VIB_LEVEL, b.r16(inst + I_VIB_LEVEL));
        self.set_synth(v, inst, hw);
        inst
    }

    /// The part of an instrument a note restarts (also with no instrument).
    fn set_synth(&mut self, v: usize, inst: u32, hw: &mut Hw) {
        let va = self.va(v);
        let b = &mut *hw.bus;
        b.w16(va + SLIDE, b.r8(inst + I_FINE + 1) as i8 as u16);
        b.w16(va + ADSR_DELAY, b.r16(inst + I_ADSR_DELAY));
        // clr.l: the sustain counter too
        b.w32(va + ADSR_POS, 0);
        b.w16(va + AMF_DELAY, b.r16(inst + I_AMF_DELAY));
        b.w16(va + AMF_POS, 0);
        b.w16(va + FX_POS, b.r16(inst + I_ARG2));
        b.w16(va + FX_WAVE_POS, 0);
        b.w16(va + FX_DELAY, b.r16(inst + I_FX_DELAY));
        b.w32(va + ARP_POS, 0);
        b.w16(va + SCOPE_VOL, b.r16(va + VOL) << 2);
        b.w16(va + FLAGS - 1, 0);
    }

    /// The original's quiet routine: volume 0, and only the high word of the
    /// instrument pointer cleared.
    fn quiet(&mut self, v: usize, hw: &mut Hw) {
        let va = self.va(v);
        self.write_wait(hw, t::VOL);
        hw.vol(v, 0);
        let b = &mut *hw.bus;
        b.w16(va + VOL, 0);
        b.w16(va + SCOPE_VOL, 0);
        b.w16(va + INSTR_NUM, 0);
        b.w16(va + INSTR, 0);
        self.loop_pending[v] = false;
    }

    /// The row's effect (on every row, with or without a note).
    fn effect(&mut self, v: usize, d7: u32, hw: &mut Hw) {
        let va = self.va(v);
        let b = &mut *hw.bus;
        b.w16(va + SLIDE_SPEED, 0);
        b.w16(va + VOL_SLIDE, 0);
        self.pend += t::ROW_EFFECT[(d7 >> 8) as usize & 0xf];
        let arg = (d7 & 0xff) as u16;
        let sarg = d7 as u8 as i8 as u16;
        match (d7 >> 8) & 0xf {
            1 => b.w16(va + SLIDE_SPEED, sarg),
            2 => b.w16(va + ADSR_POS, arg),
            4 => {
                b.w16(va + VIB_DELAY, 0);
                b.w16(va + VIB_SPEED, (arg & 0xf0) >> 3);
                b.w16(va + VIB_LEVEL, 0xa0u16.wrapping_sub((arg & 0xf) << 4));
            }
            6 => self.master = arg & 0x3f,
            7 => {
                b.w16(va + PORTA_CNT, arg);
                b.w16(va + PORTA_LEN, arg);
            }
            8 => {
                b.w16(va + PORTA_CNT, 0);
                b.w16(va + PORTA_LEN, 0);
            }
            9 if arg > 0 && arg <= 0x40 => self.rows = arg,
            0xa => b.w16(va + VOL_SLIDE, sarg),
            0xb => {
                if arg <= self.pos {
                    self.signal_songend();
                }
                self.pos = arg.wrapping_sub(1);
                self.row = self.rows;
            }
            0xc => {
                let vol = arg.min(0x40);
                b.w16(va + VOL, vol);
                b.w16(va + SCOPE_VOL, vol << 2);
            }
            0xd => self.row = self.rows,
            // ENPP_PokeCommand 1 (the filter): 0 turns it off
            0xe if arg as u8 == 0 => b.ciaa.pra |= 2,
            0xe => b.ciaa.pra &= !2,
            0xf if (arg as i8) > 0 && (arg as i8) <= 16 => self.speed = arg,
            _ => {}
        }
    }

    /// A voice's tick: arpeggio, portamento, vibrato, AMF and slide give the
    /// period; then the synth effect, and the envelope gives the volume.
    fn update_voice(&mut self, v: usize, hw: &mut Hw) {
        let va = self.va(v);
        let d7 = self.row_data[v];
        let inst = hw.bus.r32(va + INSTR);
        if hw.bus.r8(va + FLAGS) & 1 != 0 || inst == 0 {
            self.pend += t::MUTED;
            self.write_wait(hw, t::VOL);
            hw.vol(v, 0);
            let b = &mut *hw.bus;
            let s = b.r16(va + SCOPE_VOL);
            if s != 0 {
                b.w16(va + SCOPE_VOL, s.wrapping_sub(4));
            }
            return;
        }
        let b = &mut *hw.bus;
        let r = |b: &crate::bus::Bus, o: u32| b.r16(va + o);
        let ri = |b: &crate::bus::Bus, o: u32| b.r16(inst + o);
        let mut d3 = r(b, NOTE);
        let mut d4 = r(b, PREV_NOTE);

        let arp = (d7 as u16 >> 12) & 3;
        if arp != 0 {
            self.pend += t::ARPEGGIO;
            let a = inst.wrapping_add(I_ARPEGGIOS + 16 * (arp as u32 - 1));
            let pos = r(b, ARP_POS);
            let x = b.r8(a.wrapping_add(2).wrapping_add(pos as i16 as i32 as u32));
            d3 = (d3 & 0xff00) | (d3 as u8).wrapping_add(x) as u16;
            d4 = (d4 & 0xff00) | (d4 as u8).wrapping_add(x) as u16;
            let pos = pos.wrapping_add(1);
            b.w16(va + ARP_POS, pos);
            let end = b.r8(a).wrapping_add(b.r8(a + 1)) as u16;
            if !(end as i16 > pos as i16) {
                b.w16(va + ARP_POS, b.r8(a) as u16);
            }
        } else if d7 & 0xf00 == 0 && d7 & 0xff != 0 {
            self.pend += t::EFFECT_ARPEGGIO;
            let x = match (self.counter as i16) % 3 {
                0 => 0,
                1 => d7 as u16 & 0xf,
                _ => (d7 as u16 & 0xff) >> 4,
            };
            d3 = d3.wrapping_add(x);
            d4 = d4.wrapping_add(x);
        }
        let period = |n: u16| {
            let i = n << 1;
            if i >= 0xda { PERIODS[0] } else { PERIODS[i as usize / 2] }
        };
        let mut d3 = period(d3);
        let d4 = period(d4);

        let (cnt, len) = (r(b, PORTA_CNT), r(b, PORTA_LEN));
        if len != 0 && cnt != 0 && d3 != d4 {
            self.pend += t::PORTAMENTO;
            let cnt = cnt.wrapping_sub(1);
            b.w16(va + PORTA_CNT, cnt);
            let diff = d4.wrapping_sub(d3) as i16 as i32 * cnt as i16 as i32;
            d3 = divs(diff, len as i16).wrapping_add(d3);
        }

        let delay = r(b, VIB_DELAY);
        if delay != 0xff {
            if delay != 0 {
                self.pend += t::VIBRATO_DELAY;
                b.w16(va + VIB_DELAY, delay - 1);
            } else {
                let pos = r(b, VIB_POS);
                let x = VIBRATO[pos as usize & 0xff] as i32;
                let level = ri(b, I_VIB_LEVEL);
                self.pend += t::VIBRATO + if level != 0 { t::VIBRATO_LEVEL } else { 0 };
                if level != 0 {
                    d3 = d3.wrapping_add(divs(x * 4, level as i16));
                }
                b.w16(va + VIB_POS, pos.wrapping_add(ri(b, I_VIB_SPEED)) & 0xff);
            }
        }

        let amf_len = ri(b, I_AMF_LEN).wrapping_add(ri(b, I_AMF_REPEAT));
        if amf_len != 0 {
            self.pend += t::AMF;
            let table = self.amf.wrapping_add((ri(b, I_AMF_NUM) as u32) << 7);
            let pos = r(b, AMF_POS);
            let x = b.r8(table.wrapping_add(pos as i16 as i32 as u32)) as i8 as u16;
            d3 = d3.wrapping_sub(x);
            let delay = r(b, AMF_DELAY).wrapping_sub(1);
            b.w16(va + AMF_DELAY, delay);
            if delay == 0 {
                b.w16(va + AMF_DELAY, ri(b, I_AMF_DELAY));
                self.pend += t::AMF_STEP;
                let pos = pos.wrapping_add(1);
                b.w16(va + AMF_POS, pos);
                if !(amf_len as i16 > pos as i16) {
                    let mut p = ri(b, I_AMF_LEN);
                    if ri(b, I_AMF_REPEAT) == 0 {
                        p = p.wrapping_sub(1);
                    }
                    b.w16(va + AMF_POS, p);
                }
            }
        }

        d3 = d3.wrapping_sub(r(b, SLIDE));
        if (d3 as i16) < 0x71 {
            d3 = 0x71;
        }
        if self.counter != 0 {
            self.pend += t::SLIDE;
            b.w16(va + SLIDE, r(b, SLIDE).wrapping_add(r(b, SLIDE_SPEED)));
        }
        self.write_wait(hw, t::PER);
        hw.per(v, d3);

        if hw.bus.r16(inst) != 0 {
            self.synth_effect(v, inst, hw);
        }

        let b = &mut *hw.bus;
        let adsr_len = ri(b, I_ADSR_LEN).wrapping_add(ri(b, I_ADSR_REPEAT));
        self.pend += if adsr_len == 0 { t::NO_ADSR } else { t::ADSR };
        if adsr_len == 0 {
            let vol = ((r(b, VOL) as u32 * self.master as u32) >> 6) as u16;
            self.write_wait(hw, t::VOL);
            hw.vol(v, vol & 0x7f);
            let b = &mut *hw.bus;
            let s = r(b, SCOPE_VOL);
            if s != 0 {
                b.w16(va + SCOPE_VOL, s.wrapping_sub(4));
            }
        } else {
            let table = self.adsr.wrapping_add(wave_w(ri(b, I_ADSR_NUM)));
            let pos = r(b, ADSR_POS);
            let d1 = (b.r8(table.wrapping_add(pos as i16 as i32 as u32)) as u32 * self.master as u32) >> 6;
            let vol = ((r(b, VOL) as u32 * (d1 & 0xffff)) >> 6) as u16;
            self.write_wait(hw, t::VOL);
            hw.vol(v, vol & 0x7f);
            let b = &mut *hw.bus;
            b.w16(va + SCOPE_VOL, vol << 2);
            let mut advance = true;
            if d7 >> 24 == 0x80 && !((pos as i16) < ri(b, I_SUSTAIN_POINT) as i16) {
                let sustain = ri(b, I_SUSTAIN_DELAY);
                self.pend += t::SUSTAIN;
                let cnt = r(b, SUSTAIN_CNT);
                if sustain == 0 {
                    advance = false;
                } else if cnt != 0 {
                    b.w16(va + SUSTAIN_CNT, cnt - 1);
                    advance = false;
                } else {
                    b.w16(va + SUSTAIN_CNT, sustain);
                }
            }
            if advance {
                let delay = r(b, ADSR_DELAY).wrapping_sub(1);
                self.pend += t::ADSR_ADVANCE;
                b.w16(va + ADSR_DELAY, delay);
                if delay == 0 {
                    b.w16(va + ADSR_DELAY, ri(b, I_ADSR_DELAY));
                    self.pend += t::ADSR_STEP;
                    let pos = r(b, ADSR_POS).wrapping_add(1);
                    b.w16(va + ADSR_POS, pos);
                    if !(adsr_len as i16 > pos as i16) {
                        let repeat = ri(b, I_ADSR_REPEAT);
                        let mut p = ri(b, I_ADSR_LEN);
                        if repeat == 0 {
                            p = p.wrapping_sub(1);
                        }
                        b.w16(va + ADSR_POS, p);
                        if repeat == 0 && d1 as u8 == 0 {
                            b.w8(va + FLAGS, b.r8(va + FLAGS) | 1);
                        }
                    }
                }
            }
        }

        let b = &mut *hw.bus;
        let vol = r(b, VOL).wrapping_add(r(b, VOL_SLIDE));
        let vol = if (vol as i16) < 0 { 0 } else { vol.min(0x40) };
        b.w16(va + VOL, vol);
    }

    /// The instrument's synth effect, every `I_FX_DELAY` ticks, on the voice's
    /// buffer, as the original's code does it.
    fn synth_effect(&mut self, v: usize, inst: u32, hw: &mut Hw) {
        let va = self.va(v);
        let b = &mut *hw.bus;
        let delay = b.r16(va + FX_DELAY).wrapping_sub(1);
        self.pend += t::SYNTH;
        b.w16(va + FX_DELAY, delay);
        if delay != 0 {
            return;
        }
        b.w16(va + FX_DELAY, b.r16(inst + I_FX_DELAY));
        let effect = b.r16(inst + I_EFFECT) as i16;
        self.pend += t::EFFECT[effect.clamp(0, 0x11) as usize];
        if effect <= 0 || effect > 0x11 {
            return;
        }
        let ri = |b: &crate::bus::Bus, o: u32| b.r16(inst + o);
        let (arg1, arg2, arg3) = (ri(b, I_ARG1), ri(b, I_ARG2), ri(b, I_ARG3));
        let at = |o: u16| va.wrapping_add(o as i16 as i32 as u32);
        let own = self.waveforms.wrapping_add(wave_w(ri(b, I_WAVE)));
        let other = self.waveforms.wrapping_add(wave_w(arg1));
        let flags = b.r8(va + FLAGS);
        let fx_pos = b.r16(va + FX_POS);
        let wave_pos = b.r16(va + FX_WAVE_POS);
        let cost = |n: i32| n.max(0) as i64 * t::EFFECT_BYTE[effect as usize];
        let mut inc = true;
        match effect {
            // wave negator
            1 => {
                let a = at(fx_pos);
                b.w8(a, b.r8(a).wrapping_neg());
            }
            // free negator
            2 => {
                inc = false;
                if flags & 4 == 0 {
                    self.free_negator(v, inst, hw);
                }
            }
            // rotate vertical
            3 => {
                let n = for_range(b, arg2, arg3, |b, i| {
                    let a = at(i);
                    b.w8(a, b.r8(a).wrapping_add(arg1 as u8));
                });
                self.pend += cost(n);
            }
            // rotate horizontal
            4 => {
                let first = b.r8(at(arg2));
                let mut a = at(arg2);
                let mut d0 = arg3.wrapping_sub(arg2);
                loop {
                    b.w8(a, b.r8(a.wrapping_add(1)));
                    a = a.wrapping_add(1);
                    d0 = d0.wrapping_sub(1);
                    if (d0 as i16) < 0 {
                        break;
                    }
                }
                b.w8(a, first);
                self.pend += cost(arg3.wrapping_sub(arg2) as i16 as i32 + 1);
            }
            // alien voice
            5 => {
                inc = false;
                let n = for_range(b, arg2, arg3, |b, i| {
                    let a = at(i);
                    let x = b.r8(other.wrapping_add(i as i16 as i32 as u32));
                    b.w8(a, b.r8(a).wrapping_add(x));
                });
                self.pend += cost(n);
            }
            // poly negator
            6 => {
                b.w8(at(fx_pos), b.r8(own.wrapping_add(fx_pos as i16 as i32 as u32)));
                let p = if fx_pos as i16 >= arg3 as i16 { arg2.wrapping_sub(1) } else { fx_pos };
                let a = at(p.wrapping_add(1));
                b.w8(a, b.r8(a).wrapping_neg());
            }
            // shack wave 1 and 2
            7 | 8 => {
                let n = self.shack_wave(v, arg1, arg2, arg3, fx_pos, hw);
                self.pend += cost(n);
                if effect == 8 {
                    let b = &mut *hw.bus;
                    let a = at(arg2).wrapping_add(wave_pos as i16 as i32 as u32);
                    b.w8(a, b.r8(a).wrapping_neg());
                    let p = wave_pos.wrapping_add(1);
                    let p = if (arg3.wrapping_sub(arg2).wrapping_sub(p) as i16) < 0 { 0 } else { p };
                    b.w16(va + FX_WAVE_POS, p);
                }
            }
            // metamorph and oscillator
            9 | 15 => {
                inc = false;
                let mut flags = flags;
                let mut src = other;
                if effect == 15 {
                    if flags & 2 != 0 {
                        flags = (flags ^ 8) & !2;
                        b.w8(va + FLAGS, flags);
                    }
                    if flags & 8 != 0 {
                        src = own;
                    }
                } else if flags & 2 != 0 {
                    return;
                }
                let mut changed = false;
                let n = for_range(b, arg2, arg3, |b, i| {
                    let a = at(i);
                    let (x, y) = (b.r8(a) as i8, b.r8(src.wrapping_add(i as i16 as i32 as u32)) as i8);
                    if x != y {
                        changed = true;
                        b.w8(a, if x < y { x.wrapping_add(1) } else { x.wrapping_sub(1) } as u8);
                    }
                });
                if !changed {
                    b.w8(va + FLAGS, b.r8(va + FLAGS) | 2);
                }
                self.pend += cost(n);
            }
            // laser
            10 => {
                if arg3 > wave_pos {
                    b.w16(va + SLIDE, b.r16(va + SLIDE).wrapping_add(arg2 as u8 as i8 as u16));
                    b.w16(va + FX_WAVE_POS, wave_pos.wrapping_add(1));
                }
            }
            // wave alias
            11 => {
                let n = wave_alias(b, va, arg1, arg2, arg3);
                self.pend += cost(n);
            }
            // noise generator 1
            12 => {
                let a = at(fx_pos);
                let h = b.beam().1 as u8;
                b.w8(a, b.r8(a) ^ h);
            }
            // low-pass filters 1 and 2
            13 | 14 => {
                let thresholds = (effect == 14).then(|| self.waveforms.wrapping_add((arg1 as u32) << 7));
                let n = low_pass(b, va, arg1, arg2, arg3, thresholds);
                self.pend += cost(n);
            }
            // noise generator 2: the beam moves on while it runs
            16 => {
                inc = false;
                let base = at(arg2);
                let mut d1 = arg3.wrapping_sub(arg2);
                loop {
                    let b = &mut *hw.bus;
                    let a = base.wrapping_add(d1 as i16 as i32 as u32);
                    let x = (b.r8(a) ^ 5).rotate_left(2).wrapping_add(b.beam().1 as u8);
                    b.w8(a, x);
                    self.pend += t::EFFECT_BYTE[16];
                    self.spend(hw);
                    d1 = d1.wrapping_sub(1);
                    if (d1 as i16) < 0 {
                        break;
                    }
                }
            }
            // frequency drum
            17 => {
                let mut p = wave_pos;
                if !(arg3 > p) {
                    b.w16(va + SLIDE, ri(b, I_FINE));
                    p = 0;
                }
                let d = (arg2 << 8) | b.r8(inst + I_ARG1 + 1) as u16;
                b.w16(va + SLIDE, b.r16(va + SLIDE).wrapping_sub(d));
                b.w16(va + FX_WAVE_POS, p.wrapping_add(1));
            }
            _ => unreachable!(),
        }
        if inc {
            let b = &mut *hw.bus;
            let p = b.r16(va + FX_POS).wrapping_add(1);
            b.w16(va + FX_POS, if p > arg3 { arg2 } else { p });
        }
    }

    /// Free negator: the instrument's waveform, negated below the level the
    /// effect's waveform gives, copied as the original does, a longword at a
    /// time from the end.
    fn free_negator(&mut self, v: usize, inst: u32, hw: &mut Hw) {
        let va = self.va(v);
        let b = &mut *hw.bus;
        let ri = |o: u32| b.r16(inst + o);
        let (arg1, arg2, arg3) = (ri(I_ARG1), ri(I_ARG2), ri(I_ARG3));
        let table = self.waveforms.wrapping_add(((arg1 << 7) as u32) & 0xffff);
        let wave_pos = b.r16(va + FX_WAVE_POS);
        let level = (b.r8(table.wrapping_add(wave_pos as i16 as i32 as u32)) & 0x7f) as u16;
        let src = self.waveforms.wrapping_add(wave_w(ri(I_WAVE)));
        let mut d1 = ri(I_LEN) << 1;
        let mut a1 = src.wrapping_add(d1 as i16 as i32 as u32);
        let mut a0 = va.wrapping_add(d1 as i16 as i32 as u32);
        let mut d0 = level;
        let mut steps = 0;
        let mut negated = 0;
        while (d1 as i16) > 0 && !(d0 as i16 > d1 as i16) {
            for _ in 0..4 {
                a1 = a1.wrapping_sub(1);
                a0 = a0.wrapping_sub(1);
                b.w8(a0, b.r8(a1));
            }
            d1 = d1.wrapping_sub(4);
            steps += 1;
        }
        d0 = d0.wrapping_sub(d1);
        d1 = d1.wrapping_add(d0);
        a1 = a1.wrapping_add(d0 as i16 as i32 as u32);
        a0 = a0.wrapping_add(d0 as i16 as i32 as u32);
        if (d1 as i16) > 0 {
            let mut longs = true;
            if d0 & 1 != 0 {
                a1 = a1.wrapping_sub(1);
                a0 = a0.wrapping_sub(1);
                b.w8(a0, b.r8(a1).wrapping_neg());
                self.pend += t::NEGATOR_ODD;
                d1 = d1.wrapping_sub(1);
                longs = (d1 as i16) > 0;
            }
            while longs {
                for _ in 0..4 {
                    a1 = a1.wrapping_sub(1);
                    a0 = a0.wrapping_sub(1);
                    b.w8(a0, b.r8(a1).wrapping_neg());
                }
                d1 = d1.wrapping_sub(4);
                steps += 1;
                negated += 1;
                longs = (d1 as i16) > 0;
            }
        }
        let p = wave_pos.wrapping_add(1);
        b.w16(va + FX_WAVE_POS, p);
        if p > arg2.wrapping_add(arg3) {
            b.w16(va + FX_WAVE_POS, arg2);
            if arg3 == 0 && level == 0 {
                b.w8(va + FLAGS, b.r8(va + FLAGS) | 4);
            }
        }
        self.pend += t::NEGATOR + t::NEGATOR_COPY * (steps - negated) + t::NEGATOR_NEGATE * negated;
    }

    /// Shack wave: the effect's waveform byte at start + position added to the
    /// buffer from start to stop. Returns the bytes changed.
    fn shack_wave(&mut self, v: usize, arg1: u16, arg2: u16, arg3: u16, fx_pos: u16, hw: &mut Hw) -> i32 {
        let va = self.va(v);
        let b = &mut *hw.bus;
        let src = self.waveforms.wrapping_add(wave_w(arg1)).wrapping_add(arg2 as i16 as i32 as u32);
        let x = b.r8(src.wrapping_add(fx_pos as i16 as i32 as u32));
        for_range(b, arg2, arg3, |b, i| {
            let a = va.wrapping_add(i as i16 as i32 as u32);
            b.w8(a, b.r8(a).wrapping_add(x));
        })
    }
}

/// The original's byte loops from `start` to `stop` (a byte first if `start` is
/// odd, then longwords, then bytes): `f` for each offset; returns their count.
/// With stop before start it does only the odd first byte.
fn for_range(b: &mut crate::bus::Bus, start: u16, stop: u16, mut f: impl FnMut(&mut crate::bus::Bus, u16)) -> i32 {
    let n = stop.wrapping_sub(start) as i16 as i32 + 1;
    let n = if n > 0 { n } else if start & 1 != 0 { 1 } else { 0 };
    for k in 0..n {
        f(b, start.wrapping_add(k as u16));
    }
    n
}

/// Wave alias: each byte from start to stop moves by `delta` away from the
/// next one. The original does it a longword at a time with word additions,
/// so a carry runs into the neighbouring byte, and its fourth byte is compared
/// as changed by the first's carry.
fn wave_alias(b: &mut crate::bus::Bus, va: u32, delta: u16, start: u16, stop: u16) -> i32 {
    let mut a = va.wrapping_add(start as i16 as i32 as u32);
    let mut d1 = stop.wrapping_sub(start);
    let step = |d0: u32, next: u8| -> u32 {
        let w = d0 as u16;
        let w = if (d0 as u8 as i8) > next as i8 { w.wrapping_sub(delta) } else { w.wrapping_add(delta) };
        (d0 & 0xffff_0000) | w as u32
    };
    let mut n = 0;
    if start & 1 != 0 {
        let d0 = step(b.r8(a) as u32, b.r8(a.wrapping_add(1)));
        b.w8(a, d0 as u8);
        a = a.wrapping_add(1);
        n += 1;
        d1 = d1.wrapping_sub(1);
        if d1 == 0xffff {
            return n;
        }
    }
    while (d1.wrapping_sub(3) as i16) >= 0 {
        let mut d0 = b.r32(a);
        let mut d4 = (d0.rotate_left(8) & !0xff) | b.r8(a.wrapping_add(4)) as u32;
        for _ in 0..4 {
            d0 = d0.rotate_left(8);
            d4 = d4.rotate_left(8);
            d0 = step(d0, d4 as u8);
        }
        b.w32(a, d0);
        a = a.wrapping_add(4);
        d1 = d1.wrapping_sub(4);
        n += 4;
    }
    while (d1 as i16) >= 0 {
        let d0 = step(b.r8(a) as u32, b.r8(a.wrapping_add(1)));
        b.w8(a, d0 as u8);
        a = a.wrapping_add(1);
        d1 = d1.wrapping_sub(1);
        n += 1;
    }
    n
}

/// Low-pass filters: a byte further than the threshold (`delta`, or for the
/// second filter the bytes of a waveform from its stop position on) from the
/// next one (from the first for the last) moves 2 towards it.
fn low_pass(b: &mut crate::bus::Bus, va: u32, delta: u16, start: u16, stop: u16, thresholds: Option<u32>) -> i32 {
    let first = va.wrapping_add(start as i16 as i32 as u32);
    let last = va.wrapping_add(stop as i16 as i32 as u32);
    let mut th = thresholds.map(|t| t.wrapping_add(stop as i16 as i32 as u32));
    // d0's high byte is the start position's
    let hi = start & 0xff00;
    let mut a = first;
    let mut n = 0;
    loop {
        let x = b.r8(a);
        let y = if a == last { b.r8(first) } else { b.r8(a.wrapping_add(1)) };
        let up = x as i8 <= y as i8;
        let mut dif = x.wrapping_sub(y);
        if dif & 0x80 != 0 {
            dif = dif.wrapping_neg();
        }
        let limit = match &mut th {
            Some(t) => {
                let l = (b.r8(*t) & 0x7f) as u16;
                *t = t.wrapping_add(1);
                l
            }
            None => delta,
        };
        if (hi | dif as u16) as i16 > limit as i16 {
            b.w8(a, if up { x.wrapping_add(2) } else { x.wrapping_sub(2) });
        }
        a = a.wrapping_add(1);
        n += 1;
        if (last as i32) < (a as i32) {
            break;
        }
    }
    n
}

include!("sonicarranger_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// A "SOARV1.0" module: one subsong of two rows, one position, one sample
    /// instrument with no loop, played by voice 0's first row.
    fn module() -> Vec<u8> {
        let mut d = b"SOARV1.0".to_vec();
        let chunk = |d: &mut Vec<u8>, mark: &[u8], count: u32| {
            d.extend(mark);
            d.extend(count.to_be_bytes());
        };
        chunk(&mut d, b"STBL", 1);
        // speed 3, 2 rows, positions 0 to 0, restart 0, tempo 50
        for w in [3u16, 2, 0, 0, 0, 50] {
            d.extend(w.to_be_bytes());
        }
        chunk(&mut d, b"OVTB", 1);
        // voice 0 from row 0, the others from row 2 (both empty)
        for start in [0u32, 2, 2, 2] {
            d.extend((start << 16).to_be_bytes());
        }
        chunk(&mut d, b"NTBL", 4);
        // note 25 with instrument 1
        for row in [0x1901_0000u32, 0, 0, 0] {
            d.extend(row.to_be_bytes());
        }
        chunk(&mut d, b"INST", 1);
        let mut inst = [0u8; 152];
        inst[5] = 16; // 16 words
        inst[7] = 1; // no loop
        inst[0x11] = 64; // volume
        d.extend(inst);
        chunk(&mut d, b"SD8B", 1);
        d.extend([0; 8 + 30]);
        d.extend(32u32.to_be_bytes());
        d.extend((0..32).map(|i| (i * 8) as u8));
        chunk(&mut d, b"SYWT", 0);
        chunk(&mut d, b"SYAR", 0);
        chunk(&mut d, b"SYAF", 0);
        d
    }

    #[test]
    fn plays_a_sample_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.sa"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Sonic Arranger");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // both InitSounds' DMA off, then the note's off and on
        assert_eq!(&reg(0xDFF096)[..4], &[0x000f, 0x000f, 0x0001, 0x8001]);
        // voice 0: no length, the sample's, then the empty loop's
        assert_eq!(&reg(0xDFF0A4)[..3], &[0, 16, 1]);
        assert_eq!(reg(0xDFF0A6)[0], super::PERIODS[25]);
        // 64 by the master volume of $3f, over 64
        assert_eq!(reg(0xDFF0A8)[0], 63);
        // the other voices stay silent
        assert!(reg(0xDFF0B8).iter().all(|&v| v == 0));
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
