//! Jason Page (`jpn.*` with a sample companion `smp.*`), the Graftgold music system of
//! Realms, Fire & Ice, Uridium 2, Empire Soccer 94 and other Amiga games.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Jason Page player and the structure
//! of the corpus files), and this player was written from it alone and checked
//! against those writes.
//!
//! A module is pure data: a song file and a sample file (raw signed 8-bit PCM, one
//! block that the sound programs also modify while playing). A 50 Hz pattern engine,
//! divided by the song's speed, feeds four voices from order lists of patterns; a
//! note starts a sound, a small command stream per sound that sets the sample,
//! envelope, vibrato and DMA and waits. There are two layouts here: the one of Fire
//! & Ice, Uridium 2, Empire Soccer 94 and the Xmas demo (16-bit offsets), and the
//! one of Realms (32-bit offsets, pre-multiplied opcodes). A third, ViroCop's, is
//! not in any corpus and is declined.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Jason Page", detect, new };

/// Paula periods of the notes, then the value the original reads past the end of
/// its table.
const PERIODS: [u16; 85] = [
    3822, 3607, 3405, 3214, 3033, 2863, 2702, 2551, 2407, 2272, 2145, 2024, 1911, 1803, 1702, 1607, 1516, 1431, 1351,
    1275, 1203, 1136, 1072, 1012, 955, 901, 851, 803, 758, 715, 675, 637, 602, 568, 536, 506, 477, 451, 425, 401, 379,
    357, 337, 318, 301, 284, 268, 253, 238, 225, 212, 200, 189, 179, 168, 159, 150, 142, 134, 126, 119, 112, 106, 100,
    94, 89, 84, 79, 75, 71, 67, 63, 59, 56, 53, 50, 47, 44, 42, 39, 37, 35, 33, 31, 0x48E7,
];

/// The stream every voice is on while nothing plays: layout 1 zeroes the volume and
/// the envelope and loops on "end tick"; layout 3 only loops.
const IDLE1: [u16; 12] = [0x000F, 0, 0x0013, 0, 0, 0, 0, 0x0006, 0, 0x0012, 0x0007, 0];
const IDLE3: [u16; 4] = [0x0018, 0, 0x0048, 0x001C];

