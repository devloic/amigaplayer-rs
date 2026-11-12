//! Atari ST SNDH music (`.sndh`; the `SNDH` tag at offset 12 of the unpacked file; modland
//! `SNDH/`): the music is the ST program's own 68000 code, run here on a small Atari ST machine
//! (a 68000 on the repo's `m68k` core, 4 MB of RAM, the YM2149 sound chip, the MFP 68901 timers
//! and their interrupts, the STE's DMA sound with its microwire volume control, the few BIOS
//! calls the tunes make) and rendered to PCM at the output rate through the recorded-audio
//! backend (`crate::aiff::Stream`): no Paula, no Amiga filters.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the public SNDH
//! format description (the three branches at 0, 4 and 8 for init, exit and play; the tag
//! header `SNDH`, `TITL`, `COMM`, `RIPP`, `CONV`, `YEAR`, `##nn`, `#!nn`, `!#SN`, `TIME`, `FRMS`,
//! `TA`/`TB`/`TC`/`TD`/`!V`, `FLAG`, `HDNS`; init called with the subsong number in d0, play
//! called once per player tick) and the Atari ST / STE hardware documentation (YM2149 at
//! $FF8800, MFP at $FFFA00, DMA sound at $FF8900). Its machine model (where the tune is loaded,
//! the low-memory stubs, the BIOS calls that are answered, the cookie jar, how a call ends) follows
//! AtariAudio by Arnaud Carre (MIT; see `LICENSE-AtariAudio`), read as a reference; no code is
//! copied, but the behaviours it works out are kept where the tunes need them. The chip model is
//! this repository's own (`sndh_ym.rs`, `sndh_mfp.rs`), so the audio differs from AtariAudio's
//! (which uses a table measured from a real ST); the register writes do not.
//!
//! Timing. One clock for everything: the CPU at 8 MHz (a 68000's cycle counts from the core, bus
//! accesses stamped to the clock they happen on), the YM at 2 MHz (stepped every 8 of its clocks,
//! 32 CPU clocks), the MFP at 2.4576 MHz. The play routine is called at the tune's rate (50 Hz
//! unless a `TA`/`TB`/`TC`/`TD`/`!V` tag says another) in supervisor mode, mask 3, so that the
//! MFP's level-6 interrupts (a digidrum, a SID voice, a sync-buzzer) preempt it as they do on a
//! real ST; a play call that is still running at the next tick delays that tick (one tick at most
//! is remembered, as a real vertical blank latches one).
//!
//! Not done: the blitter (`Blitmode` says there is none, and the cookie jar says an ST or STE
//! without one), the pulse-width modes of the MFP timers, the 68901's USART and GPIP lines but
//! the mono-detect bit, the ST's video registers but the two a tune might look at; a STOP
//! instruction wakes on an MFP interrupt at the next instruction boundary. A tune's `FRMS`/`TIME`
//! tag gives its length; AtariAudio's database of lengths for tunes without one is not used.
//!
//! Bounds: the CPU runs for one tick's worth of 8 MHz clocks at a time whatever the tune does,
//! so a tune that never returns costs the same as one that does; an init that has not returned
//! after one second of CPU time, an illegal instruction, an address error or a jump to the
//! vectors of unset exceptions ends the tune (it plays silence).

#[path = "sndh_mfp.rs"]
pub mod mfp;
#[path = "sndh_ym.rs"]
pub mod ym;

use m68k::{AddressBus, CpuCore, CpuType, StepResult};
use mfp::Mfp;
use std::collections::VecDeque;
use ym::Ym;

pub const MAGIC: &[u8; 4] = b"SNDH";

/// The CPU clock, Hz.
const CPU_HZ: u64 = 8_000_000;
/// The CPU clocks of one YM step (8 YM clocks at 2 MHz).
const YM_STEP: u64 = 32;
/// The YM's step rate, Hz.
const YM_STEP_HZ: u64 = 250_000;
/// The ST's RAM.
const RAM_SIZE: usize = 4 << 20;
/// Where the tune is loaded (the same as AtariAudio's: some tunes do not play lower, or at a
/// 64 KiB boundary).
const BASE: u32 = 0x10002;
/// Low-memory addresses of the stubs.
const RTE_PC: u32 = 0x500;
const IDLE_PC: u32 = 0x502;
const CRASH_PC: u32 = 0x504;
const DONE_PC: u32 = 0x506;
/// The top of the stack.
const STACK_TOP: u32 = RAM_SIZE as u32 - 0x10;
/// The area GEMDOS Malloc hands out, from the top of RAM down by 1 MB.
const MALLOC_START: u32 = RAM_SIZE as u32 - (1 << 20);
/// An init that takes longer than this (CPU clocks) did not return.
const INIT_LIMIT: u64 = CPU_HZ;
/// Instructions one stretch of CPU time may take (a tune cannot make a zero-time loop).
const STEP_LIMIT: u32 = 4_000_000;
/// The SR a call runs under: supervisor, interrupt mask 3.
const CALL_SR: u16 = 0x2300;
/// The longest tune the headers can ask (seconds).
const MAX_SUBSONGS: usize = 128;

