use super::*;
use crate::engine::Engine;
use std::path::Path;

// ---------------------------------------------------------------------------------
// Synthetic modules, built from the specification's layout (no game data)

const PER: usize = 0x200;
const SDATA: usize = 0x900;

fn put32(v: &mut [u8], o: usize, x: u32) {
    v[o..o + 4].copy_from_slice(&x.to_be_bytes());
}

fn put16(v: &mut [u8], o: usize, x: u16) {
    v[o..o + 2].copy_from_slice(&x.to_be_bytes());
}

/// The order list of a channel: pattern 0 once, then `end` (a command byte); `pat` selects the pattern.
fn order(pat: u8, end: u8) -> [u8; 4] {
    [pat, 0, end, 0]
}

/// A module of variant `var` with two subsongs. Pattern 0: instrument 1, speed 3, a note, a wait,
/// end of pattern. Pattern 1: instrument 2, one short note. `ends` is the order list terminator.
fn build(var: Var, ends: u8) -> Vec<u8> {
    let mut img = vec![0u8; 0x1000];
    put32(&mut img, 0, 0x70FF_4E75);
    img[4..12].copy_from_slice(b"H.DAVIES");
    put32(&mut img, 0x0C, 0x100);
    put32(&mut img, 0x10, 0x110);
    put32(&mut img, 0x14, 0x120);
    put32(&mut img, 0x20, 0x40);
    put32(&mut img, 0x24, 0x50);
    put32(&mut img, 0x28, 0x60);
    img[0x40..0x4E].copy_from_slice(b"Synthetic Test");
    put32(&mut img, PER, PERIOD_KEY);
    for i in 2..86 {
        put16(&mut img, PER + 2 * i, 0x3000 - 0x60 * i as u16);
    }
    let dma = PER + 172;
    img[dma..dma + 4].copy_from_slice(&[8, 4, 2, 1]);
    let (cmds, ords, envl) = match var {
        Var::A => (5, 2, dma + 32),
        Var::B => (4, 3, dma + 36),
        Var::C => (5, 3, dma + 36),
    };
    let cmd_at = if var == Var::B { dma + 8 } else { dma + 4 };
    if var == Var::B {
        img[dma + 4..dma + 8].copy_from_slice(&[0xF7, 0xFB, 0xFD, 0xFE]);
    }
    for i in 0..cmds + ords {
        put32(&mut img, if i < cmds { cmd_at + 4 * i } else { dma + 24 + 4 * (i - cmds) }, 0x100);
    }
    let samp = envl + 6;
    put32(&mut img, envl, (samp - 2) as u32);
    img[samp - 2..samp].copy_from_slice(&[0x00, 0xFF]);
    // sample table: silent, plain, looped
    put16(&mut img, samp + 4, 1);
    put16(&mut img, samp + 10, 1);
    for (i, (start, len, lp, lplen, vol)) in [(0usize, 0u16, 0usize, 0u16, 0u16), (SDATA, 16, 0, 1, 0x30), (SDATA + 32, 16, SDATA + 40, 4, 0x20)]
        .into_iter()
        .enumerate()
        .skip(1)
    {
        let e = samp + 16 * i;
        put32(&mut img, e, start as u32);
        put16(&mut img, e + 4, len);
        put32(&mut img, e + 6, lp as u32);
        put16(&mut img, e + 10, lplen);
        put16(&mut img, e + 12, vol);
    }
    let sampend = samp + 48;
    put32(&mut img, 0x18, samp as u32);
    put32(&mut img, 0x1C, sampend as u32);
    let sub = if var == Var::A { sampend + 0x20 } else { sampend };
    put32(&mut img, 0x3C, sub as u32);
    // 2 subsongs x 4 channels: order lists behind the table
    let lists = sub + 32;
    for s in 0..2 {
        for c in 0..4 {
            let at = lists + 4 * (s * 4 + c);
            put32(&mut img, sub + 16 * s + 4 * c, at as u32);
            img[at..at + 4].copy_from_slice(&order(s as u8, ends));
        }
    }
    let pt = lists + 32;
    put32(&mut img, pt, (pt + 8) as u32);
    let p0 = pt + 8;
    let pat0 = [0xFE, 1, 0xFD, 3, 10, 3, 0xFC, 1, 0xFF];
    img[p0..p0 + pat0.len()].copy_from_slice(&pat0);
    let p1 = p0 + 16;
    put32(&mut img, pt + 4, p1 as u32);
    let pat1 = [0xFE, 2, 14, 0, 0xFF];
    img[p1..p1 + pat1.len()].copy_from_slice(&pat1);
    for i in 0..128 {
        img[SDATA + i] = (i * 3) as u8 | 1;
    }
    img.truncate(0x1000);
    let words = img.len() / 4;
    let mut f = Vec::new();
    for l in [0x3F3u32, 0, 1, 0, 0, 0x4000_0000 | words as u32, 0x3E9, words as u32] {
        f.extend_from_slice(&l.to_be_bytes());
    }
    f.extend_from_slice(&img);
    for l in [0x3ECu32, 0, 0x3F2] {
        f.extend_from_slice(&l.to_be_bytes());
    }
    f
}

