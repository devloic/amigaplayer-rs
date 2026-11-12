//! SUN-Tronic driver builds that `suntronic.rs` does not model (Eagleplayer "SUNtronic", module
//! prefix `sun.`), played by running the module's own driver.
//!
//! Clean-room, from black-box observation of the 68k reference (the Eagleplayer 2.06 "SUNtronic"
//! and UADE's "SUN-Tronic" running the unmodified modules under the engine's hosting): the module
//! memory the player changes before the first call (traced writes into the module), the voice
//! block it hands the driver (a memory dump at the driver's first instruction), the call target,
//! the entry registers and the call rate (traced branches into the module), and the Paula register
//! writes. No code of the player is used or reproduced. The driver itself is the module's data:
//! its code is run in place, where the host loaded the module, on the repo's 68000 core with the
//! standard cycle table (`m68k_rc::cycles`), as `markii.rs` and `fredgray.rs` do, so every Paula
//! write falls where the driver's own instructions put it.
//!
//! A module is a relocatable driver image: code at offset 0 that saves all registers and loads its
//! data block with `lea d(pc),a6`, then variables, tables, instrument records, the sequence list
//! and the streams. Pointers in the data are absolute addresses of the game the driver was ripped
//! from. What the player does around it, as observed:
//! * every long word from the data block's offset 0x24 up to the sequence list's end marker whose
//!   value lies in the module's span at the link origin is moved to the load address; the origin is
//!   the smallest such pointer below the list minus the offset of the first byte after the marker
//!   (on the five real modules this is exactly the set of long words the player rewrites);
//! * a voice block of four records is allocated and its address stored in the variable the driver
//!   loads its voice pointer from; each record gets the four stream pointers of the list's first
//!   entry and 0xFF at record offset 0x14, and the first entry's four transposition bytes are put at
//!   0x12E + voice * record size (where the A builds keep them; the B builds read them elsewhere,
//!   so there they stay zero, as in the reference);
//! * the tick, speed, row and position variables are set to fixed start values by build family;
//! * the driver's entry, offset 0, is called once per CIA tick (70 940 colour clocks) with A0 = the
//!   module and the other registers zero; the driver returns with `rts`.
//!
//! The driver's code is located by its shape, not by fixed offsets: the sequence advance (`lea
//! L(a6),a1`, `mulu #S,d0` or `lsl.w #4,d0`, `tst.l (a1,d0.w)`, `bne`, `clr.w P(a6)`) gives the
//! list, its stride and the wrap, the voice load after it (`movea.l V(a6),a0`, `move.l
//! (a1,d0.w),(a0)`, `move.l n(a1,d0.w),m(a0)` ...) the voice variable and the record size. In
//! the A family the wrap (the list's end marker reached, back to entry 0) is reported as the song
//! end, where UADE's build of the player reports it; the B family reports none, as there (the
//! Eagleplayer 2.06 build reports none at all). The two builds `suntronic.rs` models (code lengths
//! 0x6A4 and 0x46E) are left to it.
//!
//! Choices where the observations are silent (all safe, none can panic or hang): the B family's two
//! start states are told apart by the list's stride (0x10: the build without transposition bytes,
//! the reference's second set of start values; 0x14: the first); the first call's time is a fit
//! to the reference's start-ups (`first_call`; the player's start-up time is a host property); a
//! call that runs more than `CALL_CYCLES` CPU cycles or raises a CPU exception ends the song and is
//! not called again.

use super::{be16, be32, Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{CpuCore, CpuType, StepResult};

pub const OPEN: Open = Open { name: "SUN-Tronic driver", detect, new };

/// Colour clocks between two ticks (the CIA timer's period).
const TICK: u64 = 70940;
/// Colour clocks of the interrupt path from the tick to the driver's first instruction (as in
/// `suntronic.rs`): the song, and Paula's DMA line grid with it, starts one period and this
/// much before the first call.
const IRQ: u64 = 76;
/// A call that runs longer than this many CPU cycles (several ticks' worth) has failed.
const CALL_CYCLES: u64 = 1_000_000;
/// Largest module taken (the real ones are 2.7 to 6.4 KB).
const MAX_LEN: usize = 0x4_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    /// Word at 8 is `tst.b $18(a6)`.
    A,
    /// Word at 8 is `tst.b $10(a6)`.
    B,
}

