//! Art And Magic (`aam.*`), the sound driver of the game Agony (music by Jeroen Tel).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player; no original code. The module carries the game's own driver, which
//! is never run here: the replay works on the module's data only (the order lists, note streams,
//! instrument records, arpeggio lists, period table and the IFF 8SVX sample section that the
//! driver's entry routine converts in place into a wave table and a pool of sample bodies).
//!
//! Three Paula voices (hardware channels 1 to 3). Every register write is placed at the clock of
//! the original's code (static 68000 cycle costs of the path taken within the tick).
//!
//! Choices where the specification leaves the behaviour open: a pattern number outside the stream
//! table, an order list or stream that leaves the file, and a stream pointer outside the file end
//! the voice (without a song-end mark); a table index outside the real tables reads the bytes of
//! the file at that position (zero past the file's end); the fade-out machinery of the driver is
//! inert and not modelled.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Art And Magic", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// The first tick starts this many colour clocks after the first register write (spec 12).
const FIRST_TICK: u64 = 72592;
const EXIT: u32 = 0x4558_4954;
const FILES_CAP: usize = 64;

fn detect(d: &[u8]) -> bool {
    d.len() >= 0x500 && be32(d, 0x1A) == EXIT && be32(d, d.len() - 4) == EXIT
}

/// The table offsets of a module (spec 2.4).
#[derive(Clone, Copy, Debug)]
struct Lay {
    b: bool,
    wave: usize,
    order: usize,
    streams: usize,
    nstreams: usize,
    arp: usize,
    inst: usize,
    per: usize,
    /// Stored pointer minus file offset.
    k: i64,
}

fn disp(d: &[u8], at: usize) -> Option<usize> {
    let v = at as i64 + be16(d, at) as i16 as i64;
    (v >= 0 && (v as usize) < d.len()).then_some(v as usize)
}

fn locate(d: &[u8]) -> Option<Lay> {
    if !detect(d) {
        return None;
    }
    let b = d.get(0x20E..0x212) == Some(&[1, 2, 4, 8][..]);
    let wave = disp(d, 0x14)?;
    let order = disp(d, 0xE2)?;
    let inst = disp(d, 0x15E)?;
    let (per, streams, arp) = if b {
        (disp(d, 0x3A2)?, disp(d, 0x478)?, disp(d, 0x4EA)?)
    } else {
        (disp(d, 0x3A0)?, disp(d, 0x474)?, disp(d, 0x4E6)?)
    };
    let nstreams = if b { 64 } else { 96 };
    if order + 40 > d.len() || streams + 4 * nstreams > d.len() || arp + 128 > d.len() {
        return None;
    }
    let k = be32(d, arp + 124) as i64 - inst as i64;
    Some(Lay { b, wave, order, streams, nstreams, arp, inst, per, k })
}

/// The entry routine's conversion of the sample section (spec 2.6, 2.7), on a copy of the file.
fn convert(d: &mut [u8], base: u32, lay: &Lay) {
    let n = d.len() - 4; // the final EXIT
    let Some(mut pos) = disp(d, 0x0C) else { return };
    let mut pool = pos;
    for k in 0..FILES_CAP {
        if pos + 12 > n || be32(d, pos) != 0x464F_524D {
            break;
        }
        let one = be32(d, pos + 20);
        let rep = be32(d, pos + 24);
        // walk the chunks to the BODY
        let mut p = pos + 12;
        let mut body = None;
        for _ in 0..16 {
            if p + 8 > n {
                break;
            }
            let size = be32(d, p + 4) as usize;
            if be32(d, p) == 0x424F_4459 {
                body = Some((p + 8, size));
                break;
            }
            p = p.saturating_add(8).saturating_add(size);
        }
        let Some((bpos, size)) = body else { break };
        let (os, rp) = if rep == 0 { (one.wrapping_sub(2), 2u32) } else { (one, rep) };
        let start = base.wrapping_add(pool as u32);
        let e = lay.wave + 12 * k;
        if e + 12 <= d.len() {
            d[e..e + 4].copy_from_slice(&start.to_be_bytes());
            d[e + 4..e + 8].copy_from_slice(&start.wrapping_add(os).to_be_bytes());
            d[e + 8..e + 12].copy_from_slice(&os.wrapping_add(rp).to_be_bytes());
        }
        for i in 0..size {
            if bpos + i >= n || pool + i >= n {
                break;
            }
            d[pool + i] = d[bpos + i];
        }
        pool = pool.saturating_add(size);
        pos = bpos.saturating_add(size);
    }
}

