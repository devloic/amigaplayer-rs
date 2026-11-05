use super::*;
use m68k::CpuType;

/// One pattern cell: period, instrument, command, argument.
fn cell(period: u16, inst: u8, cmd: u8, arg: u8) -> [u8; 4] {
    [(inst & 0xF0) | (period >> 8) as u8, period as u8, (inst << 4) | cmd, arg]
}

/// A module: the sample headers (length in words, volume, repeat start, repeat length), the order
/// list, and the patterns (each a list of 64 rows of four cells; shorter lists are padded).
fn module(heads: &[(u16, u8, u16, u16)], orders: &[u8], restart: u8, pats: &[Vec<[[u8; 4]; 4]>]) -> Vec<u8> {
    let mut d = vec![0u8; 0x438];
    d[..4].copy_from_slice(b"Test");
    for (i, h) in heads.iter().enumerate() {
        let o = 0x14 + 30 * i;
        d[o + 22..o + 24].copy_from_slice(&h.0.to_be_bytes());
        d[o + 25] = h.1;
        d[o + 26..o + 28].copy_from_slice(&h.2.to_be_bytes());
        d[o + 28..o + 30].copy_from_slice(&h.3.to_be_bytes());
    }
    // instrument 31: an empty header with a volume, which is what detection looks for
    d[0x14 + 30 * 30 + 25] = 40;
    d[0x3B6] = orders.len() as u8;
    d[0x3B7] = restart;
    d[0x3B8..0x3B8 + orders.len()].copy_from_slice(orders);
    d.extend_from_slice(b"EXO4");
    for p in pats {
        let mut rows = p.clone();
        rows.resize(64, [[0; 4]; 4]);
        for r in rows {
            for c in r {
                d.extend_from_slice(&c);
            }
        }
    }
    let samples: usize = heads.iter().map(|h| 2 * h.0 as usize).sum();
    d.extend(std::iter::repeat(0x55).take(samples + 8));
    d
}

/// A companion with the given AM records (instrument number, record words 3..=17 by name).
fn comp(recs: &[(usize, [u16; 18])]) -> Vec<u8> {
    let mut c = vec![0u8; 0x20A0];
    c[..16].copy_from_slice(b"AudioSculpture10");
    for k in 0..16 {
        c[0xFA0 + 0x110 * k] = 0xAB;
        c[0xFA1 + 0x110 * k] = 0xCD;
    }
    for (n, w) in recs {
        let o = 0x18 + 0x78 * n;
        c[o] = b'A';
        c[o + 1] = b'M';
        for (i, v) in w.iter().enumerate().skip(3) {
            c[o + 2 * i..o + 2 * i + 2].copy_from_slice(&v.to_be_bytes());
        }
    }
    c
}

/// Words: [_, _, _, start, t1, r1, t2, r2, sus, r3, hold, _, release, wave, fall, vibdepth, vibspeed, octave].
fn am(start: u16, release: u16, wave: u16) -> [u16; 18] {
    let mut w = [0u16; 18];
    w[3] = start;
    w[4] = start;
    w[5] = 1;
    w[6] = start;
    w[8] = start;
    w[10] = 1;
    w[12] = release;
    w[13] = wave;
    w
}

fn player(d: &[u8], c: &[u8]) -> AudioSculpture {
    AudioSculpture::build(d, c, 0x10000, 0x30000).unwrap()
}

fn run(r: &mut AudioSculpture, ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    m.bus.write_bytes(0x10000, &r.d);
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    m.bus.write_log.take().unwrap()
}

fn plain() -> Vec<u8> {
    module(&[(4, 64, 0, 0)], &[0], 0, &[vec![[cell(428, 1, 0, 0), [0; 4], [0; 4], [0; 4]]]])
}

#[test]
fn detect_rules() {
    let e = module(&[(4, 64, 0, 0)], &[0], 0, &[vec![]]);
    assert!(detect(&e));
    let mut f = e.clone();
    f[0x438..0x43C].copy_from_slice(b"FLT4");
    assert!(detect(&f));
    f[0x438..0x43C].copy_from_slice(b"M.K.");
    assert!(!detect(&f));
    assert!(!detect(&e[..0x43B]));
    assert!(!detect(&[]));
    // no empty slot with a volume (slot 31 reads order-list bytes)
    let mut g = e.clone();
    for i in 0..31 {
        let o = 0x14 + 30 * i;
        g[o + 23] = 1;
    }
    for i in 0x3CC..0x3D0 {
        g[i] = 0;
    }
    assert!(!detect(&g));
}