/// What the driver's code says about its data.
#[derive(Debug)]
struct Driver {
    fam: Family,
    /// File offset of the data block.
    dd: usize,
    /// File offset of the sequence list, and its stride.
    list: usize,
    stride: usize,
    /// File offset of the wrap's `clr.w P(a6)`.
    wrap: usize,
    /// Data-block offset of the voice pointer variable.
    vvar: usize,
    /// Voice record size.
    rec: usize,
}

/// The relocation.
#[derive(Debug)]
struct Reloc {
    origin: u32,
    /// Sequence entries before the end marker.
    ents: usize,
    /// File offsets of the pointer long words.
    ptrs: Vec<usize>,
}

fn sx(w: u16) -> i64 {
    w as i16 as i64
}

fn parse(d: &[u8]) -> Option<Driver> {
    if d.len() < 0x400 || d.len() > MAX_LEN || be32(d, 0) != 0x48E7_FFFE || be16(d, 4) != 0x4DFA {
        return None;
    }
    let fam = match be32(d, 8) {
        0x4A2E_0018 => Family::A,
        0x4A2E_0010 => Family::B,
        _ => return None,
    };
    let dd = 6 + sx(be16(d, 6));
    if dd < 0x100 || dd as usize + 0x40 > d.len() {
        return None;
    }
    let dd = dd as usize;
    // the two builds `suntronic.rs` models (code lengths 0x6A4 and 0x46E) are left to it
    if dd == if fam == Family::A { 0x6A4 } else { 0x46E } {
        return None;
    }
    // the driver returns through `movem.l (a7)+,d0-d7/a0-a6; rts`
    if !(0..dd - 6).step_by(2).any(|o| be32(d, o) == 0x4CDF_7FFF && be16(d, o + 4) == 0x4E75) {
        return None;
    }
    for o in (0..dd - 8).step_by(2) {
        if be16(d, o) != 0x43EE {
            continue;
        }
        let lst = sx(be16(d, o + 2));
        // the entry index scaled to the stride
        let mut p = o + 4;
        let mut stride = 0;
        for _ in 0..8 {
            match be16(d, p) {
                0xC0FC => {
                    stride = be16(d, p + 2) as usize;
                    p += 4;
                    break;
                }
                0xE948 => {
                    stride = 16;
                    p += 2;
                    break;
                }
                _ => p += 2,
            }
        }
        if !(stride == 0x10 || stride == 0x14) {
            continue;
        }
        // tst.l (a1,d0.w); bne.b; clr.w P(a6)
        let Some(q) = (p..p + 24).step_by(2).find(|&q| {
            be32(d, q) == 0x4AB1_0000 && be16(d, q + 4) >> 8 == 0x66 && be16(d, q + 6) == 0x426E
        }) else {
            continue;
        };
        let wrap = q + 6;
        // movea.l V(a6),a0; move.l (a1,d0.w),(a0); move.l 4(a1,d0.w),R(a0)
        let Some(r) = (wrap..wrap + 24).step_by(2).find(|&r| {
            be16(d, r) == 0x206E && be32(d, r + 4) == 0x20B1_0000 && be16(d, r + 8) == 0x2171 && be16(d, r + 10) == 4
        }) else {
            continue;
        };
        let vvar = be16(d, r + 2) as usize;
        let rec = be16(d, r + 12) as usize;
        let list = dd as i64 + lst;
        if list < (dd + 0x24) as i64 || list as usize >= d.len() || vvar + 4 > 0x24 || !(0x40..=0x200).contains(&rec) {
            return None;
        }
        return Some(Driver { fam, dd, list: list as usize, stride, wrap, vvar, rec });
    }
    None
}

