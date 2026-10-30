//! Dave Lowe (`dl.*`) and Dave Lowe New (`dln.*`), the music drivers of Dave Lowe
//! (Uncle Art) for the Amiga games of 1988-95 (Power Drift, The Games, Altered
//! Beast, Frontier, Midwinter II ...; Flink, King's Quest VI, Beneath a Steel Sky,
//! Super Street Fighter II).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the players' observable
//! behaviour, and this player was written from it alone and checked against the
//! Paula register writes of UADE's Dave Lowe and Dave Lowe New players.
//!
//! Both formats are the same four-voice engine. A voice reads an order list of
//! sequences; a sequence is a stream of 16-bit words, a word above 100 a note (its
//! period, then its length in ticks) and a word of 100 or less a command (sample,
//! envelope, portamento, vibrato, rest, next sequence, jump ...). The tick is
//! 20 ms. A `dln` file is a flat data image; a `dl` file is a hunk executable that
//! carries the game's own replay routine as well, which is not run: its data tables
//! are read where the specification says they are, and the per-game differences of
//! the routine are a table (`davelowe_tables.rs`) and a few special cases.

use super::davelowe_tables::{PATCHES, ROWS};
use super::{be16, be32, Hw, Open, Replayer, WriteGaps};
use crate::machine::Machine;

pub const OPEN_DL: Open = Open { name: "Dave Lowe", detect: detect_dl, new: new_dl };
pub const OPEN_DLN: Open = Open { name: "Dave Lowe New", detect: detect_dln, new: new_dln };

/// "No position": the start-up stub and the built-in terminated order list.
const NONE: usize = usize::MAX;
/// The envelope terminator.
const END: u16 = 0x00FF;
/// DMACON master enable, written after every channel DMA write by the layout B files.
const FILLER: u16 = 0x87E0;

// ---------------------------------------------------------------- recognition

/// An order-list offset: high half zero, low half non-zero, below 0x8000 and even.
fn shape(v: u32) -> bool {
    v >> 16 == 0 && v & 0xFFFF != 0 && v < 0x8000 && v & 1 == 0
}

/// The `dln` header: its first subsong table entry's position and the layout.
fn dln_header(d: &[u8]) -> Option<(usize, bool)> {
    if d.len() < 32 {
        return None;
    }
    let w0 = be16(d, 0);
    if w0 != 8 && w0 != 4 {
        return None;
    }
    let layout_a = be32(d, 24) != 0;
    let p = if w0 == 4 && layout_a { 4 } else { 8 };
    Some((p, layout_a))
}

fn detect_dln(d: &[u8]) -> bool {
    let Some((p, _)) = dln_header(d) else { return false };
    let mut lists = [0usize; 4];
    for (i, l) in lists.iter_mut().enumerate() {
        let v = be32(d, p + 4 * i);
        if !shape(v) || v as usize + 8 > d.len() {
            return false;
        }
        *l = v as usize;
    }
    if lists.iter().any(|&o| !shape(be32(d, o))) {
        return false;
    }
    lists.iter().any(|&o| first_sample_record(d, be32(d, o) as usize).is_some())
}

/// The sample record a sequence starts with: an optional filter-off command, the
/// envelope command, then the sample command; its record must start with 1.
fn first_sample_record(d: &[u8], mut s: usize) -> Option<usize> {
    if s + 16 > d.len() {
        return None;
    }
    if be16(d, s) == 0x30 {
        s += 2;
    }
    if be16(d, s) != 0x0C || be16(d, s + 6) != 0x04 {
        return None;
    }
    let rec = be32(d, s + 8) as usize;
    (rec + 2 <= d.len() && be16(d, rec) == 1).then_some(rec)
}

const HUNK_HEADER: u32 = 0x3F3;
const STUB: u32 = 0x70FF_4E75;

/// A hunk executable of one hunk of the given type; the hunk's size in longs.
fn hunk_shape(d: &[u8], ty: u32) -> Option<usize> {
    if d.len() < 64 || be32(d, 0) != HUNK_HEADER || be32(d, 4) != 0 || be32(d, 8) != 1 || be32(d, 12) != 0 || be32(d, 16) != 0 {
        return None;
    }
    let size = be32(d, 20);
    let n = (size & 0x3FFF_FFFF) as usize;
    if size >> 30 == 0 || be32(d, 24) != ty || be32(d, 28) as usize != n || be32(d, 32) != STUB || n * 4 < 32 {
        return None;
    }
    Some(n)
}

fn detect_dl(d: &[u8]) -> bool {
    if let Some(n) = hunk_shape(d, 0x3E9) {
        // UNCLEART
        if d.get(36..44) != Some(b"UNCLEART") || d.len() < 32 + 4 * n {
            return false;
        }
        let h = |o: usize| be32(d, 32 + o) as usize;
        return [12, 16, 24].iter().all(|&o| h(o) != 0 && h(o) < 4 * n);
    }
    if let Some(n) = hunk_shape(d, 0x3EA) {
        // DAVELOWE
        if d.get(36..44) != Some(b"DAVELOWE") || be16(d, 44) != 0x0100 || d.len() < 32 + 4 * n {
            return false;
        }
        let count = be16(d, 46) as usize;
        if count == 0 || 16 + 22 * count > 4 * n {
            return false;
        }
        return (0..count).all(|i| {
            let e = 32 + 16 + 22 * i;
            (0..4).all(|c| shape4(be32(d, e + 4 * c), n)) && (be32(d, e + 16) as usize) < 4 * n
        });
    }
    false
}

fn shape4(v: u32, n: usize) -> bool {
    v != 0 && (v as usize) < 4 * n
}

// -------------------------------------------------------------------- engine

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Dl,
    Older,
    DlnA,
    DlnB,
}

/// What differs between the replay routines of the games.
#[derive(Clone)]
struct Cfg {
    kind: Kind,
    /// Words in an envelope block's header: 2, 1, or 0 (the operand is the data).
    env_hdr: u8,
    /// At the terminator the last value is written again.
    held: bool,
    rest_returns: bool,
    boost: bool,
    ik: bool,
    dma_tick: bool,
    /// The rest envelope's held value is a full volume.
    rest64: bool,
    /// The held value is preceded by a write of 1.
    held1: bool,
    /// Where the module's own silent envelope lies, if known.
    rest_at: Option<usize>,
    fade: bool,
    /// The highest command word the routine has.
    cmax: u16,
    /// Vibrato has a start delay.
    vdelay: bool,
}

impl Cfg {
    fn dln(kind: Kind) -> Cfg {
        Cfg { kind, env_hdr: 2, held: true, rest_returns: false, boost: false, ik: false, dma_tick: false, rest64: false, held1: false, rest_at: None, fade: false, cmax: 56, vdelay: true }
    }
}

