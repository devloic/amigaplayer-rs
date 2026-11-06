//! Tests on small streams built in code: the header rules, the period, the decoder (with an
//! independent bit-by-bit reference written from the specification), the start and refill write
//! sequences and buffers, the partial last block, the song end and the restart, and malformed
//! input.

use super::*;
use m68k::CpuType;

/// A pseudo-random byte stream.
fn bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}

/// A file: the header (kind, rate) and `n` pseudo-random code bytes.
fn file(three: bool, rate: u32, n: usize, seed: u32) -> Vec<u8> {
    let mut d = b"ADPCM".to_vec();
    d.push(if three { b'3' } else { b'2' });
    d.extend_from_slice(&rate.to_be_bytes());
    d.extend(bytes(n, seed));
    d
}

fn start(data: &[u8]) -> (Machine, Option<Box<dyn Replayer>>) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let r = new(data, 0, &mut m);
    (m, r)
}

fn irq(r: &mut Box<dyn Replayer>, m: &mut Machine) {
    assert!(r.audio_irq_ch(0, &mut Hw { bus: &mut m.bus }));
}

fn ram(m: &Machine, a: u32, n: usize) -> Vec<u8> {
    m.bus.ram[a as usize..a as usize + n].to_vec()
}

/// The location written by the k-th write to the register `reg` pair, as 32-bit values.
fn locs(m: &Machine, ch: u32) -> Vec<u32> {
    let log = m.bus.write_log.as_ref().unwrap();
    let hi: Vec<u32> = log.iter().filter(|w| w.0 == 0xDFF0A0 + 16 * ch).map(|w| w.1 as u32).collect();
    let lo: Vec<u32> = log.iter().filter(|w| w.0 == 0xDFF0A2 + 16 * ch).map(|w| w.1 as u32).collect();
    hi.iter().zip(&lo).map(|(a, b)| a << 16 | b).collect()
}

/// The codes of a stream in order, read as the specification says (a bit stream, most
/// significant bit first; 3-bit codes in 3-byte groups, a missing byte reads as zero).
fn codes(d: &[u8], three: bool, from: usize, nbytes: usize) -> Vec<u32> {
    let w = if three { 3 } else { 2 };
    let mut bits = Vec::new();
    for i in 0..nbytes {
        let b = d.get(from + i).copied().unwrap_or(0);
        for k in (0..8).rev() {
            bits.push((b >> k & 1) as u32);
        }
    }
    bits.chunks(w).filter(|c| c.len() == w).map(|c| c.iter().fold(0, |a, &b| a << 1 | b)).collect()
}

/// The decoder of the specification's section 2, written out in signed 64-bit arithmetic.
struct Ref {
    s: i64,
    p: i64,
}

impl Ref {
    fn run(&mut self, cs: &[u32], three: bool) -> Vec<u8> {
        if self.s == 0 {
            self.s = 5;
        }
        cs.iter()
            .map(|&c| {
                let (mut dd, m, neg) = if three {
                    let mag = (c & 3) as usize;
                    (self.s / 2 + mag as i64 * self.s, [0x399A, 0x3A9F, 0x4D14, 0x6607][mag], c & 4 != 0)
                } else {
                    let b = c & 1;
                    (self.s / 2 + b as i64 * self.s, if b != 0 { 0x5600 } else { 0x3800 }, c & 2 != 0)
                };
                dd = dd.rem_euclid(65536);
                if neg {
                    dd = (-dd).rem_euclid(65536);
                }
                self.p = (self.p + dd).rem_euclid(65536);
                let signed = if self.p >= 32768 { self.p - 65536 } else { self.p };
                self.s = ((self.s * m + 0x2000) / 16384).rem_euclid(65536);
                (signed.div_euclid(64)).clamp(-128, 127) as i8 as u8
            })
            .collect()
    }
}

/// What the channels play: the silent block, then `blocks` blocks of the stream by the
/// specification's refill rules (one fill after the other, state kept between them).
fn expected(d: &[u8], blocks: usize) -> Vec<u8> {
    let three = d[5] == b'3';
    let s = &d[10..];
    let b = if three { 384 } else { 256 };
    let mut out = vec![0u8; 1024];
    let (mut pos, mut st) = (0usize, Ref { s: 0, p: 0 });
    for _ in 0..blocks {
        if pos >= s.len() {
            pos = 0;
            st = Ref { s: 0, p: 0 };
        }
        let r = s.len() - pos;
        let mut blk;
        if r < b {
            let n = 3 * (r / 3);
            let n = if three { n.max(3) } else { n };
            blk = st.run(&codes(s, three, pos, n), three);
            blk.resize(1024, 0);
            pos = 0;
            st = Ref { s: 0, p: 0 };
        } else {
            blk = st.run(&codes(s, three, pos, b), three);
            pos += b;
        }
        out.extend(blk);
    }
    out
}

