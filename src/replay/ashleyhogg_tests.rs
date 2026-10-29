//! Tests on synthetic modules (built here, no corpus bytes): the recognition and the
//! locators on their instruction forms, and the ticks of both families.

use super::*;
use crate::bus::Bus;
use crate::replay::m68kenc as enc;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// Writes the words of an instruction at `o`.
fn code(d: &mut [u8], o: usize, ws: Vec<u16>) -> usize {
    for (i, w) in ws.iter().enumerate() {
        put16(d, o + 2 * i, *w);
    }
    o + 2 * ws.len()
}

/// Runs `n` ticks and returns each tick's writes (address, value).
fn run(p: &mut dyn Replayer, n: usize) -> Vec<Vec<(u32, u16)>> {
    let mut bus = Bus::new();
    bus.write_log = Some(Vec::new());
    let mut out = Vec::new();
    for k in 0..n {
        bus.clock = (k as u64 + 1) * TICK;
        p.tick(&mut Hw { bus: &mut bus });
        out.push(bus.write_log.take().unwrap().into_iter().map(|(a, v, _)| (a, v)).collect());
        bus.write_log = Some(Vec::new());
    }
    out
}

const DMACON: u32 = 14676118;
const fn reg(c: u32, r: u32) -> u32 {
    14676128 + 16 * c + r
}
const LOC_HI: u32 = 0;
const LOC_LO: u32 = 2;
const LEN: u32 = 4;
const PER: u32 = 6;
const VOL: u32 = 8;

// ------------------------------------------------------------------ the 1991 family

const BASE: u32 = 131072;
const PERT: usize = 512;
const CELL: usize = 624;
const EMPTY: usize = 628;
const SYN: usize = 640;
const INS: usize = 768;
const ARPO: usize = 864;
const ARPD: usize = 872;
const PATO: usize = 880;
const SONGS: usize = 896;
const SPD: usize = 912;
const SEQ: usize = 916;
const PATB: usize = 928;
const SMP: usize = 1024;

/// What the build's code does differently.
#[derive(Clone, Copy, Default)]
struct Opts {
    fade: bool,
    only3: bool,
    loop_off: bool,
}

/// The 1991 header, the key instruction forms of the init and play code, and the tables:
/// one song, the speed 3, voice 0 on `seq`, pattern 0 = `pat`; instrument 0 plays the
/// sample at offset 10 (4 bytes) at volume 32.
fn module91(seq: &[u8], pat: &[u8], o: Opts) -> Vec<u8> {
    let mut d = vec![0u8; 1100];
    for i in 0..4 {
        code(&mut d, 4 * i, enc::bcc_w(enc::BRA, 62));
    }
    // move.w #0,d0 ; bne.s over 34 bytes ; move.w d0,abs.l
    code(&mut d, 16, enc::move_w_imm_d(0, 0));
    code(&mut d, 20, enc::bne_s(34));
    code(&mut d, 22, enc::move_w_d_abs(0, 14676136));
    let mut a = 64;
    a = code(&mut d, a, enc::lea_pc_at(0, a, SPD));
    a = code(&mut d, a, enc::move_b_idx_disp(0, 0, 4, 47));
    a = code(&mut d, a, enc::lea_pc_at(0, a, SONGS));
    a = code(&mut d, a, enc::lea_pc_at(3, a, SEQ));
    a = code(&mut d, a, enc::mulu_imm_d(44, 1));
    a = code(&mut d, a, enc::lea_pc_at(3, a, INS));
    a = code(&mut d, a, enc::lea_pc_at(3, a, SMP));
    a = code(&mut d, a, enc::adda_l_disp(0, 32, 3));
    a = code(&mut d, a, enc::lea_pc_at(4, a, PERT));
    a = code(&mut d, a, enc::asl_w_imm(1, 2));
    a = code(&mut d, a, enc::lea_pc_at(3, a, ARPD));
    a = code(&mut d, a, enc::lea_pc_at(4, a, ARPO));
    a = code(&mut d, a, enc::move_w_disp_d(0, 6, 6));
    a = code(&mut d, a, enc::asl_w_imm(2, 6));
    a = code(&mut d, a, enc::lea_pc_at(3, a, PATO));
    a = code(&mut d, a, enc::move_l_idx_disp(3, 1, 0, 54));
    a = code(&mut d, a, enc::lea_pc_at(3, a, PATB));
    a = code(&mut d, a, enc::move_l_pc_d(6, a, CELL));
    a = code(&mut d, a, enc::asl_w_imm(7, 6));
    a = code(&mut d, a, enc::lea_pc_at(4, a, SYN));
    // the build's code
    if o.fade {
        a = code(&mut d, a, enc::cmpi_b_ind(130, 2));
    }
    if o.only3 {
        a = code(&mut d, a, enc::cmp_b_imm_d(8, 7));
    }
    if o.loop_off {
        a = code(&mut d, a, enc::cmpi_b_ind(129, 2));
        a = code(&mut d, a, enc::beq_s(2));
        a = code(&mut d, a, enc::nop());
        code(&mut d, a, enc::move_l_disp_d(0, 58, 0));
    }
    // the tables
    for i in 0..49 {
        put16(&mut d, PERT + 2 * i, 100 + 10 * i as u16);
    }
    // instrument 0: sustain level 1, volume word 32, sample at 10, 2 words
    d[INS + 2] = 1;
    put16(&mut d, INS + 4, 32);
    put32(&mut d, INS + 32, 10);
    put16(&mut d, INS + 40, 2);
    // arpeggio 0: 0, then back to position 0
    d[ARPD..ARPD + 3].copy_from_slice(&[0, 128, 0]);
    d[SPD] = 3;
    // voice 0 starts 2 bytes into the sequence
    put32(&mut d, SONGS, 2);
    d[SEQ + 2..SEQ + 2 + seq.len()].copy_from_slice(seq);
    d[PATB..PATB + pat.len()].copy_from_slice(pat);
    d
}

