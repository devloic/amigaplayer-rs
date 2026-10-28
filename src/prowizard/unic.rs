//! Unic Tracker and Laxity Tracker, ported from NostalgicPlayer's
//! UnicTrackerFormatBase, UnicTrackerFormat and LaxityTrackerFormat.
//!
//! Both keep ProTracker's header with a fine tune word in the sample name's last
//! two bytes, and store notes in 3 bytes: the note number with the sample's high
//! bit, then the sample's low nibble and the effect. Laxity has no title (its
//! samples start at 0) and no mark; Unic has a title and usually a "M.K."-like
//! mark, which some modules lack.

use super::core::*;

#[derive(Default)]
pub struct Unic {
    base: Base,
    laxity: bool,
    has_id: bool,
    restart: u8,
    real_patterns: u16,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![
        (45, Box::new(Unic::default())),
        (13, Box::new(Unic { laxity: true, ..Default::default() })),
    ]
}

impl Unic {
    /// The sample table checks the two formats share: the total sample size.
    fn check_samples(s: &mut Stream) -> Option<u32> {
        let mut size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
            let len = s.read_b_u16();
            if len >= 0x8000 {
                return None;
            }
            if s.read_u8() != 0 || s.read_u8() > 0x40 {
                return None;
            }
            if len == 0 {
                s.seek_cur(2);
                if s.read_b_u16() != 1 {
                    return None;
                }
            } else {
                size = size.wrapping_add(len as u32 * 2);
                let a = s.read_b_u16() as u32;
                let b = s.read_b_u16() as u32;
                if a + b > len as u32 {
                    return None;
                }
            }
        }
        Some(size)
    }

    /// CheckPatterns: whether the data reads as ProTracker patterns with notes.
    fn check_patterns(&self, s: &mut Stream) -> bool {
        let mut got = false;
        for _ in 0..self.base.number_of_patterns as u32 * 128 {
            let t = s.read_b_u32();
            if t != 0 {
                got = true;
                let t = t & 0x0fff_0000;
                if t != 0 && !(0x0071_0000..=0x0358_0000).contains(&t) {
                    return false;
                }
            }
        }
        got
    }

    fn check_unic(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x43c {
            return false;
        }
        s.seek(0x3b6);
        let t = s.read_u8();
        if t > 0x7f || t == 0 {
            return false;
        }
        let t = s.read_u8();
        if t > 0x3f && t != 0x7f {
            return false;
        }
        if s.read_u8() > 0x3f || s.read_u8() > 0x3f {
            return false;
        }
        s.seek(0x438);
        if s.read_mark() == b"SNT." {
            return false;
        }
        s.seek(20);
        let Some(size) = Self::check_samples(s) else { return false };
        if self.base.find_highest_pattern_at(s, 0x3b8, 128) > 0x40 {
            return false;
        }
        let np = self.base.number_of_patterns as i64;
        if 0x43c + np * 0x300 + size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        s.seek(0x43c);
        if self.check_patterns(s) {
            return false;
        }
        s.seek(0x43c);
        let mut ok = true;
        for _ in 0..np * 256 {
            if s.read_u8() & 0x3f > 37 {
                ok = false;
                break;
            }
            s.seek_cur(2);
        }
        self.has_id = true;
        if !ok {
            // maybe a Unic module without the mark
            s.seek(0x438);
            for _ in 0..np * 256 {
                if s.read_u8() & 0x3f > 37 {
                    return false;
                }
                s.seek_cur(2);
            }
            self.has_id = false;
        }
        true
    }

    fn check_laxity(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x43c {
            return false;
        }
        s.seek(0x3a2);
        let positions = s.read_u8();
        if positions > 0x7f || positions == 0 {
            return false;
        }
        let t = s.read_u8();
        if t > 0x3f && t != 0x7f {
            return false;
        }
        if s.read_u8() > 0x3f || s.read_u8() > 0x3f {
            return false;
        }
        s.seek(0x438);
        if s.read_mark() == b"M.K." {
            return false;
        }
        s.seek(0);
        let Some(size) = Self::check_samples(s) else { return false };
        if self.base.find_highest_pattern_at(s, 0x3a4, positions as usize) > 0x40 {
            return false;
        }
        let np = self.base.number_of_patterns as i64;
        if 0x424 + np * 0x300 + size as i64 > s.len() + MAX_MISSING {
            return false;
        }
        s.seek(0x424);
        if self.check_patterns(s) {
            return false;
        }
        s.seek(0x424);
        for _ in 0..np * 256 {
            if s.read_u8() > 0x66 {
                return false;
            }
            s.seek_cur(2);
        }
        true
    }
}

