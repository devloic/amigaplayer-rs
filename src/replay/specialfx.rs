//! Special FX (`jd.*`), the music driver of Jonathan Dunn and Matthew Cannon for the
//! Ocean / Special FX games of 1989-93 (Batman, Rambo III, RoboCop 2, Elf, Head Over
//! Heels, Hudson Hawk, Midnight Resistance, Navy SEALs, The New Zealand Story,
//! Operation Thunderbolt, Striker Rage, Wild Wheels ...).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Special FX player and the structure
//! of the corpus files), and this player was written from it alone and checked
//! against those writes.
//!
//! A `jd.*` file is the memory image of a complete 68000 program: a header of
//! branch instructions, the play routine, the data tables and the 8SVX samples.
//! The routine is not run. It is an executable image whose code is read only to
//! find the data: the tables are located structurally by generic instruction
//! anchors (a branch table at the start, the load of the period table in the play
//! routine, the four table loads of the voice routine, the song table and the
//! subsong count in the start routine, the shift counts and volume writes of the
//! hardware routine), and the engine (a four-voice player with order lists,
//! patterns, per-note envelopes, pitch tables, vibrato, slide and "drop" effects,
//! 50 Hz) is written from its behaviour. The few differences between the 29
//! shipped builds (entry layout, period table, table addressing, instrument-list
//! encoding, vibrato shift, voice count, volume shift, the pattern-number escape of
//! two builds) are read from the image the same way.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Special FX", detect, new };

/// The address at which the reference host places the module. The engine's
/// instrument records hold absolute sample addresses, and a period index outside
/// its table reads those records (and the code before the table) as 16-bit
/// halves, so what such a note plays depends on where the original run loaded the
/// image: the port computes with this address, as the specification's runs did.
const VIRT: u32 = 0xF000;

/// "No pointer": reads give zero bytes.
const NULL: i64 = i64::MIN / 4;

// Voice flag bits.
const F_DROP: u16 = 0x01;
const F_PTAB: u16 = 0x02;
const F_VIB: u16 = 0x04;
const F_SLIDE: u16 = 0x08;
const F_ENV: u16 = 0x10;
const F_FIXD: u16 = 0x20;
const F_NOTRANS: u16 = 0x40;
const F_REST: u16 = 0x80;
const F_MUSIC: u16 = 0x8000;
const F_EFFECT: u16 = 0x4000;

/// Order-list and pattern streams that never reach a note give up after this many steps.
const GUARD: u32 = 20000;

fn rw(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

fn s16(w: u16) -> i64 {
    w as i16 as i64
}

// ---------------------------------------------------------------------------------
// Recognition

/// A branch word at `i` with a non-zero, even, positive 16-bit displacement.
fn bra_ok(d: &[u8], i: usize) -> bool {
    let x = rw(d, i + 2) as usize;
    rw(d, i) == 0x6000 && x != 0 && x % 2 == 0 && x < 0x8000
}

fn entry(d: &[u8], k: usize) -> usize {
    4 * k + 2 + rw(d, 4 * k + 2) as usize
}

/// The branch-table test: `(four-entry layout, target of the first branch)`.
fn header(d: &[u8]) -> Option<(bool, usize)> {
    if d.len() < 0x200 || !(0..4).all(|k| bra_ok(d, 4 * k)) {
        return None;
    }
    let t0 = entry(d, 0);
    if rw(d, 16) == 0x6000 {
        if !(4..7).all(|k| bra_ok(d, 4 * k)) || rw(d, 28) == 0x6000 {
            return None;
        }
        // the start routine begins with two address loads (LEA abs.l,A0 / A1)
        if rw(d, t0) != 0x41F9 || rw(d, t0 + 6) != 0x43F9 {
            return None;
        }
        Some((false, t0))
    } else {
        // a subroutine call, then the same two loads
        if rw(d, t0) != 0x6100 || rw(d, t0 + 4) != 0x41F9 || rw(d, t0 + 10) != 0x43F9 {
            return None;
        }
        Some((true, t0))
    }
}

fn detect(d: &[u8]) -> bool {
    header(d).is_some()
}

// ---------------------------------------------------------------------------------
// The structural scan

/// How the instrument list of the build routine is encoded.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Samp {
    /// 8-byte items: `u32` distance to the FORM (0 ends), base, flag.
    E1,
    /// 6-byte items: `s16` distance (0 ends), base, flag.
    E2,
    /// 6-byte items: marker (0 ends), base, flag; the FORM from a pointer table.
    E3,
    /// A list of non-zero longs; the FORMs follow one another.
    E4,
}

#[derive(Clone, Debug)]
struct Lay {
    four: bool,
    origin: i64,
    pt: usize,
    pattab: usize,
    pitchtab: usize,
    envtab: usize,
    insttab: usize,
    /// Table addressing: true = each entry is relative to its own address.
    rel_pat: bool,
    rel_pitch: bool,
    rel_env: bool,
    rel_init: bool,
    songtab: usize,
    nsongs: usize,
    sh: u32,
    esc: bool,
    nv: usize,
    vs: u32,
    m1: u16,
    m2: u16,
    samp: Samp,
    list: usize,
    slot1: usize,
    tab3: usize,
    silent: usize,
}

const LEAS: [u16; 4] = [0x41F9, 0x43F9, 0x45F9, 0x47F9];

fn is_lea(w: u16) -> bool {
    LEAS.contains(&w)
}

/// Whether an instruction word is followed by a 32-bit absolute operand: an address
/// load, a call or jump, a move to or from an absolute long address.
fn is_abs_opcode(w: u16) -> bool {
    // the effective address (low six bits) is absolute long, or it is a MOVE whose
    // destination is
    w & 0x3F == 0x39 || (w & 0xC000 == 0 && w & 0x3000 != 0 && w & 0x0FC0 == 0x03C0)
}

/// The target of a `BSR` at `i`, or None if it is not one.
fn bsr_target(d: &[u8], i: usize) -> Option<usize> {
    let w = rw(d, i);
    if w >> 8 != 0x61 {
        return None;
    }
    match w & 0xFF {
        0 => Some((i as i64 + 2 + s16(rw(d, i + 2))) as usize),
        0xFF => None,
        b => Some((i as i64 + 2 + b as u8 as i8 as i64) as usize),
    }
}

/// After the table load at `p` (a 6-byte instruction on register `n`), whether the
/// next few instructions add the entry to the register: self-relative
/// (`ADDA.W (An),An`) or base-relative (`ADDA.W (An,D0.W),An`).
fn addressing(d: &[u8], p: usize, n: u16) -> Option<bool> {
    let mut q = p + 6;
    while q < p + 24 {
        let w = rw(d, q);
        if w == 0xD0D0 | n << 9 | n {
            return Some(true);
        }
        if w == 0xD0F0 | n << 9 | n {
            return Some(false);
        }
        q += 2;
    }
    None
}

