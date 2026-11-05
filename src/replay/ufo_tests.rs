use super::*;
use crate::engine::Engine;
use std::path::{Path, PathBuf};

fn be(o: &mut Vec<u8>, v: u32) {
    o.extend(v.to_be_bytes());
}

/// A sample bank: `recs` are (length, loop offset, volume) for the 40 records (missing ones are
/// empty); the data are `i8` ramps.
fn bank(recs: &[(u32, u32, u8)]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut table = Vec::new();
    for i in 0..40 {
        let (len, lp, vol) = recs.get(i).copied().unwrap_or((0, 0, 0));
        be(&mut table, len);
        be(&mut table, lp);
        table.extend([0, vol]);
        data.extend((0..len).map(|k| (k as u8).wrapping_mul(3)));
    }
    let mut o = b"FORM".to_vec();
    be(&mut o, 0);
    o.extend(b"ADATBODY");
    be(&mut o, 0);
    o.extend(table);
    o.extend([0u8; 84]);
    o.extend(data);
    let n = o.len() as u32 - 8;
    o[4..8].copy_from_slice(&n.to_be_bytes());
    o
}

fn chan(track: u8, stream: &[u8]) -> Vec<u8> {
    let mut o = b"CHAN".to_vec();
    be(&mut o, stream.len() as u32 + 2);
    o.extend([0, track]);
    o.extend(stream);
    o
}

fn blck(stream: &[u8]) -> Vec<u8> {
    let mut o = b"BLCK".to_vec();
    be(&mut o, stream.len() as u32 + 4);
    o.extend(stream);
    o
}

fn bpm(b: u8) -> Vec<u8> {
    vec![b'B', b'P', b'M', b' ', 0, b]
}

fn inst(n: u8, lp: u32, vol: u8, data: &[u8]) -> Vec<u8> {
    let mut o = b"INST".to_vec();
    be(&mut o, data.len() as u32 + 16);
    o.extend([0, n]);
    be(&mut o, data.len() as u32);
    be(&mut o, lp);
    o.extend([0, vol]);
    o.extend(data);
    o
}

fn song(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    for c in chunks {
        body.extend(c.iter());
    }
    let mut o = b"FORM".to_vec();
    be(&mut o, body.len() as u32 + 12);
    o.extend(b"DDATBODY");
    be(&mut o, body.len() as u32);
    o.extend(body);
    o
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ufo_test_{}_{tag}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Run {
    log: Vec<(u32, u16, u64)>,
    end: bool,
    ticks: usize,
}

fn run_named(name: &str, tag: &str, d: &[u8], bank: Option<(&str, &[u8])>, max_ticks: usize) -> Option<Run> {
    let dd = dir(tag);
    let path = dd.join(name);
    std::fs::write(&path, d).unwrap();
    if let Some((n, b)) = bank {
        std::fs::write(dd.join(n), b).unwrap();
    }
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new(&path), d.to_vec()).ok()?;
    let mut buf = vec![0f32; 2 * 800];
    let mut end = false;
    let mut ticks = 0;
    for _ in 0..max_ticks {
        e.render(&mut buf);
        ticks += 1;
        if e.songend() {
            end = true;
            break;
        }
    }
    let log = e.m.bus.write_log.take().unwrap();
    let _ = std::fs::remove_dir_all(&dd);
    Some(Run { log, end, ticks })
}

fn run(tag: &str, d: &[u8], b: &[u8], max_ticks: usize) -> Run {
    run_named("t.mus", tag, d, Some(("t.BANK", b)), max_ticks).expect("plays")
}

fn writes(r: &Run, lo: u32, hi: u32) -> Vec<(u32, u16)> {
    r.log.iter().filter(|w| w.0 >= lo && w.0 < hi).map(|w| (w.0, w.1)).collect()
}

fn default_bank() -> Vec<u8> {
    bank(&[(64, 0, 0x30), (64, 16, 0x20), (0, 0, 0x10), (0, 4, 0x10)])
}

