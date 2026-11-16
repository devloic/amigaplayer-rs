//! A port of libxmp's player core and software mixer, for the PC tracker
//! formats that only libxmp (or OpenMPT) plays: a format-independent module
//! model (`module`), the loader helpers (`load`), the player (`player`,
//! `effects`, `virt`, `scan`), the 16-bit stereo mixer (`mixer`) with its
//! resonant filter (`filter`) and one loader
//! per format (`loaders`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! A loader fills a [`module::ModuleData`] the way its libxmp counterpart does:
//! the module (`mod`), the quirk and flow flags, `read_event_type`,
//! `period_type`, `c4rate` and the rest. [`XmpPlayer`] then runs libxmp's
//! epilogue, scan and player on it and renders 16-bit stereo at any rate.

pub mod aormath;
pub mod effects;
pub mod extras;
pub mod far_extras;
pub mod filter;
pub mod iff;
pub mod inflate;
pub mod load;
pub mod lzw;
pub mod loaders;
pub mod mixer;
pub mod module;
pub mod opl_glue;
pub mod player;
pub mod scan;
mod spline_lut;
pub mod virt;
pub mod voltable;
pub mod vorbis;

use load::Hio;
use module::ModuleData;
use player::Context;

/// A libxmp loader: `test` is the loader's test function, `load` its loader.
pub struct Loader {
    pub name: &'static str,
    pub test: fn(&[u8]) -> bool,
    pub load: fn(&mut ModuleData, &mut Hio) -> Result<(), ()>,
}