/// Commands run per voice and tick, and steps of order lists and patterns, before
/// the player gives up on a stream or list that never ends.
const GUARD: u32 = 4096;

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// Layout 1 (`false`) or 3 (`true`) of a song file, by content.
fn layout(d: &[u8]) -> Option<bool> {
    let w = |o: usize| be16(d, o) as usize;
    if d.len() >= 0xCBE + 8 && be16(d, 0) == 0 && be32(d, 0x80) == 0 && be32(d, 0x84) == 0xCBE && be32(d, 0xCB6) == 0x0003_08BE
        && be32(d, 0xCBA) == 0x0003_09BE
    {
        return Some(true);
    }
    if d.len() < 0x32 || w(0) != 2 || w(2) & 1 != 0 {
        return None;
    }
    let d1 = w(4);
    if d1 & 1 != 0 || d1 + 2 > d.len() || w(d1) != 0 {
        return None;
    }
    let lim = w(0x30);
    for o in (2..=0x2E).step_by(2) {
        let x = w(o);
        if x == 0 || x & 1 != 0 || x >= lim {
            return None;
        }
    }
    let p = w(0x2E);
    (p + 2 <= d.len() && w(p) & 0x0F00 == 0x0F00).then_some(false)
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// The sample companion's names: `smp.NAME` for `jpn.NAME`, `jpnS.NAME` for
/// `jpnd.NAME` (the prefix in any case).
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let b = file.as_bytes();
    if b.len() < 5 || !file.is_char_boundary(4) || !file[..3].eq_ignore_ascii_case("jpn") {
        return Vec::new();
    }
    match b[3] {
        b'.' => vec![format!("smp{}", &file[3..])],
        b'd' | b'D' if b[4] == b'.' => vec![format!("jpnS{}", &file[4..])],
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    // the order list and the pattern
    ostart: usize,
    opos: usize,
    ppos: u8,
    pat: usize,
    fetch: bool,
    trans: u8,
    cur: u8,
    note: u8,
    tc: i32,
    tr: i32,
    prio: u16,
    pending: Option<u8>,
    looped: bool,
    // the stream
    idle: bool,
    sp: usize,
    wait: u16,
    stack: [(usize, u16); 4],
    top: usize,
    depth: usize,
    // the sound
    vibd: u16,
    vibl: u16,
    vibc: u16,
    porta: u16,
    period: u16,
    vol: u16,
    lvl: u16,
    env: [u16; 4],
    sstart: u32,
    sbase: u32,
    srem: u32,
    slen: u16,
}

pub struct Jpn {
    d: Vec<u8>,
    f3: bool,
    title: String,
    subsongs: u16,
    sub: u16,
    pristine: Vec<u8>,
    area: Vec<u8>,
    base: u32,
    silent: u32,
    starts: Vec<u32>,
    speed: u16,
    cnt: i32,
    v: [Voice; 4],
    songend: bool,
}

/// The ingame file of Realms is one long tune; its four order lists, 194 bytes apart,
/// are cut at their loop entries into one subsong each.
fn realms_patch(d: &mut [u8]) {
    if be32(d, 0x8BA).wrapping_add(2) != 7382 {
        return;
    }
    let mut p0 = be32(d, 0x6BE) as usize;
    let mut song = 1usize;
    while p0 + 2 <= d.len() && song < 32 {
        let hi = rb(d, p0);
        if hi == 0xFF && rb(d, p0 + 1) == 0 {
            break;
        }
        if (0xFC..=0xFE).contains(&hi) {
            for v in 0..4usize {
                let p = p0 + 194 * v;
                if p + 2 > d.len() {
                    continue;
                }
                let o = 0x6BE + 0x80 * v + 4 * song;
                if o + 4 <= d.len() {
                    d[o..o + 4].copy_from_slice(&((p + 2) as u32).to_be_bytes());
                }
                d[p] = 0xFF;
                d[p + 1] = 0;
            }
            song += 1;
        }
        p0 += 2;
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let f3 = layout(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut sample = None;
    for name in companion(&file) {
        if let Ok(s) = crate::deli::read_companion(m, &name).1 {
            sample = Some(s);
            break;
        }
    }
    let pristine = sample?;
    let mut d = d.to_vec();
    if !f3 && be16(&d, 0x30) == 7452 && d.len() > 1956 {
        d[1956] = 0xFF;
    }
    if f3 {
        realms_patch(&mut d);
    }
    // sample starts
    let mut starts = Vec::new();
    if f3 {
        let l0 = be32(&d, 0);
        for i in 0..32 {
            starts.push(be32(&d, 4 * i).wrapping_sub(l0));
        }
    } else {
        let smp = be16(&d, 2) as usize;
        let lim = be16(&d, 0x30) as usize;
        let n = (lim.saturating_sub(smp) / 4).min(33);
        let mut acc = 0u32;
        for i in 0..n {
            starts.push(acc);
            acc = acc.wrapping_add(be32(&d, smp + 4 * i));
        }
    }
    let subsongs = if f3 {
        let mut n = 0u16;
        while n < 32 {
            let p = be32(&d, 0x6BE + 4 * n as usize) as usize;
            if be16(&d, p) == 0xFF00 {
                break;
            }
            n += 1;
        }
        n.max(1)
    } else {
        let a = be16(&d, 28) as i32 - (be16(&d, 12) as i32 + 2);
        ((a / 2).clamp(1, 16)) as u16
    };
    let base = m.alloc((pristine.len() as u32).max(4));
    let silent = m.alloc(4);
    if base == 0 || silent == 0 {
        return None;
    }
    let mut r = Jpn {
        d,
        f3,
        title: file,
        subsongs,
        sub: 0,
        area: pristine.clone(),
        pristine,
        base,
        silent,
        starts,
        speed: 0,
        cnt: 0,
        v: Default::default(),
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    hw.dmacon(0x000F);
    r.start(0, &mut hw, true);
    // the Realms player's init sets the LED bit of CIA-A's port (the filter off); the
    // other layout's player never touches it
    if f3 {
        m.bus.ciaa.pra |= 2;
    }
    Some(Box::new(r))
}

impl Jpn {
    fn w(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// Starts subsong `n`: a fresh copy of the sample block and the start state.
    fn start(&mut self, n: u16, hw: &mut Hw, fresh: bool) {
        if fresh {
            self.sub = n;
            self.songend = false;
            self.area.copy_from_slice(&self.pristine);
            hw.bus.write_bytes(self.base, &self.pristine);
            self.v = Default::default();
            for v in self.v.iter_mut() {
                v.env = [0xFFFF; 4];
            }
            if self.f3 {
                for (i, x) in [0x0800u16, 0x0900, 0x0A00, 0].iter().enumerate() {
                    self.v[i].vol = *x;
                }
            }
            let s = n as usize;
            self.speed = if self.f3 { self.w(0x480 + 2 * s) } else { self.w(self.w(8) as usize + 2 * (s & 15)) & 0xFF };
        }
        self.cnt = 0;
        let s = if self.f3 { self.sub as usize } else { self.sub as usize & 15 };
        for k in 0..4 {
            let start = if self.f3 {
                be32(&self.d, 0x6BE + 0x80 * k + 4 * s) as usize
            } else {
                let idx = self.w(12 + 2 * k) as usize;
                (self.w(28 + 2 * k) as usize).wrapping_add(self.w(idx + 2 * s) as usize)
            };
            let v = &mut self.v[k];
            v.idle = true;
            v.sp = 0;
            v.ostart = start;
            v.opos = 0;
            v.ppos = 0;
            v.fetch = true;
            v.trans = 0;
            v.note = 0;
            v.tc = 0;
            v.tr = 0;
            v.prio = 0;
            v.pending = None;
        }
    }
}

impl Jpn {
    /// The word `pos` of the voice's stream.
    fn sw(&self, k: usize, pos: usize) -> u16 {
        if self.v[k].idle {
            let t: &[u16] = if self.f3 { &IDLE3 } else { &IDLE1 };
            t.get(pos / 2).copied().unwrap_or(0)
        } else {
            self.w(pos)
        }
    }

    fn nw(&mut self, k: usize) -> u16 {
        let x = self.sw(k, self.v[k].sp);
        self.v[k].sp += 2;
        x
    }

    fn nl(&mut self, k: usize) -> u32 {
        let h = self.nw(k) as u32;
        h << 16 | self.nw(k) as u32
    }

    fn silent_set(&self, k: usize, hw: &mut Hw) {
        hw.len(k, 1);
        hw.loc(k, self.silent);
        hw.per(k, self.v[k].period);
    }

    fn sample_loc(&self, k: usize) -> u32 {
        let v = &self.v[k];
        self.base.wrapping_add(v.sstart).wrapping_add(2 * v.slen as u32).wrapping_sub(v.srem)
    }

    /// The pattern engine for one tick; true if a voice met the end of the tune.
    fn patterns(&mut self) -> bool {
        for k in 0..4 {
            self.v[k].tc -= 1;
            if self.v[k].tc >= 0 {
                continue;
            }
            self.v[k].tc = self.v[k].tr;
            if self.step(k) {
                return true;
            }
        }
        false
    }

    /// Asks for sound `id` on a voice: taken if its priority is not below the
    /// one playing.
    fn trigger(&mut self, k: usize, id: u8) -> bool {
        let prio = if self.f3 { self.w(0x4C0 + 2 * id as usize) } else { self.w(self.w(10) as usize + 2 * id as usize) & 0xFF };
        let v = &mut self.v[k];
        if prio < v.prio {
            return false;
        }
        v.prio = prio;
        v.pending = Some(id);
        true
    }

    /// One step of a voice's order list and pattern; true at the end of the tune.
    fn step(&mut self, k: usize) -> bool {
        for _ in 0..GUARD {
            if self.v[k].fetch {
                let e = self.w(self.v[k].ostart + self.v[k].opos);
                let (hi, lo) = ((e >> 8) as u8, e as u8);
                match hi {
                    0..=0xFB => {
                        let pat = if self.f3 {
                            be32(&self.d, 0x8BE + 4 * hi as usize) as usize
                        } else {
                            (self.w(46) as usize).wrapping_add(self.w(self.w(44) as usize + 2 * hi as usize) as usize)
                        };
                        let v = &mut self.v[k];
                        v.pat = pat;
                        v.trans = lo;
                        v.fetch = false;
                    }
                    0xFF => return true,
                    0xFD => self.v[k].opos += 2,
                    _ => {
                        self.v[k].opos = lo as usize * 2;
                        self.v[k].looped = true;
                    }
                }
                continue;
            }
            let at = self.v[k].pat.wrapping_add(self.v[k].ppos as usize);
            let b = rb(&self.d, at);
            let v = &mut self.v[k];
            match b {
                0xFF => {
                    v.ppos = 0;
                    v.opos += 2;
                    v.fetch = true;
                }
                0xF9 => {
                    v.ppos = v.ppos.wrapping_add(1);
                    return false;
                }
                0xFE => {
                    v.porta = be16(&self.d, at + 1);
                    v.ppos = v.ppos.wrapping_add(3);
                    return false;
                }
                0x80..=0xFD => {
                    v.cur = b & 0x7F;
                    v.ppos = v.ppos.wrapping_add(1);
                }
                0x40..=0x7F => {
                    v.tr = (b & 0x3F) as i32;
                    v.tc = v.tr;
                    v.ppos = v.ppos.wrapping_add(1);
                }
                _ => {
                    v.ppos = v.ppos.wrapping_add(1);
                    let cur = v.cur;
                    let trans = v.trans;
                    if cur != 0x78 && !self.trigger(k, cur) {
                        return false;
                    }
                    let mut t = b.wrapping_add(trans) as i8;
                    if t >= 0x54 {
                        t = 0x54;
                    }
                    let v = &mut self.v[k];
                    v.note = t as u8;
                    v.period = PERIODS.get(v.note as usize & 0x7F).copied().unwrap_or(0);
                    return false;
                }
            }
        }
        true
    }

    /// DMA off, and the silent registers (layout 1 also clears the volume).
    fn silence(&mut self, k: usize, hw: &mut Hw) {
        hw.dmacon(1 << k);
        hw.wait(150);
        if !self.f3 {
            hw.vol(k, 0);
        }
        self.silent_set(k, hw);
        hw.wait(150);
    }

    fn clear(&mut self, k: usize) {
        let v = &mut self.v[k];
        v.vol = 0;
        v.env = [0xFFFF; 4];
        v.srem = 0;
    }

    fn start_sound(&mut self, k: usize, id: u8, hw: &mut Hw) {
        self.silence(k, hw);
        let sp = if self.f3 {
            be32(&self.d, 0x84 + 4 * id as usize) as usize
        } else {
            (self.w(6) as usize).wrapping_add(self.w(self.w(4) as usize + 2 * id as usize) as usize)
        };
        let f3 = self.f3;
        let v = &mut self.v[k];
        v.vibd = 0;
        v.vibl = 0;
        v.vibc = 0;
        v.porta = 0;
        v.vol = 0;
        v.env = [0xFFFF; 4];
        v.wait = 0;
        v.depth = 0;
        v.top = 0;
        v.srem = if f3 { 0xFFFF_FFFF } else { 0 };
        v.idle = false;
        v.sp = sp;
        v.pending = None;
    }

    fn stop_voice(&mut self, k: usize, hw: &mut Hw) {
        self.v[k].idle = true;
        self.v[k].sp = 0;
        self.v[k].prio = 0;
        if !self.f3 {
            self.clear(k);
        }
        self.silence(k, hw);
    }

    fn poke_area(&mut self, off: usize, hw: &mut Hw) {
        let end = (off + 128).min(self.area.len());
        if off < end {
            hw.bus.write_bytes(self.base + off as u32, &self.area[off..end]);
        }
    }

    /// The commands of a voice's stream for this tick.
    fn stream(&mut self, k: usize, hw: &mut Hw) {
        if self.v[k].wait != 0 {
            self.v[k].wait -= 1;
            if self.v[k].wait != 0 {
                return;
            }
        }
        for _ in 0..GUARD {
            let word = self.nw(k);
            let op = if self.f3 { word >> 2 } else { word & 0xFF };
            match op {
                0 => {
                    self.stop_voice(k, hw);
                    return;
                }
                1 => {
                    if self.f3 {
                        return;
                    }
                    self.stop_voice(k, hw);
                    return;
                }
                2 => {
                    let n = self.nw(k) as usize;
                    let s = self.starts.get(n).copied().unwrap_or(0);
                    self.v[k].sstart = s;
                    self.v[k].sbase = s;
                }
                3 => {
                    let x = self.nw(k);
                    self.v[k].srem = x as u32;
                    self.v[k].slen = x >> 1;
                }
                4 => {
                    let x = self.nl(k);
                    self.v[k].srem = x;
                    if x != 0 {
                        self.v[k].slen = (x as u16) >> 1;
                    }
                }
                5 => {
                    self.v[k].wait = self.nw(k);
                    return;
                }
                6 => {
                    let c = self.nw(k);
                    let pos = self.v[k].sp;
                    let v = &mut self.v[k];
                    if v.depth > 0 {
                        v.top = (v.top + 1) & 3;
                    }
                    v.stack[v.top] = (pos, c);
                    v.depth = (v.depth + 1).min(4);
                }
                7 => {
                    let v = &mut self.v[k];
                    let (pos, c) = v.stack[v.top];
                    if c == 0 {
                        v.sp = pos;
                    } else {
                        v.stack[v.top].1 = c - 1;
                        if c - 1 == 0 {
                            if v.depth > 1 {
                                v.top = (v.top + 3) & 3;
                                v.depth -= 1;
                            }
                        } else {
                            v.sp = pos;
                        }
                    }
                }
                8 => {
                    let x = self.nl(k);
                    self.v[k].sstart = self.v[k].sstart.wrapping_add(x);
                }
                9 => {
                    let x = self.nw(k) as i16 >> 1;
                    self.v[k].slen = self.v[k].slen.wrapping_add(x as u16);
                }
                10 => {
                    let x = self.nl(k);
                    self.v[k].srem = self.v[k].srem.wrapping_add(x);
                }
                11 => {
                    let x = self.nw(k);
                    self.v[k].period = self.v[k].period.wrapping_add(x);
                }
                12 => {
                    let x = self.nw(k);
                    self.v[k].vol = self.v[k].vol.wrapping_add(x);
                }
                13 => {
                    let x = self.nw(k);
                    let v = &mut self.v[k];
                    v.vibd = x >> 8;
                    v.vibl = x & 0xFF;
                    v.vibc = (x & 0xFF) >> 1;
                }
                14 => self.v[k].period = self.nw(k),
                15 => {
                    let x = self.nw(k);
                    // sets the envelope's level register too (the one the stages work on)
                    self.v[k].vol = x;
                    self.v[k].lvl = x;
                }
                16 => {
                    hw.dmacon(if self.f3 { 0x8000 } else { 0x8200 } | 1 << k);
                    hw.wait(150);
                }
                17 => {
                    if !self.f3 {
                        self.clear(k);
                    }
                    self.silence(k, hw);
                    return;
                }
                18 => return,
                19 => {
                    for i in 0..4 {
                        self.v[k].env[i] = self.nw(k);
                    }
                    self.v[k].lvl = 0;
                }
                20 => {
                    let x = self.nw(k);
                    let v = &mut self.v[k];
                    v.note = x as u8;
                    v.period = PERIODS.get(v.note as usize).copied().unwrap_or(0);
                }
                21 => {
                    let x = self.nw(k);
                    let v = &mut self.v[k];
                    v.period = PERIODS.get(x.wrapping_add(v.note as u16) as usize).copied().unwrap_or(0);
                }
                22 => {
                    let x = self.nw(k);
                    let src = self.starts.get((x >> 8) as usize).copied().unwrap_or(0) as usize;
                    let dst = self.starts.get((x & 0xFF) as usize).copied().unwrap_or(0) as usize;
                    if src + 128 <= self.area.len() && dst + 128 <= self.area.len() {
                        self.area.copy_within(src..src + 128, dst);
                        self.poke_area(dst, hw);
                    }
                }
                23 => {
                    let n = self.nw(k) as usize;
                    let dst = self.v[k].sbase as usize;
                    let src = self.starts.get(n).copied().unwrap_or(0) as usize;
                    if src + 128 <= self.area.len() && dst + 128 <= self.area.len() {
                        for i in 0..128 {
                            let t = self.area[src + i];
                            let c = &mut self.area[dst + i];
                            if *c > t {
                                *c -= 1;
                            } else if *c < t {
                                *c += 1;
                            }
                        }
                        self.poke_area(dst, hw);
                    }
                }
                _ => return,
            }
        }
    }

    /// Vibrato, portamento and the envelope.
    fn sound(&mut self, k: usize) {
        let v = &mut self.v[k];
        if v.vibd != 0 || self.f3 {
            v.period = v.period.wrapping_add(v.vibd);
            if v.vibc != 0 {
                v.vibc -= 1;
            } else {
                v.vibc = v.vibl;
                v.vibd = v.vibd.wrapping_neg();
            }
        }
        v.period = v.period.wrapping_add(v.porta);
        let [a, b, c, d] = v.env;
        if a & 0xFF != 0xFF {
            v.env[0] = (a & 0xFF00) | ((a & 0xFF).wrapping_sub(1) & 0xFF);
            match v.lvl.checked_add(a & 0xFF00) {
                Some(l) => v.lvl = l,
                None => {
                    v.lvl = 0xFF00;
                    v.env[0] = 0xFFFF;
                }
            }
            v.vol = v.lvl;
        } else if b & 0xFF != 0xFF {
            v.env[1] = (b & 0xFF00) | ((b & 0xFF).wrapping_sub(1) & 0xFF);
            match v.lvl.checked_sub(b & 0xFF00) {
                Some(l) => v.lvl = l,
                None => {
                    v.lvl = 0;
                    v.env[1] = 0xFFFF;
                }
            }
            v.vol = v.lvl;
        } else if c != 0xFFFF {
            v.env[2] = c.wrapping_sub(1);
            v.vol = v.lvl;
        } else if d != 0xFFFF {
            match v.lvl.checked_sub(d) {
                Some(l) => v.lvl = l,
                None => {
                    v.lvl = 0;
                    v.env[3] = 0xFFFF;
                }
            }
            v.vol = v.lvl;
        }
    }

    /// The registers of a voice at the end of its turn.
    fn write(&mut self, k: usize, hw: &mut Hw) {
        let v = &self.v[k];
        hw.vol(k, (v.vol >> 10).min(63));
        if self.f3 {
            hw.per(k, v.period);
            if v.srem != 0 {
                hw.loc(k, self.sample_loc(k));
                hw.len(k, v.slen);
            } else {
                self.silent_set(k, hw);
            }
        } else if v.srem != 0 {
            hw.loc(k, self.sample_loc(k));
            hw.len(k, v.slen);
            hw.per(k, v.period);
        } else {
            self.silent_set(k, hw);
        }
        hw.wait(100);
    }

    fn turn(&mut self, k: usize, hw: &mut Hw) {
        if let Some(id) = self.v[k].pending {
            self.start_sound(k, id, hw);
        }
        self.stream(k, hw);
        self.sound(k);
        if self.f3 {
            self.write(k, hw);
        }
    }
}

impl Replayer for Jpn {
    fn format(&self) -> &'static str {
        "Jason Page"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsongs - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.start(n, hw, true);
    }

    fn tick(&mut self, hw: &mut Hw) {
        hw.wait(1500);
        let mut ended = false;
        self.cnt -= 1;
        if self.cnt < 0 {
            self.cnt = self.speed as i32;
            ended = self.patterns();
        }
        for k in 0..4 {
            self.turn(k, hw);
        }
        if !self.f3 {
            for k in 0..4 {
                self.write(k, hw);
            }
        }
        if ended {
            self.songend = true;
            self.start(self.sub, hw, false);
            if self.f3 {
                for k in 0..4 {
                    hw.vol(k, 0);
                }
            }
        } else if self.v.iter().all(|v| v.looped) {
            self.songend = true;
            for v in self.v.iter_mut() {
                v.looped = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn p16(d: &mut Vec<u8>, v: u16) {
        d.extend_from_slice(&v.to_be_bytes());
    }

    fn set16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    /// The stream of sound 1 of `song`: sample 0, 16 bytes, period 300, volume 32,
    /// end tick, DMA on, end tick, stop.
    const PLAY: [u16; 13] = [0x12, 2, 0, 3, 16, 14, 300, 15, 0x8000, 0x12, 0x10, 0x12, 0];

    /// A layout-1 song: voice 0 plays pattern 0 (instrument 1, a note), voices 1-3
    /// pattern 1 (a rest); the order lists then loop, or end the tune. `stream` is
    /// the word list from sound 0 (a lone end tick) on; sound 1 starts at its word 1.
    fn song(speed: u16, end: bool, stream: &[u16]) -> Vec<u8> {
        let mut d = vec![0u8; 0x32];
        let sndidx = d.len();
        p16(&mut d, 0);
        p16(&mut d, 2);
        let streams = d.len();
        for w in stream {
            p16(&mut d, *w);
        }
        let speedtab = d.len();
        p16(&mut d, speed);
        let priotab = d.len();
        p16(&mut d, 0);
        p16(&mut d, 0);
        let ordidx = d.len();
        for _ in 0..4 {
            p16(&mut d, 0);
        }
        let mut ordbase = [0usize; 4];
        for (k, b) in ordbase.iter_mut().enumerate() {
            *b = d.len();
            p16(&mut d, if k == 0 { 0x0000 } else { 0x0100 });
            p16(&mut d, if end { 0xFF00 } else { 0xFE00 });
        }
        let patidx = d.len();
        p16(&mut d, 2);
        p16(&mut d, 5);
        let patbase = d.len();
        // a first pattern that is never played (the layout test wants 0x?F?? there),
        // then pattern 0 (instrument 1, note 12) and pattern 1 (a rest)
        d.extend_from_slice(&[0xFF, 0, 0x81, 0x0C, 0xFF, 0xF9, 0xFF, 0]);
        let smptab = d.len();
        d.extend_from_slice(&32u32.to_be_bytes());
        let lim = d.len() as u16;
        set16(&mut d, 0, 2);
        for (i, v) in [smptab, sndidx, streams, speedtab, priotab].iter().enumerate() {
            set16(&mut d, 2 + 2 * i, *v as u16);
        }
        for k in 0..4 {
            set16(&mut d, 12 + 2 * k, (ordidx + 2 * k) as u16);
            set16(&mut d, 20 + 2 * k, (ordidx + 2 * k) as u16);
            set16(&mut d, 28 + 2 * k, ordbase[k] as u16);
            set16(&mut d, 36 + 2 * k, ordbase[k] as u16);
        }
        set16(&mut d, 44, patidx as u16);
        set16(&mut d, 46, patbase as u16);
        set16(&mut d, 48, lim);
        d
    }

    fn run(d: Vec<u8>, ticks: usize) -> (Engine, Vec<(u32, u16)>) {
        let dir = Path::new("/jpn-test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("SMP.test"), vec![0x40; 32]);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&dir.join("jpn.test"), d).unwrap();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        (e, log)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_by_content() {
        let d = song(1, false, &PLAY);
        assert_eq!(layout(&d), Some(false));
        assert!(detect(&d));
        let mut e = d.clone();
        e[1] = 3;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[3] |= 1;
        assert!(!detect(&e));
        // a first pattern of the other (ViroCop) layout
        let mut e = d.clone();
        let p = be16(&e, 46) as usize;
        e[p] = 0x00;
        assert!(!detect(&e));
        assert!(!detect(&d[..0x30]));
        // layout 3: the two magic longs after the pattern pointers
        let mut r = vec![0u8; 0xD00];
        r[0x84..0x88].copy_from_slice(&0xCBEu32.to_be_bytes());
        r[0xCB6..0xCBA].copy_from_slice(&0x0003_08BEu32.to_be_bytes());
        r[0xCBA..0xCBE].copy_from_slice(&0x0003_09BEu32.to_be_bytes());
        assert_eq!(layout(&r), Some(true));
        r[0x84] = 1;
        assert_eq!(layout(&r), None);
        assert_eq!(companion("jpn.Name"), ["smp.Name"]);
        assert_eq!(companion("/x/JPN.NAME"), ["smp.NAME"]);
        assert_eq!(companion("jpnd.Name"), ["jpnS.Name"]);
        assert!(companion("jpx.Name").is_empty());
        assert!(companion("jpn").is_empty());
    }

    #[test]
    fn a_note_starts_a_sound_and_writes_its_registers() {
        let (e, log) = run(song(1, false, &PLAY), 4);
        assert_eq!(e.playing.as_ref().unwrap().format, "Jason Page");
        // the start clears the volume and sets the silent registers at the note's
        // period; the stream then sets the sample and the end-of-turn write follows
        assert_eq!(reg(&log, 0xDFF0A8)[..2], [0, 32]);
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [1, 8]);
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [PERIODS[12], 300]);
        // DMA off at the start, on from the stream
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001) && dma.contains(&0x8201));
        // the sample is written at the sample block, the silent set elsewhere
        let lc = reg(&log, 0xDFF0A2);
        assert_ne!(lc[0], lc[1]);
        // an idle voice keeps writing the silent set
        assert_eq!(reg(&log, 0xDFF0B4)[..2], [1, 1]);
    }

    #[test]
    fn the_pattern_engine_runs_every_speed_plus_one_ticks() {
        let (_, log) = run(song(3, false, &PLAY), 3);
        // one start only in three ticks
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [PERIODS[12], 300]);
        assert_eq!(reg(&log, 0xDFF0A6).iter().filter(|v| **v == PERIODS[12]).count(), 1);
    }

    #[test]
    fn the_end_marker_ends_the_song_and_restarts_it() {
        let (e, log) = run(song(0, true, &PLAY), 4);
        assert!(e.songend());
        assert!(!reg(&log, 0xDFF0A8).is_empty());
        let (e, _) = run(song(1, false, &PLAY), 2);
        assert!(!e.songend());
    }

    #[test]
    fn vibrato_and_envelope_follow_the_stream() {
        // sample 0, 16 bytes, period 300, volume 0, vibrato 2 for 4 steps, attack
        // of 0x0400 in one tick, DMA on, then end ticks
        let s = [0x12, 2, 0, 3, 16, 14, 300, 15, 0, 13, 0x0204, 19, 0x0400, 0xFFFF, 0xFFFF, 0xFFFF, 16, 18, 18, 18, 18, 18, 18];
        let (_, log) = run(song(7, false, &s), 5);
        // the vibrato adds 2 each tick, turns after two more, and the attack lifts the volume by one
        assert_eq!(reg(&log, 0xDFF0A6)[..5], [PERIODS[12], 302, 304, 306, 304]);
        assert_eq!(reg(&log, 0xDFF0A8)[..4], [0, 1, 1, 1]);
    }

    #[test]
    fn a_loop_repeats_its_commands() {
        // three passes over "wait one tick" (loop count 3), then the stop: the
        // sample registers are written on every tick until then
        let s = [0x12, 2, 0, 3, 16, 14, 300, 15, 0x8000, 16, 6, 3, 5, 1, 7, 0];
        let (_, log) = run(song(15, false, &s), 8);
        let lens = reg(&log, 0xDFF0A4);
        assert_eq!(lens[..2], [1, 8]);
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma.iter().filter(|v| **v == 0x0001).count(), 2);
    }

    #[test]
    fn the_priority_table_is_read() {
        let mut d = song(0, false, &PLAY);
        // sound 1 has priority 5 and is taken by an idle voice (priority 0)
        let prio = be16(&d, 10) as usize;
        set16(&mut d, prio + 2, 5);
        let (_, log) = run(d, 4);
        assert_eq!(reg(&log, 0xDFF0A6)[0], PERIODS[12]);
    }
}
