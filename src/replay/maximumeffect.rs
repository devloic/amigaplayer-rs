//! Maximum Effect (`max.*` + `smp.*`), the music driver of the game 'Allo 'Allo! (Alternative
//! Software, 1993; music by Jonathan Scarcliffe and Alastair Dukes).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player and from the module data; no original code and no embedded data (the
//! 36-value period table and the sine table are written from the specification). A module is a
//! pair: the song file `max.NAME` (song entries, track lists, pattern streams) and the sample bank
//! `smp.NAME` (a table of 16-byte sample records, then 8-bit samples), found in the same
//! directory. The module's own code is not run.
//!
//! Each 50 Hz tick steps the active channels in order, then writes the DMA enable word and the
//! loop-part registers of all four channels. The position in time of every write follows the
//! specification's cycle model (a cycle counter per tick, two cycles to the colour clock, the two
//! raster waits on the absolute clock).
//!
//! Quirks of the original that are kept: a song with fewer than four voices assigns its data to
//! the first channels but processes the last ones, so it plays wrongly or not at all and never
//! reports its end; the four effect parameter bytes are shared between the effects and persist
//! across events; the vibrato phase keeps only its masked value after each step; the voice records
//! are not cleared when a sub-song starts.
//!
//! Choices where the specification is undefined (all end the voice or ignore the item, never
//! panic or hang): a command above 8 is a no-operation; a note index above 35 is clamped to 35; a
//! note whose sample number is outside the bank's table is ignored; a pattern stream that is
//! pointed at outside the file, or a restart offset that points at the end marker, stops the voice;
//! an event that reads more than 4096 words without a duration word stops the voice; the portamento
//! target and arpeggio offsets index the period table clamped to its last entry; a processed
//! channel without data never reads an event (its counter is only a counter).
//!
//! The colour clock of the first tick depends on the length of the module's directory path (the
//! reference's start-up copies it): the specification's fitted formula is used with the real
//! length of the path the engine has, and `DIR_LEN` overrides that length (the shipped reference
//! logs were made with a directory part of 96 characters).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The length of the directory part of the module path (with its trailing slash) used in the
/// first-tick formula; 0 takes the real one. A test and measurement knob.
pub static DIR_LEN: AtomicUsize = AtomicUsize::new(0);

pub const OPEN: Open = Open { name: "Maximum Effect", detect, new };

const TICK: u64 = 70_940;
const LINE: u64 = 227;
/// Words one event may read before the voice is stopped (a stream with no duration word).
const MAX_EVENT_WORDS: u32 = 4096;

/// Period table (spec 7): three octaves, note index 0..35.
const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202,
    190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// Sine table (spec 7): floor(255 sin(pi i / 32)) for i = 0..31, then the negatives.
const SINE_HALF: [i16; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141,
    120, 97, 74, 49, 24,
];

fn sine(i: usize) -> i16 {
    let i = i & 63;
    if i < 32 {
        SINE_HALF[i]
    } else {
        -SINE_HALF[i - 32]
    }
}

fn period_at(offset: u8) -> u16 {
    PERIODS[(offset as usize / 2).min(35)]
}

// Per-channel cycle costs (spec 8.3)
const SET_ALL: [u64; 4] = [139, 170, 201, 209];
const VOL_PREP: [u64; 4] = [177, 194, 201, 228];
const VOL_REST: [u64; 4] = [141, 172, 203, 211];
const SONG_END: [u64; 4] = [77, 104, 127, 140];

// ------------------------------------------------------------------ recognition

/// The pointers of the song file for the detection (spec 1.2).
fn detect(d: &[u8]) -> bool {
    let len = d.len();
    if len < 20 || be32(d, 0) < 15 {
        return false;
    }
    let p1 = be32(d, 4) as usize;
    if be32(d, 4) != 0 && (be32(d, 4) >= 0x8000_0000 || p1 & 1 != 0 || p1 > len || p1 < 2 + 18 || (p1 - 2) % 18 != 0) {
        return false;
    }
    let mut any = p1 != 0;
    for o in [8usize, 12, 16] {
        let p = be32(d, o);
        if p >= 0x8000_0000 {
            return false;
        }
        if p != 0 {
            let p = p as usize;
            if p & 1 != 0 || p > len || p < 6 || be32(d, p - 6) != 0 {
                return false;
            }
            any = true;
        }
    }
    any
}

// ------------------------------------------------------------------ player

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Effect {
    None,
    Vibrato,
    VolSlide,
    Portamento,
    Arpeggio,
}

