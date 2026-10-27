//! PumaTracker, ported from NostalgicPlayer's PumaTracker player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Where the original player's register writes differ from what NostalgicPlayer
//! asks of its mixer, this follows the original (checked against Pumatracker 1.0
//! from Eagleplayer 2.06): the sample pointer and length are written every tick,
//! a stopped one-shot sample as one word; samples play with their stored length;
//! and every tick turns off the DMA of each voice that did not play.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "PumaTracker", detect, new };

#[derive(Clone, Copy, Default)]
struct Cmd {
    cmd: u8,
    a1: u8,
    a2: u8,
    a3: u8,
}

/// The end command a missing or truncated command list plays as.
const END: Cmd = Cmd { cmd: 0xe0, a1: 0, a2: 0, a3: 0 };

#[derive(Default)]
struct Instrument {
    volume: Vec<Cmd>,
    frequency: Vec<Cmd>,
}

#[derive(Clone, Copy)]
struct Row {
    note: u8,
    instrument: u8,
    effect: u8,
    arg: u8,
    wait: u8,
}

const EMPTY_ROW: Row = Row { note: 0, instrument: 0, effect: 0, arg: 0, wait: 32 };

#[derive(Clone, Copy, Default)]
struct VoicePosition {
    track: u8,
    instrument_transpose: i8,
    note_transpose: i8,
}

struct Position {
    voices: [VoicePosition; 4],
    speed: u8,
}

const SET_VOLUME_SLIDE: u8 = 0x01;
const SET_FREQUENCY: u8 = 0x02;
const VOICE_RUNNING: u8 = 0x04;
const NO_LOOP: u8 = 0x08;

#[derive(Default)]
struct Voice {
    /// Index into `tracks`, or None for the empty track.
    track: Option<usize>,
    track_position: i32,
    row_counter: u8,
    /// Index into `instruments`, or None for the empty instrument.
    instrument: Option<usize>,
    volume_position: u8,
    frequency_position: u8,
    instrument_transpose: i8,
    note_transpose: i8,
    transposed_note: u8,
    /// Index into the sample list (10 samples, then the waveforms).
    sample: Option<usize>,
    sample_length: u16,
    sample_number: u8,
    period: u16,
    volume: u8,
    portamento: i16,
    flags: u8,
    slide_counter: u8,
    slide_remaining: u8,
    slide_delta: i8,
    slide_direction: i8,
    slide_value: i16,
    frequency_counter: u8,
    vary_add: i32,
    vary_value: i32,
    waveform_counter: u8,
    waveform_add: i8,
    waveform_value: i8,
}

pub struct PumaTracker {
    title: String,
    positions: Vec<Position>,
    tracks: Vec<Vec<Row>>,
    instruments: Vec<Instrument>,
    /// (guest address, length in bytes) of the 10 samples and then the waveforms.
    samples: Vec<Option<(u32, u16)>>,
    speed: u8,
    speed_counter: u8,
    position: i32,
    row: u8,
    voices: [Voice; 4],
    songend: bool,
}

