//! Magnetic Fields Packer, ported from NostalgicPlayer's
//! MagneticFieldsPackerFormat (after libxmp).
//!
//! The module (mfp.*) holds the sample table, the position list, and per position
//! four channel offsets into the pattern data, where each channel of a pattern is
//! a three-level table: four bytes for the 16-row quarters, four per quarter for
//! the 4-row groups, four per group for the rows, each row a word index of a
//! ProTracker note. The sample data is in a companion file (smp.*), or for Kid
//! Chaos and Kid Vicious in a sample set shared by several songs (*.set).

use super::core::*;

#[derive(Default)]
pub struct Mfp {
    base: Base,
    positions: u8,
    restart: u8,
    position_list: Vec<u8>,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(71, Box::new(Mfp::default()))]
}

/// ConvertPattern: one pattern from the channels' offsets, None if any leads out
/// of the data.
fn convert_pattern(input: &[u8], starts: [u16; 4]) -> Option<Vec<u8>> {
    let mut out = vec![0u8; 1024];
    let len = input.len() as i64;
    for (j, &start) in starts.iter().enumerate() {
        let start = start as usize;
        if start as i64 >= len - 4 {
            return None;
        }
        let mut row = 0;
        for k in 0..4 {
            for x in 0..4 {
                for y in 0..4 {
                    let o = start + input[start + k] as usize + x;
                    let o = start + *input.get(o)? as usize + y;
                    let o = start + *input.get(o)? as usize * 2;
                    let n = input.get(o..o + 4)?;
                    out[row * 16 + j * 4..row * 16 + j * 4 + 4].copy_from_slice(n);
                    row += 1;
                }
            }
        }
    }
    Some(out)
}

impl Format for Mfp {
    fn name(&self) -> &'static str {
        "Magnetic Fields Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 384 {
            return false;
        }
        // the restart byte
        s.seek(249);
        if s.read_u8() != 0x7f {
            return false;
        }
        s.seek(0);
        for _ in 0..31 {
            let len = s.read_b_u16();
            if len > 0x7fff || s.read_u8() & 0xf0 != 0 || s.read_u8() > 64 {
                return false;
            }
            let loop_start = s.read_b_u16();
            if loop_start > len {
                return false;
            }
            let loop_length = s.read_b_u16();
            if loop_start as i32 + loop_length as i32 - 1 > len as i32 || (len > 0 && loop_length == 0) {
                return false;
            }
        }
        let positions = s.read_u8() as u16;
        s.seek(378);
        let t = s.read_b_u16();
        positions == t && t == s.read_b_u16()
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        s.seek(0xf8);
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        s.seek(0);
        true
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
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
        let n = self.positions as usize;
        let mut list = vec![0u8; n];
        s.seek(0xfa);
        s.read_into(&mut list);
        s.seek_cur(128 - n as i64);
        self.base.find_highest_pattern(&list);
        self.position_list = list.clone();
        Some(list)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let size1 = s.read_b_u16() as usize;
        s.seek_cur(2);
        let mut table = vec![[0u16; 4]; size1];
        for t in &mut table {
            for v in t.iter_mut() {
                *v = s.read_b_u16();
            }
        }
        let mut input = vec![0u8; (s.len() - s.position()) as usize];
        s.read_into(&mut input);
        // convert each pattern where it is first played (C# throws past the table)
        let mut converted: Vec<Option<Vec<u8>>> = vec![None; 256];
        for i in 0..self.positions as usize {
            let p = self.position_list[i] as usize;
            if converted[p].is_none() {
                converted[p] = Some(convert_pattern(&input, *table.get(i)?)?);
            }
        }
        let mut out = Vec::new();
        for i in 0..self.base.number_of_patterns as usize {
            // an unused pattern is empty
            push!(out, converted[i].take().unwrap_or_else(|| vec![0; 1024]), s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, _s: &mut Stream, w: &mut Out, files: &dyn Loader) -> bool {
        let data = files.open_by_extension("smp").or_else(|| {
            // a sample set, as used in Kid Chaos and Kid Vicious: the name without
            // its "-N" part, with .set
            files.possible_names("smp").iter().find_map(|name| {
                let mut n = name.clone();
                if let Some(i) = n.rfind('-') {
                    n = match n[i + 1..].find('.') {
                        Some(i2) => format!("{}{}", n[..i].trim_end(), &n[i + 1 + i2..]),
                        None => n[..i].trim_end().to_string(),
                    };
                }
                n.push_str(".set");
                files.open_by_name(&n)
            })
        });
        let Some(data) = data else { return false };
        let mut ss = Stream::new(&data);
        for &l in &self.base.sample_lengths {
            let l = l as i64;
            if l != 0 {
                if ss.len() - ss.position() < l - MAX_MISSING {
                    return false;
                }
                copy_data(&mut ss, w, l);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::core::*;
    use crate::prowizard::tests::*;

    /// The song as MFP: per position, each channel a three-level table with all
    /// 64 notes of its own (no sharing), and the sample data apart.
    fn pack(song: &Song) -> (Vec<u8>, Vec<u8>) {
        let mut d = Vec::new();
        for i in 0..31 {
            let s = song.samples.get(i).copied().unwrap_or((0, 0, 0, 0, 1));
            d.extend(s.0.to_be_bytes());
            d.extend([s.1, s.2]);
            d.extend(s.3.to_be_bytes());
            d.extend(s.4.to_be_bytes());
        }
        let n = song.positions.len();
        d.push(n as u8);
        d.push(0x7f);
        let mut pl = song.positions.clone();
        pl.resize(128, 0);
        d.extend(pl);
        d.extend((n as u16).to_be_bytes());
        d.extend((n as u16).to_be_bytes());
        // a channel: 4 quarter offsets, 16 group tables, then 64 notes
        let mut data = Vec::new();
        let mut table = Vec::new();
        for &p in &song.positions {
            for v in 0..4 {
                table.extend((data.len() as u16).to_be_bytes());
                let mut ch = vec![4u8, 8, 12, 16];
                for g in 0..16u8 {
                    ch.push(20 + g * 4);
                }
                for g in 0..16u8 {
                    for y in 0..4u8 {
                        ch.push(42 + (g * 4 + y) * 2);
                    }
                }
                for row in &song.patterns[p as usize] {
                    ch.extend(Song::note(row[v]));
                }
                data.extend(ch);
            }
        }
        d.extend(table);
        d.extend(data);
        let smp = (0..song.samples.len()).flat_map(|i| song.sample_data(i)).collect();
        (d, smp)
    }

    struct Smp(Vec<u8>);
    impl Loader for Smp {
        fn possible_names(&self, _: &str) -> Vec<String> {
            vec!["song.smp".into()]
        }
        fn open_by_name(&self, name: &str) -> Option<Vec<u8>> {
            (name == "song.smp").then(|| self.0.clone())
        }
    }

    #[test]
    fn mfp() {
        let song = Song::new(3);
        let (d, smp) = pack(&song);
        let files = Smp(smp);
        let (f, m) = crate::prowizard::formats()
            .into_iter()
            .find_map(|mut f| run(f.as_mut(), &d, &files).map(|m| (f.name(), m)))
            .expect("not recognised");
        assert_eq!(f, "Magnetic Fields Packer");
        assert_eq!(m, song.module(0x7f));
        // without its sample file it does not convert
        assert!(convert_any(&d).is_none());
    }
}
