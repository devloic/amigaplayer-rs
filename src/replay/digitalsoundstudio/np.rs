//! The newer Digital Sound Studio the Eagleplayer predates: a module whose
//! tempo byte (offset 8) is not 0 ("player module V1.2"). It is written from a
//! black-box specification of that player's register writes (its Paula and CIA
//! writes, as the write logs show them), not from its code: the specification
//! and an executable model of it were checked against the reference's logs of
//! 176 probe modules, the 11 real tempo modules and 1,750 random ones. Where
//! the player agrees with NostalgicPlayer's Digital Sound Studio (Copyright (c)
//! 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer) the effects
//! are that player's; the differences are the ones the logs show:
//!
//! * the timer is 1773375 / BPM plus one E-clock ticks (a header tempo under 28
//!   is 3904), anchored at the start of the run; the start writes the four
//!   volumes twice, and the first row is at tick `speed`;
//! * every row ends with the DMA set of the voices that started (always, even
//!   with none), a wait of nine raster lines, and the loop registers of all
//!   four voices (a sample without a loop loops its first word);
//! * a note needs only a period, a voice keeps its sample between rows;
//! * volumes are bytes with signed compares and the player's own limits, the
//!   master volume argument is signed, arpeggio adds its offsets (the counter
//!   is shared by the voices), `$14` sets a loop on argument 0, `$7FF` stops a
//!   voice and restarts it silent at the row's end, the delay, cut and retrigger
//!   effects compare signed bytes.
//!
//! The writes are spaced as the reference's are, to within a tick's costs of a
//! few hundred colour clocks: the first write of a tick comes about 5.6k after
//! the timer, and a DMA off or a row-end DMA on is followed by nine raster
//! lines (`wait_lines`).

use super::SAMPLES_OFF;
use crate::bus::Bus;
use crate::paula::LINE;
use crate::replay::Hw;
use m68k::AddressBus;
use std::cell::Cell;

include!("np_tables.rs");

#[derive(Clone, Copy, Default)]
struct Voice {
    sample: u8,
    period: u16,
    effect: u8,
    arg: u8,
    /// The sample's address plus its start offset; 0 is no sample.
    start: u32,
    len: u16,
    loop_start: u32,
    loop_len: u16,
    finetune: u8,
    volume: u8,
    pitch: i32,
    offset: u16,
    port_end: i32,
    port_up: bool,
    port_speed: i32,
    tone_slide: bool,
    tone_port: bool,
    loop_row: i32,
    loop_cnt: u8,
}

pub struct Np {
    base: u32,
    mem: u32,
    voices: [Voice; 4],
    tempo: u8,
    speed: u8,
    counter: u16,
    position: u16,
    row: i32,
    jump: bool,
    new_position: u16,
    set_loop: bool,
    loop_row: i32,
    inv: i32,
    next_retrig: u32,
    arp: i32,
    /// A write was made since this was cleared (the voices' pacing).
    wrote: bool,
    /// The clock the first tick is measured from (see `new`).
    anchor: i64,
    first: Cell<bool>,
    pub songend: bool,
}

fn s8(x: u32) -> i32 {
    x as u8 as i8 as i32
}

// Costs in colour clocks, measured from the reference's write logs (SPEC.md).
/// Timer to the row's first voice, and to an effect tick's first voice.
const ROW_START: u64 = 5600;
const FX_START: u64 = 5530;
/// An effect tick's voice that writes nothing.
const FX_IDLE: u64 = 75;
/// A voice's handler that writes nothing; reaching a note's first write.
const IDLE_VOICE: u64 = 50;
const NOTE_ENTRY: u64 = 0;
/// After a voice's last write, to the next voice.
const VOICE_GAP: u64 = 89;
/// From the last voice to the row end's DMA set.
const ROW_END: u64 = 223;
/// A note's LOC to LEN, LEN to PER (finetune 0), PER to VOL.
const LOC_LEN: u64 = 10;
const LEN_PER: u64 = 101;
const PER_VOL: u64 = 250;
const PER_VOL_FX: u64 = 315;
/// An effect tick's voice that writes, to the next voice.
const FX_GAP: u64 = 150;
const ARP_OTHER: u64 = 880;
/// One VOL write to the next, of a master slide.
const VOL_VOL: u64 = 140;
/// A loop pair's LEN to the next voice's LOC.
const LOOP_GAP: u64 = 40;

