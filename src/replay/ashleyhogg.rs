//! Ashley Hogg (`ash.*`), the music driver of the Ashley Hogg tunes (CJ in the USA, Nobby
//! the Aardvark, Spike in Transylvania, Cosmic Spacehead, Fantastic Dizzy): two families of
//! drivers, five builds.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no original
//! code. The specification was drawn up from the drivers' observable behaviour (the Paula
//! register writes of UADE's Ashley Hogg player) and this player was written from it and
//! checked against those writes.
//!
//! A module is a raw memory image, the driver code first. The code is never run: it is read
//! only to recognise the family and to find the tables, by the instruction forms the
//! specification describes, matched on decoded instructions (`m68kdec`).
//!
//! * The 1991 family (`ash.CJ_in_the_USA`, `ash.Nobby_The_Aardvark`,
//!   `ash.Spike_In_Transilvania`) is a single file: four voices with a 74-byte state
//!   record, instruments copied over the record, envelopes, arpeggios, a sequence list per
//!   voice that points at patterns of rows.
//! * The 1993 family (`ash.Cosmic_Spacehead`, `ash.Fantastic_Dizzy`) needs its sample file
//!   `smp.NAME`: pattern streams with note, effect and back-reference bytes, a start
//!   sequence (DMA off, locations, DMA on, loop locations) spaced out over three stages.
//!
//! Decisions where the specification leaves the choice to the port:
//! * Subsongs are numbered from 0 (subsong 0 is the specification's first).
//! * The host's start-up writes (DMA off, and the song set-up's) are made by the first tick.
//! * Song end is a flag that stays set; the player carries on, as the `-n` logs do.
//! * The 1991 attack phase follows the driver's own order (the delay counter is left one
//!   short after a step that does not end the attack), not the specification's summary.
//! * The synthetic-wave path (not exercised by the logs) points Paula at the module's own
//!   bytes in guest memory.
//! * Loops that follow data (sequence loops, restarts of a row) are bounded, so a corrupt
//!   module cannot hang the player.
//! * Reads outside the file give zero.
//! * Timing inside a tick follows the specification's simple model (section 6): a fixed
//!   start offset and a fixed step between writes, the 1993 stages spaced by nine raster
//!   lines.

use super::m68kdec::{decode, Ea, Insn, K};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Ashley Hogg", detect, new };

/// The custom chips' base address, which the 1993 drivers load into an address register.
const CUSTOM: u32 = 14675968;
/// Colour clocks between two ticks (the host's CIA timer).
const TICK: u64 = 70940;
/// Colour clocks per raster line.
const LINE: u64 = crate::paula::LINE;

fn dec(d: &[u8], at: usize) -> Option<Insn> {
    decode(d, at)
}

fn byte(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

// ------------------------------------------------------------------ recognition

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fam {
    Y91,
    Y93,
}

/// A branch always with a 16-bit displacement that is positive and even: its target.
fn bra_w(d: &[u8], at: usize) -> Option<usize> {
    let i = dec(d, at)?;
    if i.k != K::Bra || i.size != 2 {
        return None;
    }
    let disp = i.target as i64 - (at as i64 + 2);
    (disp > 0 && disp < 32768 && disp % 2 == 0).then_some(i.target)
}

/// The family, from the header: four branches, then either two more and a driver entry that
/// saves the registers and calls the routine that loads the chips' address (1993), or a
/// word move of 0 into a data register, a short branch-if-not-equal over 34 bytes and a
/// word move of that register to an absolute address (1991).
fn family(d: &[u8]) -> Option<Fam> {
    if d.len() < 64 || !(0..4).all(|i| bra_w(d, 4 * i).is_some()) {
        return None;
    }
    if bra_w(d, 16).is_some() && bra_w(d, 20).is_some() {
        let t = bra_w(d, 20)?;
        let m = dec(d, t)?;
        // all data and address registers but the stack pointer
        if m.k != K::Movem || m.dst != Ea::Pre(7) || be16(d, t + 2) != !1u16 {
            return None;
        }
        let b = dec(d, m.next())?;
        if b.k != K::Bsr || b.size != 2 {
            return None;
        }
        let l = dec(d, b.target)?;
        let ok = l.k == K::Lea && matches!(l.src, Ea::AbsL { addr: CUSTOM, .. }) && matches!(l.dst, Ea::A(_));
        return ok.then_some(Fam::Y93);
    }
    let a = dec(d, 16)?;
    let Ea::D(r) = a.dst else { return None };
    if !(a.k == K::Move && a.size == 2 && matches!(a.src, Ea::Imm { val: 0, .. })) {
        return None;
    }
    let b = dec(d, a.next())?;
    if !(b.k == K::Bcc && b.cond == 6 && b.size == 1 && b.target as i64 - (b.at as i64 + 2) == 34) {
        return None;
    }
    let c = dec(d, b.next())?;
    (c.k == K::Move && c.size == 2 && c.src == Ea::D(r) && matches!(c.dst, Ea::AbsL { .. })).then_some(Fam::Y91)
}

fn detect(d: &[u8]) -> bool {
    match family(d) {
        Some(Fam::Y91) => locate91(d).is_some_and(|l| l.nsongs > 0),
        Some(Fam::Y93) => locate93(d).is_some_and(|l| l.nsongs > 0),
        None => false,
    }
}

// ------------------------------------------------------------------ the 1991 locator

/// Where the 1991 tables are (file offsets), and what the build does differently.
#[derive(Clone, Debug, PartialEq)]
struct Lay91 {
    per: usize,
    empty: usize,
    syn: usize,
    ins: usize,
    arpo: usize,
    arpd: usize,
    pato: usize,
    patb: usize,
    songs: usize,
    spd: usize,
    seq: usize,
    smp: usize,
    nsongs: usize,
    /// Sequence command 130 is a fade.
    fade: bool,
    /// Sequence command 129 loops to the sequence start plus the byte after it.
    loop_off: bool,
    /// Only the fourth voice runs the music path.
    only3: bool,
}

/// Instruction offsets of the code, as far as the scans go.
fn offsets(d: &[u8], from: usize) -> impl Iterator<Item = usize> + '_ {
    (from..d.len().saturating_sub(2)).step_by(2)
}

