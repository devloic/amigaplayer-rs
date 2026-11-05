use super::*;
use crate::machine::Machine;

const SIZE: usize = 0x400;

fn put(d: &mut [u8], o: usize, w: u16) {
    d[o..o + 2].copy_from_slice(&w.to_be_bytes());
}

/// A short branch-always with a word displacement from `at` to `to`.
fn bra(d: &mut [u8], at: usize, to: usize) {
    put(d, at, 0x6000);
    put(d, at + 2, (to as isize - (at as isize + 2)) as i16 as u16);
}

/// Builds a small module of driver build `build` (0 = A, 1 = B, 2 = C) in code. The first words
/// pass the player's check; the code at 0x40 sets the voice 0 base in A3 and the offsets the sites
/// use, then jumps from site to site (every site's code words are those of the site table), ending
/// with a return after the song-end site. `entry` replaces the first jump when given.
fn module(build: usize, entry: Option<u16>) -> Vec<u8> {
    let mut d = vec![0u8; SIZE];
    // the check: a jump over the words it scans (the scanned words are never executed)
    bra(&mut d, 0, 0x40);
    for (i, w) in [0x41FAu16, 0, 0xE742, 0x41FA, 0, 0xD1FA, 0].into_iter().enumerate() {
        put(&mut d, 4 + 2 * i, w);
    }
    // the table of this build
    let mut p = 0;
    let mut sites = Vec::new();
    for b in 0..=build {
        let n = SITE_TABLES[p] as usize;
        if b == build {
            for i in 0..n {
                let e = p + 1 + 9 * i;
                sites.push((be16(SITE_TABLES, e) as usize, SITE_TABLES[e + 8], be16(SITE_TABLES, e + 4)));
            }
        }
        p += 1 + 9 * n;
    }
    for &(off, _, _) in &sites {
        for (j, w) in site_words(build, off).into_iter().enumerate() {
            put(&mut d, off + 2 * j, w);
        }
    }
    // data read by the LEN and PERTAB sites
    put(&mut d, 0x16, 0x0033);
    put(&mut d, 0x20, 0x0ABB);
    put(&mut d, 0x22, 0x0ABC);
    let has_pertab = sites.iter().any(|s| s.1 == 6);
    // setup: A3 = voice 0 (or its period register for the table lookup), D2 = 0x10, A1 = A0 + 0x20
    let mut c = 0x40;
    for w in [0x267Cu16, 0x00DF, if has_pertab { 0xF0A6 } else { 0xF0A0 }, 0x7410, 0x43E8, 0x0020] {
        put(&mut d, c, w);
        c += 2;
    }
    // the chain, in the order of the kinds: PERTAB, LEN, LOC or LOCCMP, PER, VOL, END
    let order = [6u8, 1, 2, 3, 4, 5, 7];
    let chain: Vec<(usize, u8)> = order.iter().filter_map(|&k| sites.iter().find(|s| s.1 == k).map(|s| (s.0, k))).collect();
    match entry {
        Some(w) => put(&mut d, c, w),
        None => bra(&mut d, c, chain[0].0),
    }
    for (i, &(off, kind)) in chain.iter().enumerate() {
        let next = chain.get(i + 1).map(|n| n.0);
        let at = off + 6;
        match (kind, next) {
            (6, Some(n)) => {
                // back from the period register to the voice base
                put(&mut d, at, 0x47EB);
                put(&mut d, at + 2, 0xFFFA);
                bra(&mut d, at + 4, n);
            }
            (3, Some(n)) => {
                // branch on "not equal" over a jump to the song end
                put(&mut d, at, 0x6604);
                bra(&mut d, at + 2, chain.last().unwrap().0);
                bra(&mut d, at + 6, n);
            }
            (4, Some(n)) => {
                put(&mut d, at, 0x7445);
                bra(&mut d, at + 2, n);
            }
            (7, _) => put(&mut d, at, 0x4E75),
            (_, Some(n)) => bra(&mut d, at, n),
            _ => {}
        }
    }
    d
}

/// The three code words of the site at `off` in the table of `build`.
fn site_words(build: usize, off: usize) -> [u16; 3] {
    let mut p = 0;
    for b in 0.. {
        let n = SITE_TABLES[p] as usize;
        if b == build {
            for i in 0..n {
                let e = p + 1 + 9 * i;
                if be16(SITE_TABLES, e) as usize == off {
                    return [be16(SITE_TABLES, e + 2), be16(SITE_TABLES, e + 4), be16(SITE_TABLES, e + 6)];
                }
            }
        }
        p += 1 + 9 * n;
    }
    unreachable!()
}

fn start(d: &[u8]) -> (Machine, Box<dyn Replayer>, u32) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let base = m.alloc(d.len() as u32);
    m.bus.write_bytes(base, d);
    let r = new(d, base, &mut m).expect("recognised");
    (m, r, base)
}

