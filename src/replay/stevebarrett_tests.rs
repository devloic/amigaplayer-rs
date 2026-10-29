use super::super::m68kenc as enc;
use super::*;
use crate::engine::Engine;
use std::path::Path;

const BASE: u32 = 0x20000;
/// PERTAB[i] in the synthetic modules.
fn period(i: usize) -> u16 {
    500 + i as u16
}

fn put_w(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}
fn put_l(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}
fn put_code(d: &mut [u8], o: usize, ws: &[u16]) {
    for (i, w) in ws.iter().enumerate() {
        put_w(d, o + 2 * i, *w);
    }
}

/// A synthetic module of build `c` (an index of `BUILDS`): the four entries, the stop
/// routine's load, the two `lea` instructions, the variables at their initial values, one
/// song whose four voices play the same sequence (pattern 0, then wrap), one pattern given by
/// the caller, one 16-byte sample with a loop (oneshot 4, repeat 8), instrument 0 and envelope 0.
fn module(c: usize, pattern: &[u8]) -> Vec<u8> {
    let b = &BUILDS[c];
    let l = layout_of(c);
    let mut d = vec![0u8; l.samples + 104 + 16 + 4];
    for (i, &e) in b.entries.iter().enumerate() {
        put_code(&mut d, 4 * i, &enc::bcc_w(enc::BRA, (e as i64 - (4 * i as i64 + 2)) as i16));
    }
    put_code(&mut d, b.entries[3], &enc::moveq(0, 0));
    put_code(&mut d, b.entries[3] + 2, &enc::movea_l_imm(5, AUD0VOL));
    // the pointer base is the file start minus 6; the first sample is at `samples`
    put_code(&mut d, 0x74, &enc::lea_pc(4, (-6i64 - 0x76) as i16));
    put_code(&mut d, 0x78, &enc::lea_pc(0, (l.samples as i64 - 0x7a) as i16));
    d[l.flag] = 1;
    d[l.speed] = 1;
    d[l.dmamask] = 15;
    for v in 0..4 {
        d[l.vrec + F_REGOFF + v] = 16 * v as u8;
        d[l.vrec + F_DMABIT + v] = 1 << v;
        if !b.early {
            d[l.vrec + F_ARPBASE + v] = 10 * v as u8;
        }
    }
    if !b.early {
        d[l.arpint..l.arpint + 8].copy_from_slice(&[3, 5, 6, 7, 8, 9, 4, 12]);
    }
    for i in 0..73 {
        put_w(&mut d, l.pertab + 2 * i, period(i));
    }
    // one sequence (pattern 0, wrap) and the pattern, after the pattern table
    let seq = l.pattab + 16;
    let pat = l.pattab + 32;
    d[seq] = 0;
    d[seq + 1] = 255;
    d[pat..pat + pattern.len()].copy_from_slice(pattern);
    put_w(&mut d, l.pattab, (pat + 6) as u16);
    for v in 0..4 {
        put_w(&mut d, l.songs + 2 * v, (seq + 6) as u16);
    }
    d[l.env] = 63;
    // the sample record: oneshot at 20, repeat at 24, BODY at 96 with its size
    let r = l.samples;
    d[r..r + 4].copy_from_slice(b"FORM");
    put_l(&mut d, r + 20, 4);
    put_l(&mut d, r + 24, 8);
    d[r + 96..r + 100].copy_from_slice(b"BODY");
    put_l(&mut d, r + 100, 16);
    d
}

/// Plays `ticks` ticks of subsong 0 in a bare machine and returns the player and the
/// register writes (address, value) it made, the load-time ones first.
fn run(d: &[u8], ticks: usize) -> (Sb, Vec<(u32, u16)>) {
    let mut e = Engine::new(48000.0);
    e.m.bus.write_log = Some(Vec::new());
    let (mut s, clear) = init(d, BASE).unwrap();
    for a in clear {
        e.m.bus.write_bytes(a, &[0, 0]);
    }
    if s.b.loops {
        s.stop(&mut Hw { bus: &mut e.m.bus }, false);
    }
    for _ in 0..ticks {
        s.tick(&mut Hw { bus: &mut e.m.bus });
    }
    let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
    (s, log)
}

fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

const PER: u32 = 0xDFF0A6;
const VOL: u32 = 0xDFF0A8;
const LEN: u32 = 0xDFF0A4;
const LCL: u32 = 0xDFF0A2;
const DMA: u32 = 0xDFF096;

