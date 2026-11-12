//! RIFF WAVE sample files (PCM, IEEE float, Microsoft ADPCM), decoded for the
//! recorded-audio player (`crate::aiff::AiffPlayer::from_pcm`, the engine's PCM backend).
//!
//! A port of NostalgicPlayer's RIFF-WAVE sample converter
//! (https://github.com/neumatho/NostalgicPlayer, MIT, Thomas Neumann:
//! `LICENSE-NostalgicPlayer`): `Source/Agents/SampleConverters/RiffWave/`
//! `RiffWaveLoaderWorkerBase.cs`, `Formats/PcmFormat.cs`, `Formats/Ieee_FloatFormat.cs`,
//! `Formats/AdpcmFormat.cs` (the loaders, not the savers). The 16 step factors of the
//! ADPCM decoder are Microsoft ADPCM's own table; the predictor coefficients come from
//! each file.
//!
//! As that source, three formats (a `fmt ` tag, or the tag in the GUID of a
//! `WAVE_FORMAT_EXTENSIBLE` header whose extension is 22 bytes):
//! - 1, PCM: 1 to 32 bits, little-endian, unsigned up to 8 bits; any number of channels.
//! - 3, IEEE float, 32 or 64 bits. A sample beyond -1..1 comes out as 0.9 (or -0.9), and
//!   the factor that brought it there is applied to every sample after it, until the next
//!   such sample sets its own.
//! - 2, Microsoft ADPCM, 4 bits, one or two channels: blocks of `block align` bytes, each
//!   starting with a predictor number, a step and two samples for every channel; the step
//!   is kept in 16 bits (it wraps), the prediction is shifted (rounds down), the sample is
//!   clipped to 16 bits.
//!
//! A-law, µ-law, IMA ADPCM, MPEG and the rest are not in that source, and not here:
//! `detect` leaves such files to whatever played them before.
//!
//! Not as that source: only the bytes of the `data` chunk are played (the source reads
//! on to the end of the file); samples of a width that is not a multiple of 8 bits are
//! taken as left-justified in their bytes (the source shifts them out); an ADPCM block
//! holds no more samples than its bytes have (the source trusts the header), a last
//! block that is cut short gives the samples it has (the source decodes what the block
//! before left in its buffer), a predictor number equal to the number of coefficient
//! sets ends the sample (the source reads past its table), and a float that is not a
//! number is silence.
//!
//! The file is not trusted: every size is cut down to the bytes that are there; PCM is
//! played from the file itself, floats take as much memory again, ADPCM four times.

use crate::aiff::{Enc, Pcm};

const WAVE_PCM: u16 = 1;
const WAVE_ADPCM: u16 = 2;
const WAVE_IEEE_FLOAT: u16 = 3;
const WAVE_EXTENSIBLE: u16 = 0xFFFE;
/// the last 12 bytes of the GUID of a format tag in an extensible header
const GUID_TAIL: [u8; 12] = [0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71];

const STEP_ADJUST: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];

fn le16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// The bytes of the first `fmt ` and `data` chunks of a RIFF WAVE file, or `None` for
/// any other file.
#[allow(clippy::type_complexity)]
fn chunks(data: &[u8]) -> Option<(Option<(usize, usize)>, Option<(usize, usize)>)> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }
    let end = data.len();
    let (mut fmt, mut body) = (None, None);
    let mut o = 12;
    // every turn moves on by 8 bytes at least
    while o + 8 <= end {
        let id = &data[o..o + 4];
        let size = le32(data, o + 4) as usize;
        let at = o + 8;
        let stop = at.saturating_add(size).min(end); // a chunk cannot outrun the file
        match id {
            b"fmt " if fmt.is_none() => fmt = Some((at, stop)),
            b"data" if body.is_none() => body = Some((at, stop)),
            // not a chunk name: the rest of the file is not read
            _ if !id.iter().all(|b| (32..=127).contains(b)) => break,
            _ => {}
        }
        // chunks are padded to an even size
        o = match at.checked_add(size).and_then(|e| e.checked_add(size & 1)) {
            Some(n) => n,
            None => break,
        };
    }
    Some((fmt, body))
}

