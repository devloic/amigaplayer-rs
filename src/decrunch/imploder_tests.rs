//! Tests of the Imploder port: literal-only files built here (the packed stream of such a file is
//! the data itself), damaged and truncated data, and, when the scratch corpus is present,
//! ancient's own Imploder test files against `test_C1.raw`, the public domain text they hold.

use super::ancient_tests::{xpk, Rng, ANCIENT};
use super::{decrunch, unpack_identified, Kind, XpkMethod};

/// An Imploder file that stores `raw` as literals only: the stream is read backwards from `end`
/// and written backwards from the end of the output, so the data goes in verbatim, and the first
/// literal run covers the whole output. `hdr` picks the variant, `add` its checksum constant.
fn imp_literals(hdr: &[u8; 4], add: u32, raw: &[u8]) -> Vec<u8> {
    let end = 12 + raw.len() + raw.len() % 2;
    let mut f = Vec::new();
    f.extend_from_slice(hdr);
    f.extend_from_slice(&(raw.len() as u32).to_be_bytes());
    f.extend_from_slice(&(end as u32).to_be_bytes());
    f.extend_from_slice(raw);
    f.resize(end + 12, 0); // (the twelve bytes the stream's start is mapped onto)
    // tail: literal count, a marker byte with bit 7 set, no anchor bits, then the tables
    f.extend_from_slice(&(raw.len() as u32).to_be_bytes());
    f.extend_from_slice(&[0x80, 0]);
    f.extend_from_slice(&[0u8; 16]);
    f.extend_from_slice(&[1u8; 12]);
    assert_eq!(f.len(), end + 0x2e);
    let sum = f.chunks(2).fold(add, |s, w| s.wrapping_add(u32::from(w[0]) << 8 | u32::from(w[1])));
    f.extend_from_slice(&sum.to_be_bytes());
    f
}

const RAW: &[u8] = b"Imploder literal-only test data, fifty-six bytes in all.";

#[test]
fn literal_stream_round_trips() {
    for (hdr, add) in [
        (b"IMP!", 7),
        (b"ATN!", 7),
        (b"EDAM", 7),
        (b"M.H.", 7),
        (b"BDPI", 0x6e8),
        (b"CHFI", 0xfe4),
        (b"RDC9", 0),
    ] {
        for raw in [RAW, &RAW[..20]] {
            let f = imp_literals(hdr, add, raw);
            assert_eq!(super::identify(&f), Some(Kind::Imploder), "{hdr:?}");
            assert_eq!(unpack_identified(&f).unwrap().1.as_deref(), Some(raw), "{hdr:?}");
            assert_eq!(decrunch(f), raw);
        }
    }
}

#[test]
fn checksum_is_checked_except_for_rdc9() {
    let mut f = imp_literals(b"IMP!", 7, RAW);
    let n = f.len();
    f[n - 1] ^= 1;
    assert_eq!(super::identify(&f), None);
    let mut f = imp_literals(b"RDC9", 0, RAW);
    let n = f.len();
    f[n - 1] ^= 1;
    assert_eq!(super::identify(&f), Some(Kind::Imploder));
}

#[test]
fn xpk_impl_round_trips() {
    // XPK-IMPL: the same stream without the checksum
    let f = imp_literals(b"IMP!", 7, RAW);
    let chunk = f[..f.len() - 4].to_vec();
    let x = xpk(b"IMPL", &[(1, chunk, RAW.len())], RAW, 0);
    let got = unpack_identified(&x).unwrap();
    assert_eq!(got.0, Kind::Xpk(XpkMethod::Impl));
    assert_eq!(got.1.as_deref(), Some(RAW));
}

#[test]
fn damaged_data_never_panics_and_stays_bounded() {
    let mut r = Rng(0x1357_9bdf_0246_8ace);
    let base = imp_literals(b"RDC9", 0, RAW);
    for i in 0..base.len() {
        let _ = unpack_identified(&base[..i]);
    }
    for _ in 0..4000 {
        let mut d = base.clone();
        for _ in 0..1 + r.next() % 4 {
            let i = r.next() as usize % d.len();
            d[i] = r.next() as u8;
        }
        if let Some((_, Some(out))) = unpack_identified(&d) {
            assert!(out.len() <= 32 << 20);
        }
    }
}

const FILES: [&str; 8] = [
    "test_C1.imp",
    "test_C1_atn.imp",
    "test_C1_edam.imp",
    "test_C1_mh.imp",
    "test_C1_bdpi.imp",
    "test_C1_chfi.imp",
    "test_C1_rdc9.imp",
    "test_C1_impl.xpkf",
];

fn dir() -> String {
    format!("{ANCIENT}/ancient-src/testing/test_files")
}

/// Every variant and XPK-IMPL unpack to `test_C1.raw`.
#[test]
fn ancient_files_equal_the_original() {
    let Ok(want) = std::fs::read(format!("{}/test_C1.raw", dir())) else { return };
    let mut n = 0;
    for name in FILES {
        let Ok(packed) = std::fs::read(format!("{}/{name}", dir())) else { continue };
        let got = unpack_identified(&packed);
        let kind = if name.ends_with("xpkf") { Kind::Xpk(XpkMethod::Impl) } else { Kind::Imploder };
        assert!(matches!(got, Some((k, Some(_))) if k == kind), "{name}");
        assert!(got.unwrap().1.unwrap() == want, "{name}: bytes differ from test_C1.raw");
        n += 1;
    }
    eprintln!("{n} Imploder test files equal to test_C1.raw");
}

#[test]
fn ancient_files_damaged_never_panic() {
    let mut r = Rng(0xfeed_0000_0000_0007);
    for name in FILES {
        let Ok(packed) = std::fs::read(format!("{}/{name}", dir())) else { continue };
        let step = (packed.len() / 40).max(1);
        for i in (0..packed.len()).step_by(step) {
            let _ = unpack_identified(&packed[..i]);
        }
        for _ in 0..60 {
            let mut d = packed.clone();
            for _ in 0..3 {
                let i = r.next() as usize % d.len();
                d[i] = r.next() as u8;
            }
            let _ = unpack_identified(&d);
        }
    }
}
