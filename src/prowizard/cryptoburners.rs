//! ProPacker 1.0, Heatseeker mc1.0 and Module Protector (the older
//! Cryptoburners packers), ported from NostalgicPlayer's ProPacker10Format,
//! HeatseekerFormat and ModuleProtectorFormat on CryptoburnersFormatBase.
//!
//! All three keep ProTracker's sample table without names, then the song
//! length, restart and position list. ProPacker 1.0 numbers tracks of 64 notes
//! (a track per voice and position); Heatseeker packs empty rows and points
//! repeated tracks back at earlier ones; Module Protector is a plain module
//! without names and mark, sometimes behind a "TRK1" id.

use super::core::*;

#[derive(Clone, Copy, PartialEq, Default)]
enum Kind {
    #[default]
    ProPacker10,
    Heatseeker,
    ModuleProtector,
}

#[derive(Default)]
pub struct Cryptoburners {
    base: Base,
    kind: Kind,
    tracks_size: u32,
    positions: u8,
    restart: u8,
    start: i64,
    real_patterns: u16,
    track_table: [Vec<u8>; 4],
    position_list: Vec<u8>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    let f = |kind| Box::new(Cryptoburners { kind, ..Default::default() });
    vec![(28, f(Kind::ProPacker10)), (10, f(Kind::Heatseeker)), (14, f(Kind::ModuleProtector))]
}

impl Cryptoburners {
    fn check_pp10(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x2fa {
            return false;
        }
        let Some(size) = cryptoburners_check(s, 0, 0xf8) else { return false };
        // the first two track numbers of voice 2 are not 0
        s.seek(0x17a);
        if s.read_u8() == 0 || s.read_u8() == 0 {
            return false;
        }
        s.seek(0xfa);
        let mut n = 0u32;
        for _ in 0..512 {
            n = n.max(s.read_u8() as u32);
        }
        self.tracks_size = (n + 1) * 64;
        // the notes in the tracks
        s.seek(0x2fa);
        for _ in 0..self.tracks_size {
            let t = s.read_b_u16();
            if t != 0 && !(0x71..=0x1358).contains(&t) {
                return false;
            }
            s.seek_cur(2);
        }
        0x2fa + self.tracks_size as i64 * 4 + size as i64 <= s.len() + MAX_MISSING
    }

    fn check_heatseeker(&mut self, s: &mut Stream) -> bool {
        if s.len() < 375 || cryptoburners_check(s, 0, 0xf8).is_none() {
            return false;
        }
        s.seek(0xf8);
        let n = s.read_u8();
        if self.base.find_highest_pattern_at(s, 0xfa, n as usize) > 64 {
            return false;
        }
        // the pattern data: 0x8000 starts empty rows, 0xc000 a repeated track
        s.seek(0x17a);
        let mut empty = 0;
        for _ in 0..512 {
            let t = s.read_b_u16();
            if t == 0x8000 {
                empty += 1;
            } else if t != 0 && t != 0xc000 && !(0x71..=0x1358).contains(&t) {
                return false;
            }
            s.seek_cur(2);
        }
        empty != 0
    }

    fn check_mp(&mut self, s: &mut Stream) -> bool {
        if s.len() < 375 {
            return false;
        }
        s.seek(0);
        let off = if s.read_mark() == b"TRK1" { 4 } else { 0 };
        if cryptoburners_check(s, off, off + 0xf8).is_none() {
            return false;
        }
        // all 128 positions: Grapevine #3 stores bigger pattern numbers after the
        // song's end than it plays
        if self.base.find_highest_pattern_at(s, off + 0xfa, 128) > 64 {
            return false;
        }
        s.seek(off);
        let mut size = 0u32;
        for _ in 0..31 {
            let t = s.read_b_u16();
            size = size.wrapping_add(t as u32 * 2);
            if t == 0 {
                s.seek_cur(4);
                if s.read_b_u16() != 1 {
                    return false;
                }
            } else {
                s.seek_cur(6);
            }
        }
        // the first 16 rows of each pattern hold ProTracker periods
        let mut periods = 0;
        for i in 0..self.base.number_of_patterns as i64 {
            s.seek(off + 0x17a + i * 1024);
            for _ in 0..16 * 4 {
                let t = s.read_b_u16() & 0x0fff;
                if t != 0 {
                    let pt = [(t >> 8) as u8, t as u8];
                    if !PERIODS[..36].contains(&pt) {
                        return false;
                    }
                    periods += 1;
                }
                s.seek_cur(2);
            }
        }
        periods != 0 && 0x17a + self.base.number_of_patterns as i64 * 1024 + size as i64 <= s.len() + MAX_MISSING
    }

