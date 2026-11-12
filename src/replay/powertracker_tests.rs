use super::*;
use m68k::CpuType;

const BASE: u32 = 0x20000;
/// A PAL frame in colour clocks (the CIA timer of the host).
const TICK: u64 = 70_940;

/// A track row: note, volume byte, instrument, effect, parameter.
type Row = (u8, u8, u8, u8, u8);

/// A module of the archive modules' layout (SPEC section 2): records (word 0, word 1, word 2,
/// word 3), four lists of equal length, `ntracks` tracks (the rows given, the rest empty: note 0,
/// volume FF), then `tail`.
fn module(recs: &[(u16, u16, u16, u16)], lists: [&[u8]; 4], tracks: &[(usize, &[Row])], ntracks: usize, tail: &[u8]) -> Vec<u8> {
    let len = lists[0].len();
    let p = 8 + 24 * recs.len() + 2 + 4 * len;
    let mut d = Vec::new();
    for w in [p as u16, (24 * recs.len()) as u16, len as u16, 0] {
        d.extend_from_slice(&w.to_be_bytes());
    }
    for r in recs {
        d.extend_from_slice(&[0; 16]);
        for w in [r.0, r.1, r.2, r.3] {
            d.extend_from_slice(&w.to_be_bytes());
        }
    }
    d.extend_from_slice(&[1, 0]);
    for l in lists {
        assert_eq!(l.len(), len);
        d.extend_from_slice(l);
    }
    assert_eq!(d.len(), p);
    for t in 0..ntracks {
        let rows = tracks.iter().find(|x| x.0 == t).map(|x| x.1).unwrap_or(&[]);
        for i in 0..64 {
            let (n, v, ins, fx, par) = rows.get(i).copied().unwrap_or((0, 0xFF, 0, 0, 0));
            d.extend_from_slice(&[n, v, (ins << 4) | (fx & 15), par]);
        }
    }
    d.extend_from_slice(tail);
    d
}

const SAMPLE: (u16, u16, u16, u16) = (8, 2, 4, 0);
const PULSE: (u16, u16, u16, u16) = (0, 2, 4, 0xFF);

fn start(d: &[u8]) -> (Machine, PowerTracker) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_bytes(BASE, d);
    let r = build(d, BASE, &mut m).expect("module accepted");
    m.bus.write_log = Some(Vec::new());
    (m, r)
}

/// One tick's writes: register, value, colour clock.
type Writes = Vec<(u32, u16, u64)>;

/// The writes of each tick.
fn run(d: &[u8], ticks: usize) -> (Vec<Writes>, PowerTracker, Machine) {
    let (mut m, mut r) = start(d);
    let mut out = Vec::new();
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        m.bus.write_log = Some(Vec::new());
        r.tick(&mut Hw { bus: &mut m.bus });
        out.push(m.bus.write_log.take().unwrap());
    }
    (out, r, m)
}

fn regs(t: &[(u32, u16, u64)]) -> Vec<(u32, u16)> {
    t.iter().filter(|w| (0xDFF000..0xDFF200).contains(&w.0)).map(|w| (w.0, w.1)).collect()
}

fn last(t: &[(u32, u16, u64)], reg: u32) -> Option<u16> {
    t.iter().rev().find(|w| w.0 == reg).map(|w| w.1)
}

/// The pulse width (the count of 0 bytes) in voice `k`'s buffer.
fn width(r: &PowerTracker, m: &mut Machine, k: usize) -> usize {
    (0..32).filter(|&i| m.bus.read_byte(r.buffer(k) + i) == 0).count()
}

