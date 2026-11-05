//! Dave Lowe (`dl`, the UNCLEART hunk files), played by running the module's own replay routine.
//!
//! A `dl` file is a hunk executable that carries the game's own 68000 replay routine (init, play
//! and stop entry points named in a small header: see `ports/davelowe/SPEC-dl.md` section 2). This
//! replayer loads the hunk, relocates it, and runs that routine on the repo's 68000 core with the
//! cycle table the recompiled players use (`m68k_rc::cycles`), so every register write falls on the
//! colour clock the routine's own instructions put it on, for every build of the driver and every
//! song, without per-build tables.
//!
//! What the Eagleplayer adds around the module is described as behaviour in the spec
//! (`fixround/spec/dl`, `dl2`, `dl3`). The Eagleplayer patches the module's own code: wherever it
//! stores to a Paula register (volume, period, address, length) or walks its order lists, it puts a
//! call to a handler of its own, which does the store through its scaling code and takes time.
//! Here each such site is found by the same byte shapes over the same range, and treated as a
//! host trap: the site's instructions run natively for their effect (the volume masked to 7 bits),
//! and the clock is charged the cycles the handler takes, with the Paula store at the handler's
//! time inside the site. The song-end counters, the subsong glue and the start-up chain are
//! modelled the same way (cycle counts charged to the clock the module's instructions follow).
//!
//! `DELI_DL_CPU=0` turns it off (the table-driven replayer in `davelowe.rs` then takes the files).

use super::{Hw, Open, Replayer, be16, be32};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{AddressBus, CpuCore, CpuType, FastMem, StepResult};

pub const OPEN: Open = Open { name: "Dave Lowe (68k)", detect, new };

fn enabled() -> bool {
    !std::env::var("DELI_DL_CPU").is_ok_and(|v| v == "0")
}

fn detect(d: &[u8]) -> bool {
    enabled() && d.get(36..44) == Some(b"UNCLEART") && (super::davelowe::OPEN_DL.detect)(d)
}

// ------------------------------------------------------------- the Eagleplayer's costs

/// Cycles of the Eagleplayer's interrupt wrapper before it calls the play routine.
const TICK_WRAP: u64 = 256;
/// The check call, then four small calls: 126 + 12 + 18 + 118 + 118 colour clocks.
const CHECK_CC: u64 = 126;
const SMALL_CC: u64 = 12 + 18 + 118 + 118;
/// Cycles the module's sample fix-up routine takes beyond the `X` the spec counts (the host's
/// call and return).
const FIXUP_EXTRA: u64 = 0;
/// A subsong change: the stop call's overhead around the module's stop routine, the default stop
/// (no stop routine: its length and the cycles of its stores), and the two fixed calls after it
/// (colour clocks).
const STOP_WRAP: u64 = 37;
const STOP_DEFAULT: u64 = 121;
const STOP_DMA_AT: u64 = 53;
const STOP_VOL_AT: [u64; 4] = [69, 81, 93, 105];
const CHANGE_CALLS_CC: u64 = 2 * 118;
/// The host's song-end callback returns (cycles).
const SONGEND_CYC: u64 = 0;

/// The rules of the module shapes the Eagleplayer patches (spec dl3 section 1), in order.
/// Rules 0 and 7 (a 10-word volume special, a location and length from `(a3)+`) are never run in the
/// corpus and their costs are not known: their words are taken from later rules, and they run
/// as the module wrote them.
const R_VOL0: u8 = 100;
const R_LIST7: u8 = 107;
const R_V1: u8 = 1;
const R_V1B: u8 = 2;
const R_PER3: u8 = 3;
const R_PER4: u8 = 4;
const R_LOC: u8 = 5;
const R_V6: u8 = 6;
const R_V2: u8 = 8;
const R_MOVE: u8 = 9;
const R_RESTART: u8 = 10;
const R_ADVANCE: u8 = 11;
const R_WORD: u8 = 12;