impl Np {
    /// Does the player's start-up: the four volumes cleared twice, after the
    /// loops over the positions and the samples that precede them.
    pub fn new(d: &[u8], base: u32, mem: u32, bus: &mut Bus) -> Np {
        let run = bus.clock;
        let n = u16::from_be_bytes([d[0x59c], d[0x59d]]) as u64;
        let (mut max, mut raised) = (0u8, 0u64);
        for k in 0..n as usize {
            let p = d.get(0x59e + k).copied().unwrap_or(0);
            if p >= max {
                max = p;
                raised += 1;
            }
        }
        let (mut samples, mut loops) = (0u64, 0u64);
        for k in 0..31 {
            let o = 0xa + 0x2e * k;
            if u16::from_be_bytes([d[o + 0x22], d[o + 0x23]]) != 0 {
                samples += 1;
                if u16::from_be_bytes([d[o + 0x28], d[o + 0x29]]) != 1 {
                    loops += 1;
                }
            }
        }
        // 2536 + 15.5 P + 2 R + 84.5 S + 10 L, fitted to the original's start
        let t0 = (2536 * 2 + 31 * n + 4 * raised + 169 * samples + 20 * loops) / 2;
        bus.clock += t0;
        let mut hw = Hw { bus };
        for round in 0..2 {
            for (i, gap) in [0u64, 138, 150, 162].into_iter().enumerate() {
                if i == 0 && round == 1 {
                    hw.wait(254);
                } else {
                    hw.wait(gap);
                }
                hw.vol(i, 0);
            }
        }
        // the ticks come the start's own time after the run's, which the empty
        // header's start (3249) does not have: every write of a tick is that
        // much later than the tick, in the logs of every module
        // (the run starts START_PHASE into a frame)
        let anchor = (hw.bus.clock - run + crate::engine::START_PHASE) as i64 - (t0 as i64 - 3249);
        Np {
            base,
            mem,
            voices: [Voice { loop_row: -1, ..Voice::default() }; 4],
            tempo: d[8],
            speed: d[9],
            counter: 0,
            position: 0,
            row: 0,
            jump: false,
            new_position: 0,
            set_loop: false,
            loop_row: 0,
            inv: 0,
            next_retrig: 0,
            arp: 3,
            wrote: false,
            anchor,
            first: Cell::new(true),
            songend: false,
        }
    }

    /// The timer's period in colour clocks: 1773375 / BPM plus one, in E-clocks
    /// of 5; a header tempo under 28 does not fit the timer and is 3904. The
    /// first one is shortened by the time the run has already spent, as the
    /// original's timer counts from the start of the run.
    pub fn tick_clocks(&self) -> u64 {
        let t = self.tempo.max(1) as u64;
        let t = if t < 28 { 3904 } else { 1_773_375 / t + 1 } * 5;
        if self.first.replace(false) {
            (t as i64 - self.anchor).max(1) as u64
        } else {
            t
        }
    }

    fn positions(&self, hw: &Hw) -> u16 {
        hw.bus.r16(self.base + 0x59c)
    }

    // The writes, noting that the voice made one.
    fn dma(&mut self, hw: &mut Hw, v: u16) {
        self.wrote = true;
        hw.dmacon(v);
    }
    fn loc(&mut self, hw: &mut Hw, i: usize, a: u32) {
        self.wrote = true;
        hw.loc(i, a);
    }
    fn len(&mut self, hw: &mut Hw, i: usize, w: u16) {
        self.wrote = true;
        hw.len(i, w);
    }
    fn per(&mut self, hw: &mut Hw, i: usize, p: i32) {
        self.wrote = true;
        hw.per(i, p as u16);
    }
    fn vol(&mut self, hw: &mut Hw, i: usize, v: i32) {
        self.wrote = true;
        hw.vol(i, v as u16);
    }

