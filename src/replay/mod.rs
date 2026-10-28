//! Open replayers: Rust ports of NostalgicPlayer's players (MIT, Thomas Neumann),
//! playing on the emulated Paula as the original 68k players do.
//!
//! The ports write the registers the original players write, in their order, so a
//! port can be checked against the Eagleplayer it replaces by comparing the write
//! logs (`DELI_WRITE_LOG`) tick by tick, and then the audio. Their sample data
//! lives in guest memory: the module where it was loaded, and the player's own
//! waveforms in memory allocated for them.

pub mod actionamics;
pub mod activisionpro;
pub mod amosmusicbank;
pub mod artofnoise;
pub mod bendaglish;
pub mod davidwhittaker;
pub mod digitalmugician;
pub mod digitalsoundstudio;
pub mod deltamusic1;
pub mod deltamusic2;
pub mod fred;
pub mod futurecomposer;
pub mod iffsmus;
pub mod instereo1;
pub mod instereo2;
pub mod hippel;
pub mod jamcracker;
pub mod musicassembler;
pub mod musicmaker;
pub mod musicmaker8;
pub mod pumatracker;
pub mod ronklaren;
pub mod sidmon1;
pub mod sidmon2;
pub mod soundcontrol;
pub mod soundfactory;
pub mod soundfx;
pub mod sonicarranger;
pub mod soundmon;
pub mod synthesis;
pub mod tfmx;

use crate::amplifier::Poke;
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
    /// An audio interrupt, for a player whose original has a handler: called as
    /// the 68k players' handlers are, when Paula raises an enabled one. Returns
    /// whether it took it; if not, the handler in the interrupt vector runs (a
    /// port may install a 68k one, as TFMX's does).
    fn audio_irq(&mut self, _hw: &mut Hw) -> bool {
        false
    }
}

/// One open replayer: how to recognise its modules and start one.
pub struct Open {
    pub name: &'static str,
    pub detect: fn(&[u8]) -> bool,
    /// The module `data` is in guest memory at `base`.
    pub new: fn(data: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>>,
}

pub const REPLAYERS: &[Open] = &[
    pumatracker::OPEN,
    sidmon2::OPEN,
    jamcracker::OPEN,
    futurecomposer::OPEN13,
    futurecomposer::OPEN14,
    soundmon::OPEN,
    deltamusic2::OPEN,
    fred::OPEN,
    soundfx::OPEN13,
    soundfx::OPEN20,
    deltamusic1::OPEN,
    sonicarranger::OPEN,
    sidmon1::OPEN,
    tfmx::OPEN_PRO,
    tfmx::OPEN_15,
    iffsmus::OPEN,
    bendaglish::OPEN,
    hippel::OPEN_COSO,
    hippel::OPEN,
    davidwhittaker::OPEN,
    actionamics::OPEN,
    soundcontrol::OPEN,
    digitalmugician::OPEN1,
    digitalmugician::OPEN2,
    artofnoise::OPEN4,
    artofnoise::OPEN8,
    ronklaren::OPEN,
    digitalsoundstudio::OPEN,
    musicassembler::OPEN,
    soundfactory::OPEN,
    instereo1::OPEN,
    instereo2::OPEN,
    synthesis::OPEN,
    activisionpro::OPEN,
    musicmaker::OPEN,
    musicmaker8::OPEN,
    amosmusicbank::OPEN,
];

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

    // The Eagleplayer amplifier's virtual voices (`start_amplifier`), poked as
    // its ENPP_Poke* and ENPP_DMAMask do for an Eagleplayer: Paula semantics,
    // each at the current colour clock, logged at amplifier::LOG_BASE.
    pub fn voice_loc(&mut self, v: usize, a: u32) {
        self.bus.amp_poke(v, Poke::Loc(a));
    }
    pub fn voice_len(&mut self, v: usize, words: u16) {
        self.bus.amp_poke(v, Poke::Len(words));
    }
    pub fn voice_per(&mut self, v: usize, period: u16) {
        self.bus.amp_poke(v, Poke::Per(period));
    }
    pub fn voice_vol(&mut self, v: usize, vol: u16) {
        self.bus.amp_poke(v, Poke::Vol(vol));
    }
    /// DMA on or off for the voices of `mask` (bit n: voice n), in voice order.
    pub fn voice_dma(&mut self, mask: u32, on: bool) {
        for v in (0..32).filter(|v| mask & 1 << v != 0) {
            self.bus.amp_poke(v, Poke::Dma(on));
        }
    }
}

/// Starts the Eagleplayer amplifier with `voices` virtual voices poked directly
/// (EPAMB_Direct, signed 8-bit samples in chip memory), sides LRRL as for an
/// Eagleplayer whose AudioStructs leave them 0; `Hw::voice_*` then play them.
pub fn start_amplifier(m: &mut Machine, voices: usize) -> bool {
    use crate::eagle::epamb;
    crate::amplifier::start(m, voices, 0, epamb::DIRECT | epamb::BIT8 | epamb::CHIP_RAM)
}

pub(crate) fn be16(d: &[u8], o: usize) -> u16 {
    d.get(o..o + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
}

pub(crate) fn be32(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