fn detect(d: &[u8]) -> bool {
    if d.len() < 80 {
        return false;
    }
    let offset = (be16(d, 12) as usize + 1) * 14 + 80;
    d.get(offset..offset + 4) == Some(b"patt")
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let title: String = d[..12].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
    let num_positions = be16(d, 12) as usize + 1;
    let num_tracks = be16(d, 14) as usize;
    let num_instruments = be16(d, 16) as usize;
    let offsets: Vec<u32> = (0..10).map(|i| be32(d, 20 + 4 * i)).collect();
    let lengths: Vec<u16> = (0..10).map(|i| be16(d, 60 + 2 * i)).collect();
    let mut p = 80;

    let mut positions = Vec::new();
    for _ in 0..num_positions {
        let b = d.get(p..p + 14)?;
        let mut voices = [VoicePosition::default(); 4];
        for (j, v) in voices.iter_mut().enumerate() {
            *v = VoicePosition {
                track: b[3 * j],
                instrument_transpose: b[3 * j + 1] as i8,
                note_transpose: b[3 * j + 2] as i8,
            };
        }
        positions.push(Position { voices, speed: b[12] });
        p += 14;
    }

    let mut tracks = Vec::new();
    for _ in 0..num_tracks {
        if d.get(p..p + 4)? != b"patt" {
            return None;
        }
        p += 4;
        let mut track = Vec::new();
        let mut row = 0u32;
        while row < 32 {
            let b = d.get(p..p + 4)?;
            track.push(Row { note: b[0], instrument: b[1] & 0x1f, effect: (b[1] & 0xe0) >> 5, arg: b[2], wait: b[3] });
            row += b[3] as u32;
            p += 4;
        }
        tracks.push(track);
    }
    if d.get(p..p + 4)? != b"patt" {
        return None;
    }
    p += 4;

    let mut instruments = Vec::new();
    for _ in 0..num_instruments {
        if d.get(p..p + 4)? != b"inst" {
            return None;
        }
        p += 4;
        let volume = commands(d, &mut p)?;
        if d.get(p..p + 4)? != b"insf" {
            return None;
        }
        p += 4;
        let frequency = commands(d, &mut p)?;
        instruments.push(Instrument { volume, frequency });
    }

    let mut samples = Vec::new();
    for i in 0..10 {
        if lengths[i] == 0 {
            samples.push(None);
            continue;
        }
        // NostalgicPlayer takes the stored length as one word too many; the original
        // player plays it as stored
        let len = lengths[i].wrapping_mul(2);
        if (offsets[i] as usize + len as usize) > d.len() + 50 {
            return None;
        }
        samples.push(Some((base + offsets[i], len)));
    }
    let wave = m.alloc((WAVEFORMS.len() * 32) as u32);
    if wave == 0 {
        return None;
    }
    for (i, w) in WAVEFORMS.iter().enumerate() {
        let a = wave + 32 * i as u32;
        m.bus.write_bytes(a, w);
        samples.push(Some((a, 32)));
    }

    Some(Box::new(PumaTracker {
        title,
        positions,
        tracks,
        instruments,
        samples,
        speed: 3,
        // NostalgicPlayer counts the first two ticks down silently; the original
        // plays the first row on its first interrupt
        speed_counter: 1,
        position: -1,
        row: 32,
        voices: Default::default(),
        songend: false,
    }))
}

/// An instrument's command list, up to its end or goto command.
fn commands(d: &[u8], p: &mut usize) -> Option<Vec<Cmd>> {
    let mut list = Vec::new();
    loop {
        let b = d.get(*p..*p + 4)?;
        // some modules (Vimto) leave out the end command before the next instrument
        let cmd = if b == b"inst" {
            END
        } else {
            *p += 4;
            Cmd { cmd: b[0], a1: b[1], a2: b[2], a3: b[3] }
        };
        list.push(cmd);
        if cmd.cmd == 0xe0 || cmd.cmd == 0xb0 {
            return Some(list);
        }
    }
}

impl Replayer for PumaTracker {
    fn format(&self) -> &'static str {
        "PumaTracker"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.row == 32 {
            self.position += 1;
            if self.position as usize == self.positions.len() {
                self.position = 0;
                self.songend = true;
            }
            self.next_position();
        }

        self.speed_counter = self.speed_counter.wrapping_sub(1);
        if self.speed_counter == 0 {
            self.row += 1;
            self.speed_counter = self.speed;
            for i in 0..4 {
                let v = &mut self.voices[i];
                v.row_counter = v.row_counter.wrapping_sub(1);
                if v.row_counter == 0 {
                    v.track_position += 1;
                    self.next_row(i, hw);
                }
            }
        }

        let mut on = 0u16;
        for i in 0..4 {
            if self.voices[i].flags & VOICE_RUNNING != 0 {
                self.volume_commands(i);
                self.frequency_commands(i);
                if self.setup_hardware(i, hw) {
                    on |= 1 << i;
                }
            }
        }
        // every voice that did not play this tick goes off, even one never started
        hw.dmacon(!on & 0xF);
        hw.dmacon(0x8000 | on);
    }
}

impl PumaTracker {
    fn next_position(&mut self) {
        self.row = 0;
        let pos = &self.positions[self.position as usize];
        for (v, vp) in self.voices.iter_mut().zip(&pos.voices) {
            v.track = ((vp.track as usize) < self.tracks.len()).then_some(vp.track as usize);
            v.track_position = -1;
            v.instrument_transpose = vp.instrument_transpose;
            v.note_transpose = vp.note_transpose;
            v.row_counter = 1;
        }
        if pos.speed != 0 {
            self.speed = pos.speed;
        }
    }

    fn row(&self, i: usize) -> Row {
        let v = &self.voices[i];
        v.track
            .and_then(|t| self.tracks[t].get(v.track_position as usize))
            .copied()
            .unwrap_or(EMPTY_ROW)
    }

