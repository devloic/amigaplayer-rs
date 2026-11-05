//! Riff Raff (`riff.*`: the sound driver of PAL Developments / Virgin games, 1989), played by
//! running the module's own driver.
//!
//! A `riff` file is an AmigaDOS hunk executable that carries the game's own 68000 sound driver, its
//! song data and its samples; a header names the driver's init routine and its two tick routines
//! (see `fixround/spec/riffraff`, section 1). This replayer loads the hunk, relocates it and runs
//! those routines on the repo's 68000 core with the standard cycle table (`m68k_rc::cycles`), so
//! every Paula write falls where the driver's own instructions put it. What the Eagleplayer adds
//! around the driver is reproduced from the specification:
//! * the hardware-address fix-up of the driver's code region (section 2.4) and the sample-pointer
//!   fill with the zero buffer (2.3);
//! * ten fixed word sequences of the driver (found with the search keys of `riffraff_sitekeys.bin`,
//!   section 2.5) are replaced by host behaviour with a fixed cost: volume scaling, song-end
//!   detection, faster waits, zero-buffer pointers (section 6);
//! * the start-up timeline, the tick sequence with its two raster waits and the song end
//!   (sections 3 to 5, 7).
//!
//! The ten search keys are bytes of the modules' own driver code, taken from the original player on
//! the project owner's decision.
//!
//! The registers the driver passes at a site are, in the specification's names: the voice record
//! pointer, the voice's custom-chip base, and the scratch values X0, X1, X2.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): a module whose
//! init routine has no period-table marker after it gets its region to the end of the hunk; a header
//! pointer outside the hunk makes the file not recognised; a driver that runs more than a few
//! million instructions in one call ends the song; a site at a record or chip pointer outside memory
//! reads zero.

use super::{Hw, Open, Replayer, be16, be32};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::{AddressBus, CpuCore, CpuType, StepResult};

pub const OPEN: Open = Open { name: "Riff Raff", detect, new };

/// The ten search keys of the driver's own code (section 2.5): for each key a byte with its word
/// count, then that many big-endian words. Taken from the original player on the project owner's
/// decision.
static SITE_KEYS: &[u8] = include_bytes!("riffraff_sitekeys.bin");

/// The Eagleplayer's chip-memory buffer of zeros (section 2.2).
const ZERO_BUF: u32 = 0x006A_C650;
/// Colour clocks between two ticks (PAL vertical blank).
const TICK: u64 = 70940;
/// The first two entries of the driver's period table: where the patch region ends.
const MARKER: u32 = 0x3580_3280;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Readback,
    Wait,
    Volume,
    Triple,
    MinVolume,
    Restart,
    ClearAll,
    ClearVoice,
    ClearRecord,
}

/// The kind of each key, in the order of the keys.
const KINDS: [Kind; 10] = [
    Kind::Readback,
    Kind::Wait,
    Kind::Wait,
    Kind::Volume,
    Kind::Triple,
    Kind::MinVolume,
    Kind::Restart,
    Kind::ClearAll,
    Kind::ClearVoice,
    Kind::ClearRecord,
];

fn keys() -> Vec<Vec<u16>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < SITE_KEYS.len() {
        let n = SITE_KEYS[p] as usize;
        let words = (0..n).map(|i| be16(SITE_KEYS, p + 1 + 2 * i)).collect();
        out.push(words);
        p += 1 + 2 * n;
    }
    out
}

fn detect(d: &[u8]) -> bool {
    d.len() > 0x60
        && be32(d, 0) == 0x3F3
        && d[0x14] != 0
        && be32(d, 0x20) == 0x70FF_4E75
        && d.get(0x24..0x2C) == Some(b"RIFFRAFF")
        && (0x2C..0x38).step_by(4).all(|o| be32(d, o) != 0)
}

