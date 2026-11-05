use super::*;
use crate::engine::Engine;
use std::path::Path;

fn put_w(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put_l(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// An opcode word and a PC-relative displacement to `target`, at `at`.
fn pc(d: &mut [u8], at: usize, op: u16, target: usize) {
    put_w(d, at, op);
    put_w(d, at + 2, (target as isize - (at as isize + 2)) as i16 as u16);
}

const PER: usize = 0x3A0;
const INS: usize = 0x440;
const SYNINS: usize = 0x460;
const SYN: usize = 0x4C0;
const VIB: usize = 0x530;
const SONG: usize = 0x540;
const SILENT: usize = 0x600;
const BANK: usize = 0x640;

/// A small generation-A module: the header, init and play code shapes the locator reads (never run),
/// one bank sample, and a subsong of four short streams.
fn module() -> Vec<u8> {
    let mut d = vec![0u8; BANK + 6 + 16 + 2];
    // entry table: play at 0x40, init at 0x60
    put_w(&mut d, 0, 0x6000);
    put_w(&mut d, 2, 0x3E);
    put_w(&mut d, 4, 0x6000);
    put_w(&mut d, 6, 0x5A);
    for o in [8usize, 12, 16] {
        put_w(&mut d, o, 0x6000);
        put_w(&mut d, o + 2, (0x40 - (o + 2)) as u16);
    }
    pc(&mut d, 20, 0x41FA, 0x40);
    put_w(&mut d, 28, 0x4E75);
    pc(&mut d, 30, 0x41FA, 0x40);
    // play: period table and vibrato table loads
    pc(&mut d, 0x40, 0x45FA, PER);
    pc(&mut d, 0x44, 0x49FA, VIB);
    put_w(&mut d, 0x48, 0x4E75);
    // init
    let mut o = 0x60;
    put_w(&mut d, o, 0x1741);
    put_w(&mut d, o + 2, 0x2F0);
    o += 4;
    pc(&mut d, o, 0x41FA, SILENT);
    o += 4;
    put_w(&mut d, o, 0xD0FC);
    put_w(&mut d, o + 2, 0x40);
    o += 4;
    pc(&mut d, o, 0x4BFA, INS);
    o += 4;
    put_w(&mut d, o, 0x7000);
    o += 2;
    pc(&mut d, o, 0x45FA, SYN);
    o += 4;
    pc(&mut d, o, 0x4BFA, SYNINS);
    o += 4;
    put_w(&mut d, o, 0x43EB);
    put_w(&mut d, o + 2, 0x300);
    o += 4;
    put_w(&mut d, o, 0x41EB);
    put_w(&mut d, o + 2, SONG as u16);
    o += 4;
    put_w(&mut d, o, 0x43E9);
    put_w(&mut d, o + 2, 0x26);
    o += 4;
    put_w(&mut d, o, 0x4E75);
    // period table: 49 notes, an octave per 12
    for i in 0..49 {
        put_w(&mut d, PER + 2 * i, (4096.0 / 2f64.powf(i as f64 / 12.0)) as u16);
    }
    // instrument 0: volume 64, no loop; synth records and their waveforms
    put_w(&mut d, INS + 0xE, 64);
    for k in 0..3 {
        put_l(&mut d, SYN + 4 * k, (0x500 + 16 * k) as u32);
        put_w(&mut d, SYNINS + 32 * k + 8, 8);
        put_w(&mut d, SYNINS + 32 * k + 0xC, 1024);
        put_w(&mut d, SYNINS + 32 * k + 0xE, 32);
        put_l(&mut d, SYNINS + 32 * k + 4, 0xFFFF_FFFF);
    }
    d[VIB..VIB + 4].copy_from_slice(&[1, 2, 3, 0x84]);
    // song table: one entry (tempo 2), then the terminating zero entry
    let lists = [0x570usize, 0x578, 0x580, 0x588];
    d[SONG + 1] = 2;
    for (v, l) in lists.iter().enumerate() {
        put_l(&mut d, SONG + 2 + 4 * v, *l as u32);
    }
    let streams = [0x5A0usize, 0x5B0, 0x5C0, 0x5D0];
    for (l, s) in lists.iter().zip(streams) {
        put_l(&mut d, *l, s as u32);
    }
    d[0x5A0..0x5A7].copy_from_slice(&[0x80, 0, 2, 10, 3, 12, 0x84]);
    d[0x5B0..0x5B7].copy_from_slice(&[0x80, 0, 1, 20, 0x82, 2, 0x84]);
    d[0x5C0..0x5C3].copy_from_slice(&[0x82, 3, 0x84]);
    d[0x5D0..0x5D6].copy_from_slice(&[0x81, 2, 1, 5, 0x84, 0]);
    // bank: one 16-byte sample, then the closing word
    put_l(&mut d, BANK, 16);
    put_w(&mut d, BANK + 4, 8363);
    for i in 0..16 {
        d[BANK + 6 + i] = (i * 8) as u8;
    }
    let n = d.len();
    put_w(&mut d, n - 2, 0x4E71);
    d
}

#[test]
fn locates_the_tables() {
    let l = locate(&module()).unwrap();
    assert_eq!(l.kind, Gen::A);
    assert_eq!((l.nsm, l.bank, l.silent, l.ins, l.syn, l.synins, l.per, l.vib, l.song), (1, BANK, SILENT, INS, SYN, SYNINS, PER, VIB, SONG));
}

#[test]
fn detect_accepts_and_rejects() {
    let d = module();
    assert!(detect(&d));
    // the closing word is part of the format
    let mut bad = d.clone();
    let n = bad.len();
    bad[n - 1] = 0;
    assert!(!detect(&bad));
    // so is the header
    let mut bad = d.clone();
    bad[8] = 0x4E;
    assert!(!detect(&bad));
    assert!(!detect(&d[..0x200]));
    assert!(!detect(&[0u8; 2000]));
}

#[test]
fn counts_subsongs_and_walks_the_bank() {
    let d = module();
    let l = locate(&d).unwrap();
    assert_eq!(song_count(&d, &l), 1);
    let b = walk_bank(&d, l.bank, 1).unwrap();
    assert_eq!(b, vec![(BANK + 6, 16, 8363)]);
    assert!(walk_bank(&d, l.bank, 2).is_none());
}

const BASE: u32 = 0x20000;

fn run(d: &[u8], ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut e = Engine::new(48000.0);
    e.m.bus.write_bytes(BASE, d);
    e.m.bus.write_log = Some(Vec::new());
    let mut s = new(d, BASE, &mut e.m).unwrap();
    for _ in 0..ticks {
        s.tick(&mut Hw { bus: &mut e.m.bus });
        e.m.bus.clock += DT_TICK_FOR_TEST;
    }
    e.m.bus.write_log.unwrap()
}

const DT_TICK_FOR_TEST: u64 = super::super::DT_TICK;

#[test]
fn first_tick_plays_the_streams() {
    let log = run(&module(), 3);
    let regs = |from: u32, to: u32| log.iter().filter(|w| (from..to).contains(&w.0)).map(|w| (w.0, w.1)).collect::<Vec<_>>();
    // voice 0 starts its note: LC hi/lo, LEN, VOL, PER, then DMACON-set
    let v0: Vec<_> = regs(0xDFF0A0, 0xDFF0B0);
    assert!(v0.contains(&(0xDFF0A8, 64)), "{v0:?}");
    let per = ((4096.0 / 2f64.powf(10.0 / 12.0)) as u32 * (3_579_545 / 8363) >> 10) as u16;
    assert!(v0.contains(&(0xDFF0A6, per)), "{v0:?}");
    assert!(log.iter().any(|w| (w.0, w.1) == (0xDFF096, 0x8201)));
    // voice 1 rests on the silent area first
    assert!(log.iter().any(|w| w.0 == 0xDFF0B2 && w.1 == (BASE + SILENT as u32) as u16));
}

#[test]
fn short_render_is_deterministic() {
    let a = run(&module(), 40);
    let b = run(&module(), 40);
    assert!(a.len() > 40);
    assert_eq!(a, b);
}

#[test]
fn engine_picks_the_player_from_the_content() {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("/rh-test/rh.test"), module()).unwrap();
    assert_eq!(e.playing.as_ref().unwrap().format, "Rob Hubbard");
    let mut out = vec![0f32; 2 * 960];
    let mut sound = false;
    for _ in 0..30 {
        e.render(&mut out);
        sound |= out.iter().any(|&x| x != 0.0);
    }
    assert!(sound);
}

/// The corpus modules, when the folder is present, are recognised with the layouts the
/// specification lists.
#[test]
fn real_modules_when_present() {
    let dir = Path::new("/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/rhimpl/mods");
    // name, generation, nsm, ins, syn, synins, per, vib, song, bank, silent
    let want = [
        ("rh.Budokan", Gen::C, 13, 0x468, 0x45C, 0x608, 0x3FA, 0x668, 0x6CC, 0x1994, 0x1954, 6),
        ("rh.driver", Gen::A, 5, 0x448, 0x43C, 0x5E8, 0x3DA, 0x648, 0x6AC, 0x12D4, 0x1294, 8),
        ("rh.FLYMUS", Gen::A, 6, 0x448, 0x43C, 0x5E8, 0x3DA, 0x648, 0x6B0, 0xBDE, 0xB9E, 2),
        ("rh.GMUSIC", Gen::A, 11, 0x44A, 0x43E, 0x5EA, 0x3DC, 0x64A, 0x6AE, 0x104E, 0x100E, 1),
        ("rh.Populous", Gen::B, 8, 0x618, 0x60C, 0x7B8, 0x5AA, 0x818, 0x87E, 0x1006, 0x3AA, 1),
    ];
    for (name, k, nsm, ins, syn, synins, per, vib, song, bank, silent, subs) in want {
        let Ok(d) = std::fs::read(dir.join(name)) else { continue };
        let l = locate(&d).unwrap_or_else(|| panic!("{name} not located"));
        assert_eq!((l.kind, l.nsm, l.ins, l.syn, l.synins, l.per, l.vib, l.song, l.bank, l.silent), (k, nsm, ins, syn, synins, per, vib, song, bank, silent), "{name}");
        assert_eq!(song_count(&d, &l), subs, "{name}");
    }
}

// --- the commands and effects, on small modules built here (expected values follow the
// specification's rules; the reference runs of the real and edited modules were compared
// register write by register write, value, order and tick-relative time, outside the repository)

/// The writes of each tick, and the engine (for the sample memory).
fn run_ticks(d: &[u8], ticks: usize) -> (Engine, Vec<Vec<(u32, u16, u64)>>, bool) {
    let mut e = Engine::new(48000.0);
    e.m.bus.write_bytes(BASE, d);
    e.m.bus.write_log = Some(Vec::new());
    let mut s = new(d, BASE, &mut e.m).unwrap();
    let mut out = Vec::new();
    for _ in 0..ticks {
        let n0 = e.m.bus.write_log.as_ref().unwrap().len();
        s.tick(&mut Hw { bus: &mut e.m.bus });
        out.push(e.m.bus.write_log.as_ref().unwrap()[n0..].to_vec());
        e.m.bus.clock += super::super::DT_TICK;
    }
    let end = s.songend();
    (e, out, end)
}

/// A module whose voice 0 plays `stream` with instrument 0 patched by `ins` (offset, value) words.
fn with_stream(stream: &[u8], ins: &[(usize, u16)]) -> Vec<u8> {
    let mut d = module();
    d[0x5A0..0x5B0].fill(0);
    d[0x5A0..0x5A0 + stream.len()].copy_from_slice(stream);
    for &(o, v) in ins {
        put_w(&mut d, INS + o, v);
    }
    d
}

fn per_writes(ticks: &[Vec<(u32, u16, u64)>], reg: u32) -> Vec<u16> {
    ticks.iter().flatten().filter(|w| w.0 == reg).map(|w| w.1).collect()
}

fn base_period() -> u16 {
    (((4096.0 / 2f64.powf(10.0 / 12.0)) as u32 * (3_579_545 / 8363)) >> 10) as u16
}

#[test]
fn slide_steps_the_period_each_tick() {
    let d = with_stream(&[0x80, 0, 0x81, 0xFE, 5, 10, 0x84], &[]);
    let (_, t, _) = run_ticks(&d, 6);
    let p = base_period();
    assert_eq!(per_writes(&t, 0xDFF0A6)[..4], [p, p - 2, p - 4, p - 6]);
}

#[test]
fn vibrato_adds_the_sequence_and_reloads_at_its_end() {
    // divisor 4, sequence 1 2 3 then the terminator
    let d = with_stream(&[0x80, 0, 5, 10, 0x84], &[(0x10, 4), (0x12, 0)]);
    let (_, t, _) = run_ticks(&d, 7);
    let p = base_period();
    let q = p / 4;
    assert_eq!(per_writes(&t, 0xDFF0A6)[..6], [p, p + q, p + 2 * q, p + 3 * q, p + q, p + 2 * q]);
}

#[test]
fn waveform_modulation_changes_the_sample_memory_in_place() {
    // limit 3, start 0: the index climbs storing 0, past the limit it stores 0x3C and turns
    let d = with_stream(&[0x80, 0, 5, 10, 0x84], &[(0x14, 3), (0x16, 0)]);
    let w = BASE + BANK as u32 + 6;
    let (mut e, _, _) = run_ticks(&d, 2);
    assert_eq!(e.m.bus.read_byte(w + 1), 0);
    assert_eq!(e.m.bus.read_byte(w + 2), 16);
    let (mut e2, _, _) = run_ticks(&d, 5);
    assert_eq!((1..4).map(|i| e2.m.bus.read_byte(w + i)).collect::<Vec<_>>(), [0, 0, 0]);
    assert_eq!(e2.m.bus.read_byte(w + 4), 0x3C);
    let (mut e3, _, _) = run_ticks(&d, 6);
    // turned: counting down, 0x3C again from the limit
    assert_eq!(e3.m.bus.read_byte(w + 3), 0x3C);
}

#[test]
fn rest_points_at_the_silent_area_and_switches_dma_off_one_tick_early() {
    let d = module();
    // voice 2: rest of 3 units of tempo 2 = 6 ticks
    let (_, t, _) = run_ticks(&d, 7);
    let silent = BASE + SILENT as u32;
    let loc: Vec<_> = t[0].iter().filter(|w| (0xDFF0C0..0xDFF0C6).contains(&w.0)).map(|w| (w.0, w.1)).collect();
    // the first tick's loop set-up and then the rest each point the channel at the silent area
    let one = [(0xDFF0C0, (silent >> 16) as u16), (0xDFF0C2, silent as u16), (0xDFF0C4, 0x20)];
    assert_eq!(loc, [one, one].concat());
    for (k, tick) in t.iter().enumerate() {
        let off = tick.iter().any(|w| (w.0, w.1) == (0xDFF096, 0x0004));
        assert_eq!(off, k == 5, "tick {k}");
    }
}

#[test]
fn first_tick_timing_follows_the_cycle_model() {
    let (_, t, _) = run_ticks(&module(), 1);
    let w = &t[0];
    // voice 3's loop set-up: LC (hi, lo at one time stamp), then LEN 16 cycles = 8 cc later
    assert_eq!(w[1].2, w[0].2);
    assert_eq!(w[2].2 - w[0].2, 8);
}

#[test]
fn song_end_fires_when_all_voices_wrapped_and_play_continues() {
    let d = module();
    let (_, t, end) = run_ticks(&d, 80);
    assert!(end);
    // the player keeps going: voice 0 restarts its first pattern (its note is written again)
    let notes = t.iter().flatten().filter(|w| (w.0, w.1) == (0xDFF096, 0x8201)).count();
    assert!(notes > 2, "{notes}");
    let (_, _, early) = run_ticks(&d, 3);
    assert!(!early);
}