#[derive(Clone, Default)]
struct Voice {
    ch: usize,
    left: u16,
    smode: u16,
    period: u16,
    pmode: u16,
    pstep: u16,
    ptarget: u16,
    pdelay: u16,
    vstate: u16,
    vcount: u16,
    vreload: u16,
    vup: u16,
    vdown: u16,
    vdelay: u16,
    vdelay_reload: u16,
    seq: usize,
    order: usize,
    order0: usize,
    env: usize,
    env0: usize,
    att: (u32, u16),
    lp: (u32, u16),
    boost: bool,
    dead: bool,
}

/// The Time Scanner's start-up sound effect on channel D.
struct Effect {
    table: usize,
    active: bool,
}

pub struct DaveLowe {
    /// The data image: a `dln` file, or a `dl` file's hunk.
    img: Vec<u8>,
    /// Where `img[0]` lies in guest memory.
    mem: u32,
    cfg: Cfg,
    title: String,
    /// Order-list offsets per subsong, and the layout B tick limits.
    subs: Vec<([usize; 4], u16)>,
    /// Fade-in parameter per subsong (the older files).
    fades: Vec<u16>,
    cur: u16,
    v: [Voice; 5],
    /// The silent envelope: a zero word, then the terminator (offset of the zero word).
    rest_env: usize,
    cnt: u16,
    songcnt: [u8; 4],
    songend: bool,
    tickno: u32,
    /// Frontier's fade-in: the reduction, and ticks to its next step.
    fade_r: u16,
    fade_wait: u16,
    /// The older files' master volume with its fade-in.
    master: u16,
    mfade: u16,
    mcount: u16,
    fx: Option<Effect>,
    special: u32,
    /// Fixed-up longs of the sample table: (hunk offset, value).
    patched: Vec<(usize, u32)>,
}

fn rows_key(d: &[u8], n: usize) -> Cfg {
    let h = |o: usize| be32(d, 32 + o);
    let mut c = Cfg::dln(Kind::Dl);
    c.boost = true;
    if let Some(r) = ROWS.iter().find(|r| r.longs as usize == n && r.init == h(12) && r.play == h(16) && r.end == h(20)) {
        c.env_hdr = r.env_hdr;
        // (the routines that return at a rest do not write the held value either,
        // whatever the table's flag says: Raffles and Soldier of Light)
        c.held = r.held && !r.rest_returns;
        if r.ik {
            // (the IK+ routine does write the held value, unlike the table's flag)
            c.held = true;
        }
        c.rest_returns = r.rest_returns;
        c.boost = r.boost;
        c.ik = r.ik;
        c.dma_tick = r.dma_tick;
        c.rest64 = r.rest64;
        c.held1 = r.held1;
        c.rest_at = Some(r.rest_env as usize);
        c.fade = r.fade;
        if r.patch == 4 {
            c.cmax = 48;
        }
        if r.ik {
            c.cmax = 40;
            c.vdelay = false;
        }
    }
    c
}

fn new_dl(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_dl(d) {
        return None;
    }
    let older = be32(d, 24) == 0x3EA;
    let n = be32(d, 28) as usize;
    let mut img = d[32..(32 + 4 * n).min(d.len())].to_vec();
    let mem = base + 32;
    let mut subs = Vec::new();
    let mut fades = Vec::new();
    let mut title = String::new();
    let cfg;
    let mut special = 0;
    let mut patched = Vec::new();
    if older {
        let mut c = Cfg::dln(Kind::Older);
        c.boost = true;
        c.dma_tick = true;
        cfg = c;
        let count = be16(&img, 14) as usize;
        for i in 0..count {
            let e = 16 + 22 * i;
            let l = [0, 1, 2, 3].map(|c| be32(&img, e + 4 * c) as usize);
            subs.push((l, 0));
            fades.push(be16(&img, e + 20));
        }
    } else {
        let mut c = rows_key(d, n);
        c.kind = Kind::Dl;
        cfg = c;
        special = be32(&img, 76);
        let name = be32(&img, 48) as usize;
        if let Some(s) = img.get(name..) {
            title = s.iter().take_while(|&&b| b != 0).take(80).map(|&b| b as char).collect();
        }
        // sample-table fix-ups of the four modules whose records hold no final addresses
        let row = ROWS.iter().find(|r| r.longs as usize == n && r.init == be32(&img, 12) && r.play == be32(&img, 16) && r.end == be32(&img, 20));
        if let Some(r) = row.filter(|r| r.patch != 0) {
            for &(o, long, v) in PATCHES[r.patch as usize - 1] {
                let o = o as usize;
                if long && o + 4 <= img.len() {
                    img[o..o + 4].copy_from_slice(&v.to_be_bytes());
                    patched.push((o, v));
                } else if !long && o + 2 <= img.len() {
                    img[o..o + 2].copy_from_slice(&(v as u16).to_be_bytes());
                }
            }
        }
        if cfg.ik {
            // one tune; the order lists start in the voices' initialised state blocks
            let l = [0x4F0usize, 0x52E, 0x56C, 0x5AA].map(|b| be32(&img, b + 0x1E) as usize);
            subs.push((l, 0));
        } else {
            let first = be32(&img, 44) as usize;
            let mut i = 0;
            while first != 0 && i < 64 {
                let e = first + 16 * i;
                if e + 16 > img.len() || (i > 0 && be32(&img, e) & !0xFF == 0) {
                    break;
                }
                subs.push(([0, 1, 2, 3].map(|c| be32(&img, e + 4 * c) as usize), 0));
                i += 1;
            }
        }
        if be32(&img, 60) == 77554 && subs.len() == 17 {
            midwinter_repair(&mut img);
        }
    }
    if subs.is_empty() {
        return None;
    }
    let mut s = DaveLowe::build(img, mem, cfg, title, subs, fades, special);
    s.patched = patched;
    m.bus.clock += DL_INIT_CLOCKS;
    Some(Box::new(s))
}

/// Colour clocks the original's init routine runs before the host starts its timer, beyond the
/// one period (measured in the reference runs).
const DL_INIT_CLOCKS: u64 = 21093;

/// Colour clocks the original's init routine runs before the host starts its timer, beyond the
/// one period (measured in the reference runs: 6016 to 6459 for the Beneath a Steel Sky songs).
const DLN_INIT_CLOCKS: u64 = 5570;

