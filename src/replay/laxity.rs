//! Laxity (Eagleplayer "Laxity-Player", UADE file `Laxity`, module prefixes `powt.` and `pt.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code and nothing embedded. No real module of this format
//! is known: the specification's eight synthetic modules are the only reference.
//!
//! A module is a header (pattern offset, instrument-area size, song length, restart position, a
//! 16-byte signature area), 24-byte instruments (sample or synth), a pattern size, four position
//! lists (one track per voice) and patterns of 4-byte cells, then 8-bit samples. Four voices with
//! a volume column (set, slide, break), sixteen commands, and synth instruments whose 32-byte
//! pulse waveform the player rewrites in guest memory (at module + 0x1166 + 32 * voice) while
//! Paula plays it. Every register write is placed at the clock of the original's code from the
//! specification's cycle ledger (host-call epochs, the host's ten-line wait after DMA on).
//!
//! Start-up state: the original keeps its variables in a block of the player file that init does
//! not clear, so the first tick runs on left-over cells. The specification gives only parts of it
//! in words (speed 6, tick counter 2, the records' addresses); the rest is reconstructed here from
//! the registers the logs show: two voices' first-tick writes (a phase-1 arpeggio on voice 2
//! giving period 0xF0, an add-to-period on voice 3 giving 0x202F), the stale loop pointers of
//! voices that have no instrument, and the first tick's cycle count. Values the logs cannot show
//! (the left-over synth records' width state, volumes, vibrato and portamento state) are guesses
//! and listed in `stale_state`.
//!
//! Host-specific values: the one-shot samples' loop word and the left-over records' waveform
//! address are blocks allocated by this replayer (the reference's are addresses inside its own
//! player and host memory); the logs differ from the reference's by a constant there.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): an arpeggio
//! tick counter above 31 reads the table as the counter modulo 3; a synth width above 32 writes
//! (and is charged as) 32 bytes only; an instrument number above the module's count has start
//! address 0; bytes read outside guest memory are zero; positions past the song length keep
//! counting (16-bit) as the specification says, and the song end is signalled each time the count
//! reaches the length.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Laxity", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// The signature text of the header's second half.
const SIG: &[u8; 16] = b"YET ANOTHER COOL";
/// The synth buffers' offset from the module's start.
const SYNTH_OFF: u32 = 0x1166;

const PER: [u16; 60] = [
    3424, 3232, 3040, 2880, 2720, 2560, 2416, 2288, 2160, 2032, 1920, 1808, 1712, 1616, 1520, 1440, 1360, 1280, 1208, 1144, 1080,
    1016, 960, 904, 856, 808, 760, 720, 680, 640, 604, 572, 540, 508, 480, 452, 428, 404, 380, 360, 340, 320, 302, 286, 270, 254,
    240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];
const FT: [i16; 60] = [
    24, 24, 20, 20, 20, 18, 16, 16, 16, 14, 14, 12, 12, 12, 10, 10, 10, 9, 8, 8, 8, 7, 7, 6, 6, 6, 5, 5, 5, 4, 4, 4, 4, 3, 3, 3, 3,
    3, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];
const SINE: [u16; 8] = [0, 24, 49, 74, 97, 120, 141, 161];

/// Whether `d` is a Laxity module: the synth flags of instruments 1 and 2, and one of the three
/// alternatives of the specification's check; plus the header's own consistency (instrument-area
/// size of the form 24 * n + 16 with at least two instruments, the lists inside the file), which
/// keeps ProTracker-style files out.
fn detect(d: &[u8]) -> bool {
    if d.len() < 0x38 || d[0x1F] != 0xFF || d[0x37] != 0xFF {
        return false;
    }
    let w = |o: usize| u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let by_name = w(0) == 0x0082_0048 && w(0x30) == 0x0001_0008 && w(0x34) == 0x0018_00FF;
    if !(by_name || d[8] == 0 || &d[8..24] == SIG) {
        return false;
    }
    let s = be16(d, 2) as usize;
    let l = be16(d, 4) as usize;
    s >= 64 && s % 24 == 16 && l > 0 && 10 + s + 4 * l <= d.len()
}

#[derive(Clone, Copy, Default)]
struct Voice {
    cell: [u8; 4],
    start: u32,
    len: u16,
    lstart: u32,
    llen: u16,
    period: u16,
    vol: u8,
    vib: u8,
    vpos: u8,
    note2: u8,
    dir: u8,
    ft: i16,
    bit: u16,
}

