//! Pierre Adane Packer (`pap.*` with a sample companion `smp.*` or `smp.set`), the
//! music of Pang and Ivanhoe (Ocean, 1990).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Pierre Adane Packer player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! The song is four position lists (one per voice) of offsets of run-length packed
//! note data, a table of fifteen or so sample records, and the samples themselves in
//! a companion file. A row lasts `speed` ticks (3 until an effect changes it), 64
//! rows make a position, and at a position's end every voice jumps to the pattern
//! its list names next. A looped sample is started at its loop point. Playback is
//! 50 Hz, Paula DMA, four voices.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Pierre Adane Packer", detect, new };

/// The notes the arpeggio steps through (three octaves).
const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
    226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// The most sample records read from the table.
const MAX_SAMPLES: usize = 64;

/// Recognition by content: four ascending, even list offsets whose spacing is the
/// same for the last three lists and two bytes more for the first, lists of even
/// offsets no greater than the first list's own, and a first pattern that starts
/// with an `FF` prefix byte.
fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// The list offsets and the length of the shorter lists, if `d` is a song.
fn layout(d: &[u8]) -> Option<([usize; 4], usize)> {
    if d.len() < 8 {
        return None;
    }
    let w: [usize; 4] = std::array::from_fn(|i| be16(d, 2 * i) as usize);
    if w.iter().any(|&x| x == 0 || x & 1 != 0 || x & 0x8000 != 0) {
        return None;
    }
    // each list holds at least one entry (an empty one is not a song)
    if w[3] <= w[2] || w[2] <= w[1] || w[1] <= w[0] {
        return None;
    }
    let l = w[3] - w[2];
    if w[2] - w[1] != l || w[1] - w[0] != l + 2 {
        return None;
    }
    if d.len() < w[3] + l {
        return None;
    }
    let x = be16(d, w[0]) as usize;
    if d.get(x) != Some(&0xFF) {
        return None;
    }
    for o in (w[0]..w[3] + l).step_by(2) {
        let v = be16(d, o) as usize;
        if v & 0x8000 != 0 || v & 1 != 0 || v > w[0] {
            return None;
        }
    }
    Some((w, l))
}

/// The names the sample companion has: `smp.NAME` for `pap.NAME` (the prefix in any
/// case), then `smp.set`.
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    if file.len() > 3 && file.is_char_boundary(3) {
        v.push(format!("smp{}", &file[3..]));
    }
    v.push("smp.set".to_string());
    v
}

#[derive(Clone, Copy, Default)]
struct Sample {
    /// The offset of the body in the sample file.
    base: u32,
    len: u16,
    vol: u16,
    loop_off: u16,
    loop_len: u16,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    ptr: usize,
    cnt: i16,
    rep: i16,
    period: u16,
    byte2: u8,
    byte3: u8,
    cur: u16,
    start: u32,
    len: u16,
    rl: u16,
}

