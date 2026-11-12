//! Open replayers: Rust ports of NostalgicPlayer's players (MIT, Thomas Neumann),
//! playing on the emulated Paula as the original 68k players do.
//!
//! The ports write the registers the original players write, in their order, so a
//! port can be checked against the Eagleplayer it replaces by comparing the write
//! logs (`DELI_WRITE_LOG`) tick by tick, and then the audio. Their sample data
//! lives in guest memory: the module where it was loaded, and the player's own
//! waveforms in memory allocated for them.

pub mod actionamics;
pub mod adpcm23;
pub mod adpcmmono;
pub mod amcomposer;
pub mod aprosys;
pub mod alcatrazpacker;
pub mod andrewparton;
pub mod ashleyhogg;
pub mod agi;
pub mod activisionpro;
pub mod amosmusicbank;
pub mod ahx;
pub mod artofnoise;
pub mod bendaglish;
pub mod bladepacker;
pub mod beathoven;
pub mod cinemaware;
mod beathoven_tables;
pub mod davelowe;
pub mod fredgray;
mod davelowe_cpu;
mod davelowe_gaps;
mod davelowe_tables;
pub mod davidhanney;
pub mod davidwhittaker;
pub mod digibooster;
pub mod digitalmugician;
pub mod digitalsoundstudio;
pub mod dynamicsynthesizer;
pub mod dsc;
pub mod ems;
pub mod deltamusic1;
pub mod deltamusic2;
pub mod coredesign;
pub mod desire;
pub mod facethemusic;
mod facethemusic_tables;
pub mod fred;
pub mod futurecomposer;
pub mod gamemusiccreator;
pub mod iffsmus;
pub mod infogrames;
pub mod instereo1;
pub mod instereo2;
pub mod hippel;
pub mod hvl;
pub mod jamcracker;
pub mod jasonbrooke;
pub mod jankomrsicflogel;
pub mod jannesalmijarvi;
pub mod jasonpage;
pub mod jeroentel;
pub mod jesperolsen;
pub mod laxity;
pub mod dirkbialluch;
pub mod kh;
pub mod legglessmusiceditor;
pub mod markcooksey;
pub mod markcookseyold;
pub mod nickpelling;
pub mod maximumeffect;
pub mod soundprogramminglanguage;
pub mod med;
pub(crate) mod m68kdec;
#[cfg(test)]
mod m68kenc;
pub mod maxtrax;
pub mod mikedavies;
pub mod medpacker;
pub mod musicassembler;
pub mod musicmaker;
pub mod multimod;
pub mod musicmaker8;
pub mod mon;
pub mod pierreadane;
pub mod pretracker;
mod pretracker_tables;
pub mod musicline;
pub mod oktalyzer;
pub mod onescapee;
pub mod ntspsystem;
pub mod dariuszendeh;
pub mod audiosculpture;
pub mod earache;
pub mod speedysystem;
pub mod futurecomposerbsi;
pub mod speedya1system;
pub mod gluemon;
pub mod kimchristensen;
pub mod scottjohnston;
pub mod majortom;
pub mod quartet;
pub mod tomytracker;
pub mod midiloriciel;
pub mod quartetst;
pub mod quartetpsg;
pub mod voodoosupremesynthesizer;
pub mod suntronic;
pub mod paulrobotham;
pub mod paultonge;
pub mod howiedavies;
pub mod professionalsoundartists;
pub mod thomashermann;
pub mod paulsummers;
pub mod forgottenworlds;
pub mod futureplayer;
pub mod anders0land;
pub mod jochenhippelst;
pub mod mcmd;
pub mod medley;
pub mod artandmagic;
pub mod robhubbard;
pub mod robhubbard2;
pub mod robhubbardst;
pub mod stevebarrett;
pub mod paulshields;
pub mod peterverswyvelen;
pub mod protracker4;
pub mod ptm;
pub mod pumatracker;
pub mod quadracomposer;
mod quadracomposer_tables;
pub mod richardjoseph;
pub mod richardjosephrj;
pub mod bendaglishsid;
pub mod markii;
pub mod pokeynoise;
pub mod ay;
pub mod riffraff;
pub mod fashiontracker;
pub mod seanconnolly;
pub mod seanconran;
pub mod soundplayer;
pub mod custommade;
pub mod ronklaren;
pub mod scumm;
pub mod silmarils;
pub mod sidmon1;
pub mod sidmon2;
pub mod soundcontrol;
pub mod soundfactory;
pub mod soundfx;
pub mod multimediasound;
pub mod soundimages;
pub mod sonicarranger;
pub mod sonix;
pub mod specialfx;
pub mod steveturner;
pub mod stp3;
pub mod soundmaster;
pub mod soundmon;
pub mod soundtracker;
pub mod st26;
pub mod synthdream;
pub mod synthesis;
pub mod syntracker;
pub mod synthpack;
pub mod specialfxst;
pub mod tfhd;
pub mod tfmx;
pub mod tfmxst;
pub mod timfollin;
pub mod tcbtracker;
pub mod timetracker;
pub mod tme;
pub mod tronic;
pub mod ufo;
pub mod wallybeben;
pub mod ym2149;
pub mod zoundmonitor;

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
    /// Colour clocks from the song's start (after `new`) to the start of the timer, on top of
    /// the first period: what the original's StartInt takes before the timer runs.
    fn timer_delay(&self) -> u64 {
        0
    }
    /// The time (colour clocks, absolute) of the next tick, when the player keeps its own
    /// timer and knows it after a tick (a CIA timer whose underflows the original's slow
    /// code can miss, as the 4-voice Musicline Editor's): the engine then takes it
    /// in place of the previous due time plus `tick_clocks`. A tick that is already due runs
    /// at once, as for the default.
    fn next_tick_at(&self) -> Option<u64> {
        None
    }
    /// The time (colour clocks, absolute) of the first tick after `set_subsong`, when the
    /// original's change of subsong takes time before its timer runs again.
    fn subsong_tick_at(&mut self) -> Option<u64> {
        None
    }
    /// The music comes from a PC tracker: the output stage is the PC one (no Amiga filters),
    /// as for the 68k players of PC formats.
    fn pc_music(&self) -> bool {
        false
    }
    /// Whether the original's code for this song needs more than a 68000's time to keep
    /// up (an 8-voice Musicline's mixer takes 1 to 5 buffers' time on one): its reference
    /// run, which counts 68000 cycles, then plays slow or repeats buffers, where UADE
    /// (a 68020 at full speed) does not. The comparison runs it with DELI_CPU_SPEED.
    fn needs_fast_reference_cpu(&self) -> bool {
        false
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
    /// port may install a 68k one).
    fn audio_irq(&mut self, _hw: &mut Hw) -> bool {
        false
    }
    /// The same, for the audio interrupt of channel `ch` (0-3) in particular:
    /// the engine calls this one, once for each channel that raised an enabled
    /// interrupt, lowest first. A player whose handlers are per channel (a
    /// 68k handler each in the original) overrides it; the default is
    /// `audio_irq`, for the players whose one handler looks at INTREQ.
    fn audio_irq_ch(&mut self, _ch: usize, hw: &mut Hw) -> bool {
        self.audio_irq(hw)
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
    // first: the strict SAP header test ("SAP", TYPE B, M or C) takes no other format's file, but
    // some weaker tests below take SAP files
    pokeynoise::OPEN,
    // strict: the IFF FORM / MODL container with the player's own chunk walk (VERS, INFO, PTDT); no other format starts so
    protracker4::OPEN,
    // strict: `ADPCM2` / `ADPCM3` and a header (the headerless ADPCM mono format below refuses `ADPC`), so first anyway
    adpcm23::OPEN,
    // strict: four branch entries, a fixed word at offset 16 and the driver's start-entry shape at the target of the third
    // entry; the sibling Quartet and Quartet ST replayers (qpa., qts.) have no such header, but first anyway: no order dependence
    quartetpsg::OPEN,
    pumatracker::OPEN,
    sidmon2::OPEN,
    jamcracker::OPEN,
    futurecomposer::OPEN13,
    futurecomposer::OPEN14,
    soundmon::OPEN,
    deltamusic2::OPEN,
    fred::OPEN,
    // before SoundFX 2.0, which takes the same files: this one only by file name (mms., sfx20.)
    multimediasound::OPEN,
    soundfx::OPEN13,
    soundfx::OPEN20,
    deltamusic1::OPEN,
    sonicarranger::OPEN,
    sidmon1::OPEN,
    // before TFMX 1.5, which takes any "TFMX" but "TFMX-SONG", and Hippel COSO
    hippel::OPEN_7V,
    tfmx::OPEN_PRO,
    tfmx::OPEN_15,
    tfmx::OPEN_7V,
    tfmxst::OPEN,
    tfhd::OPEN_15,
    tfhd::OPEN_7V,
    tfhd::OPEN_PRO,
    iffsmus::OPEN,
    bendaglish::OPEN,
    // before Hippel COSO, which takes any "COSO" with "TFMX": the files laid out as the ST's
    jochenhippelst::OPEN_COSO,
    hippel::OPEN_COSO,
    hippel::OPEN,
    davidwhittaker::OPEN,
    actionamics::OPEN,
    soundcontrol::OPEN,
    digitalmugician::OPEN1,
    digitalmugician::OPEN2,
    dynamicsynthesizer::OPEN,
    artofnoise::OPEN4,
    artofnoise::OPEN8,
    // raw Ron Klaren dumps (entry-word rules, a 12-byte marker and the driver's table markers); the
    // hunk modules of the next replayer fail the entry-word rule, so no order dependence
    custommade::OPEN,
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
    gamemusiccreator::OPEN,
    ahx::OPEN,
    med::OPEN,
    pretracker::OPEN,
    musicline::OPEN,
    hvl::OPEN,
    // the magic `JS92` at 1080 (and the size rule), before the 15-instrument SoundTracker
    // replayers, whose loose content rules take such a module (the 'JS.short' test module)
    jannesalmijarvi::OPEN,
    soundtracker::OPEN_UST10,
    soundtracker::OPEN_UST18,
    soundtracker::OPEN_ST2,
    soundtracker::OPEN_ST6,
    soundtracker::OPEN_ST9,
    soundtracker::OPEN_MST,
    soundtracker::OPEN_ST2X,
    st26::OPEN_ST26,
    st26::OPEN_ICE,
    multimod::OPEN,
    multimod::OPEN_FLT8,
    quadracomposer::OPEN,
    facethemusic::OPEN,
    oktalyzer::OPEN,
    digibooster::OPEN,
    med::OPEN_MIX,
    med::OPEN_OSS,
    tme::OPEN,
    agi::OPEN,
    ptm::OPEN,
    richardjoseph::OPEN,
    richardjosephrj::OPEN,
    sonix::OPEN,
    davelowe_cpu::OPEN,
    fredgray::OPEN,
    davelowe::OPEN_DL,
    davelowe::OPEN_DLN,
    jesperolsen::OPEN,
    pierreadane::OPEN,
    medpacker::OPEN,
    soundplayer::OPEN,
    peterverswyvelen::OPEN,
    alcatrazpacker::OPEN,
    tcbtracker::OPEN,
    mon::OPEN,
    specialfx::OPEN,
    jasonpage::OPEN,
    wallybeben::OPEN,
    beathoven::OPEN,
    mikedavies::OPEN,
    synthpack::OPEN,
    cinemaware::OPEN,
    soundimages::OPEN,
    andrewparton::OPEN,
    soundmaster::OPEN,
    timfollin::OPEN,
    dsc::OPEN,
    scumm::OPEN,
    jasonbrooke::OPEN,
    silmarils::OPEN,
    paulshields::OPEN,
    seanconran::OPEN,
    kh::OPEN,
    jeroentel::OPEN,
    amcomposer::OPEN,
    jankomrsicflogel::OPEN,
    paulrobotham::OPEN,
    dirkbialluch::OPEN,
    desire::OPEN,
    fashiontracker::OPEN,
    ems::OPEN,
    steveturner::OPEN,
    synthdream::OPEN,
    robhubbard::OPEN,
    robhubbardst::OPEN,
    stevebarrett::OPEN,
    ashleyhogg::OPEN,
    coredesign::OPEN,
    stp3::OPEN,
    maxtrax::OPEN,
    syntracker::OPEN,
    davidhanney::OPEN,
    markcooksey::OPEN,
    legglessmusiceditor::OPEN,
    soundprogramminglanguage::OPEN,
    paulsummers::OPEN,
    // strict: the block table, every row of the highest pattern in range and the sample entries; taken before the built-in ProTracker (which plays the ProWizard conversion of TMK files)
    timetracker::OPEN,
    onescapee::OPEN,
    ntspsystem::OPEN,
    speedysystem::OPEN,
    futurecomposerbsi::OPEN,
    dariuszendeh::OPEN,
    tronic::OPEN,
    forgottenworlds::OPEN,
    artandmagic::OPEN,
    howiedavies::OPEN,
    bladepacker::OPEN,
    ufo::OPEN,
    futureplayer::OPEN,
    professionalsoundartists::OPEN,
    maximumeffect::OPEN,
    seanconnolly::OPEN,
    speedya1system::OPEN,
    quartet::OPEN,
    suntronic::OPEN,
    quartetst::OPEN,
    gluemon::OPEN,
    kimchristensen::OPEN,
    scottjohnston::OPEN,
    majortom::OPEN,
    riffraff::OPEN,
    // the DAGLISH! tag at file offset 0x24 is an exact magic (the Dave Lowe wrapper has another tag): no order dependence
    bendaglishsid::OPEN,
    // strict: the module check and a match of one of three driver builds' site tables; the Darius
    // Zendeh modules fail the check and the Mark II ones have no Darius Zendeh tables: no order dependence
    markii::OPEN,
    nickpelling::OPEN,
    anders0land::OPEN,
    thomashermann::OPEN,
    voodoosupremesynthesizer::OPEN,
    // the ADRVPACK magic is exact: no order dependence
    aprosys::OPEN,
    // last: Hippel COSO and TFMX files are taken by the Hippel replayers above
    jochenhippelst::OPEN,
    // weak detection (two flag bytes and a signature): after every format with a magic word
    laxity::OPEN,
    // strict content rule, no real module known: after every other replayer
    tomytracker::OPEN,
    medley::OPEN,
    ym2149::OPEN,
    // strict byte patterns but no real module known (a file of any size can pass the first words): after the others
    specialfxst::OPEN,
    // the magic only, no real module known: after every other replayer
    mcmd::OPEN,
    // a Standard MIDI File with a sample bank beside it: strict header, after every other replayer
    midiloriciel::OPEN,
    audiosculpture::OPEN,
    // the plain kind is recognised by the header longs alone (first word zero): after every other replayer with a magic word
    earache::OPEN,
    // weak detection (a trailer of the form "df?:" or "?amp" after the computed end of the patterns; sng.* files are also ProTracker / STK modules): after every other replayer with a magic word
    zoundmonitor::OPEN,
    // weak detection (a word 12 and three list offsets whose first pattern follows a pattern end): after every other
    // format with a stricter rule, before the two weakest
    paultonge::OPEN,
    // weak detection (first byte, a size field, one zero byte): after everything else but Infogrames
    markcookseyold::OPEN,
    // after the others: its detection is loose (two bytes of the first envelope)
    infogrames::OPEN,
    // strict layout rule (offsets in order, every list closed by FF, two 416-byte tables to the file's end) and a bank beside it;
    // after Infogrames: its modules pass this rule and their <name>.ins bank passes this bank check, while
    // Infogrames declines a Rob Hubbard 2 module (no bank of its name), which the engine then gives to this one
    robhubbard2::OPEN,
    // last: its content check takes anything (the file name is checked in `new`)
    adpcmmono::OPEN,
];

/// The audio registers, written through the bus as guest code writes them (so
/// Paula runs to the moment of the write, and the write log sees it).
pub struct Hw<'a> {
    pub bus: &'a mut Bus,
}