#[derive(Clone, Copy)]
struct Voice {
    /// The channel has a song voice with a track list.
    assigned: bool,
    /// The channel is processed in a tick.
    active: bool,
    list: usize,
    pos: usize,
    stream: usize,
    count: u16,
    counter: u16,
    effect: Effect,
    /// The shared parameter bytes A, B, C, D.
    par: [u8; 4],
    base_per: u16,
    per: u16,
    vol: u16,
    loop_addr: u32,
    loop_len: u16,
}

impl Voice {
    const NEW: Voice = Voice {
        assigned: false,
        active: false,
        list: 0,
        pos: 0,
        stream: 0,
        count: 0,
        counter: 0,
        effect: Effect::None,
        par: [0; 4],
        base_per: 0,
        per: 0,
        vol: 0,
        loop_addr: 0,
        loop_len: 0,
    };
}

struct MaximumEffect {
    d: Vec<u8>,
    bank: Vec<u8>,
    bank_addr: u32,
    nsamp: usize,
    nsongs: usize,
    title: String,
    /// The name length (with `max.`) and directory length (with the slash) of the module path.
    name_len: usize,
    dir_len: usize,
    sub: usize,
    v: [Voice; 4],
    flags: u8,
    flags_init: u8,
    play_once: bool,
    songend: bool,
    next_at: Option<u64>,
    delay: u64,
    // per tick
    t0: u64,
    t: u64,
    rebase: Option<u64>,
    started: u16,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let name = file.rsplit('/').next().unwrap_or(&file).to_string();
    if name.len() < 4 || !name.is_char_boundary(3) || !name[..4].eq_ignore_ascii_case("max.") {
        return None;
    }
    // the bank: the first three characters become SMP, else SMP.set (spec 1.1)
    let rest = &name[3..];
    let cands = [format!("SMP{rest}"), format!("smp{rest}"), "SMP.set".to_string(), "smp.set".to_string()];
    let bank = cands.iter().find_map(|n| crate::deli::read_companion(m, n).1.ok())?;
    let nsamp = (be16(&bank, 0) as usize) + 1;
    if bank.len() < 2 + 16 * nsamp {
        return None;
    }
    // the load check of spec 3.2
    let last = 2 + 16 * (nsamp - 1);
    if (be32(&bank, last) as usize) + 2 * be16(&bank, last + 4) as usize > bank.len() {
        return None;
    }
    let bank_addr = m.alloc((bank.len() as u32 + 8).max(8));
    if bank_addr == 0 {
        return None;
    }
    m.bus.write_bytes(bank_addr, &bank);
    let p1 = be32(d, 4) as usize;
    let nsongs = if p1 >= 20 { ((p1 - 2) / 18).min(255) } else { 1 };
    let dir_len = match DIR_LEN.load(Ordering::Relaxed) {
        0 => {
            let dir = m.deli.module_dir.to_string_lossy();
            dir.len() + usize::from(!dir.ends_with('/'))
        }
        n => n,
    };
    let mut p = MaximumEffect {
        d: d.to_vec(),
        bank,
        bank_addr,
        nsamp,
        nsongs,
        title: name.clone(),
        name_len: name.len(),
        dir_len,
        sub: 0,
        v: [Voice::NEW; 4],
        flags: 0,
        flags_init: 0,
        play_once: false,
        songend: false,
        next_at: None,
        delay: 0,
        t0: 0,
        t: 0,
        rebase: None,
        started: 0,
    };
    p.init(0);
    // The reference's song start lies a fixed distance (TICK) before the first tick,
    // and the raster waits of the ticks see the beam at the absolute clock, whose line phase there
    // depends on the module (the fitted first-call formula, which only counts modulo a raster line):
    // the start moves on to the clock that gives the first tick that phase.
    let phase = (p.first_raw(0) - TICK) % LINE;
    let base = m.bus.clock + (phase + LINE - m.bus.clock % LINE) % LINE;
    m.bus.clock = base;
    let at = base + TICK;
    p.delay = at - base - TICK;
    Some(Box::new(p))
}

impl MaximumEffect {
    // --- safe reads of the song file

