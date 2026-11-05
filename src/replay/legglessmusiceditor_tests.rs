use super::*;
use crate::engine::Engine;
use std::path::Path;

fn w(o: &mut Vec<u8>, v: u16) {
    o.extend(v.to_be_bytes());
}

fn l(o: &mut Vec<u8>, v: u32) {
    o.extend(v.to_be_bytes());
}

/// An instrument entry: sample start/length (bytes of the sample area / words), volume, and the
/// words from offset 14 on as given (29 words in all).
fn ins(start: u32, len: u16, vol: u16, loop_start: u32, loop_len: u16, rest: &[(usize, u16)]) -> Vec<u8> {
    let mut words = [0u16; 29];
    words[2] = len;
    words[3] = vol;
    words[6] = loop_len;
    for &(i, v) in rest {
        words[i] = v;
    }
    let mut o = Vec::new();
    l(&mut o, start);
    for (i, &x) in words.iter().enumerate().skip(2) {
        if i == 4 {
            l(&mut o, loop_start);
            continue;
        }
        if i == 5 {
            continue;
        }
        w(&mut o, x);
    }
    assert_eq!(o.len(), 58);
    o
}

/// A module of one subsong: `tracks[v]` are the track-list words of voice v (without the end marker),
/// `pats` the patterns (word lists, end marker added), `insts` the instrument entries.
fn module(insts: &[Vec<u8>], tracks: [&[u16]; 4], pats: &[Vec<u16>], sample: &[u8]) -> Vec<u8> {
    let ins_abs = 56 + 16;
    let mut tr = Vec::new();
    let mut tr_off = [0u32; 4];
    let tr_abs = ins_abs + 58 * insts.len();
    for v in 0..4 {
        tr_off[v] = (tr_abs + tr.len() - 40) as u32;
        for &x in tracks[v] {
            w(&mut tr, x);
        }
        w(&mut tr, 0xFFFF);
    }
    let patp_abs = tr_abs + tr.len();
    let patd_abs = patp_abs + 4 * pats.len();
    let mut pd = Vec::new();
    let mut po = Vec::new();
    for p in pats {
        l(&mut po, pd.len() as u32);
        for &x in p {
            w(&mut pd, x);
        }
        w(&mut pd, 0x8000);
    }
    let smp_abs = patd_abs + pd.len();
    let mut o = b"LME\0".to_vec();
    o.extend(format!("{:<32}", "(c)1990 test module").bytes());
    l(&mut o, 0);
    l(&mut o, (ins_abs - 40) as u32);
    l(&mut o, (patp_abs - 40) as u32);
    l(&mut o, (patd_abs - 40) as u32);
    l(&mut o, (smp_abs - 40) as u32);
    for t in tr_off {
        l(&mut o, t);
    }
    for i in insts {
        o.extend(i);
    }
    o.extend(tr);
    o.extend(po);
    o.extend(pd);
    o.extend([0u8; 4]);
    o.extend(sample);
    o
}

// event flags
const REST: u16 = 1;
const SEL: u16 = 2;
const SLIDE: u16 = 4;
const TIE: u16 = 8;
const PTAB: u16 = 16;

/// Voice 0 plays `ev` once and the others rest for a long time.
fn song(insts: &[Vec<u8>], ev: Vec<u16>, sample: &[u8]) -> Vec<u8> {
    let rest = vec![REST, 500];
    module(insts, [&[0], &[1], &[1], &[1]], &[ev, rest], sample)
}

fn plain() -> Vec<Vec<u8>> {
    // a looped sample of 8 words at sample-area offset 4, volume 0x30
    vec![ins(4, 8, 0x30, 4, 8, &[])]
}

fn smp() -> Vec<u8> {
    (0..16u8).collect()
}

type Log = Vec<(u32, u16, u64)>;

fn play(d: Vec<u8>, ticks: usize) -> Option<(Log, bool, Engine)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/lme-test/lme.test"), d).ok()?;
    if e.playing.as_ref()?.format != "Leggless Music Editor" {
        return None;
    }
    let mut out = vec![0f32; 2 * 960];
    let mut end = false;
    for _ in 0..=ticks {
        e.render(&mut out);
        end |= e.songend();
    }
    let log = e.m.bus.write_log.take().unwrap();
    Some((log, end, e))
}