fn new_dln(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_dln(d) {
        return None;
    }
    let (_, layout_a) = dln_header(d)?;
    let w0 = be16(d, 0) as usize;
    let w2 = be16(d, 2) as usize;
    let mut subs = Vec::new();
    let kind;
    if layout_a {
        kind = Kind::DlnA;
        let mut n = 0;
        let mut o = w0;
        loop {
            let v = be32(d, o);
            if o + 4 > d.len() || v >> 16 != 0 || v & 0xFFFF == 0 || v & 1 != 0 {
                break;
            }
            n += 1;
            o += 4;
        }
        for i in 0..n / 4 {
            subs.push(([0, 1, 2, 3].map(|c| be32(d, w0 + 16 * i + 4 * c) as usize), 0));
        }
    } else {
        kind = Kind::DlnB;
        for i in 0..w2.saturating_sub(8) / 32 {
            let e = w0 + 32 * i;
            subs.push(([0, 1, 2, 3].map(|c| be32(d, e + 4 + 4 * c) as usize), be32(d, e) as u16));
        }
    }
    if subs.is_empty() {
        return None;
    }
    let mut img = d.to_vec();
    // the one module whose order lists lack their terminators
    if kind == Kind::DlnA && w0 == 8 && be32(&img, 56) == 0x74 && subs.len() == 5 {
        for o in [124usize, 188, 284] {
            if o + 8 <= img.len() {
                img[o..o + 4].copy_from_slice(&0u32.to_be_bytes());
                img[o + 4..o + 8].copy_from_slice(&1u32.to_be_bytes());
            }
        }
    }
    m.bus.clock += DLN_INIT_CLOCKS;
    Some(Box::new(DaveLowe::build(img, base, Cfg::dln(kind), String::new(), subs, Vec::new(), 0)))
}

/// Midwinter II: the order lists of subsongs 3, 11 and 13 have broken sequence
/// pointers; streams are appended to the image and the entries pointed at them.
fn midwinter_repair(img: &mut Vec<u8>) {
    let push_long = |w: &mut Vec<u16>, v: u32| {
        w.push((v >> 16) as u16);
        w.push(v as u16);
    };
    let base = (img.len() + 1) & !1;
    let s = 0x4F82u32;
    let notes = |w: &mut Vec<u16>, n: &[(u16, u16)]| {
        for &(p, l) in n {
            w.push(p);
            w.push(l);
        }
    };
    // the envelope blocks follow the three streams; their lengths are fixed
    let r1 = 0usize;
    let mut a = Vec::new();
    let e1_slot = |a: &mut Vec<u16>| {
        a.push(12);
        a.push(0xE1E1);
        a.push(0xE1E1);
    };
    // stream R1
    e1_slot(&mut a);
    a.push(4);
    push_long(&mut a, s);
    notes(&mut a, &[(0x1AC, 10), (0x194, 10), (0x1C5, 10), (0x1AC, 10), (0x1AC, 10), (0x194, 10), (0x1C5, 10), (0x1AC, 10)]);
    a.push(REST_WORD);
    let r2 = a.len();
    // stream R2
    e1_slot(&mut a);
    a.push(4);
    push_long(&mut a, s);
    notes(&mut a, &[(0x1AC, 24), (0x17D, 32), (0x168, 32), (0x17D, 40), (0x1AC, 24), (0x17D, 32), (0x168, 32), (0x140, 40)]);
    a.push(12);
    a.push(0xE2E2);
    a.push(0xE2E2);
    a.push(4);
    push_long(&mut a, s);
    notes(&mut a, &[(0x11D, 8); 13]);
    notes(&mut a, &[(0x140, 8), (0x168, 8), (0x17D, 8), (0x1AC, 8)]);
    a.push(REST_WORD);
    let r3 = a.len();
    // stream R3
    e1_slot(&mut a);
    a.push(4);
    push_long(&mut a, s);
    notes(&mut a, &[(0x1AC, 10), (0x1AC, 10), (0x0D6, 10), (0x168, 5), (0x0F0, 10), (0x140, 5), (0x0D6, 10), (0x11D, 10), (0x12E, 10)]);
    a.extend_from_slice(&[REST_WORD, 0x01AC, 0x000A, 0x0020, 0x0046, 0x0008]);
    let e1 = a.len();
    a.extend_from_slice(&[0x0000, 0x002C, 0x00FF]);
    let e2 = a.len();
    a.extend_from_slice(&[0x0000, 0x0040, 0x0040, 0x003B, 0x0036, 0x0031, 0x002C, 0x0028, 0x0023, 0x001E, 0x0019, 0x0014, 0x000F, 0x00FF]);
    // the block addresses go into the placeholders
    let e1a = (base + 2 * e1) as u32;
    let e2a = (base + 2 * e2) as u32;
    for i in 0..a.len() {
        if a[i] == 0xE1E1 && a[i + 1] == 0xE1E1 {
            a[i] = (e1a >> 16) as u16;
            a[i + 1] = e1a as u16;
        } else if a[i] == 0xE2E2 && a[i + 1] == 0xE2E2 {
            a[i] = (e2a >> 16) as u16;
            a[i + 1] = e2a as u16;
        }
    }
    img.resize(base, 0);
    for w in &a {
        img.extend_from_slice(&w.to_be_bytes());
    }
    let put = |img: &mut Vec<u8>, o: usize, v: u32| img[o..o + 4].copy_from_slice(&v.to_be_bytes());
    let (a1, a2, a3) = ((base + 2 * r1) as u32, (base + 2 * r2) as u32, (base + 2 * r3) as u32);
    for i in 0..4 {
        put(img, 0x2E44 + 4 * i, a1);
    }
    put(img, 0x261A, a2);
    for i in 0..5 {
        put(img, 0x2F6E + 4 * i, a3);
    }
}

/// The word that ends a repaired stream with a rest whose length is the next word.
const REST_WORD: u16 = 8;

impl DaveLowe {
    fn build(mut img: Vec<u8>, mem: u32, cfg: Cfg, title: String, subs: Vec<([usize; 4], u16)>, fades: Vec<u16>, special: u32) -> DaveLowe {
        // the silent envelope; for the After Burner the word before the terminator
        // is the rest's held value
        if img.len() % 2 == 1 {
            img.push(0);
        }
        let rest_env = cfg.rest_at.unwrap_or(img.len());
        let silent: &[u16] = if cfg.rest64 {
            &[0xFEF0, END]
        } else if cfg.ik {
            &[0, 0, END]
        } else if !cfg.held {
            &[END]
        } else {
            &[0, END]
        };
        if cfg.rest_at.is_none() {
            for w in silent {
                img.extend_from_slice(&w.to_be_bytes());
            }
        }
        let mut s = DaveLowe {
            img,
            mem,
            cfg,
            title,
            subs,
            fades,
            cur: 1,
            v: Default::default(),
            rest_env,
            cnt: 0,
            songcnt: [0xFE; 4],
            songend: false,
            tickno: 0,
            fade_r: 0,
            fade_wait: 0,
            master: 64,
            mfade: 0,
            mcount: 0,
            fx: None,
            special,
            patched: Vec::new(),
        };
        s.start(1);
        s
    }

    fn w(&self, o: usize) -> u16 {
        if o == NONE {
            return 0;
        }
        be16(&self.img, o)
    }

    fn l(&self, o: usize) -> u32 {
        if o == NONE {
            return 0;
        }
        be32(&self.img, o)
    }