fn scan(d: &[u8]) -> Option<Lay> {
    let (four, samp_at) = header(d)?;
    let n = d.len();
    let (play, init) = if four { (entry(d, 3), entry(d, 2)) } else { (entry(d, 6), entry(d, 4)) };
    if play + 0x40 > n || init + 0x40 > n {
        return None;
    }
    // origin: the play routine loads the address of the period table into A5
    let mut i = play;
    while rw(d, i) != 0x4BF9 {
        i += 2;
        if i > play + 0x400 || i + 6 > n {
            return None;
        }
    }
    let p5 = be32(d, i + 2) as i64;
    let mut pt = i;
    while !matches!(rw(d, pt), 0x01BF | 0x0166) {
        pt += 2;
        if pt > i + 0x2000 || pt + 48 > n {
            return None;
        }
    }
    let origin = p5 - pt as i64;
    let off = |a: u32| -> Option<usize> {
        let o = a as i64 - origin;
        (o >= 0 && (o as usize) < n).then_some(o as usize)
    };
    // the voice routine: the target of the first call in the play routine
    let mut j = play;
    while rw(d, j) >> 8 != 0x61 {
        j += 2;
        if j > play + 0x200 {
            return None;
        }
    }
    let v = bsr_target(d, j)?;
    // its first four table loads
    let mut leas = Vec::new();
    let mut j = v;
    while leas.len() < 4 {
        if is_lea(rw(d, j)) {
            leas.push((j, ((rw(d, j) >> 9) & 7), off(be32(d, j + 2))?));
            j += 6;
        } else {
            j += 2;
        }
        if j > v + 0x400 {
            return None;
        }
    }
    let rel_pat = addressing(d, leas[0].0, leas[0].1)?;
    let rel_pitch = addressing(d, leas[1].0, leas[1].1)?;
    let rel_env = addressing(d, leas[2].0, leas[2].1)?;
    let (pattab, pitchtab, envtab, insttab) = (leas[0].2, leas[1].2, leas[2].2, leas[3].2);
    // the shift of the vibrato difference
    let mut j = v;
    while !matches!(rw(d, j), 0xE640 | 0xE240) {
        j += 2;
        if j > v + 0x400 {
            return None;
        }
    }
    let sh = if rw(d, j) == 0xE640 { 3 } else { 1 };
    // the start routine: song table before the shift by three, subsong count
    let mut j = init;
    while rw(d, j) != 0xE740 {
        j += 2;
        if j > init + 0x200 {
            return None;
        }
    }
    if !is_lea(rw(d, j - 6)) {
        return None;
    }
    let songtab = off(be32(d, j - 4))?;
    let mut j = init;
    loop {
        if rw(d, j) == 0x6508 && matches!(rw(d, j - 4), 0xB07C | 0x0C40) {
            break;
        }
        j += 2;
        if j > init + 0x200 {
            return None;
        }
    }
    let nsongs = rw(d, j - 2) as usize;
    // the pattern table as loaded by the start routine (register A2)
    let mut j = init;
    while rw(d, j) != 0x45F9 {
        j += 2;
        if j > init + 0x200 {
            return None;
        }
    }
    let rel_init = addressing(d, j, 2)?;
    // the order-list loop: count its decrement-and-test steps
    let mut steps = 0;
    let mut j = init;
    while rw(d, j) != 0x4E75 && j < init + 0x200 {
        if rw(d, j) == 0x5300 && rw(d, j + 2) >> 8 == 0x66 {
            steps += 1;
        }
        j += 2;
    }
    let esc = steps >= 4;
    // the hardware routine: the last call of the play routine before its return
    let mut j = play;
    let mut last = None;
    while rw(d, j) != 0x4E75 {
        if rw(d, j) == 0x6100 {
            last = Some(j);
        }
        j += 2;
        if j > play + 0x400 {
            return None;
        }
    }
    let hw = bsr_target(d, last?)?;
    let (mut nv, mut vs, mut mem) = (0, 0, Vec::new());
    let mut j = hw;
    while rw(d, j) != 0x4E75 {
        let w = rw(d, j);
        if w == 0x3D43 && matches!(rw(d, j + 2), 0xA8 | 0xB8 | 0xC8 | 0xD8) {
            nv += 1;
        }
        if w & 0xF1FF == 0xE043 {
            vs = ((w >> 9) & 7) as u32;
        }
        if matches!(w, 0x3239 | 0x3439) {
            mem.push(off(be32(d, j + 2)).map_or(0xFFFF, |o| rw(d, o)));
        }
        j += 2;
        if j > hw + 0x400 {
            return None;
        }
    }
    if nv == 0 {
        return None;
    }
    let (m1, m2) = (mem.first().copied().unwrap_or(0xFFFF), mem.get(1).copied().unwrap_or(0xFFFF));
    // the instrument-building routine: its leading loads and calls tell the list encoding
    let mut seq = String::new();
    let mut ops = Vec::new();
    let mut j = samp_at;
    for _ in 0..4 {
        let w = rw(d, j);
        if is_lea(w) {
            seq.push('L');
            ops.push((true, off(be32(d, j + 2)).unwrap_or(0), j));
            j += 6;
        } else if let Some(t) = bsr_target(d, j) {
            seq.push('B');
            ops.push((false, t, j));
            j += if rw(d, j) & 0xFF == 0 { 4 } else { 2 };
        } else {
            break;
        }
    }
    let samp = match seq.as_str() {
        "LLB" => Samp::E1,
        "BLL" => Samp::E2,
        "BLLL" => Samp::E3,
        "LLLB" => Samp::E4,
        _ => return None,
    };
    let leas_of: Vec<usize> = ops.iter().filter(|o| o.0).map(|o| o.1).collect();
    let (list, slot1) = (leas_of[0], leas_of[1]);
    let tab3 = leas_of.get(2).copied().unwrap_or(0);
    // the two-byte silence buffer: the load before `MOVEQ #2,D1` in the build code
    let mut starts: Vec<usize> = ops.iter().filter(|o| !o.0).map(|o| o.1).collect();
    starts.push(samp_at);
    let mut silent = None;
    'find: for s in starts {
        let mut k = s;
        while k < s + 0x100 && k + 6 <= n {
            if rw(d, k) == 0x7202 && k >= 6 && is_lea(rw(d, k - 6)) {
                silent = off(be32(d, k - 4));
                break 'find;
            }
            k += 2;
        }
    }
    // NZS: the written loop address of its list encoding is 2 bytes below the load operand
    let silent = silent?.checked_sub(if samp == Samp::E2 { 2 } else { 0 })?;
    // the tables must lie in the file
    if pattab >= n || pitchtab >= n || envtab >= n || insttab >= n || songtab + 8 > n || slot1 < insttab || nsongs == 0 {
        return None;
    }
    Some(Lay {
        four,
        origin,
        pt,
        pattab,
        pitchtab,
        envtab,
        insttab,
        rel_pat,
        rel_pitch,
        rel_env,
        rel_init,
        songtab,
        nsongs,
        sh,
        esc,
        nv: nv.min(4),
        vs,
        m1,
        m2,
        samp,
        list,
        slot1,
        tab3,
        silent,
    })
}

