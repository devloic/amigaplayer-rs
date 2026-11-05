use super::*;
use m68k::CpuType;

type Row = [[u8; 2]; 4];
const NONE: [u8; 2] = [0, 0];

/// A module of the given tempo and rows, with the end mark and `pad` zero words.
fn module(tempo: u8, rows: &[Row], pad: usize) -> Vec<u8> {
    let mut d = vec![tempo, 0x50];
    for r in rows {
        for e in r {
            d.extend_from_slice(e);
        }
    }
    d.extend_from_slice(&[0xFF; 8]);
    d.extend(std::iter::repeat(0).take(2 * pad));
    d
}

fn set_with(len: [u16; 16]) -> Set {
    Set { len, start: [0; 16], special: [0; 16], loops: 0 }
}

fn player(d: &[u8], set: Set) -> Quartet {
    let mut addr = [0u32; 16];
    let mut slen = [0u16; 16];
    let mut a = 0u32;
    for i in 0..16 {
        addr[i] = a;
        if set.len[i] > 50 {
            slen[i] = set.len[i];
            a += (set.len[i] as u32 + 2) * 2;
        } else {
            slen[i] = 50;
        }
    }
    let (_, size) = song_scan(d);
    Quartet {
        d: d.to_vec(),
        tempo: d[0],
        counter: d[0],
        rp: 2,
        long: (size.saturating_sub(2) >> 8) + 1 > 2,
        set,
        samples: 0x20000,
        zero: 0x30000,
        addr,
        slen,
        ch: [Chan { p: 288, ..Chan::default() }; 4],
        origin: 0,
        songend: false,
    }
}

fn run(r: &mut Quartet, ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    m.bus.write_log.take().unwrap()
}

#[test]
fn detection() {
    let d = module(3, &[[NONE; 4]], 0);
    assert!(detect(&d));
    assert!(detect(&module(30, &[], 15)));
    assert!(!detect(&module(30, &[], 16)));
    // tempo must divide 3000 and be 1..30; the format mark is 0x50
    for t in [0u8, 7, 9, 31, 60] {
        assert!(!detect(&module(t, &[], 0)), "{t}");
    }
    let mut e = d.clone();
    e[1] = 0x51;
    assert!(!detect(&e));
    // the end mark must be four words
    let mut e = d.clone();
    let n = e.len();
    e[n - 8] = 0;
    assert!(!detect(&e));
    assert!(!detect(&[]));
    assert!(!detect(&[3, 0x50]));
    assert!(!detect(&[3]));
}

fn set_bytes(len: [u16; 16]) -> Vec<u8> {
    let sum: usize = len.iter().filter(|&&n| n != 0).map(|&n| n as usize + 2).sum();
    let z = 450 + 2 * sum - 2;
    let mut b = vec![0u8; z];
    b[4..8].copy_from_slice(b"lock");
    for i in 0..16 {
        b[z - 96 + 2 * i..z - 94 + 2 * i].copy_from_slice(&len[i].to_be_bytes());
    }
    b
}

#[test]
fn set_checks() {
    let mut len = [0u16; 16];
    len[0] = 100;
    len[3] = 20;
    let b = set_bytes(len);
    let s = parse_set(&b).unwrap();
    assert_eq!(s.len[0], 100);
    assert_eq!(s.len[3], 20);
    let mut e = b.clone();
    e[4] = b'x';
    assert!(parse_set(&e).is_none());
    let mut e = b.clone();
    e.push(0);
    e.push(0);
    assert!(parse_set(&e).is_none());
    assert!(parse_set(&b[..400]).is_none());
    assert!(parse_set(&[]).is_none());
}

#[test]
fn first_tick_clock_of_the_example() {
    // spec 5.5: a 202-byte module without padding, `QPA.basic` in a 90-character directory
    let len = [400, 300, 0, 120, 60, 51, 50, 30, 200, 100, 80, 90, 1000, 70, 64, 128];
    let sum = start_clocks(&[], 0, &set_with(len), 49, 90, "QPA.basic");
    assert_eq!(100 + sum + TICK, 75_381);
}

#[test]
fn note_start_writes_in_order() {
    // channel 1: octave code 1, volume 33, sample 2, note 5; looping sample 2
    let mut rows = [[NONE; 4]];
    rows[0][1] = [0x40 | 32, 0x25];
    let d = module(1, &rows, 0);
    let mut s = set_with([0, 0, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    s.loops = 4;
    s.start[2] = 6;
    let mut r = player(&d, s);
    let w = run(&mut r, 1);
    let kinds: Vec<u32> = w.iter().map(|x| x.0).collect();
    assert_eq!(kinds, [0xDFF096, 0xDFF0B0, 0xDFF0B2, 0xDFF0B4, 0xDFF0B6, 0xDFF0B8, 0xDFF096, 0xDFF0B0, 0xDFF0B2, 0xDFF0B4]);
    assert_eq!(w[0].1, 2);
    let addr = 0x20000u32 + 6;
    assert_eq!(((w[1].1 as u32) << 16) | w[2].1 as u32, addr);
    assert_eq!(w[1].2, w[2].2);
    assert_eq!(w[3].1, 100);
    assert_eq!(w[4].1, 257);
    assert_eq!(w[5].1, 33);
    assert_eq!(w[6].1, 0x8002);
    assert_eq!(((w[7].1 as u32) << 16) | w[8].1 as u32, addr);
    assert_eq!(w[9].1, 100);
    // the two raster waits take at least nine lines each
    assert!(w[6].2 - w[5].2 >= 8 * LINE);
    assert!(w[7].2 - w[6].2 >= 8 * LINE);
}

#[test]
fn one_shot_loops_the_zero_buffer_and_octaves_scale_the_period() {
    let mut rows = [[NONE; 4]];
    rows[0][0] = [0x80, 0x01]; // octave 2: half of 204
    rows[0][3] = [0x00 | 1, 0x07]; // octave 0: double of 288
    let d = module(1, &rows, 0);
    let mut r = player(&d, set_with([0; 16]));
    let w = run(&mut r, 1);
    let per: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A6 || x.0 == 0xDFF0D6).map(|x| x.1).collect();
    assert_eq!(per, [102, 576]);
    // the zero buffer with length 1, and sample length 50 for an empty set slot
    let lens: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A4).map(|x| x.1).collect();
    assert_eq!(lens, [50, 1]);
    let hi: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A0).map(|x| x.1).collect();
    assert_eq!(hi, [2, 3]);
}

