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
//! the low-memory stubs, the BIOS calls that are answered, the cookie jar, how a call ends and
//! what time it takes) follows AtariAudio by Arnaud Carre (MIT; see `LICENSE-AtariAudio`), read
//! as a behavioural reference; no code is copied, but the behaviours it works out are kept where
//! the tunes need them. The chip models are this repository's own (`sndh_ym.rs`, `sndh_mfp.rs`).
//!
//! Timing. The machine lives on the output-sample clock, as the reference's does, and the 68000's
//! own cycle counts only bound a call. Each output sample the YM makes its sample, the DMA its
//! sample, then each MFP timer advances one sample's worth; a timer that underflows (and is
//! enabled and unmasked) has its handler called, to its RTE, at once. At every player tick (a
//! whole number of output samples, 958 at 48 kHz for the ST's 50 Hz frame) the play routine is
//! called to its RTS, before that sample, in the same way. A call takes no machine time: the
//! register writes it makes are all at the sample it was made at, the timers do not advance in
//! it and no interrupt comes in it, so a play routine and the handlers never interleave. That is
//! not how an ST behaves (there a handler can preempt the play routine, which takes some
//! thousands of clocks), but it is how AtariAudio's tunes are tuned and checked against, and the
//! register writes then come out in the same order and on the same output sample as in that
//! player (the timers are the host's: a timer's rate is exact on average, its underflows are
//! quantised to the output rate). The chip's own timing (the 250 kHz step, the averaging into the
//! output sample) is `sndh_ym.rs`'s.
//!
//! Not done: the blitter (`Blitmode` says there is none, and the cookie jar says an ST or STE
//! without one), the pulse-width modes of the MFP timers, the 68901's USART and GPIP lines but
//! the mono-detect bit, the ST's video registers but the two a tune might look at; a STOP
//! instruction ends the call (no interrupt can wake it). A tune's `FRMS`/`TIME` tag gives its
//! length; AtariAudio's database of lengths for tunes without one is not used.
//!
//! Bounds: every call has a budget of CPU clocks (an init one second's worth at 8 MHz, a play
//! call four frames, a handler one frame); a call that exceeds it, an illegal instruction, an
//! address error or a jump to the vectors of unset exceptions ends the tune (it plays silence).
//! A tune that never returns thus costs a bounded amount of CPU time.

#[path = "sndh_mfp.rs"]
pub mod mfp;
#[path = "sndh_ym.rs"]
pub mod ym;

use m68k::{AddressBus, CpuCore, CpuType, StepResult};
use mfp::Mfp;
use ym::Ym;

pub const MAGIC: &[u8; 4] = b"SNDH";

/// The ST's RAM.
const RAM_SIZE: usize = 4 << 20;
/// Where the tune is loaded (the same as AtariAudio's: some tunes do not play lower, or at a
/// 64 KiB boundary).
const BASE: u32 = 0x10002;
/// Low-memory addresses of the stubs.
const RTE_PC: u32 = 0x500;
/// A call ends when the CPU gets here (the reference player's RESET instruction): the return address.
const DONE_PC: u32 = 0x502;
/// The area GEMDOS Malloc hands out, from the top of RAM down by 1 MB.
const MALLOC_START: u32 = RAM_SIZE as u32 - (1 << 20);
/// Malloc does not hand out the top of RAM (the stack's).
const STACK_LIMIT: u32 = RAM_SIZE as u32 - 0x8010;
/// The CPU clocks (8 MHz) of one ST video frame: 313 lines of 512.
const FRAME_CLOCKS: u64 = 160_256;
/// An init that takes longer than this (CPU clocks) did not return.
const INIT_BUDGET: u64 = 8_000_000;
/// A play call, and a timer's handler.
const PLAY_BUDGET: u64 = 4 * FRAME_CLOCKS;
const HANDLER_BUDGET: u64 = FRAME_CLOCKS;
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
    /// The tune asks for the vertical blank (the default and `!V`) and not a timer tag. (Every tick
    /// is a whole number of output samples of the ST's 8.021247 MHz frame over the rate, as the
    /// reference player's: the flag is informational.)
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

