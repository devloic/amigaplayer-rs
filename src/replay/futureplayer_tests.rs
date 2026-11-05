use super::*;
use crate::engine::Engine;
use std::path::Path;

// ---------------------------------------------------------------------------------
// Synthetic modules, built from the specification's layout (no game data)

fn put32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

const SONG: usize = 0x140;
const TRACK: usize = 0x180;
const STREAM: usize = 0x1C0;
const SOUND: usize = 0x200;
const WAVESET: usize = 0x220;
const PARAMS: usize = 0x240;
const WAVE: usize = 0x2C0;
const DATA: usize = 0x300;

/// A one-voice module: the stream is `stream` (written at STREAM), the song table has one entry
/// with parameter word `param`, the timer byte is 16 and the speed 1.
fn build(stream: &[u8], param: u32) -> Vec<u8> {
    let mut img = vec![0u8; 0x800];
    put32(&mut img, 0, 0x70FF_4E75);
    img[4..12].copy_from_slice(b"F.PLAYER");
    put32(&mut img, 12, 0x100);
    put32(&mut img, 16, 0x110);
    put32(&mut img, 20, 0x120);
    img[0x100..0x105].copy_from_slice(b"Test\0");
    put32(&mut img, 32, SONG as u32);
    put32(&mut img, 36, param);
    put32(&mut img, SONG + 8, TRACK as u32);
    img[SONG + 24] = 1;
    img[SONG + 25] = 16;
    put32(&mut img, TRACK + 8, STREAM as u32);
    img[STREAM..STREAM + stream.len()].copy_from_slice(stream);
    put32(&mut img, SOUND + 8, WAVESET as u32);
    put32(&mut img, SOUND + 12, PARAMS as u32);
    img[WAVESET + 8] = 0;
    put32(&mut img, WAVESET + 10, WAVE as u32);
    put32(&mut img, WAVE + 8, DATA as u32);
    img[WAVE + 12..WAVE + 14].copy_from_slice(&32u16.to_be_bytes());
    img[PARAMS + 8] = 255;
    img[PARAMS + 18] = 255;
    img[PARAMS + 19] = 255;
    img[PARAMS + 21] = 255;
    img[PARAMS + 23] = 255;
    file(&img)
}

fn file(img: &[u8]) -> Vec<u8> {
    let words = img.len() / 4;
    let mut f = Vec::new();
    for l in [0x3F3u32, 0, 1, 0, 0, 0x4000_0000 | words as u32, 0x3E9, words as u32] {
        f.extend_from_slice(&l.to_be_bytes());
    }
    f.extend_from_slice(img);
    for l in [0x3ECu32, 0, 0x3F2] {
        f.extend_from_slice(&l.to_be_bytes());
    }
    f
}

/// Sound select, one note (index 24, 3 beats), end of voice.
fn simple_stream() -> Vec<u8> {
    let mut s = vec![0x81, 0];
    s.extend_from_slice(&(SOUND as u32).to_be_bytes());
    s.extend_from_slice(&[24, 3, 0x80, 0]);
    s
}

fn start(d: &[u8], buffers: usize) -> (Engine, Vec<(u32, u16, u64)>) {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("fp.test"), d.to_vec()).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..buffers {
        e.render(&mut buf);
    }
    let l = e.m.bus.write_log.clone().unwrap();
    (e, l)
}

// ---------------------------------------------------------------------------------

#[test]
fn detection_follows_the_header_rules() {
    let good = build(&simple_stream(), 0);
    assert!(detect(&good));
    for (o, v) in [(0usize, 0xFFu8), (20, 0), (32, 0), (36, b'X'), (67, 0)] {
        let mut t = good.clone();
        t[o] = v;
        if o == 67 {
            put32(&mut t, 64, 0);
        }
        assert!(!detect(&t), "offset {o}");
    }
    assert!(!detect(&[]));
    assert!(!detect(&[0u8; 200]));
    for n in (0..good.len()).step_by(7) {
        let _ = detect(&good[..n]);
    }
}

#[test]
fn the_period_table_has_97_descending_entries() {
    assert_eq!(PERIODS.len(), 97);
    assert!(PERIODS.windows(2).all(|w| w[0] > w[1]));
    assert_eq!((PERIODS[0], PERIODS[24], PERIODS[96]), (7232, 1808, 28));
}

