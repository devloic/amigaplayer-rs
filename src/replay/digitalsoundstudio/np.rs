//! The newer Digital Sound Studio the Eagleplayer predates: a module whose
//! tempo byte (offset 8) is not 0. Ported from NostalgicPlayer's Digital Sound
//! Studio player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer) and written to Paula through `Hw` as the rest of the
//! port is, but not timed to the Eagleplayer's code paths (there are none to
//! follow: it has no such effects). The tempo is a BPM tempo, the speed is byte 9,
//! the finetune and the 31 effects are NostalgicPlayer's.
//!
//! NostalgicPlayer's mixer plays a sample for `length + loop length` words and
//! loops the tail; here the sample's `length` words are played and the loop
//! registers written after the row's DMA, as the Eagleplayer does. A sample
//! without a loop is played once: its loop is a word of silence. The filter
//! effect (7) has no Paula register here and is ignored; arpeggio is
//! NostalgicPlayer's, which looks the note up but never adds the offset (so it
//! only rewrites the current period).

use super::{SAMPLES_OFF, MIRROR_SIZE};
use crate::replay::Hw;
use crate::paula::LINE;

include!("np_tables.rs");

#[derive(Clone, Copy, Default)]
struct Voice {
    sample: u8,
    period: u16,
    effect: u8,
    arg: u8,
    finetune: u8,
    volume: u8,
    has_data: bool,
    start: u32,
    len: u16,
    loop_start: u32,
    loop_len: u16,
    sample_offset: u16,
    pitch: u16,
    port_up: bool,
    port_speed: u8,
    port_end: u16,
    tone_slide: bool,
    tone_port: bool,
    loop_row: i16,
    loop_counter: u16,
}

mod fx {
    pub const ARPEGGIO: u8 = 0x0;
    pub const SLIDE_UP: u8 = 0x1;
    pub const SLIDE_DOWN: u8 = 0x2;
    pub const SET_VOLUME: u8 = 0x3;
    pub const SET_MASTER: u8 = 0x4;
    pub const SET_SPEED: u8 = 0x5;
    pub const JUMP: u8 = 0x6;
    pub const FILTER: u8 = 0x7;
    pub const PITCH_UP: u8 = 0x8;
    pub const PITCH_DOWN: u8 = 0x9;
    pub const PITCH_CONTROL: u8 = 0xa;
    pub const SET_TEMPO: u8 = 0xb;
    pub const VOL_UP: u8 = 0xc;
    pub const VOL_DOWN: u8 = 0xd;
    pub const VOL_SLIDE_UP: u8 = 0xe;
    pub const VOL_SLIDE_DOWN: u8 = 0xf;
    pub const MASTER_UP: u8 = 0x10;
    pub const MASTER_DOWN: u8 = 0x11;
    pub const MASTER_SLIDE_UP: u8 = 0x12;
    pub const MASTER_SLIDE_DOWN: u8 = 0x13;
    pub const LOOP_START: u8 = 0x14;
    pub const LOOP_JUMP: u8 = 0x15;
    pub const RETRIG: u8 = 0x16;
    pub const DELAY: u8 = 0x17;
    pub const CUT: u8 = 0x18;
    pub const SAMPLE_OFFSET: u8 = 0x19;
    pub const FINETUNE: u8 = 0x1a;
    pub const PORTAMENTO: u8 = 0x1b;
    pub const PORTA_VOL_UP: u8 = 0x1c;
    pub const PORTA_VOL_DOWN: u8 = 0x1d;
    pub const PORTA_CONTROL: u8 = 0x1e;
}
use fx::*;

pub struct Np {
    base: u32,
    mem: u32,
    voices: [Voice; 4],
    tempo: u8,
    speed: u8,
    counter: u8,
    position: u16,
    row: i16,
    jump: bool,
    new_position: u16,
    set_loop_row: bool,
    loop_row: i16,
    inv_master: u16,
    next_retrig: u16,
    arp_counter: u16,
    visited: Vec<bool>,
    pub songend: bool,
}

