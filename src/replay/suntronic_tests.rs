use super::*;
use m68k::CpuType;

const ORIGIN: u32 = 0x50000;
/// The offset of the first data after the sequence list (the origin is derived from it).
const DATA: usize = 0x10 + 0x19C + 0x2C;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// The header and the sequence list shared by both variants: two entries, then a zero long word.
fn skeleton(var: Var, size: usize) -> Vec<u8> {
    let mut d = vec![0u8; size];
    put32(&mut d, 0, 0x48E7_FFFE);
    put16(&mut d, 4, 0x4DFA);
    // the data area at offset 0x10
    put16(&mut d, 6, 0x10 - 6);
    put32(&mut d, 8, if var == Var::A { 0x4A2E_0018 } else { 0x4A2E_0010 });
    d
}

/// A variant-A module: period table 100, 110, ...; one instrument (envelope of 0x40, no LFO
/// effect, a morph table of zeros, wave A all 0x10 bytes, wave B all 0x20, type 0); streams: voice 0
/// plays the given row of events in row 0 of entry 0, the others are silent; two entries.
fn module_a(row0: &[u8], speed_cmd: Option<u8>) -> Vec<u8> {
    let dd = 0x10;
    let mut d = skeleton(Var::A, 0x600);
    d[dd + 0x18] = 0x40;
    for i in 0..72 {
        put16(&mut d, dd + 0x54 + 2 * i, 100 + 10 * i as u16);
    }
    // instrument 0 at dd + 0xE4
    let r = dd + 0xE4;
    let p = |o: usize| ORIGIN + o as u32;
    put32(&mut d, r, p(DATA)); // envelope
    put16(&mut d, r + 4, 4);
    put16(&mut d, r + 6, 0);
    put32(&mut d, r + 8, p(DATA + 8)); // LFO
    put16(&mut d, r + 0xC, 2);
    put16(&mut d, r + 0xE, 0);
    put16(&mut d, r + 0x10, 0);
    put32(&mut d, r + 0x12, p(DATA + 12)); // morph
    put16(&mut d, r + 0x16, 2);
    put16(&mut d, r + 0x18, 0);
    put32(&mut d, r + 0x1A, p(DATA + 16)); // wave A
    put32(&mut d, r + 0x1E, p(DATA + 32)); // wave B
    d[r + 0x22] = 8; // HL
    d[r + 0x23] = 0; // type 0
    for i in 0..4 {
        d[DATA + i] = 0x40;
    }
    for i in 0..16 {
        d[DATA + 16 + i] = 0x10;
        d[DATA + 32 + i] = 0x20;
    }
    // the sequence list at dd + 0x19C: entry 0 streams, entry 1 the same, then a zero long word
    let list = dd + 0x19C;
    let s0 = DATA + 48;
    let mut s = Vec::new();
    s.extend_from_slice(row0);
    if let Some(sp) = speed_cmd {
        s.extend_from_slice(&[0x98, sp]);
    }
    s.push(0);
    s.extend(std::iter::repeat_n(0u8, 15));
    d[s0..s0 + s.len()].copy_from_slice(&s);
    let quiet = s0 + s.len();
    for e in 0..2 {
        let o = list + 0x14 * e;
        put32(&mut d, o, p(s0));
        for v in 1..4 {
            put32(&mut d, o + 4 * v, p(quiet));
        }
        d[o + 0x10..o + 0x14].copy_from_slice(&[1, 1, 1, 1]);
    }
    d
}

/// A variant-B module with one instrument of the blend between two waves (no wave B pointer: feedback).
fn module_b(row0: &[u8]) -> Vec<u8> {
    let dd = 0x10;
    let mut d = skeleton(Var::B, 0x800);
    for i in 0..72 {
        put16(&mut d, dd + 0x46 + 2 * i, 100 + 10 * i as u16);
    }
    let r = dd + 0xD6;
    let p = |o: usize| ORIGIN + 0x100 + o as u32;
    // the list follows the instruments at dd + 0x22E
    let list = dd + 0x22E;
    let after = list + 0x14 * 2 + 4;
    put32(&mut d, r, p(after));
    put16(&mut d, r + 4, 4);
    put16(&mut d, r + 6, 0);
    put32(&mut d, r + 8, p(after + 8));
    put16(&mut d, r + 0xC, 2);
    put32(&mut d, r + 0x12, p(after + 12));
    put16(&mut d, r + 0x16, 2);
    put32(&mut d, r + 0x1A, p(after + 16));
    put32(&mut d, r + 0x1E, 0);
    for i in 0..4 {
        d[after + i] = 0x40;
    }
    for i in 0..32 {
        d[after + 16 + i] = 0x20;
    }
    let s0 = after + 64;
    let mut s = row0.to_vec();
    s.push(0);
    s.extend(std::iter::repeat_n(0u8, 15));
    d[s0..s0 + s.len()].copy_from_slice(&s);
    let quiet = s0 + s.len();
    // entry 0x1F is reached before the first row, which advances to 0x20 (then the zero entry
    // at that index wraps to 0): the list needs a zero entry there, so the entries are copies placed
    // at index 0 and the terminator follows; indexes up to 0x20 read zero bytes past the list
    for e in 0..2 {
        let o = list + 0x14 * e;
        put32(&mut d, o, p(s0));
        for v in 1..4 {
            put32(&mut d, o + 4 * v, p(quiet));
        }
        d[o + 0x10..o + 0x14].copy_from_slice(&[1, 1, 1, 1]);
    }
    d
}