fn p91(d: &[u8]) -> Ash91 {
    Ash91::new(d, BASE, locate91(d).unwrap())
}

fn song91() -> Vec<u8> {
    module91(&[0, 0, 129], &[5, 0, 255], Opts::default())
}

#[test]
fn the_1991_header_and_the_tables_are_found_by_instruction_forms() {
    let d = song91();
    assert_eq!(family(&d), Some(Fam::Y91));
    assert!(detect(&d));
    let l = locate91(&d).unwrap();
    assert_eq!(
        l,
        Lay91 { per: PERT, empty: EMPTY, syn: SYN, ins: INS, arpo: ARPO, arpd: ARPD, pato: PATO, patb: PATB, songs: SONGS, spd: SPD, seq: SEQ, smp: SMP, nsongs: 1, fade: false, loop_off: false, only3: false }
    );
}

#[test]
fn the_builds_are_told_apart_by_their_code() {
    let l = |o| locate91(&module91(&[0, 0, 129], &[5, 0, 255], o)).unwrap();
    assert!(l(Opts { fade: true, ..Opts::default() }).fade);
    assert!(!l(Opts::default()).fade);
    assert!(l(Opts { only3: true, ..Opts::default() }).only3);
    assert!(l(Opts { loop_off: true, ..Opts::default() }).loop_off);
    assert!(!l(Opts { fade: true, only3: true, ..Opts::default() }).loop_off);
}

#[test]
fn a_missing_key_form_refuses_the_module() {
    // without the shift left by 7 there is no SYN
    let mut d = song91();
    let at = (64..400).step_by(2).find(|&o| matches!(decode(&d, o), Some(i) if i.k == K::Shift && i.data == 7)).unwrap();
    code(&mut d, at, enc::nop());
    assert!(locate91(&d).is_none());
    assert!(!detect(&d));
    // a right shift instead of the left shift by 1 is not the key
    let mut d = song91();
    let at = (64..400).step_by(2).find(|&o| matches!(decode(&d, o), Some(i) if i.k == K::Shift && i.data == 1)).unwrap();
    code(&mut d, at, enc::lsr_b_imm(1, 2));
    assert!(locate91(&d).is_none());
}

