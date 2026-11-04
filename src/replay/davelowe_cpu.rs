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
//! (`fixround/spec/dl`, `dl2`): the start-up chain and its length (`T`), the first tick 70940
//! colour clocks after it, the per-write costs of the Eagleplayer's register-write handlers, the
//! subsong number and effect request it leaves in the module's variables. Those are modelled
//! here as cycle counts charged to the same clock as the module's instructions.
//!
//! It is selected with `DELI_DL_CPU=1` (the table-driven replayer in `davelowe.rs` takes the
//! files otherwise).

use super::{Hw, Open, Replayer, be16, be32};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{AddressBus, CpuCore, CpuType, FastMem, StepResult};
use std::io::Write;

pub const OPEN: Open = Open { name: "Dave Lowe (68k)", detect, new };

fn enabled() -> bool {
    std::env::var("DELI_DL_CPU").is_ok_and(|v| v == "1")
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

// ---------------------------------------------------------------------- the 68000

/// Writes to the audio block (and DMACON) seen while a call runs, for `DELI_DL_CPU_DUMP`.
struct Dump {
    out: std::io::BufWriter<std::fs::File>,
    /// `DELI_DL_CPU_HIST`: per call, the instruction histogram (pc, count, cycles).
    hist: Option<std::io::BufWriter<std::fs::File>>,
}

/// The bus the module's instructions see: Paula's registers get the Eagleplayer's per-write
/// costs, added to the call's cycle count before and after the store.
struct HookBus<'a> {
    bus: &'a mut Bus,
    /// Cycles the call has taken so far (the clock follows them).
    cycles: u64,
    /// The instruction about to run, and whether the Eagleplayer's handler catches its stores.
    pc: u32,
    caught: u8,
    costs: &'a Costs,
    dump: Option<&'a mut Dump>,
}

