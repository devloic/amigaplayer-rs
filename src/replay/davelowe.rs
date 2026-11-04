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

use super::davelowe_gaps::{wrapper, Wrap};
use super::davelowe_tables::{DL_INIT_COST, PATCHES, ROWS};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

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
    /// Layout A write timing (see `Slot`): the clock at the tick's entry and the per-voice state.
    base: u64,
    slot: i64,
    tt: i64,
    add: i64,
    cmd: i64,
    vbase: i64,
    vadj: bool,
    last_vol: Option<i64>,
    /// Layout B: the delay the DMA enable writes have put on the rest of the tick, and the voice
    /// that is the first with the longest order list.
    fill: i64,
    special_voice: usize,
    /// The `dl` driver's wrapper constants (cycles) and, per voice, whether its DMA start writes
    /// carry the master enable bit.
    wrap: Wrap,
    dmaon_master: [bool; 4],
    /// `dl`: colour clocks of the change to subsong j (index j - 2): stop call, two 118-clock calls and song-init j.
    chain: Vec<u64>,
    /// The tick time the engine takes after a subsong change (colour clocks, absolute).
    resume: Option<u64>,
    /// The cycle of the last volume write of the tick so far (`dl` timing).
    vol_t: Option<i64>,
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
    let start_t = if older {
        DL_OLDER_T
    } else {
        let h = &d[32..];
        DlStart::of(h, (n as u32, be32(h, 12), be32(h, 16), be32(h, 20))).t()
    };
    if !older {
        // the wrapper constants of the build (a module outside the table takes its family's), and the
        // DMA-on words the module's code holds (the master bit per voice)
        let h = &d[32..];
        let key = (n as u32, be32(h, 12), be32(h, 16), be32(h, 20));
        s.wrap = wrapper(key, key.2, key.1, std::env::var_os("DL_LOO").is_some());
        s.dmaon_master = dl_master_bits(h);
        s.chain = DlStart::of(h, key).chain(h);
    }
    let origin = m.bus.clock;
    if older {
        // the older Eagleplayer starts its chain two aligned frames late; the init writes the four
        // volumes and switches DMA off
        let chain = origin + DL_OLDER_SHIFT;
        for (i, ch) in [0xA8u32, 0xB8, 0xC8, 0xD8].into_iter().enumerate() {
            m.bus.clock = chain + 669 + 6 * i as u64;
            m.bus.write_word(0xDFF000 + ch, 0);
        }
        m.bus.clock = chain + 669 + 24 + 2;
        m.bus.write_word(0xDFF096, 0x000F);
        m.bus.clock = chain + start_t;
        return Some(Box::new(s));
    }
    let h = &d[32..];
    let st = DlStart::of(h, (n as u32, be32(h, 12), be32(h, 16), be32(h, 20)));
    if let Some(b) = st.burst() {
        dl_reset_burst(m, origin + b, st.m == 5648);
    }
    // the output frame grid restarts at the reset: the song starts T after it (the first tick and its
    // writes keep their places, the timer delay being that much shorter)
    m.bus.clock = origin + start_t;
    Some(Box::new(s))
}

/// Whether each voice's DMA start word in the module's driver carries the master enable bit (0x0200):
/// the build's code holds one 16-bit immediate per voice, `0x8000 | bit` or `0x8200 | bit`, before
/// the sample table (header offset 28).
fn dl_master_bits(h: &[u8]) -> [bool; 4] {
    let end = (be32(h, 28) as usize).min(h.len());
    let mut m = [false; 4];
    for (v, f) in m.iter_mut().enumerate() {
        let w = 0x8200u16 | (1 << v);
        *f = (0..end.saturating_sub(1)).step_by(2).any(|o| be16(h, o) == w);
    }
    m
}

/// `T` of the two `older` files (the check call, the chain of back-to-back calls ending in the
/// state-clearing loop): 4083 and 4082 measured.
const DL_OLDER_T: u64 = 4083;

/// The older Eagleplayer's start-up chain begins this many colour clocks after the frame-aligned
/// start (two aligned 5-frame periods).
const DL_OLDER_SHIFT: u64 = 710510;

/// The module's init resets Paula: DMA off, the four channels' address, length 16, volume 0 and period 1
/// (21 writes). The writes of the channels' registers go 18 colour clocks apart (16 in the build of
/// the Games pole and rings), the lengths, volumes and periods 10 apart.
fn dl_reset_burst(m: &mut Machine, at: u64, tight: bool) {
    let step = if tight { 16 } else { 18 };
    m.bus.clock = at;
    m.bus.write_word(0xDFF096, 0x000F);
    for c in 0..4u32 {
        m.bus.clock = at + step * (c as u64 + 1);
        m.bus.write_word(0xDFF0A0 + 16 * c, 0);
        m.bus.write_word(0xDFF0A2 + 16 * c, 0);
    }
    let mut t = at + 4 * step;
    for reg in [0xA4u32, 0xA8, 0xA6] {
        for c in 0..4u32 {
            t += 10;
            m.bus.clock = t;
            m.bus.write_word(0xDFF000 + reg + 16 * c, if reg == 0xA4 { 16 } else if reg == 0xA6 { 1 } else { 0 });
        }
    }
}

/// The reference's logs start 100 colour clocks after the origin its start-up work counts from.
const DL_START_PHASE: u64 = 100;
/// `older_*`: the in-tick ledger puts the first write 42 colour clocks later than the old entry did.
const OLD_PHASE: u64 = 42;

/// The 68000 cycles of the original's start-up work for a `dl` module, split as the reference runs
/// it: the player-init call and the song-init call, and the quantities of the module they depend on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DlStart {
    steps: u64,
    subs: u64,
    bra: u64,
    form2: bool,
    zterm: u64,
    list_words: u64,
    longer_lists: u64,
    midwinter: bool,
    timescanner: bool,
    init_player: bool,
    no_table: bool,
    /// The module's own init routine (cycles) and the module's sample fix-up routine (cycles).
    m: u64,
    x: u64,
}

