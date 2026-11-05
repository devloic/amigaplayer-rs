use super::*;
use crate::engine::Engine;
use std::path::Path;

// Synthetic modules built in code from the specification's layout (no game data).

fn pad(mut v: Vec<u8>) -> Vec<u8> {
    if v.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// A module from its nine chunks (the last one empty).
fn module(chunks: [Vec<u8>; 8]) -> Vec<u8> {
    let mut d = b"ADRVPACK".to_vec();
    for c in chunks.into_iter().chain([Vec::new()]) {
        let c = pad(c);
        d.extend_from_slice(&(c.len() as u32).to_be_bytes());
        d.extend_from_slice(&c);
    }
    d
}

/// A block of `rows` rows with tick multiplier 1; voice 0 plays the track rows from 0 at volume 63,
/// the other voices have block volume 0 (channel volume 0) and are muted.
fn block(rows: u8) -> Vec<u8> {
    let mut b = vec![1, rows];
    b.extend_from_slice(&[0x00, 0, 63, 0, 0, 0]);
    for _ in 0..3 {
        b.extend_from_slice(&[63, 0, 0, 0, 0, 0]);
    }
    b
}

/// An instrument record: event record 0 for the note and for the release, no envelopes, volume base 64.
fn instrument() -> Vec<u8> {
    let mut i = vec![0u8; 26];
    i[21] = 64;
    i
}

fn event(e: u32, w: u16) -> Vec<u8> {
    let mut v = e.to_be_bytes().to_vec();
    v.extend_from_slice(&w.to_be_bytes());
    v
}

/// One synth record with a single block of two rows (a note, then a rest); speed multiplier 1.
fn synth_module(e: u32, w: u16, extra_events: Vec<u8>) -> Vec<u8> {
    let seq = vec![0xA0, 1, 0x21, 0, 0x00, 0];
    let rows = vec![0x80, 0, 0x20, 0, 0x3F, 0, 0, 0];
    let mut ev = event(e, w);
    ev.extend(extra_events);
    module([seq, block(2), rows, vec![0], instrument(), vec![0, 0], ev, vec![0; 64]])
}

fn run(d: &[u8], n: usize) -> (Vec<(u32, u16, u64)>, Engine) {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if e.play_data(Path::new("x.aps"), d.to_vec()).is_err() {
        return (Vec::new(), e);
    }
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..n {
        e.render(&mut buf);
    }
    (e.m.bus.write_log.clone().unwrap_or_default(), e)
}

/// The audio-register writes (DMACON, INTENA, the channels) without the CIA and INTREQ.
fn audio(l: &[(u32, u16, u64)]) -> Vec<(u32, u16, u64)> {
    l.iter().copied().filter(|w| (0xDFF096..=0xDFF0DF).contains(&w.0) && w.0 != 0xDFF09C).collect()
}

#[test]
fn detects_the_magic_and_the_chunk_walk() {
    let d = synth_module(0x3000_0004, 0, Vec::new());
    assert!(detect(&d));
    assert!(!detect(&[]));
    assert!(!detect(&d[..30]));
    let mut t = d.clone();
    t[0] = b'X';
    assert!(!detect(&t));
    let mut t = d.clone();
    t[4] = b'Q';
    assert!(!detect(&t));
    // a chunk that ends past the end of the file
    let mut t = d.clone();
    t[8..12].copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
    assert!(!detect(&t));
    // the last chunk is cut short
    assert!(!detect(&d[..d.len() - 2]) || d.len() % 2 == 1);
}

#[test]
fn division_follows_the_overflow_rule() {
    assert_eq!(q16(100, 7), 14);
    assert_eq!(q16(-100, 7) as i16, -14);
    // the quotient does not fit 16 bits: the low word of the dividend
    assert_eq!(q16(447 * 100, 1), (44700u32) as u16);
    assert_eq!(q16(-32768, -1), 0x8000);
    assert_eq!(q16(70000, 1), (70000u32 & 0xFFFF) as u16);
    // a zero divisor never happens; it must not panic
    assert_eq!(q16(5, 0), 5);
}

#[test]
fn halve_transform_follows_the_specification() {
    let i: Vec<u8> = (0..8).map(|k| (k * 16) as u8).collect();
    let o = halve(&i);
    // O[i] = I[i] >> 1, then O[i] += I[2i] >> 1 (first half), O[h+i] += I[2i] >> 2
    let sh = |x: u8, s: u32| ((x as i8) >> s) as u8;
    for k in 0..8 {
        let mut want = sh(i[k], 1);
        if k < 4 {
            want = want.wrapping_add(sh(i[2 * k], 1));
        } else {
            want = want.wrapping_add(sh(i[2 * (k - 4)], 2));
        }
        assert_eq!(o[k], want, "byte {k}");
    }
}

#[test]
fn period_table_edges() {
    assert_eq!(period_at(0), 1710);
    assert_eq!(period_at(47), 113);
    assert_eq!(period_at(50), 128);
    assert_eq!(period_at(53), 1024);
    assert_eq!(period_at(48), 0);
    assert_eq!(period_at(200), 0);
}

#[test]
fn start_and_first_tick_write_sequence() {
    // synth record: mode 0, operation 0 (pulse), 4 words (8 bytes), no comb, no filter
    let d = synth_module(0x3000_0004, 0, Vec::new());
    let (l, _) = run(&d, 4);
    let a = audio(&l);
    assert!(!a.is_empty());
    // start: DMACON 0x8200, then voice 0's row: LEN, LOC (two words), INTENA disable, DMACON on
    assert_eq!((a[0].0, a[0].1), (0xDFF096, 0x8200));
    assert_eq!((a[1].0, a[1].1), (0xDFF0A4, 4));
    assert_eq!(a[2].0, 0xDFF0A0);
    assert_eq!(a[3].0, 0xDFF0A2);
    assert_eq!((a[4].0, a[4].1), (0xDFF09A, 0x0080));
    assert_eq!((a[5].0, a[5].1), (0xDFF096, 0x8001));
    // the first tick: voice 0 PER (note 0 = 1710) and VOL (63), then the buffer address; the other
    // voices have channel volume 0 and only write VOL 0
    let per = a.iter().position(|w| w.0 == 0xDFF0A6).expect("no PER write");
    assert_eq!(a[per].1, 1710);
    assert_eq!((a[per + 1].0, a[per + 1].1), (0xDFF0A8, 63));
    assert_eq!(a[per + 2].0, 0xDFF0A0);
    assert_eq!((a[per + 4].0, a[per + 4].1), (0xDFF0B8, 0));
    assert_eq!((a[per + 5].0, a[per + 5].1), (0xDFF0C8, 0));
    assert_eq!((a[per + 6].0, a[per + 6].1), (0xDFF0D8, 0));
    // the tick points the voice at a buffer inside the synth area
    let loc_hi = |k: usize| ((a[k].1 as u32) << 16) | a[k + 1].1 as u32;
    assert_eq!(loc_hi(per + 2) % 2, 0);
}

#[test]
fn song_end_writes_intena_and_dmacon_once() {
    let d = synth_module(0x3000_0004, 0, Vec::new());
    let (l, e) = run(&d, 8);
    assert!(e.songend());
    let a = audio(&l);
    let ends: Vec<_> = a.iter().filter(|w| w.0 == 0xDFF09A && w.1 == 0x0780).collect();
    assert_eq!(ends.len(), 1);
    let k = a.iter().position(|w| w.0 == 0xDFF09A && w.1 == 0x0780).unwrap();
    assert_eq!((a[k + 1].0, a[k + 1].1), (0xDFF096, 0x000F));
}

#[test]
fn pulse_generator_fills_the_buffer() {
    // with every envelope at 0 the pulse position is 0: all bytes are -100
    let d = synth_module(0x3000_0004, 0, Vec::new());
    let (l, e) = run(&d, 3);
    let a = audio(&l);
    let per = a.iter().position(|w| w.0 == 0xDFF0A6).unwrap();
    let loc = ((a[per + 2].1 as u32) << 16) | a[per + 3].1 as u32;
    // the voice's buffer written this tick is the one the previous LOC did not point at
    let prev = ((a[2].1 as u32) << 16) | a[3].1 as u32;
    let _ = loc;
    for i in 0..8 {
        assert_eq!(e.m.bus.r8(prev + i) as i8, -100, "byte {i}");
    }
}

#[test]
fn sample_record_starts_the_sound_at_the_row() {
    // a looped sample record (type 0): the row turns DMA off, the key requests the audio
    // interrupt, whose handler writes the sample address and length and turns DMA on
    let d = {
        let seq = vec![0xA0, 1, 0x21, 0, 0x00, 0];
        let rows = vec![0x80, 0, 0x20, 0, 0x3F, 0, 0, 0];
        module([seq, block(2), rows, vec![0], instrument(), vec![0, 0], event(0x0000_0008, 16), vec![5; 64]])
    };
    let (l, _) = run(&d, 3);
    let a = audio(&l);
    assert_eq!((a[0].0, a[0].1), (0xDFF096, 0x8200));
    // DMA off for voice 0, then the key: INTENA enable and AUDxDAT
    assert_eq!((a[1].0, a[1].1), (0xDFF096, 0x0001));
    assert_eq!((a[2].0, a[2].1), (0xDFF09A, 0x8080));
    assert_eq!((a[3].0, a[3].1), (0xDFF0AA, 0));
    // the handler: disable, LOC (offset 8 words = 16 bytes into the sample chunk), LEN, INTENA, DMA on
    assert_eq!((a[4].0, a[4].1), (0xDFF09A, 0x0080));
    assert_eq!(a[5].0, 0xDFF0A0);
    assert_eq!((a[7].0, a[7].1), (0xDFF0A4, 16));
    assert_eq!((a[8].0, a[8].1), (0xDFF09A, 0x0080));
    assert_eq!((a[9].0, a[9].1), (0xDFF096, 0x8001));
}

#[test]
fn sequence_jump_budget_ends_the_song_without_hanging() {
    // a jump to itself: 255 jumps, then the song ends (met during the start: nothing is written)
    let d = module([vec![0x60, 0], block(1), vec![0; 8], vec![0], instrument(), vec![0, 0], event(0x3000_0004, 0), vec![0; 8]]);
    let (l, e) = run(&d, 3);
    assert!(e.songend());
    assert!(audio(&l).iter().all(|w| w.1 != 0x8200));
    // a loop entry with count 0 loops forever by itself; the budget ends it
    let d = module([vec![0x40, 0], block(1), vec![0; 8], vec![0], instrument(), vec![0, 0], event(0x3000_0004, 0), vec![0; 8]]);
    let (_, e) = run(&d, 3);
    assert!(e.songend());
}

#[test]
fn render_is_deterministic() {
    let d = synth_module(0x3000_0004, 0, Vec::new());
    let (a, _) = run(&d, 12);
    let (b, _) = run(&d, 12);
    assert_eq!(a, b);
    assert!(a.windows(2).all(|w| w[1].2 >= w[0].2), "times never decrease");
}

#[test]
fn tolerates_garbage_without_panicking() {
    let d = synth_module(0x3011_2284, 0x1234, event(0x3022_3300, 0x00FF));
    let mut seed = 0x1357_9BDFu32;
    for _ in 0..60 {
        let mut t = d.clone();
        for _ in 0..16 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = 8 + (seed >> 8) as usize % (t.len() - 8);
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            t[o] = (seed >> 16) as u8;
        }
        // chunk lengths may now be wrong; the module is then not recognised, which is fine
        let _ = run(&t, 6);
    }
}

#[test]
fn comb_with_a_length_not_a_multiple_of_four_is_skipped() {
    // n = 6 bytes (E & 63 = 3 words) with a comb record: the original never leaves the stage; here it is skipped
    let comb = event(0x0000_0000, 0);
    let d = synth_module(0x3000_0103, 0, comb);
    let (l, _) = run(&d, 3);
    assert!(!audio(&l).is_empty());
}
