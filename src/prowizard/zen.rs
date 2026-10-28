//! Zen Packer, ported from NostalgicPlayer's ZenPackerFormat (after ProWizard for
//! Amiga).
//!
//! A replay-ready dump with absolute addresses: the address of the position table,
//! the number of patterns and positions, 16-byte sample entries (fine tune * 0x48,
//! volume, length, loop length, start and loop addresses), then patterns of 4-byte
//! entries (row, note number * 2 with the sample's high bit, sample and effect,
//! parameter), each ended by row 0xff, then the position table of pattern
//! addresses ended by 0xffffffff.

use super::core::*;

#[derive(Default)]
pub struct Zen {
    base: Base,
    positions: u8,
    position_list: Vec<u8>,
    pattern_offsets: Vec<u32>,
    sample_addresses: Vec<u32>,
    sample_loop_addresses: Vec<u32>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(48, Box::new(Zen::default()))]
}

/// FindEndOfTables: where 0xffffffff is, from 0x1f6 on in steps of two; 0 if not.
fn find_end_of_tables(s: &mut Stream) -> u32 {
    let mut t = 0x1f6u32;
    loop {
        s.seek(t as i64);
        if s.read_b_u32() == 0xffffffff {
            return t;
        }
        t += 2;
        if t as i64 > s.len() {
            return 0;
        }
    }
}

impl Zen {
    /// CreateOffsetTables: pattern and sample addresses as offsets in the file.
    fn create_offset_tables(&mut self, s: &mut Stream, table: u32) {
        // FindBaseOffset: the lowest pattern address is at 0x1f6
        s.seek(table as i64);
        let mut base = s.read_b_u32();
        for _ in 1..self.positions {
            base = base.min(s.read_b_u32());
        }
        let base = base.wrapping_sub(0x1f6);
        s.seek(table as i64);
        self.pattern_offsets = (0..self.positions).map(|_| s.read_b_u32().wrapping_sub(base)).collect();
        s.seek(6);
        self.sample_addresses = vec![0; 31];
        self.sample_loop_addresses = vec![0; 31];
        for i in 0..31 {
            s.seek_cur(8);
            self.sample_addresses[i] = s.read_b_u32().wrapping_sub(base);
            self.sample_loop_addresses[i] = s.read_b_u32().wrapping_sub(base);
        }
    }
}