/// The shapes (`None` matches any word).
const SHAPES: [(u8, &[Option<u16>]); 13] = [
    (R_VOL0, &[Some(0x9068), Some(0x3E), Some(0x6400), Some(8), Some(0x3039), None, None, Some(0x3540), Some(8), Some(0x6000)]),
    (R_V1, &[Some(0x3540), Some(8), Some(0x2149), Some(0x12)]),
    (R_V1B, &[Some(0x3540), Some(8), Some(0x6000)]),
    (R_PER3, &[Some(0x3142), Some(6), Some(0x3542), Some(6)]),
    (R_PER4, &[Some(0x3140), Some(6), Some(0x3540), Some(6), Some(0x3159), Some(2)]),
    (R_LOC, &[Some(0x24A8), Some(0x26), Some(0x3568), Some(0x2A), Some(4)]),
    (R_V6, &[Some(0x3540), Some(8), Some(0x3559), Some(8), Some(0x2149), Some(0x12)]),
    (R_LIST7, &[Some(0x249B), Some(0x3553), Some(4)]),
    (R_V2, &[Some(0x3559), Some(8), Some(0x2149), Some(0x12)]),
    (R_MOVE, &[Some(0x3179), None, None, Some(0x42)]),
    (R_RESTART, &[Some(0x2168), Some(0x1E), Some(0x1A), Some(0x2268), Some(0x1A), Some(0x58A8), Some(0x1A)]),
    (R_ADVANCE, &[Some(0x58A8), Some(0x1A), Some(0x2251)]),
    (R_WORD, &[Some(0x3159), Some(2), Some(0x5368), Some(2)]),
];

/// Instructions run natively for the effect of a site, by rule (the third word of rule 2 is a
/// branch the site swallows; the module's own displacement word follows it).
fn instructions(rule: u8) -> usize {
    match rule {
        R_V1B | R_MOVE => 1,
        R_V1 | R_PER3 | R_LOC | R_V2 | R_ADVANCE | R_WORD => 2,
        _ => 3,
    }
}

/// Channel classes: 0..3 the Paula channels, 4 any other pointer (Time Scanner's effect voice).
fn class_of(a2: u32) -> usize {
    let a = a2 & 0xFF_FFFF;
    if (0xDF_F0A0..0xDF_F0E0).contains(&a) && a & 0xF == 0 {
        ((a - 0xDF_F0A0) >> 4) as usize
    } else {
        4
    }
}

/// Cycles charged for the whole site, by channel class (JSR, trailing nops, the handler and its
/// return), and the cycles from the site's start to each Paula store.
const V1: [u64; 5] = [354, 402, 440, 475, 361];
const V1B: [u64; 5] = [350, 398, 436, 471, 357];
const PER3: [u64; 5] = [189, 220, 251, 259, 259];
const PER4: [u64; 5] = [185, 216, 247, 255, 255];
const LOC: [u64; 5] = [346, 408, 470, 486, 486];
const V6: [u64; 5] = [726, 822, 898, 968, 740];
const V2: [u64; 5] = [386, 434, 472, 507, 393];
const MOVE_COST: u64 = 56;
const ADVANCE_MARKED: u64 = 153;
const ADVANCE_OTHER: u64 = 99;
const WORD_POSITIVE: u64 = 85;
/// A restart of a channel's list (rule 10; rule 12 with a negative word takes this less
/// `WORD_RESTART_LESS`), by class and by [counter was zero, counter was not, song end with no
/// effect, song end with an effect in subsong 1, song end with an effect in another].
const RESTART: [[u64; 5]; 5] = [
    [241, 263, 392, 461, 417],
    [268, 294, 423, 492, 448],
    [291, 317, 446, 515, 471],
    [314, 330, 459, 528, 484],
    [293, 293, 422, 491, 447],
];
const WORD_RESTART_LESS: u64 = 1;

const ST_V1: [u64; 4] = [193, 210, 217, 244];
const ST_PER: u64 = 44;
const ST_LOC: u64 = 72;
const ST_LEN: [u64; 4] = [197, 228, 259, 267];
const ST_V6B: [u64; 4] = [529, 594, 639, 701];
const ST_V2: [u64; 4] = [213, 230, 237, 264];

/// The Paula stores of the site being run: when each falls (cycles from the site's start), and
/// the volume value the handler wrote.
struct SiteStores {
    base: u64,
    times: [u64; 3],
    n: usize,
    next: usize,
    mask: bool,
    vol: Option<u16>,
}

