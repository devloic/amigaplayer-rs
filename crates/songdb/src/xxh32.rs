// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Written for amigaplayer-rs (not vendored): XXH32 after the algorithm's specification
// (https://github.com/Cyan4973/xxHash/blob/dev/doc/xxhash_spec.md), so that the song
// database's keys need no dependency. audacious-uade hashes with the xxHash library; the
// test vectors below are that library's.

//! XXH32, the 32-bit xxHash: what a song database key is made of.

const P1: u32 = 0x9E37_79B1;
const P2: u32 = 0x85EB_CA77;
const P3: u32 = 0xC2B2_AE3D;
const P4: u32 = 0x27D4_EB2F;
const P5: u32 = 0x1656_67B1;

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn round(acc: u32, input: u32) -> u32 {
    acc.wrapping_add(input.wrapping_mul(P2)).rotate_left(13).wrapping_mul(P1)
}

/// The XXH32 of `data` with `seed`.
pub fn xxh32(data: &[u8], seed: u32) -> u32 {
    let mut stripes = data.chunks_exact(16);
    let mut h = if data.len() >= 16 {
        let mut v = [seed.wrapping_add(P1).wrapping_add(P2), seed.wrapping_add(P2), seed, seed.wrapping_sub(P1)];
        for s in &mut stripes {
            for (i, acc) in v.iter_mut().enumerate() {
                *acc = round(*acc, le32(&s[4 * i..]));
            }
        }
        v[0].rotate_left(1)
            .wrapping_add(v[1].rotate_left(7))
            .wrapping_add(v[2].rotate_left(12))
            .wrapping_add(v[3].rotate_left(18))
    } else {
        seed.wrapping_add(P5)
    };
    // (the length modulo 2^32, as the specification has it)
    h = h.wrapping_add(data.len() as u32);
    let mut rest = stripes.remainder();
    while rest.len() >= 4 {
        h = h.wrapping_add(le32(rest).wrapping_mul(P3)).rotate_left(17).wrapping_mul(P4);
        rest = &rest[4..];
    }
    for &b in rest {
        h = h.wrapping_add((b as u32).wrapping_mul(P5)).rotate_left(11).wrapping_mul(P1);
    }
    h ^= h >> 15;
    h = h.wrapping_mul(P2);
    h ^= h >> 13;
    h = h.wrapping_mul(P3);
    h ^ (h >> 16)
}

#[cfg(test)]
mod tests {
    use super::xxh32;

    #[test]
    fn the_reference_implementations_values() {
        assert_eq!(xxh32(b"", 0), 0x02CC_5D05);
        assert_eq!(xxh32(b"", 1), 0x0B2C_B792);
        assert_eq!(xxh32(b"a", 0), 0x550D_7456);
        assert_eq!(xxh32(b"abc", 0), 0x32D1_53FF);
        // 4 to 15 bytes: the 4-byte steps and the tail
        assert_eq!(xxh32(b"abcd", 0), 0xA364_3705);
        // 16 bytes and more: the four accumulators
        assert_eq!(xxh32(b"Nobody inspects the spammish repetition", 0), 0xE229_3B2F);
    }

    #[test]
    fn every_length_around_the_stripe_size_differs() {
        // (a smoke test of the three length classes: none panics, none collides here)
        let data: Vec<u8> = (0..100u8).collect();
        let mut seen = std::collections::HashSet::new();
        for n in 0..=data.len() {
            assert!(seen.insert(xxh32(&data[..n], 0)), "length {n}");
        }
        assert_ne!(xxh32(&data, 0), xxh32(&data, 1));
    }
}
