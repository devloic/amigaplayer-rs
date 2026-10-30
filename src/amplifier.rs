//! The Eagleplayer amplifier, "8 Bit Amplifier compatible": up to 32 virtual
//! Paula voices mixed in software into two of Paula's channels, as
//! Eagleplayer's "8 Bit Amplifier" engine does on an Amiga.
//!
//! A player asks for it in EP_InitAmplifier (see eagle.rs). One with
//! EPAMB_Direct sends its voices through ENPP_Poke* and ENPP_DMAMask, which go
//! to virtual voices that behave as Paula's channels: LOC and LEN are latched
//! when DMA starts and again at the end of each block, period and volume take
//! effect at once, a DMA off and on restarts the voice, each at the colour
//! clock of its poke. One without leaves its voices in its AudioStructs, which
//! the mixer reads and updates (`aus`) a buffer at a time; its pokes, if any
//! (Oktalyzer pokes the channels it does not split), go into the AudioStructs
//! as Eagleplayer's ENPP_Poke* put them there: LOC and LEN to the loop, DMA on
//! makes the loop the part to play. The host mixes only for players without
//! EPAMB_Direct or with more than 4 voices; the others write Paula as before.
//!
//! The mixer is written from Paula's behaviour and the Eagleplayer manual, not
//! from the engine's code: each voice holds a byte for `period` colour clocks
//! (Paula's sample and hold, no interpolation), taken at the mix rate; the
//! voices of each side are summed with their volumes, scaled by the boost over
//! the number of voices on the busier side and clipped to 8 bits. The two
//! 8-bit streams go to guest memory, double-buffered, and Paula's channels 0
//! (left) and 1 (right) play them by DMA at the mix rate, so that Paula's
//! output stage and filters shape them as on the Amiga. Channel 0's audio
//! interrupt (a buffer started) has the host mix the next one: the emulated
//! time just passed, with each poke applied at the output sample of its colour
//! clock, so the output lags the pokes by two buffers. All in integers.
//!
//! The defaults are the 8 Bit Amplifier's factory settings on a 68020 for the
//! module's voice count n: mix rate 28000 Hz less 500 Hz for each voice past
//! 2, boost 1.0 and 0.1 more for every 4 voices, stereo (the voices' sides come
//! from their AudioStructs, Amiga LRRL where those give none).

use crate::bus::Bus;
use crate::machine::Machine;
use crate::paula::PAULA_CLOCK_PAL;
use std::collections::VecDeque;

pub const MAX_VOICES: usize = 32;
/// Where the write log puts a virtual voice's pokes: `LOG_BASE | voice << 4 |
/// register`, the registers at their Paula offsets (0/2 LOC, 4 LEN, 6 PER,
/// 8 VOL) and DMA at $A ($8000 on, 0 off).
pub const LOG_BASE: u32 = 0xD0_0000;
/// Bytes in each half of a channel's double buffer.
const BUF: u32 = 512;

/// The AudioStruct, 100 bytes a voice, as players fill it and the amplifier
/// engines read it (recovered from the players' InitAmplifier and play
/// routines and from what the engines read and write). Lengths and positions
/// count samples.
pub mod aus {
    pub const SIZE: u32 = 100;
    /// Start of the part playing (0: play the loop).
    pub const ADR: u32 = 0x00;
    /// Samples of the part played, and the fraction of the next (1/65536).
    pub const POS: u32 = 0x04;
    pub const FRAC: u32 = 0x08;
    /// The mixer's: bit 0 playing backwards, bit 1 in the loop.
    pub const STATE: u32 = 0x0A;
    pub const LEN: u32 = 0x0C;
    /// Paula period, times 4.
    pub const PER: u32 = 0x10;
    pub const LOOP_ADR: u32 = 0x12;
    pub const LOOP_LEN: u32 = 0x16;
    /// 0-64, and again (for the right side, by the pokes).
    pub const VOL: u32 = 0x1A;
    /// Non-zero: the voice plays (the pokes write a word, -1 or 0).
    pub const ON: u32 = 0x1E;
    /// Positive: left, negative: right, 0: not played. Read at the start.
    pub const PAN: u32 = 0x20;
    /// 0: no loop, bit 2: ping-pong, else forward.
    pub const LOOP: u32 = 0x22;
    /// Set by the mixer when a part without a loop has ended.
    pub const ENDED: u32 = 0x23;
    /// An Interrupt to Cause() when the voice goes round its loop
    /// (EPAMB_AudioInts).
    pub const INT: u32 = 0x2C;
}
const BACK: u8 = 1;
const IN_LOOP: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poke {
    Loc(u32),
    /// LOC at any byte (Paula's is a word address; a software mixer's voice
    /// may start or loop at an odd sample).
    LocByte(u32),
    Len(u16),
    /// LEN as a byte count (a voice's block may be longer than the 65535
    /// words Paula's register counts; players that mix in software, as
    /// OctaMED SoundStudio's does, play samples that long). Latched like LEN.
    LenBytes(u32),
    Per(u16),
    Vol(u16),
    Dma(bool),
}

