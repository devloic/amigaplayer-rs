use super::*;
use m68k::CpuType;

fn cmd(code: u16, dur: u16, a: u32, b: u32) -> [u8; 12] {
    let mut c = [0u8; 12];
    c[0..2].copy_from_slice(&code.to_be_bytes());
    c[2..4].copy_from_slice(&dur.to_be_bytes());
    c[4..8].copy_from_slice(&a.to_be_bytes());
    c[8..12].copy_from_slice(&b.to_be_bytes());
    c
}

fn note(dur: u16, f: u32) -> [u8; 12] {
    cmd(0x50, dur, f, 0)
}

fn vsel(i: u32) -> [u8; 12] {
    cmd(0x56, 0, 0, i * 4)
}

fn end() -> [u8; 12] {
    cmd(0x46, 0, 0, 0)
}

/// A score from four streams of commands (each gets its end command); the first command must be
/// a sample change.
fn score(streams: [&[[u8; 12]]; 4]) -> Vec<u8> {
    let mut d = vec![0u8; 16];
    d[1] = 1;
    d[7] = 4;
    for s in streams {
        for c in s {
            d.extend_from_slice(c);
        }
        d.extend_from_slice(&end());
    }
    d
}

/// A named-less ST sample set with `n` samples of the given lengths (bytes) and loop words.
fn set_bytes(samples: &[(u16, i16)]) -> Vec<u8> {
    let mut z = vec![0u8; 300];
    let mut o = 300usize;
    z[1] = samples.len() as u8 + 1;
    for (i, &(len, lp)) in samples.iter().enumerate() {
        z[142 + 4 * i..146 + 4 * i].copy_from_slice(&(o as u32).to_be_bytes());
        z.resize(o + 8 + len as usize, 0x90);
        z[o..o + 2].copy_from_slice(&lp.to_be_bytes());
        z[o + 4..o + 6].copy_from_slice(&len.to_be_bytes());
        o += 8 + len as usize;
    }
    z
}

fn player(d: &[u8], z: &[u8], wt: bool) -> QuartetSt {
    let set = load_set(z, wt).expect("set");
    let (starts, _) = scan_streams(d).expect("streams");
    let samp = resolve(&set, 0x20000, 0x30000);
    QuartetSt { v: voices(d, &starts, 0x30000), samp, amiga: set.amiga, empty: 0x30000, marks: [true; 4], songend: false, tick: 17_740 }
}

fn run(r: &mut QuartetSt, ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + r.tick * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    m.bus.write_log.take().unwrap()
}

#[test]
fn detection() {
    let d = score([&[vsel(0), note(1, 80_000)], &[], &[], &[]]);
    assert!(detect(&d));
    let mut e = d.clone();
    e[7] = 3;
    assert!(!detect(&e));
    let mut e = d.clone();
    e[1] = 17;
    assert!(!detect(&e));
    let mut e = d.clone();
    e[16 + 1] = 0x50;
    assert!(!detect(&e));
    let mut e = d.clone();
    e[12] = b'W';
    e[13] = b'T';
    assert!(detect(&e));
    e[13] = b'X';
    assert!(!detect(&e));
    let mut e = d.clone();
    e[27] = 0x4D;
    assert!(!detect(&e));
    assert!(!detect(&[]));
    assert!(!detect(&d[..20]));
    // four ends are needed to load
    assert!(scan_streams(&d).is_some());
    assert!(scan_streams(&d[..d.len() - 1]).is_none());
}

#[test]
fn period_function() {
    assert_eq!(per(0), 1);
    assert_eq!(per(7), 1);
    assert_eq!(per(8 * 8000), (FREQ_CONST / 8000 + 1) as u16);
    assert_eq!(per(8 * 10), PER_OVERFLOW);
    // a zero divisor (the reference crashes) takes the overflow period
    assert_eq!(per(0x80000), PER_OVERFLOW);
}