/// Where a tick's register writes fall, measured in the original's runs: the colour clocks from
/// the previous write of the tick (or from the tick's start) to each write, by the kind of write and
/// the channel. A port that has such a table switches it on with `Hw::gaps` at the start of its tick;
/// `Hw` then lets the time pass before each write.
///
/// A write is keyed `kind * 4 + channel` (`WriteGaps::LOC` .. `DMA_ON`; DMAON/DMAOFF use channel 0),
/// the tick's start is `WriteGaps::START`. `pairs` holds (previous key, key, gap) for the sequences
/// seen in the runs, `any` (key, gap) the typical gap to a key after any other write, used for a
/// sequence the runs did not show, and `start` (key, gap) the first write of the tick.
#[derive(Clone, Copy)]
pub struct WriteGaps {
    pub pairs: &'static [(u8, u8, u16)],
    pub any: &'static [(u8, u16)],
    pub start: &'static [(u8, u16)],
    prev: u8,
}

impl WriteGaps {
    pub const LOC: u8 = 0;
    pub const LEN: u8 = 1;
    pub const PER: u8 = 2;
    pub const VOL: u8 = 3;
    pub const DMA_OFF: u8 = 4;
    pub const DMA_ON: u8 = 5;
    pub const START: u8 = 255;

    pub const fn new(pairs: &'static [(u8, u8, u16)], any: &'static [(u8, u16)], start: &'static [(u8, u16)]) -> Self {
        WriteGaps { pairs, any, start, prev: Self::START }
    }