/// A virtual voice fed by pokes: Paula's registers and its DMA's position.
#[derive(Clone, Default)]
struct Voice {
    lc: u32,
    len: u16,
    /// A LEN poked in bytes (0: the word count).
    len_bytes: u32,
    per: u16,
    vol: u8,
    on: bool,
    /// The block playing: its start and its length in bytes.
    pt: u32,
    bytes: u32,
    /// The byte playing, and the colour clocks it has played.
    pos: u32,
    phase: u32,
}

impl Voice {
    fn poke(&mut self, p: Poke) {
        match p {
            Poke::Loc(a) => self.lc = a & 0xFF_FFFE,
            Poke::LocByte(a) => self.lc = a & 0xFF_FFFF,
            Poke::Len(l) => {
                self.len = l;
                self.len_bytes = 0;
            }
            Poke::LenBytes(b) => self.len_bytes = b.max(1),
            Poke::Per(p) => self.per = p,
            Poke::Vol(v) => self.vol = if v & 64 != 0 { 64 } else { (v & 63) as u8 },
            Poke::Dma(true) if !self.on => {
                self.on = true;
                self.latch();
                (self.pos, self.phase) = (0, 0);
            }
            Poke::Dma(true) => {}
            Poke::Dma(false) => self.on = false,
        }
    }

    /// LOC and LEN become the block to play.
    fn latch(&mut self) {
        self.pt = self.lc;
        self.bytes = if self.len_bytes != 0 {
            self.len_bytes
        } else if self.len == 0 {
            0x20000
        } else {
            2 * self.len as u32
        };
    }

    /// Plays `cc` colour clocks. Returns whether a block ended (Paula's
    /// interrupt).
    fn run(&mut self, cc: u32) -> bool {
        let per = if self.per == 0 { 0x10000 } else { self.per as u32 };
        self.phase += cc;
        if self.phase < per {
            return false;
        }
        self.pos += self.phase / per;
        self.phase %= per;
        let mut ended = false;
        while self.pos >= self.bytes {
            self.pos -= self.bytes;
            self.latch();
            ended = true;
        }
        ended
    }
}

/// An AudioStruct voice while it is mixed.
struct Part {
    adr: u32,
    pos: i64,
    len: u32,
    state: u8,
    ladr: u32,
    llen: u32,
    mode: u8,
    ended: bool,
}

impl Part {
    /// Moves `k` samples on, through the loop. Returns whether it went round
    /// the loop.
    fn step(&mut self, k: i64) -> bool {
        let mut looped = false;
        self.pos += if self.state & BACK != 0 { -k } else { k };
        loop {
            if self.state & BACK != 0 {
                if self.pos >= 0 {
                    return looped;
                }
                // back at the loop's start: forwards again
                self.pos = -self.pos - 1;
                self.state &= !BACK;
            } else {
                if self.pos < self.len as i64 {
                    return looped;
                }
                let over = self.pos - self.len as i64;
                if self.mode == 0 || self.llen == 0 {
                    self.ended = true;
                    return looped;
                }
                let ping = self.mode & 4 != 0;
                if ping && self.state & IN_LOOP != 0 {
                    // the loop's end: backwards
                    self.pos = self.len as i64 - 1 - over;
                    self.state |= BACK;
                } else {
                    // into the loop: forwards from its start, or backwards from
                    // its end for a ping-pong loop
                    (self.adr, self.len) = (self.ladr, self.llen);
                    self.state |= IN_LOOP;
                    if ping {
                        self.pos = self.len as i64 - 1 - over;
                        self.state |= BACK;
                    } else {
                        self.pos = over;
                    }
                }
            }
            looped = true;
        }
    }
}

