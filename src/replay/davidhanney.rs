//! David Hanney (`dh.*`), the music of Tearaway Thomas (1992).
//!
//! Clean-room, written from a behavioural specification and black-box runs of the
//! 68k player's register writes; no original code. DATA: the player's 16 instrument
//! records (waves, vibrato tables, envelopes, sweep settings) and its three PCM
//! samples are not part of a module; they are taken from the original player
//! (David Hanney, 1992) and embedded as `davidhanney_data.bin` with the user's
//! decision, not from the specification.
//!
//! A module is a chunk container (`DSNG`, `SEQU`, `INFO`, `BLK`+n) holding only
//! the order and the patterns: four voices, three bytes (note, instrument,
//! effect) a row, one tick a video frame, `speed + 1` ticks a row. Synth
//! instruments loop a 32-byte wave that a pulse sweep modifies in place; three
//! instruments are PCM samples. The register writes are spaced in time by the
//! specification's integer-cycle model (pair costs plus event costs).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "David Hanney", detect, new };

static DATA: &[u8] = include_bytes!("davidhanney_data.bin");

/// Sample lengths in words; the data holds the three samples, then an 8-byte chunk
/// header, then the instrument bank.
const SMP_WORDS: [usize; 3] = [691, 213, 1110];
const SMP_BYTES: usize = 2 * (691 + 213 + 1110);
const BANK_OFF: usize = SMP_BYTES + 8;
const REC: usize = 0x5E;

const PERIODS: [u16; 69] = [
    6848, 6464, 6096, 5760, 5424, 5120, 4832, 4560, 4304, 4064, 3840, 3616, 3424, 3232, 3048, 2880, 2712, 2560, 2416,
    2280, 2152, 2032, 1920, 1808, 1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 904, 856, 808, 762,
    720, 678, 640, 604, 564, 538, 508, 480, 452, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202,
    190, 180, 170, 160, 151, 143, 135,
];

const ARP: [[u8; 12]; 14] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [6, 5, 4, 3, 2, 1, 1, 2, 3, 4, 5, 6],
    [0, 4, 7, 0, 4, 7, 0, 4, 7, 0, 4, 7],
    [0, 5, 9, 0, 5, 9, 0, 5, 9, 0, 5, 9],
    [0, 3, 8, 0, 3, 8, 0, 3, 8, 0, 3, 8],
    [12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1],
    [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
    [0, 0, 12, 12, 0, 0, 12, 12, 0, 0, 12, 12],
    [6, 5, 4, 3, 2, 1, 6, 5, 4, 3, 2, 1],
    [1, 2, 3, 4, 5, 6, 1, 2, 3, 4, 5, 6],
    [0, 5, 7, 0, 5, 7, 0, 5, 7, 0, 5, 7],
    [0, 5, 8, 0, 5, 8, 0, 5, 8, 0, 5, 8],
    [0, 4, 7, 0, 4, 7, 0, 4, 7, 0, 4, 7],
    [0, 3, 7, 0, 3, 7, 0, 3, 7, 0, 3, 7],
];

/// The period of table index `idx`. Past the 69 notes (the last is 135) the reference reads
/// what follows its table in memory, which the logs show to be one more word (127), a row of
/// six zero words, then the arpeggio table as big-endian words (rows 14 and 15 zero).
fn period(idx: usize) -> u16 {
    match idx {
        0..=68 => PERIODS[idx],
        69 => 127,
        _ => {
            let k = 2 * (idx - 70);
            let g = |j: usize| ARP.get(j / 12).map_or(0, |r| r[j % 12]) as u16;
            g(k) << 8 | g(k + 1)
        }
    }
}

/// The chunks of a module: (id, data offset, length).
fn chunks(d: &[u8]) -> Option<Vec<([u8; 4], usize, usize)>> {
    if d.get(..4)? != b"DSNG" {
        return None;
    }
    let mut v = Vec::new();
    let mut p = 4;
    while p + 8 <= d.len() {
        let id = [d[p], d[p + 1], d[p + 2], d[p + 3]];
        if id == [0; 4] {
            break;
        }
        let len = be32(d, p + 4) as usize;
        if p + 8 + len > d.len() {
            return None;
        }
        v.push((id, p + 8, len));
        p += 8 + len;
    }
    Some(v)
}

