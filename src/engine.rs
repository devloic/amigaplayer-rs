//! The DeliTracker core: owns the emulated machine and the loaded players,
//! identifies modules, drives the player call sequence and renders audio.

use crate::deli::{self, dtg, tag, Player};
use crate::eagle;
use crate::machine::Machine;
use crate::bus::FRAME;
use crate::paulaout::Filter;
use crate::protracker::ProTracker;
use anyhow::{anyhow, bail, Result};
use m68k::CpuType;
use std::path::Path;
#[cfg(feature = "host-68k")]
use std::path::PathBuf;

pub const SCOPE_LEN: usize = 512;

/// The NotePlayer genies played through when none was chosen, cheapest first (the
/// first with enough voices plays); players needing more voices than any has are left
/// to the built-in renderer.
#[cfg(feature = "host-68k")]
const NOTE_GENIES: &[&str] = &["4Voices-NotePlayer", "8Voices-NotePlayer", "32Voices-NotePlayer"];
use crate::bus::ECLOCK_DIV;
/// Players of formats from the PC, played without the Amiga's output filters.
const PC_FORMATS: &[&str] =
    &["669", "adpcm", "aiff", "wav", "riff", "mpeg", "mp3", "fasttracker", "screamtracker", "ps3m", "taketracker"];

pub enum Backend {
    /// index into `players` (custom modules are appended there too)
    Deli(usize),
    Native(Box<ProTracker>),
    /// an open replayer (crate::replay)
    Replay(Box<dyn crate::replay::Replayer>),
    /// a PC tracker module on the port of libxmp's player and mixer
    /// (crate::xmp), which renders stereo itself: no Paula, no Amiga filters
    Mix(Box<crate::xmp::XmpPlayer>),
    /// recorded audio (crate::aiff), played as it is: no Paula, no Amiga filters
    Pcm(Box<crate::aiff::AiffPlayer>),
}

pub struct Playing {
    pub backend: Backend,
    pub module_name: String,
    pub format: String,
    pub author: String,
    pub sub_min: u16,
    pub sub_max: u16,
    pub sub: u16,
    pub frames: u64,
    custom_player: bool,
}

impl Playing {
    /// The module is a custom module: its own 68k code plays it.
    pub fn is_custom(&self) -> bool {
        self.custom_player
    }
}

#[derive(Clone)]
pub struct Settings {
    pub volume: f32,       // 0..1
    pub separation: f32,   // 0 = mono, 1 = full Amiga hard panning
    pub a500_filter: bool, // fixed 4.4 kHz RC low-pass
    pub led_filter: bool,  // honour the LED filter
    pub mono: bool,
    /// Forces the filter model (`--filter`); None follows the format (`output_filter`).
    pub filter_model: Option<Filter>,
    /// Switches the LED filter on whatever the player does (`--led force`).
    pub led_force: bool,
    /// libxmp mixer interpolation (`--interp`).
    pub interp: crate::xmp::mixer::Interp,
    /// libxmp's resonant filter for the modules that use it (`--xmp-filter`).
    pub xmp_filter: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            volume: 0.8,
            separation: 0.7,
            a500_filter: true,
            led_filter: true,
            mono: false,
            filter_model: None,
            led_force: false,
            interp: crate::xmp::mixer::Interp::Linear,
            xmp_filter: true,
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl Biquad {
    fn lowpass(rate: f64, fc: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * fc / rate;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        Biquad {
            b0: ((1.0 - c) / 2.0 / a0) as f32,
            b1: ((1.0 - c) / a0) as f32,
            b2: ((1.0 - c) / 2.0 / a0) as f32,
            a1: (-2.0 * c / a0) as f32,
            a2: ((1.0 - alpha) / a0) as f32,
            ..Default::default()
        }
    }
    fn run(&mut self, ch: usize, x: f32) -> f32 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
        self.z2[ch] = self.b2 * x - self.a2 * y;
        y
    }
}

#[derive(Default, Clone, Copy)]
struct OnePole {
    a: f32,
    y: [f32; 2],
}

impl OnePole {
    fn new(rate: f64, fc: f64) -> Self {
        OnePole { a: (1.0 - (-2.0 * std::f64::consts::PI * fc / rate).exp()) as f32, y: [0.0; 2] }
    }
    fn run(&mut self, ch: usize, x: f32) -> f32 {
        self.y[ch] += self.a * (x - self.y[ch]);
        self.y[ch]
    }
}

pub struct Engine {
    pub m: Machine,
    pub players: Vec<Player>,
    pub playing: Option<Playing>,
    pub rate: f64,
    pub settings: Settings,
    pub paused: bool,
    pub last_error: Option<String>,
    scope_ring: Vec<Vec<f32>>,
    scope_pos: usize,
    a500: OnePole,
    led: Biquad,
    /// When the next vertical blank and DeliTracker timer interrupt are due
    /// (colour clocks).
    next_vblank: u64,
    next_timer: Option<u64>,
    /// Number of players found in the players directories (custom modules
    /// are appended after them and removed again).
    builtin_players: usize,
    /// Only this player (by name) is tried, and the built-in ProTracker is not.
    pub only_player: Option<String>,
    /// Play the formats the open replayers know with them, not the 68k players.
    pub open_replayers: bool,
    /// Where the players from `load_eagleplayers` begin.
    #[cfg_attr(not(feature = "host-68k"), allow(dead_code))]
    eagle_from: Option<usize>,
    /// DeliGenies (`load_genies`): external code that is not a player.
    pub genies: Vec<Player>,
    /// The NotePlayer genie chosen to play `_note` players (`use_genie`); without one
    /// the built-in renderer does.
    note_genie: Option<usize>,
    /// No NotePlayer genie was chosen: `note_genie` is the default one for modules
    /// with no more voices than it has, and none for the others.
    #[cfg_attr(not(feature = "host-68k"), allow(dead_code))]
    note_genie_auto: bool,
    /// The other genies chosen, told about each module (DTP_InitPlayer/DTP_EndPlayer).
    #[cfg_attr(not(feature = "host-68k"), allow(dead_code))]
    active_genies: Vec<usize>,
    /// The genies playing or told about the current module.
    running_genies: Vec<usize>,
    /// The guest heap's live allocations before the replayer was created (see `stop`).
    replay_mark: Option<Vec<u32>>,
    /// Guest memory for the NoteStruct a NotePlayer genie reads (0: not allocated yet).
    #[cfg_attr(not(feature = "host-68k"), allow(dead_code))]
    genie_mem: u32,
    /// INTENA as `audio_irqs` last saw it.
    intena_seen: u16,
    /// The current subsong has made a sound (see `skip_empty_subsong`).
    heard: bool,
    /// The LED filter's state at the first sample, and each change after it
    /// (rendered frame, new state), for the first few changes.
    pub led_log: Vec<(u64, bool)>,
    /// Every change, also those past `led_log`'s.
    led_changes: u64,
    led_seen: Option<bool>,
}

/// Colour clocks into the frame at which a 68k player starts. Any phase will do for a player
/// that does not race Paula; those that do (DMA off and on within a few colour clocks, as
/// Quadra Composer's) come out differently for each, and this one agrees with the originals
/// for most of the corpus's such files (found by trying phases against refcmp).
pub const START_PHASE: u64 = 100;

impl Engine {
    pub fn new(rate: f64) -> Self {
        let mut m = Machine::new(CpuType::M68020);
        // Paula's output stage at the rate asked for (the machine's own default is 48000 Hz:
        // left there, every other rate played slower or faster by the ratio of the two)
        m.bus.paula = crate::paula::Paula::with_output(rate as u32, Filter::None);
        deli::init_globals(&mut m);
        Engine {
            m,
            players: Vec::new(),
            playing: None,
            rate,
            settings: Settings::default(),
            paused: false,
            last_error: None,
            scope_ring: Vec::new(),
            scope_pos: 0,
            a500: OnePole::new(rate, 4420.0),
            led: Biquad::lowpass(rate, 3275.0, 0.707),
            next_vblank: FRAME,
            next_timer: None,
            builtin_players: 0,
            only_player: None,
            open_replayers: false,
            eagle_from: None,
            genies: Vec::new(),
            note_genie: None,
            note_genie_auto: false,
            active_genies: Vec::new(),
            running_genies: Vec::new(),
            replay_mark: None,
            genie_mem: 0,
            intena_seen: 0,
            heard: false,
            led_log: Vec::new(),
            led_changes: 0,
            led_seen: None,
        }
    }