#[test]
fn header_checks() {
    assert!(!detect(&[0u8; 3000]));
    // a displacement of 0, odd, or 32768 and more is not a branch of the header
    for disp in [0i16, 3, -2] {
        let mut d = song91();
        code(&mut d, 8, enc::bcc_w(enc::BRA, disp));
        assert_eq!(family(&d), None, "{disp}");
    }
    // the short branch must skip 34 bytes and the move must store the register
    let mut d = song91();
    code(&mut d, 20, enc::bne_s(30));
    assert_eq!(family(&d), None);
    let mut d = song91();
    code(&mut d, 22, enc::move_w_d_abs(1, 14676136));
    assert_eq!(family(&d), None);
    // the 1993 header needs the register save and the call that loads the chips' address
    let d = module93(&[]);
    assert_eq!(family(&d), Some(Fam::Y93));
    let mut e = d.clone();
    code(&mut e, 96, enc::nop());
    code(&mut e, 98, enc::nop());
    assert_eq!(family(&e), None);
    let mut e = d.clone();
    code(&mut e, 128, enc::lea_abs(6, 14676000));
    assert_eq!(family(&e), None);
}

#[test]
fn the_first_tick_writes_only_the_init() {
    let d = song91();
    let mut p = p91(&d);
    let t = run(&mut p, 2);
    assert_eq!(t[0], vec![(DMACON, 15)]);
    assert!(t[1].len() >= 3);
}

#[test]
fn a_row_starts_the_note_then_the_effects_tick_turns_the_dma_on() {
    let d = song91();
    let mut p = p91(&d);
    let t = run(&mut p, 5);
    // tick 1 is the row: DMA off, the sample's address, its length
    let a = BASE + SMP as u32 + 10;
    assert_eq!(t[1], vec![(DMACON, 1), (reg(0, LOC_HI), (a >> 16) as u16), (reg(0, LOC_LO), a as u16), (reg(0, LEN), 2)]);
    // tick 2: DMA on, the volume ((63 * 32) / 64 = 31) and the period of note 5 (+ arpeggio 0)
    assert_eq!(t[2], vec![(DMACON, 32769), (reg(0, VOL), 31), (reg(0, PER), 150)]);
    // tick 3: no loop set: the silent loop, then volume and period
    let e = BASE + EMPTY as u32;
    assert_eq!(t[3], vec![(reg(0, LOC_HI), (e >> 16) as u16), (reg(0, LOC_LO), e as u16), (reg(0, LEN), 1), (reg(0, VOL), 31), (reg(0, PER), 150)]);
    // the next row (speed 3) starts after the pattern's end took the sequence step
    assert!(p.v[0].sp > p.lay.seq);
}

#[test]
fn the_other_voices_are_inactive() {
    let d = song91();
    let mut p = p91(&d);
    let t = run(&mut p, 4);
    for w in t.iter().flatten() {
        assert!(w.0 == DMACON || (reg(0, 0)..reg(1, 0)).contains(&w.0), "{w:?}");
    }
}

#[test]
fn the_sequence_loop_ends_the_song_and_the_pattern_restarts() {
    let d = song91();
    let mut p = p91(&d);
    run(&mut p, 2);
    assert!(!p.songend());
    // the pattern's end is met in the effects tick after the row; the loop entry ends the song
    run(&mut p, 1);
    assert!(p.songend());
    // and the pattern plays again: another DMA off
    let t = run(&mut p, 12);
    assert!(t.iter().flatten().any(|w| *w == (DMACON, 1)));
}

#[test]
fn a_loop_needs_every_voice_that_has_a_sequence() {
    let mut d = song91();
    // voice 1 has a sequence of its own: pattern 0, then a long run of the same pattern and no loop
    put32(&mut d, SONGS + 4, 40);
    d[SEQ + 40..SEQ + 46].copy_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut p = p91(&d);
    run(&mut p, 30);
    assert!(!p.songend());
}

#[test]
fn a_row_byte_of_128_or_more_stops_the_song_at_the_next_tick() {
    let d = module91(&[0, 0, 129], &[200], Opts::default());
    let mut p = p91(&d);
    let t = run(&mut p, 4);
    assert_eq!(p.m, 0);
    // the tick after the row finds the master volume 0: four volumes of 0, DMA off, song end
    assert_eq!(
        t[2],
        vec![(reg(0, VOL), 0), (reg(1, VOL), 0), (reg(2, VOL), 0), (reg(3, VOL), 0), (DMACON, 15)]
    );
    assert!(p.songend());
}

