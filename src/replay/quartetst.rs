//! Quartet ST (Rob Povey and Steve Wetherill, 1989-90; Eagleplayer "Quartet ST", score prefix
//! `qts.` with a companion sample set `SMP.*`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A score is four command streams (notes, rests,
//! slides, sample changes, loops) of 12-byte commands; the 8-bit samples come from a companion set
//! file in the score's directory. A tick runs about 200 Hz (or at the speed of the Amiga variant's
//! header). Every register write is placed at the clock of the specification's cycle ledger,
//! including the first tick's start-up cost. The audio interrupt bits are read from the repo's
//! Paula model at the clock of the read, as the reference does.
//!
//! Faithfully reproduced oddities of the reference: the slide's end check compares the target with
//! a value built from the high half of the frequency and the period (so a downward slide ends after
//! its first step and an upward one usually never ends by itself); the end command's attempt to
//! clear the loop stack has no effect.
//!
//! Choices where the specification is silent or the reference would crash or hang (all safe, none
//! can panic or hang): a frequency whose divisor is zero uses the overflow period; a set record of
//! length 0 or 1 in the ST variant, a corrupt set or a missing companion makes the score not
//! recognised; a loop end with an empty loop stack goes on after the loop end; open loops beyond 100
//! levels are not pushed; a sample index above 0x4C plays the empty sample; a voice that reads more
//! than 100 000 commands in a tick waits 65 535 ticks; the `SMP.set` fallback costs the same
//! start-up time as a companion of the score's own name; host volume, balance and voice masks are
//! not modelled (the defaults are used).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The length of the directory part of the score path (with its trailing slash) used in the
/// first-tick formula; 0 takes the real one. A test and measurement knob.
pub static DIR_LEN: AtomicUsize = AtomicUsize::new(0);

pub const OPEN: Open = Open { name: "Quartet ST", detect, new };

const INTREQ: u32 = 0xDFF09C;
/// The period of a quotient that does not fit 16 bits.
const PER_OVERFLOW: u16 = 0x9E9B;
const FREQ_CONST: u32 = 3_579_546;
/// Commands one voice may read in one tick before it is made to wait (an endless loop of
/// commands that take no time).
const READ_LIMIT: u32 = 100_000;
const MAX_LOOPS: usize = 100;
/// The built-in stream of a voice without a note: a long rest, then the end.
const DEFAULT_REST: u16 = 10_000;

fn rd16(d: &[u8], o: usize) -> u16 {
    match (d.get(o), d.get(o + 1)) {
        (Some(&a), Some(&b)) => u16::from_be_bytes([a, b]),
        (Some(&a), None) => u16::from_be_bytes([a, 0]),
        _ => 0,
    }
}

fn rd32(d: &[u8], o: usize) -> u32 {
    (rd16(d, o) as u32) << 16 | rd16(d, o.wrapping_add(2)) as u32
}

fn detect(d: &[u8]) -> bool {
    let w = rd16(d, 0);
    d.len() >= 28
        && (1..=16).contains(&w)
        && d[7] == 4
        && d[6] <= 4
        && rd32(d, 8) == 0
        && (&d[12..14] == b"WT" || rd32(d, 12) == 0)
        && rd32(d, 24) <= 0x4C
        && rd32(d, 24) % 4 == 0
        && rd16(d, 16) == 0x56
}

/// The period of frequency value `f` (spec 4.1).
fn per(f: u32) -> u16 {
    let d = f >> 3;
    if d == 0 {
        return 1;
    }
    let v = d & 0xFFFF;
    if v == 0 {
        // the reference divides by zero here
        return PER_OVERFLOW;
    }
    let q = FREQ_CONST / v;
    if q > 65_535 {
        PER_OVERFLOW
    } else {
        (q + 1) as u16
    }
}

/// K(f) of the ledger: the period computation's cost.
fn k(f: u32) -> u64 {
    if f >> 3 == 0 {
        43
    } else {
        183
    }
}

const Q: [u64; 4] = [53, 94, 135, 143];
const E: [u64; 4] = [55, 92, 125, 148];