#[test]
fn detection() {
    let s = song(&[chan(0, &[0, 0xFF]), bpm(150)]);
    assert!(detect(&s));
    assert!(!detect(&[]));
    assert!(!detect(&s[..23]));
    let mut x = s.clone();
    x[8] = b'X';
    assert!(!detect(&x));
    let mut x = s.clone();
    x[20] = b'B'; // the first chunk must be a CHAN
    assert!(!detect(&x));
    // only this replayer takes it
    let names: Vec<&str> = crate::replay::REPLAYERS.iter().filter(|o| (o.detect)(&s)).map(|o| o.name).collect();
    assert_eq!(names, ["UFO"]);
}

#[test]
fn chunk_walk_and_limits() {
    let mut ins = [Inst::default(); 40];
    let s = song(&[chan(2, &[3, 0xFF]), blck(&[0, 0xFD]), blck(&[1, 0xFD]), inst(5, 4, 9, &[1, 2, 3, 4, 5, 6, 7, 8]), bpm(100), bpm(60), b"DRUM".to_vec(), vec![0; 84], b"END ".to_vec(), chan(1, &[0, 0xFF])]);
    let p = parse_song(&s, 0x1000, &mut ins);
    assert!(p.tracks[2].active);
    assert_eq!(p.tracks[2].init_wait, 3);
    assert_eq!(p.blocks.len(), 2);
    assert_eq!(p.bpm, 60);
    assert!(p.insts[5].looped);
    assert_eq!(p.insts[5].vol, 9);
    assert_eq!(p.insts[5].end - p.insts[5].start, 8);
    assert_eq!(p.insts[5].loop_start - p.insts[5].start, 4);
    // the chunk after the unknown id is not read
    assert!(!p.tracks[1].active);
    assert_eq!(p.counts, [1, 2, 1, 0]);
}

#[test]
fn varints() {
    let mut p = 0;
    assert_eq!(varint(&[0x05], &mut p), Some(5));
    let mut p = 0;
    assert_eq!(varint(&[0x81, 0x00], &mut p), Some(128));
    let mut p = 0;
    assert_eq!(varint(&[0xFF, 0x7F], &mut p), Some(16383));
    let mut p = 0;
    assert_eq!(varint(&[0x81], &mut p), None);
}

#[test]
fn one_note_write_sequence() {
    // instrument 0 (looped? no: loop offset 0), note 60 velocity 100 on track 0, ends later
    let s = song(&[chan(0, &[0, 0x90, 60, 100, 2, 0xFF]), bpm(150)]);
    let r = run("note", &s, &default_bank(), 40);
    assert!(r.end);
    let w = writes(&r, 0xDFF096, 0xDFF0E0);
    // DMA off, pointer (2 words), period, length, volume, DMA on, next-buffer length and pointer (2 words)
    let first: Vec<u32> = w.iter().take(10).map(|x| x.0).collect();
    assert_eq!(first, [0xDFF096, 0xDFF0A0, 0xDFF0A2, 0xDFF0A6, 0xDFF0A4, 0xDFF0A8, 0xDFF096, 0xDFF0A4, 0xDFF0A0, 0xDFF0A2]);
    assert_eq!(w[0].1, 1);
    assert_eq!(w[3].1, PERIODS[36]); // note 60 - 24
    assert_eq!(w[4].1, 32); // 64 bytes
    assert_eq!(w[5].1, (100 * 0x7F >> 8) as u16); // scaled velocity
    assert_eq!(w[6].1, 0x8001);
    assert_eq!(w[7].1, 2); // no loop: the silent sample
    // the pointer of the next buffer is the bank's first bytes, which are zero
    // (silent), and the first pointer is past the 504-byte header
    let hi = w[1].1 as u32;
    let lo = w[2].1 as u32;
    let hi2 = w[8].1 as u32;
    let lo2 = w[9].1 as u32;
    assert_eq!(((hi2 << 16) | lo2) + 504, (hi << 16) | lo);
}

#[test]
fn looped_instrument_next_buffer() {
    // program change to instrument 1 (length 64, loop offset 16)
    let s = song(&[chan(0, &[0, 0xC0, 1, 0, 0x90, 48, 127, 1, 0xFF]), bpm(150)]);
    let r = run("loop", &s, &default_bank(), 40);
    let w = writes(&r, 0xDFF096, 0xDFF0E0);
    assert_eq!(w[4].1, 32);
    assert_eq!(w[7].1, 24); // (64 - 16) / 2 words
    let start = ((w[1].1 as u32) << 16) | w[2].1 as u32;
    let lp = ((w[8].1 as u32) << 16) | w[9].1 as u32;
    assert_eq!(lp, start + 16);
}

