//! ZX Spectrum "Sound Tracker" songs (the `ST11` and `STRC` types of the ZXAY container): the
//! editor's own song format (ST11) and the same song after Patrik Rak's compressor (STRC). This
//! file turns a song into the AY registers of each 50 Hz interrupt; `ay.rs` plays them on its
//! AY-3-8912.
//!
//! Original implementation, written for this project (GPL-2.0-or-later), clean room, from
//! `docs/specs/ay-soundtracker/SPEC.md`, which gives the rules of the ORIGINAL Z80 Sound Tracker
//! player (owner's decision of 2026-11-16: these are ZX songs and play as on the ZX), found by
//! black-box runs of that player, and the layout of the songs. No player code of any kind was
//! read (Z80, Eagleplayer, DeliPlayer, AYPlayer). The method is the spec's: the song is
//! converted to the Sound Tracker "compiled song" layout (STC) in a 64 KB memory image, which a
//! model of the original player's rules (spec section 6) plays, interrupt by interrupt.
//!
//! Choices the spec leaves open (section 5.5), taken as its defaults and unverified against
//! the original compiler: an envelope code that repeats the stream's last effect code is left
//! out (like every repeated code); the sample and effect nibbles of an empty cell after row 0
//! are dropped; a rest keeps its nibbles; a pattern the position list names but the file does
//! not hold is empty. A note of index 96 (B sharp of octave 7) has no STC note code: an empty
//! cell.
//!
//! Not modelled: a note index 96-110 (through an ornament or a transposition) makes the original
//! read its own code bytes as a period; here the period is 0.
//!
//! Bounds: a row reading stops after `MAX_CODES` codes; record scans stay inside the data's
//! lists; every address wraps in the 64 KB.

use std::collections::HashMap;

/// Where the original player expects the compiled song.
const BASE: usize = 0xC43C;
/// Where its period table lies.
const TABLE: usize = 0xC35F;
/// Codes one channel reads in one interrupt before it gives up.
const MAX_CODES: usize = 256;

/// The 12 periods of octave 0; octave k is shifted right by k. Measured from the original
/// player's register output; data; the owner's decision 2026-11-16.
const PERIODS: [u16; 12] = [3832, 3600, 3424, 3200, 3032, 2856, 2696, 2544, 2400, 2272, 2136, 2016];

const SEMI: [u8; 8] = [0, 9, 11, 0, 2, 4, 5, 7];

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

/// A cell of a pattern: (kind, note, sample, effect, parameter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Empty,
    Note,
    Rest,
}

type Cell = (Kind, u8, u8, u8, u8);
const EMPTY: Cell = (Kind::Empty, 0, 0, 0, 0);

#[derive(Clone)]
struct Sample {
    vol: [u8; 32],
    noise: [u8; 32],
    add: [u16; 32],
    rep: u8,
    replen: u8,
}

/// A song as the editor holds it.
struct Song {
    delay: u8,
    patlen: usize,
    samples: Vec<Sample>,
    ornaments: Vec<[u8; 32]>,
    positions: Vec<(u8, u8)>,
    /// Pattern number to rows of three cells.
    patterns: HashMap<u8, Vec<[Cell; 3]>>,
}

fn cell_st11(b: &[u8]) -> Cell {
    if b[0] & 0x80 != 0 {
        return (Kind::Rest, 0, b[1] >> 4, b[1] & 15, b[2]);
    }
    let name = ((b[0] >> 4) & 7) as usize;
    if name == 0 {
        return EMPTY;
    }
    let n = 12 * (b[0] & 7) as usize + SEMI[name] as usize + ((b[0] >> 3) & 1) as usize;
    if n > 0x5F {
        return EMPTY;
    }
    (Kind::Note, n as u8, b[1] >> 4, b[1] & 15, b[2])
}

