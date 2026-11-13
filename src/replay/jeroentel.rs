//! Jeroen Tel (`jt.*`), the music driver of the Jeroen Tel tunes (the corpus: the SL in-game
//! and title tunes, Tusker, Onslaught, Insects and the Unreal menu and title tunes).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no original
//! code. The specification was drawn up from the player's observable behaviour (the
//! Paula register writes of UADE's Jeroen Tel player and the structure of the corpus
//! files), and this player was written from it alone and checked against those writes.
//!
//! A `jt.*` file is a raw memory image: the replay routine, then its music and samples.
//! The code is not run. It is read only to find the player's tables: the instruction
//! forms the specification describes (a byte move and a shift, a compare with 255, a
//! move of a long immediate into an address register, loads of absolute addresses, an
//! indexed long move to an absolute address) are matched on decoded instructions
//! (`m68kdec`), and the stored link addresses are turned into file offsets with the
//! origin that the period table's position gives.
//!
//! Four voices at 50 Hz. A tick is a sequencing stage (rows, envelopes, vibrato, slides,
//! pitch sequences) and an output stage (period, volume and DMA-on of each voice).
//!
//! Untested by the corpus (written as the specification says): the prefix cell, the
//! instrument cell with a pitch-sequence selector, the wave-cycling path and the period
//! clamp for note indices of 36 and above.
//!
//! Decisions where the specification leaves the choice to the port:
//! * The silent buffer is 34 bytes of zeros in memory allocated for it.
//! * Notes are kept as the sum of the cell's byte and the voice's transposition (a byte,
//!   wrapping), and masked to 7 bits where they index the period table; the vibrato test
//!   looks at bit 7 of the sum.
//! * A cell with no note (the 0xC0 class) sets the duration only and starts nothing.
//! * Every cell with a note clears the slide speed; a slide cell sets it.
//! * The prefix cell takes its duration from the class byte that follows it.
//! * Wave cycling writes the sample address plus the index, and the index wraps to 0 at 32.
//! * The sequencing loops are bounded (a corrupt order list cannot hang the player).
//! * A song-end event sets a flag that stays set; the player carries on, as the logs do. The
//!   stop event sets it too (the original reports the end there).
//! * The host's start-up writes (before the first tick) are not made.

use super::m68kdec::{decode, Ea, Insn, K};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Jeroen Tel", detect, new };

/// The period table (note 0..35).
const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226,
    214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 114,
];

/// Where the player's tables are, as file offsets, and the origin of its link addresses.
#[derive(Clone, Debug, PartialEq)]
struct Lay {
    origin: u32,
    period: usize,
    repeat: [u8; 4],
    pattern: usize,
    inst: usize,
    pitch: usize,
    subtab: usize,
    subs: usize,
}

// ------------------------------------------------------------------ recognition

/// A byte AND of the immediate 1 into an absolute long address.
fn is_sig(d: &[u8], at: usize) -> bool {
    dec(d, at).is_some_and(|i| {
        i.k == K::Imm(1) && i.size == 1 && matches!(i.src, Ea::Imm { val: 1, .. }) && matches!(i.dst, Ea::AbsL { .. })
    })
}

/// A byte test of an absolute long address.
fn is_tst(d: &[u8], at: usize) -> bool {
    dec(d, at).is_some_and(|i| i.k == K::Tst && i.size == 1 && matches!(i.dst, Ea::AbsL { .. }))
}

/// A quick move of 0 into d4, then a byte move from an absolute long into d4.
fn is_load(d: &[u8], at: usize) -> bool {
    let Some(a) = dec(d, at) else { return false };
    if !(a.k == K::Moveq && a.data == 0 && a.dst == Ea::D(4)) {
        return false;
    }
    dec(d, a.next()).is_some_and(|b| b.k == K::Move && b.size == 1 && matches!(b.src, Ea::AbsL { .. }) && b.dst == Ea::D(4))
}