fn sp(c: usize, f: u32) -> u64 {
    Q[c] + 70 + k(f)
}

/// One sample slot after loading (spec 1.3).
#[derive(Clone, Copy, Default)]
struct Slot {
    /// Offset of the record, 0 when empty.
    off: u32,
    /// The record's length word and loop word (as in the file).
    len: u16,
    lp: i16,
}

/// The sample set after loading: the converted bytes, the slots and the facts the start-up cost
/// depends on.
struct Set {
    data: Vec<u8>,
    slots: [Slot; 20],
    amiga: bool,
    two_bit: bool,
    hfr0: bool,
    mp7e: bool,
    samples: u32,
    /// The sample pass's cost in cycles (the ledger's I).
    pass: u64,
}

/// Parses and converts a sample set (spec 1.3). `amiga_header`: the score has `WT`.
fn load_set(z: &[u8], amiga_header: bool) -> Option<Set> {
    let zl = z.len();
    if zl < 2 {
        return None;
    }
    let hfr0 = z.get(760..764) == Some(b"HfR0");
    let mp7e = z.get(13_588..13_592) == Some(b"MP7E");
    let big = zl >= 83_230;
    let amiga = amiga_header || (big && hfr0 && mp7e);
    let two_bit = z.get(222..226) == Some(b"2BIT");
    let mut data = z.to_vec();
    let mut slots = [Slot::default(); 20];
    let mut pass = 36u64;
    for i in 0..20 {
        let o = rd32(z, 142 + 4 * i);
        if o == 0 {
            pass += 43;
            continue;
        }
        if o & 0x8000_0000 != 0 {
            return None;
        }
        if o as usize > zl {
            pass += 116;
            continue;
        }
        pass += 143;
        if two_bit {
            let m = (o as usize).checked_sub(120).map(|p| rd32(z, p) & 0xFF_FFFF);
            if m != Some(0x42_4954) {
                pass += 196 - 143;
                continue;
            }
            pass += 55;
        }
        let ol = o as usize;
        slots[i] = Slot { off: o, len: rd16(z, ol + 4), lp: rd16(z, ol) as i16 };
    }
    pass += 16;
    let mut converted = 0u32;
    let mut max_end = 0usize;
    for i in 0..20 {
        let s = slots[i];
        if s.off == 0 {
            pass += 31;
            continue;
        }
        let o = s.off as usize;
        if data.get(o + 6..o + 8) == Some(b"WT") {
            pass += 60;
            continue;
        }
        let n = (s.len & !1) as usize;
        let odd = if s.len & 1 != 0 { 4 } else { 0 };
        if o + 8 + n > zl {
            pass += 126 + odd;
            continue;
        }
        if !amiga && n == 0 {
            // the reference halves without end
            return None;
        }
        for b in &mut data[o + 8..o + 8 + n] {
            *b = b.wrapping_sub(0x80);
        }
        let mut c = 214 + 26 * n as u64 + odd;
        if !amiga {
            for kk in 0..n / 2 {
                let a = data[o + 8 + 2 * kk] as i8 as i32;
                let b = data[o + 9 + 2 * kk] as i8 as i32;
                data[o + 8 + kk] = ((a + b) >> 1) as i8 as u8;
            }
            c += 8 + 59 * (n as u64 / 2);
        }
        if o + 8 + s.len as usize > max_end {
            c += 4;
            max_end = o + 8 + s.len as usize;
        }
        pass += c;
        data[o + 6] = b'W';
        data[o + 7] = b'T';
        converted += 1;
    }
    pass += 16;
    if converted != (z.get(1).copied().unwrap_or(0) as u32).wrapping_sub(1) {
        return None;
    }
    Some(Set { data, slots, amiga, two_bit, hfr0, mp7e, samples: converted, pass })
}

/// A sample as a `V` command selects it.
#[derive(Clone, Copy, Default)]
struct Samp {
    addr: u32,
    len: u16,
    rep_addr: u32,
    rep_len: u16,
    /// The slot is empty (the start-up cost's R part).
    empty: bool,
    /// The record has a loop (a non-negative loop word).
    looped: bool,
}

