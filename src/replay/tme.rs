//! The Musical Enlightenment (TME), by N.J. Luuring jr. (1989-90).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the format's two 68k
//! players' observable behaviour, and this player was written from it alone and
//! checked against the Eagleplayer 2.06 TME's Paula register writes.
//!
//! A module has up to 16 tunes (subsongs), a table of 12-byte entries (a track,
//! an instrument add and a note add for each of four voices), a pool of 32-row
//! tracks of 6-byte events, 256 instruction lists (vibrato and tremolo ramps,
//! delays, jumps), arpeggios, LFO tables and 32 instruments with a three-segment
//! envelope. The replayer is a plain 50 Hz tick routine that writes Paula's
//! registers; the samples are played from the module in guest memory. Everything
//! the module addresses is read from the file image as flat memory (a read past
//! its end gives 0), as the original does.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "The Musical Enlightenment", detect, new };

const HEADER: usize = 6826;
const INS: usize = 68;
const ARP: usize = 4164;
const LFO: usize = 4740;

const PERIODS: [u16; 39] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
    226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 1, 2, 1,
];

/// The table of an index past the standard three octaves: the player image's
/// three words that follow, and beyond those 1: the original's words there are below 113 (its
/// clamp to 113 runs, as its timing shows), their values unknown.
fn period_of(i: u8) -> u16 {
    PERIODS.get(i as usize).copied().unwrap_or(1)
}

/// The size a module claims (the header, its entries, events, instruction lists
/// and the samples of the named instruments), or `None` if its lists run past
/// the end of the file.
fn claimed_size(d: &[u8]) -> Option<usize> {
    let n = be16(d, 6788) as usize;
    let e = be16(d, 6790) as usize;
    let mut total = HEADER + 12 * n + 6 * e;
    if total > d.len() {
        return None;
    }
    for _ in 0..256 {
        loop {
            let b = *d.get(total)?;
            total += 4;
            if total > d.len() {
                return None;
            }
            if b == 0 {
                break;
            }
        }
    }
    for i in 0..32 {
        let o = INS + 128 * i;
        if d.get(o + 24).copied()? != 0 {
            total += be32(d, o + 4) as usize;
        }
    }
    Some(total)
}

fn detect(d: &[u8]) -> bool {
    if d.len() < HEADER {
        return false;
    }
    // at least one tune must play some of the entries, which a module of
    // another format that happens to fit the size check seldom has
    let n = be16(d, 6788) as usize;
    let tune = (0..16).any(|t| {
        let (s, e) = (d[4 + 4 * t] as usize, d[5 + 4 * t] as usize);
        s < e && e <= n
    });
    tune && claimed_size(d).is_some_and(|t| t + 64 >= d.len() && t <= d.len() + 64)
}

/// The original's code paths (68000 cycles each, `K::cycles`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    Entry, NotPlaying, On, NoOn, AfterOn, Row, NoRow, RowDec, Load, LoadWrap, VSkip, VEmpty, VOff, VRel, VNote,
    VAdd, VNoAdd, VZero, VGlideAdd, VNoGlideAdd, VGlideTo, VGlideNone, VArp, VNoFx, VRestart, VInst, VInstAdd,
    VInstFixed, VNoInst, VFx, VInsList, VNoRestart, VBase, LSkip, LDelay, LRun, LMod, LSpeed, LWait, LJump, LStop,
    SBase, SGlide, SArp, SArpStep, SArpOnce, SArpBlow, SArpCount, SPlain, ModOff, ModOn, ModL, ModS, PerClamp,
    PerOk, EnvRamp, EnvNext, EnvHold, VolClamp, VolOk, Start2, Start1, Start0, WSkip, WLoc, WLen, WPer, WVol, WLoc1,
    WLen1, WPer1, WVol1, WLoc2, WLen2, WPer2, WVol2, WLoc3, WLen3, WPer3, WVol3, VOffRet, WVol64, VArpLen8,
    VArpOnceF, VArpBlowF, ModOffT, ModOnT, ModLT, ModST, LModT, LModX, LSpeedT, LSpeedX,
}

impl K {
    fn cycles(self) -> u64 {
        COSTS[self as usize]
    }
}