#[test]
fn row_commands() {
    // 100 3: speed 3 and the counter, 99 2: wait 2, then the note
    let d = module91(&[0, 0, 129], &[100, 7, 99, 2], Opts::default());
    let mut p = p91(&d);
    run(&mut p, 2);
    assert_eq!((p.v[0].speed, p.v[0].rtc, p.v[0].wait), (7, 7, 2));
    assert_eq!(p.v[0].pp, PATB + 4);
    // 98: release; 101 x sets the sustain level, 96 h l the slide (negated)
    let d = module91(&[0, 0, 129], &[98, 5, 0, 101, 9, 96, 1, 4], Opts::default());
    let mut p = p91(&d);
    run(&mut p, 2);
    assert_eq!(p.v[0].note, 5);
    assert_eq!(p.v[0].r[2], 9);
    assert_eq!(p.v[0].w(20), 65536u32.wrapping_sub(260) as u16);
    assert_eq!(p.v[0].pp, PATB + 8);
    // instrument 63 keeps the voice's instrument and starts nothing
    let d = module91(&[0, 0, 129], &[5, 63], Opts::default());
    let mut p = p91(&d);
    let t = run(&mut p, 2);
    assert_eq!(t[1], vec![]);
    assert_eq!(p.v[0].note, 5);
}

#[test]
fn the_release_envelope_runs_down_the_level() {
    // release 1: the level falls by 1 every other tick from 32; sustain level 0
    let mut d = module91(&[0, 0, 129], &[100, 100, 5, 0, 255], Opts::default());
    d[INS + 2] = 0;
    d[INS + 3] = 1;
    let mut p = p91(&d);
    let t = run(&mut p, 12);
    let vols: Vec<u16> = t.iter().map(|w| w.iter().find(|x| x.0 == reg(0, VOL)).map_or(0, |x| x.1)).collect();
    // the level 32 falls by 1 in each effects tick before the volume is written
    assert_eq!(vols[2], 30);
    assert!(vols[3..].windows(2).all(|w| w[1] <= w[0]) && vols[11] < 30, "{vols:?}");
}

#[test]
fn the_fade_command_counts_the_master_volume_down() {
    // 130 2 (build A): the master volume falls by 1 every 2 ticks, from 63; the song ends at 0
    let d = module91(&[130, 2, 0, 0, 129], &[5, 0, 255], Opts { fade: true, ..Opts::default() });
    let mut p = p91(&d);
    run(&mut p, 10);
    assert!(p.m < 63 && p.m > 50);
    assert!(p.f == 1 || p.f == 2);
    run(&mut p, 160);
    assert_eq!(p.m, 0);
    assert!(p.songend());
}

#[test]
fn the_loop_of_the_s_build_adds_the_byte_after_the_command() {
    // two entries; 129 with 2: back to the entry at +2 of the start
    let d = module91(&[0, 0, 0, 0, 129, 2], &[5, 0, 255], Opts { loop_off: true, ..Opts::default() });
    let mut p = p91(&d);
    run(&mut p, 12);
    assert!(p.songend());
    assert_eq!(p.v[0].sp, p.v[0].ss + 4);
    // without the offset the loop goes to the start
    let d = module91(&[0, 0, 0, 0, 129, 2], &[5, 0, 255], Opts::default());
    let mut p = p91(&d);
    run(&mut p, 12);
    assert!(p.songend());
    assert_eq!(p.v[0].sp, p.v[0].ss + 2);
}

#[test]
fn sequence_entries_with_a_repeat() {
    // an entry with the high bit in the pattern byte has a repeat count; the pattern plays 3 times
    let d = module91(&[0, 128, 3, 0, 0, 129], &[5, 0, 255], Opts::default());
    let mut p = p91(&d);
    run(&mut p, 2);
    assert_eq!(p.v[0].rep, 3);
    assert_eq!(p.v[0].sp, SEQ + 2 + 3);
    assert_eq!(p.v[0].ps, PATB);
}

