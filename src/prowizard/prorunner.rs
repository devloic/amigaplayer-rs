//! ProRunner 1, Hornet Packer and ProRunner 2, ported from NostalgicPlayer's
//! ProRunner1Format, HornetPackerFormat (a ProRunner 1 with "HRT!" and shorter
//! sample names) and ProRunner2Format.
//!
//! ProRunner 1 and Hornet keep ProTracker's layout with notes as (sample, note
//! number, effect, parameter) bytes, Hornet's numbers doubled. ProRunner 2 packs
//! the patterns: an empty note, "the voice's last note again", or three bytes.

use super::core::*;

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![
        (31, Box::new(ProRunner1 { hornet: false, ..Default::default() })),
        (11, Box::new(ProRunner1 { hornet: true, ..Default::default() })),
        (32, Box::new(ProRunner2::default())),
    ]
}

#[derive(Default)]
pub struct ProRunner1 {
    base: Base,
    hornet: bool,
    positions: u8,
    restart: u8,
    real_patterns: u16,
}

impl ProRunner1 {
    fn check_pr1(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x43c {
            return false;
        }
        // the sample table
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
            let len = s.read_b_u16();
            if len > 0x8000 {
                return false;
            }
            samples_size = samples_size.wrapping_add(len as u32 * 2);
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
        // the position list: the highest pattern
        s.seek_cur(2);
        let mut n = 0u16;
        for _ in 0..128 {
            let t = s.read_u8();
            if t > 0x3f {
                return false;
            }
            n = n.max(t as u16);
        }
        self.base.number_of_patterns = n + 1;
        let biggest = self.base.number_of_patterns as u32 * 256;
        if 0x43c + biggest as i64 * 4 + samples_size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the pattern data
        s.seek(0x43c);
        let mut got_notes = false;
        for _ in 0..biggest {
            let t = s.read_b_u32();
            if t != 0 {
                // the unused nibble must be zero
                if t & 0x0000f000 != 0 {
                    return false;
                }
                let note = (t & 0x00ff0000) >> 16;
                if note > 0x24 {
                    return false;
                }
                if note > 0 {
                    got_notes = true;
                }
                if (t & 0xff000000) >> 24 > 0x1f {
                    return false;
                }
            }
        }
        got_notes
    }

    fn check_hornet(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x43c {
            return false;
        }
        s.seek(0x438);
        if s.read_mark() != b"HRT!" {
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
            samples_size = samples_size.wrapping_add(len as u32 * 2);
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            let a = s.read_b_u16() as i64;
            let b = s.read_b_u16() as i64;
            if len != 0 {
                if len as i64 - (a + b) < -0x10 {
                    return false;
                }
            } else if a + b > 2 {
                return false;
            }
        }
        s.seek(0x3b6);
        let n = s.read_u8();
        if n > 0x7f || n == 0 {
            return false;
        }
        // NoiseTracker byte
        if s.read_u8() > 0x7f {
            return false;
        }
        if self.base.find_highest_pattern_at(s, 0x3b8, 128) > 64 {
            return false;
        }
        if 0x43c + self.base.number_of_patterns as i64 * 1024 + samples_size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the first 5 patterns
        let biggest = self.base.number_of_patterns as u32 * 256;
        s.seek(0x43c);
        let mut got_notes = false;
        for _ in 0..biggest.min(0x500) {
            let t = s.read_b_u32();
            if t != 0 {
                if t & 0x0000f000 != 0 {
                    return false;
                }
                if (t & 0x00ff0000) >> 16 > 0x48 {
                    return false;
                }
                if (t & 0xff000000) >> 24 > 0x3e {
                    return false;
                }
                got_notes = true;
            }
        }
        got_notes
    }
}