// ---------------------------------------------------------------------------------
// Instruments: what the build routine makes of the 8SVX samples

fn put16(d: &mut [u8], o: usize, v: u16) {
    if let Some(b) = d.get_mut(o..o + 2) {
        b.copy_from_slice(&v.to_be_bytes());
    }
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    if let Some(b) = d.get_mut(o..o + 4) {
        b.copy_from_slice(&v.to_be_bytes());
    }
}

/// Walks the chunks of an IFF container, entering FORM / LIST / CAT.
fn walk(d: &[u8], mut p: usize, end: usize, vhdr: &mut Option<usize>, body: &mut Option<usize>, depth: u32) {
    let end = end.min(d.len());
    while p + 8 <= end {
        let tag = &d[p..p + 4];
        let sz = be32(d, p + 4) as usize;
        match tag {
            b"FORM" | b"LIST" | b"CAT " if depth < 8 => walk(d, p + 12, p.saturating_add(8).saturating_add(sz), vhdr, body, depth + 1),
            b"VHDR" => *vhdr = Some(p + 8),
            b"BODY" => *body = Some(p + 8),
            _ => {}
        }
        p = p.saturating_add(8).saturating_add(sz + (sz & 1));
    }
}

/// The runtime image: the file with the instrument records as the build routine
/// leaves them (absolute addresses relative to `VIRT`, as 32-bit values).
fn build(d: &[u8], l: &Lay) -> Vec<u8> {
    let mut rt = d.to_vec();
    let n = d.len();
    let ra = |a: u32| -> u32 {
        if a == 0 {
            0
        } else {
            (a as i64 - l.origin + VIRT as i64) as u32
        }
    };
    let first = (l.slot1 - l.insttab) / 16;
    // the records the image holds: their absolute addresses, as the host relocates them
    for r in 0..first {
        let o = l.insttab + 16 * r;
        if o + 16 > n {
            break;
        }
        put32(&mut rt, o + 2, ra(be32(d, o + 2)));
        put32(&mut rt, o + 8, ra(be32(d, o + 8)));
    }
    let to_ra = |off: usize| -> u32 { VIRT.wrapping_add(off as u32) };
    let (mut vhdr, mut body): (Option<usize>, Option<usize>) = (None, None);
    let mut item = l.list;
    let mut form4 = if l.samp == Samp::E4 { (l.tab3 != 0).then_some(l.tab3) } else { None };
    let mut k = 0usize;
    while k < 255 {
        let o = l.insttab + 16 * (first + k);
        if o + 16 > n {
            break;
        }
        let (form, base, flag): (Option<usize>, Option<u16>, Option<u16>) = match l.samp {
            Samp::E1 => {
                let dist = be32(d, item);
                if dist == 0 || item + 8 > n {
                    break;
                }
                let f = item as i64 + dist as i32 as i64;
                let r = (rw(d, item + 4), rw(d, item + 6));
                item += 8;
                ((f >= 0).then_some(f as usize), Some(r.0), Some(r.1))
            }
            Samp::E2 => {
                let dist = rw(d, item);
                if dist == 0 || item + 6 > n {
                    break;
                }
                let f = item as i64 + s16(dist);
                let r = (rw(d, item + 2), rw(d, item + 4));
                item += 6;
                ((f >= 0).then_some(f as usize), Some(r.0), Some(r.1))
            }
            Samp::E3 => {
                if rw(d, item) == 0 || item + 6 > n {
                    break;
                }
                let p = be32(d, l.tab3 + 4 * k) as i64 - l.origin + 8;
                let r = (rw(d, item + 2), rw(d, item + 4));
                item += 6;
                ((p >= 0).then_some(p as usize), Some(r.0), Some(r.1))
            }
            Samp::E4 => {
                if be32(d, item) == 0 || item + 4 > n {
                    break;
                }
                item += 4;
                let f = form4;
                form4 = f.map(|f| f + 8 + be32(d, f + 4) as usize);
                (f, None, None)
            }
        };
        if let Some(f) = form {
            if d.get(f..f + 4) == Some(b"FORM") {
                let (mut v2, mut b2) = (None, None);
                walk(d, f, f.saturating_add(8).saturating_add(be32(d, f + 4) as usize), &mut v2, &mut b2, 0);
                vhdr = v2.or(vhdr);
                body = b2.or(body);
            }
        }
        let (one, rep) = vhdr.map_or((0, 0), |v| (be32(d, v) & !1, be32(d, v + 4) & !1));
        let start = body.map_or(0, to_ra);
        let length = ((one as u64 + rep as u64) / 2) as u16;
        let (lp, ll) = if rep != 0 { (start.wrapping_add(one), (rep / 2) as u16) } else { (to_ra(l.silent), 1) };
        if let Some(b) = base {
            put16(&mut rt, o, b);
        }
        put32(&mut rt, o + 2, start);
        put16(&mut rt, o + 6, length);
        put32(&mut rt, o + 8, lp);
        put16(&mut rt, o + 12, ll);
        if let Some(f) = flag {
            put16(&mut rt, o + 14, f);
        }
        k += 1;
    }
    rt
}

// ---------------------------------------------------------------------------------
// The engine

#[derive(Clone)]
struct Voice {
    flags: u16,
    loop_ptr: i64,
    ol_ptr: i64,
    pat_ptr: i64,
    pat_loop: i64,
    rep: u8,
    transpose: u8,
    dur: u16,
    fixed_dur: u8,
    env_ptr: i64,
    envp: i64,
    env_left: u16,
    env_period: u8,
    env_cnt: u8,
    env_step: i16,
    base: i16,
    start: u32,
    len: u16,
    lp: u32,
    looplen: u16,
    period: u16,
    volume: u16,
    raw_note: u8,
    eff_note: u8,
    drop_add: u8,
    drop_delay: u8,
    drop_cnt: u8,
    slide_semis: u8,
    slide_ticks: u8,
    slide_step: i16,
    pt_start: i64,
    pt_pos: i64,
    pt_len: u8,
    pt_left: u8,
    pt_speed: u8,
    pt_cnt: u8,
    vib_semis: u8,
    vib_steps: u8,
    vib_delay: u8,
    vib_cnt: i16,
    vib_half: u8,
    vib_reload: u8,
    vib_delta: i16,
    /// The voice has executed its first jump to the order list's loop point.
    jumped: bool,
}