    fn patterns_pp10(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions as usize {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                for j in 0..4 {
                    s.seek(0x2fa + self.track_table[j][i] as i64 * 256);
                    for k in 0..64 {
                        s.read_into(&mut pat[k * 16 + j * 4..k * 16 + j * 4 + 4]);
                    }
                }
                push!(out, pat, s);
            }
        }
        Some(out)
    }

    fn patterns_heatseeker(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let np = self.base.number_of_patterns as usize;
        s.seek(0x17a);
        let mut tracks: Vec<Option<Vec<u8>>> = vec![None; np * 4];
        let mut pattern_tracks = vec![[0usize; 4]; np];
        let mut count = 0;
        for i in 0..np {
            for j in 0..4 {
                let mut data = Some(vec![0u8; 256]);
                let mut o = 0usize;
                pattern_tracks[i][j] = count;
                let mut k = 0;
                while k < 64 {
                    let mut b = [0u8; 4];
                    for x in &mut b {
                        *x = s.read_u8();
                    }
                    let t = u16::from_be_bytes([b[0], b[1]]);
                    if t == 0xc000 {
                        // the same track as an earlier one
                        let n = u16::from_be_bytes([b[2], b[3]]) as usize / 4;
                        pattern_tracks[i][j] = *pattern_tracks.get(n / 4)?.get(n % 4)?;
                        data = None;
                        break;
                    }
                    if t < 0x8000 {
                        data.as_mut()?.get_mut(o..o + 4)?.copy_from_slice(&b);
                        o += 4;
                    } else {
                        // empty rows
                        k += b[3] as usize;
                        o += (b[3] as usize + 1) * 4;
                    }
                    k += 1;
                }
                if data.is_some() {
                    tracks[count] = data;
                    count += 1;
                }
            }
        }
        let mut out = Vec::new();
        for pt in &pattern_tracks {
            let mut pat = vec![0u8; 1024];
            for (j, &t) in pt.iter().enumerate() {
                let data = tracks.get(t)?.as_ref()?;
                for k in 0..64 {
                    pat[k * 16 + j * 4..k * 16 + j * 4 + 4].copy_from_slice(&data[k * 4..k * 4 + 4]);
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }

    fn patterns_mp(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        s.seek(self.start + 0x17a);
        // a zero long word before the patterns is skipped
        if s.read_b_u32() != 0 {
            s.seek_cur(-4);
        }
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            s.read_into(&mut pat);
            push!(out, pat, s);
        }
        // skip the stored patterns no position plays
        if self.base.number_of_patterns < self.real_patterns {
            s.seek_cur((self.real_patterns - self.base.number_of_patterns) as i64 * 1024);
        }
        Some(out)
    }
}

impl Format for Cryptoburners {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::ProPacker10 => "ProPacker 1.0",
            Kind::Heatseeker => "Heatseeker mc1.0",
            Kind::ModuleProtector => "Module Protector",
        }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        match self.kind {
            Kind::ProPacker10 => self.check_pp10(s),
            Kind::Heatseeker => self.check_heatseeker(s),
            Kind::ModuleProtector => self.check_mp(s),
        }
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        // NostalgicPlayer's loader rewinds the module before converting it
        s.seek(0);
        if self.kind == Kind::ModuleProtector {
            self.start = if s.read_mark() == b"TRK1" { 4 } else { 0 };
        }
        s.seek(self.start + 0xf8);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        match self.kind {
            Kind::ProPacker10 => {
                for t in &mut self.track_table {
                    *t = vec![0; 128];
                    s.read_into(t);
                }
                let tt = &self.track_table;
                let key = |p: usize| u32::from_be_bytes([tt[0][p], tt[1][p], tt[2][p], tt[3][p]]);
                self.position_list = self.base.build_position_list(self.positions as usize, key).0;
            }
            Kind::Heatseeker => {}
            Kind::ModuleProtector => {
                // unused patterns are not saved (Grapevine #3)
                self.real_patterns = self.base.number_of_patterns;
                self.base.find_highest_pattern_at(s, self.start + 0xfa, self.positions as usize);
            }
        }
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(self.start);
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
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        if self.kind == Kind::ProPacker10 {
            return Some(self.position_list.clone());
        }
        let mut list = vec![0u8; self.positions as usize];
        s.seek(self.start + 0xfa);
        s.read_into(&mut list);
        Some(list)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        match self.kind {
            Kind::ProPacker10 => self.patterns_pp10(s),
            Kind::Heatseeker => self.patterns_heatseeker(s),
            Kind::ModuleProtector => self.patterns_mp(s),
        }
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        if self.kind == Kind::ProPacker10 {
            s.seek(0x2fa + self.tracks_size as i64 * 4);
        }
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    fn header(song: &Song) -> Vec<u8> {
        let mut d = Vec::new();
        for i in 0..31 {
            let (len, ft, vol, ls, ll) = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(len.to_be_bytes());
            d.extend([ft, vol]);
            d.extend(ls.to_be_bytes());
            d.extend(ll.to_be_bytes());
        }
        d.extend([song.positions.len() as u8, song.restart]);
        d
    }

    fn samples(song: &Song, d: &mut Vec<u8>) {
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
    }

    fn positions(song: &Song) -> Vec<u8> {
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        pl
    }

    fn track(song: &Song, p: usize, v: usize) -> Vec<u8> {
        song.patterns[p].iter().flat_map(|r| Song::note(r[v])).collect()
    }

    /// ProPacker 1.0: track p * 4 + v plays voice v of pattern p.
    fn pack_pp10(song: &Song) -> Vec<u8> {
        let mut d = header(song);
        let mut tt = vec![0u8; 512];
        for (i, &p) in song.positions.iter().enumerate() {
            for v in 0..4 {
                tt[v * 128 + i] = p * 4 + v as u8;
            }
        }
        d.extend(tt);
        for p in 0..song.patterns.len() {
            for v in 0..4 {
                d.extend(track(song, p, v));
            }
        }
        samples(song, &mut d);
        d
    }

    /// Heatseeker: runs of empty rows packed, a track already stored referenced.
    fn pack_heatseeker(song: &Song) -> Vec<u8> {
        let mut d = header(song);
        d.extend(positions(song));
        let mut stored: Vec<(Vec<u8>, usize)> = Vec::new();
        for p in 0..song.patterns.len() {
            for v in 0..4 {
                let t = track(song, p, v);
                if let Some((_, n)) = stored.iter().find(|(x, _)| *x == t) {
                    d.extend([0xc0, 0]);
                    d.extend(((*n * 4) as u16).to_be_bytes());
                    continue;
                }
                stored.push((t.clone(), p * 4 + v));
                let mut r = 0;
                while r < 64 {
                    let empty = (r..64).take_while(|&k| t[k * 4..k * 4 + 4] == [0; 4]).count();
                    if empty > 0 {
                        d.extend([0x80, 0, 0, empty as u8 - 1]);
                        r += empty;
                    } else {
                        d.extend(&t[r * 4..r * 4 + 4]);
                        r += 1;
                    }
                }
            }
        }
        samples(song, &mut d);
        d
    }

    /// Module Protector: a module without names and mark, here with the zero long
    /// word before the patterns, and optionally behind "TRK1".
    fn pack_mp(song: &Song, trk1: bool) -> Vec<u8> {
        let mut d = if trk1 { b"TRK1".to_vec() } else { Vec::new() };
        d.extend(header(song));
        d.extend(positions(song));
        d.extend([0; 4]);
        for p in &song.patterns {
            for row in p {
                for n in row {
                    d.extend(Song::note(*n));
                }
            }
        }
        samples(song, &mut d);
        d
    }

    #[test]
    fn propacker10() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack_pp10(&song)).expect("ProPacker 1.0");
        assert_eq!(f, "ProPacker 1.0");
        assert_eq!(m, song.module(song.restart));
    }

    #[test]
    fn heatseeker() {
        // the check reads 512 notes, which must all be pattern data
        let mut song = Song::new(8);
        // a voice repeating an earlier track
        for r in 0..64 {
            song.patterns[2][r][3] = song.patterns[0][r][3];
        }
        let (f, m) = convert_any(&pack_heatseeker(&song)).expect("Heatseeker");
        assert_eq!(f, "Heatseeker mc1.0");
        assert_eq!(m, song.module(song.restart));
    }

    #[test]
    fn module_protector() {
        let song = Song::new(3);
        for trk1 in [false, true] {
            let (f, m) = convert_any(&pack_mp(&song, trk1)).expect("Module Protector");
            assert_eq!(f, "Module Protector");
            assert_eq!(m, song.module(song.restart));
        }
    }
}