fn parse_st11(m: &[u8]) -> Option<Song> {
    if m.len() < 3009 {
        return None;
    }
    let samples = (0..15)
        .map(|i| {
            let x = &m[i * 130..(i + 1) * 130];
            let mut s = Sample { vol: [0; 32], noise: [0; 32], add: [0; 32], rep: x[128], replen: x[129] };
            for k in 0..32 {
                s.vol[k] = x[k];
                s.noise[k] = x[32 + k];
                s.add[k] = x[64 + 2 * k] as u16 | (x[65 + 2 * k] as u16) << 8;
            }
            s
        })
        .collect();
    let length = m[2462] as usize;
    let positions = (0..=length).map(|i| (m[1950 + 2 * i], m[1951 + 2 * i])).collect();
    let ornaments = (0..17)
        .map(|i| {
            let mut o = [0u8; 32];
            o.copy_from_slice(&m[2463 + 32 * i..2495 + 32 * i]);
            o
        })
        .collect();
    let delay = m[3007];
    let patlen = if m[3008] == 0 { 64 } else { m[3008] as usize };
    if patlen > 64 {
        return None;
    }
    let mut patterns = HashMap::new();
    for p in 0..(m.len() - 3009) / 576 {
        let x = &m[3009 + 576 * p..3009 + 576 * (p + 1)];
        let rows = (0..patlen)
            .map(|r| {
                let c = |c: usize| cell_st11(&x[r * 9 + c * 3..r * 9 + c * 3 + 3]);
                [c(0), c(1), c(2)]
            })
            .collect();
        patterns.insert(p as u8 + 1, rows);
    }
    Some(Song { delay, patlen, samples, ornaments, positions, patterns })
}

fn parse_strc(b: &[u8]) -> Option<Song> {
    // b: the song data after the container's 4 channel bytes... the whole structure
    let load = be16(b, 4)? as usize;
    let blk = b.get(10..)?;
    let mut mem = vec![0u8; 65536];
    for (i, &x) in blk.iter().take(65536 - load).enumerate() {
        mem[load + i] = x;
    }
    let m = |a: usize| mem[a & 0xFFFF];
    let w = |a: usize| m(a) as usize | (m(a + 1) as usize) << 8;
    let d = load;
    let patlen = if m(d + 1) == 0 { 64 } else { m(d + 1) as usize };
    if patlen > 64 {
        return None;
    }
    let ptr = |base: usize, k: usize| w(d + base + 2 * k);
    let mut samples = Vec::new();
    for k in 1..16 {
        let mut s = Sample {
            vol: [0; 32],
            noise: [0; 32],
            add: [0; 32],
            rep: m(d + 130 + 2 * k),
            replen: m(d + 131 + 2 * k).wrapping_sub(1),
        };
        for p in 0..32 {
            s.vol[p] = m(ptr(66, k) + p);
            s.noise[p] = m(ptr(98, k) + p);
            let v = (m(ptr(2, k) + 2 * p) as u16 | (m(ptr(2, k) + 2 * p + 1) as u16) << 8) as i16 as i32;
            s.add[p] = if v >= 0 { 0x1000 | v as u16 } else { (-v & 0xFFF) as u16 };
        }
        samples.push(s);
    }
    let mut ornaments: Vec<[u8; 32]> = (0..16)
        .map(|k| {
            let mut o = [0u8; 32];
            for (p, x) in o.iter_mut().enumerate() {
                *x = m(ptr(34, k) + p);
            }
            o
        })
        .collect();
    ornaments.push([0; 32]);
    let mut o = d + 162;
    let mut positions = Vec::new();
    let mut patterns = HashMap::new();
    while w(o) != 0 && positions.len() < 256 {
        let mut cols: Vec<Vec<Cell>> = Vec::new();
        for c in 0..3 {
            let mut q = w(o + 2 * c);
            let mut cells = Vec::new();
            for _ in 0..patlen {
                let x = m(q);
                match x {
                    0xFE => {
                        cells.push(EMPTY);
                        q += 1;
                    }
                    0xFF => {
                        cells.push((Kind::Rest, 0, 0, 0, 0));
                        q += 1;
                    }
                    0..=0x5F => {
                        let (b2, b3) = (m(q + 1), m(q + 2));
                        cells.push((Kind::Note, x, b2 >> 4, b2 & 15, b3));
                        q += 3;
                    }
                    _ => {
                        // a note byte 96-$FD: not representable
                        cells.push(EMPTY);
                        q += 1;
                    }
                }
            }
            cols.push(cells);
        }
        let n = positions.len() as u8 + 1;
        patterns.insert(n, (0..patlen).map(|r| [cols[0][r], cols[1][r], cols[2][r]]).collect());
        positions.push((n, m(o + 6)));
        o += 7;
    }
    if positions.is_empty() {
        return None;
    }
    Some(Song { delay: m(d), patlen, samples, ornaments, positions, patterns })
}

