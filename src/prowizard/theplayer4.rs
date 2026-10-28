//! The Player 4.0A, 4.0B and 4.1A (Jarno Paananen), ported from NostalgicPlayer's
//! ThePlayer4xFormatBase, ThePlayer40AFormat, ThePlayer40BFormat and
//! ThePlayer41AFormat.
//!
//! After the mark come the offsets of the track data, the track table and the
//! sample data, 16 bytes per sample (4.1A swaps volume and fine tune), and four
//! track offsets per position. A track row is 3 bytes and a count: negative,
//! the row repeats; positive, empty rows follow. A row with the high bit set
//! instead replays a number of rows from elsewhere in the track data.

use super::core::*;

#[derive(Default)]
pub struct ThePlayer4x {
    base: Base,
    mark: [u8; 4],
    samples: u8,
    positions: u8,
    track_offsets: [Vec<u16>; 4],
    position_list: Vec<u8>,
    track_data: u32,
    track_table: u32,
    sample_data: u32,
    first_sample_address: u32,
    skip_last_sample: bool,
    sample_starts: Vec<u32>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    let f = |mark: &[u8; 4]| Box::new(ThePlayer4x { mark: *mark, ..Default::default() }) as Box<dyn Format>;
    vec![(36, f(b"P40A")), (37, f(b"P40B")), (38, f(b"P41A"))]
}

/// P4xxChannel: a voice's state while its track is unpacked.
struct Channel {
    p4x: [u8; 3],
    info: i8,
    repeat_lines: u16,
    old_position: i64,
    pro: [u8; 4],
}

/// FixAddresses: offsets relative to a load address made relative to the file.
fn fix_addresses(first: u32, count: u8, tdo: &mut u32, tto: &mut u32, sdo: &mut u32) {
    if first != 0 {
        let t = (count as u32 + 1) * 16;
        let t1 = tto.wrapping_sub(t);
        *tto = t;
        *tdo = tdo.wrapping_sub(t1);
        *sdo = sdo.wrapping_sub(t1);
    }
}

impl ThePlayer4x {
    /// 4.1A stores the volume before the fine tune.
    fn v41(&self) -> bool {
        &self.mark == b"P41A"
    }

    /// CheckSampleInfo: the total sample size (0: not this format), the first
    /// sample's address and the last sample's length.
    fn check_sample_info(&self, s: &mut Stream, count: u8) -> (u32, u32, u16) {
        let mut last_start: i32 = -1;
        let mut size = 0u32;
        let (mut first, mut last_length) = (0u32, 0u16);
        s.seek(20);
        for i in 0..count {
            let start = s.read_b_i32();
            let length = s.read_b_u16();
            s.seek_cur(6);
            let (a, b) = (s.read_b_u16(), s.read_b_u16());
            let (fine_tune, volume) = if self.v41() { (b, a) } else { (a, b) };
            if i == 0 {
                first = start as u32;
            }
            last_length = length;
            if fine_tune % 74 != 0 {
                return (0, first, last_length);
            }
            // fine tune > 0xf
            if fine_tune > 0x456 {
                continue;
            }
            if volume > 0x40 || length >= 0x8000 {
                return (0, first, last_length);
            }
            if start > last_start {
                last_start = start;
                size = size.wrapping_add(length as u32 * 2);
            }
        }
        (size, first, last_length)
    }

    /// CheckForThePlayerFormat.
    fn check_format(&mut self, s: &mut Stream) -> bool {
        if s.len() < 20 {
            return false;
        }
        s.seek(0);
        if s.read_mark() != self.mark {
            return false;
        }
        s.seek_cur(1);
        let positions = s.read_u8();
        let count = s.read_u8();
        s.seek_cur(1);
        let mut tdo = s.read_b_u32();
        let mut tto = s.read_b_u32();
        let mut sdo = s.read_b_u32();
        let (size, first, last_length) = self.check_sample_info(s, count);
        if size == 0 {
            return false;
        }
        // end mark of the track table
        s.seek(positions as i64 * 8 + (count as i64 + 1) * 16 + 4);
        if s.read_b_u16() != 0xffff {
            return false;
        }
        fix_addresses(first, count, &mut tdo, &mut tto, &mut sdo);
        let end = sdo.wrapping_add(4).wrapping_add(size);
        if end as i64 > s.len() + MAX_MISSING {
            // sometimes there is a useless sample at the end
            let t = end.wrapping_sub(last_length as u32 * 2);
            if t as i64 > s.len() + MAX_MISSING {
                return false;
            }
            self.skip_last_sample = true;
        } else {
            self.skip_last_sample = false;
        }
        true
    }