#[derive(Clone, Default)]
struct Voice {
    restart: bool,
    pending: bool,
    code: u16,
    dur: u16,
    samp: Samp,
    rep: (u32, u16),
    f: u32,
    target: i32,
    delta: i32,
    s: Vec<u8>,
    start: usize,
    pos: usize,
    loops: Vec<(usize, u16)>,
}

pub struct QuartetSt {
    v: [Voice; 4],
    /// The resolved sample of each of the 20 slots.
    samp: [Samp; 20],
    amiga: bool,
    /// Guest address of the empty sample.
    empty: u32,
    marks: [bool; 4],
    songend: bool,
    tick: u64,
}

/// Where a tick's writes go: the cycle count T from the tick's start gives the colour clock.
struct Clk {
    t0: u64,
    t: u64,
}

impl Clk {
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.t / 2;
    }
}

impl QuartetSt {
    /// One tick (spec 4.2).
    fn run(&mut self, hw: &mut Hw) {
        let mut c = Clk { t0: hw.bus.clock, t: 250 };
        for ch in 0..4 {
            c.t += 56;
            c.at(hw);
            hw.vol(ch, 64);
        }
        // hardware stage
        c.t += 42;
        for ch in 0..4 {
            c.t += 45;
            if self.v[ch].restart {
                self.v[ch].restart = false;
                c.t += 25;
                if self.v[ch].code == 0x52 {
                    c.t += 16;
                    c.at(hw);
                    hw.len(ch, 1);
                    c.t += 24;
                    c.at(hw);
                    hw.loc(ch, self.empty);
                    c.t += 10;
                } else {
                    let s = self.v[ch].samp;
                    c.t += 28;
                    c.at(hw);
                    hw.loc(ch, s.addr);
                    c.t += 32;
                    c.at(hw);
                    hw.len(ch, s.len >> 1);
                    self.v[ch].pending = true;
                    c.t += 40;
                    c.at(hw);
                    hw.bus.write_word(INTREQ, 0x80 << ch);
                    c.t += 36;
                    c.at(hw);
                    hw.dmacon(0x8000 | 1 << ch);
                    c.t += 10;
                }
            } else {
                c.t += 12;
                c.at(hw);
                let now = hw.bus.clock;
                hw.bus.paula.sync(now, &hw.bus.ram);
                let bit = hw.bus.paula.intreq & (0x80 << ch) != 0;
                c.t += 21;
                if bit {
                    c.t += 20;
                    c.at(hw);
                    hw.bus.write_word(INTREQ, 0x80 << ch);
                    c.t += 29;
                    if self.v[ch].pending {
                        self.v[ch].pending = false;
                        let (a, l) = self.v[ch].rep;
                        c.t += 28;
                        c.at(hw);
                        hw.loc(ch, a);
                        c.t += 32;
                        c.at(hw);
                        hw.len(ch, l >> 1);
                    }
                }
            }
            c.t += 34 + k(self.v[ch].f);
            c.t += 12;
            c.at(hw);
            hw.per(ch, per(self.v[ch].f));
            c.t += 18;
        }
        c.t += 16;
        // slide stage
        for ch in 0..4 {
            c.t += 51;
            let v = &mut self.v[ch];
            if v.code != 0x53 {
                c.t += 16;
                continue;
            }
            v.f = v.f.wrapping_add(v.delta as u32);
            c.t += 68 + sp(ch, v.f) + 52;
            let x = ((v.f >> 3) & 0xFFFF_0000 | per(v.f) as u32) as i32;
            let end = if v.delta >= 0 { x >= v.target } else { x <= v.target };
            if end {
                v.f = v.target as u32;
                v.code = 0x50;
                c.t += 62 + sp(ch, v.f);
            }
            c.t += 16;
        }
        // command stage
        for ch in 0..4 {
            c.t += 53;
            let v = &mut self.v[ch];
            v.dur = v.dur.wrapping_sub(1);
            if v.dur != 0 {
                c.t += 16;
                continue;
            }
            c.t += 32;
            self.commands(hw, &mut c, ch);
        }
    }