impl Format for Zen {
    fn name(&self) -> &'static str {
        "Zen Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x1f6 {
            return false;
        }
        // the position table's address
        s.seek(0);
        let mut t = s.read_b_u32();
        if t & 1 != 0 || t > 0x200000 {
            return false;
        }
        self.base.number_of_patterns = s.read_u8() as u16;
        if self.base.number_of_patterns > 0x3f {
            return false;
        }
        self.base.number_of_patterns += 1;
        let len = s.read_u8();
        if len > 0x7f {
            return false;
        }
        // the first sample's address follows the position table
        t = t.wrapping_add(len as u32 * 4 + 4);
        s.seek(14);
        if s.read_b_u32() != t {
            return false;
        }
        s.seek(6);
        let mut samples_size = 0u32;
        let mut previous = 0u32;
        for _ in 0..31 {
            if s.read_b_u16() % 0x48 != 0 || s.read_b_u16() > 0x40 {
                return false;
            }
            let size = s.read_b_u16();
            if size >= 0x8000 {
                return false;
            }
            if size == 0 {
                // the loop length
                if s.read_b_u16() != 1 {
                    return false;
                }
            } else {
                samples_size += size as u32 * 2;
                s.seek_cur(2);
            }
            let a = s.read_b_u32();
            if s.read_b_u32() < a || previous > a {
                return false;
            }
            previous = a;
        }
        let end = find_end_of_tables(s);
        if end == 0 {
            return false;
        }
        // the end mark found, the last pattern's last row
        let table = end.wrapping_sub(len as u32 * 4);
        s.seek(table.wrapping_sub(4) as i64);
        if s.read_b_u32() != 0xff000000 {
            return false;
        }
        if end.wrapping_add(4).wrapping_add(samples_size) as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the row numbers in the first pattern go up
        s.seek(0x1f6);
        let mut line = s.read_u8();
        if line != 0xff {
            s.seek_cur(3);
            loop {
                let r = s.read_u8();
                if r == 0xff {
                    break;
                }
                if r <= line {
                    return false;
                }
                line = r;
                s.seek_cur(3);
            }
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(5);
        self.positions = s.read_u8();
        let end = find_end_of_tables(s);
        let table = end.wrapping_sub(self.positions as u32 * 4);
        let npat = self.base.number_of_patterns;
        self.create_offset_tables(s, table);
        let po = &self.pattern_offsets;
        self.position_list = self.base.build_position_list(self.positions as usize, |p| po[p]).0;
        // as many distinct patterns as the header says
        self.base.number_of_patterns == npat
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(6);
        let mut v = Vec::new();
        for i in 0..31 {
            let fine_tune = (s.read_b_u16() / 0x48) as u8;
            let volume = s.read_b_u16() as u8;
            let length = s.read_b_u16();
            let loop_length = s.read_b_u16();
            s.seek_cur(8);
            let loop_start = (self.sample_loop_addresses[i].wrapping_sub(self.sample_addresses[i]) / 2) as u16;
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list.clone())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions as usize {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                s.seek(self.pattern_offsets[i] as i64);
                loop {
                    let line = s.read_u8() as usize * 4;
                    let mut t = s.read_u8();
                    if t & 1 != 0 {
                        pat[line] = 0x10;
                        t &= 0xfe;
                    }
                    if t != 0 {
                        let p = PERIODS.get(((t - 2) / 2) as usize)?;
                        pat[line] |= p[0];
                        pat[line + 1] = p[1];
                    }
                    pat[line + 2] = s.read_u8();
                    pat[line + 3] = s.read_u8();
                    // (the C# loops forever at the end of the data)
                    if line == 0xff * 4 || s.eof {
                        break;
                    }
                }
                push!(out, pat, s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_addresses[0] as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::*;
    use crate::prowizard::tests::*;

    /// The song as Zen, its addresses those in the file (patterns at 0x1f6). Each
    /// pattern lists its non-empty notes by cell number and always ends with cell
    /// 255 (row 63, voice 4).
    fn pack(song: &Song) -> Vec<u8> {
        let mut pats = Vec::new();
        let mut addrs = Vec::new();
        for p in &song.patterns {
            addrs.push(0x1f6 + pats.len() as u32);
            for (r, row) in p.iter().enumerate() {
                for (v, &(smp, per, fx, par)) in row.iter().enumerate() {
                    let cell = r * 4 + v;
                    if (smp, per, fx, par) == (0, 0, 0, 0) && cell != 255 {
                        continue;
                    }
                    let n = match per {
                        0 => 0,
                        _ => PERIODS.iter().position(|x| ((x[0] as u16) << 8 | x[1] as u16) == per).unwrap() as u8 * 2 + 2,
                    };
                    pats.extend([cell as u8, n | (smp >> 4), (smp << 4) | fx, par]);
                }
            }
        }
        let table = 0x1f6 + pats.len() as u32;
        let npos = song.positions.len() as u32;
        let mut a = table + npos * 4 + 4;
        let mut d = table.to_be_bytes().to_vec();
        d.extend([song.patterns.len() as u8 - 1, npos as u8]);
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend((s.1 as u16 * 0x48).to_be_bytes());
            d.extend((s.2 as u16).to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend(s.4.to_be_bytes());
            d.extend(a.to_be_bytes());
            d.extend((a + s.3 as u32 * 2).to_be_bytes());
            a += s.0 as u32 * 2;
        }
        d.extend(pats);
        for &p in &song.positions {
            d.extend(addrs[p as usize].to_be_bytes());
        }
        d.extend([0xff; 4]);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn zen() {
        // the last pattern's last note is the end mark the check looks for: empty
        let mut song = Song::new(3);
        song.patterns[2][63][3] = (0, 0, 0, 0);
        let (f, m) = convert_any(&pack(&song)).expect("not recognised");
        assert_eq!(f, "Zen Packer");
        assert_eq!(m, song.module(0x7f));
    }
}
