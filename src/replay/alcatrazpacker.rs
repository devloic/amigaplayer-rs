//! Alcatraz Packer (`alp.*`, samples in a companion `smp.*`), the music of the games
//! Cedric (CD32, Alcatraz/NEO, 1995, music by Rainer Nilsen) and Oldtimer.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Alcatraz Packer player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A NoisePacker-like four-voice format with its own engine: a position list of
//! patterns of four track streams (rests and repeated events are commands in the
//! stream), 50 Hz, speed in ticks per row. Every row writes every channel's period
//! and volume, a note start writes the sample before the DMA is switched off and
//! on again at the end of the row, and every tick rewrites the loop registers.
//! The channels are visited from the last to the first. One subsong. The samples
//! are in the companion file `smp.NAME` of the song file `alp.NAME` unless they
//! follow the song data in the same file.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Alcatraz Packer", detect, new };

/// ProTracker's periods for the 16 finetunes (0..7, then -8..-1), 36 notes each.
const FINETUNE: [[u16; 36]; 16] = [
    [
        856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
        226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
    ],
    [
        850, 802, 757, 715, 674, 637, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318, 300, 284, 268, 253, 239,
        225, 213, 201, 189, 179, 169, 159, 150, 142, 134, 126, 119, 113,
    ],
    [
        844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316, 298, 282, 266, 251, 237,
        224, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 112,
    ],
    [
        838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 280, 264, 249, 235,
        222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118, 111,
    ],
    [
        832, 785, 741, 699, 660, 623, 588, 555, 524, 495, 467, 441, 416, 392, 370, 350, 330, 312, 294, 278, 262, 247, 233,
        220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 124, 117, 110,
    ],
    [
        826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 276, 260, 245, 232,
        219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116, 109,
    ],
    [
        820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307, 290, 274, 258, 244, 230,
        217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115, 109,
    ],
    [
        814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305, 288, 272, 258, 242, 228,
        216, 204, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114, 108,
    ],
    [
        907, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254,
        240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120,
    ],
    [
        900, 850, 802, 757, 715, 675, 636, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318, 300, 284, 268, 253,
        238, 225, 212, 200, 189, 179, 169, 159, 150, 142, 134, 126, 119,
    ],
    [
        894, 844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316, 298, 282, 266, 251,
        237, 223, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118,
    ],
    [
        887, 838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314, 296, 280, 264, 249,
        235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118,
    ],
    [
        881, 832, 785, 741, 699, 660, 623, 588, 555, 524, 494, 467, 441, 416, 392, 370, 350, 330, 312, 294, 278, 262, 247,
        233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 123, 117,
    ],
    [
        875, 826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309, 292, 276, 260, 245,
        232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116,
    ],
    [
        868, 820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307, 290, 274, 258, 244,
        230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115,
    ],
    [
        862, 814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305, 288, 272, 258, 242,
        228, 216, 204, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114,
    ],
];

/// The period of note `n` (1..36) at finetune row `ft`; what the original reads beyond
/// its table is not defined, and 0 here.
fn period(ft: usize, n: usize) -> i32 {
    if n == 0 || n > 36 {
        return 0;
    }
    FINETUNE.get(ft).map_or(0, |r| r[n - 1] as i32)
}

const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197,
    180, 161, 141, 120, 97, 74, 49, 24,
];

/// Colour clocks the original lets pass between its DMA-off, DMA-on and loop passes
/// (nine raster lines).
const DMA_WAIT: u64 = 2000;

// ---------------------------------------------------------------------------------
// Recognition

/// The sizes of the parts of a song file: `(W0, W1, W2, W3)`.
fn parts(d: &[u8]) -> Option<[usize; 4]> {
    if d.len() < 16 || d[..4] != [0x50, 0x41, 0x6E, 0x10] {
        return None;
    }
    let total = be32(d, 4);
    if total == 0 || total >= 0x8000_0000 {
        return None;
    }
    let w = [be16(d, 8) as usize, be16(d, 10) as usize, be16(d, 12) as usize, be16(d, 14) as usize];
    if w[0] < 12 || 8 + w[0] + w[1] + w[2] + w[3] > d.len() {
        return None;
    }
    Some(w)
}

