//! CrunchMania (`CrM!`, `CrM2`, and the sampled-delta `Crm!` / `Crm2`, plus the renamed
//! headers some productions used). Ported from LibAncient's `CrunchManiaDecompressor.cs`
//! (NostalgicPlayer, Thomas Neumann, MIT), itself a port of `CRMDecompressor.cpp` from ancient
//! (Teemu Suutari, BSD-2-Clause); see LICENSE-ancient.

use super::alloc_zeroed;
use super::ancient::{be16, be32, shr, Bwd, BwdOut, Huffman, Lsb, Vlc, MAX_RAW, delta_decode};

const HEADERS: [&[u8; 4]; 10] = [
    b"CrM!", b"CrM2", b"Crm!", b"Crm2",
    b"\x18\x05\x19\x73", // Fears
    b"CD\xb3\xb9",       // BiFi 2
    b"DCS!",             // Sonic Attack / Dual Crew - Shining
    b"Iron",             // Sun / TRSI
    b"MSS!",             // Infection / Mystic
    b"mss!",
];

struct Header {
    raw: usize,
    packed: usize,
    sampled: bool,
    lzh: bool,
}

fn header(d: &[u8]) -> Option<Header> {
    if d.len() < 20 {
        return None;
    }
    let mut hdr: [u8; 4] = d[..4].try_into().ok()?;
    if !HEADERS.contains(&&hdr) {
        return None;
    }
    hdr = match &hdr {
        b"\x18\x05\x19\x73" | b"CD\xb3\xb9" | b"Iron" | b"MSS!" => *b"CrM2",
        b"mss!" => *b"Crm2",
        b"DCS!" => *b"CrM!",
        _ => hdr,
    };
    let raw = be32(d, 6)? as usize;
    let packed = be32(d, 10)? as usize;
    if raw == 0 || packed == 0 || raw > MAX_RAW || packed > MAX_RAW || packed + 14 > d.len() {
        return None;
    }
    Some(Header { raw, packed, sampled: hdr[2] == b'm', lzh: hdr[3] == b'2' })
}

pub(crate) fn test(d: &[u8]) -> bool {
    header(d).is_some()
}

pub(crate) fn depack(d: &[u8]) -> Option<Vec<u8>> {
    let h = header(d)?;
    let mut out = alloc_zeroed(h.raw)?;
    let end = (h.packed + 14).checked_sub(6)?;
    let mut input = Bwd::new(d, 14, end)?;
    let mut bits = Lsb::default();
    {
        // There are empty bits at the start of the stream: take them out.
        let content = be32(d, end)?;
        let shift = be16(d, end + 4)?;
        let len = (shift + 16) as u8;
        bits.reset(shr(content, 16u32.wrapping_sub(shift)), len);
    }
    let mut o = BwdOut::new(&mut out, 0, h.raw)?;
    macro_rules! rb {
        ($n:expr) => {
            bits.bits8(&mut input, $n)?
        };
    }

    if h.lzh {
        loop {
            let length_dec = read_table(&mut bits, &mut input, 9)?;
            let distance_dec = read_table(&mut bits, &mut input, 4)?;
            let items = rb!(16) + 1;
            for _ in 0..items {
                let count = length_dec.decode(|| bits.bits8(&mut input, 1))?;
                if count & 0x100 != 0 {
                    o.write(count as u8)?;
                } else {
                    let count = count + 3;
                    let dbits = distance_dec.decode(|| bits.bits8(&mut input, 1))?;
                    let distance = if dbits == 0 {
                        rb!(1) + 1
                    } else {
                        (rb!(dbits) | 1u32.wrapping_shl(dbits)).wrapping_add(1)
                    };
                    o.copy(distance as usize, count as usize)?;
                }
            }
            if rb!(1) == 0 {
                break;
            }
        }
    } else {
        let length_dec = Huffman::with(&[(1, 0b000, 0), (2, 0b010, 1), (3, 0b110, 2), (3, 0b111, 3)])?;
        let distance_dec = Huffman::with(&[(1, 0b00, 1), (2, 0b10, 0), (2, 0b11, 2)])?;
        let length_vlc = Vlc::new(&[1, 2, 4, 8]);
        let distance_vlc = Vlc::new(&[5, 9, 14]);
        while !o.eof() {
            if rb!(1) != 0 {
                o.write(rb!(8) as u8)?;
            } else {
                let li = length_dec.decode(|| bits.bits8(&mut input, 1))?;
                let mut count = length_vlc.decode(li, |n| bits.bits8(&mut input, n))? + 2;
                if count == 23 {
                    count = if rb!(1) != 0 { rb!(5) + 15 } else { rb!(14) + 15 };
                    for _ in 0..count {
                        o.write(rb!(8) as u8)?;
                    }
                } else {
                    if count > 23 {
                        count -= 1;
                    }
                    let di = distance_dec.decode(|| bits.bits8(&mut input, 1))?;
                    let distance = distance_vlc.decode(di, |n| bits.bits8(&mut input, n))?;
                    o.copy(distance as usize, count as usize)?;
                }
            }
        }
    }
    if !o.eof() {
        return None;
    }
    if h.sampled {
        delta_decode(&mut out);
    }
    Some(out)
}

fn read_table(bits: &mut Lsb, input: &mut Bwd, code_length: u32) -> Option<Huffman> {
    let max_depth = bits.bits8(input, 4)?;
    if max_depth == 0 {
        return None;
    }
    let mut lengths = [0u32; 15];
    for i in 0..max_depth {
        lengths[i as usize] = bits.bits8(input, (i + 1).min(code_length))?;
    }
    let mut dec = Huffman::default();
    let mut code = 0u32;
    for depth in 1..=max_depth {
        for _ in 0..lengths[depth as usize - 1] {
            let value = bits.bits8(input, code_length)?;
            dec.insert(depth, code >> (max_depth - depth), value)?;
            code = code.wrapping_add(1 << (max_depth - depth));
        }
    }
    Some(dec)
}
