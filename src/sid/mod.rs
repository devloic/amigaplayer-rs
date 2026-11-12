// Port of NostalgicPlayer's SidPlay agent (Source/Agents/Players/SidPlay/SidPlayWorker.cs,
// MIT: LICENSE-NostalgicPlayer) and of its C# port of libsidplayfp (by Simon White, Antti
// Lankila and Leandro Nini): Source/Ports/LibSidPlayFp/Player.cs, SimpleMixer.cs,
// SidPlayFp/SidConfig.cs and SidPlayFp/SidPlayFp.cs, git e183943.
// Licence of the libsidplayfp port: GNU General Public License version 2
// (LICENSE-LibSidPlayFp); the upstream files carry only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more information."
//
// Structure changed for Rust: Player, SidPlayFp, SidConfig, the SID builder and the
// agent's worker are one struct, `SidPlayer`, with the agent's default settings fixed
// (see `SidPlayer`). Not ported: the settings window, STIL and song length databases,
// the MUS and PRG loaders, the ROM images, the voice mute and filter switches.

//! C64 SID tunes (`PSID` / `RSID` files) on an emulated Commodore 64: the 6510 CPU, the
//! two CIAs, the VIC-II's timing and one to three SID chips (reSIDfp's emulation of the
//! MOS 6581 and 8580, filter included), ported from NostalgicPlayer's C# ports of
//! libsidplayfp and reSIDfp. Bytes in, stereo PCM out at the rate asked for; the same
//! input gives the same samples on every run and platform.
//!
//! **Licence: this module is under the GNU General Public License version 2** (the
//! licence files of the two ports it derives from: `LICENSE-LibSidPlayFp`,
//! `LICENSE-LibReSidFp`; their source files do not say "or any later version"). A build
//! with the `sid` feature is therefore a GPL-2.0 work as a whole.
//!
//! No Commodore ROM is part of it: the KERNAL is libsidplayfp's own stand-in (interrupt
//! entry and exit, `RTS` at the jump table), the BASIC and character ROMs are empty. A
//! tune that needs BASIC is refused; one that calls into the KERNAL beyond the jump
//! table plays as far as it gets.

pub mod c64;
pub mod cia;
pub mod mos6510;
pub mod psiddrv;
pub mod resid;
pub mod sched;
pub mod tune;
pub mod vic;

use c64::{C64, Model, SidChip};
use cia::CiaModel;
use tune::{Clock, Compatibility, SidModel, Tune};

pub use tune::detect;

/// The emulation is run in pieces of this many CPU cycles (about 5 ms), after which
/// the SID chips' samples are collected. (The agent asks for 5000 events at a time.)
const CYCLES_PER_CHUNK: i64 = 5000;

/// More events than this in one chunk cannot happen (the CPU, the VIC-II and the CIAs
/// have a handful of events per cycle at most): the bound that keeps a call of
/// `render` finite whatever the tune's code does.
const MAX_EVENTS_PER_CHUNK: u32 = CYCLES_PER_CHUNK as u32 * 64;

/// libsidplayfp's power-on delay: the number of 3-event steps the machine runs before
/// the tune is put in memory, on top of 8000. Upstream draws it at random by default
/// ("Delays <= MAX produce constant results"); here it is constant.
const POWER_ON_DELAY: u32 = 0;

/// Scale_Factor of SimpleMixer
const SCALE_FACTOR: i32 = 1 << 16;
const SQRT_2: f64 = 1.41421356237;
const SQRT_3: f64 = 1.73205080757;

/// A SID tune playing.
///
/// Settings, fixed (NostalgicPlayer's defaults): PAL unless the tune says NTSC; MOS 6581
/// unless the tune says 8580; old CIA (MOS 6526); filter on; no digi boost; reSIDfp's
/// two-pass sinc resampler straight to the output rate. A tune with a second or third SID
/// gets them, mixed as libsidplayfp does (first chip left, last chip right).
pub struct SidPlayer {
    c64: Box<C64>,
    tune: Tune,
    rate: u32,
    /// PAL/NTSC switch value
    video_switch: u8,
    /// the machine could not be set up for the selected song: silence
    dead: bool,
    /// interleaved stereo samples rendered and not handed out yet
    pending: Vec<i16>,
    pending_pos: usize,
}

impl SidPlayer {
    /// Loads a PSID/RSID file and starts its default song. `rate` is the output sample
    /// rate, 8000 to 192000 Hz.
    pub fn open(data: &[u8], rate: u32) -> Result<SidPlayer, String> {
        let tune = Tune::load(data)?;
        // Check for a sane sampling frequency
        if !(8000..=192000).contains(&rate) {
            return Err("Unsupported sampling frequency".into());
        }
        if tune.compatibility == Compatibility::Basic {
            return Err("this RSID tune needs the C64 BASIC ROM, which is not part of the player".into());
        }
        let mut p =
            SidPlayer { c64: Box::new(C64::new()), tune, rate, video_switch: 1, dead: false, pending: Vec::new(), pending_pos: 0 };
        p.config()?;
        Ok(p)
    }

