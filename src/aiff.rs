//! A small AIFF / AIFF-C decoder: recorded audio played as it is (no Paula), for the
//! hosts whose own audio stack cannot read AIFF (browsers).
//!
//! Plain PCM only: big-endian signed integers of 1 to 32 bits (also AIFF-C `NONE`,
//! `twos`, `in24`, `in32`), little-endian (`sowt`), unsigned 8-bit (`raw `) and
//! IEEE floats (`fl32`, `fl64`). Any other compression (`ima4`, `ulaw`, `alaw`, ...)
//! is refused with a message naming it.
//!
//! More than two channels: the first two are played (front left and right in every
//! common layout); one channel is played on both sides. The rate is converted to the
//! output's by linear interpolation, or by averaging the source frames under each
//! output frame when the output is the slower one. Sample data is read in place, a
//! frame at a time, so a long file costs no memory beyond the file itself.

/// Whether `data` is a FORM AIFF or FORM AIFC file.
pub fn detect(data: &[u8]) -> bool {
    data.len() >= 12 && &data[0..4] == b"FORM" && (&data[8..12] == b"AIFF" || &data[8..12] == b"AIFC")
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Enc {
    Be,
    Le,
    Unsigned8,
    F32,
    F64,
}

pub struct AiffPlayer {
    data: Vec<u8>,
    /// the sample bytes within `data`
    start: usize,
    frames: usize,
    channels: usize,
    bytes: usize,
    bits: u32,
    enc: Enc,
    src_rate: f64,
    step: f64,
    /// position in source frames of the next output frame
    pos: f64,
    aifc: bool,
    title: String,
}

fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// An 80-bit IEEE extended number (sign, 15-bit exponent, 64-bit mantissa with an explicit
/// integer bit), big-endian.
fn extended(d: &[u8]) -> f64 {
    let sign = if d[0] & 0x80 != 0 { -1.0 } else { 1.0 };
    let exp = (((d[0] & 0x7F) as i32) << 8) | d[1] as i32;
    let mant = u64::from_be_bytes([d[2], d[3], d[4], d[5], d[6], d[7], d[8], d[9]]);
    if exp == 0 && mant == 0 {
        return 0.0;
    }
    if exp == 0x7FFF {
        return f64::NAN;
    }
    sign * mant as f64 * 2f64.powi(exp - 16383 - 63)
}

impl AiffPlayer {
    /// Reads the headers of `data` and prepares to play it at `rate` Hz.
    pub fn open(data: Vec<u8>, rate: u32) -> Result<AiffPlayer, String> {
        if !detect(&data) {
            return Err("AIFF: not a FORM AIFF file".into());
        }
        let aifc = &data[8..12] == b"AIFC";
        // the FORM size may be wrong (streams write 0 or too much): the file's end rules
        let end = data.len();
        let mut comm = None;
        let mut ssnd = None;
        let mut title = String::new();
        let mut o = 12;
        while o + 8 <= end {
            let id = &data[o..o + 4];
            let size = be32(&data, o + 4) as usize;
            let body = o + 8;
            let stop = body.saturating_add(size).min(end); // a chunk cannot outrun the file
            match id {
                b"COMM" => comm = Some((body, stop)),
                b"SSND" => ssnd = Some((body, stop)),
                b"NAME" if title.is_empty() => title = String::from_utf8_lossy(&data[body..stop]).trim_end_matches('\0').trim().to_string(),
                _ => {}
            }
            // chunks are padded to an even size
            o = match body.checked_add(size).and_then(|e| e.checked_add(size & 1)) {
                Some(n) => n,
                None => break,
            };
        }
        let (c, ce) = comm.ok_or("AIFF: no COMM chunk")?;
        if ce - c < 18 {
            return Err("AIFF: truncated COMM chunk".into());
        }
        let channels = be16(&data, c) as usize;
        let frames = be32(&data, c + 2) as usize;
        let bits = be16(&data, c + 6) as u32;
        let src_rate = extended(&data[c + 8..c + 18]);
        if channels == 0 || channels > 64 {
            return Err(format!("AIFF: {channels} channels"));
        }
        if !(1..=32).contains(&bits) {
            return Err(format!("AIFF: {bits}-bit samples"));
        }
        if !(src_rate.is_finite() && (1.0..=10_000_000.0).contains(&src_rate)) {
            return Err("AIFF: bad sample rate".into());
        }
        let mut enc = Enc::Be;
        if aifc {
            if ce - c < 22 {
                return Err("AIFF: truncated COMM chunk".into());
            }
            let tag: [u8; 4] = data[c + 18..c + 22].try_into().unwrap();
            enc = match &tag {
                b"NONE" | b"twos" | b"in24" | b"in32" => Enc::Be,
                b"sowt" => Enc::Le,
                b"raw " => Enc::Unsigned8,
                b"fl32" | b"FL32" => Enc::F32,
                b"fl64" | b"FL64" => Enc::F64,
                t => return Err(format!("AIFF: unsupported compression {}", String::from_utf8_lossy(t).trim())),
            };
            if (enc == Enc::F32 && bits != 32) || (enc == Enc::F64 && bits != 64) || (enc == Enc::Unsigned8 && bits > 8) {
                return Err("AIFF: sample size does not match the compression".into());
            }
        }
        let bytes = bits.div_ceil(8) as usize;
        let (s, se) = ssnd.ok_or("AIFF: no SSND chunk")?;
        if se - s < 8 {
            return Err("AIFF: truncated SSND chunk".into());
        }
        let start = (s + 8).saturating_add(be32(&data, s) as usize).min(se);
        // the frame count may promise more than the chunk (or the file) holds
        let frames = frames.min((se - start) / (bytes * channels));
        Ok(AiffPlayer {
            data,
            start,
            frames,
            channels,
            bytes,
            bits,
            enc,
            src_rate,
            step: src_rate / rate as f64,
            pos: 0.0,
            aifc,
            title,
        })
    }

    pub fn format(&self) -> &'static str {
        if self.aifc { "AIFF-C" } else { "AIFF" }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The file's own sample rate, channels and bits per sample.
    pub fn info(&self) -> (f64, usize, u32) {
        (self.src_rate, self.channels, self.bits)
    }

    pub fn ended(&self) -> bool {
        self.pos >= self.frames as f64
    }

    /// One channel of one source frame, -1..1.
    fn sample(&self, frame: usize, ch: usize) -> f32 {
        let o = self.start + (frame * self.channels + ch) * self.bytes;
        let b = &self.data[o..o + self.bytes];
        match self.enc {
            Enc::F32 => f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            Enc::F64 => f64::from_be_bytes(b.try_into().unwrap()) as f32,
            Enc::Unsigned8 => (b[0] as f32 - 128.0) / 128.0,
            Enc::Be | Enc::Le => {
                // the value sits at the top of its bytes: assemble, then scale by the width
                let mut v: u32 = 0;
                for i in 0..self.bytes {
                    let byte = if self.enc == Enc::Be { b[i] } else { b[self.bytes - 1 - i] };
                    v = (v << 8) | byte as u32;
                }
                let v = (v << (32 - 8 * self.bytes)) as i32;
                (v as f64 / 2147483648.0) as f32
            }
        }
    }

    fn frame(&self, i: usize) -> (f32, f32) {
        let i = i.min(self.frames - 1);
        let l = self.sample(i, 0);
        (l, if self.channels > 1 { self.sample(i, 1) } else { l })
    }

    /// Fills `out` with interleaved stereo frames; returns how many frames were
    /// decoded (the rest of `out` is silence).
    pub fn render(&mut self, out: &mut [f32]) -> usize {
        let mut n = 0;
        for f in out.chunks_exact_mut(2) {
            if self.ended() {
                f[0] = 0.0;
                f[1] = 0.0;
                continue;
            }
            let i = self.pos as usize;
            let (l, r) = if self.step > 1.0 {
                // slower output: the mean of the source frames this output frame covers
                let end = (((self.pos + self.step) as usize).max(i + 1)).min(self.frames);
                let (mut sl, mut sr) = (0.0, 0.0);
                for k in i..end {
                    let (a, b) = self.frame(k);
                    sl += a;
                    sr += b;
                }
                let c = (end - i) as f32;
                (sl / c, sr / c)
            } else {
                let t = (self.pos - i as f64) as f32;
                let (a, b) = self.frame(i);
                if t == 0.0 {
                    (a, b)
                } else {
                    let (c, d) = self.frame(i + 1);
                    (a + (c - a) * t, b + (d - b) * t)
                }
            };
            f[0] = l;
            f[1] = r;
            self.pos += self.step;
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(rate: f64) -> [u8; 10] {
        // exponent and mantissa of a positive integer-valued rate
        let r = rate as u64;
        let e = 63 - r.leading_zeros() as i32;
        let m = r << (63 - e);
        let mut b = [0u8; 10];
        b[0..2].copy_from_slice(&((e + 16383) as u16).to_be_bytes());
        b[2..10].copy_from_slice(&m.to_be_bytes());
        b
    }

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_be_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn file(ch: u16, bits: u16, rate: f64, comp: Option<&[u8; 4]>, pcm: &[u8]) -> Vec<u8> {
        let frames = pcm.len() / (ch as usize * (bits as usize).div_ceil(8));
        let mut c = Vec::new();
        c.extend(ch.to_be_bytes());
        c.extend((frames as u32).to_be_bytes());
        c.extend(bits.to_be_bytes());
        c.extend(ext(rate));
        if let Some(t) = comp {
            c.extend(t);
            c.extend([0u8, 0]); // empty pstring (count + pad)
        }
        let mut s = vec![0u8; 8];
        s.extend(pcm);
        let mut body = if comp.is_some() { b"AIFC".to_vec() } else { b"AIFF".to_vec() };
        body.extend(chunk(b"COMM", &c));
        body.extend(chunk(b"SSND", &s));
        let mut f = b"FORM".to_vec();
        f.extend((body.len() as u32).to_be_bytes());
        f.extend(body);
        f
    }

    fn play(d: Vec<u8>, rate: u32) -> Vec<f32> {
        let mut p = AiffPlayer::open(d, rate).unwrap();
        let mut out = vec![0f32; 64];
        let n = p.render(&mut out);
        out.truncate(n * 2);
        out
    }

    #[test]
    fn extended_rates() {
        for r in [44100.0, 48000.0, 22050.0, 8000.0, 96000.0] {
            assert_eq!(extended(&ext(r)), r);
        }
    }

    #[test]
    fn pcm_widths() {
        let o = play(file(1, 8, 48000.0, None, &[0x40, 0x80, 0x7F]), 48000);
        assert_eq!(o, vec![0.5, 0.5, -1.0, -1.0, 127.0 / 128.0, 127.0 / 128.0]);
        let o = play(file(2, 16, 48000.0, None, &[0x40, 0, 0xC0, 0]), 48000);
        assert_eq!(o, vec![0.5, -0.5]);
        let o = play(file(1, 24, 48000.0, None, &[0x40, 0, 0, 0x80, 0, 0]), 48000);
        assert_eq!(o, vec![0.5, 0.5, -1.0, -1.0]);
        let o = play(file(2, 32, 48000.0, None, &[0x20, 0, 0, 0, 0xE0, 0, 0, 0]), 48000);
        assert_eq!(o, vec![0.25, -0.25]);
        // 12 bits in two bytes, left-justified
        let o = play(file(1, 12, 48000.0, None, &[0x40, 0x00]), 48000);
        assert_eq!(o, vec![0.5, 0.5]);
    }

    #[test]
    fn aifc_encodings() {
        let o = play(file(2, 16, 48000.0, Some(b"sowt"), &[0, 0x40, 0, 0xC0]), 48000);
        assert_eq!(o, vec![0.5, -0.5]);
        let o = play(file(2, 16, 48000.0, Some(b"NONE"), &[0x40, 0, 0xC0, 0]), 48000);
        assert_eq!(o, vec![0.5, -0.5]);
        let mut pcm = 0.25f32.to_be_bytes().to_vec();
        pcm.extend((-0.5f32).to_be_bytes());
        let o = play(file(2, 32, 48000.0, Some(b"fl32"), &pcm), 48000);
        assert_eq!(o, vec![0.25, -0.5]);
        let o = play(file(1, 8, 48000.0, Some(b"raw "), &[0xC0]), 48000);
        assert_eq!(o, vec![0.5, 0.5]);
    }

    #[test]
    fn refused_and_damaged() {
        let e = AiffPlayer::open(file(1, 16, 44100.0, Some(b"ima4"), &[0, 0]), 48000).err().unwrap();
        assert_eq!(e, "AIFF: unsupported compression ima4");
        // truncated everywhere: never a panic
        let f = file(2, 16, 44100.0, None, &[0x40, 0, 0xC0, 0, 1, 2, 3, 4]);
        for n in 0..f.len() {
            let _ = AiffPlayer::open(f[..n].to_vec(), 48000).map(|mut p| p.render(&mut [0.0; 16]));
        }
        // a frame count and chunk size beyond the file are clamped to what is there
        let mut f = file(2, 16, 44100.0, None, &[0x40, 0, 0xC0, 0, 1, 2, 3, 4]);
        f[22..26].copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes()); // COMM frames
        let n = f.len();
        f[n - 16 - 4..n - 16].copy_from_slice(&0xFFFF_FF00u32.to_be_bytes()); // SSND size
        assert_eq!(AiffPlayer::open(f.clone(), 44100).unwrap().frames, 2);
        let mut p = AiffPlayer::open(f, 44100).unwrap();
        assert_eq!(p.render(&mut [0.0; 16]), 2);
        assert!(p.ended());
    }

    #[test]
    fn many_channels_take_the_first_two() {
        let o = play(file(3, 16, 48000.0, None, &[0x40, 0, 0xC0, 0, 0x20, 0]), 48000);
        assert_eq!(o, vec![0.5, -0.5]);
    }

    #[test]
    fn rate_conversion() {
        // 24 kHz to 48 kHz: the frames between are interpolated
        let o = play(file(1, 16, 24000.0, None, &[0, 0, 0x40, 0]), 48000);
        assert_eq!(&o[..6], &[0.0, 0.0, 0.25, 0.25, 0.5, 0.5]);
        // 96 kHz to 48 kHz: pairs are averaged
        let o = play(file(1, 16, 96000.0, None, &[0x40, 0, 0x20, 0, 0, 0, 0, 0]), 48000);
        assert_eq!(&o[..4], &[0.375, 0.375, 0.0, 0.0]);
    }
}