// ------------------------------------------------------------------ the file

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub title: String,
    pub author: String,
    pub ripper: String,
    pub converter: String,
    pub year: String,
    /// The player's tick rate, Hz.
    pub rate: u32,
    /// The tick is the vertical blank (the default and `!V`): the real ST's, 160,256 clocks of its
    /// 8.021247 MHz CPU (50.05 Hz) at a rate of 50; a timer's rate is exact.
    pub vbl: bool,
    pub subsongs: u16,
    /// 1-based.
    pub default_subsong: u16,
    /// The length of each subsong in player ticks (0: not stated).
    pub ticks: Vec<u32>,
    /// The tune's own name of each subsong (`!#SN`), when it has some.
    pub names: Vec<String>,
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

/// The NUL-terminated string at `o` (Latin-1) and the offset after its NUL.
fn cstr(d: &[u8], o: usize) -> (String, usize) {
    let rest = d.get(o..).unwrap_or(&[]);
    let n = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    (rest[..n].iter().map(|&b| b as char).collect(), o + n + 1)
}

/// The leading decimal digits of the string at `o`, like C's `atoi` (0 for none).
fn atoi(d: &[u8], o: usize) -> u32 {
    let mut v: u32 = 0;
    for &b in d.get(o..).unwrap_or(&[]).iter().take_while(|b| b.is_ascii_digit()).take(9) {
        v = v * 10 + (b - b'0') as u32;
    }
    v
}

/// Where the header ends: the first of the three entry branches' targets (`bra.s`, `bra.w`).
fn header_end(d: &[u8]) -> usize {
    let mut end = d.len();
    for k in 0..3 {
        let o = 4 * k;
        let t = match (d.get(o), d.get(o + 1)) {
            (Some(0x60), Some(0)) => be16(d, o + 2).map(|v| (o as i64 + 2 + v as i16 as i64).max(0) as usize),
            (Some(0x60), Some(&n)) if n < 0x80 => Some(o + 2 + n as usize),
            _ => None,
        };
        if let Some(t) = t {
            end = end.min(t);
        }
    }
    end.max(16)
}

/// Whether `d` (unpacked) is an SNDH file.
pub fn detect(d: &[u8]) -> bool {
    d.len() > 16 && &d[12..16] == MAGIC && d[0] == 0x60
}

pub fn parse(d: &[u8]) -> Option<Info> {
    if !detect(d) {
        return None;
    }
    let end = header_end(d).min(d.len());
    let mut info = Info { rate: 50, vbl: true, subsongs: 1, default_subsong: 1, ..Info::default() };
    let mut lens: Vec<u32> = Vec::new();
    let mut frms = false;
    let mut o = 16;
    while o + 4 <= end {
        let t = &d[o..o + 4];
        if t == b"HDNS" {
            break;
        } else if t == b"#!SN" || t == b"!#SN" {
            let n = info.subsongs as usize;
            let mut names = Vec::new();
            let mut after = o + 4 + 2 * n;
            for i in 0..n {
                let off = be16(d, o + 4 + 2 * i).unwrap_or(0) as usize;
                let (s, e) = cstr(d, o + off);
                if off != 0 {
                    after = after.max(e);
                }
                names.push(s);
            }
            info.names = names;
            o = after;
        } else if &t[..2] == b"#!" {
            info.default_subsong = atoi(d, o + 2).min(u16::MAX as u32) as u16;
            o = cstr(d, o + 2).1;
        } else if t == b"TITL" {
            (info.title, o) = cstr(d, o + 4);
        } else if t == b"COMM" {
            (info.author, o) = cstr(d, o + 4);
        } else if t == b"RIPP" {
            (info.ripper, o) = cstr(d, o + 4);
        } else if t == b"CONV" {
            (info.converter, o) = cstr(d, o + 4);
        } else if t == b"YEAR" {
            (info.year, o) = cstr(d, o + 4);
        } else if t == b"FLAG" {
            o = cstr(d, o + 4).1;
        } else if &t[..2] == b"##" {
            let n = ((t[2].wrapping_sub(b'0')) as usize).wrapping_mul(10) + t[3].wrapping_sub(b'0') as usize;
            info.subsongs = if (1..=MAX_SUBSONGS).contains(&n) && t[2].is_ascii_digit() && t[3].is_ascii_digit() { n as u16 } else { 1 };
            o += 4;
        } else if t == b"TIME" {
            o += 4;
            if o & 1 == 1 {
                o += 1;
            }
            if !frms {
                lens = (0..info.subsongs as usize).map(|i| be16(d, o + 2 * i).unwrap_or(0) as u32).collect();
            }
            o += 2 * info.subsongs as usize;
        } else if t == b"FRMS" {
            o += 4;
            lens = (0..info.subsongs as usize).map(|i| be32(d, o + 4 * i).unwrap_or(0)).collect();
            o += 4 * info.subsongs as usize;
            frms = true;
        } else if matches!(&t[..2], b"TA" | b"TB" | b"TC" | b"TD" | b"!V") {
            let r = atoi(d, o + 2);
            if (1..=2000).contains(&r) {
                info.rate = r;
                info.vbl = &t[..2] == b"!V";
            }
            o = cstr(d, o + 2).1;
        } else {
            o += 1;
        }
    }
    if info.default_subsong < 1 || info.default_subsong > info.subsongs {
        info.default_subsong = 1;
    }
    // TIME counts seconds, FRMS ticks
    info.ticks = if frms { lens } else { lens.iter().map(|&s| s.saturating_mul(info.rate)).collect() };
    info.ticks.resize(info.subsongs as usize, 0);
    Some(info)
}

