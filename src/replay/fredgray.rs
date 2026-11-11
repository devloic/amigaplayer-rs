//! Fred Gray (`gray.*`: Black Lamp, Star Goose, Victory Road, Eco), played by running the module's
//! own replay routine.
//!
//! A `gray` file is an AmigaDOS hunk executable that carries the game's own sound driver: a
//! header names the play routine (called once per tick) and the init routine (called with the
//! subsong number), and the song data, the tables and the driver's variables all live in the one
//! code hunk (see `fixround/spec/fredgray`, section 2). The driver is the module's data, as the
//! sequencer routines of the specification are: this replayer loads the hunk, relocates it, and
//! runs the play and init routines on the repo's 68000 core with the standard cycle table
//! (`m68k_rc::cycles`), so every Paula write falls where the routine's own instructions put it.
//!
//! What the Eagleplayer adds around the module, from the specification (sections 6.3, 10):
//! * the first entry of the section jump table (section 0, "stop") reports the song end: here the
//!   return routine that entry names is recognised when it is reached;
//! * the note start's glide test (the status register saved around the decision whether to slide
//!   to the new pitch, and the conditional branch on it) is overwritten with no-ops, so a note
//!   always sets the current pitch at once.
//!
//! The jump table is found from the relocation list, not by a fixed offset: it is the run of
//! relocated longs whose first entry names a lone `rts` placed directly behind the table. The glide
//! test is found by its shape in the module's code. A module of the same driver family needs no
//! table of offsets.

use super::{Hw, Open, Replayer, be16, be32};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{AddressBus, CpuCore, CpuType, StepResult};

pub const OPEN: Open = Open { name: "Fred Gray", detect, new };

/// Cycles of the Eagleplayer's interrupt wrapper before the play routine (calibrated against the
/// reference's first write of a tick).
const TICK_WRAP: u64 = 0;

/// Colour clocks from the start of the init routine to its first Paula store, in the builds that
/// store soonest after a start at the run's first phase (the clock of a start is 100 into a frame).
const FIRST_STORE: u64 = 33;
/// The timer starts when the last init returns and its interrupts come one 50 Hz period apart;
/// the interrupt wrapper's set-up puts the play routine's entry this long after the init's final
/// DMA-enable store plus a whole number of periods, with the build's `tick_k` on top (colour clocks).
const WRAP_CC: u64 = 126;

/// The wrapper's scan of the module's code before the first init (specification section 12.1):
/// the counts of words it steps over in its four stages and the flags that change the cost.
#[derive(Debug, Default, PartialEq, Eq)]
struct Scan {
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    /// The jump table ends in a zero long rather than the return routine.
    zero_end: bool,
    /// The marker long of the last stage was found before its limit.
    found: bool,
    /// The word behind the patched area has a zero low byte (the long branch form).
    long_form: bool,
}

