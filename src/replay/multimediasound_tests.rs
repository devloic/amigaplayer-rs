use super::*;
use crate::engine::Engine;
use std::path::Path;

/// A module with `songlen` positions (position `p` plays pattern `order[p]`), one sample (number
/// 1: 32 words, volume 48, loop start 0, loop length 1), tempo `tempo`; `rows` are
/// (pattern, row, voice, period word, byte 2, byte 3) entries.
fn module(order: &[u8], tempo: u16, rows: &[(usize, usize, usize, u16, u8, u8)]) -> Vec<u8> {
    let max = *order.iter().max().unwrap() as usize;
    let sample = 64usize;
    let mut d = vec![0u8; 0x4B8 + 1024 * (max + 1) + sample];
    d[0..4].copy_from_slice(&(sample as u32).to_be_bytes());
    d[0x7C..0x80].copy_from_slice(b"SO31");
    d[0x80..0x82].copy_from_slice(&tempo.to_be_bytes());
    d[0x90 + 22..0x90 + 30].copy_from_slice(&[0, 32, 0, 48, 0, 0, 0, 1]);
    d[0x432] = order.len() as u8;
    d[0x434..0x434 + order.len()].copy_from_slice(order);
    for &(p, r, v, w, b2, b3) in rows {
        let o = 0x4B8 + 1024 * p + 16 * r + 4 * v;
        d[o..o + 2].copy_from_slice(&w.to_be_bytes());
        d[o + 2] = b2;
        d[o + 3] = b3;
    }
    let s = 0x4B8 + 1024 * (max + 1);
    for (i, b) in d[s..].iter_mut().enumerate() {
        *b = 0x40 + i as u8;
    }
    d
}

/// Plays `d` under `name` for `ticks` ticks of 960 samples and returns the write log (without
/// the player's own start-up writes) and the engine.
fn play(name: &str, d: Vec<u8>, ticks: usize) -> (Vec<(u32, u16, u64)>, Engine) {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new(name), d).unwrap();
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..ticks {
        e.render(&mut out);
    }
    let log = e.m.bus.write_log.take().unwrap();
    (log, e)
}

fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

#[test]
fn tables_are_the_specified_ones() {
    assert_eq!(PERIODS.len(), 87);
    assert_eq!(&PERIODS[..20], &[1076; 20]);
    assert_eq!(PERIODS[20], 1076);
    assert_eq!(PERIODS[20 + 39], 113);
    assert_eq!(PERIODS[20 + 65], 113);
    assert_eq!(PERIODS[86], 0xFFFF);
    // the player's own tables in the specification folder, when this machine has it
    let dir = Path::new("/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/spec/multimediasound/data");
    if let Ok(t) = std::fs::read(dir.join("period_table.bin")) {
        let w: Vec<u16> = t.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        assert_eq!(w, PERIODS);
    }
    if let Ok(t) = std::fs::read(dir.join("vibrato_sine.bin")) {
        assert_eq!(t, SINE);
    }
}

#[test]
fn name_rule() {
    for n in ["mms.song", "MMS.Song", "sfx20.x", "SFX20.X", "tune.mms", "tune.SFX20"] {
        assert!(name_ok(n), "{n}");
    }
    for n in ["sfx.song", "SFX2.song", "mmsong", "song", "", "x.mmsx"] {
        assert!(!name_ok(n), "{n}");
    }
}

#[test]
fn detection_needs_name_and_content() {
    let d = module(&[0], 14000, &[]);
    let mine = |name: &str| {
        crate::replay::adpcmmono::set_name(name);
        let r = crate::replay::REPLAYERS.iter().find(|o| (o.detect)(&d)).map(|o| o.name);
        crate::replay::adpcmmono::set_name("");
        r
    };
    assert_eq!(mine("mms.test"), Some("MultiMedia Sound"));
    assert_eq!(mine("SFX20.test"), Some("MultiMedia Sound"));
    assert_eq!(mine("test.mms"), Some("MultiMedia Sound"));
    // the same file under another name, or none, is a SoundFX 2.0 module
    assert_eq!(mine("SFX2.test"), Some("SoundFX2.0"));
    assert_eq!(mine("test.sfx"), Some("SoundFX2.0"));
    assert_eq!(mine(""), Some("SoundFX2.0"));
    // content: odd length, too long, tempo 0, no mark, short
    let mut bad = d.clone();
    bad[3] = 1;
    assert!(!content_ok(&bad));
    let mut bad = d.clone();
    bad[1] = 3;
    assert!(!content_ok(&bad));
    let mut bad = d.clone();
    bad[0x80] = 0;
    bad[0x81] = 0;
    assert!(!content_ok(&bad));
    let mut bad = d.clone();
    bad[0x7C] = b'X';
    assert!(!content_ok(&bad));
    assert!(!content_ok(&d[..0x80]));
    assert!(content_ok(&d));
}

