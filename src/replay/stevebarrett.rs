//! Steve Barrett (`sb.*`), the music driver of four of Steve Barrett's game soundtracks
//! (Kelly X, MUNGOMUSIC, Artificial Dreams, Advanced Ski Simulator, scape, and the Wally
//! Beben build of Street Fighter).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no original
//! code. The specification was drawn up from the Paula register writes of UADE's Steve
//! Barrett player and from the structure of the corpus files, and this player was written
//! from it alone and checked against those writes.
//!
//! An `sb.*` file is a raw memory image: four branches (sample init, song init, play, stop),
//! the replay routine, its variables, its tables and the 8SVX samples. The code is not run.
//! As in the original, all player state lives in a working copy of the file (several rules
//! read the memory next to a variable), and the variables are found at fixed offsets.
//!
//! Recognition and layout. The four branch targets, the stop routine's load of the
//! AUD0VOL address, the first `lea d16(pc),a0` (its target is the first sample, `FORM`) and
//! the `lea` before it (its target is the pointer base) are matched on decoded instructions
//! (`m68kdec`). The variable offsets are not derivable from those: the specification gives
//! them as a per-build table, so the table here is keyed on the four entry offsets (the
//! specification's own identification of a build), and a file is accepted only when the
//! structure agrees with the entry: the derived sample start equals the table's, the
//! variables hold their initial values, the voice record and the arpeggio interval table
//! have their constants, and the samples parse. Any other file is refused (so that another
//! engine can take it).
//!
//! Two code families: "early" (Kelly X, MUNGOMUSIC: no slide, arpeggio or portamento) and
//! "full". Build differences: whether the sample init gives the samples their loops,
//! the subsong limit, the repeat stage (all voices, or voice 0 alone), portamento,
//! and the swapped instrument words (Street Fighter).
//!
//! Decisions where the specification leaves the choice to the port:
//! * The early family's envelope arpeggio (byte 6 of an envelope, a crash in the original)
//!   is ignored.
//! * A sequence or pattern that never yields a note (more than 1000 restarts of a row read
//!   or sequence step) ends the song as a stop byte does; the original would hang.
//! * The host's end-of-song writes (DMACON 15, the four volumes 0) follow the ending tick
//!   at once at 50 Hz, and at 100 Hz follow the next tick (which the one reference run
//!   that shows it, and the silence after a stop byte, both agree with; the specification's
//!   open question 2 leaves the grid point open).
//! * Timing inside a tick is approximated by fixed waits between the groups of writes
//!   (about the intervals the references show), not modelled per instruction.
//! * `set_subsong` stores the number as the original's song init does and restarts the
//!   end-of-song bookkeeping.
//! * A load at an address whose bit 0 is set would change the flick test of the
//!   specification (5.4); only the module base's parity matters, as pointers are kept
//!   absolute.

use super::m68kdec::{decode, Ea, K};
use super::{be16, be32, Hw, Open, Replayer, DT_TICK};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Steve Barrett", detect, new };

// ------------------------------------------------------------------ the builds

/// One code build: how it differs from the others.
struct Build {
    /// Offsets of the four entries: sample init, song init, play, stop.
    entries: [usize; 4],
    early: bool,
    /// The sample init gives the samples loops (and runs the stop routine).
    loops: bool,
    /// Song numbers 1..=limit start a song.
    limit: u8,
    /// The repeat stage covers all four voices (else voice 0 only).
    all_voices: bool,
    portamento: bool,
    /// The two instrument words are swapped (repeat length first).
    swap: bool,
    /// The host waits before the repeat stage.
    wait: bool,
}

static BUILDS: [Build; 6] = [
    Build { entries: [112, 102, 190, 1278], early: true, loops: true, limit: 5, all_voices: true, portamento: false, swap: false, wait: true },
    Build { entries: [80, 70, 158, 1246], early: true, loops: true, limit: 5, all_voices: true, portamento: false, swap: false, wait: true },
    Build { entries: [118, 108, 174, 1630], early: false, loops: false, limit: 5, all_voices: true, portamento: true, swap: false, wait: true },
    Build { entries: [122, 112, 200, 1658], early: false, loops: true, limit: 5, all_voices: true, portamento: true, swap: false, wait: true },
    Build { entries: [112, 102, 190, 1556], early: false, loops: true, limit: 5, all_voices: true, portamento: false, swap: false, wait: true },
    Build { entries: [66, 56, 126, 1570], early: false, loops: false, limit: 6, all_voices: false, portamento: true, swap: true, wait: false },
];

/// Offsets of the player's variables in a build (0 where the build has none).
#[derive(Clone, Copy, Default)]
struct Lay {
    flag: usize,
    pertab: usize,
    perout: usize,
    vrec: usize,
    cnt: usize,
    started: usize,
    speed: usize,
    spdcnt: usize,
    vibdepth: usize,
    vibshift: usize,
    arpparam: usize,
    rowbytes: usize,
    regoff: usize,
    p_steps: usize,
    p_lo: usize,
    p_lo2: usize,
    p_dir: usize,
    p_tgt: usize,
    dmamask: usize,
    arpint: usize,
    baseper: usize,
    p_delta: usize,
    smpstart: usize,
    replen: usize,
    repoff: usize,
    patptr: usize,
    seqptr: usize,
    curseq: usize,
    curpat: usize,
    vibdelta: usize,
    vibacc: usize,
    arpbuf: usize,
    smpaddr: usize,
    instr: usize,
    env: usize,
    songs: usize,
    pattab: usize,
    samples: usize,
}

