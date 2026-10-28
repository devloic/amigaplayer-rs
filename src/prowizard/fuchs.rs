//! Fuchs Tracker, ported from NostalgicPlayer's FuchsTrackerFormat (ProWizard for
//! PC).
//!
//! A 10-byte name, the total sample size, 16 sample sizes, volumes and loop starts
//! (in bytes; a sample loops to its end), up to 40 positions as words, "SONG", and
//! ProTracker patterns whose effect C parameters are decimal, "INST" and the
//! sample data.

use super::core::*;

#[derive(Default)]
pub struct Fuchs {
    base: Base,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(52, Box::new(Fuchs::default()))]
}

/// The sizes, volumes and loop starts.
fn sample_table(s: &mut Stream) -> [[u16; 16]; 3] {
    let mut t = [[0u16; 16]; 3];
    for a in &mut t {
        s.read_array_b_u16(a);
    }
    t
}

impl Format for Fuchs {
    fn name(&self) -> &'static str {
        "Fuchs Tracker"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 196 {
            return false;
        }
        s.seek(192);
        if s.read_mark() != b"SONG" {
            return false;
        }
        s.seek(10);
        let total = s.read_b_u32();
        if total <= 2 || total >= 65535 * 16 {
            return false;
        }
        let [sizes, volumes, loop_starts] = sample_table(s);
        let mut samples_size = 0u32;
        for i in 0..16 {
            if volumes[i] > 0x40 || sizes[i] < loop_starts[i] {
                return false;
            }
            samples_size += sizes[i] as u32;
        }
        if samples_size <= 2 || samples_size > total {
            return false;
        }
        // the highest pattern number
        s.seek(112);
        let mut n = 0u16;
        for _ in 0..40 {
            let t = s.read_b_u16();
            if t > 40 {
                return false;
            }
            n = n.max(t);
        }
        self.base.number_of_patterns = n + 1;
        200 + self.base.number_of_patterns as i64 * 1024 + samples_size as i64 <= s.len() + MAX_MISSING
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0u8; 20];
        s.seek(0);
        s.read_into(&mut n[..10]);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(14);
        let [sizes, volumes, loop_starts] = sample_table(s);
        let mut v = Vec::new();
        for i in 0..16 {
            let mut loop_length = sizes[i].wrapping_sub(loop_starts[i]);
            if loop_length == 0 || loop_starts[i] == 0 {
                loop_length = 1;
            } else {
                loop_length /= 2;
            }
            let si = SampleInfo {
                name: None,
                length: sizes[i] / 2,
                loop_start: loop_starts[i] / 2,
                loop_length,
                volume: volumes[i] as u8,
                fine_tune: 0,
            };
            push!(v, si, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        // right after the sample table
        let n = s.read_b_u16();
        Some((0..n).map(|_| s.read_b_u16() as u8).collect())
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        0
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(200);
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 * 4 {
                let (b1, b2, b3, mut b4) = (s.read_u8(), s.read_u8(), s.read_u8(), s.read_u8());
                // effect C's decimal parameter back to hex
                if b3 & 0x0f == 0x0c {
                    b4 = match b4 {
                        16..=25 => b4 - 6,
                        32..=41 => b4 - 12,
                        48..=57 => b4 - 18,
                        64..=73 => b4 - 24,
                        80..=89 => b4 - 30,
                        96..=100 => b4 - 36,
                        _ => b4,
                    };
                }
                pat[j * 4..j * 4 + 4].copy_from_slice(&[b1, b2, b3, b4]);
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        // Deviation from the C#: the "INST" mark between the patterns and the
        // sample data is skipped (the C# copies it as the start of the first
        // sample, shifting every sample by 4 bytes; libxmp skips it too).
        let at = s.position();
        if s.read_mark() != b"INST" {
            s.seek(at);
        }
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// The song as a Fuchs Tracker module, which has no fine tunes and only 16
    /// samples, each looping to its end.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = b"FuchsSong\0".to_vec();
        let total: u32 = song.samples.iter().map(|s| s.0 as u32 * 2).sum();
        d.extend(total.to_be_bytes());
        let get = |i: usize| song.samples.get(i).copied().unwrap_or_default();
        for i in 0..16 {
            d.extend((get(i).0 * 2).to_be_bytes());
        }
        for i in 0..16 {
            d.extend((get(i).2 as u16).to_be_bytes());
        }
        for i in 0..16 {
            d.extend((get(i).3 * 2).to_be_bytes());
        }
        d.extend((song.positions.len() as u16).to_be_bytes());
        for i in 0..40 {
            d.extend((*song.positions.get(i).unwrap_or(&0) as u16).to_be_bytes());
        }
        d.extend(b"SONG\0\0\0\0");
        for p in &song.patterns {
            for row in p {
                for &n in row {
                    let mut b = Song::note(n);
                    if n.2 == 0xc {
                        b[3] = (n.3 / 10) << 4 | n.3 % 10;
                    }
                    d.extend(b);
                }
            }
        }
        d.extend(b"INST");
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn fuchs_tracker() {
        let mut song = Song::new(3);
        song.samples[1].1 = 0;
        let (f, m) = convert_any(&pack(&song)).expect("Fuchs Tracker");
        assert_eq!(f, "Fuchs Tracker");
        let mut want = song.module(0);
        want[..10].copy_from_slice(b"FuchsSong\0");
        assert_eq!(m, want);
    }
}