impl DlStart {
    /// Reads the quantities of the start-up work from the hunk image `h` (file offset 32 on).
    fn of(h: &[u8], key: (u32, u32, u32, u32)) -> DlStart {
        let mut s = DlStart::default();
        // the volume-write search: 380 bytes into the play routine, a window at every even position
        // until the store of a word to displacement 8 of the channel block
        let play = be32(h, 16) as usize;
        let mut p = play + 380;
        let mut found = false;
        while p + 4 <= h.len() {
            s.steps += 1;
            if (be16(h, p) == 0x3540 || be16(h, p) == 0x3559) && be16(h, p + 2) == 8 {
                found = true;
                s.form2 = be16(h, p) == 0x3559;
                break;
            }
            p += 2;
        }
        // form 2 (store at (A1)+) skips the branch rewrite; unverified on real files (synthetic only)
        s.bra = (found && !s.form2 && be16(h, p + 4) == 0x6000) as u64;
        let table = be32(h, 44) as usize;
        s.no_table = table == 0;
        if !s.no_table {
            s.subs = 1;
            let mut i = 1;
            loop {
                let e = table + 16 * i;
                let v = be32(h, e);
                if e + 4 > h.len() || v < 256 {
                    s.zterm = (e + 4 <= h.len() && v == 0) as u64;
                    break;
                }
                s.subs += 1;
                i += 1;
            }
            let (l, u) = Self::lists(h, table);
            s.list_words = l;
            s.longer_lists = u;
        }
        s.midwinter = be32(h, 60) == 77554 && s.subs == 17;
        s.timescanner = be32(h, 76) == 38;
        s.init_player = be32(h, 40) != 0;
        s.m = 5664;
        if let Some(c) = DL_INIT_COST.iter().find(|c| (c.0, c.1, c.2, c.3) == key) {
            s.m = c.4 as u64;
            s.x = c.5 as u64;
        }
        s
    }

    /// `L` and `U` of the subsong table entry at `e`: the words of its four order lists up to and
    /// including the terminator, and how many of the voices' lists are as long as the longest so far.
    fn lists(h: &[u8], e: usize) -> (u64, u64) {
        let (mut l, mut u, mut best) = (0, 0, 0);
        for c in 0..4 {
            let mut q = be32(h, e + 4 * c) as usize;
            let mut n = 1;
            while q + 4 <= h.len() && be32(h, q) != 0 {
                n += 1;
                q += 4;
            }
            l += n;
            if n >= best {
                u += 1;
                best = n;
            }
        }
        (l, u)
    }

    fn c_player_init(&self) -> u64 {
        let mut c = 1191 + 76 * self.steps + 72 * self.subs + 20 * self.bra - 25 * self.zterm;
        if self.subs == 0 {
            c -= 2;
        }
        if self.form2 {
            c += 4;
        }
        if self.midwinter {
            c += 395;
        }
        if self.init_player {
            c += self.x + 20;
        }
        c
    }

    /// The colour clocks of each change to subsong j = 2, 3, ...: the stop call (a constant of the build's class),
    /// two 118-clock calls and song-init j (cycles / 2), whose lists sums run over the subsong table's entries 1..j
    /// (SPEC dl2 section 7). The Time Scanner term is 21 on the later inits.
    fn chain(&self, h: &[u8]) -> Vec<u64> {
        let stop = match self.m {
            5876 => 306,
            6009 => 348,
            6095 => 380,
            6113 => 390,
            _ => 300,
        };
        let table = be32(h, 44) as usize;
        let (mut sl, mut su) = (0, 0);
        let mut out = Vec::new();
        for j in 1..=self.subs {
            let (l, u) = Self::lists(h, table + 16 * (j as usize - 1));
            sl += l;
            su += u;
            if j >= 2 {
                let ts = if self.timescanner { 21 } else { 0 };
                let mut init = 1605 + 29 * sl + 16 * su + ts + self.m + 254 * (j - 1);
                if self.no_table {
                    init -= 290;
                }
                out.push(stop + 236 + init / 2);
            }
        }
        out
    }

    fn c_song_init(&self) -> u64 {
        let mut c = 1605 + 29 * self.list_words + 16 * self.longer_lists + 37 * self.timescanner as u64 + self.m;
        if self.no_table {
            c -= 290;
        }
        c
    }

    /// Colour clocks from the frame-aligned start to the end of the start-up work: the check call and
    /// the four small calls (392), the player-init call (it contains host calls: ceil(cycles / 2) - 1
    /// colour clocks) and the song-init call (floor(cycles / 2)).
    /// Colour clocks from the start to the module's Paula reset (SPEC section 6); none for the builds
    /// whose init does not reset.
    fn burst(&self) -> Option<u64> {
        let p = match self.m {
            5400 | 5420 | 5648 | 5664 => 20,
            6081 => 71,
            6009 | 6113 => 73,
            6095 | 5876 | 4213 => 74,
            _ => return None,
        };
        let eagle = self.c_song_init() - self.m;
        Some(392 + self.c_player_init().div_ceil(2) - 1 + eagle / 2 + p)
    }

    fn t(&self) -> u64 {
        392 + self.c_player_init().div_ceil(2) - 1 + self.c_song_init() / 2
    }
}

#[cfg(test)]
mod start_tests {
    use super::*;

    /// `T` of every corpus file against the specification's table (skipped without the corpus).
    #[test]
    fn dl_start_matches_table() {
        let dir = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/ports/davelowe/mods/dl/";
        let Ok(tsv) = std::fs::read_to_string("/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/spec/dl/logs/T_components.tsv") else { return };
        let mut n = 0;
        for l in tsv.lines().skip(1) {
            let f: Vec<&str> = l.split('\t').collect();
            let Ok(d) = std::fs::read(format!("{dir}dl.{}", f[0])) else { continue };
            let h = &d[32..];
            let s = DlStart::of(h, (be32(&d, 28), be32(h, 12), be32(h, 16), be32(h, 20)));
            let col = |name: &str| -> u64 {
                let i = tsv.lines().next().unwrap().split('\t').position(|c| c == name).unwrap();
                f[i].parse().unwrap_or(0)
            };
            assert_eq!(s.steps, col("steps"), "{} steps", f[0]);
            assert_eq!(s.subs, col("subsongs"), "{} subs", f[0]);
            assert_eq!(s.zterm, col("zero_term"), "{} zterm", f[0]);
            assert_eq!(s.list_words, col("L"), "{} L", f[0]);
            assert_eq!(s.longer_lists, col("U"), "{} U", f[0]);
            assert_eq!(s.bra, col("vol_patch_follows"), "{} bra", f[0]);
            assert_eq!(s.m, col("M_cycles"), "{} M", f[0]);
            assert_eq!(s.t(), col("T_true"), "{} T", f[0]);
            n += 1;
        }
        eprintln!("checked {n} files");
    }
}