fn detect(d: &[u8]) -> bool {
    // the first four chunk ids are compared, the lengths of the first three find the next chunk
    if d.get(..4) != Some(b"DSNG") {
        return false;
    }
    let mut p = 4usize;
    for (i, id) in [&b"SEQU"[..], b"INFO", b"BLK\0", b"BLK\x01"].iter().enumerate() {
        if d.get(p..p + 4) != Some(*id) {
            return false;
        }
        if i < 3 {
            p = match (p + 8).checked_add(be32(d, p + 4) as usize) {
                Some(n) => n,
                None => return false,
            };
        }
    }
    true
}

#[derive(Clone, Copy, Default)]
struct Voice {
    /// 0 idle, 4 start pending, 8 running
    state: u8,
    note: u8,
    ins: u8,
    fx: u8,
    arp: u8,
    arp_step: u8,
    porta: u8,
    target: u16,
    cur: u16,
    vib_delay: u8,
    vib_phase: u8,
    level: u8,
    stage: u8,
    env_cnt: u8,
    env_speed: u8,
    retrig: u8,
    base: u8,
    inst: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum K {
    Start,
    DmaOn,
    DmaOff,
    Loc,
    Len,
    Vol,
    Per,
}

pub struct DavidHanney {
    chunks: Vec<([u8; 4], usize, usize)>,
    d: Vec<u8>,
    seq: usize,
    songlen: usize,
    speed: u8,
    pos: usize,
    row: usize,
    cd: u8,
    stamp: u8,
    v: [Voice; 4],
    bank: Vec<u8>,
    /// guest address of the samples (then the bank)
    mem: u32,
    songend: bool,
    first_k: i64,
    // write timing
    pend: i64,
    prev: K,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let ch = chunks(d)?;
    let seq = ch.iter().find(|c| &c.0 == b"SEQU")?.1;
    let info = ch.iter().find(|c| &c.0 == b"INFO")?.1;
    if DATA.len() < BANK_OFF + 16 * REC {
        return None;
    }
    let mem = m.alloc((BANK_OFF + 16 * REC) as u32);
    if mem == 0 {
        return None;
    }
    let mut bank = DATA[BANK_OFF..BANK_OFF + 16 * REC].to_vec();
    // sanitise the synth records once (the bank is not re-initialised later)
    for r in 0..16 {
        let b = &mut bank[r * REC..(r + 1) * REC];
        if be16(b, 0) != 0 {
            continue;
        }
        b[0x48] = b[0x48].min(16);
        b[0x49] = b[0x49].min(16);
        b[0x46] = b[0x46].min(31);
        b[0x47] = b[0x47].min(31);
        for s in 0..4 {
            b[0x3A + 2 * s] = b[0x3A + 2 * s].min(63);
        }
        if b[0x46] > b[0x47] {
            b.swap(0x46, 0x47);
        }
        if b[0x5A] < b[0x46] || b[0x5A] > b[0x47] {
            b[0x5A] = b[0x46];
        }
        if b[0x5C] > b[0x45] {
            b[0x5C] = 0;
        }
        if b[0x5B] != 1 && b[0x5B] != 0xFF {
            b[0x5B] = 1;
        }
    }
    m.bus.write_bytes(mem, &DATA[..SMP_BYTES]);
    m.bus.write_bytes(mem + BANK_OFF as u32, &bank);
    let first_blk = d.get(seq).copied().unwrap_or(0);
    let first_k = ch.iter().position(|c| c.0 == [b'B', b'L', b'K', first_blk]).unwrap_or(2) as i64;
    let r = DavidHanney {
        d: d.to_vec(),
        seq,
        songlen: be16(d, info) as usize,
        speed: (be16(d, info + 4) & 0xFF) as u8,
        chunks: ch,
        pos: 0,
        row: 0,
        cd: 0,
        stamp: 0,
        v: [Voice { env_speed: 2, ..Default::default() }; 4],
        bank,
        mem,
        songend: false,
        first_k,
        pend: 0,
        prev: K::Start,
    };
    // the start call: DMA off, then the four volumes
    let mut hw = Hw { bus: &mut m.bus };
    // the reference host's start phase within the raster line (the engine starts at START_PHASE)
    hw.wait(201);
    hw.dmacon(0x000F);
    for c in 0..4 {
        hw.wait(if c == 0 { 105 } else { 222 });
        hw.vol(c, 0);
    }
    hw.wait(lead());
    Some(Box::new(r))
}

/// Colour clocks the reference's audio output starts after its start call's DMA write; the
/// engine's output starts after `new`, so the start call is followed by this wait and the first
/// tick's delay is shortened by it (measured on the output's start, not derived).
fn lead() -> u64 {
    4581
}

impl DavidHanney {
    fn rec(&self, i: usize) -> &[u8] {
        &self.bank[i * REC..(i + 1) * REC]
    }