/// The format tag of a `fmt ` chunk: its own, or the one in an extensible header's GUID.
fn tag(f: &[u8]) -> Result<u16, String> {
    if f.len() < 16 {
        return Err("RIFF WAVE: truncated fmt chunk".into());
    }
    let t = le16(f, 0);
    if t != WAVE_EXTENSIBLE {
        return Ok(t);
    }
    if f.len() < 40 || le16(f, 16) != 22 {
        return Err("RIFF WAVE: bad extensible header".into());
    }
    if f[28..40] != GUID_TAIL || le16(f, 26) != 0 {
        return Err("RIFF WAVE: unsupported format GUID".into());
    }
    Ok(le16(f, 24))
}

/// Whether `data` is a RIFF WAVE file with a `data` chunk and a `fmt ` chunk of a format
/// played here (PCM, IEEE float, Microsoft ADPCM).
pub fn detect(data: &[u8]) -> bool {
    matches!(chunks(data), Some((Some((a, b)), Some(_))) if matches!(tag(&data[a..b]), Ok(WAVE_PCM | WAVE_ADPCM | WAVE_IEEE_FLOAT)))
}

/// One channel's state within a Microsoft ADPCM block.
#[derive(Clone, Copy, Default)]
struct Adpcm {
    coef: (i16, i16),
    delta: i16,
    s1: i16,
    s2: i16,
}

impl Adpcm {
    fn decode(&mut self, code: u8) -> i16 {
        let delta = self.delta as i32;
        self.delta = (((STEP_ADJUST[code as usize] * delta) >> 8) as i16).max(16);
        let code = if code & 8 != 0 { code as i32 - 16 } else { code as i32 };
        let predict = (self.s1 as i32 * self.coef.0 as i32 + self.s2 as i32 * self.coef.1 as i32) >> 8;
        let s = (code * delta + predict).clamp(-32768, 32767) as i16;
        self.s2 = self.s1;
        self.s1 = s;
        s
    }
}

/// Microsoft ADPCM blocks to 16-bit big-endian samples (as `Enc::Be` reads them).
fn adpcm(body: &[u8], channels: usize, block: usize, per_block: usize, coefs: &[(i16, i16)]) -> Vec<u8> {
    let head = 7 * channels;
    // the frames the bytes of a whole block hold; the header may say fewer
    let holds = |bytes: usize| 2 + (bytes - head) * 2 / channels;
    let mut out = Vec::with_capacity(body.len() / block * holds(block).min(per_block) * channels * 2);
    'blocks: for b in body.chunks(block) {
        if b.len() < head {
            break;
        }
        let mut st = [Adpcm::default(); 2];
        for (c, s) in st.iter_mut().enumerate().take(channels) {
            let Some(&coef) = coefs.get(b[c] as usize) else { break 'blocks };
            let le = |o: usize| i16::from_le_bytes([b[o + 2 * c], b[o + 2 * c + 1]]);
            *s = Adpcm { coef, delta: le(channels), s1: le(3 * channels), s2: le(5 * channels) };
        }
        for s in &st[..channels] {
            out.extend(s.s2.to_be_bytes());
        }
        for s in &st[..channels] {
            out.extend(s.s1.to_be_bytes());
        }
        // a code a nibble, the high one first, the channels in turn
        let codes = (holds(b.len()).min(per_block) - 2) * channels;
        for i in 0..codes {
            let byte = b[head + i / 2];
            let code = if i & 1 == 0 { byte >> 4 } else { byte & 15 };
            out.extend(st[i % channels].decode(code).to_be_bytes());
        }
    }
    out
}

/// IEEE floats to 32-bit big-endian floats (as `Enc::F32` reads them), brought into -1..1
/// as the source does.
fn floats(body: &[u8], bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() / bytes * 4);
    let mut normalize = 1.0f32;
    for b in body.chunks_exact(bytes) {
        let v = if bytes == 4 {
            let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            if !(-1.0..=1.0).contains(&f) {
                normalize = 0.9 / f.abs();
            }
            f * normalize
        } else {
            let d = f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            if !(-1.0..=1.0).contains(&d) {
                normalize = (0.9f32 as f64 / d.abs()) as f32;
            }
            (d * normalize as f64) as f32
        };
        if v.is_finite() {
            out.extend(v.to_be_bytes());
        } else {
            // not a number (or an infinity, whose factor is 0): silence, and the factor starts again
            normalize = 1.0;
            out.extend(0f32.to_be_bytes());
        }
    }
    out
}