// ------------------------------------------------------------------ the bus

/// What the sound hardware did, with the CPU clock it happened on.
#[derive(Clone, Copy, Debug)]
enum Ev {
    Ym(u8, u8),
    /// The STE DMA sound's output sample (left, right), signed 8 bit.
    Dma(i8, i8),
}

/// The STE's DMA sound: frame start and end, counter, mode and the microwire registers.
#[derive(Clone)]
struct Dma {
    regs: [u8; 0x40],
    ptr: u32,
    end: u32,
    /// Output samples owed: CPU clocks times the rate, over CPU_HZ.
    acc: u64,
    mw_data: u16,
    mw_mask: u16,
    mw_shift: u8,
    /// Master volume 0..=64.
    volume: u32,
    /// The end-of-frame event waits for the bus owner to hand it to the MFP.
    frame_end: bool,
}

impl Dma {
    fn new() -> Dma {
        Dma { regs: [0; 0x40], ptr: 0, end: 0, acc: 0, mw_data: 0, mw_mask: 0, mw_shift: 0, volume: 64, frame_end: false }
    }

    fn fetch_pointers(&mut self) {
        let r = &self.regs;
        self.ptr = (r[3] as u32) << 16 | (r[5] as u32) << 8 | (r[7] & 0xFE) as u32;
        self.end = (r[0xF] as u32) << 16 | (r[0x11] as u32) << 8 | (r[0x13] & 0xFE) as u32;
    }

    fn playing(&self) -> bool {
        self.regs[1] & 1 != 0
    }

    fn write8(&mut self, o: usize, v: u8) {
        if o & 1 == 0 || o >= 0x40 {
            return;
        }
        match o {
            0x01 => {
                if v & 1 != 0 && self.regs[1] & 1 == 0 {
                    self.fetch_pointers();
                    self.acc = 0;
                }
            }
            0x07 | 0x0D => {
                self.regs[o] = v & 0xFE;
                return;
            }
            _ => {}
        }
        self.regs[o] = v;
    }

    fn write16(&mut self, o: usize, v: u16) {
        match o {
            0x22 => {
                self.mw_data = v;
                self.microwire();
                self.mw_shift = 16;
            }
            0x24 => self.mw_mask = v,
            _ => self.write8(o + 1, v as u8),
        }
    }

    fn read8(&self, o: usize) -> u8 {
        if o & 1 == 0 || o >= 0x40 {
            return 0xFF;
        }
        match o {
            0x09 => (self.ptr >> 16) as u8,
            0x0B => (self.ptr >> 8) as u8,
            0x0D => self.ptr as u8,
            _ => self.regs[o],
        }
    }

    fn read16(&mut self, o: usize) -> u16 {
        match o {
            0x22 => self.mw_data,
            0x24 => {
                if self.mw_shift > 0 {
                    self.mw_mask = self.mw_mask.rotate_left(1);
                    self.mw_shift -= 1;
                }
                self.mw_mask
            }
            _ => 0xFF00 | self.read8(o + 1) as u16,
        }
    }

    /// The microwire interface: the data bits the mask selects are sent, an 11-bit command
    /// (2-bit address, 3-bit command, 6-bit data); master volume is command 3 (0..40, 2 dB steps).
    fn microwire(&mut self) {
        let (mut v, mut n) = (0u16, 0);
        for i in 0..16 {
            if self.mw_mask >> i & 1 != 0 {
                if self.mw_data >> i & 1 != 0 {
                    v |= 1 << n;
                }
                n += 1;
            }
        }
        if n == 11 && v >> 9 == 2 && (v >> 6) & 7 == 3 {
            let d = (v & 0x3F) as u32;
            self.volume = if d > 40 { 64 } else { d * 64 / 40 };
        }
    }

    /// Lets `cycles` CPU clocks pass, with the output samples the DMA makes.
    fn advance(&mut self, cycles: u64, t: u64, out: &mut VecDeque<(u64, Ev)>, ram: &[u8]) {
        if !self.playing() {
            return;
        }
        let rate = [6258u64, 12517, 25033, 50066][(self.regs[0x21] & 3) as usize];
        let stereo = self.regs[0x21] & 0x80 == 0;
        self.acc += cycles * rate;
        while self.acc >= CPU_HZ {
            self.acc -= CPU_HZ;
            if self.ptr == self.end {
                self.frame_end = true;
                self.fetch_pointers();
                if self.regs[1] & 2 == 0 {
                    self.regs[1] &= 0xFE;
                    out.push_back((t, Ev::Dma(0, 0)));
                    return;
                }
            }
            let get = |a: u32| ram.get((a & 0xFF_FFFF) as usize).map_or(0, |&b| b as i8);
            let (l, r) = if stereo { (get(self.ptr), get(self.ptr + 1)) } else { (get(self.ptr), get(self.ptr)) };
            self.ptr = self.ptr.wrapping_add(if stereo { 2 } else { 1 }) & 0xFF_FFFF;
            out.push_back((t, Ev::Dma(l, r)));
        }
    }
}

