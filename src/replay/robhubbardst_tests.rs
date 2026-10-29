use super::super::m68kenc::move_w_imm_abs;
use super::*;
use crate::engine::Engine;
use std::path::Path;

const SONG_A: usize = 2338;
const SONG_B: usize = 2356;

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

fn words(w: &[u16]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_be_bytes()).collect()
}

/// A synthetic module of build `a` (true: A, false: B) with the given acknowledge value:
/// the code is one `move.w #ack,intreq`, the period table runs 3421 down by 20, one
/// envelope stream (index 0, end marker), one subsong whose three track lists hold
/// the given streams (a list per voice, each ending in a zero entry).
fn module(a: bool, ack: u16, streams: [&[&[u8]]; 3]) -> Vec<u8> {
    let (song, env, per) = if a { (SONG_A, 1928, 1736) } else { (SONG_B, 1946, 1754) };
    let mut d = vec![0u8; song + 400];
    let code = words(&move_w_imm_abs(ack, 0xDFF09C));
    d[..code.len()].copy_from_slice(&code);
    for i in 0..96 {
        d[per + 2 * i..per + 2 * i + 2].copy_from_slice(&(3421 - 20 * i as u16).to_be_bytes());
    }
    for (i, off) in [16u32, 18, 22, 26].into_iter().enumerate() {
        put32(&mut d, env + 4 * i, off);
    }
    d[env + 16] = 0;
    d[env + 17] = 135;
    // the song table: 1 subsong; lists after it, then the streams
    put32(&mut d, song, 12);
    let mut list = song + 12;
    let mut at = song + 12 + 3 * 16;
    for (v, ss) in streams.iter().enumerate() {
        put32(&mut d, song + 4 * v + if v == 0 { 0 } else { 0 }, (list - song) as u32);
        for (k, s) in ss.iter().enumerate() {
            put32(&mut d, list + 4 * k, (at - song) as u32);
            d[at..at + s.len()].copy_from_slice(s);
            at += s.len() + 2;
        }
        list += 16;
    }
    d
}

const IDLE: &[u8] = &[224 + 2, 128 + 15];
/// Voice 2's stream that drops its bit of the mask, so that it plays the table sample.
const TABLE: &[u8] = &[128 + 12, 224 + 2, 128 + 15];

fn plain(a: bool, ack: u16, v0: &'static [u8]) -> Vec<u8> {
    module(a, ack, [&[v0], &[TABLE], &[IDLE]])
}

fn run(d: Vec<u8>, ticks: usize) -> Option<(Vec<(u32, u16, u64)>, bool)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/rho-test/rho.Test"), d).ok()?;
    if e.playing.as_ref()?.format != "Rob Hubbard ST" {
        return None;
    }
    let mut out = vec![0f32; 2 * 960];
    let mut end = false;
    // the first tick falls one tick period after the start
    for _ in 0..=ticks {
        e.render(&mut out);
        end |= e.songend();
    }
    Some((e.m.bus.write_log.take().unwrap(), end))
}

fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

#[test]
fn recognises_builds_by_ack_and_table() {
    assert!(detect(&plain(true, 64, IDLE)));
    assert!(detect(&plain(false, 4, IDLE)));
    // build C's value is refused, as is a build's value with the other build's layout
    assert!(!detect(&plain(true, 1024, IDLE)));
    assert!(!detect(&plain(false, 1024, IDLE)));
    assert!(!detect(&plain(false, 64, IDLE)));
    assert!(!detect(&plain(true, 4, IDLE)));
    // an unrelated acknowledge value
    assert!(!detect(&plain(true, 8, IDLE)));
}

#[test]
fn refuses_damaged_modules() {
    let d = plain(true, 64, IDLE);
    assert!(!detect(&[]));
    assert!(!detect(&d[..2000]));
    let mut e = d.clone();
    e[1736 + 1] ^= 1; // period table
    assert!(!detect(&e));
    let mut e = d.clone();
    put32(&mut e, SONG_A, 13); // not a multiple of 12
    assert!(!detect(&e));
    let mut e = d.clone();
    put32(&mut e, SONG_A, 0); // no subsong
    assert!(!detect(&e));
    let mut e = d.clone();
    put32(&mut e, SONG_A + 4, 0x7000_0000); // a list outside the file
    assert!(!detect(&e));
    // two different acknowledge values: not the driver
    let mut e = d.clone();
    let more = words(&move_w_imm_abs(4, 0xDFF09C));
    e[20..20 + more.len()].copy_from_slice(&more);
    assert!(!detect(&e));
    // a module too large
    let mut e = d.clone();
    e.resize(9000, 0);
    assert!(!detect(&e));
}

