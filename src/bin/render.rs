//! `amigaplayer-render`: a module file to WAV or raw PCM, on a file or on standard output.
//! No window, no sound card, no SDL: for servers and pipes (`amigaplayer-render x.mod | ffmpeg ...`).
//!
//! Unlike the library's `Song` it hosts 68k code, so custom modules (`CUST.*`, which carry
//! their own player) play, and so do the formats of original DeliPlayers / Eagleplayers when
//! their directories are given. Nothing is read from the user's configuration directories.
//!
//! Standard output carries only audio (or the `--info` line), and only once the module has
//! started: a file that is missing or not recognised gives exit status 1 and no output.

use amigaplayer_rs::engine::{Backend, Engine};
use anyhow::{bail, Context, Result};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

const USAGE: &str = "usage: amigaplayer-render [OPTIONS] MODULE

  --out FILE        where the audio goes; `-` (the default) is standard output
  --format wav|raw  WAV (the default) or headerless samples; 16-bit little-endian stereo
  --rate HZ         sample rate, 8000 to 192000 (default 44100)
  --subsong N|first|default
                    subsong to play: a number, the lowest one, or the module's own
                    start (the default)
  --seconds S       stop after S seconds (default 600; 0 = no limit)
  --silence S       stop after S seconds of silence (default 10; 0 = never)
  --no-songend      play on when the player reports the song's end
  --volume V        output volume (default 0.8; UADE's level is 1)
  --info            print one line of JSON about the module on standard output
                    (title, format, backend, subsongs) and render nothing
  --players DIR, --eagleplayers DIR, --genies DIR
                    original 68k DeliPlayers, Eagleplayers and DeliGenies (default: none)
  --68k-first       let those players take a module before the built-in replayers

exit status: 0 played, 1 the module could not be played, 2 wrong usage";

struct Args {
    out: String,
    raw: bool,
    rate: u32,
    subsong: Sub,
    seconds: f64,
    silence: f64,
    songend: bool,
    volume: Option<f32>,
    info: bool,
    players: Option<PathBuf>,
    eagleplayers: Option<PathBuf>,
    genies: Option<PathBuf>,
    m68k_first: bool,
    file: PathBuf,
}

#[derive(Clone, Copy)]
enum Sub {
    Default,
    First,
    Number(u16),
}

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut a = Args {
        out: "-".into(),
        raw: false,
        rate: 44100,
        subsong: Sub::Default,
        seconds: 600.0,
        silence: 10.0,
        songend: true,
        volume: None,
        info: false,
        players: None,
        eagleplayers: None,
        genies: None,
        m68k_first: false,
        file: PathBuf::new(),
    };
    let mut file = None;
    let mut it = std::env::args().skip(1);
    fn num<T: std::str::FromStr>(v: Option<String>) -> T {
        v.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage())
    }
    while let Some(s) = it.next() {
        match s.as_str() {
            "--out" => a.out = it.next().unwrap_or_else(|| usage()),
            "--format" => {
                a.raw = match it.next().as_deref() {
                    Some("wav") => false,
                    Some("raw") => true,
                    _ => usage(),
                }
            }
            "--rate" => a.rate = num(it.next()),
            "--subsong" => {
                a.subsong = match it.next().as_deref() {
                    Some("default") => Sub::Default,
                    Some("first") => Sub::First,
                    Some(n) => Sub::Number(n.parse().unwrap_or_else(|_| usage())),
                    None => usage(),
                }
            }
            "--seconds" => a.seconds = num(it.next()),
            "--silence" => a.silence = num(it.next()),
            "--no-songend" => a.songend = false,
            "--volume" => a.volume = Some(num(it.next())),
            "--info" => a.info = true,
            "--players" => a.players = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--eagleplayers" => a.eagleplayers = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--genies" => a.genies = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--68k-first" => a.m68k_first = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ if s.starts_with("--") => usage(),
            _ if file.is_none() => file = Some(PathBuf::from(s)),
            _ => usage(),
        }
    }
    a.file = file.unwrap_or_else(|| usage());
    if !(8000..=192000).contains(&a.rate) || !a.seconds.is_finite() || a.seconds < 0.0 || !a.silence.is_finite() || a.silence < 0.0 {
        usage();
    }
    a
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn wav_header(rate: u32, data_len: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&data_len.saturating_add(36).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&2u16.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * 4).to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

