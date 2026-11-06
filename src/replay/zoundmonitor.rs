//! ZoundMonitor (`sng.*`; AJ of Activas, 1988; the replayer of the demo "The Musical FruitBasket").
//!
//! Clean-room, written from a behavioural specification and black-box runs of the 68k player's
//! register writes; no original code, only three bytes of player state constants embedded (see
//! `STATE_CONSTANTS`; the 37-value period table is the specification's). A song file holds the
//! song, the position table, the single-voice patterns and up to 15 sample descriptions; the sample data are separate files in a `Samples` directory
//! beside the song. Four voices, 50 Hz, a row every `speed` ticks: note starts, instrument loads,
//! arpeggio, slide and portamento; the sample started on a row tick is switched on by the next
//! tick. Every register write is placed at the cycle of the specification's cost model (the tick's
//! entry, the per-step costs, the wait for nine raster-line changes of an effect tick).
//!
//! Decisions where the specification leaves the choice: a missing sample file makes the module not
//! load; an instrument number past slot 16 loads an all-zero slot; a note index past the state
//! words of the period table reads 0; a pattern or position past the song data reads zeros past
//! the end of the file; host volume and balance are not applied (the defaults).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "ZoundMonitor", detect, new };

/// Three bytes of the original player's state that a note index past the period table reads:
/// byte 0 is the high byte of the word at index 37 (its low byte is the song speed), bytes 1 and 2
/// are the whole word at index 125. Taken from the original player on the project owner's
/// decision (a small player table, embedded like the other such tables).
const STATE_CONSTANTS: &[u8; 3] = include_bytes!("zoundmonitor_state_constants.bin");