/// `lea d16(pc),an`: the target.
fn lea_pc(d: &[u8], at: usize) -> Option<usize> {
    let i = dec(d, at)?;
    match (i.k, i.src, i.dst) {
        (K::Lea, Ea::Pc(t), Ea::A(_)) => Some(t),
        _ => None,
    }
}

/// The first `lea d16(pc),an` at or after `from`, within 64 bytes: (offset of the next
/// instruction, target).
fn next_lea_pc(d: &[u8], from: usize) -> Option<(usize, usize)> {
    (from..from + 64).step_by(2).find_map(|o| lea_pc(d, o).map(|t| (o + 4, t)))
}

fn locate91(d: &[u8]) -> Option<Lay91> {
    // SPD: the address load before the indexed byte move into the speed field (offset 47)
    // of the first voice; SONGS and SEQ are the next two address loads
    let (spd, after) = offsets(d, 20).find_map(|o| {
        let t = lea_pc(d, o)?;
        let Ea::A(r) = dec(d, o)?.dst else { return None };
        let m = dec(d, o + 4)?;
        let ok = m.k == K::Move && m.size == 1 && matches!(m.src, Ea::Idx(a, ..) if a == r) && matches!(m.dst, Ea::Disp(_, 47));
        ok.then_some((t, m.next()))
    })?;
    let (n1, songs) = next_lea_pc(d, after)?;
    let (_, seq) = next_lea_pc(d, n1)?;
    // INS: the address load right after the multiplication by 44
    let ins = offsets(d, 20).find_map(|o| {
        let m = dec(d, o)?;
        if m.k == K::Mulu && matches!(m.src, Ea::Imm { val: 44, .. }) && matches!(m.dst, Ea::D(_)) {
            lea_pc(d, m.next())
        } else {
            None
        }
    })?;
    // SMP: the address load followed by the add of the long at offset 32 of the voice
    let smp = offsets(d, 20).find_map(|o| {
        let t = lea_pc(d, o)?;
        let a = dec(d, o + 4)?;
        (a.k == K::Adda && matches!(a.src, Ea::Disp(_, 32))).then_some(t)
    })?;
    // PER: the address load followed by the shift left of the note index by 1
    let per = offsets(d, 20).find_map(|o| {
        let t = lea_pc(d, o)?;
        let s = dec(d, o + 4)?;
        (s.k == K::Shift && s.size == 2 && s.left && s.data == 1).then_some(t)
    })?;
    // ARPD, ARPO: two consecutive address loads, then the table number (offset 6) shifted
    // left by 2
    let (arpd, arpo) = offsets(d, 20).find_map(|o| {
        let a = lea_pc(d, o)?;
        let b = lea_pc(d, o + 4)?;
        let m = dec(d, o + 8)?;
        let s = dec(d, m.next())?;
        let ok = m.k == K::Move && matches!(m.src, Ea::Disp(_, 6)) && s.k == K::Shift && s.size == 2 && s.left && s.data == 2;
        ok.then_some((a, b))
    })?;
    // PATO: the address load before the indexed long move into the pattern pointer (offset
    // 54); PATB the next address load
    let (pato, patb) = offsets(d, 20).find_map(|o| {
        let t = lea_pc(d, o)?;
        let Ea::A(r) = dec(d, o)?.dst else { return None };
        let m = dec(d, o + 4)?;
        let ok = m.k == K::Move && m.size == 4 && matches!(m.src, Ea::Idx(a, ..) if a == r) && matches!(m.dst, Ea::Disp(_, 54));
        if !ok {
            return None;
        }
        next_lea_pc(d, m.next()).map(|(_, b)| (t, b))
    })?;
    // EMPTY: the long cell loaded by the loop step, plus 4
    let empty = offsets(d, 20).find_map(|o| {
        let m = dec(d, o)?;
        match (m.k, m.size, m.src, m.dst) {
            (K::Move, 4, Ea::Pc(t), Ea::D(_)) => Some(t + 4),
            _ => None,
        }
    })?;
    // SYN: the address load after the shift left by 7
    let syn = offsets(d, 20).find_map(|o| {
        let s = dec(d, o)?;
        if s.k == K::Shift && s.size == 2 && s.left && s.data == 7 {
            lea_pc(d, s.next())
        } else {
            None
        }
    })?;
    if spd <= songs || (spd - songs) % 16 != 0 || [per, empty, syn, ins, arpo, arpd, pato, patb, songs, spd, seq, smp].iter().any(|&t| t >= d.len()) {
        return None;
    }
    // the build: what the code does that the others' do not
    let code_end = per.min(ins).min(syn).min(empty);
    let mut fade = false;
    let mut only3 = false;
    let mut loop_off = false;
    for o in offsets(d, 20).take_while(|&o| o < code_end) {
        let Some(i) = dec(d, o) else { continue };
        let imm = |v: u32| matches!(i.src, Ea::Imm { val, .. } if val == v);
        if i.k == K::Imm(6) && i.size == 1 && matches!(i.dst, Ea::Ind(_)) {
            if imm(130) {
                fade = true;
            }
            if imm(129) && !loop_off {
                // the loop target: the sequence start (a long move) or plus a byte
                let b = dec(d, i.next());
                loop_off = b.is_some_and(|b| {
                    b.k == K::Bcc && b.cond == 7 && dec(d, b.target).is_some_and(|t| t.k == K::Move && t.size == 4 && matches!(t.src, Ea::Disp(_, 58)) && matches!(t.dst, Ea::D(_)))
                });
            }
        }
        if i.k == K::Cmp && i.size == 1 && i.dst == Ea::D(7) && imm(8) {
            only3 = true;
        }
    }
    Some(Lay91 { per, empty, syn, ins, arpo, arpd, pato, patb, songs, spd, seq, smp, nsongs: (spd - songs) / 16, fade, loop_off, only3 })
}