    /// ReadSampleInfo, with the sample's start address.
    fn read_sample_info(&self, s: &mut Stream) -> (SampleInfo, u32) {
        let start = s.read_b_u32();
        let length = s.read_b_u16();
        let loop_start = s.read_b_u32();
        let loop_length = s.read_b_u16();
        let (a, b) = (s.read_b_u16(), s.read_b_u16());
        let (fine_tune, volume) = if self.v41() { (b, a) } else { (a, b) };
        if fine_tune > 0x456 {
            let si = SampleInfo { name: None, length: 0, loop_start: 0, loop_length: 1, volume: 0, fine_tune: 0 };
            return (si, start);
        }
        let si = SampleInfo {
            name: None,
            length,
            loop_start: (loop_start.wrapping_sub(start) / 2) as u16,
            loop_length,
            volume: volume as u8,
            fine_tune: (fine_tune / 74) as u8,
        };
        (si, start)
    }

    /// ConvertData: the next row of a voice's track; true at a position jump or
    /// pattern break.
    fn convert_data(&self, s: &mut Stream, ch: &mut Channel, pattern: &mut [u8], off: usize) -> bool {
        if ch.info < 0 {
            // the same row as the last one
            ch.info += 1;
            pattern[off..off + 4].copy_from_slice(&ch.pro);
            return false;
        }
        if ch.info > 0 {
            // an empty row
            ch.info -= 1;
            pattern[off..off + 4].fill(0);
            return false;
        }
        if ch.repeat_lines != 0 {
            // the next row of a replayed part
            ch.repeat_lines -= 1;
            ch.p4x = [s.read_u8(), s.read_u8(), s.read_u8()];
            ch.info = s.read_i8();
        } else {
            if ch.old_position != -1 {
                s.seek(ch.old_position);
                ch.old_position = -1;
            }
            let t = s.read_u8();
            if t & 0x80 != 0 {
                // replay rows from elsewhere, then come back here
                ch.repeat_lines = s.read_u8() as u16;
                let offset = s.read_b_u16();
                ch.old_position = s.position();
                s.seek(self.track_data.wrapping_add(offset as u32).wrapping_add(4) as i64);
                ch.p4x = [s.read_u8(), s.read_u8(), s.read_u8()];
                ch.info = s.read_i8();
            } else {
                ch.p4x = [t, s.read_u8(), s.read_u8()];
                ch.info = s.read_i8();
            }
        }
        ch.pro[0] = if ch.p4x[0] & 1 != 0 { 0x10 } else { 0x00 };
        let t = ch.p4x[0] & 0xfe;
        if t != 0 {
            let n = (t / 2).wrapping_sub(1) as usize;
            if n < PERIODS.len() {
                ch.pro[0] |= PERIODS[n][0];
                ch.pro[1] = PERIODS[n][1];
            } else {
                ch.pro[1] = 0;
            }
        } else {
            ch.pro[1] = 0;
        }
        ch.pro[2] = ch.p4x[1];
        ch.pro[3] = ch.p4x[2];
        let mut brk = false;
        match ch.pro[2] & 0x0f {
            0xb => {
                ch.pro[3] /= 2;
                brk = true;
            }
            0xd => {
                // decimal to hex
                ch.pro[3] = (ch.pro[3] / 0x10).wrapping_mul(10).wrapping_add(ch.pro[3] % 0x10);
                brk = true;
            }
            0xe => {
                if ch.pro[3] & 0xf0 == 0 {
                    if ch.pro[3] == 0x02 {
                        ch.pro[3] = 0x01;
                    }
                } else if ch.pro[2] & 0xf0 == 0xc0 {
                    // (as the C# has it: tests the sample nibble, meant the note cut)
                    ch.pro[3] = ch.pro[3].wrapping_add(1);
                }
            }
            0xa | 0x5 | 0x6 => {
                if ch.pro[3] >= 0x80 {
                    ch.pro[3] = ((ch.pro[3] as i8).wrapping_neg() as u8) << 4;
                    if ch.pro[3] >= 0x80 {
                        ch.pro[3] = (ch.pro[3] as i8).wrapping_neg() as u8;
                    }
                }
            }
            0x8 => ch.pro[2] &= 0xf0,
            _ => {}
        }
        pattern[off..off + 4].copy_from_slice(&ch.pro);
        brk
    }
}