    /// Nine raster lines after the DMACON write just made: the next write is
    /// at the 9th line boundary on, 79 colour clocks into it.
    fn wait_lines(&self, hw: &mut Hw) {
        let c = hw.bus.clock;
        let target = ((c + 31) / LINE + 9) * LINE + 79;
        hw.wait(target.saturating_sub(c));
    }

    pub fn tick(&mut self, hw: &mut Hw) {
        self.counter = self.counter.wrapping_add(1);
        if self.counter == self.speed as u16 {
            self.counter = 0;
            self.next_retrig = 0;
            self.arp = 3;
            self.row(hw);
        } else {
            hw.wait(FX_START);
            for i in 0..4 {
                self.wrote = false;
                hw.wait(FX_IDLE);
                self.make_effects(i, hw);
                if self.wrote {
                    hw.wait(FX_GAP);
                }
            }
        }
    }

    fn row(&mut self, hw: &mut Hw) {
        self.new_position = self.position;
        let pat = if self.position < self.positions(hw) {
            hw.bus.r8(self.base + 0x59e + self.position as u32) as u32
        } else {
            0
        };
        hw.wait(ROW_START);
        let mut mask = 0u16;
        for i in 0..4 {
            let a = self.base + 0x61e + (pat << 10) + 16 * self.row as u32 + 4 * i as u32;
            self.wrote = false;
            hw.wait(IDLE_VOICE);
            self.setup(i, a, hw, &mut mask);
            if self.wrote && i < 3 {
                hw.wait(VOICE_GAP);
            }
        }
        hw.wait(ROW_END);
        self.dma(hw, 0x8000 | mask);
        self.wait_lines(hw);
        for i in 0..4 {
            if self.voices[i].effect != 0x17 {
                self.write_loop(i, hw);
                hw.wait(LOOP_GAP);
            }
        }
        if self.set_loop {
            self.row = self.loop_row;
            self.set_loop = false;
        }
        self.row += 1;
        if self.row >= 64 || self.jump {
            self.jump = false;
            self.row = 0;
            for v in &mut self.voices {
                v.loop_row = -1;
            }
            let old = self.position;
            let mut p = self.new_position.wrapping_add(1);
            let mut wrapped = false;
            if p == self.positions(hw) {
                p = 0;
                wrapped = true;
            }
            self.position = p;
            if wrapped || p < old {
                self.songend = true;
            }
        }
    }

    fn write_loop(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        self.loc(hw, i, v.loop_start);
        hw.wait(LOC_LEN);
        self.len(hw, i, v.loop_len);
    }

    /// A retrigger or a delayed note: the voice's DMA off, its sample, DMA on,
    /// and only its loop pair.
    fn play(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        self.dma(hw, 1 << i);
        hw.wait(12);
        self.loc(hw, i, v.start);
        hw.wait(LOC_LEN);
        self.len(hw, i, v.len);
        self.wait_lines(hw);
        self.dma(hw, 0x8000 | 1 << i);
        self.wait_lines(hw);
        self.write_loop(i, hw);
    }

    fn set_volume(&mut self, i: usize, hw: &mut Hw) {
        let v = (self.voices[i].volume as i32 - self.inv).max(0) & 0x7f;
        self.vol(hw, i, v);
    }

    /// The volume writes of the add, subtract and master routines: the same
    /// but for the mask.
    fn vol_out(&mut self, i: usize, hw: &mut Hw) {
        let v = (self.voices[i].volume as i32 - self.inv).max(0);
        self.vol(hw, i, v);
    }

    fn vol_out_all(&mut self, hw: &mut Hw) {
        for i in 0..4 {
            if i != 0 {
                hw.wait(VOL_VOL);
            }
            self.vol_out(i, hw);
        }
    }

