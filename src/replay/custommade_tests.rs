//! Tests on small dumps built in code: the detection rules, the init scan, the first-tick write
//! sequence and timing of the specification's worked example, envelope, arpeggio, portamento and
//! phase effects, the stop, subsong changes, and malformed input.

use super::*;
use m68k::CpuType;

const BASE: u32 = 0x4000;
const ORIGIN: u32 = 0x1000;
const SIZE: usize = 4096;

fn put_w(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}
fn put_l(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}
/// The absolute address the dump's data pointers hold for image offset `o`.
fn abs(o: usize) -> u32 {
    ORIGIN + o as u32
}

// Layout of the built dump (image offsets).
const ARPS: usize = 0x400;
const INSTRS: usize = 0x500;
const SONGS: usize = 0x600;
const SAMPLE_D: usize = 0x700;
const VIB_D: usize = 0x710;
const VIB_T: usize = 0x720;
const LISTS: usize = 0x900;
const TRACKS: usize = 0xA00;
const WAVE: usize = 0x800;

/// A dump with `nsub` subsongs. Subsong `n` has four track lists of one entry each; voice 0 plays
/// `track` (a byte stream), the other voices `[0x83]`... Tracks live at `TRACKS + 0x40 * i`.
fn dump(nsub: usize, tracks: &[&[u8]]) -> Vec<u8> {
    let mut d = vec![0u8; SIZE];
    put_w(&mut d, 0, 0x4EF9);
    put_w(&mut d, 6, 0x4EF9);
    d[8..20].copy_from_slice(&MARKER);
    // timer 0x2581; the old play address points at the songs marker below
    d[0x1F] = 0x25;
    put_l(&mut d, 0x20, M_TIMER);
    d[0x27] = 0x81;
    put_w(&mut d, 0x3C, 0x21FC);
    put_l(&mut d, 0x3E, abs(0x100));
    put_l(&mut d, 0x60, M_ARPS);
    put_w(&mut d, 0x64, (ARPS - 0x64) as u16);
    put_l(&mut d, 0x80, M_PTOFF);
    put_w(&mut d, 0x84, 0x10);
    put_l(&mut d, 0xA0, M_INSTRS);
    put_w(&mut d, 0xA6, (INSTRS - 0xA6) as u16);
    put_l(&mut d, 0x100, M_SONGS_A);
    put_w(&mut d, 0x106, W_SONGS);
    put_w(&mut d, 0x108, (SONGS - 0x10 - 0x108) as u16);
    put_l(&mut d, 0x140, M_STOP);
    // arpeggio 0: zeros; arpeggio 1: 0, 4, 7, 0, 4, 7 ... (index x of 12 bytes)
    for i in 0..12 {
        d[ARPS + 12 + i] = [0, 4, 7][i % 3];
    }
    // instrument 0: envelope to 0x3F in steps of 16, fast; instrument 1 one-shot with phase
    for i in 0..3 {
        let o = INSTRS + 32 * i;
        put_l(&mut d, o, abs(SAMPLE_D));
        put_l(&mut d, o + 4, abs(VIB_D));
        d[o + 14] = 0x3F;
        d[o + 15] = 0x3F;
        d[o + 16] = 0x3F;
        d[o + 17] = 0x3F;
        d[o + 18] = 16;
        d[o + 19] = 16;
        d[o + 20] = 16;
        d[o + 21] = 16;
    }
    d[INSTRS + 32 + 8] = 1;
    // instrument 2: phase modulation (speed 1, length 3, value 0x55) and nothing else
    d[INSTRS + 64 + 9] = 1;
    d[INSTRS + 64 + 10] = 3;
    d[INSTRS + 64 + 22] = 0x55;
    // sample descriptor and vibrato descriptor, wave data
    put_l(&mut d, SAMPLE_D, abs(WAVE));
    put_w(&mut d, SAMPLE_D + 4, 8);
    put_w(&mut d, SAMPLE_D + 6, 0);
    put_l(&mut d, VIB_D, abs(VIB_T));
    put_w(&mut d, VIB_D + 4, 4);
    for i in 0..4 {
        d[VIB_T + i] = [2u8, 0xFE, 1, 0xFF][i];
    }
    // song table
    for n in 0..nsub {
        for v in 0..4 {
            put_l(&mut d, SONGS + 16 * n + 4 * v, abs(LISTS + 0x40 * n + 0x10 * v));
        }
    }
    put_l(&mut d, SONGS + 16 * nsub, 0x00DF_F0A0);
    // track lists: one entry (track, transposition 0, play count 1) and the end marker
    for n in 0..nsub {
        for v in 0..4 {
            let l = LISTS + 0x40 * n + 0x10 * v;
            let t = tracks[(n * 4 + v).min(tracks.len() - 1)];
            let at = TRACKS + 0x20 * (n * 4 + v);
            d[at..at + t.len()].copy_from_slice(t);
            put_l(&mut d, l, abs(at));
            put_w(&mut d, l + 10, 1);
            put_l(&mut d, l + 12, 0xFFFF_FFFF);
        }
    }
    d
}

