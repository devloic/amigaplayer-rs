use super::*;
use m68k::CpuType;

/// A small module: one effect sequence (start sample 0 with DMA off, hold), one instrument
/// (volume sequence: 0x40 then hold), one pattern of `ps / 2` rows with `rows` as (row, voice
/// column is the same for all four voices) note bytes, `npos` positions of pattern 0, speed 2.
fn module(rows: &[(usize, u8, u8)], ps: u16, npos: u16, restart: u16) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"MCMD");
    for w in [1u16, 1, 1, npos, ps, 1, 1] {
        d.extend_from_slice(&w.to_be_bytes());
    }
    // effect sequence 0
    let mut e = [0u8; 64];
    e[..3].copy_from_slice(&[0xE2, 0, 0xE1]);
    d.extend_from_slice(&e);
    // instrument 0: speed 1, effect 0, no vibrato, volume sequence 0x40 then hold
    let mut i = [0u8; 64];
    i[0] = 1;
    i[5] = 0x40;
    i[6] = 0xE1;
    d.extend_from_slice(&i);
    // pattern 0
    let mut p = vec![0u8; ps as usize];
    for &(r, n, b1) in rows {
        p[2 * r] = n;
        p[2 * r + 1] = b1;
    }
    d.extend_from_slice(&p);
    // positions: pattern 0 on all voices
    for _ in 0..npos {
        d.extend_from_slice(&[0u8; 12]);
    }
    // subsong: first 0, last npos - 1, speed 2, restart
    for w in [0u16, npos - 1, 2, restart] {
        d.extend_from_slice(&w.to_be_bytes());
    }
    // sample record: offset 0, 4 words, loop start 2, loop length 2
    let mut s = [0u8; 28];
    s[22..24].copy_from_slice(&4u16.to_be_bytes());
    s[24..26].copy_from_slice(&2u16.to_be_bytes());
    s[26..28].copy_from_slice(&2u16.to_be_bytes());
    d.extend_from_slice(&s);
    d.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    d
}

const T0: u64 = 1_000_000;

fn player(d: &[u8]) -> Mcmd {
    build(d, 0x20000, 0)
}

fn run(r: &mut Mcmd, ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    for k in 0..ticks {
        m.bus.clock = T0 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    m.bus.write_log.take().unwrap()
}

#[test]
fn detection() {
    let d = module(&[], 4, 1, 0);
    assert!(detect(&d));
    assert!(!detect(&d[..10]));
    let mut bad = d.clone();
    bad[0] = b'X';
    assert!(!detect(&bad));
}

#[test]
fn section_lengths_wrap_as_signed_16_bit() {
    assert_eq!(sect(2, 64), 128);
    assert_eq!(sect(512, 64), -32768);
    assert_eq!(sect(1024, 64), 0);
}

#[test]
fn layout_and_start_state() {
    let d = module(&[(0, 0x20, 0)], 4, 2, 0);
    let r = player(&d);
    assert_eq!(r.eff_base, 18);
    assert_eq!(r.ins_base, 18 + 64);
    assert_eq!(r.pat_base, 18 + 128);
    assert_eq!(r.v[0].n, 75);
    assert_eq!(r.v[2].n, 12);
    assert_eq!(r.v[2].ll, 1);
    assert_eq!(r.speed, 2);
    assert_eq!(r.v[0].pos_off, 12);
}

#[test]
fn first_tick_write_order() {
    let d = module(&[(0, 0x20, 0)], 4, 2, 0);
    let mut r = player(&d);
    let w = run(&mut r, 1);
    // four DMA-off writes of the row step come first (every voice has no sample yet)
    let regs: Vec<u32> = w.iter().map(|e| e.0).collect();
    assert_eq!(&regs[..4], &[0xDFF096; 4]);
    assert_eq!(w[0].1, 1);
    assert_eq!(w[3].1, 8);
    // each voice: DMA off, LC (two words), LEN, PER, VOL
    assert_eq!(&regs[4..11], &[0xDFF096, 0xDFF0A0, 0xDFF0A2, 0xDFF0A4, 0xDFF0A6, 0xDFF0A8, 0xDFF096]);
    assert_eq!(w[4].1, 1);
    assert_eq!(w[5].1, 2);
}

#[test]
fn dma_phase_restarts_with_loop_values() {
    let d = module(&[(0, 0x20, 0)], 4, 2, 0);
    let mut r = player(&d);
    let w = run(&mut r, 1);
    // the DMA-on write with all four channels, then LC/LEN of each channel (loop start + 4 bytes)
    let on = w.iter().position(|e| e.0 == 0xDFF096 && e.1 == 0x800F).unwrap();
    let tail = &w[on + 1..];
    assert_eq!(tail.len(), 12);
    assert_eq!(tail[0].0, 0xDFF0A0);
    let want = 0x20000 + r.smp_base as u32 + 2;
    assert_eq!(((tail[0].1 as u32) << 16) | tail[1].1 as u32, want);
    assert_eq!(tail[2].1, 2);
}

#[test]
fn volume_written_after_period() {
    let d = module(&[(0, 0x20, 0)], 4, 2, 0);
    let mut r = player(&d);
    let w = run(&mut r, 1);
    let vols: Vec<u16> = w.iter().filter(|e| e.0 == 0xDFF0A8).map(|e| e.1).collect();
    assert_eq!(vols, vec![0x40]);
}

#[test]
fn wrap_reports_song_end_and_continues() {
    let d = module(&[], 2, 1, 0);
    let mut r = player(&d);
    // one row per pattern, speed 2: the wrap comes on the second row tick
    assert!(!r.songend());
    run(&mut r, 8);
    assert!(r.songend());
}

#[test]
fn deterministic() {
    let d = module(&[(0, 0x20, 0), (1, 0x84, 0x40)], 8, 2, 0);
    let a = run(&mut player(&d), 40);
    let b = run(&mut player(&d), 40);
    assert_eq!(a, b);
    assert!(!a.is_empty());
}

#[test]
fn stop_control_byte_silences() {
    let mut d = module(&[], 4, 2, 0);
    // positions start at 150; position 1, voice 0: control byte 0x80 (stop)
    d[150 + 12 + 2] = 0x80;
    let mut r = player(&d);
    run(&mut r, 20);
    assert!(r.stop);
    // in the stop state a tick writes DMACON 0x000F then the four volumes
    let w = run(&mut r, 1);
    assert_eq!(w.len(), 5);
    assert_eq!((w[0].0, w[0].1), (0xDFF096, 0x000F));
}

#[test]
fn malformed_input_never_panics() {
    let mut seed = 0x1234_5678u32;
    for n in 0..50 {
        let len = 18 + (n * 37) % 600;
        let mut d = vec![0u8; len];
        d[..4].copy_from_slice(b"MCMD");
        for b in d[4..].iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *b = (seed >> 24) as u8;
        }
        let mut r = player(&d);
        run(&mut r, 60);
    }
}