/// The per-build table, one column per build in the order of `BUILDS`.
fn layout_of(c: usize) -> Lay {
    macro_rules! col {
        ($($f:ident: [$a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $g:expr]),* $(,)?) => {
            Lay { $($f: [$a, $b, $c, $d, $e, $g][c]),* }
        };
    }
    col! {
        flag: [1686, 1654, 2274, 2300, 2042, 2214],
        pertab: [1688, 1656, 2276, 2302, 2044, 2216],
        perout: [1834, 1802, 2416, 2442, 2224, 2356],
        vrec: [1850, 1818, 2432, 2458, 2240, 2364],
        cnt: [1978, 1946, 2560, 2586, 2368, 2492],
        started: [1979, 1947, 2561, 2587, 2369, 2493],
        speed: [1980, 1948, 2562, 2588, 2370, 2494],
        spdcnt: [1981, 1949, 2563, 2589, 2371, 2495],
        vibdepth: [1982, 1950, 2564, 2590, 2372, 2496],
        vibshift: [1983, 1951, 2565, 2591, 2373, 2497],
        arpparam: [1984, 1952, 2566, 2592, 2374, 2498],
        rowbytes: [1985, 1953, 2567, 2593, 2375, 2499],
        regoff: [1986, 1954, 2568, 2594, 2376, 2500],
        p_steps: [0, 0, 2570, 2596, 0, 2502],
        p_lo: [0, 0, 2571, 2597, 0, 2503],
        p_lo2: [0, 0, 2572, 2598, 0, 2504],
        p_dir: [0, 0, 2573, 2600, 0, 2505],
        p_tgt: [0, 0, 2574, 2601, 0, 2506],
        dmamask: [1994, 1962, 2575, 2602, 2384, 2507],
        arpint: [0, 0, 2576, 2603, 2385, 2508],
        baseper: [1996, 1964, 2584, 2612, 2394, 2516],
        p_delta: [0, 0, 2592, 2620, 0, 2524],
        smpstart: [2014, 1982, 2602, 2630, 2412, 2534],
        replen: [2030, 1998, 2618, 2646, 2428, 2550],
        repoff: [2038, 2006, 2626, 2654, 2436, 2558],
        patptr: [2046, 2014, 2634, 2662, 2444, 2566],
        seqptr: [2062, 2030, 2650, 2678, 2460, 2582],
        curseq: [2078, 2046, 2666, 2694, 2476, 2598],
        curpat: [2082, 2050, 2670, 2698, 2480, 2602],
        vibdelta: [2086, 2054, 2674, 2702, 2484, 2606],
        vibacc: [2090, 2058, 2678, 2706, 2488, 2610],
        arpbuf: [0, 0, 2682, 2710, 2492, 2614],
        smpaddr: [2094, 2062, 2722, 2750, 2502, 2654],
        instr: [2136, 2110, 2812, 2834, 2568, 2744],
        env: [2192, 2174, 2940, 2946, 2656, 2824],
        songs: [2312, 2326, 3076, 3188, 2832, 2968],
        pattab: [2320, 2350, 3092, 3220, 2840, 3008],
        samples: [3596, 4058, 4084, 5394, 3728, 4010],
    }
}

// Voice record fields (the byte of voice v is at `vrec + F + v`).
const F_REGOFF: usize = 0;
const F_ARPBASE: usize = 4;
const F_NOTE: usize = 8;
const F_SEQPOS: usize = 12;
const F_DUR: usize = 16;
const F_DURCNT: usize = 20;
const F_MODE: usize = 24;
const F_ARP: usize = 28;
const F_TRANSPOSE: usize = 32;
const F_PTARGET: usize = 36;
const F_PPARAM: usize = 40;
const F_SLIDEDOWN: usize = 44;
const F_SLIDEUP: usize = 48;
const F_FLICK: usize = 52;
const F_ARPPOS: usize = 56;
const F_VIBDIR: usize = 60;
const F_VIBCNT: usize = 64;
const F_VIBDELAY: usize = 68;
const F_VIBWAIT: usize = 72;
const F_INSTR: usize = 76;
const F_ENV: usize = 80;
const F_ATARGET: usize = 84;
const F_ASTEP: usize = 88;
const F_RSTEP: usize = 92;
const F_VOL: usize = 96;
const F_DMABIT: usize = 100;
const F_VIBPARAM: usize = 104;
const F_RELEASING: usize = 108;
const F_SUSTAIN: usize = 112;

/// The AUD0VOL address (custom-chip base plus 168).
const AUD0VOL: u32 = 0xDFF0A8;

/// Samples size (see `hz100`) of the one build whose tick runs at 100 Hz.
const SAMPLES_SIZE_100HZ: usize = 62372;

/// Colour clocks of the host's wait before the repeat stage (about 8 raster lines).
const REPEAT_WAIT: u64 = 2000;