#[test]
fn volume_stage_and_release() {
    // velocity 0x7F note then note off after 3 rows: the volume falls by half the sample volume
    // (0x30) each interrupt
    let s = song(&[chan(0, &[0, 0x90, 60, 127, 3, 0x80, 60, 0, 12, 0xFF]), bpm(150)]);
    let r = run("rel", &s, &default_bank(), 60);
    let vols: Vec<u16> = writes(&r, 0xDFF0A8, 0xDFF0A9).iter().map(|x| x.1).collect();
    // initial volume (127 * 127) >> 8 = 63, then held, then falling by 24 each time
    assert!(vols.contains(&63));
    let i = vols.iter().rposition(|&v| v == 63).unwrap();
    assert_eq!(&vols[i + 1..i + 4], &[39, 15, 0]);
}

#[test]
fn song_end_silences_and_stops() {
    let s = song(&[chan(0, &[0, 0x90, 60, 127, 1, 0xFF]), bpm(150)]);
    let r = run("end", &s, &default_bank(), 60);
    assert!(r.end);
    assert!(r.ticks < 10);
    // the last four writes are the volume stage of the stopped player: zero
    let w = writes(&r, 0xDFF0A8, 0xDFF0E0);
    let last: Vec<u16> = w.iter().rev().take(4).map(|x| x.1).collect();
    assert_eq!(last, [0, 0, 0, 0]);
}

#[test]
fn restart_event_signals_the_end() {
    let s = song(&[chan(0, &[0, 0xC0, 0x7E, 1, 0xFF]), bpm(150)]);
    let r = run("restart", &s, &default_bank(), 60);
    assert!(r.end);
}

#[test]
fn bend_rewrites_the_period() {
    // note 48, then bend +4 (data 0x14) and -3 (data 0x0D)
    let s = song(&[chan(0, &[0, 0x90, 48, 100, 1, 0xE0, 0x14, 1, 0xE0, 0x0D, 1, 0xFF]), bpm(150)]);
    let r = run("bend", &s, &default_bank(), 60);
    let per: Vec<u16> = writes(&r, 0xDFF0A6, 0xDFF0A7).iter().map(|x| x.1).collect();
    let n = 24usize;
    let p = PERIODS[n] as i32;
    let up = (p - PERIODS[n + 2] as i32) >> 4;
    let down = (PERIODS[n - 2] as i32 - p) >> 4;
    assert_eq!(per, [p as u16, (p - up * 4) as u16, (p + down * 3) as u16]);
}

#[test]
fn notes_outside_the_table_are_silent() {
    // note 120 is index 96 (past the 84 entries): period 0; the low notes fold up
    let s = song(&[chan(0, &[0, 0x90, 120, 100, 1, 0x90, 3, 100, 1, 0xFF]), bpm(150)]);
    let r = run("oob", &s, &default_bank(), 60);
    let per: Vec<u16> = writes(&r, 0xDFF0A6, 0xDFF0A7).iter().map(|x| x.1).collect();
    assert_eq!(per, [0, PERIODS[3]]);
}

#[test]
fn call_and_return_and_running_status() {
    // call block 0 (a note and a running-status note), then a note after the return
    let s = song(&[chan(0, &[0, 0xFE, 0, 1, 0x90, 40, 100, 1, 0xFF]), blck(&[0, 0x90, 50, 100, 0, 52, 100, 1, 0xFD]), bpm(150)]);
    let r = run("call", &s, &default_bank(), 60);
    let per: Vec<u16> = writes(&r, 0xDFF0A6, 0xDFF0A7).iter().map(|x| x.1).collect();
    assert_eq!(per, [PERIODS[26], PERIODS[28], PERIODS[16]]);
}

#[test]
fn embedded_instrument_and_no_sample() {
    // instrument 3 has no sample in the bank (note does nothing audible), instrument 7 is embedded
    let data: Vec<u8> = (0..20).collect();
    let s = song(&[
        chan(0, &[0, 0xC0, 3, 0, 0x90, 60, 100, 1, 0xC0, 7, 0, 0x90, 60, 100, 1, 0xFF]),
        inst(7, 0, 0x10, &data),
        bpm(150),
    ]);
    let r = run("emb", &s, &default_bank(), 60);
    let w = writes(&r, 0xDFF096, 0xDFF0A0);
    // the first note only turns the DMA off
    assert_eq!(w[0].1, 1);
    assert_eq!(w[1].1, 1);
    let l = writes(&r, 0xDFF0A4, 0xDFF0A5);
    assert_eq!(l[0].1, 10); // 20 bytes
}

