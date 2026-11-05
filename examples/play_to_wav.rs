//! Renders a module file to a 16-bit stereo WAV file. No sound card, no SDL.
//!
//! ```text
//! cargo run --release --no-default-features --example play_to_wav -- tune.mod tune.wav [SECONDS [SUBSONG]]
//! ```
//!
//! `--demo` instead of MODULE renders the synthetic module of `amigaplayer_rs::testdata`.
//!
//! The render stops at the replayer's song end, after `SECONDS` (default 120), or after 10
//! seconds of silence, whichever comes first. Files beside the module (and in an
//! `Instruments/` directory) are handed over as companions, for the formats that read
//! sample files (TFMX `smpl.*`, IFF-SMUS instruments, ...).

use amigaplayer_rs::Song;
use std::io::Write;
use std::path::Path;

const RATE: u32 = 44100;

fn companions(module: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    let Some(dir) = module.parent().map(|p| if p.as_os_str().is_empty() { Path::new(".") } else { p }) else { return v };
    for (sub, prefix) in [("", ""), ("Instruments", "Instruments/")] {
        let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
        for e in rd.flatten().take(300) {
            let p = e.path();
            if p == module || !p.is_file() || e.metadata().map_or(true, |m| m.len() > 4 << 20) {
                continue;
            }
            if let (Some(n), Ok(d)) = (p.file_name().and_then(|n| n.to_str()), std::fs::read(&p)) {
                v.push((format!("{prefix}{n}"), d));
            }
        }
    }
    v
}

fn write_wav(out: &mut impl Write, pcm: &[i16]) -> std::io::Result<()> {
    let bytes = (pcm.len() * 2) as u32;
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + bytes).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?; // PCM
    out.write_all(&2u16.to_le_bytes())?; // channels
    out.write_all(&RATE.to_le_bytes())?;
    out.write_all(&(RATE * 4).to_le_bytes())?;
    out.write_all(&4u16.to_le_bytes())?;
    out.write_all(&16u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&bytes.to_le_bytes())?;
    for s in pcm {
        out.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: play_to_wav MODULE OUT.wav [SECONDS [SUBSONG]]");
        std::process::exit(2);
    }
    let path = Path::new(&args[0]);
    let seconds: f64 = args.get(2).map_or(Ok(120.0), |s| s.parse())?;
    let demo = args[0] == "--demo";
    let data = if demo { amigaplayer_rs::testdata::minimal_mod() } else { std::fs::read(path)? };
    let files = if demo { Vec::new() } else { companions(path) };
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("module");

    let mut song = Song::open_with(name, &data, &refs, RATE)?;
    if let Some(n) = args.get(3) {
        song.set_subsong(n.parse()?)?;
    }
    let s = song.subsongs();
    eprintln!("{} [{}] subsong {} of {}..={}", song.title(), song.format(), s.current, s.first, s.last);

    let mut pcm: Vec<i16> = Vec::new();
    let mut buf = vec![0i16; 2 * 2048];
    let mut quiet = 0u32;
    let recording = song.format().starts_with("AIFF");
    while !song.songend() && song.elapsed_secs() < seconds {
        song.render_i16(&mut buf);
        quiet = if buf.iter().all(|&x| x == 0) { quiet + 2048 } else { 0 };
        pcm.extend_from_slice(&buf);
        if !recording && quiet >= 10 * RATE {
            break;
        }
    }
    write_wav(&mut std::io::BufWriter::new(std::fs::File::create(&args[1])?), &pcm)?;
    eprintln!("{:.1} s written to {}", pcm.len() as f64 / 2.0 / RATE as f64, args[1]);
    Ok(())
}