// ------------------------------------------------------------------ recognition

/// What the structure of a file says.
struct Found {
    build: usize,
    /// The pointer base as a signed offset from the file start.
    pbase: i32,
    lay: Lay,
    subs: usize,
}

/// The loop-count rule: "repeat n times, test first".
fn count_first(n: u8) -> u32 {
    if n <= 128 { n as u32 } else { 0 }
}

/// The loop-count rule: "repeat n+1 times, test after".
fn count_after(n: u8) -> u32 {
    if n <= 128 { n as u32 + 1 } else { 1 }
}

/// The tick runs at 100 Hz when the samples size is 62372: the end of the samples
/// (as the sample init leaves it) plus 4, minus the start of the samples.
fn hz100(end: usize, samples: usize) -> bool {
    end + 4 == samples + SAMPLES_SIZE_100HZ
}

/// Walks the sample records; calls `f(index, record, data start, data size)` for each and
/// returns the offset past the last one, or `None` when a record is malformed.
fn walk_samples(d: &[u8], start: usize, mut f: impl FnMut(usize, usize, usize, usize)) -> Option<usize> {
    let mut rec = start;
    let mut i = 0;
    while d.get(rec..rec + 4).is_some_and(|s| s == b"FORM") {
        let mut j = rec;
        while d.get(j..j + 4)? != b"BODY" {
            j += 4;
        }
        let size = be32(d, j + 4) as usize;
        let data = j + 8;
        if data + size > d.len() {
            return None;
        }
        f(i, rec, data, size);
        rec = data + size;
        i += 1;
    }
    if i == 0 { None } else { Some(rec) }
}

/// Recognition and layout (specification sections 1 to 3).
fn find(d: &[u8]) -> Option<Found> {
    if d.len() < 64 {
        return None;
    }
    // 1. four word-displacement branches with even displacements
    let mut entries = [0usize; 4];
    for (i, e) in entries.iter_mut().enumerate() {
        let b = decode(d, 4 * i)?;
        if b.k != K::Bra || b.size != 2 {
            return None;
        }
        let disp = b.target as i64 - (b.at as i64 + 2);
        if disp % 2 != 0 {
            return None;
        }
        *e = b.target;
    }
    if entries.iter().any(|&e| e + 8 > d.len()) {
        return None;
    }
    // 2. the stop routine's second instruction loads the AUD0VOL address into an address register
    let s = decode(d, entries[3] + 2)?;
    if !(s.k == K::Movea && s.size == 4 && matches!(s.src, Ea::Imm { val, .. } if val == AUD0VOL) && matches!(s.dst, Ea::A(_))) {
        return None;
    }
    // 3. the first `lea d16(pc),a0` points at the first sample; the `lea` before it at the pointer base
    let lea = (0..d.len().min(8192)).step_by(2).find_map(|o| {
        let i = decode(d, o)?;
        (i.k == K::Lea && i.dst == Ea::A(0) && matches!(i.src, Ea::Pc(_))).then_some(i)
    })?;
    let Ea::Pc(first) = lea.src else { return None };
    if d.get(first..first + 4)? != b"FORM" {
        return None;
    }
    let prev = decode(d, lea.at.checked_sub(4)?)?;
    let Ea::Pc(pb) = prev.src else { return None };
    if prev.k != K::Lea || prev.len != 4 {
        return None;
    }
    let pbase = pb as i64 as i32;
    // the table row of this build, and the structure that must agree with it
    let build = BUILDS.iter().position(|b| b.entries == entries)?;
    let b = &BUILDS[build];
    let lay = layout_of(build);
    if lay.samples != first || d.len() <= lay.samples {
        return None;
    }
    let at = |o: usize| d.get(o).copied();
    let initial = [
        (lay.flag, 1),
        (lay.started, 0),
        (lay.speed, 1),
        (lay.spdcnt, 0),
        (lay.dmamask, 15),
        (lay.cnt, 0),
    ];
    if initial.iter().any(|&(o, v)| at(o) != Some(v)) {
        return None;
    }
    if !b.early && (0..6).any(|i| at(lay.arpint + i) != Some([3, 5, 6, 7, 8, 9][i])) {
        return None;
    }
    // the voice record's constants
    for v in 0..4usize {
        let ok = at(lay.vrec + F_REGOFF + v) == Some(16 * v as u8)
            && at(lay.vrec + F_DMABIT + v) == Some(1 << v)
            && (b.early || at(lay.vrec + F_ARPBASE + v) == Some(10 * v as u8));
        if !ok {
            return None;
        }
    }
    let subs = lay.pattab.checked_sub(lay.songs)? / 8;
    if subs == 0 || subs > b.limit as usize {
        return None;
    }
    if lay.pattab + 2 > d.len() || lay.env + 8 > d.len() || lay.instr + 8 > d.len() {
        return None;
    }
    walk_samples(d, lay.samples, |_, _, _, _| {})?;
    Some(Found { build, pbase, lay, subs })
}

fn detect(d: &[u8]) -> bool {
    find(d).is_some()
}

// ------------------------------------------------------------------ the player