#[test]
fn subsong_scan() {
    let b = |a: u8| vec![[cell(0, 0, 0xB, a), [0; 4], [0; 4], [0; 4]]];
    // positions 0, 1, 2 each hold a B with arguments 3, 5, 7: three subsongs, the second starts
    // after position 0 and the third after position 1
    let d = module(&[(4, 64, 0, 0)], &[0, 1, 2], 0, &[b(3), b(5), b(7)]);
    let r = player(&d, &comp(&[]));
    assert_eq!(r.subs, vec![1, 2]);
    assert_eq!(r.subsongs(), (0, 2, 0));
    // a B that repeats the first argument collapses the count to one
    let d = module(&[(4, 64, 0, 0)], &[0, 1, 2], 0, &[b(3), b(5), b(3)]);
    assert!(player(&d, &comp(&[])).subs.is_empty());
    // no B at all
    assert!(player(&plain(), &comp(&[])).subs.is_empty());
}

#[test]
fn companion_names() {
    assert_eq!(super::companion("/x/y/Song.adsc"), vec!["Song.adsc.AS", "Song.adsc.NT"]);
}

#[test]
fn sample_row_writes_in_order() {
    let d = plain();
    let mut r = player(&d, &comp(&[]));
    let log = run(&mut r, 6);
    let seq: Vec<(u32, u16)> = log.iter().map(|w| (w.0, w.1)).collect();
    // the instrument's volume, voice off, LC (two halves), LEN, PER
    let i = seq.iter().position(|w| *w == (0xDFF0A8, 64)).unwrap();
    assert_eq!(seq[i + 1], (0xDFF096, 0x0001));
    assert_eq!(seq[i + 2].0, 0xDFF0A0);
    assert_eq!(seq[i + 4], (0xDFF0A4, 4));
    assert_eq!(seq[i + 5], (0xDFF0A6, 428));
    // DMA on with the note's bit, then the loop writes (length 0 repeat = the header's 0)
    let j = seq.iter().position(|w| *w == (0xDFF096, 0x8001)).unwrap();
    assert!(j > i);
    assert_eq!(seq[j + 3], (0xDFF0A4, 0));
}

#[test]
fn volume_command_and_clamp() {
    let d = module(&[(4, 64, 0, 0)], &[0], 0, &[vec![[cell(428, 1, 0xC, 200), [0; 4], [0; 4], [0; 4]]]]);
    let mut r = player(&d, &comp(&[]));
    let log = run(&mut r, 6);
    let vols: Vec<u16> = log.iter().filter(|w| w.0 == 0xDFF0A8).map(|w| w.1).collect();
    // the instrument's 64, then the command's argument clamped to 64
    assert_eq!(&vols[..2], &[64, 64]);
}

#[test]
fn slide_up_effect_ticks() {
    let d = module(&[(4, 64, 0, 0)], &[0], 0, &[vec![[cell(428, 1, 1, 4), [0; 4], [0; 4], [0; 4]]]]);
    let mut r = player(&d, &comp(&[]));
    let log = run(&mut r, 8);
    let per: Vec<u16> = log.iter().filter(|w| w.0 == 0xDFF0A6).map(|w| w.1).collect();
    // ticks 6.. : the row tick writes 428, then each effect tick subtracts 4
    let k = per.iter().position(|&p| p == 428).unwrap();
    assert_eq!(per[k + 1], 424);
}

#[test]
fn am_note_envelope_volume_and_waveform() {
    let rec = am(256, 8, 2);
    let d = module(&[(0, 64, 0, 0)], &[0], 0, &[vec![[cell(428, 1, 0, 0), [0; 4], [0; 4], [0; 4]]]]);
    let c = comp(&[(1, rec)]);
    let mut r = player(&d, &c);
    let log = run(&mut r, 8);
    // the AM waveform is the third of the player's four, 32 bytes each
    let lc: Vec<(u32, u16)> = log.iter().filter(|w| w.0 == 0xDFF0A2).map(|w| (w.0, w.1)).collect();
    assert!(lc.iter().any(|w| w.1 == (0x30000 + 64) as u16));
    // the volume at note start is level >> 2 times the volume / 64: 64 * 64 / 64
    let vols: Vec<u16> = log.iter().filter(|w| w.0 == 0xDFF0A8).map(|w| w.1).collect();
    assert!(vols.contains(&64));
}

