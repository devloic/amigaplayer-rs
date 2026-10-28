//! Channel Player 1, 2 and 3, ported from NostalgicPlayer's
//! ChannelPlayerFormatBase, ChannelPlayer1Format, ChannelPlayer2Format and
//! ChannelPlayer3Format.
//!
//! The header gives the sizes of the sample table, the track table and the track
//! data, and the sample table holds "memory addresses". Version 1 tracks are 64
//! rows of 3 bytes, numbered (a track table entry is 4 bytes); versions 2 and 3
//! address their tracks by offset (8 bytes an entry) and pack rows without an
//! effect (2: 0x80 + note, 3: 0x80 alone for an empty row). The version is found
//! from the track data: v1 never has 0x80 commands, only v3 four 0x80 in a row.

use super::core::*;

#[derive(Default)]
pub struct ChannelPlayer {
    base: Base,
    version: i32,
    sample_info_length: u16,
    position_list_length: u16,
    pattern_length: u16,
    position_list: Vec<u8>,
    positions: usize,
    track_table_offset: i64,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    (1..=3).map(|v| (v as u32 + 1, Box::new(ChannelPlayer { version: v, ..Default::default() }) as Box<dyn Format>)).collect()
}

/// The sign-magnitude slide values (0xf1-0xff down) as ProTracker nibbles, and
/// pattern breaks from decimal.
fn fix_effect(b3: u8, b4: u8) -> u8 {
    match b3 & 0x0f {
        0xd => (b4 / 10) * 16 + b4 % 10,
        0xa | 0x5 | 0x6 => {
            if b4 >= 0xf1 {
                (b4 as i8).wrapping_neg() as u8
            } else {
                b4 << 4
            }
        }
        _ => b4,
    }
}

/// A note number (even, 2 = C-1) as a period, with the sample's high bit.
fn note(b: u8, b1: &mut u8) -> u8 {
    if b == 0 {
        return 0;
    }
    let n = (b - 2) / 2;
    *b1 |= PERIODS[n as usize][0];
    PERIODS[n as usize][1]
}

impl ChannelPlayer {
    /// CheckForChannelPlayerFormat: the version, or None.
    fn check_format(&self, s: &mut Stream) -> Option<i32> {
        if s.len() < 26 {
            return None;
        }
        s.seek(0);
        let sil = s.read_b_u16();
        if sil & 0xf00f != 0x000a {
            return None;
        }
        let t = sil & 0x0ff0;
        if t == 0 || t > 0x01f0 {
            return None;
        }
        let pll = s.read_b_u16();
        if pll == 0 {
            return None;
        }
        let patl = s.read_b_u16();
        if patl == 0 {
            return None;
        }
        let sample_length = s.read_b_u32();
        let mut calc = 0u32;
        for _ in 0..sil >> 4 {
            let addr = s.read_b_u32();
            if addr > 0x1fe000 {
                return None;
            }
            let len = s.read_b_u16();
            if len >= 0x8000 {
                return None;
            }
            calc += len as u32 * 2;
            if s.read_b_u16() > 0x40 {
                return None;
            }
            let loop_addr = s.read_b_u32();
            if loop_addr > 0x1fe000 {
                return None;
            }
            if len == 0 && loop_addr != addr {
                return None;
            }
            s.seek_cur(2);
            if s.read_b_u16() % 0x48 != 0 {
                return None;
            }
        }
        if calc != sample_length {
            return None;
        }
        if (sil as u32 + pll as u32 + patl as u32 + calc) as i64 > s.len() + MAX_MISSING {
            return None;
        }
        let pattern_offset = sil as i64 + pll as i64;
        // -4 because of the test
        let sample_offset = pattern_offset + patl as i64 - 4;
        // (the C# loops forever where the track data is cut short)
        s.seek(pattern_offset);
        let mut found = false;
        while s.position() < sample_offset && !s.eof {
            // 0x80 commands are not possible in v1
            if s.read_u8() == 0x80 && s.read_u8() <= 0x0f {
                found = true;
                break;
            }
        }
        if !found {
            return Some(1);
        }
        s.seek(pattern_offset);
        while s.position() < sample_offset && !s.eof {
            // only v3 can have 4 0x80 right after each other
            if s.read_b_u32() == 0x80808080 {
                return Some(3);
            }
        }
        Some(2)
    }