struct Rig {
    m: Machine,
    r: Box<dyn Replayer>,
}

fn start(d: &[u8]) -> Rig {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    m.bus.write_bytes(BASE, d);
    let r = new(d, BASE, &mut m).expect("recognised");
    Rig { m, r }
}

impl Rig {
    /// One tick starting at the current clock plus a gap; the new log entries.
    fn tick(&mut self) -> Vec<(u32, u16, u64)> {
        self.m.bus.clock += 100_000;
        let from = self.m.bus.write_log.as_ref().unwrap().len();
        self.r.tick(&mut Hw { bus: &mut self.m.bus });
        self.m.bus.write_log.as_ref().unwrap()[from..].to_vec()
    }
}

fn rel(log: &[(u32, u16, u64)]) -> Vec<(u32, u16, u64)> {
    // (the tick's first write is the stamp 0 of the specification's table)
    let t0 = log[0].2;
    log.iter().map(|w| (w.0, w.1, w.2 - t0)).collect()
}

const TRK_A: &[u8] = &[0x82, 0x00, 0x84, 0x00, 0x24, 0x02, 0x83];
/// A voice that just holds a long note.
const HOLD: &[u8] = &[0x82, 0x00, 0x10, 0x3F, 0x83];

#[test]
fn detection_rules() {
    let d = dump(1, &[TRK_A]);
    assert!(detect(&d));
    // size
    assert!(!detect(&d[..3000]));
    // entry words: the branch form and the jump form are both fine, anything else is not
    let mut e = d.clone();
    put_w(&mut e, 0, 0x6000);
    put_w(&mut e, 4, 0x6000);
    assert!(detect(&e));
    put_w(&mut e, 4, 0x6001);
    assert!(!detect(&e));
    let mut e = d.clone();
    put_w(&mut e, 0, 0x4EB9);
    assert!(detect(&e));
    put_w(&mut e, 6, 0x4EB9);
    assert!(!detect(&e));
    // a hunk executable
    let mut e = d.clone();
    e[..4].copy_from_slice(&[0, 0, 3, 0xF3]);
    assert!(!detect(&e));
    // the marker must lie at an even offset between 8 and 407
    let mut e = d.clone();
    e[8..20].fill(0);
    assert!(!detect(&e));
    put_w(&mut e, 0, 0x4EF9);
    e[408..420].copy_from_slice(&MARKER);
    assert!(!detect(&e));
    e[406..418].copy_from_slice(&MARKER);
    assert!(detect(&e));
    // a file without the ptoff marker is not playable
    let mut e = d.clone();
    put_l(&mut e, 0x80, 0);
    assert!(!detect(&e));
}