    fn rw(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }
    fn rl(&self, o: usize) -> usize {
        be32(&self.d, o) as usize
    }
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }
    fn bankw(&self, o: usize) -> u16 {
        be16(&self.bank, o)
    }

    // --- starting a sub-song (spec 4)

    fn init(&mut self, n: usize) {
        let n = n.min(self.nsongs - 1);
        self.sub = n;
        self.songend = false;
        let e = 2 + 18 * n;
        let mask = self.rb(e + 1);
        let k = (mask & 15).count_ones() as usize;
        self.play_once = mask & 0x10 != 0;
        // only the fields set at the start are touched; the rest of a record persists
        let mut ch = 0;
        for vi in 0..4 {
            if mask & (1 << vi) != 0 {
                let ptr = self.rl(e + 2 + 4 * vi);
                let v = &mut self.v[ch];
                v.list = ptr;
                v.pos = 0;
                v.count = 0;
                v.counter = 0x8000;
                v.assigned = ptr != 0 && ptr < self.d.len();
                ch += 1;
            }
        }
        for c in 0..4 {
            if c >= ch {
                self.v[c].assigned = false;
            }
            self.v[c].active = c >= 4 - k;
        }
        self.flags_init = mask & 15;
        self.flags = self.flags_init;
    }

    /// The number of entries of a track list, not counting its end marker.
    fn list_entries(&self, ptr: usize) -> usize {
        let mut n = 0;
        let mut p = ptr;
        while p + 6 <= self.d.len() && self.rl(p) != 0 && n < 100_000 {
            n += 1;
            p += 6;
        }
        n
    }

    /// The colour clock of the first tick of sub-song `s` after the host's start-up (README 3),
    /// moved to the next point on the same raster-line grid at or after `now + TICK`.
    fn first_tick(&self, now: u64, s: usize) -> u64 {
        let f = self.first_raw(s);
        let min = now + TICK;
        if f >= min {
            f
        } else {
            f + (min - f).div_ceil(LINE) * LINE
        }
    }

    /// The first-call formula for sub-song `s` as fitted (the clock of a run that starts at 0).
    fn first_raw(&self, s: usize) -> u64 {
        let z = [8usize, 12, 16].iter().filter(|&&o| self.rl(o) == 0).count() as i64;
        let c = (654 - 65 * z) / 2;
        let x = (2254 + 22 * (self.name_len as i64 - 6) + 17 * (self.dir_len as i64 - 96)) / 2;
        let mut entries = 0i64;
        for j in 0..self.nsongs {
            for vi in 0..4 {
                let p = self.rl(2 + 18 * j + 2 + 4 * vi);
                if p != 0 {
                    entries += self.list_entries(p) as i64;
                }
            }
        }
        let i = (499 + 416 * self.nsongs as i64 + 61 * entries + 143 * self.nsamp as i64) / 2;
        let mut sum = 0i64;
        for j in 0..=s.min(self.nsongs - 1) {
            let e = 2 + 18 * j;
            let k = (self.rb(e + 1) & 15).count_ones() as i64;
            let p0 = self.rl(e + 2);
            let l = if p0 != 0 { 47 * self.list_entries(p0) as i64 } else { -29 };
            sum += (2537 + 156 * k + l) / 2 + if j > 0 { 284 } else { 0 };
        }
        (100 + c + x + i + 270 + sum).max(0) as u64 + TICK
    }

    // --- time inside a tick (spec 8.1)

    fn now(&self) -> u64 {
        match self.rebase {
            None => self.t0 + self.t / 2,
            Some(tn) => self.t0 + tn / 2 + 8 + (self.t - tn) / 2,
        }
    }
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.now();
    }

    /// The raster wait of spec 8.4.
    fn raster_wait(&mut self) {
        self.t += 28;
        for _ in 0..9 {
            self.t += 16;
            let l = self.now() / LINE;
            loop {
                self.t += 16;
                let line = self.now() / LINE;
                self.t += 9;
                if line != l {
                    break;
                }
            }
            self.t += 10;
        }
        self.t += 44;
    }

    // --- the voice step (spec 5.1)

    fn step(&mut self, c: usize, hw: &mut Hw) {
        self.t += 25;
        self.v[c].counter = self.v[c].counter.wrapping_sub(1);
        if self.v[c].counter & 0x8000 != 0 || !self.v[c].assigned {
            self.t += 39;
            if self.v[c].effect == Effect::None {
                self.t += 16;
                return;
            }
            self.t += 16;
            self.run_effect(c);
            self.t += 10;
            self.output(c, hw);
            return;
        }
        self.new_event(c, hw);
    }

    /// Stops the voice (it leaves the processed set).
    fn stop(&mut self, c: usize) {
        self.v[c].active = false;
    }

    fn new_event(&mut self, c: usize, hw: &mut Hw) {
        self.t += 45;
        self.v[c].effect = Effect::None;
        if self.v[c].count == 0 {
            self.t += 67;
            let p = self.v[c].list + self.v[c].pos;
            let ptr = self.rl(p);
            let entry;
            if ptr != 0 {
                self.v[c].stream = ptr;
                entry = p;
            } else {
                self.t += 33;
                if self.play_once {
                    self.t += 48;
                    self.stop(c);
                    return;
                }
                self.t += 41;
                let restart = self.rw(p + 4) as usize;
                self.v[c].pos = restart;
                self.t += 74;
                let l = self.v[c].list;
                let s = self.rl(l + restart);
                self.v[c].stream = s;
                self.t += SONG_END[c];
                self.flags &= !(1 << c);
                self.t += 21;
                if self.flags == 0 {
                    self.t += 68;
                    self.rebase = Some(self.t);
                    self.songend = true;
                    self.flags = self.flags_init;
                }
                self.t += 44;
                if s == 0 {
                    // the restart offset points at the end marker: undefined, the voice stops
                    self.stop(c);
                    return;
                }
                entry = l + restart;
            }
            self.v[c].count = self.rw(entry + 4);
            self.v[c].pos = (entry - self.v[c].list) + 6;
            self.t += 38;
        }
        if self.v[c].stream >= self.d.len() {
            self.stop(c);
            return;
        }
        // read the event (spec 5.4)
        let mut first = true;
        let mut words = 0u32;
        loop {
            words += 1;
            if words > MAX_EVENT_WORDS {
                self.stop(c);
                return;
            }
            let w = self.rw(self.v[c].stream);
            self.v[c].stream += 2;
            self.t += if first { 33 } else { 17 };
            if w & 0x8000 == 0 {
                self.command(c, w, hw);
            } else if first {
                self.note(c, w, hw);
            } else {
                self.v[c].counter = w;
                self.v[c].count = self.v[c].count.wrapping_sub(1);
                self.t += 44;
                break;
            }
            first = false;
        }
        self.output(c, hw);
    }

    /// Note start (spec 5.4.1).
    fn note(&mut self, c: usize, w: u16, hw: &mut Hw) {
        let r = ((w >> 8) & 0x7F) as usize;
        let n = ((w >> 1) & 0x7F) as usize;
        self.t += 57;
        if r >= self.nsamp {
            return; // undefined: no note
        }
        let rec = 2 + 16 * r;
        if w & 1 == 0 {
            self.v[c].vol = self.bankw(rec + 6);
            self.t += 22;
        }
        self.v[c].per = PERIODS[n.min(35)];
        self.t += 22 + 16;
        self.at(hw);
        hw.dmacon(1 << c);
        self.t += 4;
        self.started |= 1 << c;
        self.t += 26;
        self.at(hw);
        hw.loc(c, self.bank_addr.wrapping_add(be32(&self.bank, rec)));
        self.t += 22;
        self.at(hw);
        hw.len(c, self.bankw(rec + 4));
        self.t += 18 + SET_ALL[c];
        self.v[c].loop_addr = self.bank_addr.wrapping_add(be32(&self.bank, rec + 8));
        self.v[c].loop_len = self.bankw(rec + 12);
        self.t += 52;
    }

    /// The output step (spec 5.6).
    fn output(&mut self, c: usize, hw: &mut Hw) {
        self.t += 20;
        self.at(hw);
        hw.per(c, self.v[c].per);
        self.t += VOL_PREP[c];
        self.at(hw);
        hw.vol(c, ((self.v[c].vol as u32 * 64) & 0xFFFF) as u16 >> 6);
        self.t += VOL_REST[c];
    }

    fn next_byte(&mut self, c: usize) -> u8 {
        let b = self.rb(self.v[c].stream);
        self.v[c].stream += 1;
        b
    }

    /// A command word (spec 5.5).
    fn command(&mut self, c: usize, w: u16, hw: &mut Hw) {
        let cmd = (w >> 8) & 0x7F;
        let b = (w & 0xFF) as u8;
        self.t += 60 + 10;
        match cmd {
            1 | 2 => {
                self.t += 24;
                hw.bus.clock = self.now();
                let x = hw.bus.read_byte(0xBFE001);
                hw.bus.write_byte(0xBFE001, if cmd == 1 { x | 2 } else { x & !2 });
                self.t += 10;
            }
            3 => {
                self.v[c].vol = b as u16;
                self.t += 22;
            }
            4 | 5 => {
                self.t += if cmd == 4 { 16 + 106 } else { 106 };
                if cmd == 4 {
                    self.v[c].par[1] = 0;
                }
                self.v[c].effect = Effect::Vibrato;
                self.v[c].par[2] = b;
                self.v[c].par[3] = self.next_byte(c);
                self.v[c].par[0] = self.next_byte(c);
                self.v[c].base_per = self.v[c].per;
                self.vibrato(c);
                self.t += 10;
            }
            6 => {
                self.v[c].effect = Effect::VolSlide;
                self.v[c].par[0] = b;
                self.t += 46;
            }
            7 => {
                self.v[c].effect = Effect::Portamento;
                self.v[c].par[1] = b;
                self.v[c].par[2] = self.next_byte(c);
                self.v[c].par[0] = self.next_byte(c);
                self.t += 86;
                self.portamento(c);
                self.t += 10;
            }
            8 => {
                self.v[c].effect = Effect::Arpeggio;
                self.v[c].par[0] = b;
                self.v[c].par[1] = self.next_byte(c);
                self.v[c].par[2] = self.next_byte(c);
                self.v[c].par[3] = 0xFF;
                self.t += 94;
            }
            // 0, and the undefined ones above 8: no operation
            _ => {}
        }
    }

    // --- the effect steps (spec 5.7)

    fn run_effect(&mut self, c: usize) {
        match self.v[c].effect {
            Effect::None => {}
            Effect::Vibrato => self.vibrato(c),
            Effect::VolSlide => self.vol_slide(c),
            Effect::Portamento => self.portamento(c),
            Effect::Arpeggio => self.arpeggio(c),
        }
    }

    fn vol_slide(&mut self, c: usize) {
        let s = self.v[c].par[0] as i8 as i16;
        if s == 0 {
            self.t += 37;
            return;
        }
        let v = self.v[c].vol.wrapping_add(s as u16);
        if v & 0x8000 != 0 {
            self.v[c].vol = 0;
            self.t += 82;
        } else if v > 0x40 {
            self.v[c].vol = 0x40;
            self.t += 113;
        } else {
            self.v[c].vol = v;
            self.t += 95;
        }
    }

    fn vibrato(&mut self, c: usize) {
        self.t += 136;
        let [_, b, sp, depth] = self.v[c].par;
        let entry = sine(((b & 0x7E) / 2) as usize) as i32;
        let sh = depth & 63;
        let shifted = if sh >= 16 { 0 } else { (entry << sh) as i16 };
        let delta = shifted >> 7;
        self.v[c].per = self.v[c].base_per.wrapping_add(delta as u16);
        self.v[c].par[1] = (b & 0x7E).wrapping_add(sp);
        self.vol_slide(c);
    }

    fn portamento(&mut self, c: usize) {
        self.t += 84;
        let t = period_at(self.v[c].par[1]) as i16;
        let step = self.v[c].par[2] as i8 as i16;
        let mut p = (self.v[c].per as i16).wrapping_add(step);
        if step >= 0 {
            if p > t {
                p = t;
                self.t += 14;
            }
        } else if p < t {
            p = t;
            self.t += 4;
        }
        self.v[c].per = p as u16;
        self.t += 22;
        self.vol_slide(c);
    }

    fn arpeggio(&mut self, c: usize) {
        self.t += 113;
        let mut d = self.v[c].par[3].wrapping_add(1);
        if d >= 3 {
            d = 0;
            self.t += 4;
        }
        self.v[c].par[3] = d;
        let x = self.v[c].par[d as usize];
        self.v[c].per = period_at(x);
    }
}

