//! Janne Salmijarvi Optimizer (`js.*`, magic `JS92`; the replay routine of the music disk "A Taste
//! Of U4IA", Megawatts, 1992).
//!
//! Clean-room, written from a behavioural specification and black-box runs of the 68k player's
//! register writes; no original code. A ProTracker-2.x-like player with its own pattern-cell
//! encoding (note as a table offset, whole command and sample bytes), four voices, and an
//! interrupt that runs four times per tick: phase 0 plays the tick, phase 1 switches DMA on,
//! phase 2 writes the loop registers, phase 3 does nothing. Every register write is placed at the
//! cycle of the specification's cost model (entry cost, per-voice helper costs, per-command costs).
//!
//! The period table is the standard ProTracker one (the repository's own, as used by the Multimod
//! port) with the specification's one different entry (finetune row 9, note 23: 230).
//!
//! Choices where the specification is silent (never a hang or a panic): a command byte above 15
//! does nothing, a sample number above 31 is ignored, table lookups outside the table give 0 and
//! finetunes use their low four bits, an invert loop on a voice with no sample changes nothing.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Janne Salmijarvi Optimizer", detect, new };

include!("multimod_tables.rs");

const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24,
];
const INVERT: [u8; 16] = [0, 5, 6, 7, 8, 10, 11, 13, 16, 19, 22, 26, 32, 43, 64, 128];

/// Voice-dependent helper costs (68000 cycles) by voice.
const PB: [u64; 4] = [79, 110, 141, 149];
const VO: [u64; 4] = [192, 215, 238, 228];
const VA: [u64; 4] = [113, 144, 175, 183];
const SB: [u64; 4] = [133, 164, 195, 203];
const PL: [u64; 4] = [38, 34, 34, 16];

/// The interrupt period in colour clocks for tempo `t`.
fn interrupt_clocks(t: u8) -> u64 {
    (1773375 / (4 * t.max(1) as u64) + 1) * 5
}

/// The flat period table: row-major 16 x 36, with the specification's one difference.
fn table() -> [u16; 576] {
    let mut t = [0u16; 576];
    for (f, row) in PERIODS.iter().enumerate() {
        t[f * 36..f * 36 + 36].copy_from_slice(row);
    }
    t[9 * 36 + 23] = 230;
    t
}

/// The table word at byte offset `o` from one word before row 0 (offset 0 is the special word).
fn table_at(t: &[u16; 576], o: usize) -> u16 {
    match o / 2 {
        0 => 0x4080,
        k => t.get(k - 1).copied().unwrap_or(0),
    }
}

