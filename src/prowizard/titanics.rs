//! Titanics Player, ported from NostalgicPlayer's TitanicsPlayerFormat: a
//! 15-sample module.
//!
//! 15 sample entries (data address, length, volume word, loop start and length),
//! then a pattern offset per position up to 0xffff, and the patterns: four bytes
//! an entry, the first one's bit 7 ending the pattern and its low bits the rows
//! from the previous entry (the first entry is on row 0), then the voice and the
//! note, and the ProTracker sample/effect byte and effect value. A note n plays
//! the table's period n (not n - 1), as the C# converter has it.

use super::core::*;

#[derive(Default)]
pub struct Titanics {
    base: Base,
    positions: usize,
    pattern_offsets: Vec<u16>,
    position_list: Vec<u8>,
    sample_addresses: Vec<u32>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(68, Box::new(Titanics::default()))]
}

impl Format for Titanics {
    fn name(&self) -> &'static str {
        "Titanics Player"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn fifteen(&self) -> bool {
        true
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 180 {
            return false;
        }
        s.seek(0);
        let mut samples_size = 0u32;
        for _ in 0..15 {
            let t = s.read_b_u32();
            if t as i64 > s.len() || (t < 180 && t != 0) {
                return false;
            }
            let length = s.read_b_u16();
            let volume = s.read_b_u16();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            if volume > 0x40 {
                return false;
            }
            if loop_start > length || loop_length as u32 > length as u32 + 1 || length > 32768 {
                return false;
            }
            if loop_length == 0 || (length == 0 && (loop_start != 0 || loop_length != 1)) {
                return false;
            }
            if length != 0 && length == loop_start {
                return false;
            }
            samples_size += length as u32;
        }
        if samples_size < 2 {
            return false;
        }
        // the pattern addresses
        for _ in 0..128 {
            if s.eof {
                return false;
            }
            let t = s.read_b_u16();
            if t == 0xffff {
                return true;
            }
            if t < 180 {
                return false;
            }
        }
        false
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(180);
        self.pattern_offsets = vec![0; 128];
        self.positions = 0;
        while self.positions < 128 {
            let o = s.read_b_u16();
            if o == 0xffff {
                break;
            }
            self.pattern_offsets[self.positions] = o;
            self.positions += 1;
        }
        let po = &self.pattern_offsets;
        self.position_list = self.base.build_position_list(self.positions, |p| po[p]).0;
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        self.sample_addresses = vec![0; 15];
        s.seek(0);
        let mut v = Vec::new();
        for i in 0..15 {
            self.sample_addresses[i] = s.read_b_u32();
            let length = s.read_b_u16();
            let volume = s.read_b_u16() as u8;
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: None, length, loop_start, loop_length, volume, fine_tune: 0 }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, _s: &mut Stream) -> Option<Vec<u8>> {
        Some(self.position_list.clone())
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        0x78
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut last: i32 = -1;
        for i in 0..self.positions {
            if self.position_list[i] as i32 > last {
                last += 1;
                let mut pat = vec![0u8; 1024];
                s.seek(self.pattern_offsets[i] as i64);
                let mut next = s.read_u8();
                let mut row = 0usize;
                loop {
                    let c1 = next;
                    let c2 = s.read_u8();
                    let c3 = s.read_u8();
                    let c4 = s.read_u8();
                    let o = row * 16 + ((c2 >> 6) & 3) as usize * 4;
                    let note = (c2 & 0x3f) as usize;
                    if note > 0 && note <= 36 {
                        pat[o] = PERIODS[note][0];
                        pat[o + 1] = PERIODS[note][1];
                    }
                    pat[o + 2] = c3;
                    pat[o + 3] = c4;
                    next = s.read_u8();
                    row += (next & 0x7f) as usize;
                    if row >= 64 || c1 & 0x80 != 0 {
                        break;
                    }
                    // (the C# loops forever at the end of the data)
                    if s.eof {
                        return None;
                    }
                }
                push!(out, pat, s);
            }
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        for i in 0..15 {
            let l = self.base.sample_lengths[i] as i64;
            if l != 0 {
                s.seek(self.sample_addresses[i] as i64);
                if s.len() - s.position() < l - MAX_MISSING {
                    return false;
                }
                w.set_sample_data_marker(s, l);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::PERIODS;
    use crate::prowizard::tests::*;

    /// The song without what Titanics cannot hold: fine tune, and the period at
    /// note 0 (856), which becomes 808.
    fn song() -> Song {
        let mut song = Song::new(3);
        song.samples[1].1 = 0;
        for p in &mut song.patterns {
            for row in p.iter_mut() {
                for n in row.iter_mut() {
                    if n.1 == 856 {
                        n.1 = 808;
                    }
                }
            }
        }
        song
    }

    fn pack(song: &Song) -> Vec<u8> {
        let mut sdata = Vec::new();
        let mut pats = Vec::new();
        let mut poffs = Vec::new();
        for p in &song.patterns {
            poffs.push(pats.len());
            let mut entries: Vec<(usize, usize, (u8, u16, u8, u8))> = Vec::new();
            for (r, row) in p.iter().enumerate() {
                for (v, &n) in row.iter().enumerate() {
                    if n != (0, 0, 0, 0) || (r == 0 && v == 0) {
                        entries.push((r, v, n));
                    }
                }
            }
            let mut prev = 0;
            for (i, &(r, v, (smp, per, fx, val))) in entries.iter().enumerate() {
                let last = if i + 1 == entries.len() { 0x80 } else { 0 };
                let note = if per == 0 { 0 } else { PERIODS.iter().position(|x| u16::from_be_bytes(*x) == per).unwrap() };
                pats.extend([last | (r - prev) as u8, (v << 6) as u8 | note as u8, (smp << 4) | fx, val]);
                prev = r;
            }
            pats.push(0);
        }
        let patterns_at = 180 + (song.positions.len() + 1) * 2;
        let samples_at = patterns_at + pats.len();
        let mut d = Vec::new();
        for i in 0..15 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            let addr = if i < song.samples.len() { samples_at + sdata.len() } else { 0 };
            d.extend((addr as u32).to_be_bytes());
            d.extend(s.0.to_be_bytes());
            d.extend((s.2 as u16).to_be_bytes());
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
            if i < song.samples.len() {
                sdata.extend(song.sample_data(i));
            }
        }
        for &p in &song.positions {
            d.extend(((patterns_at + poffs[p as usize]) as u16).to_be_bytes());
        }
        d.extend([0xff, 0xff]);
        d.extend(pats);
        d.extend(sdata);
        d
    }

    /// The 15-sample module: the 31-sample one without samples 16-31 and the mark,
    /// with Titanics' restart 0x78.
    fn module15(song: &Song) -> Vec<u8> {
        let m = song.module(0x78);
        let mut st = m[..20 + 15 * 30].to_vec();
        st.extend(&m[950..1080]);
        st.extend(&m[1084..]);
        st
    }

    #[test]
    fn titanics() {
        let song = song();
        let (f, m) = convert_any(&pack(&song)).expect("Titanics");
        assert_eq!(f, "Titanics Player");
        assert_eq!(m, module15(&song));
    }

    /// A pattern offset pointing at the end of the data must not loop for ever.
    #[test]
    fn titanics_truncated_pattern_terminates() {
        for off in [180u16, 184, 65534] {
            let mut d = vec![0u8; 180];
            for i in 0..15 {
                let o = i * 12;
                if i == 0 {
                    d[o + 4..o + 6].copy_from_slice(&2u16.to_be_bytes());
                }
                d[o + 10..o + 12].copy_from_slice(&1u16.to_be_bytes());
            }
            d.extend(off.to_be_bytes());
            d.extend([0xff, 0xff]);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(convert_any(&d).is_none());
            });
            rx.recv_timeout(std::time::Duration::from_secs(10)).expect("convert_any hangs");
        }
    }
}
