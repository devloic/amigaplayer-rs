//! SUN-Tronic (Eagleplayer "SUN-Tronic", module prefix `sun.`; a game's sound driver by Felix Schmidt,
//! adapted for the Eagleplayer by Mr.Larmer/Wanted Team). Not the data-file format `Tronic`
//! (`tronic.rs`): a SUN-Tronic module is a relocatable driver image that carries its own tables.
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes and
//! execution-time histograms of the 68k reference player; no original code. The driver code in the
//! module is not run: the replayer interprets the module's data tables as the specification says.
//! Two driver revisions are played, told apart by the word at offset 8: variant A (a tracker with
//! a song-end hook, five synthesised instruments of 0x24 bytes, waveform types 0 to 4, fade and noise
//! commands) and variant B (no song-end hook, ten instruments of 0x22 bytes, a fixed 32-byte wave
//! blended between two waves or fed back). Four voices play looping waveforms that are rebuilt
//! every tick. Each register write is placed at the cycle count the specification's ledger gives.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): reads outside the
//! file give zero; an instrument index past the record table, and sample instruments (stream notes
//! 0x01..0x3F), are ignored (the note does nothing); a wave length above 0x40 words is clamped to the
//! 0x80-byte buffer when the wave is built (the LEN register still gets the module's value);
//! type 2 with a morph value above the wave length is clamped; type 3 with a zero divisor copies
//! wave A, and a first part longer than the wave is cut; type 4 with a zero divisor uses the
//! overflow value; the end-of-song marker of variant A and an empty or unterminated entry list wrap
//! to entry 0 like the list's end (at most two passes, then the voices keep their streams); a
//! stream that does not end within 4096 events ends its row; out-of-range period-table and
//! arpeggio indices read the module's following bytes unrelocated. A voice that has not started
//! yet copies one long word in the variant-A buffer step. The host's phase of the first call differs
//! a few colour clocks from module to module (the first call of the real modules: 98 786 and
//! 111 533); the real modules' values are used. The voice block's address is the replayer's own
//! chip-RAM allocation; the strides between voices are the reference's, so address differences
//! are a constant.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "SUN-Tronic", detect, new };

/// Colour clocks between two ticks (the CIA timer's period).
const TICK: u64 = 70940;
/// The interrupt entry before a call's first cycle, in colour clocks.
const IRQ: u64 = 76;
/// First call's start in colour clocks from the run's origin: variant A, variant B.
const START_A: u64 = 98786;
const START_B: u64 = 111533;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Var {
    A,
    B,
}

#[derive(Clone, Copy)]
struct Voice {
    /// Stream pointer (the module's own absolute address).
    stream: u32,
    trans: u8,
    /// File offset of the instrument record in use.
    inst: Option<usize>,
    pitch: u16,
    slide: u16,
    level: u8,
    lslide: i8,
    arp_n: u8,
    arp_ph: u8,
    env_pos: u16,
    morph_pos: u16,
    lfo_acc: u16,
    lfo_pos: u16,
    vol: u8,
    per: u16,
    mode: u8,
    /// The start flag word, which is also the start and loop length (0: none pending).
    pend: u16,
    /// Remembered note byte (variant A).
    note: u8,
}

impl Voice {
    fn new() -> Self {
        Voice {
            stream: 0,
            trans: 0,
            inst: None,
            pitch: 0,
            slide: 0,
            level: 0,
            lslide: 0,
            arp_n: 0,
            arp_ph: 0,
            env_pos: 0,
            morph_pos: 0,
            lfo_acc: 0,
            lfo_pos: 0,
            vol: 0,
            per: 0,
            mode: 0xFF,
            pend: 0,
            note: 0,
        }
    }
}

/// The layout the detection finds.
struct Layout {
    var: Var,
    /// Link origin (1.5).
    origin: u32,
    list: usize,
    ptab: usize,
    arp: usize,
    inst0: usize,
}