    /// Cycles from the previous write kind to this one (the specification's table; pairs it
    /// does not list take a typical value of the kind).
    fn pair(prev: K, k: K) -> i64 {
        use K::*;
        match (prev, k) {
            (Start, Per) => 439,
            (Start, DmaOn) => 302,
            (Start, DmaOff) => 633,
            (Per, Vol) => 267,
            (Vol, DmaOn) => 253,
            (DmaOn, Per) => 464,
            (DmaOn, DmaOn) => 327,
            (DmaOn, Len) => 242,
            (DmaOn, DmaOff) => 658,
            (Len, DmaOff) => 674,
            (Len, Per) => 480,
            (Len, DmaOn) => 343,
            (Per, DmaOff) => 678,
            (Per, Per) => 483,
            (DmaOff, DmaOff) => 686,
            (DmaOff, Loc) => 182,
            (Loc, Len) => 210,
            (Len, Vol) => 190,
            (Vol, Per) => 186,
            (DmaOff, Per) => 492,
            (_, DmaOff) => 660,
            (_, DmaOn) => 300,
            (_, Per) => 460,
            (_, Loc) => 182,
            (_, Len) => 210,
            (_, Vol) => 200,
            (_, Start) => 0,
        }
    }

    /// Lets the time of the write pass: the pair cost plus the events since the last write.
    fn at(&mut self, hw: &mut Hw, k: K) {
        let c = (Self::pair(self.prev, k) + self.pend).max(0);
        self.pend = 0;
        hw.wait((c / 2) as u64);
        self.prev = k;
    }

    fn dmaon(&mut self, hw: &mut Hw, ch: usize) {
        self.at(hw, K::DmaOn);
        hw.dmacon(0x8000 | 1 << ch);
        // the host call returns ten video lines after the line of the write
        let t = hw.bus.clock;
        hw.bus.clock = (t / 227 + 10) * 227;
    }

    fn dmaoff(&mut self, hw: &mut Hw, ch: usize) {
        self.at(hw, K::DmaOff);
        hw.dmacon(1 << ch);
    }

    fn block(&self, n: u8) -> Option<(usize, usize, usize)> {
        let i = self.chunks.iter().position(|c| c.0 == [b'B', b'L', b'K', n])?;
        let c = self.chunks[i];
        Some((i, c.1, c.2))
    }

    fn decode(&mut self, hw: &mut Hw, i: usize) {
        let v = self.v[i];
        if v.note == 0 && v.ins == 0 && v.fx == 0 {
            self.pend += 89;
            return;
        }
        self.v[i].porta = 0;
        let p = v.fx & 15;
        match v.fx >> 4 {
            1 => {
                self.pend += 265;
                self.v[i].porta = p + 1;
                self.v[i].target = period(v.note as usize);
                return;
            }
            2 => {
                self.pend += 79;
                self.v[i].env_speed = p;
            }
            3 => {
                self.pend += 213;
                self.v[i].arp = p;
            }
            4 => {
                self.pend += 103;
                self.v[i].retrig = self.speed.wrapping_add(1) >> 1;
            }
            _ => {}
        }
        match v.note {
            0 => self.pend += 181,
            1 => {
                self.pend -= 237;
                self.dmaoff(hw, i);
                self.v[i].state = 0;
            }
            2 => {}
            n => {
                let idx = (n - 2) as usize;
                let per = period(idx);
                self.v[i].base = idx as u8;
                self.v[i].cur = per;
                let ins = (v.ins & 15) as usize;
                self.v[i].inst = ins;
                let ty = be16(self.rec(ins), 0);
                if ty == 0 {
                    self.dmaoff(hw, i);
                    self.at(hw, K::Loc);
                    hw.loc(i, self.mem + (BANK_OFF + ins * REC + 0x0A) as u32);
                    let wl = self.rec(ins)[0x48] as u16;
                    self.at(hw, K::Len);
                    hw.len(i, wl);
                    let vd = self.rec(ins)[0x43];
                    let vo = &mut self.v[i];
                    vo.state = 8;
                    vo.level = 0;
                    vo.stage = 0;
                    vo.env_cnt = 0;
                    vo.vib_delay = vd;
                    vo.vib_phase = 0;
                } else {
                    let s = (ty.min(3) - 1) as usize;
                    self.pend += [-39, -6, 14][s];
                    self.dmaoff(hw, i);
                    self.pend -= 7;
                    let off: usize = SMP_WORDS[..s].iter().map(|w| 2 * w).sum();
                    self.at(hw, K::Loc);
                    hw.loc(i, self.mem + off as u32);
                    self.pend -= 7;
                    self.at(hw, K::Len);
                    hw.len(i, SMP_WORDS[s] as u16);
                    self.at(hw, K::Vol);
                    hw.vol(i, 63);
                    self.at(hw, K::Per);
                    hw.per(i, per);
                    self.v[i].state = 4;
                }
            }
        }
    }