#[test]
fn first_frame_writes_in_order() {
    // initial volume 10 (attack 0, release 0, decay 3), duration 2, note 12
    let (log, end) = run(plain(true, 64, &[208 + 10, 0, 0, 3, 224 + 1, 12]), 3).unwrap();
    assert!(!end);
    // the start block: voice 3 sample, voice 2, voice 1, restart; then ADKCON and DMA
    assert_eq!(reg(&log, 0xDFF09E).first(), Some(&0x00FF));
    assert_eq!(reg(&log, 0xDFF096).first(), Some(&0x820F));
    // each frame restarts channel 3 with 4 words and acknowledges with the build's value
    assert!(reg(&log, 0xDFF0D4).iter().all(|&v| v == 4));
    assert_eq!(reg(&log, 0xDFF09C).iter().filter(|&&v| v == 64).count(), 3);
    // the note: index 0 + 12 of the period table, times 4
    assert_eq!(reg(&log, 0xDFF0A6).last(), Some(&((3421u16 - 240) << 2)));
    // voice 1 volume: the attack takes it to the initial volume
    assert!(reg(&log, 0xDFF0A8).iter().all(|&v| v <= 63));
    // voice 2 plays the 100-word table sample, with period (mode + 2) * 16
    assert_eq!(reg(&log, 0xDFF0B4).last(), Some(&100));
    assert_eq!(reg(&log, 0xDFF0B6).last(), Some(&32));
}

#[test]
fn build_b_acknowledges_with_its_value_and_its_sample() {
    let (log, _) = run(plain(false, 4, &[224 + 1, 12]), 2).unwrap();
    assert_eq!(reg(&log, 0xDFF09C).iter().filter(|&&v| v == 4).count(), 2);
    let (la, _) = run(plain(true, 64, &[224 + 1, 12]), 2).unwrap();
    // voice 2's table sample sits 210 bytes lower in build B
    let lo = |l: &[(u32, u16, u64)]| reg(l, 0xDFF0B2).last().copied().unwrap() as i32;
    assert_eq!(lo(&la) - lo(&log), 210);
}

#[test]
fn speed_sets_ticks_per_row() {
    // speed 4 (byte 187), duration 1, note 12, then note 20: the second row starts 4 ticks on
    let (log, _) = run(plain(true, 64, &[187, 224, 12, 20]), 6).unwrap();
    let p = reg(&log, 0xDFF0A6)[1..].to_vec();
    let per = |i: usize| (3421u16 - 20 * i as u16) << 2;
    assert_eq!(p[..4], [per(12); 4]);
    assert_eq!(p[4], per(20));
}

#[test]
fn song_end_command_stops_the_player() {
    let (log, end) = run(plain(true, 64, &[224 + 1, 128 + 14]), 4).unwrap();
    assert!(end);
    // the end routine silences everything: a second start block with periods 447
    assert!(reg(&log, 0xDFF0A6).iter().filter(|&&v| v == 447).count() >= 2);
    // build A then writes voice 2's volume 0 every frame; the output no longer runs
    let n0 = reg(&log, 0xDFF0B8).len();
    let (log6, _) = run(plain(true, 64, &[224 + 1, 128 + 14]), 6).unwrap();
    assert!(reg(&log6, 0xDFF0B8).len() > n0);
    // build B does not
    let (lb, _) = run(plain(false, 4, &[224 + 1, 128 + 14]), 4).unwrap();
    let (lb6, _) = run(plain(false, 4, &[224 + 1, 128 + 14]), 6).unwrap();
    assert_eq!(reg(&lb, 0xDFF0B8).len(), reg(&lb6, 0xDFF0B8).len());
}

#[test]
fn longest_track_wrapping_ends_the_song() {
    // voice 3's list is the longest: stream A hops to B, B plays a note, then hops on
    // and wraps to the start of the list
    let a: &[u8] = &[184, 224, 128 + 7];
    let b: &[u8] = &[12, 128 + 7, 12];
    let m = module(true, 64, [&[IDLE], &[IDLE], &[a, b]]);
    let (_, end) = run(m.clone(), 1).unwrap();
    assert!(!end);
    let (_, end) = run(m, 2).unwrap();
    assert!(end);
    // the same wrap on a voice whose list is not the longest does not end it
    let m = module(true, 64, [&[a, b], &[IDLE, IDLE, IDLE], &[IDLE]]);
    let (_, end) = run(m, 8).unwrap();
    assert!(!end);
}

#[test]
fn envelope_decays_and_gates_by_the_release() {
    // initial volume 15, attack 255 (jumps to full), release 0, decay count 2; row of 20 ticks
    let (log, _) = run(plain(true, 64, &[208 + 15, 255, 0, 2, 224 + 19, 12]), 10).unwrap();
    let v = reg(&log, 0xDFF0A8);
    assert!(v.len() >= 10);
    assert!(v.iter().all(|&x| x % 4 == 0 && x <= 60));
}

#[test]
fn detection_is_not_tripped_by_other_content() {
    assert!(!detect(&vec![0u8; 3000]));
    assert!(!detect(&vec![0xA5; 4000]));
}
