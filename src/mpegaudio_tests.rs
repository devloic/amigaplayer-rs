use super::*;
use crate::aiff::AiffPlayer;
use std::path::Path;

const MP3_MONO22: &[u8] = include_bytes!("mpegaudio_fixtures/sine.mp3"); // MPEG 2 layer 3, 22.05 kHz mono, 0.5 s
const MP3_STEREO44: &[u8] = include_bytes!("mpegaudio_fixtures/sine44.mp3"); // MPEG 1 layer 3, 44.1 kHz stereo, 0.4 s
const MP2_MONO24: &[u8] = include_bytes!("mpegaudio_fixtures/sine.mp2"); // MPEG 2 layer 2, 24 kHz mono, 0.5 s
const WAV_MP3: &[u8] = include_bytes!("mpegaudio_fixtures/sinemp3.wav"); // tag $55
const WAV_MP2: &[u8] = include_bytes!("mpegaudio_fixtures/sinemp2.wav"); // tag $50

/// A layer 1 stream of mono 44.1 kHz 128 kbit/s frames, subband 0 only, 4-bit samples of a square wave.
fn layer1(frames: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..frames {
        let mut bits: Vec<bool> = Vec::new();
        let mut put = |v: u32, n: u32| (0..n).rev().for_each(|i| bits.push(v >> i & 1 != 0));
        put(3, 4); // subband 0: 4 bits a sample
        for _ in 1..32 {
            put(0, 4);
        }
        put(20, 6); // scale factor
        for i in 0..12 {
            put(if i % 4 < 2 { 0xC } else { 0x3 }, 4);
        }
        let mut f = vec![0xFF, 0xFF, 0x40, 0xC0]; // MPEG 1, layer 1, no CRC, 128 kbit/s, 44.1 kHz, mono
        f.extend(bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (b as u8) << (7 - i))));
        f.resize(136, 0);
        out.extend(f);
    }
    out
}

fn samples(p: &Pcm) -> Vec<i16> {
    p.data.chunks(2).map(|c| i16::from_be_bytes([c[0], c[1]])).collect()
}

fn peak(v: &[i16]) -> i32 {
    v.iter().map(|&x| (x as i32).abs()).max().unwrap_or(0)
}

#[test]
fn layer_3_mono() {
    assert!(detect(MP3_MONO22));
    let p = decode(MP3_MONO22.to_vec()).unwrap();
    assert_eq!((p.channels, p.rate, p.format), (1, 22050.0, "MPEG audio layer 3"));
    // 0.5 s, the encoder's padding at most a few frames more
    assert!((11025..11025 + 2000).contains(&p.frames), "{}", p.frames);
    let s = samples(&p);
    assert!(peak(&s) > 3000);
    // a 440 Hz tone: 880 sign changes a second
    let mid = &s[2000..9000];
    let crossings = mid.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
    let expect = 7000.0 / 22050.0 * 880.0;
    assert!((crossings as f64 - expect).abs() < 6.0, "{crossings} vs {expect}");
}

#[test]
fn layer_3_stereo_and_mpeg_1() {
    assert!(detect(MP3_STEREO44));
    let p = decode(MP3_STEREO44.to_vec()).unwrap();
    assert_eq!((p.channels, p.rate), (2, 44100.0));
    assert!((17640..17640 + 2500).contains(&p.frames), "{}", p.frames);
    assert!(peak(&samples(&p)) > 2000); // ffmpeg upmixes mono to stereo at -3 dB
}

#[test]
fn layer_2() {
    assert!(detect(MP2_MONO24));
    let p = decode(MP2_MONO24.to_vec()).unwrap();
    assert_eq!((p.channels, p.rate, p.format), (1, 24000.0, "MPEG audio layer 2"));
    assert!((12000..12000 + 2000).contains(&p.frames), "{}", p.frames);
    assert!(peak(&samples(&p)) > 3000);
}

#[test]
fn layer_1() {
    let f = layer1(10);
    assert!(detect(&f));
    let p = decode(f).unwrap();
    assert_eq!((p.channels, p.rate, p.format, p.frames), (1, 44100.0, "MPEG audio layer 1", 10 * 384));
    assert!(peak(&samples(&p)[384..]) > 100, "layer 1 gave silence");
}