#[test]
fn detect_accepts_the_layout_and_rejects_changes() {
    let d = module(&[SAMPLE], [&[0], &[0], &[0], &[0]], &[], 1, &[0; 16]);
    assert!(detect(&d));
    let set = |o: usize, v: u16| {
        let mut x = d.clone();
        x[o..o + 2].copy_from_slice(&v.to_be_bytes());
        x
    };
    assert!(!detect(&set(0, be16(&d, 0) + 2))); // the tracks' offset off the layout
    assert!(!detect(&set(2, 23))); // records not a multiple of 24
    assert!(!detect(&set(2, 0)));
    assert!(!detect(&set(4, 0))); // no positions
    assert!(!detect(&set(6, 1))); // restart position past the list
    assert!(!detect(&set(8 + 24, 0x0200))); // the two bytes before the lists not 01 00
    assert!(!detect(&set(8 + 24, 0x0101)));
    assert!(!detect(&d[..be16(&d, 0) as usize - 1])); // cut before the tracks
    assert!(detect(&d[..be16(&d, 0) as usize]));
    assert!(!detect(&[]));
}

#[test]
fn sample_note_writes_volume_pointer_length_period_then_dma_and_loop() {
    let d = module(&[SAMPLE], [&[0], &[0xFF], &[0xFF], &[0xFF]], &[(0, &[(0x19, 0x30, 1, 0, 0)])], 1, &[0; 64]);
    let (t, r, _) = run(&d, 2);
    // the samples start after the tracks up to the highest list byte, FF giving none (SPEC
    // section 3): here at the tracks
    let smp = BASE + r.lay.tracks as u32;
    let w = regs(&t[0]);
    let want_head = [(0xDFF0A8, 0x30), (0xDFF0A0, (smp >> 16) as u16), (0xDFF0A2, smp as u16), (0xDFF0A4, 8), (0xDFF0A6, 856)];
    assert_eq!(&w[..5], &want_head);
    // the other voices: volume 0, then DMA off for voice 0, DMA on, the loops
    assert_eq!(&w[5..8], &[(0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0)]);
    assert_eq!(w[8], (0xDFF096, 0x0001));
    assert_eq!(w[9], (0xDFF096, 0x8001));
    let lp = smp + 2 * 2;
    assert_eq!(&w[10..13], &[(0xDFF0A0, (lp >> 16) as u16), (0xDFF0A2, lp as u16), (0xDFF0A4, 4)]);
    // writes are in time order, the loop after the host's two waits
    assert!(t[0].windows(2).all(|x| x[0].2 <= x[1].2));
    // an effect tick: DMA on again, the loops again, no new note
    assert_eq!(regs(&t[1])[0], (0xDFF096, 0x8001));
}

#[test]
fn list_byte_ff_silences_and_fe_leaves_the_voice() {
    let d = module(&[SAMPLE], [&[0], &[0xFF], &[0xFE], &[0]], &[(0, &[(0x19, 0x30, 1, 0, 0)])], 1, &[0; 64]);
    let (t, _, _) = run(&d, 1);
    let vols: Vec<u32> = regs(&t[0]).iter().filter(|w| (0xDFF0A8..=0xDFF0D8).contains(&w.0) && w.0 & 15 == 8).map(|w| w.0).collect();
    assert_eq!(vols, [0xDFF0A8, 0xDFF0B8, 0xDFF0D8]);
}

#[test]
fn pulse_wave_sweeps_between_its_bounds() {
    let d = module(&[PULSE], [&[0], &[0xFF], &[0xFF], &[0xFF]], &[(0, &[(0x19, 0x30, 1, 0, 0)])], 1, &[0; 64]);
    let (mut m, mut r) = start(&d);
    let mut w = Vec::new();
    for k in 0..8 {
        m.bus.clock = 1_000_000 + TICK * k;
        r.tick(&mut Hw { bus: &mut m.bus });
        w.push(width(&r, &mut m, 0));
    }
    assert_eq!(w, [3, 4, 4, 3, 2, 2, 3, 4]);
    // a pulse note plays the voice's buffer, 16 words, and loops it
    let (t, r, _) = run(&d, 1);
    let b = r.buffer(0);
    assert_eq!(last(&t[0], 0xDFF0A2), Some(b as u16));
    assert_eq!(last(&t[0], 0xDFF0A4), Some(16));
}

