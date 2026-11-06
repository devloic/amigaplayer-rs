//! Tests of the libxmp depacker ports on small synthetic data built here
//! (a few encoders for the simplest codes of each format).

use super::decrunch;

fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

// ---------------------------------------------------------------- Pack-Ice

enum Ev {
    Bits(u32, u32),
    Lit(u8),
}

enum Item {
    Slot,
    Byte(u8),
}

/// Lays out a Pack-Ice stream (read backwards by the depacker) for events in
/// consumption order. `wide` selects 32-bit words instead of bytes.
fn ice_stream(ev: &[Ev], wide: bool) -> Vec<u8> {
    let size: u32 = if wide { 32 } else { 8 };
    let mut items = vec![Item::Slot];
    let mut bits: Vec<bool> = Vec::new();
    let mut avail = size as i32 - 1; // the first word has a terminator bit
    for e in ev {
        match *e {
            Ev::Bits(v, n) => {
                if n as i32 > avail {
                    items.push(Item::Slot);
                    avail += size as i32;
                }
                for i in (0..n).rev() {
                    bits.push(v >> i & 1 == 1);
                }
                avail -= n as i32;
            }
            Ev::Lit(b) => items.push(Item::Byte(b)),
        }
    }
    assert_eq!(avail, 0, "bit count does not fill the last word");
    let mut slots = Vec::new();
    let mut it = bits.into_iter();
    for k in 0..items.iter().filter(|i| matches!(i, Item::Slot)).count() {
        let mut w: u32 = 0;
        let n = if k == 0 { size - 1 } else { size };
        for _ in 0..n {
            w = w << 1 | it.next().unwrap() as u32;
        }
        if k == 0 {
            w = w << 1 | 1;
        }
        slots.push(w);
    }
    let mut slots = slots.into_iter();
    let mut out = Vec::new();
    let mut by_item = Vec::new();
    for i in &items {
        by_item.push(match i {
            Item::Slot => {
                let w = slots.next().unwrap();
                if wide { w.to_be_bytes().to_vec() } else { vec![w as u8] }
            }
            Item::Byte(b) => vec![*b],
        });
    }
    for b in by_item.iter().rev() {
        out.extend_from_slice(b);
    }
    out
}

/// A copy of `len` bytes (2..=4) from raw distance 1 ("near") or 33 ("far").
struct Copy {
    len: u32,
    far: bool,
}

/// Events for `lits` literals, then the copies, ending the stream properly.
/// Returns the events and the expected output.
fn ice_events(lits: &[u8], copies: &[Copy], wide: bool, v1: bool, filter: bool) -> (Vec<Ev>, Vec<u8>) {
    let mut ev = vec![
        Ev::Bits(1, 1),
        Ev::Bits(1, 1),
        Ev::Bits(3, 2),
        Ev::Bits(3, 2),
        Ev::Bits(7, 3),
    ];
    let n = lits.len() as u32 - 15;
    if v1 {
        ev.push(Ev::Bits(n, 10));
    } else {
        ev.push(Ev::Bits(n, 8));
    }
    let total = lits.len() + copies.iter().map(|c| c.len as usize).sum::<usize>();
    let mut out = vec![0u8; total];
    let mut pos = total;
    for &b in lits {
        ev.push(Ev::Lit(b));
        pos -= 1;
        out[pos] = b;
    }
    for c in copies {
        match c.len {
            2 => ev.push(Ev::Bits(0, 1)),
            3 => {
                ev.push(Ev::Bits(1, 1));
                ev.push(Ev::Bits(0, 1));
            }
            _ => {
                ev.push(Ev::Bits(6, 3));
                ev.push(Ev::Bits(0, 1));
            }
        }
        let raw = if c.far { 33 } else { 1 };
        if c.len == 2 {
            ev.push(Ev::Bits(raw - 1, 7));
        } else if c.far {
            ev.push(Ev::Bits(0, 1));
            ev.push(Ev::Bits(raw - 33, 8));
        } else {
            ev.push(Ev::Bits(1, 1));
            ev.push(Ev::Bits(0, 1));
            ev.push(Ev::Bits(raw - 1, 5));
        }
        ev.push(Ev::Bits(0, 1)); // empty literal run
        let dist = if wide {
            raw + c.len - 1
        } else if raw > 1 {
            raw + c.len - 2
        } else {
            raw
        };
        let mut w = dist as usize + pos;
        for _ in 0..c.len {
            pos -= 1;
            w -= 1;
            out[pos] = out[w];
        }
    }
    assert_eq!(pos, 0);
    if filter {
        ev.push(Ev::Bits(0, 1));
    }
    (ev, out)
}