/// Runs the player for `irqs` refills and returns the bytes that channel 0 plays: the buffers
/// in the order they were pointed at (the first, silent one included).
fn played(d: &[u8], irqs: usize) -> (Machine, Vec<u8>) {
    let (mut m, r) = start(d);
    let mut r = r.unwrap();
    let a = *locs(&m, 0).last().unwrap();
    let mut out = ram(&m, a, 1024);
    for _ in 0..irqs {
        irq(&mut r, &mut m);
        // (the buffer just pointed at has been filled in the same call)
        let a = *locs(&m, 0).last().unwrap();
        out.extend(ram(&m, a, 1024));
    }
    (m, out)
}

#[test]
fn header_rules() {
    assert!(detect(&file(false, 8526, 10, 1)));
    assert!(detect(&file(true, 16726, 0, 1)));
    for bad in [&b"ADPCM1"[..], b"ADPCM4", b"ADPC", b"adpcm2", b"ADPCM", b"XDPCM2"] {
        let mut d = bad.to_vec();
        d.extend_from_slice(&[0, 0, 0x20, 0x00, 0, 0, 0, 0]);
        assert!(!detect(&d), "{:?}", bad);
    }
    // the header is ten bytes
    assert!(!detect(b"ADPCM2\0\0\0"));
    assert!(!detect(&[]));
    assert!(detect(b"ADPCM2\0\0\x20\x00"));
}

#[test]
fn coexists_with_the_headerless_format() {
    use crate::replay::{adpcmmono, REPLAYERS};
    let who = |d: &[u8], name: &str| {
        adpcmmono::set_name(name);
        let r = REPLAYERS.iter().find(|o| (o.detect)(d)).map(|o| o.name);
        adpcmmono::set_name("");
        r
    };
    // a file with the header is this format whatever its name, a file named like the raw one is not
    for name in ["x.ADPCM", "x.adpcm", "dancing.adpcm", "ParaMiAmiga1.PCM", ""] {
        assert_eq!(who(&file(true, 16726, 800, 3), name), Some(OPEN.name), "{name}");
        assert_eq!(who(&file(false, 8526, 800, 4), name), Some(OPEN.name), "{name}");
    }
    // the raw stream with such a name stays the other's (unless it starts with the magic)
    let raw = bytes(600, 5);
    if REPLAYERS.iter().filter(|o| o.name != adpcmmono::OPEN.name).all(|o| !(o.detect)(&raw)) {
        assert_eq!(who(&raw, "x.ADPCM"), Some(adpcmmono::OPEN.name));
    }
    assert!(!(adpcmmono::OPEN.detect)(&file(false, 8526, 800, 4)));
}

#[test]
fn multipliers_match_the_specification_data() {
    assert_eq!([MUL2[0], MUL2[1], MUL3[0], MUL3[1], MUL3[2], MUL3[3]], [0x3800, 0x5600, 0x399A, 0x3A9F, 0x4D14, 0x6607]);
    // the specification's data file, where the folder exists
    let p = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/spec/adpcm23/data/step_multipliers.bin";
    if let Ok(b) = std::fs::read(p) {
        let w: Vec<u32> = b.chunks(2).map(|c| (c[0] as u32) << 8 | c[1] as u32).collect();
        assert_eq!(w, [MUL2.as_slice(), MUL3.as_slice()].concat());
    }
}

#[test]
fn period_division() {
    assert_eq!(period(16726), Some(212));
    assert_eq!(period(8526), Some(416));
    assert_eq!(period(22050), Some(160));
    // only the low 16 bits of the rate count
    assert_eq!(period(0x0001_0000 + 8526), Some(416));
    assert_eq!(period(0), None);
    assert_eq!(period(0x1_0000), None);
    // a rate below 55 Hz overflows the division: the low 16 bits of the dividend
    assert_eq!(period(50), Some(FIVE_E as u16));
    assert_eq!(period(55), Some((FIVE_E / 55) as u16));
}

