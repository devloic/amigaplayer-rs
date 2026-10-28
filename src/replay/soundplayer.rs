//! SoundPlayer (`sjs.*` with a sample companion `smp.*`), the music of the Amiga
//! game Lemmings and its add-on and sequel releases, by Scott Johnston.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's SoundPlayer and the structure
//! of the corpus files), and this player was written from it alone and checked
//! against those writes.
//!
//! A song file is a three-byte header (timer word, low byte first; voice mask) and
//! a grid of 12-byte rows of four (note, instrument, command) groups; each voice
//! walks the grid on its own. The companion is a run of IFF 8SVX `FORM` chunks
//! whose positions (in steps of four bytes) number the instrument slots. The
//! timer sets the tick; every sixth tick is a row tick. A note stops a voice's DMA
//! on the row tick and starts the sample on the next tick.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "SoundPlayer", detect, new };

/// Paula periods of the notes 1..39.
const PERIODS: [u16; 39] = [
    0x434, 0x3F8, 0x3C0, 0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C, 0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC,
    0x194, 0x17D, 0x168, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2, 0x0D6, 0x0CA, 0x0BE, 0x0B4, 0x0AA,
    0x0A0, 0x097, 0x08F, 0x087, 0x07F, 0x078, 0x071,
];

/// Recognition by content: the first row has no notes or instruments and one
/// command on every playing voice.
fn detect(b: &[u8]) -> bool {
    if b.len() < 15 {
        return false;
    }
    if b[1] == 0 || b[2] == 0 || !(0x0B..=0xA0).contains(&b[1]) {
        return false;
    }
    if b[3] != 0 || b[4] != 0 || be16(b, 6) != 0 || b[9] != 0 || b[10] != 0 || be16(b, 12) != 0 {
        return false;
    }
    let c = b[5];
    c != 0 && (b[2] == 7 || (b[2] == 15 && b[14] == c)) && b[11] == c && b[8] == c
}

/// The names the sample companion may have: `smp.NAME` for `sjs.NAME` (the prefix
/// in any case), else `smp.set`.
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    if let Some(p) = file.get(..4) {
        if p.eq_ignore_ascii_case("sjs.") {
            v.push(format!("smp.{}", &file[4..]));
        }
    }
    v.push("smp.set".to_string());
    v
}

#[derive(Clone, Copy, Default)]
struct Slot {
    start: u32,
    len: u16,
    loop_start: u32,
    loop_len: u16,
}

fn find(d: &[u8], tag: &[u8; 4], from: usize, window: usize) -> Option<usize> {
    (from..from + window).find(|&i| d.get(i..i + 4) == Some(&tag[..]))
}

/// The slots of the sample file; `base` is where its first byte lies in memory.
fn slots(d: &[u8], base: u32) -> [Slot; 39] {
    let mut s = [Slot::default(); 39];
    let (mut cur, mut slot) = (0usize, 1usize);
    loop {
        if d.get(cur..cur + 4) == Some(b"FORM") {
            if let (Some(v), Some(b)) = (find(d, b"VHDR", cur, 0x400), find(d, b"BODY", cur, 0x800)) {
                let one = be32(d, v + 8);
                let rep = be32(d, v + 12);
                let start = base.wrapping_add(b as u32 + 8);
                s[slot] = Slot {
                    start,
                    len: (one >> 1) as u16,
                    loop_start: start.wrapping_add((one as u16 as i16) as i32 as u32),
                    loop_len: (rep >> 1) as u16,
                };
            }
            cur = cur.saturating_add(8).saturating_add(be32(d, cur + 4) as usize);
        } else {
            cur += 4;
        }
        if cur >= d.len() || slot == 38 {
            break;
        }
        slot += 1;
    }
    s
}

#[derive(Clone, Copy)]
struct Voice {
    row: i32,
    hold: u8,
    loopsave: i32,
    loopcnt: u8,
    vol: u8,
    per: u16,
    pend_start: u32,
    pend_len: u16,
    loop_addr: u32,
    loop_len: u16,
    hold_loop: bool,
    env_speed: u8,
    env_cnt: u8,
    env_up: bool,
    /// The fetched group of the current row tick.
    fetched: Option<[u8; 3]>,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            row: 0,
            hold: 0,
            loopsave: 0,
            loopcnt: 0xFF,
            vol: 0,
            per: 0,
            pend_start: 0,
            pend_len: 0,
            loop_addr: 0,
            loop_len: 0,
            hold_loop: false,
            env_speed: 0,
            env_cnt: 0,
            env_up: true,
            fetched: None,
        }
    }
}

