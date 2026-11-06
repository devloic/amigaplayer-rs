//! ADPCM mono (the Eagleplayer "ADPCM_mono", Adaptative Delta Pulse Code Modulation): a raw
//! 4-bit ADPCM stream, played as a sample streamer.
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register
//! writes and DMA fetches of the 68k reference player; no original code. All the modules the
//! port was checked on are synthetic (the corpus has no real `.ADPCM` file).
//!
//! The file has no header: every byte is two 4-bit codes, high nibble first. They are decoded
//! with the standard IMA step table into signed 8-bit samples (the high byte of the 16-bit
//! predictor), 1024 at a time into two chip buffers that all four Paula channels play (volumes
//! 64, 64, 1, 1: mono) at period 125. The audio channel 0 interrupt, once a buffer, fills the
//! other buffer and points the channels at it. The register writes are placed at the clocks
//! of the reference (the decode cost depends on how many samples saturate or clamp the index).
//!
//! Recognition: the file name must end in `.ADPCM` (the reference's quirky case rule, see
//! `name_ok`) and the content must not start with `ADPC`. A replayer's `detect` takes only the
//! content, so the engine (and the library's `Song::open`) hand the file name over with
//! `set_name` around their detection; without it `detect` is false. Any content is accepted
//! but what another format takes. `new` checks the name again from the guest's file name.
//!
//! A file of fewer than 512 bytes is decoded as the reference does: the stream goes on with the
//! zero bytes past the file and the song never ends. The song end of a longer file is announced
//! as the reference does, and the stream then restarts.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Adaptative Delta Pulse Code Modulation", detect, new };

/// The standard IMA/DVI step table.
const STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

/// The step index change by the low three bits of a code.
const ADJUST: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Samples in a buffer.
const BUF: usize = 1024;
/// Colour clocks from the request of the audio interrupt to its first write.
const IRQ_LATENCY: u64 = 68;
/// Where the first volume write falls (colour clocks into the run) in the reference.
const START_AT: u64 = 618;
/// The fill of a buffer past the end of the stream: nothing to decode, 228 + 256 * 15.
const SILENT_COST: u64 = 4068;

/// The file name's suffix rule of the reference (reading the name from its end): one of
/// `mM`, one of `cC`, zero or more `p`, one `P`, one of `dD`, one of `aA`, then a dot.
pub(crate) fn name_ok(name: &str) -> bool {
    let b = name.as_bytes();
    let mut i = b.len();
    let mut take = |ok: &dyn Fn(u8) -> bool| -> bool {
        if i > 0 && ok(b[i - 1]) {
            i -= 1;
            true
        } else {
            false
        }
    };
    if !take(&|c| c == b'm' || c == b'M') || !take(&|c| c == b'c' || c == b'C') {
        return false;
    }
    while take(&|c| c == b'p') {}
    take(&|c| c == b'P') && take(&|c| c == b'd' || c == b'D') && take(&|c| c == b'a' || c == b'A') && take(&|c| c == b'.')
}

