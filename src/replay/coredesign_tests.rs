//! Tests on synthetic modules (built here, no corpus bytes): the container, the
//! locator on its instruction forms, the refusals, and the tick and interrupt procedures.

use super::*;
use crate::replay::m68kenc as enc;

const SUBPOS: usize = 1000;
const SMP: usize = 1100;
const SEQ: usize = 1200;
const PATTAB: usize = 1300;
const PATBASE: usize = 1400;
const VIB: usize = 1500;
const SIL: usize = 1600;
const CHAN: usize = 700;
const PER: usize = 800;
const REC: usize = 1900;
const LEN: usize = 2200;
const BASE: u32 = 0x2_0000;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// Puts an instruction (image offset `o`) and returns the offset after it.
fn code(img: &mut [u8], o: usize, ws: Vec<u16>) -> usize {
    for (i, w) in ws.iter().enumerate() {
        put16(img, o + 2 * i, *w);
    }
    o + 2 * ws.len()
}

/// `bsr` from `o` to `target`.
fn bsr(img: &mut [u8], o: usize, target: usize) -> usize {
    code(img, o, enc::bcc_w(enc::BSR, (target as isize - o as isize - 2) as i16))
}

/// `lea target(pc),an` at `o`.
fn lea_pc(img: &mut [u8], o: usize, n: u16, target: usize) -> usize {
    code(img, o, enc::lea_pc(n, (target as isize - o as isize - 2) as i16))
}

#[derive(Clone, Copy)]
struct Opt {
    family: Family,
    channel3: bool,
    skip0: bool,
    noteadd: u8,
    /// The bit the output routine tests.
    flag_bit: u8,
}

fn opt(family: Family) -> Opt {
    Opt { family, channel3: false, skip0: false, noteadd: 12, flag_bit: if family == Family::R { 1 } else { 6 } }
}

/// The module image: header, the five routines, the tables, one song (a 3-tick note
/// on track 0, the other tracks ended) and one effect on channel 1.
fn image_bytes(o: Opt) -> Vec<u8> {
    let mut m = vec![0u8; LEN];
    let (start, play, init, seq, out, irq) = (100, 200, 300, 400, 500, 600);
    code(&mut m, 0, enc::moveq(0, 0xFF));
    code(&mut m, 2, enc::rts());
    m[4..12].copy_from_slice(b"S.PHIPPS");
    for (i, v) in [play, irq, start, 150, 2, SMP, SMP + 14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].iter().enumerate() {
        put32(&mut m, 12 + 4 * i, *v as u32);
    }
    // subsong start
    let mut p = code(&mut m, start, enc::lea_abs(0, SUBPOS as u32));
    p = code(&mut m, p, enc::lea_abs(6, CHAN as u32));
    p = code(&mut m, p, enc::cmpi_w_ind(1, 0));
    let beq = p;
    p = code(&mut m, p, enc::beq_s(0));
    p = bsr(&mut m, p, init);
    p = code(&mut m, p, enc::rts());
    let eff = p;
    m[beq + 1] = (eff - beq - 2) as u8;
    if o.channel3 {
        p = code(&mut m, p, enc::tst_b_d(1));
    }
    if o.skip0 {
        p = code(&mut m, p, enc::move_b_imm_d(1, 1));
    }
    code(&mut m, p, enc::rts());
    // play: output, subsong start, sequencer
    let mut p = bsr(&mut m, play, out);
    p = bsr(&mut m, p, start);
    p = bsr(&mut m, p, seq);
    code(&mut m, p, enc::rts());
    // song init: three track records, the sequence start table, the sample table
    let mut p = init;
    for t in [900, 934, 968, SEQ, SMP] {
        p = lea_pc(&mut m, p, 1, t);
    }
    code(&mut m, p, enc::rts());
    // sequencer: sixteen loads, the note offset
    let mut p = seq;
    for t in [890, 900, 1904, 900, 1934, 934, 1964, 968, 1000, PATTAB, PATBASE, PER, 1000, SMP, VIB, PER] {
        p = lea_pc(&mut m, p, 0, t);
    }
    p = code(&mut m, p, enc::addi_b_d(o.noteadd, 0));
    code(&mut m, p, enc::rts());
    // output routine: the flag test, the silent block store
    let p = code(&mut m, out, enc::btst_imm_disp(o.flag_bit, 6, 28));
    let p = code(&mut m, p, enc::move_l_imm_disp(SIL as u32, 4, 160));
    code(&mut m, p, enc::rts());
    // interrupt routine
    let mut p = irq;
    if o.family == Family::B {
        p = code(&mut m, p, enc::lsr_w_imm(7, 0));
    }
    p = code(&mut m, p, enc::move_l_imm_disp(SIL as u32, 0, 14));
    code(&mut m, p, enc::rte());
    // subsong table: song 0 (sequence 0), effect: sample 0, period 300, selector 1
    for (i, w) in [0u16, 0, 0, 0, 1, 0, 300, 1].iter().enumerate() {
        put16(&mut m, SUBPOS + 2 * i, *w);
    }
    // sample 0: attack 20, sustain 10, peak 60, flags 1 (hold/silence), record REC
    m[SMP..SMP + 6].copy_from_slice(&[20, 0, 10, 0, 60, 1]);
    put32(&mut m, SMP + 6, REC as u32);
    put16(&mut m, REC, 4);
    // sequence start table for sequence 0
    for (i, w) in [16u16, 20, 20].iter().enumerate() {
        put16(&mut m, SEQ + 2 * i, *w);
    }
    m[SEQ + 16..SEQ + 18].copy_from_slice(&[0, 255]);
    m[SEQ + 20] = 255;
    put16(&mut m, PATTAB, 16);
    m[PATBASE + 16..PATBASE + 19].copy_from_slice(&[12, 3, 255]);
    for i in 0..85 {
        put16(&mut m, PER + 2 * i, 1000 + i as u16);
    }
    m
}

