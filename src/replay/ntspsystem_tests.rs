use super::*;
use crate::engine::Engine;
use std::path::Path;

/// A module of stream length `n`: the magic, the length and pseudo-random block bytes whose
/// header bytes never have shift 0 (so the shift-0 cost is out of the picture).
fn module(n: u32) -> Vec<u8> {
    let mut d = b"SPNT".to_vec();
    d.extend(n.to_be_bytes());
    let mut s = 0x1234_5678u32;
    for i in 0..(5 * n as usize / 9 + 5) {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        let mut b = (s >> 24) as u8;
        if i % 5 == 0 && b >> 5 == 0 {
            b |= 0x20;
        }
        d.push(b);
    }
    d
}

#[test]
fn block_decoding_examples() {
    assert_eq!(block([0x6B, 0x12, 0xF3, 0x80, 0x7F]), [0x2C, 0x08, 0x10, 0xF8, 0x18, 0xC0, 0x00, 0x38, 0xF8]);
    assert_eq!(block([0x1F, 0x9A, 0x00, 0x00, 0x00]), [0xFF, 0xF9, 0xFA, 0, 0, 0, 0, 0, 0]);
    // shift 7 keeps the low eight bits only; the first value is shifted by six
    let y = block([0xE1, 0x10, 0, 0, 0]);
    assert_eq!(y[0], 64);
    assert_eq!(y[1], 0x80);
}

#[test]
fn detection() {
    assert!(detect(&module(1800)));
    assert!(detect(&module(16)));
    assert!(!detect(&module(15)));
    assert!(!detect(&module(0)));
    let mut d = module(3600);
    d[0] = b'X';
    assert!(!detect(&d));
    for n in 0..8 {
        assert!(!detect(&module(3600)[..n]));
    }
    assert!(!detect(&[]));
    // other formats are not taken
    assert!(!detect(b"TME\0\0\0\0\0\0\0\0\0"));
    assert!(!detect(&[0u8; 4096]));
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
    assert_eq!(e.playing.as_ref().unwrap().format, "NTSP-system");
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
fn startup_writes() {
    let r = run(module(5400), 100);
    let t = r.log[0].2;
    let seq: Vec<(u32, u16, u64)> = r.log.iter().take(22).map(|w| (w.0 & 0xFFF, w.1, w.2 - t)).collect();
    // four location pairs 8 apart
    for ch in 0..4u32 {
        assert_eq!(seq[2 * ch as usize].0, 0x0A0 + 16 * ch);
        assert_eq!(seq[2 * ch as usize].2, 8 * ch as u64);
        assert_eq!(seq[2 * ch as usize + 1].0, 0x0A2 + 16 * ch);
    }
    let lens: Vec<u64> = seq[8..12].iter().map(|w| w.2).collect();
    assert_eq!(lens, [42, 48, 54, 60]);
    assert!(seq[8..12].iter().all(|w| w.1 == 900));
    let pers: Vec<u64> = seq[12..16].iter().map(|w| w.2).collect();
    assert_eq!(pers, [70, 76, 82, 88]);
    assert!(seq[12..16].iter().all(|w| w.1 == 161));
    let vols: Vec<(u16, u64)> = seq[16..20].iter().map(|w| (w.1, w.2)).collect();
    assert_eq!(vols, [(64, 96), (64, 102), (1, 110), (1, 116)]);
    assert_eq!((seq[20].0, seq[20].1, seq[20].2), (0x09A, 0x8080, 201));
    assert_eq!((seq[21].0, seq[21].1, seq[21].2), (0x096, 0x800F, 211));
}

#[test]
fn interrupt_group_and_alternation() {
    let r = run(module(5400), 75);
    let intena = writes(&r, 0x09A);
    let off: Vec<u64> = intena.iter().filter(|w| w.0 == 0x0080).map(|w| w.1).collect();
    assert!(off.len() >= 4);
    assert_eq!(off[2] - off[1], 289_800);
    assert_eq!(off[3] - off[2], 289_800);
    let ack = writes(&r, 0x09C);
    assert_eq!(ack[0], (0x0080, off[0] + 8));
    let lo = writes(&r, 0x0A2);
    // the start-up location, then buffers Y, X, Y ...
    assert_eq!(lo[1].0.wrapping_sub(lo[0].0), 1800);
    assert_eq!(lo[2].0, lo[0].0);
    assert_eq!(lo[3].0, lo[1].0);
    let ptr = writes(&r, 0x0A0);
    // the first handler decodes a full chunk (no shift-0 block): 51202 after the INTREQ write
    assert_eq!(ptr[1].1, off[0] + 8 + 51_202);
    assert_eq!(writes(&r, 0x0D2)[1].1, ptr[1].1 + 24);
    let on: Vec<u64> = intena.iter().filter(|w| w.0 == 0x8080).map(|w| w.1).collect();
    assert_eq!(on[1], ptr[1].1 + 40);
    // 5400 = three chunks: the third interrupt hits the end of stream with nothing to decode
    assert_eq!(ptr[3].1, off[2] + 8 + 20_011);
    assert!(r.songend_at.is_some());
}

#[test]
fn partial_chunk_cost() {
    // 3600 + 450: the third buffer is a 450-byte chunk (50 blocks) and 1350 cleared
    let r = run(module(4050), 75);
    let off: Vec<u64> = writes(&r, 0x09A).iter().filter(|w| w.0 == 0x0080).map(|w| w.1).collect();
    let ptr = writes(&r, 0x0A0);
    let want = 211 + 11 * 1350 + 207 + 509 * 50 / 2;
    assert_eq!(ptr[2].1 - off[1] - 8, want);
}

#[test]
fn buffer_contents_tail_and_loop() {
    let n = 4000u32;
    let d = module(n);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("x.dat"), d.clone()).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..2 {
        e.render(&mut buf);
    }
    let dec = |k: usize| block(std::array::from_fn(|i| d[8 + 5 * k + i]));
    let mut want = Vec::new();
    for k in 0..200 {
        want.extend(dec(k));
    }
    let ram = &e.m.bus.ram;
    let x = ram.windows(1800).position(|w| w == &want[..]).expect("first chunk in memory");
    // the second chunk is in the buffer after it
    let mut want2 = Vec::new();
    for k in 200..400 {
        want2.extend(dec(k));
    }
    assert_eq!(&ram[x + 1800..x + 3600], &want2[..]);
    // the third (400 bytes: 44 blocks, 4 bytes not written) goes into the first buffer again,
    // over the old data; the song end is reported and the cleared part is zero
    for _ in 0..8 {
        if e.songend() {
            break;
        }
        e.render(&mut buf);
    }
    assert!(e.songend());
    let ram = &e.m.bus.ram;
    for k in 0..44 {
        assert_eq!(&ram[x + 9 * k..x + 9 * k + 9], &dec(400 + k)[..]);
    }
    assert_eq!(&ram[x + 396..x + 400], &want[396..400]);
    assert!(ram[x + 400..x + 1800].iter().all(|&b| b == 0));
}

#[test]
fn short_and_truncated_modules_are_safe() {
    // a length under one buffer, a stream far longer than the file, a huge length
    for (n, keep) in [(16u32, 100usize), (500, 8), (1799, 1000), (10_000, 30), (0xFFFF_FFF0, 64)] {
        let mut d = module(n.min(20_000));
        d[4..8].copy_from_slice(&n.to_be_bytes());
        d.truncate(keep.max(8));
        let r = run(d, 120);
        assert!(r.log.len() >= 22);
    }
}
