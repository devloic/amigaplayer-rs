//! Pha Packer, ported from NostalgicPlayer's PhaPackerFormat.
//!
//! 14 bytes per sample (with its address, the fine tune as a period offset), a
//! pattern address per position, the sample data at 0x3c0, then the patterns:
//! (sample, note number, effect, parameter) per note in row order, where 0xff and
//! a negative count holds the last note that many rows on its voice.

use super::core::*;

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(20, Box::new(PhaPacker::default()))]
}

#[derive(Default)]
pub struct PhaPacker {
    base: Base,
    end_offset: u32,
    positions: u8,
    pattern_offsets: Vec<u32>,
    position_list: Vec<u8>,
}

impl Format for PhaPacker {
    fn name(&self) -> &'static str {
        "Pha Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 0x3c0 {
            return false;
        }
        // the first sample's address
        s.seek(8);
        if s.read_b_u32() != 0x3c0 {
            return false;
        }
        s.seek(0x1b4);
        let t = s.read_b_u16();
        if t == 0 || t > 0x200 || t & 3 != 0 {
            return false;
        }
        // the sample table
        s.seek(0);
        for _ in 0..31 {
            let len = s.read_b_u16();
            s.seek_cur(1);
            if s.read_u8() > 0x40 {
                return false;
            }
            let lp = s.read_b_u16().wrapping_add(s.read_b_u16());
            if len == 0 {
                if lp != 1 {
                    return false;
                }
            } else if lp == 0 || lp > len {
                return false;
            }
            s.seek_cur(4);
            if s.read_b_u16() % 0x48 != 0 {
                return false;
            }
        }
        // the last pattern, which must end where the module does
        s.seek(0x1c0);
        let mut offset = 0u32;
        for _ in 0..128 {
            offset = offset.max(s.read_b_u32());
        }
        let mut count: i16 = 0;
        for i in 0..64 * 4 {
            count = count.wrapping_add(1);
            if count >= 0 {
                // the next note
                offset = offset.wrapping_add(4);
                s.seek(offset as i64);
                let t = s.read_b_u16();
                if t >= 0x8000 {
                    // a new counter
                    offset = offset.wrapping_add(2);
                    count = t as i16;
                    if t & 0xff00 == 0xff00 {
                        continue;
                    }
                    if i != 64 * 4 - 1 {
                        return false;
                    }
                    offset = offset.wrapping_sub(2);
                }
            }
        }
        if offset as i64 > s.len() {
            return false;
        }
        self.end_offset = offset;
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0x1b4);
        self.positions = (s.read_b_u16() / 4) as u8;
        s.seek(0x1c0);
        let (list, offsets) = self.base.build_position_list(self.positions as usize, |_| s.read_b_u32());
        self.position_list = list;
        self.pattern_offsets = offsets;
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        s.seek(0);
        let mut v = Vec::new();
        for _ in 0..31 {
            let length = s.read_b_u16();
            s.seek_cur(1);
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            s.seek_cur(4);
            let fine_tune = (s.read_b_u16() / 0x48) as u8;
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune }, s);
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
            if self.position_list[i] as i32 <= last {
                continue;
            }
            last += 1;
            let offset = self.pattern_offsets[last as usize];
            if offset == 0 {
                break;
            }
            let mut pat = vec![0u8; 1024];
            s.seek(offset as i64);
            // where the next note goes; C#'s index errors fail the conversion
            let mut wo: usize = 0;
            let mut j = 0;
            while j < 256 {
                if s.position() >= self.end_offset as i64 {
                    break;
                }
                let t = s.read_u8();
                if t == 0xff {
                    // hold the last note for this many rows
                    let lines = (s.read_i8() as i32).wrapping_neg() as i8;
                    j -= 1;
                    let mut to = wo.checked_sub(4)?;
                    let mut b = [pat[to], pat[to + 1], pat[to + 2], pat[to + 3]];
                    if b == [0; 4] {
                        // held silence, cleared again below
                        b = [0xee; 4];
                    }
                    for _ in 0..lines {
                        pat.get_mut(to..to + 4)?.copy_from_slice(&b);
                        to += 16;
                    }
                } else {
                    // sample
                    let mut b3 = t;
                    let mut b1 = 0;
                    if b3 >= 0x10 {
                        b1 = 0x10;
                        b3 -= 0x10;
                    }
                    b3 <<= 4;
                    // note
                    let mut b2 = s.read_u8();
                    if b2 != 0 {
                        let p = PERIODS.get((b2.wrapping_sub(2) / 2) as usize)?;
                        b1 |= p[0];
                        b2 = p[1];
                    }
                    // effect
                    b3 |= s.read_u8() & 0x0f;
                    let mut b4 = s.read_u8();
                    // position jump
                    if b3 & 0x0f == 0x0b {
                        b4 = b4.wrapping_add(1);
                    }
                    // fix for "Raw 3 - Intro"
                    if b3 & 0x0f == 0x0f && b4 == 0x20 {
                        b4 = 0x1f;
                    }
                    // fix for "What Mag"
                    if b3 & 0x0f == 0x0f && b4 == 0x4f {
                        b3 &= 0xf0;
                        b4 = 0;
                    }
                    // the next free row
                    while *pat.get(wo..wo + 4)? != [0; 4] {
                        wo += 4;
                        j += 1;
                        if j == 256 {
                            break;
                        }
                    }
                    if j == 256 {
                        break;
                    }
                    pat[wo..wo + 4].copy_from_slice(&[b1, b2, b3, b4]);
                    wo += 4;
                }
                j += 1;
            }
            // the held silences
            for c in pat.chunks_mut(4) {
                if c == [0xee; 4] {
                    c.fill(0);
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(0x3c0);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// Pack the song as Pha Packer: an empty note followed by empty rows on its
    /// voice is held with 0xff.
    fn pack(song: &Song) -> Vec<u8> {
        let mut d = Vec::new();
        let mut addr = 0x3c0u32;
        for i in 0..31 {
            let (len, ft, vol, ls, ll) = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(len.to_be_bytes());
            d.extend([0, vol]);
            d.extend(ls.to_be_bytes());
            d.extend(ll.to_be_bytes());
            d.extend(addr.to_be_bytes());
            d.extend((ft as u16 * 0x48).to_be_bytes());
            addr += len as u32 * 2;
        }
        d.resize(0x1b4, 0);
        d.extend((song.positions.len() as u16 * 4).to_be_bytes());
        d.resize(0x1c0, 0);
        let mut pats = Vec::new();
        let mut offs = Vec::new();
        let per = |p: u16| super::PERIODS.iter().position(|x| ((x[0] as u16) << 8 | x[1] as u16) == p).unwrap() as u8;
        for p in &song.patterns {
            offs.push(addr + pats.len() as u32);
            let mut held = [false; 256];
            for c in 0..256 {
                if held[c] {
                    continue;
                }
                let (smp, period, fx, par) = p[c / 4][c % 4];
                let note = if period != 0 { per(period) * 2 + 2 } else { 0 };
                pats.extend([smp, note, fx, par]);
                if (smp, period, fx, par) == (0, 0, 0, 0) {
                    let mut n = 0;
                    while c / 4 + n + 1 < 64 && p[c / 4 + n + 1][c % 4] == (0, 0, 0, 0) && n < 100 {
                        n += 1;
                    }
                    if n > 0 {
                        for k in 1..=n {
                            held[c + k * 4] = true;
                        }
                        pats.extend([0xff, (-(n as i8 + 1)) as u8]);
                    }
                }
            }
        }
        for &q in &song.positions {
            d.extend(offs[q as usize].to_be_bytes());
        }
        d.resize(0x3c0, 0);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d.extend(pats);
        d
    }

    #[test]
    fn pha() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("Pha Packer");
        assert_eq!(f, "Pha Packer");
        assert_eq!(m, song.module(0x7f));
    }
}
