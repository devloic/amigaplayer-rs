//! Gnu Protected Module, ported from NostalgicPlayer's GnuProtectedModuleFormat
//! (ProWizard for PC).
//!
//! A ProTracker module with "GPMO" for its mark and the sample volumes doubled.

use super::core::*;

#[derive(Default)]
pub struct GnuProtected {
    base: Base,
    positions: u8,
    restart: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(55, Box::new(GnuProtected::default()))]
}

impl Format for GnuProtected {
    fn name(&self) -> &'static str {
        "Gnu Protected Module"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x440 {
            return false;
        }
        s.seek(0x438);
        if s.read_mark() != b"GPMO" {
            return false;
        }
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
            let size = (s.read_b_u16() as u32 * 2) as u16;
            let fine_tune = s.read_u8();
            let volume = s.read_u8() / 2;
            let loop_start = (s.read_b_u16() as u32 * 2) as u16;
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
        if 0x43c + self.base.number_of_patterns as i64 * 1024 + samples_size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the first pattern's notes
        s.seek(0x43c);
        for _ in 0..4 * 64 {
            let t = s.read_u8();
            if t > 0x13 {
                return false;
            }
            let period = (t & 0x0f) as u16 * 256 + s.read_u8() as u16;
            if period > 0 && period < 0x1c {
                return false;
            }
            s.seek_cur(2);
        }
        true
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
            let volume = s.read_u8() / 2;
            let loop_start = s.read_b_u16();
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
            s.read_into(&mut pat);
            // the Skizzo 2 demo triggers its effects with 8xx; it is removed so the
            // module is not taken for a PC one
            for n in pat.chunks_mut(4) {
                if n[2] & 0x0f == 0x08 {
                    n[2] &= 0xf0;
                    n[3] = 0;
                }
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

    /// The song as a Gnu Protected Module, with an 8xx effect the conversion drops.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = song.module(song.restart);
        for i in 0..31 {
            d[20 + i * 30 + 25] *= 2;
        }
        d[1080..1084].copy_from_slice(b"GPMO");
        // row 0, voice 0: 812 (the song leaves it empty)
        let o = 1084;
        assert_eq!(d[o..o + 4], [0; 4]);
        d[o + 2] = 0x08;
        d[o + 3] = 0x12;
        d
    }

    #[test]
    fn gnu_protected_module() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("Gnu Protected Module");
        assert_eq!(f, "Gnu Protected Module");
        assert_eq!(m, song.module(song.restart));
    }
}