/// The loaders, in libxmp's order (`format_loaders` of `format.c`).
pub static LOADERS: &[Loader] = &[
    Loader { name: "Fast Tracker II", test: loaders::xm::test, load: loaders::xm::load },
    // libxmp's ProTracker loader, for one of its magics only: the hacked MODs of the game
    // "Apocalypse Abyss" (Software Visions, `.dmf`; ".M.K", header and first pattern
    // byte-swapped). The other MODs stay on the built-in ProTracker (see `mod_load`).
    Loader { name: "Software Visions DMF", test: software_visions_test, load: loaders::mod_load::load },
    // Un4seen MO3 (`.mo3`): IT, XM, S3M, MOD and MTM modules with packed music data and
    // compressed samples, put back into their format for that format's loader (see `mo3`).
    Loader { name: "Un4seen MO3", test: crate::mo3::test, load: crate::mo3::load },
    // MadTracker 2, DSIK (RIFF DSMF) and Velvet Studio / Extreme's Tracker AMS: no libxmp
    // loader; read as libopenmpt reads them and put into IT or S3M for that loader (see
    // `mptconv`).
    Loader { name: "MadTracker 2", test: crate::mptconv::mt2::test, load: crate::mptconv::mt2::load },
    Loader { name: "DSIK RIFF DSM", test: crate::mptconv::dsm::test, load: crate::mptconv::dsm::load },
    Loader { name: "Dynamic Studio", test: crate::mptconv::dsm::test_dynamic, load: crate::mptconv::dsm::load_dynamic },
    Loader { name: "Velvet Studio AMS", test: crate::mptconv::ams::test, load: crate::mptconv::ams::load },
    Loader { name: "Graoumf Tracker", test: crate::mptconv::gt2::test, load: crate::mptconv::gt2::load },
    Loader { name: "Disorder Tracker 2 PLM", test: crate::mptconv::plm::test, load: crate::mptconv::plm::load },
    Loader { name: "Impulse Tracker", test: loaders::it::test, load: loaders::it::load },
    Loader { name: "Scream Tracker 3", test: loaders::s3m::test, load: loaders::s3m::load },
    Loader { name: "Scream Tracker 2", test: loaders::stm::test, load: loaders::stm::load },
    Loader { name: "STMIK 0.2", test: loaders::stx::test, load: loaders::stx::load },
    Loader { name: "MultiTracker", test: loaders::mtm::test, load: loaders::mtm::load },
    Loader { name: "Digitrakker", test: loaders::mdl::test, load: loaders::mdl::load },
    Loader { name: "Imago Orpheus v1.0", test: loaders::imf::test, load: loaders::imf::load },
    Loader { name: "Ultra Tracker", test: loaders::ult::test, load: loaders::ult::load },
    Loader { name: "Liquid Tracker", test: loaders::liq::test, load: loaders::liq::load },
    Loader { name: "Liquid Tracker NO", test: loaders::no::test, load: loaders::no::load },
    Loader { name: "General Digital Music", test: loaders::gdm::test, load: loaders::gdm::load },
    Loader { name: "Megatracker", test: loaders::mgt::test, load: loaders::mgt::load },
    Loader { name: "Epic MegaGames MASI", test: loaders::masi::test, load: loaders::masi::load },
    Loader { name: "Epic MegaGames MASI 16", test: loaders::masi16::test, load: loaders::masi16::load },
    Loader { name: "MUSE container", test: loaders::muse::test, load: loaders::muse::load },
    Loader { name: "Galaxy Music System 5.0 (J2B)", test: loaders::gal5::test, load: loaders::gal5::load },
    Loader { name: "Galaxy Music System 4.0", test: loaders::gal4::test, load: loaders::gal4::load },
    Loader { name: "DSMI Advanced Module Format", test: loaders::amf::test, load: loaders::amf::load },
    Loader { name: "Asylum Music Format v1.0", test: loaders::asylum::test, load: loaders::asylum::load },
    Loader { name: "Chiptracker", test: loaders::chip::test, load: loaders::chip::load },
    Loader { name: "Real Tracker", test: loaders::rtm::test, load: loaders::rtm::load },
    Loader { name: "Digital Tracker", test: loaders::dt::test, load: loaders::dt::load },
    Loader { name: "Archimedes Tracker", test: loaders::arch::test, load: loaders::arch::load },
    Loader { name: "Digital Symphony", test: loaders::sym::test, load: loaders::sym::load },
    Loader { name: "DigiBooster Pro", test: loaders::dbm::test, load: loaders::dbm::load },
    Loader { name: "Farandole Composer", test: loaders::far::test, load: loaders::far::load },
    Loader { name: "Epic Games UMX", test: loaders::umx::test, load: loaders::umx::load },
    Loader { name: "Coconizer", test: loaders::coco::test, load: loaders::coco::load },
    Loader { name: "Images Music System", test: loaders::ims::test, load: loaders::ims::load },
    Loader { name: "Composer 669", test: loaders::c669::test, load: loaders::c669::load },
    Loader { name: "Funktracker", test: loaders::fnk::test, load: loaders::fnk::load },
    Loader { name: "Magnetic Fields Packer", test: loaders::mfp::test, load: loaders::mfp::load },
    Loader { name: "Astroidea XMF", test: loaders::xmf::test, load: loaders::xmf::load },
    // not a libxmp loader: Symphonie plays on `crate::symmod` (XmpPlayer hands it there)
    Loader { name: "Symphonie", test: crate::symmod::test, load: |_, _| Err(()) },
    // not a libxmp loader: X-Tracker plays on `crate::dmf` (XmpPlayer hands it there)
    Loader { name: "X-Tracker", test: crate::dmf::test, load: |_, _| Err(()) },
];

/// libxmp's `mod_test` for a module whose magic is ".M.K" (Software Visions DMF).
fn software_visions_test(data: &[u8]) -> bool {
    data.get(1080..1084) == Some(b".M.K") && loaders::mod_load::test(data)
}

/// The loader that claims `data`, if any.
pub fn detect(data: &[u8]) -> Option<&'static Loader> {
    LOADERS.iter().find(|l| (l.test)(data))
}