/// The ST's memory and I/O as the CPU sees them.
struct StBus {
    ram: Vec<u8>,
    /// The YM register selected, and the registers as the CPU reads them back.
    ym_sel: u8,
    ym_regs: [u8; 16],
    mfp: Mfp,
    dma: Dma,
    /// Where the sound hardware's events go: (CPU clock, event), in time order.
    events: VecDeque<(u64, Ev)>,
    /// The clock at the start of the instruction running and the offset of the access now.
    t: u64,
    off: u64,
}

impl StBus {
    fn new() -> StBus {
        StBus {
            ram: vec![0; RAM_SIZE],
            ym_sel: 0,
            ym_regs: [0; 16],
            mfp: Mfp::new(CPU_HZ),
            dma: Dma::new(),
            events: VecDeque::new(),
            t: 0,
            off: 0,
        }
    }

    fn now(&self) -> u64 {
        self.t + self.off
    }

    fn ym_write(&mut self, port: u32, v: u8) {
        if port & 2 != 0 {
            let r = self.ym_sel;
            if r < 14 {
                const MASK: [u8; 14] = [0xFF, 0x0F, 0xFF, 0x0F, 0xFF, 0x0F, 0x1F, 0xFF, 0x1F, 0x1F, 0x1F, 0xFF, 0xFF, 0x0F];
                self.ym_regs[r as usize] = v & MASK[r as usize];
                let t = self.now();
                self.events.push_back((t, Ev::Ym(r, v)));
            } else if r < 16 {
                self.ym_regs[r as usize] = v;
            }
        } else {
            self.ym_sel = v;
        }
    }

    fn ym_read(&self, port: u32) -> u8 {
        if port & 2 == 0 { self.ym_regs[(self.ym_sel & 15) as usize] } else { 0xFF }
    }

    fn io_read8(&mut self, a: u32) -> u8 {
        match a {
            0xFF8800..=0xFF88FF => self.ym_read(a & 0xFF),
            0xFF8260 => 0,
            0xFF820A => 2,
            0xFFFA00..=0xFFFA25 => {
                let o = (a - 0xFFFA00) as usize;
                if o == 1 { (self.mfp.read(o) & 0x7F) | 0x80 } else { self.mfp.read(o) }
            }
            0xFF8900..=0xFF8925 => self.dma.read8((a - 0xFF8900) as usize),
            0xFFFC00..=0xFFFC06 => 0,
            _ => 0xFF,
        }
    }

    fn io_write8(&mut self, a: u32, v: u8) {
        match a {
            0xFF8800..=0xFF88FF => self.ym_write(a & 0xFE, v),
            0xFFFA00..=0xFFFA25 => self.mfp.write((a - 0xFFFA00) as usize, v),
            0xFF8900..=0xFF8925 => self.dma.write8((a - 0xFF8900) as usize, v),
            _ => {}
        }
    }

    fn io_read16(&mut self, a: u32) -> u16 {
        match a {
            0xFF8800..=0xFF88FF => (self.ym_read(a & 0xFE) as u16) << 8,
            0xFFFA00..=0xFFFA25 => 0xFF00 | self.mfp.read((a - 0xFFFA00 + 1) as usize) as u16,
            0xFF8900..=0xFF8925 => self.dma.read16((a - 0xFF8900) as usize),
            0xFFFC00..=0xFFFC06 => 0,
            _ => 0xFFFF,
        }
    }

    fn io_write16(&mut self, a: u32, v: u16) {
        match a {
            0xFF8800..=0xFF88FF => self.ym_write(a & 0xFE, (v >> 8) as u8),
            0xFFFA00..=0xFFFA25 => self.mfp.write((a - 0xFFFA00 + 1) as usize, v as u8),
            0xFF8900..=0xFF8925 => self.dma.write16((a - 0xFF8900) as usize, v),
            _ => {}
        }
    }

    fn r8(&self, a: u32) -> u8 {
        self.ram.get(a as usize).copied().unwrap_or(0)
    }

    fn r16(&self, a: u32) -> u16 {
        (self.r8(a) as u16) << 8 | self.r8(a.wrapping_add(1)) as u16
    }

    fn r32(&self, a: u32) -> u32 {
        (self.r16(a) as u32) << 16 | self.r16(a.wrapping_add(2)) as u32
    }

    fn w8(&mut self, a: u32, v: u8) {
        if let Some(b) = self.ram.get_mut(a as usize) {
            *b = v;
        }
    }

    fn w16(&mut self, a: u32, v: u16) {
        self.w8(a, (v >> 8) as u8);
        self.w8(a.wrapping_add(1), v as u8);
    }

    fn w32(&mut self, a: u32, v: u32) {
        self.w16(a, (v >> 16) as u16);
        self.w16(a.wrapping_add(2), v as u16);
    }
}

impl AddressBus for StBus {
    fn read_byte(&mut self, a: u32) -> u8 {
        let a = a & 0xFF_FFFF;
        let v = if (a as usize) < RAM_SIZE { self.ram[a as usize] } else { self.io_read8(a) };
        self.off += 4;
        v
    }

    fn read_word(&mut self, a: u32) -> u16 {
        let a = a & 0xFF_FFFE;
        let v = if (a as usize) < RAM_SIZE - 1 { self.r16(a) } else { self.io_read16(a) };
        self.off += 4;
        v
    }