/// The song of a ZXAY ST11 / STRC file: `typ` is the type tag, `sd` the song data's offset.
fn parse(d: &[u8], typ: &[u8], sd: usize) -> Option<Song> {
    match typ {
        b"ST11" => parse_st11(d.get(sd + 8..)?),
        b"STRC" => parse_strc(d.get(sd..)?),
        _ => None,
    }
}

// ------------------------------------------------------------------ the conversion to STC

/// The code stream of one channel of one pattern (spec 5.3).
fn channel(rows: &[[Cell; 3]], c: usize) -> Vec<u8> {
    let mut events: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut smp_state = 0u8;
    let mut fx_state: Vec<u8> = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let (kind, n, smp, eff, par) = row[c];
        if kind == Kind::Empty {
            if r == 0 {
                events.push((r, vec![0x81]));
            }
            continue;
        }
        let mut codes = Vec::new();
        if smp != 0 && smp != smp_state {
            codes.push(0x60 + smp);
            smp_state = smp;
        }
        if eff != 0 {
            let fx = match eff {
                1 | 2 => vec![0x82],
                15 => vec![0x70 + (par & 15)],
                _ => vec![0x80 + eff, par],
            };
            if fx != fx_state {
                codes.extend_from_slice(&fx);
            }
            fx_state = fx;
        }
        codes.push(if kind == Kind::Note { n } else { 0x80 });
        events.push((r, codes));
    }
    let mut out = Vec::new();
    let mut cur: Option<usize> = None;
    for (k, (r, codes)) in events.iter().enumerate() {
        let nxt = events.get(k + 1).map_or(rows.len(), |e| e.0);
        let dur = nxt - r - 1;
        if cur != Some(dur) {
            out.push(0xA1 + dur as u8);
            cur = Some(dur);
        }
        out.extend_from_slice(codes);
    }
    out.push(0xFF);
    out
}

/// The sample and ornament numbers a channel stream selects.
fn stream_codes(st: &[u8], smp: &mut [bool; 16], orn: &mut [bool; 16]) {
    let mut q = 0;
    while q < st.len() && st[q] != 0xFF {
        let x = st[q];
        match x {
            0x60..=0x6F => smp[(x - 0x60) as usize] = true,
            0x70..=0x7F => orn[(x - 0x70) as usize] = true,
            _ => {}
        }
        q += if (0x83..=0x8E).contains(&x) { 2 } else { 1 };
    }
}

/// The song as STC data (spec 5.1), or None if it does not fit the player's memory.
fn compile(s: &Song) -> Option<Vec<u8>> {
    let mut used: Vec<u8> = s.positions.iter().map(|p| p.0).collect();
    used.sort_unstable();
    used.dedup();
    let empty = vec![[EMPTY; 3]; s.patlen];
    let mut streams: HashMap<u8, [Vec<u8>; 3]> = HashMap::new();
    let (mut smp_used, mut orn_used) = ([false; 16], [false; 16]);
    orn_used[0] = true;
    for &p in &used {
        let rows = s.patterns.get(&p).unwrap_or(&empty);
        let st = [channel(rows, 0), channel(rows, 1), channel(rows, 2)];
        for x in &st {
            stream_codes(x, &mut smp_used, &mut orn_used);
        }
        streams.insert(p, st);
    }
    let mut body = Vec::new();
    for (i, sm) in s.samples.iter().enumerate() {
        if !smp_used[i + 1] {
            continue;
        }
        body.push(i as u8 + 1);
        for k in 0..32 {
            let a = sm.add[k];
            body.push((sm.vol[k] & 15) | (((a >> 8) & 15) as u8) << 4);
            body.push((sm.noise[k] & 0xDF) | if a & 0x1000 != 0 { 0x20 } else { 0 });
            body.push(a as u8);
        }
        body.extend_from_slice(&[sm.rep, sm.replen]);
    }
    let pos_off = 27 + body.len();
    body.push((s.positions.len() - 1) as u8);
    for &(p, t) in &s.positions {
        body.extend_from_slice(&[p, t]);
    }
    let orn_off = 27 + body.len();
    for (i, o) in s.ornaments.iter().enumerate() {
        if i < 16 && orn_used[i] {
            body.push(i as u8);
            body.extend_from_slice(o);
        }
    }
    let pat_off = 27 + body.len();
    let tab = body.len();
    body.resize(tab + 7 * used.len() + 1, 0);
    let mut placed: HashMap<Vec<u8>, usize> = HashMap::new();
    for (j, p) in used.iter().enumerate() {
        let mut ent = vec![*p];
        for st in &streams[p] {
            let at = *placed.entry(st.clone()).or_insert_with(|| {
                let a = 27 + body.len();
                body.extend_from_slice(st);
                a
            });
            ent.extend_from_slice(&(at as u16).to_le_bytes());
        }
        body[tab + 7 * j..tab + 7 * j + 7].copy_from_slice(&ent);
    }
    body[tab + 7 * used.len()] = 0xFF;
    let mut stc = vec![0u8; 27];
    stc[0] = s.delay;
    stc[1..3].copy_from_slice(&(pos_off as u16).to_le_bytes());
    stc[3..5].copy_from_slice(&(orn_off as u16).to_le_bytes());
    stc[5..7].copy_from_slice(&(pat_off as u16).to_le_bytes());
    stc[7..25].copy_from_slice(b"SONG BY ST COMPILE");
    stc[25..27].copy_from_slice(&((27 + body.len()) as u16).to_le_bytes());
    stc.extend_from_slice(&body);
    (BASE + stc.len() <= 0x10000 && stc.len() < 0x10000).then_some(stc)
}

