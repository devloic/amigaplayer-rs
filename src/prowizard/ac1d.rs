//! AC1D Packer, ported from NostalgicPlayer's AC1DPackerFormat.
//!
//! The header is the song length, restart, the 0xAC1D mark, the sample data
//! offset and the sample table; then the pattern addresses at 0x100 and the
//! position list at 0x300. A pattern holds its 4 tracks one after the other, each
//! row a note number byte (bit 6 for samples 16+) and the sample/effect bytes, or
//! a byte giving a run of empty rows.

use super::core::*;

#[derive(Default)]
pub struct Ac1d {
    base: Base,
    positions: u8,
    restart: u8,
    sample_data_offset: u32,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(1, Box::new(Ac1d::default()))]
}

impl Format for Ac1d {
    fn name(&self) -> &'static str {
        "AC1D Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        self.base.number_of_patterns = 0;
        if s.len() < 896 {
            return false;
        }
        s.seek(0);
        let pos = s.read_u8();
        let restart = s.read_u8();
        let id = s.read_b_u16();
        if id != 0xac1d && id != 0xd1ca {
            return false;
        }
        if pos == 0 || pos > 127 || restart > 0x7f {
            return false;
        }
        let sample_offset = s.read_b_u32();
        if sample_offset == 0 || sample_offset & 1 != 0 {
            return false;
        }
        let mut samples_size = 0u32;
        for _ in 0..31 {
            let len = s.read_b_u16() as u32;
            if len >= 0x8000 {
                return false;
            }
            samples_size += len * 2;
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            s.seek_cur(4);
        }
        if sample_offset.wrapping_add(samples_size) as i64 > s.len() + MAX_MISSING {
            return false;
        }
        let mut list = [0u8; 128];
        s.seek(0x300);
        if s.read_into(&mut list) != 128 {
            return false;
        }
        self.base.find_highest_pattern(&list) <= 64
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        // NostalgicPlayer rewinds the stream before converting
        s.seek(0);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        s.seek_cur(2);
        self.sample_data_offset = s.read_b_u32();
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let mut v = Vec::new();
        for _ in 0..31 {
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut list = vec![0u8; 128];
        s.seek(0x300);
        s.read_into(&mut list);
        list.truncate(self.positions as usize);
        Some(list)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut addr = [0u32; 128];
        s.seek(0x100);
        s.read_array_b_u32(&mut addr);
        let first = addr[0];
        let mut out = Vec::new();
        for i in 0..self.base.number_of_patterns as usize {
            let mut pat = vec![0u8; 1024];
            s.seek(addr[i].wrapping_sub(first).wrapping_add(0x380 + 12) as i64);
            for j in 0..4 {
                let mut w = j * 4;
                let mut k = 0;
                while k < 64 {
                    let mut b = s.read_u8();
                    if b >= 0x81 {
                        // a run of empty rows
                        b -= 0x80;
                        w += b as usize * 16;
                        k += b as usize;
                        continue;
                    }
                    if b == 0x7f || b == 0x3f {
                        // no note: sample 16 (0x7f) or none, then the effect
                        pat[w] = if b == 0x7f { 0x10 } else { 0 };
                        pat[w + 1] = 0;
                        pat[w + 2] = s.read_u8();
                        pat[w + 3] = s.read_u8();
                        w += 16;
                        k += 1;
                        continue;
                    }
                    if b >= 0x4c {
                        pat[w] = 0x10;
                        b -= 0x40;
                    }
                    let n = b.wrapping_sub(0x0c) as usize;
                    pat[w] |= PERIODS[n][0];
                    pat[w + 1] = PERIODS[n][1];
                    let b = s.read_u8();
                    if b & 0x0f == 0x07 {
                        // sample only, no effect
                        pat[w + 2] = b & 0xf0;
                    } else {
                        pat[w + 2] = b;
                        pat[w + 3] = s.read_u8();
                    }
                    w += 16;
                    k += 1;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_data_offset as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    /// Packs the song as AC1D does; the note must be in the period table.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = vec![song.positions.len() as u8, song.restart, 0xac, 0x1d, 0, 0, 0, 0];
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(s.0.to_be_bytes());
            d.extend([s.1, s.2]);
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
        }
        d.resize(0x380, 0);
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d[0x300..0x380].copy_from_slice(&pl);
        // the first pattern starts 12 bytes after 0x380
        d.resize(0x38c, 0);
        let mut addr = Vec::new();
        for p in &song.patterns {
            addr.push(d.len() as u32);
            for v in 0..4 {
                let mut empty = 0u8;
                for (r, row) in p.iter().enumerate() {
                    let n = row[v];
                    if n == (0, 0, 0, 0) {
                        empty += 1;
                        if r < 63 {
                            continue;
                        }
                    }
                    if empty > 0 {
                        d.push(0x80 + empty);
                        empty = 0;
                        if n == (0, 0, 0, 0) {
                            continue;
                        }
                    }
                    if n.1 == 0 {
                        d.push(if n.0 >= 0x10 { 0x7f } else { 0x3f });
                        d.extend([(n.0 << 4) | n.2, n.3]);
                        continue;
                    }
                    let idx = PERIODS.iter().position(|p| (p[0] as u16) << 8 | p[1] as u16 == n.1).unwrap() as u8;
                    d.push(idx + 0x0c + if n.0 >= 0x10 { 0x40 } else { 0 });
                    if n.2 == 0 && n.3 == 0 {
                        d.push((n.0 << 4) | 0x07);
                    } else {
                        d.extend([(n.0 << 4) | n.2, n.3]);
                    }
                }
            }
        }
        while d.len() % 2 != 0 {
            d.push(0);
        }
        let so = d.len() as u32;
        d[4..8].copy_from_slice(&so.to_be_bytes());
        for (i, a) in addr.iter().enumerate() {
            d[0x100 + i * 4..0x104 + i * 4].copy_from_slice(&a.to_be_bytes());
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn ac1d() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("AC1D");
        assert_eq!(f, "AC1D Packer");
        assert_eq!(m, song.module(song.restart));
    }
}
