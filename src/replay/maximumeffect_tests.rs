use super::*;
use crate::engine::Engine;
use std::path::Path;

// ---------------------------------------------------------------------------------
// Synthetic modules, built here from the specification's layout (no game data)

/// One block of a track list: the stream words and the event counter.
type Block = (Vec<u16>, u16);

/// One song: mask byte, the track lists of the present voices (in voice order) and the restart offset.
struct Song {
    mask: u8,
    voices: Vec<Vec<Block>>,
    restart: u16,
}

fn note(sample: u16, n: u16, keep: bool) -> u16 {
    0x8000 | (sample << 8) | (n << 1) | keep as u16
}
fn dur(d: u16) -> u16 {
    0x8000 + d
}
fn cmd(c: u16, b: u16) -> u16 {
    (c << 8) | b
}

/// The song file: the song table right behind the count, then the lists, then the streams.
fn module(songs: &[Song]) -> Vec<u8> {
    let table = 2 + 18 * songs.len();
    let mut lists = Vec::new();
    let mut streams = Vec::new();
    // sizes first: lists then streams
    let mut list_off = table;
    let mut plan = Vec::new();
    for s in songs {
        let mut ptrs = Vec::new();
        for v in &s.voices {
            ptrs.push(list_off);
            list_off += 6 * (v.len() + 1);
        }
        plan.push(ptrs);
    }
    let mut stream_off = list_off;
    for (si, s) in songs.iter().enumerate() {
        for (vi, v) in s.voices.iter().enumerate() {
            let _ = (si, vi);
            for (st, cnt) in v {
                lists.extend_from_slice(&(stream_off as u32).to_be_bytes());
                lists.extend_from_slice(&cnt.to_be_bytes());
                for w in st {
                    streams.extend_from_slice(&w.to_be_bytes());
                }
                stream_off += 2 * st.len();
            }
            lists.extend_from_slice(&[0, 0, 0, 0]);
            lists.extend_from_slice(&s.restart.to_be_bytes());
        }
    }
    let mut d = ((songs.len() - 1) as u16).to_be_bytes().to_vec();
    for (s, ptrs) in songs.iter().zip(&plan) {
        d.push(0);
        d.push(s.mask);
        let mut it = ptrs.iter();
        for vi in 0..4 {
            let p = if s.mask & (1 << vi) != 0 { *it.next().unwrap_or(&0) } else { 0 };
            d.extend_from_slice(&(p as u32).to_be_bytes());
        }
    }
    d.extend_from_slice(&lists);
    d.extend_from_slice(&streams);
    d
}

/// A sample bank: records (start offset from the data, length in words, volume, loop offset, loop
/// length), the data 1000 bytes.
fn bank(recs: &[(u32, u16, u16, u32, u16)]) -> Vec<u8> {
    let tbl = 2 + 16 * recs.len() as u32;
    let mut b = ((recs.len() - 1) as u16).to_be_bytes().to_vec();
    for r in recs {
        b.extend_from_slice(&(tbl + r.0).to_be_bytes());
        b.extend_from_slice(&r.1.to_be_bytes());
        b.extend_from_slice(&r.2.to_be_bytes());
        b.extend_from_slice(&(tbl + r.3).to_be_bytes());
        b.extend_from_slice(&r.4.to_be_bytes());
        b.extend_from_slice(&[0, 0]);
    }
    b.extend((0..1000).map(|i| (i as u8).wrapping_mul(5)));
    b
}

fn std_bank() -> Vec<u8> {
    bank(&[(0, 50, 0x30, 20, 10), (100, 40, 0x20, 100, 40), (200, 30, 0x40, 0, 0)])
}

type Log = Vec<(u32, u16)>;

fn run_ticks(m: Vec<u8>, b: Option<Vec<u8>>, ticks: usize, sub: u32) -> (Log, bool) {
    let dir = Path::new("/me-test");
    let mut e = Engine::new(48000.0);
    if let Some(s) = b {
        e.m.os.mem_files.insert(dir.join("SMP.test"), s);
    }
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if e.play_data(&dir.join("max.test"), m).is_err() {
        return (Vec::new(), false);
    }
    for _ in 0..sub {
        e.change_subsong(1).unwrap();
    }
    let mut out = vec![0f32; 2 * 960 * (ticks + 2)]; // the first tick is a little over one tick period in
    e.render(&mut out);
    let ended = e.playing.as_ref().is_some_and(|p| p.format == "Maximum Effect");
    (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), ended)
}