#[test]
fn set_conversion_st_and_amiga() {
    let z = set_bytes(&[(8, -1), (6, 2)]);
    let s = load_set(&z, false).unwrap();
    assert!(!s.amiga);
    // 0x90 - 0x80 = 0x10 for both bytes of each pair: the halves are 0x10
    let o = s.slots[0].off as usize;
    assert_eq!(&s.data[o + 8..o + 12], &[0x10; 4]);
    assert_eq!(&s.data[o + 6..o + 8], b"WT");
    let a = load_set(&z, true).unwrap();
    assert_eq!(&a.data[o + 8..o + 16], &[0x10; 8]);
    // a wrong sample count is corrupt; a set bit in a slot offset too; a zero-length ST record hangs
    let mut e = z.clone();
    e[1] = 9;
    assert!(load_set(&e, false).is_none());
    let mut e = z.clone();
    e[142] = 0x80;
    assert!(load_set(&e, false).is_none());
    assert!(load_set(&set_bytes(&[(0, -1)]), false).is_none());
    assert!(load_set(&set_bytes(&[(0, -1)]), true).is_some());
    assert!(load_set(&[], false).is_none());
}

#[test]
fn first_tick_and_note_start_writes() {
    let z = set_bytes(&[(40, -1)]);
    let d = score([&[vsel(0), note(3, 8 * 8000)], &[], &[], &[]]);
    let mut r = player(&d, &z, false);
    let w = run(&mut r, 3);
    // tick 0: four volumes, four periods of 1; tick 1: volumes, periods, DMACON off for voice 0 is
    // from tick 0's command stage
    let kinds: Vec<u32> = w.iter().map(|x| x.0).collect();
    assert_eq!(&kinds[..4], &[0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8]);
    assert!(w.iter().any(|x| x.0 == 0xDFF096 && x.1 == 1));
    assert!(w.iter().any(|x| x.0 == 0xDFF096 && x.1 == 0x8001));
    assert!(w.iter().any(|x| x.0 == 0xDFF0A6 && x.1 == per(8 * 8000)));
    let mut times: Vec<u64> = w.iter().map(|x| x.2).collect();
    let sorted = {
        let mut t = times.clone();
        t.sort();
        t
    };
    assert_eq!(times, sorted);
    times.dedup();
    // volume stamps: tick start plus T/2 of the ledger
    assert_eq!(w[0].2, 1_000_000 + 153);
    assert_eq!(w[1].2, 1_000_000 + 181);
}

#[test]
fn volumes_follow_the_gain_rule() {
    let d = score([&[], &[], &[], &[]]);
    let d = {
        let mut x = d;
        x.splice(16..16, vsel(0));
        x
    };
    let z = set_bytes(&[]);
    let mut r = player(&d, &z, false);
    let w = run(&mut r, 1);
    assert_eq!(w.iter().filter(|x| x.1 == 0x40 && (0xDFF0A8..=0xDFF0D8).contains(&x.0)).count(), 4);
}

#[test]
fn slide_end_check_is_the_reference_bug() {
    let z = set_bytes(&[(40, -1)]);
    // a downward slide to a normal target jumps to it after one step
    let d = score([&[vsel(0), note(1, 80_000), cmd(0x53, 20, 40_000, (-8i32) as u32), note(1, 1)], &[], &[], &[]]);
    let mut r = player(&d, &z, false);
    run(&mut r, 4);
    assert_eq!(r.v[0].f, 40_000);
    // an upward slide to a target above the period keeps going
    let d = score([&[vsel(0), note(1, 80_000), cmd(0x53, 20, 90_000, 8), note(1, 1)], &[], &[], &[]]);
    let mut r = player(&d, &z, false);
    run(&mut r, 6);
    assert_eq!(r.v[0].code, 0x53);
    assert!(r.v[0].f > 80_000 && r.v[0].f % 8 == 0);
}

