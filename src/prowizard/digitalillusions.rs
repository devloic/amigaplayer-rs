//! Digital Illusions, ported from NostalgicPlayer's DigitalIllusionsFormat.
//!
//! The header is the number of samples and the offsets of the position list
//! (ended by 0xff), the patterns and the sample data, then the sample table and a
//! word offset per pattern. A row of a pattern is 4 packed notes: 0xff for an empty
//! one, else sample and note in two bytes, and the effect parameter in a third
//! when the first has bit 7 set. Parsing a pattern stops after a row with a
//! position jump or pattern break (Thomas Neumann's fix).

use super::core::*;

#[derive(Default)]
pub struct DigitalIllusions {
    base: Base,
    samples: u16,
    position_list_offset: u32,
    sample_data_offset: u32,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(6, Box::new(DigitalIllusions::default()))]
}

impl Format for DigitalIllusions {
    fn name(&self) -> &'static str {
        "Digital Illusions"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 17 {
            return false;
        }
        s.seek(0);
        let count = s.read_b_u16();
        if count > 0x1f {
            return false;
        }
        let o1 = s.read_b_u32();
        let o2 = s.read_b_u32();
        let o3 = s.read_b_u32();
        if (o1 | o2 | o3) & 0xffff0000 != 0 {
            return false;
        }
        if o1 & 0xffff == 0 || o2 & 0xffff == 0 || o3 & 0xffff == 0 {
            return false;
        }
        // the first pattern offset
        s.seek(14 + count as i64 * 8);
        if s.read_b_u16() as u32 != o2 {
            return false;
        }
        s.seek(14);
        let mut samples_size = 0u32;
        for _ in 0..count {
            let t = s.read_b_u16();
            if t >= 0x8000 {
                return false;
            }
            samples_size += t as u32 * 2;
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            if t != 0 {
                let a = s.read_b_u16() as u32;
                let b = s.read_b_u16() as u32;
                if a + b > t as u32 {
                    return false;
                }
            } else {
                s.seek_cur(4);
            }
        }
        // the position list's end mark
        s.seek(o2 as i64 - 1);
        if s.read_u8() != 0xff {
            return false;
        }
        s.seek(o1 as i64);
        let mut n = 0u16;
        loop {
            let p = s.read_u8();
            // (the C# loops forever where the end mark is missing)
            if s.eof {
                return false;
            }
            if p == 0xff {
                break;
            }
            n = n.max(p as u16);
        }
        self.base.number_of_patterns = n + 1;
        // the pattern offsets must be in ascending order
        if self.base.number_of_patterns > 2 {
            s.seek(14 + count as i64 * 8);
            let mut prev = 0;
            for _ in 0..self.base.number_of_patterns {
                let t = s.read_b_u16();
                if t < prev {
                    return false;
                }
                prev = t;
            }
        }
        (o3 + samples_size) as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        // NostalgicPlayer rewinds the stream before converting
        s.seek(0);
        self.samples = s.read_b_u16();
        self.position_list_offset = s.read_b_u32();
        s.seek_cur(4); // pattern offset
        self.sample_data_offset = s.read_b_u32();
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let mut v = Vec::new();
        for _ in 0..self.samples {
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        s.seek(self.position_list_offset as i64);
        let mut v = Vec::new();
        while v.len() < 128 {
            let p = s.read_u8();
            if p == 0xff {
                break;
            }
            v.push(p);
        }
        Some(v)
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let n = self.base.number_of_patterns as usize;
        let mut offs = vec![0u16; n];
        s.seek(14 + self.samples as i64 * 8);
        s.read_array_b_u16(&mut offs);
        let mut out = Vec::new();
        for &o in &offs {
            let mut pat = vec![0u8; 1024];
            let mut w = 0;
            s.seek(o as i64);
            for _ in 0..64 {
                let mut stop = false;
                for _ in 0..4 {
                    let b = s.read_u8();
                    if b == 0xff {
                        // blank note
                        w += 4;
                        continue;
                    }
                    let b2 = s.read_u8();
                    if matches!(b2 & 0x0f, 0xb | 0xd) {
                        stop = true;
                    }
                    let sample = (b >> 2) & 0x1f;
                    let note = ((b2 & 0xf0) >> 4) | ((b & 0x03) << 4);
                    if b >= 0x80 {
                        // with an effect parameter
                        pat[w + 3] = s.read_u8();
                    }
                    pat[w + 2] = (b2 & 0x0f) | ((sample & 0x0f) << 4);
                    if note > 0 {
                        pat[w + 1] = PERIODS[note as usize - 1][1];
                        pat[w] = PERIODS[note as usize - 1][0];
                    }
                    if sample >= 0x10 {
                        pat[w] |= 0x10;
                    }
                    w += 4;
                }
                if stop {
                    break;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_data_offset as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    /// Packs the song as Digital Illusions does: a pattern per position list entry.
    fn pack(song: &Song) -> Vec<u8> {
        let ns = song.samples.len();
        let mut d = vec![0u8; 14];
        d[..2].copy_from_slice(&(ns as u16).to_be_bytes());
        for s in &song.samples {
            d.extend(s.0.to_be_bytes());
            d.extend([s.1, s.2]);
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
        }
        let po = d.len();
        d.resize(po + song.patterns.len() * 2, 0);
        let pl = d.len() as u32;
        d.extend(&song.positions);
        d.push(0xff);
        let mut offs = Vec::new();
        for p in &song.patterns {
            offs.push(d.len() as u16);
            for row in p {
                for n in row {
                    if *n == (0, 0, 0, 0) {
                        d.push(0xff);
                        continue;
                    }
                    let note = if n.1 == 0 {
                        0
                    } else {
                        PERIODS.iter().position(|p| (p[0] as u16) << 8 | p[1] as u16 == n.1).unwrap() as u8 + 1
                    };
                    let b = (n.0 << 2) | (note >> 4) | if n.3 != 0 { 0x80 } else { 0 };
                    d.extend([b, (note << 4) | n.2]);
                    if n.3 != 0 {
                        d.push(n.3);
                    }
                }
            }
        }
        for (i, o) in offs.iter().enumerate() {
            d[po + i * 2..po + i * 2 + 2].copy_from_slice(&o.to_be_bytes());
        }
        let so = d.len() as u32;
        d[2..6].copy_from_slice(&pl.to_be_bytes());
        d[6..10].copy_from_slice(&(offs[0] as u32).to_be_bytes());
        d[10..14].copy_from_slice(&so.to_be_bytes());
        for i in 0..ns {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn digital_illusions() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("Digital Illusions");
        assert_eq!(f, "Digital Illusions");
        // only the samples there are, the rest are written empty
        assert_eq!(m, song.module(0x7f));
    }
}
