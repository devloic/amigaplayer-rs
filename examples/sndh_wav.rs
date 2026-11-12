//! Renders an SNDH tune on the built-in Atari ST machine to a 16-bit stereo WAV file, for a check
//! against another player's output (AtariAudio's own renderer writes 48 kHz mono).
//!
//!   cargo run --release --no-default-features --features archives --example sndh_wav -- FILE OUT.wav [SUBSONG [SECONDS [RATE]]]
//!
//! FILE may be ICE!-packed; SUBSONG is 1-based (default: the file's own); SECONDS (default 10);
//! RATE the output rate (default 48000). The samples are the machine's own 16-bit values, not
//! normalised.

use amigaplayer_rs::aiff::Stream;
use amigaplayer_rs::replay::sndh::SndhPlayer;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(file), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: sndh_wav FILE OUT.wav [SUBSONG [SECONDS [RATE]]]");
        std::process::exit(2);
    };
    let data = std::fs::read(file).unwrap_or_else(|e| {
        eprintln!("{file}: {e}");
        std::process::exit(1);
    });
    let data = amigaplayer_rs::decrunch::decrunch(data);
    let rate: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(48000);
    let mut p = match SndhPlayer::new(&data, rate) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{file}: {e}");
            std::process::exit(1);
        }
    };
    if let Some(s) = args.get(2).and_then(|s| s.parse::<u16>().ok()) {
        p.start(s.saturating_sub(1));
    }
    let secs: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let frames = (secs * rate as f64) as usize;
    let mut buf = vec![0f32; 2 * frames];
    p.render(&mut buf);
    let mut wav = Vec::with_capacity(44 + 4 * frames);
    let bytes = (4 * frames) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for x in &buf {
        wav.extend_from_slice(&((x * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(out, wav).unwrap_or_else(|e| {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    });
}