fn detect(d: &[u8]) -> bool {
    parts(d).is_some()
}

// ---------------------------------------------------------------------------------
// The player

#[derive(Clone, Default)]
struct Rec {
    /// Note byte (2 * note) of the last note started.
    n: u8,
    sm: u8,
    e: u8,
    q: u8,
    v: u16,
    vb: u8,
    vp: u8,
    tg: i32,
    per: i32,
    ps: u8,
    pd: bool,
    so: u8,
    tp: usize,
    k: u8,
    kf: u8,
    la: u32,
    ll: u16,
}

struct Alp {
    d: Vec<u8>,
    title: String,
    /// Offsets of the position list, the pattern table and the track data.
    poslist: usize,
    pattab: usize,
    tracks: usize,
    end: u16,
    restart: u16,
    /// Absolute start address of each sample table entry (index 1..).
    starts: Vec<u32>,
    c: u8,
    speed: u8,
    r: u8,
    f: u8,
    p: u16,
    dmask: u8,
    rec: [Rec; 4],
    ft: [usize; 4],
    songend: bool,
}

fn sx8(b: u8) -> i32 {
    b as i8 as i32
}

impl Alp {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn w(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// Loads the patterns of the position `p`.
    fn load_position(&mut self) {
        self.r = 0x40;
        if self.p == self.end {
            self.p = self.restart;
            self.songend = true;
        }
        let x = self.w(self.poslist + self.p as usize) as i16 as isize;
        let pat = self.pattab.wrapping_add_signed(x);
        for r in 0..4 {
            let t = self.w(pat.wrapping_add(2 * r)) as i16 as isize;
            self.rec[r].tp = self.tracks.wrapping_add_signed(t);
            self.rec[r].k = 0;
        }
    }

    fn entry_start(&self, n: usize) -> u32 {
        self.starts.get(n).copied().unwrap_or(0)
    }

    fn vol_add(v: u16, d: i32) -> u16 {
        let b = ((v as i32 + d) & 0xFF) as u16;
        if b >= 0x80 {
            0
        } else {
            b.min(0x40)
        }
    }

    /// The row routine of record `r`.
    fn row(&mut self, r: usize, d5: &mut u8, hw: &mut Hw) {
        let ch = 3 - r;
        let mut a1 = self.rec[r].tp;
        let mut skip = false;
        if self.rec[r].k != 0 {
            self.rec[r].k -= 1;
            if self.rec[r].kf != 0 {
                skip = true;
            }
        }
        while !skip {
            let b = self.rb(a1);
            a1 += 1;
            if b >= 0x80 {
                let nb = !b;
                let rec = &mut self.rec[r];
                rec.kf = if nb & 0x40 != 0 { 0xFF } else { 0 };
                rec.k = nb & 0x3F;
                rec.tp = a1;
                rec.e = 0;
                if rec.kf != 0 {
                    skip = true;
                }
                continue;
            }
            // an event
            let b2 = self.rb(a1);
            a1 += 1;
            let sp = (b2 >> 4) as usize + if b & 1 != 0 { 16 } else { 0 };
            if sp != 0 {
                self.rec[r].sm = sp as u8;
                self.rec[r].v = self.rb(16 * sp + 7) as u16;
            }
            let entry = 16 * self.rec[r].sm as usize;
            self.rec[r].e = b2 & 15;
            if self.rec[r].e != 0 {
                self.rec[r].q = self.rb(a1);
                a1 += 1;
            }
            let note = b & 0xFE;
            let e = self.rec[r].e;
            if note != 0 {
                let ft = self.rb(entry + 6) as usize;
                self.ft[ch] = ft * 72;
                let per = period(ft, note as usize / 2);
                if e == 3 || e == 5 {
                    let rec = &mut self.rec[r];
                    rec.tg = if per == rec.per { 0 } else { per };
                    rec.pd = per < rec.per;
                } else {
                    let start = self.entry_start(self.rec[r].sm as usize);
                    let (len, la_off, ll) = (self.w(entry + 4), self.w(entry + 14) as u32, self.w(entry + 12));
                    let rec = &mut self.rec[r];
                    rec.n = note;
                    rec.per = per;
                    rec.vp = 0;
                    *d5 |= 1 << ch;
                    hw.loc(ch, start);
                    hw.len(ch, len);
                    rec.la = start.wrapping_add(2 * la_off);
                    rec.ll = ll;
                }
            }
            if e >= 9 {
                self.tick0_effect(r, entry, hw);
            }
            break;
        }
        if self.rec[r].k == 0 {
            self.rec[r].tp = a1;
        }
        hw.per(ch, self.rec[r].per as u16);
        hw.vol(ch, self.rec[r].v);
    }

    /// Effects 9..F on the first tick of a row.
    fn tick0_effect(&mut self, r: usize, entry: usize, hw: &mut Hw) {
        let ch = 3 - r;
        let q = self.rec[r].q;
        match self.rec[r].e {
            0xA => {
                if q != 0 {
                    self.rec[r].so = q;
                }
                let start = self.entry_start(self.rec[r].sm as usize);
                let len = self.w(entry + 4) as i16;
                let off = (self.rec[r].so as u16) << 7;
                let mut l = len.wrapping_sub(off as i16);
                let mut s = start;
                if l <= 0 {
                    l = 1;
                }
                s = s.wrapping_add(off.wrapping_mul(2) as i16 as i32 as u32);
                hw.len(ch, l as u16);
                hw.loc(ch, s);
            }
            0xB => {
                self.p = q as i8 as i16 as u16;
                self.r |= 0x80;
            }
            0xC => self.rec[r].v = q as u16,
            0xD => self.r |= 0x80,
            0xE => {
                let (hi, x) = (q >> 4, (q & 15) as i32);
                let rec = &mut self.rec[r];
                match hi {
                    1 => {
                        rec.per = (rec.per + x).clamp(113, 856);
                        hw.per(ch, rec.per as u16);
                    }
                    2 => {
                        rec.per = (rec.per - x).clamp(113, 856);
                        hw.per(ch, rec.per as u16);
                    }
                    3 => rec.v = Self::vol_add(rec.v, -x),
                    4 => rec.v = Self::vol_add(rec.v, x),
                    _ => {}
                }
            }
            0xF => self.speed = q,
            _ => {}
        }
    }

    fn vibrato(rec: &mut Rec, ch: usize, hw: &mut Hw) {
        let idx = ((rec.vp >> 2) & 31) as usize;
        let mut dl = (SINE[idx] as i32 * (rec.vb & 15) as i32) >> 7;
        if rec.vp & 0x80 != 0 {
            dl = -dl;
        }
        hw.per(ch, (rec.per + dl) as u16);
        rec.vp = rec.vp.wrapping_add((rec.vb >> 2) & 0x3C);
    }

    fn porta(rec: &mut Rec, ch: usize, hw: &mut Hw) {
        if rec.tg == 0 {
            return;
        }
        if !rec.pd {
            rec.per += rec.ps as i32;
            if rec.tg <= rec.per {
                rec.per = rec.tg;
                rec.tg = 0;
            }
        } else {
            rec.per -= rec.ps as i32;
            if rec.tg >= rec.per {
                rec.per = rec.tg;
                rec.tg = 0;
            }
        }
        hw.per(ch, rec.per as u16);
    }

    /// The per-tick effect of record `r` on a tick that starts no row.
    fn effect(&mut self, r: usize, hw: &mut Hw) {
        let ch = 3 - r;
        let (cc, speed) = (self.c, self.speed);
        let ft = self.ft[ch];
        let rec = &mut self.rec[r];
        let q = rec.q;
        match rec.e {
            1 => {
                let mut dd = cc.wrapping_sub(speed) as i8;
                while dd < 0 {
                    dd += 3;
                }
                match dd {
                    0 => hw.per(ch, rec.per as u16),
                    d => {
                        let nib = if d == 1 { q & 15 } else { q >> 4 } as usize;
                        let i = (rec.n as usize + 2 * nib).min(72);
                        hw.per(ch, period(ft / 72, i / 2) as u16);
                    }
                }
            }
            2 => {
                rec.per = (rec.per - sx8(q)).clamp(113, 856);
                hw.per(ch, rec.per as u16);
            }
            3 | 5 => {
                if q != 0 && rec.e == 3 {
                    rec.ps = q;
                }
                Self::porta(rec, ch, hw);
                if rec.e == 5 {
                    rec.v = Self::vol_add(rec.v, sx8(q));
                }
            }
            4 | 6 => {
                if q != 0 && rec.e == 4 {
                    rec.vb = q;
                }
                Self::vibrato(rec, ch, hw);
                if rec.e == 6 {
                    rec.v = Self::vol_add(rec.v, sx8(q));
                }
            }
            8 => rec.v = Self::vol_add(rec.v, sx8(q)),
            _ => {}
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.f != 0 {
            self.f = 0;
            self.r = self.r.wrapping_sub(1);
            if (self.r as i8) <= 0 {
                self.p = self.p.wrapping_add(2);
                self.load_position();
            }
        }
        self.c = self.c.wrapping_sub(1);
        if (self.c as i8) > 0 {
            for r in 0..4 {
                if (1..=8).contains(&self.rec[r].e) {
                    self.effect(r, hw);
                }
                hw.vol(3 - r, self.rec[r].v);
            }
        } else {
            self.f = 0xFF;
            let mut d5 = 0u8;
            for r in 0..4 {
                self.row(r, &mut d5, hw);
            }
            self.c = self.speed;
            self.dmask = d5;
            if d5 != 0 {
                hw.dmacon(d5 as u16);
            }
        }
        hw.wait(DMA_WAIT);
        hw.dmacon(0x8200 | self.dmask as u16);
        hw.wait(DMA_WAIT);
        for r in 0..4 {
            hw.loc(3 - r, self.rec[r].la);
            hw.len(3 - r, self.rec[r].ll);
        }
    }
}

impl Replayer for Alp {
    fn format(&self) -> &'static str {
        "Alcatraz Packer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        Alp::tick(self, hw);
    }
}

/// The song `d` with its samples at `smp` (an absolute address): the player at its start.
fn start(d: &[u8], smp: u32, title: String) -> Option<Alp> {
    let w = parts(d)?;
    let poslist = 8 + w[0];
    let mut starts = vec![0u32];
    let mut a = smp;
    for n in 1..=w[0] >> 4 {
        starts.push(a);
        a = a.wrapping_add(2 * be16(d, 16 * n + 4) as u32);
    }
    let mut p = Alp {
        d: d.to_vec(),
        title,
        poslist,
        pattab: poslist + w[1],
        tracks: poslist + w[1] + w[2],
        end: be16(d, 4 + w[0]),
        restart: be16(d, 6 + w[0]),
        starts,
        c: 0,
        speed: 6,
        r: 0x40,
        f: 0,
        p: 0,
        dmask: 0,
        rec: Default::default(),
        ft: [0; 4],
        songend: false,
    };
    p.load_position();
    // the position the song starts at is not the end of it
    p.songend = false;
    Some(p)
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let w = parts(d)?;
    let total = be32(d, 4) as usize;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let name = file.rsplit('/').next().unwrap_or(&file).to_string();
    let smp = if total > d.len() {
        let rest = match name.get(..4) {
            Some(p) if p.eq_ignore_ascii_case("alp.") => &name[4..],
            _ => return None,
        };
        let data = crate::deli::read_companion(m, &format!("smp.{rest}")).1.ok()?;
        let a = m.alloc((data.len() as u32).max(4));
        if a == 0 {
            return None;
        }
        m.bus.write_bytes(a, &data);
        a
    } else {
        base + (8 + w[0] + w[1] + w[2] + w[3]) as u32
    };
    Some(Box::new(start(d, smp, name)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    /// A song with one sample (8 words, volume 0x30) whose single pattern plays a note
    /// on record 0 (hardware channel 3), rests for the rest of the pattern, and
    /// repeats a note on record 1. The samples follow the song in the file.
    fn module() -> Vec<u8> {
        let w0 = 16 * 2 + 12;
        let w1 = 2usize;
        let w2 = 8usize;
        // record 0: note 12 with sample 1, then 63 empty rows
        let mut tracks = vec![0x18, 0x10, 0x81];
        // record 1: note 16 on two rows (repeat command) with volume 0x10, 62 empty rows
        let t1 = tracks.len();
        tracks.extend_from_slice(&[0xFE, 0x20, 0x1C, 0x10, 0x82]);
        // records 2 and 3: 64 empty rows
        let t2 = tracks.len();
        tracks.extend_from_slice(&[0x80]);
        let w3 = tracks.len();
        let song = 8 + w0 + w1 + w2 + w3;
        let mut d = vec![0u8; song + 16];
        d[..4].copy_from_slice(&[0x50, 0x41, 0x6E, 0x10]);
        d[4..8].copy_from_slice(&((song + 16) as u32).to_be_bytes());
        put16(&mut d, 8, w0 as u16);
        put16(&mut d, 10, w1 as u16);
        put16(&mut d, 12, w2 as u16);
        put16(&mut d, 14, w3 as u16);
        // sample 1: 8 words, finetune 0, volume 0x30, loop length 1, loop start 0
        put16(&mut d, 16 + 4, 8);
        d[16 + 7] = 0x30;
        put16(&mut d, 16 + 12, 1);
        // END and RESTART, position list (one pattern at offset 0), pattern table
        put16(&mut d, 4 + w0, 2);
        put16(&mut d, 6 + w0, 0);
        let pl = 8 + w0;
        put16(&mut d, pl, 0);
        let pt = pl + w1;
        put16(&mut d, pt, 0);
        put16(&mut d, pt + 2, t1 as u16);
        put16(&mut d, pt + 4, t2 as u16);
        put16(&mut d, pt + 6, t2 as u16);
        let tr = pt + w2;
        d[tr..tr + tracks.len()].copy_from_slice(&tracks);
        d
    }

    #[test]
    fn recognises_modules() {
        let d = module();
        assert!(detect(&d));
        let mut e = d.clone();
        e[3] = 0x11;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(!detect(&e));
        let mut e = d.clone();
        e[4] = 0x80;
        assert!(!detect(&e));
        // the parts must fit in the file
        let mut e = d.clone();
        put16(&mut e, 14, 0x4000);
        assert!(!detect(&e));
        assert!(!detect(&[0u8; 0x100]));
    }

    fn run(data: Vec<u8>, ticks: usize) -> (Vec<(u32, u16)>, bool) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/alp-test/alp.test"), data).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Alcatraz Packer");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let end = e.songend();
        (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), end)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn the_first_record_plays_on_the_last_channel() {
        let (log, _) = run(module(), 3);
        // note 12 -> period 453 on AUD3: sample start and length, then period and volume
        assert_eq!(reg(&log, 0xDFF0D4)[0], 8);
        assert_eq!(reg(&log, 0xDFF0D6)[0], 453);
        assert_eq!(reg(&log, 0xDFF0D8)[0], 0x30);
        // the DMA of the two channels that start is switched off, then on again
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..2], [0x000C, 0x820C]);
    }

    #[test]
    fn a_repeated_event_is_played_on_every_row() {
        // record 1 repeats a note on two rows, volume set by effect C on the first
        let (log, _) = run(module(), 14);
        let ch2 = reg(&log, 0xDFF0C6);
        assert!(ch2.iter().filter(|&&p| p == 360).count() >= 2);
        assert_eq!(reg(&log, 0xDFF0C8)[0], 0x10);
    }

    #[test]
    fn loop_registers_are_rewritten_every_tick() {
        let (log, _) = run(module(), 4);
        // four channels x (hi, lo) per tick for LC after the first tick
        assert!(reg(&log, 0xDFF0D0).len() >= 4);
        assert!(reg(&log, 0xDFF0D4).len() >= 4);
    }

    #[test]
    fn song_end_is_announced_at_the_end_position() {
        // one pattern of 64 rows at speed 6; the end is announced a tick after the last row
        let (_, end) = run(module(), 64 * 6 - 10);
        assert!(!end);
        let (_, end) = run(module(), 64 * 6 + 10);
        assert!(end);
    }

    #[test]
    fn volume_add_wraps_like_a_byte() {
        assert_eq!(Alp::vol_add(0x40, 0x7F), 0);
        assert_eq!(Alp::vol_add(0x20, 0x30), 0x40);
        assert_eq!(Alp::vol_add(5, -9), 0);
        assert_eq!(Alp::vol_add(5, 4), 9);
    }
}