#[test]
fn slide_reaches_the_target_after_n_ticks() {
    // tempo 2: a note, a slide marker, an empty row, then the target note (row 3: k = 3, N = 6)
    let mut rows = vec![[NONE; 4]; 4];
    rows[0][0] = [0x40 | 5, 0x01]; // period 204
    rows[1][0] = [0x01, 0xFE];
    rows[3][0] = [0x40 | 5, 0x0C]; // period 384
    let d = module(2, &rows, 0);
    let mut r = player(&d, set_with([0; 16]));
    run(&mut r, 2);
    assert!(r.ch[0].slide);
    assert_eq!(r.ch[0].n, 6);
    // six ticks of steps end on the target
    let w = run(&mut r, 6);
    let last = w.iter().filter(|x| x.0 == 0xDFF0A6).last().unwrap();
    assert_eq!(last.1, 384);
}

#[test]
fn slide_search_past_the_end_reads_zero() {
    let mut rows = vec![[NONE; 4]; 2];
    rows[0][0] = [0x40 | 5, 0x01];
    rows[1][0] = [0x01, 0xFE];
    let d = module(1, &rows, 0);
    let mut r = player(&d, set_with([0; 16]));
    run(&mut r, 1);
    // nothing but the end mark and zeros: the mark's 0xFF bytes count as a hit
    assert!(r.ch[0].slide);
    let d = module(1, &rows[..1], 0);
    let mut r = player(&d, set_with([0; 16]));
    run(&mut r, 1);
    assert!(!r.ch[0].slide);
}

#[test]
fn song_end_restarts_a_long_song_and_not_a_short_one() {
    let mut rows = vec![[NONE; 4]; 70];
    rows[0][0] = [0x40, 0x01];
    let d = module(1, &rows, 0);
    let mut r = player(&d, set_with([0; 16]));
    assert!(r.long);
    let w = run(&mut r, 71);
    assert!(r.songend());
    // the first row played in the tick of the end: a second note start
    assert_eq!(w.iter().filter(|x| x.0 == 0xDFF096 && x.1 == 0x8001).count(), 2);
    let short = module(1, &rows[..2], 0);
    let mut r = player(&short, set_with([0; 16]));
    assert!(!r.long);
    run(&mut r, 3);
    assert!(r.songend());
    assert_eq!(r.counter, 0);
}

#[test]
fn malformed_input_does_not_panic() {
    // pseudo-random rows and sets: bounded loops, reads past the end are zero
    let mut x = 0x1234_5678u32;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    for _ in 0..20 {
        let rows: Vec<Row> = (0..(rnd() % 80) as usize)
            .map(|_| {
                let mut r = [NONE; 4];
                for e in r.iter_mut() {
                    *e = [rnd() as u8, if rnd() % 4 == 0 { 0xFE } else { rnd() as u8 }];
                }
                r
            })
            .collect();
        let tempo = [1u8, 2, 3, 5, 6, 30][(rnd() % 6) as usize];
        let d = module(tempo, &rows, (rnd() % 16) as usize);
        let mut len = [0u16; 16];
        for l in len.iter_mut() {
            *l = (rnd() % 120) as u16;
        }
        let mut s = set_with(len);
        s.loops = rnd() as u16;
        s.special = [(rnd() % 3) as u16; 16];
        let mut r = player(&d, s);
        run(&mut r, 600);
        let again = {
            let mut r2 = player(&d, set_with(len));
            r2.set.loops = r.set.loops;
            r2.set.special = r.set.special;
            run(&mut r2, 600).len()
        };
        let _ = again;
    }
}

#[test]
fn rendering_is_deterministic() {
    let mut rows = vec![[NONE; 4]; 12];
    rows[0][0] = [0x40 | 9, 0x23];
    rows[1][0] = [0x01, 0xFE];
    rows[6][2] = [0x80 | 20, 0x31];
    rows[8][0] = [0xC0 | 3, 0x2C];
    let d = module(3, &rows, 2);
    let len = [0, 0, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let a = run(&mut player(&d, set_with(len)), 80);
    let b = run(&mut player(&d, set_with(len)), 80);
    assert_eq!(a, b);
    assert!(a.len() > 20);
}
