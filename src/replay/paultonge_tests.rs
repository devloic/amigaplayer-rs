use super::*;
use crate::engine::Engine;
use std::path::Path;

fn p16(v: &mut [u8], o: usize, x: u16) {
    v[o..o + 2].copy_from_slice(&x.to_be_bytes());
}

fn p32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

const INSTR: usize = 0x30;
const PAT: usize = 20;

/// A song with voice 0 only: speed 2, instrument 0 (sample 0, volume 15, no envelope steps),
/// a pattern of `body` after a 0x80 pad byte, and a list that loops.
fn song(body: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; INSTR + 12];
    p16(&mut d, 0, 12);
    p16(&mut d, 8, INSTR as u16);
    d[10] = 2;
    // the list: the pattern, the terminator and the restart offset
    p16(&mut d, 12, PAT as u16);
    p16(&mut d, 14, 0);
    p16(&mut d, 16, 12);
    d[19] = 0x80;
    d[INSTR + 2] = 15;
    d.truncate(PAT);
    d.extend_from_slice(body);
    d.resize(INSTR, 0);
    d.resize(INSTR + 12, 0);
    d[INSTR + 2] = 15;
    d
}

/// A bank with one sample: attack part of 8 words at 0x40, loop part of 4 words at 0x60.
fn bank() -> Vec<u8> {
    let mut b = vec![0u8; 0x80];
    p32(&mut b, 0, 0x40);
    p16(&mut b, 4, 8);
    p32(&mut b, 6, 0x60);
    p16(&mut b, 10, 4);
    b[0x40..0x60].fill(0x10);
    b
}

fn play(name: &str, d: Vec<u8>, bank: Option<(&str, Vec<u8>)>, secs: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let dir = Path::new("/pt-test");
    let mut e = Engine::new(48000.0);
    if let Some((n, c)) = bank {
        e.m.os.mem_files.insert(dir.join(n), c);
    }
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    if e.play_data(&dir.join(name), d).is_err() {
        return (Vec::new(), false);
    }
    let mut out = vec![0f32; 2 * 48000 * secs];
    e.render(&mut out);
    let ended = e.songend();
    (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1, w.2)).collect(), ended)
}

/// Instrument 0, length 2 rows, note 5, then stop.
fn one_note() -> Vec<u8> {
    song(&[0xC0, 0xE1, 5, 0x8F])
}

#[test]
fn tables() {
    assert_eq!((0..15).map(volume).collect::<Vec<_>>(), [0, 2, 3, 4, 6, 8, 10, 13, 17, 22, 28, 35, 40, 51, 64]);
    assert_eq!(volume(16), 0x37);
    assert_eq!(volume(17), 0);
    assert_eq!((period_of(0), period_of(35)), (856, 113));
}

#[test]
fn detection() {
    let d = one_note();
    assert!(detect(&d));
    // each rule of 1.2
    let mut e = d.clone();
    p16(&mut e, 0, 14);
    assert!(!detect(&e), "first word");
    let mut e = d.clone();
    p16(&mut e, 2, 0x8000);
    assert!(!detect(&e), "bit 15");
    let mut e = d.clone();
    p16(&mut e, 4, 13);
    assert!(!detect(&e), "odd list offset");
    let mut e = d.clone();
    p16(&mut e, 2, 12);
    assert!(detect(&e), "a second voice on the same list");
    e[19] = 0x81;
    assert!(!detect(&e), "pattern end byte");
    let mut e = d.clone();
    p16(&mut e, 2, 12);
    p16(&mut e, 12, 0x8014);
    assert!(!detect(&e), "negative first pattern");
    let mut e = d.clone();
    p16(&mut e, 6, 0x7000);
    assert!(!detect(&e), "list past the end");
    let mut e = d.clone();
    p16(&mut e, 2, 12);
    e[19] = 0x8F;
    assert!(detect(&e), "0x8F ends a pattern too");
    assert!(!detect(&[]));
    assert!(!detect(&[0, 12, 0, 0]));
    assert!(!detect(&bank()));
}

#[test]
fn bank_names() {
    assert_eq!(bank_name("PAT.TUNE1"), "SMP.TUNE1");
    assert_eq!(bank_name("pat.title"), "SMP.title");
    assert_eq!(bank_name("song.mod"), "SMP.set");
    assert_eq!(bank_name("PAT"), "SMP.set");
}

