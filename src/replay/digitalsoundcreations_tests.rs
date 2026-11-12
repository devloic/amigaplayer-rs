use super::*;
use crate::machine::Machine;
use m68k::CpuType;

/// A Populous module (SPEC section 2): pattern lengths, the sequence, notes as (pattern, track,
/// row, byte), instrument entries (period, length, volume, sample, loop offset) and samples
/// (data, looped).
fn populous(head: &[(usize, u16)], seq: &[(u16, u16)], notes: &[(usize, usize, usize, u8)], ents: &[(u16, u16, u8, u8, u16)], smps: &[(Vec<u8>, bool)]) -> Vec<u8> {
    let mut h = [64u16; 64];
    for &(p, n) in head {
        h[p] = n;
    }
    let mut d = Vec::new();
    for v in h {
        d.extend_from_slice(&v.to_be_bytes());
    }
    d.extend_from_slice(&(seq.len() as u16).to_be_bytes());
    for &(p, c) in seq {
        d.extend_from_slice(&p.to_be_bytes());
        d.extend_from_slice(&c.to_be_bytes());
    }
    d.extend_from_slice(&[0, 0, 0x40, 0]);
    let mut grid = vec![0x20u8; GRID];
    for &(p, k, r, c) in notes {
        grid[p * 256 + k * 64 + r] = c;
    }
    d.extend_from_slice(&grid);
    d.extend_from_slice(&104u16.to_be_bytes());
    for i in 0..104 {
        let (per, len, vol, s, lo) = ents.get(i).copied().unwrap_or((428, 2, 0, 0, 0));
        d.extend_from_slice(&per.to_be_bytes());
        d.extend_from_slice(&len.to_be_bytes());
        d.extend_from_slice(&[vol, s]);
        d.extend_from_slice(&lo.to_be_bytes());
    }
    d.extend_from_slice(&(smps.len() as u16).to_be_bytes());
    let mut off = 1000u32;
    for (s, looped) in smps {
        d.extend_from_slice(&(s.len() as u16).to_be_bytes());
        d.extend_from_slice(&(if *looped { 0 } else { s.len() as u16 }).to_be_bytes());
        d.extend_from_slice(&off.to_be_bytes());
        // the offsets have gaps the player does not follow (the lengths place the samples)
        off += s.len() as u32 + 6;
    }
    let data: Vec<u8> = smps.iter().flat_map(|s| s.0.clone()).collect();
    d.extend_from_slice(&(data.len() as u32).to_be_bytes());
    d.extend_from_slice(&data);
    d.extend_from_slice(&[0, 0]);
    d
}

/// A DTRK song: tempo T (9 bits), the sequence, pattern lengths, notes.
fn dtrk(t: u16, seq: &[(u16, u8)], head: &[(usize, u16)], notes: &[(usize, usize, usize, u8)]) -> Vec<u8> {
    let mut thdr = vec![0x20u8; 43];
    thdr[..4].copy_from_slice(b"kit\0");
    thdr[37] = if t & 0x100 != 0 { 0x80 } else { 0 };
    thdr[40] = t as u8;
    let mut grid = vec![0x20u8; GRID];
    for &(p, k, r, c) in notes {
        grid[p * 256 + k * 64 + r] = c;
    }
    let seqn: Vec<u8> = seq.iter().flat_map(|&(p, c)| [(p >> 8) as u8, p as u8, c]).collect();
    let mut h = [64u16; 64];
    for &(p, n) in head {
        h[p] = n;
    }
    let mlen: Vec<u8> = h.iter().flat_map(|v| v.to_be_bytes()).collect();
    let mut body = b"DTRK".to_vec();
    for (id, c) in [(b"THDR", &thdr), (b"BODY", &grid), (b"SEQN", &seqn), (b"MLEN", &mlen)] {
        body.extend_from_slice(id);
        body.extend_from_slice(&(c.len() as u32).to_be_bytes());
        body.extend_from_slice(c);
    }
    let mut d = b"FORM".to_vec();
    d.extend_from_slice(&(body.len() as u32).to_be_bytes());
    d.extend_from_slice(&body);
    d
}