/// Decodes the file: the frames at the file's rate, interleaved.
pub fn decode(data: Vec<u8>) -> Result<Pcm, String> {
    let (fmt, body) = chunks(&data).ok_or("RIFF WAVE: not a RIFF WAVE file")?;
    let (fa, fb) = fmt.ok_or("RIFF WAVE: no fmt chunk")?;
    let (at, stop) = body.ok_or("RIFF WAVE: no data chunk")?;
    let f = &data[fa..fb];
    let format = tag(f)?;
    let channels = le16(f, 2) as usize;
    let rate = le32(f, 4);
    let block = le16(f, 12) as usize;
    let bits = le16(f, 14) as u32;
    if channels == 0 || channels > 64 {
        return Err(format!("RIFF WAVE: {channels} channels"));
    }
    if !(1..=10_000_000).contains(&rate) {
        return Err("RIFF WAVE: bad sample rate".into());
    }
    let pcm = |data: Vec<u8>, start: usize, len: usize, bits: u32, enc: Enc, format: &'static str| Pcm {
        data,
        start,
        frames: len / (bits.div_ceil(8) as usize * channels),
        channels,
        bits,
        enc,
        rate: rate as f64,
        format,
        title: String::new(),
        loop_start: None,
    };
    match format {
        WAVE_PCM => {
            if !(1..=32).contains(&bits) {
                return Err(format!("RIFF WAVE: {bits}-bit samples"));
            }
            // played from the file itself
            Ok(pcm(data, at, stop - at, bits, if bits <= 8 { Enc::Unsigned8 } else { Enc::Le }, "RIFF WAVE"))
        }
        WAVE_IEEE_FLOAT => {
            if bits != 32 && bits != 64 {
                return Err(format!("RIFF WAVE: {bits}-bit floats"));
            }
            let d = floats(&data[at..stop], bits as usize / 8);
            let n = d.len();
            Ok(pcm(d, 0, n, 32, Enc::F32, "RIFF WAVE (IEEE float)"))
        }
        WAVE_ADPCM => {
            if bits != 4 {
                return Err(format!("RIFF WAVE: {bits}-bit ADPCM"));
            }
            if channels > 2 {
                return Err(format!("RIFF WAVE: ADPCM of {channels} channels"));
            }
            // the extension: its size, the samples of a block, the coefficient sets
            if f.len() < 22 {
                return Err("RIFF WAVE: truncated ADPCM header".into());
            }
            let (extra, per_block, sets) = (le16(f, 16) as usize, le16(f, 18) as usize, le16(f, 20) as usize);
            if extra != 4 + 4 * sets || f.len() < 22 + 4 * sets {
                return Err("RIFF WAVE: bad ADPCM header".into());
            }
            if block < 7 * channels || per_block < 2 {
                return Err("RIFF WAVE: bad ADPCM block size".into());
            }
            let coefs: Vec<(i16, i16)> = (0..sets).map(|i| (le16(f, 22 + 4 * i) as i16, le16(f, 24 + 4 * i) as i16)).collect();
            let d = adpcm(&data[at..stop], channels, block, per_block, &coefs);
            let n = d.len();
            Ok(pcm(d, 0, n, 16, Enc::Be, "RIFF WAVE (ADPCM)"))
        }
        t => Err(format!("RIFF WAVE: unsupported format ${t:04X}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aiff::AiffPlayer;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn fmt(tag: u16, ch: u16, rate: u32, block: u16, bits: u16, extra: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(tag.to_le_bytes());
        v.extend(ch.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend(rate.wrapping_mul(block as u32).to_le_bytes());
        v.extend(block.to_le_bytes());
        v.extend(bits.to_le_bytes());
        v.extend(extra);
        chunk(b"fmt ", &v)
    }

    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut body = b"WAVE".to_vec();
        for c in chunks {
            body.extend(c);
        }
        let mut f = b"RIFF".to_vec();
        f.extend((body.len() as u32).to_le_bytes());
        f.extend(body);
        f
    }

    fn wav(tag: u16, ch: u16, bits: u16, extra: &[u8], data: &[u8]) -> Vec<u8> {
        riff(&[fmt(tag, ch, 48000, ch * bits.div_ceil(8), bits, extra), chunk(b"data", data)])
    }

    /// what the player gives at the file's own rate: the decoded frames themselves
    fn play(f: Vec<u8>) -> Vec<f32> {
        let mut p = AiffPlayer::from_pcm(decode(f).unwrap(), 48000).unwrap();
        let mut out = vec![0f32; 64];
        let n = p.render(&mut out);
        out.truncate(n * 2);
        out
    }

    #[test]
    fn pcm_widths() {
        let f = wav(1, 1, 8, &[], &[0xC0, 0x00, 0xFF]);
        assert!(detect(&f));
        assert_eq!(play(f), [0.5, 0.5, -1.0, -1.0, 127.0 / 128.0, 127.0 / 128.0]);
        assert_eq!(play(wav(1, 2, 16, &[], &[0, 0x40, 0, 0xC0])), [0.5, -0.5]);
        assert_eq!(play(wav(1, 1, 24, &[], &[0, 0, 0x40, 0, 0, 0x80])), [0.5, 0.5, -1.0, -1.0]);
        assert_eq!(play(wav(1, 2, 32, &[], &[0, 0, 0, 0x20, 0, 0, 0, 0xE0])), [0.25, -0.25]);
        // 12 bits in two bytes; six channels: the first two
        assert_eq!(play(wav(1, 1, 12, &[], &[0x00, 0x40])), [0.5, 0.5]);
        assert_eq!(play(wav(1, 6, 16, &[], &[0, 0x40, 0, 0xC0, 0, 1, 0, 2, 0, 3, 0, 4])), [0.5, -0.5]);
        let p = decode(wav(1, 2, 16, &[], &[0; 9])).unwrap();
        assert_eq!((p.format, p.frames, p.channels, p.rate), ("RIFF WAVE", 2, 2, 48000.0));
    }

    #[test]
    fn chunks_before_and_after_and_odd_sizes() {
        // an odd chunk before fmt and an odd data chunk: the pad bytes are skipped, and
        // what follows the data is not played
        let f = riff(&[chunk(b"LIST", b"abc"), fmt(1, 1, 48000, 1, 8, &[]), chunk(b"fact", &[1, 0, 0, 0]), chunk(b"data", &[0xC0, 0x40, 0x80]), chunk(b"LIST", &[9; 6])]);
        assert!(detect(&f));
        assert_eq!(play(f), [0.5, 0.5, -0.5, -0.5, 0.0, 0.0]);
    }

    #[test]
    fn floats_and_their_normalizing() {
        let mut d = Vec::new();
        for v in [0.25f32, -0.5, 2.0, 0.5, -4.0, 0.5, f32::NAN, 0.5] {
            d.extend(v.to_le_bytes());
        }
        let f = wav(3, 1, 32, &[], &d);
        assert!(detect(&f));
        let o: Vec<f32> = play(f).into_iter().step_by(2).collect();
        // 2.0 becomes 0.9 and halves what follows; -4.0 becomes -0.9 and quarters it
        assert_eq!(o, [0.25, -0.5, 0.9, 0.225, -0.9, 0.1125, 0.0, 0.5]);
        let mut d = Vec::new();
        for v in [0.25f64, -2.0, 1.0] {
            d.extend(v.to_le_bytes());
        }
        let p = decode(wav(3, 1, 64, &[], &d)).unwrap();
        assert_eq!(p.format, "RIFF WAVE (IEEE float)");
        let o: Vec<f32> = p.data.chunks(4).map(|b| f32::from_be_bytes(b.try_into().unwrap())).collect();
        assert_eq!(o, [0.25, -0.9, 0.45]);
        assert_eq!(decode(wav(3, 1, 16, &[], &[0; 8])).err().unwrap(), "RIFF WAVE: 16-bit floats");
    }

    fn adpcm_extra(per_block: u16) -> Vec<u8> {
        let coefs: [(i16, i16); 7] = [(256, 0), (512, -256), (0, 0), (192, 64), (240, 0), (460, -208), (392, -232)];
        let mut e = Vec::new();
        e.extend(32u16.to_le_bytes());
        e.extend(per_block.to_le_bytes());
        e.extend(7u16.to_le_bytes());
        for (a, b) in coefs {
            e.extend(a.to_le_bytes());
            e.extend(b.to_le_bytes());
        }
        e
    }

    fn be16s(p: &Pcm) -> Vec<i16> {
        p.data.chunks(2).map(|b| i16::from_be_bytes([b[0], b[1]])).collect()
    }

    #[test]
    fn ms_adpcm_mono() {
        // predictor 1 (512, -256: a straight line through the last two), step 16,
        // samples 100 then 200; codes 0, 1, -1 (15), 7
        let block = [1, 16, 0, 200, 0, 100, 0, 0x01, 0xF7];
        let f = riff(&[fmt(2, 1, 8000, 9, 4, &adpcm_extra(6)), chunk(b"data", &block)]);
        assert!(detect(&f));
        let p = decode(f).unwrap();
        assert_eq!((p.format, p.frames, p.bits), ("RIFF WAVE (ADPCM)", 6, 16));
        // 300 (step 16*230>>8 = 14, kept at 16), 400 + 16, 532 - 16 (step 16), 616 + 7 * 16
        assert_eq!(be16s(&p), [100, 200, 300, 416, 516, 728]);
        // predictor 0 (the last sample), a step that clips: 30000 + 7 * 1000
        let block = [0, 0xE8, 3, 0x30, 0x75, 0, 0, 0x78];
        let f = riff(&[fmt(2, 1, 8000, 8, 4, &adpcm_extra(4)), chunk(b"data", &block)]);
        // then the step is 1000 * 614 >> 8 = 2398, and code -8 takes 19184 off
        assert_eq!(be16s(&decode(f).unwrap()), [0, 30000, 32767, 32767 - 19184]);
    }

    #[test]
    fn ms_adpcm_stereo_blocks_and_a_short_last_one() {
        // two channels, 16-byte blocks of 2 + 2 frames; left predictor 0 step 16, right
        // predictor 2 (no prediction) step 32
        let block = [0, 2, 16, 0, 32, 0, 10, 0, 20, 0, 5, 0, 7, 0, 0x12, 0x3F];
        let mut d = block.to_vec();
        d.extend(block);
        d.extend(&block[..15]); // a last block one byte short: one frame fewer
        let f = riff(&[fmt(2, 2, 8000, 16, 4, &adpcm_extra(4)), chunk(b"data", &d)]);
        let p = decode(f).unwrap();
        assert_eq!((p.channels, p.frames), (2, 11));
        // (the right step after its code 2: 32 * 230 >> 8 = 28)
        let one = [5, 7, 10, 20, 10 + 16, 2 * 32, 26 + 3 * 16, -28];
        let s = be16s(&p);
        assert_eq!(&s[..8], &one);
        assert_eq!(&s[8..16], &one);
        assert_eq!(&s[16..], &one[..6]);
        // a header promising more samples than a block has bytes for: what the bytes hold
        let f = riff(&[fmt(2, 2, 8000, 16, 4, &adpcm_extra(1000)), chunk(b"data", &block)]);
        assert_eq!(decode(f).unwrap().frames, 4);
        // a predictor number beyond the sets ends the sample
        let mut bad = block;
        bad[1] = 7;
        d[16..32].copy_from_slice(&bad);
        let f = riff(&[fmt(2, 2, 8000, 16, 4, &adpcm_extra(4)), chunk(b"data", &d)]);
        assert_eq!(decode(f).unwrap().frames, 4);
    }

    #[test]
    fn extensible() {
        let ext = |tag: u16| {
            let mut e = Vec::new();
            e.extend(22u16.to_le_bytes());
            e.extend(16u16.to_le_bytes());
            e.extend(3u32.to_le_bytes());
            e.extend(tag.to_le_bytes());
            e.extend([0u8, 0]);
            e.extend(GUID_TAIL);
            e
        };
        let f = wav(0xFFFE, 2, 16, &ext(1), &[0, 0x40, 0, 0xC0]);
        assert!(detect(&f));
        assert_eq!(play(f), [0.5, -0.5]);
        let mut d = 0.25f32.to_le_bytes().to_vec();
        d.extend((-0.5f32).to_le_bytes());
        assert_eq!(play(wav(0xFFFE, 2, 32, &ext(3), &d)), [0.25, -0.5]);
        // another GUID, another extension size
        assert!(!detect(&wav(0xFFFE, 2, 16, &ext(6), &[0; 4])));
        let mut e = ext(1);
        e[0] = 24;
        assert!(!detect(&wav(0xFFFE, 2, 16, &e, &[0; 4])));
    }

    #[test]
    fn the_engine_plays_it_to_its_end() {
        use std::path::Path;
        // 100 frames at half the output's rate: 200 output frames, then the song is over
        let mut d = Vec::new();
        for i in 0..100i16 {
            d.extend((i * 300).to_le_bytes());
            d.extend((-i * 300).to_le_bytes());
        }
        let f = riff(&[fmt(1, 2, 24000, 4, 16, &[]), chunk(b"data", &d)]);
        let mut s = crate::open::Song::open(Path::new("x.wav"), f, 48000).unwrap();
        assert_eq!(s.format(), "RIFF WAVE");
        let mut out = vec![0f32; 2 * 300];
        s.render(&mut out[..2 * 150]);
        assert!(!s.songend());
        s.render(&mut out[2 * 150..]);
        assert!(s.songend());
        // frame 20 of the output is frame 10 of the file, at the engine's default volume
        assert_eq!((out[40], out[41]), (3000.0 / 32768.0, -3000.0 / 32768.0));
        assert!(out[2 * 200..].iter().all(|&v| v == 0.0));
        // an 8SVX with a repeat part reports its end and plays on
        let mut b = b"8SVXVHDR".to_vec();
        b.extend(20u32.to_be_bytes());
        b.extend([0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0xBB, 0x80, 1, 0, 0, 1, 0, 0]);
        b.extend(b"BODY\0\0\0\x04\x10\x20\x30\x40");
        let mut f = b"FORM".to_vec();
        f.extend((b.len() as u32).to_be_bytes());
        f.extend(b);
        let mut s = crate::open::Song::open(Path::new("x.8svx"), f, 48000).unwrap();
        assert_eq!(s.format(), "IFF 8SVX");
        let mut out = vec![0f32; 2 * 8];
        s.render(&mut out);
        assert!(s.songend());
        let left: Vec<f32> = out.iter().step_by(2).map(|v| v * 128.0).collect();
        assert_eq!(left, [16.0, 32.0, 48.0, 64.0, 48.0, 64.0, 48.0, 64.0]);
    }

    #[test]
    fn refused() {
        assert!(!detect(b"RIFF\x04\0\0\0AVI "));
        // a-law, mu-law, IMA ADPCM, MPEG: other players' business
        for tag in [6, 7, 0x11, 0x55] {
            let f = wav(tag, 1, 8, &[], &[0; 4]);
            assert!(!detect(&f));
            assert_eq!(decode(f).err().unwrap(), format!("RIFF WAVE: unsupported format ${tag:04X}"));
        }
        // no data, no fmt, a short fmt
        assert!(!detect(&riff(&[fmt(1, 1, 8000, 1, 8, &[])])));
        assert!(!detect(&riff(&[chunk(b"data", &[0; 4])])));
        assert!(!detect(&riff(&[chunk(b"fmt ", &[1, 0, 1, 0]), chunk(b"data", &[0; 4])])));
        let e = |f: Vec<u8>| decode(f).err().unwrap();
        assert_eq!(e(riff(&[fmt(1, 0, 8000, 1, 8, &[]), chunk(b"data", &[0; 4])])), "RIFF WAVE: 0 channels");
        assert_eq!(e(riff(&[fmt(1, 1, 0, 1, 8, &[]), chunk(b"data", &[0; 4])])), "RIFF WAVE: bad sample rate");
        assert_eq!(e(wav(1, 1, 40, &[], &[0; 8])), "RIFF WAVE: 40-bit samples");
        assert_eq!(e(wav(1, 1, 0, &[], &[0; 8])), "RIFF WAVE: 0-bit samples");
        assert_eq!(e(riff(&[fmt(2, 1, 8000, 6, 4, &adpcm_extra(4)), chunk(b"data", &[0; 8])])), "RIFF WAVE: bad ADPCM block size");
        assert_eq!(e(riff(&[fmt(2, 1, 8000, 8, 4, &adpcm_extra(1)), chunk(b"data", &[0; 8])])), "RIFF WAVE: bad ADPCM block size");
        assert_eq!(e(riff(&[fmt(2, 3, 8000, 64, 4, &adpcm_extra(4)), chunk(b"data", &[0; 8])])), "RIFF WAVE: ADPCM of 3 channels");
        assert_eq!(e(riff(&[fmt(2, 1, 8000, 8, 16, &adpcm_extra(4)), chunk(b"data", &[0; 8])])), "RIFF WAVE: 16-bit ADPCM");
        assert_eq!(e(riff(&[fmt(2, 1, 8000, 8, 4, &adpcm_extra(4)[..20]), chunk(b"data", &[0; 8])])), "RIFF WAVE: bad ADPCM header");
    }

    #[test]
    fn damaged() {
        let play = |f: Vec<u8>| {
            if let Ok(p) = decode(f) {
                let mut x = AiffPlayer::from_pcm(p, 44100).unwrap();
                for _ in 0..4 {
                    x.render(&mut [0.0; 64]);
                }
            }
        };
        let block = [0, 2, 16, 0, 32, 0, 10, 0, 20, 0, 5, 0, 7, 0, 0x12, 0x3F, 1, 1, 0xFF, 0x7F, 0xFF, 0x7F, 0, 0x80, 0, 0x80, 0xFF, 0x7F, 0xFF, 0x7F, 0x88, 0x77];
        let files = [
            wav(1, 2, 16, &[], &[1, 2, 3, 4, 5, 6, 7, 8, 9]),
            wav(1, 1, 24, &[], &[1, 2, 3, 4, 5, 6, 7, 8, 9]),
            wav(3, 2, 32, &[], &[0x7F; 17]),
            wav(3, 1, 64, &[], &[0xFF; 17]),
            riff(&[fmt(2, 2, 8000, 16, 4, &adpcm_extra(4)), chunk(b"data", &block)]),
            riff(&[fmt(2, 1, 8000, 16, 4, &adpcm_extra(20)), chunk(b"data", &block)]),
        ];
        // cut anywhere, and every byte of the headers changed: never a panic
        for f in files {
            for n in 0..f.len() {
                play(f[..n].to_vec());
                for v in [0, 1, 2, 0x7F, 0x80, 0xFF] {
                    let mut g = f.clone();
                    g[n] = v;
                    play(g);
                }
            }
        }
        // a data chunk of 4 GB, a rate and a block size as large as they go: what is there
        let mut f = riff(&[fmt(2, 1, 10_000_000, 65535, 4, &adpcm_extra(65535)), chunk(b"data", &block)]);
        let n = f.len();
        f[n - 36..n - 32].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let p = decode(f).unwrap();
        assert_eq!((p.frames, p.data.len()), (2 + 25 * 2, 104));
        let mut f = wav(1, 2, 16, &[], &[1, 2, 3, 4, 5, 6, 7, 8]);
        let n = f.len();
        f[n - 12..n - 8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert_eq!(decode(f).unwrap().frames, 2);
        // no samples: over at once
        let mut x = AiffPlayer::from_pcm(decode(wav(1, 2, 16, &[], &[])).unwrap(), 44100).unwrap();
        assert_eq!(x.render(&mut [0.0; 8]), 0);
        assert!(x.ended());
    }
}