/// What the mixer left in an AudioStruct, and the exact colour clocks into
/// the sample that its 16-bit fraction rounds: while the player leaves the
/// struct alone the mixer goes on from there.
#[derive(Clone, Default)]
struct Held {
    left: Option<(u32, u32, u16)>,
    phase: u32,
}

pub struct Amplifier {
    pub n: usize,
    /// The player's AudioStructs (0: none).
    structs: u32,
    flags: u32,
    /// The voices are poked (EPAMB_Direct), not left in the AudioStructs.
    direct: bool,
    voices: Vec<Voice>,
    held: Vec<Held>,
    /// Each voice's side: 1 left, -1 right, 0 not played.
    pans: Vec<i8>,
    /// Voices on the busier side.
    side: i64,
    /// Pokes not mixed yet, with their colour clocks.
    queue: VecDeque<(u64, usize, Poke)>,
    /// Guest memory: channel 0's two halves, then channel 1's.
    pub buf: u32,
    /// Paula period of the mix rate.
    pub period: u16,
    /// Boost in tenths.
    pub boost: u32,
    /// The mix wraps round at 8 bits, as a NotePlayer genie's does, where it would clip.
    pub wrap: bool,
    /// The half channel 0's LOC points to.
    loc_half: u32,
    /// The emulated time mixed so far.
    mixed_to: u64,
}

fn r8(ram: &[u8], a: u32) -> u8 {
    ram.get((a & 0xFF_FFFF) as usize).copied().unwrap_or(0)
}
fn r16(ram: &[u8], a: u32) -> u16 {
    (r8(ram, a) as u16) << 8 | r8(ram, a.wrapping_add(1)) as u16
}
fn r32(ram: &[u8], a: u32) -> u32 {
    (r16(ram, a) as u32) << 16 | r16(ram, a.wrapping_add(2)) as u32
}
fn w8(ram: &mut [u8], a: u32, v: u8) {
    if let Some(m) = ram.get_mut((a & 0xFF_FFFF) as usize) {
        *m = v;
    }
}
fn w16(ram: &mut [u8], a: u32, v: u16) {
    w8(ram, a, (v >> 8) as u8);
    w8(ram, a.wrapping_add(1), v as u8);
}
fn w32(ram: &mut [u8], a: u32, v: u32) {
    w16(ram, a, (v >> 16) as u16);
    w16(ram, a.wrapping_add(2), v as u16);
}

impl Amplifier {
    /// `n` voices, with the factory defaults for `n`; `pans` as in the
    /// AudioStructs (missing or 0: LRRL for poked voices).
    pub fn new(n: usize, structs: u32, flags: u32, pans: &[i16]) -> Self {
        let n = n.clamp(1, MAX_VOICES);
        let direct = flags & crate::eagle::epamb::DIRECT != 0;
        let lrrl = |v: usize| if matches!(v % 4, 0 | 3) { 1 } else { -1 };
        let pans: Vec<i8> = (0..n)
            .map(|v| match pans.get(v).copied().unwrap_or(0) {
                0 if direct => lrrl(v),
                p => p.signum() as i8,
            })
            .collect();
        let side = pans.iter().filter(|&&p| p > 0).count().max(pans.iter().filter(|&&p| p < 0).count()).max(1);
        let rate = 28000 - 500 * (n.max(2) as u64 - 2);
        let clock = PAULA_CLOCK_PAL as u64;
        Amplifier {
            n,
            structs,
            flags,
            direct,
            voices: vec![Voice::default(); n],
            held: vec![Held::default(); n],
            pans,
            side: side as i64,
            queue: VecDeque::new(),
            buf: 0,
            period: ((clock + rate / 2) / rate) as u16,
            boost: 10 + (n as u32 - 1) / 4,
            wrap: false,
            loc_half: 0,
            mixed_to: 0,
        }
    }

    /// The mix rate Paula plays, in Hz.
    pub fn rate(&self) -> f64 {
        PAULA_CLOCK_PAL / self.period as f64
    }

    /// The colour clocks one buffer plays.
    fn buffer_clocks(&self) -> u64 {
        BUF as u64 * self.period as u64
    }

