//! Mosh Player, ported from NostalgicPlayer's MoshFormat.
//!
//! A ProTracker module without names: a sample's words in the order loop length,
//! loop start, fine tune and volume, length; then the song length, restart,
//! positions and M.K. at 0x17a, and the patterns as they are.

use super::core::*;

#[derive(Default)]
pub struct Mosh {
    base: Base,
    positions: u8,
    restart: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(58, Box::new(Mosh::default()))]
}

impl Format for Mosh {
    fn name(&self) -> &'static str {
        "Mosh Player"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x17e {
            return false;
        }
        s.seek(0x17a);
        if s.read_mark() != b"M.K." {
            return false;
        }
        s.seek(0);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            let loop_size = s.read_b_u16().wrapping_mul(2);
            let loop_start = s.read_b_u16().wrapping_mul(2);
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let size = s.read_b_u16().wrapping_mul(2);
            if !test_sample(size, loop_start, loop_size, volume, fine_tune) {
                return false;
            }
            samples_size += size as u32;
        }
        let t = s.read_u8();
        if t == 0 || t > 0x7f {
            return false;
        }
        if self.base.find_highest_pattern_at(s, 250, 128) > 64 {
            return false;
        }
        if 0x17e + self.base.number_of_patterns as i64 * 1024 + samples_size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the first pattern
        s.seek(0x17e);
        for _ in 0..4 * 64 {
            let t = s.read_u8();
            if t > 0x13 {
                return false;
            }
            let p = (t & 0x0f) as u16 * 256 + s.read_u8() as u16;
            if p > 0 && p < 0x1c {
                return false;
            }
            s.seek_cur(2);
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0xf8);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(0);
        let mut v = Vec::new();
        for _ in 0..31 {
            let loop_length = s.read_b_u16();
            let loop_start = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut p = vec![0u8; self.positions as usize];
        s.seek(0xfa);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x17e);
        let mut out = Vec::new();
        let mut pat = vec![0u8; 1024];
        for _ in 0..self.base.number_of_patterns {
            s.read_into(&mut pat);
            push!(out, pat.clone(), s);
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

    fn pack(song: &Song) -> Vec<u8> {
        let m = song.module(song.restart);
        let mut d = Vec::new();
        for i in 0..31 {
            let o = 20 + i * 30 + 22;
            // length, fine tune + volume, loop start, loop length -> reordered
            d.extend(&m[o + 6..o + 8]);
            d.extend(&m[o + 4..o + 6]);
            d.extend(&m[o + 2..o + 4]);
            d.extend(&m[o..o + 2]);
        }
        d.extend(&m[950..1084]);
        d.extend(&m[1084..]);
        d
    }

    #[test]
    fn mosh() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("Mosh");
        assert_eq!(f, "Mosh Player");
        assert_eq!(m, song.module(song.restart));
    }
}