/// The loaders (by name) whose formats the open chain — `open::detect`, which
/// gates the wasm, and the engine with `open_replayers` — leaves to the players
/// that had them before the port. Empty: XM, S3M and STM, which went natively to
/// the 68k FastTracker_2.0 player and ScreamTracker_3.0 Eagleplayer and, in the
/// archive's web app, past the open replayers to UADE and ft2play / st3play, play
/// on the libxmp port. Add a loader's name ("Fast Tracker II", "Scream Tracker 3",
/// "Scream Tracker 2") to hand its format back to those players. IT, which
/// nothing played natively and the web app sent to UADE and then it2play
/// (libopenmpt and libxmp for the ITs it2play refuses), plays on the port too;
/// add "Impulse Tracker" to hand it back. DigiBooster Pro (DBM0) plays on the
/// port too: natively the 68k DigiBoosterPRO Eagleplayer claims it but renders
/// silence (it needs digibooster.library), and in the archive's web app UADE gives
/// nothing and au-replay's libdigibooster3 came next; add "DigiBooster Pro" to hand
/// it back to them. Digital Symphony and Archimedes Tracker have no other player.
pub const LEFT_TO_OTHER_PLAYERS: &[&str] = &[];

/// `detect`, for the open chain: None for a format in `LEFT_TO_OTHER_PLAYERS`.
pub fn detect_open(data: &[u8]) -> Option<&'static Loader> {
    detect(data).filter(|l| !LEFT_TO_OTHER_PLAYERS.contains(&l.name))
}

/// The format name of the module in `data` that the open chain plays here, if it loads.
pub fn format_of(data: &[u8]) -> Option<String> {
    if crate::symmod::test(data) {
        return crate::symmod::Symmod::new(data, 48000).map(|s| s.format().to_string());
    }
    if crate::dmf::test(data) {
        return crate::dmf::Dmf::new(data, 48000).map(|s| s.format().to_string());
    }
    load_module(data).map(|c| c.m.module.type_)
}

/// `xmp_load_module`: the module in `data`, checked and scanned.
pub fn load_module(data: &[u8]) -> Option<Context> {
    load_module_at(data, None)
}

/// `load_module` for a module read from `path`: the loaders that need a
/// companion file (Magnetic Fields Packer's `smp.*`) look for it next to it.
pub fn load_module_at(data: &[u8], path: Option<&std::path::Path>) -> Option<Context> {
    let loader = detect(data)?;
    let mut m = ModuleData::default();
    if let Some(p) = path {
        m.dirname = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        m.basename = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    }
    (loader.load)(&mut m, &mut Hio::new(data)).ok()?;
    load::load_epilogue(&mut m).ok()?;
    let mut ctx = Context::new(m);
    ctx.scan_sequences();
    Some(ctx)
}

/// A module playing through libxmp's player and mixer, or, for Symphonie, through
/// `crate::symmod`'s own player and mixer.
pub struct XmpPlayer(Core);

enum Core {
    Xmp(LibXmp),
    Sym(Box<crate::symmod::Symmod>),
    Dmf(Box<crate::dmf::Dmf>),
}

impl XmpPlayer {
    /// Loads `data` and starts it at `rate` Hz, sequence 0.
    pub fn new(data: &[u8], rate: u32) -> Option<XmpPlayer> {
        XmpPlayer::open(data, rate, None)
    }

    /// `new` for a module read from `path` (companion files are looked for there).
    pub fn open(data: &[u8], rate: u32, path: Option<&std::path::Path>) -> Option<XmpPlayer> {
        if crate::symmod::test(data) {
            return crate::symmod::Symmod::new(data, rate).map(|s| XmpPlayer(Core::Sym(Box::new(s))));
        }
        if crate::dmf::test(data) {
            return crate::dmf::Dmf::new(data, rate).map(|s| XmpPlayer(Core::Dmf(Box::new(s))));
        }
        LibXmp::open(data, rate, path).map(|x| XmpPlayer(Core::Xmp(x)))
    }

