//! Tests on synthetic modules (built here, no corpus bytes).

use super::*;

const BANK: u32 = 1000;
const BUF: u32 = 5000;

fn w16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn w32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn words(ws: &[u16]) -> Vec<u8> {
    let mut v = Vec::new();
    for &w in ws {
        w16(&mut v, w);
    }
    v
}

/// An instrument record: fixed-note flag, fine tune, four envelope offsets, sample start,
/// length in words, loop start and loop length.
fn inst(fixed: u8, fine: i16, env: [u32; 4], s: u32, sl: u16, ls: u32, ll: u16) -> [u8; 32] {
    let mut v = vec![0, fixed];
    w16(&mut v, fine as u16);
    for e in env {
        w32(&mut v, e);
    }
    w32(&mut v, s);
    w16(&mut v, sl);
    w32(&mut v, ls);
    w16(&mut v, ll);
    v.try_into().unwrap()
}

/// A module whose voice 0 plays `pat` (repeated `rep` extra times) and whose other voices
/// are off; `extra` follows the instruments, at the returned offset.
fn build(pat: &[u8], rep: u8, insts: &[[u8; 32]], extra: &[u8]) -> (Vec<u8>, usize) {
    // header, subsong table, padding ending in 65535, track lists
    let lists = 40;
    let l0 = lists;
    let l1 = l0 + 24;
    let stop = |v: &mut Vec<u8>| {
        v.extend_from_slice(&[255, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        w16(v, 65535);
    };
    let pats = l1 + 36;
    let itab = pats + pat.len() + 2;
    let mut d = Vec::new();
    w32(&mut d, itab as u32);
    w32(&mut d, 28);
    w32(&mut d, 28);
    for l in [l0, l1, l1 + 12, l1 + 24] {
        w32(&mut d, l as u32);
    }
    d.extend_from_slice(&[0; 10]);
    w16(&mut d, 65535);
    // voice 0: one entry and the loop marker
    w32(&mut d, pats as u32);
    d.extend_from_slice(&[0, rep, 200, 0, 0, 0, 0, 0]);
    d.extend_from_slice(&[254, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    w16(&mut d, 65279);
    for _ in 0..3 {
        stop(&mut d);
    }
    assert_eq!(d.len(), pats);
    d.extend_from_slice(pat);
    d.extend_from_slice(&[255, 0]);
    d.extend_from_slice(&[0; 0]);
    assert_eq!(d.len(), itab);
    // the word before the instrument table must be 65535
    let n = d.len();
    d[n - 2] = 255;
    d[n - 1] = 255;
    for i in insts {
        d.extend_from_slice(i);
    }
    d.extend_from_slice(&[1; 32]);
    let e = d.len();
    d.extend_from_slice(extra);
    (d, e)
}

fn plain() -> [u8; 32] {
    inst(0, 0, [0; 4], 100, 10, 200, 4)
}

#[test]
fn tables_match_the_formulas() {
    let t = tables();
    assert_eq!(t.len(), 2 * 944 + 256);
    let w = |i: usize| be16(&t, 2 * i);
    assert_eq!(w(0), 53421);
    assert_eq!(w(143), 14);
    assert_eq!(w(DOWN_AT), 8192);
    assert_eq!(w(DOWN_AT + 399), 7733);
    assert_eq!(w(UP_AT), 8192);
    assert_eq!(w(UP_AT + 399), 8678);
    assert_eq!(&t[WAVES_AT..WAVES_AT + 2], &[127, 127]);
    assert_eq!(t[WAVES_AT + 16 * 3 + 2], 0x81);
    assert_eq!(t[WAVES_AT + 16 * 3 + 3], 0x7F);
}

#[test]
fn detects_the_layout_and_nothing_else() {
    let (d, _) = build(&words(&[22, 3, 12, 1]), 0, &[plain(), plain()], &[]);
    assert!(detect(&d));
    assert!(!detect(&[0; 200]));
    assert!(!detect(&d[..30]));
    let mut bad = d.clone();
    bad[3] = 3; // odd instrument table
    assert!(!detect(&bad));
    let mut bad = d.clone();
    bad[7] = 30; // switch table not a multiple of 4
    assert!(!detect(&bad));
    let mut bad = d.clone();
    let n = be32(&bad, 0) as usize;
    bad[n - 1] = 0; // no 65535 before the instrument table
    assert!(!detect(&bad));
}

#[test]
fn first_second_and_last_tick_of_a_row() {
    // row 22: duration 3, note 12, instrument 1
    let (d, _) = build(&words(&[22, 3, 12, 1]), 0, &[plain(), plain()], &[]);
    let mut s = Sdr::new(&d, BANK, BUF);
    assert_eq!(s.subsongs, 1);
    let t1 = s.tick();
    assert_eq!(
        t1,
        vec![Ev::Loc(0, 1100), Ev::Len(0, 10), Ev::Per(0, 26710), Ev::Vol(0, 64), Ev::Dma(0x8001)]
    );
    let t2 = s.tick();
    assert_eq!(t2, vec![Ev::Loc(0, 1200), Ev::Len(0, 4), Ev::Per(0, 26710), Ev::Vol(0, 64), Ev::Dma(0x8001)]);
    // last tick: DMA off
    assert_eq!(s.tick(), vec![Ev::Per(0, 26710), Ev::Vol(0, 64), Ev::Dma(1)]);
    assert!(!s.songend);
}

#[test]
fn legato_keeps_dma_on_and_a_longer_row_continues() {
    let (d, _) = build(&words(&[22, 4, 12, 1, 40]), 0, &[plain(), plain()], &[]);
    let mut s = Sdr::new(&d, BANK, BUF);
    s.tick();
    s.tick();
    assert_eq!(s.tick(), vec![Ev::Per(0, 26710), Ev::Vol(0, 64), Ev::Dma(0x8001)]);
    assert_eq!(s.tick(), vec![Ev::Per(0, 26710), Ev::Vol(0, 64)]);
}

#[test]
fn song_end_in_the_tick_of_the_loop_marker_and_the_pattern_restarts() {
    let (d, _) = build(&words(&[22, 1, 12, 1]), 0, &[plain(), plain()], &[]);
    let mut s = Sdr::new(&d, BANK, BUF);
    assert!(!s.songend);
    let t1 = s.tick();
    assert_eq!(t1.len(), 5);
    assert!(!s.songend);
    // the pattern is over: the loop marker is met, the song ends and the row plays again
    let t2 = s.tick();
    assert!(s.songend);
    assert_eq!(t2, t1);
}

#[test]
fn pattern_repeat_count_delays_the_end() {
    let (d, _) = build(&words(&[22, 1, 12, 1]), 2, &[plain(), plain()], &[]);
    let mut s = Sdr::new(&d, BANK, BUF);
    for _ in 0..3 {
        s.tick();
        assert!(!s.songend);
    }
    s.tick();
    assert!(s.songend);
}

#[test]
fn rest_switches_the_channel_off() {
    let (d, _) = build(&words(&[30, 2]), 0, &[plain(), plain()], &[]);
    let mut s = Sdr::new(&d, BANK, BUF);
    assert_eq!(s.tick(), vec![Ev::Dma(1)]);
    assert_eq!(s.tick(), vec![Ev::Dma(1)]);
}

#[test]
fn volume_envelope_and_reductions() {
    // vol envelope: one segment of one value (50) repeated 255 times; row 28: duration 3, note 12,
    // volume reduction 5, instrument 1; the track reduction is 0
    let mut env = Vec::new();
    env.extend_from_slice(&[1, 255, 50]);
    let (mut d, e) = build(&words(&[28, 3, 12, 5, 1]), 0, &[plain(), plain()], &[]);
    // the envelope goes after the instruments; patch the instrument now that its place is known
    d.extend_from_slice(&env);
    let off = e as u32;
    let it = be32(&d, 0) as usize + 32;
    d[it + 4..it + 8].copy_from_slice(&off.to_be_bytes());
    let mut s = Sdr::new(&d, BANK, BUF);
    let t = s.tick();
    assert_eq!(t[3], Ev::Vol(0, 45));
    assert_eq!(s.tick()[3], Ev::Vol(0, 45));
}

#[test]
fn synth_voice_writes_a_wave_row_then_lowers_one_byte() {
    // wave envelope: one value (row 3, depth 5) sustained
    let mut env = Vec::new();
    env.extend_from_slice(&[1, 255]);
    env.extend_from_slice(&((3u16 << 8) | 5).to_be_bytes());
    let (mut d, e) = build(&words(&[14, 2, 1]), 0, &[plain(), plain()], &[]);
    d.extend_from_slice(&env);
    let it = be32(&d, 0) as usize + 32;
    d[it + 16..it + 20].copy_from_slice(&(e as u32).to_be_bytes());
    let mut s = Sdr::new(&d, BANK, BUF);
    let t = s.tick();
    let mut row = [0x7Fu8; 16];
    row[..3].fill(0x81);
    assert_eq!(t[0], Ev::Wave(0, row));
    assert_eq!(t[1], Ev::Sub(0, 3, 122));
    assert_eq!(t[2], Ev::Loc(0, BUF));
    assert_eq!(t[3], Ev::Len(0, 8));
    // the second tick of the row loops the same buffer
    let t = s.tick();
    assert_eq!(t[2], Ev::Loc(0, BUF));
}

#[test]
fn transpose_and_fixed_note_flag() {
    let mut fixed = plain();
    fixed[1] = 1;
    for (inst_no, period) in [(1u16, PERIODS[12]), (0, PERIODS[14])] {
        let (mut d, _) = build(&words(&[22, 2, 12, inst_no]), 0, &[plain(), fixed], &[]);
        let l0 = be32(&d, 12) as usize;
        d[l0 + 4] = 2; // track transpose
        let mut s = Sdr::new(&d, BANK, BUF);
        assert_eq!(s.tick()[2], Ev::Per(0, period));
    }
}

#[test]
fn instrument_match_replaces_the_number() {
    // match byte 200 -> replacement 0: instrument 200 plays instrument 0 (sample at 100);
    // instrument 1 would too, so make instrument 0 different
    let mut zero = plain();
    zero[20..24].copy_from_slice(&300u32.to_be_bytes());
    let (mut d, _) = build(&words(&[22, 2, 12, 200]), 0, &[zero, plain()], &[]);
    d.truncate(d.len());
    let mut s = Sdr::new(&d, BANK, BUF);
    assert_eq!(s.tick()[0], Ev::Loc(0, 1300));
}

/// `SDR_MODULE=file SDR_REF=log cargo test --release fetch_check -- --ignored --nocapture`:
/// every reference DMA fetch from a synth buffer (64280 + 16 v) against the buffer the
/// model holds at the end of that tick, or of the one before or after.
#[test]
#[ignore]
fn fetch_check() {
    let (Ok(m), Ok(r)) = (std::env::var("SDR_MODULE"), std::env::var("SDR_REF")) else {
        return;
    };
    let mut s = Sdr::new(&std::fs::read(m).unwrap(), 0, 64280);
    let mut buf = [[0u8; 16]; 4];
    let mut hist: Vec<[[u8; 16]; 4]> = Vec::new();
    let mut frame: i64 = -1;
    let (mut same, mut prev, mut next, mut none) = (0, 0, 0, 0);
    for l in std::fs::read_to_string(r).unwrap().lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() != 4 && f.len() != 3 {
            continue;
        }
        if f[0] == "dff09c" && f[1] == "0008" {
            frame += 1;
        } else if f[0].starts_with('F') && frame >= 0 && f.len() == 4 {
            let a = u32::from_str_radix(f[1], 16).unwrap();
            if (64280..64344).contains(&a) {
                let v = (a - 64280) as usize / 16;
                let o = (a - 64280) as usize % 16;
                let w = u16::from_str_radix(f[2], 16).unwrap();
                let fr = frame as usize;
                while hist.len() <= fr + 1 {
                    for e in s.tick() {
                        match e {
                            Ev::Wave(c, b) => buf[c] = b,
                            Ev::Sub(c, i, x) => buf[c][i] = x,
                            _ => {}
                        }
                    }
                    hist.push(buf);
                }
                let at = |i: usize| u16::from_be_bytes([hist[i][v][o], hist[i][v][o + 1]]);
                if at(fr) == w {
                    same += 1;
                } else if fr > 0 && at(fr - 1) == w {
                    prev += 1;
                } else if at(fr + 1) == w {
                    next += 1;
                } else {
                    none += 1;
                }
            }
        }
    }
    println!("fetches: this tick {same}, previous {prev}, next {next}, none {none}");
}
