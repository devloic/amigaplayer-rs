//! STIM (Slam Tilt), ported from NostalgicPlayer's SlamTiltFormat.
//!
//! "STIM", the offset of the sample table, the song length and pattern count at
//! 18, 128 positions, and 64 pattern offsets (from byte 12) at 0x96. A pattern is
//! four track offsets and the tracks: a byte with bit 7 set skips that many rows
//! more, else three bytes hold the sample (5 bits), note (6 bits), effect (split
//! over both) and its value. The sample table is 31 offsets (from the table) of a
//! ProTracker entry without name followed by the sample's data.

use super::core::*;

#[derive(Default)]
pub struct SlamTilt {
    base: Base,
    /// Only set by the conversion: the C# check uses it still zero.
    sample_info_offset: u32,
    sample_start_offsets: [u32; 31],
    pattern_start_offsets: Vec<u32>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(64, Box::new(SlamTilt::default()))]
}

impl Format for SlamTilt {
    fn name(&self) -> &'static str {
        "STIM (Slam Tilt)"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 406 {
            return false;
        }
        s.seek(0);
        if s.read_mark() != b"STIM" {
            return false;
        }
        let info = s.read_b_u32();
        if info < 406 {
            return false;
        }
        s.seek(18);
        if s.read_b_u16() > 128 {
            return false;
        }
        let npat = s.read_b_u16();
        self.base.number_of_patterns = npat;
        if npat > 64 || npat == 0 {
            return false;
        }
        for _ in 0..128 {
            if s.read_u8() as u16 > npat {
                return false;
            }
        }
        s.seek(info as i64);
        let mut offs = [0u32; 31];
        s.read_array_b_u32(&mut offs);
        let mut samples_size = 0u32;
        for o in offs {
            s.seek(info.wrapping_add(o) as i64);
            samples_size = samples_size.wrapping_add(s.read_b_u16() as u32 * 2);
        }
        if samples_size < 4 {
            return false;
        }
        let need = self.sample_info_offset.wrapping_add(31 * 4 + 31 * 8).wrapping_add(samples_size);
        need as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(4);
        self.sample_info_offset = s.read_b_u32();
        s.seek(self.sample_info_offset as i64);
        s.read_array_b_u32(&mut self.sample_start_offsets);
        s.seek(0x96);
        self.pattern_start_offsets = (0..64).map(|_| s.read_b_u32().wrapping_add(12)).collect();
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let mut v = Vec::new();
        for i in 0..31 {
            s.seek(self.sample_info_offset.wrapping_add(self.sample_start_offsets[i]) as i64);
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
        s.seek(18);
        let n = s.read_b_u16() as usize;
        s.seek_cur(2);
        let mut p = vec![0u8; n];
        s.read_into(&mut p);
        Some(p)
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for i in 0..self.base.number_of_patterns as usize {
            let offset = self.pattern_start_offsets[i] as i64;
            s.seek(offset);
            let mut tracks = [0u16; 4];
            s.read_array_b_u16(&mut tracks);
            let mut pat = vec![0u8; 1024];
            for (j, &t) in tracks.iter().enumerate() {
                s.seek(offset + t as i64);
                let mut k = 0;
                while k < 64 {
                    let c1 = s.read_u8();
                    if c1 & 0x80 != 0 {
                        k += (c1 & 0x7f) as usize + 1;
                        continue;
                    }
                    let c2 = s.read_u8();
                    let c3 = s.read_u8();
                    let smp = c1 & 0x1f;
                    let mut note = c2 & 0x3f;
                    let fx = ((c1 >> 5) & 0x03) | ((c2 >> 4) & 0x0c);
                    let o = k * 16 + j * 4;
                    pat[o] = smp & 0xf0;
                    if note != 0 {
                        note -= 1;
                        let p = PERIODS.get(note as usize)?;
                        pat[o] |= p[0];
                        pat[o + 1] = p[1];
                    }
                    pat[o + 2] = (smp << 4) | fx;
                    pat[o + 3] = c3;
                    k += 1;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        for i in 0..31 {
            let l = self.base.sample_lengths[i] as i64;
            if l != 0 {
                s.seek(self.sample_info_offset.wrapping_add(self.sample_start_offsets[i]).wrapping_add(8) as i64);
                if s.len() - s.position() < l - MAX_MISSING {
                    return false;
                }
                w.set_sample_data_marker(s, l);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    fn pack(song: &Song) -> Vec<u8> {
        let npat = song.patterns.len();
        let mut d = b"STIM".to_vec();
        d.extend([0; 4]); // the sample table's offset, set below
        d.resize(18, 0);
        d.extend((song.positions.len() as u16).to_be_bytes());
        d.extend((npat as u16).to_be_bytes());
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d.extend(pl);
        assert_eq!(d.len(), 0x96);
        let mut pats = Vec::new();
        let mut poffs = Vec::new();
        for p in &song.patterns {
            poffs.push(406 + pats.len() - 12);
            let mut tracks: Vec<Vec<u8>> = Vec::new();
            for v in 0..4 {
                let mut t = Vec::new();
                let mut empty = 0u8;
                for row in p {
                    let (smp, per, fx, val) = row[v];
                    if (smp, per, fx, val) == (0, 0, 0, 0) {
                        empty += 1;
                        continue;
                    }
                    if empty > 0 {
                        t.push(0x80 | (empty - 1));
                        empty = 0;
                    }
                    let note = if per == 0 { 0 } else { PERIODS.iter().position(|x| u16::from_be_bytes(*x) == per).unwrap() + 1 };
                    t.extend([((fx & 3) << 5) | smp, ((fx & 0xc) << 4) | note as u8, val]);
                }
                if empty > 0 {
                    t.push(0x80 | (empty - 1));
                }
                tracks.push(t);
            }
            let mut o = 8;
            for t in &tracks {
                pats.extend((o as u16).to_be_bytes());
                o += t.len();
            }
            pats.extend(tracks.concat());
        }
        for i in 0..64 {
            d.extend((poffs.get(i).copied().unwrap_or(0) as u32).to_be_bytes());
        }
        assert_eq!(d.len(), 406);
        d.extend(pats);
        let info = d.len();
        d[4..8].copy_from_slice(&(info as u32).to_be_bytes());
        let mut entries = Vec::new();
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            let off = 31 * 4 + entries.len();
            d.extend((off as u32).to_be_bytes());
            entries.extend(s.0.to_be_bytes());
            entries.extend([s.1, s.2]);
            entries.extend(s.3.to_be_bytes());
            entries.extend(s.4.to_be_bytes());
            if i < song.samples.len() {
                entries.extend(song.sample_data(i));
            }
        }
        d.extend(entries);
        d
    }

    #[test]
    fn slam_tilt() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("STIM");
        assert_eq!(f, "STIM (Slam Tilt)");
        assert_eq!(m, song.module(0x7f));
    }
}
