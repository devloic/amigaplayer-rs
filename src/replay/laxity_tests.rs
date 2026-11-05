use super::*;
use m68k::CpuType;

const BASE: u32 = 0x45E0;

/// An instrument: (word 0, word 2, word 4, finetune, synth flag).
type Ins = (u16, u16, u16, i8, u8);

fn cell(note: u8, ins: u8, vol: u8, cmd: u8, par: u8) -> [u8; 4] {
    [(ins >> 4 & 3) << 6 | (note & 0x3F), vol, (ins & 15) << 4 | (cmd & 15), par]
}

/// A module: instruments 1.., four position lists, patterns of `rows` rows, sample bytes, padded
/// so that the synth buffers lie inside it, as the specification's modules do.
fn module(ins: &[Ins], lists: [&[u8]; 4], pats: &[Vec<[u8; 4]>], rows: usize, samples: &[u8], restart: u16) -> Vec<u8> {
    let n = ins.len();
    let s = 24 * n + 16;
    let l = lists[0].len();
    let p = 10 + s + 4 * l;
    let mut d = Vec::new();
    for w in [p as u16, s as u16, l as u16, restart] {
        d.extend_from_slice(&w.to_be_bytes());
    }
    d.extend_from_slice(SIG);
    for &(a, b, c, f, sy) in ins {
        for w in [a, b, c] {
            d.extend_from_slice(&w.to_be_bytes());
        }
        d.push(f as u8);
        d.push(sy);
        d.extend_from_slice(&[0; 16]);
    }
    d.extend_from_slice(&((rows * 4) as u16).to_be_bytes());
    for li in lists {
        d.extend_from_slice(li);
    }
    let mx = lists.iter().flat_map(|l| l.iter()).copied().max().unwrap_or(0) as usize;
    for k in 0..=mx {
        let mut pat = pats.get(k).cloned().unwrap_or_default();
        pat.resize(rows, [0; 4]);
        for c in pat {
            d.extend_from_slice(&c);
        }
    }
    d.extend_from_slice(samples);
    if d.len() < 0x11E6 {
        d.resize(0x11E6, 0);
    }
    d
}

/// Two synth instruments (1: reload 3, widths 4..8; 2: widths 10..20), a one-shot sample (3) and
/// a looping one (4).
fn ins4() -> Vec<Ins> {
    let mut v = vec![(0x0003, 0x0004, 0x0008, 0, 0xFF), (0x0001, 0x000A, 0x0014, 0, 0xFF)];
    v.push((8, 0, 0, 0, 0));
    v.push((8, 2, 4, 0, 0));
    v
}

fn start(d: &[u8]) -> (Machine, Box<dyn Replayer>) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_bytes(BASE, d);
    m.bus.write_log = Some(Vec::new());
    let r = (OPEN.new)(d, BASE, &mut m).expect("module accepted");
    m.bus.write_log = Some(Vec::new());
    (m, r)
}

/// Runs `ticks` ticks at 70 940 colour clocks; the log and the song-end flag.
fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let (mut m, mut r) = start(d);
    let t0 = m.bus.clock + TICK;
    for k in 0..ticks {
        m.bus.clock = t0 + TICK * k as u64;
        r.tick(&mut Hw { bus: &mut m.bus });
    }
    (m.bus.write_log.take().unwrap(), r.songend())
}

fn simple() -> Vec<u8> {
    let pat = vec![cell(25, 3, 0x30, 0, 0), cell(0, 0, 0, 0, 0), cell(27, 4, 0x28, 1, 2), cell(0, 1, 0x20, 0, 0)];
    module(&ins4(), [&[0, 0], &[0, 0], &[0, 0], &[0, 0]], &[pat], 4, &[0u8; 64], 0)
}

#[test]
fn tables() {
    assert_eq!(PER.len(), 60);
    assert_eq!(FT.len(), 60);
    assert!(PER.windows(2).all(|w| w[0] > w[1]));
}

#[test]
fn detection() {
    let d = simple();
    assert!(detect(&d));
    // the zero byte at offset 8 replaces the signature
    let mut e = d.clone();
    e[8..24].fill(0);
    assert!(detect(&e));
    // neither the signature nor a zero byte at 8
    let mut e = d.clone();
    e[8] = b'X';
    assert!(!detect(&e));
    // the flag bytes
    for o in [0x1F, 0x37] {
        let mut e = d.clone();
        e[o] = 0;
        assert!(!detect(&e), "{o}");
    }
    assert!(!detect(&d[..0x30]));
    assert!(!detect(&[]));
}

#[test]
fn rejects_protracker_modules() {
    // a ProTracker file: 20-byte title, 31 sample headers of 30 bytes, song length, order list,
    // "M.K." at 1080; also with the weak check's bytes forced (0xFF at 0x1F and 0x37, zero at 8)
    let mut m = vec![0u8; 1084 + 1024];
    m[..9].copy_from_slice(b"TEST SONG");
    for i in 0..31 {
        let o = 20 + 30 * i;
        m[o..o + 6].copy_from_slice(b"sample");
        m[o + 22] = 0;
        m[o + 23] = 8;
        m[o + 25] = 64;
    }
    m[950] = 1;
    m[1080..1084].copy_from_slice(b"M.K.");
    assert!(!detect(&m));
    m[0x1F] = 0xFF;
    m[0x37] = 0xFF;
    m[8] = 0;
    assert!(!detect(&m), "header sizes of a ProTracker file are not a Laxity header");
    // a Laxity Tracker (.lax) style file: starts with a name, ProTracker-like headers
    let mut x = vec![0u8; 600];
    x[..4].copy_from_slice(b"LAX ");
    assert!(!detect(&x));
}

