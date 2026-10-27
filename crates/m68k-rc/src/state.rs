//! Machine-state serialisation (savestates): a little-endian byte [`Writer`]/[`Reader`] pair and
//! the [`Snap`] trait the crates above implement for their state structs (round 5, agent S).
//!
//! No serde: every struct writes its fields explicitly, in a fixed order, and reads them back in
//! place (`get(&mut self)`), so structs with private fields implement it in their own module. The
//! implementations destructure `self` without `..`, so that a field added later fails to compile
//! until it is captured (or explicitly skipped, with the reason). The format has no padding or
//! platform-dependent widths (`usize` is written as u64), so a state written on x86_64 loads on
//! wasm32 and back.

use crate::cpu::Cpu;
use crate::runtime::Exit;

/// A malformed or truncated state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Byte sink of a state.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

/// Byte source of a state.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

/// Zero runs at least this long end a literal in [`Writer::zbytes`].
const ZRUN: usize = 16;

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn len(&mut self, n: usize) {
        self.u64(n as u64);
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.len(b.len());
        self.buf.extend_from_slice(b);
    }
    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    /// A large, mostly-zero byte array (memory): its length, then (zero run, literal length,
    /// literal bytes) records (u32, u32, bytes), ended by a (0, 0) record; the rest is zero.
    pub fn zbytes(&mut self, b: &[u8]) {
        self.len(b.len());
        let n = b.len();
        let mut z0 = 0;
        let mut i = zero_end(b, 0);
        while i < n {
            // the literal ends where a run of ZRUN zeros begins, or at the end (less its zeros)
            let lit0 = i;
            let end = loop {
                let z = next_zero(b, i);
                let k = zero_end(b, z);
                if k == n || k - z >= ZRUN {
                    break z;
                }
                i = k;
            };
            self.u32((lit0 - z0) as u32);
            self.u32((end - lit0) as u32);
            self.buf.extend_from_slice(&b[lit0..end]);
            i = zero_end(b, end);
            z0 = end;
        }
        self.u32(0);
        self.u32(0);
    }
    /// A run-length coded u32 array (an RGBA frame buffer: runs of equal pixels).
    pub fn rle32(&mut self, px: &[u8]) {
        self.len(px.len());
        let word = |i: usize| u32::from_le_bytes(px[4 * i..4 * i + 4].try_into().unwrap());
        let words = px.len() / 4;
        let mut i = 0;
        while i < words {
            let v = word(i);
            let mut n = 1;
            while i + n < words && word(i + n) == v && n < u32::MAX as usize {
                n += 1;
            }
            self.u32(n as u32);
            self.u32(v);
            i += n;
        }
        self.buf.extend_from_slice(&px[words * 4..]);
    }
    pub fn put<T: Snap + ?Sized>(&mut self, v: &T) {
        v.save(self);
    }
}

/// The first zero byte of `b` at or after `i` (or its end), a word at a time.
fn next_zero(b: &[u8], mut i: usize) -> usize {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let n = b.len();
    while i + 8 <= n {
        let w = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let z = w.wrapping_sub(LO) & !w & HI;
        if z != 0 {
            return i + (z.trailing_zeros() / 8) as usize;
        }
        i += 8;
    }
    while i < n && b[i] != 0 {
        i += 1;
    }
    i
}

