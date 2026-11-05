//! AudioSculpture (the AudioSculpture / StarTrekker "AM" replay as adapted for UADE; Eagleplayer
//! "AudioSculpture", module prefixes `adsc.` and `mod_adsc4.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes,
//! DMA fetches and cycle costs of the 68k reference player; no original code. A module is a
//! 4-channel, 31-sample tracker module tagged `EXO4` or `FLT4`, whose synthetic ("AM") instruments
//! live in a companion file (the module's name plus `.AS`, else `.NT`): an attack, decay, sustain,
//! hold and release envelope on the volume, a waveform (sine, ramp, square or a noise buffer that
//! is regenerated from the beam position every tick), a pitch fall and a table-driven vibrato.
//! Everything else is ProTracker-like with its own quirks (see the specification). Every register
//! write is placed at the colour clock of the original's code from the specification's cycle
//! ledger, including the raster waits of the row tick and the beam reads of the noise generator.
//!
//! The 180-word AM vibrato table is embedded (`audiosculpture_vibrato.bin`): it is not a clean
//! formula, and the project owner decided to take it from the original player.
//!
//! The companion's user-code slots (command E) hold machine code in the original; an open
//! replayer cannot run it, so every slot is treated as empty and command E does nothing (the
//! output then differs from the original for a module that uses it).
//!
//! Choices where the specification is silent (all safe, none can panic or hang): bytes asked for
//! beyond the end of the module or the companion read as zero; the 4-byte clear of each sample's
//! start at song start is skipped past the end of the module; an AM waveform number above 14 plays
//! as 14 (numbers 4 to 14 read the vibrato table that follows the four waveforms in the player's
//! own memory); an instrument number above 31 reads its header and record from wherever the
//! arithmetic puts them (zero past the end, so a sample or nothing); a subsong index past the
//! count plays the last one; the module is refused without a companion.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "AudioSculpture", detect, new };

/// Colour clocks between two ticks (125 BPM).
const TICK: u64 = 70940;
/// Colour clock (from the run's origin) of the reference's first initialisation write.
const INIT_AT: u64 = 31070;
/// Colour clocks from the last initialisation write to the first tick.
const FIRST_TICK: u64 = 71281;
/// Colour clocks from the last LED write to a host-made re-initialisation's first write.
const STOP_AT: u64 = 370;
/// Colour clocks from the stop's DMA write to the initialisation's first write.
const STOP_TO_INIT: u64 = 35;
const LINE: u64 = 227;

const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254,
    240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];
const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212,
    197, 180, 161, 141, 120, 97, 74, 49, 24,
];
/// The AM vibrato table: 180 big-endian signed words, a half sine of amplitude 128.
static AMVIB_BYTES: &[u8] = include_bytes!("audiosculpture_vibrato.bin");

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn word(d: &[u8], o: usize) -> u16 {
    ((byte(d, o) as u16) << 8) | byte(d, o + 1) as u16
}

fn amvib(i: i32) -> i16 {
    if i < 0 {
        return 0;
    }
    let o = 2 * i as usize;
    match AMVIB_BYTES.get(o..o + 2) {
        Some(b) => i16::from_be_bytes([b[0], b[1]]),
        None => 0,
    }
}

fn detect(d: &[u8]) -> bool {
    if d.len() < 0x43C || !(&d[0x438..0x43C] == b"EXO4" || &d[0x438..0x43C] == b"FLT4") {
        return false;
    }
    (0..32).any(|i| {
        let s = 0x2A + 30 * i;
        word(d, s) == 0 && byte(d, s + 3) != 0
    })
}

/// An AM record of the companion.
#[derive(Clone, Copy, Default)]
struct Rec {
    am: bool,
    start: u16,
    t1: i16,
    r1: i16,
    t2: i16,
    r2: i16,
    sus: i16,
    r3: i16,
    hold: i16,
    release: i16,
    wave: u16,
    fall: i16,
    vibdepth: i16,
    vibspeed: i16,
    octave: u16,
}

