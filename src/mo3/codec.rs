//! The MP3 and Ogg Vorbis samples of MO3 files, decoded the way libopenmpt 0.8.7's
//! `Load_mo3.cpp` hands them to libmpg123 and libvorbisfile (the decoders of openmpt123), here
//! through Symphonia's pure-Rust decoders (`symphonia-bundle-mp3`, `symphonia-codec-vorbis`;
//! MPL-2.0).
//!
//! MP3 (`ReadMP3Sample` with `raw`, `mo3Decode`): a LAME / Xing info frame at the start is
//! dropped when the sample's encoder delay covers it (and its samples come off the delay); the
//! stream is decoded with no gapless trimming; then the encoder delay (in bytes of the decoded
//! 16-bit frames) comes off the start and the length is capped at the header's. The result is
//! 16-bit, mono or stereo as the stream is. A frame Symphonia cannot decode gives a frame of
//! silence, so that the frames after it stay where libmpg123 puts them; an info frame left in
//! the stream (Symphonia skips it, libmpg123 told to ignore info frames decodes it) too.
//!
//! Ogg Vorbis: the sample's pages, after the header pages of another sample when it shares
//! them, are read as one logical stream; the identification and setup headers start the
//! decoder; the audio packets are decoded in order, the first one giving no output (as
//! libvorbis, which overlaps it with nothing), into the sample's own bit depth and channel count
//! (float to integer as libopenmpt converts: clamped to -1..1, times 128 or 32768, rounded half
//! up) until the sample's length is full. A stream of another channel count writes nothing.
//!
//! Ported from libopenmpt 0.8.7 (BSD-3-Clause, OpenMPT Project Developers and Contributors; see
//! LICENSE-openmpt): `Load_mo3.cpp`, `MPEGFrame.cpp`, `SampleFormatMP3.cpp`.

use std::io::Cursor;
use symphonia_bundle_mp3::{MpaDecoder, MpaReader};
use symphonia_codec_vorbis::VorbisDecoder;
use symphonia_core::codecs::CodecParameters;
use symphonia_core::codecs::audio::well_known::CODEC_ID_VORBIS;
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia_core::formats::{FormatOptions, FormatReader};
use symphonia_core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia_core::packet::PacketRef;
use symphonia_core::units::{Duration, Timestamp};