/// The load file around an image.
fn file(img: &[u8]) -> Vec<u8> {
    let n = img.len() / 4;
    let mut d = Vec::new();
    for v in [1011u32, 0, 1, 0, 0, 0x4000_0000 | n as u32, 1001, n as u32] {
        d.extend_from_slice(&v.to_be_bytes());
    }
    d.extend_from_slice(img);
    for v in [1004u32, 1, 0, 0, 0, 1010] {
        d.extend_from_slice(&v.to_be_bytes());
    }
    d
}

#[test]
fn locator_reads_each_family_and_the_effect_rules() {
    for fam in [Family::A, Family::R, Family::B] {
        let o = opt(fam);
        let d = file(&image_bytes(o));
        assert!(detect(&d));
        let lay = locate(image(&d).unwrap()).unwrap();
        assert_eq!(lay.family, fam);
        assert_eq!((lay.sub, lay.seq, lay.pattab, lay.patbase, lay.per, lay.vib, lay.smp, lay.sil, lay.noteadd), (SUBPOS, SEQ, PATTAB, PATBASE, PER, VIB, SMP, SIL, 12));
        assert_eq!(lay.sfx, Sfx::Selector { skip0: false });
        assert_eq!((lay.subs, lay.disable), (2, None));
    }
    let mut o = opt(Family::B);
    o.skip0 = true;
    o.noteadd = 36;
    let lay = locate(&image_bytes(o)[..]).unwrap();
    assert_eq!((lay.sfx, lay.noteadd), (Sfx::Selector { skip0: true }, 36));
    let mut o = opt(Family::R);
    o.channel3 = true;
    assert_eq!(locate(&image_bytes(o)[..]).unwrap().sfx, Sfx::Channel3);
}

#[test]
fn refusals() {
    let good = file(&image_bytes(opt(Family::A)));
    assert!(detect(&good));
    // not a load file / wrong signature / not chip / extra hunk table / truncated
    let mut d = good.clone();
    d[3] ^= 1;
    assert!(!detect(&d));
    let mut d = good.clone();
    d[32 + 4] = b'X';
    assert!(!detect(&d));
    let mut d = good.clone();
    d[20] = 0;
    assert!(!detect(&d));
    assert!(!detect(&good[..good.len() - 40]));
    assert!(!detect(&good[..64]));
    // a zero header field
    let mut d = good.clone();
    put32(&mut d, 32 + 24, 0);
    assert!(!detect(&d));
    // the flag test of neither family
    let mut o = opt(Family::A);
    o.flag_bit = 3;
    assert!(!detect(&file(&image_bytes(o))));
    // the return -1 stub replaced
    let mut img = image_bytes(opt(Family::A));
    code(&mut img, 0, enc::moveq(0, 5));
    assert!(!detect(&file(&img)));
    // two note offsets in the sequencer are ambiguous
    let mut img = image_bytes(opt(Family::A));
    code(&mut img, 470, enc::addi_b_d(3, 1));
    assert!(locate(&img).is_none());
    // a silent block that is not silent
    let mut img = image_bytes(opt(Family::A));
    img[SIL + 5] = 1;
    assert!(locate(&img).is_none());
    // the sequence table outside the image
    let mut img = image_bytes(opt(Family::A));
    put32(&mut img, 32, 100_000);
    assert!(locate(&img).is_none());
    assert!(!detect(b"too short"));
}

struct Rig {
    e: crate::engine::Engine,
    s: CoreDesign,
}

fn rig(o: Opt, sub: u16) -> Rig {
    let mut e = crate::engine::Engine::new(48000.0);
    e.m.bus.write_log = Some(Vec::new());
    let img = image_bytes(o);
    let lay = locate(&img).unwrap();
    let s = CoreDesign { lay, img, base: BASE, sub, started: false, songend: false, st: State::default(), gap: 0 };
    Rig { e, s }
}

impl Rig {
    fn tick(&mut self) -> Vec<(u32, u16)> {
        let mut hw = Hw { bus: &mut self.e.m.bus };
        self.s.tick(&mut hw);
        let l = self.e.m.bus.write_log.replace(Vec::new()).unwrap();
        self.e.m.bus.clock += 70940 - 3000;
        l.iter().map(|w| (w.0, w.1)).collect()
    }