/// In the order of `K`, fitted exactly on the write gaps (`NotPlaying`, `VGlideNone`, `LSpeedX`: not
/// seen, 0).
const COSTS: [u64; 93] = [
    0, 0, 266, 240, 202, 0, 3, 272, 863, 130, 147, 160, 151, 224, 0, 12, 0, 244, 16, 0, 125, 0, 326, 0, 583, 109,
    16, 0, 0, 47, 3, 0, 524, 84, 115, 221, 216, 154, 28, 146, 0, 0, 233, 129, 227, 0, 286, 0, 0, 585, 839, 206, 218,
    9, 1, 219, 118, 0, 12, 0, 107, 141, 0, 53, 303, 202, 186, 335, 303, 202, 186, 358, 303, 202, 186, 381, 303, 202,
    186, 381, 238, 4, 12, 16, 0, 1, 255, 206, 218, 376, 74, 0, 0,
];

/// The start-up, fitted to the reference's clocks on the 8 modules and modified copies: the
/// first write (ADKCON) at `INIT_BASE` colour clocks plus 68000 cycles per instruction-list unit
/// and per named instrument, halved; the first DMACON that many cycles after it again
/// (`SCAN_*`, counted on from the first write), the second and third (the tune's start) 208 and
/// 601 cycles on, and the song's start 1168 after it (130 more when the tune is one entry long
/// and its table wraps at once).
const INIT_BASE: u64 = 8863;
const INIT_UNIT: u64 = 105;
const INIT_NAMED: u64 = 40;
const SCAN_BASE: u64 = 17556;
const SCAN_UNIT: u64 = 31;
const SCAN_NAMED: u64 = 38;
const C_STOP2: u64 = 208;
const C_STOP3: u64 = 601;
const C_START: u64 = 1168;
const C_START_WRAP: u64 = 130;

/// The trace of code paths for fitting their cycles (TME_TRACE=file): each write's clock and
/// the paths since the previous one.
#[derive(Default)]
struct Trace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    pend: Vec<K>,
}

