//! Sierra AGI sound files (`agi.*`), the music and sound effects of the PC
//! adventure games (King's Quest, Space Quest and others) as the Amiga versions
//! play them.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour, and this player was written from it alone and checked against the
//! Paula register writes of UADE's Sierra-AGI player.
//!
//! A module is four note lists (three tone voices and one noise voice), each note
//! five bytes: a 16-bit little-endian duration in ticks, two bytes of pitch, and an
//! attenuation. The tone voices play a 4-byte square wave, the noise voice a 4096
//! byte table that the player generates; both loop, and a note is the sample
//! pointer and length, a period and a volume, and the DMA enable again. The tick is
//! 60 Hz. When every voice has ended the song ends, and a tune with an empty noise
//! voice starts again at once.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Sierra AGI", detect, new };

/// The end marker of a voice, in the place of a duration.
const END: u16 = 0xFFFF;
const NOISE_LEN: usize = 4096;
/// Paula periods of the noise voice for its selector.
const NOISE_PERIOD: [u16; 4] = [0x0200, 0x0400, 0x0800, 0x0800];

fn le16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

/// The offset of the end marker of the voice stream starting at `p`, walking its
/// five-byte notes, if it has one in `d`.
fn find_end(d: &[u8], mut p: usize) -> Option<usize> {
    loop {
        if le16(d, p)? == END {
            return Some(p);
        }
        p += 5;
    }
}

/// The module is four voice streams, the first at offset 8 and the others at
/// rising offsets, each ending with the marker. The format's own check (the
/// offsets, and the marker just before each of the last three) needs the file's
/// name to be safe, which a detector here does not have, so the streams are also
/// walked note by note: each must reach its end marker before the next stream
/// starts (or the file ends), as every known file does.
fn detect(d: &[u8]) -> bool {
    if d.len() < 10 || le16(d, 0) != Some(8) {
        return false;
    }
    let start: Vec<usize> = (0..4).map(|v| le16(d, 2 * v).unwrap() as usize).collect();
    for v in 0..4 {
        let limit = if v < 3 { start[v + 1] } else { d.len() };
        if v < 3 && (start[v + 1] <= start[v] || start[v + 1] >= d.len() || d[start[v + 1] - 2..start[v + 1]] != [0xFF, 0xFF]) {
            return false;
        }
        match find_end(d, start[v]) {
            Some(e) if e + 2 <= limit => {}
            _ => return false,
        }
    }
    true
}

#[derive(Clone, Copy, Default)]
struct Voice {
    pos: usize,
    wait: u16,
    active: bool,
}

pub struct Agi {
    d: Vec<u8>,
    start: [usize; 4],
    v: [Voice; 4],
    active: u8,
    /// Chip memory of the tone wave (4 bytes) and of the noise table.
    tone: u32,
    noise: u32,
    /// The noise voice has no notes, so the tune loops.
    loops: bool,
    songend: bool,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    // the tone wave, 8 spare bytes, then the noise table: the low byte of a 16-bit
    // shift register's successive states
    let block = m.alloc(12 + NOISE_LEN as u32);
    if block == 0 {
        return None;
    }
    let mut wave = vec![0x00, 0x80, 0x00, 0x80, 0, 0, 0, 0, 0, 0, 0, 0];
    wave.extend(noise_table());
    m.bus.write_bytes(block, &wave);
    let start = [0, 1, 2, 3].map(|v| le16(d, 2 * v).unwrap() as usize);
    let mut a = Agi {
        d: d.to_vec(),
        start,
        v: Default::default(),
        active: 0,
        tone: block,
        noise: block + 12,
        loops: le16(d, start[3]) == Some(END),
        songend: false,
    };
    a.init();
    Some(Box::new(a))
}

fn noise_table() -> Vec<u8> {
    let mut r: u16 = 1;
    (0..NOISE_LEN)
        .map(|_| {
            r = if r & 1 != 0 { (r >> 1) ^ 0x0CA0 } else { r >> 1 };
            r as u8
        })
        .collect()
}

impl Agi {
    fn init(&mut self) {
        for (v, s) in self.v.iter_mut().zip(self.start) {
            *v = Voice { pos: s, wait: 1, active: true };
        }
        self.active = 4;
    }

