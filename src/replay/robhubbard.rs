//! Rob Hubbard on the Amiga (`rh.*`), the driver of Rob Hubbard's Amiga game soundtracks
//! (Budokan, Populous, The Bard's Tale III, PGA Tour Golf).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the Paula register writes of the
//! 68k reference player and execution traces, and this player was written from it
//! alone and checked against those writes (value, order and tick-relative time).
//!
//! A module is the driver's own position-independent code image followed by its data:
//! tables, instrument records, song table, track lists, note streams and the sample
//! bank. The code is never run. The driver's tables are located by a bounded
//! byte-pattern scan of the image's own init and play code (the address loads it makes),
//! and the driver is written here from its behaviour: four voices, one Paula channel
//! each, 50 Hz, a note stream with instrument select, slide, rest and next-pattern
//! commands, vibrato and waveform modulation. Three code generations are written
//! (A, B and C).

use super::m68kdec::{decode, Ea, K};
use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Rob Hubbard", detect, new };

#[derive(Clone, Copy, PartialEq, Debug)]
enum Gen {
    A,
    B,
    C,
}

/// The layout parameters of a module (offsets in the image).
#[derive(Clone, Copy, Debug)]
struct Lay {
    kind: Gen,
    nsm: usize,
    bank: usize,
    silent: usize,
    ins: usize,
    syn: usize,
    synins: usize,
    per: usize,
    vib: usize,
    song: usize,
}

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn rw(d: &[u8], o: usize) -> u16 {
    ((rb(d, o) as u16) << 8) | rb(d, o + 1) as u16
}

fn rl(d: &[u8], o: usize) -> u32 {
    ((rw(d, o) as u32) << 16) | rw(d, o + 2) as u32
}

// ---------------------------------------------------------------------------------
// Recognition and layout

/// The sample bank walked from `bank` for `n` entries: the offsets of the sample data and their
/// byte lengths and rates, if it ends exactly on the `0x4E71` word that closes the file.
fn walk_bank(d: &[u8], bank: usize, n: usize) -> Option<Vec<(usize, usize, u16)>> {
    let mut v = Vec::new();
    let mut p = bank;
    for _ in 0..n {
        if p + 6 > d.len() {
            return None;
        }
        let len = rl(d, p) as usize;
        if len > 0x10000 {
            return None;
        }
        v.push((p + 6, len, rw(d, p + 4)));
        p += 6 + len;
    }
    if d.len() < 2 || p != d.len() - 2 || rw(d, p) != 0x4E71 {
        return None;
    }
    Some(v)
}