fn tick(m: &mut Machine, r: &mut Box<dyn Replayer>) {
    m.bus.clock += 70940;
    r.tick(&mut Hw { bus: &mut m.bus });
}

fn find(log: &[(u32, u16, u64)], a: u32) -> Vec<(u16, u64)> {
    log.iter().filter(|e| e.0 == a).map(|e| (e.1, e.2)).collect()
}

#[test]
fn the_site_tables_parse() {
    assert_eq!(SITE_TABLES.len(), 156);
    let mut counts = Vec::new();
    let mut p = 0;
    while p < SITE_TABLES.len() {
        let n = SITE_TABLES[p] as usize;
        counts.push(n);
        p += 1 + 9 * n;
    }
    assert_eq!(counts, vec![5, 6, 6]);
}

#[test]
fn detect_accepts_the_three_builds_and_identifies_them() {
    for b in 0..3 {
        let d = module(b, None);
        assert!(detect(&d), "build {b}");
        assert_eq!(sites_of(&d).unwrap().0, b);
    }
}

#[test]
fn detect_rejects_what_the_check_or_the_tables_reject() {
    let d = module(0, None);
    // the word at offset 4, the three scanned words, each site word
    for o in [4usize, 8, 10, 14] {
        let mut bad = d.clone();
        put(&mut bad, o, 0x4E71);
        assert!(!detect(&bad), "offset {o:#x}");
    }
    for (o, _) in [(0x174usize, 0), (0x186, 0), (0x1D4, 0), (0x274, 0), (0xE4, 0)] {
        let mut bad = d.clone();
        bad[o + 3] ^= 0x10;
        assert!(!detect(&bad), "site {o:#x}");
    }
    // a file too short for the sites, empty, garbage, another format's header
    assert!(!detect(&d[..0x100]));
    assert!(!detect(&[]));
    assert!(!detect(&[0xFF; 600]));
    assert!(!detect(&[0; 4000]));
    assert!(!detect(b"DARIUS ZENDEH MODULE, NOT MARK II.."));
}

#[test]
fn the_check_scans_within_its_budgets() {
    let d = module(0, None);
    let c = check(&d).unwrap();
    assert_eq!((c.ne, c.nf, c.nd, c.nb), (5, 1, 2, 3));
    assert_eq!(c.cycles(), 87 + 31 * 8 + 33 * 3);
    // no 0xD1FA within 24 words after the 0x41FA
    let mut bad = d.clone();
    for o in (0xE..0x40).step_by(2) {
        put(&mut bad, o, 0x4E71);
    }
    assert!(check(&bad).is_none());
    // the 0xD1FA at the 24th word is found (with a 0x41FA before it), at the 25th it is not
    let mut far = d.clone();
    for o in (0xE..0x40).step_by(2) {
        put(&mut far, o, 0x4E71);
    }
    put(&mut far, 0x38, 0x41FA);
    put(&mut far, 0xC + 2 * 23, 0xD1FA);
    assert_eq!(check(&far).map(|c| c.nd), Some(24));
    put(&mut far, 0xC + 2 * 23, 0x4E71);
    put(&mut far, 0xC + 2 * 24, 0xD1FA);
    assert!(check(&far).is_none());
}

#[test]
fn the_compare_flags_are_those_of_a_byte_compare() {
    assert_eq!(cmp_flags(0x32), 0b0100);
    assert_eq!(cmp_flags(0x33), 0b0000);
    assert_eq!(cmp_flags(0x31), 0b1001);
    assert_eq!(cmp_flags(0x80), 0b0010);
    assert_eq!(cmp_flags(0x00), 0b1001);
}

#[test]
fn the_voice_of_a_base_costs_as_voice_3_when_unknown() {
    assert_eq!([0xDFF0A0, 0xDFF0B0, 0xDFF0C0, 0xDFF0D0, 0xDFF000, 0].map(voice_of), [0, 1, 2, 3, 3, 3]);
}

#[test]
fn start_runs_the_sites_at_the_specified_clocks() {
    let d = module(0, None);
    let (m, r, base) = start(&d);
    let log = m.bus.write_log.clone().unwrap();
    // before the first site: a jump (10 cycles), the setup (12 + 4 + 8) and a jump (10), and the
    // call's own 32 cycles; the length store comes 76 cycles after the site is reached
    let c = check(&d).unwrap();
    let i = S0 + c.cycles() / 2 + C_INIT[0] / 2;
    let len = find(&log, 0xDFF0A4);
    assert_eq!(len, vec![(0x0033, i + (32 + 10 + 24 + 10 + 76) / 2)]);
    // location (long), period 0 (D1 is 0 in the start call), volume 0x45
    let loc_hi = find(&log, 0xDFF0A0);
    let loc_lo = find(&log, 0xDFF0A2);
    assert_eq!(loc_hi[0].1, loc_lo[0].1);
    assert_eq!(((loc_hi[0].0 as u32) << 16) | loc_lo[0].0 as u32, base + 0x10);
    assert_eq!(find(&log, 0xDFF0A6).iter().map(|e| e.0).collect::<Vec<_>>(), vec![0]);
    assert_eq!(find(&log, 0xDFF0A8).iter().map(|e| e.0).collect::<Vec<_>>(), vec![0x45]);
    // the song-end site is reached in the start call too: the report, and the clock after the
    // call is the first tick's origin
    assert!(r.songend());
    assert!(m.bus.clock > i);
}

