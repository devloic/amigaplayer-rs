//! Mark II Sound-System (`mk2.*`, `mkii.*`; modules of the editor by Andreas Scholl and Darius
//! Zendeh, played by the Eagleplayer "MarkII"), played by running the module's own driver.
//!
//! A Mark II module is a plain 68000 program image: the driver's code at offset 0, a parameter
//! block, tables, song data and samples (see `fixround/spec/markii`). The driver is position
//! independent and writes Paula itself, with one entry point and a command in D0. This replayer
//! runs it in place, where the host loaded the module, on the repo's 68000 core with the standard
//! cycle table (`m68k_rc::cycles`), as the Riff Raff and Benn Daglish SID replayers do, so every
//! Paula write falls where the driver's own instructions put it. What the Eagleplayer adds around
//! the driver is reproduced from the specification:
//! * the module check and the identification of the driver build by fixed code words at fixed
//!   offsets (sections 1.3 and 1.4);
//! * the sites, five or six fixed code shapes of the driver that are replaced by host behaviour
//!   with a fixed cost (section 5): length, location, period and volume stores, the period-table
//!   lookup of the gate tick and the song-end report;
//! * the start-up timeline, the tick and the song end (sections 4 and 6).
//!
//! The song-end site clears the wrong word of the module, as the original player does (section 6):
//! after the song end one voice writes to 0xDFF000..0xDFF008 and keeps sounding its last note. That
//! is reproduced, because it is what the reference does and what the write logs show.
//!
//! The three site tables (`markii_sites.bin`, 156 bytes: per table an entry count, then per entry a
//! module offset, three words of the driver's own code and a kind) are taken from the original
//! player on the project owner's decision.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a module whose
//! check fails or whose sites match no table is not recognised; a driver that runs more than a few
//! million instructions in one call ends the song and is not called again; a cleared-word address
//! outside the module is ignored.

use super::{Hw, Open, Replayer, be16};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{AddressBus, CpuCore, CpuType, StepResult};

pub const OPEN: Open = Open { name: "Mark II", detect, new };

/// The three site tables (section 1.4): for each a byte with its entry count, then per entry the
/// module offset (word), three code words and a kind byte. Taken from the original player on the
/// project owner's decision.
static SITE_TABLES: &[u8] = include_bytes!("markii_sites.bin");

/// Where the run's detection starts, in the reference's log clock (the engine's start phase).
const S0: u64 = crate::engine::START_PHASE;
/// Words examined by the check for the second and third word (section 1.3).
const BUDGET: usize = 288;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Len,
    Loc,
    LocCmp,
    Per,
    Vol,
    PerTab,
    End,
}

#[derive(Clone, Copy, Debug)]
struct Site {
    off: u32,
    kind: Kind,
    /// The second code word of the entry (the displacement the end site uses).
    k: u16,
}

fn w16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

/// The module check (section 1.3): the numbers of words examined by each step.
struct Check {
    ne: usize,
    nf: usize,
    nd: usize,
    nb: usize,
}

impl Check {
    /// The player's check cost in cycles.
    fn cycles(&self) -> u64 {
        87 + 31 * (self.ne + self.nf + self.nd) as u64 + 33 * self.nb as u64
    }
}

fn check(d: &[u8]) -> Option<Check> {
    if w16(d, 4)? != 0x41FA {
        return None;
    }
    // the first 0xE742 from offset 0
    let mut p = 0;
    let mut ne = 0;
    loop {
        let x = w16(d, p)?;
        ne += 1;
        p += 2;
        if x == 0xE742 {
            break;
        }
        if ne > BUDGET {
            return None;
        }
    }
    // the first 0x41FA after it
    let mut nf = 0;
    loop {
        let x = w16(d, p)?;
        nf += 1;
        p += 2;
        if x == 0x41FA {
            break;
        }
        if ne + nf > BUDGET {
            return None;
        }
    }
    if ne + nf > BUDGET {
        return None;
    }
    // at most 24 words for 0xD1FA
    let mut nd = 0;
    loop {
        if nd >= 24 {
            return None;
        }
        let x = w16(d, p)?;
        nd += 1;
        p += 2;
        if x == 0xD1FA {
            break;
        }
    }
    // backwards from the word before, at most 16 words for 0x41FA
    let mut nb = 0;
    let mut q = p;
    loop {
        if nb >= 16 || q < 2 {
            return None;
        }
        q -= 2;
        nb += 1;
        if w16(d, q)? == 0x41FA {
            break;
        }
    }
    Some(Check { ne, nf, nd, nb })
}

