//! Benn Daglish SID (`bds.*`: the sound driver of Daglish's music for early Teque games,
//! 1987-88), played by running the module's own driver.
//!
//! A `bds` file is an AmigaDOS hunk executable that carries the game's own 68000 sound driver, its
//! song data and its samples, behind a header tagged `DAGLISH!` that names three entry points: a
//! play routine (once per tick), an audio-interrupt routine and an init routine (see
//! `fixround/spec/bendaglishsid`). This replayer loads the hunk, relocates it and runs those
//! routines on the repo's 68000 core with the standard cycle table (`m68k_rc::cycles`), as the Riff
//! Raff replayer does, so every Paula write falls where the driver's own instructions put it. What
//! the Eagleplayer adds around the driver is reproduced from the specification:
//! * the walk over the audio-interrupt routine that turns its end into a plain return (section 2.2);
//! * five fixed word sequences of the driver (found with the search keys of
//!   `bendaglishsid_site_keys.bin`, section 2.3) replaced by host behaviour with a fixed cost: song
//!   end detection, the location and length stores of the three voices, a raster-line wait;
//! * the start-up timeline, the tick, the audio-interrupt delivery and the two ways a song ends
//!   (sections 3 to 7).
//!
//! The five search keys and the three words of the audio-routine walk
//! (`bendaglishsid_scan_words.bin`) are words of the modules' own driver code, taken from the
//! original game drivers on the project owner's decision.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a header pointer
//! outside the hunk makes the file not recognised; a driver that runs more than a few million
//! instructions in one call ends the song; an audio-routine walk that finds no end word stops at
//! the end of the hunk; a subsong number out of range is ignored.

use super::{Hw, Open, Replayer, be16, be32};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{AddressBus, CpuCore, CpuType, StepResult};

pub const OPEN: Open = Open { name: "Benn Daglish SID", detect, new };

/// The five search keys (section 2.3): for each key a byte with its word count, then that many
/// big-endian words. Taken from the original game drivers on the project owner's decision.
static SITE_KEYS: &[u8] = include_bytes!("bendaglishsid_site_keys.bin");
/// The end word, the replacement word and the volume-load word of the audio-routine walk
/// (section 2.2). Taken from the original game drivers on the project owner's decision.
static SCAN_WORDS: &[u8] = include_bytes!("bendaglishsid_scan_words.bin");

/// Colour clocks between two ticks (PAL vertical blank).
const TICK: u64 = 70940;
/// Bytes of the hunk searched for the sites, from the play routine's start (section 2.3).
const SEARCH: usize = 2600;
/// The custom-chip base the audio-interrupt routine is called with.
const CHIP: u32 = 0xDF_F000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    SongEnd,
    Adr(u8),
    Wait,
}

/// The kind of each key, in the order of the keys.
const KINDS: [Kind; 5] = [Kind::SongEnd, Kind::Adr(0), Kind::Adr(1), Kind::Adr(2), Kind::Wait];

fn keys() -> Vec<Vec<u16>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < SITE_KEYS.len() {
        let n = SITE_KEYS[p] as usize;
        out.push((0..n).map(|i| be16(SITE_KEYS, p + 1 + 2 * i)).collect());
        p += 1 + 2 * n;
    }
    out
}

fn detect(d: &[u8]) -> bool {
    d.len() > 0x60
        && be32(d, 0) == 0x3F3
        && d[0x14] != 0
        && be32(d, 0x20) == 0x70FF_4E75
        && d.get(0x24..0x2C) == Some(b"DAGLISH!")
        && (0x2C..0x3C).step_by(4).all(|o| be32(d, o) != 0)
}