impl Voice {
    /// The state after the stop routine.
    fn new() -> Voice {
        Voice {
            flags: 0,
            loop_ptr: NULL,
            ol_ptr: 0,
            pat_ptr: 0,
            pat_loop: NULL,
            rep: 0,
            transpose: 0,
            dur: 0,
            fixed_dur: 0,
            env_ptr: NULL,
            envp: 0,
            env_left: 0,
            env_period: 0,
            env_cnt: 0,
            env_step: 0,
            base: 0,
            start: 0,
            len: 0,
            lp: 0,
            looplen: 0,
            period: 1,
            volume: 0,
            raw_note: 0,
            eff_note: 0,
            drop_add: 0,
            drop_delay: 0,
            drop_cnt: 0,
            slide_semis: 0,
            slide_ticks: 0,
            slide_step: 0,
            pt_start: 0,
            pt_pos: 0,
            pt_len: 0,
            pt_left: 0,
            pt_speed: 0,
            pt_cnt: 0,
            vib_semis: 0,
            vib_steps: 0,
            vib_delay: 0,
            vib_cnt: 0,
            vib_half: 0,
            vib_reload: 0,
            vib_delta: 0,
            jumped: false,
        }
    }
}

struct Sfx {
    lay: Lay,
    rt: Vec<u8>,
    base: u32,
    title: String,
    sub: u16,
    started: bool,
    songend: bool,
    v: [Voice; 4],
    /// The voices of the song, and those that have not jumped yet.
    in_song: [bool; 4],
    active: bool,
    dmamask: u8,
    /// A voice's duration counter is decremented even at zero (the builds with a
    /// four-entry branch table).
    unguarded: bool,
}

impl Sfx {
    fn rd8(&self, o: i64) -> u8 {
        if o < 0 { 0 } else { self.rt.get(o as usize).copied().unwrap_or(0) }
    }

    fn rd16(&self, o: i64) -> u16 {
        if o < 0 { 0 } else { be16(&self.rt, o as usize) }
    }

    /// `PERIOD(i)`: the word `i` places from the start of the table, wherever in the image.
    fn period(&self, i: i32) -> u16 {
        let o = self.lay.pt as i64 + 2 * i as i64;
        if o < 0 || o >= self.lay.pt as i64 {
            return self.rd16(o);
        }
        // Before the table lies the code. The reference host relocates its absolute
        // 32-bit operands to the load address, so such a word reads as the address
        // (decided from Head Over Heels song 6, the one run that reads there).
        let o = o as usize;
        let hi_of = |p: usize| -> Option<u32> {
            // a word at p that is the high half of an operand, or None
            let op = rw(&self.rt, p.checked_sub(2)?);
            let a = be32(&self.rt, p);
            (is_abs_opcode(op) && a >> 20 == 0).then_some(a)
        };
        if let Some(a) = hi_of(o) {
            return (self.reloc(a) >> 16) as u16;
        }
        if let Some(p) = o.checked_sub(2) {
            if let Some(a) = hi_of(p) {
                return self.reloc(a) as u16;
            }
        }
        self.rd16(o as i64)
    }

    /// An absolute address of the original image as the reference host has it.
    fn reloc(&self, a: u32) -> u32 {
        if a == 0 { 0 } else { (a as i64 - self.lay.origin + VIRT as i64) as u32 }
    }

    /// The target of entry `i` of the table at `tab`.
    fn target(&self, tab: usize, rel_self: bool, i: u8) -> i64 {
        let e = tab as i64 + 2 * i as i64;
        let w = s16(self.rd16(e));
        if rel_self { e + w } else { tab as i64 + w }
    }

    fn pattern(&self, i: u8) -> i64 {
        self.target(self.lay.pattab, self.lay.rel_pat, i)
    }

    /// The guest address of a runtime-image address.
    fn guest(&self, ra: u32) -> u32 {
        if ra == 0 { 0 } else { self.base.wrapping_add(ra.wrapping_sub(VIRT)) }
    }

    fn stop(&mut self, hw: &mut Hw) {
        for ch in 0..4 {
            hw.vol(ch, 0);
        }
        self.v = [Voice::new(), Voice::new(), Voice::new(), Voice::new()];
        self.in_song = [false; 4];
        self.active = false;
        self.dmamask = 0;
    }

    /// Starts subsong `n` (1-based).
    fn init(&mut self, n: usize, hw: &mut Hw) {
        if n == 0 || n > self.lay.nsongs {
            self.stop(hw);
            return;
        }
        for v in 0..4 {
            let at = self.lay.songtab + 8 * (n - 1) + 2 * v;
            let w = self.rd16(at as i64);
            if w == 0 {
                continue;
            }
            let mut rd = at as i64 + s16(w);
            let vo = &mut self.v[v];
            vo.volume = 0;
            vo.loop_ptr = NULL;
            vo.pat_loop = NULL;
            vo.transpose = 0;
            vo.rep = 1;
            vo.flags = F_MUSIC;
            vo.jumped = false;
            let mut guard = 0;
            loop {
                guard += 1;
                let b = self.rd8(rd);
                rd += 1;
                let mut pat = None;
                match b {
                    0..=0x7F => pat = Some(b),
                    0x84 if self.lay.esc => {
                        pat = Some(self.rd8(rd));
                        rd += 1;
                    }
                    0x80 => {
                        let lp = self.v[v].loop_ptr;
                        if lp == NULL {
                            self.v[v].flags = 0;
                            break;
                        }
                        rd = lp;
                    }
                    0x81 => self.v[v].loop_ptr = rd,
                    0x82 => {
                        self.v[v].transpose = self.rd8(rd);
                        rd += 1;
                    }
                    0x83 => {
                        self.v[v].rep = self.rd8(rd);
                        rd += 1;
                    }
                    _ => {}
                }
                if let Some(p) = pat {
                    let t = self.target(self.lay.pattab, self.lay.rel_init, p);
                    let vo = &mut self.v[v];
                    vo.ol_ptr = rd;
                    vo.pat_ptr = t;
                    vo.dur = 1;
                    break;
                }
                if guard > GUARD {
                    self.v[v].flags = 0;
                    break;
                }
            }
        }
        self.in_song = std::array::from_fn(|v| self.v[v].flags != 0);
        self.active = self.v.iter().any(|v| v.flags != 0);
    }

    fn tick_sfx(&mut self, hw: &mut Hw) {
        self.dmamask = 0;
        if self.active {
            self.active = false;
            for v in 0..self.lay.nv {
                if self.v[v].flags != 0 {
                    self.voice(v);
                    self.active |= self.v[v].flags & (F_MUSIC | F_EFFECT) != 0;
                }
            }
        }
        self.hardware(hw);
        // the song ends when every voice of the song has jumped to its loop point
        if (0..4).filter(|&v| self.in_song[v]).all(|v| self.v[v].jumped) {
            self.songend = true;
        }
    }

    fn voice(&mut self, v: usize) {
        let mut parse = false;
        if self.unguarded || self.v[v].dur != 0 {
            self.v[v].dur = self.v[v].dur.wrapping_sub(1);
            parse = self.v[v].dur == 0;
        }
        if parse {
            self.parse(v);
            if self.v[v].flags == 0 {
                return;
            }
        }
        if self.v[v].flags & F_REST != 0 {
            return;
        }
        self.effects(v);
        self.envelope(v);
    }