impl Np {
    pub fn new(d: &[u8], base: u32, mem: u32) -> Np {
        let positions = u16::from_be_bytes([d[0x59c], d[0x59d]]) as usize;
        let mut visited = vec![false; positions.max(1)];
        visited[0] = true;
        Np {
            base,
            mem,
            voices: [Voice::default(); 4],
            tempo: d[8],
            speed: d[9],
            counter: 0,
            position: 0,
            row: 0,
            jump: false,
            new_position: 0,
            set_loop_row: false,
            loop_row: 0,
            inv_master: 0,
            next_retrig: 0,
            arp_counter: 3,
            visited,
            songend: false,
        }
    }

    /// Colour clocks of the CIA timer at the BPM tempo (ProTracker's
    /// 1773447 / BPM, plus one, in E-clock ticks of 5 colour clocks).
    pub fn tick_clocks(&self) -> u64 {
        (1_773_447 / self.tempo.max(1) as u64 + 1) * 5
    }

    fn positions(&self, hw: &Hw) -> u16 {
        hw.bus.r16(self.base + 0x59c)
    }

    pub fn tick(&mut self, hw: &mut Hw) {
        self.counter = self.counter.wrapping_add(1);
        if self.counter == self.speed {
            self.counter = 0;
            self.next_retrig = 0;
            self.arp_counter = 3;
            self.row(hw);
        } else {
            for i in 0..4 {
                if self.voices[i].period != 0 {
                    self.make_effects(i, hw);
                }
            }
        }
    }

    fn row(&mut self, hw: &mut Hw) {
        self.new_position = self.position;
        let pat = hw.bus.r8(self.base + 0x59e + self.position as u32) as u32;
        let mut mask = 0u16;
        for i in 0..4 {
            let a = self.base + 0x61e + (pat << 10) + 16 * self.row as u32 + 4 * i as u32;
            self.setup(i, a, hw, &mut mask);
        }
        if mask != 0 {
            hw.dmacon(0x8000 | mask);
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
        }
        for i in 0..4 {
            let v = self.voices[i];
            if v.effect != DELAY && (v.loop_len != 0 || mask & (1 << i) != 0) {
                self.write_loop(i, hw);
            }
        }
        if self.set_loop_row {
            self.row = self.loop_row;
            self.set_loop_row = false;
        }
        self.row += 1;
        if self.row >= 64 || self.jump {
            self.jump = false;
            self.row = 0;
            for v in &mut self.voices {
                v.loop_row = -1;
            }
            self.position = self.new_position.wrapping_add(1);
            if self.position == self.positions(hw) {
                self.position = 0;
            }
            let p = self.position as usize;
            if p < self.visited.len() {
                if self.visited[p] {
                    self.songend = true;
                }
                self.visited[p] = true;
            }
        }
    }

    fn write_loop(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        if v.loop_len != 0 {
            hw.loc(i, v.loop_start);
            hw.len(i, v.loop_len);
        } else {
            hw.loc(i, self.mem + MIRROR_SIZE);
            hw.len(i, 1);
        }
    }

    /// Starts the voice's sample (DMA off, its start and length) without turning
    /// its DMA on.
    fn begin(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        hw.dmacon(1 << i);
        hw.loc(i, v.start);
        hw.len(i, v.len);
    }

    /// NostalgicPlayer's PlaySample, for a retrigger or a delayed note.
    fn play(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        if v.has_data && v.len > 0 {
            self.begin(i, hw);
            hw.dmacon(0x8000 | 1 << i);
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
            self.write_loop(i, hw);
        }
    }

    fn mute(&mut self, i: usize, hw: &mut Hw, mask: Option<&mut u16>) {
        hw.dmacon(1 << i);
        if let Some(m) = mask {
            *m &= !(1 << i);
        }
    }

    fn set_volume(&mut self, i: usize, hw: &mut Hw) {
        let v = (self.voices[i].volume as i32 - self.inv_master as i32).max(0);
        hw.vol(i, v as u16);
    }