    /// Departs from the Amiga where UADE does, to compare with it: 312 lines a
    /// frame, Paula's volume 64 as 63 and periods below 16 as 16, hard panning
    /// and full volume.
    pub fn set_uade_compat(&mut self) {
        self.m.bus.frame = 312 * crate::paula::LINE;
        self.m.bus.paula.set_uade_quirks(true);
        self.settings.separation = 1.0;
        self.settings.volume = 1.0;
    }

    #[cfg(feature = "host-68k")]
    /// Loads the DeliGenies under `dir`. Like Eagleplayers they go to the top of the
    /// heap, so that loading them does not move the memory players and modules get.
    pub fn load_genies(&mut self, dir: &Path) -> usize {
        let mut files = Vec::new();
        collect_files(dir, &mut files, 2);
        files.sort();
        self.m.alloc_top = true;
        for f in files {
            let Ok(data) = std::fs::read(&f) else { continue };
            if !crate::hunk::is_hunk_file(&data) {
                continue;
            }
            match deli::load_player(&mut self.m, &data, &f) {
                Ok(g) if !g.is_player() => {
                    log::info!("genie: {} v{}.{} ({})", g.name, g.version >> 16, g.version & 0xFFFF, f.display());
                    if let Some(pp) = deli::PlayerProc::new(&g.tags) {
                        self.m.deli.procs.push(pp);
                    }
                    self.genies.push(g);
                }
                Ok(p) => log::debug!("{}: a player, not a genie", p.name),
                Err(e) => log::debug!("{}: {e}", f.display()),
            }
        }
        self.m.alloc_top = false;
        if self.note_genie.is_none() {
            // DeliTracker's default NotePlayer, which plays the notes on Paula
            self.note_genie = self.default_note_genie(0);
            self.note_genie_auto = self.note_genie.is_some();
        }
        self.genies.len()
    }

    #[cfg(feature = "host-68k")]
    /// Chooses a genie by name: a NotePlayer genie plays `_note` players from now on, any
    /// other is told about each module. `builtin` has the built-in renderer play them.
    pub fn use_genie(&mut self, name: &str) -> Result<()> {
        if name.eq_ignore_ascii_case("builtin") {
            (self.note_genie, self.note_genie_auto) = (None, false);
            return Ok(());
        }
        let Some(i) = self.genies.iter().position(|g| g.name.eq_ignore_ascii_case(name)) else {
            let names: Vec<&str> = self.genies.iter().map(|g| g.name.as_str()).collect();
            bail!("no genie \"{name}\" (loaded: {})", names.join(", "));
        };
        if self.genies[i].has(tag::NOTE_PLAYER) {
            (self.note_genie, self.note_genie_auto) = (Some(i), false);
        } else if !self.active_genies.contains(&i) {
            self.active_genies.push(i);
        }
        Ok(())
    }

    #[cfg(feature = "host-68k")]
    /// The NotePlayer genie for a player with `voices` voices: DeliTracker picks the
    /// cheapest one that has enough (CPU use first, so the Paula based 4 and 8 voice
    /// genies before the software mixer of the 32 voice one).
    fn default_note_genie(&self, voices: u32) -> Option<usize> {
        NOTE_GENIES.iter().find_map(|name| {
            let i = self.genies.iter().position(|g| g.name.eq_ignore_ascii_case(name))?;
            let r = self.genies[i].tag(tag::NOTE_PLAYER).filter(|&r| r != 0)?;
            (self.m.bus.r16(r) as u32 + self.m.bus.r16(r + 2) as u32 >= voices).then_some(i)
        })
    }

    /// Calls one of a running genie's functions; a genie that fails is dropped.
    fn genie_call(&mut self, i: usize, t: u32) {
        let Some(f) = self.genies[i].tag(t) else { return };
        if let Err(e) = deli::call_fn(&mut self.m, self.genies[i].globals, f, None) {
            log::warn!("genie {} failed: {e}", self.genies[i].name);
            self.running_genies.retain(|&g| g != i);
            if self.note_genie == Some(i) {
                self.m.deli.note_signal = 0;
        self.m.deli.genie_notes = None;
            }
        }
    }

    #[cfg(feature = "host-68k")]
    /// The module started (after the player's InitPlayer): the genies hear of it. A
    /// NotePlayer genie gets the player's NoteStruct and takes over its notes.
    fn start_genies(&mut self) {
        self.running_genies.clear();
        self.m.deli.note_signal = 0;
        let ns = self.m.deli.note_struct;
        if self.note_genie_auto && ns != 0 {
            let mut voices = 0;
            let mut ch = self.m.bus.r32(ns);
            while ch != 0 && voices < crate::notes::GENIE_CHANNELS as u32 {
                voices += 1;
                ch = self.m.bus.r32(ch);
            }
            log::debug!("player has {voices} note channels");
            self.note_genie = self.default_note_genie(voices);
        }
        if let (Some(i), ns) = (self.note_genie, ns) {
            if ns != 0 {
                if self.genie_mem == 0 {
                    self.m.alloc_top = true;
                    self.genie_mem = self.m.alloc(crate::notes::GENIE_NOTES_SIZE);
                    self.m.alloc_top = false;
                }
                let g = &self.genies[i];
                let record = g.tag(tag::NOTE_PLAYER).unwrap_or(0);
                if self.genie_mem != 0 && record != 0 {
                    let gn = crate::notes::GenieNotes::new(&mut self.m.bus, self.genie_mem, record, ns);
                    if let Some(info) = g.tag(tag::NOTE_INFO) {
                        self.m.bus.w32(info, gn.ns);
                    }
                    self.m.deli.genie_notes = Some(gn);
                }
                let (f, globals) = (g.tag(tag::INIT_PLAYER), g.globals);
                let ok = match f {
                    Some(f) => match deli::call_fn(&mut self.m, globals, f, None) {
                        Ok(r) => r[0] == 0,
                        Err(e) => {
                            log::warn!("genie {}: {e}", self.genies[i].name);
                            false
                        }
                    },
                    None => true,
                };
                if ok {
                    self.m.deli.note_signal = self.genies[i].tag(tag::NOTE_SIGNAL).unwrap_or(0);
                    self.running_genies.push(i);
                    log::info!("notes played by genie {}", self.genies[i].name);
                } else {
                    log::warn!("genie {} refused the module; the built-in renderer plays it", self.genies[i].name);
                }
            }
        }
        for i in self.active_genies.clone() {
            self.running_genies.push(i);
            self.genie_call(i, tag::INIT_PLAYER);
        }
    }

    #[cfg(feature = "host-68k")]
    /// Loads every DeliPlayer found (recursively) under `dir`.
    pub fn load_players(&mut self, dir: &Path) -> usize {
        self.m.os.progdir = dir.parent().unwrap_or(dir).to_path_buf();
        self.load_dir(dir)
    }

    #[cfg(feature = "host-68k")]
    /// Loads every Eagleplayer found under `dir`. They are tried last, after the
    /// players loaded before them and the SoundTracker heuristic, so call this
    /// after `load_players`.
    pub fn load_eagleplayers(&mut self, dir: &Path) -> usize {
        self.eagle_from.get_or_insert(self.players.len());
        self.m.os.env_dir = dir.join("ENV");
        self.m.alloc_top = true;
        let n = self.load_dir(dir);
        self.m.alloc_top = false;
        n
    }

    #[cfg(feature = "host-68k")]
    fn load_dir(&mut self, dir: &Path) -> usize {
        let first = self.players.len();
        let mut files = Vec::new();
        collect_files(dir, &mut files, 2);
        files.sort();
        let mut n = 0;
        for f in files {
            let Ok(data) = std::fs::read(&f) else { continue };
            if !crate::hunk::is_hunk_file(&data) {
                continue;
            }
            match deli::load_player(&mut self.m, &data, &f) {
                Ok(p) if p.is_player() || (std::env::var_os("DELI_CHECKLESS").is_some() && (p.has(tag::INTERRUPT) || p.has(tag::START_INT))) => {
                    let kind = if p.eagle { "eagleplayer" } else { "player" };
                    log::info!("{kind}: {} v{}.{} ({})", p.name, p.version >> 16, p.version & 0xFFFF, f.display());
                    if let Some(pp) = deli::PlayerProc::new(&p.tags) {
                        self.m.deli.procs.push(pp);
                    }
                    self.players.push(p);
                    n += 1;
                }
                Ok(p) => log::debug!("skipping genie {}", p.name),
                Err(e) => log::debug!("{}: {e}", f.display()),
            }
        }
        // a later version of the same player (from the same directory)
        // replaces the earlier one
        let mut keep: Vec<Player> = Vec::new();
        for p in self.players.drain(first..) {
            if let Some(o) = keep.iter_mut().find(|o| o.name == p.name) {
                if p.version > o.version {
                    *o = p;
                }
            } else {
                keep.push(p);
            }
        }
        self.players.extend(keep);
        self.builtin_players = self.players.len();
        n
    }