    fn add_volume(&mut self, i: usize, a: u8, hw: &mut Hw) {
        let mut n = self.voices[i].volume.wrapping_add(a);
        if n as i8 > 64 {
            n = 64;
        }
        self.voices[i].volume = n;
        self.vol_out(i, hw);
    }

    fn sub_volume(&mut self, i: usize, a: u8, hw: &mut Hw) {
        let v = self.voices[i].volume;
        let n = if (v as i8) < (a as i8) { 0 } else { v.wrapping_sub(a) };
        self.voices[i].volume = n;
        self.vol_out(i, hw);
    }

    /// The note's period in the voice's finetune: the table of finetune 0 is
    /// searched for it, the last entry stands for one not in it.
    fn adjust_finetune(&self, period: u16, ft: u8) -> (u16, u64) {
        if ft == 0 {
            return (period, 0);
        }
        let k = (0..48).find(|&k| NP_PERIODS[0][k] == period).unwrap_or(47);
        (NP_PERIODS[ft as usize & 15][k], 152 + 27 * k as u64 / 2 - LEN_PER)
    }

    fn adjust_tone(&self, period: i32, ft: u8) -> i32 {
        let t = &NP_PERIODS[ft as usize & 15];
        t.iter().find(|&&p| period >= p as i32).copied().unwrap_or(t[47]) as i32
    }

    fn apply_pitch(&mut self, i: usize, tone: bool, hw: &mut Hw) {
        let v = self.voices[i];
        let p = if tone { self.adjust_tone(v.pitch, v.finetune) } else { v.pitch };
        self.per(hw, i, p);
    }

    fn do_portamento(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        if v.port_end == 0 {
            return;
        }
        if v.port_up {
            v.pitch -= v.port_speed;
            if v.pitch <= v.port_end {
                v.pitch = v.port_end;
                v.port_end = 0;
            }
        } else {
            v.pitch += v.port_speed;
            if v.pitch >= v.port_end {
                v.pitch = v.port_end;
                v.port_end = 0;
            }
        }
        self.apply_pitch(i, self.voices[i].tone_port, hw);
    }

    fn slide(&mut self, i: usize, up: bool, tone: bool, hw: &mut Hw) {
        let v = &mut self.voices[i];
        let lim = NP_LIMITS[v.finetune as usize & 15];
        if up {
            v.pitch -= v.arg as i32;
            v.pitch = v.pitch.max(lim[1] as i32);
        } else {
            v.pitch += v.arg as i32;
            v.pitch = v.pitch.min(lim[0] as i32);
        }
        self.apply_pitch(i, tone, hw);
    }

