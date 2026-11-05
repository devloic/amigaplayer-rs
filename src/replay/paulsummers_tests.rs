use super::*;
use crate::engine::Engine;
use std::path::Path;

/// `PS_MODULE=file PS_TICKS=n PS_LOG=out [PS_SUBSONG=k] [PS_PHASE=c] cargo test --release dump -- --ignored`:
/// the register log of `n` ticks of a module (the first tick call on raster phase `c`).
#[test]
#[ignore]
fn dump() {
    let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("PS_MODULE"), std::env::var("PS_TICKS"), std::env::var("PS_LOG")) else {
        return;
    };
    if let Some(p) = std::env::var("PS_PHASE").ok().and_then(|s| s.parse::<u64>().ok()) {
        PHASE.store(p, Ordering::Relaxed);
    }
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
    let sub: u32 = std::env::var("PS_SUBSONG").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    for _ in 0..sub {
        e.change_subsong(1).unwrap();
    }
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

// ---------------------------------------------------------------------------------
// Synthetic modules (built here from the specification's layout; no game data)

const SONG: usize = 2000;
const PER: usize = 1400;
const INS: usize = 2400;
const SAMPLE: usize = 2600;
const STREAM: usize = 3100;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A module with `subs` song-table entries 1.., voice streams `streams[v]` (None: unused voice)
/// placed from `STREAM` on, `speed` and `timer` in every entry.
fn module(streams: [Option<Vec<u8>>; 4], subs: usize, speed: i8, timer: u16) -> Vec<u8> {
    let mut d = vec![0u8; 4400];
    // the driver's code as far as the detection and the table location read it
    put32(&mut d, 650, 0x46FC_2700);
    put16(&mut d, 654, 0x4E73);
    put16(&mut d, 656, 0x41FA);
    put16(&mut d, 660, 0x41FA);
    put16(&mut d, 662, (-662i16) as u16);
    put16(&mut d, 664, 0x45FA);
    put16(&mut d, 666, ((SONG + 20 * (subs + 1)) as i64 - 666) as u16);
    put16(&mut d, 668, 0xC0FC);
    put16(&mut d, 674, (SONG as i64 - 674) as u16);
    put16(&mut d, 676, 0x4480);
    put16(&mut d, 680, (PER as i64 - 680) as u16);
    // period table: index i at PER - 4 * i, 233.0 at index 0, 2^(1/12) between neighbours
    for i in -30i64..=60 {
        let p = (233.0 * 65536.0 * 2f64.powf(-(i as f64) / 12.0)) as u32;
        put32(&mut d, (PER as i64 - 4 * i) as usize, p);
    }
    // instrument table: one entry for every note, base note 48, 8 words of data
    put16(&mut d, INS, 1);
    put32(&mut d, INS + 2, (SAMPLE - (INS + 2)) as u32);
    put32(&mut d, INS + 6, 8);
    put16(&mut d, INS + 10, 48);
    put16(&mut d, INS + 12, 0);
    put16(&mut d, INS + 14, 127);
    for b in d[SAMPLE..SAMPLE + 16].iter_mut() {
        *b = 0x40;
    }
    let mut at = STREAM;
    for s in 1..=subs {
        let e = SONG + 20 * s;
        let mut at2 = at;
        for (v, st) in streams.iter().enumerate() {
            if let Some(st) = st {
                put32(&mut d, e + 4 * v, (at2 - SONG) as u32);
                d[at2..at2 + st.len()].copy_from_slice(st);
                at2 += st.len();
            }
        }
        d[e + 16] = speed as u8;
        put16(&mut d, e + 18, timer);
        at = at2;
    }
    d
}

/// The stream prologue: select the instrument table at `INS`, from a command at `at`.
fn ins_cmd(at: usize) -> Vec<u8> {
    let mut v = vec![0x86];
    v.extend((((INS as i64) - (at as i64 + 1)) as i32).to_be_bytes());
    v
}

fn one_voice_stream() -> Vec<u8> {
    let mut s = ins_cmd(STREAM);
    s.extend([48, 2]); // note 48, two ticks
    s.extend([0xAE, 40]); // volume 40
    s.extend([50, 1]); // note 50, one tick
    s.extend([0x80, 1]); // rest one tick
    s.push(0x8F);
    s
}

fn song() -> Vec<u8> {
    module([Some(one_voice_stream()), None, None, None], 1, 0, 0)
}

type Log = Vec<(u32, u16, u64)>;

fn play(d: Vec<u8>, ticks: usize) -> Option<(Log, bool)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/ps-test/snk.test"), d).ok()?;
    if e.playing.as_ref()?.format != "Paul Summers" {
        return None;
    }
    let mut out = vec![0f32; 2 * 960];
    let mut end = false;
    for _ in 0..ticks {
        e.render(&mut out);
        end |= e.songend();
    }
    Some((e.m.bus.write_log.take().unwrap(), end))
}