/// The end-of-song bookkeeping and a 68k-free copy of the player's memory.
struct Sb {
    /// The working copy of the module (all variables live here).
    m: Vec<u8>,
    base: u32,
    pbase: i32,
    b: &'static Build,
    l: Lay,
    subs: u16,
    sub: u16,
    hz100: bool,
    /// The carry value H of specification 7.6.
    carry: u16,
    /// Where the last pattern pointer was read or stored (flick test, 5.4).
    last_slot: usize,
    end_flags: [bool; 4],
    /// Stop routine runs since the song started.
    stops: u32,
    end_pending: bool,
    /// The host's end-of-song writes, after this many more ticks.
    end_writes: Option<u8>,
    songend: bool,
}

impl Sb {
    fn rb(&self, o: usize) -> u8 {
        self.m.get(o).copied().unwrap_or(0)
    }
    fn rw(&self, o: usize) -> u16 {
        be16(&self.m, o)
    }
    fn rl(&self, o: usize) -> u32 {
        be32(&self.m, o)
    }
    fn wb(&mut self, o: usize, v: u8) {
        if let Some(x) = self.m.get_mut(o) {
            *x = v;
        }
    }
    fn ww(&mut self, o: usize, v: u16) {
        self.wb(o, (v >> 8) as u8);
        self.wb(o + 1, v as u8);
    }
    fn wl(&mut self, o: usize, v: u32) {
        self.ww(o, (v >> 16) as u16);
        self.ww(o + 2, v as u16);
    }
    /// A byte at an absolute guest address inside the module (0 outside).
    fn rba(&self, a: u32) -> u8 {
        self.rb(a.wrapping_sub(self.base) as usize)
    }
    /// The absolute address of the pointer base plus `w`.
    fn pabs(&self, w: u32) -> u32 {
        self.base.wrapping_add(self.pbase as u32).wrapping_add(w)
    }
    fn g(&self, f: usize, v: usize) -> u8 {
        self.rb(self.l.vrec + f + v)
    }
    fn s(&mut self, f: usize, v: usize, x: u8) {
        self.wb(self.l.vrec + f + v, x);
    }
    /// A word of the period table by byte offset (`pertab + off`).
    fn per(&self, off: usize) -> u16 {
        self.rw(self.l.pertab + off)
    }

    fn stop(&mut self, hw: &mut Hw, counts: bool) {
        for v in 0..4 {
            hw.vol(v, 0);
            hw.wait(8);
            self.s(F_VOL, v, 0);
        }
        if counts {
            self.stops += 1;
            if self.stops == 2 {
                self.end_pending = true;
            }
        }
    }

    /// Sequence wrap (trigger B of the song end).
    fn wrap_signal(&mut self) {
        let c = self.rb(self.l.cnt) as usize;
        if c < 4 {
            self.end_flags[c] = false;
        }
        if self.end_flags.iter().all(|f| !f) {
            self.end_flags = [true; 4];
            self.end_pending = true;
        }
    }

    /// Specification 5.1. Returns false if the sequence never gives a pattern.
    fn seq_advance(&mut self, v: usize) -> bool {
        let l = self.l;
        let seq = self.rl(l.seqptr + 4 * v);
        self.wl(l.curseq, seq);
        for _ in 0..1000 {
            let pos = self.g(F_SEQPOS, v);
            let b = self.rba(seq.wrapping_add(pos as u32));
            self.s(F_SEQPOS, v, pos.wrapping_add(1));
            match b {
                255 => {
                    self.wrap_signal();
                    self.s(F_SEQPOS, v, 0);
                }
                128..=254 => self.s(F_TRANSPOSE, v, b.wrapping_add(64)),
                _ => {
                    let slot = l.pattab + 2 * b as usize;
                    let p = self.pabs(self.rw(slot) as u32);
                    self.wl(l.patptr + 4 * v, p);
                    self.wl(l.curpat, p);
                    self.last_slot = slot;
                    return true;
                }
            }
        }
        false
    }

    /// Song init, run on the tick after the song number is stored (6.8).
    fn song_init(&mut self, hw: &mut Hw) {
        let l = self.l;
        let n = self.rb(l.flag) as usize;
        let rec = l.songs + 8 * n.saturating_sub(1);
        for v in 0..4 {
            let w = self.rw(rec + 2 * v) as i16 as i32 as u32;
            let p = self.pabs(w);
            self.wl(l.seqptr + 4 * v, p);
        }
        for v in (0..4).rev() {
            self.s(F_DURCNT, v, 0);
            self.s(F_MODE, v, 0);
            self.s(F_SEQPOS, v, 0);
            if !self.seq_advance(v) {
                self.wb(l.flag, 0);
            }
        }
        self.stop(hw, true);
        self.wb(l.spdcnt, 0);
        self.wb(l.started, 1);
        self.wb(l.dmamask, 15);
    }

    /// Builds the arpeggio of voice `v` from ARPPARAM (7.4).
    fn build_arp(&mut self, v: usize) {
        let l = self.l;
        let mut q = self.g(F_ARPBASE, v) as usize + 1;
        let mut a = self.rb(l.arpparam);
        for i in 0..8 {
            if a & 1 != 0 {
                let x = self.rb(l.arpint + i);
                self.wb(l.arpbuf + q, x);
                q += 1;
            }
            a >>= 1;
        }
        self.wb(l.arpparam, a);
        self.wb(l.arpbuf + q, 255);
        self.s(F_PTARGET, v, 0);
    }