#[derive(Clone, Copy, Default)]
struct Syn {
    on: bool,
    addr: u32,
    count: i8,
    reload: u8,
    step: i8,
    width: u8,
    min: i8,
    max: i8,
}

/// The cycle counter of one tick and the host-call epoch (specification 6.2).
struct Ck {
    e: u64,
    te: u64,
    t: u64,
}

impl Ck {
    fn at(&self) -> u64 {
        self.e + (self.t - self.te) / 2
    }

    /// A host call: `to` cycles to the call, the write, then `back` cycles; the epoch moves to the
    /// write's clock (after any wait the write caused).
    fn call(&mut self, hw: &mut Hw, to: u64, back: u64, f: impl FnOnce(&mut Hw)) {
        self.t += to;
        hw.bus.clock = self.at();
        f(hw);
        self.e = hw.bus.clock;
        self.te = self.t;
        self.t += back;
    }

    fn vol(&mut self, hw: &mut Hw, v: usize, x: u8, tail: bool) {
        self.call(hw, if tail { 84 } else { 102 }, 60, |h| h.vol(v, x as u16));
    }
    fn lc(&mut self, hw: &mut Hw, v: usize, a: u32) {
        self.call(hw, 102, 60, |h| h.loc(v, a));
    }
    fn len(&mut self, hw: &mut Hw, v: usize, w: u16) {
        self.call(hw, 134, 68, |h| h.len(v, w));
    }
    fn per(&mut self, hw: &mut Hw, v: usize, p: u16) {
        self.call(hw, 110, 60, |h| h.per(v, p));
    }
    fn dma_off(&mut self, hw: &mut Hw, mask: u16) {
        self.call(hw, 112, 68, |h| h.dmacon(mask));
    }
    /// DMA on, then the host's wait for the beam: the tenth raster line after the current one.
    fn dma_on(&mut self, hw: &mut Hw, v: u16) {
        self.call(hw, 112, 68, |h| {
            h.dmacon(v);
            h.wait_lines(10, 0);
        });
    }
    /// A host call without a register write.
    fn silent(&mut self, hw: &mut Hw, to: u64, back: u64) {
        self.call(hw, to, back, |_| {});
    }
    /// A byte write to CIA-A PRA: stamped like a host call's but it does not move the epoch.
    fn cia(&mut self, hw: &mut Hw, v: u8) {
        hw.bus.clock = self.at();
        hw.bus.write_byte(0xBFE001, v);
    }
}

pub struct Laxity {
    base: u32,
    /// Header values: instrument-area size.
    s: u16,
    /// Pattern offset (sign-extended).
    p: i32,
    z: u16,
    speed: u16,
    counter: u16,
    pos: u16,
    restart: u16,
    row: u16,
    len: u16,
    brk: u8,
    slow: u8,
    mask: u16,
    dma: u16,
    syn: [Syn; 4],
    v: [Voice; 4],
    /// Sample start of instrument `i` (index 0 unused).
    starts: Vec<u32>,
    zero: u32,
    songend: bool,
}

impl Laxity {
    fn rb(&self, hw: &Hw, o: u32) -> u8 {
        hw.bus.r8(self.base.wrapping_add(o))
    }
    fn rw(&self, hw: &Hw, o: u32) -> u16 {
        hw.bus.r16(self.base.wrapping_add(o))
    }

    /// `PER[idx]` of the specification, including the entries past the table that read the
    /// player's own variables.
    fn per_at(&self, idx: usize) -> u16 {
        match idx {
            0 => 1,
            1..=60 => PER[idx - 1],
            61 => 252,
            62 => 3648,
            64 => self.z,
            65 => self.speed,
            66 => self.counter,
            67 => self.pos,
            68 => self.restart,
            69 => self.row,
            70 => self.len,
            71 => (self.brk as u16) << 8,
            72 => (self.slow as u16) << 8,
            73 => self.mask,
            74..=78 => {
                let s = &self.syn[0];
                match idx {
                    74 => ((s.addr >> 16) as u16 & 0xFF) | if s.on { 0 } else { 0xFF00 },
                    75 => s.addr as u16,
                    76 => (s.count as u8 as u16) << 8 | s.reload as u16,
                    77 => (s.step as u8 as u16) << 8 | s.width as u16,
                    _ => (s.min as u8 as u16) << 8 | s.max as u8 as u16,
                }
            }
            _ => 0,
        }
    }