fn w16(h: &[u8], o: usize) -> Option<u16> {
    h.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn w32(h: &[u8], o: usize) -> Option<u32> {
    h.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Counts the scan stages. `play` is the play entry, `entry0` the offset of the first jump table
/// entry. A marker that is not in the image ends its stage at the image's end.
fn scan(h: &[u8], play: usize, entry0: usize) -> Scan {
    let mut sc = Scan::default();
    // stage A: from the play entry to the first word 0xE586, inclusive
    let mut p = play;
    while p + 2 <= h.len() && w16(h, p) != Some(0xE586) {
        p += 2;
    }
    sc.a = ((p - play) / 2 + 1) as u64;
    // stage B: the table from entry 1 to its terminator
    let e1 = entry0 + 4;
    let mut t = e1;
    while t + 4 <= h.len() && w16(h, t) != Some(0x4E75) && w32(h, t) != Some(0) {
        t += 4;
    }
    sc.zero_end = w16(h, t) != Some(0x4E75) && w32(h, t) == Some(0);
    sc.b = 1 + ((t - e1) / 4) as u64;
    // stage C: after the marker, to the first 0x40E7 and then the first 0x46DF behind it
    let from = p + 2;
    let mut q = from;
    while q + 2 <= h.len() && w16(h, q) != Some(0x40E7) {
        q += 2;
    }
    while q + 2 <= h.len() && w16(h, q) != Some(0x46DF) {
        q += 2;
    }
    sc.c = (q.saturating_sub(from) / 2).saturating_sub(1) as u64;
    // stage D: from behind the patched area to the marker long, or the limit
    sc.long_form = w16(h, q + 2).is_some_and(|w| w & 0xFF == 0);
    let st = q + if sc.long_form { 6 } else { 4 };
    let lim = play + 0xD48;
    let mut r = st;
    while r < lim && r + 4 <= h.len() {
        if w32(h, r) == Some(0x3BBC_0080) {
            sc.found = true;
            break;
        }
        r += 2;
    }
    sc.d = (r.saturating_sub(st) / 2).saturating_sub(1) as u64;
    sc
}

/// The time of the first write of the first init, from the start of the run (specification 12.1),
/// in colour clocks, rounded to the nearest.
fn first_init_time(sc: &Scan) -> u64 {
    // in half colour clocks
    let t2 = 2 * 1686
        + 21 * sc.a
        + 60 * sc.b
        + 44 * sc.c
        + 52 * sc.d
        + if sc.zero_end { 22 } else { 0 }
        + if sc.found { 28 } else { 0 }
        + if sc.long_form { 17 } else { 0 };
    t2.div_ceil(2)
}
/// A change of subsong: colour clocks from the end of the last init to the host's DMA stop, and to
/// the next init.
const STOP_AT: u64 = 14;
const CHANGE_CC: u64 = 916;

fn detect(d: &[u8]) -> bool {
    d.len() > 0x40
        && be32(d, 0) == 0x3F3
        && d[0x14] != 0
        && be32(d, 0x20) == 0x70FF_4E75
        && d.get(0x24..0x2C) == Some(b"FREDGRAY")
        && (0x2C..0x38).step_by(4).all(|o| be32(d, o) != 0)
}

/// 68000 cycles (half colour clocks) the reference's wrapper takes at a store of the play routine
/// to an audio register, on top of the module's own instructions: after the period stores of
/// channels 0, 1 and 2, after the volume stores, after the low word of a pointer and after a
/// length store, and before the first channel's period store (`pre`). Measured store by store
/// against reference logs that carry the half colour clock: every figure is the same on every
/// tick of every module of the build.
struct Extra {
    per: [u64; 3],
    vol: [u64; 3],
    loc: u64,
    /// An odd number of cycles (whatever the cycle of the store): the stores behind a length
    /// store fall on the other half of the colour clock.
    len: u64,
    pre: u64,
    /// Timer start calibration (colour clocks): the specification's 122-131 colour clocks of
    /// play-entry delay is a cycle estimate; the first store of a tick measured against the logs
    /// sets this.
    tick_k: u64,
}

/// The builds that store through the `a5` base (Black Lamp, Star Goose, Victory Road).
const EXTRA_A: Extra = Extra { per: [12, 12, 116], vol: [104, 104, 128], loc: 20, len: 183, pre: 0, tick_k: 22 };
/// The build that stores to absolute addresses (Eco): its pointer and length stores cost nothing.
const EXTRA_B: Extra = Extra { per: [44, 60, 148], vol: [104, 104, 128], loc: 0, len: 0, pre: 68, tick_k: 2 };

/// Cycles the reference's song-end routine (the replaced first jump table entry, a call into the
/// host) takes on top of a plain return, on every tick that reaches it. The engine's time after a
/// host call is a whole colour clock; every measured case reached it on one, so the dropped odd
/// cycle is the engine's rule here, not a measurement.
const SONG_END: u64 = 116;

/// The bus the routine sees: in the play routine its audio stores take the wrapper's time.
struct HookBus<'a> {
    bus: &'a mut Bus,
    play: bool,
    x: &'static Extra,
    /// Cycles of the call so far, the running instruction included.
    cycles: u64,
    /// Cycles the wrapper takes after this instruction's stores.
    extra: u64,
    l: Option<u64>,
}

impl HookBus<'_> {
    fn store(&mut self, a: u32) {
        let a = a & 0xFF_FFFF;
        if self.play && (0xDF_F0A0..0xDF_F0D0).contains(&a) {
            let ch = ((a - 0xDF_F0A0) >> 4) as usize;
            if a & 0xF == 6 && ch == 0 && self.x.pre != 0 {
                self.cycles += self.x.pre;
                m68k_rc::Bus::cycles(self.bus, self.cycles);
            }
            self.extra += match a & 0xF {
                2 => self.x.loc,
                4 => self.x.len,
                6 => self.x.per[ch],
                8 => self.x.vol[ch],
                _ => 0,
            };
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
        self.store(a);
        if !self.play && a & 0xFF_FFFF == 0xDF_F096 && v == 0x8007 {
            self.l = Some(self.bus.clock);
        }
        self.bus.write_word(a, v)
    }
    fn write_long(&mut self, a: u32, v: u32) {
        self.store(a);
        self.bus.write_long(a, v)
    }
    fn fast_mem(&mut self) -> Option<m68k::FastMem> {
        self.bus.fast_mem()
    }
}

struct Cpu68 {
    cpu: CpuCore,
    sp: u32,
    x: &'static Extra,
    /// The time of the last DMA-enable store of an init run.
    l: Option<u64>,
}

impl Cpu68 {
    fn new(sp: u32, x: &'static Extra) -> Cpu68 {
        let mut cpu = CpuCore::new();
        cpu.set_cpu_type(CpuType::M68000);
        Cpu68 { cpu, sp, x, l: None }
    }

    /// Runs the routine at `entry` until it returns, from `start`, with `d0` in D0 and A5 at the
    /// custom chips. Returns the cycles it took (counting `pre` and, in the play routine, the
    /// wrapper's time at the stores and at the song end), and whether it reached `stop`.
    fn call(&mut self, bus: &mut Bus, entry: u32, start: u64, pre: u64, d0: u32, stop: u32, play: bool) -> Result<(u64, bool), String> {
        let ret = self.sp & !3;
        let sp = ret - 4;
        bus.w32(sp, ret);
        self.cpu.dar = [0; 16];
        self.cpu.dar[0] = d0;
        self.cpu.dar[13] = 0x00DF_F000;
        self.cpu.dar[15] = sp;
        self.cpu.set_sr(0x2000);
        self.cpu.pc = entry;
        self.cpu.invalidate_prefetch();
        bus.clock = start;
        bus.epoch = (start, 0);
        let mut cycles = pre;
        let mut executed = 0u64;
        let mut ended = false;
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok((cycles, ended));
            }
            if pc == stop {
                ended = true;
                if play {
                    cycles = (cycles + SONG_END) & !1;
                }
            }
            executed += 1;
            if executed > 5_000_000 {
                return Err(format!("fredgray: routine did not return (PC=${pc:06x})"));
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68000);
            cycles += m68k_rc::cycles::cycles(&ins) as u64;
            m68k_rc::Bus::cycles(bus, cycles);
            let mut hb = HookBus { bus, play, x: self.x, cycles, extra: 0, l: None };
            let r = self.cpu.step(&mut hb);
            cycles = hb.cycles + hb.extra;
            if hb.l.is_some() {
                self.l = hb.l;
            }
            match r {
                StepResult::Ok { .. } => {}
                other => return Err(format!("fredgray: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }
}

struct FredGray {
    cpu: Cpu68,
    h: Vec<u8>,
    img: u32,
    /// Address of the return routine that section 0 names.
    stop: u32,
    title: String,
    subs: u16,
    cur: u16,
    songend: bool,
    resume: Option<u64>,
    /// Ticks run so far: the reference's timer is not restarted by a change of subsong once it runs.
    ticks: u64,
    /// Colour clocks from a timer interrupt to the play routine's entry: the interrupt wrapper's
    /// set-up. The timer itself starts at the end of the last init. The interrupt is the engine's
    /// event, not the entry: the audio DMA of the lines in between reads the wave buffer as the
    /// tick leaves it at its first register store, in the reference too (Eco rewrites its buffer
    /// before that store).
    wrap: u64,
}

/// Adds the load address to every longword the hunk's relocation list names; returns their
/// offsets within the hunk's data.
fn relocate(d: &[u8], words: usize, bus: &mut Bus, img: u32) -> Vec<usize> {
    let mut offs = Vec::new();
    let mut p = 0x20 + 4 * words;
    if d.len() < p + 4 || be32(d, p) != 0x3EC {
        return offs;
    }
    p += 4;
    loop {
        if p + 4 > d.len() {
            break;
        }
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
                offs.push(off as usize);
            }
        }
        p += 4 * count;
    }
    offs
}

/// The first entry of the section jump table (section 0, "stop"): the table is a run of
/// relocated longs (at least a few) whose first entry names a lone `rts`. Of the runs the
/// longest is taken. Returns the offset of that entry. (In Eco the `rts` is the return of another
/// routine, so the entry is pointed at a return of its own, as the Eagleplayer replaces it.)
fn find_stop(h: &[u8], relocs: &[usize]) -> Option<usize> {
    let mut sorted = relocs.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1] == sorted[j] + 4 {
            j += 1;
        }
        // the run is sorted[i..=j]; the table starts at its first entry that names an rts
        for k in i..=j {
            let v = be32(h, sorted[k]) as usize;
            if v + 2 <= h.len() && be16(h, v) == 0x4E75 {
                let n = j - k + 1;
                if n >= 4 && best.is_none_or(|(bn, _)| n > bn) {
                    best = Some((n, sorted[k]));
                }
                break;
            }
        }
        i = j + 1;
    }
    best.map(|(_, o)| o)
}