/// The first site table whose every entry matches the module, with the build's index.
fn sites_of(d: &[u8]) -> Option<(usize, Vec<Site>)> {
    let mut p = 0;
    let mut build = 0;
    while p < SITE_TABLES.len() {
        let n = SITE_TABLES[p] as usize;
        p += 1;
        let mut sites = Vec::new();
        let mut ok = true;
        for i in 0..n {
            let e = p + 9 * i;
            let off = be16(SITE_TABLES, e) as usize;
            let kind = match SITE_TABLES[e + 8] {
                1 => Kind::Len,
                2 => Kind::Loc,
                3 => Kind::LocCmp,
                4 => Kind::Per,
                5 => Kind::Vol,
                6 => Kind::PerTab,
                _ => Kind::End,
            };
            if (0..3).any(|j| w16(d, off + 2 * j) != Some(be16(SITE_TABLES, e + 2 + 2 * j))) {
                ok = false;
                break;
            }
            sites.push(Site { off: off as u32, kind, k: be16(SITE_TABLES, e + 4) });
        }
        p += 9 * n;
        if ok {
            return Some((build, sites));
        }
        build += 1;
    }
    None
}

fn detect(d: &[u8]) -> bool {
    check(d).is_some() && sites_of(d).is_some()
}

/// The player's InitPlayer cost in cycles by build (section 4.1).
const C_INIT: [u64; 3] = [1526, 1966, 2485];

/// The voice a register base belongs to for the sites' costs: any other base costs as voice 3.
fn voice_of(base: u32) -> usize {
    match base {
        0xDF_F0A0 => 0,
        0xDF_F0B0 => 1,
        0xDF_F0C0 => 2,
        _ => 3,
    }
}

/// The N, Z, V and C flags (bits 3 to 0) of comparing the byte `dst` with 0x32.
fn cmp_flags(dst: u8) -> u8 {
    let src = 0x32u8;
    let r = dst.wrapping_sub(src);
    let n = r & 0x80 != 0;
    let z = r == 0;
    let v = ((src ^ dst) & (dst ^ r)) & 0x80 != 0;
    let c = dst < src;
    (n as u8) << 3 | (z as u8) << 2 | (v as u8) << 1 | c as u8
}

pub struct MarkII {
    base: u32,
    len: u32,
    sites: Vec<Site>,
    stack: u32,
    songend: bool,
    /// The driver failed to return: it is not called again.
    dead: bool,
    cpu: CpuCore,
    // the call in progress: the clock at count 0 and the cycle count
    s: u64,
    cnt: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let chk = check(d)?;
    let (build, sites) = sites_of(d)?;
    let stack = m.alloc(0x1000);
    if stack == 0 {
        return None;
    }
    let mut cpu = CpuCore::new();
    cpu.set_cpu_type(CpuType::M68000);
    let mut r = MarkII { base, len: d.len() as u32, sites, stack, songend: false, dead: false, cpu, s: 0, cnt: 0 };
    // the start call (section 4.1)
    let i = S0 + chk.cycles() / 2 + C_INIT[build] / 2;
    m.bus.clock = m.bus.clock.max(i);
    r.begin(&mut m.bus, i, 32);
    r.call(&mut m.bus, 0xFFFF_FFFF, 0);
    m.bus.clock = m.bus.clock.max(r.now());
    Some(Box::new(r))
}

impl MarkII {
    fn advance(&mut self, bus: &mut Bus, cycles: u64) {
        self.cnt += cycles;
        m68k_rc::Bus::cycles(bus, self.cnt);
    }

    /// The clock at the current count.
    fn now(&self) -> u64 {
        self.s + self.cnt / 2
    }

    /// Starts a call: the clock `start` at count 0, `pre` cycles already spent.
    fn begin(&mut self, bus: &mut Bus, start: u64, pre: u64) {
        self.s = start;
        self.cnt = pre;
        bus.clock = bus.clock.max(start);
        bus.epoch = (start, 0);
    }

    fn call(&mut self, bus: &mut Bus, d0: u32, d1: u32) {
        if let Err(e) = self.run(bus, d0, d1) {
            log::error!("{e}");
            self.songend = true;
            self.dead = true;
        }
    }