fn reloc(d: &[u8], drv: &Driver) -> Option<Reloc> {
    let n = d.len();
    // the end marker: the first entry whose first long word is 0 or all ones
    let mut t = drv.list;
    while be32(d, t) != 0 && be32(d, t) != 0xFFFF_FFFF {
        t += drv.stride;
        if t + 4 > n {
            return None;
        }
    }
    if t == drv.list {
        return None;
    }
    let a = t + 4;
    // the stream pointers of the list: the densest window of the module's size over them
    let mut lv: Vec<u32> = (drv.list..t).step_by(drv.stride).flat_map(|e| (0..4).map(move |k| be32(d, e + 4 * k))).collect();
    lv.sort_unstable();
    let (mut best, mut bi, mut j) = (0, 0, 0);
    for i in 0..lv.len() {
        while (lv[i] - lv[j]) as usize >= n {
            j += 1;
        }
        if i + 1 - j > best {
            best = i + 1 - j;
            bi = i;
        }
    }
    if 4 * best < 3 * lv.len() {
        return None;
    }
    let win = &lv[bi + 1 - best..=bi];
    let (lmin, lmax) = (win[0], win[best - 1]);
    let lo = (lmax as i64 - n as i64 + 1).max(0) as u32;
    // the smallest pointer below the list
    let mut min = lmin;
    for o in (drv.dd + 0x24..drv.list.saturating_sub(3)).step_by(2) {
        let v = be32(d, o);
        if v >= lo && v < min {
            min = v;
        }
    }
    let origin = min.checked_sub(a as u32)?;
    let mut ptrs = Vec::new();
    let mut o = drv.dd + 0x24;
    while o + 4 <= t {
        let v = be32(d, o);
        if v >= origin && ((v - origin) as usize) < n {
            ptrs.push(o);
            o += 4;
        } else {
            o += 2;
        }
    }
    if ptrs.len() < 8 {
        return None;
    }
    Some(Reloc { origin, ents: (t - drv.list) / drv.stride, ptrs })
}

fn detect(d: &[u8]) -> bool {
    parse(d).is_some_and(|drv| reloc(d, &drv).is_some())
}

/// The start values of the variables (data-block offset, byte), by start state.
const START_A: &[(usize, u8)] = &[
    (0x0C, 0), (0x0D, 0), (0x0E, 0), (0x0F, 0), (0x10, 0), (0x11, 0), (0x12, 0), (0x13, 0),
    (0x14, 0x05), (0x15, 0xFF), (0x16, 0x06), (0x1A, 0), (0x1B, 0), (0x1C, 0), (0x1D, 0), (0x1E, 0), (0x1F, 0),
];
const START_B1: &[(usize, u8)] = &[(0x10, 0x04), (0x11, 0x0F), (0x12, 0x05), (0x13, 0), (0x14, 0), (0x15, 0x1F)];
const START_B2: &[(usize, u8)] = &[
    (0x0C, 0), (0x0D, 0), (0x0E, 0), (0x0F, 0), (0x10, 0x05), (0x11, 0xFF), (0x12, 0x06), (0x13, 0), (0x14, 0), (0x15, 0x40),
];

pub struct SunTronicDriver {
    base: u32,
    stack: u32,
    wrap: u32,
    /// The song end is reported (family A, as UADE's build of the player does; the B family has none).
    report_end: bool,
    songend: bool,
    dead: bool,
    cpu: CpuCore,
    s: u64,
    cnt: u64,
}

/// The first call's time (the driver's first instruction) from the run's origin: a fixed part by
/// start state, a part per relocated long word and one per sequence entry (in hundredths of a
/// colour clock), fitted to the runs of UADE's build of the player, the comparison's reference.
/// A: shades 98 862, shades2 126 546, msx-intro 133 060 with 121, 384, 447 pointers and 24, 86, 98
/// entries (three points, three constants: residuals 0, +1, +1; the synthetic a001, with the
/// counts of shades, starts 30 later, so the player's time depends on the data a little too). B:
/// msx-cracktro 2 111 609 with 169 pointers and the synthetic b002 100 599 with 74 (both 32
/// entries), exact; the second B start state: msx-cracktro 96 065 with 80, exact. The Eagleplayer
/// 2.06 build starts 571 to 668 (A) and 1 956 (B) colour clocks sooner.
fn first_call(start: Start, ptrs: u64, ents: u64) -> u64 {
    let (c, k, j) = match start {
        Start::A => (86_312, 9_555, 4_121),
        Start::B1 => (92_024, 11_589, 0),
        Start::B2 => (86_794, 11_589, 0),
    };
    c + (ptrs * k + ents * j) / 100
}

