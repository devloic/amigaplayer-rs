//! EMS v6 (`*.ems`, "E.M.S. V6.0x": Sean Connolly's Electronic Music System, the game music of
//! Stormlord, Tropical Islands, A Sporting Chance, Robocop 3 and others).
//!
//! Clean-room, written from the behaviour specification `docs/specs/ems6/SPEC.md` and black-box
//! runs; no original code, and no byte of any player. The specification was drawn up from
//! observation only: the Paula register writes of the reference (UADE's `EMS-6` Eagleplayer on this
//! project's 68000 engine), the bytes it writes into its output buffers, which bytes of the module
//! it reads and when, and how the output changes when module bytes are changed (built modules).
//! Nothing of the player was disassembled or read.
//!
//! The format is a software mixer: up to 32 tracks, even ones mixed to the left, odd ones to the
//! right, into two 32 KB rings of 8-bit samples that Paula plays in a loop at period 253 (two
//! channels a side). A CIA timer interrupt every 71050 colour clocks mixes the next 280 or 282
//! samples (282 when an accumulator stepping by 97 passes 512) and runs the sequencer every `tlen`
//! samples on the way. Each sample of a side is the sum over its voices of `s * vol >> 6`,
//! divided (truncated) by the module's voice count `a`. Paula's pointer leads the mixer by about
//! 1100 samples, so what is heard is the ring's previous lap (silence for the first 2.3 s).
//!
//! Choices where the specification is silent (none can panic or hang):
//! * an arpeggio that takes a note past the pitch table (index 0x80 to 0xFF, which only notes
//!   written with bit 7 set reach) reads the original's memory past its table; the bytes there are
//!   the player's, not the format's, so this port does not reproduce them and plays the index
//!   modulo 128 (SPEC section 5.3);
//! * the fine tune (`EF`) of the notes 0x60 to 0x7F, which are on a table of their own, is not
//!   applied; a volume above 64 is used as a factor as it is;
//! * reads outside the file give zeros; a pattern past the file ends; a sequence word past the
//!   track's list ends the list; a row reads at most 256 bytes and a pattern change looks at most
//!   at 64 sequence words;
//! * a voice count `a` over 255 or a track count over 32 is refused; the start-up time for `a`
//!   over 64 is the formula fitted to the measured ones.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "EMS v6", detect, new };

/// Colour clocks between two mixer interrupts (CIA timer value 0x3781).
const TICK: u64 = 71050;
/// Colour clocks from the reference's timer start to the end of its start (the song's start): 21,
/// or 22 for the voice counts `a` whose bit `a - 1` is set here (measured for 1 to 64; 21 above).
const START_22: u64 = 0x3565_4D58_4CA0_F4BE;

fn start_after_timer(a: u32) -> u64 {
    if (1..=64).contains(&a) && START_22 >> (a - 1) & 1 != 0 {
        22
    } else {
        21
    }
}
/// The ring of one side, in bytes (Paula's length 0x4000 words).
const RING: usize = 0x8000;
/// Paula's period for the rings.
const PERIOD: u16 = 0xFD;
/// The colour-clock constant of the period to step conversion (SPEC 5.1).
const PCLOCK: u32 = 3_579_264;
/// The mixing rate's divisor in the step conversion.
const RATE_DIV: u32 = 3504;

/// Steps (16.16 bytes a sample) of the notes 0x60 to 0x7F: measured frequency words (SPEC 5.2).
const F_HI: [u32; 32] = [
    248, 263, 278, 295, 312, 331, 351, 372, 394, 418, 442, 468, 497, 526, 557, 591, 625, 663, 702, 744, 789, 836,
    885, 937, 994, 1052, 1115, 1182, 1251, 1327, 1404, 1488,
];

