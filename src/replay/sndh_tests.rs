use super::mfp::Mfp;
use super::ym::Ym;
use super::*;
use crate::aiff::Stream;

// ------------------------------------------------------------------ synthetic tunes

/// A 68000 `move.b #imm,$ff88xx.l` (xx = 00 select, 02 data).
fn ym_set(code: &mut Vec<u8>, reg: u8, val: u8) {
    code.extend_from_slice(&[0x13, 0xFC, 0, reg, 0, 0xFF, 0x88, 0x00]);
    code.extend_from_slice(&[0x13, 0xFC, 0, val, 0, 0xFF, 0x88, 0x02]);
}

/// The header of an SNDH file: three `bra.w` entries (init, exit, play) after the tags, the tags
/// themselves, then `code_init` and `code_play` (the exit routine is an RTS).
fn sndh(tags: &[u8], code_init: &[u8], code_play: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 12];
    d.extend_from_slice(b"SNDH");
    d.extend_from_slice(tags);
    d.extend_from_slice(b"HDNS\0");
    if d.len() & 1 == 1 {
        d.push(0);
    }
    let hdr = d.len();
    let init = hdr;
    let exit = init + code_init.len();
    let play = exit + 2;
    for (k, target) in [init, exit, play].into_iter().enumerate() {
        d[4 * k] = 0x60;
        d[4 * k + 2..4 * k + 4].copy_from_slice(&((target - (4 * k + 2)) as u16).to_be_bytes());
    }
    d.extend_from_slice(code_init);
    d.extend_from_slice(&[0x4E, 0x75]);
    d.extend_from_slice(code_play);
    d
}

/// Init stores d0 (the subsong) at $1000; play writes it to YM register 0 and R1 = 7.
fn two_song_tune() -> Vec<u8> {
    let mut tags = Vec::new();
    tags.extend_from_slice(b"TITLTest tune\0COMMA Composer\0YEAR1990\0##02\0TC50\0TIME");
    // the words of TIME are on an even file offset (the tags start at 16)
    if (16 + tags.len()) & 1 == 1 {
        tags.push(0);
    }
    tags.extend_from_slice(&[0, 3, 0, 4]);
    let init = [0x11, 0xC0, 0x10, 0x00, 0x4E, 0x75];
    let mut play = Vec::new();
    // move.b #0,$ff8800 ; move.b $1000.w,d0 ; move.b d0,$ff8802
    play.extend_from_slice(&[0x13, 0xFC, 0, 0, 0, 0xFF, 0x88, 0x00, 0x10, 0x38, 0x10, 0x00, 0x13, 0xC0, 0x00, 0xFF, 0x88, 0x02]);
    ym_set(&mut play, 1, 7);
    play.extend_from_slice(&[0x4E, 0x75]);
    sndh(&tags, &init, &play)
}

fn writes(d: &[u8], sub0: u16, ticks: u64) -> Vec<(u64, u8, u8)> {
    let mut p = SndhPlayer::new(d, 48000).unwrap();
    p.start(sub0);
    p.ym_writes(ticks)
}

#[test]
fn parses_the_tags() {
    let d = two_song_tune();
    assert!(detect(&d));
    let i = parse(&d).unwrap();
    assert_eq!((i.title.as_str(), i.author.as_str(), i.year.as_str()), ("Test tune", "A Composer", "1990"));
    assert_eq!((i.subsongs, i.default_subsong, i.rate, i.vbl), (2, 1, 50, false));
    assert_eq!(i.ticks, vec![150, 200]);
}

#[test]
fn rate_tags_and_vbl() {
    let d = sndh(b"##01\0!V60\0", &[0x4E, 0x75], &[0x4E, 0x75]);
    let i = parse(&d).unwrap();
    assert_eq!((i.rate, i.vbl), (60, true));
    // an absurd rate is ignored
    let d = sndh(b"TC0\0", &[0x4E, 0x75], &[0x4E, 0x75]);
    assert_eq!(parse(&d).unwrap().rate, 50);
    // no tags at all is still an SNDH file
    let d = sndh(b"", &[0x4E, 0x75], &[0x4E, 0x75]);
    let i = parse(&d).unwrap();
    assert_eq!((i.subsongs, i.rate, i.vbl), (1, 50, true));
}