// ------------------------------------------------------------------ the 1991 player

/// Colour clocks into a tick of the first write, and between two writes.
const T91_START: u64 = 296;
const T91_LOC_LEN: u64 = 6;
const T91_STEP: u64 = 150;

#[derive(Clone, Copy)]
struct V91 {
    /// Bytes 0..44: the instrument record.
    r: [u8; 44],
    /// Pitch offset.
    p: u16,
    rtc: u8,
    speed: u8,
    wait: u8,
    rep: u8,
    sp: usize,
    pp: usize,
    ss: usize,
    ps: usize,
    note: u16,
    at: u16,
    st: u16,
    e: u8,
}

impl Default for V91 {
    fn default() -> Self {
        V91 { r: [0; 44], p: 0, rtc: 0, speed: 0, wait: 0, rep: 0, sp: 0, pp: 0, ss: 0, ps: 0, note: 0, at: 0, st: 0, e: 0 }
    }
}

impl V91 {
    fn w(&self, o: usize) -> u16 {
        u16::from_be_bytes([self.r[o], self.r[o + 1]])
    }
    fn set_w(&mut self, o: usize, v: u16) {
        self.r[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn l(&self, o: usize) -> u32 {
        u32::from_be_bytes([self.r[o], self.r[o + 1], self.r[o + 2], self.r[o + 3]])
    }
    fn release(&mut self) {
        self.r[0] = 0;
        self.r[1] = 0;
        self.r[2] = 0;
        self.e = 0;
    }
}

pub struct Ash91 {
    d: Vec<u8>,
    lay: Lay91,
    base: u32,
    sub: u16,
    started: bool,
    songend: bool,
    /// Master volume, fade counter and reload.
    m: u16,
    f: u16,
    fr: u16,
    first: bool,
    adv: u8,
    looped: [bool; 4],
    v: [V91; 4],
}

impl Ash91 {
    fn new(d: &[u8], base: u32, lay: Lay91) -> Ash91 {
        Ash91 { d: d.to_vec(), lay, base, sub: 0, started: false, songend: false, m: 0, f: 0, fr: 0, first: false, adv: 0, looped: [false; 4], v: [V91::default(); 4] }
    }

    fn b(&self, o: usize) -> u8 {
        byte(&self.d, o)
    }

    fn guest(&self, o: usize) -> u32 {
        self.base.wrapping_add(o as u32)
    }

    /// The song's start: master volume, voices, sequence pointers.
    fn init(&mut self) {
        let s = self.sub as usize;
        self.m = 63;
        self.f = 0;
        self.fr = 0;
        self.first = true;
        self.adv = 0;
        self.looped = [false; 4];
        self.songend = false;
        let speed = self.b(self.lay.spd + s);
        for c in 0..4 {
            let off = be32(&self.d, self.lay.songs + 16 * s + 4 * c) as usize;
            let start = self.lay.seq.wrapping_add(off);
            let mut v = V91::default();
            v.r[42] = 255;
            v.r[43] = 255;
            v.speed = speed;
            v.rtc = 1;
            v.sp = start;
            v.ss = start;
            self.v[c] = v;
        }
    }

    fn inactive(&self, c: usize) -> bool {
        self.v[c].sp == self.lay.seq || (self.lay.only3 && c != 3)
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            hw.dmacon(15);
            self.init();
        }
        if self.m == 0 {
            for c in 0..4 {
                hw.vol(c, 0);
            }
            hw.dmacon(15);
            self.songend = true;
            return;
        }
        if self.f != 0 {
            self.f -= 1;
            if self.f == 0 {
                self.f = self.fr;
                self.m = self.m.wrapping_sub(1);
            }
        }
        if self.first {
            self.first = false;
            for c in 0..4 {
                self.seq_step(c);
            }
            return;
        }
        self.adv = 0;
        hw.wait(T91_START);
        for c in 0..4 {
            if self.inactive(c) {
                continue;
            }
            let v = &mut self.v[c];
            v.rtc = v.rtc.wrapping_sub(1);
            let mut row = false;
            if v.rtc == 0 {
                v.rtc = v.speed;
                if v.wait != 0 {
                    v.wait -= 1;
                    row = v.wait == 0;
                } else {
                    row = true;
                }
            }
            if row {
                self.row(c, hw);
            } else {
                self.effects(c, hw);
            }
        }
    }

    /// A row at the pattern pointer (spec 4.5).
    fn row(&mut self, c: usize, hw: &mut Hw) {
        let mut q = self.v[c].pp;
        for _ in 0..16 {
            let b = self.b(q);
            match b {
                128..=255 => {
                    self.m = 0;
                    self.v[c].pp = q;
                    return;
                }
                98 => {
                    self.v[c].release();
                    q += 1;
                }
                100 => {
                    let x = self.b(q + 1);
                    self.v[c].speed = x;
                    self.v[c].rtc = x;
                    q += 2;
                }
                99 => {
                    self.v[c].wait = self.b(q + 1);
                    q += 2;
                    break;
                }
                n => {
                    self.v[c].note = n as u16;
                    let i = self.b(q + 1);
                    q += 2;
                    if i != 63 {
                        let at = self.lay.ins + 44 * i as usize;
                        for k in 0..44 {
                            self.v[c].r[k] = self.b(at + k);
                        }
                        let vc = self.v[c].r[22];
                        self.v[c].r[22] = vc >> 1;
                        self.v[c].e = 0;
                        hw.dmacon(1 << c);
                        let off = self.v[c].l(32) as i32;
                        if off >= 0 {
                            let a = self.guest(self.lay.smp.wrapping_add(off as usize));
                            hw.loc(c, a);
                            hw.wait(T91_LOC_LEN);
                            let len = self.v[c].w(40);
                            hw.len(c, len);
                            hw.wait(T91_STEP);
                        }
                    }
                    self.v[c].r[20] = 0;
                    self.v[c].r[21] = 0;
                    self.v[c].p = 0;
                    break;
                }
            }
        }
        // the row tail
        if self.b(q) == 101 {
            self.v[c].r[2] = self.b(q + 1);
            q += 2;
        }
        if self.b(q) == 96 {
            let s = (self.b(q + 1) as u16) << 8 | self.b(q + 2) as u16;
            self.v[c].set_w(20, s.wrapping_neg());
            q += 3;
        }
        self.v[c].pp = q;
        self.v[c].at = self.v[c].st;
    }

    /// The envelope of the level (spec 4.7).
    fn envelope(&mut self, c: usize) {
        let v = &mut self.v[c];
        if v.r[0] != 0 {
            if v.e != 0 {
                v.e -= 1;
            } else {
                v.e = v.r[0];
                v.r[5] = v.r[5].wrapping_add(1);
                if v.r[5] == v.r[2] {
                    v.r[0] = 0;
                    v.e = 0;
                } else {
                    v.e = v.e.wrapping_sub(1);
                }
            }
            return;
        }
        if v.r[5] == 0 {
            if v.r[2] == 0 {
                return;
            }
            v.r[5] = v.r[2];
        }
        if v.r[2] == 0 && v.r[5] != 0 {
            if v.e != 0 {
                v.e -= 1;
            } else {
                v.e = v.r[3];
                if v.r[3] == 0 {
                    v.r[5] = 0;
                } else {
                    v.r[5] -= 1;
                    if v.r[5] != 0 {
                        v.e = v.e.wrapping_sub(1);
                    }
                }
            }
        }
    }

    /// A tick of a voice that reads no row (spec 4.6).
    fn effects(&mut self, c: usize, hw: &mut Hw) {
        let bit = 1u16 << c;
        // 1. note duration
        if self.v[c].w(10) != 0 {
            let n = self.v[c].w(10) - 1;
            self.v[c].set_w(10, n);
            if n == 0 {
                self.v[c].release();
            }
        }
        // 2. envelope
        self.envelope(c);
        // 3. sample
        let off = self.v[c].l(32) as i32;
        if off == 0 {
            let lo = self.v[c].l(36) as usize;
            let (a, len) = if lo == 0 { (self.guest(self.lay.empty), 1) } else { (self.guest(self.lay.smp.wrapping_add(lo)), self.v[c].w(40)) };
            hw.loc(c, a);
            hw.wait(T91_LOC_LEN);
            hw.len(c, len);
            hw.wait(T91_STEP);
        } else if off > 0 {
            hw.dmacon(0x8000 | bit);
            hw.wait(T91_STEP);
            self.v[c].r[32..36].fill(0);
        }
        // 4. volume
        let vol = ((self.m as u32 * self.v[c].w(4) as u32) & 0xFFFF) >> 6;
        hw.vol(c, (vol & 127) as u16);
        hw.wait(T91_STEP);
        // 5. slide
        let v = &mut self.v[c];
        v.p = v.p.wrapping_add(v.w(20));
        // 6. vibrato
        if v.w(26) != 0 {
            let n = v.w(26) - 1;
            v.set_w(26, n);
        } else if v.w(24) != 0 {
            v.p = v.p.wrapping_add(v.w(24));
            v.r[22] = v.r[22].wrapping_sub(1);
            if v.r[22] == 0 {
                v.r[22] = v.r[23];
                let nd = v.w(24).wrapping_neg();
                v.set_w(24, nd);
                if (nd as i16) >= 0 && (v.w(30) as i16) > nd as i16 {
                    v.set_w(24, nd.wrapping_add(v.w(28)));
                }
            }
        }
        // 7. arpeggio and period
        let tn = self.v[c].w(6) as i16 as isize;
        let t = self.lay.arpd.wrapping_add(be32(&self.d, self.lay.arpo.wrapping_add((tn * 4) as usize)) as usize);
        let pos = self.v[c].w(8) as i16 as isize;
        let at = t.wrapping_add(pos as usize);
        let k = self.b(at) as i8 as i16 as u16;
        let v = &self.v[c];
        let mut n = k.wrapping_add(v.note);
        if (v.w(42) as i16) < 0 {
            n = n.wrapping_add(v.at);
        }
        let idx = (n << 1) as i16 as isize;
        let per = be16(&self.d, self.lay.per.wrapping_add(idx as usize)).wrapping_add(v.p);
        hw.per(c, per);
        hw.wait(T91_STEP);
        if self.b(at + 1) == 128 {
            self.v[c].r[9] = self.b(at + 2);
            self.v[c].r[8] = 0;
        } else {
            self.v[c].r[9] = self.v[c].r[9].wrapping_add(1);
        }
        // 8. synthetic wave
        if (self.v[c].l(32) as i32) < 0 {
            let v = &mut self.v[c];
            let pos = v.w(12).wrapping_add(v.w(14));
            v.set_w(12, pos);
            if v.r[16] == v.r[12] || v.r[18] == v.r[12] {
                let s = v.w(14).wrapping_neg();
                v.set_w(14, s);
            }
            let a = self.guest(self.lay.syn + 128 * ((pos & 4095) >> 5) as usize);
            hw.loc(c, a);
            hw.wait(T91_LOC_LEN);
            hw.len(c, 64);
            hw.wait(T91_STEP);
            hw.dmacon(0x8000 | bit);
            hw.wait(T91_STEP);
        }
        // 9. the end of the pattern
        if self.adv == 0 && self.b(self.v[c].pp) >= 128 {
            self.seq_step(c);
        }
    }

    /// The next sequence entry (spec 4.9).
    fn seq_step(&mut self, c: usize) {
        if self.v[c].sp == self.lay.seq {
            return;
        }
        if self.v[c].rep != 0 {
            self.v[c].rep -= 1;
            if self.v[c].rep != 0 {
                self.v[c].pp = self.v[c].ps;
                return;
            }
        }
        for _ in 0..64 {
            let q = self.v[c].sp;
            let b = self.b(q);
            match b {
                129 => {
                    self.looped[c] = true;
                    let active = |s: &Self, i: usize| s.v[i].ss != s.lay.seq;
                    if (0..4).all(|i| !active(self, i) || self.looped[i]) {
                        self.songend = true;
                        self.looped = [false; 4];
                    }
                    let add = if self.lay.loop_off { self.b(q + 1) as usize } else { 0 };
                    self.v[c].sp = self.v[c].ss.wrapping_add(add);
                }
                130 if self.lay.fade => {
                    if self.f == 0 {
                        self.f = self.b(q + 1) as u16;
                        self.fr = self.f;
                    }
                    self.v[c].sp += 2;
                }
                128 => return,
                t => {
                    self.adv = self.adv.wrapping_add(1);
                    let p = self.b(q + 1);
                    self.v[c].st = t as i8 as i16 as u16;
                    self.v[c].sp += 2;
                    let rel = be32(&self.d, self.lay.pato + 4 * (p & 127) as usize) as usize;
                    let a = self.lay.patb.wrapping_add(rel);
                    self.v[c].pp = a;
                    self.v[c].ps = a;
                    if p >= 128 {
                        self.v[c].rep = self.b(self.v[c].sp);
                        self.v[c].sp += 1;
                    }
                    return;
                }
            }
        }
    }
}

impl Replayer for Ash91 {
    fn format(&self) -> &'static str {
        "Ashley Hogg"
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

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.nsongs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (n as usize) < self.lay.nsongs {
            self.sub = n;
            self.started = false;
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        Ash91::tick(self, hw);
    }
}

// ------------------------------------------------------------------ the 1993 locator

#[derive(Clone, Debug, PartialEq)]
struct Lay93 {
    dur: usize,
    trans: usize,
    imap: usize,
    pt: usize,
    songt: usize,
    nsongs: usize,
}

/// The first row of the period table.
const PT_ROW: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214,
    202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

fn locate93(d: &[u8]) -> Option<Lay93> {
    // PT: eight rows of 36 words, the song table directly after
    let pt = offsets(d, 0).find(|&o| PT_ROW.iter().enumerate().all(|(i, &p)| be16(d, o + 2 * i) == p))?;
    let songt = pt + 8 * 72;
    if songt + 4 > d.len() {
        return None;
    }
    // DUR: the indexed byte read into the voice's wait field
    let dur = offsets(d, 28).find_map(|o| {
        let i = dec(d, o)?;
        match (i.k, i.size, i.src, i.dst) {
            (K::Move, 1, Ea::PcIdx(t, _), Ea::Disp(..)) => Some(t),
            _ => None,
        }
    })?;
    // TRANS: the indexed word added to the note
    let trans = offsets(d, 28).find_map(|o| {
        let i = dec(d, o)?;
        match (i.k, i.size, i.src, i.dst) {
            (K::Add, 2, Ea::PcIdx(t, _), Ea::D(_)) => Some(t),
            _ => None,
        }
    })?;
    // IMAP: the address load after the song index shifted left by 4
    let imap = offsets(d, 28).find_map(|o| {
        let s = dec(d, o)?;
        if s.k == K::Shift && s.size == 2 && s.left && s.data == 4 {
            lea_pc(d, s.next())
        } else {
            None
        }
    })?;
    if [dur, trans, imap].iter().any(|&t| t >= d.len()) {
        return None;
    }
    Some(Lay93 { dur, trans, imap, pt, songt, nsongs: be16(d, songt) as usize / 4 })
}

// ------------------------------------------------------------------ the 1993 player

const T93_START: u64 = 514;
const T93_STEP: u64 = 150;
/// Raster lines the host waits between the stages.
const STAGE_LINES: u64 = 9;

#[derive(Clone, Copy, Default)]
struct C93 {
    sp: usize,
    rp: usize,
    br: bool,
    bc: u16,
    inn: u8,
    nt: u16,
    w: u8,
    k: bool,
    v: u8,
    ts: u16,
    start: u32,
    lstart: u32,
    len: u16,
    llen: u16,
    ft: i16,
    st: u8,
    ic: bool,
    et: u8,
    ed: u8,
    edl: i8,
    ep: u16,
    po: u16,
    ef: bool,
}

pub struct Ash93 {
    d: Vec<u8>,
    lay: Lay93,
    smp: Vec<u8>,
    /// Where the sample file is in guest memory.
    sbase: u32,
    sub: u16,
    started: bool,
    songend: bool,
    pos: u16,
    npos: u16,
    cnt: u16,
    speed: u16,
    mv: u32,
    c: [C93; 4],
}

impl Ash93 {
    fn new(d: &[u8], lay: Lay93, smp: &[u8], sbase: u32) -> Ash93 {
        Ash93 { d: d.to_vec(), lay, smp: smp.to_vec(), sbase, sub: 0, started: false, songend: false, pos: 0, npos: 0, cnt: 0, speed: 0, mv: 0, c: [C93::default(); 4] }
    }

