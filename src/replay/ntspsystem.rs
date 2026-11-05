//! NTSP-system (magic `SPNT`), a compressed sample stream played through two buffers.
//!
//! Clean-room, written from a behavioural specification and black-box runs of the 68k
//! player on synthetic modules; no original code and no bytes of it. There is no real
//! module in the archive, so everything here is checked against synthetic ones only.
//!
//! A module is the magic, the big-endian length N of the decoded stream, then blocks of
//! five bytes that each decode to nine signed 8-bit samples. The player decodes 1800
//! bytes at a time into one of two alternating buffers and plays the same buffer on all
//! four channels (period 161, volumes 64, 64, 1, 1). The audio-0 interrupt, once a
//! buffer, decodes the next chunk into the buffer that is not playing and repoints the
//! channels at it. At the end of the stream the rest of the buffer is cleared, the song
//! end is reported and the stream starts again.
//!
//! Detection is by the magic only (a length under 16 is refused, as the player does): the
//! file name prefix is shared with another format. Choices where the specification says
//! the original misbehaves: a length under 1800 is taken, the first chunk simply being a
//! partial one (end of stream follows at once), and bytes the stream asks for beyond the
//! end of the file read as zero. The stop actions are from the player's description only
//! and were never observed; the engine's own stop does the DMA and interrupt part.

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "NTSP-system", detect, new };

/// A buffer, in bytes.
const BUF: u32 = 1800;
const PERIOD: u16 = 161;
const MIN_N: u32 = 16;

/// Colour clocks from the host's start phase to the first location write, without the
/// shift-0 blocks of the first chunk (measured on the reference's runs).
const START_EXTRA: u64 = 51749 - 100;
/// First location write to the DMA enable.
const INIT_LEAD: u64 = 211;
/// Handler cost from the INTREQ write to the first pointer write.
const B_FULL: u64 = 51202;
const B_CLEAR: u64 = 20011;

fn detect(d: &[u8]) -> bool {
    d.len() >= 8 && &d[..4] == b"SPNT" && be32(d, 4) >= MIN_N
}

struct Ntsp {
    d: Vec<u8>,
    n: u32,
    bufs: [u32; 2],
    /// Compressed read position and decoded bytes still to produce.
    pos: usize,
    lt: u32,
    /// The buffer the next interrupt fills.
    next: usize,
    songend: bool,
}

/// Decodes one block into nine bytes; `b` holds the five bytes.
fn block(b: [u8; 5]) -> [u8; 9] {
    let s = b[0] >> 5;
    let v = ((b[0] << 3) as i8 >> 3) as i32;
    let mut y = [0u8; 9];
    y[0] = (v << s.saturating_sub(1)) as u8;
    for i in 0..8 {
        let byte = b[1 + i / 2];
        let nib = if i % 2 == 0 { byte >> 4 } else { byte & 15 };
        let u = ((nib << 4) as i8 >> 4) as i32;
        y[1 + i] = (u << s) as u8;
    }
    y
}

impl Ntsp {
    fn byte(&self, p: usize) -> u8 {
        self.d.get(p).copied().unwrap_or(0)
    }

    /// Decodes `n` bytes' worth of blocks from the read position into `dst`; the bytes of a
    /// tail that is not a multiple of nine are left as they were. Returns the blocks decoded
    /// and how many of them had shift 0.
    fn decode(&mut self, ram: &mut [u8], dst: usize, n: u32) -> (u64, u64) {
        let blocks = (n / 9) as usize;
        let mut n0 = 0;
        for k in 0..blocks {
            let b = std::array::from_fn(|i| self.byte(self.pos + 5 * k + i));
            if b[0] >> 5 == 0 {
                n0 += 1;
            }
            if let Some(out) = ram.get_mut(dst + 9 * k..dst + 9 * k + 9) {
                out.copy_from_slice(&block(b));
            }
        }
        self.pos += 5 * blocks;
        (blocks as u64, n0)
    }

