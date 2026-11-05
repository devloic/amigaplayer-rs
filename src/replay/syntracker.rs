//! SynTracker (`st.*`, `synmod.*`; the Eagleplayer "SynTracker"): a four-channel synthesis
//! tracker. Each channel plays a 32-byte looping waveform that is rebuilt every tick
//! (wavetable "scanning" and low-pass "morphing"), or a plain 8-bit sample.
//!
//! Clean-room, written from a behavioural specification (drawn up from register-write and
//! audio-DMA logs of UADE's SynTracker player on synthetic modules) and checked against
//! those logs; no original code. The specification's model reproduces every write (address,
//! value, order) of 17 synthetic modules; the port was written from the text alone.
//!
//! A module starts with `SYNTRACKER-SONG:`, then the song length and highest pattern number,
//! 32 text fields, 32 instrument records, four lists of pattern numbers and four of
//! transpositions (one byte per position), the 128-byte patterns (32 rows of note,
//! instrument, effect letter, parameter) and, behind them, the instrument data, vibrato
//! tables and scan windows. The player runs once per frame; a row tick comes every `speed`
//! frames (initially 6).
//!
//! The chip buffer of the original (a zero area, four working and four playing waveforms of
//! 32 bytes) is allocated here; Paula plays the playing waveforms, to which the working ones
//! are copied at the end of every tick.
//!
//! Decisions where the specification leaves the choice to the port:
//! * Timing inside a tick follows the specification's fitted gap tables (section 10); the
//!   cost of the row processing is a per-channel constant fitted to the observed spaces
//!   between the start writes (10.3), not modelled per effect.
//! * Reads beyond the module (a pattern, instrument or table past its end) give 0.
//! * Reads past the specification's tables (note indices 88 and above) give 0.
//! * The vibrato table search that does not find the period (an arpeggio past the table's
//!   end) leaves the period as it is.
//! * The song-end flag is raised when the position wraps; the player goes on.

use super::{be16, be32, Hw, Open, Replayer, DT_TICK};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "SynTracker", detect, new };

const MAGIC: &[u8; 16] = b"SYNTRACKER-SONG:";

/// Offsets in the file.
const O_INSTR: usize = 0x414;
const O_PATS: usize = 0x614;
const O_TRANS: usize = 0x814;
const O_PATDATA: usize = 0xA14;

/// The period table, one entry per semitone (spec section 3).
const PT: [u16; 72] = [
    0x1ab0, 0x1940, 0x17d0, 0x1670, 0x1530, 0x1400, 0x12e0, 0x11d0, 0x10d0, 0x0fe0, 0x0f00, 0x0e20, 0x0d58, 0x0ca0, 0x0be8,
    0x0b38, 0x0a98, 0x0a00, 0x0970, 0x08e8, 0x0868, 0x07f0, 0x0780, 0x0710, 0x06ac, 0x0650, 0x05f4, 0x059c, 0x054c, 0x0500,
    0x04b8, 0x0474, 0x0434, 0x03f8, 0x03c0, 0x0388, 0x0356, 0x0328, 0x02fa, 0x02ce, 0x02a6, 0x0280, 0x025c, 0x023a, 0x021a,
    0x01fc, 0x01e0, 0x01c4, 0x01ab, 0x0194, 0x017d, 0x0167, 0x0153, 0x0140, 0x012e, 0x011d, 0x010d, 0x00fe, 0x00f0, 0x00e2,
    0x00d6, 0x00ca, 0x00be, 0x00b4, 0x00a9, 0x00a0, 0x0097, 0x008f, 0x0086, 0x007f, 0x0078, 0x0071,
];

/// The built-in vibrato wave.
const SINE: [u8; 32] = [
    0x00, 0x18, 0x2f, 0x45, 0x58, 0x68, 0x74, 0x7b, 0x7e, 0x7d, 0x77, 0x6c, 0x5e, 0x4c, 0x37, 0x20, 0x08, 0xf1, 0xd9, 0xc3, 0xaf,
    0x9e, 0x90, 0x87, 0x82, 0x82, 0x86, 0x8f, 0x9c, 0xad, 0xc1, 0xd7,
];