/// The tables the init and play code name.
fn locate(d: &[u8]) -> Option<Lay> {
    if d.len() < 0x400 {
        return None;
    }
    for o in [0usize, 4, 8, 12, 16] {
        if rw(d, o) != 0x6000 {
            return None;
        }
    }
    if rw(d, 20) != 0x41FA || rl(d, 28) != 0x4E75_41FA {
        return None;
    }
    let init = (6 + rw(d, 6) as i16 as isize) as usize;
    if init % 2 != 0 || init >= d.len() {
        return None;
    }
    // decode the init code up to its first return
    let mut code = Vec::new();
    let mut at = init;
    loop {
        let i = decode(d, at)?;
        let end = i.k == K::Rts;
        at = i.next();
        code.push(i);
        if end {
            break;
        }
        if code.len() > 300 || at >= d.len() {
            return None;
        }
    }
    let pc_load = |reg: u8| -> Vec<(usize, usize)> {
        code.iter()
            .enumerate()
            .filter_map(|(k, i)| match (i.k, i.src, i.dst) {
                (K::Lea, Ea::Pc(t), Ea::A(r)) if r == reg => Some((k, t)),
                _ => None,
            })
            .collect()
    };
    let base_load = |reg: u8| -> Option<usize> {
        code.iter().find_map(|i| match (i.k, i.src, i.dst) {
            (K::Lea, Ea::Disp(3, dd), Ea::A(r)) if r == reg => Some(dd as u16 as usize),
            _ => None,
        })
    };
    // bank and silent area
    let a0 = *pc_load(0).first()?;
    let (k0, t0) = a0;
    let add40 = code.get(k0 + 1).map_or(false, |i| match (i.k, i.src, i.dst) {
        (K::Adda, Ea::Imm { val: 0x40, .. }, Ea::A(0)) => true,
        (K::Lea, Ea::Disp(0, 0x40), Ea::A(0)) => true,
        _ => false,
    });
    // instruments, synth waveforms
    let (kins, ins) = *pc_load(5).first()?;
    let nsm = code[kins + 1..].iter().take(4).find_map(|i| (i.k == K::Moveq).then(|| (i.data & 0xFF) as usize + 1))?;
    let synins = pc_load(5).get(1).map(|e| e.1)?;
    let syn = *pc_load(2).first().map(|e| &e.1)?;
    let voices = base_load(1)?;
    let song = base_load(0)?;
    // the record size: the immediate added to a1 after the voices are loaded
    let mut rec = 0x26usize;
    for i in &code {
        match (i.k, i.src, i.dst) {
            (K::Adda, Ea::Imm { val, .. }, Ea::A(1)) if val == 0x26 || val == 0x28 => rec = val as usize,
            (K::Lea, Ea::Disp(1, dd), Ea::A(1)) if dd == 0x26 || dd == 0x28 => rec = dd as usize,
            _ => {}
        }
    }
    // generation A keeps a default instrument: a byte copied from d1 at the very start
    let defins = code.iter().take(4).any(|i| i.k == K::Move && i.size == 1 && i.src == Ea::D(1));
    let kind = if rec == 0x28 {
        Gen::C
    } else if defins {
        Gen::A
    } else {
        Gen::B
    };
    // the code ends where the voice records start; its address loads name the other tables
    let code_end = voices.min(d.len());
    let mut per = None;
    let mut vibs = Vec::new();
    let mut base_loads = Vec::new();
    let mut o = 0;
    while o + 4 <= code_end {
        let w = rw(d, o);
        let disp = rw(d, o + 2) as i16 as isize;
        let t = (o as isize + 2 + disp) as usize;
        if w == 0x45FA && per.is_none() && t + 24 < d.len() {
            // a period table falls from its first entry
            let e: Vec<u16> = (0..12).map(|k| rw(d, t + 2 * k)).collect();
            if (1000..=16384).contains(&e[0]) && e.windows(2).all(|p| p[0] > p[1]) {
                per = Some(t);
            }
        }
        if w == 0x49FA && t < d.len() {
            vibs.push(t);
        }
        if w & 0xF1FF == 0x41EB && o >= 24 {
            base_loads.push(disp as u16 as usize);
        }
        o += 2;
    }
    let per = per?;
    let vib = vibs.iter().copied().find(|&t| t >= per + 98 && t < song).or_else(|| vibs.first().copied())?;
    let (silent, bank) = if add40 {
        (t0, t0 + 0x40)
    } else {
        let s = base_loads.iter().copied().find(|&s| {
            s != voices && s != song && s + 512 <= d.len() && s + 512 <= t0 + 1 && d[s..s + 512].iter().all(|&b| b == 0)
        })?;
        (s, t0)
    };
    if bank >= d.len() || song + 18 > d.len() || ins + 32 * (nsm + 3) > d.len() {
        return None;
    }
    walk_bank(d, bank, nsm)?;
    Some(Lay { kind, nsm, bank, silent, ins, syn, synins, per, vib, song })
}

fn detect(d: &[u8]) -> bool {
    locate(d).is_some()
}

/// The number of subsongs: song-table entries up to the first whose first two bytes are zero.
fn song_count(d: &[u8], l: &Lay) -> usize {
    let mut n = 0;
    while n < 256 && l.song + 18 * (n + 1) <= d.len() && (rb(d, l.song + 18 * n) != 0 || rb(d, l.song + 18 * n + 1) != 0) {
        n += 1;
    }
    n
}

// ---------------------------------------------------------------------------------
// Timing helpers (68000 cycles; the clock is cycles / 2)

/// The channel chain of the Eagleplayer's register routines.
const CH: [u64; 4] = [31, 62, 93, 101];
const CV: [u64; 4] = [174, 197, 220, 210];
/// Wait for the last-flag branch of a wrapping list, by voice.
const P_WRAP: [u64; 4] = [45, 72, 95, 108];