#[test]
fn the_period_follows_the_arpeggio_and_the_transpose() {
    // arpeggio 0: 0, 4, then (128 marks the end) back to position 0
    let mut d = module91(&[7, 0, 129], &[100, 100, 5, 0, 255], Opts::default());
    d[ARPD..ARPD + 4].copy_from_slice(&[0, 4, 128, 0]);
    // transpose enabled: the voice adds the sequence's transpose (7)
    put16(&mut d, INS + 42, 65535);
    let mut p = p91(&d);
    let t = run(&mut p, 5);
    let per = |k: usize| t[k].iter().find(|x| x.0 == reg(0, PER)).unwrap().1;
    assert_eq!(per(2), 100 + 10 * 12);
    assert_eq!(per(3), 100 + 10 * 16);
    assert_eq!(per(4), 100 + 10 * 12);
}

#[test]
fn rows_on_voice_three_only_in_the_n_build() {
    let mut d = module91(&[0, 0, 129], &[5, 0, 255], Opts { only3: true, ..Opts::default() });
    put32(&mut d, SONGS, 0);
    put32(&mut d, SONGS + 12, 2);
    let mut p = p91(&d);
    let t = run(&mut p, 3);
    assert!(t[1].contains(&(DMACON, 8)));
    assert!(!t.iter().flatten().any(|w| w.0 == reg(0, VOL)));
}

// ------------------------------------------------------------------ the 1993 family

const D_DUR: usize = 176;
const D_TRANS: usize = 180;
const D_IMAP: usize = 192;
const D_PT: usize = 256;
const D_SONGT: usize = D_PT + 576;
const SBASE: u32 = 196608;

/// A 1993 module of one song: the position list [0], pattern 0 with the four streams
/// `s0`..`s3`.
fn module93(s: &[&[u8]]) -> Vec<u8> {
    let mut d = vec![0u8; D_SONGT + 256];
    for i in 0..7 {
        code(&mut d, 4 * i, enc::bcc_w(enc::BRA, 94 - 4 * i as i16));
    }
    code(&mut d, 96, enc::movem_l_save_all());
    code(&mut d, 100, enc::bcc_w(enc::BSR, 26));
    code(&mut d, 128, enc::lea_abs(6, 14675968));
    let mut a = 144;
    a = code(&mut d, a, enc::move_b_pcidx_disp(0, a, D_DUR, 1, 17));
    a = code(&mut d, a, enc::add_w_pcidx_d(0, a, D_TRANS, 4));
    a = code(&mut d, a, enc::asl_w_imm(4, 1));
    code(&mut d, a, enc::lea_pc_at(2, a, D_IMAP));
    d[D_DUR..D_DUR + 3].copy_from_slice(&[0, 1, 3]);
    d[D_IMAP + 1] = 0;
    for r in 0..8 {
        for (i, p) in PT_ROW.iter().enumerate() {
            put16(&mut d, D_PT + 72 * r + 2 * i, *p);
        }
    }
    // the song table: one song, the position list at +4, the pattern table at +8
    let t = D_SONGT;
    put16(&mut d, t, 4);
    put16(&mut d, t + 2, 8);
    d[t + 4] = 1;
    d[t + 5] = 0;
    // the streams from +32 on, 16 bytes each; the pattern table's words are relative to +8
    for c in 0..4 {
        let at = t + 32 + 16 * c;
        put16(&mut d, t + 8 + 2 * c, (at - (t + 8)) as u16);
        let st: &[u8] = s.get(c).copied().unwrap_or(&REST);
        d[at..at + st.len()].copy_from_slice(st);
    }
    d
}

/// A stream of rests and then the end of the pattern.
const REST: [u8; 13] = [2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 255];

fn sample_file() -> Vec<u8> {
    let mut f = vec![0u8; 128];
    // sample 1: start 64, loop 72, 4 words, loop of 2, volume 40, finetune 0
    put32(&mut f, 0, 64);
    put32(&mut f, 4, 72);
    put16(&mut f, 8, 4);
    put16(&mut f, 10, 2);
    f[13] = 40;
    f
}

fn p93(d: &[u8]) -> Ash93 {
    Ash93::new(d, locate93(d).unwrap(), &sample_file(), SBASE)
}