/// Colour clock of the reference's DMA start (DMACON 800F) for the voice counts 1 to 64: its
/// start-up builds tables whose size grows with `a` (SPEC section 9).
const DMA_ON: [u32; 64] = [
    162145, 169254, 179915, 194130, 211895, 233212, 258082, 286503, 318477, 354002, 393079, 435710, 481891, 531624,
    584910, 641747, 702137, 766078, 833571, 904618, 979215, 1057364, 1139066, 1224319, 1313125, 1405482, 1501391,
    1600854, 1703867, 1810432, 1920550, 2034219, 2151441, 2272214, 2396539, 2524418, 2655847, 2790828, 2929362,
    3071447, 3217085, 3366274, 3519015, 3675310, 3835155, 3998552, 4165502, 4336003, 4510057, 4687662, 4868819,
    5053530, 5241791, 5433604, 5628970, 5827887, 6030357, 6236378, 6445951, 6659078, 6875755, 7095984, 7319766,
    7547099,
];
/// Colour clock of the reference's timer start for the voice counts 1 to 64 (the first of its
/// host's 71051-clock slots after the DMA start, at least 5700 clocks later).
const TIMER_ON: [u32; 64] = [
    185688, 185687, 185687, 256739, 256739, 256739, 327789, 327789, 327790, 398841, 398840, 469892, 540943, 540943,
    611994, 683044, 754096, 825147, 896198, 967249, 1038300, 1109351, 1180402, 1251453, 1322503, 1464605, 1535657,
    1606708, 1748809, 1819860, 1961962, 2104065, 2175116, 2317218, 2459320, 2530370, 2672472, 2814575, 2956676,
    3098779, 3240881, 3382983, 3525084, 3738238, 3880340, 4022442, 4235595, 4377697, 4519799, 4732952, 4875053,
    5088207, 5301360, 5443461, 5656614, 5869768, 6082921, 6296074, 6509226, 6722380, 6935532, 7148685, 7361840,
    7574993,
];
/// The reference's first register write (DMACON 000F) is at 3209 + 2.126 `a` colour clocks.
fn first_write(a: u32) -> u64 {
    (3209.04 + 2.126 * a as f64).round() as u64
}

fn start_times(a: u32) -> (u64, u64) {
    if (1..=64).contains(&a) {
        return (DMA_ON[a as usize - 1] as u64, TIMER_ON[a as usize - 1] as u64);
    }
    let x = a as f64;
    let on = (1776.0 * x * x + 1781.5 * x + 158587.25).round() as u64;
    let k = (on + 5700).saturating_sub(185688).div_ceil(71051);
    (on, 185688 + 71051 * k)
}

/// The period rows of the fine tunes 0 to 15 (0..7, -8..-1) for the notes 0 to 0x5F: the
/// ProTracker rows (octaves 1-3) scaled by 16, 8, 4 and 2 below, octave 3 halved above (SPEC 5.1).
fn period_rows() -> Vec<[u16; 96]> {
    let pt = &crate::protracker::PERIODS;
    (0..16)
        .map(|k| {
            let r = &pt[k * 37..k * 37 + 36];
            let mut q = [0u16; 96];
            for i in 0..12 {
                for (o, m) in [16u16, 8, 4, 2].iter().enumerate() {
                    q[o * 12 + i] = r[i] * m;
                }
                q[48 + i] = r[i];
                q[60 + i] = r[12 + i];
                q[72 + i] = r[24 + i];
                q[84 + i] = r[24 + i] / 2;
            }
            q
        })
        .collect()
}

fn s8(x: u8) -> i32 {
    x as i8 as i32
}

/// The 16.16 step of period `q`, or of note `n` 0x60..0x7F (no period).
fn step_of(q: Option<i32>, n: usize) -> u32 {
    match q {
        Some(q) => ((PCLOCK / q.max(1) as u32) as u64 * 65536 / RATE_DIV as u64) as u32,
        None if (0x60..0x80).contains(&n) => F_HI[n - 0x60] * 65536 / RATE_DIV,
        None => 0,
    }
}

const MAGIC: &[u8] = b"E.M.S. V6.";

/// The layout: block offsets and the voice and track counts.
struct Layout {
    b0: usize,
    b1: usize,
    b2: usize,
    b3: usize,
    l0: usize,
    l1: usize,
    l2: usize,
    a: u32,
    tracks: usize,
}

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 64 || &d[..10] != MAGIC {
        return None;
    }
    let l0 = be32(d, 0x0E) as usize;
    let l1 = be32(d, 0x12) as usize;
    let l2 = be32(d, 0x16) as usize;
    let l3 = be32(d, 0x1A) as usize;
    if l0 == 0 || l0 > d.len() {
        return None;
    }
    let b1 = 0x22 + l0;
    let (a, b) = (be16(d, b1) as u32, be16(d, b1 + 2) as usize);
    if a == 0 || a > 255 || b == 0 || b > 32 {
        return None;
    }
    let b2 = b1.checked_add(l1)?;
    let b3 = 0x22usize.checked_add(l3)?;
    Some(Layout { b0: 0x22, b1, b2, b3, l0, l1, l2, a, tracks: b })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// An IFF 8SVX sample: the data at +0x68, one-shot and repeat lengths.
    Svx { one: i64, rep: i64 },
    /// `SYN@`: 64-byte waves from +6.
    Syn,
    /// Anything else: raw bytes from the start, the record's length (to the next record).
    Raw { len: i64 },
}