    /// Set period (6.3); `carry` records the high byte for the vibrato quirk (7.6).
    fn set_period(&mut self, v: usize, carry: bool) {
        let p = self.per(2 * self.g(F_NOTE, v) as usize);
        self.ww(self.l.baseper + 2 * v, p);
        self.ww(self.l.perout + 2 * v, p);
        if carry {
            self.carry = p >> 8;
        }
    }

    /// The row read (5.2). `None` on the stop byte; else the note byte and the bytes used.
    fn row_read(&mut self, v: usize) -> Option<(u8, usize)> {
        let l = self.l;
        let full = !self.b.early;
        if full {
            if self.g(F_MODE, v) != 2 {
                self.s(F_MODE, v, 0);
                self.s(F_ARP, v, 0);
            }
        } else {
            self.s(F_MODE, v, 0);
        }
        self.wb(l.rowbytes, 0);
        if full {
            self.s(F_PTARGET, v, 0);
            self.s(F_SLIDEDOWN, v, 0);
            self.s(F_SLIDEUP, v, 0);
        }
        self.s(F_RELEASING, v, 0);
        let slot = l.patptr + 4 * v;
        let p = self.rl(slot);
        self.wl(l.curpat, p);
        self.last_slot = slot;
        for _ in 0..1000 {
            self.wb(l.rowbytes, self.rb(l.rowbytes).wrapping_add(1));
            let pat = self.rl(l.curpat);
            let mut n = 0usize;
            macro_rules! next {
                () => {{
                    let x = self.rba(pat.wrapping_add(n as u32));
                    n += 1;
                    x
                }};
            }
            macro_rules! restart {
                () => {{
                    if !self.seq_advance(v) {
                        self.wb(l.flag, 0);
                        return None;
                    }
                    self.wb(l.rowbytes, 0);
                    continue;
                }};
            }
            let mut b = next!();
            if b == 255 {
                restart!();
            }
            if b == 254 {
                self.wb(l.flag, 0);
                return None;
            }
            if b > 239 {
                self.wb(l.speed, b & 15);
                b = next!();
            }
            if b > 223 {
                self.s(F_INSTR, v, (b & 15) << 3);
                b = next!();
                if b == 255 {
                    restart!();
                }
            }
            if b > 191 {
                self.s(F_ENV, v, (b & 31) << 3);
                b = next!();
                if b == 255 {
                    restart!();
                }
            }
            if b > 127 {
                self.s(F_DUR, v, b & 63);
                b = next!();
            }
            if b > 95 {
                match b {
                    127 | 126 if full => {
                        self.s(F_SLIDEDOWN, v, (b == 127) as u8);
                        self.s(F_SLIDEUP, v, (b == 126) as u8);
                        self.s(F_MODE, v, 1);
                        b = next!();
                    }
                    125 | 124 if full => {
                        self.s(F_MODE, v, if b == 125 { 1 } else { 2 });
                        let a = next!();
                        self.s(F_ARP, v, a);
                        self.wb(l.arpparam, a);
                        self.build_arp(v);
                        b = next!();
                    }
                    123 if full => {
                        self.s(F_ARP, v, 0);
                        self.s(F_MODE, v, 1);
                        let t = next!();
                        let tr = self.g(F_TRANSPOSE, v);
                        self.s(F_PTARGET, v, t.wrapping_add(tr));
                        let pp = next!();
                        self.s(F_PPARAM, v, pp);
                        b = next!();
                    }
                    122 => {
                        let x = next!();
                        self.s(F_VIBDELAY, v, x);
                        b = next!();
                    }
                    _ => {}
                }
            }
            return Some((b, n));
        }
        self.wb(l.flag, 0);
        None
    }

    /// The instrument load (6.4).
    fn instrument(&mut self, v: usize, hw: &mut Hw) {
        let l = self.l;
        let r = l.instr + self.g(F_INSTR, v) as usize;
        let smp = self.rb(r) as usize;
        let (a, b) = (self.rw(r + 2), self.rw(r + 4));
        let (rl, ro) = if self.b.swap { (a, b) } else { (b, a) };
        self.ww(l.replen + 2 * v, rl);
        self.ww(l.repoff + 2 * v, ro);
        let rec = l.smpaddr + 6 * smp;
        let (len, addr) = (self.rw(rec), self.rl(rec + 2));
        let ch = (self.rb(l.regoff) as usize / 16) & 3;
        hw.len(ch, len);
        hw.wait(94);
        hw.loc(ch, addr);
        self.wl(l.smpstart + 4 * v, addr);
    }

