//! Promizer 0.1, 1.0c, 1.8a, 2.0 and 4.0, ported from NostalgicPlayer's
//! Promizer01Format, Promizer1xFormatBase, Promizer10Format, Promizer18Format,
//! Promizer20Format and Promizer40Format.
//!
//! 0.1 is a ProTracker module with its position list as pattern offsets and its
//! pattern data inverted. 1.x and 2.0 come with their replay code in front; they,
//! and 4.0, store a pattern per position as a list of indexes into a table of
//! distinct notes, cut short after a pattern break. Promizer stores periods
//! tuned to each sample's fine tune; 0.1 and 1.x get the untuned ones back.

use super::core::*;

#[derive(Clone, Copy, PartialEq, Default)]
enum Kind {
    #[default]
    P01,
    P10,
    P18,
    P20,
    P40,
}

#[derive(Default)]
pub struct Promizer {
    base: Base,
    kind: Kind,
    fine_tunes: [u8; 31],
    real_patterns: u16,
    positions: u8,
    position_list: Vec<u8>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    let f = |kind| Box::new(Promizer { kind, ..Default::default() });
    vec![(23, f(Kind::P01)), (24, f(Kind::P10)), (25, f(Kind::P18)), (26, f(Kind::P20)), (27, f(Kind::P40))]
}

/// Promizer1xFormatBase's offsets: the note count, the sample table, the base.
fn offsets(kind: Kind) -> (u32, u32, u32) {
    match kind {
        Kind::P10 => (0x1168, 0x116c, 0x1164),
        _ => (0x116c, 0x1170, 0x1168),
    }
}

/// Read `n` words, failing where the C# array would be too large to allocate.
fn read_words(s: &mut Stream, n: u32) -> Option<Vec<u16>> {
    if n as i64 > s.len() {
        return None;
    }
    let mut v = vec![0u16; n as usize];
    s.read_array_b_u16(&mut v);
    Some(v)
}

