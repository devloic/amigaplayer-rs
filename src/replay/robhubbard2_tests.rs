use super::*;
use crate::machine::Machine;
use m68k::CpuType;

/// A 13-byte envelope record: byte 0, then four (count, delta, ticks) steps.
fn rec(b0: u8, steps: [(u8, i8, u8); 4]) -> [u8; 13] {
    let mut r = [b0; 13];
    for (i, (c, d, t)) in steps.iter().enumerate() {
        r[1 + 3 * i] = *c;
        r[2 + 3 * i] = *d as u8;
        r[3 + 3 * i] = *t;
    }
    r
}

const FLAT: [(u8, i8, u8); 4] = [(1, 0, 1); 4];

/// A module of the corpus' layout (SPEC section 2): `pats` patterns, the four track lists (FF
/// appended), tables A and B with the given records, the rest flat (A: volume 63 at once).
fn module(speed: u16, pats: &[&[u8]], tracks: [&[u8]; 4], a: &[(usize, [u8; 13])], b: &[(usize, [u8; 13])]) -> Vec<u8> {
    let hdr = 18 + 2 * pats.len();
    let mut body = Vec::new();
    let mut toff = [0usize; 4];
    for (k, t) in tracks.iter().enumerate() {
        toff[k] = hdr + body.len();
        body.extend_from_slice(t);
        body.push(0xFF);
    }
    let mut poff = Vec::new();
    for p in pats {
        poff.push(hdr + body.len());
        body.extend_from_slice(p);
    }
    let at = hdr + body.len();
    let mut ta = vec![rec(0, [(1, 63, 1), (1, 0, 1), (1, 0, 1), (1, 0, 1)]); 32];
    let mut tb = vec![rec(0, FLAT); 32];
    for &(i, r) in a {
        ta[i] = r;
    }
    for &(i, r) in b {
        tb[i] = r;
    }
    let end = at + 832;
    let mut d = Vec::new();
    for v in [2u16, speed, (at - 2) as u16, (at + 416 - 2) as u16] {
        d.extend_from_slice(&v.to_be_bytes());
    }
    for t in toff {
        d.extend_from_slice(&((t - 2) as u16).to_be_bytes());
    }
    d.extend_from_slice(&((end - 2) as u16).to_be_bytes());
    for p in poff {
        d.extend_from_slice(&((p - 2) as u16).to_be_bytes());
    }
    d.extend_from_slice(&body);
    for r in ta.iter().chain(tb.iter()) {
        d.extend_from_slice(r);
    }
    assert_eq!(d.len(), end);
    d
}

