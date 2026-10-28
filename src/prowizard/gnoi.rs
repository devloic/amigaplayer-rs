//! GnoiPacker, ported from NostalgicPlayer's GnoiPackerFormat (ProWizard for PC).
//!
//! A name, the position count (bit 7: a padding byte follows the list), restart,
//! sample and pattern counts, "-GD-", the samples (length, volume word, loop) and
//! the position list, then patterns of 3-byte notes: sample bit 4 and the note
//! number, sample bits 0-3 and the effect, the parameter.

use super::core::*;

#[derive(Default)]
pub struct Gnoi {
    base: Base,
    positions: u8,
    restart: u8,
    samples: u8,
    padding: bool,
    real_patterns: u16,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(53, Box::new(Gnoi::default()))]
}

impl Format for Gnoi {
    fn name(&self) -> &'static str {
        "GnoiPacker"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 28 {
            return false;
        }
        s.seek(22);
        let count = s.read_u8();
        if count == 0 || count > 31 {
            return false;
        }
        self.base.number_of_patterns = s.read_u8() as u16;
        if self.base.number_of_patterns == 0 || self.base.number_of_patterns > 64 {
            return false;
        }
        if s.read_mark() != b"-GD-" {
            return false;
        }
        for _ in 0..count {
            let size = (s.read_b_u16() as u32 * 2) as u16;
            let volume = s.read_b_u16() as u8;
            let loop_start = (s.read_b_u16() as u32 * 2) as u16;
            let loop_size = (s.read_b_u16() as u32 * 2) as u16;
            if !test_sample(size, loop_start, loop_size, volume, 0) {
                return false;
            }
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(20);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        self.samples = s.read_u8();
        self.padding = self.positions & 0x80 != 0;
        self.positions &= 0x7f;
        if self.padding {
            self.positions = self.positions.wrapping_sub(1);
        }
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0u8; 20];
        s.seek(0);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(28);
        let mut v = Vec::new();
        for _ in 0..self.samples {
            let length = s.read_b_u16();
            let volume = s.read_b_u16() as u8;
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune: 0 }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        // right after the samples
        let mut p = vec![0u8; self.positions as usize];
        s.read_into(&mut p);
        if self.padding {
            s.seek_cur(1);
        }
        self.real_patterns = self.base.number_of_patterns;
        self.base.find_highest_pattern(&p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 * 4 {
                let (mut b1, mut b2, mut b3) = (s.read_u8(), s.read_u8(), s.read_u8());
                // the Skizzo demo triggers its effects with 8xx; it is removed so the
                // module is not taken for a PC one
                if b2 & 0x0f == 0x08 {
                    b2 &= 0xf0;
                    b3 = 0;
                }
                pat[j * 4] = (b1 >> 3) & 0x10;
                pat[j * 4 + 2] = b2;
                pat[j * 4 + 3] = b3;
                b1 &= 0x7f;
                if b1 != 0 {
                    // a note past the table fails, as the C# index does
                    let p = PERIODS.get(b1 as usize - 1)?;
                    pat[j * 4] |= p[0];
                    pat[j * 4 + 1] = p[1];
                }
            }
            push!(out, pat, s);
        }
        // skip the patterns no position plays
        s.seek_cur((self.real_patterns as i64 - self.base.number_of_patterns as i64) * 768);
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// The song as a GnoiPacker module (no fine tunes), with an unused pattern
    /// after the played ones and a padded position list.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = vec![0u8; 20];
        let n = song.positions.len() as u8;
        d.extend([0x80 | (n + 1), song.restart, song.samples.len() as u8, song.patterns.len() as u8 + 1]);
        d.extend(b"-GD-");
        for s in &song.samples {
            for w in [s.0, s.2 as u16, s.3, s.4] {
                d.extend(w.to_be_bytes());
            }
        }
        d.extend(&song.positions);
        d.push(0);
        let table: Vec<u16> = super::PERIODS.iter().map(|p| (p[0] as u16) << 8 | p[1] as u16).collect();
        for p in &song.patterns {
            for row in p {
                for &(smp, per, eff, par) in row {
                    let n = if per == 0 { 0 } else { table.iter().position(|&x| x == per).unwrap() as u8 + 1 };
                    d.extend([(smp & 0x10) << 3 | n, (smp << 4) | eff, par]);
                }
            }
        }
        d.extend([0x55; 768]);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn gnoi_packer() {
        let mut song = Song::new(3);
        song.samples[1].1 = 0;
        let (f, m) = convert_any(&pack(&song)).expect("GnoiPacker");
        assert_eq!(f, "GnoiPacker");
        assert_eq!(m, song.module(song.restart));
    }
}
