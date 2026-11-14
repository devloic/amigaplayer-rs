//! Tests of the TFHD container and players on small modules built here (no module data is
//! kept in the repository): the container's parse and the players' detection rules, the
//! Pro player's song table, and short renders that show the differences the specification
//! gives for each engine (1.5: DMA on per voice at the macro's command; Pro: master volume
//! $3F and no 9-line waits; 7V: the plain engine, only the container is new).

use super::*;
use crate::engine::Engine;
use m68k::CpuType;

/// The song part of the old fixed layout: song 0 at track step 0 (speed 5), pattern 0 at $A00
/// on the first track, macro 0 at $B00 (the commands of `macro_`); `id` is its first bytes.
fn song(id: &[u8], macro_: &[u32]) -> Vec<u8> {
    let mut d = vec![0u8; 0xc00];
    let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_be_bytes());
    d[..id.len()].copy_from_slice(id);
    d[0x180..0x182].copy_from_slice(&5u16.to_be_bytes());
    put(&mut d, 0x400, 0xa00);
    put(&mut d, 0x404, 0xa20);
    put(&mut d, 0x600, 0xb00);
    put(&mut d, 0x7fc, 0xb00);
    for k in 1..8 {
        d[0x800 + 2 * k] = 0xff;
    }
    // pattern 0: note 30, macro 0, velocity 15, voice 0; wait 16; end
    put(&mut d, 0xa00, 0x1e00_f000);
    put(&mut d, 0xa04, 0xf310_0000);
    put(&mut d, 0xa08, 0xf000_0000);
    put(&mut d, 0xa20, 0xf000_0000);
    for (k, &c) in macro_.iter().enumerate() {
        put(&mut d, 0xb00 + 4 * k, c);
    }
    d
}

/// DMA off, SetBegin 0, SetLen 16, SetVol $30, AddNote 0, DMA on, Stop.
const MACRO: [u32; 7] = [0x0000_0000, 0x0200_0000, 0x0300_0010, 0x0e00_0030, 0x0800_0000, 0x0100_0000, 0x0700_0000];

/// A TFHD file: header size `h`, type byte `ty`, the song part and the samples.
fn tfhd(ty: u8, h: usize, song: &[u8], smpl: &[u8]) -> Vec<u8> {
    let mut d = b"TFHD".to_vec();
    d.extend_from_slice(&(h as u32).to_be_bytes());
    d.push(ty);
    d.push(0);
    d.extend_from_slice(&(song.len() as u32).to_be_bytes());
    d.extend_from_slice(&(smpl.len() as u32).to_be_bytes());
    d.resize(h, 0);
    d.extend_from_slice(song);
    d.extend_from_slice(smpl);
    d
}

/// The period of note 30.
const PERIOD_30: u16 = 0x12f;

fn square() -> Vec<u8> {
    (0..64).map(|i| if i < 32 { 0x40 } else { 0xc0 }).collect()
}

/// A song part of the 7-voice kind: the first song's first step is `$EFFE 0003`.
fn song_7v() -> Vec<u8> {
    let mut d = song(b"TFMX-SONG ", &MACRO);
    d[0x800..0x804].copy_from_slice(&[0xef, 0xfe, 0x00, 0x03]);
    d[0x804..0x808].copy_from_slice(&[0x00, 0x0c, 0x00, 0x00]);
    // the next step: the song's real first one
    d[0x810..0x814].copy_from_slice(&[0x00, 0x00, 0xff, 0x00]);
    d
}

/// Plays the file for 0.2 s with the open replayers; returns the player's format and the
/// writes.
fn play(data: Vec<u8>) -> Result<(String, Vec<(u32, u16, u64)>), String> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    let dir = std::env::temp_dir();
    e.play_data(&dir.join("amigaplayer-tfhd-test.tfhd"), data).map_err(|x| x.to_string())?;
    let format = e.playing.as_ref().ok_or("not playing")?.format.clone();
    let mut out = vec![0f32; 2 * 9600];
    e.render(&mut out);
    Ok((format, e.m.bus.write_log.take().unwrap()))
}

fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

#[test]
fn container_fields() {
    let s = song(b"TFMX \0\0\x01\0\0", &MACRO);
    let f = tfhd(0x81, 0x12, &s, &square());
    let p = parse(&f).unwrap();
    // bit 7 of the type is ignored
    assert_eq!(p, Head { ty: 1, h: 0x12, m: s.len(), s: 64 });
    // another header size moves both parts
    let f = tfhd(2, 0x14, &s, &square());
    assert_eq!(parse(&f).unwrap(), Head { ty: 2, h: 0x14, m: s.len(), s: 64 });
    // trailing bytes are ignored, a song part past the file's end is refused
    let mut f = tfhd(2, 0x12, &s, &square());
    f.extend_from_slice(&[0; 100]);
    assert!(parse(&f).is_some());
    assert!(parse(&f[..0x12 + s.len() - 1]).is_none());
    // wrong magic, too short, a song part too short for a song table
    assert!(parse(b"TFHE\0\0\0\x12\x01\0").is_none());
    assert!(parse(&f[..0x11]).is_none());
    assert!(parse(&tfhd(1, 0x12, &[0; 0x100], &[])).is_none());
}