    fn b(&self, o: usize) -> u8 {
        byte(&self.d, o)
    }

    /// The song set-up (spec 5.3).
    fn setup(&mut self) {
        self.c = [C93::default(); 4];
        self.cnt = 5;
        self.speed = 6;
        self.mv = 16384;
        self.pos = 0;
        self.songend = false;
        let a = be16(&self.d, self.lay.songt + 4 * self.sub as usize) as usize;
        self.npos = self.b(self.lay.songt + a) as u16;
        for c in &mut self.c {
            c.v = 64;
        }
        self.load_pos();
    }

    fn pat_no(&self) -> u8 {
        let a = be16(&self.d, self.lay.songt + 4 * self.sub as usize) as usize;
        self.b(self.lay.songt + a + 1 + self.pos as usize)
    }

    /// Spec 5.4.
    fn load_pos(&mut self) {
        let p = self.pat_no() as usize;
        let tab = self.lay.songt + be16(&self.d, self.lay.songt + 4 * self.sub as usize + 2) as usize;
        for c in 0..4 {
            let rel = be16(&self.d, tab + 8 * p + 2 * c) as i16 as isize;
            self.c[c].sp = tab.wrapping_add(rel as usize);
            self.c[c].br = false;
            self.c[c].bc = 0;
        }
    }

