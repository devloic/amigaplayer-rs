use super::*;
use crate::engine::Engine;
use std::path::Path;

/// A one-channel module: one vibrato table, one envelope table, one sample
/// (start 0, 16 words, loop at 16 bytes of 8 words), a stream of
/// sample 0, level 63, note 16 (period 339) for 4 ticks, a tie for 2, an end.
fn module() -> Vec<u8> {
    let mut d = Vec::new();
    for w in [1u16, 1, 1, 1] {
        d.extend_from_slice(&w.to_be_bytes());
    }
    let smp = 8 + 4 + 4 + 4;
    let end = smp + 12;
    let vib = end + 254;
    let env = end;
    let stream = vib + 256;
    d.extend_from_slice(&(stream as u32).to_be_bytes());
    d.extend_from_slice(&(vib as u32).to_be_bytes());
    d.extend_from_slice(&(env as u32).to_be_bytes());
    // sample entry
    d.extend_from_slice(&0u32.to_be_bytes());
    d.extend_from_slice(&16u16.to_be_bytes());
    d.extend_from_slice(&16u32.to_be_bytes());
    d.extend_from_slice(&8u16.to_be_bytes());
    // the 0x3F run (the envelope table: 127 words, so phases 0..253 give 63)
    d.extend_from_slice(&[0x3F; 254]);
    assert_eq!(d.len(), vib);
    d.extend_from_slice(&[0; 256]);
    // sample 0, level 63, note 16 for 5 ticks, tie for 5 ticks, end
    d.extend_from_slice(&[0x8F, 0, 0x8D, 63, 16, 0x3F, 0x7F, 0x3F, 0]);
    d
}

fn log_of(file: &str, d: Vec<u8>, ssd: Option<&str>, ticks: usize) -> Option<Vec<(u32, u16)>> {
    let dir = Path::new("/paulrobotham-test");
    let mut e = Engine::new(48000.0);
    if let Some(n) = ssd {
        e.m.os.mem_files.insert(dir.join(n), vec![0x40; 128]);
    }
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(&dir.join(file), d).ok()?;
    if e.playing.as_ref()?.format != "Paul Robotham" {
        return None;
    }
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..ticks {
        e.render(&mut out);
    }
    Some(e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect())
}

fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

#[test]
fn recognises_by_content() {
    let d = module();
    assert!(detect(&d));
    let mut e = d.clone();
    e[1] = 5; // channel count
    assert!(!detect(&e));
    let mut f = d.clone();
    f[1] = 0; // no channel
    assert!(!detect(&f));
    let mut e = d.clone();
    e[32 + 10] = 0; // inside the 0x3F run
    assert!(!detect(&e));
    assert!(!detect(&d[..30]));
    assert!(!detect(&[]));
}

#[test]
fn companion_names() {
    assert_eq!(companion("/x/song.dat"), ["song.SSD", "mdtest.ssd"]);
    assert_eq!(companion("dat.win"), ["mdtest.ssd"]);
}

#[test]
fn refused_without_its_companion() {
    assert!(log_of("dat.test", module(), None, 1).is_none());
    assert!(log_of("dat.test", module(), Some("mdtest.ssd"), 1).is_some());
}

#[test]
fn plays_a_note_with_a_trigger_and_a_restart() {
    let d = module();
    let log = log_of("dat.test", d, Some("mdtest.ssd"), 20).unwrap();
    // the note starts at tick 10: clear write, DMA off, address, length 16, period 339
    assert_eq!(reg(&log, 0xDFF0AA)[0], 0);
    assert_eq!(reg(&log, 0xDFF0A4)[..2], [16, 8]);
    let per = reg(&log, 0xDFF0A6);
    assert!(per.iter().filter(|&&p| p == 339).count() >= 3);
    // one DMA on, with the audio DMA bit
    assert!(reg(&log, 0xDFF096).contains(&0x8201));
    // the loop is installed by the restart: address 16 bytes after the first
    let lc = reg(&log, 0xDFF0A2);
    assert_eq!(lc[1] - lc[0], 16);
    // the envelope (all 63) times level 63 gives volume 63
    assert!(reg(&log, 0xDFF0A8).contains(&63));
}

#[test]
fn channel_end_stops_the_channel() {
    // channel 0 is the only one: no song end; a two-channel module ends when
    // channel 0 reaches the byte before channel 1's start.
    let d = module();
    let log = log_of("dat.test", d, Some("mdtest.ssd"), 60).unwrap();
    // the stream ends: DMA off, no more writes for the channel
    assert!(reg(&log, 0xDFF096).iter().filter(|&&v| v == 1).count() >= 2);
}
