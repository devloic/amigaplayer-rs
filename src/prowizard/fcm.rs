//! FC-M Packer, ported from NostalgicPlayer's FcmPackerFormat.
//!
//! An IFF-like file: "FC-M", then NAME (title), INST (the ProTracker sample
//! table without names), LONG (song length, restart), PATT (position list),
//! SONG (ProTracker patterns) and SAMP (sample data) parts.

use super::core::*;

const NAME: u32 = 0x4e414d45;
const INST: u32 = 0x494e5354;
const LONG: u32 = 0x4c4f4e47;
const PATT: u32 = 0x50415454;
const SONG: u32 = 0x534f4e47;
const SAMP: u32 = 0x53414d50;

#[derive(Default)]
pub struct Fcm {
    base: Base,
    positions: u8,
    restart: u8,
}

pub fn formats() -> Vec<(u32, Box<dyn Format>)> {
    vec![(8, Box::new(Fcm::default()))]
}

/// FindPart: the stream just after the part's id, the first found at an even
/// offset. (The C# search reads through a 1 KiB buffer and loses 2 bytes of its
/// count with every refill, so past the first 2 KiB it seeks a little before the
/// part: the sample data came out shifted. This searches the data itself.)
fn find_part(s: &mut Stream, chunk: u32) -> bool {
    let d = s.data();
    let id = chunk.to_be_bytes();
    match (0..d.len().saturating_sub(3)).step_by(2).find(|&i| d[i..i + 4] == id) {
        Some(i) => {
            s.seek(i as i64 + 4);
            true
        }
        None => {
            // as the C# search ends: read to the end
            s.seek(s.len());
            s.eof = true;
            false
        }
    }
}

impl Format for Fcm {
    fn name(&self) -> &'static str {
        "FC-M Packer"
    }
    fn base(&mut self) -> &mut Base {
        &mut self.base
    }
    fn check(&mut self, s: &mut Stream) -> bool {
        if s.len() < 34 {
            return false;
        }
        s.seek(0);
        if s.read_b_u32() != 0x46432d4d {
            return false;
        }
        if !find_part(s, INST) || !find_part(s, LONG) {
            return false;
        }
        let len = s.read_u8();
        if !find_part(s, PATT) {
            return false;
        }
        self.base.find_highest_pattern_read(s, len as usize);
        true
    }
    fn prepare(&mut self, s: &mut Stream) -> bool {
        if !find_part(s, LONG) {
            return false;
        }
        self.positions = s.read_u8();
        self.restart = s.read_u8();
        true
    }
    fn module_name(&mut self, s: &mut Stream) -> Option<Vec<u8>> {
        if !find_part(s, NAME) {
            return None;
        }
        let mut n = vec![0u8; 20];
        s.read_into(&mut n);
        Some(n)
    }
    fn samples(&mut self, s: &mut Stream) -> Option<Vec<SampleInfo>> {
        let mut v = Vec::new();
        if !find_part(s, INST) {
            // no samples: the driver fails
            return Some(v);
        }
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
        if !find_part(s, PATT) {
            return None;
        }
        let mut v = vec![0u8; self.positions as usize];
        s.read_into(&mut v);
        Some(v)
    }
    fn restart(&mut self, _s: &mut Stream) -> u8 {
        self.restart
    }
    fn patterns(&mut self, s: &mut Stream) -> Option<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        if !find_part(s, SONG) {
            // no patterns written, as the C# generator ends
            return Some(out);
        }
        let mut pat = vec![0u8; 1024];
        for _ in 0..self.base.number_of_patterns {
            s.read_into(&mut pat);
            push!(out, pat.clone(), s);
        }
        Some(out)
    }
    fn write_sample_data(&mut self, s: &mut Stream, w: &mut Out, _: &dyn Loader) -> bool {
        if !find_part(s, SAMP) {
            return false;
        }
        self.base.save_marks_for_all_samples(s, w)
    }
}

#[cfg(test)]
mod tests {
    use crate::prowizard::tests::*;

    /// Packs the song as FC-M does.
    fn pack(song: &Song) -> Vec<u8> {
        let m = song.module(song.restart);
        let mut d = b"FC-M\0\x01NAME".to_vec();
        d.extend(&m[..20]);
        d.extend(b"INST");
        for i in 0..31 {
            let o = 20 + i * 30 + 22;
            d.extend(&m[o..o + 8]);
        }
        d.extend(b"LONG");
        d.extend([song.positions.len() as u8, song.restart]);
        d.extend(b"PATT");
        d.extend(&song.positions);
        if d.len() % 2 != 0 {
            d.push(0);
        }
        d.extend(b"SONG");
        d.extend(&m[1084..1084 + song.patterns.len() * 1024]);
        d.extend(b"SAMP");
        d.extend(&m[1084 + song.patterns.len() * 1024..]);
        d
    }

    #[test]
    fn fcm() {
        // 3 patterns: SAMP is past the C# search's first 2 KiB
        let song = Song::new(3);
        let (f, m) = convert_any(&pack(&song)).expect("FC-M Packer");
        assert_eq!(f, "FC-M Packer");
        assert_eq!(m, song.module(song.restart));
    }
}
