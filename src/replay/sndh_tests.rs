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
    // the machine's first event is the chip's mixer reset; play writes R0 and R1 each tick
    let r1: Vec<u64> = w.iter().filter(|x| x.1 == 1).map(|x| x.0).collect();
    assert_eq!(r1.len(), 10);
    // TC50 is an exact 50 Hz: 160,000 clocks of 8 MHz between calls
    for p in r1.windows(2) {
        assert_eq!(p[1] - p[0], 160_000);
    }
}

#[test]
fn vbl_tick_is_the_st_frame() {
    let d = sndh(b"##01\0!V50\0", &[0x4E, 0x75], &{
        let mut p = Vec::new();
        ym_set(&mut p, 1, 7);
        p.extend_from_slice(&[0x4E, 0x75]);
        p
    });
    let w = writes(&d, 0, 6);
    let t: Vec<u64> = w.iter().filter(|x| x.1 == 1).map(|x| x.0).collect();
    // 160,256 clocks of 8.021247 MHz = 159,832 clocks of 8 MHz (rounded by the tick's own arithmetic)
    let step = t[3] - t[2];
    assert!((159_800..159_870).contains(&step), "{step}");
}

#[test]
fn timer_interrupts_preempt_and_return() {
    // init: vector $134 = handler (timer A), timer A: data 128, prescale 16 (2457600 / 16 / 128 = 1200 Hz),
    // enabled and unmasked; handler: YM R0 = $55, RTE. Play does nothing.
    let mut init = Vec::new();
    // lea handler(pc),a0 ; move.l a0,$134.w
    let lea_at = init.len();
    init.extend_from_slice(&[0x41, 0xFA, 0, 0]);
    init.extend_from_slice(&[0x21, 0xC8, 0x01, 0x34]);
    for (reg, val) in [(0x07u16, 0x20u8), (0x13, 0x20), (0x1F, 0x80), (0x19, 0x03)] {
        init.extend_from_slice(&[0x13, 0xFC, 0, val, 0x00, 0xFF, 0xFA, reg as u8]);
    }
    init.extend_from_slice(&[0x4E, 0x75]);
    let handler_at = init.len();
    init[lea_at + 2..lea_at + 4].copy_from_slice(&((handler_at - (lea_at + 2)) as u16).to_be_bytes());
    ym_set(&mut init, 0, 0x55);
    init.extend_from_slice(&[0x4E, 0x73]);
    let d = sndh(b"##01\0TC50\0", &init, &[0x4E, 0x75]);
    let w = writes(&d, 0, 10);
    let hits: Vec<u64> = w.iter().filter(|x| x.1 == 0 && x.2 == 0x55).map(|x| x.0).collect();
    // 10 ticks = 0.2 s at 1200 Hz = 240 interrupts (the init and the first interrupt take a little)
    assert!((236..=241).contains(&hits.len()), "{}", hits.len());
    // the spacing is 8,000,000 / 1200 = 6,666.7 clocks
    let gap = hits[100] - hits[99];
    assert!((6660..=6675).contains(&gap), "{gap}");
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
    let mut m = Machine::new(&d, 50, false, 1);
    m.finish_init();
    // 8 stereo frames at $20000: left i, right -i
    for i in 0..8u32 {
        m.bus.w8(0x20000 + 2 * i, i as u8);
        m.bus.w8(0x20001 + 2 * i, (i as u8).wrapping_neg());
    }
    let b = &mut m.bus;
    let set = |b: &mut StBus, a: u32, v: u8| b.write_byte(0xFF8900 + a, v);
    // start $020000, end $020010; mode: stereo, 50 kHz (rate 3); control: play, loop off
    for (a, v) in [(3, 2), (5, 0), (7, 0), (0xF, 2), (0x11, 0), (0x13, 0x10), (0x21, 3), (1, 1)] {
        set(b, a, v);
    }
    b.events.clear();
    // 8 samples at 50,066 Hz = 1,278 clocks, the frame's end is noticed at the 9th (1,438)
    m.pass(1500);
    let s: Vec<(i8, i8)> = m.bus.events.iter().filter_map(|e| if let Ev::Dma(l, r) = e.1 { Some((l, r)) } else { None }).collect();
    assert_eq!(&s[..8], &[(0, 0), (1, -1), (2, -2), (3, -3), (4, -4), (5, -5), (6, -6), (7, -7)]);
    // the frame ended: playing stopped (no loop), the output is back to 0
    assert_eq!(m.bus.read_byte(0xFF8901) & 1, 0);
    assert_eq!(*s.last().unwrap(), (0, 0));
}

