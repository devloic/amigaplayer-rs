//! Power Music, ported from NostalgicPlayer's PowerMusicFormat (after ProWizard
//! for Amiga).
//!
//! A ProTracker module marked "!PM!" whose sample data is stored as deltas; a
//! restart of 0xff is ProTracker's 0x7f.

use super::core::*;

#[derive(Default)]
pub struct PowerMusic {
    base: Base,
    positions: u8,
    restart: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(22, Box::new(PowerMusic::default()))]
}

impl Format for PowerMusic {
    fn name(&self) -> &'static str {
        "Power Music"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 1084 {
            return false;
        }
        s.seek(0x438);
        if s.read_mark() != b"!PM!" {
            return false;
        }
        s.seek(0x3b6);
        let len = s.read_u8();
        if len == 0 || len > 0x7f {
            return false;
        }
        // the NoiseTracker restart byte
        let ntk = s.read_u8();
        if ntk > 0x7f && ntk != 0xff {
            return false;
        }
        if s.read_u8() > 0x3f || s.read_u8() > 0x3f {
            return false;
        }
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
            let size = s.read_b_u16();
            if size >= 0x8000 {
                return false;
            }
            samples_size += size as u32 * 2;
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            // (the C# tests the restart byte here, not the sample size)
            if ntk != 0 {
                let t = size as i32 - s.read_b_u16() as i32 + s.read_b_u16() as i32;
                if t < -4 {
                    return false;
                }
            } else {
                let t = s.read_b_u16() as i32 + s.read_b_u16() as i32;
                if t > 2 {
                    return false;
                }
            }
        }
        if self.base.find_highest_pattern_at(s, 0x3b8, len as usize) > 64 {
            return false;
        }
        let npat = self.base.number_of_patterns as i64;
        if 0x43c + npat * 1024 + samples_size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the periods in the first patterns
        s.seek(0x43c);
        let mut period_num = 0;
        let n = ((npat * 256) as u16).min(0x600);
        for _ in 0..n {
            let p = s.read_b_u16() & 0x0fff;
            if p != 0 {
                let (hi, lo) = ((p >> 8) as u8, p as u8);
                if !PERIODS[..36].iter().any(|e| e[0] == hi && e[1] == lo) {
                    return false;
                }
                period_num += 1;
            }
            s.seek_cur(2);
        }
        period_num != 0
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x3b6);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut name = vec![0u8; 20];
        s.seek(0);
        s.read_into(&mut name);
        Some(name)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        // follows the module name
        let mut v = Vec::new();
        for _ in 0..31 {
            let mut name = vec![0u8; 22];
            s.read_into(&mut name);
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut list = vec![0u8; self.positions as usize];
        s.seek(0x3b8);
        s.read_into(&mut list);
        Some(list)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        if self.restart == 0xff { 0x7f } else { self.restart }
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x43c);
        let mut out = Vec::new();
        let mut pat = vec![0u8; 1024];
        for _ in 0..self.base.number_of_patterns {
            s.read_into(&mut pat);
            push!(out, pat.clone(), s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        // undo the deltas; what is missing at the end stays at the last value
        let size: u32 = self.base.sample_lengths.iter().sum();
        let mut all = vec![0u8; size as usize];
        s.read_into(&mut all);
        for i in 1..all.len() {
            all[i] = all[i].wrapping_add(all[i - 1]);
        }
        w.write(&all);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    #[test]
    fn power_music() {
        let song = Song::new(3);
        // a ProTracker module, "!PM!", restart 0xff and delta samples
        let m = song.module(0x7f);
        let mut d = m[..1084].to_vec();
        d[951] = 0xff;
        d[1080..1084].copy_from_slice(b"!PM!");
        d.extend(&m[1084..1084 + 3 * 1024]);
        let mut prev = 0u8;
        for &b in &m[1084 + 3 * 1024..] {
            d.push(b.wrapping_sub(prev));
            prev = b;
        }
        let (f, c) = convert_any(&d).expect("not recognised");
        assert_eq!(f, "Power Music");
        assert_eq!(c, m);
    }
}