fn start(d: &[u8], ticks: usize) -> (Engine, Vec<(u32, u16, u64)>) {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("test.hd"), d.to_vec()).unwrap();
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..ticks {
        e.render(&mut buf);
    }
    let l = e.m.bus.write_log.clone().unwrap();
    (e, l)
}

// ---------------------------------------------------------------------------------

#[test]
fn recognises_the_three_variants() {
    for var in [Var::A, Var::B, Var::C] {
        let d = build(var, 0xFF);
        let l = locate(&d).unwrap_or_else(|| panic!("{var:?} not recognised"));
        assert_eq!(l.var, var);
        assert_eq!(l.nsub, 2);
        assert_eq!(l.npat, 2);
        assert_eq!(l.nsamp, 3);
        assert!(detect(&d));
    }
}

#[test]
fn rejects_layouts_that_do_not_verify() {
    let good = build(Var::C, 0xFF);
    assert!(detect(&good));
    // a damaged signature, hunk header, DMA bit table, envelope list, sample entry, order list
    let mut t = good.clone();
    t[0x24] = b'X';
    assert!(!detect(&t));
    let mut t = good.clone();
    t[3] = 0xF4;
    assert!(!detect(&t));
    let mut t = good.clone();
    t[0x14] = 0;
    assert!(!detect(&t));
    let mut t = good.clone();
    let dma = 0x20 + PER + 172;
    t[dma + 1] = 5;
    assert!(!detect(&t));
    let mut t = good.clone();
    t[0x20 + PER + 172 + 40 + 1] ^= 0xFF; // the envelope data
    assert!(!detect(&t));
    // an unknown distance between the DMA bit table and the envelope list
    let mut t = good.clone();
    t.splice(0x20 + dma_end()..0x20 + dma_end(), [0u8; 4]);
    assert!(!detect(&t));
    // a pattern index beyond the pattern table in an order list
    let l = locate(&good).unwrap();
    let first = be32(&good[0x20..], l.sub) as usize;
    let mut t = good.clone();
    t[0x20 + first] = 9;
    assert!(!detect(&t));
    // not an hd module at all, and every truncation
    assert!(!detect(&[]));
    assert!(!detect(&[0u8; 200]));
    for n in (0..good.len()).step_by(37) {
        let _ = detect(&good[..n]);
    }
}

fn dma_end() -> usize {
    PER + 172 + 4
}

#[test]
fn the_start_scan_costs_decide_the_first_call() {
    let d = build(Var::C, 0xFF);
    let img = image(&d).unwrap();
    // the scan walks 401 words of the tick routine's area and then the init routine's up to the
    // period table: no marker longs there, so the cost is the plain per-word sum
    let c = scan_cycles(img, 0x100, 0x120).unwrap();
    let words_to_key = (PER - 0x120) / 2;
    assert_eq!(c, 44 + 401 * 47 + 16 + (words_to_key as u64) * (29 + 21 + 29 + 21) + 29);
}

#[test]
fn first_interrupt_and_note_start_follow_the_spec() {
    for var in [Var::A, Var::B, Var::C] {
        let d = build(var, 0xFF);
        let (e, log) = start(&d, 3);
        let img = image(&d).unwrap();
        let scan = scan_cycles(img, 0x100, 0x120).unwrap();
        let init_cycles: u64 = match var {
            Var::A => 3482,
            Var::B => 2954,
            Var::C => 3026,
        };
        let call = 957 + scan / 2;
        let tick0 = call + init_cycles / 2 + FRAME;
        // the first DMACON write after the init block
        let first = log.iter().find(|w| w.0 == 0xDFF096 && w.1 == 0x800F).expect("no interrupt");
        let wrapper = if var == Var::B { 150 } else { 148 };
        assert_eq!(first.2, tick0 + wrapper, "{var:?}");
        // order after it: four voice pointer/length pairs, then the first step's note start of
        // channel 0 (DMACON bit, pointer, period, length, volume)
        let i = log.iter().position(|w| w == first).unwrap();
        let addrs: Vec<u32> = log[i..].iter().map(|w| w.0).take(8 + 12 + 6 + 3).collect();
        let loc = |c: u32| [0xDFF0A0 + 16 * c, 0xDFF0A2 + 16 * c, 0xDFF0A4 + 16 * c];
        let mut want = vec![0xDFF096];
        for c in 0..4 {
            want.extend(loc(c));
        }
        assert_eq!(&addrs[..13], &want[..], "{var:?}");
        // the first writes after the voices: the instrument was set, so channel 0's note starts
        assert_eq!(addrs[13], 0xDFF096);
        assert_eq!(log[i + 13].1, 1);
        assert_eq!(&addrs[14..19], &[0xDFF0A0, 0xDFF0A2, 0xDFF0A6, 0xDFF0A4, 0xDFF0A8]);
        // the period is table word 10; the volume the instrument's 0x30
        assert_eq!(log[i + 16].1, be16(img, PER + 20));
        assert_eq!(log[i + 18].1, 0x30);
        // the sample pointer is the module's sample (past the 0x20 bytes of hunk header)
        let hi = log[i + 14].1 as u32;
        let lo = log[i + 15].1 as u32;
        let ptr = (hi << 16) | lo;
        let base = e.m.deli.list.first().unwrap().0;
        assert_eq!(ptr, base + 0x20 + SDATA as u32);
    }
}

