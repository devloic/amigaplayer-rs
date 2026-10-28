//! The Player 2.2A and 3.0A (Jarno Paananen), ported from NostalgicPlayer's
//! ThePlayer2x_3xFormatBase, ThePlayer22AFormat and ThePlayer30AFormat.
//!
//! After the "P22A"/"P30A" mark come the offsets of the track data, the track
//! table and the sample data, then 16 bytes per sample. A position is one offset
//! into the track data, where the four voices' rows are interleaved, each 4 bytes
//! with a count of empty rows after it (3.0A: or of repeats of the row).

use super::core::*;

#[derive(Default)]
pub struct ThePlayer2x {
    base: Base,
    v30: bool,
    samples: u8,
    positions: u8,
    track_offsets: Vec<u16>,
    position_list: Vec<u8>,
    track_data: u32,
    track_table: u32,
    sample_data: u32,
    sample_starts: Vec<u32>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![
        (66, Box::new(ThePlayer2x { v30: false, ..Default::default() })),
        (67, Box::new(ThePlayer2x { v30: true, ..Default::default() })),
    ]
}

fn store(pattern: &mut [u8], channel: usize, row: usize, b: [u8; 4]) -> Option<()> {
    // past the pattern, the C# converter throws
    pattern.get_mut(row * 16 + channel * 4..row * 16 + channel * 4 + 4)?.copy_from_slice(&b);
    Some(())
}

impl ThePlayer2x {
    /// CheckForThePlayerFormat.
    fn check_format(&mut self, s: &mut Stream, mark: &[u8]) -> bool {
        if s.len() < 20 {
            return false;
        }
        s.seek(0);
        if s.read_mark() != mark {
            return false;
        }
        self.base.number_of_patterns = s.read_u8() as u16;
        if self.base.number_of_patterns > 0x7f {
            return false;
        }
        s.seek_cur(1);
        let count = s.read_u8();
        if count > 0x1f || count == 0 {
            return false;
        }
        s.seek(20);
        if s.read_b_u32() != 0 {
            return false;
        }
        s.seek_cur(-4);
        let mut size = 0u32;
        for _ in 0..count {
            s.seek_cur(4);
            let length = s.read_b_u16() as u32 * 2;
            s.seek_cur(4);
            let loop_length = s.read_b_u16() as u32 * 2;
            if length > 0xffff || loop_length > 0xffff || loop_length > length + 2 {
                return false;
            }
            size += length;
            s.seek_cur(3);
            if s.read_u8() > 0x40 {
                return false;
            }
        }
        size > 4
    }

    /// ConvertEffect56AValue: the volume slide argument.
    fn effect_56a(&self, c3: u8) -> u8 {
        if self.v30 {
            if c3 > 0x7f { (c3 << 4) & 0xf0 } else { c3 }
        } else if c3 > 0x7f {
            ((0x100 - c3 as u32) << 4) as u8
        } else {
            c3
        }
    }

    /// ConvertData: a row as ProTracker bytes.
    fn convert_data(&self, c1: u8, mut c2: u8, mut c3: u8) -> Option<[u8; 4]> {
        let sample = ((c1 << 4) & 0x10) | ((c2 >> 4) & 0x0f);
        let note = c1 & 0x7f;
        let (p1, p2) = if note != 0 {
            let p = PERIODS.get((note / 2).wrapping_sub(1) as usize)?;
            (p[0], p[1])
        } else {
            (0, 0)
        };
        match c2 & 0x0f {
            0x08 => c2 -= 0x08,
            0x05 | 0x06 | 0x0a => c3 = self.effect_56a(c3),
            _ => {}
        }
        Some([(sample & 0xf0) | (p1 & 0x0f), p2, c2, c3])
    }

    /// ConvertPatternData of 2.2A and 3.0A.
    fn convert_pattern_data(&self, c: [u8; 4], channel: usize, pattern: &mut [u8], rows: &mut [usize; 4]) -> Option<()> {
        let [c1, c2, c3, c4] = c;
        if !self.v30 || c1 != 0x80 {
            let d = self.convert_data(c1, c2, c3)?;
            store(pattern, channel, rows[channel], d)?;
        } else {
            let d = self.convert_data(c1 & 0x7f, c2, c3)?;
            for r in 0..=c4 as usize {
                store(pattern, channel, rows[channel] + r, d)?;
            }
        }
        if c4 > 0x00 && c4 < 0x80 {
            rows[channel] += c4 as usize;
        }
        rows[channel] += 1;
        Some(())
    }
}