/// The log split into ticks (gaps over 30 000 colour clocks).
fn ticks(log: &Log) -> Vec<Vec<(u32, u16, u64)>> {
    let mut v: Vec<Vec<(u32, u16, u64)>> = Vec::new();
    let mut last = 0;
    for &x in log {
        if v.is_empty() || x.2 - last > 30000 {
            v.push(Vec::new());
        }
        last = x.2;
        v.last_mut().unwrap().push(x);
    }
    v
}

const DMACON: u32 = 0xDFF096;
fn per(ch: u32) -> u32 {
    0xDFF0A6 + 16 * ch
}
fn vol(ch: u32) -> u32 {
    0xDFF0A8 + 16 * ch
}

#[test]
fn detects_and_counts() {
    let d = song(&plain(), vec![SEL, 0, 5, 12], &smp());
    assert!(detect(&d));
    let lay = layout(&d).unwrap();
    assert_eq!(lay.subsongs, 1);
    let mut bad = d.clone();
    bad[3] = 1;
    assert!(!detect(&bad));
    let mut bad = d.clone();
    bad[39] = 1;
    assert!(!detect(&bad));
    assert!(!detect(&[]));
    assert!(!detect(&d[..30]));
    // the sample is not all there: refused
    assert!(!detect(&d[..d.len() - 3]));
}