pub struct Pap {
    d: Vec<u8>,
    w: [usize; 4],
    samples: Vec<Sample>,
    /// Where the sample file is in guest memory.
    base: u32,
    title: String,
    v: [Voice; 4],
    counter: u16,
    speed: u16,
    pos: usize,
    row: u8,
    dmacon: u16,
    songend: bool,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (w, _) = layout(d)?;
    let mut d = d.to_vec();
    // the one known-bad file is repaired by the size of the song
    if 2 * w[3] - w[2] == 2604 && be16(&d, 16) == 0x06FC {
        d[16..18].copy_from_slice(&0x06AEu16.to_be_bytes());
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut sample = None;
    for name in companion(&file) {
        if let Ok(s) = crate::deli::read_companion(m, &name).1 {
            sample = Some(s);
            break;
        }
    }
    let sample = sample?;
    let base = m.alloc((sample.len() as u32).max(4));
    if base == 0 {
        return None;
    }
    m.bus.write_bytes(base, &sample);
    // the sample table: eight bytes a record, up to a record starting with FF
    let mut samples = Vec::new();
    let mut at = 0u32;
    for i in 0..MAX_SAMPLES {
        let r = 8 + 8 * i;
        if d.get(r).is_none_or(|&b| b == 0xFF) {
            break;
        }
        let s = Sample {
            base: at,
            len: be16(&d, r),
            vol: be16(&d, r + 2),
            loop_off: be16(&d, r + 4),
            loop_len: be16(&d, r + 6),
        };
        at = at.wrapping_add(2 * s.len as u32);
        samples.push(s);
    }
    let mut p = Pap {
        d,
        w,
        samples,
        base,
        title: file,
        v: Default::default(),
        counter: 0,
        speed: 3,
        pos: 2,
        row: 0,
        dmacon: 0,
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    p.start(&mut hw);
    Some(Box::new(p))
}

impl Pap {
    fn start(&mut self, hw: &mut Hw) {
        for ch in 0..4 {
            hw.vol(ch, 0);
        }
        hw.dmacon(0x000F);
        self.songend = false;
        self.counter = 0;
        self.speed = 3;
        self.pos = 2;
        self.row = 0;
        self.dmacon = 0;
        self.v = Default::default();
        for ch in 0..4 {
            self.v[ch].ptr = be16(&self.d, self.w[ch]) as usize;
        }
    }

    /// Takes the next entry of the voice's note data (a row tick).
    fn fetch(&mut self, ch: usize) {
        let v = &mut self.v[ch];
        v.cnt = v.cnt.wrapping_sub(1);
        if v.cnt >= 0 {
            v.period = 0;
            v.byte2 = 0;
            v.byte3 = 0;
            return;
        }
        let mut p = v.ptr;
        if self.d.get(p) == Some(&0xFF) {
            v.rep = (self.d.get(p + 1).copied().unwrap_or(0) as i16).wrapping_sub(1);
            p += 2;
        }
        v.period = be16(&self.d, p);
        v.byte2 = self.d.get(p + 2).copied().unwrap_or(0);
        v.byte3 = self.d.get(p + 3).copied().unwrap_or(0);
        v.ptr = p + 4;
        v.cnt = v.rep;
    }

    fn row_tick(&mut self, hw: &mut Hw) {
        self.counter = 0;
        self.dmacon = 0x8000;
        for ch in 0..4 {
            self.fetch(ch);
            let (period, byte2, byte3) = (self.v[ch].period, self.v[ch].byte2, self.v[ch].byte3);
            let n = (byte2 >> 4) as usize;
            // a sample number past the table is ignored
            if n != 0 && n <= self.samples.len() {
                let s = self.samples[n - 1];
                let v = &mut self.v[ch];
                v.start = s.base;
                v.len = s.len;
                let vol = if byte2 & 15 == 12 { byte3 as u16 } else { s.vol };
                hw.vol(ch, vol & 0x7F);
                if s.loop_len != 1 {
                    v.start = s.base.wrapping_add(s.loop_off as u32);
                    v.len = s.loop_len;
                }
                v.rl = s.loop_len;
            }
            if period != 0 {
                let v = &mut self.v[ch];
                if v.rl == 0 {
                    v.rl = 1;
                }
                v.cur = period;
                hw.dmacon(1 << ch);
                hw.loc(ch, self.base.wrapping_add(v.start));
                hw.len(ch, v.len);
                hw.per(ch, period);
                self.dmacon |= 1 << ch;
            } else if byte2 != 0 || byte3 != 0 {
                self.effect(ch, hw);
            }
        }
        self.row = (self.row + 1) & 63;
        if self.row == 0 {
            self.set_data();
        }
    }

    /// Every 64 rows: the next pattern of every voice, or the song's end.
    fn set_data(&mut self) {
        if be16(&self.d, self.w[0] + self.pos) == 0 {
            self.songend = true;
            self.pos = 0;
        }
        for ch in 0..4 {
            self.v[ch].ptr = be16(&self.d, self.w[ch] + self.pos) as usize;
        }
        self.pos += 2;
    }

    fn effect(&mut self, ch: usize, hw: &mut Hw) {
        let e = self.v[ch].byte2 & 15;
        let p = self.v[ch].byte3;
        match e {
            0 => {
                let nib = match self.counter {
                    0 | 2 | 5 => p & 15,
                    3 | 6 => 0,
                    _ => p >> 4,
                } as usize;
                let cur = self.v[ch].cur;
                // a base period outside the table is written as it is; a step
                // past the table's end stays on its last note
                let per = match PERIODS.iter().position(|&x| x == cur) {
                    Some(i) => PERIODS[(i + nib).min(35)],
                    None => cur,
                };
                hw.per(ch, per);
            }
            1 => {
                let v = &mut self.v[ch];
                v.cur = (v.cur as i32 - (p & 15) as i32).max(0x71) as u16;
                hw.per(ch, v.cur);
            }
            2 => {
                let v = &mut self.v[ch];
                v.cur = (v.cur as i32 + (p & 15) as i32).min(0x358) as u16;
                hw.per(ch, v.cur);
            }
            12 => hw.vol(ch, (p & 0x7F) as u16),
            15 => {
                if p & 15 != 0 {
                    self.speed = (p & 15) as u16;
                }
            }
            _ => {}
        }
    }

    fn mid_tick(&mut self, hw: &mut Hw) {
        for ch in 0..4 {
            if self.v[ch].byte3 != 0 {
                self.effect(ch, hw);
            }
        }
    }
}

impl Replayer for Pap {
    fn format(&self) -> &'static str {
        "Pierre Adane Packer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn set_subsong(&mut self, _n: u16, hw: &mut Hw) {
        self.start(hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.counter = self.counter.wrapping_add(1);
        if self.counter == self.speed {
            self.row_tick(hw);
        } else {
            self.mid_tick(hw);
        }
        // the second half of a tick: the DMA settles first
        hw.wait(2000);
        hw.dmacon(self.dmacon);
        for ch in 0..4 {
            if self.v[ch].rl == 1 {
                self.v[ch].rl = 0;
                hw.len(ch, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A song of three positions. Voice 0 plays sample 1 (looped from byte 4, 2
    /// words) at period 428 and then a slide down; voice 1 plays sample 2
    /// (unlooped) at period 339 with the volume command C20; voices 2 and 3 rest.
    /// The first list ends with the 0 that ends the song.
    fn song() -> Vec<u8> {
        let mut d = vec![0u8; 0x100];
        let (w0, w1, w2, w3) = (0x80usize, 0x86, 0x8A, 0x8E);
        for (i, w) in [w0, w1, w2, w3].iter().enumerate() {
            d[2 * i..2 * i + 2].copy_from_slice(&(*w as u16).to_be_bytes());
        }
        // sample table at 8: sample 1 (8 words, volume 40, loop at byte 4, 2 words),
        // sample 2 (4 words, volume 30, no loop), terminator
        let t = [0, 8, 0, 40, 0, 4, 0, 2, 0, 4, 0, 30, 0, 0, 0, 1, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0];
        d[8..8 + t.len()].copy_from_slice(&t);
        let p0 = [0xFF, 2, 0x01, 0xAC, 0x10, 0x00, 0x00, 0x00, 0x02, 0x02];
        d[0x30..0x30 + p0.len()].copy_from_slice(&p0);
        let p1 = [0xFF, 4, 0x01, 0x53, 0x2C, 0x20];
        d[0x40..0x40 + p1.len()].copy_from_slice(&p1);
        d[0x50..0x56].copy_from_slice(&[0xFF, 4, 0, 0, 0, 0]);
        let put = |d: &mut Vec<u8>, o: usize, v: u16| d[o..o + 2].copy_from_slice(&v.to_be_bytes());
        for (w, p) in [(w0, 0x30), (w1, 0x40), (w2, 0x50), (w3, 0x50)] {
            put(&mut d, w, p);
            put(&mut d, w + 2, p);
        }
        put(&mut d, w0 + 4, 0);
        d.truncate(w3 + 4);
        d
    }

    #[test]
    fn recognises_by_content() {
        let d = song();
        assert!(detect(&d));
        for (i, v) in [(1usize, 0x81u8), (0, 0x80), (3, 0x85)] {
            let mut e = d.clone();
            e[i] = v;
            assert!(!detect(&e), "{i}");
        }
        // an odd pattern offset in a list
        let mut e = d.clone();
        e[0x80 + 1] = 0x31;
        assert!(!detect(&e));
        // the first pattern starts with a prefix byte
        let mut e = d.clone();
        e[0x30] = 0;
        assert!(!detect(&e));
        assert!(!detect(&d[..d.len() - 1]));
        assert!(!detect(&[0; 4]));
        assert_eq!(companion("PAP.level"), ["smp.level", "smp.set"]);
        assert_eq!(companion("pap.ingame_01"), ["smp.ingame_01", "smp.set"]);
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_row_every_third_tick() {
        let dir = Path::new("/pap-test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("smp.set"), vec![0x40; 64]);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&dir.join("pap.test"), song()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Pierre Adane Packer");
        let mut out = vec![0f32; 2 * 960 * 12];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        // voice 0 starts at the loop point of sample 1: 2 words, no length reset
        assert_eq!(reg(&log, 0xDFF0A4), [2]);
        assert_eq!(reg(&log, 0xDFF0A8)[1], 40);
        // voice 1: unlooped sample 2, volume from command C, length reset after DMA on
        assert_eq!(reg(&log, 0xDFF0B4), [4, 1]);
        assert_eq!(reg(&log, 0xDFF0B8)[1], 0x20);
        assert_eq!(reg(&log, 0xDFF0B6)[0], 339);
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x8003));
        // the slide down by 2 on the third row and its mid ticks
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per[..4], [428, 430, 432, 434]);
    }

    #[test]
    fn needs_its_samples() {
        let dir = Path::new("/pap-test");
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        let r = e.play_data(&dir.join("pap.test"), song());
        assert!(r.is_err() || e.playing.as_ref().map_or(true, |p| p.format != "Pierre Adane Packer"));
    }
}