    fn read_long(&mut self, a: u32) -> u32 {
        let a = a & 0xFF_FFFE;
        let hi = if (a as usize) < RAM_SIZE - 1 { self.r16(a) } else { self.io_read16(a) } as u32;
        self.off += 4;
        let b = (a + 2) & 0xFF_FFFE;
        let lo = if (b as usize) < RAM_SIZE - 1 { self.r16(b) } else { self.io_read16(b) } as u32;
        self.off += 4;
        hi << 16 | lo
    }

    fn write_byte(&mut self, a: u32, v: u8) {
        let a = a & 0xFF_FFFF;
        if (a as usize) < RAM_SIZE {
            self.ram[a as usize] = v;
        } else {
            self.io_write8(a, v);
        }
        self.off += 4;
    }

    fn write_word(&mut self, a: u32, v: u16) {
        let a = a & 0xFF_FFFE;
        if (a as usize) < RAM_SIZE - 1 {
            self.w16(a, v);
        } else {
            self.io_write16(a, v);
        }
        self.off += 4;
    }

    fn write_long(&mut self, a: u32, v: u32) {
        let a = a & 0xFF_FFFE;
        let (hi, lo) = ((v >> 16) as u16, v as u16);
        if (a as usize) < RAM_SIZE - 1 {
            self.w16(a, hi);
        } else {
            self.io_write16(a, hi);
        }
        self.off += 4;
        let b = (a + 2) & 0xFF_FFFE;
        if (b as usize) < RAM_SIZE - 1 {
            self.w16(b, lo);
        } else {
            self.io_write16(b, lo);
        }
        self.off += 4;
    }

    fn sync(&mut self, cpu_clocks: u32) {
        self.off += cpu_clocks as u64;
    }

    fn interrupt_acknowledge(&mut self, _level: u8) -> u32 {
        self.mfp.acknowledge()
    }
}

// ------------------------------------------------------------------ the machine

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Running the init routine, which must return within the clock `.0`.
    Init(u64),
    /// Nothing runs: the CPU waits for an interrupt or the next play tick.
    Idle,
    /// A play call (or a handler of an interrupt that came in it) is running.
    Play,
    /// The tune has crashed: it plays silence.
    Dead,
}

struct Machine {
    cpu: CpuCore,
    bus: StBus,
    mode: Mode,
    /// The clock (CPU, from the subsong's start).
    t: u64,
    /// The next play tick's number: its clock is `tick * CPU_HZ / rate`.
    tick: u64,
    rate: u64,
    vbl: bool,
    /// The clock at which the init returned: the audio, and the play ticks, count from it (the init
    /// takes no time of the tune's, as in the reference player).
    origin: u64,
    malloc_next: u32,
    /// XBIOS DoSound: the script's address, the value register and the wait.
    dosnd_ptr: u32,
    dosnd_val: u8,
    dosnd_delay: u8,
}

impl Machine {
    fn new(data: &[u8], rate: u32, vbl: bool, subsong: u16) -> Machine {
        let mut bus = StBus::new();
        let n = data.len().min(RAM_SIZE - BASE as usize - 0x1000);
        bus.ram[BASE as usize..BASE as usize + n].copy_from_slice(&data[..n]);
        // the vectors: the MFP's (0x100-0x13C) and the division by zero go to an RTE, the rest of
        // the exceptions end the tune
        for v in 0..256u32 {
            bus.w32(4 * v, CRASH_PC);
        }
        for v in 0x40..0x50u32 {
            bus.w32(4 * v, RTE_PC);
        }
        bus.w32(0x14, RTE_PC);
        bus.w16(RTE_PC, 0x4E73);
        bus.w16(IDLE_PC, 0x60FE);
        // the cookie jar (an ST: the sound chip only; no blitter, no DMA sound for a tune that asks)
        bus.w32(0x900, 0x5F53_4E44);
        bus.w32(0x904, 0x3);
        bus.w32(0x908, 0x5F4D_4348);
        bus.w32(0x90C, 0x0001_0000);
        bus.w32(0x910, 0);
        bus.w32(0x5A0, 0x900);
        // the ST's start: the YM's mixer closed
        bus.ym_regs[7] = 0x3F;
        bus.events.push_back((0, Ev::Ym(7, 0x3F)));
        let mut cpu = CpuCore::new();
        cpu.set_cpu_type(CpuType::M68000);
        let mut m = Machine {
            cpu,
            bus,
            mode: Mode::Idle,
            t: 0,
            tick: 0,
            rate: rate as u64,
            vbl,
            origin: 0,
            malloc_next: MALLOC_START,
            dosnd_ptr: 0,
            dosnd_val: 0,
            dosnd_delay: 0,
        };
        m.start_call(BASE, subsong as u32);
        m.mode = Mode::Init(INIT_LIMIT);
        m
    }

    /// Begins a call of the routine at `pc`: d0 as given, the return address a stub that ends it.
    fn start_call(&mut self, pc: u32, d0: u32) {
        let c = &mut self.cpu;
        c.set_sr(CALL_SR);
        c.dar[0] = d0;
        c.dar[15] = STACK_TOP - 4;
        self.bus.w32(STACK_TOP - 4, DONE_PC);
        c.pc = pc;
        c.stopped = 0;
        c.invalidate_prefetch();
        self.mode = Mode::Play;
    }

