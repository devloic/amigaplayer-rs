//! Peter Verswyvelen Packer (`pvp.*`), the music of the games Ziriax and Zarathrusta
//! (music by Tomas Dahlgren, packer by Peter Verswyvelen, 1989/90).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Peter Verswyvelen player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A four-voice tracker format with its own engine (it is not a ProTracker module
//! in disguise): a step is four words that each pick a 16-row chunk of track data,
//! an event of four bytes lasts one or more rows, a note can repeat on every row of
//! its event, and a note start switches the channel's DMA off, waits, switches it on
//! and only then writes the sample data. 50 Hz, one tick per vertical blank, one
//! subsong. The file is a single module whose sample data follows the pattern data.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Peter Verswyvelen", detect, new };

/// Half a sine, used unsigned, for the vibrato.
const SINE: [u8; 32] = [
    0x00, 0x18, 0x31, 0x4A, 0x61, 0x78, 0x8D, 0xA1, 0xB4, 0xC5, 0xD4, 0xE0, 0xEB, 0xF4, 0xFA, 0xFD, 0xFF, 0xFD, 0xFA,
    0xF4, 0xEB, 0xE0, 0xD4, 0xC5, 0xB4, 0xA1, 0x8D, 0x78, 0x61, 0x4A, 0x31, 0x18,
];

/// The periods the arpeggio steps through (36 notes and the last repeated); the
/// reads past it, which the original makes into whatever follows its table, are 0.
const PERIODS: [u16; 36] = [
    0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C, 0x23A, 0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC, 0x194, 0x17D, 0x168,
    0x153, 0x140, 0x12E, 0x11D, 0x10D, 0xFE, 0xF0, 0xE2, 0xD6, 0xCA, 0xBE, 0xB4, 0xAA, 0xA0, 0x97, 0x8F, 0x87, 0x7F,
    0x78, 0x71,
];

/// Colour clocks the original lets pass between its DMA-off, DMA-on and data passes
/// (nine raster lines).
const DMA_WAIT: u64 = 2000;

// ---------------------------------------------------------------------------------
// Recognition

/// The layout of a module: the sizes of its parts, or `None` if it is not one.
struct Layout {
    /// Words of the pointer table.
    n: usize,
    /// Order-table entries (8 bytes each).
    s: usize,
    restart: usize,
    /// Start of the pattern data.
    pat: usize,
    /// Start of the sample data.
    smp: usize,
}

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 0x102 {
        return None;
    }
    for i in 0..31 {
        let o = 8 * i;
        if be16(d, o) >= 0x8000 || be16(d, o + 2) > 0x40 || be16(d, o + 4) >= 0x8000 || be16(d, o + 6) >= 0x8000 {
            return None;
        }
    }
    let n = be16(d, 0xF8) as usize;
    let s = be16(d, 0xFA) as usize;
    let restart = be16(d, 0xFC) as usize;
    let p = be16(d, 0xFE) as usize;
    if n < 2 || n >= 0x8000 || s == 0 || s >= 0x8000 || s % 2 != 0 || restart >= s || p == 0 || p >= 0x8000 {
        return None;
    }
    if 0x100 + 2 * n > d.len() {
        return None;
    }
    for k in 0..n - 1 {
        let e = be16(d, 0x100 + 2 * k);
        if e >= 0x8000 || e % 2 != 0 || e > be16(d, 0x100 + 2 * k + 2) || e as usize > p {
            return None;
        }
    }
    let pat = 0x100 + 2 * n + 8 * s;
    let smp = pat + p;
    let end = smp + (0..31).map(|i| 2 * be16(d, 8 * i) as usize).sum::<usize>();
    if end > d.len() {
        return None;
    }
    Some(Layout { n, s, restart, pat, smp })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

// ---------------------------------------------------------------------------------
// The player

#[derive(Clone, Default)]
struct Chan {
    tp: usize,
    d: i8,
    rep: bool,
    p6: u16,
    e: u8,
    b3: u8,
    x: u8,
    y: u8,
    a12: u32,
    l16: u16,
    v18: u16,
    v1a: u16,
    a1c: u32,
    l20: u16,
    p22: u16,
    p24: u16,
    target: u16,
    down: bool,
    s29: u8,
    vib: u8,
    vp: u8,
}