/// The STE's DMA sound: frame start and end, counter, mode and the microwire registers. It runs
/// at the output rate like everything here: `sample` is called once per output sample.
#[derive(Clone)]
struct Dma {
    regs: [u8; 0x40],
    ptr: u32,
    end: u32,
    /// The DAC's clock accumulated towards the next output sample, in units of 1 / host rate.
    inner: u32,
    host_rate: u32,
    mw_data: u16,
    mw_mask: u16,
    mw_shift: u8,
    /// Master volume 0..=64.
    volume: i32,
    /// The 50 kHz mode's two-sample average.
    acc: (i32, i32),
    half: bool,
    /// The output now (left, right), in the chip's 16-bit units, the volume applied.
    level: (i32, i32),
}

impl Dma {
    fn new(host_rate: u32) -> Dma {
        Dma {
            regs: [0; 0x40],
            ptr: 0,
            end: 0,
            inner: 0,
            host_rate: host_rate.max(1),
            mw_data: 0,
            mw_mask: 0,
            mw_shift: 0,
            volume: 64,
            acc: (0, 0),
            half: false,
            level: (0, 0),
        }
    }

    fn fetch_pointers(&mut self) {
        let r = &self.regs;
        self.ptr = (r[3] as u32) << 16 | (r[5] as u32) << 8 | (r[7] & 0xFE) as u32;
        self.end = (r[0xF] as u32) << 16 | (r[0x11] as u32) << 8 | (r[0x13] & 0xFE) as u32;
    }

    fn write8(&mut self, o: usize, v: u8) {
        if o & 1 == 0 || o >= 0x40 {
            return;
        }
        match o {
            0x01 => {
                if v & 1 != 0 && (v ^ self.regs[1]) & 1 != 0 {
                    self.fetch_pointers();
                }
            }
            0x07 | 0x0D => {
                self.regs[o] = v & 0xFE;
                return;
            }
            0x21 => {
                if (v ^ self.regs[0x21]) & 3 != 0 {
                    self.acc = (0, 0);
                    self.half = false;
                }
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
            let d = (v & 0x3F) as i32;
            self.volume = if d > 40 { 64 } else { d * 64 / 40 };
        }
    }

    /// One output sample: the DAC's output (left, right) and whether a frame ended. The DAC
    /// reads the samples at its own rate, so a few (or none) are consumed per output sample; in
    /// the 50 kHz mode two successive ones are averaged (the output is 25 kHz).
    fn sample(&mut self, ram: &[u8]) -> ((i32, i32), bool) {
        let mut frame_end = false;
        if self.regs[1] & 1 == 0 {
            self.level = (0, 0);
            return (self.level, false);
        }
        let mode = self.regs[0x21];
        let rate = [6258u32, 12517, 25033, 50066][(mode & 3) as usize];
        let stereo = mode & 0x80 == 0;
        self.inner += rate;
        while self.inner >= self.host_rate {
            if self.ptr == self.end {
                frame_end = true;
                self.fetch_pointers();
                if self.regs[1] & 2 == 0 {
                    // no loop: the replay stops
                    self.regs[1] &= 0xFE;
                    self.level = (0, 0);
                    break;
                }
            }
            let get = |a: u32| ram.get((a & 0xFF_FFFF) as usize).map_or(0, |&b| b as i8 as i32);
            // a stereo frame is (left, right); the output is at the reference's level for a
            // centred sound (the sum of the two), so a mono source is not doubled
            let (l, r) = if stereo { (2 * get(self.ptr), 2 * get(self.ptr + 1)) } else { (get(self.ptr), get(self.ptr)) };
            if mode & 3 == 3 {
                self.acc.0 += l;
                self.acc.1 += r;
                self.half = !self.half;
                if !self.half {
                    self.level = ((self.acc.0 * self.volume) >> 1, (self.acc.1 * self.volume) >> 1);
                    self.acc = (0, 0);
                }
            } else {
                self.level = (l * self.volume, r * self.volume);
            }
            self.ptr = self.ptr.wrapping_add(if stereo { 2 } else { 1 }) & 0xFF_FFFF;
            self.inner -= self.host_rate;
        }
        (self.level, frame_end)
    }
}

/// The ST's memory and I/O as the CPU sees them.
struct StBus {
    ram: Vec<u8>,
    /// The YM register selected, and the registers as the CPU reads them back.
    ym_sel: u8,
    ym_regs: [u8; 16],
    ym: Ym,
    mfp: Mfp,
    dma: Dma,
    /// The output samples the chip has made so far: the time of a register write.
    sample_no: u64,
    /// The register writes (output sample, register, value) of a run that asks for them.
    log: Option<Vec<(u64, u8, u8)>>,
}

impl StBus {
    fn new(host_rate: u32) -> StBus {
        StBus {
            ram: vec![0; RAM_SIZE],
            ym_sel: 0,
            ym_regs: [0; 16],
            ym: Ym::new(host_rate),
            mfp: Mfp::new(host_rate),
            dma: Dma::new(host_rate),
            sample_no: 0,
            log: None,
        }
    }

