//! Native ProTracker/NoiseTracker/SoundTracker replayer, standing in for
//! DeliTracker's internal player. It drives the emulated Paula exactly like
//! a 68k replay routine would (DMA off, latch sample, DMA on, latch loop),
//! so the same mixer, filters and scopes apply.

use crate::paula::Paula;
use std::collections::HashSet;

const BASE_PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, //
    428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, //
    214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244,
    235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24,
];

#[derive(Clone, Default)]
struct Sample {
    offset: u32, // inside module
    len: u32,    // bytes
    finetune: u8,
    volume: u8,
    rep_start: u32, // bytes
    rep_len: u32,   // bytes
}

#[derive(Clone, Copy, Default)]
struct Note {
    sample: u8,
    period: u16,
    cmd: u8,
    param: u8,
}

#[derive(Clone, Default)]
struct Chan {
    sample: usize,
    period: u16,
    volume: u8,
    finetune: u8,
    note_idx: usize,
    porta_target: u16,
    porta_speed: u8,
    vib_cmd: u8,
    vib_pos: u8,
    vib_wave: u8,
    trem_cmd: u8,
    trem_pos: u8,
    trem_wave: u8,
    trem_vol: Option<u8>,
    gliss: bool,
    offset_mem: u8,
    loop_row: usize,
    loop_count: u8,
    cur: Note,
    out_period: u16,
}

pub struct ProTracker {
    pub title: String,
    base: u32, // module address in guest RAM
    samples: Vec<Sample>,
    orders: Vec<u8>,
    song_len: usize,
    restart: usize,
    patterns: Vec<[Note; 256]>,
    ch: [Chan; 4],
    speed: u32,
    pub bpm: u32,
    tick: u32,
    pos: usize,
    row: usize,
    break_pos: Option<usize>,
    break_row: Option<usize>,
    pattern_delay: u32,
    delay_rows: u32,
    visited: HashSet<(usize, usize)>,
    pub songend: bool,
    pub filter: bool,
    /// Channels triggered this tick whose DMA `dma_on` turns on again, and the
    /// loop each one then gets.
    pub dma_pending: u16,
    loops: [(u32, u32); 4],
}

/// An audio register write ($DFF0A0 + `reg`), logged for DELI_WRITE_LOG.
fn write_reg(paula: &mut Paula, reg: u32, v: u16) {
    let t = paula.now();
    if let Some(l) = &mut paula.native_log {
        l.push((0xDFF0A0 + reg, v, t));
    }
    paula.write_audio_reg(reg, v);
}

/// A DMACON write, logged for DELI_WRITE_LOG.
fn write_dma(paula: &mut Paula, v: u16) {
    let t = paula.now();
    if let Some(l) = &mut paula.native_log {
        l.push((0xDFF096, v, t));
    }
    paula.write_dmacon(v);
}

fn word(d: &[u8], o: usize) -> u32 {
    ((d[o] as u32) << 8) | d[o + 1] as u32
}

fn period_for(note: usize, finetune: u8) -> u16 {
    let ft = (finetune & 0xF) as i32;
    let ft = if ft > 7 { ft - 16 } else { ft };
    let p = BASE_PERIODS[note.min(35)] as f64 * 2f64.powf(-ft as f64 / 96.0);
    p.round() as u16
}

fn note_index(period: u16) -> Option<usize> {
    if period == 0 {
        return None;
    }
    // closest base period (modules store finetune-0 periods)
    BASE_PERIODS
        .iter()
        .enumerate()
        .min_by_key(|(_, p)| (**p as i32 - period as i32).abs())
        .map(|(i, _)| i)
}

