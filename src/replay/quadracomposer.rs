//! Quadra Composer (IFF EMOD, `FORM....EMODEMIC`), ported from NostalgicPlayer's
//! QuadraComposer player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! Where the original player's register writes differ from what NostalgicPlayer
//! asks of its mixer, this follows the original (checked against the "IFF EMOD"
//! Eagleplayer). The samples are played from the module in guest memory (the 8SMP
//! chunk). A row's DMA goes off and on once for all its notes, after the writes
//! of the voices' new samples, and the loop registers of all four voices follow it.

use super::quadracomposer_tables::{PERIODS, VIBRATO};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Quadra Composer", detect, new };

#[derive(Clone, Default)]
struct Sample {
    vol: u8,
    /// Bytes.
    length: u32,
    finetune: u8,
    looped: bool,
    loop_start: u32,
    loop_len: u32,
    /// Where the sample data lies in guest memory.
    addr: u32,
}

#[derive(Clone, Default)]
struct Pattern {
    rows: u8,
    /// (rows + 1) rows of four voices' [sample, note, effect, argument].
    cells: Vec<[u8; 4]>,
}

#[derive(Clone, Copy, Default)]
struct Cell {
    sample: u8,
    note: i8,
    effect: u8,
    arg: u8,
}

#[derive(Clone, Default)]
struct Chan {
    cell: Cell,
    loop_start: u32,
    loop_len: u32,
    period: u16,
    volume: u16,
    length: u32,
    start: u32,
    note_nr: i16,
    wanted_period: u16,
    port_up: bool,
    vib_wave: u8,
    gliss: u8,
    vib_cmd: u8,
    vib_pos: u16,
    trem_wave: u8,
    trem_cmd: u8,
    trem_pos: u16,
    sample_offset: u8,
    retrig: u8,
    port_speed: u16,
    finetune: u8,
    addr: u32,
}

pub struct QuadraComposer {
    title: String,
    samples: Vec<Option<Sample>>,
    patterns: Vec<Option<Pattern>>,
    positions: Vec<u8>,
    // global playing info
    position: u16,
    new_position: u16,
    break_row: u16,
    new_row: u16,
    row: u16,
    loop_row: u16,
    pattern_wait: u8,
    tempo: u16,
    speed: u16,
    speed_count: u16,
    new_position_flag: bool,
    jump_break_flag: bool,
    loop_count: u8,
    intro_row: bool,
    set_tempo: bool,
    /// The CIA timer value for the tempo.
    timer: u32,
    ch: [Chan; 4],
    /// Voices whose DMA the row's notes start.
    dma_mask: u16,
    songend: bool,
    visited: Vec<bool>,
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 64 && &d[0..4] == b"FORM" && &d[8..16] == b"EMODEMIC" && be16(d, 20) == 1
}

