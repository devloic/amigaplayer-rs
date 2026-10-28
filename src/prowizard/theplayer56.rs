//! The Player 5.0A, 6.0A and 6.1A (Jarno Paananen), ported from NostalgicPlayer's
//! ThePlayer5x_6xFormatBase, ThePlayer50AFormat, ThePlayer60AFormat and
//! ThePlayer61AFormat.
//!
//! An optional mark, the sample data offset, the pattern and sample counts (with
//! flags for packed and delta samples), 6 bytes per sample, four track offsets
//! per pattern and the position list ended by 0xff. The track data follows: rows
//! of 1-3 bytes with repeat and empty-row counts, and back references copying
//! rows from earlier in the data. Which of the three it is shows only in the data:
//! odd positions (6.x), and 0x00 pairs and back references (5.0A/6.0A).

use super::core::*;

/// The 4-bit deltas of packed samples.
const PACK_TABLE: [i8; 16] = [0, 1, 2, 4, 8, 16, 32, 64, -128, -64, -32, -16, -8, -4, -2, -1];

#[derive(Default)]
pub struct ThePlayer5x {
    base: Base,
    version: u32,
    track_data: u32,
    sample_data: u32,
    samples: u8,
    packed_samples: bool,
    delta_samples: bool,
    sample_starts: Vec<u32>,
    sample_packed: Vec<bool>,
    track_offsets: Vec<u16>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    let f = |version| Box::new(ThePlayer5x { version, ..Default::default() }) as Box<dyn Format>;
    vec![(39, f(50)), (40, f(60)), (41, f(61))]
}

/// A pattern being written; writing past its end fails the conversion, as the C#
/// converter throws there.
struct Pat {
    d: Vec<u8>,
    off: usize,
}

impl Pat {
    fn put(&mut self, b: [u8; 4]) -> Option<()> {
        self.d.get_mut(self.off..self.off + 4)?.copy_from_slice(&b);
        self.off += 16;
        Some(())
    }
    /// SetMultipleRows.
    fn rows(&mut self, count: u8, b: [u8; 4]) -> Option<()> {
        for _ in 0..count {
            self.put(b)?;
        }
        Some(())
    }
}

/// GotId: skips a "P50A"/"P60A"/"P61A" mark.
fn got_id(s: &mut Stream) -> bool {
    s.seek(0);
    let m = s.read_mark();
    if m != b"P50A" && m != b"P60A" && m != b"P61A" {
        s.seek(0);
        return false;
    }
    true
}

/// GetNote: the hi sample bit and the period of a note byte.
fn get_note(mut t: u8) -> Option<(u8, u8)> {
    let mut b1 = 0;
    if t & 1 != 0 {
        b1 = 0x10;
        t &= 0xfe;
    }
    if t == 0 {
        return Some((b1, 0));
    }
    let p = PERIODS.get(((t - 2) / 2) as usize)?;
    Some((b1 | p[0], p[1]))
}

/// CheckEffect.
fn check_effect(b3: &mut u8, b4: &mut u8) {
    match *b3 & 0x0f {
        0xd => *b4 = 0,
        0xe => {
            if *b4 == 0x02 {
                *b4 = 0x01;
            }
        }
        0x5 | 0x6 | 0xa => {
            if *b4 >= 0x80 {
                *b4 = ((*b4 as i8).wrapping_neg() as u8) << 4;
            }
        }
        0x8 => *b3 &= 0xf0,
        _ => {}
    }
}

/// A note with its sample and effect bytes from the stream.
fn full_note(s: &mut Stream, t: u8) -> Option<[u8; 4]> {
    let (b1, b2) = get_note(t)?;
    let (mut b3, mut b4) = (s.read_u8(), s.read_u8());
    check_effect(&mut b3, &mut b4);
    Some([b1, b2, b3, b4])
}