    fn ym_write(&mut self, port: u32, v: u8) {
        if port & 2 != 0 {
            let r = self.ym_sel;
            if r < 14 {
                const MASK: [u8; 14] = [0xFF, 0x0F, 0xFF, 0x0F, 0xFF, 0x0F, 0x1F, 0x3F, 0x1F, 0x1F, 0x1F, 0xFF, 0xFF, 0x0F];
                self.ym_regs[r as usize] = v & MASK[r as usize];
                self.ym.write(r, v);
                if let Some(l) = &mut self.log {
                    l.push((self.sample_no, r, v));
                }
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
        if (a as usize) < RAM_SIZE { self.ram[a as usize] } else { self.io_read8(a) }
    }

    fn read_word(&mut self, a: u32) -> u16 {
        let a = a & 0xFF_FFFE;
        if (a as usize) < RAM_SIZE - 1 { self.r16(a) } else { self.io_read16(a) }
    }

    fn read_long(&mut self, a: u32) -> u32 {
        let a = a & 0xFF_FFFE;
        let hi = if (a as usize) < RAM_SIZE - 1 { self.r16(a) } else { self.io_read16(a) } as u32;
        let b = (a + 2) & 0xFF_FFFE;
        let lo = if (b as usize) < RAM_SIZE - 1 { self.r16(b) } else { self.io_read16(b) } as u32;
        hi << 16 | lo
    }

    fn write_byte(&mut self, a: u32, v: u8) {
        let a = a & 0xFF_FFFF;
        if (a as usize) < RAM_SIZE {
            self.ram[a as usize] = v;
        } else {
            self.io_write8(a, v);
        }
    }

    fn write_word(&mut self, a: u32, v: u16) {
        let a = a & 0xFF_FFFE;
        if (a as usize) < RAM_SIZE - 1 {
            self.w16(a, v);
        } else {
            self.io_write16(a, v);
        }
    }

    fn write_long(&mut self, a: u32, v: u32) {
        let a = a & 0xFF_FFFE;
        let (hi, lo) = ((v >> 16) as u16, v as u16);
        if (a as usize) < RAM_SIZE - 1 {
            self.w16(a, hi);
        } else {
            self.io_write16(a, hi);
        }
        let b = (a + 2) & 0xFF_FFFE;
        if (b as usize) < RAM_SIZE - 1 {
            self.w16(b, lo);
        } else {
            self.io_write16(b, lo);
        }
    }
}

// ------------------------------------------------------------------ the machine

struct Machine {
    cpu: CpuCore,
    bus: StBus,
    /// The tune's code ended or failed: it plays silence.
    dead: bool,
    malloc_next: u32,
    /// XBIOS DoSound: the script's address, the value register and the wait.
    dosnd_ptr: u32,
    dosnd_val: u8,
    dosnd_delay: u8,
}

impl Machine {
    fn new(data: &[u8], host_rate: u32) -> Machine {
        let mut bus = StBus::new(host_rate);
        let n = data.len().min(RAM_SIZE - BASE as usize - 0x1000);
        bus.ram[BASE as usize..BASE as usize + n].copy_from_slice(&data[..n]);
        // the vectors are zero but the timer C's (an RTE: the operating system's own handler is
        // not run), the division by zero's and those of an exception that was left: an unset one
        // runs the zeroed memory up to the RTE at 0x500, which returns from the exception
        bus.w32(0x14, RTE_PC);
        bus.w32(mfp::VECTORS[2], RTE_PC);
        bus.w16(RTE_PC, 0x4E73);
        bus.w16(DONE_PC, 0x4E70);
        // the cookie jar (an ST: the sound chip only; no blitter, no DMA sound for a tune that asks)
        bus.w32(0x900, 0x5F53_4E44);
        bus.w32(0x904, 0x3);
        bus.w32(0x908, 0x5F4D_4348);
        bus.w32(0x90C, 0x0001_0000);
        bus.w32(0x910, 0);
        bus.w32(0x5A0, 0x900);
        let mut cpu = CpuCore::new();
        cpu.set_cpu_type(CpuType::M68000);
        Machine { cpu, bus, dead: false, malloc_next: MALLOC_START, dosnd_ptr: 0, dosnd_val: 0, dosnd_delay: 0 }
    }

    /// Calls the routine at `pc` and runs it to its return, at once: no time passes in the
    /// machine while it runs (so no interrupt comes in it), and its register writes are all stamped
    /// with the output sample it is called at. The CPU starts as after a reset (supervisor, mask
    /// 7, the stack pointer and the start address left at the bottom of memory as the reset
    /// vectors) with the registers as the last call left them (d0 as given); the return address
    /// is a stub that ends the call, the return an RTS, or an RTE for a timer's handler. A call
    /// that does not return within `budget` CPU clocks, or that crashes, ends the tune.
    fn call(&mut self, pc: u32, d0: Option<u32>, rte: bool, budget: u64) -> bool {
        if self.dead {
            return false;
        }
        let top = RAM_SIZE as u32;
        self.bus.w32(top - 4, DONE_PC);
        let sp = if rte {
            self.bus.w16(top - 6, 0x2300);
            top - 6
        } else {
            top - 4
        };
        self.bus.w32(0, sp);
        self.bus.w32(4, pc);
        self.bus.w32(0x14, RTE_PC);
        let c = &mut self.cpu;
        c.set_sr(0x2700);
        if let Some(d) = d0 {
            c.dar[0] = d;
        }
        c.dar[15] = sp;
        c.pc = pc;
        c.stopped = 0;
        c.invalidate_prefetch();
        let mut spent = 0u64;
        while spent <= budget {
            if self.cpu.pc == DONE_PC {
                return true;
            }
            let cycles = match self.cpu.step(&mut self.bus) {
                StepResult::Ok { cycles } => cycles.max(4) as u64,
                StepResult::TrapInstruction { trap_num } => {
                    if !self.trap(trap_num) {
                        self.cpu.take_trap_exception(&mut self.bus, trap_num).max(34) as u64
                    } else {
                        34
                    }
                }
                // an illegal instruction, a line-A or line-F opcode, or a STOP (which waits for an
                // interrupt that cannot come) ends the tune
                _ => break,
            };
            spent += cycles;
        }
        self.dead = true;
        false
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
                        if self.malloc_next > STACK_LIMIT {
                            self.malloc_next = STACK_LIMIT;
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
                        self.bus.w32(mfp::VECTORS[timer as usize], vec);
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

    /// One output sample of the machine: the chip's, the DMA's, then the timers (whose handlers
    /// run now and take effect from the next sample). Returns (left, right) in 16-bit units, and
    /// false when a handler failed (the sample is then silent).
    fn sample(&mut self) -> (i32, i32, bool) {
        self.bus.sample_no += 1;
        let ym = self.bus.ym.sample();
        let (dma, frame_end) = {
            let b = &mut self.bus;
            b.dma.sample(&b.ram)
        };
        if frame_end {
            self.bus.mfp.event_a();
        }
        let clip = |v: i32| v.clamp(-32768, 32767);
        let (mut l, mut r) = (clip(ym + dma.0), clip(ym + dma.1));
        let mut ok = true;
        for t in 0..mfp::TIMERS {
            if self.bus.mfp.tick(t) {
                let pc = self.bus.r32(mfp::VECTORS[t]);
                self.bus.ym.set_in_irq(true);
                let done = self.call(pc, None, true, HANDLER_BUDGET);
                self.bus.ym.set_in_irq(false);
                if !done {
                    ok = false;
                    l = 0;
                    r = 0;
                    break;
                }
            }
        }
        (l, r, ok)
    }
}

// ------------------------------------------------------------------ the player

pub struct SndhPlayer {
    data: Vec<u8>,
    info: Info,
    /// The subsong now, 0-based.
    cur: u16,
    m: Machine,
    rate: u32,
    /// Output samples to a player tick, and those left before the next one.
    per_tick: u32,
    left: u32,
    /// Output samples made.
    samples: u64,
    /// The register writes the init made (kept for `ym_writes`).
    init_log: Vec<(u64, u8, u8)>,
}

impl SndhPlayer {
    pub fn new(d: &[u8], rate: u32) -> Result<SndhPlayer, String> {
        let info = parse(d).ok_or("SNDH: not an SNDH file")?;
        let rate = rate.clamp(1000, 1_000_000);
        let first = info.default_subsong - 1;
        let mut p = SndhPlayer {
            data: d.to_vec(),
            info,
            cur: first,
            m: Machine::new(d, rate),
            rate,
            per_tick: 1,
            left: 0,
            samples: 0,
            init_log: Vec::new(),
        };
        p.start(first);
        Ok(p)
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// True when the tune's code ended (a crash, an init that does not return): it plays silence.
    pub fn dead(&self) -> bool {
        self.m.dead
    }

    /// The output samples of one player tick: the ST's frame of 313 lines of 512 clocks at 50 Hz
    /// (8.021247 MHz in this model of the CPU's clock) over the tick rate, whole samples (the
    /// reference player does the same).
    fn samples_per_tick(rate: u32, tick_rate: u32) -> u32 {
        let n = rate as u64 * 313 * 512 * 50 / (tick_rate.max(1) as u64 * 8_021_247);
        n.max(1) as u32
    }

    /// Starts subsong `n` (0-based): the machine afresh, init called with n + 1 in d0.
    pub fn start(&mut self, n: u16) {
        let n = n.min(self.info.subsongs - 1);
        self.cur = n;
        self.m = Machine::new(&self.data, self.rate);
        self.m.bus.log = Some(Vec::new());
        self.m.call(BASE, Some(n as u32 + 1), false, INIT_BUDGET);
        self.init_log = self.m.bus.log.take().unwrap_or_default();
        self.m.bus.sample_no = 0;
        self.per_tick = Self::samples_per_tick(self.rate, self.info.rate);
        self.left = 0;
        self.samples = 0;
    }

    /// One output sample (left, right), as a fraction of full scale.
    fn next_sample(&mut self) -> (f32, f32) {
        if self.m.dead {
            return (0.0, 0.0);
        }
        if self.left == 0 {
            self.m.dosound_tick();
            if !self.m.call(BASE + 8, Some(0), false, PLAY_BUDGET) {
                return (0.0, 0.0);
            }
            self.left = self.per_tick;
        }
        self.left -= 1;
        self.samples += 1;
        let (l, r, _) = self.m.sample();
        (l as f32 / 32768.0, r as f32 / 32768.0)
    }

    /// Runs the machine for `ticks` player ticks and returns the YM register writes it made, as
    /// (output sample, register, value) (those of the init first, at sample 0), for a check
    /// against another player. The sample is the count of output samples made when the write
    /// happened.
    pub fn ym_writes(&mut self, ticks: u64) -> Vec<(u64, u8, u8)> {
        let mut v = std::mem::take(&mut self.init_log);
        self.m.bus.log = Some(Vec::new());
        let start = self.samples;
        let want = (self.samples + ticks * self.per_tick as u64).max(start);
        while self.samples < want && !self.m.dead {
            self.next_sample();
        }
        v.extend(self.m.bus.log.take().unwrap_or_default());
        v
    }

    fn length(&self) -> Option<u64> {
        let t = *self.info.ticks.get(self.cur as usize)?;
        (t != 0).then_some(t as u64)
    }
}

impl crate::aiff::Stream for SndhPlayer {
    fn render(&mut self, out: &mut [f32]) -> usize {
        for f in out.chunks_exact_mut(2) {
            let (l, r) = self.next_sample();
            f[0] = l;
            f[1] = r;
        }
        out.len() / 2
    }

    fn ended(&self) -> bool {
        self.length().is_some_and(|l| self.samples >= l * self.per_tick as u64)
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