#[test]
fn decoder_hand_values() {
    // 2-bit, step 5, code 0: delta 2, new step (5 * 0x3800 + 0x2000) >> 14 = 4
    let mut d = Dec { s: 5, p: 0 };
    assert_eq!(d.code(0b00, false), 0);
    assert_eq!((d.s, d.p), (4, 2));
    // 3-bit, step 5, magnitude 3: delta 2 + 15 = 17, new step (5 * 0x6607 + 0x2000) >> 14 = 8
    let mut d = Dec { s: 5, p: 0 };
    d.code(0b011, true);
    assert_eq!((d.s, d.p), (8, 17));
    // the sign bit subtracts
    let mut d = Dec { s: 5, p: 0 };
    d.code(0b111, true);
    assert_eq!(d.p as i16, -17);
    // 2-bit with the magnitude bit: delta 2 + 5 = 7, new step (5 * 0x5600 + 0x2000) >> 14 = 7
    let mut d = Dec { s: 5, p: 0 };
    d.code(0b01, false);
    assert_eq!((d.s, d.p), (7, 7));
    // then a negative one at step 7: delta 3 + 7 = 10
    d.code(0b11, false);
    assert_eq!(d.p as i16, 7 - 10);
    // the step 0 of a call becomes 5 (the first call, or after a reset)
    let mut a = Adpcm { d: vec![0], three: false, pos: 0, dec: Dec::default(), bufs: [0; 2], next: 0, refills: 0, songend: false };
    let mut buf = [0u8; BUF];
    a.decode(&mut buf, 1);
    assert_eq!((a.dec.s, a.dec.p), (4, 8), "four zero codes from step 5: deltas 2 each, the step stays 4");
}

#[test]
fn decoder_wraps_and_clamps() {
    // no clamping of the predictor: a big step wraps it from positive to negative
    let mut d = Dec { s: 20000, p: 30000 };
    let o = d.code(0b001, true);
    // delta = 10000 + 20000 = 30000: 60000 wraps to -5536, output -87
    assert_eq!(d.p as i16, -5536);
    assert_eq!(o as i8, -87);
    // output clamps to a signed byte
    let mut d = Dec { s: 100, p: 20000 };
    assert_eq!(d.code(0b000, true) as i8, 127);
    let mut d = Dec { s: 100, p: (-20000i16) as u16 };
    assert_eq!(d.code(0b100, true) as i8, -128);
    // the delta itself is kept to 16 bits
    let mut d = Dec { s: 65535, p: 0 };
    d.code(0b011, true);
    assert_eq!(d.p, ((32767u32 + 3 * 65535) & 0xFFFF) as u16);
    // the step: 65535 * 0x6607 does not overflow 32 bits
    assert_eq!(d.s, ((65535u64 * 0x6607 + 0x2000) >> 14) as u32 & 0xFFFF);
}

#[test]
fn bit_order_of_the_codes() {
    // 2-bit: four codes a byte, the top pair first; 3-bit: eight codes in three bytes
    for three in [false, true] {
        let d = file(three, 8000, 24, 9);
        let mut a = Adpcm { d: d[10..].to_vec(), three, pos: 0, dec: Dec::default(), bufs: [0; 2], next: 0, refills: 0, songend: false };
        let mut buf = [0u8; BUF];
        a.decode(&mut buf, 24);
        let n = if three { 64 } else { 96 };
        let want = Ref { s: 0, p: 0 }.run(&codes(&d[10..], three, 0, 24), three);
        assert_eq!(want.len(), n);
        assert_eq!(buf[..n], want[..], "three={three}");
    }
}

#[test]
fn decoder_matches_the_reference_over_random_streams() {
    for (three, seed) in [(false, 1u32), (false, 2), (true, 3), (true, 4)] {
        let d = file(three, 12000, 4000, seed);
        let blocks = 4000 / if three { 384 } else { 256 };
        let (_, got) = played(&d, blocks);
        assert_eq!(got.len(), 1024 * (blocks + 1));
        let want = expected(&d, blocks);
        assert_eq!(got, want[..got.len()], "three={three} seed={seed}");
    }
}

