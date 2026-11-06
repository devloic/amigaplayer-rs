//! Tronic (the Eagleplayer "Tronic", shown as "TronicTracker(?)"; files such as `crystal.dp`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! and execution-time histograms of the 68k reference player; no original code. A module is a
//! pure data file with no magic text: four sequence lists, then seven blocks (arpeggio lists,
//! instrument offset table, instrument records, wave table index, wave data, patterns, sample
//! table) and the sample area. Recognition is structural (the spec's rule: the block sizes chain
//! to a place where the first two instrument offsets read 0x0058 and 0x00B0).
//!
//! Four voices, one Paula channel each. A tick steps a speed counter, then every voice: loop set
//! of a sample instrument, row processing, wave list, vibrato, envelope, portamento, arpeggio,
//! period and volume. Every register write is placed at the clock of the original's code (the
//! spec's static 68000 cycle costs of every step, the helper blocks per channel).
//!
//! Choices where the specification leaves the behaviour undefined (never a panic or a hang):
//! - a period table index above 85 reads 0 (the reference read 0 there in the synthetic run that hit it);
//! - a pointer that leaves the file reads zero bytes (the spec says a read past the end of the
//!   file returned 0 in the reference);
//! - effect numbers 9 and above are no effect (no cost);
//! - an arpeggio list whose first byte is 0x80 reads the entry as 0 after the restart (the
//!   original never ends the list);
//! - a module whose blocks do not fit in the file is not recognised.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Tronic", detect, new };

/// Colour clocks from the last write of the sound start to the first tick (measured in the
/// reference host's runs: the first tick starts at cc 73 949, the sound start's last write is at 1 869,
/// the first write at 466).
const FIRST_TICK_AFTER_INIT: u64 = 73949 - 1869;
const TICK_CLOCKS: u64 = 70940;
/// Colour clocks before the first volume write: the reference host first write is at cc 466 and the
/// engine hands a replayer the clock at 100, so the logs then agree in absolute time.
const START_OFFSET: u64 = 366;

/// The player-resident period table (spec 1.6), indices 0 to 85.
const PERIODS: [u16; 86] = [
    0, 6848, 6464, 6096, 5760, 5424, 5120, 4832, 4560, 4304, 4064, 3840, 3616, //
    3424, 3232, 3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1808, //
    1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 904, //
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 452, //
    428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, //
    214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, //
    113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 113, 0, 113,
];

/// The default instrument (spec 1.6): 46 bytes.
const DEFAULT_INST: [u8; 46] = {
    let mut r = [0u8; 46];
    r[1] = 4; // length: 4 words
    // envelope phase 0: rate 0, target 1, hold 255
    r[7] = 1;
    r[8] = 255;
    // vibrato phase 0: speed 0, length 1, delay 1
    r[0x16] = 1;
    r[0x17] = 1;
    // vibrato phases 1 to 4: speed 2, length 2, delay 2
    let mut p = 1;
    while p < 5 {
        r[0x15 + 3 * p] = 2;
        r[0x15 + 3 * p + 1] = 2;
        r[0x15 + 3 * p + 2] = 2;
        p += 1;
    }
    r[0x27] = 254; // wave step interval
    r[0x29] = 0xFF; // wave list: 0, marker, 0
    r
};

// ---------------------------------------------------------------------------------
// Layout and recognition

/// Offsets of the parts of a module (data starts: after the size words).
#[derive(Clone, Copy, Debug)]
struct Lay {
    seq: [usize; 4],
    seqlen: [u16; 4],
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    e: usize,
    f: usize,
    g: usize,
    s0: usize,
}

fn s16(d: &[u8], o: usize) -> i64 {
    be16(d, o) as i16 as i64
}

