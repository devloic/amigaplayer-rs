//! Tests on small containers built in code: the container rules (detection, chunk walk, INFO
//! fields), the first row's write sequence, the deviations from the built-in ProTracker (length
//! 0 for 1, ascending repeat block, retrigger's last period, the fine slide mask, F00, the INFO
//! tempo and flags), the timing of the tick and of the first row, and malformed input.

use super::*;
use m68k::CpuType;

const BASE: u32 = 0x10000;

/// A ProTracker cell: instrument, period, command, parameter.
fn cell(ins: u8, period: u16, cmd: u8, par: u8) -> [u8; 4] {
    [(ins & 0xF0) | (period >> 8) as u8, period as u8, (ins << 4) | cmd, par]
}

/// A module of `nv` voices: one song position per entry of `orders`, the samples
/// (length in words, repeat start, repeat length, volume, finetune) and the cells
/// (pattern, row, voice, cell).
fn module(nv: usize, orders: &[u8], samples: &[(u16, u16, u16, u8, u8)], cells: &[(usize, usize, usize, [u8; 4])]) -> Vec<u8> {
    let npat = *orders.iter().max().unwrap() as usize + 1;
    let mut m = vec![0u8; 1084];
    m[..4].copy_from_slice(b"test");
    for (i, s) in samples.iter().enumerate() {
        let o = 20 + 30 * i;
        m[o + 22..o + 24].copy_from_slice(&s.0.to_be_bytes());
        m[o + 24] = s.4;
        m[o + 25] = s.3;
        m[o + 26..o + 28].copy_from_slice(&s.1.to_be_bytes());
        m[o + 28..o + 30].copy_from_slice(&s.2.to_be_bytes());
    }
    m[950] = orders.len() as u8;
    m[952..952 + orders.len()].copy_from_slice(orders);
    m[1080..1084].copy_from_slice(if nv == 8 { b"8CHN" } else { b"M.K." });
    m.resize(1084 + npat * nv * 256, 0);
    for &(p, r, v, c) in cells {
        let o = 1084 + ((p * 64 + r) * nv + v) * 4;
        m[o..o + 4].copy_from_slice(&c);
    }
    for s in samples {
        for i in 0..s.0 as usize * 2 {
            m.push((i * 7) as u8);
        }
    }
    m
}

/// A ProTracker 4 container the way ProTracker 4 writes it: VERS with its size 0x16 for 18
/// bytes, INFO and CMNT counting their own header, PTDT not.
fn container(pt: &[u8], tempo: u16, flags: u8) -> Vec<u8> {
    let mut info = vec![0u8; 64];
    info[..4].copy_from_slice(b"song");
    info[0x28..0x2A].copy_from_slice(&tempo.to_be_bytes());
    info[0x2A] = flags;
    let mut body = b"MODL".to_vec();
    body.extend_from_slice(b"VERS");
    body.extend_from_slice(&0x16u32.to_be_bytes());
    body.extend_from_slice(b"\0\0\0\0PT4.00");
    body.extend_from_slice(b"INFO");
    body.extend_from_slice(&72u32.to_be_bytes());
    body.extend_from_slice(&info);
    body.extend_from_slice(b"CMNT");
    body.extend_from_slice(&72u32.to_be_bytes());
    body.extend_from_slice(&[0u8; 64]);
    body.extend_from_slice(b"PTDT");
    body.extend_from_slice(&(pt.len() as u32).to_be_bytes());
    body.extend_from_slice(pt);
    if pt.len() & 1 != 0 {
        body.push(0);
    }
    let mut f = b"FORM".to_vec();
    f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    f.extend_from_slice(&body);
    f
}

fn start(file: &[u8]) -> (Machine, Box<dyn Replayer>) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    m.bus.write_bytes(BASE, file);
    m.bus.clock = 100;
    let r = new(file, BASE, &mut m).expect("recognised");
    (m, r)
}