#[test]
fn pulse_delay_is_a_signed_byte_counter() {
    // delay 2: a step every 3 ticks; 80: every 129; 81 to FF: every tick (SPEC section 7)
    for (delay, want) in [(2u16, vec![0, 3, 6]), (0x80, vec![0, 129]), (0xC0, vec![0, 1, 2]), (0x101, vec![0, 2, 4])] {
        let d = module(&[(delay, 4, 0x20, 0xFF)], [&[0], &[0xFF], &[0xFF], &[0xFF]], &[(0, &[(0x19, 0x30, 1, 0, 0)])], 1, &[0; 64]);
        let (mut m, mut r) = start(&d);
        let mut steps = Vec::new();
        let mut prev = width(&r, &mut m, 0);
        for k in 0..140u64 {
            m.bus.clock = 1_000_000 + TICK * k;
            r.tick(&mut Hw { bus: &mut m.bus });
            let w = width(&r, &mut m, 0);
            if w != prev {
                steps.push(k);
            }
            prev = w;
        }
        assert_eq!(&steps[..want.len()], &want[..], "delay {delay:#x}");
    }
}

#[test]
fn detune_steps_by_the_note_and_keeps_its_offset_in_the_arpeggio() {
    for (b6, note, step) in [(3i8, 0x19u8, 6i16), (-2, 0x01, 24), (1, 0x2E, 1), (5, 0x0C, 12)] {
        let rec = (8, 0, 4, (b6 as u8 as u16) << 8);
        let d = module(&[rec], [&[0], &[0xFF], &[0xFF], &[0xFF]], &[(0, &[(note, 0x30, 1, 0, 0x37)])], 1, &[0; 64]);
        let (t, _, _) = run(&d, 3);
        let off = (b6 as i16 * step) as u16;
        assert_eq!(last(&t[0], 0xDFF0A6), Some(period(note).wrapping_add(off)), "{b6} {note:#x}");
        // the arpeggio's notes take the triggered note's offset
        assert_eq!(last(&t[1], 0xDFF0A6), Some(period(note + 7).wrapping_add(off)));
        assert_eq!(last(&t[2], 0xDFF0A6), Some(period(note + 3).wrapping_add(off)));
    }
}

#[test]
fn volume_fe_breaks_on_every_tick_and_the_wrap_ends_the_song() {
    // lists of 3 positions; FE on row 1: the position steps on each tick of that row, the
    // third step wraps (SPEC section 6.1)
    let rows: [Row; 2] = [(0x19, 0x30, 1, 0, 0), (0, 0xFE, 0, 0, 0)];
    let d = module(&[SAMPLE], [&[0, 1, 2], &[0xFF; 3], &[0xFF; 3], &[0xFF; 3]], &[(0, &rows)], 3, &[0; 64]);
    let (mut m, mut r) = start(&d);
    let mut ends = Vec::new();
    for k in 0..12u64 {
        m.bus.clock = 1_000_000 + TICK * k;
        r.tick(&mut Hw { bus: &mut m.bus });
        if r.songend_call {
            ends.push(k);
        }
    }
    assert_eq!(ends, [8, 11]);
    assert!(r.songend());
}

#[test]
fn jumps_and_breaks() {
    // B 00 sets the low byte of the position to FF and steps on: position 256
    let d = module(&[SAMPLE], [&[0, 0], &[0xFF; 2], &[0xFF; 2], &[0xFF; 2]], &[(0, &[(0x19, 0x30, 1, 0xB, 0)])], 1, &[0; 64]);
    let (_, r, _) = run(&d, 1);
    assert_eq!((r.pos, r.row), (256, 0));
    // B 01: position 1; D: the next position, row 0; B to the list's length: the restart
    for (fx, par, pos) in [(0xB, 1, 1), (0xD, 0x20, 1), (0xB, 2, 0)] {
        let d = module(&[SAMPLE], [&[0, 0], &[0xFF; 2], &[0xFF; 2], &[0xFF; 2]], &[(0, &[(0x19, 0x30, 1, fx, par)])], 1, &[0; 64]);
        let (_, r, _) = run(&d, 1);
        assert_eq!((r.pos, r.row), (pos, 0), "{fx:x} {par:02x}");
        assert_eq!(r.songend(), pos == 0);
    }
}

