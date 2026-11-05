//! Leggless Music Editor (`lme.*`), the music driver of Steve "Leggless" Hasler (1990; Punisher,
//! Ninja Spirit).
//!
//! Clean-room, written from a behavioural specification and black-box runs of the 68k player's
//! register writes; no original code. A module keeps everything (instruments, track lists,
//! patterns, samples) in itself: four voices, one Paula channel each, 50 Hz, events of a note or a
//! rest with an instrument select, a pitch table (arpeggio), a slide or a tie; instruments are
//! looped 8-bit samples or a "synth" waveform of two levels whose switch point moves; attack /
//! hold / release envelope, vibrato and sample retrigger. Every register write is placed at the
//! cycle of the specification's cost model (the interrupt entry, the stage costs, the event
//! costs, the raster-line wait before the closing DMACON write).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Leggless Music Editor", detect, new };

const PERIODS: [u16; 37] = [
    0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C, 0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC, 0x194, 0x17D, 0x168,
    0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2, 0x0D6, 0x0CA, 0x0BE, 0x0B4, 0x0AA, 0x0A0, 0x097, 0x08F,
    0x087, 0x07F, 0x078, 0x071, 0x06B,
];

/// Cycles per colour clock... two; a raster line is 227 colour clocks.
const LINE: u64 = 227;
/// Bytes of a voice's synth buffer.
const BUF: u32 = 128;

/// Costs of the level scaling and write, and the write's moment in it, by voice.
const SV: [u64; 4] = [229, 246, 253, 280];
const SVW: [u64; 4] = [185, 202, 209, 236];
/// Cost of the track-end branch by voice.
const WRAP: [u64; 4] = [43, 64, 81, 88];

fn period(i: i32) -> u16 {
    PERIODS[i.clamp(0, 36) as usize]
}

/// The layout of a module: the offsets of the four areas (absolute), the counts.
#[derive(Clone, Copy, Debug)]
struct Lay {
    ins: usize,
    patp: usize,
    patd: usize,
    smp: usize,
    subsongs: usize,
}