/// Interrupts of one pass of the position list (spec 7.3), at most 65535 like the EMUL header.
pub fn pass_length(d: &[u8], typ: &[u8], sd: usize) -> Option<u16> {
    let s = parse(d, typ, sd)?;
    Some((s.delay as usize * s.patlen * s.positions.len()).min(0xFFFF) as u16)
}

// ------------------------------------------------------------------ the player

#[derive(Clone, Copy, Default)]
struct Chan {
    /// Row base of the selected sample (0 until a sample code).
    r: u16,
    /// Ornament address.
    o: u16,
    env: bool,
    new_env: bool,
    note: u8,
    p: u8,
    /// Counter; $FF = silent.
    c: u8,
    ptr: u16,
    d: u8,
    w: u8,
    /// Held period registers.
    per: u16,
}

/// What the interrupt's last record lookup was (it decides the mixer bits of a silent channel A).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    None,
    Sample,
    Ornament,
    Position,
}

pub struct St {
    mem: Box<[u8; 65536]>,
    ch: [Chan; 3],
    pos: usize,
    trans: u8,
    tick: u8,
    shape: u8,
    param: u8,
    pending: bool,
    r6: u8,
    n: u64,
    delay: u8,
    // the STC lists
    smp: (usize, usize),
    orn: (usize, usize),
    pat: usize,
    pos_list: usize,
    look: Look,
    /// Whether the last interrupt sounded a note index 96-110 (a period not modelled).
    pub code_index: bool,
}

impl St {
    /// The song of a ZXAY file (`typ` the type tag, `sd` the song data offset).
    pub fn new(d: &[u8], typ: &[u8], sd: usize) -> Option<St> {
        let s = parse(d, typ, sd)?;
        let stc = compile(&s)?;
        let mut mem = Box::new([0u8; 65536]);
        // the host's low memory (spec 6.7)
        mem[..0x100].fill(0xC9);
        mem[0x100..0x4000].fill(0xFF);
        mem[0x38] = 0xFB;
        mem[..13].copy_from_slice(&[0xF3, 0xCD, 0x00, 0xC0, 0xED, 0x56, 0xFB, 0x76, 0xCD, 0x06, 0xC0, 0x18, 0xF7]);
        for i in 0..96 {
            let v = PERIODS[i % 12] >> (i / 12);
            mem[TABLE + 2 * i] = v as u8;
            mem[TABLE + 2 * i + 1] = (v >> 8) as u8;
        }
        mem[BASE..BASE + stc.len()].copy_from_slice(&stc);
        let w = |o: usize| stc[o] as usize | (stc[o + 1] as usize) << 8;
        let (pos_list, orn, pat) = (w(1), w(3), w(5));
        Some(St {
            mem,
            ch: [Chan { c: 0xFF, ..Default::default() }; 3],
            pos: 0,
            trans: 0,
            tick: 0,
            shape: 0,
            param: 0,
            pending: false,
            r6: 0,
            n: 0,
            delay: stc[0],
            smp: (27, pos_list),
            orn: (orn, pat),
            pat,
            pos_list,
            look: Look::None,
            code_index: false,
        })
        .map(|mut st| {
            let o0 = st.find(st.orn, 33, 0).unwrap_or(0);
            for c in st.ch.iter_mut() {
                c.o = o0 as u16;
            }
            st
        })
    }