impl Promizer {
    fn check_01(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x2fe {
            return false;
        }
        s.seek(0xf8);
        let t = s.read_b_u16();
        if t > 0x200 || t & 1 != 0 {
            return false;
        }
        // the first 4 pattern offsets
        let t = s.read_b_u32() | s.read_b_u32() | s.read_b_u32() | s.read_b_u32();
        if t & 0xffff_03ff != 0 {
            return false;
        }
        s.seek(0);
        let mut size = 0u32;
        for _ in 0..31 {
            let mut t = s.read_b_u16();
            if t >= 0x8000 {
                return false;
            }
            if t == 0 {
                for _ in 0..3 {
                    t = t.wrapping_add(s.read_b_u16());
                }
                if t != 1 {
                    return false;
                }
            } else {
                if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                    return false;
                }
                if s.read_b_u16() as u32 + s.read_b_u16() as u32 > t as u32 {
                    return false;
                }
                size = size.wrapping_add(t as u32 * 2);
            }
        }
        // the position list: the highest pattern
        s.seek(0xfa);
        let mut count = 0u32;
        for _ in 0..128 {
            let t = s.read_b_u32();
            if t % 1024 != 0 {
                return false;
            }
            count = count.max(t);
        }
        self.base.number_of_patterns = (count / 1024 + 1) as u16;
        let np = self.base.number_of_patterns as u32;
        if np > 64 {
            return false;
        }
        // the size of all patterns
        s.seek(0x2fa);
        let t = s.read_b_u32();
        if t == 0 || t & 3 != 0 || t != np * 1024 {
            return false;
        }
        // the first pattern
        for _ in 0..64 * 4 {
            let t = (s.read_b_u16() ^ 0xffff) & 0x0fff;
            if t != 0 && !(0x6c..=0x38b).contains(&t) {
                return false;
            }
            s.seek_cur(2);
        }
        0x2fe + np as i64 * 1024 + size as i64 <= s.len() + MAX_MISSING
    }

    /// CheckForPromizerFormat: 10, 18 or None.
    fn check_1x(&mut self, s: &mut Stream) -> Option<Kind> {
        if s.len() < 0x1168 {
            return None;
        }
        // some of the replay code
        s.seek(14);
        if s.read_b_u32() != 0x48e7_80c0 {
            return None;
        }
        s.seek(21);
        let kind = if s.read_u8() == 0xce { Kind::P10 } else { Kind::P18 };
        let base = offsets(self.kind).2;
        s.seek(base as i64 + 8);
        let mut size = 0u32;
        for _ in 0..31 {
            let t = s.read_b_u16();
            if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                return None;
            }
            if t != 0 {
                size = size.wrapping_add(t as u32 * 2);
                if s.read_b_u16() as u32 + s.read_b_u16() as u32 > t as u32 {
                    return None;
                }
            } else {
                s.seek_cur(4);
            }
        }
        s.seek(base as i64);
        let t = s.read_b_u32();
        if base.wrapping_add(t).wrapping_add(4).wrapping_add(size) as i64 > s.len() + MAX_MISSING {
            return None;
        }
        Some(kind)
    }

    fn check_20(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x164e {
            return false;
        }
        // some of the replay code
        s.seek(0);
        if s.read_b_u32() != 0x6000_0016 || s.read_b_u32() != 0x6000_0140 {
            return false;
        }
        s.seek(0x16);
        if s.read_b_u16() != 0x4e75 || s.read_b_u32() != 0x48e7_7ffe {
            return false;
        }
        s.seek(0x1552);
        let mut size = 0u32;
        for _ in 0..31 {
            size = size.wrapping_add(s.read_b_u16() as u32 * 2);
            s.seek_cur(1);
            if s.read_u8() > 0x40 {
                return false;
            }
            s.seek_cur(4);
        }
        let t = s.read_b_u32();
        0x1452u32.wrapping_add(t).wrapping_add(size) as i64 <= s.len() + MAX_MISSING
    }

    fn check_40(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x204 {
            return false;
        }
        s.seek(0);
        if s.read_mark() != b"PM40" {
            return false;
        }
        if s.read_b_u32() > 0x7f {
            return false;
        }
        for _ in 0..128 {
            if s.read_b_u16() & 1 != 0 {
                return false;
            }
        }
        let mut size = 0u32;
        for _ in 0..31 {
            let mut t = s.read_b_u16();
            if t >= 0x8000 {
                return false;
            }
            if t == 0 {
                for _ in 0..3 {
                    t = t.wrapping_add(s.read_b_u16());
                }
                if t != 0 {
                    return false;
                }
            } else {
                size = size.wrapping_add(t as u32 * 2);
                if s.read_u8() > 0x0f || s.read_u8() > 0x40 {
                    return false;
                }
                s.seek_cur(4);
            }
        }
        let t = s.read_b_u32();
        t.wrapping_add(4).wrapping_add(size) as i64 <= s.len() + MAX_MISSING
    }

    /// AdjustFineTunes: tuned periods back to the untuned ones.
    fn adjust_fine_tunes(&self, pat: &mut [u8]) -> Option<()> {
        for n in pat.chunks_exact_mut(4) {
            let sample = (n[0] & 0x10) | (n[2] >> 4);
            if sample != 0 && self.fine_tunes[sample as usize - 1] != 0 {
                let ft = self.fine_tunes[sample as usize - 1] as usize;
                let period = (((n[0] & 0x0f) as i16) << 8) | n[1] as i16;
                let row = TUNING_PERIODS.get(ft)?;
                if let Some(j) = row.iter().position(|&p| p == period) {
                    n[0] = (n[0] & 0xf0) | PERIODS[j][0];
                    n[1] = PERIODS[j][1];
                }
            }
        }
        Some(())
    }

    fn patterns_01(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        s.seek(0x2fe);
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for n in pat.chunks_exact_mut(4) {
                n[0] = s.read_u8() ^ 0xff;
                n[1] = s.read_u8() ^ 0xff;
                n[2] = s.read_u8() ^ 0xff;
                n[3] = s.read_u8() ^ 0xf0;
            }
            self.adjust_fine_tunes(&mut pat)?;
            push!(out, pat, s);
        }
        // skip the stored patterns no position plays
        if self.base.number_of_patterns < self.real_patterns {
            s.seek_cur((self.real_patterns - self.base.number_of_patterns) as i64 * 1024);
        }
        Some(out)
    }

    fn patterns_1x(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let (o1, o2, _) = offsets(self.kind);
        let base = o2 + 31 * 8 + 2;
        let notes_table = base + 512;
        let mut pattern_offsets = [0u32; 128];
        s.seek(base as i64);
        s.read_array_b_u32(&mut pattern_offsets);
        s.seek(o1 as i64);
        let notes = s.read_b_u32();
        s.seek(notes_table as i64);
        let single = read_words(s, notes / 2)?;
        let ref_offset = s.position();
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions as usize {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                let mut w = 0;
                let mut offset = pattern_offsets[i] / 2;
                'rows: for _ in 0..64 {
                    let mut brk = false;
                    for _ in 0..4 {
                        let mut n = [0u8; 4];
                        if notes_table as i64 + offset as i64 * 2 >= ref_offset {
                            offset = offset.wrapping_add(1);
                        } else {
                            let r = *single.get(offset as usize)? as i64;
                            offset += 1;
                            s.seek(r * 4 + ref_offset);
                            for b in &mut n {
                                *b = s.read_u8();
                            }
                        }
                        pat[w..w + 4].copy_from_slice(&n);
                        w += 4;
                        if matches!(n[2] & 0x0f, 0x0d | 0x0b) {
                            brk = true;
                        }
                    }
                    if brk {
                        break 'rows;
                    }
                }
                self.adjust_fine_tunes(&mut pat)?;
                push!(out, pat, s);
            }
        }
        Some(out)
    }

    /// Promizer 2.0 and 4.0 patterns: (offset table, note table, notes, end of
    /// the note indexes) and how a note is decoded.
    fn patterns_20_40(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let p40 = self.kind == Kind::P40;
        let mut pattern_offsets = [0u16; 128];
        let (notes_table, notes_offset, note_end);
        if p40 {
            s.seek(0x8);
            s.read_array_b_u16(&mut pattern_offsets);
            s.seek(0x200);
            let sample_offset = s.read_b_u32().wrapping_add(4);
            notes_offset = s.read_b_u32().wrapping_add(4);
            notes_table = 0x208u32;
            note_end = sample_offset;
        } else {
            s.seek(0x1452);
            s.read_array_b_u16(&mut pattern_offsets);
            s.seek(0x1552 + 31 * 8 + 4);
            let add = s.read_b_u32();
            notes_table = 0x1552 + 31 * 8 + 8;
            notes_offset = 0x1452u32.wrapping_add(add);
            note_end = notes_offset.wrapping_sub(4);
        }
        s.seek(notes_table as i64);
        let single = read_words(s, notes_offset.wrapping_sub(notes_table) / 2)?;
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions as usize {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                let mut offset = pattern_offsets[i] / 2;
                for j in 0..64 {
                    let mut brk = false;
                    for k in 0..4 {
                        if (notes_table as i64 + offset as i64 * 2) >= note_end as i64 {
                            continue;
                        }
                        let po = *single.get(offset as usize)? as u32;
                        offset = offset.wrapping_add(1);
                        let at = if p40 {
                            notes_offset.wrapping_add(po * 4)
                        } else if po != 0 {
                            notes_offset.wrapping_add((po - 1) * 4)
                        } else {
                            continue;
                        };
                        s.seek(at as i64);
                        let mut b3 = s.read_u8();
                        if !p40 {
                            b3 >>= 2;
                        }
                        let mut b1 = 0u8;
                        if b3 >= 0x10 {
                            b3 -= 0x10;
                            b1 = 0x10;
                        }
                        b3 <<= 4;
                        let mut b2 = s.read_u8();
                        if b2 != 0 {
                            b2 = if p40 { b2 - 1 } else { b2.wrapping_sub(2) / 2 };
                            let p = PERIODS.get(b2 as usize)?;
                            b1 |= p[0];
                            b2 = p[1];
                        }
                        b3 |= s.read_u8() & 0x0f;
                        let b4 = s.read_u8();
                        let o = j * 16 + k * 4;
                        pat[o..o + 4].copy_from_slice(&[b1, b2, b3, b4]);
                        if matches!(b3 & 0x0f, 0x0d | 0x0b) {
                            brk = true;
                        }
                    }
                    if brk {
                        break;
                    }
                }
                push!(out, pat, s);
            }
        }
        Some(out)
    }
}

