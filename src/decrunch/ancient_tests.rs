//! Tests of the ancient ports: small samples built here (an XPK container writer, a bzip2
//! stream made with Python's bz2 module, encoders for the literal codes of CrunchMania and
//! SMPL), damaged and truncated data, and, when the scratch corpus is present, ancient's own
//! test files against the output of the ancient C++ command line tool.

use super::{decrunch, unpack_identified, Kind, XpkMethod};

// ------------------------------------------------------------- XPK container

/// An XPK file with short chunk headers: `(type, data, unpacked size)` per chunk, then the end
/// chunk. `flags` goes to byte 32 (1 long headers is not used here, 2 password).
pub(super) fn xpk(method: &[u8; 4], chunks: &[(u8, Vec<u8>, usize)], raw: &[u8], flags: u8) -> Vec<u8> {
    let mut body = Vec::new();
    let mut put_chunk = |ty: u8, data: &[u8], unpacked: usize| {
        let mut sum = [0u8; 2];
        for (i, &b) in data.iter().enumerate() {
            sum[i & 1] ^= b;
        }
        let mut h = [ty, 0, sum[0], sum[1], 0, 0, 0, 0];
        h[4..6].copy_from_slice(&(data.len() as u16).to_be_bytes());
        h[6..8].copy_from_slice(&(unpacked as u16).to_be_bytes());
        h[1] = h.iter().fold(0, |a, &x| a ^ x);
        body.extend_from_slice(&h);
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
    };
    for (ty, data, unpacked) in chunks {
        put_chunk(*ty, data, *unpacked);
    }
    put_chunk(15, &[], 0);
    let mut f = Vec::new();
    f.extend_from_slice(b"XPKF");
    f.extend_from_slice(&((36 + body.len() - 8) as u32).to_be_bytes());
    f.extend_from_slice(method);
    f.extend_from_slice(&(raw.len() as u32).to_be_bytes());
    let mut first = [0u8; 16];
    let n = raw.len().min(16);
    first[..n].copy_from_slice(&raw[..n]);
    f.extend_from_slice(&first);
    f.extend_from_slice(&[flags, 0, 0, 0]);
    f[33] = f.iter().fold(0, |a, &x| a ^ x);
    f.extend_from_slice(&body);
    f
}

const BZP2_RAW: &[u8] = b"Amiga XPK BZP2 test. Amiga XPK BZP2 test. Amiga XPK BZP2 test. \
Amiga XPK BZP2 test. The end.";

/// `bz2.compress(BZP2_RAW, 9)` from Python.
fn bzp2_stream() -> Vec<u8> {
    let hex = "425a68393141592653592a891e4d00000d1f80400110003008445026e30c002000405524341a64620a\
               6134d01a624a264a85667c5a86fcc46858b1ac2a7866fb50c18c992f0f8e0fc5dc914e14240aa2479340";
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

fn bzp2_file() -> (Vec<u8>, Vec<u8>) {
    let raw = BZP2_RAW.to_vec();
    (xpk(b"BZP2", &[(1, bzp2_stream(), raw.len())], &raw, 0), raw)
}

/// MSB-first bits to bytes, zero padded.
fn msb_bytes(bits: &[bool]) -> Vec<u8> {
    bits.chunks(8)
        .map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (u8::from(b) << (7 - i))))
        .collect()
}

fn push_msb(bits: &mut Vec<bool>, v: u32, n: u32) {
    for i in (0..n).rev() {
        bits.push(v >> i & 1 == 1);
    }
}

/// SMPL with a two-symbol code (delta 0 is `0`, delta 1 is `1`).
fn smpl_file() -> (Vec<u8>, Vec<u8>) {
    let raw = vec![0u8, 1, 2, 3, 3, 3, 4, 5, 5, 6];
    let mut bits = Vec::new();
    for i in 0..256 {
        if i < 2 {
            push_msb(&mut bits, 1, 4);
            push_msb(&mut bits, i, 1);
        } else {
            push_msb(&mut bits, 0, 4);
        }
    }
    let mut prev = 0u8;
    for &b in &raw {
        push_msb(&mut bits, u32::from(b.wrapping_sub(prev)), 1);
        prev = b;
    }
    let mut chunk = vec![0, 1];
    chunk.extend(msb_bytes(&bits));
    (xpk(b"SMPL", &[(1, chunk, raw.len())], &raw, 0), raw)
}

