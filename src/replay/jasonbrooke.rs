//! Jason C. Brooke (`jcb.*`), the sound drivers of Jason Brooke's music for the US Gold
//! / Tiertex conversions of 1987-90 (1943, After Burner '89, Feud, Harley-Davidson,
//! Heavy Metal, Hellfire Attack, Ikari Warriors, Out Run, Pac-Land, Resolution 101,
//! Rockford, Side Arms, Space Harrier, Thunder Blade, Thunderstrike, Vixen, World Darts).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's JasonBrooke player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A `jcb` file is a raw memory image of the game's sound driver: 68k code with the
//! music data and samples behind it. The code is not run. It is read only to find the
//! driver's tables and variables, by short generic instruction signposts (the
//! PC-relative operands of a few loads), and to tell the code builds apart:
//!
//! * build B (families "B-28" and "B-34", by the channel-record stride) and build B2
//!   (Harley-Davidson, Resolution 101, Thunderstrike): one play routine per tick, four
//!   channel records, a two-step sample start (the sample, then its loop), envelopes
//!   of (level step, count) pairs, arpeggios, vibrato, slides, bends and a pulse-width
//!   generator that writes into one sample's data;
//! * build A (Feud, Pac-Land, World Darts): a four-voice synthesiser on 128-byte
//!   looped waveforms that rewrites period, volume and waveform pointer every tick.
//!
//! The driver's variables (the channel records and globals) live in the image, and
//! the port keeps them there, in its own copy, because the drivers read some of them
//! past their nominal ends: the build-B period table is read past its end, onto the
//! code behind it as the Eagleplayer patches it (its JSR/NOP rewrites of a few
//! instruction sequences, applied here at the offsets and with the values the
//! specification gives; the JSR stub address is the host's and reads as 0), and the
//! song-end test reads a byte 0x32 into each record, which in the B-28 layout is the
//! next record's.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Jason Brooke", detect, new };

/// Bytes read when following a pattern before a row is given up as never ending.
const GUARD: usize = 4096;

fn w(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

fn b(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

/// The first even offset at or after `from` where the words `pat` stand.
fn find(d: &[u8], from: usize, pat: &[u16]) -> Option<usize> {
    let mut o = from + (from & 1);
    while o + 2 * pat.len() <= d.len() {
        if pat.iter().enumerate().all(|(i, &p)| w(d, o + 2 * i) == p) {
            return Some(o);
        }
        o += 2;
    }
    None
}

/// The first even offset at or after `from` whose word satisfies `f`.
fn find_by(d: &[u8], from: usize, f: impl Fn(u16) -> bool) -> Option<usize> {
    let mut o = from + (from & 1);
    while o + 2 <= d.len() {
        if f(w(d, o)) {
            return Some(o);
        }
        o += 2;
    }
    None
}

/// The first `op` at or after `from` whose extension word's high byte is `hi` (for
/// `d8(pc,Xn)` / `d8(An,Xn)` operands: the index register and size).
fn find_ext(d: &[u8], from: usize, op: u16, hi: u16) -> Option<usize> {
    let mut o = from;
    loop {
        let a = find(d, o, &[op])?;
        if w(d, a + 2) >> 8 == hi {
            return Some(a);
        }
        o = a + 2;
    }
}

/// The last even offset before `before` whose word satisfies `f`.
fn find_back(d: &[u8], before: usize, f: impl Fn(u16) -> bool) -> Option<usize> {
    let mut o = before & !1;
    while o >= 2 {
        o -= 2;
        if f(w(d, o)) {
            return Some(o);
        }
    }
    None
}

/// The target of a `d16(pc)` operand whose extension word is at `at + 2`.
fn pc16(d: &[u8], at: usize) -> Option<usize> {
    let t = (at + 2) as i64 + w(d, at + 2) as i16 as i64;
    (0..d.len() as i64).contains(&t).then_some(t as usize)
}

/// The target of a `d8(pc,Xn)` operand whose extension word is at `at + 2`.
fn pc8(d: &[u8], at: usize) -> Option<usize> {
    let t = (at + 2) as i64 + b(d, at + 3) as i8 as i64;
    (0..d.len() as i64).contains(&t).then_some(t as usize)
}

/// A `lea d16(pc),An`.
fn is_lea(x: u16) -> bool {
    x & 0xF1FF == 0x41FA
}

// ---------------------------------------------------------------- recognition

/// Build A starts with this register save, clear and image-relative lea (specification 1.1).
const SIG_A: [u16; 6] = [0x48E7, 0xF0F0, 0x4240, 0x47FA, 0xFFF8, 0x4A2B];

fn is_a(d: &[u8]) -> bool {
    d.len() >= 0x40 && SIG_A.iter().enumerate().all(|(i, &v)| w(d, 2 * i) == v)
}

/// The build-B play routine `P` (specification 1.1).
fn play_b(d: &[u8]) -> Option<usize> {
    let p = (0..26).map(|i| 0xAC + 2 * i).find(|&o| w(d, o) == 0x48E7)?;
    let ok = matches!(w(d, p + 2), 0xF8FC | 0xF8F8)
        && be32(d, p + 4) == 0x08F9_0001
        && be32(d, p + 8) == 0x00BF_E001
        && be32(d, p + 12) == 0x33FC_0780
        && be32(d, p + 16) == 0x00DF_F09A
        && w(d, p + 20) == 0x47FA
        && (p + 22) as i64 + w(d, p + 22) as i16 as i64 == 0;
    ok.then_some(p)
}

fn detect(d: &[u8]) -> bool {
    if is_a(d) {
        layout_a(d).is_some()
    } else {
        layout_b(d).is_some()
    }
}

// --------------------------------------------------------------- build B, B2

/// A channel record's field offsets (specification 2.3, 8.2).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fields {
    envpos: usize,
    envnum: usize,
    envcnt: usize,
    envlvl: usize,
    vol: usize,
    rowcnt: usize,
    rowlen: usize,
    sldelay: usize,
    note: usize,
    vdep: usize,
    vspd: usize,
    vpos: usize,
    tr: usize,
    nextins: usize,
    /// Build B: the arpeggio start; B2: the arpeggio number.
    arp: usize,
    arppos: usize,
    ins: usize,
}

const FB: Fields = Fields {
    envpos: 0x0C,
    envnum: 0x0D,
    envcnt: 0x0E,
    envlvl: 0x0F,
    vol: 0x10,
    rowcnt: 0x11,
    rowlen: 0x12,
    sldelay: 0x13,
    note: 0x14,
    vdep: 0x15,
    vspd: 0x16,
    vpos: 0x17,
    tr: 0x18,
    nextins: 0x19,
    arp: 0x1A,
    arppos: 0x1B,
    ins: 0x1C,
};

const FB2: Fields = Fields {
    envpos: 0x0E,
    envnum: 0x0F,
    envcnt: 0x10,
    envlvl: 0x11,
    vol: 0x12,
    rowcnt: 0x13,
    rowlen: 0x14,
    sldelay: 0x15,
    note: 0x16,
    vdep: 0x17,
    vspd: 0x18,
    vpos: 0x19,
    tr: 0x1A,
    nextins: 0x1B,
    arp: 0x1C,
    arppos: 0x1D,
    ins: 0x1E,
};

/// B2's commands 55 and 56, told apart by their handler's first instruction.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Extra {
    /// `move.b (a1)+,$1c(a0)`: the arpeggio number.
    Arp,
    /// `move.b (a1)+,g+5(a3)`: the tempo.
    Tempo,
    None,
}

#[derive(Clone, Debug, PartialEq)]
struct LayB {
    b2: bool,
    p: usize,
    t: usize,
    songs: usize,
    per: usize,
    ins: usize,
    sil_data: usize,
    sil_len: usize,
    env: usize,
    arp: usize,
    arpidx: usize,
    envidx: usize,
    envvol: usize,
    pwm: usize,
    pwmlen: usize,
    st: usize,
    thr: u8,
    gtr: Option<usize>,
    dma: usize,
    g: usize,
    clears: Vec<usize>,
    nclear: usize,
    /// B2: the arpeggio offset is added to the note (else computed and discarded).
    arp_used: bool,
    /// B2: the byte command 4C sets to 1 (the envelope number or the next instrument).
    pwm_field: usize,
    extra: [Extra; 2],
}