    fn set_volume_all(&mut self, hw: &mut Hw) {
        for i in 0..4 {
            self.set_volume(i, hw);
        }
    }

    fn add_volume(&mut self, i: usize, add: u8, hw: &mut Hw) {
        let v = &mut self.voices[i];
        v.volume = v.volume.wrapping_add(add);
        if v.volume > 64 {
            v.volume = 64;
        }
        self.set_volume(i, hw);
    }

    fn sub_volume(&mut self, i: usize, sub: u8, hw: &mut Hw) {
        let v = &mut self.voices[i];
        v.volume = (v.volume as i32 - sub as i32).max(0) as u8;
        self.set_volume(i, hw);
    }

    fn add_master(&mut self, add: u8, hw: &mut Hw) {
        self.inv_master = (self.inv_master as i32 - add as i32).max(0) as u16;
        self.set_volume_all(hw);
    }

    fn sub_master(&mut self, sub: u8, hw: &mut Hw) {
        self.inv_master = (self.inv_master + sub as u16).min(64);
        self.set_volume_all(hw);
    }

    fn adjust_finetune(&self, period: u16, ft: u8) -> u16 {
        if ft == 0 {
            return period;
        }
        let mut i = 0;
        while i < 48 {
            if NP_PERIODS[0][i] == period {
                i += 1;
                break;
            }
            i += 1;
        }
        NP_PERIODS[ft as usize & 15][i - 1]
    }

    fn adjust_tone(&self, period: u16, ft: u8) -> u16 {
        let mut i = 0;
        while i < 48 {
            if period >= NP_PERIODS[ft as usize & 15][i] {
                i += 1;
                break;
            }
            i += 1;
        }
        NP_PERIODS[ft as usize & 15][i - 1]
    }

    fn apply_pitch(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        let p = if v.tone_slide { self.adjust_tone(v.pitch, v.finetune) } else { v.pitch };
        hw.per(i, p);
    }

    fn do_portamento(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        if v.port_end == 0 {
            return;
        }
        if v.port_up {
            v.pitch = v.pitch.wrapping_sub(v.port_speed as u16);
            if v.pitch <= v.port_end {
                v.pitch = v.port_end;
                v.port_end = 0;
            }
        } else {
            v.pitch = v.pitch.wrapping_add(v.port_speed as u16);
            if v.pitch >= v.port_end {
                v.pitch = v.port_end;
                v.port_end = 0;
            }
        }
        let v = self.voices[i];
        let p = if v.tone_port { self.adjust_tone(v.pitch, v.finetune) } else { v.pitch };
        hw.per(i, p);
    }

