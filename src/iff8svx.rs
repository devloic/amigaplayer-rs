//! IFF 8SVX sample files (8-bit, uncompressed or Fibonacci-delta), decoded for the
//! recorded-audio player (`crate::aiff::AiffPlayer::from_pcm`, the engine's PCM backend).
//!
//! A port of NostalgicPlayer's IFF-8SVX sample converter and of what its Sample player
//! does with it (https://github.com/neumatho/NostalgicPlayer, MIT, Thomas Neumann:
//! `LICENSE-NostalgicPlayer`): `Source/Agents/SampleConverters/Iff8Svx/`
//! `Iff8SvxLoaderWorkerBase.cs`, `Formats/PcmFormat.cs`, `Formats/FibonacciFormat.cs`,
//! and `Source/Agents/Players/Sample/SampleWorker.cs`. The 16 Fibonacci deltas are the
//! format's own table.
//!
//! As that source:
//! - `VHDR` gives the lengths, the rate, the octaves and the compression (0 PCM,
//!   1 Fibonacci delta; the volume is not applied); `CHAN` = 6 makes it stereo, the left
//!   channel's data followed by the right's; `NAME` is the title.
//! - A file of several octaves is played through, one octave after the other, as one
//!   sample, and does not repeat.
//! - A one-octave file with a repeat part repeats from the end of its one-shot part to
//!   the end of its data, for ever; the end is reported each time it gets there.
//! - Fibonacci delta: the second byte is the first value, which is output twice, then
//!   two samples a byte, high nibble first; the 8-bit accumulator wraps (no clipping).
//!   The output is two samples longer than `VHDR` says.
//!
//! Not as that source, which reads sample data from the start of `BODY` to the end of
//! the file whatever the chunk's size: only the bytes of the `BODY` chunk are played.
//! A stereo file's repeat start is counted in frames (the source seeks to half of it).
//! A repeating Fibonacci file goes on with the values decoded the first time (the
//! source restarts its accumulator at 0 when it seeks).
//!
//! The file is not trusted: every size is cut down to the bytes that are there, and the
//! decoded data is at most twice the file's size.

use crate::aiff::{Enc, Pcm};

const FIB: [i8; 16] = [-34, -21, -13, -8, -5, -3, -2, -1, 0, 1, 2, 3, 5, 8, 13, 21];

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

struct Chunks {
    /// VHDR: one-shot length, repeat length, rate, octaves, compression
    vhdr: Option<(u32, u32, u16, u8, u8)>,
    /// BODY: its bytes within the file
    body: Option<(usize, usize)>,
    stereo: bool,
    title: String,
}