fn text(d: &[u8], o: usize, n: usize) -> String {
    let s = d.get(o..o + n).unwrap_or(&[]);
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    s[..end].iter().map(|&c| c as char).collect::<String>().trim_end().to_string()
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let mut samples: Vec<Option<Sample>> = vec![None; 256];
    let mut patterns: Vec<Option<Pattern>> = vec![None; 256];
    let mut positions = Vec::new();
    let mut title = String::new();
    let mut start_tempo = 0;
    let mut patt: Option<usize> = None;
    let mut smp: Option<usize> = None;

    // the chunks, as NostalgicPlayer reads them
    let mut o = 12usize;
    while o + 8 <= d.len() {
        let name = &d[o..o + 4];
        let size = be32(d, o + 4) as usize;
        let body = o + 8;
        if size > d.len() - body {
            return None;
        }
        match name {
            b"EMIC" => {
                let mut p = body + 2;
                title = text(d, p, 20);
                p += 40;
                start_tempo = *d.get(p)?;
                let ns = *d.get(p + 1)? as usize;
                p += 2;
                for _ in 0..ns {
                    let e = d.get(p..p + 34)?;
                    let number = e[0] as usize;
                    if number == 0 {
                        return None;
                    }
                    samples[number - 1] = Some(Sample {
                        vol: e[1],
                        length: be16(e, 2) as u32 * 2,
                        looped: e[24] & 1 != 0,
                        finetune: e[25] & 0x0f,
                        loop_start: be16(e, 26) as u32 * 2,
                        loop_len: be16(e, 28) as u32 * 2,
                        addr: 0,
                    });
                    p += 34;
                }
                p += 1;
                let np = *d.get(p)? as usize;
                p += 1;
                for _ in 0..np {
                    let e = d.get(p..p + 26)?;
                    patterns[e[0] as usize] = Some(Pattern { rows: e[1], cells: Vec::new() });
                    p += 26;
                }
                let npos = *d.get(p)? as usize;
                p += 1;
                positions = d.get(p..p + npos)?.to_vec();
            }
            b"PATT" => patt = Some(body),
            b"8SMP" => smp = Some(body),
            _ => {}
        }
        o = body + size;
        // the chunks are not padded in the way IFF asks: NostalgicPlayer takes the next one
        // straight after
    }

    // the patterns follow each other in the PATT chunk, by number
    let mut p = patt?;
    for pat in patterns.iter_mut().flatten() {
        let n = (pat.rows as usize + 1) * 4;
        let b = d.get(p..p + n * 4)?;
        pat.cells = b.chunks(4).map(|c| [c[0], c[1], c[2] & 0x0f, c[3]]).collect();
        p += n * 4;
    }
    // and the samples in the 8SMP chunk
    let mut p = smp?;
    for s in samples.iter_mut().flatten() {
        if s.length == 0 {
            continue;
        }
        if p + s.length as usize > d.len() {
            return None;
        }
        s.addr = base + p as u32;
        p += s.length as usize;
    }
    if positions.is_empty() || positions.iter().any(|&n| patterns[n as usize].is_none()) {
        return None;
    }

    let first = patterns[positions[0] as usize].as_ref()?.rows as u16;
    let n = positions.len();
    let mut q = QuadraComposer {
        title,
        samples,
        patterns,
        positions,
        position: 0,
        new_position: 0,
        break_row: first,
        new_row: 0,
        row: 0,
        loop_row: 0,
        pattern_wait: 0,
        tempo: start_tempo as u16,
        speed: 6,
        speed_count: 6,
        new_position_flag: false,
        jump_break_flag: false,
        loop_count: 0,
        intro_row: true,
        set_tempo: true,
        timer: QuadraComposer::timer_for(start_tempo as u16),
        ch: Default::default(),
        dma_mask: 0,
        songend: false,
        visited: vec![false; n],
    };
    q.visited[0] = true;
    // the init routine sets the LED bit of CIA-A's port: the filter is off
    m.bus.ciaa.pra |= 2;
    Some(Box::new(q))
}

impl Replayer for QuadraComposer {
    fn format(&self) -> &'static str {
        "Quadra Composer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.change_tempo();
        self.dma_mask = 0;
        self.speed_count += 1;
        if self.speed_count < self.speed {
            self.run_tick_effects(hw);
        } else if self.pattern_wait != 0 {
            self.pattern_wait -= 1;
            self.speed_count = 0;
            self.run_tick_effects(hw);
        } else {
            self.get_notes(hw);
        }
        self.start_voices(hw);
    }
}

/// Colour clock of the raster line at which the loop register writes start (measured).
const LOOP_PHASE: u64 = 113;

impl QuadraComposer {
    fn timer_for(tempo: u16) -> u32 {
        if tempo == 0 { crate::deli::DEFAULT_TIMER as u32 } else { 1789773 / tempo as u32 }
    }

    fn change_tempo(&mut self) {
        if self.set_tempo {
            self.set_tempo = false;
            // the CIA timer of a tempo: 1789773 / tempo E-clock ticks (the original's constant,
            // measured from its logs)
            self.timer = Self::timer_for(self.tempo);
        }
    }

