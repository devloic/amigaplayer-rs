//! NES Sound Format tunes (`.nsf`, magic `NESM\x1a`): the music is 6502 code plus data from an
//! NES game or a homebrew tool, run on a 2A03 CPU (`nsf_cpu.rs`) driving the 2A03 APU
//! (`nsf_apu.rs`), rendered to PCM at the output rate through the recorded-audio backend
//! (`crate::aiff::Stream`): no Paula, no Amiga filters.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the NESdev wiki's
//! "NSF" page (the header, the bankswitching of $8000-$FFFF in 4 KB banks by $5FF8-$5FFF with
//! the image padded by the load address AND $0FFF, the init procedure: RAM $0000-$07FF and
//! $6000-$7FFF cleared, $4000-$4013 cleared, $4015 written $00 then $0F, $4017 written $40, the
//! banks set, A = song - 1, X = 0 for NTSC / 1 for PAL, the stack at $01FF; PLAY called at the
//! header's rate, 1,000,000 / period Hz, a call still running when the next is due skipping
//! it) and its CPU and APU pages (see those files); the clocks from "Cycle reference chart"
//! (NTSC 21.477272 MHz / 12, PAL 26.601712 MHz / 16). No emulator's or player's code was
//! consulted.
//!
//! The host calls INIT and PLAY as subroutines: it pushes a return address that points at an
//! address no tune runs code at (`RETURN`, in the unmapped $5000 page) and runs the CPU until
//! the program counter reaches it. Between calls the CPU is idle and the APU runs on. The
//! machine: 2 KB of RAM mirrored to $1FFF, the APU at $4000-$4017, 8 KB of RAM at
//! $6000-$7FFF, the program at $8000-$FFFF (read only). The DMC's fetches stall the CPU 4
//! cycles. IRQs are not delivered (NSF 1 defines none); `$4015` reports their flags.
//!
//! A tune that needs an expansion sound chip (header byte $7B: VRC6, VRC7, FDS, MMC5, Namco
//! 163, Sunsoft 5B, VT02+) is refused with a message naming it. NSF 2 files play as NSF 1 (their
//! program length field is honoured, their metadata chunks are ignored). NSFe is not read.
//!
//! Output: mono (on both sides), the APU's nonlinear mix from the wiki's lookup tables, the
//! mean of the CPU cycles each output sample covers, then the NES's first-order high-pass at
//! 90 Hz and low-pass at 14 kHz (the high-pass starts at the first sample's level: no click),
//! times 2. The format states no length: a song ends after 5 s without any change in the APU's
//! output, else plays on.
//!
//! Bounds: the CPU runs in step with the output (at most the CPU cycles of the audio rendered),
//! whatever the code does. INIT runs at most 2 s of CPU time (then the host gives up on it and
//! calls PLAY); a PLAY call that does not return makes the later ones skipped; an STP opcode
//! ends the call it is in. The file's program data is clipped to 1 MB.

#[path = "nsf_cpu.rs"]
pub mod cpu;
#[path = "nsf_apu.rs"]
pub mod apu;

use apu::Apu;
use cpu::{Bus, Cpu};

pub const MAGIC: &[u8; 5] = b"NESM\x1a";
const HEADER: usize = 0x80;
const MAX_DATA: usize = 1 << 20;
/// Where INIT and PLAY return to (nothing is mapped in that page).
const RETURN: u16 = 0x5000;
/// NTSC and PAL CPU clocks in Hz.
pub const NTSC_HZ: u64 = 1_789_773;
pub const PAL_HZ: u64 = 1_662_607;
/// Seconds without a change in the output that end a song.
const SILENCE_SECS: u64 = 5;

/// The expansion sound chips of header byte $7B, by bit.
pub const CHIPS: [&str; 7] = ["VRC6", "VRC7", "FDS", "MMC5", "Namco 163", "Sunsoft 5B", "VT02+"];

#[derive(Clone, Debug)]
pub struct Header {
    pub version: u8,
    pub songs: u8,
    /// 1-based, as in the file.
    pub start: u8,
    pub load: u16,
    pub init: u16,
    pub play: u16,
    pub name: String,
    pub artist: String,
    pub copyright: String,
    pub ntsc_us: u16,
    pub pal_us: u16,
    pub banks: [u8; 8],
    pub region: u8,
    pub chips: u8,
}

