//! Xann Packer, ported from NostalgicPlayer's XannPackerFormat (after ProWizard for
//! Amiga).
//!
//! A replay-ready dump with absolute addresses: 128 pattern addresses as the
//! position list, 16-byte sample entries with loop and start addresses, then
//! patterns of notes as sample * 8, note number * 2, effect * 4 and parameter.

use super::core::*;

#[derive(Default)]
pub struct Xann {
    base: Base,
    low_pattern_offset: u32,
    origin: u32,
    positions: u8,
    pattern_offsets: Vec<u32>,
    sample_addresses: Vec<u32>,
    sample_loop_addresses: Vec<u32>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(47, Box::new(Xann::default()))]
}

/// FindPatternOffsets: the lowest and highest pattern address and how many
/// positions there are (non-zero addresses after the first).
fn find_pattern_offsets(s: &mut Stream) -> (u32, u32, u8) {
    s.seek(0);
    let mut low = s.read_b_u32();
    let mut hi = low;
    let mut count = 1u8;
    for _ in 1..127 {
        let t = s.read_b_u32();
        if t != 0 {
            low = low.min(t);
            hi = hi.max(t);
            count += 1;
        }
    }
    (low, hi, count)
}

/// ConvertEffect: Xann's effect number to ProTracker's, with the parameter.
fn convert_effect(e: &mut u8, v: &mut u8) {
    *e = match *e {
        0x00 => 0x0,
        0x04 => 0x0,
        0x08 => 0x1,
        0x0c => 0x2,
        0x10 => {
            *v = 0;
            0x3
        }
        0x14 => 0x3,
        0x18 => {
            *v = 0;
            0x4
        }
        0x1c => 0x4,
        0x20 => {
            *v <<= 4;
            0x5
        }
        0x24 => 0x5,
        0x28 => {
            *v <<= 4;
            0x6
        }
        0x2c => 0x6,
        0x38 => 0x9,
        0x3c => {
            *v <<= 4;
            0xa
        }
        0x40 => 0xa,
        0x44 => 0xb,
        0x48 => 0xc,
        0x4c => 0xd,
        0x50 => 0xf,
        0x58 => {
            *v = 0x01;
            0xe
        }
        0x5c => {
            *v |= 0x10;
            0xe
        }
        0x60 => {
            *v |= 0x20;
            0xe
        }
        0x74 | 0x78 => {
            *v |= 0x60;
            0xe
        }
        0x84 => {
            *v |= 0x90;
            0xe
        }
        0x88 => {
            *v |= 0xa0;
            0xe
        }
        0x8c => {
            *v |= 0xb0;
            0xe
        }
        0x94 => {
            *v |= 0xd0;
            0xe
        }
        0x98 => {
            *v |= 0xe0;
            0xe
        }
        _ => {
            *v = 0;
            0x0
        }
    };
}