fn w16(h: &[u8], o: usize) -> Option<u16> {
    h.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn w32(h: &[u8], o: usize) -> Option<u32> {
    h.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// The patch region (section 2.4): from the init routine to the marker long, in words.
fn region(h: &[u8], init: usize) -> (usize, usize) {
    let mut p = init & !1;
    while p + 4 <= h.len() {
        if w32(h, p) == Some(MARKER) {
            return (init & !1, p);
        }
        p += 2;
    }
    (init & !1, h.len() & !1)
}

/// Where the sites are: for each word of the hunk, the kind and length in words of the site that
/// starts there; words of a site after its first are marked as used.
struct Sites {
    at: Vec<Option<(Kind, u8)>>,
}

impl Sites {
    fn find(img: &[u8], start: usize, end: usize) -> Sites {
        let mut at = vec![None; img.len() / 2 + 1];
        let mut used = vec![false; img.len() / 2 + 1];
        for (key, kind) in keys().iter().zip(KINDS) {
            let n = key.len();
            if n == 0 {
                continue;
            }
            let mut p = start;
            while p + 2 * n <= end {
                let hit = (0..n).all(|i| w16(img, p + 2 * i) == Some(key[i])) && (0..n).all(|i| !used[p / 2 + i]);
                if hit {
                    at[p / 2] = Some((kind, n as u8));
                    (0..n).for_each(|i| used[p / 2 + i] = true);
                    p += 2 * n;
                } else {
                    p += 2;
                }
            }
        }
        Sites { at }
    }
}

/// Costs of a site by the voice of its chip pointer (voices 0 to 3), section 6.4.
const VOL_TOTAL: [u64; 4] = [373, 427, 481, 479];
const VOL_STORE: [u64; 4] = [232, 255, 278, 268];
const MIN_TOTAL: [u64; 4] = [402, 456, 510, 508];
const MIN_STORE: [u64; 4] = [241, 264, 287, 277];
const TRIPLE_TOTAL: [u64; 4] = [281, 312, 343, 351];
const TRIPLE_STORE: [u64; 3] = [48, 68, 88];
const RESTART_TOTAL: [u64; 4] = [190, 217, 240, 253];

/// The state of the player around the driver.
struct Riff {
    /// The hunk as in the file, before relocation.
    h: Vec<u8>,
    img: u32,
    sites: Sites,
    stack: u32,
    /// Raw shadow volumes of the four voices (section 6.3).
    shadow: [u16; 4],
    /// Scales of the four voices: the host's volume and balance at the defaults.
    scale: [u32; 4],
    /// Voices not yet restarted (section 7).
    flags: [bool; 4],
    track3: bool,
    mask_ptr: Option<u32>,
    title: String,
    subs: u16,
    cur: u16,
    songend: bool,
    resume: Option<u64>,
    ticks: u64,
    // the call in progress: the cpu, the clock at count 0 and the cycle count
    cpu: CpuCore,
    s: u64,
    cnt: u64,
    ended: bool,
}

/// Counts of the load step (section 2.7): words of the region, fixed words, words searched,
/// and the sample-table term.
struct Load {
    w: u64,
    d: u64,
    k: u64,
    t: i64,
}

impl Load {
    fn cycles(&self) -> u64 {
        let f = 50 * self.w.saturating_sub(self.d) + 26 * self.d + 29;
        ((932 + f + 21 * self.k) as i64 + self.t).max(0) as u64
    }
}

fn voice_of(chip: u32) -> usize {
    match chip & 0xFF_FFFF {
        0xDF_F000 => 0,
        0xDF_F010 => 1,
        0xDF_F020 => 2,
        _ => 3,
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let words = (be32(d, 0x1C) as usize) & 0x00FF_FFFF;
    let h = d.get(0x20..0x20 + 4 * words)?.to_vec();
    if h.len() < 0x34 {
        return None;
    }
    let hl = h.len() as u32;
    let routine_b = be32(&h, 0x0C);
    let init = be32(&h, 0x10);
    let tick_a = be32(&h, 0x18);
    if routine_b >= hl || init >= hl || tick_a >= hl {
        return None;
    }
    let img = m.alloc(h.len() as u32 + 8);
    let stack = m.alloc(0x1000);
    if img == 0 || stack == 0 {
        return None;
    }
    m.bus.write_bytes(img, &h);
    relocate(d, words, &mut m.bus, img);
    m.bus.write_bytes(ZERO_BUF, &[0; 4]);

    // the sample pointer fill (section 2.3)
    let tab = be32(&h, 0x28) as usize;
    let tab_end = be32(&h, 0x2C) as usize;
    // (a table longer than the hunk is cut: bounded loops)
    let n = (tab_end.saturating_sub(tab) / 4).min(h.len() / 4);
    let mut load = Load { w: 0, d: 0, k: 0, t: 0 };
    if n == 0 {
        load.t = -37;
    } else if tab + 4 <= h.len() {
        let first = m.bus.r32(img + tab as u32);
        if first != 0 {
            let j = (1..n).find(|&i| m.bus.r32(img + (tab + 4 * i) as u32) == first);
            load.t = match j {
                Some(j) => 16 + 46 * j as i64 + 10,
                None => 16 + 46 * (n as i64 - 1) + 15,
            };
        } else {
            m.bus.w32(img + tab as u32, ZERO_BUF);
            let second = tab_end + 2 * n;
            let mut z = 0;
            for i in 0..n {
                let a = img + (second + 4 * i) as u32;
                if second + 4 * i + 4 <= h.len() && m.bus.r32(a) == 0 {
                    m.bus.w32(a, ZERO_BUF);
                    z += 1;
                }
            }
            load.t = 68 + 44 * n as i64 + 12 * z;
        }
    }

    // the hardware address fix-up of the region (section 2.4)
    let (start, end) = region(&h, init as usize);
    load.w = ((end - start) / 2) as u64;
    let mut p = start;
    while p + 2 <= end {
        let a = img + p as u32;
        if m.bus.r16(a) == 0x00DF && p + 4 <= end {
            let v = m.bus.r16(a + 2) | 0xF000;
            m.bus.w16(a + 2, v);
            load.d += 1;
            p += 4;
        } else {
            p += 2;
        }
    }
    // the sites are searched in the fixed-up image
    let fixed: Vec<u8> = (0..h.len()).map(|i| m.bus.ram[img as usize + i]).collect();
    let sites = Sites::find(&fixed, start, end);

    // the pointer to the driver's mask of voices in use (section 2.6)
    let mut mask_ptr = None;
    let mut q = routine_b as usize & !1;
    let mut k = 0;
    while q + 2 <= h.len() {
        k += 1;
        if be16(&h, q) == 0x33F9 {
            mask_ptr = w32(&h, q + 2).map(|o| img + o);
            break;
        }
        q += 2;
    }
    load.k = k;
    let c_load = load.cycles();

    let title = {
        let o = be32(&h, 0x1C) as usize;
        h.get(o..).map_or(String::new(), |s| s.iter().take_while(|&&b| b != 0).take(80).map(|&b| b as char).collect())
    };
    let mut cpu = CpuCore::new();
    cpu.set_cpu_type(CpuType::M68000);
    let mut r = Riff {
        subs: (be32(&h, 0x14) as u16).max(1),
        h,
        img,
        sites,
        stack,
        shadow: [0; 4],
        scale: [64; 4],
        flags: [true; 4],
        track3: true,
        mask_ptr,
        title,
        cur: 0,
        songend: false,
        resume: None,
        ticks: 0,
        cpu,
        s: 0,
        cnt: 0,
        ended: false,
    };
    // the host's first volume refresh, then the init call (section 4.1)
    let v0 = 476 + c_load / 2;
    m.bus.clock = m.bus.clock.max(v0);
    let t = m.bus.clock;
    r.start_song(&mut m.bus, 0, t);
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

impl Riff {
    /// The beam line (low 8 bits) at count `cnt` of the call.
    fn line(&self, cnt: u64) -> u64 {
        ((self.s + cnt / 2) / 227 % 313) & 0xFF
    }

    fn advance(&mut self, bus: &mut Bus, cycles: u64) {
        self.cnt += cycles;
        m68k_rc::Bus::cycles(bus, self.cnt);
    }

    /// The raster wait of the driver's waits and of a tick (section 6.2).
    fn line_wait(&mut self, bus: &mut Bus, p: u64, q: u64) {
        self.advance(bus, p);
        for _ in 0..9 {
            self.advance(bus, 16);
            let reference = self.line(self.cnt);
            // bounded: the line changes within 454 cycles
            for _ in 0..1000 {
                self.advance(bus, 21);
                let l = self.line(self.cnt);
                self.advance(bus, 4);
                if l != reference {
                    break;
                }
            }
        }
        self.advance(bus, 15);
        self.advance(bus, q);
    }

    /// Starts a call: the clock `start` at count 0, `pre` cycles already spent.
    fn begin(&mut self, bus: &mut Bus, start: u64, pre: u64) {
        self.s = start;
        self.cnt = pre;
        bus.clock = bus.clock.max(start);
        bus.epoch = (start, 0);
        self.ended = false;
    }

    /// The clock at the current count.
    fn now(&self) -> u64 {
        self.s + self.cnt / 2
    }

    /// Runs the routine at `entry` until it returns; `d0` is the register X0 on entry.
    fn run(&mut self, bus: &mut Bus, entry: u32, d0: u32) -> Result<(), String> {
        let ret = (self.stack + 0x800) & !3;
        let sp = ret - 4;
        bus.w32(sp, ret);
        let d2 = self.cpu.dar[2];
        self.cpu.dar = [0; 16];
        self.cpu.dar[0] = d0;
        self.cpu.dar[2] = d2;
        self.cpu.dar[15] = sp;
        self.cpu.set_sr(0x2000);
        self.cpu.pc = entry;
        self.cpu.invalidate_prefetch();
        let mut executed = 0u64;
        let img_end = self.img + self.h.len() as u32;
        loop {
            let pc = self.cpu.pc;
            if pc == ret && self.cpu.dar[15] == ret {
                return Ok(());
            }
            executed += 1;
            if executed > 5_000_000 {
                return Err(format!("riffraff: routine did not return (PC=${pc:06x})"));
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
                other => return Err(format!("riffraff: {other:?} at PC=${:06x}", self.cpu.ppc)),
            }
        }
    }

    /// A scaled volume write for voice `v` at the chip base `chip`.
    fn volume_write(&mut self, bus: &mut Bus, v: usize, chip: u32, x: u16) {
        let raw = x & 0x7F;
        self.shadow[v] = raw;
        let out = (raw as u32 * self.scale[v]) >> 6;
        m68k::AddressBus::write_word(bus, chip.wrapping_add(0xA8), out as u16);
    }

    /// Performs a site's behaviour and cost (section 6.4), then continues after its words.
    fn site(&mut self, bus: &mut Bus, kind: Kind, len: u8) {
        let pc = self.cpu.pc;
        let rec = self.cpu.dar[8];
        let chip = self.cpu.dar[9];
        let v = voice_of(chip);
        let c0 = self.cnt;
        let store_at = |me: &mut Riff, bus: &mut Bus, off: u64| {
            me.cnt = c0 + off;
            m68k_rc::Bus::cycles(bus, me.cnt);
        };
        let mut total = 0;
        match kind {
            Kind::Readback => {
                self.cpu.dar[0] &= 0xFFFF_0000;
                total = 44;
            }
            Kind::Wait => {
                self.line_wait(bus, 66, 64);
            }
            Kind::Volume => {
                let x = bus.r16(rec);
                store_at(self, bus, VOL_STORE[v]);
                self.volume_write(bus, v, chip, x);
                total = VOL_TOTAL[v];
            }
            Kind::MinVolume => {
                let (x1, x2) = (self.cpu.dar[1] as u16, self.cpu.dar[2] as u16);
                let (x, extra) = if x2 < x1 { (x2, 10) } else { (x1, 0) };
                store_at(self, bus, MIN_STORE[v] + extra);
                self.volume_write(bus, v, chip, x);
                total = MIN_TOTAL[v] + extra;
            }
            Kind::Triple => {
                let loc = bus.r32(rec.wrapping_add(2));
                let length = bus.r16(rec.wrapping_add(6));
                let period = bus.r16(rec.wrapping_add(8));
                store_at(self, bus, TRIPLE_STORE[0]);
                m68k::AddressBus::write_long(bus, chip.wrapping_add(0xA0), loc);
                store_at(self, bus, TRIPLE_STORE[1]);
                m68k::AddressBus::write_word(bus, chip.wrapping_add(0xA4), length);
                store_at(self, bus, TRIPLE_STORE[2]);
                m68k::AddressBus::write_word(bus, chip.wrapping_add(0xA6), period);
                total = TRIPLE_TOTAL[v];
            }
            Kind::Restart => {
                let src = bus.r32(rec.wrapping_add(0x2A));
                bus.w32(rec.wrapping_add(0x1C), src);
                total = RESTART_TOTAL[v];
                let exact = matches!(chip & 0xFF_FFFF, 0xDF_F000 | 0xDF_F010 | 0xDF_F020 | 0xDF_F030);
                if exact {
                    self.flags[v] = false;
                }
                if exact && !self.flags.iter().any(|&f| f) {
                    // the song end: the flags are set again, the report falls 60 cycles before the
                    // site's end and the clock is re-based there (section 3; the half cycle is lost)
                    self.flags = [true, true, true, self.track3];
                    total += 88;
                    let at = (c0 + total).saturating_sub(60);
                    let t = self.s + at / 2;
                    self.s = t;
                    self.cnt = 60;
                    bus.epoch = (t, 0);
                    bus.clock = bus.clock.max(t);
                    self.ended = true;
                    self.cpu.pc = pc + 2 * len as u32;
                    self.cpu.invalidate_prefetch();
                    return;
                }
            }
            Kind::ClearAll => {
                for (i, off) in [48u64, 76, 104, 132].into_iter().enumerate() {
                    store_at(self, bus, off);
                    m68k::AddressBus::write_long(bus, 0xDF_F0A0 + 16 * i as u32, ZERO_BUF);
                }
                total = 184;
            }
            Kind::ClearVoice => {
                store_at(self, bus, 44);
                m68k::AddressBus::write_long(bus, chip.wrapping_add(0xA0), ZERO_BUF);
                store_at(self, bus, 60);
                m68k::AddressBus::write_word(bus, chip.wrapping_add(0xA4), 1);
                total = 84;
            }
            Kind::ClearRecord => {
                bus.w16(rec.wrapping_add(0x32), 0);
                bus.w32(rec.wrapping_add(2), ZERO_BUF);
                total = 80;
            }
        }
        if kind != Kind::Wait {
            self.cnt = c0 + total;
            m68k_rc::Bus::cycles(bus, self.cnt);
        }
        self.cpu.pc = pc + 2 * len as u32;
        self.cpu.invalidate_prefetch();
    }

    /// The host's volume refresh: the four volume registers from the shadow volumes, from `t`.
    fn refresh_volumes(&mut self, bus: &mut Bus, t: u64) {
        for (i, off) in [0u64, 132, 276, 415, 664, 796, 940, 1079].into_iter().enumerate() {
            let v = i & 3;
            bus.clock = bus.clock.max(t + off);
            let out = (self.shadow[v] as u32 * self.scale[v]) >> 6;
            m68k::AddressBus::write_word(bus, 0xDF_F0A8 + 16 * v as u32, out as u16);
        }
    }

    /// The init call for subsong `sub` from clock `i` (section 4.1 step 2); leaves the bus clock at E.
    fn init_call(&mut self, bus: &mut Bus, i: u64, sub: u16) -> Option<()> {
        self.flags = [true, true, true, true];
        self.begin(bus, i, 1140);
        let entry = self.img + be32(&self.h, 0x10);
        self.run(bus, entry, sub as u32).map_err(|e| log::error!("{e}")).ok()?;
        let three = self.mask_ptr.is_some_and(|a| bus.r16(a) & 8 == 0);
        self.track3 = !three;
        self.flags[3] = self.track3;
        self.advance(bus, if three { 139 } else { 127 });
        bus.clock = bus.clock.max(self.now());
        Some(())
    }

    /// Start at subsong `sub` with the host's volume rounds first (`v0`).
    fn start_song(&mut self, bus: &mut Bus, sub: u16, v0: u64) {
        self.refresh_volumes(bus, v0);
        if self.init_call(bus, v0 + 1108, sub).is_none() {
            self.songend = true;
        }
    }
}

impl Replayer for Riff {
    fn format(&self) -> &'static str {
        "Riff Raff"
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
        let e = hw.bus.clock;
        // the player's stop routine (section 4.2): DMA off, volume 0 on the four voices directly
        hw.bus.clock = e + 14;
        hw.bus.write_word(0xDF_F096, 0x000F);
        for v in 0..4u64 {
            hw.bus.clock = e + 22 + 8 * v;
            hw.bus.write_word(0xDF_F0A8 + 16 * v as u32, 0);
        }
        hw.bus.clock = e + 54;
        let v0 = e + 274;
        self.start_song(hw.bus, n, v0);
        if self.ticks == 0 {
            self.resume = Some(hw.bus.clock + TICK);
        }
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.resume.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.ticks += 1;
        let start = hw.bus.clock;
        let tick_a = be32(&self.h, 0x18);
        let routine_b = self.img + be32(&self.h, 0x0C);
        let r = (|| -> Result<(), String> {
            if tick_a == 0 {
                self.begin(hw.bus, start, 281);
                self.run(hw.bus, routine_b, 0)?;
            } else {
                self.begin(hw.bus, start, 269);
                self.cpu.dar[2] = 0;
                self.run(hw.bus, self.img + tick_a, 0)?;
                let dma = self.cpu.dar[2] as u16;
                self.line_wait(hw.bus, 46, 44);
                self.advance(hw.bus, 16);
                m68k::AddressBus::write_word(hw.bus, 0xDF_F096, dma);
                self.line_wait(hw.bus, 46, 44);
                self.advance(hw.bus, 32);
                self.run(hw.bus, routine_b, 0)?;
            }
            self.advance(hw.bus, 168);
            Ok(())
        })();
        hw.bus.clock = hw.bus.clock.max(self.now());
        if let Err(e) = r {
            log::error!("{e}");
            self.songend = true;
        }
        if self.ended {
            self.ended = false;
            self.songend = true;
        }
    }
}

#[cfg(test)]
#[path = "riffraff_tests.rs"]
mod tests;