    fn ft_at(n: usize) -> i16 {
        match n {
            1..=60 => FT[n - 1],
            61 => 3424,
            62 => 3232,
            63 => 3040,
            _ => 0,
        }
    }

    // ---- volume column and slides ----

    fn up(&mut self, hw: &mut Hw, ck: &mut Ck, v: usize, x: u8) {
        ck.t += 41;
        let mut vol = self.v[v].vol.wrapping_add(x);
        if (vol as i8) > 64 {
            vol = 64;
            ck.t += 26;
        }
        self.v[v].vol = vol;
        ck.t += 26;
        ck.vol(hw, v, vol, true);
    }

    fn down(&mut self, hw: &mut Hw, ck: &mut Ck, v: usize, x: u8) {
        ck.t += 25;
        let vol = if (self.v[v].vol as i8 as i16) - (x as i16) < 0 {
            ck.t += 16;
            0
        } else {
            self.v[v].vol - x
        };
        self.v[v].vol = vol;
        ck.t += 26;
        ck.vol(hw, v, vol, true);
    }

    /// 3.4: the volume column of the voice's current cell.
    fn vc(&mut self, hw: &mut Hw, ck: &mut Ck, v: usize) {
        let b = self.v[v].cell[1];
        ck.t += 23;
        if b < 0x80 {
            ck.t += 16;
            return;
        }
        ck.t += 19;
        if b == 0xFF {
            ck.t += 16;
            return;
        }
        ck.t += 15;
        if b == 0xFE {
            ck.t += 36;
            self.brk = 0xFF;
            return;
        }
        ck.t += 41;
        if b & 0xF0 == 0x80 {
            self.up(hw, ck, v, b & 15);
            return;
        }
        ck.t += 15;
        if b & 0xF0 == 0x90 {
            self.down(hw, ck, v, b & 15);
            return;
        }
        ck.t += 16;
    }

    // ---- position, restart, synth ----

    fn next_position(&mut self, hw: &mut Hw, ck: &mut Ck) {
        ck.t += 81;
        self.row = 0;
        self.brk = 0;
        self.pos = self.pos.wrapping_add(1);
        if self.pos == self.len {
            ck.silent(hw, 64, 44);
            self.songend = true;
            self.pos = self.restart;
            ck.t += 20;
        }
    }

    /// 3.5: DMA on and every voice's loop registers.
    fn restart_dma(&mut self, hw: &mut Hw, ck: &mut Ck) {
        ck.t += 52;
        ck.dma_on(hw, self.dma);
        ck.t += 36;
        for v in 0..4 {
            if v > 0 {
                ck.t += 24;
            }
            ck.lc(hw, v, self.v[v].lstart);
            ck.t += 12;
            ck.len(hw, v, self.v[v].llen);
        }
        ck.t += 60;
    }

    fn synth(&mut self, hw: &mut Hw, ck: &mut Ck) {
        ck.t += 16;
        for r in 0..4 {
            ck.t += 21;
            let s = &mut self.syn[r];
            if !s.on {
                ck.t += 18;
                continue;
            }
            ck.t += 25;
            s.count = s.count.wrapping_sub(1);
            if s.count >= 0 {
                ck.t += 18;
                continue;
            }
            ck.t += 57;
            s.count = s.reload as i8;
            if s.step != 0 {
                ck.t += 33;
                s.width = s.width.wrapping_add(s.step as u8);
                if (s.width as i8) < s.min {
                    ck.t += 38;
                    s.width = s.min as u8;
                    s.step = s.step.wrapping_neg();
                } else {
                    ck.t += 21;
                    if (s.width as i8) > s.max {
                        ck.t += 28;
                        s.width = s.max as u8;
                        s.step = s.step.wrapping_neg();
                    }
                }
            }
            // the waveform: 16 - w/2 bytes of 0x80, w zero bytes, 16 - ceil(w/2) of 0x7F; a width
            // above 32 is cut to 32 (the original writes far past the buffer)
            let w = (s.width as usize).min(32);
            let a = 16 - w / 2;
            let c = 16 - w.div_ceil(2);
            ck.t += 67 + 18 * a as u64 + 10 + 13 + 22 * w as u64 + 21 + 18 * c as u64 + 18;
            let addr = s.addr;
            for k in 0..32usize {
                let b = if k < a {
                    0x80
                } else if k < a + w {
                    0
                } else {
                    0x7F
                };
                hw.bus.w8(addr.wrapping_add(k as u32), b);
            }
        }
        ck.t += 16;
    }

