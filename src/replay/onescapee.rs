//! onEscapee (`one.*`), Akos Divianszky's 1997 player for one long raw sample.
//!
//! Clean-room, written from a behavioural specification and black-box runs of the 68k
//! player on synthetic modules; no original code and no bytes of it. There is no real
//! module in the archive, so everything here is checked against the synthetic ones only.
//!
//! A module is a single signed 8-bit sample stream, each big-endian long of the file
//! obscured by an exclusive-or with a constant, played on all four channels at one period
//! out of two alternating 1024-byte buffers. The audio-0 interrupt, once a buffer, decodes
//! the next 1024 bytes into the buffer that is not playing and points the four channels at
//! it. When the data is used up the last buffer is zero-padded, the song end is reported and
//! the sample starts again from its first byte.
//!
//! Detection is by content only (the first 100 bytes), as the specification gives the rule.
//! Choices where the specification says the original misbehaves: a file under 1024 bytes
//! is not taken (the original's counter underflows and it plays memory past the module).
//! The stop actions (DMA off, volumes 0, interrupt masked) are from the player's source
//! only and were never observed; the engine's own stop does the DMA and interrupt part.

use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "onEscapee", detect, new };

const KEY: [u8; 4] = [0xAA, 0x55, 0xFF, 0x00];
const W_A: u32 = 0xAA55FF00;
const W_B: u32 = 0x55AA00FF;
/// A buffer, in bytes.
const BUF: u32 = 1024;
const PERIOD: u16 = 222;
/// Smallest file taken: the original needs a whole first buffer.
const MIN_SIZE: usize = 1024;

/// Colour clocks from the host's start phase to the first location write in the reference
/// runs (the 68k host's own start burst; measured).
const START_EXTRA: u64 = 7315;
/// Colour clocks the reference's check takes longer when it does not exit early (the start of the
/// run moves, the distances inside it do not; the same for every length of the leading run).
const NOT_ALL_A_EXTRA: u64 = 19;
/// Colour clocks of the start-up: the first location write to the DMA enable.
const INIT_LEAD: u64 = 211;
/// The reference's song start (where Paula's output and line grids restart) is this many colour
/// clocks after the DMA enable.
const SONG_START_LAG: u64 = 8;
/// Handler cost from its first write to the first pointer write.
const D_FULL: u64 = 6641;
const D_LAST_FULL: u64 = 6639;
const D_PARTIAL: u64 = 6641 + 120;
const D_PARTIAL_LONG: u64 = 15;
const D_CLEAR: u64 = 224 + 15 * 256;

/// The first 24 words are all the first key word (the check's early exit).
fn all_a(d: &[u8]) -> bool {
    (0..24).all(|j| be32(d, 4 * j) == W_A)
}

/// The player's own content rule on the first 25 words.
fn detect(d: &[u8]) -> bool {
    if d.len() < MIN_SIZE {
        return false;
    }
    let w = |i: usize| be32(d, 4 * i);
    let mut j = 0;
    while j < 24 && w(j) == W_A {
        j += 1;
    }
    if j == 24 {
        return true;
    }
    (j + 1..25).all(|i| w(i) == W_B)
}

struct OneEscapee {
    d: Vec<u8>,
    /// File size with the two low bits cleared.
    l: u32,
    bufs: [u32; 2],
    /// Read position and bytes still to take.
    pos: u32,
    rem: u32,
    /// The buffer the next interrupt fills.
    next: usize,
    songend: bool,
}

impl OneEscapee {
    /// Decodes `n` bytes from the read position into `dst` (the rest of the buffer is zero).
    fn fill(&mut self, ram: &mut [u8], dst: u32, n: u32) {
        let dst = dst as usize;
        if dst + BUF as usize > ram.len() {
            return;
        }
        for i in 0..n as usize {
            let p = self.pos as usize + i;
            ram[dst + i] = self.d[p] ^ KEY[p & 3];
        }
        ram[dst + n as usize..dst + BUF as usize].fill(0);
        self.pos += n;
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
        let d = if self.rem == 0 {
            hw.bus.ram[buf as usize..(buf + BUF) as usize].fill(0);
            self.rem = self.l;
            self.pos = 0;
            self.songend = true;
            D_CLEAR
        } else if self.rem >= BUF {
            let last = self.rem == BUF;
            self.fill(&mut hw.bus.ram, buf, BUF);
            self.rem -= BUF;
            if last {
                D_LAST_FULL
            } else {
                D_FULL
            }
        } else {
            let n = self.rem;
            self.fill(&mut hw.bus.ram, buf, n);
            self.rem = self.l;
            self.pos = 0;
            self.songend = true;
            D_PARTIAL + D_PARTIAL_LONG * ((BUF - n) / 4) as u64
        };
        self.next ^= 1;
        let t = t0 + d;
        for ch in 0..4 {
            hw.bus.clock = t + 8 * ch as u64;
            hw.loc(ch, buf);
        }
        hw.bus.clock = t + 40;
        w(hw, 0xDFF09A, 0x8080);
    }
}

impl Replayer for OneEscapee {
    fn format(&self) -> &'static str {
        "onEscapee"
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
    let mut p = OneEscapee {
        d: d.to_vec(),
        l: d.len() as u32 & !3,
        bufs: [buf, buf + BUF],
        pos: 0,
        rem: 0,
        next: 1,
        songend: false,
    };
    p.rem = p.l;
    p.fill(&mut m.bus.ram, buf, BUF);
    p.rem -= BUF;
    // the start-up writes, at the distances measured in the reference runs
    let t0 = m.bus.clock + START_EXTRA + if all_a(&d) { 0 } else { NOT_ALL_A_EXTRA };
    let e = t0 + INIT_LEAD;
    let mut hw = Hw { bus: &mut m.bus };
    for ch in 0..4 {
        hw.bus.clock = t0 + 8 * ch as u64;
        hw.loc(ch, buf);
    }
    for (ch, off) in [169u64, 163, 157, 151].into_iter().enumerate() {
        hw.bus.clock = e - off;
        hw.len(ch, (BUF / 2) as u16);
    }
    for (ch, off) in [141u64, 135, 129, 123].into_iter().enumerate() {
        hw.bus.clock = e - off;
        hw.per(ch, PERIOD);
    }
    for (ch, (off, v)) in [(115u64, 64u16), (109, 64), (101, 1), (95, 1)].into_iter().enumerate() {
        hw.bus.clock = e - off;
        hw.vol(ch, v);
    }
    hw.bus.clock = e - 10;
    m68k::AddressBus::write_word(hw.bus, 0xDFF09A, 0x8080);
    hw.bus.clock = e;
    hw.dmacon(0x800F);
    hw.bus.clock = e + SONG_START_LAG;
    Some(Box::new(p))
}

#[cfg(test)]
#[path = "onescapee_tests.rs"]
mod tests;
