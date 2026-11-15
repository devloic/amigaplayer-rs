mod ui;

use amigaplayer_rs::paulaout::Filter;
use amigaplayer_rs::xmp::mixer::Interp;
#[cfg(feature = "host-68k")]
use amigaplayer_rs::{default_eagleplayers_dir, default_genies_dir, default_libs_dir, default_players_dir};
use amigaplayer_rs::{engine, SAMPLE_RATE};
use amigaplayer_rs::engine::Settings;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// The options that need the 68k players, which only a build with `host-68k` has.
#[cfg(not(feature = "host-68k"))]
const NEEDS_68K: &[&str] = &["--players", "--eagleplayers", "--player", "--68k-players", "--genies", "--genie", "--noteplayer", "--libs"];

struct Args {
    #[cfg(feature = "host-68k")]
    players: Option<PathBuf>,
    #[cfg(feature = "host-68k")]
    eagleplayers: Option<PathBuf>,
    #[cfg(feature = "host-68k")]
    genies: Option<PathBuf>,
    #[cfg(feature = "host-68k")]
    libs: Option<PathBuf>,
    #[cfg(feature = "host-68k")]
    use_genies: Vec<String>,
    #[cfg(feature = "host-68k")]
    only_player: Option<String>,
    #[cfg(feature = "host-68k")]
    open_replayers: bool,
    wav: Option<PathBuf>,
    /// the --wav render's sample rate
    rate: u32,
    seconds: f64,
    subsong: i32,
    led: bool,
    led_force: bool,
    filter: Option<Filter>,
    separation: Option<f32>,
    mono: bool,
    interp: Interp,
    xmp_filter: bool,
    volume: Option<f32>,
    archive_entry: Option<String>,
    files: Vec<PathBuf>,
}

fn usage() -> ! {
    eprintln!("{}", USAGE_HEAD);
    #[cfg(feature = "host-68k")]
    eprintln!("{}", USAGE_68K);
    #[cfg(not(feature = "host-68k"))]
    eprintln!(
        "The 68k-player options ({}) need a build with the host-68k feature:\n\
         cargo build --release --features host-68k",
        NEEDS_68K.join(" ")
    );
    eprintln!("{}", USAGE_TAIL);
    std::process::exit(2)
}

const USAGE_HEAD: &str = "usage: amigaplayer-rs [--wav OUT.wav [--rate HZ] [--seconds N] [--subsong N] [--volume V]]\n\
         \x20                 [--filter a500|a1200|none|auto] [--led auto|on|off|force]\n\
         \x20                 [--separation 0..1] [--mono] [--interp nearest|linear|spline]\n\
         \x20                 [--xmp-filter on|off] [--archive-entry NAME]\n\
         \x20                 [FILES/DIRS...]\n\
         \n\
         Without --wav the SDL3 player window opens with FILES as the module list.\n\
         The formats the open replayers (Rust ports of NostalgicPlayer's players and clean-room\n\
         ports), the built-in ProTracker and the libxmp ports know are played with them\n\
         (--open-replayers is accepted and changes nothing).";

/// The options of a build with `host-68k`.
#[cfg(feature = "host-68k")]
const USAGE_68K: &str = "68k players (this build has them): [--players DIR] [--eagleplayers DIR] [--player NAME]\n\
         \x20                 [--68k-players] [--genies DIR] [--genie NAME]... [--noteplayer NAME] [--libs DIR]\n\
         --players DIR holds the original 68k DeliPlayers (default: $DELIPLAYERS,\n\
         ~/.config/amigaplayer-rs/DeliPlayers, ./DeliPlayers), --eagleplayers DIR the\n\
         Eagleplayers (default: $EAGLEPLAYERS, ~/.config/amigaplayer-rs/Eagleplayers,\n\
         ./Eagleplayers); DeliPlayers are tried first. The player directories are read when a\n\
         module needs a 68k player, not before. --player NAME tries only the\n\
         player of that name (a player without a module check, like BFB Master, plays\n\
         only when chosen so). A library a player opens is loaded from the module's\n\
         directory, its libs/, ../libs/, then --libs DIR (LIBS:; default:\n\
         $AMIGAPLAYER_LIBS, ~/.config/amigaplayer-rs/Libs, ./Libs).\n\
         --genies DIR holds the DeliGenies (default: $DELIGENIES,\n\
         ~/.config/amigaplayer-rs/DeliGenies, ./DeliGenies); --genie NAME runs one, and a\n\
         NotePlayer genie (--noteplayer NAME) plays the notes of _note players\n\
         (default: 4Voices-NotePlayer when installed; --noteplayer builtin has the\n\
         built-in renderer play them). --68k-players\n\
         plays the formats the open replayers know with the 68k players instead\n\
         (--player NAME always does).";

