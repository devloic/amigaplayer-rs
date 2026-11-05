use super::*;
use crate::machine::Machine;

/// Builds a one-hunk module in code. The play routine points the chip base at the custom chips,
/// loads voice 0's location and length registers' sources and runs the location/length shape, then
/// the line-wait shape, then stores a marker period. The init routine points a work-area register
/// at the module's work area, stores `flag` in the song-end flag byte and runs the song-end shape.
/// The audio routine holds `vols` volume-load words (each with a pointer long), a store of a
/// volume through the register it is called with, and the end word. `play_body` replaces the play
/// routine when given.
fn module(flag: u8, vols: usize, play_body: Option<&[u16]>) -> Vec<u8> {
    let mut h: Vec<u8> = vec![0; 0x38];
    let mut relocs: Vec<u32> = vec![0x0C, 0x10, 0x14, 0x1C, 0x20, 0x24];
    let w = |h: &mut Vec<u8>, v: u16| h.extend(v.to_be_bytes());
    let l = |h: &mut Vec<u8>, v: u32| h.extend(v.to_be_bytes());
    h[0..4].copy_from_slice(&[0x70, 0xFF, 0x4E, 0x75]);
    h[4..12].copy_from_slice(b"DAGLISH!");
    let play = h.len();
    match play_body {
        Some(b) => b.iter().for_each(|&x| w(&mut h, x)),
        None => {
            // chip base in the fifth address register; location and length sources in the fourth
            // and third data registers
            for x in [0x4BF9u16, 0x00DF, 0xF000, 0x283C, 0x0001, 0x2345, 0x363C, 0x0020] {
                w(&mut h, x);
            }
            // the location/length shape of voice 0
            for x in [0x2B44u16, 0x00A0, 0x3B43, 0x00A4] {
                w(&mut h, x);
            }
            // the line-wait shape
            for x in [0x51C8u16, 0xFFFE, 0x302D, 0x0002] {
                w(&mut h, x);
            }
            // a marker period store through an absolute address, then return
            for x in [0x33FCu16, 0x0042, 0x00DF, 0xF0A6, 0x4E75] {
                w(&mut h, x);
            }
        }
    }
    let irqr = h.len();
    for _ in 0..vols {
        w(&mut h, 0x1239);
        relocs.push(h.len() as u32);
        l(&mut h, 0x30);
    }
    for x in [0x317Cu16, 0x0020, 0x00A8, 0x4E73] {
        w(&mut h, x);
    }
    let init = h.len();
    w(&mut h, 0x49F9);
    relocs.push(h.len() as u32);
    let work_ptr = h.len();
    l(&mut h, 0);
    for x in [0x197Cu16, flag as u16, 0x0011, 0x4BF9, 0x00DF, 0xF000] {
        w(&mut h, x);
    }
    // the song-end shape
    for x in [0x3B7Cu16, 0x000F, 0x0096, 0x4E75] {
        w(&mut h, x);
    }
    let name = h.len();
    h.extend(b"Song\0\0\0\0");
    while h.len() % 4 != 0 {
        h.push(0);
    }
    let work = h.len();
    h.extend([0u8; 0x20]);
    let mut put = |h: &mut Vec<u8>, o: usize, v: u32| h[o..o + 4].copy_from_slice(&v.to_be_bytes());
    put(&mut h, 0x0C, play as u32);
    put(&mut h, 0x10, irqr as u32);
    put(&mut h, 0x14, init as u32);
    put(&mut h, 0x18, 1);
    put(&mut h, 0x1C, name as u32);
    put(&mut h, 0x20, name as u32);
    put(&mut h, 0x24, name as u32);
    put(&mut h, work_ptr, work as u32);
    let mut f = Vec::new();
    for v in [0x3F3u32, 0, 1, 0, 0, 0x4000_0000 | (h.len() as u32 / 4), 0x3E9, h.len() as u32 / 4] {
        f.extend(v.to_be_bytes());
    }
    f.extend(&h);
    f.extend(0x3ECu32.to_be_bytes());
    f.extend((relocs.len() as u32).to_be_bytes());
    f.extend(0u32.to_be_bytes());
    for o in &relocs {
        f.extend(o.to_be_bytes());
    }
    f.extend(0u32.to_be_bytes());
    f.extend(0x3F2u32.to_be_bytes());
    f
}

fn start(d: &[u8]) -> (Machine, Box<dyn Replayer>) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let base = m.alloc(d.len() as u32);
    m.bus.write_bytes(base, d);
    let r = new(d, base, &mut m).expect("recognised");
    (m, r)
}

fn tick(m: &mut Machine, r: &mut Box<dyn Replayer>) {
    r.tick(&mut Hw { bus: &mut m.bus });
}

