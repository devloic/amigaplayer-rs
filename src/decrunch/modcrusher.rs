//! modCRUSHER V2.0 (Olly Koenders, 1996): `Okm0` (lossless) and `OkmC` (lossy sample modes 1-4)
//! packed modules. Written clean-room from `docs/specs/modcrusher/SPEC.md`, which was derived
//! from the tool's input/output behaviour alone: an LZ77 + adaptive-Huffman stream (317 symbols)
//! and, for `OkmC`, a 4-bit Fibonacci-delta sample section.
//!
//! Once the root weight reaches 0x8000 (after 32451 coded symbols) the tool freezes its
//! Huffman model: no weight changes, no exchanges, no rescaling, to the end of the stream
//! (found by observing the tool's memory; see the spec).

use super::MAX_RAW_OUT;

const NSYM: usize = 317;
const TREE: usize = 2 * NSYM - 1;
const ROOT: usize = TREE - 1;
const LIMIT: u32 = 0x8000;

/// Why a stream was not decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    /// Header or stream is damaged or truncated.
    Corrupt,
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// (is lossy, total, smp_off, smp_len)
fn header(d: &[u8]) -> Option<(bool, usize, usize, usize)> {
    if d.len() < 17 {
        return None;
    }
    let lossy = match &d[..4] {
        b"Okm0" => false,
        b"OkmC" => true,
        _ => return None,
    };
    let total = be32(d, 4)? as usize;
    let smp_off = be32(d, 8)? as usize;
    let smp_len = be32(d, 12)? as i32;
    if total == 0 || total > MAX_RAW_OUT {
        return None;
    }
    if lossy && (smp_len < 0 || smp_off > total || smp_off + smp_len as usize > total) {
        return None;
    }
    Some((lossy, total, smp_off, smp_len.max(0) as usize))
}

pub(crate) fn test(d: &[u8]) -> bool {
    header(d).is_some()
}

struct Bits<'a> {
    d: &'a [u8],
    p: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32, Error> {
        let b = *self.d.get(self.p >> 3).ok_or(Error::Corrupt)?;
        let v = (b >> (7 - (self.p & 7))) & 1;
        self.p += 1;
        Ok(u32::from(v))
    }
    fn bits(&mut self, n: u32) -> Result<usize, Error> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()? as usize;
        }
        Ok(v)
    }
}

/// The adaptive Huffman tree. Nodes 0..ROOT are in non-decreasing weight order; node `k`'s
/// children are `kid[k]` (bit 0) and `kid[k] + 1` (bit 1); `kid[k] >= TREE` marks the leaf of
/// symbol `kid[k] - TREE`.
pub(crate) struct Tree {
    f: Vec<u32>,
    kid: Vec<usize>,
    up: Vec<usize>,
}

impl Tree {
    pub(crate) fn new() -> Tree {
        let mut f = vec![0u32; TREE + 1];
        let mut kid = vec![0usize; TREE];
        let mut up = vec![0usize; TREE + NSYM];
        for s in 0..NSYM {
            f[s] = 1;
            kid[s] = s + TREE;
            up[s + TREE] = s;
        }
        let mut a = 0;
        for k in NSYM..TREE {
            f[k] = f[a] + f[a + 1];
            kid[k] = a;
            up[a] = k;
            up[a + 1] = k;
            a += 2;
        }
        f[TREE] = u32::MAX; // guard above the root
        Tree { f, kid, up }
    }

    fn update(&mut self, s: usize) -> Result<(), Error> {
        if self.f[ROOT] >= LIMIT {
            return Ok(()); // the model is frozen from here on
        }
        let mut n = self.up[s + TREE];
        loop {
            self.f[n] += 1;
            let v = self.f[n];
            if v > self.f[n + 1] {
                let mut m = n + 1;
                while v > self.f[m + 1] {
                    m += 1;
                }
                // exchange what hangs at n and m
                self.f[n] = self.f[m];
                self.f[m] = v;
                let a = self.kid[n];
                self.up[a] = m;
                if a < TREE {
                    self.up[a + 1] = m;
                }
                let b = self.kid[m];
                self.kid[m] = a;
                self.up[b] = n;
                if b < TREE {
                    self.up[b + 1] = n;
                }
                self.kid[n] = b;
                n = m;
            }
            n = self.up[n];
            if n == 0 {
                break;
            }
        }
        Ok(())
    }