/// Finds the driver's tables (specification 2.1, 2.3, 1.3, 8.1).
fn layout_b(d: &[u8]) -> Option<LayB> {
    let p = play_b(d)?;
    let b2 = w(d, p + 0x18) == 0x383C && w(d, p + 0x1A) == 0x002C;
    // the stop and start routines
    let x = find(d, p, &[0x49F9])?;
    let stop = find(d, x + 8, &[0x7600])? + 2;
    let z = find(d, stop, &[0x1032, 0x0000])?;
    let start = find(d, z + 4, &[0x48E7])?.checked_sub(4)?;
    let rts = find(d, start, &[0x4E75])?;
    let lt = find(d, start, &[0x43FA])?;
    let t = pc16(d, lt)?;
    // tables
    let o = find_ext(d, p, 0x323B, 0x00)?;
    let per = pc8(d, o)?;
    let ci = find(d, p, &[0xC2FC, 0x000A])?;
    let li = find(d, ci + 4, &[0x45FA])?;
    let ins = pc16(d, li)?;
    let ls = find_by(d, li + 4, is_lea)?;
    let sil_data = pc16(d, ls)?;
    let ll = find_by(d, ls + 4, is_lea)?;
    let sil_len = pc16(d, ll)?;
    let ze = find(d, z, &[0x1032, 0x0000])?;
    let le = find_back(d, ze, |x| x == 0x45FA)?;
    let envidx = pc16(d, le)?;
    // `move.b dd(a2,d0.w),d1`, right after the `move.b (a2,d0.w),d0`
    let ev = find(d, ze + 4, &[0x1232]).filter(|&o| o == ze + 4)?;
    let envvol = envidx + b(d, ev + 3) as usize;
    let lp = find(d, p, &[0x41FA])?;
    let pwm = pc16(d, lp)?;
    let pl = find(d, p, &[0x3740])?;
    let pwmlen = w(d, pl + 2) as usize;
    let sl = find(d, p, &[0x383C])?;
    let st = w(d, sl + 2) as usize;
    let tl = find(d, p, &[0x143C])?;
    let thr = b(d, tl + 3);
    let dl = find(d, p, &[0x117C, 0x0002])?;
    if w(d, dl + 4) >= 0x100 {
        return None;
    }
    let dma = w(d, dl + 4) as usize;
    let (env, arp, arpidx, gtr);
    if b2 {
        let ev = find(d, p, &[0x4A28, 0x0010])?;
        env = pc16(d, find_back(d, ev, |x| x == 0x43FA)?)?;
        let a1 = find_ext(d, p, 0x1231, 0x10)?;
        arpidx = (env as i64 + b(d, a1 + 3) as i8 as i64) as usize;
        let a2 = find_ext(d, a1 + 4, 0x1031, 0x10)?;
        arp = (env as i64 + b(d, a2 + 3) as i8 as i64) as usize;
        let mode = w(d, a1 + 28);
        if mode != 0xD028 && mode != 0x1028 {
            return None;
        }
        let g = find(d, p, &[0xD028, 0x001A])?;
        gtr = (w(d, g + 4) == 0xD03A).then(|| pc16(d, g + 4)).flatten();
        let lay = LayB {
            b2,
            p,
            t,
            songs: 0,
            per,
            ins,
            sil_data,
            sil_len,
            env,
            arp,
            arpidx,
            envidx,
            envvol,
            pwm,
            pwmlen,
            st,
            thr,
            gtr,
            dma,
            g: 4 * st,
            clears: Vec::new(),
            nclear: 0,
            arp_used: mode == 0xD028,
            pwm_field: 0,
            extra: [Extra::None; 2],
        };
        return finish_b(d, lay, start, rts);
    }
    let ev = find(d, p, &[0x4A28, 0x000E])?;
    env = pc16(d, find_back(d, ev, |x| x == 0x43FA)?)?;
    let a1 = find_ext(d, p, 0x1031, 0x10)?;
    arp = (env as i64 + b(d, a1 + 3) as i8 as i64) as usize;
    let ai = find(d, p, &[0x1172, 0x0000])?;
    arpidx = pc16(d, find_back(d, ai, is_lea)?)?;
    gtr = find(d, p, &[0xD028, 0x0018]).filter(|&g| w(d, g + 4) == 0xD03A).and_then(|g| pc16(d, g + 4));
    let lay = LayB {
        b2,
        p,
        t,
        songs: 0,
        per,
        ins,
        sil_data,
        sil_len,
        env,
        arp,
        arpidx,
        envidx,
        envvol,
        pwm,
        pwmlen,
        st,
        thr,
        gtr,
        dma,
        g: 4 * st,
        clears: Vec::new(),
        nclear: 0,
        arp_used: true,
        pwm_field: FB.nextins,
        extra: [Extra::None; 2],
    };
    finish_b(d, lay, start, rts)
}

/// The start routine's clears, the song count and B2's command variants.
fn finish_b(d: &[u8], mut lay: LayB, start: usize, rts: usize) -> Option<LayB> {
    let mut o = start;
    while let Some(c) = find(d, o, &[0x422B]).filter(|&c| c < rts) {
        lay.clears.push(w(d, c + 2) as usize);
        o = c + 4;
    }
    let mq = find_by(d, start, |x| x & 0xFF00 == 0x7200).filter(|&m| m < rts)?;
    lay.nclear = 2 * (b(d, mq + 1) as usize + 1);
    if !matches!(lay.st, 0x28 | 0x2C | 0x34) || lay.dma + 2 >= lay.st || lay.nclear > lay.st || lay.t == 0 {
        return None;
    }
    // song count (1.3)
    let t1 = lay.t - 1;
    let cap = lay.envidx.checked_sub(t1)? / 18;
    let mut n = 1;
    loop {
        let x = w(d, t1 + 18 * n);
        if x >> 8 != 0 || x == 1 || t1 + 18 * n + 2 > d.len() {
            break;
        }
        n += 1;
    }
    lay.songs = n.min(cap);
    if lay.songs == 0 {
        return None;
    }
    if lay.b2 {
        let cmd = cmd_table(d, lay.p)?;
        let h = |c: usize| w(d, cmd + 2 * c) as usize;
        // command 4C: the `move.b d0,xx(a0)` storing its 1
        let h4c = h(0x4C - 0x41);
        let s = find(d, h4c, &[0x1140]).filter(|&s| s < h4c + 0x20)?;
        lay.pwm_field = w(d, s + 2) as usize;
        for (k, c) in [0x55, 0x56].into_iter().enumerate() {
            let a = h(c - 0x41);
            lay.extra[k] = match (w(d, a), w(d, a + 2) as usize) {
                (0x1159, x) if x == FB2.arp => Extra::Arp,
                (0x1759, x) if x == lay.g + 5 => Extra::Tempo,
                _ => Extra::None,
            };
        }
    }
    Some(lay)
}

fn cmd_table(d: &[u8], p: usize) -> Option<usize> {
    let c = find_ext(d, p, 0xD4FB, 0x00)?;
    pc8(d, c)
}

/// The Eagleplayer's code rewrites (specification 2.2): each sequence, wherever it
/// stands word-aligned in `[P, P+0x578)`, becomes a JSR absolute to a host stub
/// (whose address is the host's; 0 here) padded with NOPs. Only the bytes matter to
/// the port: the period table is read past its end onto them.
fn patch(img: &mut [u8], p: usize) {
    const SEQS: [&[u16]; 10] = [
        &[0x5201, 0x1941, 0x0009],
        &[0x4841, 0x1941, 0x0009],
        &[0x3968, 0x0020, 0x0006],
        &[0x0810, 0x0002, 0x6718, 0x4A28, 0x0013, 0x660E, 0x3428, 0x0008, 0xD568, 0x000A, 0xD268, 0x000A, 0x6004, 0x5328, 0x0013, 0x3941, 0x0006],
        &[0x3941, 0x0004, 0x2889],
        &[0x3968, 0x0022, 0x0006],
        &[0x3941, 0x0004, 0x288D],
        &[0x3968, 0x001E, 0x0006],
        &[0xB028, 0x0006, 0x6604, 0x1028, 0x0007, 0x3228, 0x0004],
        &[0x33FC, 0x00FF, 0x00DF, 0xF09E, 0x33FC, 0x000F, 0x00DF, 0xF096],
    ];
    let end = (p + 0x578).min(img.len());
    for seq in SEQS {
        let mut o = p;
        while o + 2 * seq.len() <= end {
            if seq.iter().enumerate().all(|(i, &v)| w(img, o + 2 * i) == v) {
                let mut r = vec![0x4EB9, 0, 0];
                r.resize(seq.len(), 0x4E71);
                for (i, v) in r.into_iter().enumerate() {
                    img[o + 2 * i..o + 2 * i + 2].copy_from_slice(&u16::to_be_bytes(v));
                }
                o += 2 * seq.len();
            } else {
                o += 2;
            }
        }
    }
}

