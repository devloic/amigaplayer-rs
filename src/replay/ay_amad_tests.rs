//! Tests of the ZXAY container types other than EMUL (`ay_amad.rs`) and the register dump used
//! to compare them with the EMUL rips of the same tunes.

use super::amad::{period, Amad, Header};
use super::*;
use crate::aiff::Stream;

const LOAD: u16 = 0xC000;

/// A ZXAYAMAD file with one song: `voices` are the three voices' byte code, `extra` more data
/// (envelopes, vibratos) after them; addresses in the code are absolute (data at LOAD).
fn amad_file(voices: [&[u8]; 3], extra: &[u8], mask: u8, length: u16) -> Vec<u8> {
    let mut d = b"ZXAYAMAD".to_vec();
    d.extend_from_slice(&[0, 0, 0, 0]); // versions, special player
    d.extend_from_slice(&[0, 8]); // author at 12 + 8 = 20
    d.extend_from_slice(&[0, 6]); // misc at 14 + 6 = 20
    d.extend_from_slice(&[0, 0]); // one song, the first
    d.extend_from_slice(&[0, 4]); // songs structure at 18 + 4 = 22
    d.extend_from_slice(b"X\0"); // 20: author and misc
    d.extend_from_slice(&[0, 4, 0, 4]); // 22: name at 26, data at 24 + 4 = 28
    d.extend_from_slice(b"T\0"); // 26: name
    // 28: the song data structure
    d.extend_from_slice(&LOAD.to_be_bytes());
    d.extend_from_slice(&[mask, 1]);
    d.extend_from_slice(&length.to_be_bytes());
    d.extend_from_slice(&0u16.to_be_bytes());
    d.extend_from_slice(&50u16.to_be_bytes());
    d.extend_from_slice(&[0, 1, 2, 3]);
    let mut a = LOAD + 6;
    let mut body = Vec::new();
    for v in voices {
        d.extend_from_slice(&a.to_le_bytes());
        body.extend_from_slice(v);
        a += v.len() as u16;
    }
    d.extend_from_slice(&body);
    d.extend_from_slice(extra);
    d
}

/// The address `extra[i]` gets in `amad_file`.
fn extra_addr(voices: [&[u8]; 3], i: usize) -> u16 {
    LOAD + 6 + voices.iter().map(|v| v.len() as u16).sum::<u16>() + i as u16
}

fn lo(a: u16) -> u8 {
    a as u8
}
fn hi(a: u16) -> u8 {
    (a >> 8) as u8
}

fn song(d: &[u8]) -> Amad {
    Amad::new(d, 28).unwrap()
}

#[test]
fn amad_period_formula() {
    // a few of the 84 periods the original routines write
    assert_eq!(period(1), 4031);
    assert_eq!(period(7), 2850);
    assert_eq!(period(24), 1068);
    assert_eq!(period(31), 712);
    assert_eq!(period(36), 534);
    assert_eq!(period(84), 33);
}

#[test]
fn amad_detect_and_parse() {
    let silent: &[u8] = &[0, 10, 0x80, lo(LOAD + 6), hi(LOAD + 6)];
    let d = amad_file([silent, silent, silent], &[], 31, 500);
    assert!(detect(&d));
    assert_eq!(type_id(&d), Some(&b"AMAD"[..]));
    let f = parse(&d).unwrap();
    assert_eq!(f.songs.len(), 1);
    assert_eq!(f.songs[0].name, "T");
    assert_eq!(f.songs[0].amad, Some(28));
    assert_eq!((f.songs[0].length, f.songs[0].fade), (500, 50));
    let h = Header::parse(&d, 28).unwrap();
    assert_eq!((h.load, h.noise_mask, h.length), (LOAD, 31, 500));
    // the other ZXAY types are not claimed
    let mut st = d.clone();
    st[4..8].copy_from_slice(b"ST11");
    assert!(!detect(&st));
    assert_eq!(type_id(&st), Some(&b"ST11"[..]));
}