#[derive(Clone, Default)]
struct Voice {
    active: bool,
    list_start: usize,
    list_pos: usize,
    stream_pos: usize,
    pattern: i8,
    repeat_left: u8,
    transpose: u8,
    bank: u8,
    speed_a: u16,
    speed_b: u16,
    step_count: u16,
    step_toggle: u16,
    row_count: u8,
    row_len: u8,
    note: u8,
    inst: i32,
    wave: usize,
    period: u32,
    base_vol: u16,
    env_vol: u16,
    phase: u8,
    attack: bool,
    env_counter: u16,
    decay_limit: u16,
    flags: u8,
    vib_threshold: u16,
    vib_shift: u8,
    vib_count: u8,
    vib_dir: u8,
    slide_trigger: u16,
    slide_mul: u8,
    slide_target: u8,
    arp_sel: usize,
    arp_pos: usize,
}

pub struct ArtAndMagic {
    d: Vec<u8>,
    /// The file length (the image holds two more bytes: the initial stream).
    n: usize,
    lay: Lay,
    v: [Voice; 3],
    songend: bool,
    // timing
    h: u64,
    t: u64,
    /// After a song-end notification: the cycle count of the last one and the stamp then.
    tn: Option<(u64, u64)>,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = locate(d)?;
    let mut img = d.to_vec();
    convert(&mut img, base, &lay);
    m.bus.write_bytes(base, &img);
    let n = img.len();
    img.extend_from_slice(&[0xFF, 0xFF]);
    let mut s = ArtAndMagic { d: img, n, lay, v: Default::default(), songend: false, h: 0, t: 0, tn: None };
    for vi in 0..3 {
        let e = lay.order + 8 * (vi + 1);
        let p = be32(&s.d, e) as i64 - lay.k;
        let start = if p >= 0 && (p as usize) < n { p as usize } else { n };
        let v = &mut s.v[vi];
        *v = Voice {
            active: start < n,
            list_start: start,
            list_pos: start,
            stream_pos: n,
            row_count: 1,
            speed_a: be16(&s.d, e + 4),
            speed_b: be16(&s.d, e + 6),
            ..Default::default()
        };
    }
    // the entry routine's four writes to AUD0DAT, at the start of the run (spec 4)
    let i = m.bus.clock;
    for off in [0u64, 17, 33, 50] {
        m.bus.clock = i + off;
        m.bus.write_word(0xDFF0AA, 0);
    }
    // the entry routine runs on: the song starts so that the first tick falls one period later
    m.bus.clock = i + FIRST_TICK - TICK;
    Some(Box::new(s))
}

impl ArtAndMagic {
    fn b(&self, o: i64) -> u8 {
        if o >= 0 {
            self.d.get(o as usize).copied().unwrap_or(0)
        } else {
            0
        }
    }

    fn inst_b(&self, vi: usize, k: i64) -> u8 {
        self.b(self.lay.inst as i64 + self.v[vi].inst as i64 + k)
    }

    /// The period table entry `i` (a signed index; outside the table the file's bytes).
    fn pt(&self, i: i32) -> u16 {
        let o = self.lay.per as i64 + 2 * i as i64;
        ((self.b(o) as u16) << 8) | self.b(o + 1) as u16
    }

    /// A stored pointer as a file offset (None outside the file).
    fn ptr(&self, stored: u32) -> Option<usize> {
        let p = stored as i64 - self.lay.k;
        (p >= 0 && (p as usize) < self.n).then_some(p as usize)
    }

    // --- the clock