/// Two chunks: one stored, one SMPL.
fn xpk_stored_and_smpl() -> (Vec<u8>, Vec<u8>) {
    let (smpl, mut raw) = smpl_file();
    let stored = b"stored bytes".to_vec();
    // The SMPL chunk of `smpl_file`: after the 36-byte header and its own 8-byte header.
    let n = u16::from_be_bytes([smpl[40], smpl[41]]) as usize;
    let chunk = smpl[44..44 + n].to_vec();
    let mut all = stored.clone();
    all.append(&mut raw);
    let f = xpk(b"SMPL", &[(0, stored.clone(), stored.len()), (1, chunk, all.len() - stored.len())], &all, 0);
    (f, all)
}

#[test]
fn xpk_bzp2_smpl_and_stored_chunks() {
    for (f, want, m) in [
        (bzp2_file().0, bzp2_file().1, XpkMethod::Bzp2),
        (smpl_file().0, smpl_file().1, XpkMethod::Smpl),
        (xpk_stored_and_smpl().0, xpk_stored_and_smpl().1, XpkMethod::Smpl),
    ] {
        assert_eq!(unpack_identified(&f), Some((Kind::Xpk(m), Some(want.clone()))));
        assert_eq!(decrunch(f), want);
    }
}

#[test]
fn xpk_checks_and_refusals() {
    let (f, _) = bzp2_file();
    // Header checksum.
    let mut d = f.clone();
    d[20] ^= 1;
    assert_eq!(unpack_identified(&d), Some((Kind::Xpk(XpkMethod::Bzp2), None)));
    // Chunk checksum.
    let mut d = f.clone();
    d[50] ^= 1;
    assert_eq!(unpack_identified(&d), Some((Kind::Xpk(XpkMethod::Bzp2), None)));
    // Password: recognised, then refused.
    let raw = BZP2_RAW.to_vec();
    let d = xpk(b"BZP2", &[(1, bzp2_stream(), raw.len())], &raw, 2);
    assert_eq!(unpack_identified(&d), Some((Kind::Xpk(XpkMethod::Bzp2), None)));
    assert_eq!(decrunch(d.clone()), d);
    // A method LibAncient does not have: not recognised, passed through.
    let d = xpk(b"FAST", &[(1, bzp2_stream(), raw.len())], &raw, 0);
    assert_eq!(unpack_identified(&d), None);
    assert_eq!(decrunch(d.clone()), d);
    // SQSH still goes to the libxmp port.
    let mut d = f.clone();
    d[8..12].copy_from_slice(b"SQSH");
    assert_eq!(super::identify(&d), Some(Kind::XpkSqsh));
}

// -------------------------------------------------------------- CrunchMania

/// A CrunchMania file of literals only. The stream is read backwards, LSB first: the 32 bits
/// of the long before the trailing shift word come first (shift 16 keeps all of them), then
/// the bytes before it, downwards.
fn crm_file(magic: &[u8; 4], raw: &[u8]) -> Vec<u8> {
    let sampled = magic[2] == b'm';
    let mut stored = raw.to_vec();
    if sampled {
        let mut prev = 0u8;
        for b in stored.iter_mut() {
            let v = *b;
            *b = v.wrapping_sub(prev);
            prev = v;
        }
    }
    // Output is written from the end, so the last byte is decoded first.
    let mut bits = Vec::new();
    for &b in stored.iter().rev() {
        bits.push(true);
        for i in 0..8 {
            bits.push(b >> i & 1 == 1);
        }
    }
    while bits.len() < 32 || (bits.len() - 32) % 8 != 0 {
        bits.push(false);
    }
    let word = bits[..32].iter().enumerate().fold(0u32, |a, (i, &b)| a | (u32::from(b) << i));
    let tail: Vec<u8> = bits[32..]
        .chunks(8)
        .map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (u8::from(b) << i)))
        .collect();
    let mut stream: Vec<u8> = tail.into_iter().rev().collect();
    stream.extend_from_slice(&word.to_be_bytes());
    stream.extend_from_slice(&16u16.to_be_bytes());
    let mut f = magic.to_vec();
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(&(raw.len() as u32).to_be_bytes());
    f.extend_from_slice(&(stream.len() as u32).to_be_bytes());
    f.extend_from_slice(&stream);
    f
}