fn rec_at(c: &[u8], n: usize) -> Rec {
    let o = 0x18 + 0x78 * n;
    let w = |k: usize| word(c, o + k);
    Rec {
        am: byte(c, o) == b'A' && byte(c, o + 1) == b'M',
        start: w(6),
        t1: w(8) as i16,
        r1: w(10) as i16,
        t2: w(12) as i16,
        r2: w(14) as i16,
        sus: w(16) as i16,
        r3: w(18) as i16,
        hold: w(20) as i16,
        release: w(24) as i16,
        wave: w(26),
        fall: w(28) as i16,
        vibdepth: w(30) as i16,
        vibspeed: w(32) as i16,
        octave: w(34),
    }
}

#[derive(Clone, Copy, Default)]
struct Chan {
    cell: [u8; 4],
    start: u32,
    length: u16,
    loopstart: u32,
    replen: u16,
    period: u16,
    outper: u16,
    volume: u32,
    target: u16,
    speed: u16,
    dir: u8,
    vcmd: u8,
    vpos: u8,
    am: bool,
    inst: u8,
    level: i16,
    stage: u8,
    hold: i16,
    phase: i16,
}

/// The cycle ledger of a tick, in the run's own clock (colour clocks from the origin, doubled).
struct Led {
    origin: u64,
    a: u64,
}

impl Led {
    /// Adds a cost; the clock of the next write is taken after adding its own.
    fn at(&mut self, hw: &mut Hw, n: u64) {
        self.a += n;
        hw.bus.clock = self.origin + self.a / 2;
    }

    fn line(&self) -> u64 {
        self.a / 2 / LINE
    }

    /// A raster wait: five line changes, each found by sampling the beam (a 16-cycle read, then
    /// 9 cycles whether or not the line has changed, then 10 once it has).
    fn wait(&mut self) {
        self.a += 26;
        for _ in 0..5 {
            self.a += 16;
            let l0 = self.line();
            loop {
                self.a += 16;
                let changed = self.line() != l0;
                self.a += 9;
                if changed {
                    break;
                }
            }
            self.a += 10;
        }
        self.a += 114;
    }
}

pub struct AudioSculpture {
    d: Vec<u8>,
    base: u32,
    recs: Vec<Rec>,
    /// Sample start offsets (from the module's start) of instruments 1 to 255.
    starts: Vec<u32>,
    wave_addr: u32,
    noise: [u8; 32],
    songlen: u8,
    restart: u8,
    /// Start positions of the subsongs after the first.
    subs: Vec<u8>,
    sub: u16,
    m_count: u64,
    origin: u64,
    led: u64,
    speed: u8,
    counter: u8,
    row: u8,
    pos: u8,
    brk: bool,
    dmamask: u16,
    visited: [u64; 128],
    ch: [Chan; 4],
    songend: bool,
}

impl AudioSculpture {
    fn build(d: &[u8], comp: &[u8], base: u32, wave_addr: u32) -> Option<Self> {
        if !detect(d) {
            return None;
        }
        let mut npat = 0u32;
        let mut m_count = 0u64;
        let mut max = 0i8;
        for i in 0..128 {
            let e = byte(d, 0x3B8 + i) as i8;
            if e > max {
                max = e;
                m_count += 1;
            }
        }
        if max >= 0 {
            npat = max as u32;
        }
        npat += 1;
        let mut starts = vec![0u32; 256];
        let mut s = 0x43C + 1024 * npat;
        for n in 1..256usize {
            starts[n] = s;
            s = s.wrapping_add(2 * word(d, 0x2A + 30 * (n - 1)) as u32);
        }
        let recs = (0..256).map(|n| rec_at(comp, n)).collect();
        let songlen = byte(d, 0x3B6);
        let mut r = AudioSculpture {
            d: d.to_vec(),
            base,
            recs,
            starts,
            wave_addr,
            noise: [0; 32],
            songlen,
            restart: byte(d, 0x3B7),
            subs: Vec::new(),
            sub: 0,
            m_count,
            origin: 0,
            led: 0,
            speed: 6,
            counter: 0,
            row: 0,
            pos: 0,
            brk: false,
            dmamask: 0,
            visited: [0; 128],
            ch: [Chan::default(); 4],
            songend: false,
        };
        r.subs = r.scan_subsongs();
        Some(r)
    }