impl Format for Promizer {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::P01 => "Promizer 0.1",
            Kind::P10 => "Promizer 1.0c",
            Kind::P18 => "Promizer 1.8a",
            Kind::P20 => "Promizer 2.0",
            Kind::P40 => "Promizer 4.0",
        }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        match self.kind {
            Kind::P01 => self.check_01(s),
            Kind::P10 | Kind::P18 => self.check_1x(s) == Some(self.kind),
            Kind::P20 => self.check_20(s),
            Kind::P40 => self.check_40(s),
        }
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        // NostalgicPlayer's loader rewinds the module before converting it
        s.seek(0);
        match self.kind {
            Kind::P01 => {}
            Kind::P10 | Kind::P18 => {
                let (_, o2, o3) = offsets(self.kind);
                s.seek(o3 as i64 + 0x100);
                self.positions = (s.read_b_u16() / 4) as u8;
                self.position_list = self.base.build_position_list(self.positions as usize, |_| s.read_b_u32()).0;
                // the sample table is read where this leaves the stream
                s.seek(o2 as i64);
            }
            Kind::P20 | Kind::P40 => {
                if self.kind == Kind::P20 {
                    s.seek(0x1450);
                    self.positions = (s.read_b_u16() / 2) as u8;
                } else {
                    s.seek(7);
                    self.positions = s.read_u8();
                }
                self.position_list = self.base.build_position_list(self.positions as usize, |_| s.read_b_u16()).0;
            }
        }
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let mut v = Vec::new();
        match self.kind {
            Kind::P01 | Kind::P10 | Kind::P18 => {
                for i in 0..31 {
                    let length = s.read_b_u16();
                    let fine_tune = s.read_u8();
                    let volume = s.read_u8();
                    let loop_start = s.read_b_u16();
                    let loop_length = s.read_b_u16();
                    self.fine_tunes[i] = fine_tune;
                    push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
                }
            }
            Kind::P20 | Kind::P40 => {
                s.seek(if self.kind == Kind::P20 { 0x1552 } else { 0x108 });
                for _ in 0..31 {
                    let length = s.read_b_u16();
                    let mut fine_tune = s.read_u8();
                    if self.kind == Kind::P20 {
                        fine_tune /= 2;
                    }
                    let volume = s.read_u8();
                    let loop_start = s.read_b_u16();
                    let loop_length = s.read_b_u16().max(1);
                    push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
                }
            }
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        if self.kind != Kind::P01 {
            return Some(self.position_list.clone());
        }
        s.seek(0xf8);
        let n = s.read_b_u16() as usize / 4;
        self.real_patterns = self.base.number_of_patterns;
        let list: Vec<u8> = (0..n).map(|_| (s.read_b_u32() / 1024) as u8).collect();
        self.base.find_highest_pattern(&list);
        Some(list)
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        match self.kind {
            Kind::P01 => self.patterns_01(s),
            Kind::P10 | Kind::P18 => self.patterns_1x(s),
            Kind::P20 | Kind::P40 => self.patterns_20_40(s),
        }
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        match self.kind {
            Kind::P01 => {}
            Kind::P10 | Kind::P18 => {
                let o3 = offsets(self.kind).2;
                s.seek(o3 as i64);
                let off = s.read_b_u32();
                s.seek(o3.wrapping_add(off).wrapping_add(4) as i64);
            }
            Kind::P20 => {
                s.seek(0x1552 + 31 * 8);
                let off = s.read_b_u32();
                s.seek(0x1452u32.wrapping_add(off).wrapping_sub(4) as i64);
            }
            Kind::P40 => {
                s.seek(0x200);
                let off = s.read_b_u32();
                s.seek(off.wrapping_add(4) as i64);
            }
        }
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use super::super::core::{PERIODS, TUNING_PERIODS};
    use crate::prowizard::tests::*;

    fn period_index(period: u16) -> Option<usize> {
        PERIODS[..36].iter().position(|p| ((p[0] as u16) << 8 | p[1] as u16) == period)
    }

    /// A note as Promizer 0.1 and 1.x store it: the period tuned to the sample's
    /// fine tune.
    fn tuned(song: &Song, n: (u8, u16, u8, u8)) -> [u8; 4] {
        let ft = if n.0 != 0 { song.samples[n.0 as usize - 1].1 } else { 0 };
        let mut n = n;
        if ft != 0 && n.1 != 0 {
            n.1 = TUNING_PERIODS[ft as usize][period_index(n.1).unwrap()] as u16;
        }
        Song::note(n)
    }

    fn samples(song: &Song, ft_mul: u8, empty_ll: u16) -> Vec<u8> {
        let mut d = Vec::new();
        for i in 0..31 {
            let (len, ft, vol, ls, ll) = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, empty_ll));
            d.extend(len.to_be_bytes());
            d.extend([ft * ft_mul, vol]);
            d.extend(ls.to_be_bytes());
            d.extend(ll.to_be_bytes());
        }
        d
    }

    fn sample_data(song: &Song, d: &mut Vec<u8>) {
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
    }

    fn put(d: &mut Vec<u8>, at: usize, b: &[u8]) {
        if d.len() < at + b.len() {
            d.resize(at + b.len(), 0);
        }
        d[at..at + b.len()].copy_from_slice(b);
    }

    fn pack_01(song: &Song) -> Vec<u8> {
        let mut d = samples(song, 1, 1);
        d.extend((song.positions.len() as u16 * 4).to_be_bytes());
        for i in 0..128 {
            d.extend((*song.positions.get(i).unwrap_or(&0) as u32 * 1024).to_be_bytes());
        }
        d.extend((song.patterns.len() as u32 * 1024).to_be_bytes());
        for p in &song.patterns {
            for row in p {
                for n in row {
                    let b = tuned(song, *n);
                    d.extend([b[0] ^ 0xff, b[1] ^ 0xff, b[2] ^ 0xff, b[3] ^ 0xf0]);
                }
            }
        }
        sample_data(song, &mut d);
        d
    }

    /// The index table of each pattern (256 words each) and the distinct notes;
    /// with `empty_first`, note 0 is the empty note.
    fn index_tables(song: &Song, note: impl Fn((u8, u16, u8, u8)) -> [u8; 4], empty_first: bool) -> (Vec<u8>, Vec<u8>) {
        let mut notes: Vec<[u8; 4]> = if empty_first { vec![note((0, 0, 0, 0))] } else { Vec::new() };
        let mut idx = Vec::new();
        for p in &song.patterns {
            for row in p {
                for &n in row {
                    let b = note(n);
                    let i = notes.iter().position(|x| *x == b).unwrap_or_else(|| {
                        notes.push(b);
                        notes.len() - 1
                    });
                    idx.extend((i as u16).to_be_bytes());
                }
            }
        }
        (idx, notes.concat())
    }

    /// Promizer 1.0c (`v10`) or 1.8a, with just the replay code the check reads.
    fn pack_1x(song: &Song, v10: bool) -> Vec<u8> {
        let (o3, o1, o2) = if v10 { (0x1164, 0x1168, 0x116c) } else { (0x1168, 0x116c, 0x1170) };
        let mut d = vec![0u8; o3];
        put(&mut d, 14, &[0x48, 0xe7, 0x80, 0xc0]);
        put(&mut d, 21, &[if v10 { 0xce } else { 0 }]);
        let (idx, notes) = index_tables(song, |n| tuned(song, n), false);
        let base = o2 + 31 * 8 + 2;
        let data = base + 512 + idx.len() + notes.len();
        put(&mut d, o3, &((data - o3 - 4) as u32).to_be_bytes());
        put(&mut d, o1, &(idx.len() as u32).to_be_bytes());
        put(&mut d, o2, &samples(song, 1, 1));
        put(&mut d, o2 + 248, &(song.positions.len() as u16 * 4).to_be_bytes());
        for (i, &p) in song.positions.iter().enumerate() {
            put(&mut d, base + i * 4, &(p as u32 * 512).to_be_bytes());
        }
        put(&mut d, base + 512, &idx);
        d.extend(notes);
        sample_data(song, &mut d);
        d
    }

    /// Promizer 2.0: note numbers, sample * 4, index 0 an empty note.
    fn pack_20(song: &Song) -> Vec<u8> {
        let mut d = vec![0u8; 0x1450];
        put(&mut d, 0, &[0x60, 0, 0, 0x16, 0x60, 0, 1, 0x40]);
        put(&mut d, 0x16, &[0x4e, 0x75, 0x48, 0xe7, 0x7f, 0xfe]);
        put(&mut d, 0x1450, &(song.positions.len() as u16 * 2).to_be_bytes());
        for (i, &p) in song.positions.iter().enumerate() {
            put(&mut d, 0x1452 + i * 2, &(p as u16 * 512).to_be_bytes());
        }
        put(&mut d, 0x1552, &samples(song, 2, 1));
        let note = |n: (u8, u16, u8, u8)| {
            let b1 = period_index(n.1).map_or(0, |j| j as u8 * 2 + 2);
            [n.0 << 2, b1, n.2, n.3]
        };
        let (mut idx, notes) = index_tables(song, note, true);
        // index 0 is no note, index i the note i - 1
        for w in idx.chunks_exact_mut(2) {
            let i = u16::from_be_bytes([w[0], w[1]]);
            w.copy_from_slice(&(i + 1).to_be_bytes());
        }
        // the notes start 4 bytes after the index table, which ends 4 before them
        let notes_at = 0x1652 + idx.len() + 4;
        let data = notes_at + notes.len();
        put(&mut d, 0x164a, &((data - 0x1452 + 4) as u32).to_be_bytes());
        put(&mut d, 0x164e, &((notes_at - 0x1452) as u32).to_be_bytes());
        put(&mut d, 0x1652, &idx);
        d.extend([0; 4]);
        d.extend(&notes);
        sample_data(song, &mut d);
        d
    }

    /// Promizer 4.0: "PM40", sample numbers whole, note numbers from 1.
    fn pack_40(song: &Song) -> Vec<u8> {
        let mut d = b"PM40".to_vec();
        d.extend((song.positions.len() as u32).to_be_bytes());
        for i in 0..128 {
            d.extend((*song.positions.get(i).unwrap_or(&0) as u16 * 512).to_be_bytes());
        }
        d.extend(samples(song, 1, 0));
        let note = |n: (u8, u16, u8, u8)| [n.0, period_index(n.1).map_or(0, |j| j as u8 + 1), n.2, n.3];
        let (idx, notes) = index_tables(song, note, true);
        let notes_at = 0x208 + idx.len();
        let data = notes_at + notes.len();
        d.extend(((data - 4) as u32).to_be_bytes());
        d.extend(((notes_at - 4) as u32).to_be_bytes());
        d.extend(idx);
        d.extend(notes);
        sample_data(song, &mut d);
        d
    }

    #[test]
    fn promizer() {
        let song = Song::new(3);
        let packs = [
            (pack_01(&song), "Promizer 0.1"),
            (pack_1x(&song, true), "Promizer 1.0c"),
            (pack_1x(&song, false), "Promizer 1.8a"),
            (pack_20(&song), "Promizer 2.0"),
            (pack_40(&song), "Promizer 4.0"),
        ];
        for (packed, name) in packs {
            let (f, m) = convert_any(&packed).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f), "{name}");
        }
    }
}