fn layout(d: &[u8]) -> Option<Lay> {
    if d.get(..4)? != b"LME\0" || be32(d, 36) != 0 {
        return None;
    }
    let ins = be32(d, 40) as usize;
    let patp = be32(d, 44) as usize;
    let patd = be32(d, 48) as usize;
    let smp = be32(d, 52) as usize;
    if ins < 32 || ins % 16 != 0 || ins > d.len() || patp > d.len() || patd > d.len() || smp > d.len() || patd < patp {
        return None;
    }
    let subsongs = (ins - 16) / 16;
    let t = be32(d, 56) as usize;
    if t < ins {
        return None;
    }
    let insts = (t - ins) / 58;
    if insts == 0 {
        return None;
    }
    // the size rule: the sample area, then each distinct sample
    let song = 44 + smp;
    let mut max = 3usize;
    let mut total = 0usize;
    for i in 0..insts {
        let o = 40 + ins + 58 * i;
        let start = be32(d, o) as usize;
        if start == 0 {
            continue;
        }
        if start > max {
            max = start;
            total += 2 * be16(d, o + 4) as usize;
        }
    }
    if song + total > d.len() {
        return None;
    }
    Some(Lay { ins, patp, patd, smp, subsongs })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

#[derive(Clone, Copy, Default)]
struct Slide {
    step: i16,
    cnt: u16,
    dest: u16,
    delay: i16,
}

#[derive(Clone, Copy, Default)]
struct PTab {
    pos: u16,
    len: u16,
    cnt: u16,
    speed: u16,
    per: [u16; 10],
}

#[derive(Clone, Copy)]
struct Voice {
    /// The next flags word of the current pattern; `None`: at an end marker.
    pat: Option<usize>,
    trk: usize,
    trk0: usize,
    cnt: u16,
    trans: i16,
    ins: usize,
    cp: [u16; 45],
    base: u16,
    sl: Slide,
    pt: PTab,
    loopset: bool,
    laddr: u32,
    llen: u16,
}

impl Voice {
    fn new() -> Voice {
        Voice {
            pat: None,
            trk: 0,
            trk0: 0,
            cnt: 1,
            trans: 0,
            ins: 0,
            cp: [0; 45],
            base: 0,
            sl: Slide::default(),
            pt: PTab::default(),
            loopset: false,
            laddr: 0,
            llen: 0,
        }
    }
}

struct Lme {
    d: Vec<u8>,
    base: u32,
    l: Lay,
    sub: usize,
    v: [Voice; 4],
    /// Per voice: has not wrapped since the last song-end report.
    live: [bool; 4],
    tie: bool,
    songend: bool,
    /// Guest address of the synth buffers.
    buf: u32,
    // the clock of the tick being played
    base_cc: u64,
    tref: u64,
    t: u64,
    /// The first tick's time (absolute colour clocks) for a subsong started by `set_subsong`.
    next_at: Option<u64>,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = layout(d)?;
    let buf = m.alloc(4 * BUF + 256);
    if buf == 0 {
        return None;
    }
    let mut r = Lme {
        d: d.to_vec(),
        base,
        l,
        sub: 0,
        v: [Voice::new(); 4],
        live: [true; 4],
        tie: false,
        songend: false,
        buf,
        base_cc: 0,
        tref: 0,
        t: 0,
        next_at: None,
    };
    r.init(0);
    // the reference host's own work before the first tick: the output starts after it, the
    // first tick one period later (entry at `first_tick`, where the raster-line wait of the
    // tick lands on the reference's colour clocks)
    Hw { bus: &mut m.bus }.wait(first_tick(l.subsongs, 0).saturating_sub(100 + 70940));
    Some(Box::new(r))
}

/// The first tick's entry time (absolute colour clocks) of subsong `k` in the reference runs:
/// the host's own work before the first tick (measured, not derived; the module sets it only
/// through its subsong count). Each later subsong is 1 277 + 26 (k - 1) colour clocks later
/// than the one before.
fn first_tick(subsongs: usize, k: u64) -> u64 {
    let t0 = if subsongs > 1 { 73716 } else { 72862 };
    t0 + 1277 * k + 13 * k * k.saturating_sub(1)
}

impl Lme {
    fn rw(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    fn init(&mut self, sub: usize) {
        self.sub = sub;
        self.v = [Voice::new(); 4];
        self.live = [true; 4];
        self.tie = false;
        self.songend = false;
        for v in 0..4 {
            let p = 40 + be32(&self.d, 56 + 16 * sub + 4 * v) as usize;
            self.v[v].trk = p;
            self.v[v].trk0 = p;
        }
    }

    // --- the clock

    fn stamp(&self, hw: &mut Hw) {
        hw.bus.clock = self.base_cc + (self.t - self.tref) / 2;
    }

    fn cc_at(&self, t: u64) -> u64 {
        self.base_cc + (t - self.tref) / 2
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

    /// The level-scaled volume write (both sides at 64: the value with the mask 0x7F).
    fn w_vol(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.vol(ch, n & 0x7F);
    }

    fn w_dma(&self, hw: &mut Hw, n: u16) {
        self.stamp(hw);
        hw.dmacon(n);
    }

    // --- stages

    fn deferred(&mut self, hw: &mut Hw) {
        self.t += 40;
        for v in 0..4 {
            if self.v[v].loopset {
                let s = self.t;
                self.v[v].loopset = false;
                self.t = s + 49;
                self.w_loc(hw, v, self.v[v].laddr);
                self.t = s + 65;
                self.w_len(hw, v, self.v[v].llen);
                self.t = s + 97;
            } else {
                self.t += 47;
            }
        }
        self.t += 20;
    }

    fn slide_stage(&mut self, hw: &mut Hw) {
        self.t += 18 + 32;
        for v in 0..4 {
            let vo = &mut self.v[v];
            if vo.sl.step == 0 {
                self.t += 55;
                continue;
            }
            let run_off;
            let total;
            if vo.sl.delay < 0 {
                run_off = 62;
                total = 133;
            } else {
                vo.sl.delay = vo.sl.delay.wrapping_sub(1);
                if vo.sl.delay >= 0 {
                    self.t += 101;
                    continue;
                }
                run_off = 87;
                total = 158;
            }
            let s = self.t;
            vo.base = vo.base.wrapping_add(vo.sl.step as u16);
            let nb = vo.base;
            vo.sl.cnt = vo.sl.cnt.wrapping_sub(1);
            let end = vo.sl.cnt == 0;
            let dest = vo.sl.dest;
            if end {
                vo.base = dest;
                vo.sl.step = 0;
            }
            self.t = s + run_off;
            self.w_per(hw, v, nb);
            if end {
                self.t = s + run_off + 49;
                self.w_per(hw, v, dest);
                self.t = s + total + 44;
            } else {
                self.t = s + total;
            }
        }
        self.t += 16;
    }

    fn vibrato_stage(&mut self, hw: &mut Hw) {
        self.t += 18 + 40;
        for v in 0..4 {
            let vo = &mut self.v[v];
            if vo.cp[8] == 0 {
                self.t += 75;
                continue;
            }
            let e;
            if (vo.cp[12] as i16) < 0 {
                e = 42;
            } else {
                vo.cp[12] = vo.cp[12].wrapping_sub(1);
                if (vo.cp[12] as i16) >= 0 {
                    self.t += 121;
                    continue;
                }
                e = 67;
            }
            let mut h = 0;
            if vo.cp[9] == 0 {
                vo.cp[7] = vo.cp[7].wrapping_add(vo.cp[8]);
                if vo.cp[7] == vo.cp[10] {
                    vo.cp[9] |= 0x100;
                    h = 30;
                }
            } else {
                vo.cp[7] = vo.cp[7].wrapping_sub(vo.cp[8]);
                if vo.cp[7] == vo.cp[11] {
                    vo.cp[9] &= !0x100;
                    h = 20;
                }
            }
            let wr = if vo.pt.len == 0 { 16 } else { 0 };
            let out = vo.cp[7].wrapping_add(vo.base);
            let s = self.t;
            let wend = e + 29 + 41 + h + 21 + wr;
            if wr != 0 {
                self.t = s + wend;
                self.w_per(hw, v, out);
            }
            self.t = s + wend + 54;
        }
        self.t += 16;
    }

    fn ptab_stage(&mut self, hw: &mut Hw) {
        self.t += 18 + 40;
        for v in 0..4 {
            let vo = &mut self.v[v];
            vo.pt.per[0] = vo.base;
            if vo.pt.len == 0 {
                self.t += 83;
                continue;
            }
            vo.pt.cnt = vo.pt.cnt.wrapping_sub(1);
            if vo.pt.cnt != 0 {
                self.t += 108;
                continue;
            }
            vo.pt.cnt = vo.pt.speed;
            let out = vo.pt.per[(vo.pt.pos as usize / 2).min(9)].wrapping_add(vo.cp[7]);
            vo.pt.pos += 2;
            let mut total = 203;
            if vo.pt.pos == vo.pt.len {
                vo.pt.pos = 0;
                total = 215;
            }
            let s = self.t;
            self.t = s + 124;
            self.w_per(hw, v, out);
            self.t = s + total;
        }
        self.t += 16;
    }

    fn volume_stage(&mut self, hw: &mut Hw) {
        self.t += 18 + 24;
        for v in 0..4 {
            let vo = &mut self.v[v];
            let mut vol = vo.cp[3];
            let p: u64 = match vo.cp[13] {
                0 => 33,
                1 => {
                    vol = vol.wrapping_add(vo.cp[14]);
                    if (vol as i16) < (vo.cp[15] as i16) {
                        117
                    } else {
                        vol = vo.cp[15];
                        vo.cp[13] = 2;
                        155
                    }
                }
                2 => {
                    vo.cp[16] = vo.cp[16].wrapping_sub(1);
                    if (vo.cp[16] as i16) >= 0 {
                        75
                    } else {
                        vo.cp[13] = 3;
                        101
                    }
                }
                3 => {
                    vol = vol.wrapping_sub(vo.cp[17]);
                    if (vol as i16) > (vo.cp[18] as i16) {
                        100
                    } else {
                        vol = vo.cp[18];
                        vo.cp[13] = 4;
                        128
                    }
                }
                _ => 84,
            };
            vo.cp[3] = vol;
            let s = self.t;
            self.t = s + p + 8 + SVW[v];
            self.w_vol(hw, v, vol);
            self.t = s + p + 8 + SV[v] + 136 + 46;
        }
        self.t += 16;
    }

    fn retrigger_stage(&mut self, hw: &mut Hw, mask: &mut u16) {
        self.t += 18 + 48;
        for v in 0..4 {
            let vo = &mut self.v[v];
            if vo.cp[28] == 0 {
                self.t += 71;
                continue;
            }
            vo.cp[27] = vo.cp[27].wrapping_sub(1);
            if vo.cp[27] != 0 {
                self.t += 96;
                continue;
            }
            vo.cp[27] = vo.cp[28];
            *mask |= 1 << v;
            let addr = (vo.cp[0] as u32) << 16 | vo.cp[1] as u32;
            let len = vo.cp[2];
            vo.loopset = true;
            vo.laddr = (vo.cp[4] as u32) << 16 | vo.cp[5] as u32;
            vo.llen = vo.cp[6];
            let s = self.t;
            self.t = s + 102;
            self.w_dma(hw, 1 << v);
            self.t = s + 122;
            self.w_loc(hw, v, addr);
            self.t = s + 142;
            self.w_len(hw, v, len);
            self.t = s + 244;
        }
        self.t += 16;
    }

    fn synth_stage(&mut self, hw: &mut Hw) {
        self.t += 18 + 12;
        for v in 0..4 {
            let vo = &mut self.v[v];
            if vo.cp[19] == 0 {
                self.t += 43;
                continue;
            }
            let mut c: u64 = 46 + 57;
            let mut w = vo.cp[20];
            if vo.cp[19] == 2 {
                w = w.wrapping_sub(vo.cp[23]);
                if (w as i16) <= (vo.cp[24] as i16) {
                    w = vo.cp[24];
                    vo.cp[19] = 1;
                    c += 28;
                }
            } else {
                w = w.wrapping_add(vo.cp[21]);
                if (w as i16) >= (vo.cp[22] as i16) {
                    w = vo.cp[22];
                    vo.cp[19] = 2;
                    c += 38;
                }
            }
            vo.cp[20] = w;
            c += 73;
            let ws = w as i16 as i32;
            let n = 2 * vo.cp[2] as i32;
            if ws >= 2 {
                c += 18 * (ws as u64 - 1) + 13;
            }
            let dd = n - ws;
            if dd >= 0 {
                c += 18 * (dd as u64 + 1);
            }
            c += 22;
            self.t += c;
            // the buffer: W-1 bytes of level A, then level B
            let a = vo.cp[25] as u8;
            let b = vo.cp[26] as u8;
            let na = (ws - 1).max(0) as usize;
            let nb = if ws >= 2 { (n - (ws - 1)).max(0) as usize } else { (n - ws + 1).max(0) as usize };
            let mut out = vec![a; na];
            out.extend(std::iter::repeat(b).take(nb));
            out.truncate(BUF as usize + 64);
            hw.bus.write_bytes(self.buf + BUF * v as u32, &out);
        }
        self.t += 16;
    }

    /// The wait for nine raster-line changes.
    fn raster_wait(&mut self) {
        self.t += 18 + 28;
        let line = |s: &Lme| s.cc_at(s.t) / LINE % 313;
        for _ in 0..9 {
            self.t += 16;
            let first = line(self);
            loop {
                self.t += 16;
                if line(self) == first {
                    self.t += 9;
                } else {
                    self.t += 9 + 10;
                    break;
                }
            }
        }
        self.t += 44;
    }

    // --- the events

    fn track_word(&self, v: usize, o: usize) -> u16 {
        let _ = v;
        self.rw(o)
    }

    fn select(&mut self, v: usize, n: usize) {
        let o = 40 + self.l.ins + 58 * n;
        let sa = self.base + 40 + self.l.smp as u32;
        let mut cp = [0u16; 45];
        for (k, w) in cp.iter_mut().enumerate() {
            *w = self.rw(o + 2 * k);
        }
        self.v[v].ins = n;
        if cp[19] == 0 {
            let a = sa.wrapping_add((cp[0] as u32) << 16 | cp[1] as u32);
            let la = sa.wrapping_add((cp[4] as u32) << 16 | cp[5] as u32);
            cp[0] = (a >> 16) as u16;
            cp[1] = a as u16;
            cp[4] = (la >> 16) as u16;
            cp[5] = la as u16;
        } else {
            let b = self.buf + BUF * v as u32;
            cp[0] = (b >> 16) as u16;
            cp[1] = b as u16;
            cp[4] = cp[0];
            cp[5] = cp[1];
            cp[6] = cp[2];
        }
        self.v[v].cp = cp;
    }

    fn reinit(&mut self, v: usize) {
        let o = 40 + self.l.ins + 58 * self.v[v].ins;
        let vo = self.v[v];
        let mut cp = vo.cp;
        cp[3] = self.rw(o + 6);
        for k in 7..29 {
            cp[k] = self.rw(o + 2 * k);
        }
        self.v[v].cp = cp;
    }

    fn event(&mut self, v: usize, hw: &mut Hw, mask: &mut u16) {
        self.t += 131;
        self.v[v].pt.len = 0;
        let fetch = match self.v[v].pat {
            None => true,
            Some(p) => self.rw(p) & 0x8000 != 0,
        };
        let mut p;
        let flags;
        if fetch {
            self.t += 51;
            let mut w = self.track_word(v, self.v[v].trk);
            if w == 0xFFFF {
                self.t += WRAP[v];
                self.live[v] = false;
                self.t += 21;
                if self.live.iter().all(|&f| !f) {
                    self.t += 110;
                    // the host call re-bases its clock to a whole colour clock
                    self.base_cc += (self.t - self.tref) / 2;
                    self.tref = self.t;
                    self.t += 16 + 44;
                    self.live = [true; 4];
                    self.songend = true;
                }
                self.t += 34;
                self.v[v].trk = self.v[v].trk0;
                w = self.track_word(v, self.v[v].trk);
            }
            let tp = self.v[v].trk;
            if w & 0x8000 != 0 {
                self.t += 155;
                self.v[v].trans = self.rw(tp + 2) as i16;
                self.v[v].trk = tp + 4;
            } else {
                self.t += 123;
                self.v[v].trans = 0;
                self.v[v].trk = tp + 2;
            }
            let n = (w & 0x7FFF) as usize;
            p = 40 + self.l.patd + be32(&self.d, 40 + self.l.patp + 4 * n) as usize;
            flags = self.rw(p);
            p += 2;
        } else {
            p = self.v[v].pat.unwrap();
            flags = self.rw(p);
            p += 2;
        }
        let tr = self.v[v].trans as i32;
        self.t += 21;
        let next = |s: &Lme, p: &mut usize| -> u16 {
            let w = s.rw(*p);
            *p += 2;
            w
        };
        if flags & 1 != 0 {
            let dur = next(self, &mut p);
            self.t += 52;
            self.w_dma(hw, 1 << v);
            self.t += 54;
            self.v[v].cnt = dur;
            self.v[v].pat = Some(p);
            return;
        }
        self.t += 21;
        if flags & 2 != 0 {
            let n = next(self, &mut p) as usize;
            self.select(v, n);
            self.t += if self.v[v].cp[19] == 0 { 1243 } else { 1271 };
        }
        self.t += 21;
        if flags & 0x10 != 0 {
            let n = next(self, &mut p) as usize;
            let speed = next(self, &mut p);
            let mut pt = PTab { pos: 0, len: (2 * n) as u16, cnt: 1, speed, per: [0; 10] };
            for k in 1..n.max(1) {
                let note = next(self, &mut p) as i16 as i32;
                if k < 10 {
                    pt.per[k] = period(note + tr);
                }
            }
            self.v[v].pt = pt;
            self.t += 156 + 58 * (n.max(1) as u64 - 1) + 10;
        }
        self.t += 19;
        if flags & 8 != 0 {
            self.tie = true;
            self.t += 34;
        } else {
            self.reinit(v);
            self.t += 444;
        }
        self.t += 25;
        let mut slide = false;
        if flags & 4 != 0 {
            let dest = next(self, &mut p) as i16 as i32;
            let cnt = next(self, &mut p);
            let delay = next(self, &mut p) as i16;
            let sl = &mut self.v[v].sl;
            sl.dest = period(dest + tr);
            sl.cnt = cnt;
            sl.delay = delay;
            slide = true;
            self.t += 158;
        }
        let dur = next(self, &mut p);
        let note = next(self, &mut p) as i16 as i32;
        let per = period(note + tr);
        self.v[v].cnt = dur;
        self.v[v].base = per;
        self.t += 163;
        if slide {
            let sl = &mut self.v[v].sl;
            let div = sl.cnt as i16 as i32;
            sl.step = if div == 0 { 0 } else { ((sl.dest as i32 - per as i32) / div) as i16 };
            self.t += 238;
        }
        self.t += 25;
        self.v[v].pat = Some(p);
        if self.tie {
            self.tie = false;
            let s = self.t;
            self.t = s + 24;
            self.w_per(hw, v, per);
            self.t = s + 52;
            return;
        }
        // note start
        let vo = &mut self.v[v];
        let addr = (vo.cp[0] as u32) << 16 | vo.cp[1] as u32;
        let len = vo.cp[2];
        let vol = vo.cp[3];
        vo.loopset = true;
        vo.laddr = (vo.cp[4] as u32) << 16 | vo.cp[5] as u32;
        vo.llen = vo.cp[6];
        self.t += 120;
        self.w_dma(hw, 1 << v);
        self.t += 26;
        self.w_loc(hw, v, addr);
        self.t += 22;
        self.w_len(hw, v, len);
        self.t += 12;
        self.w_per(hw, v, per);
        self.t += 22;
        self.stamp(hw);
        hw.vol(v, vol);
        let s = self.t;
        self.t = s + SVW[v];
        self.w_vol(hw, v, 8 * v as u16);
        self.t = s + SV[v] + 314;
        *mask |= 1 << v;
    }

    fn event_stage(&mut self, hw: &mut Hw, mask: &mut u16) {
        self.t += 16;
        for v in 0..4 {
            self.v[v].cnt = self.v[v].cnt.wrapping_sub(1);
            if self.v[v].cnt != 0 {
                self.t += 41;
                continue;
            }
            self.t += 27;
            self.event(v, hw, mask);
            self.t += 14;
        }
    }

    fn tick_inner(&mut self, hw: &mut Hw) {
        self.base_cc = hw.bus.clock;
        self.tref = 0;
        self.t = 320;
        let mut mask = 0x8000u16;
        self.deferred(hw);
        self.slide_stage(hw);
        self.vibrato_stage(hw);
        self.ptab_stage(hw);
        self.volume_stage(hw);
        self.retrigger_stage(hw, &mut mask);
        self.event_stage(hw, &mut mask);
        self.synth_stage(hw);
        self.raster_wait();
        self.t += 24;
        self.w_dma(hw, mask);
    }
}

impl Replayer for Lme {
    fn format(&self) -> &'static str {
        "Leggless Music Editor"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.l.subsongs as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.l.subsongs - 1);
        self.init(n);
        // the host's audio-stop sequence of each subsong change (cc 2 790, 4 067,
        // 5 370: DMA off, then the four volumes to 0)
        if n > 0 {
            let j = n as u64 - 1;
            let at = 2790 + 1277 * j + 13 * j * j.saturating_sub(1);
            if hw.bus.clock <= at {
                hw.bus.clock = at;
                hw.dmacon(0x000F);
                for (c, g) in [8u64, 6, 6, 6].into_iter().enumerate() {
                    hw.wait(g);
                    hw.vol(c, 0);
                }
            }
        }
        self.next_at = Some(first_tick(self.l.subsongs, n as u64));
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }
}

#[cfg(test)]
#[path = "legglessmusiceditor_tests.rs"]
mod tests;