    fn next_row(&mut self, i: usize, hw: &mut Hw) {
        let row = self.row(i);
        let num_instruments = self.instruments.len();
        let v = &mut self.voices[i];
        if row.note != 0 {
            hw.dmacon(1 << i);
            v.transposed_note = row.note.wrapping_add(v.note_transpose as u8);
            let n = row.instrument as i32 + v.instrument_transpose as i32;
            v.instrument = (n >= 1 && (n as usize) <= num_instruments).then(|| n as usize - 1);
            v.flags = SET_VOLUME_SLIDE | SET_FREQUENCY | VOICE_RUNNING;
            v.volume_position = 0;
            v.frequency_position = 0;
        }
        v.row_counter = row.wait;
        let (volume, portamento) = match row.effect {
            1 => (row.arg, 0),
            2 => (64, row.arg as i16),
            3 => (64, -(row.arg as i16)),
            _ => (64, 0),
        };
        v.volume = volume;
        v.portamento = portamento;
    }

    fn command(list: Option<&Vec<Cmd>>, pos: u8) -> Cmd {
        list.and_then(|l| l.get(pos as usize)).copied().unwrap_or(END)
    }

    fn volume_commands(&mut self, i: usize) {
        loop {
            let v = &self.voices[i];
            let list = v.instrument.map(|n| &self.instruments[n].volume);
            let c = Self::command(list, v.volume_position);
            let more = match c.cmd {
                0xa0 => self.volume_slide(i, c),
                0xc0 => {
                    self.set_sample(i, c);
                    true
                }
                0xb0 => {
                    self.voices[i].volume_position = c.a1 / 4;
                    true
                }
                _ => {
                    let v = &mut self.voices[i];
                    v.slide_value = 0;
                    v.flags &= !VOICE_RUNNING;
                    false
                }
            };
            if !more {
                break;
            }
        }
    }

    fn volume_slide(&mut self, i: usize, c: Cmd) -> bool {
        let v = &mut self.voices[i];
        let first = v.flags & SET_VOLUME_SLIDE != 0;
        v.flags &= !SET_VOLUME_SLIDE;
        if first {
            v.slide_counter = c.a3.wrapping_add(1);
            v.slide_value = c.a1 as i16;
            v.slide_remaining = 0;
            v.slide_direction = 1;
            let mut delta = c.a2 as i32 - c.a1 as i32;
            if delta < 0 {
                delta = -delta;
                v.slide_direction = -1;
            }
            v.slide_delta = delta as i8;
            return false;
        }
        v.slide_counter = v.slide_counter.wrapping_sub(1);
        if v.slide_counter == 0 {
            v.volume_position = v.volume_position.wrapping_add(1);
            v.flags |= SET_VOLUME_SLIDE;
            return true;
        }
        let time = c.a3 as i32;
        let mut count = v.slide_remaining as i32 + v.slide_delta as i32 - time;
        if count >= 0 {
            let mut add: i16 = 0;
            loop {
                add = add.wrapping_add(v.slide_direction as i16);
                count -= time;
                // a time of 0 would never end
                if count < 0 || time == 0 {
                    break;
                }
            }
            v.slide_value = v.slide_value.wrapping_add(add);
        }
        count += time;
        v.slide_remaining = count as u8;
        false
    }

    fn set_sample(&mut self, i: usize, c: Cmd) {
        let s = self.samples.get(c.a1 as usize).copied().flatten();
        let v = &mut self.voices[i];
        v.waveform_add = c.a2 as i8;
        v.waveform_value = c.a2 as i8;
        v.waveform_counter = c.a3;
        v.sample_length = s.map_or(0, |(_, len)| len);
        if v.sample_length >= 80 * 2 {
            v.flags |= NO_LOOP;
        }
        v.sample = s.map(|_| c.a1 as usize);
        v.sample_number = c.a1;
        v.volume_position = v.volume_position.wrapping_add(1);
        v.flags |= SET_VOLUME_SLIDE;
    }

    fn frequency_commands(&mut self, i: usize) {
        loop {
            let v = &self.voices[i];
            let list = v.instrument.map(|n| &self.instruments[n].frequency);
            // a missing frequency list does nothing (the end command is not one of its commands)
            let Some(c) = list.and_then(|l| l.get(v.frequency_position as usize)).copied() else { break };
            let more = match c.cmd {
                0xa0 => self.vary_frequency(i, c),
                0xd0 => self.constant_frequency(i, c),
                0xb0 => {
                    self.voices[i].frequency_position = c.a1 / 4;
                    true
                }
                _ => false,
            };
            if !more {
                break;
            }
        }
    }

    /// A note past the period table makes the original read its own variables
    /// there, a long period that is near silence (NostalgicPlayer clamps to the last
    /// note instead, an audible one). Period 0 is Paula's slowest: silent too.
    fn period(index: i32) -> u16 {
        usize::try_from(index).ok().and_then(|i| PERIODS.get(i)).copied().unwrap_or(0)
    }