#[derive(Clone, Copy)]
struct Inst {
    start: usize,
    kind: Kind,
}

#[derive(Default, Clone, Copy)]
struct Voice {
    on: bool,
    inst: Option<usize>,
    /// 16.16 position from `base`.
    pos: i64,
    step: u32,
    vol: i32,
    pending: bool,
    hold: bool,
    p0: i64,
    end: i64,
    base: usize,
    ls: i64,
    lr: i64,
    disp: i64,
    dispw: i32,
    /// The instrument is raw bytes (not 8SVX or `SYN@`).
    raw: bool,
}

#[derive(Clone, Copy)]
struct Slide {
    target: i32,
    delta: i32,
    count: i32,
    delay: i32,
    note: usize,
}

#[derive(Clone, Copy)]
struct Retrig {
    every: i32,
    left: i32,
    times: i32,
}

/// A pattern: bytes `start..end` of the module (empty: one that ends at once).
#[derive(Clone, Copy, Default)]
struct Pat {
    start: usize,
    end: usize,
}

struct Track {
    seq: Vec<u16>,
    si: usize,
    rep: i32,
    tr: i32,
    iofs: usize,
    /// None: the track is off (FFFE).
    pat: Option<Pat>,
    pi: usize,
    wait: i32,
    dur: u32,
    inst: usize,
    env: [u8; 4],
    vol: i32,
    cnt: i32,
    f4on: bool,
    f4cnt: u8,
    rel: (u8, u8),
    vib: (u8, u8, u8),
    vib_v: i32,
    vib_ph: u8,
    vib_cnt: i32,
    n: usize,
    raw: usize,
    legato: bool,
    ft: usize,
    q: Option<i32>,
    slide: Option<Slide>,
    f3: Option<(u8, u8)>,
    f2: Option<[u8; 4]>,
    wave: i32,
    f2up: bool,
    retrig: Option<Retrig>,
    relsub: i32,
    arp: Option<usize>,
    arp_i: usize,
    arp_last: u8,
    looped: bool,
    v: Voice,
}

pub struct Ems6 {
    d: Vec<u8>,
    a: i32,
    b2: usize,
    pats: Vec<Pat>,
    tables: Vec<Pat>,
    insts: Vec<Inst>,
    qrows: Vec<[u16; 96]>,
    tracks: Vec<Track>,
    speed: i32,
    tlen: u32,
    tleft: u32,
    acc: u32,
    irqs: u32,
    ring: u32,
    w: usize,
    delay: u64,
    songend: bool,
}

/// Offsets of a block's records: the first long / 4 longs from `at`, each record running to the
/// next one (the last to `end`).
fn records(d: &[u8], at: usize, end: usize) -> Vec<Pat> {
    let n = (be32(d, at) / 4).min(4096) as usize;
    let offs: Vec<usize> = (0..n).map(|i| be32(d, at + 4 * i) as usize).collect();
    (0..n)
        .map(|i| {
            let s = at.saturating_add(offs[i]);
            let e = if i + 1 < n { at.saturating_add(offs[i + 1]) } else { end };
            Pat { start: s.min(d.len()), end: e.min(d.len()).max(s.min(d.len())) }
        })
        .collect()
}