/// Instrument 1, then a note of 10 with k = 1.
const NOTE: [u8; 4] = [241, 41, 2, 255];

#[test]
fn the_1993_tables_are_found_by_instruction_forms() {
    let d = module93(&[]);
    assert!(detect(&d));
    assert_eq!(locate93(&d).unwrap(), Lay93 { dur: D_DUR, trans: D_TRANS, imap: D_IMAP, pt: D_PT, songt: D_SONGT, nsongs: 1 });
    // without the shift left by 4 before the address load there is no IMAP
    let mut e = d.clone();
    let at = (144..200).step_by(2).find(|&o| matches!(decode(&e, o), Some(i) if i.k == K::Shift && i.data == 4)).unwrap();
    code(&mut e, at, enc::nop());
    assert!(locate93(&e).is_none());
    // a period table with another first row is no PT
    let mut e = d.clone();
    for r in 0..8 {
        put16(&mut e, D_PT + 72 * r, 857);
    }
    assert!(!detect(&e));
}

#[test]
fn the_first_1993_tick_writes_the_init_and_the_volumes_and_periods() {
    let d = module93(&[]);
    let mut p = p93(&d);
    let t = run(&mut p, 1);
    let mut want = vec![(DMACON, 15), (DMACON, 15)];
    for c in 0..4 {
        want.push((reg(c, VOL), 64));
        want.push((reg(c, PER), 856));
    }
    assert_eq!(t[0], want);
}

#[test]
fn a_note_starts_in_three_stages() {
    let d = module93(&[&NOTE]);
    let mut p = p93(&d);
    let t = run(&mut p, 3);
    // tick 1: the rows of voices 0-2, the effects of voice 3 only
    assert_eq!(t[1], vec![(reg(3, VOL), 64), (reg(3, PER), 856)]);
    // tick 2: voice 3's row, then the trigger (DMA off, address and length, DMA on, loop), then
    // the effects of all voices: voice 0 has the sample's default volume and note 10
    let s = SBASE + 64;
    let l = SBASE + 72;
    let mut want = vec![
        (DMACON, 1),
        (reg(0, LOC_HI), (s >> 16) as u16),
        (reg(0, LOC_LO), s as u16),
        (reg(0, LEN), 4),
        (DMACON, 32769),
        (reg(0, LOC_HI), (l >> 16) as u16),
        (reg(0, LOC_LO), l as u16),
        (reg(0, LEN), 2),
        (reg(0, VOL), 40),
        (reg(0, PER), PT_ROW[10]),
    ];
    for c in 1..4 {
        want.push((reg(c, VOL), 64));
        want.push((reg(c, PER), 856));
    }
    assert_eq!(t[2], want);
}

#[test]
fn the_stages_are_spaced_by_nine_raster_lines() {
    let d = module93(&[&NOTE]);
    let mut p = p93(&d);
    let mut bus = Bus::new();
    bus.write_log = Some(Vec::new());
    for k in 0..3u64 {
        bus.clock = (k + 1) * TICK;
        p.tick(&mut Hw { bus: &mut bus });
    }
    let l = bus.write_log.take().unwrap();
    let at = |a: u32, v: u16| l.iter().rev().find(|e| e.0 == a && e.1 == v).unwrap().2;
    let off = at(DMACON, 1);
    let on = at(DMACON, 32769);
    let first_loc = l.iter().rev().find(|e| e.0 == reg(0, LOC_LO) && e.1 == (SBASE + 64) as u16).unwrap().2;
    let loop_loc = l.iter().rev().find(|e| e.0 == reg(0, LOC_LO) && e.1 == (SBASE + 72) as u16).unwrap().2;
    assert!(first_loc - off >= 9 * 227 - 227 && first_loc - off < 11 * 227, "{}", first_loc - off);
    assert!(loop_loc - on >= 8 * 227 && loop_loc - on < 11 * 227, "{}", loop_loc - on);
}

#[test]
fn the_1993_song_end_is_the_position_wrap() {
    let d = module93(&[&[255]]);
    let mut p = p93(&d);
    run(&mut p, 1);
    assert!(!p.songend());
    run(&mut p, 1);
    // the 255 at the start of a stream: position 1 = the count: wrap to 0 and song end
    assert!(p.songend());
    assert_eq!(p.pos, 0);
}