/// One instrument record.
#[derive(Clone, Copy, Default)]
struct Ins {
    wave: u32,
    loopoff: i32,
    len: u16,
    scale: u16,
    vol: u16,
    vdiv: u16,
    vseq: i16,
    mlim: u16,
    mstart: u16,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    flags: u8,
    note: u8,
    stream: u32,
    list: u32,
    listpos: u32,
    counter: u16,
    period: u16,
    slide: i8,
    started: bool,
    ins: Option<usize>,
    vibpos: u32,
    vibstart: u32,
    modidx: u16,
    over: u16,
}

struct RobHubbard {
    img: Vec<u8>,
    base: u32,
    l: Lay,
    songs: usize,
    sub: usize,
    recs: Vec<Ins>,
    vs: [Voice; 4],
    tempo: u8,
    playing: bool,
    defins: u8,
    /// Per voice: has not yet wrapped since the last song-end notification.
    live: [bool; 4],
    /// The voice with the longest track list.
    posvoice: usize,
    songend: bool,
    // the clock of the tick being played
    base_cc: u64,
    t: u64,
    tref: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = locate(d)?;
    let songs = song_count(d, &l);
    if songs == 0 {
        return None;
    }
    let bank = walk_bank(d, l.bank, l.nsm)?;
    let mut img = d.to_vec();
    // the host rewrites every stop command byte from the terminating song-table entry up to the bank
    let from = l.song + 18 * songs;
    for b in img.iter_mut().take((l.bank + 1).min(d.len())).skip(from) {
        if *b == 0x85 {
            *b = 0x84;
        }
    }
    let mut recs = Vec::with_capacity(l.nsm + 3);
    for k in 0..l.nsm + 3 {
        let o = if k < l.nsm { l.ins + 32 * k } else { l.synins + 32 * (k - l.nsm) };
        let mut r = Ins {
            wave: 0,
            loopoff: rl(d, o + 4) as i32,
            len: rw(d, o + 8),
            scale: rw(d, o + 0xC),
            vol: rw(d, o + 0xE),
            vdiv: rw(d, o + 0x10),
            vseq: rw(d, o + 0x12) as i16,
            mlim: rw(d, o + 0x14),
            mstart: rw(d, o + 0x16),
        };
        if k < l.nsm {
            let (off, len, rate) = bank[k];
            r.wave = base.wrapping_add(off as u32);
            r.len = (len / 2) as u16;
            r.scale = if rate == 0 { 0 } else { (3_579_545u32 / rate as u32) as u16 };
        } else {
            r.wave = base.wrapping_add(rl(d, l.syn + 4 * (k - l.nsm)));
        }
        recs.push(r);
    }
    let mut s = RobHubbard {
        img,
        base,
        l,
        songs,
        sub: 0,
        recs,
        vs: [Voice::default(); 4],
        tempo: 0,
        playing: false,
        defins: 0,
        live: [true; 4],
        posvoice: 0,
        songend: false,
        base_cc: 0,
        t: 0,
        tref: 0,
    };
    let mut hw = Hw { bus: &mut m.bus };
    s.init(0, &mut hw);
    Some(Box::new(s))
}

impl RobHubbard {
    fn list_len(&self, voice: usize) -> usize {
        let e = self.song_entry(self.sub);
        let list = rl(&self.img, e + 2 + 4 * voice) as usize;
        let mut n = 0;
        while n < 4096 && rl(&self.img, list + 4 * n) != 0 && list + 4 * n + 4 <= self.img.len() {
            n += 1;
        }
        n
    }

    fn song_entry(&self, s: usize) -> usize {
        self.l.song + 18 * s
    }

    /// The driver's init for subsong `n`.
    fn init(&mut self, n: usize, hw: &mut Hw) {
        hw.dmacon(0x000F);
        hw.bus.write_word(0xDFF09E, 0x00FF);
        self.playing = false;
        self.sub = n;
        let e = self.song_entry(n);
        self.tempo = rb(&self.img, e + 1);
        for v in 0..4 {
            let list = rl(&self.img, e + 2 + 4 * v);
            let vs = &mut self.vs[v];
            vs.counter = 1;
            vs.flags = 0;
            vs.list = list;
            vs.listpos = 4;
            vs.stream = rl(&self.img, list as usize);
            if self.l.kind == Gen::C {
                vs.over = 0;
            }
        }
        self.defins = self.list_len(3) as u8;
        // the longest list (ties: the highest voice)
        let mut best = (0, 0);
        for v in 0..4 {
            let n = self.list_len(v);
            if n >= best.0 {
                best = (n, v);
            }
        }
        self.posvoice = best.1;
        self.live = [true; 4];
        self.playing = true;
    }

