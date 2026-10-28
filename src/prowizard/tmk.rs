//! TMK Replay, ported from NostalgicPlayer's TmkReplayFormat.
//!
//! "TMK" 1, the song length, the sample count (bit 7: delta-packed samples), the
//! used samples' entries (length, loop start, loop length, fine tune, volume),
//! positions as pattern numbers times two (padded to a word), then 64 row offsets
//! per pattern into the row data that follows: per voice 0x80 (nothing), 0xc0 |
//! effect and its value, 0x80 | note and the sample, or a note times two plus the
//! sample's high bit, the sample/effect byte and the effect value. The sample data
//! follows the last row read.

use super::core::*;

#[derive(Default)]
pub struct Tmk {
    base: Base,
    positions: u8,
    samples: u8,
    delta: bool,
    sample_start: i64,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(69, Box::new(Tmk::default()))]
}

impl Format for Tmk {
    fn name(&self) -> &'static str {
        "TMK Replay"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 100 {
            return false;
        }
        s.seek(0);
        if s.read_mark_n(3) != b"TMK" || s.read_u8() != 1 {
            return false;
        }
        let pos_len = s.read_u8();
        if pos_len > 0x80 {
            return false;
        }
        let count = s.read_u8() & 0x7f;
        if count > 0x1f {
            return false;
        }
        if 6 + count as i64 * 8 + pos_len as i64 > s.len() {
            return false;
        }
        let mut samples_size = 0u32;
        for _ in 0..count {
            samples_size += s.read_b_u16() as u32 * 2;
            s.seek_cur(4);
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
        }
        self.base.find_highest_pattern_read(s, pos_len as usize);
        let n = self.base.number_of_patterns;
        self.base.number_of_patterns = (n - 1) / 2 + 1;
        let need = 6 + count as i64 * 8 + pos_len as i64 + self.base.number_of_patterns as i64 * 64 * 2 + samples_size as i64;
        need <= s.len()
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(4);
        self.positions = s.read_u8();
        let n = s.read_u8();
        self.delta = n & 0x80 != 0;
        self.samples = n & 0x7f;
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(6);
        let mut v = Vec::new();
        for _ in 0..self.samples {
            let length = s.read_b_u16();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        // follows the samples
        let p = (0..self.positions).map(|_| s.read_u8() / 2).collect();
        if self.positions % 2 != 0 {
            s.seek_cur(1);
        }
        Some(p)
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        self.sample_start = 0;
        let ref_start = s.position();
        let npat = self.base.number_of_patterns as usize;
        let refs: Vec<u16> = (0..npat * 64).map(|_| s.read_b_u16()).collect();
        let mut out = Vec::new();
        for i in 0..npat {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 {
                s.seek(ref_start + refs[i * 64 + j] as i64);
                for k in 0..4 {
                    let o = j * 16 + k * 4;
                    let mut t = s.read_u8();
                    if t == 0x80 {
                        continue;
                    }
                    if t & 0xc0 == 0xc0 {
                        // effect only
                        pat[o + 2] = t & 0x0f;
                        pat[o + 3] = s.read_u8();
                        continue;
                    }
                    if t & 0xc0 == 0x80 {
                        // note and sample
                        t &= 0x7f;
                        if t != 0 {
                            let p = PERIODS.get(t as usize - 1)?;
                            pat[o] = p[0];
                            pat[o + 1] = p[1];
                        }
                        let smp = s.read_u8();
                        pat[o] |= smp & 0xf0;
                        pat[o + 2] = smp << 4;
                        continue;
                    }
                    pat[o] = (t & 1) << 4;
                    pat[o + 2] = s.read_u8();
                    pat[o + 3] = s.read_u8();
                    t &= 0xfe;
                    if t != 0 {
                        let p = PERIODS.get((t / 2) as usize - 1)?;
                        pat[o] |= p[0];
                        pat[o + 1] = p[1];
                    }
                }
                self.sample_start = self.sample_start.max(s.position());
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_start);
        if !self.delta {
            return self.base.save_marks_for_all_samples(s, w);
        }
        for i in 0..self.samples as usize {
            let l = self.base.sample_lengths[i] as i64;
            if l != 0 {
                if s.len() - s.position() < l - MAX_MISSING {
                    return false;
                }
                let mut c = s.read_i8();
                w.write_u8(c as u8);
                for _ in 1..l {
                    c = c.wrapping_sub(s.read_i8());
                    w.write_u8(c as u8);
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    /// The song in TMK form, every kind of voice entry used: effect-only ones,
    /// notes without effect as note + sample, the others in the long form.
    fn pack(song: &Song, delta: bool) -> Vec<u8> {
        let n = song.samples.len();
        let mut d = b"TMK".to_vec();
        d.extend([1, song.positions.len() as u8, n as u8 | if delta { 0x80 } else { 0 }]);
        for i in 0..n {
            let s = song.samples[i];
            d.extend(s.0.to_be_bytes());
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
            d.extend([s.1, s.2]);
        }
        d.extend(song.positions.iter().map(|p| p * 2));
        if song.positions.len() % 2 != 0 {
            d.push(0);
        }
        let npat = song.patterns.len();
        let mut rows: Vec<Vec<u8>> = Vec::new();
        let mut refs = Vec::new();
        let mut off = npat * 64 * 2;
        for p in &song.patterns {
            for row in p {
                let mut r = Vec::new();
                for &(smp, per, fx, val) in row {
                    let note = if per == 0 { 0 } else { PERIODS.iter().position(|x| u16::from_be_bytes(*x) == per).unwrap() as u8 + 1 };
                    if (smp, per, fx, val) == (0, 0, 0, 0) {
                        r.push(0x80);
                    } else if smp == 0 && per == 0 {
                        r.extend([0xc0 | fx, val]);
                    } else if fx == 0 && val == 0 && note != 0 {
                        r.extend([0x80 | note, smp]);
                    } else {
                        r.extend([note * 2 | (smp >> 4), (smp << 4) | fx, val]);
                    }
                }
                match rows.iter().position(|x| *x == r) {
                    Some(i) => refs.push(offset_of(&rows, i, npat)),
                    None => {
                        refs.push(off);
                        off += r.len();
                        rows.push(r);
                    }
                }
            }
        }
        for r in refs {
            d.extend((r as u16).to_be_bytes());
        }
        d.extend(rows.concat());
        for i in 0..n {
            let data = song.sample_data(i);
            if delta {
                let mut prev = data[0];
                d.push(prev);
                for &b in &data[1..] {
                    d.push(prev.wrapping_sub(b));
                    prev = b;
                }
            } else {
                d.extend(data);
            }
        }
        d
    }

    /// Where row `i` of the distinct rows starts, from the reference table.
    fn offset_of(rows: &[Vec<u8>], i: usize, npat: usize) -> usize {
        npat * 64 * 2 + rows[..i].iter().map(|r| r.len()).sum::<usize>()
    }

    #[test]
    fn tmk() {
        let song = Song::new(3);
        for delta in [false, true] {
            let (f, m) = convert_any(&pack(&song, delta)).expect("TMK");
            assert_eq!(f, "TMK Replay");
            assert_eq!(m, song.module(0x7f));
        }
    }
}