    /// Reads commands of voice `ch` until one that takes time (spec 4.2 step 4).
    fn commands(&mut self, hw: &mut Hw, c: &mut Clk, ch: usize) {
        for _ in 0..READ_LIMIT {
            let v = &mut self.v[ch];
            let p = v.pos;
            let code = rd16(&v.s, p);
            v.code = code;
            c.t += 20;
            match code {
                0x50 => {
                    c.t += 41;
                    c.at(hw);
                    hw.dmacon(1 << ch);
                    v.dur = rd16(&v.s, p + 2);
                    v.f = rd32(&v.s, p + 4);
                    v.restart = true;
                    v.pos = p + 12;
                    c.t += 58 + sp(ch, v.f) + 38 + 48;
                    return;
                }
                0x52 => {
                    c.t += 58;
                    c.at(hw);
                    hw.dmacon(1 << ch);
                    v.dur = rd16(&v.s, p + 2);
                    v.restart = true;
                    v.pos = p + 12;
                    c.t += 54 + 48;
                    return;
                }
                0x53 => {
                    v.dur = rd16(&v.s, p + 2);
                    v.target = rd32(&v.s, p + 4) as i32;
                    v.delta = rd32(&v.s, p + 8) as i32;
                    v.pos = p + 12;
                    c.t += 199 + 48;
                    return;
                }
                0x56 => {
                    let idx = rd32(&v.s, p + 8) as usize / 4;
                    let s = self.samp.get(idx).copied().unwrap_or(self.samp[0]);
                    v.samp = s;
                    v.rep = (s.rep_addr, s.rep_len);
                    v.pos = p + 12;
                    c.t += 145 + if self.amiga { 0 } else { 16 } + 18 + Q[ch] + 72 + 21;
                    if !s.looped {
                        c.t += 50;
                    } else {
                        c.t += 21 + if self.amiga { 0 } else { 8 } + 62;
                    }
                }
                0x6C => {
                    if v.loops.len() < MAX_LOOPS {
                        v.loops.push((p + 12, 0));
                    }
                    v.pos = p + 12;
                    c.t += 131;
                }
                0x4C => {
                    let n = rd16(&v.s, p + 8);
                    c.t += 160;
                    v.pos = p + 12;
                    if let Some(top) = v.loops.last_mut() {
                        let mut cnt = top.1;
                        if cnt == 0 {
                            cnt = n.wrapping_add(1);
                            c.t += 8;
                        }
                        c.t += 13;
                        cnt = cnt.wrapping_sub(1);
                        if cnt == 0 {
                            v.loops.pop();
                            c.t += 18;
                        } else {
                            top.1 = cnt;
                            v.pos = top.0;
                            c.t += 30;
                        }
                    } else {
                        c.t += 13 + 18;
                    }
                }
                0x46 => {
                    v.pos = v.start;
                    c.t += 113 + 32 + E[ch] + 21;
                    self.marks[ch] = false;
                    if self.marks.iter().all(|m| !m) {
                        self.songend = true;
                        self.marks = [true; 4];
                        c.t += 68;
                        c.t -= c.t % 2;
                        c.t += 16;
                    }
                    c.t += 54;
                }
                _ => {
                    v.pos = p + 2;
                    c.t += 125 + 48;
                    return;
                }
            }
        }
        // endless run of commands without time: wait
        self.v[ch].dur = 0;
        self.v[ch].code = 0;
    }
}

impl Replayer for QuartetSt {
    fn format(&self) -> &'static str {
        "Quartet ST"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        self.tick
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.run(hw);
    }
}

/// The streams of a score: offsets of each stream's start and end (after its `F`), and the offset
/// after the fourth `F`; None if there are not four `F` commands inside the file (spec 1.2).
fn scan_streams(d: &[u8]) -> Option<([usize; 4], usize)> {
    let mut starts = [16usize; 4];
    let mut end = 16usize;
    let mut found = 0;
    let mut o = 28;
    while o + 12 <= d.len() && found < 4 {
        if rd16(d, o) == 0x46 {
            found += 1;
            end = o + 12;
            if found < 4 {
                starts[found] = end;
            }
        }
        o += 12;
    }
    (found == 4).then_some((starts, end))
}

