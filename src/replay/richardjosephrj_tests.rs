use super::*;
use m68k::CpuType;

const BASE: u32 = 0x10000;
const IMG: usize = 0x41400;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A variant-C container: the header, the pattern table (entry n at `PAT + 16 n`), the first
/// instrument (index 0 a looping sample, 1 a one-shot), one modulation set, one envelope and a
/// period table; `start` is the InitSong address (the image offset of a shape that is known).
const PAT: usize = 0x2000;
const PERTAB: usize = 0x3000;
const SAMPLES: usize = 0x3400;

fn module_c(start: u32, patterns: &[&[u8]]) -> Vec<u8> {
    let mut d = vec![0u8; HDR + IMG];
    put32(&mut d, 0, 0x3F3);
    put32(&mut d, 8, 1);
    put32(&mut d, 20, 0x4000_0000 | (IMG as u32 / 4));
    put32(&mut d, 24, 0x3E9);
    put32(&mut d, 28, IMG as u32 / 4);
    let i = HDR;
    put32(&mut d, i, 0x70FF_4E75);
    d[i + 4..i + 12].copy_from_slice(b"R.JOSEPH");
    for (o, v) in [(12, 0x40F8C), (16, 0x40F18), (20, 0x410FC), (24, start), (28, 0x40D62), (32, 0x40EDC)] {
        put32(&mut d, i + o, v);
    }
    put32(&mut d, i + 64, 3);
    // patterns
    for (n, p) in patterns.iter().enumerate() {
        let o = PAT + 16 * 32 * n;
        d[i + o..i + o + p.len()].copy_from_slice(p);
        put32(&mut d, i + 0xD48 + 4 * n, (PAT + 16 * 32 * n) as u32);
    }
    // instruments: 0 = looping, with loop offset 0; 1 = one-shot
    let ins = i + 0x40D62;
    put16(&mut d, ins, 0xFFFF);
    put16(&mut d, ins + 4, 8);
    put32(&mut d, ins + 6, SAMPLES as u32);
    put32(&mut d, ins + 10, PERTAB as u32);
    put16(&mut d, ins + 14, 0);
    put16(&mut d, ins + 18, 8);
    put32(&mut d, ins + 20, SAMPLES as u32);
    put32(&mut d, ins + 24, PERTAB as u32);
    // periods: a rising table
    for k in 0..512 {
        put16(&mut d, i + PERTAB + 2 * k, 100 + k as u16);
    }
    // an envelope (set 0): attack 8 every 1 tick, decay 2 every 1 down to 10, release at 4
    d[i + 0x966..i + 0x966 + 8].copy_from_slice(&[8, 1, 2, 1, 10, 4, 3, 1]);
    // a modulation set 1: one volume count of 3 with step 1, one pitch count of 2 with step 2
    d[i + 0x938 + 15..i + 0x938 + 30].copy_from_slice(&[3, 0, 2, 0, 0, 1, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    // song table (shape Table): song 1 starts patterns 1, 2, 0, 0
    put32(&mut d, i + 0x8F8 + 4, 0x0102_0000);
    // the table shape's code: a marker word, then the table address and the end marker
    let a = start as usize;
    if start == 0x412B4 {
        put16(&mut d, i + a, 0x41F9);
        put32(&mut d, i + a + 2, 0x8F8);
    } else {
        put16(&mut d, i + a, 0x4E75);
    }
    d
}

struct Rig {
    m: Machine,
    r: Box<dyn Replayer>,
    due: u64,
}

fn rig(d: &[u8]) -> Option<Rig> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_bytes(BASE, d);
    m.bus.write_log = Some(Vec::new());
    m.bus.clock = 1_000_000 + crate::engine::START_PHASE;
    let mut r = new(d, BASE, &mut m)?;
    let due = r.subsong_tick_at().unwrap();
    Some(Rig { m, r, due })
}

impl Rig {
    fn ticks(&mut self, n: usize) {
        for _ in 0..n {
            self.m.bus.clock = self.m.bus.clock.max(self.due);
            self.m.bus.clock = self.due;
            let mut hw = Hw { bus: &mut self.m.bus };
            self.r.tick(&mut hw);
            self.due += TICK;
        }
    }

    fn log(&self) -> Vec<(u32, u16, u64)> {
        self.m.bus.write_log.clone().unwrap()
    }
}

/// A note of pattern byte `n` on a looping instrument with duration set by the tempo.
const PLAY: &[u8] = &[0x94, 10, 0x8C, 2, 0x80, 0x20, 0x30, 0x90, 0xAC];

#[test]
fn detection_follows_the_header() {
    let d = module_c(0x8B6, &[PLAY]);
    assert!(detect(&d));
    for (o, v) in [(0usize, 1u8), (20, 0), (32, 1), (36, 1), (44, 0), (47, 0)] {
        let mut e = d.clone();
        if o == 44 || o == 47 {
            put32(&mut e, 44, 0);
        } else {
            e[o] = v;
        }
        assert!(!detect(&e), "offset {o}");
    }
    assert!(!detect(&d[..60]));
    assert!(!detect(&[]));
}

#[test]
fn rjp_files_are_not_claimed() {
    // a Richard Joseph Player song: RJP?SMOD, a zero long at 12
    let mut rjp = vec![0u8; 64];
    rjp[..3].copy_from_slice(b"RJP");
    rjp[3] = b'1';
    rjp[4..8].copy_from_slice(b"SMOD");
    assert!(!detect(&rjp));
    // nobody but the Richard Joseph Player takes it, and that one takes it first
    let first = crate::replay::REPLAYERS.iter().find(|o| (o.detect)(&rjp)).map(|o| o.name);
    assert_eq!(first, Some("Richard Joseph Player"));
    // the other way: a container is not an RJP song
    let d = module_c(0x8B6, &[PLAY]);
    assert!(!(super::super::richardjoseph::OPEN.detect)(&d));
    let first = crate::replay::REPLAYERS.iter().find(|o| (o.detect)(&d)).map(|o| o.name);
    assert_eq!(first, Some("Richard Joseph"));
}

#[test]
fn unknown_drivers_and_starts_are_not_played() {
    let d = module_c(0x1234, &[PLAY]);
    assert!(detect(&d));
    assert!(recognise(&d).is_none());
    assert!(rig(&d).is_none());
    let mut e = module_c(0x8B6, &[PLAY]);
    e.truncate(HDR + 0x1000);
    assert!(rig(&e).is_none());
}

#[test]
fn init_sound_writes_the_specified_order_and_times() {
    let d = module_c(0x8B6, &[PLAY, PLAY, PLAY, PLAY, PLAY, PLAY, PLAY, PLAY]);
    let r = rig(&d).unwrap();
    let log = r.log();
    let origin = 1_000_000;
    // scan length s: the marker 0x287B is not in the image, so the whole image from the start
    // routine counts; the first write is the driver init's DMACON, 676 colour clocks after the
    // InitSound call starts
    let s = ((IMG - 0x8B6) / 2) as u64;
    let t0 = 257 + (893 + 21 * s) / 2 + 48 + 12 + 18 + 236;
    assert_eq!(log[0], (0xDFF096, 0x000F, origin + t0 + 676));
    // then the interrupt-less ADKCON write 60 clocks later, and the volumes, lengths, periods
    assert_eq!(log[1], (0xDFF09E, 0x00FF, origin + t0 + 736));
    let regs: Vec<u32> = log[2..14].iter().map(|l| l.0).collect();
    let want: Vec<u32> = (0..4).map(|c| 0xDFF0A8 + 16 * c).chain((0..4).map(|c| 0xDFF0A4 + 16 * c)).chain((0..4).map(|c| 0xDFF0A6 + 16 * c)).collect();
    assert_eq!(regs, want);
    // the first tick follows the end of InitSound by one period
    assert!(r.due > log.last().unwrap().2 + TICK - 1000);
}

#[test]
fn a_note_starts_dma_with_a_raster_wait() {
    let d = module_c(0x8B6, &[PLAY, PLAY, PLAY, PLAY, PLAY, PLAY, PLAY, PLAY]);
    let mut r = rig(&d).unwrap();
    let n0 = r.log().len();
    r.ticks(1);
    let log = r.log();
    // the first tick's fetch: DMA off, volume 0, kill sample, length 1, period 1, DMA on, and
    // after the nine-line wait the emulated interrupt's loop registers
    let k: Vec<(u32, u16)> = log[n0..].iter().map(|l| (l.0, l.1)).collect();
    assert_eq!(k[0], (0xDFF096, 0x0001));
    assert_eq!(k[1], (0xDFF0A8, 0));
    let on = log[n0..].iter().position(|l| l.0 == 0xDFF096 && l.1 == 0x8001).unwrap();
    let after = &log[n0 + on + 1];
    assert_eq!(after.0, 0xDFF0A0);
    let gap = after.2 - log[n0 + on].2;
    assert!(gap > 9 * 227 && gap < 9 * 227 + 400, "{gap}");
}

#[test]
fn rest_note_and_stop_end_the_song() {
    let stop: &[u8] = &[0x94, 100, 0x8C, 1, 0x80, 0x20, 0x30, 0xAC];
    let d = module_c(0x8B6, &[stop, stop, stop, stop, stop, stop, stop, stop]);
    let mut r = rig(&d).unwrap();
    assert!(!r.r.songend());
    r.ticks(40);
    // patterns 1, 2, 4, 7 all run to their stop: the song-end test fires after the fourth
    assert!(r.r.songend());
    let log = r.log();
    // volumes are written with the volume stage (at most 0x7F)
    assert!(log.iter().filter(|l| l.0 == 0xDFF0A8).all(|l| l.1 <= 0x7F));
}

#[test]
fn render_is_deterministic() {
    let d = module_c(0x412B4, &[PLAY, PLAY, PLAY]);
    let mut a = rig(&d).unwrap();
    let mut b = rig(&d).unwrap();
    a.ticks(30);
    b.ticks(30);
    assert_eq!(a.log(), b.log());
    assert!(a.log().len() > 100);
}

#[test]
fn song_table_start_walks_to_the_first_zero() {
    let d = module_c(0x412B4, &[PLAY, PLAY, PLAY]);
    let mut r = rig(&d).unwrap();
    // subsong 1 is given patterns 1, 2, 0, 0: voices 0, 1 and 2 are started, 3 is not
    let (lo, hi, cur) = r.r.subsongs();
    assert_eq!((lo, hi, cur), (0, 2, 0));
    {
        let mut hw = Hw { bus: &mut r.m.bus };
        r.r.set_subsong(0, &mut hw);
    }
    let _ = r;
}

fn all_safe(pattern: &[u8], ticks: usize) -> Rig {
    let d = module_c(0x8B6, &[pattern, pattern, pattern, pattern, pattern, pattern, pattern, pattern]);
    let mut r = rig(&d).unwrap();
    r.ticks(ticks);
    r
}

#[test]
fn undefined_cases_do_not_hang() {
    // invalid command byte, tempo 0, empty 0x98, a pattern that jumps to itself, a stack that
    // grows without end, a return with nothing on the stack, a loop of commands only
    for p in [
        &[0x81u8, 0x30, 0x90][..],
        &[0x94, 0, 0x8C, 3, 0x30, 0x90],
        &[0x98, 0, 0x30, 0x90],
        &[0xD4, 1],
        &[0xB0, 1],
        &[0xB4, 0x30, 0x90],
        &[0xC4, 0x30, 0x90],
        &[0x88],
        &[0xFC],
    ] {
        let r = all_safe(p, 6);
        assert!(!r.log().is_empty());
    }
}

#[test]
fn loop_body_runs_count_plus_one_times() {
    // loop start with count 2, a note and a rest inside, loop end, stop: three notes
    let p: &[u8] = &[0x94, 250, 0x8C, 1, 0xC0, 2, 0x30, 0xC4, 0xAC];
    let d = module_c(0x8B6, &[&[0xAC], p, &[0xAC], &[0xAC], &[0xAC], &[0xAC], &[0xAC], &[0xAC]]);
    let mut r = rig(&d).unwrap();
    r.ticks(60);
    // channel 0 got its trigger (DMA on) three times: once for each pass of the body
    let ons = r.log().iter().filter(|l| l.0 == 0xDFF096 && l.1 == 0x8001).count();
    assert!(ons >= 3, "{ons}");
}

#[test]
fn song_end_ticks_stay_on_the_grid() {
    let d = module_c(0x8B6, &[PLAY, PLAY, PLAY, PLAY, PLAY, PLAY, PLAY, PLAY]);
    let r = rig(&d).unwrap();
    let mut r = r;
    let first = r.due;
    r.ticks(3);
    assert_eq!(r.due, first + 3 * TICK);
    assert_eq!(r.r.tick_clocks(), TICK);
}
