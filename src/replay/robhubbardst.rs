//! Rob Hubbard on the Atari ST (`rho.*`), the driver Rob Hubbard wrote for the ST
//! games ported to the Amiga (Battleships and Gold Runner).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Rob Hubbard ST player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! Two builds are specified and verified (A: Battleships, B: Gold Runner); a third
//! (Jupiter Probe) is not, and is refused. The module is the driver's code image
//! with its data behind it: three voice records, a song table, track lists of
//! offsets to pattern streams, envelope streams and a period table. The file
//! carries no header, so the builds are told apart by the interrupt-acknowledge
//! value the driver's code writes each frame (found by decoding the instructions)
//! and confirmed by the song table at the build's offset.
//!
//! Three voices play at 50 Hz: every frame restarts a one-word-pair sample on
//! channel 3 and acknowledges, then (while playing) runs a tick whose row
//! counter steps the pattern streams, computes envelope, slide and vibrato per
//! voice, and writes channels 0 to 2.

use super::m68kdec::{decode, Ea, K};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Rob Hubbard ST", detect, new };

/// What differs between the two builds (all offsets into the module).
struct Build {
    /// The value the driver writes to the interrupt-request register each frame.
    ack: u16,
    /// The song table; also the base the track lists' and streams' offsets count from.
    song: usize,
    /// The envelope table (4 offsets from it to the envelope streams).
    env: usize,
    /// The period table (96 words).
    per: usize,
    /// The 100-word sample voice 2 plays.
    smp2: u32,
    /// Whether the voice-1 sweep and the stopped-state silence of voice 2 exist, and
    /// whether the voice-2 effect gates the output.
    a: bool,
}

const BUILD_A: Build = Build { ack: 64, song: 2338, env: 1928, per: 1736, smp2: 412, a: true };
const BUILD_B: Build = Build { ack: 4, song: 2356, env: 1946, per: 1754, smp2: 202, a: false };

/// Sample offsets common to the builds.
const SMP_V1: u32 = 8;
const SMP_V2_INIT: u32 = 0;
const SMP_V2_ALT: u32 = 16;
const SMP_V3: u32 = 40;
const SMP_RESTART: u32 = 24;

const INTREQ: u32 = 0xDFF09C;
const ADKCON: u32 = 0xDFF09E;

// Colour clocks between the writes, as the original's code takes time (measured in
// the reference runs).
/// Colour clocks per event from the end of the acknowledge to the first output write, and
/// the constant last (least-squares fits to the reference runs' times).
const COST_A: [i32; NEV + 1] = [108, 79, 16, 0, 62, 80, 89, 89, 65, 90, 0, -19, 21, 80, -24, 44, 24, 103, 8, 0, 0, 31, 154, 0, 114];
const COST_B: [i32; NEV + 1] = [111, 60, 54, 0, 41, 6, 86, 76, 71, 93, 0, -12, 18, 65, -6, 42, 26, 66, 112, 0, 6, 18, 112, 0, 38];
const NEV: usize = 24;
const E_ROW: usize = 0;
const E_ROW_VOICE: usize = 1;
const E_NOTE: usize = 2;
const E_TIE: usize = 3;
const E_DUR: usize = 4;
const E_SPEED: usize = 5;
const E_ENVSEL: usize = 6;
const E_ADSR: usize = 7;
const E_CMD: usize = 8;
const E_TRACK: usize = 9;
const E_CMD_VIB: usize = 10;
const E_CMD_SLIDE: usize = 11;
const E_ENV_GE16: usize = 12;
const E_ATTACK: usize = 13;
const E_ATTACK_N: usize = 14;
const E_DECAY: usize = 15;
const E_WRAP: usize = 16;
const E_SLIDE: usize = 17;
const E_SHIFT: usize = 18;
const E_VIB: usize = 19;
const E_MASK1: usize = 20;
const E_MASK2: usize = 21;
const E_VOICE: usize = 22;
const T_FIRST: u64 = 138;
const T_ACK: u64 = 10;
const T_ABANDON: u64 = 104;

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The value the code writes to the interrupt-request register: `Some` when all such
/// writes agree.
fn ack_value(d: &[u8]) -> Option<u16> {
    let mut found = None;
    let mut at = 0;
    while at + 10 <= d.len() {
        if let Some(i) = decode(d, at) {
            if i.k == K::Move && i.size == 2 {
                if let (Ea::Imm { val, .. }, Ea::AbsL { addr: INTREQ, .. }) = (i.src, i.dst) {
                    match found {
                        None => found = Some(val as u16),
                        Some(v) if v == val as u16 => {}
                        _ => return None,
                    }
                }
            }
        }
        at += 2;
    }
    found
}

