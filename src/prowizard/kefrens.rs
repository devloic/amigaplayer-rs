//! Kefrens Sound Machine, ported from NostalgicPlayer's
//! KefrensSoundMachineFormat.
//!
//! "M." and a 13-byte title, 15 sample entries of 32 bytes (sizes and loops in
//! bytes), 4 track numbers per position at 0x200 (ended by 0xff), and from 0x600
//! tracks of 64 3-byte rows: note number, sample/effect (D is volume slide, A in
//! ProTracker) and parameter. Track 0 is empty.

use super::core::*;

#[derive(Default)]
pub struct Kefrens {
    base: Base,
    positions: usize,
    track_table: Vec<u8>,
    position_list: Vec<u8>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(12, Box::new(Kefrens::default()))]
}

impl Format for Kefrens {
    fn name(&self) -> &'static str {
        "Kefrens Sound Machine"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x600 {
            return false;
        }
        s.seek(0);
        if s.read_b_u16() != 0x4d2e {
            return false;
        }
        s.seek(32);
        let mut samples_size = 0u32;
        let mut offset = 0u32;
        for _ in 0..15 {
            s.seek_cur(16);
            // sample offsets ascend
            let t = s.read_b_u32();
            if t == 0 || t <= offset {
                return false;
            }
            offset = t;
            let len = s.read_b_u16();
            if len == 0 || len & 1 != 0 {
                return false;
            }
            samples_size += len as u32;
            if s.read_u8() > 0x40 {
                return false;
            }
            s.seek_cur(9);
        }
        // the position list (KSM has up to 256 positions, 128 are supported)
        loop {
            if s.position() > 0x400 {
                return false;
            }
            if s.read_u8() == 0xff {
                break;
            }
            s.seek_cur(3);
        }
        // track 0 is zeros
        s.seek(0x600);
        let mut t = 0u32;
        for _ in 0..48 {
            t = t.wrapping_add(s.read_b_u32());
        }
        if t != 0 {
            return false;
        }
        for _ in 0..48 {
            if s.read_u8() > 0x24 {
                return false;
            }
            s.seek_cur(2);
        }
        s.seek(0x30);
        s.read_b_u32().wrapping_add(samples_size) as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x200);
        self.track_table = vec![0; 0x200];
        s.read_into(&mut self.track_table);
        self.position_list = vec![0; 128];
        let mut used: Vec<u32> = Vec::new();
        self.base.number_of_patterns = 0;
        self.positions = 0;
        while self.positions < 128 {
            let t = &self.track_table[self.positions * 4..self.positions * 4 + 4];
            if t[0] == 0xff {
                break;
            }
            let k = u32::from_be_bytes([t[0], t[1], t[2], t[3]]);
            match used.iter().position(|&u| u == k) {
                Some(j) => self.position_list[self.positions] = j as u8,
                None => {
                    self.position_list[self.positions] = self.base.number_of_patterns as u8;
                    self.base.number_of_patterns += 1;
                    used.push(k);
                }
            }
            self.positions += 1;
        }
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0u8; 20];
        s.seek(2);
        s.read_into(&mut n[..13]);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(32);
        let mut v = Vec::new();
        for _ in 0..15 {
            s.seek_cur(20);
            let length = s.read_b_u16() / 2;
            let volume = s.read_u8();
            s.seek_cur(1);
            let mut loop_start = s.read_b_u16();
            let mut loop_length = 1;
            if loop_start != 0 {
                loop_start /= 2;
                loop_length = length.wrapping_sub(loop_start);
            }
            s.seek_cur(6);
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune: 0 }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list[..self.positions].to_vec())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                for j in 0..4 {
                    s.seek(0x600 + self.track_table[i * 4 + j] as i64 * 192);
                    for k in 0..64 {
                        let o = k * 16 + j * 4;
                        let n = s.read_u8();
                        if n != 0 {
                            pat[o] = PERIODS[n as usize - 1][0];
                            pat[o + 1] = PERIODS[n as usize - 1][1];
                        }
                        let mut b1 = s.read_u8();
                        let b2 = s.read_u8();
                        // effect D to A
                        if b1 & 0x0f == 0x0d {
                            b1 = (b1 & 0xf0) | 0x0a;
                        }
                        pat[o + 2] = b1;
                        pat[o + 3] = b2;
                    }
                }
                push!(out, pat, s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0x30);
        let off = s.read_b_u32();
        s.seek(off as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    /// Packs the song as KSM does. KSM has 15 samples, all of them used, no fine
    /// tune, and loops to the end of the sample.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = vec![0u8; 0x600];
        d[..2].copy_from_slice(b"M.");
        d[2..9].copy_from_slice(b"Kefrens");
        let mut off = 0x600 + (song.patterns.len() * 4 + 1) as u32 * 192;
        for i in 0..15 {
            let s = song.samples[i];
            let o = 32 + i * 32;
            d[o + 16..o + 20].copy_from_slice(&off.to_be_bytes());
            d[o + 20..o + 22].copy_from_slice(&(s.0 * 2).to_be_bytes());
            d[o + 22] = s.2;
            d[o + 24..o + 26].copy_from_slice(&(s.3 * 2).to_be_bytes());
            off += s.0 as u32 * 2;
        }
        for (i, &p) in song.positions.iter().enumerate() {
            for v in 0..4 {
                d[0x200 + i * 4 + v] = p * 4 + v as u8 + 1;
            }
        }
        d[0x200 + song.positions.len() * 4] = 0xff;
        d.extend([0u8; 192]);
        for p in &song.patterns {
            for v in 0..4 {
                for row in p {
                    let n = row[v];
                    let note = if n.1 == 0 {
                        0
                    } else {
                        PERIODS.iter().position(|p| (p[0] as u16) << 8 | p[1] as u16 == n.1).unwrap() as u8 + 1
                    };
                    d.extend([note, (n.0 << 4) | n.2, n.3]);
                }
            }
        }
        for i in 0..15 {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn kefrens() {
        let mut song = Song::new(3);
        song.samples = (0..15).map(|i| (4 + i as u16, 0, 40, if i % 2 == 1 { 2 } else { 0 }, if i % 2 == 1 { 2 + i as u16 } else { 1 })).collect();
        let (f, m) = convert_any(&pack(&song)).expect("Kefrens Sound Machine");
        assert_eq!(f, "Kefrens Sound Machine");
        let mut want = song.module(0x7f);
        want[..7].copy_from_slice(b"Kefrens");
        assert_eq!(m, want);
    }
}