/// A kit of entries (rate, length, volume, sample number, loop word).
fn kit(ents: &[(u32, u32, u8, u8, u16)]) -> Vec<u8> {
    let mut b = Vec::new();
    for &(a, len, vol, s, lo) in ents {
        let mut e = [0u8; 44];
        e[..4].copy_from_slice(b"inst");
        e[32..36].copy_from_slice(&a.to_be_bytes());
        e[36..40].copy_from_slice(&len.to_be_bytes());
        e[40] = vol;
        e[41] = s;
        e[42..44].copy_from_slice(&lo.to_be_bytes());
        b.extend_from_slice(&e);
    }
    let mut d = b"FORM\0\0\0\0DKITKHDR\0\0\0\x04h,\0\0BODY".to_vec();
    d.extend_from_slice(&(b.len() as u32).to_be_bytes());
    d.extend_from_slice(&b);
    let n = (d.len() - 8) as u32;
    d[4..8].copy_from_slice(&n.to_be_bytes());
    d
}

/// An 8SVX FORM holding `data`.
fn svx(data: &[u8]) -> Vec<u8> {
    let mut body = b"8SVXVHDR\0\0\0\x14".to_vec();
    body.extend_from_slice(&[0; 20]);
    body.extend_from_slice(b"BODY");
    body.extend_from_slice(&(data.len() as u32).to_be_bytes());
    body.extend_from_slice(data);
    if data.len() % 2 == 1 {
        body.push(0);
    }
    let mut d = b"FORM".to_vec();
    d.extend_from_slice(&(body.len() as u32).to_be_bytes());
    d.extend_from_slice(&body);
    d
}

const BASE: u32 = 0x20000;

/// Plays `r` as the engine calls an open replayer: at each due time (the next tick, or the replayer's
/// `next_tick_at`), with Paula run to it first. Returns the audio writes (time relative to the
/// first tick) and the song end's tick time.
fn drive(r: &mut DigitalSoundCreations, m: &mut Machine, clocks: u64) -> (Vec<(u32, u16, u64)>, Option<u64>) {
    m.bus.write_log = Some(Vec::new());
    let start = 1_000_000;
    m.bus.clock = start;
    m.bus.paula.sync(start, &m.bus.ram);
    let mut due = start + r.tick_clocks();
    let mut end = None;
    for _ in 0..2_000_000 {
        if due > start + clocks {
            break;
        }
        m.bus.clock = m.bus.clock.max(due);
        let now = m.bus.clock;
        m.bus.paula.sync(now, &m.bus.ram);
        r.tick(&mut Hw { bus: &mut m.bus });
        if end.is_none() && r.songend() {
            end = Some(now - (start + r.tick_clocks()));
        }
        due = r.next_tick_at().unwrap_or(due + r.tick_clocks());
    }
    let l = m.bus.write_log.take().unwrap();
    let first = start + r.tick_clocks();
    (l.into_iter().filter(|w| (0xDFF096..=0xDFF0DF).contains(&w.0)).map(|(a, v, t)| (a, v, t - first)).collect(), end)
}

fn pop_player(d: &[u8]) -> (DigitalSoundCreations, Machine) {
    let (song, lay) = pop_layout(d).unwrap();
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_bytes(BASE, d);
    (DigitalSoundCreations::build(song, pop_notes(d, &lay, BASE)), m)
}

/// The writes at time `t`.
fn at(l: &[(u32, u16, u64)], t: u64) -> Vec<(u32, u16)> {
    l.iter().filter(|w| w.2 == t).map(|w| (w.0, w.1)).collect()
}