fn layout(d: &[u8]) -> Option<Lay> {
    let n = d.len();
    if n < 20 {
        return None;
    }
    let mut len = [0u16; 4];
    for (v, l) in len.iter_mut().enumerate() {
        if be16(d, v * 4) != 0 {
            return None;
        }
        *l = be16(d, v * 4 + 2);
        if *l & 0x8001 != 0 {
            return None;
        }
    }
    let sum: usize = len.iter().map(|&l| l as usize).sum();
    let asz = 16 + sum;
    if asz + 2 > n {
        return None;
    }
    let sa = be16(d, asz) as usize;
    let dd = asz + sa;
    if dd % 2 != 0 || dd + 8 > n || be32(d, dd + 4) != 0x0058_00B0 {
        return None;
    }
    let mut seq = [0usize; 4];
    let mut p = 16;
    for v in 0..4 {
        seq[v] = p;
        p += len[v] as usize;
    }
    let a = asz + 2;
    let bsz = a + sa;
    if bsz + 2 > n {
        return None;
    }
    let sb = be16(d, bsz) as usize;
    let b = bsz + 2;
    if sb < 2 || b + sb > n {
        return None;
    }
    let last_b = be16(d, b + sb - 2) as usize;
    let c = b + sb;
    let dsz = c + last_b + 0x58;
    if dsz + 2 > n {
        return None;
    }
    let sd = be16(d, dsz) as usize;
    let dd = dsz + 2;
    let esz = dd + sd;
    if esz + 2 > n {
        return None;
    }
    let se = be16(d, esz) as usize;
    let e = esz + 2;
    let fsz = e + se;
    if fsz + 4 > n {
        return None;
    }
    let sf = be32(d, fsz) as usize;
    let f = fsz + 4;
    let g = f.checked_add(sf)?;
    if g.checked_add(32)? > n {
        return None;
    }
    Some(Lay { seq, seqlen: len, a, b, c, d: dd, e, f, g, s0: g + 32 })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

// ---------------------------------------------------------------------------------
// State

#[derive(Clone, Default)]
struct Voice {
    seqpos: u16,
    seqend: u16,
    row: u16,
    pat: usize,
    /// The current instrument record's offset in the file; `None` is the default instrument.
    inst: Option<usize>,
    pend: u8,
    wwait: u8,
    wpos: u8,
    cur: u16,
    tgt: u16,
    note: u8,
    trans: u8,
    maxvol: u8,
    slide: u16,
    level: u16,
    epos: u16,
    ehold: u8,
    pspeed: u8,
    vdir: u8,
    vacc: u16,
    vcnt: u8,
    vphase: u8,
    vdelay: u8,
    abase: usize,
    aidx: u16,
}

pub struct Tronic {
    d: Vec<u8>,
    base: u32,
    lay: Lay,
    silent: u32,
    v: [Voice; 4],
    speed: u8,
    counter: i8,
    master: u8,
    /// The scratch word of the spec (7.3).
    sc: u32,
    songend: bool,
    // timing: stamp = ts + tb + t / 2
    ts: u64,
    tb: u64,
    t: u64,
}

const HLC: [u64; 4] = [105, 136, 167, 175];
const HLEN: [u64; 4] = [101, 132, 163, 171];
const VOLW: [u64; 4] = [195, 218, 241, 231];

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    let silent = m.alloc(8);
    if silent == 0 {
        return None;
    }
    m.bus.write_bytes(silent, &[0; 8]);
    let mut s = Tronic {
        d: d.to_vec(),
        base,
        lay,
        silent,
        v: Default::default(),
        speed: 5,
        counter: 0,
        master: 0x3F,
        sc: 0,
        songend: false,
        ts: 0,
        tb: 0,
        t: 0,
    };
    for c in 0..4 {
        s.v[c] = Voice {
            seqend: lay.seqlen[c],
            maxvol: 0x3F,
            abase: lay.a,
            ..Default::default()
        };
    }
    let mut hw = Hw { bus: &mut m.bus };
    s.start_sequence(&mut hw);
    Some(Box::new(s))
}

impl Tronic {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn rec(&self, v: &Voice, off: usize) -> u8 {
        match v.inst {
            None => DEFAULT_INST.get(off).copied().unwrap_or(0),
            Some(o) => self.rb(o.wrapping_add(off)),
        }
    }

    fn rec16(&self, v: &Voice, off: usize) -> u16 {
        (self.rec(v, off) as u16) << 8 | self.rec(v, off + 1) as u16
    }

    /// The host's start (spec 6.2): the volumes twice, then the sound start.
    fn start_sequence(&mut self, hw: &mut Hw) {
        let ts = hw.bus.clock + START_OFFSET;
        for (i, off) in [0u64, 156, 324, 486, 692, 848, 1016, 1178].iter().enumerate() {
            hw.bus.clock = ts + off;
            hw.vol(i & 3, 0);
        }
        let t0 = ts + 1301;
        hw.bus.clock = t0;
        hw.dmacon(0x000F);
        for c in 0..4 {
            hw.bus.clock = t0 + 12 + 8 * c as u64;
            hw.loc(c, self.silent);
        }
        for c in 0..4 {
            hw.bus.clock = t0 + 44 + 8 * c as u64;
            hw.len(c, 4);
        }
        for c in 0..4 {
            hw.bus.clock = t0 + 78 + 8 * c as u64;
            hw.per(c, 0);
            hw.vol(c, 0);
        }
        // the song's audio counts from one period before the first tick (the host starts its
        // timer when the start returns), as the reference's wav origin does
        hw.bus.clock = t0 + 102 + (FIRST_TICK_AFTER_INIT - TICK_CLOCKS);
    }