fn build_of(d: &[u8]) -> Option<&'static Build> {
    if !(2500..=8192).contains(&d.len()) {
        return None;
    }
    let b = match ack_value(d)? {
        64 => &BUILD_A,
        4 => &BUILD_B,
        _ => return None,
    };
    let songs = be32(d, b.song) as usize / 12;
    if be32(d, b.song) % 12 != 0 || !(1..=16).contains(&songs) || be16(d, b.per) != 3421 {
        return None;
    }
    for i in 0..3 * songs {
        let off = be32(d, b.song + 4 * i) as usize;
        let list = b.song + off;
        if off < 12 * songs || list + 4 > d.len() {
            return None;
        }
        let first = be32(d, list) as usize;
        if first == 0 || b.song + first >= d.len() {
            return None;
        }
    }
    Some(b)
}

fn detect(d: &[u8]) -> bool {
    build_of(d).is_some()
}

#[derive(Clone, Copy, Default)]
struct Voice {
    flags: u8,
    stream: usize,
    base: usize,
    index: usize,
    vib_acc: u16,
    env_loop: usize,
    env_ptr: usize,
    vib_step: u16,
    vib_delay: u16,
    countdown: u16,
    duration: u16,
    note: u16,
    vol: u8,
    attack: u8,
    release: u8,
    level: u8,
    decay: u8,
    count: u8,
    init_vol: u8,
    slide_limit: u16,
    slide_step: u16,
    slide_acc: u16,
    mode: u8,
    flag96: u8,
    vmask: u8,
}

pub struct RobHubbardSt {
    d: Vec<u8>,
    b: &'static Build,
    base: u32,
    title: String,
    /// The track list that decides the song end (the longest).
    long: usize,
    v: [Voice; 3],
    sfx2: u8,
    sfx2_sub: u8,
    sweep: u16,
    started: u16,
    speed: u16,
    tickc: u16,
    gmode: u8,
    dmode: u8,
    mask: u8,
    per: [u16; 3],
    mode396: u8,
    flag397: u8,
    vols: [u8; 3],
    songend: bool,
    subsong: usize,
    songs: usize,
    /// Events of the tick in progress, for its running time.
    ev: [u32; NEV],
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let b = build_of(d)?;
    let songs = be32(d, b.song) as usize / 12;
    let title = m.bus.read_cstr(m.deli.file_buf);
    // the longest track list of all subsongs (the later one on a tie)
    let (mut long, mut best) = (0, 0);
    for i in 0..3 * songs {
        let list = b.song + be32(d, b.song + 4 * i) as usize;
        let n = (0..).take_while(|k| be32(d, list + 4 * k) != 0 && list + 4 * k < d.len()).count();
        if n >= best {
            best = n;
            long = list;
        }
    }
    let mut v = [Voice::default(); 3];
    for (i, vo) in v.iter_mut().enumerate() {
        vo.vmask = [9, 18, 36][i];
    }
    let mut r = RobHubbardSt {
        d: d.to_vec(),
        b,
        base,
        title,
        long,
        v,
        sfx2: 0,
        sfx2_sub: 0,
        sweep: 0,
        started: 0,
        speed: 0,
        tickc: 0,
        gmode: 0,
        dmode: 0,
        mask: 0,
        per: [0; 3],
        mode396: 0,
        flag397: 0,
        vols: [0; 3],
        songend: false,
        subsong: 0,
        songs,
        ev: [0; NEV],
    };
    let mut hw = Hw { bus: &mut m.bus };
    r.init(0, &mut hw);
    Some(Box::new(r))
}

impl RobHubbardSt {
    fn hardware(&self, hw: &mut Hw) {
        for (n, (ch, smp)) in [(2, SMP_V3), (1, SMP_V2_INIT), (0, SMP_V1), (3, SMP_RESTART)].into_iter().enumerate() {
            if n > 0 {
                hw.wait(14);
            }
            hw.loc(ch, self.base + smp);
        }
        for ch in 0..4 {
            hw.wait(10);
            hw.len(ch, 4);
        }
        for ch in 0..4 {
            hw.wait(if ch == 0 { 10 } else { 8 });
            hw.vol(ch, 0);
        }
        for ch in 0..4 {
            hw.wait(if ch == 0 { 8 } else { 10 });
            hw.per(ch, 447);
        }
        hw.wait(10);
        hw.bus.write_word(ADKCON, 0x00FF);
        hw.wait(10);
        hw.dmacon(0x820F);
    }