#[test]
fn amad_notes_env_vib() {
    // voice 1: ENV e, VIB v, note 24 for 3 frames, rest 2, JUMP back
    let pre = 3 + 3; // ENV, VIB
    let v1_len = pre + 2 + 2 + 3;
    let v23: &[u8] = &[0, 50, 0x80, 0, 0]; // patched below
    let dummy1 = vec![0u8; v1_len];
    let e = extra_addr([&dummy1, v23, v23], 0);
    let vb = e + 5;
    let v1: Vec<u8> = vec![0x87, lo(e), hi(e), 0x86, lo(vb), hi(vb), 24, 3, 0, 2, 0x80, lo(LOAD + 6), hi(LOAD + 6)];
    assert_eq!(v1.len(), v1_len);
    let a2 = LOAD + 6 + v1_len as u16;
    let v2: Vec<u8> = vec![0, 50, 0x80, lo(a2), hi(a2)];
    let a3 = a2 + 5;
    let v3: Vec<u8> = vec![0, 50, 0x80, lo(a3), hi(a3)];
    // envelope 13 for 2 frames then 62 (12) for one, then 10 held; vibrato +5, -3 repeated
    let extra = [13, 2, 62, 10, 0, 5, 0xFD, 0x80, lo(vb), hi(vb)];
    let d = amad_file([&v1, &v2, &v3], &extra, 31, 0);
    let mut a = song(&d);
    let p = period(24);
    let f: Vec<[u8; 13]> = (0..6).map(|_| a.tick()).collect();
    let per = |r: &[u8; 13]| r[0] as u16 | (r[1] as u16) << 8;
    // first frame pure, then +5, then -3
    assert_eq!(per(&f[0]), p);
    assert_eq!(per(&f[1]), p + 5);
    assert_eq!(per(&f[2]), p + 2);
    assert_eq!([f[0][8], f[1][8], f[2][8]], [13, 13, 12]);
    // the rest: period 0, volume 0
    assert_eq!((per(&f[3]), f[3][8]), (0, 0));
    assert_eq!((per(&f[4]), f[4][8]), (0, 0));
    // and again
    assert_eq!((per(&f[5]), f[5][8]), (p, 13));
    // voices 2 and 3 are silent, all tone, no noise
    assert_eq!(f[0][9], 0);
    assert_eq!(f[0][7], 0x38);
}

#[test]
fn amad_loops_calls_transpose_noise() {
    // voice 1: TR 2, LOOP 3 { CALL sub } JUMP self-loop of rests; sub: TR+ 1 note 10 len 1 NOISE+ -3 RET
    let base = LOAD + 6;
    let sub = base + 11;
    let mut v1 = vec![0x88, 2, 0x84, 1, 0x82, 3, 0x81, lo(sub), hi(sub), 0x83];
    let tail = base + v1.len() as u16;
    v1.extend_from_slice(&[0, 9, 0x80, lo(tail), hi(tail)]);
    // fix the sub address: it follows the tail loop
    let sub = base + v1.len() as u16;
    v1[7] = lo(sub);
    v1[8] = hi(sub);
    v1.extend_from_slice(&[0x8E, 1, 0x85, 1, 10, 1, 0x8D, 0xFD, 0x89]);
    let a2 = base + v1.len() as u16;
    let v2 = vec![0, 50, 0x80, lo(a2), hi(a2)];
    let a3 = a2 + 5;
    let v3 = vec![0, 50, 0x80, lo(a3), hi(a3)];
    // the envelope and vibrato pointers are left at 0: they read the zero data at $0000
    let d = amad_file([&v1, &v2, &v3], &[], 15, 0);
    let mut a = song(&d);
    let f: Vec<[u8; 13]> = (0..4).map(|_| a.tick()).collect();
    let per = |r: &[u8; 13]| r[0] as u16 | (r[1] as u16) << 8;
    assert_eq!(per(&f[0]), period(13));
    assert_eq!(per(&f[1]), period(14));
    assert_eq!(per(&f[2]), period(15));
    assert_eq!(per(&f[3]), 0, "the loop ended: rests");
    // NOISE 1, then -3 at each call's end, ANDed with 15: 1, 14, 11, 8
    assert_eq!([f[0][6], f[1][6], f[2][6], f[3][6]], [1, 14, 11, 8]);
    // TYPE 1: tone off, noise on for channel A
    assert_eq!(f[0][7] & 9, 1);
}