#[test]
fn default_subsong_and_frms() {
    let mut tags = b"##03\0#!02\0TC50\0FRMS".to_vec();
    tags.extend_from_slice(&[0, 0, 0, 100, 0, 0, 0, 200, 0, 0, 1, 0]);
    let d = sndh(&tags, &[0x4E, 0x75], &[0x4E, 0x75]);
    let i = parse(&d).unwrap();
    assert_eq!((i.subsongs, i.default_subsong), (3, 2));
    assert_eq!(i.ticks, vec![100, 200, 256]);
    // a default subsong beyond the count falls back to 1
    let d = sndh(b"##03\0#!09\0", &[0x4E, 0x75], &[0x4E, 0x75]);
    assert_eq!(parse(&d).unwrap().default_subsong, 1);
}

#[test]
fn rejects_non_sndh() {
    assert!(!detect(b"SNDH"));
    assert!(!detect(&[0u8; 64]));
    let mut d = two_song_tune();
    d[0] = 0x4E;
    assert!(!detect(&d));
    assert!(SndhPlayer::new(&d, 48000).is_err());
}

#[test]
fn init_gets_the_subsong_in_d0() {
    let d = two_song_tune();
    for sub in 0..2u16 {
        let w = writes(&d, sub, 3);
        let r0: Vec<u8> = w.iter().filter(|x| x.1 == 0).map(|x| x.2).collect();
        assert!(!r0.is_empty());
        assert!(r0.iter().all(|&v| v == sub as u8 + 1), "subsong {sub}: {r0:?}");
    }
}

#[test]
fn play_is_called_once_per_tick_at_the_rate() {
    let d = two_song_tune();
    let w = writes(&d, 0, 10);
    // play writes R0 and R1 each tick; the init's writes (none here) would be at sample 0
    let r1: Vec<u64> = w.iter().filter(|x| x.1 == 1).map(|x| x.0).collect();
    assert_eq!(r1.len(), 10);
    // a tick is a whole number of output samples: 48000 * 313 * 512 * 50 / (50 * 8,021,247) = 958
    assert_eq!(r1[0], 0);
    for p in r1.windows(2) {
        assert_eq!(p[1] - p[0], 958);
    }
}

#[test]
fn tick_length_is_the_references() {
    // whole samples of the ST's frame (313 lines of 512 clocks at 8.021247 MHz) over the tick rate
    assert_eq!(SndhPlayer::samples_per_tick(48000, 50), 958);
    assert_eq!(SndhPlayer::samples_per_tick(44100, 50), 881);
    assert_eq!(SndhPlayer::samples_per_tick(48000, 60), 799);
    assert_eq!(SndhPlayer::samples_per_tick(48000, 200), 239);
    assert_eq!(SndhPlayer::samples_per_tick(1000, 2000), 1);
}

/// The init code of a tune that sets timer A (data `data`, control `ctrl`) enabled and unmasked
/// with the handler `handler` (which ends in an RTE, or not).
fn timer_a_init(data: u8, ctrl: u8, handler: &[u8]) -> Vec<u8> {
    let mut init = Vec::new();
    // lea handler(pc),a0 ; move.l a0,$134.w
    let lea_at = init.len();
    init.extend_from_slice(&[0x41, 0xFA, 0, 0]);
    init.extend_from_slice(&[0x21, 0xC8, 0x01, 0x34]);
    for (reg, val) in [(0x07u16, 0x20u8), (0x13, 0x20), (0x1F, data), (0x19, ctrl)] {
        init.extend_from_slice(&[0x13, 0xFC, 0, val, 0x00, 0xFF, 0xFA, reg as u8]);
    }
    init.extend_from_slice(&[0x4E, 0x75]);
    let handler_at = init.len();
    init[lea_at + 2..lea_at + 4].copy_from_slice(&((handler_at - (lea_at + 2)) as u16).to_be_bytes());
    init.extend_from_slice(handler);
    init
}

/// A handler that writes YM R0 = $55 and returns.
fn write_55_handler() -> Vec<u8> {
    let mut h = Vec::new();
    ym_set(&mut h, 0, 0x55);
    h.extend_from_slice(&[0x4E, 0x73]);
    h
}