    fn cell(&self, pos: u8, row: u8, c: usize) -> [u8; 4] {
        let pat = byte(&self.d, 0x3B8 + (pos & 0x7F) as usize) as usize;
        let o = 0x43C + 1024 * pat + 16 * row as usize + 4 * c;
        [byte(&self.d, o), byte(&self.d, o + 1), byte(&self.d, o + 2), byte(&self.d, o + 3)]
    }

    /// The subsong scan of the specification: the start positions of subsongs 2 and on.
    fn scan_subsongs(&self) -> Vec<u8> {
        let mut starts = Vec::new();
        let mut a0 = None;
        for p in 0..self.songlen.min(128) {
            let mut found = None;
            'cells: for row in 0..64u8 {
                for c in 0..4 {
                    let cell = self.cell(p, row, c);
                    if cell[2] & 0x0F == 0x0B {
                        found = Some(cell[3]);
                        break 'cells;
                    }
                }
            }
            if let Some(arg) = found {
                match a0 {
                    None => a0 = Some(arg),
                    Some(a) if a == arg => return Vec::new(),
                    _ => {}
                }
                starts.push(p.wrapping_add(1));
            }
        }
        starts.pop();
        starts
    }

    fn sample_addr(&self, n: usize) -> u32 {
        self.base.wrapping_add(self.starts[n & 0xFF])
    }

    /// The table that arpeggio reads past the period table (spec 7.1).
    fn etable(&self, i: usize) -> u16 {
        match i {
            0..=35 => PERIODS[i],
            36 => 0,
            37 => (self.speed as u16) << 8 | self.counter as u16,
            38 => (self.row as u16) << 4,
            39 => (self.pos as u16) << 8 | self.brk as u16,
            40 => self.dmamask,
            _ => {
                let k = i - 41;
                let n = k / 2 + 1;
                if n > 255 {
                    return 0;
                }
                let a = self.sample_addr(n);
                if k % 2 == 0 { (a >> 16) as u16 } else { a as u16 }
            }
        }
    }

    fn visited(&self, pos: u8, row: u8) -> bool {
        self.visited[(pos & 0x7F) as usize] >> row & 1 != 0
    }

    // ---- init ----

    /// The initialisation's writes from the first one at `first` (run clock); sets the clock of
    /// the LED write and of the first tick.
    fn init(&mut self, hw: &mut Hw, first: u64, sub: u16) {
        for v in 0..4u64 {
            hw.bus.clock = self.origin + first + 10 * v;
            hw.vol(v as usize, 0);
        }
        hw.bus.clock = self.origin + first + 40;
        hw.dmacon(0x000F);
        // the first four bytes of every sample are cleared (the module is in guest memory)
        for n in 1..=31usize {
            let o = self.starts[n] as usize;
            if o + 4 <= self.d.len() {
                hw.bus.write_bytes(self.base.wrapping_add(o as u32), &[0; 4]);
                for k in 0..4 {
                    self.d[o + k] = 0;
                }
            }
            let f = 0x14 + 30 * (n - 1) + 24;
            if f < self.d.len() {
                self.d[f] = 0;
            }
        }
        self.led = first + 3676 + 2 * self.m_count;
        hw.bus.clock = self.origin + self.led;
        let pra = hw.bus.read_byte(0xBFE001);
        hw.bus.write_byte(0xBFE001, pra | 2);
        self.speed = 6;
        self.counter = 0;
        self.row = 0;
        self.brk = false;
        self.sub = sub.min(self.subs.len() as u16);
        self.pos = if self.sub == 0 { 0 } else { self.subs[self.sub as usize - 1] };
        // the clock after the init: one tick period before the first tick
        hw.bus.clock = self.origin + self.led + FIRST_TICK - TICK;
    }

    // ---- row tick ----

    fn row_commands(&mut self, c: usize, hw: &mut Hw, l: &mut Led) {
        let cmd = self.ch[c].cell[2] & 0x0F;
        let arg = self.ch[c].cell[3];
        l.a += 37;
        if cmd == 0x0E {
            // an empty user-code slot (see the header)
            l.a += 271 + 116;
            return;
        }
        l.a += 19;
        if cmd == 0x0D {
            l.a += 36;
            self.brk = true;
            return;
        }
        l.a += 19;
        if cmd == 0x0B {
            l.a += 68;
            self.brk = true;
            self.pos = arg.wrapping_sub(1);
            return;
        }
        l.a += 19;
        if cmd == 0x0C {
            l.a += 25;
            let mut a = arg;
            if a > 64 {
                l.a += 16;
                a = 64;
                self.ch[c].cell[3] = 64;
            }
            l.a += 28;
            self.ch[c].volume = a as u32;
            l.at(hw, 12);
            hw.vol(c, a as u16);
            l.a += 16;
            return;
        }
        l.a += 19;
        if cmd == 0x0F {
            l.a += 33;
            let mut a = arg;
            if a > 31 {
                l.a += 4;
                a = 31;
            }
            l.a += 13;
            if a == 0 {
                l.a += 4;
                a = 1;
            }
            self.speed = a;
            l.a += 32;
            return;
        }
        l.a += 16;
    }

    fn note(&mut self, c: usize, pos: u8, hw: &mut Hw, l: &mut Led) {
        let cell = self.cell(pos, self.row, c);
        self.ch[c].cell = cell;
        let inst = (cell[0] & 0xF0) | (cell[2] >> 4);
        let per = ((((cell[0] as u16) << 8) | cell[1] as u16) & 0x0FFF) as u16;
        let cmd = cell[2] & 0x0F;
        let bit = 1u16 << c;
        l.a += 87;
        if inst != 0 {
            l.a += 177;
            let n = inst as usize;
            let rec = self.recs[n];
            self.ch[c].inst = inst;
            self.ch[c].am = rec.am;
            if rec.am {
                l.a += 38;
            }
            let h = 0x14 + 30 * (n - 1);
            let start = self.sample_addr(n);
            let ch = &mut self.ch[c];
            ch.start = start;
            ch.length = word(&self.d, h + 22);
            ch.volume = byte(&self.d, h + 25) as u32;
            l.a += 203;
            let rs = word(&self.d, h + 26);
            let rl = word(&self.d, h + 28);
            if rs != 0 {
                ch.loopstart = start.wrapping_add(2 * rs as u32);
                ch.length = rs.wrapping_add(rl);
                l.a += 98;
            } else {
                ch.loopstart = start;
                l.a += 40;
            }
            ch.replen = rl;
            l.a += 110;
            let g = if rec.am { (rec.start >> 2) as u32 } else { 64 };
            let v = ((g * ch.volume) & 0xFFFF) >> 6;
            l.at(hw, 12);
            hw.vol(c, v as u16);
        }
        l.a += 27;
        if per == 0 {
            self.row_commands(c, hw, l);
            return;
        }
        l.a += 21;
        if !self.ch[c].am {
            l.a += 23;
            if self.ch[c].length == 0 {
                l.at(hw, 24);
                hw.dmacon(bit);
                l.a += 10;
                self.row_commands(c, hw, l);
                return;
            }
            l.a += 62;
            let tp = if cmd == 5 {
                true
            } else {
                l.a += 19;
                cmd == 3
            };
            if tp {
                l.a += 69;
                self.ch[c].target = per;
                self.ch[c].dir = 0;
                if per == self.ch[c].period {
                    self.ch[c].target = 0;
                    l.a += 32;
                    return;
                }
                l.a += 11;
                if (per as i16) < (self.ch[c].period as i16) {
                    self.ch[c].dir = 1;
                    l.a += 26;
                }
                self.row_commands(c, hw, l);
                return;
            }
        }
        // note start
        l.a += 36;
        l.at(hw, 24);
        hw.dmacon(bit);
        l.a += 37;
        self.ch[c].period = per;
        self.ch[c].vpos = 0;
        if self.ch[c].am {
            let rec = self.recs[self.ch[c].inst as usize];
            l.a += 172;
            let w = self.wave_addr + 32 * rec.wave.min(14) as u32;
            l.at(hw, 12);
            hw.loc(c, w);
            l.at(hw, 16);
            hw.len(c, 16);
            let ch = &mut self.ch[c];
            ch.loopstart = w;
            ch.replen = 16;
            ch.level = rec.start as i16;
            ch.phase = 0;
            ch.stage = 1;
            let sh = rec.octave & 63;
            ch.period = ((ch.period as u64) << sh) as u16;
            ch.outper = ch.period;
            let p = ch.period;
            l.a += 146;
            l.at(hw, 12);
            hw.per(c, p);
            l.a += 30;
        } else {
            let ch = self.ch[c];
            l.at(hw, 24);
            hw.loc(c, ch.start);
            l.at(hw, 20);
            hw.len(c, ch.length);
            l.at(hw, 20);
            hw.per(c, per);
        }
        self.dmamask |= bit;
        l.a += 42;
        self.row_commands(c, hw, l);
    }

    fn advance(&mut self, l: &mut Led) {
        self.brk = false;
        self.pos = (self.pos + 1) & 0x7F;
        l.a += 113;
        if self.pos == self.songlen {
            self.pos = self.restart;
            l.a += 24;
        }
        l.a += 27;
    }

    fn row_tick(&mut self, hw: &mut Hw, l: &mut Led) {
        l.a += 30 + 351;
        if self.visited(self.pos, self.row) {
            l.a += 278;
            self.songend = true;
        }
        self.visited[(self.pos & 0x7F) as usize] |= 1 << self.row;
        l.a += 120;
        self.dmamask = 0;
        // a position command in an earlier channel does not change where the later cells come from
        let pos = self.pos;
        for c in 0..4 {
            l.a += if c == 0 { 38 } else { 46 };
            self.note(c, pos, hw, l);
        }
        l.wait();
        l.a += 20;
        l.at(hw, 16);
        hw.dmacon(self.dmamask | 0x8000);
        l.wait();
        l.a += 20;
        for c in 0..4 {
            if c > 0 {
                l.a += 8;
            }
            let ch = self.ch[c];
            l.at(hw, 28);
            hw.loc(c, ch.loopstart);
            if c < 3 {
                l.at(hw, 20);
                hw.len(c, ch.replen);
                l.a += 21;
                if !ch.am {
                    l.at(hw, 20);
                    hw.vol(c, ch.volume as u16);
                }
            } else {
                l.a += 21;
                if !ch.am {
                    l.at(hw, 20);
                    hw.len(c, ch.replen);
                    l.at(hw, 20);
                    hw.vol(c, ch.volume as u16);
                }
            }
        }
        l.a += 53;
        self.row += 1;
        if self.row == 64 {
            l.a += 20;
            self.row = 0;
            self.advance(l);
        } else {
            l.a += 27;
            if self.brk {
                l.a += 30;
                self.row = 0;
                self.advance(l);
            }
        }
    }

    // ---- effect tick ----

    fn tone_step(&mut self, c: usize, hw: &mut Hw, l: &mut Led) {
        l.a += 21;
        let ch = &mut self.ch[c];
        if ch.target == 0 {
            l.a += 16;
            return;
        }
        l.a += 37 + 49;
        if ch.dir == 0 {
            ch.period = ch.period.wrapping_add(ch.speed);
            if !((ch.target as i16) > (ch.period as i16)) {
                ch.period = ch.target;
                ch.target = 0;
                l.a += 36;
            }
        } else {
            ch.period = ch.period.wrapping_sub(ch.speed);
            if !((ch.target as i16) < (ch.period as i16)) {
                ch.period = ch.target;
                ch.target = 0;
                l.a += 36;
            }
        }
        ch.outper = ch.period;
        let p = ch.period;
        l.at(hw, 20);
        hw.per(c, p);
        l.a += 36;
    }

    fn vib_step(&mut self, c: usize, hw: &mut Hw, l: &mut Led) {
        l.a += 175;
        let ch = &mut self.ch[c];
        let d = ((SINE[((ch.vpos >> 2) & 31) as usize] as i32 * (ch.vcmd & 15) as i32) >> 7) as u16;
        let v = if ch.vpos & 0x80 != 0 {
            l.a += 4;
            ch.period.wrapping_sub(d)
        } else {
            l.a += 14;
            ch.period.wrapping_add(d)
        };
        ch.outper = v;
        l.at(hw, 12);
        hw.per(c, v);
        let ch = &mut self.ch[c];
        ch.vpos = ch.vpos.wrapping_add((ch.vcmd >> 2) & 0x3C);
        l.a += 74;
    }

    fn vol_slide(&mut self, c: usize, hw: &mut Hw, l: &mut Led) {
        l.a += 39;
        let a = self.ch[c].cell[3];
        let ch = &mut self.ch[c];
        if a >> 4 != 0 {
            l.a += 41;
            ch.volume += (a >> 4) as u32;
            if ch.volume >= 64 {
                ch.volume = 64;
                l.a += 16;
            }
        } else {
            l.a += 45;
            let v = ch.volume as i32 - (a & 15) as i32;
            if v < 0 {
                l.a += 16;
                ch.volume = 0;
            } else {
                ch.volume = v as u32;
            }
        }
        l.a += 21;
        if !ch.am {
            l.a += 16;
            let v = ch.volume as u16;
            l.at(hw, 12);
            hw.vol(c, v);
        }
        l.a += 16;
    }

    fn effect_channel(&mut self, c: usize, hw: &mut Hw, l: &mut Led) {
        let cmd = self.ch[c].cell[2] & 0x0F;
        let a = self.ch[c].cell[3];
        l.a += 29;
        if cmd == 0 && a == 0 {
            let p = self.ch[c].period;
            self.ch[c].outper = p;
            l.at(hw, 20);
            hw.per(c, p);
            l.a += 36;
            return;
        }
        l.a += 33;
        if cmd == 0 {
            l.a += 39;
            let k = self.counter % 3;
            if k == 0 {
                let p = self.ch[c].period;
                self.ch[c].outper = p;
                l.at(hw, 20);
                hw.per(c, p);
                l.a += 36;
                return;
            }
            let x = (if k == 1 { a >> 4 } else { a & 15 }) as usize;
            l.a += if k == 1 { 57 } else { 41 };
            l.a += 40;
            let p = self.ch[c].period & 0xFFF;
            let i = PERIODS.iter().position(|&t| p >= t).unwrap_or(36);
            l.a += 27 * i as u64 + 17;
            let v = self.etable(i + x);
            self.ch[c].outper = v;
            l.at(hw, 22);
            hw.per(c, v);
            l.a += 38;
            return;
        }
        l.a += 19;
        if cmd == 1 {
            l.a += 61;
            let ch = &mut self.ch[c];
            ch.period = ch.period.wrapping_sub(a as u16);
            if (ch.period.wrapping_sub(0x71) as i16) < 0 {
                ch.period = 0x71;
                l.a += 16;
            }
            ch.outper = ch.period;
            let p = ch.period;
            l.at(hw, 20);
            hw.per(c, p);
            l.a += 36;
            return;
        }
        l.a += 19;
        if cmd == 2 {
            l.a += 61;
            let ch = &mut self.ch[c];
            ch.period = ch.period.wrapping_add(a as u16);
            if (ch.period.wrapping_sub(0x358) as i16) >= 0 {
                ch.period = 0x358;
                l.a += 16;
            }
            ch.outper = ch.period;
            let p = ch.period;
            l.at(hw, 20);
            hw.per(c, p);
            l.a += 36;
            return;
        }
        l.a += 17;
        if cmd == 0x0E {
            l.a += 41;
            return;
        }
        l.a += 19;
        if cmd == 3 {
            l.a += 21;
            if a != 0 {
                l.a += 28;
                self.ch[c].speed = a as u16;
                self.ch[c].cell[3] = 0;
            }
            self.tone_step(c, hw, l);
            return;
        }
        l.a += 19;
        if cmd == 4 {
            l.a += 21;
            if a != 0 {
                l.a += 12;
                self.ch[c].vcmd = a;
            }
            self.vib_step(c, hw, l);
            return;
        }
        l.a += 19;
        if cmd == 5 {
            l.a += 18;
            self.tone_step(c, hw, l);
            self.vol_slide(c, hw, l);
            return;
        }
        l.a += 19;
        if cmd == 6 {
            l.a += 18;
            self.vib_step(c, hw, l);
            l.a += 10;
            self.vol_slide(c, hw, l);
            return;
        }
        l.a += 21;
        if !self.ch[c].am {
            let p = self.ch[c].period;
            l.at(hw, 20);
            hw.per(c, p);
        }
        l.a += 19;
        if cmd == 0x0A {
            self.vol_slide(c, hw, l);
        } else {
            l.a += 16;
        }
    }

    fn effect_tick(&mut self, hw: &mut Hw, l: &mut Led) {
        l.a += 18;
        for c in 0..4 {
            l.a += 38;
            self.effect_channel(c, hw, l);
        }
        l.a += 53;
    }

    // ---- AM pass and noise ----

    fn am_channel(&mut self, c: usize, hw: &mut Hw, l: &mut Led) {
        l.a += 23;
        if !self.ch[c].am {
            l.a += 26;
            return;
        }
        l.a += 135;
        if self.ch[c].stage == 0 {
            l.a += 26;
            return;
        }
        let rec = self.recs[self.ch[c].inst as usize];
        let ch = &mut self.ch[c];
        l.a += 25 * ch.stage.min(4) as u64;
        match ch.stage {
            1..=3 => {
                let (target, rate) = match ch.stage {
                    1 => (rec.t1, rec.r1),
                    2 => (rec.t2, rec.r2),
                    _ => (rec.sus, rec.r3),
                };
                l.a += 33;
                let mut next = false;
                if ch.level == target {
                    l.a += 26;
                    next = true;
                } else {
                    l.a += 21 + 52 + if ch.stage == 3 { 9 } else { 11 };
                    let rising = ch.level < target;
                    if rising {
                        ch.level = ch.level.wrapping_add(rate);
                    } else {
                        ch.level = ch.level.wrapping_sub(rate);
                    }
                    let reached = if rising { ch.level >= target } else { ch.level <= target };
                    if reached {
                        ch.level = target;
                        l.a += 46;
                        if rising {
                            l.a += 10;
                        }
                        next = true;
                    }
                }
                if next {
                    ch.stage += 1;
                    if ch.stage == 4 {
                        ch.hold = rec.hold;
                        l.a += 20;
                    }
                }
            }
            4 => {
                l.a += 25;
                ch.hold = ch.hold.wrapping_sub(1);
                if ch.hold < 0 {
                    ch.stage = 5;
                    l.a += 26;
                }
            }
            _ => {
                l.a += 37;
                ch.level = ch.level.wrapping_sub(rec.release);
                if ch.level < 0 {
                    ch.am = false;
                    ch.level = 0;
                    ch.stage = 0;
                    l.a += 40;
                    l.at(hw, 24);
                    hw.dmacon(1 << c);
                }
            }
        }
        let ch = &mut self.ch[c];
        l.a += 106;
        let v = ((((ch.level as u16) >> 2) as u32 * ch.volume) & 0xFFFF) >> 6;
        l.at(hw, 12);
        hw.vol(c, v as u16);
        let ch = &mut self.ch[c];
        l.a += 65;
        ch.outper = ch.outper.wrapping_add(rec.fall as u16);
        ch.period = ch.period.wrapping_add(rec.fall as u16);
        let mut vib: i16 = 0;
        if rec.vibdepth != 0 {
            l.a += 33;
            let mut p = ch.phase as i32;
            let mut neg = false;
            if p >= 360 {
                l.a += 12;
                p -= 360;
                neg = true;
            }
            l.a += 109;
            let x = (rec.vibdepth as i32).wrapping_mul(amvib(p / 2) as i32) as i16;
            vib = x >> 7;
            if neg {
                vib = vib.wrapping_neg();
                l.a += 4;
            }
        }
        l.a += 12;
        let p = ch.outper.wrapping_add(vib as u16);
        l.at(hw, 12);
        hw.per(c, p);
        let ch = &mut self.ch[c];
        l.a += 57;
        ch.phase = ch.phase.wrapping_add(rec.vibspeed.wrapping_mul(2));
        if ch.phase >= 720 {
            ch.phase -= 720;
            l.a += 20;
        }
        l.a += 26;
    }

    fn am_pass(&mut self, hw: &mut Hw, l: &mut Led, effect: bool) {
        l.a += 28;
        for c in 0..4 {
            self.am_channel(c, hw, l);
        }
        // the noise generator, byte by byte against the beam (after an effect tick it starts 158
        // cycles later than the specification's ledger says: measured on the reference's noise
        // fetches, where the clock of every register write is already exact)
        l.a += 24 + if effect { 158 } else { 0 };
        let mut n: u16 = 0x7327;
        for k in 0..32u32 {
            l.a += 8;
            self.noise[k as usize] = n as u8;
            let t = self.origin + l.a / 2;
            // Paula's fetches before this store see the old byte
            hw.bus.paula.sync(t, &hw.bus.ram);
            hw.bus.w8(self.wave_addr + 96 + k, n as u8);
            l.a += 16;
            let h = ((l.a / 2) % LINE) as u16;
            n = (n & 0xFF00) | ((n as u8).wrapping_add(h as u8) as u16);
            n ^= 0x7C;
            n = n.rotate_left(3);
            l.a += 30;
        }
        l.a += 16;
    }

    fn run(&mut self, hw: &mut Hw) {
        let origin = self.origin;
        let mut l = Led { origin, a: 2 * hw.bus.clock.saturating_sub(origin) };
        l.a += 267;
        self.counter = self.counter.wrapping_add(1);
        let effect = self.counter < self.speed;
        if effect {
            self.effect_tick(hw, &mut l);
        } else {
            self.counter = 0;
            self.row_tick(hw, &mut l);
        }
        self.am_pass(hw, &mut l, effect);
        l.a += 148;
        hw.bus.clock = origin + l.a / 2;
    }
}