    /// Sets the mixer's interpolation and whether its resonant filter runs
    /// (libxmp's `XMP_INTERP_*` and `XMP_DSP_LOWPASS`; Symphonie: nearest or linear, and its
    /// filter always runs).
    pub fn set_mixer(&mut self, interp: mixer::Interp, filter: bool) {
        match &mut self.0 {
            Core::Xmp(x) => x.set_mixer(interp, filter),
            Core::Sym(s) => s.set_nearest(matches!(interp, mixer::Interp::Nearest)),
            Core::Dmf(s) => s.set_nearest(matches!(interp, mixer::Interp::Nearest)),
        }
    }

    /// Whether the module sets libxmp's filter quirk (IT, Liquid, Imago).
    pub fn uses_resonant_filter(&self) -> bool {
        match &self.0 {
            Core::Xmp(x) => x.uses_resonant_filter(),
            Core::Sym(_) | Core::Dmf(_) => false,
        }
    }

    /// libxmp's module type ("Composer 669", "MultiTracker 1.00 MTM"...).
    pub fn format(&self) -> &str {
        match &self.0 {
            Core::Xmp(x) => x.format(),
            Core::Sym(s) => s.format(),
            Core::Dmf(s) => s.format(),
        }
    }

    pub fn title(&self) -> &str {
        match &self.0 {
            Core::Xmp(x) => x.title(),
            Core::Sym(s) => s.title(),
            Core::Dmf(s) => s.title(),
        }
    }

    /// The sequences libxmp's scan found.
    pub fn subsongs(&self) -> usize {
        match &self.0 {
            Core::Xmp(x) => x.subsongs(),
            Core::Sym(_) | Core::Dmf(_) => 1,
        }
    }

    pub fn subsong(&self) -> usize {
        match &self.0 {
            Core::Xmp(x) => x.subsong(),
            Core::Sym(_) | Core::Dmf(_) => 0,
        }
    }

    /// The scanned duration of a sequence, in milliseconds.
    pub fn duration_ms(&self, seq: usize) -> i32 {
        match &self.0 {
            Core::Xmp(x) => x.duration_ms(seq),
            Core::Sym(s) if seq == 0 => s.duration_ms(),
            Core::Sym(_) => 0,
            Core::Dmf(s) if seq == 0 => s.duration_ms(),
            Core::Dmf(_) => 0,
        }
    }

    /// Starts sequence `seq` from its beginning.
    pub fn start(&mut self, seq: usize) {
        match &mut self.0 {
            Core::Xmp(x) => x.start(seq),
            Core::Sym(s) => s.start(),
            Core::Dmf(s) => s.start(),
        }
    }

    /// True once the module ended or looped.
    pub fn ended(&self) -> bool {
        match &self.0 {
            Core::Xmp(x) => x.ended(),
            Core::Sym(s) => s.ended(),
            Core::Dmf(s) => s.ended(),
        }
    }

    /// Fills `out` (interleaved stereo) and returns the frames written: fewer
    /// than asked only at the end of the module.
    pub fn render(&mut self, out: &mut [i16]) -> usize {
        match &mut self.0 {
            Core::Xmp(x) => x.render(out),
            Core::Sym(s) => s.render(out),
            Core::Dmf(s) => s.render(out),
        }
    }

    /// The order, pattern and row playing (for display).
    pub fn position(&self) -> (i32, i32, i32) {
        match &self.0 {
            Core::Xmp(x) => x.position(),
            Core::Sym(s) => s.position(),
            Core::Dmf(s) => s.position(),
        }
    }
}

/// The libxmp port playing a module.
struct LibXmp {
    ctx: Context,
    rate: i32,
    /// The rest of the last tick, interleaved stereo.
    pending: Vec<i16>,
    at: usize,
    ended: bool,
    subsong: usize,
}