/// The first non-zero byte of `b` at or after `i` (or its end), a word at a time.
fn zero_end(b: &[u8], mut i: usize) -> usize {
    let n = b.len();
    // long zero stretches (most of the memory) 64 bytes at a time, which vectorises
    while i + 64 <= n && b[i..i + 64].iter().fold(0u8, |a, &x| a | x) == 0 {
        i += 64;
    }
    while i + 8 <= n {
        let w = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        if w != 0 {
            return i + (w.trailing_zeros() / 8) as usize;
        }
        i += 8;
    }
    while i < n && b[i] == 0 {
        i += 1;
    }
    i
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error(format!("state truncated at byte {} (need {} more)", self.pos, n)));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// A length, sanity-checked against what is left (each element takes at least `min` bytes).
    pub fn len(&mut self, min: usize) -> Result<usize> {
        let n = self.u64()?;
        if min > 0 && n > (self.remaining() / min) as u64 {
            return Err(Error(format!("state corrupt at byte {}: length {} too large", self.pos, n)));
        }
        Ok(n as usize)
    }
    pub fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.len(1)?;
        Ok(self.take(n)?.to_vec())
    }
    pub fn string(&mut self) -> Result<String> {
        String::from_utf8(self.bytes()?).map_err(|_| Error("state corrupt: a string is not UTF-8".into()))
    }
    /// [`Writer::zbytes`] into `out`, which must have the written length.
    pub fn zbytes(&mut self, out: &mut [u8]) -> Result<()> {
        let n = self.len(0)?;
        if n != out.len() {
            return Err(Error(format!("state: memory block of {} bytes, expected {}", n, out.len())));
        }
        // each byte written once: the zero runs, the literals, the zero rest
        let mut i = 0usize;
        loop {
            let z = self.u32()? as usize;
            let l = self.u32()? as usize;
            if z == 0 && l == 0 {
                out[i..].fill(0);
                return Ok(());
            }
            if i + z + l > n {
                return Err(Error("state corrupt: memory block overruns".into()));
            }
            out[i..i + z].fill(0);
            i += z;
            out[i..i + l].copy_from_slice(self.take(l)?);
            i += l;
        }
    }
    /// [`Writer::rle32`] into a vector (resized to the written length).
    pub fn rle32(&mut self, out: &mut Vec<u8>) -> Result<()> {
        let n = self.len(0)?;
        if n > 64 << 20 {
            return Err(Error(format!("state corrupt: frame buffer of {} bytes", n)));
        }
        out.clear();
        out.reserve(n);
        while out.len() + 4 <= n {
            let k = self.u32()? as usize;
            let v = self.u32()?.to_le_bytes();
            if k == 0 || out.len() + 4 * k > n {
                return Err(Error("state corrupt: frame buffer run overruns".into()));
            }
            for _ in 0..k {
                out.extend_from_slice(&v);
            }
        }
        let tail = n - out.len();
        out.extend_from_slice(self.take(tail)?);
        Ok(())
    }
    pub fn get<T: Snap + ?Sized>(&mut self, v: &mut T) -> Result<()> {
        v.load(self)
    }
    /// A fresh value read from the state.
    pub fn read<T: Snap + Default>(&mut self) -> Result<T> {
        let mut v = T::default();
        v.load(self)?;
        Ok(v)
    }
    /// Checks that a tag (an enum discriminant) is below `n`.
    pub fn tag(&mut self, n: u8, what: &str) -> Result<u8> {
        let t = self.u8()?;
        if t >= n {
            return Err(Error(format!("state corrupt at byte {}: {} tag {}", self.pos - 1, what, t)));
        }
        Ok(t)
    }
}

/// A part of the machine state: written with [`Snap::save`], read back in place with [`Snap::load`].
pub trait Snap {
    fn save(&self, w: &mut Writer);
    fn load(&mut self, r: &mut Reader) -> Result<()>;
}

macro_rules! snap_int {
    ($($t:ty => $f:ident),*) => {$(
        impl Snap for $t {
            fn save(&self, w: &mut Writer) {
                w.$f(*self as _);
            }
            fn load(&mut self, r: &mut Reader) -> Result<()> {
                *self = r.$f()? as $t;
                Ok(())
            }
        }
    )*};
}
snap_int!(u8 => u8, i8 => u8, u16 => u16, i16 => u16, u32 => u32, i32 => u32, u64 => u64, i64 => u64, usize => u64);

impl Snap for bool {
    fn save(&self, w: &mut Writer) {
        w.u8(*self as u8);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        *self = r.tag(2, "bool")? != 0;
        Ok(())
    }
}