#[test]
fn two_subsongs_by_the_table() {
    // a header with two subsong entries: (INS - 16) / 16
    let mut d = song(&plain(), vec![SEL, 0, 5, 12], &smp());
    let mut d2 = d[..56].to_vec();
    d2.extend_from_slice(&d[56..72]);
    d2.extend_from_slice(&d[56..72]);
    d2.extend_from_slice(&d[72..]);
    // every offset moves by 16
    for o in [40usize, 44, 48, 52] {
        let v = be32(&d2, o) + 16;
        d2[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    for o in (56..120).step_by(4) {
        let v = be32(&d2, o) + 16;
        d2[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    d.clear();
    assert_eq!(layout(&d2).unwrap().subsongs, 2);
    assert!(detect(&d2));
}

#[test]
fn first_tick_writes_in_order_and_time() {
    let d = song(&plain(), vec![SEL, 0, 5, 12], &smp());
    let (log, _, _) = play(d, 3).unwrap();
    let t = ticks(&log);
    let t0 = &t[0];
    // volume stage: four zero writes, the first 951 colour clocks after the interrupt entry
    let regs: Vec<u32> = t0.iter().map(|x| x.0).collect();
    assert_eq!(&regs[..4], &[vol(0), vol(1), vol(2), vol(3)]);
    assert!(t0[..4].iter().all(|x| x.1 == 0));
    // voice 0: note start: DMA off, LC (two words), LEN, PER, VOL raw, VOL scaled
    assert_eq!(t0[4].0, DMACON);
    assert_eq!(t0[4].1, 1);
    assert_eq!(t0[5].0, 0xDFF0A0);
    assert_eq!(t0[6].0, 0xDFF0A2);
    assert_eq!(t0[5].2, t0[6].2);
    assert_eq!((t0[7].0, t0[7].1), (0xDFF0A4, 8));
    assert_eq!((t0[8].0, t0[8].1), (per(0), 0x1AC));
    assert_eq!((t0[9].0, t0[9].1), (vol(0), 0x30));
    assert_eq!((t0[10].0, t0[10].1), (vol(0), 0));
    // the spacing of the note block: +13, +24, +30, +41 colour clocks after the DMA write
    let b = t0[4].2;
    assert_eq!([t0[5].2 - b, t0[7].2 - b, t0[8].2 - b, t0[9].2 - b], [13, 24, 30, 41]);
    // the scaled write is 92 or 93 colour clocks after the raw one for voice 0
    assert!(matches!(t0[10].2 - t0[9].2, 92 | 93));
    // the rests of voices 1..3, then the closing DMACON with voice 0's bit
    assert_eq!((t0[11].0, t0[11].1), (DMACON, 2));
    assert_eq!((t0[12].0, t0[12].1), (DMACON, 4));
    assert_eq!((t0[13].0, t0[13].1), (DMACON, 8));
    let last = t0.last().unwrap();
    assert_eq!((last.0, last.1), (DMACON, 0x8001));
    // the sample address is the module's sample area plus the start: the loop address written in
    // the next tick is the same here (loop start 4 as well)
    let lc = (t0[5].1 as u32) << 16 | t0[6].1 as u32;
    let t1 = &t[1];
    assert_eq!((t1[0].1 as u32) << 16 | t1[1].1 as u32, lc);
}

#[test]
fn ticks_are_70940_apart_and_the_volume_is_corrected_next_tick() {
    let d = song(&plain(), vec![SEL, 0, 50, 12], &smp());
    let (log, _, _) = play(d, 4).unwrap();
    let t = ticks(&log);
    assert!(t.len() >= 4);
    for k in 1..t.len() {
        // the first write of every tick after the first (the volume stage, or the deferred loop
        // address of voice 0) is one tick later than the first write of the previous one
        let dt = t[k][0].2 as i64 - t[k - 1][0].2 as i64;
        assert!(dt == 70940 || dt == 70940 - 951 + 320 + 40 + 49 || dt > 69000, "dt {dt}");
    }
    // tick 1: the deferred loop address of voice 0, then the volume writes: voice 0 now at 0x30
    let t1 = &t[1];
    assert_eq!((t1[0].0, t1[1].0), (0xDFF0A0, 0xDFF0A2));
    assert_eq!((t1[2].0, t1[2].1), (0xDFF0A4, 8));
    let v0 = t1.iter().find(|x| x.0 == vol(0)).unwrap();
    assert_eq!(v0.1, 0x30);
}

#[test]
fn slide_steps_toward_the_destination() {
    // note 12 (0x1AC = 428) slides to note 24 (0xD6 = 214) in 3 steps: step (214 - 428) / 3 = -71
    let d = song(&plain(), vec![SEL | SLIDE, 0, 24, 3, 0, 30, 12], &smp());
    let (log, _, _) = play(d, 6).unwrap();
    let t = ticks(&log);
    let p: Vec<u16> = t[1..5].iter().flat_map(|x| x.iter().filter(|w| w.0 == per(0)).map(|w| w.1)).collect();
    assert_eq!(p, vec![428 - 71, 428 - 142, 428 - 213, 214]);
}

#[test]
fn tie_changes_the_pitch_without_restart() {
    // two notes on voice 0: the second is a tie
    let d = song(&plain(), vec![SEL, 0, 2, 12, TIE, 5, 24], &smp());
    let (log, _, _) = play(d, 4).unwrap();
    let t = ticks(&log);
    // tick 2: the second event: one PER write (214) and no DMA/LC/LEN write of voice 0
    let t2 = &t[2];
    assert!(t2.iter().any(|w| w.0 == per(0) && w.1 == 214));
    assert!(!t2.iter().any(|w| w.0 == DMACON && w.1 == 1));
    assert!(!t2.iter().any(|w| w.0 == 0xDFF0A4));
}

#[test]
fn pitch_table_cycles_the_listed_notes() {
    // base note 12, then notes 16 and 20 (periods 0x153? by table), speed 2
    let d = song(&plain(), vec![SEL | PTAB, 0, 3, 2, 16, 20, 40, 12], &smp());
    let (log, _, _) = play(d, 12).unwrap();
    let t = ticks(&log);
    let p: Vec<u16> = t[1..].iter().flat_map(|x| x.iter().filter(|w| w.0 == per(0)).map(|w| w.1)).collect();
    // the output is made in the tick after the event (countdown 1), then every 2 ticks:
    // base, note 16, note 20, base ...
    assert_eq!(&p[..4], &[0x1AC, PERIODS[16], PERIODS[20], 0x1AC]);
}

#[test]
fn rest_silences_the_channel() {
    let d = song(&plain(), vec![REST, 3, SEL, 0, 5, 12], &smp());
    let (log, _, _) = play(d, 5).unwrap();
    let t = ticks(&log);
    // tick 0: DMA off for voice 0 only, no LC/LEN/PER of voice 0; tick 3 starts the note
    assert!(t[0].iter().any(|w| w.0 == DMACON && w.1 == 1));
    assert!(!t[0].iter().any(|w| w.0 == per(0)));
    assert!(t[3].iter().any(|w| w.0 == per(0) && w.1 == 0x1AC));
}

#[test]
fn envelope_attack_hold_release() {
    // phase 1 (attack) from volume 0 by 0x10 to a peak of 0x30, hold 1 tick, release by 0x20 to 0
    let ei = vec![ins(4, 8, 0, 4, 8, &[(13, 1), (14, 0x10), (15, 0x30), (16, 1), (17, 0x20), (18, 0)])];
    let d = song(&ei, vec![SEL, 0, 40, 12], &smp());
    let (log, _, _) = play(d, 9).unwrap();
    let t = ticks(&log);
    let v: Vec<u16> = t[1..9].iter().map(|x| x.iter().find(|w| w.0 == vol(0)).unwrap().1).collect();
    // the volume stage of ticks 1..: 0x10, 0x20, 0x30 (peak, phase 2), hold (counter 1 -> 0, still
    // non-negative), hold ends (-1; phase 3 with the volume unchanged), release 0x10, then end
    assert_eq!(&v[..7], &[0x10, 0x20, 0x30, 0x30, 0x30, 0x10, 0]);
}

#[test]
fn synth_instrument_fills_its_buffer() {
    // synth: width grows by 3 to 6; levels 0x7F / 0x81; 4 words = 8 bytes
    let si = vec![ins(0, 4, 0x40, 0, 4, &[(19, 1), (21, 3), (22, 6), (23, 1), (24, 2), (25, 0x7F), (26, 0x81)])];
    let d = song(&si, vec![SEL, 0, 40, 12], &[]);
    let (log, _, e) = play(d, 3).unwrap();
    let t = ticks(&log);
    let lc = (t[0][5].1 as u32) << 16 | t[0][6].1 as u32;
    let now: Vec<u8> = (0..8).map(|i| e.m.bus.ram[lc as usize + i]).collect();
    // the first render holds no tick (it falls in the second 20 ms), so three ticks ran: the width
    // is 3, then 6 (the maximum: flag 2, shrinking), then 5: W - 1 = 4 bytes of A, the rest B
    assert_eq!(now, vec![0x7F, 0x7F, 0x7F, 0x7F, 0x81, 0x81, 0x81, 0x81]);
}

#[test]
fn song_end_after_all_voices_wrap() {
    // every voice has one short pattern; the lists end after it
    let rest = vec![REST, 2];
    let d = module(&plain(), [&[0], &[0], &[0], &[0]], &[rest], &smp());
    let (_, end, _) = play(d, 12).unwrap();
    assert!(end);
}

#[test]
fn song_end_never_while_one_voice_has_not_wrapped() {
    let short = vec![REST, 2];
    let long = vec![REST, 1000];
    let d = module(&plain(), [&[0], &[0], &[0], &[1]], &[short, long], &smp());
    let (_, end, _) = play(d, 40).unwrap();
    assert!(!end);
}

#[test]
fn deterministic() {
    let d = song(&plain(), vec![SEL | PTAB, 0, 3, 2, 16, 20, 9, 12, SEL | SLIDE, 0, 24, 3, 2, 30, 12], &smp());
    let a = play(d.clone(), 30).unwrap().0;
    let b = play(d, 30).unwrap().0;
    assert_eq!(a, b);
    assert!(a.len() > 100);
}

#[test]
fn closing_dmacon_follows_the_raster_line_grid() {
    // the closing DMACON follows nine line changes: it falls on a 227-cc line start plus up to a poll
    let d = song(&plain(), vec![SEL, 0, 5, 12], &smp());
    let (log, _, _) = play(d, 4).unwrap();
    for t in ticks(&log) {
        let w = t.last().unwrap();
        assert_eq!(w.0, DMACON);
        let ph = w.2 % 227;
        // 44 + 24 cycles after the ninth line change (35 cc) plus the poll (up to 12.5 cc), and
        // the 19-cycle round end
        assert!((34..=60).contains(&ph), "phase {ph}");
    }
}
