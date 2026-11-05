use super::*;
use m68k::CpuType;

/// An empty module: all sequences end at once, instrument 0 is 200 bytes (looped from 40, length 80)
/// when `looped`, else one-shot.
fn module(looped: bool) -> Vec<u8> {
    let mut d = vec![0u8; SMP + 400];
    d[..4].copy_from_slice(b"FUCO");
    for v in 0..4 {
        d[SEQ + 256 * v] = 0xFE;
    }
    d[INS + 4..INS + 8].copy_from_slice(&200u32.to_be_bytes());
    if looped {
        d[INS + 8..INS + 12].copy_from_slice(&40u32.to_be_bytes());
        d[INS + 12..INS + 16].copy_from_slice(&80u32.to_be_bytes());
    }
    d
}

fn set_voice0(d: &mut [u8], seq: &[u8], pat: &[u8]) {
    d[SEQ..SEQ + seq.len()].copy_from_slice(seq);
    d[PAT..PAT + pat.len()].copy_from_slice(pat);
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let mut r = FcBsi::build(d, 0x10000);
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    (m.bus.write_log.take().unwrap(), r.songend())
}

#[test]
fn detection() {
    let d = module(false);
    assert!(detect(&d));
    assert!(!detect(&d[..SMP - 1]));
    let mut e = d.clone();
    e[0] = b'X';
    assert!(!detect(&e));
    assert!(!detect(&[]));
}

#[test]
fn period_table_shape() {
    assert_eq!(PERIODS.len(), 84);
    assert!(PERIODS.windows(2).all(|w| w[0] > w[1]));
    assert_eq!((PERIODS[41], PERIODS[53]), (165, 83));
}

#[test]
fn looped_note_write_order() {
    let mut d = module(true);
    // pattern 0: duration 4, instrument 0, note 12, end
    set_voice0(&mut d, &[0, 0xFE], &[0x84, 0xC0, 12, 0xFF]);
    let (w, _) = run(&d, 1);
    let regs: Vec<(u32, u16)> = w.iter().map(|&(a, v, _)| (a, v)).collect();
    let st = (0x10000 + SMP) as u32;
    // voice 3, 2, 1 end the song first (empty sequences), so only the song end is seen there
    assert!(regs.contains(&(0xDFF096, 1)));
    let i = regs.iter().position(|&r| r == (0xDFF096, 1)).unwrap();
    assert_eq!(
        &regs[i..i + 8],
        &[
            (0xDFF096, 1),
            (0xDFF0A0, (st >> 16) as u16),
            (0xDFF0A2, st as u16),
            (0xDFF0A4, 100),
            (0xDFF0A6, PERIODS[12]),
            (0xDFF096, 0x8001),
            (0xDFF0A4, 40),
            (0xDFF0A0, ((st + 40) >> 16) as u16),
        ]
    );
}

#[test]
fn one_shot_note_ends_with_length_one_and_volume() {
    let mut d = module(false);
    set_voice0(&mut d, &[0xFC, 0x20, 0, 0xFE], &[0xC0, 5, 0xFF]);
    // voices 3..1 finish the song in tick 0 but voice 0 is processed in the same tick
    let (w, ended) = run(&d, 1);
    assert!(ended);
    let regs: Vec<(u32, u16)> = w.iter().map(|&(a, v, _)| (a, v)).collect();
    assert!(regs.contains(&(0xDFF0A8, 0x20)));
    let on = regs.iter().position(|&r| r == (0xDFF096, 0x8001)).unwrap();
    assert_eq!(regs[on + 1], (0xDFF0A4, 1));
    assert_eq!(regs[on + 2], (0xDFF0A8, 0x20));
}

#[test]
fn song_end_then_only_dma_off() {
    let d = module(false);
    let (w, ended) = run(&d, 3);
    assert!(ended);
    let tail: Vec<_> = w.iter().rev().take(2).collect();
    assert!(tail.iter().all(|&&(a, v, _)| a == 0xDFF096 && v == 0x000F));
}

#[test]
fn read_limit_ends_a_looping_sequence() {
    let mut d = module(false);
    for v in 0..4 {
        d[SEQ + 256 * v] = 0xFF;
    }
    let (_, ended) = run(&d, 1);
    assert!(ended);
}

#[test]
fn filter_entry_writes_the_cia_port() {
    let mut d = module(false);
    set_voice0(&mut d, &[0xC1, 0xC0, 0, 0xFE], &[0x84, 0xC0, 12, 0xFF]);
    let (w, _) = run(&d, 1);
    let f: Vec<u16> = w.iter().filter(|x| x.0 == 0xBFE001).map(|x| x.1).collect();
    assert_eq!(f, vec![2, 0]);
}

#[test]
fn deterministic_and_never_panics_on_garbage() {
    let mut d = module(true);
    let mut s = 12345u32;
    for b in d.iter_mut().skip(4).take(SMP - 4) {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        *b = (s >> 24) as u8;
    }
    let a = run(&d, 400);
    let b = run(&d, 400);
    assert_eq!(a.0, b.0);
    // a truncated module reads zeros
    run(&d[..SMP], 100);
}
