//! XPK-BZP2 (a bzip2 stream per chunk). Ported from LibAncient's `BZip2Decompressor.cs`
//! (NostalgicPlayer, Thomas Neumann, MIT), a port of ancient's `BZIP2Decompressor.cpp` (Teemu
//! Suutari, BSD-2-Clause); see LICENSE-ancient. The randomisation table is the one of the bzip2
//! reference (LICENSE-ancient carries its notice too).

use crate::decrunch::ancient::{be32, crc32_rev, Fwd, Huffman, Msb};

/// `BZh1`..`BZh9`: the block size in units of 100 000 bytes.
pub(super) fn check(chunk: &[u8]) -> Option<usize> {
    let hdr = be32(chunk, 0)?;
    let level = hdr & 0xff;
    if hdr & 0xffff_ff00 != u32::from_be_bytes(*b"BZh\0") || !(u32::from(b'1')..=u32::from(b'9')).contains(&level) {
        return None;
    }
    Some((level - u32::from(b'0')) as usize * 100_000)
}

/// The last stage: the pre-0.9.5 randomisation and the run length coding of 4 equal bytes
/// plus a count.
struct Rle {
    randomized: bool,
    random_pos: usize,
    random_counter: u32,
    ch: u8,
    count: u32,
    pos: usize,
}

impl Rle {
    fn byte(&mut self, mut c: u8, out: &mut [u8]) -> Option<()> {
        if self.randomized {
            // One LSB flipped after the n-th byte, n from the table.
            let flip = if self.random_counter == 0 {
                self.random_counter = u32::from(RANDOM_TABLE[self.random_pos & 511]);
                self.random_pos += 1;
                self.random_counter != 0
            } else {
                self.random_counter -= 1;
                false
            };
            if flip {
                c ^= 1;
            }
        }
        if self.count == 0 {
            self.ch = c;
            self.count = 1;
        } else if c == self.ch && self.count != 4 {
            self.count += 1;
        } else {
            let n = if self.count == 4 { u32::from(c) + 4 } else { self.count } as usize;
            out.get_mut(self.pos..self.pos.checked_add(n)?)?.fill(self.ch);
            self.pos += n;
            if self.count == 4 {
                self.count = 0;
            } else {
                self.ch = c;
                self.count = 1;
            }
        }
        Some(())
    }
}

fn un_mtf(value: u8, map: &mut [u8]) -> u8 {
    let v = value as usize;
    let ret = map[v];
    if v != 0 {
        map.copy_within(0..v, 1);
        map[0] = ret;
    }
    ret
}

