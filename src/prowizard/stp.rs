//! StarTrekker Packer, ported from NostalgicPlayer's StarTrekkerPackerFormat (after
//! ProWizard for Amiga).
//!
//! The title and sample table of a StarTrekker module, the position list as byte
//! offsets of patterns, the offset of the sample data, and patterns of ProTracker
//! notes with the sample number times four, a 0x80 byte standing for an empty
//! note. Converts to a FLT4 module, as StarTrekker's own.

use super::core::*;

#[derive(Default)]
pub struct Stp {
    base: Base,
    positions: u8,
    pattern_offsets: Vec<u32>,
    position_list: Vec<u8>,
    sample_offset: u32,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(35, Box::new(Stp::default()))]
}

impl Format for Stp {
    fn name(&self) -> &'static str {
        "StarTrekker Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x314 {
            return false;
        }
        // the last two samples are empty (?!?, says the C#)
        s.seek(0xfc);
        for _ in 0..2 {
            if s.read_b_u32() != 0 || s.read_b_u32() != 1 {
                return false;
            }
        }
        // the size of the position table
        let t = s.read_b_u16();
        if t == 0 || t & 3 != 0 {
            return false;
        }
        s.seek(0);
        for _ in 0..20 {
            let c = s.read_u8();
            if c != 0 && c < 0x20 {
                return false;
            }
        }
        let mut samples_size = 0u32;
        for _ in 0..31 {
            let len = s.read_b_u16();
            if len >= 0x8000 {
                return false;
            }
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            let a = s.read_b_u16() as u32;
            let b = s.read_b_u16() as u32;
            if len == 0 {
                if a + b != 1 {
                    return false;
                }
            } else {
                if a + b > len as u32 {
                    return false;
                }
                samples_size += len as u32 * 2;
            }
        }
        s.seek(0x310);
        let off = s.read_b_u32();
        if off == 0 {
            return false;
        }
        if 0x314u32.wrapping_add(off).wrapping_add(samples_size) as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the first 64 notes
        for _ in 0..64 {
            if s.read_u8() != 0x80 {
                s.seek_cur(-1);
                let p = s.read_b_u16() & 0x0fff;
                if p != 0 && !(0x71..=0x358).contains(&p) {
                    return false;
                }
                s.seek_cur(2);
            }
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x10c);
        self.positions = (s.read_b_u16() / 4) as u8;
        s.seek(0x110);
        self.pattern_offsets = vec![0; self.positions as usize];
        s.read_array_b_u32(&mut self.pattern_offsets);
        let po = &self.pattern_offsets;
        self.position_list = self.base.build_position_list(self.positions as usize, |p| po[p]).0;
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut name = vec![0u8; 20];
        s.seek(0);
        s.read_into(&mut name);
        Some(name)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(20);
        let mut v = Vec::new();
        for _ in 0..31 {
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list.clone())
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        0
    }
    fn mark(&self) -> &'static [u8; 4] {
        b"FLT4"
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x310);
        self.sample_offset = 0x314u32.wrapping_add(s.read_b_u32());
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions as usize {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                s.seek(0x314u32.wrapping_add(self.pattern_offsets[i]) as i64);
                for j in 0..64 * 4 {
                    // not into the sample data
                    if s.position() >= self.sample_offset as i64 {
                        break;
                    }
                    let t1 = s.read_u8();
                    if t1 == 0x80 {
                        continue;
                    }
                    let t2 = s.read_u8();
                    let t3 = s.read_u8();
                    let t4 = s.read_u8();
                    // the sample number divided by 4
                    let mut b3 = ((t3 >> 4) | (t1 & 0xf0)) / 4;
                    let b1 = b3 & 0xf0;
                    b3 <<= 4;
                    pat[j * 4..j * 4 + 4].copy_from_slice(&[(t1 & 0x0f) | b1, t2, (t3 & 0x0f) | b3, t4]);
                }
                push!(out, pat, s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_offset as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// The song as StarTrekker Packer: patterns of notes with the sample number
    /// times 4, 0x80 for an empty note.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = vec![0u8; 20];
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(s.0.to_be_bytes());
            d.extend([s.1, s.2]);
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
        }
        d.extend((song.positions.len() as u16 * 4).to_be_bytes());
        d.extend([0, 0]);
        let mut pats = Vec::new();
        let mut offs = Vec::new();
        for p in &song.patterns {
            offs.push(pats.len() as u32);
            for row in p {
                for &(smp, per, fx, par) in row {
                    if (smp, per, fx, par) == (0, 0, 0, 0) {
                        pats.push(0x80);
                    } else {
                        let s4 = smp * 4;
                        pats.extend([(s4 & 0xf0) | (per >> 8) as u8, per as u8, (s4 << 4) | fx, par]);
                    }
                }
            }
        }
        for &p in &song.positions {
            d.extend(offs[p as usize].to_be_bytes());
        }
        d.resize(0x310, 0);
        d.extend((pats.len() as u32).to_be_bytes());
        d.extend(pats);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn stp() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("not recognised");
        assert_eq!(f, "StarTrekker Packer");
        let mut want = song.module(0);
        want[1080..1084].copy_from_slice(b"FLT4");
        assert_eq!(m, want);
    }
}