/// The derived layout: pattern count and the start of the sample data (offsets in the file).
fn layout(d: &[u8]) -> Option<(usize, usize)> {
    if d.len() <= 2112 || d.get(1080..1084)? != b"JS92" {
        return None;
    }
    let n = d[951] as usize + 1;
    let mut max = 0i32;
    for i in 0..n {
        let b = *d.get(952 + i)? as i8 as i32;
        if b > max {
            max = b;
        }
    }
    let p = (1 + max) as usize;
    let data = 1084 + 1024 * p;
    let mut total = 0usize;
    for s in 0..31 {
        total += 2 * u16::from_be_bytes([d[20 + 30 * s + 22], d[20 + 30 * s + 23]]) as usize;
    }
    if data + total > d.len() {
        return None;
    }
    Some((p, data))
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

#[derive(Clone, Copy, Default)]
struct Voice {
    cell: [u8; 4],
    start: u32,
    len: u16,
    loop_start: u32,
    loop_len: u16,
    period: u16,
    fine: u8,
    vol: u8,
    bit: u16,
    dir: u8,
    speed: u8,
    target: u16,
    vib_cmd: u8,
    vib_pos: u8,
    trem_cmd: u8,
    trem_pos: u8,
    wave: u8,
    gliss: u8,
    offs: u8,
    loop_row: u16,
    loop_cnt: u8,
    inv_acc: u8,
    inv_ptr: u32,
    last_note: u8,
}

struct Js {
    d: Vec<u8>,
    /// Absolute address of the sample data (and of the empty-loop word).
    data: u32,
    title: String,
    song_len: u8,
    sample: [(u32, u16, u8, u8, u16, u16); 31],
    v: [Voice; 4],
    speed: u8,
    counter: u8,
    pos: u8,
    row_off: u16,
    break_row: u16,
    jump: bool,
    loop_jump: bool,
    slide_mask: u8,
    delay: u8,
    delay_cnt: u8,
    note_mask: u16,
    retrig_mask: u16,
    phase: u8,
    tempo: u8,
    pra: u8,
    songend: bool,
    ptab: [u16; 576],
    // the clock of the interrupt being run, and the cycles charged since its entry
    e: u64,
    t: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (pats, data) = layout(d)?;
    let title = String::from_utf8_lossy(&d[..20]).trim_end_matches('\0').trim_end().to_string();
    let mut sample = [(0u32, 0u16, 0u8, 0u8, 0u16, 0u16); 31];
    let mut at = base + data as u32;
    for (s, e) in sample.iter_mut().enumerate() {
        let h = 20 + 30 * s;
        let w = |o: usize| u16::from_be_bytes([d[h + o], d[h + o + 1]]);
        *e = (at, w(22), d[h + 24], d[h + 25], w(26), w(28));
        at = at.wrapping_add(2 * w(22) as u32);
    }
    // the first word of the sample data plays as the empty loop
    m.bus.w16(base + data as u32, 0);
    let mut r = Js {
        d: d.to_vec(),
        data: base + data as u32,
        title,
        song_len: d[950],
        sample,
        v: [Voice::default(); 4],
        speed: 6,
        counter: 0,
        pos: 0,
        row_off: 0,
        break_row: 0,
        jump: false,
        loop_jump: false,
        slide_mask: 0,
        delay: 0,
        delay_cnt: 0,
        note_mask: 0,
        retrig_mask: 0,
        phase: 0,
        tempo: 125,
        pra: 0,
        songend: false,
        ptab: table(),
        e: 0,
        t: 0,
    };
    for (i, v) in r.v.iter_mut().enumerate() {
        v.bit = 1 << i;
    }
    // the reference host's start: two host volume calls (all four to 0), then the player's init
    // (port A bit 1 set, volumes 0, all DMA off); the first interrupt follows (`timer_delay`)
    let mut hw = Hw { bus: &mut m.bus };
    // The first host volume call comes 2 974 colour clocks after the start, plus a few colour
    // clocks that depend on the module: fitted to the seven reference runs (all equal to the
    // cc), 6 per pattern and 8 per sample with data, measured from a module of two patterns
    // and four samples.
    let (pats, samples) = (pats as i64, r.sample.iter().filter(|s| s.1 != 0).count() as i64);
    let c0 = (hw.bus.clock as i64 + 2974 + 6 * pats + 8 * samples - 44) as u64;
    for (k, gap) in [0u64, 128, 268, 403, 648, 776, 916, 1051].into_iter().enumerate() {
        hw.bus.clock = c0 + gap;
        hw.vol(k & 3, 0);
    }
    // (the init call follows the host calls by 1 cc less per pattern, from a module of two)
    hw.bus.clock = (c0 as i64 + 1077 + 3780 + 2 - pats) as u64;
    r.pra = hw.bus.read_byte(0xBFE001) | 0x02;
    hw.bus.write_byte(0xBFE001, r.pra);
    hw.bus.clock += 230;
    for c in 0..4 {
        hw.vol(c, 0);
        hw.bus.clock += 8;
    }
    hw.dmacon(0x000F);
    Some(Box::new(r))
}

impl Js {
    // --- the clock

    fn c(&mut self, n: u64) {
        self.t += n;
    }

    fn stamp(&self, hw: &mut Hw) {
        hw.bus.clock = self.e + self.t / 2;
    }

    fn w_loc(&self, hw: &mut Hw, ch: usize, a: u32) {
        self.stamp(hw);
        hw.loc(ch, a);
    }

    fn w_len(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.len(ch, n);
    }

    fn w_per(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.per(ch, n);
    }

    fn w_dma(&self, hw: &mut Hw, n: u16) {
        self.stamp(hw);
        hw.dmacon(n);
    }

    /// The volume output: VO cycles to the AUDxVOL write (value masked as the host's default
    /// volume scale gives), VA after it.
    fn vol_out(&mut self, hw: &mut Hw, v: usize, val: u8) {
        self.t += VO[v];
        self.stamp(hw);
        hw.vol(v, (val & 0x7F) as u16);
        self.t += VA[v];
    }

    fn tab(&self, o: usize) -> u16 {
        table_at(&self.ptab, o)
    }

    fn note_period(&self, note: u8, fine: u8) -> u16 {
        self.tab(note as usize + 72 * (fine & 15) as usize)
    }

    // --- songs and rows

    fn next_position(&mut self) {
        self.row_off = self.break_row.wrapping_mul(16);
        self.break_row = 0;
        self.jump = false;
        self.pos = self.pos.wrapping_add(1) & 0x7F;
        if self.pos >= self.song_len {
            self.pos = 0;
            self.songend = true;
        }
    }

    fn advance_row(&mut self) {
        self.row_off = self.row_off.wrapping_add(16);
        if self.delay != 0 {
            self.delay_cnt = self.delay;
            self.delay = 0;
        }
        if self.delay_cnt != 0 {
            self.delay_cnt -= 1;
            if self.delay_cnt != 0 {
                self.row_off = self.row_off.wrapping_sub(16);
            }
        }
        if self.loop_jump {
            self.loop_jump = false;
            self.row_off = self.break_row.wrapping_mul(16);
            self.break_row = 0;
        }
        if self.row_off >= 1024 {
            self.next_position();
        }
        // bounded: a jump flag is cleared by every position step
        while self.jump {
            self.next_position();
        }
    }

    fn cell_at(&self, v: usize) -> [u8; 4] {
        let pat = self.d.get(952 + self.pos as usize).copied().unwrap_or(0) as usize;
        let o = 1084 + pat * 1024 + self.row_off as usize + v * 4;
        match self.d.get(o..o + 4) {
            Some(b) => [b[0], b[1], b[2], b[3]],
            None => [0; 4],
        }
    }

    fn select_sample(&mut self, v: usize, n: u8) {
        if n == 0 || n > 31 {
            return;
        }
        let (start, len, fine, vol, ls, ll) = self.sample[n as usize - 1];
        let data = self.data;
        let vo = &mut self.v[v];
        vo.start = start;
        vo.len = len;
        vo.fine = fine;
        vo.vol = vol;
        vo.loop_len = ll;
        vo.loop_start = if ll == 1 { data } else { start.wrapping_add(2 * ls as u32) };
        vo.inv_ptr = vo.loop_start;
    }

    // --- invert loop

    /// One invert-loop step of voice `v`; returns its cost.
    fn invert_step(&mut self, hw: &mut Hw, v: usize) -> u64 {
        let s = (self.v[v].gliss >> 4) as usize;
        if s == 0 {
            return 109;
        }
        let vo = &mut self.v[v];
        vo.inv_acc = vo.inv_acc.wrapping_add(INVERT[s]);
        if vo.inv_acc & 0x80 == 0 {
            return 166;
        }
        vo.inv_acc = 0;
        vo.inv_ptr = vo.inv_ptr.wrapping_add(1);
        let mut wrapped = false;
        if vo.inv_ptr >= vo.loop_start.wrapping_add(2 * vo.loop_len as u32) {
            vo.inv_ptr = vo.loop_start;
            wrapped = true;
        }
        let (p, ls) = (vo.inv_ptr, vo.loop_start);
        if ls != 0 {
            let b = hw.bus.r8(p);
            hw.bus.w8(p, 255 - b);
        }
        if wrapped {
            327
        } else {
            311
        }
    }
}

impl Replayer for Js {
    fn format(&self) -> &'static str {
        "Janne Salmijarvi Optimizer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        interrupt_clocks(self.tempo)
    }

    fn timer_delay(&self) -> u64 {
        8
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.e = hw.bus.clock;
        self.t = 281;
        let phase = self.phase;
        self.phase = (self.phase + 1) & 3;
        match phase {
            0 => self.tick_main(hw),
            1 | 2 => self.dma_phase(hw, phase),
            _ => {}
        }
    }
}