    /// Reads a byte of the stream; a back-reference counts its bytes.
    fn rd(&mut self, c: usize) -> u8 {
        let ch = &mut self.c[c];
        let b = byte(&self.d, ch.sp);
        ch.sp += 1;
        if ch.br {
            ch.bc = ch.bc.wrapping_sub(1);
            if ch.bc == 0 {
                ch.sp = ch.rp;
                ch.br = false;
            }
        }
        b
    }

    fn load_inst(&mut self, c: usize) {
        let ch = self.c[c];
        if ch.nt == 0 || ch.inn == 0 {
            return;
        }
        let s = self.b(self.lay.imap + 16 * self.sub as usize + ch.inn as usize) as usize + 1;
        let r = 16 * (s - 1);
        let (st, ls, len, llen, ft) = (be32(&self.smp, r), be32(&self.smp, r + 4), be16(&self.smp, r + 8), be16(&self.smp, r + 10), be16(&self.smp, r + 14) as i16);
        let ch = &mut self.c[c];
        ch.ts = s as u16;
        ch.start = st;
        ch.lstart = ls;
        ch.len = len;
        ch.llen = llen;
        ch.ft = ft;
    }

    /// A row of one voice (spec 5.6).
    fn row(&mut self, c: usize) {
        {
            let ch = &mut self.c[c];
            if ch.ed != 0 {
                ch.ep = ch.ep.wrapping_add(ch.edl as i16 as u16);
                ch.ed -= 1;
                if ch.ed == 0 {
                    ch.et = 0;
                    ch.edl = 0;
                    ch.ef = false;
                }
            }
            if ch.w != 0 {
                ch.w -= 1;
                return;
            }
        }
        'start: for _ in 0..64 {
            if self.c[c].sp == 0 {
                return;
            }
            self.c[c].ef = false;
            loop {
                let x = self.rd(c);
                match x {
                    255 => {
                        self.pos = self.pos.wrapping_add(1);
                        if self.pos == self.npos {
                            self.pos = 0;
                            self.songend = true;
                        }
                        self.load_pos();
                        continue 'start;
                    }
                    254 => {
                        let y = self.rd(c);
                        self.pos = (self.pos & !255) | y as u16;
                        self.load_pos();
                        continue 'start;
                    }
                    253 => self.speed = self.rd(c) as u16,
                    241..=252 => {
                        self.c[c].inn = x - 240;
                        self.c[c].ic = true;
                    }
                    217..=240 => {
                        let e = x - 217;
                        let y = self.rd(c);
                        let ch = &mut self.c[c];
                        ch.ed = (ch.ed & 12) | (e & 3);
                        ch.et = e / 4 + 1;
                        ch.edl = (y / 64) as i8 - 1;
                        ch.ep = (y & 63) as u16;
                        ch.ef = true;
                        if ch.et == 6 {
                            ch.k = true;
                        }
                    }
                    184..=216 => {
                        let ch = &mut self.c[c];
                        ch.et = 6;
                        ch.ep = ((x - 184) & 63) as u16;
                        ch.ed = 0;
                        ch.k = true;
                    }
                    148..=183 => {
                        let ch = &mut self.c[c];
                        let dist = byte(&self.d, ch.sp) as usize;
                        ch.sp += 1;
                        ch.bc = x as u16 - 142;
                        ch.rp = ch.sp;
                        ch.sp = ch.sp.wrapping_sub(dist + 8);
                        ch.br = true;
                        continue 'start;
                    }
                    _ => {
                        let k = x & 3;
                        let n = ((x / 4) & 63) as u16;
                        if k != 3 {
                            let ch = &mut self.c[c];
                            if ch.ef {
                                ch.ed = 1;
                                ch.edl = 0;
                            }
                            ch.w = byte(&self.d, self.lay.dur + k as usize);
                            if n != 0 {
                                ch.nt = n;
                                self.load_inst(c);
                            }
                        } else {
                            if n != 0 {
                                self.c[c].nt = n;
                            }
                            let y = self.rd(c);
                            let ch = &mut self.c[c];
                            ch.w = (y & 63).wrapping_sub(1);
                            if ch.ef {
                                ch.ed = ((((y / 16) & 12) | (ch.ed & 3)) as u16 + 1) as u8;
                            }
                            if n != 0 {
                                self.load_inst(c);
                            }
                        }
                        return;
                    }
                }
            }
        }
    }