impl Header {
    pub fn bankswitched(&self) -> bool {
        self.banks.iter().any(|&b| b != 0)
    }
    /// Whether the tune is played as PAL: PAL only (region bit 0 set, bit 1 clear); a dual
    /// tune plays as NTSC.
    pub fn pal(&self) -> bool {
        self.region & 3 == 1
    }
    /// The names of the expansion chips the tune needs.
    pub fn chip_names(&self) -> Vec<&'static str> {
        (0..7).filter(|b| self.chips & (1 << b) != 0).map(|b| CHIPS[b]).collect()
    }
}

fn text(d: &[u8]) -> String {
    let s: String = d.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
    let s = s.trim();
    if s == "<?>" {
        String::new()
    } else {
        s.to_string()
    }
}

pub fn parse(d: &[u8]) -> Option<Header> {
    if d.len() <= HEADER || &d[..5] != MAGIC {
        return None;
    }
    let w = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
    let h = Header {
        version: d[5],
        songs: d[6],
        start: d[7],
        load: w(8),
        init: w(0xA),
        play: w(0xC),
        name: text(&d[0x0E..0x2E]),
        artist: text(&d[0x2E..0x4E]),
        copyright: text(&d[0x4E..0x6E]),
        ntsc_us: w(0x6E),
        pal_us: w(0x78),
        banks: d[0x70..0x78].try_into().unwrap(),
        region: d[0x7A],
        chips: d[0x7B],
    };
    // at least one song; the program below $6000 cannot be placed
    if h.songs == 0 || h.load < 0x6000 {
        return None;
    }
    Some(h)
}

/// An NSF file with a header this player can read (it may still need an expansion chip:
/// `check` says).
pub fn detect(d: &[u8]) -> bool {
    parse(d).is_some()
}

/// Whether the tune can play here: `Err` names what it needs that is not emulated.
pub fn check(d: &[u8]) -> Result<Header, String> {
    let h = parse(d).ok_or("NSF: not an NSF file (or no song in it)")?;
    if h.chips & 0x7F != 0 {
        return Err(format!(
            "NSF: needs the expansion sound chip{} {}, which {} not emulated",
            if h.chip_names().len() > 1 { "s" } else { "" },
            h.chip_names().join(", "),
            if h.chip_names().len() > 1 { "are" } else { "is" }
        ));
    }
    Ok(h)
}

// ------------------------------------------------------------------ the machine

struct Mach {
    ram: [u8; 0x800],
    wram: Box<[u8; 0x2000]>,
    /// The program image: 4 KB banks, the first padded at its start.
    rom: Vec<u8>,
    /// The offset in `rom` of each 4 KB window of $8000-$FFFF.
    win: [usize; 8],
    bankswitched: bool,
    apu: Apu,
    /// CPU cycles the DMC's fetches have taken from the CPU.
    stall: u32,
}

impl Mach {
    fn rom_byte(&self, a: u16) -> u8 {
        let i = ((a - 0x8000) >> 12) as usize;
        self.rom.get(self.win[i] + (a as usize & 0xFFF)).copied().unwrap_or(0)
    }
    fn set_bank(&mut self, i: usize, v: u8) {
        let n = self.rom.len().div_ceil(0x1000).max(1);
        self.win[i] = (v as usize % n) * 0x1000;
    }
    /// One CPU cycle of the APU (and the DMC's fetch); returns the mix.
    fn tick(&mut self) -> f32 {
        let o = self.apu.clock();
        if let Some(a) = self.apu.dmc.wants() {
            let v = self.read(a);
            self.apu.dmc.fill(v);
            self.stall += 4;
        }
        o
    }
}