/// Overwrites the glide decision of the note start with no-ops: the store of the status register
/// right after the bit clear of the note's top bit, and, further on, the restore of the status
/// register together with the conditional branch that follows it. The reference does not look at
/// what follows the restore: it overwrites the restore and the word behind it, and one word more
/// when that word's low byte is zero (in a module edited to have a shift there, each note start
/// of the reference is shorter by the shift's cycles less a no-op's).
fn patch_glide(h: &[u8], bus: &mut Bus, img: u32) {
    let mut p = 0;
    while p + 6 <= h.len() {
        if be16(h, p) == 0x0884 && be16(h, p + 2) == 7 && be16(h, p + 4) == 0x40E7 {
            let mut q = p + 6;
            while q < p + 40 && q + 2 <= h.len() && be16(h, q) != 0x46DF {
                q += 2;
            }
            if q < p + 40 && q + 4 <= h.len() {
                // the word behind the restore is taken for the branch whatever it is: a zero low
                // byte is the long form (specification 12.1)
                let words = if be16(h, q + 2) & 0xFF == 0 && q + 6 <= h.len() { 3 } else { 2 };
                bus.w16(img + p as u32 + 4, 0x4E71);
                for i in 0..words {
                    bus.w16(img + q as u32 + 2 * i, 0x4E71);
                }
                p = q;
            }
        }
        p += 2;
    }
}