/// A pattern: duration 4, note 20, stop.
const ONE_NOTE: [u8; 3] = [0x84, 20, 0xFE];

#[test]
fn recognises_every_build_by_structure() {
    for c in 0..BUILDS.len() {
        let d = module(c, &ONE_NOTE);
        assert!(detect(&d), "build {c}");
        let f = find(&d).unwrap();
        assert_eq!((f.build, f.pbase), (c, -6));
    }
}

#[test]
fn refuses_what_does_not_fit() {
    let d = module(0, &ONE_NOTE);
    let l = layout_of(0);
    assert!(!detect(&[]));
    assert!(!detect(&d[..100]));
    // an unknown set of entries (a branch retargeted)
    let mut e = d.clone();
    put_code(&mut e, 0, &enc::bcc_w(enc::BRA, 200));
    assert!(!detect(&e));
    // a short branch instead of a word branch
    let mut e = d.clone();
    put_code(&mut e, 0, &enc::bra_s(0x40));
    assert!(!detect(&e));
    // the stop routine does not load the AUD0VOL address
    let mut e = d.clone();
    put_code(&mut e, BUILDS[0].entries[3] + 2, &enc::movea_l_imm(5, 0x1234));
    assert!(!detect(&e));
    // the first lea does not reach `FORM`
    let mut e = d.clone();
    e[l.samples] = b'X';
    assert!(!detect(&e));
    // the instruction before it is not a pc-relative lea
    let mut e = d.clone();
    put_code(&mut e, 0x74, &enc::nop());
    put_code(&mut e, 0x76, &enc::nop());
    assert!(!detect(&e));
    // a variable that does not hold its initial value
    for o in [l.flag, l.dmamask, l.speed, l.started, l.vrec + F_DMABIT + 1] {
        let mut e = d.clone();
        e[o] ^= 0x40;
        assert!(!detect(&e), "offset {o}");
    }
    // the arpeggio interval table of a full build
    let f = module(2, &ONE_NOTE);
    let mut e = f.clone();
    e[layout_of(2).arpint] = 9;
    assert!(!detect(&e));
    // a sample without a body
    let mut e = d.clone();
    e[l.samples + 96] = 0;
    assert!(!detect(&e));
}

#[test]
fn counts_subsongs_from_the_tables() {
    let f = find(&module(2, &ONE_NOTE)).unwrap();
    assert_eq!(f.subs, (layout_of(2).pattab - layout_of(2).songs) / 8);
    let (s, _) = init(&module(5, &ONE_NOTE), BASE).unwrap();
    assert_eq!(s.subsongs(), (0, 4, 0));
}

#[test]
fn sample_init_of_loops_builds() {
    let d = module(0, &ONE_NOTE);
    let l = layout_of(0);
    let (s, clear) = init(&d, BASE).unwrap();
    // the record's first word and the data's first word are cleared, the loop goes into instrument 0
    assert_eq!(s.rw(l.samples), 0);
    assert_eq!((s.rw(l.instr + 2), s.rw(l.instr + 4)), (4, 8));
    let data = l.samples + 104;
    assert_eq!(clear, vec![BASE + data as u32]);
    assert_eq!((s.rw(l.smpaddr), s.rl(l.smpaddr + 2)), (8, BASE + data as u32));
    // the load-time stop
    let (_, log) = run(&d, 0);
    assert_eq!(log, (0..4).map(|v| (VOL + 16 * v, 0)).collect::<Vec<_>>());
    // a plain build leaves the record alone and writes nothing
    let (p, log) = run(&module(2, &ONE_NOTE), 0);
    assert_eq!(p.rb(layout_of(2).samples), b'F');
    assert!(log.is_empty());
}