    /// The start of subsong `n` (1-based): all voices fresh, the first fetch a wrap.
    fn start(&mut self, n: u16) {
        self.cur = n;
        let (lists, _) = self.subs[n as usize - 1];
        for (c, v) in self.v.iter_mut().enumerate() {
            *v = Voice { ch: c.min(3), seq: NONE, order: NONE, order0: if c < 4 { lists[c] } else { NONE }, env0: 0, ..Default::default() };
        }
        self.v[4].dead = true;
        self.songcnt = [0xFE; 4];
        self.songend = false;
        self.tickno = 0;
        self.fade_r = 0;
        self.fade_wait = 0;
        self.master = 64;
        self.mfade = 0;
        self.mcount = 0;
        if self.cfg.kind == Kind::Older {
            self.mfade = self.fades.get(n as usize - 1).copied().unwrap_or(0);
            if self.mfade != 0 {
                self.master = 0;
                self.mcount = self.mfade;
            }
        }
        if self.cfg.fade && n == 10 {
            self.fade_r = 64;
            self.fade_wait = 19;
        }
        self.fx = None;
        if self.special == 38 && n == 1 {
            self.fx = Some(Effect { table: 0xD54, active: false });
        }
    }

    // ------------------------------------------------------------- writes

    /// Whether voice `vi`'s register writes are discarded (the Time Scanner's music
    /// voice D while the effect owns the channel).
    fn muted(&self, vi: usize) -> bool {
        vi == 3 && self.fx.as_ref().is_some_and(|f| f.active)
    }

    fn dma(&mut self, vi: usize, on: bool, hw: &mut Hw) {
        if self.muted(vi) && !on {
            return;
        }
        let bit = 1u16 << self.v[vi].ch;
        hw.dmacon(if on { 0x8000 | bit } else { bit });
        if self.cfg.kind == Kind::DlnB {
            hw.dmacon(FILLER);
        }
    }

    fn lc(&mut self, vi: usize, a: u32, len: u16, note: bool, hw: &mut Hw) {
        if self.muted(vi) {
            return;
        }
        hw.loc(self.v[vi].ch, self.mem.wrapping_add(a));
        if note && self.cfg.kind == Kind::Dl {
            hw.wait(NOTE_LEN[self.v[vi].ch & 3]);
        }
        hw.len(self.v[vi].ch, len);
    }

    fn per(&mut self, vi: usize, p: u16, hw: &mut Hw) {
        if !self.muted(vi) {
            hw.per(self.v[vi].ch, p);
        }
    }

    /// A volume written straight, which the master volume does not see.
    fn vol_raw(&mut self, vi: usize, val: u16, hw: &mut Hw) {
        if !self.muted(vi) {
            hw.vol(self.v[vi].ch, val & 0x7F);
        }
    }

    fn vol(&mut self, vi: usize, val: u16, hw: &mut Hw) {
        let mut x = val;
        if let Some(f) = &self.fx {
            if f.active && vi < 3 {
                let v = &self.v[vi];
                let h1 = if self.cfg.env_hdr == 2 { self.w(v.env0.wrapping_sub(4)) } else { self.w(v.env0.wrapping_sub(2)) };
                x = x.saturating_sub(h1);
            }
        }
        if self.fade_r != 0 {
            x = x.saturating_sub(self.fade_r);
        }
        if self.cfg.kind == Kind::Older {
            let m = self.master;
            if self.mfade != 0 && m != 64 {
                if self.mcount != 0 {
                    self.mcount -= 1;
                } else {
                    self.mcount = self.mfade;
                    self.master += 1;
                }
            }
            x = ((x as u32 * m as u32) >> 6) as u16;
        }
        if !self.muted(vi) {
            hw.vol(self.v[vi].ch, x & 0x7F);
        }
    }

    /// The volume step: the envelope's next value, or the held one.
    fn vol_step(&mut self, vi: usize, hw: &mut Hw) {
        let e = self.v[vi].env;
        let w = self.w(e);
        if w == END {
            if self.cfg.held {
                if self.cfg.held1 {
                    self.vol(vi, 1, hw);
                }
                let x = self.w(e.wrapping_sub(2));
                self.vol(vi, x, hw);
            }
            return;
        }
        let mut x = w;
        if self.v[vi].boost && self.cfg.boost {
            x = x.wrapping_add(self.w(self.v[vi].env0.wrapping_sub(2)));
        }
        self.v[vi].env = e + 2;
        self.vol(vi, x, hw);
    }

    fn wrap_test(&mut self, vi: usize) {
        if vi >= 4 {
            return;
        }
        if self.songcnt[vi] != 0 {
            self.songcnt[vi] = self.songcnt[vi].wrapping_add(1);
        }
        if self.songcnt.iter().all(|&c| c == 0) {
            self.songend = true;
            self.songcnt = [0xFF; 4];
        }
    }

    // ------------------------------------------------------------ the voice

    fn voice(&mut self, vi: usize, hw: &mut Hw) {
        if self.v[vi].dead {
            return;
        }
        let mut off = false;
        if self.v[vi].left == 0 {
            self.fetch(vi, hw);
        } else {
            if self.v[vi].left == 1 {
                if self.v[vi].smode != 0 {
                    off = true;
                    if !self.cfg.dma_tick {
                        self.dma(vi, false, hw);
                    }
                }
                self.v[vi].left = 0;
            } else {
                if self.v[vi].smode == 3 {
                    let (a, l) = self.v[vi].lp;
                    self.lc(vi, a, l, false, hw);
                    self.v[vi].smode = 1;
                }
                self.v[vi].left -= 1;
            }
            self.slide(vi, hw);
            self.vol_step(vi, hw);
        }
        if self.cfg.dma_tick && !self.v[vi].dead {
            self.dma(vi, !off, hw);
        }
    }

    /// Portamento and vibrato.
    fn slide(&mut self, vi: usize, hw: &mut Hw) {
        let v = &mut self.v[vi];
        let mut p = v.period;
        let mut changed = false;
        if v.pmode != 0 {
            if v.pdelay != 0 {
                v.pdelay -= 1;
            } else {
                changed = true;
                if v.pmode == 1 {
                    p = p.wrapping_add(v.pstep);
                    if p > v.ptarget {
                        p = v.ptarget;
                        v.pmode = 0;
                    }
                } else {
                    p = p.wrapping_sub(v.pstep);
                    if p <= v.ptarget {
                        p = v.ptarget;
                        v.pmode = 0;
                    }
                }
            }
        }
        if v.vstate != 0 {
            if v.vdelay != 0 {
                v.vdelay -= 1;
            } else if v.vcount != 0 {
                v.vcount -= 1;
            } else {
                v.vcount = v.vreload;
                changed = true;
                if v.vstate < 3 {
                    p = p.wrapping_sub(v.vdown);
                    v.vstate += 1;
                } else {
                    p = p.wrapping_add(v.vup);
                    v.vstate += 1;
                    if v.vstate == 5 {
                        v.vstate = 1;
                    }
                }
            }
        }
        if changed {
            v.period = p;
            self.per(vi, p, hw);
        }
    }