impl ThePlayer5x {
    /// CheckForThePlayerFormat: 50, 60, 61 or None.
    fn check_format(&mut self, s: &mut Stream) -> Option<u32> {
        if s.len() < 4 {
            return None;
        }
        let mut info_offset = 0i64;
        let mut ty = 50;
        if got_id(s) {
            info_offset = 4;
        }
        let sample_offset = s.read_b_u16();
        if sample_offset & 1 != 0 || sample_offset < 0x24 {
            return None;
        }
        let pats = s.read_u8();
        self.base.number_of_patterns = pats as u16;
        if pats == 0 || pats > 64 {
            return None;
        }
        let mut count = s.read_u8();
        if count > 0xdf {
            return None;
        }
        info_offset += if count & 0x40 != 0 { 8 } else { 4 };
        count &= 0x1f;
        if count == 0 {
            return None;
        }
        if sample_offset as i32 <= count as i32 * 6 + pats as i32 * 8 {
            return None;
        }
        s.seek(info_offset);
        let mut size = 0u32;
        let mut lengths = vec![0u16; count as usize];
        for i in 0..count as usize {
            let mut t = s.read_b_u16();
            let fine_tune = s.read_u8();
            if t >= 0x8000 {
                // the same as another sample
                t = (t as i16).wrapping_neg() as u16;
                if t > 31 {
                    return None;
                }
                t = *lengths.get(t as usize - 1)?;
            } else {
                size += t as u32;
                // a packed sample has a negative fine tune
                if fine_tune & 0x80 == 0 {
                    size += t as u32;
                }
            }
            lengths[i] = t;
            if fine_tune & 0x70 != 0 || s.read_u8() > 64 {
                return None;
            }
            let l = s.read_b_u16();
            if l < 0x8000 && lengths[i] as i32 - l as i32 >= 0x8000 {
                return None;
            }
        }
        // the first track offset is 0
        if s.read_b_u16() != 0 {
            return None;
        }
        s.seek_cur(-2);
        // a pattern's track offsets go up
        for _ in 0..pats {
            for _ in 0..3 {
                let t2 = s.read_b_u16();
                let t3 = s.read_b_u16().wrapping_sub(3);
                if t3 < t2 || t2 > sample_offset || t3 > sample_offset {
                    return None;
                }
                s.seek_cur(-2);
            }
            s.seek_cur(2);
        }
        // the position list
        let mut taken = 0;
        loop {
            let t = s.read_u8();
            if t == 0xff {
                break;
            }
            if t % 2 == 0 {
                if t > 0x7e {
                    return None;
                }
            } else {
                ty = 60;
                if t > 0x3f {
                    return None;
                }
            }
            taken += 1;
            if taken > 0x7f {
                return None;
            }
        }
        if sample_offset as i64 + size as i64 > s.len() + MAX_MISSING {
            return None;
        }
        // 6.0A or 6.1A: scan the track data. (Where the data ends before the
        // sample offset says, the C# loops forever; this stops at the end.)
        let mut end = (sample_offset as u32).wrapping_sub(4) as i64;
        let offset = s.position();
        let mut pairs = 0;
        while s.position() < end && s.position() + 1 < s.len() {
            let (b1, b2) = (s.read_u8(), s.read_u8());
            if b1 == 0 && b2 == 0 {
                pairs += 1;
            }
            s.seek_cur(-1);
        }
        if pairs == 0 {
            ty = 61;
        } else {
            // then look for back references
            end -= 4;
            let mut refs = 0;
            s.seek(offset);
            while s.position() < end && !s.eof {
                if s.read_u8() == 0x80 && s.read_u8() <= 0x0f {
                    let t = s.read_b_u16();
                    if s.position() - t as i64 > offset {
                        refs += 1;
                    }
                }
            }
            if refs == 0 {
                ty = 61;
            }
        }
        Some(ty)
    }

    /// 5.0A/6.0A ConvertData: a row, or a back reference to `count` rows.
    fn convert_5x(&self, s: &mut Stream, p: &mut Pat) -> Option<usize> {
        let t = s.read_u8();
        if t != 0x80 {
            return self.convert_5x_row(s, t, p);
        }
        let mut count = s.read_u8() as u16 + 1;
        let back = s.read_b_u16();
        let old = s.position();
        s.seek(old - back as i64);
        let mut k = 0;
        while count != 0 {
            let t = s.read_u8();
            k += self.convert_5x_row(s, t, p)?;
            count -= 1;
        }
        s.seek(old);
        Some(k)
    }