impl LibXmp {
    /// `new` for a module read from `path` (companion files are looked for there).
    fn open(data: &[u8], rate: u32, path: Option<&std::path::Path>) -> Option<LibXmp> {
        let ctx = load_module_at(data, path)?;
        let mut p = LibXmp { ctx, rate: rate as i32, pending: Vec::new(), at: 0, ended: false, subsong: 0 };
        p.start(0);
        Some(p)
    }

    /// Sets the mixer's interpolation and whether its resonant filter runs
    /// (libxmp's `XMP_INTERP_*` and `XMP_DSP_LOWPASS`).
    fn set_mixer(&mut self, interp: mixer::Interp, filter: bool) {
        self.ctx.s.interp = interp;
        self.ctx.s.dsp = if filter { mixer::XMP_DSP_LOWPASS } else { 0 };
    }

    /// Whether the module sets libxmp's filter quirk (IT, Liquid, Imago).
    fn uses_resonant_filter(&self) -> bool {
        self.ctx.m.quirk & module::QUIRK_FILTER != 0
    }

    /// libxmp's module type ("Composer 669", "MultiTracker 1.00 MTM"...).
    fn format(&self) -> &str {
        &self.ctx.m.module.type_
    }

    fn title(&self) -> &str {
        &self.ctx.m.module.name
    }

    /// The sequences libxmp's scan found.
    fn subsongs(&self) -> usize {
        self.ctx.m.num_sequences.max(1)
    }

    fn subsong(&self) -> usize {
        self.subsong
    }

    /// The scanned duration of a sequence, in milliseconds.
    fn duration_ms(&self, seq: usize) -> i32 {
        self.ctx.m.seq_data.get(seq).map_or(0, |s| s.duration)
    }

    /// `xmp_start_player`, then `xmp_set_position` to the sequence's entry
    /// point (what `xmp_set_subsong`... the player's sequence selection does).
    fn start(&mut self, seq: usize) {
        let seq = seq.min(self.subsongs() - 1);
        // a fresh player state that keeps what the load-time scan found
        let old = std::mem::take(&mut self.ctx.p);
        self.ctx.p.scan = old.scan;
        self.ctx.p.sequence_control = old.sequence_control;
        self.ctx.p.scan_time_factor = old.scan_time_factor;
        self.ctx.start_player(self.rate);
        if seq > 0 {
            let ep = self.ctx.m.seq_data[seq].entry_point as i32;
            self.ctx.set_position(ep, 0);
        }
        self.subsong = seq;
        self.pending.clear();
        self.at = 0;
        self.ended = false;
    }

    /// True once the module ended or looped.
    fn ended(&self) -> bool {
        self.ended
    }

    /// Fills `out` (interleaved stereo) and returns the frames written: fewer
    /// than asked only at the end of the module, which is where libxmp's frame
    /// loop stops (`-XMP_END`) or the loop counter reaches 1.
    fn render(&mut self, out: &mut [i16]) -> usize {
        let want = out.len() / 2;
        let mut done = 0;
        while done < want {
            if self.at >= self.pending.len() {
                if self.ended || !self.next_tick() {
                    self.ended = true;
                    break;
                }
            }
            let n = ((self.pending.len() - self.at) / 2).min(want - done);
            out[done * 2..(done + n) * 2].copy_from_slice(&self.pending[self.at..self.at + n * 2]);
            self.at += n * 2;
            done += n;
        }
        done
    }

    fn next_tick(&mut self) -> bool {
        if !self.ctx.play_frame() || self.ctx.p.loop_count >= 1 {
            return false;
        }
        let n = (self.ctx.s.ticksize * 2) as usize;
        self.pending.clear();
        self.pending.extend_from_slice(&self.ctx.s.buffer[..n]);
        self.at = 0;
        true
    }

    /// The order, pattern and row playing (for display).
    fn position(&self) -> (i32, i32, i32) {
        let p = &self.ctx.p;
        let pat = self.ctx.m.module.xxo.get(p.ord.max(0) as usize).copied().unwrap_or(0) as i32;
        (p.ord, pat, p.row)
    }
}