    fn fetch(&mut self, vi: usize, hw: &mut Hw) {
        let mut pos = self.v[vi].seq;
        self.v[vi].boost = false;
        // (a sequence of commands that loops back on itself never reaches a note)
        for _ in 0..4096 {
            let w = if pos == NONE { 8 } else { self.w(pos) };
            if w > 100 {
                self.note(vi, pos, hw);
                return;
            }
            if w >= 60 || w % 4 != 0 || w > self.cfg.cmax {
                return; // halt
            }
            match w / 4 {
                0 => return,
                1 => {
                    let rec = self.l(pos + 2) as usize;
                    pos += 6;
                    let mut r = [0u8; 14];
                    for (i, b) in r.iter_mut().enumerate() {
                        *b = self.img.get(rec + i).copied().unwrap_or(0);
                    }
                    if let Some(&(_, val)) = self.patched.iter().find(|p| p.0 == rec) {
                        // a record read from the middle of a fixed-up record: the first long is
                        // an absolute address, so its type word is that address' high half
                        // (never 0 where the load address is, and then 1: see the module header)
                        let abs = self.mem.wrapping_add(val);
                        r[..4].copy_from_slice(&abs.to_be_bytes());
                        if abs >> 16 == 0 {
                            r[1] = 1;
                        }
                    }
                    let ty = be16(&r, 0);
                    let start = be32(&r, 2);
                    let len = be16(&r, 6);
                    if ty == 0 {
                        self.v[vi].smode = 0;
                        self.lc(vi, start, len, false, hw);
                    } else {
                        let lp = (be32(&r, 8), be16(&r, 12));
                        let v = &mut self.v[vi];
                        v.smode = ty;
                        v.att = (start, len);
                        v.lp = lp;
                    }
                }
                2 => {
                    let v = &self.v[vi];
                    if v.order0 == NONE && v.order == NONE {
                        return;
                    }
                    let mut order = v.order;
                    if order == NONE || self.l(order) == 0 {
                        order = self.v[vi].order0;
                        self.wrap_test(vi);
                    }
                    let entry = self.l(order) as usize;
                    self.v[vi].order = order + 4;
                    pos = entry;
                }
                3 => {
                    let b = self.l(pos + 2) as usize;
                    pos += 6;
                    self.v[vi].env0 = match self.cfg.env_hdr {
                        2 => b + 4,
                        1 => b + 2,
                        _ => b,
                    };
                }
                4 => {
                    let v = &mut self.v[vi];
                    let a = [2, 4, 6, 8].map(|k| be16(&self.img, pos + k));
                    v.pmode = a[0];
                    v.pstep = a[1];
                    v.ptarget = a[2];
                    v.pdelay = a[3];
                    pos += 10;
                }
                5 => {
                    let a = [2, 4, 6, 8, 10, 12].map(|k| be16(&self.img, pos + k));
                    let v = &mut self.v[vi];
                    v.vstate = 1;
                    v.vcount = a[0];
                    v.vreload = a[1];
                    v.vup = a[2];
                    v.vdown = a[3];
                    if self.cfg.vdelay {
                        v.vdelay = a[4];
                        v.vdelay_reload = a[5];
                    }
                    pos += 14;
                }
                6 => {
                    self.v[vi].pmode = 0;
                    pos += 2;
                }
                7 => {
                    self.v[vi].vstate = 0;
                    pos += 2;
                }
                8 => {
                    let len = self.w(pos + 2);
                    let v = &mut self.v[vi];
                    v.left = len.wrapping_sub(1);
                    v.seq = pos + 4;
                    v.env = self.rest_env;
                    self.vol_raw(vi, 0, hw);
                    if len & 0x8000 != 0 {
                        self.wrap_test(vi);
                    }
                    if !self.cfg.rest_returns {
                        self.vol_step(vi, hw);
                    }
                    return;
                }
                9 | 10 if !self.cfg.ik => {
                    self.stop(vi, hw);
                    return;
                }
                9 | 10 => pos += 2,
                // bclr / bset.b #1,$bfe001: the LED filter on, off
                11 => {
                    hw.bus.ciaa.pra &= !2;
                    pos += 2;
                }
                12 => {
                    hw.bus.ciaa.pra |= 2;
                    pos += 2;
                }
                13 => {
                    self.v[vi].boost = true;
                    pos += 2;
                }
                14 => {
                    pos = self.l(pos + 2) as usize;
                }
                _ => return,
            }
        }
    }

    /// A note: `pos` is at its period.
    fn note(&mut self, vi: usize, pos: usize, hw: &mut Hw) {
        let period = self.w(pos);
        self.per(vi, period, hw);
        let length = self.w(pos + 2);
        let v = &mut self.v[vi];
        v.period = period;
        v.left = length.wrapping_sub(1);
        v.seq = pos + 4;
        v.env = v.env0;
        v.vdelay = v.vdelay_reload;
        if v.vstate != 0 {
            v.vstate = 1;
        }
        if v.smode != 0 {
            v.smode = 3;
            let (a, l) = v.att;
            self.lc(vi, a, l, true, hw);
        }
        if !self.cfg.dma_tick {
            self.dma(vi, true, hw);
        }
        self.vol_step(vi, hw);
    }

    /// The stop command: the voice's DMA off, a period of 1 on the first channel
    /// (as the routine has it), volume 0, and the voice stays as it is.
    fn stop(&mut self, vi: usize, hw: &mut Hw) {
        if vi == 4 {
            hw.per(3, 1);
            hw.dmacon(0x0008);
            hw.vol(3, 0);
            self.v[4].dead = true;
            if let Some(f) = &mut self.fx {
                f.active = false;
            }
            return;
        }
        self.dma(vi, false, hw);
        if !self.muted(vi) {
            hw.per(0, 1);
            hw.vol(self.v[vi].ch, 0);
        }
        self.v[vi].dead = true;
    }
}

/// Colour clocks from the song's start to the host's timer start, beyond the first period (measured
/// in the reference runs).
const TIMER_DELAY: u64 = 565;