    fn go_idle(&mut self) {
        let c = &mut self.cpu;
        c.set_sr(CALL_SR);
        c.dar[15] = STACK_TOP - 8;
        c.pc = IDLE_PC;
        c.stopped = 0;
        c.invalidate_prefetch();
        self.mode = Mode::Idle;
    }

    /// Nothing runs: the CPU sits on the idle stub with no handler running.
    fn at_idle(&self) -> bool {
        self.mode == Mode::Idle && self.cpu.pc == IDLE_PC && self.cpu.dar[15] == STACK_TOP - 8
    }

    fn die(&mut self) {
        self.mode = Mode::Dead;
    }

    /// The clock of play tick `n`.
    fn tick_clock(&self, n: u64) -> u64 {
        let ticks = if self.vbl {
            // 160,256 clocks of the ST's 8.021247 MHz CPU make a vertical blank at a rate of 50
            n as u128 * 160_256 * 50 * CPU_HZ as u128 / (8_021_247 * self.rate as u128)
        } else {
            n as u128 * CPU_HZ as u128 / self.rate as u128
        };
        self.origin + ticks as u64
    }

    /// Runs the init to its return (or its death).
    fn finish_init(&mut self) {
        while matches!(self.mode, Mode::Init(_)) {
            self.run_to(self.t + CPU_HZ / 50);
        }
    }

    /// The CPU clocks pass: the clock, the MFP, the DMA.
    fn pass(&mut self, cycles: u64) {
        self.t += cycles;
        self.bus.mfp.advance(cycles);
        let t = self.t;
        let b = &mut self.bus;
        if b.dma.playing() {
            b.dma.advance(cycles, t, &mut b.events, &b.ram);
            if std::mem::take(&mut b.dma.frame_end) {
                b.mfp.event_a();
            }
        }
    }

    /// XBIOS DoSound's script, one step per play tick.
    fn dosound_tick(&mut self) {
        if self.dosnd_ptr == 0 {
            return;
        }
        if self.dosnd_delay > 0 {
            self.dosnd_delay -= 1;
            return;
        }
        for _ in 0..4096 {
            let p = self.dosnd_ptr;
            let cmd = self.bus.r8(p);
            if cmd < 0x80 {
                self.bus.ym_write(0, cmd & 15);
                let v = self.bus.r8(p + 1);
                self.bus.ym_write(2, v);
                self.dosnd_ptr = p + 2;
            } else if cmd == 0x80 {
                self.dosnd_val = self.bus.r8(p + 1);
                self.dosnd_ptr = p + 2;
            } else if cmd == 0x81 {
                let r = self.bus.r8(p + 1) & 15;
                self.bus.ym_write(0, r);
                self.dosnd_val = self.dosnd_val.wrapping_add(self.bus.r8(p + 2));
                let v = self.dosnd_val;
                self.bus.ym_write(2, v);
                if self.dosnd_val == self.bus.r8(p + 3) {
                    self.dosnd_ptr = p + 4;
                }
                break;
            } else {
                self.dosnd_delay = self.bus.r8(p + 1);
                if self.dosnd_delay == 0 {
                    self.dosnd_ptr = 0;
                }
                break;
            }
        }
    }

    /// The BIOS calls the tunes make (TRAP #1 GEMDOS, TRAP #14 XBIOS); the arguments are on the
    /// stack the TRAP was made with. Returns whether it was one of them.
    fn trap(&mut self, n: u8) -> bool {
        let sp = self.cpu.dar[15];
        let func = self.bus.r16(sp);
        match n {
            1 => match func {
                0x48 => {
                    let size = self.bus.r32(sp + 2);
                    if size == 0xFFFF_FFFF {
                        self.cpu.dar[0] = RAM_SIZE as u32 - self.malloc_next;
                    } else {
                        self.cpu.dar[0] = self.malloc_next;
                        self.malloc_next = (self.malloc_next.saturating_add(size).saturating_add(1)) & !1;
                        if self.malloc_next > STACK_TOP - 0x8000 {
                            self.malloc_next = STACK_TOP - 0x8000;
                            self.cpu.dar[0] = 0;
                        }
                    }
                }
                0x30 => self.cpu.dar[0] = 0,
                _ => self.cpu.dar[0] = 0,
            },
            14 => match func {
                31 => {
                    let timer = self.bus.r16(sp + 2);
                    let ctrl = self.bus.r16(sp + 4) as u8;
                    let data = self.bus.r16(sp + 6) as u8;
                    let vec = self.bus.r32(sp + 8);
                    if timer < 4 {
                        const IVEC: [u32; 4] = [0x134, 0x120, 0x114, 0x110];
                        self.bus.w32(IVEC[timer as usize], vec);
                        // (control port, data port, enable port, enable bit, control mask, control value)
                        let (cp, dp, ep, bit, mask, cv) = match timer {
                            0 => (0x19, 0x1F, 0x07, 5, 0x00, ctrl),
                            1 => (0x1B, 0x21, 0x07, 0, 0x00, ctrl),
                            2 => (0x1D, 0x23, 0x09, 5, 0x0F, (ctrl & 0xF) << 4),
                            _ => (0x1D, 0x25, 0x09, 4, 0xF0, ctrl & 0xF),
                        };
                        let m = &mut self.bus.mfp;
                        let back = m.read(cp) & mask;
                        m.write(cp, back);
                        m.write(dp, data);
                        m.write(cp, back | cv);
                        let e = m.read(ep) | 1 << bit;
                        m.write(ep, e);
                        let k = m.read(ep + 12) | 1 << bit;
                        m.write(ep + 12, k);
                    }
                }
                32 => self.dosnd_ptr = self.bus.r32(sp + 2),
                38 => {
                    // Supexec: call the routine, returning after the TRAP
                    let ret = self.cpu.pc;
                    let sp = sp.wrapping_sub(4);
                    self.bus.w32(sp, ret);
                    self.cpu.dar[15] = sp;
                    self.cpu.pc = self.bus.r32(sp.wrapping_add(6));
                    self.cpu.invalidate_prefetch();
                }
                // Blitmode: no blitter
                64 => self.cpu.dar[0] = 0,
                _ => self.cpu.dar[0] = 0,
            },
            _ => return false,
        }
        true
    }

