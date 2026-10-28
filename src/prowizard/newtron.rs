//! Newtron Packer 1.0 and 2.0, ported from NostalgicPlayer's NewtronPacker10Format
//! and NewtronPacker20Format.
//!
//! 1.0: the sample table size (8 bytes a sample), the song length, the used
//! samples' ProTracker entries without names, the positions and the patterns.
//! 2.0: the song length, a zero, the size of the patterns plus 0x178, 31 sample
//! entries, 128 positions and the patterns at 380.

use super::core::*;

#[derive(Default)]
pub struct Newtron {
    base: Base,
    v2: bool,
    positions: u8,
    samples: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![
        (59, Box::new(Newtron::default())),
        (60, Box::new(Newtron { v2: true, ..Default::default() })),
    ]
}

/// The common sample entry check: (size, loop start, loop size) in bytes.
fn read_sample(s: &mut Stream) -> (u16, u16, u16, bool) {
    let size = s.read_b_u16().wrapping_mul(2);
    let fine_tune = s.read_u8();
    let volume = s.read_u8();
    let loop_start = s.read_b_u16().wrapping_mul(2);
    let loop_size = s.read_b_u16().wrapping_mul(2);
    (size, loop_start, loop_size, test_sample(size, loop_start, loop_size, volume, fine_tune))
}

impl Newtron {
    fn check10(&mut self, s: &mut Stream) -> bool {
        if s.len() < 8 {
            return false;
        }
        s.seek(0);
        let count = s.read_b_u16();
        if count == 0 || count % 8 != 0 {
            return false;
        }
        let count = count / 8;
        if s.read_u8() != 0 {
            return false;
        }
        let pos_len = s.read_u8();
        if pos_len == 0 || pos_len > 0x7f {
            return false;
        }
        s.seek(8);
        let mut samples_size = 0u32;
        for _ in 0..count {
            let (size, ls, lz, ok) = read_sample(s);
            if !ok || (size == 0 && ls == 0 && lz == 0) {
                return false;
            }
            samples_size += size as u32;
        }
        if samples_size <= 2 {
            return false;
        }
        if self.base.find_highest_pattern_read(s, pos_len as usize) > 64 {
            return false;
        }
        let need = 8 + count as i64 * 8 + pos_len as i64 + self.base.number_of_patterns as i64 * 1024 + samples_size as i64;
        if need > s.len() + MAX_MISSING {
            return false;
        }
        // the first pattern
        for _ in 0..4 * 64 {
            let t = s.read_u8();
            if t & 0x0f > 0x03 || t & 0xf0 > 0x10 {
                return false;
            }
            let p = (t & 0x0f) as u16 * 256 + s.read_u8() as u16;
            if p > 0 && p < 0x71 {
                return false;
            }
            s.seek_cur(2);
        }
        true
    }

    fn check20(&mut self, s: &mut Stream) -> bool {
        if s.len() < 380 {
            return false;
        }
        s.seek(0);
        let pos_len = s.read_u8();
        if pos_len == 0 || pos_len > 0x7f {
            return false;
        }
        if s.read_u8() != 0 {
            return false;
        }
        let pattern_size = s.read_b_u16();
        let mut samples_size = 0u32;
        for _ in 0..31 {
            let (size, _, _, ok) = read_sample(s);
            if !ok {
                return false;
            }
            samples_size += size as u32;
        }
        if samples_size <= 2 {
            return false;
        }
        if self.base.find_highest_pattern_read(s, 128) > 64 {
            return false;
        }
        let pats = self.base.number_of_patterns as i64 * 1024;
        if 380 + pats != pattern_size as i64 + 4 {
            return false;
        }
        if 380 + pats + samples_size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        for _ in 0..4 * 64 {
            let t = s.read_u8();
            if t > 0x13 {
                return false;
            }
            let p = (t & 0x0f) as u16 * 256 + s.read_u8() as u16;
            if p > 0 && p < 0x71 {
                return false;
            }
            s.seek_cur(2);
        }
        true
    }
}

impl Format for Newtron {
    fn name(&self) -> &'static str {
        if self.v2 { "Newtron Packer 2.0" } else { "Newtron Packer 1.0" }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if self.v2 { self.check20(s) } else { self.check10(s) }
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0);
        if self.v2 {
            self.positions = s.read_u8();
        } else {
            self.samples = (s.read_b_u16() / 8) as u8;
            self.positions = s.read_b_u16() as u8;
        }
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let n = if self.v2 {
            s.seek(4);
            31
        } else {
            s.seek(8);
            self.samples as usize
        };
        let mut v = Vec::new();
        for _ in 0..n {
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
        // follows the samples
        let mut p = vec![0u8; self.positions as usize];
        s.read_into(&mut p);
        Some(p)
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        // 1.0's follow the positions
        if self.v2 {
            s.seek(380);
        }
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

    fn entries(m: &[u8], n: usize) -> Vec<u8> {
        (0..n).flat_map(|i| m[20 + i * 30 + 22..20 + i * 30 + 30].to_vec()).collect()
    }

    fn pack(song: &Song, v2: bool) -> Vec<u8> {
        let m = song.module(0x7f);
        let npat = song.patterns.len();
        let mut d = Vec::new();
        if v2 {
            d.extend([song.positions.len() as u8, 0]);
            d.extend(((380 + npat * 1024 - 4) as u16).to_be_bytes());
            d.extend(entries(&m, 31));
            d.extend(&m[952..1080]);
        } else {
            let n = song.samples.len();
            d.extend(((n * 8) as u16).to_be_bytes());
            d.extend([0, song.positions.len() as u8, 0, 0, 0, 0]);
            d.extend(entries(&m, n));
            d.extend(&song.positions);
        }
        d.extend(&m[1084..]);
        d
    }

    #[test]
    fn newtron() {
        let song = Song::new(3);
        for (v2, name) in [(false, "Newtron Packer 1.0"), (true, "Newtron Packer 2.0")] {
            let (f, m) = convert_any(&pack(&song, v2)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f));
        }
    }
}