impl Format for ThePlayer4x {
    fn name(&self) -> &'static str {
        match &self.mark {
            b"P40A" => "The Player 4.0A",
            b"P40B" => "The Player 4.0B",
            _ => "The Player 4.1A",
        }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        self.check_format(s)
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(5);
        self.positions = s.read_u8();
        self.samples = s.read_u8();
        s.seek_cur(1);
        self.track_data = s.read_b_u32();
        self.track_table = s.read_b_u32();
        self.sample_data = s.read_b_u32();
        self.first_sample_address = s.read_b_u32();
        fix_addresses(self.first_sample_address, self.samples, &mut self.track_data, &mut self.track_table, &mut self.sample_data);
        // the track offset table
        s.seek(20 + self.samples as i64 * 16);
        let n = self.positions as usize;
        self.track_offsets = std::array::from_fn(|_| vec![0; n]);
        for i in 0..n {
            for j in 0..4 {
                self.track_offsets[j][i] = s.read_b_u16() & 0xfffe;
            }
        }
        let t = &self.track_offsets;
        let key = |p: usize| (t[0][p] as u64) << 48 | (t[1][p] as u64) << 32 | (t[2][p] as u64) << 16 | t[3][p] as u64;
        self.position_list = self.base.build_position_list(n, key).0;
        if self.skip_last_sample {
            self.samples = self.samples.wrapping_sub(1);
        }
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        self.sample_starts = vec![0; self.samples as usize];
        s.seek(20);
        let mut v = Vec::new();
        for i in 0..(self.samples as usize).min(31) {
            let (si, start) = self.read_sample_info(s);
            push!(v, si, s);
            self.sample_starts[i] = start.wrapping_sub(self.first_sample_address);
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
                let mut max_rows = 64;
                for j in 0..4 {
                    let mut ch = Channel { p4x: [0; 3], info: 0, repeat_lines: 0, old_position: -1, pro: [0; 4] };
                    let to = self.track_offsets[j][i] as u32;
                    s.seek(self.track_data.wrapping_add(4).wrapping_add(to) as i64);
                    for k in 0..max_rows {
                        if self.convert_data(s, &mut ch, &mut pattern, k * 16 + j * 4) {
                            // a break (B or D) ends the pattern for the other voices too
                            max_rows = k + 1;
                            break;
                        }
                    }
                }
                push!(out, pattern, s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        let off = self.sample_data.wrapping_add(4);
        for i in 0..(self.samples as usize).min(31) {
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
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    /// The Player 4.xx: each voice's track separately, every row stored.
    fn pack(song: &Song, mark: &[u8; 4]) -> Vec<u8> {
        let n = song.samples.len();
        let mut d = mark.to_vec();
        d.push(song.patterns.len() as u8);
        d.push(song.positions.len() as u8);
        d.push(n as u8);
        d.push(0);
        let tt = 20 + 16 * n as u32;
        let td = tt + 8 * song.positions.len() as u32 + 2;
        let track_len = 64 * 4;
        let sd = td + 4 + (track_len * 4 * song.patterns.len()) as u32;
        for v in [td, tt, sd] {
            d.extend(v.to_be_bytes());
        }
        let mut start = 0u32;
        for s in &song.samples {
            d.extend(start.to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend((start + s.3 as u32 * 2).to_be_bytes());
            d.extend(s.4.to_be_bytes());
            let (ft, vol) = ((s.1 as u16 * 74).to_be_bytes(), (s.2 as u16).to_be_bytes());
            if mark == b"P41A" {
                d.extend(vol);
                d.extend(ft);
            } else {
                d.extend(ft);
                d.extend(vol);
            }
            start += s.0 as u32 * 2;
        }
        for &p in &song.positions {
            for v in 0..4 {
                d.extend(((p as usize * 4 + v) as u16 * track_len as u16).to_be_bytes());
            }
        }
        d.extend([0xff, 0xff, 0, 0, 0, 0]);
        for p in &song.patterns {
            for v in 0..4 {
                for row in p {
                    let (smp, period, eff, par) = row[v];
                    let b0 = match period {
                        0 => 0,
                        _ => (PERIODS.iter().position(|x| (x[0] as u16) << 8 | x[1] as u16 == period).unwrap() as u8 + 1) * 2,
                    } | (smp >> 4);
                    d.extend([b0, (smp << 4) | eff, par, 0]);
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
    fn the_player_4x() {
        let song = Song::new(3);
        for (mark, name) in [(b"P40A", "The Player 4.0A"), (b"P40B", "The Player 4.0B"), (b"P41A", "The Player 4.1A")] {
            let (f, m) = convert_any(&pack(&song, mark)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f));
        }
    }
}
