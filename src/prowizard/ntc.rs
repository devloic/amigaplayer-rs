//! NoiseTracker Compressed, ported from NostalgicPlayer's
//! NoiseTrackerCompressedFormat.
//!
//! Sample pointers in place of names, the song at 0x174, then "PATT" chunks of
//! packed voices: a byte with the note number and flags (sample high bit, effect
//! present), the sample and effect bytes that follow, and 0xff runs of empty rows.

use super::core::*;

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(19, Box::new(NoiseTrackerCompressed::default()))]
}

#[derive(Default)]
pub struct NoiseTrackerCompressed {
    base: Base,
    positions: u8,
    restart: u8,
}

impl Format for NoiseTrackerCompressed {
    fn name(&self) -> &'static str {
        "NoiseTracker Compressed"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 1080 {
            return false;
        }
        // the first pattern mark
        s.seek(0x1f6);
        if s.read_mark() != b"PATT" {
            return false;
        }
        s.seek(0x174);
        let n = s.read_u8();
        if n == 0 || n >= 128 {
            return false;
        }
        if self.base.find_highest_pattern_at(s, 0x176, n as usize) > 64 {
            return false;
        }
        // the offset of the first sample
        s.seek(0);
        let first = s.read_b_u32();
        if first & 1 != 0 {
            return false;
        }
        // a "PATT" per pattern before the samples
        s.seek(0x1f6);
        for _ in 0..self.base.number_of_patterns {
            loop {
                let m = s.read_mark();
                // (the C# loops for ever where the data ends before the samples)
                if s.eof {
                    return false;
                }
                s.seek_cur(-2);
                if m == b"PATT" {
                    break;
                }
                if s.position() >= first as i64 {
                    return false;
                }
            }
        }
        // the sample table
        s.seek(0);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            if s.read_b_u32() & 1 != 0 {
                return false;
            }
            let len = s.read_b_u16();
            if len >= 0x8000 {
                return false;
            }
            let t1 = s.read_b_u16() as u32;
            let t2 = s.read_b_u16() as u32;
            let t3 = s.read_b_u16() as u32;
            // volume and fine tune
            if t1 & 0x00ff > 0x40 || t1 & 0xff00 > 0x0f00 {
                return false;
            }
            if len == 0 {
                if t1 + t2 + t3 > 1 {
                    return false;
                }
            } else {
                if t2 + t3 > len as u32 {
                    return false;
                }
                samples_size = samples_size.wrapping_add(len as u32 * 2);
            }
        }
        first as i64 + samples_size as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x174);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(0);
        let mut v = Vec::new();
        for _ in 0..31 {
            s.seek_cur(4);
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let ll = s.read_b_u16();
            let loop_length = if ll == 0 { 1 } else { ll };
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut p = vec![0; self.positions as usize];
        s.seek(0x176);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x1f6);
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            // an even offset, then skip "PATT" and three words
            if s.position() & 1 != 0 {
                s.seek_cur(1);
            }
            s.seek_cur(4 + 3 * 2);
            let mut pat = vec![0u8; 1024];
            for j in 0..4 {
                let mut k: i32 = 0;
                while k < 64 {
                    let mut t = s.read_u8();
                    if t == 0xff {
                        // empty rows
                        k += s.read_u8() as i32;
                        continue;
                    }
                    let o = k as usize * 16 + j * 4;
                    // 0xc0: sample 1x, no effect; 0x80: 1x and effect; 0x40: 0x,
                    // no effect; below: 0x and effect
                    let (hi, effect) = match t {
                        0xc0.. => (true, false),
                        0x80.. => (true, true),
                        0x40.. => (false, false),
                        _ => (false, true),
                    };
                    t &= 0x3f;
                    if t != 0 {
                        let p = PERIODS.get(t as usize - 1)?;
                        pat[o] = p[0];
                        pat[o + 1] = p[1];
                    }
                    if hi {
                        pat[o] |= 0x10;
                    }
                    pat[o + 2] = s.read_u8();
                    if effect {
                        pat[o + 3] = s.read_u8();
                    }
                    k += 1;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0);
        let off = s.read_b_u32();
        s.seek(off as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    fn pack(song: &Song) -> Vec<u8> {
        let mut pats = Vec::new();
        let per = |p: u16| super::PERIODS.iter().position(|x| ((x[0] as u16) << 8 | x[1] as u16) == p).unwrap() as u8;
        for p in &song.patterns {
            if pats.len() % 2 != 0 {
                pats.push(0);
            }
            pats.extend(b"PATT");
            pats.extend([0; 6]);
            for v in 0..4 {
                let mut empty = 0u8;
                for row in p {
                    let (smp, period, fx, par) = row[v];
                    if (smp, period, fx, par) == (0, 0, 0, 0) {
                        empty += 1;
                        continue;
                    }
                    if empty > 0 {
                        pats.extend([0xff, empty]);
                        empty = 0;
                    }
                    let n = if period != 0 { per(period) + 1 } else { 0 };
                    let hi = if smp >= 16 { 0x80 } else { 0 };
                    if fx == 0 && par == 0 {
                        pats.extend([n | hi | 0x40, smp << 4]);
                    } else {
                        pats.extend([n | hi, (smp << 4) | fx, par]);
                    }
                }
                if empty > 0 {
                    pats.extend([0xff, empty]);
                }
            }
        }
        if pats.len() % 2 != 0 {
            pats.push(0);
        }
        let first = 0x1f6 + pats.len() as u32;
        let mut d = Vec::new();
        let mut ptr = first;
        for i in 0..31 {
            let (len, ft, vol, ls, ll) = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(ptr.to_be_bytes());
            d.extend(len.to_be_bytes());
            d.extend([ft, vol]);
            d.extend(ls.to_be_bytes());
            d.extend(ll.to_be_bytes());
            ptr += len as u32 * 2;
        }
        d.resize(0x174, 0);
        d.push(song.positions.len() as u8);
        d.push(song.restart);
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d.extend(pl);
        d.extend(pats);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn noisetracker_compressed() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("NoiseTracker Compressed");
        assert_eq!(f, "NoiseTracker Compressed");
        assert_eq!(m, song.module(song.restart));
    }
}