#[test]
fn scan_finds_the_tables() {
    let d = dump(3, &[TRK_A]);
    let l = scan(&d).unwrap();
    assert_eq!(l.timer, 0x2581);
    assert_eq!(l.origin, ORIGIN);
    assert_eq!((l.songs, l.arps, l.instrs), (SONGS, ARPS, INSTRS));
    assert_eq!(count_subsongs(&d, l.songs), 3);
    let r = start(&d);
    assert_eq!(r.r.subsongs(), (1, 3, 1));
    assert_eq!(r.r.tick_clocks(), 5 * 0x2582);
    assert_eq!(r.r.format(), "CustomMade");
}

#[test]
fn init_writes_and_host_volumes() {
    let r = start(&dump(1, &[TRK_A]));
    let log = r.m.bus.write_log.as_ref().unwrap();
    // eight zero volume writes at the specification's spacing, then per voice the sample start
    // and length
    let vols: Vec<_> = log[..8].iter().map(|w| (w.0, w.1)).collect();
    let want: Vec<_> = (0..8).map(|i| (0xDFF0A8 + 16 * (i as u32 % 4), 0)).collect();
    assert_eq!(vols, want);
    let gaps: Vec<u64> = log[..8].windows(2).map(|w| w[1].2 - w[0].2).collect();
    assert_eq!(gaps, HOST_GAPS);
    let rest: Vec<_> = log[8..].iter().map(|w| (w.0, w.1)).collect();
    let mut want = Vec::new();
    for k in 0..4u32 {
        want.push((0xDFF0A0 + 16 * k, ((BASE + (WAVE as u32)) >> 16) as u16));
        want.push((0xDFF0A2 + 16 * k, (BASE + WAVE as u32) as u16));
        want.push((0xDFF0A4 + 16 * k, 8));
    }
    assert_eq!(rest, want);
    // LC then LEN 10 colour clocks later, the next voice 387 or 388 after
    assert_eq!(log[10].2 - log[8].2, 10);
    assert!(matches!(log[11].2 - log[8].2, 387 | 388));
    assert!(r.r.songend() == false);
}

/// The worked example of the specification: voice 0's first tick (one-shot pending, an
/// instrument command, an envelope speed, a note) and voice 1's start.
#[test]
fn first_tick_matches_the_worked_example() {
    let mut r = start(&dump(1, &[TRK_A, &[0x83]]));
    let log = rel(&r.tick());
    let w: Vec<_> = log.iter().take(10).map(|e| (e.0, e.1, e.2)).collect();
    let lc = BASE + WAVE as u32;
    let want = vec![
        (0xDFF096, 0x8001, 0),
        (0xDFF0A4, 1, 26),
        (0xDFF096, 0x0001, 164),
        (0xDFF0A0, (lc >> 16) as u16, 218),
        (0xDFF0A2, lc as u16, 218),
        (0xDFF0A4, 8, 228),
        (0xDFF096, 0x0001, 436),
        (0xDFF0A0, (lc >> 16) as u16, 490),
        (0xDFF0A2, lc as u16, 490),
        (0xDFF0A4, 8, 500),
    ];
    assert_eq!(w, want);
    // the note's period (note 0x24, index 36) at 593, then voice 1's DMA start at 731
    assert_eq!((log[10].0, log[10].1, log[10].2), (0xDFF0A6, PERIODS[0x24], 593));
    assert_eq!((log[11].0, log[11].1, log[11].2), (0xDFF096, 0x8002, 731));
}