impl HookBus<'_> {
    fn custom(&mut self, a: u32, v: u16) {
        let off = a & 0x1FE;
        if self.caught != 0 && (0xA0..0xE0).contains(&off) {
            let ch = ((off - 0xA0) >> 4) as usize;
            let (pre, post) = self.costs.of(self.caught, off & 0xF, ch);
            self.cycles += pre;
            m68k_rc::Bus::cycles(self.bus, self.cycles);
            self.bus.write_word(a, v);
            self.cycles += post;
        } else {
            self.bus.write_word(a, v);
        }
        if let Some(d) = self.dump.as_mut() {
            if off == 0x96 || (0xA0..0xE0).contains(&off) {
                let _ = writeln!(d.out, "{:03x} {:04x} {} pc={:06x} cyc={} caught={}", off, v, self.bus.clock, self.pc, self.cycles, self.caught);
            }
        }
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

/// Kinds of module store the Eagleplayer's handlers catch (the module's own code is patched
/// there): the volume store in its two forms, the period, the address and the length.
const K_V1: u8 = 1;
const K_V2: u8 = 2;
const K_PER: u8 = 3;
const K_LOC: u8 = 4;
const K_LEN: u8 = 5;

/// The Eagleplayer's cost, in 68000 cycles, before and after the store, by class (volume form 1,
/// volume form 2, period, address high, address low, length) and channel, as measured on the
/// write logs of the corpus.
struct Costs {
    pre: [[i64; 4]; 6],
    post: [[i64; 4]; 6],
}

const COSTS: Costs = Costs {
    pre: [[181, 203, 208, 233], [197, 214, 214, 240], [19, 20, 13, 13], [50, 52, 52, 53], [0; 4], [0, 0, -2, 0]],
    post: [[141, 172, 206, 201], [157, 195, 227, 227], [122, 152, 183, 191], [0; 4], [105, 136, 169, 174], [149, 180, 211, 220]],
};

impl Costs {
    fn of(&self, kind: u8, reg: u32, ch: usize) -> (u64, u64) {
        let class = match (kind, reg) {
            (K_V1, 8) => 0,
            (K_V2, 8) => 1,
            (K_PER, 6) => 2,
            (K_LOC, 0) => 3,
            (K_LOC, 2) => 4,
            (K_LEN, 4) => 5,
            _ => return (0, 0),
        };
        (self.pre[class][ch & 3].max(0) as u64, self.post[class][ch & 3].max(0) as u64)
    }
}

/// For each word of the module's code, the kind of store it begins, if the Eagleplayer catches it.
fn store_kinds(h: &[u8]) -> Vec<u8> {
    let mut k = vec![0u8; h.len() / 2 + 2];
    for i in 0..h.len() / 2 {
        let w = |j: usize| if 2 * (i + j) + 2 <= h.len() { be16(h, 2 * (i + j)) } else { 0 };
        let (op, e1, e2) = (w(0), w(1), w(2));
        k[i] = match (op, e1, e2) {
            (0x3540..=0x3547, 8, _) => K_V1,
            (0x3559, 8, _) => K_V2,
            (0x3540..=0x3547, 6, _) => K_PER,
            (0x24A8, 0x26, _) => K_LOC,
            (0x3568, 0x2A, 4) => K_LEN,
            _ => 0,
        };
    }
    k
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

    /// Calls the routine at `entry`, whose first instruction starts at `start` (colour clocks)
    /// with `pre` cycles already spent in the call; returns the cycles of the whole call.
    fn call(
        &mut self,
        bus: &mut Bus,
        entry: u32,
        start: u64,
        pre: u64,
        costs: &Costs,
        kinds: &[u8],
        img: u32,
        mut dump: Option<&mut Dump>,
    ) -> Result<u64, String> {
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
        let mut hist: std::collections::BTreeMap<u32, (u64, u64)> = Default::default();
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                if let Some(h) = dump.as_deref_mut().and_then(|d| d.hist.as_mut()) {
                    let _ = writeln!(h, "C {cycles}");
                    for (pc, (n, c)) in &hist {
                        let _ = writeln!(h, "H {pc:06x} {n} {c}");
                    }
                }
                return Ok(cycles);
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68020);
            let ic = m68k_rc::cycles::cycles(&ins) as u64;
            if dump.is_some() {
                let h = hist.entry(pc).or_default();
                h.0 += 1;
                h.1 += ic;
            }
            cycles += ic;
            m68k_rc::Bus::cycles(bus, cycles);
            let mut hb = HookBus { bus, cycles, pc, caught: kinds.get((pc.wrapping_sub(img) / 2) as usize).copied().filter(|_| pc >= img).unwrap_or(0), costs, dump: dump.as_deref_mut() };
            let r = self.cpu.step(&mut hb);
            cycles = hb.cycles;
            executed += 1;
            if executed > 20_000_000 {
                return Err(format!("dl: routine did not return (PC=${:06x})", self.cpu.pc));
            }
            match r {
                StepResult::Ok { .. } => {}
                other => return Err(format!("dl: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }
}

// -------------------------------------------------------------------------------- module

/// What the start-up work of the Eagleplayer costs, from the module's own data (spec dl section 4).
#[derive(Default, Clone, Copy)]
struct Quantities {
    steps: u64,
    /// The volume store found is of form 2 (`move.w (a1)+,8(an)`).
    form2: bool,
    bra: u64,
    subs: u64,
    zterm: u64,
    list_words: u64,
    longer_lists: u64,
    midwinter: bool,
    timescanner: bool,
    init_player: bool,
    no_table: bool,
}

impl Quantities {
    fn of(h: &[u8]) -> Quantities {
        let mut s = Quantities::default();
        let play = be32(h, 16) as usize;
        let mut p = play + 380;
        let mut found = false;
        while p + 4 <= h.len() {
            s.steps += 1;
            let (w0, w1) = (be16(h, p), be16(h, p + 2));
            if w1 == 8 && (w0 == 0x3540 || w0 == 0x3559) {
                found = true;
                s.form2 = w0 == 0x3559;
                break;
            }
            p += 2;
        }
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
        s
    }

    /// `L` and `U` of the subsong table entry at `e`.
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

    /// Cycles of the song-init call before the module's init routine runs.
    fn p_song_init(&self) -> u64 {
        let mut c = 1605 + 29 * self.list_words + 16 * self.longer_lists + 37 * self.timescanner as u64;
        if self.no_table {
            c -= 290;
        }
        c
    }
}

struct DaveLowe {
    cpu: Cpu68,
    /// The hunk image in guest memory.
    img: u32,
    /// The hunk image as in the file, before relocation.
    h: Vec<u8>,
    q: Quantities,
    title: String,
    subs: u16,
    cur: u16,
    songend: bool,
    dump: Option<Dump>,
    kinds: Vec<u8>,
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
    let mut s = DaveLowe {
        cpu: Cpu68::new(stack + 0x1000),
        img,
        h: h.to_vec(),
        q,
        title,
        subs: q.subs.max(1) as u16,
        cur: 1,
        songend: false,
        dump: std::env::var("DELI_DL_CPU_DUMP").ok().and_then(|p| std::fs::File::create(p).ok()).map(|f| Dump { out: std::io::BufWriter::new(f), hist: std::env::var("DELI_DL_CPU_HIST").ok().and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new) }),
        kinds: store_kinds(h),
    };
    s.start(m, 1)?;
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
    /// A header long of the hunk as stored in the file (the relocation turns some into addresses).
    fn hdr(&self, _bus: &Bus, o: u32) -> u32 {
        be32(&self.h, o as usize)
    }

    /// The Eagleplayer's start-up chain at subsong `sub`, from the aligned start `bus.clock`:
    /// leaves the clock at its end.
    fn start(&mut self, m: &mut Machine, sub: u16) -> Option<()> {
        let a = m.bus.clock;
        let q = self.q;
        // the sample fix-up the header names, run for its cycles and its effect on the samples
        let mut c_ip = q.c_player_init();
        let fix = self.hdr(&m.bus, 40);
        if fix != 0 {
            let c = self.cpu.call(&mut m.bus, self.img + fix, a, 0, &COSTS, &self.kinds, self.img, None).ok()?;
            c_ip += c.saturating_sub(FIXUP_EXTRA) + 20;
        }
        let t_song = a + CHECK_CC + (c_ip.div_ceil(2) - 1) + SMALL_CC;
        // the Eagleplayer leaves the subsong number and the effect request in the module's variables
        let var = self.img + self.hdr(&m.bus, 24);
        m.bus.w16(var, sub);
        let fx = self.hdr(&m.bus, 76);
        if fx != 0 && sub == 1 {
            m.bus.w32(var.wrapping_sub(6), fx);
        }
        let init = self.hdr(&m.bus, 12);

        let dump = self.dump.as_mut();
        let cyc = self.cpu.call(&mut m.bus, self.img + init, t_song, q.p_song_init(), &COSTS, &self.kinds, self.img, dump).map_err(|e| log::error!("{e}")).ok()?;
        m.bus.clock = t_song + cyc / 2;
        if let Some(d) = self.dump.as_mut() {
            let _ = writeln!(d.out, "# init end");
        }
        Some(())
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

    fn tick(&mut self, hw: &mut Hw) {
        let start = hw.bus.clock;
        let play = self.img + be32(&self.h, 16);
        let r = self.cpu.call(hw.bus, play, start, TICK_WRAP, &COSTS, &self.kinds, self.img, self.dump.as_mut());
        match r {
            Ok(c) => hw.bus.clock = start + c / 2,
            Err(e) => {
                log::error!("{e}");
                self.songend = true;
            }
        }
        if let Some(d) = self.dump.as_mut() {
            let _ = writeln!(d.out, "# tick end");
            let _ = d.out.flush();
            if let Some(h) = d.hist.as_mut() {
                let _ = h.flush();
            }
        }
    }
}
