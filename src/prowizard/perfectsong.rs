//! Perfect Song 1 and 2, ported from NostalgicPlayer's PerfectSongFormatBase,
//! PerfectSong1Format and PerfectSong2Format.
//!
//! "PERFSONG"/"PERFSNG2", the offset of the texts (module and sample names) plus
//! 690, 31 sample entries (data and loop offsets, length, loop length, fine tune,
//! volume), the song length and a pattern offset per position, and the patterns at
//! 0x3fe: a note number times two (plus the sample's high bit), the sample, an
//! effect number times four and its value. 2's sample offsets count from after the
//! texts and a 12-byte header there; 1's are from the start of the file.

use super::core::*;

#[derive(Default)]
pub struct PerfectSong {
    base: Base,
    v2: bool,
    text_offset: u32,
    sample_start: u32,
    sample_offsets: Vec<u32>,
    position_list: Vec<u8>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![
        (62, Box::new(PerfectSong::default())),
        (63, Box::new(PerfectSong { v2: true, ..Default::default() })),
    ]
}

impl PerfectSong {
    /// The effects as ProTracker's; 1 and 2 differ only in 0x44 (B and F).
    fn convert_effect(&self, effect: &mut u8, val: &mut u8) {
        let swap = |v: u8| (v >> 4) | (v << 4);
        *effect = match *effect {
            0x00 | 0x04 => 0x0,
            0x08 => 0x1,
            0x0c => 0x2,
            0x10 | 0x14 => 0x3,
            0x18 | 0x1c => 0x4,
            0x20 => 0x5,
            0x24 => {
                *val = swap(*val);
                0x6
            }
            0x28 | 0x2c => 0x6,
            0x30 => 0x7,
            0x38 => 0x9,
            0x3c => {
                *val = swap(*val);
                0xa
            }
            0x40 => 0xa,
            0x44 => {
                if self.v2 { 0xf } else { 0xb }
            }
            0x48 => 0xc,
            0x4c => 0xd,
            0x50 => 0xf,
            0x58 => {
                *val = 0x01;
                0xe
            }
            0x5c => {
                *val |= 0x10;
                0xe
            }
            0x60 => {
                *val |= 0x20;
                0xe
            }
            0x74 => {
                *val |= 0xa0;
                0xe
            }
            0x78 => {
                *val |= 0xb0;
                0xe
            }
            0x7c => {
                *val |= 0xe0;
                0xe
            }
            _ => {
                *val = 0;
                0x0
            }
        };
    }
}

impl Format for PerfectSong {
    fn name(&self) -> &'static str {
        if self.v2 { "Perfect Song 2" } else { "Perfect Song 1" }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x3fe {
            return false;
        }
        s.seek(0);
        if s.read_mark_n(8) != if self.v2 { b"PERFSNG2" } else { b"PERFSONG" } {
            return false;
        }
        let mut t = s.read_b_u32().wrapping_add(if self.v2 { 24 } else { 12 });
        if t as i64 > s.len() || t < 802 {
            return false;
        }
        if self.v2 {
            s.seek(t as i64 - 4);
            t = t.wrapping_add(s.read_b_u32());
            if t as i64 > s.len() + MAX_MISSING {
                return false;
            }
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(8);
        let t = s.read_b_u32();
        self.text_offset = t.wrapping_sub(690);
        self.sample_start = if self.v2 { t.wrapping_add(12) } else { 0 };
        // CreatePositionList
        s.seek(0x1fc);
        let len = s.read_b_u16() as usize;
        self.position_list = (0..len).map(|_| (s.read_b_u32().wrapping_sub(0x3fe) / 1024) as u8).collect();
        self.base.find_highest_pattern(&self.position_list.clone());
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0u8; 20];
        s.seek(self.text_offset as i64);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        self.sample_offsets = vec![0; 31];
        let mut v = Vec::new();
        for i in 0..31 {
            let mut name = vec![0u8; 22];
            s.seek(self.text_offset as i64 + 20 + i as i64 * 22);
            s.read_into(&mut name);
            s.seek(12 + i as i64 * 16);
            self.sample_offsets[i] = s.read_b_u32().wrapping_add(self.sample_start);
            let lp = s.read_b_u32().wrapping_add(self.sample_start);
            let length = s.read_b_u16();
            let loop_length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = (lp.wrapping_sub(self.sample_offsets[i]) / 2) as u16;
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list.clone())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(0x3fe);
        let mut out = Vec::new();
        let mut pat = vec![0u8; 1024];
        for _ in 0..self.base.number_of_patterns {
            for j in 0..64 * 4 {
                let mut note = s.read_u8();
                let sample = s.read_u8();
                let mut effect = s.read_u8();
                let mut val = s.read_u8();
                self.convert_effect(&mut effect, &mut val);
                let sample = ((note & 1) << 4) | (sample >> 4);
                pat[j * 4] = sample & 0xf0;
                pat[j * 4 + 2] = (sample << 4) | effect;
                pat[j * 4 + 3] = val;
                note /= 2;
                if note != 0 {
                    let p = PERIODS.get(note as usize - 1)?;
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
                s.seek(self.sample_offsets[i] as i64);
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

    /// The song in Perfect Song form; its only effect, C, is 0x48.
    fn pack(song: &Song, v2: bool) -> Vec<u8> {
        let npat = song.patterns.len();
        let text = 0x3fe + npat * 1024;
        let base = if v2 { text + 702 } else { 0 };
        let mut sdata = Vec::new();
        let mut offs = Vec::new();
        let first = if v2 { 12 } else { text + 702 };
        for i in 0..31 {
            offs.push(first + sdata.len());
            if i < song.samples.len() {
                sdata.extend(song.sample_data(i));
            }
        }
        let mut d = Vec::new();
        d.extend(if v2 { b"PERFSNG2" } else { b"PERFSONG" });
        d.extend(((text + 690) as u32).to_be_bytes());
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend((offs[i] as u32).to_be_bytes());
            d.extend(((offs[i] + s.3 as usize * 2) as u32).to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend(s.4.to_be_bytes());
            d.extend([s.1, s.2, 0, 0]);
        }
        d.extend((song.positions.len() as u16).to_be_bytes());
        for &p in &song.positions {
            d.extend((0x3fe + p as u32 * 1024).to_be_bytes());
        }
        d.resize(0x3fe, 0);
        for p in &song.patterns {
            for row in p {
                for &(smp, per, fx, val) in row {
                    let note = if per == 0 { 0 } else { PERIODS.iter().position(|x| u16::from_be_bytes(*x) == per).unwrap() + 1 };
                    let fx = if fx == 0xc { 0x48 } else { fx };
                    d.extend([(note * 2) as u8 | (smp >> 4), smp << 4, fx, val]);
                }
            }
        }
        assert_eq!(d.len(), text);
        d.resize(text + 702, 0);
        if v2 {
            d.extend([0; 8]);
            d.extend((sdata.len() as u32).to_be_bytes());
        }
        assert_eq!(d.len(), base + first);
        d.extend(sdata);
        d
    }

    #[test]
    fn perfect_song() {
        let song = Song::new(3);
        for (v2, name) in [(false, "Perfect Song 1"), (true, "Perfect Song 2")] {
            let (f, m) = convert_any(&pack(&song, v2)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f));
        }
    }
}