    /// The duration byte of a note or rest, added to the counter.
    fn add_duration(&mut self, v: usize) {
        let d = if self.v[v].flags & F_FIXD != 0 {
            self.v[v].fixed_dur
        } else {
            let b = self.rd8(self.v[v].pat_ptr);
            self.v[v].pat_ptr += 1;
            b
        };
        self.v[v].dur = self.v[v].dur.wrapping_add(d as u16);
    }

    fn next(&mut self, v: usize) -> u8 {
        let b = self.rd8(self.v[v].pat_ptr);
        self.v[v].pat_ptr += 1;
        b
    }

    fn parse(&mut self, v: usize) {
        self.v[v].flags &= !F_REST;
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > GUARD {
                self.v[v].flags = 0;
                return;
            }
            let b = self.next(v);
            if b < 0x80 {
                self.note(v, b);
                return;
            }
            self.v[v].flags &= !F_SLIDE;
            if b >= 0xA0 {
                continue;
            }
            match b {
                0x80 => {
                    if !self.end_of_pattern(v) {
                        return;
                    }
                }
                0x81 => self.v[v].pat_loop = self.v[v].pat_ptr,
                0x84 => {
                    self.v[v].flags |= F_FIXD;
                    self.v[v].fixed_dur = self.next(v);
                }
                0x85 => self.v[v].flags &= !F_FIXD,
                0x86 => self.v[v].dur = self.v[v].dur.wrapping_add(0x100),
                0x87 => {
                    self.v[v].flags &= !F_ENV;
                    self.v[v].flags |= F_REST;
                    self.v[v].volume = 0;
                    self.add_duration(v);
                    return;
                }
                0x88 => {
                    self.v[v].flags |= F_SLIDE;
                    self.v[v].slide_semis = self.next(v);
                    self.v[v].slide_ticks = self.next(v);
                }
                0x89 => {
                    self.v[v].flags = (self.v[v].flags & !7) | F_DROP;
                    self.v[v].drop_add = self.next(v);
                    self.v[v].drop_delay = self.next(v);
                }
                0x8A => self.v[v].flags &= !F_DROP,
                0x8B => {
                    self.v[v].flags = (self.v[v].flags & !7) | F_VIB;
                    self.v[v].vib_delay = self.next(v);
                    self.v[v].vib_semis = self.next(v);
                    self.v[v].vib_steps = self.next(v);
                }
                0x8C => self.v[v].flags &= !F_VIB,
                0x8D => {
                    self.v[v].flags = (self.v[v].flags & !7) | F_PTAB;
                    let i = self.next(v);
                    let t = self.target(self.lay.pitchtab, self.lay.rel_pitch, i);
                    self.v[v].pt_speed = self.rd8(t);
                    self.v[v].pt_len = self.rd8(t + 1);
                    self.v[v].pt_start = t + 2;
                }
                0x8E => self.v[v].flags &= !F_PTAB,
                0x8F => {
                    let i = self.next(v);
                    self.v[v].env_ptr = self.target(self.lay.envtab, self.lay.rel_env, i);
                }
                0x90 => {
                    let i = self.next(v);
                    let o = (self.lay.insttab + 16 * i as usize) as i64;
                    let (base, start, len, lp, ll, flag) = (
                        self.rd16(o) as i16,
                        (self.rd16(o + 2) as u32) << 16 | self.rd16(o + 4) as u32,
                        self.rd16(o + 6),
                        (self.rd16(o + 8) as u32) << 16 | self.rd16(o + 10) as u32,
                        self.rd16(o + 12),
                        self.rd16(o + 14),
                    );
                    let vo = &mut self.v[v];
                    vo.base = base;
                    vo.start = start;
                    vo.len = len;
                    vo.lp = lp;
                    vo.looplen = ll;
                    vo.flags &= !F_NOTRANS;
                    if flag != 0 {
                        vo.flags |= F_NOTRANS;
                    }
                }
                _ => {}
            }
        }
    }

    /// The end of a pattern. Returns false when the voice has ended.
    fn end_of_pattern(&mut self, v: usize) -> bool {
        if self.v[v].pat_loop != NULL {
            self.v[v].pat_ptr = self.v[v].pat_loop;
            return true;
        }
        let mut rd = self.v[v].ol_ptr;
        self.v[v].rep = self.v[v].rep.wrapping_sub(1);
        if self.v[v].rep != 0 {
            let p = self.rd8(rd - 1);
            self.v[v].ol_ptr = rd;
            self.v[v].pat_ptr = self.pattern(p);
            return true;
        }
        self.v[v].rep = 1;
        self.v[v].transpose = 0;
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > GUARD {
                self.v[v].flags = 0;
                return false;
            }
            let b = self.rd8(rd);
            rd += 1;
            if b < 0x80 {
                self.v[v].ol_ptr = rd;
                self.v[v].pat_ptr = self.pattern(b);
                return true;
            }
            match b - 0x80 {
                0 => {
                    self.v[v].jumped = true;
                    rd = self.v[v].loop_ptr;
                    if rd == NULL {
                        let vo = &mut self.v[v];
                        vo.period = 1;
                        vo.volume = 0;
                        vo.flags = 0;
                        return false;
                    }
                }
                1 => self.v[v].loop_ptr = rd,
                2 => {
                    self.v[v].transpose = self.rd8(rd);
                    rd += 1;
                }
                3 => {
                    self.v[v].rep = self.rd8(rd);
                    rd += 1;
                }
                _ => {}
            }
        }
    }

    fn note(&mut self, v: usize, n: u8) {
        let sh = self.lay.sh;
        let fl = self.v[v].flags;
        let base = self.v[v].base as i32;
        let mut t = n;
        if fl & F_NOTRANS == 0 {
            t = t.wrapping_add(self.v[v].transpose);
        }
        self.v[v].raw_note = t;
        if fl & F_DROP != 0 {
            t = t.wrapping_add(self.v[v].drop_add);
            self.v[v].drop_cnt = self.v[v].drop_delay;
        }
        self.v[v].eff_note = t;
        let mut p = self.period(t as i8 as i32 - base);
        if fl & F_VIB != 0 {
            let target = self.period(t.wrapping_add(self.v[v].vib_semis) as i8 as i32 - base);
            let diff = (target.wrapping_sub(p) as i16) >> sh;
            let steps = self.v[v].vib_steps as i8 as i16;
            self.v[v].vib_delta = if steps == 0 { 0 } else { diff.wrapping_div(steps) };
            self.v[v].vib_half = self.v[v].vib_steps;
            self.v[v].vib_reload = self.v[v].vib_steps.wrapping_mul(2);
            self.v[v].vib_cnt = self.v[v].vib_delay as i16;
        }
        if fl & F_SLIDE != 0 {
            let target = self.period(t.wrapping_add(self.v[v].slide_semis) as i8 as i32 - base);
            let diff = target.wrapping_sub(p) as i16;
            let ticks = self.v[v].slide_ticks as i16;
            let step = if ticks == 0 { 0 } else { diff.wrapping_div(ticks) };
            self.v[v].slide_step = step;
            p = p.wrapping_add(step.wrapping_mul(ticks) as u16);
        }
        self.v[v].period = p;
        if fl & F_PTAB != 0 {
            let vo = &mut self.v[v];
            vo.pt_cnt = 1;
            vo.pt_pos = vo.pt_start;
            vo.pt_left = vo.pt_len;
        }
        let vo = &mut self.v[v];
        vo.flags |= F_ENV;
        vo.envp = vo.env_ptr;
        vo.env_left = 1;
        vo.volume = 0;
        self.dmamask |= 1 << v;
        self.add_duration(v);
    }

    fn effects(&mut self, v: usize) {
        let fl = self.v[v].flags;
        let base = self.v[v].base as i32;
        let mut p = self.v[v].period;
        if fl & F_SLIDE != 0 {
            let vo = &mut self.v[v];
            vo.slide_ticks = vo.slide_ticks.wrapping_sub(1);
            if vo.slide_ticks == 0 {
                vo.flags &= !F_SLIDE;
            }
            p = p.wrapping_sub(vo.slide_step as u16);
        } else if fl & F_DROP != 0 {
            let vo = &mut self.v[v];
            let wrapped = vo.drop_cnt == 0;
            vo.drop_cnt = vo.drop_cnt.wrapping_sub(1);
            if wrapped {
                vo.drop_cnt = 0xFF;
                vo.eff_note = vo.raw_note;
                p = self.period(self.v[v].raw_note as i8 as i32 - base);
            }
        } else if fl & F_PTAB != 0 {
            self.v[v].pt_cnt = self.v[v].pt_cnt.wrapping_sub(1);
            if self.v[v].pt_cnt == 0 {
                let o = self.rd8(self.v[v].pt_pos);
                let vo = &mut self.v[v];
                vo.pt_pos += 1;
                vo.pt_left = vo.pt_left.wrapping_sub(1);
                if vo.pt_left == 0 {
                    vo.pt_pos = vo.pt_start;
                    vo.pt_left = vo.pt_len;
                }
                vo.pt_cnt = vo.pt_speed;
                let e = vo.eff_note;
                p = self.period(o.wrapping_add(e) as i8 as i32 - base);
            }
        } else if fl & F_VIB != 0 {
            let vo = &mut self.v[v];
            vo.vib_cnt -= 1;
            if vo.vib_cnt < 0 {
                vo.vib_cnt = 0;
                vo.vib_half = vo.vib_half.wrapping_sub(1);
                if vo.vib_half == 0 {
                    vo.vib_delta = vo.vib_delta.wrapping_neg();
                    vo.vib_half = vo.vib_reload;
                }
                p = p.wrapping_add(vo.vib_delta as u16);
            }
        }
        self.v[v].period = p;
    }

    fn envelope(&mut self, v: usize) {
        if self.v[v].flags & F_ENV == 0 {
            return;
        }
        self.v[v].env_left = self.v[v].env_left.wrapping_sub(1);
        if self.v[v].env_left == 0 {
            let e = self.rd8(self.v[v].envp);
            self.v[v].envp += 1;
            if e == 0 {
                let m = self.rd8(self.v[v].envp);
                if m == 0 {
                    self.v[v].flags &= !F_ENV;
                    return;
                }
                let mag = (m as i8 as i32).abs();
                let r = mag - self.v[v].dur as i32;
                let vo = &mut self.v[v];
                if r < 0 {
                    vo.env_left = (-r) as u16;
                    vo.env_period = 0;
                    vo.env_cnt = 0;
                    vo.env_step = 0;
                } else {
                    vo.flags &= !F_ENV;
                }
                return;
            } else if e >= 0x80 {
                let mut s = self.rd8(self.v[v].envp) as u16;
                self.v[v].envp += 1;
                let vo = &mut self.v[v];
                vo.env_left = 256 - e as u16;
                vo.env_step = 1;
                if s >= 0x80 {
                    s = 256 - s;
                    vo.env_step = -1;
                }
                vo.env_period = s as u8;
                vo.env_cnt = 1;
            } else {
                let st = self.rd8(self.v[v].envp) as i8 as i16;
                let vo = &mut self.v[v];
                vo.envp += 1;
                vo.env_left = e as u16;
                vo.env_period = 1;
                vo.env_cnt = 1;
                vo.env_step = st;
            }
        }
        let vo = &mut self.v[v];
        vo.env_cnt = vo.env_cnt.wrapping_sub(1);
        if vo.env_cnt == 0 {
            vo.env_cnt = vo.env_period;
            vo.volume = vo.volume.wrapping_add(vo.env_step as u16);
        }
    }

    fn hardware(&mut self, hw: &mut Hw) {
        let d = self.dmamask;
        if d != 0 {
            // stop the voices that start a note and set their output to the idle state;
            // the original waits for the audio interrupt requests and three raster
            // lines here (Paula has no AUDxDAT register to write in this model)
            hw.dmacon(d as u16);
            for ch in 0..self.lay.nv {
                if d & 1 << ch != 0 {
                    hw.per(ch, 1);
                }
            }
            hw.wait(2000);
        }
        for ch in 0..self.lay.nv {
            let vo = &self.v[ch];
            let mask = if vo.flags & F_EFFECT != 0 { self.lay.m2 } else { self.lay.m1 };
            let x = vo.volume & mask;
            let x = ((x as u32 * 64) as u16) >> 6;
            hw.vol(ch, x >> self.lay.vs);
            hw.per(ch, vo.period);
            if d & 1 << ch != 0 {
                hw.len(ch, vo.len);
                hw.loc(ch, self.guest(vo.start));
            } else {
                hw.len(ch, vo.looplen);
                hw.loc(ch, self.guest(vo.lp));
            }
            hw.wait(100);
        }
        hw.dmacon(0x8000 | d as u16);
    }
}

