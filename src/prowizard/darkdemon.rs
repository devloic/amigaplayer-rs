//! The Dark Demon, ported from NostalgicPlayer's TheDarkDemonFormat.
//!
//! The song length, restart and 128 positions, 31 sample entries (data address,
//! length, fine tune, volume, loop address, loop length) up to 564, the sample
//! data, then the patterns: the whole sample number, a note number times two, the
//! effect and its value.

use super::core::*;

#[derive(Default)]
pub struct DarkDemon {
    base: Base,
    positions: u8,
    restart: u8,
    sample_addresses: Vec<u32>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(65, Box::new(DarkDemon::default()))]
}

impl Format for DarkDemon {
    fn name(&self) -> &'static str {
        "The Dark Demon"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x234 {
            return false;
        }
        s.seek(0);
        let pos_len = s.read_u8();
        if pos_len > 0x7f || pos_len == 0 {
            return false;
        }
        if self.base.find_highest_pattern_at(s, 2, pos_len as usize) > 0x7f {
            return false;
        }
        for _ in pos_len..128 {
            if s.read_u8() != 0 {
                return false;
            }
        }
        let mut samples_size = 0u32;
        for _ in 0..31 {
            let addr = s.read_b_u32();
            let length = s.read_b_u16().wrapping_mul(2) as u32;
            s.seek_cur(1);
            let volume = s.read_u8();
            let lp = s.read_b_u32();
            let loop_length = s.read_b_u16().wrapping_mul(2) as u32;
            if volume > 64 || lp < addr || addr < 564 || lp < 564 {
                return false;
            }
            if lp - addr > length || (lp - addr) + loop_length > length + 2 {
                return false;
            }
            samples_size += length;
        }
        if samples_size <= 2 || samples_size > 31 * 65535 {
            return false;
        }
        if samples_size as i64 + 564 + self.base.number_of_patterns as i64 * 1024 > s.len() {
            return false;
        }
        // the first pattern
        s.seek(564 + samples_size as i64);
        for _ in 0..64 * 4 {
            if s.read_u8() > 31 {
                return false;
            }
            let t = s.read_u8();
            if t > 0x48 || t & 1 != 0 {
                return false;
            }
            let fx = s.read_u8() & 0x0f;
            let val = s.read_u8();
            if (fx == 0x0c || fx == 0x0d) && val > 0x40 || fx == 0x0b && val > 0x7f {
                return false;
            }
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        self.sample_addresses = vec![0; 31];
        s.seek(130);
        let mut v = Vec::new();
        for i in 0..31 {
            self.sample_addresses[i] = s.read_b_u32();
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = (s.read_b_u32().wrapping_sub(self.sample_addresses[i]) / 2) as u16;
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut p = vec![0u8; self.positions as usize];
        s.seek(2);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let size: u32 = self.base.sample_lengths.iter().fold(0u32, |a, &l| a.wrapping_add(l));
        s.seek(564 + size as i32 as i64);
        let mut out = Vec::new();
        let mut pat = vec![0u8; 1024];
        for _ in 0..self.base.number_of_patterns {
            for j in 0..64 * 4 {
                let mut n = [0u8; 4];
                for b in &mut n {
                    *b = s.read_u8();
                }
                pat[j * 4 + 3] = n[3];
                pat[j * 4 + 2] = (n[2] & 0x0f) | (n[0] << 4);
                pat[j * 4] = n[0] & 0xf0;
                if n[1] != 0 {
                    let p = PERIODS.get(((n[1] / 2) as usize).wrapping_sub(1))?;
                    pat[j * 4] |= p[0];
                    pat[j * 4 + 1] = p[1];
                } else {
                    pat[j * 4 + 1] = 0;
                }
            }
            push!(out, pat.clone(), s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        for i in 0..31 {
            let l = self.base.sample_lengths[i] as i64;
            if l != 0 {
                s.seek(self.sample_addresses[i] as i64);
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
        let mut d = vec![song.positions.len() as u8, song.restart];
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d.extend(pl);
        let mut addr = 564;
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend((addr as u32).to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend([s.1, s.2]);
            d.extend(((addr + s.3 as usize * 2) as u32).to_be_bytes());
            d.extend(s.4.to_be_bytes());
            addr += s.0 as usize * 2;
        }
        assert_eq!(d.len(), 564);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        for p in &song.patterns {
            for row in p {
                for &(smp, per, fx, val) in row {
                    let note = if per == 0 { 0 } else { PERIODS.iter().position(|x| u16::from_be_bytes(*x) == per).unwrap() + 1 };
                    d.extend([smp, note as u8 * 2, fx, val]);
                }
            }
        }
        d
    }

    #[test]
    fn dark_demon() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("The Dark Demon");
        assert_eq!(f, "The Dark Demon");
        assert_eq!(m, song.module(song.restart));
    }
}