#[test]
fn crunchmania_literals_plain_and_sampled() {
    let raw: Vec<u8> = b"CrunchMania literal test, with some bytes".to_vec();
    for magic in [b"CrM!", b"Crm!", b"DCS!"] {
        let f = crm_file(magic, &raw);
        assert_eq!(unpack_identified(&f), Some((Kind::CrunchMania, Some(raw.clone()))), "{:?}", magic);
    }
    // Too short a stream for its unpacked size.
    let mut f = crm_file(b"CrM!", &raw);
    f[9] += 1;
    assert_eq!(unpack_identified(&f), Some((Kind::CrunchMania, None)));
}

#[test]
fn powerplayer_header_checks() {
    let mut f = b"SFCD".to_vec();
    f.extend_from_slice(&100u32.to_be_bytes());
    f.extend_from_slice(&4u32.to_be_bytes());
    f.extend_from_slice(&[0xff; 4]);
    assert_eq!(super::identify(&f), Some(Kind::PowerPlayer));
    // A packed size past the end of the file: not PowerPlayer.
    f[11] = 5;
    assert_eq!(super::identify(&f), None);
}

// ------------------------------------------------------- damaged data

pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

fn shake(s: &[u8], r: &mut Rng, per_byte: usize, random: usize) {
    for i in 0..s.len() {
        for _ in 0..per_byte {
            let mut d = s.to_vec();
            d[i] = r.next() as u8;
            let _ = unpack_identified(&d);
        }
        let _ = unpack_identified(&s[..i]);
    }
    for _ in 0..random {
        let mut d = s.to_vec();
        for _ in 0..4 {
            let i = r.next() as usize % d.len();
            d[i] = r.next() as u8;
        }
        let _ = unpack_identified(&d);
    }
}

#[test]
fn damaged_streams_never_panic() {
    let raw: Vec<u8> = (0..60u8).collect();
    let samples = [
        bzp2_file().0,
        smpl_file().0,
        xpk_stored_and_smpl().0,
        crm_file(b"CrM!", &raw),
        crm_file(b"Crm!", &raw),
    ];
    let mut r = Rng(0x0123_4567_89ab_cdef);
    for s in &samples {
        shake(s, &mut r, 8, 300);
    }
    // Every method id and header magic on random data.
    for magic in [&b"CrM2"[..], b"Crm2", b"Iron", b"MSS!", b"mss!", b"\x18\x05\x19\x73", b"SFHD", b"SFCD"] {
        for _ in 0..200 {
            let mut d: Vec<u8> = (0..(r.next() % 400 + 20)).map(|_| r.next() as u8).collect();
            d[..4].copy_from_slice(magic);
            let n = d.len() as u32;
            d[6..10].copy_from_slice(&(r.next() % 5000 + 1).to_be_bytes());
            d[10..14].copy_from_slice(&(r.next() % (n - 14) + 1).to_be_bytes());
            if magic[0] == b'S' {
                d[4..8].copy_from_slice(&(r.next() % 5000 + 1).to_be_bytes());
                d[8..12].copy_from_slice(&(r.next() % (n - 12)).to_be_bytes());
            }
            let _ = unpack_identified(&d);
        }
    }
    for m in [b"BZP2", b"BLZW", b"LHLB", b"MASH", b"NUKE", b"DUKE", b"RAKE", b"SHRI", b"SMPL"] {
        for _ in 0..200 {
            let len = r.next() as usize % 300 + 4;
            let mut chunk: Vec<u8> = (0..len).map(|_| r.next() as u8).collect();
            match m {
                b"SHRI" => chunk[0] = 1,
                b"SMPL" => chunk[..2].copy_from_slice(&[0, 1]),
                b"BLZW" => chunk[..2].copy_from_slice(&[0, 12]),
                b"BZP2" => chunk[..4].copy_from_slice(b"BZh9"),
                _ => {}
            }
            let raw: Vec<u8> = (0..(r.next() % 3000 + 1)).map(|_| r.next() as u8).collect();
            let f = xpk(m, &[(1, chunk, raw.len())], &raw, 0);
            assert!(matches!(unpack_identified(&f), Some((Kind::Xpk(_), _))));
        }
    }
}