/// Section 1: the signature and the branch to the code that reads the channel state.
fn signature(d: &[u8]) -> bool {
    if d.len() <= 1700 {
        return false;
    }
    let Some(s) = (0..=38).step_by(2).find(|&o| is_sig(d, o)) else { return false };
    let Some(b) = dec(d, s + 8) else { return false };
    // a branch-if-not-equal, short, with a displacement of 1 to 127
    let disp = b.target as i64 - (b.at as i64 + 2);
    if !(b.k == K::Bcc && b.cond == 6 && b.size == 1 && (1..128).contains(&disp)) {
        return false;
    }
    let Some(r) = dec(d, b.next()) else { return false };
    if r.k != K::Rts {
        return false;
    }
    let t = r.at + disp as usize;
    // four byte tests 18 bytes apart, then the load 54 to 96 bytes after the first; or the load alone
    if (0..4).all(|i| is_tst(d, t + 18 * i)) {
        return (t + 54..=t + 96).step_by(2).any(|o| is_load(d, o));
    }
    is_load(d, t)
}

fn detect(d: &[u8]) -> bool {
    signature(d) && locate(d).is_ok_and(|l| l.subs > 0)
}

// ------------------------------------------------------------------ the locator

fn dec(d: &[u8], at: usize) -> Option<Insn> {
    decode(d, at)
}

/// A `move.b d0,d2` and an arithmetic shift left of d2 by 1 right after it.
fn is_k1(d: &[u8], at: usize) -> bool {
    let Some(a) = dec(d, at) else { return false };
    if !(a.k == K::Move && a.size == 1 && a.src == Ea::D(0) && a.dst == Ea::D(2)) {
        return false;
    }
    let Some(b) = dec(d, a.next()) else { return false };
    // an arithmetic shift left of the byte in d2 by 1
    b.k == K::Shift && b.size == 1 && b.dst == Ea::D(2) && b.data == 1 && b.left && b.shk == 0
}

/// A byte compare of d1 with the immediate 255 (CMP or CMPI).
fn is_k3(d: &[u8], at: usize) -> bool {
    let Some(a) = dec(d, at) else { return false };
    a.size == 1
        && match a.k {
            K::Imm(6) => a.dst == Ea::D(1) && matches!(a.src, Ea::Imm { val: 255, .. }),
            K::Cmp => a.dst == Ea::D(1) && matches!(a.src, Ea::Imm { val: 255, .. }),
            _ => false,
        }
}

/// `movea.l #imm,a3`: the immediate.
fn k4(d: &[u8], at: usize) -> Option<(u32, usize)> {
    let a = dec(d, at)?;
    match (a.k, a.size, a.src, a.dst) {
        (K::Movea, 4, Ea::Imm { val, .. }, Ea::A(3)) => Some((val, a.next())),
        _ => None,
    }
}

/// `lea abs.l,a4`: the address.
fn k5(d: &[u8], at: usize) -> Option<u32> {
    let a = dec(d, at)?;
    match (a.k, a.src, a.dst) {
        (K::Lea, Ea::AbsL { addr, .. }, Ea::A(4)) => Some(addr),
        _ => None,
    }
}

/// `move.l (d8,a4,Xn),abs.l`.
fn is_k7(d: &[u8], at: usize) -> bool {
    let Some(a) = dec(d, at) else { return false };
    a.k == K::Move && a.size == 4 && matches!(a.src, Ea::Idx(4, ..)) && matches!(a.dst, Ea::AbsL { .. })
}

