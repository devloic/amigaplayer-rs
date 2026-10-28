//! Eureka Packer, ported from NostalgicPlayer's EurekaPackerFormat.
//!
//! A ProTracker header whose mark is the sample data offset, then a word offset
//! per pattern and voice; a track's rows are a whole note (high nibble 0/1), an
//! effect only (0x4x), a note only (0x8x) or a run of empty rows (0xc0 + n - 1).

use super::core::*;

#[derive(Default)]
pub struct Eureka {
    base: Base,
    positions: u8,
    restart: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(7, Box::new(Eureka::default()))]
}

impl Format for Eureka {
    fn name(&self) -> &'static str {
        "Eureka Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 1084 {
            return false;
        }
        s.seek(950);
        let len = s.read_u8();
        if len > 0x7f || len == 0 {
            return false;
        }
        // NTK byte, then the first two positions
        if s.read_u8() > 0x7f || s.read_u8() > 0x3f || s.read_u8() > 0x3f {
            return false;
        }
        // no mark is allowed
        s.seek(1080);
        let sample_offset = s.read_b_u32();
        if sample_offset == 0x4d2e4b2e || sample_offset == 0x534e542e {
            return false;
        }
        s.seek(1464);
        let mark = s.read_b_u32();
        if mark == 0x4d544e00 || mark == 0x49543130 {
            return false;
        }
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
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
        if sample_offset.wrapping_add(samples_size) as i64 > s.len() + MAX_MISSING {
            return false;
        }
        if self.base.find_highest_pattern_at(s, 952, len as usize) > 64 {
            return false;
        }
        // the track offsets must be in ascending order
        s.seek(1084);
        let mut prev = 0;
        for _ in 0..self.base.number_of_patterns * 4 {
            let t = s.read_b_u16();
            if t < prev {
                return false;
            }
            prev = t;
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(950);
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
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut v = vec![0u8; self.positions as usize];
        s.seek(952);
        s.read_into(&mut v);
        Some(v)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let n = self.base.number_of_patterns as usize;
        let mut offs = vec![0u16; n * 4];
        s.seek(1084);
        s.read_array_b_u16(&mut offs);
        let mut out = Vec::new();
        for i in 0..n {
            let mut pat = vec![0u8; 1024];
            for j in 0..4 {
                s.seek(offs[i * 4 + j] as i64);
                let mut w = j * 4;
                let mut k = 0;
                while k < 64 {
                    let b = s.read_u8();
                    match b & 0xf0 {
                        0x00 | 0x10 => {
                            // a whole note
                            pat[w] = b;
                            pat[w + 1] = s.read_u8();
                            pat[w + 2] = s.read_u8();
                            pat[w + 3] = s.read_u8();
                            w += 16;
                        }
                        0x40 => {
                            // only an effect
                            pat[w + 2] = b & 0x0f;
                            pat[w + 3] = s.read_u8();
                            w += 16;
                        }
                        0x80 => {
                            // only a note
                            pat[w] = s.read_u8();
                            pat[w + 1] = s.read_u8();
                            pat[w + 2] = (b & 0x0f) << 4;
                            w += 16;
                        }
                        _ => {
                            // empty rows
                            let t = b.wrapping_sub(0xc0) as usize;
                            w += (t + 1) * 16;
                            k += t;
                        }
                    }
                    k += 1;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(1080);
        let off = s.read_b_u32();
        s.seek(off as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// Packs the song as Eureka does; samples below 16, so a note-only row is
    /// one with no effect.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = song.module(song.restart)[..1084].to_vec();
        let np = song.patterns.len();
        let to = d.len();
        d.resize(to + np * 8, 0);
        let mut offs = Vec::new();
        for p in &song.patterns {
            for v in 0..4 {
                offs.push(d.len() as u16);
                let mut empty = 0;
                for (r, row) in p.iter().enumerate() {
                    let n = row[v];
                    if n == (0, 0, 0, 0) {
                        empty += 1;
                    } else {
                        if empty > 0 {
                            d.push(0xc0 + empty - 1);
                            empty = 0;
                        }
                        let b = Song::note(n);
                        if n.1 == 0 && n.0 == 0 {
                            d.extend([0x40 | n.2, n.3]);
                        } else if n.2 == 0 && n.3 == 0 {
                            d.extend([0x80 | (n.0 & 0x0f), b[0], b[1]]);
                        } else {
                            d.extend(b);
                        }
                    }
                    if r == 63 && empty > 0 {
                        d.push(0xc0 + empty - 1);
                    }
                }
            }
        }
        for (i, o) in offs.iter().enumerate() {
            d[to + i * 2..to + i * 2 + 2].copy_from_slice(&o.to_be_bytes());
        }
        let so = d.len() as u32;
        d[1080..1084].copy_from_slice(&so.to_be_bytes());
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn eureka() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("Eureka Packer");
        assert_eq!(f, "Eureka Packer");
        assert_eq!(m, song.module(song.restart));
    }
}
