//! MPEG audio, layers 1, 2 and 3 (`.mp1`, `.mp2`, `.mp3`, and RIFF WAVE files whose format
//! tag is $50 or $55), decoded for the recorded-audio player (`crate::aiff::AiffPlayer::from_pcm`).
//!
//! The decoding is Symphonia's (`symphonia-bundle-mp3`, MPL-2.0, pure Rust); this module
//! recognises the files, hands it the bytes and turns what it gives into 16-bit samples.
//!
//! Recognition is by content: after an optional ID3v2 tag, a frame header (11 sync bits, a
//! valid version, layer, bit rate and sample rate) must be followed by two more of the same
//! layer, each where the frame length of the one before puts it. A module of any Amiga format
//! has no such run of frames. A stream that starts with some junk is found when a run begins
//! within the first 4 KiB.
//!
//! The whole file is decoded at once (as the ADPCM of `riffwave`): 2 bytes a sample. The length
//! of the song is what the frames hold. A frame that does not decode is skipped; the sample
//! rate and channel count of the first frame hold for the rest (frames of another kind are
//! dropped). No encoder delay is removed.

use crate::aiff::{Enc, Pcm};
use std::io::Cursor;
use symphonia_bundle_mp3::{MpaDecoder, MpaReader};
use symphonia_core::codecs::CodecParameters;
use symphonia_core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia_core::formats::{FormatOptions, FormatReader};
use symphonia_core::io::{MediaSourceStream, MediaSourceStreamOptions};

/// How far into the data the first frame of a stream may lie.
const SEARCH: usize = 4096;
/// Frames in a row that make a stream.
const RUN: usize = 3;