#[test]
fn soundfx_module_under_another_name_still_goes_to_soundfx() {
    let d = module(&[0], 14000, &[(0, 0, 0, 428, 0x10, 0)]);
    let (_, e) = play("tune.sfx", d.clone(), 2);
    assert_eq!(e.playing.as_ref().unwrap().format, "SoundFX2.0");
    let (_, e) = play("mms.tune", d, 2);
    assert_eq!(e.playing.as_ref().unwrap().format, "MultiMedia Sound");
}

#[test]
fn malformed_modules_are_refused_not_crashed() {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    // the samples do not fit
    let mut d = module(&[0], 14000, &[]);
    d.truncate(d.len() - 2);
    assert!(e.play_data(Path::new("mms.short"), d).is_err());
    // song length 0
    let mut d = module(&[0], 14000, &[]);
    d[0x432] = 0;
    assert!(e.play_data(Path::new("mms.zero"), d).is_err());
    // an order list naming patterns past the end
    let mut d = module(&[0], 14000, &[]);
    d[0x434] = 200;
    assert!(e.play_data(Path::new("mms.far"), d).is_err());
}

/// The first row is played on the sixth tick; its first voice writes the volume at cycle 1380 of
/// the tick (the specification's worked example), then DMA off, the sample, period; DMA on after
/// two beam waits, then the loops of voices 3 to 0.
#[test]
fn first_row_writes() {
    let d = module(&[0], 14187, &[(0, 0, 0, 428, 0x10, 0)]);
    let spacing = (14187 + 1) * 5;
    FIRST_TICK.with(|c| c.set(Some(100_000)));
    let (log, e) = play("mms.test", d, 8);
    FIRST_TICK.with(|c| c.set(None));
    assert_eq!(e.playing.as_ref().unwrap().format, "MultiMedia Sound");
    let row = 100_000 + 5 * spacing as u64;
    let first = log.iter().find(|w| w.2 >= row).unwrap();
    assert_eq!((first.0, first.1, first.2), (0xDFF0A8, 48, row + 690));
    let after: Vec<_> = log.iter().filter(|w| w.2 >= row).map(|w| (w.0, w.1)).collect();
    let names = [
        (0xDFF0A8, 48),
        (0xDFF096, 1),
        (0xDFF0A0, 0),
        (0xDFF0A4, 32),
        (0xDFF0A6, 428),
        (0xDFF0B8, 0),
        (0xDFF0C8, 0),
        (0xDFF0D8, 0),
    ];
    // voice 0's start is the sample, so its location words follow the first length
    assert_eq!(after[0], names[0]);
    assert_eq!(after[1], names[1]);
    assert_eq!(after[2].0, 0xDFF0A0);
    assert_eq!(after[3].0, 0xDFF0A2);
    assert_eq!(after[4], names[3]);
    assert_eq!(after[5], names[4]);
    // volumes of the other voices follow, with no sample: 0
    assert_eq!((after[6], after[7], after[8]), (names[5], names[6], names[7]));
    // DMA on carries the voice, then the loops: voice 3, 2, 1, 0
    let on = after.iter().position(|w| w.0 == 0xDFF096 && w.1 == 0x8001).unwrap();
    let loops: Vec<u32> = after[on + 1..].iter().filter(|w| w.0 % 16 == 0 || w.0 % 16 == 4).map(|w| w.0).collect();
    assert_eq!(&loops[..8], &[0xDFF0D0, 0xDFF0D4, 0xDFF0C0, 0xDFF0C4, 0xDFF0B0, 0xDFF0B4, 0xDFF0A0, 0xDFF0A4]);
    // the first long of the sample was cleared
    let a = (reg(&log, 0xDFF0A0)[0] as u32) << 16 | reg(&log, 0xDFF0A2)[0] as u32;
    assert_eq!(e.m.bus.r32(a), 0);
    assert_eq!(e.m.bus.r8(a + 4), 0x44);
}

