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

/// What the reference's wrapper does before the first init (it patches the module: the time it
/// takes depends on the module, in a way the specification does not give) and the timer delay of
/// the four known modules, keyed by hunk size in longwords and the play and init entries:
/// (size, play, init, colour clocks before the init, timer delay).
const KNOWN: [(u32, u32, u32, u64, u64); 4] = [
    (0x4C6, 0xE2, 0x8C, 42504, 140),
    (0x5E5, 0x1DA, 0xA6, 40486, 120),
    (0x72D, 0xE4, 0x8E, 38665, 140),
    (0x740, 0xEC, 0xA6, 34589, 140),
];
/// The start of a module not in the table: about the mean of the four.
const START_OTHER: (u64, u64) = (39000, 140);
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

/// Colour clocks the reference's wrapper takes at a store of the play routine to an audio
/// register, measured against the plain cycle count of the module's own instructions (the
/// specification's timing section): after the period stores of channels 0, 1 and 2, after the
/// volume stores, after a pointer store and after a length store, and before the first period
/// store of a tick (`pre`; `pre_loc` when the store before it was one or two pointer stores, not three).
struct Extra {
    per: [u64; 3],
    vol: [u64; 3],
    loc: u64,
    len: u64,
    pre: u64,
    pre_loc: u64,
}

/// The builds that store through the `a5` base (Black Lamp, Star Goose, Victory Road).
const EXTRA_A: Extra = Extra { per: [6, 6, 58], vol: [52, 52, 64], loc: 10, len: 92, pre: 0, pre_loc: 0 };
/// The build that stores to absolute addresses (Eco).
const EXTRA_B: Extra = Extra { per: [22, 30, 74], vol: [52, 52, 64], loc: 10, len: 0, pre: 34, pre_loc: 24 };

/// The bus the routine sees: in the play routine its audio stores take the wrapper's time.
struct HookBus<'a> {
    bus: &'a mut Bus,
    play: bool,
    x: &'static Extra,
    extra: u64,
    /// Pointer stores in a row so far.
    run: u32,
}

impl HookBus<'_> {
    fn store(&mut self, a: u32) {
        let a = a & 0xFF_FFFF;
        if self.play && (0xDF_F0A0..0xDF_F0D0).contains(&a) {
            let ch = ((a - 0xDF_F0A0) >> 4) as usize;
            if a & 0xF == 6 && ch == 0 {
                let pre = if (1..3).contains(&self.run) { self.x.pre_loc } else { self.x.pre };
                self.bus.epoch.0 += pre;
                self.bus.clock += pre;
            }
            self.run = if a & 0xF == 2 { self.run + 1 } else { 0 };
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
    run: u32,
    x: &'static Extra,
}

impl Cpu68 {
    fn new(sp: u32, x: &'static Extra) -> Cpu68 {
        let mut cpu = CpuCore::new();
        cpu.set_cpu_type(CpuType::M68000);
        Cpu68 { cpu, sp, run: 0, x }
    }

    /// Runs the routine at `entry` until it returns, from `start`, with `d0` in D0 and A5 at the
    /// custom chips. Returns the cycles it took (counting `pre`), and whether it reached `stop`.
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
        self.run = 0;
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok((cycles, ended));
            }
            if pc == stop {
                ended = true;
            }
            executed += 1;
            if executed > 5_000_000 {
                return Err(format!("fredgray: routine did not return (PC=${pc:06x})"));
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68000);
            cycles += m68k_rc::cycles::cycles(&ins) as u64;
            m68k_rc::Bus::cycles(bus, cycles);
            let mut hb = HookBus { bus, play, x: self.x, extra: 0, run: self.run };
            let r = self.cpu.step(&mut hb);
            let extra = hb.extra;
            self.run = hb.run;
            if extra != 0 {
                bus.epoch.0 += extra;
                bus.clock += extra;
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
    /// Colour clocks before the first init and the timer delay.
    start: (u64, u64),
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

/// Overwrites the glide decision of the note start with no-ops: `move sr,-(a7)` after the
/// `bclr #7,d4`, and, further on, `move (a7)+,sr` with the conditional branch that follows it.
fn patch_glide(h: &[u8], bus: &mut Bus, img: u32) {
    let mut p = 0;
    while p + 6 <= h.len() {
        if be16(h, p) == 0x0884 && be16(h, p + 2) == 7 && be16(h, p + 4) == 0x40E7 {
            let mut q = p + 6;
            while q < p + 40 && q + 2 <= h.len() && be16(h, q) != 0x46DF {
                q += 2;
            }
            if q < p + 40 && q + 4 <= h.len() {
                let bne = be16(h, q + 2);
                let words = if bne & 0xFF00 == 0x6600 && bne & 0xFF != 0 { 2 } else if bne == 0x6600 { 3 } else { 0 };
                if words != 0 {
                    bus.w16(img + p as u32 + 4, 0x4E71);
                    for i in 0..words {
                        bus.w16(img + q as u32 + 2 * i, 0x4E71);
                    }
                    p = q;
                }
            }
        }
        p += 2;
    }
}

/// Whether the output stage stores to absolute addresses (`move.w d0,$dff0a6.l`), as Eco's does.
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
        start: KNOWN
            .iter()
            .find(|k| (k.0, k.1, k.2) == (words as u32, be32(h, 0x0C), be32(h, 0x10)))
            .map_or(START_OTHER, |k| (k.3, k.4)),
    };
    m.bus.clock += s.start.0;
    s.init_song(&mut m.bus, 0)?;
    Some(Box::new(s))
}

impl FredGray {
    fn init_song(&mut self, bus: &mut Bus, sub: u16) -> Option<()> {
        let entry = self.img + be32(&self.h, 0x10);
        let start = bus.clock;
        let (cyc, _) = self.cpu.call(bus, entry, start, 0, sub as u32, self.stop, false).map_err(|e| log::error!("{e}")).ok()?;
        bus.clock = start + cyc / 2;
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

    fn timer_delay(&self) -> u64 {
        self.start.1
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
                self.resume = Some(hw.bus.clock + super::DT_TICK + self.start.1);
            }
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticks += 1;
        let start = hw.bus.clock;
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
        // play: move.w #$123,$dff0a6 ; movea.l $54.l,a0 ; jsr (a0) ; rts
        put(0x30, &[0x33, 0xFC, 0x01, 0x23, 0x00, 0xDF, 0xF0, 0xA6, 0x20, 0x79, 0, 0, 0, 0x54, 0x4E, 0x90, 0x4E, 0x75]);
        // init: move.w #8,$dff0a4 ; rts
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
        // bclr #7,d4 ; move sr,-(a7) ; move.b d4,2(a6,d6.w) ; move (a7)+,sr ; bne.b +4 ; move.w d4,$a(a6,d6.w)
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
    }

    #[test]
    fn absolute_output_stage_is_the_second_build() {
        assert!(absolute_stores(&[0x33, 0xC0, 0x00, 0xDF, 0xF0, 0xA6, 0, 0]));
        assert!(!absolute_stores(&[0x3B, 0x45, 0x00, 0xA6, 0, 0, 0, 0]));
    }
}