#[test]
fn timer_handlers_run_on_the_output_sample_grid() {
    // timer A: data 128, prescale 16 (2457600 / 16 / 128 = 1200 Hz): one underflow per 40 samples
    let d = sndh(b"##01\0TC50\0", &timer_a_init(128, 3, &write_55_handler()), &[0x4E, 0x75]);
    let w = writes(&d, 0, 10);
    let hits: Vec<u64> = w.iter().filter(|x| x.1 == 0 && x.2 == 0x55).map(|x| x.0).collect();
    // 10 ticks = 9580 samples
    assert_eq!(hits.len(), 239);
    // the first underflow is at the 40th sample, after which they come every 40
    assert_eq!(hits[0], 40);
    assert!(hits.windows(2).all(|p| p[1] - p[0] == 40));
}

#[test]
fn a_timer_and_the_play_routine_never_interleave() {
    // play writes R0 = 1, runs a long loop, then writes R1 = 7: on a real ST the 1200 Hz timer
    // would come in between; here a call takes no machine time and the two writes are on one sample
    let mut play = Vec::new();
    ym_set(&mut play, 0, 1);
    play.extend_from_slice(&[0x30, 0x3C, 0x01, 0xF4, 0x51, 0xC8, 0xFF, 0xFE]); // move.w #500,d0 ; dbra d0,*
    ym_set(&mut play, 1, 7);
    play.extend_from_slice(&[0x4E, 0x75]);
    let d = sndh(b"##01\0TC50\0", &timer_a_init(128, 3, &write_55_handler()), &play);
    let w = writes(&d, 0, 20);
    let mut seen = 0;
    for (i, x) in w.iter().enumerate() {
        if x.1 == 0 && x.2 == 1 {
            assert_eq!((w[i + 1].1, w[i + 1].2, w[i + 1].0), (1, 7, x.0), "{i}");
            seen += 1;
        }
    }
    assert_eq!(seen, 20);
    assert!(w.iter().any(|x| x.1 == 0 && x.2 == 0x55));
}

#[test]
fn a_handler_that_never_returns_ends_the_tune() {
    // the handler is `bra.s *`
    let d = sndh(b"##01\0TC50\0", &timer_a_init(128, 3, &[0x60, 0xFE]), &[0x4E, 0x75]);
    let mut p = SndhPlayer::new(&d, 48000).unwrap();
    assert!(!p.dead());
    let mut out = vec![0f32; 2 * 4800];
    p.render(&mut out);
    assert!(p.dead());
    assert!(out[out.len() - 100..].iter().all(|&x| x == 0.0));
}

#[test]
fn a_stop_in_the_play_routine_ends_the_tune() {
    // stop #$2000: nothing can wake it
    let d = sndh(b"##01\0TC50\0", &[0x4E, 0x75], &[0x4E, 0x72, 0x20, 0x00, 0x4E, 0x75]);
    let mut p = SndhPlayer::new(&d, 48000).unwrap();
    let mut out = vec![0f32; 2 * 4800];
    p.render(&mut out);
    assert!(p.dead());
}

#[test]
fn the_init_writes_are_at_sample_zero() {
    let mut init = Vec::new();
    ym_set(&mut init, 8, 9);
    init.extend_from_slice(&[0x4E, 0x75]);
    let d = sndh(b"##01\0TC50\0", &init, &[0x4E, 0x75]);
    let w = writes(&d, 0, 3);
    assert_eq!(w, vec![(0, 8, 9)]);
}

#[test]
fn a_period_written_in_a_handler_restarts_the_square_wave() {
    // timer A's handler writes tone period 1 to channel A (the sync-square effect): the wave is
    // restarted at the handler's end, so the channel's edge flips at each call
    let mut y = Ym::new(48000);
    y.write(7, 0x3E);
    y.write(8, 15);
    y.write(0, 0x00);
    y.write(1, 0x08); // a slow tone: period $800
    let before = y.sample();
    y.set_in_irq(true);
    y.write(0, 1);
    y.write(1, 0);
    y.set_in_irq(false);
    y.write(0, 0);
    y.write(1, 8);
    // the edge flipped by the restart: the level seen is the other half of the square wave
    let after = y.sample();
    assert_ne!(before, after);
}

