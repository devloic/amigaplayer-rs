//! Tests of the X-Tracker port on modules built here (no downloaded file is needed).

use super::*;

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
}

/// A version 8 module: `tracks` tracks, one pattern of `rows` rows whose track data is
/// `data` (global track byte first on each row, as X-Tracker packs it), the order list
/// `order`, and one sample.
fn build(tracks: u8, rows: u16, data: &[u8], order: &[u16], sample: &[u8], flags: u8, loop_: (u32, u32)) -> Vec<u8> {
    let mut d = b"DDMF".to_vec();
    d.push(8);
    d.extend_from_slice(b"XTRACKER");
    let mut name = [b' '; 30];
    name[..4].copy_from_slice(b"test");
    d.extend_from_slice(&name);
    let mut comp = [b' '; 20];
    comp[..2].copy_from_slice(b"me");
    d.extend_from_slice(&comp);
    d.extend_from_slice(&[1, 1, 96]);
    let mut seq = Vec::new();
    seq.extend_from_slice(&0u16.to_le_bytes()); // loop start
    seq.extend_from_slice(&((order.len() as u16).saturating_sub(1)).to_le_bytes()); // loop end
    for o in order {
        seq.extend_from_slice(&o.to_le_bytes());
    }
    chunk(&mut d, b"SEQU", &seq);
    let mut patt = Vec::new();
    patt.extend_from_slice(&1u16.to_le_bytes());
    patt.push(tracks);
    patt.push(tracks); // pattern header: tracks, beat, rows, length
    patt.push(0);
    patt.extend_from_slice(&rows.to_le_bytes());
    patt.extend_from_slice(&(data.len() as u32).to_le_bytes());
    patt.extend_from_slice(data);
    chunk(&mut d, b"PATT", &patt);
    let mut smpi = vec![1u8, 1, b'a'];
    smpi.extend_from_slice(&(sample.len() as u32).to_le_bytes());
    smpi.extend_from_slice(&loop_.0.to_le_bytes());
    smpi.extend_from_slice(&loop_.1.to_le_bytes());
    smpi.extend_from_slice(&8363u16.to_le_bytes());
    smpi.push(0); // volume 0: none
    smpi.push(flags);
    smpi.extend_from_slice(&[0; 8]); // library name
    smpi.extend_from_slice(&[0; 6]);
    chunk(&mut d, b"SMPI", &smpi);
    let mut smpd = (sample.len() as u32).to_le_bytes().to_vec();
    smpd.extend_from_slice(sample);
    chunk(&mut d, b"SMPD", &smpd);
    d
}

fn square() -> Vec<u8> {
    (0..2048).map(|i| if i % 64 < 32 { 90u8 } else { (-90i8) as u8 }).collect()
}

/// Two tracks, 16 rows: a note with instrument 1 on track 1 and a volume on track 2's note,
/// then a note cut on row 8.
fn song() -> Vec<u8> {
    let mut data = Vec::new();
    // row 0: global none; track 1: instrument 1, note 49 (C-5 before the +24), volume 200;
    // track 2: instrument 1, note 56, volume 100 (the sample has no volume of its own, and a
    // channel starts at 0)
    data.extend_from_slice(&[0, 0x70, 1, 49, 200, 0x70, 1, 56, 100]);
    // rows 1..7: nothing, packed with counters
    data.extend_from_slice(&[0x80, 6, 0x80, 6, 0x80, 6]);
    // row 8: track 1 note cut (instrument effect 1), track 2 nothing
    data.extend_from_slice(&[0, 0x08, 1, 0, 0]);
    // rows 9..15
    data.extend_from_slice(&[0x80, 6, 0x80, 6, 0x80, 6]);
    build(2, 16, &data, &[0], &square(), 1, (0, 2048))
}

fn render_all(p: &mut Dmf, max_frames: usize) -> Vec<i16> {
    let mut out = Vec::new();
    let mut buf = vec![0i16; 4096];
    while out.len() / 2 < max_frames {
        let n = p.render(&mut buf);
        out.extend_from_slice(&buf[..n * 2]);
        if n < buf.len() / 2 {
            break;
        }
    }
    out
}