/// What a pattern command does (specification 4.3, 8.5).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cmd {
    Rest,
    FlagsClear,
    Slide,
    BendDown,
    BendUp,
    Next,
    Vibrato,
    End,
    Transpose,
    Adkcon,
    NoRetrig,
    Pwm,
    Volume,
    NoEnv,
    Both,
    Store,
    GlobalTr,
    RepStart,
    RepTwice,
    RepEnd,
    Arp,
    Tempo,
    Ignore,
}

const CMDS_B: [Cmd; 17] = [
    Cmd::Rest,
    Cmd::FlagsClear,
    Cmd::Slide,
    Cmd::BendDown,
    Cmd::BendUp,
    Cmd::Next,
    Cmd::Vibrato,
    Cmd::End,
    Cmd::Transpose,
    Cmd::Adkcon,
    Cmd::NoRetrig,
    Cmd::Pwm,
    Cmd::Volume,
    Cmd::NoEnv,
    Cmd::Both,
    Cmd::Store,
    Cmd::GlobalTr,
];

const CMDS_B2: [Cmd; 20] = [
    Cmd::Rest,
    Cmd::FlagsClear,
    Cmd::Slide,
    Cmd::BendDown,
    Cmd::BendUp,
    Cmd::Next,
    Cmd::Vibrato,
    Cmd::End,
    Cmd::Transpose,
    Cmd::Adkcon,
    Cmd::NoRetrig,
    Cmd::Pwm,
    Cmd::Volume,
    Cmd::NoEnv,
    Cmd::Both,
    Cmd::Store,
    Cmd::GlobalTr,
    Cmd::RepStart,
    Cmd::RepTwice,
    Cmd::RepEnd,
];

pub struct BuildB {
    lay: LayB,
    f: Fields,
    /// The patched image, with the driver's variables.
    img: Vec<u8>,
    pristine: Vec<u8>,
    base: u32,
    sub: u16,
    started: bool,
    songend: bool,
    /// The channels that have not yet wrapped their sequence list (song-end hook).
    unwrapped: u8,
}

impl BuildB {
    fn rec(&self, ch: usize) -> usize {
        self.lay.st * ch
    }
    fn r(&self, ch: usize, f: usize) -> u8 {
        b(&self.img, self.rec(ch) + f)
    }
    fn set(&mut self, ch: usize, f: usize, v: u8) {
        let o = self.rec(ch) + f;
        if let Some(x) = self.img.get_mut(o) {
            *x = v;
        }
    }
    fn rw(&self, ch: usize, f: usize) -> u16 {
        w(&self.img, self.rec(ch) + f)
    }
    fn setw(&mut self, ch: usize, f: usize, v: u16) {
        self.setw_at(self.rec(ch) + f, v);
    }
    fn setw_at(&mut self, o: usize, v: u16) {
        if let Some(x) = self.img.get_mut(o..o + 2) {
            x.copy_from_slice(&v.to_be_bytes());
        }
    }
    fn g(&self, k: usize) -> u8 {
        b(&self.img, self.lay.g + k)
    }
    fn setg(&mut self, k: usize, v: u8) {
        self.set_at(self.lay.g + k, v);
    }
    fn set_at(&mut self, o: usize, v: u8) {
        if let Some(x) = self.img.get_mut(o) {
            *x = v;
        }
    }
    fn bit(&self, ch: usize) -> u16 {
        self.r(ch, self.lay.dma + 2) as u16
    }

    /// 5.2, 8.6.
    fn stop(&mut self, hw: &mut Hw) {
        self.setg(0, 0);
        for ch in 0..4 {
            self.set(ch, self.lay.dma - 1, 0);
        }
        hw.bus.write_word(0xDFF09E, 0x00FF);
        hw.dmacon(0x000F);
    }

    /// 5.1, 8.6.
    fn start(&mut self, s: usize, hw: &mut Hw) {
        self.stop(hw);
        for ch in 0..4 {
            for k in 0..self.lay.nclear {
                self.set(ch, k, 0);
            }
        }
        for c in self.lay.clears.clone() {
            self.set_at(c, 0);
        }
        self.setg(4, 0xFF);
        self.setg(6, 0xFF);
        let e = self.lay.t + 18 * s;
        self.setg(5, b(&self.img, e));
        for ch in 0..4 {
            let blk = e + 1 + 4 * ch;
            self.set(ch, 6, b(&self.img, blk));
            self.set(ch, 7, b(&self.img, blk + 1));
            let seq = w(&self.img, blk + 2);
            self.setw(ch, 4, seq);
            self.setw(ch, 2, w(&self.img, seq as usize));
            self.set(ch, self.f.vol, 0xFF);
            self.set(ch, self.f.rowcnt, 1);
        }
        self.setg(0, 0xFF);
        self.unwrapped = 0xF;
    }

    /// The tick (3.1, 8.3).
    fn tick_b(&mut self, hw: &mut Hw) {
        hw.bus.write_word(0xDFF09A, 0x0780);
        let (s, c) = self.g(4).overflowing_add(self.g(3));
        self.setg(4, s);
        if c {
            return;
        }
        // the pulse-width generator
        let pos = self.g(7);
        let lim = if self.g(0xA) != 0 { self.g(9) } else { self.g(8) };
        if pos != lim {
            let (v, np) = if pos < lim { (0x80, pos.wrapping_add(1)) } else { (0, pos.wrapping_sub(1)) };
            let o = self.lay.pwm + pos as usize;
            self.set_at(o, v);
            hw.bus.write_byte(self.base.wrapping_add(o as u32), v);
            self.setg(7, np);
        } else {
            self.setg(0xA, self.g(0xA) ^ 0xFF);
        }
        for ch in 0..4 {
            self.output(ch, hw);
        }
        if self.g(0) != 0 {
            let (s, c) = self.g(6).overflowing_add(self.g(5));
            self.setg(6, s);
            if c {
                for ch in (0..4).rev() {
                    if !self.step(ch, hw) {
                        break;
                    }
                }
            }
        }
    }

    /// The channel output (3.2, 8.3).
    fn output(&mut self, ch: usize, hw: &mut Hw) {
        let f = self.f;
        if self.r(ch, self.lay.dma - 1) != 0 {
            // a sound effect: never started by the music
            return;
        }
        if self.g(0) == 0 {
            hw.vol(ch, 0);
            hw.dmacon(self.bit(ch));
            return;
        }
        // the sample start (3.3)
        let e = self.lay.ins + 10 * self.r(ch, f.ins) as usize;
        let tun = w(&self.img, e);
        let pend = self.r(ch, self.lay.dma);
        if pend != 0 {
            let mut lc = be32(&self.img, e + 2);
            let mut len = w(&self.img, e + 6);
            let mut skip = 0u16;
            self.set(ch, self.lay.dma, pend - 1);
            if pend == 1 {
                skip = w(&self.img, e + 8);
                if skip & 0x8000 != 0 {
                    lc = self.lay.sil_data as u32;
                    len = w(&self.img, self.lay.sil_len);
                    skip = 0;
                }
            }
            lc = lc.wrapping_add(skip as i16 as i32 as u32);
            hw.len(ch, len.wrapping_sub(skip) >> 1);
            hw.loc(ch, self.base.wrapping_add(lc));
            hw.dmacon(0x8200 | self.bit(ch));
        }
        // the volume
        let v = ((self.r(ch, f.envlvl) >> 2) as u32 * self.g(1) as u32) & 0xFFFF;
        let v = (v * self.r(ch, f.vol) as u32) >> 16;
        hw.vol(ch, (v & 0x7F) as u16);
        // the envelope
        let cnt = self.r(ch, f.envcnt);
        if cnt != 0 {
            let mut c = self.r(ch, f.envpos);
            self.set(ch, f.envcnt, cnt - 1);
            let mut add = true;
            if cnt == 1 {
                let n = b(&self.img, self.lay.env + c as usize + 1);
                self.set(ch, f.envcnt, n);
                if n == 0 {
                    add = false;
                } else {
                    c = c.wrapping_add(2);
                    self.set(ch, f.envpos, c);
                }
            }
            if add {
                let l = self.r(ch, f.envlvl).wrapping_add(b(&self.img, self.lay.env + c as usize));
                self.set(ch, f.envlvl, l);
            }
        }
        // the arpeggio
        let off;
        if self.lay.b2 {
            let k = b(&self.img, self.lay.arpidx + self.r(ch, f.arp) as usize).wrapping_add(self.r(ch, f.arppos));
            let a = b(&self.img, self.lay.arp + k as usize);
            if a < 0x80 {
                off = a;
                self.set(ch, f.arppos, 0);
            } else if a == 0xFF {
                off = 0;
            } else {
                off = a;
                self.set(ch, f.arppos, self.r(ch, f.arppos).wrapping_add(1));
            }
        } else {
            let k = self.r(ch, f.arppos).wrapping_add(1);
            let a = b(&self.img, self.lay.arp + k as usize);
            if a < 0x80 {
                self.set(ch, f.arppos, k);
                off = a;
            } else if a == 0xFF {
                off = 0;
            } else {
                off = a - 0x80;
                self.set(ch, f.arppos, self.r(ch, f.arp));
            }
        }
        // the period
        let mut n = self.r(ch, f.note).wrapping_add(self.r(ch, f.tr));
        if let Some(g) = self.lay.gtr {
            n = n.wrapping_add(b(&self.img, g));
        }
        if self.lay.arp_used {
            n = n.wrapping_add(off);
        }
        let i = n.wrapping_mul(2) as usize;
        let pv = w(&self.img, self.lay.per + i) as u32;
        let mut per = ((pv * tun as u32) << 3 >> 16) as u16;
        // the vibrato
        let flags = self.r(ch, 0);
        if flags & 0x40 != 0 {
            let mut s = self.r(ch, f.vspd);
            if s == 0 {
                s = flags & 1;
            }
            let d = self.r(ch, f.vdep);
            if flags & 0x10 != 0 {
                let x = self.r(ch, f.vpos).wrapping_add(s);
                self.set(ch, f.vpos, x);
                if x == d {
                    self.set(ch, 0, self.r(ch, 0) ^ 0x10);
                }
                s = x;
            } else {
                let x = self.r(ch, f.vpos).wrapping_sub(s);
                self.set(ch, f.vpos, x);
                if x == 0 {
                    self.set(ch, 0, self.r(ch, 0) ^ 0x10);
                }
            }
            let t = s as i16 - (d >> 1) as i16;
            per = per.wrapping_add(t as u16);
        }
        self.set(ch, 0, self.r(ch, 0) ^ 1);
        // the slide
        if self.r(ch, 0) & 4 != 0 {
            let dl = self.r(ch, f.sldelay);
            if dl != 0 {
                self.set(ch, f.sldelay, dl - 1);
            } else {
                let acc = self.rw(ch, 0x0A).wrapping_add(self.rw(ch, 0x08));
                self.setw(ch, 0x0A, acc);
                per = per.wrapping_add(acc);
            }
        }
        hw.per(ch, per);
    }