    /// A poke for voice `v` at colour clock `t`, logged when `log` is on.
    pub fn poke(&mut self, log: &mut Option<Vec<(u32, u16, u64)>>, ram: &mut [u8], t: u64, v: usize, p: Poke) {
        if v >= self.n {
            log::debug!("amplifier: voice {v} of {}", self.n);
            return;
        }
        if let Some(l) = log {
            let a = LOG_BASE | (v as u32) << 4;
            match p {
                Poke::Loc(x) => {
                    l.push((a, (x >> 16) as u16, t));
                    l.push((a + 2, x as u16, t));
                }
                Poke::Len(x) => l.push((a + 4, x, t)),
                Poke::LocByte(x) => {
                    l.push((a, (x >> 16) as u16, t));
                    l.push((a + 2, x as u16, t));
                }
                Poke::LenBytes(x) => l.push((a + 4, (x / 2).min(0xFFFF) as u16, t)),
                Poke::Per(x) => l.push((a + 6, x, t)),
                Poke::Vol(x) => l.push((a + 8, x, t)),
                Poke::Dma(on) => l.push((a + 0xA, if on { 0x8000 } else { 0 }, t)),
            }
        }
        if self.direct {
            self.queue.push_back((t, v, p));
            return;
        }
        let s = self.structs + v as u32 * aus::SIZE;
        match p {
            Poke::Loc(a) | Poke::LocByte(a) => w32(ram, s + aus::LOOP_ADR, a),
            Poke::Len(l) => {
                w32(ram, s + aus::LOOP_LEN, 2 * l as u32);
                w8(ram, s + aus::LOOP, (l != 0) as u8);
            }
            Poke::LenBytes(b) => {
                w32(ram, s + aus::LOOP_LEN, b);
                w8(ram, s + aus::LOOP, (b != 0) as u8);
            }
            Poke::Per(p) => w16(ram, s + aus::PER, p << 2),
            Poke::Vol(x) => {
                w16(ram, s + aus::VOL, x.min(64));
                w16(ram, s + aus::VOL + 2, x.min(64));
            }
            Poke::Dma(on) => {
                if on && r16(ram, s + aus::ON) == 0 {
                    w32(ram, s + aus::ADR, r32(ram, s + aus::LOOP_ADR));
                    w32(ram, s + aus::POS, 0);
                    w32(ram, s + aus::FRAC, 0);
                    w32(ram, s + aus::LEN, r32(ram, s + aus::LOOP_LEN));
                    w8(ram, s + aus::ENDED, 0);
                }
                w16(ram, s + aus::ON, if on { 0xFFFF } else { 0 });
            }
        }
    }

    /// Every voice's DMA off now (the host stopping the song).
    pub fn all_off(&mut self) {
        while let Some((_, v, p)) = self.queue.pop_front() {
            self.voices[v].poke(p);
        }
        for v in &mut self.voices {
            v.on = false;
        }
    }

    /// The Interrupt to Cause() when voice `v` goes round its loop, if any.
    fn int(&self, ram: &[u8], v: usize) -> u32 {
        if self.flags & crate::eagle::epamb::AUDIO_INTS == 0 || self.structs == 0 {
            return 0;
        }
        r32(ram, self.structs + v as u32 * aus::SIZE + aus::INT)
    }

    /// Mixes the `left.len()` output samples from `mixed_to` on into `left`
    /// and `right`; `ints` gets the Interrupts to Cause().
    fn mix(&mut self, ram: &mut [u8], left: &mut [i8], right: &mut [i8], ints: &mut Vec<u32>) {
        let mp = self.period as u32;
        let n = left.len();
        let mut acc = vec![[0i32; 2]; n];
        if self.direct {
            for (i, a) in acc.iter_mut().enumerate() {
                let t = self.mixed_to + i as u64 * mp as u64;
                while let Some(&(at, v, p)) = self.queue.front() {
                    if at > t {
                        break;
                    }
                    self.queue.pop_front();
                    self.voices[v].poke(p);
                }
                for v in 0..self.n {
                    let voice = &mut self.voices[v];
                    if !voice.on {
                        continue;
                    }
                    let s = r8(ram, voice.pt.wrapping_add(voice.pos)) as i8 as i32 * voice.vol as i32;
                    match self.pans[v] {
                        1 => a[0] += s,
                        -1 => a[1] += s,
                        _ => {}
                    }
                    if voice.run(mp) {
                        let is = self.int(ram, v);
                        if is != 0 {
                            ints.push(is);
                        }
                    }
                }
            }
        } else {
            for v in 0..self.n {
                self.mix_struct(ram, v, &mut acc, ints);
            }
        }
        let div = 64 * 10 * self.side;
        for (i, a) in acc.iter().enumerate() {
            let wrap = self.wrap;
            let o = |x: i32| {
                let y = x as i64 * self.boost as i64 / div;
                if wrap { y as i8 } else { y.clamp(-128, 127) as i8 }
            };
            (left[i], right[i]) = (o(a[0]), o(a[1]));
        }
        self.mixed_to += n as u64 * mp as u64;
    }