    /// Fills `buf` with the next chunk and returns the handler's cost B (the model of the
    /// reference's runs).
    fn chunk(&mut self, ram: &mut [u8], buf: u32) -> u64 {
        let mut off = 0u32;
        let mut want = BUF;
        let mut n0 = 0;
        let mut m = 0;
        let entry_lt = self.lt;
        let mut cleared = 0;
        while want > 0 {
            if self.lt == 0 {
                let a = (buf + off) as usize;
                if let Some(s) = ram.get_mut(a..a + want as usize) {
                    s.fill(0);
                }
                cleared = want;
                self.lt = self.n;
                self.pos = 8;
                self.songend = true;
                break;
            }
            let d1 = want.min(self.lt);
            let (b, z) = self.decode(ram, (buf + off) as usize, d1);
            m += b;
            n0 += z;
            self.lt -= d1;
            off += d1;
            want -= d1;
        }
        if cleared == 0 {
            B_FULL + 2 * n0
        } else if entry_lt == 0 {
            B_CLEAR
        } else {
            let c = if m == 0 { 193 } else { 207 + 509 * m / 2 + 2 * n0 };
            211 + 11 * cleared as u64 + c
        }
    }

    fn handler(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        hw.bus.paula.sync(t0, &hw.bus.ram);
        if hw.bus.paula.intena & hw.bus.paula.intreq & 0x80 == 0 {
            return;
        }
        let w = |hw: &mut Hw, a: u32, v: u16| m68k::AddressBus::write_word(hw.bus, a, v);
        w(hw, 0xDFF09A, 0x0080);
        hw.bus.clock = t0 + 8;
        w(hw, 0xDFF09C, 0x0080);
        let buf = self.bufs[self.next];
        let b = self.chunk(&mut hw.bus.ram, buf);
        self.next ^= 1;
        let t = t0 + 8 + b;
        for ch in 0..4 {
            hw.bus.clock = t + 8 * ch as u64;
            hw.loc(ch, buf);
        }
        hw.bus.clock = t + 40;
        w(hw, 0xDFF09A, 0x8080);
    }
}

impl Replayer for Ntsp {
    fn format(&self) -> &'static str {
        "NTSP-system"
    }

    fn title(&self) -> String {
        String::new()
    }

    /// There is no tick: the audio interrupt does everything.
    fn tick(&mut self, _hw: &mut Hw) {}

    fn songend(&self) -> bool {
        self.songend
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if ch != 0 {
            return false;
        }
        self.handler(hw);
        true
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let buf = m.alloc(2 * BUF);
    if buf == 0 {
        return None;
    }
    let n = be32(d, 4);
    let mut p = Ntsp {
        d: d.to_vec(),
        n,
        bufs: [buf, buf + BUF],
        pos: 8,
        lt: n,
        next: 1,
        songend: false,
    };
    // the first chunk is decoded before the hardware is set up; a stream that ends inside
    // it (a length under 1800, outside the specification) is cut short and its song end
    // is left to the first interrupt
    let first = n.min(BUF);
    let (_, n0) = p.decode(&mut m.bus.ram, buf as usize, first);
    p.lt -= first;
    let t = m.bus.clock + START_EXTRA + 2 * n0;
    let mut hw = Hw { bus: &mut m.bus };
    let at = |hw: &mut Hw, off: u64| hw.bus.clock = t + off;
    for ch in 0..4 {
        at(&mut hw, 8 * ch as u64);
        hw.loc(ch, buf);
    }
    for (ch, off) in [42u64, 48, 54, 60].into_iter().enumerate() {
        at(&mut hw, off);
        hw.len(ch, (BUF / 2) as u16);
    }
    for (ch, off) in [70u64, 76, 82, 88].into_iter().enumerate() {
        at(&mut hw, off);
        hw.per(ch, PERIOD);
    }
    for (ch, (off, v)) in [(96u64, 64u16), (102, 64), (110, 1), (116, 1)].into_iter().enumerate() {
        at(&mut hw, off);
        hw.vol(ch, v);
    }
    at(&mut hw, 201);
    m68k::AddressBus::write_word(hw.bus, 0xDFF09A, 0x8080);
    at(&mut hw, INIT_LEAD);
    hw.dmacon(0x800F);
    Some(Box::new(p))
}

#[cfg(test)]
#[path = "ntspsystem_tests.rs"]
mod tests;