/// Whether the output stage stores its period words to absolute addresses (the first channel's
/// period register as a long absolute operand of a register-to-memory word move), as Eco's does.
fn absolute_stores(h: &[u8]) -> bool {
    (0..h.len().saturating_sub(6)).step_by(2).any(|p| be16(h, p) == 0x33C0 && be32(h, p + 2) == 0x00DF_F0A6)
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let words = (be32(d, 0x1C) as usize) & 0x00FF_FFFF;
    let h = d.get(0x20..0x20 + 4 * words)?;
    // the hunk gets a memory of its own, as the loader gives it (the module's file stays where it is)
    let _ = base;
    let img = m.alloc(4 * words as u32);
    if img == 0 {
        return None;
    }
    m.bus.write_bytes(img, h);
    let relocs = relocate(d, words, &mut m.bus, img);
    let entry0 = find_stop(h, &relocs)?;
    patch_glide(h, &mut m.bus, img);
    let stack = m.alloc(0x1000);
    if stack == 0 {
        return None;
    }
    // section 0 gets a return routine of its own, whose being reached is the song end
    let stop = stack;
    m.bus.w16(stop, 0x4E75);
    m.bus.w32(img + entry0 as u32, stop);
    let title = {
        let o = be32(h, 0x18) as usize;
        h.get(o..).map_or(String::new(), |s| s.iter().take_while(|&&b| b != 0).take(80).map(|&b| b as char).collect())
    };
    let subs = (be32(h, 0x14) as u16).max(1);
    let mut s = FredGray {
        cpu: Cpu68::new(stack + 0x1000, if absolute_stores(h) { &EXTRA_B } else { &EXTRA_A }),
        h: h.to_vec(),
        img,
        stop,
        title,
        subs,
        cur: 0,
        songend: false,
        resume: None,
        ticks: 0,
        wrap: 0,
    };
    // the first write of the first init falls at the time the scan gives (the init's own first
    // store comes FIRST_STORE colour clocks after the start's clock, which holds the phase)
    let t = first_init_time(&scan(h, be32(h, 0x0C) as usize, entry0));
    m.bus.clock = t.saturating_sub(FIRST_STORE).max(m.bus.clock);
    s.init_song(&mut m.bus, 0)?;
    Some(Box::new(s))
}

