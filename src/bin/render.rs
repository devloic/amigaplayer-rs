//! `amigaplayer-render`: a module file to WAV or raw PCM, on a file or on standard output.
//! No window, no sound card, no SDL: for servers and pipes (`amigaplayer-render x.mod | ffmpeg ...`).
//!
//! Unlike the library's `Song` it hosts 68k code, so custom modules (`CUST.*`, which carry
//! their own player) play (feature `custom-modules`, in the default build), and, in a build
//! with `host-68k`, so do the formats of original DeliPlayers / Eagleplayers when their
//! directories are given. Nothing is read from the user's configuration directories.
//!
//! Standard output carries only audio (or the `--info` line), and only once the module has
//! started: a file that is missing or not recognised gives exit status 1 and no output.
//!
//! Built with the `songdb` feature it also looks the file up in audacious-uade's song database
//! (`--songdb DIR`: lengths, author, album, ...; the data is not part of this program) and can
//! use the length found there as a place to stop (`--stop-at songdb`).
//!
//! An archive (LhA, zip, tar, gzip / xz of a tar) plays as the list of its entries, extracted
//! into a cache directory (see `amigaplayer_rs::playlist`).
//!
//! `--songlengths FILE` does the same for C64 tunes with HVSC's `Songlengths.md5`
//! (`--stop-at songlengths`: stop at that length, since a SID tune never ends by itself).

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
  --songdb DIR      look the file up in the song database of audacious-uade in DIR
                    (its conf/songdb: xxh32idx.tsv, songlengths.tsv, modinfos.tsv,
                    metadata.tsv; CC BY-NC-SA 4.0, not shipped with this program) and
                    add what it knows to the JSON, as its member songdb (null: not in it).
                    Needs a build with the songdb feature
  --stop-at songdb  with --songdb: once the subsong's length in the database has
                    passed, stop where the output falls digitally silent (for half
                    a second), keeping none of that silence. Never earlier than that
                    length; the other stops (song end, --silence, --seconds) stay.
                    Without an entry for the subsong: as without the option
  --songlengths FILE
                    HVSC's DOCUMENTS/Songlengths.md5 (not shipped with this program): the
                    file's MD5 is looked up there, and the length of the subsong is added
                    to the JSON as songlength_ms (null: not listed)
  --stop-at songlengths
                    with --songlengths: stop when that length is reached (a C64 tune never
                    ends by itself); the other stops stay. Not listed: as without the option
  --players DIR, --eagleplayers DIR, --genies DIR
                    original 68k DeliPlayers, Eagleplayers and DeliGenies (default: none)
  --libs DIR        LIBS:, where a library a player opens is looked for after the
                    module's directory, its libs/ and ../libs/ (default: none)
  --68k-first       let those players take a module before the built-in replayers
  --archive-entry NAME
                    MODULE is an archive: play only its entry NAME (its path in the
                    archive or its file name, in any case)

