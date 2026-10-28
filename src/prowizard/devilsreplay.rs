//! Devils Replay, ported from NostalgicPlayer's DevilsReplayFormat (ProWizard for
//! PC).
//!
//! A ProTracker header ("M.K."), then the size of the pattern data, the pattern
//! data, the size of a reference table and the table. A row of a pattern is a byte
//! whose bits tell which of the row's eight words (voice 0 note high word first)
//! are not zero, and the offset in the reference table where those words follow.

use super::core::*;

#[derive(Default)]
pub struct DevilsReplay {
    base: Base,
    positions: u8,
    restart: u8,
    pattern_size: u16,
    reference_size: u16,
    reference_table: Vec<u8>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(51, Box::new(DevilsReplay::default()))]
}

impl Format for DevilsReplay {
    fn name(&self) -> &'static str {
        "Devils Replay"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x440 {
            return false;
        }
        s.seek(0x438);
        if s.read_mark() != b"M.K." {
            return false;
        }
        s.seek(20);
        for _ in 0..31 {
            s.seek_cur(22);
            let size = (s.read_b_u16() as u32 * 2) as u16;
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = (s.read_b_u16() as u32 * 2) as u16;
            let loop_size = (s.read_b_u16() as u32 * 2) as u16;
            if !test_sample(size, loop_start, loop_size, volume, fine_tune) {
                return false;
            }
        }
        if self.base.find_highest_pattern_at(s, 0x3b8, 128) > 64 {
            return false;
        }
        // the pattern data and reference table sizes
        s.seek(0x43c);
        let t = s.read_b_u16() as i64;
        if t == 0 || t + 2 + 1084 > s.len() {
            return false;
        }
        if self.base.number_of_patterns as i64 * 192 + 4 != t {
            return false;
        }
        s.seek_cur(t);
        let t1 = s.read_b_u16() as i64;
        !(t1 == 0 || t + t1 + 2 + 1084 + 2 > s.len())
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x3b6);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        s.seek(0x43c);
        self.pattern_size = s.read_b_u16();
        s.seek_cur(self.pattern_size as i64);
        self.reference_size = s.read_b_u16();
        self.reference_table = vec![0; self.reference_size as usize];
        s.read_into(&mut self.reference_table);
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
        let mut p = vec![0u8; self.positions as usize];
        s.seek(0x3b8);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x43e);
        let end = 1084 + 2 + self.pattern_size as i64;
        let rt = &self.reference_table;
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 {
                if s.position() >= end {
                    break;
                }
                let filter = s.read_u8();
                let mut r = s.read_b_u16();
                // a bit per word of the row, voice 0's first word in bit 7
                for k in 0..8 {
                    if filter & (0x80 >> k) != 0 {
                        pat[j * 16 + k * 2] = *rt.get(r as usize)?;
                        pat[j * 16 + k * 2 + 1] = *rt.get(r.wrapping_add(1) as usize)?;
                        r = r.wrapping_add(2);
                    }
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(1084 + 2 + self.pattern_size as i64 + 2 + self.reference_size as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// The song as a Devils Replay module: each row's non-zero words go to the
    /// reference table in order.
    fn pack(song: &Song) -> Vec<u8> {
        let m = song.module(song.restart);
        let mut d = m[..1084].to_vec();
        let (mut pd, mut rt): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
        for p in &song.patterns {
            for row in p {
                let bytes: Vec<u8> = row.iter().flat_map(|&n| Song::note(n)).collect();
                let mut filter = 0u8;
                let at = rt.len() as u16;
                for k in 0..8 {
                    if bytes[k * 2] != 0 || bytes[k * 2 + 1] != 0 {
                        filter |= 0x80 >> k;
                        rt.extend(&bytes[k * 2..k * 2 + 2]);
                    }
                }
                pd.push(filter);
                pd.extend(at.to_be_bytes());
            }
        }
        pd.extend([0; 4]);
        d.extend((pd.len() as u16).to_be_bytes());
        d.extend(pd);
        d.extend((rt.len() as u16).to_be_bytes());
        d.extend(rt);
        d.extend(&m[1084 + song.patterns.len() * 1024..]);
        d
    }

    #[test]
    fn devils_replay() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("Devils Replay");
        assert_eq!(f, "Devils Replay");
        assert_eq!(m, song.module(song.restart));
    }
}