    /// A voice's cell in a row step.
    fn setup(&mut self, i: usize, a: u32, hw: &mut Hw, mask: &mut u16) {
        let note = hw.bus.r32(a);
        let sample = (note >> 27) as u8;
        let period = (note >> 16) as u16 & 0x7ff;
        let (effect, arg) = ((note >> 8) as u8, note as u8);
        {
            let v = &mut self.voices[i];
            v.sample = sample;
            v.period = period;
            v.effect = effect;
            v.arg = arg;
        }
        // a sample selection and an effect take their time before the voice's
        // writes (measured on rows that write nothing)
        hw.wait(
            if sample != 0 { 146 } else { 0 }
                + match effect {
                    0 => 0,
                    5 => 93,
                    0x14 => 208,
                    0x15 => 243,
                    _ => 168,
                },
        );
        if sample != 0 {
            let info = self.base + 0xa + (sample as u32 - 1) * 0x2e;
            let entry = hw.bus.r32(self.mem + SAMPLES_OFF + 4 * (sample as u32 - 1));
            let addr = entry.wrapping_add(hw.bus.r32(info + 0x1e));
            let v = &mut self.voices[i];
            v.start = addr;
            if addr != 0 {
                v.len = hw.bus.r16(info + 0x22);
                let ll = hw.bus.r16(info + 0x28);
                if ll == 0 {
                    v.loop_start = addr;
                    v.loop_len = 1;
                } else {
                    v.loop_start = addr.wrapping_add(hw.bus.r32(info + 0x24));
                    v.loop_len = ll;
                }
                v.finetune = hw.bus.r8(info + 0x2a);
                v.volume = hw.bus.r8(info + 0x2b);
            }
        }
        if period != 0 {
            if matches!(effect, 0x1b..=0x1d) {
                let end = self.adjust_finetune(period, self.voices[i].finetune).0 as i32;
                let v = &mut self.voices[i];
                v.port_end = end;
                v.port_up = false;
                hw.wait(150);
                if v.port_end == v.pitch {
                    v.port_end = 0;
                } else if v.port_end < v.pitch {
                    v.port_up = true;
                }
                self.set_volume(i, hw);
                return;
            }
            if period == 0x7ff {
                hw.wait(NOTE_ENTRY);
                self.dma(hw, 1 << i);
                self.wait_lines(hw);
                self.vol(hw, i, 0);
                *mask |= 1 << i;
                return;
            }
            match effect {
                0x1a => self.voices[i].finetune = arg & 15,
                0x17 => {
                    hw.wait(NOTE_ENTRY);
                    self.dma(hw, 1 << i);
                    self.wait_lines(hw);
                    self.set_volume(i, hw);
                    return;
                }
                0x19 => {
                    let v = &mut self.voices[i];
                    if arg != 0 {
                        v.offset = (arg as u16) << 7;
                    }
                    if v.offset != 0 {
                        if v.offset >= v.len {
                            v.len = 1;
                        } else {
                            v.len -= v.offset;
                            v.start = v.start.wrapping_add(v.offset as u32 * 2);
                        }
                    }
                }
                _ => {}
            }
            hw.wait(NOTE_ENTRY);
            let v = self.voices[i];
            self.dma(hw, 1 << i);
            self.wait_lines(hw);
            if v.start != 0 {
                self.loc(hw, i, v.start);
                hw.wait(LOC_LEN);
                self.len(hw, i, v.len);
                let (p, extra) = self.adjust_finetune(period, v.finetune);
                hw.wait(LEN_PER + extra);
                self.voices[i].pitch = p as i32;
                self.per(hw, i, p as i32);
                *mask |= 1 << i;
                hw.wait(if effect != 0 { PER_VOL_FX } else { PER_VOL } + if i > 0 { 60 } else { 0 });
            } else {
                self.loc(hw, i, 0);
            }
        }
        match effect {
            4 => {
                let n = 64 - s8(arg as u32);
                if n >= 0 {
                    self.inv = n;
                }
            }
            0xc => {
                if arg != 0 {
                    self.add_volume(i, arg, hw);
                }
            }
            0xd => {
                if arg != 0 {
                    self.sub_volume(i, arg, hw);
                }
            }
            0x10 => {
                if arg != 0 {
                    self.inv = (self.inv - arg as i32).max(0);
                    self.vol_out_all(hw);
                }
            }
            0x11 => {
                if arg != 0 {
                    self.inv = (self.inv + arg as i32).min(64);
                    self.vol_out_all(hw);
                }
            }
            3 => {
                self.voices[i].volume = arg;
                self.set_volume(i, hw);
                return;
            }
            _ => {}
        }
        if period != 0 {
            self.set_volume(i, hw);
        }
        match effect {
            5 => {
                if arg != 0 {
                    self.counter = 0;
                    self.next_retrig = 0;
                    self.arp = 3;
                    self.speed = arg;
                }
            }
            6 => {
                if arg == 0 || arg == 0xff {
                    self.new_position = self.position;
                    self.jump = true;
                } else if arg as u16 <= self.positions(hw) {
                    self.new_position = (arg as u16).wrapping_sub(2);
                    self.jump = true;
                }
            }
            // bset.b #1,$bfe001 for 0 (filter off), bclr for the rest
            7 => {
                let pra = hw.bus.read_byte(0xBFE001);
                hw.bus.write_byte(0xBFE001, if arg == 0 { pra | 2 } else { pra & !2 });
            }
            8 | 9 => {
                if arg != 0 {
                    self.slide(i, effect == 8, false, hw);
                }
            }
            0xa => self.voices[i].tone_slide = arg != 0,
            0xb => {
                if arg >= 28 {
                    self.tempo = arg;
                    self.counter = 0;
                    self.next_retrig = 0;
                    self.arp = 3;
                }
            }
            0x14 => {
                if arg == 0 {
                    self.voices[i].loop_row = self.row;
                }
            }
            0x15 => {
                let v = &mut self.voices[i];
                if arg == 0 {
                    v.loop_row = -1;
                } else if v.loop_row >= 0 {
                    if v.loop_cnt == 0 {
                        v.loop_cnt = arg;
                    } else {
                        v.loop_cnt -= 1;
                        if v.loop_cnt == 0 {
                            v.loop_row = -1;
                            return;
                        }
                    }
                    self.loop_row = v.loop_row - 1;
                    self.set_loop = true;
                    v.loop_row = -1;
                }
            }
            0x1e => self.voices[i].tone_port = arg != 0,
            _ => {}
        }
    }