/// The chunks of a FORM 8SVX file (the first `VHDR` and `BODY`), or `None` for any other file.
fn chunks(data: &[u8]) -> Option<Chunks> {
    if data.len() < 12 || &data[0..4] != b"FORM" || &data[8..12] != b"8SVX" {
        return None;
    }
    let end = data.len();
    let mut c = Chunks { vhdr: None, body: None, stereo: false, title: String::new() };
    let mut o = 12;
    // every turn moves on by 8 bytes at least
    while o + 8 <= end {
        let id = &data[o..o + 4];
        let size = be32(data, o + 4) as usize;
        let at = o + 8;
        let stop = at.saturating_add(size).min(end); // a chunk cannot outrun the file
        match id {
            b"VHDR" if c.vhdr.is_none() && stop - at >= 20 => {
                c.vhdr = Some((be32(data, at), be32(data, at + 4), u16::from_be_bytes([data[at + 12], data[at + 13]]), data[at + 14], data[at + 15]));
            }
            b"BODY" if c.body.is_none() => c.body = Some((at, stop)),
            b"CHAN" if stop - at >= 4 => c.stereo = be32(data, at) == 6,
            b"NAME" if c.title.is_empty() => {
                // the Amiga's character set is Latin-1
                c.title = data[at..stop].iter().take_while(|&&b| b != 0).take(256).map(|&b| b as char).collect::<String>().trim().to_string();
            }
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
    Some(c)
}

/// Whether `data` is a FORM 8SVX file with a `VHDR` of a known compression and a `BODY`.
pub fn detect(data: &[u8]) -> bool {
    chunks(data).is_some_and(|c| c.body.is_some() && c.vhdr.is_some_and(|v| v.4 <= 1))
}

/// The samples `src` (one channel's Fibonacci-delta bytes) decodes to.
fn fibonacci(src: &[u8]) -> Vec<u8> {
    if src.len() < 2 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(src.len() * 2 - 2);
    let mut v = src[1] as i8;
    out.push(v as u8);
    out.push(v as u8);
    for &b in &src[2..] {
        v = v.wrapping_add(FIB[(b >> 4) as usize]);
        out.push(v as u8);
        v = v.wrapping_add(FIB[(b & 15) as usize]);
        out.push(v as u8);
    }
    out
}

/// Decodes the file: signed 8-bit samples at the file's rate, the frames interleaved.
pub fn decode(data: Vec<u8>) -> Result<Pcm, String> {
    let c = chunks(&data).ok_or("IFF 8SVX: not a FORM 8SVX file")?;
    let (one_shot, repeat, rate, octaves, comp) = c.vhdr.ok_or("IFF 8SVX: no VHDR chunk")?;
    let (at, stop) = c.body.ok_or("IFF 8SVX: no BODY chunk")?;
    if comp > 1 {
        return Err(format!("IFF 8SVX: unsupported compression {comp}"));
    }
    if rate == 0 {
        return Err("IFF 8SVX: bad sample rate".into());
    }
    let fib = comp == 1;
    // the samples of one channel, all octaves (each twice as long as the one before)
    let mut total = one_shot as u64 + repeat as u64;
    let mut len = total;
    for _ in 1..octaves {
        len = len.saturating_mul(2);
        total = total.saturating_add(len);
    }
    // where a repeating sample goes back to (the Fibonacci output starts with two more samples)
    let loop_start = (octaves == 1 && repeat != 0).then_some(one_shot as usize + if fib { 2 } else { 0 });
    let format = if fib { "IFF 8SVX (Fibonacci)" } else { "IFF 8SVX" };
    let title = c.title;
    let pcm = |data: Vec<u8>, start: usize, frames: usize, channels: usize| Pcm {
        data,
        start,
        frames,
        channels,
        bits: 8,
        enc: Enc::Be,
        rate: rate as f64,
        format,
        title,
        loop_start: loop_start.filter(|&s| s < frames),
    };
    if !c.stereo {
        if fib {
            let d = fibonacci(&data[at..stop]);
            let n = d.len();
            return Ok(pcm(d, 0, n, 1));
        }
        // played from the file itself
        return Ok(pcm(data, at, stop - at, 1));
    }
    // stereo: the right channel's bytes follow the left's, which are as many as VHDR says
    // (a header that does not fit the data: half of it each)
    let body = &data[at..stop];
    let left = if fib { total / 2 + 2 } else { total };
    let left = if left == 0 || left >= body.len() as u64 { body.len() / 2 } else { left as usize };
    let n = left.min(body.len() - left);
    let (l, r) = (&body[..n], &body[left..left + n]);
    let (l, r) = if fib { (fibonacci(l), fibonacci(r)) } else { (l.to_vec(), r.to_vec()) };
    let mut d = Vec::with_capacity(l.len() * 2);
    for (a, b) in l.iter().zip(&r) {
        d.push(*a);
        d.push(*b);
    }
    let n = l.len();
    Ok(pcm(d, 0, n, 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aiff::AiffPlayer;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_be_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn vhdr(one_shot: u32, repeat: u32, rate: u16, octaves: u8, comp: u8) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(one_shot.to_be_bytes());
        v.extend(repeat.to_be_bytes());
        v.extend(0u32.to_be_bytes());
        v.extend(rate.to_be_bytes());
        v.push(octaves);
        v.push(comp);
        v.extend(0x10000u32.to_be_bytes());
        chunk(b"VHDR", &v)
    }

    fn form(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut body = b"8SVX".to_vec();
        for c in chunks {
            body.extend(c);
        }
        let mut f = b"FORM".to_vec();
        f.extend((body.len() as u32).to_be_bytes());
        f.extend(body);
        f
    }

    fn samples(p: &Pcm) -> Vec<i8> {
        p.data[p.start..p.start + p.frames * p.channels].iter().map(|&b| b as i8).collect()
    }

    #[test]
    fn pcm_mono_with_an_odd_body_and_chunks_around_it() {
        let f = form(&[chunk(b"NAME", b"Bell\0"), vhdr(3, 0, 8000, 1, 0), chunk(b"ANNO", b"odd"), chunk(b"BODY", &[0x40, 0x80, 0x7F]), chunk(b"ANNO", b"after")]);
        assert!(detect(&f));
        let p = decode(f).unwrap();
        assert_eq!((p.channels, p.rate, p.title.as_str(), p.loop_start, p.format), (1, 8000.0, "Bell", None, "IFF 8SVX"));
        // the pad byte and the chunk after BODY are not sample data
        assert_eq!(samples(&p), [0x40, -128, 0x7F]);
        let mut x = AiffPlayer::from_pcm(p, 8000).unwrap();
        let mut out = [0f32; 12];
        assert_eq!(x.render(&mut out), 3);
        assert_eq!(&out[..8], &[0.5, 0.5, -1.0, -1.0, 127.0 / 128.0, 127.0 / 128.0, 0.0, 0.0]);
        assert!(x.ended());
    }

    #[test]
    fn pcm_stereo_is_left_then_right() {
        let f = form(&[vhdr(2, 0, 8000, 1, 0), chunk(b"CHAN", &6u32.to_be_bytes()), chunk(b"BODY", &[1, 2, 0xFF, 0xFE])]);
        let p = decode(f).unwrap();
        assert_eq!((p.channels, p.frames), (2, 2));
        assert_eq!(samples(&p), [1, -1, 2, -2]);
        // a header that does not fit the data: half each
        let f = form(&[vhdr(0, 0, 8000, 1, 0), chunk(b"CHAN", &6u32.to_be_bytes()), chunk(b"BODY", &[1, 2, 3, 4, 5])]);
        assert_eq!(samples(&decode(f).unwrap()), [1, 3, 2, 4]);
        // CHAN 2 or 4 (one side only) is mono
        let f = form(&[vhdr(4, 0, 8000, 1, 0), chunk(b"CHAN", &2u32.to_be_bytes()), chunk(b"BODY", &[1, 2, 3, 4])]);
        assert_eq!(decode(f).unwrap().channels, 1);
    }

    #[test]
    fn fibonacci_mono_and_stereo() {
        // first value 10 (twice), then +1 -1, +21 +21, and a wrap: 52 + 4 * 21 = 136 = -120
        let body = [0, 10, 0x97, 0xFF, 0xFF, 0xFF];
        let f = form(&[vhdr(8, 0, 8000, 1, 1), chunk(b"BODY", &body)]);
        assert!(detect(&f));
        let p = decode(f).unwrap();
        assert_eq!(p.format, "IFF 8SVX (Fibonacci)");
        assert_eq!(samples(&p), [10, 10, 11, 10, 31, 52, 73, 94, 115, -120]);
        // stereo: each channel 2 + 4 / 2 bytes
        let f = form(&[vhdr(4, 0, 8000, 1, 1), chunk(b"CHAN", &6u32.to_be_bytes()), chunk(b"BODY", &[0, 5, 0x9A, 0x9A, 0, 0xFB, 0x76, 0x76])]);
        let p = decode(f).unwrap();
        assert_eq!((p.channels, p.frames), (2, 6));
        assert_eq!(samples(&p), [5, -5, 5, -5, 6, -6, 8, -8, 9, -9, 11, -11]);
    }

    #[test]
    fn several_octaves_play_through_and_do_not_repeat() {
        let f = form(&[vhdr(1, 1, 8000, 2, 0), chunk(b"BODY", &[1, 2, 3, 4, 5, 6])]);
        let p = decode(f).unwrap();
        assert_eq!((p.frames, p.loop_start), (6, None));
    }

    #[test]
    fn a_repeat_part_plays_on() {
        let f = form(&[vhdr(2, 2, 8000, 1, 0), chunk(b"BODY", &[0x10, 0x20, 0x30, 0x40])]);
        let p = decode(f).unwrap();
        assert_eq!(p.loop_start, Some(2));
        let mut x = AiffPlayer::from_pcm(p, 8000).unwrap();
        let mut out = [0f32; 16];
        assert_eq!(x.render(&mut out[..6]), 3);
        assert!(!x.ended());
        assert_eq!(x.render(&mut out[6..]), 5);
        // the end was reached, and it goes on
        assert!(x.ended());
        let left: Vec<f32> = out.iter().step_by(2).map(|v| v * 128.0).collect();
        assert_eq!(left, [16.0, 32.0, 48.0, 64.0, 48.0, 64.0, 48.0, 64.0]);
        // at another rate too: the last frame leads into the repeat part's first
        let f = form(&[vhdr(2, 2, 4000, 1, 0), chunk(b"BODY", &[0x10, 0x20, 0x30, 0x40])]);
        let mut x = AiffPlayer::from_pcm(decode(f).unwrap(), 8000).unwrap();
        let mut out = [0f32; 24];
        assert_eq!(x.render(&mut out), 12);
        let left: Vec<f32> = out.iter().step_by(2).map(|v| v * 128.0).collect();
        assert_eq!(left, [16.0, 24.0, 32.0, 40.0, 48.0, 56.0, 64.0, 56.0, 48.0, 56.0, 64.0, 56.0]);
        // a repeat start beyond the data: plays once
        let f = form(&[vhdr(9, 2, 8000, 1, 0), chunk(b"BODY", &[1, 2, 3, 4])]);
        assert_eq!(decode(f).unwrap().loop_start, None);
    }

    #[test]
    fn refused() {
        assert!(!detect(b"FORM\0\0\0\x04AIFF"));
        // no BODY, no VHDR, a compression nobody knows: other players' business
        assert!(!detect(&form(&[vhdr(1, 0, 8000, 1, 0)])));
        assert!(!detect(&form(&[chunk(b"BODY", &[1])])));
        assert!(!detect(&form(&[vhdr(1, 0, 8000, 1, 2), chunk(b"BODY", &[1])])));
        assert_eq!(decode(form(&[vhdr(1, 0, 8000, 1, 2), chunk(b"BODY", &[1])])).err().unwrap(), "IFF 8SVX: unsupported compression 2");
        assert_eq!(decode(form(&[vhdr(1, 0, 0, 1, 0), chunk(b"BODY", &[1])])).err().unwrap(), "IFF 8SVX: bad sample rate");
        // a short VHDR is none
        assert!(!detect(&form(&[chunk(b"VHDR", &[0; 19]), chunk(b"BODY", &[1])])));
    }

    #[test]
    fn damaged() {
        let play = |f: Vec<u8>| {
            if let Ok(p) = decode(f) {
                let mut x = AiffPlayer::from_pcm(p, 48000).unwrap();
                for _ in 0..4 {
                    x.render(&mut [0.0; 64]);
                }
            }
        };
        // cut anywhere, and every byte of the headers changed: never a panic
        for (comp, stereo) in [(0, false), (0, true), (1, false), (1, true)] {
            let mut c = vec![vhdr(4, 4, 8000, 1, comp)];
            if stereo {
                c.push(chunk(b"CHAN", &6u32.to_be_bytes()));
            }
            c.push(chunk(b"BODY", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]));
            let f = form(&c);
            for n in 0..f.len() {
                play(f[..n].to_vec());
                for v in [0, 1, 0x7F, 0x80, 0xFF] {
                    let mut g = f.clone();
                    g[n] = v;
                    play(g);
                }
            }
        }
        // sizes and lengths of 4 GB, 255 octaves: nothing is allocated for what is not there
        let mut f = form(&[vhdr(0xFFFF_FFFF, 0xFFFF_FFFF, 65535, 255, 1), chunk(b"CHAN", &6u32.to_be_bytes()), chunk(b"BODY", &[0, 1, 0x99, 0x99, 0, 2, 0x99, 0x99])]);
        let n = f.len();
        f[n - 12..n - 8].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        let p = decode(f).unwrap();
        assert_eq!((p.frames, p.data.len()), (6, 12));
        // an empty BODY plays nothing and is over at once
        let p = decode(form(&[vhdr(0, 0, 8000, 1, 0), chunk(b"BODY", &[])])).unwrap();
        let mut x = AiffPlayer::from_pcm(p, 48000).unwrap();
        assert_eq!(x.render(&mut [0.0; 8]), 0);
        assert!(x.ended());
    }
}