    // ---- row tick ----

    /// The instrument's byte `o` (24 bytes per instrument, from the module's start).
    fn ib(&self, hw: &Hw, i: usize, o: u32) -> u8 {
        self.rb(hw, 24 * i as u32 + o)
    }
    fn iw(&self, hw: &Hw, i: usize, o: u32) -> u16 {
        self.rw(hw, 24 * i as u32 + o)
    }

    fn row_voice(&mut self, hw: &mut Hw, ck: &mut Ck, v: usize, pos: u16, row: u16) {
        ck.t += 25;
        let l = self.len as u32;
        let b = self.rb(hw, 10 + self.s as u32 + v as u32 * l + pos as u32);
        if b == 0xFE {
            ck.t += 31;
            return;
        }
        if b == 0xFF {
            ck.t += 31;
            ck.vol(hw, v, 0, false);
            ck.t += 12 + 126;
            self.syn[v].on = false;
            return;
        }
        let pat = self.p.wrapping_add((b as i8 as i32).wrapping_mul(self.z as i32)).wrapping_add(row as i32) as u32;
        let cell = [0, 1, 2, 3].map(|k| self.rb(hw, pat.wrapping_add(k)));
        ck.t += 195;
        if cell[1] < 0x80 {
            ck.t += 33;
            self.v[v].vol = cell[1];
        }
        ck.t += 35;
        let cmd = cell[2] & 15;
        self.v[v].cell = [cell[0], cell[1], cmd, cell[3]];
        let ins = ((cell[0] >> 6) as usize) << 4 | (cell[2] >> 4) as usize;
        if ins != 0 {
            ck.t += 85;
            if self.ib(hw, ins, 7) != 0 {
                // synth instrument
                ck.t += 18 + 352 + 70;
                let buf = self.base.wrapping_add(SYNTH_OFF + 32 * v as u32);
                let reload = self.ib(hw, ins, 1);
                let mn = self.ib(hw, ins, 3);
                let mx = self.ib(hw, ins, 5);
                self.syn[v] = Syn { on: true, addr: buf, count: 0, reload, step: 1, width: mn, min: mn as i8, max: mx as i8 };
                let x = &mut self.v[v];
                x.start = buf;
                x.lstart = buf;
                x.len = 16;
                x.llen = 16;
            } else {
                ck.t += 18 + 126 + 99;
                self.syn[v].on = false;
                let start = self.starts.get(ins).copied().unwrap_or(0);
                let len = self.iw(hw, ins, 0);
                let llen = self.iw(hw, ins, 4);
                let loff = 2 * self.iw(hw, ins, 2) as u32;
                let x = &mut self.v[v];
                x.start = start;
                x.len = len;
                if llen == 0 {
                    ck.t += 50;
                    x.llen = 1;
                    x.lstart = self.zero;
                } else {
                    ck.t += 60;
                    x.llen = llen;
                    x.lstart = start.wrapping_add(loff);
                }
            }
        }
        ck.t += 32;
        let vol = self.v[v].vol;
        ck.vol(hw, v, vol, false);
        ck.t += 39;
        let n = (cell[0] & 0x3F) as usize;
        if n == 0 {
            self.tail(hw, ck, v);
            return;
        }
        ck.t += 39;
        self.v[v].note2 = (n * 2) as u8;
        if self.v[v].len == 0 {
            self.mask |= self.v[v].bit;
            ck.t += 32;
            self.tail(hw, ck, v);
            return;
        }
        ck.t += 43;
        let f = if ins != 0 { self.ib(hw, ins, 6) } else { self.rb(hw, 6) } as i8;
        let ftoff = if f != 0 {
            ck.t += 72;
            (f as i16).wrapping_mul(Self::ft_at(n))
        } else {
            0
        };
        self.v[v].ft = ftoff;
        ck.t += 61;
        let newp = self.per_at(n).wrapping_add(ftoff as u16);
        if cmd == 3 {
            ck.t += 27;
            if newp == self.v[v].period {
                ck.t += 16;
                return;
            }
            self.v[v].dir = if (newp as i16) < (self.v[v].period as i16) { 0xFF } else { 0 };
            ck.t += 27;
            self.tail(hw, ck, v);
            return;
        }
        ck.t += 40;
        self.v[v].period = newp;
        let (start, len) = (self.v[v].start, self.v[v].len);
        ck.lc(hw, v, start);
        ck.t += 12;
        ck.len(hw, v, len);
        ck.t += 12;
        ck.per(hw, v, newp);
        ck.t += 60;
        self.v[v].vpos = 0;
        self.mask |= self.v[v].bit;
        self.tail(hw, ck, v);
    }