#[test]
fn a_tune_that_never_returns_from_init_plays_silence() {
    // init: bra.s * (an endless loop)
    let d = sndh(b"##01\0TC50\0", &[0x60, 0xFE], &[0x4E, 0x75]);
    let mut p = SndhPlayer::new(&d, 48000).unwrap();
    assert!(p.dead());
    let mut out = vec![0f32; 2 * 4800];
    p.render(&mut out);
    let peak = out.iter().fold(0f32, |a, &x| a.max(x.abs()));
    // the chip's level 1 at volume 0 leaves a tiny DC step that the high-pass lets fall away
    assert!(peak < 0.01, "peak {peak}");
    let tail = out[out.len() - 200..].iter().fold(0f32, |a, &x| a.max(x.abs()));
    assert!(tail < 0.001, "tail {tail}");
}

#[test]
fn illegal_instruction_ends_the_tune() {
    let d = sndh(b"##01\0TC50\0", &[0x4E, 0x75], &[0x4A, 0xFC]);
    let mut p = SndhPlayer::new(&d, 48000).unwrap();
    let mut out = vec![0f32; 2 * 4800];
    p.render(&mut out);
    assert!(p.dead());
}

#[test]
fn render_makes_a_tone() {
    // init: tone A on at period $0100, full volume; play does nothing
    let mut init = Vec::new();
    ym_set(&mut init, 0, 0x00);
    ym_set(&mut init, 1, 0x01);
    ym_set(&mut init, 7, 0x3E);
    ym_set(&mut init, 8, 0x0F);
    init.extend_from_slice(&[0x4E, 0x75]);
    let d = sndh(b"##01\0TC50\0", &init, &[0x4E, 0x75]);
    let mut p = SndhPlayer::new(&d, 48000).unwrap();
    let mut out = vec![0f32; 2 * 48000];
    p.render(&mut out);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let rms = (left.iter().map(|x| x * x).sum::<f32>() / left.len() as f32).sqrt();
    assert!(rms > 0.05, "{rms}");
    // the pitch: 2,000,000 / (16 * 256) = 488 Hz, rising edges counted
    let mut edges = 0;
    for w in left[2400..].windows(2) {
        if w[0] < 0.0 && w[1] >= 0.0 {
            edges += 1;
        }
    }
    let secs = (left.len() - 2400) as f32 / 48000.0;
    let hz = edges as f32 / secs;
    assert!((hz - 488.3).abs() < 10.0, "{hz}");
}

#[test]
fn ended_at_the_time_tag_and_subsongs() {
    let d = two_song_tune();
    let mut p = SndhPlayer::new(&d, 48000).unwrap();
    assert_eq!(p.subsongs(), (0, 1, 0));
    assert_eq!(p.duration_secs(), Some(3.0));
    p.set_subsong(1);
    assert_eq!(p.duration_secs(), Some(4.0));
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..199 {
        p.render(&mut out);
    }
    assert!(!p.ended());
    p.render(&mut out);
    assert!(p.ended());
}

#[test]
fn engine_plays_it() {
    let d = two_song_tune();
    let mut e = crate::engine::Engine::new(48000.0);
    e.play_data(std::path::Path::new("test.sndh"), d).unwrap();
    let p = e.playing.as_ref().unwrap();
    assert_eq!(p.format, "SNDH");
    assert_eq!(p.module_name, "Test tune (1990)");
    assert_eq!(p.author, "A Composer");
    assert_eq!((p.sub_min, p.sub_max), (0, 1));
}