    /// CreatePositionTable: positions from the distinct track table entries.
    fn create_position_table(&mut self, s: &mut Stream) {
        self.position_list = vec![0; 128];
        if self.version == 1 {
            self.track_table_offset = ((self.sample_info_length & 0xfff0) >> 4) as i64 * 16 + 10;
        } else {
            s.seek(0);
            self.track_table_offset = ((s.read_b_u16() & 0xfff0) >> 4) as i64 * 16 + 10;
        }
        s.seek(self.track_table_offset);
        let entry = if self.version == 1 { 4 } else { 8 };
        let read = |s: &mut Stream| if entry == 4 { s.read_b_u32() as u64 } else { s.read_b_u64() };
        let mut used = vec![read(s)];
        self.base.number_of_patterns = 1;
        self.positions = (self.position_list_length / entry) as usize;
        for i in 1..self.positions {
            let t = read(s);
            match used.iter().position(|&u| u == t) {
                Some(j) => self.position_list[i] = j as u8,
                None => {
                    self.position_list[i] = self.base.number_of_patterns as u8;
                    self.base.number_of_patterns += 1;
                    used.push(t);
                }
            }
        }
    }

    fn patterns_v1(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut pat = vec![0u8; 1024];
        let start = self.sample_info_length as i64 + self.position_list_length as i64;
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions {
            if (*self.position_list.get(i)? as i8) as i32 > last {
                last += 1;
                s.seek(self.track_table_offset);
                let mut tracks = [0u8; 4];
                s.read_into(&mut tracks);
                self.track_table_offset += 4;
                if tracks == [0; 4] {
                    break;
                }
                for (j, &t) in tracks.iter().enumerate() {
                    s.seek(start + t as i64 * 0xc0);
                    for k in 0..64 {
                        let rb = s.read_u8();
                        let mut b1 = (rb & 0x01) << 4;
                        let b3 = s.read_u8(); // sample number + effect
                        let b2 = note(rb & 0xfe, &mut b1);
                        let b4 = fix_effect(b3, s.read_u8());
                        pat[k * 16 + j * 4..k * 16 + j * 4 + 4].copy_from_slice(&[b1, b2, b3, b4]);
                    }
                }
                push!(out, pat.clone(), s);
            } else {
                self.track_table_offset += 4;
            }
        }
        Some(out)
    }

    fn patterns_v23(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut pat = vec![0u8; 1024];
        let start = self.sample_info_length as i64 + self.position_list_length as i64;
        let mut to = self.track_table_offset;
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions {
            if (*self.position_list.get(i)? as i8) as i32 > last {
                last += 1;
                s.seek(to);
                let mut tracks = [0u16; 4];
                s.read_array_b_u16(&mut tracks);
                to += 8;
                for (j, &t) in tracks.iter().enumerate() {
                    s.seek(start + t as i64);
                    for k in 0..64 {
                        let b = s.read_u8();
                        let row = if b == 0x80 {
                            // no note (v2 keeps the effect number)
                            let e = if self.version == 2 { s.read_u8() & 0x0f } else { 0 };
                            [0, 0, e, 0]
                        } else if b > 0x80 && self.version == 3 {
                            // only a note
                            let mut b1 = 0;
                            let b2 = note(b - 0x80, &mut b1);
                            [b1, b2, 0, 0]
                        } else if b > 0x80 {
                            // no effect value
                            let mut b1 = (b & 0x01) << 4;
                            let b3 = s.read_u8();
                            let b2 = note((b - 0x80) & 0xfe, &mut b1);
                            [b1, b2, b3, 0]
                        } else {
                            let mut b1 = (b & 0x01) << 4;
                            let b3 = s.read_u8();
                            let b2 = note(b & 0xfe, &mut b1);
                            [b1, b2, b3, fix_effect(b3, s.read_u8())]
                        };
                        pat[k * 16 + j * 4..k * 16 + j * 4 + 4].copy_from_slice(&row);
                    }
                }
                push!(out, pat.clone(), s);
            } else {
                to += 8;
            }
        }
        Some(out)
    }
}