    fn cell(&self, ch: usize) -> Cell {
        let pat = self.patterns[self.positions[self.position as usize] as usize].as_ref().unwrap();
        let c = pat.cells.get(self.row as usize * 4 + ch).copied().unwrap_or_default();
        Cell { sample: c[0], note: c[1] as i8, effect: c[2], arg: c[3] }
    }

    fn get_notes(&mut self, hw: &mut Hw) {
        if !self.intro_row {
            self.change_tempo();
            if self.new_position_flag {
                self.new_position_flag = false;
                self.position = self.new_position;
                self.init_position();
            } else if self.jump_break_flag {
                self.jump_break_flag = false;
                if self.loop_row <= self.break_row {
                    self.row = self.loop_row;
                }
            } else {
                self.row += 1;
                if self.row > self.break_row {
                    self.position += 1;
                    self.init_position();
                }
            }
        }
        self.intro_row = false;
        self.speed_count = 0;
        for i in 0..4 {
            let c = self.cell(i);
            self.play_note(i, c, hw);
        }
    }

    /// The DMA of the voices whose samples this tick started goes off and on together, and the
    /// loop registers of all four voices follow: the original waits ten raster lines after the
    /// DMA is on and then for the beam to be at colour clock 113 of a line (in every run, whatever
    /// the DMA-on time), and writes the pairs at fixed spacing.
    fn start_voices(&mut self, hw: &mut Hw) {
        if self.dma_mask != 0 {
            let m = self.dma_mask;
            hw.dmacon(m);
            hw.wait(110);
            hw.dmacon(0x8000 | m);
            hw.wait_lines(10, LOOP_PHASE);
            for i in 0..4 {
                self.write_loop(i, hw);
            }
        }
    }

    fn write_loop(&mut self, i: usize, hw: &mut Hw) {
        let c = &self.ch[i];
        let (loc, len) = if c.loop_len != 0 { (c.addr + c.loop_start, (c.loop_len / 2) as u16) } else { (0, 0) };
        hw.loc(i, loc);
        // clearing the pair takes two colour clocks longer than loading it (measured)
        hw.wait(if loc == 0 { 107 } else { 105 });
        hw.len(i, len);
        hw.wait(97);
    }

    fn init_position(&mut self) {
        if self.position as usize >= self.positions.len() {
            self.position = 0;
        }
        if self.visited[self.position as usize] {
            self.songend = true;
        }
        self.visited[self.position as usize] = true;
        let pat = self.patterns[self.positions[self.position as usize] as usize].as_ref().unwrap();
        self.break_row = pat.rows as u16;
        self.row = self.new_row;
        self.new_row = 0;
        if self.break_row < self.row {
            self.row = self.break_row;
        }
    }

    fn period(&self, ft: u8, note: i16) -> u16 {
        PERIODS[(ft & 15) as usize].get(note as usize).copied().unwrap_or(0)
    }

    /// Starts the voice's sample from `start`, its registers; DMA is the caller's.
    fn sample_regs(&mut self, i: usize, hw: &mut Hw) {
        let c = &self.ch[i];
        hw.loc(i, c.addr + c.start);
        hw.wait(5);
        // a sample of one word or less is played with a length of 0, as the original writes it
        let words = (c.length.wrapping_sub(c.start) / 2) as u16;
        hw.len(i, if words <= 1 { 0 } else { words });
        hw.wait(5);
    }

    /// Plays the voice's sample again, at its period.
    fn retrigger(&mut self, i: usize, hw: &mut Hw) {
        self.sample_regs(i, hw);
        let p = self.ch[i].period;
        self.set_period(i, hw, p);
        self.dma_mask |= 1 << i;
    }

    fn set_period(&mut self, i: usize, hw: &mut Hw, p: u16) {
        hw.per(i, p);
        hw.wait(5);
    }

    fn set_vol(&mut self, i: usize, hw: &mut Hw, v: u16) {
        hw.vol(i, v);
        hw.wait(5);
    }