impl Replayer for MaximumEffect {
    fn format(&self) -> &'static str {
        "Maximum Effect"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn timer_delay(&self) -> u64 {
        self.delay
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsongs as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // the host's stop routine (spec 9.2)
        // it follows the start-up of the sub-song being left by 14 colour clocks
        let at = self.first_raw(self.sub) - TICK + 14;
        hw.bus.clock = hw.bus.clock.max(at);
        hw.dmacon(0x000F);
        for (i, off) in [8u64, 6, 6, 6].iter().enumerate() {
            hw.bus.clock += off;
            hw.vol(i, 0);
        }
        self.init(n as usize);
        self.next_at = Some(self.first_tick(hw.bus.clock, n as usize));
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.t = 300;
        self.rebase = None;
        self.started = 0;
        for c in 0..4 {
            self.t += 25;
            if self.v[c].active {
                self.t += if c == 0 { 42 } else { 46 };
                self.step(c, hw);
            }
        }
        self.t += 52 + 56;
        self.raster_wait();
        self.t += 20;
        self.at(hw);
        hw.dmacon(0x8000 | self.started);
        self.t += 18;
        self.raster_wait();
        for c in 0..4 {
            self.t += 28;
            self.at(hw);
            hw.loc(c, self.v[c].loop_addr);
            self.t += 20;
            self.at(hw);
            hw.len(c, self.v[c].loop_len);
        }
        self.t += 16 + 168;
    }
}

#[cfg(test)]
#[path = "maximumeffect_tests.rs"]
mod tests;