fn lits() -> Vec<u8> {
    (0..60u32).map(|i| (i * 7 + 3) as u8).collect()
}

fn five() -> Vec<Copy> {
    (0..5).map(|_| near(2)).collect()
}

fn near(len: u32) -> Copy {
    Copy { len, far: false }
}

fn far(len: u32) -> Copy {
    Copy { len, far: true }
}

fn ice2_file(magic: &[u8; 4], stream: &[u8], out_len: usize) -> Vec<u8> {
    let mut f = magic.to_vec();
    f.extend(be32(stream.len() as u32 + 12));
    f.extend(be32(out_len as u32));
    f.extend_from_slice(stream);
    f
}

#[test]
fn ice_8bit_new_and_old_magic() {
    let (ev, want) = ice_events(&lits(), &[near(2), near(3), near(3)], false, false, true);
    let s = ice_stream(&ev, false);
    for magic in [b"ICE!", b"Ice!"] {
        let f = ice2_file(magic, &s, want.len());
        assert_eq!(decrunch(f), want, "{:?}", magic);
    }
}

#[test]
fn ice_32bit_hacked_magics_and_ambiguous() {
    let (ev, want) = ice_events(&lits(), &five(), true, false, true);
    let s = ice_stream(&ev, true);
    for magic in [b"MICK", b"-CJ-", b"SHE!", b"TMM!", b"TSM!", b"Ice!"] {
        let f = ice2_file(magic, &s, want.len());
        assert_eq!(decrunch(f), want, "{:?}", magic);
    }
    // ICE! is always an 8-bit stream: the same data is refused.
    let f = ice2_file(b"ICE!", &s, want.len());
    assert_eq!(decrunch(f.clone()), f);
}

#[test]
fn ice_v1_trailer() {
    let (ev, want) = ice_events(&lits(), &[near(3), near(3), far(3), far(3)], true, true, false);
    let mut f = ice_stream(&ev, true);
    f.extend(be32(want.len() as u32));
    f.extend_from_slice(b"Ice!");
    assert_eq!(decrunch(f), want);
}

#[test]
fn ice_bitplane_filter() {
    // 8-bit stream with the filter flag and the default 4000 words: the
    // output must be at least 32000 bytes, so use a long RLE-like copy chain.
    // Instead check the filter on its own against the formula.
    let mut d: Vec<u8> = (0..64u32).map(|i| (i * 37 + 11) as u8).collect();
    let orig = d.clone();
    assert!(super::ice::bitplane_filter(&mut d, 8));
    // libxmp's non-fast reference loop.
    let mut want = orig.clone();
    let mut pos = want.len();
    while pos > 0 {
        let (mut p0, mut p1, mut p2, mut p3) = (0u32, 0u32, 0u32, 0u32);
        for _ in 0..4 {
            pos -= 2;
            let mut x = (u16::from_be_bytes([want[pos], want[pos + 1]]) as u32) << 16;
            for _ in 0..4 {
                p0 = p0 << 1 | x >> 31;
                x <<= 1;
                p1 = p1 << 1 | x >> 31;
                x <<= 1;
                p2 = p2 << 1 | x >> 31;
                x <<= 1;
                p3 = p3 << 1 | x >> 31;
                x <<= 1;
            }
        }
        for (k, p) in [p0, p1, p2, p3].into_iter().enumerate() {
            want[pos + 2 * k..pos + 2 * k + 2].copy_from_slice(&(p as u16).to_be_bytes());
        }
    }
    assert_eq!(d, want);
    assert!(!super::ice::bitplane_filter(&mut d, 9));
}