    /// Mixes AudioStruct `v` into `acc` and leaves its new position there.
    fn mix_struct(&mut self, ram: &mut [u8], v: usize, acc: &mut [[i32; 2]], ints: &mut Vec<u32>) {
        let s = self.structs + v as u32 * aus::SIZE;
        let side = match self.pans[v] {
            1 => 0,
            -1 => 1,
            _ => return,
        };
        if r8(ram, s + aus::ON) == 0 || r8(ram, s + aus::ENDED) != 0 {
            return;
        }
        let mut p = Part {
            adr: r32(ram, s + aus::ADR),
            pos: r32(ram, s + aus::POS) as i64,
            len: r32(ram, s + aus::LEN),
            state: r8(ram, s + aus::STATE),
            ladr: r32(ram, s + aus::LOOP_ADR),
            llen: r32(ram, s + aus::LOOP_LEN),
            mode: r8(ram, s + aus::LOOP),
            ended: false,
        };
        let frac = r16(ram, s + aus::FRAC);
        if p.adr == 0 {
            // no part of its own: the loop
            (p.adr, p.len, p.pos, p.state) = (p.ladr, p.llen, 0, IN_LOOP);
            if p.adr == 0 || p.len == 0 {
                return;
            }
        }
        // in quarter colour clocks
        let per = match r16(ram, s + aus::PER) {
            0 => return,
            x => x as u32,
        };
        let vol = r16(ram, s + aus::VOL).min(64) as i32;
        log::trace!(
            "AudioStruct {v}: ${:06x}+{}/{} per {per} vol {vol} loop ${:06x}/{} mode {}",
            p.adr,
            p.pos,
            p.len,
            p.ladr,
            p.llen,
            p.mode
        );
        let held = &self.held[v];
        let mut phase = if held.left == Some((p.adr, p.pos as u32, frac)) {
            held.phase
        } else {
            ((frac as u32 * per) >> 16).min(per - 1)
        };
        let unsigned = self.flags & crate::eagle::epamb::BIT8 == 0;
        let int = self.int(ram, v);
        let mp = 4 * self.period as u32;
        // a part already played to its end (or empty) goes on into its loop
        p.step(0);
        for a in acc.iter_mut() {
            if p.ended {
                break;
            }
            let mut b = r8(ram, p.adr.wrapping_add(p.pos as u32));
            if unsigned {
                b ^= 0x80;
            }
            a[side] += b as i8 as i32 * vol;
            phase += mp;
            if phase >= per {
                if p.step((phase / per) as i64) && int != 0 {
                    ints.push(int);
                }
                phase %= per;
            }
        }
        if p.ended {
            w8(ram, s + aus::ENDED, 0xFF);
        }
        let pos = p.pos.max(0) as u32;
        let frac = ((phase as u64) << 16) / per as u64;
        w32(ram, s + aus::ADR, p.adr);
        w32(ram, s + aus::POS, pos);
        w16(ram, s + aus::FRAC, frac as u16);
        w8(ram, s + aus::STATE, p.state);
        w32(ram, s + aus::LEN, p.len);
        self.held[v] = Held { left: Some((p.adr, pos, frac as u16)), phase };
    }
}

/// Starts the amplifier for `n` voices: its buffers in guest memory and Paula
/// playing them.
pub fn start(m: &mut Machine, n: usize, structs: u32, flags: u32) -> bool {
    start_with(m, n, structs, flags, None, 100, None)
}

/// `start` with the boost at `gain` percent of the default's, for a player whose
/// original mixes louder than the 8 Bit Amplifier's factory settings.
pub fn start_gain(m: &mut Machine, n: usize, structs: u32, flags: u32, gain: u32) -> bool {
    start_with(m, n, structs, flags, None, gain, None)
}

