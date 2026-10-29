//! A.M.Composer 1.2 (`amc.*`), a four-voice tracker whose modules start with the tags
//! `AMC V1.2 REPLAY!`.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no original
//! code. The specification was drawn up from the Paula register writes of UADE's player
//! and the module structure of the corpus files, and this player was written from it alone
//! and checked against those writes.
//!
//! Decisions where the specification leaves the choice to the port:
//! * The host's volume/balance factors are 64 (the reference's), so a record volume reaches
//!   Paula unchanged and no voice is masked.
//! * The four volume-0 writes of the host's start-up hook are made at the first tick,
//!   before the tick's own writes (channel order 1..4).
//! * A timer byte of 0 is treated as 1 (an interval of one E-clock is not playable).
//! * The song end is reported in the tick of the wrap and stays reported; the song goes on
//!   (as in the reference's `-n` runs) if the host does not stop.
//! * A record past the end of the module never matches a row; reads outside the module are 0.
//! * Sample index 0 addresses the entry 65520 bytes past the table, as the word arithmetic
//!   of the original does; it reads as zeros unless the module is that long.
//! * An unknown command code skips three bytes (row and code), as the specification derives.
//! * A slide with a duration of 0 does nothing; a count of 0 wraps the byte countdown to 255.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "A.M.Composer", detect, new };

/// Colour clocks from the tick's first write to the effect writes (the raster wait).
const T_RASTER: u64 = 1950;
const T_WRITE: u64 = 24;
const T_VOICE: u64 = 60;

fn detect(d: &[u8]) -> bool {
    d.len() >= 72 + 16 && &d[0..16] == b"AMC V1.2 REPLAY!" && be16(d, 24) == 0
}

/// One register write of the replayer, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum W {
    Dma(u16),
    Loc(usize, u32),
    Len(usize, u16),
    Per(usize, u16),
    Vol(usize, u16),
    Wait(u64),
}

#[derive(Clone, Copy, Default)]
struct Slide {
    dur: u16,
    cnt: u8,
    reload: u8,
    step: u16,
    acc: u16,
}

#[derive(Clone, Copy, Default)]
struct Table {
    cnt: u8,
    idx: u8,
    v: [u16; 8],
}

struct Amc {
    d: Vec<u8>,
    base: u32,
    rows: u16,
    timer: u8,
    start: [usize; 8],
    ptr: [usize; 8],
    row: u16,
    sub: u8,
    pend: u8,
    loopp: [usize; 4],
    slide: [Slide; 4],
    table: [Table; 4],
    started: bool,
    songend: bool,
}

impl Amc {
    fn new(d: &[u8], base: u32) -> Amc {
        let mut start = [0usize; 8];
        for (i, s) in start.iter_mut().enumerate() {
            *s = be32(d, 24 + 4 * i) as usize;
        }
        Amc {
            d: d.to_vec(),
            base,
            rows: be16(d, 16),
            timer: d.get(57).copied().unwrap_or(0).max(1),
            start,
            ptr: start,
            row: 0,
            sub: 0,
            pend: 0,
            loopp: [0; 4],
            slide: [Slide::default(); 4],
            table: [Table::default(); 4],
            started: false,
            songend: false,
        }
    }

