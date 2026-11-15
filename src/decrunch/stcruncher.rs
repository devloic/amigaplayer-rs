//! SoundTracker Cruncher V1.0 (OTT-Design, 1990): `STC\0` packed modules. Written clean-room
//! from `docs/specs/stcruncher/SPEC.md`, which was derived from the tool's input/output
//! behaviour alone: a bitstream of literal / repeat / zero-run tokens and delta runs (bytes and
//! big-endian longwords), with no back-references.
//!
//! The tool's Save writes 2-4 bytes less than the real stream, so the file as saved can end
//! inside the last token. When the stream runs out the decoder keeps what the cut token gave
//! and fills the rest of the output with zeros (the spec's non-strict mode).

use super::MAX_RAW_OUT;

/// Why a stream was not decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    /// Header is not an STC header, or the stream holds an unusable code (count code 56-63).
    Corrupt,
}

/// The original length, if `d` is an STC file with a usable header.
fn header(d: &[u8]) -> Option<usize> {
    if d.len() < 9 || &d[..4] != b"STC\0" {
        return None;
    }
    let size = u32::from_be_bytes([d[4], d[5], d[6], d[7]]) as usize;
    if size == 0 || size > MAX_RAW_OUT {
        return None;
    }
    Some(size)
}

pub(crate) fn test(d: &[u8]) -> bool {
    header(d).is_some()
}

/// MSB-first bit reader; `None` once the data has run out.
struct Bits<'a> {
    d: &'a [u8],
    p: usize,
}

impl Bits<'_> {
    fn peek(&self, n: usize) -> Option<u32> {
        if self.p + n > self.d.len() * 8 {
            return None;
        }
        let mut v = 0u32;
        for i in 0..n {
            let q = self.p + i;
            v = (v << 1) | u32::from((self.d[q >> 3] >> (7 - (q & 7))) & 1);
        }
        Some(v)
    }
    fn get(&mut self, n: usize) -> Option<u32> {
        let v = self.peek(n)?;
        self.p += n;
        Some(v)
    }
}

/// Why decoding stopped early: the data is bad, or the stream simply ran out (not fatal).
enum Stop {
    Bad,
    End,
}

fn get(b: &mut Bits, n: usize) -> Result<u32, Stop> {
    b.get(n).ok_or(Stop::End)
}

fn count(b: &mut Bits) -> Result<usize, Stop> {
    let n = get(b, 6)?;
    Ok(match n {
        0..=47 => n as usize + 1,
        48..=53 => ((((n - 48) << 7) | get(b, 7)?) as usize) + 1,
        54 => get(b, 16)? as usize + 1,
        55 => get(b, 32)? as usize + 1,
        _ => return Err(Stop::Bad),
    })
}

fn signed(v: u32, w: usize) -> i32 {
    if v >> (w - 1) != 0 {
        v as i32 - (1i32 << w)
    } else {
        v as i32
    }
}

fn run(b: &mut Bits, out: &mut Vec<u8>, size: usize) -> Result<(), Stop> {
    while out.len() < size {
        if b.peek(1).ok_or(Stop::End)? == 0 {
            let kind = get(b, 2)?;
            let n = count(b)?.min(size - out.len());
            if kind == 0 {
                for _ in 0..n {
                    out.push(get(b, 8)? as u8);
                }
            } else {
                let last = *out.last().ok_or(Stop::Bad)?;
                out.resize(out.len() + n, last);
            }
            continue;
        }
        let op = get(b, 4)?;
        match op {
            0b1011 => {
                let n = count(b)?.min(size - out.len());
                out.resize(out.len() + n, 0);
            }
            0b1100 | 0b1101 | 0b1110 | 0b1111 | 0b1000 => {
                let w = match op {
                    0b1100 => 3,
                    0b1101 => 4,
                    0b1110 => 5,
                    0b1111 => 6,
                    _ => 7,
                };
                let stop = 1u32 << (w - 1);
                while out.len() < size {
                    let v = get(b, w)?;
                    if v == stop {
                        break;
                    }
                    let last = *out.last().ok_or(Stop::Bad)?;
                    out.push((i32::from(last) + signed(v, w)) as u8);
                }
            }
            _ => {
                // 1001 / 1010: longword delta run
                let mut w = if op == 0b1001 { 8 } else { 16 };
                while out.len() < size {
                    let nib = b.peek(4).ok_or(Stop::End)?;
                    if nib == 0b1000 {
                        b.p += 4;
                        break;
                    }
                    if nib == 0b0111 {
                        b.p += 4;
                        w = 24 - w;
                    }
                    let d = signed(get(b, w)?, w);
                    let l = out.len();
                    if l < 4 {
                        return Err(Stop::Bad);
                    }
                    let prev = u32::from_be_bytes([out[l - 4], out[l - 3], out[l - 2], out[l - 1]]);
                    out.extend_from_slice(&prev.wrapping_add(d as u32).to_be_bytes());
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn decode(d: &[u8]) -> Result<Vec<u8>, Error> {
    let size = header(d).ok_or(Error::Corrupt)?;
    let mut b = Bits { d: &d[8..], p: 0 };
    let mut out = Vec::new();
    out.try_reserve_exact(size + 3).map_err(|_| Error::Corrupt)?;
    let mut res = Ok(());
    for _ in 0..size.min(4) {
        match b.get(8) {
            Some(v) => out.push(v as u8),
            None => {
                res = Err(Stop::End);
                break;
            }
        }
    }
    if res.is_ok() {
        res = run(&mut b, &mut out, size);
    }
    if let Err(Stop::Bad) = res {
        return Err(Error::Corrupt);
    }
    // the stream ran out (zero-fill), or a longword step overshot by up to 3 bytes (cut)
    out.resize(size, 0);
    Ok(out)
}

pub(crate) fn depack(d: &[u8]) -> Option<Vec<u8>> {
    match decode(d) {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("SoundTracker Cruncher: {e:?}");
            None
        }
    }
}