    /// The sequence wrap's song-end hook (5.3).
    fn wrapped(&mut self, ch: usize) {
        if matches!(b(&self.img, self.rec(ch) + 0x32), 0xA0 | 0xB0 | 0xC0 | 0xD0) {
            self.unwrapped &= !(1 << ch);
            if self.unwrapped == 0 {
                self.songend = true;
                self.unwrapped = 0xF;
            }
        }
    }

    fn cmd(&self, x: u8) -> Cmd {
        if self.lay.b2 {
            let k = (x - 0x41) as usize;
            match k {
                0..=19 => CMDS_B2[k],
                20 | 21 => match self.lay.extra[k - 20] {
                    Extra::Arp => Cmd::Arp,
                    Extra::Tempo => Cmd::Tempo,
                    Extra::None => Cmd::Ignore,
                },
                _ => Cmd::Ignore,
            }
        } else {
            let k = (x - 0x80) as usize;
            match k {
                0..=14 => CMDS_B[k],
                15 => Cmd::Store,
                16 if self.lay.gtr.is_some() => Cmd::GlobalTr,
                _ => Cmd::Ignore,
            }
        }
    }

    /// The sequencer step of channel `ch` (4, 8.5); false when the end command ran.
    fn step(&mut self, ch: usize, hw: &mut Hw) -> bool {
        let f = self.f;
        let rc = self.r(ch, f.rowcnt).wrapping_sub(1);
        self.set(ch, f.rowcnt, rc);
        if rc != 0 {
            let fl = self.r(ch, 0);
            if fl & 8 != 0 {
                let n = self.r(ch, f.note);
                self.set(ch, f.note, if fl & 0x80 != 0 { n.wrapping_sub(1) } else { n.wrapping_add(1) });
            }
            return true;
        }
        self.set(ch, 0, self.r(ch, 0) & 0x50);
        let mut pp = self.rw(ch, 2) as usize;
        let sfx = self.r(ch, self.lay.dma - 1) != 0;
        let rd = |s: &Self, pp: &mut usize| {
            let v = b(&s.img, *pp);
            *pp = (*pp + 1) & 0xFFFF;
            v
        };
        for _ in 0..GUARD {
            let x = rd(self, &mut pp);
            let note = if self.lay.b2 {
                match x {
                    0x00..=0x40 => {
                        self.set(ch, f.rowlen, x);
                        continue;
                    }
                    0x60..=0x7F => {
                        self.set(ch, f.envnum, x - 0x60);
                        continue;
                    }
                    0xE0..=0xFF => {
                        self.set(ch, f.nextins, x - 0xE0);
                        continue;
                    }
                    0x80..=0xDF => Some(x - 0x80),
                    _ => None,
                }
            } else if x >= self.lay.thr {
                match x {
                    0xE0..=0xFF => self.set(ch, f.rowlen, x - 0xE0 + 1),
                    0xC0..=0xDF => self.set(ch, f.envnum, x - 0xC0),
                    0xA0..=0xBF => self.set(ch, f.nextins, x - 0xA0),
                    _ => {
                        let a = b(&self.img, self.lay.arpidx + x.wrapping_add(0x60) as usize);
                        self.set(ch, f.arp, a);
                    }
                }
                continue;
            } else if x < 0x80 {
                Some(x)
            } else {
                None
            };
            if let Some(n) = note {
                if !sfx {
                    self.set(ch, f.note, n);
                    if self.r(ch, 0) & 2 == 0 {
                        let c = b(&self.img, self.lay.envidx + self.r(ch, f.envnum) as usize);
                        self.set(ch, f.envpos, c);
                        self.set(ch, f.envlvl, b(&self.img, self.lay.envvol + c as usize));
                        self.set(ch, f.envcnt, 1);
                    }
                    if self.lay.b2 {
                        self.set(ch, f.arppos, 0);
                    } else {
                        self.set(ch, f.arppos, self.r(ch, f.arp));
                    }
                    self.set(ch, f.ins, self.r(ch, f.nextins));
                    if self.r(ch, 0) & 0x20 == 0 {
                        self.set(ch, self.lay.dma, 2);
                        hw.dmacon(self.bit(ch));
                    }
                }
                break;
            }
            match self.cmd(x) {
                Cmd::Rest => {
                    if !sfx {
                        self.set(ch, f.ins, 0);
                        self.set(ch, self.lay.dma, 2);
                        hw.dmacon(self.bit(ch));
                    }
                    break;
                }
                Cmd::FlagsClear => self.set(ch, 0, 0),
                Cmd::Slide => {
                    let v = (rd(self, &mut pp) as u16) << 8 | rd(self, &mut pp) as u16;
                    self.setw(ch, 8, v);
                    self.setw(ch, 0x0A, 0);
                    self.set(ch, 0, self.r(ch, 0) | 4);
                    let dl = rd(self, &mut pp);
                    self.set(ch, f.sldelay, dl);
                }
                Cmd::BendDown => self.set(ch, 0, self.r(ch, 0) | 0x88),
                Cmd::BendUp => self.set(ch, 0, self.r(ch, 0) | 0x08),
                Cmd::Next => {
                    let mut k = self.r(ch, 1).wrapping_add(2);
                    if k == self.r(ch, 6) {
                        k = self.r(ch, 7);
                        self.wrapped(ch);
                    }
                    let l = self.rw(ch, 4) as usize + k as usize;
                    pp = w(&self.img, l) as usize;
                    self.set(ch, 1, k);
                }
                Cmd::Vibrato => {
                    let s = rd(self, &mut pp);
                    let d = rd(self, &mut pp);
                    self.set(ch, f.vspd, s);
                    self.set(ch, f.vpos, d);
                    self.set(ch, f.vdep, d.wrapping_mul(2));
                    self.set(ch, 0, self.r(ch, 0) | 0x40);
                }
                Cmd::End => {
                    self.stop(hw);
                    self.songend = true;
                    return false;
                }
                Cmd::Transpose => {
                    let t = rd(self, &mut pp);
                    self.set(ch, f.tr, t);
                }
                Cmd::Adkcon => {
                    let v = rd(self, &mut pp) as i8 as i16 as u16;
                    hw.bus.write_word(0xDFF09E, v & 0x807F);
                }
                Cmd::NoRetrig => self.set(ch, 0, self.r(ch, 0) | 0x20),
                Cmd::Pwm => {
                    let up = rd(self, &mut pp);
                    let lo = rd(self, &mut pp);
                    let ln = rd(self, &mut pp);
                    self.setg(9, up);
                    self.setg(8, lo);
                    self.setw_at(self.lay.pwmlen, ln as u16 + 1);
                    self.set(ch, self.lay.pwm_field, 1);
                }
                Cmd::Volume => {
                    let v = rd(self, &mut pp);
                    self.set(ch, f.vol, v);
                }
                Cmd::NoEnv => self.set(ch, 0, self.r(ch, 0) | 0x02),
                Cmd::Both => self.set(ch, 0, self.r(ch, 0) | 0x22),
                Cmd::Store => {
                    let v = rd(self, &mut pp);
                    self.setg(0xB, v);
                }
                Cmd::GlobalTr => {
                    let v = rd(self, &mut pp);
                    if let Some(g) = self.lay.gtr {
                        self.set_at(g, v);
                    }
                }
                Cmd::RepStart => {
                    let n = rd(self, &mut pp);
                    self.set(ch, 0x20, n);
                    self.setw(ch, 0x0C, pp as u16);
                }
                Cmd::RepTwice => {
                    self.set(ch, 0x20, 2);
                    self.setw(ch, 0x0C, pp as u16);
                }
                Cmd::RepEnd => {
                    let n = self.r(ch, 0x20).wrapping_sub(1);
                    self.set(ch, 0x20, n);
                    if n != 0 {
                        pp = self.rw(ch, 0x0C) as usize;
                    }
                }
                Cmd::Arp => {
                    let a = rd(self, &mut pp);
                    self.set(ch, f.arp, a);
                }
                Cmd::Tempo => {
                    let t = rd(self, &mut pp);
                    self.setg(5, t);
                }
                Cmd::Ignore => {}
            }
        }
        let rl = self.r(ch, f.rowlen);
        self.set(ch, f.rowcnt, rl);
        self.setw(ch, 2, pp as u16);
        true
    }
}

