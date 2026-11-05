//! YM-2149 (Eagleplayer "YM-2149", prefixes `ym`, `ymst`): the MYST format of Nicolas Pomarede, the
//! music of Atari ST games and demos.
//!
//! Clean-room, written from a behavioural specification (`fixround/spec/ym2149`). A MYST file carries
//! an Atari ST replay routine (68000 code, patched by the converter) with its music data, or only the
//! music data plus the name of a separate replay file. That code is the module's data: it is run on
//! the repo's 68000 core with the standard static cycle table (`m68k_rc::cycles`), at the entry points
//! the file's tag list names. It never touches the ST sound chip's effect on Paula; it stores the state
//! of the YM-2149 registers into a block that the player owns, and the player's "emulator" (section 4
//! to 5 of the specification) turns that block into Paula register writes and into sample buffers it
//! generates itself (square waves, noise, envelopes and their mixes). This file is that emulator, with
//! its tables, the block and the buffers in guest memory (the module reads and writes the block), and
//! the register writes placed on the colour clocks the specification's cycle ledger gives.
//!
//! Choices where the specification is silent or the original faults (all safe, nothing can panic or
//! hang):
//! * a CPU error, or a module routine that runs over its instruction budget, in a tick ends the song;
//!   in the load or the start of a subsong it makes the module not playable (not recognised);
//! * a division by zero in the emulator (a SID or envelope period whose scaled value is 0) stops the
//!   playback, as the reference does (its CPU raises an exception);
//! * reads of the ST sound-chip port, and every access outside the chip memory, give zero / are
//!   ignored (the bus's unmapped area); voice masks other than 0xF are written as the specification
//!   says but were never checked (the host here always gives 0xF);
//! * the three emulator services cost an assumed 40 cycles per byte (the specification gives none);
//!   service numbers other than 10 to 12 return at once;
//! * reads of the noise tables past their end give whatever follows in guest memory (zeros here);
//! * a file of type 1 (replay only) or without the needed tags is not recognised.