#[test]
fn first_tick_is_the_left_over_effect_tick() {
    let (log, _) = run(&simple(), 1);
    // AUD2PER = 0x00F0, AUD3PER = 0x202F, 198 colour clocks apart, the first 427 after the call
    assert_eq!(log.len(), 2, "{log:x?}");
    assert_eq!((log[0].0, log[0].1), (0xDFF0C6, 0x00F0));
    assert_eq!((log[1].0, log[1].1), (0xDFF0D6, 0x202F));
    assert_eq!(log[1].2 - log[0].2, 198);
}

#[test]
fn note_start_write_sequence() {
    let (log, _) = run(&simple(), 2);
    let regs: Vec<(u32, u16)> = log.iter().map(|w| (w.0, w.1)).collect();
    // row tick: volume, loop start, length, period for voice 0 (sample instrument 3: 8 words)
    let i = regs.iter().position(|w| w.0 == 0xDFF0A8).unwrap();
    assert_eq!(regs[i].1, 0x30);
    assert_eq!(regs[i + 1].0, 0xDFF0A0);
    assert_eq!(regs[i + 3], (0xDFF0A4, 8));
    assert_eq!(regs[i + 4], (0xDFF0A6, PER[24]));
    // spacing of the first voice's writes: volume to start 192, start to length 103, length to period 95
    assert_eq!(log[i + 1].2 - log[i].2, 192);
    assert_eq!(log[i + 3].2 - log[i + 1].2, 103);
    assert_eq!(log[i + 4].2 - log[i + 3].2, 95);
    // the row's DMA off for the four voices, then on, then the loop registers, ten lines later
    let off = regs.iter().position(|w| w.0 == 0xDFF096).unwrap();
    assert_eq!(regs[off].1, 0x000F);
    assert_eq!(regs[off + 1].1, 0x800F);
    assert_eq!(log[off + 1].2 - log[off].2, 135);
    let lines = log[off + 2].2 - log[off + 1].2;
    assert!((2150..=2373).contains(&lines), "{lines}");
}

#[test]
fn synth_waveform_is_written_into_guest_memory() {
    let pat = vec![cell(30, 1, 0x20, 0, 0)];
    let d = module(&ins4(), [&[0], &[0], &[0], &[0]], &[pat], 8, &[0; 16], 0);
    let (mut m, mut r) = start(&d);
    let t0 = m.bus.clock + TICK;
    for k in 0..3 {
        m.bus.clock = t0 + TICK * k;
        r.tick(&mut Hw { bus: &mut m.bus });
    }
    // width 5 (minimum 4 plus one step): 14 bytes of 0x80, 5 zero bytes... checked by shape
    let a = BASE + SYNTH_OFF;
    let w: Vec<u8> = (0..32).map(|k| m.bus.r8(a + k)).collect();
    let zeros = w.iter().filter(|&&b| b == 0).count();
    assert_eq!(w[0], 0x80);
    assert_eq!(w[31], 0x7F);
    assert!((4..=8).contains(&zeros), "{w:x?}");
    assert!(w.iter().all(|&b| b == 0 || b == 0x80 || b == 0x7F));
}

#[test]
fn song_end_signals_and_continues_at_the_restart() {
    // one position, four rows: the song ends after 4 rows of 6 ticks
    let d = module(&ins4(), [&[0], &[0], &[0], &[0]], &[vec![cell(25, 3, 0x30, 0, 0)]], 4, &[0; 16], 0);
    let (log, end) = run(&d, 40);
    assert!(end);
    // it keeps playing: notes start again in the second pass
    let starts = log.iter().filter(|w| w.0 == 0xDFF0A6).count();
    assert!(starts >= 2, "{starts}");
}

#[test]
fn deterministic() {
    let d = simple();
    assert_eq!(run(&d, 50).0, run(&d, 50).0);
}

#[test]
fn safe_on_degenerate_input() {
    // arpeggio with a speed above 32 (tick counter beyond the table), a synth width above 32,
    // pattern break, jumps, speed 0, volume column break on every tick
    let cmds = vec![
        cell(30, 1, 0x20, 15, 40),
        cell(30, 2, 0xFE, 0, 0x37),
        cell(32, 4, 0x20, 0, 0x12),
        cell(0, 0, 0x8F, 11, 0),
        cell(0, 0, 0x9F, 13, 2),
        cell(0, 0, 0, 15, 0),
        cell(0, 0, 0, 14, 0),
        cell(0, 0, 0, 6, 3),
    ];
    let mut i = ins4();
    i[1] = (0x0001, 0x0050, 0x0060, 0, 0xFF);
    let d = module(&i, [&[0, 0xFE, 0xFF], &[0, 0, 0], &[0, 0, 0], &[0, 0, 0]], &[cmds], 8, &[0; 16], 7);
    let (log, _) = run(&d, 600);
    assert!(!log.is_empty());
    // a module of zeros and one of ones, truncated everywhere: never a panic
    let mut m = Machine::new(CpuType::M68000);
    let good = simple();
    for cut in (0..good.len()).step_by(7) {
        if let Some(mut r) = (OPEN.new)(&good[..cut], BASE, &mut m) {
            for k in 0..5 {
                m.bus.clock += TICK * k;
                r.tick(&mut Hw { bus: &mut m.bus });
            }
        }
    }
    for fill in [0u8, 0xFF] {
        let mut e = good.clone();
        for b in e[0x38..].iter_mut().take(300) {
            *b = fill;
        }
        m.bus.write_bytes(BASE, &e);
        if let Some(mut r) = (OPEN.new)(&e, BASE, &mut m) {
            for k in 0..200 {
                m.bus.clock += TICK * k;
                r.tick(&mut Hw { bus: &mut m.bus });
            }
        }
    }
}