impl Replayer for AudioSculpture {
    fn format(&self) -> &'static str {
        "AudioSculpture"
    }

    fn title(&self) -> String {
        let t = &self.d[..20.min(self.d.len())];
        let end = t.iter().position(|&b| b == 0).unwrap_or(t.len());
        t[..end].iter().map(|&b| if (32..127).contains(&b) { b as char } else { '?' }).collect()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subs.len() as u16, self.sub)
    }

    /// The host stops the player and starts it again for each step of the subsong number: a stop
    /// (volumes, DMA off) 370 colour clocks after the last LED write, then the initialisation.
    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let s = self.led + STOP_AT;
        for v in 0..4u64 {
            hw.bus.clock = self.origin + s + 10 * v;
            hw.vol(v as usize, 0);
        }
        hw.bus.clock = self.origin + s + 40;
        hw.dmacon(0x000F);
        self.init(hw, s + 40 + STOP_TO_INIT, n);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        Some(self.origin + self.led + FIRST_TICK)
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.run(hw);
    }
}

/// The companion's name: the module's file name with `.AS` appended, else `.NT`.
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    vec![format!("{file}.AS"), format!("{file}.NT")]
}

/// Whether `d` is an AudioSculpture module with its companion beside it (for a `FLT4` module,
/// which the built-in ProTracker would otherwise take as a StarTrekker one).
pub fn takes_over(m: &Machine, d: &[u8]) -> bool {
    detect(d) && read_companion(m).is_some()
}