const USAGE_TAIL: &str = "--led off renders with the LED filter\n\
         ignored (UADE's start state; an Amiga starts with it on); auto (the\n\
         default, also spelled on) follows the player's LED; force switches it on\n\
         whatever the player does. --filter picks the Amiga filter model (auto:\n\
         A500, none for PC formats; the A1200 has no fixed filter). --separation\n\
         (default 0.7; 1 = hard Amiga panning) and --mono shape the Paula mix.\n\
         --interp and --xmp-filter are libxmp's mixer's (linear, on). --volume V\n\
         sets the output volume (default 0.8; UADE's level is 1). --rate HZ is the\n\
         --wav render's sample rate (8000 to 192000, default 48000). With --wav the\n\
         stages in effect are printed on stderr (`output: ...`, `led-switched: ...`).\n\
         An archive (LhA, zip, tar, .tar.gz, .tar.xz) is extracted into a cache\n\
         directory ($AMIGAPLAYER_ARCHIVE_CACHE, else ~/.cache/amigaplayer-rs/archives)\n\
         and its entries play in order, those no player takes skipped (with --wav:\n\
         one after the other, each up to --seconds); --archive-entry NAME plays only\n\
         the entry of that name (its path in the archive or its file name, any case).\n\
         A gzip or xz file of one module plays as the module.";