/// Colour clocks from the song's start to the host's timer start, beyond the first period (measured
/// in the reference runs).
const TIMER_DELAY: u64 = 188;

impl Replayer for BuildB {
    fn format(&self) -> &'static str {
        "Jason Brooke"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn timer_delay(&self) -> u64 {
        TIMER_DELAY
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.songs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (n as usize) < self.lay.songs {
            self.sub = n;
            self.started = false;
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        // every play routine of build B opens with bset.b #1,$bfe001: the LED filter off
        hw.bus.ciaa.pra |= 2;
        if !self.started {
            // a fresh image, as the host loads the module for every song
            self.img.copy_from_slice(&self.pristine);
            let (o, e) = (self.lay.pwm.min(self.img.len()), (self.lay.pwm + 0x100).min(self.img.len()));
            hw.bus.write_bytes(self.base + o as u32, &self.img[o..e]);
            self.started = true;
            self.songend = false;
            self.start(self.sub as usize, hw);
        }
        self.tick_b(hw);
    }
}

// --------------------------------------------------------------------- build A

#[derive(Clone, Debug, PartialEq)]
struct LayA {
    pl: usize,
    /// Pac-Land: the skip flag, counter and reload.
    skip: Option<(usize, usize, u8)>,
    cnt: usize,
    tmp: usize,
    rec: usize,
    st: usize,
    arp: usize,
    env: usize,
    envidx: usize,
    per: usize,
    aib: usize,
    cmd: usize,
    wave: usize,
    wf: usize,
    song: usize,
    songs: usize,
    vol_first: bool,
    end_output: bool,
    idle_output: bool,
    /// The envelope reads ENV + start + a separate 16-bit index (Pac-Land).
    env_d2: bool,
    gv: Option<usize>,
    half_speed: bool,
    wave_copy: bool,
    form50: bool,
    use_sub: bool,
}

/// Finds build A's tables (specification 9.1).
fn layout_a(d: &[u8]) -> Option<LayA> {
    if !is_a(d) {
        return None;
    }
    let pl = w(d, 0x0C) as usize;
    let skip = if w(d, 0x10) == 0x4A2B && w(d, 0x16) == 0x532B && w(d, 0x1C) == 0x177C {
        Some((w(d, 0x12) as usize, w(d, 0x18) as usize, b(d, 0x1F)))
    } else {
        None
    };
    let from = if skip.is_some() { 0x22 } else { 0x0E };
    let c = find(d, from, &[0x532B])?;
    let cnt = w(d, c + 2) as usize;
    let tl = find(d, c + 4, &[0x177A])?;
    let tmp = pc16(d, tl)?;
    let rl = find(d, tl + 6, &[0x41FA])?;
    let rec = pc16(d, rl)?;
    let sl = find(d, rl + 4, &[0xD0FC])?;
    let st = w(d, sl + 2) as usize;
    let al = find(d, tl + 6, &[0x43FA])?;
    let arp = pc16(d, al)?;
    let ol = find(d, 0, &[0x43F9, 0x00DF, 0xF0A0])?;
    let vol_first = find(d, ol + 6, &[0x1368, 0x000D, 0x0009]) == Some(ol + 6);
    let wl = find(d, ol + 6, &[0x45FA])?;
    let wave = pc16(d, wl)?;
    let restore = |t: usize| w(d, t) == 0x4CDF;
    let el = find(d, 0, &[0x2E88])?;
    let end_output = !(w(d, el - 4) == 0x41FA && pc16(d, el - 4).is_some_and(restore));
    let idle_output = !(b(d, 0x0E) == 0x67 && restore((0x10 + b(d, 0x0F) as i8 as isize) as usize));
    let pb = find(d, 0, &[0xB03C, 0x0090])?;
    let ai = find_ext(d, pb, 0x117B, 0x00)?;
    if w(d, ai + 4) != 0x0016 {
        return None;
    }
    let aib = pc8(d, ai)?;
    let cl = find_ext(d, pb, 0xD4FB, 0x00)?;
    let cmd = pc8(d, cl)?;
    let a18 = find(d, pb, &[0x0600, 0x0018])?;
    let bc = find_by(d, a18 + 4, |x| x >> 8 == 0x65)?;
    let bt = (bc + 2) as isize + b(d, bc + 1) as i8 as isize;
    let wfl = find(d, bt as usize, &[0x1140]).filter(|&o| o < bt as usize + 8)?;
    let wf = w(d, wfl + 2) as usize;
    let u = find(d, 0, &[0x0C28, 0x00C0, 0x000F])?;
    let ei = find_ext(d, u, 0x103B, 0x00)?;
    let envidx = pc8(d, ei)?;
    let env_d2 = find(d, u, &[0x1428, 0x000E]).is_some_and(|o| o < u + 0x20);
    let env = if env_d2 {
        pc16(d, find(d, u, &[0x45FA])?)?
    } else {
        let o = find(d, u, &[0x1171]).filter(|&o| w(d, o + 4) == 0x000F)?;
        (arp as isize + b(d, o + 3) as i8 as isize) as usize
    };
    let gv = find(d, 0, &[0x0200, 0x003F]).and_then(|m| {
        let g = find(d, m + 4, &[0x123A]).filter(|&g| g < m + 4 + 10)?;
        pc16(d, g)
    });
    let pr = find_ext(d, u, 0x323B, 0x00)?;
    let per = pc8(d, pr)?;
    let half_speed = find(d, u, &[0x0810, 0x0006]).is_some_and(|o| w(d, o + 0x10) == 0x0810 && w(d, o + 0x12) == 0x0004);
    let wave_copy = find(d, u, &[0x1168, 0x001A, 0x0015]).is_some();
    let form50 = find(d, 0, &[0x10BC, 0x0050]).is_some_and(|o| o < u);
    let s7 = find(d, u, &[0x137B])?;
    let song = pc8(d, s7)?.checked_sub(1)?;
    let st_start = find_back(d, s7, |x| x == 0x48E7)?;
    let use_sub = find(d, st_start, &[0xC0FC, 0x000A]).is_some_and(|o| o < s7);
    let mut songs = 1;
    loop {
        let x = w(d, song + 10 * songs);
        if x == 0 || x >> 8 != 0 || song + 10 * songs + 10 > d.len() {
            break;
        }
        songs += 1;
    }
    if !matches!(st, 0x1C | 0x32) || wf >= st || rec + 4 * st > d.len() {
        return None;
    }
    Some(LayA {
        pl,
        skip,
        cnt,
        tmp,
        rec,
        st,
        arp,
        env,
        envidx,
        per,
        aib,
        cmd,
        wave,
        wf,
        song,
        songs,
        vol_first,
        end_output,
        idle_output,
        env_d2,
        gv,
        half_speed,
        wave_copy,
        form50,
        use_sub,
    })
}

/// Pac-Land's envelope index high byte at the start of a tick: the D2 that UADE's
/// score leaves in the interrupted registers, INTENAR (0x4020) after InitSound
/// (specification 9.6). Its envelope reads then land past the image, which reads as 0.
const PACLAND_H: u8 = 0x40;

pub struct BuildA {
    lay: LayA,
    img: Vec<u8>,
    pristine: Vec<u8>,
    base: u32,
    sub: u16,
    started: bool,
    songend: bool,
    unwrapped: u8,
}

impl BuildA {
    fn rec(&self, ch: usize) -> usize {
        self.lay.rec + self.lay.st * ch
    }
    fn r(&self, ch: usize, f: usize) -> u8 {
        b(&self.img, self.rec(ch) + f)
    }
    fn set(&mut self, ch: usize, f: usize, v: u8) {
        self.set_at(self.rec(ch) + f, v);
    }
    fn set_at(&mut self, o: usize, v: u8) {
        if let Some(x) = self.img.get_mut(o) {
            *x = v;
        }
    }
    fn rw(&self, ch: usize, f: usize) -> u16 {
        w(&self.img, self.rec(ch) + f)
    }
    fn setw(&mut self, ch: usize, f: usize, v: u16) {
        let o = self.rec(ch) + f;
        if let Some(x) = self.img.get_mut(o..o + 2) {
            x.copy_from_slice(&v.to_be_bytes());
        }
    }
    fn pacland(&self) -> bool {
        self.lay.skip.is_some()
    }

