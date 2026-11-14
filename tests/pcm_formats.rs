//! `Song` takes the recorded-audio formats the engine plays (AIFF, IFF 8SVX, RIFF WAVE, MPEG audio,
//! ZX Spectrum AY) as `amigaplayer-render` does: same recognition, same samples.

use amigaplayer_rs::{detect, detect_with, formats, Error, Song};

const RATE: u32 = 44100;

const MP3_MONO22: &[u8] = include_bytes!("../src/mpegaudio_fixtures/sine.mp3");
const MP3_STEREO44: &[u8] = include_bytes!("../src/mpegaudio_fixtures/sine44.mp3");
const MP2_MONO24: &[u8] = include_bytes!("../src/mpegaudio_fixtures/sine.mp2");
const WAV_MP3: &[u8] = include_bytes!("../src/mpegaudio_fixtures/sinemp3.wav");
const WAV_MP2: &[u8] = include_bytes!("../src/mpegaudio_fixtures/sinemp2.wav");

fn chunk(id: &[u8; 4], body: &[u8], be: bool) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend(if be { (body.len() as u32).to_be_bytes() } else { (body.len() as u32).to_le_bytes() });
    v.extend(body);
    if body.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn wav_pcm16() -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend(1u16.to_le_bytes());
    fmt.extend(2u16.to_le_bytes());
    fmt.extend(22050u32.to_le_bytes());
    fmt.extend((22050u32 * 4).to_le_bytes());
    fmt.extend(4u16.to_le_bytes());
    fmt.extend(16u16.to_le_bytes());
    let data: Vec<u8> = (0..400i32).flat_map(|i| (((i * 97) % 20000 - 10000) as i16).to_le_bytes()).collect();
    let mut body = b"WAVE".to_vec();
    body.extend(chunk(b"fmt ", &fmt, false));
    body.extend(chunk(b"data", &data, false));
    let mut f = b"RIFF".to_vec();
    f.extend((body.len() as u32).to_le_bytes());
    f.extend(body);
    f
}

fn svx(fib: bool) -> Vec<u8> {
    let mut vhdr = Vec::new();
    vhdr.extend(300u32.to_be_bytes());
    vhdr.extend(0u32.to_be_bytes());
    vhdr.extend(0u32.to_be_bytes());
    vhdr.extend(8000u16.to_be_bytes());
    vhdr.push(1);
    vhdr.push(fib as u8);
    vhdr.extend(0x10000u32.to_be_bytes());
    let body: Vec<u8> = if fib { (0..302u32).map(|i| (i * 37) as u8).collect() } else { (0..300u32).map(|i| (i * 13) as u8).collect() };
    let mut b = b"8SVX".to_vec();
    b.extend(chunk(b"VHDR", &vhdr, true));
    b.extend(chunk(b"BODY", &body, true));
    let mut f = b"FORM".to_vec();
    f.extend((b.len() as u32).to_be_bytes());
    f.extend(b);
    f
}

fn aiff() -> Vec<u8> {
    let mut comm = Vec::new();
    comm.extend(1u16.to_be_bytes());
    comm.extend(200u32.to_be_bytes());
    comm.extend(16u16.to_be_bytes());
    comm.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]); // 44100 Hz
    let ssnd: Vec<u8> = [0u8; 8].into_iter().chain((0..200i32).flat_map(|i| (((i * 211) % 16000 - 8000) as i16).to_be_bytes())).collect();
    let mut b = b"AIFF".to_vec();
    b.extend(chunk(b"COMM", &comm, true));
    b.extend(chunk(b"SSND", &ssnd, true));
    let mut f = b"FORM".to_vec();
    f.extend((b.len() as u32).to_be_bytes());
    f.extend(b);
    f
}

fn cases() -> Vec<(&'static str, Vec<u8>, &'static str)> {
    vec![
        ("a.wav", wav_pcm16(), "RIFF WAVE"),
        ("a.8svx", svx(false), "IFF 8SVX"),
        ("b.8svx", svx(true), "IFF 8SVX (Fibonacci)"),
        ("a.aiff", aiff(), "AIFF"),
        ("a.mp3", MP3_MONO22.to_vec(), "MPEG audio layer 3"),
        ("b.mp3", MP3_STEREO44.to_vec(), "MPEG audio layer 3"),
        ("a.mp2", MP2_MONO24.to_vec(), "MPEG audio layer 2"),
        ("w3.wav", WAV_MP3.to_vec(), "RIFF WAVE (MPEG audio layer 3)"),
        ("w2.wav", WAV_MP2.to_vec(), "RIFF WAVE (MPEG audio layer 2)"),
    ]
}