impl Snap for f64 {
    fn save(&self, w: &mut Writer) {
        w.u64(self.to_bits());
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        *self = f64::from_bits(r.u64()?);
        Ok(())
    }
}

impl Snap for String {
    fn save(&self, w: &mut Writer) {
        w.str(self);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        *self = r.string()?;
        Ok(())
    }
}

impl<T: Snap + Default> Snap for Vec<T> {
    fn save(&self, w: &mut Writer) {
        w.len(self.len());
        for x in self {
            x.save(w);
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let n = r.len(1)?;
        self.clear();
        for _ in 0..n {
            self.push(r.read()?);
        }
        Ok(())
    }
}

impl<T: Snap, const N: usize> Snap for [T; N] {
    fn save(&self, w: &mut Writer) {
        for x in self {
            x.save(w);
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        for x in self {
            x.load(r)?;
        }
        Ok(())
    }
}

impl<T: Snap + Default> Snap for Option<T> {
    fn save(&self, w: &mut Writer) {
        match self {
            None => w.u8(0),
            Some(v) => {
                w.u8(1);
                v.save(w);
            }
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        *self = match r.tag(2, "Option")? {
            0 => None,
            _ => Some(r.read()?),
        };
        Ok(())
    }
}

impl<A: Snap, B: Snap> Snap for (A, B) {
    fn save(&self, w: &mut Writer) {
        self.0.save(w);
        self.1.save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.0.load(r)?;
        self.1.load(r)
    }
}

impl<A: Snap, B: Snap, C: Snap> Snap for (A, B, C) {
    fn save(&self, w: &mut Writer) {
        self.0.save(w);
        self.1.save(w);
        self.2.save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.0.load(r)?;
        self.1.load(r)?;
        self.2.load(r)
    }
}

impl Snap for Cpu {
    fn save(&self, w: &mut Writer) {
        let Cpu { d, a, pc, x, n, z, v, c, s, t, imask, other_sp, stopped, ticks, interp_count, budget, cycles, cycle_budget, model, vbr, sfc, dfc, cacr, caar, msp } = self;
        w.put(d);
        w.put(a);
        w.put(pc);
        for f in [x, n, z, v, c, s, t] {
            w.put(f);
        }
        w.put(imask);
        w.put(other_sp);
        w.put(stopped);
        w.put(ticks);
        w.put(interp_count);
        w.put(budget);
        w.put(cycles);
        w.put(cycle_budget);
        w.put(&(*model == crate::types::Model::M68020));
        // The 68020 control registers follow the model, so that a 68000 state is as before.
        if *model == crate::types::Model::M68020 {
            for f in [vbr, sfc, dfc, cacr, caar, msp] {
                w.put(f);
            }
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let Cpu { d, a, pc, x, n, z, v, c, s, t, imask, other_sp, stopped, ticks, interp_count, budget, cycles, cycle_budget, model, vbr, sfc, dfc, cacr, caar, msp } = self;
        r.get(d)?;
        r.get(a)?;
        r.get(pc)?;
        for f in [x, n, z, v, c, s, t] {
            r.get(f)?;
        }
        r.get(imask)?;
        r.get(other_sp)?;
        r.get(stopped)?;
        r.get(ticks)?;
        r.get(interp_count)?;
        r.get(budget)?;
        r.get(cycles)?;
        r.get(cycle_budget)?;
        let mut m020 = false;
        r.get(&mut m020)?;
        *model = if m020 { crate::types::Model::M68020 } else { crate::types::Model::M68000 };
        for f in [vbr, sfc, dfc, cacr, caar, msp] {
            *f = 0;
            if m020 {
                r.get(f)?;
            }
        }
        Ok(())
    }
}

impl Default for Exit {
    fn default() -> Exit {
        Exit::Budget
    }
}

impl Snap for Exit {
    fn save(&self, w: &mut Writer) {
        w.u8(match self {
            Exit::Stop => 0,
            Exit::Stopped => 1,
            Exit::Budget => 2,
        });
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        *self = [Exit::Stop, Exit::Stopped, Exit::Budget][r.tag(3, "Exit")? as usize];
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zbytes_round_trip() {
        let mut cases: Vec<Vec<u8>> = vec![vec![], vec![0; 100], vec![1; 7], vec![0, 0, 1], vec![1, 0, 0]];
        let mut m = vec![0u8; 70_000];
        for (i, b) in m.iter_mut().enumerate() {
            if (i / 300) % 3 == 1 || i % 977 == 0 {
                *b = (i * 7) as u8 | 1;
            }
        }
        m[69_999] = 5;
        cases.push(m.clone());
        m[69_999] = 0;
        cases.push(m);
        let mut z = vec![0u8; 40];
        z[3] = 1;
        z[20] = 2; // a zero run of exactly 16 between literals
        cases.push(z);
        for c in cases {
            let mut w = Writer::new();
            w.zbytes(&c);
            w.u8(0xAB);
            let mut out = vec![0xEEu8; c.len()];
            let mut r = Reader::new(&w.buf);
            r.zbytes(&mut out).unwrap();
            assert_eq!(out, c);
            assert_eq!(r.u8().unwrap(), 0xAB, "reader consumed exactly what was written");
        }
    }

    /// The byte-at-a-time encoder the word-at-a-time one replaced: the output must not change.
    fn zbytes_bytewise(w: &mut Writer, b: &[u8]) {
        w.len(b.len());
        let n = b.len();
        let mut i = 0;
        loop {
            let z0 = i;
            while i < n && b[i] == 0 {
                i += 1;
            }
            if i == n {
                break;
            }
            let lit0 = i;
            let mut zeros = 0;
            let mut end = n;
            while i < n {
                if b[i] == 0 {
                    zeros += 1;
                    if zeros == ZRUN {
                        end = i + 1 - ZRUN;
                        break;
                    }
                } else {
                    zeros = 0;
                }
                i += 1;
            }
            if i == n {
                end = n - zeros;
            }
            w.u32((lit0 - z0) as u32);
            w.u32((end - lit0) as u32);
            w.buf.extend_from_slice(&b[lit0..end]);
            i = end;
        }
        w.u32(0);
        w.u32(0);
    }

    #[test]
    fn zbytes_output_unchanged() {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        for len in [0usize, 1, 7, 8, 9, 15, 16, 17, 31, 33, 100, 1000, 4097, 65_536 + 3] {
            for density in [0u64, 1, 4, 16, 64, 200, 256] {
                let b: Vec<u8> = (0..len)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        if (x >> 8) % 256 < density { (x >> 24) as u8 | 1 } else { 0 }
                    })
                    .collect();
                let (mut a, mut c) = (Writer::new(), Writer::new());
                a.zbytes(&b);
                zbytes_bytewise(&mut c, &b);
                assert_eq!(a.buf, c.buf, "len {len} density {density}");
            }
        }
    }

    #[test]
    fn rle32_and_cpu_round_trip() {
        let px: Vec<u8> = (0..4000u32).flat_map(|i| (if i % 50 < 40 { 0xFF00_0000u32 } else { i }).to_le_bytes()).collect();
        let mut w = Writer::new();
        w.rle32(&px);
        let mut cpu = Cpu::default();
        cpu.d[3] = 0x1234_5678;
        cpu.set_sr(0x2714);
        cpu.ticks = 99;
        w.put(&cpu);
        let mut r = Reader::new(&w.buf);
        let mut out = vec![];
        r.rle32(&mut out).unwrap();
        assert_eq!(out, px);
        let mut c2 = Cpu::default();
        r.get(&mut c2).unwrap();
        assert_eq!(c2, cpu);
        assert_eq!(r.remaining(), 0);
    }
}