impl Format for Xann {
    fn name(&self) -> &'static str {
        "Xann Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x83c {
            return false;
        }
        // the first two pattern addresses
        s.seek(0);
        let mut t = s.read_b_u32();
        if t > 0xf8000 {
            return false;
        }
        let t1 = s.read_b_u32();
        if t1 == 0 || t1 > 0xf8000 {
            return false;
        }
        t = if t1 > t { t1 - t } else { t - t1 };
        if t % 1024 != 0 {
            return false;
        }
        s.seek(0x206);
        let mut samples_size = 0u32;
        let mut previous = 0u32;
        for _ in 0..31 {
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            let loop_addr = s.read_b_u32();
            let loop_len = s.read_b_u16();
            let addr = s.read_b_u32();
            let len = s.read_b_u16();
            s.seek_cur(2);
            if addr == 0 || addr < previous {
                return false;
            }
            previous = addr;
            if len == 0 {
                if loop_len != 1 || loop_addr != addr {
                    return false;
                }
            } else {
                if len > 0x8000 || loop_len > len {
                    return false;
                }
                samples_size = samples_size.wrapping_add(len as u32 * 2);
            }
        }
        // the first patterns
        s.seek(0x43c);
        for _ in 0..1024 {
            s.seek_cur(1);
            if s.read_u8() > 0x48 {
                return false;
            }
            // the volume command's volume
            if s.read_u8() == 0x48 {
                if s.read_u8() > 0x40 {
                    return false;
                }
            } else {
                s.seek_cur(1);
            }
        }
        let (low, high, _) = find_pattern_offsets(s);
        let t = high.wrapping_sub(low);
        if t == 0 || t % 1024 != 0 {
            return false;
        }
        self.base.number_of_patterns = (t / 1024 + 1) as u16;
        // the first sample's address follows the last pattern
        s.seek(0x20e);
        let first = s.read_b_u32();
        if high.wrapping_add(1024) != first || previous == first {
            return false;
        }
        let t = first.wrapping_sub(low & 0xf000);
        t.wrapping_add(samples_size) as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        let (low, _, count) = find_pattern_offsets(s);
        self.low_pattern_offset = low;
        self.origin = low & 0xf000;
        self.positions = count;
        s.seek(0);
        self.pattern_offsets = (0..count).map(|_| s.read_b_u32().wrapping_sub(self.origin)).collect();
        s.seek(0x206);
        self.sample_addresses = vec![0; 31];
        self.sample_loop_addresses = vec![0; 31];
        for i in 0..31 {
            s.seek_cur(2);
            self.sample_loop_addresses[i] = s.read_b_u32().wrapping_sub(self.origin);
            s.seek_cur(2);
            self.sample_addresses[i] = s.read_b_u32().wrapping_sub(self.origin);
            s.seek_cur(4);
        }
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(0x206);
        let mut v = Vec::new();
        for i in 0..31 {
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            s.seek_cur(4);
            let loop_length = s.read_b_u16();
            s.seek_cur(4);
            let length = s.read_b_u16();
            s.seek_cur(2);
            let loop_start = (self.sample_loop_addresses[i].wrapping_sub(self.sample_addresses[i]) / 2) as u16;
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        let (o, low) = (self.origin, self.low_pattern_offset);
        Some(self.pattern_offsets.iter().map(|&p| (p.wrapping_add(o).wrapping_sub(low) / 1024) as u8).collect())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        // (as the C# has it: the lowest address plus its 4K origin, not minus)
        s.seek(self.low_pattern_offset.wrapping_add(self.origin) as i64);
        let mut out = Vec::new();
        let mut pat = vec![0u8; 1024];
        for _ in 0..self.base.number_of_patterns {
            for j in 0..64 * 4 {
                let o = j * 4;
                let mut t = s.read_u8() >> 3;
                if t >= 0x10 {
                    pat[o] = 0x10;
                    t -= 0x10;
                } else {
                    pat[o] = 0;
                }
                pat[o + 2] = t << 4;
                let n = s.read_u8();
                if n != 0 {
                    let p = PERIODS.get((n.wrapping_sub(2) / 2) as usize)?;
                    pat[o] |= p[0];
                    pat[o + 1] = p[1];
                } else {
                    pat[o + 1] = 0;
                }
                let mut e = s.read_u8();
                let mut v = s.read_u8();
                convert_effect(&mut e, &mut v);
                pat[o + 2] |= e;
                pat[o + 3] = v;
            }
            push!(out, pat.clone(), s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_addresses[0] as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::*;
    use crate::prowizard::tests::*;

    /// The song as Xann, its addresses those in the file (patterns at 0x43c).
    fn pack(song: &Song) -> Vec<u8> {
        let npat = song.patterns.len() as u32;
        let mut d = Vec::new();
        for &p in &song.positions {
            d.extend((0x43c + p as u32 * 1024).to_be_bytes());
        }
        d.resize(0x206, 0);
        let mut a = 0x43c + npat * 1024;
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend([s.1, s.2]);
            d.extend((a + s.3 as u32 * 2).to_be_bytes());
            d.extend(s.4.to_be_bytes());
            d.extend(a.to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend([0, 0]);
            a += s.0 as u32 * 2;
        }
        d.resize(0x43c, 0);
        for p in &song.patterns {
            for row in p {
                for &(smp, per, fx, par) in row {
                    let n = match per {
                        0 => 0,
                        _ => PERIODS.iter().position(|x| ((x[0] as u16) << 8 | x[1] as u16) == per).unwrap() as u8 * 2 + 2,
                    };
                    let e = match fx {
                        0 => 0x00,
                        0xc => 0x48,
                        _ => unreachable!(),
                    };
                    d.extend([smp << 3, n, e, par]);
                }
            }
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn xann() {
        // the check reads four patterns' worth of notes
        let song = Song::new(4);
        let (f, m) = convert_any(&pack(&song)).expect("not recognised");
        assert_eq!(f, "Xann Packer");
        assert_eq!(m, song.module(0x7f));
    }
}