#[test]
fn a_new_note_writes_in_the_specified_order_and_ends_the_song() {
    let d = build(&simple_stream(), 0);
    let (e, log) = start(&d, 6);
    // the host's start-up rounds first (DMA 0x000F), then the first tick's writes
    let i = log.iter().position(|w| w.0 == 0xDFF096 && w.1 == 1).expect("no retrigger");
    let w: Vec<(u32, u16)> = log[i..].iter().map(|w| (w.0, w.1)).take(9).collect();
    let want_addr = [0xDFF096, 0xDFF0A6, 0xDFF0A4, 0xDFF0A0, 0xDFF0A2, 0xDFF0A8, 0xDFF096];
    assert_eq!(w.iter().take(7).map(|x| x.0).collect::<Vec<_>>(), want_addr);
    assert_eq!(w[1].1, PERIODS[24]);
    assert_eq!(w[2].1, 16); // 32 bytes
    assert_eq!(w[5].1, 63); // full level, volume scale and master
    assert_eq!(w[6].1, 0x8001);
    // the wait for the beam before the channel starts: 9 raster lines at least
    assert!(log[i + 6].2 - log[i + 5].2 >= 9 * 227);
    // the voice ends after 3 beats: DMA off for channel 0, and the song is over
    assert!(e.songend());
    let ends = log.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 1).count();
    assert!(ends >= 2);
}

#[test]
fn ticks_are_spaced_by_the_timer_byte() {
    let d = build(&simple_stream(), 0);
    let (_, log) = start(&d, 6);
    let ons: Vec<u64> = log.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 1).map(|w| w.2).collect();
    // the retrigger of tick 0 and the voice end three ticks later: three intervals (plus the
    // sequencer and processing differences of the two ticks)
    assert!(ons.len() >= 2);
    let gap = ons[1] - ons[0];
    let interval = 5 * (256 * 16 + 1);
    assert!(gap > 2 * interval && gap < 4 * interval, "{gap}");
}

#[test]
fn output_is_deterministic() {
    let d = build(&simple_stream(), 0);
    let (_, a) = start(&d, 6);
    let (_, b) = start(&d, 6);
    assert_eq!(a, b);
    assert!(a.len() > 20);
}

#[test]
fn fade_in_starts_the_master_at_the_step() {
    // fade-in step 64: the first tick's volume uses master 64
    let d = build(&simple_stream(), 64 << 16);
    let (_, log) = start(&d, 3);
    let v = log.iter().find(|w| w.0 == 0xDFF0A8 && w.2 > 20000 && w.1 != 0).expect("no volume").1;
    // level 255 * scale 255 * master 64, bits 16-23, shifted by 2
    assert_eq!(v as u64, (((255u64 * 255 * 64) >> 16) & 0xFF) >> 2);
}

#[test]
fn a_song_without_voices_ends_at_once() {
    let mut img = vec![0u8; 0x400];
    put32(&mut img, 0, 0x70FF_4E75);
    img[4..12].copy_from_slice(b"F.PLAYER");
    put32(&mut img, 32, SONG as u32);
    img[SONG + 25] = 16;
    let (e, _) = start(&file(&img), 1);
    assert!(e.songend());
}

#[test]
fn out_of_range_values_do_not_panic_or_hang() {
    // a note above the period table, a command number above 14, a call to nowhere, a jump to
    // the end of the image
    let mut s = vec![0x81, 0];
    s.extend_from_slice(&(SOUND as u32).to_be_bytes());
    s.extend_from_slice(&[0xBF, 0, 127, 2, 0x86, 0, 0xFF, 0xFF, 0xFF, 0xF0, 0x87, 0, 0, 0, 0xFF, 0xF0]);
    let d = build(&s, 0);
    let _ = start(&d, 20);
    // an endless chain of no-op commands is cut off by the 255-command abort
    let mut s = vec![0x81, 0];
    s.extend_from_slice(&(SOUND as u32).to_be_bytes());
    s.extend(std::iter::repeat([0x85u8, 0]).take(300).flatten());
    let d = build(&s, 0);
    let _ = start(&d, 20);
}

#[test]
fn garbage_images_are_played_safely() {
    let good = build(&simple_stream(), 0);
    let mut seed = 0x1234_5678u32;
    for _ in 0..30 {
        let mut t = good.clone();
        for _ in 0..40 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = 36 + (seed >> 8) as usize % (t.len() - 36);
            t[o] = (seed >> 24) as u8;
        }
        if detect(&t) {
            let _ = start(&t, 6);
        }
    }
}