    /// The volume column and the row command of the voice's current cell.
    fn tail(&mut self, hw: &mut Hw, ck: &mut Ck, v: usize) {
        ck.t += 18;
        self.vc(hw, ck, v);
        ck.t += 31;
        let cmd = self.v[v].cell[2] & 15;
        let par = self.v[v].cell[3];
        if cmd == 0 {
            ck.t += 16;
            return;
        }
        const ORDER: [u8; 7] = [5, 7, 11, 12, 13, 14, 15];
        match ORDER.iter().position(|&c| c == cmd) {
            None => ck.t += 7 * 15 + 16,
            Some(k) => ck.t += 15 * (k as u64 + 1),
        }
        match cmd {
            5 => {
                ck.t += 36;
                self.slow = 1;
            }
            7 => ck.t += 114,
            11 => {
                ck.t += 68;
                self.pos = (self.pos & 0xFF00) | (par.wrapping_sub(1) as u16);
                self.brk = 0xFF;
            }
            12 => {
                ck.t += 33;
                let mut x = par;
                if x > 0x40 {
                    ck.t += 4;
                    x = 0x40;
                }
                ck.t += 43;
                self.v[v].vib = x;
                ck.vol(hw, v, x, true);
            }
            13 => {
                ck.t += 78;
                self.row = (par as u16) << 2;
                self.brk = self.brk.wrapping_add(1);
            }
            14 => {
                ck.t += 24;
                // the filter on: bit 1 of PRA cleared by read-modify-write, written twice
                let x = hw.bus.read_byte(0xBFE001) & !2;
                ck.cia(hw, x);
                ck.t += 20;
                ck.cia(hw, x);
                ck.t += 16;
            }
            15 => {
                ck.t += 40;
                self.speed = par as u16;
            }
            _ => {}
        }
    }

    // ---- effect tick ----