#[test]
fn tempo_sets_rows_per_interrupt() {
    // B = 255: T = 102, so two rows in some interrupts; every row plays a note on voice 0
    let notes: Vec<u8> = [0u8, 0x90, 60, 100, 1].iter().copied().chain((0..40).flat_map(|_| [60u8, 100, 1])).chain([0xFF]).collect();
    let fast = song(&[chan(0, &notes), bpm(255)]);
    let slow = song(&[chan(0, &notes), bpm(150)]);
    let a = run("fast", &fast, &default_bank(), 100);
    let b = run("slow", &slow, &default_bank(), 100);
    assert!(a.ticks < b.ticks);
}

#[test]
fn bank_name_rules() {
    assert_eq!(bank_stem("x.mus"), Some("x."));
    assert_eq!(bank_stem("song.MUS"), Some("song."));
    assert_eq!(bank_stem("mus.x"), None);
    assert_eq!(bank_stem("mus"), None);
    assert_eq!(bank_stem(".mus"), None);
    assert_eq!(bank_stem("é.mus"), Some("é."));
}

#[test]
fn bank_fallback_to_smp_set_and_missing_bank() {
    let s = song(&[chan(0, &[0, 0x90, 60, 100, 1, 0xFF]), bpm(150)]);
    // `mus.x` takes SMP.set
    let r = run_named("mus.x", "smp", &s, Some(("SMP.set", &default_bank())), 40).expect("plays");
    assert!(r.end);
    assert!(!writes(&r, 0xDFF0A6, 0xDFF0A7).is_empty());
    // no bank at all: not started
    assert!(run_named("t.mus", "nobank", &s, None, 5).is_none());
    // an invalid bank (no FORM) is refused
    assert!(run_named("t.mus", "badbank", &s, Some(("t.BANK", &[0u8; 600])), 5).is_none());
}

#[test]
fn deterministic() {
    let s = song(&[chan(0, &[0, 0x90, 60, 100, 1, 0x90, 64, 90, 2, 0xFF]), chan(5, &[1, 0x90, 40, 100, 3, 0xFF]), bpm(130)]);
    let a = run("det1", &s, &default_bank(), 80);
    let b = run("det2", &s, &default_bank(), 80);
    assert_eq!(a.log, b.log);
}

#[test]
fn malformed_input_does_not_panic_or_hang() {
    let b = default_bank();
    // runaway: a call to a block that returns to a zero-delay call; a missing block; truncated data
    let looping = song(&[chan(0, &[0, 0xFE, 0, 0]), blck(&[0, 0xFE, 0, 0, 0xFD])]);
    let _ = run("loopy", &looping, &b, 5);
    let missing = song(&[chan(0, &[0, 0xFE, 9, 1])]);
    let r = run("missing", &missing, &b, 5);
    assert!(r.end);
    let trunc = song(&[chan(0, &[0, 0x90, 60])]);
    let _ = run("trunc", &trunc, &b, 5);
    // pseudo-random chunk bodies and sizes
    let mut seed = 0x1234_5678u32;
    let mut next = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 16) as u8
    };
    for k in 0..40 {
        let len = 20 + (next() as usize) % 90;
        let body: Vec<u8> = (0..len).map(|_| next()).collect();
        let mut s = song(&[chan(0, &[0]), chan(1, &body), blck(&body), inst(2, 3, 4, &body[..len / 2]), bpm(next())]);
        // damage a few bytes after the header
        for _ in 0..4 {
            let i = 24 + (next() as usize) % (s.len() - 24);
            s[i] = next();
        }
        let _ = run(&format!("fz{k}"), &s, &b, 30);
    }
    // directly: parse and construct with garbage chunk sizes
    let mut ins = [Inst::default(); 40];
    let mut g = song(&[chan(0, &[0, 0xFF])]);
    g[24..28].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
    let _ = parse_song(&g, 0, &mut ins);
}