#[test]
fn detect_takes_both_kinds_and_rejects_changes() {
    let d = populous(&[], &[(0, 1)], &[], &[], &[(vec![1; 8], false)]);
    assert!(detect(&d));
    let mut x = d.clone();
    x.push(0); // the size no longer fits
    assert!(!detect(&x));
    let mut x = d.clone();
    x[0x86] = 0x41; // the grid length word
    assert!(!detect(&x));
    let s = dtrk(432, &[(0, 1)], &[], &[]);
    assert!(detect(&s));
    let mut x = s.clone();
    x[8..12].copy_from_slice(b"DKIT");
    assert!(!detect(&x));
    // a BODY of another size
    let mut x = s.clone();
    let b = x.windows(4).position(|w| w == b"BODY").unwrap();
    x[b + 6] = 0x3F;
    assert!(!detect(&x));
    for n in [0usize, 1, 12, 200, 20000] {
        let r: Vec<u8> = (0..n as u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        assert!(!detect(&r));
    }
}

#[test]
fn dtrk_period_and_tick() {
    assert_eq!(dtrk_period(4363), 820);
    assert_eq!(dtrk_period(23705), 151);
    assert_eq!(dtrk_period(75536), 357); // the rate's low word (10000)
    assert_eq!(dtrk_period(32), 0x9E99); // the division overflows: the dividend's low word
    assert_eq!(dtrk_period(0), 0x9E99);
    // the reference's tick periods: Fusion 432, Druid_music 399, Druid_game 329, and probes
    for (t, p) in [(432, 80285), (399, 86530), (329, 103570), (511, 68430), (256, 130235), (128, 236455), (255, 130695)] {
        let (song, _) = dtrk_layout(&dtrk(t, &[(0, 1)], &[], &[])).unwrap();
        assert_eq!(song.tick, p, "tempo {t}");
    }
}

#[test]
fn bank_table_has_an_empty_slot_at_a_gap() {
    let mut b = svx(&[1; 10]);
    b.extend_from_slice(&svx(&[2; 6]));
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&svx(&[3; 4]));
    let s = bank_samples(&b);
    assert_eq!(s.len(), 4);
    assert!(s[0].is_some() && s[1].is_some() && s[2].is_none() && s[3].is_some());
    assert_eq!(&b[s[3].unwrap()..s[3].unwrap() + 4], &[3; 4]);
    assert_eq!(bank_name("dh1:music/han.Fusion"), "smp.Fusion");
    assert_eq!(bank_name("Fusion"), "smp.Fusion");
    // sample 0 is the first; past the table, or the empty slot: no sample
    let k = kit(&[(4363, 10, 64, 0, 0), (4363, 10, 64, 1, 0), (4363, 10, 64, 3, 0), (4363, 10, 64, 5, 0)]);
    let n = dtrk_notes(&k, kit_body(&k).unwrap(), &s, 0x1000);
    assert_eq!(n[0].sample, n[1].sample);
    assert_eq!(n[0].sample, Some((0x1000 + s[0].unwrap() as u32, 10)));
    assert_eq!(n[2].sample, None);
    assert_eq!(n[3].sample, None);
    // entries past the kit read zeros: rate 0, no sample... sample 0 is the first
    assert_eq!(n[50].vol, 0);
}

#[test]
fn a_note_writes_at_the_reference_times_and_ends_after_one_pass() {
    // entry 0: 400 bytes of sample 0, volume 40, on track 0 row 0
    let d = populous(&[], &[(0, 1)], &[(0, 0, 0, 0x41)], &[(400, 400, 40, 0, 0)], &[(vec![9; 400], false)]);
    let (mut r, mut m) = pop_player(&d);
    let (l, _) = drive(&mut r, &mut m, 30 * POP_TICK);
    // the first row is the tenth tick; its CMD_WRITE 1032 colour clocks in
    let t0 = 9 * POP_TICK + 1032;
    let data = BASE + pop_layout(&d).unwrap().1.data as u32;
    assert_eq!(
        at(&l, t0),
        vec![(0xDFF0A0, (data >> 16) as u16), (0xDFF0A2, data as u16), (0xDFF0A4, 200), (0xDFF096, 1), (0xDFF096, 0x8001)]
    );
    // period and volume once Paula takes the sample
    let pv: Vec<_> = l.iter().filter(|w| w.0 == 0xDFF0A6 || w.0 == 0xDFF0A8).collect();
    assert_eq!(pv.iter().map(|w| w.1).collect::<Vec<_>>(), vec![400, 40]);
    // (the tick ends 592 colour clocks after the write; Paula is seen at the output sample after)
    assert!(pv[0].2 > t0 + 592 && pv[0].2 < t0 + 592 + 80, "{} {t0}", pv[0].2);
    // the DMA off after a pass (400 bytes at period 400: 160000 colour clocks, more for the words
    // Paula fetched before the period was set)
    let off: Vec<_> = l.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 1 && w.2 > t0).collect();
    assert_eq!(off.len(), 1);
    assert!(off[0].2 - pv[0].2 > 160000 && off[0].2 - pv[0].2 < 300000, "{}", off[0].2 - pv[0].2);
}

