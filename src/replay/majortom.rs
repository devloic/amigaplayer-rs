//! Major Tom ("Major Tom's Player 2.01"; Eagleplayer "MajorTom", module suffix `.arp`; the
//! prefixes `hn`, `mtp2` and `thn` of the Eagleplayer match no known file).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! and cycle costs of the 68k reference player; no original code. A module is the mark `ARP.`, a
//! table of 13 offsets, a song (length, restart, speed, position list, transpose list), an
//! instrument table, four banks of 64-byte sequences (volume, waveform, arpeggio, pitch),
//! 32-byte single-cycle waveforms and 1024-byte patterns of 64 rows of four voice entries.
//! Every write is placed at the clock of the original's code from the specification's cycle
//! ledger.
//!
//! Two small tables that belong to the original player rather than to the format are embedded on
//! the project owner's decision, taken from the original player/reference host: the 3000-byte
//! noise block (`majortom_noise.bin`, played for waveform value 0xFF) and the 15 words the
//! original reads past the end of its period table (`majortom_overrun.bin`).
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a file whose
//! first twelve offsets do not lie inside it is not a module; bytes asked for beyond the end of
//! the file read as zero; period table indices that the specification does not know (104..390
//! other than 180) read as zero; pattern numbers of 32 and over address memory before the
//! patterns as the specification says (bytes outside the file read as zero); speed 0 gives a
//! 65536-tick row; the next/previous-position functions the host never calls are not provided.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "MajorTom", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// Colour clock at which the host calls the player's sound initialisation.
const INIT_AT: u64 = 1600;

const NOISE: &[u8; 3000] = include_bytes!("majortom_noise.bin");
const OVERRUN: &[u8; 30] = include_bytes!("majortom_overrun.bin");

const PERIODS: [u16; 88] = [
    0x0D40, 0x0C80, 0x0BD0, 0x0B20, 0x0A80, 0x09F0, 0x0960, 0x08D0, 0x0850, 0x07E0, 0x0770, 0x0700, 0x06A0,
    0x0640, 0x05E8, 0x0590, 0x0540, 0x04F8, 0x04B0, 0x0468, 0x0428, 0x03F0, 0x03B8, 0x0380, 0x0350, 0x0320,
    0x02F4, 0x02C8, 0x02A0, 0x027C, 0x0258, 0x0234, 0x0214, 0x01F8, 0x01DC, 0x01C0, 0x01A8, 0x0190, 0x017A,
    0x0164, 0x0150, 0x013E, 0x012C, 0x011A, 0x010A, 0x00FC, 0x00EE, 0x00E0, 0x00D4, 0x00C8, 0x00BD, 0x00B2,
    0x00A8, 0x009F, 0x0096, 0x008D, 0x0085, 0x007E, 0x0077, 0x0070, 0x00D4, 0x00C8, 0x00BD, 0x00B2, 0x00A8,
    0x009F, 0x0096, 0x008D, 0x0085, 0x007E, 0x0077, 0x0070, 0x0070, 0x0070, 0x0070, 0x0070, 0x0070, 0x0070,
    0x0070, 0x0070, 0x0070, 0x0070, 0x0070, 0x0070, 0x0070, 0x0070, 0x0070, 0x0070,
];

/// Cycle costs of selecting a voice's registers: for a period or address write, and for a volume write.
const SV: [u64; 4] = [23, 54, 85, 93];
const WV: [u64; 4] = [107, 130, 153, 143];

fn byte(d: &[u8], o: i64) -> u8 {
    usize::try_from(o).ok().and_then(|o| d.get(o)).copied().unwrap_or(0)
}

fn word(d: &[u8], o: i64) -> u16 {
    (byte(d, o) as u16) << 8 | byte(d, o + 1) as u16
}

fn long(d: &[u8], o: i64) -> u32 {
    (word(d, o) as u32) << 16 | word(d, o + 2) as u32
}

fn off(d: &[u8], i: usize) -> i64 {
    long(d, 6 + 4 * i as i64) as i64
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 6 + 52 && d[..4] == *b"ARP." && (0..12).all(|i| off(d, i) < d.len() as i64)
}

/// The period table including what the original's memory holds past its end.
fn table(i: u32) -> u16 {
    match i {
        0..=87 => PERIODS[i as usize],
        88 => 0x4246,
        89..=103 => {
            let k = 2 * (i - 89) as usize;
            u16::from_be_bytes([OVERRUN[k], OVERRUN[k + 1]])
        }
        180 => 0x0076,
        _ => 0,
    }
}

/// One of a voice's four sequences. `start` is None for the all-zero area of a voice that has not
/// had an instrument yet.
#[derive(Clone, Copy)]
struct Seq {
    start: Option<i64>,
    pos: u32,
    cnt: u8,
    speed: u8,
}