MODULE may be an archive (LhA, zip, tar, .tar.gz, .tar.xz): it is extracted into a
cache directory ($AMIGAPLAYER_ARCHIVE_CACHE, else ~/.cache/amigaplayer-rs/archives),
where a module finds the companion files of the archive beside it, and its entries
play one after the other in the archive's order, each with the stops above; the
entries no player takes are skipped (named on standard error). --info prints one
line per entry that plays, with its name as archive_entry. A gzip or xz file of one
module plays as that module.

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
    songdb: Option<PathBuf>,
    stop_songdb: bool,
    songlengths: Option<PathBuf>,
    stop_songlengths: bool,
    players: Option<PathBuf>,
    eagleplayers: Option<PathBuf>,
    genies: Option<PathBuf>,
    libs: Option<PathBuf>,
    m68k_first: bool,
    archive_entry: Option<String>,
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
        songdb: None,
        stop_songdb: false,
        songlengths: None,
        stop_songlengths: false,
        players: None,
        eagleplayers: None,
        genies: None,
        libs: None,
        m68k_first: false,
        archive_entry: None,
        file: PathBuf::new(),
    };
    let mut file = None;
    // (a file name need not be UTF-8: options are read as text, the file name as given)
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mut it = raw.iter().map(|o| o.to_string_lossy().into_owned());
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
            "--songdb" => a.songdb = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--stop-at" => {
                a.stop_songdb = match it.next().as_deref() {
                    Some("songdb") => true,
                    Some("songlengths") => {
                        a.stop_songlengths = true;
                        a.stop_songdb
                    }
                    Some("default") => {
                        a.stop_songlengths = false;
                        false
                    }
                    _ => usage(),
                }
            }
            "--songlengths" => a.songlengths = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--players" => a.players = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--eagleplayers" => a.eagleplayers = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--genies" => a.genies = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--libs" => a.libs = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--68k-first" => a.m68k_first = true,
            "--archive-entry" => a.archive_entry = Some(it.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ if s.starts_with("--") => usage(),
            _ if file.is_none() => file = Some(PathBuf::from(&raw[raw.len() - it.len() - 1])),
            _ => usage(),
        }
    }
    a.file = file.unwrap_or_else(|| usage());
    if !(8000..=192000).contains(&a.rate) || !a.seconds.is_finite() || a.seconds < 0.0 || !a.silence.is_finite() || a.silence < 0.0 {
        usage();
    }
    if a.stop_songlengths && a.songlengths.is_none() {
        eprintln!("amigaplayer-render: --stop-at songlengths needs --songlengths FILE");
        std::process::exit(2);
    }
    if a.stop_songdb && a.songdb.is_none() {
        eprintln!("amigaplayer-render: --stop-at songdb needs --songdb DIR");
        std::process::exit(2);
    }
    #[cfg(not(feature = "songdb"))]
    if a.songdb.is_some() {
        eprintln!(
            "amigaplayer-render: --songdb needs a build with the songdb feature:\n  \
             cargo build --release --no-default-features --features \"custom-modules songdb\" --bin amigaplayer-render"
        );
        std::process::exit(2);
    }
    #[cfg(not(feature = "host-68k"))]
    {
        let given: Vec<&str> = [
            (a.players.is_some(), "--players"),
            (a.eagleplayers.is_some(), "--eagleplayers"),
            (a.genies.is_some(), "--genies"),
            (a.libs.is_some(), "--libs"),
            (a.m68k_first, "--68k-first"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| *n)
        .collect();
        if !given.is_empty() {
            eprintln!(
                "amigaplayer-render: {}: built without 68k player support; rebuild with --features host-68k",
                given.join(", ")
            );
            std::process::exit(2);
        }
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

/// What the song database says about the file, as the value of the JSON's `"songdb"`:
/// `duration_ms` and `songend` are those of subsong `sub`.
#[cfg(feature = "songdb")]
fn songdb_json(info: Option<&amigaplayer_rs::songdb::Info>, sub: u16) -> String {
    let Some(i) = info else { return "null".into() };
    let text = |v: &str| if v.is_empty() { "null".to_string() } else { json_str(v) };
    let number = |v: u32| if v == 0 { "null".to_string() } else { v.to_string() };
    let cur = i.subsong(sub);
    let subs: Vec<String> = i
        .subsongs
        .iter()
        .map(|s| {
            format!(
                "{{\"subsong\":{},\"duration_ms\":{},\"songend\":{},\"duplicate\":{}}}",
                s.subsong,
                s.length_ms,
                json_str(s.status.as_str()),
                s.is_duplicate
            )
        })
        .collect();
    format!(
        "{{\"key\":{},\"duration_ms\":{},\"songend\":{},\"author\":{},\"album\":{},\"publisher\":{},\"year\":{},\"format\":{},\"channels\":{},\"subsongs\":[{}]}}",
        json_str(&i.key),
        cur.map_or("null".to_string(), |s| s.length_ms.to_string()),
        cur.map_or("null".to_string(), |s| json_str(s.status.as_str())),
        text(&i.author),
        text(&i.album),
        text(&i.publisher),
        number(i.year as u32),
        text(&i.format),
        number(i.channels as u32),
        subs.join(",")
    )
}

/// How long the output has to be digitally silent, past the song database's length, for
/// `--stop-at songdb` to stop there.
const SONGDB_SILENCE_SECS: f64 = 0.5;

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

/// Where the audio goes: opened when the first module has started (a file that does not play
/// gives no output at all).
struct Sink {
    file: Option<std::fs::File>,
    w: Box<dyn Write>,
    /// The frames written to it, over all the modules.
    done: u64,
}

impl Sink {
    fn open(a: &Args) -> Result<Sink> {
        let to_stdout = a.out == "-";
        let file = if to_stdout { None } else { Some(std::fs::File::create(&a.out).with_context(|| a.out.clone())?) };
        let mut w: Box<dyn Write> = match &file {
            Some(f) => Box::new(std::io::BufWriter::new(f.try_clone()?)),
            None => Box::new(std::io::BufWriter::new(std::io::stdout())),
        };
        if !a.raw {
            // a stream's length is not known when the header goes out: the largest, as is usual
            w.write_all(&wav_header(a.rate, if to_stdout { u32::MAX - 36 } else { 0 }))?;
        }
        Ok(Sink { file, w, done: 0 })
    }
}

/// The databases given on the command line, opened once for all the modules.
struct Dbs {
    #[cfg(feature = "songdb")]
    songdb: Option<amigaplayer_rs::songdb::SongDb>,
    songlengths: Option<amigaplayer_rs::songlengths::SongLengths>,
}

/// A module that has started, with the stops its databases give.
struct Started {
    /// where `--stop-at songdb` may stop: the length the database gives the subsong, in frames
    songdb_len: Option<u64>,
    /// `--stop-at songlengths`: the subsong's length in HVSC's list, in frames
    songlength_stop: Option<u64>,
}

fn run(a: &Args) -> Result<()> {
    let mut e = Engine::new(a.rate as f64);
    #[cfg(feature = "host-68k")]
    {
        e.m.os.libs_dir = a.libs.clone();
        if let Some(d) = &a.players {
            e.load_players(d);
        }
        if let Some(d) = &a.eagleplayers {
            e.load_eagleplayers(d);
        }
        if let Some(d) = &a.genies {
            e.load_genies(d);
        }
    }
    e.open_replayers = !a.m68k_first;
    if let Some(v) = a.volume {
        e.settings.volume = v;
    }
    // an archive: its entries in order, those no player takes skipped (`items` gives the file
    // itself for anything else)
    let items = amigaplayer_rs::playlist::items(&a.file, a.archive_entry.as_deref()).map_err(anyhow::Error::msg)?;
    let dbs = Dbs {
        #[cfg(feature = "songdb")]
        songdb: match &a.songdb {
            Some(dir) => Some(amigaplayer_rs::songdb::SongDb::open(dir)?),
            None => None,
        },
        songlengths: match &a.songlengths {
            Some(f) => Some(amigaplayer_rs::songlengths::SongLengths::open(f).with_context(|| f.display().to_string())?),
            None => None,
        },
    };
    let mut sink: Option<Sink> = None;
    let mut played = 0;
    for item in &items {
        let started = match start(&mut e, a, &dbs, item) {
            Ok(s) => s,
            Err(err) => match &item.entry {
                Some(name) => {
                    eprintln!("amigaplayer-render: entry {name}: skipped ({err:#})");
                    continue;
                }
                None => return Err(err),
            },
        };
        played += 1;
        if a.info {
            continue;
        }
        if sink.is_none() {
            sink = Some(Sink::open(a)?);
        }
        if !render_module(&mut e, a, &started, sink.as_mut().unwrap())? {
            // the reader went away (`| head`, a stopped ffmpeg): not a failure of the render
            return Ok(());
        }
    }
    if played == 0 {
        bail!("{}: no entry of the archive plays", a.file.display());
    }
    let Some(mut s) = sink else { return Ok(()) };
    match s.w.flush() {
        Err(err) if err.kind() != std::io::ErrorKind::BrokenPipe => return Err(err.into()),
        _ => {}
    }
    drop(s.w);
    if let (Some(f), false) = (&mut s.file, a.raw) {
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&wav_header(a.rate, (s.done * 4).min(u32::MAX as u64 - 36) as u32))?;
    }
    eprintln!("{:.2} s rendered", s.done as f64 / a.rate as f64);
    Ok(())
}

/// Starts `item` at the subsong asked for and prints its line of JSON (on standard output
/// with `--info`, else on standard error).
fn start(e: &mut Engine, a: &Args, dbs: &Dbs, item: &amigaplayer_rs::playlist::Item) -> Result<Started> {
    let data = std::fs::read(&item.path)?;
    #[cfg(feature = "songdb")]
    let db_info = dbs.songdb.as_ref().map(|db| db.lookup(&data));
    // the HVSC lengths of the file's songs, if it is listed
    let hvsc: Option<Vec<u32>> = dbs.songlengths.as_ref().and_then(|l| l.lookup(&data).map(|l| l.to_vec()));
    let raw = data.clone();
    e.play_data(&item.path, data)?;
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
        #[cfg(feature = "sid")]
        Backend::Sid(_) => "sid",
    };
    #[allow(unused_mut)]
    let mut info = format!(
        "{{\"title\":{},\"format\":{},\"author\":{},\"backend\":\"{backend}\",\"subsong_min\":{lo},\"subsong_max\":{hi},\"subsong_default\":{start},\"subsong\":{},\"text\":{}}}",
        json_str(&p.module_name),
        json_str(&p.format),
        json_str(&p.author),
        p.sub,
        json_str(&amigaplayer_rs::text::module_text(&p.format, &raw))
    );
    // the entry of the archive this is
    if let Some(name) = &item.entry {
        info.pop();
        info.push_str(&format!(",\"archive_entry\":{}}}", json_str(name)));
    }
    #[allow(unused_mut)]
    let mut songdb_len: Option<u64> = None;
    #[cfg(feature = "songdb")]
    if let Some(found) = &db_info {
        // (`--info` does not start the subsong asked for: it describes it all the same)
        let sub = if a.info { want } else { p.sub };
        info.pop();
        info.push_str(&format!(",\"songdb\":{}}}", songdb_json(found.as_ref(), sub)));
        use amigaplayer_rs::songdb::SongEndStatus as S;
        // a length measured on a song that failed or stayed silent says nothing
        let measured = |s: &&amigaplayer_rs::songdb::SubSongInfo| s.length_ms > 0 && !matches!(s.status, S::Error | S::None | S::NoSound);
        if a.stop_songdb {
            songdb_len = found.as_ref().and_then(|i| i.subsong(sub)).filter(measured).map(|s| s.length_ms as u64 * a.rate as u64 / 1000);
        }
    }
    let mut songlength_stop: Option<u64> = None;
    if a.songlengths.is_some() {
        let sub = if a.info { want } else { p.sub };
        let ms = hvsc.as_ref().and_then(|l| l.get(sub as usize).copied());
        info.pop();
        info.push_str(&format!(",\"songlength_ms\":{}}}", ms.map_or("null".to_string(), |v| v.to_string())));
        if a.stop_songlengths {
            songlength_stop = ms.map(|v| v as u64 * a.rate as u64 / 1000);
        }
    }
    if a.info {
        println!("{info}");
    } else {
        eprintln!("{info}");
    }
    Ok(Started { songdb_len, songlength_stop })
}