struct Pvp {
    d: Vec<u8>,
    base: u32,
    lay: Layout,
    /// Start (offset in the file) of each sample.
    starts: [usize; 31],
    title: String,
    speed: u8,
    c: i8,
    pos: u32,
    rc: i16,
    m: u8,
    ch: [Chan; 4],
    songend: bool,
}

impl Pvp {
    fn new(d: &[u8], base: u32, title: String, hw: Option<&mut Hw>) -> Option<Pvp> {
        let lay = layout(d)?;
        let mut starts = [0usize; 31];
        let mut a = lay.smp;
        for (i, s) in starts.iter_mut().enumerate() {
            *s = a;
            a += 2 * be16(d, 8 * i) as usize;
        }
        // the first long of every sample is cleared, as the original does
        if let Some(hw) = hw {
            for s in starts {
                if s + 4 <= d.len() {
                    hw.bus.w32(base + s as u32, 0);
                }
            }
        }
        Some(Pvp { d: d.to_vec(), base, lay, starts, title, speed: 6, c: 0, pos: 0, rc: 0, m: 0, ch: Default::default(), songend: false })
    }

    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn s8(&self) -> u32 {
        8 * self.lay.s as u32
    }

    /// Loads the step at `pos`; `pos` then names the next one.
    fn load_step(&mut self) {
        let old = self.pos as usize;
        let mut next = self.pos + 8;
        if next >= self.s8() {
            next = 8 * self.lay.restart as u32;
            self.songend = true;
        }
        self.pos = next;
        self.rc = 16;
        for i in 0..4 {
            let o = be16(&self.d, 0x100 + 2 * self.lay.n + old + 2 * i) as usize;
            self.ch[i].tp = self.lay.pat + be16(&self.d, 0x100 + o) as usize;
        }
    }

    fn trigger(&mut self, i: usize, p: u16, hw: &mut Hw) {
        let bit = 1u8 << i;
        self.ch[i].p22 = p;
        self.ch[i].vp = 0;
        hw.vol(i, 0);
        hw.per(i, 1);
        hw.dmacon(bit as u16);
        self.m |= bit;
        hw.loc(i, self.ch[i].a12);
        hw.len(i, self.ch[i].l16);
        self.post_trigger(i, hw);
    }

    fn post_trigger(&mut self, i: usize, hw: &mut Hw) {
        let c = &mut self.ch[i];
        c.v1a = c.v18;
        c.p24 = c.p22;
        let b3 = c.b3;
        match c.e {
            11 => {
                self.pos = ((b3 as u32).wrapping_sub(1) << 5) & 0xFFFF;
                self.rc = 0;
            }
            12 => {
                c.v18 = (b3 as u16).min(64);
                hw.vol(i, c.v18 & 0x7F);
            }
            13 => {
                let mut n = (self.pos & 0xFFE0) + 0x20;
                if n >= self.s8() {
                    n = 8 * self.lay.restart as u32;
                    self.songend = true;
                }
                self.pos = n;
                self.rc = 0;
            }
            15 => {
                self.speed = b3;
                self.c = b3 as i8;
            }
            _ => {}
        }
    }

