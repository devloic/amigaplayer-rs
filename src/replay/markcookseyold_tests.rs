use super::*;
use m68k::CpuType;

const BASE: u32 = 0x20000;
const T0: u64 = 1_000_000;

fn p16(d: &mut Vec<u8>, x: u16) {
    d.extend_from_slice(&x.to_be_bytes());
}

fn p32(d: &mut Vec<u8>, x: u32) {
    d.extend_from_slice(&x.to_be_bytes());
}

/// A one-subsong module: four voice tracks, patterns (called by number), and one instrument
/// (a record of 4 words, 8 sample bytes). Offsets are relative to the data after the 28-byte header.
fn module(voices: [&[u8]; 4], patterns: &[&[u8]]) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    // header: pattern table offset, instrument table offset (patched), subsong table
    p32(&mut body, 0);
    p32(&mut body, 0);
    // one subsong: the first track begins after the table (8 + 16)
    let mut at = 24u32;
    let mut starts = [0u32; 4];
    for (i, v) in voices.iter().enumerate() {
        starts[i] = at;
        at += v.len() as u32;
    }
    for s in starts {
        p32(&mut body, s);
    }
    for v in voices {
        body.extend_from_slice(v);
    }
    if body.len() % 2 == 1 {
        body.push(0);
    }
    let mut pat_at = Vec::new();
    for p in patterns {
        pat_at.push(body.len() as u32);
        body.extend_from_slice(p);
        if body.len() % 2 == 1 {
            body.push(0);
        }
    }
    let pt = body.len() as u32;
    for a in &pat_at {
        p16(&mut body, (*a as i64 - pt as i64) as u16);
    }
    if pat_at.is_empty() {
        p16(&mut body, 0);
    }
    let it = body.len() as u32;
    p32(&mut body, 4);
    // record: 4 words, then 8 sample bytes
    p16(&mut body, 4);
    body.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    body[0..4].copy_from_slice(&pt.to_be_bytes());
    body[4..8].copy_from_slice(&it.to_be_bytes());
    let mut d = vec![0x60, 0x1A];
    let total = (body.len() + 28) as u32;
    d.extend_from_slice(&(total - 28).to_be_bytes());
    d.resize(28, 0);
    d.extend_from_slice(&body);
    // the size field agrees with the length in its upper 24 bits (real files: size - 28)
    let len = d.len() as u32;
    d[2..6].copy_from_slice(&(len - 28).to_be_bytes());
    d
}

struct Rig {
    m: Machine,
    r: Box<dyn Replayer>,
    k: u64,
    next: u64,
}

fn rig(d: &[u8]) -> Rig {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.clock = T0;
    m.bus.write_log = Some(Vec::new());
    let mut r = new(d, BASE, &mut m).expect("recognised");
    let next = r.subsong_tick_at().unwrap();
    Rig { m, r, k: 0, next }
}

impl Rig {
    fn tick(&mut self) {
        self.m.bus.clock = self.next + TICK * self.k;
        self.k += 1;
        let mut hw = Hw { bus: &mut self.m.bus };
        self.r.tick(&mut hw);
    }
    fn run(&mut self, n: usize) -> Vec<(u32, u16, u64)> {
        for _ in 0..n {
            self.tick();
        }
        self.m.bus.write_log.take().unwrap()
    }
}

/// Pads a module so that its length passes the size-field test (the long at 2 and the length
/// agree in the upper 24 bits).
fn fit(mut d: Vec<u8>) -> Vec<u8> {
    loop {
        let len = d.len() as u32;
        let size = len - 28;
        if (size & 0xFFFF_FF00) == (len & 0xFFFF_FF00) && size & 0xFF < 0xFF {
            d[2..6].copy_from_slice(&size.to_be_bytes());
            return d;
        }
        d.push(0);
        let len = d.len() as u32;
        d[2..6].copy_from_slice(&(len - 28).to_be_bytes());
    }
}

fn mk(voices: [&[u8]; 4], patterns: &[&[u8]]) -> Vec<u8> {
    fit(module(voices, patterns))
}