impl Ems6 {
    fn build(d: &[u8], ring: u32) -> Option<Self> {
        let lay = layout(d)?;
        let pats = records(d, lay.b2, lay.b2.saturating_add(lay.l2));
        let tables = records(d, lay.b0, lay.b0 + lay.l0);
        // instruments: the offset list up to an FFFFFFFF; a raw record's length is the next offset
        // less its own (the last one's runs to the FFFFFFFF, negative)
        let n3 = (be32(d, lay.b3) / 4).min(4096) as usize;
        let offs: Vec<u32> = (0..n3).map(|i| be32(d, lay.b3 + 4 * i)).collect();
        let mut insts = Vec::new();
        for k in 0..n3 {
            if offs[k] == 0xFFFF_FFFF {
                break;
            }
            let start = lay.b3.saturating_add(offs[k] as usize);
            let next = if k + 1 < n3 { offs[k + 1] } else { 0 };
            let len = next.wrapping_sub(offs[k]) as i32 as i64;
            let at = |o: usize| d.get(start.saturating_add(o)..start.saturating_add(o + 4));
            let kind = if at(0) == Some(b"FORM") && at(8) == Some(b"8SVX") {
                Kind::Svx { one: be32(d, start + 0x14) as i64, rep: be32(d, start + 0x18) as i64 }
            } else if at(0) == Some(b"SYN@") {
                Kind::Syn
            } else {
                Kind::Raw { len }
            };
            insts.push(Inst { start, kind });
        }
        let seq_of = |i: usize| -> Vec<u16> {
            let o = be32(d, lay.b1 + 4 + 4 * i) as usize;
            let e = if i + 1 < 32 { be32(d, lay.b1 + 8 + 4 * i) as usize } else { lay.l1 };
            let (s, e) = (lay.b1.saturating_add(o), lay.b1.saturating_add(e).min(d.len()));
            (s..e).step_by(2).take(0x8000).map(|p| be16(d, p)).collect()
        };
        let mut r = Ems6 {
            d: d.to_vec(),
            a: lay.a as i32,
            b2: lay.b2,
            pats,
            tables,
            insts,
            qrows: period_rows(),
            tracks: Vec::new(),
            speed: 6,
            tlen: 280,
            tleft: 0,
            acc: 450,
            irqs: 0,
            ring,
            w: 0,
            delay: 0,
            songend: false,
        };
        for i in 0..lay.tracks {
            let mut t = Track::new(seq_of(i));
            r.next_pattern(&mut t);
            r.tracks.push(t);
        }
        Some(r)
    }