fn parse_args() -> Args {
    let mut a = Args {
        #[cfg(feature = "host-68k")]
        players: None,
        #[cfg(feature = "host-68k")]
        eagleplayers: None,
        #[cfg(feature = "host-68k")]
        genies: None,
        #[cfg(feature = "host-68k")]
        libs: None,
        #[cfg(feature = "host-68k")]
        use_genies: Vec::new(),
        #[cfg(feature = "host-68k")]
        only_player: None,
        #[cfg(feature = "host-68k")]
        open_replayers: true,
        wav: None, rate: SAMPLE_RATE, seconds: 60.0, subsong: 0, led: true, led_force: false, filter: None, separation: None, mono: false, interp: Interp::Linear, xmp_filter: true, volume: None, archive_entry: None, files: Vec::new() };
    // (a file name need not be UTF-8: options are read as text, file names as given)
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mut it = raw.iter().map(|o| o.to_string_lossy().into_owned());
    while let Some(s) = it.next() {
        // not a silent no-op: these options act only in a build that has the 68k players
        #[cfg(not(feature = "host-68k"))]
        if NEEDS_68K.contains(&s.as_str()) {
            eprintln!("amigaplayer-rs: {s}: built without 68k player support; rebuild with --features host-68k");
            std::process::exit(2);
        }
        match s.as_str() {
            #[cfg(feature = "host-68k")]
            "--players" => a.players = it.next().map(PathBuf::from),
            #[cfg(feature = "host-68k")]
            "--eagleplayers" => a.eagleplayers = it.next().map(PathBuf::from),
            #[cfg(feature = "host-68k")]
            "--player" => a.only_player = it.next(),
            // (the open replayers are the default; the flag stays accepted)
            #[cfg(feature = "host-68k")]
            "--open-replayers" => a.open_replayers = true,
            #[cfg(not(feature = "host-68k"))]
            "--open-replayers" => {}
            #[cfg(feature = "host-68k")]
            "--68k-players" => a.open_replayers = false,
            #[cfg(feature = "host-68k")]
            "--genies" => a.genies = it.next().map(PathBuf::from),
            #[cfg(feature = "host-68k")]
            "--libs" => a.libs = it.next().map(PathBuf::from),
            #[cfg(feature = "host-68k")]
            "--genie" | "--noteplayer" => a.use_genies.push(it.next().unwrap_or_else(|| usage())),
            "--wav" => a.wav = it.next().map(PathBuf::from),
            "--rate" => {
                a.rate = it.next().and_then(|v| v.parse().ok()).filter(|r| (8000..=192000).contains(r)).unwrap_or_else(|| usage())
            }
            "--seconds" => a.seconds = it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--subsong" => a.subsong = it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--led" => {
                a.led = match it.next().as_deref() {
                    Some("on" | "auto") => true,
                    Some("off") => false,
                    Some("force") => {
                        a.led_force = true;
                        true
                    }
                    _ => usage(),
                }
            }
            "--filter" => {
                a.filter = match it.next().as_deref() {
                    Some("a500") => Some(Filter::A500),
                    Some("a1200") => Some(Filter::A1200),
                    Some("none") => Some(Filter::None),
                    Some("auto") => None,
                    _ => usage(),
                }
            }
            "--separation" => a.separation = Some(it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage())),
            "--mono" => a.mono = true,
            "--interp" => {
                a.interp = match it.next().as_deref() {
                    Some("nearest") => Interp::Nearest,
                    Some("linear") => Interp::Linear,
                    Some("spline") => Interp::Spline,
                    _ => usage(),
                }
            }
            "--xmp-filter" => {
                a.xmp_filter = match it.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => usage(),
                }
            }
            "--volume" => a.volume = Some(it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage())),
            "--archive-entry" => a.archive_entry = Some(it.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            _ => a.files.push(PathBuf::from(&raw[raw.len() - it.len() - 1])),
        }
    }
    a
}

