//! NoisePacker 1, 2 and 3, ported from NostalgicPlayer's NoisePackerFormatBase,
//! NoisePacker1Format, NoisePacker2Format and NoisePacker3Format.
//!
//! A header with the sample count (and a 'C' nibble), the position list and track
//! table sizes; 16 bytes per sample; a position list of pattern offsets; a track
//! table of four track offsets per pattern (stored last voice first); then the
//! tracks: three bytes a row (NoisePacker 3 packs empty rows and ends tracks
//! early), notes as numbers and some effects renumbered.

use super::core::*;

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![
        (15, Box::new(NoisePacker { version: 1, ..Default::default() })),
        (16, Box::new(NoisePacker { version: 2, ..Default::default() })),
        (17, Box::new(NoisePacker { version: 3, ..Default::default() })),
    ]
}

#[derive(Default)]
pub struct NoisePacker {
    base: Base,
    version: i32,
    number_of_samples: usize,
    number_of_positions: usize,
    position_list_offset: i64,
    track_data_offset: i64,
    sample_data_offset: i64,
    track_offset_table: Vec<u16>,
}

/// BuildNoteAndSamplePair: the note number (and the sample's high bit) as a
/// period. None where the C# reads past its period table.
fn note_and_sample(dat1: u8) -> Option<(u8, u8)> {
    let mut b1 = 0;
    let mut b2 = 0;
    let mut note = dat1 & 0xfe;
    if note != dat1 {
        b1 = 0x10;
    }
    if dat1 > 0x01 {
        note = note.wrapping_sub(2) / 2;
        let p = PERIODS.get(note as usize)?;
        b1 |= p[0];
        b2 = p[1];
    }
    Some((b1, b2))
}

/// BuildEffect: the effect and its parameter in ProTracker's numbering.
fn effect(dat2: u8, mut dat3: u8, break_offset: u8) -> (u8, u8) {
    let b3 = dat2 & 0xf0;
    let mut e = dat2 & 0x0f;
    match e {
        // pattern break
        0xb => dat3 = dat3.wrapping_add(break_offset) / 2,
        // volume slide, vibrato + volume slide, portamento + volume slide
        0x5..=0x7 => {
            if e == 0x7 {
                e = 0xa;
            }
            dat3 = if (dat3 as i8) < 0 { (dat3 as i8).wrapping_neg() as u8 } else { dat3 << 4 };
        }
        // arpeggio
        0x8 => e = 0,
        // filter
        0xe => {
            if dat3 & 0xf0 == 0 && dat3 != 0 {
                dat3 = 0x01;
            }
        }
        _ => {}
    }
    (b3 | e, dat3)
}