impl FredGray {
    fn init_song(&mut self, bus: &mut Bus, sub: u16) -> Option<()> {
        let entry = self.img + be32(&self.h, 0x10);
        let start = bus.clock;
        let (cyc, _) = self.cpu.call(bus, entry, start, 0, sub as u32, self.stop, false).map_err(|e| log::error!("{e}")).ok()?;
        bus.clock = start + cyc / 2;
        if let Some(l) = self.cpu.l.take() {
            self.wrap = (l + WRAP_CC + self.cpu.x.tick_k).saturating_sub(bus.clock);
        }
        Some(())
    }
}

impl Replayer for FredGray {
    fn format(&self) -> &'static str {
        "Fred Gray"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subs - 1, self.cur)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        if n < self.subs {
            self.songend = false;
            self.cur = n;
            // the reference's change of subsong: the host stops all voices, then calls the init
            let t0 = hw.bus.clock;
            hw.bus.clock = t0 + STOP_AT;
            hw.bus.write_word(0xDF_F096, 0x000F);
            for ch in 0..4 {
                hw.bus.clock += 8;
                hw.bus.write_word(0xDF_F0A8 + 16 * ch, 0);
            }
            hw.bus.clock = t0 + CHANGE_CC;
            if self.init_song(hw.bus, n).is_some() && self.ticks == 0 {
                self.resume = Some(hw.bus.clock + super::DT_TICK);
            }
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticks += 1;
        // an interrupt that fell due while the host changed the subsong is served when the init
        // returns (the engine calls at once): the wrapper's set-up comes after it as on any tick
        let start = hw.bus.clock + self.wrap;
        let play = self.img + be32(&self.h, 0x0C);
        match self.cpu.call(hw.bus, play, start, TICK_WRAP, 0, self.stop, true) {
            Ok((c, ended)) => {
                hw.bus.clock = start + c / 2;
                if ended {
                    self.songend = true;
                }
            }
            Err(e) => {
                log::error!("{e}");
                self.songend = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny module of the format: init writes a length, play writes a period and calls
    /// the first jump table entry (section 0), whose routine is the song end.
    fn module() -> Vec<u8> {
        let mut h = vec![0u8; 0x80];
        let mut put = |o: usize, b: &[u8]| h[o..o + b.len()].copy_from_slice(b);
        put(0, &[0x70, 0xFF, 0x4E, 0x75]);
        put(4, b"FREDGRAY");
        put(0x0C, &0x30u32.to_be_bytes());
        put(0x10, &0x44u32.to_be_bytes());
        put(0x14, &2u32.to_be_bytes());
        put(0x18, &0x70u32.to_be_bytes());
        put(0x1C, &0x70u32.to_be_bytes());
        put(0x20, &0x70u32.to_be_bytes());
        put(0x24, &0x80u32.to_be_bytes());
        // play: store a period word to the first channel, load the first table entry, call it, return
        put(0x30, &[0x33, 0xFC, 0x01, 0x23, 0x00, 0xDF, 0xF0, 0xA6, 0x20, 0x79, 0, 0, 0, 0x54, 0x4E, 0x90, 0x4E, 0x75]);
        // init: store a length word to the first channel, return
        put(0x44, &[0x33, 0xFC, 0x00, 0x08, 0x00, 0xDF, 0xF0, 0xA4, 0x4E, 0x75]);
        for i in 0..4 {
            put(0x54 + 4 * i, &0x68u32.to_be_bytes());
        }
        put(0x68, &[0x4E, 0x75]);
        put(0x70, b"Test\0");
        let mut f = Vec::new();
        for l in [0x3F3u32, 0, 1, 0, 0, 0x4000_0020, 0x3E9, 0x20] {
            f.extend(l.to_be_bytes());
        }
        f.extend(&h);
        for l in [0x3ECu32, 5, 0, 0x3A, 0x54, 0x58, 0x5C, 0x60, 0, 0x3F2] {
            f.extend(l.to_be_bytes());
        }
        f
    }

    fn start(d: &[u8]) -> (Machine, Box<dyn Replayer>) {
        let mut m = Machine::new(CpuType::M68000);
        m.bus.write_log = Some(Vec::new());
        let base = m.alloc(d.len() as u32);
        m.bus.write_bytes(base, d);
        let r = new(d, base, &mut m).expect("recognised");
        (m, r)
    }

    #[test]
    fn detect_checks_the_header() {
        let d = module();
        assert!(detect(&d));
        let mut bad = d.clone();
        bad[0x28] = b'X';
        assert!(!detect(&bad));
        let mut bad = d.clone();
        bad[0x14] = 0;
        assert!(!detect(&bad));
        let mut bad = d;
        bad[0x2C..0x30].fill(0);
        assert!(!detect(&bad));
    }

    #[test]
    fn init_play_and_song_end() {
        let d = module();
        let (mut m, mut r) = start(&d);
        assert_eq!(r.title(), "Test");
        assert_eq!(r.subsongs(), (0, 1, 0));
        let init = m.bus.write_log.replace(Vec::new()).unwrap();
        assert_eq!(init.iter().map(|&(a, v, _)| (a, v)).collect::<Vec<_>>(), vec![(0xDFF0A4, 8)]);
        assert!(!r.songend());
        r.tick(&mut Hw { bus: &mut m.bus });
        let log = m.bus.write_log.take().unwrap();
        assert_eq!(log.iter().map(|&(a, v, _)| (a, v)).collect::<Vec<_>>(), vec![(0xDFF0A6, 0x123)]);
        // section 0 was reached through the jump table
        assert!(r.songend());
    }

    #[test]
    fn jump_table_is_the_run_naming_a_return() {
        let d = module();
        let h = &d[0x20..0xA0];
        assert_eq!(find_stop(h, &[0x3A, 0x54, 0x58, 0x5C, 0x60]), Some(0x54));
        // fewer than four entries: no table
        assert_eq!(find_stop(h, &[0x3A, 0x54, 0x58]), None);
    }

    #[test]
    fn glide_test_is_overwritten_with_nops() {
        // bit clear, status register save, a byte store, status register restore, short not-equal branch, a word store
        let code: [u16; 12] = [0x0884, 7, 0x40E7, 0x1D84, 0x6002, 0x46DF, 0x6604, 0x3D84, 0x600A, 0x4E75, 0, 0];
        let mut m = Machine::new(CpuType::M68000);
        for (i, w) in code.iter().enumerate() {
            m.bus.w16(0x1000 + 2 * i as u32, *w);
        }
        let h: Vec<u8> = code.iter().flat_map(|w| w.to_be_bytes()).collect();
        patch_glide(&h, &mut m.bus, 0x1000);
        let out: Vec<u16> = (0..12).map(|i| m.bus.r16(0x1000 + 2 * i)).collect();
        assert_eq!(out[..7], [0x0884, 7, 0x4E71, 0x1D84, 0x6002, 0x4E71, 0x4E71]);
        assert_eq!(out[7..], code[7..]);
        // the word behind the restore is overwritten whatever it is, and one more when its low byte is zero
        for (word, n) in [(0xE544u16, 2), (0x6600, 3), (0x7000, 3)] {
            let mut c = code;
            c[6] = word;
            for (i, w) in c.iter().enumerate() {
                m.bus.w16(0x1000 + 2 * i as u32, *w);
            }
            let h: Vec<u8> = c.iter().flat_map(|w| w.to_be_bytes()).collect();
            patch_glide(&h, &mut m.bus, 0x1000);
            let out: Vec<u16> = (0..12).map(|i| m.bus.r16(0x1000 + 2 * i)).collect();
            assert!(out[5..5 + n].iter().all(|&w| w == 0x4E71), "{word:04x}");
            assert_eq!(out[5 + n..], c[5 + n..], "{word:04x}");
        }
    }

    /// The wrapper's time at the stores of the first build, in 68000 cycles on top of the 20 of
    /// each `move.w #x,abs.l`: 12 after the first two periods, 183 after a length (an odd number:
    /// the store behind it falls half a colour clock later than a whole count would put it).
    #[test]
    fn store_costs_are_counted_in_cycles() {
        let mut d = module();
        let play: [u16; 17] = [
            0x33FC, 0x0123, 0x00DF, 0xF0A6, 0x33FC, 0x0124, 0x00DF, 0xF0B6, 0x33FC, 0x0008, 0x00DF, 0xF0A4, 0x33FC, 0x0001,
            0x00DF, 0xF0B8, 0x4E75,
        ];
        // the play routine goes into new room behind the hunk's 0x80 bytes; no relocation touches it
        let tail = d.split_off(0x20 + 0x80);
        d.extend(vec![0u8; 0x40]);
        d.extend(tail);
        let code: Vec<u8> = play.iter().flat_map(|w| w.to_be_bytes()).collect();
        d[0x20 + 0x90..0x20 + 0x90 + code.len()].copy_from_slice(&code);
        d[0x20 + 0x0C..0x20 + 0x10].copy_from_slice(&0x90u32.to_be_bytes());
        d[0x14..0x18].copy_from_slice(&0x4000_0030u32.to_be_bytes());
        d[0x1C..0x20].copy_from_slice(&0x30u32.to_be_bytes());
        let (mut m, mut r) = start(&d);
        m.bus.write_log.replace(Vec::new());
        let t0 = m.bus.clock;
        r.tick(&mut Hw { bus: &mut m.bus });
        let log = m.bus.write_log.take().unwrap();
        let cycles = [20u64, 20 + 12 + 20, 52 + 12 + 20, 84 + 183 + 20];
        let want: Vec<(u32, u64)> = [0xDFF0A6u32, 0xDFF0B6, 0xDFF0A4, 0xDFF0B8].into_iter().zip(cycles.map(|c| c / 2)).collect();
        assert_eq!(log.iter().map(|&(a, _, t)| (a, t - t0)).collect::<Vec<_>>(), want);
        // the volume store's own wrapper time and the return
        assert_eq!(m.bus.clock - t0, (287 + 104 + 16) / 2);
    }

    /// A tick that reaches the song end takes the host call's time on top of the return.
    #[test]
    fn song_end_costs_the_host_call() {
        let d = module();
        let (mut m, mut r) = start(&d);
        let t0 = m.bus.clock;
        r.tick(&mut Hw { bus: &mut m.bus });
        assert!(r.songend());
        let with_end = m.bus.clock - t0;
        // the same play routine with the call through the table replaced by a no-op
        let mut e = d.clone();
        e[0x20 + 0x3E..0x20 + 0x40].copy_from_slice(&[0x4E, 0x71]);
        let (mut m, mut r) = start(&e);
        let t0 = m.bus.clock;
        r.tick(&mut Hw { bus: &mut m.bus });
        assert!(!r.songend());
        // the call (16 cycles) and the return (16) against the no-op (4), and the host call
        assert_eq!(with_end - (m.bus.clock - t0), (16 + 16 - 4 + SONG_END) / 2);
    }

    #[test]
    fn absolute_output_stage_is_the_second_build() {
        assert!(absolute_stores(&[0x33, 0xC0, 0x00, 0xDF, 0xF0, 0xA6, 0, 0]));
        assert!(!absolute_stores(&[0x3B, 0x45, 0x00, 0xA6, 0, 0, 0, 0]));
    }

    #[test]
    fn scan_counts_the_stages() {
        // words: 3 filler, marker (A = 4), 2 filler, first marker of the glide pair, 2 filler,
        // second marker with a short branch behind it, 2 filler, the long marker, then the table
        let mut w = vec![0x4E71u16; 60];
        w[3] = 0xE586;
        w[6] = 0x40E7;
        w[9] = 0x46DF;
        w[10] = 0x6604;
        w[13] = 0x3BBC;
        w[14] = 0x0080;
        let mut h: Vec<u8> = w.iter().flat_map(|x| x.to_be_bytes()).collect();
        let put = |h: &mut Vec<u8>, o: usize, v: u32| h[o..o + 4].copy_from_slice(&v.to_be_bytes());
        for (o, v) in [(80, 0x20u32), (84, 0x20), (88, 0x20), (92, 0x20), (96, 0x4E75_0000)] {
            put(&mut h, o, v);
        }
        let sc = scan(&h, 0, 80);
        assert_eq!(sc, Scan { a: 4, b: 4, c: 4, d: 1, zero_end: false, found: true, long_form: false });
        // a zero terminator and a long branch form
        put(&mut h, 96, 0);
        h[20..22].copy_from_slice(&0x6600u16.to_be_bytes());
        let sc = scan(&h, 0, 80);
        assert!(sc.zero_end && sc.long_form);
        assert_eq!(sc.b, 4);
        // no marker long within the image: the stage runs to its limit or the image's end
        let h2 = vec![0x4Eu8; 40];
        assert!(!scan(&h2, 0, 8).found);
    }

    #[test]
    fn start_time_formula() {
        let sc = Scan { a: 62, b: 20, c: 696, d: 938, ..Default::default() };
        assert_eq!(first_init_time(&sc), 42637);
        let sc = Scan { a: 75, b: 46, c: 1442, d: 123, zero_end: false, found: true, long_form: true };
        assert_eq!(first_init_time(&sc), 38798);
        let sc = Scan { a: 80, b: 26, c: 1174, d: 441, zero_end: true, found: false, long_form: true };
        assert_eq!(first_init_time(&sc), 40620);
    }

    #[test]
    fn unknown_module_start_comes_from_the_scan() {
        let first_write = |d: &[u8]| {
            let (m, _r) = start(d);
            m.bus.write_log.as_ref().unwrap()[0].2
        };
        let d = module();
        let t0 = first_write(&d);
        // an edit that moves the first marker closer to the play entry changes the scan, hence the start
        let mut e = d.clone();
        e[0x20 + 0x30 + 2..0x20 + 0x30 + 4].copy_from_slice(&0xE586u16.to_be_bytes());
        let t1 = first_write(&e);
        assert_ne!(t0, t1);
        assert!(t0 > 1686 && t1 > 1686);
        assert_ne!(t0, 39000);
    }
}