fn regs(log: &Log, reg: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == reg).map(|w| w.1).collect()
}

/// A four-voice song whose voices all use `stream` (one block of `events` events).
fn four(stream: Vec<u16>, events: u16) -> Song {
    Song { mask: 0x0F, voices: (0..4).map(|_| vec![(stream.clone(), events)]).collect(), restart: 0 }
}

// ---------------------------------------------------------------------------------
// Tables and detection

#[test]
fn tables_match_the_specification() {
    assert_eq!(PERIODS[0], 856);
    assert_eq!(PERIODS[12], 428);
    assert_eq!(PERIODS[35], 113);
    let sines: Vec<i16> = (0..18).map(sine).collect();
    assert_eq!(sines, [0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253]);
    assert_eq!(sine(32), 0);
    assert_eq!(sine(33), -24);
    assert_eq!(sine(48), -255);
    for i in 0..32 {
        assert_eq!(SINE_HALF[i], (255.0 * (std::f64::consts::PI * i as f64 / 32.0).sin()).floor() as i16);
    }
}

#[test]
fn detection_follows_the_content_rule() {
    let m = module(&[four(vec![note(0, 0, false), dur(3)], 1)]);
    assert!(detect(&m));
    assert!(!detect(&[]));
    assert!(!detect(&m[..19]));
    assert!(!detect(&vec![0u8; 200]));
    assert!(!detect(&vec![0xFFu8; 200]));
    // first long below 15
    let mut b = m.clone();
    b[3] = 14;
    b[0..3].fill(0);
    assert!(!detect(&b));
    // odd P1 / P1 not 2 + a multiple of 18 / P1 past the end
    for v in [21u32, 19, 2 + 36 + 2, m.len() as u32 + 2, 0x8000_0000 | 20] {
        let mut b = m.clone();
        b[4..8].copy_from_slice(&v.to_be_bytes());
        assert!(!detect(&b), "P1 {v:#x}");
    }
    // a pointer P2 that is not preceded by an end marker
    let mut b = m.clone();
    let p = 40; // inside the lists, behind no zero long
    b[8..12].copy_from_slice(&(p as u32).to_be_bytes());
    assert!(!detect(&b));
    // a negative pointer
    let mut b = m.clone();
    b[12..16].copy_from_slice(&0x8000_0010u32.to_be_bytes());
    assert!(!detect(&b));
}

/// `MAXEFF_CORPUS=list.txt cargo test --release detect_corpus -- --ignored`: the detector against
/// a list of other-format modules (one path a line); none may pass.
#[test]
#[ignore]
fn detect_corpus() {
    let Ok(list) = std::env::var("MAXEFF_CORPUS") else { return };
    let mut n = 0;
    let mut hits = Vec::new();
    for p in std::fs::read_to_string(list).unwrap().lines() {
        let Ok(d) = std::fs::read(p) else { continue };
        n += 1;
        if detect(&d) {
            hits.push(p.to_string());
        }
    }
    println!("{n} files, {} accepted: {hits:?}", hits.len());
    assert!(hits.is_empty());
}

// ---------------------------------------------------------------------------------
// Playing

#[test]
fn a_note_start_writes_the_registers_of_the_specification() {
    let m = module(&[four(vec![note(0, 12, false), dur(1), note(1, 0, true), dur(0)], 2)]);
    let (log, _) = run_ticks(m, Some(std_bank()), 4, 0);
    // the first tick: DMA off, LC, LEN, PER, VOL on channel 0
    assert_eq!(log[0], (0xDFF096, 0x0001));
    assert_eq!((log[1].0, log[2].0), (0xDFF0A0, 0xDFF0A2));
    assert_eq!(log[3], (0xDFF0A4, 50));
    assert_eq!(log[4], (0xDFF0A6, 428));
    assert_eq!(log[5], (0xDFF0A8, 0x30));
    // channel 1 follows with the same note
    assert_eq!(log[6], (0xDFF096, 0x0002));
    // the end of the tick: the DMA enable of the four channels, then the loop registers
    let tail: Vec<u32> = log.iter().skip_while(|w| w.1 != 0x800F).map(|w| w.0).take(13).collect();
    assert_eq!(tail[0], 0xDFF096);
    assert_eq!(&tail[1..5], &[0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF0B0]);
    // the second event (tick 2) keeps the volume bit: the volume stays 0x30 / 0x20 from the first note
    let vols = regs(&log, 0xDFF0A8);
    assert_eq!(vols[0], 0x30);
    assert_eq!(vols[1], 0x30);
    let lens = regs(&log, 0xDFF0A4);
    // the note LEN, the loop LEN at the end of each tick, the second note's LEN and its loop LEN
    assert_eq!(&lens[..5], &[50, 10, 10, 40, 40]);
}