#[test]
fn output_is_deterministic() {
    for var in [Var::A, Var::B, Var::C] {
        let d = build(var, 0xFF);
        let (_, a) = start(&d, 40);
        let (_, b) = start(&d, 40);
        assert_eq!(a, b, "{var:?}");
        assert!(a.len() > 100);
    }
}

#[test]
fn song_end_is_reported_when_every_channel_looped() {
    for var in [Var::A, Var::B, Var::C] {
        let d = build(var, 0xFF);
        let (e, _) = start(&d, 4);
        assert!(!e.songend(), "{var:?} too early");
        let (e, _) = start(&d, 80);
        assert!(e.songend(), "{var:?}");
    }
}

#[test]
fn variant_c_dead_end_parks_the_channel_without_song_end() {
    let d = build(Var::C, 0xFD);
    let (e, log) = start(&d, 300);
    assert!(!e.songend());
    // after the pattern ran once the volume writes stop: the tail of the log holds only the
    // per-tick voice writes of the DMA routine
    let tail: Vec<_> = log.iter().rev().take(200).collect();
    assert!(tail.iter().all(|w| w.0 != 0xDFF0A8 && w.0 != 0xDFF0A6));
}

#[test]
fn variant_b_stop_command_clears_the_channel_bits() {
    let d = build(Var::B, 0xFD);
    let (e, log) = start(&d, 120);
    assert!(e.songend(), "all four channels stopped");
    // once every channel has stopped the interrupt only switches DMA with an empty mask
    let last = log.iter().rev().find(|w| w.0 == 0xDFF096).unwrap();
    assert_eq!(last.1, 0x8000);
}

#[test]
fn subsong_change_runs_endsound_then_initsound() {
    for var in [Var::A, Var::B, Var::C] {
        let d = build(var, 0xFF);
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.hd"), d.clone()).unwrap();
        e.change_subsong(1).unwrap();
        let log = e.m.bus.write_log.clone().unwrap();
        let img = image(&d).unwrap();
        let scan = scan_cycles(img, 0x100, 0x120).unwrap();
        let init_cycles: u64 = match var {
            Var::A => 3482,
            Var::B => 2954,
            Var::C => 3026,
        };
        let end0 = 957 + scan / 2 + init_cycles / 2;
        // EndSound: DMACON 000f 14 colour clocks in, then the four volumes 0, 6 apart
        let ds = log.iter().find(|w| w.0 == 0xDFF096 && w.2 == end0 + 14).expect("EndSound DMACON");
        assert_eq!(ds.1, 0x000F);
        for c in 0..4u64 {
            let v = log.iter().find(|w| w.0 == 0xDFF0A8 + 16 * c as u32 && w.2 == end0 + 22 + 6 * c).expect("EndSound volume");
            assert_eq!(v.1, 0);
        }
        // the second InitSound starts 236 colour clocks after EndSound's 48
        if var == Var::A {
            let cia: Vec<_> = log.iter().filter(|w| w.0 == 0xBFE001).collect();
            assert_eq!(cia.len(), 2);
            assert_eq!(cia[1].2, end0 + 48 + 236 + (1168 + 24) / 2);
        }
        // the interrupts follow the second init by 70940
        let init1_end = end0 + 48 + 236 + init_cycles / 2;
        let wrapper = if var == Var::B { 150 } else { 148 };
        assert_eq!(e.m.bus.write_log.as_ref().unwrap().iter().filter(|w| w.0 == 0xDFF096 && w.1 == 0x800F).count(), 0);
        let (_, log2) = {
            let mut buf = vec![0f32; 2 * 960];
            e.render(&mut buf);
            e.render(&mut buf);
            (0, e.m.bus.write_log.clone().unwrap())
        };
        let first = log2.iter().find(|w| w.0 == 0xDFF096 && w.1 == 0x800F).expect("interrupt after the change");
        assert_eq!(first.2, init1_end + FRAME + wrapper, "{var:?}");
    }
}

#[test]
fn damaged_modules_do_not_panic() {
    let mut seed = 12345u32;
    let mut rnd = move || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        seed >> 8
    };
    for var in [Var::A, Var::B, Var::C] {
        let good = build(var, 0xFF);
        for _ in 0..60 {
            let mut t = good.clone();
            for _ in 0..4 {
                // damage the data area (orders, patterns, samples) rather than the header
                let o = 0x20 + 0x2B0 + rnd() as usize % (0x1000 - 0x2B0);
                t[o] = rnd() as u8;
            }
            if detect(&t) {
                let _ = start(&t, 30);
            }
        }
    }
}