/// The colour clocks of the host's wait between the start and the first timer interrupt,
/// fitted to the reference run (start sequence 11,864, first tick 83,103).
const TIMER_DELAY: u64 = 299;

// ---- timing constants (colour clocks), spec section 10
/// Row processing: a channel without a start, before a start, after a start's DMACON, and
/// the tail after the fourth channel (fitted to 10.3).
const ROW_IDLE: f64 = 323.0;
const ROW_PRE: f64 = 387.0;
const ROW_POST: f64 = 330.0;
const ROW_TAIL: f64 = 325.0;
/// PER gaps by channel (1..3): no slot change, slot 0, slot 1 or 2 selected.
const PER_GAP: [[f64; 4]; 4] = [[0.0; 4], [155.7, 194.0, 208.0, 208.0], [171.0, 209.0, 223.0, 223.0], [186.7, 225.0, 238.0, 238.0]];
const VIB_COST: f64 = 143.0;
const VIB_NEG: f64 = 9.0;
const VIB_STEP: f64 = 8.5;
const VOL0_BASE: f64 = 283.2;
const ENV_NONE: f64 = 99.5;
const ENV_DONE: f64 = 153.0;
const ENV_ATTACK: f64 = 245.0;
const ENV_DECAY: f64 = 228.5;
const ENV_RELEASE: f64 = 241.0;
const ENV_SWITCH: f64 = 77.5;

fn pt(i: i32) -> u16 {
    if (0..72).contains(&i) {
        PT[i as usize]
    } else if (72..88).contains(&i) {
        let j = 2 * (i - 72) as usize;
        u16::from_be_bytes([SINE[j], SINE[j + 1]])
    } else {
        0
    }
}

pub fn detect(d: &[u8]) -> bool {
    d.len() >= O_PATDATA + 128 && &d[..16] == MAGIC
}

/// Where a channel's vibrato wave comes from.
#[derive(Clone, Copy, PartialEq)]
enum Wave {
    Sine,
    /// A page: the 32 bytes at this file offset.
    Page(usize),
}

#[derive(Clone)]
struct Chan {
    /// Note index remembered, 0..127.
    nidx: u8,
    /// The remembered instrument byte (1..32), 0 = none.
    inst: u8,
    slots: [u16; 3],
    arp_state: u8,
    arp_cnt: u16,
    arp_rate: u8,
    cur: u16,
    glide: u16,
    slide: u16,
    vwave: Wave,
    vidx: u8,
    vspeed: u8,
    vdepth: u8,
    volume: i32,
    fade: i32,
    /// File offset of the synth block and of the base wave, if a synth instrument plays.
    blk: Option<usize>,
    wave: usize,
    env_phase: u8,
    env_cnt: u16,
    level: u8,
    mph_phase: u8,
    mph_cnt: u16,
    scan_dir: i8,
    scan_pos: u8,
    scan_cnt: u8,
    scan_per: u8,
    first: bool,
    /// Pending Paula loads: (address, length in words).
    load: Option<(u32, u16)>,
    load2: Option<(u32, u16)>,
}

impl Chan {
    fn new() -> Chan {
        Chan {
            nidx: 0,
            inst: 0,
            slots: [0; 3],
            arp_state: 0,
            arp_cnt: 0,
            arp_rate: 1,
            cur: 0,
            glide: 0,
            slide: 0,
            vwave: Wave::Sine,
            vidx: 0,
            vspeed: 0,
            vdepth: 0,
            volume: 0,
            fade: 0,
            blk: None,
            wave: 0,
            env_phase: 8,
            env_cnt: 0,
            level: 0,
            mph_phase: 8,
            mph_cnt: 0,
            scan_dir: 1,
            scan_pos: 0,
            scan_cnt: 0,
            scan_per: 0,
            first: false,
            load: None,
            load2: None,
        }
    }
}