/// Finds the tables. The error names the key that failed.
fn locate(d: &[u8]) -> Result<Lay, &'static str> {
    let n = d.len();
    // K2: the period table's first two entries
    let period = (0..n.saturating_sub(4)).step_by(2).find(|&o| be16(d, o) == 856 && be16(d, o + 2) == 808).ok_or("K2")?;
    // the whole table (section 4.1) must be the 36 periods
    if period + 72 > n || !PERIODS.iter().enumerate().all(|(i, &p)| be16(d, period + 2 * i) == p) {
        return Err("K2 table");
    }
    let k1 = (0..n.saturating_sub(10)).step_by(2).find(|&o| is_k1(d, o)).ok_or("K1")?;
    let plink = be32(d, k1 + 6);
    let origin = plink.checked_sub(period as u32).ok_or("K1 origin")?;
    let off = |link: u32| -> Result<usize, &'static str> {
        let o = link.checked_sub(origin).ok_or("link below origin")? as usize;
        if o < n { Ok(o) } else { Err("link beyond file") }
    };
    let k3 = (0..n.saturating_sub(4)).step_by(2).find(|&o| is_k3(d, o)).ok_or("K3")?;
    let mut repeat = [0u8; 4];
    // a byte move of an immediate to an absolute long address, 6 bytes after K3
    if let Some(m) = dec(d, k3 + 6) {
        if m.k == K::Move && m.size == 1 && matches!(m.src, Ea::Imm { .. }) && matches!(m.dst, Ea::AbsL { .. }) {
            for (i, r) in repeat.iter_mut().enumerate() {
                *r = d.get(k3 + 9 + 8 * i).copied().unwrap_or(0);
            }
        }
    }
    let (k4_at, pat_link) = (k3 + 2..n.saturating_sub(6))
        .step_by(2)
        .find_map(|o| k4(d, o).map(|(v, _)| (o, v)))
        .ok_or("K4")?;
    let (k5_at, inst_link) = (k4_at + 6..n.saturating_sub(6))
        .step_by(2)
        .find_map(|o| k5(d, o).map(|v| (o, v)))
        .ok_or("K5")?;
    let k6_at = (k5_at + 2..n.saturating_sub(8)).step_by(2).find(|&o| be16(d, o) == 38 && k4(d, o + 2).is_some()).ok_or("K6")?;
    let pitch_link = k4(d, k6_at + 2).ok_or("K6")?.0;
    let k7 = (k6_at + 2..n.saturating_sub(8)).step_by(2).find(|&o| is_k7(d, o)).ok_or("K7")?;
    if k7 < 4 {
        return Err("K7");
    }
    let subs_link = be32(d, k7 - 4);
    let pattern = off(pat_link).map_err(|_| "K4 target")?;
    let inst = off(inst_link).map_err(|_| "K5 target")?;
    let pitch = off(pitch_link).map_err(|_| "K6 target")?;
    let subtab = off(subs_link).map_err(|_| "K7 target")?;
    let mut subs = 0;
    while subtab + 18 * (subs + 1) <= n {
        let e = subtab + 18 * subs;
        let links = [be32(d, e), be32(d, e + 4), be32(d, e + 8), be32(d, e + 12)];
        if d[e + 16] != 12 || links.iter().all(|&l| l == links[0]) {
            break;
        }
        subs += 1;
    }
    Ok(Lay { origin, period, repeat, pattern, inst, pitch, subtab, subs })
}

// ------------------------------------------------------------------ the player

/// The waits between writes, in colour clocks.
const T_PRE: u64 = 290;
const T_VOICE: u64 = 480;
const T_NOTE_LOC: u64 = 116;
const T_LOC_LEN: u64 = 6;
/// Stage 2 starts this long after the tick's start with nothing to do in stage 1, plus
/// the costs per note start and per loop write (the means of the reference runs).
const T_BURST: u64 = 3290;
const T_BURST_NOTE: u64 = 420;
const T_BURST_LOOP: u64 = 40;
const T_PER_VOL: [u64; 4] = [112, 132, 150, 162];
const T_VOL_DMA: u64 = 4;
const T_NEXT: u64 = 34;

#[derive(Clone, Copy, Default)]
struct Voice {
    dur: u8,
    opos: u8,
    ppos: u8,
    note: u8,
    target: u8,
    state: u8,
    noteon: u8,
    vdir: u8,
    slide: u16,
    vcnt: i16,
    period: u16,
    tc: u16,
    vol: i16,
    inst: Option<usize>,
    order: usize,
    pseq: u8,
    wave: u32,
    sel: u8,
    transp: i8,
}

pub struct JeroenTel {
    d: Vec<u8>,
    lay: Lay,
    base: u32,
    silent: u32,
    sub: u16,
    started: bool,
    songend: bool,
    enabled: bool,
    t0: u64,
    notes: u64,
    loops: u64,
    bb: u8,
    bc: u8,
    v: [Voice; 4],
}