fn read_companion(m: &Machine) -> Option<Vec<u8>> {
    let file = m.bus.read_cstr(m.deli.file_buf);
    companion(&file).iter().find_map(|name| crate::deli::read_companion(m, name).1.ok())
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let comp = read_companion(m)?;
    // the four waveforms, then the vibrato table, in the player's own memory; the last 32 bytes
    // of the waveform area are the noise buffer
    let wave = m.alloc(128 + 360);
    if wave == 0 {
        return None;
    }
    let mut w = [0u8; 128];
    for i in 0..32 {
        w[i] = (127.0 * (2.0 * std::f64::consts::PI * i as f64 / 32.0).sin()).round() as i8 as u8;
        w[32 + i] = 0x80u8.wrapping_add(8 * i as u8);
        w[64 + i] = if i < 16 { 0x80 } else { 0x7F };
    }
    m.bus.write_bytes(wave, &w);
    m.bus.write_bytes(wave + 128, AMVIB_BYTES);
    let mut r = AudioSculpture::build(d, &comp, base, wave)?;
    // the reference's start-up counts from the run's origin, START_PHASE colour clocks before the
    // engine's aligned start; the first tick follows one period after the clock left here
    r.origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut hw = Hw { bus: &mut m.bus };
    r.init(&mut hw, INIT_AT, 0);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "audiosculpture_tests.rs"]
mod tests;
