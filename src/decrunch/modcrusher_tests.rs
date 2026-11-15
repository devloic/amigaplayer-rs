//! Tests of the modCRUSHER decruncher: small vectors made with the tool itself (module-shaped
//! files, synthetic samples on a corpus header, all five modes; `fixtures/modcrusher/*.okm` is
//! the tool's packed file, `.expected` its own unpack output), and a stream built here that
//! runs past the 32451-symbol point where the tool freezes its adaptive model.

use super::modcrusher::{decode, Error, Tree};
use super::{decrunch, unpack_identified, Kind};

macro_rules! vector {
    ($name:literal) => {
        (
            include_bytes!(concat!("fixtures/modcrusher/", $name, ".okm")).as_slice(),
            include_bytes!(concat!("fixtures/modcrusher/", $name, ".expected")).as_slice(),
        )
    };
}

const VECTORS: [(&str, (&[u8], &[u8])); 5] = [
    ("synmod_square_m0 (Okm0)", vector!("synmod_square_m0")),
    ("synmod_sine_m1 (OkmC mode 1)", vector!("synmod_sine_m1")),
    ("synmod_noise_m3 (OkmC mode 3)", vector!("synmod_noise_m3")),
    ("synmod_zeros_m4 (OkmC mode 4)", vector!("synmod_zeros_m4")),
    ("edge_trunc101_m0 (Okm0, truncated module)", vector!("edge_trunc101_m0")),
];

#[test]
fn vectors_decode_byte_identical() {
    for (name, (packed, expected)) in VECTORS {
        let got = decode(packed).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(got == expected, "{name}: output differs from the tool's unpack");
    }
}

#[test]
fn vectors_through_the_decrunch_hook() {
    for (name, (packed, expected)) in VECTORS {
        assert_eq!(super::identify(packed), Some(Kind::ModCrusher), "{name}");
        let (kind, out) = unpack_identified(packed).unwrap();
        assert_eq!(kind, Kind::ModCrusher);
        assert!(out.as_deref() == Some(expected), "{name}");
        assert!(decrunch(packed.to_vec()) == expected, "{name}");
    }
}

#[test]
fn truncated_stream_is_an_error() {
    let (packed, _) = VECTORS[0].1;
    assert_eq!(decode(&packed[..packed.len() / 2]), Err(Error::Corrupt));
    // a header only
    assert!(!super::modcrusher::test(&packed[..16]));
}

#[test]
fn lossy_header_inconsistency_is_not_recognised() {
    let (packed, _) = VECTORS[1].1;
    let mut f = packed.to_vec();
    f[8..12].copy_from_slice(&u32::MAX.to_be_bytes()); // smp_off beyond total
    assert!(!super::modcrusher::test(&f));
    assert_eq!(decode(&f), Err(Error::Corrupt));
}

#[test]
fn unrelated_data_is_left_alone() {
    let d = b"Okm1 not a modCRUSHER file at all, just some text".to_vec();
    assert!(super::identify(&d).is_none());
    assert_eq!(decrunch(d.clone()), d);
}

/// One literal 0, then `n` matches of length 60 at distance 1 (class 0, raw bits 000001),
/// coded with the same adaptive model, behind an `Okm0` header claiming `total` bytes.
fn run_stream(n: usize, total: usize) -> Vec<u8> {
    let mut t = Tree::new();
    let mut bits: Vec<bool> = Vec::new();
    for sym in std::iter::once(0usize).chain(std::iter::repeat_n(255 + 60, n)) {
        bits.extend(t.encode(sym).expect("encoder"));
        if sym > 255 {
            bits.extend([false, false, false, false, false, false, false, false, true]);
        }
    }
    let mut f = b"Okm0".to_vec();
    f.extend_from_slice(&(total as u32).to_be_bytes());
    f.extend_from_slice(&0u32.to_be_bytes());
    f.extend_from_slice(&(total as u32).to_be_bytes());
    for c in bits.chunks(8) {
        f.push(c.iter().enumerate().fold(0u8, |b, (i, &x)| b | (u8::from(x) << (7 - i))));
    }
    f.extend_from_slice(&[0; 4]);
    f
}

#[test]
fn stream_within_the_limit_decodes() {
    // 32450 matches + the literal = 32451 symbols: the last one the model can code
    let total = 1 + 60 * 32450;
    let f = run_stream(32450, total);
    let out = decode(&f).unwrap();
    assert_eq!(out.len(), total);
    assert!(out.iter().all(|&b| b == 0));
}

#[test]
fn stream_past_the_limit_decodes_with_the_model_frozen() {
    // 32451 matches + the literal = 32452 symbols: the last one is coded on the frozen tree
    let total = 1 + 60 * 32451;
    let f = run_stream(32451, total);
    let out = decode(&f).unwrap();
    assert_eq!(out.len(), total);
    assert!(out.iter().all(|&b| b == 0));
}

/// The module `synth_freeze_m0.okm` was made from: a synthetic M.K. header and pattern
/// (no third-party data), a 1984000-byte sample of 1980000 zero bytes, then 4000 bytes of
/// xorshift32 (seed 0x12345678, top byte of each step).
fn synth_freeze_module() -> Vec<u8> {
    const Z: usize = 1_980_000;
    const T: usize = 4000;
    let n = Z + T;
    let mut h = vec![0u8; 1084];
    h[0..11].copy_from_slice(b"freeze test");
    h[20..26].copy_from_slice(b"sample");
    h[42..44].copy_from_slice(&(((n / 2) & 0xFFFF) as u16).to_be_bytes());
    h[45] = 64;
    h[48..50].copy_from_slice(&1u16.to_be_bytes());
    for i in 1..31 {
        let o = 20 + 30 * i + 28;
        h[o..o + 2].copy_from_slice(&1u16.to_be_bytes());
    }
    h[950] = 1;
    h[951] = 127;
    h[1080..1084].copy_from_slice(b"M.K.");
    let mut pat = vec![0u8; 1024];
    pat[0..4].copy_from_slice(&[0x01, 0xAC, 0x10, 0x00]);
    h.extend_from_slice(&pat);
    h.resize(h.len() + Z, 0);
    let mut x: u32 = 0x1234_5678;
    for _ in 0..T {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        h.push((x >> 24) as u8);
    }
    h
}

#[test]
fn tool_stream_past_the_limit_is_byte_identical() {
    // packed by the tool itself (mode 0): 37072 coded symbols, the last 4621 on the
    // frozen tree, among them the 4000 xorshift literals
    let packed = include_bytes!("fixtures/modcrusher/synth_freeze_m0.okm");
    let want = synth_freeze_module();
    let got = decode(packed).unwrap();
    assert!(got == want, "output differs from the module the tool packed");
    assert!(decrunch(packed.to_vec()) == want);
}