    /// Runs one instruction (or waits, when stopped); advances the clocks.
    fn step(&mut self) {
        let irq = self.bus.mfp.irq();
        self.cpu.set_irq(if irq { 6 } else { 0 });
        self.bus.t = self.t;
        self.bus.off = 0;
        let cycles = match self.cpu.step(&mut self.bus) {
            StepResult::Ok { cycles } => cycles.max(4) as u64,
            StepResult::TrapInstruction { trap_num } => {
                if !self.trap(trap_num) {
                    let c = self.cpu.take_trap_exception(&mut self.bus, trap_num);
                    c.max(34) as u64
                } else {
                    34
                }
            }
            StepResult::Stopped => {
                if self.cpu.is_halted() {
                    self.die();
                    return;
                }
                // STOP: wake on the interrupt, else let the time pass to the next event
                if irq && (self.cpu.int_mask >> 8) < 6 {
                    self.cpu.stopped = 0;
                    4
                } else {
                    self.bus.mfp.next_event().unwrap_or(160).min(160)
                }
            }
            _ => {
                self.die();
                return;
            }
        };
        self.pass(cycles);
    }

    /// Runs until the clock reaches `until`.
    fn run_to(&mut self, until: u64) {
        let mut steps = 0u32;
        while self.t < until && self.mode != Mode::Dead {
            // a play tick is due
            let next = self.tick_clock(self.tick);
            if self.at_idle() && self.t >= next {
                self.dosound_tick();
                self.start_call(BASE + 8, 0);
                self.tick += 1;
                let now = self.t;
                // a tick missed by a whole period is lost
                while self.tick_clock(self.tick) <= now {
                    self.tick += 1;
                }
                continue;
            }
            let pc = self.cpu.pc;
            if pc == CRASH_PC {
                self.die();
                break;
            }
            if pc == DONE_PC && self.cpu.dar[15] == STACK_TOP {
                // (the first play call follows the init at once)
                if matches!(self.mode, Mode::Init(_)) {
                    self.origin = self.t;
                    self.tick = 0;
                }
                self.go_idle();
                continue;
            }
            if let Mode::Init(limit) = self.mode {
                if self.t > limit {
                    self.die();
                    break;
                }
            }
            if self.at_idle() && !self.bus.mfp.irq() {
                // nothing to do until the next timer interrupt or play tick
                let mut to = until.min(next.max(self.t + 1));
                if let Some(e) = self.bus.mfp.next_event() {
                    to = to.min(self.t + e);
                }
                if self.bus.dma.playing() {
                    to = to.min(self.t + 160);
                }
                let dt = to.saturating_sub(self.t).max(1);
                self.pass(dt);
                continue;
            }
            self.step();
            steps += 1;
            if steps > STEP_LIMIT {
                // a zero-time loop: the clock moves at least 4 per step, so this cannot be reached
                // within one call's time; kept as the guard it is
                self.die();
            }
        }
    }
}

// ------------------------------------------------------------------ the player

pub struct SndhPlayer {
    data: Vec<u8>,
    info: Info,
    /// The subsong now, 0-based.
    cur: u16,
    m: Machine,
    chip: Ym,
    /// The CPU clock of the next chip step.
    chip_t: u64,
    /// The STE DMA's current output (left, right), -1..1 before the master volume.
    dma: (f32, f32),
    out_rate: u64,
    acc: u64,
    sum: [f32; 2],
    count: u32,
    /// The high-pass state per side: the last input and output.
    hp: [(f32, f32); 2],
    hp_k: f32,
    buf: Vec<f32>,
    pos: usize,
    /// Player ticks rendered.
    frames: u64,
}

const GAIN: f32 = 0.42;
const DMA_GAIN: f32 = 0.5;