    /// A tick that is not a row, for voice `i`.
    fn make_effects(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        let (a, e) = (v.arg, v.effect);
        if v.period == 0x7ff {
            return;
        }
        if a != 0 {
            match e {
                0 => {
                    let x = self.arp - self.counter as i32;
                    if x == 0 {
                        self.arp += 3;
                        self.per(hw, i, v.pitch);
                    } else {
                        hw.wait(ARP_OTHER);
                        let off = if x == 1 { a & 15 } else { a >> 4 } as usize;
                        let t = &NP_PERIODS[v.finetune as usize & 15];
                        let k = (0..48).find(|&k| t[k] as i32 == v.pitch).unwrap_or(47);
                        self.per(hw, i, t[(k + off).min(47)] as i32);
                    }
                }
                1 => self.slide(i, true, v.tone_slide, hw),
                2 => self.slide(i, false, v.tone_slide, hw),
                0xe => self.add_volume(i, a, hw),
                0xf => self.sub_volume(i, a, hw),
                0x12 => {
                    self.inv = (self.inv - a as i32).max(0);
                    self.vol_out_all(hw);
                }
                0x13 => {
                    self.inv = (self.inv + a as i32).min(64);
                    self.vol_out_all(hw);
                }
                0x16 => {
                    if s8(a as u32) < s8(self.speed as u32) {
                        if self.next_retrig == 0 {
                            self.next_retrig = a as u32;
                        }
                        if self.counter as u32 == self.next_retrig {
                            self.next_retrig += a as u32;
                            self.play(i, hw);
                        }
                    }
                }
                0x17 => {
                    if s8(a as u32) < s8(self.speed as u32) {
                        if a as u16 == self.counter && (v.sample != 0 || v.period != 0) {
                            if v.period != 0 {
                                let p = self.adjust_finetune(v.period, v.finetune).0;
                                self.voices[i].pitch = p as i32;
                                self.per(hw, i, p as i32);
                            }
                            self.play(i, hw);
                        }
                    } else {
                        self.voices[i].effect = 0;
                        self.voices[i].arg = 0;
                    }
                }
                0x18 => {
                    if s8(a as u32) < s8(self.speed as u32) {
                        if a as u16 == self.counter {
                            self.voices[i].volume = 0;
                            self.vol(hw, i, 0);
                        }
                    } else {
                        self.voices[i].effect = 0;
                        self.voices[i].arg = 0;
                    }
                }
                0x1c => {
                    self.do_portamento(i, hw);
                    self.add_volume(i, a, hw);
                }
                0x1d => {
                    self.do_portamento(i, hw);
                    self.sub_volume(i, a, hw);
                }
                _ => {}
            }
        }
        if e == 0x1b {
            let v = &mut self.voices[i];
            if v.arg != 0 {
                v.port_speed = v.arg as i32;
                v.arg = 0;
            }
            self.do_portamento(i, hw);
        }
    }
}
