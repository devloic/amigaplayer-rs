//! Fallback for Sound Programming Language (`spl.*`) and Beathoven Synthesizer (`bss.*`) files
//! of a build the open ports (`soundprogramminglanguage`, `beathoven`) were not fitted to.
//!
//! Both formats are AmigaOS hunk executables of one code hunk that carry the game's own replay
//! routines, its tables, its note data and its samples, behind a small header of the Wanted Team.
//! The ports interpret the data of the builds they were measured on; a build they do not know is
//! played here instead, by running the hunk's own code on the repo's 68000 core with the standard
//! cycle table, as `riffraff` and `markii` do: the hunk is relocated where the host loaded the file,
//! one routine starts a subsong and another is called once per timer tick, and every Paula write
//! falls where the routine's own instructions put it.
//!
//! The entry points and the calling convention come from the header and from black-box runs only
//! (calls of the routines the header names, their writes into the hunk and to Paula compared with
//! the write log of UADE's player); the code inside the hunks was not read.
//!
//! * Sound Programming Language (`!SOPROL!` at hunk offset 4): the longs at hunk offsets 0x0C and
//!   0x10 are the tick routine and the start routine, the long at 0x14 the number of subsongs, the
//!   long at 0x18 the offset of the title and the long at 0x34 the timer's reload value (a tick
//!   every `timer + 1` E-clock cycles). The start routine takes the subsong, counted from 1, in D0.
//! * Beathoven Synthesizer (`BEATHOVEN` and a generation tag at hunk offset 4): the longs at hunk
//!   offsets 0x10 and 0x14 are the tick routine and the start routine, the long at 0x18 the number
//!   of subsongs and the long at 0x24 the offset of the title. The start routine takes the subsong,
//!   counted from 0, in D0. The long at 0x48 is the timer's reload value in the "100" and "109"
//!   generations (0: the host's 50 Hz); in the "NEW" one it is something else.
//!
//! Both routines are called with every other register 0 and a return address on a stack of the
//! replayer's own, in supervisor mode. A routine that does not return within a few million
//! instructions, or that the core cannot execute, ends the song and is not called again.
//!
//! No end of song is reported: no routine of the hunks is known to give one. The song plays until
//! the host stops it.
//!
//! The ports come first in `REPLAYERS`: a build either of them knows plays as it always did.

use super::{be32, Hw, Open, Replayer, DT_TICK};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{CpuCore, CpuType, StepResult};

pub const OPEN: Open = Open { name: "Embedded 68000 replayer", detect, new };

/// Instructions one call may run before the routine counts as hung.
const MAX_INSNS: u64 = 5_000_000;
/// The replayer's own stack, in bytes.
const STACK: u32 = 0x2000;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Spl,
    // `new`: the "NEW" generation, whose long at 0x48 is not a timer
    Bss { new: bool },
}

/// The format of the file and the size of its hunk in longs, if it has the header of one.
fn kind(d: &[u8]) -> Option<(Kind, usize)> {
    // HUNK_HEADER, no resident libraries, one hunk (first = last = 0), its size, HUNK_CODE
    if d.len() < 0x80 || [0, 4, 8, 12, 16].iter().zip([0x3F3, 0, 1, 0, 0]).any(|(&o, v)| be32(d, o) != v) {
        return None;
    }
    let size = be32(d, 20);
    let n = (size & 0x3FFF_FFFF) as usize;
    // the hunk starts with the Wanted Team's moveq #-1,d0 / rts
    if size >> 30 > 1 || be32(d, 24) != 0x3E9 || be32(d, 28) as usize != n || be32(d, 32) != 0x70FF_4E75 || d.len() < 32 + 4 * n || n < 0x20 {
        return None;
    }
    let k = match d.get(36..44)? {
        b"!SOPROL!" => Kind::Spl,
        b"BEATHOVE" => match d.get(44..48)? {
            b"N100" | b"N109" => Kind::Bss { new: false },
            b"NNEW" => Kind::Bss { new: true },
            _ => return None,
        },
        _ => return None,
    };
    Some((k, n))
}

fn detect(d: &[u8]) -> bool {
    kind(d).is_some()
}

pub struct Embedded {
    kind: Kind,
    stack: u32,
    init: u32,
    play: u32,
    title: String,
    subs: u16,
    cur: u16,
    tick: u64,
    songend: bool,
    /// A routine failed to return: nothing is called again.
    dead: bool,
    cpu: CpuCore,
    // the call in progress: the clock at count 0 and the cycle count
    s: u64,
    cnt: u64,
}

