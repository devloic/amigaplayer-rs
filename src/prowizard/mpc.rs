//! Module-Patterncompressor ("PMD3"), ported from NostalgicPlayer's
//! ModulePatterncompressorFormat.
//!
//! A ProTracker header with the mark PMD3 (or PMd3), then the control and data
//! lengths and two offsets per pattern and voice: a voice is a list of control
//! bytes, each a note's offset in the data (a multiple of 4) and a repeat count
//! (low two bits, plus one). A module with 0x64 ('d') as the mark's third byte has
//! eight voices and converts with the mark CD81, which the built-in ProTracker does
//! not play.

use super::core::*;

#[derive(Default)]
pub struct Mpc {
    base: Base,
    eight_channels: bool,
    positions: u8,
    restart: u8,
    /// (control offset, data offset) per pattern and voice.
    pattern_offsets: Vec<Vec<(u32, u32)>>,
    control_start: i64,
    data_start: i64,
    sample_start: i64,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(57, Box::new(Mpc::default()))]
}

impl Format for Mpc {
    fn name(&self) -> &'static str {
        "Module-Patterncompressor"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 1092 {
            return false;
        }
        s.seek(1080);
        let mark = s.read_mark();
        if mark != b"PMD3" && mark != b"PMd3" {
            return false;
        }
        s.seek(20);
        let mut samples_size = 0u32;
        for _ in 0..31 {
            s.seek_cur(22);
            let size = s.read_b_u16().wrapping_mul(2);
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16().wrapping_mul(2);
            let loop_size = s.read_b_u16().wrapping_mul(2);
            if !test_sample(size, loop_start, loop_size, volume, fine_tune) {
                return false;
            }
            samples_size += size as u32;
        }
        if s.read_u8() > 127 {
            return false;
        }
        if self.base.find_highest_pattern_at(s, 952, 128) > 127 {
            return false;
        }
        s.seek(1084);
        let a = s.read_b_u32() as i64;
        let b = s.read_b_u32() as i64;
        1092 + a + b + samples_size as i64 <= s.len() + MAX_MISSING
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(950);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        s.seek(1080);
        self.eight_channels = s.read_b_u32() & 0xff00 == 0x6400;
        let control_len = s.read_b_u32();
        let data_len = s.read_b_u32();
        let channels = if self.eight_channels { 8 } else { 4 };
        let npat = self.base.number_of_patterns as usize;
        self.pattern_offsets = (0..npat)
            .map(|_| (0..channels).map(|_| (s.read_b_u32(), s.read_b_u32())).collect())
            .collect();
        self.control_start = 1092 + (npat * channels * 4 * 2) as i64;
        // C# int arithmetic on the uint lengths
        self.data_start = (self.control_start as i32).wrapping_add(control_len as i32) as i64;
        self.sample_start = (self.data_start as i32).wrapping_add(data_len as i32) as i64;
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut n = vec![0u8; 20];
        s.seek(0);
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        // follows the module name
        let mut v = Vec::new();
        for _ in 0..31 {
            let mut name = vec![0u8; 22];
            s.read_into(&mut name);
            let length = s.read_b_u16();
            let fine_tune = s.read_u8();
            let volume = s.read_u8();
            let loop_start = s.read_b_u16();
            let loop_length = s.read_b_u16();
            push!(v, SampleInfo { name: Some(name), length, loop_start, loop_length, volume, fine_tune }, s);
        }
        Some(v)
    }
    fn position_list(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        let mut p = vec![0u8; self.positions as usize];
        s.seek(952);
        s.read_into(&mut p);
        Some(p)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn mark(&self) -> &'static [u8; 4] {
        if self.eight_channels { b"CD81" } else { b"M.K." }
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let (channels, inc) = if self.eight_channels { (8, 32) } else { (4, 16) };
        let mut out = Vec::new();
        for i in 0..self.base.number_of_patterns as usize {
            let mut pat = vec![0u8; channels * 64 * 4];
            for j in 0..channels {
                let (co, dof) = self.pattern_offsets[i][j];
                let (mut k, mut m) = (0usize, 0i64);
                while k < 64 {
                    s.seek(self.control_start + co as i64 + m);
                    let c = s.read_u8();
                    let c1 = (c & 3) as usize + 1;
                    s.seek(self.data_start + dof as i64 + (c & 0xfc) as i64);
                    let mut n = [0u8; 4];
                    s.read_into(&mut n);
                    let mut w = j * 4 + k * inc;
                    for _ in 0..c1 {
                        // a repeat past the last row is out of the C# array
                        pat.get_mut(w..w + 4)?.copy_from_slice(&n);
                        w += inc;
                    }
                    k += c1;
                    m += 1;
                }
            }
            push!(out, pat, s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        s.seek(self.sample_start);
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// The song as a 4-voice PMD3: per pattern and voice the distinct notes in the
    /// data, and one control byte per row (repeats where a note repeats).
    fn pack(song: &Song) -> Vec<u8> {
        let m = song.module(song.restart);
        let mut d = m[..1080].to_vec();
        let npat = song.patterns.len();
        let mut data = Vec::new();
        let mut control = Vec::new();
        let mut offsets = Vec::new();
        for p in &song.patterns {
            for v in 0..4 {
                offsets.push((control.len() as u32, data.len() as u32));
                let mut notes: Vec<[u8; 4]> = Vec::new();
                let mut r = 0;
                while r < 64 {
                    let n = Song::note(p[r][v]);
                    let i = notes.iter().position(|x| *x == n).unwrap_or_else(|| {
                        notes.push(n);
                        notes.len() - 1
                    });
                    let mut rep = 1;
                    while rep < 4 && r + rep < 64 && Song::note(p[r + rep][v]) == n {
                        rep += 1;
                    }
                    control.push((i * 4) as u8 | (rep - 1) as u8);
                    r += rep;
                }
                data.extend(notes.concat());
            }
        }
        d.extend(b"PMD3");
        d.extend((control.len() as u32).to_be_bytes());
        d.extend((data.len() as u32).to_be_bytes());
        for (c, o) in offsets {
            d.extend(c.to_be_bytes());
            d.extend(o.to_be_bytes());
        }
        assert_eq!(d.len(), 1092 + npat * 32);
        d.extend(control);
        d.extend(data);
        for i in 0..song.samples.len() {
            d.extend(song.sample_data(i));
        }
        d
    }

    #[test]
    fn module_patterncompressor() {
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("PMD3");
        assert_eq!(f, "Module-Patterncompressor");
        assert_eq!(m, song.module(song.restart));
    }
}