#[test]
fn detect_checks_the_header() {
    let d = module(1, 3, None);
    assert!(detect(&d));
    for (o, v) in [(3usize, 0u8), (0x14, 0), (0x23, 0), (0x24, b'X'), (0x2B, b'?')] {
        let mut bad = d.clone();
        bad[o] = v;
        assert!(!detect(&bad), "offset {o:#x}");
    }
    // each of the four longs at 0x2C..0x3C (play, audio routine, init, subsong count)
    for o in [0x2Cusize, 0x30, 0x34, 0x38] {
        let mut bad = d.clone();
        bad[o..o + 4].fill(0);
        assert!(!detect(&bad), "long at {o:#x}");
    }
    assert!(!detect(&d[..0x40]));
    assert!(!detect(&[]));
    assert!(!detect(b"RIFFRAFF"));
}

#[test]
fn site_keys_and_scan_words_parse() {
    let k = keys();
    assert_eq!(k.iter().map(|k| k.len()).collect::<Vec<_>>(), vec![3, 4, 4, 4, 4]);
    assert_eq!(SCAN_WORDS.len(), 6);
}

#[test]
fn the_audio_walk_counts_words_and_volume_loads() {
    let d = module(1, 3, None);
    let h = &d[0x20..];
    let irqr = be32(h, 0x10) as usize;
    // three volume-load words with their longs (the walk goes word by word), a store of three
    // words, and the end word: 3 * 3 + 3 + 1 words
    assert_eq!(scan(h, irqr), (13, 3, Some(irqr + 24)));
    // no end word: the walk stops at the end of the hunk
    let (w, m, end) = scan(&h[..irqr + 8], irqr);
    assert_eq!((w, m, end), (4, 2, None));
}

#[test]
fn start_up_runs_the_init_path_and_installs_the_audio_routine() {
    let d = module(1, 3, None);
    let (m, _r) = start(&d);
    let log = m.bus.write_log.clone().unwrap();
    // the song-end shape inside the init routine stores DMACON and does not report the song end
    let dma = log.iter().find(|e| e.0 == 0xDFF096).expect("dmacon");
    assert_eq!(dma.1, 0x000F);
    let i = 656 + 21 * 13 + 12 * 3;
    assert!(dma.2 >= i + 580 && dma.2 < i + 640, "{} vs {}", dma.2, i);
    // the end of the init path: the rest of the site, the return, 120 + 3 * 66 + 88 cycles with
    // three host points (a half cycle lost at each) and one for the 88: about 231 colour clocks
    let tail = m.bus.clock - dma.2;
    assert!((229..=233).contains(&tail), "{tail}");
}

#[test]
fn a_tick_writes_location_length_wait_and_marker() {
    let d = module(1, 3, None);
    let (mut m, mut r) = start(&d);
    m.bus.write_log = Some(Vec::new());
    m.bus.clock += TICK;
    tick(&mut m, &mut r);
    let log = m.bus.write_log.take().unwrap();
    assert!(!r.songend());
    let loc: Vec<_> = log.iter().filter(|e| e.0 == 0xDFF0A0 || e.0 == 0xDFF0A2).collect();
    assert_eq!(loc.len(), 2);
    assert_eq!((loc[0].1, loc[1].1), (1, 0x2345));
    assert_eq!(loc[0].2, loc[1].2);
    let len = log.iter().find(|e| e.0 == 0xDFF0A4).unwrap();
    assert_eq!(len.1, 0x20);
    // the length store follows the location store by 12 cycles
    assert_eq!(len.2 - loc[0].2, 6);
    // the line wait takes nine lines or a little more
    let marker = log.iter().find(|e| e.0 == 0xDFF0A6).unwrap();
    assert_eq!(marker.1, 0x42);
    assert!(marker.2 - len.2 > 1900 && marker.2 - len.2 < 2400, "{}", marker.2 - len.2);
    // a tick begins 256 cycles after its clock
    assert!(loc[0].2 >= m.bus.clock - 1 - 6000);
    // the flag byte is not zero: no song-end path, no stop routine
    assert!(log.iter().all(|e| e.0 != 0xDFF096));
}

#[test]
fn a_zero_flag_byte_stops_reports_and_restarts() {
    let d = module(0, 3, None);
    let (mut m, mut r) = start(&d);
    m.bus.write_log = Some(Vec::new());
    m.bus.clock += TICK;
    tick(&mut m, &mut r);
    let log = m.bus.write_log.take().unwrap();
    assert!(r.songend());
    // the stop routine: DMACON, then the four volumes 8, 6, 6, 6 colour clocks later, and after
    // it the init path's own DMACON write (the song-end shape inside the init routine)
    let dmas: Vec<usize> = log.iter().enumerate().filter(|(_, e)| e.0 == 0xDFF096).map(|(i, _)| i).collect();
    assert_eq!(dmas.len(), 2);
    let a = dmas[0];
    assert_eq!(log[a].1, 0x000F);
    let vols: Vec<_> = log[a + 1..a + 5].iter().map(|e| (e.0, e.1, e.2 - log[a].2)).collect();
    assert_eq!(vols, vec![(0xDFF0A8, 0, 8), (0xDFF0B8, 0, 14), (0xDFF0C8, 0, 20), (0xDFF0D8, 0, 26)]);
    assert_eq!(log[dmas[1]].1, 0x000F);
}