impl Js {
    // --- phases 1 and 2

    fn dma_phase(&mut self, hw: &mut Hw, phase: u8) {
        self.c(42);
        if self.retrig_mask != 0 {
            if phase == 1 {
                self.c(81);
                let m = self.retrig_mask | 0x8000;
                self.w_dma(hw, m);
            } else {
                self.c(76);
                for v in 0..4 {
                    if self.retrig_mask & (1 << v) != 0 {
                        self.c(if v == 0 { 51 } else { 55 });
                        let (a, n) = (self.v[v].loop_start, self.v[v].loop_len);
                        self.w_loc(hw, v, a);
                        self.c(20);
                        self.w_len(hw, v, n);
                    } else {
                        self.c(27);
                    }
                }
                self.retrig_mask = 0;
            }
        } else {
            self.c(23);
            if self.note_mask == 0 {
                return;
            }
            if phase == 1 {
                self.c(73);
                let m = self.note_mask | 0x8000;
                self.w_dma(hw, m);
            } else {
                self.c(76);
                for v in 0..4 {
                    self.c(28);
                    let (a, n) = (self.v[v].loop_start, self.v[v].loop_len);
                    self.w_loc(hw, v, a);
                    self.c(20);
                    self.w_len(hw, v, n);
                }
                self.note_mask = 0;
            }
        }
    }

