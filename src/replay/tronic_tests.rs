use super::*;
use crate::engine::Engine;
use std::path::Path;

/// `TR_MODULE=file TR_TICKS=n TR_LOG=out cargo test --release dump -- --ignored`:
/// the register log of `n` ticks of a module.
#[test]
#[ignore]
fn dump() {
    let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("TR_MODULE"), std::env::var("TR_TICKS"), std::env::var("TR_LOG")) else {
        return;
    };
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
    println!("format {}", e.playing.as_ref().unwrap().format);
    let mut buf = vec![0f32; 2 * 960];
    let n: usize = ticks.parse().unwrap();
    for _ in 0..n {
        e.render(&mut buf);
    }
    let l = e.m.bus.write_log.take().unwrap();
    let text: String = l.iter().map(|(a, v, t)| format!("{a:06x} {v:04x} {t}\n")).collect();
    std::fs::write(out, text).unwrap();
}

/// `TR_FILES=a,b,c cargo test --release sweep -- --ignored --nocapture`: which open replayers
/// accept each file, in registration order.
#[test]
#[ignore]
fn sweep() {
    let Ok(files) = std::env::var("TR_FILES") else { return };
    for f in files.split(',') {
        let d = std::fs::read(f).unwrap();
        let names: Vec<&str> = crate::replay::REPLAYERS.iter().filter(|o| (o.detect)(&d)).map(|o| o.name).collect();
        println!("{f}: {names:?}");
    }
}

fn w(o: &mut Vec<u8>, v: u16) {
    o.extend(v.to_be_bytes());
}

/// A small module: patterns of 16 rows (`rows` fill the first rows of pattern 0); lists of
/// `list_len` bytes per voice (all entries pattern 0, transpose 0); two arpeggio lists; one
/// wave-list instrument and one sample instrument; two waves; eight sample table entries and
/// a 64-byte sample area.
fn module(rows: &[[u8; 4]], list_len: u16) -> Vec<u8> {
    let mut o = Vec::new();
    for _ in 0..4 {
        o.extend([0, 0]);
        w(&mut o, list_len);
    }
    for _ in 0..4 {
        for _ in 0..list_len / 2 {
            o.extend([0, 0]);
        }
    }
    // block A: two lists
    w(&mut o, 32);
    o.extend([0, 4, 7, 12, 0, 4, 7, 12, 0, 4, 7, 12, 0, 4, 7, 12]);
    o.extend([0, 3, 7, 10, 0, 3, 7, 10, 0, 3, 7, 10, 0, 3, 7, 0x80]);
    // block B: two instruments
    w(&mut o, 4);
    w(&mut o, 0x58);
    w(&mut o, 0xB0);
    // block C: record 0 (unused) and two instruments
    let inst = |sample: bool| {
        let mut r = vec![0u8; 0x58];
        r[1] = 4;
        r[0x04] = 0;
        r[0x05] = 2;
        // envelope phase 0: rate 8, target 60, hold 2; phase 1: rate 2, target 20, hold 255
        r[6] = 8;
        r[7] = 60;
        r[8] = 2;
        r[9] = 2;
        r[10] = 20;
        r[11] = 255;
        r[12] = 0;
        r[13] = 20;
        r[14] = 255;
        // vibrato: speed 1, length 3, delay 4
        r[0x15] = 1;
        r[0x16] = 3;
        r[0x17] = 4;
        r[0x26] = if sample { 0x80 } else { 0 };
        r[0x27] = if sample { 1 } else { 2 };
        if !sample {
            r[0x28] = 0;
            r[0x29] = 1;
            r[0x2A] = 0xFF;
            r[0x2B] = 0;
        }
        r
    };
    o.extend(vec![0u8; 0x58]);
    o.extend(inst(false));
    o.extend(inst(true));
    // block D: two waves
    w(&mut o, 4);
    w(&mut o, 0);
    w(&mut o, 8);
    // block E: 16 bytes of wave data
    w(&mut o, 16);
    o.extend((0..16).map(|i| (i * 8) as u8));
    // block F: one pattern
    o.extend(64u32.to_be_bytes());
    let mut pat = vec![0u8; 64];
    for (i, r) in rows.iter().enumerate() {
        pat[i * 4..i * 4 + 4].copy_from_slice(r);
    }
    o.extend(pat);
    // block G: eight sample offsets
    for k in 0..8u32 {
        o.extend((k * 8).to_be_bytes());
    }
    // sample area
    o.extend((0..64).map(|i| (i * 3) as u8));
    o
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("t.dp"), d.to_vec()).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    let mut end = false;
    for _ in 0..ticks {
        e.render(&mut buf);
        end |= e.songend();
    }
    (e.m.bus.write_log.take().unwrap(), end)
}

#[test]
fn layout_of_small_module() {
    let d = module(&[[1, 1, 0, 0]], 32);
    let l = layout(&d).expect("recognised");
    assert_eq!(l.seqlen, [32; 4]);
    assert!(detect(&d));
}

#[test]
fn detect_rejects_garbage() {
    assert!(!detect(&[]));
    assert!(!detect(&[0u8; 100]));
    assert!(!detect(&[0xFF; 4096]));
    let d = module(&[[1, 1, 0, 0]], 32);
    assert!(!detect(&d[..d.len() - 90]));
    let mut d = module(&[[1, 1, 0, 0]], 32);
    let l = layout(&d).unwrap();
    d[l.b + 1] ^= 0x10; // the first instrument offset is no longer 0x58
    assert!(!detect(&d));
}

#[test]
fn every_tick_has_volumes_and_the_dma_write() {
    let d = module(&[[13, 1, 0, 0], [0, 0, 0, 0], [20, 2, 5, 3]], 32);
    let (log, _) = run(&d, 40);
    let n_dma = log.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 0x800F).count();
    assert!(n_dma >= 38, "{n_dma}");
    let vols = log.iter().filter(|w| w.0 == 0xDFF0A8).count();
    assert!(vols >= 38);
}

#[test]
fn deterministic_and_bounded_on_noise() {
    let d = module(&[[13, 1, 3, 0xF0], [200, 7, 8, 0xFF], [250, 0, 9, 1], [1, 1, 6, 0xFF], [30, 2, 7, 0xFF]], 32);
    let a = run(&d, 120);
    let b = run(&d, 120);
    assert_eq!(a.0, b.0);
    // an arpeggio list that starts with the end marker, and a stray list number
    let mut d = module(&[[13, 1, 8, 1], [14, 2, 8, 200]], 32);
    let l = layout(&d).unwrap();
    d[l.a] = 0x80;
    d[l.a + 16] = 0x80;
    let _ = run(&d, 60);
}

#[test]
fn song_end_after_the_list_wraps() {
    let d = module(&[[13, 1, 0, 0]], 2);
    let (_, end) = run(&d, 80);
    assert!(end);
}

#[test]
fn first_row_is_processed_in_tick_six() {
    let d = module(&[[13, 1, 0, 0]], 32);
    let (log, _) = run(&d, 10);
    // the first note's DMACON write (channel bit alone) falls in the sixth tick
    let first = log.iter().position(|w| w.0 == 0xDFF096 && w.1 == 1).expect("note start");
    let ticks_before = log[..first].iter().filter(|w| w.0 == 0xDFF096 && w.1 == 0x800F).count();
    assert_eq!(ticks_before, 5);
}