/// Decodes the bzip2 stream in `chunk` into `out`, which it must not overflow (the C++ output
/// cannot grow inside an XPK chunk); what it does not fill stays zero.
pub(super) fn decompress(chunk: &[u8], out: &mut [u8]) -> Option<()> {
    let block_size = check(chunk)?;
    let mut input = Fwd::new(chunk, 4, chunk.len())?;
    let mut bits = Msb::default();
    macro_rules! rb {
        ($n:expr) => {
            bits.bits8(&mut input, $n)?
        };
    }
    let mut pos = 0usize;
    let mut crc = 0u32;

    let selector_dec =
        Huffman::with(&[(1, 0b000000, 0), (2, 0b000010, 1), (3, 0b000110, 2), (4, 0b001110, 3), (5, 0b011110, 4), (6, 0b111110, 5)])?;
    // -1 is stored as u32::MAX.
    let delta_dec = Huffman::with(&[(1, 0b00, 0), (2, 0b10, 1), (2, 0b11, u32::MAX)])?;
    let mut tmp = vec![0u8; block_size];

    loop {
        let hi = rb!(32);
        let lo = rb!(16);
        if hi == 0x3141_5926 && lo == 0x5359 {
            rb!(32); // block CRC, not used
            let randomized = rb!(1) != 0;
            let mut current_ptr = rb!(24);
            let mut block_len = 0usize;
            {
                let mut num_items = 2u32;
                let mut values = [0u8; 256];
                let mut used = [false; 16];
                for u in used.iter_mut() {
                    *u = rb!(1) != 0;
                }
                let mut map = [false; 256];
                for i in 0..16 {
                    for j in 0..16 {
                        map[i * 16 + j] = if used[i] { rb!(1) != 0 } else { false };
                    }
                }
                let mut k = 0;
                for (i, &m) in map.iter().enumerate() {
                    if m {
                        num_items += 1;
                        values[k] = i as u8;
                        k += 1;
                    }
                }
                if num_items == 2 {
                    return None;
                }
                let groups = rb!(3);
                if !(2..=6).contains(&groups) {
                    return None;
                }
                let selectors_used = rb!(15);
                if selectors_used == 0 {
                    return None;
                }
                let mut selectors = vec![0u8; selectors_used as usize];
                let mut sel_mtf = [0u8, 1, 2, 3, 4, 5];
                for s in selectors.iter_mut() {
                    let v = selector_dec.decode(|| bits.bits8(&mut input, 1))? as u8;
                    let item = un_mtf(v, &mut sel_mtf);
                    if u32::from(item) >= groups {
                        return None;
                    }
                    *s = item;
                }
                let mut decoders = Vec::with_capacity(groups as usize);
                for _ in 0..groups {
                    let mut lengths = vec![0u8; num_items as usize];
                    let mut current = rb!(5);
                    for l in lengths.iter_mut() {
                        loop {
                            let delta = delta_dec.decode(|| bits.bits8(&mut input, 1))?;
                            current = current.wrapping_add(delta);
                            if delta == 0 {
                                break;
                            }
                        }
                        if !(1..=20).contains(&current) {
                            return None;
                        }
                        *l = current as u8;
                    }
                    decoders.push(Huffman::orderly(&lengths)?);
                }

                let mut data_mtf = [0u8; 256];
                for (i, m) in data_mtf.iter_mut().enumerate().take(num_items as usize - 2) {
                    *m = i as u8;
                }
                let mut run_length = 0u32;
                let mut rle_weight = 1u32;
                let mut huffman_index = 0usize;
                let mut decoder = &decoders[0];
                macro_rules! decode_rle {
                    () => {
                        if run_length != 0 {
                            if block_len.checked_add(run_length as usize)? > block_size {
                                return None;
                            }
                            let v = values[data_mtf[0] as usize];
                            tmp[block_len..block_len + run_length as usize].fill(v);
                            block_len += run_length as usize;
                        }
                        run_length = 0;
                        rle_weight = 1;
                    };
                }
                let mut stream_index = 0u32;
                loop {
                    if stream_index % 50 == 0 {
                        if huffman_index >= selectors.len() {
                            return None;
                        }
                        decoder = &decoders[selectors[huffman_index] as usize];
                        huffman_index += 1;
                    }
                    stream_index = stream_index.wrapping_add(1);
                    let symbol_mtf = decoder.decode(|| bits.bits8(&mut input, 1))?;
                    // The stop marker comes once, last, so it needs no un-MTF for detection.
                    if symbol_mtf == num_items - 1 {
                        break;
                    }
                    if block_len >= block_size {
                        return None;
                    }
                    if symbol_mtf < 2 {
                        run_length = run_length.wrapping_add(rle_weight.wrapping_shl(symbol_mtf));
                        rle_weight = rle_weight.wrapping_shl(1);
                    } else {
                        decode_rle!();
                        let symbol = un_mtf((symbol_mtf - 1) as u8, &mut data_mtf);
                        if block_len >= block_size {
                            return None;
                        }
                        tmp[block_len] = values[symbol as usize];
                        block_len += 1;
                    }
                }
                decode_rle!();
                debug_assert!(run_length == 0 && rle_weight == 1);
                if current_ptr as usize >= block_len {
                    return None;
                }
            }

            // Inverse BWT and the final run length decoding.
            let block = &tmp[..block_len];
            let mut rank = [0u32; 256];
            for &b in block {
                rank[b as usize] += 1;
            }
            let mut tot = 0u32;
            for r in rank.iter_mut() {
                let s = *r;
                *r = tot;
                tot += s;
            }
            let mut forward = vec![0u32; block_len];
            for (i, &b) in block.iter().enumerate() {
                forward[rank[b as usize] as usize] = i as u32;
                rank[b as usize] += 1;
            }
            let start = pos;
            let mut st = Rle {
                randomized,
                random_pos: 1,
                random_counter: u32::from(RANDOM_TABLE[0]).wrapping_sub(1),
                ch: 0,
                count: 0,
                pos,
            };
            for _ in 0..block_len {
                current_ptr = forward[current_ptr as usize];
                st.byte(block[current_ptr as usize], out)?;
            }
            // Flush the state, the C++ way.
            if st.count != 0 {
                let c = if st.count == 4 { 0 } else { !st.ch };
                st.byte(c, out)?;
            }
            pos = st.pos;
            crc = crc.rotate_left(1) ^ crc32_rev(&out[start..pos], 0)?;
        } else if hi == 0x1772_4538 && lo == 0x5090 {
            if rb!(32) != crc {
                return None;
            }
            break;
        } else {
            return None;
        }
    }
    Some(())
}