fn render(s: &mut Song, frames: usize) -> Vec<i16> {
    let mut out = vec![0i16; frames * 2];
    for c in out.chunks_mut(2000) {
        s.render_i16(c);
    }
    out
}

#[test]
fn recognised_and_played_under_the_engines_name() {
    let f = formats();
    for want in ["IFF 8SVX", "RIFF WAVE", "MPEG audio layer 3", "ZXAYEMUL"] {
        assert!(f.contains(&want), "{want} missing from formats()");
    }
    for (name, data, format) in cases() {
        assert_eq!(detect(&data).as_deref(), Some(format), "detect {name}");
        assert_eq!(detect_with(name, &data, &[]).as_deref(), Some(format), "detect_with {name}");
        let mut s = Song::open(name, &data, RATE).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(s.format(), format, "{name}");
        assert!(render(&mut s, 2000).iter().any(|&x| x != 0), "{name} is silent");
    }
}

#[test]
fn damaged_audio_files_are_not_claimed() {
    // an MPEG stream cut to less than a frame, and noise that is none of them
    assert_eq!(detect(&MP3_MONO22[..40]), None);
    assert_eq!(detect(&[0x55u8; 3000]), None);
    assert!(matches!(Song::open("x.mp3", &MP3_MONO22[..40], RATE), Err(Error::NotRecognised)));
    // an 8SVX of an unknown compression is not claimed either
    let mut s = svx(false);
    s[8 + 4 + 8 + 15] = 7; // sCompression
    assert_eq!(detect(&s), None);
}

#[cfg(feature = "custom-modules")]
mod against_the_render_tool {
    use super::*;
    use std::process::Command;

    fn tool(path: &std::path::Path, args: &[&str]) -> std::process::Output {
        let out = Command::new(env!("CARGO_BIN_EXE_amigaplayer-render")).env("SDL_AUDIO_DRIVER", "dummy").args(args).arg(path).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        out
    }

    #[test]
    fn same_samples_as_amigaplayer_render() {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pcm_formats");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, data, format) in cases() {
            let p = dir.join(name);
            std::fs::write(&p, &data).unwrap();
            let info = String::from_utf8_lossy(&tool(&p, &["--info"]).stdout).to_string();
            assert!(info.contains(&format!("\"format\":\"{format}\"")), "{name}: {info}");
            let raw = tool(&p, &["--format", "raw", "--seconds", "0", "--silence", "0", "--rate", "44100"]).stdout;
            let want: Vec<i16> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            assert!(want.len() >= 2 * 1000, "{name}: {} samples", want.len());
            let mut s = Song::open(name, &data, RATE).unwrap();
            let got = render(&mut s, want.len() / 2);
            assert!(got == want, "{name}: samples differ");
        }
    }
}

/// One file of each PCM group of the refcmp baseline (when the corpus is on this machine).
#[test]
fn refcmp_pcm_groups_are_recognised() {
    let tsv = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tools/refcmp-baseline.tsv")).unwrap_or_default();
    let mut lines = tsv.lines();
    let Some(head) = lines.next() else { return };
    let cols: Vec<&str> = head.split('\t').collect();
    let (fi, pi) = (cols.iter().position(|c| *c == "fmt").unwrap(), cols.iter().position(|c| *c == "path").unwrap());
    let mut seen = std::collections::BTreeMap::new();
    for l in lines {
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() > pi.max(fi) {
            let g = c[fi];
            if ["AIFF", "IFF 8SVX", "RIFF WAVE", "MPEG audio", "ZX Spectrum AY"].iter().any(|k| g.contains(k)) {
                seen.entry(g.to_string()).or_insert_with(|| c[pi].to_string());
            }
        }
    }
    for (g, p) in seen {
        let src = if std::path::Path::new(&p).exists() { p.clone() } else { format!("{p}.xz") };
        let Ok(mut data) = std::fs::read(&src) else { continue };
        if data.starts_with(b"\xfd7zXZ\0") {
            let Ok(o) = std::process::Command::new("xz").arg("-dc").arg(&src).output() else { continue };
            data = o.stdout;
        }
        let name = std::path::Path::new(&p).file_name().unwrap().to_string_lossy().trim_end_matches(".xz").to_string();
        assert!(detect(&data).is_some(), "{g}: detect");
        let mut s = Song::open(&name, &data, RATE).unwrap_or_else(|e| panic!("{g} {p}: {e}"));
        let mut buf = vec![0f32; 4000];
        s.render_f32(&mut buf);
    }
}