fn write_wav(path: &Path, rate: u32, frames: &[f32]) -> Result<()> {
    let mut d = Vec::with_capacity(44 + frames.len() * 2);
    let data_len = (frames.len() * 2) as u32;
    d.extend_from_slice(b"RIFF");
    d.extend_from_slice(&(36 + data_len).to_le_bytes());
    d.extend_from_slice(b"WAVEfmt ");
    d.extend_from_slice(&16u32.to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&2u16.to_le_bytes());
    d.extend_from_slice(&rate.to_le_bytes());
    d.extend_from_slice(&(rate * 4).to_le_bytes());
    d.extend_from_slice(&4u16.to_le_bytes());
    d.extend_from_slice(&16u16.to_le_bytes());
    d.extend_from_slice(b"data");
    d.extend_from_slice(&data_len.to_le_bytes());
    for &s in frames {
        d.extend_from_slice(&((s * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, d)?;
    Ok(())
}

/// The output options of the command line, for the window and `--wav` alike.
fn apply_settings(args: &Args, s: &mut Settings) {
    s.led_filter = args.led;
    s.led_force = args.led_force;
    s.filter_model = args.filter;
    s.mono = args.mono;
    s.interp = args.interp;
    s.xmp_filter = args.xmp_filter;
    if let Some(v) = args.volume {
        s.volume = v;
    }
    if let Some(v) = args.separation {
        s.separation = v;
    }
}

fn render_wav(args: &Args) -> Result<()> {
    let wav = args.wav.as_ref().unwrap();
    let Some(file) = args.files.first() else { bail!("--wav needs a module file") };
    let rate = args.rate;
    let mut e = engine::Engine::new(rate as f64);
    #[cfg(feature = "host-68k")]
    {
        e.m.os.libs_dir = args.libs.clone().or_else(default_libs_dir);
        // (before the players load: a player without a check is loaded only when chosen by name)
        e.only_player = args.only_player.clone();
        // The directories are read when a module needs a 68k player (see `set_player_dirs`),
        // at once when a genie is chosen by name (it must be loaded to be found).
        e.set_player_dirs(engine::PlayerDirs {
            players: args.players.clone().or_else(default_players_dir),
            eagleplayers: args.eagleplayers.clone().or_else(default_eagleplayers_dir),
            genies: args.genies.clone().or_else(default_genies_dir),
        });
        if !args.use_genies.is_empty() {
            e.load_pending_players();
            report_loads(&mut e);
        }
        for g in &args.use_genies {
            e.use_genie(g)?;
        }
        e.open_replayers = args.open_replayers;
    }
    #[cfg(not(feature = "host-68k"))]
    {
        e.open_replayers = true;
    }
    apply_settings(args, &mut e.settings);
    if let Some(n) = std::env::var("DELI_CPU_SPEED").ok().and_then(|v| v.parse::<u64>().ok()) {
        // the guest CPU's 68000 cycles per colour clock are 2; a larger speed divides them
        amigaplayer_rs::bus::CPU_CYCLES.store(amigaplayer_rs::bus::CPU_CYCLES_68000 * n.max(1), std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var_os("DELI_UADE").is_some() {
        e.set_uade_compat();
    }
    if let Some(v) = args.separation {
        e.settings.separation = v;
    }
    if std::env::var_os("DELI_DMA_LOG").is_some() {
        e.m.bus.paula.dma_log = Some(Vec::new());
    }
    if std::env::var_os("DELI_WRITE_LOG").is_some() {
        e.m.bus.write_log = Some(Vec::new());
        e.m.bus.paula.native_log = Some(Vec::new());
    }
    // an archive: its entries in order, each rendered as a module given alone would be (up to
    // --seconds or its song end), one after the other; the entries no player takes are skipped
    let items = amigaplayer_rs::playlist::items(file, args.archive_entry.as_deref()).map_err(anyhow::Error::msg)?;
    let total = (args.seconds * rate as f64) as usize;
    let mut out: Vec<f32> = Vec::new();
    let mut done = 0;
    let mut played = 0;
    for item in &items {
        if let Some(name) = &item.entry {
            let r = e.play_file(&item.path);
            #[cfg(feature = "host-68k")]
            report_loads(&mut e);
            if let Err(err) = r {
                eprintln!("entry {name}: skipped ({err})");
                continue;
            }
            eprintln!("entry {name}");
        } else {
            let r = e.play_file(&item.path);
            #[cfg(feature = "host-68k")]
            report_loads(&mut e);
            r?;
        }
        played += 1;
        for _ in 0..args.subsong.unsigned_abs() {
            e.change_subsong(args.subsong.signum())?;
        }
        if let Some(p) = &e.playing {
            eprintln!(
                "playing \"{}\" [{}] subsongs {}..{} (current {})",
                p.module_name, p.format, p.sub_min, p.sub_max, p.sub
            );
        }
        // printed after the render: the built-in voices are only known to be in use
        // once the player has played a note
        let mut report = e.output_report();
        let start = done;
        out.resize(out.len() + total * 2, 0.0);
        while done - start < total {
            // blocks of 0.1 s
            let n = (total - (done - start)).min(rate as usize / 10);
            e.render(&mut out[done * 2..(done + n) * 2]);
            done += n;
            if e.playing.is_none() {
                break;
            }
            if e.songend() {
                eprintln!("song end after {:.1}s", (done - start) as f64 / rate as f64);
                break;
            }
        }
        out.truncate(done * 2);
        if e.playing.is_some() {
            report = e.output_report();
        }
        eprintln!("{report}");
        eprintln!("{}", e.led_report());
        if let Some(err) = &e.last_error {
            eprintln!("error: {err}");
        }
        for c in &e.m.os.missing_calls {
            let what = if c.fatal { "error" } else { "warning" };
            eprintln!("{what}: the module called {c}, which the host does not implement");
        }
        if !e.m.os.console.is_empty() {
            eprintln!("guest console:\n{}", e.m.os.console);
        }
    }
    if played == 0 {
        bail!("{}: no entry of the archive plays", file.display());
    }
    let peak = out.iter().fold(0f32, |a, &b| a.max(b.abs()));
    eprintln!("{} frames written, peak {:.3}", done, peak);
    if let (Ok(p), Some(l)) = (std::env::var("DELI_WRITE_LOG"), &mut e.m.bus.write_log) {
        // the built-in ProTracker's writes, in time order with the bus's
        l.extend(e.m.bus.paula.native_log.take().unwrap_or_default());
        l.sort_by_key(|w| w.2);
        // where the module was loaded, so logs of runs that loaded it elsewhere compare
        let head: String = e.m.deli.list.first().map(|(a, n)| format!("# module {a:06x} {n}\n")).unwrap_or_default();
        let text: String =
            head + &l.iter().map(|(a, v, t)| format!("{a:06x} {v:04x} {t}\n")).collect::<String>();
        std::fs::write(&p, text)?;
    }
    // DELI_DMA_LOG=FILE: every audio DMA word fetched (CLOCK CHANNEL ADDRESS WORD)
    if let (Some(p), Some(l)) = (std::env::var_os("DELI_DMA_LOG"), e.m.bus.paula.dma_log.take()) {
        let text: String = l.iter().map(|(t, c, a, w)| format!("{t} {c} {a:06x} {w:04x}\n")).collect();
        std::fs::write(p, text)?;
    }
    // DELI_MEM_DUMP=FILE: guest memory as it is when the render ends, for comparing two runs
    if let Some(p) = std::env::var_os("DELI_MEM_DUMP") {
        std::fs::write(p, &e.m.bus.ram)?;
    }
    let n = e.m.native_stats;
    if n.calls > 0 || n.patched_calls > 0 {
        let pct = n.native as f64 * 100.0 / (n.native + n.interpreted).max(1) as f64;
        eprintln!(
            "native: {} calls, {} instructions native, {} interpreted ({pct:.2}% native), {} calls after self-modification",
            n.calls, n.native, n.interpreted, n.patched_calls
        );
    }
    write_wav(wav, rate, &out)
}

/// Prints what loading the 68k player directories found.
#[cfg(feature = "host-68k")]
fn report_loads(e: &mut engine::Engine) {
    for l in e.take_load_report() {
        eprintln!("{l}");
    }
}

/// The window's list: FILES, or with --archive-entry the entry of that name in each archive.
fn window_files(args: &Args) -> Result<Vec<PathBuf>> {
    let Some(want) = args.archive_entry.as_deref() else { return Ok(args.files.clone()) };
    let mut v = Vec::new();
    for f in &args.files {
        v.extend(amigaplayer_rs::playlist::items(f, Some(want)).map_err(anyhow::Error::msg)?.into_iter().map(|i| i.path));
    }
    Ok(v)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = parse_args();
    let r = if args.wav.is_some() {
        render_wav(&args)
    } else {
        window_files(&args).and_then(|files| {
            ui::run(
                #[cfg(feature = "host-68k")]
                ui::Players68k {
                    dirs: engine::PlayerDirs {
                        players: args.players.clone().or_else(default_players_dir),
                        eagleplayers: args.eagleplayers.clone().or_else(default_eagleplayers_dir),
                        genies: args.genies.clone().or_else(default_genies_dir),
                    },
                    libs: args.libs.clone().or_else(default_libs_dir),
                    use_genies: args.use_genies.clone(),
                    only_player: args.only_player.clone(),
                    open_replayers: args.open_replayers,
                },
                files,
                {
                    let mut s = Settings::default();
                    apply_settings(&args, &mut s);
                    s
                },
            )
        })
    };
    if let Err(e) = r {
        eprintln!("amigaplayer-rs: {e:#}");
        std::process::exit(1);
    }
}
