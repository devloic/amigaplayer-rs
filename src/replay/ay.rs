//! ZX Spectrum and Amstrad CPC AY tunes in the "ZXAYEMUL" container (`.ay`; modland names them
//! `.emul`; the Eagleplayer "PlayAY" with its "ZXAYEMUL" AYPlayer played them): the music is
//! Z80 code plus data, run on a Z80 interpreter (`ay_z80.rs`) driving an AY-3-8912 emulation
//! (`ay_chip.rs`), rendered to PCM at the output rate through the recorded-audio backend
//! (`crate::aiff::Stream`): no Paula, no Amiga filters.
//!
//! Original implementation, written for this project (GPL-2.0-or-later) from the public
//! description of the ZXAYEMUL format (the "AY file format" text of the Project AY / AY
//! Emulator documentation: header fields with 16-bit big-endian pointers relative to their own
//! position, the song structures, the points and memory blocks, and the player procedure:
//! memory filled with RET / $FF / 0, EI at $0038, the stub DI; CALL init; IM 2 or IM 1; EI;
//! HALT; [CALL interrupt]; JR, all registers loaded with HiReg/LoReg, I = 3, SP from the points,
//! the AY reset), from the Spectrum 128's and the CPC's documented AY ports, and from
//! observations of the 13 test files' headers. No emulator's or player's code was consulted.
//!
//! Machine: the Spectrum 128 (Z80 at 3.5469 MHz, 70,908 T-states per 50 Hz frame, AY at
//! 1.77345 MHz on ports $FFFD/$BFFD, the beeper on bit 4 of port $FE). A tune that writes the
//! CPC's PPI ports ($F4xx data, $F6xx control) within its first second is played as a CPC
//! (Z80 at 4 MHz, 80,000 T-states per frame, AY at 1 MHz); the interrupt stays at 50 Hz, as the
//! format's player procedure has it. The maskable interrupt is raised at the start of each
//! frame and held for 32 T-states.
//!
//! Output: ABC stereo (channel A left, C right, B in the middle at -3 dB on both sides), the
//! beeper in the middle, a DC-removing high-pass at 5 Hz. The chip is stepped every 8 of its
//! clocks and each output sample is the mean of the steps it covers. A song's length and fade
//! (in 1/50 s) end it: the fade is linear, then `ended`; a song without a length plays on.
//!
//! Bounds: each frame runs at most one frame of T-states (every instruction costs at least 4),
//! so a tune that never halts costs the same as one that does; the file's blocks are clipped to
//! the file and to 64 KB, at most 256 songs and 1,024 blocks per song are read.

#[path = "ay_z80.rs"]
pub mod z80;
#[path = "ay_chip.rs"]
pub mod chip;

use chip::Ay;
use z80::{Bus, Z80};

pub const MAGIC: &[u8; 8] = b"ZXAYEMUL";

const SPECTRUM: Clock = Clock { cpu: 3_546_900, frame: 70_938, ay: 1_773_450 };
const CPC: Clock = Clock { cpu: 4_000_000, frame: 80_000, ay: 1_000_000 };
/// How many T-states the interrupt line stays low at the start of a frame.
const INT_LEN: u64 = 32;
/// Frames the CPC test runs (a tune that has not touched an AY port by then is a Spectrum one).
const PROBE_FRAMES: u32 = 50;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Clock {
    cpu: u64,
    frame: u64,
    ay: u64,
}

// ------------------------------------------------------------------ the file