/// Runs `n` ticks the way the engine does: the first one a period plus the start delay after
/// the start, the next ones a period after the previous one's due time.
fn run(m: &mut Machine, r: &mut Box<dyn Replayer>, n: usize) -> Vec<u64> {
    let mut due = m.bus.clock + r.tick_clocks() + r.timer_delay();
    let mut entries = Vec::new();
    for _ in 0..n {
        let period = r.tick_clocks();
        m.bus.clock = due;
        entries.push(due);
        r.tick(&mut Hw { bus: &mut m.bus });
        due += period;
    }
    entries
}

/// The log as (register offset within the audio block or the name, value) pairs.
fn log(m: &Machine) -> Vec<(u32, u16, u64)> {
    m.bus.write_log.as_ref().unwrap().clone()
}

/// The writes from tick `from` on, as strings like "v0 PER 1ac" and "DMA 8001".
fn names(m: &Machine, skip: usize) -> Vec<String> {
    log(m)
        .iter()
        .skip(skip)
        .map(|&(a, v, _)| match a {
            0xDFF096 => format!("DMA {v:04x}"),
            _ => {
                let c = (a - 0xDFF0A0) / 16;
                let r = ["LOCH", "LOCL", "LEN", "PER", "VOL"][((a - 0xDFF0A0) % 16 / 2) as usize];
                format!("v{c} {r} {v:x}")
            }
        })
        .collect()
}

fn simple(cells: &[(usize, usize, usize, [u8; 4])]) -> Vec<u8> {
    module(4, &[0], &[(8, 0, 1, 64, 0), (4, 2, 2, 40, 3)], cells)
}

#[test]
fn detects_the_container_only() {
    let f = container(&simple(&[]), 125, 0x81);
    assert!(detect(&f));
    assert!(super::super::REPLAYERS.iter().filter(|o| (o.detect)(&f)).count() == 1);
    // a plain module, a PolyTracker file and short or damaged ones are not
    assert!(!detect(&simple(&[])));
    let mut ptm = vec![0u8; 2000];
    ptm[44..48].copy_from_slice(b"PTMF");
    assert!(!detect(&ptm));
    assert!(!detect(&f[..40]));
    assert!(!detect(&f[..f.len().min(1000)]));
    let mut g = f.clone();
    g[8..12].copy_from_slice(b"SMUS");
    assert!(!detect(&g));
    let mut g = f.clone();
    g[4..8].copy_from_slice(&0x7fff_ffffu32.to_be_bytes());
    assert!(!detect(&g));
}

#[test]
fn the_chunk_walk_follows_the_players_rules() {
    let f = container(&simple(&[]), 125, 0x81);
    let p = parse(&f).unwrap();
    // VERS (18) + INFO (72) + CMNT (72) + PTDT id and size: the data starts 8 bytes on
    assert_eq!(p.data, 12 + 18 + 72 + 72 + 8);
    assert_eq!((p.tempo, p.flags, p.name.as_str()), (125, 0x81, "song"));
    // a VERS size other than 0x16 starts the walk at the VERS chunk itself: the walk lands
    // inside the next chunk and finds nothing (the original then plays silence)
    let mut g = f.clone();
    g[16..20].copy_from_slice(&10u32.to_be_bytes());
    assert!(parse(&g).is_none());
    // a chunk of size 0 stops the walk
    let mut g = f.clone();
    g[34..38].copy_from_slice(&0u32.to_be_bytes());
    assert!(parse(&g).is_none());
    // no PTDT
    let mut g = f.clone();
    let at = f.windows(4).position(|w| w == b"PTDT").unwrap();
    g[at..at + 4].copy_from_slice(b"XXXX");
    assert!(parse(&g).is_none());
}