#[test]
fn in_a_wave_file() {
    for (f, layer, rate) in [(WAV_MP3, 3, 22050.0), (WAV_MP2, 2, 24000.0)] {
        assert!(detect(f));
        // the PCM reader of riffwave leaves it alone
        assert!(!crate::riffwave::detect(f));
        let p = decode(f.to_vec()).unwrap();
        assert_eq!((p.channels, p.rate), (1, rate));
        assert_eq!(p.format, format!("RIFF WAVE (MPEG audio layer {layer})"));
        assert!(peak(&samples(&p)) > 3000);
    }
}

#[test]
fn id3_tag_and_leading_junk() {
    let mut f = b"ID3\x03\0\0\0\0\0\x0A".to_vec();
    f.extend([0u8; 10]);
    f.extend(MP3_MONO22);
    assert!(detect(&f));
    assert_eq!(decode(f).unwrap().rate, 22050.0);
    let mut f = vec![0x55; 700];
    f.extend(MP3_STEREO44);
    assert!(detect(&f));
    assert_eq!(decode(f).unwrap().channels, 2);
}

#[test]
fn not_other_files() {
    // no run of frames: one frame, a header alone, noise, silence, 0xFF
    assert!(!detect(&MP3_MONO22[..40]));
    assert!(!detect(&[0xFF, 0xFB, 0x90, 0x00]));
    assert!(!detect(&[0; 5000]));
    assert!(!detect(&[0xFF; 5000]));
    assert!(!detect(b""));
    let mut x = 12345u32;
    let noise: Vec<u8> = (0..20000)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect();
    assert!(!detect(&noise));
    // a RIFF of another kind, and a WAVE of PCM
    assert!(!detect(b"RIFF\x04\0\0\0AVI "));
    let mut w = WAV_MP3.to_vec();
    w[20] = 1;
    w[21] = 0;
    assert!(!detect(&w));
}

#[test]
fn damaged() {
    // cut anywhere and a byte changed anywhere: never a panic
    for f in [MP3_MONO22, MP2_MONO24, WAV_MP3] {
        for n in (0..f.len()).step_by(7) {
            let _ = decode(f[..n].to_vec());
            for v in [0u8, 0xFF, 0x55] {
                let mut g = f.to_vec();
                g[n] = v;
                if detect(&g) {
                    let _ = decode(g);
                }
            }
        }
    }
}

#[test]
fn the_engine_plays_it_to_its_end() {
    let mut s = crate::open::Song::open(Path::new("x.mp3"), MP3_MONO22.to_vec(), 44100).unwrap();
    assert_eq!(s.format(), "MPEG audio layer 3");
    let p = decode(MP3_MONO22.to_vec()).unwrap();
    let want = p.frames * 2; // 22.05 kHz to 44.1 kHz
    let mut out = vec![0f32; 2 * (want + 200)];
    s.render(&mut out[..2 * (want - 200)]);
    assert!(!s.songend());
    assert!(out[2 * 4000..2 * (want - 200)].iter().any(|v| v.abs() > 0.05));
    s.render(&mut out[2 * (want - 200)..]);
    assert!(s.songend());
    let p_secs = p.frames as f64 / 22050.0;
    let x = AiffPlayer::from_pcm(p, 44100).unwrap();
    assert!((x.duration_secs() - p_secs).abs() < 0.01);
}

#[test]
fn digital_sound_creations_modules_are_not_mpeg() {
    // their data holds three frame headers in a row by chance (refcmp rows 105-109)
    let dir = std::path::Path::new("/media/lolo/kingston2tb/claude-scratch/deliplayerrust/ports/batch11/mods/dsc");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut n = 0;
    for e in rd.flatten() {
        let d = std::fs::read(e.path()).unwrap();
        assert!(!detect(&d), "{} taken for MPEG audio", e.path().display());
        n += 1;
    }
    assert!(n > 0);
}