    /// The envelope load (6.5).
    fn envelope(&mut self, v: usize) {
        let e = self.l.env + self.g(F_ENV, v) as usize;
        let (e0, e1, e2, e3, e4, e5, e6) = (self.rb(e), self.rb(e + 1), self.rb(e + 2), self.rb(e + 3), self.rb(e + 4), self.rb(e + 5), self.rb(e + 6));
        self.s(F_VOL, v, e0);
        self.s(F_ATARGET, v, e1);
        self.s(F_ASTEP, v, e2);
        self.s(F_RSTEP, v, e3);
        self.s(F_SUSTAIN, v, e4);
        self.s(F_VIBPARAM, v, e5);
        if e6 != 0 && !self.b.early {
            self.s(F_ARP, v, e6);
            self.s(F_MODE, v, e6);
            self.wb(self.l.arpparam, e6);
            self.build_arp(v);
        }
    }

    /// Note start (5.3).
    fn note_start(&mut self, v: usize, nb: u8, n: usize, hw: &mut Hw) {
        let l = self.l;
        let m = self.rb(l.dmamask) & !(1 << v);
        self.wb(l.dmamask, m);
        hw.dmacon(1 << v);
        hw.wait(158);
        self.wb(l.rowbytes, n as u8);
        self.instrument(v, hw);
        self.envelope(v);
        let note = nb.wrapping_add(self.g(F_TRANSPOSE, v));
        self.s(F_NOTE, v, note);
        if !self.b.early {
            // the flick test (5.4)
            let e = l.env + self.g(F_ENV, v) as usize;
            if self.g(F_ARP, v) == 0 {
                let e6 = self.rb(e + 6);
                if e6 != 0 {
                    self.s(F_ARP, v, e6);
                }
            }
            let test = if self.g(F_ARP, v) == 0 { self.rb(e + 7) } else { self.rb(self.last_slot + 7) };
            if test & 1 != 0 {
                self.s(F_FLICK, v, note);
                self.s(F_NOTE, v, note.wrapping_add(1));
            }
        }
        self.set_period(v, false);
        self.carry = 0;
        let pp = self.rl(l.patptr + 4 * v).wrapping_add(self.rb(l.rowbytes) as u32);
        self.wl(l.patptr + 4 * v, pp);
        let dur = self.g(F_DUR, v);
        self.s(F_DURCNT, v, dur);
        let vd = self.g(F_VIBDELAY, v);
        self.s(F_VIBWAIT, v, vd);
        if !self.b.early {
            let ab = self.g(F_ARPBASE, v);
            self.s(F_ARPPOS, v, ab.wrapping_add(1));
        }
        hw.wait(300);
    }

    /// Pitch effects of a tick without a note start (7.1).
    fn pitch(&mut self, v: usize) {
        if self.g(F_NOTE, v) == 0 {
            return;
        }
        if self.g(F_MODE, v) == 0 && self.g(F_VIBPARAM, v) != 0 {
            self.vibrato(v);
        } else if self.b.early {
        } else if self.g(F_ARP, v) != 0 {
            self.arpeggio(v);
        } else if self.g(F_PTARGET, v) != 0 && self.b.portamento {
            self.portamento(v);
        }
    }

    /// Vibrato (7.3).
    fn vibrato(&mut self, v: usize) {
        let l = self.l;
        let vp = self.g(F_VIBPARAM, v);
        self.wb(l.vibshift, vp & 15);
        self.wb(l.vibdepth, ((self.carry << 8 | vp as u16) >> 4) as u8);
        let note = self.g(F_NOTE, v) as usize;
        let off = (2 * note) & 255;
        let mut delta = self.per(off).wrapping_sub(self.per(off + 2));
        for _ in 0..count_first(vp & 15) {
            delta = (delta / 2) | 1;
        }
        self.wb(l.vibshift, 255);
        self.ww(l.vibdelta, delta);
        let mut dir = self.g(F_VIBDIR, v) as i8;
        let mut cnt = self.g(F_VIBCNT, v);
        let mut up = true;
        if dir < 0 {
            up = false;
            cnt = cnt.wrapping_sub(1);
            if cnt == 0 {
                dir = dir.wrapping_add(1);
                up = dir < 0;
            }
        }
        if up {
            cnt = cnt.wrapping_add(1);
            if cnt >= self.rb(l.spdcnt) {
                dir = dir.wrapping_sub(1);
            }
        }
        self.s(F_VIBDIR, v, dir as u8);
        self.s(F_VIBCNT, v, cnt);
        let w = self.g(F_VIBWAIT, v);
        if w != 0 {
            self.s(F_VIBWAIT, v, w - 1);
            return;
        }
        let mut acc = self.rw(l.baseper + 2 * v);
        for _ in 0..count_first(self.rb(l.vibdepth) >> 1) {
            acc = acc.wrapping_add(delta);
        }
        for _ in 0..count_first(cnt) {
            acc = acc.wrapping_sub(delta);
        }
        self.ww(l.vibacc, acc);
        self.ww(l.perout + 2 * v, acc);
    }

    /// Arpeggio step (7.4).
    fn arpeggio(&mut self, v: usize) {
        let l = self.l;
        let mut pos = self.g(F_ARPPOS, v);
        if self.rb(l.arpbuf + pos as usize) == 255 {
            pos = self.g(F_ARPBASE, v);
        }
        let x = self.rb(l.arpbuf + pos as usize).wrapping_add(self.g(F_NOTE, v));
        let p = self.per(2 * x as usize);
        self.ww(l.perout + 2 * v, p);
        self.s(F_ARPPOS, v, pos.wrapping_add(1));
    }