/// Renders the module that has started into `out`, up to its stops. `Ok(false)` when the
/// reader of the output went away.
fn render_module(e: &mut Engine, a: &Args, st: &Started, out: &mut Sink) -> Result<bool> {
    let songdb_len = st.songdb_len;
    let w = &mut out.w;
    let limit = if a.seconds > 0.0 { (a.seconds * a.rate as f64) as u64 } else { u64::MAX };
    let limit = st.songlength_stop.map_or(limit, |l| limit.min(l));
    let quiet_limit = if a.silence > 0.0 { (a.silence * a.rate as f64) as u64 } else { u64::MAX };
    let mut buf = vec![0f32; 2 * 4096];
    let mut bytes = Vec::with_capacity(4 * 4096);
    let (mut done, mut quiet) = (0u64, 0u64);
    // `--stop-at songdb`: the silent frames in a row up to now, and those of them past the
    // database's length, which are not written until sound follows them
    let window = (SONGDB_SILENCE_SECS * a.rate as f64) as u64;
    let (mut run, mut held) = (0u64, 0u64);
    // (s * 32768).round().clamp(-32768, 32767), without the libm call `round` is on x86-64
    // before SSE4.1: clamping first gives the same (the bounds are whole numbers), and in
    // that range truncation and the fraction left are exact. NaN gives 0 either way.
    let i16_of = |s: f32| {
        let x = (s * 32768.0).clamp(-32768.0, 32767.0);
        let t = x as i32;
        let f = x - t as f32;
        // (without branches: which way a sample rounds is unpredictable)
        (t + (f >= 0.5) as i32 - (f <= -0.5) as i32) as i16
    };
    while done < limit {
        let mut n = (limit - done).min(4096) as usize;
        if let Some(len) = songdb_len.filter(|&len| done < len) {
            // (a chunk ends at that length, so that the output can)
            n = n.min((len - done) as usize);
        }
        e.render(&mut buf[..2 * n]);
        bytes.clear();
        let mut silent = true;
        let past = songdb_len.is_some_and(|len| done >= len);
        for f in buf[..2 * n].chunks_exact(2) {
            let (l, r) = (i16_of(f[0]), i16_of(f[1]));
            let zero = l == 0 && r == 0;
            silent &= zero;
            run = if zero { run + 1 } else { 0 };
            if past && zero {
                held += 1;
                continue;
            }
            // sound after a silence that was held back: the silence is part of the song
            bytes.resize(bytes.len() + 4 * std::mem::take(&mut held) as usize, 0);
            bytes.extend_from_slice(&l.to_le_bytes());
            bytes.extend_from_slice(&r.to_le_bytes());
        }
        if let Err(err) = w.write_all(&bytes) {
            if err.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(false);
            }
            return Err(err.into());
        }
        done += n as u64;
        quiet = if silent { quiet + n as u64 } else { 0 };
        if songdb_len.is_some_and(|len| done >= len) && run >= window {
            // the database's length has passed and the output is silent: the song ends where
            // the silence began, or at that length if it began before (nothing held is written)
            done -= std::mem::take(&mut held);
            break;
        }
        if e.playing.is_none() || (a.songend && e.songend()) || quiet >= quiet_limit {
            break;
        }
    }
    // (a stop for another reason keeps the silence before it, as without the option)
    if held > 0 {
        w.write_all(&vec![0u8; 4 * held as usize]).or_else(|err| if err.kind() == std::io::ErrorKind::BrokenPipe { Ok(()) } else { Err(err) })?;
    }
    out.done += done;
    Ok(true)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let a = parse_args();
    if let Err(e) = run(&a) {
        eprintln!("amigaplayer-render: {e:#}");
        std::process::exit(1);
    }
}
