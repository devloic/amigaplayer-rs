use super::*;
use m68k::CpuType;

const MK: usize = 0x14;
const SAMPLE_X: usize = 0x1401;

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A module with speed `speed`: position 0 plays `rows` on voice 0 (padded to 16 rows with a run
/// of empty rows) and a run of empty rows on the others; positions 1.. play 16 song-end rows on
/// voice 0. Sample slot 0 is a raw object of 100 bytes at `SAMPLE_X`.
fn module(speed: u8, rows: &[[u8; 3]]) -> Vec<u8> {
    let mut d = vec![0u8; 0x1600];
    d[..13].copy_from_slice(MAGIC);
    d[MK - 1] = speed;
    d[MK - 2] = speed;
    put32(&mut d, 14, (0x1600 - MK) as u32);
    // tracks: voice 0's at 0x1190, an empty one at 0x11C0, the song end at 0x11D0
    let mut o = MK + 0x1190;
    for r in rows {
        d[o..o + 3].copy_from_slice(r);
        o += 3;
    }
    if rows.len() < 16 {
        d[o] = 0x92;
        d[o + 1] = (16 - rows.len()) as u8;
    }
    d[MK + 0x11C0] = 0x92;
    d[MK + 0x11C1] = 16;
    d[MK + 0x11D0] = 0x90;
    d[MK + 0x11D1] = 16;
    for p in 0..256 {
        for v in 0..4 {
            let ptr = match (v, p) {
                (0, 0) => 0x1190,
                (0, _) => 0x11D0,
                _ => 0x11C0,
            };
            put32(&mut d, MK + v * 0x400 + 4 * p, ptr);
        }
    }
    for i in 0..50 {
        put32(&mut d, MK + 0x1000 + 4 * i, 0xFFFC_BE40);
    }
    put32(&mut d, MK + 0x1000, (SAMPLE_X - MK - 7) as u32);
    put32(&mut d, MK + 0x10C8, 100);
    d
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let mk = locate(d).unwrap();
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let mut r = SpeedySystem::build(d, mk, 0x10000, 0x20000);
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    (m.bus.write_log.take().unwrap(), r.songend())
}

const NOTE: [u8; 3] = [0x31, 0x11, 0x00];

#[test]
fn detection() {
    let d = module(2, &[NOTE]);
    assert!(detect(&d));
    assert_eq!(locate(&d), Some(MK));
    // magic, marker, extent, track pointers
    let mut e = d.clone();
    e[0] = b'X';
    assert!(!detect(&e));
    let mut e = d.clone();
    e[MK + 3] = 0x91;
    assert!(!detect(&e));
    assert!(!detect(&d[..MK + 0x1190]));
    assert!(!detect(&d[..13]));
    assert!(!detect(&[]));
    let mut e = d.clone();
    put32(&mut e, MK + 0x400 + 8, 0x10_0000);
    assert!(!detect(&e));
    // the other player's marker search window ends at offset 268
    let mut e = vec![0u8; d.len() + 400];
    e[..d.len()].copy_from_slice(&d);
    e.copy_within(MK..MK + 4, 270);
    e[MK + 3] = 0;
    assert!(!detect(&e));
}

#[test]
fn marker_at_another_offset_is_found() {
    let mut d = module(2, &[NOTE]);
    // shift the whole module 8 bytes up: the marker is then at 28 (the speed byte moves with it)
    let mut e = vec![0u8; 8];
    e.extend_from_slice(&d);
    e[..13].copy_from_slice(MAGIC);
    d = e;
    assert_eq!(locate(&d), Some(MK + 8));
    run(&d, 20);
}

#[test]
fn tracks_expand_with_runs_clamped() {
    let mut d = vec![0u8; 64];
    d[0..3].copy_from_slice(&[0x31, 1, 2]);
    d[3] = 0x92;
    d[4] = 0;
    d[5] = 0x90;
    d[6] = 200;
    let mut c = Cost::default();
    let rows = expand(&d, 0, &mut c);
    assert_eq!(rows[0], [0x31, 1, 2]);
    assert_eq!(rows[1], [0xFF; 3]);
    assert_eq!(rows[15], [0xFF; 3]);
    assert_eq!((c.lit, c.run, c.n90, c.n92, c.cut), (1, 15, 1, 1, 1));
    // a track running off the end of the data reads as zero rows
    let mut c = Cost::default();
    let rows = expand(&d[..4], 2, &mut c);
    assert_eq!(rows[15], [0; 3]);
}

#[test]
fn first_note_writes_in_order_and_time() {
    let d = module(2, &[NOTE]);
    let (w, _) = run(&d, 1);
    let regs: Vec<u32> = w.iter().map(|x| x.0).collect();
    // DMA off, DMA on, period, location (two words), length, the tick's DMA mask, volume
    assert_eq!(&regs[..8], &[0xDFF096, 0xDFF096, 0xDFF0A6, 0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF096, 0xDFF0A8]);
    assert_eq!((w[0].1, w[1].1), (1, 0x8200));
    assert_eq!(w[2].1, PERIODS[0]);
    assert_eq!(((w[3].1 as u32) << 16) | w[4].1 as u32, 0x10000 + SAMPLE_X as u32 + 1);
    assert_eq!(w[3].2, w[4].2);
    assert_eq!(w[5].1, 44);
    assert_eq!(w[6].1, 0x8201);
    assert_eq!(w[7].1, 8);
    // expansion: 1 literal and a 15-row run on voice 0, a 16-row run on each other voice
    let exp = 1756 + 123 + 104 * (15 + 48) + 70 * 4;
    let s0 = 501 + exp + 40;
    assert_eq!(w[0].2, 1_000_000 + (s0 + 368) / 2);
    assert_eq!(w[1].2, 1_000_000 + (s0 + 388) / 2);
    assert_eq!(w[2].2, 1_000_000 + (s0 + 1277) / 2);
    assert_eq!(w[5].2, 1_000_000 + (s0 + 1653) / 2);
}

#[test]
fn envelope_attacks_then_releases_and_ends_the_note() {
    // attack period 0 (completes at once), release period 1
    let d = module(1, &[[0x31, 0x01, 0x00]]);
    let (w, _) = run(&d, 24);
    let vols: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A8).map(|x| x.1).collect();
    assert_eq!(vols[0], 0x40);
    // down in steps of eight, each held for one waiting tick, ending in 0
    assert!(vols.windows(2).all(|p| p[1] <= p[0]));
    assert_eq!(*vols.last().unwrap(), 0);
    // the note ended: DMA off, DMA on, then volume 0
    let end = w.iter().rposition(|x| x.0 == 0xDFF0A8).unwrap();
    assert_eq!((w[end - 2].0, w[end - 2].1), (0xDFF096, 1));
    assert_eq!((w[end - 1].0, w[end - 1].1), (0xDFF096, 0x8200));
}