// -------------------------------------------------------------------- SQSH

fn sqsh_checksum(data: &[u8]) -> u16 {
    let mut p = data.to_vec();
    p.extend([0u8; 3]);
    while p.len() % 4 != 0 {
        p.push(0);
    }
    let mut sum = 0u32;
    for w in p[..(data.len() + 3) / 4 * 4].chunks_exact(4) {
        sum ^= u32::from_be_bytes([w[0], w[1], w[2], w[3]]);
    }
    (sum ^ (sum >> 16)) as u16
}

fn sqsh_chunk(ty: u8, packed: &[u8], unpacked: usize) -> Vec<u8> {
    let mut c = vec![ty, 0];
    c.extend(sqsh_checksum(packed).to_be_bytes());
    c.extend((packed.len() as u16).to_be_bytes());
    c.extend((unpacked as u16).to_be_bytes());
    c.extend_from_slice(packed);
    while c.len() % 4 != 0 {
        c.push(0);
    }
    c
}

fn sqsh_file() -> (Vec<u8>, Vec<u8>) {
    let verbatim: Vec<u8> = (0..40u32).map(|i| (i * 5 + 1) as u8).collect();
    // A type 1 chunk: a raw first byte, then seven "0 + 8 bit delta" codes.
    let bytes = [200u8, 3, 250, 17, 17, 0, 99, 128];
    let mut bits: Vec<bool> = Vec::new();
    for w in bytes.windows(2) {
        bits.push(false);
        let d = w[0].wrapping_sub(w[1]);
        for i in (0..8).rev() {
            bits.push(d >> i & 1 == 1);
        }
    }
    let mut packed = vec![0u8, 0, bytes[0]];
    for ch in bits.chunks(8) {
        let mut b = 0u8;
        for (i, &v) in ch.iter().enumerate() {
            b |= (v as u8) << (7 - i);
        }
        packed.push(b);
    }
    packed.resize(32, 0);
    let mut body = vec![0u8; 20];
    body.extend(sqsh_chunk(0, &verbatim, verbatim.len()));
    body.extend(sqsh_chunk(1, &packed, bytes.len()));
    let want: Vec<u8> = verbatim.iter().chain(bytes.iter()).copied().collect();
    let mut f = b"XPKF".to_vec();
    f.extend(be32(body.len() as u32 + 8));
    f.extend_from_slice(b"SQSH");
    f.extend(be32(want.len() as u32));
    f.extend_from_slice(&body[..]);
    // The first 20 bytes of `body` stand in for the XPK chunk header.
    (f, want)
}

#[test]
fn sqsh_verbatim_and_delta_chunks() {
    let (f, want) = sqsh_file();
    assert_eq!(f.len(), 124);
    assert_eq!(decrunch(f.clone()), want);
    // A damaged checksum stops the unpacking short: refused.
    let mut bad = f.clone();
    bad[38] ^= 1;
    assert_eq!(decrunch(bad.clone()), bad);
}

// -------------------------------------------------------------------- S404