/// kbit/s by bit rate index 1..=14: MPEG 1 layers 1, 2, 3; MPEG 2 / 2.5 layer 1, layers 2 and 3.
const KBPS: [[u32; 14]; 5] = [
    [32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
    [32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
    [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    [32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
    [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
];

/// The header at `o`: (layer, frame length in bytes), or `None`.
fn frame(d: &[u8], o: usize) -> Option<(u8, usize)> {
    let h = d.get(o..o + 4)?;
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 0: 2.5, 1: reserved, 2: 2, 3: 1
    let layer = match (h[1] >> 1) & 3 {
        1 => 3u8,
        2 => 2,
        3 => 1,
        _ => return None,
    };
    let (br, sr) = (h[2] >> 4, (h[2] >> 2) & 3);
    if version == 1 || br == 0 || br == 15 || sr == 3 {
        return None;
    }
    let pad = ((h[2] >> 1) & 1) as usize;
    let row = if version == 3 { layer as usize - 1 } else if layer == 1 { 3 } else { 4 };
    let bps = KBPS[row][br as usize - 1] as usize * 1000;
    let rate = [44100usize, 48000, 32000][sr as usize] >> [2, 0, 1, 0][version as usize];
    let len = match layer {
        1 => (12 * bps / rate + pad) * 4,
        3 if version != 3 => 72 * bps / rate + pad,
        _ => 144 * bps / rate + pad,
    };
    (len >= 4).then_some((layer, len))
}

/// The length of an ID3v2 tag at the start of `d` (0 when there is none).
fn skip_id3(d: &[u8]) -> usize {
    if d.len() >= 10 && &d[..3] == b"ID3" && d[3] != 0xFF && d[6..10].iter().all(|b| b & 0x80 == 0) {
        let n = d[6..10].iter().fold(0usize, |a, &b| a << 7 | b as usize);
        let footer = if d[5] & 0x10 != 0 { 10 } else { 0 };
        return 10 + n + footer;
    }
    0
}

/// Where the stream of MPEG audio frames begins in `d`, and its layer.
fn raw_stream(d: &[u8]) -> Option<(usize, u8)> {
    let start = skip_id3(d);
    let end = d.len().min(start.saturating_add(SEARCH));
    for o in start..end {
        let Some((layer, mut len)) = frame(d, o) else { continue };
        let mut at = o;
        let mut n = 1;
        while n < RUN {
            at += len;
            match frame(d, at) {
                Some((l, nl)) if l == layer => (len, n) = (nl, n + 1),
                _ => break,
            }
        }
        if n == RUN {
            return Some((o, layer));
        }
    }
    None
}

/// The `data` chunk of a RIFF WAVE file whose format tag is MPEG audio ($50 layer 2, $55 layer 3).
fn wave(d: &[u8]) -> Option<(usize, usize)> {
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return None;
    }
    let (mut tag, mut body) = (None, None);
    let mut o = 12;
    while o + 8 <= d.len() {
        let size = u32::from_le_bytes([d[o + 4], d[o + 5], d[o + 6], d[o + 7]]) as usize;
        let at = o + 8;
        let stop = at.saturating_add(size).min(d.len());
        match &d[o..o + 4] {
            b"fmt " if tag.is_none() && stop - at >= 2 => {
                let t = u16::from_le_bytes([d[at], d[at + 1]]);
                // an extensible header carries the tag in its GUID
                tag = Some(if t == 0xFFFE && stop - at >= 26 { u16::from_le_bytes([d[at + 24], d[at + 25]]) } else { t });
            }
            b"data" if body.is_none() => body = Some((at, stop)),
            id if !id.iter().all(|b| (32..=127).contains(b)) => break,
            _ => {}
        }
        o = at.checked_add(size)?.checked_add(size & 1)?;
    }
    if matches!(tag, Some(0x50 | 0x55)) { body } else { None }
}

/// The bytes of the MPEG stream in `d`, its layer, and whether it lies in a WAVE file.
fn stream(d: &[u8]) -> Option<(&[u8], u8, bool)> {
    // a RIFF file is a stream only through its `data` chunk, and only for a MPEG format tag
    if d.starts_with(b"RIFF") {
        let (a, b) = wave(d)?;
        let s = &d[a..b];
        return raw_stream(s).map(|(o, l)| (&s[o..], l, true));
    }
    raw_stream(d).map(|(o, l)| (&d[o..], l, false))
}

/// Whether `data` is an MPEG audio stream (layer 1, 2 or 3), bare or in a RIFF WAVE file.
pub fn detect(data: &[u8]) -> bool {
    stream(data).is_some()
}

#[cfg(test)]
#[path = "mpegaudio_tests.rs"]
mod tests;

/// Decodes the whole stream to 16-bit big-endian samples.
pub fn decode(data: Vec<u8>) -> Result<Pcm, String> {
    let (s, layer, in_wave) = stream(&data).ok_or("MPEG audio: no frames")?;
    let format = match (layer, in_wave) {
        (1, false) => "MPEG audio layer 1",
        (2, false) => "MPEG audio layer 2",
        (_, false) => "MPEG audio layer 3",
        (1, true) => "RIFF WAVE (MPEG audio layer 1)",
        (2, true) => "RIFF WAVE (MPEG audio layer 2)",
        _ => "RIFF WAVE (MPEG audio layer 3)",
    };
    let mss = MediaSourceStream::new(Box::new(Cursor::new(s.to_vec())), MediaSourceStreamOptions::default());
    let mut reader = MpaReader::try_new(mss, FormatOptions::default()).map_err(|e| format!("MPEG audio: {e}"))?;
    let params = match reader.tracks().first().and_then(|t| t.codec_params.clone()) {
        Some(CodecParameters::Audio(p)) => p,
        _ => return Err("MPEG audio: no audio track".into()),
    };
    let mut dec = MpaDecoder::try_new(&params, &AudioDecoderOptions::default()).map_err(|e| format!("MPEG audio: {e}"))?;
    let mut out: Vec<u8> = Vec::new();
    let (mut rate, mut channels) = (0u32, 0usize);
    let mut tmp: Vec<i16> = Vec::new();
    while let Ok(Some(packet)) = reader.next_packet() {
        let Ok(audio) = dec.decode(&packet) else { continue };
        let ch = audio.spec().channels().count();
        let r = audio.spec().rate();
        if channels == 0 {
            (rate, channels) = (r, ch);
        } else if (r, ch) != (rate, channels) {
            continue;
        }
        if audio.frames() == 0 || ch == 0 {
            continue;
        }
        tmp.clear();
        audio.copy_to_vec_interleaved(&mut tmp);
        for v in &tmp {
            out.extend(v.to_be_bytes());
        }
    }
    if channels == 0 || out.is_empty() {
        return Err("MPEG audio: no frame decodes".into());
    }
    Ok(Pcm { frames: out.len() / (2 * channels), data: out, start: 0, channels, bits: 16, enc: Enc::Be, rate: rate as f64, format, title: String::new(), loop_start: None })
}