    fn play_note(&mut self, i: usize, c: Cell, hw: &mut Hw) {
        self.ch[i].cell = c;
        if c.sample != 0 {
            let Some(s) = self.samples[c.sample as usize - 1].clone() else { return };
            let ch = &mut self.ch[i];
            ch.volume = s.vol as u16;
            ch.length = s.length;
            ch.finetune = s.finetune;
            ch.addr = s.addr;
            ch.start = 0;
            if s.looped {
                ch.loop_start = s.loop_start;
                ch.length = s.loop_start + s.loop_len;
                ch.loop_len = s.loop_len;
            } else {
                ch.loop_start = 0;
                ch.loop_len = 0;
            }
            self.set_vol(i, hw, s.vol as u16);
        }
        if c.note >= 0 {
            self.ch[i].note_nr = c.note as i16;
            if c.effect == 0xe && c.arg & 0xf0 == 0x50 {
                self.ch[i].finetune = c.arg & 0x0f;
            } else if c.effect == 3 || c.effect == 5 {
                let ch = &mut self.ch[i];
                ch.wanted_period = PERIODS[(ch.finetune & 15) as usize].get(ch.note_nr as usize).copied().unwrap_or(0);
                ch.port_up = ch.wanted_period > ch.period;
                return;
            }
            let p = self.period(self.ch[i].finetune, self.ch[i].note_nr);
            self.ch[i].period = p;
            if c.effect == 0xe && c.arg & 0xf0 == 0xd0 {
                self.note_delay(i, hw);
                return;
            }
            self.sample_regs(i, hw);
            self.set_period(i, hw, p);
            self.dma_mask |= 1 << i;
        }
        self.after_period_effect(i, hw);
    }

