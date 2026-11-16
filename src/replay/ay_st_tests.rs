//! Tests of the Sound Tracker songs (ZXAY ST11 and STRC, `ay_st.rs`) and the register dump used
//! to compare them with the original Z80 player (docs/specs/ay-soundtracker/SPEC.md section 9).

use super::*;
use crate::aiff::Stream;

/// Writes the AY registers 0..=13 after each 50 Hz interrupt of a ZXAY Sound Tracker song, in
/// the format of `amad_tests::regdump` (AY_DUMP=file [AY_FRAMES=n] [AY_WRITES=1] [AY_OUT=file]),
/// so the log can be compared with the one of the original Z80 player over the reference EMUL
/// built by tools/zxay-st-ref.py. AY_FLAGS=file also writes one line per interrupt, "1" where a
/// channel sounded a note index 96-110 (a period this player does not model), else "0".
#[test]
#[ignore]
fn st_regdump() {
    let Ok(f) = std::env::var("AY_DUMP") else { return };
    let d = std::fs::read(f).unwrap();
    let mut p = AyPlayer::new(&d, 48000).unwrap();
    let frames: u64 = std::env::var("AY_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    let writes = std::env::var("AY_WRITES").is_ok_and(|v| v == "1");
    let (mut out, mut flags) = (String::new(), String::new());
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
        flags += if p.st.as_ref().is_some_and(|s| s.code_index) { "1\n" } else { "0\n" };
    }
    match std::env::var("AY_OUT") {
        Ok(o) => std::fs::write(o, out).unwrap(),
        Err(_) => print!("{out}"),
    }
    if let Ok(o) = std::env::var("AY_FLAGS") {
        std::fs::write(o, flags).unwrap();
    }
}

// ------------------------------------------------------------------ synthetic songs

/// The container of a one-song ZXAY file (type `typ`), the song data after it at offset 26.
fn container(typ: &[u8; 4], song: &[u8]) -> Vec<u8> {
    let mut d = b"ZXAY".to_vec();
    d.extend_from_slice(typ);
    d.extend_from_slice(&[0; 8]);
    d.extend_from_slice(&[0, 0]); // one song, the first
    d.extend_from_slice(&[0, 2]); // song table at 18 + 2
    d.extend_from_slice(&[0, 4, 0, 4]); // name at 24, data at 22 + 4
    d.extend_from_slice(b"T\0");
    d.extend_from_slice(song);
    d
}

/// An ST11 song builder: sample 1 plays volume 15 then 7s, tone, no noise, no period offset, no
/// loop; one position (pattern 1, no transposition).
struct St11 {
    m: Vec<u8>,
}

impl St11 {
    fn new(delay: u8, patlen: u8) -> St11 {
        let mut m = vec![0u8; 3009 + 576];
        for k in 0..32 {
            m[k] = 7;
            m[32 + k] = 0x80; // noise off
        }
        m[0] = 15;
        m[1950] = 1; // pattern 1
        m[3007] = delay;
        m[3008] = patlen;
        St11 { m }
    }
    fn cell(mut self, row: usize, ch: usize, b: [u8; 3]) -> St11 {
        let o = 3009 + row * 9 + ch * 3;
        self.m[o..o + 3].copy_from_slice(&b);
        self
    }
    fn file(&self) -> Vec<u8> {
        let mut s = vec![0, 1, 2, 3, 0, 0, 0, 50];
        s.extend_from_slice(&self.m);
        container(b"ST11", &s)
    }
}

/// The registers after each of `n` interrupts of a file.
fn regs(d: &[u8], n: usize) -> Vec<[u8; 14]> {
    let mut p = AyPlayer::new(d, 8000).unwrap();
    (0..n)
        .map(|_| {
            p.frame();
            let mut r = [0u8; 14];
            r.copy_from_slice(&p.m.regs[..14]);
            r
        })
        .collect()
}

/// The register writes of each of `n` interrupts of a file.
fn writes(d: &[u8], n: usize) -> Vec<Vec<(u8, u8)>> {
    let mut p = AyPlayer::new(d, 8000).unwrap();
    (0..n)
        .map(|_| {
            p.frame();
            p.m.events.iter().filter_map(|e| if let Ev::Reg(r, v) = e.1 { Some((r, v)) } else { None }).collect()
        })
        .collect()
}