/// The same for poked voices whose sides the player chooses: `pans[v]` positive
/// left, negative right, 0 the Amiga LRRL side of voice `v`; `gain` is the boost
/// in percent of the default's (100: as `start`), for a player whose mix is not
/// scaled down by the voices of a side as far as the default is.
pub fn start_panned(m: &mut Machine, n: usize, flags: u32, pans: &[i16], gain: u32) -> bool {
    start_with(m, n, 0, flags, Some(pans), gain, None)
}

/// `start_panned` for a player whose host mixes at its own rate and scale: the mix rate as a
/// Paula period and the boost in tenths are given, not the 8 Bit Amplifier's factory ones.
pub fn start_panned_mix(m: &mut Machine, n: usize, flags: u32, pans: &[i16], period: u16, boost: u32) -> bool {
    start_with(m, n, 0, flags, Some(pans), 100, Some((period, boost)))
}

fn start_with(m: &mut Machine, n: usize, structs: u32, flags: u32, sides: Option<&[i16]>, gain: u32, mix: Option<(u16, u32)>) -> bool {
    let n = n.min(MAX_VOICES);
    let pans: Vec<i16> = match sides {
        Some(p) => p.to_vec(),
        None => (0..n as u32).map(|v| if structs == 0 { 0 } else { m.bus.r16(structs + v * aus::SIZE + aus::PAN) as i16 }).collect(),
    };
    let mut a = Amplifier::new(n, structs, flags, &pans);
    a.boost = a.boost * gain / 100;
    if let Some((period, boost)) = mix {
        (a.period, a.boost, a.wrap) = (period, boost, true);
    }
    a.buf = m.alloc(4 * BUF);
    if a.buf == 0 {
        return false;
    }
    log::info!(
        "amplifier: {n} voices ({}), mix rate {:.0} Hz (period {}), boost {}.{}, sides {:?}",
        if a.direct { "poked" } else { "AudioStructs" },
        a.rate(),
        a.period,
        a.boost / 10,
        a.boost % 10,
        a.pans
    );
    m.bus.amp = Some(Box::new(a));
    restart(&mut m.bus);
    true
}

/// Paula's registers as the amplifier writes them: at the current colour clock,
/// past the write log, which keeps the voices' pokes.
fn paula(bus: &mut Bus, reg: u32, v: u16) {
    bus.paula.sync(bus.clock, &bus.ram);
    match reg {
        0x096 => bus.paula.write_dmacon(v),
        0x09A => bus.paula.write_intena(v),
        0x09C => bus.paula.write_intreq(v),
        r => bus.paula.write_audio_reg(r - 0x0A0, v),
    }
}

fn set_loc(bus: &mut Bus, ch: u32, a: u32) {
    paula(bus, 0x0A0 + 16 * ch, (a >> 16) as u16);
    paula(bus, 0x0A2 + 16 * ch, a as u16);
}

/// Starts Paula on the buffers again (silent), as at the start: after the host
/// turned DMA off. The voices go on.
pub fn restart(bus: &mut Bus) {
    let Some(a) = bus.amp.as_mut() else { return };
    let (buf, period) = (a.buf, a.period);
    a.loc_half = 0;
    // the first buffer, filled when the DMA starts, is the time before it
    a.mixed_to = bus.clock.saturating_sub(a.buffer_clocks());
    bus.write_bytes(buf, &[0; 4 * BUF as usize]);
    paula(bus, 0x096, 0x0003);
    for ch in 0..2 {
        set_loc(bus, ch, buf + 2 * BUF * ch);
        paula(bus, 0x0A4 + 16 * ch, (BUF / 2) as u16);
        paula(bus, 0x0A6 + 16 * ch, period);
        paula(bus, 0x0A8 + 16 * ch, 64);
    }
    paula(bus, 0x09C, 0x0080);
    paula(bus, 0x09A, 0x8080);
    paula(bus, 0x096, 0x8203);
}