    /// Lets Paula run on and takes the audio interrupts it raised.
    fn irqs(&mut self) -> Vec<(u32, u16)> {
        let b = &mut self.e.m.bus;
        b.clock += 2000;
        let c = b.clock;
        b.paula.sync(c, &b.ram);
        let mut hw = Hw { bus: &mut self.e.m.bus };
        for ch in 0..4 {
            self.s.audio_irq_ch(ch, &mut hw);
        }
        let l = self.e.m.bus.write_log.replace(Vec::new()).unwrap();
        l.iter().map(|w| (w.0, w.1)).collect()
    }
}

fn regs(t: &[(u32, u16)], a: u32) -> Vec<u16> {
    t.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

#[test]
fn song_start_note_start_output_and_interrupt() {
    for fam in [Family::A, Family::R, Family::B] {
        let mut r = rig(opt(fam), 0);
        let t1 = r.tick();
        // host volume reset, song init, the four interrupt enables, then the note start
        assert_eq!(regs(&t1, 0xDFF0A8), [0], "{fam:?}");
        let (dma_init, ena_init) = if fam == Family::R { (0x000E, 0x0700) } else { (0x000F, 0x0780) };
        assert_eq!(regs(&t1, 0xDFF096), [dma_init, 0x0002]);
        assert_eq!(regs(&t1, 0xDFF09A), [ena_init, 0x8080, 0x8100, 0x8200, 0x8400, 0x0100][..]);
        assert_eq!(regs(&t1, 0xDFF0B8), [0]);
        // the next tick writes the channel: volume 0 (the envelope's attack to 20 follows), the period of
        // note 12 + 12 from the table, the sample record's data, its length, and restarts DMA
        let t2 = r.tick();
        assert_eq!(regs(&t2, 0xDFF0B8), [0]);
        assert_eq!(regs(&t2, 0xDFF0B6), [1024]);
        assert_eq!((regs(&t2, 0xDFF0B0), regs(&t2, 0xDFF0B2)), (vec![((BASE + REC as u32 + 2) >> 16) as u16], vec![(BASE + REC as u32 + 2) as u16]));
        assert_eq!(regs(&t2, 0xDFF0B4), [4]);
        assert_eq!(regs(&t2, 0xDFF096), [0x8002]);
        assert_eq!(regs(&t2, 0xDFF09A)[0], 0x8100);
        // the interrupt Paula raises acknowledges (INTENA then INTREQ) and, the sample's flag 0
        // set, points the channel at the silent block
        let irq = r.irqs();
        assert_eq!(regs(&irq, 0xDFF09A), [0x0100]);
        assert_eq!(regs(&irq, 0xDFF09C), [0x0100]);
        assert_eq!((r.s.st.ch[1].lc, r.s.st.ch[1].len), (SIL as u32, 128));
        let t3 = r.tick();
        assert_eq!(regs(&t3, 0xDFF0B4), [128]);
        assert_eq!(regs(&t3, 0xDFF0B8), [10]);
        assert!(regs(&t3, 0xDFF096).is_empty());
    }
}

#[test]
fn the_song_restarts_after_its_tracks_end() {
    let mut r = rig(opt(Family::A), 0);
    for _ in 0..8 {
        r.tick();
        r.irqs();
        if r.s.songend {
            break;
        }
    }
    assert!(r.s.songend);
    // the restart plays the song again: the init writes come again
    let t = r.tick();
    assert!(regs(&t, 0xDFF096).contains(&0x000F) || r.s.st.playing);
}

#[test]
fn effects_start_a_note_on_the_selected_channel_and_stop_playing() {
    let mut r = rig(opt(Family::A), 1);
    let t1 = r.tick();
    assert_eq!(regs(&t1, 0xDFF0B8), [0, 0]);
    assert_eq!(regs(&t1, 0xDFF096), [0x0002, 0x8002]);
    assert!(!r.s.st.playing);
    let t2 = r.tick();
    assert_eq!(regs(&t2, 0xDFF0B6), [300]);
    // the effect never ends the song
    assert!(!r.s.songend);
}

#[test]
fn channel3_effects_use_channel_3_and_the_entry_sustain() {
    let mut o = opt(Family::R);
    o.channel3 = true;
    let mut r = rig(o, 1);
    let t1 = r.tick();
    assert_eq!(regs(&t1, 0xDFF0D8), [0, 0]);
    assert_eq!(regs(&t1, 0xDFF096), [0x0008, 0x8008]);
    // the entry's field (1) was counted down by the tick's output step
    assert_eq!(r.s.st.ch[3].sustain, 0);
}

#[test]
fn title_and_subsongs() {
    let r = rig(opt(Family::A), 0);
    assert_eq!(r.s.subsongs(), (0, 1, 0));
    assert_eq!(r.s.format(), "Core Design");
}
