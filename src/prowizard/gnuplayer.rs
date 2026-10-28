//! GnuPlayer, ported from NostalgicPlayer's GnuPlayerFormat (ProWizard for PC).
//!
//! No patterns: two tracks of (command, argument) pairs, each playing a pair of
//! voices with one period for every note (set volume, slide volume, set speed,
//! advance rows, set note). The converter builds a pattern per position from them.
//! The samples are stored as 4-bit deltas.

use super::core::*;

#[derive(Default)]
pub struct GnuPlayer {
    base: Base,
    period: u16,
    track1: Vec<u8>,
    track2: Vec<u8>,
    positions: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(54, Box::new(GnuPlayer::default()))]
}

/// A track: its length (counting the length word) and data. A length under 2
/// fails, as the C# array does.
fn read_track(s: &mut Stream) -> Option<Vec<u8>> {
    let len = (s.read_b_u16() as usize).checked_sub(2)?;
    let mut t = vec![0u8; len];
    s.read_into(&mut t);
    Some(t)
}

/// The rows the track advances in all.
fn lines(t: &[u8]) -> Option<i32> {
    let mut n = 0;
    for i in (0..t.len()).step_by(2) {
        if t[i] == 4 {
            n += *t.get(i + 1)? as i32;
        }
    }
    Some(n)
}

impl GnuPlayer {
    /// FillTrack: the next 64 rows of a track into a voice pair of the pattern.
    fn fill_track(&self, pat: &mut [u8], mut po: usize, track: &[u8], to: &mut usize, empty: &mut i32) -> Option<()> {
        let mut eff_count = 0;
        let mut i = 0;
        while i < 64 {
            if *empty > 0 {
                po += 16;
                *empty -= 1;
                i += 1;
                continue;
            }
            let cmd = *track.get(*to)?;
            let arg = *track.get(*to + 1)?;
            if cmd == 0 {
                break;
            }
            *to += 2;
            match cmd {
                // set volume
                1 => {
                    pat[po + 2] |= 0x0c;
                    pat[po + 3] = arg;
                    pat[po + 6] |= 0x0c;
                    pat[po + 7] = arg;
                    eff_count += 1;
                }
                // slide volume
                2 => {
                    pat[po + 2] |= 0x0a;
                    pat[po + 3] = arg;
                    pat[po + 6] |= 0x0a;
                    pat[po + 7] = arg;
                    eff_count += 1;
                }
                // set speed, on the first voice without an effect
                3 => {
                    let o = if eff_count == 0 { po + 2 } else { po + 6 };
                    pat[o] |= 0x0f;
                    pat[o + 1] = arg;
                    eff_count += 1;
                }
                // advance rows
                4 => {
                    *empty = arg as i32;
                    eff_count = 0;
                }
                // set note: the argument is the sample
                5 => {
                    let b1 = (arg & 0xf0) | ((self.period >> 8) as u8 & 0x0f);
                    let b2 = self.period as u8;
                    let b3 = arg << 4;
                    pat[po] = b1;
                    pat[po + 1] = b2;
                    pat[po + 2] |= b3;
                    pat[po + 4] = b1;
                    pat[po + 5] = b2;
                    pat[po + 6] |= b3;
                }
                _ => {}
            }
        }
        Some(())
    }
}