/// Where the writes of a tick fall in the original's run (Line of Fire), by the previous write: a
/// voice's work between two writes takes this long. The tick's first write sits at its start plus
/// the `start` gap (shifted by 141 colour clocks, which `timer_delay` takes off again).
static DL_GAPS: WriteGaps = WriteGaps::new(
    &[
        (0, 4, 10), // LOC0 -> LEN0 x87
        (1, 5, 10), // LOC1 -> LEN1 x73
        (2, 6, 10), // LOC2 -> LEN2 x135
        (3, 7, 10), // LOC3 -> LEN3 x23
        (4, 12, 257), // LEN0 -> VOL0 x43
        (4, 20, 83), // LEN0 -> DMA_ON x44
        (5, 13, 270), // LEN1 -> VOL1 x36
        (5, 20, 98), // LEN1 -> DMA_ON x37
        (6, 14, 269), // LEN2 -> VOL2 x67
        (6, 20, 113), // LEN2 -> DMA_ON x68
        (7, 15, 307), // LEN3 -> VOL3 x11
        (7, 20, 118), // LEN3 -> DMA_ON x12
        (8, 0, 185), // PER0 -> LOC0 x44
        (9, 1, 200), // PER1 -> LOC1 x37
        (10, 2, 215), // PER2 -> LOC2 x68
        (11, 3, 228), // PER3 -> LOC3 x12
        (11, 15, 317), // PER3 -> VOL3 x183
        (12, 1, 235), // VOL0 -> LOC1 x36
        (12, 9, 247), // VOL0 -> PER1 x37
        (12, 12, 193), // VOL0 -> VOL0 x4
        (12, 13, 475), // VOL0 -> VOL1 x374
        (12, 16, 231), // VOL0 -> DMA_OFF x50
        (13, 2, 250), // VOL1 -> LOC2 x67
        (13, 10, 263), // VOL1 -> PER2 x68
        (13, 13, 201), // VOL1 -> VOL1 x13
        (13, 14, 494), // VOL1 -> VOL2 x295
        (13, 16, 246), // VOL1 -> DMA_OFF x67
        (14, 3, 265), // VOL2 -> LOC3 x11
        (14, 11, 414), // VOL2 -> PER3 x191
        (14, 15, 559), // VOL2 -> VOL3 x284
        (14, 16, 262), // VOL2 -> DMA_OFF x11
        (16, 11, 177), // DMA_OFF -> PER3 x4
        (16, 12, 249), // DMA_OFF -> VOL0 x48
        (16, 13, 257), // DMA_OFF -> VOL1 x50
        (16, 14, 261), // DMA_OFF -> VOL2 x67
        (16, 15, 311), // DMA_OFF -> VOL3 x7
        (20, 12, 188), // DMA_ON -> VOL0 x44
        (20, 13, 201), // DMA_ON -> VOL1 x37
        (20, 14, 205), // DMA_ON -> VOL2 x68
        (20, 15, 214), // DMA_ON -> VOL3 x12
    ],
    &[(0, 185), (1, 200), (2, 216), (3, 228), (4, 10), (5, 10), (6, 10), (7, 10), (9, 247), (10, 263), (11, 414), (12, 249), (13, 475), (14, 494), (15, 523), (16, 246), (20, 98)],
    &[(0, 4), (8, 179), (12, 236), (16, 0)],
);

/// The same for the Beneath a Steel Sky, King's Quest 6 and Midwinter-style songs (layout A), whose
/// player works differently from Line of Fire's.
static DLNA_GAPS: WriteGaps = WriteGaps::new(
    &[
        (0, 4, 10), // LOC0 -> LEN0 x84
        (1, 5, 10), // LOC1 -> LEN1 x116
        (2, 6, 10), // LOC2 -> LEN2 x137
        (3, 7, 10), // LOC3 -> LEN3 x34
        (4, 12, 220), // LEN0 -> VOL0 x42
        (4, 20, 147), // LEN0 -> DMA_ON x42
        (5, 13, 229), // LEN1 -> VOL1 x58
        (5, 20, 178), // LEN1 -> DMA_ON x58
        (6, 14, 257), // LEN2 -> VOL2 x67
        (6, 20, 209), // LEN2 -> DMA_ON x70
        (7, 15, 246), // LEN3 -> VOL3 x17
        (7, 20, 217), // LEN3 -> DMA_ON x17
        (8, 0, 147), // PER0 -> LOC0 x42
        (9, 1, 162), // PER1 -> LOC1 x58
        (9, 13, 233), // PER1 -> VOL1 x425
        (10, 2, 185), // PER2 -> LOC2 x70
        (10, 14, 252), // PER2 -> VOL2 x126
        (11, 3, 182), // PER3 -> LOC3 x17
        (11, 15, 270), // PER3 -> VOL3 x75
        (12, 1, 258), // VOL0 -> LOC1 x58
        (12, 9, 396), // VOL0 -> PER1 x469
        (12, 13, 462), // VOL0 -> VOL1 x2396
        (12, 16, 254), // VOL0 -> DMA_OFF x59
        (13, 2, 273), // VOL1 -> LOC2 x67
        (13, 10, 411), // VOL1 -> PER2 x184
        (13, 13, 152), // VOL1 -> VOL1 x5
        (13, 14, 480), // VOL1 -> VOL2 x2660
        (13, 16, 269), // VOL1 -> DMA_OFF x71
        (14, 3, 289), // VOL2 -> LOC3 x17
        (14, 11, 427), // VOL2 -> PER3 x91
        (14, 14, 155), // VOL2 -> VOL2 x3
        (14, 15, 510), // VOL2 -> VOL3 x2850
        (14, 16, 285), // VOL2 -> DMA_OFF x24
        (15, 15, 169), // VOL3 -> VOL3 x7
        (16, 9, 155), // DMA_OFF -> PER1 x14
        (16, 10, 163), // DMA_OFF -> PER2 x12
        (16, 11, 155), // DMA_OFF -> PER3 x1
        (16, 12, 212), // DMA_OFF -> VOL0 x42
        (16, 13, 221), // DMA_OFF -> VOL1 x45
        (16, 14, 225), // DMA_OFF -> VOL2 x59
        (16, 15, 238), // DMA_OFF -> VOL3 x23
        (20, 12, 152), // DMA_ON -> VOL0 x42
        (20, 13, 161), // DMA_ON -> VOL1 x58
        (20, 14, 164), // DMA_ON -> VOL2 x70
        (20, 15, 177), // DMA_ON -> VOL3 x17
    ],
    &[(0, 147), (1, 214), (2, 186), (3, 239), (4, 10), (5, 10), (6, 10), (7, 10), (9, 396), (10, 411), (11, 427), (12, 212), (13, 462), (14, 480), (15, 510), (16, 269), (20, 178)],
    &[(0, 505), (8, 507), (12, 700), (16, 501)],
);

