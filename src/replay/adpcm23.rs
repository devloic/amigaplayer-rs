//! ADPCM2 / ADPCM3 (the Eagleplayer "ADPCM", ADPCM routines by Christian Buchner): one mono
//! stream of the 2-bit or 3-bit ADPCM of the ZyXEL voice modems behind a 10-byte header, played
//! as a sample streamer on Paula channels 0 and 1 (left and right, the same data).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! and DMA fetches of the 68k reference player; no original code. The only data are the six step
//! multipliers, typed in below from the specification.
//!
//! The file: `ADPCM`, `2` or `3`, the sample rate (big-endian 32 bits), then the codes to the
//! end of the file. ADPCM2 packs 4 codes to a byte, most significant pair first; ADPCM3 packs 8
//! codes in 3 bytes read as a 24-bit big-endian number, the first code in the top bits. There
//! is no step table and no index: the step is multiplied by one of six fixed factors after each
//! code, and the predictor wraps (no saturation); the output is the predictor's bits 13-6,
//! clamped to a signed byte.
//!
//! Playback: the period is `5 * E / rate` (E the E-clock), both channels play the same two
//! 1024-byte chip buffers alternately at volume 64. The first buffer is silent. The audio
//! channel 0 interrupt, once per 1024-sample block, points both channels at the buffer filled
//! for the next block and fills the other one. The tail of the stream is a zero-padded block
//! (up to two bytes of it are never played), then the song end is announced and the stream
//! restarts with a fresh decoder state.
//!
//! Recognition is strict: the first six bytes are `ADPCM2` or `ADPCM3` (and there is a byte
//! of header to the end). The headerless ADPCM mono format refuses `ADPC` files, so the two
//! never meet.
//!
//! Choices where the specification leaves the behaviour open: a rate of 0 (low 16 bits) is
//! refused at the start; a stream with no code bytes plays silence and announces the song
//! end; the bytes read past the end of the stream by the last ADPCM3 group are zeros; the
//! decode cost of the original's 68000 is not modelled (the refill takes no time), so a stream
//! whose rate is so high that the original underruns plays here without the repeated buffer.
//! The duration the original shows for ADPCM2 is wrong; nothing here reports a duration.
//! The format name carries `ADPCM`, so the engine's output stage is the PC one (no Amiga
//! filters), as for the original's run.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "ADPCM2 / ADPCM3", detect, new };

/// The step multipliers of the 2-bit codes (magnitude bit 0 clear, set).
const MUL2: [u32; 2] = [0x3800, 0x5600];
/// The step multipliers of the 3-bit codes by their magnitude 0..3.
const MUL3: [u32; 4] = [0x399A, 0x3A9F, 0x4D14, 0x6607];

/// Samples (and bytes) in a buffer.
const BUF: usize = 1024;
/// Five times the E-clock (709 379 Hz, PAL): the period is this over the rate.
const FIVE_E: u32 = 3_546_895;
/// Colour clocks from the start of the run to the first DMA write (2-bit, 3-bit codes): 3028 and
/// 3176 as the reference measures them, plus 64 and 128, which put the DMA on at the reference's
/// phase in the raster line (where the audio DMA slots fall; its run starts at that phase).
const START_AT: [u64; 2] = [3028 + 64, 3176 + 128];
/// The gaps between the start writes (colour clocks): from the DMA off to the first location, then
/// the length, period and volume of channel 0, the gap to channel 1's location, its length,
/// period and volume, and the gap to the DMA on.
const START_GAPS: [u64; 9] = [7787, 89, 95, 85, 91, 89, 95, 85, 102];
/// Colour clocks from the request of the audio interrupt to the location write.
const IRQ_LATENCY: u64 = 117;
/// The latency of the later requests, which the original's host serves while its foreground runs:
/// 117 plus 0 to 73 colour clocks (the service times fall on a grid of about 73.6 colour clocks
/// of the host's), here its mean.
const LATE_LATENCY: u64 = 153;
/// Colour clocks from the DMA on to the end of the original's start call, where the song starts
/// (2-bit, 3-bit codes).
const SONG_AT: [u64; 2] = [2281, 2296];
/// Colour clocks between the two channels' location writes.
const CH_GAP: u64 = 19;

/// The header of a file: (3-bit codes, rate), when the first ten bytes are one.
fn header(d: &[u8]) -> Option<(bool, u32)> {
    if d.len() < 10 || &d[..5] != b"ADPCM" || (d[5] != b'2' && d[5] != b'3') {
        return None;
    }
    Some((d[5] == b'3', u32::from_be_bytes([d[6], d[7], d[8], d[9]])))
}

fn detect(d: &[u8]) -> bool {
    header(d).is_some()
}

/// The playing period of a rate: the 32-by-16-bit division of the original (only the low 16 bits
/// of the rate; a quotient that does not fit leaves the low 16 bits of the dividend). None for 0.
fn period(rate: u32) -> Option<u16> {
    let r = rate & 0xFFFF;
    if r == 0 {
        return None;
    }
    let q = FIVE_E / r;
    Some(if q > 0xFFFF { FIVE_E as u16 } else { q as u16 })
}

/// The decoder state: step and predictor, both 16 bits.
#[derive(Clone, Copy, Default)]
struct Dec {
    s: u32,
    p: u16,
}

impl Dec {
    /// One code (2 or 3 bits, by `three`) to its output sample.
    fn code(&mut self, c: u32, three: bool) -> u8 {
        let s = self.s;
        let (mut d, mul, neg) = if three {
            let m = (c & 3) as usize;
            (s / 2 + m as u32 * s, MUL3[m], c & 4 != 0)
        } else {
            let b = (c & 1) as usize;
            (s / 2 + if b != 0 { s } else { 0 }, MUL2[b], c & 2 != 0)
        };
        d &= 0xFFFF;
        if neg {
            d = d.wrapping_neg() & 0xFFFF;
        }
        self.p = self.p.wrapping_add(d as u16);
        self.s = ((s * mul + 0x2000) >> 14) & 0xFFFF;
        ((self.p as i16) >> 6).clamp(-128, 127) as i8 as u8
    }
}

