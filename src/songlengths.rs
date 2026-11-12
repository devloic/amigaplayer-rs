//! HVSC song lengths: the High Voltage SID Collection's `DOCUMENTS/Songlengths.md5`, which
//! gives every C64 tune of the collection the length of each of its songs. The file is not
//! part of this program; give its path.
//!
//! Its format (HVSC 68 and later): a `[Database]` line, comments starting with `;` (the
//! tune's path in the collection), and one line a tune, `MD5=LENGTH LENGTH ...`. The MD5 is
//! that of the whole file, in lower-case hex; the lengths are those of songs 1, 2, ... in
//! `m:ss` or `m:ss.fff`, and older entries may carry a suffix in brackets (`(G)`, `(M)`, ...),
//! which is ignored.
//!
//! ```
//! let db = amigaplayer_rs::songlengths::SongLengths::parse(
//!     "[Database]\n; /DEMOS/x.sid\n900150983cd24fb0d6963f7d28e17f72=1:17 0:05.500(G)\n",
//! );
//! assert_eq!(db.lookup(b"abc"), Some(&[77_000, 5_500][..]));
//! ```

use std::collections::HashMap;
use std::path::Path;

/// The lengths of the songs of every tune listed, by the MD5 of the file.
#[derive(Debug, Default, Clone)]
pub struct SongLengths {
    by_md5: HashMap<[u8; 16], Vec<u32>>,
}

impl SongLengths {
    /// Reads `Songlengths.md5`.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<SongLengths> {
        let bytes = std::fs::read(path)?;
        Ok(SongLengths::parse(&String::from_utf8_lossy(&bytes)))
    }

    /// Reads the file's text. Lines it does not understand are skipped.
    pub fn parse(text: &str) -> SongLengths {
        let mut by_md5 = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('[') {
                continue;
            }
            let Some((key, values)) = line.split_once('=') else { continue };
            let Some(key) = hex16(key.trim()) else { continue };
            let lengths: Option<Vec<u32>> = values.split_whitespace().map(length_ms).collect();
            if let Some(l) = lengths.filter(|l| !l.is_empty()) {
                by_md5.insert(key, l);
            }
        }
        SongLengths { by_md5 }
    }

    /// How many tunes are listed.
    pub fn len(&self) -> usize {
        self.by_md5.len()
    }

    /// Whether no tune is listed.
    pub fn is_empty(&self) -> bool {
        self.by_md5.is_empty()
    }

    /// The lengths in milliseconds of the songs of the tune whose file is `data`, song 1 first.
    pub fn lookup(&self, data: &[u8]) -> Option<&[u32]> {
        self.by_md5.get(&md5(data)).map(|v| v.as_slice())
    }

    /// The length in milliseconds of song `n` (0-based, as the engine counts) of `data`.
    pub fn song_ms(&self, data: &[u8], n: u16) -> Option<u32> {
        self.lookup(data)?.get(n as usize).copied()
    }
}

fn hex16(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 || !s.is_ascii() {
        return None;
    }
    let mut k = [0u8; 16];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(k)
}

/// `m:ss`, `m:ss.f` .. `m:ss.fff`, with any `(...)` suffix, in milliseconds.
fn length_ms(v: &str) -> Option<u32> {
    let v = v.split('(').next()?;
    let (m, rest) = v.split_once(':')?;
    let (s, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let (m, s): (u32, u32) = (m.parse().ok()?, s.parse().ok()?);
    if s >= 60 || frac.len() > 3 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms = if frac.is_empty() { 0 } else { frac.parse::<u32>().ok()? * 10u32.pow(3 - frac.len() as u32) };
    m.checked_mul(60_000)?.checked_add(s * 1000 + ms)
}

/// MD5 (RFC 1321) of `data`.
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
        21,
    ];
    // K[i] = floor(abs(sin(i + 1)) * 2^32)
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: [u8; 16]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_test_suite() {
        // RFC 1321 appendix A.5
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"a")), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(hex(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex(md5(b"message digest")), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(hex(md5(b"abcdefghijklmnopqrstuvwxyz")), "c3fcd3d76192e4007dfb496cca67e13b");
        assert_eq!(
            hex(md5(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890")),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
        // lengths around the padding boundary
        assert_eq!(hex(md5(&[0u8; 55])), "c9ea3314b91c9fd4e38f9432064fd1f2");
        assert_eq!(hex(md5(&[0u8; 64])), "3b5d3c7d207e37dceeedd301e35e2e58");
    }

    #[test]
    fn lengths_and_suffixes() {
        assert_eq!(length_ms("1:17"), Some(77_000));
        assert_eq!(length_ms("4:33.108"), Some(273_108));
        assert_eq!(length_ms("2:46.93"), Some(166_930));
        assert_eq!(length_ms("0:05.5"), Some(5_500));
        assert_eq!(length_ms("3:02(G)"), Some(182_000));
        assert_eq!(length_ms("0:61"), None);
        assert_eq!(length_ms("x"), None);
        assert_eq!(length_ms("1:00.1234"), None);
    }

    #[test]
    fn parse_skips_what_it_does_not_understand() {
        let db = SongLengths::parse(
            "[Database]\n; /a.sid\n900150983cd24fb0d6963f7d28e17f72=1:17 0:05.500(G)\nnot a line\nzz=1:00\n\
             0cc175b9c0f1b6a831c399e269772661=1:00 bad\n",
        );
        assert_eq!(db.len(), 1);
        assert_eq!(db.song_ms(b"abc", 1), Some(5_500));
        assert_eq!(db.song_ms(b"abc", 2), None);
        assert_eq!(db.lookup(b"a"), None);
    }
}