impl SndhPlayer {
    pub fn new(d: &[u8], rate: u32) -> Result<SndhPlayer, String> {
        let info = parse(d).ok_or("SNDH: not an SNDH file")?;
        let out_rate = rate.clamp(1000, 1_000_000) as u64;
        let first = info.default_subsong - 1;
        let mut p = SndhPlayer {
            data: d.to_vec(),
            info,
            cur: first,
            m: Machine::new(d, 50, true, 1),
            chip: Ym::new(),
            chip_t: 0,
            dma: (0.0, 0.0),
            out_rate,
            acc: 0,
            sum: [0.0; 2],
            count: 0,
            hp: [(0.0, 0.0); 2],
            hp_k: 1.0 - 2.0 * std::f32::consts::PI * 5.0 / out_rate as f32,
            buf: Vec::new(),
            pos: 0,
            frames: 0,
        };
        p.start(first);
        Ok(p)
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// True when the tune's code ended (a crash, an init that does not return): it plays silence.
    pub fn dead(&self) -> bool {
        self.m.mode == Mode::Dead
    }

    /// Starts subsong `n` (0-based): the machine afresh, init called with n + 1 in d0.
    pub fn start(&mut self, n: u16) {
        let n = n.min(self.info.subsongs - 1);
        self.cur = n;
        self.m = Machine::new(&self.data, self.info.rate, self.info.vbl, n + 1);
        self.m.finish_init();
        self.chip.reset();
        self.chip_t = self.m.origin;
        self.dma = (0.0, 0.0);
        self.acc = 0;
        self.sum = [0.0; 2];
        self.count = 0;
        self.hp = [(0.0, 0.0); 2];
        self.buf.clear();
        self.pos = 0;
        self.frames = 0;
    }

    /// Runs the machine for `ticks` player ticks and returns the YM register writes it made, as
    /// (CPU clock, register, value), for a check against another player. The audio of those ticks
    /// is not made.
    pub fn ym_writes(&mut self, ticks: u64) -> Vec<(u64, u8, u8)> {
        let mut v = Vec::new();
        for _ in 0..ticks {
            let end = self.m.tick_clock(self.frames + 1);
            self.m.run_to(end);
            for (t, e) in self.m.bus.events.drain(..) {
                if let Ev::Ym(r, x) = e {
                    v.push((t.saturating_sub(self.m.origin), r, x));
                }
            }
            self.frames += 1;
        }
        v
    }

    /// Runs the machine through player tick `frames` and appends its samples to `buf`.
    fn frame(&mut self) {
        let end = self.m.tick_clock(self.frames + 1);
        self.m.run_to(end);
        if self.m.mode == Mode::Dead {
            self.chip.reset();
            self.m.bus.events.clear();
            self.dma = (0.0, 0.0);
        }
        let vol = self.m.bus.dma.volume as f32 / 64.0;
        while self.chip_t < end {
            while let Some(&(t, e)) = self.m.bus.events.front() {
                if t > self.chip_t {
                    break;
                }
                self.m.bus.events.pop_front();
                match e {
                    Ev::Ym(r, v) => self.chip.write(r, v),
                    Ev::Dma(l, r) => self.dma = (l as f32 / 128.0, r as f32 / 128.0),
                }
            }
            let y = self.chip.step() * GAIN;
            self.sum[0] += y + self.dma.0 * vol * DMA_GAIN;
            self.sum[1] += y + self.dma.1 * vol * DMA_GAIN;
            self.count += 1;
            self.acc += self.out_rate;
            if self.acc >= YM_STEP_HZ {
                self.acc -= YM_STEP_HZ;
                for side in 0..2 {
                    let x = self.sum[side] / self.count as f32;
                    let (px, py) = self.hp[side];
                    let y = x - px + self.hp_k * py;
                    self.hp[side] = (x, y);
                    self.buf.push(y.clamp(-1.0, 1.0));
                }
                self.sum = [0.0; 2];
                self.count = 0;
            }
            self.chip_t += YM_STEP;
        }
        self.frames += 1;
    }

    fn length(&self) -> Option<u64> {
        let t = *self.info.ticks.get(self.cur as usize)?;
        (t != 0).then_some(t as u64)
    }
}

impl crate::aiff::Stream for SndhPlayer {
    fn render(&mut self, out: &mut [f32]) -> usize {
        let mut i = 0;
        while i + 1 < out.len() {
            if self.pos >= self.buf.len() {
                self.buf.clear();
                self.pos = 0;
                self.frame();
                if self.buf.is_empty() {
                    continue;
                }
            }
            let n = (self.buf.len() - self.pos).min(out.len() - i) & !1;
            out[i..i + n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            i += n;
        }
        out.len() / 2
    }

    fn ended(&self) -> bool {
        self.length().is_some_and(|l| self.frames >= l)
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.info.subsongs - 1, self.cur)
    }

    fn set_subsong(&mut self, n: u16) {
        self.start(n);
    }

    fn duration_secs(&self) -> Option<f64> {
        self.length().map(|l| l as f64 / self.info.rate as f64)
    }

    fn author(&self) -> String {
        self.info.author.clone()
    }
}

/// The recorded-audio player of the engine on an SNDH tune.
pub fn player(d: &[u8], rate: u32) -> Result<crate::aiff::AiffPlayer, String> {
    let p = SndhPlayer::new(d, rate)?;
    let mut title = p.info.title.clone();
    if !p.info.year.is_empty() {
        title = format!("{title} ({})", p.info.year);
    }
    Ok(crate::aiff::AiffPlayer::from_stream(Box::new(p), "SNDH", title, rate))
}

#[cfg(test)]
#[path = "sndh_tests.rs"]
mod tests;