    /// 9.5.
    fn start(&mut self, s: usize, hw: &mut Hw) {
        let s = if self.lay.use_sub { s } else { 0 };
        if self.pacland() {
            hw.dmacon(0x000F);
            hw.bus.write_word(0xDFF09A, 0x0380);
            hw.bus.write_word(0xDFF09E, 0x00FF);
        } else {
            hw.bus.write_word(0xDFF09E, 0x00FF);
            hw.dmacon(0x000F);
            hw.bus.write_word(0xDFF09A, 0x0380);
        }
        self.set_at(self.lay.pl, 0);
        self.set_at(self.lay.cnt, 1);
        if let Some((_, c, n)) = self.lay.skip {
            self.set_at(c, n);
        }
        let e = self.lay.song + 10 * s;
        self.set_at(self.lay.tmp, b(&self.img, e + 1));
        for ch in 0..4 {
            self.set(ch, 0x0A, 1);
            self.set(ch, 0, 0);
            self.set(ch, 1, 2);
            self.set(ch, 0x14, 0);
            if self.pacland() {
                self.set(ch, 0x0D, 0);
            }
            let seq = w(&self.img, e + 2 + 2 * ch);
            self.setw(ch, 4, seq);
            self.setw(ch, 2, w(&self.img, seq as usize));
            hw.len(ch, 0x40);
            hw.vol(ch, 0);
            if self.pacland() {
                hw.loc(ch, self.base.wrapping_add(self.lay.wave as u32));
            }
        }
        hw.dmacon(0x820F);
        self.set_at(self.lay.pl, 0xFF);
        self.unwrapped = 0xF;
    }

    /// 9.3.
    fn tick_a(&mut self, hw: &mut Hw) {
        if b(&self.img, self.lay.pl) == 0 {
            if self.lay.idle_output {
                self.output(hw);
            }
            return;
        }
        if let Some((flag, c, n)) = self.lay.skip {
            if b(&self.img, flag) != 0 {
                let k = b(&self.img, c).wrapping_sub(1);
                self.set_at(c, k);
                if k == 0 {
                    self.set_at(c, n);
                    self.output(hw);
                    return;
                }
            }
        }
        let k = b(&self.img, self.lay.cnt).wrapping_sub(1);
        self.set_at(self.lay.cnt, k);
        let mut ended = false;
        if k == 0 {
            self.set_at(self.lay.cnt, b(&self.img, self.lay.tmp));
            for ch in 0..4 {
                if !self.step(ch, hw) {
                    ended = true;
                    break;
                }
            }
        }
        if ended {
            if self.lay.end_output {
                self.output(hw);
            }
            return;
        }
        let mut h = PACLAND_H;
        for ch in 0..4 {
            self.update(ch, &mut h);
        }
        self.output(hw);
    }

    /// The channel update (9.3, step 4).
    fn update(&mut self, ch: usize, h: &mut u8) {
        let (env, envidx, env_d2) = (self.lay.env, self.lay.envidx, self.lay.env_d2);
        // the envelope
        let o = self.r(ch, 0x0F);
        if o < 0xC0 {
            self.set(ch, 0x0F, o.wrapping_sub(0x40));
            if o < 0x40 {
                let pos = self.r(ch, 0x0E);
                let start = b(&self.img, envidx + self.r(ch, 0x10) as usize);
                let v = if env_d2 {
                    let dd = ((*h as u16) << 8 | pos as u16) as i16 as i64;
                    let a = env as i64 + start as i64 + dd;
                    if a >= 0 { b(&self.img, a as usize) } else { 0 }
                } else {
                    b(&self.img, env + start.wrapping_add(pos) as usize)
                };
                self.set(ch, 0x0E, pos.wrapping_add(1));
                self.set(ch, 0x0F, v);
            }
        }
        // the volume
        let mut v = self.r(ch, 0x0F) & 0x3F;
        if let Some(g) = self.lay.gv {
            let gv = b(&self.img, g);
            if gv < 0x40 {
                v = ((v as u16 * gv as u16) >> 6) as u8;
            }
        }
        self.set(ch, 0x0D, v);
        // the arpeggio
        let mut k = self.r(ch, 0x17).wrapping_add(1);
        let mut a = b(&self.img, self.lay.arp + k as usize);
        if a >= 0x80 {
            k = self.r(ch, 0x16);
            a &= 0x7F;
        }
        self.set(ch, 0x17, k);
        // the period
        let n = a.wrapping_add(self.r(ch, 0x0C)).wrapping_add(self.r(ch, 0x14));
        let mut per = w(&self.img, self.lay.per + n.wrapping_mul(2) as usize);
        // the vibrato
        let fl = self.r(ch, 0);
        if fl & 0x40 != 0 {
            let (dep, mut x, sp) = (self.r(ch, 0x11), self.r(ch, 0x13), self.r(ch, 0x12));
            if !(self.lay.half_speed && fl & 0x10 != 0 && fl & 1 == 0) {
                if fl & 0x20 != 0 {
                    x = x.wrapping_add(sp);
                    if x >= dep {
                        x = dep;
                        self.set(ch, 0, self.r(ch, 0) ^ 0x20);
                    }
                } else if x < sp {
                    x = 0;
                    self.set(ch, 0, self.r(ch, 0) ^ 0x20);
                } else {
                    x -= sp;
                }
                self.set(ch, 0x13, x);
            }
            let t = x as i16 - (dep >> 1) as i16;
            per = per.wrapping_add(t as u16);
        }
        self.set(ch, 0, self.r(ch, 0) ^ 1);
        // the slide
        if self.r(ch, 0) & 4 != 0 {
            let dl = self.r(ch, 7);
            if dl != 0 {
                self.set(ch, 7, dl - 1);
            } else {
                let step = self.r(ch, 6) as i8;
                if self.lay.env_d2 {
                    *h = if step < 0 { 0xFF } else { 0 };
                }
                let acc = self.rw(ch, 8).wrapping_add(step as i16 as u16);
                self.setw(ch, 8, acc);
                per = per.wrapping_add(acc);
            }
        }
        self.setw(ch, 0x18, per);
        if self.lay.wave_copy {
            self.set(ch, 0x15, self.r(ch, 0x1A));
        }
    }

    /// The output (9.3, step 6).
    fn output(&mut self, hw: &mut Hw) {
        for ch in 0..4 {
            let per = self.rw(ch, 0x18);
            let vol = (self.r(ch, 0x0D) & 0x7F) as u16;
            if self.lay.vol_first {
                hw.vol(ch, vol);
                hw.per(ch, per);
            } else {
                hw.per(ch, per);
                hw.vol(ch, vol);
            }
            let lc = self.lay.wave as u32 + 128 * self.r(ch, 0x15) as u32;
            hw.loc(ch, self.base.wrapping_add(lc));
        }
    }