thread_local! {
    /// The name of the file being asked about; empty when nobody has said.
    static NAME: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Tells `detect` the name of the file whose content it is asked about (the engine and the
/// library do, around their detection; `detect` takes only the content). An empty name
/// clears it. Without a name that ends as the rule says, `detect` is false.
pub fn set_name(name: &str) {
    NAME.with(|n| {
        let mut n = n.borrow_mut();
        n.clear();
        n.push_str(name);
    });
}

/// The name `set_name` was last given (empty when none), for the other open replayers whose
/// detection depends on the file name.
pub(crate) fn current_name() -> String {
    NAME.with(|n| n.borrow().clone())
}

fn detect(d: &[u8]) -> bool {
    // the name (see `set_name`) ends in `.ADPCM`; anything but the other format's magic and
    // what another format takes: the built-in ProTracker, libxmp's formats and every other open
    // replayer
    NAME.with(|n| name_ok(&n.borrow())) && content_ok(d)
}

fn content_ok(d: &[u8]) -> bool {
    if d.is_empty() || d.starts_with(b"ADPC") {
        return false;
    }
    crate::xmp::detect_open(d).is_none()
        && crate::protracker::ProTracker::detect(d).is_none()
        && !super::REPLAYERS.iter().any(|o| o.name != OPEN.name && (o.detect)(d))
}

/// The decoder state.
#[derive(Clone, Copy, Default)]
struct Dec {
    p: i32,
    i: i32,
}

impl Dec {
    /// One code: the sample byte, whether the predictor saturated and whether the index
    /// clamped (reaching 88 counts).
    fn code(&mut self, n: u8) -> (i8, bool, bool) {
        let m = (n & 7) as i32;
        let mut v = STEP[self.i as usize] * (2 * m + 1);
        if n & 8 != 0 {
            v = -v;
        }
        let dd = (v >> 3).clamp(-32768, 32767);
        let q = self.p + dd;
        let sat = !(-32768..=32767).contains(&q);
        self.p = q.clamp(-32768, 32767);
        let ni = self.i + ADJUST[m as usize];
        let clamp = !(0..88).contains(&ni);
        self.i = ni.clamp(0, 88);
        ((self.p >> 8) as i8, sat, clamp)
    }
}

struct Adpcm {
    d: Vec<u8>,
    /// Samples in the stream (twice the even part of the file).
    len: i64,
    /// Read position, in samples.
    pos: usize,
    /// Samples still to decode.
    left: i64,
    dec: Dec,
    bufs: [u32; 2],
    next: usize,
    songend: bool,
    title: String,
}

impl Adpcm {
    /// Decodes 1024 samples from the read position (past the file: zero bytes) into `out`;
    /// returns the decode cost in colour clocks.
    fn decode(&mut self, out: &mut [u8; BUF]) -> u64 {
        let (mut sat, mut clamp) = (0u64, 0u64);
        for k in 0..BUF / 2 {
            let b = self.d.get(self.pos / 2 + k).copied().unwrap_or(0);
            for (j, n) in [b >> 4, b & 15].into_iter().enumerate() {
                let (s, a, c) = self.dec.code(n);
                out[2 * k + j] = s as u8;
                sat += a as u64;
                clamp += c as u64;
            }
        }
        self.pos += BUF;
        // 63507 + 10 per saturation + 6.5 per index clamp, rounded to the clock
        (63_507 * 2 + 20 * sat + 13 * clamp) / 2
    }

    /// The stream restarts, and the end of the song is announced.
    fn restart(&mut self) {
        self.pos = 0;
        self.left = self.len;
        self.dec = Dec::default();
        self.songend = true;
    }

    /// Fills the next buffer; returns the colour clocks to the first LC write. `init` is the
    /// first fill of the song (always a decode).
    fn fill(&mut self, hw: &mut Hw, init: bool) -> u64 {
        let mut out = [0u8; BUF];
        let cost;
        if !init && self.left <= 0 {
            self.restart();
            cost = SILENT_COST;
        } else {
            let mut c = self.decode(&mut out);
            let mut n = self.left.clamp(0, BUF as i64) as usize;
            self.left -= BUF as i64;
            if init && self.left < 0 {
                // a stream shorter than a buffer: the init decode does not check the length, so
                // its buffer holds the decoded memory past the file (zero bytes) as well, and
                // the count of samples left wraps round: the song never ends
                n = BUF;
                self.left = i64::from(u32::MAX) - BUF as i64;
            }
            if n < BUF {
                out[n..].fill(0);
                c += 133 + 15 * ((BUF - n) / 4) as u64;
                if !init {
                    self.restart();
                }
            }
            cost = c;
        }
        hw.bus.write_bytes(self.bufs[self.next], &out);
        cost
    }

    /// The four channels' locations to the buffer just filled; the other one is next.
    fn point(&mut self, hw: &mut Hw) {
        let a = self.bufs[self.next];
        for ch in 0..4 {
            if ch > 0 {
                hw.wait(8);
            }
            hw.loc(ch, a);
        }
        self.next ^= 1;
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let file = m.bus.read_cstr(m.deli.file_buf);
    if !name_ok(&file) || !content_ok(d) {
        return None;
    }
    let a = m.alloc(2 * BUF as u32);
    if a == 0 {
        return None;
    }
    let len = 2 * (d.len() & !1) as i64;
    let mut r = Adpcm {
        d: d.to_vec(),
        len,
        pos: 0,
        left: len,
        dec: Dec::default(),
        bufs: [a, a + BUF as u32],
        next: 0,
        songend: false,
        title: file,
    };
    let mut hw = Hw { bus: &mut m.bus };
    let start = hw.bus.clock;
    if start < START_AT {
        hw.wait(START_AT - start);
    }
    hw.vol(0, 64);
    hw.wait(6);
    hw.vol(1, 64);
    hw.wait(8);
    hw.vol(2, 1);
    hw.wait(6);
    hw.vol(3, 1);
    let cost = r.fill(&mut hw, true);
    hw.wait(cost.saturating_sub(53));
    r.point(&mut hw);
    hw.wait(18);
    for ch in 0..4 {
        if ch > 0 {
            hw.wait(6);
        }
        hw.len(ch, 0x200);
    }
    hw.wait(8);
    for ch in 0..4 {
        if ch > 0 {
            hw.wait(6);
        }
        hw.per(ch, 125);
    }
    hw.wait(85);
    hw.bus.write_word(0xDFF09A, 0x8080);
    hw.wait(10);
    hw.dmacon(0x800F);
    Some(Box::new(r))
}

impl Replayer for Adpcm {
    fn format(&self) -> &'static str {
        "Adaptative Delta Pulse Code Modulation"
    }

    fn title(&self) -> String {
        self.title.clone()
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
        hw.wait(IRQ_LATENCY);
        hw.bus.write_word(0xDFF09A, 0x0080);
        hw.wait(8);
        hw.bus.write_word(0xDFF09C, 0x0080);
        let cost = self.fill(hw, false);
        hw.wait(cost);
        self.point(hw);
        hw.wait(16);
        hw.bus.write_word(0xDFF09A, 0x8080);
        true
    }
}

#[cfg(test)]
#[path = "adpcmmono_tests.rs"]
mod tests;
