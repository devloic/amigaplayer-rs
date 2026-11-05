//! Professional Sound Artists (`psa.*`; Dave "Sinbad" Hasler, 1990, game music such as The
//! Punisher).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A module is plain data (no code): a header with
//! subsong records, an instrument table of 64-byte records (sample or synth, with a sweep and a
//! four-phase envelope), a position list and 64-row patterns of four 4-byte entries.
//!
//! One interrupt of 70940 colour clocks is the music step (a new row every speed + 1 ticks, then
//! the four voices' effects, sweeps and envelopes), a wait of nine raster lines, and the DMA step
//! (DMACON start bits, then the loop pointers of the sample notes started in the row). Every
//! register write is placed at the clock of the original's code: the specification's cycle costs
//! of the path taken, accumulated in cycles and halved when a clock is needed, the two raster waits
//! taken from the absolute colour clock, and the song-end report (a host call) re-basing the clock.
//!
//! The synthesiser waveform memory (256 bytes of 0x80, then 256 of 0x7F, then 4 zero bytes, the last
//! being the silent loop) is built in code in guest memory.
//!
//! Choices where the specification leaves the behaviour open: every read outside the file
//! (an instrument number beyond the table, a pattern beyond the file, a sample or loop offset
//! outside it) reads zero; a note index beyond the period table reads the three values the
//! reference showed (37: 0x5754, 38: 4) and zero beyond; the run is not stopped at the song-end
//! report, the module only keeps its own stop (the host decides what to do with the signal).

use super::{Hw, Open, Replayer, be32};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Professional Sound Artists", detect, new };

/// Colour clocks between two interrupts.
const FRAME: u64 = 70940;
/// Colour clocks of one raster line.
const LINE: u64 = 227;

const PERIODS: [u16; 37] = [
    0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C, 0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC, 0x194, 0x17D, 0x168,
    0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2, 0x0D6, 0x0CA, 0x0BE, 0x0B4, 0x0AA, 0x0A0, 0x097, 0x08F,
    0x087, 0x07F, 0x078, 0x071, 0x06B,
];

/// Cycles of the hardware-register helpers, from the call to the write, by voice.
const H_LC: [u64; 4] = [67, 98, 129, 137];
const H_LEN: [u64; 4] = [63, 94, 125, 133];
const H_VOL_PRE: [u64; 4] = [174, 197, 220, 210];
const H_VOL_POST: [u64; 4] = [113, 144, 175, 183];

/// Waveform memory layout: 256 bytes of 0x80, 256 of 0x7F, 4 zero bytes.
const WAVE_BOUNDARY: u32 = 0x100;
const WAVE_SILENT: u32 = 0x200;

fn detect(d: &[u8]) -> bool {
    if d.len() < 64 || d[..4] != *b"PSA\0" {
        return false;
    }
    let instr = be32(d, 40) as usize;
    instr >= 64 && instr <= d.len() && (instr - 56) / 8 <= 256
}

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn rw(d: &[u8], o: usize) -> u16 {
    (rb(d, o) as u16) << 8 | rb(d, o.wrapping_add(1)) as u16
}

fn rl(d: &[u8], o: usize) -> u32 {
    (rw(d, o) as u32) << 16 | rw(d, o.wrapping_add(2)) as u32
}