    // --- the clock

    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.ts + self.tb + self.t / 2;
    }

    fn write_len(&mut self, hw: &mut Hw, c: usize, words: u16) {
        self.at(hw);
        hw.len(c, words);
    }

    fn write_lc(&mut self, hw: &mut Hw, c: usize, addr: u32) {
        self.at(hw);
        hw.loc(c, addr);
    }

    /// The guest address of the sample of the instrument (spec 4.1).
    fn sample_start(&self, v: &Voice) -> u32 {
        let k = self.rec(v, 0x27) as usize;
        let off = be32(&self.d, self.lay.g + 4 * k);
        self.base.wrapping_add(self.lay.s0 as u32).wrapping_add(off)
    }

    fn period_of(idx: u8) -> u16 {
        match PERIODS.get(idx as usize) {
            Some(&p) => p,
            // Past the table the original reads whatever follows it. The only index seen to
            // reach there in the reference runs, 254 (note + transpose wrapping a byte), reads
            // 0x8000 (the logged period is then 0x8000 + the slide); the rest are unknown (0).
            None if idx == 254 => 0x8000,
            None => 0,
        }
    }

    // --- one voice

    fn voice(&mut self, hw: &mut Hw, c: usize) {
        let mut v = std::mem::take(&mut self.v[c]);
        self.t += 49;
        // 4.1 loop set
        if v.pend & 0x80 != 0 {
            v.pend = 0;
            self.t += 118;
            let w = self.rec16(&v, 0x04);
            self.write_len(hw, c, w);
            self.t += HLEN[c];
            self.t += 48;
            let ss = self.sample_start(&v);
            // the first word of the sample goes silent
            hw.bus.write_bytes(ss, &[0, 0]);
            let o = ss.wrapping_sub(self.base) as usize;
            if o < self.d.len() && o + 2 <= self.d.len() {
                self.d[o] = 0;
                self.d[o + 1] = 0;
            }
            let lc = ss.wrapping_add(self.rec16(&v, 0x02) as u32);
            self.write_lc(hw, c, lc);
            self.t += HLC[c];
        }
        self.t += 23;
        if self.counter == 0 {
            self.row(hw, c, &mut v);
        }
        self.wave_list(hw, c, &mut v);
        self.vibrato(&mut v);
        self.envelope(&mut v);
        self.portamento(&mut v);
        let base = self.arpeggio(&mut v);
        // 5.7 period
        self.t += 64;
        let d = self.rec16(&v, 0x24).wrapping_sub(v.slide);
        self.sc = self.sc & 0xFFFF_0000 | d as u32;
        v.vacc = v.vacc.wrapping_sub(d);
        let period = base.wrapping_add(v.vacc);
        self.at(hw);
        hw.per(c, period);
        self.t += HLEN[c];
        // 5.8 volume
        self.t += 51;
        let mut vol = ((v.level >> 2) & 0x3F) as u8;
        if vol.wrapping_sub(v.maxvol) & 0x80 == 0 {
            vol = v.maxvol;
            self.t += 12;
        }
        self.t += 21;
        if vol.wrapping_sub(self.master) & 0x80 == 0 {
            vol = self.master;
            self.t += 12;
        }
        self.t += VOLW[c];
        self.at(hw);
        hw.vol(c, (vol & 0x7F) as u16);
        self.t += 44 + 40;
        self.v[c] = v;
    }

    /// 4.2 to 4.4 row processing.
    fn row(&mut self, hw: &mut Hw, c: usize, v: &mut Voice) {
        self.t += 21;
        if v.row == 0 {
            self.t += 159;
            let p = self.lay.seq[c] + v.seqpos as usize;
            let (pn, tr) = (self.rb(p), self.rb(p + 1));
            v.trans = tr;
            v.pat = self.lay.f.wrapping_add(64 * pn as usize);
            self.sc = self.sc & 0xFFFF_0000 | v.seqpos.wrapping_add(2) as u32;
            v.seqpos = v.seqpos.wrapping_add(2);
            if v.seqpos.wrapping_sub(v.seqend) & 0x8000 == 0 {
                v.seqpos = 0;
                self.t += 92;
                self.t += 16;
                self.tb += self.t / 2;
                self.t = 0;
                self.songend = true;
            }
        }
        self.t += 63;
        let r = v.pat.wrapping_add(v.row as usize);
        let (note, ins, eff, par) = (self.rb(r), self.rb(r + 1), self.rb(r + 2), self.rb(r + 3));
        if note != 0 {
            self.new_note(hw, c, v, note, ins);
        }
        self.t += 29;
        if (1..=8).contains(&eff) {
            self.t += 30;
            self.effect(v, eff, par);
        }
        self.t += 36;
        v.row = (v.row + 4) % 64;
    }

    fn new_note(&mut self, hw: &mut Hw, c: usize, v: &mut Voice, note: u8, ins: u8) {
        self.t += 24;
        self.at(hw);
        hw.dmacon(1 << c);
        self.t += 185;
        v.note = note;
        v.tgt = Self::period_of(note.wrapping_add(v.trans));
        let s = (ins as u16).wrapping_mul(2).wrapping_sub(2);
        self.sc = s as u32;
        let o = self.lay.b as i64 + s as i16 as i64;
        let off = if o < 0 { 0 } else { be16(&self.d, o as usize) as usize };
        v.inst = Some(self.lay.c.wrapping_add(off));
        v.pend = self.rec(v, 0x26);
        if v.pend & 0x80 != 0 {
            self.t += 90;
            let ss = self.sample_start(v);
            self.write_lc(hw, c, ss);
            self.t += HLC[c];
            self.t += 20;
            let w = self.rec16(v, 0x00);
            self.write_len(hw, c, w);
            self.t += HLEN[c];
        }
        self.t += 152;
        v.wwait = 0;
        v.wpos = 0;
        v.level = 0;
        v.epos = 0;
        v.ehold = 0;
        v.aidx = 0;
        v.vdir = 0;
        v.vacc = 0;
        v.vcnt = self.rec(v, 0x16);
        v.vphase = 0;
        v.vdelay = self.rec(v, 0x17);
    }

    fn effect(&mut self, v: &mut Voice, eff: u8, par: u8) {
        self.sc = self.sc & 0xFFFF_FF00 | par as u32;
        match eff {
            1 => {
                self.t += 32;
                self.speed = par;
            }
            2 => self.t += 33,
            3 => {
                self.t += 36;
                let w = (self.sc as u16).wrapping_neg();
                self.sc = self.sc & 0xFFFF_0000 | w as u32;
                v.slide = w;
            }
            4 => {
                self.t += 32;
                v.slide = self.sc as u16;
            }
            5 => {
                self.t += 32;
                v.pspeed = par;
            }
            6 => {
                self.t += 32;
                v.maxvol = par;
            }
            7 => {
                self.t += 32;
                self.master = par;
            }
            _ => {
                self.t += 66;
                self.sc = self.sc.wrapping_shl(4);
                v.abase = self.lay.a.wrapping_add(self.sc as usize);
            }
        }
    }

    /// 5.2 the wave list of a wave-list instrument.
    fn wave_list(&mut self, hw: &mut Hw, c: usize, v: &mut Voice) {
        self.t += 21;
        if self.rec(v, 0x26) & 0x80 != 0 {
            return;
        }
        self.t += 21;
        if v.wwait != 0 {
            self.t += 26;
            v.wwait -= 1;
            return;
        }
        self.t += 48;
        v.wwait = self.rec(v, 0x27);
        let mut i = v.wpos as usize;
        self.sc = self.sc & 0xFFFF_FF00 | i as u32;
        self.t += 35;
        if self.rec(v, 0x28 + i) == 0xFF {
            self.t += 45;
            i = self.rec(v, 0x28 + i + 1) as usize;
            self.sc = self.sc & 0xFFFF_FF00 | i as u32;
            if self.rec(v, 0x28 + i) == 0xFF {
                self.t += 10;
                return;
            }
            self.t += 35;
        }
        let w = self.rec(v, 0x28 + i) as usize;
        let o = s16(&self.d, self.lay.d + 2 * w);
        let lc = self.base.wrapping_add(self.lay.e as u32).wrapping_add(o as u32);
        self.t += 86;
        self.write_lc(hw, c, lc);
        self.t += HLC[c];
        self.t += 32;
        let len = self.rec16(v, 0x00);
        self.write_len(hw, c, len);
        self.t += HLEN[c];
        self.t += 16;
        v.wpos = (i as u8).wrapping_add(1);
        self.sc = self.sc & 0xFFFF_FF00 | v.wpos as u32;
    }

    /// 5.3 vibrato.
    fn vibrato(&mut self, v: &mut Voice) {
        self.t += 61;
        let ph = v.vphase as usize;
        let speed = self.rec(v, 0x15 + ph) as u16;
        if v.vdir == 0 {
            self.t += 26;
            v.vacc = v.vacc.wrapping_add(speed);
        } else {
            self.t += 16;
            v.vacc = v.vacc.wrapping_sub(speed);
        }
        self.t += 25;
        v.vcnt = v.vcnt.wrapping_sub(1);
        if v.vcnt == 0 {
            self.t += 36;
            v.vcnt = self.rec(v, 0x16 + ph);
            v.vdir = 0xFFu8.wrapping_sub(v.vdir);
        }
        self.t += 21;
        if v.vdelay != 0 {
            self.t += 26;
            v.vdelay -= 1;
        } else {
            self.t += 41;
            let old = v.vphase as usize;
            v.vphase = v.vphase.wrapping_add(3);
            if v.vphase == 15 {
                self.t += 16;
                v.vphase = 12;
            }
            self.t += 20;
            v.vdelay = self.rec(v, 0x15 + old + 5);
        }
    }

    /// 5.4 envelope.
    fn envelope(&mut self, v: &mut Voice) {
        self.t += 21;
        if v.ehold != 0 {
            self.t += 26;
            v.ehold -= 1;
            return;
        }
        self.t += 77;
        let e = 0x06 + v.epos as usize;
        let rate = self.rec(v, e) as u16;
        let target = self.rec(v, e + 1) as u16;
        let hold = self.rec(v, e + 2);
        self.sc = target as u32;
        if target.wrapping_sub(v.level) & 0x8000 == 0 {
            self.t += 37;
            v.level = v.level.wrapping_add(rate);
            if target.wrapping_sub(v.level) & 0x8000 != 0 {
                self.t += 53;
                v.level = target;
                v.epos = v.epos.wrapping_add(3);
                if v.epos == 15 {
                    self.t += 16;
                    v.epos = 12;
                }
                self.t += 20;
                v.ehold = hold;
            }
        } else {
            self.t += 37;
            v.level = v.level.wrapping_sub(rate);
            if target.wrapping_sub(v.level) & 0x8000 == 0 {
                self.t += 58;
                v.level = target;
                v.epos = v.epos.wrapping_add(3);
                v.ehold = hold;
            }
        }
    }

    /// 5.5 portamento.
    fn portamento(&mut self, v: &mut Voice) {
        self.t += 25;
        if v.pspeed == 0 {
            return;
        }
        self.t += 33;
        self.sc = self.sc & 0xFFFF_0000 | v.tgt as u32;
        let ps = v.pspeed as u16;
        if v.tgt.wrapping_sub(v.cur) & 0x8000 == 0 {
            self.t += 37;
            v.cur = v.cur.wrapping_add(ps);
            if v.tgt.wrapping_sub(v.cur) & 0x8000 != 0 {
                self.t += 12;
                v.cur = v.tgt;
            }
        } else {
            self.t += 37;
            v.cur = v.cur.wrapping_sub(ps);
            if v.tgt.wrapping_sub(v.cur) & 0x8000 == 0 {
                self.t += 22;
                v.cur = v.tgt;
            }
        }
    }

    /// 5.6 arpeggio list and base period.
    fn arpeggio(&mut self, v: &mut Voice) -> u16 {
        self.t += 63;
        self.sc = self.sc & 0xFFFF_0000 | v.aidx as u32;
        let mut e = self.rb(v.abase.wrapping_add(v.aidx as usize));
        if e == 0x80 {
            self.t += 26;
            v.aidx = 0;
            self.t += 63;
            self.sc &= 0xFFFF_0000;
            e = self.rb(v.abase);
            if e == 0x80 {
                // a list that starts with the end marker never ends in the original
                e = 0;
            }
        }
        v.aidx = (v.aidx + 1) % 16;
        self.t += 36;
        self.t += 21;
        if v.pspeed != 0 {
            self.t += 22;
            v.cur
        } else {
            self.t += 62;
            let b = Self::period_of(e.wrapping_add(v.note).wrapping_add(v.trans));
            v.cur = b;
            b
        }
    }
}

impl Replayer for Tronic {
    fn format(&self) -> &'static str {
        "Tronic"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK_CLOCKS
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ts = hw.bus.clock;
        self.tb = 0;
        self.t = 273;
        self.sc = 0;
        self.counter = self.counter.wrapping_sub(1);
        if self.counter < 0 {
            self.counter = self.speed as i8;
            self.t += 20;
        }
        for c in 0..4 {
            self.t += 26;
            self.voice(hw, c);
        }
        self.t += 20;
        self.at(hw);
        hw.dmacon(0x800F);
    }
}

#[cfg(test)]
#[path = "tronic_tests.rs"]
mod tests;
