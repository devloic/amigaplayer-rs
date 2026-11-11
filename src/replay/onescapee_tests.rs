use super::*;
use crate::engine::Engine;
use std::path::Path;

/// A module of `len` bytes: the first 96 decoded bytes are zero (24 words of the first
/// accepted head), the rest a ramp, encoded with the key.
fn module(len: usize) -> Vec<u8> {
    (0..len)
        .map(|p| {
            let v = if p < 96 { 0 } else { (p % 251) as u8 };
            v ^ KEY[p & 3]
        })
        .collect()
}

fn words(w: &[u32]) -> Vec<u8> {
    let mut d: Vec<u8> = w.iter().flat_map(|x| x.to_be_bytes()).collect();
    d.resize(2048, 0);
    d
}

#[test]
fn detection_rule() {
    let a = W_A;
    let b = W_B;
    let mut w = vec![a; 24];
    assert!(detect(&words(&w))); // 24 x A, word 24 not examined
    w.extend([0x12345678]);
    assert!(detect(&words(&w)));
    // five A, an arbitrary word, nineteen B
    let mut w = vec![a; 5];
    w.push(0x11223344);
    w.extend(vec![b; 19]);
    assert!(detect(&words(&w)));
    // an arbitrary word then 24 B; and 25 B
    let mut w = vec![0x11223344];
    w.extend(vec![b; 24]);
    assert!(detect(&words(&w)));
    assert!(detect(&words(&vec![b; 25])));
    // 23 A, arbitrary, nothing / one B
    let mut w = vec![a; 23];
    w.push(0x11223344);
    assert!(!detect(&words(&w)));
    w.push(b);
    assert!(detect(&words(&w)));
    // 24 B then 0: only 23 of the 24 following words match
    let mut w = vec![b; 24];
    w.push(0);
    assert!(!detect(&words(&w)));
    // five A, arbitrary, eighteen B, 0
    let mut w = vec![a; 5];
    w.push(0x11223344);
    w.extend(vec![b; 18]);
    w.push(0);
    assert!(!detect(&words(&w)));
    assert!(!detect(&[0u8; 4096]));
}

#[test]
fn short_and_odd_files_are_safe() {
    for n in [0, 3, 99, 100, 1000, 1023] {
        assert!(!detect(&module(n)), "{n}");
    }
    assert!(detect(&module(1024)));
    assert!(detect(&module(1027)));
}

struct Run {
    log: Vec<(u32, u16, u64)>,
    songend_at: Option<usize>,
}

fn run(d: Vec<u8>, steps: usize) -> Run {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("x.dat"), d).unwrap();
    assert_eq!(e.playing.as_ref().unwrap().format, "onEscapee");
    let mut buf = vec![0f32; 2 * 960];
    let mut songend_at = None;
    for t in 0..steps {
        e.render(&mut buf);
        if songend_at.is_none() && e.songend() {
            songend_at = Some(t);
        }
    }
    Run { log: e.m.bus.write_log.take().unwrap(), songend_at }
}

fn writes(r: &Run, reg: u32) -> Vec<(u16, u64)> {
    r.log.iter().filter(|w| w.0 & 0xFFF == reg).map(|w| (w.1, w.2)).collect()
}

#[test]
fn init_and_interrupt_timing() {
    // 5000 bytes: three full buffers after the first, a 904-byte tail
    let r = run(module(5000), 400);
    let dma = writes(&r, 0x096);
    let e = dma[0].1;
    assert_eq!(dma[0].0, 0x800F);
    let first = r.log[0].2;
    assert_eq!(e - first, 211);
    // period, length and volumes
    assert!(writes(&r, 0x0A6).iter().take(1).all(|w| w.0 == 222));
    assert_eq!(writes(&r, 0x0A4)[0].0, 0x200);
    assert_eq!(writes(&r, 0x0A8)[0].0, 64);
    assert_eq!(writes(&r, 0x0C8)[0].0, 1);
    // interrupt entries: the DMA enable falls before the song start, so the engine's Paula moves
    // its pending first line with the start (`align_to_start`): the first interrupt is requested
    // 235 colour clocks after the DMA enable (before that change: 92, or 160 for a host that
    // serves it at the next output sample); between the first two handlers
    // are 227111, then 227328 each
    let clears: Vec<u64> = writes(&r, 0x09A).iter().filter(|w| w.0 == 0x0080).map(|w| w.1).collect();
    assert_eq!(clears[0] - e, 235);
    assert_eq!(clears[1] - clears[0], 227111);
    assert_eq!(clears[2] - clears[1], 227328);
    assert_eq!(clears[3] - clears[2], 227328);
    // INTREQ acknowledge 8 later, pointer alternation B1, B0, B1 ...
    let acks = writes(&r, 0x09C);
    assert_eq!(acks[0], (0x0080, clears[0] + 8));
    let lo = writes(&r, 0x0A2);
    assert_eq!(lo[0].0, lo[2].0);
    assert_eq!(lo[1].0, lo[3].0);
    assert_eq!(lo[1].0.wrapping_sub(lo[0].0), 0x400);
    // the first handler: 6641 colour clocks to the first pointer, 8 apart, INTENA at +40
    let ptr = writes(&r, 0x0A0);
    assert_eq!(ptr[1].1, clears[0] + 6641);
    assert_eq!(writes(&r, 0x0B2)[1].1, ptr[1].1 + 8);
    assert_eq!(writes(&r, 0x0D2)[1].1, ptr[1].1 + 24);
    let on: Vec<u64> = writes(&r, 0x09A).iter().filter(|w| w.0 == 0x8080).map(|w| w.1).collect();
    assert_eq!(on[1], ptr[1].1 + 40);
    // the interrupt that takes the 904-byte tail costs 120 + 15 * 30 more
    assert!(r.songend_at.is_some());
    assert_eq!(ptr[4].1 - clears[3], 6641 + 120 + 15 * 30);
}

#[test]
fn buffers_hold_the_decoded_stream() {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    let d = module(3000);
    e.play_data(Path::new("x.dat"), d.clone()).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    // after the first interrupt the second buffer holds chunk 1
    for _ in 0..2 {
        e.render(&mut buf);
    }
    let mut want = Vec::new();
    for p in 1024..2048 {
        want.push(d[p] ^ KEY[p & 3]);
    }
    let ram = &e.m.bus.ram;
    // find the 1024-byte run in chip memory: the buffers are contiguous, the second follows the first
    let hit = ram.windows(1024).position(|w| w == &want[..]);
    assert!(hit.is_some());
}

#[test]
fn exact_multiple_ends_one_interrupt_later() {
    // 4096 bytes: buffers 0..3 are data, the cleared buffer follows with the song end
    let r = run(module(4096), 400);
    let clears: Vec<u64> = writes(&r, 0x09A).iter().filter(|w| w.0 == 0x0080).map(|w| w.1).collect();
    let ptr = writes(&r, 0x0A0);
    // handlers 0 and 1 are full (6641), handler 2 is the last full buffer (6639), handler 3 clears
    assert_eq!(ptr[1].1 - clears[0], 6641);
    assert_eq!(ptr[2].1 - clears[1], 6641);
    assert_eq!(ptr[3].1 - clears[2], 6639);
    assert_eq!(ptr[4].1 - clears[3], 4064);
    assert!(r.songend_at.is_some());
}

#[test]
fn garbage_does_not_panic() {
    let mut d = module(2048);
    d.truncate(1029);
    let r = run(d, 100);
    assert!(r.songend_at.is_some());
}