pub struct Sjs {
    d: Vec<u8>,
    title: String,
    timer: u32,
    mask: u8,
    slots: [Slot; 39],
    silence: u32,
    v: [Voice; 4],
    ticks: u32,
    songend: bool,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
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
    // a silent long, then the sample file, then zeros for loops that run past it
    let size = 4 + sample.len() as u32 + 0x20000;
    let silence = m.alloc(size);
    if silence == 0 {
        return None;
    }
    m.bus.write_bytes(silence + 4, &sample);
    let mut r = Sjs {
        d: d.to_vec(),
        title: file,
        timer: d[0] as u32 | (d[1] as u32) << 8,
        mask: d[2],
        slots: slots(&sample, silence + 4),
        silence,
        v: Default::default(),
        ticks: 0,
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    r.start(&mut hw);
    Some(Box::new(r))
}

impl Sjs {
    fn start(&mut self, hw: &mut Hw) {
        self.v = Default::default();
        self.ticks = 0;
        self.songend = false;
        for ch in 0..4 {
            hw.dmacon(1 << ch);
            hw.loc(ch, self.silence);
            hw.len(ch, 1);
        }
    }

    /// The group of row `row` (1-based) of voice `ch`; zeros past the file's end.
    fn group(&self, row: i32, ch: usize) -> [u8; 3] {
        let mut g = [0u8; 3];
        if row >= 1 {
            let o = 3 + 12 * (row as usize - 1) + 3 * ch;
            for (i, x) in g.iter_mut().enumerate() {
                *x = self.d.get(o + i).copied().unwrap_or(0);
            }
        }
        g
    }