    /// "PSID" or "RSID"
    pub fn format(&self) -> &'static str {
        self.tune.format
    }

    pub fn title(&self) -> &str {
        &self.tune.name
    }

    pub fn author(&self) -> &str {
        &self.tune.author
    }

    pub fn released(&self) -> &str {
        &self.tune.released
    }

    /// The header and what was made of it.
    pub fn tune(&self) -> &Tune {
        &self.tune
    }

    /// Number of songs in the file.
    pub fn subsongs(&self) -> u16 {
        self.tune.songs as u16
    }

    /// The song playing, 0-based.
    pub fn subsong(&self) -> u16 {
        (self.tune.current_song - 1) as u16
    }

    /// Starts song `n` (0-based) from its beginning.
    pub fn set_subsong(&mut self, n: u16) -> Result<(), String> {
        if n as u32 >= self.tune.songs {
            return Err(format!("no subsong {n}"));
        }
        self.tune.select_song(n as u32 + 1);
        self.config()
    }

    /// The CPU has stopped on a halt instruction: nothing more will happen (the SID
    /// chips go on with what they were doing).
    pub fn halted(&self) -> bool {
        self.dead || self.c64.cpu.halted
    }

    /// The SID models playing ("6581" / "8580"), the base chip first.
    pub fn sid_models(&self) -> Vec<&'static str> {
        self.c64
            .sids
            .iter()
            .map(|s| match s.chip_model {
                resid::ChipModel::Mos6581 => "6581",
                resid::ChipModel::Csg8580 => "8580",
            })
            .collect()
    }

    /// "PAL" or "NTSC", and "VBI" or "CIA": the machine and the song's speed source.
    pub fn clock(&self) -> (&'static str, &'static str) {
        (if self.video_switch == 1 { "PAL" } else { "NTSC" }, if self.tune.song_speed == tune::SPEED_CIA_1A { "CIA" } else { "VBI" })
    }

    /// Player.Config with a tune loaded: the SID chips, the machine's model, the
    /// sampling parameters, then the machine's start.
    fn config(&mut self) -> Result<(), String> {
        self.pending.clear();
        self.pending_pos = 0;
        self.dead = true;
        // SidRelease
        self.c64.clear_sids();

        let mut addresses = Vec::new();
        if self.tune.sid_chip_base(1) != 0 {
            addresses.push(self.tune.sid_chip_base(1));
        }
        if self.tune.sid_chip_base(2) != 0 {
            addresses.push(self.tune.sid_chip_base(2));
        }

        // SID emulation setup (must be performed before the
        // environment setup call)
        self.sid_create(&addresses)?;

        // Determine C64 model
        let model = self.c64_model();
        self.c64.set_model(model);
        self.c64.set_cia_model(CiaModel::Mos6526);

        // SidParams
        let cpu_freq = self.c64.get_main_cpu_speed();
        for s in &mut self.c64.sids {
            s.sampling(cpu_freq as f32, self.rate as f32, resid::SamplingMethod::Resample)?;
        }

        // Configure, setup and install C64 environment/events
        self.initialize()?;
        self.dead = false;
        Ok(())
    }

    /// Initialize the emulation
    fn initialize(&mut self) -> Result<(), String> {
        self.c64.reset();

        let size = self.tune.load_addr as u32 + self.tune.c64_data_len - 1;
        if size > 0xffff {
            return Err("SIDPLAYER ERROR: Size of music data exceeds C64 memory.".into());
        }

        let power_on_delay = POWER_ON_DELAY + 8000;

        // Run for ~[25000,50000] cycles
        for _ in 0..=power_on_delay {
            for _ in 0..3 {
                self.c64.clock();
            }
            self.c64.clock_sids();
            for chip in &mut self.c64.sids {
                chip.buffer_pos = 0;
            }
        }

        let driver = psiddrv::PsidDrv::drv_reloc(&self.tune)?;
        driver.install(&mut self.c64, &self.tune, self.video_switch);

        self.tune.place_sid_tune_in_c64_mem(&mut self.c64);

        self.c64.reset_cpu();
        Ok(())
    }

    /// Get the C64 model for the current loaded tune (defaultC64Model PAL, not forced)
    fn c64_model(&mut self) -> Model {
        match self.tune.clock_speed {
            // Use preferred speed if song speed is unknown
            Clock::Unknown | Clock::Any | Clock::Pal => {
                self.video_switch = 1;
                Model::PalB
            }
            Clock::Ntsc => {
                self.video_switch = 0;
                Model::NtscM
            }
        }
    }

    /// Get the SID model (defaultSidModel as given, not forced)
    fn get_sid_model(sid_model: SidModel, default_model: resid::ChipModel) -> resid::ChipModel {
        match sid_model {
            SidModel::Unknown | SidModel::Any => default_model,
            SidModel::Mos6581 => resid::ChipModel::Mos6581,
            SidModel::Mos8580 => resid::ChipModel::Csg8580,
        }
    }

    /// Create the SID emulation(s)
    fn sid_create(&mut self, extra_sid_addresses: &[u16]) -> Result<(), String> {
        // Setup base SID
        let mut default_model = resid::ChipModel::Mos6581;
        let user_model = Self::get_sid_model(self.tune.sid_model(0), default_model);
        let mut s = SidChip::new();
        s.model(user_model, false);
        self.c64.set_base_sid(s);

        // Setup extra SIDs if needed
        // If bits 6-7 are set to Unknown then the second SID will be set to the same SID
        // model as the first SID
        default_model = user_model;
        for (i, &address) in extra_sid_addresses.iter().enumerate() {
            let user_model = Self::get_sid_model(self.tune.sid_model(i + 1), default_model);
            let mut s = SidChip::new();
            s.model(user_model, false);
            if !self.c64.add_extra_sid(s, address) {
                return Err("SIDPLAYER ERROR: Unsupported SID address.".into());
            }
        }
        Ok(())
    }

    /// Runs the machine for a chunk and mixes what the SID chips gave into `pending`.
    fn run_chunk(&mut self) {
        let end = self.c64.time() + CYCLES_PER_CHUNK;
        let mut events = 0u32;
        while self.c64.time() < end {
            if !self.c64.clock() || events >= MAX_EVENTS_PER_CHUNK {
                // cannot happen: no event left, or more events than a chunk can hold
                self.dead = true;
                break;
            }
            events += 1;
        }
        // Clock the chips and get the buffers; the sample count is the same for all
        // chips
        self.c64.clock_sids();
        let sids = &mut self.c64.sids;
        let n = sids.iter().map(|s| s.buffer_pos.min(s.buffer.len())).min().unwrap_or(0);
        self.pending.clear();
        self.pending_pos = 0;
        self.pending.reserve(n * 2);

        // SimpleMixer, stereo. Channel matrix:
        //
        //   C1
        // L 1.0
        // R 1.0
        //
        //   C1    C2
        // L 1.0   0.5
        // R 0.5   1.0
        //
        //   C1    C2    C3
        // L 1.0   1.0   0.5
        // R 0.5   1.0   1.0
        let scale2 = ((1.0 / SQRT_2) * SCALE_FACTOR as f64) as i32;
        let scale3 = ((1.0 / SQRT_3) * SCALE_FACTOR as f64) as i32;
        match sids.len() {
            1 => {
                for &s in &sids[0].buffer[..n] {
                    self.pending.push(s);
                    self.pending.push(s);
                }
            }
            2 => {
                for i in 0..n {
                    let (a, b) = (sids[0].buffer[i] as f64, sids[1].buffer[i] as f64);
                    self.pending.push(((a + 0.5 * b) * scale2 as f64 / SCALE_FACTOR as f64) as i32 as i16);
                    self.pending.push(((0.5 * a + b) * scale2 as f64 / SCALE_FACTOR as f64) as i32 as i16);
                }
            }
            3 => {
                for i in 0..n {
                    let (a, b, c) = (sids[0].buffer[i] as f64, sids[1].buffer[i] as f64, sids[2].buffer[i] as f64);
                    self.pending.push(((a + b + 0.5 * c) * scale3 as f64 / SCALE_FACTOR as f64) as i32 as i16);
                    self.pending.push(((0.5 * a + b + c) * scale3 as f64 / SCALE_FACTOR as f64) as i32 as i16);
                }
            }
            _ => {}
        }
        // Reset the buffers
        for s in sids.iter_mut() {
            s.buffer_pos = 0;
        }
    }

    /// Fills `out` (interleaved stereo, 16 bit) and returns the number of frames written:
    /// always `out.len() / 2`. A tune has no end; silence follows if the machine could
    /// not be started.
    pub fn render_i16(&mut self, out: &mut [i16]) -> usize {
        let frames = out.len() / 2;
        let mut done = 0;
        let mut empty = 0;
        while done < frames {
            if self.pending_pos >= self.pending.len() {
                if self.dead {
                    out[done * 2..frames * 2].fill(0);
                    break;
                }
                self.run_chunk();
                if self.pending.is_empty() {
                    // a chunk is about 5 ms: it always holds samples
                    empty += 1;
                    self.dead |= empty > 4;
                    continue;
                }
                empty = 0;
            }
            let n = ((self.pending.len() - self.pending_pos) / 2).min(frames - done);
            out[done * 2..(done + n) * 2].copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + n * 2]);
            self.pending_pos += n * 2;
            done += n;
        }
        frames
    }

    /// `render_i16` as f32 in -1.0..1.0.
    pub fn render(&mut self, out: &mut [f32]) -> usize {
        let mut buf = vec![0i16; out.len() & !1];
        let frames = self.render_i16(&mut buf);
        for (o, &s) in out.iter_mut().zip(buf.iter()) {
            *o = s as f32 / 32768.0;
        }
        frames
    }
}

#[cfg(test)]
mod tests;