    /// Runs the driver's entry point until it returns, with `d0` and `d1` as entry registers.
    fn run(&mut self, bus: &mut Bus, d0: u32, d1: u32) -> Result<(), String> {
        let ret = (self.stack + 0x800) & !3;
        let sp = ret - 4;
        bus.w32(sp, ret);
        self.cpu.dar = [0; 16];
        self.cpu.dar[0] = d0;
        self.cpu.dar[1] = d1;
        self.cpu.dar[8] = self.base;
        self.cpu.dar[15] = sp;
        self.cpu.set_sr(0x2000);
        self.cpu.pc = self.base;
        self.cpu.invalidate_prefetch();
        let mut executed = 0u64;
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok(());
            }
            executed += 1;
            if executed > 5_000_000 {
                return Err(format!("markii: driver did not return (PC=${pc:06x})"));
            }
            let rel = pc.wrapping_sub(self.base);
            if rel < self.len {
                if let Some(s) = self.sites.iter().copied().find(|s| s.off == rel) {
                    self.site(bus, s);
                    continue;
                }
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68000);
            self.cnt += m68k_rc::cycles::cycles(&ins) as u64;
            m68k_rc::Bus::cycles(bus, self.cnt);
            match self.cpu.step(bus) {
                StepResult::Ok { .. } => {}
                other => return Err(format!("markii: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }

    /// Performs a site's behaviour and cost (section 5), then continues after its six bytes.
    fn site(&mut self, bus: &mut Bus, s: Site) {
        let pc = self.cpu.pc;
        let c0 = self.cnt;
        let a3 = self.cpu.dar[11];
        let d1 = self.cpu.dar[1];
        let d2 = self.cpu.dar[2];
        let to = |me: &mut MarkII, bus: &mut Bus, off: u64| {
            me.cnt = c0 + off;
            m68k_rc::Bus::cycles(bus, me.cnt);
        };
        match s.kind {
            Kind::Len => {
                let v = (d2 as u16 as i16 as i32 as u32).wrapping_add(self.cpu.dar[8]).wrapping_add(6);
                let w = bus.r16(v);
                to(self, bus, 76);
                bus.write_word(a3.wrapping_add(4), w);
                to(self, bus, [169, 192, 215, 205][voice_of(a3)]);
            }
            Kind::Loc | Kind::LocCmp => {
                let cmp = s.kind == Kind::LocCmp;
                if !cmp {
                    self.cpu.dar[8] = self.cpu.dar[8].wrapping_add(d2);
                    self.cpu.dar[2] = (d2 & 0xFFFF_0000) | (d1 & 0xFFFF);
                }
                to(self, bus, if cmp { 50 } else { 58 });
                AddressBus::write_long(bus, a3, self.cpu.dar[8]);
                if cmp {
                    // the condition codes of a byte compare of D1 with 0x32 (X is kept)
                    let x = (self.cpu.get_sr() & 0x10) as u8;
                    self.cpu.set_ccr(x | cmp_flags(d1 as u8));
                }
                let t = if cmp { [143, 166, 189, 179] } else { [147, 170, 193, 183] }[voice_of(a3)];
                to(self, bus, t);
            }
            Kind::Per => {
                to(self, bus, 50);
                bus.write_word(a3.wrapping_add(6), d1 as u16);
                self.cpu.dar[2] = 0;
                to(self, bus, [135, 158, 181, 171][voice_of(a3)]);
            }
            Kind::Vol => {
                // the player's per-voice volume factor is 64 for all voices (section 7)
                let v = (((d2 & 0x7F) * 64) >> 6) as u16;
                let vo = voice_of(a3);
                to(self, bus, [231, 254, 277, 267][vo]);
                bus.write_word(a3.wrapping_add(8), v);
                self.cpu.dar[9] = self.cpu.dar[9].wrapping_add(2);
                to(self, bus, [311, 334, 357, 347][vo]);
            }
            Kind::PerTab => {
                let idx = d1 as u16 as i16 as i32 as u32;
                let a = self.cpu.dar[9].wrapping_add(idx.wrapping_mul(2));
                let w = bus.r16(a);
                self.cpu.dar[1] = (d1 & 0xFFFF_0000) | w as u32;
                to(self, bus, 80);
                bus.write_word(a3, w);
                to(self, bus, [169, 192, 215, 205][voice_of(a3.wrapping_sub(6))]);
            }
            Kind::End => {
                to(self, bus, 52);
                self.songend = true;
                // the player clears the word four bytes past the one the replaced instruction
                // would have (section 6)
                let o = self.base.wrapping_add(s.off).wrapping_add(6).wrapping_add(s.k as i16 as i32 as u32);
                if o.wrapping_sub(self.base) < self.len.saturating_sub(1) {
                    bus.w16(o, 0);
                }
                self.cpu.dar[8] = o;
                to(self, bus, 116);
            }
        }
        self.cpu.pc = pc + 6;
        self.cpu.invalidate_prefetch();
    }
}

impl Replayer for MarkII {
    fn format(&self) -> &'static str {
        "Mark II"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.dead {
            return;
        }
        let start = hw.bus.clock;
        self.begin(hw.bus, start, 264);
        self.call(hw.bus, 0, 1);
        if !self.dead {
            self.advance(hw.bus, 168);
        }
        hw.bus.clock = hw.bus.clock.max(self.now());
    }
}

#[cfg(test)]
#[path = "markii_tests.rs"]
mod tests;
