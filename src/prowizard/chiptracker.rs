//! ChipTracker ("KRIS"), ported from NostalgicPlayer's ChipTrackerFormat.
//!
//! A ProTracker-like header with the mark at 0x3b8, then a track number and
//! transpose per voice and position; tracks of 64 4-byte rows (note number, sample,
//! effect, parameter; 0xa8 for no note) follow at 0x7c0.

use super::core::*;

/// The period table with extra octaves.
const CHIP_PERIODS: [u16; 84] = [
    3424, 3232, 3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1812, 1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140,
    1076, 1016, 960, 906, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302,
    285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 107, 101, 95, 90, 85, 80, 75, 71,
    67, 63, 60, 56, 53, 50, 47, 45, 42, 40, 37, 35, 33, 31, 30, 28,
];

#[derive(Default)]
pub struct ChipTracker {
    base: Base,
    positions: usize,
    track_offsets: Vec<u16>,
    position_list: Vec<u8>,
    tracks_size: i64,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(5, Box::new(ChipTracker::default()))]
}

impl Format for ChipTracker {
    fn name(&self) -> &'static str {
        "ChipTracker"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x7c0 {
            return false;
        }
        s.seek(0x3b8);
        if s.read_mark() != b"KRIS" {
            return false;
        }
        s.seek(0x16);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(0x16);
            let t = s.read_b_u16();
            if t >= 0x8000 {
                return false;
            }
            samples_size += t as u32 * 2;
            if s.read_b_u16() > 0x40 {
                return false;
            }
            s.seek_cur(4);
        }
        s.seek(0x3bc);
        let t = s.read_b_u16();
        if t == 0 {
            return false;
        }
        let n = t as usize / 256;
        let mut tracks = 0u32;
        for _ in 0..n * 4 {
            tracks = tracks.max((s.read_b_u16() & 0xff00) as u32);
        }
        tracks += 0x100;
        if (0x7c0 + tracks + samples_size) as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the first 4 tracks must have empty rows
        s.seek(0x7c0);
        let mut empty = 0;
        for _ in 0..4 * 64 {
            if s.read_b_u16() == 0xa800 {
                empty += 1;
            }
            s.seek_cur(2);
        }
        empty != 0
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x3bc);
        self.positions = s.read_b_u16() as usize / 256;
        self.track_offsets = vec![0; self.positions * 4];
        s.read_array_b_u16(&mut self.track_offsets);
        self.tracks_size = self.track_offsets.iter().map(|&t| (t & 0xff00) as i64).max().unwrap_or(0) + 0x100;
        let t = &self.track_offsets;
        let key = |p: usize| (t[p * 4] as u64) << 48 | (t[p * 4 + 1] as u64) << 32 | (t[p * 4 + 2] as u64) << 16 | t[p * 4 + 3] as u64;
        self.position_list = self.base.build_position_list(self.positions, key).0;
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        // 22 bytes are read, the first 20 written
        let mut n = vec![0u8; 0x16];
        s.seek(0);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let mut v = Vec::new();
        for _ in 0..31 {
            let name = if s.read_u8() == 0x01 {
                // no name
                s.seek_cur(21);
                None
            } else {
                let mut n = vec![0u8; 22];
                s.seek_cur(-1);
                s.read_into(&mut n);
                Some(n)
            };
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16() / 2;
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name, length, loop_start, loop_length, volume, fine_tune }, s);
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
        for i in 0..self.position_list.len() {
            if self.position_list[i] as i32 > last {
                last += 1;
                for j in 0..4 {
                    let t = self.track_offsets[i * 4 + j];
                    let transpose = t & 0x00ff;
                    s.seek(0x7c0 + (t & 0xff00) as i64);
                    for k in 0..64 {
                        let mut c = [0u8; 4];
                        for b in &mut c {
                            *b = s.read_u8();
                        }
                        let (mut b1, mut b3) = (0u8, c[1]);
                        if b3 >= 0x10 {
                            b1 = 0x10;
                            b3 -= 0x10;
                        }
                        b3 = (b3 << 4) | c[2];
                        let b2 = if c[0] == 0xa8 {
                            0
                        } else {
                            let n = (c[0].wrapping_add((transpose * 2) as u8).wrapping_sub(0x18) / 2) as usize;
                            b1 |= (CHIP_PERIODS[n] >> 8) as u8;
                            CHIP_PERIODS[n] as u8
                        };
                        pat[k * 16 + j * 4..k * 16 + j * 4 + 4].copy_from_slice(&[b1, b2, b3, c[3]]);
                    }
                }
                push!(out, pat.clone(), s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0x7c0 + self.tracks_size);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use super::CHIP_PERIODS;
    use crate::prowizard::tests::*;

    /// Packs the song as ChipTracker: a track per pattern and voice, no transpose.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = vec![0u8; 0x7c0];
        d[..4].copy_from_slice(b"Song");
        for i in 0..31 {
            let o = 0x16 + i * 30;
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d[o] = 1; // no name
            d[o + 22..o + 24].copy_from_slice(&s.0.to_be_bytes());
            d[o + 24] = s.1;
            d[o + 25] = s.2;
            d[o + 26..o + 28].copy_from_slice(&(s.3 * 2).to_be_bytes());
            d[o + 28..o + 30].copy_from_slice(&s.4.to_be_bytes());
        }
        d[0x3b8..0x3bc].copy_from_slice(b"KRIS");
        d[0x3bc] = song.positions.len() as u8;
        for (i, &p) in song.positions.iter().enumerate() {
            for v in 0..4 {
                d[0x3be + (i * 4 + v) * 2] = p * 4 + v as u8;
            }
        }
        for p in &song.patterns {
            for v in 0..4 {
                for row in p {
                    let n = row[v];
                    let note = if n.1 == 0 { 0xa8 } else { CHIP_PERIODS.iter().position(|&x| x == n.1).unwrap() as u8 * 2 + 0x18 };
                    d.extend([note, n.0, n.2, n.3]);
                }
            }
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn chiptracker() {
        let mut song = Song::new(3);
        // the check reads the fine tune and volume as one word, so no fine tune
        song.samples[1].1 = 0;
        let (f, m) = convert_any(&pack(&song)).expect("ChipTracker");
        assert_eq!(f, "ChipTracker");
        let mut want = song.module(0x7f);
        want[..20].copy_from_slice(b"Song\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(m, want);
    }
}