fn w16(h: &[u8], o: usize) -> Option<u16> {
    h.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

/// Where the sites are: for each word of the hunk, the kind and length in words of the site that
/// starts there.
struct Sites {
    at: Vec<Option<(Kind, u8)>>,
}

impl Sites {
    /// Searches `[start, end)` at even offsets for each key in turn; a match lies wholly inside
    /// the range and the search goes on right after it.
    fn find(img: &[u8], start: usize, end: usize) -> Sites {
        let mut at = vec![None; img.len() / 2 + 1];
        for (key, kind) in keys().iter().zip(KINDS) {
            let n = key.len();
            if n == 0 {
                continue;
            }
            let mut p = start;
            while p + 2 * n <= end {
                if (0..n).all(|i| w16(img, p + 2 * i) == Some(key[i])) {
                    at[p / 2] = Some((kind, n as u8));
                    p += 2 * n;
                } else {
                    p += 2;
                }
            }
        }
        Sites { at }
    }
}

/// The walk over the audio-interrupt routine (section 2.2): words examined (the end word
/// included), volume-load words found, and the offset of the end word if there is one.
fn scan(img: &[u8], start: usize) -> (u64, u64, Option<usize>) {
    let (end, vol) = (be16(SCAN_WORDS, 0), be16(SCAN_WORDS, 4));
    let (mut w, mut m) = (0, 0);
    let mut p = start & !1;
    while let Some(x) = w16(img, p) {
        w += 1;
        if x == end {
            return (w, m, Some(p));
        }
        if x == vol {
            m += 1;
        }
        p += 2;
    }
    (w, m, None)
}

/// The state of the player around the driver.
struct Bds {
    img: u32,
    hl: u32,
    sites: Sites,
    stack: u32,
    play: u32,
    irqr: u32,
    init: u32,
    title: String,
    subs: u16,
    cur: u16,
    songend: bool,
    resume: Option<u64>,
    ticks: u64,
    /// The audio-interrupt routine is installed (section 4.3).
    irq_on: bool,
    /// The init routine is running (section 6.2).
    init_running: bool,
    /// The work-area pointer: A4 at the return of the init routine (section 4.3).
    wa: u32,
    // the call in progress: the cpu, the clock at count 0 and the cycle count
    cpu: CpuCore,
    s: u64,
    cnt: u64,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let words = (be32(d, 0x1C) as usize) & 0x00FF_FFFF;
    let h = d.get(0x20..0x20 + 4 * words)?;
    if h.len() < 0x28 {
        return None;
    }
    let hl = h.len() as u32;
    let (play, irqr, init) = (be32(h, 0x0C), be32(h, 0x10), be32(h, 0x14));
    if play >= hl || irqr >= hl || init >= hl {
        return None;
    }
    let block = m.alloc(hl + 8);
    let stack = m.alloc(0x1000);
    if block == 0 || stack == 0 {
        return None;
    }
    let img = block + 8;
    m.bus.write_bytes(img, h);
    relocate(d, words, &mut m.bus, img);

    // the audio-routine walk: its end word becomes a plain return
    let image: Vec<u8> = m.bus.ram[img as usize..(img + hl) as usize].to_vec();
    let (w, vols, end_at) = scan(&image, irqr as usize);
    if let Some(p) = end_at {
        m.bus.w16(img + p as u32, be16(SCAN_WORDS, 2));
    }
    let start = play as usize & !1;
    let sites = Sites::find(&image, start, (start + SEARCH).min(image.len()));

    let title = {
        let o = be32(h, 0x1C) as usize;
        h.get(o..).map_or(String::new(), |s| s.iter().take_while(|&&b| b != 0).take(80).map(|&b| b as char).collect())
    };
    let mut cpu = CpuCore::new();
    cpu.set_cpu_type(CpuType::M68000);
    let mut r = Bds {
        img,
        hl,
        sites,
        stack,
        play,
        irqr,
        init,
        title,
        subs: (be32(h, 0x18).min(0xFFFF) as u16).max(1),
        cur: 0,
        songend: false,
        resume: None,
        ticks: 0,
        irq_on: false,
        init_running: false,
        wa: 0,
        cpu,
        s: 0,
        cnt: 0,
    };
    // the init call of the first start (section 4.1)
    let i = 656 + 21 * w + 12 * vols;
    m.bus.clock = m.bus.clock.max(i);
    r.begin(&mut m.bus, i, 0);
    r.init_path(&mut m.bus, 0);
    m.bus.clock = m.bus.clock.max(r.now());
    Some(Box::new(r))
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

impl Bds {
    /// The beam line (low 8 bits) at count `cnt` of the call.
    fn line(&self, cnt: u64) -> u64 {
        ((self.s + cnt / 2) / 227 % 313) & 0xFF
    }

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

    /// A host point (section 3): the count restarts at 0 from the current clock; the odd half
    /// cycle is lost.
    fn host_point(&mut self, bus: &mut Bus) {
        let t = self.now();
        self.s = t;
        self.cnt = 0;
        bus.clock = bus.clock.max(t);
        bus.epoch = (t, 0);
    }

    fn store(&mut self, bus: &mut Bus, addr: u32, v: u16) {
        bus.write_word(addr, v);
    }

    /// Runs the routine at `entry` until it returns, with `d0` and `a0` as entry registers.
    fn run(&mut self, bus: &mut Bus, entry: u32, d0: u32, a0: u32) -> Result<(), String> {
        let ret = (self.stack + 0x800) & !3;
        let sp = ret - 4;
        bus.w32(sp, ret);
        self.cpu.dar = [0; 16];
        self.cpu.dar[0] = d0;
        self.cpu.dar[8] = a0;
        self.cpu.dar[15] = sp;
        self.cpu.set_sr(0x2000);
        self.cpu.pc = entry;
        self.cpu.invalidate_prefetch();
        let mut executed = 0u64;
        let img_end = self.img + self.hl;
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok(());
            }
            executed += 1;
            if executed > 5_000_000 {
                return Err(format!("bendaglishsid: routine did not return (PC=${pc:06x})"));
            }
            if pc >= self.img && pc < img_end && pc & 1 == 0 {
                if let Some(Some((kind, len))) = self.sites.at.get(((pc - self.img) / 2) as usize).copied() {
                    self.site(bus, kind, len);
                    continue;
                }
            }
            let ins = m68k_rc::decode::decode_model(pc, &mut |a| bus.r16(a), m68k_rc::Model::M68000);
            self.cnt += m68k_rc::cycles::cycles(&ins) as u64;
            m68k_rc::Bus::cycles(bus, self.cnt);
            match self.cpu.step(bus) {
                StepResult::Ok { .. } => {}
                other => return Err(format!("bendaglishsid: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }

    /// Performs a site's behaviour and cost (section 6.2), then continues after its words.
    fn site(&mut self, bus: &mut Bus, kind: Kind, len: u8) {
        let pc = self.cpu.pc;
        let a5 = self.cpu.dar[13];
        let c0 = self.cnt;
        let at = |me: &mut Bds, bus: &mut Bus, off: u64| {
            me.cnt = c0 + off;
            m68k_rc::Bus::cycles(bus, me.cnt);
        };
        match kind {
            Kind::SongEnd => {
                at(self, bus, 36);
                self.store(bus, a5.wrapping_add(0x96), 0x000F);
                if self.init_running {
                    at(self, bus, 77);
                } else {
                    at(self, bus, 151);
                    self.songend = true;
                    self.host_point(bus);
                    self.cnt = 76;
                    m68k_rc::Bus::cycles(bus, self.cnt);
                }
            }
            Kind::Adr(v) => {
                let v = v as usize;
                let loc = self.cpu.dar[4 + v];
                let length = self.cpu.dar[3 - v] as u16;
                let base = a5.wrapping_add(16 * v as u32);
                at(self, bus, 36);
                AddressBus::write_long(bus, base.wrapping_add(0xA0), loc);
                at(self, bus, 48);
                self.store(bus, base.wrapping_add(0xA4), length);
                at(self, bus, 128);
            }
            Kind::Wait => {
                self.advance(bus, 234);
                for _ in 0..9 {
                    self.advance(bus, 16);
                    let reference = self.line(self.cnt);
                    // bounded: the line changes within 454 cycles
                    for _ in 0..1000 {
                        self.advance(bus, 16);
                        let l = self.line(self.cnt);
                        self.advance(bus, 9);
                        if l != reference {
                            break;
                        }
                    }
                    self.advance(bus, 10);
                }
                self.advance(bus, 56);
                let v = AddressBus::read_word(bus, a5.wrapping_add(2));
                self.cpu.dar[0] = (self.cpu.dar[0] & 0xFFFF_0000) | v as u32;
                self.advance(bus, 20);
            }
        }
        self.cpu.pc = pc + 2 * len as u32;
        self.cpu.invalidate_prefetch();
    }

    /// The init path (section 4.3), from the current count, for subsong `sub`.
    fn init_path(&mut self, bus: &mut Bus, sub: u16) {
        self.advance(bus, 1160);
        self.init_running = true;
        let entry = self.img + self.init;
        let r = self.run(bus, entry, sub as u32, entry);
        self.init_running = false;
        if let Err(e) = r {
            log::error!("{e}");
            self.songend = true;
        }
        self.wa = self.cpu.dar[12];
        self.advance(bus, 120);
        for p in [66, 66, 66] {
            self.host_point(bus);
            self.advance(bus, p);
        }
        self.host_point(bus);
        self.advance(bus, 88);
        self.irq_on = true;
    }

    /// The stop routine (section 4.4), from the current count; `p` is its first cost.
    fn stop(&mut self, bus: &mut Bus, p: u64) {
        self.advance(bus, p);
        for c in [54, 54, 46] {
            self.host_point(bus);
            self.advance(bus, c);
        }
        self.host_point(bus);
        self.irq_on = false;
        self.advance(bus, 44);
        self.store(bus, 0xDF_F096, 0x000F);
        self.advance(bus, 16);
        for (i, c) in [12u64, 12, 12, 16].into_iter().enumerate() {
            self.store(bus, 0xDF_F0A8 + 16 * i as u32, 0);
            self.advance(bus, c);
        }
    }
}

impl Replayer for Bds {
    fn format(&self) -> &'static str {
        "Benn Daglish SID"
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
        if n >= self.subs {
            return;
        }
        self.songend = false;
        self.cur = n;
        // the stop routine called directly, then the init path (section 4.2)
        let e = hw.bus.clock;
        self.begin(hw.bus, e, 0);
        self.stop(hw.bus, 72);
        self.init_path(hw.bus, n);
        hw.bus.clock = hw.bus.clock.max(self.now());
        if self.ticks == 0 {
            self.resume = Some(hw.bus.clock + TICK);
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        // not installed (before the first init path ends, inside the stop routine): nothing
        // runs; a request already acknowledged by an earlier run of the round: nothing runs
        if !self.irq_on || ch > 3 || hw.bus.paula.intreq & (0x80 << ch) == 0 {
            return true;
        }
        let start = hw.bus.clock;
        self.begin(hw.bus, start, 0);
        let entry = self.img + self.irqr;
        if let Err(e) = self.run(hw.bus, entry, 0, CHIP) {
            log::error!("{e}");
            self.songend = true;
        }
        hw.bus.clock = hw.bus.clock.max(self.now());
        true
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticks += 1;
        let start = hw.bus.clock;
        self.begin(hw.bus, start, 256);
        let entry = self.img + self.play;
        let r = self.run(hw.bus, entry, 0, entry);
        match r {
            Err(e) => {
                log::error!("{e}");
                self.songend = true;
            }
            Ok(()) => {
                self.advance(hw.bus, 195);
                if hw.bus.r8(self.wa.wrapping_add(0x11)) != 0 {
                    self.advance(hw.bus, 168);
                } else {
                    // the flag byte is zero: the driver ended the song itself (section 7.2)
                    self.stop(hw.bus, 90);
                    self.advance(hw.bus, 48);
                    self.host_point(hw.bus);
                    self.songend = true;
                    self.advance(hw.bus, 34);
                    let cur = self.cur;
                    self.init_path(hw.bus, cur);
                    self.advance(hw.bus, 168);
                }
            }
        }
        hw.bus.clock = hw.bus.clock.max(self.now());
    }
}

#[cfg(test)]
#[path = "bendaglishsid_tests.rs"]
mod tests;