impl Format for GnuPlayer {
    fn name(&self) -> &'static str {
        "GnuPlayer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x96 {
            return false;
        }
        s.seek(0x92);
        if s.read_mark() != b"GnPl" {
            return false;
        }
        s.seek(0x14);
        for _ in 0..31 {
            let size = s.read_b_u16() as u32;
            let loop_start = s.read_b_u16() as u32;
            if loop_start > size + 1 {
                return false;
            }
        }
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x90);
        self.period = s.read_b_u16();
        s.seek_cur(4);
        let (Some(t1), Some(t2)) = (read_track(s), read_track(s)) else { return false };
        let (Some(l1), Some(l2)) = (lines(&t1), lines(&t2)) else { return false };
        (self.track1, self.track2) = (t1, t2);
        self.positions = ((l1.max(l2) + 1) / 64) as u8;
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0u8; 20];
        s.seek(0);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let mut v = Vec::new();
        for _ in 0..31 {
            let length = s.read_b_u16();
            let loop_start = s.read_b_u16();
            let loop_length = if loop_start == 0 { 1 } else { length.wrapping_sub(loop_start) };
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume: 0x40, fine_tune: 0 }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        // a pattern per position
        Some((0..self.positions).collect())
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let (mut e1, mut p1, mut e2, mut p2) = (0, 0, 0, 0);
        let mut out = Vec::new();
        for _ in 0..self.positions {
            let mut pat = vec![0u8; 1024];
            self.fill_track(&mut pat, 0, &self.track1, &mut p1, &mut e1)?;
            self.fill_track(&mut pat, 8, &self.track2, &mut p2, &mut e2)?;
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0x96 + 2 + self.track1.len() as i64 + 2 + self.track2.len() as i64);
        for i in 0..31 {
            let length = self.base.sample_lengths[i] as usize;
            if length == 0 {
                continue;
            }
            if s.len() - s.position() < length as i64 / 2 - MAX_MISSING {
                return false;
            }
            let mut d = vec![0u8; length + 1];
            // skip the sample length
            s.seek_cur(2);
            let mut smp = s.read_i8();
            d[0] = smp as u8;
            let mut j = 1;
            while j < length {
                let b = s.read_u8();
                for nib in [b >> 4, b & 0x0f] {
                    let delta = if nib & 8 != 0 { nib.wrapping_sub(0x10) } else { nib };
                    smp = smp.wrapping_add(delta as i8);
                    d[j] = smp as u8;
                    j += 1;
                }
            }
            if s.position() % 2 != 0 {
                s.seek_cur(1);
            }
            w.write(&d[..length]);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    const PERIOD: u16 = 428;

    /// Two tracks over two patterns, the samples delta-packed.
    fn song() -> (Song, Vec<u8>) {
        let mut song = Song::new(2);
        song.samples = vec![(8, 0, 0x40, 0, 1), (16, 0, 0x40, 4, 12)];
        song.positions = vec![0, 1];
        let empty = [(0u8, 0u16, 0u8, 0u8); 4];
        for p in &mut song.patterns {
            p.iter_mut().for_each(|r| *r = empty);
        }
        let (a, b) = song.patterns.split_at_mut(1);
        a[0][0] = [(1, PERIOD, 0xc, 0x30), (1, PERIOD, 0xc, 0x30), (2, PERIOD, 0xa, 5), (2, PERIOD, 0xa, 5)];
        a[0][16][0] = (2, PERIOD, 0, 0);
        a[0][16][1] = (2, PERIOD, 0, 0);
        b[0][0][0] = (1, PERIOD, 0xf, 6);
        b[0][0][1] = (1, PERIOD, 0, 0);
        let t1: Vec<u8> = vec![5, 1, 1, 0x30, 4, 16, 5, 2, 4, 48, 3, 6, 5, 1, 4, 64, 0, 0];
        let t2: Vec<u8> = vec![2, 5, 5, 2, 4, 128, 0, 0];
        let mut d = vec![0u8; 20];
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(s.0.to_be_bytes());
            d.extend(s.3.to_be_bytes());
        }
        d.extend(PERIOD.to_be_bytes());
        d.extend(b"GnPl");
        for t in [&t1, &t2] {
            d.extend((t.len() as u16 + 2).to_be_bytes());
            d.extend(t);
        }
        for s in &song.samples {
            let data = data(s.0 as usize * 2);
            d.extend(s.0.to_be_bytes());
            d.push(data[0] as u8);
            let mut deltas: Vec<u8> = data.windows(2).map(|w| (w[1] - w[0]) as u8 & 0x0f).collect();
            deltas.push(0);
            d.extend(deltas.chunks(2).map(|c| c[0] << 4 | c[1]));
            if d.len() % 2 != 0 {
                d.push(0);
            }
        }
        (song, d)
    }

    fn data(n: usize) -> Vec<i8> {
        (0..n).map(|k| (k % 8) as i8 - 4).collect()
    }

    #[test]
    fn gnu_player() {
        let (song, packed) = song();
        let (f, m) = convert_any(&packed).expect("GnuPlayer");
        assert_eq!(f, "GnuPlayer");
        let mut want = song.module(0x7f);
        want.truncate(1084 + 2 * 1024);
        // every sample has volume 64, the empty ones too
        for i in 2..31 {
            want[20 + i * 30 + 25] = 0x40;
        }
        for s in &song.samples {
            want.extend(data(s.0 as usize * 2).iter().map(|&x| x as u8));
        }
        assert_eq!(m, want);
    }
}