    pub fn key(kind: u8, ch: usize) -> u8 {
        kind * 4 + if kind >= Self::DMA_OFF { 0 } else { ch as u8 & 3 }
    }

    /// The colour clocks before the write `key`; it becomes the previous write.
    fn gap(&mut self, key: u8) -> u64 {
        let prev = std::mem::replace(&mut self.prev, key);
        if prev == Self::START {
            return self.start.iter().find(|e| e.0 == key).map_or(0, |e| e.1 as u64);
        }
        if let Some(e) = self.pairs.iter().find(|e| e.0 == prev && e.1 == key) {
            return e.2 as u64;
        }
        self.any.iter().find(|e| e.0 == key).map_or(0, |e| e.1 as u64)
    }
}

impl Hw<'_> {
    /// Switches the write-spacing model on for this tick (the previous write is the tick's start).
    pub fn gaps(&mut self, g: &WriteGaps) {
        let mut g = *g;
        g.prev = WriteGaps::START;
        self.bus.gaps = Some(g);
    }
    fn space(&mut self, kind: u8, ch: usize) {
        if let Some(g) = self.bus.gaps.as_mut() {
            let w = g.gap(WriteGaps::key(kind, ch));
            self.bus.clock += w;
        }
    }
    pub fn dmacon(&mut self, v: u16) {
        self.space(if v & 0x8000 != 0 { WriteGaps::DMA_ON } else { WriteGaps::DMA_OFF }, 0);
        self.bus.write_word(0xDFF096, v);
    }
    pub fn loc(&mut self, ch: usize, a: u32) {
        self.space(WriteGaps::LOC, ch);
        let r = 0xDFF0A0 + 16 * ch as u32;
        self.bus.write_word(r, (a >> 16) as u16);
        self.bus.write_word(r + 2, a as u16);
    }
    pub fn len(&mut self, ch: usize, words: u16) {
        self.space(WriteGaps::LEN, ch);
        self.bus.write_word(0xDFF0A4 + 16 * ch as u32, words);
    }
    pub fn per(&mut self, ch: usize, period: u16) {
        self.space(WriteGaps::PER, ch);
        self.bus.write_word(0xDFF0A6 + 16 * ch as u32, period);
    }
    pub fn vol(&mut self, ch: usize, vol: u16) {
        self.space(WriteGaps::VOL, ch);
        self.bus.write_word(0xDFF0A8 + 16 * ch as u32, vol);
    }
    /// Lets `clocks` colour clocks pass before the next write, as the original's
    /// code takes time between two (Paula runs on to the next write's moment).
    pub fn wait(&mut self, clocks: u64) {
        self.bus.clock += clocks;
    }
    /// Lets `n` raster line changes pass, then `phase` colour clocks of the line, as a routine that
    /// waits for the beam does.
    pub fn wait_lines(&mut self, n: u64, phase: u64) {
        use crate::paula::LINE;
        let now = self.bus.clock;
        let boundary = (now / LINE + 1) * LINE;
        self.bus.clock = boundary + (n - 1) * LINE + phase;
    }

    // The Eagleplayer amplifier's virtual voices (`start_amplifier`), poked as
    // its ENPP_Poke* and ENPP_DMAMask do for an Eagleplayer: Paula semantics,
    // each at the current colour clock, logged at amplifier::LOG_BASE.
    pub fn voice_loc(&mut self, v: usize, a: u32) {
        self.bus.amp_poke(v, Poke::Loc(a));
    }
    /// LOC at any byte, for a voice that mixes in software.
    pub fn voice_loc_byte(&mut self, v: usize, a: u32) {
        self.bus.amp_poke(v, Poke::LocByte(a));
    }
    pub fn voice_len(&mut self, v: usize, words: u16) {
        self.bus.amp_poke(v, Poke::Len(words));
    }
    /// LEN in bytes, for a block longer than Paula's register counts.
    pub fn voice_len_bytes(&mut self, v: usize, bytes: u32) {
        self.bus.amp_poke(v, Poke::LenBytes(bytes));
    }
    pub fn voice_per(&mut self, v: usize, period: u16) {
        self.bus.amp_poke(v, Poke::Per(period));
    }
    /// The period in 1/PER_SUB colour clocks (`amplifier::PER_SUB`).
    pub fn voice_per_fine(&mut self, v: usize, period: u32) {
        self.bus.amp_poke(v, Poke::PerFine(period));
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

/// The same with the voices' sides chosen by the player and the boost `gain` percent
/// of the default's (see `amplifier::start_panned`).
pub fn start_amplifier_panned(m: &mut Machine, pans: &[i16], gain: u32) -> bool {
    use crate::eagle::epamb;
    crate::amplifier::start_panned(m, pans.len(), epamb::DIRECT | epamb::BIT8 | epamb::CHIP_RAM, pans, gain)
}

pub(crate) fn be16(d: &[u8], o: usize) -> u16 {
    d.get(o..o + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
}

pub(crate) fn be32(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Reference runs of a replayer against the original's write log: plays a module
/// tick by tick without stopping at the song end and writes the register log.
#[cfg(test)]
mod run_dump {
    use crate::engine::Engine;
    use std::path::Path;

    /// `PK_MODULE=file PK_TICKS=n PK_LOG=out [PK_SUBSONG=k] cargo test --release run_dump -- --ignored`:
    /// `n` ticks (960 samples each at 48 kHz) of the module, the log in `DELI_WRITE_LOG`'s
    /// format, and the tick in which the song end was announced on stdout; `PK_SUBSONG=k`
    /// first steps `k` subsongs on, as `--subsong k` does; `PK_SKEW=c` lets `c` colour clocks pass
    /// first (to give the ticks the reference run's phase within the raster line).
    #[test]
    #[ignore]
    fn dump() {
        let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("PK_MODULE"), std::env::var("PK_TICKS"), std::env::var("PK_LOG"))
        else {
            return;
        };
        let ticks: usize = ticks.parse().unwrap();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        if let Some(c) = std::env::var("PK_SKEW").ok().and_then(|s| s.parse::<u64>().ok()) {
            e.m.bus.clock += c;
        }
        e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
        let sub: u32 = std::env::var("PK_SUBSONG").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        for _ in 0..sub {
            e.change_subsong(1).unwrap();
        }
        println!("format {}", e.playing.as_ref().unwrap().format);
        let mut buf = vec![0f32; 2 * 960];
        let mut end = None;
        for t in 0..ticks {
            e.render(&mut buf);
            if end.is_none() && e.songend() {
                end = Some(t);
            }
        }
        println!("songend tick {:?}", end);
        let l = e.m.bus.write_log.take().unwrap();
        let text: String = l.iter().map(|(a, v, t)| format!("{a:06x} {v:04x} {t}\n")).collect();
        std::fs::write(out, text).unwrap();
    }
}