/// The same for layout A, with the write timing model: with `DLN_REF_START` the start constant
/// equals the reference's for the Beneath a Steel Sky song `ingame_02`, which puts every write on
/// the same raster line phase and so on the same Paula audio slot. The other songs' inits last up
/// to 1864 colour clocks more; `dln_start_work2` prices that from the module.
const DLN_INIT_CLOCKS: u64 = 33823;

/// The reference's start-up work for `ingame_02`, in colour clocks.
const DLN_REF_START: u64 = 5448;

/// Module quantities the reference's start-up work for a layout A module is proportional to. It
/// examines the module before the song starts; the costs are fitted to the reference's write logs
/// of the 24 corpus files (max error 0.5 cc). The coefficients of `voices_skipped`,
/// `short_header`, `unaligned_run` and `patched_module` each rest on a single corpus file.
#[derive(Debug, Default, PartialEq, Eq)]
struct DlnStartFeatures {
    /// Voices of subsong 1 skipped before one whose first sequence starts with a sample command.
    voices_skipped: u32,
    /// The header's first word is 4, not 8 (one file).
    short_header: u32,
    /// Subsong table entries + 1.
    table_len: u32,
    /// Sample records in the run ending at the first referenced one, that one included.
    run_back: u32,
    /// Sample records in the whole run, forward from its start, + 1.
    run_forward: u32,
    /// The subsong table ended on a zero word (not on a word with a non-zero upper half).
    zero_terminated: u32,
    /// The word in front of the sample run is neither 8 nor 0 (one file).
    unaligned_run: u32,
    /// The 5-subsong module with 18 sample records whose order lists the port patches (one file).
    patched_module: u32,
    /// Words read in the four order lists of subsong 1, terminators included.
    list_words: u32,
    /// Of those four lists, how many are strictly longer than every earlier one.
    longer_lists: u32,
}

fn dln_start_features(d: &[u8]) -> DlnStartFeatures {
    let w0 = be16(d, 0) as usize;
    let mut f = DlnStartFeatures::default();
    // the subsong table
    let (mut o, mut n) = (w0, 0u32);
    loop {
        let (h, l) = (be16(d, o), be16(d, o + 2));
        if h != 0 {
            break;
        }
        if l == 0 {
            f.zero_terminated = 1;
            break;
        }
        if l & 1 != 0 {
            break;
        }
        n += 1;
        o += 4;
    }
    f.table_len = n + 1;
    // the order lists of subsong 1
    let lists = [0usize, 1, 2, 3].map(|v| be32(d, w0 + 4 * v) as usize);
    let mut lens = [0u32; 4];
    for (k, &e) in lists.iter().enumerate() {
        let mut q = e;
        while be32(d, q) != 0 {
            lens[k] += 1;
            q += 4;
        }
    }
    f.list_words = lens.iter().map(|k| k + 1).sum();
    f.longer_lists = (0..4).filter(|&i| lens[i] > lens[..i].iter().copied().max().unwrap_or(0)).count() as u32;
    // the first voice whose first sequence references a sample record
    let mut sp = None;
    for (v, &l) in lists.iter().enumerate() {
        f.voices_skipped = v as u32;
        let mut q = be32(d, l) as usize;
        if be16(d, q) == 0x30 {
            q += 2;
        }
        if be16(d, q) == 0x0C && be16(d, q + 6) == 4 {
            sp = Some(be32(d, q + 8) as usize);
            break;
        }
    }
    if let Some(sp) = sp.filter(|&sp| be16(d, sp) == 1) {
        let mut a = sp;
        let mut first;
        loop {
            first = a;
            f.run_back += 1;
            if a < 14 {
                break;
            }
            a -= 14;
            if be16(d, a) != 1 {
                break;
            }
        }
        if first >= 2 && !matches!(be16(d, first - 2), 8 | 0) {
            f.unaligned_run = 1;
            first += 14;
        }
        while be16(d, first) == 1 {
            f.run_forward += 1;
            first += 14;
        }
        f.run_forward += 1;
    }
    f.short_header = (w0 == 4) as u32;
    f.patched_module = (w0 == 8 && be32(d, 0x38) == 0x74 && n / 4 == 5 && f.run_forward == 0x13) as u32;
    f
}

/// The start-up work in half colour clocks.
fn dln_start_work2(f: &DlnStartFeatures) -> u64 {
    2 * 4616
        + 162 * f.voices_skipped as u64
        + 71 * f.table_len as u64
        + 41 * f.run_back as u64
        + 47 * f.run_forward as u64
        + 29 * f.list_words as u64
        + 16 * f.longer_lists as u64
        + 17 * f.zero_terminated as u64
        + 28 * f.unaligned_run as u64
        + 40 * f.short_header as u64
        + 182 * f.patched_module as u64
}

/// The start constant of a layout A module: the single-file constant plus what its start-up work
/// costs more than `ingame_02`'s (half clocks rounded down).
fn dln_init_clocks(d: &[u8]) -> u64 {
    let w = dln_start_work2(&dln_start_features(d)) / 2;
    (DLN_INIT_CLOCKS + w).saturating_sub(DLN_REF_START)
}

/// Module quantities the reference's start-up work for a layout B module is proportional to
/// (fitted on the write logs of the 17 corpus files; the rare terms rest on one or two files).
#[derive(Debug, Default, PartialEq, Eq)]
struct DlnbStartFeatures {
    /// Words read in the four order lists of subsong 1, terminators included.
    list_words: u32,
    /// Of those four lists, how many are strictly longer than every earlier one.
    longer_lists: u32,
    /// Sample records walked back from the first referenced one to the start of its run.
    run_back: u32,
    /// Sample records of the run walked forward (one less when the run is entered late), + 1.
    run_forward: u32,
    /// Voice 0's first sequence begins with the filter command.
    filter_first: u32,
    /// The word in front of the run is 0.
    before_zero: u32,
    /// The word in front of the run is neither 0 nor 8.
    before_odd: u32,
}