/// Number of commands before the stream's `F` and of notes, from `start`.
fn stream_counts(d: &[u8], start: usize) -> (u64, u64) {
    let (mut n, mut p) = (0u64, 0u64);
    let mut o = start;
    while o + 2 <= d.len() && rd16(d, o) != 0x46 {
        if rd16(d, o) == 0x50 {
            p += 1;
        }
        n += 1;
        o += 12;
    }
    (n, p)
}

/// Cycles of the start-up calls (spec 5.3), each halved and rounded down.
fn start_clocks(d: &[u8], set: &Set, streams: &[usize; 4], j: u64, dir: usize, name: &str, samp: &[Samp; 20], fallback: bool) -> u64 {
    let wt = &d[12..14] == b"WT";
    let speed = rd16(d, 14);
    let b = name.as_bytes();
    let x: u64 = [21u64, 25, 25]
        .iter()
        .enumerate()
        .map(|(i, &cost)| if b.get(i).is_some_and(|c| c.is_ascii_lowercase()) { cost } else { 0 })
        .sum();
    let mut u = if wt { 16 } else { 0 };
    if set.two_bit {
        u += 12;
    }
    if set.data.len() >= 83_230 {
        u += 33;
        if set.hfr0 {
            u += 33;
            if set.mp7e {
                u += 16;
            }
        }
    }
    // stream set-up
    let mut longest = 0usize;
    let mut longer = 0u64;
    let mut s_each = [0u64; 3];
    let mut r_sum = 0u64;
    let mut p_sum = 0u64;
    for v in 0..4 {
        let st = streams[v];
        let mut o = st;
        let mut n = 0u64;
        let mut nr = 0u64;
        let mut np = 0u64;
        while o + 2 <= d.len() && rd16(d, o) != 0x46 {
            let code = rd16(d, o);
            if code == 0x56 {
                let idx = rd32(d, o + 8) as usize / 4;
                nr += 150 + if samp.get(idx).is_none_or(|s| s.empty) { 24 } else { 0 };
            } else {
                nr += 87;
            }
            if code == 0x50 {
                np += 1;
            }
            n += 1;
            o += 12;
        }
        let len = o + 12 - st;
        if v > 0 && len > longest {
            longer += 1;
        }
        longest = longest.max(len);
        if v < 3 {
            s_each[v] = 43 * n + 49;
        }
        r_sum += 34 + nr + 37;
        p_sum += 18 + 109 * n + 4 * np + 34 + if np > 0 { 32 } else { 64 };
    }
    let r = 101 + s_each[0] + 72 + s_each[1] + 75 + s_each[2] + 57 + 75 + 164 + 16 + 12 * longer + r_sum;
    let p = 184 + p_sum;
    let v = if wt && speed != 0 { 192 } else { 14 };
    let l = name.len() as u64;
    let dd = dir as u64;
    let calls = [
        261 + if wt { 0 } else { 25 },
        60,
        284 + 17 * (dd + 1) + 21 * (l + 1) + x,
        45,
        36,
        16 + 247 + 48 * j,
        16 + 322 + u + set.pass,
        16,
        24,
        236,
        236,
        2634 + v + r + p,
    ];
    // (measured: a bank found only as SMP.set, after the failed open of SMP.<name>, costs one
    // more host call, which scans the directory path again: 17 cycles per character)
    let fb = if fallback { (17 * dd + 351) / 2 } else { 0 };
    fb + calls.iter().map(|c| c / 2).sum::<u64>()
}

