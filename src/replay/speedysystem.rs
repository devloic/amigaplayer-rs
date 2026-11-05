//! Speedy System (Michael Winterberg, 1988; Eagleplayer "Speedy System", module prefix `ss.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code. A module is the 13-byte text `SPEEDY-SYSTEM`, a
//! marker word 0x1190 that fixes the base of all other offsets, four 256-entry position tables,
//! 50 sample offsets and lengths, then the tracks (16 rows of 3 bytes, with run codes) and the
//! 8-bit samples (8SVX-style objects in three shapes). Four voices, a per-note attack and release
//! envelope on the volume, a one-time loop reload and an add/subtract period effect. Every register
//! write is placed at the clock of the original's code, from the specification's cycle ledger.
//!
//! "Speedy A1 System" (prefix `sas.`) is a different player and is not covered.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a sample index
//! outside 0..0x30, or naming an empty slot or one whose object lies outside the file, is rejected
//! like the specified 0x31..0xB0 range (the voice stays inactive); a marker at an offset other than
//! 0x14 is taken as the first match of the search (offsets 14..268); bytes asked for beyond the end
//! of the file read as zero; command bytes other than 0x50 and 0x51 are stored and have no effect;
//! when all 256 positions are identical the start-up scan stops at the table's start.
//! The silent-sample address is a block of zeros the replayer allocates itself.

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Speedy System", detect, new };

/// Colour clocks between two ticks (the CIA timer's period).
const TICK: u64 = 70940;
const MAGIC: &[u8; 13] = b"SPEEDY-SYSTEM";
const MARKER: u32 = 0x1190;
/// Colour clocks from the start of the run to the first tick, before the scan's share.
const E0_BASE: u64 = 87738;

const PERIODS: [u16; 13] = [0x194, 0x17D, 0x167, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2, 0x0D6, 0x000];
/// Octave term of a note's slot, by octave 1..4.
const OCT: [u64; 5] = [0, 53, 8, 0, 45];

const IGNORED: [u64; 4] = [206, 206, 206, 257];
const COMMAND: [u64; 4] = [385, 385, 385, 436];
const NOTE_N0: [u64; 4] = [322, 322, 322, 373];
const NOTE_BIG: [u64; 4] = [347, 347, 347, 398];
const SILENT: [u64; 4] = [836, 891, 946, 1052];
const REJECTED: [u64; 4] = [1077, 1132, 1187, 1293];
/// Slot durations of an accepted note by sample case (A, B, L, M, R).
const ACCEPT: [[u64; 4]; 5] = [
    [3055, 3265, 3475, 3621],
    [3090, 3300, 3510, 3656],
    [3170, 3380, 3590, 3736],
    [3063, 3273, 3483, 3629],
    [2763, 2911, 3059, 3189],
];
const DMA_AT: [u64; 4] = [368, 423, 478, 533];
const PER_AT: [u64; 4] = [1277, 1332, 1387, 1442];
const LEN_AT: [u64; 4] = [1653, 1770, 1887, 1958];
/// Second AUDxLC offset by case (A, B, L, M).
const LC2_AT: [[u64; 4]; 4] = [
    [2323, 2471, 2619, 2698],
    [2358, 2506, 2654, 2733],
    [2484, 2632, 2780, 2859],
    [2331, 2479, 2627, 2706],
];
const VO: [u64; 4] = [0, 23, 46, 36];
const VD: [u64; 4] = [0, 54, 108, 106];
const EFF_ADD: [u64; 4] = [228, 259, 290, 298];
const EFF_SUB: [u64; 4] = [251, 282, 313, 321];
const REL_LEN_AT: [u64; 4] = [416, 447, 478, 486];

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The offset of the marker, searched as the player does (offsets 14 to 268, two at a time).
fn find_marker(d: &[u8]) -> Option<usize> {
    (0..128).map(|i| 14 + 2 * i).find(|&o| o + 4 <= d.len() && be32(d, o) == MARKER)
}

