//! Binary Packer, ported from NostalgicPlayer's BinaryPackerFormat (ProWizard for
//! PC).
//!
//! A ProTracker module with "BNR!" for its mark and the loop length stored before
//! the loop start. A note is the effect and two flags (no note, force effect F) in
//! the first byte, 36 - note number in the second (* 2), the parameter and the
//! sample number (* 8).

use super::core::*;

#[derive(Default)]
pub struct BinaryPacker {
    base: Base,
    positions: u8,
    restart: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(50, Box::new(BinaryPacker::default()))]
}

impl Format for BinaryPacker {
    fn name(&self) -> &'static str {
        "Binary Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x43c {
            return false;
        }
        s.seek(0x438);
        if s.read_mark() != b"BNR!" {
            return false;
        }
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
            let size = (s.read_b_u16() as u32 * 2) as u16;
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_size = (s.read_b_u16() as u32 * 2) as u16;
            let loop_start = (s.read_b_u16() as u32 * 2) as u16;
            if !test_sample(size, loop_start, loop_size, volume, fine_tune) {
                return false;
            }
            samples_size += size as u32;
        }
        let t = s.read_u8();
        if t == 0 || t > 0x7f {
            return false;
        }
        if self.base.find_highest_pattern_at(s, 0x3b8, 128) > 64 {
            return false;
        }
        0x43c + self.base.number_of_patterns as i64 * 1024 + samples_size as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x3b6);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0u8; 20];
        s.seek(0);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let mut v = Vec::new();
        for _ in 0..31 {
            let mut name = vec![0u8; 22];
            s.read_into(&mut name);
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_length = s.read_b_u16();
            let loop_start = s.read_b_u16();
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut p = vec![0u8; self.positions as usize];
        s.seek(0x3b8);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x43c);
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 * 4 {
                let (b1, b2, b3, b4) = (s.read_u8(), s.read_u8(), s.read_u8(), s.read_u8());
                pat[j * 4 + 3] = b3;
                pat[j * 4 + 2] = ((b1 >> 2) & 0x0f) | ((b4 << 1) & 0xf0);
                pat[j * 4] = (b4 >> 3) & 0xf0;
                // no note
                if b1 & 0x40 != 0 {
                    continue;
                }
                // force a set speed
                if b1 & 0x80 != 0 {
                    pat[j * 4 + 2] |= 0x0f;
                }
                // a note before the table fails, as the C# index does
                let index = usize::try_from(36 - (b2 >> 1) as i32).ok()?;
                pat[j * 4] |= PERIODS[index][0];
                pat[j * 4 + 1] = PERIODS[index][1];
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// The song as a Binary Packer module. An empty note has the no-note flag.
    fn pack(song: &Song) -> Vec<u8> {
        let m = song.module(song.restart);
        let mut d = m[..1080].to_vec();
        // loop length before loop start
        for i in 0..31 {
            let o = 20 + i * 30 + 26;
            d[o..o + 4].copy_from_slice(&[m[o + 2], m[o + 3], m[o], m[o + 1]]);
        }
        d.extend(b"BNR!");
        let table: Vec<u16> = super::PERIODS.iter().map(|p| (p[0] as u16) << 8 | p[1] as u16).collect();
        for p in &song.patterns {
            for row in p {
                for &(smp, per, eff, par) in row {
                    let (flag, n) = if per == 0 {
                        (0x40, 0)
                    } else {
                        (0, (36 - table.iter().position(|&x| x == per).unwrap() as u8) * 2)
                    };
                    d.extend([flag | (eff << 2), n, par, smp << 3]);
                }
            }
        }
        d.extend(&m[1084 + song.patterns.len() * 1024..]);
        d
    }

    #[test]
    fn binary_packer() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("Binary Packer");
        assert_eq!(f, "Binary Packer");
        assert_eq!(m, song.module(song.restart));
    }
}