#[test]
fn speed_effect_and_volume_slides() {
    // F 02: two ticks a row; volume byte 83: +3 each tick, to 40 at most
    let rows: [Row; 3] = [(0x19, 0x3C, 1, 0xF, 2), (0, 0x83, 0, 0, 0), (0, 0x92, 0, 0, 0)];
    let d = module(&[SAMPLE], [&[0], &[0xFF], &[0xFF], &[0xFF]], &[(0, &rows)], 1, &[0; 64]);
    let (t, _, _) = run(&d, 6);
    let v: Vec<Option<u16>> = t.iter().map(|x| last(x, 0xDFF0A8)).collect();
    assert_eq!(v, [Some(0x3C), None, Some(0x3F), Some(0x40), Some(0x3E), Some(0x3C)]);
}

#[test]
fn malformed_input_does_not_panic_or_hang() {
    // wild rows (every effect, signed and missing tracks, notes past the table, instruments past
    // the records), wild records, a short tail
    let mut seed = 12345u32;
    let mut rnd = move || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (seed >> 16) as u8
    };
    for _ in 0..40 {
        let recs: Vec<(u16, u16, u16, u16)> =
            (0..3).map(|_| (rnd() as u16 * 3, rnd() as u16, rnd() as u16 * 2, (rnd() as u16) << 8 | rnd() as u16)).collect();
        let rows: Vec<Row> = (0..64).map(|_| (rnd(), rnd(), rnd() & 15, rnd() & 15, rnd())).collect();
        let l = [rnd(), rnd() & 1, 0xFF, rnd()];
        let d = module(&recs, [&[0, l[0]], &[l[1], 0], &[l[2], 0xFE], &[0, l[3]]], &[(0, &rows), (1, &rows[5..])], 2, &[rnd(); 3]);
        if !detect(&d) {
            continue;
        }
        let (mut m, mut r) = start(&d);
        for k in 0..400u64 {
            m.bus.clock = 1_000_000 + TICK * k;
            r.tick(&mut Hw { bus: &mut m.bus });
        }
    }
    // random files are not modules
    for n in [0usize, 1, 10, 100, 900, 5000] {
        let r: Vec<u8> = (0..n as u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        assert!(!detect(&r));
    }
}

#[test]
fn library_names_the_format() {
    let d = module(&[SAMPLE, PULSE], [&[0, 1], &[1, 0], &[0xFF; 2], &[0, 0]], &[(0, &[(0x19, 0x30, 1, 0, 0)])], 2, &[0; 64]);
    assert_eq!(crate::detect(&d).as_deref(), Some("PowerTracker"));
}

#[test]
fn real_modules_when_present() {
    // the two modules of modland's Powertracker/Merge (Kefrens intros)
    let dir = "/media/lolo/kingston2tb/claude-scratch/deliplayerrust/dl-modules/PowerTracker/files";
    let mut n = 0;
    for name in ["graphics are none.pt", "inspiration is gone.pt"] {
        let Ok(d) = std::fs::read(format!("{dir}/{name}")) else { continue };
        assert!(detect(&d), "{name}");
        assert!(!crate::open::claimed_by_several(&d), "{name}");
        let (t, _, _) = run(&d, 300);
        assert!(t.iter().flatten().any(|w| [0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8].contains(&w.0) && w.1 > 0));
        n += 1;
    }
    assert!(n == 0 || n == 2);
}

/// `PT_LISTS=list.txt[:list.txt...]`: no file named in the lists (one path a line) is taken.
/// Run with `--ignored`.
#[test]
#[ignore]
fn takes_no_module_of_the_corpus_lists() {
    let Ok(lists) = std::env::var("PT_LISTS") else { return };
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
    assert!(taken.is_empty(), "{taken:?}");
}