impl Seq {
    const ZERO: Seq = Seq { start: None, pos: 1, cnt: 0, speed: 0 };

    fn at(&self, d: &[u8], i: u32) -> u8 {
        self.start.map_or(0, |s| byte(d, s + i as i64))
    }

    /// Counts down; true when a step is due (the counter is reloaded).
    fn due(&mut self) -> bool {
        self.cnt = self.cnt.wrapping_sub(1);
        if self.cnt == 0 {
            self.cnt = self.speed;
            true
        } else {
            false
        }
    }

    /// The step value, following a jump; the bool says whether there was one.
    fn read(&mut self, d: &[u8]) -> (u8, bool) {
        let v = self.at(d, self.pos);
        if v == 0xF0 {
            self.pos = self.at(d, self.pos + 1) as u32;
            (self.at(d, self.pos), true)
        } else {
            (v, false)
        }
    }

    /// Moves on; true when the position reached 63 and was pulled back to `clamp`.
    fn advance(&mut self, clamp: u32) -> bool {
        self.pos = self.pos.wrapping_add(1);
        if self.pos == 63 {
            self.pos = clamp;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy)]
struct Voice {
    note: u8,
    eff: u8,
    par: u8,
    /// Volume, waveform, arpeggio and pitch sequences.
    seq: [Seq; 4],
    slide: u16,
    noise: bool,
    pcur: u16,
    ptgt: u16,
    dir: u8,
    fresh: bool,
    arp: u8,
}

impl Voice {
    const NEW: Voice = Voice {
        note: 0,
        eff: 0,
        par: 0,
        seq: [Seq::ZERO; 4],
        slide: 0,
        noise: false,
        pcur: 0,
        ptgt: 0,
        dir: 0,
        fresh: false,
        arp: 0,
    };
}

const VOL: usize = 0;
const WAVE: usize = 1;
const ARP: usize = 2;
const PIT: usize = 3;

pub struct MajorTom {
    d: Vec<u8>,
    base: u32,
    noise: u32,
    len: u8,
    restart: u8,
    speed: u16,
    counter: u16,
    pos: u8,
    row: u16,
    v: [Voice; 4],
    songend: bool,
}

/// The cycle ledger of one tick: the cycle count and the clock the tick was called at.
struct Ledger {
    t0: u64,
    t: u64,
}

impl Ledger {
    fn add(&mut self, c: u64) {
        self.t += c;
    }
    /// Adds `c` cycles and moves the clock to the write that ends them.
    fn at(&mut self, hw: &mut Hw, c: u64) {
        self.t += c;
        hw.bus.clock = self.t0 + self.t / 2;
    }
}

impl MajorTom {
    fn build(d: &[u8], base: u32, noise: u32) -> Self {
        let o = |i| off(d, i);
        MajorTom {
            d: d.to_vec(),
            base,
            noise,
            len: byte(d, o(0)),
            restart: byte(d, o(1)),
            speed: word(d, o(2)),
            counter: 4,
            pos: 0,
            row: 0,
            v: [Voice::NEW; 4],
            songend: false,
        }
    }

    fn o(&self, i: usize) -> i64 {
        off(&self.d, i)
    }

    /// One voice's entry of the current row.
    fn fetch(&mut self, v: usize, entry: i64, transpose: u8, l: &mut Ledger) {
        let note = byte(&self.d, entry);
        let ins = byte(&self.d, entry + 1);
        let (eff, par) = (byte(&self.d, entry + 2), byte(&self.d, entry + 3));
        l.add(19);
        if note == 0 {
            l.add(56);
            let s = &mut self.v[v];
            s.eff = eff;
            s.par = par;
            s.fresh = false;
            return;
        }
        l.add(75);
        let s = &mut self.v[v];
        s.note = note.wrapping_add(transpose);
        s.pcur = s.ptgt;
        s.dir = 0;
        s.eff = eff;
        s.par = par;
        if ins == 0 {
            l.add(56);
            s.fresh = false;
            return;
        }
        l.add(594);
        let rec = self.o(5) + 4 * ins as i64;
        for k in 0..4 {
            let n = byte(&self.d, rec + k as i64) as i64;
            let start = self.o(6 + k) + 64 * n;
            let speed = byte(&self.d, start);
            self.v[v].seq[k] = Seq { start: Some(start), pos: 1, cnt: 1, speed };
        }
        let s = &mut self.v[v];
        s.slide = 0;
        s.fresh = true;
        s.arp = 1;
    }

    fn play_row(&mut self, l: &mut Ledger) {
        let pat = byte(&self.d, self.o(3) + self.pos as i64) as i64;
        let transpose = byte(&self.d, self.o(4) + self.pos as i64);
        let start = self.o(11) + (pat * 1024) as u16 as i16 as i64;
        for v in 0..4 {
            l.add(if v == 0 { 180 } else { 188 });
            self.fetch(v, start + self.row as i64 + 4 * v as i64, transpose, l);
            l.add(25);
            if self.v[v].eff == 0x0F {
                l.add(52);
                self.v[v].eff = 0;
                self.speed = self.v[v].par as u16;
            }
        }
    }