const STOP: &[u8] = &[0x18];
/// A voice that rests for a long time and writes nothing.
const FILL: &[u8] = &[0x10, 0x7F];

#[test]
fn detection() {
    let d = mk([&[0x08, 0, 0x00, 0, 5, 0x18], STOP, STOP, STOP], &[]);
    assert!(detect(&d));
    assert_eq!(header(&d), Some(1));
    let mut e = d.clone();
    e[0] = 0x61;
    assert!(!detect(&e), "first byte");
    let mut e = d.clone();
    e[32] = 1;
    assert!(!detect(&e), "byte 32");
    let mut e = d.clone();
    e[3] ^= 0xFF;
    e[4] ^= 0x01;
    assert!(!detect(&e), "size field");
    assert!(!detect(&d[..30]));
    assert!(!detect(&[]));
    // a subsong table that is not a multiple of 16
    let mut e = d.clone();
    e[28 + 11] = e[28 + 11].wrapping_add(4);
    assert!(!detect(&e));
}

#[test]
fn subsong_count_and_range() {
    let d = mk([STOP, STOP, STOP, STOP], &[]);
    let r = rig(&d);
    assert_eq!(r.r.subsongs(), (0, 0, 0));
    // three subsongs: a table of 48 bytes
    let mut body = Vec::new();
    p32(&mut body, 0);
    p32(&mut body, 0);
    for s in 0..3u32 {
        for _ in 0..4 {
            p32(&mut body, 8 + 48 + 4 * s);
        }
    }
    body.extend_from_slice(&[0x18; 16]);
    let mut d = vec![0x60, 0x1A, 0, 0, 0, 0];
    d.resize(28, 0);
    d.extend_from_slice(&body);
    let d = fit(d);
    assert_eq!(header(&d), Some(3));
}

#[test]
fn first_note_sequence() {
    // instrument 0, note (x = 4: one entry above the centre), duration 5, then stop
    let d = mk([&[0x08, 0, 0x00, 4, 5, 0x18], STOP, STOP, STOP], &[]);
    let mut r = rig(&d);
    let log = r.run(1);
    let w: Vec<(u32, u16)> = log.iter().map(|x| (x.0, x.1)).collect();
    // voice 0: LC high/low, LEN, VOL (initial 0x14), PER (index 4: 678 -> offset 4 is entry 4),
    // DMA on, silence LC, LEN 2
    assert_eq!(w[0].0, 0xDFF0A0);
    assert_eq!(w[1].0, 0xDFF0A2);
    assert_eq!(w[2], (0xDFF0A4, 4));
    assert_eq!(w[3], (0xDFF0A8, 0x14));
    assert_eq!(w[4], (0xDFF0A6, PERIODS[4]));
    assert_eq!(w[5], (0xDFF096, 0x8001));
    assert_eq!(w[6].0, 0xDFF0A0);
    assert_eq!(w[7].0, 0xDFF0A2);
    assert_eq!(w[8], (0xDFF0A4, 2));
    // the sample address is the record plus two
    let lc = ((w[0].1 as u32) << 16) | w[1].1 as u32;
    assert!(lc > BASE && lc < BASE + d.len() as u32);
    // timing: LC->LEN 6, LEN->VOL 10, VOL->PER 8, PER->DMACON 16 colour clocks (spec 5.3)
    let t: Vec<u64> = log.iter().map(|x| x.2).collect();
    assert_eq!(t[0], t[1]);
    let near = |a: u64, b: u64| a.abs_diff(b) <= 1;
    assert!(near(t[2] - t[1], 6), "{}", t[2] - t[1]);
    assert!(near(t[3] - t[2], 10), "{}", t[3] - t[2]);
    assert!(near(t[4] - t[3], 8), "{}", t[4] - t[3]);
    assert!(near(t[5] - t[4], 16), "{}", t[5] - t[4]);
    // the five-line wait lasts 1029..1167 colour clocks to the silence write
    assert!((1029..=1167).contains(&(t[6] - t[5])), "{}", t[6] - t[5]);
    assert_eq!(t[6], t[7]);
    assert!(near(t[8] - t[7], 8));
}