    fn tick_effect(&mut self, hw: &mut Hw, ck: &mut Ck, v: usize) {
        ck.t += 18;
        self.vc(hw, ck, v);
        ck.t += 23;
        let cmd = self.v[v].cell[2] & 15;
        let par = self.v[v].cell[3];
        match cmd {
            0 => {
                ck.t += 23;
                if par == 0 {
                    ck.t += 16;
                    return;
                }
                ck.t += 37;
                match self.counter % 3 {
                    0 => {
                        ck.t += 24;
                        let p = self.v[v].period;
                        ck.per(hw, v, p);
                        ck.t += 28;
                    }
                    ph => {
                        let h = if ph == 1 {
                            ck.t += 13 + 14;
                            par >> 4
                        } else {
                            ck.t += 13;
                            par & 15
                        };
                        ck.t += 58;
                        let idx = ((2 * h as usize + self.v[v].note2 as usize) & 0xFF) / 2;
                        let p = self.per_at(idx).wrapping_add(self.v[v].ft as u16);
                        ck.per(hw, v, p);
                        ck.t += 16;
                    }
                }
            }
            1 => {
                ck.t += 15 + 57;
                let mut p = self.v[v].period.wrapping_sub(par as u16);
                if (p as i16) < 0x71 {
                    ck.t += 12;
                    p = 0x71;
                }
                self.v[v].period = p;
                ck.t += 20;
                ck.per(hw, v, p);
                ck.t += 28;
            }
            2 => {
                ck.t += 30 + 46;
                let p = self.v[v].period.wrapping_add(par as u16);
                self.v[v].period = p;
                ck.t += 20;
                ck.per(hw, v, p);
                ck.t += 28;
            }
            3 => {
                ck.t += 43 + 23;
                if self.v[v].dir == 0 {
                    ck.t += 16;
                    return;
                }
                ck.t += 37 + 49;
                let p = self.v[v].period.wrapping_sub(par as u16);
                self.v[v].period = p;
                ck.t += 24;
                if (p as i16) < 0 {
                    // the target is always 0: the period went below it
                    ck.per(hw, v, 0);
                    self.v[v].dir = 0;
                    ck.t += 44;
                } else {
                    ck.per(hw, v, p);
                    ck.t += 28;
                }
            }
            4 => {
                ck.t += 58 + 21;
                if par != 0 {
                    ck.t += 23;
                    let x = &mut self.v[v];
                    if par & 15 == 0 {
                        ck.t += 46;
                        x.vib = (x.vib & 0x0F) | (par & 0xF0);
                    } else {
                        ck.t += 29;
                        if par & 0xF0 == 0 {
                            ck.t += 46;
                            x.vib = (x.vib & 0xF0) | (par & 0x0F);
                        } else {
                            ck.t += 12;
                            x.vib = par;
                        }
                    }
                }
                ck.t += 175;
                let x = &mut self.v[v];
                let d = SINE[((x.vpos & 0x1F) >> 2) as usize] * (x.vib & 15) as u16 >> 7;
                let p = if x.vpos & 0x80 == 0 {
                    ck.t += 14;
                    x.period.wrapping_add(d)
                } else {
                    ck.t += 4;
                    x.period.wrapping_sub(d)
                };
                x.vpos = x.vpos.wrapping_add((x.vib >> 2) & 0x3C);
                ck.per(hw, v, p);
                ck.t += 62;
            }
            5 => {
                ck.t += 73 + 31;
                self.slow = self.slow.wrapping_sub(1);
                if self.slow != 0 {
                    ck.t += 16;
                    return;
                }
                ck.t += 35;
                if par >> 4 != 0 {
                    ck.t += 30;
                    self.slow = par >> 4;
                    self.up(hw, ck, v, 1);
                } else {
                    ck.t += 50;
                    self.slow = par & 15;
                    self.down(hw, ck, v, 1);
                }
            }
            6 => {
                ck.t += 88 + 27;
                if par == 0 {
                    ck.t += 16;
                    return;
                }
                ck.t += 159;
                if self.counter % par as u16 != 0 {
                    ck.t += 16;
                    return;
                }
                ck.t += 12;
                let bit = self.v[v].bit;
                ck.dma_off(hw, bit);
                self.dma = bit | 0x8000;
                ck.t += 52;
                let (start, len) = (self.v[v].start, self.v[v].len);
                ck.lc(hw, v, start);
                ck.t += 12;
                ck.len(hw, v, len);
                ck.t += 22;
                self.restart_dma(hw, ck);
            }
            _ => {
                ck.t += 88 + 24;
                let p = self.v[v].period;
                ck.per(hw, v, p);
                ck.t += 27;
                if cmd == 10 {
                    ck.t += 39;
                    if par >> 4 != 0 {
                        self.up(hw, ck, v, par >> 4);
                    } else {
                        ck.t += 20;
                        self.down(hw, ck, v, par & 15);
                    }
                } else {
                    ck.t += 16;
                }
            }
        }
    }

    // ---- the tick ----

    fn run(&mut self, hw: &mut Hw) {
        let mut ck = Ck { e: hw.bus.clock, te: 0, t: 157 };
        self.counter = self.counter.wrapping_sub(1);
        if (self.counter as i16) > 0 {
            for v in 0..4 {
                ck.t += if v == 0 { 38 } else { 34 };
                self.tick_effect(hw, &mut ck, v);
            }
            ck.t += 18;
            self.synth(hw, &mut ck);
            ck.t += 10;
        } else {
            ck.t += 150;
            let (pos, row) = (self.pos, self.row);
            self.mask = 0;
            for v in 0..4 {
                if v > 0 {
                    ck.t += 58;
                }
                self.row_voice(hw, &mut ck, v, pos, row);
            }
            ck.t += 18;
            self.synth(hw, &mut ck);
            ck.t += 29;
            if self.mask != 0 {
                let m = self.mask;
                ck.dma_off(hw, m);
                self.dma = m | 0x8000;
                ck.t += 38;
                self.restart_dma(hw, &mut ck);
            }
            ck.t += 65;
            self.counter = self.speed;
            self.row = self.row.wrapping_add(4);
            if self.row >= self.z {
                self.next_position(hw, &mut ck);
            }
        }
        ck.t += 29;
        while self.brk != 0 {
            self.next_position(hw, &mut ck);
            ck.t += 29;
        }
        ck.silent(hw, 34, 164);
    }
}