    fn run_step(&mut self, hw: &mut Hw, i: usize) {
        let ins = self.v[i].inst;
        let mut r = [0u8; REC];
        r.copy_from_slice(self.rec(ins));
        // period
        let mut per;
        if self.v[i].porta == 0 {
            let vo = &mut self.v[i];
            let idx = vo.base.wrapping_add(ARP.get(vo.arp as usize).map_or(0, |a| a[vo.arp_step as usize]));
            per = period(idx as usize);
            vo.arp_step += 1;
            if vo.arp_step == 12 {
                vo.arp_step = 0;
                self.pend += 4;
            }
        } else {
            let vo = &mut self.v[i];
            let (cur, tgt, sp) = (vo.cur, vo.target, vo.porta as u16);
            if cur == tgt {
                self.pend -= 50;
            } else {
                if cur > tgt {
                    if cur - tgt <= sp {
                        vo.cur = tgt;
                        self.pend += 31;
                    } else {
                        vo.cur = cur - sp;
                        self.pend += 18;
                    }
                } else if tgt - cur <= sp {
                    vo.cur = tgt;
                    self.pend += 22;
                } else {
                    vo.cur = cur + sp;
                    self.pend += 18;
                }
                vo.stage = 0;
            }
            per = vo.cur;
        }
        // vibrato
        let rate = r[0x42];
        if rate != 0 {
            let vo = &mut self.v[i];
            if vo.vib_delay != 0 {
                vo.vib_delay -= 1;
                self.pend += 47;
            } else {
                let t = *r.get(0x2A + vo.vib_phase as usize).unwrap_or(&0) as i8 as i16;
                per = per.wrapping_add(t.wrapping_mul(r[0x44] as i16) as u16);
                let borrow = vo.vib_phase < rate;
                vo.vib_phase = vo.vib_phase.wrapping_sub(rate);
                if borrow {
                    vo.vib_phase = vo.vib_phase.wrapping_add(r[0x49].wrapping_mul(2));
                    self.pend += 186;
                } else {
                    self.pend += 154;
                }
            }
        }
        self.at(hw, K::Per);
        hw.per(i, per);
        // envelope
        let vo = &mut self.v[i];
        let step_now = vo.env_cnt == 0;
        vo.env_cnt = if step_now { vo.env_speed } else { vo.env_cnt - 1 };
        if step_now {
            let s = vo.stage as usize;
            let target = r[0x3A + s].wrapping_mul(4);
            let step = r[0x3B + s];
            let cur = vo.level;
            let last = s == 6;
            let (new, cost);
            if cur >= target {
                let d = cur - target;
                if d < step {
                    new = target;
                    cost = if last { 161 } else { 177 };
                } else if d == step {
                    new = target;
                    cost = if last { 147 } else { 163 };
                } else {
                    new = cur - step;
                    cost = 133;
                }
            } else {
                let d = target - cur;
                if step > d {
                    new = target;
                    cost = if last { 151 } else { 167 };
                } else if step == d {
                    new = target;
                    cost = if last { 147 } else { 163 };
                } else {
                    new = cur + step;
                    cost = 133;
                }
            }
            vo.level = new;
            if new == target && !last {
                vo.stage += 2;
            }
            self.pend += cost;
        }
        let vol = self.v[i].level >> 2;
        self.at(hw, K::Vol);
        hw.vol(i, vol as u16);
        // pulse sweep
        if r[0x45] != 0 {
            let rb = ins * REC;
            if self.bank[rb + 0x5D] == self.stamp {
                self.pend += 33;
            } else {
                self.bank[rb + 0x5D] = self.stamp;
                let cnt = self.bank[rb + 0x5C];
                self.bank[rb + 0x5C] = cnt.wrapping_sub(1);
                if cnt != 0 {
                    self.pend += 73;
                } else {
                    self.bank[rb + 0x5C] = r[0x45].wrapping_sub(1);
                    let pos = self.bank[rb + 0x5A];
                    let down = self.bank[rb + 0x5B] == 0xFF;
                    let (val, edge) = if down { (0x3F, r[0x46]) } else { (0xC0, r[0x47]) };
                    let a = self.mem + (BANK_OFF + rb + 0x0A) as u32 + pos as u32;
                    self.bank[rb + 0x0A + pos as usize % 32] = val;
                    hw.bus.write_bytes(a, &[val]);
                    if pos == edge {
                        self.bank[rb + 0x5B] = if down { 1 } else { 0xFF };
                        self.pend += 183;
                    } else {
                        self.bank[rb + 0x5A] = if down { pos.wrapping_sub(1) } else { pos.wrapping_add(1) };
                        self.pend += 193;
                    }
                }
            }
        }
        self.dmaon(hw, i);
    }