    fn convert_5x_row(&self, s: &mut Stream, t: u8, p: &mut Pat) -> Option<usize> {
        if t > 0x80 {
            // a note, then empty rows or repeats of it
            let b = full_note(s, ((t as i8).wrapping_neg() as u8).wrapping_sub(1))?;
            p.put(b)?;
            let v = s.read_u8();
            if v <= 64 {
                p.rows(v, [0; 4])?;
                Some(1 + v as usize)
            } else {
                let n = (v as i8).wrapping_neg() as u8;
                p.rows(n, b)?;
                Some(1 + n as usize)
            }
        } else {
            let b = full_note(s, t)?;
            p.put(b)?;
            Some(1)
        }
    }

    /// 6.1A ConvertData: a row, a run of empty rows, or a back reference.
    fn convert_61(&self, s: &mut Stream, p: &mut Pat) -> Option<usize> {
        let t = s.read_u8();
        let mut k = self.convert_61_row(s, t, p)?;
        if k != 0 {
            return Some(k);
        }
        let t = s.read_u8();
        if t < 0x40 {
            // empty rows
            p.rows(t + 1, [0; 4])?;
            return Some(k + t as usize + 1);
        }
        let (mut count, read_offset) = if t < 0x80 {
            // (the position before the offset byte is read)
            let pos = s.position() as u32;
            let b = s.read_u8() as u32;
            ((t - 0x40) as u16, pos.wrapping_sub(b).wrapping_add(1))
        } else {
            let b = s.read_b_u16() as u32;
            ((t as u16).wrapping_sub(0xc0), (s.position() as u32).wrapping_sub(b))
        };
        let old = s.position();
        s.seek(read_offset as i64);
        count = count.wrapping_add(1);
        while count != 0 {
            let t = s.read_u8();
            let n = self.convert_61_row(s, t, p)?;
            if n == 0 {
                let e = s.read_u8().wrapping_add(1);
                p.rows(e, [0; 4])?;
                k += e as usize;
            } else {
                k += n;
            }
            count -= 1;
        }
        s.seek(old);
        Some(k)
    }

    fn convert_61_row(&self, s: &mut Stream, t: u8, p: &mut Pat) -> Option<usize> {
        // rows after the first: none, or repeats of it
        let runs = |s: &mut Stream, p: &mut Pat, b: [u8; 4]| -> Option<usize> {
            let v = s.read_u8();
            if v < 0x80 {
                p.rows(v, [0; 4])?;
            } else {
                p.rows(v - 0x80, b)?;
            }
            Some(1 + (v & 0x7f) as usize)
        };
        if t == 0x7f {
            // one empty row
            p.put([0; 4])?;
            Some(1)
        } else if t < 0x60 {
            // note, sample and effect
            let b = full_note(s, t)?;
            p.put(b)?;
            Some(1)
        } else if t < 0x80 {
            if t & 0xf0 == 0x60 {
                // effect only
                let (mut b3, mut b4) = (t & 0x0f, s.read_u8());
                check_effect(&mut b3, &mut b4);
                p.put([0, 0, b3, b4])?;
            } else {
                // note and sample
                let t1 = s.read_u8();
                let (b1, b2) = get_note((t << 4) | (t1 >> 4))?;
                p.put([b1, b2, t1 << 4, 0])?;
            }
            Some(1)
        } else if t < 0xd0 {
            // note, sample and effect, then empty rows or repeats
            let b = full_note(s, t - 0x80)?;
            p.put(b)?;
            runs(s, p, b)
        } else if t < 0xf0 {
            // effect, then empty rows or repeats
            let (mut b3, mut b4) = (t & 0x0f, s.read_u8());
            check_effect(&mut b3, &mut b4);
            let b = [0, 0, b3, b4];
            p.put(b)?;
            runs(s, p, b)
        } else if t < 0xff {
            // note and sample, then empty rows or repeats
            let t1 = s.read_u8();
            let (b1, b2) = get_note((t << 4) | (t1 >> 4))?;
            let b = [b1, b2, t1 << 4, 0];
            p.put(b)?;
            runs(s, p, b)
        } else {
            Some(0)
        }
    }
}