#[test]
fn ste_microwire_sets_the_master_volume() {
    let mut dma = super::Dma::new();
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

#[test]
fn ym_tone_noise_and_envelope() {
    let mut y = Ym::new();
    y.reset();
    // channel A, period 2: toggles every 2 steps; volume 15 -> level 31
    y.write(0, 2);
    y.write(7, 0x3E);
    y.write(8, 15);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..40 {
        seen.insert((y.step() * 1000.0) as i32);
    }
    assert_eq!(seen.len(), 2, "{seen:?}");
    // tone period 0 holds the channel high (sample playing through the volume register)
    let mut y = Ym::new();
    y.write(7, 0x3E);
    y.write(8, 15);
    let a = y.step();
    assert!(a > 0.9 && (0..50).all(|_| (y.step() - a).abs() < 1e-6));
    // noise only: the output both varies and is not constant
    let mut y = Ym::new();
    y.write(7, 0x37);
    y.write(8, 15);
    y.write(6, 1);
    let v: Vec<f32> = (0..400).map(|_| y.step()).collect();
    let (lo, hi) = (v.iter().cloned().fold(9.0, f32::min), v.iter().cloned().fold(0.0, f32::max));
    assert!(lo < 0.05 && hi > 0.9, "{lo} {hi}");
    // envelope shape 0x0E, period 1: a triangle through 32 levels up and down
    let mut y = Ym::new();
    y.write(7, 0x3E);
    y.write(0, 0);
    y.write(8, 0x10);
    y.write(11, 1);
    y.write(12, 0);
    y.write(13, 0x0E);
    let levels: Vec<u8> = (0..70).map(|_| { y.step(); y.env_level() }).collect();
    assert_eq!(levels[0], 1);
    assert_eq!(levels[30], 31);
    assert!(levels[40] < 31 && levels[62] < 3);
    // shape 0x00: one down ramp, then silence
    let mut y = Ym::new();
    y.write(13, 0);
    y.write(11, 1);
    for _ in 0..100 {
        y.step();
    }
    assert_eq!(y.env_level(), 0);
}

#[test]
fn ym_register_masks() {
    let mut y = Ym::new();
    y.write(1, 0xFF);
    y.write(6, 0xFF);
    y.write(8, 0xFF);
    assert_eq!((y.read(1), y.read(6), y.read(8)), (0x0F, 0x1F, 0x1F));
}

#[test]
fn mfp_timer_counts_and_vectors() {
    let mut m = Mfp::new(8_000_000);
    m.write(0x07, 0x20); // IERA: timer A
    m.write(0x13, 0x20); // IMRA
    m.write(0x1F, 4); // TADR = 4
    m.write(0x19, 1); // prescale 4 -> 16 MFP clocks per underflow
    assert!(!m.irq());
    // 16 MFP clocks = 16 * 8e6 / 2.4576e6 = 52.08 CPU clocks
    m.advance(52);
    assert!(!m.irq());
    m.advance(1);
    assert!(m.irq());
    assert_eq!(m.acknowledge(), 0x40 + 13);
    assert!(!m.irq());
    // next_event predicts the next underflow
    let n = m.next_event().unwrap();
    assert!((51..=54).contains(&n), "{n}");
    // data register read-back counts down
    m.advance(30);
    assert!(m.read(0x1F) < 4);
    // masked: pending but not delivered
    m.write(0x13, 0);
    m.advance(200);
    assert!(!m.irq());
    m.write(0x13, 0x20);
    assert!(m.irq());
    // priority: timer A (13) over timer C (5)
    m.write(0x09, 0x20);
    m.write(0x15, 0x20);
    m.write(0x23, 1);
    m.write(0x1D, 0x10);
    m.advance(2000);
    assert_eq!(m.acknowledge(), 0x40 + 13);
    assert_eq!(m.acknowledge(), 0x40 + 5);
}

#[test]
fn mfp_in_service_blocks_with_software_eoi() {
    let mut m = Mfp::new(8_000_000);
    m.write(0x17, 0x48); // software end of interrupt
    m.write(0x07, 0x20);
    m.write(0x13, 0x20);
    m.write(0x1F, 1);
    m.write(0x19, 1);
    m.advance(200);
    assert_eq!(m.acknowledge(), 0x40 + 13);
    m.advance(200);
    // pending again but in service: not delivered until the handler clears ISR
    assert!(!m.irq());
    m.write(0x0F, 0xDF);
    assert!(m.irq());
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
        // (the machine's own first event, the mixer reset, is not the tune's)
        let got: Vec<(u8, u8)> = w.iter().skip(1).take(want.len()).map(|x| (x.1, x.2)).collect();
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