#[test]
fn sustain_points_at_the_silent_sample() {
    let d = module(0, &[[0x31, 0x10, 0x00]]);
    let (w, _) = run(&d, 20);
    // release period 0: location = the replayer's zero block, length 4, no volume write
    let k = w.iter().position(|x| x.0 == 0xDFF0A0 && x.1 == 2).unwrap();
    assert_eq!(((w[k].1 as u32) << 16) | w[k + 1].1 as u32, 0x20000);
    assert_eq!(w[k + 2].1, 4);
}

#[test]
fn slide_command_changes_the_period_every_tick() {
    let d = module(0, &[NOTE, [0x50, 0x00, 0x04], [0x51, 0x00, 0x01]]);
    let (w, _) = run(&d, 6);
    let per: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    assert_eq!(per[0], PERIODS[0]);
    // tick 0: note; tick 1: add 4; tick 2: subtract 1 (the command byte is replaced)
    assert_eq!(per[1], PERIODS[0] + 4);
    assert_eq!(per[2], PERIODS[0] + 3);
    assert_eq!(per[3], PERIODS[0] + 2);
}

#[test]
fn octave_shifts_the_period() {
    for (b0, want) in [(0x11u8, PERIODS[0] << 2), (0x21, PERIODS[0] << 1), (0x31, PERIODS[0]), (0x41, PERIODS[0] >> 1)] {
        let (w, _) = run(&module(2, &[[b0, 0x11, 0]]), 1);
        assert_eq!(w[2].1, want);
    }
}

#[test]
fn song_ends_at_the_end_row_and_restarts() {
    // position 0 has a note, 16 rows at speed 1: 32 ticks, then position 1 ends the song
    let d = module(1, &[NOTE]);
    let (_, end) = run(&d, 31);
    assert!(!end);
    let (w, end) = run(&d, 34);
    assert!(end);
    // after the end the song starts again at position 0: a second note start
    let starts = w.iter().filter(|x| x.0 == 0xDFF096 && x.1 == 1).count();
    assert!(starts >= 2);
}

#[test]
fn the_position_wrap_ends_the_song() {
    // no end row at all: every position replays position 0's pointers
    let mut d = module(0, &[NOTE]);
    for p in 1..256 {
        put32(&mut d, MK + 4 * p, 0x11C0);
    }
    let (_, end) = run(&d, 255 * 16);
    assert!(!end);
    let (_, end) = run(&d, 256 * 16 + 1);
    assert!(end);
}