fn rms(x: &[i16]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

#[test]
fn plays_a_built_module() {
    let d = song();
    assert!(test(&d));
    let m = load(&d).expect("loads");
    assert_eq!(m.channels, 3);
    assert_eq!(m.title, "test");
    assert_eq!(m.artist, "me");
    assert_eq!(m.tracker, "X-Tracker");
    // row 0: notes +24, the first track's volume moved to the effect column
    let c = &m.patterns[0].cells;
    assert_eq!(c[1].note, 73);
    assert_eq!(c[2].note, 80);
    let mut p = Dmf::new(&d, 48000).unwrap();
    assert_eq!(p.format(), "Delusion Digital Music Format v8 (X-Tracker)");
    // 16 rows at speed 6, tempo 120 (2.5 * 48000 / 120 = 1000 frames a tick): 96000 frames
    assert_eq!(p.duration_ms(), 2000);
    let out = render_all(&mut p, 200_000);
    assert_eq!(out.len() / 2, 96000);
    // both tracks sound in the first half; the cut leaves only track 2 after row 8
    let a = rms(&out[2000..2 * 40000]);
    let b = rms(&out[2 * 56000..2 * 90000]);
    assert!(a > 1000.0, "{a}");
    assert!(b > 300.0 && b < a, "{a} {b}");
    // centred channels: both sides alike
    let l: Vec<i16> = out.iter().step_by(2).copied().collect();
    let r: Vec<i16> = out.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(l, r);
}

#[test]
fn renders_identically_twice() {
    let d = song();
    let a = render_all(&mut Dmf::new(&d, 44100).unwrap(), 1 << 20);
    let mut p = Dmf::new(&d, 44100).unwrap();
    let b = render_all(&mut p, 1 << 20);
    assert_eq!(a, b);
    p.start();
    let c = render_all(&mut p, 1 << 20);
    assert_eq!(a, c);
}

#[test]
fn refuses_what_is_not_a_module() {
    let d = song();
    assert!(load(&d[..60]).is_err());
    let mut bad = d.clone();
    bad[4] = 11; // version past 10
    assert!(!test(&bad));
    let mut bad = d.clone();
    bad[0] = b'X';
    assert!(load(&bad).is_err());
    // no PATT chunk
    let mut nopatt = d[..66].to_vec();
    nopatt.extend_from_slice(b"SEQU\x02\x00\x00\x00\x00\x00");
    assert!(load(&nopatt).is_err());
}

#[test]
fn survives_truncated_and_mangled_files() {
    let d = song();
    for n in (0..d.len()).step_by(3) {
        if let Some(mut p) = Dmf::new(&d[..n], 22050) {
            let _ = p.duration_ms();
            render_all(&mut p, 50_000);
        }
    }
    let mut seed = 0x1234_5678u32;
    for _ in 0..300 {
        let mut m = d.clone();
        for _ in 0..8 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let i = 66 + (seed as usize >> 8) % (m.len() - 66);
            m[i] = (seed >> 24) as u8;
        }
        if let Some(mut p) = Dmf::new(&m, 22050) {
            let _ = p.duration_ms();
            render_all(&mut p, 100_000);
        }
    }
}

#[test]
fn huge_claims_stay_cheap() {
    // a pattern claiming 4096 rows of 32 tracks with no data, ordered 60000 times: the cell
    // budget turns the order entries past it into song ends
    let mut d = b"DDMF".to_vec();
    d.push(8);
    d.extend_from_slice(&[b' '; 61]);
    let mut seq = vec![0u8; 4];
    for _ in 0..60000 {
        seq.extend_from_slice(&0u16.to_le_bytes());
    }
    chunk(&mut d, b"SEQU", &seq);
    let mut patt = vec![1, 0, 32];
    patt.extend_from_slice(&[32, 0]);
    patt.extend_from_slice(&4096u16.to_le_bytes());
    patt.extend_from_slice(&0u32.to_le_bytes());
    chunk(&mut d, b"PATT", &patt);
    let m = load(&d).expect("loads");
    let cells: usize = m.patterns.iter().map(|p| p.cells.len()).sum();
    assert!(cells <= 8 << 20, "{cells}");
    assert!(m.patterns.len() < 100);
    let p = Dmf::new(&d, 8000).unwrap();
    assert!(p.duration_ms() > 0);
}

#[test]
fn compressed_samples_decode() {
    // a tree of one root and two leaves (0 and 1), then 8 codes: sign 0, right (value 1)
    // gives deltas 1, so the output climbs 1, 2, 3 ...
    let mut bits: Vec<u8> = Vec::new();
    let mut push = |v: u32, n: u32| {
        for k in 0..n {
            bits.push(((v >> k) & 1) as u8);
        }
    };
    push(0, 7);
    push(1, 1);
    push(1, 1); // root: value 0, both children
    push(0, 7);
    push(0, 1);
    push(0, 1); // left leaf 0
    push(1, 7);
    push(0, 1);
    push(0, 1); // right leaf 1
    for _ in 0..8 {
        push(0, 1);
        push(1, 1);
    }
    let mut bytes = vec![0u8; bits.len().div_ceil(8)];
    for (i, b) in bits.iter().enumerate() {
        bytes[i / 8] |= b << (i % 8);
    }
    let mut out = vec![0u8; 8];
    load::tests_unpack(&bytes, &mut out);
    assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8]);
}
