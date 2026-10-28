//! Fuzzac Packer ("M1.0"), ported from NostalgicPlayer's FuzzacPackerFormat.
//!
//! The sample table has 60-byte names; then the song length, the size of the track
//! data and, per voice and position, a track offset (and a word not used). The
//! offsets count from 0x40 before the end of that table; the tracks are 64
//! ProTracker notes, ended by "SEnd", then the sample data.

use super::core::*;

#[derive(Default)]
pub struct Fuzzac {
    base: Base,
    positions: u8,
    track_offsets: [Vec<u16>; 4],
    position_list: Vec<u8>,
    notes_offset: u32,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(9, Box::new(Fuzzac::default()))]
}

impl Format for Fuzzac {
    fn name(&self) -> &'static str {
        "Fuzzac Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x84a {
            return false;
        }
        s.seek(0);
        if s.read_mark() != b"M1.0" {
            return false;
        }
        s.seek(6);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(60);
            let t = s.read_b_u16() as u32;
            if t >= 0x8000 {
                return false;
            }
            if t != 0 {
                samples_size += t * 2;
                let a = s.read_b_u16() as u32;
                let b = s.read_b_u16() as u32;
                if a + b > t {
                    return false;
                }
                if s.read_b_u16() > 0x40 {
                    return false;
                }
            } else {
                s.seek_cur(6);
            }
        }
        let len = s.read_u8();
        if len == 0 || len > 0x7f {
            return false;
        }
        let offset = s.read_b_u16();
        if offset == 0 {
            return false;
        }
        // the highest track offset must be the one stored
        s.seek(0x846);
        let mut max = 0u16;
        for _ in 0..len as usize * 4 {
            max = max.max(s.read_b_u16());
            s.seek_cur(2);
        }
        if max as u32 + 0x100 != offset as u32 {
            return false;
        }
        let notes = (s.position() - 0x40) as u32;
        let samples = notes + offset as u32 + 4;
        s.seek(samples as i64 - 4);
        if s.read_mark() != b"SEnd" {
            return false;
        }
        // the first track's periods
        s.seek(notes as i64 + 0x40);
        for _ in 0..64 {
            let p = (s.read_b_u32() >> 16) & 0x0fff;
            if p != 0 && !(0x71..=0x358).contains(&p) {
                return false;
            }
        }
        (samples + samples_size) as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x842);
        self.positions = s.read_u8();
        s.seek(0x846);
        for t in &mut self.track_offsets {
            *t = vec![0; self.positions as usize];
            for o in t.iter_mut() {
                *o = s.read_b_u16();
                s.seek_cur(2);
            }
        }
        self.notes_offset = (s.position() - 0x40) as u32;
        let t = &self.track_offsets;
        let key = |p: usize| (t[0][p] as u64) << 48 | (t[1][p] as u64) << 32 | (t[2][p] as u64) << 16 | t[3][p] as u64;
        self.position_list = self.base.build_position_list(self.positions as usize, key).0;
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(6);
        let mut v = Vec::new();
        for _ in 0..31 {
            let mut name = vec![0u8; 60];
            s.read_into(&mut name);
            let length = s.read_b_u16();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_length = if loop_length != 0 { loop_length } else { 1 };
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list.clone())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut pat = vec![0u8; 1024];
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions as usize {
            if self.position_list[i] as i32 > last {
                last += 1;
                for j in 0..4 {
                    s.seek(self.notes_offset as i64 + self.track_offsets[j][i] as i64);
                    for k in 0..64 {
                        s.read_into(&mut pat[j * 4 + k * 16..j * 4 + k * 16 + 4]);
                    }
                }
                push!(out, pat.clone(), s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0x843);
        let off = s.read_b_u16() as i64;
        s.seek(self.notes_offset as i64 + off + 4);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// Packs the song as Fuzzac does: a track per pattern and voice.
    fn pack(song: &Song) -> Vec<u8> {
        let m = song.module(0x7f);
        let mut d = b"M1.0\0\0".to_vec();
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 0));
            d.extend([0u8; 60]);
            d.extend(s.0.to_be_bytes());
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
            d.extend([s.1, s.2]);
        }
        let np = song.patterns.len() as u16;
        d.push(song.positions.len() as u8);
        // the largest track offset + 0x100: tracks count from 0x40
        d.extend(((np * 4 - 1) * 0x100 + 0x40 + 0x100).to_be_bytes());
        d.push(0);
        for v in 0..4 {
            for &p in &song.positions {
                d.extend(((p as u16 * 4 + v) * 0x100 + 0x40).to_be_bytes());
                d.extend([0, 0]);
            }
        }
        for p in 0..np as usize {
            for v in 0..4 {
                for r in 0..64 {
                    let o = 1084 + p * 1024 + r * 16 + v * 4;
                    d.extend(&m[o..o + 4]);
                }
            }
        }
        d.extend(b"SEnd");
        d.extend(&m[1084 + np as usize * 1024..]);
        d
    }

    #[test]
    fn fuzzac() {
        let mut song = Song::new(3);
        // the check reads the fine tune and volume as one word, so no fine tune
        song.samples[1].1 = 0;
        let (f, m) = convert_any(&pack(&song)).expect("Fuzzac Packer");
        assert_eq!(f, "Fuzzac Packer");
        assert_eq!(m, song.module(0x7f));
    }
}