/// A bank of samples given as (data, loop offset in the data, loop words).
fn bank(samples: &[(Vec<u8>, usize, u16)]) -> Vec<u8> {
    let table = 16 * samples.len();
    let mut data = Vec::new();
    let mut ent = Vec::new();
    for (s, lo, lw) in samples {
        let start = table + data.len();
        ent.extend_from_slice(&(start as u32).to_be_bytes());
        ent.extend_from_slice(&((start + lo) as u32).to_be_bytes());
        ent.extend_from_slice(&((start + lo) as u32).to_be_bytes());
        ent.extend_from_slice(&((s.len() / 2) as u16).to_be_bytes());
        ent.extend_from_slice(&lw.to_be_bytes());
        data.extend_from_slice(s);
    }
    let mut b = ((table + data.len()) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(&ent);
    b.extend_from_slice(&data);
    b
}

fn std_bank() -> Vec<u8> {
    bank(&[((0..1000).map(|i| i as u8).collect(), 800, 100), (vec![7; 20], 0, 1)])
}

const BANK_AT: u32 = 0x20000;
const SCRATCH: u32 = 0x40000;

fn run(d: &[u8], b: &[u8], ticks: usize) -> (Vec<Vec<(u32, u16, u64)>>, bool) {
    let mut r = RobHubbard2::build(d, b.to_vec(), BANK_AT, SCRATCH).unwrap();
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_bytes(BANK_AT, b);
    let mut out = Vec::new();
    for k in 0..ticks {
        m.bus.write_log = Some(Vec::new());
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
        out.push(m.bus.write_log.take().unwrap());
        if r.songend() {
            return (out, true);
        }
    }
    (out, false)
}

fn last(t: &[(u32, u16, u64)], reg: u32) -> Option<u16> {
    t.iter().rev().find(|w| w.0 == reg).map(|w| w.1)
}

const SIL: &[u8] = &[0xC0, 0xA0, 0x85, 0x00, 0xFF];

#[test]
fn detect_accepts_the_layout_and_rejects_changes() {
    let d = module(3, &[SIL, &[0xA0, 60, 0xFF]], [&[1], &[0], &[0], &[0]], &[], &[]);
    assert!(detect(&d));
    let mut x = d.clone();
    x[1] = 3; // word 0 not 2
    assert!(!detect(&x));
    let mut x = d.clone();
    x.push(0); // the tables no longer end the file
    assert!(!detect(&x));
    let mut x = d.clone();
    let t0 = be16(&d, 8) as usize + 2;
    x[t0] = 5; // a pattern number past the table
    assert!(!detect(&x));
    let mut x = d.clone();
    x[t0 + 1] = 0; // track list 0 no longer closed by FF
    assert!(!detect(&x));
    let mut x = d.clone();
    x[2] = 0;
    x[3] = 0; // speed 0
    assert!(!detect(&x));
    assert!(!detect(&d[..d.len() - 1]));
    assert!(!detect(&[]));
}

#[test]
fn bank_names_and_bank_check() {
    assert_eq!(bank_names("rh2.bobolav3.dum"), ["rh.ins", "rh2.ins", "rh2.bobolav3.ins", "rh2.bobolav.ins"]);
    assert_eq!(bank_names("x"), ["rh.ins", "rh2.ins"]);
    assert!(bank_ok(&std_bank()));
    assert!(!bank_ok(&std_bank()[..10]));
    let mut b = std_bank();
    b[0] = 0x7F; // length past the file
    assert!(!bank_ok(&b));
}

#[test]
fn note_starts_the_sample_with_the_genie_order() {
    let d = module(3, &[SIL, &[0xC1, 0x8B, 0xA0, 60, 0xFF]], [&[1], &[0], &[0], &[0]], &[], &[]);
    let (t, _) = run(&d, &std_bank(), 4);
    let regs: Vec<u32> = t[0].iter().map(|w| w.0).collect();
    // period 1 on the starting voices, DMA off, then the voices
    assert_eq!(&regs[..2], &[0xDFF0A6, 0xDFF0B6]);
    assert!(t[0].iter().any(|w| w.0 == 0xDFF096 && w.1 == 0x000F));
    assert_eq!(last(&t[0], 0xDFF0A6), Some(written_period(854)));
    assert_eq!(last(&t[0], 0xDFF0A8), Some(63));
    // the sample from the bank, then its loop two ticks later
    let loc: Vec<_> = t.iter().flatten().filter(|w| w.0 == 0xDFF0A2).map(|w| w.1).collect();
    assert_eq!(loc[0], ((BANK_AT + 4 + 32) & 0xFFFF) as u16);
    assert_eq!(loc[1], ((BANK_AT + 4 + 32 + 800) & 0xFFFF) as u16);
    assert_eq!(last(&t[2], 0xDFF0A4), Some(100));
    let on = t[0].iter().rposition(|w| w.0 == 0xDFF096).unwrap();
    assert_eq!(t[0][on].1, 0x800F);
}

#[test]
fn written_period_rounds_through_the_frequency() {
    assert_eq!(written_period(0), 0);
    assert_eq!(written_period(-5), 0);
    assert_eq!(written_period(16), 122);
    assert_eq!(written_period(854), 854);
    assert_eq!(written_period(861), 860);
    assert_eq!(written_period(27340), 27283);
}

#[test]
fn volume_envelope_loops_only_with_bit_7() {
    let steps = [(1, 20, 1), (1, 1, 1), (1, 0, 1), (1, 0, 1)];
    for (b0, want) in [(0x80u8, [20, 21, 21, 21, 22, 22, 22, 23]), (0x00, [20, 21, 21, 21, 21, 21, 21, 21])] {
        let d = module(3, &[SIL, &[0xC1, 0x8B, 0xA0, 60, 0xFF]], [&[1], &[0], &[0], &[0]], &[(1, rec(b0, steps))], &[]);
        let (t, _) = run(&d, &std_bank(), 8);
        let v: Vec<u16> = t.iter().map(|x| last(x, 0xDFF0A8).unwrap()).collect();
        assert_eq!(v, want, "byte 0 {b0:02x}");
    }
}

#[test]
fn period_envelope_always_loops_to_its_second_step() {
    let b = rec(0, [(1, 5, 1), (1, 1, 1), (1, 0, 1), (1, 0, 1)]);
    let d = module(3, &[SIL, &[0xE1, 1, 0x8B, 0xA0, 60, 0xFF]], [&[1], &[0], &[0], &[0]], &[], &[(1, b)]);
    let (t, _) = run(&d, &std_bank(), 10);
    let p: Vec<u16> = t.iter().map(|x| last(x, 0xDFF0A6).unwrap()).collect();
    let want: Vec<u16> = [5, 6, 6, 6, 7, 7, 7, 8, 8, 8].iter().map(|&e| written_period(854 + e)).collect();
    assert_eq!(p, want);
}

#[test]
fn lengths_transpose_and_note_zero() {
    // 80 = 2 steps at speed 2: the first note lasts 3 ticks (the first step comes one tick after
    // the start, SPEC section 7); E0 transposes, note 0 does not
    let d = module(2, &[SIL, &[0x80, 0xE0, 12, 0xA0, 41, 0xA0, 0, 0xFF]], [&[1], &[0], &[0], &[0]], &[], &[]);
    let (t, _) = run(&d, &std_bank(), 6);
    let p: Vec<u16> = t.iter().map(|x| last(x, 0xDFF0A6).unwrap()).collect();
    assert_eq!(&p[..3], &[written_period(1280); 3]);
    assert_eq!(p[3], written_period(27340));
}

#[test]
fn a_note_without_a_sample_byte_does_not_restart() {
    let d = module(2, &[SIL, &[0x80, 0xA0, 60, 62, 0xFF]], [&[1], &[0], &[0], &[0]], &[], &[]);
    let (t, _) = run(&d, &std_bank(), 6);
    let dma_on = |x: &Vec<(u32, u16, u64)>| x.iter().any(|w| w.0 == 0xDFF096 && w.1 & 0x8001 == 0x8001);
    assert!(dma_on(&t[0]));
    assert!(!dma_on(&t[3]));
    assert_eq!(last(&t[3], 0xDFF0A6), Some(written_period(PERIODS[62] as i32)));
}

#[test]
fn short_sample_plays_from_the_buffers_and_one_word_loop_is_silence() {
    let d = module(3, &[SIL, &[0x8B, 0xA1, 60, 0xFF]], [&[1], &[0], &[0], &[0]], &[], &[]);
    let b = std_bank();
    let (t, _) = run(&d, &b, 4);
    // sample 1 is 20 bytes: from voice 0's first buffer, with its audio interrupt on
    let loc: Vec<_> = t.iter().flatten().filter(|w| w.0 == 0xDFF0A2).map(|w| w.1).collect();
    assert_eq!(loc[0], ((SCRATCH + 2 * BUF as u32) & 0xFFFF) as u16);
    assert!(t[0].iter().any(|w| w.0 == 0xDFF09A && w.1 == 0xC080));
    let mut r = RobHubbard2::build(&d, b, BANK_AT, SCRATCH).unwrap();
    assert_eq!(r.block(1, 4), Some((4 + 32 + 1000, 1)));
    let mut m = Machine::new(CpuType::M68000);
    let mut hw = Hw { bus: &mut m.bus };
    r.v[0].sample = 1;
    r.v[0].cur = (4 + 32 + 1000, 20);
    r.set_loop(0, &mut hw);
    assert_eq!(r.v[0].looped, Some((SCRATCH, 1)));
}

#[test]
fn song_ends_when_all_track_lists_end() {
    let d = module(1, &[SIL, &[0x80, 0xA0, 60, 0xFF]], [&[1], &[0], &[0], &[0]], &[], &[]);
    let (t, end) = run(&d, &std_bank(), 100);
    assert!(end);
    assert!(t.len() < 20);
    assert_eq!(last(t.last().unwrap(), 0xDFF0A8), Some(0));
}

#[test]
fn malformed_input_does_not_panic_or_hang() {
    // patterns without notes, a pattern of commands only, wild bytes, samples outside the bank
    let mut wild: Vec<u8> = (0..200u32).map(|i| (i * 37 % 251) as u8).collect();
    wild.push(0xFF);
    let pats: [&[u8]; 4] = [&[0xFF], &[0xE5, 0xFF], &[0xBF, 0xDF, 0xE1, 0xFF, 0xFF], &wild];
    for p in pats {
        let d = module(1, &[SIL, p], [&[1, 1, 1], &[0], &[1, 0], &[1]], &[], &[]);
        for b in [std_bank(), bank(&[(vec![], 0, 0)]), {
            let mut b = std_bank();
            b[4] = 0xFF; // sample 0 starts past the bank
            b
        }] {
            let _ = run(&d, &b, 300);
        }
    }
    // a track list of 255 empty patterns: bounded per tick
    let d = module(1, &[&[0xFF]], [&[0; 250], &[0], &[0], &[0]], &[], &[]);
    let (_, end) = run(&d, &std_bank(), 50);
    assert!(end);
    // random files are not modules
    for n in [0usize, 1, 17, 18, 900, 5000] {
        let r: Vec<u8> = (0..n as u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        assert!(!detect(&r));
    }
}

#[test]
fn real_modules_when_present() {
    let w = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/port-rh2/m";
    let dirs = [format!("{w}/bobo"), format!("{w}/blood")];
    let mut n = 0;
    for dir in &dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let b = std::fs::read(format!("{dir}/rh.ins")).unwrap();
        for e in rd.flatten() {
            let p = e.path();
            let d = std::fs::read(&p).unwrap();
            let is = p.extension().is_some_and(|x| x == "dum");
            assert_eq!(detect(&d), is, "{}", p.display());
            if is {
                n += 1;
                let (t, _) = run(&d, &b, 200);
                assert!(t.iter().flatten().any(|w| w.0 == 0xDFF0A8 && w.1 > 0));
            }
        }
    }
    // the rh.* driver images are not this format
    if let Ok(rd) = std::fs::read_dir("/media/lolo/kingston2tb/claude-scratch/deliplayerrust/fixround/rhimpl/mods") {
        for e in rd.flatten() {
            assert!(!detect(&std::fs::read(e.path()).unwrap()));
        }
    }
    if n > 0 {
        assert_eq!(n, 9);
    }
}

/// `RH2_LISTS=list.txt[:list.txt...]`: no file named in the lists (one path a line) is taken,
/// except `rh2.*` ones. Run with `--ignored`.
#[test]
#[ignore]
fn takes_no_module_of_the_corpus_lists() {
    let Ok(lists) = std::env::var("RH2_LISTS") else { return };
    let (mut read, mut taken) = (0, Vec::new());
    for l in lists.split(':') {
        for p in std::fs::read_to_string(l).unwrap_or_default().lines().filter(|p| !p.starts_with('#')) {
            let Ok(d) = std::fs::read(p.trim()) else { continue };
            read += 1;
            if detect(&d) {
                taken.push(p.to_string());
            }
        }
    }
    eprintln!("{read} files read, {} taken", taken.len());
    assert!(taken.iter().all(|p| p.to_lowercase().contains("/rh2.")), "{taken:?}");
}

#[test]
fn library_names_the_format_the_bank_decides() {
    // an Infogrames module's check takes this layout too, and comes first: the module alone is
    // named Infogrames, with the Rob Hubbard 2 bank beside it Rob Hubbard 2 (which plays it)
    let d = module(3, &[SIL, &[0xA0, 60, 0xFF]], [&[1], &[0], &[0], &[0]], &[], &[]);
    let b = std_bank();
    let several = crate::open::claimed_by_several(&d);
    let named = crate::detect_with("rh2.test.dum", &d, &[("rh.ins", &b)]).unwrap();
    assert!(named.contains("Hubbard"), "{named}");
    if several {
        assert!(!crate::detect(&d).unwrap().contains("Hubbard"));
    }
}