    fn vary_frequency(&mut self, i: usize, c: Cmd) -> bool {
        let v = &mut self.voices[i];
        let first = v.flags & SET_FREQUENCY != 0;
        v.flags &= !SET_FREQUENCY;
        if first {
            v.frequency_counter = c.a3;
            let period = Self::period(v.transposed_note as i32 / 2) as i16;
            let begin = (c.a1 as i8 as i16).wrapping_add(period);
            let end = (c.a2 as i8 as i16).wrapping_add(period);
            v.period = begin as u16;
            v.vary_value = (begin as i32) << 16;
            let delta = end.wrapping_sub(begin) as i32;
            let counter = (v.frequency_counter as i32).max(1);
            let add = delta * 256 / counter;
            v.vary_add = if add < 65536 { add << 8 } else { (delta / counter) << 16 };
            return false;
        }
        v.frequency_counter = v.frequency_counter.wrapping_sub(1);
        if v.frequency_counter == 0 {
            v.frequency_position = v.frequency_position.wrapping_add(1);
            v.flags |= SET_FREQUENCY;
            return true;
        }
        v.vary_value = v.vary_value.wrapping_add(v.vary_add);
        v.period = (v.vary_value >> 16) as u16;
        false
    }

    fn constant_frequency(&mut self, i: usize, c: Cmd) -> bool {
        let v = &mut self.voices[i];
        let first = v.flags & SET_FREQUENCY != 0;
        v.flags &= !SET_FREQUENCY;
        if first {
            v.frequency_counter = c.a3;
            v.period = Self::period((c.a1 as i8 as i32 + v.transposed_note as i32) / 2);
            return false;
        }
        v.frequency_counter = v.frequency_counter.wrapping_sub(1);
        if v.frequency_counter == 0 {
            v.flags |= SET_FREQUENCY;
            v.frequency_position = v.frequency_position.wrapping_add(1);
            return true;
        }
        false
    }

    /// Writes the voice's registers. Returns whether it plays (else its DMA goes off).
    fn setup_hardware(&mut self, i: usize, hw: &mut Hw) -> bool {
        let v = &mut self.voices[i];
        v.period = v.period.wrapping_add(v.portamento as u16);
        let mut sample = v.sample;
        if v.waveform_add != 0 {
            if v.waveform_value <= 0 || v.waveform_value as i32 >= v.waveform_counter as i32 {
                v.waveform_add = v.waveform_add.wrapping_neg();
            }
            v.waveform_value = v.waveform_value.wrapping_add(v.waveform_add);
            let n = v.sample_number as i32 + v.waveform_value as i32;
            sample = (n >= 0).then_some(n as usize).filter(|&n| self.samples.get(n).copied().flatten().is_some());
        }
        let Some(addr) = sample.and_then(|n| self.samples[n]).map(|(a, _)| a) else { return false };
        hw.loc(i, addr);
        hw.len(i, v.sample_length / 2);
        if v.sample_length > 2 {
            if v.flags & NO_LOOP != 0 {
                v.flags &= !NO_LOOP;
                v.sample_length = 2;
            }
        }
        hw.per(i, v.period);
        let volume = (v.slide_value as i32 + v.volume as i32 - 64).max(0);
        hw.vol(i, volume as u16);
        true
    }
}

include!("pumatracker_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One position, one track with one note, one instrument playing the first
    /// waveform at a constant note.
    fn module() -> Vec<u8> {
        let mut d = b"test        ".to_vec();
        d.extend([0, 0, 0, 1, 0, 1, 0, 0]); // 1 position, 1 track, 1 instrument
        d.resize(80, 0); // no samples
        d.extend([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // all voices on track 0
        d.extend(b"patt");
        d.extend([24, 1, 0, 32]); // note 24, instrument 1, for 32 rows
        d.extend(b"patt");
        d.extend(b"inst");
        d.extend([0xc0, 10, 0, 0, 0xa0, 64, 64, 100, 0xe0, 0, 0, 0]); // waveform 0, volume 64
        d.extend(b"insf");
        d.extend([0xd0, 0, 0, 100, 0xe0, 0, 0, 0]);
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.puma"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "PumaTracker");
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // every voice: the note's DMA off, then the waveform at period PERIODS[12], volume 64
        assert_eq!(reg(0xDFF0A6)[0], super::PERIODS[12]);
        assert_eq!(reg(0xDFF0A4)[0], 16);
        assert_eq!(reg(0xDFF0A8)[0], 64);
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..6], &[0x0001, 0x0002, 0x0004, 0x0008, 0x0000, 0x800F]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}