    fn advance_row(&mut self, l: &mut Ledger) {
        l.add(73);
        self.row += 16;
        if self.row == 1024 {
            l.add(73);
            self.row = 0;
            self.pos = self.pos.wrapping_add(1);
            if self.pos == self.len {
                l.add(60);
                l.t &= !1;
                l.add(16);
                self.pos = self.restart;
                self.songend = true;
            }
        }
    }

    fn tick_inner(&mut self, hw: &mut Hw) {
        let mut l = Ledger { t0: hw.bus.clock, t: 409 };
        self.counter = self.counter.wrapping_sub(1);
        if self.counter == 0 {
            l.add(28);
            self.counter = self.speed;
            self.play_row(&mut l);
            self.advance_row(&mut l);
        }
        for v in 0..4 {
            l.add(37);
            if !self.v[v].fresh {
                l.add(30);
                let ctx = Ctx { d: &self.d, base: self.base, noise: self.noise, o10: self.o(10), pos: self.pos, len: self.len };
                if update(&ctx, hw, v, &mut self.v[v], &mut l) {
                    // pattern break: the next position, or the first one past the song's end
                    self.row = 0;
                    self.pos = self.pos.wrapping_add(1);
                    if self.pos >= self.len {
                        self.pos = 0;
                    }
                }
            }
            l.add(16);
            self.v[v].fresh = false;
        }
        l.at(hw, 20);
        hw.dmacon(0x0000);
        l.at(hw, 20);
        hw.dmacon(0x800F);
    }
}

struct Ctx<'a> {
    d: &'a [u8],
    base: u32,
    noise: u32,
    o10: i64,
    pos: u8,
    len: u8,
}

/// One voice's update (the specification's 4.6 to 4.12); true when it performed a pattern break.
fn update(c: &Ctx, hw: &mut Hw, v: usize, s: &mut Voice, l: &mut Ledger) -> bool {
    let d = c.d;
    // waveform sequence
    l.add(27);
    if s.seq[WAVE].due() {
        l.add(93);
        let raw = s.seq[WAVE].at(d, s.seq[WAVE].pos);
            if raw != 0 {
            l.add(17);
            let mut w = raw;
            if raw == 0xF0 {
                l.add(62);
                let q = &mut s.seq[WAVE];
                q.pos = q.at(d, q.pos + 1) as u32;
                // the original reads the value from the module at the sequence's start plus the
                // long in the sequence's own bytes 24..27 (a defect); 24-bit offsets
                w = match q.start {
                    Some(st) => byte(d, (st + long(d, st + 24) as i64) & 0xFF_FFFF),
                    None => 0,
                };
            }
            l.add(25);
            if w == 0xFF {
                l.at(hw, 12);
                hw.dmacon(1 << v);
                l.at(hw, 60);
                hw.loc(v, c.noise.wrapping_add(2u32.wrapping_mul(s.seq[VOL].pos)));
                l.at(hw, 32);
                hw.len(v, 700);
                l.at(hw, 16);
                hw.per(v, 200);
                l.add(134 + SV[v]);
                s.noise = true;
            } else {
                l.add(25);
                if s.noise {
                    l.at(hw, 12);
                    hw.dmacon(1 << v);
                    s.noise = false;
                    l.add(16);
                }
                l.at(hw, 52);
                hw.loc(v, c.base.wrapping_add((c.o10 + 32 * w as i64) as u32));
                l.add(48 + SV[v]);
            }
        }
        l.add(57);
        if s.seq[WAVE].advance(62) {
            l.add(24);
        }
    }
    // arpeggio sequence
    l.add(25);
    if s.seq[ARP].due() {
        l.add(83);
        let (a, jumped) = s.seq[ARP].read(d);
            if jumped {
            l.add(28);
        }
        s.arp = a;
        l.add(50 + 57);
        if s.seq[ARP].advance(62) {
            l.add(24);
        }
    }
    // period
    l.add(255);
    let n = s.note as u32;
    let a = s.arp as u32;
    let idx = (n & 15) + (a & 15) + 12 * ((n.wrapping_sub(16) & 255) >> 4) + 12 * (a >> 4);
    s.ptgt = table(idx);
    // portamento
    if s.dir == 0 {
        l.add(25);
    }
    if s.dir == 0 && s.eff == 3 {
        l.add(41);
        if (s.ptgt.wrapping_sub(s.pcur) as i16) >= 0 {
            s.dir = 1;
            l.add(26);
        } else {
            s.dir = 2;
            l.add(16);
        }
    }
    let mut period = s.ptgt;
    if s.dir != 0 {
        l.add(94);
        let step = s.par as u16;
        let reached;
        if s.dir == 1 {
            s.pcur = s.pcur.wrapping_add(step);
            reached = (s.pcur.wrapping_sub(s.ptgt) as i16) >= 0;
        } else {
            s.pcur = s.pcur.wrapping_sub(step);
            reached = (s.ptgt.wrapping_sub(s.pcur) as i16) >= 0;
        }
        if reached {
            s.pcur = s.ptgt;
            l.add(42);
        } else {
            l.add(12);
        }
        period = s.pcur;
    }
    // pattern break
    let mut brk = false;
    l.add(25);
    if s.eff == 0x0D {
        l.add(101);
        s.eff = 0;
        brk = true;
        if c.pos.wrapping_add(1) >= c.len {
            l.add(20);
        }
    }
    l.add(25);
    if s.eff == 2 {
        l.add(32);
        s.slide = s.slide.wrapping_add(s.par as u16);
    }
    l.add(25);
    if s.eff == 1 {
        l.add(32);
        s.slide = s.slide.wrapping_sub(s.par as u16);
    }
    l.add(12);
    period = period.wrapping_add(s.slide);
    // pitch sequence
    l.add(39);
    if s.seq[PIT].due() {
        l.add(83);
        let (p, jumped) = s.seq[PIT].read(d);
            if jumped {
            l.add(62);
        }
        l.add(25);
        if p < 0x10 {
            l.add(14);
            period = period.wrapping_add(p as u16);
        } else {
            l.add(12);
            period = period.wrapping_sub((p & 15) as u16);
        }
        l.at(hw, 44 + SV[v]);
        hw.per(v, period);
        l.add(25);
        if !s.noise {
            l.at(hw, 16);
            hw.len(v, 16);
            l.add(16);
        }
        l.add(69);
        if s.seq[PIT].advance(60) {
            l.add(24);
        }
    }
    // volume sequence
    l.add(25);
    if s.seq[VOL].due() {
        l.add(83);
        let (x, jumped) = s.seq[VOL].read(d);
        if jumped {
            l.add(62);
        }
        l.at(hw, 104 + WV[v]);
        hw.vol(v, x as u16);
        l.add(101);
        if s.seq[VOL].advance(60) {
            l.add(24);
        }
    }
    l.add(16);
    brk
}