impl Format for ProRunner1 {
    fn name(&self) -> &'static str {
        if self.hornet { "Hornet Packer" } else { "ProRunner 1" }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if self.hornet { self.check_hornet(s) } else { self.check_pr1(s) }
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(950);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        // unused patterns are not saved
        self.real_patterns = self.base.number_of_patterns;
        self.base.find_highest_pattern_at(s, 0x3b8, self.positions as usize);
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0; 20];
        s.seek(0);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        // they follow the name
        let mut v = Vec::new();
        for _ in 0..31 {
            let mut name = vec![0; 22];
            if self.hornet {
                s.read_into(&mut name[..18]);
                s.seek_cur(4);
            } else {
                s.read_into(&mut name);
            }
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
        let mut p = vec![0; self.positions as usize];
        s.seek(952);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        // GetAllPatterns
        let div = if self.hornet { 2 } else { 1 };
        s.seek(0x43c);
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 * 4 {
                let mut t = s.read_u8() / div;
                if t >= 0x10 {
                    pat[j * 4] = 0x10;
                    t -= 0x10;
                }
                pat[j * 4 + 2] = t << 4;
                let t = s.read_u8() / div;
                if t != 0 {
                    let p = PERIODS.get(t as usize - 1)?;
                    pat[j * 4] |= p[0];
                    pat[j * 4 + 1] = p[1];
                }
                pat[j * 4 + 2] |= s.read_u8();
                pat[j * 4 + 3] = s.read_u8();
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0x43c + self.real_patterns as i64 * 1024);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[derive(Default)]
pub struct ProRunner2 {
    base: Base,
    positions: u8,
    restart: u8,
    pattern_offsets: Vec<u16>,
}

impl Format for ProRunner2 {
    fn name(&self) -> &'static str {
        "ProRunner 2"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x284 {
            return false;
        }
        s.seek(8);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            let len = s.read_b_u16();
            if len > 0x8000 {
                return false;
            }
            samples_size = samples_size.wrapping_add(len as u32 * 2);
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
        // the module length
        s.seek(4);
        let sample_offset = s.read_b_u32();
        if sample_offset as i64 + samples_size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        s.seek(0x100);
        let n = s.read_u8();
        if n > 0x7f || n == 0 {
            return false;
        }
        // NoiseTracker byte
        let t = s.read_u8();
        if t > 0x7f && t != 0xff {
            return false;
        }
        let mut highest = 0u16;
        for _ in 0..n {
            let t = s.read_u8();
            if t > 0x3f {
                return false;
            }
            highest = highest.max(t as u16);
        }
        self.base.number_of_patterns = highest + 1;
        // the pattern offsets: rising, and before the samples
        s.seek(0x282);
        if s.read_b_u16() != 0 {
            return false;
        }
        for _ in 0..self.base.number_of_patterns.saturating_sub(1) {
            s.seek_cur(-2);
            let a = s.read_b_u16();
            let b = s.read_b_u16();
            if b as u32 >= sample_offset || a >= b {
                return false;
            }
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x100);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        self.pattern_offsets = vec![0; self.base.number_of_patterns as usize];
        s.seek(0x282);
        s.read_array_b_u16(&mut self.pattern_offsets);
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(8);
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
        let mut p = vec![0; self.positions as usize];
        s.seek(0x102);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart & 0x7f
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        // each voice's last note, kept across patterns
        let mut prev = [[0u8; 4]; 4];
        for i in 0..self.base.number_of_patterns as usize {
            let mut pat = vec![0u8; 1024];
            s.seek(0x302 + self.pattern_offsets[i] as i64);
            for j in 0..64 {
                for k in 0..4 {
                    let o = j * 16 + k * 4;
                    let mut t = s.read_u8();
                    // empty note
                    if t == 0x80 {
                        continue;
                    }
                    // the voice's last note again
                    if t == 0xc0 {
                        pat[o..o + 4].copy_from_slice(&prev[k]);
                        continue;
                    }
                    let (mut b1, mut b2, mut b3) = (0u8, 0u8, 0u8);
                    if t & 1 != 0 {
                        // low bit of the sample number
                        b3 = 0x10;
                        t &= 0xfe;
                    }
                    if t != 0 {
                        let p = PERIODS.get((t.wrapping_sub(2) / 2) as usize)?;
                        b1 = p[0];
                        b2 = p[1];
                    }
                    let t1 = s.read_u8();
                    let mut t = (t1 & 0xf0) >> 3;
                    if t >= 0x10 {
                        // high bit of the sample number
                        b1 |= 0x10;
                        t -= 0x10;
                    }
                    b3 |= t << 4;
                    b3 |= t1 & 0x0f;
                    let b4 = s.read_u8();
                    pat[o..o + 4].copy_from_slice(&[b1, b2, b3, b4]);
                    if b1 != 0 || b2 != 0 || b3 != 0 || b4 != 0 {
                        prev[k] = [b1, b2, b3, b4];
                    }
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(4);
        let off = s.read_b_u32();
        s.seek(off as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    fn note_number(period: u16) -> u8 {
        if period == 0 {
            return 0;
        }
        super::PERIODS.iter().position(|x| ((x[0] as u16) << 8 | x[1] as u16) == period).unwrap() as u8 + 1
    }

    /// ProRunner 1 (`hornet` false) or Hornet: the module with its notes as bytes.
    fn pack_pr1(song: &Song, hornet: bool) -> Vec<u8> {
        let mut d = song.module(song.restart);
        let m = if hornet { 2 } else { 1 };
        if hornet {
            d[0x438..0x43c].copy_from_slice(b"HRT!");
        } else {
            d[0x438..0x43c].copy_from_slice(b"SNT.");
        }
        for p in 0..song.patterns.len() {
            for (r, row) in song.patterns[p].iter().enumerate() {
                for (v, &(smp, per, fx, par)) in row.iter().enumerate() {
                    let o = 0x43c + p * 1024 + r * 16 + v * 4;
                    d[o..o + 4].copy_from_slice(&[smp * m, note_number(per) * m, fx, par]);
                }
            }
        }
        d
    }

    /// ProRunner 2: every note packed, empty ones as 0x80, a repeat of the voice's
    /// last note as 0xc0.
    fn pack_pr2(song: &Song) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(b"SNT!");
        let mut pats = Vec::new();
        let mut offs = Vec::new();
        let mut prev = [(0u8, 0u16, 0u8, 0u8); 4];
        for p in &song.patterns {
            offs.push(pats.len() as u16);
            for row in p {
                for (v, &n) in row.iter().enumerate() {
                    if n == (0, 0, 0, 0) {
                        pats.push(0x80);
                        continue;
                    }
                    if n == prev[v] {
                        pats.push(0xc0);
                        continue;
                    }
                    prev[v] = n;
                    let (smp, per, fx, par) = n;
                    // the sample's bit 0 in the note byte, bits 1-4 in the effect byte
                    pats.extend([note_number(per) * 2 | (smp & 1), ((smp >> 1) << 4) | fx, par]);
                }
            }
        }
        let sample_offset = 0x302 + pats.len() as u32;
        d.extend(sample_offset.to_be_bytes());
        for i in 0..31 {
            let (len, ft, vol, ls, ll) = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(len.to_be_bytes());
            d.extend([ft, vol]);
            d.extend(ls.to_be_bytes());
            d.extend(ll.to_be_bytes());
        }
        d.resize(0x100, 0);
        d.push(song.positions.len() as u8);
        d.push(song.restart);
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d.extend(pl);
        d.resize(0x282, 0);
        for o in &offs {
            d.extend(o.to_be_bytes());
        }
        d.resize(0x302, 0);
        d.extend(pats);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn prorunner() {
        let song = Song::new(3);
        for (hornet, name) in [(false, "ProRunner 1"), (true, "Hornet Packer")] {
            let (f, m) = convert_any(&pack_pr1(&song, hornet)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(song.restart));
        }
        let (f, m) = convert_any(&pack_pr2(&song)).expect("ProRunner 2");
        assert_eq!(f, "ProRunner 2");
        assert_eq!(m, song.module(song.restart));
    }
}