    fn rec(&self, i: Option<usize>) -> Ins {
        match i {
            Some(k) => self.recs.get(k).copied().unwrap_or_else(|| self.raw_rec(k)),
            None => Ins::default(),
        }
    }

    /// An instrument number past the bank and the synth records: whatever the image holds there.
    fn raw_rec(&self, k: usize) -> Ins {
        let o = self.l.ins + 32 * k;
        let d = &self.img;
        Ins {
            wave: self.base.wrapping_add(rl(d, o)),
            loopoff: rl(d, o + 4) as i32,
            len: rw(d, o + 8),
            scale: rw(d, o + 0xC),
            vol: rw(d, o + 0xE),
            vdiv: rw(d, o + 0x10),
            vseq: rw(d, o + 0x12) as i16,
            mlim: rw(d, o + 0x14),
            mstart: rw(d, o + 0x16),
        }
    }

    // --- clock

    fn stamp(&self, hw: &mut Hw) {
        hw.bus.clock = self.base_cc + (self.t - self.tref) / 2;
    }

    fn w_loc(&self, hw: &mut Hw, ch: usize, a: u32) {
        self.stamp(hw);
        hw.loc(ch, a);
    }

    fn w_len(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.len(ch, n);
    }

    fn w_vol(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.vol(ch, n & 0x7F);
    }

    fn w_per(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.per(ch, n);
    }

    fn w_dma(&self, hw: &mut Hw, n: u16) {
        self.stamp(hw);
        hw.dmacon(n);
    }

    fn silent_len(&self) -> u16 {
        if self.l.kind == Gen::B {
            0x100
        } else {
            0x20
        }
    }

    // --- the tick

    fn tick_inner(&mut self, hw: &mut Hw) {
        self.base_cc = hw.bus.clock;
        self.tref = 0;
        self.t = 256;
        if !self.playing {
            return;
        }
        self.t += 33;
        for v in (0..4).rev() {
            if !self.voice(v, hw) {
                return;
            }
        }
        self.t += 16;
    }

    /// One voice's slot; false when the stop command ended the tick.
    fn voice(&mut self, v: usize, hw: &mut Hw) -> bool {
        self.t += 173;
        let ins_at_start = self.vs[v].ins;
        let r = self.rec(ins_at_start);
        if !self.vs[v].started {
            self.t += 41;
            self.vs[v].started = true;
            if r.loopoff == 0 {
                self.t += 8 + 16;
                self.w_loc(hw, v, self.base + self.l.silent as u32);
                self.t += 16;
                self.w_len(hw, v, self.silent_len());
            } else if r.loopoff < 0 {
                self.t += 9;
            } else {
                self.t += 57;
                let s = self.t;
                let lo = r.loopoff;
                self.t = s + 36;
                self.w_loc(hw, v, r.wave.wrapping_add(lo as u32));
                self.t = s + 36 + 18 + 52 + CH[v] + 12;
                self.w_len(hw, v, r.len.wrapping_sub((lo as u16) >> 1));
                self.t += 18 + 48 + CH[v] + 16;
                self.t += 14;
            }
        }
        self.t += 25;
        let c = self.vs[v].counter.wrapping_sub(1);
        self.vs[v].counter = c;
        if c != 0 {
            self.t += 27;
            if c == 1 {
                self.t += 12 + 16;
                self.w_dma(hw, 1 << v);
                self.t += 10;
            }
            self.effects(v, r, hw);
            self.t += 10;
            return true;
        }
        self.fetch(v, hw)
    }