/// Channel 0's audio interrupt: a buffer has started, so the next one is mixed
/// into the other half. Returns the Interrupts to Cause().
pub fn interrupt(bus: &mut Bus) -> Vec<u32> {
    let mut ints = Vec::new();
    let Some(a) = bus.amp.as_mut() else { return ints };
    a.loc_half ^= 1;
    let (l, r) = (a.buf + a.loc_half * BUF, a.buf + 2 * BUF + a.loc_half * BUF);
    let (mut left, mut right) = ([0i8; BUF as usize], [0i8; BUF as usize]);
    a.mix(&mut bus.ram, &mut left, &mut right, &mut ints);
    bus.write_bytes(l, &left.map(|x| x as u8));
    bus.write_bytes(r, &right.map(|x| x as u8));
    paula(bus, 0x09C, 0x0080);
    set_loc(bus, 0, l);
    set_loc(bus, 1, r);
    ints
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eagle::epamb;

    const MP: u32 = 100;

    /// An amplifier of `n` poked voices mixing at period `MP`, boost 1.0, with
    /// no clipping for one voice a side (side 1).
    fn amp(n: usize) -> Amplifier {
        let mut a = Amplifier::new(n, 0, epamb::DIRECT | epamb::BIT8, &[1, -1, 1, -1, 1, -1, 1, -1]);
        (a.period, a.boost, a.side) = (MP as u16, 10, 1);
        a
    }

    fn ram() -> Vec<u8> {
        let mut r = vec![0u8; 0x10000];
        for i in 0..16 {
            r[0x1000 + i] = (i as u8 + 1) * 4; // 4, 8, .. 64
            r[0x2000 + i] = (0x80 | i) as u8; // negative
        }
        r
    }

    /// Mixes `n` samples; the left side's output.
    fn left(a: &mut Amplifier, ram: &mut [u8], n: usize) -> Vec<i8> {
        let (mut l, mut r) = (vec![0; n], vec![0; n]);
        a.mix(ram, &mut l, &mut r, &mut Vec::new());
        l
    }

    fn start(a: &mut Amplifier, t: u64, v: usize, lc: u32, len: u16, per: u16, vol: u16) {
        let mut log = None;
        for p in [Poke::Loc(lc), Poke::Len(len), Poke::Per(per), Poke::Vol(vol), Poke::Dma(true)] {
            a.poke(&mut log, &mut [], t, v, p);
        }
    }

    #[test]
    fn sample_start_and_loop_reload() {
        let (mut a, mut r) = (amp(2), ram());
        // 4 bytes at volume 64: each byte held for 2 output samples (period 200)
        start(&mut a, 0, 0, 0x1000, 2, 200, 64);
        // the loop: LOC/LEN written while playing, latched at the block's end
        let mut log = None;
        a.poke(&mut log, &mut [], 10, 0, Poke::Loc(0x1008));
        a.poke(&mut log, &mut [], 10, 0, Poke::Len(1));
        assert_eq!(left(&mut a, &mut r, 14), [4, 4, 8, 8, 12, 12, 16, 16, 36, 36, 40, 40, 36, 36]);
    }

    #[test]
    fn period_and_volume_change_mid_sample() {
        let (mut a, mut r) = (amp(2), ram());
        start(&mut a, 0, 0, 0x1000, 8, 200, 64);
        let mut log = None;
        // from sample 4 on (colour clock 400): a byte per sample, half volume
        a.poke(&mut log, &mut [], 400, 0, Poke::Per(100));
        a.poke(&mut log, &mut [], 400, 0, Poke::Vol(32));
        assert_eq!(left(&mut a, &mut r, 8), [4, 4, 8, 8, 6, 8, 10, 12]);
    }

    #[test]
    fn dma_off_on_retriggers() {
        let (mut a, mut r) = (amp(2), ram());
        start(&mut a, 0, 0, 0x1000, 8, 100, 64);
        let mut log = None;
        // off and on again between two output samples: the sample restarts
        a.poke(&mut log, &mut [], 250, 0, Poke::Dma(false));
        a.poke(&mut log, &mut [], 250, 0, Poke::Dma(true));
        // on while on: nothing happens
        a.poke(&mut log, &mut [], 450, 0, Poke::Dma(true));
        // off: silence
        a.poke(&mut log, &mut [], 650, 0, Poke::Dma(false));
        assert_eq!(left(&mut a, &mut r, 8), [4, 8, 12, 4, 8, 12, 16, 0]);
    }

    #[test]
    fn simultaneous_pokes_mix_and_log() {
        let (mut a, mut r) = (amp(4), ram());
        a.side = 2;
        let mut log = Some(Vec::new());
        // voices 0 and 2 on the left, started by one DMA mask at the same clock
        for v in [0, 2] {
            a.poke(&mut log, &mut [], 5, v, Poke::Loc(if v == 0 { 0x1000 } else { 0x2000 }));
            a.poke(&mut log, &mut [], 5, v, Poke::Len(8));
            a.poke(&mut log, &mut [], 5, v, Poke::Per(100));
            a.poke(&mut log, &mut [], 5, v, Poke::Vol(64));
        }
        for v in [0, 2] {
            a.poke(&mut log, &mut [], 5, v, Poke::Dma(true));
        }
        // samples 4..: (4k) + (-128 + k), over 2 voices; the first at clock 0
        // is before the pokes
        let out = left(&mut a, &mut r, 4);
        assert_eq!(out, [0, -62, -59, -57]);
        let l = log.unwrap();
        assert_eq!(l[0], (LOG_BASE, 0, 5));
        assert_eq!(l[1], (LOG_BASE + 2, 0x1000, 5));
        assert_eq!(l[5], (LOG_BASE | 0x20, 0, 5));
        assert_eq!(l[l.len() - 1], (LOG_BASE | 0x2A, 0x8000, 5));
    }

    #[test]
    fn audio_struct_one_shot_and_ping_pong() {
        let mut r = ram();
        let s = 0x3000;
        let mut a = Amplifier::new(1, s, epamb::BIT8, &[1]);
        (a.period, a.boost) = (MP as u16, 10);
        let set = |r: &mut Vec<u8>, mode: u8| {
            w32(r, s + aus::ADR, 0x1000);
            w32(r, s + aus::POS, 0);
            w32(r, s + aus::FRAC, 0);
            w32(r, s + aus::LEN, 4);
            w16(r, s + aus::PER, 4 * MP as u16);
            w32(r, s + aus::LOOP_ADR, 0x1001);
            w32(r, s + aus::LOOP_LEN, 3);
            w16(r, s + aus::VOL, 64);
            w8(r, s + aus::ON, 1);
            w8(r, s + aus::LOOP, mode);
            w8(r, s + aus::ENDED, 0);
        };
        set(&mut r, 0);
        assert_eq!(left(&mut a, &mut r, 6), [4, 8, 12, 16, 0, 0]);
        assert_eq!(r8(&r, s + aus::ENDED), 0xFF);
        set(&mut r, 1);
        assert_eq!(left(&mut a, &mut r, 8), [4, 8, 12, 16, 8, 12, 16, 8]);
        // ping-pong: back from the loop's end, forwards from its start
        set(&mut r, 4);
        assert_eq!(left(&mut a, &mut r, 10), [4, 8, 12, 16, 16, 12, 8, 8, 12, 16]);
        // the position is left in the struct, and the mixer goes on from it
        assert_eq!((r32(&r, s + aus::POS), r8(&r, s + aus::STATE)), (2, BACK | IN_LOOP));
        assert_eq!(left(&mut a, &mut r, 2), [16, 12]);
    }

    /// Pokes for a player that leaves its voices in AudioStructs go there, as
    /// Eagleplayer's ENPP_Poke* put them: the sample plays from DMA on.
    #[test]
    fn audio_struct_pokes() {
        let mut r = ram();
        let s = 0x3000;
        let mut a = Amplifier::new(1, s, epamb::BIT8, &[1]);
        (a.period, a.boost) = (MP as u16, 10);
        let mut log = Some(Vec::new());
        for p in [Poke::Loc(0x1004), Poke::Len(2), Poke::Per(MP as u16), Poke::Vol(64), Poke::Dma(true)] {
            a.poke(&mut log, &mut r, 0, 0, p);
        }
        assert_eq!(r16(&r, s + aus::PER), 4 * MP as u16);
        assert_eq!(left(&mut a, &mut r, 6), [20, 24, 28, 32, 20, 24]);
        // on while on goes on; off and on restarts
        a.poke(&mut log, &mut r, 0, 0, Poke::Dma(true));
        assert_eq!(left(&mut a, &mut r, 2), [28, 32]);
        a.poke(&mut log, &mut r, 0, 0, Poke::Dma(false));
        assert_eq!(left(&mut a, &mut r, 1), [0]);
        a.poke(&mut log, &mut r, 0, 0, Poke::Dma(true));
        assert_eq!(left(&mut a, &mut r, 1), [20]);
        assert_eq!(log.unwrap().len(), 9);
    }
}