fn s404_file() -> (Vec<u8>, Vec<u8>) {
    // Literals (9 bits: 0 + byte) and one 2-byte copy of distance 1.
    let o_len = 70usize;
    let mut out = vec![0u8; o_len];
    let mut d = o_len;
    let mut bits: Vec<bool> = Vec::new();
    let push = |bits: &mut Vec<bool>, v: u32, n: u32| {
        for i in (0..n).rev() {
            bits.push(v >> i & 1 == 1);
        }
    };
    // 66 literals, one copy, 2 literals (decoded from the end of the output).
    for k in 0..69 {
        if k == 66 {
            push(&mut bits, 0x190, 9);
            push(&mut bits, 0, 1);
            for _ in 0..2 {
                d -= 1;
                out[d] = out[d + 1];
            }
        } else {
            let b = (k * 11 + 5) as u8;
            push(&mut bits, b as u32, 9);
            d -= 1;
            out[d] = b;
        }
    }
    assert_eq!(d, 0);
    let p_len = 84usize;
    let mut region = vec![0u8; p_len + 2];
    region[p_len - 4..p_len - 2].copy_from_slice(&12u16.to_be_bytes()); // eff
    for (k, ch) in bits.chunks(16).enumerate() {
        let mut w = 0u16;
        for i in 0..16 {
            w = w << 1 | ch.get(i).copied().unwrap_or(false) as u16;
        }
        let at = p_len - 6 - 2 * k;
        region[at..at + 2].copy_from_slice(&w.to_be_bytes());
    }
    let mut f = b"S404".to_vec();
    f.extend(be32(0));
    f.extend(be32(o_len as u32));
    f.extend(be32(p_len as u32));
    f.extend(region);
    (f, out)
}

#[test]
fn s404_literals_and_copy() {
    let (f, want) = s404_file();
    assert_eq!(decrunch(f), want);
}

// ------------------------------------------------------------------- MMCMP