    fn b(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// Whether the record at stream `s` is for `row` (and inside the module).
    fn due(&self, s: usize, row: u16) -> bool {
        let p = self.ptr[s];
        p + 2 <= self.d.len() && be16(&self.d, p) == row
    }

    /// One interrupt call: the writes in order.
    fn tick(&mut self, out: &mut Vec<W>) {
        if !self.started {
            self.started = true;
            // the host's volume hook at start-up
            for ch in 0..4 {
                out.push(W::Vol(ch, 0));
            }
            out.push(W::Wait(T_VOICE));
        }
        out.push(W::Dma(0x8000 | self.pend as u16));
        out.push(W::Wait(T_RASTER));
        for v in 0..4 {
            let s = &mut self.slide[v];
            if s.dur != 0 {
                s.dur -= 1;
                if s.dur != 0 {
                    s.cnt = s.cnt.wrapping_sub(1);
                    if s.cnt == 0 {
                        s.cnt = s.reload;
                        s.acc = s.acc.wrapping_sub(s.step);
                        out.push(W::Per(v, s.acc));
                        out.push(W::Wait(T_WRITE));
                    }
                }
            }
            let t = &mut self.table[v];
            if t.cnt != 0 {
                t.cnt -= 1;
                out.push(W::Per(v, t.v[(t.idx >> 1) as usize & 7]));
                out.push(W::Wait(T_WRITE));
                t.idx = (t.idx + 2) & 15;
            }
        }
        for ch in 0..4 {
            if self.pend & 1 << ch != 0 {
                let lp = self.loopp[ch];
                out.push(W::Loc(ch, self.base.wrapping_add(be32(&self.d, lp))));
                out.push(W::Len(ch, be16(&self.d, lp + 4)));
                out.push(W::Wait(T_WRITE));
            }
        }
        self.pend = 0;
        self.sub += 1;
        if self.sub >= 4 {
            self.sub = 0;
            self.row_tick(out);
        }
    }

    fn row_tick(&mut self, out: &mut Vec<W>) {
        self.row = self.row.wrapping_add(1);
        if self.row == self.rows {
            self.songend = true;
            self.row = 0;
            self.ptr = self.start;
        }
        let row = self.row;
        for v in 0..4 {
            if !self.due(v, row) {
                continue;
            }
            let p = self.ptr[v];
            self.ptr[v] += 6;
            let idx = self.b(p + 2) as u32;
            let vol = self.b(p + 3) as u16;
            let per = be16(&self.d, p + 4);
            out.push(W::Dma(1 << v));
            self.table[v].cnt = 0;
            self.slide[v].dur = 0;
            self.pend |= 1 << v;
            // the entry of sample `idx` (1-based; the word arithmetic wraps for 0)
            let e = 72 + (idx.wrapping_sub(1).wrapping_mul(16) & 0xFFFF) as usize;
            out.push(W::Loc(v, self.base.wrapping_add(be32(&self.d, e))));
            out.push(W::Len(v, be16(&self.d, e + 4)));
            self.loopp[v] = e + 6;
            out.push(W::Vol(v, vol));
            out.push(W::Per(v, per));
            out.push(W::Wait(T_VOICE));
        }
        for v in 0..4 {
            if !self.due(4 + v, row) {
                continue;
            }
            let p = self.ptr[4 + v];
            match self.b(p + 2) {
                1 => {
                    out.push(W::Per(v, be16(&self.d, p + 4)));
                    self.ptr[4 + v] += 6;
                }
                2 => {
                    let x = self.b(p + 3);
                    self.slide[v] = Slide {
                        cnt: x,
                        reload: x,
                        step: be16(&self.d, p + 4),
                        dur: be16(&self.d, p + 6),
                        acc: be16(&self.d, p + 8),
                    };
                    self.table[v].cnt = 0;
                    self.ptr[4 + v] += 10;
                }
                3 => {
                    out.push(W::Vol(v, be16(&self.d, p + 4)));
                    self.ptr[4 + v] += 6;
                }
                4 => {
                    let mut t = Table { cnt: self.b(p + 3), idx: 2, v: [0; 8] };
                    for (i, x) in t.v.iter_mut().enumerate() {
                        *x = be16(&self.d, p + 4 + 2 * i);
                    }
                    self.table[v] = t;
                    self.slide[v].dur = 0;
                    self.ptr[4 + v] += 20;
                }
                7 => self.ptr[4 + v] += 4,
                _ => self.ptr[4 + v] += 3,
            }
            out.push(W::Wait(T_VOICE));
        }
    }
}

impl Replayer for Amc {
    fn format(&self) -> &'static str {
        "A.M.Composer"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        5 * (256 * self.timer as u64 + 1)
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        let mut w = Vec::new();
        Amc::tick(self, &mut w);
        for x in w {
            match x {
                W::Dma(v) => hw.dmacon(v),
                W::Loc(c, a) => hw.loc(c, a),
                W::Len(c, n) => hw.len(c, n),
                W::Per(c, p) => hw.per(c, p),
                W::Vol(c, v) => hw.vol(c, v),
                W::Wait(n) => hw.wait(n),
            }
        }
    }
}

