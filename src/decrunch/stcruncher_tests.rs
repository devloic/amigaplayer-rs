//! Tests of the SoundTracker Cruncher decruncher. The vectors are synthetic modules made for
//! the clean-room work, packed with the tool itself: `NAME.stc` as the tool saved it (2-4 bytes
//! short of the stream), `NAME.full.stc` with the missing tail recaptured, `NAME.mod` the input.

use super::stcruncher::{decode, Error};
use super::{decrunch, identify, unpack_identified, Kind, MAX_RAW_OUT};

macro_rules! file {
    ($name:literal) => {
        include_bytes!(concat!("fixtures/stcruncher/", $name)).as_slice()
    };
}

/// (name, as saved, full stream, original module, how many trailing bytes the saved file loses)
const VECTORS: [(&str, &[u8], &[u8], &[u8], usize); 3] = [
    ("p01", file!("p01.stc"), file!("p01.full.stc"), file!("p01.mod"), 0),
    ("p02", file!("p02.stc"), file!("p02.full.stc"), file!("p02.mod"), 8),
    ("t1", file!("t1.stc"), file!("t1.full.stc"), file!("t1.mod"), 4),
];

#[test]
fn full_streams_decode_byte_identical() {
    for (name, _, full, orig, _) in VECTORS {
        let got = decode(full).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(got == orig, "{name}: output differs from the input module");
    }
}

#[test]
fn as_saved_files_are_zero_filled() {
    for (name, saved, _, orig, bad) in VECTORS {
        let got = decode(saved).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(got.len(), orig.len(), "{name}");
        if bad == 0 {
            assert!(got == orig, "{name}: should be exact");
        } else {
            // only the cut token's data is lost: everything before the last `bad` bytes is exact
            let k = orig.len() - bad;
            assert!(got[..k] == orig[..k], "{name}: head differs");
            assert!(got[k..] != orig[k..], "{name}: tail unexpectedly exact");
        }
    }
}

#[test]
fn zero_fill_of_a_cut_stream() {
    // 4 raw bytes, then a literal token (00, count code 0 = one byte) whose byte is cut off
    let mut d = b"STC\0\0\0\0\x08".to_vec();
    d.extend_from_slice(&[1, 2, 3, 4, 0x00]);
    assert_eq!(decode(&d).unwrap(), [1, 2, 3, 4, 0, 0, 0, 0]);
}

#[test]
fn through_the_decrunch_hook() {
    for (name, saved, full, orig, _) in VECTORS {
        for d in [saved, full] {
            assert_eq!(identify(d), Some(Kind::StCruncher), "{name}");
            let (kind, out) = unpack_identified(d).unwrap();
            assert_eq!(kind, Kind::StCruncher);
            assert_eq!(out.unwrap().len(), orig.len(), "{name}");
        }
        assert!(decrunch(full.to_vec()) == orig, "{name}");
    }
}

#[test]
fn count_codes_56_to_63_are_rejected() {
    for n in 56u8..64 {
        // 4 raw bytes, then `00` + a 6-bit count code n
        let mut d = b"STC\0\0\0\0\x20".to_vec();
        d.extend_from_slice(&[1, 2, 3, 4, n, 0]);
        assert_eq!(decode(&d), Err(Error::Corrupt), "code {n}");
        // the same for the repeat token
        let mut d = b"STC\0\0\0\0\x20".to_vec();
        d.extend_from_slice(&[1, 2, 3, 4, 0x40 | n, 0]);
        assert_eq!(decode(&d), Err(Error::Corrupt), "repeat code {n}");
    }
    // a rejected stream is left as it is by the hook
    let mut d = b"STC\0\0\0\0\x20".to_vec();
    d.extend_from_slice(&[1, 2, 3, 4, 56, 0]);
    assert_eq!(unpack_identified(&d).unwrap().1, None);
    assert_eq!(decrunch(d.clone()), d);
}

#[test]
fn oversized_or_empty_length_is_not_an_stc_file() {
    let mut d = b"STC\0".to_vec();
    d.extend_from_slice(&((MAX_RAW_OUT as u32) + 1).to_be_bytes());
    d.extend_from_slice(&[0; 16]);
    assert_eq!(identify(&d), None);
    assert_eq!(decode(&d), Err(Error::Corrupt));
    let mut d = b"STC\0\0\0\0\0".to_vec();
    d.extend_from_slice(&[0; 16]);
    assert_eq!(identify(&d), None);
    // a huge count cannot push the output past the declared length: zero run (1011), code 55
    // (110111) and a 32-bit count of 0xFFFFFFFF
    let mut d = b"STC\0\0\0\0\x20".to_vec();
    d.extend_from_slice(&[1, 2, 3, 4, 0b1011_1101, 0b1111_1111, 0xFF, 0xFF, 0xFF, 0xF0]);
    let got = decode(&d).unwrap();
    assert_eq!(got.len(), 0x20);
    assert_eq!(&got[..4], &[1, 2, 3, 4]);
}

#[test]
fn damaged_input_never_panics() {
    for (_, _, full, _, _) in VECTORS {
        for cut in 0..full.len() {
            let _ = decode(&full[..cut]);
        }
        let mut seed = 0x1234_5678u32;
        for _ in 0..400 {
            let mut d = full.to_vec();
            for _ in 0..3 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let i = 8 + (seed >> 8) as usize % (d.len() - 8);
                d[i] ^= 1 << (seed % 8);
            }
            let _ = decode(&d);
            let _ = decrunch(d);
        }
    }
}