    // --- phase 0

    fn tick_main(&mut self, hw: &mut Hw) {
        self.c(18);
        self.counter = self.counter.wrapping_add(1);
        if self.counter < self.speed {
            self.c(69);
            for v in 0..4 {
                self.effect_voice(hw, v, self.counter);
            }
            while self.jump {
                self.next_position();
            }
        } else {
            self.counter = 0;
            if self.delay_cnt != 0 {
                self.c(108);
                for v in 0..4 {
                    self.effect_voice(hw, v, 0);
                }
                self.advance_row();
            } else {
                self.c(90);
                self.note_mask = 0;
                for v in 0..4 {
                    self.c(if v == 0 { 188 } else { 34 });
                    self.row_voice(hw, v);
                }
                self.advance_row();
            }
        }
    }

    // --- the effect pass

    fn period_write(&mut self, hw: &mut Hw, v: usize) {
        let p = self.v[v].period;
        self.w_per(hw, v, p);
    }

    fn effect_voice(&mut self, hw: &mut Hw, v: usize, counter: u8) {
        self.c(PL[v]);
        let f = self.invert_step(hw, v);
        self.c(f + 41);
        let cell = self.v[v].cell;
        if cell[2] == 0 && cell[3] == 0 {
            self.c(20);
            self.period_write(hw, v);
            self.c(42 + PB[v] + 28);
            return;
        }
        self.c(50);
        let (cmd, par) = (cell[2], cell[3]);
        match cmd {
            8 | 9 | 0xB | 0xC | 0xD | 0xF => self.c(16),
            0 => {
                self.c(49);
                let step = counter % 3;
                if step == 0 {
                    self.c(34);
                    self.period_write(hw, v);
                } else {
                    self.c(if step == 1 { 59 } else { 49 });
                    let n = if step == 1 { par >> 4 } else { par & 15 } as usize;
                    let mut o = self.v[v].last_note as usize + 2 * n;
                    self.c(85);
                    if o > 72 {
                        o = 72;
                        self.c(4);
                    }
                    let val = self.tab(o + 72 * (self.v[v].fine & 15) as usize);
                    self.c(30);
                    self.w_per(hw, v, val);
                }
                self.c(34 + PB[v] + 28);
            }
            1 | 2 => {
                self.slide(hw, v, cmd == 1, par);
                self.c(42 + PB[v] + 28);
            }
            3 => {
                self.c(23);
                if par != 0 {
                    self.c(28);
                    self.v[v].speed = par;
                    self.v[v].cell[3] = 0;
                }
                self.porta_body(hw, v);
            }
            4 => {
                self.c(23);
                if par != 0 {
                    self.c(31);
                    if par & 15 != 0 {
                        self.c(12);
                        self.v[v].vib_cmd = (self.v[v].vib_cmd & 0xF0) | (par & 15);
                    }
                    self.c(31);
                    if par >> 4 != 0 {
                        self.c(12);
                        self.v[v].vib_cmd = (self.v[v].vib_cmd & 0x0F) | (par & 0xF0);
                    }
                    self.c(12);
                }
                self.vib_body(hw, v);
            }
            5 => {
                self.c(18);
                self.porta_body(hw, v);
                self.c(10);
                self.vol_slide(hw, v, par);
            }
            6 => {
                self.c(18);
                self.vib_body(hw, v);
                self.c(10);
                self.vol_slide(hw, v, par);
            }
            7 => {
                self.c(20);
                self.period_write(hw, v);
                self.c(42 + PB[v] + 12);
                self.tremolo(hw, v, par);
            }
            0xA => {
                self.c(20);
                self.period_write(hw, v);
                self.c(42 + PB[v] + 12);
                self.vol_slide(hw, v, par);
            }
            0xE => {
                self.c(56);
                self.ext(hw, v, counter);
            }
            _ => self.c(16),
        }
    }