#[test]
fn first_tick_is_the_song_init_second_starts_the_notes() {
    let d = module(0, &ONE_NOTE);
    let l = layout_of(0);
    let (s, log) = run(&d, 1);
    // the stop routine only
    assert_eq!(log.len(), 8);
    assert_eq!((s.rb(l.started), s.rb(l.flag), s.rb(l.dmamask)), (1, 255, 15));
    let (s, log) = run(&d, 2);
    // voices 3, 2, 1, 0: DMA off, length, address; then the output stage
    let w = &log[8..];
    assert_eq!(w[0], (DMA, 8));
    assert_eq!(w[1], (0xDFF0D4, 8));
    assert_eq!((w[2].0, w[3].0), (0xDFF0D0, 0xDFF0D2));
    assert_eq!(reg(&log, PER), vec![period(20)]);
    for v in 0..4u32 {
        assert_eq!(reg(&log, PER + 16 * v), vec![period(20)]);
        assert_eq!(reg(&log, VOL + 16 * v).last(), Some(&63));
    }
    // the DMA-on write, then its masked pair
    let dm = reg(&log, DMA);
    assert_eq!(&dm[dm.len() - 2..], &[0x820F, 0x0000]);
    // the repeat stage: length halved, address plus the repeat offset
    assert_eq!(reg(&log, LEN).last(), Some(&4));
    let smp = (BASE + (l.samples + 104) as u32) & 0xFFFF;
    assert_eq!(reg(&log, LCL).last(), Some(&(smp as u16 + 4)));
    assert_eq!(s.g(F_NOTE, 3), 20);
    assert_eq!(s.g(F_DURCNT, 0), 4);
}

#[test]
fn song_ends_on_the_second_stop() {
    let d = module(1, &ONE_NOTE);
    let (s, log) = run(&d, 30);
    assert!(s.songend());
    // the end writes: DMACON 15, then the four volumes 0
    let n = log.len();
    assert_eq!(log[n - 5], (DMA, 15));
    assert!(log[n - 4..].iter().all(|w| w.1 == 0));
    // the player stays silent afterwards
    let (_, longer) = run(&d, 60);
    assert_eq!(longer.len(), n);
}

#[test]
fn song_ends_when_every_voice_has_wrapped() {
    // the pattern is a note; the sequence wraps every pass
    let d = module(0, &[0x82, 10, 0xFF]);
    let (s, _) = run(&d, 40);
    assert!(s.songend());
}

#[test]
fn byte_classes_of_a_row() {
    // speed 3, instrument 5, envelope 2, duration 5, note 7
    let d = module(0, &[0xF3, 0xE5, 0xC2, 0x85, 7, 0xFE]);
    let (s, log) = run(&d, 2);
    let l = layout_of(0);
    assert_eq!(s.rb(l.speed), 3);
    assert_eq!((s.g(F_INSTR, 1), s.g(F_ENV, 1), s.g(F_DUR, 1), s.g(F_NOTE, 1)), (40, 16, 5, 7));
    assert_eq!(reg(&log, PER + 16), vec![period(7)]);
    // the pattern pointer moved past the five bytes of the row
    assert_eq!(s.rl(l.patptr + 4), s.pabs(s.rw(l.pattab) as u32) + 5);
}

#[test]
fn end_of_pattern_byte_advances_the_sequence() {
    let d = module(0, &[0xFF]);
    let (s, _) = run(&d, 3);
    // the sequence wrapped to the pattern again, so the voices keep reading it
    assert_eq!(s.rb(layout_of(0).flag), 255);
}

#[test]
fn transpose_bytes_of_a_sequence() {
    let mut d = module(0, &ONE_NOTE);
    let l = layout_of(0);
    // sequence: transpose +4 (196), pattern 0
    d[l.pattab + 16..l.pattab + 19].copy_from_slice(&[196, 0, 255]);
    let (s, log) = run(&d, 2);
    assert_eq!(s.g(F_TRANSPOSE, 0), 4);
    assert_eq!(reg(&log, PER), vec![period(24)]);
}

#[test]
fn slide_down_effect_of_the_full_family() {
    // duration 4, slide down, note 20
    let d = module(2, &[0x84, 127, 20, 0xFE]);
    let (s, _) = run(&d, 4);
    assert_eq!((s.g(F_SLIDEDOWN, 0), s.g(F_MODE, 0)), (1, 1));
    // the tick after the row has SPDCNT 1; the next one slides
    assert_eq!(s.g(F_NOTE, 0), 19);
    assert_eq!(s.rw(layout_of(2).perout), period(19));
    // the early family reads 127 as the note itself
    let e = module(0, &[0x84, 127, 20, 0xFE]);
    let (s, _) = run(&e, 2);
    assert_eq!(s.g(F_NOTE, 0), 127);
}

#[test]
fn arpeggio_effect_steps_through_the_intervals() {
    // arpeggio parameter 0b101: intervals 3 and 6
    let d = module(3, &[0x88, 125, 0b101, 20, 0xFE]);
    let l = layout_of(3);
    let (s, _) = run(&d, 3);
    assert_eq!(s.g(F_ARP, 0), 5);
    assert_eq!(&s.m[l.arpbuf + 1..l.arpbuf + 4], &[3, 6, 255]);
    // the tick after the row steps the arpeggio: the first interval above the note
    assert_eq!(s.rw(l.perout), period(23));
}