/// Counts that the end-of-tick work's cost depends on (10.1).
#[derive(Default)]
struct Work {
    nblk: u32,
    ncount: u32,
    nadv: u32,
    nfilt: u32,
    nfirst: u32,
    nupd: u32,
    ntrans: u32,
    nhi: u32,
    nlo: u32,
}

pub struct SynTracker {
    d: Vec<u8>,
    base: u32,
    /// The chip buffer: zero area, then four working and four playing waveforms.
    buf: u32,
    /// The start of the data area.
    dd: usize,
    npos: usize,
    ch: [Chan; 4],
    master: [i32; 4],
    speed: u8,
    tickc: u8,
    pos: usize,
    row: usize,
    jump: u16,
    /// Leftover upper half of the glide divide (8.1).
    upper: u32,
    work: [u8; 128],
    songend: bool,
    frac: f64,
}

impl SynTracker {
    fn byte(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn word(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// Lets `cc` colour clocks pass, carrying the fraction.
    fn wt(&mut self, hw: &mut Hw, cc: f64) {
        self.frac += cc;
        let n = (self.frac + 0.5).floor();
        self.frac -= n;
        if n > 0.0 {
            hw.wait(n as u64);
        }
    }

    /// The period table with the out-of-range read below the lowest note (8.2): the pending
    /// loop-length word of channel 3.
    fn ptx(&self, i: i32) -> u16 {
        if i == -1 {
            self.ch[3].load2.map_or(0, |l| l.1)
        } else {
            pt(i)
        }
    }

    fn zero_area(&self) -> u32 {
        self.buf
    }

    fn playing(&self, k: usize) -> u32 {
        self.buf + 0xA0 + 32 * k as u32
    }

    /// Instrument record field.
    fn rec(&self, j: usize, o: usize) -> usize {
        O_INSTR + 16 * j + o
    }

    fn inst_vol(&self, inst: u8) -> i32 {
        if inst == 0 {
            return self.word(O_INSTR + 16 * 255 + 6) as i32;
        }
        self.word(self.rec(inst as usize - 1, 6)) as i32
    }

    fn vib_byte(&self, c: &Chan) -> i8 {
        let i = c.vidx as usize & 31;
        match c.vwave {
            Wave::Sine => SINE[i] as i8,
            Wave::Page(o) => self.byte(o + i) as i8,
        }
    }

    // ------------------------------------------------------------------ instrument start

    fn start(&mut self, k: usize, inst: u8, hw: &mut Hw) {
        hw.dmacon(1 << k);
        let j = inst as usize - 1;
        let len = self.word(self.rec(j, 4));
        let vol = self.word(self.rec(j, 6)) as i32;
        let off = be32(&self.d, self.rec(j, 0)) as usize;
        let data = self.dd.wrapping_add(off);
        let load;
        let mut load2 = None;
        let mut blk = None;
        if j < 16 {
            load = Some((self.playing(k), len));
        } else if len != 0 {
            let first = self.base.wrapping_add((data + self.word(self.rec(j, 8)) as usize) as u32);
            load = Some((first, len));
            let ll = self.word(self.rec(j, 12));
            load2 = Some(if ll != 0 {
                (self.base.wrapping_add((data + self.word(self.rec(j, 10)) as usize) as u32), ll)
            } else {
                (self.zero_area(), 1)
            });
        } else {
            hw.vol(k, 0);
            load = Some((self.zero_area(), 1));
        }
        if j < 16 {
            blk = Some(data + 0x20);
        }
        let (vwave, vspeed, vdepth, per, page);
        if let Some(b) = blk {
            let v = self.byte(b + 0x10);
            vspeed = v >> 4;
            vdepth = v & 15;
            page = Some(self.dd.wrapping_add(64 * self.byte(b + 0x12) as usize));
            per = self.byte(b + 0x11);
            vwave = page.map(Wave::Page);
        } else {
            vspeed = 0;
            vdepth = 0;
            per = 0;
            vwave = None;
        }
        let startpos = blk.map_or(0, |b| self.byte(b + 0x14));
        let c = &mut self.ch[k];
        c.load = load;
        c.load2 = load2;
        if j < 16 || len != 0 {
            c.volume = (63 * vol) >> 6;
        }
        c.fade = 0;
        c.env_cnt = 0;
        c.level = 0;
        c.mph_cnt = 0;
        c.blk = blk;
        c.wave = data;
        if blk.is_some() {
            c.env_phase = 0;
            c.mph_phase = 0;
            c.vspeed = vspeed;
            c.vdepth = vdepth;
            if let Some(w) = vwave {
                c.vwave = w;
            }
            c.scan_dir = 1;
            c.scan_pos = startpos;
            c.scan_per = per;
            c.scan_cnt = if per != 0 { per.wrapping_sub(1) } else { 0 };
            c.first = true;
        } else {
            c.env_phase = 8;
            c.mph_phase = 8;
            c.vspeed = 0;
            c.vdepth = 0;
            c.scan_per = 0;
            c.first = false;
        }
    }

    // ------------------------------------------------------------------ rows

    fn row_part(&mut self, hw: &mut Hw) {
        if self.jump != 0xFFFF {
            self.pos = self.jump as usize;
            self.row = 0;
            self.jump = 0xFFFF;
        }
        self.upper = 0;
        for k in 0..4 {
            self.row_channel(k, hw);
        }
        self.wt(hw, ROW_TAIL);
        self.row += 1;
        if self.row >= 32 {
            self.row = 0;
            self.pos += 1;
            if self.pos >= self.npos {
                self.pos = 0;
                self.songend = true;
            }
        }
    }

    fn pat_row(&self, k: usize, row: usize) -> [u8; 4] {
        let pat = self.byte(O_PATS + 0x80 * k + self.pos) as usize;
        let o = O_PATDATA + 128 * pat + 4 * row;
        [self.byte(o), self.byte(o + 1), self.byte(o + 2), self.byte(o + 3)]
    }

    fn row_channel(&mut self, k: usize, hw: &mut Hw) {
        let [note, inst, eff, par] = self.pat_row(k, self.row);
        let tr = self.byte(O_TRANS + 0x80 * k + self.pos) as i8;
        let mut started = false;
        if (0x40..=0x7F).contains(&inst) {
            if inst <= 0x4F {
                let j = (inst - 0x40) as usize;
                let off = be32(&self.d, self.rec(j, 0)) as usize;
                let data = self.dd.wrapping_add(off);
                let c = &mut self.ch[k];
                c.blk = Some(data + 0x20);
                c.wave = data;
                c.first = true;
            }
        } else if note != 0 {
            let idx = (note as i32 + tr as i32 - 1) as u8;
            let idx = if idx & 0x80 != 0 { 0 } else { idx };
            let base = pt(idx as i32);
            {
                let c = &mut self.ch[k];
                c.nidx = idx;
                c.slots = [base; 3];
                c.cur = base;
                c.glide = 0;
                c.slide = 0;
            }
            if inst != 0xFF {
                let mut st = None;
                if inst == 0 {
                    if self.ch[k].inst != 0 {
                        st = Some(self.ch[k].inst);
                    } else {
                        st = Some(0);
                    }
                } else if inst <= 0x20 {
                    let c = &mut self.ch[k];
                    c.vwave = Wave::Sine;
                    c.vidx = 0;
                    c.vdepth = 0;
                    c.inst = inst;
                    st = Some(inst);
                }
                if let Some(i) = st {
                    // instrument 0 before any was set: record 255, undefined; start record 31
                    let i = if i == 0 { 32 } else { i };
                    self.wt(hw, ROW_PRE);
                    self.start(k, i, hw);
                    self.wt(hw, ROW_POST);
                    started = true;
                }
            }
        }
        if !started {
            self.wt(hw, ROW_IDLE);
        }
        self.effect(k, inst, eff, par, tr, hw);
    }

    fn effect(&mut self, k: usize, inst: u8, eff: u8, p: u8, tr: i8, hw: &mut Hw) {
        let nidx = self.ch[k].nidx as i32;
        match eff {
            b'A' => {
                if nidx != 0 {
                    let c = &mut self.ch[k];
                    c.slots[1] = pt(nidx + (p & 15) as i32);
                    c.slots[2] = pt(nidx + (p >> 4) as i32);
                }
            }
            b'L' => {
                let v = self.inst_vol(self.ch[k].inst);
                self.ch[k].volume = (p as i32 * v) >> 6;
            }
            b'M' => self.ch[k].scan_per = p,
            b'V' => {
                let c = &mut self.ch[k];
                c.vspeed = p >> 4;
                c.vdepth = p & 15;
                c.vwave = Wave::Sine;
            }
            b'F' => {
                // the LED filter: bit 1 of CIA-A port A
                let x = hw.bus.read_byte(0xBFE001);
                hw.bus.write_byte(0xBFE001, if p == 0 { x | 2 } else { x & !2 });
            }
            b'S' => {
                if p != 0 {
                    self.speed = p;
                }
            }
            b'O' => {
                if p != 0 {
                    let c = &mut self.ch[k];
                    c.arp_rate = p;
                    c.arp_cnt = 0;
                }
            }
            b'B' => {
                if p == 0 {
                    let t = self.pos + 1;
                    self.jump = if t >= self.npos { 0 } else { t as u16 };
                } else if (p as usize) < self.npos {
                    self.jump = p as u16;
                } else {
                    self.jump = 0xFFFF;
                }
            }
            b'U' => {
                let s2 = self.ch[k].slots[2] as i32;
                if s2 - p as i32 >= 0x39 {
                    let c = &mut self.ch[k];
                    c.slide = c.slide.wrapping_sub(p as u16);
                }
            }
            b'D' => {
                let b = self.ch[k].slots[0] as i32;
                if b + p as i32 <= 0x1AB0 {
                    let c = &mut self.ch[k];
                    c.slide = c.slide.wrapping_add(p as u16);
                }
            }
            b'G' => {
                let v = self.inst_vol(self.ch[k].inst);
                if inst == 0 {
                    self.ch[k].volume = (63 * v) >> 6;
                }
                self.ch[k].fade = (p as i32 * v) >> 6;
            }
            b'E' => self.glide_effect(k, tr),
            _ => {}
        }
    }

    /// Effect E: glide to the next note of the pattern; needs a note on this row.
    fn glide_effect(&mut self, k: usize, tr: i8) {
        let [note, ..] = self.pat_row(k, self.row);
        if note == 0 {
            return;
        }
        let Some(ahead) = (self.row + 1..32).find(|&r| self.pat_row(k, r)[0] != 0) else {
            return;
        };
        let n = self.pat_row(k, ahead)[0];
        let rows = (ahead - self.row) as u32;
        let d = (rows * self.speed as u32).max(1);
        let t = pt((n as i32 + tr as i32 - 1) as u8 as i32) as u32;
        let cur = self.ch[k].slots[0] as u32;
        let g: u16 = if cur <= t {
            ((t - cur).div_ceil(d)) as u16
        } else {
            let mut x = self.upper.wrapping_mul(65536).wrapping_add(cur - t);
            if self.upper < d {
                x = ((x % d) << 16) | (x / d) & 0xFFFF;
            }
            if x > 0xFFFF {
                x = (x & 0xFFFF_0000) | ((x & 0xFFFF).wrapping_add(1) & 0xFFFF);
            }
            self.upper = x >> 16;
            ((x & 0xFFFF) as u16).wrapping_neg()
        };
        self.ch[k].glide = g;
    }

    // ------------------------------------------------------------------ the per-tick part

    /// The period of channel `k` and the cost of getting to it (the gap before its write).
    fn period(&mut self, k: usize) -> (u16, f64) {
        let mut ev = 0usize;
        let mut vib = 0.0;
        let c = &mut self.ch[k];
        c.arp_cnt = c.arp_cnt.wrapping_add(1);
        if c.arp_cnt & 0xFF == c.arp_rate as u16 {
            c.arp_cnt &= 0xFF00;
            ev = match c.arp_state {
                0 => 1,
                1 => 2,
                _ => 3,
            };
            c.cur = c.slots[c.arp_state as usize];
            c.arp_state = (c.arp_state + 1) % 3;
        }
        let mut p = self.ch[k].cur;
        let d = self.ch[k].vdepth as i32;
        if d != 0 {
            vib = VIB_COST;
            let v = self.vib_byte(&self.ch[k]) as i32;
            let a = (v.abs() * d) >> 7;
            let nidx = self.ch[k].nidx as i32;
            let found = (nidx..nidx + 100).find(|&i| pt(i) == p);
            if let Some(i) = found {
                vib += VIB_STEP * (i - nidx) as f64;
                if v >= 0 {
                    let diff = (p.wrapping_sub(pt(i + 1)) as u32 * a as u32) & 0xFFFF;
                    p = p.wrapping_sub((diff >> 5) as u16);
                } else {
                    vib += VIB_NEG;
                    let diff = (self.ptx(i - 1).wrapping_sub(p) as u32 * a as u32) & 0xFFFF;
                    p = p.wrapping_add((diff >> 5) as u16);
                }
            } else if v < 0 {
                vib += VIB_NEG;
            }
            let c = &mut self.ch[k];
            c.vidx = c.vidx.wrapping_add(c.vspeed) & 31;
        }
        let c = &mut self.ch[k];
        c.slide = c.slide.wrapping_add(c.glide);
        (p.wrapping_add(c.slide), PER_GAP[k][ev] + vib)
    }

    /// Envelope step and output volume of channel `k`; returns the volume and the cost.
    fn envelope(&mut self, k: usize) -> (i32, f64) {
        let c = &mut self.ch[k];
        let mut cost;
        if let Some(b) = c.blk {
            let mut sw = 0.0;
            let mut kind = ENV_DONE;
            while c.env_phase != 8 {
                let ph = c.env_phase as usize;
                let (count, amount) = (self.d.get(b + ph).copied().unwrap_or(0), self.d.get(b + ph + 1).copied().unwrap_or(0));
                if c.env_cnt & 0xFF == count as u16 {
                    c.env_cnt = 0;
                    c.env_phase += 2;
                    sw += ENV_SWITCH;
                    continue;
                }
                c.env_cnt += 1;
                if ph == 0 || ph == 4 {
                    let r = (c.level as i8).wrapping_add(amount as i8);
                    let r = if r > 0x3F { 0x3F } else { r };
                    c.level = r as u8;
                    kind = ENV_ATTACK;
                } else {
                    let r = (c.level as i8).wrapping_sub(amount as i8);
                    c.level = if r < 0 { 0 } else { r as u8 };
                    kind = if ph == 2 { ENV_DECAY } else { ENV_RELEASE };
                }
                break;
            }
            cost = kind + sw;
            let v = ((c.level as i32 & 0x3F) * c.volume) >> 6;
            return ((v * self.master[k]) >> 6, cost);
        }
        cost = ENV_NONE;
        let _ = &mut cost;
        ((c.volume * self.master[k]) >> 6, cost)
    }

    fn morph(&mut self, k: usize, w: &mut Work) {
        let Some(b) = self.ch[k].blk else { return };
        let c = &mut self.ch[k];
        while c.mph_phase != 8 {
            let ph = c.mph_phase as usize;
            let count = self.d.get(b + 8 + ph).copied().unwrap_or(0);
            let coef = self.d.get(b + 9 + ph).copied().unwrap_or(0) as i16;
            if c.mph_cnt & 0xFF == count as u16 {
                c.mph_cnt = 0;
                c.mph_phase += 2;
                w.nadv += 1;
                continue;
            }
            c.mph_cnt += 1;
            let slot = &mut self.work[32 * k..32 * k + 32];
            let mut y: i16 = slot[31] as i16;
            for s in slot.iter_mut() {
                let x = *s as i8 as i16;
                y = y.wrapping_sub(x);
                y = y.wrapping_mul(coef) >> 7;
                y = y.wrapping_add(x);
                *s = y as u8;
            }
            w.nfilt += 1;
            break;
        }
    }

    fn scan(&mut self, k: usize, wrote: &mut usize, w: &mut Work) {
        let Some(b) = self.ch[k].blk else { return };
        let wave = self.ch[k].wave;
        let mut out = [0u8; 32];
        let mut write = false;
        let per = self.ch[k].scan_per;
        if per == 0 {
            if self.ch[k].first {
                for (i, o) in out.iter_mut().enumerate() {
                    *o = self.byte(wave + i);
                }
                self.ch[k].first = false;
                w.nfirst += 1;
                write = true;
            }
        } else {
            self.ch[k].first = false;
            self.ch[k].scan_cnt = self.ch[k].scan_cnt.wrapping_add(1);
            if (per as i8) >= (self.ch[k].scan_cnt as i8) {
                w.ncount += 1;
                return;
            }
            self.ch[k].scan_cnt = 0;
            let lo = self.byte(b + 0x14);
            let hi = self.byte(b + 0x15);
            let act = self.byte(b + 0x16);
            let c = &mut self.ch[k];
            c.scan_pos = c.scan_pos.wrapping_add(c.scan_dir as u8) & 31;
            if c.scan_pos == lo && act & 0x80 != 0 {
                c.scan_dir = 1;
                w.nlo += 1;
            } else if c.scan_pos == hi {
                w.nhi += 1;
                if act & 0x80 != 0 {
                    c.scan_dir = -1;
                } else {
                    c.scan_pos = c.scan_pos.wrapping_add(act) & 31;
                }
            }
            let pos = c.scan_pos as usize;
            let win = self.dd.wrapping_add(64 * self.byte(b + 0x18) as usize);
            for (i, o) in out.iter_mut().enumerate() {
                let wv = self.byte(win + ((pos + i) & 31));
                if wv & 0x80 != 0 {
                    w.ntrans += 1;
                    *o = self.byte(wave + i);
                } else {
                    *o = wv;
                }
            }
            w.nupd += 1;
            write = true;
        }
        if write {
            let pos = 32 * k + 32 * *wrote;
            if pos + 32 <= 128 {
                self.work[pos..pos + 32].copy_from_slice(&out);
            }
            *wrote += 1;
        }
    }

    fn run_tick(&mut self, hw: &mut Hw) {
        // periods
        let mut pers = [(0u16, 0.0f64); 4];
        for (k, p) in pers.iter_mut().enumerate() {
            *p = self.period(k);
        }
        hw.per(0, pers[0].0);
        for (k, p) in pers.iter().enumerate().skip(1) {
            self.wt(hw, p.1);
            hw.per(k, p.0);
        }
        // fades, envelopes and volumes
        for c in self.ch.iter_mut() {
            c.volume = (c.volume - c.fade).max(0);
        }
        for k in 0..4 {
            let (v, cost) = self.envelope(k);
            let gap = if k == 0 { VOL0_BASE + cost } else { cost };
            self.wt(hw, gap);
            hw.vol(k, v as u16);
        }
        // morph and scan
        let mut w = Work::default();
        for k in 0..4 {
            self.morph(k, &mut w);
        }
        let mut wrote = 0usize;
        for k in 0..4 {
            self.scan(k, &mut wrote, &mut w);
        }
        w.nblk = self.ch.iter().filter(|c| c.blk.is_some()).count() as u32;
        let g = 496.0
            + 21.3 * w.nblk as f64
            + 12.3 * w.ncount as f64
            + 74.8 * w.nadv as f64
            + 2227.0 * w.nfilt as f64
            + 364.5 * w.nfirst as f64
            + 1116.0 * w.nupd as f64
            - 1.9 * w.ntrans as f64
            + 27.0 * w.nhi as f64
            + 8.7 * w.nlo as f64;
        // the pending loads, placed so that the chain ends before the DMACON
        let pend: Vec<usize> = (0..4).filter(|&k| self.ch[k].load.is_some()).collect();
        let mut sched: Vec<(usize, f64, f64)> = Vec::new(); // (channel, LC time, LEN time) from the VOL3 write
        if let Some(&last) = pend.last() {
            let mut len_t = g - (45.0 + 27.5 * (3 - last) as f64);
            let mut prev = last;
            for &k in pend.iter().rev() {
                if k != last {
                    len_t = sched.last().unwrap().1 - (57.5 + 27.5 * (prev - k - 1) as f64);
                }
                sched.push((k, len_t - 24.0, len_t));
                prev = k;
            }
            sched.reverse();
        }
        let mut t = 0.0;
        for &(k, lc_t, len_t) in &sched {
            self.wt(hw, (lc_t - t).max(0.0));
            let (a, l) = self.ch[k].load.take().unwrap();
            hw.loc(k, a);
            self.wt(hw, (len_t - lc_t).max(0.0));
            hw.len(k, l);
            t = len_t.max(lc_t);
        }
        self.wt(hw, (g - t).max(0.0));
        hw.dmacon(0x800F);
        // copy of the working waveforms to the playing ones, about 370 colour clocks
        let at = self.playing(0);
        for i in 0..8 {
            self.wt(hw, 42.0);
            hw.bus.write_bytes(at + 16 * i as u32, &self.work[16 * i..16 * i + 16]);
            let t = hw.bus.clock;
            hw.bus.paula.sync(t, &hw.bus.ram);
        }
        // second loads
        let mut prev: Option<usize> = None;
        for k in 0..4 {
            let Some((a, l)) = self.ch[k].load2.take() else { continue };
            match prev {
                None => self.wt(hw, 368.5 + 27.5 * k as f64 - 8.0 * 42.0),
                Some(p) => self.wt(hw, 57.5 + 27.5 * (k - p - 1) as f64),
            }
            hw.loc(k, a);
            self.wt(hw, 24.0);
            hw.len(k, l);
            prev = Some(k);
        }
    }
}

impl Replayer for SynTracker {
    fn format(&self) -> &'static str {
        "SynTracker"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        DT_TICK
    }

    fn timer_delay(&self) -> u64 {
        TIMER_DELAY
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tickc = self.tickc.wrapping_add(1);
        if self.tickc == self.speed {
            self.tickc = 0;
            self.row_part(hw);
        }
        self.run_tick(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let buf = m.alloc(0x120);
    if buf == 0 {
        return None;
    }
    let hdr12 = be16(d, 0x12) as usize;
    let mut s = SynTracker {
        d: d.to_vec(),
        base,
        buf,
        dd: O_PATDATA + (hdr12 + 1) * 128,
        npos: be16(d, 0x10) as usize,
        ch: [Chan::new(), Chan::new(), Chan::new(), Chan::new()],
        master: [64; 4],
        speed: 6,
        tickc: 0,
        pos: 0,
        row: 0,
        jump: 0,
        upper: 0,
        work: [0; 128],
        songend: false,
        frac: 0.0,
    };
    let mut hw = Hw { bus: &mut m.bus };
    s.start_sequence(&mut hw);
    Some(Box::new(s))
}

impl SynTracker {
    /// The writes before the first tick (spec section 9), spaced as the reference's.
    fn start_sequence(&mut self, hw: &mut Hw) {
        hw.wait(545);
        for round in 0..2 {
            for k in 0..4 {
                hw.vol(k, 0);
                hw.wait([152, 164, 158, if round == 0 { 202 } else { 3237 }][k]);
            }
        }
        hw.bus.write_word(0xDFF09A, 0x4000);
        hw.wait(4613);
        hw.dmacon(0x000F);
        hw.wait(19);
        hw.dmacon(0x000F);
        hw.wait(1045);
        for k in 0..4 {
            hw.loc(k, self.zero_area());
            hw.wait(14);
            hw.len(k, 0x10);
            hw.per(k, 0x3E8);
            hw.wait(10);
            hw.vol(k, 0);
            hw.wait(10);
        }
        hw.wait(997);
        hw.dmacon(0x800F);
        hw.wait(10);
        hw.bus.write_word(0xDFF09E, 0x00FF);
        hw.wait(12);
        let x = hw.bus.read_byte(0xBFE001);
        hw.bus.write_byte(0xBFE001, x | 2);
    }
}

#[cfg(test)]
#[path = "syntracker_tests.rs"]
mod tests;