impl Replayer for Laxity {
    fn format(&self) -> &'static str {
        "Laxity"
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
        self.run(hw);
    }
}

/// The left-over cells, records and variables of the player's state block (specification 2.1),
/// as far as the logs show them. `stale` is the block standing for the original's absolute
/// addresses 0x57D3E, 0x57D5E, 0x57D7E and 0x57D9E (records 0 to 2 are synth records writing
/// there; record 3 is off).
///
/// What the logs fix: voice 2's cell is an arpeggio with note 47 and a parameter whose high nibble is
/// zero (first-tick write 0x00F0 at colour clock 427, tick-effect phase 1 of counter 1, volume
/// column 0xFF), voice 3's an add-to-period command (0x202F, 38 cycles away); voices 0 and 1 hold
/// a command-0 cell without parameter; each voice's sample and loop start is its record's address
/// with 16 words. Voice 3's volume is 0x20 (syn4's first row writes it unchanged); the others' volumes are guesses (0), vibrato and portamento state zero, the synth records' limits (min 3,
/// max 16: not visible). The records' state (countdown 1, reload 1, step +1, width 3) follows from
/// the waveform the reference's DMA reads in syn4's first row (widths 4, 5, 6 on every second tick).
fn stale_state(stale: u32) -> ([Voice; 4], [Syn; 4]) {
    let mut v = [Voice::default(); 4];
    let mut s = [Syn::default(); 4];
    for i in 0..4 {
        let a = stale + 32 * i as u32;
        v[i] = Voice { start: a, len: 16, lstart: a, llen: 16, bit: 1 << i, ..Voice::default() };
        s[i] = Syn { on: i < 3, addr: a, count: 1, reload: 1, step: 1, width: 3, min: 3, max: 16 };
    }
    // voice 2: arpeggio, note 47 (period 240), parameter 1 (low nibble only, high nibble 0)
    v[2].cell = [47, 0xFF, 0x00, 0x01];
    v[2].note2 = 94;
    v[2].period = 0x00F0;
    // voice 3: add-to-period with parameter 0, period 0x202F
    v[3].cell = [0, 0, 0x02, 0x00];
    v[3].period = 0x202F;
    v[3].vol = 0x20;
    (v, s)
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let zero = m.alloc(2);
    let stale = m.alloc(128);
    if zero == 0 || stale == 0 {
        return None;
    }
    let rb = |o: u32| m.bus.r8(base.wrapping_add(o));
    let rw = |o: u32| m.bus.r16(base.wrapping_add(o));
    let p = rw(0) as i16 as i32;
    let s = rw(2);
    let len = rw(4);
    let restart = rw(6);
    let z = rw(8 + s as u32);
    // the sample starts: after the patterns the lists name (the largest position byte plus one,
    // times the pattern size, as a signed 16-bit value)
    let mut mx = 0u8;
    let mut updates = 0u64;
    for k in 0..4 * len as u32 {
        let b = rb(10 + s as u32 + k);
        if b > mx {
            mx = b;
            updates += 1;
        }
    }
    let dd = ((mx as u16 + 1).wrapping_mul(z)) as i16 as i32;
    let mut at = base.wrapping_add((10 + s as u32).wrapping_add(4 * len as u32)).wrapping_add(dd as u32);
    let n = ((s as i16).max(0) / 24) as usize;
    let mut starts = vec![0u32; n + 2];
    for i in 1..=n {
        starts[i] = at;
        at = at.wrapping_add(2 * rw(24 * i as u32) as u32);
    }
    let (v, syn) = stale_state(stale);
    let r = Laxity {
        base,
        s,
        p,
        z,
        speed: 6,
        counter: 2,
        pos: 0,
        restart,
        row: 0,
        len,
        brk: 0,
        slow: 0,
        mask: 0,
        dma: 0,
        syn,
        v,
        starts,
        zero,
        songend: false,
    };
    // the reference's first call: its start-up (a fixed part plus the init's work: 54 colour
    // clocks per position, 29 per instrument, 6 per new largest position byte) counted from the
    // run's origin, START_PHASE before the engine's aligned start; the clock is left one period
    // before it, as the engine adds the first period
    let call = 76173 + 54 * len as u64 + 29 * n as u64 + 6 * updates;
    let t = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    m.bus.clock = (t + call).saturating_sub(TICK).max(m.bus.clock);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "laxity_tests.rs"]
mod tests;