    /// Waits until `n` raster-line boundaries have passed.
    fn lines(hw: &mut Hw) {
        hw.bus.clock = (hw.bus.clock / LINE + STAGE_LINES) * LINE;
    }

    /// The trigger (spec 5.8).
    fn trigger(&mut self, hw: &mut Hw) {
        let mut mask = 0u16;
        for c in 0..4 {
            let ch = &mut self.c[c];
            if ch.ts == 0 {
                continue;
            }
            mask |= 1 << c;
            hw.dmacon(1 << c);
            hw.wait(T93_STEP);
            let dv = byte(&self.smp, 16 * (ch.ts as usize - 1) + 13);
            if ch.ic {
                ch.ic = false;
                if !ch.k {
                    if ch.et == 6 {
                        ch.et = 0;
                    }
                    ch.v = dv;
                }
            } else if ch.et != 6 {
                ch.v = dv;
            }
            ch.k = false;
            ch.ts = 0;
            ch.st = 1;
            ch.ef = false;
        }
        if mask == 0 {
            return;
        }
        Self::lines(hw);
        for c in 0..4 {
            if self.c[c].st != 0 {
                hw.loc(c, self.sbase.wrapping_add(self.c[c].start));
                hw.wait(6);
                hw.len(c, self.c[c].len);
                hw.wait(T93_STEP);
            }
        }
        hw.dmacon(0x8000 | mask);
        Self::lines(hw);
        for c in 0..4 {
            if self.c[c].st == 1 {
                self.c[c].po = 0;
                hw.loc(c, self.sbase.wrapping_add(self.c[c].lstart));
                hw.wait(6);
                hw.len(c, self.c[c].llen);
                hw.wait(T93_STEP);
                self.c[c].st = 0;
            }
        }
    }