    /// NostalgicPlayer's SetupInstrument: the row's note and its row effects.
    fn setup(&mut self, i: usize, a: u32, hw: &mut Hw, mask: &mut u16) {
        let note = hw.bus.r32(a);
        let b = self.base;
        {
            let v = &mut self.voices[i];
            v.sample = (note >> 27) as u8;
            v.period = (note >> 16) as u16 & 0x7ff;
            v.effect = (note >> 8) as u8;
            v.arg = note as u8;
        }
        let sample = self.voices[i].sample;
        if sample != 0 {
            let idx = (sample - 1) as u32;
            let info = b + 0xa + idx * 0x2e;
            let entry = hw.bus.r32(self.mem + SAMPLES_OFF + 4 * idx);
            let v = &mut self.voices[i];
            v.has_data = entry != 0;
            v.loop_start = 0;
            v.loop_len = 0;
            if v.has_data {
                let off = hw.bus.r32(info + 0x1e) & !1;
                v.start = entry.wrapping_add(off);
                v.len = hw.bus.r16(info + 0x22);
                let ll = hw.bus.r16(info + 0x28);
                if ll > 1 {
                    v.loop_start = v.start.wrapping_add(hw.bus.r32(info + 0x24));
                    v.loop_len = ll;
                }
                v.finetune = hw.bus.r8(info + 0x2a);
                v.volume = hw.bus.r8(info + 0x2b);
            }
        }
        let v = self.voices[i];
        let period = v.period;
        if sample != 0 && period != 0 {
            if matches!(v.effect, PORTAMENTO | PORTA_VOL_UP | PORTA_VOL_DOWN) {
                let end = self.adjust_finetune(period, v.finetune);
                let v = &mut self.voices[i];
                v.port_end = end;
                v.port_up = false;
                if v.port_end == v.pitch {
                    v.port_end = 0;
                } else if v.port_end < v.pitch {
                    v.port_up = true;
                }
                self.set_volume(i, hw);
                return;
            }
            if period == 0x7ff {
                self.mute(i, hw, Some(mask));
                return;
            }
            match v.effect {
                FINETUNE => self.voices[i].finetune = v.arg & 15,
                DELAY => {
                    self.set_volume(i, hw);
                    return;
                }
                SAMPLE_OFFSET => {
                    let v = &mut self.voices[i];
                    if v.arg != 0 {
                        v.sample_offset = (v.arg as u16) << 7;
                    }
                    if v.sample_offset != 0 {
                        if v.sample_offset >= v.len {
                            v.len = 0;
                        } else {
                            v.len -= v.sample_offset;
                            v.start = v.start.wrapping_add(v.sample_offset as u32 * 2);
                        }
                    }
                }
                _ => {}
            }
            let v = self.voices[i];
            if v.has_data && v.len > 0 {
                self.begin(i, hw);
                *mask |= 1 << i;
                let p = self.adjust_finetune(period, v.finetune);
                self.voices[i].pitch = p;
                hw.per(i, p);
            } else {
                self.mute(i, hw, Some(mask));
            }
        }
        let v = self.voices[i];
        let arg = v.arg;
        match v.effect {
            SET_MASTER => {
                let n = 64 - arg as i32;
                if n >= 0 {
                    self.inv_master = n as u16;
                }
            }
            VOL_UP => {
                if arg != 0 {
                    self.add_volume(i, arg, hw);
                }
            }
            VOL_DOWN => {
                if arg != 0 {
                    self.sub_volume(i, arg, hw);
                }
            }
            MASTER_UP => {
                if arg != 0 {
                    self.add_master(arg, hw);
                }
            }
            MASTER_DOWN => {
                if arg != 0 {
                    self.sub_master(arg, hw);
                }
            }
            SET_VOLUME => {
                self.voices[i].volume = arg;
                self.set_volume(i, hw);
                return;
            }
            _ => {}
        }
        if period != 0 {
            self.set_volume(i, hw);
        }
        match v.effect {
            SET_SPEED => {
                if arg != 0 {
                    self.counter = 0;
                    self.next_retrig = 0;
                    self.arp_counter = 3;
                    self.speed = arg;
                }
            }
            JUMP => {
                if arg == 0xff || arg == 0 {
                    self.new_position = self.position;
                    self.jump = true;
                } else if arg as u16 <= self.positions(hw) {
                    self.new_position = (arg as u16).wrapping_sub(2);
                    self.jump = true;
                }
            }
            FILTER => {}
            PITCH_UP | PITCH_DOWN => {
                if arg != 0 {
                    let keep = self.voices[i].tone_slide;
                    self.voices[i].tone_slide = false;
                    self.slide(i, v.effect == PITCH_UP, hw);
                    self.voices[i].tone_slide = keep;
                }
            }
            PITCH_CONTROL => self.voices[i].tone_slide = arg != 0,
            SET_TEMPO => {
                if arg >= 28 {
                    self.tempo = arg;
                    self.counter = 0;
                    self.next_retrig = 0;
                    self.arp_counter = 3;
                }
            }
            LOOP_START => {
                if arg != 0 {
                    self.voices[i].loop_row = self.row;
                }
            }
            LOOP_JUMP => {
                let vv = &mut self.voices[i];
                if arg == 0 {
                    vv.loop_row = -1;
                } else if vv.loop_row >= 0 {
                    if vv.loop_counter == 0 {
                        vv.loop_counter = arg as u16;
                    } else {
                        vv.loop_counter -= 1;
                        if vv.loop_counter == 0 {
                            vv.loop_row = -1;
                            return;
                        }
                    }
                    self.loop_row = vv.loop_row - 1;
                    self.set_loop_row = true;
                    vv.loop_row = -1;
                }
            }
            PORTA_CONTROL => self.voices[i].tone_port = arg != 0,
            _ => {}
        }
    }