#[test]
fn effects_envelope_and_arpeggio() {
    // instrument 0, envelope speed 0, one note of duration 2 (8 ticks), arpeggio 1
    let trk: &[u8] = &[0x80, 0x01, 0x82, 0x00, 0x84, 0x00, 0x10, 0x02, 0x83];
    let mut r = start(&dump(1, &[trk, HOLD]));
    r.tick();
    // effects ticks: the volume rises 16, 32, 48, 63 and stays; the period follows the arpeggio
    // from index 0 then 11, 10, ...
    let mut vols = Vec::new();
    let mut pers = Vec::new();
    for _ in 0..6 {
        for w in r.tick() {
            if w.0 == 0xDFF0A8 {
                vols.push(w.1);
            }
            if w.0 == 0xDFF0A6 {
                pers.push(w.1);
            }
        }
    }
    assert_eq!(vols[..4], [16, 32, 48, 63]);
    // arpeggio entry 0 is note 0x10 + 0, then entries 11, 10, 9 ... = 7, 4, 0, 7, 4
    let want: Vec<u16> = [0u8, 7, 4, 0, 7, 4].iter().map(|a| PERIODS[0x10 + *a as usize]).collect();
    // (voice 0 only: the other voices stop at their first row, so their periods are not written)
    assert_eq!(pers, want);
}

#[test]
fn portamento_moves_to_the_target_and_clamps() {
    // note 0x10 for one row (4 ticks), then a portamento to note 0x14, speed 100, 2 rows
    let trk: &[u8] = &[0x82, 0x00, 0x10, 0x01, 0x81, 0x14, 100, 0x02, 0x83];
    let mut r = start(&dump(1, &[trk, HOLD]));
    r.tick();
    for _ in 0..3 {
        r.tick();
    }
    // the portamento row writes the current period unchanged
    let log = r.tick();
    let per = log.iter().find(|w| w.0 == 0xDFF0A6).unwrap().1;
    assert_eq!(per, PERIODS[0x10]);
    // then the period moves by 100 per tick until it is clamped at the target (a smaller period
    // lies below: it moves down)
    let mut seen = Vec::new();
    for _ in 0..4 {
        for w in r.tick() {
            if w.0 == 0xDFF0A6 {
                seen.push(w.1);
            }
        }
    }
    let (mut c, t) = (PERIODS[0x10], PERIODS[0x14]);
    let mut want = Vec::new();
    for _ in 0..4 {
        c = if c > t { (c - 100).max(t) } else { (c + 100).min(t) };
        want.push(c);
    }
    assert_eq!(seen, want);
}

#[test]
fn stop_ends_the_song_and_later_ticks_do_nothing() {
    let mut r = start(&dump(1, &[&[0x83]]));
    let first = r.tick();
    // each voice starts (DMA on and the one-shot length) and then reaches its stop
    assert_eq!(first.iter().filter(|w| w.0 == 0xDFF096).count(), 4);
    assert!(r.r.songend());
    assert!(r.tick().is_empty());
}

#[test]
fn track_list_wrap_reports_the_song_end_once_all_voices_wrapped() {
    // every voice plays one short note per list pass and then ends its track
    let trk: &[u8] = &[0x82, 0x00, 0x10, 0x01, 0xFF];
    let mut r = start(&dump(1, &[trk]));
    let mut ticks = 0;
    while !r.r.songend() && ticks < 40 {
        r.tick();
        ticks += 1;
    }
    assert!(r.r.songend(), "the lists wrapped");
    // the song goes on after the report
    assert!(!r.tick().is_empty());
}

#[test]
fn zero_duration_note_changes_only_the_period() {
    // a duration-0 note, then a real one: no sample writes for the first
    let trk: &[u8] = &[0x82, 0x00, 0x12, 0x00, 0x14, 0x01, 0x83];
    let mut r = start(&dump(1, &[trk, HOLD]));
    let log = r.tick();
    let pers: Vec<_> = log.iter().filter(|w| w.0 == 0xDFF0A6).collect();
    assert_eq!(pers.len(), 1);
    assert_eq!(pers[0].1, PERIODS[0x14]);
}