    fn decode(&mut self, br: &mut Bits) -> Result<usize, Error> {
        let mut k = self.kid[ROOT];
        while k < TREE {
            k = self.kid[k + br.bit()? as usize];
        }
        let s = k - TREE;
        self.update(s)?;
        Ok(s)
    }

    /// Test helper for an encoder: the code bits of `s` (root first), then the update.
    #[cfg(test)]
    pub(crate) fn encode(&mut self, s: usize) -> Result<Vec<bool>, Error> {
        let mut bits = Vec::new();
        let mut n = self.up[s + TREE];
        while n != ROOT {
            let p = self.up[n];
            bits.push(n != self.kid[p]);
            n = p;
        }
        bits.reverse();
        self.update(s)?;
        Ok(bits)
    }
}

/// Class (0..63) of the distance prefix code: canonical, with lengths 3 (x1), 4 (x3), 5 (x8),
/// 6 (x12), 7 (x24) and 8 (x16).
fn read_class(br: &mut Bits) -> Result<usize, Error> {
    const LEN: [(u32, usize); 6] = [(3, 1), (4, 3), (5, 8), (6, 12), (7, 24), (8, 16)];
    let (mut code, mut first, mut ln, mut base) = (0usize, 0usize, 0u32, 0usize);
    for (l, count) in LEN {
        while ln < l {
            code = (code << 1) | br.bit()? as usize;
            ln += 1;
        }
        if code >= first && code < first + count {
            return Ok(base + code - first);
        }
        first = (first + count) << 1;
        base += count;
    }
    Err(Error::Corrupt)
}

fn lz_decode(data: &[u8], n: usize) -> Result<Vec<u8>, Error> {
    let mut br = Bits { d: data, p: 0 };
    let mut t = Tree::new();
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve_exact(n).map_err(|_| Error::Corrupt)?;
    while out.len() < n {
        let s = t.decode(&mut br)?;
        if s < 256 {
            out.push(s as u8);
        } else {
            let len = s - 255;
            let d = (read_class(&mut br)? << 6) | br.bits(6)?;
            if d < 1 || d > out.len() {
                return Err(Error::Corrupt);
            }
            for _ in 0..len {
                out.push(out[out.len() - d]);
            }
        }
    }
    out.truncate(n);
    Ok(out)
}

fn fib_step(n: u8, unit: i32) -> i32 {
    const F: [i32; 9] = [0, 1, 2, 3, 5, 8, 13, 21, 34];
    if n >= 8 {
        F[usize::from(n) - 8] * unit
    } else {
        -F[8 - usize::from(n)] * unit
    }
}

pub(crate) fn decode(d: &[u8]) -> Result<Vec<u8>, Error> {
    let (lossy, total, smp_off, smp_len) = header(d).ok_or(Error::Corrupt)?;
    let bits = &d[16..];
    if !lossy {
        return lz_decode(bits, total);
    }
    let tail = total - smp_off - smp_len;
    let raw = lz_decode(bits, smp_off + 2 + smp_len / 2 + tail)?;
    let mode = raw[smp_off];
    if mode > 3 {
        return Err(Error::Corrupt);
    }
    let unit = 1i32 << mode;
    let packed = &raw[smp_off + 2..smp_off + 2 + smp_len / 2];
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&raw[..smp_off]);
    let (mut acc, mut flip) = (0i32, false);
    for i in 0..smp_len {
        let b = packed[i >> 1];
        let n = if i & 1 == 0 { b >> 4 } else { b & 15 };
        acc += fib_step(n, unit);
        if !(-128..=127).contains(&acc) {
            acc = ((acc + 128) & 255) - 128;
            flip = !flip;
        }
        out.push((if flip { -acc } else { acc }) as u8);
    }
    out.extend_from_slice(&raw[smp_off + 2 + smp_len / 2..]);
    Ok(out)
}

pub(crate) fn depack(d: &[u8]) -> Option<Vec<u8>> {
    match decode(d) {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("modCRUSHER: {e:?}");
            None
        }
    }
}