#[test]
fn ste_dma_plays_a_frame_and_signals_its_end() {
    let d = sndh(b"##01\0TC50\0", &[0x4E, 0x75], &[0x4E, 0x75]);
    let mut m = Machine::new(&d, 48000);
    assert!(m.call(BASE, Some(1), false, INIT_BUDGET));
    // 8 stereo frames at $20000: left i, right -i
    for i in 0..8u32 {
        m.bus.w8(0x20000 + 2 * i, i as u8);
        m.bus.w8(0x20001 + 2 * i, (i as u8).wrapping_neg());
    }
    let set = |b: &mut StBus, a: u32, v: u8| b.write_byte(0xFF8900 + a, v);
    // start $020000, end $020010; mode: stereo, 25 kHz (rate 2); control: play, loop off
    for (a, v) in [(3, 2), (5, 0), (7, 0), (0xF, 2), (0x11, 0), (0x13, 0x10), (0x21, 2), (1, 1)] {
        set(&mut m.bus, a, v);
    }
    let mut levels: Vec<(i32, i32)> = Vec::new();
    let mut ends = 0;
    for _ in 0..40 {
        let b = &mut m.bus;
        let (l, e) = b.dma.sample(&b.ram);
        ends += e as u32;
        if levels.last() != Some(&l) {
            levels.push(l);
        }
    }
    // each frame's output is held for the output samples until the next (a DAC sample per 1.92
    // output samples); volume 64 and a stereo source's gain of 2: 128 per count
    let want: Vec<(i32, i32)> = (0..8).map(|i| (128 * i, -128 * i)).chain([(0, 0)]).collect();
    assert_eq!(levels, want);
    // the frame ended once: playing stopped (no loop), the output is back to 0
    assert_eq!(ends, 1);
    assert_eq!(m.bus.read_byte(0xFF8901) & 1, 0);
}

#[test]
fn ste_dma_frame_end_counts_on_timer_a() {
    // timer A in event-count mode with a count of 2, enabled and unmasked, counts the DMA's frame
    // ends; GPI7 does too (count 1)
    let mut m = Mfp::new(48000);
    m.write(0x07, 0xA0);
    m.write(0x13, 0xA0);
    m.write(0x1F, 2);
    m.write(0x19, 8);
    // the GPI7 line fires at the first event, timer A at the second
    m.event_a();
    assert!(m.tick(4));
    assert!(!m.tick(0));
    m.event_a();
    assert!(m.tick(0));
    assert!(m.tick(4));
    // an event is counted once
    assert!(!m.tick(0) && !m.tick(4));
}

#[test]
fn ste_microwire_sets_the_master_volume() {
    let mut dma = super::Dma::new(48000);
    assert_eq!(dma.volume, 64);
    dma.write16(0x24, 0x07FF);
    dma.write16(0x22, 0x04C0 | 20); // master volume 20 of 40
    assert_eq!(dma.volume, 32);
    // the mask rotates on 16 reads, as a driver waits for the transfer to end
    for _ in 0..16 {
        dma.read16(0x24);
    }
    assert_eq!(dma.read16(0x24), 0x07FF);
}

// ------------------------------------------------------------------ the chips

/// A chip that makes one step per output sample (the output rate of the chip's step rate).
fn ym_at_step_rate() -> Ym {
    Ym::new(250_000)
}

#[test]
fn ym_tone_noise_and_envelope() {
    // channel A, period 4: a square wave of 8 steps, at full volume (level 31)
    let mut y = ym_at_step_rate();
    y.write(0, 4);
    y.write(7, 0x3E);
    y.write(8, 15);
    let v: Vec<i32> = (0..800).map(|_| y.sample()).collect();
    let (lo, hi) = (v[400..].iter().min().unwrap(), v[400..].iter().max().unwrap());
    assert!(hi - lo > 15000, "{lo} {hi}");
    let mid = (lo + hi) / 2;
    let edges = v[400..].windows(2).filter(|w| w[0] < mid && w[1] >= mid).count();
    assert!((49..=51).contains(&edges), "{edges}");
    // tone period 0 holds the channel high (sample playing through the volume register)
    let mut y = ym_at_step_rate();
    y.write(7, 0x3E);
    y.write(8, 15);
    let v: Vec<i32> = (0..50).map(|_| y.sample()).collect();
    assert!(v.iter().all(|&x| x > 15000), "{v:?}");
    // noise only on channel A: the output varies between silence and the channel's level
    let mut y = ym_at_step_rate();
    y.write(7, 0x37);
    y.write(8, 15);
    y.write(6, 1);
    let v: Vec<i32> = (0..20000).map(|_| y.sample()).collect();
    let (lo, hi) = (*v.iter().min().unwrap(), *v.iter().max().unwrap());
    assert!(hi - lo > 15000, "{lo} {hi}");
    // a silent chip is silent
    let mut y = ym_at_step_rate();
    assert!((0..100).all(|_| y.sample() == 0));
}