    fn stamp(&self) -> u64 {
        match self.tn {
            Some((tn, base)) => base + (self.t - tn) / 2,
            None => self.t / 2,
        }
    }

    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.h + self.stamp();
    }

    // --- the stream

    fn nb(&mut self, vi: usize) -> u8 {
        let p = self.v[vi].stream_pos;
        self.v[vi].stream_pos = p.wrapping_add(1);
        if p < self.d.len() {
            self.d[p]
        } else {
            0xFE
        }
    }

    fn back(&mut self, vi: usize) {
        let v = &mut self.v[vi];
        v.stream_pos = v.stream_pos.wrapping_sub(1);
    }

    // --- the tick

    fn tick_inner(&mut self, hw: &mut Hw) {
        self.h = hw.bus.clock;
        self.t = 258;
        self.tn = None;
        for vi in 0..3 {
            self.voice(vi, hw);
        }
    }

    fn voice(&mut self, vi: usize, hw: &mut Hw) {
        let ch = vi + 1;
        let bit = 1u16 << ch;
        self.t += 22 + 113;
        if !self.v[vi].active {
            self.t += 16;
            return;
        }
        self.t += 100;
        let det = (self.inst_b(vi, 4) as i8).wrapping_neg() as i16 as u16;
        let per = ((self.v[vi].period >> 16) as u16).wrapping_add(det);
        self.at(hw);
        hw.per(ch, per);
        self.t += 286;
        let p = ((0x40u32 * self.v[vi].env_vol as u32) & 0xFFFF) >> 6;
        self.at(hw);
        hw.vol(ch, p as u16);
        self.t += 44 + 25;
        match self.v[vi].phase {
            2 => {
                self.t += 62;
                self.at(hw);
                hw.dmacon(0x8000 | bit);
                self.v[vi].phase = 1;
                self.t += 10;
            }
            ph => {
                self.t += 25;
                if ph == 1 {
                    self.v[vi].phase = 0;
                    let w = self.v[vi].wave;
                    let a0 = be32(&self.d, w);
                    let lp = be32(&self.d, w + 4);
                    let total = be32(&self.d, w + 8);
                    self.t += 104;
                    self.at(hw);
                    hw.loc(ch, lp);
                    let rep = total.wrapping_sub(lp.wrapping_sub(a0));
                    self.t += 66;
                    self.at(hw);
                    hw.len(ch, (rep >> 1) as u16);
                }
            }
        }
        self.t += 25;
        let wrapped = self.v[vi].step_count == 0;
        self.v[vi].step_count = self.v[vi].step_count.wrapping_sub(1);
        if wrapped {
            self.t += 103;
            let v = &mut self.v[vi];
            v.step_toggle = !v.step_toggle;
            v.step_count = if v.step_toggle & 2 != 0 { v.speed_b } else { v.speed_a };
            v.row_count = v.row_count.wrapping_sub(1);
            if v.row_count == 0 {
                self.t += 18;
                self.event(vi, hw);
                self.t += 16;
                return;
            }
        }
        self.t += 21;
        if self.v[vi].note & 0x80 != 0 {
            self.t += 16;
            return;
        }
        self.t += 18;
        self.effects(vi, hw);
        self.t += 16;
    }

    // --- events (spec 6)

    fn event(&mut self, vi: usize, hw: &mut Hw) {
        let ch = vi + 1;
        let bit = 1u16 << ch;
        self.t += if self.lay.b { 59 } else { 73 };
        self.v[vi].slide_trigger = 0xFFFF;
        let mut b = self.nb(vi);
        if b != 0xFB {
            self.t += 38;
            self.at(hw);
            hw.dmacon(bit);
            self.t += 10;
        } else {
            self.t += 8;
            b = self.nb(vi);
        }
        self.t += 17;
        if b == 0xFF {
            self.t += 18;
            self.next_pattern(vi);
            self.t += 24;
            b = self.nb(vi);
        }
        self.t += 17;
        if b == 0xFE {
            self.t += 32;
            self.v[vi].active = false;
            return;
        }
        self.t += 17;
        if b == 0xFD {
            self.slide_body(vi);
            return self.finish(vi, hw);
        }
        self.v[vi].base_vol = 0;
        self.t += 33;
        if b == 0xFC {
            self.t += 51;
            self.v[vi].base_vol = self.nb(vi) as i8 as i16 as u16;
            b = self.nb(vi);
            if b == 0xFD {
                self.slide_body(vi);
                return self.finish(vi, hw);
            }
            self.t += 19;
            if b >= 0xFB {
                self.t += 8;
                self.back(vi);
                return self.finish(vi, hw);
            }
        }
        self.t += 17;
        if b >= 0xF0 {
            self.t += 62;
            let v = &mut self.v[vi];
            v.row_len = b & 15;
            v.note = 0xFF;
            v.row_count = v.row_len;
            self.t += 36;
            return;
        }
        self.t += 17;
        if b >= 0xC0 {
            self.t += 75;
            let x = ((b & 0x3F).wrapping_add(self.v[vi].bank)) as i8 as i32;
            self.v[vi].inst = x * 8;
            b = self.nb(vi);
            if b == 0xFD {
                self.slide_body(vi);
                return self.finish(vi, hw);
            }
            self.t += 17;
            if b >= 0xC0 {
                self.t += 8;
                self.back(vi);
                return self.finish(vi, hw);
            }
        }
        self.t += 17;
        if b >= 0x80 {
            self.t += 47;
            self.v[vi].row_len = b & 0x3F;
            b = self.nb(vi);
            if b == 0xFD {
                self.slide_body(vi);
                return self.finish(vi, hw);
            }
            self.t += 17;
            if b >= 0x80 {
                self.t += 8;
                self.back(vi);
                return self.finish(vi, hw);
            }
        }
        self.t += 17;
        if b >= 0x60 {
            self.t += 55;
            self.v[vi].arp_sel = (b & 0x1F) as usize;
            b = self.nb(vi);
            if b >= 0x60 {
                self.t += 8;
                self.back(vi);
                return self.finish(vi, hw);
            }
        }
        self.t += 34;
        self.v[vi].note = b.wrapping_add(self.v[vi].transpose);
        self.finish(vi, hw)
    }

    /// The slide event's three bytes (spec 6.2).
    fn slide_body(&mut self, vi: usize) {
        self.t += 164;
        let g = self.nb(vi);
        let n1 = self.nb(vi);
        let n2 = self.nb(vi);
        let v = &mut self.v[vi];
        v.slide_mul = (g >> 4) + 1;
        v.slide_trigger = (v.row_len.wrapping_sub(g & 15).wrapping_add(1)) as i8 as i16 as u16;
        v.note = n1.wrapping_add(v.transpose);
        v.slide_target = n2.wrapping_add(v.transpose);
    }

    fn finish(&mut self, vi: usize, hw: &mut Hw) {
        self.t += 34;
        self.note_start(vi, hw);
        self.t += 36;
        let v = &mut self.v[vi];
        v.row_count = v.row_len;
    }

    /// The note start (spec 6.3, 11.6).
    fn note_start(&mut self, vi: usize, hw: &mut Hw) {
        let ch = vi + 1;
        let w = self.lay.wave + self.inst_b(vi, 7) as usize * 12;
        self.v[vi].wave = w;
        self.t += 152;
        self.at(hw);
        hw.loc(ch, be32(&self.d, w));
        self.t += 40;
        self.at(hw);
        hw.len(ch, (be32(&self.d, w + 8) >> 1) as u16);
        self.t += 21;
        if self.v[vi].base_vol == 0 {
            self.t += 30;
            self.v[vi].base_vol = self.inst_b(vi, 0) as i8 as i16 as u16;
        }
        self.t += if self.lay.b { 159 } else { 157 };
        let pe = self.pt(self.v[vi].note as i8 as i32);
        let m = self.inst_b(vi, 6);
        {
            let v = &mut self.v[vi];
            v.env_vol = v.base_vol;
            v.period = (pe as u32) << 16;
            v.phase = 2;
            v.attack = false;
            v.decay_limit = 0xFFFF;
            if m & 0x80 != 0 {
                v.attack = true;
                v.env_vol = 0;
                v.env_counter = 0x200;
            } else if m != 0 {
                v.decay_limit = m as u16 + 1;
                v.env_counter = 0x200;
            }
        }
        self.t += if m & 0x80 != 0 {
            54
        } else if m == 0 {
            9
        } else {
            41
        };
        self.t += 27;
        if self.inst_b(vi, 3) & 0x80 != 0 {
            self.t += 40;
            self.at(hw);
            hw.bus.write_byte(0xBFE001, 0x00);
        }
        self.t += 40;
        self.at(hw);
        hw.bus.write_byte(0xBFE001, 0x02);
        self.t += 16;
    }

    // --- the next pattern (spec 7, 11.5)

    fn next_pattern(&mut self, vi: usize) {
        self.t += 25;
        let old = self.v[vi].repeat_left;
        self.v[vi].repeat_left = old.wrapping_sub(1);
        if old != 0 {
            self.t += 22 + 54;
            self.fetch_stream(vi);
            return;
        }
        self.t += 37;
        let mut p = self.v[vi].list_pos;
        let mut e = self.b(p as i64);
        p += 1;
        if e == 0xFF {
            self.t += 4 + 80;
            self.tn = Some((self.t, self.stamp()));
            self.songend = true;
            self.t += 64;
            let off = self.b(p as i64) as usize;
            p = self.v[vi].list_start + off;
            e = self.b(p as i64);
            p += 1;
            self.t += 42;
        }
        self.t += 17;
        if e >= 0x80 {
            self.t += 28;
            self.v[vi].transpose = e & 0x7F;
            e = self.b(p as i64);
            p += 1;
        }
        self.t += 17;
        if e >= 0x70 {
            self.t += 28;
            self.v[vi].bank = e & 15;
            e = self.b(p as i64);
            p += 1;
        }
        self.t += 33;
        self.v[vi].repeat_left = 0;
        if e >= 0x60 {
            self.t += 28;
            self.v[vi].repeat_left = e & 15;
            e = self.b(p as i64);
            p += 1;
        }
        self.t += 36;
        let v = &mut self.v[vi];
        v.pattern = e as i8;
        v.list_pos = p;
        self.t += 54;
        self.fetch_stream(vi);
    }

    /// The stream of the current pattern; outside the stream table the voice ends.
    fn fetch_stream(&mut self, vi: usize) {
        let pat = self.v[vi].pattern;
        let mut pos = usize::MAX;
        if pat >= 0 && (pat as usize) < self.lay.nstreams {
            let stored = be32(&self.d, self.lay.streams + 4 * pat as usize);
            if let Some(o) = self.ptr(stored) {
                pos = o;
            }
        }
        self.v[vi].stream_pos = pos;
    }

    // --- effects (spec 8, 11.7)

    fn effects(&mut self, vi: usize, hw: &mut Hw) {
        self.t += 27;
        if self.v[vi].phase == 1 {
            self.t += 43;
            let b1 = self.inst_b(vi, 1);
            if b1 != 0 {
                self.t += 152;
                let th = (self.v[vi].row_len.wrapping_sub(self.inst_b(vi, 2)).wrapping_add(1)) as i8 as i16 as u16;
                let v = &mut self.v[vi];
                v.vib_threshold = th;
                v.vib_shift = b1 >> 4;
                v.vib_count = ((b1 & 15) + 1) >> 1;
                v.vib_dir = 0;
            } else {
                self.t += 16;
                self.v[vi].vib_threshold = 0xFFFF;
            }
            self.t += 49;
            self.v[vi].flags = self.inst_b(vi, 3);
            if self.v[vi].flags & 0x40 != 0 {
                self.t += 54;
                self.v[vi].arp_pos = self.arp_start(vi);
            }
        }
        self.t += 37;
        let rem = self.v[vi].row_count as u16;
        self.t += 21;
        if rem as i16 <= self.v[vi].slide_trigger as i16 {
            self.slide(vi);
        } else {
            self.t += 21;
            if rem.wrapping_sub(self.v[vi].vib_threshold) & 0x8000 != 0 {
                self.vibrato(vi);
            }
        }
        self.t += 25;
        if self.v[vi].flags & 0x40 != 0 {
            self.t += 41;
            let mut pos = self.v[vi].arp_pos;
            let mut x = self.b(pos as i64);
            pos += 1;
            if x == 0xFF {
                self.t += 70;
                pos = self.arp_start(vi) + self.b(pos as i64) as usize;
                x = self.b(pos as i64);
                pos += 1;
            }
            self.t += 17;
            if x == 0xFE {
                self.t += 30;
                self.v[vi].flags &= !0x40;
            } else {
                self.t += 82;
                let idx = x.wrapping_add(self.v[vi].note) as i8 as i32;
                self.v[vi].period = (self.pt(idx) as u32) << 16;
                self.v[vi].arp_pos = pos;
            }
        }
        self.t += 25;
        let rate = self.inst_b(vi, 5) as u16 + 1;
        if self.v[vi].attack {
            if self.lay.b {
                self.t += 16;
                self.at(hw);
                hw.bus.write_word(0xDFF182, 0x0FF0);
            }
            self.t += 18 + 67;
            let c = self.v[vi].env_counter;
            self.v[vi].env_counter = c.wrapping_sub(rate);
            if c < rate {
                self.t += 16 + 9 + 36;
                let v = &mut self.v[vi];
                v.attack = false;
                v.env_vol = v.base_vol;
            } else {
                let v = &mut self.v[vi];
                v.env_vol = env_f(0x200 - v.env_counter as u32, v.base_vol);
                self.t += 12 + 16 + 21 + 18 + 110 + 16 + 10 + 16;
                return;
            }
        }
        self.t += 21;
        let rem = self.v[vi].row_count as u16;
        if rem.wrapping_sub(self.v[vi].decay_limit) & 0x8000 != 0 {
            self.t += 18 + 67;
            let c = self.v[vi].env_counter;
            self.v[vi].env_counter = c.wrapping_sub(rate);
            if c < rate {
                self.t += 16 + 9 + 32;
                let v = &mut self.v[vi];
                v.decay_limit = 0xFFFF;
                v.env_vol = 0;
            } else {
                let v = &mut self.v[vi];
                v.env_vol = env_f(v.env_counter as u32, v.base_vol);
                self.t += 12 + 16 + 9 + 18 + 110 + 16 + 10;
            }
        }
        self.t += 16;
    }

    fn arp_start(&self, vi: usize) -> usize {
        let stored = be32(&self.d, self.lay.arp + 4 * (self.v[vi].arp_sel & 31));
        self.ptr(stored).unwrap_or(self.n)
    }

    fn slide(&mut self, vi: usize) {
        self.t += 212 + 9;
        let (note, target) = (self.v[vi].note as i8 as i32, self.v[vi].slide_target as i8 as i32);
        let tt = self.pt(target);
        let d = (self.pt(note).wrapping_sub(tt) as i16 as i32).wrapping_mul(self.v[vi].slide_mul as i32).wrapping_mul(1024);
        let old = self.v[vi].period;
        let new = old.wrapping_sub(d as u32);
        self.v[vi].period = new;
        let hi = (new >> 16) as u16 as i16;
        let arrived;
        if d as u32 > old {
            self.t += 35;
            arrived = hi >= tt as i16;
        } else {
            self.t += 35;
            arrived = hi <= tt as i16;
            if arrived {
                self.t += 10;
            }
        }
        if arrived {
            self.t += 138;
            let v = &mut self.v[vi];
            v.period = (tt as u32) << 16;
            v.note = v.slide_target;
            v.slide_trigger = 0xFFFF;
            v.vib_count = ((v.vib_shift & 15) + 1) >> 1;
            v.vib_dir = 0;
        }
        self.t += 10;
    }

    fn vibrato(&mut self, vi: usize) {
        self.t += 25;
        let b1 = self.inst_b(vi, 1);
        let v = &mut self.v[vi];
        v.vib_count = v.vib_count.wrapping_sub(1);
        if v.vib_count & 0x80 != 0 {
            self.t += 70;
            v.vib_dir = !v.vib_dir;
            v.vib_count = b1 & 15;
        }
        self.t += 114;
        let note = self.v[vi].note as i8 as i32;
        let a = self.pt(note).wrapping_sub(self.pt(note + 1)).wrapping_mul(2);
        let v = &mut self.v[vi];
        let mut s = ((a as u32) << 16) as i32 >> (v.vib_shift & 31);
        if v.vib_dir & 0x80 != 0 {
            s = s.wrapping_neg();
            self.t += 15;
        } else {
            self.t += 9;
        }
        v.period = v.period.wrapping_add(s as u32);
        self.t += 24;
    }
}


/// The envelope volume of a counter value (spec 8.5).
fn env_f(c: u32, base_vol: u16) -> u16 {
    ((c.wrapping_mul(base_vol as u32) >> 8) & 0xFFFF) as u16 >> 1
}

impl Replayer for ArtAndMagic {
    fn format(&self) -> &'static str {
        "Art And Magic"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }
}

#[cfg(test)]
#[path = "artandmagic_tests.rs"]
mod tests;
