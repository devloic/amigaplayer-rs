//! Pygmy Packer, ported from NostalgicPlayer's PygmyPackerFormat (after ProWizard
//! for Amiga).
//!
//! A replay-ready dump: 16-byte sample entries with absolute start and loop
//! addresses, the period table, then patterns stored per voice and bottom row
//! first, notes as period indexes and effects renumbered; the position list
//! (ended by 0xff) follows the sample data. Leading empty samples are dropped,
//! so the samples move down.

use super::core::*;

#[derive(Default)]
pub struct Pygmy {
    base: Base,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(33, Box::new(Pygmy::default()))]
}

/// ConvertEffect: Pygmy's effect number to ProTracker's, with the parameter.
fn convert_effect(eff: u8, val: &mut u8) -> u8 {
    match eff {
        0x00 | 0x10 => 0x00,
        0x14 => 0x01,
        0x18 => 0x02,
        0x04 => 0x03,
        0x24 => {
            // vibrato with its nibbles swapped
            *val = (*val >> 4) | (*val << 4);
            0x04
        }
        0x0c => 0x05,
        0x2c => 0x06,
        0x1c => {
            // volume slide up
            *val <<= 4;
            0x0a
        }
        0x20 => 0x0a,
        0x34 => 0x0c,
        0x38 => 0x0d,
        0x3c => {
            if *val == 0x02 {
                *val = 0x01;
            }
            0x0e
        }
        0x40 => 0x0f,
        _ => {
            // unknown: cleared
            *val = 0;
            0x00
        }
    }
}

impl Format for Pygmy {
    fn name(&self) -> &'static str {
        "Pygmy Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x2a4 {
            return false;
        }
        // the last values in the period table
        s.seek(0x2a0);
        if s.read_b_u32() != 0x00780071 {
            return false;
        }
        // the sample addresses go up
        s.seek(0);
        let first = s.read_b_u32();
        s.seek(0);
        let mut next = 0u32;
        for _ in 0..30 {
            let t = s.read_b_u32();
            if t == 0 {
                return false;
            }
            s.seek_cur(12);
            next = s.read_b_u32();
            if next < t {
                return false;
            }
            s.seek_cur(-4);
        }
        if first == next {
            return false;
        }
        s.seek(0);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            let t = s.read_b_u32();
            let len = s.read_b_u16();
            if len != 0 {
                samples_size = samples_size.wrapping_add(len as u32 * 2);
                let l = s.read_b_u32().wrapping_sub(t);
                let l = (l / 2).wrapping_add(s.read_b_u16() as u32);
                if l > len as u32 {
                    return false;
                }
            } else {
                s.seek_cur(6);
            }
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            s.seek_cur(2);
        }
        s.seek(0x1f0);
        if s.read_b_u32() != samples_size {
            return false;
        }
        s.seek(0x25c);
        for i in 0..36 {
            if s.read_b_u16() as i32 != TUNING_PERIODS[0][i] as i32 {
                return false;
            }
        }
        // the offset to the patterns
        s.seek(0x59c);
        let t = first.wrapping_sub(s.read_b_u32());
        if t == 0 || t & 1 != 0 {
            return false;
        }
        self.base.number_of_patterns = (t / 1024) as u16;
        if self.base.number_of_patterns > 64 {
            return false;
        }
        0x636 + self.base.number_of_patterns as i64 * 1024 + samples_size as i64 + 128 <= s.len() + MAX_MISSING
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        // the first sample with a length (the C# would loop forever without one)
        s.seek(4);
        let mut j = 0;
        while s.read_b_u16() == 0 && j < 31 {
            j += 1;
            s.seek_cur(14);
        }
        s.seek(j as i64 * 16);
        let mut v = Vec::new();
        for _ in 0..31 - j {
            let start = s.read_b_u32();
            let length = s.read_b_u16();
            let loop_start = (s.read_b_u32().wrapping_sub(start) / 2) as u16;
            let loop_length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
            s.seek_cur(2);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let size: u32 = self.base.sample_lengths.iter().sum();
        s.seek((0x636u32.wrapping_add(self.base.number_of_patterns as u32 * 1024).wrapping_add(size)) as i64);
        let mut list = Vec::new();
        for _ in 0..128 {
            let t = s.read_u8();
            if t == 0xff {
                break;
            }
            list.push(t);
        }
        Some(list)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        // NoiseTracker mode
        0
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x636);
        let mut out = Vec::new();
        let mut pat = vec![0u8; 1024];
        for _ in 0..self.base.number_of_patterns {
            for j in 0..4 {
                for k in (1..=64).rev() {
                    let mut b1 = s.read_u8();
                    let b2;
                    if b1 != 0 {
                        if b1 >= 0xb8 {
                            b1 = b1.wrapping_neg();
                        }
                        let n = (b1.wrapping_sub(2) / 2) as usize;
                        let p = PERIODS.get(n)?;
                        b2 = p[1];
                        b1 = p[0];
                    } else {
                        b2 = 0;
                    }
                    let mut b3 = s.read_u8();
                    if b3 & 0x80 != 0 {
                        b1 |= 0x10;
                    }
                    b3 <<= 1;
                    let eff = s.read_u8();
                    let mut b4 = s.read_u8();
                    b3 |= convert_effect(eff, &mut b4);
                    let o = (k - 1) * 16 + j * 4;
                    pat[o..o + 4].copy_from_slice(&[b1, b2, b3, b4]);
                }
            }
            push!(out, pat.clone(), s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0x636 + self.base.number_of_patterns as i64 * 1024);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::*;
    use crate::prowizard::tests::*;

    /// The song as Pygmy: absolute sample addresses from 0x10000, the period table,
    /// voices' rows bottom up with note indexes and Pygmy's effect numbers.
    fn pack(song: &Song) -> Vec<u8> {
        let npat = song.patterns.len() as u32;
        let first = 0x10000u32;
        let total: u32 = song.samples.iter().map(|s| s.0 as u32 * 2).sum();
        let mut d = Vec::new();
        let mut a = first;
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(a.to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend((a + s.3 as u32 * 2).to_be_bytes());
            d.extend(s.4.to_be_bytes());
            d.extend([s.1, s.2, 0, 0]);
            a += s.0 as u32 * 2;
        }
        d.extend(total.to_be_bytes());
        d.resize(0x25c, 0);
        for p in TUNING_PERIODS[0] {
            d.extend((p as u16).to_be_bytes());
        }
        d.resize(0x59c, 0);
        d.extend((first - npat * 1024).to_be_bytes());
        d.resize(0x636, 0);
        for p in &song.patterns {
            for v in 0..4 {
                for r in (0..64).rev() {
                    let (smp, per, fx, par) = p[r][v];
                    let n = match per {
                        0 => 0,
                        _ => PERIODS.iter().position(|x| ((x[0] as u16) << 8 | x[1] as u16) == per).unwrap() as u8 * 2 + 2,
                    };
                    let eff = match fx {
                        0 => 0x00,
                        0xc => 0x34,
                        _ => unreachable!(),
                    };
                    d.extend([n, ((smp & 0x0f) << 3) | if smp & 0x10 != 0 { 0x80 } else { 0 }, eff, par]);
                }
            }
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d.extend(&song.positions);
        d.push(0xff);
        d.resize(d.len() + 127 - song.positions.len(), 0);
        d
    }

    #[test]
    fn pygmy() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("not recognised");
        assert_eq!(f, "Pygmy Packer");
        assert_eq!(m, song.module(0));
    }
}