#[derive(Clone, Debug)]
pub struct Song {
    pub name: String,
    /// Length and fade in 1/50 s (0: not stated).
    pub length: u16,
    pub fade: u16,
    pub hi: u8,
    pub lo: u8,
    pub stack: u16,
    pub init: u16,
    pub interrupt: u16,
    /// Address in Z80 memory, offset in the file, length (already clipped).
    pub blocks: Vec<(u16, usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct AyFile {
    pub author: String,
    pub misc: String,
    pub first: u16,
    pub songs: Vec<Song>,
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

/// The target of the relative pointer at `o`, inside the file.
fn rel(d: &[u8], o: usize) -> Option<usize> {
    let v = be16(d, o)? as i16 as i64;
    let t = o as i64 + v;
    (0..d.len() as i64).contains(&t).then_some(t as usize)
}

/// A NUL-terminated string (at most 255 bytes), read as Latin-1.
fn cstr(d: &[u8], o: Option<usize>) -> String {
    let Some(o) = o else { return String::new() };
    d[o..].iter().take(255).take_while(|&&b| b != 0).map(|&b| b as char).collect::<String>().trim().to_string()
}

pub fn parse(d: &[u8]) -> Option<AyFile> {
    if d.len() < 20 || &d[..8] != MAGIC {
        return None;
    }
    let n = d[16] as usize + 1;
    let ss = rel(d, 18)?;
    let mut songs = Vec::new();
    for i in 0..n.min(256) {
        let o = ss + 4 * i;
        let Some(sd) = rel(d, o + 2) else { break };
        if sd + 14 > d.len() {
            break;
        }
        let name = cstr(d, rel(d, o));
        let mut song = Song {
            name,
            length: be16(d, sd + 4)?,
            fade: be16(d, sd + 6)?,
            hi: d[sd + 8],
            lo: d[sd + 9],
            stack: 0,
            init: 0,
            interrupt: 0,
            blocks: Vec::new(),
        };
        let pp = rel(d, sd + 10)?;
        song.stack = be16(d, pp)?;
        song.init = be16(d, pp + 2)?;
        // a points structure cut short by the end of the file: no interrupt routine
        song.interrupt = be16(d, pp + 4).unwrap_or(0);
        let mut pa = rel(d, sd + 12)?;
        for _ in 0..1024 {
            let Some(addr) = be16(d, pa) else { break };
            if addr == 0 {
                break;
            }
            let Some(len) = be16(d, pa + 2) else { break };
            if let Some(off) = rel(d, pa + 4) {
                let len = (len as usize).min(0x10000 - addr as usize).min(d.len() - off);
                if len > 0 {
                    song.blocks.push((addr, off, len));
                }
            }
            pa += 6;
        }
        songs.push(song);
    }
    if songs.is_empty() {
        return None;
    }
    let first = (d[17] as usize).min(songs.len() - 1) as u16;
    Some(AyFile { author: cstr(d, rel(d, 12)), misc: cstr(d, rel(d, 14)), first, songs })
}

/// A ZXAYEMUL file with at least one song this player can read.
pub fn detect(d: &[u8]) -> bool {
    d.len() >= 8 && &d[..8] == MAGIC && parse(d).is_some()
}

// ------------------------------------------------------------------ the machine

#[derive(Clone, Copy, Debug)]
enum Ev {
    Reg(u8, u8),
    Beep(bool),
}

/// The Z80's memory and ports: the AY's registers as the tune sees them, and the writes of the
/// current frame with their T-state (relative to the frame) for the sound to follow.
#[derive(Clone)]
struct Mach {
    mem: Box<[u8; 65536]>,
    regs: [u8; 16],
    sel: u8,
    /// The CPC's PPI: port A's latch and the AY function on port C's bits 7 and 6.
    ppi_a: u8,
    ppi_fn: u8,
    cpc: bool,
    frame_start: u64,
    events: Vec<(u64, Ev)>,
}

/// The AY's register masks, as reads give them back.
const MASK: [u8; 16] = [0xFF, 0x0F, 0xFF, 0x0F, 0xFF, 0x0F, 0x1F, 0xFF, 0x1F, 0x1F, 0x1F, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF];

impl Mach {
    fn ay_write(&mut self, v: u8, t: u64) {
        if self.sel < 16 {
            self.regs[self.sel as usize] = v & MASK[self.sel as usize];
            self.events.push((t.saturating_sub(self.frame_start), Ev::Reg(self.sel, v)));
        }
    }
    fn ppi_apply(&mut self, t: u64) {
        match self.ppi_fn {
            3 => self.sel = if self.ppi_a < 16 { self.ppi_a } else { 0xFF },
            2 => self.ay_write(self.ppi_a, t),
            _ => {}
        }
    }
}

impl Bus for Mach {
    fn read(&mut self, a: u16) -> u8 {
        self.mem[a as usize]
    }
    fn write(&mut self, a: u16, v: u8) {
        self.mem[a as usize] = v;
    }
    fn input(&mut self, port: u16, _t: u64) -> u8 {
        let hi = (port >> 8) as u8;
        if hi == 0xF4 {
            self.cpc = true;
            return if self.ppi_fn == 1 && self.sel < 16 { self.regs[self.sel as usize] } else { 0xFF };
        }
        if port & 0xC002 == 0xC000 && self.sel < 16 {
            return self.regs[self.sel as usize];
        }
        0xFF
    }
    fn output(&mut self, port: u16, v: u8, t: u64) {
        match (port >> 8) as u8 {
            0xF4 => {
                self.cpc = true;
                self.ppi_a = v;
                self.ppi_apply(t);
                return;
            }
            0xF6 => {
                self.cpc = true;
                self.ppi_fn = v >> 6;
                self.ppi_apply(t);
                return;
            }
            _ => {}
        }
        if port & 0xC002 == 0xC000 {
            self.sel = if v < 16 { v } else { 0xFF };
        } else if port & 0xC002 == 0x8000 {
            self.ay_write(v, t);
        } else if port & 1 == 0 && !self.cpc {
            self.events.push((t.saturating_sub(self.frame_start), Ev::Beep(v & 0x10 != 0)));
        }
    }
}

/// Memory, stub and registers for song `s` (the format's player procedure).
fn boot(f: &[u8], s: &Song) -> (Z80, Mach) {
    let mut mem = Box::new([0u8; 65536]);
    mem[..0x100].fill(0xC9);
    mem[0x100..0x4000].fill(0xFF);
    mem[0x38] = 0xFB;
    let init = if s.init != 0 { s.init } else { s.blocks.first().map_or(0, |b| b.0) };
    let mut stub = vec![0xF3, 0xCD, init as u8, (init >> 8) as u8];
    if s.interrupt == 0 {
        stub.extend_from_slice(&[0xED, 0x5E, 0xFB, 0x76, 0x18, 0xFA]);
    } else {
        let i = s.interrupt;
        stub.extend_from_slice(&[0xED, 0x56, 0xFB, 0x76, 0xCD, i as u8, (i >> 8) as u8, 0x18, 0xF7]);
    }
    mem[..stub.len()].copy_from_slice(&stub);
    for &(addr, off, len) in &s.blocks {
        mem[addr as usize..addr as usize + len].copy_from_slice(&f[off..off + len]);
    }
    let mut z = Z80::new();
    let hl = (s.hi as u16) << 8 | s.lo as u16;
    z.set_af(hl);
    z.set_bc(hl);
    z.set_de(hl);
    z.set_hl(hl);
    (z.af2, z.bc2, z.de2, z.hl2) = (hl, hl, hl, hl);
    z.ix = hl;
    z.iy = hl;
    z.i = 3;
    z.sp = s.stack;
    z.pc = 0;
    z.im = 0;
    let m = Mach {
        mem,
        regs: [0; 16],
        sel: 0,
        ppi_a: 0,
        ppi_fn: 0,
        cpc: false,
        frame_start: 0,
        events: Vec::with_capacity(256),
    };
    (z, m)
}

/// Runs one frame of `clock.frame` T-states, the interrupt at its start.
fn run_frame(z: &mut Z80, m: &mut Mach, clock: Clock) {
    m.events.clear();
    let start = m.frame_start;
    let end = start + clock.frame;
    while z.t < end {
        let rel = z.t - start;
        if rel < INT_LEN && z.iff1 && z.interrupt(m) != 0 {
            continue;
        }
        if z.halted && !(rel < INT_LEN && z.iff1) {
            z.halt_skip(end - z.t);
            break;
        }
        z.step(m);
    }
    m.frame_start = end;
}

// ------------------------------------------------------------------ the player

pub struct AyPlayer {
    file: Vec<u8>,
    ay: AyFile,
    cur: u16,
    z: Z80,
    m: Mach,
    chip: Ay,
    clock: Clock,
    rate: u64,
    beep: bool,
    /// T-state (relative to the frame) of the next chip step.
    phase: u64,
    /// The output-rate accumulator (in units of rate * 8 per chip step) and the sums of the
    /// current output sample.
    acc: u64,
    sum: [f32; 2],
    count: u32,
    /// The high-pass state per side: the last input and output.
    hp: [(f32, f32); 2],
    hp_k: f32,
    /// Rendered frames waiting to be taken (interleaved stereo) and the read position.
    buf: Vec<f32>,
    pos: usize,
    frames: u64,
}

const GAIN: f32 = 0.5;
const SIDE: f32 = 0.5;

impl AyPlayer {
    pub fn new(d: &[u8], rate: u32) -> Result<AyPlayer, String> {
        let ay = parse(d).ok_or("ZXAYEMUL: no song in the file")?;
        let rate = rate.clamp(1000, 1_000_000) as u64;
        let first = ay.first;
        let s = &ay.songs[first as usize];
        let (z, m) = boot(d, s);
        let mut p = AyPlayer {
            file: d.to_vec(),
            ay,
            cur: first,
            z,
            m,
            chip: Ay::new(),
            clock: SPECTRUM,
            rate,
            beep: false,
            phase: 0,
            acc: 0,
            sum: [0.0; 2],
            count: 0,
            hp: [(0.0, 0.0); 2],
            hp_k: 1.0 - 2.0 * std::f32::consts::PI * 5.0 / rate as f32,
            buf: Vec::new(),
            pos: 0,
            frames: 0,
        };
        p.start(first);
        Ok(p)
    }

    pub fn file(&self) -> &AyFile {
        &self.ay
    }

    /// Starts song `n` from the player procedure.
    pub fn start(&mut self, n: u16) {
        let n = n.min(self.ay.songs.len() as u16 - 1);
        self.cur = n;
        let s = &self.ay.songs[n as usize];
        let (z, m) = boot(&self.file, s);
        // the CPC test: a copy runs the first second
        let (mut tz, mut tm) = (z.clone(), m.clone());
        for _ in 0..PROBE_FRAMES {
            run_frame(&mut tz, &mut tm, SPECTRUM);
            if tm.cpc {
                break;
            }
        }
        self.clock = if tm.cpc { CPC } else { SPECTRUM };
        self.z = z;
        self.m = m;
        self.chip.reset();
        self.beep = false;
        self.phase = 0;
        self.acc = 0;
        self.sum = [0.0; 2];
        self.count = 0;
        self.hp = [(0.0, 0.0); 2];
        self.buf.clear();
        self.pos = 0;
        self.frames = 0;
    }

    pub fn is_cpc(&self) -> bool {
        self.clock == CPC
    }

    /// The gain of the fade at frame `f` of the song.
    fn fade_gain(&self, f: u64) -> f32 {
        let s = &self.ay.songs[self.cur as usize];
        if s.length == 0 || f < s.length as u64 {
            return 1.0;
        }
        let x = f - s.length as u64;
        if x >= s.fade as u64 {
            0.0
        } else {
            1.0 - x as f32 / s.fade as f32
        }
    }

    /// Runs one 50 Hz frame and appends its samples to `buf`.
    fn frame(&mut self) {
        run_frame(&mut self.z, &mut self.m, self.clock);
        let clock = self.clock;
        let per = 8 * clock.cpu / clock.ay;
        let gain = GAIN * self.fade_gain(self.frames);
        let dac = self.chip.dac;
        let mut ev = 0;
        let n_ev = self.m.events.len();
        let mut t = self.phase;
        while t < clock.frame {
            while ev < n_ev && self.m.events[ev].0 <= t {
                match self.m.events[ev].1 {
                    Ev::Reg(r, v) => self.chip.write(r, v),
                    Ev::Beep(b) => self.beep = b,
                }
                ev += 1;
            }
            let o = self.chip.step();
            let (a, b, c) = (dac[o[0] as usize], dac[o[1] as usize], dac[o[2] as usize]);
            let beep = if self.beep { 0.5 } else { 0.0 };
            self.sum[0] += a + SIDE * b + beep;
            self.sum[1] += c + SIDE * b + beep;
            self.count += 1;
            self.acc += self.rate * 8;
            if self.acc >= clock.ay {
                self.acc -= clock.ay;
                for side in 0..2 {
                    let x = self.sum[side] / self.count as f32;
                    let (px, py) = self.hp[side];
                    let y = x - px + self.hp_k * py;
                    self.hp[side] = (x, y);
                    self.buf.push((y * gain).clamp(-1.0, 1.0));
                }
                self.sum = [0.0; 2];
                self.count = 0;
            }
            t += per;
        }
        for &(_, e) in &self.m.events[ev..] {
            match e {
                Ev::Reg(r, v) => self.chip.write(r, v),
                Ev::Beep(b) => self.beep = b,
            }
        }
        self.phase = t - clock.frame;
        self.frames += 1;
    }

    fn ended_at(&self) -> Option<u64> {
        let s = &self.ay.songs[self.cur as usize];
        (s.length != 0).then_some(s.length as u64 + s.fade as u64)
    }
}

impl crate::aiff::Stream for AyPlayer {
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
        self.ended_at().is_some_and(|e| self.frames >= e)
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.ay.songs.len() as u16 - 1, self.cur)
    }

    fn set_subsong(&mut self, n: u16) {
        self.start(n);
    }

    fn duration_secs(&self) -> Option<f64> {
        self.ended_at().map(|f| f as f64 / 50.0)
    }
}

/// The recorded-audio player of the engine on an AY tune.
pub fn player(d: &[u8], rate: u32) -> Result<crate::aiff::AiffPlayer, String> {
    let p = AyPlayer::new(d, rate)?;
    let s = &p.ay.songs[p.cur as usize];
    let title = if p.ay.misc.is_empty() { s.name.clone() } else { p.ay.misc.clone() };
    Ok(crate::aiff::AiffPlayer::from_stream(Box::new(p), "ZXAYEMUL", title, rate))
}

#[cfg(test)]
#[path = "ay_tests.rs"]
mod tests;
