//! Tests on synthetic modules (built here, no corpus bytes): the locator on its
//! instruction forms, the detector's refusals and the tick procedure.

use super::*;
use crate::bus::Bus;

const ORIGIN: u32 = 0x10000;
const PERIOD: usize = 0x100;
const PATTERN: usize = 0x200;
const INST: usize = 0x240;
const PITCH: usize = 0x280;
const SEQ: usize = 0x2C0;
const SUBS: usize = 0x300;
const ORDER: usize = 0x340;
const CELLS: usize = 0x380;
const RECORD: usize = 0x400;
const SAMPLE: usize = 0x500;
const LOOP: usize = 0x540;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A small encoder of the instruction forms the locator matches, from their descriptions
/// (sizes and registers), so the tests carry no literal instruction words.
mod enc {
    const ABS_L: u16 = 7 << 3 | 1;
    const IMM: u16 = 7 << 3 | 4;
    /// Size field of the move group: byte 1, long 2.
    const MB: u16 = 1;
    const ML: u16 = 2;

    fn l(v: u32) -> [u16; 2] {
        [(v >> 16) as u16, v as u16]
    }

    /// `andi.b #imm,abs.l`.
    pub fn andi_b_abs(imm: u8, addr: u32) -> Vec<u16> {
        [&[1 << 9 | ABS_L, imm as u16][..], &l(addr)].concat()
    }
    /// `bne.s` with a displacement.
    pub fn bne_s(disp: i8) -> Vec<u16> {
        vec![6 << 12 | 6 << 8 | disp as u8 as u16]
    }
    pub fn rts() -> Vec<u16> {
        vec![0x4E << 8 | 0x75]
    }
    pub fn nop() -> Vec<u16> {
        vec![0x4E << 8 | 0x71]
    }
    /// `moveq #imm,dn`.
    pub fn moveq(n: u16, imm: u8) -> Vec<u16> {
        vec![7 << 12 | n << 9 | imm as u16]
    }
    /// `move.b abs.l,dn`.
    pub fn move_b_abs_d(n: u16, addr: u32) -> Vec<u16> {
        [&[MB << 12 | n << 9 | ABS_L][..], &l(addr)].concat()
    }
    /// `tst.b abs.l`.
    pub fn tst_b_abs(addr: u32) -> Vec<u16> {
        [&[0x4A << 8 | ABS_L][..], &l(addr)].concat()
    }
    /// `move.b dm,dn`.
    pub fn move_b_dd(m: u16, n: u16) -> Vec<u16> {
        vec![MB << 12 | n << 9 | m]
    }
    /// `asl.b #count,dn`.
    pub fn asl_b_imm(count: u16, n: u16) -> Vec<u16> {
        vec![0xE << 12 | count << 9 | 1 << 8 | n]
    }
    /// `lsr.b #count,dn`.
    pub fn lsr_b_imm(count: u16, n: u16) -> Vec<u16> {
        vec![0xE << 12 | count << 9 | 1 << 3 | n]
    }
    /// `cmpi.b #imm,dn`.
    pub fn cmpi_b_d(imm: u8, n: u16) -> Vec<u16> {
        vec![6 << 9 | n, imm as u16]
    }
    /// `cmp.b #imm,dn`.
    pub fn cmp_b_imm_d(imm: u8, n: u16) -> Vec<u16> {
        vec![0xB << 12 | n << 9 | IMM, imm as u16]
    }
    /// `movea.l #imm,an`.
    pub fn movea_l_imm(n: u16, imm: u32) -> Vec<u16> {
        [&[ML << 12 | n << 9 | 1 << 6 | IMM][..], &l(imm)].concat()
    }
    /// `lea abs.l,an`.
    pub fn lea_abs(n: u16, addr: u32) -> Vec<u16> {
        [&[0x4 << 12 | n << 9 | 7 << 6 | ABS_L][..], &l(addr)].concat()
    }
    /// `move.l (d8,an,d0.w),abs.l`.
    pub fn move_l_idx_abs(n: u16, d8: u8, addr: u32) -> Vec<u16> {
        [&[ML << 12 | 1 << 9 | 7 << 6 | 6 << 3 | n, d8 as u16][..], &l(addr)].concat()
    }
    /// `move.b #imm,abs.l`.
    pub fn move_b_imm_abs(imm: u8, addr: u32) -> Vec<u16> {
        [&[MB << 12 | 1 << 9 | 7 << 6 | IMM, imm as u16][..], &l(addr)].concat()
    }
}

/// Writes the words of an instruction at `o`.
fn code(d: &mut [u8], o: usize, ws: Vec<u16>) {
    for (i, w) in ws.iter().enumerate() {
        put16(d, o + 2 * i, *w);
    }
}

