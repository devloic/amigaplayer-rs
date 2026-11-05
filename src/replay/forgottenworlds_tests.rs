use super::*;
use crate::engine::Engine;
use std::path::Path;

/// `FW_MODULE=file FW_TICKS=n FW_LOG=out cargo test --release fw_dump -- --ignored`:
/// the register log of `n` ticks of a module.
#[test]
#[ignore]
fn fw_dump() {
    let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("FW_MODULE"), std::env::var("FW_TICKS"), std::env::var("FW_LOG")) else {
        return;
    };
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..ticks.parse::<usize>().unwrap() {
        e.render(&mut buf);
    }
    let l = e.m.bus.write_log.take().unwrap();
    let text: String = l.iter().map(|(a, v, t)| format!("{a:06x} {v:04x} {t}\n")).collect();
    std::fs::write(out, text).unwrap();
}

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

/// A small synthetic module: one instrument; channel 0 plays a note and a rest, then ends.
fn module() -> Vec<u8> {
    let mut d = vec![0u8; 16 + 8 + 64 + 8 + 8 + 8 + 64];
    d[..4].copy_from_slice(b"FWMP");
    // the streams follow the four start words; patterns at 16+40, instruments at +48, directory at +56
    for (i, off) in [8u16, 20, 22, 24].iter().enumerate() {
        put16(&mut d, 16 + 2 * i, *off);
    }
    let (a, b, c) = (40u32, 48u32, 56u32);
    d[4..8].copy_from_slice(&a.to_be_bytes());
    d[8..12].copy_from_slice(&b.to_be_bytes());
    d[12..16].copy_from_slice(&c.to_be_bytes());
    // channel 0: instrument 0, note 12 for 3 ticks, rest 2, end
    let s = 16 + 8;
    d[s..s + 7].copy_from_slice(&[0x84, 0, 12, 3, 0x86, 2, 0x80]);
    for k in 1..4 {
        d[16 + [8usize, 20, 22, 24][k]] = 0x80;
    }
    // instrument 0: directory offset 0, sustain 0x20, attack 0x10, decay -4, hold 4, release -8
    let i = 16 + 48;
    d[i..i + 6].copy_from_slice(&[0, 0x20, 0x10, 0xFC, 4, 0xF8]);
    // waveform directory: data at 8, 4 words, loop at 4 bytes, 2 words
    let w = 16 + 56;
    put16(&mut d, w, 8);
    put16(&mut d, w + 2, 4);
    put16(&mut d, w + 4, 4);
    put16(&mut d, w + 6, 2);
    d
}

#[test]
fn detects_header() {
    assert!(detect(&module()));
    assert!(!detect(b"FWMX0000000000000000000000"));
    assert!(!detect(&[0u8; 8]));
}

#[test]
fn plays_a_note() {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("t.fw"), module()).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..12 {
        e.render(&mut buf);
    }
    let l = e.m.bus.write_log.take().unwrap();
    // the note's period write (after the period-1 write) and one DMACON 0x800F per tick
    assert!(l.iter().any(|(a, v, _)| *a == 0xDFF0A6 && *v == 856));
    assert!(l.iter().filter(|(a, v, _)| *a == 0xDFF096 && *v == 0x800F).count() >= 5);
}