#[test]
fn a_volume_byte_sets_the_volume_of_the_voice() {
    // 190 = 184 + 6: effect 6 with parameter 6; the voice keeps its volume at the trigger,
    // then the effect sets it
    let d = module93(&[&[241, 190, 41, 255]]);
    let mut p = p93(&d);
    let t = run(&mut p, 4);
    assert_eq!(p.c[0].et, 6);
    assert!(t[2].contains(&(reg(0, VOL), 6)));
}

#[test]
fn a_back_reference_replays_earlier_bytes() {
    // the dictionary 16 bytes before the stream: a speed command and rests; the stream jumps there
    let mut d = module93(&[&[148, 10, 255]]);
    let st = D_SONGT + 32;
    d[st - 16..st - 10].copy_from_slice(&[253, 9, 2, 2, 2, 2]);
    let mut p = p93(&d);
    p.setup();
    p.row(0);
    // 148: 6 bytes from the dictionary: 253 9 (speed), then the rest 2 (k = 2: wait 3); the row
    // ends there, inside the reference
    assert_eq!(p.speed, 9);
    assert_eq!(p.c[0].w, 3);
    assert!(p.c[0].br);
    assert_eq!(p.c[0].bc, 6 - 3);
    // the other bytes of the reference are skipped to when it is read on
    p.c[0].w = 0;
    p.row(0);
    p.c[0].w = 0;
    p.row(0);
    p.c[0].w = 0;
    p.row(0);
    assert!(!p.c[0].br);
}

#[test]
fn a_position_jump_to_itself_is_cut_off() {
    // 254 0: position 0 again, which loads the same streams and meets the jump again
    let d = module93(&[&[254, 0, 241, 41, 255]]);
    let mut p = p93(&d);
    p.setup();
    p.row(0);
    assert_eq!(p.pos, 0);
    assert_eq!(p.c[0].inn, 0);
    assert!(!p.songend());
}

#[test]
fn effects_two_and_one_move_the_period_offset() {
    // 217 + 4 = 221: effect type 2 (e / 4 + 1), duration bits 1, parameter byte 8 (delta 0)
    let d = module93(&[&[221, 8 + 64, 41, 255]]);
    let mut p = p93(&d);
    p.setup();
    p.row(0);
    assert_eq!((p.c[0].et, p.c[0].ep, p.c[0].edl), (2, 8, 0));
    let before = p.c[0].po;
    let mut bus = Bus::new();
    p.effects(0, &mut Hw { bus: &mut bus });
    // the first effects tick after the row sets the flag back only
    assert_eq!(p.c[0].po, before);
    p.effects(0, &mut Hw { bus: &mut bus });
    assert_eq!(p.c[0].po, before + 8);
}

#[test]
fn companion_names() {
    assert_eq!(companion("ash.Cosmic_Spacehead").as_deref(), Some("smp.Cosmic_Spacehead"));
    assert_eq!(companion("/x/y/ASH.Name").as_deref(), Some("smp.Name"));
    assert_eq!(companion("a:ash.x").as_deref(), Some("smp.x"));
    assert_eq!(companion("xsh.Name"), None);
    assert_eq!(companion("ash"), None);
}

#[test]
fn the_replayers_list_has_this_one() {
    assert!(super::super::REPLAYERS.iter().any(|r| r.name == "Ashley Hogg"));
}

/// `ASH_LIST=file ASH_NAMES=... cargo test --release ashley_claims -- --ignored --nocapture`: for each
/// path in the list file, the first and all open replayers that recognise it (tab separated).
#[test]
#[ignore]
fn ashley_claims() {
    let Ok(list) = std::env::var("ASH_LIST") else { return };
    for path in std::fs::read_to_string(list).unwrap().lines() {
        let Ok(d) = std::fs::read(path) else { continue };
        let hit: Vec<&str> = super::super::REPLAYERS.iter().filter(|r| (r.detect)(&d)).map(|r| r.name).collect();
        println!("CLAIM\t{}\t{}\t{}", path, hit.first().copied().unwrap_or(""), hit.join(","));
    }
}
