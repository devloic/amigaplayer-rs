use crate::engine::Engine;
use std::path::Path;

fn put16(d: &mut [u8], at: usize, v: u16) {
    d[at..at + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

const SONG: usize = 0x34;
const BLOCKARR: usize = SONG + 0x314;
const BLOCK: usize = BLOCKARR + 4;
const LINES: usize = 8;
const SMPLARR: usize = BLOCK + 8 + LINES * 4 * 4;
const SAMPLE: usize = SMPLARR + 8;
const SMPLEN: usize = 1000;
const SYNTH: usize = SAMPLE + 6 + SMPLEN;

/// An MMD1 module, tempo 125, 3 ticks a row, one 8-line block: track 0 plays
/// C-2 with instrument 1 (a 1000-byte one-shot sample), track 1 note 1 with
/// instrument 2 (a synth sound on one 16-word waveform) at finetune +1
/// (command 15).
fn module() -> Vec<u8> {
    let mut d = vec![0u8; SYNTH + 0x11c + 32];
    d[0..4].copy_from_slice(b"MMD1");
    let n = d.len() as u32;
    put32(&mut d, 4, n);
    put32(&mut d, 8, SONG as u32);
    put32(&mut d, 0x10, BLOCKARR as u32);
    put32(&mut d, 0x18, SMPLARR as u32);
    for i in 0..2 {
        put16(&mut d, SONG + 8 * i + 2, 1); // no loop
        d[SONG + 8 * i + 6] = 64;
    }
    put16(&mut d, SONG + 0x1f8, 1); // blocks
    put16(&mut d, SONG + 0x1fa, 1); // song length
    put16(&mut d, SONG + 0x2fc, 125);
    d[SONG + 0x2ff] = 0x10; // volumes in hex
    d[SONG + 0x301] = 3;
    for t in 0..16 {
        d[SONG + 0x302 + t] = 64;
    }
    d[SONG + 0x312] = 64;
    d[SONG + 0x313] = 2;
    put32(&mut d, BLOCKARR, BLOCK as u32);
    put16(&mut d, BLOCK, 4);
    put16(&mut d, BLOCK + 2, LINES as u16 - 1);
    d[BLOCK + 8..BLOCK + 12].copy_from_slice(&[13, 1, 0, 0]);
    d[BLOCK + 12..BLOCK + 16].copy_from_slice(&[1, 2, 0x15, 1]);
    put32(&mut d, SMPLARR, SAMPLE as u32);
    put32(&mut d, SMPLARR + 4, SYNTH as u32);
    put32(&mut d, SAMPLE, SMPLEN as u32);
    for k in 0..SMPLEN {
        d[SAMPLE + 6 + k] = (k * 7) as u8;
    }
    put16(&mut d, SYNTH + 4, 0xffff);
    put16(&mut d, SYNTH + 0xe, 2);
    put16(&mut d, SYNTH + 0x10, 2);
    d[SYNTH + 0x12] = 1;
    d[SYNTH + 0x13] = 1;
    put16(&mut d, SYNTH + 0x14, 1);
    d[SYNTH + 0x16..SYNTH + 0x18].copy_from_slice(&[0x30, 0xff]); // volume 48, end
    d[SYNTH + 0x96..SYNTH + 0x98].copy_from_slice(&[0, 0xff]); // waveform 0, end
    put32(&mut d, SYNTH + 0x116, 0x11a);
    put16(&mut d, SYNTH + 0x11a, 16);
    d
}

#[test]
fn plays_a_sample_and_a_synth_sound() {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("mod.test"), module()).unwrap();
    let mut out = vec![0f32; 2 * 24000];
    e.render(&mut out);
    let log = e.m.bus.write_log.take().unwrap();
    let p = e.playing.as_ref().unwrap();
    assert_eq!(p.format, "MED");
    let base = e.m.deli.list.first().unwrap().0;
    let reg = |a: u32| {
        log.iter()
            .filter(|w| w.0 == a)
            .map(|w| w.1)
            .collect::<Vec<_>>()
    };
    let loc = |ch: u32| {
        let (hi, lo) = (reg(0xDFF0A0 + 16 * ch), reg(0xDFF0A2 + 16 * ch));
        hi.iter()
            .zip(lo)
            .map(|(&h, l)| (h as u32) << 16 | l as u32)
            .collect::<Vec<_>>()
    };
    // track 0 (on each turn of the song): the sample from its start, its
    // words, C-2's period; then the player's silent word with length 0, ten
    // lines after the DMA went on
    let l0 = loc(0);
    assert!(l0.len() >= 4 && l0.len() % 2 == 0);
    assert!(l0.chunks(2).all(|c| c == [base + SAMPLE as u32 + 6, l0[1]]));
    assert_ne!(l0[1], l0[0]);
    assert!(reg(0xDFF0A4).chunks(2).all(|c| c == [SMPLEN as u16 / 2, 0]));
    assert!(reg(0xDFF0A6).iter().all(|&p| p == 428));
    assert!(reg(0xDFF0A8).iter().all(|&v| v == 64));
    let on = log
        .iter()
        .position(|w| w.0 == 0xDFF096 && w.1 == 0x8003)
        .unwrap();
    let loop0 = log.iter().rposition(|w| w.0 == 0xDFF0A2).unwrap();
    assert!(loop0 > on && log[loop0].2 - log[on].2 >= 9 * crate::paula::LINE);
    // track 1: the synth sound's waveform, length then start, at finetune +1
    // two octaves down (4 * 850), at its volume program's 48
    assert!(loc(1).iter().all(|&a| a == base + (SYNTH + 0x11c) as u32));
    assert!(reg(0xDFF0B4).iter().all(|&l| l == 16));
    assert!(reg(0xDFF0B6).iter().all(|&p| p == 3400));
    assert!(reg(0xDFF0B8).iter().all(|&v| v == 48));
    // tracks 2 and 3: no note; their DMA off on the first tick, period 0
    assert!(reg(0xDFF0C6).iter().all(|&p| p == 0));
    assert!(log.iter().any(|w| w.0 == 0xDFF096 && w.1 == 8));
}

#[test]
fn finetune_octaves_below_c1() {
    let p = super::periods();
    let t = |ft: i8| &p[(ft as u8 & 15) as usize * super::TABLE..];
    // finetune 0: ProTracker's octave times 4 and 2
    assert_eq!(t(0)[..3], [3424, 3232, 3048]);
    assert_eq!(t(0)[12], 1712);
    // the others in equal-tempered steps from 4 times C-1
    assert_eq!(t(1)[..4], [3400, 3209, 3029, 2859]);
    assert_eq!(t(-8)[3], 3051);
    assert_eq!(t(-7)[0], 3588);
    assert_eq!(t(1)[super::C1..super::C1 + 2], [850, 802]);
}

/// A MED 2.10 file, tempo2 3 and no version chunk: instrument 1 at volume 40,
/// one 4-track block of 64 lines (C-2 with instrument 1 and command 0C20 on
/// track 0, line 0), one 8-byte sample.
fn med4() -> Vec<u8> {
    let mut d = b"MED\x04".to_vec();
    d.extend([0x80, 0x80]); // one mask byte: instrument 1
    d.extend([0x4f, 1, b'a', 40]); // volume only
    d.extend(1u16.to_be_bytes()); // blocks
    d.extend(1u16.to_be_bytes()); // song length
    d.push(0);
    d.extend(125u16.to_be_bytes());
    d.extend([0, 0x08]); // transpose; flags: samples in the file
    d.extend(3u16.to_be_bytes());
    d.extend([0; 20]);
    d.extend([64; 16]);
    d.push(64);
    // the block: lines 0-31 by masks (line 0), lines 32-63 empty
    d.extend([13, 4, 63, 0, 4, 0x05]);
    d.extend(0x8000_0000u32.to_be_bytes());
    d.extend(0x8000_0000u32.to_be_bytes());
    // track 0: note 0x0d, instrument 1; command C, argument 20
    d.extend([0x80, 0xd1, 0x8c, 0x20]);
    d.extend((1u64 << 62).to_be_bytes()); // the samples in the file: instrument 1
    d.extend(8u32.to_be_bytes());
    d.extend(0u16.to_be_bytes());
    d.extend([1, 2, 3, 4, 5, 6, 7, 8]);
    d
}

#[test]
fn converts_med4() {
    let c = super::med4::to_mmd0(&med4()).unwrap();
    assert_eq!(&c[0..4], b"MMD0");
    let be32 = |o: usize| u32::from_be_bytes(c[o..o + 4].try_into().unwrap()) as usize;
    let song = be32(8);
    assert_eq!(c[song + 6], 40);
    assert_eq!(c[song + 0x301], 6);
    let blk = be32(be32(0x10));
    assert_eq!(c[blk..blk + 5], [4, 63, 0x0d, 0x1c, 0x20]);
    let smp = be32(be32(0x18));
    assert_eq!(smp % 8, 0);
    assert_eq!(c[smp..smp + 14], [0, 0, 0, 8, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(super::detect(&med4()));
}