impl Format for ThePlayer2x {
    fn name(&self) -> &'static str {
        if self.v30 { "The Player 3.0A" } else { "The Player 2.2A" }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        self.check_format(s, if self.v30 { b"P30A" } else { b"P22A" })
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(5);
        self.positions = s.read_u8() / 2;
        self.samples = s.read_u8();
        s.seek_cur(1);
        self.track_data = s.read_b_u32();
        self.track_table = s.read_b_u32();
        self.sample_data = s.read_b_u32();
        self.track_offsets = vec![0; self.positions as usize];
        s.seek(self.track_table as i64 + 4);
        s.read_array_b_u16(&mut self.track_offsets);
        let t = &self.track_offsets;
        self.position_list = self.base.build_position_list(self.positions as usize, |p| t[p]).0;
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        self.sample_starts = vec![0; self.samples as usize];
        s.seek(20);
        let mut v = Vec::new();
        for i in 0..self.samples as usize {
            self.sample_starts[i] = s.read_b_u32();
            let length = s.read_b_u16();
            let loop_start = s.read_b_u32();
            let loop_length = s.read_b_u16();
            let fine_tune = s.read_b_u16();
            let volume = s.read_b_u16();
            let si = SampleInfo {
                name: None,
                length,
                loop_start: (loop_start.wrapping_sub(self.sample_starts[i]) / 2) as u16,
                loop_length,
                volume: volume as u8,
                fine_tune: (fine_tune / 74) as u8,
            };
            push!(v, si, s);
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
                let mut pattern = vec![0u8; 1024];
                let mut rows = [0usize; 4];
                let to = self.track_offsets[i] as u32;
                s.seek(self.track_data.wrapping_add(4).wrapping_add(to) as i64);
                for k in 0..64 {
                    for j in 0..4 {
                        if rows[j] > k {
                            continue;
                        }
                        let c = [s.read_u8(), s.read_u8(), s.read_u8(), s.read_u8()];
                        self.convert_pattern_data(c, j, &mut pattern, &mut rows)?;
                    }
                }
                push!(out, pattern, s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        let off = self.sample_data.wrapping_add(4);
        for i in 0..self.samples as usize {
            let length = self.base.sample_lengths[i] as i64;
            if length != 0 {
                s.seek(off.wrapping_add(self.sample_starts[i]) as i64);
                if s.len() - s.position() < length - MAX_MISSING {
                    return false;
                }
                w.set_sample_data_marker(s, length);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// The Player 2.2A/3.0A: every row of every voice stored, no empty-row counts.
    fn pack(song: &Song, mark: &[u8; 4]) -> Vec<u8> {
        let n = song.samples.len();
        let mut d = mark.to_vec();
        d.push(song.patterns.len() as u8);
        d.push(song.positions.len() as u8 * 2);
        d.push(n as u8);
        d.push(0);
        let tt = 20 + 16 * n as u32;
        let td = tt + 4 + 2 * song.positions.len() as u32;
        let track_len = 64 * 4 * 4;
        let sd = td + 4 + (track_len * song.patterns.len()) as u32;
        for v in [td, tt, sd] {
            d.extend(v.to_be_bytes());
        }
        let mut start = 0u32;
        for s in &song.samples {
            d.extend(start.to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend((start + s.3 as u32 * 2).to_be_bytes());
            d.extend(s.4.to_be_bytes());
            d.extend((s.1 as u16 * 74).to_be_bytes());
            d.extend((s.2 as u16).to_be_bytes());
            start += s.0 as u32 * 2;
        }
        d.extend([0; 4]);
        for &p in &song.positions {
            d.extend((p as u16 * track_len as u16).to_be_bytes());
        }
        d.extend([0; 4]);
        for p in &song.patterns {
            for row in p {
                for &(smp, period, eff, par) in row {
                    let c1 = match period {
                        0 => 0,
                        _ => (crate::prowizard::core::PERIODS.iter().position(|x| (x[0] as u16) << 8 | x[1] as u16 == period).unwrap() as u8 + 1) * 2,
                    } | (smp >> 4);
                    d.extend([c1, (smp << 4) | eff, par, 0]);
                }
            }
        }
        d.extend([0; 4]);
        for i in 0..n {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn the_player_2x_3x() {
        let song = Song::new(3);
        for (mark, name) in [(b"P22A", "The Player 2.2A"), (b"P30A", "The Player 3.0A")] {
            let (f, m) = convert_any(&pack(&song, mark)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f));
        }
    }
}