#[test]
fn start_sets_dma_off_and_the_timer_from_info() {
    for (tempo, want) in [(125, 70940), (0, 70940), (205, 43255), (90, 98525)] {
        let f = container(&simple(&[]), tempo, 0x81);
        let (m, r) = start(&f);
        assert_eq!(log(&m)[0].0, 0xDFF096);
        assert_eq!(log(&m)[0].1, 0x000F);
        assert_eq!(r.tick_clocks(), want, "tempo {tempo}");
    }
}

#[test]
fn first_row_comes_on_the_sixth_interrupt_in_the_players_order() {
    // voice 0: instrument 1, period 428 (index 12 of the table), voice 1: nothing
    let f = container(&simple(&[(0, 0, 0, cell(1, 428, 0, 0)), (0, 0, 2, cell(2, 428, 0, 0))]), 125, 0x81);
    let (mut m, mut r) = start(&f);
    run(&mut m, &mut r, 5);
    // five effect ticks: four volumes of 0 each
    let w = names(&m, 1);
    assert_eq!(w.len(), 20);
    assert!(w.iter().all(|s| s.ends_with("VOL 0")));
    let before = log(&m).len();
    run_one_more(&mut m, &mut r, 5);
    let w = names(&m, before);
    let s0 = BASE + (12 + 18 + 72 + 72 + 8 + 1084 + 1024) as u32;
    let s1 = s0 + 16;
    let mut want: Vec<String> = Vec::new();
    // voice 0: the empty previous cell's period, DMA off, length, location, period, period, volume
    want.extend(["v0 PER 0", "DMA 0001", "v0 LEN 8"].map(String::from));
    want.push(format!("v0 LOCH {:x}", s0 >> 16));
    want.push(format!("v0 LOCL {:x}", s0 & 0xFFFF));
    want.extend(["v0 PER 1ac", "v0 PER 1ac", "v0 VOL 40"].map(String::from));
    // (an empty cell after an empty cell: the period twice, the second from "more effects")
    want.extend(["v1 PER 0", "v1 PER 0", "v1 VOL 0"].map(String::from));
    // voice 2: finetune 3 gives period 420 + ..., length is repeat start + repeat length
    want.extend(["v2 PER 0", "DMA 0004", "v2 LEN 4"].map(String::from));
    want.push(format!("v2 LOCH {:x}", s1 >> 16));
    want.push(format!("v2 LOCL {:x}", s1 & 0xFFFF));
    let p2 = crate::protracker::PERIODS[3 * 37 + 12];
    want.extend([format!("v2 PER {p2:x}"), format!("v2 PER {p2:x}"), "v2 VOL 28".to_string()]);
    want.extend(["v3 PER 0", "v3 PER 0", "v3 VOL 0"].map(String::from));
    want.push("DMA 8005".to_string());
    // the repeat block, ascending: voice 0 (repeat start = sample start, length 1 written as 0),
    // voice 1 (nothing), voice 2 (repeat start 2 words in, length 2), voice 3
    let r2 = s1 + 4;
    for (v, loc, len) in [(0, s0, 0), (1, 0, 0), (2, r2, 2), (3, 0, 0)] {
        want.push(format!("v{v} LOCH {:x}", loc >> 16));
        want.push(format!("v{v} LOCL {:x}", loc & 0xFFFF));
        want.push(format!("v{v} LEN {len:x}"));
    }
    assert_eq!(w, want);
}

/// One more tick after `run` (which restarts its schedule): the sixth interrupt.
fn run_one_more(m: &mut Machine, r: &mut Box<dyn Replayer>, _k: usize) {
    m.bus.clock += r.tick_clocks();
    r.tick(&mut Hw { bus: &mut m.bus });
}

#[test]
fn the_first_tick_follows_the_start_by_a_period() {
    let f = container(&simple(&[]), 125, 0x81);
    let (mut m, mut r) = start(&f);
    let t0 = m.bus.clock;
    let e = run(&mut m, &mut r, 1);
    assert_eq!(e[0], t0 + 70940);
    // the first write of an effect tick is a volume 366 colour clocks into the interrupt
    assert_eq!(log(&m)[1].2, e[0] + 366);
    // and the next voice's volume 300 later
    assert_eq!(log(&m)[2].2, e[0] + 366 + 300);
}

