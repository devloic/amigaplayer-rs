use super::*;

/// A module of `n` channels with one 64-byte looped sample (loop from byte 16), one pattern with
/// `cells` at (row, channel) and the sample's bytes after it.
fn module(tag: &[u8; 4], n: usize, cells: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
    let mut d = vec![0u8; 1084];
    d[..8].copy_from_slice(b"test mod");
    d[20..26].copy_from_slice(b"sample");
    d[42..44].copy_from_slice(&32u16.to_be_bytes());
    d[45] = 48;
    d[46..48].copy_from_slice(&8u16.to_be_bytes());
    d[48..50].copy_from_slice(&24u16.to_be_bytes());
    d[950] = 1;
    d[1080..1084].copy_from_slice(tag);
    let mut pat = vec![0u8; 256 * n];
    for &(row, ch, c) in cells {
        let o = (row * n + ch) * 4;
        pat[o..o + 4].copy_from_slice(&c);
    }
    d.extend(pat);
    d.extend((0..64u8).map(|i| (i % 32).wrapping_mul(7)));
    d
}

/// Period `p`, sample `s`, effect `e`, parameter `a`.
fn cell(p: u16, s: u8, e: u8, a: u8) -> [u8; 4] {
    [(s & 0xf0) | (p >> 8) as u8, p as u8, (s << 4) | e, a]
}

fn render_all(p: &mut Ft2Player, max_frames: usize) -> Vec<f32> {
    let mut out = vec![0f32; 2 * 4800];
    let mut all = Vec::new();
    while all.len() < max_frames * 2 && !p.ended() {
        let n = p.render(&mut out);
        all.extend_from_slice(&out[..n * 2]);
        if n == 0 {
            break;
        }
    }
    all
}

#[test]
fn takes_the_multichannel_tags() {
    for (tag, n) in [(b"6CHN", 6), (b"8CHN", 8), (b"2CHN", 2), (b"10CH", 10), (b"32CH", 32)] {
        assert!(detect(&module(tag, n, &[])), "{}", String::from_utf8_lossy(tag));
    }
    // the four-channel tags stay with ProTracker, the odd counts and the others are not ft2play's
    for (tag, n) in [(b"M.K.", 4), (b"4CHN", 4), (b"7CHN", 7), (b"11CH", 11), (b"OCTA", 8), (b"FLT8", 8)] {
        assert!(!detect(&module(tag, n, &[])), "{}", String::from_utf8_lossy(tag));
    }
}

#[test]
fn loader_stops_where_ft2plays_memory_file_does() {
    let d = module(b"8CHN", 8, &[]);
    assert!(Module::load(&d).is_some());
    // a file that ends exactly where the last pattern ends (no sample data) fails: the read
    // that takes the last byte sets the end-of-file flag, which the loader checks
    assert!(Module::load(&d[..1084 + 256 * 8]).is_none());
    // as does one that ends within the header
    assert!(Module::load(&d[..1084]).is_none());
    // a sample the file ends within loads, the rest of it zero
    let m = Module::load(&d[..1084 + 256 * 8 + 10]).unwrap();
    let s = m.instr[1].as_ref().unwrap();
    assert_eq!(s.len, 64);
    assert!(s.data[10..].iter().all(|&b| b == 0));
}

#[test]
fn converts_the_notes_and_effects_as_the_loader_does() {
    let d = module(b"8CHN", 8, &[(0, 0, cell(428, 1, 0x1, 0)), (1, 3, cell(0, 0, 0x5, 0)), (2, 1, cell(0, 0, 0xc, 99)), (3, 2, cell(27, 0, 0, 0))]);
    let m = Module::load(&d).unwrap();
    let p = m.patt[0].as_ref().unwrap();
    // 428 is the 49th period of ft2play's table, and a slide of 0 is no effect
    assert_eq!((p[0].ton, p[0].instr, p[0].eff_typ), (49, 1, 0));
    // 5xy with 0 is 3xy, a volume above 64 is 64, a period below the last note's has none
    assert_eq!(p[8 + 3].eff_typ, 3);
    assert_eq!(p[16 + 1].eff, 64);
    assert_eq!(p[24 + 2].ton, 0);
    // the sample: finetune 0, volume 48, loop from 16 bytes for 48
    let s = m.instr[1].as_ref().unwrap();
    assert_eq!((s.len, s.rep_s, s.rep_l, s.vol, s.fine, s.typ), (64, 16, 48, 48, 0, 1));
    // the tap after the loop is the loop's start
    assert_eq!(s.data[64], s.data[16]);
}