    fn voice_step(&mut self, hw: &mut Hw, i: usize) {
        match self.v[i].state {
            4 => {
                self.dmaon(hw, i);
                self.at(hw, K::Len);
                hw.len(i, 1);
                self.v[i].state = 0;
            }
            8 => self.run_step(hw, i),
            _ => self.pend += 90,
        }
        if self.v[i].retrig != 0 {
            self.v[i].retrig -= 1;
            if self.v[i].retrig == 0 {
                self.pend += 66;
                self.v[i].fx = 0;
                self.decode(hw, i);
            } else {
                self.pend += 58;
            }
        } else {
            self.pend += 33;
        }
    }

    fn row_phase(&mut self, hw: &mut Hw) {
        let blk = self.d.get(self.seq + self.pos).copied().unwrap_or(0);
        let (k, off, len) = self.block(blk).unwrap_or((0, 0, 0));
        let rows = if len >= 2 { be16(&self.d, off) as usize } else { 0 }.max(1);
        self.pend += 723 + 60 * k as i64;
        let ro = off + 2 + 12 * self.row;
        for i in 0..4 {
            let g = |j: usize| self.d.get(ro + 3 * i + j).copied().unwrap_or(0);
            let (n, s, f) = (g(0), g(1), g(2));
            let vo = &mut self.v[i];
            vo.note = n;
            vo.ins = s;
            vo.fx = f;
        }
        for i in 0..4 {
            self.decode(hw, i);
        }
        if self.row + 1 < rows {
            self.row += 1;
        } else {
            self.pos += 1;
            if self.pos >= self.songlen {
                self.songend = true;
                self.pos = 0;
            }
            self.row = 0;
        }
    }
}

impl Replayer for DavidHanney {
    fn format(&self) -> &'static str {
        "David Hanney"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn timer_delay(&self) -> u64 {
        // the first tick falls 76305 colour clocks after the start call's DMA write, plus
        // 30 for each chunk the walk to the first block passes; the engine adds one period
        // to the time of the last volume write (771 after the DMA write)
        (76305 + 30 * (self.first_k - 2) - 771 - 70940 - lead() as i64).max(0) as u64
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.stamp = self.stamp.wrapping_add(1);
        self.pend = 0;
        self.prev = K::Start;
        for i in 0..4 {
            self.voice_step(hw, i);
        }
        if self.cd == 0 {
            self.cd = self.speed;
            self.row_phase(hw);
        } else {
            self.cd -= 1;
        }
    }
}

#[cfg(test)]
#[path = "davidhanney_tests.rs"]
mod tests;