#[test]
fn portamento_moves_the_period_toward_the_target() {
    // duration 8, portamento to note 24 with parameter 0x13, note 20
    let d = module(2, &[0x88, 123, 24, 0x13, 20, 0xFE]);
    let l = layout_of(2);
    let (s, _) = run(&d, 3);
    assert_eq!((s.g(F_PTARGET, 0), s.g(F_PPARAM, 0)), (24, 0x13));
    // the target is a higher note: the period moves by the difference rotated right by 3
    assert_eq!(s.rb(l.p_tgt), 24);
    assert_eq!(s.rb(l.p_lo), 3);
}

#[test]
fn vibrato_follows_the_envelope() {
    // envelope 0: vibrato parameter 0x22
    let mut d = module(0, &[0x88, 20, 0xFE]);
    d[layout_of(0).env + 5] = 0x22;
    let (s, log) = run(&d, 4);
    let l = layout_of(0);
    assert_eq!(s.g(F_VIBPARAM, 0), 0x22);
    // the vibrato ran on the tick between the rows: shift 2, depth 2
    assert_eq!(s.rb(l.vibdepth), 0x02);
    assert_eq!(s.rw(l.vibdelta), ((period(20) - period(21)) / 2 | 1) / 2 | 1);
    assert!(reg(&log, PER).len() >= 2);
}

#[test]
fn loop_counts_and_tick_rate() {
    assert_eq!([0, 1, 128, 129, 255].map(count_first), [0, 1, 128, 0, 0]);
    assert_eq!([0, 1, 128, 129, 255].map(count_after), [1, 2, 129, 1, 1]);
    assert!(hz100(5394 + SAMPLES_SIZE_100HZ - 4, 5394));
    assert!(!hz100(5394 + SAMPLES_SIZE_100HZ, 5394));
    let (s, _) = init(&module(0, &ONE_NOTE), BASE).unwrap();
    assert_eq!(s.tick_clocks(), DT_TICK);
    let mut h = s;
    h.hz100 = true;
    assert_eq!(h.tick_clocks(), DT_TICK / 2);
}

#[test]
fn a_100_hz_song_end_waits_one_tick() {
    let d = module(1, &ONE_NOTE);
    let mut e = Engine::new(48000.0);
    e.m.bus.write_log = Some(Vec::new());
    let (mut s, _) = init(&d, BASE).unwrap();
    s.hz100 = true;
    let mut ended_at = None;
    let mut wrote_at = None;
    for t in 0..40 {
        let n0 = e.m.bus.write_log.as_ref().unwrap().len();
        s.tick(&mut Hw { bus: &mut e.m.bus });
        let log = e.m.bus.write_log.as_ref().unwrap();
        if ended_at.is_none() && s.songend() {
            ended_at = Some(t);
        }
        // the host's DMACON 15 (an output stage writes 15 only right after DMACON 0x8200)
        let host = (n0..log.len()).any(|i| (log[i].0, log[i].1) == (DMA, 15) && (i == 0 || (log[i - 1].0, log[i - 1].1) != (DMA, 0x8200)));
        if wrote_at.is_none() && host {
            wrote_at = Some(t);
        }
    }
    assert_eq!(wrote_at.unwrap(), ended_at.unwrap() + 1);
}

#[test]
fn set_subsong_restarts() {
    let d = module(2, &ONE_NOTE);
    let mut e = Engine::new(48000.0);
    let (mut s, _) = init(&d, BASE).unwrap();
    for _ in 0..30 {
        s.tick(&mut Hw { bus: &mut e.m.bus });
    }
    assert!(s.songend());
    s.set_subsong(1, &mut Hw { bus: &mut e.m.bus });
    assert!(!s.songend());
    assert_eq!(s.subsongs().2, 1);
    assert_eq!(s.rb(layout_of(2).flag), 2);
    s.set_subsong(9, &mut Hw { bus: &mut e.m.bus });
    assert_eq!(s.subsongs().2, 1);
}

#[test]
fn engine_picks_the_player_from_the_content() {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/sb-test/sb.song"), module(0, &ONE_NOTE)).unwrap();
    assert_eq!(e.playing.as_ref().unwrap().format, "Steve Barrett");
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..30 {
        e.render(&mut out);
    }
    assert!(e.songend());
}