/// The period of table entry `i`; beyond the table the reference read the player's memory.
fn table(i: u16) -> u16 {
    match i {
        0..=36 => PERIODS[i as usize],
        37 => 0x5754,
        38 => 0x0004,
        _ => 0,
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    base: u16,
    idx: u16,
    pspeed: i16,
    ptarget: u16,
    vol: u16,
    vstep: i16,
    vacc: u16,
    vcnt: u8,
    vrel: u8,
    /// instrument bytes 14 to 21
    syn: [u8; 8],
    delay: u8,
    count: u8,
    back: bool,
    ecnt: u8,
    /// instrument bytes 22 to 33
    env: [u8; 12],
    lp: u32,
    lplen: u16,
    wave: u32,
    pending: bool,
}

pub struct ProfessionalSoundArtists {
    d: Vec<u8>,
    base: u32,
    wave: u32,
    title: String,
    instr: usize,
    poslist: usize,
    pats: usize,
    nsub: usize,
    sub: usize,
    v: [Voice; 4],
    running: bool,
    speed: u8,
    step: u8,
    row: u8,
    pos: u16,
    end: u16,
    restart: u16,
    dmabits: u16,
    phase: u8,
    row_ptr: usize,
    next_row: usize,
    songend: bool,
    resume: Option<u64>,
    // the clock of the call being modelled, and the cycles into it
    t0: u64,
    cy: u64,
}

/// The duration (colour clocks) of the host's player initialisation, from the instrument table.
fn init_duration(d: &[u8], instr: usize, poslist: usize) -> u64 {
    let n = poslist.saturating_sub(instr) / 64;
    let (mut s, mut nn, mut o) = (0u64, 0u64, 0u64);
    let mut max = 3u32;
    for i in 0..n.min(4096) {
        let off = rl(d, instr.wrapping_add(64 * i));
        if off == 0 {
            s += 1;
        } else if off > max {
            nn += 1;
            max = off;
        } else {
            o += 1;
        }
    }
    (598 + 47 * s + 122 * nn + 62 * o) / 2
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let wave = m.alloc(516);
    if wave == 0 {
        return None;
    }
    for i in 0..256 {
        m.bus.ram[wave as usize + i] = 0x80;
        m.bus.ram[wave as usize + 256 + i] = 0x7F;
    }
    let instr = be32(d, 40) as usize;
    let poslist = be32(d, 44) as usize;
    let pats = be32(d, 48) as usize;
    let title = d[4..36].iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { ' ' }).collect::<String>();
    let mut s = ProfessionalSoundArtists {
        d: d.to_vec(),
        base,
        wave,
        title: title.trim().to_string(),
        instr,
        poslist,
        pats,
        nsub: (instr - 56) / 8,
        sub: 0,
        v: [Voice::default(); 4],
        running: true,
        speed: 6,
        step: 0,
        row: 0,
        pos: 0,
        end: 0,
        restart: 0,
        dmabits: 0,
        phase: 0,
        row_ptr: 0,
        next_row: 0,
        songend: false,
        resume: None,
        t0: 0,
        cy: 0,
    };
    let dur = init_duration(d, instr, poslist);
    let mut hw = Hw { bus: &mut m.bus };
    // the host's player initialisation, then the two volume set-ups and the subsong's init
    let start = 168 + dur;
    s.volume_setups(&mut hw, start);
    s.init(&mut hw, 0);
    Some(Box::new(s))
}

impl ProfessionalSoundArtists {
    /// The host's set-volume and set-balance calls from `start`: 648 colour clocks each.
    fn volume_setups(&mut self, hw: &mut Hw, start: u64) {
        for k in 0..2 {
            for (c, off) in [220u64, 348, 488, 623].into_iter().enumerate() {
                hw.bus.clock = hw.bus.clock.max(start + 648 * k + off);
                hw.vol(c, 0);
            }
        }
        hw.bus.clock = hw.bus.clock.max(start + 2 * 648);
    }

    /// The init of subsong `sub` from the current clock: 976 colour clocks.
    fn init(&mut self, hw: &mut Hw, sub: usize) {
        let t = hw.bus.clock;
        self.sub = sub;
        let rec = 56 + 8 * sub.min(255);
        self.pos = rw(&self.d, rec);
        self.end = rw(&self.d, rec + 2);
        self.restart = rw(&self.d, rec + 4);
        self.speed = 6;
        self.step = 0;
        self.running = true;
        self.row = 0;
        self.dmabits = 0;
        self.songend = false;
        self.next_row = self.pattern_at(self.pos);
        hw.bus.clock = t + 703;
        hw.dmacon(0x000F);
        for c in 0..4 {
            hw.bus.clock = t + 703 + 10 * (c as u64 + 1);
            hw.vol(c, 0);
        }
        hw.bus.clock = t + 976;
    }

    /// The start of the pattern named by the position list word at offset `p`.
    fn pattern_at(&self, p: u16) -> usize {
        let n = rw(&self.d, self.poslist.wrapping_add(p as usize)) as usize;
        self.pats.wrapping_add(1024 * n)
    }

    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    fn line(&self) -> u64 {
        ((self.t0 + self.cy / 2) / LINE) & 0xFF
    }

    // --- hardware helpers: the cost up to the write, the write

    fn h_lc(&mut self, hw: &mut Hw, n: usize, a: u32) {
        self.cy += H_LC[n];
        self.at(hw);
        hw.loc(n, a);
    }

    fn h_len(&mut self, hw: &mut Hw, n: usize, w: u16) {
        self.cy += H_LEN[n];
        self.at(hw);
        hw.len(n, w);
    }

