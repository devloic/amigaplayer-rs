mod ui;

use amigaplayer_rs::paulaout::Filter;
use amigaplayer_rs::xmp::mixer::Interp;
use amigaplayer_rs::{default_eagleplayers_dir, default_genies_dir, default_players_dir, engine, SAMPLE_RATE};
use amigaplayer_rs::engine::Settings;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

struct Args {
    players: Option<PathBuf>,
    eagleplayers: Option<PathBuf>,
    genies: Option<PathBuf>,
    use_genies: Vec<String>,
    only_player: Option<String>,
    open_replayers: bool,
    wav: Option<PathBuf>,
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
    files: Vec<PathBuf>,
}

fn usage() -> ! {
    eprintln!(
        "usage: amigaplayer-rs [--players DIR] [--eagleplayers DIR] [--player NAME] [--open-replayers]\n\
         \x20                 [--genies DIR] [--genie NAME]... [--noteplayer NAME]\n\
         \x20                 [--wav OUT.wav [--seconds N] [--subsong N] [--volume V]]\n\
         \x20                 [--filter a500|a1200|none|auto] [--led auto|on|off|force]\n\
         \x20                 [--separation 0..1] [--mono] [--interp nearest|linear|spline]\n\
         \x20                 [--xmp-filter on|off]\n\
         \x20                 [FILES/DIRS...]\n\
         \n\
         Without --wav the SDL3 player window opens with FILES as the module list.\n\
         --players DIR holds the original 68k DeliPlayers (default: $DELIPLAYERS,\n\
         ~/.config/amigaplayer-rs/DeliPlayers, ./DeliPlayers), --eagleplayers DIR the\n\
         Eagleplayers (default: $EAGLEPLAYERS, ~/.config/amigaplayer-rs/Eagleplayers,\n\
         ./Eagleplayers); DeliPlayers are tried first. --player NAME tries only the\n\
         player of that name. --genies DIR holds the DeliGenies (default: $DELIGENIES,\n\
         ~/.config/amigaplayer-rs/DeliGenies, ./DeliGenies); --genie NAME runs one, and a\n\
         NotePlayer genie (--noteplayer NAME) plays the notes of _note players\n\
         (default: 4Voices-NotePlayer when installed; --noteplayer builtin has the\n\
         built-in renderer play them). --open-replayers plays the formats the\n\
         open replayers (Rust ports of NostalgicPlayer's players) know with them\n\
         instead of the 68k players. --led off renders with the LED filter\n\
         ignored (UADE's start state; an Amiga starts with it on); auto (the\n\
         default, also spelled on) follows the player's LED; force switches it on\n\
         whatever the player does. --filter picks the Amiga filter model (auto:\n\
         A500, none for PC formats; the A1200 has no fixed filter). --separation\n\
         (default 0.7; 1 = hard Amiga panning) and --mono shape the Paula mix.\n\
         --interp and --xmp-filter are libxmp's mixer's (linear, on). --volume V\n\
         sets the output volume (default 0.8; UADE's level is 1). With --wav the\n\
         stages in effect are printed on stderr (`output: ...`, `led-switched: ...`)."
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut a = Args { players: None, eagleplayers: None, genies: None, use_genies: Vec::new(), only_player: None, open_replayers: false, wav: None, seconds: 60.0, subsong: 0, led: true, led_force: false, filter: None, separation: None, mono: false, interp: Interp::Linear, xmp_filter: true, volume: None, files: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(s) = it.next() {
        match s.as_str() {
            "--players" => a.players = it.next().map(PathBuf::from),
            "--eagleplayers" => a.eagleplayers = it.next().map(PathBuf::from),
            "--player" => a.only_player = it.next(),
            "--open-replayers" => a.open_replayers = true,
            "--genies" => a.genies = it.next().map(PathBuf::from),
            "--genie" | "--noteplayer" => a.use_genies.push(it.next().unwrap_or_else(|| usage())),
            "--wav" => a.wav = it.next().map(PathBuf::from),
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
            "-h" | "--help" => usage(),
            _ => a.files.push(PathBuf::from(s)),
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
    let mut e = engine::Engine::new(SAMPLE_RATE as f64);
    if let Some(dir) = args.players.clone().or_else(default_players_dir) {
        let n = e.load_players(&dir);
        eprintln!("{n} players loaded from {}", dir.display());
    }
    if let Some(dir) = args.eagleplayers.clone().or_else(default_eagleplayers_dir) {
        let n = e.load_eagleplayers(&dir);
        eprintln!("{n} eagleplayers loaded from {}", dir.display());
    }
    if let Some(dir) = args.genies.clone().or_else(default_genies_dir) {
        let n = e.load_genies(&dir);
        eprintln!("{n} genies loaded from {}", dir.display());
    }
    for g in &args.use_genies {
        e.use_genie(g)?;
    }
    e.only_player = args.only_player.clone();
    e.open_replayers = args.open_replayers;
    apply_settings(args, &mut e.settings);
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
    e.play_file(file)?;
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
    let total = (args.seconds * SAMPLE_RATE as f64) as usize;
    let mut out = vec![0f32; total * 2];
    let mut done = 0;
    while done < total {
        let n = (total - done).min(4800);
        e.render(&mut out[done * 2..(done + n) * 2]);
        done += n;
        if e.playing.is_none() {
            break;
        }
        if e.songend() {
            eprintln!("song end after {:.1}s", done as f64 / SAMPLE_RATE as f64);
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
    if !e.m.os.console.is_empty() {
        eprintln!("guest console:\n{}", e.m.os.console);
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
    write_wav(wav, SAMPLE_RATE, &out)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = parse_args();
    let r = if args.wav.is_some() {
        render_wav(&args)
    } else {
        ui::run(
            args.players.clone().or_else(default_players_dir),
            args.eagleplayers.clone().or_else(default_eagleplayers_dir),
            args.genies.clone().or_else(default_genies_dir),
            &args.use_genies,
            args.files.clone(),
            {
                let mut s = Settings::default();
                apply_settings(&args, &mut s);
                s
            },
        )
    };
    if let Err(e) = r {
        eprintln!("amigaplayer-rs: {e:#}");
        std::process::exit(1);
    }
}