    fn voice(&mut self, ch: usize, hw: &mut Hw) {
        let v = &mut self.v[ch];
        if !v.active {
            return;
        }
        v.wait = v.wait.wrapping_sub(1);
        if v.wait != 0 {
            return;
        }
        // a stream that is cut short counts as ended
        let note = le16(&self.d, v.pos).filter(|&dur| dur != END).and_then(|dur| {
            let n = self.d.get(v.pos + 2..v.pos + 5)?;
            Some((dur, n[0], n[1], n[2]))
        });
        let Some((dur, a, b, att)) = note else {
            v.active = false;
            self.active -= 1;
            hw.dmacon(1 << ch);
            return;
        };
        v.pos += 5;
        v.wait = dur;
        let (loc, len, per) = if ch < 3 {
            (self.tone, 2, ((a as u16 & 0x3F) * 16 + (b as u16 & 0x0F)) * 16)
        } else {
            (self.noise, (NOISE_LEN / 2) as u16, NOISE_PERIOD[b as usize & 3])
        };
        let vol = (15 - (att as u16 & 0x0F)) * 64 / 15;
        hw.loc(ch, loc);
        hw.len(ch, len);
        hw.per(ch, per);
        hw.vol(ch, vol);
        hw.dmacon(0x8000 | 1 << ch);
    }
}

impl Replayer for Agi {
    fn format(&self) -> &'static str {
        "Sierra AGI"
    }

    fn title(&self) -> String {
        String::new()
    }

    /// 60 Hz: the host's 50 Hz timer reload times 5/6, in colour clocks.
    fn tick_clocks(&self) -> u64 {
        (crate::deli::DEFAULT_TIMER as u64 * 5 / 6 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        for ch in 0..4 {
            self.voice(ch, hw);
        }
        if self.active == 0 {
            self.songend = true;
            if self.loops {
                self.init();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// Voice 0: a 2-tick note then a 1-tick one; voice 1 a 5-tick rest; voices 2
    /// and 3 are empty, so the tune loops.
    fn module() -> Vec<u8> {
        let mut d = vec![8, 0, 0, 0, 0, 0, 0, 0];
        fn put(d: &mut Vec<u8>, slot: usize, notes: &[[u8; 5]]) {
            let p = d.len() as u16;
            d[slot..slot + 2].copy_from_slice(&p.to_le_bytes());
            for n in notes {
                d.extend_from_slice(n);
            }
            d.extend_from_slice(&[0xFF, 0xFF]);
        }
        put(&mut d, 0, &[[2, 0, 0x3E, 0x80, 0x90], [1, 0, 0x31, 0x81, 0x9F]]);
        put(&mut d, 2, &[[5, 0, 0, 0x80, 0xFF]]);
        put(&mut d, 4, &[]);
        put(&mut d, 6, &[]);
        d
    }

    #[test]
    fn recognises_by_content() {
        let d = module();
        assert!(detect(&d));
        assert!(!detect(&d[..d.len() - 1]));
        let mut e = d.clone();
        e[0] = 9;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[2] = e[4];
        assert!(!detect(&e));
        assert!(!detect(&[8, 0, 8, 0]));
    }

    #[test]
    fn noise_table_starts_as_specified() {
        assert_eq!(
            noise_table()[..16],
            [0xA0, 0x50, 0x28, 0x94, 0xCA, 0x65, 0x92, 0x49, 0x84, 0xC2, 0xE1, 0x50, 0xA8, 0x54, 0xAA, 0xD5]
        );
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.agi"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 800 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, log.iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_notes_and_loops() {
        let (format, log) = play(module(), 14);
        assert_eq!(format, "Sierra AGI");
        // voice 0: divider 0x3E0 (period 0x3E00), then 0x311 (0x3110); attenuation 0 and 15
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(&per[..2], &[0x3E00, 0x3110]);
        assert_eq!(&reg(&log, 0xDFF0A8)[..2], &[64, 0]);
        assert_eq!(&reg(&log, 0xDFF0A4)[..1], &[2]);
        // the noise voice is empty: DMA off alone, no sample
        assert!(reg(&log, 0xDFF0D6).is_empty());
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x8001) && dma.contains(&0x0008));
        // the voices end together at tick 6, then the tune starts again: voice 0's
        // first note comes a second time
        assert!(per.len() >= 3 && per[2] == 0x3E00);
    }
}
