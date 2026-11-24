use super::*;

/// A module with FM instrument 0 (a plain sine pair) and, optionally, a looped PCM sample 0;
/// every pattern is `pat` (note stream bytes).
fn build(speed: u8, orders: &[u8], pat: &[u8], pcm: Option<&[u8]>) -> Vec<u8> {
    let mut d = vec![0u8; PATTERN_TABLE];
    d[0] = speed;
    // FM instrument 0 (byte 0 feedback/connection, then modulator, carrier)
    let f = FM_INSTR;
    d[f..f + 11].copy_from_slice(&[0x00, 0x21, 0x10, 0xF2, 0x77, 0x00, 0x21, 0x00, 0xF2, 0x77, 0x00]);
    if let Some(p) = pcm {
        d[SAMPLE_HDRS + 4..SAMPLE_HDRS + 8].copy_from_slice(&(p.len() as u32).to_le_bytes());
        d[SAMPLE_HDRS + 12..SAMPLE_HDRS + 16].copy_from_slice(&0xFFFF_FF00u32.to_le_bytes());
    }
    for s in 1..32 {
        d[SAMPLE_HDRS + 16 * s + 12..SAMPLE_HDRS + 16 * s + 16].copy_from_slice(&0xFFFF_FF00u32.to_le_bytes());
    }
    d[ORDERS..ORDERS + 256].fill(0xFF);
    d[ORDERS..ORDERS + orders.len()].copy_from_slice(orders);
    for p in 0..128 {
        d[HEADER + 512 + 4 * p..HEADER + 512 + 4 * p + 4].copy_from_slice(&(pat.len() as u32).to_le_bytes());
    }
    d.extend_from_slice(pat);
    if let Some(p) = pcm {
        d.extend_from_slice(p);
    }
    d
}

/// FM note C-5 (octave 4) on pattern channel 4 at volume 15, then 4 rows, then the end.
const FM_PAT: &[u8] = &[0x04, 0x40, 0x0F, 0x40, 0x04, 0x60];

#[test]
fn detects_and_plays_fm() {
    let d = build(6, &[0], FM_PAT, None);
    assert!(test(&d));
    let mut p = C67::new(&d, 48000).unwrap();
    // 4 rows at speed 6, tick 2.5 / 143 s
    assert_eq!(p.duration_ms(), (4.0 * 6.0 * 839.0 / 48.0f64).round() as i32);
    let mut buf = vec![0i16; 2 * 48000];
    let n = p.render(&mut buf);
    assert!(p.ended());
    assert!((n as i64 - 4 * 6 * 839).abs() <= 2, "{n}");
    let peak = buf[..n * 2].iter().map(|v| v.unsigned_abs()).max().unwrap();
    assert!(peak > 500, "peak {peak}");
    assert_eq!(p.format(), "CDFM (Composer 670)");
}

#[test]
fn plays_a_pcm_sample() {
    let sq: Vec<u8> = (0..64).map(|i| if i < 32 { 0xFF } else { 0x01 }).collect();
    // octave 4 on channel 0, instrument 0, volume 15
    let d = build(3, &[0], &[0x00, 0x40, 0x0F, 0x40, 0x08, 0x60], Some(&sq));
    let mut p = C67::new(&d, 48000).unwrap();
    let mut buf = vec![0i16; 2 * 8000];
    let n = p.render(&mut buf);
    assert!(n > 1000);
    let (l, r): (i64, i64) =
        buf[..n * 2].chunks(2).fold((0, 0), |a, f| (a.0 + f[0].unsigned_abs() as i64, a.1 + f[1].unsigned_abs() as i64));
    assert!(l > 0 && l > 2 * r, "channel 0 is panned left: {l} {r}");
}

#[test]
fn beating_detunes_by_channel() {
    let mut a = Opl::new(48000, 13);
    let mut b = Opl::new(48000, 13);
    let patch: OplPatch = [0x21, 0x21, 0x10, 0x00, 0xF2, 0xF2, 0x77, 0x77, 0, 0, 0, 0];
    a.patch(5, &patch);
    b.patch(5, &patch);
    a.pan(5, 128);
    b.pan(5, 128);
    a.frequency_beating(5, 261_625, false, false);
    b.frequency_beating(5, 261_625, false, true);
    let wa: Vec<i16> = (0..2000).map(|_| a.sample().0).collect();
    let wb: Vec<i16> = (0..2000).map(|_| b.sample().0).collect();
    assert_ne!(wa, wb);
}

#[test]
fn refuses_what_is_not_c67() {
    assert!(!test(&[]));
    assert!(!test(&vec![0u8; 5000]));
    let mut d = build(6, &[0], FM_PAT, None);
    d[0] = 16; // speed out of range
    assert!(!test(&d));
    let mut d = build(6, &[0], FM_PAT, None);
    d[ORDERS] = 200; // order out of range
    assert!(!test(&d));
    let d = build(6, &[0], &[0x50, 0x60, 0x00], None);
    assert!(!test(&d), "unknown pattern command");
    assert!(!test(&build(6, &[0], FM_PAT, None)[..2980]), "pattern data cut short");
}

/// No file of the list in `C67_LIST` (one path per line) but a `.c67` is claimed.
#[test]
#[ignore]
fn claims_none_of_a_list() {
    let list = std::fs::read_to_string(std::env::var("C67_LIST").unwrap()).unwrap();
    let (mut seen, mut hits) = (0, Vec::new());
    for p in list.lines() {
        if let Ok(d) = std::fs::read(p) {
            seen += 1;
            if test(&d) {
                hits.push(p.to_string());
            }
        }
    }
    eprintln!("read {seen} files, claimed {}: {hits:?}", hits.len());
    assert!(hits.iter().all(|h| h.ends_with(".c67")), "claimed files that are not .c67");
}

/// The module (set C67_TEST_FILE), writing raw 16-bit stereo to C67_OUT when set.
#[test]
#[ignore]
fn plays_the_real_file() {
    let f = std::env::var("C67_TEST_FILE").unwrap();
    let d = std::fs::read(f).unwrap();
    let mut p = C67::new(&d, 48000).unwrap();
    eprintln!("duration {} ms", p.duration_ms());
    let mut out = Vec::new();
    let mut buf = vec![0i16; 2 * 4800];
    loop {
        let n = p.render(&mut buf);
        out.extend_from_slice(&buf[..n * 2]);
        if n == 0 {
            break;
        }
    }
    if let Ok(w) = std::env::var("C67_OUT") {
        let mut v = Vec::new();
        for s in &out {
            v.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(w, v).unwrap();
    }
    eprintln!("frames {}", out.len() / 2);
}