#[test]
fn a_quieter_note_waits_for_the_louder_one() {
    // a long sample (no end within the test) at volume 40; then 30 (skipped), 40 (restarts), 50 (restarts)
    let notes = [(0, 0, 0, 0x41), (0, 0, 1, 0x42), (0, 0, 2, 0x41), (0, 0, 3, 0x43)];
    let ents = [(1000, 60000, 40, 0, 0), (1000, 60000, 30, 0, 0), (1000, 60000, 50, 0, 0)];
    let d = populous(&[], &[(0, 1)], &notes, &ents, &[(vec![9; 60000], false)]);
    let (mut r, mut m) = pop_player(&d);
    let (l, _) = drive(&mut r, &mut m, 45 * POP_TICK);
    let starts: Vec<u64> = l.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 0x8001).map(|w| w.2 / POP_TICK).collect();
    assert_eq!(starts, vec![9, 29, 39]);
    // a held request is aborted (its DMA off) at the AbortIO, 91 colour clocks after the CheckIO
    let abort = 29 * POP_TICK + 340 + 91;
    assert_eq!(at(&l, abort), vec![(0xDFF096, 1)]);
    assert!(at(&l, abort + 638).contains(&(0xDFF096, 0x8001)));
}

#[test]
fn a_looped_sample_plays_on() {
    let d = populous(&[], &[(0, 1)], &[(0, 1, 0, 0x41)], &[(300, 200, 64, 0, 6)], &[(vec![9; 200], true)]);
    let (mut r, mut m) = pop_player(&d);
    let (l, _) = drive(&mut r, &mut m, 60 * POP_TICK);
    let dma: Vec<_> = l.iter().filter(|w| w.0 == 0xDFF096).collect();
    // started once (track 1, 1032 + 121 colour clocks in, after an empty track 0), never stopped
    assert_eq!(dma.len(), 2);
    assert_eq!(dma[1].2, 9 * POP_TICK + 461 + 54 + 683);
}

#[test]
fn the_sequence_repeats_by_a_counter_never_reset() {
    // entry 0 pattern 1 count 1 (once), entry 1 pattern 2 count 3: the counter is 1 after entry 0,
    // so pattern 2 plays twice (2 < 3, then 3)
    let notes = [(1, 0, 0, 0x41), (2, 0, 0, 0x41)];
    let d = populous(&[(1, 2), (2, 3)], &[(1, 1), (2, 3)], &notes, &[(300, 2, 64, 0, 0)], &[(vec![9; 2], false)]);
    let (mut r, mut m) = pop_player(&d);
    let (l, end) = drive(&mut r, &mut m, 200 * POP_TICK);
    let starts: Vec<u64> = l.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 0x8001).map(|w| w.2 / POP_TICK).collect();
    // rows at ticks 9, 19 (pattern 1), 29, 39, 49 and 59, 69, 79 (pattern 2 twice)
    assert_eq!(starts, vec![9, 29, 59]);
    assert_eq!(end, Some(79 * POP_TICK));
    // no entry at all: pattern 0 once
    let d = populous(&[(0, 3)], &[], &[(0, 2, 2, 0x41)], &[(300, 2, 64, 0, 0)], &[(vec![9; 2], false)]);
    let (mut r, mut m) = pop_player(&d);
    let (_, end) = drive(&mut r, &mut m, 100 * POP_TICK);
    assert_eq!(end, Some(29 * POP_TICK));
}

#[test]
fn dtrk_plays_from_the_bank_with_the_kit() {
    let s = dtrk(432, &[(0, 1)], &[(0, 2)], &[(0, 2, 0, 0x42), (0, 3, 1, 0x41)]);
    let (song, _) = dtrk_layout(&s).unwrap();
    let mut b = svx(&[5; 100]);
    b.extend_from_slice(&svx(&[6; 300]));
    let k = kit(&[(8363, 100, 64, 1, 0), (4363, 300, 30, 2, 0)]);
    let samples = bank_samples(&b);
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_bytes(BASE, &b);
    let mut r = DigitalSoundCreations::build(song, dtrk_notes(&k, kit_body(&k).unwrap(), &samples, BASE));
    let (l, end) = drive(&mut r, &mut m, 20 * 80285);
    // row 0 at the fourth tick: track 2 (entry 1) after two empty tracks
    let t0 = 3 * 80285 + 340 + 121 + 120 + 54 + 733;
    let a = BASE + samples[1].unwrap() as u32;
    assert_eq!(at(&l, t0)[..3], [(0xDFF0C0, (a >> 16) as u16), (0xDFF0C2, a as u16), (0xDFF0C4, 150)]);
    let per: Vec<u16> = l.iter().filter(|w| w.0 == 0xDFF0C6 || w.0 == 0xDFF0D6).map(|w| w.1).collect();
    assert_eq!(per, vec![820, 428]);
    assert_eq!(end, Some(7 * 80285));
}