    fn slide(&mut self, i: usize, up: bool, hw: &mut Hw) {
        let v = &mut self.voices[i];
        let lim = NP_LIMITS[v.finetune as usize & 15];
        if up {
            v.pitch = v.pitch.wrapping_sub(v.arg as u16);
            if v.pitch < lim[1] {
                v.pitch = lim[1];
            }
        } else {
            v.pitch = v.pitch.wrapping_add(v.arg as u16);
            if v.pitch > lim[0] {
                v.pitch = lim[0];
            }
        }
        self.apply_pitch(i, hw);
    }

    /// NostalgicPlayer's MakeEffects: a tick that is not a row.
    fn make_effects(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        let arg = v.arg;
        if arg != 0 {
            match v.effect {
                ARPEGGIO => {
                    let a = self.arp_counter as i32 - self.counter as i32;
                    if a == 0 {
                        self.arp_counter += 3;
                        hw.per(i, v.pitch);
                    } else {
                        // looks the current period up, the offset never added
                        let t = &NP_PERIODS[v.finetune as usize & 15];
                        let low = NP_LIMITS[v.finetune as usize & 15][1];
                        if let Some(&p) = t.iter().find(|&&p| p == low || p == v.pitch) {
                            hw.per(i, p);
                        }
                    }
                }
                SLIDE_UP => self.slide(i, true, hw),
                SLIDE_DOWN => self.slide(i, false, hw),
                VOL_SLIDE_UP => self.add_volume(i, arg, hw),
                VOL_SLIDE_DOWN => self.sub_volume(i, arg, hw),
                MASTER_SLIDE_UP => self.add_master(arg, hw),
                MASTER_SLIDE_DOWN => self.sub_master(arg, hw),
                RETRIG => {
                    if (arg as u16) < self.speed as u16 {
                        if self.next_retrig == 0 {
                            self.next_retrig = arg as u16;
                        }
                        if self.counter as u16 == self.next_retrig {
                            self.next_retrig += arg as u16;
                            self.play(i, hw);
                        }
                    }
                }
                DELAY => {
                    if arg < self.speed {
                        if arg == self.counter && v.sample != 0 {
                            if v.period != 0 {
                                let p = self.adjust_finetune(v.period, v.finetune);
                                self.voices[i].pitch = p;
                                hw.per(i, p);
                            }
                            self.play(i, hw);
                        }
                    } else {
                        self.voices[i].effect = 0;
                        self.voices[i].arg = 0;
                    }
                }
                CUT => {
                    if arg < self.speed {
                        if arg == self.counter {
                            self.voices[i].volume = 0;
                            self.mute(i, hw, None);
                        }
                    } else {
                        self.voices[i].effect = 0;
                        self.voices[i].arg = 0;
                    }
                }
                PORTA_VOL_UP => {
                    self.do_portamento(i, hw);
                    self.add_volume(i, arg, hw);
                }
                PORTA_VOL_DOWN => {
                    self.do_portamento(i, hw);
                    self.sub_volume(i, arg, hw);
                }
                _ => {}
            }
        }
        if v.effect == PORTAMENTO {
            let vv = &mut self.voices[i];
            if vv.arg != 0 {
                vv.port_speed = vv.arg;
                vv.arg = 0;
            }
            self.do_portamento(i, hw);
        }
    }
}