#[test]
fn ym_envelope_shapes() {
    let down: Vec<u8> = (0..32).rev().collect();
    let up: Vec<u8> = (0..32).collect();
    let zero = [0u8; 32];
    let hi = [31u8; 32];
    let check = |shape: u8, segs: [&[u8]; 4]| {
        let e = super::ym::envelope(shape);
        let want: Vec<u8> = segs.iter().flat_map(|s| s.iter().copied()).collect();
        assert_eq!(e.to_vec(), want, "shape {shape:#x}");
    };
    check(0x00, [&down, &zero, &zero, &zero]);
    check(0x04, [&up, &zero, &zero, &zero]);
    check(0x08, [&down, &down, &down, &down]);
    check(0x09, [&down, &zero, &zero, &zero]);
    check(0x0A, [&down, &up, &down, &up]);
    check(0x0B, [&down, &hi, &hi, &hi]);
    check(0x0C, [&up, &up, &up, &up]);
    check(0x0D, [&up, &hi, &hi, &hi]);
    check(0x0E, [&up, &down, &up, &down]);
    check(0x0F, [&up, &zero, &zero, &zero]);
}

#[test]
fn ym_envelope_runs_through_the_shape() {
    // channel A on the envelope, shape $0E (triangle), period 1: one level per step; the output
    // follows the 32 levels up and down
    let mut y = ym_at_step_rate();
    y.write(7, 0x3F);
    y.write(0, 0);
    y.write(7, 0x3E);
    y.write(8, 0x10);
    y.write(11, 1);
    y.write(12, 0);
    y.write(13, 0x0E);
    let v: Vec<i32> = (0..130).map(|_| y.sample()).collect();
    // rising through the first 32 steps, falling through the next 32
    let mx = v.iter().position(|&x| x == *v[..70].iter().max().unwrap()).unwrap();
    assert!((28..=36).contains(&mx), "{mx}");
    assert!(v[8] < v[16] && v[16] < v[28]);
    assert!(v[40] > v[52] && v[52] > v[60]);
}

#[test]
fn ym_register_masks() {
    let d = sndh(b"##01\0TC50\0", &[0x4E, 0x75], &[0x4E, 0x75]);
    let mut m = Machine::new(&d, 48000);
    for (r, v) in [(1u8, 0xFFu8), (6, 0xFF), (7, 0xFF), (8, 0xFF)] {
        m.bus.ym_write(0, r);
        m.bus.ym_write(2, v);
    }
    let rd = |m: &mut Machine, r: u8| {
        m.bus.ym_write(0, r);
        m.bus.ym_read(0)
    };
    assert_eq!((rd(&mut m, 1), rd(&mut m, 6), rd(&mut m, 7), rd(&mut m, 8)), (0x0F, 0x1F, 0x3F, 0x1F));
}

#[test]
fn mix_model_is_near_the_measured_levels() {
    // one channel at the 16 volumes: the first and the last, and the saturation of three
    let t = super::ym::mix_table();
    let one = |l: usize| t[l] as f32;
    assert!((one(31) - 17344.0).abs() < 1500.0, "{}", one(31));
    assert!(one(31) > 15.0 * one(15));
    let two = t[31 | 31 << 5] as f32;
    let three = t[31 | 31 << 5 | 31 << 10] as f32;
    assert!(two > 1.4 * one(31) && two < 1.8 * one(31), "{two}");
    assert!(three > two && three <= 32767.0);
}

#[test]
fn mix_table_treats_even_levels_as_the_next_odd_one_beside_an_odd_one() {
    let t = super::ym::mix_table();
    let at = |a: usize, b: usize, c: usize| t[a | b << 5 | c << 10];
    // an odd level among the three promotes the even ones; three even levels stand for themselves
    assert_eq!(at(10, 1, 20), at(11, 1, 21));
    assert_eq!(at(30, 0, 5), at(31, 1, 5));
    assert_ne!(at(10, 0, 20), at(11, 1, 21));
    // symmetric in the channels, silent with none
    assert_eq!(at(7, 20, 3), at(3, 7, 20));
    assert_eq!(at(0, 0, 0), 0);
    // the output rises with each level
    assert!((1..16).all(|v| at(2 * v + 1, 0, 0) >= at(2 * v - 1, 0, 0)));
    assert!((1..16).all(|v| at(2 * v, 0, 0) >= at(2 * v - 2, 0, 0)));
}