impl Replayer for MajorTom {
    fn format(&self) -> &'static str {
        "MajorTom"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }
}

/// The start-up writes at their ledger clocks; returns the clock sound initialisation returns at.
fn start_writes(hw: &mut Hw, origin: u64, d: &[u8], zero: u32) -> u64 {
    for (round, first) in [401u64, 1077].into_iter().enumerate() {
        for (v, step) in [0u64, 152, 316, 474].into_iter().enumerate() {
            let _ = round;
            hw.bus.clock = origin + first + step;
            hw.vol(v, 0);
        }
    }
    let call = origin + INIT_AT;
    let mut t: u64 = 3852;
    let n = byte(d, off(d, 0)) as i64 + 1;
    let mut max = 0u8;
    for i in 0..n {
        let b = byte(d, off(d, 1) + i);
        t += 31;
        if b >= max {
            max = b;
            t += 4;
        }
    }
    t += 40;
    let at = |hw: &mut Hw, t: u64| hw.bus.clock = call + t / 2;
    for v in 0..4 {
        t += 16;
        at(hw, t);
        hw.loc(v, zero);
    }
    t += 4;
    for v in 0..4 {
        t += 12;
        at(hw, t);
        hw.len(v, 2);
    }
    t += 8;
    for v in 0..4 {
        t += 12;
        at(hw, t);
        hw.per(v, 200);
    }
    t += 4;
    for v in 0..4 {
        t += 12;
        at(hw, t);
        hw.vol(v, 0);
    }
    t += 576 + 16;
    at(hw, t);
    hw.dmacon(0x000F);
    t += 140;
    call + t / 2
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    // the noise block with the 4-byte zero area directly after it, as in the reference
    let buf = m.alloc(NOISE.len() as u32 + 4);
    if buf == 0 {
        return None;
    }
    m.bus.write_bytes(buf, NOISE);
    m.bus.write_bytes(buf + NOISE.len() as u32, &[0; 4]);
    let r = MajorTom::build(d, base, buf);
    // the reference's start-up counts from the run's origin, START_PHASE colour clocks before the
    // engine's aligned start; the first tick follows one period after the clock left here
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let end = {
        let mut hw = Hw { bus: &mut m.bus };
        start_writes(&mut hw, origin, d, buf + NOISE.len() as u32)
    };
    m.bus.clock = end.max(m.bus.clock);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "majortom_tests.rs"]
mod tests;