#[test]
fn a_song_end_shape_outside_the_init_routine_reports_the_end() {
    // the play routine runs the song-end shape with the chip base set: it is reported at once
    let body = [0x4BF9u16, 0x00DF, 0xF000, 0x3B7C, 0x000F, 0x0096, 0x4E75];
    let d = module(1, 3, Some(&body));
    let (mut m, mut r) = start(&d);
    assert!(!r.songend());
    m.bus.clock += TICK;
    m.bus.write_log = Some(Vec::new());
    tick(&mut m, &mut r);
    assert!(r.songend());
    let log = m.bus.write_log.take().unwrap();
    assert_eq!(log.iter().filter(|e| e.0 == 0xDFF096).count(), 1);
}

#[test]
fn the_audio_routine_runs_only_for_a_pending_request() {
    let d = module(1, 3, None);
    let (mut m, mut r) = start(&d);
    m.bus.write_log = Some(Vec::new());
    m.bus.clock += 1000;
    // no request: nothing runs
    assert!(r.audio_irq_ch(0, &mut Hw { bus: &mut m.bus }));
    assert!(m.bus.write_log.as_ref().unwrap().is_empty());
    m.bus.paula.intreq |= 0x80;
    let t = m.bus.clock;
    assert!(r.audio_irq_ch(0, &mut Hw { bus: &mut m.bus }));
    let log = m.bus.write_log.take().unwrap();
    // the routine stores through the chip base it is called with, 0xDFF000
    assert_eq!(log.len(), 1);
    assert_eq!((log[0].0, log[0].1), (0xDFF0A8, 0x20));
    assert!(log[0].2 > t);
    assert!(m.bus.clock >= log[0].2);
}

#[test]
fn a_subsong_start_runs_the_stop_routine_then_the_init_path() {
    let d = module(1, 3, None);
    let (mut m, mut r) = start(&d);
    assert_eq!(r.title(), "Song");
    assert_eq!(r.subsongs(), (0, 0, 0));
    let e = m.bus.clock;
    m.bus.write_log = Some(Vec::new());
    r.set_subsong(0, &mut Hw { bus: &mut m.bus });
    let log = m.bus.write_log.take().unwrap();
    // the stop routine from count 0 at E: DMACON at E + 135, volumes at +143, +149, +155, +161
    assert_eq!(log[0].0, 0xDFF096);
    assert_eq!(log[0].2, e + 135);
    let vols: Vec<_> = log[1..5].iter().map(|x| (x.0, x.2 - e)).collect();
    assert_eq!(vols, vec![(0xDFF0A8, 143), (0xDFF0B8, 149), (0xDFF0C8, 155), (0xDFF0D8, 161)]);
    // the init path follows from E + 169, and the first tick is one period after its end
    assert_eq!(r.subsong_tick_at(), Some(m.bus.clock + TICK));
    assert_eq!(r.subsong_tick_at(), None);
    // a number out of range is ignored
    m.bus.write_log = Some(Vec::new());
    r.set_subsong(5, &mut Hw { bus: &mut m.bus });
    assert_eq!(m.bus.write_log.as_ref().unwrap().len(), 0);
}

#[test]
fn malformed_files_are_not_recognised_or_harmless() {
    let d = module(1, 3, None);
    let mut m = Machine::new(CpuType::M68000);
    // truncated hunk: fewer bytes than the size long says
    assert!(new(&d[..0x80], 0, &mut m).is_none());
    // an entry pointer past the end of the hunk
    for o in [0x0Cusize, 0x10, 0x14] {
        let mut bad = d.clone();
        bad[0x20 + o..0x20 + o + 4].copy_from_slice(&0x00FF_0000u32.to_be_bytes());
        assert!(new(&bad, 0, &mut m).is_none());
    }
    // an audio routine with no end word, and a header with a huge subsong count
    let mut bad = d.clone();
    bad[0x20 + 0x18..0x20 + 0x1C].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
    let r = new(&bad, 0, &mut m);
    assert!(r.is_some());
    assert!(r.unwrap().subsongs().1 < 0xFFFF);
}

#[test]
fn a_driver_that_never_returns_ends_the_song() {
    // the play routine branches to itself
    let d = module(1, 3, Some(&[0x60FE]));
    let (mut m, mut r) = start(&d);
    m.bus.clock += TICK;
    tick(&mut m, &mut r);
    assert!(r.songend());
}

#[test]
fn ticks_are_deterministic() {
    let run = || {
        let d = module(1, 3, None);
        let (mut m, mut r) = start(&d);
        for _ in 0..3 {
            m.bus.clock += TICK;
            tick(&mut m, &mut r);
        }
        m.bus.write_log.take().unwrap()
    };
    assert_eq!(run(), run());
}