    fn h_per(&mut self, hw: &mut Hw, n: usize, p: u16) {
        self.cy += H_LEN[n];
        self.at(hw);
        hw.per(n, p);
    }

    /// The volume sequence: scale, write, record.
    fn h_vol(&mut self, hw: &mut Hw, n: usize) {
        self.cy += H_VOL_PRE[n];
        self.at(hw);
        hw.vol(n, self.v[n].vol & 0x7F);
        self.cy += H_VOL_POST[n];
    }

    /// A raster wait (specification 7.5).
    fn wait(&mut self) {
        self.cy += 28;
        for _ in 0..9 {
            self.cy += 16;
            let l0 = self.line();
            for _ in 0..10_000 {
                self.cy += 16;
                let l = self.line();
                self.cy += 9;
                if l != l0 {
                    break;
                }
            }
            self.cy += 10;
        }
        self.cy += 44;
    }

    /// The song-end report: a host call that re-bases the clock.
    fn report(&mut self) {
        self.cy += 18 + 72;
        if self.cy & 1 == 1 {
            self.cy -= 1;
        }
        self.cy += 16 + 44;
        self.songend = true;
    }

    // --- one interrupt

    fn interrupt(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 242;
        self.music(hw);
        self.cy += 18;
        self.wait();
        self.cy += 18;
        self.dma_step(hw);
        self.cy += 168;
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    fn music(&mut self, hw: &mut Hw) {
        if !self.running {
            self.cy += 41 + 28;
            return;
        }
        self.cy += 41 + 162 + 9;
        self.step = self.step.wrapping_sub(1);
        if self.step & 0x80 != 0 {
            self.cy += 18;
            self.new_row(hw);
        }
        self.cy += 33;
        self.phase += 1;
        if self.phase >= 3 {
            self.phase = 0;
            self.cy += 4;
        }
        for n in 0..4 {
            self.cy += if n == 0 { 66 } else { 34 };
            self.voice(hw, n);
        }
        self.cy += 176;
    }

    fn dma_step(&mut self, hw: &mut Hw) {
        self.cy += 56;
        self.at(hw);
        hw.dmacon(0x8000 | self.dmabits);
        self.dmabits = 0;
        self.cy += 34;
        self.wait();
        for n in 0..4 {
            if self.v[n].pending {
                self.v[n].pending = false;
                self.cy += 69;
                self.at(hw);
                hw.loc(n, self.v[n].lp);
                self.cy += 24;
                self.at(hw);
                hw.len(n, self.v[n].lplen);
            } else {
                self.cy += 21;
            }
        }
        self.cy += 44;
    }

    // --- a new row

    fn new_row(&mut self, hw: &mut Hw) {
        self.cy += 82;
        let rp = self.next_row;
        self.row_ptr = rp;
        for n in 0..4 {
            self.entry(hw, n, rp.wrapping_add(4 * n));
            self.cy += if n < 3 { 12 + 34 } else { 12 };
        }
        self.cy += 45;
        self.row = (self.row + 1) & 63;
        let lookup;
        if self.row != 0 {
            self.next_row = rp.wrapping_add(16);
            lookup = false;
        } else {
            self.cy += 53;
            let mut p = self.pos.wrapping_add(2);
            let mut enter = true;
            if p == self.end {
                self.report();
                p = self.restart;
                self.cy += 29;
                if p == 0xFFFF {
                    self.report();
                    self.cy += 18;
                    self.stop(hw);
                    self.cy += 10;
                    self.running = false;
                    enter = false;
                }
            }
            if enter {
                self.pos = p;
                self.next_row = self.pattern_at(p);
            }
            lookup = enter;
        }
        if lookup {
            self.cy += 88;
        }
        // (a row that did not wrap goes straight to the store)
        self.cy += 54 + 384 + 16;
        self.step = self.speed;
    }

    /// The stop sequence at the end of the song.
    fn stop(&mut self, hw: &mut Hw) {
        self.cy += 56;
        self.at(hw);
        hw.dmacon(0x000F);
        for n in 0..4 {
            self.cy += 20;
            self.at(hw);
            hw.vol(n, 0);
        }
        self.cy += 28;
    }

    /// One row entry of voice `n` at file offset `ent`.
    fn entry(&mut self, hw: &mut Hw, n: usize, ent: usize) {
        let (note, ins, eff, par) = (rb(&self.d, ent), rb(&self.d, ent.wrapping_add(1)), rb(&self.d, ent.wrapping_add(2)), rb(&self.d, ent.wrapping_add(3)));
        self.cy += 63;
        self.v[n].pending = false;
        if note != 0 {
            self.cy += 15;
            if eff >= 0x80 {
                self.set_period(hw, n, note);
                self.checks(hw, n, eff, par);
            } else {
                self.cy += 17;
                if eff == 3 {
                    let target = table(note as u16 - 1);
                    self.v[n].ptarget = target;
                    let diff = target.wrapping_sub(self.v[n].base) as i16;
                    if diff >= 0 {
                        self.v[n].pspeed = (par as i16).wrapping_neg();
                        self.cy += 55 + 26;
                    } else {
                        self.v[n].pspeed = par as i16;
                        self.cy += 55 + 22;
                    }
                } else {
                    self.new_sound(hw, n, ins, eff, par);
                    self.set_period(hw, n, note);
                    self.checks(hw, n, eff, par);
                }
            }
        } else {
            self.checks(hw, n, eff, par);
        }
        self.cy += 20;
    }

    fn set_period(&mut self, hw: &mut Hw, n: usize, note: u8) {
        self.cy += 34 + 18;
        let idx = note as u16 - 1;
        self.v[n].idx = idx;
        self.v[n].base = table(idx);
        let p = self.v[n].base;
        self.h_per(hw, n, p);
        self.cy += 28 + 12;
    }

    fn new_sound(&mut self, hw: &mut Hw, n: usize, ins: u8, eff: u8, par: u8) {
        let i = self.instr.wrapping_add(64 * ins as usize);
        self.cy += 16;
        self.at(hw);
        hw.dmacon(1 << n);
        self.dmabits |= 1 << n;
        self.cy += 233;
        {
            let d = &self.d;
            let v = &mut self.v[n];
            for k in 0..8 {
                v.syn[k] = rb(d, i.wrapping_add(14 + k));
            }
            for k in 0..12 {
                v.env[k] = rb(d, i.wrapping_add(22 + k));
            }
            v.ecnt = 1;
        }
        let len = rw(&self.d, i.wrapping_add(4));
        let volw = if eff == 12 { par as u16 } else { rw(&self.d, i.wrapping_add(12)) };
        let e12 = eff == 12;
        if self.v[n].syn[0] != 0 {
            // synth instrument
            self.v[n].back = false;
            self.v[n].delay = self.v[n].syn[3];
            self.v[n].count = self.v[n].syn[4];
            self.cy += 114;
            self.h_len(hw, n, len);
            self.cy += 28 + 98;
            let wbase = (self.wave + WAVE_BOUNDARY).wrapping_sub(2 * len as u32);
            self.v[n].wave = wbase;
            let start = wbase.wrapping_add(self.v[n].syn[1] as u32);
            self.h_lc(hw, n, start);
            self.cy += 28 + 29;
            self.cy += if e12 { 34 } else { 46 };
            self.v[n].vol = volw;
            self.h_vol(hw, n);
            self.cy += 34;
            self.v[n].lplen = rw(&self.d, i.wrapping_add(10));
            self.cy += 20;
            self.v[n].pending = false;
        } else {
            self.v[n].pending = true;
            self.cy += 102;
            let a = self.base.wrapping_add(rl(&self.d, i));
            self.h_lc(hw, n, a);
            self.cy += 28 + 30;
            self.h_len(hw, n, len);
            self.cy += 28 + 29;
            self.cy += if e12 { 34 } else { 46 };
            self.v[n].vol = volw;
            self.h_vol(hw, n);
            self.cy += if e12 { 34 } else { 24 };
            self.cy += 25;
            let lp = rl(&self.d, i.wrapping_add(6));
            if lp == 0 {
                self.cy += 30;
                self.v[n].lp = self.wave + WAVE_SILENT;
            } else {
                self.cy += 64;
                self.v[n].lp = self.base.wrapping_add(lp);
            }
            self.v[n].lplen = rw(&self.d, i.wrapping_add(10));
            self.cy += 20;
        }
    }

    /// The effect checks after a row entry (specification 4.4.3).
    fn checks(&mut self, hw: &mut Hw, n: usize, eff: u8, par: u8) {
        self.cy += 17;
        if eff == 5 {
            self.cy += 16;
            self.v[n].env[7] = 1;
        }
        self.cy += 17;
        if eff == 12 {
            self.cy += 34;
            self.v[n].vol = par as u16;
            self.h_vol(hw, n);
            self.cy += 34;
            return;
        }
        self.cy += 17;
        if eff == 13 {
            self.cy += 26;
            self.row = 63;
            return;
        }
        self.cy += 17;
        if eff == 15 {
            self.cy += 12;
            self.speed = par;
        }
    }

    // --- the per-tick processing of one voice

    fn voice(&mut self, hw: &mut Hw, n: usize) {
        let ent = self.row_ptr.wrapping_add(4 * n);
        let eff = rb(&self.d, ent.wrapping_add(2)) & 0x7F;
        let p = rb(&self.d, ent.wrapping_add(3));
        let phase = self.phase;
        self.cy += 45;
        if eff == 0 && p != 0 {
            self.cy += 13 + 13;
            let v = self.v[n];
            let per = match phase {
                0 => {
                    self.cy += 42;
                    v.base
                }
                1 => {
                    self.cy += 37 + 66;
                    table(v.idx.wrapping_add((p >> 4) as u16))
                }
                _ => {
                    self.cy += 37 + 60;
                    table(v.idx.wrapping_add((p & 15) as u16))
                }
            };
            self.h_per(hw, n, per);
            self.cy += 28 + 22;
        } else if eff == 4 {
            if p != 0 {
                self.cy += 13;
            }
            self.cy += 17 + 33;
            if self.step == self.speed {
                self.cy += 13;
                if p != 0 {
                    self.cy += 90;
                    let v = &mut self.v[n];
                    v.vstep = (p >> 4) as i16;
                    v.vcnt = p & 15;
                    v.vrel = (p & 15).wrapping_mul(2);
                    v.vacc = 0;
                }
            }
            self.cy += 48 + 34;
            let v = &mut self.v[n];
            v.vacc = v.vacc.wrapping_add(v.vstep as u16);
            let per = v.base.wrapping_add(v.vacc);
            self.h_per(hw, n, per);
            self.cy += 28 + 39;
            let v = &mut self.v[n];
            v.vcnt = v.vcnt.wrapping_sub(1);
            if v.vcnt == 0 {
                v.vcnt = v.vrel;
                v.vstep = v.vstep.wrapping_neg();
                self.cy += 46;
            }
        } else {
            // the common prefix: a plain period write
            if p != 0 {
                self.cy += 13;
            }
            self.cy += 17 + 20;
            let b = self.v[n].base;
            self.at(hw);
            hw.per(n, b);
            self.cy += 17;
            match eff {
                10 => {
                    self.cy += 13 + 33;
                    let v = &mut self.v[n];
                    if p & 0x80 == 0 {
                        v.vol = v.vol.wrapping_add(p as u16);
                        if (v.vol as u8).wrapping_sub(0x40) & 0x80 == 0 {
                            v.vol = (v.vol & 0xFF00) | 0x40;
                            self.cy += 8;
                        }
                    } else {
                        v.vol = v.vol.wrapping_sub((p & 0x7F) as u16);
                        if v.vol & 0x8000 != 0 {
                            v.vol = 0;
                            self.cy += 4;
                        }
                    }
                    self.cy += 34;
                    self.h_vol(hw, n);
                    self.cy += 34;
                }
                1 | 2 => {
                    self.cy += if eff == 1 { 17 + 50 } else { 17 + 17 + 50 };
                    let v = &mut self.v[n];
                    v.base = if eff == 1 { v.base.wrapping_sub(p as u16) } else { v.base.wrapping_add(p as u16) };
                    let b = v.base;
                    self.h_per(hw, n, b);
                    self.cy += 28 + 34;
                }
                3 => {
                    self.cy += 17 * 3;
                    let s = self.v[n].pspeed;
                    if s == 0 {
                        self.cy += 21;
                    } else {
                        self.cy += 21 + 9 + 37;
                        let v = &mut self.v[n];
                        let new = v.base.wrapping_sub(s as u16);
                        let d = new.wrapping_sub(v.ptarget);
                        let reached = if s > 0 { d & 0x8000 != 0 } else { d & 0x8000 == 0 };
                        if reached {
                            v.base = v.ptarget;
                            v.pspeed = 0;
                            self.cy += 42;
                            let b = v.base;
                            self.h_per(hw, n, b);
                            self.cy += 28 + 58;
                        } else {
                            v.base = new;
                            self.cy += 18;
                            self.h_per(hw, n, new);
                            self.cy += 28 + 12;
                        }
                    }
                }
                _ => self.cy += 17 * 3,
            }
        }
        self.sweep(hw, n);
        self.envelope(hw, n);
        self.cy += 24;
    }

    fn sweep(&mut self, hw: &mut Hw, n: usize) {
        self.cy += 21;
        if self.v[n].syn[0] == 0 {
            return;
        }
        self.cy += 25;
        let v = &mut self.v[n];
        v.delay = v.delay.wrapping_sub(1);
        if v.delay != 0 {
            return;
        }
        self.cy += 37 + 57 + 70;
        if !v.back {
            v.delay = v.syn[3];
            v.syn[1] = v.syn[1].wrapping_add(v.syn[2]);
            v.count = v.count.wrapping_sub(1);
            if v.count == 0 {
                v.count = v.syn[7];
                v.back = true;
                self.cy += 46;
            }
        } else {
            v.delay = v.syn[6];
            v.syn[1] = v.syn[1].wrapping_sub(v.syn[5]);
            v.count = v.count.wrapping_sub(1);
            if v.count == 0 {
                v.count = v.syn[4];
                v.back = false;
                self.cy += 36;
            }
        }
        let a = v.wave.wrapping_add(v.syn[1] as u32);
        self.h_lc(hw, n, a);
        self.cy += 28 + 12;
    }

    fn envelope(&mut self, hw: &mut Hw, n: usize) {
        self.cy += 23;
        if self.v[n].env[0] == 0 {
            return;
        }
        self.cy += 27;
        let v = &mut self.v[n];
        v.ecnt = v.ecnt.wrapping_sub(1);
        if v.ecnt != 0 {
            return;
        }
        self.cy += 29;
        // env[k]: 0 phase, 1..3 attack step/interval/count, 4..6 decay, 7 sustain, 8..10 release
        let mut write = true;
        match v.env[0] {
            1 => {
                v.ecnt = v.env[2];
                let lo = (v.vol as u8).wrapping_add(v.env[1]);
                v.vol = (v.vol & 0xFF00) | lo as u16;
                v.env[3] = v.env[3].wrapping_sub(1);
                self.cy += 57;
                if v.env[3] == 0 {
                    v.env[0] = 2;
                    self.cy += 26;
                }
            }
            2 => {
                v.ecnt = v.env[5];
                let lo = (v.vol as u8).wrapping_sub(v.env[4]);
                v.vol = (v.vol & 0xFF00) | lo as u16;
                v.env[6] = v.env[6].wrapping_sub(1);
                self.cy += 17 + 57;
                if v.env[6] == 0 {
                    v.env[0] = 3;
                    self.cy += 26;
                }
            }
            3 => {
                v.ecnt = 1;
                write = false;
                self.cy += 17 + 17 + 37;
                if v.env[7] != 0 {
                    self.cy += 25;
                    v.env[7] -= 1;
                    if v.env[7] == 0 {
                        v.env[0] = 4;
                        self.cy += 26;
                    }
                }
            }
            _ => {
                v.ecnt = v.env[9];
                let lo = (v.vol as u8).wrapping_sub(v.env[8]);
                v.vol = (v.vol & 0xFF00) | lo as u16;
                v.env[10] = v.env[10].wrapping_sub(1);
                self.cy += 17 + 17 + 57;
                if v.env[10] == 0 {
                    v.env[0] = 0;
                    self.cy += 26;
                }
            }
        }
        if write {
            self.cy += 13;
            let v = &mut self.v[n];
            if v.vol & 0x80 != 0 {
                v.vol = 0;
                self.cy += 4;
            }
            self.cy += 30;
            self.h_vol(hw, n);
        }
    }
}

impl Replayer for ProfessionalSoundArtists {
    fn format(&self) -> &'static str {
        "Professional Sound Artists"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        FRAME
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsub.max(1) as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.nsub.max(1) - 1);
        // the host's stop call, the two volume set-ups and the init of the subsong
        let t = hw.bus.clock;
        hw.bus.clock = t + 33;
        hw.dmacon(0x000F);
        for c in 0..4 {
            hw.bus.clock = t + 33 + 10 * (c as u64 + 1);
            hw.vol(c, 0);
        }
        hw.bus.clock = t + 87;
        let s = hw.bus.clock;
        self.volume_setups(hw, s);
        self.init(hw, n);
        self.resume = Some(hw.bus.clock + FRAME);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.interrupt(hw);
    }
}

#[cfg(test)]
#[path = "professionalsoundartists_tests.rs"]
mod tests;