#[test]
fn loop_part_is_written_every_tick_and_zero_before_a_note() {
    let m = module(&[Song {
        mask: 0x0F,
        voices: vec![
            vec![(vec![note(0, 0, false), dur(9)], 1)],
            vec![(vec![cmd(0, 0), dur(9)], 1)],
            vec![(vec![cmd(0, 0), dur(9)], 1)],
            vec![(vec![cmd(0, 0), dur(9)], 1)],
        ],
        restart: 0,
    }]);
    let (log, _) = run_ticks(m, Some(std_bank()), 3, 0);
    // channel 1 never starts a note: its loop length is 0 every tick
    assert!(regs(&log, 0xDFF0B4).iter().all(|&l| l == 0));
    assert!(regs(&log, 0xDFF0B4).len() >= 3);
    // channel 0: the loop length of sample 0 each tick
    assert!(regs(&log, 0xDFF0A4).iter().skip(1).all(|&l| l == 10));
}

#[test]
fn volume_commands_and_slide_clamp() {
    let m = module(&[four(vec![note(0, 0, false), cmd(6, 0xF0), dur(5)], 1)]); // slide -16 from 0x30
    let (log, _) = run_ticks(m, Some(std_bank()), 8, 0);
    let v = regs(&log, 0xDFF0A8);
    assert_eq!(&v[..4], &[0x30, 0x20, 0x10, 0x00]);
    assert_eq!(v[4], 0); // clamped at zero, the effect continues to write
    // command 3 sets the volume as is (not limited to 64)
    let m = module(&[four(vec![note(0, 0, true), cmd(3, 200), dur(0)], 1)]);
    let (log, _) = run_ticks(m, Some(std_bank()), 2, 0);
    assert_eq!(regs(&log, 0xDFF0A8)[0], 200);
}

#[test]
fn volume_slide_clamps_at_the_top() {
    let m = module(&[four(vec![note(2, 0, false), cmd(6, 0x20), dur(3)], 1)]); // 0x40 + 32
    let (log, _) = run_ticks(m, Some(std_bank()), 3, 0);
    assert_eq!(&regs(&log, 0xDFF0A8)[..3], &[0x40, 0x40, 0x40]);
}

#[test]
fn portamento_moves_to_the_target_and_stops() {
    // start at index 0 (856), target offset 2 (index 1, 808), step -20
    // the extra bytes: step 0xEC (-20), slide parameter 0
    let m = module(&[four(vec![note(0, 0, false), cmd(7, 2), 0xEC00, dur(6)], 1)]);
    let (log, _) = run_ticks(m, Some(std_bank()), 5, 0);
    let p = regs(&log, 0xDFF0A6);
    // the step runs once at once: 856 - 20 = 836, then 816, then clamps at 808
    assert_eq!(&p[..4], &[836, 816, 808, 808]);
}

#[test]
fn arpeggio_cycles_through_three_offsets() {
    // offsets 0, 4, 7 bytes... even offsets: 0, 8, 14 -> indices 0, 4, 7
    let m = module(&[four(vec![note(0, 0, false), cmd(8, 0), 0x080E, dur(6)], 1)]);
    let (log, _) = run_ticks(m, Some(std_bank()), 6, 0);
    let p = regs(&log, 0xDFF0A6);
    assert_eq!(p[0], 856);
    assert_eq!(&p[1..5], &[856, 678, 570, 856]);
}

#[test]
fn vibrato_phase_keeps_its_masked_value() {
    // cmd 4: speed 1 (odd), depth shift 7, slide 0: the phase byte is masked after each step
    let m = module(&[four(vec![note(0, 12, false), cmd(4, 1), 0x0700, dur(4)], 1)]);
    let (log, _) = run_ticks(m, Some(std_bank()), 5, 0);
    let p = regs(&log, 0xDFF0A6);
    // step 1 at once: phase 0, sine[0] = 0; then phase = 1 (masked 0 + speed 1); next entry uses 1 & 0x7E = 0
    assert_eq!(&p[..3], &[428, 428, 428]);
}

