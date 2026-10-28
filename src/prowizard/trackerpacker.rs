//! Tracker Packer 1, 2 and 3 (Mexx and Complex), ported from NostalgicPlayer's
//! TrackerPacker1Format, TrackerPackerNewFormatBase, TrackerPacker2Format and
//! TrackerPacker3Format.
//!
//! All three have an id ("MEXX", "MEXX_TP2", "CPLX_TP3"), the title and a sample
//! table of their own. Version 1 stores a pattern per position at an offset,
//! version 2 and 3 a track per voice and pattern; their notes are packed into one
//! to three bytes (empty rows, an effect only, or a note with its sample).

use super::core::*;

#[derive(Default)]
pub struct TrackerPacker {
    base: Base,
    version: u32,
    samples: u16,
    positions: u8,
    sample_offset: u32,
    // version 1
    pattern_offsets: Vec<u32>,
    position_list: Vec<u8>,
    // versions 2 and 3: [voice][pattern]
    track_offsets: [Vec<u16>; 4],
    note_start: u32,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    let f = |version| Box::new(TrackerPacker { version, ..Default::default() });
    vec![(42, f(1)), (43, f(2)), (44, f(3))]
}

/// ConvertEffect: Tracker Packer's arpeggio and volume slide arguments.
fn convert_effect(effect: &mut u8, val: &mut u8) {
    match *effect {
        0x8 => *effect = 0,
        0x5 | 0x6 | 0xa => {
            *val = if (*val as i8) < 0 { (*val as i8).wrapping_neg() as u8 } else { *val << 4 };
        }
        _ => {}
    }
}

impl TrackerPacker {
    fn check_1(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x11a {
            return false;
        }
        s.seek(0);
        if s.read_mark() != b"MEXX" {
            return false;
        }
        // the module length stored in the module
        let t = s.read_b_u32();
        if t == 0 || t & 1 != 0 || t as i64 > s.len() + MAX_MISSING {
            return false;
        }
        // the sample offset
        s.seek(0x1c);
        let t = s.read_b_u32();
        if t == 0 || t & 1 != 0 {
            return false;
        }
        let mut size = 0u32;
        for _ in 0..31 {
            let volume = s.read_b_u16();
            let len = s.read_b_u16();
            if len == 0 {
                s.seek_cur(2);
                if s.read_b_u16() != 1 {
                    return false;
                }
            } else {
                size = size.wrapping_add(len as u32 * 2);
                if volume > 0x40 {
                    return false;
                }
                s.seek_cur(4);
            }
        }
        // the position list size
        size != 0 && s.read_b_u16() != 0
    }

    /// CheckForTrackerPackerFormat (2 and 3).
    fn check_new(&mut self, s: &mut Stream) -> bool {
        s.seek(0);
        let id: &[u8] = if self.version == 2 { b"MEXX_TP2" } else { b"CPLX_TP3" };
        if s.read_mark_n(8) != id {
            return false;
        }
        if s.len() < 0x1e {
            return false;
        }
        s.seek(0x1c);
        let n = s.read_b_u16();
        if n == 0 {
            return false;
        }
        let n = n / 8;
        let mut size = 0u32;
        for _ in 0..n {
            s.seek_cur(2);
            let len = s.read_b_u16();
            if len == 0 {
                return false;
            }
            size = size.wrapping_add(len as u32 * 2);
            s.seek_cur(4);
        }
        s.seek(0x1e + n as i64 * 8);
        let positions = s.read_b_u16();
        let mut np = 0u16;
        for _ in 0..positions {
            np = np.max(s.read_b_u16() / 8);
        }
        self.base.number_of_patterns = np.wrapping_add(1);
        let mut off = (s.position() as u32).wrapping_add(self.base.number_of_patterns as u32 * 8 + 2);
        s.seek(off as i64 - 2);
        off = off.wrapping_add(s.read_b_u16() as u32);
        off.wrapping_add(size) as i64 <= s.len() + MAX_MISSING
    }