#[test]
fn detection_by_type() {
    let s15 = song(b"TFMX \0\0\x01\0\0", &MACRO);
    let spro = song(b"TFMX-SONG ", &MACRO);
    let s7 = song_7v();
    // the player's own type is taken without a look at the song part
    let junk = vec![0u8; 0x400];
    assert!(detect_15(&tfhd(1, 0x12, &junk, &[])));
    assert!(detect_pro(&tfhd(2, 0x12, &junk, &[])));
    assert!(detect_7v(&tfhd(3, 0x12, &junk, &[])));
    // another type is refused (the bits above 7 aside), and so is an unknown one
    assert!(!detect_15(&tfhd(2, 0x12, &s15, &[])));
    assert!(!detect_pro(&tfhd(1, 0x12, &spro, &[])));
    assert!(!detect_7v(&tfhd(1, 0x12, &s7, &[])));
    assert!(!detect_pro(&tfhd(4, 0x12, &spro, &[])));
    assert!(detect_pro(&tfhd(0x82, 0x12, &junk, &[])));
    // type 0: the player's plain check, on the song part
    assert!(detect_15(&tfhd(0, 0x12, &s15, &[])));
    assert!(!detect_15(&tfhd(0, 0x12, &spro, &[])));
    assert!(!detect_15(&tfhd(0, 0x12, &junk, &[])));
    assert!(detect_pro(&tfhd(0, 0x12, &spro, &[])));
    // Pro takes any TFMX text, a 1.5 module among them
    assert!(detect_pro(&tfhd(0, 0x12, &s15, &[])));
    assert!(!detect_pro(&tfhd(0, 0x12, &junk, &[])));
    assert_eq!(detect_7v(&tfhd(0, 0x12, &s7, &[])), tfmx::detect_7v(&s7));
    // a bare module (no TFHD header) is not for these players
    assert!(!detect_15(&s15) && !detect_pro(&spro) && !detect_7v(&s7));
}

#[test]
fn pro_check_needs_tables_or_a_second_long() {
    // fixed tables ($1D0 = 0) and nothing at 4: refused (type 0 only)
    let mut s = song(b"TFMX-SONG ", &MACRO);
    s[4..8].fill(0);
    assert!(!detect_pro(&tfhd(0, 0x12, &s, &[])));
    assert!(detect_pro(&tfhd(2, 0x12, &s, &[])));
}