    fn effects(&mut self, v: usize, r: Ins, hw: &mut Hw) {
        self.t += 27;
        if self.vs[v].flags & 2 != 0 {
            self.t += 28;
            let p = self.vs[v].period.wrapping_add(self.vs[v].slide as i16 as u16);
            self.vs[v].period = p;
            let s = self.t;
            self.t = s + 44;
            self.w_per(hw, v, p);
            self.t = s + 44 + 18 + 48 + CH[v] + 16;
            self.t += 4;
        }
        self.t += 21;
        if r.vdiv != 0 {
            self.t += 181;
            let mut pos = self.vs[v].vibpos;
            let mut x = rb(&self.img, pos as usize);
            pos += 1;
            if x == 0x84 {
                self.t += 24;
                pos = self.vs[v].vibstart;
                x = rb(&self.img, pos as usize);
                pos += 1;
            }
            self.vs[v].vibpos = pos;
            self.t += 58;
            let q = self.vs[v].period / r.vdiv;
            let val = (self.vs[v].period as i32).wrapping_add((q as i16 as i32).wrapping_mul(x as i8 as i32)) as u16;
            let s = self.t;
            self.t = s + 36;
            self.w_per(hw, v, val);
            self.t = s + 36 + 12 + 4 + 18 + 48 + CH[v] + 12 + 16;
            self.t += 16;
        }
        self.t += if self.l.kind == Gen::A { 33 } else { 35 };
        if r.mlim != 0 {
            self.t += 61 + 17;
            let w = r.wave as usize;
            let vs = &mut self.vs[v];
            let mut idx = vs.modidx;
            let (store, extra);
            if vs.flags & 4 == 0 {
                idx = idx.wrapping_add(1);
                if idx <= r.mlim {
                    store = 0u8;
                    extra = 0;
                } else {
                    vs.flags |= 4;
                    store = 0x3C;
                    extra = 26;
                }
            } else {
                idx = idx.wrapping_sub(1);
                if idx > r.mstart {
                    store = 0x3C;
                    extra = 0;
                } else {
                    vs.flags &= !4;
                    store = 0;
                    extra = 26;
                }
            }
            vs.modidx = idx;
            self.t += extra + 40;
            hw.bus.write_bytes((w + idx as usize) as u32, &[store]);
        }
    }