fn start(d: &[u8]) -> (Machine, Box<dyn Replayer>) {
    let mut m = Machine::new(CpuType::M68000);
    let r = (OPEN.new)(d, 0x10000, &mut m).expect("module accepted");
    m.bus.write_log = Some(Vec::new());
    (m, r)
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let (mut m, mut r) = start(d);
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        r.tick(&mut Hw { bus: &mut m.bus });
    }
    (m.bus.write_log.take().unwrap(), r.songend())
}

fn tick_writes(log: &[(u32, u16, u64)], k: usize) -> Vec<(u32, u16)> {
    let lo = 1_000_000 + TICK * k as u64;
    log.iter().filter(|w| w.2 >= lo && w.2 < lo + TICK).map(|w| (w.0, w.1)).collect()
}

#[test]
fn detection() {
    let a = module_a(&[0x40], None);
    assert!(detect(&a));
    assert!(layout(&a).unwrap().var == Var::A);
    let b = module_b(&[0x40]);
    assert!(detect(&b));
    assert!(layout(&b).unwrap().var == Var::B);
    assert_eq!(layout(&b).unwrap().origin, ORIGIN + 0x100);
    for o in [0usize, 5] {
        let mut e = a.clone();
        e[o] ^= 1;
        assert!(!detect(&e), "{o}");
    }
    // the pseudo-opcode word of other formats at offset 8
    let mut e = a.clone();
    put32(&mut e, 8, 0x4A2E_07C0);
    assert!(!detect(&e));
    assert!(!detect(&a[..0x200]));
    assert!(!detect(&[]));
}

#[test]
fn variant_a_first_call_timing() {
    let d = module_a(&[0x40], None);
    let (log, _) = run(&d, 5);
    // DMA off first, 219 colour clocks into the call (call-relative 286 cycles, 76 of interrupt entry)
    assert_eq!((log[0].0, log[0].1), (0xDFF096, 0));
    assert_eq!(log[0].2, 1_000_000 + 219);
    // the same write pattern every idle call, DMA on last but not first
    let t1 = tick_writes(&log, 1);
    // call 1 restarts the voice the first row started; later calls are idle
    let t2 = tick_writes(&log, 3);
    let t3 = tick_writes(&log, 4);
    assert_eq!(t2.len(), 10);
    assert_eq!(t2.len(), t3.len());
    assert_eq!(t1[0].0, 0xDFF096);
    assert!(t1.iter().any(|w| w.0 == 0xDFF096 && w.1 & 0x8000 != 0));
}

#[test]
fn variant_b_runs() {
    let d = module_b(&[0x40]);
    let (log, _) = run(&d, 5);
    assert!(!log.is_empty());
    assert!(log.iter().all(|w| w.0 >= 0xDFF000));
    assert!(log.windows(2).all(|w| w[0].2 <= w[1].2));
}

#[test]
fn song_end_when_the_list_wraps() {
    let d = module_a(&[0x40], None);
    let (_, end) = run(&d, 5);
    assert!(!end);
    let (_, end) = run(&d, 2500);
    assert!(end);
}

#[test]
fn deterministic() {
    for d in [module_a(&[0x99, 0x40, 0x40], None), module_b(&[0x40])] {
        let (a, _) = run(&d, 120);
        let (b, _) = run(&d, 120);
        assert_eq!(a, b);
        assert!(a.len() > 100);
    }
}

#[test]
fn damaged_modules_do_not_panic() {
    let mut m = Machine::new(CpuType::M68000);
    for base in [module_a(&[0x99, 0x40, 0x40], None), module_b(&[0x40])] {
        // truncations
        for cut in (0..base.len()).step_by(37) {
            if let Some(mut r) = (OPEN.new)(&base[..cut], 0x10000, &mut m) {
                for k in 0..4 {
                    m.bus.clock += TICK * (k + 1);
                    r.tick(&mut Hw { bus: &mut m.bus });
                }
            }
        }
        // hostile bytes
        for o in (0x10..base.len()).step_by(7) {
            for v in [0xFFu8, 0x80, 0x01] {
                let mut d = base.clone();
                d[o] = v;
                if let Some(mut r) = (OPEN.new)(&d, 0x10000, &mut m) {
                    for k in 0..30 {
                        m.bus.clock += TICK * (k + 1);
                        r.tick(&mut Hw { bus: &mut m.bus });
                    }
                }
            }
        }
    }
}

#[test]
fn stream_end_follows_the_grammar() {
    let mut s = vec![0x41, 0x9B, 0x00, 0x20, 0xA0, 0];
    s.extend(std::iter::repeat_n(0u8, 15));
    // 16 row terminators: the first row ends at the zero after 0xA0; 15 more follow
    assert_eq!(stream_end(&s, 0, Var::A), Some(s.len()));
    assert_eq!(stream_end(&s[..10], 0, Var::A), None);
    let mut u = vec![0x9B, 0x01, 0];
    u.extend(std::iter::repeat_n(0u8, 15));
    assert_eq!(stream_end(&u, 0, Var::B), Some(u.len()));
}