/// What `MPEGFrame` reads from the frame at the start of `d`: frame size, samples (times two
/// for a stereo frame) and whether it is a Xing / Info frame.
fn mpeg_frame(d: &[u8]) -> Option<(usize, usize, bool)> {
    const SAMPLES: [[usize; 3]; 2] = [[384, 1152, 1152], [384, 1152, 576]];
    const RATES: [[u32; 15]; 6] = [
        [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
        [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    ];
    const FREQ: [[u32; 3]; 4] = [[11025, 12000, 8000], [0, 0, 0], [22050, 24000, 16000], [44100, 48000, 32000]];
    const COEF: [[u32; 3]; 2] = [[12, 144, 144], [12, 144, 72]];
    const SIDE: [[usize; 2]; 2] = [[17, 32], [9, 17]];
    let h = d.get(..4)?;
    if !(h[0] == 0xFF && h[1] & 0xE0 == 0xE0 && h[1] & 0x18 != 0x08 && h[1] & 0x06 != 0 && h[2] & 0x0C != 0x0C && h[2] & 0xF0 != 0xF0) {
        return None;
    }
    let version = ((h[1] & 0x18) >> 3) as usize;
    let mpeg1 = if version == 3 { 0 } else { 1 };
    let layer = 3 - ((h[1] & 0x06) >> 1) as usize;
    let rate = ((h[2] & 0xF0) >> 4) as usize;
    let freq = ((h[2] & 0x0C) >> 2) as usize;
    let padding = ((h[2] & 0x02) >> 1) as u32;
    let stereo = (h[3] & 0xC0) >> 6 != 3;
    let br = RATES[mpeg1 * 3 + layer][rate] * 1000;
    let size = (COEF[mpeg1][layer] * br / FREQ[version][freq] + padding) * if layer == 0 { 4 } else { 1 };
    let size = (size & 0xFFFF) as usize;
    let mut samples = SAMPLES[mpeg1][layer];
    if stereo {
        samples *= 2;
    }
    let lame = SIDE[mpeg1][stereo as usize];
    if size < lame + 8 {
        return Some((size, samples, false));
    }
    let mut frame = [0u8; 36];
    let avail = d.len().saturating_sub(4).min(lame + 4);
    frame[..avail].copy_from_slice(&d[4..4 + avail]);
    if frame[2..lame].iter().any(|&b| b != 0) {
        return Some((size, samples, false));
    }
    let tag = &frame[lame..lame + 4];
    Some((size, samples, tag == b"Info" || tag == b"Xing"))
}

/// An MP3 sample: 16-bit frames (interleaved) and the channel count, or None when nothing
/// decodes.
pub fn mp3(chunk: &[u8], encoder_delay: u16, header_len: u32) -> Option<(Vec<i16>, usize)> {
    let mut data = chunk;
    let mut delay = encoder_delay as usize;
    if let Some((size, samples, true)) = mpeg_frame(chunk) {
        let frame_delay = samples * 2;
        if delay >= frame_delay {
            delay -= frame_delay;
            data = chunk.get(size..).unwrap_or(&[]);
        }
    }
    // an info frame left in the stream: libmpg123 decodes it (to silence), Symphonia skips it
    let lead = match mpeg_frame(data) {
        Some((_, samples, true)) => samples,
        _ => 0,
    };
    let mss = MediaSourceStream::new(Box::new(Cursor::new(data.to_vec())), MediaSourceStreamOptions::default());
    let mut reader = MpaReader::try_new(mss, FormatOptions::default()).ok()?;
    let params = match reader.tracks().first().and_then(|t| t.codec_params.clone()) {
        Some(CodecParameters::Audio(p)) => p,
        _ => return None,
    };
    let opts = AudioDecoderOptions::default().gapless(false);
    let mut dec = MpaDecoder::try_new(&params, &opts).ok()?;
    let mut channels = 0usize;
    let mut out: Vec<i16> = Vec::new();
    let mut tmp: Vec<i16> = Vec::new();
    let mut lead_done = false;
    while let Ok(Some(packet)) = reader.next_packet() {
        match dec.decode(&packet) {
            Ok(audio) => {
                let ch = audio.spec().channels().count();
                if channels == 0 {
                    channels = ch;
                }
                if !lead_done {
                    // (`lead` counts the info frame's samples of all channels)
                    out.resize(lead / ch.max(1) * channels, 0);
                    lead_done = true;
                }
                if ch != channels || audio.frames() == 0 {
                    continue;
                }
                tmp.clear();
                audio.copy_to_vec_interleaved(&mut tmp);
                out.extend_from_slice(&tmp);
            }
            Err(_) if channels > 0 => {
                let n = packet.dur.get() as usize;
                out.resize(out.len() + n * channels, 0);
            }
            Err(_) => {}
        }
    }
    if channels == 0 || out.is_empty() {
        return None;
    }
    // the encoder delay, in bytes of the decoded sample
    let bytes = out.len() * 2;
    let frame_bytes = 2 * channels;
    let mut frames = out.len() / channels;
    if delay > 0 && delay < bytes {
        let mut b: Vec<u8> = out.iter().flat_map(|v| v.to_le_bytes()).collect();
        b.drain(..delay);
        b.resize(bytes, 0);
        out = b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        frames -= delay / frame_bytes;
    }
    frames = frames.min(header_len as usize);
    out.truncate(frames * channels);
    Some((out, channels))
}

/// The packets of the Ogg pages in `d` (pages found as libopenmpt's `ReadPageAndSkipJunk`
/// finds them: at the next "OggS"), in order, and whether they come from more than one
/// logical stream.
fn ogg_packets(d: &[u8], out: &mut Vec<Vec<u8>>, serials: &mut Vec<u32>) {
    let mut pos = 0;
    let mut cur: Option<Vec<u8>> = None;
    while pos + 27 <= d.len() {
        if &d[pos..pos + 4] != b"OggS" {
            pos += 1;
            continue;
        }
        let h = &d[pos..];
        let nseg = h[26] as usize;
        if h.len() < 27 + nseg {
            break;
        }
        let serial = u32::from_le_bytes([h[14], h[15], h[16], h[17]]);
        if !serials.contains(&serial) {
            serials.push(serial);
        }
        let table = &h[27..27 + nseg];
        let body: usize = table.iter().map(|&b| b as usize).sum();
        if h.len() < 27 + nseg + body {
            break;
        }
        let continued = h[5] & 1 != 0;
        if !continued {
            if let Some(p) = cur.take() {
                out.push(p);
            }
        }
        let mut at = 27 + nseg;
        for &s in table {
            let p = cur.get_or_insert_with(Vec::new);
            p.extend_from_slice(&h[at..at + s as usize]);
            at += s as usize;
            if s < 255 {
                out.push(cur.take().unwrap_or_default());
            }
        }
        pos += 27 + nseg + body;
    }
    if let Some(p) = cur.take() {
        out.push(p);
    }
}

/// libopenmpt's float to integer sample conversion (`SC::Convert<int16 / int8, float>`).
#[inline]
fn to_int(v: f32, scale: f32) -> i32 {
    let v = if v.is_nan() { 0.0 } else { v.clamp(-1.0, 1.0) } * scale;
    (v + 0.5).floor() as i32
}

/// An Ogg Vorbis sample of `frames` frames of `channels` channels, 16-bit or 8-bit (the
/// sample's own format): `header` holds the stream's header pages (another sample's, or the
/// sample's own data). None when the stream does not open.
pub fn vorbis(header: &[u8], data: &[u8], shared: bool, frames: usize, channels: usize, bits16: bool) -> Option<Vec<i32>> {
    let mut packets = Vec::new();
    let mut serials = Vec::new();
    if shared {
        ogg_packets(header, &mut packets, &mut serials);
        serials.clear();
    }
    ogg_packets(data, &mut packets, &mut serials);
    // (libvorbisfile refuses a chained stream: more than one logical stream in the data)
    if serials.len() > 1 || packets.len() < 3 {
        return None;
    }
    let is_header = |p: &[u8], t: u8| p.len() >= 7 && p[0] == t && &p[1..7] == b"vorbis";
    if !is_header(&packets[0], 1) || !is_header(&packets[1], 3) || !is_header(&packets[2], 5) {
        return None;
    }
    let mut extra = packets[0].clone();
    extra.extend_from_slice(&packets[2]);
    let mut params = AudioCodecParameters::new();
    params.for_codec(CODEC_ID_VORBIS).with_extra_data(extra.into_boxed_slice());
    // gapless: the first packet's output is dropped (libvorbis gives none for it); the packets
    // carry no trimming
    let opts = AudioDecoderOptions::default().gapless(true);
    let mut dec = VorbisDecoder::try_new(&params, &opts).ok()?;
    // (a packet decodes to at most 4096 frames: a longer length than the stream can fill only
    // adds silence, which is left out)
    let frames = frames.min((packets.len() - 3).saturating_mul(4096));
    let mut out = vec![0i32; frames * channels];
    let scale = if bits16 { 32768.0 } else { 128.0 };
    let mut offset = 0usize;
    let mut tmp: Vec<f32> = Vec::new();
    for p in &packets[3..] {
        if offset >= frames {
            break;
        }
        let pr = PacketRef::new(0, Timestamp::new(0), Duration::new(0), p);
        let Ok(audio) = dec.decode_ref(&pr) else { continue };
        let ch = audio.spec().channels().count();
        let n = audio.frames().min(frames - offset);
        if n > 0 && ch == channels {
            tmp.clear();
            audio.copy_to_vec_interleaved(&mut tmp);
            for (k, &v) in tmp[..n * ch].iter().enumerate() {
                out[offset * ch + k] = to_int(v, scale).clamp(-(scale as i32), scale as i32 - 1);
            }
        }
        offset += n;
    }
    Some(out)
}