fn be16(d: &[u8], o: usize) -> u16 {
    d.get(o..o + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
}

fn be32(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 0x400 || be32(d, 0) != 0x48E7_FFFE || be16(d, 4) != 0x4DFA {
        return None;
    }
    let var = match be32(d, 8) {
        0x4A2E_0018 => Var::A,
        0x4A2E_0010 => Var::B,
        _ => return None,
    };
    let disp = be16(d, 6) as i16 as i64;
    let dd = 6 + disp;
    if dd < 0 {
        return None;
    }
    let dd = dd as usize;
    let (list, ptab, arp, inst0, isz, n) = match var {
        Var::A => (dd + 0x19C, dd + 0x54, dd + 0x4F3, dd + 0xE4, 0x24, 5),
        Var::B => (dd + 0x22E, dd + 0x46, dd + 0x5A2, dd + 0xD6, 0x22, 10),
    };
    if list + 0x14 > d.len() {
        return None;
    }
    // the sequence list ends at the first long word (at 4-byte steps) that is 0 or all ones
    let mut p = list;
    loop {
        if p + 4 > d.len() {
            return None;
        }
        let w = be32(d, p);
        if w == 0 || w == 0xFFFF_FFFF {
            break;
        }
        p += 4;
    }
    // at least one entry
    if p < list + 0x14 {
        return None;
    }
    let a = (p + 4) as u32;
    let mut min = u32::MAX;
    for i in 0..n {
        for o in [0usize, 8, 0x12, 0x1A, 0x1E] {
            let v = be32(d, inst0 + isz * i + o);
            if v != 0 && v < min {
                min = v;
            }
        }
    }
    if min == u32::MAX || min < a {
        return None;
    }
    let rule = min - a;
    let origin = rule - best_slack(d, var, list, p, rule);
    Some(Layout { var, origin, list, ptab, arp, inst0 })
}

/// The end of the 16-row stream that starts at file offset `o` (None if it leaves the file).
fn stream_end(d: &[u8], mut o: usize, var: Var) -> Option<usize> {
    let mut rows = 0;
    while rows < 16 {
        let b = *d.get(o)?;
        o += 1;
        if b == 0 {
            rows += 1;
        } else if (0x80..=0xB7).contains(&b) {
            o += match (var, b) {
                (_, 0x9C | 0x9A | 0x99 | 0x98) | (Var::B, 0x9B) => 1,
                (Var::A, 0x9B | 0x97 | 0x93) => 2,
                (Var::A, 0x94 | 0x92) => 1,
                _ => 0,
            };
        }
    }
    Some(o)
}

/// How far below the rule's origin the true link origin lies. The specification's rule (smallest
/// instrument pointer minus the end of the list) assumes that the first data after the list is
/// referenced by an instrument. Where it is not (the specification's synthetic variant-A modules,
/// whose reference runs read the data 0x33 bytes higher), the streams of the first entries, which the
/// builder lays end to end, identify the origin: the shift that makes the most streams end exactly
/// where the next begins is taken if it beats the rule's own count clearly. Real modules keep the
/// rule's value.
fn best_slack(d: &[u8], var: Var, list: usize, term: usize, rule: u32) -> u32 {
    let ents = ((term - list) / 0x14).min(6);
    let score = |slack: u32| -> usize {
        let o = (rule - slack) as i64;
        let mut n = 0;
        for e in 0..ents {
            for v in 0..4 {
                let ptr = |e: usize, v: usize| be32(d, list + 0x14 * e + 4 * v) as i64 - o;
                let next = if v < 3 { ptr(e, v + 1) } else { ptr(e + 1, 0) };
                let s = ptr(e, v);
                if s >= 0 && stream_end(d, s as usize, var).map(|x| x as i64) == Some(next) {
                    n += 1;
                }
            }
        }
        n
    };
    let base = score(0);
    let (mut best, mut at) = (base, 0);
    for slack in 1..0x200u32.min(rule) {
        let n = score(slack);
        if n > best {
            best = n;
            at = slack;
        }
    }
    if best >= 8 && best > base + 4 {
        at
    } else {
        0
    }
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

pub struct SunTronic {
    d: Vec<u8>,
    /// The module as the driver sees it in memory: the pointer fields hold the relocated addresses
    /// (an index past a table reads them).
    mem: Vec<u8>,
    var: Var,
    origin: u32,
    list: usize,
    ptab: usize,
    arp: usize,
    inst0: usize,
    ninst: usize,
    isz: usize,
    tick: u8,
    row: u8,
    speed: u8,
    entry: u32,
    master: u8,
    fade_lvl: u8,
    fade_en: u8,
    fade_cnt: u8,
    n1: u16,
    n2: u16,
    v: [Voice; 4],
    /// The first buffers (read by Paula) and the second buffers (built every tick).
    buf1: [u32; 4],
    buf2: [[u8; 0x80]; 4],
    songend: bool,
}

/// Charges `n` cycles to the tick's ledger (only variant A's logic costs matter to writes).
fn add(a: bool, c: &mut u64, n: u64) {
    if a {
        *c += n;
    }
}

impl SunTronic {
    fn byte(&self, o: i64) -> u8 {
        if o < 0 {
            return 0;
        }
        self.d.get(o as usize).copied().unwrap_or(0)
    }

    fn word(&self, o: i64) -> u16 {
        (self.byte(o) as u16) << 8 | self.byte(o + 1) as u16
    }

    fn long(&self, o: i64) -> u32 {
        (self.word(o) as u32) << 16 | self.word(o + 2) as u32
    }

    /// The file offset a module pointer names.
    fn rel(&self, p: u32) -> i64 {
        p as i64 - self.origin as i64
    }

    /// The byte at index `i` of the table whose pointer is the long word at record offset `o`.
    fn tab(&self, r: usize, o: usize, i: i64) -> u8 {
        self.byte(self.rel(self.long((r + o) as i64)) + i)
    }

    fn mbyte(&self, o: i64) -> u8 {
        if o < 0 {
            return 0;
        }
        self.mem.get(o as usize).copied().unwrap_or(0)
    }

    fn mword(&self, o: i64) -> u16 {
        (self.mbyte(o) as u16) << 8 | self.mbyte(o + 1) as u16
    }

    fn a(&self) -> bool {
        self.var == Var::A
    }

    /// The write time of cycle `c` of the call that starts at `s`.
    fn at(hw: &mut Hw, s: u64, c: u64) {
        hw.bus.clock = s + IRQ + c / 2;
    }

    fn load_entry(&mut self, e: u32) -> bool {
        let p = self.list as i64 + 0x14 * e as i64;
        let first = self.long(p);
        if first == 0 || first == 0xFFFF_FFFF {
            return false;
        }
        for i in 0..4 {
            self.v[i].stream = self.long(p + 4 * i as i64);
            self.v[i].trans = self.byte(p + 0x10 + i as i64);
        }
        true
    }

    // ---------------------------------------------------------------- stream parse (1.9, 5.3)

    fn operand(&mut self, v: usize) -> u8 {
        let p = self.v[v].stream;
        self.v[v].stream = p.wrapping_add(1);
        self.byte(self.rel(p))
    }

    fn note_start(&mut self, v: usize, b: u8, c: &mut u64) {
        let a = self.a();
        add(a, c, 31);
        self.v[v].note = b;
        if b < 0x40 {
            // sample instruments: unspecified, ignored
            return;
        }
        add(a, c, 304);
        let i = (b - 0x40) as usize;
        if i >= self.ninst {
            return;
        }
        let r = self.inst0 + self.isz * i;
        let len = if a { self.byte((r + 0x22) as i64) as u16 } else { 16 };
        let s = &mut self.v[v];
        s.inst = Some(r);
        s.arp_n = 0;
        s.arp_ph = 0;
        s.env_pos = 0;
        s.morph_pos = 0;
        s.lfo_acc = 0;
        s.lfo_pos = 0;
        s.mode = 1;
        s.pend = len;
    }

    fn parse(&mut self, v: usize, c: &mut u64) {
        let a = self.a();
        add(a, c, 12);
        for _ in 0..4096 {
            let b = self.operand(v);
            add(a, c, 19);
            if b == 0 {
                add(a, c, 28);
                return;
            }
            add(a, c, if a { 11 } else { 9 });
            if b < 0x80 {
                self.note_start(v, b, c);
                continue;
            }
            add(a, c, 19);
            if b >= 0xB8 {
                add(a, c, 83);
                let s = &mut self.v[v];
                s.pitch = ((!b).wrapping_sub(s.trans) as u16) << 8;
                s.slide = 0;
                let n = s.note;
                if a && n != 0 {
                    add(a, c, 10);
                    self.note_start(v, n, c);
                }
                continue;
            }
            self.command(v, b, c);
        }
    }

    /// A command byte 0x80..0xB7 (variant B knows only 0x98..0x9C).
    fn command(&mut self, v: usize, b: u8, c: &mut u64) {
        let a = self.a();
        const TESTS: [u8; 11] = [0x9C, 0x9B, 0x9A, 0x99, 0x98, 0x97, 0x96, 0x95, 0x94, 0x93, 0x92];
        const BODY: [u64; 11] = [42, 64, 26, 50, 26, 80, 26, 26, 78, 42, 26];
        let known = if a { 11 } else { 5 };
        let mut hit = None;
        for (k, &t) in TESTS.iter().enumerate().take(known) {
            add(a, c, 17);
            if t == b {
                hit = Some(k);
                break;
            }
        }
        let Some(k) = hit else {
            add(a, c, 10);
            return;
        };
        add(a, c, BODY[k]);
        match b {
            0x9C => {
                let x = self.operand(v);
                self.v[v].arp_n = x;
                self.v[v].arp_ph = 0;
            }
            0x9B => {
                let s = if a {
                    let h = self.operand(v);
                    let l = self.operand(v);
                    (h as u16) << 8 | l as u16
                } else {
                    self.operand(v) as i8 as i16 as u16
                };
                self.v[v].slide = s;
            }
            0x9A => {
                let x = self.operand(v);
                self.v[v].lslide = x as i8;
            }
            0x99 => {
                let x = self.operand(v);
                self.v[v].level = x.wrapping_mul(2);
                self.v[v].lslide = 0;
            }
            0x98 => self.speed = self.operand(v),
            0x97 => {
                let h = self.operand(v);
                let l = self.operand(v);
                self.n1 = (h as u16) << 8 | l as u16;
                self.n2 = self.n1 ^ 0x7E28;
            }
            0x96 => self.v[v].env_pos = 0,
            0x95 => self.v[v].morph_pos = 0,
            0x94 => {
                let x = self.operand(v);
                let s = &mut self.v[v];
                s.pitch = ((!x).wrapping_sub(s.trans) as u16) << 8;
                s.slide = 0;
            }
            0x93 => {
                self.fade_en = self.operand(v);
                // the second operand is not read by anything
                self.operand(v);
            }
            _ => self.fade_lvl = self.operand(v),
        }
    }

    // ---------------------------------------------------------------- envelope step (5.4)

    fn env(&mut self, v: usize, c: &mut u64) {
        let a = self.a();
        add(a, c, 23);
        let mut s = self.v[v];
        let Some(r) = s.inst.filter(|_| s.mode & 0x80 == 0) else {
            add(a, c, 16);
            return;
        };
        // volume
        let e = self.tab(r, 0, s.env_pos as i64) as u32;
        let mut vol = (((e * s.level as u32) & 0xFFFF) >> 7) & 0xFF;
        if a {
            vol = (((vol * self.master as u32) & 0xFFFF) >> 6) & 0xFF;
        }
        add(a, c, 244);
        s.vol = vol as u8;
        // period
        add(a, c, 145);
        let q = self.tab(r, 8, s.lfo_pos as i64) as i32;
        let mut x = s.lfo_acc;
        if x & 0x8000 != 0 {
            x = x.wrapping_neg();
            add(a, c, 4);
        }
        let y = x.wrapping_sub(0x4000) as i16 as i32;
        let z = ((y.wrapping_mul(q)) as u32 >> 12) as u16;
        add(a, c, 106);
        let p2 = s.pitch.wrapping_add(z);
        let arp = self.mbyte(self.arp as i64 + (if a { 16 } else { 8 }) * s.arp_n as i64 + s.arp_ph as i64);
        let i = ((p2 >> 8) as u8).wrapping_sub(arp) as i64;
        let t0 = self.mword(self.ptab as i64 + 2 * i) as u32;
        let f = (p2 & 0xFF) as u32;
        add(a, c, 75);
        s.per = if f == 0 {
            t0 as u16
        } else {
            add(a, c, 100);
            let t1 = self.mword(self.ptab as i64 + 2 * i + 2) as u32;
            if a {
                t0.wrapping_add(((t1.wrapping_sub(t0) & 0xFFFF) * f) >> 8) as u16
            } else {
                ((t0 * (0x100 - f) + t1 * f) >> 8) as u16
            }
        };
        if !a && s.mode & 1 != 0 {
            self.blend_b(v, &mut s, r);
        }
        // pitch slide
        add(a, c, 49);
        s.pitch = s.pitch.wrapping_add(s.slide);
        if a {
            if s.pitch & 0x8000 != 0 {
                s.pitch = s.pitch.wrapping_add(0x4800);
                add(a, c, 20);
            }
            add(a, c, 25);
            if (s.pitch.wrapping_sub(0x4800) as i16) >= 0 {
                s.pitch = s.pitch.wrapping_sub(0x4800);
                add(a, c, 20);
            }
        }
        // level slide
        add(a, c, 49);
        let t = s.level as i16 + s.lslide as i16;
        s.level = if t < 0 {
            add(a, c, 14);
            0
        } else {
            add(a, c, 17);
            if t >= 0x81 {
                add(a, c, 8);
                0x80
            } else {
                t as u8
            }
        };
        add(a, c, 97);
        s.arp_ph = (s.arp_ph + 1) & if a { 0xF } else { 7 };
        s.env_pos = s.env_pos.wrapping_add(1);
        if s.env_pos == self.word((r + 4) as i64) {
            s.env_pos = self.word((r + 6) as i64);
            add(a, c, 20);
        }
        add(a, c, 77);
        s.lfo_acc = s.lfo_acc.wrapping_add(self.word((r + 0x10) as i64));
        s.lfo_pos = s.lfo_pos.wrapping_add(1);
        if s.lfo_pos == self.word((r + 0xC) as i64) {
            s.lfo_pos = self.word((r + 0xE) as i64);
            add(a, c, 20);
        }
        if !a && s.mode & 1 != 0 {
            s.morph_pos = s.morph_pos.wrapping_add(1);
            if s.morph_pos == self.word((r + 0x16) as i64) {
                s.morph_pos = self.word((r + 0x18) as i64);
            }
        }
        add(a, c, 16);
        self.v[v] = s;
    }

    /// The waveform blend of variant B (5.8), into the voice's second buffer.
    fn blend_b(&mut self, v: usize, s: &mut Voice, r: usize) {
        let m = self.tab(r, 0x12, s.morph_pos as i64) as i32;
        let k0 = 0x80 - m;
        let wa = self.rel(self.long((r + 0x1A) as i64));
        let pb = self.long((r + 0x1E) as i64);
        let mut out = [0u8; 32];
        if pb != 0 {
            let wb = self.rel(pb);
            for (i, o) in out.iter_mut().enumerate() {
                let sa = self.byte(wa + i as i64) as i8 as i32;
                let sb = self.byte(wb + i as i64) as i8 as i32;
                *o = ((((sa * k0 + sb * m) & 0xFFFF) as u16 as i16) >> 7) as u8;
            }
        } else {
            let mut src = [0u8; 32];
            if s.mode & 2 != 0 {
                src.copy_from_slice(&self.buf2[v][..32]);
            } else {
                for (i, b) in src.iter_mut().enumerate() {
                    *b = self.byte(wa + i as i64);
                }
            }
            s.mode |= 2;
            let mut prev = src[31] as i8 as i32;
            for i in 0..32 {
                let x = src[i] as i8 as i32;
                prev = (((x * k0 + prev * m) & 0xFFFF) as u16 as i16 >> 7) as i32;
                out[i] = prev as u8;
            }
        }
        self.buf2[v][..32].copy_from_slice(&out);
    }

    // ---------------------------------------------------------------- waveform stage, variant A (5.7)

    fn wave_a(&mut self, v: usize) {
        let mut s = self.v[v];
        let Some(r) = s.inst else { return };
        if s.mode & 1 == 0 || s.mode & 0x80 != 0 {
            return;
        }
        let m = self.tab(r, 0x12, s.morph_pos as i64);
        let hl = (self.byte((r + 0x22) as i64) as usize).min(0x40);
        let cnt = hl * 2;
        let ty = self.byte((r + 0x23) as i64);
        let wa = self.rel(self.long((r + 0x1A) as i64));
        let wb = self.rel(self.long((r + 0x1E) as i64));
        let dd = &self.d;
        let rd = |o: i64| if o < 0 { 0 } else { dd.get(o as usize).copied().unwrap_or(0) };
        let ga = |i: i64| rd(wa + i);
        let gb = |i: i64| rd(wb + i);
        let mut out = self.buf2[v];
        match ty {
            0 => {
                for i in 0..cnt {
                    let da = ga(i as i64) as i8 as i32;
                    let db = gb(i as i64) as i8 as i32;
                    let t = ((((db - da) * m as i32) & 0xFFFF) as u16 as i16) >> 7;
                    out[i] = (da + t as i32) as u8;
                }
            }
            1 => {
                if m == 0xFE {
                    for (i, o) in out.iter_mut().enumerate().take(cnt) {
                        *o = ga(i as i64);
                    }
                } else {
                    let mut src = [0u8; 0x80];
                    if s.mode & 2 != 0 {
                        src = self.buf2[v];
                    } else {
                        for (i, b) in src.iter_mut().enumerate().take(cnt) {
                            *b = ga(i as i64);
                        }
                    }
                    s.mode |= 2;
                    if m == 0xFF {
                        let mut w = self.n1 as u32;
                        for i in 0..hl {
                            w = (((w * w) >> 8) & 0xFFFF) ^ 0xAC91;
                            out[2 * i] = (w >> 8) as u8;
                            out[2 * i + 1] = w as u8;
                        }
                        let n2 = self.n2 as u32;
                        self.n2 = ((((n2 * n2) >> 8) & 0xFFFF) ^ 0x6E47) as u16;
                        self.n1 = (w as u16) ^ self.n2;
                    } else if cnt > 0 {
                        let k = (0x80i32 - m as i32) & 0xFF;
                        let mut prev = src[cnt - 1] as i8 as i16;
                        for i in 0..cnt {
                            let x = src[i] as i8 as i16;
                            let t = (((x.wrapping_sub(prev) as i32 * k) & 0xFFFF) as u16 as i16) >> 7;
                            prev = prev.wrapping_add(t);
                            out[i] = prev as u8;
                        }
                    }
                }
            }
            2 => {
                let k = (m as usize).min(cnt);
                for i in 0..cnt {
                    out[i] = if i < k { gb(i as i64) } else { ga(i as i64) };
                }
            }
            3 => {
                let d = (m as i8 as i32 + 0x40) as u16 as u32;
                if d == 0 || cnt == 0 {
                    // the original traps: copy wave A
                    for (i, o) in out.iter_mut().enumerate().take(cnt) {
                        *o = ga(i as i64);
                    }
                } else {
                    let step1 = (0x8000 / d) << 8;
                    let n1 = ((d * (cnt as u32 - 1)) & 0xFFFF) >> 7;
                    let n2 = (cnt as i32 - 1) - n1 as i32;
                    let mut pos = 0u32;
                    let mut o = 0usize;
                    for _ in 0..=n1 {
                        if o < cnt {
                            out[o] = ga((pos >> 16) as u16 as i16 as i64);
                            o += 1;
                        }
                        pos = pos.wrapping_add(step1);
                    }
                    if n2 - 1 >= 0 {
                        let d2 = (0x80u32.wrapping_sub(d)) & 0xFFFF;
                        if d2 != 0 {
                            let step2 = (0x8000 / d2) << 8;
                            for _ in 0..n2 {
                                if o < cnt {
                                    out[o] = ga((pos >> 16) as u16 as i16 as i64);
                                    o += 1;
                                }
                                pos = pos.wrapping_add(step2);
                            }
                        }
                    }
                }
            }
            4 => {
                let mut src = [0u8; 0x80];
                if s.mode & 2 != 0 {
                    src = self.buf2[v];
                } else {
                    for (i, b) in src.iter_mut().enumerate().take(cnt) {
                        *b = ga(i as i64);
                    }
                }
                s.mode |= 2;
                if cnt > 0 {
                    let cc = (m as u32 + 0x20) & 0xFFFF;
                    let mut rr = if cc == 0 { 0xFFE0 } else { 0x000F_FFE0 / cc };
                    if rr > 0xFFFF {
                        rr = 0xFFE0;
                    }
                    let rs = (rr as i32 - 0x26 * (m as i8 as i32)) as u16 as i16 as i32;
                    let mut x = ((src[cnt - 1] as i8 as i16) << 7) as u16;
                    for i in 0..cnt {
                        let inp = ((ga(i as i64) as i8 as i16) << 7) as u16;
                        let df = inp.wrapping_sub(x) as i16 as i32;
                        let t = ((df * rs) >> 16) as u16;
                        x = x.wrapping_add(t.wrapping_mul(2));
                        out[i] = (x >> 7) as u8;
                    }
                }
            }
            _ => {
                if s.mode & 2 == 0 {
                    s.mode |= 2;
                    for (i, o) in out.iter_mut().enumerate().take(cnt) {
                        *o = ga(i as i64);
                    }
                }
            }
        }
        self.buf2[v] = out;
        s.morph_pos = s.morph_pos.wrapping_add(1);
        if s.morph_pos == self.word((r + 0x16) as i64) {
            s.morph_pos = self.word((r + 0x18) as i64);
        }
        self.v[v] = s;
    }

    // ---------------------------------------------------------------- a call

    /// Copies voice `v`'s second buffer to the first, `longs` long words, one at a time (each at its
    /// cycle count `c0 + step * (k + 1)`), so that Paula fetches what the driver had written by then.
    fn copy(&self, hw: &mut Hw, s: u64, v: usize, longs: usize, c0: u64, step: u64) {
        let n = longs.min(0x20);
        for k in 0..n {
            Self::at(hw, s, c0 + step * (k as u64 + 1));
            let (clock, ram) = (hw.bus.clock, &hw.bus.ram);
            hw.bus.paula.sync(clock, ram);
            hw.bus.write_bytes(self.buf1[v] + 4 * k as u32, &self.buf2[v][4 * k..4 * k + 4]);
        }
    }

    /// The song's logic after stage 1: tick counter, row, entry, parse, envelope steps (5.1).
    fn logic(&mut self, c: &mut u64) {
        let a = self.a();
        if a {
            // fade step
            *c += 21;
            if self.fade_en != 0 {
                *c += 25;
                self.fade_cnt = self.fade_cnt.wrapping_sub(1);
                if self.fade_cnt & 0x80 != 0 {
                    *c += 57;
                    self.fade_cnt = self.fade_en;
                    self.fade_lvl = self.fade_lvl.wrapping_mul(2);
                    if self.fade_lvl & 0x80 != 0 {
                        *c += 32;
                        self.fade_lvl = 0;
                        self.fade_en = 0;
                    }
                    *c += 25;
                    if self.fade_lvl >= 0x41 && self.fade_lvl & 0x80 == 0 {
                        *c += 32;
                        self.fade_lvl = 0x40;
                        self.fade_en = 0;
                    }
                }
            }
        }
        add(a, c, 51);
        self.tick = self.tick.wrapping_add(1);
        if self.tick != self.speed {
            add(a, c, 20);
            for v in 0..4 {
                add(a, c, 18);
                self.env(v, c);
                add(a, c, 22);
            }
            return;
        }
        add(a, c, 57);
        self.tick = 0;
        self.row = self.row.wrapping_add(1);
        if self.row == 0x10 {
            add(a, c, 40);
            self.row = 0;
            self.entry += 1;
            self.advance(c);
        }
        add(a, c, 20);
        for v in 0..4 {
            add(a, c, 18);
            self.parse(v, c);
            add(a, c, 18);
            self.env(v, c);
            add(a, c, 22);
        }
        add(a, c, 10);
    }

    /// The entry lookup (5.10), repeated after a wrap.
    fn advance(&mut self, c: &mut u64) {
        let a = self.a();
        for _ in 0..2 {
            add(a, c, if a { 109 } else { 103 });
            let first = self.long(self.list as i64 + 0x14 * self.entry as i64);
            if a && first == 0xFFFF_FFFF {
                // the end-of-song marker: the master volume drops, the host is told; the entry after
                // it is undefined, so the list wraps here
                self.master = 1;
                self.songend = true;
                self.entry = 0;
                continue;
            }
            add(a, c, 27);
            if first == 0 {
                if a {
                    add(a, c, 482);
                    self.songend = true;
                } else {
                    add(false, c, 0);
                }
                self.entry = 0;
                continue;
            }
            add(a, c, 220);
            self.load_entry(self.entry);
            return;
        }
    }

    fn call_a(&mut self, hw: &mut Hw) {
        let s = hw.bus.clock;
        let mut c: u64 = 166;
        // stage 1
        c += 20;
        let mut m = 0u16;
        for v in 0..4 {
            c += 21;
            if self.v[v].pend != 0 {
                c += 12;
                m |= 1 << v;
            }
        }
        c += 16;
        Self::at(hw, s, c);
        hw.dmacon(m);
        c += 32;
        let mut loops = [0u16; 4];
        for v in 0..4 {
            c += 20;
            Self::at(hw, s, c);
            hw.vol(v, self.v[v].vol as u16 * 0x101);
            c += 20;
            Self::at(hw, s, c);
            hw.per(v, self.v[v].per);
            c += 21;
            if self.v[v].pend != 0 {
                c += 24;
                Self::at(hw, s, c);
                hw.loc(v, self.buf1[v]);
                c += 20;
                Self::at(hw, s, c);
                hw.len(v, self.v[v].pend);
            }
            c += 90;
            loops[v] = self.v[v].pend;
        }
        c += 28;
        self.logic(&mut c);
        // stage 2
        c += 18 + 24 + 16;
        // voices that were not restarted this call are copied just before the DMA is set; the
        // restarted ones (flagged) are copied after the loop section, below
        for v in 0..4 {
            if self.v[v].mode != 0 && loops[v] == 0 {
                let hl = self.v[v].inst.map_or(0, |r| self.byte((r + 0x22) as i64) as usize);
                self.copy(hw, s, v, (hl >> 1).max(1).min(0x20), c, 0);
            }
        }
        Self::at(hw, s, c);
        hw.dmacon(0x8000 | m);
        c += 4 + 970 + 32;
        for v in 0..4 {
            c += 21;
            if loops[v] != 0 {
                c += 48 + 12;
                Self::at(hw, s, c);
                hw.loc(v, self.buf1[v]);
                c += 20;
                Self::at(hw, s, c);
                hw.len(v, loops[v]);
                c += 16;
                // the loop step clears the voice's start flag as it is now: a note that this very
                // call's parse started on a voice restarted in stage 1 is lost
                self.v[v].pend = 0;
            }
            c += 34;
        }
        c += 20;
        for v in (0..4).rev() {
            c += 21;
            if self.v[v].mode != 0 {
                c += 53 + 8;
                let hl = self.v[v].inst.map_or(0, |r| self.byte((r + 0x22) as i64) as usize);
                let longs = (hl >> 1).max(1);
                if loops[v] != 0 {
                    self.copy(hw, s, v, longs.min(0x20), c, 34);
                }
                c += 34 * longs as u64;
            }
            c += 22;
        }
        // the waveform stage and the call's end move no more registers
        for v in 0..4 {
            self.wave_a(v);
        }
    }

    fn call_b(&mut self, hw: &mut Hw) {
        // the call that follows a new-row call starts 4 colour clocks late in the reference's host
        let s = hw.bus.clock + if self.tick == 0 { 4 } else { 0 };
        let mut c: u64 = 157;
        if self.tick == 0 {
            c += 10 + 20;
            let mut m = 0u16;
            for v in 0..4 {
                c += 21;
                if self.v[v].pend != 0 {
                    c += 12;
                    m |= 1 << v;
                }
            }
            c += 16;
            Self::at(hw, s, c);
            hw.dmacon(m);
            c += 32;
            for v in 0..4 {
                c += 20;
                Self::at(hw, s, c);
                hw.vol(v, self.v[v].vol as u16 * 0x101);
                c += 20;
                Self::at(hw, s, c);
                hw.per(v, self.v[v].per);
                c += 21;
                if self.v[v].pend != 0 {
                    c += 24;
                    Self::at(hw, s, c);
                    hw.loc(v, self.buf1[v]);
                    c += 20;
                    Self::at(hw, s, c);
                    hw.len(v, self.v[v].pend);
                }
                c += 26;
            }
            c += 28;
            for v in 0..4 {
                self.copy(hw, s, v, 8, c, 236 / 8);
                c += 236;
            }
            c += 8 + 2570 + 8 + 16;
            Self::at(hw, s, c);
            hw.dmacon(0x8000 | m);
            c += 32;
            for v in 0..4 {
                c += 21;
                if self.v[v].pend != 0 {
                    c += 52 + 12;
                    Self::at(hw, s, c);
                    hw.loc(v, self.buf1[v]);
                    c += 20;
                    Self::at(hw, s, c);
                    hw.len(v, self.v[v].pend);
                    c += 16;
                    self.v[v].pend = 0;
                }
                c += 26;
            }
        } else {
            c += 18 + 32;
            for v in 0..4 {
                c += 20;
                Self::at(hw, s, c);
                hw.vol(v, self.v[v].vol as u16 * 0x101);
                c += 20;
                Self::at(hw, s, c);
                hw.per(v, self.v[v].per);
                c += 25;
                if self.v[v].mode & 1 != 0 {
                    c += 12;
                    self.copy(hw, s, v, 8, c, 34);
                    c += 34 * 8;
                }
                c += 26;
            }
        }
        let mut c = 0u64;
        self.logic(&mut c);
    }
}

impl Replayer for SunTronic {
    fn format(&self) -> &'static str {
        "SUN-Tronic"
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
        match self.var {
            Var::A => self.call_a(hw),
            Var::B => self.call_b(hw),
        }
    }
}

/// The module's bytes after the player's relocation (1.5): the pointer fields of the instrument
/// records and the four stream pointers of every sequence entry are moved from the link origin to the
/// load address `base`.
fn relocated(d: &[u8], l: &Layout, base: u32) -> Vec<u8> {
    let mut m = d.to_vec();
    let delta = base.wrapping_sub(l.origin);
    let (isz, n) = if l.var == Var::A { (0x24, 5) } else { (0x22, 10) };
    let mut fix = |o: usize| {
        let v = be32(d, o);
        if v != 0 && o + 4 <= m.len() {
            m[o..o + 4].copy_from_slice(&v.wrapping_add(delta).to_be_bytes());
        }
    };
    for i in 0..n {
        for f in [0usize, 8, 0x12, 0x1A, 0x1E] {
            fix(l.inst0 + isz * i + f);
        }
    }
    let mut e = l.list;
    while e + 0x14 <= d.len() && be32(d, e) != 0 && be32(d, e) != 0xFFFF_FFFF {
        for k in 0..4 {
            fix(e + 4 * k);
        }
        e += 0x14;
    }
    m
}

impl SunTronic {
    fn build(d: &[u8], l: &Layout, base: u32, block: u32) -> Self {
        let a = l.var == Var::A;
        let dd = if a { l.inst0 - 0xE4 } else { l.inst0 - 0xD6 };
        let (stride, first) = if a { (0x130, 0x26) } else { (0x68, 0x22) };
        let mut r = SunTronic {
            d: d.to_vec(),
            mem: relocated(d, l, base),
            var: l.var,
            origin: l.origin,
            list: l.list,
            ptab: l.ptab,
            arp: l.arp,
            inst0: l.inst0,
            ninst: if a { 5 } else { 10 },
            isz: if a { 0x24 } else { 0x22 },
            tick: if a { 5 } else { 4 },
            row: if a { 0xFF } else { 0x0F },
            speed: if a { 6 } else { 5 },
            entry: if a { 0 } else { 0x1F },
            master: if a { d.get(dd + 0x18).copied().unwrap_or(0) } else { 0 },
            fade_lvl: if a { d.get(dd + 0x19).copied().unwrap_or(0) } else { 0 },
            fade_en: 0,
            fade_cnt: if a { d.get(dd + 0x1C).copied().unwrap_or(0) } else { 0 },
            n1: if a { be16(d, dd + 0x24) } else { 0 },
            n2: if a { be16(d, dd + 0x26) } else { 0 },
            v: [Voice::new(); 4],
            buf1: [0; 4],
            buf2: [[0; 0x80]; 4],
            songend: false,
        };
        for v in 0..4 {
            r.buf1[v] = block + v as u32 * stride + first;
        }
        // all voices start with the streams and transpositions of entry 0
        r.load_entry(0);
        r
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = layout(d)?;
    let a = l.var == Var::A;
    let block = m.alloc(if a { 4 * 0x130 } else { 4 * 0x68 });
    if block == 0 {
        return None;
    }
    // the reference's start-up counts from the run's origin, which is START_PHASE colour clocks
    // before the engine's aligned start; the first call follows one period after the clock left here
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let s0 = if a { START_A } else { START_B };
    m.bus.clock = (origin + s0 - TICK).max(m.bus.clock);
    Some(Box::new(SunTronic::build(d, &l, base, block)))
}

#[cfg(test)]
#[path = "suntronic_tests.rs"]
mod tests;