#[test]
fn malformed_input_does_not_panic_or_hang() {
    // pattern numbers past 63, lengths 0 and 65535, every byte value as a note, entries without samples
    let notes: Vec<(usize, usize, usize, u8)> = (0..256).map(|i| (i % 64, (i / 64) % 4, i % 64, i as u8)).collect();
    let ents: Vec<(u16, u16, u8, u8, u16)> = (0..104).map(|i| (i as u16 * 7, 0xFFFF - i as u16, i as u8, i as u8, 0xFFFF)).collect();
    for seq in [vec![(70u16, 0u16)], vec![(0, 0xFFFF), (1, 2), (63, 1)], vec![(5, 1); 300]] {
        let d = populous(&[(0, 0), (1, 65535), (63, 200)], &seq, &notes, &ents, &[(vec![1; 3], true), (vec![], false)]);
        assert!(detect(&d));
        let (mut r, mut m) = pop_player(&d);
        let _ = drive(&mut r, &mut m, 400 * POP_TICK);
    }
    for t in [0u16, 1, 83, 511] {
        let s = dtrk(t, &[(64, 0), (0xFFFF, 255), (3, 2)], &[(3, 0)], &[(3, 0, 0, 0xA8), (3, 1, 0, 0x00)]);
        let (song, _) = dtrk_layout(&s).unwrap();
        assert!(song.tick > 0);
        let k = kit(&[(0, 0xFFFF_FFFF, 255, 255, 1); 3]);
        let mut m = Machine::new(CpuType::M68000);
        let mut r = DigitalSoundCreations::build(song, dtrk_notes(&k, kit_body(&k).unwrap(), &[Some(0), None], BASE));
        let _ = drive(&mut r, &mut m, 200 * 80000);
    }
    // a bank of junk, a kit without a body
    assert!(bank_samples(&[0x46, 0x4F, 0x52, 0x4D, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]).len() <= 1);
    assert!(kit_body(b"FORM\0\0\0\x04DKIT").is_none());
    // truncated modules
    let d = populous(&[], &[(0, 1)], &[], &[], &[(vec![1; 8], false)]);
    for n in (0..d.len()).step_by(997) {
        assert!(!detect(&d[..n]));
    }
    let s = dtrk(432, &[(0, 1)], &[], &[]);
    for n in (0..s.len()).step_by(331) {
        assert!(!detect(&s[..n]));
    }
}

const CORPUS: &str = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/dl-modules/Digital_Sound_Creations/files";

#[test]
fn real_modules_when_present() {
    let songs = [
        ("Populous/han.gmusic", 99.3),
        ("Populous/han.gwords", 12.9),
        ("Populous/The_Final_Frontier/han.gwords", 7.9),
        ("Fusion/han.Fusion", 69.6),
        ("Enlightenment/han.Druid_music", 285.4),
        ("Enlightenment/han.Druid_game", 166.4),
    ];
    for (f, secs) in songs {
        let p = format!("{CORPUS}/{f}");
        let Ok(d) = std::fs::read(&p) else { continue };
        assert!(detect(&d), "{p}");
        let mut e = crate::engine::Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(std::path::Path::new(&p), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Digital Sound Creations");
        // the reference's song end (to the 0.1 s it reports)
        let mut buf = vec![0f32; 2 * 4800];
        let mut n = 0;
        while !e.songend() && n < 4000 {
            e.render(&mut buf);
            n += 1;
        }
        assert!((n as f64 / 10.0 - secs).abs() <= 0.15, "{p}: {n}");
    }
    // the kits and banks are not modules
    for f in ["Fusion/Fusion_kit", "Fusion/smp.Fusion", "Enlightenment/Druid_game_kit", "Enlightenment/smp.Druid_music"] {
        if let Ok(d) = std::fs::read(format!("{CORPUS}/{f}")) {
            assert!(!detect(&d), "{f}");
        }
    }
}

/// No file of the corpus lists (`ports/*.txt`, one path a line) is taken but this format's own.
/// Run with `--ignored`.
#[test]
#[ignore]
fn takes_no_module_of_the_corpus_lists() {
    let dir = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/ports";
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let (mut read, mut taken) = (0, Vec::new());
    for l in rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "txt")) {
        for p in std::fs::read_to_string(&l).unwrap_or_default().lines().filter(|p| !p.starts_with('#')) {
            let Ok(d) = std::fs::read(p.trim()) else { continue };
            read += 1;
            if detect(&d) {
                taken.push(p.to_string());
            }
        }
    }
    eprintln!("{read} files read, {} taken", taken.len());
    assert!(taken.iter().all(|p| p.contains("Digital_Sound_Creations")), "{taken:?}");
}