// --------------------------------------- ancient's test files, against its CLI

/// ancient's repository (`testing/test_files`) and the CLI's output for each file (`ref/`,
/// made by `scripts/mkref.sh` there).
pub(super) const ANCIENT: &str = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/port-ancient";

fn ancient_files() -> Vec<(&'static str, Kind)> {
    let mut v = vec![];
    for f in [
        "test_C1.crm",
        "test_C1_18051973.crm",
        "test_C1_cd31.crm",
        "test_C1_dcs.crm",
        "test_C1_delta.crm",
        "test_C1_iron.crm",
        "test_C1_lz.crm",
        "test_C1_lz_delta.crm",
        "test_C1_mss.crm",
        "test_C1_mss_delta.crm",
    ] {
        v.push((f, Kind::CrunchMania));
    }
    v.push(("test_C2.pmc1", Kind::PowerPlayer));
    v.push(("test_C2.pmc2", Kind::PowerPlayer));
    for (f, m) in [
        ("test_C1_bzp2.xpkf", XpkMethod::Bzp2),
        ("test_C1_blzw.xpkf", XpkMethod::Blzw),
        ("test_C1_rake.xpkf", XpkMethod::Rake),
        ("test_C1_smpl.xpkf", XpkMethod::Smpl),
        ("test_C1_shri.xpkf", XpkMethod::Shri),
        ("test_C1_lhlb.xpkf", XpkMethod::Lhlb),
        ("test_C1_mash.xpkf", XpkMethod::Mash),
        ("test_C1_nuke.xpkf", XpkMethod::Nuke),
        ("test_C1_duke.xpkf", XpkMethod::Duke),
    ] {
        v.push((f, Kind::Xpk(m)));
    }
    v
}

fn read_pair(name: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let packed = std::fs::read(format!("{ANCIENT}/ancient-src/testing/test_files/{name}")).ok()?;
    let want = std::fs::read(format!("{ANCIENT}/ref/{name}.out")).ok()?;
    Some((packed, want))
}

#[test]
fn ancient_test_files_equal_the_cli() {
    let mut n = 0;
    for (name, kind) in ancient_files() {
        let Some((packed, want)) = read_pair(name) else { continue };
        let got = unpack_identified(&packed);
        assert!(matches!(got, Some((k, Some(_))) if k == kind), "{name}: {:?}", got.map(|g| (g.0, g.1.map(|v| v.len()))));
        assert!(got.unwrap().1.unwrap() == want, "{name}: bytes differ from the ancient CLI");
        n += 1;
    }
    eprintln!("{n} ancient test files equal to the CLI output");
}

#[test]
fn ancient_test_files_damaged_never_panic() {
    let mut r = Rng(0x5eed_0000_0000_0001);
    // ANCIENT_FUZZ_ROUNDS raises the number of damaged copies per file for a longer run.
    let rounds = std::env::var("ANCIENT_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for (name, _) in ancient_files() {
        let Some((packed, _)) = read_pair(name) else { continue };
        // Truncations at a spread of lengths and random byte changes.
        let step = (packed.len() / 40).max(1);
        for i in (0..packed.len()).step_by(step) {
            let _ = unpack_identified(&packed[..i]);
        }
        for _ in 0..rounds {
            let mut d = packed.clone();
            for _ in 0..3 {
                let i = r.next() as usize % d.len();
                d[i] = r.next() as u8;
            }
            let _ = unpack_identified(&d);
        }
    }
}