fn link(o: usize) -> u32 {
    ORIGIN + o as u32
}

/// A module in the shape of build class A: the signature at 0, the five key forms,
/// and one song whose four voices play `order` (pattern 0 is one instrument cell
/// of duration 1: instrument 0, note 12).
fn module(order: &[u8], cell: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 2000];
    // andi.b #1,abs.l ; bne.s +0x26 ; rts ; (target) moveq #0,d4 / move.b abs.l,d4
    code(&mut d, 0, enc::andi_b_abs(1, 0x100));
    code(&mut d, 8, enc::bne_s(0x26));
    code(&mut d, 10, enc::rts());
    code(&mut d, 0x30, enc::moveq(4, 0));
    code(&mut d, 0x32, enc::move_b_abs_d(4, 0x200));
    // K1: move.b d0,d2 ; asl.b #1,d2 ; nop ; the period table's link at +6
    code(&mut d, 0x40, enc::move_b_dd(0, 2));
    code(&mut d, 0x42, enc::asl_b_imm(1, 2));
    code(&mut d, 0x44, enc::nop());
    put32(&mut d, 0x46, link(PERIOD));
    // K3: cmpi.b #255,d1, then (6 bytes on) four byte moves of immediates to absolute longs
    code(&mut d, 0x50, enc::cmpi_b_d(255, 1));
    for i in 0..4 {
        code(&mut d, 0x56 + 8 * i, enc::move_b_imm_abs(0, 0x300));
    }
    // K4: movea.l #pattern table,a3
    code(&mut d, 0x78, enc::movea_l_imm(3, link(PATTERN)));
    // K5: lea instrument table,a4
    code(&mut d, 0x80, enc::lea_abs(4, link(INST)));
    // K6: the word 38, then movea.l #pitch table,a3
    put16(&mut d, 0x88, 38);
    code(&mut d, 0x8A, enc::movea_l_imm(3, link(PITCH)));
    // the song table's link, then K7: move.l 8(a4,d0.w),abs.l
    put32(&mut d, 0x92, link(SUBS));
    code(&mut d, 0x96, enc::move_l_idx_abs(4, 8, 0x200));
    // period table, pattern table, instrument table, pitch table and its sequence
    for (i, p) in PERIODS.iter().enumerate() {
        put16(&mut d, PERIOD + 2 * i, *p);
    }
    put32(&mut d, PATTERN, link(CELLS));
    put32(&mut d, INST, link(RECORD));
    put32(&mut d, PITCH, link(SEQ));
    d[SEQ..SEQ + 3].copy_from_slice(&[0, 0, 0xFF]);
    // one song (speed 0: a row every two ticks) and an end entry
    for v in 0..4 {
        put32(&mut d, SUBS + 4 * v, link(ORDER + 0x10 * v));
        d[ORDER + 0x10 * v..ORDER + 0x10 * v + order.len()].copy_from_slice(order);
    }
    d[SUBS + 16] = 12;
    d[SUBS + 17] = 0;
    d[CELLS..CELLS + cell.len()].copy_from_slice(cell);
    // the instrument: peak 32, 4 words of sample, loop of 2 words, attack 8, decay 1, sustain 16, release 2
    for (o, v) in [(0, 32u16), (2, 4), (12, 2), (14, 8), (16, 1), (18, 16), (20, 2)] {
        put16(&mut d, RECORD + o, v);
    }
    put32(&mut d, RECORD + 4, link(SAMPLE));
    put32(&mut d, RECORD + 8, link(LOOP));
    d
}

fn song() -> Vec<u8> {
    module(&[0x00, 0xFF], &[0x81, 0x00, 0x0C, 0xFF])
}

fn player(d: &[u8]) -> JeroenTel {
    JeroenTel {
        d: d.to_vec(),
        lay: locate(d).unwrap(),
        base: 0x20000,
        silent: 0x8000,
        sub: 0,
        started: false,
        songend: false,
        enabled: false,
        t0: 0,
        notes: 0,
        loops: 0,
        bb: 0,
        bc: 0,
        v: [Voice::default(); 4],
    }
}

/// Runs `n` ticks and returns each tick's writes (address, value).
fn run(p: &mut JeroenTel, n: usize) -> Vec<Vec<(u32, u16)>> {
    let mut bus = Bus::new();
    bus.write_log = Some(Vec::new());
    let mut out = Vec::new();
    for k in 0..n {
        bus.clock = (k as u64 + 1) * 70940;
        p.tick(&mut Hw { bus: &mut bus });
        out.push(bus.write_log.take().unwrap().into_iter().map(|(a, v, _)| (a, v)).collect());
        bus.write_log = Some(Vec::new());
    }
    out
}