    fn patterns_1(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.position_list.len() {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                s.seek(0x31a + self.pattern_offsets[i] as i64);
                for n in pat.chunks_exact_mut(4) {
                    let mut t = s.read_u8();
                    // an empty note
                    if t == 0xc0 {
                        continue;
                    }
                    // an effect only
                    if t >= 0x80 {
                        n[2] = (t >> 2) & 0x0f;
                        n[3] = s.read_u8();
                        continue;
                    }
                    // note, sample and effect
                    if t & 1 != 0 {
                        n[0] = 0x10;
                        t &= 0xfe;
                    }
                    if t != 0 {
                        let p = PERIODS.get(((t - 2) / 2) as usize)?;
                        n[0] |= p[0];
                        n[1] = p[1];
                    }
                    n[2] = s.read_u8();
                    n[3] = s.read_u8();
                }
                push!(out, pat, s);
            }
        }
        Some(out)
    }

    /// GetAllPatterns (2 and 3).
    fn patterns_new(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let v2 = self.version == 2;
        let mut out = Vec::new();
        for i in 0..self.base.number_of_patterns as usize {
            let mut pat = vec![0u8; 1024];
            for j in 0..4 {
                s.seek(self.note_start as i64 + *self.track_offsets[j].get(i)? as i64);
                let mut k = 0usize;
                while k < 64 {
                    let o = k * 16 + j * 4;
                    let mut t = s.read_u8();
                    if t >= 0xc0 {
                        // empty rows
                        k = k.wrapping_add((-(t as i8 as i32) - 1) as usize) + 1;
                        continue;
                    }
                    if t >= 0x80 {
                        // an effect only
                        t -= 0x80;
                        t >>= if v2 { 2 } else { 1 };
                        t &= 0x0f;
                        let mut val = s.read_u8();
                        convert_effect(&mut t, &mut val);
                        pat[o + 2] = t;
                        pat[o + 3] = val;
                        k += 1;
                        continue;
                    }
                    // the sample's high bit
                    if v2 {
                        if t & 1 != 0 {
                            pat[o] = 0x10;
                            t &= 0xfe;
                        }
                    } else if t >= 0x5b {
                        pat[o] = 0x10;
                        t = (-(t as i8 as i32) - 0x81) as u8;
                    }
                    if t != 0 {
                        t = if v2 { t.wrapping_sub(2) / 2 } else { t - 1 };
                        let p = PERIODS.get(t as usize)?;
                        pat[o] |= p[0];
                        pat[o + 1] = p[1];
                    }
                    // sample, effect and its argument
                    let b = s.read_u8();
                    pat[o + 2] = b & 0xf0;
                    let mut e = b & 0x0f;
                    if e != 0 {
                        let mut val = s.read_u8();
                        convert_effect(&mut e, &mut val);
                        pat[o + 2] |= e;
                        pat[o + 3] = val;
                    }
                    k += 1;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
}

impl Format for TrackerPacker {
    fn name(&self) -> &'static str {
        match self.version {
            1 => "Tracker Packer 1",
            2 => "Tracker Packer 2",
            _ => "Tracker Packer 3",
        }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if self.version == 1 { self.check_1(s) } else { self.check_new(s) }
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        // NostalgicPlayer's loader rewinds the module before converting it
        s.seek(0);
        s.seek(0x1c);
        if self.version == 1 {
            self.sample_offset = s.read_b_u32();
            // the pattern offsets, from the lowest one
            s.seek(0x118);
            self.positions = s.read_b_u16().wrapping_add(1) as u8;
            self.pattern_offsets = (0..self.positions).map(|_| s.read_b_u32()).collect();
            let start = self.pattern_offsets.iter().copied().min().unwrap_or(u32::MAX);
            for o in &mut self.pattern_offsets {
                *o = o.wrapping_sub(start);
            }
            let po = &self.pattern_offsets;
            self.position_list = self.base.build_position_list(self.positions as usize, |p| po[p]).0;
        } else {
            self.samples = s.read_b_u16() / 8;
            // CreateOffsetTable
            s.seek(0x1e + self.samples as i64 * 8);
            self.positions = s.read_b_u16() as u8;
            s.seek_cur(self.positions as i64 * 2);
            let np = self.base.number_of_patterns as usize;
            for t in &mut self.track_offsets {
                *t = vec![0; np];
            }
            for i in 0..np {
                for j in 0..4 {
                    self.track_offsets[j][i] = s.read_b_u16();
                }
            }
            self.note_start = (s.position() as u32).wrapping_add(2);
            self.sample_offset = self.note_start.wrapping_add(s.read_b_u16() as u32);
        }
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0u8; 20];
        s.seek(8);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let n = if self.version == 1 {
            s.seek(0x20);
            31
        } else {
            s.seek(0x1e);
            self.samples
        };
        let mut v = Vec::new();
        for _ in 0..n {
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let length = s.read_b_u16();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        if self.version == 1 {
            return Some(self.position_list.clone());
        }
        s.seek(0x1e + self.samples as i64 * 8 + 2);
        Some((0..self.positions).map(|_| (s.read_b_u16() / 8) as u8).collect())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        if self.version == 1 { self.patterns_1(s) } else { self.patterns_new(s) }
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_offset as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use super::super::core::PERIODS;
    use crate::prowizard::tests::*;

    fn period_index(period: u16) -> u8 {
        PERIODS[..36].iter().position(|p| ((p[0] as u16) << 8 | p[1] as u16) == period).unwrap() as u8
    }

    fn sample_data(song: &Song, d: &mut Vec<u8>) {
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
    }

    /// Tracker Packer 1: a pattern per position, its notes in one to three bytes.
    fn pack_1(song: &Song) -> Vec<u8> {
        let mut pats = Vec::new();
        let mut offsets = Vec::new();
        for p in &song.patterns {
            offsets.push(pats.len() as u32);
            for row in p {
                for &(s, per, e, v) in row {
                    if (s, per, e, v) == (0, 0, 0, 0) {
                        pats.push(0xc0);
                    } else if s == 0 && per == 0 {
                        pats.extend([0x80 | e << 2, v]);
                    } else {
                        let n = if per == 0 { 0 } else { period_index(per) * 2 + 2 };
                        pats.extend([n | (s >> 4), (s << 4) | e, v]);
                    }
                }
            }
        }
        let mut d = b"MEXX".to_vec();
        d.extend([0; 4]);
        d.extend([0; 20]);
        // the sample data (and module length) is even
        if pats.len() % 2 != 0 {
            pats.push(0);
        }
        let samples_at = 0x31a + pats.len();
        d.extend((samples_at as u32).to_be_bytes());
        for i in 0..31 {
            let (len, ft, vol, ls, ll) = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend([ft, vol]);
            d.extend(len.to_be_bytes());
            d.extend(ls.to_be_bytes());
            d.extend(ll.to_be_bytes());
        }
        d.extend((song.positions.len() as u16 - 1).to_be_bytes());
        for &p in &song.positions {
            // stored from some base the converter subtracts
            d.extend((0x400 + offsets[p as usize]).to_be_bytes());
        }
        d.resize(0x31a, 0);
        d.extend(pats);
        sample_data(song, &mut d);
        let len = d.len() as u32;
        d[4..8].copy_from_slice(&len.to_be_bytes());
        d
    }

    /// Tracker Packer 2 (`v2`) or 3: a track per voice and pattern.
    fn pack_new(song: &Song, v2: bool) -> Vec<u8> {
        let mut notes = Vec::new();
        let mut tracks: Vec<[u16; 4]> = Vec::new();
        for p in &song.patterns {
            let mut t = [0u16; 4];
            for (v, o) in t.iter_mut().enumerate() {
                *o = notes.len() as u16;
                let mut r = 0;
                while r < 64 {
                    let (s, per, e, val) = p[r][v];
                    let empty = (r..64).take_while(|&k| p[k][v] == (0, 0, 0, 0)).count();
                    if empty > 0 {
                        notes.push((0x100 - empty) as u8);
                        r += empty;
                        continue;
                    }
                    if s == 0 && per == 0 {
                        notes.extend([0x80 | if v2 { e << 2 } else { e << 1 }, val]);
                    } else {
                        let n = if per == 0 {
                            0
                        } else if v2 {
                            period_index(per) * 2 + 2
                        } else {
                            period_index(per) + 1
                        };
                        notes.push(n);
                        notes.push((s << 4) | e);
                        if e != 0 {
                            notes.push(val);
                        }
                    }
                    r += 1;
                }
            }
            tracks.push(t);
        }
        let mut d = if v2 { b"MEXX_TP2".to_vec() } else { b"CPLX_TP3".to_vec() };
        d.extend([0; 20]);
        d.extend((song.samples.len() as u16 * 8).to_be_bytes());
        for &(len, ft, vol, ls, ll) in &song.samples {
            d.extend([ft, vol]);
            d.extend(len.to_be_bytes());
            d.extend(ls.to_be_bytes());
            d.extend(ll.to_be_bytes());
        }
        d.extend((song.positions.len() as u16).to_be_bytes());
        for &p in &song.positions {
            d.extend((p as u16 * 8).to_be_bytes());
        }
        for t in &tracks {
            for o in t {
                d.extend(o.to_be_bytes());
            }
        }
        d.extend((notes.len() as u16).to_be_bytes());
        d.extend(notes);
        sample_data(song, &mut d);
        d
    }

    #[test]
    fn trackerpacker() {
        let mut song = Song::new(3);
        // Tracker Packer 1's check reads the fine tune as the volume's high byte
        song.samples[1].1 = 0;
        let (f, m) = convert_any(&pack_1(&song)).expect("Tracker Packer 1");
        assert_eq!(f, "Tracker Packer 1");
        assert_eq!(m, song.module(0x7f));
        let song = Song::new(3);
        for (v2, name) in [(true, "Tracker Packer 2"), (false, "Tracker Packer 3")] {
            let (f, m) = convert_any(&pack_new(&song, v2)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f));
        }
    }
}