#[test]
fn start_tick_and_a_note() {
    let (w, ended) = play("PAT.t", one_note(), Some(("SMP.set", bank())), 2);
    let regs: Vec<(u32, u16)> = w.iter().map(|x| (x.0, x.1)).collect();
    // tick 1: the empty accumulators, the start, the voice's first writes (index 16 of the curve)
    assert_eq!(
        &regs[..9],
        [
            (0xDFF096, 0),
            (0xDFF09C, 0),
            (0xDFF09A, 0),
            (0xDFF09A, 0x0780),
            (0xDFF09E, 0x00FF),
            (0xDFF0A6, 0),
            (0xDFF0A8, 0x37),
            (0xDFF0A8, 0x37),
            (0xDFF0A0, 0)
        ]
    );
    // the first write is 221 colour clocks after the call, the next two 18 and 12 later
    assert_eq!(w[1].2 - w[0].2, 18);
    assert_eq!(w[2].2 - w[1].2, 12);
    // the note start: LC (both words at one clock), LEN 8, INTENA clear, DMACON clear, INTREQ clear
    assert_eq!(w[8].2, w[9].2);
    assert_eq!(regs[10], (0xDFF0A4, 8));
    assert_eq!(&regs[11..14], [(0xDFF09A, 0x0080), (0xDFF096, 0x0001), (0xDFF09C, 0x0080)]);
    // the next tick: DMA and interrupt on for the voice, the note's period, the volume (index
    // 15 of the curve reads a driver word: 0x4A28 scaled to 0x228)
    assert_eq!(&regs[14..17], [(0xDFF096, 0x8201), (0xDFF09C, 0x0080), (0xDFF09A, 0x8080)]);
    assert_eq!(&regs[17..20], [(0xDFF0A6, period_of(5)), (0xDFF0A8, 0x228), (0xDFF0A8, 0x228)]);
    // the audio interrupt swaps the loop part in: LC, LEN 4, then INTREQ and INTENA clear
    let i = regs.iter().position(|x| *x == (0xDFF0A4, 4)).expect("loop LEN");
    assert_eq!(&regs[i + 1..i + 3], [(0xDFF09C, 0x0080), (0xDFF09A, 0x0080)]);
    // the stop command: DMACON 0x000F, song end
    assert!(regs.contains(&(0xDFF096, 0x000F)));
    assert!(ended);
}

#[test]
fn handler_timing() {
    // a handler call that serves channel 0 writes LC 117 colour clocks after the call and
    // returns 246 after it; the INTREQ and INTENA clears are 16 cycles (8 clocks) apart
    let (w, _) = play("PAT.t", one_note(), Some(("SMP.set", bank())), 2);
    let i = w.iter().position(|x| x.0 == 0xDFF0A4 && x.1 == 4).unwrap();
    assert_eq!(w[i - 1].2, w[i - 2].2, "LC words at one clock");
    assert_eq!(w[i].2 - w[i - 1].2, 10);
    assert_eq!(w[i + 2].2 - w[i + 1].2, 8);
}

#[test]
fn song_end_by_the_lists_and_restart() {
    // the list ends after the pattern: the song end is reported and the pattern plays again
    let d = song(&[0xC0, 0xE0, 5, 0x80]);
    let (w, ended) = play("PAT.t", d, Some(("SMP.set", bank())), 3);
    assert!(ended);
    assert!(w.iter().filter(|x| x.0 == 0xDFF0A4 && x.1 == 8).count() >= 2);
}

#[test]
fn volume_commands_and_release() {
    // volume 4, fade out is not needed: the instrument volume byte 0xB4 sets 4 (curve 6)
    let d = song(&[0xC0, 0xB4, 0xE1, 5, 0x8F]);
    let (w, _) = play("PAT.t", d, Some(("SMP.set", bank())), 2);
    assert!(w.iter().any(|x| x.0 == 0xDFF0A8 && x.1 == 6));
}

#[test]
fn companion_is_needed() {
    let (w, _) = play("PAT.t", one_note(), None, 1);
    assert!(w.is_empty());
    // SMP.t is preferred over SMP.set
    let mut b = bank();
    p16(&mut b, 4, 6);
    let dir = Path::new("/pt-test");
    let mut e = Engine::new(48000.0);
    e.m.os.mem_files.insert(dir.join("SMP.t"), b);
    e.m.os.mem_files.insert(dir.join("SMP.set"), bank());
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(&dir.join("PAT.t"), one_note()).unwrap();
    let mut out = vec![0f32; 2 * 48000];
    e.render(&mut out);
    let l = e.m.bus.write_log.take().unwrap();
    assert!(l.iter().any(|x| x.0 == 0xDFF0A4 && x.1 == 6));
    assert!(!l.iter().any(|x| x.0 == 0xDFF0A4 && x.1 == 8));
}

#[test]
fn deterministic() {
    let a = play("PAT.t", one_note(), Some(("SMP.set", bank())), 2);
    let b = play("PAT.t", one_note(), Some(("SMP.set", bank())), 2);
    assert_eq!(a, b);
}

#[test]
fn damaged_songs_do_not_panic_or_hang() {
    // undefined bytes, a pattern with no end, a list that runs off the file, wild offsets
    for body in [
        &[0xC0, 0x9E, 5][..],
        &[0xC3, 0xE0, 5],
        &[0x80, 0x80, 0x80],
        &[0x90, 0x7F, 0xFF],
        &[0xC0, 0xE0, 0x85, 1, 2, 3],
        &[0xC0, 0xA0, 0x98, 0x99],
        &[0xC0, 0x91, 0x8D, 0x8C, 0x96, 0x97, 0x8B, 0, 0x92, 1, 1],
        &[0xC0, 0xE0, 0x88, 1, 0, 1, 0x80, 3, 5, 0x8A, 1, 2, 3, 0x81, 5],
    ] {
        let d = song(body);
        let (_w, _e) = play("PAT.t", d, Some(("SMP.set", bank())), 2);
    }
    let mut d = one_note();
    for o in (0..d.len()).step_by(3) {
        d[o] ^= 0x5A;
        if detect(&d) {
            let _ = play("PAT.t", d.clone(), Some(("SMP.set", bank())), 1);
        }
    }
    // a bank too short for its entries
    let (_w, _e) = play("PAT.t", one_note(), Some(("SMP.set", vec![0u8; 12])), 1);
}