fn run(a: &Args) -> Result<()> {
    let mut e = Engine::new(a.rate as f64);
    if let Some(d) = &a.players {
        e.load_players(d);
    }
    if let Some(d) = &a.eagleplayers {
        e.load_eagleplayers(d);
    }
    if let Some(d) = &a.genies {
        e.load_genies(d);
    }
    e.open_replayers = !a.m68k_first;
    if let Some(v) = a.volume {
        e.settings.volume = v;
    }
    e.play_file(&a.file)?;
    let (lo, hi, start) = match &e.playing {
        Some(p) => (p.sub_min, p.sub_max, p.sub),
        None => bail!("{}", e.last_error.clone().unwrap_or_else(|| "the module did not start".into())),
    };
    let want = match a.subsong {
        Sub::Default => start,
        Sub::First => lo,
        Sub::Number(n) => {
            if n < lo || n > hi {
                bail!("subsong {n} is not one of {lo}..{hi}");
            }
            n
        }
    };
    if !a.info {
        let step = if want > start { 1 } else { -1 };
        for _ in 0..(want as i32 - start as i32).abs() {
            e.change_subsong(step)?;
        }
    }
    let Some(p) = &e.playing else { bail!("the subsong did not start") };
    if !a.info && p.sub != want {
        bail!("subsong {want} did not start");
    }
    let backend = match p.backend {
        Backend::Deli(_) => "68k",
        Backend::Native(_) => "protracker",
        Backend::Replay(_) => "open-replayer",
        Backend::Mix(_) => "xmp",
        Backend::Pcm(_) => "pcm",
    };
    let info = format!(
        "{{\"title\":{},\"format\":{},\"author\":{},\"backend\":\"{backend}\",\"subsong_min\":{lo},\"subsong_max\":{hi},\"subsong_default\":{start},\"subsong\":{}}}",
        json_str(&p.module_name),
        json_str(&p.format),
        json_str(&p.author),
        p.sub
    );
    if a.info {
        println!("{info}");
        return Ok(());
    }
    eprintln!("{info}");

    let to_stdout = a.out == "-";
    let mut file = if to_stdout { None } else { Some(std::fs::File::create(&a.out).with_context(|| a.out.clone())?) };
    let stdout = std::io::stdout();
    let mut w: Box<dyn Write> = match &file {
        Some(f) => Box::new(std::io::BufWriter::new(f.try_clone()?)),
        None => Box::new(std::io::BufWriter::new(stdout.lock())),
    };
    if !a.raw {
        // a stream's length is not known when the header goes out: the largest, as is usual
        w.write_all(&wav_header(a.rate, if to_stdout { u32::MAX - 36 } else { 0 }))?;
    }
    let limit = if a.seconds > 0.0 { (a.seconds * a.rate as f64) as u64 } else { u64::MAX };
    let quiet_limit = if a.silence > 0.0 { (a.silence * a.rate as f64) as u64 } else { u64::MAX };
    let mut buf = vec![0f32; 2 * 4096];
    let mut bytes = Vec::with_capacity(4 * 4096);
    let (mut done, mut quiet) = (0u64, 0u64);
    while done < limit {
        let n = (limit - done).min(4096) as usize;
        e.render(&mut buf[..2 * n]);
        bytes.clear();
        let mut silent = true;
        for &s in &buf[..2 * n] {
            let v = (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
            silent &= v == 0;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        if let Err(err) = w.write_all(&bytes) {
            // the reader went away (`| head`, a stopped ffmpeg): not a failure of the render
            if err.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(err.into());
        }
        done += n as u64;
        quiet = if silent { quiet + n as u64 } else { 0 };
        if e.playing.is_none() || (a.songend && e.songend()) || quiet >= quiet_limit {
            break;
        }
    }
    match w.flush() {
        Err(err) if err.kind() != std::io::ErrorKind::BrokenPipe => return Err(err.into()),
        _ => {}
    }
    drop(w);
    if let (Some(f), false) = (&mut file, a.raw) {
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&wav_header(a.rate, (done * 4).min(u32::MAX as u64 - 36) as u32))?;
    }
    eprintln!("{:.2} s rendered", done as f64 / a.rate as f64);
    Ok(())
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let a = parse_args();
    if let Err(e) = run(&a) {
        eprintln!("amigaplayer-render: {e:#}");
        std::process::exit(1);
    }
}