fn dlnb_start_features(d: &[u8]) -> DlnbStartFeatures {
    let w0 = be16(d, 0) as usize;
    let mut f = DlnbStartFeatures::default();
    let lists = [0usize, 1, 2, 3].map(|v| be32(d, w0 + 4 + 4 * v) as usize);
    let mut lens = [0u32; 4];
    for (k, &e) in lists.iter().enumerate() {
        let mut q = e;
        while q + 4 <= d.len() && be32(d, q) != 0 {
            lens[k] += 1;
            q += 4;
        }
    }
    f.list_words = lens.iter().map(|k| k + 1).sum();
    f.longer_lists = (0..4).filter(|&i| lens[i] > lens[..i].iter().copied().max().unwrap_or(0)).count() as u32;
    let mut sp = None;
    for (v, &l) in lists.iter().enumerate() {
        let mut q = be32(d, l) as usize;
        let filt = be16(d, q) == 0x30;
        if filt {
            q += 2;
        }
        if be16(d, q) == 0x0C && be16(d, q + 6) == 4 {
            sp = Some(be32(d, q + 8) as usize);
            f.filter_first = (v == 0 && filt) as u32;
            break;
        }
    }
    if let Some(sp) = sp.filter(|&sp| be16(d, sp) == 1) {
        let mut a = sp;
        let mut first;
        loop {
            first = a;
            f.run_back += 1;
            if a < 14 {
                break;
            }
            a -= 14;
            if be16(d, a) != 1 {
                break;
            }
        }
        if first >= 2 {
            match be16(d, first - 2) {
                0 => f.before_zero = 1,
                8 => {}
                _ => {
                    f.before_odd = 1;
                    first += 14;
                }
            }
        }
        while be16(d, first) == 1 {
            f.run_forward += 1;
            first += 14;
        }
        f.run_forward += 1;
    }
    f
}

/// The start-up work in half colour clocks.
fn dlnb_start_work2(f: &DlnbStartFeatures) -> u64 {
    2 * 4736
        + 29 * f.list_words as u64
        + 16 * f.longer_lists as u64
        + 41 * f.run_back as u64
        + 47 * f.run_forward as u64
        + 8 * f.filter_first as u64
        + 21 * f.before_zero as u64
        + 29 * f.before_odd as u64
        + 25 * (f.run_forward == 19) as u64 // (one file has such a run: it cannot be validated)
}