#[test]
fn speed_command_changes_the_row_length() {
    // F03 on the first row: the command runs on the first effect tick after it (timer 1), so the
    // second row comes when the timer reaches 3, two ticks later
    let d = module(&[0], 14187, &[(0, 0, 0, 428, 0x1F, 3)]);
    FIRST_TICK.with(|c| c.set(Some(100_000)));
    let (log, _) = play("mms.test", d, 20);
    FIRST_TICK.with(|c| c.set(None));
    let spacing = 14188u64 * 5;
    let rows: Vec<u64> = log.iter().filter(|w| w.0 == 0xDFF0A8 && w.2 >= 100_000).map(|w| (w.2 - 100_000) / spacing).collect();
    assert_eq!(&rows[..4], &[5, 8, 11, 14]);
}

#[test]
fn song_end_and_wrap() {
    let d = module(&[0], 14187, &[]);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("mms.test"), d).unwrap();
    let mut out = vec![0f32; 2 * 960];
    let mut end = None;
    for t in 0..(6 * 64 + 10) {
        e.render(&mut out);
        if end.is_none() && e.songend() {
            end = Some(t);
        }
    }
    // 64 rows of 6 ticks: the 64th row is played on tick 6 * 64
    let t = end.expect("song end");
    assert!((380..390).contains(&t), "{t}");
}

#[test]
fn subsongs_split_at_empty_patterns() {
    // positions: 0 music, 1 empty, 2 music, 3 empty-or-stopped, 4 music
    let mut d = module(
        &[0, 1, 2, 3, 4],
        14187,
        &[(0, 0, 0, 428, 0x10, 0), (2, 0, 0, 428, 0x10, 0), (3, 0, 0, 0xFFFE, 0, 0), (4, 0, 0, 428, 0x10, 0)],
    );
    // pattern 1 is entirely empty, pattern 3 holds a stop only: both separate
    let l = layout(&d).unwrap();
    assert_eq!(subsongs(&d, &l, 14187), [(0, 1), (2, 3), (4, 5)]);
    // a low tempo, or one position, does not split
    assert_eq!(subsongs(&d, &l, 0x1BB6), [(0, 5)]);
    d[0x432] = 1;
    let l = layout(&d).unwrap();
    assert_eq!(subsongs(&d, &l, 14187), [(0, 1)]);
}

#[test]
fn split_abort_leaves_a_subsong_without_an_end() {
    // 0 music, 1 empty, 2 music, 3 empty, 4 empty (aborts), 5 music
    let d = module(&[0, 1, 2, 3, 4, 5], 14187, &[(0, 0, 0, 428, 0, 0), (2, 0, 0, 428, 0, 0), (5, 0, 0, 428, 0, 0)]);
    let l = layout(&d).unwrap();
    assert_eq!(subsongs(&d, &l, 14187), [(0, 6), (2, 0)]);
}

#[test]
fn subsong_change_restarts() {
    let d = module(&[0, 1, 2], 14187, &[(0, 0, 0, 428, 0x10, 0), (2, 0, 0, 428, 0x10, 0)]);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("mms.test"), d).unwrap();
    let p = e.playing.as_ref().unwrap();
    assert_eq!((p.sub_min, p.sub_max), (0, 1));
    e.change_subsong(1).unwrap();
    assert_eq!(e.playing.as_ref().unwrap().sub, 1);
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..12 {
        e.render(&mut out);
    }
}

/// A subsong an aborted split leaves without an end runs on past the order list (here into the
/// patterns and beyond the module): it plays on without a crash or a song end.
#[test]
fn subsong_without_end_runs_on() {
    let rows = [(0, 0, 0, 428, 0x10, 0), (2, 0, 0, 428, 0x10, 0), (5, 0, 0, 428, 0x10, 0)];
    let d = module(&[0, 1, 2, 3, 4, 5], 14187, &rows);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.play_data(Path::new("mms.test"), d).unwrap();
    assert_eq!(e.playing.as_ref().unwrap().sub_max, 1);
    e.change_subsong(1).unwrap();
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..(6 * 64 * 10) {
        e.render(&mut out);
    }
    assert!(!e.songend());
}