#[test]
fn amad_plays_and_ends() {
    let silent: &[u8] = &[40, 10, 0x80, lo(LOAD + 6), hi(LOAD + 6)];
    let d = amad_file([silent, silent, silent], &[], 31, 100);
    let mut p = AyPlayer::new(&d, 48000).unwrap();
    assert_eq!(p.duration_secs(), Some(3.0));
    let mut buf = vec![0f32; 2 * 48000];
    let mut frames = 0;
    while !p.ended() && frames < 10 {
        p.render(&mut buf);
        frames += 1;
    }
    assert!(p.ended() && frames == 3, "ends after length + fade ({frames} s)");
    let x = player(&d, 48000).unwrap();
    drop(x);
}

#[test]
fn amad_malformed_is_bounded() {
    let base = LOAD + 6;
    // a voice that jumps to itself without a note, one that calls itself forever, one that
    // loops 0 times around nothing
    let v1: &[u8] = &[0x80, lo(base), hi(base)];
    let a2 = base + 3;
    let v2: &[u8] = &[0x81, lo(a2), hi(a2)];
    let v3: &[u8] = &[0x82, 0, 0x83, 0x89, 0x83, 0x8C, 0, 0];
    let d = amad_file([v1, v2, v3], &[], 31, 0);
    let mut a = song(&d);
    for _ in 0..200 {
        a.tick();
    }
    // truncated files: no song, or no panic
    for n in 0..d.len() {
        let t = &d[..n];
        if detect(t) {
            let mut p = AyPlayer::new(t, 8000).unwrap();
            let mut buf = vec![0f32; 2 * 800];
            p.render(&mut buf);
        }
    }
    // pseudo-random data after a valid header
    let mut x = 0x1234_5678u32;
    for _ in 0..20 {
        let mut r = d[..28 + 14].to_vec();
        for _ in 0..600 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            r.push(x as u8);
        }
        let mut p = AyPlayer::new(&r, 8000).unwrap();
        let mut buf = vec![0f32; 2 * 8000];
        p.render(&mut buf);
        assert!(buf.iter().all(|s| s.is_finite()));
    }
}

/// Writes the AY registers 0..=13 after each 50 Hz frame, one line per frame (hex), for
/// AY_DUMP=file [AY_SONG=n] [AY_FRAMES=n] to AY_OUT (default stdout). With AY_WRITES=1 each
/// line also lists the frame's register writes in order, after " |", as " register=value"
/// (register decimal, value hex): the reference format of tools/zxay-st-ref.py.
#[test]
#[ignore]
fn regdump() {
    let Ok(f) = std::env::var("AY_DUMP") else { return };
    let d = std::fs::read(f).unwrap();
    let mut p = AyPlayer::new(&d, 48000).unwrap();
    p.start(std::env::var("AY_SONG").ok().and_then(|s| s.parse().ok()).unwrap_or(0));
    let frames: u64 = std::env::var("AY_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    let writes = std::env::var("AY_WRITES").is_ok_and(|v| v == "1");
    let mut out = String::new();
    for _ in 0..frames {
        p.frame();
        let r = p.m.regs;
        out += &r[..14].iter().map(|v| format!("{v:02x}")).collect::<Vec<_>>().join(" ");
        if writes {
            out += " |";
            for &(_, e) in &p.m.events {
                if let Ev::Reg(r, v) = e {
                    out += &format!(" {r}={v:02x}");
                }
            }
        }
        out.push('\n');
    }
    match std::env::var("AY_OUT") {
        Ok(o) => std::fs::write(o, out).unwrap(),
        Err(_) => print!("{out}"),
    }
}