    fn row(&mut self, i: usize, hw: &mut Hw) {
        let bit = 1u8 << i;
        self.ch[i].d = self.ch[i].d.wrapping_sub(1);
        if self.ch[i].d >= 0 {
            let c = &mut self.ch[i];
            if !c.rep {
                c.p6 = 0;
                c.e = 0;
                c.b3 = 0;
            } else if c.p6 != 0 {
                c.v18 = c.v1a;
                c.p22 = c.p24;
                if c.e != 3 {
                    let p6 = c.p6;
                    self.trigger(i, p6, hw);
                }
            }
            return;
        }
        // a new event
        let o = self.ch[i].tp;
        let (b0, b1, b2, b3) = (self.rb(o), self.rb(o + 1), self.rb(o + 2), self.rb(o + 3));
        self.ch[i].tp = o + 4;
        let sample = (b0 >> 3) as usize;
        let period = (((b0 as u16) << 8) | b1 as u16) & 0x3FF;
        if sample != 0 {
            let h = 8 * (sample - 1);
            let (len, vol, ls, ll) = (be16(&self.d, h), be16(&self.d, h + 2), be16(&self.d, h + 4), be16(&self.d, h + 6));
            let a12 = self.base + self.starts[sample - 1] as u32;
            let c = &mut self.ch[i];
            c.a12 = a12;
            c.l16 = len;
            c.v18 = vol;
            if ls != 0 {
                c.l20 = ll;
                c.l16 = ls.wrapping_add(ll);
                c.a1c = a12 + 2 * ls as u32;
            } else {
                c.a1c = a12;
                c.l20 = ll;
            }
        }
        let c = &mut self.ch[i];
        c.rep = b0 & 4 != 0;
        c.e = b2 & 15;
        c.d = (b2 >> 4) as i8;
        c.b3 = b3;
        c.x = b3 >> 4;
        c.y = b3 & 15;
        c.p6 = period;
        let e = c.e;
        if period == 0 {
            self.post_trigger(i, hw);
        } else if e != 3 {
            self.trigger(i, period, hw);
        } else {
            c.target = period;
            c.down = (period as i16) < (c.p22 as i16);
            c.s29 = b3;
            self.m |= bit;
        }
    }

    fn effects(&mut self, i: usize, hw: &mut Hw) {
        let (speed, cc) = (self.speed, self.c);
        let c = &mut self.ch[i];
        match c.e {
            0 => {
                if c.b3 != 0 {
                    let k = speed.wrapping_sub(cc as u8) as i8;
                    let v = if k == 0 || k == 3 {
                        c.p22
                    } else {
                        let n = if k == 2 || k == 5 { c.y } else { c.x } as usize;
                        let idx = PERIODS.iter().position(|&t| c.p22 >= t).unwrap_or(36);
                        PERIODS.get(idx + n).copied().unwrap_or(0)
                    };
                    hw.per(i, v);
                }
            }
            1 => {
                c.p22 = (c.p22 as i32 - c.b3 as i32).max(0x71) as u16;
                hw.per(i, c.p22);
            }
            2 => {
                c.p22 = (c.p22 as i32 + c.b3 as i32).min(0x358) as u16;
                hw.per(i, c.p22);
            }
            3 => {
                if c.b3 != 0 {
                    c.s29 = c.b3;
                }
                if c.p22 != c.target {
                    let (p, t, s) = (c.p22 as i16, c.target as i16, c.s29 as i16);
                    let v = if c.down {
                        let n = p.wrapping_sub(s);
                        if t >= n {
                            t
                        } else {
                            n
                        }
                    } else {
                        let n = p.wrapping_add(s);
                        if t <= n {
                            t
                        } else {
                            n
                        }
                    };
                    c.p22 = v as u16;
                    hw.per(i, c.p22);
                }
            }
            4 => {
                if c.b3 != 0 {
                    c.vib = c.b3;
                }
                let idx = ((c.vp & 0x7F) >> 2) as usize;
                let delta = (SINE[idx] as u16 * (c.vib & 15) as u16) >> 6;
                hw.per(i, c.p22.wrapping_add(delta));
                c.vp = c.vp.wrapping_add((c.vib >> 2) & 0x3C);
            }
            10 => {
                c.v18 = if c.x != 0 { (c.v18 + c.x as u16).min(64) } else { c.v18.saturating_sub(c.y as u16) };
                hw.vol(i, c.v18 & 0x7F);
            }
            _ => {}
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.c = self.c.wrapping_sub(1);
        if self.c > 0 {
            for i in 0..4 {
                self.effects(i, hw);
            }
            return;
        }
        self.c = self.speed as i8;
        self.m = 0;
        self.rc = self.rc.wrapping_sub(1);
        if self.rc <= 0 {
            self.load_step();
        }
        for i in 0..4 {
            self.row(i, hw);
        }
        hw.wait(DMA_WAIT);
        hw.dmacon(0x8200 | self.m as u16);
        hw.wait(DMA_WAIT);
        for i in 0..4 {
            if self.m & (1 << i) != 0 {
                let c = &self.ch[i];
                hw.loc(i, c.a1c);
                hw.len(i, c.l20);
                hw.per(i, c.p22);
                hw.vol(i, c.v18 & 0x7F);
            }
        }
    }
}

impl Replayer for Pvp {
    fn format(&self) -> &'static str {
        "Peter Verswyvelen"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        Pvp::tick(self, hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let title = m.bus.read_cstr(m.deli.file_buf);
    let mut hw = Hw { bus: &mut m.bus };
    Some(Box::new(Pvp::new(d, base, title, Some(&mut hw))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    /// One sample of 8 words, a pointer table of two chunks, a single 4-step group
    /// (the order table holds `s = 4` entries) whose first channel plays one note.
    fn module() -> Vec<u8> {
        let n = 2usize;
        let s = 4usize;
        let pat = 0x100 + 2 * n + 8 * s;
        let mut d = vec![0u8; pat + 0x20 + 16];
        put16(&mut d, 0, 8);
        put16(&mut d, 2, 0x30);
        put16(&mut d, 4, 0);
        put16(&mut d, 6, 1);
        put16(&mut d, 0xF8, n as u16);
        put16(&mut d, 0xFA, s as u16);
        put16(&mut d, 0xFC, 0);
        put16(&mut d, 0xFE, 0x20);
        put16(&mut d, 0x100, 0);
        put16(&mut d, 0x102, 0x10);
        // order: channel 0 -> chunk 0, the others chunk 1 (empty)
        let ord = 0x100 + 2 * n;
        for st in 0..4 {
            put16(&mut d, ord + 8 * st, 0);
            for ch in 1..4 {
                put16(&mut d, ord + 8 * st + 2 * ch, 2);
            }
        }
        // chunk 0: sample 1, period 0x1AC, lasting 16 rows
        d[pat] = 1 << 3 | 0x01;
        d[pat + 1] = 0xAC;
        d[pat + 2] = 0xF0;
        // chunk 1: no note, 16 rows
        d[pat + 0x10 + 2] = 0xF0;
        d
    }

    #[test]
    fn recognises_modules() {
        let d = module();
        assert!(detect(&d));
        let mut e = d.clone();
        put16(&mut e, 0xFA, 3);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 0xF8, 1);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 2, 0x41);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 0x100, 0x30);
        assert!(!detect(&e));
        assert!(!detect(&d[..d.len() - 2]));
        assert!(!detect(&[0u8; 0x400]));
    }