    /// Portamento (7.5).
    fn portamento(&mut self, v: usize) {
        let l = self.l;
        let tgt = self.g(F_PTARGET, v);
        let pp = self.g(F_PPARAM, v);
        self.wb(l.p_tgt, tgt);
        self.wb(l.p_lo, pp & 15);
        self.wb(l.p_lo2, pp & 15);
        let t = (pp / 16).wrapping_add(self.g(F_DURCNT, v));
        let dur = self.g(F_DUR, v);
        if !(t <= dur && dur < t.wrapping_add(pp & 15)) {
            return;
        }
        let w = |s: &Self, i: u8| s.per((2 * i as usize) & 255);
        let note = self.g(F_NOTE, v);
        let (dir, delta) = if (tgt as i8) < (note as i8) {
            (0u8, w(self, tgt).wrapping_sub(w(self, note)))
        } else {
            (1u8, w(self, note).wrapping_sub(w(self, tgt)))
        };
        self.wb(l.p_dir, dir);
        let mut steps = 0u8;
        for _ in 0..count_after(self.rb(l.speed)) {
            steps = steps.wrapping_add(pp & 15);
        }
        self.wb(l.p_steps, steps);
        let step = delta.rotate_right(3);
        self.ww(l.p_delta, step);
        let bp = self.rw(l.baseper + 2 * v);
        let bp = if dir == 1 { bp.wrapping_sub(step) } else { bp.wrapping_add(step) };
        self.ww(l.baseper + 2 * v, bp);
        self.ww(l.perout + 2 * v, bp);
        self.carry = bp >> 8;
    }

    /// The volume step (8).
    fn vol_step(&mut self, v: usize) {
        let dc = self.g(F_DURCNT, v);
        if dc == 0 {
            return;
        }
        if self.g(F_RELEASING, v) == 0 {
            let sus = self.g(F_SUSTAIN, v);
            if sus == 0 {
                self.s(F_RELEASING, v, 1);
                return;
            }
            if sus == 255 {
                return;
            }
            let sh = (sus & 63) as u32;
            let k = if sh >= 8 { 0 } else { self.g(F_DUR, v) >> sh };
            if k.wrapping_sub(dc) != 0 {
                let vol = self.g(F_VOL, v);
                if vol != self.g(F_ATARGET, v) {
                    let a = self.g(F_ASTEP, v);
                    self.s(F_VOL, v, vol.wrapping_add(a));
                }
                return;
            }
        }
        self.s(F_RELEASING, v, 1);
        self.carry = 0;
        let x = self.g(F_VOL, v).wrapping_sub(self.g(F_RSTEP, v));
        if (1..=127).contains(&x) {
            self.s(F_VOL, v, x);
        } else {
            self.s(F_VOL, v, 0);
            let m = self.rb(self.l.dmamask) | 1 << v;
            self.wb(self.l.dmamask, m);
        }
    }

    /// One voice of a tick (6.2.1). False when the voice hit the stop byte.
    fn voice(&mut self, v: usize, hw: &mut Hw) -> bool {
        let l = self.l;
        self.wb(l.cnt, v as u8);
        let ro = self.g(F_REGOFF, v);
        self.wb(l.regoff, ro);
        if self.rb(l.spdcnt) != 0 {
            if self.g(F_FLICK, v) != 0 {
                self.s(F_FLICK, v, 0);
                let n = self.g(F_NOTE, v).wrapping_sub(1);
                self.s(F_NOTE, v, n);
                self.set_period(v, true);
            } else {
                self.pitch(v);
            }
            self.vol_step(v);
            return true;
        }
        let dc = self.g(F_DURCNT, v).wrapping_sub(1);
        self.s(F_DURCNT, v, dc);
        let slides = if self.g(F_SUSTAIN, v) == 0 && dc == 0 {
            let m = self.rb(l.dmamask) | 1 << v;
            self.wb(l.dmamask, m);
            true
        } else if (dc as i8) < 0 {
            let Some((nb, n)) = self.row_read(v) else { return false };
            self.note_start(v, nb, n, hw);
            self.vol_step(v);
            return true;
        } else {
            true
        };
        if slides {
            if self.g(F_SLIDEDOWN, v) != 0 {
                let n = self.g(F_NOTE, v).wrapping_sub(1);
                self.s(F_NOTE, v, n);
                self.set_period(v, true);
            } else if self.g(F_SLIDEUP, v) != 0 {
                let n = self.g(F_NOTE, v).wrapping_add(1);
                self.s(F_NOTE, v, n);
                self.set_period(v, true);
            } else {
                self.pitch(v);
            }
            self.vol_step(v);
        }
        true
    }