    /// Slide up (`up`) or down by the parameter under the slide mask: cost to the period write
    /// included, nothing after it.
    fn slide(&mut self, hw: &mut Hw, v: usize, up: bool, par: u8) {
        let d = (par & self.slide_mask) as u16;
        self.slide_mask = 0xFF;
        self.c(91);
        let vo = &mut self.v[v];
        let mut limited = false;
        if up {
            vo.period = vo.period.wrapping_sub(d);
            if (vo.period as i16) <= 113 {
                vo.period = 113;
                limited = true;
            }
        } else {
            vo.period = vo.period.wrapping_add(d);
            if (vo.period as i16) >= 856 {
                vo.period = 856;
                limited = true;
            }
        }
        if limited {
            self.c(16);
        }
        self.c(20);
        self.period_write(hw, v);
    }

    fn porta_body(&mut self, hw: &mut Hw, v: usize) {
        self.c(23);
        if self.v[v].target == 0 {
            self.c(16);
            return;
        }
        self.c(39);
        let vo = &mut self.v[v];
        let sp = vo.speed as u16;
        if vo.dir == 0 {
            vo.period = vo.period.wrapping_add(sp);
            if (vo.target as i16) > vo.period as i16 {
                self.t += 39;
            } else {
                vo.period = vo.target;
                vo.target = 0;
                self.t += 39 + 46;
            }
        } else {
            vo.period = vo.period.wrapping_sub(sp);
            if (vo.target as i16) < vo.period as i16 {
                self.t += 51;
            } else {
                vo.period = vo.target;
                vo.target = 0;
                self.t += 51 + 36;
            }
        }
        self.c(43);
        let mut val = self.v[v].period;
        if self.v[v].gliss & 15 != 0 {
            self.c(58);
            let row = (self.v[v].fine & 15) as usize;
            let mut hit = None;
            for i in 0..36 {
                if val >= self.ptab[row * 36 + i] {
                    hit = Some(i);
                    break;
                }
            }
            match hit {
                Some(i) => {
                    self.c((i as u64 + 1) * 25 + i as u64 * 23);
                    val = self.ptab[row * 36 + i];
                }
                None => {
                    self.c(36 * 25 + 36 * 23 + 4);
                    val = self.ptab[row * 36 + 35];
                }
            }
            self.c(14);
        }
        self.c(12);
        self.w_per(hw, v, val);
        self.c(34 + PB[v] + 28);
    }

    fn wave_value(&self, wf: u8, i: usize, vib_pos: u8) -> u32 {
        match wf & 3 {
            0 => SINE[i] as u32,
            1 => {
                let r = ((i * 8) & 0xFF) as u32;
                if vib_pos & 0x80 != 0 {
                    255 - r
                } else {
                    r
                }
            }
            _ => 255,
        }
    }

    /// The waveform cost shared by vibrato and tremolo (after the +65 / +79 lead-in).
    fn wave_cost(&mut self, wf: u8, vib_pos: u8) {
        match wf & 3 {
            0 => self.c(14),
            1 => {
                self.c(31 + 23);
                self.c(if vib_pos & 0x80 != 0 { 22 } else { 14 });
            }
            _ => self.c(31 + 18),
        }
    }

    fn vib_body(&mut self, hw: &mut Hw, v: usize) {
        self.c(65);
        let vo = self.v[v];
        let wf = vo.wave & 3;
        let pos = vo.vib_pos;
        self.wave_cost(wf, pos);
        let i = ((pos >> 2) & 31) as usize;
        let val = self.wave_value(wf, i, pos);
        let depth = (vo.vib_cmd & 15) as u32;
        let a = ((val * depth) >> 7) as u16;
        self.c(129);
        let out = if pos & 0x80 != 0 {
            self.c(4);
            vo.period.wrapping_sub(a)
        } else {
            self.c(14);
            vo.period.wrapping_add(a)
        };
        self.c(12);
        self.w_per(hw, v, out);
        self.v[v].vib_pos = pos.wrapping_add((vo.vib_cmd >> 4).wrapping_mul(4));
        self.c(18 + PB[v] + 62);
    }