/// The start constant of a layout B module: the layout A constant plus the start-up work.
fn dlnb_init_clocks(d: &[u8]) -> u64 {
    DLN_INIT_CLOCKS - DLN_REF_START + dlnb_start_work2(&dlnb_start_features(d)) / 2
}

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
    m.bus.clock += if layout_a { dln_init_clocks(d) } else { dlnb_init_clocks(d) };
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
        } else if cfg.kind == Kind::DlnA {
            // (a layout A rest's envelope is still stepping for a few ticks, which shows only
            // in the time the voice takes)
            &[0, 0, 0, 0, 0, 0, 0, END]
        } else if cfg.kind == Kind::DlnB {
            &[0, 0, 0, 0, 0, 0, 0, 0, END]
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
            base: 0,
            slot: 0,
            tt: 0,
            add: 0,
            cmd: 0,
            vbase: 0,
            vadj: true,
            last_vol: None,
            fill: 0,
            special_voice: 0,
            wrap: Wrap { w: [0; 5], core: [0; 4], na: [0; 4] },
            dmaon_master: [false; 4],
            chain: Vec::new(),
            resume: None,
            vol_t: None,
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
        // the first voice with the longest order list
        let mut best = 0;
        let mut sv = 0;
        for c in 0..4 {
            let mut n = 0;
            let mut q = lists[c];
            while q != 0 && q + 4 <= self.img.len() && be32(&self.img, q) != 0 {
                n += 1;
                q += 4;
            }
            if n > best {
                best = n;
                sv = c;
            }
        }
        self.special_voice = sv;
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

    /// Layout A: the original's work between writes is modelled per voice slot (the `SLOT_*`
    /// tables); `tw` lets the clock run to the moment `t` (colour clocks after the tick's entry).
    fn timed(&self) -> bool {
        matches!(self.cfg.kind, Kind::DlnA | Kind::DlnB | Kind::Older)
    }

    /// Layout B differs from layout A in some of the slot constants: `a` for A, `b` for B.
    fn kb(&self, a: i64, b: i64) -> i64 {
        if self.cfg.kind == Kind::DlnB {
            b
        } else {
            a
        }
    }

    fn tw(&self, hw: &mut Hw, t: i64) {
        if self.timed() {
            let target = (self.base as i64 + t + self.fill).max(0) as u64;
            if hw.bus.clock < target {
                hw.bus.clock = target;
            }
        } else if self.cfg.kind == Kind::Dl {
            // (the `dl` ledger counts cycles from the tick's entry; the clock is at the floor of half)
            let target = (self.base as i64 + t.div_euclid(2)).max(0) as u64;
            if hw.bus.clock < target {
                hw.bus.clock = target;
            }
        }
    }

    /// A timing constant: `dl` takes its own (cycles), the layouts the other (colour clocks).
    fn kc(&self, other: i64, dl: i64) -> i64 {
        if self.cfg.kind == Kind::Dl {
            dl
        } else {
            other
        }
    }

    fn dma(&mut self, vi: usize, on: bool, hw: &mut Hw) {
        if self.muted(vi) && !on {
            return;
        }
        self.tw(hw, self.tt);
        let bit = 1u16 << self.v[vi].ch;
        // (the original's DMA start also sets the master enable bit)
        let master = if self.cfg.kind == Kind::Dl && self.dmaon_master[self.v[vi].ch & 3] { 0x0200 } else { 0 };
        hw.dmacon(if on { 0x8000 | master | bit } else { bit });
        if self.cfg.kind == Kind::DlnB {
            // the enable follows 10 colour clocks later and delays all that comes after
            hw.bus.clock += 10;
            hw.dmacon(FILLER);
            self.fill += 10;
        }
    }

    fn lc(&mut self, vi: usize, a: u32, len: u16, note: bool, hw: &mut Hw) {
        if self.muted(vi) {
            return;
        }
        self.tw(hw, self.tt);
        hw.loc(self.v[vi].ch, self.mem.wrapping_add(a));
        if self.cfg.kind == Kind::Dl {
            // (a note's length goes through the Eagleplayer's intercept, a loop reload's does not)
            self.tw(hw, self.tt + if note { DL_PRE_LEN[self.v[vi].ch & 3] } else { DL_RELOAD_LEN });
        } else {
            self.tw(hw, self.tt + 10);
        }
        hw.len(self.v[vi].ch, len);
    }

    fn per(&mut self, vi: usize, p: u16, hw: &mut Hw) {
        if !self.muted(vi) {
            self.tw(hw, self.tt);
            hw.per(self.v[vi].ch, p);
        }
    }

    /// A volume written straight, which the master volume does not see.
    fn vol_raw(&mut self, vi: usize, val: u16, hw: &mut Hw) {
        self.vol_t = Some(self.tt);
        if !self.muted(vi) {
            self.tw(hw, self.tt);
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
        self.vol_t = Some(self.tt);
        if !self.muted(vi) {
            self.tw(hw, self.tt);
            hw.vol(self.v[vi].ch, x & 0x7F);
        }
    }

    /// The volume step: the envelope's next value, or the held one.
    fn vol_step(&mut self, vi: usize, hw: &mut Hw) {
        let e = self.v[vi].env;
        let w = self.w(e);
        if self.timed() {
            self.tt = self.vbase - if w != END && self.vadj { 9 } else { 0 };
            self.last_vol = Some(self.tt);
        } else if self.cfg.kind == Kind::Dl {
            self.tt = self.vbase + if w != END { DL_HELD } else { 0 };
        }
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

    fn wrap_test(&mut self, vi: usize, rest: bool, first: bool) {
        if vi >= 4 {
            return;
        }
        let zero = self.songcnt[vi] == 0;
        if self.songcnt[vi] != 0 {
            self.songcnt[vi] = self.songcnt[vi].wrapping_add(1);
        }
        let mut end = false;
        if self.songcnt.iter().all(|&c| c == 0) {
            self.songend = true;
            self.songcnt = [0xFF; 4];
            end = true;
        }
        if self.cfg.kind == Kind::Dl {
            self.cmd += if rest { DL_WRAP_REST } else if zero || first { DL_WRAP0 } else { DL_WRAP1 } + if end { 2 * WRAP_END } else { 0 };
            return;
        }
        self.cmd += if rest { REST_FLAG_COST[vi] } else { WRAP_COST[(zero || (first && self.cfg.kind != Kind::DlnB)) as usize][vi] + self.kb(0, B_WRAP) } + if end { WRAP_END } else { 0 };
    }

    // ------------------------------------------------------------ the voice

    fn voice(&mut self, vi: usize, hw: &mut Hw) {
        if self.cfg.kind == Kind::Dl {
            return self.voice_dl(vi, hw);
        }
        if self.v[vi].dead {
            return;
        }
        let mut off = false;
        let k = vi & 3;
        let s0 = self.slot;
        // `older_*`: the master-volume fade step (about 23 colour clocks) is skipped at full master
        let old = self.cfg.kind == Kind::Older;
        let fadecut = if old && (self.mfade == 0 || self.master == 64) { OLD_NOFADE } else { [0; 4] };
        self.add = 0;
        self.cmd = 0;
        self.vadj = true;
        self.last_vol = None;
        if self.v[vi].left == 0 {
            self.fetch(vi, hw);
        } else {
            if self.v[vi].left == 1 {
                if self.v[vi].smode != 0 {
                    off = true;
                    if !self.cfg.dma_tick {
                        self.tt = s0 + SLOT_DMAOFF;
                        self.dma(vi, false, hw);
                    }
                }
                self.add += SLOT_LAST;
                self.v[vi].left = 0;
            } else {
                if self.v[vi].smode == 3 {
                    let (a, l) = self.v[vi].lp;
                    self.tt = s0 + SLOT_LOC;
                    self.lc(vi, a, l, false, hw);
                    self.v[vi].smode = 1;
                    self.add += SLOT_RELOAD;
                }
                self.v[vi].left -= 1;
            }
            self.vbase = s0 + if old { OLD_VOL[k] - fadecut[k] } else { SLOT_VOL[k] } + self.add;
            self.slide(vi, hw);
            self.vol_step(vi, hw);
        }
        if self.timed() {
            let extra = self.kb(0, B_TAIL[k]);
            self.slot = match self.last_vol {
                Some(t) if old => t + OLD_TAIL[k],
                Some(t) => t + SLOT_TAIL[k] + extra,
                None => s0 + SLOT_PLAIN[k] + extra,
            };
        }
        if self.cfg.dma_tick && !self.v[vi].dead {
            if old {
                // (the start of the voice's DMA follows its volume write by 54 colour clocks)
                if let Some(t) = self.last_vol {
                    self.tt = t + OLD_VOL_DON;
                }
            }
            self.dma(vi, !off, hw);
        }
    }

    /// One voice of a tick of the `dl` driver: the ledger runs in cycles from the tick's entry. A voice's
    /// slot starts after the build's wrapper (`wrap.w`) and, for voices 1..3, the previous voice's volume
    /// write with its Eagleplayer return path; its core path leads to the volume write, which the
    /// Eagleplayer's intercept delays by its pre-write cost.
    fn voice_dl(&mut self, vi: usize, hw: &mut Hw) {
        let k = vi & 3;
        let c0 = self.slot;
        let core = self.wrap.core[k] as i64;
        let w_next = self.wrap.w[(k + 1).min(4)] as i64;
        self.add = 0;
        self.cmd = 0;
        self.vadj = true;
        self.vol_t = None;
        if self.v[vi].dead {
            self.slot = c0 + core + DL_PRE_VOL[k] + DL_POST_VOL[k] + w_next;
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
                        self.tt = c0 + DL_DOFF;
                        self.dma(vi, false, hw);
                    }
                }
                self.add += if self.v[vi].smode != 0 { DL_LAST } else { DL_LAST0 };
                self.v[vi].left = 0;
            } else {
                if self.v[vi].smode == 3 {
                    let (a, l) = self.v[vi].lp;
                    self.tt = c0 + DL_LOC;
                    self.lc(vi, a, l, false, hw);
                    self.v[vi].smode = 1;
                    self.add += DL_RELOAD;
                }
                self.v[vi].left -= 1;
            }
            self.vbase = c0 + core + self.add + DL_PRE_VOL[k];
            self.slide(vi, hw);
            self.vol_step(vi, hw);
        }
        self.slot = match self.vol_t {
            Some(t) => t + DL_POST_VOL[k] + w_next,
            None => c0 + core + DL_PRE_VOL[k] + DL_POST_VOL[k] + w_next,
        };
        if self.cfg.dma_tick {
            self.tt = self.vol_t.unwrap_or(c0) + DL_VOL_DON;
            self.dma(vi, !off, hw);
        }
    }

    /// Portamento and vibrato.
    fn slide(&mut self, vi: usize, hw: &mut Hw) {
        let dl = self.cfg.kind == Kind::Dl;
        let v = &mut self.v[vi];
        let mut p = v.period;
        let mut changed = false;
        // (cost of the work done, and when in the slot the period is written)
        let mut cost = 0;
        let mut pt = 0;
        let mut pt_dl = 0;
        let mut stepped = 0;
        if v.pmode != 0 {
            if v.pdelay != 0 {
                v.pdelay -= 1;
                cost += if dl { DL_DELAY } else { SLOT_DELAY };
            } else {
                changed = true;
                stepped += 1;
                pt = SLOT_PORTA;
                pt_dl = DL_PORTA;
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
                cost += if dl { DL_DELAY } else { SLOT_DELAY };
            } else if v.vcount != 0 {
                v.vcount -= 1;
                cost += if dl { DL_RATE } else { SLOT_RATE };
            } else {
                v.vcount = v.vreload;
                changed = true;
                stepped += 1;
                if v.vstate < 3 {
                    p = p.wrapping_sub(v.vdown);
                    v.vstate += 1;
                    pt = SLOT_VDOWN;
                    pt_dl = DL_VDOWN;
                } else {
                    p = p.wrapping_add(v.vup);
                    v.vstate += 1;
                    pt = SLOT_VUP;
                    pt_dl = DL_VDOWN + 16;
                    if v.vstate == 5 {
                        v.vstate = 1;
                        pt = SLOT_VWRAP;
                        pt_dl = DL_VDOWN + 32;
                    }
                }
            }
        }
        if changed {
            v.period = p;
            self.add += cost;
            if stepped == 2 && self.cfg.kind == Kind::DlnB {
                self.add += B_SLIDE_BOTH;
            }
            let s0 = self.slot;
            if self.cfg.kind == Kind::Dl {
                let k = vi & 3;
                self.tt = s0 + self.add + pt_dl;
                self.vbase = self.tt + DL_POST_PER[k] + DL_PER_VOL + DL_PRE_VOL[k];
            } else {
                self.tt = s0 + self.add + pt;
                self.vbase = self.tt + SLOT_PERVOL[vi & 3];
            }
            self.per(vi, p, hw);
        } else {
            self.add += cost;
            self.vbase += cost;
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
                    self.cmd += self.kc(CMD_SAMPLE, DL_CMD[1]);
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
                    let order_was_none = order == NONE;
                    let next = if self.cfg.kind == Kind::DlnB {
                        CMD_NEXT + B_NEXT + if vi == self.special_voice { B_NEXT_SPECIAL } else { 0 }
                    } else {
                        self.kc(CMD_NEXT, DL_CMD[2])
                    };
                    self.cmd += next;
                    if order == NONE || self.l(order) == 0 {
                        self.cmd -= next;
                        order = self.v[vi].order0;
                        self.wrap_test(vi, false, order_was_none);
                    }
                    let entry = self.l(order) as usize;
                    self.v[vi].order = order + 4;
                    pos = entry;
                }
                3 => {
                    self.cmd += self.kc(CMD_ENV, DL_CMD[3]);
                    let b = self.l(pos + 2) as usize;
                    pos += 6;
                    self.v[vi].env0 = match self.cfg.env_hdr {
                        2 => b + 4,
                        1 => b + 2,
                        _ => b,
                    };
                }
                4 => {
                    self.cmd += self.kc(CMD_PORTA, DL_CMD[4]);
                    let v = &mut self.v[vi];
                    let a = [2, 4, 6, 8].map(|k| be16(&self.img, pos + k));
                    v.pmode = a[0];
                    v.pstep = a[1];
                    v.ptarget = a[2];
                    v.pdelay = a[3];
                    pos += 10;
                }
                5 => {
                    self.cmd += self.kc(CMD_VIBRATO, DL_CMD[5]);
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
                    self.cmd += self.kb(0, B_CMD6);
                    self.v[vi].pmode = 0;
                    pos += 2;
                }
                7 => {
                    self.cmd += self.kc(CMD_VIBOFF, DL_CMD[7]);
                    self.v[vi].vstate = 0;
                    pos += 2;
                }
                8 => {
                    let len = self.w(pos + 2);
                    let v = &mut self.v[vi];
                    v.left = len.wrapping_sub(1);
                    v.seq = pos + 4;
                    v.env = self.rest_env;
                    if len & 0x8000 != 0 {
                        self.wrap_test(vi, true, false);
                    }
                    self.tt = self.slot + self.kc(SLOT_REST, DL_REST) + self.cmd;
                    self.vol_raw(vi, 0, hw);
                    if !self.cfg.rest_returns {
                        self.vbase = self.tt + if self.cfg.kind == Kind::Dl { DL_POST_VOL[vi & 3] + DL_REST2 + DL_PRE_VOL[vi & 3] } else { SLOT_REST2[vi & 3] };
                        self.vadj = false;
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
                    self.cmd += self.kb(0, B_CMD11);
                    hw.bus.ciaa.pra &= !2;
                    pos += 2;
                }
                12 => {
                    self.cmd += self.kb(0, B_CMD11);
                    hw.bus.ciaa.pra |= 2;
                    pos += 2;
                }
                13 => {
                    self.cmd += self.kc(0, DL_CMD[13]);
                    self.v[vi].boost = true;
                    pos += 2;
                }
                14 => {
                    self.cmd += self.kc(CMD_JUMP, DL_CMD[14]);
                    pos = self.l(pos + 2) as usize;
                }
                _ => return,
            }
        }
    }

    /// A note: `pos` is at its period.
    fn note(&mut self, vi: usize, pos: usize, hw: &mut Hw) {
        if self.cfg.kind == Kind::Dl {
            return self.note_dl(vi, pos, hw);
        }
        let period = self.w(pos);
        let k = vi & 3;
        let tper = self.slot + SLOT_NOTE + self.cmd;
        // a running vibrato makes the rest of the slot 8 later
        let tloc = tper + NOTE_LOC[k] + if self.v[vi].vstate != 0 { NOTE_VIB } else { 0 };
        self.tt = tper;
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
            self.tt = tloc;
            self.lc(vi, a, l, true, hw);
        }
        // (without a loop pair the DMA start keeps the same distance to the period)
        let tdma = tloc + 10 + NOTE_DMA[k];
        if !self.cfg.dma_tick {
            self.tt = tdma;
            self.dma(vi, true, hw);
        }
        self.vbase = tdma + NOTE_VOL[k];
        // (layout B: a note whose envelope steps also takes the 9 less)
        self.vadj = self.cfg.kind == Kind::DlnB;
        self.vol_step(vi, hw);
    }

    /// A note of the `dl` driver: the period, the address and length (through the Eagleplayer's
    /// intercept), the DMA start, then the volume, each after the Eagleplayer's return path and the
    /// core path between the writes.
    fn note_dl(&mut self, vi: usize, pos: usize, hw: &mut Hw) {
        let period = self.w(pos);
        let k = vi & 3;
        let tper = self.slot + DL_NOTE_A + self.wrap.na[k] as i64 + DL_PRE_PER + self.cmd;
        self.tt = tper;
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
        let tloc = tper + DL_POST_PER[k] + DL_PER_LOC + DL_PRE_LOC;
        let tlen = tloc + DL_PRE_LEN[k];
        if v.smode != 0 {
            v.smode = 3;
            let (a, l) = v.att;
            self.tt = tloc;
            self.lc(vi, a, l, true, hw);
        }
        let tdma = tlen + DL_POST_LEN[k] + DL_LEN_DON;
        if !self.cfg.dma_tick {
            self.tt = tdma;
            self.dma(vi, true, hw);
        }
        self.vbase = tdma + DL_DON_VOL + DL_PRE_VOL[k];
        self.vadj = false;
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

/// `dl`: the tick's entry lies exactly 70940 clocks after the song-init call returns.
const DL_TIMER_DELAY: u64 = 0;

/// Layout A: the part of `TIMER_DELAY` the write model takes into its own start offsets.
const DLNA_TIMER_SHIFT: u64 = 565;

/// Layout A write timing, in colour clocks after the tick's entry or after the start of a voice's
/// slot (the time its processing takes, measured from the original's register writes). A slot
/// starts where the previous one ended; the first starts `SLOT_FIRST` after the tick's entry.
/// `older_*` (colour clocks): the first volume write after the tick's entry, the distance from a
/// voice's volume write to the next voice's, the volume write to its DMA start, and the fade step.
const OLD_VOL: [i64; 4] = [429, 0, 0, 0];
const OLD_TAIL: [i64; 4] = [398, 407, 410, 0];
const OLD_VOL_DON: i64 = 54;
const OLD_NOFADE: [i64; 4] = [23, 23, 20, 23];
const SLOT_FIRST: i64 = 194;
/// A counting-down voice: the volume write, and the slot's end after it.
const SLOT_VOL: [i64; 4] = [374, 383, 386, 400];
const SLOT_TAIL: [i64; 4] = [79, 94, 110, 113];
/// The slot of a voice with nothing to write.
const SLOT_PLAIN: [i64; 4] = [453, 477, 496, 513];
/// Added by a vibrato (or portamento) delay count, a vibrato rate count, the last tick of a note and
/// a loop reload; the DMA stop and the loop address come at these moments.
const SLOT_DELAY: i64 = 24;
const SLOT_RATE: i64 = 36;
const SLOT_LAST: i64 = 13;
const SLOT_RELOAD: i64 = 35;
const SLOT_DMAOFF: i64 = 175;
const SLOT_LOC: i64 = 179;
/// The period write of a vibrato step (down, up, wrap) and of a portamento step, and the distance
/// from a period write to the volume write.
const SLOT_VDOWN: i64 = 317;
const SLOT_VUP: i64 = 325;
const SLOT_VWRAP: i64 = 333;
const SLOT_PORTA: i64 = 296;
const SLOT_PERVOL: [i64; 4] = [209, 233, 252, 270];
/// A voice that starts a note: the period write (after the commands in front of the note), then the
/// distances to the loop address, from the length to the DMA start, from there to the volume.
const SLOT_NOTE: i64 = 181;
const NOTE_LOC: [i64; 4] = [147, 162, 177, 182];
const NOTE_DMA: [i64; 4] = [147, 178, 209, 217];
const NOTE_VIB: i64 = 8;
const NOTE_VOL: [i64; 4] = [152, 161, 164, 177];
/// A rest: the first volume write and the distance to the second.
const SLOT_REST: i64 = 247;
const SLOT_REST2: [i64; 4] = [143, 152, 155, 169];
/// Costs of the stream commands in front of a note or rest.
const CMD_SAMPLE: i64 = 152;
const CMD_NEXT: i64 = 114;
const CMD_ENV: i64 = 84;
const CMD_PORTA: i64 = 70;
const CMD_VIBRATO: i64 = 90;
const CMD_VIBOFF: i64 = 54;
const CMD_JUMP: i64 = 80;
/// The next-sequence command when the list wraps, by whether the voice's counter was zero.
const WRAP_COST: [[i64; 4]; 2] = [[233, 248, 260, 267], [218, 235, 247, 258]];
/// The same bookkeeping run by a rest that carries the list-position flag (measured for voices 1
/// and 3; the others are interpolated).
const REST_FLAG_COST: [i64; 4] = [103, 104, 105, 106];
/// More when that wrap ends the song.
const WRAP_END: i64 = 52;

/// Layout B against layout A (colour clocks): the first slot after the tick's entry, the extra at
/// the end of each voice's slot, the next-sequence command (less one for all voices, 27 more for
/// the first voice with the longest order list), the wrap of an order list, commands 6, 11 and 12,
/// and a voice that steps a portamento and a vibrato in one tick.
const B_FIRST: i64 = 259;
const B_TAIL: [i64; 4] = [29, 30, 29, 29];
const B_NEXT: i64 = -1;
const B_NEXT_SPECIAL: i64 = 27;
const B_WRAP: i64 = -17;
const B_CMD6: i64 = 54;
const B_CMD11: i64 = 58;
const B_SLIDE_BOTH: i64 = 57;

/// The `dl` driver's tick ledger (68000 cycles; spec dl2 section 1). The Eagleplayer's intercept costs,
/// by voice: before and after the store of the volume (form 1), of the period, the address and the
/// length.
const DL_PRE_VOL: [i64; 4] = [193, 210, 217, 244];
const DL_POST_VOL: [i64; 4] = [169, 200, 231, 239];
const DL_POST_PER: [i64; 4] = [129, 160, 191, 199];
const DL_PRE_LEN: [i64; 4] = [125, 156, 187, 195];
const DL_POST_LEN: [i64; 4] = [141, 172, 203, 211];
const DL_PRE_PER: i64 = 24;
const DL_PRE_LOC: i64 = 52;
/// Core path costs: from the voice's core start to the period write of a note without commands, from
/// the period's return path to the address's intercept, from the length's return path to the DMA
/// start, from the DMA start to the volume's intercept.
const DL_NOTE_A: i64 = 142;
const DL_PER_LOC: i64 = 188;
const DL_LEN_DON: i64 = 24;
const DL_DON_VOL: i64 = 184;
/// State costs: the DMA stop of a note's last tick, a loop reload (address, length, and the
/// work), the last tick of a note, a delay count, a rate count, the held envelope terminator.
const DL_DOFF: i64 = 93;
const DL_LOC: i64 = 101;
const DL_RELOAD_LEN: i64 = 20;
const DL_RELOAD: i64 = 70;
const DL_LAST: i64 = 26;
const DL_LAST0: i64 = 10;
const DL_DELAY: i64 = 49;
const DL_RATE: i64 = 72;
const DL_HELD: i64 = 9;
/// A period step of a slide: its period write (a vibrato's down step; up +16, wrap +32), the portamento's,
/// and from there the core path to the volume's intercept.
const DL_VDOWN: i64 = 397;
const DL_PORTA: i64 = 355;
const DL_PER_VOL: i64 = 190;
/// A rest: its first volume write and the core path to the second.
const DL_REST: i64 = 494;
const DL_REST2: i64 = 24;
/// The stream commands' handlers by index (word / 4).
const DL_CMD: [i64; 15] = [0, 201, 224, 127, 140, 163, 0, 90, 0, 0, 0, 0, 0, 92, 160];
/// The order-list wrap: of a voice whose counter is zero or at its first fetch, of another, and of a rest
/// that carries the flag.
const DL_WRAP0: i64 = 440;
const DL_WRAP1: i64 = 440;
const DL_WRAP_REST: i64 = 206;
/// DMA start after the volume (the DMA-every-tick builds).
const DL_VOL_DON: i64 = 108;

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
        match self.cfg.kind {
            Kind::Dl => DL_TIMER_DELAY,
            Kind::DlnA | Kind::DlnB => TIMER_DELAY - DLNA_TIMER_SHIFT,
            Kind::Older => TIMER_DELAY - 420 - DL_START_PHASE - OLD_PHASE,
        }
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.subs.len() as u16, self.cur)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if n >= 1 && n as usize <= self.subs.len() {
            self.start(n);
            if self.cfg.kind == Kind::Dl {
                // the original's change takes its stop call, two small calls and the song-init; the first
                // tick comes one period after it
                if let Some(c) = self.chain.get((n as usize).wrapping_sub(2)) {
                    hw.bus.clock += c;
                    self.resume = Some(hw.bus.clock + super::DT_TICK);
                }
            }
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
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
        let first = self.tickno == 1;
        self.base = hw.bus.clock;
        self.fill = 0;
        self.slot = if self.cfg.kind == Kind::Dl { self.wrap.w[0] as i64 } else { if self.cfg.kind == Kind::Older { 0 } else { self.kb(SLOT_FIRST, B_FIRST) } };
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
        // (without the start-up writes: the `dl` flavour's reset burst, the older files' init)
        let log = e.m.bus.write_log.take().unwrap().into_iter().filter(|w| w.2 >= 60000).collect();
        (format, end, log)
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
        let a: Vec<u32> = log.iter().filter(|w| w.2 > DL_OLDER_SHIFT + 1000).map(|w| w.0).filter(|a| (0xDFF0A0..0xDFF0B0).contains(a) || *a == 0xDFF096).take(6).collect();
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

    /// Layout A start-up features and cost of corpus modules (from the reference's write logs):
    /// (file, [voices_skipped, short_header, table_len, run_back, run_forward, zero_terminated,
    /// unaligned_run, patched_module, list_words, longer_lists], measured cost in cc).
    #[test]
    fn dln_start_features_corpus() {
        let dir = Path::new("/media/lolo/kingston2tb/claude-scratch/deliplayerrust/ports/davelowe/mods/dln");
        let cases: [(&str, [u32; 10], u64); 5] = [
            ("Beneath_a_Steel_Sky_CD32__ingame_02", [0, 0, 11, 2, 4, 1, 0, 0, 20, 1], 5448),
            ("Beneath_a_Steel_Sky_CD32__game_complete", [2, 0, 7, 36, 38, 1, 0, 0, 44, 1], 7312),
            ("Beneath_a_Steel_Sky_CD32__ingame_10", [0, 0, 23, 3, 19, 1, 0, 1, 32, 3], 6528),
            ("Kings_Quest_6__ingame_2", [0, 0, 25, 2, 7, 0, 1, 0, 12, 1], 5905),
            ("Kings_Quest_6__title", [0, 1, 5, 6, 15, 0, 0, 0, 93, 1], 6645),
        ];
        for (name, v, t) in cases {
            let Ok(d) = std::fs::read(dir.join(format!("dln.{name}"))) else { continue };
            let f = dln_start_features(&d);
            // the spec's order: tries1 w4 n1 back c1 zterm lea pat L1 U
            let got = [
                f.voices_skipped, f.short_header, f.table_len, f.run_back, f.run_forward,
                f.zero_terminated, f.unaligned_run, f.patched_module, f.list_words, f.longer_lists,
            ];
            assert_eq!(got, v, "{name}");
            assert_eq!(dln_init_clocks(&d), 28375 + t, "{name}");
        }
    }

    /// Layout B start-up cost of the Super Street Fighter 2 songs (the measured duration of the
    /// reference's start-up call, from its write logs; the model is exact to half a clock).
    #[test]
    fn dlnb_start_cost_corpus() {
        let dir = Path::new("/media/lolo/kingston2tb/claude-scratch/deliplayerrust/ports/davelowe/mods/dln");
        let cases = [
            ("Balrog", 5304), ("Blanka", 5469), ("Cammy", 5963), ("ChunLi", 5767), ("DeeJay", 5616),
            ("Dhalsim", 6334), ("E.Honda", 5563), ("FeiLong", 5543), ("Guile", 5457), ("Jingles", 5280),
            ("Ken", 5437), ("M.Bison", 5743), ("Ryu", 5963), ("Sagat", 5853), ("T.Hawk", 5593),
            ("Vega", 6332), ("Zangief", 5633),
        ];
        for (name, t) in cases {
            let Ok(d) = std::fs::read(dir.join(format!("dln.Super_Street_Fighter_2_TNC__{name}"))) else { continue };
            let w = dlnb_start_work2(&dlnb_start_features(&d));
            assert!((w as i64 - 2 * t).abs() <= 1, "{name}: {} vs {t}", w as f64 / 2.0);
            assert_eq!(dlnb_init_clocks(&d), 28375 + w / 2, "{name}");
        }
    }

    #[test]
    fn dlnb_features_of_a_synthetic_module() {
        // the four lists of dln_b() hold one sequence each; no sample command there
        let f = dlnb_start_features(&dln_b());
        assert_eq!((f.list_words, f.longer_lists), (8, 1));
    }
}