/// The log split into ticks (a gap of more than 20000 colour clocks starts one); the start
/// sequence is the first group.
fn groups(l: &Log) -> Vec<Vec<(u32, u16, u64)>> {
    let mut g: Vec<Vec<(u32, u16, u64)>> = Vec::new();
    let mut last = 0;
    for &w in l {
        if g.is_empty() || w.2 - last > 20000 {
            g.push(Vec::new());
        }
        last = w.2;
        g.last_mut().unwrap().push(w);
    }
    g
}

#[test]
fn detects_the_driver() {
    let d = song();
    assert!(detect(&d));
    let mut bad = d.clone();
    put16(&mut bad, 662, 0); // the pointer to the start of the file is gone
    assert!(!detect(&bad));
    assert!(!detect(&d[..2900]));
    assert!(!detect(&vec![0u8; 4000]));
}

#[test]
fn detects_the_old_form() {
    let mut d = vec![0u8; 4400];
    put16(&mut d, 654, 0x41FA);
    for o in [658, 664, 670, 676] {
        put32(&mut d, o, 0x117C_0088);
    }
    put16(&mut d, 682, 0x41FA);
    put16(&mut d, 684, (-684i16) as u16);
    assert!(detect_code(&d));
    put32(&mut d, 670, 0);
    assert!(!detect_code(&d));
}

#[test]
fn locates_the_tables() {
    let l = locate(&song()).unwrap();
    assert_eq!((l.song, l.per, l.subsongs), (SONG, PER, 1));
    let l = locate(&module([Some(one_voice_stream()), None, None, None], 3, 0, 0)).unwrap();
    assert_eq!(l.subsongs, 3);
}

#[test]
fn start_sequence_and_song_start() {
    let (l, _) = play(song(), 6).unwrap();
    let g = groups(&l);
    // the host's start: volumes twice, then DMA off
    let regs: Vec<u32> = g[0].iter().map(|w| w.0).collect();
    assert_eq!(
        regs,
        vec![0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8, 0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8, 0xDFF096]
    );
    assert_eq!(g[0][8].1, 0x000F);
    assert_eq!(g[0][8].2 - g[0][0].2, 6240);
    // the first tick: DMA off, four volumes 0, DMA off, then voice 0's fetch (a DMA stop for it)
    let t: Vec<(u32, u16)> = g[1].iter().map(|w| (w.0, w.1)).collect();
    assert_eq!(&t[..7], &[(0xDFF096, 0xF), (0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0), (0xDFF096, 0xF), (0xDFF096, 1)]);
}

#[test]
fn note_start_writes() {
    let (l, _) = play(song(), 6).unwrap();
    let g = groups(&l);
    let t = &g[1];
    // after the stop: AUD0LC (two words), LEN, PER, DMA on, the silent block LC and LEN
    let n: Vec<(u32, u16)> = t[7..].iter().map(|w| (w.0, w.1)).collect();
    // sample address: the module's base plus its offset
    let lc_hi = n[0].1 as u32;
    let lc_lo = n[1].1 as u32;
    assert_eq!((n[0].0, n[1].0), (0xDFF0A0, 0xDFF0A2));
    let addr = lc_hi << 16 | lc_lo;
    let base = addr - SAMPLE as u32;
    assert!(base > 0 && base < 0x200000);
    assert_eq!(&n[2..6], &[(0xDFF0A4, 8), (0xDFF0A6, 233), (0xDFF096, 0x8201), (0xDFF0A0, n[5].1)]);
    assert_ne!((n[5].1 as u32) << 16 | n[6].1 as u32, addr);
    assert_eq!((n[6].0, n[7].0, n[7].1), (0xDFF0A2, 0xDFF0A4, 1));
    // both halves of the LC write have the same stamp; the gap from the period write to the DMA
    // start holds the first raster wait (spec: about 2000 colour clocks)
    assert_eq!(t[7].2, t[8].2);
    assert_eq!(t[7 + 5].2, t[7 + 6].2);
    let per = t.iter().find(|w| w.0 == 0xDFF0A6).unwrap().2;
    let on = t.iter().find(|w| w.0 == 0xDFF096 && w.1 == 0x8201).unwrap().2;
    assert!((1900..2300).contains(&(on - per)), "{}", on - per);
}