#[test]
fn phase_step_changes_the_wave_and_the_instrument_state() {
    // instrument 2 has phase modulation: the wave byte at the phase position changes
    let trk: &[u8] = &[0x82, 0x02, 0x10, 0x02, 0x83];
    let mut r = start(&dump(1, &[trk, HOLD]));
    // (the phase base of the descriptor is 0 and the instrument's length 3: the low byte
    // becomes 0xFD, so the position 0 is the byte 253 past the wave)
    let at = BASE as usize + WAVE + 253;
    r.tick();
    assert_eq!(r.m.bus.ram[at], 0);
    r.tick();
    assert_eq!(r.m.bus.ram[at], 0x55);
    r.tick();
    assert_eq!(r.m.bus.ram[at + 1], 0x55);
}

#[test]
fn subsong_change_runs_the_hosts_stop_and_init() {
    let mut r = start(&dump(3, &[TRK_A, HOLD]));
    let n = r.m.bus.write_log.as_ref().unwrap().len();
    r.r.set_subsong(2, &mut Hw { bus: &mut r.m.bus });
    let log = r.m.bus.write_log.as_ref().unwrap()[n..].to_vec();
    // DMA off, four zero volumes, eight zero volumes, then the init
    assert_eq!((log[0].0, log[0].1), (0xDFF096, 0x000F));
    assert!(log[1..13].iter().all(|w| w.1 == 0 && (0xDFF0A8..=0xDFF0D8).contains(&w.0)));
    assert_eq!(log.len(), 13 + 12);
    assert_eq!(r.r.subsongs().2, 2);
    assert!(r.r.subsong_tick_at().unwrap() > log.last().unwrap().2);
}

#[test]
fn period_lookup_outside_the_table_is_safe() {
    let d = dump(1, &[TRK_A]);
    let cm = CustomMade {
        img: d.clone(),
        base: BASE,
        lay: scan(&d).unwrap(),
        nsub: 1,
        sub: 0,
        v: [Voice::default(); 4],
        playing: true,
        flags: [true; 4],
        songend: false,
        lastvol: [0; 4],
        t0: 0,
        cyc: 0,
        stops: 0,
        last_init_len: 0,
        first_tick: None,
        period_cc: 1,
        instruments: 0,
    };
    assert_eq!(cm.period_at(0), 0x1AC0);
    assert_eq!(cm.period_at(83), 0x0038);
    assert_eq!(cm.period_at(84), 0x003F);
    assert_eq!(cm.period_at(85), 0);
    assert_eq!(cm.period_at(-1), 0);
    assert_eq!(cm.period_off(0x7FFE), 0);
    assert_eq!(cm.period_clean(200, 0xFFFF), 0);
}

#[test]
fn malformed_images_do_not_panic() {
    let base = dump(2, &[TRK_A, &[0x82, 0x09, 0x30, 0x00, 0x84, 0x01, 0xFF, 0x81, 0x02, 0x03, 0x04]]);
    // corrupt many positions with pseudo-random bytes, run a few hundred ticks of each
    let mut x = 12345u32;
    for _ in 0..60 {
        let mut d = base.clone();
        for _ in 0..40 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let at = 0x140 + (x >> 8) as usize % (SIZE - 0x140);
            d[at] = (x >> 24) as u8;
        }
        if !detect(&d) {
            continue;
        }
        let mut r = start(&d);
        for _ in 0..300 {
            r.tick();
            if r.r.songend() {
                break;
            }
        }
        r.r.set_subsong(2, &mut Hw { bus: &mut r.m.bus });
        r.tick();
    }
    // wholly random content and short files are refused
    assert!(!detect(&[0u8; 100]));
    assert!(!detect(&vec![0x4Eu8; 5000]));
    let mut m = Machine::new(CpuType::M68000);
    assert!(new(&[0u8; 10], 0x4000, &mut m).is_none());
}

#[test]
fn the_same_dump_plays_the_same() {
    let d = dump(1, &[TRK_A, &[0x82, 0x01, 0x20, 0x03, 0xFF]]);
    let run = || {
        let mut r = start(&d);
        for _ in 0..30 {
            r.tick();
        }
        r.m.bus.write_log.clone().unwrap()
    };
    assert_eq!(run(), run());
}