#[test]
fn a_length_of_one_is_written_as_zero_and_the_dma_wait_is_ten_lines() {
    // sample 1 is 1 word long
    let pt = module(4, &[0], &[(1, 0, 1, 64, 0)], &[(0, 0, 0, cell(1, 428, 0, 0))]);
    let f = container(&pt, 125, 0x81);
    let (mut m, mut r) = start(&f);
    run(&mut m, &mut r, 6);
    let l = log(&m);
    let len = l.iter().position(|w| w.0 == 0xDFF0A4 && w.2 > 0).unwrap();
    // the first LEN write after the start is the note's length of 1, written as 0
    let first_len = l.iter().filter(|w| w.0 == 0xDFF0A4).next().unwrap();
    assert_eq!(first_len.1, 0, "{len}");
    let on = l.iter().position(|w| w.0 == 0xDFF096 && w.1 & 0x8000 != 0).unwrap();
    let t = l[on].2;
    // the repeat location comes after the wait to the start of the line ten lines on
    let wait_end = (t / 227 + 10) * 227;
    let next = l[on + 1].2;
    assert_eq!(next, wait_end + 130);
}

#[test]
fn retrigger_writes_a_final_period() {
    // E91 on every row after a note: a retrigger on the ticks where the counter divides by 1
    let cells = [(0, 0, 0, cell(1, 428, 0, 0)), (0, 1, 0, cell(0, 0, 0xE, 0x93))];
    let f = container(&module(4, &[0], &[(8, 0, 1, 64, 0)], &cells), 125, 0x81);
    let (mut m, mut r) = start(&f);
    run(&mut m, &mut r, 6 + 6 + 3);
    let w = names(&m, 0);
    // the retrigger: DMA off, LOC, LEN, PER, DMA on, LOC, LEN, PER (the last one is the deviation)
    let i = w.iter().rposition(|s| s == "DMA 8001").unwrap();
    assert!(w[i - 5].starts_with("DMA 0001") || w[i - 6].starts_with("DMA 0001"), "{:?}", &w[i - 8..i + 6]);
    let tail = &w[i + 1..i + 7];
    assert!(tail[0].starts_with("v0 LOCH") && tail[1].starts_with("v0 LOCL") && tail[2].starts_with("v0 LEN"), "{tail:?}");
    assert!(tail[3].starts_with("v0 PER 1ac"), "{tail:?}");
}

#[test]
fn the_first_porta_does_not_slide_and_f00_does_nothing() {
    // row 0: a note; row 1: 1 02 (slide up by 2: the fine slide mask starts at 0, so it does
    // not move the pitch the first time); row 2: F00, which must not end the song; row 3: 1 02
    let cells = [
        (0, 0, 0, cell(1, 428, 0, 0)),
        (0, 1, 0, cell(0, 0, 1, 2)),
        (0, 2, 0, cell(0, 0, 0xF, 0)),
        (0, 3, 0, cell(0, 0, 1, 2)),
    ];
    let f = container(&module(4, &[0], &[(8, 0, 1, 64, 0)], &cells), 125, 0x81);
    let (mut m, mut r) = start(&f);
    run(&mut m, &mut r, 6 * 5);
    let pers: Vec<String> = names(&m, 0).into_iter().filter(|s| s.starts_with("v0 PER")).map(|s| s[7..].to_string()).collect();
    // the empty cell's period, the note's (twice), then row 1: its row tick (428), the first
    // 1xx tick that does not move (the mask is 0) and then steps of 2
    assert_eq!(pers[..9], ["0", "1ac", "1ac", "1ac", "1ac", "1aa", "1a8", "1a6", "1a4"]);
    assert!(!r.songend());
}