/// And for the Super Street Fighter 2 songs (layout B); the model's start gaps need the timer 23
/// colour clocks earlier.
static DLNB_GAPS: WriteGaps = WriteGaps::new(
    &[
        (0, 4, 10), // LOC0 -> LEN0 x369
        (1, 5, 10), // LOC1 -> LEN1 x206
        (2, 6, 10), // LOC2 -> LEN2 x472
        (3, 7, 10), // LOC3 -> LEN3 x441
        (4, 12, 220), // LEN0 -> VOL0 x184
        (4, 20, 147), // LEN0 -> DMA_ON x185
        (5, 13, 254), // LEN1 -> VOL1 x95
        (5, 20, 178), // LEN1 -> DMA_ON x111
        (6, 14, 232), // LEN2 -> VOL2 x235
        (6, 20, 209), // LEN2 -> DMA_ON x237
        (7, 15, 246), // LEN3 -> VOL3 x219
        (7, 20, 217), // LEN3 -> DMA_ON x222
        (8, 0, 147), // PER0 -> LOC0 x185
        (9, 1, 170), // PER1 -> LOC1 x111
        (9, 13, 234), // PER1 -> VOL1 x294
        (10, 2, 178), // PER2 -> LOC2 x237
        (11, 3, 182), // PER3 -> LOC3 x222
        (11, 15, 270), // PER3 -> VOL3 x127
        (12, 1, 287), // VOL0 -> LOC1 x95
        (12, 9, 425), // VOL0 -> PER1 x385
        (12, 13, 515), // VOL0 -> VOL1 x1044
        (12, 16, 283), // VOL0 -> DMA_OFF x107
        (13, 2, 303), // VOL1 -> LOC2 x235
        (13, 10, 540), // VOL1 -> PER2 x237
        (13, 14, 510), // VOL1 -> VOL2 x917
        (13, 16, 299), // VOL1 -> DMA_OFF x242
        (14, 3, 318), // VOL2 -> LOC3 x219
        (14, 11, 321), // VOL2 -> PER3 x344
        (14, 14, 155), // VOL2 -> VOL2 x3
        (14, 15, 539), // VOL2 -> VOL3 x845
        (14, 16, 314), // VOL2 -> DMA_OFF x223
        (16, 20, 10), // DMA_OFF -> DMA_ON x760
        (20, 9, 164), // DMA_ON -> PER1 x20
        (20, 11, 155), // DMA_ON -> PER3 x5
        (20, 12, 212), // DMA_ON -> VOL0 x373
        (20, 13, 160), // DMA_ON -> VOL1 x198
        (20, 14, 215), // DMA_ON -> VOL2 x479
        (20, 15, 178), // DMA_ON -> VOL3 x440
        (20, 20, 10), // DMA_ON -> DMA_ON x755
    ],
    &[(0, 147), (1, 170), (2, 178), (3, 190), (4, 10), (5, 10), (6, 10), (7, 10), (9, 425), (10, 540), (11, 321), (12, 212), (13, 491), (14, 510), (15, 539), (16, 299), (20, 10)],
    &[(0, 4), (8, 6), (12, 198), (16, 0)],
);

/// A note's length write follows its address later than a loop update's does: colour clocks more,
/// by channel.
const NOTE_LEN: [u64; 4] = [52, 68, 83, 87];

impl Replayer for DaveLowe {
    fn format(&self) -> &'static str {
        match self.cfg.kind {
            Kind::DlnA | Kind::DlnB => "Dave Lowe New",
            _ => "Dave Lowe",
        }
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn timer_delay(&self) -> u64 {
        // the write-spacing models' start gaps are shifted so that none is negative
        TIMER_DELAY
            - match self.cfg.kind {
                Kind::Dl => 141,
                Kind::DlnB => 23,
                _ => 0,
            }
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.subs.len() as u16, self.cur)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if n >= 1 && n as usize <= self.subs.len() {
            self.start(n);
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tickno += 1;
        if self.cfg.kind == Kind::DlnB {
            let limit = self.subs[self.cur as usize - 1].1;
            self.cnt = self.cnt.wrapping_add(1);
            if self.cnt > limit {
                self.cnt = 0;
                return;
            }
        }
        if self.fade_r != 0 {
            self.fade_wait -= 1;
            if self.fade_wait == 0 {
                self.fade_r -= 1;
                self.fade_wait = 19;
            }
        }
        match self.cfg.kind {
            Kind::Dl => hw.gaps(&DL_GAPS),
            Kind::DlnA => hw.gaps(&DLNA_GAPS),
            Kind::DlnB => hw.gaps(&DLNB_GAPS),
            Kind::Older => {}
        }
        let first = self.tickno == 1;
        for vi in 0..3 {
            self.voice(vi, hw);
        }
        if first && self.fx.is_some() {
            self.effect_start(hw);
        }
        self.voice(3, hw);
        if !first && self.fx.as_ref().is_some_and(|f| f.active) {
            self.voice(4, hw);
        }
    }
}