impl NoisePacker {
    /// CheckSampleInfo: the total sample size (0 if wrong) and the version it
    /// suggests (NoisePacker 1 and 2 share theirs).
    fn check_sample_info(&self, s: &mut Stream, count: u16) -> (u32, i32) {
        let mut size = 0u32;
        s.seek(8);
        if self.version == 3 {
            for _ in 0..count {
                if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                    return (0, 3);
                }
                if s.read_b_u32() > 0x1fe000 {
                    return (0, 3);
                }
                let len = s.read_b_u16();
                if len > 0x8000 {
                    return (0, 3);
                }
                size = size.wrapping_add(len as u32 * 2);
                if s.read_b_u32() > 0x1fe000 {
                    return (0, 3);
                }
                let a = s.read_b_u16() as u32;
                let b = s.read_b_u16() as u32;
                if a + b > len as u32 {
                    return (0, 3);
                }
            }
            return (size, 3);
        }
        let mut version = 2;
        for _ in 0..count {
            if s.read_b_u32() > 0x1fe000 {
                return (0, version);
            }
            let len = s.read_b_u16();
            // no empty samples are written in this format
            if len > 0x8000 || len == 0 {
                return (0, version);
            }
            size = size.wrapping_add(len as u32 * 2);
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return (0, version);
            }
            if s.read_b_u32() > 0x1fe000 {
                return (0, version);
            }
            let a = s.read_b_u16() as u32;
            let b = s.read_b_u16() as u32;
            if a + b > len as u32 {
                // NoisePacker 1 stores the loop length in bytes
                if a + b / 2 > len as u32 {
                    return (0, version);
                }
                version = 1;
            }
        }
        (size, version)
    }

    /// CheckPatternData over the track data.
    fn check_pattern_data(&self, s: &mut Stream, count: u16, track_length: u16, version: &mut i32) -> bool {
        if self.version == 3 {
            let mut i = 0;
            while i < track_length as i32 {
                let t1 = s.read_u8();
                if t1 & 0x80 == 0x80 {
                    i += 1;
                    continue;
                }
                if t1 > 0x49 {
                    return false;
                }
                let t2 = s.read_u8();
                let t3 = s.read_u8();
                if t2 & 0x0f == 0x0a {
                    return false;
                }
                if t2 & 0x0f == 0x0d && t3 > 0x64 {
                    return false;
                }
                i += 3;
            }
            return true;
        }
        let mut i = 0;
        while i < track_length as i32 {
            let t1 = s.read_u8();
            if t1 > 0x49 {
                return false;
            }
            let t2 = s.read_u8();
            if (((t1 as u16) << 4) & 0x10 | ((t2 >> 4) & 0x0f) as u16) > count {
                return false;
            }
            let t3 = s.read_u8();
            if t2 & 0x0f == 0 && t3 != 0 {
                // an effect parameter without effect: NoisePacker 1
                if self.version == 2 {
                    return false;
                }
                *version = 1;
                return true;
            }
            i += 3;
        }
        true
    }

    /// CheckForNoisePackerFormat: the version, or None.
    fn check_format(&mut self, s: &mut Stream) -> Option<i32> {
        if s.len() < 15 {
            return None;
        }
        // the extra 'C' in the number of samples
        s.seek(0);
        let t = s.read_b_u16();
        if t & 0xf00f != 0x000c {
            return None;
        }
        let t = t & 0x0ff0;
        if t == 0 || t > 0x01f0 {
            return None;
        }
        let count = t >> 4;
        // the number of positions, stored twice
        s.seek(8 + count as i64 * 16);
        let t = s.read_b_u16();
        if t == 0 || t > 0xfe || t & 1 != 0 {
            return None;
        }
        s.seek(2);
        let length1 = s.read_b_u16();
        if length1 != t {
            return None;
        }
        // track table and track data lengths
        let length2 = s.read_b_u16();
        if length2 == 0 || length2 & 1 != 0 {
            return None;
        }
        let length3 = s.read_b_u16();
        if length3 == 0 || length3 & 1 != 0 {
            return None;
        }
        let (samples_size, mut version) = self.check_sample_info(s, count);
        if samples_size == 0 {
            return None;
        }
        // the position list, and the highest pattern
        s.seek(8 + count as i64 * 16);
        let mut highest = 0u16;
        let n = s.read_b_u16() / 2;
        s.seek_cur(2);
        for _ in 0..n {
            let t = s.read_b_u16();
            if t % 8 != 0 {
                return None;
            }
            highest = highest.max(t);
        }
        self.base.number_of_patterns = highest / 8 + 1;
        // skip the track table
        s.seek_cur(self.base.number_of_patterns as i64 * 8);
        if !self.check_pattern_data(s, count, length3, &mut version) {
            return None;
        }
        // the module length
        let total = (8 + count as u32 * 16 + 4)
            .wrapping_add(length1 as u32)
            .wrapping_add(length2 as u32)
            .wrapping_add(length3 as u32)
            .wrapping_add(samples_size);
        if total as i64 > s.len() + MAX_MISSING {
            return None;
        }
        Some(version)
    }
}