    /// The output stage after voice 0 (6.7).
    fn output(&mut self, hw: &mut Hw) {
        let l = self.l;
        for v in 0..4 {
            hw.per(v, self.rw(l.perout + 2 * v));
            hw.wait(8);
        }
        hw.wait(40);
        let d = ((!self.rb(l.dmamask) & 15) as u16) | 0x8200;
        hw.dmacon(d);
        hw.wait(36);
        hw.dmacon(d ^ 0x820F);
        hw.wait(64);
        for v in 0..4 {
            hw.vol(v, self.g(F_VOL, v) as u16);
            hw.wait(68);
        }
        if self.b.all_voices {
            if self.b.wait {
                hw.wait(REPEAT_WAIT);
            }
            for v in (0..4).rev() {
                hw.len(v, self.rw(l.replen + 2 * v) >> 1);
                hw.wait(48);
                let a = self.rl(l.smpstart + 4 * v).wrapping_add(self.rw(l.repoff + 2 * v) as u32);
                hw.loc(v, a);
                hw.wait(40);
            }
        } else if self.rw(l.replen) != 0 {
            let ch = (self.rb(l.regoff) as usize / 16) & 3;
            hw.len(ch, self.rw(l.replen));
            let a = self.rl(l.smpstart).wrapping_add(self.rw(l.repoff) as u32);
            hw.loc(ch, a);
        }
        let sc = self.rb(l.spdcnt).wrapping_sub(1);
        self.wb(l.spdcnt, if (sc as i8) < 0 { self.rb(l.speed) } else { sc });
    }

    /// Sample init (4.1). Returns the guest addresses of the sample words it clears.
    fn sample_init(&mut self) -> Vec<u32> {
        let l = self.l;
        let mut clear = Vec::new();
        let mut recs = Vec::new();
        let end = walk_samples(&self.m.clone(), l.samples, |i, rec, data, size| recs.push((i, rec, data, size)));
        for (i, rec, data, size) in recs {
            if self.b.loops {
                self.ww(rec, 0);
                let ib = l.instr + 8 * i;
                self.ww(ib + 2, 0);
                self.ww(ib + 4, 2);
                if self.rl(rec + 24) != 0 {
                    let a = self.rw(rec + 22);
                    let b = self.rw(rec + 26);
                    self.ww(ib + 2, a);
                    self.ww(ib + 4, b);
                }
            }
            self.ww(l.smpaddr + 6 * i, (size / 2) as u16);
            self.wl(l.smpaddr + 6 * i + 2, self.base.wrapping_add(data as u32));
            if self.b.loops {
                self.ww(data, 0);
                clear.push(self.base.wrapping_add(data as u32));
            }
        }
        self.hz100 = end.is_some_and(|e| hz100(e, l.samples));
        clear
    }

    fn restart_end(&mut self) {
        self.end_flags = [true; 4];
        self.stops = 0;
        self.end_pending = false;
        self.end_writes = None;
        self.songend = false;
    }
}

impl Replayer for Sb {
    fn format(&self) -> &'static str {
        "Steve Barrett"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        if self.hz100 { DT_TICK / 2 } else { DT_TICK }
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subs - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if n < self.subs {
            self.sub = n;
            self.wb(self.l.flag, (n + 1) as u8);
            self.restart_end();
        }
    }

    /// Tick dispatch (6.2).
    fn tick(&mut self, hw: &mut Hw) {
        let l = self.l;
        self.carry = 0;
        loop {
            let flag = self.rb(l.flag);
            if flag == 0 {
                self.wb(l.started, 0);
                self.stop(hw, true);
                break;
            }
            if flag <= self.b.limit {
                self.song_init(hw);
                break;
            }
            if self.rb(l.started) == 0 {
                break;
            }
            if (0..4).rev().any(|v| !self.voice(v, hw)) {
                continue;
            }
            self.wb(l.cnt, 255);
            self.output(hw);
            break;
        }
        self.wb(l.flag, 255);
        if self.end_pending {
            self.end_pending = false;
            self.songend = true;
            self.end_writes = Some(self.hz100 as u8);
        }
        match self.end_writes {
            Some(0) => {
                self.end_writes = None;
                hw.dmacon(15);
                for v in 0..4 {
                    hw.vol(v, 0);
                }
            }
            Some(n) => self.end_writes = Some(n - 1),
            None => {}
        }
    }
}

/// The player on a module loaded at `base`, after the sample init, and the guest addresses of
/// the sample words that init clears.
fn init(d: &[u8], base: u32) -> Option<(Sb, Vec<u32>)> {
    let f = find(d)?;
    let mut s = Sb {
        m: d.to_vec(),
        base,
        pbase: f.pbase,
        b: &BUILDS[f.build],
        l: f.lay,
        subs: f.subs as u16,
        sub: 0,
        hz100: false,
        carry: 0,
        last_slot: 0,
        end_flags: [true; 4],
        stops: 0,
        end_pending: false,
        end_writes: None,
        songend: false,
    };
    let clear = s.sample_init();
    s.wb(s.l.spdcnt, 0);
    s.wb(s.l.started, 0);
    s.wb(s.l.flag, 1);
    Some((s, clear))
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (mut s, clear) = init(d, base)?;
    for a in clear {
        m.bus.write_bytes(a, &[0, 0]);
    }
    if s.b.loops {
        let mut hw = Hw { bus: &mut m.bus };
        s.stop(&mut hw, false);
    }
    Some(Box::new(s))
}

#[cfg(test)]
#[path = "stevebarrett_tests.rs"]
mod tests;