/// Period table, indices 0 to 36.
const PERIODS: [u16; 37] = [
    0, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254,
    240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// A raster line is 227 colour clocks.
const LINE: u64 = 227;
/// Cycle count at the tick's entry.
const T0: u64 = 309;
/// Helper-block costs by voice.
const CH: [u64; 4] = [0, 31, 62, 70];
const VOL_AT: [u64; 4] = [155, 172, 179, 206];

const SLOTS: usize = 5;
const SLOT_LEN: usize = 54;
const POS_AT: usize = 869;

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn rd16(d: &[u8], o: usize) -> u16 {
    (rb(d, o) as u16) << 8 | rb(d, o + 1) as u16
}

fn rd32(d: &[u8], o: usize) -> u32 {
    (rd16(d, o) as u32) << 16 | rd16(d, o + 2) as u32
}

/// The end of the pattern area.
fn end_of_patterns(d: &[u8]) -> Option<usize> {
    let t = *d.first()? as usize;
    let p = *d.get(1)? as usize;
    Some(POS_AT + 16 * (t + 1) + 128 * (p + 1))
}

fn detect(d: &[u8]) -> bool {
    let e = match end_of_patterns(d) {
        Some(e) => e,
        None => return false,
    };
    if e >= d.len() {
        return false;
    }
    let b = |i: usize| d.get(e + i).copied();
    if b(0) == Some(b'd') {
        b(1) == Some(b'f') && b(3) == Some(b':')
    } else {
        b(1) == Some(b'a') && b(2) == Some(b'm') && b(3) == Some(b'p')
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    vol: u16,
    delta: i16,
    per: u16,
    bit: u16,
    len: u16,
    rlen: u16,
    start: u32,
    rstart: u32,
    note: u8,
    flags: u8,
    arp: [u8; 3],
    ivol: i16,
    pend: bool,
    cnt: u16,
    pdelta: i16,
    pdur: u16,
    last: u8,
    cur: u8,
}

impl Voice {
    /// The 20-word memory image read by out-of-range note indices.
    fn image(&self, k: usize) -> u16 {
        match k {
            0 => self.vol,
            1 => self.delta as u16,
            2 => self.per,
            3 => self.bit,
            4 => self.len,
            5 => self.rlen,
            6 => (self.start >> 16) as u16,
            7 => self.start as u16,
            8 => (self.rstart >> 16) as u16,
            9 => self.rstart as u16,
            10 => (self.note as u16) << 8 | self.flags as u16,
            11 => self.arp[1] as u16,
            12 => (self.arp[2] as u16) << 8,
            13 => self.ivol as u16,
            14 => self.pend as u16,
            15 => self.cnt,
            16 => self.pdelta as u16,
            17 => self.pdur,
            18 => (self.last as u16) << 8 | self.cur as u16,
            _ => 0,
        }
    }
}

struct Zm {
    d: Vec<u8>,
    t_max: u8,
    p_max: u8,
    start: u8,
    end: u8,
    speed: u8,
    /// Memory address and length (words) of each of the 16 slots' sample, as patched on load.
    addr: [u32; 16],
    len: [u16; 16],
    pos: u8,
    row: u8,
    tick: u8,
    mask: u16,
    v: [Voice; 4],
    songend: bool,
    // the clock of the tick being played
    base_cc: u64,
    t: u64,
}

fn slot(i: usize) -> usize {
    SLOTS + SLOT_LEN * i
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let mut z = Zm {
        d: d.to_vec(),
        t_max: d[0],
        p_max: d[1],
        start: d[2],
        end: d[3],
        speed: d[4],
        addr: [0; 16],
        len: [0; 16],
        pos: d[2],
        row: 0,
        tick: d[4].wrapping_sub(1),
        mask: 0,
        v: [Voice::default(); 4],
        songend: false,
        base_cc: 0,
        t: 0,
    };
    // the sample slots, then the sample files in slot order
    for i in 0..16 {
        let s = slot(i);
        z.addr[i] = rd32(d, s);
        z.len[i] = rd16(d, s + 46);
        let name: Vec<u8> = d.get(s + 4..s + 44)?.iter().copied().take_while(|&c| c != 0).collect();
        if name.is_empty() {
            continue;
        }
        let name: String = name.iter().map(|&c| c as char).collect();
        let file = crate::deli::read_companion(m, &format!("Samples/{name}")).1.ok()?;
        let (data, len) = if file.starts_with(b"FORM") {
            let n = (rd32(&file, 100) / 2) as u16;
            (file.get(104..).unwrap_or(&[]).to_vec(), Some(n))
        } else {
            (file, None)
        };
        // each file is followed by 16 spare bytes
        let a = m.alloc(data.len() as u32 + 16);
        if a == 0 {
            return None;
        }
        m.bus.write_bytes(a, &data);
        // the first word of the data is cleared
        m.bus.write_bytes(a, &[0, 0][..data.len().min(2)]);
        if i < 15 {
            z.addr[i] = a;
            if let Some(n) = len {
                z.len[i] = n;
            }
        }
    }
    for (v, x) in z.v.iter_mut().enumerate() {
        x.bit = 1 << v;
    }
    // The host's own work before the first tick. It depends on the host (directory and file names,
    // sample count), not on the module; the first entry's clock is taken from the reference logs
    // (92 830 for three samples, 94 344 to 94 391 with four; 94 360 is a mid value). It only shifts
    // the raster-line phase of the effect-tick wait, so a different value moves those writes by a
    // few colour clocks and changes no value or order.
    let samples = (0..15).filter(|&i| d.get(slot(i) + 4).copied().unwrap_or(0) != 0).count() as u64;
    let first = if samples <= 3 { 92830 } else { 94360 };
    Hw { bus: &mut m.bus }.wait(first - 100 - 70940);
    Some(Box::new(z))
}

impl Zm {
    fn stamp(&self, hw: &mut Hw) {
        hw.bus.clock = self.base_cc + self.t / 2;
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

    fn w_vol(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.vol(ch, n & 0x7F);
    }

    fn w_dma(&self, hw: &mut Hw, n: u16) {
        self.stamp(hw);
        hw.dmacon(n);
    }

    /// The period table read with an 8-bit index; past the table, the player's state words.
    fn ptab(&self, i: u8) -> u16 {
        let i = i as usize;
        match i {
            0..=36 => PERIODS[i],
            37 => (STATE_CONSTANTS[0] as u16) << 8 | self.speed as u16,
            38 => 15,
            39 => self.p_max as u16,
            40 => (self.t_max as u16) << 8 | self.tick as u16,
            41 => (self.row as u16) << 8 | self.pos as u16,
            42 | 43 => (self.start as u16) << 8 | self.end as u16,
            44 => self.mask,
            45..=124 => self.v[(i - 45) / 20].image((i - 45) % 20),
            125 => u16::from_be_bytes([STATE_CONSTANTS[1], STATE_CONSTANTS[2]]),
            _ => 0,
        }
    }

    fn periodh(c: usize) -> u64 {
        79 + CH[c]
    }

    /// EFFECT(v): the per-voice effect step.
    fn effect(&mut self, hw: &mut Hw, c: usize) {
        self.t += 25;
        let f = self.v[c].flags;
        if f & 1 != 0 {
            self.t += 25;
            if f & 2 == 0 {
                // arpeggio
                self.t += 43;
                let v = &mut self.v[c];
                v.cnt = v.cnt.wrapping_add(1);
                if v.cnt % 2 == 0 {
                    self.t += 204;
                    let v = self.v[c];
                    let idx = (v.cnt / 2 % 3) as usize;
                    let per = self.ptab(v.note.wrapping_add(v.arp[idx]));
                    self.t += 22;
                    self.w_per(hw, c, per);
                    self.t += 44 + Self::periodh(c) + 22;
                }
            } else {
                // portamento
                self.t += 33;
                let v = self.v[c];
                if v.cnt != v.pdur {
                    self.v[c].cnt = v.cnt.wrapping_add(1);
                    let cnt = v.cnt.wrapping_add(1);
                    self.t += 244;
                    let prod = (v.pdelta as i32).wrapping_mul(cnt as i16 as i32);
                    let q = match v.pdur as i16 as i32 {
                        0 => 0,
                        d => prod.wrapping_div(d),
                    };
                    let per = v.per.wrapping_add(q as u16);
                    self.t += 12;
                    self.w_per(hw, c, per);
                    self.t += 34 + Self::periodh(c) + 22;
                }
            }
        } else {
            self.t += 25;
            if f & 2 != 0 {
                // slide
                self.t += 36;
                let v = &mut self.v[c];
                v.per = v.per.wrapping_add(v.delta as u16);
                let per = v.per;
                self.t += 12;
                self.w_per(hw, c, per);
                self.t += 34 + Self::periodh(c) + 12;
            }
        }
        // always: the repeat part
        let v = self.v[c];
        self.t += 24;
        self.w_loc(hw, c, v.rstart);
        self.t += 20;
        self.w_len(hw, c, v.rlen);
        self.t += 16;
    }

    fn pattern_word(&self, pat: u8, row: usize) -> (u32, u32) {
        let base = POS_AT + 16 * (self.t_max as usize + 1);
        let o = base + 128 * pat as usize + 4 * row;
        let r = rd32(&self.d, o);
        let r2 = if row == 31 && pat == self.p_max {
            // the song data moved five bytes towards the file start: the word is the last bytes
            let e = base + 128 * (self.p_max as usize + 1);
            rd32(&self.d, e - 5)
        } else {
            rd32(&self.d, o + 4)
        };
        (r, r2)
    }

    fn row_tick(&mut self, hw: &mut Hw) {
        self.tick = 0;
        self.t += 102;
        let pos = self.pos as usize;
        for c in 0..4 {
            self.t += 39 + 25;
            if self.v[c].pend {
                self.t += 38;
                self.v[c].pend = false;
                self.effect(hw, c);
                self.t += 10 + 56;
                continue;
            }
            let e = POS_AT + 16 * pos + 4 * c;
            let pat = rb(&self.d, e);
            let voff = rb(&self.d, e + 1) as i8 as i32;
            let ioff = rb(&self.d, e + 2);
            let tr = rb(&self.d, e + 3);
            self.t += 167;
            let (r, r2) = self.pattern_word(pat, self.row as usize);
            let mut note = ((r >> 24) & 0x3F) as u8;
            if note == 0 {
                self.t += 18;
                self.effect(hw, c);
                self.t += 10 + 56;
                continue;
            }
            // the note start
            let flags = ((r >> 16) & 15) as u8;
            self.t += 89;
            if flags & 4 == 0 {
                self.t += 28;
                note = note.wrapping_add(tr);
            }
            self.v[c].flags = flags;
            self.v[c].note = note;
            self.t += 54 + 25;
            let per = self.ptab(note);
            self.v[c].per = per;
            let data = (r & 0xFF) as u8;
            if flags & 1 != 0 {
                self.t += 25;
                if flags & 2 == 0 {
                    self.t += 80;
                    let v = &mut self.v[c];
                    v.arp = [0, (r >> 4) as u8 & 15, r as u8 & 15];
                    v.cnt = 0;
                } else {
                    self.t += 63;
                    let mut target = ((r2 >> 24) & 0x3F) as u8;
                    if r2 & 0x40000 == 0 {
                        self.t += 12;
                        target = target.wrapping_add(tr);
                    }
                    self.t += 182;
                    let tp = self.ptab(target);
                    let speed = self.speed;
                    let v = &mut self.v[c];
                    v.pend = true;
                    v.pdelta = tp.wrapping_sub(per) as i16;
                    v.cnt = 0;
                    v.pdur = (data as u16).wrapping_mul(speed as u16);
                }
            } else {
                self.t += 25;
                if flags & 2 != 0 {
                    self.t += 28;
                    self.v[c].delta = data as i8 as i16;
                }
            }
            // the instrument
            self.t += 53;
            let field = ((r >> 20) & 15) as u8;
            let mut ins = field;
            if field != 0 {
                self.t += 25;
                if flags & 8 == 0 {
                    self.t += 12;
                    ins = ins.wrapping_add(ioff);
                }
                self.t += 9;
                if ins != 0 {
                    self.t += 21;
                }
            }
            let load = ins != 0 && ins != self.v[c].cur;
            if load {
                self.t += 130;
                self.v[c].cur = ins;
                // instrument numbers past slot 16 read an all-zero slot
                let n = ins as usize - 1;
                self.v[c].ivol = if n < 16 { rb(&self.d, slot(n) + 44) as i8 as i16 } else { 0 };
            }
            self.t += 18;
            // the volume
            self.t += 89;
            let vol = self.v[c].ivol as i32 + (r >> 8) as u8 as i8 as i32 + voff;
            let vol = if (0..64).contains(&vol) {
                self.t += 24;
                vol
            } else if vol < 0 {
                self.t += 13 + 4 + 24;
                0
            } else {
                self.t += 13 + 18 + 24;
                64
            };
            self.v[c].vol = vol as u16;
            if load {
                let n = ins as usize - 1;
                let (addr, len, rl, rs) = if n < 16 {
                    (self.addr[n], self.len[n], rd16(&self.d, slot(n) + 48), rd16(&self.d, slot(n) + 50))
                } else {
                    (0, 0, 0, 0)
                };
                let v = &mut self.v[c];
                v.start = addr;
                v.len = len;
                v.rlen = rl;
                v.rstart = addr.wrapping_add(2 * rs as u32);
                self.t += 126;
            } else {
                self.t += 10;
            }
            // restart or not
            self.t += 19;
            let mut restart = true;
            if r & 0x8000_0000 != 0 {
                self.t += 33;
                if self.v[c].cur != self.v[c].last {
                    self.t += 12;
                    self.v[c].last = self.v[c].cur;
                } else {
                    restart = false;
                }
            }
            if restart {
                let bit = self.v[c].bit;
                self.t += 24;
                self.w_dma(hw, bit);
                self.t += 25;
                if note == 63 {
                    self.t += 56;
                    continue;
                }
                self.mask |= bit;
                self.t += 28;
                let v = self.v[c];
                self.t += 24;
                self.w_loc(hw, c, v.start);
                self.t += 20;
                self.w_len(hw, c, v.len);
                self.t += 18 + 115 + CH[c];
            }
            // period and volume
            let v = self.v[c];
            self.t += 20;
            self.w_per(hw, c, v.per);
            self.t += 42 + Self::periodh(c) + 8 + 18;
            self.t += VOL_AT[c];
            self.w_vol(hw, c, v.vol);
            self.t += 28;
            self.t += 18 + 79 + CH[c] + 12;
            self.t += 56;
        }
        self.row += 1;
        if self.row == 32 {
            self.row = 0;
            self.pos = self.pos.wrapping_add(1);
            if self.pos == self.end {
                self.pos = self.start;
                self.songend = true;
            }
        }
    }

    /// The wait for nine raster-line changes.
    fn raster_wait(&mut self) {
        let line = |s: &Zm| (s.base_cc + s.t / 2) / LINE;
        for _ in 0..9 {
            self.t += 16;
            let l = line(self);
            self.t += 16;
            while line(self) == l {
                self.t += 25;
            }
            self.t += 19;
        }
    }

    fn effect_tick(&mut self, hw: &mut Hw) {
        self.t += 21;
        if self.mask != 0 {
            self.t += 36;
            let m = self.mask | 0x8200;
            self.w_dma(hw, m);
            self.mask = 0;
            self.t += 16;
        }
        self.t += 30 + 28;
        self.raster_wait();
        self.t += 44 + 16;
        for c in 0..4 {
            self.t += 25 + 30;
            self.effect(hw, c);
            self.t += 48;
        }
    }
}

impl Replayer for Zm {
    fn format(&self) -> &'static str {
        "ZoundMonitor"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.base_cc = hw.bus.clock;
        self.t = T0;
        self.tick = self.tick.wrapping_add(1);
        if self.tick == self.speed {
            self.row_tick(hw);
        } else {
            self.effect_tick(hw);
        }
    }
}

#[cfg(test)]
#[path = "zoundmonitor_tests.rs"]
mod tests;