#[test]
fn play_once_voices_stop_and_do_not_report_the_end() {
    let m = module(&[Song { mask: 0x1F, voices: (0..4).map(|_| vec![(vec![note(0, 0, false), dur(0)], 1)]).collect(), restart: 0 }]);
    let (log, _) = run_ticks(m, Some(std_bank()), 20, 0);
    // one note start per channel and nothing more (the loop registers go on being written)
    assert_eq!(regs(&log, 0xDFF0A6).len(), 1);
}

#[test]
fn a_looping_four_voice_song_reports_the_end() {
    let m = module(&[four(vec![note(0, 0, false), dur(0)], 1)]);
    let dir = Path::new("/me-test");
    let mut e = Engine::new(48000.0);
    e.m.os.mem_files.insert(dir.join("SMP.test"), std_bank());
    e.open_replayers = true;
    e.play_data(&dir.join("max.test"), m).unwrap();
    let mut out = vec![0f32; 2 * 960 * 6];
    e.render(&mut out);
    assert!(e.songend(), "song end after the first block");
}

#[test]
fn songs_with_fewer_than_four_voices_play_wrongly() {
    // two voices: the processed channels are 2 and 3, the data is on 0 and 1: nothing plays
    let s = |mask: u8, n: usize| Song { mask, voices: (0..n).map(|_| vec![(vec![note(0, 0, false), dur(0)], 1)]).collect(), restart: 0 };
    // (a lone song with a mask below 15 is not recognised: the first long of the file must be 15 or more)
    let dummy = || four(vec![cmd(0, 0), dur(0)], 1);
    let (log, _) = run_ticks(module(&[dummy(), s(0x03, 2)]), Some(std_bank()), 6, 1);
    assert!(regs(&log, 0xDFF0A6).is_empty());
    assert!(regs(&log, 0xDFF0B6).is_empty());
    // three voices: channels 1 and 2 play
    let (log, _) = run_ticks(module(&[dummy(), s(0x07, 3)]), Some(std_bank()), 6, 1);
    assert!(regs(&log, 0xDFF0A6).is_empty());
    assert!(!regs(&log, 0xDFF0B6).is_empty());
    assert!(!regs(&log, 0xDFF0C6).is_empty());
    assert!(regs(&log, 0xDFF0D6).is_empty());
}

#[test]
fn shared_parameter_bytes_persist_across_events() {
    // event 1: arpeggio sets A, B, C; event 2 (no new effect): volume slide with A taken from command 6
    // overwrites A only; a vibrato (command 5) afterwards reads B (the arpeggio's second offset) as phase
    let m = module(&[four(vec![
        note(0, 12, false),
        cmd(8, 0),
        0x0400,
        dur(0),
        cmd(5, 0),
        0x0500,
        dur(0),
    ], 2)]);
    let (log, _) = run_ticks(m, Some(std_bank()), 3, 0);
    // tick 2: command 5: speed 0, depth 5, slide 0: phase B = 4 -> sine[2] = 49 << 5 >> 7 = 12 : 428 + 12
    assert_eq!(regs(&log, 0xDFF0A6)[1], 440);
}

// ---------------------------------------------------------------------------------
// Sub-songs, determinism, timing

#[test]
fn subsongs_and_the_stop_group() {
    let a = four(vec![note(0, 0, false), dur(5)], 1);
    let b = four(vec![note(1, 5, false), dur(5)], 1);
    let (log, _) = run_ticks(module(&[a, b]), Some(std_bank()), 4, 1);
    // the stop group of the host before the first tick of sub-song 1
    assert_eq!(log[0], (0xDFF096, 0x000F));
    assert_eq!(&log[1..5].iter().map(|w| w.0).collect::<Vec<_>>(), &[0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8]);
    assert_eq!(regs(&log, 0xDFF0A6)[0], PERIODS[5]);
}

#[test]
fn runs_are_deterministic() {
    let m = module(&[four(vec![note(0, 3, false), cmd(4, 3), 0x0401, dur(2), note(1, 9, false), cmd(7, 6), 0x0203, dur(4)], 2)]);
    let a = run_ticks(m.clone(), Some(std_bank()), 40, 0).0;
    let b = run_ticks(m, Some(std_bank()), 40, 0).0;
    assert!(a.len() > 100);
    assert_eq!(a, b);
}