use super::{be32, Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{AddressBus, CpuCore, CpuType, StepResult};

pub const OPEN: Open = Open { name: "YM-2149", detect, new };

// ------------------------------------------------------------------------------------ the header

/// The tags of a MYST file: (letter after `Y`, value).
#[derive(Debug, Clone, Default)]
struct Header {
    magic: usize,
    tags: Vec<(u8, u32)>,
}

impl Header {
    fn get(&self, c: u8) -> Option<u32> {
        self.tags.iter().find(|t| t.0 == c).map(|t| t.1)
    }
}

/// Reads the magic and the tag list. Entries run to the zero tag; a tag that is not of the
/// `0x8000_59xx` family is skipped.
fn parse(d: &[u8]) -> Option<Header> {
    let lim = d.len().min(256);
    let magic = (0..lim).step_by(2).find(|&o| d.get(o..o + 4) == Some(b"YMST"))?;
    let mut h = Header { magic, tags: Vec::new() };
    let mut p = magic + 4;
    while p + 8 <= d.len() && h.tags.len() < 128 {
        let tag = be32(d, p);
        if tag == 0 {
            break;
        }
        if tag >> 8 == 0x8000_59 {
            h.tags.push(((tag & 0xFF) as u8, be32(d, p + 4)));
        }
        p += 8;
    }
    Some(h)
}

fn cstr(d: &[u8], o: usize, max: usize) -> String {
    d.get(o..).map_or(String::new(), |s| s.iter().take_while(|&&b| b != 0).take(max).map(|&b| b as char).collect())
}

/// The offset-valued tags that must lie in the file for it to be playable here.
fn offset_ok(d: &[u8], h: &Header, c: u8) -> bool {
    h.get(c).is_some_and(|v| h.magic.saturating_add(v as usize) < d.len())
}

fn detect(d: &[u8]) -> bool {
    let Some(h) = parse(d) else { return false };
    match h.get(b'M') {
        Some(0) => [b'Z', b'\\', b'P'].iter().all(|&c| offset_ok(d, &h, c)),
        Some(2) => offset_ok(d, &h, b'O') && offset_ok(d, &h, b'Z'),
        _ => false,
    }
}

/// One MYST file in guest memory: where its magic is and its tags.
struct GFile {
    addr: u32,
    h: Header,
}

impl GFile {
    /// The address an offset tag names.
    fn at(&self, c: u8) -> Option<u32> {
        self.h.get(c).map(|v| self.addr.wrapping_add(v))
    }
}

// ----------------------------------------------------------------------------- the emulator block

const VOL: [u8; 16] = [0, 2, 3, 4, 6, 8, 11, 14, 17, 22, 26, 30, 35, 42, 50, 64];
const LIN: [u8; 16] = [0, 5, 10, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64];
/// The envelope pair a YM envelope shape uses.
const PAIR: [u32; 16] = [0, 0, 0, 0, 1, 1, 1, 1, 2, 0, 3, 4, 5, 6, 7, 1];
/// The kind of each half envelope: 0 down, 1 up, 2 all zero, 3 all full.
const HALF: [u8; 16] = [0, 2, 1, 2, 0, 0, 0, 1, 0, 3, 1, 1, 1, 3, 1, 0];
/// Offsets of the eight square samples from the table's start.
const SQ: [u32; 8] = [0, 0x80, 0x7E, 0x7A, 0x72, 0x62, 0x42, 0x02];

// layout, relative to the silent word S
const O_VOL: i32 = -0x2218;
const O_LIN: i32 = -0x2208;
const O_SC: i32 = -0x21F8;
const O_ENVT: i32 = -0x1158;
const O_BLK: i32 = -0xF58;
const O_MIX: i32 = -0xE00;
const O_SCRATCH: i32 = -0x200;
const O_SQ: i32 = 2;
const O_WAVE: i32 = 0x104;
const O_N: i32 = 0x204;
const O_NH: i32 = 0x1204;
const O_NL: i32 = 0x2204;
/// Bytes from the area's start to S, and the area's size (with room for the noise reads that run on).
const BEFORE: u32 = 0x2218;
const AREA: u32 = BEFORE + 0x3204 + 0x2000;

// block fields
const B_PER: [u32; 3] = [0x00, 0x04, 0x08];
const B_VOL: [u32; 3] = [0x02, 0x06, 0x0A];
const B_NOISE: u32 = 0x0C;
const B_MIX: u32 = 0x0E;
const B_TRIG: u32 = 0x10;
const B_SHAPE: u32 = 0x11;
const B_ENV: u32 = 0x12;
const B_SVC: u32 = 0x14;
const B_MASK: u32 = 0x1A;
const B_SRC4: u32 = 0x1C;
const B_SEL: u32 = 0x11A;
const B_THRESH: u32 = 0x14E;
const B_COUNT: u32 = 0x150;
/// Voice records: A (channel 0), B (channel 3), C (channel 1), the fourth (channel 2).
const REC: [u32; 4] = [0x1E, 0x52, 0x86, 0xBA];
const PAULA: [u32; 4] = [0xDF_F0A0, 0xDF_F0D0, 0xDF_F0B0, 0xDF_F0C0];
const DMABIT: [u16; 4] = [1, 8, 2, 4];
// record fields
const R_SID: u32 = 2;
const R_SIDPER: u32 = 6;
const R_LEVELS: u32 = 8;
const R_NPH: u32 = 0x0C;
const R_PHASE: u32 = 0x10;
const R_ENV: u32 = 0x14;
const R_GAIN: u32 = 0x1E;
const SET_A: u32 = 0x20;
const SET_B: u32 = 0x2A;
// inside a set
const S_LEN: u32 = 4;
const S_PER: u32 = 6;
const S_VOL: u32 = 8;

fn rd(bus: &Bus, a: u32) -> u16 {
    bus.r16(a)
}

/// The emulator: where its area is in guest memory.
#[derive(Clone, Copy)]
struct Emu {
    s: u32,
    blk: u32,
    svc: u32,
}

fn off(s: u32, o: i32) -> u32 {
    (s as i32).wrapping_add(o) as u32
}

fn sc_value(v: usize, i: u8) -> u8 {
    let s = i as i8 as i32;
    let vol = VOL[v] as i32;
    ((((s + 128) * vol) >> 6) - ((128 * vol) >> 6)) as u8
}

impl Emu {
    fn env_table(&self, pair: u32) -> u32 {
        off(self.s, O_ENVT) + 64 * pair
    }
    fn env_wave(&self, pair: u32) -> u32 {
        off(self.s, O_WAVE) + 32 * pair
    }
    fn square(&self, k: usize) -> u32 {
        off(self.s, O_SQ) + SQ[k]
    }
    fn mix(&self, i: usize) -> u32 {
        off(self.s, O_MIX) + 512 * i as u32
    }
    fn fld(&self, o: u32) -> u32 {
        self.blk + o
    }

    /// Builds the tables and the block (specification 4.2); the Paula writes are the caller's.
    fn init(&self, bus: &mut Bus) {
        let s = self.s;
        bus.write_bytes(off(s, O_VOL), &VOL);
        bus.write_bytes(off(s, O_LIN), &LIN);
        let sc = off(s, O_SC);
        for v in 0..16 {
            for i in 0..256 {
                bus.w8(sc + 256 * v as u32 + i, sc_value(v, i as u8));
            }
        }
        // square table
        let sq = off(s, O_SQ);
        bus.w8(sq, 0x7F);
        bus.w8(sq + 1, 0x80);
        for i in 0..128 {
            bus.w8(sq + 2 + i, 0x7F);
            bus.w8(sq + 130 + i, 0x80);
        }
        // noise
        let mut x: u64 = 1;
        let mut next = || {
            x = 16807 * x % 2_147_483_647;
            x
        };
        next();
        for j in 0..2048u32 {
            let n: i32 = if next() & 1 == 1 { -127 } else { 127 };
            let nh = (n + 127).min(127);
            let nl = (n - 127).max(-127);
            for k in [j, j + 2048] {
                bus.w8(off(s, O_N) + k, n as u8);
                bus.w8(off(s, O_NH) + k, nh as u8);
                bus.w8(off(s, O_NL) + k, nl as u8);
            }
        }
        // envelopes
        for p in 0..8u32 {
            for half in 0..2usize {
                let kind = HALF[2 * p as usize + half];
                for l in 0..16u32 {
                    let level = match kind {
                        0 => 15 - l,
                        1 => l,
                        2 => 0,
                        _ => 15,
                    };
                    let idx = 16 * half as u32 + l;
                    bus.w16(self.env_table(p) + 2 * idx, (level << 8) as u16);
                    let w = (sc_value(level as usize, 0x7F) as i8 as i32 - 0x40) * 2;
                    bus.w8(self.env_wave(p) + idx, w as u8);
                }
            }
        }
        // the block
        for i in 0..0x158 {
            bus.w8(self.blk + i, 0);
        }
        bus.w8(self.fld(B_TRIG), 0xFF);
        bus.w16(self.fld(B_MIX), 0x3F);
        bus.w32(self.fld(B_SVC), self.svc);
        bus.w16(self.fld(B_MASK), 0x000F);
        bus.w16(self.fld(B_SRC4), 1);
        for (i, &r) in REC.iter().enumerate() {
            bus.w32(self.blk + 0xEE + [0, 12, 4, 8][i], self.blk + r);
        }
        bus.w32(self.blk + 0xFE, off(s, O_VOL));
        for i in 0..6 {
            bus.w32(self.blk + 0x102 + 4 * i as u32, self.mix(i));
        }
        bus.w32(self.blk + 0x11C, off(s, O_N));
        bus.w32(self.blk + 0x120, off(s, O_NH));
        bus.w32(self.blk + 0x124, off(s, O_NL));
        bus.w16(self.blk + 0x128, 0x800);
        bus.w32(self.blk + 0x12A, s);
        for k in 0..8 {
            bus.w32(self.blk + 0x12E + 4 * k as u32, self.square(k));
        }
        for (i, &r) in REC.iter().enumerate() {
            let rec = self.blk + r;
            bus.w16(rec, 0x8000);
            if i < 3 {
                bus.w32(rec + R_ENV, self.env_table(0));
            }
            bus.w32(rec + 0x18, PAULA[i]);
            bus.w16(rec + 0x1C, DMABIT[i]);
            bus.w16(rec + R_GAIN, 64);
            for set in [SET_A, SET_B] {
                bus.w32(rec + set, s);
                bus.w16(rec + set + S_LEN, 1);
                bus.w16(rec + set + S_PER, 0x8C);
                bus.w16(rec + set + S_VOL, 0);
            }
        }
        // the service table: every entry returns at once (services 10 to 12 are taken by the CPU loop)
        for i in 0..14 {
            bus.w16(self.svc + 4 * i, 0x4E75);
            bus.w16(self.svc + 4 * i + 2, 0x4E71);
        }
    }

    /// The fourth voice copies its source's flags and register sets (specification 5.1).
    fn copy_fourth(&self, bus: &mut Bus) {
        let src = rd(bus, self.fld(B_SRC4)) as i16;
        if src < 0 {
            return;
        }
        let from = REC[(src as usize).min(2)] + self.blk;
        let to = self.blk + REC[3];
        bus.w16(to, bus.r16(from));
        for o in (SET_A..SET_B + 10).step_by(2) {
            bus.w16(to + o, bus.r16(from + o));
        }
    }

    fn set_lc(&self, bus: &mut Bus, rec: u32, lc: u32, len: u16) {
        for set in [SET_A, SET_B] {
            bus.w32(rec + set, lc);
            bus.w16(rec + set + S_LEN, len);
        }
    }

    fn set_pv(&self, bus: &mut Bus, rec: u32, per: u16, vol: u16) {
        for set in [SET_A, SET_B] {
            bus.w16(rec + set + S_PER, per);
            bus.w16(rec + set + S_VOL, vol);
        }
    }

    /// Mode 0: the silent sample.
    fn silent(&self, bus: &mut Bus, rec: u32) -> u16 {
        if bus.r32(rec + SET_A) == self.s {
            return 0x4000;
        }
        self.set_lc(bus, rec, self.s, 1);
        self.set_pv(bus, rec, 0x80, 0);
        0
    }

    /// Mode 2: noise.
    fn noise(&self, bus: &mut Bus, rec: u32, noise: u16, vol: u16) -> u16 {
        let n = off(self.s, O_N);
        let code = if bus.r32(rec + SET_A) != n {
            self.set_lc(bus, rec, n, 0x800);
            2
        } else {
            0x4002
        };
        self.set_pv(bus, rec, noise.wrapping_mul(16).wrapping_add(0x8C), VOL[(vol & 15) as usize] as u16);
        code
    }

    /// Mode 4: the envelope alone as a waveform.
    fn envelope(&self, bus: &mut Bus, rec: u32) -> u16 {
        let e = rd(bus, self.fld(B_ENV)) as i16;
        if bus.r16(rec + R_SID) != 0 || e == 0 {
            return self.silent(bus, rec);
        }
        let e = (e as i32).clamp(10, 0x11E1) as u32;
        let shape = bus.r8(self.fld(B_SHAPE)) & 15;
        self.set_lc(bus, rec, self.env_wave(PAIR[shape as usize]), 0x10);
        self.set_pv(bus, rec, ((e * 0xE517) >> 12) as u16, 64);
        0x40
    }

    fn sc(&self, bus: &Bus, level: u32, i: u32) -> u8 {
        bus.r8(off(self.s, O_SC) + (level * 256 + i))
    }

    /// The computation of one tick (specification 5.3). `Err` is the division fault.
    fn compute(&self, bus: &mut Bus) -> Result<(), ()> {
        for i in 0..3 {
            let p = rd(bus, self.fld(B_PER[i]));
            bus.w16(self.fld(B_PER[i]), p & 0xFFF);
            let v = rd(bus, self.fld(B_VOL[i]));
            bus.w16(self.fld(B_VOL[i]), v & 0x1F);
        }
        let noise = rd(bus, self.fld(B_NOISE)) & 0x1F;
        bus.w16(self.fld(B_NOISE), noise);
        let mixer = rd(bus, self.fld(B_MIX)) & 0x3F;
        bus.w16(self.fld(B_MIX), mixer);
        let shape = bus.r8(self.fld(B_SHAPE)) & 15;
        bus.w8(self.fld(B_SHAPE), shape);
        let sel = if rd(bus, self.fld(B_SEL)) == 0 { 4 } else { 0 };
        bus.w16(self.fld(B_SEL), sel);
        if bus.r8(self.fld(B_TRIG)) < 0x80 {
            for r in &REC[..3] {
                let rec = self.blk + r;
                bus.w32(rec + R_PHASE, 0);
                bus.w32(rec + R_ENV, self.env_table(PAIR[shape as usize]));
            }
            bus.w8(self.fld(B_TRIG), 0xFF);
        }
        let thresh = rd(bus, self.fld(B_THRESH)) as i16;
        for v in 0..3usize {
            let rec = self.blk + REC[v];
            let f = bus.r16(rec);
            if f & 0x80 != 0 {
                continue;
            }
            let p = rd(bus, self.fld(B_PER[v])) as u32;
            let vr = rd(bus, self.fld(B_VOL[v]));
            let tone = mixer >> v & 1 == 0;
            let noise_on = mixer >> (3 + v) & 1 == 0;
            let env = vr & 0x10 != 0 || bus.r16(rec + R_SID) != 0;
            let idx = tone as u8 + 2 * noise_on as u8 + 4 * env as u8;
            let buf = self.mix(2 * v + (sel == 4) as usize);
            let level = VOL[(vr & 15) as usize] as u16;
            let code = match idx {
                0 => self.silent(bus, rec),
                1 => {
                    if (p as i32) <= thresh as i32 {
                        self.silent(bus, rec)
                    } else {
                        let p = p.max(10);
                        let k = octave(p);
                        let code = if bus.r32(rec + SET_A) != self.square(k) {
                            self.set_lc(bus, rec, self.square(k), 1 << k);
                            1
                        } else {
                            0x4001
                        };
                        self.set_pv(bus, rec, amiper(p, k), level);
                        code
                    }
                }
                2 => self.noise(bus, rec, noise, vr),
                3 => {
                    if p == 0 {
                        self.noise(bus, rec, noise, vr)
                    } else {
                        self.tone_noise(bus, rec, p, noise, level, buf);
                        4
                    }
                }
                4 => self.envelope(bus, rec),
                5 => {
                    if p == 0 {
                        self.envelope(bus, rec)
                    } else {
                        self.tone_env(bus, rec, p, buf)?;
                        8
                    }
                }
                6 => 0x10,
                _ => 0x20,
            };
            let nf = if code & 0x4000 != 0 {
                code
            } else if code == f {
                f
            } else {
                code | 0x8000
            };
            bus.w16(rec, nf);
        }
        Ok(())
    }

    /// Tone and noise mixed into the voice's buffer (specification 5.3.1).
    fn tone_noise(&self, bus: &mut Bus, rec: u32, p: u32, noise: u16, level: u16, buf: u32) {
        let p = p.max(10);
        let k = octave(p);
        let per = amiper(p, k);
        self.set_lc(bus, rec, buf, 0x100);
        self.set_pv(bus, rec, per, level);
        let div = noise as u32 * 16 + 0x8C;
        let step = divstep((per as u32) << 13, div).unwrap_or(0);
        let mut ph = bus.r32(rec + R_NPH) & 0x07FF_FFFF;
        let o = ((step >> 8).wrapping_add(ph >> 16)) & 0x7FF;
        let (nh, nl) = (off(self.s, O_NH), off(self.s, O_NL));
        for j in 0..256u32 {
            let t = if (j >> k) & 1 == 0 { nh } else { nl };
            let i = (ph >> 16) as u16 as i16 as i32;
            let a = t.wrapping_add(i as u32);
            let b = t.wrapping_add((i + o as i32) as u32);
            bus.w8(buf + j, bus.r8(a));
            bus.w8(buf + 256 + j, bus.r8(b));
            ph = ph.wrapping_add(step);
        }
        bus.w32(rec + R_NPH, ph);
    }

    /// Tone and envelope, or tone and SID, mixed into the voice's buffer (specification 5.3.2).
    fn tone_env(&self, bus: &mut Bus, rec: u32, p: u32, buf: u32) -> Result<(), ()> {
        let p = p.max(10);
        let k = octave(p);
        let per = amiper(p, k);
        self.set_lc(bus, rec, buf, 0x100);
        self.set_pv(bus, rec, per, 64);
        let scratch = off(self.s, O_SCRATCH);
        for j in 0..512u32 {
            bus.w8(scratch + j, if (j >> k) & 1 == 0 { 0x7F } else { 0x80 });
        }
        let sid = bus.r16(rec + R_SID);
        match sid {
            0 => {
                let mut e = rd(bus, self.fld(B_ENV)) as i16 as i32;
                if e == 0 {
                    e = 1;
                } else if e >= 0x11E1 {
                    e = 0x11E1;
                }
                let big = (((e as u16 as u32) * 0xE517) >> 12) & 0xFFFF;
                let step = divstep((per as u32) << 13, big).ok_or(())?;
                let mut ph = bus.r32(rec + R_PHASE);
                let w = bus.r32(rec + R_ENV);
                for j in 0..512u32 {
                    let lv = (bus.r16(w + 2 * ((ph >> 16) & 31)) >> 8) as u32;
                    let v = self.sc(bus, lv, bus.r8(scratch + j) as u32);
                    bus.w8(buf + j, v);
                    ph = ph.wrapping_add(step);
                }
                bus.w32(rec + R_PHASE, (ph & 0x001F_0000) | (ph & 0xFFFF));
            }
            1 => {
                let mut sp = bus.r16(rec + R_SIDPER) as i16 as i32;
                if sp >= 0x11E1 {
                    sp = 0x11E1;
                }
                let big = (((sp as u16 as u32) * 0xE517) >> 12) & 0xFFFF;
                let mut step = divstep((per as u32) << 13, big).ok_or(())?;
                if step > 0x10000 {
                    step = 0xFFFF;
                }
                let step = step & 0xFFFF;
                // the current level word is the long's low word (the swap exchanges the halves)
                let mut l0 = bus.r16(rec + R_LEVELS + 2);
                let mut l1 = bus.r16(rec + R_LEVELS);
                let mut ph = bus.r16(rec + R_PHASE + 2) as u32;
                for j in 0..512u32 {
                    ph += step;
                    if ph > 0xFFFF {
                        ph &= 0xFFFF;
                        std::mem::swap(&mut l0, &mut l1);
                    }
                    l0 = (l0 & 0xFF00) | bus.r8(scratch + j) as u16;
                    let v = bus.r8(off(self.s, O_SC) + l0 as u32);
                    bus.w8(buf + j, v);
                }
                bus.w16(rec + R_LEVELS + 2, l0);
                bus.w16(rec + R_LEVELS, l1);
                bus.w16(rec + R_PHASE + 2, ph as u16);
            }
            _ => {}
        }
        Ok(())
    }
}

/// The octave of a tone period of at least 10.
fn octave(p: u32) -> usize {
    [(0x500, 7), (0x280, 6), (0x140, 5), (0xA0, 4), (0x50, 3), (0x28, 2), (0x14, 1), (0x0A, 0)]
        .iter()
        .find(|t| p >= t.0)
        .map_or(0, |t| t.1)
}

/// The Paula period of a tone period in an octave.
fn amiper(p: u32, k: usize) -> u16 {
    (((p * 0xE517) >> k) >> 12) as u16
}

/// An unsigned 32 by 16 bit division as the 68000 does it, keeping the low word of the result:
/// `None` for a zero divisor; when the quotient does not fit 16 bits the destination is left as it
/// was, so the low word of the dividend is kept (the specification says "the dividend unchanged";
/// the reference's audio shows the low word, which is what the code behind the division takes).
fn divu(dividend: u32, divisor: u32) -> Option<u32> {
    if divisor == 0 {
        return None;
    }
    let q = dividend / divisor;
    Some(if q > 0xFFFF { dividend & 0xFFFF } else { q })
}

/// The phase step of a mix: the division, then three places up (a 16.16 value).
fn divstep(dividend: u32, divisor: u32) -> Option<u32> {
    Some(divu(dividend, divisor)? << 3)
}

// ----------------------------------------------------------------------------- the module's 68000

/// Cycles assumed for each byte a service converts (the specification gives none).
const SERVICE_BYTE_CYCLES: u64 = 40;
/// Instructions a call may run before it counts as hung.
const LIMIT_LOAD: u64 = 10_000_000;
const LIMIT_TICK: u64 = 4_000_000;

struct Cpu68 {
    cpu: CpuCore,
    sp: u32,
    e: Emu,
}

impl Cpu68 {
    fn new(sp: u32, e: Emu) -> Cpu68 {
        let mut cpu = CpuCore::new();
        cpu.set_cpu_type(CpuType::M68000);
        cpu.set_sr(0x2000);
        Cpu68 { cpu, sp, e }
    }

    fn a(&mut self, n: usize, v: u32) {
        self.cpu.dar[8 + n] = v;
    }

    /// A service (specification 4.4): converts the bytes of `[a0, a1)` in place. Returns its cycles.
    fn service(&mut self, bus: &mut Bus, k: u32) -> u64 {
        let (a0, a1) = (self.cpu.dar[8], self.cpu.dar[9]);
        let n = if a1 > a0 { (a1 - a0).min(0x40_0000) } else { 0 };
        let conv = |tab: &[u8; 16], b: u8| -> u8 {
            let v = if b >= 0x80 { 0 } else { (b & 15) as usize };
            ((tab[v] as u32 * 255) >> 6).wrapping_sub(0x80) as u8
        };
        for i in 0..n {
            let a = a0 + i;
            let b = bus.r8(a);
            let nb = match k {
                10 => conv(&VOL, b),
                11 => conv(&LIN, b),
                _ => b ^ 0x80,
            };
            bus.w8(a, nb);
        }
        n as u64 * SERVICE_BYTE_CYCLES + 16
    }

    /// Runs the routine at `entry` until it returns; the registers stay as the previous call left them
    /// (the caller sets the ones the specification names). Returns the cycles it took.
    fn call(&mut self, bus: &mut Bus, entry: u32, limit: u64) -> Result<u64, String> {
        let ret = self.sp & !3;
        let sp = ret - 4;
        bus.w32(sp, ret);
        self.cpu.dar[15] = sp;
        self.cpu.pc = entry;
        self.cpu.invalidate_prefetch();
        let svc = self.e.svc;
        let mut cycles = 0u64;
        let mut n = 0u64;
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok(cycles);
            }
            n += 1;
            if n > limit {
                return Err(format!("ym2149: routine did not return (PC=${pc:06x})"));
            }
            if (svc + 0x28..=svc + 0x30).contains(&pc) && (pc - svc) % 4 == 0 {
                cycles += self.service(bus, (pc - svc) / 4);
                let sp = self.cpu.dar[15];
                self.cpu.pc = bus.r32(sp);
                self.cpu.dar[15] = sp.wrapping_add(4);
                self.cpu.invalidate_prefetch();
                continue;
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68000);
            cycles += m68k_rc::cycles::cycles(&ins) as u64;
            match self.cpu.step(bus) {
                StepResult::Ok { .. } => {}
                other => return Err(format!("ym2149: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }
}

// ------------------------------------------------------------------------------------ the player

/// The 50 Hz tick, in colour clocks.
const TICK: u64 = 70940;
/// The reference's first emulator write, from the run's origin, for a module with nothing to run at load.
const INIT_AT: u64 = 1_197_490;
/// From the first emulator write to the first tick's call, for a module whose start routine takes nothing.
const FIRST_TICK_AFTER: u64 = 76_761;

struct Ym {
    e: Emu,
    cpu: Cpu68,
    files: Vec<GFile>,
    typ: u32,
    first: u16,
    last: u16,
    cur: u16,
    title: String,
    /// The reference's time zero: the line counter the emulator waits on counts from it.
    origin: u64,
    dead: bool,
    ticks: u64,
    resume: Option<u64>,
}

fn stamp(bus: &mut Bus, t: u64, c0: u64) {
    bus.clock = c0 + t / 2;
}

fn dmacon(bus: &mut Bus, c0: u64, t: u64, v: u16) {
    stamp(bus, t, c0);
    bus.write_word(0xDF_F096, v);
}

fn put(bus: &mut Bus, c0: u64, t: u64, a: u32, v: u16) {
    stamp(bus, t, c0);
    bus.write_word(a, v);
}

fn put_lc(bus: &mut Bus, c0: u64, t: u64, a: u32, v: u32) {
    stamp(bus, t, c0);
    bus.write_word(a, (v >> 16) as u16);
    bus.write_word(a + 2, v as u16);
}

impl Ym {
    fn replay(&self) -> &GFile {
        self.files.last().expect("a file")
    }

    /// Stores a0 and a1 - a0 into the replay file's slots, as the host does between the data file's
    /// routines and the replay's, and makes a1 the length.
    fn hand_over(&mut self, bus: &mut Bus) {
        let (a0, a1) = (self.cpu.cpu.dar[8], self.cpu.cpu.dar[9]);
        let len = a1.wrapping_sub(a0);
        if let Some(a) = self.files[1].at(b'c') {
            bus.w32(a, a0);
        }
        if let Some(a) = self.files[1].at(b'd') {
            bus.w32(a, len);
        }
        self.cpu.a(1, len);
    }

    /// Calls the routine `c` of file `i`, if it has one (a6 holds its own address). Returns its cycles.
    fn routine(&mut self, bus: &mut Bus, i: usize, c: u8) -> Result<u64, String> {
        match self.files[i].at(c) {
            Some(a) => {
                self.cpu.a(6, a);
                self.cpu.call(bus, a, LIMIT_LOAD)
            }
            None => Ok(0),
        }
    }

    /// The load of the module (specification section 2): the relocation and patch routines.
    fn load(&mut self, bus: &mut Bus) -> Result<u64, String> {
        let yp = self.files[0].at(b'P').ok_or("ym2149: no YP")?;
        self.cpu.a(0, self.e.blk);
        self.cpu.a(1, yp);
        let mut cyc = 0;
        for c in [b'X', b'Y'] {
            let present = self.files[0].at(c).is_some();
            cyc += self.routine(bus, 0, c)?;
            if self.typ == 2 {
                // (the hand-over follows a data routine that exists; without one a0 and a1 are not
                // the data's start and end)
                if present {
                    self.hand_over(bus);
                }
                cyc += self.routine(bus, 1, c)?;
            }
        }
        Ok(cyc)
    }

    /// The start of subsong `n`. Returns the cycles of the routines.
    fn start_sub(&mut self, bus: &mut Bus, n: u16) -> Result<u64, String> {
        let map = self.files[0].at(b'`');
        let d0 = match map {
            Some(t) => bus.r32(t.wrapping_add(4 * (n as u32).saturating_sub(1))),
            None => n as u32,
        };
        self.cpu.cpu.dar[0] = d0;
        self.cpu.cpu.dar[1] = map.unwrap_or(0);
        let mut cyc = self.routine(bus, 0, b'Z')?;
        if self.typ == 2 {
            self.hand_over(bus);
            cyc += self.routine(bus, 1, b'Z')?;
        }
        Ok(cyc)
    }

    /// One tick: the module's routines, then the emulator's commit and computation.
    fn tick_inner(&mut self, bus: &mut Bus) -> Result<(), String> {
        let e = self.e;
        let c0 = bus.clock;
        for &r in &REC {
            bus.w16(e.blk + r + R_GAIN, 64);
        }
        bus.w16(e.fld(B_MASK), 0x000F);
        let h = if self.typ == 2 { 8 } else { 0 };
        let (pre, play) = (self.replay().at(b'['), self.replay().at(b'\\').ok_or("ym2149: no play routine")?);
        let mut t: u64;
        if let Some(a) = pre {
            t = 522 + h;
            self.cpu.a(0, a);
            self.cpu.cpu.dar[0] = a;
            t += self.cpu.call(bus, a, LIMIT_TICK)?;
            t += 44;
        } else {
            t = 546 + h;
        }
        self.cpu.a(0, play);
        t += self.cpu.call(bus, play, LIMIT_TICK)?;
        t += 40;
        let src = bus.r16(e.fld(B_SRC4)) as i16;
        t += match src {
            s if s < 0 => 157,
            0 => 326,
            1 => 351,
            _ => 359,
        };
        e.copy_fourth(bus);
        t += 107 + 42;
        // the commit (specification 5.2)
        let mask = bus.r16(e.fld(B_MASK));
        let mut restarted = 0u16;
        let mut full_taken = false;
        for i in 0..4 {
            let rec = e.blk + REC[i];
            let base = PAULA[i];
            let cnt = e.fld(B_COUNT + 2 * i as u32);
            if i > 0 {
                t += 34;
            }
            t += 25;
            let f = bus.r16(rec);
            let gain = bus.r16(rec + R_GAIN) as u32;
            let volg = |bus: &Bus, set: u32| ((bus.r16(rec + set + S_VOL) as u32 * gain) >> 6) as u16;
            if f & 0x8000 != 0 {
                bus.w16(rec, f & 0x7FFF);
                restarted |= DMABIT[i];
                bus.w16(cnt, 2);
                t += 56;
                dmacon(bus, c0, t, restarted);
                t += 10;
            } else {
                t += 25;
                if f & 0x4000 != 0 {
                    bus.w16(rec, f & !0x4000);
                    t += 72;
                    put(bus, c0, t, base + 6, bus.r16(rec + SET_A + S_PER));
                    t += 84;
                    let v = volg(bus, SET_A);
                    put(bus, c0, t, base + 8, v);
                    t += 16;
                    continue;
                }
            }
            // step 3
            t += 65;
            if DMABIT[i] & mask != 0 {
                full_taken = true;
            }
            let mut full = f & 0x80 == 0;
            if !full {
                let c = bus.r16(cnt);
                t += 17;
                if c == 0 {
                    t += 16;
                } else {
                    t += 21;
                    bus.w16(cnt, c - 1);
                    if c - 1 != 0 {
                        full = true;
                    } else {
                        t += 28;
                        put_lc(bus, c0, t, base, bus.r32(rec + SET_B));
                        t += 12;
                        put(bus, c0, t, base + 4, bus.r16(rec + SET_B + S_LEN));
                        t += 26;
                    }
                }
            }
            if full {
                t += 28;
                put_lc(bus, c0, t, base, bus.r32(rec + SET_A));
                t += 12;
                put(bus, c0, t, base + 4, bus.r16(rec + SET_A + S_LEN));
                t += 12;
                put(bus, c0, t, base + 6, bus.r16(rec + SET_A + S_PER));
                t += 84;
                let v = volg(bus, SET_A);
                put(bus, c0, t, base + 8, v);
                t += 16;
            }
        }
        t += 107 + 21;
        if full_taken {
            t += 28;
        }
        t += 21;
        if restarted & mask != 0 {
            let origin = self.origin;
            let line = |t: u64| ((c0 + t / 2).saturating_sub(origin) / 227 % 312) & 0xFF;
            t += 16;
            for _ in 0..11 {
                t += 8;
                let b = line(t);
                loop {
                    t += 8;
                    if line(t) != b {
                        t += 19;
                        break;
                    }
                    t += 9;
                }
            }
        }
        t += 36;
        let on = 0x8200 | mask;
        dmacon(bus, c0, t, on);
        t += 24;
        dmacon(bus, c0, t, on ^ 0x820F);
        // the new state
        if e.compute(bus).is_err() {
            return Err("ym2149: division by zero in the emulator".into());
        }
        e.copy_fourth(bus);
        stamp(bus, t, c0);
        Ok(())
    }
}

impl Replayer for Ym {
    fn format(&self) -> &'static str {
        "YM-2149"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.dead
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (self.first, self.last, self.cur)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if n < self.first || n > self.last || self.dead {
            return;
        }
        let bus = &mut *hw.bus;
        // the host's end of a subsong: all voices off, volumes to zero
        let t0 = bus.clock;
        bus.write_word(0xDF_F096, 0x000F);
        for ch in 0..4u64 {
            bus.clock = t0 + 10 * (ch + 1);
            bus.write_word(0xDF_F0A8 + 16 * ch as u32, 0);
        }
        self.cur = n;
        match self.start_sub(bus, n) {
            Ok(c) => {
                bus.clock = t0 + 40 + c / 2;
                if self.ticks == 0 {
                    self.resume = Some(bus.clock + TICK);
                }
            }
            Err(e) => {
                log::error!("{e}");
                self.dead = true;
            }
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.dead {
            return;
        }
        self.ticks += 1;
        let start = hw.bus.clock;
        if let Err(e) = self.tick_inner(hw.bus) {
            log::error!("{e}");
            self.dead = true;
            hw.bus.clock = hw.bus.clock.max(start);
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let h = parse(d)?;
    let typ = h.get(b'M')?;
    let mut files = vec![GFile { addr: base + h.magic as u32, h: h.clone() }];
    if typ == 2 {
        let name = cstr(d, h.magic + h.get(b'O')? as usize, 255);
        let rd_ = crate::deli::read_companion(m, &name).1.ok()?;
        let rh = parse(&rd_)?;
        if ![b'Z', b'\\', b'P', b'c', b'd'].iter().all(|&c| offset_ok(&rd_, &rh, c)) {
            return None;
        }
        let a = m.alloc(rd_.len() as u32 + 16);
        if a == 0 {
            return None;
        }
        m.bus.write_bytes(a, &rd_);
        files.push(GFile { addr: a + rh.magic as u32, h: rh });
    }
    let area = m.alloc(AREA + 8);
    let svc = m.alloc(14 * 4 + 4);
    let stack = m.alloc(0x2000);
    if area == 0 || svc == 0 || stack == 0 {
        return None;
    }
    let s = (area + BEFORE + 3) & !3;
    let e = Emu { s, blk: off(s, O_BLK), svc };
    e.init(&mut m.bus);
    for f in &files {
        if let Some(a) = f.at(b'P') {
            m.bus.w32(a, e.blk);
        }
    }
    let title = cstr(d, h.magic + h.get(b'S').unwrap_or(u32::MAX >> 1) as usize, 80);
    let first = h.get(b']').unwrap_or(1) as u16;
    let last = h.get(b'^').unwrap_or(1) as u16;
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut ym = Ym {
        e,
        cpu: Cpu68::new(stack + 0x2000, e),
        files,
        typ,
        first,
        last: last.max(first),
        cur: first,
        title,
        origin,
        dead: false,
        ticks: 0,
        resume: None,
    };
    let _lc = ym.load(&mut m.bus).map_err(|e| log::error!("{e}")).ok()?;
    let _yz = ym.start_sub(&mut m.bus, first).map_err(|e| log::error!("{e}")).ok()?;
    // the emulator's start-up writes, 10 colour clocks apart (specification 4.2)
    let t0 = origin + INIT_AT;
    let bus = &mut m.bus;
    let chans = [0xDF_F0A0u32, 0xDF_F0B0, 0xDF_F0C0, 0xDF_F0D0];
    let mut seq: Vec<Vec<(u32, u16)>> = Vec::new();
    for p in chans {
        seq.push(vec![(p, (s >> 16) as u16), (p + 2, s as u16)]);
    }
    for p in chans {
        seq.push(vec![(p + 4, 1)]);
    }
    for p in chans {
        seq.push(vec![(p + 6, 0x80)]);
    }
    seq.push(vec![(0xDF_F096, 0x000F)]);
    for (k, ws) in seq.iter().enumerate() {
        bus.clock = t0 + 10 * k as u64;
        for &(a, v) in ws {
            bus.write_word(a, v);
        }
    }
    bus.clock = t0 + 10 * seq.len() as u64 + 8;
    bus.write_word(0xDF_F096, 0x820F);
    bus.clock = (origin + INIT_AT + FIRST_TICK_AFTER).saturating_sub(TICK).max(bus.clock);
    Some(Box::new(ym))
}

#[cfg(test)]
#[path = "ym2149_tests.rs"]
mod tests;