fn per(r: &[u8; 14], c: usize) -> u16 {
    r[2 * c] as u16 | (r[2 * c + 1] as u16) << 8
}

/// Note C of octave 2 (name 3) and D of octave 2 (name 4).
const C2: u8 = 0x32;
const D2: u8 = 0x42;

#[test]
fn st_detect_and_formats() {
    let d = St11::new(3, 4).cell(0, 0, [C2, 0x10, 0]).file();
    assert!(detect(&d));
    assert_eq!(format_of(&d), Some("ZXAYST11"));
    assert_eq!(type_id(&d), Some(&b"ST11"[..]));
    let f = parse(&d).unwrap();
    assert_eq!(f.songs.len(), 1);
    assert_eq!(f.songs[0].name, "T");
    assert_eq!(f.songs[0].st, Some(26));
    // one pass: delay 3 x 4 rows x 1 position, no fade
    assert_eq!((f.songs[0].length, f.songs[0].fade), (12, 0));
    let p = AyPlayer::new(&d, 8000).unwrap();
    assert_eq!(p.duration_secs(), Some(12.0 / 50.0));
    assert!(player(&d, 8000).is_ok());
    // another type tag is not claimed
    let mut e = d.clone();
    e[4..8].copy_from_slice(b"XXXX");
    assert!(!detect(&e));
}