    fn tremolo(&mut self, hw: &mut Hw, v: usize, par: u8) {
        self.c(23);
        if par != 0 {
            self.c(31);
            if par & 15 != 0 {
                self.c(12);
                self.v[v].trem_cmd = (self.v[v].trem_cmd & 0xF0) | (par & 15);
            }
            self.c(31);
            if par >> 4 != 0 {
                self.c(12);
                self.v[v].trem_cmd = (self.v[v].trem_cmd & 0x0F) | (par & 0xF0);
            }
            self.c(12);
        }
        self.c(79);
        let vo = self.v[v];
        let wf = (vo.wave >> 4) & 3;
        self.wave_cost(wf, vo.vib_pos);
        let i = ((vo.trem_pos >> 2) & 31) as usize;
        let val = self.wave_value(wf, i, vo.vib_pos);
        let a = ((val * (vo.trem_cmd & 15) as u32) >> 6) as i32;
        self.c(131);
        let mut w = if vo.trem_pos & 0x80 != 0 {
            self.c(4);
            vo.vol as i32 - a
        } else {
            self.c(14);
            vo.vol as i32 + a
        };
        self.c(11);
        if w < 0 {
            self.c(4);
            w = 0;
        }
        if w > 64 {
            w = 64;
            self.c(27);
        } else {
            self.c(19);
        }
        self.vol_out(hw, v, w as u8);
        self.c(62);
        self.v[v].trem_pos = vo.trem_pos.wrapping_add((vo.trem_cmd >> 4).wrapping_mul(4));
    }

    fn vol_up(&mut self, hw: &mut Hw, v: usize, x: u8) {
        self.c(43);
        let vo = &mut self.v[v];
        vo.vol = vo.vol.wrapping_add(x);
        if (vo.vol.wrapping_sub(64) as i8) >= 0 {
            vo.vol = 64;
            self.t += 16;
        }
        self.c(20);
        let val = self.v[v].vol;
        self.vol_out(hw, v, val);
        self.c(16);
    }

    fn vol_down(&mut self, hw: &mut Hw, v: usize, y: u8) {
        self.c(27);
        let vo = &mut self.v[v];
        vo.vol = vo.vol.wrapping_sub(y);
        if vo.vol & 0x80 != 0 {
            vo.vol = 0;
            self.t += 16;
        }
        self.c(20);
        let val = self.v[v].vol;
        self.vol_out(hw, v, val);
        self.c(16);
    }

    fn vol_slide(&mut self, hw: &mut Hw, v: usize, par: u8) {
        self.c(45);
        if par >> 4 != 0 {
            self.vol_up(hw, v, par >> 4);
        } else {
            self.c(24);
            self.vol_down(hw, v, par & 15);
        }
    }

    // --- extended commands