#[test]
fn first_tick_entry_time() {
    // a volume command first: its write falls a fixed time after the tick's entry
    let d = mk([&[0x0C, 0x10, 0x18], STOP, STOP, STOP], &[]);
    let mut r = rig(&d);
    let entry = r.next;
    let log = r.run(1);
    // prologue 247, visit 84, command 36 + 32 cycles
    assert_eq!(log[0], (0xDFF0A8, 0x40, entry + (247 + 84 + 36 + 32) / 2));
}

#[test]
fn running_channel_takes_the_long_path() {
    // two notes on voice 0: the second finds the channel's DMA on
    let d = mk([&[0x08, 0, 0x00, 4, 1, 0x00, 6, 1, 0x18], FILL, FILL, FILL], &[]);
    let mut r = rig(&d);
    let log = r.run(3);
    let w: Vec<(u32, u16)> = log.iter().map(|x| (x.0, x.1)).collect();
    let regs: Vec<u32> = w.iter().map(|x| x.0).collect();
    // first note: 9 writes; second: DMA off, PER 2, VOL 0, DAT 0, then the 9 of a note
    assert_eq!(&w[9..13], &[(0xDFF096, 0x0001), (0xDFF0A6, 2), (0xDFF0A8, 0), (0xDFF0AA, 0)]);
    assert_eq!(regs[13], 0xDFF0A0);
    // the ten-line wait: 2133..2333 colour clocks from VOL=0 to DAT
    let dt = log[12].2 - log[11].2;
    assert!((2133..=2333).contains(&dt), "{}", dt);
    assert_eq!(w[17], (0xDFF0A6, PERIODS[(CENTRE + 3) as usize]));
}

#[test]
fn volume_is_raw_and_command_04_writes_two_table_words() {
    let d = mk([&[0x0C, 0x50, 0x04, 6, 2, 0x18], STOP, STOP, STOP], &[]);
    let mut r = rig(&d);
    let log = r.run(1);
    assert_eq!((log[0].0, log[0].1), (0xDFF0A8, 0x140));
    // PER = entry 5, VOL = entry 6, same colour clock
    assert_eq!((log[1].0, log[1].1), (0xDFF0A6, PERIODS[5]));
    assert_eq!((log[2].0, log[2].1), (0xDFF0A8, PERIODS[6]));
    assert_eq!(log[1].2, log[2].2);
}

#[test]
fn song_end_when_every_counter_is_dead() {
    // stop on all voices ends the song in the tick that ran them
    let d = mk([STOP, STOP, STOP, STOP], &[]);
    let mut r = rig(&d);
    assert!(!r.r.songend());
    r.tick();
    assert!(r.r.songend());
    // a rest of 0x80 or more makes the voice dead at once; the loop-to-mark never ends a song
    let d = mk([&[0x10, 0x80], &[0x10, 0xFF], &[0x30, 0x10, 5, 0x34], STOP], &[]);
    let mut r = rig(&d);
    r.tick();
    r.tick();
    assert!(!r.r.songend());
    // duration 0: dead after two ticks
    let d = mk([&[0x08, 0, 0x00, 4, 0], STOP, STOP, STOP], &[]);
    let mut r = rig(&d);
    r.tick();
    r.tick();
    r.tick();
    assert!(r.r.songend());
}

#[test]
fn loops_and_calls() {
    // loop begin 3, note of duration 1, loop end: three notes in three ticks, then the stop
    let d = mk([&[0x08, 0, 0x24, 3, 0x00, 4, 1, 0x28, 0x18], STOP, STOP, STOP], &[]);
    let mut r = rig(&d);
    let log = r.run(8);
    assert_eq!(log.iter().filter(|x| x.0 == 0xDFF096 && x.1 == 0x8001).count(), 3);
    // a transposed call: the note's period is two entries up
    let pat: &[u8] = &[0x00, 4, 1, 0x20];
    let d = mk([&[0x08, 0, 0x2C, 4, 0, 0x18], STOP, STOP, STOP], &[pat]);
    let mut r = rig(&d);
    let log = r.run(4);
    let per: Vec<u16> = log.iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    assert_eq!(per, vec![PERIODS[(CENTRE + 4) as usize]]);
}