#[test]
fn a_tune_that_never_writes_the_envelope_shape_has_a_silent_envelope() {
    // channel B on the envelope with the tone and noise out of the way: nothing sounds
    let mut y = ym_at_step_rate();
    y.write(7, 0x3F);
    y.write(9, 0x10);
    y.write(11, 1);
    assert!((0..200).all(|_| y.sample().abs() < 20));
}

#[test]
fn mfp_timer_counts_and_fires() {
    let mut m = Mfp::new(48000);
    m.write(0x07, 0x20); // IERA: timer A
    m.write(0x13, 0x20); // IMRA
    m.write(0x1F, 128); // TADR = 128
    m.write(0x19, 3); // prescale 16: 3.2 counts per output sample at 48 kHz
    let mut fires = Vec::new();
    for s in 1..=400 {
        if m.tick(0) {
            fires.push(s);
        }
    }
    assert_eq!(fires.len(), 10);
    assert_eq!(fires[0], 40);
    assert!(fires.windows(2).all(|p| p[1] - p[0] == 40));
    // the data register reads back what is left
    m.tick(0);
    assert!(m.read(0x1F) < 128);
}

#[test]
fn mfp_enable_counts_and_mask_gates() {
    // not enabled: it does not count at all
    let mut m = Mfp::new(48000);
    m.write(0x13, 0x20);
    m.write(0x1F, 2);
    m.write(0x19, 1);
    assert!((0..100).all(|_| !m.tick(0)));
    // enabled but masked: counts, no handler
    m.write(0x13, 0);
    m.write(0x07, 0x20);
    assert!((0..100).all(|_| !m.tick(0)));
    // enabling restarts the counter; several underflows in one sample are one call
    m.write(0x13, 0x20);
    assert!(m.tick(0));
    // timer C is enabled and unmasked at the start (stopped, no handler until started)
    let mut m = Mfp::new(48000);
    assert!(!m.tick(2));
    m.write(0x23, 1);
    m.write(0x1D, 0x10);
    assert!(m.tick(2));
    // timer D's own bits
    m.write(0x09, 0x30);
    m.write(0x15, 0x30);
    m.write(0x25, 1);
    m.write(0x1D, 0x11);
    assert!(m.tick(3));
}

// ------------------------------------------------------------------ real tunes, when present

fn corpus() -> Vec<(String, Vec<u8>)> {
    let dir = std::env::var("SNDH_CORPUS")
        .unwrap_or_else(|_| "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/dl-modules/SNDH/files".to_string());
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if let Ok(d) = std::fs::read(e.path()) {
                v.push((e.file_name().to_string_lossy().into_owned(), crate::decrunch::decrunch(d)));
            }
        }
    }
    v.sort();
    v
}

#[test]
fn corpus_tunes_play() {
    let files = corpus();
    if files.is_empty() {
        eprintln!("skipped: no SNDH corpus");
        return;
    }
    for (name, d) in &files {
        assert!(detect(d), "{name}");
        let mut p = SndhPlayer::new(d, 48000).unwrap();
        let mut out = vec![0f32; 2 * 48000];
        p.render(&mut out);
        assert!(!p.dead(), "{name}");
        let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.01, "{name}: silent ({rms})");
        match name.as_str() {
            "Thrust.sndh" => assert_eq!((p.info().title.as_str(), p.info().author.as_str()), ("Thrust", "Rob Hubbard")),
            "Bionic_Commando.sndh" => assert_eq!(p.info().subsongs, 7),
            "Gem_X.sndh" => assert_eq!(p.info().subsongs, 4),
            _ => {}
        }
    }
}