struct Adpcm {
    /// The code bytes (the file past its header).
    d: Vec<u8>,
    three: bool,
    /// Bytes consumed so far.
    pos: usize,
    dec: Dec,
    bufs: [u32; 2],
    next: usize,
    /// Refills so far.
    refills: u64,
    songend: bool,
    /// The song start: the first request, raised during the start call, is served this
    /// interrupt latency after it.
    start: u64,
}

impl Adpcm {
    /// Decodes `bytes` bytes from the read position (zeros past the stream) into the start of
    /// `out`, with the decoder's state kept, as one call of the original's decoder.
    fn decode(&mut self, out: &mut [u8; BUF], bytes: usize) {
        if self.dec.s == 0 {
            self.dec.s = 5;
        }
        let at = |i: usize| self.d.get(self.pos + i).copied().unwrap_or(0) as u32;
        let mut n = 0;
        if self.three {
            for g in 0..bytes / 3 {
                let v = at(3 * g) << 16 | at(3 * g + 1) << 8 | at(3 * g + 2);
                for k in 0..8 {
                    if n < BUF {
                        out[n] = self.dec.code((v >> (21 - 3 * k)) & 7, true);
                        n += 1;
                    }
                }
            }
        } else {
            for i in 0..bytes {
                let v = at(i);
                for k in 0..4 {
                    if n < BUF {
                        out[n] = self.dec.code((v >> (6 - 2 * k)) & 3, false);
                        n += 1;
                    }
                }
            }
        }
    }

    /// The stream restarts: position to the start, the decoder state reset.
    fn restart(&mut self) {
        self.pos = 0;
        self.dec = Dec::default();
    }

    /// Fills the next buffer by the rules of the specification's refill.
    fn fill(&mut self, hw: &mut Hw) {
        let block = if self.three { 384 } else { 256 };
        let mut out = [0u8; BUF];
        if self.pos >= self.d.len() {
            self.songend = true;
            self.restart();
        }
        let r = self.d.len() - self.pos;
        if r == 0 {
            // an empty stream: silence (the original decodes whatever follows the module)
        } else if r < block {
            let n = 3 * (r / 3);
            // a group is the least the 3-bit decoder does, even when no whole group of bytes is left
            self.decode(&mut out, if self.three { n.max(3) } else { n });
            self.restart();
            self.songend = true;
        } else {
            self.decode(&mut out, block);
            self.pos += block;
        }
        hw.bus.write_bytes(self.bufs[self.next], &out);
    }

    /// One refill: both channels to the next buffer, which is then filled.
    fn refill(&mut self, hw: &mut Hw, latency: u64) {
        hw.wait(latency);
        let a = self.bufs[self.next];
        hw.loc(0, a);
        hw.wait(CH_GAP);
        hw.loc(1, a);
        self.fill(hw);
        self.next ^= 1;
        self.refills += 1;
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (three, rate) = header(d)?;
    let per = period(rate)?;
    let a = m.alloc(2 * BUF as u32);
    if a == 0 {
        return None;
    }
    let mut r = Adpcm {
        d: d[10..].to_vec(),
        three,
        pos: 0,
        dec: Dec::default(),
        bufs: [a, a + BUF as u32],
        next: 1,
        refills: 0,
        songend: false,
        start: 0,
    };
    let mut hw = Hw { bus: &mut m.bus };
    hw.bus.write_bytes(a, &[0u8; 2 * BUF]);
    let start = START_AT[three as usize];
    if hw.bus.clock < start {
        hw.wait(start - hw.bus.clock);
    }
    hw.bus.write_word(0xDFF096, 0x0003);
    hw.wait(START_GAPS[0]);
    for (ch, k) in [(0usize, 0usize), (1, 4)] {
        hw.loc(ch, a);
        hw.wait(START_GAPS[k + 1]);
        hw.len(ch, (BUF / 2) as u16);
        hw.wait(START_GAPS[k + 2]);
        hw.per(ch, per);
        hw.wait(START_GAPS[k + 3]);
        hw.vol(ch, 64);
        hw.wait(START_GAPS[k + 4]);
    }
    // (the interrupt enable goes to Paula unlogged: the original's host serves the refill without it)
    hw.bus.paula.write_intena(0x8080);
    hw.bus.write_word(0xDFF096, 0x8003);
    hw.wait(SONG_AT[three as usize]);
    r.start = hw.bus.clock;
    Some(Box::new(r))
}

impl Replayer for Adpcm {
    fn format(&self) -> &'static str {
        if self.three {
            "ADPCM3"
        } else {
            "ADPCM2"
        }
    }

    fn title(&self) -> String {
        String::new()
    }

    /// Everything happens in the audio interrupt.
    fn tick(&mut self, _hw: &mut Hw) {}

    fn songend(&self) -> bool {
        self.songend
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        if ch != 0 {
            return false;
        }
        // (the request is cleared unlogged, as the enable is)
        let clock = hw.bus.clock;
        hw.bus.paula.sync(clock, &hw.bus.ram);
        hw.bus.paula.write_intreq(0x0080);
        let latency = if self.refills == 0 { (self.start + IRQ_LATENCY).saturating_sub(hw.bus.clock) } else { LATE_LATENCY };
        self.refill(hw, latency);
        true
    }
}

#[cfg(test)]
#[path = "adpcm23_tests.rs"]
mod tests;