impl Format for ChannelPlayer {
    fn name(&self) -> &'static str {
        ["Channel Player 1", "Channel Player 2", "Channel Player 3"][self.version as usize - 1]
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        self.check_format(s) == Some(self.version)
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        // NostalgicPlayer rewinds the stream before converting
        s.seek(0);
        self.sample_info_length = s.read_b_u16();
        self.position_list_length = s.read_b_u16();
        self.pattern_length = s.read_b_u16();
        self.create_position_table(s);
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(10);
        let mut v = Vec::new();
        for _ in 0..(self.sample_info_length & 0xfff0) >> 4 {
            let addr = s.read_b_u32();
            let length = s.read_b_u16();
            s.seek_cur(1);
            let volume = s.read_u8();
            let loop_addr = s.read_b_u32();
            let loop_length = s.read_b_u16();
            let fine_tune = s.read_b_u16();
            let loop_start = (loop_addr.wrapping_sub(addr) / 2) as u16;
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune: (fine_tune / 0x48) as u8 }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list.get(..self.positions)?.to_vec())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        if self.version == 1 { self.patterns_v1(s) } else { self.patterns_v23(s) }
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_info_length as i64 + self.position_list_length as i64 + self.pattern_length as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    fn note_number(period: u16) -> u8 {
        if period == 0 {
            return 0;
        }
        PERIODS.iter().position(|p| (p[0] as u16) << 8 | p[1] as u16 == period).unwrap() as u8 * 2 + 2
    }

    /// Packs the song as Channel Player `v` does: one track per pattern and voice.
    /// Samples must be below 16 and the song's effect (C) needs no fixing.
    fn pack(song: &Song, v: u32) -> Vec<u8> {
        let ns = song.samples.len();
        let sil = (10 + ns * 16) as u16;
        let entry = if v == 1 { 4 } else { 8 };
        let pll = (song.positions.len() * entry) as u16;
        // what tells the versions apart: v1 has track 0 unused (four zero track
        // numbers end the song), v2 a 0x80 command, v3 four 0x80 too
        let mut tracks = match v {
            1 => vec![0u8; 0xc0],
            2 => vec![0x80, 0, 0, 0],
            _ => vec![0x80, 0, 0, 0, 0x80, 0x80, 0x80, 0x80],
        };
        let mut offs = Vec::new();
        for p in &song.patterns {
            for voice in 0..4 {
                offs.push(tracks.len() as u16);
                for row in p {
                    let n = row[voice];
                    let nb = note_number(n.1) | (n.0 >> 4);
                    let b3 = ((n.0 & 0x0f) << 4) | n.2;
                    if v == 1 {
                        tracks.extend([nb, b3, n.3]);
                    } else if n == (0, 0, 0, 0) {
                        tracks.extend(if v == 2 { vec![0x80, 0] } else { vec![0x80] });
                    } else if n.3 == 0 && n.2 == 0 && v == 3 && n.0 == 0 {
                        tracks.push(0x80 + nb);
                    } else if n.3 == 0 && v == 2 {
                        tracks.extend([0x80 + nb, b3]);
                    } else {
                        tracks.extend([nb, b3, n.3]);
                    }
                }
            }
        }
        let mut d = Vec::new();
        d.extend(sil.to_be_bytes());
        d.extend(pll.to_be_bytes());
        d.extend((tracks.len() as u16).to_be_bytes());
        let total: u32 = song.samples.iter().map(|s| s.0 as u32 * 2).sum();
        d.extend(total.to_be_bytes());
        let mut addr = 0x1000u32;
        for s in &song.samples {
            d.extend(addr.to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend((s.2 as u16).to_be_bytes());
            d.extend((addr + s.3 as u32 * 2).to_be_bytes());
            d.extend(s.4.to_be_bytes());
            d.extend((s.1 as u16 * 0x48).to_be_bytes());
            addr += s.0 as u32 * 2;
        }
        for &p in &song.positions {
            for voice in 0..4 {
                let t = p as usize * 4 + voice;
                if v == 1 {
                    d.push(t as u8 + 1);
                } else {
                    d.extend(offs[t].to_be_bytes());
                }
            }
        }
        d.extend(tracks);
        for i in 0..ns {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn channel_player() {
        let song = Song::new(3);
        for (v, name) in [(1, "Channel Player 1"), (2, "Channel Player 2"), (3, "Channel Player 3")] {
            let (f, m) = convert_any(&pack(&song, v)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f), "{name}");
        }
    }
}