impl Trace {
    fn open() -> Self {
        let out =
            std::env::var_os("TME_TRACE").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new);
        Trace { out, pend: Vec::new() }
    }

    fn write(&mut self, clock: u64, what: &str) {
        if let Some(o) = &mut self.out {
            use std::io::Write;
            let _ = write!(o, "{} {}", clock, what);
            for e in self.pend.drain(..) {
                let _ = write!(o, " {:?}", e);
            }
            let _ = writeln!(o);
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Modulator {
    tlevel: u8,
    level: u8,
    tspeed: u8,
    speed: u8,
    phase: u8,
    lramp: u16,
    sramp: u16,
    lfo: u8,
}

impl Modulator {
    /// Adds the LFO, scaled by the ramped level, to `src` (`shift` 7 for the
    /// period, 9 for the volume).
    fn eval(&mut self, d: &[u8], src: u16, shift: u32, ks: &mut Vec<K>) -> u16 {
        if self.tlevel == 0 {
            ks.push(if shift == 9 { K::ModOffT } else { K::ModOff });
            return src;
        }
        ks.push(if shift == 9 { K::ModOnT } else { K::ModOn });
        if self.lramp != 0 {
            ks.push(if shift == 9 { K::ModLT } else { K::ModL });
            let diff = self.tlevel.wrapping_sub(self.level) as i8 as i32;
            self.level = self.level.wrapping_add((diff / self.lramp as i16 as i32) as u8);
            self.lramp -= 1;
        }
        if self.sramp != 0 {
            ks.push(if shift == 9 { K::ModST } else { K::ModS });
            let diff = self.tspeed.wrapping_sub(self.speed) as i8 as i32;
            self.speed = self.speed.wrapping_add((diff / self.sramp as i16 as i32) as u8);
            self.sramp -= 1;
        }
        let v = d.get(LFO + 128 * self.lfo as usize + self.phase as usize).copied().unwrap_or(0) as i8 as i32;
        let product = v * self.level as i32;
        self.phase = self.phase.wrapping_add(self.speed) & 127;
        src.wrapping_add((product >> shift) as u16)
    }
}

#[derive(Clone, Default)]
struct Voice {
    // shadow registers
    start: u32,
    len: u16,
    period: u16,
    vol: u16,
    /// Offset of the instrument record.
    inst: Option<usize>,
    ev: usize,
    adds: usize,
    /// Offset of the next instruction unit; `usize::MAX` for a list that is only a stop.
    ip: usize,
    waiting: bool,
    delay: u16,
    /// Vibrato, tremolo, special.
    m: [Modulator; 3],
    src_period: u16,
    sound_start: u8,
    env_phase: u8,
    env_cnt: u16,
    env_level: u16,
    env_target: u16,
    base_vol: i16,
    glide_add: i16,
    sg_count: u32,
    sg_target: u16,
    skip: bool,
    arp: usize,
    arp_len: u8,
    arp_speed: u8,
    arp_cd: u8,
    arp_pos: u8,
    arp_left: u8,
    arp_once: bool,
    arp_blow: bool,
    base_note: u8,
}

pub struct Tme {
    d: Vec<u8>,
    title: String,
    entries: usize,
    lists: [usize; 256],
    /// Absolute address of the sample of each instrument slot.
    sample: [u32; 32],
    tunes: u16,
    tune: u16,
    first: usize,
    end: usize,
    table: usize,
    mask: u8,
    speed: u8,
    clock: u8,
    rows_left: u16,
    pending: u16,
    playing: bool,
    songend: bool,
    v: [Voice; 4],
    /// What a voice with no instrument yet reads as its instrument record: the original's
    /// record pointer is 0, so it reads the machine's first 128 bytes (the exception vectors).
    low: [u8; 128],
    /// The clock the current stretch of the original's time counts from, and its cycles since.
    t0: u64,
    cy: u64,
    ks: Vec<K>,
    trace: Trace,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let entries = be16(d, 6788) as usize;
    let events = be16(d, 6790) as usize;
    // the lists, then the samples after them
    let mut lists = [0usize; 256];
    let mut p = HEADER + 12 * entries + 6 * events;
    let mut units = 0u64;
    for l in lists.iter_mut() {
        *l = p;
        loop {
            let b = d.get(p).copied().unwrap_or(0);
            p += 4;
            units += 1;
            if b == 0 {
                break;
            }
        }
    }
    let mut sample = [0u32; 32];
    for (i, s) in sample.iter_mut().enumerate() {
        let o = INS + 128 * i;
        if d[o + 24] != 0 {
            *s = base.wrapping_add(p as u32);
            p += be32(d, o + 4) as usize;
        } else {
            *s = be32(d, o);
        }
    }
    let name = d.get(6794..6826).unwrap_or(&[]);
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    let title = name[..end].iter().map(|&c| c as char).collect::<String>().trim_end().to_string();
    let tunes = (0..16).take_while(|&t| d[4 + 4 * t] != 0 || d[5 + 4 * t] != 0).count() as u16;
    let mut t = Tme {
        d: d.to_vec(),
        title,
        entries,
        lists,
        sample,
        tunes,
        tune: 0,
        first: 0,
        end: 0,
        table: 0,
        mask: 0,
        speed: 0,
        clock: 0,
        rows_left: 0,
        pending: 0,
        playing: false,
        songend: false,
        v: Default::default(),
        low: std::array::from_fn(|a| m.bus.r8(a as u32)),
        t0: 0,
        cy: 0,
        ks: Vec::new(),
        trace: Trace::open(),
    };
    // InitPlayer's and InitSound's writes at the original's clocks: the host's start-up and the
    // module check (its walk of the instruction lists and the named instruments' samples), the
    // walk again for the samples' addresses, then the stop and the tune's start
    let named = (0..32).filter(|&i| d[INS + 128 * i + 24] != 0).count() as u64;
    let first = INIT_BASE + (INIT_UNIT * units + INIT_NAMED * named) / 2;
    let mut hw = Hw { bus: &mut m.bus };
    hw.bus.clock = hw.bus.clock.max(first);
    hw.bus.write_word(0xDFF09E, 0x00FF);
    // from here on the original's cycles run on without a host call: each write at half of them
    let mut cy = SCAN_BASE + SCAN_UNIT * units + SCAN_NAMED * named;
    hw.bus.clock = first + cy / 2;
    hw.dmacon(0x000F);
    cy += C_STOP2;
    hw.bus.clock = first + cy / 2;
    hw.dmacon(0x000F);
    cy += C_STOP3;
    hw.bus.clock = first + cy / 2;
    t.start(0, &mut hw);
    cy += C_START + if t.songend { C_START_WRAP } else { 0 };
    hw.bus.clock = first + cy / 2;
    Some(Box::new(t))
}

impl Tme {
    fn k(&mut self, e: K) {
        self.cy += e.cycles();
        if self.trace.out.is_some() {
            self.trace.pend.push(e);
        }
    }

    fn flush_ks(&mut self) {
        for e in std::mem::take(&mut self.ks) {
            self.k(e);
        }
    }

    /// The clock of the next write: the previous write's (or the stretch's start) plus half the
    /// cycles since, rounded down.
    fn at(&mut self, hw: &mut Hw, what: &str) {
        self.flush_ks();
        let t = self.t0 + self.cy / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
        self.trace.write(hw.bus.clock, what);
        // each write ends a stretch of the original's time: the next counts from it
        self.t0 = hw.bus.clock;
        self.cy = 0;
    }

    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// The stop, then the start of tune `n`, as InitSound and the play call do;
    /// the voices' state is not reset.
    fn start(&mut self, n: u16, hw: &mut Hw) {
        hw.dmacon(0x000F);
        self.pending = 0;
        self.playing = false;
        self.songend = false;
        self.tune = n;
        let rec = 4 + 4 * n as usize;
        let (s, e) = (self.rb(rec), self.rb(rec + 1));
        if s == 0 && e == 0 {
            return;
        }
        self.first = s as usize;
        self.end = e as usize;
        self.speed = self.rb(rec + 2);
        self.mask = self.rb(rec + 3) & 0x0F;
        for v in self.v.iter_mut() {
            v.env_target = 0;
        }
        self.table = self.first;
        self.load_entry();
        self.clock = 0;
        self.rows_left = 33;
        self.playing = true;
    }

    fn load_entry(&mut self) {
        let ev0 = HEADER + 12 * self.entries;
        let e = HEADER + 12 * self.table;
        for i in 0..4 {
            let g = e + 3 * i;
            self.v[i].ev = ev0 + 192 * self.rb(g) as usize;
            self.v[i].adds = g + 1;
        }
        self.rows_left = 32;
        self.table += 1;
        if self.table == self.end {
            self.table = self.first;
            self.songend = true;
        }
    }

    fn inst(&self, v: usize, off: usize) -> u8 {
        self.v[v].inst.map_or(self.low[off], |i| self.rb(i + off))
    }

    fn inst16(&self, v: usize, off: usize) -> u16 {
        self.v[v].inst.map_or(be16(&self.low, off), |i| be16(&self.d, i + off))
    }

    fn inst32(&self, v: usize, off: usize) -> u32 {
        self.v[v].inst.map_or(be32(&self.low, off), |i| be32(&self.d, i + off))
    }

    fn row(&mut self, hw: &mut Hw) {
        self.k(K::RowDec);
        self.rows_left -= 1;
        if self.rows_left == 0 {
            self.k(K::Load);
            self.load_entry();
            if self.table == self.first {
                self.k(K::LoadWrap);
            }
        }
        for v in 0..4 {
            self.voice_row(v, hw);
        }
    }

    fn voice_row(&mut self, v: usize, hw: &mut Hw) {
        let e = self.v[v].ev;
        self.v[v].ev += 6;
        if self.v[v].skip {
            self.k(K::VSkip);
            self.v[v].skip = false;
            return;
        }
        let note = self.rb(e);
        let n = note & 127;
        let bit = 1u16 << v;
        if n == 0 {
            self.k(K::VEmpty);
            return;
        }
        if n == 127 {
            self.k(K::VOff);
            self.at(hw, "off");
            hw.dmacon(bit & self.mask as u16);
            self.k(K::VOffRet);
            return;
        }
        if n == 126 {
            self.k(K::VRel);
            self.v[v].env_phase = 2;
            return;
        }
        self.k(K::VNote);
        let flags = self.rb(e + 4);
        let par = self.rb(e + 5);
        let adds = self.v[v].adds;
        let note_add = self.rb(adds + 1);
        self.k(if flags & 4 == 0 { K::VAdd } else { K::VNoAdd });
        let sum = if flags & 4 == 0 { n.wrapping_add(note_add) } else { n };
        if sum == 0 {
            self.k(K::VZero);
            return;
        }
        let idx = sum - 1;
        self.v[v].base_note = idx;
        self.v[v].src_period = period_of(idx);
        // simple glide
        self.k(if flags & 2 != 0 { K::VGlideAdd } else { K::VNoGlideAdd });
        self.v[v].glide_add = if flags & 2 != 0 { par as i8 as i16 } else { 0 };
        // glide or arpeggio
        self.v[v].arp_left = 0;
        let mut glide = false;
        if flags & 0x10 != 0 {
            let nn = self.rb(e + 6) & 127;
            let s = if flags & 4 == 0 { nn.wrapping_add(note_add) } else { nn };
            self.k(if s != 0 { K::VGlideTo } else { K::VGlideNone });
            if s != 0 {
                self.v[v].sg_target = period_of(s - 1);
                self.v[v].sg_count = par as u32 + 1;
                self.v[v].skip = true;
                glide = true;
            }
        } else if flags & 1 != 0 {
            self.k(K::VArp);
            let a = ARP + 9 * par as usize;
            let h = self.rb(a);
            let w = &mut self.v[v];
            w.arp = a;
            w.arp_speed = h & 15;
            w.arp_len = if h >> 4 == 0 { 8 } else { h >> 4 };
            w.arp_once = flags & 0x20 != 0;
            w.arp_blow = flags & 0x40 != 0;
            if h >> 4 == 0 {
                self.ks.push(K::VArpLen8);
            }
            if flags & 0x20 != 0 {
                self.ks.push(K::VArpOnceF);
            }
            if flags & 0x40 != 0 {
                self.ks.push(K::VArpBlowF);
            }
            w.arp_cd = w.arp_speed.wrapping_add(1);
            w.arp_pos = 0;
            w.arp_left = w.arp_len.wrapping_sub(1);
        } else {
            self.k(K::VNoFx);
        }
        if !glide {
            self.v[v].sg_count = 0;
        }
        // restart
        if note & 0x80 == 0 {
            self.k(K::VRestart);
            let w = &mut self.v[v];
            w.env_phase = 0;
            w.env_level = 0;
            w.env_cnt = 0;
            w.sound_start = 2;
            let s = self.d.get(e + 1).copied().unwrap_or(0);
            if s != 0 {
                self.k(K::VInst);
                let mut num = s - 1;
                self.k(if flags & 8 == 0 { K::VInstAdd } else { K::VInstFixed });
                if flags & 8 == 0 {
                    num = num.wrapping_add(self.rb(adds)) & 0x1F;
                }
                self.v[v].inst = Some(INS + 128 * num as usize);
            } else {
                self.k(K::VNoInst);
            }
            self.at(hw, "off");
            hw.dmacon(bit & self.mask as u16);
            self.pending |= bit & self.mask as u16;
            let fx = self.rb(e + 2);
            self.k(if fx != 0 { K::VFx } else { K::VInsList });
            self.v[v].ip = if fx != 0 {
                self.lists[fx as usize]
            } else {
                self.v[v].inst.map_or(usize::MAX, |i| i + 64)
            };
            let w = &mut self.v[v];
            for m in w.m.iter_mut() {
                m.level = 0;
                m.speed = 0;
                m.phase = 0;
            }
            w.waiting = true;
            w.delay = 0;
        } else {
            self.k(K::VNoRestart);
        }
        self.k(K::VBase);
        // base volume
        let bv = self.inst(v, 56).wrapping_add(self.rb(e + 3));
        self.v[v].base_vol = bv as i8 as i16;
    }

    /// The instruction lists of the voices that are waiting on effects.
    fn run_lists(&mut self) {
        for v in 0..4 {
            if !self.v[v].waiting {
                self.k(K::LSkip);
                continue;
            }
            if self.v[v].delay != 0 {
                self.k(K::LDelay);
                self.v[v].delay -= 1;
                continue;
            }
            self.k(K::LRun);
            // a list that jumps to itself would never end
            for _ in 0..4096 {
                let ip = self.v[v].ip;
                let (b0, val, time) = if ip == usize::MAX {
                    (0, 0, 0)
                } else {
                    (self.rb(ip), self.rb(ip + 1), be16(&self.d, ip + 2))
                };
                self.k(match b0 >> 4 {
                    1 => K::LMod,
                    3 => K::LModT,
                    5 => K::LModX,
                    2 => K::LSpeed,
                    4 => K::LSpeedT,
                    6 => K::LSpeedX,
                    7 => K::LWait,
                    8 => K::LJump,
                    _ => K::LStop,
                });
                let w = &mut self.v[v];
                match b0 >> 4 {
                    op @ (1 | 3 | 5) => {
                        let m = &mut w.m[(op as usize - 1) / 2];
                        m.tlevel = val;
                        m.lramp = time.wrapping_add(1);
                        m.lfo = b0 & 15;
                        w.ip += 4;
                    }
                    op @ (2 | 4 | 6) => {
                        let m = &mut w.m[(op as usize - 2) / 2];
                        m.tspeed = val;
                        m.sramp = time.wrapping_add(1);
                        w.ip += 4;
                    }
                    7 => {
                        w.delay = time;
                        w.ip += 4;
                        break;
                    }
                    8 => w.ip = self.lists[val as usize],
                    _ => {
                        w.waiting = false;
                        break;
                    }
                }
            }
        }
    }

    /// The new shadow registers of voice `v`.
    fn sound(&mut self, v: usize, hw: &mut Hw) {
        // source period
        self.k(K::SBase);
        {
            let w = &mut self.v[v];
            w.src_period = w.src_period.wrapping_add(w.glide_add as u16);
            if w.sg_count != 0 {
                self.ks.push(K::SGlide);
                let diff = w.sg_target as i32 - w.src_period as i32;
                w.src_period = w.src_period.wrapping_add((diff / w.sg_count as u16 as i16 as i32) as u16);
                w.sg_count -= 1;
            } else if w.arp_left != 0 {
                self.ks.push(K::SArp);
                let mut blow = false;
                if w.arp_cd == 0 {
                    self.ks.push(K::SArpStep);
                    w.arp_pos = (w.arp_pos + 1) % w.arp_len;
                    w.arp_cd = w.arp_speed;
                    if w.arp_once {
                        self.ks.push(K::SArpOnce);
                        w.arp_left -= 1;
                    }
                    if w.arp_blow {
                        self.ks.push(K::SArpBlow);
                        w.sound_start = 2;
                        w.env_phase = 0;
                        w.env_cnt = 0;
                        w.env_level = 0;
                        blow = true;
                    }
                } else {
                    self.ks.push(K::SArpCount);
                    w.arp_cd -= 1;
                }
                if blow {
                    self.flush_ks();
                    let bit = 1u16 << v;
                    self.at(hw, "off");
                    hw.dmacon(bit & self.mask as u16);
                    self.pending |= bit & self.mask as u16;
                }
                let w = &self.v[v];
                let off = self.rb(w.arp + 1 + w.arp_pos as usize);
                self.v[v].src_period = period_of(w.base_note.wrapping_add(off));
            } else {
                self.ks.push(K::SPlain);
            }
        }
        // vibrato
        let src = self.v[v].src_period;
        let p = self.v[v].m[0].eval(&self.d, src, 7, &mut self.ks) as i16;
        self.ks.push(if p < 113 { K::PerClamp } else { K::PerOk });
        self.v[v].period = if p < 113 { 113 } else { p as u16 };
        // envelope
        let (rate, lvl) = (
            |s: &Self, ph: u8| s.inst(v, 58 + ph as usize),
            |s: &Self, ph: u8| s.inst(v, 61 + ph as usize),
        );
        let phase = self.v[v].env_phase;
        if self.v[v].env_cnt != 0 {
            self.ks.push(K::EnvRamp);
            let w = &mut self.v[v];
            let diff = w.env_target as i32 - w.env_level as i32;
            w.env_level = (w.env_level as i32 + diff / w.env_cnt as i32) as u16;
            w.env_cnt -= 1;
        } else if phase != 3 {
            self.ks.push(K::EnvNext);
            let (r, l) = (rate(self, phase), lvl(self, phase));
            let w = &mut self.v[v];
            w.env_cnt = r as u16 + 1;
            w.env_target = l as u16;
            w.env_phase = if phase == 0 { 1 } else { 3 };
        } else {
            self.ks.push(K::EnvHold);
        }
        // volume
        let w = &self.v[v];
        let src = w.base_vol.wrapping_mul(w.env_level as i16) >> 8;
        let t = self.v[v].m[1].eval(&self.d, src as u16, 9, &mut self.ks) as i16;
        self.ks.push(if t > 64 { K::VolClamp } else { K::VolOk });
        self.v[v].vol = if t > 64 { 64 } else { t as u16 };
        // start and loop
        let (start, first, loop_off, loop_len) = (
            self.v[v].inst.map_or(be32(&self.low, 0), |i| self.sample_addr(i)),
            self.inst16(v, 10),
            self.inst32(v, 12),
            self.inst16(v, 18),
        );
        self.ks.push([K::Start0, K::Start1, K::Start2][self.v[v].sound_start.min(2) as usize]);
        self.flush_ks();
        let w = &mut self.v[v];
        match w.sound_start {
            2 => {
                w.start = start;
                w.len = first;
                w.sound_start = 1;
            }
            1 => {
                w.start = start.wrapping_add(loop_off.wrapping_mul(2));
                w.len = loop_len;
                w.sound_start = 0;
            }
            _ => {}
        }
    }

    /// Where the sample of the instrument record at `i` lies in guest memory.
    fn sample_addr(&self, i: usize) -> u32 {
        let slot = (i - INS) / 128;
        self.sample.get(slot).copied().unwrap_or_else(|| be32(&self.d, i))
    }
}

impl Replayer for Tme {
    fn format(&self) -> &'static str {
        "The Musical Enlightenment"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.tunes.saturating_sub(1), self.tune)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.start(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 0;
        self.trace.write(self.t0, "tick");
        self.k(K::Entry);
        if !self.playing {
            self.k(K::NotPlaying);
            return;
        }
        if self.pending != 0 {
            self.k(K::On);
            self.at(hw, "on");
            hw.dmacon(0x8000 | self.pending);
            self.pending = 0;
            // the wait for the DMA: to the start of the 10th raster line after the write
            hw.wait_lines(10, 0);
            self.t0 = hw.bus.clock;
            self.cy = 0;
            self.k(K::AfterOn);
        } else {
            self.k(K::NoOn);
        }
        if self.clock == 0 {
            self.k(K::Row);
            self.clock = self.speed;
            self.row(hw);
        } else {
            self.k(K::NoRow);
            self.clock -= 1;
        }
        self.run_lists();
        for v in 0..4 {
            self.sound(v, hw);
        }
        for v in 0..4 {
            if self.mask & 1 << v == 0 {
                self.k(K::WSkip);
                continue;
            }
            let w = &self.v[v];
            let (s, l, p, vol) = (w.start, w.len, w.period, w.vol);
            self.k([K::WLoc, K::WLoc1, K::WLoc2, K::WLoc3][v]);
            self.at(hw, "loc");
            hw.loc(v, s);
            self.k([K::WLen, K::WLen1, K::WLen2, K::WLen3][v]);
            self.at(hw, "len");
            hw.len(v, l);
            self.k([K::WPer, K::WPer1, K::WPer2, K::WPer3][v]);
            self.at(hw, "per");
            hw.per(v, p);
            // Paula takes the low 7 bits of a volume, 64 and over as 64
            let vol = vol & 0x7F;
            self.k([K::WVol, K::WVol1, K::WVol2, K::WVol3][v]);
            if vol >= 64 {
                self.k(K::WVol64);
            }
            self.at(hw, "vol");
            hw.vol(v, vol.min(64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A synthetic module: one tune of one entry, one named instrument, a note
    /// on voice 0.
    fn module() -> Vec<u8> {
        let (n, e) = (1usize, 64usize);
        let mut d = vec![0u8; HEADER + 12 * n + 6 * e + 256 * 4];
        d[4..8].copy_from_slice(&[0, 1, 1, 31]);
        for t in 1..16 {
            d[4 + 4 * t..8 + 4 * t].copy_from_slice(&[0, 0, 5, 15]);
        }
        let o = INS;
        d[o + 4..o + 8].copy_from_slice(&200u32.to_be_bytes());
        d[o + 8..o + 12].copy_from_slice(&100u32.to_be_bytes());
        d[o + 16..o + 20].copy_from_slice(&1u32.to_be_bytes());
        d[o + 24] = b'a';
        d[o + 56] = 0x40;
        d[o + 61] = 255;
        d[o + 62] = 255;
        d[6788..6790].copy_from_slice(&(n as u16).to_be_bytes());
        d[6790..6792].copy_from_slice(&(e as u16).to_be_bytes());
        // voices 1 to 3 play the empty track 1
        for v in 1..4 {
            d[HEADER + 3 * v] = 1;
        }
        // track 0, row 0: note 13 with instrument 1
        let ev = HEADER + 12 * n;
        d[ev] = 13;
        d[ev + 1] = 1;
        // the lists are 256 terminators
        d.resize(d.len() + 200, 0x20);
        d
    }

    #[test]
    fn recognises_by_claimed_size() {
        let d = module();
        assert!(detect(&d));
        assert!(detect(&d[..d.len() - 60]));
        assert!(!detect(&d[..d.len() - 70]));
        let mut longer = d.clone();
        longer.resize(d.len() + 65, 0);
        assert!(!detect(&longer));
        assert!(!detect(&d[..HEADER - 1]));
        let mut notune = d.clone();
        notune[4..8].copy_from_slice(&[0, 0, 5, 15]);
        assert!(!detect(&notune));
        assert!(!detect(b"FORM\0\0\0\0EMODEMIC"));
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.tme"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (format, log.iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note_with_its_envelope_and_loop() {
        let (format, log) = play(module(), 6);
        assert_eq!(format, "The Musical Enlightenment");
        // the stop at the start, then voice 0's DMA off for its note, then on
        let dma = reg(&log, 0xDFF096);
        assert_eq!(&dma[..5], &[0x000f, 0x000f, 0x000f, 0x0001, 0x8001]);
        // the first tick writes the first-play region (100 words), the next the loop (1 word)
        let len = reg(&log, 0xDFF0A4);
        assert_eq!(&len[..2], &[100, 1]);
        // note 13 is index 12: period 428; volume 0 on the restart tick, then 64 * 255 / 256 = 63
        assert_eq!(&reg(&log, 0xDFF0A6)[..2], &[428, 428]);
        assert_eq!(&reg(&log, 0xDFF0A8)[..2], &[0, 63]);
        // an unused voice writes a period of 113 and no sample
        assert_eq!(&reg(&log, 0xDFF0B6)[..1], &[113]);
        assert_eq!(&reg(&log, 0xDFF0B4)[..1], &[0]);
    }

    #[test]
    fn modulator_ramps_to_its_level() {
        let mut d = vec![0u8; LFO + 128];
        for (i, b) in d[LFO..LFO + 128].iter_mut().enumerate() {
            *b = if i < 64 { 100 } else { 0x9c };
        }
        let mut ks = Vec::new();
        let mut m = Modulator { tlevel: 12, lramp: 4, tspeed: 0, ..Default::default() };
        // level rises 3 per tick; at phase 0 the LFO is 100
        assert_eq!(m.eval(&d, 400, 7, &mut ks), 400 + (100 * 3) / 128);
        assert_eq!(m.level, 3);
        for _ in 0..3 {
            m.eval(&d, 400, 7, &mut ks);
        }
        assert_eq!(m.level, 12);
        // with a zero target level the source passes through
        let mut off = Modulator::default();
        assert_eq!(off.eval(&d, 7, 7, &mut ks), 7);
    }
}