    fn ext(&mut self, hw: &mut Hw, v: usize, counter: u8) {
        let par = self.v[v].cell[3];
        let y = par & 15;
        match par >> 4 {
            0 => {
                self.c(36);
                self.pra &= 0xFD;
                self.stamp(hw);
                hw.bus.write_byte(0xBFE001, self.pra);
                self.c(20);
                self.pra |= par;
                self.stamp(hw);
                hw.bus.write_byte(0xBFE001, self.pra);
                self.c(16);
            }
            x @ (1 | 2) => {
                self.c(if x == 1 { 23 } else { 27 });
                if counter != 0 {
                    self.c(16);
                } else {
                    self.c(16);
                    self.slide_mask = 15;
                    self.slide(hw, v, x == 1, par);
                    self.c(42 + PB[v] + 28);
                }
            }
            3 => {
                self.c(72);
                self.v[v].gliss = (self.v[v].gliss & 0xF0) | y;
            }
            4 => {
                self.c(72);
                self.v[v].wave = (self.v[v].wave & 0xF0) | y;
            }
            5 => {
                self.c(48);
                self.v[v].fine = y;
            }
            6 => {
                self.c(27);
                if counter != 0 {
                    self.c(16);
                    return;
                }
                self.c(31);
                if y == 0 {
                    self.c(54);
                    self.v[v].loop_row = self.row_off / 16;
                } else {
                    self.c(23);
                    let vo = &mut self.v[v];
                    let jump;
                    if vo.loop_cnt == 0 {
                        vo.loop_cnt = y;
                        jump = true;
                        self.t += 22 + 52;
                    } else {
                        vo.loop_cnt -= 1;
                        if vo.loop_cnt == 0 {
                            jump = false;
                            self.t += 27 + 16;
                        } else {
                            jump = true;
                            self.t += 27 + 52;
                        }
                    }
                    if jump {
                        self.break_row = self.v[v].loop_row;
                        self.loop_jump = true;
                    }
                }
            }
            7 => {
                self.c(86);
                self.v[v].wave = (self.v[v].wave & 0x0F) | (y << 4);
            }
            9 => {
                self.c(35);
                if y == 0 {
                    self.c(16);
                    return;
                }
                self.c(27);
                let has_note = self.v[v].cell[0] != 0;
                if counter == 0 {
                    self.c(23);
                    if has_note {
                        self.c(16);
                        return;
                    }
                }
                let mut c = counter as i32;
                // bounded: the counter falls by y >= 1 every round
                loop {
                    c -= y as i32;
                    self.c(15);
                    if c == 0 {
                        self.retrigger(hw, v);
                        break;
                    }
                    if c < 0 {
                        self.c(27);
                        break;
                    }
                    self.c(11);
                }
            }
            0xA => {
                self.c(23);
                if counter != 0 {
                    self.c(16);
                } else {
                    self.c(34);
                    self.vol_up(hw, v, y);
                }
            }
            0xB => {
                self.c(23);
                if counter != 0 {
                    self.c(16);
                } else {
                    self.c(34);
                    self.vol_down(hw, v, y);
                }
            }
            0xC => {
                self.c(47);
                if counter != y {
                    self.c(16);
                } else {
                    self.c(32);
                    self.v[v].vol = 0;
                    self.vol_out(hw, v, 0);
                    self.c(28);
                }
            }
            0xD => {
                self.c(47);
                if counter != y {
                    self.c(16);
                } else {
                    self.c(23);
                    if self.v[v].cell[0] == 0 {
                        self.c(16);
                    } else {
                        self.c(10);
                        self.retrigger(hw, v);
                    }
                }
            }
            0xE => {
                self.c(23);
                if counter != 0 {
                    self.c(16);
                } else {
                    self.c(47);
                    if self.delay_cnt != 0 {
                        self.c(16);
                    } else {
                        self.c(32);
                        self.delay = y + 1;
                    }
                }
            }
            0xF => {
                self.c(23);
                if counter != 0 {
                    self.c(16);
                } else {
                    self.c(85);
                    self.v[v].gliss = (self.v[v].gliss & 0x0F) | (y << 4);
                    if y == 0 {
                        self.c(16);
                    } else {
                        let f = self.invert_step(hw, v);
                        self.c(f);
                    }
                }
            }
            _ => self.c(16),
        }
    }

    fn retrigger(&mut self, hw: &mut Hw, v: usize) {
        self.c(24);
        let bit = self.v[v].bit;
        self.w_dma(hw, bit);
        self.c(24);
        let vo = self.v[v];
        self.w_loc(hw, v, vo.start);
        self.c(20);
        self.w_len(hw, v, vo.len);
        self.c(20);
        self.w_per(hw, v, vo.period);
        self.c(SB[v] + 42 + PB[v] + 12 + 20);
        self.vol_out(hw, v, vo.vol);
        self.c(44);
        self.retrig_mask |= bit;
    }

    // --- the row pass

    fn row_voice(&mut self, hw: &mut Hw, v: usize) {
        self.c(57);
        let cell = self.cell_at(v);
        self.v[v].cell = cell;
        if cell == [0; 4] {
            self.c(16);
            return;
        }
        self.c(27);
        if cell[1] != 0 {
            self.c(209);
            self.select_sample(v, cell[1]);
            self.c(if self.v[v].loop_len == 1 && cell[1] <= 31 { 66 } else { 102 });
        }
        self.c(23);
        let (note, cmd, par) = (cell[0], cell[2], cell[3]);
        if note == 0 {
            self.row_cmds(hw, v);
            return;
        }
        self.c(45);
        if cmd == 0xE && par >> 4 == 5 {
            self.c(66 + 10);
            self.v[v].fine = par & 15;
            self.note_start(hw, v, note);
            return;
        }
        self.c(31);
        if cmd == 3 {
            self.porta_setup(hw, v, note);
            return;
        }
        self.c(19);
        if cmd == 5 {
            self.porta_setup(hw, v, note);
            return;
        }
        self.c(19);
        if cmd == 9 {
            self.c(18);
            self.row_cmds(hw, v);
            self.c(10);
        }
        self.note_start(hw, v, note);
    }