#[test]
fn rejected_notes_stay_inactive_and_odd_rows_are_inert() {
    let rows = [
        [0x31, 0xFF, 0x00],  // silent note
        [0x31, 0x11, 0x31],  // sample index above the table's range
        [0x31, 0x11, 0xC0],  // far above
        [0x31, 0x11, 0x05],  // empty slot
        [0x30, 0x11, 0x00],  // note 0
        [0x3E, 0x11, 0x00],  // note 14
        [0x52, 0x00, 0x01],  // unknown command
        [0x09, 0x00, 0x00],  // below the notes
        [0x91, 0x00, 0x00],  // a literal row that is ignored
    ];
    let d = module(0, &rows);
    let (w, _) = run(&d, 10);
    // only the DMA off/on pairs of the first four rows and the tick masks are written
    assert!(w.iter().all(|x| x.0 == 0xDFF096));
    assert!(w.iter().filter(|x| x.0 == 0xDFF096 && x.1 == 0x8200).count() >= 8);
}

#[test]
fn loop_shaped_sample_reloads_after_the_start() {
    let mut d = module(2, &[NOTE]);
    // an IFF-like object at the sample: one-shot 64, repeat 32, the body chunk id at +0x60
    let b = SAMPLE_X & !1;
    put32(&mut d, b + 0x14, 64);
    put32(&mut d, b + 0x18, 32);
    d[b + 0x60..b + 0x64].copy_from_slice(b"BODY");
    put32(&mut d, b + 0x64, 96);
    let (w, _) = run(&d, 1);
    let lc: Vec<u32> = w.iter().filter(|x| x.0 == 0xDFF0A0).map(|x| x.1 as u32).collect();
    let len: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A4).map(|x| x.1).collect();
    // start, body start, loop start (high words: the base is below 0x10000 + 0x2000)
    assert_eq!(len, vec![44, 30, 16]);
    assert_eq!(lc.len(), 3);
    let lo: Vec<u16> = w.iter().filter(|x| x.0 == 0xDFF0A2).map(|x| x.1).collect();
    assert_eq!(lo[1] as usize, 0x10000 + b + 0x68 - 0x10000);
    assert_eq!(lo[2] as usize, b + 64 + 0x64);
}

#[test]
fn start_up_scan_cost() {
    let mut d = module(2, &[NOTE]);
    // here voice 0 ends every position with the same pointer as the others do
    assert_eq!(scan_cost(&d, MK), 2162 + 62 + 8);
    for p in 1..256 {
        put32(&mut d, MK + 4 * p, 0x11C0);
    }
    // positions 255 down to 1 repeat; position 0 differs on voice 0 (k = 255, j = 0); one populated
    // sample slot and none empty in front of it
    assert_eq!(scan_cost(&d, MK), 2162 + 136 * 255 + 62 + 8);
    // two empty slots in front of the sample, and a speed that differs from the byte before it
    let mut e = d.clone();
    put32(&mut e, MK + 0x1000, 0xFFFF_FFFF);
    put32(&mut e, MK + 0x1000 + 8, (SAMPLE_X - MK - 7) as u32);
    e[MK - 2] = 9;
    assert_eq!(scan_cost(&e, MK), 2162 + 136 * 255 + 62 + 39 * 2 + 8 - 2);
}

#[test]
fn deterministic() {
    let d = module(1, &[NOTE, [0x41, 0x2A, 0], [0x50, 1, 1]]);
    assert_eq!(run(&d, 80).0, run(&d, 80).0);
}

#[test]
fn damaged_modules_never_hang_or_panic() {
    let base = module(1, &[NOTE, [0x42, 0x2A, 0], [0x50, 1, 1], [0x90, 3, 0], [0x92, 0, 0], [0x31, 0x11, 49]]);
    let mut seed = 987_654_321u32;
    for _ in 0..300 {
        let mut d = base.clone();
        for _ in 0..30 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = (seed >> 8) as usize % d.len();
            // keep the header and the position tables, damage tracks, samples and tables
            if o >= MK + 0x1000 {
                d[o] = (seed >> 24) as u8;
            }
        }
        if locate(&d).is_some() {
            run(&d, 200);
        }
    }
    // speed 255, and the last sample offset pointing outside the file
    let mut d = base.clone();
    d[MK - 1] = 255;
    run(&d, 300);
    let mut d = base;
    put32(&mut d, MK + 0x1000, 0x7FFF_FFF0);
    run(&d, 40);
}
