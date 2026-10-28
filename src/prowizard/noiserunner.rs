//! NoiseRunner, ported from NostalgicPlayer's NoiseRunnerFormat.
//!
//! A ProTracker module with its sample table rewritten (volume, sample and loop
//! pointers, a fine tune as a period offset) and its notes as bytes: effect,
//! parameter, note number, sample * 8.

use super::core::*;

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(18, Box::new(NoiseRunner::default()))]
}

#[derive(Default)]
pub struct NoiseRunner {
    base: Base,
    positions: u8,
    restart: u8,
}

impl Format for NoiseRunner {
    fn name(&self) -> &'static str {
        "NoiseRunner"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 1080 {
            return false;
        }
        // the sample table
        s.seek(0);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            if s.read_b_u16() > 0x40 {
                return false;
            }
            let start = s.read_b_u32();
            if start == 0 {
                return false;
            }
            let len = s.read_b_u16();
            let lp = s.read_b_u32();
            if lp == 0 {
                return false;
            }
            if len == 0 {
                if start != lp {
                    return false;
                }
                s.seek_cur(4);
            } else {
                let t = (lp.wrapping_sub(start) / 2).wrapping_add(s.read_b_u16() as u32);
                if t > len as u32 {
                    return false;
                }
                s.seek_cur(2);
            }
            samples_size = samples_size.wrapping_add(len as u32 * 2);
        }
        // the position list
        s.seek(0x3b6);
        let n = s.read_u8();
        if n == 0 || n > 0x7f {
            return false;
        }
        s.seek_cur(1);
        if s.read_u8() > 0x3f || s.read_u8() > 0x3f {
            return false;
        }
        // some of the "left-over" in the module: the last sample of the original
        s.seek(0x3ae);
        let len = s.read_b_u16();
        if len >= 0x8000 {
            return false;
        }
        if len == 0 {
            let t = s.read_b_u16().wrapping_add(s.read_b_u16()).wrapping_add(s.read_b_u16());
            if t != 1 {
                return false;
            }
        } else {
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return false;
            }
            if s.read_b_u16() as u32 + s.read_b_u16() as u32 > len as u32 {
                return false;
            }
        }
        if self.base.find_highest_pattern_at(s, 0x3b8, n as usize) > 64 {
            return false;
        }
        if 0x43c + self.base.number_of_patterns as i64 * 1024 + samples_size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the note numbers of the first pattern
        for i in 0..64 * 4 {
            s.seek(0x43c + i * 4 + 2);
            if s.read_u8() > 0x48 {
                return false;
            }
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x3b6);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(0);
        let mut v = Vec::new();
        for _ in 0..31 {
            let volume = s.read_b_u16();
            let start = s.read_b_u32();
            let length = s.read_b_u16();
            let lp = s.read_b_u32();
            let loop_length = s.read_b_u16();
            // the fine tune, as a period offset
            let t = s.read_b_u16();
            let fine_tune = if t & 0xf000 == 0xf000 { (((0x1000 - t as i32) & 0x0fff) / 0x48) as u8 } else { 0 };
            let loop_start = (lp.wrapping_sub(start) / 2) as u16;
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume: volume as u8, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut p = vec![0; self.positions as usize];
        s.seek(0x3b8);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x43c);
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 * 4 {
                let b1 = s.read_u8();
                let b2 = s.read_u8();
                let b3 = s.read_u8();
                let b4 = s.read_u8();
                // sample number
                let mut t = b4 / 8;
                if t >= 0x10 {
                    pat[j * 4] = 0x10;
                    t -= 0x10;
                }
                pat[j * 4 + 2] = t << 4;
                // note (C#: (byte)((b3 - 2) / 2), truncating towards zero)
                if b3 != 0 {
                    let p = PERIODS.get(((b3 as i32 - 2) / 2) as u8 as usize)?;
                    pat[j * 4] |= p[0];
                    pat[j * 4 + 1] = p[1];
                }
                pat[j * 4 + 3] = b2;
                // effect: 0 is tone portamento, C none, the others times 4
                pat[j * 4 + 2] |= match b1 {
                    0x0 => 0x3,
                    0xc => 0x0,
                    e => e / 4,
                };
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        // the sample data follows the patterns
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// Pack the song as NoiseRunner: only volume (C) effects and no fine tunes
    /// are used, which it stores as 0x30 and a zero period offset.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = Vec::new();
        let mut ptr = 0x10000u32;
        for i in 0..31 {
            let (len, _, vol, ls, ll) = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend((vol as u16).to_be_bytes());
            d.extend(ptr.to_be_bytes());
            d.extend(len.to_be_bytes());
            d.extend((ptr + ls as u32 * 2).to_be_bytes());
            d.extend(ll.to_be_bytes());
            d.extend([0, 0]);
            ptr += len as u32 * 2;
        }
        // the "left-over": an empty last sample of the original module
        d.resize(0x3ae, 0);
        d.extend([0, 0, 0, 0, 0, 0, 0, 1]);
        d.push(song.positions.len() as u8);
        d.push(song.restart);
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d.extend(pl);
        d.extend(b"M.K.");
        let per = |p: u16| super::PERIODS.iter().position(|x| ((x[0] as u16) << 8 | x[1] as u16) == p).unwrap() as u8;
        for p in &song.patterns {
            for row in p {
                for &(smp, period, fx, par) in row {
                    let note = if period != 0 { per(period) * 2 + 2 } else { 0 };
                    let e = if fx == 0xc { 0x30 } else { fx * 4 };
                    // the empty effect: C -> 0 in the conversion
                    let e = if fx == 0 { 0xc } else { e };
                    d.extend([e, par, note, smp * 8]);
                }
            }
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn noiserunner() {
        let mut song = Song::new(3);
        song.samples[1].1 = 0;
        let (f, m) = convert_any(&pack(&song)).expect("NoiseRunner");
        assert_eq!(f, "NoiseRunner");
        assert_eq!(m, song.module(song.restart));
    }
}
