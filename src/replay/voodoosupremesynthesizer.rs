//! Voodoo Supreme Synthesizer (`vss.*`, extension form `NAME.vss`; Tomas Partl, Voodoo Software,
//! 1993; Eagleplayer name "VSS").
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! and execution traces of the 68k reference player; no original code. A module is pure data: a
//! header at the end of the file (magic `VSS0`, the subsong count and one song-table offset per
//! subsong), a table of object offsets per subsong, and the objects (four note streams, waveforms,
//! envelopes, wave sequences, samples). Four voices; every video frame a tick runs the voices'
//! note streams, volume and pitch envelopes, portamento and pitch command, writes the period and
//! volume of the previous tick and builds the voice's synth waveform half by half (mix of two
//! waves, xor window, morph window) or steps through a sample block by block from the voice's audio
//! interrupt.
//!
//! The two player tables (96 note periods and 13 semitone ratio pairs) are tables of the original
//! player, not of the modules. They are embedded on the project owner's decision
//! (`voodoosupremesynthesizer_period.bin`, `voodoosupremesynthesizer_ratio.bin`, taken from the
//! original player); nothing else of the original is embedded.
//!
//! Every register write is placed at the colour clock the specification's cycle ledger gives
//! (a call's counter `T` in 68000 cycles, a write stamped at the call's start plus `T / 2`). The
//! tick is called by the host once per frame, delayed by a running audio interrupt handler; the
//! handler, one per channel, acknowledges the interrupt and, in sample mode, moves the channel to
//! the next block.
//!
//! Choices where the specification is silent (all safe, none can panic or hang): an undefined
//! command byte, a portamento with a zero length or equal notes, a note index of 122 or more (past
//! the tables), a subroutine or loop nesting of more than 20 stack entries, a return or loop end
//! with nothing on the stack, and a stream that does not end a read within 65536 commands end the
//! voice: it falls silent (volume 0, no more events) and the other voices go on. A pitch-corrected
//! sample step whose reference period index lies past the tables plays the plain step (0x80). A
//! module whose song table offsets lie outside the file is not recognised. Reads of machine memory
//! at an address below 0x400 (a pointer that was never set) give the bytes the specification
//! gives (section 5), other reads the guest memory.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Voodoo Supreme Synthesizer", detect, new };

const PERIODS: &[u8] = include_bytes!("voodoosupremesynthesizer_period.bin");
const RATIOS: &[u8] = include_bytes!("voodoosupremesynthesizer_ratio.bin");

/// Colour clocks between two tick calls (one video frame).
const TICK: u64 = crate::bus::FRAME;
/// The first AUDxLEN write of the start-up, from the run's origin, for a header position index of
/// 0, and the colour clocks one index step adds (the host's header scan; measured).
const START_BASE: u64 = 2631;
const START_STEP: u64 = 49;
/// INTENA is written this many colour clocks after the first LEN write.
const INTENA_AFTER: u64 = 544;
/// Colour clocks from the end of the start-up call to the first audio interrupt handler call.
const START_TO_IRQ: u64 = 33;

/// Cycles from the end of voice 3 to the tick's return (fitted to the reference's interrupt times).
const TICK_END: u64 = 217;

/// Cycles the song-end signal adds to the tick.
const SONGEND_COST: u64 = 72;

/// Subroutine stack entries per voice.
const STACK: usize = 20;
/// Commands one stream read may execute before the voice is ended.
const READ_LIMIT: usize = 65536;

const INTREQ: u32 = 0xDFF09C;
const INTENA: u32 = 0xDFF09A;

fn be16(d: &[u8], o: usize) -> u16 {
    super::be16(d, o)
}

fn be32(d: &[u8], o: usize) -> u32 {
    super::be32(d, o)
}

/// The header of a module: the offset of the `VSS0` magic, the subsong count, and the position
/// index `k` (0..31) of the magic in the scan.
fn header(d: &[u8]) -> Option<(usize, usize, usize)> {
    if d.len() % 2 != 0 || d.len() < 12 {
        return None;
    }
    for k in 0..32usize {
        let o = (d.len() as i64 - 64 + 2 * k as i64) as isize;
        if o < 0 || o as usize + 8 > d.len() {
            continue;
        }
        let o = o as usize;
        if be32(d, o) != 0x5653_5330 {
            continue;
        }
        let n = be32(d, o + 4);
        if n >= 256 {
            return None;
        }
        return Some((o, n as usize, k));
    }
    None
}