impl JeroenTel {
    fn b(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn w(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// A link address as a file offset.
    fn off(&self, link: u32) -> usize {
        link.wrapping_sub(self.lay.origin) as usize
    }

    /// A field of the voice's instrument record (zeros for none).
    fn iw(&self, v: usize, o: usize) -> u16 {
        self.v[v].inst.map_or(0, |i| self.w(i + o))
    }

    fn ib(&self, v: usize, o: usize) -> u8 {
        self.v[v].inst.map_or(0, |i| self.b(i + o))
    }

    fn il(&self, v: usize, o: usize) -> u32 {
        self.v[v].inst.map_or(0, |i| be32(&self.d, i + o))
    }

    /// The guest address of a link address.
    fn guest(&self, link: u32) -> u32 {
        self.base.wrapping_add(link.wrapping_sub(self.lay.origin))
    }

    fn period_of(&self, note: u8) -> u16 {
        let i = ((note & 0x7F) as usize * 2).min(70);
        be16(&self.d, self.lay.period + i)
    }

    fn start(&mut self, n: u16) {
        let e = self.lay.subtab + 18 * n as usize;
        self.v = [Voice::default(); 4];
        for i in 0..4 {
            self.v[i].state = 2;
            self.v[i].order = self.off(be32(&self.d, e + 4 * i));
        }
        self.bc = self.b(e + 17);
        self.bb = 0;
        self.enabled = true;
    }

    /// The stop event: the voices off, the sequencing halted, and the song's end reported (the
    /// original reports it here: UADE's player ends `mon.ingame` subsong 0 at its stop, 3.2 s).
    fn stop(&mut self, hw: &mut Hw) {
        self.enabled = false;
        self.songend = true;
        hw.dmacon(0x000F);
    }

    /// Stage 1.
    fn play1(&mut self, hw: &mut Hw) {
        if !self.enabled {
            return;
        }
        self.t0 = hw.bus.clock;
        self.notes = 0;
        self.loops = 0;
        hw.wait(T_PRE);
        self.bb = self.bb.wrapping_sub(1);
        if self.bb == 0xFE {
            self.bb = self.bc;
        }
        for v in (0..4).rev() {
            if !self.enabled {
                break;
            }
            if self.bb == 0xFF {
                if self.v[v].dur == 0xFF {
                    self.row(v, hw);
                } else {
                    self.effects(v, hw);
                }
            } else if self.bb == 0 {
                let d = self.v[v].dur.wrapping_sub(1);
                self.v[v].dur = d;
                if d & 0x80 != 0 {
                    self.v[v].noteon = 0;
                }
                self.effects(v, hw);
            } else {
                self.effects(v, hw);
            }
            hw.wait(T_VOICE);
        }
    }

    /// Stage 2.
    fn play2(&mut self, hw: &mut Hw) {
        if !self.enabled {
            return;
        }
        let at = self.t0 + T_BURST + T_BURST_NOTE * self.notes + T_BURST_LOOP * self.loops;
        if hw.bus.clock < at {
            hw.wait(at - hw.bus.clock);
        }
        for v in (0..4).rev() {
            hw.per(v, self.v[v].period);
            hw.wait(T_PER_VOL[v]);
            let vol = if self.v[v].vol > 64 { 64 } else { self.v[v].vol as u16 };
            hw.vol(v, vol);
            hw.wait(T_VOL_DMA);
            hw.dmacon(0x8000 | 1 << v);
            hw.wait(T_NEXT);
        }
    }

    fn effects(&mut self, v: usize, hw: &mut Hw) {
        // wave cycling (not in the corpus)
        let wl = self.iw(v, 32);
        if wl != 0 && self.v[0].tc & 1 != 0 {
            let a = self.guest(self.il(v, 4)).wrapping_add(self.v[v].wave);
            hw.loc(v, a);
            self.v[v].wave += 1;
            if self.v[v].wave >= 32 {
                self.v[v].wave = 0;
            }
        }
        let rel = self.iw(v, 30);
        if rel != 0 && (rel as i16) <= (self.v[v].tc as i16) {
            self.v[v].state = 2;
            self.v[v].noteon = 0;
        }
        self.v[v].tc = self.v[v].tc.wrapping_add(1);
        if self.v[v].tc == 1 {
            let ls = self.il(v, 8);
            let a = if ls == 0 { self.silent } else { self.guest(ls) };
            hw.loc(v, a);
            hw.wait(T_LOC_LEN);
            hw.len(v, self.iw(v, 12));
            self.loops += 1;
        }
        if self.v[v].slide != 0 {
            self.slide(v);
        } else {
            let delay = self.iw(v, 22);
            if self.v[v].note & 0x80 != 0 || (delay != 0 && self.v[v].tc > delay) {
                self.vibrato(v);
            }
        }
        self.tail(v, true);
    }

    fn slide(&mut self, v: usize) {
        let t = self.period_of(self.v[v].target) as i32;
        let s = self.v[v].slide as i32;
        let mut p = self.v[v].period as i32;
        if (self.v[v].target as i8) >= (self.v[v].note as i8) {
            p -= s;
            if p < t {
                p = t;
                self.v[v].slide = 0;
            }
        } else {
            p += s;
            if p > t {
                p = t;
                self.v[v].slide = 0;
            }
        }
        self.v[v].period = p as u16;
    }

    fn vibrato(&mut self, v: usize) {
        let h = self.iw(v, 24);
        let s = self.iw(v, 26);
        let x = &mut self.v[v];
        x.period = if x.vdir == 0 { x.period.wrapping_sub(s) } else { x.period.wrapping_add(s) };
        x.vcnt = x.vcnt.wrapping_sub(1);
        if x.vcnt < 0 {
            x.vdir ^= 1;
            x.vcnt = h as i16;
        }
    }

    fn attack(&mut self, v: usize) {
        let peak = self.iw(v, 0);
        let step = self.iw(v, 14);
        let x = &mut self.v[v];
        x.vol = x.vol.wrapping_add(step as i16);
        if (x.vol as u16) > peak {
            x.vol = peak as i16;
            x.state = 1;
        }
    }

    /// The envelope and the pitch sequence; `env` is false right after a note start,
    /// where the attack is applied directly.
    fn tail(&mut self, v: usize, env: bool) {
        if env {
            if self.v[v].noteon == 0 {
                let r = self.iw(v, 20) as i16;
                let x = &mut self.v[v];
                x.vol = x.vol.wrapping_sub(r);
                if x.vol < 0 {
                    x.vol = 0;
                }
            } else if self.v[v].tc & 1 != 0 {
            } else if self.v[v].state == 0 {
                self.attack(v);
            } else if self.v[v].state == 2 {
            } else {
                let dec = self.iw(v, 16) as i16;
                let sus = self.iw(v, 18) as i16;
                let x = &mut self.v[v];
                x.vol = x.vol.wrapping_sub(dec);
                if x.vol < 0 || x.vol < sus {
                    x.vol = sus;
                    x.state = 2;
                }
            }
        } else {
            self.attack(v);
        }
        self.pitch(v);
    }

    fn pitch(&mut self, v: usize) {
        let sel = self.v[v].sel as usize;
        let seq = self.off(be32(&self.d, self.lay.pitch + sel));
        let mut p = self.v[v].pseq as usize;
        for _ in 0..8 {
            let x = self.b(seq + p);
            if x == 0xFF {
                p = self.b(seq) as usize;
                continue;
            }
            if x == 0xFE {
                break;
            }
            self.v[v].pseq = (p + 1) as u8;
            let note = if x < 0x80 { x.wrapping_add(self.v[v].note) & 0x7F } else { x & 0x7F };
            self.v[v].period = self.period_of(note);
            return;
        }
        self.v[v].pseq = p as u8;
    }

    /// The row step: the order list, the cell, the note start.
    fn row(&mut self, v: usize, hw: &mut Hw) {
        let mut pat = 0u8;
        let mut found = false;
        for _ in 0..64 {
            let b = self.b(self.v[v].order + self.v[v].opos as usize);
            match b {
                0..=0x7F => {
                    pat = b;
                    found = true;
                    break;
                }
                0xFE => {
                    self.stop(hw);
                    return;
                }
                0xFF => {
                    self.songend = true;
                    self.v[v].opos = self.lay.repeat[v];
                    let b2 = self.b(self.v[v].order + self.v[v].opos as usize);
                    if b2 >= 0x80 {
                        self.v[v].transp = b2.wrapping_sub(0xC0) as i8;
                        self.v[v].opos = self.v[v].opos.wrapping_add(1);
                    }
                }
                _ => {
                    self.v[v].transp = b.wrapping_sub(0xC0) as i8;
                    self.v[v].opos = self.v[v].opos.wrapping_add(1);
                }
            }
        }
        if !found {
            self.stop(hw);
            return;
        }
        let pd = self.off(be32(&self.d, self.lay.pattern + 4 * pat as usize));
        let at = |s: &Self, n: usize| s.b(pd + n);
        let pos = self.v[v].ppos as usize;
        let b0 = at(self, pos);
        let mut retrig = b0 & 0x20 != 0;
        let mut c = pos;
        let mut cls = b0;
        let mut dur = b0 & 0x1F;
        if b0 >= 0xE0 {
            c = pos + 1;
            cls = at(self, c);
            self.v[v].sel = cls;
            retrig = false;
            dur = cls & 0x1F;
        }
        let tr = self.v[v].transp as u8;
        let note_at;
        let mut has_note = true;
        match cls {
            0xC0..=0xDF => {
                note_at = c;
                has_note = false;
            }
            0x80..=0xBF => {
                let ix = at(self, c + 1) as usize;
                self.v[v].inst = Some(self.off(be32(&self.d, self.lay.inst + 4 * ix)));
                self.v[v].sel = self.ib(v, 28);
                let x = at(self, c + 2);
                if x < 0x80 {
                    self.v[v].note = x.wrapping_add(tr);
                    note_at = c + 2;
                } else {
                    self.v[v].sel = x & 0x7F;
                    retrig = false;
                    self.v[v].note = at(self, c + 3).wrapping_add(tr);
                    note_at = c + 3;
                }
                self.v[v].slide = 0;
            }
            0x40..=0x7F => {
                self.v[v].slide = at(self, c + 1) as u16;
                self.v[v].target = at(self, c + 2).wrapping_add(tr);
                self.v[v].note = at(self, c + 3).wrapping_add(tr);
                note_at = c + 3;
            }
            _ => {
                self.v[v].note = at(self, c + 1).wrapping_add(tr);
                note_at = c + 1;
                self.v[v].slide = 0;
            }
        }
        // the next position
        if at(self, note_at + 1) == 0xFF {
            self.v[v].ppos = 0;
            self.v[v].opos = self.v[v].opos.wrapping_add(1);
        } else {
            self.v[v].ppos = (note_at + 1) as u8;
        }
        self.v[v].dur = dur;
        if !has_note {
            self.effects(v, hw);
            return;
        }
        if retrig {
            self.v[v].state = 2;
            self.v[v].vol = self.iw(v, 18) as i16;
            self.tail(v, true);
            return;
        }
        hw.dmacon(1 << v);
        hw.wait(T_NOTE_LOC);
        self.notes += 1;
        let half = self.iw(v, 24) >> 1;
        {
            let x = &mut self.v[v];
            x.vol = 0;
            x.state = 0;
            x.tc = 0;
            x.wave = 0;
            x.pseq = 1;
            x.vcnt = half as i16;
            x.vdir = 1;
        }
        hw.loc(v, self.guest(self.il(v, 4)));
        hw.wait(T_LOC_LEN);
        hw.len(v, self.iw(v, 2));
        self.v[v].noteon = 1;
        self.tail(v, false);
    }
}

impl Replayer for JeroenTel {
    fn format(&self) -> &'static str {
        "Jeroen Tel"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.subs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (n as usize) < self.lay.subs {
            self.sub = n;
            self.started = false;
            self.songend = false;
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.start(self.sub);
        }
        self.play1(hw);
        self.play2(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !signature(d) {
        return None;
    }
    let lay = locate(d).ok().filter(|l| l.subs > 0)?;
    let silent = m.alloc(34);
    if silent == 0 {
        return None;
    }
    Some(Box::new(JeroenTel {
        d: d.to_vec(),
        lay,
        base,
        silent,
        sub: 0,
        started: false,
        songend: false,
        enabled: false,
        t0: 0,
        notes: 0,
        loops: 0,
        bb: 0,
        bc: 0,
        v: [Voice::default(); 4],
    }))
}

#[cfg(test)]
#[path = "jeroentel_tests.rs"]
mod tests;