#[test]
fn period_index_outside_the_table_clamps() {
    let d = mk([&[0x08, 0, 0x2C, 0xC0, 0, 0x18], STOP, STOP, STOP], &[&[0x00, 0, 1, 0x00, 0x7E, 1, 0x20]]);
    let mut r = rig(&d);
    let log = r.run(6);
    let per: Vec<u16> = log.iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    // index -30 lies before the table: the first entry; index 33 is in the table
    assert_eq!(per[0], PERIODS[0]);
    assert!(per.iter().all(|p| *p == 2 || PERIODS.contains(p)));
}

#[test]
fn period_index_before_the_table_reads_the_first_entry() {
    // transposition -6 puts note 0 at index -1, -8 at index -2: the original reads its own code
    // words there, the port the table's first entry
    let d = mk([&[0x08, 0, 0x2C, 0xFA, 0, 0x2C, 0xF8, 0, 0x18], STOP, STOP, STOP], &[&[0x00, 0, 1, 0x20]]);
    let mut r = rig(&d);
    let log = r.run(12);
    let per: Vec<u16> = log.iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    // each note start is followed by the period write of the silence loop (2)
    assert_eq!((per[0], per[2]), (PERIODS[0], PERIODS[0]));
}

#[test]
fn subsong_change_before_the_first_tick_delays_it() {
    let d = mk([STOP, STOP, STOP, STOP], &[]);
    let mut r = rig(&d);
    let first = r.next;
    // a second subsong would delay the first tick by 621 colour clocks per call
    let mut hw = Hw { bus: &mut r.m.bus };
    r.r.set_subsong(0, &mut hw);
    assert_eq!(r.r.subsong_tick_at(), Some(first));
}

#[test]
fn garbage_does_not_panic_or_hang() {
    // every command byte in every voice, wild parameters
    for seed in 0..40u32 {
        let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
        let mut track = Vec::new();
        for _ in 0..200 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            track.push((x >> 24) as u8);
        }
        let v: [&[u8]; 4] = [&track[..50], &track[50..100], &track[100..150], &track[150..]];
        let d = mk(v, &[&track[10..60], &track[80..120]]);
        let mut r = rig(&d);
        let _ = r.run(300);
    }
    // random file contents: detection and construction never panic
    let mut x = 12345u32;
    let mut junk = vec![0x60u8; 300];
    for _ in 0..200 {
        for b in junk.iter_mut().skip(1) {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (x >> 24) as u8;
        }
        junk[32] = 0;
        junk[2..6].copy_from_slice(&272u32.to_be_bytes());
        junk[36..40].copy_from_slice(&40u32.to_be_bytes());
        for i in 1..8 {
            let v = (junk[36 + 4 * i + 3] as u32) % 200;
            junk[36 + 4 * i..40 + 4 * i].copy_from_slice(&v.to_be_bytes());
        }
        let _ = detect(&junk);
        let mut m = Machine::new(CpuType::M68000);
        if let Some(mut r) = new(&junk, BASE, &mut m) {
            for _ in 0..50 {
                let mut hw = Hw { bus: &mut m.bus };
                r.tick(&mut hw);
            }
        }
    }
}

#[test]
fn deterministic() {
    let d = mk([&[0x08, 0, 0x24, 3, 0x00, 4, 1, 0x28, 0x18], &[0x0C, 8, 0x00, 6, 2, 0x18], STOP, STOP], &[]);
    let a = rig(&d).run(20);
    let b = rig(&d).run(20);
    assert_eq!(a, b);
    assert!(!a.is_empty());
}