impl Format for ThePlayer5x {
    fn name(&self) -> &'static str {
        match self.version {
            50 => "The Player 5.0A",
            60 => "The Player 6.0A",
            _ => "The Player 6.1A",
        }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        self.check_format(s) == Some(self.version)
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        got_id(s);
        self.sample_data = s.read_b_u16() as u32;
        s.seek_cur(1);
        let n = s.read_u8();
        self.packed_samples = n & 0x40 != 0;
        self.delta_samples = n & 0x80 != 0;
        self.samples = n & 0x1f;
        if self.packed_samples {
            s.seek_cur(4);
        }
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let n = self.samples as usize;
        self.sample_starts = vec![0; n];
        self.sample_packed = vec![false; n];
        // the lengths the driver has taken (in bytes) when it reads the next sample
        let mut taken = [0u32; 31];
        let mut start = 0u32;
        let mut v = Vec::new();
        for i in 0..n {
            let mut length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let mut loop_start = s.read_b_u16();
            if length >= 0x8000 {
                // the same as another sample
                let o = (length as i16).wrapping_neg() as u16 as usize;
                self.sample_starts[i] = *self.sample_starts.get(o.wrapping_sub(1))?;
                length = (*taken.get(o.wrapping_sub(1))? / 2) as u16;
            } else {
                self.sample_starts[i] = start;
                start = start.wrapping_add(length as u32);
                if fine_tune & 0x80 == 0 {
                    start = start.wrapping_add(length as u32);
                }
            }
            let loop_length = if loop_start == 0xffff {
                loop_start = 0;
                1
            } else {
                length.wrapping_sub(loop_start)
            };
            self.sample_packed[i] = fine_tune & 0x80 != 0;
            taken[i] = length as u32 * 2;
            let si = SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune: fine_tune & 0x0f };
            push!(v, si, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        // LoadPositionList: the track table, then the positions
        self.track_offsets = vec![0; 4 * self.base.number_of_patterns as usize];
        s.read_array_b_u16(&mut self.track_offsets);
        let divisor = if self.version == 50 { 2 } else { 1 };
        let mut list = Vec::new();
        loop {
            let p = s.read_u8();
            if p == 0xff {
                break;
            }
            if list.len() == 128 {
                return None;
            }
            list.push(p / divisor);
        }
        self.track_data = s.position() as u32;
        Some(list)
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut ordered = self.track_offsets.clone();
        ordered.sort();
        let mut out = Vec::new();
        // (one buffer for all patterns, as in the C#)
        let mut p = Pat { d: vec![0u8; 1024], off: 0 };
        for i in 0..self.base.number_of_patterns as usize {
            for j in 0..4 {
                let to = self.track_offsets[i * 4 + j];
                // a track ends where the next one (in offset order) starts; with two
                // voices on one track that is where it starts, as the C# has it
                let index = ordered.iter().position(|&o| o == to)?;
                let end = if index < ordered.len() - 1 {
                    self.track_data.wrapping_add(ordered[index + 1] as u32)
                } else {
                    self.sample_data
                };
                s.seek(self.track_data.wrapping_add(to as u32) as i64);
                let mut k = 0;
                while k < 64 {
                    p.off = k * 16 + j * 4;
                    if s.position() < end as i64 {
                        k += if self.version == 61 { self.convert_61(s, &mut p)? } else { self.convert_5x(s, &mut p)? };
                    } else {
                        p.put([0; 4])?;
                        k += 1;
                    }
                }
            }
            push!(out, p.d.clone(), s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        for i in 0..self.samples as usize {
            let length = self.base.sample_lengths[i] as usize;
            if length == 0 {
                continue;
            }
            s.seek(self.sample_data.wrapping_add(self.sample_starts[i]) as i64);
            if self.delta_samples || self.packed_samples {
                let mut dst = vec![0u8; length];
                if self.sample_packed[i] {
                    let mut smp = 0i8;
                    for pair in dst.chunks_mut(2) {
                        let b = s.read_u8();
                        smp = smp.wrapping_sub(PACK_TABLE[(b >> 4) as usize]);
                        pair[0] = smp as u8;
                        smp = smp.wrapping_sub(PACK_TABLE[(b & 0x0f) as usize]);
                        pair[1] = smp as u8;
                    }
                } else if self.delta_samples {
                    let mut smp = s.read_i8();
                    dst[0] = smp as u8;
                    for d in &mut dst[1..] {
                        smp = smp.wrapping_sub(s.read_i8());
                        *d = smp as u8;
                    }
                } else {
                    s.read_into(&mut dst);
                }
                w.write(&dst);
            } else {
                if s.len() - s.position() < length as i64 - MAX_MISSING {
                    return false;
                }
                w.set_sample_data_marker(s, length as i64);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    fn note_byte(period: u16) -> u8 {
        match period {
            0 => 0,
            _ => (PERIODS.iter().position(|x| (x[0] as u16) << 8 | x[1] as u16 == period).unwrap() as u8 + 1) * 2,
        }
    }

    /// The Player 5.0A/6.0A (`v61` false: 3-byte rows, the last row of a track a
    /// back reference to an earlier equal row) or 6.1A (empty rows 0x7f, rows
    /// with only an effect 0x6x, no back references), with delta samples if asked.
    fn pack(song: &Song, version: u32, delta: bool) -> Vec<u8> {
        let n = song.samples.len();
        let mut tracks: Vec<Vec<u8>> = Vec::new();
        for p in &song.patterns {
            for v in 0..4 {
                let mut t = Vec::new();
                let mut rows: Vec<(usize, [u8; 3])> = Vec::new();
                for (r, row) in p.iter().enumerate() {
                    let (smp, period, eff, par) = row[v];
                    let c = [note_byte(period) | (smp >> 4), (smp << 4) | eff, par];
                    if version == 61 {
                        if c == [0, 0, 0] {
                            t.push(0x7f);
                        } else if c[0] == 0 && c[1] < 0x10 {
                            t.extend([0x60 | c[1], c[2]]);
                        } else {
                            t.extend(c);
                        }
                        continue;
                    }
                    let earlier = rows.iter().find(|x| x.1 == c && x.0 > 0).map(|x| x.0);
                    match earlier {
                        Some(at) if r == 63 => {
                            let back = t.len() + 4 - at;
                            t.extend([0x80, 0x00, (back >> 8) as u8, back as u8]);
                        }
                        _ => {
                            rows.push((t.len(), c));
                            t.extend(c);
                        }
                    }
                }
                tracks.push(t);
            }
        }
        let mut d = Vec::new();
        let positions: Vec<u8> = match version {
            50 => song.positions.iter().map(|p| p * 2).collect(),
            _ => song.positions.clone(),
        };
        let header = 4 + 6 * n + 8 * song.patterns.len() + positions.len() + 1;
        let mut offsets = Vec::new();
        let mut at = 0;
        for t in &tracks {
            offsets.push(at as u16);
            at += t.len();
        }
        let mut sample_offset = header + at + 4;
        sample_offset += sample_offset & 1;
        d.extend((sample_offset as u16).to_be_bytes());
        d.push(song.patterns.len() as u8);
        d.push(n as u8 | if delta { 0x80 } else { 0 });
        for s in &song.samples {
            d.extend(s.0.to_be_bytes());
            d.extend([s.1, s.2]);
            let ls: u16 = if s.4 <= 1 { 0xffff } else { s.3 };
            d.extend(ls.to_be_bytes());
        }
        for o in offsets {
            d.extend(o.to_be_bytes());
        }
        d.extend(&positions);
        d.push(0xff);
        for t in &tracks {
            d.extend(t);
        }
        d.resize(sample_offset, 0);
        for i in 0..n {
            let data = song.sample_data(i);
            if delta {
                let mut prev = 0u8;
                for (k, &x) in data.iter().enumerate() {
                    d.push(if k == 0 { x } else { prev.wrapping_sub(x) });
                    prev = x;
                }
            } else {
                d.extend(data);
            }
        }
        d
    }

    #[test]
    fn the_player_5x_6x() {
        let song = Song::new(3);
        for (version, delta, name) in [
            (50, false, "The Player 5.0A"),
            (60, false, "The Player 6.0A"),
            (61, false, "The Player 6.1A"),
            (61, true, "The Player 6.1A"),
        ] {
            let (f, m) = convert_any(&pack(&song, version, delta)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f));
        }
    }
}