impl Replayer for Sfx {
    fn format(&self) -> &'static str {
        "Special FX"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.nsongs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.sub = n;
        self.started = false;
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.songend = false;
            self.stop(hw);
            self.init(self.sub as usize + 1, hw);
            if !self.in_song.iter().any(|&b| b) {
                // a song without voices ends at once
                self.songend = true;
            }
        }
        self.tick_sfx(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = scan(d)?;
    let title = m.bus.read_cstr(m.deli.file_buf);
    let rt = build(d, &lay);
    let unguarded = lay.four;
    Some(Box::new(Sfx {
        lay,
        rt,
        base,
        title,
        sub: 0,
        started: false,
        songend: false,
        v: [Voice::new(), Voice::new(), Voice::new(), Voice::new()],
        in_song: [false; 4],
        active: false,
        dmamask: 0,
        unguarded,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn w16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn w32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// A synthetic module of the seven-entry layout (origin 0): the anchors the scan
    /// looks for, one song of one voice, one instrument (an 8SVX sample of 8 + 8
    /// bytes) and `pattern` as the voice's only pattern, played once.
    fn module(pattern: &[u8], escape: bool) -> Vec<u8> {
        let mut d = vec![0u8; 0x800];
        let (samp, stop, init, play, vr, hwr, build_r) = (0x40usize, 0x60usize, 0x80usize, 0xE0usize, 0x120usize, 0x160usize, 0x1A0usize);
        let targets = [samp, stop, stop, stop, init, init, play];
        for (k, t) in targets.iter().enumerate() {
            w16(&mut d, 4 * k, 0x6000);
            w16(&mut d, 4 * k + 2, (*t - 4 * k - 2) as u16);
        }
        let (pt, insttab, list, form, songtab, pattab, pitchtab, envtab, silent, mask) =
            (0x200usize, 0x230usize, 0x400usize, 0x440usize, 0x500usize, 0x600usize, 0x640usize, 0x680usize, 0x740usize, 0x750usize);
        // SAMP: LEA list,A0; LEA slot 1,A1; BSR build; RTS
        w16(&mut d, samp, 0x41F9);
        w32(&mut d, samp + 2, list as u32);
        w16(&mut d, samp + 6, 0x43F9);
        w32(&mut d, samp + 8, (insttab + 16) as u32);
        w16(&mut d, samp + 12, 0x6100);
        w16(&mut d, samp + 14, (build_r - samp - 14) as u16);
        w16(&mut d, samp + 16, 0x4E75);
        w16(&mut d, stop, 0x4E75);
        // INIT: CMP.W #1,D0; BCS.B; LEA songtab,A0; ASL.W #3,D0; LEA pattab,A2; ADDA.W (A2),A2; steps
        w16(&mut d, init, 0xB07C);
        w16(&mut d, init + 2, 1);
        w16(&mut d, init + 4, 0x6508);
        w16(&mut d, init + 6, 0x43F9);
        w32(&mut d, init + 8, songtab as u32);
        w16(&mut d, init + 12, 0xE740);
        w16(&mut d, init + 14, 0x45F9);
        w32(&mut d, init + 16, pattab as u32);
        w16(&mut d, init + 20, 0xD4D2);
        let steps = if escape { 4 } else { 3 };
        for s in 0..steps {
            w16(&mut d, init + 24 + 4 * s, 0x5300);
            w16(&mut d, init + 26 + 4 * s, 0x6606);
        }
        w16(&mut d, init + 24 + 4 * steps, 0x4E75);
        // PLAY: LEA period table,A5; BSR voice; BSR hardware; RTS
        w16(&mut d, play, 0x4BF9);
        w32(&mut d, play + 2, pt as u32);
        w16(&mut d, play + 6, 0x6100);
        w16(&mut d, play + 8, (vr - play - 8) as u16);
        w16(&mut d, play + 10, 0x6100);
        w16(&mut d, play + 12, (hwr - play - 12) as u16);
        w16(&mut d, play + 14, 0x4E75);
        // VOICE: the four table loads, each followed by its self-relative add, the shift
        for (k, (op, a)) in [(0x41F9u16, pattab), (0x43F9, pitchtab), (0x45F9, envtab), (0x47F9, insttab)].iter().enumerate() {
            w16(&mut d, vr + 8 * k, *op);
            w32(&mut d, vr + 8 * k + 2, *a as u32);
            let n = k as u16;
            w16(&mut d, vr + 8 * k + 6, if k < 3 { 0xD0D0 | n << 9 | n } else { 0x4E71 });
        }
        w16(&mut d, vr + 32, 0xE240);
        w16(&mut d, vr + 34, 0x4E75);
        // HARDWARE: the four volume writes, the two mask loads
        w16(&mut d, hwr, 0x3239);
        w32(&mut d, hwr + 2, mask as u32);
        w16(&mut d, hwr + 6, 0x3439);
        w32(&mut d, hwr + 8, (mask + 2) as u32);
        for (k, r) in [0xA8u16, 0xB8, 0xC8, 0xD8].iter().enumerate() {
            w16(&mut d, hwr + 12 + 4 * k, 0x3D43);
            w16(&mut d, hwr + 14 + 4 * k, *r);
        }
        w16(&mut d, hwr + 28, 0x4E75);
        // the build routine: the silence buffer before MOVEQ #2,D1
        w16(&mut d, build_r, 0x41F9);
        w32(&mut d, build_r + 2, silent as u32);
        w16(&mut d, build_r + 6, 0x7202);
        w16(&mut d, build_r + 8, 0x4E75);
        w16(&mut d, mask, 0xFFFF);
        w16(&mut d, mask + 2, 0xFFFF);
        // period table (new)
        let per = [
            0x1BFu16, 0x1A6, 0x18F, 0x178, 0x163, 0x14F, 0x13C, 0x12B, 0x11A, 0x10A, 0xFB, 0xED, 0xE0, 0xD3, 0xC7, 0xBC, 0xB2, 0xA8, 0x9E, 0x95,
            0x8D, 0x85, 0x7E, 0x77,
        ];
        for (i, p) in per.iter().enumerate() {
            w16(&mut d, pt + 2 * i, *p);
        }
        // instrument list: one item (FORM 0x40 bytes on, base 0, flag 0), then the end
        w32(&mut d, list, (form - list) as u32);
        d[form..form + 4].copy_from_slice(b"FORM");
        w32(&mut d, form + 4, 4 + 8 + 20 + 8 + 16);
        d[form + 8..form + 12].copy_from_slice(b"8SVX");
        d[form + 12..form + 16].copy_from_slice(b"VHDR");
        w32(&mut d, form + 16, 20);
        w32(&mut d, form + 20, 8);
        w32(&mut d, form + 24, 8);
        d[form + 40..form + 44].copy_from_slice(b"BODY");
        w32(&mut d, form + 44, 16);
        // song 1: voice 1 has an order list [pattern 0, end]
        w16(&mut d, songtab, 0x40);
        d[songtab + 0x40] = 0;
        d[songtab + 0x41] = 0x80;
        // pattern table, self-relative
        w16(&mut d, pattab, 0x100);
        d[pattab + 0x100..pattab + 0x100 + pattern.len()].copy_from_slice(pattern);
        // envelope 0: four ticks of +8, end
        w16(&mut d, envtab, 0x40);
        d[envtab + 0x40..envtab + 0x44].copy_from_slice(&[4, 8, 0, 0]);
        w16(&mut d, pitchtab, 0x60);
        d
    }

    /// Instrument 1, envelope 0, note 12 for 4 ticks, end of pattern.
    const SIMPLE: [u8; 8] = [0x90, 1, 0x8F, 0, 12, 4, 0x80, 0];

    #[test]
    fn scan_finds_the_tables() {
        let d = module(&SIMPLE, false);
        assert!(detect(&d));
        let l = scan(&d).unwrap();
        assert_eq!((l.four, l.origin, l.pt, l.pattab, l.insttab, l.songtab), (false, 0, 0x200, 0x600, 0x230, 0x500));
        assert_eq!((l.nsongs, l.sh, l.nv, l.vs, l.esc, l.samp, l.silent), (1, 1, 4, 0, false, Samp::E1, 0x740));
        assert!(l.rel_pat && l.rel_pitch && l.rel_env && l.rel_init);
        // four decrement-and-test steps in the start routine: the pattern escape
        assert!(scan(&module(&SIMPLE, true)).unwrap().esc);
    }

    #[test]
    fn detection_is_the_branch_table_and_the_two_loads() {
        let d = module(&SIMPLE, false);
        for (o, v) in [(0usize, 0x6100u16), (2, 0), (6, 0x8002), (28, 0x6000)] {
            let mut e = d.clone();
            w16(&mut e, o, v);
            assert!(!detect(&e), "{o:#x}");
        }
        let mut e = d.clone();
        w16(&mut e, 0x40, 0x43F9);
        assert!(!detect(&e));
        assert!(!detect(&d[..0x100]));
        assert!(!detect(&[0u8; 0x400]));
        // a branch table whose start routine lacks the loads is not claimed
        let mut e = d.clone();
        w16(&mut e, 0x46, 0x41F9);
        assert!(!detect(&e));
    }

    #[test]
    fn builds_the_instrument_from_the_8svx_chunks() {
        let d = module(&SIMPLE, false);
        let l = scan(&d).unwrap();
        let rt = build(&d, &l);
        let o = l.insttab + 16;
        // start = the BODY data, length (8 + 8) / 2 words, loop after the one-shot part
        let body = 0x440 + 48;
        assert_eq!(be32(&rt, o + 2), VIRT + body as u32);
        assert_eq!(rw(&rt, o + 6), 8);
        assert_eq!(be32(&rt, o + 8), VIRT + body as u32 + 8);
        assert_eq!(rw(&rt, o + 12), 4);
    }

    fn run(d: Vec<u8>, ticks: usize) -> (Vec<(u32, u16)>, bool) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/jd-test/jd.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Special FX");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let end = e.songend();
        (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), end)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn a_note_starts_with_its_envelope_and_then_loops() {
        let (log, _) = run(module(&SIMPLE, false), 8);
        // the volume starts at the first envelope step (+8) and rises by 8 a tick
        assert_eq!(&reg(&log, 0xDFF0A8)[1..5], &[8, 16, 24, 32]);
        assert_eq!(reg(&log, 0xDFF0A6)[1], 0xE0);
        // tick 0: the full sample (16 bytes = 8 words), tick 1: the repeat part (4 words)
        assert_eq!(&reg(&log, 0xDFF0A4)[..2], &[8, 4]);
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1] - lc[0], 8);
        // the voice's DMA is stopped, then started at the end of the tick
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001) && dma.contains(&0x8001));
        // the other three voices idle: period 1, volume 0
        assert!(reg(&log, 0xDFF0B6).iter().all(|&p| p == 1));
        assert!(reg(&log, 0xDFF0B8).iter().all(|&p| p == 0));
    }

    #[test]
    fn the_song_ends_when_the_only_voice_jumps_to_its_loop() {
        // the note lasts 4 ticks, then the pattern ends and the order list's 0x80 is read
        let (_, end) = run(module(&SIMPLE, false), 3);
        assert!(!end);
        let (log, end) = run(module(&SIMPLE, false), 14);
        assert!(end);
        // the ended voice keeps writing period 1 and volume 0
        assert_eq!(*reg(&log, 0xDFF0A6).last().unwrap(), 1);
        assert_eq!(*reg(&log, 0xDFF0A8).last().unwrap(), 0);
    }

    #[test]
    fn a_rest_holds_volume_zero_and_starts_no_dma() {
        // a rest of 2 ticks (volume 0, no effects, no DMA start), then a note
        let pat = [0x90, 1, 0x8F, 0, 0x87, 2, 12, 4, 0x80, 0];
        let (log, _) = run(module(&pat, false), 10);
        let vol = reg(&log, 0xDFF0A8);
        // STOP, rest (tick 0), rest, then the note's first envelope step
        assert_eq!(&vol[1..4], &[0, 0, 8]);
        // only the note started DMA
        assert_eq!(reg(&log, 0xDFF096).iter().filter(|&&v| v == 0x0001).count(), 1);
    }

    #[test]
    fn a_period_index_beyond_the_table_reads_the_instrument_records() {
        // note 24 is index 24: the word after the table, the first half of record 0's
        // base/start; the engine does not clamp it
        let pat = [0x90, 1, 0x8F, 0, 24, 2, 0x80, 0];
        let d = module(&pat, false);
        let (log, _) = run(d.clone(), 6);
        let want = rw(&build(&d, &scan(&d).unwrap()), 0x200 + 48);
        assert_eq!(reg(&log, 0xDFF0A6)[1], want);
    }

    #[test]
    fn malformed_streams_cannot_hang_the_player() {
        // an order list that loops over only a loop marker, a pattern of commands only
        let pat = [0x85; 100];
        let (_, _) = run(module(&pat, false), 3);
        let mut d = module(&SIMPLE, false);
        d[0x500 + 0x40] = 0x81;
        d[0x500 + 0x41] = 0x80;
        let (_, _) = run(d, 3);
    }

    /// A harness, not a test: plays `JD_FILE` (subsong `JD_SUB`, 0-based) for `JD_SECS`
    /// seconds through the engine tick by tick and writes the ordered register writes
    /// to `JD_OUT` in the format of `DELI_WRITE_LOG`, with the number of ticks at which
    /// the song end was announced; `JD_NOEND` plays on past it.
    #[test]
    #[ignore]
    fn dump_run() {
        let (Ok(f), Ok(out)) = (std::env::var("JD_FILE"), std::env::var("JD_OUT")) else { return };
        let sub: usize = std::env::var("JD_SUB").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let secs: usize = std::env::var("JD_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
        let noend = std::env::var_os("JD_NOEND").is_some();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new(&f), std::fs::read(&f).unwrap()).unwrap();
        for _ in 0..sub {
            e.change_subsong(1).unwrap();
        }
        let mut end = None;
        let mut buf = vec![0f32; 2 * 960];
        for t in 0..secs * 50 {
            e.render(&mut buf);
            if end.is_none() && e.songend() {
                end = Some(t + 1);
                if !noend {
                    break;
                }
            }
        }
        let log = e.m.bus.write_log.take().unwrap();
        let head = e.m.deli.list.first().map(|(a, n)| format!("# module {a:06x} {n}\n")).unwrap_or_default();
        let mut text = head;
        for (a, v, t) in &log {
            text += &format!("{a:06x} {v:04x} {t}\n");
        }
        text += &format!("# songend {}\n", end.map_or(-1, |x| x as i64));
        std::fs::write(out, text).unwrap();
    }
}