/// Adds the load address to every longword the hunk's relocation list names.
fn relocate(d: &[u8], words: usize, bus: &mut Bus, img: u32) {
    let mut p = 0x20 + 4 * words;
    if d.len() < p + 4 || be32(d, p) != 0x3EC {
        return;
    }
    p += 4;
    while p + 8 <= d.len() {
        let count = be32(d, p) as usize;
        if count == 0 || p + 8 + 4 * count > d.len() {
            break;
        }
        let hunk = be32(d, p + 4);
        p += 8;
        for i in 0..count {
            let off = be32(d, p + 4 * i);
            if hunk == 0 && (off as usize) + 4 <= 4 * words {
                let a = img + off;
                let v = bus.r32(a).wrapping_add(img);
                bus.w32(a, v);
            }
        }
        p += 4 * count;
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (k, words) = kind(d)?;
    let h = d.get(0x20..0x20 + 4 * words)?;
    let hl = h.len() as u32;
    let (play, init, subs, title_at, timer) = match k {
        Kind::Spl => (be32(h, 0x0C), be32(h, 0x10), be32(h, 0x14), be32(h, 0x18), be32(h, 0x34)),
        Kind::Bss { new } => (be32(h, 0x10), be32(h, 0x14), be32(h, 0x18), be32(h, 0x24), if new { 0 } else { be32(h, 0x48) }),
    };
    // both routines inside the hunk, on an even address
    if play >= hl || init >= hl || (play | init) & 1 != 0 {
        return None;
    }
    // the hunk runs where the host loaded the file, its relocation list applied in place: the
    // addresses it gives Paula are offsets into the module, as for every other format
    let img = base.checked_add(0x20)?;
    if img as usize + h.len() > m.bus.ram.len() {
        return None;
    }
    let stack = m.alloc(STACK);
    if stack == 0 {
        return None;
    }
    relocate(d, words, &mut m.bus, img);
    let title = h
        .get(title_at as usize..)
        .map_or(String::new(), |s| s.iter().take_while(|&&b| b != 0).take(80).map(|&b| b as char).collect());
    let mut cpu = CpuCore::new();
    cpu.set_cpu_type(CpuType::M68000);
    let mut r = Embedded {
        kind: k,
        stack,
        init: img + init,
        play: img + play,
        title,
        subs: subs.clamp(1, 255) as u16,
        cur: 0,
        tick: if timer == 0 { DT_TICK } else { (timer as u64 + 1) * 5 },
        songend: false,
        dead: false,
        cpu,
        s: 0,
        cnt: 0,
    };
    let t = m.bus.clock;
    r.start_song(&mut m.bus, 0, t);
    Some(Box::new(r))
}

impl Embedded {
    /// The clock at the current count.
    fn now(&self) -> u64 {
        self.s + self.cnt / 2
    }

    /// Starts a call: the clock `start` at count 0.
    fn begin(&mut self, bus: &mut Bus, start: u64) {
        self.s = start;
        self.cnt = 0;
        bus.clock = bus.clock.max(start);
        bus.epoch = (start, 0);
    }

    /// Runs the routine at `entry` with `d0` until it returns, and ends the song if it cannot.
    fn call(&mut self, bus: &mut Bus, entry: u32, d0: u32) {
        if let Err(e) = self.run(bus, entry, d0) {
            log::error!("{e}");
            self.songend = true;
            self.dead = true;
        }
        bus.clock = bus.clock.max(self.now());
    }

    /// The start routine, for subsong `n` (counted from 0).
    fn start_song(&mut self, bus: &mut Bus, n: u16, at: u64) {
        self.cur = n;
        self.begin(bus, at);
        let d0 = match self.kind {
            Kind::Spl => n as u32 + 1,
            Kind::Bss { .. } => n as u32,
        };
        self.call(bus, self.init, d0);
    }

    fn run(&mut self, bus: &mut Bus, entry: u32, d0: u32) -> Result<(), String> {
        let ret = (self.stack + STACK - 0x100) & !3;
        let sp = ret - 4;
        bus.w32(sp, ret);
        self.cpu.dar = [0; 16];
        self.cpu.dar[0] = d0;
        self.cpu.dar[15] = sp;
        self.cpu.set_sr(0x2000);
        self.cpu.pc = entry;
        self.cpu.invalidate_prefetch();
        let mut executed = 0u64;
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok(());
            }
            executed += 1;
            if executed > MAX_INSNS {
                return Err(format!("embedded68k: routine did not return (PC=${pc:06x})"));
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68000);
            self.cnt += m68k_rc::cycles::cycles(&ins) as u64;
            m68k_rc::Bus::cycles(bus, self.cnt);
            match self.cpu.step(bus) {
                StepResult::Ok { .. } => {}
                other => return Err(format!("embedded68k: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }
}

impl Replayer for Embedded {
    fn format(&self) -> &'static str {
        match self.kind {
            Kind::Spl => "Sound Programming Language",
            Kind::Bss { .. } => "Beathoven Synthesizer",
        }
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick_clocks(&self) -> u64 {
        self.tick
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subs - 1, self.cur)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if n >= self.subs || self.dead {
            return;
        }
        let t = hw.bus.clock;
        self.start_song(hw.bus, n, t);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.dead {
            return;
        }
        let start = hw.bus.clock;
        self.begin(hw.bus, start);
        self.call(hw.bus, self.play, 0);
    }
}

#[cfg(test)]
#[path = "embedded68k_tests.rs"]
mod tests;
