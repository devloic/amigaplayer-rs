//! ProPacker 2.1 and 3.0 (Cryptoburners), ported from NostalgicPlayer's
//! ProPackerNewFormatBase, ProPacker21Format and ProPacker30Format.
//!
//! The header is the sample table, the song length and restart, and a track number
//! per voice and position. A track is 64 words, each an index into a table of
//! ProTracker notes: a note number (2.1) or its byte offset (3.0, number * 4).

use super::core::*;

#[derive(Default)]
pub struct ProPackerNew {
    base: Base,
    version: u32,
    tracks_size: u32,
    positions: u8,
    restart: u8,
    track_table: [Vec<u8>; 4],
    reference_table: Vec<u16>,
    position_list: Vec<u8>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![
        (29, Box::new(ProPackerNew { version: 21, ..Default::default() })),
        (30, Box::new(ProPackerNew { version: 30, ..Default::default() })),
    ]
}

impl ProPackerNew {
    /// CheckForProPackerFormat: 21, 30, or None.
    fn check_format(&mut self, s: &mut Stream) -> Option<u32> {
        if s.len() < 0x2fa {
            return None;
        }
        // first value in the track data
        s.seek(0x2fa);
        if s.read_b_u16() != 0 {
            return None;
        }
        let samples_size = cryptoburners_check(s, 0, 0xf8)?;
        // the highest track number
        s.seek(0xfa);
        let mut n = 0u32;
        for _ in 0..512 {
            n = n.max(s.read_u8() as u32);
        }
        self.tracks_size = (n + 1) * 64 * 2;
        // the module length
        s.seek(0x2fa + self.tracks_size as i64);
        let v = s.read_b_u32();
        let t = v.wrapping_add(4).wrapping_add(samples_size);
        if t & 1 != 0 || t as i64 > s.len() + MAX_MISSING {
            return None;
        }
        // the notes in the reference table
        if v == 0 {
            return None;
        }
        let refs = (v / 4).min(64 * 2);
        let mut count = 0;
        for _ in 0..refs {
            let t = s.read_b_u16();
            if t != 0 {
                if !(0x71..=0x1358).contains(&t) {
                    return None;
                }
                count += 1;
            }
            s.seek_cur(2);
        }
        if count == 0 {
            return None;
        }
        // the track data tells the version
        let words = ((self.tracks_size / 128) * 64).min(64 * 4);
        s.seek(0x2fa);
        let mut count = 0;
        for _ in 0..words {
            let t = s.read_b_u16();
            if t >= 0x1800 {
                return None;
            }
            if t % 4 != 0 {
                count += 1;
            }
        }
        Some(if count != 0 { 21 } else { 30 })
    }
}

impl Format for ProPackerNew {
    fn name(&self) -> &'static str {
        if self.version == 21 { "ProPacker 2.1" } else { "ProPacker 3.0" }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        self.check_format(s) == Some(self.version)
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0xf8);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        for t in &mut self.track_table {
            *t = vec![0; 128];
            s.read_into(t);
        }
        self.reference_table = vec![0; self.tracks_size as usize / 2];
        s.seek(0x2fa);
        s.read_array_b_u16(&mut self.reference_table);
        let tt = &self.track_table;
        let key = |p: usize| u32::from_be_bytes([tt[0][p], tt[1][p], tt[2][p], tt[3][p]]);
        self.position_list = self.base.build_position_list(self.positions as usize, key).0;
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(0);
        let mut v = Vec::new();
        for _ in 0..31 {
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list.clone())
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mult = if self.version == 21 { 4 } else { 1 };
        let ref_start = 0x2fa + self.tracks_size + 4;
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions as usize {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                for j in 0..4 {
                    let mut to = self.track_table[j][i] as usize * 64;
                    for k in 0..64 {
                        let r = *self.reference_table.get(to)? as u32;
                        to += 1;
                        s.seek((r * mult + ref_start) as i64);
                        s.read_into(&mut pat[k * 16 + j * 4..k * 16 + j * 4 + 4]);
                    }
                }
                push!(out, pat, s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0x2fa + self.tracks_size as i64);
        let off = s.read_b_u32();
        s.seek_cur(off as i64);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// ProPacker 2.1 (`v21`) or 3.0: a track per pattern and voice, of indexes into
    /// a table of distinct notes.
    fn pack_propacker(song: &Song, v21: bool) -> Vec<u8> {
        let mut d = Vec::new();
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(s.0.to_be_bytes());
            d.extend([s.1, s.2]);
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
        }
        d.push(song.positions.len() as u8);
        d.push(song.restart);
        let mut tt = vec![0u8; 512];
        for (i, &p) in song.positions.iter().enumerate() {
            for v in 0..4 {
                tt[v * 128 + i] = p * 4 + v as u8;
            }
        }
        d.extend(tt);
        // the note table starts with an empty note, as the check wants
        let mut notes: Vec<[u8; 4]> = vec![[0; 4]];
        let mut tracks = Vec::new();
        for p in &song.patterns {
            for v in 0..4 {
                for row in p {
                    let n = Song::note(row[v]);
                    let i = notes.iter().position(|x| *x == n).unwrap_or_else(|| {
                        notes.push(n);
                        notes.len() - 1
                    });
                    let r = if v21 { i as u16 } else { i as u16 * 4 };
                    tracks.extend(r.to_be_bytes());
                }
            }
        }
        d.extend(tracks);
        let table: Vec<u8> = notes.concat();
        d.extend((table.len() as u32).to_be_bytes());
        d.extend(table);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn propacker() {
        let song = Song::new(3);
        for (v21, name) in [(true, "ProPacker 2.1"), (false, "ProPacker 3.0")] {
            let packed = pack_propacker(&song, v21);
            let (f, m) = convert_any(&packed).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(song.restart));
        }
    }
}