#[test]
fn st_init_and_first_notes() {
    // C2 on A at row 0 with sample 1, D2 on row 1; delay 3, patlen 4
    let d = St11::new(3, 4).cell(0, 0, [C2, 0x10, 0]).cell(1, 0, [D2, 0, 0]).file();
    let w = writes(&d, 8);
    // the init writes R10 down to R0 as 0, in that order
    assert_eq!(w[0], (0..=10).rev().map(|r| (r, 0)).collect::<Vec<_>>());
    let r = regs(&d, 8);
    // octave 2: the table's 3832 and 3424 shifted right by 2
    assert_eq!(per(&r[1], 0), 958);
    assert_eq!(per(&r[2], 0), 958);
    assert_eq!(r[1][8], 15);
    assert_eq!(r[2][8], 7);
    // tone on, noise off for A; B and C silent with their mixer bits 0
    assert_eq!(r[1][7], 0x08);
    // the next row is read three interrupts later (interrupt 4)
    assert_eq!(per(&r[3], 0), 958);
    assert_eq!(per(&r[4], 0), 856);
    assert_eq!(r[4][8], 15, "a new note restarts the sample");
    // no envelope was written; every interrupt writes R10..R0 in this order
    assert!(w.iter().skip(1).all(|x| x.iter().all(|&(r, _)| r < 11)));
    let order: Vec<u8> = w[1].iter().map(|x| x.0).collect();
    assert_eq!(order, vec![10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
}

#[test]
fn st_sample_length_and_loop() {
    // the note plays 31 positions, then the sample ends (rep 0)
    let d = St11::new(10, 8).cell(0, 0, [C2, 0x10, 0]).file();
    let r = regs(&d, 40);
    assert_eq!(r[31][8], 7);
    assert_eq!(r[32][8], 0, "the 32nd interrupt of the note is silent");
    // a loop: rep 1 (from position 0), replen 1 (2 positions)
    let mut s = St11::new(10, 8).cell(0, 0, [C2, 0x10, 0]);
    s.m[128] = 1;
    s.m[129] = 1;
    let r = regs(&s.file(), 40);
    let v: Vec<u8> = (32..36).map(|i| r[i][8]).collect();
    assert_eq!(v, vec![15, 7, 15, 7]);
}

#[test]
fn st_envelope_ornament_transposition() {
    // effect 3 (envelope shape 3) parameter 7 on the first note
    let d = St11::new(5, 4).cell(0, 0, [C2, 0x13, 7]).file();
    let w = writes(&d, 4);
    assert_eq!(&w[1][..3], &[(13, 3), (12, 0), (11, 7)]);
    assert_eq!(w[1][3], (10, 0));
    assert_eq!(w[1][5], (8, 0x1F), "volume 15 with the envelope bit");
    // written at its own interrupt only
    assert!(w[2].iter().all(|x| x.0 < 11));
    // effect 15 parameter 1: ornament 1 (all +12) drops the period by an octave
    let mut s = St11::new(5, 4).cell(0, 0, [C2, 0x1F, 1]);
    for k in 0..32 {
        s.m[2463 + 32 + k] = 12;
    }
    let r = regs(&s.file(), 3);
    assert_eq!(per(&r[1], 0), 479);
    // a transposition of 2 on the position: C2 sounds as D2
    let mut s = St11::new(5, 4).cell(0, 0, [C2, 0x10, 0]);
    s.m[1951] = 2;
    let r = regs(&s.file(), 3);
    assert_eq!(per(&r[1], 0), 856);
}

#[test]
fn st_rest_and_period_offset() {
    // note then a rest: the channel goes silent; a period offset of +16 (sign bit 12 set)
    let mut s = St11::new(2, 4).cell(0, 0, [C2, 0x10, 0]).cell(1, 0, [0xF0, 0, 0]);
    s.m[64] = 16;
    s.m[65] = 0x10;
    let r = regs(&s.file(), 4);
    assert_eq!(per(&r[1], 0), 958 + 16);
    assert_eq!(r[2][8], 7);
    assert_eq!(r[3][8], 0, "the rest at row 1 (interrupt 3)");
    // the period offset against the sign: 958 - 16
    let mut s = St11::new(2, 4).cell(0, 0, [C2, 0x10, 0]);
    s.m[64] = 16;
    let r = regs(&s.file(), 2);
    assert_eq!(per(&r[1], 0), 958 - 16);
}

#[test]
fn st_loops_the_position_list() {
    // delay 1, 2 rows, two positions: a pass is 4 interrupts, then position 0 again
    let mut s = St11::new(1, 2).cell(0, 0, [C2, 0x10, 0]);
    s.m[2462] = 1;
    s.m[1952] = 1;
    let f = s.file();
    assert_eq!(parse(&f).unwrap().songs[0].length, 4);
    let r = regs(&f, 12);
    // position 0: C2 at interrupt 1; position 1 (pattern 1 again) at interrupt 3, and so on
    assert_eq!(per(&r[1], 0), 958);
    assert_eq!(r[1][8], 15);
    assert_eq!(r[3][8], 15, "pattern 1 again: a new note");
    assert_eq!(r[5][8], 15, "the second pass is the first again");
}

#[test]
fn st_period_table_and_unmodelled_index() {
    // note G of octave 7 (name 7, octave 7): 2544 >> 7
    let s = St11::new(1, 1).cell(0, 0, [0x77, 0x10, 0]).file();
    let r = regs(&s, 2);
    assert_eq!(per(&r[1], 0), 2544 >> 7);
    // an ornament that lifts the index to 96..110 gives period 0, and the player says so
    let mut s = St11::new(1, 1).cell(0, 0, [0x77, 0x1F, 1]);
    for k in 0..32 {
        s.m[2463 + 32 + k] = 5; // G7 (91) + 5 = 96
    }
    let d = s.file();
    let mut p = AyPlayer::new(&d, 8000).unwrap();
    p.frame();
    p.frame();
    assert_eq!(p.m.regs[0] as u16 | (p.m.regs[1] as u16) << 8, 0);
    assert!(p.st.as_ref().unwrap().code_index);
}

/// A STRC song with one sample (volume 11), one position, `patlen` 4 and delay 3: the A stream
/// plays note `note` with sample 1 on its first row, then three empty rows.
fn strc_file(note: u8) -> Vec<u8> {
    let load = 0x8000usize;
    let mut b = vec![0u8; 162 + 7 + 2];
    b[0] = 3;
    b[1] = 4;
    // tables after the block's header: 32 zero words, volumes, noise bytes, an ornament
    let zero = load + b.len();
    let vol = zero + 64;
    let noi = vol + 32;
    let orn = noi + 32;
    let streams = orn + 32;
    let setw = |b: &mut Vec<u8>, o: usize, v: usize| {
        b[o] = v as u8;
        b[o + 1] = (v >> 8) as u8;
    };
    for k in 0..16 {
        setw(&mut b, 2 + 2 * k, zero);
        setw(&mut b, 34 + 2 * k, orn);
        setw(&mut b, 66 + 2 * k, vol);
        setw(&mut b, 98 + 2 * k, noi);
        b[130 + 2 * k] = 0; // rep
        b[131 + 2 * k] = 32; // loop length
    }
    setw(&mut b, 162, streams);
    setw(&mut b, 164, streams + 6);
    setw(&mut b, 166, streams + 6);
    let mut tail = vec![0u8; 64 + 32 + 32 + 32];
    tail[64..96].fill(11);
    tail[96..128].fill(0x80); // noise off
    tail.extend_from_slice(&[note, 0x10, 0, 0xFE, 0xFE, 0xFE]);
    tail.extend_from_slice(&[0xFE; 4]);
    b.extend_from_slice(&tail);
    let mut s = vec![0, 1, 2, 3];
    s.extend_from_slice(&(load as u16).to_be_bytes());
    s.extend_from_slice(&[0, 0, 0, 50]);
    s.extend_from_slice(&b);
    container(b"STRC", &s)
}

#[test]
fn strc_plays_like_its_decoded_song() {
    let d = strc_file(24);
    assert!(detect(&d));
    assert_eq!(format_of(&d), Some("ZXAYSTRC"));
    assert_eq!(parse(&d).unwrap().songs[0].length, 12);
    let r = regs(&d, 6);
    assert_eq!(per(&r[1], 0), 958);
    assert_eq!(r[1][8], 11);
    assert_eq!(r[1][7], 0x08);
    // the same song as ST11
    let mut e = St11::new(3, 4);
    e.m[0..32].fill(11);
    let e = e.cell(0, 0, [C2, 0x10, 0]).file();
    assert_eq!(regs(&d, 100), regs(&e, 100));
    // a note byte past 95 is an empty cell, not a crash
    let r = regs(&strc_file(0xF0), 6);
    assert_eq!(r[1][8], 0);
}

fn junk(seed: &mut u32, n: usize) -> Vec<u8> {
    (0..n)
        .map(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            *seed as u8
        })
        .collect()
}

#[test]
fn st_damaged_files_are_refused_or_bounded() {
    let mut seed = 0x2468_ACE1u32;
    let good = [St11::new(3, 4).cell(0, 0, [C2, 0x13, 7]).file(), strc_file(24)];
    for d in &good {
        // truncated at every length (stepped through the long ones)
        let mut n = 0;
        while n < d.len() {
            let t = &d[..n];
            if detect(t) {
                let mut p = AyPlayer::new(t, 8000).unwrap();
                let mut buf = vec![0f32; 2 * 800];
                p.render(&mut buf);
            }
            n += if n < 120 { 1 } else { 97 };
        }
        // the song data replaced by noise after the container
        for _ in 0..30 {
            let mut r = d[..26].to_vec();
            r.extend(junk(&mut seed, d.len() - 26));
            if detect(&r) {
                let mut p = AyPlayer::new(&r, 8000).unwrap();
                let mut buf = vec![0f32; 2 * 4000];
                p.render(&mut buf);
                assert!(buf.iter().all(|s| s.is_finite()));
            }
        }
        // a few bytes of the song changed at random places
        for _ in 0..30 {
            let mut r = d.clone();
            for x in junk(&mut seed, 40) {
                let at = 26 + (x as usize * 131 + junk(&mut seed, 1)[0] as usize * 7) % (d.len() - 26);
                r[at] = junk(&mut seed, 1)[0];
            }
            if detect(&r) {
                let mut p = AyPlayer::new(&r, 8000).unwrap();
                let mut buf = vec![0f32; 2 * 8000];
                p.render(&mut buf);
            }
        }
    }
    // a pattern length over 64, and the tables cut short
    assert!(!detect(&St11::new(3, 65).file()));
    let mut s = St11::new(3, 4).file();
    s.truncate(s.len() - 3000);
    assert!(!detect(&s));
}

#[test]
fn st_missing_pattern_is_empty() {
    // the position names pattern 2, which the file does not hold: silence
    let mut s = St11::new(3, 4).cell(0, 0, [C2, 0x10, 0]);
    s.m[1950] = 2;
    let r = regs(&s.file(), 10);
    assert!(r.iter().all(|x| x[8] == 0 && x[9] == 0 && x[10] == 0));
}