#[test]
fn locator_finds_the_tables_by_instruction_forms() {
    let d = song();
    assert!(signature(&d));
    let l = locate(&d).unwrap();
    assert_eq!(l, Lay { origin: ORIGIN, period: PERIOD, repeat: [0; 4], pattern: PATTERN, inst: INST, pitch: PITCH, subtab: SUBS, subs: 1 });
    assert!(detect(&d));
}

#[test]
fn locator_names_the_failing_key() {
    // without the shift after the byte move, K1 is missing
    let mut d = song();
    code(&mut d, 0x42, enc::nop());
    assert_eq!(locate(&d), Err("K1"));
    // a logical shift or one by 2 is not the key either
    code(&mut d, 0x42, enc::lsr_b_imm(1, 2));
    assert_eq!(locate(&d), Err("K1"));
    code(&mut d, 0x42, enc::asl_b_imm(2, 2));
    assert_eq!(locate(&d), Err("K1"));
    // a compare of d2 instead of d1 is not K3
    let mut d = song();
    code(&mut d, 0x50, enc::cmpi_b_d(255, 2));
    assert_eq!(locate(&d), Err("K3"));
    // the CMP form of K3 is accepted: cmp.b #255,d1
    let mut d = song();
    code(&mut d, 0x50, enc::cmp_b_imm_d(255, 1));
    assert!(locate(&d).is_ok());
    // a lea into a5 is not K5
    let mut d = song();
    code(&mut d, 0x80, enc::lea_abs(5, link(INST)));
    assert_eq!(locate(&d), Err("K5"));
}

#[test]
fn repeat_indices_come_from_the_move_after_the_compare() {
    // the four immediates are at +9, +17, +25, +33 of K3
    let mut d = song();
    for (i, r) in [7u8, 5, 8, 8].iter().enumerate() {
        d[0x50 + 9 + 8 * i] = *r;
    }
    let l = locate(&d).unwrap();
    assert_eq!(l.repeat, [7, 5, 8, 8]);
}

#[test]
fn detection_refuses_other_files() {
    assert!(!detect(&[0u8; 3000]));
    let mut d = song();
    d.truncate(1700);
    assert!(!detect(&d));
    // the signature without its branch and return
    let mut d = song();
    code(&mut d, 0x0A, enc::nop());
    assert!(!detect(&d));
    // a branch displacement of 0x80 or more
    let mut d = song();
    code(&mut d, 8, enc::bne_s(-128));
    assert!(!detect(&d));
    // the signature and branch, but nothing at the target
    let mut d = song();
    code(&mut d, 0x30, enc::nop());
    code(&mut d, 0x32, enc::nop());
    assert!(!detect(&d));
    // a 4-chunk chain of tests (18 bytes apart) before the load is accepted
    let mut d = song();
    for i in 0..4 {
        code(&mut d, 0x30 + 18 * i, enc::tst_b_abs(0x200));
    }
    code(&mut d, 0x30 + 72, enc::moveq(4, 0));
    code(&mut d, 0x30 + 74, enc::move_b_abs_d(4, 0x200));
    assert!(signature(&d));
}

#[test]
fn the_first_tick_writes_the_silent_loop_for_each_voice() {
    let d = song();
    let mut p = player(&d);
    let t = run(&mut p, 1);
    // voices 3, 2, 1, 0: address (high, low) and length
    for (i, v) in [3u32, 2, 1, 0].iter().enumerate() {
        let r = 0xDFF0A0 + 16 * v;
        assert_eq!(&t[0][3 * i..3 * i + 3], &[(r, 0), (r + 2, 0x8000), (r + 4, 0)]);
    }
    // then stage 2: period, volume, DMA on, per voice
    assert_eq!(&t[0][12..15], &[(0xDFF0D6, 856), (0xDFF0D8, 0), (0xDFF096, 0x8008)]);
    assert_eq!(t[0].len(), 24);
}

#[test]
fn a_row_starts_the_note() {
    let d = song();
    let mut p = player(&d);
    let t = run(&mut p, 3);
    // tick 2 is the first row: DMA off, the sample's address and length, for voices 3..0
    let w = &t[2];
    let v0: Vec<_> = w.iter().filter(|(a, _)| (0xDFF0A0..0xDFF0B0).contains(a) || *a == 0xDFF096).cloned().collect();
    // voice 0: DMA off (1), address 0x20000 + 0x500, length 4, then its stage 2: period 428,
    // volume 8 (one attack step), DMA on
    assert!(v0.contains(&(0xDFF096, 1)));
    assert!(v0.contains(&(0xDFF0A0, 2)));
    assert!(v0.contains(&(0xDFF0A2, SAMPLE as u16)));
    assert!(v0.contains(&(0xDFF0A4, 4)));
    assert!(v0.contains(&(0xDFF0A6, 428)));
    assert!(v0.contains(&(0xDFF0A8, 8)));
    assert!(v0.contains(&(0xDFF096, 0x8001)));
    // the note start's DMA off comes before the address, which comes before the length
    let pos = |x: (u32, u16)| w.iter().position(|e| *e == x).unwrap();
    assert!(pos((0xDFF096, 1)) < pos((0xDFF0A0, 2)) && pos((0xDFF0A2, SAMPLE as u16)) < pos((0xDFF0A4, 4)));
}