    /// The sequencer step of channel `ch` (9.4); false when the end command ran.
    fn step(&mut self, ch: usize, hw: &mut Hw) -> bool {
        let rc = self.r(ch, 0x0A).wrapping_sub(1);
        self.set(ch, 0x0A, rc);
        if rc != 0 {
            let fl = self.r(ch, 0);
            if fl & 8 != 0 {
                let n = self.r(ch, 0x0C);
                self.set(ch, 0x0C, if fl & 0x80 != 0 { n.wrapping_sub(1) } else { n.wrapping_add(1) });
            }
            return true;
        }
        self.set(ch, 0, self.r(ch, 0) & 0x70);
        let mut pp = self.rw(ch, 2) as usize;
        let rd = |s: &Self, pp: &mut usize| {
            let v = b(&s.img, *pp);
            *pp = (*pp + 1) & 0xFFFF;
            v
        };
        for _ in 0..GUARD {
            let x = rd(self, &mut pp);
            match x {
                0xE0..=0xFF => self.set(ch, 0x0B, x - 0xDF),
                0xB8..=0xDF => self.set(ch, 0x10, x - 0xB8),
                0xA0..=0xB7 => self.set(ch, self.lay.wf, x - 0xA0),
                0x90..=0x9F => {
                    let a = b(&self.img, self.lay.aib + x.wrapping_add(0x60) as usize);
                    self.set(ch, 0x16, a);
                }
                0x00..=0x7F => {
                    self.set(ch, 0x0C, x);
                    self.set(ch, 0x0E, 0);
                    self.set(ch, 0x0F, 0);
                    self.set(ch, 0x17, self.r(ch, 0x16));
                    break;
                }
                0x80 => {
                    self.set(ch, 0x0F, 0xC0);
                    break;
                }
                0x81 => self.set(ch, 0, 0),
                0x82 => {
                    let s = rd(self, &mut pp);
                    let dl = rd(self, &mut pp);
                    self.set(ch, 6, s);
                    self.setw(ch, 8, 0);
                    self.set(ch, 0, self.r(ch, 0) | 4);
                    self.set(ch, 7, dl);
                }
                0x83 => self.set(ch, 0, self.r(ch, 0) | 0x88),
                0x84 => self.set(ch, 0, self.r(ch, 0) | 0x08),
                0x85 => {
                    let mut dd = self.r(ch, 1) as usize;
                    let l = self.rw(ch, 4) as usize;
                    if w(&self.img, l + dd) == 0 {
                        dd = 0;
                        self.unwrapped &= !(1 << ch);
                        if self.unwrapped == 0 {
                            self.songend = true;
                            self.unwrapped = 0xF;
                        }
                    }
                    pp = w(&self.img, l + dd) as usize;
                    self.set(ch, 1, (dd + 2) as u8);
                }
                0x86 => {
                    let s = rd(self, &mut pp);
                    let dd = rd(self, &mut pp);
                    self.set(ch, 0x12, s);
                    self.set(ch, 0, if self.lay.form50 && dd >= 0x80 { 0x50 } else { 0x40 });
                    let dep = dd.wrapping_mul(2);
                    self.set(ch, 0x11, dep);
                    self.set(ch, 0x13, if self.lay.form50 { dep >> 1 } else { dd });
                }
                0x87 => {
                    self.set_at(self.lay.pl, 0);
                    if self.pacland() {
                        for c in 0..4 {
                            self.set(c, 0x0D, 0);
                            self.set(c, 0x30, 0);
                        }
                    } else {
                        hw.dmacon(0x000F);
                    }
                    self.songend = true;
                    return false;
                }
                0x88 => {
                    let t = rd(self, &mut pp);
                    self.set(ch, 0x14, t);
                }
                0x89 => {
                    let v = rd(self, &mut pp) as i8 as i16 as u16;
                    hw.bus.write_word(0xDFF09E, v & 0x800F);
                }
                _ => {}
            }
        }
        let rl = self.r(ch, 0x0B);
        self.set(ch, 0x0A, rl);
        self.setw(ch, 2, pp as u16);
        true
    }
}

impl Replayer for BuildA {
    fn format(&self) -> &'static str {
        "Jason Brooke"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn timer_delay(&self) -> u64 {
        TIMER_DELAY
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.songs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (n as usize) < self.lay.songs {
            self.sub = n;
            self.started = false;
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.img.copy_from_slice(&self.pristine);
            self.started = true;
            self.songend = false;
            self.start(self.sub as usize, hw);
        }
        self.tick_a(hw);
    }
}

/// Colour clocks the original's init routine runs before the host starts its timer, beyond the
/// one period (measured in the reference runs).
const INIT_CLOCKS: u64 = 17171;

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if is_a(d) {
        let lay = layout_a(d)?;
        m.bus.clock += INIT_CLOCKS;
        return Some(Box::new(BuildA {
            lay,
            img: d.to_vec(),
            pristine: d.to_vec(),
            base,
            sub: 0,
            started: false,
            songend: false,
            unwrapped: 0xF,
        }));
    }
    let lay = layout_b(d)?;
    m.bus.clock += INIT_CLOCKS;
    let mut img = d.to_vec();
    patch(&mut img, lay.p);
    Some(Box::new(BuildB {
        f: if lay.b2 { FB2 } else { FB },
        lay,
        pristine: img.clone(),
        img,
        base,
        sub: 0,
        started: false,
        songend: false,
        unwrapped: 0xF,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn words(d: &mut [u8], o: usize, ws: &[u16]) {
        for (i, &v) in ws.iter().enumerate() {
            d[o + 2 * i..o + 2 * i + 2].copy_from_slice(&v.to_be_bytes());
        }
    }

    /// A `d16(pc)` instruction `op` at `at` pointing at `target`.
    fn lea(d: &mut [u8], at: usize, op: u16, target: usize) {
        words(d, at, &[op, (target as i64 - (at as i64 + 2)) as i16 as u16]);
    }

    /// A synthetic build-B (B-28) image: the signposts the scan looks for, one song
    /// at 0x401 (tempo 0xFF), channel 0 playing `pat0` and the others resting, one
    /// instrument (tuning 0x2000, so the period is the table's; a 32-byte sample at
    /// 0x800 looping from byte 16), periods 0x100 + n at 0x180, an envelope holding
    /// level 0xFC, arpeggios of 0xFF (no offset).
    fn module_b(pat0: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x900];
        words(&mut d, 0xAC, &[0x48E7, 0xF8FC, 0x08F9, 0x0001, 0x00BF, 0xE001, 0x33FC, 0x0780, 0x00DF, 0xF09A]);
        lea(&mut d, 0xC0, 0x47FA, 0);
        words(&mut d, 0xC4, &[0x383C, 0x0028]);
        lea(&mut d, 0xC8, 0x41FA, 0x600);
        words(&mut d, 0xCC, &[0x3740, 0x0716, 0x143C, 0x0090, 0x117C, 0x0002, 0x0025]);
        words(&mut d, 0xDA, &[0x4E71, 0x4E71, 0xD4FB, 0x0070, 0xC2FC, 0x000A]);
        lea(&mut d, 0xE6, 0x45FA, 0x700);
        lea(&mut d, 0xEA, 0x41FA, 0x7F0);
        lea(&mut d, 0xEE, 0x43FA, 0x7EE);
        lea(&mut d, 0xF2, 0x43FA, 0x500);
        words(&mut d, 0xF6, &[0x4A28, 0x000E, 0x1031, 0x1040]);
        lea(&mut d, 0xFE, 0x41FA, 0x580);
        words(&mut d, 0x102, &[0x1172, 0x0000, 0x49F9, 0x00DF, 0xF000, 0x4E71, 0x7600]);
        words(&mut d, 0x110, &[0x33FC, 0x00FF, 0x00DF, 0xF09E, 0x33FC, 0x000F, 0x00DF, 0xF096, 0x4E75]);
        lea(&mut d, 0x122, 0x45FA, 0x4FE);
        words(&mut d, 0x126, &[0x1032, 0x0000, 0x1232, 0x0002, 0x6100, 0xFFE0, 0x48E7, 0xFFFE, 0x720E]);
        lea(&mut d, 0x138, 0x43FA, 0x401);
        // move.w $40(pc,d0.w),d1: the periods at 0x180
        words(&mut d, 0x13C, &[0x4E75, 0x323B, 0x0040]);
        for n in 0..40 {
            words(&mut d, 0x180 + 2 * n, &[0x100 + n as u16]);
        }
        // song 0: tempo, then (end, loop, sequence list) per channel; the padding ends the table
        d[0x401] = 0xFF;
        d[0x402..0x406].copy_from_slice(&[4, 0, 0x04, 0x20]);
        for ch in 1..4 {
            d[0x402 + 4 * ch..0x406 + 4 * ch].copy_from_slice(&[2, 0, 0x04, 0x30]);
        }
        d[0x412] = 0xFF;
        words(&mut d, 0x420, &[0x440, 0x450]);
        words(&mut d, 0x430, &[0x460]);
        d[0x440..0x440 + pat0.len()].copy_from_slice(pat0);
        d[0x460..0x463].copy_from_slice(&[0xFF, 0x80, 0x85]);
        // envelope 0 at ENV + 0: level 0xFC, then hold
        d[0x4FE] = 0;
        d[0x500] = 0xFC;
        d[0x540..0x560].fill(0xFF);
        // instrument 0
        words(&mut d, 0x700, &[0x2000, 0, 0x800, 0x20, 0x10]);
        // master volume, the channels' register offsets and DMA bits
        d[0xA1] = 0xFF;
        for ch in 0..4 {
            d[0x28 * ch + 0x26] = 0xA0 + 0x10 * ch as u8;
            d[0x28 * ch + 0x27] = 1 << ch;
        }
        d
    }

    /// A synthetic build-A image (Feud-like: no skip, no global volume, the period
    /// written first): one song, tempo 1, channel 0 a note 5 on waveform 1 with an
    /// envelope of level 0x3F, then the end command; the others resting.
    fn module_a() -> Vec<u8> {
        let mut d = vec![0u8; 0x500];
        words(&mut d, 0, &SIG_A);
        words(&mut d, 0x0C, &[0x0300, 0x6710, 0x532B, 0x0302, 0x177A, 0x02EB, 0x0302]);
        lea(&mut d, 0x1A, 0x41FA, 0x320);
        words(&mut d, 0x1E, &[0xD0FC, 0x001C]);
        lea(&mut d, 0x22, 0x43FA, 0x3A0);
        words(&mut d, 0x26, &[0x43F9, 0x00DF, 0xF0A0, 0x4E71]);
        lea(&mut d, 0x2E, 0x45FA, 0x400);
        lea(&mut d, 0x32, 0x41FA, 0x38);
        words(&mut d, 0x36, &[0x2E88, 0x4E71, 0xB03C, 0x0090, 0x117B, 0x0010, 0x0016, 0xD4FB, 0x007A]);
        words(&mut d, 0x48, &[0x0600, 0x0018, 0x6502, 0x4E71, 0x1140, 0x0015]);
        words(&mut d, 0x54, &[0x0C28, 0x00C0, 0x000F, 0x103B, 0x0064, 0x1171, 0x0020, 0x000F, 0x323B, 0x001A]);
        words(&mut d, 0x68, &[0x0810, 0x0006, 0x4E71, 0x4E71, 0x48E7, 0x0000, 0x137B, 0x004F, 0x0000, 0x4E75]);
        for n in 0..32 {
            words(&mut d, 0x80 + 2 * n, &[0x100 + n as u16]);
        }
        d[0xC0] = 0;
        words(&mut d, 0xC4, &[0x0001, 0x200, 0x210, 0x210, 0x210]);
        words(&mut d, 0x200, &[0x240, 0]);
        words(&mut d, 0x210, &[0x260, 0]);
        d[0x240..0x245].copy_from_slice(&[0xE0, 0xB8, 0xA1, 0x05, 0x87]);
        d[0x260..0x263].copy_from_slice(&[0xE0, 0x80, 0x85]);
        d[0x3A0] = 0x80;
        d[0x3A1] = 0x80;
        d[0x3C0] = 0x3F;
        d[0x3C1] = 0xE0;
        d
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, bool, Vec<(u32, u16, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("jcb.test"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * (960 * ticks + 100)];
        e.render(&mut out);
        let end = e.songend();
        (format, end, e.m.bus.write_log.take().unwrap())
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn finds_the_build_b_tables() {
        let d = module_b(&[0x80]);
        let lay = layout_b(&d).unwrap();
        assert!(!lay.b2);
        assert_eq!((lay.p, lay.t, lay.songs, lay.per, lay.ins, lay.sil_data, lay.sil_len), (0xAC, 0x401, 1, 0x180, 0x700, 0x7F0, 0x7EE));
        assert_eq!((lay.env, lay.arp, lay.arpidx, lay.envidx, lay.envvol), (0x500, 0x540, 0x580, 0x4FE, 0x500));
        assert_eq!((lay.pwm, lay.pwmlen, lay.st, lay.thr, lay.dma, lay.g, lay.nclear, lay.gtr), (0x600, 0x716, 0x28, 0x90, 0x25, 0xA0, 0x1E, None));
        assert!(detect(&d));
        // the lea at P+20 must point at the file start
        let mut e = d.clone();
        lea(&mut e, 0xC0, 0x47FA, 2);
        assert!(!detect(&e));
        assert!(!detect(&d[..0x100]));
        assert!(!detect(b"FORM\0\0\0\0EMODEMIC"));
    }

    #[test]
    fn patches_the_eagleplayer_sequences() {
        let mut d = module_b(&[0x80]);
        words(&mut d, 0x200, &[0x5201, 0x1941, 0x0009]);
        words(&mut d, 0x700, &[0x5201, 0x1941, 0x0009]);
        patch(&mut d, 0xAC);
        assert_eq!(&d[0x200..0x206], &[0x4E, 0xB9, 0, 0, 0, 0]);
        assert_eq!(&d[0x110..0x120], &[0x4E, 0xB9, 0, 0, 0, 0, 0x4E, 0x71, 0x4E, 0x71, 0x4E, 0x71, 0x4E, 0x71, 0x4E, 0x71]);
        // only within [P, P+0x578)
        assert_eq!(w(&d, 0x700), 0x5201);
    }

    #[test]
    fn plays_build_b_notes_samples_and_the_end() {
        // length 1, instrument 0, envelope 0, volume 0x80, note 5; length 4, note 7; end
        let d = module_b(&[0xE0, 0xA0, 0xC0, 0x8C, 0x80, 0x05, 0xE3, 0x07, 0x87]);
        let (format, end, log) = play(d, 8);
        assert_eq!(format, "Jason Brooke");
        assert!(end);
        assert_eq!(reg(&log, 0xDFF09E)[..2], [0x00FF, 0x00FF]);
        let dma = reg(&log, 0xDFF096);
        // start: all off; tick 0: the rests' and the note's DMA off (channels 3..0)
        assert_eq!(dma[..5], [0x000F, 0x0008, 0x0004, 0x0002, 0x0001]);
        assert!(dma.contains(&0x8201));
        let per = reg(&log, 0xDFF0A6);
        assert_eq!(per[..3], [0x100, 0x105, 0x107]);
        // the sample, again for the second note, then its loop from byte 16
        assert_eq!(reg(&log, 0xDFF0A4)[..3], [0x10, 0x10, 0x08]);
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[2].wrapping_sub(lc[0]), 0x10);
        // (0xFC >> 2) * 0xFF * 0x80 >> 16
        assert!(reg(&log, 0xDFF0A8).contains(&0x1F));
        // after the end: volume 0 and the channel's DMA off on every tick
        assert_eq!(dma[dma.len() - 4..], [0x0001, 0x0002, 0x0004, 0x0008]);
    }

    #[test]
    fn plays_build_a_waveforms_and_the_end() {
        let d = module_a();
        let lay = layout_a(&d).unwrap();
        assert_eq!((lay.pl, lay.cnt, lay.tmp, lay.rec, lay.st, lay.arp, lay.env, lay.envidx), (0x300, 0x302, 0x301, 0x320, 0x1C, 0x3A0, 0x3C0, 0xC0));
        assert_eq!((lay.per, lay.aib, lay.wave, lay.wf, lay.song, lay.songs), (0x80, 0x50, 0x400, 0x15, 0xC4, 1));
        assert!(lay.end_output && lay.idle_output && !lay.vol_first && !lay.env_d2 && lay.gv.is_none() && lay.skip.is_none());
        let (format, end, log) = play(d, 4);
        assert_eq!(format, "Jason Brooke");
        assert!(end);
        assert_eq!(reg(&log, 0xDFF096)[..3], [0x000F, 0x820F, 0x000F]);
        assert_eq!(reg(&log, 0xDFF0A4), [0x40]);
        assert_eq!(reg(&log, 0xDFF0A6)[..3], [0x105, 0x105, 0x105]);
        assert_eq!(reg(&log, 0xDFF0A8)[..2], [0, 0x3F]);
        // waveform 1 for channel 0, waveform 0 for channel 1
        let (a, b) = (reg(&log, 0xDFF0A2)[0], reg(&log, 0xDFF0B2)[0]);
        assert_eq!(a.wrapping_sub(b), 0x80);
    }

    /// `JCB_DIR=dir cargo test --release jasonbrooke::tests::layouts -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn layouts() {
        let Ok(dir) = std::env::var("JCB_DIR") else { return };
        let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        v.sort();
        for p in v {
            let d = std::fs::read(&p).unwrap();
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            if is_a(&d) {
                println!("{n}\n  {:x?}", layout_a(&d));
            } else {
                println!("{n}\n  {:x?}", layout_b(&d));
            }
        }
    }
}