fn mmcmp_file() -> (Vec<u8>, Vec<u8>) {
    let raw: Vec<u8> = (0..40u32).map(|i| (i * 9 + 2) as u8).collect();
    let table: Vec<u8> = (0..16u32).map(|i| (100 + i * 3) as u8).collect();
    let syms: Vec<u8> = (0..30u32).map(|i| ((i * 5 + 1) % 15) as u8).collect();
    let mut packed_bits = vec![0u8; 15];
    for (k, &s) in syms.iter().enumerate() {
        packed_bits[k / 2] |= s << (4 * (k % 2));
    }
    let mut want = raw.clone();
    want.extend(syms.iter().map(|&s| table[s as usize]));

    let blk = |unpk: u32, pk: u32, flags: u16, tt: u16, nbits: u16, pos: u32, size: u32| {
        let mut b = Vec::new();
        b.extend(unpk.to_le_bytes());
        b.extend(pk.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(flags.to_le_bytes());
        b.extend(tt.to_le_bytes());
        b.extend(nbits.to_le_bytes());
        b.extend(pos.to_le_bytes());
        b.extend(size.to_le_bytes());
        b
    };
    let mut b1 = blk(40, 40, 0, 0, 0, 0, 40);
    b1.extend_from_slice(&raw);
    let mut b2 = blk(30, 16 + 15, 1, 16, 3, 40, 30);
    b2.extend_from_slice(&table);
    b2.extend_from_slice(&packed_bits);

    let blktable = 24u32;
    let off1 = blktable + 8;
    let off2 = off1 + b1.len() as u32;
    let mut f = b"ziRCONia".to_vec();
    f.extend(14u16.to_le_bytes());
    f.extend(0x0210u16.to_le_bytes());
    f.extend(2u16.to_le_bytes());
    f.extend((want.len() as u32).to_le_bytes());
    f.extend(blktable.to_le_bytes());
    f.extend([0u8, 0]);
    f.extend(off1.to_le_bytes());
    f.extend(off2.to_le_bytes());
    f.extend(b1);
    f.extend(b2);
    (f, want)
}

#[test]
fn mmcmp_copy_and_8bit_blocks() {
    let (f, want) = mmcmp_file();
    assert_eq!(decrunch(f.clone()), want);
    // A block offset past the end: refused, passed through.
    let mut bad = f.clone();
    bad[27] = 0xff;
    assert_eq!(decrunch(bad.clone()), bad);
}

// ------------------------------------------------------- sanity and fuzzing

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

#[test]
fn magic_by_chance_passes_through() {
    let mut r = Rng(0x1234_5678_9abc_def1);
    for magic in [&b"ICE!"[..], b"Ice!", b"-CJ-", b"S404", b"ziRCONia", b"XPKF\0\0\0\0SQSH", b"MICK"] {
        for len in [12usize, 63, 64, 100, 300] {
            let mut d: Vec<u8> = (0..len).map(|_| r.next() as u8).collect();
            d[..magic.len()].copy_from_slice(magic);
            assert_eq!(decrunch(d.clone()), d, "{:?} {}", magic, len);
        }
    }
    // The trailer magic of Pack-Ice v1.
    let mut d: Vec<u8> = (0..200).map(|_| r.next() as u8).collect();
    let n = d.len();
    d[n - 4..].copy_from_slice(b"Ice!");
    assert_eq!(decrunch(d.clone()), d);
}

#[test]
fn damaged_streams_never_panic() {
    let (e, _) = ice_events(&lits(), &[near(2), near(3), near(3)], false, false, true);
    let ice = ice2_file(b"ICE!", &ice_stream(&e, false), 68);
    let ice32 = {
        let (e, w) = ice_events(&lits(), &five(), true, false, true);
        ice2_file(b"MICK", &ice_stream(&e, true), w.len())
    };
    let samples = [ice, ice32, sqsh_file().0, s404_file().0, mmcmp_file().0];
    let mut r = Rng(0xdead_beef_1234_5678);
    for s in &samples {
        for i in 0..s.len() {
            for _ in 0..8 {
                let mut d = s.clone();
                d[i] = r.next() as u8;
                let _ = decrunch(d);
            }
            let _ = decrunch(s[..i].to_vec());
        }
        for _ in 0..300 {
            let mut d = s.clone();
            for _ in 0..4 {
                let i = r.next() as usize % d.len();
                d[i] = r.next() as u8;
            }
            let _ = decrunch(d);
        }
    }
}

// ------------------------------------------------------ the public unpack API

#[test]
fn public_unpack_reports_packer_and_bytes() {
    use crate::unpack::{packer, unpack, Packer};
    let (e, want) = ice_events(&lits(), &[near(2), near(3), near(3)], false, false, true);
    let ice = ice2_file(b"ICE!", &ice_stream(&e, false), want.len());
    assert_eq!(packer(&ice), Some(Packer::PackIce));
    assert_eq!(unpack(&ice).unwrap(), Some((Packer::PackIce, want)));

    let (sq, want) = sqsh_file();
    assert_eq!(packer(&sq), Some(Packer::XpkSqsh));
    assert_eq!(unpack(&sq).unwrap(), Some((Packer::XpkSqsh, want)));

    let (s4, want) = s404_file();
    assert_eq!(packer(&s4), Some(Packer::StoneCracker));
    assert_eq!(unpack(&s4).unwrap(), Some((Packer::StoneCracker, want)));

    let (mm, want) = mmcmp_file();
    assert_eq!(packer(&mm), Some(Packer::Mmcmp));
    assert_eq!(unpack(&mm).unwrap(), Some((Packer::Mmcmp, want)));
}

#[test]
fn public_unpack_not_packed_and_corrupt() {
    use crate::unpack::{packer, unpack};
    assert_eq!(packer(b"too short"), None);
    assert!(unpack(&[0u8; 200]).unwrap().is_none());
    // Header says SQSH but the checksum is damaged: an error, not a panic.
    let (mut sq, _) = sqsh_file();
    sq[38] ^= 1;
    assert!(matches!(unpack(&sq), Err(crate::Error::Invalid(_))));
    // PowerPacker header with nothing behind it.
    let mut pp = b"PP20".to_vec();
    pp.extend_from_slice(&[0u8; 20]);
    assert!(unpack(&pp).is_err());
}