    fn byte(&self, a: usize) -> u8 {
        self.mem[a & 0xFFFF]
    }

    /// The address of the data after the number byte of record `num` in the list `range`
    /// (records of `step` bytes, in the data at `BASE`).
    fn find(&self, range: (usize, usize), step: usize, num: u8) -> Option<usize> {
        (range.0..range.1)
            .step_by(step)
            .find(|&o| self.byte(BASE + o) == num)
            .map(|o| BASE + o + 1)
    }

    fn load_position(&mut self) {
        let pl = BASE + self.pos_list;
        let n = self.byte(pl) as usize + 1;
        if self.pos >= n {
            self.pos = 0;
        }
        let num = self.byte(pl + 1 + 2 * self.pos);
        self.trans = self.byte(pl + 2 + 2 * self.pos);
        // the pattern table: records of 7 bytes up to the $FF
        let mut o = BASE + self.pat;
        let mut at = None;
        while self.byte(o) != 0xFF && o < BASE + 0xFFFF {
            if self.byte(o) == num {
                at = Some(o);
                break;
            }
            o += 7;
        }
        let o = at.unwrap_or(BASE + self.pat);
        for c in 0..3 {
            // the table's offsets are from the start of the song data
            let off = u16::from_le_bytes([self.byte(o + 1 + 2 * c), self.byte(o + 2 + 2 * c)]);
            self.ch[c].ptr = off.wrapping_add(BASE as u16);
            self.ch[c].w = 0;
        }
        self.look = Look::Position;
    }

    /// Reads codes of channel `c` up to an event.
    fn read_channel(&mut self, c: usize) {
        if self.ch[c].w > 0 {
            self.ch[c].w -= 1;
            return;
        }
        for _ in 0..MAX_CODES {
            let ptr = self.ch[c].ptr as usize;
            let code = self.byte(ptr);
            self.ch[c].ptr = self.ch[c].ptr.wrapping_add(1);
            match code {
                0xFF => {
                    if c == 0 {
                        self.pos += 1;
                        self.load_position();
                        continue;
                    }
                    return;
                }
                0..=0x5F => {
                    self.ch[c].note = code;
                    self.ch[c].p = 0;
                    self.ch[c].c = 32;
                    break;
                }
                0x60..=0x6F => {
                    self.look = Look::Sample;
                    if let Some(a) = self.find(self.smp, 99, code - 0x60) {
                        self.ch[c].r = a as u16;
                    }
                }
                0x70..=0x7F => {
                    self.look = Look::Ornament;
                    if let Some(a) = self.find(self.orn, 33, code - 0x70) {
                        self.ch[c].o = a as u16;
                    }
                    self.ch[c].env = false;
                }
                0x80 => {
                    self.ch[c].c = 0xFF;
                    break;
                }
                0x81 => break,
                0x82 => {
                    self.look = Look::Ornament;
                    if let Some(a) = self.find(self.orn, 33, 0) {
                        self.ch[c].o = a as u16;
                    }
                    self.ch[c].env = false;
                }
                0x83..=0x8E => {
                    let par = self.byte(self.ch[c].ptr as usize);
                    self.ch[c].ptr = self.ch[c].ptr.wrapping_add(1);
                    self.shape = code - 0x80;
                    self.param = par;
                    self.pending = true;
                    self.ch[c].new_env = true;
                    self.ch[c].env = true;
                    self.look = Look::Ornament;
                    if let Some(a) = self.find(self.orn, 33, 0) {
                        self.ch[c].o = a as u16;
                    }
                }
                0xA1..=0xFE => self.ch[c].d = code - 0xA1,
                _ => {} // $8F-$A0: not produced by the compiler
            }
        }
        self.ch[c].w = self.ch[c].d;
    }