impl Format for Unic {
    fn name(&self) -> &'static str {
        if self.laxity { "Laxity Tracker" } else { "Unic Tracker" }
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if self.laxity { self.check_laxity(s) } else { self.check_unic(s) }
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        // NostalgicPlayer's loader rewinds the module before converting it
        s.seek(0);
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        if self.laxity {
            return None;
        }
        let mut n = vec![0u8; 20];
        s.seek(0);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        // fine tunes are converted only when the NTK byte is 0x7f or 0
        let pos = s.position();
        s.seek(if self.laxity { 0x3a3 } else { 0x3b7 });
        let t = s.read_u8();
        let set_fine_tune = t == 0x7f || t == 0;
        s.seek(pos);
        // GetAllSamples
        let mut v = Vec::new();
        for _ in 0..31 {
            let mut name = vec![0u8; 22];
            s.read_into(&mut name[..20]);
            let fine_tune = if set_fine_tune {
                ((-(s.read_b_u16() as u8 as i8 as i32)) & 0x0f) as u8
            } else {
                s.seek_cur(2);
                0
            };
            let length = s.read_b_u16();
            let volume = s.read_b_u16() as u8;
            let mut loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            if loop_start as i32 * 2 + loop_length as i32 <= length as i32 {
                loop_start = loop_start.wrapping_mul(2);
            }
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let n = s.read_u8();
        self.restart = s.read_u8();
        if self.restart == 0 {
            self.restart = 0x7f;
        }
        let mut list = vec![0u8; n as usize];
        s.read_into(&mut list);
        s.seek_cur(128 - n as i64);
        // unused patterns are not saved
        self.real_patterns = self.base.number_of_patterns;
        self.base.find_highest_pattern(&list);
        Some(list)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        if !self.laxity && self.has_id {
            s.seek_cur(4);
        }
        let mut out = Vec::new();
        for _ in 0..self.base.number_of_patterns {
            let mut pat = vec![0u8; 1024];
            for j in 0..64 * 4 {
                let t = s.read_u8();
                let mut b1: u8 = if t & 0x40 != 0 { 0x10 } else { 0 };
                let mut b3 = s.read_u8();
                let mut b4 = s.read_u8();
                let mut b2 = t & 0x3f;
                if b2 != 0 && b2 != 0x3f {
                    let p = PERIODS.get(b2 as usize - 1)?;
                    b1 |= p[0];
                    b2 = p[1];
                } else {
                    b2 = 0;
                }
                // pattern break arguments are decimal in ProTracker
                if b3 & 0x0f == 0x0d {
                    b4 = ((b4 / 10) as u32 * 0x10 + (b4 % 10) as u32) as u8;
                }
                // Beast-Busters.unic stops a sub-song with 800: make it F00
                if b3 & 0x0f == 0x08 && b4 == 0 {
                    b3 = (b3 & 0xf0) | 0x0f;
                }
                pat[j * 4..j * 4 + 4].copy_from_slice(&[b1, b2, b3, b4]);
            }
            push!(out, pat, s);
        }
        // skip the stored patterns no position plays
        if self.base.number_of_patterns < self.real_patterns {
            s.seek_cur((self.real_patterns - self.base.number_of_patterns) as i64 * 0x300);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use super::super::core::PERIODS;
    use crate::prowizard::tests::*;

    /// The 1-based note number of a ProTracker period.
    fn note(period: u16) -> u8 {
        if period == 0 {
            return 0;
        }
        1 + PERIODS.iter().position(|p| ((p[0] as u16) << 8 | p[1] as u16) == period).unwrap() as u8
    }

    /// Unic (`laxity` false: title, samples at 20, a mark) or Laxity Tracker.
    fn pack(song: &Song, laxity: bool) -> Vec<u8> {
        let mut d = if laxity { Vec::new() } else { vec![0u8; 20] };
        for i in 0..31 {
            let (len, ft, vol, ls, ll) = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend([0u8; 20]);
            // the fine tune word, negated
            d.extend([0, (-(ft as i32)) as u8]);
            d.extend(len.to_be_bytes());
            d.extend((vol as u16).to_be_bytes());
            // the loop start in words where it fits
            d.extend(if ls % 2 == 0 && ls + ll <= len { ls / 2 } else { ls }.to_be_bytes());
            d.extend(ll.to_be_bytes());
        }
        // the NTK byte 0x7f: fine tunes are set, the restart is 0x7f
        d.extend([song.positions.len() as u8, 0x7f]);
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d.extend(pl);
        if !laxity {
            d.extend(b"UNIC");
        }
        for p in &song.patterns {
            for row in p {
                for &(s, per, e, v) in row {
                    d.extend([((s & 0x10) << 2) | note(per), ((s & 0x0f) << 4) | e, v]);
                }
            }
        }
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn unic_and_laxity() {
        let song = Song::new(3);
        for (laxity, name) in [(false, "Unic Tracker"), (true, "Laxity Tracker")] {
            let (f, m) = convert_any(&pack(&song, laxity)).expect(name);
            assert_eq!(f, name);
            assert_eq!(m, song.module(0x7f));
        }
    }
}