/// The randomisation table of bzip2 before 0.9.5 (each entry minus one), from the bzip2
/// reference via ancient.
const RANDOM_TABLE: [u16; 512] = [
    618, 719, 126, 480, 930, 815, 812, 232, 565, 246, 984, 723, 204, 453, 862, 490,
    740, 241, 948, 213, 732, 858, 334, 707, 620, 573, 72, 653, 729, 471, 418, 435,
    277, 495, 866, 209, 398, 679, 479, 50, 877, 464, 810, 168, 868, 674, 610, 696,
    866, 560, 861, 686, 506, 282, 481, 128, 806, 590, 732, 622, 149, 237, 58, 378,
    683, 876, 624, 168, 642, 104, 169, 606, 519, 931, 726, 475, 692, 424, 173, 646,
    72, 121, 334, 529, 441, 852, 694, 248, 444, 514, 908, 544, 702, 918, 873, 473,
    881, 499, 593, 611, 640, 800, 219, 161, 818, 983, 588, 512, 494, 798, 160, 603,
    957, 532, 220, 399, 385, 866, 599, 781, 381, 595, 413, 170, 515, 374, 681, 484,
    910, 275, 97, 552, 162, 353, 665, 932, 423, 340, 532, 869, 226, 729, 474, 185,
    262, 646, 536, 685, 599, 223, 468, 67, 769, 918, 189, 372, 293, 821, 807, 205,
    183, 942, 794, 383, 382, 460, 403, 757, 838, 886, 714, 66, 617, 275, 203, 917,
    872, 776, 603, 559, 950, 159, 577, 721, 78, 803, 95, 408, 712, 939, 651, 933,
    969, 446, 317, 352, 858, 671, 111, 784, 644, 862, 802, 349, 138, 92, 353, 98,
    819, 907, 608, 771, 153, 273, 579, 183, 78, 625, 629, 741, 652, 281, 761, 622,
    679, 80, 926, 625, 788, 124, 410, 520, 937, 299, 820, 77, 342, 174, 127, 249,
    169, 773, 971, 274, 998, 638, 494, 77, 351, 125, 856, 955, 357, 618, 579, 123,
    736, 593, 700, 611, 668, 111, 133, 693, 362, 991, 808, 742, 167, 973, 943, 374,
    747, 51, 599, 746, 641, 181, 861, 80, 343, 804, 987, 738, 510, 654, 813, 333,
    248, 514, 896, 954, 663, 980, 648, 112, 973, 458, 892, 227, 432, 836, 552, 267,
    925, 239, 101, 653, 458, 50, 685, 753, 805, 759, 492, 402, 414, 393, 686, 699,
    945, 669, 655, 609, 737, 391, 759, 798, 886, 652, 977, 320, 575, 616, 625, 501,
    893, 678, 242, 439, 679, 878, 193, 571, 639, 723, 925, 55, 203, 699, 706, 150,
    456, 448, 796, 194, 790, 557, 944, 678, 296, 58, 86, 823, 712, 662, 411, 692,
    341, 605, 133, 107, 570, 363, 630, 211, 173, 642, 303, 328, 342, 96, 429, 750,
    496, 313, 982, 373, 821, 927, 139, 205, 72, 262, 979, 735, 875, 477, 429, 304,
    169, 513, 363, 691, 828, 81, 854, 952, 675, 245, 368, 969, 293, 749, 806, 826,
    149, 789, 287, 922, 803, 377, 214, 827, 591, 280, 564, 554, 709, 81, 895, 830,
    546, 260, 523, 461, 292, 464, 501, 55, 660, 820, 975, 990, 657, 868, 904, 757,
    744, 192, 767, 549, 607, 932, 377, 285, 214, 978, 791, 960, 60, 687, 792, 643,
    985, 402, 105, 365, 904, 643, 371, 566, 465, 433, 644, 209, 388, 549, 918, 134,
    779, 772, 634, 388, 706, 99, 625, 957, 164, 503, 919, 175, 192, 712, 856, 264,
    202, 49, 667, 107, 644, 989, 625, 196, 509, 356, 357, 849, 857, 363, 935, 637,
];