    /// One interrupt: the AY writes it makes, in order (register, value).
    pub fn tick(&mut self) -> Vec<(u8, u8)> {
        let mut w = Vec::with_capacity(16);
        if self.n == 0 {
            self.n = 1;
            for r in (0..=10).rev() {
                w.push((r, 0));
            }
            return w;
        }
        self.look = Look::None;
        self.code_index = false;
        if self.n == 1 {
            self.pos = 0;
            self.load_position();
        }
        self.n += 1;
        if self.tick == 0 {
            for c in 0..3 {
                self.read_channel(c);
            }
        }
        self.tick = (self.tick + 1) % self.delay.max(1);
        let mut vol = [0u8; 3];
        let mut mix = 0u8;
        for c in 0..3 {
            let (mut tone_off, mut noise_off) = (false, false);
            let r = self.ch[c].r as usize;
            let mut sounding = self.ch[c].c != 0xFF;
            let mut off = false;
            if sounding {
                self.ch[c].c = self.ch[c].c.wrapping_sub(1);
                if self.ch[c].c == 0 {
                    let rep = self.byte(r + 96);
                    if rep == 0 || rep >= 129 {
                        off = true;
                    } else {
                        self.ch[c].p = rep - 1;
                        self.ch[c].c = self.byte(r + 97).wrapping_add(1);
                        if self.ch[c].c == 0xFF {
                            off = true;
                        }
                    }
                } else if self.ch[c].c == 0xFF {
                    off = true;
                }
                if off {
                    self.ch[c].c = 0xFF;
                    sounding = false;
                }
            }
            if sounding || off {
                let row = r + (3 * self.ch[c].p as usize & 0xFF);
                let (b0, b1, b2) = (self.byte(row), self.byte(row + 1), self.byte(row + 2));
                if off {
                    // volume 0; channel A's mixer bits from the row, B and C's 0
                    if c == 0 {
                        tone_off = b1 & 0x40 != 0;
                        noise_off = b1 & 0x80 != 0;
                    }
                } else {
                    let o = self.byte(self.ch[c].o as usize + self.ch[c].p as usize) as i8 as i32;
                    let i = ((self.ch[c].note as i32 + self.trans as i8 as i32 + o) & 127) as usize;
                    let base = (if i < 96 {
                        PERIODS[i % 12] >> (i / 12)
                    } else if i < 111 {
                        self.code_index = true;
                        0 // not modelled: the original reads its own code bytes here
                    } else {
                        u16::from_le_bytes([self.byte(TABLE + 2 * i), self.byte(TABLE + 2 * i + 1)])
                    }) as i32;
                    let m = ((b0 >> 4) as i32) << 8 | b2 as i32;
                    let per = if b1 & 0x20 != 0 { base + m } else { base - m };
                    self.ch[c].per = per as u16; // 16 bits: the original writes the high byte unmasked
                    vol[c] = (b0 & 15) | if self.ch[c].env { 0x10 } else { 0 };
                    tone_off = b1 & 0x40 != 0;
                    noise_off = b1 & 0x80 != 0;
                    if !noise_off {
                        self.r6 = b1 & 31;
                    }
                    if self.ch[c].env {
                        if self.ch[c].new_env {
                            self.ch[c].new_env = false;
                        } else {
                            self.pending = false;
                        }
                    }
                    self.ch[c].p = (self.ch[c].p + 1) & 31;
                }
            } else if c == 0 {
                // a silent channel A: the fitted offset of spec 6.5
                let l = match self.look {
                    Look::None => 247,
                    Look::Sample => 41,
                    Look::Ornament => 99,
                    Look::Position => 180,
                };
                let b1 = self.byte(r + l + 1);
                tone_off = b1 & 0x40 != 0;
                noise_off = b1 & 0x80 != 0;
            }
            mix |= (tone_off as u8) << c | (noise_off as u8) << (c + 3);
        }
        if self.pending {
            w.push((13, self.shape));
            w.push((12, 0));
            w.push((11, self.param));
        }
        w.push((10, vol[2]));
        w.push((9, vol[1]));
        w.push((8, vol[0]));
        w.push((7, mix));
        w.push((6, self.r6));
        for c in (0..3).rev() {
            w.push((2 * c as u8 + 1, (self.ch[c].per >> 8) as u8));
            w.push((2 * c as u8, self.ch[c].per as u8));
        }
        w
    }
}