impl ProTracker {
    /// Recognises a 31-instrument (M.K. and friends) or 15-instrument module.
    pub fn detect(d: &[u8]) -> Option<&'static str> {
        if d.len() >= 1084 {
            match &d[1080..1084] {
                b"M.K." | b"M!K!" | b"M&K!" | b"N.T." => return Some("ProTracker"),
                b"FLT4" => return Some("StarTrekker"),
                b"4CHN" => return Some("FastTracker 4ch"),
                _ => {}
            }
        }
        if d.len() >= 600 + 1024 && Self::plausible_st(d) {
            return Some("SoundTracker");
        }
        None
    }

    fn plausible_st(d: &[u8]) -> bool {
        let song_len = d[470] as usize;
        if song_len == 0 || song_len > 128 {
            return false;
        }
        if d[..20].iter().any(|&c| c != 0 && !(32..127).contains(&c)) {
            return false;
        }
        let mut total = 0usize;
        for i in 0..15 {
            let o = 20 + i * 30;
            if d[o + 25] > 64 {
                return false;
            }
            if d[o..o + 22].iter().any(|&c| c != 0 && !(32..127).contains(&c)) {
                return false;
            }
            total += word(d, o + 22) as usize * 2;
        }
        let pats = d[472..600].iter().take(128).copied().max().unwrap_or(0) as usize + 1;
        if pats > 64 {
            return false;
        }
        let need = 600 + pats * 1024 + total;
        // allow truncated sample data, but the size must be in the ballpark
        need <= d.len() + 0x8000 && 600 + pats * 1024 <= d.len()
    }

    pub fn new(d: &[u8], base: u32) -> Option<Self> {
        let format = Self::detect(d)?;
        let nsamp = if format == "SoundTracker" { 15 } else { 31 };
        let hdr = 20 + nsamp * 30;
        let song_len = (d[hdr] as usize).clamp(1, 128);
        let restart = d[hdr + 1] as usize;
        let orders: Vec<u8> = d[hdr + 2..hdr + 130].to_vec();
        let pat_start = hdr + 130 + if nsamp == 31 { 4 } else { 0 };
        let npat = orders.iter().copied().max().unwrap_or(0) as usize + 1;
        let mut patterns = Vec::with_capacity(npat);
        for p in 0..npat {
            let mut pat = [Note::default(); 256];
            for (i, n) in pat.iter_mut().enumerate() {
                let o = pat_start + p * 1024 + i * 4;
                if o + 4 > d.len() {
                    break;
                }
                let b = &d[o..o + 4];
                *n = Note {
                    sample: (b[0] & 0xF0) | (b[2] >> 4),
                    period: (((b[0] & 0x0F) as u16) << 8) | b[1] as u16,
                    cmd: b[2] & 0x0F,
                    param: b[3],
                };
            }
            patterns.push(pat);
        }
        let mut samples = vec![Sample::default()];
        let mut off = (pat_start + npat * 1024) as u32;
        for i in 0..nsamp {
            let o = 20 + i * 30;
            let len = word(d, o + 22) * 2;
            let mut rs = word(d, o + 26);
            let rl = word(d, o + 28) * 2;
            if nsamp == 15 {
                // SoundTracker repeat start is in bytes
            } else {
                rs *= 2;
            }
            samples.push(Sample {
                offset: off,
                len,
                finetune: d[o + 24] & 0xF,
                volume: d[o + 25].min(64),
                rep_start: rs,
                rep_len: rl,
            });
            off += len;
        }
        let title: String = d[..20].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
        let mut pt = ProTracker {
            title: title.trim().to_string(),
            base,
            samples,
            orders,
            song_len,
            restart: if restart < song_len { restart } else { 0 },
            patterns,
            ch: Default::default(),
            speed: 6,
            bpm: 125,
            tick: 0,
            pos: 0,
            row: 0,
            break_pos: None,
            break_row: None,
            pattern_delay: 0,
            delay_rows: 0,
            visited: HashSet::new(),
            songend: false,
            filter: false,
            dma_pending: 0,
            loops: [(0, 0); 4],
        };
        pt.restart_song();
        Some(pt)
    }

    pub fn restart_song(&mut self) {
        self.ch = Default::default();
        self.speed = 6;
        self.bpm = 125;
        self.tick = 0;
        self.pos = 0;
        self.row = 0;
        self.break_pos = None;
        self.break_row = None;
        self.pattern_delay = 0;
        self.delay_rows = 0;
        self.visited.clear();
        self.songend = false;
    }

    pub fn position(&self) -> (usize, usize, usize) {
        (self.pos, self.song_len, self.row)
    }

    pub fn jump(&mut self, delta: i32) {
        let p = (self.pos as i32 + delta).clamp(0, self.song_len as i32 - 1) as usize;
        self.pos = p;
        self.row = 0;
        self.tick = 0;
        self.visited.clear();
    }

    /// Colour clocks between ticks: the CIA timer latch ProTracker sets for the
    /// tempo (1773447 / BPM), plus one, E-clock ticks of 5 colour clocks.
    pub fn tick_clocks(&self) -> u64 {
        (1_773_447 / self.bpm.max(1) as u64 + 1) * 5
    }

    fn set_hw_period(&mut self, paula: &mut Paula, c: usize, period: u16) {
        self.ch[c].out_period = period;
        write_reg(paula, c as u32 * 16 + 6, period);
    }

    fn set_hw_volume(&mut self, paula: &mut Paula, c: usize, v: u8) {
        write_reg(paula, c as u32 * 16 + 8, v as u16);
    }

    fn trigger(&mut self, paula: &mut Paula, c: usize, offset: u32) {
        let s = self.samples[self.ch[c].sample].clone();
        if s.len == 0 {
            return;
        }
        let r = c as u32 * 16;
        write_dma(paula, 1 << c);
        let start = self.base + s.offset;
        let (mut st, mut len) = (start, s.len);
        if offset > 0 {
            if offset < s.len {
                st += offset;
                len -= offset;
            } else {
                st = start + s.rep_start;
                len = s.rep_len.max(2);
            }
        }
        write_reg(paula, r, (st >> 16) as u16);
        write_reg(paula, r + 2, st as u16);
        write_reg(paula, r + 4, (len / 2).max(1) as u16);
        self.dma_pending |= 1 << c;
        self.loops[c] = if s.rep_len > 2 {
            (start + s.rep_start, s.rep_len)
        } else {
            // one-shot: loop over a (silent) first word, like PT does
            (start, 2)
        };
    }

    /// The second half of a tick that triggered notes: after its DMA wait (the
    /// channels turned off have finished their word), DMA on again, then the loop
    /// latches, which take effect at the end of the first pass.
    pub fn dma_on(&mut self, paula: &mut Paula) {
        let mask = std::mem::take(&mut self.dma_pending);
        if mask == 0 {
            return;
        }
        write_dma(paula, 0x8000 | mask);
        for c in 0..4 {
            if mask & 1 << c != 0 {
                let (r, (ls, ll)) = (c as u32 * 16, self.loops[c]);
                write_reg(paula, r, (ls >> 16) as u16);
                write_reg(paula, r + 2, ls as u16);
                write_reg(paula, r + 4, (ll / 2).max(1) as u16);
            }
        }
    }

    pub fn clear_loop_words(&self, ram: &mut [u8]) {
        for s in &self.samples[1..] {
            if s.len >= 2 && s.rep_len <= 2 {
                let a = (self.base + s.offset) as usize;
                if a + 1 < ram.len() {
                    ram[a] = 0;
                    ram[a + 1] = 0;
                }
            }
        }
    }

    pub fn tick(&mut self, paula: &mut Paula) {
        if self.tick == 0 {
            if self.delay_rows > 0 {
                self.delay_rows -= 1;
            } else {
                self.play_row(paula);
            }
        } else {
            for c in 0..4 {
                self.effects_tick(paula, c);
            }
        }
        self.tick += 1;
        if self.tick >= self.speed {
            self.tick = 0;
            if self.delay_rows == 0 {
                self.advance_row();
            }
        }
    }

    fn advance_row(&mut self) {
        if self.pattern_delay > 0 {
            self.delay_rows = self.pattern_delay;
            self.pattern_delay = 0;
            return;
        }
        let mut jumped = false;
        if self.break_pos.is_some() || self.break_row.is_some() {
            let np = match self.break_pos.take() {
                Some(p) => p,
                None => self.pos + 1,
            };
            self.row = self.break_row.take().unwrap_or(0).min(63);
            self.pos = np;
            jumped = true;
        } else {
            self.row += 1;
            if self.row >= 64 {
                self.row = 0;
                self.pos += 1;
            }
        }
        if self.pos >= self.song_len {
            self.pos = self.restart;
            self.songend = true;
            self.visited.clear();
        }
        if jumped && !self.visited.insert((self.pos, self.row)) {
            self.songend = true;
            self.visited.clear();
        } else if !jumped {
            self.visited.insert((self.pos, self.row));
        }
    }

    fn play_row(&mut self, paula: &mut Paula) {
        let pat = self.orders[self.pos.min(127)] as usize;
        let row = self.row;
        for c in 0..4 {
            let n = self.patterns.get(pat).map(|p| p[row * 4 + c]).unwrap_or_default();
            self.play_note(paula, c, n);
        }
    }

    fn play_note(&mut self, paula: &mut Paula, c: usize, n: Note) {
        self.ch[c].cur = n;
        self.ch[c].trem_vol = None;
        if n.sample > 0 && (n.sample as usize) < self.samples.len() {
            let s = &self.samples[n.sample as usize];
            self.ch[c].sample = n.sample as usize;
            self.ch[c].volume = s.volume;
            self.ch[c].finetune = s.finetune;
        }
        if n.cmd == 0xE && n.param >> 4 == 5 {
            self.ch[c].finetune = n.param & 0xF;
        }
        let toneporta = n.cmd == 3 || n.cmd == 5;
        let delayed = n.cmd == 0xE && n.param >> 4 == 0xD && n.param & 0xF != 0;
        if n.period != 0 {
            let idx = note_index(n.period).unwrap_or(0);
            let period = period_for(idx, self.ch[c].finetune);
            if toneporta {
                self.ch[c].porta_target = period;
                if n.cmd == 3 && n.param != 0 {
                    self.ch[c].porta_speed = n.param;
                }
            } else {
                self.ch[c].note_idx = idx;
                self.ch[c].period = period;
                if self.ch[c].vib_wave & 4 == 0 {
                    self.ch[c].vib_pos = 0;
                }
                if self.ch[c].trem_wave & 4 == 0 {
                    self.ch[c].trem_pos = 0;
                }
                if !delayed {
                    let off = if n.cmd == 9 {
                        if n.param != 0 {
                            self.ch[c].offset_mem = n.param;
                        }
                        self.ch[c].offset_mem as u32 * 256
                    } else {
                        0
                    };
                    self.trigger(paula, c, off);
                }
            }
        }
        self.effects_row(paula, c);
        let p = self.ch[c].period;
        self.set_hw_period(paula, c, p);
        let v = self.ch[c].volume;
        self.set_hw_volume(paula, c, v);
    }

    fn effects_row(&mut self, paula: &mut Paula, c: usize) {
        let n = self.ch[c].cur;
        let (x, y) = (n.param >> 4, n.param & 0xF);
        match n.cmd {
            3 => {
                if n.param != 0 {
                    self.ch[c].porta_speed = n.param;
                }
            }
            4 => {
                let v = &mut self.ch[c].vib_cmd;
                if x != 0 {
                    *v = (*v & 0x0F) | (x << 4);
                }
                if y != 0 {
                    *v = (*v & 0xF0) | y;
                }
            }
            7 => {
                let v = &mut self.ch[c].trem_cmd;
                if x != 0 {
                    *v = (*v & 0x0F) | (x << 4);
                }
                if y != 0 {
                    *v = (*v & 0xF0) | y;
                }
            }
            0xB => {
                self.break_pos = Some(n.param as usize);
                if self.break_row.is_none() {
                    self.break_row = Some(0);
                }
            }
            0xC => self.ch[c].volume = n.param.min(64),
            0xD => {
                self.break_row = Some((x * 10 + y) as usize);
            }
            0xE => match x {
                0 => self.filter = y & 1 == 0,
                1 => self.ch[c].period = self.ch[c].period.saturating_sub(y as u16).max(113),
                2 => self.ch[c].period = (self.ch[c].period + y as u16).min(856),
                3 => self.ch[c].gliss = y != 0,
                4 => self.ch[c].vib_wave = y,
                6 => {
                    if y == 0 {
                        self.ch[c].loop_row = self.row;
                    } else {
                        if self.ch[c].loop_count == 0 {
                            self.ch[c].loop_count = y;
                        } else {
                            self.ch[c].loop_count -= 1;
                        }
                        if self.ch[c].loop_count != 0 {
                            self.break_pos = Some(self.pos);
                            self.break_row = Some(self.ch[c].loop_row);
                            // loops are not song-end jumps
                            for r in self.ch[c].loop_row..=self.row {
                                self.visited.remove(&(self.pos, r));
                            }
                        }
                    }
                }
                7 => self.ch[c].trem_wave = y,
                0xA => self.ch[c].volume = (self.ch[c].volume + y).min(64),
                0xB => self.ch[c].volume = self.ch[c].volume.saturating_sub(y),
                0xC if y == 0 => self.ch[c].volume = 0,
                0xE => self.pattern_delay = y as u32,
                _ => {}
            },
            0xF => {
                if n.param == 0 {
                    // F00 stops the song in PT; treat as song end
                    self.songend = true;
                } else if n.param < 32 {
                    self.speed = n.param as u32;
                } else {
                    self.bpm = n.param as u32;
                }
            }
            _ => {}
        }
        let _ = paula;
    }

    fn vibrato_delta(&mut self, c: usize) -> i32 {
        let ch = &mut self.ch[c];
        let pos = ch.vib_pos & 31;
        let amp = match ch.vib_wave & 3 {
            0 => SINE[pos as usize] as i32,
            1 => {
                let v = (pos as i32) * 8;
                if ch.vib_pos & 32 != 0 { 255 - v } else { v }
            }
            _ => 255,
        };
        let d = amp * (ch.vib_cmd & 0xF) as i32 / 128;
        let d = if ch.vib_pos & 32 != 0 { -d } else { d };
        ch.vib_pos = ch.vib_pos.wrapping_add(ch.vib_cmd >> 4) & 63;
        d
    }

    fn tremolo_delta(&mut self, c: usize) -> i32 {
        let ch = &mut self.ch[c];
        let pos = ch.trem_pos & 31;
        let amp = match ch.trem_wave & 3 {
            0 => SINE[pos as usize] as i32,
            1 => {
                let v = (pos as i32) * 8;
                if ch.trem_pos & 32 != 0 { 255 - v } else { v }
            }
            _ => 255,
        };
        let d = amp * (ch.trem_cmd & 0xF) as i32 / 64;
        let d = if ch.trem_pos & 32 != 0 { -d } else { d };
        ch.trem_pos = ch.trem_pos.wrapping_add(ch.trem_cmd >> 4) & 63;
        d
    }

    fn volume_slide(&mut self, c: usize, param: u8) {
        let (x, y) = (param >> 4, param & 0xF);
        let v = &mut self.ch[c].volume;
        if x != 0 {
            *v = (*v + x).min(64);
        } else {
            *v = v.saturating_sub(y);
        }
    }

    fn tone_porta(&mut self, c: usize) {
        let ch = &mut self.ch[c];
        if ch.porta_target == 0 {
            return;
        }
        let sp = ch.porta_speed as u16;
        if ch.period < ch.porta_target {
            ch.period = (ch.period + sp).min(ch.porta_target);
        } else if ch.period > ch.porta_target {
            ch.period = ch.period.saturating_sub(sp).max(ch.porta_target);
        }
    }

    fn effects_tick(&mut self, paula: &mut Paula, c: usize) {
        let n = self.ch[c].cur;
        let (x, y) = (n.param >> 4, n.param & 0xF);
        let mut period = self.ch[c].period;
        let mut vol = self.ch[c].volume;
        match n.cmd {
            0 if n.param != 0 => {
                let k = match self.tick % 3 {
                    0 => 0,
                    1 => x,
                    _ => y,
                } as usize;
                let idx = (self.ch[c].note_idx + k).min(35);
                period = period_for(idx, self.ch[c].finetune);
            }
            1 => {
                self.ch[c].period = self.ch[c].period.saturating_sub(n.param as u16).max(113);
                period = self.ch[c].period;
            }
            2 => {
                self.ch[c].period = (self.ch[c].period + n.param as u16).min(856);
                period = self.ch[c].period;
            }
            3 | 5 => {
                self.tone_porta(c);
                period = self.ch[c].period;
                if self.ch[c].gliss {
                    if let Some(i) = note_index(period) {
                        period = period_for(i, self.ch[c].finetune);
                    }
                }
                if n.cmd == 5 {
                    self.volume_slide(c, n.param);
                    vol = self.ch[c].volume;
                }
            }
            4 | 6 => {
                period = (period as i32 + self.vibrato_delta(c)).clamp(113, 856) as u16;
                if n.cmd == 6 {
                    self.volume_slide(c, n.param);
                    vol = self.ch[c].volume;
                }
            }
            7 => {
                vol = (vol as i32 + self.tremolo_delta(c)).clamp(0, 64) as u8;
            }
            0xA => {
                self.volume_slide(c, n.param);
                vol = self.ch[c].volume;
            }
            0xE => match x {
                9 if y != 0 && self.tick % y as u32 == 0 => self.trigger(paula, c, 0),
                0xC if self.tick == y as u32 => {
                    self.ch[c].volume = 0;
                    vol = 0;
                }
                0xD if self.tick == y as u32 && n.period != 0 => self.trigger(paula, c, 0),
                _ => {}
            },
            _ => {}
        }
        self.set_hw_period(paula, c, period);
        self.set_hw_volume(paula, c, vol);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finetune_zero_matches_table() {
        for (i, &p) in BASE_PERIODS.iter().enumerate() {
            assert_eq!(period_for(i, 0), p);
        }
    }
}