#[test]
fn malformed_input_does_not_panic() {
    let s = song(b"TFMX-SONG ", &MACRO);
    let full = tfhd(2, 0x12, &s, &square());
    for n in 0..full.len().min(0x240) {
        let _ = detect_15(&full[..n]) | detect_pro(&full[..n]) | detect_7v(&full[..n]);
    }
    // sizes that overflow, and headers that point outside the file
    let mut f = full.clone();
    f[10..14].copy_from_slice(&0xffff_ffffu32.to_be_bytes());
    assert!(!detect_pro(&f));
    let mut f = full.clone();
    f[4..8].copy_from_slice(&0xffff_fff0u32.to_be_bytes());
    assert!(!detect_pro(&f));
    // a song part of garbage plays or is refused, but does not hang or panic
    for ty in 1..=3 {
        let junk: Vec<u8> = (0..0x1000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let _ = play(tfhd(ty, 0x12, &junk, &junk[..64]));
        let _ = play(tfhd(ty, 0x12, &[0; 0x200], &[]));
    }
}

#[test]
fn song_table_is_compacted() {
    let mut m = Machine::new(CpuType::M68000);
    let a = m.alloc(0x400);
    // entries: 0: (0,0) tempo 3 (kept: empty song with a tempo), 1: (0,0) tempo 0 (dropped),
    // 2: (5,9) kept, 3,4: (7,7) twice (a run, dropped), 5: (8,8) kept, 6: (0,0) tempo 5
    // (dropped), 7: (4,4) kept, the rest (0,0) tempo 0 (dropped)
    let ent: [(u16, u16, u16); 8] = [(0, 0, 3), (0, 0, 0), (5, 9, 6), (7, 7, 6), (7, 7, 6), (8, 8, 6), (0, 0, 5), (4, 4, 1)];
    for (i, (s, l, t)) in ent.iter().enumerate() {
        m.bus.w16(a + 0x100 + 2 * i as u32, *s);
        m.bus.w16(a + 0x140 + 2 * i as u32, *l);
        m.bus.w16(a + 0x180 + 2 * i as u32, *t);
    }
    // entry 1 is a repeat of entry 0 only in start and last, and run members are dropped
    // whole: (0,0) with its run is dropped, so entry 0 goes too
    let n = compact_songs(&mut m.bus, a);
    let got: Vec<(u16, u16, u16)> =
        (0..n as u32).map(|i| (m.bus.r16(a + 0x100 + 2 * i), m.bus.r16(a + 0x140 + 2 * i), m.bus.r16(a + 0x180 + 2 * i))).collect();
    assert_eq!(got, vec![(5, 9, 6), (8, 8, 6), (4, 4, 1)]);
    // an empty table keeps nothing
    let b = m.alloc(0x400);
    assert_eq!(compact_songs(&mut m.bus, b), 0);
}

#[test]
fn v15_switches_the_voices_on_at_the_ticks_end() {
    // the Eagleplayer TFMX_1.5 plays a type-1 TFHD file as an mdat: no start writes of its
    // own, DMA on once at the end of the tick through ENPP_DMAMask (the mask's low bits and
    // the set bit), then the host's wait for the DMA
    let s = song(b"TFMX \0\0\x01\0\0", &MACRO);
    let (_, log) = play(tfhd(1, 0x12, &s, &square())).unwrap();
    assert!(reg(&log, 0xDFF09E).is_empty());
    let dmacon = reg(&log, 0xDFF096);
    assert!(dmacon.contains(&0x8001), "{dmacon:x?}");
    assert!(!dmacon.contains(&0x8201));
    let at = log.iter().position(|w| w.0 == 0xDFF096 && w.1 == 0x8001).unwrap();
    if let Some(next) = log.get(at + 1) {
        assert!(next.2 - log[at].2 >= 9 * crate::paula::LINE, "{} {}", next.2, log[at].2);
    }
}

#[test]
fn v15_tfhd_equals_the_plain_engine_in_values() {
    // the same song part played through TFHD gives the plain player's volumes, periods and
    // lengths
    let s = song(b"TFMX \0\0\x01\0\0", &MACRO);
    let (_, log) = play(tfhd(1, 0x12, &s, &square())).unwrap();
    assert!(reg(&log, 0xDFF0A4).contains(&16));
    assert!(reg(&log, 0xDFF0A6).contains(&PERIOD_30));
    assert!(reg(&log, 0xDFF0A8).contains(&(0x30 * 4 * 0x3f >> 8)));
}

#[test]
fn pro_master_volume_starts_at_3f_and_no_waits() {
    let s = song(b"TFMX-SONG ", &MACRO);
    let (_, log) = play(tfhd(2, 0x12, &s, &square())).unwrap();
    // the master volume is $3F: the volume $30 goes out as ($30 * 4 * $3F) >> 8
    let vol = reg(&log, 0xDFF0A8);
    assert!(vol.contains(&(((0x30u32 * 4 * 0x3f) >> 8) as u16)), "{vol:x?}");
    assert!(!vol.contains(&0x30));
    // DMA on at the end of the tick comes right after the periods (no 9-line wait)
    let per = log.iter().rposition(|w| w.0 == 0xDFF0A6 && w.1 == PERIOD_30).unwrap();
    let on = log.iter().position(|w| w.0 == 0xDFF096 && w.1 == 0x8201).unwrap();
    let next_per = log[..on].iter().rposition(|w| (0xDFF0A6..=0xDFF0D6).contains(&w.0)).unwrap_or(per);
    assert!(log[on].2 - log[next_per].2 < 8 * crate::paula::LINE, "{} {}", log[on].2, log[next_per].2);
    // the start: two groups of volumes, DMA off and the interrupts off, the interrupt
    // line on (INTENA $A000) and the modulation off
    assert_eq!(log[0].0, 0xDFF09A);
    assert!(reg(&log, 0xDFF09A).contains(&0xa000));
    assert!(reg(&log, 0xDFF09E).contains(&0x00ff));
}

#[test]
fn pro_has_no_track_mute_table() {
    // the plain Pro player mutes a track whose word at $1C0.. is not 0; the TFHD one does not
    let mut s = song(b"TFMX-SONG ", &MACRO);
    s[0x1c0..0x1c2].copy_from_slice(&0x00ffu16.to_be_bytes());
    let (_, log) = play(tfhd(2, 0x12, &s, &square())).unwrap();
    assert!(reg(&log, 0xDFF0A8).iter().any(|&v| v > 0x20), "track 0 plays");
}

#[test]
fn seven_voices_play_through_the_container() {
    let s = song_7v();
    let a = play(tfhd(3, 0x12, &s, &square())).unwrap();
    let b = play(tfhd(0, 0x12, &s, &square())).unwrap();
    assert_eq!(a.0, "TFMX 7V");
    assert!(!a.1.is_empty());
    // a type-0 file is taken by whichever player claims it first; it plays too
    assert!(!b.1.is_empty());
    // deterministic
    let c = play(tfhd(3, 0x12, &s, &square())).unwrap();
    assert_eq!(a.1, c.1);
}