    fn run_tick_effects(&mut self, hw: &mut Hw) {
        for i in 0..4 {
            let c = self.ch[i].cell;
            match c.effect {
                0 => self.arpeggio(i, hw),
                1 => self.slide_up(i, hw),
                2 => self.slide_down(i, hw),
                3 => self.tone_portamento(i, hw),
                4 => self.vibrato(i, hw),
                5 => {
                    if self.ch[i].wanted_period != 0 {
                        self.actual_tone_portamento(i, hw);
                    }
                    self.volume_slide(i, hw);
                }
                6 => {
                    // the original's vibrato here has no depth: it writes the period as it is
                    let p = self.ch[i].period;
                    self.set_period(i, hw, p);
                    self.volume_slide(i, hw);
                }
                7 => self.tremolo(i, hw),
                0xa => self.volume_slide(i, hw),
                0xe => match c.arg & 0xf0 {
                    0x90 => self.retrig_note(i, hw),
                    0xc0 => self.note_cut(i, hw),
                    0xd0 => self.note_delay(i, hw),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn after_period_effect(&mut self, i: usize, hw: &mut Hw) {
        let c = self.ch[i].cell;
        match c.effect {
            0 => self.arpeggio(i, hw),
            9 => self.set_sample_offset(i, hw),
            0xb => {
                self.new_position = c.arg as u16;
                self.new_position_flag = true;
                self.new_row = 0;
            }
            0xc => {
                let v = (c.arg as u16).min(64);
                self.ch[i].volume = v;
                self.set_vol(i, hw, v);
            }
            0xd => {
                self.new_position = self.position + 1;
                self.new_row = c.arg as u16;
                self.new_position_flag = true;
            }
            0xe => match c.arg & 0xf0 {
                0x10 => {
                    let ch = &mut self.ch[i];
                    ch.period = ch.period.wrapping_sub((c.arg & 0x0f) as u16);
                    if ch.period < 113 {
                        ch.period = 113;
                    }
                    let p = ch.period;
                    self.set_period(i, hw, p);
                }
                0x20 => {
                    let ch = &mut self.ch[i];
                    ch.period = ch.period.wrapping_add((c.arg & 0x0f) as u16);
                    if ch.period > 856 {
                        ch.period = 856;
                    }
                    let p = ch.period;
                    self.set_period(i, hw, p);
                }
                0x30 => self.ch[i].gliss = c.arg & 0x0f,
                0x40 => self.ch[i].vib_wave = c.arg & 0x0f,
                0x50 => self.ch[i].finetune = c.arg & 0x0f,
                0x60 => {
                    let a = c.arg & 0x0f;
                    if a == 0 {
                        self.loop_row = self.row;
                    } else if self.loop_count == 0 {
                        self.loop_count = a;
                        self.jump_break_flag = true;
                    } else {
                        self.loop_count -= 1;
                        if self.loop_count != 0 {
                            self.jump_break_flag = true;
                        }
                    }
                }
                0x70 => self.ch[i].trem_wave = c.arg & 0x0f,
                0x90 => {
                    self.ch[i].retrig = 0;
                    self.retrig_note(i, hw);
                }
                0xa0 => {
                    let v = (self.ch[i].volume + (c.arg & 0x0f) as u16).min(64);
                    self.ch[i].volume = v;
                    self.set_vol(i, hw, v);
                }
                0xb0 => {
                    let v = (self.ch[i].volume as i16 - (c.arg & 0x0f) as i16).max(0) as u16;
                    self.ch[i].volume = v;
                    self.set_vol(i, hw, v);
                }
                0xc0 => self.note_cut(i, hw),
                0xd0 => self.note_delay(i, hw),
                0xe0 => self.pattern_wait = c.arg & 0x0f,
                _ => {}
            },
            0xf => {
                if c.arg > 31 {
                    self.tempo = c.arg as u16;
                    self.set_tempo = true;
                } else {
                    self.speed = (c.arg as u16).max(1);
                    self.speed_count = 0;
                }
            }
            _ => {}
        }
    }

    fn arpeggio(&mut self, i: usize, hw: &mut Hw) {
        let c = self.ch[i].cell;
        if c.arg != 0 {
            let p = match self.speed_count % 3 {
                0 => self.ch[i].period,
                n => {
                    let off = if n == 1 { c.arg >> 4 } else { c.arg & 0x0f };
                    let note = (off as i16 + self.ch[i].note_nr).min(35);
                    self.period(self.ch[i].finetune, note)
                }
            };
            self.set_period(i, hw, p);
        }
    }

    fn slide_up(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        ch.period = ch.period.wrapping_sub(ch.cell.arg as u16);
        if ch.period < 113 {
            ch.period = 113;
        }
        let p = ch.period;
        self.set_period(i, hw, p);
    }

    fn slide_down(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        ch.period = ch.period.wrapping_add(ch.cell.arg as u16);
        if ch.period > 856 {
            ch.period = 856;
        }
        let p = ch.period;
        self.set_period(i, hw, p);
    }

    fn tone_portamento(&mut self, i: usize, hw: &mut Hw) {
        if self.ch[i].wanted_period == 0 {
            return;
        }
        if self.ch[i].cell.arg != 0 {
            self.ch[i].port_speed = self.ch[i].cell.arg as u16;
        }
        self.actual_tone_portamento(i, hw);
    }

    fn actual_tone_portamento(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        let speed = ch.port_speed;
        if ch.port_up {
            ch.period = ch.period.wrapping_add(speed);
            if ch.period >= ch.wanted_period {
                ch.period = ch.wanted_period;
                ch.wanted_period = 0;
                let p = ch.period;
                self.set_period(i, hw, p);
                return;
            }
        } else {
            let np = ch.period.wrapping_sub(speed) as i16;
            if np <= ch.wanted_period as i16 {
                ch.period = ch.wanted_period;
                ch.wanted_period = 0;
                let p = ch.period;
                self.set_period(i, hw, p);
                return;
            }
            ch.period = np as u16;
        }
        let (gliss, ft, period) = (ch.gliss, ch.finetune, ch.period);
        if gliss != 0 {
            if let Some(&p) = PERIODS[(ft & 15) as usize].iter().find(|&&p| period >= p) {
                self.set_period(i, hw, p);
            }
        } else {
            self.set_period(i, hw, period);
        }
    }

    fn vibrato(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        let arg = ch.cell.arg;
        if arg != 0 {
            if arg & 0x0f != 0 {
                ch.vib_cmd = (ch.vib_cmd & 0xf0) | (arg & 0x0f);
            }
            if arg & 0xf0 != 0 {
                ch.vib_cmd = (ch.vib_cmd & 0x0f) | (arg & 0xf0);
            }
        }
        self.actual_vibrato(i, hw);
    }

    fn actual_vibrato(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        ch.vib_pos = (ch.vib_pos + (ch.vib_cmd >> 4) as u16) & 0x3f;
        let v = VIBRATO[(ch.vib_wave as usize).min(2)][ch.vib_pos as usize] as i32;
        let np = (ch.period as i16 as i32 + (((ch.vib_cmd & 0x0f) as i32 * v) >> 14)) as i16;
        let p = np.clamp(113, 856) as u16;
        self.set_period(i, hw, p);
    }

    fn tremolo(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        let arg = ch.cell.arg;
        if arg != 0 {
            if arg & 0x0f != 0 {
                ch.trem_cmd = (ch.trem_cmd & 0xf0) | (arg & 0x0f);
            }
            if arg & 0xf0 != 0 {
                ch.trem_cmd = (ch.trem_cmd & 0x0f) | (arg & 0xf0);
            }
        }
        ch.trem_pos = (ch.trem_pos + (ch.trem_cmd >> 4) as u16) & 0x3f;
        let v = VIBRATO[(ch.trem_wave as usize).min(2)][ch.trem_pos as usize] as i32;
        let nv = (ch.volume as i32 + (((ch.trem_cmd & 0x0f) as i32 * v) >> 14)).clamp(0, 64) as u16;
        self.set_vol(i, hw, nv);
    }

    fn set_sample_offset(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        if ch.cell.arg != 0 {
            ch.sample_offset = ch.cell.arg;
        }
        let offset = ch.sample_offset as u32 * 512;
        if offset < ch.length {
            ch.start = offset;
        } else {
            ch.start = ch.loop_start;
            ch.length = ch.loop_len;
        }
        if ch.length > 0 {
            self.sample_regs(i, hw);
        } else {
            // the voice is muted
            self.set_vol(i, hw, 0);
        }
    }

    fn volume_slide(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        let mut v = ch.volume as i16;
        let up = (ch.cell.arg >> 4) as i16;
        if up != 0 {
            v = (v + up).min(64);
        } else {
            v = (v - (ch.cell.arg & 0x0f) as i16).max(0);
        }
        ch.volume = v as u16;
        self.set_vol(i, hw, v as u16);
    }

    fn retrig_note(&mut self, i: usize, hw: &mut Hw) {
        let ch = &mut self.ch[i];
        ch.retrig = ch.retrig.wrapping_add(1);
        if ch.retrig >= ch.cell.arg & 0x0f {
            ch.retrig = 0;
            self.retrigger(i, hw);
        }
    }

    fn note_cut(&mut self, i: usize, hw: &mut Hw) {
        if (self.ch[i].cell.arg & 0x0f) as u16 <= self.speed_count {
            self.ch[i].volume = 0;
            self.set_vol(i, hw, 0);
        }
    }

    fn note_delay(&mut self, i: usize, hw: &mut Hw) {
        if self.ch[i].note_nr >= 0 && (self.ch[i].cell.arg & 0x0f) as u16 == self.speed_count {
            self.retrigger(i, hw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn chunk(out: &mut Vec<u8>, name: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(name);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(body);
    }

    /// A module of one position and one pattern of `rows` + 1 rows, two samples (the second
    /// looped from word 8, 8 words long), with `cells` at (row, voice); tempo 125.
    fn module(rows: u8, cells: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
        let mut emic = vec![0, 1];
        emic.extend_from_slice(b"test song\0\0\0\0\0\0\0\0\0\0\0");
        emic.extend_from_slice(b"nobody\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        emic.extend_from_slice(&[125, 2]);
        for (n, vol, ctl, ls, ll) in [(1u8, 48u8, 0u8, 0u16, 1u16), (2, 40, 1, 8, 8)] {
            let mut e = vec![n, vol];
            e.extend_from_slice(&16u16.to_be_bytes());
            e.extend_from_slice(&[0; 20]);
            e.extend_from_slice(&[ctl, 0]);
            e.extend_from_slice(&ls.to_be_bytes());
            e.extend_from_slice(&ll.to_be_bytes());
            e.extend_from_slice(&[0; 4]);
            emic.extend(e);
        }
        emic.push(0);
        emic.push(1);
        emic.extend_from_slice(&[0, rows]);
        emic.extend_from_slice(&[0; 24]);
        emic.extend_from_slice(&[1, 0]);
        // an empty cell has note -1
        let mut patt: Vec<u8> = (0..(rows as usize + 1) * 4).flat_map(|_| [0, 0xff, 0, 0]).collect();
        for &(row, voice, c) in cells {
            let o = row * 16 + voice * 4;
            patt[o..o + 4].copy_from_slice(&c);
        }
        let mut d = b"FORM\0\0\0\0EMOD".to_vec();
        chunk(&mut d, b"EMIC", &emic);
        chunk(&mut d, b"PATT", &patt);
        chunk(&mut d, b"8SMP", &[0x20; 64]);
        let n = d.len() as u32 - 8;
        d[4..8].copy_from_slice(&n.to_be_bytes());
        d
    }

    /// Sample `s`, note `n`, effect `e`, argument `a`.
    fn cell(s: u8, n: i8, e: u8, a: u8) -> [u8; 4] {
        [s, n as u8, e, a]
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.emod"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, log.iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn identifies_and_rejects() {
        let d = module(3, &[]);
        assert!(detect(&d));
        let mut bad = d.clone();
        bad[8..12].copy_from_slice(b"ILBM");
        assert!(!detect(&bad));
        bad = d.clone();
        bad[21] = 2;
        assert!(!detect(&bad));
        assert!(!detect(&d[..40]));
    }

    #[test]
    fn plays_a_note_and_its_loop() {
        let d = module(3, &[(0, 0, cell(1, 12, 0, 0)), (0, 1, cell(2, 12, 0, 0))]);
        let (format, log) = play(d, 30);
        assert_eq!(format, "Quadra Composer");
        assert_eq!(&reg(&log, 0xDFF0A8)[..1], &[48]);
        assert_eq!(&reg(&log, 0xDFF0B8)[..1], &[40]);
        // voice 0 plays its 16 words once (then no loop), voice 1 up to the end of its loop
        assert_eq!(&reg(&log, 0xDFF0A4)[..2], &[16, 0]);
        assert_eq!(&reg(&log, 0xDFF0B4)[..2], &[16, 8]);
        // note 12 of fine tune 0
        assert_eq!(&reg(&log, 0xDFF0A6)[..1], &[428]);
        // both DMA bits go off and on together, once
        assert_eq!(&reg(&log, 0xDFF096)[..2], &[0x0003, 0x8003]);
    }

    #[test]
    fn effects_set_volume_and_slide_the_period() {
        let d = module(
            3,
            &[(0, 0, cell(1, 12, 0xc, 0x20)), (1, 0, cell(0, -1, 1, 4)), (1, 1, cell(2, 12, 0xf, 3))],
        );
        let (_, log) = play(d, 40);
        let vol = reg(&log, 0xDFF0A8);
        assert_eq!(&vol[..2], &[48, 0x20]);
        // slide up by 4 on each of the row's two other ticks, at speed 3
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(&per[..3], &[428, 424, 420]);
    }
}