#[test]
fn the_envelope_attacks_decays_to_the_sustain_and_the_loop_is_set_up() {
    let d = song();
    let mut p = player(&d);
    let t = run(&mut p, 12);
    let vols: Vec<u16> = t.iter().map(|w| w.iter().find(|(a, _)| *a == 0xDFF0A8).map_or(0, |e| e.1)).collect();
    // tick 2: the attack step applied at the note start; later ticks add it on even tick counters
    assert_eq!(vols[2], 8);
    assert!(vols[3..].iter().all(|&v| v <= 32));
    // the first effects step after the note start writes the loop (address then length)
    assert!(t[3].contains(&(0xDFF0A2, LOOP as u16)));
    assert!(t[3].contains(&(0xDFF0A4, 2)));
}

#[test]
fn the_period_table_clamps_high_notes() {
    let d = song();
    let p = player(&d);
    assert_eq!(p.period_of(0), 856);
    assert_eq!(p.period_of(35), 114);
    // 36 and above select the last entry; the note is masked to 7 bits
    assert_eq!(p.period_of(36), 114);
    assert_eq!(p.period_of(0x7F), 114);
    assert_eq!(p.period_of(0x80), 856);
}

#[test]
fn a_stop_byte_switches_every_channel_off_and_ends_the_writes() {
    let d = module(&[0xFE], &[0x81, 0x00, 0x0C, 0xFF]);
    let mut p = player(&d);
    let t = run(&mut p, 5);
    assert!(t[2].contains(&(0xDFF096, 0x000F)));
    assert!(t[3].is_empty() && t[4].is_empty());
}

#[test]
fn the_order_list_end_raises_the_song_end_and_restarts() {
    let d = song();
    let mut p = player(&d);
    // the cell has duration 1: two row periods of two ticks; the FF is read at the next row
    run(&mut p, 3);
    assert!(!p.songend());
    run(&mut p, 4);
    assert!(p.songend());
    // and the song goes on from the repeat index
    let t = run(&mut p, 4);
    assert!(t.iter().any(|w| w.contains(&(0xDFF096, 1))));
}

#[test]
fn transposition_bytes_and_the_slide_cell() {
    // 0xC0 + 2: a transposition of +2, then pattern 0 with a slide cell: speed 4, target 14, note 12
    let mut d = module(&[0xC2, 0x00, 0xFF], &[0x41, 0x04, 0x0E, 0x0C, 0xFF]);
    // the pitch sequence plays its note once, then stops (the period is then left to the slide)
    d[SEQ..SEQ + 3].copy_from_slice(&[0, 0, 0xFE]);
    let mut p = player(&d);
    run(&mut p, 3);
    // the cell sets transposition-adjusted notes: note 14, target 16
    assert_eq!(p.v[0].note, 14);
    assert_eq!(p.v[0].target, 16);
    assert_eq!(p.v[0].transp, 2);
    // the slide moves the period towards the target's (the sliding voice's) period, up in pitch
    let before = p.v[0].period;
    run(&mut p, 1);
    assert!(p.v[0].period < before || p.v[0].slide == 0);
}

#[test]
fn the_subsong_table_ends_at_a_wrong_start_voice_or_equal_addresses() {
    let mut d = song();
    // a second entry whose start-voice byte is not 12 ends the list
    for v in 0..4 {
        put32(&mut d, SUBS + 18 + 4 * v, link(ORDER) + v as u32);
    }
    d[SUBS + 18 + 16] = 3;
    assert_eq!(locate(&d).unwrap().subs, 1);
    d[SUBS + 18 + 16] = 12;
    assert_eq!(locate(&d).unwrap().subs, 2);
    // all four addresses equal: not an entry
    for v in 0..4 {
        put32(&mut d, SUBS + 18 + 4 * v, link(ORDER));
    }
    assert_eq!(locate(&d).unwrap().subs, 1);
}

#[test]
fn the_replayers_list_ends_with_this_one() {
    assert_eq!(super::super::REPLAYERS.last().unwrap().name, "Jeroen Tel");
}