    fn porta_setup(&mut self, hw: &mut Hw, v: usize, note: u8) {
        self.c(177);
        let target = self.note_period(note, self.v[v].fine);
        let vo = &mut self.v[v];
        if target == vo.period {
            vo.dir = 0;
            vo.target = 0;
            self.t += 32;
        } else if (target as i16) > vo.period as i16 {
            vo.dir = 0;
            vo.target = target;
            self.t += 27;
        } else {
            vo.dir = 1;
            vo.target = target;
            self.t += 43;
        }
        self.c(18);
        self.row_cmds(hw, v);
        self.c(20);
        let val = self.v[v].vol;
        self.vol_out(hw, v, val);
        self.c(16);
    }

    fn note_start(&mut self, hw: &mut Hw, v: usize, note: u8) {
        self.c(141);
        self.v[v].last_note = note;
        self.v[v].period = self.note_period(note, self.v[v].fine);
        let cell = self.v[v].cell;
        if cell[2] == 0xE && cell[3] >> 4 == 0xD {
            self.row_cmds(hw, v);
            return;
        }
        self.c(24);
        let bit = self.v[v].bit;
        self.w_dma(hw, bit);
        if self.v[v].wave & 4 == 0 {
            self.v[v].vib_pos = 0;
            self.c(43);
        } else {
            self.c(27);
        }
        if self.v[v].wave & 0x40 == 0 {
            self.v[v].trem_pos = 0;
            self.c(43);
        } else {
            self.c(27);
        }
        self.c(24);
        let vo = self.v[v];
        self.w_loc(hw, v, vo.start);
        self.c(20);
        self.w_len(hw, v, vo.len);
        self.c(20);
        self.w_per(hw, v, vo.period);
        self.c(SB[v] + 42 + PB[v] + 12 + 20);
        self.vol_out(hw, v, vo.vol);
        self.c(38);
        self.note_mask |= bit;
        self.row_cmds(hw, v);
    }

    fn row_cmds(&mut self, hw: &mut Hw, v: usize) {
        self.c(18);
        let f = self.invert_step(hw, v);
        self.c(f + 50);
        let (cmd, par) = (self.v[v].cell[2], self.v[v].cell[3]);
        match cmd {
            0..=8 | 0xA => {
                self.c(20);
                self.period_write(hw, v);
                self.c(42 + PB[v] + 28);
            }
            9 => {
                self.c(27);
                if par != 0 {
                    self.c(12);
                    self.v[v].offs = par;
                }
                self.c(55);
                let o = self.v[v].offs as i32 * 128;
                if o < self.v[v].len as i16 as i32 {
                    self.v[v].len -= o as u16;
                    self.v[v].start = self.v[v].start.wrapping_add(2 * o as u32);
                    self.c(60);
                } else {
                    self.v[v].len = 1;
                    self.c(32);
                }
            }
            0xB => {
                self.c(225);
                self.pos = par.wrapping_sub(1);
                self.break_row = 0;
                self.jump = true;
                self.songend = true;
            }
            0xC => {
                self.c(35);
                let mut x = par;
                if x > 64 {
                    x = 64;
                    self.c(4);
                }
                self.v[v].vol = x;
                self.c(20);
                self.vol_out(hw, v, x);
                self.c(16);
            }
            0xD => {
                self.c(52);
                self.break_row = par as u16;
                self.jump = true;
            }
            0xE => {
                self.c(56);
                self.ext(hw, v, 0);
            }
            0xF => {
                self.c(27);
                if par == 0 {
                    self.c(16);
                } else if par < 0x20 {
                    self.c(63);
                    self.speed = par;
                    self.counter = 0;
                } else {
                    self.c(345);
                    self.tempo = par;
                }
            }
            _ => self.c(16),
        }
    }
}

#[cfg(test)]
#[path = "jannesalmijarvi_tests.rs"]
mod tests;