    /// The event fetch of a voice whose counter ran out; false when the stop command ended the tick.
    fn fetch(&mut self, v: usize, hw: &mut Hw) -> bool {
        let kind = self.l.kind;
        self.vs[v].flags = 0;
        self.t += 12;
        let order: &[(u8, u64)] = match kind {
            Gen::C => &[(0x86, 19), (0x84, 19), (0x80, 17), (0x81, 17), (0x82, 17), (0x83, 17), (0x85, 17), (0x87, 19)],
            Gen::B => &[(0x84, 17), (0x80, 17), (0x81, 17), (0x82, 17), (0x83, 17)],
            Gen::A => &[(0x84, 19), (0x80, 17), (0x81, 17), (0x82, 17), (0x83, 17), (0x85, 17)],
        };
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 5000 {
                self.t += 10;
                return true;
            }
            let b = rb(&self.img, self.vs[v].stream as usize);
            self.vs[v].stream += 1;
            self.t += 23;
            if b < 0x80 {
                self.note(v, b, hw);
                self.t += 10;
                return true;
            }
            // command dispatch: the tests up to the matching one are paid
            let mut found = false;
            for &(cmd, cost) in order {
                self.t += cost;
                if cmd == b {
                    found = true;
                    break;
                }
            }
            if !found {
                self.t += 10;
                continue;
            }
            match b {
                0x80 => {
                    let mut n = rb(&self.img, self.vs[v].stream as usize) as usize;
                    self.vs[v].stream += 1;
                    self.t += 126;
                    if kind == Gen::A {
                        self.t += 11;
                        if n >= 0x80 {
                            self.t += 12;
                            n = self.defins as usize;
                        }
                    }
                    self.vs[v].ins = Some(n);
                    let vq = (self.l.vib as i64 + self.rec(Some(n)).vseq as i64) as u32;
                    self.vs[v].vibstart = vq;
                    self.vs[v].vibpos = vq;
                }
                0x81 => {
                    self.vs[v].slide = rb(&self.img, self.vs[v].stream as usize) as i8;
                    self.vs[v].stream += 1;
                    self.vs[v].flags |= 2;
                    self.t += 42;
                }
                0x82 => {
                    let n = rb(&self.img, self.vs[v].stream as usize) as u16;
                    self.vs[v].stream += 1;
                    self.vs[v].counter = n.wrapping_mul(self.tempo as u16);
                    self.t += 114 + 16;
                    self.w_loc(hw, v, self.base + self.l.silent as u32);
                    self.t += 16;
                    self.w_len(hw, v, self.silent_len());
                    self.t += 20;
                    return true;
                }
                0x83 | 0x86 => self.t += 10,
                0x87 => {
                    self.vs[v].over = rb(&self.img, self.vs[v].stream as usize) as u16;
                    self.vs[v].stream += 1;
                    self.t += 34;
                }
                0x84 => self.next_pattern(v),
                _ => {
                    // 0x85: stop
                    self.playing = false;
                    self.t += 20;
                    self.w_dma(hw, 0x000F);
                    self.t += 16;
                    self.stamp(hw);
                    hw.bus.write_word(0xDFF09E, 0x00FF);
                    return false;
                }
            }
        }
    }

    fn next_pattern(&mut self, v: usize) {
        self.t += 48 + 20 + 24 + 12 + 9;
        let list = self.vs[v].list as usize;
        let a = list + self.vs[v].listpos as usize;
        let pos = if rl(&self.img, a) == 0 {
            self.t += 8 + P_WRAP[v] + 12 + 9;
            self.live[v] = false;
            if self.live.iter().all(|&f| !f) {
                self.t += 20 + 20 + 16 + 16 + 16;
                self.songend = true;
                // the notification re-bases the clock
                self.base_cc += (self.t - self.tref) / 2;
                self.tref = self.t;
                self.live = [true; 4];
                self.t += 16;
            }
            self.t += 16 + 4;
            self.vs[v].listpos = 4;
            list
        } else {
            self.vs[v].listpos += 4;
            a
        };
        self.vs[v].stream = rl(&self.img, pos);
        self.t += 18 + 9 + if v == self.posvoice { 32 } else { 0 } + 28 + 12 + 16;
        self.t += 12;
        self.t += 8 + 16 + 10;
    }

    fn note(&mut self, v: usize, b: u8, hw: &mut Hw) {
        let kind = self.l.kind;
        let dur = (b as u16).wrapping_mul(self.tempo as u16);
        let note = rb(&self.img, self.vs[v].stream as usize);
        self.vs[v].stream += 1;
        self.vs[v].note = note;
        let r = self.rec(self.vs[v].ins);
        self.vs[v].modidx = r.mstart;
        self.vs[v].vibpos = self.vs[v].vibstart;
        self.t += 162;
        let s = self.t;
        let over = kind == Gen::C && self.vs[v].over != 0;
        self.t = s + 72;
        self.w_loc(hw, v, r.wave);
        let lenw = 72 + 18 + 52 + CH[v] + 24;
        self.t = s + lenw;
        self.w_len(hw, v, r.len);
        let x = match (kind, over) {
            (Gen::C, false) => 37,
            (Gen::C, true) => 49,
            _ => 12,
        };
        let volw = lenw + 18 + 48 + CH[v] + x + 18 + CV[v];
        self.t = s + volw;
        let vol = if over { self.vs[v].over } else { r.vol };
        self.w_vol(hw, v, vol);
        self.t = s + volw + 16 + 18 + 48 + CH[v] + 28 + 16;
        self.t += if kind == Gen::C { 48 } else { 24 };
        self.t += 132;
        let per = ((rw(&self.img, self.l.per + 2 * note as usize) as u32).wrapping_mul(r.scale as u32) >> 10) as u16;
        self.vs[v].period = per;
        let s = self.t;
        self.t = s + 44;
        self.w_per(hw, v, per);
        self.t = s + 44 + 18 + 48 + CH[v] + 16;
        self.t += 4 + 8 + 8 + 16;
        self.w_dma(hw, 0x8200 | (1 << v));
        self.t += 16;
        self.vs[v].started = false;
        self.vs[v].counter = dur;
    }
}

impl Replayer for RobHubbard {
    fn format(&self) -> &'static str {
        "Rob Hubbard"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.songs as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.songs - 1);
        self.songend = false;
        self.init(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }
}

#[cfg(test)]
#[path = "robhubbard_tests.rs"]
mod tests;