impl DaveLowe {
    /// The Time Scanner's request: channel D stopped, a period of 1 and a length of
    /// 32 written, and an effect voice started on the effect's sequence.
    fn effect_start(&mut self, hw: &mut Hw) {
        let Some(f) = &mut self.fx else { return };
        f.active = true;
        let table = f.table;
        hw.dmacon(0x0008);
        hw.per(3, 1);
        hw.len(3, 0x20);
        let seq = self.l(table + 4 * 38) as usize;
        self.v[4] = Voice { ch: 3, seq, order: NONE, order0: NONE, ..Default::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn p16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn p32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// A sequence at `s`: the envelope at `env` (header 5, 7; data 40, 30), the sample
    /// record at `rec`, a 3-tick note of period 300, then the next-sequence command;
    /// with `boost` the volume-boost command before the note, with `direct` a direct
    /// sample (else attack and loop).
    fn sequence(d: &mut [u8], s: usize, env: usize, rec: usize, boost: bool, direct: bool) {
        let mut o = s;
        p16(d, o, 12);
        p32(d, o + 2, env as u32);
        p16(d, o + 6, 4);
        p32(d, o + 8, rec as u32);
        o += 12;
        if boost {
            p16(d, o, 52);
            o += 2;
        }
        p16(d, o, 300);
        p16(d, o + 2, 3);
        p16(d, o + 4, 8);
        p16(d, env, 5);
        p16(d, env + 2, 7);
        p16(d, env + 4, 40);
        p16(d, env + 6, 30);
        p16(d, env + 8, 0xFF);
        if direct {
            p16(d, rec, 0);
            p32(d, rec + 2, 0x360);
            p16(d, rec + 6, 4);
        } else {
            p16(d, rec, 1);
            p32(d, rec + 2, 0xE0);
            p16(d, rec + 6, 4);
            p32(d, rec + 8, 0xF0);
            p16(d, rec + 12, 2);
        }
    }

    /// Layout A: one subsong, four order lists of one sequence each.
    fn dln_a() -> Vec<u8> {
        let mut d = vec![0u8; 0x100];
        p16(&mut d, 0, 8);
        for c in 0..4 {
            p32(&mut d, 8 + 4 * c, 0x40 + 8 * c as u32);
            p32(&mut d, 0x40 + 8 * c, 0x80);
        }
        p32(&mut d, 24, 2);
        sequence(&mut d, 0x80, 0xC0, 0xD0, false, false);
        d
    }

    /// Layout B: a tick limit of 1, so every second tick is skipped.
    fn dln_b() -> Vec<u8> {
        let mut d = dln_a();
        d[8..32].fill(0);
        p16(&mut d, 0, 4);
        p16(&mut d, 2, 40);
        p32(&mut d, 4, 1);
        for c in 0..4 {
            p32(&mut d, 8 + 4 * c, 0x40 + 8 * c as u32);
        }
        d
    }

    /// A hunk executable with the UNCLEART header and one subsong.
    fn dl_hunk() -> Vec<u8> {
        let n = 0x100usize;
        let mut d = vec![0u8; 32 + 4 * n];
        p32(&mut d, 0, HUNK_HEADER);
        p32(&mut d, 8, 1);
        p32(&mut d, 20, 0x4000_0000 | n as u32);
        p32(&mut d, 24, 0x3E9);
        p32(&mut d, 28, n as u32);
        let h = 32;
        p32(&mut d, h, STUB);
        d[h + 4..h + 12].copy_from_slice(b"UNCLEART");
        p32(&mut d, h + 12, 0x200);
        p32(&mut d, h + 16, 0x210);
        p32(&mut d, h + 24, 0x220);
        p32(&mut d, h + 44, 0x240);
        for c in 0..4 {
            p32(&mut d, h + 0x240 + 4 * c, 0x260 + 8 * c as u32);
            p32(&mut d, h + 0x260 + 8 * c, 0x300);
        }
        // the table's second entry starts below 0x100: the end of the subsong table
        p32(&mut d, h + 0x250, 0x10);
        sequence(&mut d[h..], 0x300, 0x340, 0x350, true, true);
        d
    }

    fn play(d: Vec<u8>, ticks: usize) -> (String, bool, Vec<(u32, u16, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test"), d).unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * (960 * ticks + 100)];
        e.render(&mut out);
        let end = e.songend();
        (format, end, e.m.bus.write_log.take().unwrap())
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    fn tick(w: &(u32, u16, u64)) -> u64 {
        (w.2 + 3000) / 70940
    }

    #[test]
    fn recognises_by_content() {
        assert!(detect_dln(&dln_a()));
        assert!(detect_dln(&dln_b()));
        assert!(!detect_dl(&dln_a()));
        assert!(detect_dl(&dl_hunk()));
        assert!(!detect_dln(&dl_hunk()));
        // the sample command no longer follows the envelope command
        let mut d = dln_a();
        p16(&mut d, 0x86, 0);
        assert!(!detect_dln(&d));
        // the first sample record must start with 1
        let mut d = dln_a();
        p16(&mut d, 0xD0, 2);
        assert!(!detect_dln(&d));
        // an odd order-list offset
        let mut d = dln_a();
        p32(&mut d, 8, 0x41);
        assert!(!detect_dln(&d));
        assert!(!detect_dln(&dln_a()[..20]));
        let mut d = dl_hunk();
        d[36] = b'V';
        assert!(!detect_dl(&d));
        let mut d = dl_hunk();
        p32(&mut d, 28, 0x101);
        assert!(!detect_dl(&d));
        assert!(!detect_dl(b"FORM\0\0\0\0EMODEMIC"));
    }

    #[test]
    fn layout_a_plays_attack_loop_envelope_and_song_end() {
        let (format, end, log) = play(dln_a(), 4);
        assert_eq!(format, "Dave Lowe New");
        // note, loop pair, DMA off on the last tick, the note again
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [300, 300]);
        assert_eq!(reg(&log, 0xDFF0A4)[..3], [4, 2, 4]);
        assert_eq!(reg(&log, 0xDFF0A8)[..4], [40, 30, 30, 40]);
        let dma: Vec<u16> = reg(&log, 0xDFF096).into_iter().filter(|v| v & 1 != 0).collect();
        assert_eq!(dma[..3], [0x8001, 0x0001, 0x8001]);
        let lc = |i: usize| (reg(&log, 0xDFF0A0)[i] as u32) << 16 | reg(&log, 0xDFF0A2)[i] as u32;
        assert_eq!(lc(1) - lc(0), 0x10);
        // every voice has wrapped its list twice (the start-up stub, then the end): song end
        assert!(end);
        let (_, end3, _) = play(dln_a(), 3);
        assert!(!end3);
    }

    #[test]
    fn layout_b_skips_ticks_and_follows_dma_with_the_enable() {
        let (_, _, log) = play(dln_b(), 5);
        let vol: Vec<_> = log.iter().filter(|w| w.0 == 0xDFF0A8).map(|w| (tick(w), w.1)).collect();
        assert_eq!(vol[..3], [(1, 40), (3, 30), (5, 30)]);
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..2], [0x8001, FILLER]);
    }

    #[test]
    fn hunk_file_boost_and_direct_sample() {
        let (format, _, log) = play(dl_hunk(), 3);
        assert_eq!(format, "Dave Lowe");
        // a direct sample: LC and LEN at the sample command, before the period
        let a: Vec<u32> = log.iter().map(|w| w.0).filter(|a| (0xDFF0A0..0xDFF0B0).contains(a) || *a == 0xDFF096).take(6).collect();
        assert_eq!(a, [0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF0A6, 0xDFF096, 0xDFF0A8]);
        assert_eq!(reg(&log, 0xDFF0A4)[0], 4);
        // command 52 adds the envelope's second header word to each value, but not to the held one
        assert_eq!(reg(&log, 0xDFF0A8)[..3], [47, 37, 30]);
        assert_eq!(reg(&log, 0xDFF0A6)[0], 300);
    }

    #[test]
    fn older_flavour_writes_dma_after_the_voice() {
        let mut d = dl_hunk();
        p32(&mut d, 24, 0x3EA);
        d[36..44].copy_from_slice(b"DAVELOWE");
        let h = 32;
        d[h + 12..h + 48].fill(0);
        p16(&mut d, h + 12, 0x0100);
        p16(&mut d, h + 14, 1);
        for c in 0..4 {
            p32(&mut d, h + 16 + 4 * c, 0x260 + 8 * c as u32);
        }
        assert!(detect_dl(&d));
        let (_, _, log) = play(d, 2);
        let a: Vec<u32> = log.iter().map(|w| w.0).filter(|a| (0xDFF0A0..0xDFF0B0).contains(a) || *a == 0xDFF096).take(6).collect();
        assert_eq!(a, [0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF0A6, 0xDFF0A8, 0xDFF096]);
    }

    #[test]
    fn subsongs_are_counted_from_the_tables() {
        let mut d = dln_a();
        // a second subsong: the table is now 8 longs
        p32(&mut d, 24, 0x40);
        p32(&mut d, 28, 0x48);
        p32(&mut d, 32, 0x50);
        p32(&mut d, 36, 0x58);
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new("t.dln"), d).unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!((p.sub_min, p.sub_max, p.sub), (1, 2, 1));
    }
}