    /// The running time of the tick's work so far, in colour clocks, from its events.
    fn cost(&self) -> u64 {
        let w = if self.b.a { &COST_A } else { &COST_B };
        let sum: i64 = self.ev.iter().zip(w.iter()).map(|(&e, &w)| e as i64 * w as i64).sum::<i64>() + w[NEV] as i64;
        sum.max(0) as u64
    }

    fn init(&mut self, n: usize, hw: &mut Hw) {
        self.started = 0;
        self.hardware(hw);
        self.mask = 248;
        self.subsong = n;
        let s = self.b.song;
        let def = self.b.env + be32(&self.d, self.b.env) as usize;
        for i in 0..3 {
            let base = s + be32(&self.d, s + 12 * n + 4 * i) as usize;
            let vo = &mut self.v[i];
            vo.base = base;
            vo.countdown = 1;
            vo.flags = 0;
            vo.mode = 0;
            vo.index = 4;
            vo.stream = s + be32(&self.d, base) as usize;
            vo.env_loop = def;
            vo.env_ptr = def;
        }
        self.tickc = 1;
        self.started = 255;
    }

    /// The end routine: stop, clear the effect state and the shadows, silence the hardware.
    fn end_routine(&mut self, hw: &mut Hw) {
        self.started = 0;
        self.sfx2 = 0;
        self.sweep = 0;
        self.per = [0; 3];
        self.mode396 = 0;
        self.flag397 = 0;
        self.vols = [0; 3];
        self.hardware(hw);
        self.flag397 = 255;
    }

    /// Reads the next track entry of voice `i` into its stream pointer.
    fn track_advance(&mut self, i: usize) {
        let s = self.b.song;
        let vo = &mut self.v[i];
        let mut e = be32(&self.d, vo.base + vo.index);
        vo.index += 4;
        if e == 0 {
            vo.index = 4;
            e = be32(&self.d, vo.base);
            if vo.base == self.long {
                self.songend = true;
            }
        }
        vo.stream = s + e as usize;
    }

    /// The row procedure of voice `i`. Returns false when the song-end command
    /// abandoned the tick.
    fn row(&mut self, i: usize, hw: &mut Hw) -> bool {
        let vo = &mut self.v[i];
        vo.countdown = vo.countdown.wrapping_sub(1);
        if vo.countdown != 0 {
            return true;
        }
        vo.flags = 0;
        vo.flag96 &= !1;
        self.ev[E_ROW_VOICE] += 1;
        for _ in 0..100_000 {
            let vo = &mut self.v[i];
            let b = rb(&self.d, vo.stream);
            vo.stream += 1;
            if (128..=183).contains(&b) {
                self.ev[E_CMD] += 1;
            }
            match b {
                0..=127 => {
                    vo.note = b as u16;
                    self.ev[if vo.flags & 16 == 0 { E_NOTE } else { E_TIE }] += 1;
                    if vo.flags & 16 == 0 {
                        vo.vol = vo.init_vol;
                        vo.flags |= 32 | 64;
                        vo.flags &= !16;
                        vo.level = vo.attack;
                        vo.count = vo.decay;
                    }
                    break;
                }
                128..=183 => match b - 128 {
                    0 => {
                        vo.vol = 0;
                        vo.flags &= !32;
                        break;
                    }
                    1 => vo.mode = 0,
                    2 => vo.mode = 64,
                    3 => vo.mode = 192,
                    4 => {
                        self.ev[E_CMD_VIB] += 1;
                        vo.vib_step = rb(&self.d, vo.stream) as i8 as i16 as u16;
                        vo.vib_acc = 0;
                        vo.vib_delay = rb(&self.d, vo.stream + 1) as u16;
                        vo.stream += 2;
                        vo.flags |= 4;
                    }
                    5 => vo.flags |= 8,
                    6 => vo.flags |= 0x88,
                    7 => {
                        self.ev[E_TRACK] += 1;
                        self.track_advance(i)
                    }
                    8 => {
                        self.ev[E_CMD_SLIDE] += 1;
                        vo.slide_step = rb(&self.d, vo.stream) as u16;
                        vo.slide_limit = rb(&self.d, vo.stream + 1) as u16;
                        vo.slide_acc = vo.slide_limit;
                        vo.stream += 2;
                    }
                    9 => {
                        self.gmode = rb(&self.d, vo.stream);
                        vo.stream += 1;
                    }
                    10 => {
                        let m = vo.vmask & 56;
                        self.mask |= m;
                        self.mask &= !(m >> 3);
                    }
                    11 => {
                        let m = vo.vmask & 7;
                        self.mask |= m;
                        self.mask &= !(m << 3);
                    }
                    12 => self.mask &= !vo.vmask,
                    13 => vo.flags |= 2,
                    14 => {
                        self.songend = true;
                        hw.wait(self.cost());
                        self.end_routine(hw);
                        return false;
                    }
                    15 => {
                        vo.flags &= !32;
                        break;
                    }
                    16 => vo.flags |= 16,
                    17 => vo.flag96 |= 1,
                    _ => {}
                },
                184..=191 => {
                    self.ev[E_SPEED] += 1;
                    self.speed = (b - 183) as u16
                }
                192..=207 => {
                    self.ev[E_ENVSEL] += 1;
                    let p = self.b.env + be32(&self.d, self.b.env + 4 * (b - 192) as usize) as usize;
                    vo.env_ptr = p;
                    vo.env_loop = p;
                }
                208..=223 => {
                    self.ev[E_ADSR] += 1;
                    vo.init_vol = b - 208;
                    vo.attack = rb(&self.d, vo.stream);
                    vo.release = rb(&self.d, vo.stream + 1);
                    vo.decay = rb(&self.d, vo.stream + 2);
                    vo.stream += 3;
                }
                224..=255 => {
                    self.ev[E_DUR] += 1;
                    vo.duration = (b - 223) as u16
                }
            }
        }
        let vo = &mut self.v[i];
        vo.countdown = vo.duration;
        true
    }