/// The song table offset (in the file) of subsong `s`, when it lies inside the file.
fn song_table(d: &[u8], hdr: usize, s: usize) -> Option<usize> {
    let rel = be32(d, hdr + 8 + 4 * s) as i32 as i64;
    let a = hdr as i64 + rel;
    (a >= 0 && (a as usize) + 4 <= d.len()).then_some(a as usize)
}

fn detect(d: &[u8]) -> bool {
    let Some((hdr, n, _)) = header(d) else { return false };
    n >= 1 && hdr + 8 + 4 * n <= d.len() && (0..n).all(|s| song_table(d, hdr, s).is_some())
}

/// The period table entry `i`: the period table, then the ratio table read as words; past that
/// the player's own work memory, which is not specified (`None`).
fn period_at(i: u8) -> Option<u16> {
    let i = i as usize;
    if i < 96 {
        Some(be16(PERIODS, 2 * i))
    } else if i < 96 + RATIOS.len() / 2 {
        Some(be16(RATIOS, 2 * (i - 96)))
    } else {
        None
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    w1: u32,
    w2: u32,
    half: u32,
    phase: u8,
    speed: u8,
    sphase: u8,
    buf: u32,
    mask: u8,
    scnt: u8,
    sp: u32,
    ev: u8,
    vol: u8,
    per: u16,
    stack: [u32; STACK],
    depth: usize,
    venv: u32,
    vpos: u8,
    vcnt: u8,
    penv: u32,
    ppos: u8,
    pcnt: u8,
    pcmd: u8,
    pstep: u8,
    pint: u8,
    picd: u8,
    plen: u8,
    trans: u8,
    flags: u8,
    seq: u32,
    spos: u8,
    mode: u8,
    cstep: u8,
    oper: u16,
    ovol: u8,
    vdelta: u8,
    /// Ended by an undefined case (see the module comment).
    halted: bool,
}

pub struct Vss {
    hdr: u32,
    nsub: u16,
    sub: u16,
    /// The 512-byte block: four 64-byte synth buffers, then the silent area.
    mem: u32,
    songtab: u32,
    v: [Voice; 4],
    vscale: u32,
    songend: bool,
    jumps: u16,
    /// The tick's call time and the cycle counter of the call.
    t0: u64,
    t: u64,
    /// The time of the first tick (taken by `subsong_tick_at`).
    first_tick: Option<u64>,
}

impl Vss {
    fn zero(&self) -> u32 {
        self.mem + 0x100
    }

    /// One byte of the machine's memory as the player sees it.
    fn rd8(&self, hw: &Hw, a: u32) -> u8 {
        mem8(hw, a)
    }

    fn rd16(&self, hw: &Hw, a: u32) -> u16 {
        ((self.rd8(hw, a) as u16) << 8) | self.rd8(hw, a.wrapping_add(1)) as u16
    }

    fn rd32(&self, hw: &Hw, a: u32) -> u32 {
        ((self.rd16(hw, a) as u32) << 16) | self.rd16(hw, a.wrapping_add(2)) as u32
    }

    /// Object `i` of the song table.
    fn obj(&self, hw: &Hw, i: u8) -> u32 {
        let e = self.songtab.wrapping_add(4 * i as u32);
        self.songtab.wrapping_add(self.rd32(hw, e))
    }

    /// Lets the call's counter reach the write: the clock is the call's start plus `T / 2`.
    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.t / 2;
    }

    fn halt(&mut self, v: usize) {
        let vo = &mut self.v[v];
        vo.halted = true;
        vo.vol = 0;
        vo.vdelta = 0;
        vo.vcnt = 0;
    }

    /// The state of a subsong's start (section 2).
    fn init(&mut self, hw: &Hw, sub: u16) {
        let hdr = self.hdr;
        self.sub = sub;
        self.songtab = hdr.wrapping_add(self.rd32(hw, hdr + 8 + 4 * sub as u32));
        let vscale = self.vscale;
        self.v = [Voice::default(); 4];
        for v in 0..4 {
            let sp = self.obj(hw, v as u8);
            let vo = &mut self.v[v];
            vo.ev = 1;
            vo.speed = 1;
            vo.sp = sp;
            vo.buf = self.mem + 0x40 * v as u32;
        }
        self.vscale = vscale;
        self.songend = false;
        self.jumps = 0;
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let (hdr, n, k) = header(d)?;
    let mem = m.alloc(512);
    if mem == 0 {
        return None;
    }
    let mut r = Vss {
        hdr: base.wrapping_add(hdr as u32),
        nsub: n as u16,
        sub: 0,
        mem,
        songtab: 0,
        v: [Voice::default(); 4],
        vscale: 64,
        songend: false,
        jumps: 0,
        t0: 0,
        t: 0,
        first_tick: None,
    };
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut hw = Hw { bus: &mut m.bus };
    r.init(&hw, 0);
    let l0 = origin + START_BASE + START_STEP * k as u64;
    r.start_writes(&mut hw, l0);
    hw.bus.clock = l0 + INTENA_AFTER;
    m68k::AddressBus::write_word(hw.bus, INTENA, 0x8780);
    hw.bus.clock += START_TO_IRQ;
    // the host's vertical blank runs the first tick one frame after the run's origin
    r.first_tick = Some(origin + TICK);
    Some(Box::new(r))
}

impl Vss {
    /// The start-up writes (section 2) from the first LEN write at `l0`.
    fn start_writes(&mut self, hw: &mut Hw, l0: u64) {
        for c in 0..4 {
            hw.bus.clock = l0 + 8 * c as u64;
            hw.len(c, 0x10);
        }
        for c in 0..4 {
            hw.bus.clock = l0 + 32 + 8 * c as u64;
            hw.vol(c, 0);
        }
        let z = self.zero();
        for c in 0..4 {
            hw.bus.clock = l0 + 192 + 16 * c as u64;
            hw.loc(c, z);
        }
        hw.bus.clock = l0 + 248;
        hw.dmacon(0x800F);
    }

    // ---------------------------------------------------------------- the tick

    fn tick_voices(&mut self, hw: &mut Hw) {
        self.jumps = 0;
        self.t = 236;
        for v in 0..4 {
            if v > 0 {
                self.t += 38;
            }
            self.voice(hw, v);
        }
        if self.jumps == 15 {
            self.songend = true;
            // the call that signals the song end to the host (fitted to the interrupt times
            // that follow the song-end tick)
            self.t += SONGEND_COST;
        }
        self.t += TICK_END;
        self.at(hw);
    }

    fn voice(&mut self, hw: &mut Hw, v: usize) {
        // 4.1 event counter
        self.t += 29;
        if !self.v[v].halted {
            self.v[v].ev = self.v[v].ev.wrapping_sub(1);
            if self.v[v].ev == 0 {
                self.t += 18;
                self.read_stream(hw, v);
            }
        }
        // 4.2 volume envelope
        self.t += 25;
        if !self.v[v].halted {
            self.volume_env(hw, v);
        }
        // 4.3 pitch envelope
        self.pitch_env(hw, v);
        // 4.4 portamento
        self.portamento(v);
        // 4.5 pitch command
        self.pitch_command(v);
        // 4.6 register writes
        self.t += 16;
        self.at(hw);
        hw.dmacon(0x8000 | 1 << v);
        let vo = self.v[v];
        self.t += 36;
        self.at(hw);
        hw.per(v, vo.oper);
        self.t += 30;
        self.at(hw);
        hw.vol(v, vo.ovol as u16);
        self.t += 140;
        let vscale = self.vscale;
        let vo = &mut self.v[v];
        vo.oper = vo.per;
        vo.ovol = ((vo.vol as u32 * vscale) >> 6) as u8;
        // 4.7 mode work
        self.t += 27;
        if self.v[v].mode & 0x40 != 0 {
            self.mode_b(hw, v);
            return;
        }
        self.t += 27;
        if self.v[v].mode & 0x80 != 0 {
            self.mode_c(hw, v);
            return;
        }
        self.t += 27;
        if self.v[v].mode & 0x20 != 0 {
            self.mode_d(hw, v);
        } else {
            self.mode_a(hw, v);
        }
    }

    fn volume_env(&mut self, hw: &Hw, v: usize) {
        let mut vo = self.v[v];
        vo.vcnt = vo.vcnt.wrapping_sub(1);
        let mut skip = false;
        if vo.vcnt == 0 {
            let mut guard = 0;
            loop {
                guard += 1;
                if guard > 1024 {
                    self.v[v] = vo;
                    self.halt(v);
                    return;
                }
                self.t += 67;
                let a = vo.venv.wrapping_add(vo.vpos as u32);
                let val = self.rd8(hw, a);
                if val == 0x88 {
                    self.t += 30;
                    vo.vpos = self.rd8(hw, a.wrapping_add(1));
                    continue;
                }
                let c = self.rd8(hw, a.wrapping_add(1));
                self.t += 25;
                vo.vpos = vo.vpos.wrapping_add(2);
                if c == 0 {
                    vo.vol = val;
                    vo.vcnt = 1;
                    self.t += 54;
                    skip = true;
                } else {
                    vo.vdelta = val;
                    vo.vcnt = c;
                    self.t += 52;
                }
                break;
            }
        }
        if !skip {
            vo.vol = vo.vol.wrapping_add(vo.vdelta);
            self.t += 28;
        }
        self.v[v] = vo;
    }

    fn pitch_env(&mut self, hw: &Hw, v: usize) {
        let mut vo = self.v[v];
        self.t += 29;
        vo.pcnt = vo.pcnt.wrapping_sub(1);
        if vo.pcnt == 0 {
            let mut guard = 0;
            loop {
                guard += 1;
                if guard > 1024 {
                    self.v[v] = vo;
                    self.halt(v);
                    return;
                }
                self.t += 73;
                let a = vo.penv.wrapping_add(vo.ppos as u32);
                let val = self.rd8(hw, a);
                if val == 0xFF {
                    self.t += 30;
                    vo.ppos = self.rd8(hw, a.wrapping_add(1));
                    continue;
                }
                vo.pcmd = val;
                vo.pcnt = self.rd8(hw, a.wrapping_add(1));
                vo.ppos = vo.ppos.wrapping_add(2);
                self.t += 52;
                break;
            }
        }
        self.v[v] = vo;
    }

    fn portamento(&mut self, v: usize) {
        let vo = &mut self.v[v];
        self.t += 29;
        vo.plen = vo.plen.wrapping_sub(1);
        if vo.plen == 0 {
            vo.pstep = 0;
            vo.picd = 0;
            self.t += 42;
            return;
        }
        self.t += 29;
        vo.picd = vo.picd.wrapping_sub(1);
        if vo.picd == 0 {
            vo.picd = vo.pint;
            self.t += 55;
            if vo.pstep < 0x7F {
                vo.per = vo.per.wrapping_add(vo.pstep as u16);
                self.t += 26;
            } else {
                vo.per = vo.per.wrapping_sub((vo.pstep & 0x7F) as u16);
                self.t += 24;
            }
        }
    }

    fn pitch_command(&mut self, v: usize) {
        let vo = &mut self.v[v];
        match vo.pcmd {
            0xFE => {
                vo.per <<= 1;
                self.t += 61;
            }
            0x7F => {
                vo.per >>= 1;
                self.t += 78;
            }
            0x7E => {
                let n = vo.pcnt;
                vo.pcnt = 1;
                let down = n & 0x80 != 0;
                let mut s = if down { n & 0x7F } else { n } as usize;
                let mut per = vo.per;
                self.t += 122 + if down { 25 } else { 17 };
                if s >= 13 {
                    self.t += 194;
                    let q = (s / 12) & 7;
                    s %= 12;
                    per = if down { per << q } else { per >> q };
                }
                self.t += if down { 64 } else { 74 };
                self.t += 357;
                let a = be16(RATIOS, 4 * s) as u32;
                let b = be16(RATIOS, 4 * s + 2) as u32;
                let (mult, div) = if down { (a, b) } else { (b, a) };
                let prod = per as u32 * mult;
                let (res, rem) = if div == 0 {
                    (prod as u16, false)
                } else if prod / div > 0xFFFF {
                    (prod as u16, prod >> 16 != 0)
                } else {
                    ((prod / div) as u16, prod % div != 0)
                };
                if rem {
                    self.t += 4;
                }
                vo.per = if down && rem { res.wrapping_add(1) } else { res };
                self.t += 22;
            }
            c if c < 0x7E => {
                vo.per = vo.per.wrapping_add(c as u16);
                self.t += 104;
            }
            c => {
                vo.per = vo.per.wrapping_sub((c & 0x7F) as u16);
                self.t += 122;
            }
        }
    }

    // ---------------------------------------------------------------- the stream

    fn read_stream(&mut self, hw: &mut Hw, v: usize) {
        self.t += 16;
        for _ in 0..READ_LIMIT {
            let b = self.next(hw, v);
            self.t += 33;
            if b < 0x80 {
                let d = self.next(hw, v);
                let Some(p) = period_at(b.wrapping_add(self.v[v].trans)) else {
                    self.halt(v);
                    return;
                };
                self.v[v].per = p;
                self.v[v].ev = d;
                self.t += 96;
                self.tail(v, d);
                return;
            }
            self.t += 25;
            if b == 0xFF {
                let d = self.next(hw, v);
                let vo = &mut self.v[v];
                vo.vol = 0;
                vo.vcnt = 0;
                vo.ovol = 0;
                vo.vdelta = 0;
                vo.ev = d;
                self.t += 74 + 32;
                return;
            }
            self.t += 24 + 22 * (b as u64 - 0x80);
            if !self.command(hw, v, b) {
                return;
            }
            if self.v[v].halted {
                return;
            }
        }
        self.halt(v);
    }

    fn next(&mut self, hw: &Hw, v: usize) -> u8 {
        let a = self.v[v].sp;
        self.v[v].sp = a.wrapping_add(1);
        self.rd8(hw, a)
    }

    fn push(&mut self, v: usize, x: u32) -> bool {
        let vo = &mut self.v[v];
        if vo.depth >= STACK {
            return false;
        }
        vo.stack[vo.depth] = x;
        vo.depth += 1;
        true
    }

    fn pop(&mut self, v: usize) -> Option<u32> {
        let vo = &mut self.v[v];
        if vo.depth == 0 {
            return None;
        }
        vo.depth -= 1;
        Some(vo.stack[vo.depth])
    }

    /// The note tail (3.1) after a note or a portamento.
    fn tail(&mut self, v: usize, d: u8) {
        let vo = &mut self.v[v];
        vo.mode &= !2;
        vo.ev = d;
        self.t += 36 + 25;
        if vo.flags & 0x20 == 0 {
            vo.spos = 0;
            vo.scnt = 1;
            vo.phase = vo.sphase;
            self.t += 72;
        }
        self.t += 25;
        if vo.flags & 0x80 == 0 {
            vo.vpos = 0;
            vo.vcnt = 1;
            self.t += 32;
        }
        self.t += 25;
        if vo.flags & 0x40 == 0 {
            vo.ppos = 0;
            vo.pcnt = 1;
            self.t += 32;
        }
        self.t += 32;
    }

    /// Executes command byte `b` (0x81..0xFE); false ends the read.
    fn command(&mut self, hw: &mut Hw, v: usize, b: u8) -> bool {
        match b {
            0x81 => {
                let i = self.next(hw, v);
                let sp = self.v[v].sp;
                if !self.push(v, sp) {
                    self.halt(v);
                    return false;
                }
                self.v[v].sp = self.obj(hw, i);
                self.t += 200;
            }
            0x82 => {
                let Some(p) = self.pop(v) else {
                    self.halt(v);
                    return false;
                };
                self.v[v].sp = p;
                self.t += 54;
            }
            0x83 => {
                let c = self.next(hw, v);
                let _ = self.next(hw, v);
                let sp = self.v[v].sp;
                if self.v[v].depth + 2 > STACK {
                    self.halt(v);
                    return false;
                }
                self.push(v, sp);
                self.push(v, c as u32);
                self.t += 92;
            }
            0x84 => {
                let (Some(c), Some(p)) = (self.pop(v), self.pop(v)) else {
                    self.halt(v);
                    return false;
                };
                let c = c.wrapping_sub(1);
                self.t += 75;
                if c != 0 {
                    self.v[v].sp = p;
                    self.push(v, p);
                    self.push(v, c);
                    self.t += 54;
                }
            }
            0x85 => {
                let i = self.next(hw, v);
                let j = self.next(hw, v);
                self.v[v].w1 = self.obj(hw, i);
                self.v[v].w2 = self.obj(hw, j);
                self.t += 386;
            }
            0x86 => {
                let i = self.next(hw, v);
                let o = self.obj(hw, i);
                let vo = &mut self.v[v];
                vo.venv = o;
                vo.vpos = 0;
                vo.vcnt = 1;
                self.t += 232;
            }
            0x87 => {
                let i = self.next(hw, v);
                let o = self.obj(hw, i);
                let vo = &mut self.v[v];
                vo.penv = o;
                vo.ppos = 0;
                vo.pcnt = 1;
                self.t += 232;
            }
            0x88 => {
                let i = self.next(hw, v);
                let m = self.next(hw, v);
                let o = self.obj(hw, i);
                let prev = self.v[v].mode;
                {
                    let vo = &mut self.v[v];
                    vo.seq = o;
                    vo.spos = 0;
                    vo.scnt = 1;
                    vo.mode = m;
                }
                let tc = self.t;
                if m & 0x20 == 0 {
                    {
                        let vo = &mut self.v[v];
                        vo.phase = m & 0x1F;
                        vo.sphase = m & 0x1F;
                    }
                    if prev & 0x20 != 0 {
                        let vo = &mut self.v[v];
                        vo.speed = 0;
                        vo.sphase = 0;
                        let buf = vo.buf;
                        self.t = tc + 384;
                        self.at(hw);
                        hw.loc(v, buf);
                        self.t = tc + 400;
                        self.at(hw);
                        hw.len(v, 0x10);
                        self.t = tc + 426;
                    } else {
                        self.t = tc + 370;
                    }
                } else {
                    self.t = tc + 305;
                    self.at(hw);
                    hw.len(v, 0x40);
                    self.t = tc + 315;
                }
            }
            0x89 => return self.cmd_portamento(hw, v),
            0x8A => {
                let t = self.next(hw, v);
                self.v[v].trans = t;
                self.t += 26;
            }
            0x8B => {
                let i = self.next(hw, v);
                self.v[v].sp = self.obj(hw, i);
                self.jumps = (self.jumps << 1) | 1;
                self.t += 232;
            }
            0x8C => {
                let f = self.next(hw, v);
                self.v[v].flags = f;
                self.t += 26;
            }
            0x8D => {
                let k = self.next(hw, v);
                self.v[v].mask = k;
                self.t += 26;
            }
            _ => {
                // undefined command: the reference jumps to an arbitrary address
                self.halt(v);
                return false;
            }
        }
        true
    }

    fn cmd_portamento(&mut self, hw: &mut Hw, v: usize) -> bool {
        let n1 = self.next(hw, v);
        let n2 = self.next(hw, v);
        let len = self.next(hw, v);
        let tr = self.v[v].trans;
        let (Some(p1), Some(p2)) = (period_at(n1.wrapping_add(tr)), period_at(n2.wrapping_add(tr))) else {
            self.halt(v);
            return false;
        };
        let diff = p2.wrapping_sub(p1) as i16;
        let down = diff < 0;
        let diff = diff.unsigned_abs() as u32;
        if len == 0 || diff == 0 {
            self.halt(v);
            return false;
        }
        let mut step = (diff / len as u32) as u8;
        let step0 = step == 0;
        if step0 {
            step = 1;
        }
        let mut int = (len as u32 / diff) as u8;
        let int0 = int == 0;
        if int0 {
            int = 1;
        }
        {
            let vo = &mut self.v[v];
            vo.per = p1;
            vo.pstep = if down { 0x80 } else { 0 } | step;
            vo.pint = int;
            vo.picd = 1;
            vo.plen = len;
        }
        self.t += 227 + if down { 12 } else { 0 };
        self.t += 163 + if step0 { 8 } else { 0 };
        self.t += 185 + if int0 { 16 } else { 0 };
        self.t += 38;
        self.tail(v, len);
        false
    }

    // ---------------------------------------------------------------- modes

    /// The wave sequence step of a mode (`kind` 0 = A, 1 = B, 2 = C, 3 = D).
    fn wave_seq(&mut self, hw: &Hw, v: usize, kind: u8) {
        self.t += 25;
        let mut vo = self.v[v];
        vo.scnt = vo.scnt.wrapping_sub(1);
        if vo.scnt == 0 {
            let mut guard = 0;
            loop {
                guard += 1;
                if guard > 1024 {
                    self.v[v] = vo;
                    self.halt(v);
                    return;
                }
                self.t += if kind == 0 { 69 } else { 67 };
                let a = vo.seq.wrapping_add(vo.spos as u32);
                let b0 = self.rd8(hw, a);
                let marker = if kind == 0 { b0 & 0x80 != 0 } else { b0 == 0xFF };
                if marker {
                    self.t += 30;
                    vo.spos = self.rd8(hw, a.wrapping_add(1));
                    continue;
                }
                let b1 = self.rd8(hw, a.wrapping_add(1));
                match kind {
                    0 | 1 => {
                        vo.speed = b0;
                        vo.scnt = b1;
                        vo.spos = vo.spos.wrapping_add(2);
                        self.t += if kind == 0 { 48 } else { 52 };
                    }
                    2 => {
                        vo.speed = b0;
                        vo.cstep = b1;
                        vo.scnt = self.rd8(hw, a.wrapping_add(2));
                        vo.spos = vo.spos.wrapping_add(3);
                        self.t += 72;
                    }
                    _ => {
                        vo.w1 = vo.w2.wrapping_add(((b0 as u32) << 8) | b1 as u32);
                        vo.mode &= !2;
                        vo.scnt = self.rd8(hw, a.wrapping_add(2));
                        vo.spos = vo.spos.wrapping_add(3);
                        self.t += 136;
                    }
                }
                break;
            }
        }
        self.v[v] = vo;
    }

    /// Writes the buffer half just filled to AUDxLC and flips; returns (previous half address,
    /// the half to fill).
    fn flip(&mut self, hw: &mut Hw, v: usize) -> (u32, u32) {
        let vo = &mut self.v[v];
        let prev = vo.buf.wrapping_add(vo.half);
        vo.half ^= 0x20;
        let target = vo.buf.wrapping_add(vo.half);
        self.at(hw);
        hw.loc(v, prev);
        (prev, target)
    }

    fn wave_byte(&self, hw: &Hw, w: u32, i: u32) -> u8 {
        self.rd8(hw, w.wrapping_add(i))
    }

    fn mode_a(&mut self, hw: &mut Hw, v: usize) {
        self.wave_seq(hw, v, 0);
        {
            let vo = &mut self.v[v];
            vo.phase = vo.phase.wrapping_add(vo.speed) & 31;
        }
        self.t += 114;
        let (_prev, target) = self.flip(hw, v);
        self.t += 184;
        let vo = self.v[v];
        for i in 0..32u32 {
            let a = self.wave_byte(hw, vo.w1, i) as i8 as i32;
            let b = self.wave_byte(hw, vo.w2, (vo.phase as u32 + i) & 31) as i8 as i32;
            let s = ((a + b) >> 1) as u8;
            let neg = s & 0x80 != 0;
            let out = if neg { 0u8.wrapping_sub(0u8.wrapping_sub(s) | vo.mask) } else { s | vo.mask };
            self.t += if neg { 105 } else { 87 };
            hw.bus.w8(target.wrapping_add(i), out);
        }
        self.t += 20 + 16;
    }

    /// The window of modes B and C: (start, end, inside at 0, wraps).
    fn window(&self, v: usize) -> (u32, u32, bool) {
        let vo = &self.v[v];
        let start = vo.phase as u32;
        let mut end = start + (vo.speed & 0x1F) as u32;
        let inside = end >= 32;
        if inside {
            end -= 32;
        }
        (start, end, inside)
    }

    fn copy_wave1(&mut self, hw: &mut Hw, v: usize, target: u32) {
        let w1 = self.v[v].w1;
        for i in 0..32u32 {
            let b = self.wave_byte(hw, w1, i);
            hw.bus.w8(target.wrapping_add(i), b);
        }
    }

    fn mode_b(&mut self, hw: &mut Hw, v: usize) {
        self.wave_seq(hw, v, 1);
        self.t += 82;
        let (prev, target) = self.flip(hw, v);
        self.t += 83;
        let vo = self.v[v];
        if vo.speed & 0x80 != 0 {
            self.copy_wave1(hw, v, target);
            self.t += 740;
            return;
        }
        let (start, end, mut inside) = self.window(v);
        self.t += 75 + if (vo.phase as u32 + (vo.speed & 0x1F) as u32) >= 32 { 16 } else { 0 } + 4;
        for i in 0..32u32 {
            let mut toggles = 0;
            if i == start {
                inside = !inside;
                toggles += 1;
            }
            if i == end {
                inside = !inside;
                toggles += 1;
            }
            self.t += 90 + 8 * toggles + if inside { 24 } else { 0 };
            let mut out = hw.bus.r8(prev.wrapping_add(i));
            if inside {
                out ^= self.wave_byte(hw, vo.w2, i) & vo.mask;
            }
            hw.bus.w8(target.wrapping_add(i), out);
        }
        {
            let vo = &mut self.v[v];
            vo.phase = vo.speed.wrapping_add(vo.phase) & 31;
        }
        self.t += 60;
    }

    fn mode_c(&mut self, hw: &mut Hw, v: usize) {
        self.wave_seq(hw, v, 2);
        self.t += 82;
        let (prev, target) = self.flip(hw, v);
        self.t += 99;
        let vo = self.v[v];
        if vo.speed == 0x80 {
            self.copy_wave1(hw, v, target);
            self.t += 740;
            return;
        }
        let to_w1 = vo.speed & 0xC0 == 0x40;
        let morph = if to_w1 { vo.w1 } else { vo.w2 };
        let (start, end, mut inside) = self.window(v);
        let wraps = (vo.phase as u32 + (vo.speed & 0x1F) as u32) >= 32;
        self.t += 41 + if to_w1 { 12 } else { 0 } + 59 + if wraps { 16 } else { 0 } + 4;
        let step = vo.cstep as i32;
        for i in 0..32u32 {
            let mut toggles = 0;
            if i == start {
                inside = !inside;
                toggles += 1;
            }
            if i == end {
                inside = !inside;
                toggles += 1;
            }
            self.t += 90 + 8 * toggles;
            let mut out = hw.bus.r8(prev.wrapping_add(i));
            if inside {
                self.t += 37;
                let c = (out ^ 0x80) as i32;
                let t = (self.wave_byte(hw, morph, i) ^ 0x80) as i32;
                let reached = (t - c).abs() <= step;
                let n = if reached {
                    t
                } else if t < c {
                    c - step
                } else {
                    c + step
                };
                self.t += match (t < c, reached) {
                    (true, true) => 35,
                    (true, false) => 39,
                    (false, true) => 31,
                    (false, false) => 25,
                };
                out = (n as u8) ^ 0x80;
            }
            hw.bus.w8(target.wrapping_add(i), out);
        }
        let adv = vo.speed & 0xC0 != 0xC0;
        if adv {
            let vo = &mut self.v[v];
            vo.phase = vo.speed.wrapping_add(vo.phase) & 31;
        }
        self.t += 41 + if adv { 32 } else { 0 } + 16;
    }

    fn mode_d(&mut self, hw: &mut Hw, v: usize) {
        self.wave_seq(hw, v, 3);
        self.t += 25;
        let vo = &mut self.v[v];
        // the step is stored in the wave speed (high byte) and start phase (low byte) fields
        let step = if vo.mode & 1 == 0 {
            self.t += 32;
            0x0080
        } else {
            self.t += 504;
            let reference = period_at(vo.mask).unwrap_or(0) as u32;
            if reference == 0 {
                0x0080
            } else {
                (128 * vo.per as u32 / reference) as u16
            }
        };
        vo.speed = (step >> 8) as u8;
        vo.sphase = step as u8;
    }

    // ---------------------------------------------------------------- audio interrupt

    fn handler(&mut self, ch: usize, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        // a request an earlier handler of the round cleared is not pending any more
        hw.bus.paula.sync(t0, &hw.bus.ram);
        if hw.bus.paula.intreq & (0x80 << ch) == 0 {
            return;
        }
        let at = |hw: &mut Hw, c: u64| hw.bus.clock = t0 + c / 2;
        if self.v[ch].mode & 0x20 == 0 {
            at(hw, 353);
            m68k::AddressBus::write_word(hw.bus, INTREQ, 0x80 << ch);
            at(hw, 369);
            return;
        }
        let zero = self.zero();
        let vo = &mut self.v[ch];
        let old = vo.w1;
        let step = (((vo.speed as u32) << 8) | vo.sphase as u32) & 0xFFFF;
        vo.w1 = vo.w1.wrapping_add(step);
        let (lc, when);
        if vo.mode & 2 != 0 {
            lc = zero;
            vo.w1 = zero;
            when = 350;
        } else {
            let len = {
                let a = vo.w2.wrapping_sub(2);
                ((mem8(hw, a) as u32) << 8) | mem8(hw, a.wrapping_add(1)) as u32
            };
            if vo.w2.wrapping_add(len) >= vo.w1 {
                lc = old;
                when = 377;
            } else {
                lc = zero;
                vo.w1 = zero;
                vo.mode |= 2;
                when = 419;
            }
        }
        at(hw, when);
        hw.loc(ch, lc);
        at(hw, when + 168);
        m68k::AddressBus::write_word(hw.bus, INTREQ, 0x80 << ch);
        at(hw, when + 184);
    }
}

/// One byte of the machine's memory as the player sees it: below 0x400 the host's bytes
/// (specification section 5), elsewhere the guest memory.
fn mem8(hw: &Hw, a: u32) -> u8 {
    let a = a & 0xFF_FFFF;
    if a < 0x400 {
        return match a {
            0..=3 => 0,
            4..=7 => [0, 0, 0x23, 0x48][a as usize - 4],
            _ => [0, 0, 4, 4][(a & 3) as usize],
        };
    }
    hw.bus.r8(a)
}

impl Replayer for Vss {
    fn format(&self) -> &'static str {
        "Voodoo Supreme Synthesizer"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.first_tick.take()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsub.saturating_sub(1), self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = if n < self.nsub { n } else { 0 };
        self.init(hw, n);
        let l0 = hw.bus.clock;
        self.start_writes(hw, l0);
        self.first_tick = Some(l0 + TICK);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.tick_voices(hw);
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if ch < 4 {
            self.handler(ch, hw);
            return true;
        }
        false
    }
}

#[cfg(test)]
#[path = "voodoosupremesynthesizer_tests.rs"]
mod tests;