#[test]
fn first_tick_clock_follows_the_directory_path() {
    // module, one song, 4 voices, no zero pointers: the same module, directory 1 character longer
    // starts 8.5 cc later (17 / 2)
    let m = module(&[four(vec![note(0, 0, false), dur(1)], 1)]);
    let first = |dir: &str| {
        let d = Path::new(dir);
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(d.join("SMP.test"), std_bank());
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&d.join("max.test"), m.clone()).unwrap();
        let mut out = vec![0f32; 2 * 960 * 3];
        e.render(&mut out);
        e.m.bus.write_log.take().unwrap()[0].2
    };
    // The song start (where Paula's grids restart) lies a fixed time before the first tick, so
    // the path length moves the clock of the first write only by its raster-line phase: 272 cc
    // for 32 characters longer, modulo a raster line of 227 (the beam the ticks' waits see).
    let a = first("/aaaa");
    let b = first("/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // 32 characters longer: 272 cc
    assert_eq!((b as i64 - a as i64).rem_euclid(227), 272 % 227);
}

// ---------------------------------------------------------------------------------
// Undefined and malformed input: end safely, never panic or hang

#[test]
fn undefined_commands_and_indexes_do_not_panic() {
    let m = module(&[four(vec![
        note(0, 100, false), // note index above 35: clamped
        cmd(50, 7),          // command above 8: no operation
        cmd(9, 1),
        dur(1),
        note(90, 3, false), // sample number outside the table: ignored
        dur(1),
        cmd(7, 200), // portamento target beyond the table
        0x0000,
        dur(1),
        cmd(8, 250),
        0xFFFF,
        dur(2),
    ], 4)]);
    let (log, _) = run_ticks(m, Some(std_bank()), 30, 0);
    assert!(!log.is_empty());
    assert_eq!(regs(&log, 0xDFF0A6)[0], 113);
}

#[test]
fn a_restart_offset_at_the_end_marker_stops_the_voice() {
    // one block, restart offset 6 = the marker itself
    let mut s = four(vec![note(0, 0, false), dur(0)], 1);
    s.restart = 6;
    let (log, _) = run_ticks(module(&[s]), Some(std_bank()), 30, 0);
    assert!(!log.is_empty());
}

#[test]
fn event_without_a_duration_word_stops_the_voice() {
    // a stream of command words only, up to the end of the file and beyond (reads as zero)
    let m = module(&[four(vec![cmd(0, 0), cmd(0, 0)], 1)]);
    let (log, _) = run_ticks(m, Some(std_bank()), 10, 0);
    assert!(!log.is_empty());
}

#[test]
fn a_stream_pointer_outside_the_file_stops_the_voice() {
    let mut m = module(&[four(vec![note(0, 0, false), dur(0)], 1)]);
    // the first list entry of voice 0: point it past the end
    let l = be32(&m, 4) as usize;
    m[l..l + 4].copy_from_slice(&0x00FF_0000u32.to_be_bytes());
    let (log, _) = run_ticks(m, Some(std_bank()), 10, 0);
    assert!(!log.is_empty());
}

#[test]
fn missing_or_bad_bank_is_not_recognised() {
    let m = module(&[four(vec![note(0, 0, false), dur(0)], 1)]);
    assert!(run_ticks(m.clone(), None, 2, 0).0.is_empty());
    // a bank shorter than its last record needs
    let mut b = std_bank();
    b.truncate(2 + 16 * 3 + 100);
    assert!(run_ticks(m, Some(b), 2, 0).0.is_empty());
}

#[test]
fn damaged_modules_never_panic() {
    let base = module(&[
        four(vec![note(0, 3, false), cmd(4, 3), 0x0401, dur(2), note(1, 9, false), cmd(7, 6), 0x0203, dur(4), cmd(8, 2), 0x0406, dur(3)], 3),
        Song { mask: 0x1F, voices: (0..4).map(|_| vec![(vec![note(1, 4, true), dur(2)], 3)]).collect(), restart: 6 },
    ]);
    let mut seed = 0x1234_5678u32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed >> 8
    };
    for _ in 0..60 {
        let mut m = base.clone();
        for _ in 0..(1 + rnd() % 6) {
            let i = rnd() as usize % m.len();
            m[i] = rnd() as u8;
        }
        let mut b = std_bank();
        if rnd() % 4 == 0 {
            let i = rnd() as usize % 60;
            b[i] = rnd() as u8;
        }
        let _ = run_ticks(m, Some(b), 40, rnd() % 2);
    }
}
