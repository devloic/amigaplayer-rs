mod ui;

use deliplayer::{default_eagleplayers_dir, default_genies_dir, default_players_dir, engine, SAMPLE_RATE};

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
    files: Vec<PathBuf>,
}

fn usage() -> ! {
    eprintln!(
        "usage: deliplayer [--players DIR] [--eagleplayers DIR] [--player NAME] [--open-replayers]\n\
         \x20                 [--genies DIR] [--genie NAME]... [--noteplayer NAME]\n\
         \x20                 [--wav OUT.wav [--seconds N] [--subsong N]] [FILES/DIRS...]\n\
         \n\
         Without --wav the SDL3 player window opens with FILES as the module list.\n\
         --players DIR holds the original 68k DeliPlayers (default: $DELIPLAYERS,\n\
         ~/.config/deliplayer/DeliPlayers, ./DeliPlayers), --eagleplayers DIR the\n\
         Eagleplayers (default: $EAGLEPLAYERS, ~/.config/deliplayer/Eagleplayers,\n\
         ./Eagleplayers); DeliPlayers are tried first. --player NAME tries only the\n\
         player of that name. --genies DIR holds the DeliGenies (default: $DELIGENIES,\n\
         ~/.config/deliplayer/DeliGenies, ./DeliGenies); --genie NAME runs one, and a\n\
         NotePlayer genie (--noteplayer NAME) plays the notes of _note players\n\
         (default: 4Voices-NotePlayer when installed; --noteplayer builtin has the\n\
         built-in renderer play them). --open-replayers plays the formats the\n\
         open replayers (Rust ports of NostalgicPlayer's players) know with them\n\
         instead of the 68k players."
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut a = Args { players: None, eagleplayers: None, genies: None, use_genies: Vec::new(), only_player: None, open_replayers: false, wav: None, seconds: 60.0, subsong: 0, files: Vec::new() };
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
    if std::env::var_os("DELI_UADE").is_some() {
        e.set_uade_compat();
    }
    if std::env::var_os("DELI_WRITE_LOG").is_some() {
        e.m.bus.write_log = Some(Vec::new());
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
    if let Some(err) = &e.last_error {
        eprintln!("error: {err}");
    }
    if !e.m.os.console.is_empty() {
        eprintln!("guest console:\n{}", e.m.os.console);
    }
    let peak = out.iter().fold(0f32, |a, &b| a.max(b.abs()));
    eprintln!("{} frames written, peak {:.3}", done, peak);
    if let (Ok(p), Some(l)) = (std::env::var("DELI_WRITE_LOG"), &e.m.bus.write_log) {
        // where the module was loaded, so logs of runs that loaded it elsewhere compare
        let head: String = e.m.deli.list.first().map(|(a, n)| format!("# module {a:06x} {n}\n")).unwrap_or_default();
        let text: String =
            head + &l.iter().map(|(a, v, t)| format!("{a:06x} {v:04x} {t}\n")).collect::<String>();
        std::fs::write(&p, text)?;
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
        )
    };
    if let Err(e) = r {
        eprintln!("deliplayer: {e:#}");
        std::process::exit(1);
    }
}