/// The arpeggio, pitch bend and step commands write the periods the specification gives.
#[test]
fn effect_periods() {
    // voice 0: arpeggio 0x47 on period 428 (table index 16): +4 notes at timer 1 and 5, +7 at
    // timers 2 and 4, the period itself at 3; voice 1: pitch bend up 3; voice 2: pitch bend down 2
    let rows = [(0, 0, 0, 428, 0x11, 0x47), (0, 0, 1, 380, 0x12, 0x30), (0, 0, 2, 380, 0x12, 0x02)];
    let d = module(&[0], 14187, &rows);
    let (log, _) = play("mms.fx", d, 13);
    let p0 = reg(&log, 0xDFF0A6);
    // the note, then timers 1..5: table[16+4]=339, table[16+7]=285, 428, 285, 339
    assert_eq!(&p0[..6], &[428, 339, 285, 428, 285, 339]);
    let p1 = reg(&log, 0xDFF0B6);
    assert_eq!(&p1[..6], &[380, 383, 386, 389, 392, 395]);
    let p2 = reg(&log, 0xDFF0C6);
    assert_eq!(&p2[..6], &[380, 378, 376, 374, 372, 370]);
}

/// The same module twice gives the same writes.
#[test]
fn deterministic() {
    let rows = [(0, 0, 0, 428, 0x11, 0x47), (0, 1, 1, 0x1000 | 380, 0x2f, 3), (0, 2, 2, 0xFFFE, 0, 0), (0, 3, 3, 0xFFFC, 0, 0)];
    let d = module(&[0, 0], 14187, &rows);
    let a = play("mms.t", d.clone(), 40).0;
    let b = play("mms.t", d, 40).0;
    assert!(!a.is_empty());
    assert_eq!(a, b);
}

#[test]
fn effects_without_panics() {
    // every command and special word on every voice, over many ticks
    let mut rows = vec![];
    for r in 0..16usize {
        for v in 0..4usize {
            let w = [428u16, 0xFFFD, 0xFFFB, 0xFFFC, 0xFFFE, 0xFFFF, 0, 0x1000 | 300][(r + v) % 8];
            rows.push((0, r, v, w, ((r as u8 & 3) << 4) | (r + v) as u8 & 15, (17 * (r + v)) as u8));
        }
    }
    let d = module(&[0], 8000, &rows);
    let (log, _) = play("mms.fx", d, 300);
    assert!(log.len() > 100);
}

/// Reference-run dump: `MMS_MODULE=file MMS_TICKS=n MMS_LOG=out [MMS_SUB=k] [MMS_E0=clock]
/// cargo test --release mms_dump -- --ignored --nocapture`: the module played as `n` ticks of
/// 960 samples and the register log in DELI_WRITE_LOG's format; `MMS_E0` sets the absolute time
/// of the first tick (of the last change of subsong, for `MMS_SUB`).
#[test]
#[ignore]
fn mms_dump() {
    let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("MMS_MODULE"), std::env::var("MMS_TICKS"), std::env::var("MMS_LOG")) else {
        return;
    };
    let e0 = std::env::var("MMS_E0").ok().and_then(|s| s.parse::<u64>().ok());
    let sub: u32 = std::env::var("MMS_SUB").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    FIRST_TICK.with(|c| c.set(if sub == 0 { e0 } else { None }));
    let name = Path::new(&file).file_name().unwrap().to_string_lossy().to_string();
    e.play_data(Path::new(&name), std::fs::read(&file).unwrap()).unwrap();
    for k in 0..sub {
        FIRST_TICK.with(|c| c.set(if k + 1 == sub { e0 } else { None }));
        e.change_subsong(1).unwrap();
    }
    println!("format {}", e.playing.as_ref().unwrap().format);
    let mut buf = vec![0f32; 2 * 960];
    let mut end = None;
    for t in 0..ticks.parse::<usize>().unwrap() {
        e.render(&mut buf);
        if end.is_none() && e.songend() {
            end = Some(t);
        }
    }
    println!("songend tick {:?}", end);
    let l = e.m.bus.write_log.take().unwrap();
    let head = e.m.deli.list.first().map(|(a, n)| format!("# module {a:06x} {n}\n")).unwrap_or_default();
    let mut text = head;
    text += &l.iter().map(|(a, v, t)| format!("{a:06x} {v:04x} {t}\n")).collect::<String>();
    std::fs::write(out, text).unwrap();
}