impl Format for NoisePacker {
    fn name(&self) -> &'static str {
        match self.version {
            1 => "NoisePacker 1",
            2 => "NoisePacker 2",
            _ => "NoisePacker 3",
        }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        self.check_format(s) == Some(self.version)
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        // NostalgicPlayer's loader seeks to the start before converting
        s.seek(0);
        self.number_of_samples = ((s.read_b_u16() & 0x0ff0) >> 4) as usize;
        self.number_of_positions = (s.read_b_u16() / 2) as usize;
        let track_length = s.read_b_u16() as i64;
        let track_data_length = s.read_b_u16() as i64;
        self.position_list_offset = 8 + self.number_of_samples as i64 * 16 + 4;
        let track_list_offset = self.position_list_offset + self.number_of_positions as i64 * 2;
        self.track_data_offset = track_list_offset + track_length;
        self.sample_data_offset = self.track_data_offset + track_data_length;
        s.seek(track_list_offset);
        self.track_offset_table = vec![0; self.base.number_of_patterns as usize * 4];
        s.read_array_b_u16(&mut self.track_offset_table);
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(8);
        let mut v = Vec::new();
        for _ in 0..self.number_of_samples {
            let (length, fine_tune, volume, loop_length, loop_start);
            if self.version == 3 {
                fine_tune = s.read_u8();
                volume = s.read_u8();
                s.seek_cur(4);
                length = s.read_b_u16();
                s.seek_cur(4);
                loop_length = s.read_b_u16();
                loop_start = s.read_b_u16();
            } else {
                s.seek_cur(4);
                length = s.read_b_u16();
                fine_tune = s.read_u8();
                volume = s.read_u8();
                s.seek_cur(4);
                loop_length = s.read_b_u16();
                // NoisePacker 1 stores the loop start in bytes
                let ls = s.read_b_u16();
                loop_start = if self.version == 1 { ls / 2 } else { ls };
            }
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        s.seek(self.position_list_offset);
        Some((0..self.number_of_positions).map(|_| (s.read_b_u16() / 8) as u8).collect())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut ti = 0;
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..4 {
                let off = *self.track_offset_table.get(ti)? as i64;
                ti += 1;
                s.seek(self.track_data_offset + off);
                let mut k: i32 = 0;
                while k < 64 {
                    let dat1 = s.read_u8();
                    if self.version == 3 {
                        if dat1 >= 0xc1 {
                            // empty rows
                            k -= dat1 as i8 as i32;
                            continue;
                        }
                        if dat1 == 0xc0 {
                            break;
                        }
                    }
                    let dat2 = s.read_u8();
                    let dat3 = s.read_u8();
                    let (b1, b2) = note_and_sample(dat1)?;
                    let (b3, b4) = effect(dat2, dat3, if self.version == 3 { 4 } else { 2 });
                    let o = k as usize * 16 + (3 - j) * 4;
                    pat[o..o + 4].copy_from_slice(&[b1, b2, b3, b4]);
                    // NoisePacker 3 ends a track at a pattern break or jump
                    if self.version == 3 && matches!(b3 & 0x0f, 0x0b | 0x0d) {
                        break;
                    }
                    k += 1;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_data_offset);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// NoisePacker's note number (and sample high bit) for a period.
    fn note_number(period: u16) -> u8 {
        let i = super::PERIODS.iter().position(|p| ((p[0] as u16) << 8 | p[1] as u16) == period).unwrap();
        (i as u8 + 1) * 2
    }

    /// Pack the song as NoisePacker `version` (1, 2 or 3) does. Patterns are not
    /// shared, each voice's track is stored once.
    fn pack(song: &Song, version: u8) -> Vec<u8> {
        let ns = song.samples.len();
        let mut d = Vec::new();
        d.extend((((ns as u16) << 4) | 0x000c).to_be_bytes());
        d.extend((song.positions.len() as u16 * 2).to_be_bytes());
        let npat = song.patterns.len();
        d.extend((npat as u16 * 8).to_be_bytes());
        let mut tracks = Vec::new();
        let mut table = Vec::new();
        for p in &song.patterns {
            // the track table lists voice 3 first
            for v in (0..4).rev() {
                table.extend((tracks.len() as u16).to_be_bytes());
                let mut empty = 0u8;
                for row in p {
                    let (smp, per, fx, par) = row[v];
                    if version == 3 && smp == 0 && per == 0 && fx == 0 && par == 0 {
                        empty += 1;
                        continue;
                    }
                    if empty > 0 {
                        tracks.push((-(empty as i8)) as u8);
                        empty = 0;
                    }
                    let n = if per != 0 { note_number(per) } else { 0 } | (smp >> 4);
                    // only C (volume) effects in the song: stored as they are
                    tracks.extend([n, (smp << 4) | fx, par]);
                }
                if version == 3 && empty > 0 {
                    tracks.push(0xc0);
                }
            }
        }
        if tracks.len() % 2 != 0 {
            tracks.push(0);
        }
        d.extend((tracks.len() as u16).to_be_bytes());
        let mut ptr = 0u32;
        for i in 0..ns {
            let (len, ft, vol, ls, ll) = song.samples[i];
            let stored_ls = if version == 1 { ls * 2 } else { ls };
            if version == 3 {
                d.extend([ft, vol]);
                d.extend(ptr.to_be_bytes());
                d.extend(len.to_be_bytes());
                d.extend((ptr + ls as u32 * 2).to_be_bytes());
            } else {
                d.extend(ptr.to_be_bytes());
                d.extend(len.to_be_bytes());
                d.extend([ft, vol]);
                d.extend((ptr + ls as u32 * 2).to_be_bytes());
            }
            d.extend(ll.to_be_bytes());
            d.extend(stored_ls.to_be_bytes());
            ptr += len as u32 * 2;
        }
        d.extend((song.positions.len() as u16 * 2).to_be_bytes());
        d.extend([0, 0]);
        for &p in &song.positions {
            d.extend((p as u16 * 8).to_be_bytes());
        }
        d.extend(table);
        d.extend(tracks);
        for i in 0..ns {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn noisepacker() {
        for (version, name) in [(2u8, "NoisePacker 2"), (3, "NoisePacker 3")] {
            let song = Song::new(3);
            let packed = pack(&song, version);
            let (f, m) = convert_any(&packed).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f));
        }
        // NoisePacker 1: told apart by a loop stored in bytes
        let mut song = Song::new(3);
        song.samples[1] = (16, 0, 32, 4, 12);
        let packed = pack(&song, 1);
        let (f, m) = convert_any(&packed).expect("NoisePacker 1");
        assert_eq!(f, "NoisePacker 1");
        assert_eq!(m, song.module(0x7f));
    }
}