    /// The per-tick computation of voice `i`: its shadows, the mask step.
    fn voice(&mut self, i: usize) {
        let d = &self.d;
        let vo = &mut self.v[i];
        let mut f = vo.flags;
        let (mut v, mut e, mut n) = (vo.vol, vo.level, vo.count);
        if f & 32 != 0 {
            if e >= 16 {
                self.ev[E_ENV_GE16] += 1;
                e -= 16;
            } else if f & 64 != 0 {
                self.ev[E_ATTACK] += 1;
                let t = (e as u16 + 240 + v as u16).min(255);
                v = (t + 16) as u8;
                if n >= 16 {
                    self.ev[E_ATTACK_N] += 1;
                    n -= 16;
                } else {
                    f &= !64;
                    e = vo.release;
                }
            } else {
                self.ev[E_DECAY] += 1;
                v = v.wrapping_sub(e);
                if v & 0x80 != 0 {
                    v = 0;
                }
                n = n.wrapping_sub(1);
                if n == 0 {
                    f &= !32;
                }
            }
        }
        vo.vol = v;
        vo.level = e;
        vo.count = n;
        // the envelope stream
        let mut b = rb(d, vo.env_ptr);
        vo.env_ptr += 1;
        if b == 135 {
            self.ev[E_WRAP] += 1;
            vo.env_ptr = vo.env_loop;
            b = rb(d, vo.env_ptr);
            vo.env_ptr += 1;
        }
        let index = (b as u16).wrapping_add(vo.note);
        let mut p = be16(d, self.b.per + 2 * index as usize);
        if vo.mode & 64 != 0 {
            self.ev[E_SLIDE] += 1;
            let l = vo.slide_limit.wrapping_mul(2);
            let mut s = vo.slide_acc;
            if vo.mode & 128 != 0 && f & 1 != 0 {
                // held
            } else if vo.mode & 32 == 0 {
                s = s.wrapping_sub(vo.slide_step);
                if (s as i16) < 0 {
                    vo.mode |= 32;
                    s = 0;
                }
            } else {
                s = s.wrapping_add(vo.slide_step);
                if s > l {
                    vo.mode &= !32;
                    s = l;
                }
            }
            vo.slide_acc = s;
            let mut dd = s.wrapping_sub(l >> 1);
            let mut t = (2 * index as i16).wrapping_sub(96);
            while t < 0 {
                self.ev[E_SHIFT] += 1;
                dd = dd.wrapping_shl(1);
                t += 24;
            }
            p = p.wrapping_add(dd);
        }
        f ^= 1;
        if f & 4 != 0 {
            self.ev[E_VIB] += 1;
            let dl = vo.vib_delay.wrapping_sub(1);
            if dl != 0 {
                vo.vib_delay = dl;
            } else {
                vo.vib_acc = vo.vib_acc.wrapping_add(vo.vib_step);
                p = p.wrapping_add(vo.vib_acc);
            }
        }
        vo.flags = f;
        // the mask step
        let value;
        if vo.flag96 & 1 != 0 {
            vo.flag96 &= !1;
            self.dmode = 0;
            self.ev[E_MASK1] += 1;
            value = 7;
        } else if f & 3 == 3 {
            self.dmode = self.gmode ^ 8;
            self.ev[E_MASK2] += 1;
            value = 7;
        } else {
            value = self.mask;
        }
        let m = vo.vmask;
        self.flag397 = (self.flag397 & !m) | (value & m);
        self.per[i] = p;
        self.vols[i] = v;
    }