/// The first ticks' register writes of two tunes are those of the reference player (AtariAudio,
/// patched to log them), in the same order (the digits are from its log, the chip's reset writes
/// left out); every tune's writes are in range and never go back in time.
#[test]
fn corpus_register_streams_match_the_reference() {
    let files = corpus();
    if files.is_empty() {
        eprintln!("skipped: no SNDH corpus");
        return;
    }
    const THRUST: &[(u8, u8)] = &[(13, 0), (12, 0), (11, 0), (10, 0), (9, 15), (8, 15), (7, 42), (6, 23), (5, 14), (4, 222), (3, 14), (2, 238), (1, 2), (0, 48), (13, 0), (12, 0), (11, 0), (10, 0), (9, 15), (8, 15), (7, 42), (6, 31), (5, 14), (4, 222), (3, 14), (2, 238), (1, 2), (0, 40), (13, 0), (12, 0), (11, 0), (10, 0), (9, 14), (8, 15), (7, 42), (6, 23), (5, 14), (4, 238), (3, 14), (2, 238), (1, 2), (0, 32), (13, 0), (12, 0), (11, 0), (10, 0), (9, 13), (8, 15)];
    const GEM_X: &[(u8, u8)] = &[(7, 255), (13, 0), (7, 255), (13, 0), (8, 14), (2, 140), (3, 0), (9, 9), (10, 9), (6, 0), (7, 254), (7, 246), (0, 35), (1, 3), (8, 14), (2, 139), (3, 0), (9, 9), (7, 242), (7, 210), (4, 168), (5, 0), (10, 11), (6, 8), (7, 210), (7, 218), (0, 71), (1, 6), (8, 14), (2, 140), (3, 0), (9, 9), (7, 218), (7, 250), (4, 81), (5, 1), (10, 11), (6, 8), (8, 14), (2, 142), (3, 0), (9, 9), (10, 11), (6, 8)];
    for (name, d) in &files {
        let mut p = SndhPlayer::new(d, 48000).unwrap();
        let w = p.ym_writes(100);
        assert!(w.len() > 50, "{name}");
        assert!(w.windows(2).all(|p| p[0].0 <= p[1].0), "{name}");
        assert!(w.iter().all(|x| x.1 < 14), "{name}");
        let want = match name.as_str() {
            "Thrust.sndh" => THRUST,
            "Gem_X.sndh" => GEM_X,
            _ => continue,
        };
        let got: Vec<(u8, u8)> = w.iter().take(want.len()).map(|x| (x.1, x.2)).collect();
        assert_eq!(got, want, "{name}");
    }
}

/// A deterministic stream of numbers (xorshift) for the damage tests.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn play_a_little(d: &[u8]) {
    if !detect(d) {
        return;
    }
    if let Ok(mut p) = SndhPlayer::new(d, 44100) {
        let mut out = vec![0f32; 2 * 4410];
        for _ in 0..3 {
            p.render(&mut out);
        }
        assert!(out.iter().all(|x| x.is_finite()));
        p.set_subsong(p.subsongs().1);
        p.render(&mut out);
    }
}

#[test]
fn damaged_files_do_not_panic_or_hang() {
    let mut files = vec![("synthetic".to_string(), two_song_tune())];
    files.extend(corpus());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for (name, d) in &files {
        // every truncation at a spread of lengths
        let step = (d.len() / 40).max(1);
        for n in (0..d.len()).step_by(step) {
            play_a_little(&d[..n]);
        }
        // random byte damage, with the header kept in a third of the cases
        for k in 0..12 {
            let mut x = d.clone();
            let hits = 1 + (rng.next() % 8) as usize;
            for _ in 0..hits {
                let lo = if k % 3 == 0 { 24.min(x.len() - 1) } else { 0 };
                let at = lo + (rng.next() as usize) % (x.len() - lo);
                x[at] = rng.next() as u8;
            }
            play_a_little(&x);
        }
        let _ = name;
    }
}

#[test]
fn garbage_with_the_magic_does_not_panic() {
    let mut rng = Rng(12345);
    for _ in 0..30 {
        let mut d: Vec<u8> = (0..2000).map(|_| rng.next() as u8).collect();
        d[0] = 0x60;
        d[12..16].copy_from_slice(b"SNDH");
        play_a_little(&d);
    }
}