    /// Effects, volume and period of one voice (spec 5.9).
    fn effects(&mut self, c: usize, hw: &mut Hw) {
        let ch = &mut self.c[c];
        if ch.et != 0 {
            if ch.ef {
                ch.ef = false;
            } else {
                match ch.et {
                    1 => ch.po = ch.po.wrapping_sub(ch.ep),
                    2 => ch.po = ch.po.wrapping_add(ch.ep),
                    4 | 5 => {
                        let u = ((ch.ep / 16) & 15) as u8;
                        let v = ch.v & 127;
                        ch.v = if u != 0 { (v + u).min(64) } else { v.saturating_sub((ch.ep & 15) as u8) };
                        ch.k = false;
                    }
                    6 => {
                        ch.v = ch.ep as u8;
                        ch.k = false;
                    }
                    _ => {}
                }
            }
        }
        let ch = self.c[c];
        let vol = (((ch.v & 127) as u32 * (self.mv / 256)) / 64) & 127;
        hw.vol(c, vol as u16);
        hw.wait(T93_STEP);
        let tr = be16(&self.d, self.lay.trans + 2 * self.sub as usize) as i16 as i32;
        let idx = 72 * ch.ft as i32 + 2 * (ch.nt as i32 + tr);
        let per = be16(&self.d, self.lay.pt.wrapping_add(idx as usize)).wrapping_add(ch.po);
        hw.per(c, per);
        hw.wait(T93_STEP);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            hw.dmacon(15);
            self.setup();
            hw.dmacon(15);
        }
        self.cnt = self.cnt.wrapping_add(1);
        hw.wait(T93_START);
        if self.cnt == 1 {
            for c in 0..3 {
                self.row(c);
            }
            self.effects(3, hw);
        } else if self.cnt == 2 {
            self.row(3);
            self.trigger(hw);
            for c in 0..4 {
                self.effects(c, hw);
            }
        } else {
            if self.cnt == self.speed {
                self.cnt = 0;
            }
            for c in 0..4 {
                self.effects(c, hw);
            }
        }
    }
}