#[test]
fn loops_play_n_plus_one_times() {
    let z = set_bytes(&[(40, -1)]);
    let lp = |n: u32| {
        let mut e = cmd(0x4C, 0, 0, 0);
        e[8..10].copy_from_slice(&(n as u16).to_be_bytes());
        e
    };
    let d = score([&[vsel(0), cmd(0x6C, 0, 0, 0), note(1, 80_000), lp(2), note(1, 40_000)], &[], &[], &[]]);
    let mut r = player(&d, &z, false);
    let w = run(&mut r, 10);
    // three notes of 80 000, then 40 000 (the stream then starts again): the first note's period comes at the second tick
    let pers: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    let a = per(80_000);
    let b = per(40_000);
    let mut it = pers.iter().skip(1);
    let seq: Vec<u16> = (0..4).map(|_| *it.next().unwrap()).collect();
    assert_eq!(seq, [a, a, a, b]);
}

#[test]
fn unknown_code_waits_and_never_hangs() {
    let z = set_bytes(&[(40, -1)]);
    let d = score([&[vsel(0), cmd(0x41, 0, 0, 0), note(1, 80_000)], &[], &[], &[]]);
    let mut r = player(&d, &z, false);
    run(&mut r, 5);
    assert!(r.v[0].dur > 65_000);
    // an empty loop stack and an endless run of timeless commands are bounded
    let d = score([&[vsel(0), cmd(0x4C, 0, 0, 1), note(1, 80_000)], &[], &[], &[]]);
    run(&mut player(&d, &z, false), 3);
    let d = score([&[vsel(0), cmd(0x6C, 0, 0, 0), cmd(0x4C, 0, 0, 65_535), note(1, 8)], &[], &[], &[]]);
    run(&mut player(&d, &z, false), 3);
}

#[test]
fn song_end_when_all_voices_ended_and_default_streams() {
    let z = set_bytes(&[(40, -1)]);
    let s: [&[[u8; 12]]; 4] = [&[vsel(0), note(2, 80_000)], &[vsel(0), note(3, 80_000)], &[vsel(0), note(1, 80_000)], &[vsel(0), cmd(0x52, 1, 0, 0), note(1, 5)]];
    let d = score(s);
    let mut r = player(&d, &z, false);
    assert!(!r.songend());
    run(&mut r, 3);
    assert!(!r.songend());
    run(&mut r, 8);
    assert!(r.songend());
    // a voice without a note rests 10 000 ticks, so the song does not end in a few ticks
    let d = score([&[vsel(0), note(1, 80_000)], &[vsel(0), note(1, 80_000)], &[vsel(0), note(1, 80_000)], &[vsel(0)]]);
    let mut r = player(&d, &z, false);
    run(&mut r, 20);
    assert!(!r.songend());
}

#[test]
fn rendering_is_deterministic_and_malformed_scores_are_safe() {
    let z = set_bytes(&[(40, 8), (24, -1)]);
    let d = score([&[vsel(0), note(2, 80_000), cmd(0x52, 2, 0, 0), vsel(1), note(1, 24_000)], &[vsel(1), note(5, 0x80008)], &[vsel(3), note(1, 1)], &[vsel(0), cmd(0x53, 3, 1, 1), note(1, 9)]]);
    let a = run(&mut player(&d, &z, false), 200);
    let b = run(&mut player(&d, &z, false), 200);
    assert_eq!(a, b);
    assert!(a.len() > 100);
    // random garbage commands
    let mut x = 0x1234_5678u32;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    for _ in 0..20 {
        let mut d = d.clone();
        for b in d.iter_mut().skip(28) {
            if rnd() % 5 == 0 {
                *b = rnd() as u8;
            }
        }
        if let Some((starts, _)) = scan_streams(&d) {
            let set = load_set(&z, false).unwrap();
            let samp = resolve(&set, 0x20000, 0x30000);
            let mut r = QuartetSt { v: voices(&d, &starts, 0x30000), samp, amiga: false, empty: 0x30000, marks: [true; 4], songend: false, tick: 17_740 };
            run(&mut r, 300);
        }
    }
}