fn new(d: &[u8], base: u32, _m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    Some(Box::new(Amc::new(d, base)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(d: &mut Vec<u8>, v: u16) {
        d.extend_from_slice(&v.to_be_bytes());
    }
    fn put32(d: &mut Vec<u8>, v: u32) {
        d.extend_from_slice(&v.to_be_bytes());
    }

    /// One sample; voice 1 has a note at row 1 (sample 1, volume 40, period 400) and `cmd`
    /// as its command stream (records, without sentinel); the other streams are empty.
    fn module(rows: u16, timer: u8, cmd: &[u8]) -> Vec<u8> {
        let mut d = b"AMC V1.2 REPLAY!".to_vec();
        put16(&mut d, rows);
        put16(&mut d, 0);
        put32(&mut d, 0);
        let v1 = 88usize;
        let v2 = v1 + 6 + 2;
        let c1 = v2 + 6;
        let c2 = c1 + cmd.len() + 2;
        let smp = c2 + 6;
        for o in [v1, v2, v2 + 2, v2 + 4, c1, c2, c2 + 2, c2 + 4] {
            put32(&mut d, o as u32);
        }
        d.resize(57, 0);
        d.push(timer);
        d.resize(72, 0);
        put32(&mut d, smp as u32);
        put16(&mut d, 4);
        put32(&mut d, smp as u32 + 8);
        put16(&mut d, 2);
        put32(&mut d, 0);
        put16(&mut d, 1);
        d.extend_from_slice(&[1, 40]);
        put16(&mut d, 400);
        put16(&mut d, rows);
        for _ in 0..3 {
            put16(&mut d, rows);
        }
        d.extend_from_slice(cmd);
        put16(&mut d, rows);
        for _ in 0..3 {
            put16(&mut d, rows);
        }
        d.resize(smp + 16, 0);
        d
    }

    fn run(d: &[u8], ticks: usize) -> Vec<Vec<W>> {
        let mut a = Amc::new(d, 0x1000);
        (0..ticks)
            .map(|_| {
                let mut w = Vec::new();
                a.tick(&mut w);
                w.retain(|x| !matches!(x, W::Wait(_)));
                w
            })
            .collect()
    }

    #[test]
    fn detects() {
        let d = module(8, 32, &[]);
        assert!(detect(&d));
        let mut e = d.clone();
        e[5] = b'3';
        assert!(!detect(&e));
        let mut f = d.clone();
        f[25] = 1;
        assert!(!detect(&f));
        assert!(!detect(&d[..40]));
    }

    #[test]
    fn tick_rate() {
        let d = module(8, 32, &[]);
        assert_eq!(Amc::new(&d, 0).tick_clocks(), 5 * (256 * 32 + 1));
        let z = module(8, 0, &[]);
        assert_eq!(Amc::new(&z, 0).tick_clocks(), 5 * 257);
    }

    #[test]
    fn row_and_note() {
        let d = module(8, 32, &[]);
        let r = run(&d, 6);
        assert_eq!(r[0][..5], [W::Vol(0, 0), W::Vol(1, 0), W::Vol(2, 0), W::Vol(3, 0), W::Dma(0x8000)]);
        assert_eq!(r[1], vec![W::Dma(0x8000)]);
        assert_eq!(r[2], vec![W::Dma(0x8000)]);
        // row 1 on the fourth tick: stop DMA, sample, volume, period
        assert_eq!(r[3][0], W::Dma(0x8000));
        assert_eq!(r[3][1], W::Dma(1));
        assert_eq!(r[3][2], W::Loc(0, 0x1000 + be32(&d, 72)));
        assert_eq!(r[3][3], W::Len(0, 4));
        assert_eq!(r[3][4], W::Vol(0, 40));
        assert_eq!(r[3][5], W::Per(0, 400));
        // the next tick starts the DMA and installs the loop
        assert_eq!(r[4][0], W::Dma(0x8001));
        assert_eq!(r[4][2], W::Len(0, 2));
    }

    #[test]
    fn wrap_and_songend() {
        let d = module(2, 32, &[]);
        let mut a = Amc::new(&d, 0);
        let mut w = Vec::new();
        for _ in 0..7 {
            a.tick(&mut w);
        }
        assert!(!a.songend());
        a.tick(&mut w); // tick 8: row 2 == R, wrap, row 0 has no note
        assert!(a.songend());
        let mut w2 = Vec::new();
        for _ in 0..4 {
            a.tick(&mut w2);
        }
        // the note of row 1 comes again 4 ticks later
        assert!(w2.contains(&W::Per(0, 400)));
    }

    #[test]
    fn slide_and_table() {
        // code 2 at row 1: X=2, step 10, duration 5, accumulator 300
        let mut c = Vec::new();
        put16(&mut c, 1);
        c.extend_from_slice(&[2, 2]);
        put16(&mut c, 10);
        put16(&mut c, 5);
        put16(&mut c, 300);
        let d = module(40, 32, &c);
        let r = run(&d, 9);
        let pers: Vec<_> = r[4..].iter().flatten().filter(|x| matches!(x, W::Per(..))).collect();
        // duration 5: four live ticks, a step on every second -> 290, 280
        assert_eq!(pers, vec![&W::Per(0, 290), &W::Per(0, 280)]);

        // code 4: count 3, table 100..800 (first value played is the second)
        let mut t = Vec::new();
        put16(&mut t, 1);
        t.extend_from_slice(&[4, 3]);
        for i in 1..=8u16 {
            put16(&mut t, i * 100);
        }
        let d = module(40, 32, &t);
        let r = run(&d, 9);
        let pers: Vec<_> = r[4..].iter().flatten().filter(|x| matches!(x, W::Per(..))).collect();
        assert_eq!(pers, vec![&W::Per(0, 200), &W::Per(0, 300), &W::Per(0, 400)]);
    }

    #[test]
    fn volume_command() {
        let mut c = Vec::new();
        put16(&mut c, 1);
        c.extend_from_slice(&[3, 0]);
        put16(&mut c, 33);
        let d = module(40, 32, &c);
        let r = run(&d, 4);
        assert!(r[3].contains(&W::Vol(0, 33)));
    }
}
