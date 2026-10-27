//! Open replayers: Rust ports of NostalgicPlayer's players (MIT, Thomas Neumann),
//! playing on the emulated Paula as the original 68k players do.
//!
//! The ports write the registers the original players write, in their order, so a
//! port can be checked against the Eagleplayer it replaces by comparing the write
//! logs (`DELI_WRITE_LOG`) tick by tick, and then the audio. Their sample data
//! lives in guest memory: the module where it was loaded, and the player's own
//! waveforms in memory allocated for them.

pub mod jamcracker;
pub mod pumatracker;
pub mod sidmon2;

use crate::bus::Bus;
use crate::machine::Machine;
use m68k::AddressBus;

/// Colour clocks between two ticks of DeliTracker's default timer (1/50 s).
pub const DT_TICK: u64 = (crate::deli::DEFAULT_TIMER as u64 + 1) * 5;

pub trait Replayer: Send {
    fn format(&self) -> &'static str;
    fn title(&self) -> String;
    /// One timer interrupt.
    fn tick(&mut self, hw: &mut Hw);
    /// Colour clocks until the next tick.
    fn tick_clocks(&self) -> u64 {
        DT_TICK
    }
    fn songend(&self) -> bool;
    /// The first, last and current subsong, in DeliTracker's numbering.
    fn subsongs(&self) -> (u16, u16, u16) {
        (0, 0, 0)
    }
    /// Restarts at subsong `n` (within `subsongs`), as the original player's
    /// InitSound does; all DMA is already off.
    fn set_subsong(&mut self, _n: u16, _hw: &mut Hw) {}
}

/// One open replayer: how to recognise its modules and start one.
pub struct Open {
    pub name: &'static str,
    pub detect: fn(&[u8]) -> bool,
    /// The module `data` is in guest memory at `base`.
    pub new: fn(data: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>>,
}

pub const REPLAYERS: &[Open] = &[pumatracker::OPEN, sidmon2::OPEN, jamcracker::OPEN];

/// The audio registers, written through the bus as guest code writes them (so
/// Paula runs to the moment of the write, and the write log sees it).
pub struct Hw<'a> {
    pub bus: &'a mut Bus,
}

impl Hw<'_> {
    pub fn dmacon(&mut self, v: u16) {
        self.bus.write_word(0xDFF096, v);
    }
    pub fn loc(&mut self, ch: usize, a: u32) {
        let r = 0xDFF0A0 + 16 * ch as u32;
        self.bus.write_word(r, (a >> 16) as u16);
        self.bus.write_word(r + 2, a as u16);
    }
    pub fn len(&mut self, ch: usize, words: u16) {
        self.bus.write_word(0xDFF0A4 + 16 * ch as u32, words);
    }
    pub fn per(&mut self, ch: usize, period: u16) {
        self.bus.write_word(0xDFF0A6 + 16 * ch as u32, period);
    }
    pub fn vol(&mut self, ch: usize, vol: u16) {
        self.bus.write_word(0xDFF0A8 + 16 * ch as u32, vol);
    }
    /// Lets `clocks` colour clocks pass before the next write, as the original's
    /// code takes time between two (Paula runs on to the next write's moment).
    pub fn wait(&mut self, clocks: u64) {
        self.bus.clock += clocks;
    }
}

pub(crate) fn be16(d: &[u8], o: usize) -> u16 {
    d.get(o..o + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
}

pub(crate) fn be32(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
