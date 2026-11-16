use super::*;
use m68k::CpuType;

/// A NESA3 file (no header blocks) of records `(delay, [(register, value)])`.
fn file(records: &[(u8, &[(u8, u8)])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (d, pairs) in records {
        body.push(*d);
        for (i, (r, v)) in pairs.iter().enumerate() {
            body.push(*v);
            body.push(r | if i + 1 < pairs.len() { 0x20 } else { 0 });
        }
    }
    let mut f = b"NESA\x23\x0f\x00\x00\x00\x00\x00\x00".to_vec();
    f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    f.extend_from_slice(&body);
    f.extend_from_slice(&[0xFF; 3]);
    f
}

fn seq(d: &[u8]) -> Seq {
    Seq::new(d)
}

/// (flags, waveform, period, volume) of each voice after each tick.
fn run(d: &[u8], ticks: usize) -> Vec<[(u8, Option<u8>, u32, u16); 4]> {
    let mut s = seq(d);
    (0..ticks)
        .map(|_| {
            s.tick();
            s.v.map(|v| (v.flags, v.wave, v.per, v.vol))
        })
        .collect()
}

#[test]
fn detection() {
    let f = file(&[(2, &[(0, 0)])]);
    assert!(detect(&f));
    let mut g = f.clone();
    g[4] = 0x22;
    assert!(!detect(&g), "NESA2 is not taken");
    assert!(!detect(b"NESB\x23\x0f\x60\x00\x00\x00\x00\x00\x00\x00\x00\x00"));
    assert!(!detect(b"NESA\x23"));
}

#[test]
fn header_blocks_move_the_data() {
    // byte 6 bits 4-6: two 32-byte blocks before the data, as in the rips
    let f = file(&[(1, &[(0x0C, 0x0F), (0x0F, 0x08)])]);
    let mut g = f[..16].to_vec();
    g[6] = 0x60;
    g.extend_from_slice(&[0xFE; 64]);
    g.extend_from_slice(&f[16..]);
    assert_eq!(run(&g, 1), run(&f, 1));
    assert_eq!(run(&g, 1)[0][3], (0x1C, Some(W_NOISE), 0, 60));
}

#[test]
fn first_record_after_its_delay_and_song_end() {
    // first delay 2: tick 0 no change, tick 1 the record; the end one tick after the last record
    let f = file(&[(2, &[(0x00, 0x8F), (0x02, 0x00), (0x03, 0x01)]), (3, &[(0x02, 0x10)])]);
    let t = run(&f, 12);
    assert_eq!(t[0], [(0, None, 0, 0); 4]);
    assert_eq!(t[1][0], (0x1C, Some(2), 4 * 0x100, 38));
    assert_eq!(t[4][0], (0x1C, Some(2), 4 * 0x110, 38));
    // the song's end in tick 5: all four volume 0; then the first record again 2 ticks later
    assert!(t[5].iter().all(|v| v.0 == F_VOL && v.3 == 0));
    assert_eq!(t[7][0], (0x1C, Some(2), 4 * 0x100, 38));
    let mut s = seq(&f);
    let ends: Vec<usize> = (0..20).filter(|_| s.tick()).collect();
    assert_eq!(ends.len(), 3);
}

#[test]
fn pulse_length_and_fade() {
    // $4003 value >> 3 = 3: LENP 2, runs out after one tick, then the fade 37, 36, ...
    let f = file(&[(2, &[(0x00, 0x0F), (0x02, 0x40), (0x03, 0x18)]), (100, &[(0x10, 0)])]);
    let t = run(&f, 6);
    assert_eq!(t[1][0], (0x1C, Some(0), 4 * 0x40, 38));
    assert_eq!(t[2][0], (F_VOL, Some(0), 4 * 0x40, 37));
    assert_eq!(t[3][0], (F_VOL, Some(0), 4 * 0x40, 36));
    // halt (bit 5): no fade
    let f = file(&[(2, &[(0x04, 0x2F), (0x06, 0x40), (0x07, 0x18)]), (100, &[(0x10, 0)])]);
    let t = run(&f, 6);
    assert_eq!(t[1][1], (0x1C, Some(0), 4 * 0x40, 38));
    assert_eq!(t[3][1].0, 0);
    // entry 1 of LENP is 2 for the pulses
    let f = file(&[(2, &[(0x00, 0x0F), (0x02, 0x40), (0x03, 0x08)]), (100, &[(0x10, 0)])]);
    assert_eq!(run(&f, 3)[2][0].3, 37);
}

#[test]
fn triangle_and_noise() {
    // $400B: level 46; LEN[1] = 254 for the triangle
    let f = file(&[(2, &[(0x08, 0x80), (0x0A, 0xFB), (0x0B, 0x09)]), (100, &[(0x10, 0)])]);
    let t = run(&f, 4);
    assert_eq!(t[1][2], (0x1C, Some(W_TRIANGLE), 4 * 0x1FB, 46));
    assert_eq!(t[2][2].0, 0);
    // linear value 4: volume 0 after 1 tick, then 45, 44 ...
    let f = file(&[(2, &[(0x08, 0x04), (0x0A, 0xFB), (0x0B, 0x09)]), (100, &[(0x10, 0)])]);
    let t = run(&f, 5);
    assert_eq!(t[2][2], (F_VOL, Some(W_TRIANGLE), 4 * 0x1FB, 0));
    assert_eq!(t[3][2].3, 45);
    assert_eq!(t[4][2].3, 44);
    // noise: period 500 after a $400E write, volume 4 n; the length runs out (LEN[0] = 10: 5 ticks)
    let f = file(&[(2, &[(0x0C, 0x0B), (0x0E, 0x03), (0x0F, 0x00)]), (100, &[(0x10, 0)])]);
    let t = run(&f, 8);
    assert_eq!(t[1][3], (0x1C, Some(W_NOISE), 500, 44));
    assert_eq!(t[6][3], (F_VOL, Some(W_NOISE), 500, 0));
    assert_eq!(t[5][3].0, 0);
}

#[test]
fn genie_period() {
    for (p, w) in [(124, 123), (500, 499), (1024, 1023), (2028, 2027), (8188, 8191), (120, 122), (0, 0)] {
        assert_eq!(written_period(p), w, "{p}");
    }
}

#[test]
fn waveforms() {
    assert_eq!(NOISE[..4], [0xCC, 0x99, 0x32, 0x63]);
    assert_eq!(NOISE[508..], [0xC5, 0x35, 0x6A, 0x00]);
    assert_eq!(DUTY[3], [0x7F, 0x7F, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80]);
}

#[test]
fn genie_writes() {
    let f = file(&[(2, &[(0x0C, 0x0F), (0x0E, 0x00), (0x0F, 0x08)]), (100, &[(0x10, 0)])]);
    let mut m = Machine::new(CpuType::M68000);
    m.bus.clock = 1000;
    let mut r = new(&f, 0, &mut m).unwrap();
    assert_eq!(r.subsong_tick_at(), Some(1000 + T_FIRST_TICK));
    assert_eq!(r.subsong_tick_at(), None);
    for _ in 0..2 {
        m.bus.write_log = Some(Vec::new());
        r.tick(&mut Hw { bus: &mut m.bus });
    }
    // tick 1: the noise's period, then its volume 142 cc later
    let log = m.bus.write_log.take().unwrap();
    let per = log.iter().find(|w| w.0 == 0xDFF0D6).expect("period");
    let vol = log.iter().find(|w| w.0 == 0xDFF0D8).expect("volume");
    assert_eq!((per.1, vol.1, vol.2 - per.2), (499, 60, 142));
}

#[test]
fn genie_buffer_fill() {
    let f = file(&[(2, &[(0x0C, 0x0F), (0x0E, 0x00), (0x0F, 0x08)]), (100, &[(0x10, 0)])]);
    let mut m = Machine::new(CpuType::M68000);
    let buffers = m.alloc((8 * BUF) as u32);
    let mut r = Nesa {
        seq: Seq::new(&f),
        ch: [Chan { next: 1, ..Chan::default() }; 4],
        buffers,
        first_tick: None,
        ticks: 0,
        first_record: true,
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    // Paula's request of channel 3, then its handler
    let irq = |r: &mut Nesa, hw: &mut Hw| {
        hw.bus.write_word(0xDFF09C, 0x8400);
        r.audio_irq_ch(3, hw);
        assert_eq!(hw.bus.paula.intreq & 0x400, 0, "the handler clears its request");
    };
    hw.bus.write_bytes(r.buffer(3, 1), &[1; BUF]);
    // before the first waveform: zeros
    irq(&mut r, &mut hw);
    assert!((0..BUF as u32).all(|i| hw.bus.r8(r.buffer(3, 1) + i) == 0));
    // a request already cleared (the start's INTREQ write): nothing is filled
    hw.bus.write_bytes(r.buffer(3, 0), &[1; BUF]);
    r.audio_irq_ch(3, &mut hw);
    assert_eq!(hw.bus.r8(r.buffer(3, 0)), 1);
    r.tick(&mut hw);
    r.tick(&mut hw);
    irq(&mut r, &mut hw);
    irq(&mut r, &mut hw);
    assert!((0..BUF).all(|i| hw.bus.r8(r.buffer(3, 0) + i as u32) == NOISE[i]));
    assert!((0..BUF).all(|i| hw.bus.r8(r.buffer(3, 1) + i as u32) == NOISE[BUF + i]));
}
