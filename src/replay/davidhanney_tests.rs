use super::*;
use crate::engine::Engine;
use std::path::Path;

/// A synthetic module: `blocks` of rows of 12 bytes, the order `seq`, the speed.
fn module(blocks: &[Vec<u8>], seq: &[u8], speed: u16) -> Vec<u8> {
    let mut o = b"DSNG".to_vec();
    o.extend(b"SEQU");
    o.extend(256u32.to_be_bytes());
    o.extend(seq);
    o.extend(vec![0u8; 256 - seq.len()]);
    o.extend(b"INFO");
    o.extend(10u32.to_be_bytes());
    for w in [seq.len() as u16, seq.len() as u16, speed, 0, 0] {
        o.extend(w.to_be_bytes());
    }
    for (i, b) in blocks.iter().enumerate() {
        o.extend(b"BLK");
        o.push(i as u8);
        o.extend((2 + b.len() as u32).to_be_bytes());
        o.extend(((b.len() / 12) as u16).to_be_bytes());
        o.extend(b);
    }
    o.extend([0u8; 4]);
    o
}

fn song() -> Vec<u8> {
    let mut b0 = vec![0u8; 12 * 8];
    b0[0..3].copy_from_slice(&[40, 3, 0]);
    b0[3..6].copy_from_slice(&[44, 0, 0x32]);
    b0[12 * 2 + 6..12 * 2 + 9].copy_from_slice(&[30, 1, 0]);
    b0[12 * 4..12 * 4 + 3].copy_from_slice(&[1, 0, 0]);
    module(&[b0.clone(), b0], &[0, 1], 3)
}

fn play(d: Vec<u8>, ticks: usize) -> Option<(Vec<(u32, u16, u64)>, bool)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/dh-test/dh.test"), d).ok()?;
    if e.playing.as_ref()?.format != "David Hanney" {
        return None;
    }
    let mut out = vec![0f32; 2 * 960];
    let mut end = false;
    for _ in 0..=ticks {
        e.render(&mut out);
        end |= e.songend();
    }
    Some((e.m.bus.write_log.take().unwrap(), end))
}

#[test]
fn parses_chunks() {
    let d = song();
    let c = chunks(&d).unwrap();
    assert_eq!(c.len(), 4);
    assert_eq!(&c[0].0, b"SEQU");
    assert_eq!(c[0].2, 256);
    assert_eq!(&c[3].0, b"BLK\x01");
    assert_eq!(c[2].2, 2 + 12 * 8);
}

#[test]
fn detects_by_chunk_order() {
    let d = song();
    assert!(detect(&d));
    assert!(!detect(&[]));
    assert!(!detect(&d[..20]));
    // one block only is refused
    let one = module(&[vec![0u8; 24]], &[0], 3);
    assert!(!detect(&one));
    let mut e = d.clone();
    e[0] = b'X';
    assert!(!detect(&e));
    let mut e = d.clone();
    e[4] = b'X';
    assert!(!detect(&e));
}

#[test]
fn embedded_data_has_the_bank() {
    assert_eq!(DATA.len(), BANK_OFF + 16 * REC);
    assert_eq!(&DATA[SMP_BYTES..SMP_BYTES + 4], b"iblk");
    // instruments 0..2 are the samples, the rest synth
    for (i, t) in [1u16, 2, 3, 0, 0].into_iter().enumerate() {
        assert_eq!(be16(DATA, BANK_OFF + i * REC), t);
    }
}

#[test]
fn renders_deterministically() {
    let (a, _) = play(song(), 40).unwrap();
    let (b, _) = play(song(), 40).unwrap();
    assert!(a.len() > 20);
    assert_eq!(a, b);
    // a synth note starts with the channel's DMA off, the wave location and its length
    let dma: Vec<u16> = a.iter().filter(|w| w.0 == 0xDFF096).map(|w| w.1).collect();
    assert_eq!(dma[0], 0x000F);
    assert!(dma.contains(&0x8001));
}

#[test]
fn song_end_is_reported() {
    let (_, end) = play(song(), 80).unwrap();
    assert!(end);
}