impl Replayer for Ash93 {
    fn format(&self) -> &'static str {
        "Ashley Hogg"
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

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.nsongs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (n as usize) < self.lay.nsongs {
            self.sub = n;
            self.started = false;
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        Ash93::tick(self, hw);
    }
}

// ------------------------------------------------------------------ starting

/// The sample companion's name: the module's name with the leading `ash.` replaced by
/// `smp.`.
fn companion(file: &str) -> Option<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    if file.len() < 5 || !file.is_char_boundary(4) || !file[..4].eq_ignore_ascii_case("ash.") {
        return None;
    }
    Some(format!("smp{}", &file[3..]))
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    match family(d)? {
        Fam::Y91 => {
            let lay = locate91(d).filter(|l| l.nsongs > 0)?;
            Some(Box::new(Ash91::new(d, base, lay)))
        }
        Fam::Y93 => {
            let lay = locate93(d).filter(|l| l.nsongs > 0)?;
            let file = m.bus.read_cstr(m.deli.file_buf);
            let name = companion(&file)?;
            let smp = crate::deli::read_companion(m, &name).1.ok()?;
            let sbase = m.alloc((smp.len() as u32).max(4));
            if sbase == 0 {
                return None;
            }
            m.bus.write_bytes(sbase, &smp);
            Some(Box::new(Ash93::new(d, lay, &smp, sbase)))
        }
    }
}

#[cfg(test)]
#[path = "ashleyhogg_tests.rs"]
mod tests;