    fn command(&mut self, ch: usize, c: u8, hw: &mut Hw) {
        let v = &mut self.v[ch];
        match c {
            0x03..=0x42 => {
                v.vol = c - 3;
                hw.vol(ch, v.vol as u16);
            }
            0x43 => hw.dmacon(1 << ch),
            0x57..=0x88 => {
                if v.hold == 0 {
                    v.hold = c - 0x56;
                }
            }
            0xA7..=0xB0 => {
                v.env_up = true;
                v.env_speed = c - 0xA6;
                v.env_cnt = v.env_speed;
            }
            0xB1..=0xBA => {
                v.env_up = false;
                v.env_speed = c - 0xB0;
                v.env_cnt = v.env_speed;
            }
            0xCF => v.hold_loop = true,
            0xD0 => v.hold_loop = false,
            0xD2..=0xDB => {
                if v.loopcnt == 0xFF {
                    v.loopsave = v.row;
                    v.loopcnt = c - 0xD1;
                }
            }
            0xDC => {
                if v.loopcnt == 0 {
                    v.loopcnt = 0xFF;
                } else {
                    v.loopcnt -= 1;
                    v.row = v.loopsave;
                }
            }
            0xDD => v.row -= 1,
            0xDE => {
                v.row = 0;
                self.songend = true;
            }
            0xDF..=0xE4 => hw.bus.write_word(0xDFF09E, [0x8001, 0x8002, 0x8004, 0x8010, 0x8020, 0x8040][(c - 0xDF) as usize]),
            0xF9..=0xFD => hw.bus.write_word(0xDFF09E, [0x0001, 0x0002, 0x0004, 0x0010, 0x0020][(c - 0xF9) as usize]),
            // filter, sync flags and the unused values change no Paula register
            _ => {}
        }
    }
}

impl Replayer for Sjs {
    fn format(&self) -> &'static str {
        "SoundPlayer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticks += 1;
        for ch in 0..4 {
            let v = &mut self.v[ch];
            if !v.hold_loop {
                if v.loop_len != 0 {
                    hw.loc(ch, v.loop_addr);
                    hw.len(ch, v.loop_len);
                } else {
                    hw.loc(ch, self.silence);
                    hw.len(ch, 1);
                }
            }
            if v.env_speed != 0 {
                v.env_cnt = v.env_cnt.wrapping_sub(1);
                if v.env_cnt == 0 {
                    v.env_cnt = v.env_speed;
                    if v.env_up {
                        if v.vol == 63 {
                            v.env_speed = 0;
                        } else {
                            v.vol += 1;
                            hw.vol(ch, v.vol as u16);
                        }
                    } else if v.vol == 0 {
                        v.env_speed = 0;
                    } else {
                        v.vol -= 1;
                        hw.vol(ch, v.vol as u16);
                    }
                }
            }
        }
        for ch in 0..4 {
            let v = &mut self.v[ch];
            if v.pend_len != 0 {
                hw.loc(ch, v.pend_start);
                hw.len(ch, v.pend_len);
                hw.per(ch, v.per);
                hw.vol(ch, v.vol as u16);
                hw.dmacon(0x8000 | 1 << ch);
                v.pend_len = 0;
            }
        }
        if self.ticks % 6 != 0 {
            return;
        }
        for ch in 0..4 {
            self.v[ch].fetched = None;
            if self.mask & (1 << ch) == 0 {
                continue;
            }
            let v = &mut self.v[ch];
            if v.hold != 0 {
                v.hold -= 1;
                if v.hold != 0 {
                    continue;
                }
            }
            v.row += 1;
            let row = v.row;
            self.v[ch].fetched = Some(self.group(row, ch));
        }
        for ch in 0..4 {
            let Some([note, ins, _]) = self.v[ch].fetched else { continue };
            if note != 0 {
                hw.dmacon(1 << ch);
                let s = if (1..=38).contains(&ins) { self.slots[ins as usize] } else { Slot::default() };
                let v = &mut self.v[ch];
                // notes above the table have no period (unexercised)
                v.per = PERIODS.get(note as usize - 1).copied().unwrap_or(0);
                v.pend_start = s.start;
                v.pend_len = s.len;
                v.loop_addr = s.loop_start;
                v.loop_len = s.loop_len;
            }
        }
        for ch in 0..4 {
            if let Some([_, _, c]) = self.v[ch].fetched {
                self.command(ch, c, hw);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// Timer word 6539 (0x198B), 3 voices; row 1 sets volume 0x1E-3, row 2 plays
    /// note 20 with instrument 1, row 3 stops the voice, row 4 restarts the song.
    fn song() -> Vec<u8> {
        let mut d = vec![0x8B, 0x19, 7];
        let mut row = |g: [u8; 3]| {
            for _ in 0..3 {
                d.extend_from_slice(&g);
            }
            d.extend_from_slice(&[0, 0, 0]);
        };
        row([0, 0, 0x1E]);
        row([20, 1, 0]);
        row([0, 0, 0x43]);
        row([0, 0, 0xDE]);
        d
    }

    /// One FORM at 0: VHDR at 12 with one-shot 16 bytes and repeat 8, BODY at 40.
    fn smp() -> Vec<u8> {
        let mut s = b"FORM".to_vec();
        s.extend_from_slice(&60u32.to_be_bytes());
        s.extend_from_slice(b"8SVXVHDR");
        s.extend_from_slice(&20u32.to_be_bytes());
        s.extend_from_slice(&16u32.to_be_bytes());
        s.extend_from_slice(&8u32.to_be_bytes());
        s.extend_from_slice(&[0; 12]);
        s.extend_from_slice(b"BODY");
        s.extend_from_slice(&24u32.to_be_bytes());
        s.extend_from_slice(&[0x40; 24]);
        s
    }

    #[test]
    fn recognises_by_content() {
        let d = song();
        assert!(detect(&d));
        let mut e = d.clone();
        e[1] = 0x0A;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[3] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[2] = 15;
        assert!(!detect(&e));
        assert!(!detect(&d[..14]));
        assert_eq!(companion("SJS.x"), ["smp.x", "smp.set"]);
    }

    #[test]
    fn slots_follow_the_scan() {
        let mut s = vec![0u8; 8];
        s.extend_from_slice(&smp());
        let sl = slots(&s, 1000);
        // the FORM is at 8: slot 3
        assert_eq!(sl[3].len, 8);
        assert_eq!(sl[3].loop_len, 4);
        assert_eq!(sl[3].start, 1000 + 8 + 40 + 8);
        assert_eq!(sl[3].loop_start, sl[3].start + 16);
        assert_eq!(sl[1].len, 0);
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note_one_tick_after_the_row() {
        let dir = Path::new("/sjs-test");
        let path = dir.join("sjs.test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("smp.test"), smp());
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&path, song()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "SoundPlayer");
        let mut out = vec![0f32; 2 * 800 * 40];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        // volume 0x1B at the first row tick, then the note's start with it
        assert_eq!(reg(&log, 0xDFF0A8)[..2], [0x1B, 0x1B]);
        assert_eq!(reg(&log, 0xDFF0A6)[0], 0x153);
        assert!(reg(&log, 0xDFF0A4).contains(&8));
        assert!(reg(&log, 0xDFF096).contains(&0x8001));
    }
}
