//! Wanton Packer and Polka Packer, ported from NostalgicPlayer's
//! WantonPackerFormat and PolkaPackerFormat.
//!
//! Both are ProTracker modules with the notes rewritten as 4 bytes: note number,
//! sample, effect, parameter. Wanton's mark is "WN\0" and the pattern count, and
//! patterns past those the song uses may follow (skipped). Polka ("PWR." or
//! "PSUX") keeps a sample address in the name and, for "PWR.", the loop start in
//! bytes.

use super::core::*;

#[derive(Default)]
pub struct Wanton {
    base: Base,
    /// Polka's loop start divisor; 0 for Wanton.
    polka: u16,
    restart: u8,
    real_patterns: u16,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(46, Box::new(Wanton::default())), (21, Box::new(Wanton { polka: 1, ..Default::default() }))]
}

impl Wanton {
    fn check_wanton(&mut self, s: &mut Stream) -> bool {
        s.seek(0x438);
        let t = s.read_b_u32();
        if t & 0xffffff00 != 0x574e0000 {
            return false;
        }
        self.base.number_of_patterns = (t & 0xff) as u16;
        if self.base.number_of_patterns == 0 || self.base.number_of_patterns > 64 {
            return false;
        }
        if !check_first_pattern(s) {
            return false;
        }
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
            let len = s.read_b_u16();
            if len >= 0x8000 {
                return false;
            }
            samples_size += len as u32 * 2;
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            if len != 0 {
                let a = s.read_b_u16() as u32;
                let b = s.read_b_u16() as u32;
                if a + b > len as u32 {
                    return false;
                }
            } else {
                s.seek_cur(4);
            }
        }
        (0x43c + self.base.number_of_patterns as u32 * 1024 + samples_size) as i64 <= s.len() + MAX_MISSING
    }

    fn check_polka(&mut self, s: &mut Stream) -> bool {
        s.seek(0x438);
        self.polka = match &s.read_mark()[..] {
            b"PWR." => 2,
            b"PSUX" => 1,
            _ => return false,
        };
        if !check_first_pattern(s) {
            return false;
        }
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(18);
            // the sample's address
            if s.read_b_u32() == 0 {
                return false;
            }
            let len = s.read_b_u16();
            if len > 0x8000 {
                return false;
            }
            samples_size += len as u32 * 2;
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            if len != 0 {
                let a = (s.read_b_u16() / self.polka) as u32;
                let b = s.read_b_u16() as u32;
                if a + b > len as u32 {
                    return false;
                }
            } else {
                s.seek_cur(4);
            }
        }
        s.seek(0x3b6);
        let len = s.read_u8();
        if self.base.find_highest_pattern_at(s, 0x3b8, len as usize) > 64 {
            return false;
        }
        (0x43c + self.base.number_of_patterns as u32 * 1024 + samples_size) as i64 <= s.len() + MAX_MISSING
    }
}

/// The sample numbers in the first pattern.
fn check_first_pattern(s: &mut Stream) -> bool {
    for _ in 0..64 * 4 {
        s.seek_cur(1);
        if s.read_u8() > 0x1f {
            return false;
        }
        s.seek_cur(2);
    }
    true
}

impl Format for Wanton {
    fn name(&self) -> &'static str {
        if self.polka == 0 { "Wanton Packer" } else { "Polka Packer" }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x43c {
            return false;
        }
        if self.polka == 0 { self.check_wanton(s) } else { self.check_polka(s) }
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
            if self.polka == 0 {
                s.read_into(&mut name);
            } else {
                s.read_into(&mut name[..18]);
                s.seek_cur(4);
            }
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let mut loop_start = s.read_b_u16();
            if self.polka != 0 {
                loop_start /= self.polka;
            }
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let n = s.read_u8();
        self.restart = s.read_u8();
        let mut v = vec![0u8; n as usize];
        s.read_into(&mut v);
        s.seek_cur(128 - n as i64);
        if self.polka == 0 {
            // the number of patterns stored, unused ones included
            s.seek_cur(3);
            self.real_patterns = s.read_u8() as u16;
        } else {
            s.seek_cur(4);
        }
        Some(v)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 * 4 {
                let n = s.read_u8();
                let (mut b1, b2) = if n != 0 {
                    let i = (n.wrapping_sub(2) / 2) as usize;
                    (PERIODS[i][0], PERIODS[i][1])
                } else {
                    (0, 0)
                };
                let mut smp = s.read_u8();
                if smp >= 0x10 {
                    b1 |= 0x10;
                    smp -= 0x10;
                }
                let b3 = (smp << 4) | s.read_u8();
                let b4 = s.read_u8();
                pat[j * 4..j * 4 + 4].copy_from_slice(&[b1, b2, b3, b4]);
            }
            push!(out, pat, s);
        }
        // skip the patterns not used
        if self.base.number_of_patterns < self.real_patterns {
            s.seek_cur((self.real_patterns - self.base.number_of_patterns) as i64 * 0x300);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    /// Packs the song as Wanton (`mark` None) or Polka with the mark given.
    fn pack(song: &Song, mark: Option<&[u8; 4]>) -> Vec<u8> {
        let mut d = song.module(song.restart)[..1084].to_vec();
        let np = song.patterns.len();
        match mark {
            None => d[1080..1084].copy_from_slice(&[b'W', b'N', 0, np as u8]),
            Some(m) => {
                d[1080..1084].copy_from_slice(m);
                for i in 0..31 {
                    let o = 20 + i * 30;
                    d[o + 18..o + 22].copy_from_slice(&0x1000u32.to_be_bytes());
                    if m == b"PWR." {
                        let ls = u16::from_be_bytes([d[o + 26], d[o + 27]]) * 2;
                        d[o + 26..o + 28].copy_from_slice(&ls.to_be_bytes());
                    }
                }
            }
        }
        for p in &song.patterns {
            for row in p {
                for n in row {
                    let note = if n.1 == 0 {
                        0
                    } else {
                        PERIODS.iter().position(|p| (p[0] as u16) << 8 | p[1] as u16 == n.1).unwrap() as u8 * 2 + 2
                    };
                    d.extend([note, n.0, n.2, n.3]);
                }
            }
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    /// The song with a high note: its note number (above 0x1f) is what keeps
    /// ProRunner 1's check, tried first, from taking these modules.
    fn song() -> Song {
        let mut song = Song::new(3);
        song.patterns[1][5][2] = (2, 214, 0, 0);
        song
    }

    #[test]
    fn wanton() {
        let song = song();
        let (f, m) = convert_any(&pack(&song, None)).expect("Wanton Packer");
        assert_eq!(f, "Wanton Packer");
        assert_eq!(m, song.module(song.restart));
    }

    #[test]
    fn polka() {
        let song = song();
        for mark in [b"PWR.", b"PSUX"] {
            let (f, m) = convert_any(&pack(&song, Some(mark))).expect("Polka Packer");
            assert_eq!(f, "Polka Packer");
            // the name keeps 18 bytes, the address is dropped
            assert_eq!(m, song.module(song.restart));
        }
    }
}