#[test]
fn start_writes_and_timing() {
    for (three, at) in [(false, 3028u64), (true, 3176)] {
        let d = file(three, 16726, 2000, 6);
        let (m, r) = start(&d);
        let r = r.unwrap();
        assert_eq!(r.format(), if three { "ADPCM3" } else { "ADPCM2" });
        assert!(!r.songend());
        let log = m.bus.write_log.clone().unwrap();
        let regs: Vec<(u32, u16)> = log.iter().map(|&(a, v, _)| (a, v)).collect();
        let t: Vec<u64> = log.iter().map(|&(_, _, t)| t).collect();
        // DMA off, channel 0 and 1: location (two words), length, period, volume; (enable); DMA on
        assert_eq!(regs[0], (0xDFF096, 0x0003));
        assert_eq!(t[0], at);
        let a = (regs[1].1 as u32) << 16 | regs[2].1 as u32;
        for ch in 0..2u32 {
            let o = 1 + 5 * ch as usize;
            let b = 0xDFF0A0 + 16 * ch;
            assert_eq!([regs[o].0, regs[o + 1].0, regs[o + 2].0, regs[o + 3].0, regs[o + 4].0], [b, b + 2, b + 4, b + 6, b + 8]);
            assert_eq!((regs[o].1 as u32) << 16 | regs[o + 1].1 as u32, a);
            assert_eq!([regs[o + 2].1, regs[o + 3].1, regs[o + 4].1], [0x200, 212, 64]);
        }
        assert_eq!(regs[11], (0xDFF096, 0x8003));
        assert_eq!(regs.len(), 12);
        // the gaps of the specification's table
        assert_eq!(t[1] - t[0], 7787);
        assert_eq!(t[2], t[1], "both words of a location at one clock");
        assert_eq!([t[3] - t[1], t[4] - t[3], t[5] - t[4], t[6] - t[5]], [89, 95, 85, 91]);
        assert_eq!([t[8] - t[6], t[9] - t[8], t[10] - t[9], t[11] - t[10]], [89, 95, 85, 102]);
        // the buffers are zero
        assert!(ram(&m, a, 2048).iter().all(|&x| x == 0));
    }
}

#[test]
fn refill_timing_and_alternation() {
    let d = file(false, 8526, 2000, 7);
    let (mut m, r) = start(&d);
    let mut r = r.unwrap();
    // the first request comes at the start of the DMA
    let t0 = m.bus.clock;
    m.bus.write_log.as_mut().unwrap().clear();
    irq(&mut r, &mut m);
    let first = m.bus.write_log.clone().unwrap();
    assert_eq!(first.len(), 4);
    assert_eq!(first[0].2, t0 + FIRST_LATENCY);
    assert_eq!(first[2].2, first[0].2 + CH_GAP);
    m.bus.write_log.as_mut().unwrap().clear();
    let t1 = m.bus.clock;
    irq(&mut r, &mut m);
    let log = m.bus.write_log.clone().unwrap();
    assert_eq!(log[0].2, t1 + IRQ_LATENCY);
    assert_eq!(log[2].2, log[0].2 + 19);
    assert_eq!([log[0].0, log[1].0, log[2].0, log[3].0], [0xDFF0A0, 0xDFF0A2, 0xDFF0B0, 0xDFF0B2]);
    assert_eq!((log[0].1, log[1].1), (log[2].1, log[3].1));
    // buffers: B (after the zero one A), A, B, A, ...
    for _ in 0..4 {
        irq(&mut r, &mut m);
    }
    let l = locs(&m, 0);
    // (the log was cleared after the first refill, which took B: A, B, A, B, A)
    assert_eq!(l.len(), 5);
    assert_eq!(l[1], l[0] + 1024);
    for k in 0..5 {
        assert_eq!(l[k], l[k % 2], "{k}");
    }
    assert_eq!(l, locs(&m, 1));
    // only channel 0's interrupt is taken
    assert!(!r.audio_irq_ch(1, &mut Hw { bus: &mut m.bus }));
    assert!(!r.audio_irq_ch(3, &mut Hw { bus: &mut m.bus }));
}

#[test]
fn stream_with_whole_blocks_restarts_without_a_gap() {
    for three in [false, true] {
        let b = if three { 384 } else { 256 };
        let d = file(three, 8526, 4 * b, 8);
        let (m, got) = played(&d, 10);
        let want = expected(&d, 10);
        assert_eq!(got[..], want[..got.len()]);
        // the fifth block is the first again, with a fresh state
        assert_eq!(got[1024..2048], got[1024 + 4 * 1024..2048 + 4 * 1024]);
        drop(m);
        // the song end is announced during the fill of the restarted first block
        let (mut m, r) = start(&d);
        let mut r = r.unwrap();
        for k in 0..4 {
            irq(&mut r, &mut m);
            assert!(!r.songend(), "after {k}");
        }
        irq(&mut r, &mut m);
        assert!(r.songend());
    }
}

