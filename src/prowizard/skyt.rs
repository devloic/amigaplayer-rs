//! SKYT Packer, ported from NostalgicPlayer's SkytPackerFormat (after ProWizard
//! for Amiga).
//!
//! The sample table, "SKYT", the number of positions less one, and per position
//! four track offsets (0x100 per track); tracks are 64 notes of note number,
//! sample, effect and parameter. The sample data follows the last track.

use super::core::*;

#[derive(Default)]
pub struct Skyt {
    base: Base,
    positions: u8,
    track_offsets: Vec<[u16; 4]>,
    position_list: Vec<u8>,
    sample_offset: u16,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(34, Box::new(Skyt::default()))]
}

impl Format for Skyt {
    fn name(&self) -> &'static str {
        "SKYT Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x106 {
            return false;
        }
        s.seek(0x100);
        if s.read_mark() != b"SKYT" {
            return false;
        }
        s.seek(0);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            let t = s.read_b_u16();
            if t >= 0x8000 {
                return false;
            }
            samples_size += t as u32 * 2;
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            if t != 0 {
                let a = s.read_b_u16() as u32;
                let b = s.read_b_u16() as u32;
                if a + b > t as u32 {
                    return false;
                }
            } else {
                s.seek_cur(4);
            }
        }
        s.seek(0x104);
        let len = s.read_u8().wrapping_add(1);
        // the track offsets' low bytes (only as many words as positions)
        s.seek(0x106);
        for _ in 0..len {
            if s.read_b_u16() & 0xff != 0 {
                return false;
            }
        }
        0x106 + len as i64 * 2 * 4 + samples_size as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x104);
        self.positions = s.read_u8().wrapping_add(1);
        s.seek(0x106);
        self.track_offsets = (0..self.positions).map(|_| [0; 4].map(|_: u16| s.read_b_u16())).collect();
        let t = &self.track_offsets;
        let key = |p: usize| (t[p][0] as u64) << 48 | (t[p][1] as u64) << 32 | (t[p][2] as u64) << 16 | t[p][3] as u64;
        self.position_list = self.base.build_position_list(self.positions as usize, key).0;
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(0);
        let mut v = Vec::new();
        for _ in 0..31 {
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16().max(1);
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list.clone())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let track_start = (0x106 + self.positions as u32 * 2 * 4) as u16;
        self.sample_offset = 0;
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions as usize {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                for j in 0..4 {
                    let mut to = self.track_offsets[i][j];
                    // the sample data follows the track furthest on
                    self.sample_offset = self.sample_offset.max(to);
                    if to != 0 {
                        to = to.wrapping_sub(0x100);
                    }
                    s.seek(track_start as i64 + to as i64);
                    for k in 0..64 {
                        let mut b1 = s.read_u8();
                        let b2 = s.read_u8();
                        let b3 = s.read_u8();
                        let b4 = s.read_u8();
                        let o = k * 16 + j * 4;
                        let mut smp = b2 & 0x1f;
                        if smp >= 0x10 {
                            pat[o] = 0x10;
                            smp -= 0x10;
                        }
                        pat[o + 2] = smp << 4;
                        if b1 != 0 {
                            b1 -= 1;
                            let p = PERIODS.get(b1 as usize)?;
                            pat[o] |= p[0];
                            pat[o + 1] = p[1];
                        }
                        pat[o + 2] |= b3 & 0x0f;
                        pat[o + 3] = b4;
                    }
                }
                push!(out, pat, s);
            }
        }
        self.sample_offset = self.sample_offset.wrapping_add(track_start);
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_offset as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::*;
    use crate::prowizard::tests::*;

    /// The song as SKYT: one track per pattern and voice, numbered from 1 (offset
    /// 0x100 per track).
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = Vec::new();
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(s.0.to_be_bytes());
            d.extend([s.1, s.2]);
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
        }
        d.resize(0x100, 0);
        d.extend(b"SKYT");
        d.extend([song.positions.len() as u8 - 1, 0]);
        for &p in &song.positions {
            for v in 0..4u16 {
                d.extend(((p as u16 * 4 + v + 1) * 0x100).to_be_bytes());
            }
        }
        for p in &song.patterns {
            for v in 0..4 {
                for row in p {
                    let (smp, per, fx, par) = row[v];
                    let n = match per {
                        0 => 0,
                        _ => PERIODS.iter().position(|x| ((x[0] as u16) << 8 | x[1] as u16) == per).unwrap() as u8 + 1,
                    };
                    d.extend([n, smp, fx, par]);
                }
            }
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn skyt() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("not recognised");
        assert_eq!(f, "SKYT Packer");
        assert_eq!(m, song.module(0x7f));
    }
}