fn resolve(set: &Set, base: u32, empty: u32) -> [Samp; 20] {
    let mut out = [Samp::default(); 20];
    let e = Samp { addr: empty, len: if set.amiga { 4 } else { 2 }, rep_addr: empty, rep_len: 2, empty: true, looped: false };
    for (i, s) in set.slots.iter().enumerate() {
        if s.off == 0 {
            out[i] = e;
            continue;
        }
        let a = base.wrapping_add(s.off).wrapping_add(8);
        let l = if set.amiga { s.len } else { s.len >> 1 };
        let mut x = Samp { addr: a, len: l, rep_addr: empty, rep_len: 2, empty: false, looped: false };
        if s.lp >= 0 {
            let p = if set.amiga { s.lp as u16 } else { (s.lp as u16) >> 1 };
            x.looped = true;
            x.rep_len = p;
            x.rep_addr = a.wrapping_add((l as i16 as i32 - p as i16 as i32) as u32);
        }
        out[i] = x;
    }
    out
}

/// The four voices at the start (spec 3): a stream without a note is replaced by the built-in one.
fn voices(d: &[u8], starts: &[usize; 4], empty: u32) -> [Voice; 4] {
    let mut def = vec![0u8; 24];
    def[0..2].copy_from_slice(&0x52u16.to_be_bytes());
    def[2..4].copy_from_slice(&DEFAULT_REST.to_be_bytes());
    def[12..14].copy_from_slice(&0x46u16.to_be_bytes());
    let mut v: [Voice; 4] = Default::default();
    for (i, vo) in v.iter_mut().enumerate() {
        let (_, np) = stream_counts(d, starts[i]);
        let (s, start) = if np == 0 { (def.clone(), 0) } else { (d.to_vec(), starts[i]) };
        *vo = Voice {
            code: 0x52,
            dur: 1,
            samp: Samp { addr: empty, len: 2, rep_addr: empty, rep_len: 2, empty: true, looped: false },
            rep: (empty, 2),
            s,
            start,
            pos: start,
            ..Voice::default()
        };
    }
    v
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let (starts, size) = scan_streams(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let name = file.rsplit('/').next().unwrap_or(&file).to_string();
    let wt = &d[12..14] == b"WT";
    let speed = rd16(d, 14);
    // the companion: the first three characters become SMP when the name starts with QTS.
    let mut cands = Vec::new();
    if name.len() >= 4 && name.as_bytes()[..3].eq_ignore_ascii_case(b"qts") && name.as_bytes()[3] == b'.' {
        let rest = &name[3..];
        cands.push(format!("SMP{rest}"));
        cands.push(format!("smp{rest}"));
    }
    cands.push("SMP.set".to_string());
    cands.push("smp.set".to_string());
    let (zdata, set, bank) = cands.iter().find_map(|n| {
        let z = crate::deli::read_companion(m, n).1.ok()?;
        let s = load_set(&z, wt)?;
        Some((z, s, n.as_str()))
    })?;
    // (the failed open of SMP.<name> that precedes it: only for a QTS. name)
    let fallback = bank.eq_ignore_ascii_case("smp.set") && cands.len() > 2;
    let _ = zdata;
    let set_addr = m.alloc(set.data.len() as u32 + 16);
    let empty = m.alloc(16);
    if set_addr == 0 || empty == 0 {
        return None;
    }
    m.bus.write_bytes(set_addr, &set.data);
    let samp = resolve(&set, set_addr, empty);
    let v = voices(d, &starts, empty);
    // J: index of the fourth F, counting the command at 16 as 0
    let j = (size - 12 - 16) as u64 / 12;
    let timer: u64 = if !wt || speed == 0 {
        3547
    } else if speed <= 10 {
        53_990
    } else {
        709_350 / speed as u64
    };
    let dir = match DIR_LEN.load(Ordering::Relaxed) {
        0 => {
            let dir = m.deli.module_dir.to_string_lossy();
            dir.len() + usize::from(!dir.ends_with('/'))
        }
        n => n,
    };
    let sum = start_clocks(d, &set, &starts, j, dir, &name, &samp, fallback);
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    m.bus.clock = (origin + 100 + sum).max(m.bus.clock);
    let _ = set.samples;
    Some(Box::new(QuartetSt {
        v,
        samp,
        amiga: set.amiga,
        empty,
        marks: [true; 4],
        songend: false,
        tick: 5 * (timer + 1),
    }))
}

#[cfg(test)]
#[path = "quartetst_tests.rs"]
mod tests;