#[test]
fn note_duration_and_period() {
    // note 50 is two steps above the base: period index 2
    let (l, _) = play(song(), 8).unwrap();
    let g = groups(&l);
    let pers: Vec<u16> = l.iter().filter(|w| w.0 == 0xDFF0A6).map(|w| w.1).collect();
    assert_eq!(pers[0], 233);
    assert_eq!(pers[1], (233.0 * 2f64.powf(-2.0 / 12.0)) as u16);
    // tick 2: the note of two ticks stops in its second tick
    let t2: Vec<(u32, u16)> = g[2].iter().map(|w| (w.0, w.1)).collect();
    assert_eq!(t2, vec![(0xDFF096, 1)]);
}

#[test]
fn renders_deterministically() {
    let a = play(song(), 40).unwrap();
    let b = play(song(), 40).unwrap();
    assert_eq!(a.0, b.0);
    assert!(a.0.len() > 20);
}

#[test]
fn song_end_needs_every_voice() {
    // one voice ends: unused voices never end
    let (_, end) = play(song(), 30).unwrap();
    assert!(!end);
    let s = vec![0x8F];
    let d = module([Some(s.clone()), Some(s.clone()), Some(s.clone()), Some(s)], 1, 0, 0);
    let (l, end) = play(d, 6).unwrap();
    assert!(end);
    // the first tick starts the song, every voice ends in it (no note was written)
    let g = groups(&l);
    assert!(g.len() >= 2);
    assert!(l.iter().all(|w| w.0 == 0xDFF096 || w.0 & 0xF == 8));
}

#[test]
fn volume_and_rest_commands() {
    let (l, _) = play(song(), 12).unwrap();
    let vols: Vec<u16> = l.iter().filter(|w| w.0 == 0xDFF0A8).map(|w| w.1).collect();
    // the host's two volume sets and the song start, then the volume command
    assert_eq!(&vols[..2], &[0, 0]);
    assert_eq!(*vols.last().unwrap(), 40);
}

#[test]
fn undefined_cases_end_safely() {
    // a positive speed value, command 10 and a stack underflow do not hang or panic
    let s = vec![0x8A, 0x8F];
    let d = module([Some(s), None, None, None], 1, 0, 0);
    assert!(play(d, 6).is_some());
    let d = module([Some(vec![0x83]), None, None, None], 1, 0, 0);
    assert!(play(d, 6).is_some());
    let d = module([Some(one_voice_stream()), None, None, None], 1, 5, 0);
    assert!(play(d, 6).is_some());
}

#[test]
fn speed_value_skips_ticks() {
    // speed -2: the first tick is skipped, then two play, one is skipped
    let d = module([Some(one_voice_stream()), None, None, None], 1, -2, 0);
    let (l, _) = play(d, 6).unwrap();
    let g = groups(&l);
    // start group, then song start (second call), ...
    assert!(g[1].iter().any(|w| w.0 == 0xDFF0A8 && w.1 == 0));
    let first = g[1][0].2 - g[0][0].2;
    assert!(first > 70940 + 6240, "{first}");
}

#[test]
fn timer_value_sets_the_tick_period() {
    // a note of one tick, forever: every tick's first write is the stop of the fetch
    let mut st = ins_cmd(STREAM);
    let jump_at = STREAM + st.len() + 2;
    st.extend([48, 1, 0x81]);
    st.extend((((STREAM + 5) as i64 - (jump_at as i64 + 1)) as i32).to_be_bytes());
    let d = module([Some(st), None, None, None], 1, 0, 0x2531);
    let (l, _) = play(d, 40).unwrap();
    let g = groups(&l);
    assert!(g.len() > 6);
    assert_eq!(g[4][0].2 - g[3][0].2, 5 * (0x2531 + 1));
}

#[test]
fn call_loop_and_subsongs() {
    // a call with return, and a loop of three notes
    let at = STREAM;
    let mut s = ins_cmd(at);
    let call_at = at + s.len();
    s.push(0x82); // call the subroutine placed after the end command
    s.extend([0, 0, 0, 0]);
    s.extend([0x84, 0, 3]); // loop start, count 3
    s.extend([48, 1]);
    s.push(0x85);
    s.push(0x8F);
    let sub_at = at + s.len();
    s.extend([49, 1, 0x83]);
    let rel = (sub_at as i64 - (call_at as i64 + 1)) as i32;
    let pos = call_at - at + 1;
    s[pos..pos + 4].copy_from_slice(&rel.to_be_bytes());
    let d = module([Some(s.clone()), None, None, None], 2, 0, 0);
    let l = locate(&d).unwrap();
    assert_eq!(l.subsongs, 2);
    let (log, _) = play(d, 20).unwrap();
    // note 49 (period index 1), then note 48 three times
    let pers: Vec<u16> = log.iter().filter(|w| w.0 == 0xDFF0A6).map(|w| w.1).collect();
    let p1 = (233.0 * 2f64.powf(-1.0 / 12.0)) as u16;
    assert_eq!(&pers[..4], &[p1, 233, 233, 233]);
}