/// The player's start state of the variables.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Start {
    A,
    B1,
    B2,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let drv = parse(d)?;
    let rl = reloc(d, &drv)?;
    let block = m.alloc((4 * drv.rec).max(0x12E + 3 * drv.rec + 1) as u32);
    let stack = m.alloc(0x1000);
    if block == 0 || stack == 0 {
        return None;
    }
    let bus = &mut m.bus;
    let delta = base.wrapping_sub(rl.origin);
    for &o in &rl.ptrs {
        bus.w32(base + o as u32, be32(d, o).wrapping_add(delta));
    }
    let start = match (drv.fam, drv.stride) {
        (Family::A, _) => Start::A,
        (Family::B, 0x10) => Start::B2,
        (Family::B, _) => Start::B1,
    };
    let dd = base + drv.dd as u32;
    let vars = match start {
        Start::A => START_A,
        Start::B1 => START_B1,
        Start::B2 => START_B2,
    };
    for &(o, v) in vars {
        bus.w8(dd + o as u32, v);
    }
    bus.w32(dd + drv.vvar as u32, block);
    for v in 0..4u32 {
        let r = block + v * drv.rec as u32;
        bus.w32(r, bus.r32(base + (drv.list + 4 * v as usize) as u32));
        bus.w8(r + 0x14, 0xFF);
        let tr = d.get(drv.list + 0x10 + v as usize).copied().unwrap_or(0);
        if drv.stride == 0x14 {
            bus.w8(block + 0x12E + v * drv.rec as u32, tr);
        }
    }
    let mut cpu = CpuCore::new();
    cpu.set_cpu_type(CpuType::M68000);
    // the reference's start-up counts from the run's origin, START_PHASE colour clocks before the
    // engine's aligned start; the first tick comes one period after the clock left here
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let first = first_call(start, rl.ptrs.len() as u64, rl.ents as u64);
    m.bus.clock = (origin + first - IRQ - TICK).max(m.bus.clock);
    Some(Box::new(SunTronicDriver {
        base,
        stack,
        wrap: base + drv.wrap as u32,
        report_end: drv.fam == Family::A,
        songend: false,
        dead: false,
        cpu,
        s: 0,
        cnt: 0,
    }))
}

impl SunTronicDriver {
    fn now(&self) -> u64 {
        self.s + self.cnt / 2
    }

    /// Runs the driver's entry until it returns.
    fn run(&mut self, bus: &mut Bus) -> Result<(), String> {
        let ret = (self.stack + 0x800) & !3;
        let sp = ret - 4;
        bus.w32(sp, ret);
        self.cpu.dar = [0; 16];
        self.cpu.dar[8] = self.base;
        self.cpu.dar[15] = sp;
        self.cpu.set_sr(0x2000);
        self.cpu.pc = self.base;
        self.cpu.invalidate_prefetch();
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok(());
            }
            if pc == self.wrap && self.report_end {
                self.songend = true;
            }
            if self.cnt > CALL_CYCLES {
                return Err(format!("sun-tronic driver: call did not return (PC=${pc:06x})"));
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68000);
            self.cnt += m68k_rc::cycles::cycles(&ins) as u64;
            m68k_rc::Bus::cycles(bus, self.cnt);
            match self.cpu.step(bus) {
                StepResult::Ok { .. } => {}
                other => return Err(format!("sun-tronic driver: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }
}

impl Replayer for SunTronicDriver {
    fn format(&self) -> &'static str {
        "SUN-Tronic"
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

    fn tick(&mut self, hw: &mut Hw) {
        if self.dead {
            return;
        }
        let start = hw.bus.clock;
        self.s = start;
        self.cnt = 2 * IRQ;
        hw.bus.epoch = (start, 0);
        if let Err(e) = self.run(hw.bus) {
            log::error!("{e}");
            self.songend = true;
            self.dead = true;
        }
        hw.bus.clock = hw.bus.clock.max(self.now());
    }
}

#[cfg(test)]
#[path = "suntronic_driver_tests.rs"]
mod tests;