// ---------------------------------------------------------------------- the 68000

/// The bus the module's instructions see: inside a patched site the Paula stores fall where the
/// handler puts them.
struct HookBus<'a> {
    bus: &'a mut Bus,
    site: Option<&'a mut SiteStores>,
}

impl HookBus<'_> {
    fn custom(&mut self, a: u32, v: u16) {
        let off = a & 0x1FE;
        let mut v = v;
        if let Some(s) = self.site.as_mut() {
            if (0xA0..0xE0).contains(&off) {
                if s.next < s.n {
                    let t = s.base + s.times[s.next];
                    s.next += 1;
                    m68k_rc::Bus::cycles(self.bus, t);
                }
                if s.mask && off & 0xF == 8 {
                    v &= 0x7F;
                    s.vol.get_or_insert(v);
                }
            }
        }
        self.bus.write_word(a, v);
    }
}

impl AddressBus for HookBus<'_> {
    fn read_byte(&mut self, a: u32) -> u8 {
        self.bus.read_byte(a)
    }
    fn read_word(&mut self, a: u32) -> u16 {
        self.bus.read_word(a)
    }
    fn read_long(&mut self, a: u32) -> u32 {
        self.bus.read_long(a)
    }
    fn write_byte(&mut self, a: u32, v: u8) {
        self.bus.write_byte(a, v)
    }
    fn write_word(&mut self, a: u32, v: u16) {
        if (a & 0xFF_FFFF) >= 0xDF_F000 {
            self.custom(a, v)
        } else {
            self.bus.write_word(a, v)
        }
    }
    fn write_long(&mut self, a: u32, v: u32) {
        self.write_word(a, (v >> 16) as u16);
        self.write_word(a.wrapping_add(2), v as u16);
    }
    fn fast_mem(&mut self) -> Option<FastMem> {
        self.bus.fast_mem()
    }
}

/// The Eagleplayer's state around the module: the patched sites, the song-end counters, the
/// longest voice.
struct Stubs {
    img: u32,
    /// For each word of the image: the rule and the length in words of the site that starts there.
    sites: Vec<(u8, u8)>,
    counters: [u8; 4],
    /// The last voice with the longest list in the current subsong's entry (4: none).
    marker: usize,
    sub: u16,
    /// The effect request (header +76) and the module variable it goes to.
    fx: u32,
    var: u32,
    /// The song ended during the call.
    ended: bool,
}

impl Stubs {
    /// Finds the sites of `h` (the hunk as in the file) as the Eagleplayer does.
    fn scan(&mut self, h: &[u8]) {
        self.sites = vec![(0, 0); h.len() / 2 + 2];
        let play = be32(h, 16) as usize;
        let table = be32(h, 44) as usize;
        let end = if table == 0 { play + 900 } else { table }.min(h.len());
        for (rule, shape) in SHAPES {
            let mut p = play & !1;
            while p + 2 * shape.len() <= end {
                let hit = shape.iter().enumerate().all(|(i, w)| w.is_none_or(|w| be16(h, p + 2 * i) == w))
                    && self.sites[p / 2].0 == 0
                    && (0..shape.len()).all(|i| self.sites[p / 2 + i].1 == 0);
                if hit {
                    self.sites[p / 2] = (rule, shape.len() as u8);
                    for i in 1..shape.len() {
                        self.sites[p / 2 + i] = (0xFF, 1);
                    }
                    p += 2 * shape.len();
                } else {
                    p += 2;
                }
            }
        }
    }

    /// A restart of a channel's list: the counters, the song end; returns the cycles.
    fn restart(&mut self, bus: &mut Bus, ch: usize) -> u64 {
        let mut was_zero = true;
        if ch < 4 && self.counters[ch] != 0 {
            was_zero = false;
            self.counters[ch] = self.counters[ch].wrapping_add(1);
        }
        if self.counters.iter().all(|&c| c == 0) {
            self.counters = [0xFF; 4];
            self.ended = true;
            if self.fx != 0 && self.sub == 1 {
                bus.w32(self.var.wrapping_sub(6), self.fx);
            }
            let col = if self.fx == 0 {
                2
            } else if self.sub == 1 {
                3
            } else {
                4
            };
            RESTART[ch][col] + SONGEND_CYC
        } else {
            RESTART[ch][!was_zero as usize]
        }
    }
}

