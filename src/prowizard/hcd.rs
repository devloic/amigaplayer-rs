//! HCD Protector, ported from NostalgicPlayer's HcdProtectorFormat (ProWizard for
//! PC).
//!
//! A ProTracker module whose loop starts are offset by 0x345 words and whose notes
//! are shifted left by 3 bits.

use super::core::*;

#[derive(Default)]
pub struct Hcd {
    base: Base,
    positions: u8,
    restart: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(56, Box::new(Hcd::default()))]
}

impl Format for Hcd {
    fn name(&self) -> &'static str {
        "HCD Protector"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x43c {
            return false;
        }
        s.seek(0x438);
        if s.read_mark() != b"M.K." {
            return false;
        }
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
            let size = (s.read_b_u16() as u32 * 2) as u16;
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = (s.read_b_u16() as u32 * 2).wrapping_sub(0x68a) as u16;
            let loop_size = (s.read_b_u16() as u32 * 2) as u16;
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
            let loop_start = s.read_b_u16().wrapping_sub(0x345);
            let loop_length = s.read_b_u16();
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
                pat[j * 4] = b1 >> 3;
                pat[j * 4 + 1] = (b2 >> 3) | (b1 << 5);
                pat[j * 4 + 2] = (b3 >> 3) | (b2 << 5);
                pat[j * 4 + 3] = (b4 >> 3) | (b3 << 5);
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

    /// The song as an HCD-protected module.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = song.module(song.restart);
        for i in 0..31 {
            let o = 20 + i * 30 + 26;
            let ls = u16::from_be_bytes([d[o], d[o + 1]]) + 0x345;
            d[o..o + 2].copy_from_slice(&ls.to_be_bytes());
        }
        let end = 1084 + song.patterns.len() * 1024;
        for n in d[1084..end].chunks_mut(4) {
            let v = u32::from_be_bytes([n[0], n[1], n[2], n[3]]) << 3;
            n.copy_from_slice(&v.to_be_bytes());
        }
        d
    }

    #[test]
    fn hcd_protector() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("HCD Protector");
        assert_eq!(f, "HCD Protector");
        assert_eq!(m, song.module(song.restart));
    }
}