/// The marker's offset if the file is a module: the text, a marker, and tables and track
/// pointers that lie in the file.
fn locate(d: &[u8]) -> Option<usize> {
    if d.len() < 14 + 4 || &d[..13] != MAGIC {
        return None;
    }
    let mk = find_marker(d)?;
    // position tables, sample tables and the first track
    if d.len() < mk + 0x1190 + 3 {
        return None;
    }
    // the first sample table entries are offsets or empty slots; every track pointer is inside
    for i in 0..1024 {
        if mk + be32(d, mk + 4 * i) as usize >= d.len() {
            return None;
        }
    }
    Some(mk)
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

/// What a decoded track costs the expansion, summed over the tracks of a tick.
#[derive(Default, Clone, Copy)]
struct Cost {
    lit: u64,
    run: u64,
    n90: u64,
    n92: u64,
    cut: u64,
}

type Rows = [[u8; 3]; 16];

/// Expands the track at `pos` into 16 rows (spec 1.4). Every item consumes input, and bytes past
/// the file read as zero (a literal row), so it ends.
fn expand(d: &[u8], mut pos: usize, c: &mut Cost) -> Rows {
    let mut rows = [[0u8; 3]; 16];
    let mut n = 0;
    while n < 16 {
        let b0 = byte(d, pos);
        if b0 == 0x90 || b0 == 0x92 {
            let k = (byte(d, pos + 1) as usize).min(16 - n);
            pos += 2;
            let fill = if b0 == 0x90 { 0xFF } else { 0 };
            for r in rows.iter_mut().skip(n).take(k) {
                *r = [fill; 3];
            }
            n += k;
            c.run += k as u64;
            if b0 == 0x90 {
                c.n90 += 1;
            } else {
                c.n92 += 1;
            }
            if n < 16 {
                c.cut += 1;
            }
        } else {
            rows[n] = [b0, byte(d, pos + 1), byte(d, pos + 2)];
            pos += 3;
            n += 1;
            c.lit += 1;
        }
    }
    rows
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    attack: bool,
    vol: u16,
    att: u16,
    rel: u16,
    timer: u16,
    period: u16,
    cmd: u8,
    arg: u16,
    /// Pending loop reload: guest address and length in words.
    reload: Option<(u32, u16)>,
}

/// A sample ready to start: the writes of spec 4.3.
struct Sample {
    lc: u32,
    len: u16,
    /// Case index into the duration tables (A, B, L, M, R) and the second write set.
    case: usize,
    second: Option<(u32, u16)>,
    reload: Option<(u32, u16)>,
}

/// Places a write: `ct` is the song-end quirk's trap cursor (spec 5.5).
struct Stamp {
    t0: u64,
    ct: Option<u64>,
}

impl Stamp {
    fn at(&self, hw: &mut Hw, c: u64) {
        hw.bus.clock = self.t0
            + match self.ct {
                Some(ct) if c > ct => ct / 2 + (c - ct) / 2,
                _ => c / 2,
            };
    }
}

pub struct SpeedySystem {
    d: Vec<u8>,
    base: u32,
    mk: usize,
    speed: u8,
    silent: u32,
    counter: u8,
    row: usize,
    pos: usize,
    v: [Voice; 4],
    songend: bool,
}

fn map_period(i: u8) -> u16 {
    match i {
        14 => 16,
        15 => 24,
        i => i as u16,
    }
}

impl SpeedySystem {
    fn sample(&self, i: usize) -> Option<Sample> {
        let d = &self.d;
        if i > 0x30 {
            return None;
        }
        let off = be32(d, self.mk + 0x1000 + 4 * i);
        if off & 0x8000_0000 != 0 {
            return None;
        }
        let x = self.mk + off as usize + 7;
        if x >= d.len() {
            return None;
        }
        let stored = be32(d, self.mk + 0x10C8 + 4 * i);
        let b = x & !1;
        let g = |o: usize| self.base.wrapping_add((b + o) as u32);
        let lc = self.base.wrapping_add(((x + 1) & !1) as u32);
        let len = (stored.wrapping_sub(12) >> 1) as u16;
        const BODY: u32 = 0x424F_4459;
        let one = be32(d, b + 0x14);
        let rep = be32(d, b + 0x18);
        let (case, second, reload);
        if be32(d, b + 0x60) == BODY {
            let body = be32(d, b + 0x64);
            let full = (g(0x68), (body >> 1) as u16);
            if rep == 0 {
                (case, second, reload) = (0, Some(full), None);
            } else if one == 0 {
                (case, second, reload) = (1, Some(full), None);
            } else {
                let first = (g(0x68), (one.wrapping_sub(4) >> 1) as u16);
                let rl = (self.base.wrapping_add((b as u32).wrapping_add(one).wrapping_add(0x64)), (rep >> 1) as u16);
                (case, second, reload) = (2, Some(first), Some(rl));
            }
        } else if be32(d, b + 0x28) == BODY {
            (case, second, reload) = (3, Some((g(0x30), (one.wrapping_sub(4) >> 1) as u16)), None);
        } else {
            (case, second, reload) = (4, None, None);
        }
        Some(Sample { lc, len, case, second, reload })
    }

    /// One tick at `hw.bus.clock`.
    fn run(&mut self, hw: &mut Hw) {
        let mut st = Stamp { t0: hw.bus.clock, ct: None };
        let full = self.counter == 0;
        let mut cur: u64;
        if full {
            self.counter = self.speed;
            let mut cost = Cost::default();
            let mut rows = [[[0u8; 3]; 16]; 4];
            for (v, r) in rows.iter_mut().enumerate() {
                let p = be32(&self.d, self.mk + v * 0x400 + 4 * self.pos) as usize;
                *r = expand(&self.d, self.mk + p, &mut cost);
            }
            let exp = 1756 + 123 * cost.lit + 104 * cost.run + 47 * cost.n90 + 70 * cost.n92 + 31 * cost.cut;
            cur = 501 + exp + 40;
            cur = self.row(hw, &mut st, &rows, cur);
            let nact = self.v.iter().filter(|v| v.active).count() as u64;
            st.at(hw, cur + 622 + 8 * nact);
            let mask = 0x8200 | self.v.iter().enumerate().fold(0, |m, (i, v)| m | (v.active as u16) << i);
            hw.dmacon(mask);
            cur += 656 + 8 * nact + 20;
        } else {
            self.counter -= 1;
            cur = 335;
        }
        cur = self.update(hw, &st, cur);
        if full {
            self.reloads(hw, &st, cur + 34 + 38);
        }
    }

    /// Spec 3.3: the row of every voice; returns the cursor after the slots (D0).
    fn row(&mut self, hw: &mut Hw, st: &mut Stamp, rows: &[Rows; 4], s0: u64) -> u64 {
        let mut cur = s0;
        let r = self.row;
        for v in 0..4 {
            let extra = if v == 3 && r == 15 { 75 } else { 0 };
            let [b0, b1, b2] = rows[v][r];
            if b0 == 0xFF {
                if v == 0 {
                    st.ct = Some(cur + 258);
                    self.pos = 0;
                    self.row = 0;
                    self.counter = 0;
                    self.songend = true;
                    return cur + 385;
                }
                cur += IGNORED[v] + 8 + extra;
                continue;
            }
            if b0 >= 0x90 || b0 < 0x10 {
                cur += IGNORED[v] + extra;
                continue;
            }
            if b0 >= 0x50 {
                self.v[v].cmd = b0;
                self.v[v].arg = (b1 as u16) << 8 | b2 as u16;
                cur += COMMAND[v] + extra;
                continue;
            }
            let o = (b0 >> 4) as usize;
            let n = (b0 & 15) as usize;
            if n == 0 {
                cur += NOTE_N0[v] + extra;
                continue;
            }
            if n >= 14 {
                cur += NOTE_BIG[v] + extra;
                continue;
            }
            let oo = OCT[o];
            self.v[v].active = false;
            self.v[v].cmd = 0;
            st.at(hw, cur + DMA_AT[v]);
            hw.dmacon(1 << v);
            st.at(hw, cur + DMA_AT[v] + 20);
            hw.dmacon(0x8200);
            let p = PERIODS[n - 1];
            self.v[v].period = if o < 3 { p << (3 - o) } else { p >> (o - 3) };
            if b1 == 0xFF {
                cur += SILENT[v] + oo + extra;
                continue;
            }
            let smp = if b2 > 0x30 { None } else { self.sample(b2 as usize) };
            let Some(s) = smp else {
                cur += REJECTED[v] + oo + extra;
                continue;
            };
            let vs = &mut self.v[v];
            vs.att = map_period(b1 >> 4);
            vs.rel = map_period(b1 & 15);
            vs.attack = true;
            vs.vol = 0;
            vs.timer = 0;
            vs.active = true;
            vs.reload = s.reload;
            let per = vs.period;
            st.at(hw, cur + PER_AT[v] + oo);
            hw.per(v, per);
            st.at(hw, cur + PER_AT[v] + oo + 16);
            hw.loc(v, s.lc);
            st.at(hw, cur + LEN_AT[v] + oo);
            hw.len(v, s.len);
            if let Some((lc, len)) = s.second {
                let t = cur + LC2_AT[s.case][v] + oo;
                st.at(hw, t);
                hw.loc(v, lc);
                st.at(hw, t + 12);
                hw.len(v, len);
            }
            cur += ACCEPT[s.case][v] + oo + extra;
        }
        self.row += 1;
        if self.row == 16 {
            self.row = 0;
            self.pos += 1;
            if self.pos == 256 {
                self.pos = 0;
                self.counter = 0;
                self.songend = true;
                st.ct = Some(cur + 67);
                cur += 194;
            }
        }
        cur
    }

    /// Spec 3.5: the voice update chain from cursor `t`; returns the cursor after it.
    fn update(&mut self, hw: &mut Hw, st: &Stamp, mut t: u64) -> u64 {
        for v in 0..4 {
            if !self.v[v].active {
                t += 156;
                continue;
            }
            let mut eo = 0;
            let vs = &mut self.v[v];
            if vs.cmd == 0x50 || vs.cmd == 0x51 {
                let add = vs.cmd == 0x50;
                vs.period = if add { vs.period.wrapping_add(vs.arg) } else { vs.period.wrapping_sub(vs.arg) };
                st.at(hw, t + if add { 310 } else { 333 });
                hw.per(v, vs.period);
                eo = if add { EFF_ADD[v] } else { EFF_SUB[v] };
            }
            // (volume write offset, duration) at voice 0, and the volume to write
            let vs = &mut self.v[v];
            let vol_write: Option<u16>;
            let off_dur: (u64, u64);
            if vs.attack {
                if vs.timer != 0 {
                    vs.timer -= 1;
                    off_dur = (719, 903);
                } else if vs.att == 0 {
                    vs.attack = false;
                    vs.vol = 0x40;
                    vs.timer = vs.rel;
                    off_dur = (858, 1042);
                } else {
                    vs.timer = vs.att;
                    vs.vol += 8;
                    if vs.vol >= 0x40 {
                        vs.attack = false;
                        vs.vol = 0x40;
                        vs.timer = vs.rel;
                        off_dur = (941, 1125);
                    } else {
                        off_dur = (835, 1019);
                    }
                }
                vol_write = Some(vs.vol);
            } else if vs.rel == 0 {
                // sustain: the silent sample, no volume write
                st.at(hw, t + 433 + eo);
                hw.loc(v, self.silent);
                st.at(hw, t + 453 + eo);
                hw.len(v, 4);
                t += 524 + eo;
                continue;
            } else if vs.timer != 0 {
                vs.timer -= 1;
                vol_write = Some(vs.vol);
                off_dur = (819, 1003);
            } else {
                vs.timer = vs.rel;
                vs.vol = vs.vol.saturating_sub(8);
                if vs.vol == 0 {
                    // the note ends
                    vs.active = false;
                    let k = 55 * v as u64;
                    st.at(hw, t + 742 + k + eo);
                    hw.dmacon(1 << v);
                    st.at(hw, t + 762 + k + eo);
                    hw.dmacon(0x8200);
                    st.at(hw, t + 1168 + k + VO[v] + eo);
                    hw.vol(v, 0);
                    t += 1352 + k + VD[v] + eo;
                    continue;
                }
                vol_write = Some(vs.vol);
                off_dur = (949, 1133);
            }
            if let Some(vol) = vol_write {
                st.at(hw, t + off_dur.0 + VO[v] + eo);
                hw.vol(v, vol);
            }
            t += off_dur.1 + VD[v] + eo;
        }
        t
    }

    /// Spec 3.6 and 5.4: the pending loop reloads, from cursor `t`.
    fn reloads(&mut self, hw: &mut Hw, st: &Stamp, mut t: u64) {
        for v in 0..4 {
            if let Some((lc, len)) = self.v[v].reload.take() {
                st.at(hw, t + 273);
                hw.loc(v, lc);
                st.at(hw, t + REL_LEN_AT[v]);
                hw.len(v, len);
                t += 278 + 320 + 62 * v as u64;
            } else {
                t += 278;
            }
        }
    }
}

impl Replayer for SpeedySystem {
    fn format(&self) -> &'static str {
        "Speedy System"
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

/// The start-up scan's cost in cycles and the number of populated sample slots (spec 6.1).
fn scan_cost(d: &[u8], mk: usize) -> u64 {
    let ptr = |v: usize, p: usize| be32(d, mk + v * 0x400 + 4 * p);
    let p3 = ptr(3, 255);
    let mut k = 0usize;
    let mut j = 3usize;
    'scan: for p in (0..256).rev() {
        for &v in &[3usize, 2, 1, 0] {
            if ptr(v, p) != p3 {
                j = v;
                break 'scan;
            }
        }
        // every pointer matched: a further identical position
        k += 1;
        j = 0;
    }
    let a: i64 = match j {
        3 => -31,
        2 => 0,
        1 => 31,
        _ => 62,
    };
    let mut lead = 0;
    while lead < 50 && be32(d, mk + 0x1000 + 4 * lead) & 0x8000_0000 != 0 {
        lead += 1;
    }
    let np = (0..50).filter(|&i| be32(d, mk + 0x1000 + 4 * i) & 0x8000_0000 == 0).count();
    let speed = byte(d, mk - 1);
    let q = 2162 + 136 * k as i64 + a + 39 * lead as i64 + 8 * np as i64 - if speed != byte(d, mk - 2) { 2 } else { 0 };
    q.max(0) as u64
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let mk = locate(d)?;
    let silent = m.alloc(16);
    if silent == 0 {
        return None;
    }
    let q2 = scan_cost(d, mk) / 2;
    // the reference's start-up counts from the run's origin, which is where the engine's aligned
    // start (START_PHASE into a video frame) is START_PHASE colour clocks later
    let t = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    {
        let mut hw = Hw { bus: &mut m.bus };
        let offs = [497u64, 629, 773, 912];
        for round in 0..2u64 {
            for (v, off) in offs.iter().enumerate() {
                hw.bus.clock = t + q2 + off + 664 * round;
                hw.vol(v, 0);
            }
        }
    }
    // the host starts its timer when the player's start-up returns, one period before the first
    // tick; the song's audio starts counting from that moment (the engine clears what Paula made
    // before), so the clock is left there and the first tick follows after one period
    let e0 = E0_BASE + q2;
    m.bus.clock = (t + e0 - TICK).max(m.bus.clock);
    Some(Box::new(SpeedySystem::build(d, mk, base, silent)))
}

impl SpeedySystem {
    fn build(d: &[u8], mk: usize, base: u32, silent: u32) -> Self {
        SpeedySystem {
            d: d.to_vec(),
            base,
            mk,
            speed: byte(d, mk - 1),
            silent,
            counter: 0,
            row: 0,
            pos: 0,
            v: [Voice::default(); 4],
            songend: false,
        }
    }
}

#[cfg(test)]
#[path = "speedysystem_tests.rs"]
mod tests;