#[test]
fn info_flag_decides_whether_fxx_sets_the_tempo() {
    let cells = [(0, 0, 0, cell(0, 0, 0xF, 0x40))];
    // flag bit 0 set: tempo 64 (tick period 5 * (1773447 / 64 + 1)); clear: speed 64
    for (flags, tempo_set) in [(0x81, true), (0x80, false)] {
        let f = container(&module(4, &[0], &[(8, 0, 1, 64, 0)], &cells), 125, flags);
        let (mut m, mut r) = start(&f);
        run(&mut m, &mut r, 6);
        let want = if tempo_set { 5 * (1773447 / 64 + 1) } else { 70940 };
        assert_eq!(r.tick_clocks(), want, "flags {flags:x}");
    }
}

#[test]
fn song_ends_when_the_position_wraps() {
    // a one-position song of 64 empty rows at speed 6: the end after 64 * 6 interrupts
    let f = container(&simple(&[]), 125, 0x81);
    let (mut m, mut r) = start(&f);
    run(&mut m, &mut r, 64 * 6 - 1);
    assert!(!r.songend());
    run_one_more(&mut m, &mut r, 1);
    assert!(r.songend());
}

#[test]
fn eight_voice_module_pokes_the_amplifier() {
    let pt = module(8, &[0], &[(8, 0, 1, 64, 0)], &[(0, 0, 5, cell(1, 428, 0, 0))]);
    let f = container(&pt, 125, 0x81);
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    m.bus.write_bytes(BASE, &f);
    m.bus.clock = 100;
    let Some(mut r) = new(&f, BASE, &mut m) else { panic!("8 voices not started") };
    run(&mut m, &mut r, 6);
    let l = log(&m);
    // every write goes to the amplifier's virtual voices (0xD00000 | voice << 4 | register)
    assert!(l.iter().all(|w| w.0 >> 20 == 0xD));
    assert!(l.iter().any(|w| w.0 == 0xD00058 && w.1 == 0x40));
    assert!(l.iter().any(|w| w.0 == 0xD0005A && w.1 == 0x8000));
}

#[test]
fn malformed_input_does_not_panic() {
    let cells = [(0, 0, 0, cell(1, 428, 0x9, 0x20)), (0, 1, 1, cell(1, 300, 0xE, 0x80)), (0, 2, 2, cell(40, 100, 0xD, 0xFF))];
    let good = container(&module(4, &[0, 5, 0], &[(8, 4, 3, 64, 15), (0, 0, 0, 0, 0)], &cells), 125, 0x81);
    // truncated at every length, and with every byte of the header region set to 0xFF
    for n in (0..good.len()).step_by(37) {
        let _ = detect(&good[..n]);
        let mut m = Machine::new(CpuType::M68000);
        m.bus.write_bytes(BASE, &good[..n]);
        if let Some(mut r) = new(&good[..n], BASE, &mut m) {
            run(&mut m, &mut r, 20);
        }
    }
    for i in (0..1300).step_by(7) {
        let mut g = good.clone();
        g[i] = 0xFF;
        let mut m = Machine::new(CpuType::M68000);
        m.bus.write_bytes(BASE, &g);
        if let Some(mut r) = new(&g, BASE, &mut m) {
            run(&mut m, &mut r, 200);
        }
    }
}

#[test]
fn runs_are_deterministic() {
    let cells = [(0, 0, 0, cell(1, 428, 0, 0)), (0, 1, 1, cell(2, 300, 4, 0x35)), (0, 2, 2, cell(1, 200, 0xA, 0x04))];
    let f = container(&simple(&cells), 125, 0x81);
    let one = |f: &[u8]| {
        let (mut m, mut r) = start(f);
        run(&mut m, &mut r, 300);
        log(&m)
    };
    assert_eq!(one(&f), one(&f));
}
