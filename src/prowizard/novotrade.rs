//! NovoTrade Packer, ported from NostalgicPlayer's NovoTradePackerFormat (after
//! ProWizard for PC).
//!
//! "MODU", the title, the sample table (only the samples used, each with its
//! number), the position list in words and the pattern offsets; a "BODY" chunk of
//! patterns, each row a word whose bits say which voices have a ProTracker note
//! (0x80 ends the pattern); and a "SAMP" chunk of sample data.

use super::core::*;

#[derive(Default)]
pub struct NovoTrade {
    base: Base,
    body_offset: u16,
    samples_offset: u16,
    number_of_samples: u16,
    positions: u8,
    pattern_offsets: Vec<u16>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(61, Box::new(NovoTrade::default()))]
}

impl Format for NovoTrade {
    fn name(&self) -> &'static str {
        "NovoTrade Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 38 {
            return false;
        }
        s.seek(0);
        if s.read_mark() != b"MODU" {
            return false;
        }
        s.seek(20);
        let offset1 = s.read_b_u16().wrapping_add(4);
        if offset1 as i64 + 4 > s.len() {
            return false;
        }
        s.seek(26);
        self.base.number_of_patterns = s.read_b_u16();
        let offset2 = s.read_b_u16().wrapping_add(offset1).wrapping_add(4);
        if offset2 as i64 + 2 > s.len() {
            return false;
        }
        s.seek(offset1 as i64);
        if s.read_mark() != b"BODY" {
            return false;
        }
        s.seek(offset2 as i64);
        s.read_mark() == b"SAMP"
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(20);
        self.body_offset = s.read_b_u16().wrapping_add(4);
        self.number_of_samples = s.read_b_u16();
        self.positions = s.read_b_u16() as u8;
        s.seek_cur(2);
        self.samples_offset = s.read_b_u16().wrapping_add(self.body_offset).wrapping_add(4);
        self.pattern_offsets = vec![0; self.base.number_of_patterns as usize];
        s.seek(30 + self.number_of_samples as i64 * 8 + self.positions as i64 * 2);
        s.read_array_b_u16(&mut self.pattern_offsets);
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut name = vec![0u8; 20];
        s.seek(4);
        s.read_into(&mut name[..16]);
        Some(name)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(30);
        let mut table: Vec<Option<SampleInfo>> = vec![None; 31];
        for _ in 0..self.number_of_samples {
            let number = s.read_u8() as usize;
            let volume = s.read_u8();
            let length = s.read_b_u16();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            // (a number past 31 throws in the C#)
            *table.get_mut(number)? = Some(SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune: 0 });
        }
        let mut v = Vec::new();
        for info in table {
            push!(v, info.unwrap_or(SampleInfo { loop_length: 1, ..Default::default() }), s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        // follows the sample table
        Some((0..self.positions).map(|_| s.read_b_u16() as u8).collect())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for i in 0..self.base.number_of_patterns as usize {
            let mut pat = vec![0u8; 1024];
            s.seek(self.body_offset as i64 + 4 + self.pattern_offsets[i] as i64);
            for j in 0..64 {
                let t = s.read_b_u16();
                // 0x80 ends the pattern. Deviation from the C#, which skips a row
                // whose mask has the end bit 0x8000 and so loses the last row's
                // notes: such a row plays its voices, then the pattern ends (as
                // libxmp reads it).
                let end = t == 0x80 || t & 0x8000 != 0;
                let mask = if end { t & 0x0f } else { t };
                if !end && (mask == 0 || mask > 0x0f) {
                    continue;
                }
                for v in 0..4 {
                    if mask & (1 << v) != 0 {
                        s.read_into(&mut pat[j * 16 + v * 4..j * 16 + v * 4 + 4]);
                    }
                }
                if end {
                    break;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.samples_offset as i64 + 4);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// The song as NovoTrade: rows as a voice mask and the voices' notes.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = b"MODU".to_vec();
        d.extend([0u8; 16]);
        let ns = song.samples.len();
        let (np, npat) = (song.positions.len(), song.patterns.len());
        let mut body = Vec::new();
        let mut offs = Vec::new();
        for p in &song.patterns {
            offs.push(body.len() as u16);
            for (r, row) in p.iter().enumerate() {
                let mut mask = 0u16;
                let mut notes = Vec::new();
                for (v, n) in row.iter().enumerate() {
                    if *n != (0, 0, 0, 0) {
                        mask |= 1 << v;
                        notes.extend(Song::note(*n));
                    }
                }
                // the last row carries the end bit
                if r == 63 {
                    mask |= 0x8000;
                }
                body.extend(mask.to_be_bytes());
                body.extend(notes);
            }
        }
        let body_at = 30 + ns * 8 + np * 2 + npat * 2;
        let samp_at = body_at + 4 + body.len();
        d.extend(((body_at - 4) as u16).to_be_bytes());
        d.extend((ns as u16).to_be_bytes());
        d.extend((np as u16).to_be_bytes());
        d.extend((npat as u16).to_be_bytes());
        d.extend(((samp_at - body_at - 4) as u16).to_be_bytes());
        for (i, s) in song.samples.iter().enumerate() {
            d.extend([i as u8, s.2]);
            d.extend(s.0.to_be_bytes());
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
        }
        for &p in &song.positions {
            d.extend((p as u16).to_be_bytes());
        }
        for o in offs {
            d.extend(o.to_be_bytes());
        }
        d.extend(b"BODY");
        d.extend(body);
        d.extend(b"SAMP");
        for i in 0..ns {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn novotrade() {
        // no fine tunes in the format
        let mut song = Song::new(3);
        song.samples[1].1 = 0;
        let (f, m) = convert_any(&pack(&song)).expect("not recognised");
        assert_eq!(f, "NovoTrade Packer");
        assert_eq!(m, song.module(0x7f));
    }
}