#[test]
fn a_tick_passes_d1_and_costs_like_the_specification() {
    let d = module(0, None);
    let (mut m, mut r, _) = start(&d);
    m.bus.write_log = Some(Vec::new());
    let t = m.bus.clock + 70940;
    tick(&mut m, &mut r);
    let log = m.bus.write_log.take().unwrap();
    // a tick: 264 cycles, then as above; the period is D1 = 1
    assert_eq!(find(&log, 0xDFF0A4), vec![(0x0033, t + (264 + 10 + 24 + 10 + 76) / 2)]);
    assert_eq!(find(&log, 0xDFF0A6).iter().map(|e| e.0).collect::<Vec<_>>(), vec![1]);
    // the period store 50 cycles after its site is reached; the volume 231 after its own, and
    // the call ends 168 cycles after the driver's return
    let per = find(&log, 0xDFF0A6)[0].1;
    let vol = find(&log, 0xDFF0A8)[0].1;
    assert!(vol > per);
    assert_eq!(m.bus.clock, t + (264 + 10 + 24 + 10 + 169 + 10 + 147 + 10 + 135 + 4 + 10 + 311 + 10 + 116 + 16 + 168) / 2);
}

#[test]
fn builds_b_and_c_look_up_the_period_table_and_b_branches_on_the_compare() {
    for b in [1, 2] {
        let d = module(b, None);
        let (mut m, mut r, base) = start(&d);
        m.bus.write_log = Some(Vec::new());
        tick(&mut m, &mut r);
        let log = m.bus.write_log.take().unwrap();
        // the table lookup writes the word at A1 + 2 * D1 to the period register, then the
        // period site writes D1 again with its upper half replaced
        let per: Vec<u16> = find(&log, 0xDFF0A6).iter().map(|e| e.0).collect();
        assert_eq!(per, vec![0x0ABC, 0x0ABC], "build {b}");
        assert_eq!(find(&log, 0xDFF0A4)[0].0, 0x0033);
        assert_eq!(find(&log, 0xDFF0A8)[0].0, 0x45);
        // build B stores A0 as it is, build C adds D2 first
        let loc_lo = find(&log, 0xDFF0A2)[0].0;
        let loc_hi = find(&log, 0xDFF0A0)[0].0;
        let want = if b == 1 { base } else { base + 0x10 };
        assert_eq!(((loc_hi as u32) << 16) | loc_lo as u32, want, "build {b}");
    }
}

#[test]
fn the_song_end_clears_the_word_past_the_replaced_one() {
    for (b, off, k) in [(0usize, 0xE4usize, 0x01C2usize), (1, 0x144, 0x020A), (2, 0x144, 0x01FE)] {
        let mut d = module(b, None);
        let cleared = off + 6 + k;
        put(&mut d, cleared, 0xFFFF);
        // the word the replaced instruction names, four bytes before, is not touched
        put(&mut d, cleared - 4, 0xEEEE);
        let (m, r, base) = start(&d);
        assert!(r.songend(), "build {b}");
        assert_eq!(m.bus.r16(base + cleared as u32), 0, "build {b}");
        assert_eq!(m.bus.r16(base + cleared as u32 - 4), 0xEEEE, "build {b}");
    }
}

#[test]
fn a_driver_that_never_returns_ends_the_song_and_is_not_called_again() {
    // a branch to itself in place of the first jump
    let d = module(0, Some(0x60FE));
    let (mut m, mut r, _) = start(&d);
    assert!(r.songend());
    let n = m.bus.write_log.as_ref().unwrap().len();
    tick(&mut m, &mut r);
    assert_eq!(m.bus.write_log.as_ref().unwrap().len(), n);
}

#[test]
fn malformed_input_is_harmless() {
    let mut m = Machine::new(CpuType::M68000);
    for len in [0usize, 4, 6, 0x40, 0x174, 0x270] {
        let d = module(0, None);
        assert!(new(&d[..len.min(d.len())], 0, &mut m).is_none(), "{len}");
    }
    // a driver that jumps into the weeds ends the song instead of panicking
    let d = module(0, Some(0x4AFC));
    let (_m, r, _) = start(&d);
    assert!(r.songend());
}

#[test]
fn ticks_are_deterministic() {
    let run = || {
        let d = module(1, None);
        let (mut m, mut r, _) = start(&d);
        for _ in 0..3 {
            tick(&mut m, &mut r);
        }
        m.bus.write_log.take().unwrap()
    };
    assert_eq!(run(), run());
}
