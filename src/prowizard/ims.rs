//! Images Music System, ported from NostalgicPlayer's ImagesMusicSystemFormat
//! (after libxmp).
//!
//! A ProTracker module with 20-character sample names and a negated fine tune
//! word, and patterns of 3-byte notes: a note number (0x3f: none) with the
//! sample's high bit, then the sample's low nibble, effect and parameter.

use super::core::*;

/// Note numbers to periods; 48 and above are no period.
const EXTENDED_PERIODS: [u16; 64] = [
    1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570,
    538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160,
    151, 143, 135, 127, 120, 113, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[derive(Default)]
pub struct Ims {
    base: Base,
    real_number_of_patterns: u16,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(70, Box::new(Ims::default()))]
}

/// TestName: no high or control characters.
fn test_name(b: &[u8]) -> bool {
    b.iter().all(|&c| c <= 0x7f && !(c > 0 && c < 32))
}

impl Format for Ims {
    fn name(&self) -> &'static str {
        "Images Music System"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x43c {
            return false;
        }
        s.seek(20);
        let mut buf = [0u8; 128];
        let mut sample_size = 0i32;
        for _ in 0..31 {
            s.read_into(&mut buf[..20]);
            s.seek_cur(2);
            let length = s.read_b_u16();
            s.seek_cur(1);
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            if s.eof {
                return false;
            }
            sample_size += length as i32 * 2;
            if !test_name(&buf) || volume > 0x40 || length > 0x8000 || loop_start > length {
                return false;
            }
            if length != 0 && loop_length as u32 > 2 * length as u32 {
                return false;
            }
        }
        if sample_size < 8 {
            return false;
        }
        let positions = s.read_u8();
        let zero = s.read_u8();
        s.read_into(&mut buf);
        let magic = s.read_b_u32();
        if s.eof {
            return false;
        }
        // (the C# is not sure what the zero byte is)
        if zero > 1 || magic & 0xff != 0x3c || positions > 0x7f || positions == 0 {
            return false;
        }
        self.base.find_highest_pattern(&buf) <= 0x7f
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
            s.read_into(&mut name[..20]);
            let fine_tune = ((-(s.read_b_u16() as u8 as i8 as i32)) & 0x0f) as u8;
            let length = s.read_b_u16();
            let volume = s.read_b_u16() as u8;
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let n = s.read_u8() as usize;
        s.seek_cur(1);
        let mut list = vec![0u8; n];
        s.read_into(&mut list);
        s.seek_cur(128 - n as i64);
        // unused patterns are not saved
        self.real_number_of_patterns = self.base.number_of_patterns;
        self.base.find_highest_pattern(&list);
        Some(list)
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        // skip the mark
        s.seek_cur(4);
        let mut out = Vec::new();
        let mut pat = vec![0u8; 1024];
        for _ in 0..self.base.number_of_patterns {
            for j in 0..64 * 4 {
                let t = s.read_u8();
                let mut b1: u8 = if t & 0x40 != 0 { 0x10 } else { 0 };
                let mut b3 = s.read_u8();
                let mut b4 = s.read_u8();
                let mut b2 = t & 0x3f;
                if b2 != 0x3f {
                    let p = EXTENDED_PERIODS[b2 as usize];
                    b1 |= ((p & 0x0f00) >> 8) as u8;
                    b2 = p as u8;
                } else {
                    b2 = 0;
                }
                // pattern break in hex, as in Unic Tracker (libxmp assumes so for IMS too)
                if b3 & 0x0f == 0x0d {
                    b4 = ((b4 / 10) as u32 * 0x10 + (b4 % 10) as u32) as u8;
                }
                // Beast-Busters.ims stops a sub-song with 800: make it F00
                if b3 & 0x0f == 0x08 && b4 == 0 {
                    b3 = (b3 & 0xf0) | 0x0f;
                }
                pat[j * 4..j * 4 + 4].copy_from_slice(&[b1, b2, b3, b4]);
            }
            push!(out, pat.clone(), s);
        }
        // extra patterns stored are skipped
        if self.base.number_of_patterns < self.real_number_of_patterns {
            s.seek_cur((self.real_number_of_patterns - self.base.number_of_patterns) as i64 * 0x300);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use super::EXTENDED_PERIODS;
    use crate::prowizard::tests::*;

    /// The song as IMS: 30-byte sample entries with 20-character names and a
    /// negated fine tune, and 3-byte notes.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = vec![0u8; 20];
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend([0u8; 20]);
            d.extend(((-(s.1 as i16)) as u16).to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend((s.2 as u16).to_be_bytes());
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
        }
        d.push(song.positions.len() as u8);
        d.push(0);
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d.extend(pl);
        d.extend([0, 0, 0, 0x3c]);
        for p in &song.patterns {
            for row in p {
                for &(smp, per, fx, par) in row {
                    let n = if per == 0 { 0x3f } else { EXTENDED_PERIODS.iter().position(|&x| x == per).unwrap() as u8 };
                    d.extend([n | if smp & 0x10 != 0 { 0x40 } else { 0 }, (smp << 4) | fx, par]);
                }
            }
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn ims() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("not recognised");
        assert_eq!(f, "Images Music System");
        assert_eq!(m, song.module(0x7f));
    }
}