/// The module's code is run by this CPU on the engine's bus.
struct Cpu68 {
    cpu: CpuCore,
    sp: u32,
}

impl Cpu68 {
    fn new(sp: u32) -> Cpu68 {
        let mut cpu = CpuCore::new();
        cpu.set_cpu_type(CpuType::M68000);
        Cpu68 { cpu, sp }
    }

    /// Runs the `n` instructions at the PC natively, their Paula stores timed by `site`.
    fn native(&mut self, bus: &mut Bus, n: usize, site: &mut SiteStores) -> Result<(), String> {
        for _ in 0..n {
            let mut hb = HookBus { bus, site: Some(site) };
            match self.cpu.step(&mut hb) {
                StepResult::Ok { .. } => {}
                other => return Err(format!("dl: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
        Ok(())
    }

    /// Handles the patched site of `rule` at the PC, `words` long; returns its cycles.
    fn site(&mut self, bus: &mut Bus, st: &mut Stubs, rule: u8, words: u8, cycles: u64) -> Result<u64, String> {
        let pc = self.cpu.pc;
        let ch = class_of(self.cpu.dar[10]);
        let c4 = ch.min(3);
        let mut s = SiteStores { base: cycles, times: [0; 3], n: 0, next: 0, mask: false, vol: None };
        let d0 = self.cpu.dar[0];
        let total = match rule {
            R_V1 | R_V1B | R_V6 | R_V2 => {
                s.mask = true;
                let (t, tab) = match rule {
                    R_V1 => (ST_V1[c4], V1),
                    R_V1B => (ST_V1[c4], V1B),
                    R_V6 => (ST_V1[c4], V6),
                    _ => (ST_V2[c4], V2),
                };
                s.times = [t, if rule == R_V6 { ST_V6B[c4] } else { 0 }, 0];
                s.n = if ch < 4 { 1 + (rule == R_V6) as usize } else { 0 };
                self.native(bus, instructions(rule), &mut s)?;
                if rule == R_V1B {
                    // the handler also stores the list pointer; the module's branch (made short) skips its own store
                    let a0 = self.cpu.dar[8];
                    bus.w32(a0.wrapping_add(18), self.cpu.dar[9]);
                }
                if rule != R_V2 {
                    self.cpu.dar[0] = if ch < 4 { s.vol.map_or(d0 & 0x7F, |v| v as u32) } else { (d0 & 0xFFFF_0000) | (d0 & 0x7F) };
                }
                tab[ch]
            }
            R_PER3 | R_PER4 => {
                s.times = [ST_PER, 0, 0];
                s.n = (ch < 4) as usize;
                self.native(bus, instructions(rule), &mut s)?;
                if rule == R_PER3 { PER3[ch] } else { PER4[ch] }
            }
            R_LOC => {
                s.times = [ST_LOC, ST_LOC, ST_LEN[c4]];
                s.n = if ch < 4 { 3 } else { 0 };
                self.native(bus, instructions(rule), &mut s)?;
                LOC[ch]
            }
            R_MOVE => {
                self.native(bus, 1, &mut s)?;
                MOVE_COST
            }
            R_RESTART => {
                self.native(bus, 3, &mut s)?;
                st.restart(bus, ch)
            }
            R_ADVANCE => {
                self.native(bus, 2, &mut s)?;
                if ch < 4 && ch == st.marker { ADVANCE_MARKED } else { ADVANCE_OTHER }
            }
            R_WORD => {
                let w = bus.r16(self.cpu.dar[9]);
                self.native(bus, 2, &mut s)?;
                if (w as i16) < 0 { st.restart(bus, ch) - WORD_RESTART_LESS } else { WORD_POSITIVE }
            }
            _ => return Err(format!("dl: site rule {rule} at ${pc:06x}")),
        };
        self.cpu.pc = pc + 2 * words as u32;
        self.cpu.invalidate_prefetch();
        Ok(total)
    }

    /// Calls the routine at `entry`, whose first instruction starts at `start` (colour clocks)
    /// with `pre` cycles already spent in the call; returns the cycles of the whole call.
    fn call(&mut self, bus: &mut Bus, entry: u32, start: u64, pre: u64, st: &mut Stubs) -> Result<u64, String> {
        let ret = self.sp & !3;
        let sp = ret - 4;
        bus.w32(sp, ret);
        self.cpu.dar = [0; 16];
        self.cpu.dar[15] = sp;
        self.cpu.set_sr(0x2000);
        self.cpu.pc = entry;
        self.cpu.invalidate_prefetch();
        bus.clock = start;
        bus.epoch = (start, 0);
        let mut cycles = pre;
        let mut executed = 0u64;
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok(cycles);
            }
            executed += 1;
            if executed > 20_000_000 {
                return Err(format!("dl: routine did not return (PC=${pc:06x})"));
            }
            if pc >= st.img && pc & 1 == 0 {
                let (rule, words) = st.sites.get(((pc - st.img) / 2) as usize).copied().unwrap_or((0, 0));
                if rule != 0 && rule != 0xFF && rule < R_VOL0 {
                    let c = self.site(bus, st, rule, words, cycles)?;
                    cycles += c;
                    m68k_rc::Bus::cycles(bus, cycles);
                    continue;
                }
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68020);
            cycles += m68k_rc::cycles::cycles(&ins) as u64;
            m68k_rc::Bus::cycles(bus, cycles);
            let mut hb = HookBus { bus, site: None };
            match self.cpu.step(&mut hb) {
                StepResult::Ok { .. } => {}
                other => return Err(format!("dl: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }
}

// -------------------------------------------------------------------------------- module

/// What the start-up work of the Eagleplayer costs, from the module's own data (spec dl section 4).
#[derive(Default, Clone)]
struct Quantities {
    steps: u64,
    bra: u64,
    subs: u64,
    zterm: u64,
    /// For each subsong table entry: the list lengths summed, the restarts, the longest voice.
    entries: Vec<(u64, u64, usize)>,
    midwinter: bool,
    timescanner: bool,
    no_table: bool,
}

impl Quantities {
    fn of(h: &[u8]) -> Quantities {
        let mut s = Quantities::default();
        let play = be32(h, 16) as usize;
        let mut p = play + 380;
        let mut found = false;
        let mut form2 = false;
        while p + 4 <= h.len() {
            s.steps += 1;
            let (w0, w1) = (be16(h, p), be16(h, p + 2));
            if w1 == 8 && (w0 == 0x3540 || w0 == 0x3559) {
                found = true;
                form2 = w0 == 0x3559;
                break;
            }
            p += 2;
        }
        s.bra = (found && !form2 && be16(h, p + 4) == 0x6000) as u64;
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
            s.entries = (0..s.subs as usize).map(|k| Self::lists(h, table + 16 * k)).collect();
        }
        s.midwinter = be32(h, 60) == 77554 && s.subs == 17;
        s.timescanner = be32(h, 76) == 38;
        s
    }

    /// `L`, `U` and the longest voice of the subsong table entry at `e`.
    fn lists(h: &[u8], e: usize) -> (u64, u64, usize) {
        let (mut l, mut u, mut best, mut marker) = (0, 0, 0, 4);
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
                marker = c;
            }
        }
        (l, u, marker)
    }

    /// Cycles of the player-init call, without the module's sample fix-up.
    fn c_player_init(&self) -> u64 {
        let mut c = 1191 + 76 * self.steps + 72 * self.subs + 20 * self.bra - 25 * self.zterm;
        if self.subs == 0 {
            c -= 2;
        }
        if self.midwinter {
            c += 395;
        }
        c
    }

    /// Cycles of the song-init call for subsong `n` before the module's init routine runs.
    fn p_song_init(&self, n: u16) -> u64 {
        let k = (n as usize).min(self.entries.len());
        let (l, u) = self.entries[..k].iter().fold((0, 0), |a, e| (a.0 + e.0, a.1 + e.1));
        let ts = if self.timescanner { if n == 1 { 37 } else { 21 } } else { 0 };
        let mut c = 1605 + 29 * l + 16 * u + 254 * (n as u64 - 1) + ts;
        if self.no_table {
            c -= 290;
        }
        c
    }

    fn marker(&self, n: u16) -> usize {
        self.entries.get(n as usize - 1).map_or(0, |e| e.2)
    }
}

struct DaveLowe {
    cpu: Cpu68,
    stubs: Stubs,
    /// The hunk as in the file, before relocation.
    h: Vec<u8>,
    q: Quantities,
    title: String,
    subs: u16,
    cur: u16,
    songend: bool,
    /// The first tick after a subsong change.
    resume: Option<u64>,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let n = be32(d, 28) as usize;
    let img = base + 32;
    let h = d.get(32..32 + 4 * n)?;
    relocate(d, n, &mut m.bus, img);
    let q = Quantities::of(h);
    let stack = m.alloc(0x1000);
    if stack == 0 {
        return None;
    }
    let title = {
        let o = be32(h, 48) as usize;
        h.get(o..).map_or(String::new(), |s| s.iter().take_while(|&&b| b != 0).take(80).map(|&b| b as char).collect())
    };
    let var = img + be32(h, 24);
    let mut s = DaveLowe {
        cpu: Cpu68::new(stack + 0x1000),
        stubs: Stubs { img, sites: Vec::new(), counters: [0xFE; 4], marker: 4, sub: 1, fx: be32(h, 76), var, ended: false },
        h: h.to_vec(),
        subs: q.subs.max(1) as u16,
        q,
        title,
        cur: 1,
        songend: false,
        resume: None,
    };
    s.prepare(&mut m.bus);
    if let Some(sites) = std::env::var_os("DELI_DL_SITES") {
        s.dump_sites(&sites);
    }
    // a module whose routines do not run on the core is left to the table-driven replayer
    let clock = m.bus.clock;
    if s.start(&mut m.bus).is_none() {
        m.bus.clock = clock;
        return (super::davelowe::OPEN_DL.new)(d, base, m);
    }
    Some(Box::new(s))
}

/// Applies the hunk's relocation table (`HUNK_RELOC32` entries for hunk 0) to the image.
fn relocate(d: &[u8], n: usize, bus: &mut Bus, img: u32) {
    let mut p = 32 + 4 * n;
    if be32(d, p) != 0x3EC {
        return;
    }
    p += 4;
    loop {
        let count = be32(d, p) as usize;
        if count == 0 || p + 8 + 4 * count > d.len() {
            break;
        }
        let hunk = be32(d, p + 4);
        p += 8;
        for i in 0..count {
            let off = be32(d, p + 4 * i);
            if hunk == 0 {
                let a = img + off;
                let v = bus.r32(a).wrapping_add(img);
                bus.w32(a, v);
            }
        }
        p += 4 * count;
    }
}

impl DaveLowe {
    fn hdr(&self, o: u32) -> u32 {
        be32(&self.h, o as usize)
    }

    /// The Eagleplayer's patching of the image: the init-player branch fix-up, then the sites.
    fn prepare(&mut self, bus: &mut Bus) {
        let h = &self.h;
        let play = be32(h, 16) as usize;
        let mut p = play + 380;
        while p + 4 <= h.len() {
            let (w0, w1) = (be16(h, p), be16(h, p + 2));
            if w1 == 8 && (w0 == 0x3540 || w0 == 0x3559) {
                if w0 == 0x3540 && p + 8 <= h.len() && be16(h, p + 4) == 0x6000 {
                    let a = self.stubs.img + p as u32 + 6;
                    let v = bus.r16(a).wrapping_add(0x6002);
                    bus.w16(a, v);
                }
                break;
            }
            p += 2;
        }
        self.stubs.scan(&self.h);
    }

    fn dump_sites(&self, path: &std::ffi::OsStr) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::File::create(path) {
            for (i, &(r, w)) in self.stubs.sites.iter().enumerate() {
                if r != 0 && r != 0xFF {
                    let r = if r >= R_VOL0 { r - R_VOL0 } else { r };
                    let _ = writeln!(f, "site {r} {:#x} {w}", 2 * i);
                }
            }
        }
    }

    /// Song-init state for subsong `sub`.
    fn song_state(&mut self, bus: &mut Bus, sub: u16) {
        self.stubs.counters = [0xFE; 4];
        self.stubs.marker = self.q.marker(sub);
        self.stubs.sub = sub;
        self.stubs.ended = false;
        // the Eagleplayer leaves the subsong number and the effect request in the module's variables
        bus.w16(self.stubs.var, sub);
        if self.stubs.fx != 0 && sub == 1 {
            bus.w32(self.stubs.var.wrapping_sub(6), self.stubs.fx);
        }
    }

    /// The Eagleplayer's start-up chain at subsong 1, from the aligned start `bus.clock`:
    /// leaves the clock at its end.
    fn start(&mut self, bus: &mut Bus) -> Option<()> {
        let a = bus.clock;
        // the sample fix-up the header names, run for its cycles and its effect on the samples
        let mut c_ip = self.q.c_player_init();
        let fix = self.hdr(40);
        if fix != 0 {
            let c = self.cpu.call(bus, self.stubs.img + fix, a, 0, &mut self.stubs).ok()?;
            c_ip += c.saturating_sub(FIXUP_EXTRA) + 20;
        }
        let t_song = a + CHECK_CC + (c_ip.div_ceil(2) - 1) + SMALL_CC;
        self.init_song(bus, 1, t_song)
    }

    /// Runs the song-init for subsong `sub` from `t_song`; leaves the clock at its end.
    fn init_song(&mut self, bus: &mut Bus, sub: u16, t_song: u64) -> Option<()> {
        self.song_state(bus, sub);
        let init = self.hdr(12);
        let cyc = self.cpu.call(bus, self.stubs.img + init, t_song, self.q.p_song_init(sub), &mut self.stubs).map_err(|e| log::error!("{e}")).ok()?;
        bus.clock = t_song + cyc / 2;
        self.stubs.ended = false;
        Some(())
    }

    /// The Eagleplayer's change of subsong: the stop call, two small calls, the song-init.
    fn change(&mut self, bus: &mut Bus, sub: u16) {
        let a = bus.clock;
        let stop = self.hdr(20);
        let took = if stop != 0 {
            match self.cpu.call(bus, self.stubs.img + stop, a, STOP_WRAP, &mut self.stubs) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("{e}");
                    0
                }
            }
        } else {
            let at = |c: u64, bus: &mut Bus, reg: u32, v: u16| {
                bus.clock = a + c / 2;
                bus.write_word(reg, v);
            };
            at(STOP_DMA_AT, bus, 0xDF_F096, 0x000F);
            for (i, c) in STOP_VOL_AT.iter().enumerate() {
                at(*c, bus, 0xDF_F0A8 + 16 * i as u32, 0);
            }
            STOP_DEFAULT
        };
        // the module's header fix-up runs again for the new subsong (its effect on the module's
        // tables; the corpus shows no time for it): F-15 Strike Eagle II needs it, and the rest
        // of its subsong 2 is not understood (spec dl3 README 1)
        let fix = self.hdr(40);
        if fix != 0 {
            let _ = self.cpu.call(bus, self.stubs.img + fix, bus.clock, 0, &mut self.stubs);
        }
        let t_song = a + took / 2 + CHANGE_CALLS_CC;
        bus.clock = t_song;
        if self.init_song(bus, sub, t_song).is_some() {
            self.resume = Some(bus.clock + super::DT_TICK);
        }
        self.cur = sub;
    }
}

impl Replayer for DaveLowe {
    fn format(&self) -> &'static str {
        "Dave Lowe"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.subs, self.cur)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if n >= 1 && n <= self.subs {
            self.songend = false;
            self.change(hw.bus, n);
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        let start = hw.bus.clock;
        let play = self.stubs.img + self.hdr(16);
        match self.cpu.call(hw.bus, play, start, TICK_WRAP, &mut self.stubs) {
            Ok(c) => hw.bus.clock = start + c / 2,
            Err(e) => {
                log::error!("{e}");
                self.songend = true;
            }
        }
        if self.stubs.ended {
            self.stubs.ended = false;
            self.songend = true;
        }
    }
}