    fn byte(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// The pattern of sequence word `w` (below 0x8000).
    fn pattern(&self, w: usize) -> Pat {
        self.pats.get(w).copied().unwrap_or_default()
    }

    /// The track's next pattern from its sequence (SPEC section 3).
    fn next_pattern(&self, t: &mut Track) {
        if t.rep > 0 {
            t.rep -= 1;
            t.pi = 0;
            return;
        }
        for _ in 0..64 {
            let Some(&w) = t.seq.get(t.si) else {
                t.pat = Some(Pat::default());
                return;
            };
            t.si += 1;
            match w {
                0xFFFF => {
                    t.looped = true;
                    t.si = t.seq.get(t.si).map_or(0, |&x| x as usize);
                }
                0xFFFE => {
                    t.pat = None;
                    return;
                }
                0x8000.. => match w >> 8 {
                    0xFB => t.rep = (w & 0xFF) as i32 - 1,
                    0xFD => t.tr = s8(w as u8),
                    0xFC => t.iofs = (w & 0xFF) as usize,
                    0xFA => t.si += 1,
                    0xF8 | 0xF9 => {
                        // played as a pattern whose data starts at block 2 (its offset table)
                        t.pat = Some(Pat { start: self.b2.min(self.d.len()), end: self.d.len() });
                        t.pi = 0;
                        return;
                    }
                    _ => {}
                },
                _ => {
                    t.pat = Some(self.pattern(w as usize));
                    t.pi = 0;
                    return;
                }
            }
        }
    }

    fn qrow(&self, t: &Track) -> &[u16; 96] {
        &self.qrows[t.ft]
    }

    fn period(&self, t: &Track, n: usize) -> Option<i32> {
        (n < 0x60).then(|| self.qrow(t)[n] as i32)
    }

    /// A note (SPEC section 4); `retrig` starts the instrument's sample.
    fn note(&self, t: &mut Track, n: u8, retrig: bool) {
        t.raw = (n as i32 + t.tr).rem_euclid(256) as usize;
        t.n = t.raw & 0x7F;
        t.q = self.period(t, t.n);
        t.slide = None;
        if retrig {
            let k = (t.inst + t.iofs) & 0xFF;
            if let Some(ins) = self.insts.get(k) {
                let v = &mut t.v;
                v.on = true;
                v.pending = false;
                v.disp = 0;
                v.inst = Some(k);
                v.pos = 0;
                v.raw = matches!(ins.kind, Kind::Raw { .. });
                match ins.kind {
                    Kind::Svx { one, rep } => {
                        v.base = ins.start + 0x68;
                        v.end = one + rep;
                        v.ls = one;
                        v.lr = rep;
                    }
                    Kind::Syn => {
                        v.base = ins.start + 6;
                        v.end = 64;
                        t.wave = t.f2.map_or(0, |f| f[2] as i32);
                        v.ls = 64 * t.wave as i64;
                        v.lr = 64;
                        t.f2up = true;
                    }
                    Kind::Raw { len } => {
                        v.base = ins.start;
                        v.end = len;
                        v.lr = 0;
                    }
                }
            }
        }
        t.retrig = None;
        t.relsub = 0;
        t.arp_i = 0;
        t.arp_last = 0;
        if let Some((a, b)) = t.f3.take() {
            t.retrig = Some(Retrig { every: a as i32, left: a as i32, times: b as i32 - 1 });
        }
        t.vib_v = 0;
        t.vib_ph = 0;
        t.vib_cnt = t.vib.2 as i32;
    }

    /// F3: the sample again from its start.
    fn restart(&self, t: &mut Track) {
        let v = &mut t.v;
        v.pos = 0;
        v.pending = false;
        v.disp = 0;
        match v.inst.map(|k| self.insts[k].kind) {
            Some(Kind::Svx { one, rep }) => v.end = one + rep,
            Some(Kind::Syn) => v.end = 64,
            _ => {}
        }
    }

    fn set_wait(&self, t: &mut Track) {
        let rows = if t.dur == 0 { 256 } else { t.dur as i32 };
        t.wait = rows * self.speed - 1;
    }

    /// One row of the track's pattern: commands up to a note or a hold (SPEC section 4).
    fn parse(&mut self, t: &mut Track) {
        for _ in 0..256 {
            let Some(p) = t.pat else { return };
            if p.start + t.pi >= p.end {
                self.next_pattern(t);
                if t.pat.is_none() {
                    return;
                }
                continue;
            }
            let at = p.start + t.pi;
            let b = self.byte(at);
            t.pi += 1;
            if b < 0xEF {
                self.note(t, b, !t.legato);
                self.set_wait(t);
                return;
            }
            let arg = |k: usize| if at + 1 + k < p.end { self.byte(at + 1 + k) } else { 0 };
            let (a0, a1, a2, a3) = (arg(0), arg(1), arg(2), arg(3));
            match b {
                0xFF => {
                    self.next_pattern(t);
                    if t.pat.is_none() {
                        return;
                    }
                }
                0xFD => {
                    t.dur = a0 as u32;
                    t.pi += 1;
                }
                0xFE => {
                    t.inst = a0 as usize;
                    t.pi += 1;
                }
                0xF7 => {
                    t.env = [a0, a1, a2, a3];
                    t.vol = a0 as i32;
                    t.cnt = a3 as i32;
                    t.pi += 4;
                }
                0xF3 => {
                    t.f3 = Some((a0, a1));
                    t.pi += 2;
                }
                0xFC => {
                    if a0 < 0x20 {
                        self.speed = a0 as i32;
                    } else {
                        // 14019 * 2.5 / x
                        self.tlen = 70095 / (2 * a0 as u32);
                    }
                    t.pi += 1;
                }
                0xEF => {
                    t.ft = (a0 & 0x0F) as usize;
                    t.pi += 1;
                }
                0xF0 | 0xF1 => t.legato = b == 0xF0,
                0xF6 => {
                    if t.v.inst.is_some() {
                        t.v.on = true;
                    }
                    self.set_wait(t);
                    return;
                }
                0xF4 => {
                    if !t.f4on {
                        t.f4on = true;
                        t.f4cnt = a0;
                    }
                    t.f4cnt = t.f4cnt.wrapping_sub(1);
                    if t.f4cnt == 0 {
                        t.f4on = false;
                        t.pi += 2;
                    } else {
                        t.pi -= 1;
                    }
                    self.note(t, a1, !t.legato);
                    self.set_wait(t);
                    return;
                }
                0xFB | 0xFA => {
                    t.pi += 4;
                    self.note(t, a0, b == 0xFB);
                    let n4 = (a3 as i32 + t.tr).rem_euclid(128) as usize;
                    let tq = self.period(t, n4);
                    if let (Some(q), Some(tq)) = (t.q, tq) {
                        if a1 != 0 {
                            let delta = (tq - q) / a1 as i32;
                            t.slide =
                                Some(Slide { target: tq, delta, count: a1 as i32, delay: a2 as i32, note: n4 });
                        }
                    }
                    self.set_wait(t);
                    return;
                }
                0xF2 => {
                    t.f2 = Some([a0, a1, a2, a3]);
                    t.pi += 4;
                }
                0xF9 => {
                    t.arp = ((a0 as usize) < self.tables.len()).then_some(a0 as usize);
                    t.pi += 1;
                }
                0xF8 => {
                    t.vib = (a0, a1, a2);
                    t.arp = None;
                    t.pi += 3;
                }
                0xF5 => {
                    t.rel = (a0, a1);
                    t.pi += 2;
                }
                _ => {}
            }
        }
    }

    fn arp_byte(&self, k: usize, i: usize) -> u8 {
        let p = self.tables[k];
        if p.start + i < p.end { self.byte(p.start + i) } else { 0xFE }
    }

    /// The track's tick: wave sweep, row, retrigger, volume, pitch (SPEC section 7.2).
    fn track_tick(&mut self, t: &mut Track) {
        if t.pat.is_none() {
            return;
        }
        if let Some([lo, hi, _, dd]) = t.f2 {
            if t.v.on && t.v.inst.is_some_and(|k| self.insts[k].kind == Kind::Syn) {
                let (lo, hi, dd) = (lo as i32, hi as i32, dd as i32);
                if t.f2up {
                    t.wave += dd;
                    if t.wave >= hi {
                        t.wave -= dd;
                        t.f2up = false;
                    }
                } else {
                    t.wave -= dd;
                    if t.wave <= lo {
                        t.wave += dd;
                        t.f2up = true;
                    }
                }
                t.v.ls = 64 * t.wave as i64;
            }
        }
        let mut just = false;
        if t.wait > 0 {
            t.wait -= 1;
        } else {
            self.parse(t);
            just = true;
        }
        if let Some(rt) = t.retrig.as_mut() {
            if rt.times > 0 && !just {
                rt.left -= 1;
                if rt.left == 0 {
                    rt.left = rt.every;
                    rt.times -= 1;
                    self.restart(t);
                }
            }
        }
        // volume: the release in the note's last ticks (the envelope stands still), else the
        // envelope
        let [_, target, speed, delay] = t.env;
        if t.wait < t.rel.0 as i32 {
            t.relsub += t.rel.1 as i32;
        } else {
            t.relsub = 0;
            if t.cnt == 0 {
                let (target, speed) = (target as i32, speed as i32);
                if t.vol < target {
                    t.vol = (t.vol + speed).min(target);
                } else if t.vol > target {
                    t.vol = (t.vol - speed).max(target);
                }
                t.cnt = delay as i32;
            } else {
                t.cnt -= 1;
            }
        }
        t.v.vol = (t.vol - t.relsub).max(0);
        // pitch: the slide, the arpeggio, the vibrato
        let sliding = t.slide.is_some();
        if let Some(sl) = t.slide.as_mut() {
            if sl.delay > 0 {
                sl.delay -= 1;
            } else {
                sl.count -= 1;
                if sl.count <= 0 {
                    t.q = Some(sl.target);
                    t.n = sl.note;
                    t.raw = sl.note;
                    t.slide = None;
                } else {
                    t.q = t.q.map(|q| q + sl.delta);
                }
            }
        }
        let mut q = t.q;
        let mut n = t.n;
        let mut arping = false;
        if let Some(k) = t.arp {
            let mut x = self.arp_byte(k, t.arp_i);
            if x == 0xFF {
                t.arp_i = 0;
                let p = self.tables[k];
                x = if p.end > p.start { self.byte(p.start) } else { 0 };
            }
            if x == 0xFE {
                x = t.arp_last;
            } else {
                t.arp_i += 1;
                arping = true;
            }
            t.arp_last = x;
            if x != 0 || t.raw >= 0x80 {
                // past the table (0x80..0xFF): the original's memory there; modulo 128 here
                let m = (t.raw + x as usize) & 0x7F;
                if m < 0x60 {
                    let row = self.qrow(t);
                    q = Some(match q {
                        Some(q) if n < 0x60 => row[m] as i32 + (q - row[n] as i32),
                        _ => row[m] as i32,
                    });
                } else {
                    q = None;
                }
                n = m;
            }
        }
        let (a, b, _) = t.vib;
        if a != 0 && b != 0 && !arping && !sliding {
            if t.vib_cnt > 0 {
                t.vib_cnt -= 1;
            } else {
                let b = b as i32;
                match t.vib_ph {
                    0 => {
                        t.vib_v += 1;
                        if t.vib_v >= b {
                            t.vib_ph = 1;
                        }
                    }
                    1 => {
                        t.vib_v -= 2;
                        if t.vib_v <= -b {
                            t.vib_ph = 2;
                        }
                    }
                    2 => {
                        t.vib_v += 1;
                        if t.vib_v >= 0 {
                            t.vib_ph = 3;
                        }
                    }
                    _ => t.vib_ph = 0,
                }
            }
            if let Some(qq) = q {
                if t.vib_v != 0 {
                    let row = self.qrow(t);
                    let d = if n > 0 && n < 0x60 { row[n - 1] as i32 - row[n] as i32 } else { 0 };
                    q = Some(qq + t.vib_v * d.div_euclid(a as i32));
                }
            }
        }
        t.v.step = step_of(q, n);
    }

    fn tick_tracks(&mut self) {
        let mut tracks = std::mem::take(&mut self.tracks);
        for t in tracks.iter_mut() {
            self.track_tick(t);
        }
        self.tracks = tracks;
        if !self.songend && self.tracks.iter().all(|t| t.looped || t.pat.is_none()) {
            self.songend = true;
        }
    }

    /// The loop at a voice's end: the loop start plus the overshoot, or the voice ends.
    fn wrap(v: &mut Voice) {
        if v.lr != 0 {
            v.pos = ((v.ls + (v.pos >> 16) - v.end) << 16) | (v.pos & 0xFFFF);
            v.end = v.ls + v.lr;
            if v.disp != 0 {
                v.dispw -= 1;
                if v.dispw == 0 {
                    v.disp = 0;
                }
            }
        } else {
            v.on = false;
        }
    }

    /// `cnt` samples of both sides into `out` from index `at` (SPEC section 7.3).
    fn mix(&mut self, out: &mut [[u8; 282]; 2], at: usize, cnt: usize) {
        let d = &self.d;
        let a = self.a;
        for side in 0..2 {
            let mut vs: Vec<&mut Voice> =
                self.tracks.iter_mut().skip(side).step_by(2).map(|t| &mut t.v).filter(|v| v.on).collect();
            for v in vs.iter_mut() {
                // a voice past its end when the part starts plays the part without a wrap; one
                // exactly at its end wraps before its first sample
                v.hold = (v.pos >> 16) > v.end && !v.pending;
                if !v.hold && (v.pos >> 16) >= v.end {
                    v.pending = true;
                }
                // a raw-instrument voice at its end keeps the start of the part it reached it in:
                // it stops and goes back there (and F6 plays on from there); an 8SVX one stays
                // at its end (Stormlord tick 3990, Tropical Islands tick 3756)
                if !(v.pending && v.raw) {
                    v.p0 = v.pos;
                }
            }
            for j in 0..cnt {
                let mut acc = 0i32;
                for v in vs.iter_mut() {
                    if v.pending {
                        v.pending = false;
                        Self::wrap(v);
                        if !v.on {
                            v.pos = v.p0;
                        }
                    }
                    if !v.on {
                        continue;
                    }
                    let ip = v.base as i64 + (v.pos >> 16) - v.disp;
                    let s = if ip >= 0 { d.get(ip as usize).map_or(0, |&x| s8(x)) } else { 0 };
                    acc += (s * v.vol) >> 6;
                    v.pos += v.step as i64;
                    if !v.hold && (v.pos >> 16) >= v.end {
                        v.pending = true;
                    }
                }
                out[side][at + j] = (acc / a) as u8;
            }
            // the part's end: a voice past its end wraps now and reads behind until its next wrap
            for v in vs.iter_mut() {
                if !v.on {
                    continue;
                }
                let ov = (v.pos >> 16) - v.end;
                if ov == 0 && !v.hold {
                    v.pending = true;
                    continue;
                }
                v.pending = false;
                if ov >= 0 {
                    let lr = v.lr;
                    Self::wrap(v);
                    if ov >= 1 && v.on {
                        v.disp = lr + ov;
                        v.dispw = 1;
                    }
                }
            }
        }
    }

    /// One mixer interrupt's block: 280 or 282 samples a side, the sequencer ticks where they fall.
    fn block(&mut self) -> ([[u8; 282]; 2], usize) {
        let mut out = [[0u8; 282]; 2];
        self.acc += 97;
        let mut n = 280;
        if self.acc >= 512 {
            self.acc -= 512;
            n = 282;
        }
        let mut at = 0;
        while at < n {
            if self.tleft == 0 {
                self.tick_tracks();
                self.tleft = self.tlen;
            }
            let k = (n - at).min(self.tleft as usize);
            self.mix(&mut out, at, k);
            self.tleft -= k as u32;
            at += k;
        }
        (out, n)
    }
}

impl Track {
    fn new(seq: Vec<u16>) -> Self {
        Track {
            seq,
            si: 0,
            rep: 0,
            tr: 0,
            iofs: 0,
            pat: Some(Pat::default()),
            pi: 0,
            wait: 0,
            dur: 1,
            inst: 0,
            env: [0; 4],
            vol: 0,
            cnt: 0,
            f4on: false,
            f4cnt: 0,
            rel: (0, 0),
            vib: (0, 0, 0),
            vib_v: 0,
            vib_ph: 0,
            vib_cnt: 0,
            n: 0,
            raw: 0,
            legato: false,
            ft: 0,
            q: None,
            slide: None,
            f3: None,
            f2: None,
            wave: 0,
            f2up: true,
            retrig: None,
            relsub: 0,
            arp: None,
            arp_i: 0,
            arp_last: 0,
            looped: false,
            v: Voice::default(),
        }
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let ring = m.alloc(2 * RING as u32);
    if ring == 0 {
        return None;
    }
    let mut r = Ems6::build(d, ring)?;
    let (on, timer) = start_times(r.a as u32);
    let mut hw = Hw { bus: &mut m.bus };
    // the reference's start-up writes, at its times (SPEC section 9)
    let t0 = first_write(r.a as u32);
    if hw.bus.clock < t0 {
        hw.wait(t0 - hw.bus.clock);
    }
    hw.bus.write_word(0xDFF096, 0x000F);
    hw.wait(71);
    for ch in 0..4 {
        hw.len(ch, (RING / 2) as u16);
        hw.wait(1);
    }
    for ch in 0..4 {
        hw.per(ch, PERIOD);
        hw.wait(1);
    }
    hw.wait(3);
    for (ch, a) in [(0, ring), (1, ring + RING as u32), (2, ring + RING as u32), (3, ring)] {
        hw.loc(ch, a);
        hw.wait(1);
    }
    for ch in 0..4 {
        hw.vol(ch, 64);
        hw.wait(1);
    }
    // the LED (filter) off
    let led = t0 + 31472;
    if hw.bus.clock < led {
        hw.wait(led - hw.bus.clock);
    }
    hw.bus.write_byte(0xBFE001, 0x02);
    if hw.bus.clock < on {
        hw.wait(on - hw.bus.clock);
    }
    hw.bus.write_word(0xDFF096, 0x800F);
    // the reference's start returns 21 or 22 colour clocks after it starts its timer: the song
    // (and Paula's output grid) starts there. Its first interrupt, which mixes nothing, would
    // then come less than a period later, which `timer_delay` cannot express; the engine's
    // first tick is the second interrupt (also mixing nothing), one period later.
    let start = timer + start_after_timer(r.a as u32);
    if hw.bus.clock < start {
        hw.wait(start - hw.bus.clock);
    }
    r.irqs = 1;
    r.delay = (timer + TICK).saturating_sub(hw.bus.clock);
    Some(Box::new(r))
}

impl Replayer for Ems6 {
    fn format(&self) -> &'static str {
        "EMS v6"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn timer_delay(&self) -> u64 {
        self.delay
    }

    fn needs_fast_reference_cpu(&self) -> bool {
        // the original's mixer misses timer interrupts on a 68000
        true
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        // the first two interrupts mix nothing (the engine's first tick is the second)
        if self.irqs < 2 {
            self.irqs += 1;
            return;
        }
        let (out, n) = self.block();
        for (side, o) in out.iter().enumerate() {
            let base = self.ring as usize + side * RING;
            for (j, &x) in o[..n].iter().enumerate() {
                let a = base + (self.w + j) % RING;
                if let Some(b) = hw.bus.ram.get_mut(a) {
                    *b = x;
                }
            }
        }
        self.w = (self.w + n) % RING;
    }
}

#[cfg(test)]
#[path = "ems6_tests.rs"]
mod tests;