    #[cfg(feature = "host-68k")]
    fn configure(&mut self, idx: usize) -> Result<()> {
        if self.players[idx].configured {
            return Ok(());
        }
        self.players[idx].configured = true;
        if let Some(cfg) = self.players[idx].tag(tag::CONFIG) {
            let r = deli::call_fn(&mut self.m, self.players[idx].globals, cfg, None).map_err(|e| anyhow!(e))?;
            // Eagleplayer ignores the result: SoundMon's Config returns the
            // counter its copy loop leaves in D0
            if r[0] != 0 && !self.players[idx].eagle {
                bail!("Config of {} failed", self.players[idx].name);
            }
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        let Some(p) = self.playing.take() else { return };
        if let Backend::Deli(i) = p.backend {
            let pl = &self.players[i];
            let (stop_int, end_sound, end_player) =
                (pl.tag(tag::STOP_INT), pl.tag(tag::END_SOUND), pl.tag(tag::END_PLAYER));
            let (has_int, g) = (pl.timer_driven(), pl.globals);
            if !has_int {
                if let Some(f) = stop_int {
                    let _ = deli::call_fn(&mut self.m, g, f, None);
                }
            }
            self.m.deli.timer_running = false;
            for f in [end_sound, end_player].into_iter().flatten() {
                let _ = deli::call_fn(&mut self.m, g, f, None);
            }
            for i in std::mem::take(&mut self.running_genies) {
                let ts: &[u32] = if Some(i) == self.note_genie {
                    &[tag::STOP_INT, tag::END_SOUND, tag::END_PLAYER]
                } else {
                    &[tag::END_PLAYER]
                };
                self.running_genies.push(i);
                for &t in ts {
                    self.genie_call(i, t);
                }
                self.running_genies.retain(|&g| g != i);
            }
            self.m.deli.note_signal = 0;
            if p.custom_player {
                self.players.truncate(self.builtin_players);
            }
        }
        self.m.bus.paula.write_dmacon(0x000F);
        self.m.bus.paula.write_intena(0x0780);
        if let Some(a) = self.m.bus.amp.take() {
            self.m.heap.free(a.buf);
        }
        self.next_timer = None;
        self.m.os.audio = Default::default();
        self.m.os.audio_alloc = Default::default();
        self.m.deli.voices.reset();
        self.m.deli.note_struct = 0;
        eagle::stop(&mut self.m);
        for (a, _) in std::mem::take(&mut self.m.deli.list) {
            self.m.heap.free(a);
        }
        if let Some(mark) = self.replay_mark.take() {
            self.m.heap.free_since(&mark);
        }
        self.m.os.int_vectors = [0; 16];
        self.m.os.int_servers = Default::default();
        self.m.os.icr_vectors = [[0; 5]; 2];
        self.m.bus.ciab = Default::default();
        // the LED (CIA-A PRA bit 1) as UADE leaves it for a player that never touches it: off
        // (the filter is bypassed), not as the last module left it
        self.m.bus.ciaa.pra = crate::cia::Cia::default().pra | 2;
    }

    /// Loads and starts playing a module file.
    pub fn play_file(&mut self, path: &Path) -> Result<()> {
        let data = std::fs::read(path)?;
        self.play_data(path, data)
    }

    /// Play a module already in memory. `path` still matters: its directory is where
    /// players look for companion files, and its name is what name-based checks see.
    pub fn play_data(&mut self, path: &Path, data: Vec<u8>) -> Result<()> {
        let r = self.start_module(path, data);
        self.song_starts();
        r
    }

    /// The song starts now: the time its player took to get ready (its precalculation
    /// takes seconds on a 68000) passed while it loaded, not while it played.
    fn song_starts(&mut self) {
        self.m.bus.paula.sync(self.m.bus.clock, &self.m.bus.ram);
        self.m.bus.paula.align_to_start(self.m.bus.clock);
        self.m.bus.paula.samples.clear();
        self.heard = false;
        self.led_log.clear();
        self.led_changes = 0;
        self.led_seen = None;
    }

    /// A subsong that ends within its first seconds without a sound (an empty
    /// one: Richard Joseph songs often start with one) is skipped for the next,
    /// as UADE and DeliTracker go on to the next subsong at a song end.
    fn skip_empty_subsong(&mut self) {
        const EMPTY_SECS: f64 = 2.0;
        let Some(p) = &self.playing else { return };
        if self.heard || p.sub >= p.sub_max || matches!(p.backend, Backend::Native(_)) {
            return;
        }
        if self.elapsed_secs() < EMPTY_SECS && self.songend() {
            if let Err(e) = self.change_subsong(1) {
                log::debug!("next subsong: {e}");
            }
        }
    }

    fn start_module(&mut self, path: &Path, data: Vec<u8>) -> Result<()> {
        self.stop();
        self.last_error = None;
        self.m.deli.songend = false;
        // AIFF / AIFF-C recordings are played by our own decoder, from the file itself
        // (not copied into guest memory); an unsupported compression is an error, not a
        // reason to try the other players
        if self.only_player.is_none() && crate::aiff::detect(&data) {
            let x = crate::aiff::AiffPlayer::open(data, self.rate as u32).map_err(|e| anyhow!(e))?;
            return self.start_pcm(Box::new(x), path);
        }
        let data = crate::decrunch::decrunch(data);
        // (a TimeTracker song is the open replayer's own under --open-replayers, not the ProWizard conversion's)
        let own = self.open_replayers && crate::replay::timetracker::is_timetracker(&data);
        let data = if self.only_player.is_some() || own { data } else { crate::prowizard::convert(data, path) };
        if data.is_empty() {
            bail!("empty file");
        }
        // module into guest memory (padded so 1k checks never read garbage)
        let size = data.len() as u32;
        let addr = self.m.alloc(size.max(1024) + 1024);
        if addr == 0 {
            bail!("module too large");
        }
        self.m.bus.write_bytes(addr, &data);
        log::debug!("module at ${addr:06x}, {size} bytes");
        self.m.deli.list = vec![(addr, size)];
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut dir_s = dir.to_string_lossy().to_string();
        if !dir_s.is_empty() && !dir_s.ends_with('/') {
            dir_s.push('/');
        }
        let file_s = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let (db, fb, pb) = (self.m.deli.dir_buf, self.m.deli.file_buf, self.m.deli.path_buf);
        self.m.bus.write_cstr(db, &dir_s);
        self.m.bus.write_cstr(fb, &file_s);
        self.m.bus.write_cstr(pb, "");
        self.m.os.module_dir = dir.clone();
        self.m.deli.module_dir = dir;
        for g in [self.m.deli.globals, self.m.deli.eagle.globals] {
            if g != 0 {
                self.m.bus.w32(g + dtg::CHK_DATA, addr);
                self.m.bus.w32(g + dtg::CHK_SIZE, size);
                self.m.bus.w16(g + dtg::SND_NUM, 0);
                self.m.bus.w16(g + dtg::TIMER, deli::DEFAULT_TIMER);
            }
        }

        #[cfg(feature = "host-68k")]
        {
        if let Some(want) = self.only_player.clone() {
            let found = (0..self.builtin_players).find(|&i| self.players[i].name.eq_ignore_ascii_case(&want));
            if let Some(idx) = found {
                if self.check_player(idx)? {
                    return self.start_deli(idx, path, false);
                }
            }
            self.m.deli.list.clear();
            self.m.heap.free(addr);
            match found {
                Some(_) => bail!("{want} does not recognise this module"),
                None => bail!("no player named {want}"),
            }
        }
        }
        // 1. the built-in ProTracker for 31-instrument modules
        // (a StarTrekker-tagged module with an AudioSculpture companion beside it is not one)
        if let Some(fmt) = ProTracker::detect(&data).filter(|f| *f != "SoundTracker").filter(|f| {
            !(self.open_replayers && *f == "StarTrekker" && crate::replay::audiosculpture::takes_over(&self.m, &data))
        }) {
            return self.start_native(path, &data, addr, fmt);
        }
        if self.open_replayers {
            // (the format recognised by its file name needs it to be told)
            crate::replay::adpcmmono::set_name(&file_name(path));
            let found = crate::replay::REPLAYERS.iter().find(|o| (o.detect)(&data));
            crate::replay::adpcmmono::set_name("");
            if let Some(o) = found {
                // a replayer's InitSound may write Paula's registers, as the 68k
                // players' do after start_deli's reset; it starts at the same phase
                self.align_start();
                self.m.bus.paula.reset();
                // (what the replayer allocates is freed again at stop, or here if it declines)
                let mark = self.m.heap.mark();
                // the module's path in the host's path buffer, as the host of a 68k player leaves
                // it before the player starts (start_deli): it lies in low memory, below the
                // module, where a voice that runs on past a one-shot sample (LOC 0, LEN 0) or a
                // sample start moved out of the module plays it
                let full = self.m.bus.read_cstr(self.m.deli.dir_buf) + &self.m.bus.read_cstr(self.m.deli.file_buf);
                self.m.bus.write_cstr(self.m.deli.path_buf, &full);
                if let Some(r) = (o.new)(&data, addr, &mut self.m) {
                    self.replay_mark = Some(mark);
                    return self.start_replay(r, path);
                }
                self.m.heap.free_since(&mark);
            }
            // the PC tracker formats of libxmp's loaders (those the open chain
            // takes: see xmp::LEFT_TO_OTHER_PLAYERS)
            if let Some(x) =
                crate::xmp::detect_open(&data).and_then(|_| crate::xmp::XmpPlayer::open(&data, self.rate as u32, Some(path)))
            {
                self.m.deli.list.clear();
                self.m.heap.free(addr);
                return self.start_mix(Box::new(x), path);
            }
        }
        #[cfg(feature = "host-68k")]
        let eagle_from = self.eagle_from.unwrap_or(self.builtin_players);
        #[cfg(feature = "host-68k")]
        {
        // 2. a custom module carries its own player
        if crate::hunk::is_hunk_file(&data) {
            if let Ok(p) = deli::load_player(&mut self.m, &data, path) {
                if p.is_custom() {
                    self.players.push(p);
                    let idx = self.players.len() - 1;
                    return self.start_deli(idx, path, true);
                }
            }
        }
        // 3. external players
        for idx in 0..eagle_from {
            if by_name_only(&self.players[idx].name, path) && self.check_player(idx)? {
                log::info!("{} recognised by {}", path.display(), self.players[idx].name);
                return self.start_deli(idx, path, false);
            }
        }
        }
        // 4. the 15-instrument SoundTracker heuristic
        if ProTracker::detect(&data) == Some("SoundTracker") {
            return self.start_native(path, &data, addr, "SoundTracker");
        }
        #[cfg(feature = "host-68k")]
        {
        // 5. last resort: the Eagleplayers, whose checks are often looser
        for idx in eagle_from..self.builtin_players {
            if by_name_only(&self.players[idx].name, path) && self.check_player(idx)? {
                log::info!("{} recognised by {}", path.display(), self.players[idx].name);
                return self.start_deli(idx, path, false);
            }
        }
        }
        self.m.deli.list.clear();
        self.m.heap.free(addr);
        bail!("unknown module format")
    }

    #[cfg(feature = "host-68k")]
    fn check_player(&mut self, idx: usize) -> Result<bool> {
        // every player is checked from the start phase with Paula just reset, so the one that
        // takes the module runs on the same timeline whatever was checked before it (and the
        // open replayers, which align and reset before their start, on the same as theirs)
        self.align_start();
        self.m.bus.paula.reset();
        if let Err(e) = self.configure(idx) {
            log::debug!("{e}");
            return Ok(false);
        }
        let Some(chk) = self.players[idx].check() else { return Ok(std::env::var_os("DELI_CHECKLESS").is_some()) };
        match deli::call_fn(&mut self.m, self.players[idx].globals, chk, None) {
            Ok(r) => Ok(r[0] == 0),
            Err(e) => {
                // a player whose check crashes is not asked again
                log::info!("check of {} crashed, disabling it: {e}", self.players[idx].name);
                for t in [tag::CHECK1, tag::CHECK2, eagle::tag::CHECK3, eagle::tag::CHECK5] {
                    self.players[idx].tags.remove(&t);
                }
                Ok(false)
            }
        }
    }

    fn start_native(&mut self, path: &Path, data: &[u8], addr: u32, fmt: &str) -> Result<()> {
        let pt = ProTracker::new(data, addr).ok_or_else(|| anyhow!("bad module"))?;
        pt.clear_loop_words(&mut self.m.bus.ram);
        self.m.bus.paula.reset();
        self.intena_seen = 0;
        self.next_timer = None;
        let name = if pt.title.is_empty() { file_name(path) } else { pt.title.clone() };
        self.playing = Some(Playing {
            backend: Backend::Native(Box::new(pt)),
            module_name: name,
            format: fmt.to_string(),
            author: String::new(),
            sub_min: 0,
            sub_max: 0,
            sub: 0,
            frames: 0,
            custom_player: false,
        });
        Ok(())
    }

    /// Moves the clock on to `START_PHASE` into the next video frame (and E clock period),
    /// where a 68k player or an open replayer starts.
    fn align_start(&mut self) {
        self.m.bus.clock = self.aligned(self.m.bus.clock);
        self.m.bus.paula.sync(self.m.bus.clock, &self.m.bus.ram);
    }

    /// The first clock at `START_PHASE` into a video frame (and E clock period) not before `t`.
    fn aligned(&self, t: u64) -> u64 {
        let q = self.m.bus.frame * ECLOCK_DIV;
        t.saturating_sub(START_PHASE).div_ceil(q) * q + START_PHASE
    }

    fn start_replay(&mut self, r: Box<dyn crate::replay::Replayer>, path: &Path) -> Result<()> {
        self.intena_seen = 0;
        self.next_timer = None;
        let title = r.title();
        let (sub_min, sub_max, sub) = r.subsongs();
        let mut r = r;
        // a replayer that knows when its first tick falls (before the timer period it would
        // get otherwise)
        if let Some(t) = r.subsong_tick_at() {
            self.next_timer = Some(t);
        }
        self.playing = Some(Playing {
            module_name: if title.is_empty() { file_name(path) } else { title },
            format: r.format().to_string(),
            backend: Backend::Replay(r),
            author: String::new(),
            sub_min,
            sub_max,
            sub,
            frames: 0,
            custom_player: false,
        });
        Ok(())
    }

    fn start_mix(&mut self, mut x: Box<crate::xmp::XmpPlayer>, path: &Path) -> Result<()> {
        x.set_mixer(self.settings.interp, self.settings.xmp_filter);
        self.m.bus.paula.reset();
        self.intena_seen = 0;
        self.next_timer = None;
        let title = x.title().to_string();
        self.playing = Some(Playing {
            module_name: if title.is_empty() { file_name(path) } else { title },
            format: x.format().to_string(),
            sub_min: 0,
            sub_max: (x.subsongs() - 1) as u16,
            sub: 0,
            backend: Backend::Mix(x),
            author: String::new(),
            frames: 0,
            custom_player: false,
        });
        Ok(())
    }

    fn start_pcm(&mut self, x: Box<crate::aiff::AiffPlayer>, path: &Path) -> Result<()> {
        self.m.bus.paula.reset();
        self.intena_seen = 0;
        self.next_timer = None;
        let title = x.title().to_string();
        self.playing = Some(Playing {
            module_name: if title.is_empty() { file_name(path) } else { title },
            format: x.format().to_string(),
            sub_min: 0,
            sub_max: 0,
            sub: 0,
            backend: Backend::Pcm(x),
            author: String::new(),
            frames: 0,
            custom_player: false,
        });
        Ok(())
    }

    #[cfg(feature = "host-68k")]
    fn start_deli(&mut self, idx: usize, path: &Path, custom: bool) -> Result<()> {
        if custom {
            self.configure(idx)?;
        }
        // The player starts at a fixed phase of the video frame and the E clock whatever ran
        // before (other players' checks take arbitrary time): Paula's line grid, the beam
        // and the vertical blank are then the same whichever way the player was chosen, so
        // register writes that race the audio state machine (DMA off and on within a few
        // colour clocks) come out the same
        // a custom module's player has no check, which aligns and resets Paula for the others
        if custom {
            self.align_start();
            self.m.bus.paula.reset();
        }
        self.intena_seen = 0;
        self.next_timer = None;
        self.m.deli.voices.reset();
        let g = self.players[idx].globals;
        let pl = &self.players[idx];
        if pl.eagle {
            let size = self.m.bus.r32(g + dtg::CHK_SIZE);
            eagle::prepare(&mut self.m, pl.tag_list, size);
        }
        if let Some(f) = pl.tag(tag::EXT_LOAD) {
            // the module's path, as UADE's score leaves it (copy_module_name):
            // IFF-SMUS opens the score through it; players that build a path
            // clear it first, as CopyDir appends
            let full = self.m.bus.read_cstr(self.m.deli.dir_buf) + &self.m.bus.read_cstr(self.m.deli.file_buf);
            self.m.bus.write_cstr(self.m.deli.path_buf, &full);
            let r = deli::call_fn(&mut self.m, g, f, None).map_err(|e| anyhow!(e))?;
            if r[0] != 0 {
                bail!("{}: could not load additional files", self.players[idx].name);
            }
        }
        // DeliTracker leaves the module's path here: IFF-8SVX opens it to stream the sample
        let full = self.m.bus.read_cstr(self.m.deli.dir_buf) + &self.m.bus.read_cstr(self.m.deli.file_buf);
        self.m.bus.write_cstr(self.m.deli.path_buf, &full);
        let pl = &self.players[idx];
        let init = pl.tag(tag::INIT_PLAYER);
        let ssr = pl.tag(tag::SUBSONG_RANGE);
        if let Some(f) = init {
            let r = deli::call_fn(&mut self.m, g, f, None).map_err(|e| anyhow!(e))?;
            if r[0] != 0 {
                bail!("InitPlayer of {} failed", self.players[idx].name);
            }
        }
        // the amplifier comes after InitPlayer (see score.s)
        if let Some(f) = self.players[idx].tag(eagle::tag::INIT_AMPLIFIER) {
            let r = deli::call_fn(&mut self.m, g, f, None).map_err(|e| anyhow!(e))?;
            if r[0] != 0 {
                log::warn!("InitAmplifier of {} failed ({})", self.players[idx].name, r[0] as i32);
            } else {
                eagle::amplifier_ready(&mut self.m, &self.players[idx].name);
            }
        }
        let info = self.module_info(idx);
        let mi = |m: &Machine, t| info.and_then(|l| eagle::find_tag(&m.bus, l, t));
        // subsong range (InitPlayer usually fills the NewSubSongRange array)
        let pl = &self.players[idx];
        let mut ranged = pl.has(tag::SUBSONG_RANGE) || pl.has(tag::NEW_SUBSONG_RANGE);
        let (def, lo, mut hi) = if let Some(f) = ssr {
            let r = deli::call_fn(&mut self.m, g, f, None).map_err(|e| anyhow!(e))?;
            (r[0] as u16, r[0] as u16, r[1] as u16)
        } else if let Some(r) = pl.tag(tag::NEW_SUBSONG_RANGE) {
            (self.m.bus.r16(r), self.m.bus.r16(r + 2), self.m.bus.r16(r + 4))
        } else {
            let n = self.m.bus.r16(g + dtg::SND_NUM);
            // an Eagleplayer may give the count in EPG_SubSongs or MI_SubSongs
            let (first, count) = if pl.eagle {
                (self.m.bus.r16(g + eagle::epg::FIRST_SND), self.m.bus.r16(g + eagle::epg::SUB_SONGS))
            } else {
                (0xFFFF, 0xFFFF)
            };
            let count = Some(count).filter(|&c| c != 0xFFFF).or(mi(&self.m, eagle::mi::SUB_SONGS).map(|c| c as u16));
            match count.filter(|&c| (2..0x8000).contains(&c)) {
                Some(c) => {
                    ranged = true;
                    let first = if first == 0xFFFF { n } else { first };
                    (first, first, (first as u32 + c as u32).wrapping_sub(1) as u16)
                }
                None => (n, n, n),
            }
        };
        if hi < lo {
            hi = lo;
        }
        let pl = &self.players[idx];
        if ranged {
            self.m.bus.w16(g + dtg::SND_NUM, def);
        }
        if let Some(ns) = pl.tag(tag::NOTE_STRUCT) {
            self.m.deli.note_struct = self.m.bus.r32(ns);
        }
        let init_note = pl.tag(tag::INIT_NOTE);
        let s = |m: &Machine, t: Option<u32>| t.map(|a| m.bus.r32(a)).filter(|&a| a != 0).map(|a| m.bus.read_cstr(a));
        let mi_s = |m: &Machine, t| mi(m, t).filter(|&a| a != 0).map(|a| m.bus.read_cstr(a)).filter(|s| !s.is_empty());
        let module_name = s(&self.m, pl.tag(tag::MODULE_NAME))
            .or_else(|| mi_s(&self.m, eagle::mi::SONG_NAME))
            .unwrap_or_else(|| file_name(path));
        let author =
            s(&self.m, pl.tag(tag::AUTHOR_NAME)).or_else(|| mi_s(&self.m, eagle::mi::AUTHOR_NAME)).unwrap_or_default();
        let format = if custom { "Custom".to_string() } else { pl.name.clone() };
        if let Some(f) = init_note {
            deli::call_fn(&mut self.m, g, f, None).map_err(|e| anyhow!(e))?;
        }
        self.start_genies();
        self.playing = Some(Playing {
            backend: Backend::Deli(idx),
            module_name,
            format,
            author,
            sub_min: lo,
            sub_max: hi,
            sub: def,
            frames: 0,
            custom_player: custom,
        });
        self.start_sound(idx)
    }

    #[cfg(feature = "host-68k")]
    /// An Eagleplayer's ModuleInfo tag list, once InitPlayer has filled it.
    fn module_info(&mut self, idx: usize) -> Option<u32> {
        let pl = &self.players[idx];
        if let Some(l) = pl.tag(eagle::tag::NEW_MODULE_INFO) {
            return Some(l);
        }
        let f = pl.tag(eagle::tag::GET_MODULE_INFO)?;
        let r = deli::call_fn(&mut self.m, pl.globals, f, None).ok()?;
        Some(r[8]).filter(|&a| a != 0)
    }

    fn start_sound(&mut self, idx: usize) -> Result<()> {
        let g = self.players[idx].globals;
        let pl = &self.players[idx];
        let (vol, bal, init_sound, start_int, has_int) = (
            pl.tag(tag::VOLUME),
            pl.tag(tag::BALANCE),
            pl.tag(tag::INIT_SOUND),
            pl.tag(tag::START_INT),
            pl.timer_driven(),
        );
        for f in [vol, bal].into_iter().flatten() {
            deli::call_fn(&mut self.m, g, f, None).map_err(|e| anyhow!(e))?;
        }
        if let Some(f) = init_sound {
            deli::call_fn(&mut self.m, g, f, None).map_err(|e| anyhow!(e))?;
        }
        // the NotePlayer genie plays along
        if let Some(i) = self.note_genie.filter(|i| self.running_genies.contains(i)) {
            for t in [tag::VOLUME, tag::BALANCE, tag::INIT_SOUND, tag::START_INT] {
                self.genie_call(i, t);
            }
        }
        if has_int {
            self.m.deli.timer_running = true;
        } else if let Some(f) = start_int {
            deli::call_fn(&mut self.m, g, f, None).map_err(|e| anyhow!(e))?;
        }
        // the amplifier's channels play whatever the player did to Paula
        crate::amplifier::restart(&mut self.m.bus);
        Ok(())
    }

    fn stop_sound(&mut self, idx: usize) {
        if let Some(i) = self.note_genie.filter(|i| self.running_genies.contains(i)) {
            for t in [tag::STOP_INT, tag::END_SOUND] {
                self.genie_call(i, t);
            }
        }
        let pl = &self.players[idx];
        let (stop_int, end_sound, has_int) = (pl.tag(tag::STOP_INT), pl.tag(tag::END_SOUND), pl.timer_driven());
        if has_int {
            self.m.deli.timer_running = false;
        } else if let Some(f) = stop_int {
            let _ = deli::call_fn(&mut self.m, self.players[idx].globals, f, None);
        }
        if let Some(f) = end_sound {
            let _ = deli::call_fn(&mut self.m, self.players[idx].globals, f, None);
        }
        self.m.bus.paula.write_dmacon(0x000F);
        if let Some(a) = &mut self.m.bus.amp {
            a.all_off();
        }
        self.m.deli.voices.reset();
    }

    /// Changes subsong by `delta` (-1/+1).
    pub fn change_subsong(&mut self, delta: i32) -> Result<()> {
        let Some(p) = &self.playing else { return Ok(()) };
        let Backend::Deli(idx) = p.backend else {
            match &mut self.playing {
                Some(Playing { backend: Backend::Native(pt), .. }) => pt.jump(delta),
                Some(Playing { backend: Backend::Replay(r), sub, frames, .. }) => {
                    let (lo, hi, cur) = r.subsongs();
                    let n = cur as i32 + delta;
                    if n < lo as i32 || n > hi as i32 {
                        return Ok(());
                    }
                    self.m.bus.paula.write_dmacon(0x000F);
                    if let Some(a) = &mut self.m.bus.amp {
                        a.all_off();
                    }
                    r.set_subsong(n as u16, &mut crate::replay::Hw { bus: &mut self.m.bus });
                    if let Some(t) = r.subsong_tick_at() {
                        self.next_timer = Some(t);
                    }
                    crate::amplifier::restart(&mut self.m.bus);
                    (*sub, *frames) = (n as u16, 0);
                    self.song_starts();
                }
                Some(Playing { backend: Backend::Mix(x), sub, frames, .. }) => {
                    let n = x.subsong() as i32 + delta;
                    if n < 0 || n >= x.subsongs() as i32 {
                        return Ok(());
                    }
                    x.start(n as usize);
                    (*sub, *frames) = (n as u16, 0);
                    self.heard = false;
                }
                _ => {}
            }
            return Ok(());
        };
        let (lo, hi, cur) = (p.sub_min, p.sub_max, p.sub);
        let pl = &self.players[idx];
        if lo == hi {
            // no range known: use the player's own functions if any
            let f = if delta > 0 { pl.tag(tag::NEXT_SONG) } else { pl.tag(tag::PREV_SONG) };
            if let Some(f) = f {
                deli::call_fn(&mut self.m, self.players[idx].globals, f, None).map_err(|e| anyhow!(e))?;
            }
            return Ok(());
        }
        let test = pl.tag(tag::SUBSONG_TEST);
        let mut n = cur as i32;
        loop {
            n += delta;
            if n < lo as i32 || n > hi as i32 {
                return Ok(());
            }
            self.m.bus.w16(self.players[idx].globals + dtg::SND_NUM, n as u16);
            match test {
                Some(f) => {
                    if deli::call_fn(&mut self.m, self.players[idx].globals, f, None).map_err(|e| anyhow!(e))?[0] == 0 {
                        break;
                    }
                }
                None => break,
            }
        }
        self.stop_sound(idx);
        self.m.bus.w16(self.players[idx].globals + dtg::SND_NUM, n as u16);
        if let Some(p) = &mut self.playing {
            p.sub = n as u16;
            p.frames = 0;
        }
        self.m.deli.songend = false;
        let r = self.start_sound(idx);
        self.song_starts();
        r
    }

    pub fn pattern_step(&mut self, delta: i32) {
        match &mut self.playing {
            Some(Playing { backend: Backend::Native(pt), .. }) => pt.jump(delta),
            Some(Playing { backend: Backend::Deli(idx), .. }) => {
                let t = if delta > 0 { tag::NEXT_PATT } else { tag::PREV_PATT };
                if let Some(f) = self.players[*idx].tag(t) {
                    let _ = deli::call_fn(&mut self.m, self.players[*idx].globals, f, None);
                }
            }
            Some(Playing { backend: Backend::Replay(_) | Backend::Mix(_) | Backend::Pcm(_), .. }) | None => {}
        }
    }

    pub fn songend(&self) -> bool {
        match &self.playing {
            Some(Playing { backend: Backend::Native(pt), .. }) => pt.songend,
            Some(Playing { backend: Backend::Replay(r), .. }) => r.songend(),
            Some(Playing { backend: Backend::Mix(x), .. }) => x.ended(),
            Some(Playing { backend: Backend::Pcm(x), .. }) => x.ended(),
            Some(_) => self.m.deli.songend,
            None => false,
        }
    }

    fn fail(&mut self, e: String) {
        log::error!("player crashed: {e}");
        self.last_error = Some(e);
        self.m.deli.timer_running = false;
        let p = self.playing.take();
        if let Some(Playing { custom_player: true, .. }) = p {
            self.players.truncate(self.builtin_players);
        }
        self.m.bus.paula.write_dmacon(0x000F);
        self.m.deli.voices.reset();
    }

    /// Runs the machine to the next thing that happens: a timer or vertical blank
    /// interrupt that is due, an audio interrupt, or the end of an output sample.
    /// Interrupts that came due while guest code ran are taken late, at once.
    fn advance(&mut self) {
        let Some(p) = &mut self.playing else { return };
        let now = self.m.bus.clock;
        // the time guest code took since Paula last ran (its interrupts were requested
        // then, and `audio_irqs` takes them)
        self.m.bus.paula.sync(now, &self.m.bus.ram);
        // DeliTracker's own timer (CIA-B timer A on the Amiga): its period is the
        // latch + 1 E-clock ticks, and a new latch is loaded at the next underflow
        let period = match &p.backend {
            Backend::Native(pt) => Some(pt.tick_clocks()),
            Backend::Replay(r) => Some(r.tick_clocks()),
            Backend::Mix(_) | Backend::Pcm(_) => None,
            Backend::Deli(idx) => self
                .m
                .deli
                .timer_running
                .then(|| (deli::timer_value(&self.m, self.players[*idx].globals) as u64 + 1) * ECLOCK_DIV),
        };
        match period {
            None => self.next_timer = None,
            Some(n) if self.next_timer.is_none() => {
                let delay = match &p.backend {
                    Backend::Replay(r) => r.timer_delay(),
                    _ => 0,
                };
                self.next_timer = Some(now + n + delay)
            }
            _ => {}
        }
        let cias = [&self.m.bus.ciaa, &self.m.bus.ciab];
        let cia_due = cias.iter().filter_map(|c| c.until_underflow()).min().map(|n| (self.m.bus.e_ticks + n) * ECLOCK_DIV);
        let paula = &self.m.bus.paula;
        let mut t = paula.now() + paula.out.until_sample() as u64;
        for d in [Some(self.next_vblank), self.next_timer, cia_due].into_iter().flatten() {
            t = t.min(d);
        }
        if t > now {
            self.m.bus.clock = self.m.bus.paula.sync_to_irq(t, &self.m.bus.ram);
        }
        let now = self.m.bus.clock;
        // CIA timers (players with their own interrupt)
        self.m.bus.cia_sync();
        for cia in 0..2 {
            let c = if cia == 0 { &mut self.m.bus.ciaa } else { &mut self.m.bus.ciab };
            let fired = c.take_pending();
            for bit in 0..2 {
                if fired & (1 << bit) != 0 {
                    let is = self.m.os.icr_vectors[cia][bit];
                    if is != 0 {
                        let (code, data) = (self.m.bus.r32(is + 18), self.m.bus.r32(is + 14));
                        crate::os::call_int_code(&mut self.m, code, data, 0);
                    }
                }
            }
        }
        if let (Some(d), Some(n)) = (self.next_timer, period) {
            if d <= now {
                self.next_timer = Some(d + n);
                self.timer_interrupt();
                if let Some(Playing { backend: Backend::Replay(r), .. }) = &self.playing {
                    if let Some(t) = r.next_tick_at() {
                        self.next_timer = Some(t);
                    }
                }
            }
        }
        if self.next_vblank <= now {
            while self.next_vblank <= now {
                self.next_vblank += self.m.bus.frame;
            }
            for is in self.m.os.int_servers[crate::os::INTB_VERTB].clone() {
                let (code, data) = (self.m.bus.r32(is + 18), self.m.bus.r32(is + 14));
                crate::os::call_int_code(&mut self.m, code, data, 0);
            }
            let is = self.m.os.int_vectors[crate::os::INTB_VERTB];
            if is != 0 {
                let (code, data) = (self.m.bus.r32(is + 18), self.m.bus.r32(is + 14));
                crate::os::call_int_code(&mut self.m, code, data, 0);
            }
        }
        if let Some(e) = self.m.crashed.take() {
            self.fail(e);
            return;
        }
        // audio.device writes that finished wake the processes waiting for them
        crate::os::audio_service(&mut self.m);
        if !self.m.deli.procs.is_empty() {
            deli::run_procs(&mut self.m);
        }
        self.audio_irqs();
    }

    /// DeliTracker's timer interrupt: the player's DTP_Interrupt, or a tick of
    /// the built-in ProTracker.
    fn timer_interrupt(&mut self) {
        let Some(p) = &mut self.playing else { return };
        match &mut p.backend {
            Backend::Replay(r) => {
                r.tick(&mut crate::replay::Hw { bus: &mut self.m.bus });
                self.m.bus.gaps = None;
            }
            Backend::Mix(_) | Backend::Pcm(_) => {}
            Backend::Native(pt) => {
                // its DMA waits run the clock on, as the 68k replay's loops do
                pt.tick(&mut self.m.bus);
                // E0x / LED filter
                if pt.filter {
                    self.m.bus.ciaa.pra &= !2;
                } else {
                    self.m.bus.ciaa.pra |= 2;
                }
            }
            Backend::Deli(idx) => {
                let idx = *idx;
                if let Some(f) = self.players[idx].tag(tag::INTERRUPT) {
                    if let Err(e) = deli::call_fn(&mut self.m, self.players[idx].globals, f, None) {
                        self.fail(e);
                    }
                }
            }
        }
    }

    /// Runs the handlers of the audio interrupts requested since the last call (by Paula
    /// or by guest code starting DMA), and of those already requested whose INTENA bit
    /// was set since: an interrupt pending when it is enabled is taken at once.
    fn audio_irqs(&mut self) {
        let p = &mut self.m.bus.paula;
        let mut raised = (std::mem::take(&mut p.raised) | p.intreq & p.intena & !self.intena_seen) & (0xF << 7);
        self.intena_seen = p.intena;
        if raised == 0 || self.m.bus.paula.pending_audio_irqs() == 0 {
            return;
        }
        if raised & 0x80 != 0 && self.m.bus.amp.is_some() {
            // the amplifier's buffer started: the host mixes the next
            raised &= !0x80;
            for is in crate::amplifier::interrupt(&mut self.m.bus) {
                let (code, data) = (self.m.bus.r32(is + 18), self.m.bus.r32(is + 14));
                crate::os::call_int_code(&mut self.m, code, data, 0);
            }
        }
        for ch in 0..4 {
            let bit = 0x80u16 << ch;
            if raised & bit == 0 || self.m.bus.paula.intena & bit == 0 {
                continue;
            }
            if let Some(Playing { backend: Backend::Replay(r), .. }) = &mut self.playing {
                if r.audio_irq_ch(ch, &mut crate::replay::Hw { bus: &mut self.m.bus }) {
                    continue;
                }
            }
            // an earlier handler of this round may have cleared the request already (Digital
            // Sonix & Chrome's, which clears every audio bit): nothing is pending then
            let is = self.m.os.int_vectors[crate::os::INTB_AUD0 + ch];
            if is != 0 && self.m.bus.paula.intreq & bit != 0 {
                let (code, data) = (self.m.bus.r32(is + 18), self.m.bus.r32(is + 14));
                let active = (self.m.bus.paula.intena & self.m.bus.paula.intreq) as u32;
                crate::os::call_int_code(&mut self.m, code, data, active);
            }
        }
        if let Some(e) = self.m.crashed.take() {
            self.fail(e);
        }
    }

    /// The output stage model for the module playing: an Amiga's, unless the
    /// format comes from the PC (its music was not made for Paula's filters).
    fn output_filter(&self) -> Filter {
        let pc = self.playing.as_ref().is_some_and(|p| {
            let f = p.format.to_ascii_lowercase();
            PC_FORMATS.iter().any(|n| f.contains(n)) || matches!(&p.backend, Backend::Replay(r) if r.pc_music())
        });
        if let Some(f) = self.settings.filter_model {
            return f;
        }
        match (pc, self.settings.a500_filter) {
            (true, _) => Filter::None,
            (false, true) => Filter::A500,
            (false, false) => Filter::A1200,
        }
    }

    /// The output stages in effect for the module playing, after every
    /// automatic choice, as one line of `key=value` words (what `--wav` prints
    /// on stderr as `output: ...`).
    pub fn output_report(&self) -> String {
        let Some(p) = &self.playing else { return "output: nothing playing".into() };
        let sx = &self.settings;
        let (backend, player) = match &p.backend {
            Backend::Deli(i) => ("68k", self.players.get(*i).map_or("?".to_string(), |x| x.name.clone())),
            Backend::Native(_) => ("builtin-protracker", p.format.clone()),
            Backend::Replay(_) => ("open-replayer", p.format.clone()),
            Backend::Mix(_) => ("libxmp-port", p.format.clone()),
            Backend::Pcm(_) => ("aiff-decoder", p.format.clone()),
        };
        let q = |t: &str| t.replace(' ', "_");
        let mut o = format!("output: backend={backend} player={} format={}", q(&player), q(&p.format));
        let name = |f: Filter| match f {
            Filter::None => "none",
            Filter::A500 => "a500",
            Filter::A1200 => "a1200",
        };
        if let Backend::Mix(x) = &p.backend {
            let interp = match sx.interp {
                crate::xmp::mixer::Interp::Nearest => "nearest",
                crate::xmp::mixer::Interp::Linear => "linear",
                crate::xmp::mixer::Interp::Spline => "spline",
            };
            let used = x.uses_resonant_filter();
            o += &format!(
                " filter=none led=n/a sep=libxmp(100) mono={} resampler=libxmp-mixer amplifier=none interp={interp} xmp-filter={}",
                sx.mono as u8,
                match (sx.xmp_filter, used) {
                    (true, true) => "on(used)",
                    (true, false) => "on(unused)",
                    (false, true) => "off(module uses it)",
                    (false, false) => "off",
                }
            );
        } else {
            let f = self.output_filter();
            let auto = if sx.filter_model.is_none() { "(auto)" } else { "" };
            let led = if f == Filter::None {
                "n/a".to_string()
            } else if !sx.led_filter {
                "off".to_string()
            } else if sx.led_force {
                "force".to_string()
            } else {
                "auto(player)".to_string()
            };
            let amp = match &self.m.bus.amp {
                Some(a) => format!("8bit@{:.0}Hz,voices={},boost={}.{}", a.rate(), a.n, a.boost / 10, a.boost % 10),
                None => "none".to_string(),
            };
            let voices = if self.m.deli.voices.in_use { "builtin-noteplayer(onepole4420+led-biquad3275)" } else { "no" };
            o += &format!(
                " filter={}{auto} led={led} sep={:.2} mono={} resampler=uade-default amplifier={amp} builtin-voices={voices}",
                name(f),
                sx.separation.clamp(0.0, 1.0),
                sx.mono as u8
            );
        }
        o += &format!(" volume={:.2}", sx.volume);
        if let Some(Playing { backend: Backend::Replay(r), .. }) = &self.playing {
            if r.needs_fast_reference_cpu() {
                o += " reference-cpu=fast";
            }
        }
        o
    }

    /// What the player did with the LED filter during the render: the state at
    /// the first sample, then "yes at Ns (on|off)" for the first change.
    pub fn led_report(&self) -> String {
        let t = |f: u64| f as f64 / self.rate;
        match self.led_log.as_slice() {
            [] => "led-switched: n/a".into(),
            [(_, s)] => format!("led-switched: no (led {} throughout)", if *s { "on" } else { "off" }),
            [(_, s0), (f, s), ..] => format!(
                "led-switched: yes at {:.2}s ({}, {} change(s); led {} at start)",
                t(*f),
                if *s { "on" } else { "off" },
                self.led_changes,
                if *s0 { "on" } else { "off" }
            ),
        }
    }

    /// Fills `out` with interleaved stereo f32 frames.
    pub fn render(&mut self, out: &mut [f32]) {
        if let Some(Playing { backend: Backend::Mix(_), .. }) = &self.playing {
            self.render_mix(out);
            return;
        }
        if let Some(Playing { backend: Backend::Pcm(_), .. }) = &self.playing {
            self.render_pcm(out);
            return;
        }
        let sep = self.settings.separation.clamp(0.0, 1.0);
        let vol = self.settings.volume;
        let filter = self.output_filter();
        self.m.bus.paula.out.set_filter(filter);
        for frame in out.chunks_exact_mut(2) {
            if self.playing.is_none() || self.paused {
                frame[0] = 0.0;
                frame[1] = 0.0;
                continue;
            }
            let led_now = self.m.bus.led();
            if self.led_seen != Some(led_now) {
                if self.led_seen.is_some() {
                    self.led_changes += 1;
                }
                if self.led_log.len() < 16 {
                    self.led_log.push((self.playing.as_ref().map_or(0, |p| p.frames), led_now));
                }
            }
            self.led_seen = Some(led_now);
            let led_on = self.settings.led_force || led_now;
            self.m.bus.paula.out.led = filter != Filter::None && self.settings.led_filter && led_on;
            let (pl, pr) = loop {
                if let Some(s) = self.m.bus.paula.samples.pop_front() {
                    break s;
                }
                if self.playing.is_none() {
                    break (0, 0);
                }
                self.advance();
            };
            self.heard |= pl != 0 || pr != 0;
            let (hl, hr) = (pl as f32 / 32768.0, pr as f32 / 32768.0);
            let mut l = hl * (0.5 + sep * 0.5) + hr * (0.5 - sep * 0.5);
            let mut r = hr * (0.5 + sep * 0.5) + hl * (0.5 - sep * 0.5);
            if self.m.deli.voices.in_use {
                // the built-in NotePlayer renderer has its own filters
                let (nl, nr) = self.m.deli.voices.render(&self.m.bus.ram, self.rate);
                self.heard |= nl != 0.0 || nr != 0.0;
                let mut vl = nl * (0.5 + sep * 0.5) + nr * (0.5 - sep * 0.5);
                let mut vr = nr * (0.5 + sep * 0.5) + nl * (0.5 - sep * 0.5);
                if filter == Filter::A500 {
                    vl = self.a500.run(0, vl);
                    vr = self.a500.run(1, vr);
                }
                if self.settings.led_filter && filter != Filter::None && led_on {
                    vl = self.led.run(0, vl);
                    vr = self.led.run(1, vr);
                }
                l += vl * 0.5;
                r += vr * 0.5;
            }
            if self.settings.mono {
                let m = (l + r) * 0.5;
                l = m;
                r = m;
            }
            frame[0] = (l * vol).clamp(-1.0, 1.0);
            frame[1] = (r * vol).clamp(-1.0, 1.0);
            if let Some(p) = &mut self.playing {
                p.frames += 1;
            }
            let paula = &self.m.bus.paula;
            self.push_scopes([0, 1, 2, 3].map(|i| paula.output(i)));
        }
        self.skip_empty_subsong();
    }

    /// `render` for the libxmp mixer: its 16-bit stereo, at full scale for the
    /// default volume (0.8), so that the 16-bit WAV the CLI writes holds libxmp's
    /// samples unchanged. Separation is libxmp's (its mix setting 100).
    fn render_mix(&mut self, out: &mut [f32]) {
        let Some(Playing { backend: Backend::Mix(x), frames, .. }) = &mut self.playing else { return };
        let gain = self.settings.volume / 0.8;
        let mut buf = vec![0i16; out.len()];
        let n = if self.paused { 0 } else { x.render(&mut buf) };
        *frames += n as u64;
        for (o, &s) in out.iter_mut().zip(buf.iter()) {
            *o = (s as f32 / 32768.0 * gain).clamp(-1.0, 1.0);
        }
        self.heard |= buf[..n * 2].iter().any(|&s| s != 0);
        if self.settings.mono {
            for f in out.chunks_exact_mut(2) {
                let m = (f[0] + f[1]) * 0.5;
                f[0] = m;
                f[1] = m;
            }
        }
        self.skip_empty_subsong();
    }

    /// `render` for recorded audio: the decoder's samples at the default volume (0.8)
    /// unchanged, so that the 16-bit WAV the CLI writes holds the file's samples.
    fn render_pcm(&mut self, out: &mut [f32]) {
        let Some(Playing { backend: Backend::Pcm(x), frames, .. }) = &mut self.playing else { return };
        let gain = self.settings.volume / 0.8;
        let n = if self.paused { 0 } else { x.render(out) };
        if self.paused {
            out.fill(0.0);
        }
        *frames += n as u64;
        for o in out.iter_mut() {
            *o = (*o * gain).clamp(-1.0, 1.0);
        }
        self.heard |= out[..n * 2].iter().any(|&s| s != 0.0);
        if self.settings.mono {
            for f in out.chunks_exact_mut(2) {
                let m = (f[0] + f[1]) * 0.5;
                f[0] = m;
                f[1] = m;
            }
        }
    }

    fn push_scopes(&mut self, paula: [f32; 4]) {
        let nv = if self.m.deli.voices.in_use { self.m.deli.voices.voices.len() } else { 0 };
        let n = 4 + nv;
        if self.scope_ring.len() != n {
            self.scope_ring = vec![vec![0.0; SCOPE_LEN]; n];
        }
        let i = self.scope_pos;
        for (k, v) in paula.iter().enumerate() {
            self.scope_ring[k][i] = *v;
        }
        for k in 0..nv {
            self.scope_ring[4 + k][i] = self.m.deli.voices.voices[k].scope;
        }
        self.scope_pos = (i + 1) % SCOPE_LEN;
    }

    /// Recent output of every channel (oldest first), for oscilloscopes.
    /// Silent channels are left out.
    pub fn scope_data(&self) -> Vec<Vec<f32>> {
        let mut out = Vec::new();
        let voices = self.scope_ring.len() > 4;
        for (k, ring) in self.scope_ring.iter().enumerate() {
            // Paula scopes always show, unless a NoteStruct player owns the
            // sound; voice scopes only while they are making sound
            let used = (k < 4 && !voices) || ring.iter().any(|&x| x != 0.0);
            if !used {
                continue;
            }
            let mut v = Vec::with_capacity(SCOPE_LEN);
            v.extend_from_slice(&ring[self.scope_pos..]);
            v.extend_from_slice(&ring[..self.scope_pos]);
            out.push(v);
        }
        out
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.playing.as_ref().map(|p| p.frames as f64 / self.rate).unwrap_or(0.0)
    }

    pub fn native_position(&self) -> Option<(usize, usize, usize, u32)> {
        match &self.playing {
            Some(Playing { backend: Backend::Native(pt), .. }) => {
                let (p, l, r) = pt.position();
                Some((p, l, r, pt.bpm))
            }
            _ => None,
        }
    }
}

/// Whether a player may check this file. TME's check (Eagleplayer 2.06, V2.3)
/// takes most files of other formats that hold 256 zero bytes where it looks
/// for its tables, and plays them silent; so, as UADE's eagleplayer.conf does,
/// it is asked only for files named as TME modules ("tme." or ".tme").
#[cfg(feature = "host-68k")]
fn by_name_only(player: &str, path: &Path) -> bool {
    if !player.eq_ignore_ascii_case("TME") {
        return true;
    }
    let name = file_name(path).to_ascii_lowercase();
    name.starts_with("tme.") || name.ends_with(".tme")
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
}

#[cfg(feature = "host-68k")]
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                collect_files(&p, out, depth - 1);
            }
        } else if !p.extension().is_some_and(|x| x == "info") {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "host-68k")]
    use super::*;

    #[test]
    #[cfg(feature = "host-68k")]
    fn tme_checks_only_tme_names() {
        assert!(by_name_only("TME", Path::new("x/tme.Labyrinth.msx")));
        assert!(by_name_only("TME", Path::new("x/hawkeye.TME")));
        assert!(!by_name_only("TME", Path::new("x/OSS.Mighty Dub Players")));
        assert!(!by_name_only("TME", Path::new("x/agi.ingame_17")));
        assert!(by_name_only("OctaMED", Path::new("x/OSS.Mighty Dub Players")));
    }
}
