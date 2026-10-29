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
use crate::amplifier::aus;
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
const X_STOP: u8 = 0x40;
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
    let (version, channels, packed) = (d[24], d[25] as usize, d[26] != 0);
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
        s.fine_tune = d[579 + i] as i8;
        if (0x10..=0x13).contains(&version) {
            s.fine_tune = 0;
        }
        let (mut ls, mut ll) = (ls, ll);
        if ls > s.length || ll == 0 {
            ls = 0;
            ll = 0;
        } else if ls + ll >= s.length {
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
        if s.length != 0 {
            // a file cut short plays on into whatever follows it in memory, as the original's
            s.data = Some(p);
            p += s.length as usize;
        }
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
    porta_up_old: u8,
    porta_down_old: u8,
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
    twice: bool,
    /// The colour clock at which a sample with no loop has played out.
    ends_at: u64,
    looped: bool,
}

pub struct DigiBooster {
    song: Song,
    base: u32,
    cia_tempo: u16,
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
    title: String,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let song = load(d)?;
    let structs = m.alloc(8 * aus::SIZE);
    if structs == 0 {
        return None;
    }
    // left, left, right, right, right, right, left, left
    for v in 0..8u32 {
        let pan: u16 = if matches!(v, 0 | 1 | 6 | 7) { 1 } else { 0xFFFF };
        m.bus.w16(structs + v * aus::SIZE + aus::PAN, pan);
    }
    use crate::eagle::epamb;
    if !crate::amplifier::start(m, 8, structs, epamb::DIRECT | epamb::BIT8 | epamb::CHIP_RAM) {
        return None;
    }
    let channels = song.channels.clamp(1, 8);
    let title = song.name.clone();
    let mut o = DigiBooster {
        base,
        cia_tempo: 125,
        tempo: 6,
        count: 2,
        song_pos: 0,
        pattern_pos: 0,
        row: [TrackLine::default(); 8],
        pause_vbl: 0,
        pause_enabled: false,
        chans: vec![Chan::default(); channels],
        visited: [false; 256],
        songend: false,
        row_out: false,
        title,
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
        (1_773_447 / self.cia_tempo.max(1) as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.play(hw);
    }
}

impl DigiBooster {
    fn play(&mut self, hw: &mut Hw) {
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
        let mut dma_on = 0u32;
        for v in 0..n {
            self.play_voice(hw, v, &mut dma_on);
        }
        if dma_on != 0 {
            hw.voice_dma(dma_on, true);
        }
        if self.row_out {
            self.write_loops(hw);
        }
        let parsed = self.tempo != 0 && self.count >= self.tempo;
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
    /// Eagleplayer writes: a voice playing a note has its period and volume set
    /// on the tick after its row (and its sample started, if it is a new note),
    /// its volume on every tick, its period on every tick that an effect is on.
    fn play_voice(&mut self, hw: &mut Hw, v: usize, dma_on: &mut u32) {
        let mut c = std::mem::take(&mut self.chans[v]);
        let row_tick = self.row_out;
        // a row with an effect has the effect's code write the period once, in place of the second write
        let row_fx = matches!(c.row.2, E_SONG_REPEAT | E_SET_VOLUME | E_PATTERN_BREAK | E_EXTRA | E_SET_SPEED);
        // E1x and E2x: the period before the slide, and after it
        let slide = c.row.2 == E_EXTRA && matches!(c.row.3 & 0xf0, X_FINE_SLIDE_UP | X_FINE_SLIDE_DOWN);
        if c.main_period != 0 {
            let mut retrig = false;
            if c.off_enable {
                c.off_enable = false;
                retrig = true;
                if c.play_pointer && matches!(c.last.effect, E_GLISSANDO | E_GLISSANDO_VOLSLIDE) {
                    retrig = false;
                }
            }
            if c.main_period == -1 {
                hw.voice_per(v, 0);
                hw.voice_vol(v, 0);
                c.play_pointer = true;
                c.main_period = 0;
            } else {
                let per = c.main_period as u16;
                let vol = c.main_volume.min(64) as u16;
                if retrig {
                    hw.voice_per(v, 0);
                    if let Some(i) = c.sample_data {
                        // Paula plays up to the end of the loop first
                        let smp = &self.song.samples[i];
                        let end = if smp.loop_len > 0 { smp.loop_start + smp.loop_len } else { smp.length };
                        let len = end.saturating_sub(c.start_offset);
                        let loc = self.sample_addr(i, c.start_offset);
                        c.looped = smp.loop_len > 0;
                        c.ends_at = hw.bus.clock + len as u64 * per as u64;
                        hw.voice_dma(1 << v, false);
                        hw.voice_len(v, (len / 2) as u16);
                        hw.voice_loc(v, loc);
                        *dma_on |= 1 << v;
                    }
                    if slide {
                        hw.voice_per(v, c.original_period);
                    }
                    hw.voice_per(v, per);
                    if !row_fx {
                        hw.voice_per(v, per);
                    }
                    c.twice = false;
                } else if row_tick {
                    if !row_fx {
                        hw.voice_per(v, per);
                        if c.twice {
                            hw.voice_per(v, per);
                        }
                    }
                    c.twice = true;
                } else if c.row.2 != 0 {
                    hw.voice_per(v, per);
                }
                hw.voice_vol(v, vol);
                c.play_pointer = true;
            }
        } else {
            if row_tick {
                hw.voice_per(v, 0);
                if !row_fx {
                    hw.voice_per(v, 0);
                }
            } else if c.row.2 != 0 {
                hw.voice_per(v, 0);
            }
            hw.voice_vol(v, 0);
        }
        // whether the voice still plays: the original knows from the end of the sample's first block
        c.is_active = c.main_period > 0 && c.sample_data.is_some() && (c.looped || hw.bus.clock < c.ends_at);
        self.chans[v] = c;
    }

    /// The loops of all voices, written after a row: the sample's loop, or its start with
    /// no length; nothing for a voice with no sample.
    fn write_loops(&self, hw: &mut Hw) {
        for v in 0..8 {
            let (loc, len) = match self.chans.get(v).and_then(|c| c.sample_data.map(|i| (i, c.start_offset))) {
                Some((i, off)) if self.chans[v].main_period != 0 => {
                    let s = &self.song.samples[i];
                    if s.loop_len > 0 {
                        (self.sample_addr(i, s.loop_start), (s.loop_len / 2) as u16)
                    } else {
                        (self.sample_addr(i, off), 0)
                    }
                }
                _ => (0, 0),
            };
            hw.voice_loc(v, loc);
            hw.voice_len(v, len);
        }
    }

    fn parse_voice(&mut self, v: usize, row: TrackLine) {
        let mut c = std::mem::take(&mut self.chans[v]);
        c.volume = c.old_volume;
        if self.tempo != 0 && self.count >= self.tempo {
            self.parse_row_data(&mut c, row);
            self.do_trigger_effects(&mut c);
        }
        if self.tempo.wrapping_sub(1) == self.count {
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
                        c.volume = self.samples_volume(number);
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
        if s.fine_tune == 0 {
            return t.period;
        }
        // a period is stretched by an eighth of a semitone per step of the fine
        // tune, 2^(-ft/96): not by NostalgicPlayer's tables, which round to
        // ProTracker's
        (t.period as f64 * 2f64.powf(-(s.fine_tune as f64) / 96.0)) as u16
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
        if c.on_off {
            c.last = LastRound::default();
        } else if c.last.effect == E_EXTRA && c.last.arg == X_STOP {
            c.off_enable = true;
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
                        c.volume = c.volume.wrapping_add(arg).min(64);
                        c.last.effect = 0;
                        c.last.arg = 0;
                    }
                    X_FINE_VOL_DOWN => {
                        c.volume = c.volume.wrapping_sub(arg);
                        if (c.volume as i8) < 0 {
                            c.volume = 0;
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
            E_PORTAMENTO_UP => {
                let a = if arg == 0 { c.porta_up_old } else { arg };
                c.porta_up_old = a;
                c.last.period = c.last.period.wrapping_sub(a as u16);
                if c.last.period < 113 {
                    c.last.period = 113;
                }
            }
            E_PORTAMENTO_DOWN => {
                let a = if arg == 0 { c.porta_down_old } else { arg };
                c.porta_down_old = a;
                c.last.period = c.last.period.wrapping_add(a as u16);
                if c.last.period > 856 {
                    c.last.period = 856;
                }
            }
            E_GLISSANDO => self.glissando(c, arg),
            E_VIBRATO => self.vibrato(c, arg),
            E_GLISSANDO_VOLSLIDE => {
                self.volume_slide(c, arg);
                self.glissando(c, 0);
            }
            E_VIBRATO_VOLSLIDE => {
                self.volume_slide(c, arg);
                self.vibrato(c, 0);
            }
            E_VOLUME_SLIDE => self.volume_slide(c, arg),
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
        if a < 0x10 {
            c.volume = c.volume.wrapping_sub(a);
            if (c.volume as i8) < 0 {
                c.volume = 0;
            }
        } else {
            c.volume = c.volume.wrapping_add(a >> 4);
            if c.volume > 64 {
                c.volume = 64;
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
        let mut a = if arg == 0 { c.glissando_old_value } else { arg };
        if self.count == 1 {
            c.glissando_old_value = a;
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
        } else {
            if c.glissando_enable {
                c.glissando_old_period = c.glissando_old_period.wrapping_sub(a as u16);
                if c.glissando_old_period <= c.glissando_new_period {
                    c.glissando_old_period = c.glissando_new_period;
                    c.glissando_new_period = 0;
                }
            } else {
                c.glissando_old_period = c.glissando_old_period.wrapping_add(a as u16);
                if c.glissando_old_period >= c.glissando_new_period {
                    c.glissando_old_period = c.glissando_new_period;
                    c.glissando_new_period = 0;
                }
            }
            c.last.period = c.glissando_old_period;
        }
        let _ = &mut a;
    }

    fn vibrato(&self, c: &mut Chan, arg: u8) {
        let mut effect_arg = arg;
        if self.count == self.tempo && c.vibrato_period == 0 {
            c.vibrato_period = c.last.period;
        }
        let mut period = c.vibrato_period;
        'body: {
            if self.count == self.tempo.wrapping_sub(1) {
                c.vibrato_period = 0;
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