impl Bus for Mach {
    fn read(&mut self, a: u16) -> u8 {
        match a {
            0x0000..=0x1FFF => self.ram[a as usize & 0x7FF],
            0x4015 => self.apu.read_status(),
            0x6000..=0x7FFF => self.wram[a as usize - 0x6000],
            0x8000..=0xFFFF => self.rom_byte(a),
            _ => (a >> 8) as u8,
        }
    }
    fn write(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => self.ram[a as usize & 0x7FF] = v,
            0x4000..=0x4017 => self.apu.write(a, v),
            0x5FF8..=0x5FFF if self.bankswitched => self.set_bank(a as usize - 0x5FF8, v),
            0x6000..=0x7FFF => self.wram[a as usize - 0x6000] = v,
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ the player

pub struct NsfPlayer {
    h: Header,
    /// The program data (after the header).
    data: Vec<u8>,
    cur: u16,
    cpu: Cpu,
    m: Mach,
    pal: bool,
    hz: u64,
    rate: u64,
    /// A call (INIT or PLAY) is running; INIT is still running.
    busy: bool,
    in_init: bool,
    /// CPU cycles since the song's start; the cycle INIT gives up at.
    cyc: u64,
    init_limit: u64,
    /// The CPU cycles INIT took (when it has returned or been given up on).
    init_cycles: u64,
    /// The next PLAY call, in CPU cycles times 1,000,000 (the period is in microseconds).
    next_play: u64,
    period_us: u64,
    /// Output-rate accumulator (units of `rate` per CPU cycle) and the current sample's sum.
    acc: u64,
    sum: f32,
    count: u32,
    /// The filters' states: the high-pass's last input and output, the low-pass's output.
    hp: (f32, f32),
    lp: f32,
    hp_k: f32,
    lp_k: f32,
    /// Silence detection: the level the output keeps, output samples it has kept it.
    still_level: f32,
    still: u64,
    buf: Vec<f32>,
    pos: usize,
}

const GAIN: f32 = 2.0;

impl NsfPlayer {
    pub fn new(d: &[u8], rate: u32) -> Result<NsfPlayer, String> {
        let h = check(d)?;
        let len = u32::from_le_bytes([d[0x7D], d[0x7E], d[0x7F], 0]) as usize;
        let body = &d[HEADER..];
        let body = if h.version >= 2 && len > 0 { &body[..len.min(body.len())] } else { body };
        let data = body[..body.len().min(MAX_DATA)].to_vec();
        let pal = h.pal();
        let hz = if pal { PAL_HZ } else { NTSC_HZ };
        let rate = rate.clamp(1000, 1_000_000) as u64;
        let period_us = match if pal { h.pal_us } else { h.ntsc_us } {
            0 => {
                if pal {
                    19997
                } else {
                    16639
                }
            }
            p => p as u64,
        };
        let dt = 1.0 / rate as f32;
        let rc = |f: f32| 1.0 / (2.0 * std::f32::consts::PI * f);
        let mut p = NsfPlayer {
            cur: (h.start.max(1) - 1).min(h.songs - 1) as u16,
            data,
            cpu: Cpu::new(),
            m: Mach {
                ram: [0; 0x800],
                wram: Box::new([0; 0x2000]),
                rom: Vec::new(),
                win: [0; 8],
                bankswitched: false,
                apu: Apu::new(pal),
                stall: 0,
            },
            pal,
            hz,
            rate,
            busy: false,
            in_init: false,
            cyc: 0,
            init_limit: 0,
            init_cycles: 0,
            next_play: 0,
            period_us,
            acc: 0,
            sum: 0.0,
            count: 0,
            hp: (0.0, 0.0),
            lp: 0.0,
            hp_k: rc(90.0) / (rc(90.0) + dt),
            lp_k: dt / (rc(14_000.0) + dt),
            still_level: 0.0,
            still: 0,
            buf: Vec::new(),
            pos: 0,
            h,
        };
        p.start(p.cur);
        Ok(p)
    }

    pub fn header(&self) -> &Header {
        &self.h
    }

    pub fn is_pal(&self) -> bool {
        self.pal
    }

    /// Starts song `n` (0-based) by the init procedure.
    pub fn start(&mut self, n: u16) {
        let n = n.min(self.h.songs as u16 - 1);
        self.cur = n;
        let h = &self.h;
        let bs = h.bankswitched();
        // the image: bankswitched, padded by the load address's offset within its bank;
        // else the program at the load address, from $8000 (a part below $8000 goes to the RAM)
        let mut rom;
        let mut wram = Box::new([0u8; 0x2000]);
        if bs {
            rom = vec![0u8; (h.load & 0xFFF) as usize];
            rom.extend_from_slice(&self.data);
        } else {
            rom = vec![0u8; 0x8000];
            for (i, &b) in self.data.iter().enumerate() {
                let a = h.load as usize + i;
                match a {
                    0x6000..=0x7FFF => wram[a - 0x6000] = b,
                    0x8000..=0xFFFF => rom[a - 0x8000] = b,
                    _ => break,
                }
            }
        }
        self.m = Mach {
            ram: [0; 0x800],
            wram,
            rom,
            win: std::array::from_fn(|i| i * 0x1000),
            bankswitched: bs,
            apu: Apu::new(self.pal),
            stall: 0,
        };
        for a in 0x4000..=0x4013 {
            self.m.write(a, 0);
        }
        self.m.write(0x4015, 0);
        self.m.write(0x4015, 0x0F);
        self.m.write(0x4017, 0x40);
        if bs {
            for i in 0..8 {
                self.m.write(0x5FF8 + i as u16, h.banks[i]);
            }
        }
        self.cpu = Cpu::new();
        self.cpu.s = 0xFF;
        let init = h.init;
        self.call(init, n as u8, self.pal as u8);
        self.in_init = true;
        self.cyc = 0;
        self.init_limit = 2 * self.hz;
        self.next_play = 0;
        self.acc = 0;
        self.sum = 0.0;
        self.count = 0;
        self.hp = (f32::NAN, 0.0);
        self.lp = 0.0;
        self.still_level = 0.0;
        self.still = 0;
        self.buf.clear();
        self.pos = 0;
    }

    /// Calls the subroutine at `addr` with A and X (Y = 0); the stack at $01FF.
    fn call(&mut self, addr: u16, a: u8, x: u8) {
        let c = &mut self.cpu;
        c.s = 0xFF;
        c.a = a;
        c.x = x;
        c.y = 0;
        c.p |= cpu::I;
        c.jammed = false;
        let r = RETURN.wrapping_sub(1);
        c.push(&mut self.m, (r >> 8) as u8);
        c.push(&mut self.m, r as u8);
        c.pc = addr;
        self.busy = true;
    }

    /// Runs `n` CPU cycles of the machine, the CPU's instruction first, then its cycles of the
    /// APU; appends finished output samples to `buf`.
    fn run(&mut self, n: u64) {
        let end = self.cyc + n;
        while self.cyc < end {
            // a PLAY call is due: made if the CPU is idle, else skipped
            if !self.in_init && self.cyc * 1_000_000 >= self.next_play {
                if !self.busy {
                    let play = self.h.play;
                    self.call(play, 0, 0);
                }
                while self.next_play <= self.cyc * 1_000_000 {
                    self.next_play += self.period_us * self.hz;
                }
            }
            let mut c = 1;
            if self.m.stall > 0 {
                c = self.m.stall;
                self.m.stall = 0;
            } else if self.busy {
                c = self.cpu.step(&mut self.m);
                if self.cpu.pc == RETURN || self.cpu.jammed {
                    self.busy = false;
                }
                if self.in_init && (!self.busy || self.cyc >= self.init_limit) {
                    self.busy = false;
                    self.in_init = false;
                    self.init_cycles = self.cyc;
                    self.next_play = self.cyc * 1_000_000;
                }
            }
            for _ in 0..c {
                let o = self.m.tick();
                self.sample(o);
            }
            self.cyc += c as u64;
        }
    }

    fn sample(&mut self, o: f32) {
        self.sum += o;
        self.count += 1;
        self.acc += self.rate;
        if self.acc < self.hz {
            return;
        }
        self.acc -= self.hz;
        let x = self.sum / self.count as f32;
        self.sum = 0.0;
        self.count = 0;
        if (x - self.still_level).abs() > 1e-3 {
            self.still_level = x;
            self.still = 0;
        } else {
            self.still += 1;
        }
        // the first sample sets the high-pass's input level: no click from the APU's resting
        // level (the triangle holds 15)
        if self.hp.0.is_nan() {
            self.hp.0 = x;
        }
        let (px, py) = self.hp;
        let y = self.hp_k * (py + x - px);
        self.hp = (x, y);
        self.lp += self.lp_k * (y - self.lp);
        let v = (self.lp * GAIN).clamp(-1.0, 1.0);
        self.buf.push(v);
        self.buf.push(v);
    }

    /// Renders about 1/100 s.
    fn chunk(&mut self) {
        self.run(self.hz / 100);
    }
}

impl crate::aiff::Stream for NsfPlayer {
    fn render(&mut self, out: &mut [f32]) -> usize {
        let mut i = 0;
        while i + 1 < out.len() {
            if self.pos >= self.buf.len() {
                self.buf.clear();
                self.pos = 0;
                self.chunk();
                continue;
            }
            let n = (self.buf.len() - self.pos).min(out.len() - i) & !1;
            out[i..i + n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            i += n;
        }
        out.len() / 2
    }

    fn ended(&self) -> bool {
        self.still >= SILENCE_SECS * self.rate
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.h.songs as u16 - 1, self.cur)
    }

    fn set_subsong(&mut self, n: u16) {
        self.start(n);
    }

    fn duration_secs(&self) -> Option<f64> {
        None
    }

    fn author(&self) -> String {
        self.h.artist.clone()
    }
}

/// The recorded-audio player of the engine on an NSF tune.
pub fn player(d: &[u8], rate: u32) -> Result<crate::aiff::AiffPlayer, String> {
    let p = NsfPlayer::new(d, rate)?;
    let title = p.h.name.clone();
    Ok(crate::aiff::AiffPlayer::from_stream(Box::new(p), "NSF", title, rate))
}

#[cfg(test)]
#[path = "nsf_tests.rs"]
mod tests;