#[test]
fn plays_a_note_centred_and_ends_at_the_song_end() {
    let d = module(b"8CHN", 8, &[(0, 0, cell(428, 1, 0, 0))]);
    let mut p = Ft2Player::new(&d, 48000).unwrap();
    assert_eq!(p.subsongs(), (0, 0, 0));
    let all = render_all(&mut p, 48000 * 60);
    assert!(p.ended());
    // 64 rows of 6 ticks of 960 frames, and the second pass of the pattern (what ends it) is not output
    let frames = all.len() / 2;
    assert!(frames >= 64 * 6 * 960 && frames <= 2 * 64 * 6 * 960, "{frames}");
    assert!(all.iter().any(|&s| s != 0.0));
    // every pan is the middle: both sides alike
    assert!(all.chunks_exact(2).all(|f| f[0] == f[1]));
    // output is 16-bit samples as they are
    assert!(all.iter().all(|&s| (s * 32768.0).fract() == 0.0));
}

#[test]
fn a_second_tune_starts_at_its_position() {
    // pattern 0 jumps back to its start at row 5; pattern 1 is a second tune playing a note
    let mut d = module(b"8CHN", 8, &[(5, 0, cell(0, 0, 0xb, 0))]);
    d[950] = 2;
    d[953] = 1;
    let mut pat = vec![0u8; 256 * 8];
    pat[..4].copy_from_slice(&cell(428, 1, 0, 0));
    let at = 1084 + 256 * 8;
    d.splice(at..at, pat);
    let mut p = Ft2Player::new(&d, 48000).unwrap();
    assert_eq!(p.subsongs(), (0, 1, 0));
    let mut buf = vec![0f32; 2 * 960];
    p.render(&mut buf);
    assert!(buf.iter().all(|&s| s == 0.0), "the first tune starts silent");
    p.set_subsong(1);
    assert_eq!(p.subsongs(), (0, 1, 1));
    p.render(&mut buf);
    assert!(buf.iter().any(|&s| s != 0.0), "the second tune plays its note at once");
}

#[test]
fn restart_is_the_same_audio() {
    let d = module(b"6CHN", 6, &[(0, 0, cell(428, 1, 0, 0)), (4, 1, cell(300, 1, 0x4, 0x44)), (8, 2, cell(500, 1, 0x3, 0x10))]);
    let mut p = Ft2Player::new(&d, 44100).unwrap();
    let mut a = vec![0f32; 2 * 20000];
    let mut b = a.clone();
    p.render(&mut a);
    p.set_subsong(0);
    p.render(&mut b);
    assert_eq!(a, b);
}

// ---- the reference: the audacious-uade player's ft2play backend ----

/// MD5 (RFC 1321), for the hashes of the reference's output.
fn md5(data: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16,
        23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend((data.len() as u64 * 8).to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let m: Vec<u32> = chunk.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    [a0, b0, c0, d0].iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

#[test]
fn md5_is_rfc_1321() {
    assert_eq!(md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5(b"The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
}

/// The 16-bit stereo stream of a subsong, as the player writes it (little-endian here).
fn stream(d: &[u8], rate: u32, subsong: u16) -> Vec<u8> {
    let mut p = Ft2Player::new(d, rate).unwrap();
    p.set_subsong(subsong);
    let mut bytes = Vec::new();
    let mut out = vec![0f32; 2 * 4096];
    while !p.ended() {
        let n = p.render(&mut out);
        for &s in &out[..2 * n] {
            bytes.extend(((s * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes());
        }
        if n == 0 {
            break;
        }
    }
    bytes
}

/// FST.NeverendingStory'97 (8CHN) against the audacious-uade player built from the sources
/// ft2play is ported from (`PROBE=1 PLAYER=ft2play player RATE FILE SUBSONG`): the hashes of its
/// stereo output (all pans centred, so the songdb's mono mix, `sox ... remix 1-2`, is its left
/// channel; its md5 at 11025 Hz is the songdb's audioMd5). Set FT2PLAY_FST to the file.
#[test]
#[ignore]
fn neverending_story_is_bit_exact() {
    let path = std::env::var("FT2PLAY_FST").unwrap_or_else(|_| {
        "/media/lolo/kingston2tb/claude-scratch/amiga-acoustide/pilot/modules/914cef9f956c0710913e44e8f0b878be/FST.NeverendingStory'97".into()
    });
    let d = std::fs::read(path).unwrap();
    assert!(detect(&d));
    for (rate, sub, len, hash, mono) in [
        (11025, 0, 5213120, "45a0a8247f50f726768e46e2312a9282", Some("cf8938cefb91173b9fdca9b9770b397e")),
        (11025, 1, 2533520, "20a0ac05d12f3826e6ae28508d1c07ec", Some("d203fb1abc3f2c4591caa25699d17ff6")),
        (48000, 0, 22748160, "abb79cadbd88737be9a9c23398ff6526", None),
        (48000, 1, 11055360, "9b4b2cfc628d49d125a8828f156f0ff3", None),
    ] {
        let s = stream(&d, rate, sub);
        assert_eq!(s.len(), len, "{rate} Hz subsong {sub}: length");
        assert_eq!(md5(&s), hash, "{rate} Hz subsong {sub}: stereo");
        if let Some(m) = mono {
            let left: Vec<u8> = s.chunks_exact(4).flat_map(|f| [f[0], f[1]]).collect();
            assert_eq!(md5(&left), m, "{rate} Hz subsong {sub}: mono");
        }
    }
}