#[test]
fn partial_last_block() {
    // ADPCM2: 1 whole block and 160 bytes more: 3 * 53 = 159 decoded, 636 samples, then zeros
    let d = file(false, 8526, 256 + 160, 9);
    let (m, got) = played(&d, 3);
    let want = expected(&d, 3);
    assert_eq!(got, want[..got.len()]);
    let last = &got[2048..3072];
    assert!(last[636..].iter().all(|&x| x == 0));
    assert!(last[..636].iter().any(|&x| x != 0));
    drop(m);
    let (mut m, r) = start(&d);
    let mut r = r.unwrap();
    irq(&mut r, &mut m);
    assert!(!r.songend());
    irq(&mut r, &mut m);
    assert!(r.songend(), "reported in the fill of the partial block");
    // ADPCM3: 99 of 100 bytes
    let d = file(true, 8526, 384 + 100, 10);
    let (_, got) = played(&d, 3);
    assert_eq!(got, expected(&d, 3)[..got.len()]);
    assert!(got[2048 + 264..3072].iter().all(|&x| x == 0), "99 bytes: 264 samples");
    assert!(got[2048..2048 + 264].iter().any(|&x| x != 0));
    // ADPCM3 with 1 or 2 bytes left: one group is still decoded, reading zeros past the end
    for extra in [1, 2] {
        let d = file(true, 8526, 384 + extra, 11);
        let (_, got) = played(&d, 3);
        assert_eq!(got, expected(&d, 3)[..got.len()], "{extra}");
        assert!(got[2048 + 8..3072].iter().all(|&x| x == 0));
        // and the next fill starts over, with a fresh state
        assert_eq!(got[3072..4096], got[1024..2048]);
    }
    // ADPCM2 with 1 or 2 bytes left: nothing at all is played from them
    for extra in [1, 2] {
        let d = file(false, 8526, 256 + extra, 12);
        let (_, got) = played(&d, 2);
        assert!(got[2048..3072].iter().all(|&x| x == 0), "{extra}");
    }
}

#[test]
fn restart_resets_the_state() {
    let d = file(false, 8526, 300, 13);
    let (_, got) = played(&d, 5);
    // blocks 1, 2 (partial), 3 = block 1, 4 = block 2
    assert_eq!(got[1024..2048], got[3072..4096]);
    assert_eq!(got[2048..3072], got[4096..5120]);
}

#[test]
fn degenerate_streams_are_safe() {
    // no code bytes: silence, the song end announced
    for three in [false, true] {
        let d = file(three, 8526, 0, 14);
        let (mut m, r) = start(&d);
        let mut r = r.unwrap();
        for _ in 0..4 {
            irq(&mut r, &mut m);
        }
        assert!(r.songend());
        let l = locs(&m, 0);
        for a in l {
            assert!(ram(&m, a, 1024).iter().all(|&x| x == 0));
        }
    }
    // rate 0 and too short a file are refused
    assert!(start(&file(false, 0, 100, 15)).1.is_none());
    assert!(start(&file(false, 0x1_0000, 100, 15)).1.is_none());
    assert!(start(b"ADPCM2\0\0\x20").1.is_none());
    assert!(start(&[]).1.is_none());
    // very low and very high rates start
    assert!(start(&file(true, 1, 100, 16)).1.is_some());
    assert!(start(&file(true, 0xFFFF_FFFF, 100, 16)).1.is_some());
}

#[test]
fn fuzz_headers_and_lengths_do_not_panic() {
    for (i, n) in (0..40).chain([255, 256, 257, 383, 384, 385, 767, 768, 769, 1500]).enumerate() {
        for three in [false, true] {
            let d = file(three, 100 + 977 * i as u32, n, i as u32);
            let (mut m, r) = start(&d);
            let mut r = r.unwrap();
            for _ in 0..8 {
                irq(&mut r, &mut m);
            }
        }
    }
}

#[test]
fn deterministic() {
    let d = file(true, 16726, 3000, 17);
    let (a, x) = played(&d, 6);
    let (b, y) = played(&d, 6);
    assert_eq!(x, y);
    assert_eq!(a.bus.write_log, b.bus.write_log);
}