#[test]
fn noise_buffer_is_rewritten_every_tick() {
    let d = plain();
    let mut r = player(&d, &comp(&[]));
    let mut m = Machine::new(CpuType::M68000);
    m.bus.clock = 1_000_000;
    let mut hw = Hw { bus: &mut m.bus };
    r.tick(&mut hw);
    let a = r.noise;
    assert_eq!(a[0], 0x27);
    assert!(a.iter().any(|&b| b != 0));
    assert_eq!(hw.bus.r8(0x30000 + 96), 0x27);
    let first = a;
    hw.bus.clock = 1_000_000 + TICK;
    r.tick(&mut hw);
    // a different line phase gives different bytes after the first
    assert_eq!(r.noise[0], 0x27);
    assert_ne!(r.noise[1..], first[1..]);
}

#[test]
fn song_end_on_revisit_and_determinism() {
    let d = module(&[(4, 64, 0, 0)], &[0], 0, &[vec![[cell(428, 1, 0, 0), [0; 4], [0; 4], [0; 4]]]]);
    let c = comp(&[]);
    let mut a = player(&d, &c);
    run(&mut a, 6 * 64);
    assert!(!a.songend);
    let mut b = player(&d, &c);
    let x = run(&mut b, 6 * 65 + 1);
    assert!(b.songend);
    let mut e = player(&d, &c);
    let y = run(&mut e, 6 * 65 + 1);
    assert_eq!(x, y);
}

#[test]
fn position_jump_and_break() {
    // row 0 of pattern 0 jumps to position 1 (argument 1); pattern 1 holds a note
    let p0 = vec![[cell(0, 0, 0xB, 1), [0; 4], [0; 4], [0; 4]]];
    let p1 = vec![[cell(300, 1, 0, 0), [0; 4], [0; 4], [0; 4]]];
    let d = module(&[(4, 64, 0, 0)], &[0, 1], 0, &[p0, p1]);
    let mut r = player(&d, &comp(&[]));
    let log = run(&mut r, 12);
    assert!(log.iter().any(|w| w.0 == 0xDFF0A6 && w.1 == 300));
}

#[test]
fn speed_command_limits() {
    let p = vec![[cell(0, 0, 0xF, 0), [0; 4], [0; 4], [0; 4]], [cell(0, 0, 0xF, 99), [0; 4], [0; 4], [0; 4]]];
    let d = module(&[(4, 64, 0, 0)], &[0], 0, &[p]);
    let mut r = player(&d, &comp(&[]));
    run(&mut r, 6);
    assert_eq!(r.speed, 1);
    run(&mut r, 2);
    assert_eq!(r.speed, 31);
}

#[test]
fn arpeggio_past_the_table_reads_addresses() {
    let d = module(&[(4, 64, 0, 0)], &[0], 0, &[vec![[cell(0, 0, 0, 0x01), [0; 4], [0; 4], [0; 4]]]]);
    let mut r = player(&d, &comp(&[]));
    r.counter = 0;
    // table[36 + 3] is pos * 256 + break
    r.pos = 2;
    assert_eq!(r.etable(39), 512);
    assert_eq!(r.etable(36), 0);
    // sample 1 starts after the 0x43C header and one pattern; the high word comes first
    let s = r.sample_addr(1);
    assert_eq!(r.etable(41), (s >> 16) as u16);
    assert_eq!(r.etable(42), s as u16);
}

#[test]
fn sample_start_is_cleared_at_init() {
    let d = plain();
    let mut r = player(&d, &comp(&[]));
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_bytes(0x10000, &d);
    let start = r.starts[1] as usize;
    assert_eq!(m.bus.r8(0x10000 + start as u32), 0x55);
    let mut hw = Hw { bus: &mut m.bus };
    r.init(&mut hw, INIT_AT, 0);
    assert_eq!(hw.bus.r8(0x10000 + start as u32), 0);
    assert_eq!(hw.bus.r8(0x10000 + start as u32 + 4), 0x55);
}

#[test]
fn malformed_input_does_not_panic() {
    // truncated module and companion, wild orders and instruments
    let mut d = plain();
    d.truncate(0x500);
    let mut r = player(&d, &[]);
    run(&mut r, 400);
    let mut d = plain();
    for b in d[0x3B8..0x438].iter_mut() {
        *b = 0xFF;
    }
    d[0x3B6] = 0xFF;
    let mut r = player(&d, &comp(&[(1, am(65535, 65535, 60000))]));
    run(&mut r, 300);
    let mut d = plain();
    for (i, b) in d[0x43C..0x43C + 1024].iter_mut().enumerate() {
        *b = (i * 37 + 11) as u8;
    }
    let mut r = player(&d, &comp(&[(1, am(300, 7, 3)), (5, am(100, 100, 0))]));
    run(&mut r, 1500);
}