    fn run(data: Vec<u8>, ticks: usize) -> (Vec<(u32, u16)>, bool) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/pvp-test/pvp.test"), data).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Peter Verswyvelen");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let end = e.songend();
        (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), end)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn a_note_starts_with_dma_off_then_on_then_the_loop() {
        let (log, _) = run(module(), 4);
        // retrigger: volume 0 and period 1, DMA off, then the sample start and
        // length; after the DMA-on word the loop, period and volume
        assert_eq!(reg(&log, 0xDFF0A8)[..2], [0, 0x30]);
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [1, 0x1AC]);
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [8, 1]);
        let dma = reg(&log, 0xDFF096);
        let i = dma.iter().position(|&v| v == 0x0001).unwrap();
        assert_eq!(dma[i + 1], 0x8201);
    }

    #[test]
    fn the_dma_word_is_written_on_every_row_tick() {
        let (log, _) = run(module(), 14);
        let on: Vec<u16> = reg(&log, 0xDFF096).into_iter().filter(|v| v & 0x8200 == 0x8200).collect();
        // rows at ticks 0 and 6 (speed 6): the second has no note
        assert!(on.len() >= 3 && on[0] == 0x8201 && on[1] == 0x8200);
    }

    #[test]
    fn song_end_is_announced_when_the_order_wraps() {
        let (_, end) = run(module(), 20);
        assert!(!end);
        let (_, end) = run(module(), 4 * 16 * 6 + 10);
        assert!(end);
    }
}