    fn output(&mut self, hw: &mut Hw) {
        let vol = |v: u8| ((v as u16) << 2) & 63;
        let base = self.base;
        let gated = !self.b.a && self.sfx2 != 0;
        hw.vol(0, vol(self.vols[0]));
        hw.wait(56);
        hw.per(0, self.per[0] << 2);
        if !gated {
            hw.wait(if self.b.a { 96 } else { 106 });
            hw.vol(1, vol(self.vols[1]));
            if self.flag397 & 16 == 0 {
                hw.wait(62);
                hw.loc(1, base + self.b.smp2);
                hw.wait(20);
                hw.per(1, ((self.mode396 as u16 + 2) << 2) << 2);
                hw.wait(14);
                hw.len(1, 100);
            } else {
                hw.wait(54);
                hw.loc(1, base + SMP_V2_ALT);
                hw.wait(20);
                hw.per(1, self.per[1] << 2);
                hw.wait(14);
                hw.len(1, 4);
            }
            hw.wait(102);
        } else {
            hw.wait(102);
        }
        hw.vol(2, vol(self.vols[2]));
        hw.wait(58);
        hw.per(2, self.per[2] << 2);
    }

    fn tail(&mut self, hw: &mut Hw) {
        let base = self.base;
        if self.b.a && self.sweep != 0 {
            self.sweep = self.sweep.wrapping_add(7);
            if self.sweep > 340 {
                self.sweep = 0;
                hw.vol(0, 0);
                self.vols[0] = 0;
                self.flag397 |= 36;
            }
            if self.sweep != 0 {
                hw.loc(0, base + SMP_V1);
                hw.len(0, 4);
                hw.vol(0, 6);
                hw.per(0, self.sweep << 2);
            }
        }
        if self.sfx2 != 0 {
            let old = self.sfx2;
            self.sfx2_sub = self.sfx2_sub.wrapping_sub(1);
            if self.sfx2_sub == 0 {
                self.sfx2_sub = 2;
                self.sfx2 -= 1;
            }
            hw.loc(1, base + self.b.smp2);
            hw.len(1, 100);
            hw.vol(1, ((old as u16) << 2) & 63);
            hw.per(1, 2550);
        } else if self.b.a && self.started == 0 {
            hw.vol(1, 0);
        }
    }
}

impl Replayer for RobHubbardSt {
    fn format(&self) -> &'static str {
        "Rob Hubbard ST"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.songs as u16 - 1, self.subsong as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.songs - 1);
        self.songend = false;
        self.init(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        hw.wait(T_FIRST);
        hw.loc(3, self.base + SMP_RESTART);
        hw.wait(T_ACK);
        hw.len(3, 4);
        hw.wait(T_ACK);
        hw.bus.write_word(INTREQ, self.b.ack);
        self.ev = [0; NEV];
        if self.started != 0 {
            self.dmode = self.gmode;
            self.tickc = self.tickc.wrapping_sub(1);
            let mut live = true;
            if self.tickc == 0 {
                self.ev[E_ROW] += 1;
                for i in 0..3 {
                    if !self.row(i, hw) {
                        live = false;
                        break;
                    }
                }
                self.tickc = self.speed;
            }
            if live {
                for i in 0..3 {
                    self.ev[E_VOICE] += 1;
                    self.voice(i);
                }
                self.mode396 = self.dmode;
            }
            hw.wait(if live { self.cost() } else { T_ABANDON });
            self.output(hw);
        }
        self.tail(hw);
    }
}

#[cfg(test)]
#[path = "robhubbardst_tests.rs"]
mod tests;
