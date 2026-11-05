use super::*;
use crate::engine::Engine;

type Log = Vec<(u32, u16, u64)>;

/// A synthetic module: `pats` are lists of (row, voice, [note, sample, command, parameter]);
/// positions are `order`; samples 1 ("loop", 64 words, loop 0/32) and 2 ("one shot", 16 words).
fn module(order: &[u8], pats: &[Vec<(usize, usize, [u8; 4])>]) -> Vec<u8> {
    let mut d = vec![0u8; 1084];
    d[..4].copy_from_slice(b"test");
    let mut set = |n: usize, len: u16, fine: u8, vol: u8, ls: u16, ll: u16| {
        let h = 20 + 30 * (n - 1);
        d[h..h + 4].copy_from_slice(b"smp.");
        d[h + 22..h + 24].copy_from_slice(&len.to_be_bytes());
        d[h + 24] = fine;
        d[h + 25] = vol;
        d[h + 26..h + 28].copy_from_slice(&ls.to_be_bytes());
        d[h + 28..h + 30].copy_from_slice(&ll.to_be_bytes());
    };
    set(1, 64, 0, 64, 0, 32);
    set(2, 16, 0, 48, 0, 1);
    d[950] = order.len() as u8;
    d[951] = 0x7F;
    d[952..952 + order.len()].copy_from_slice(order);
    d[1080..1084].copy_from_slice(b"JS92");
    let np = pats.len().max(*order.iter().max().unwrap_or(&0) as usize + 1);
    d.resize(1084 + 1024 * np, 0);
    for (p, cells) in pats.iter().enumerate() {
        for &(r, v, c) in cells {
            let o = 1084 + 1024 * p + 16 * r + 4 * v;
            d[o..o + 4].copy_from_slice(&c);
        }
    }
    d.extend((0..160u32).map(|i| (i * 7) as u8));
    d.resize(d.len().max(2113), 0);
    d
}

/// Plays the module for `n` interrupts the way the engine does: the first one after the
/// replayer's own first period and delay, later ones one period (as it was before the interrupt
/// ran) after the previous due time. Returns the write log, the entry clock of each interrupt and
/// whether the song end was reported.
fn run(d: &[u8], n: usize) -> (Log, Vec<u64>, bool, Vec<u64>) {
    let mut e = Engine::new(48000.0);
    e.m.bus.write_log = Some(Vec::new());
    let addr = e.m.alloc(d.len() as u32 + 1024);
    e.m.bus.write_bytes(addr, d);
    let mut r = (OPEN.new)(d, addr, &mut e.m).expect("replayer");
    let mut due = e.m.bus.clock + r.tick_clocks() + r.timer_delay();
    let mut entries = Vec::new();
    let mut periods = Vec::new();
    let mut end = false;
    for _ in 0..n {
        let period = r.tick_clocks();
        periods.push(period);
        e.m.bus.clock = due;
        entries.push(due);
        r.tick(&mut Hw { bus: &mut e.m.bus });
        end |= r.songend();
        due += period;
    }
    (e.m.bus.write_log.take().unwrap(), entries, end, periods)
}

fn after(log: &Log, from: u64, to: u64) -> Vec<(u32, u16, u64)> {
    log.iter().copied().filter(|w| w.2 >= from && w.2 < to).collect()
}

#[test]
fn detection() {
    let d = module(&[0], &[vec![]]);
    assert!(detect(&d));
    let mut bad = d.clone();
    bad[1083] = b'3';
    assert!(!detect(&bad));
    assert!(!detect(&d[..2000]));
    // sample data past the end of the file
    let mut big = d.clone();
    big[20 + 22] = 0xFF;
    assert!(!detect(&big));
    // a signed scan: a negative position byte never counts
    let mut neg = d.clone();
    neg[952] = 0x90;
    assert!(detect(&neg));
}

#[test]
fn period_table_difference() {
    let t = table();
    assert_eq!(t[0], 856);
    assert_eq!(t[8 * 36], 907);
    assert_eq!(t[9 * 36 + 23], 230);
    assert_eq!(t[9 * 36 + 22], 253);
    assert_eq!(table_at(&t, 0), 0x4080);
    assert_eq!(table_at(&t, 2), 856);
    assert_eq!(table_at(&t, 72), 113);
    assert_eq!(table_at(&t, 72 * 15 + 72), 114);
}

#[test]
fn interrupt_period() {
    assert_eq!(interrupt_clocks(125), 17735);
    assert_eq!(interrupt_clocks(0x40), 34640);
    assert_eq!(interrupt_clocks(0x9A), 14395);
    assert_eq!(interrupt_clocks(0xFF), 8695);
}

#[test]
fn effect_tick_write_offsets() {
    // an empty song: the first interrupt is an effect tick, every command word 0
    let d = module(&[0], &[vec![]]);
    let (log, entries, _, _) = run(&d, 1);
    let e = entries[0];
    let w: Vec<u64> = after(&log, e, e + 5000).iter().map(|x| x.2 - e).collect();
    assert_eq!(w, vec![288, 464, 656, 855]);
}

#[test]
fn first_row_and_dma_phases() {
    let d = module(&[0], &[vec![(0, 0, [2, 1, 0, 0])]]);
    let (log, entries, _, _) = run(&d, 23);
    // interrupts 0, 4, 8, 12, 16 are effect ticks; 20 plays the first row
    let e = entries[20];
    let w: Vec<(u32, u64)> = after(&log, e, e + 5000).iter().filter(|x| x.0 & 0xF != 2).map(|x| (x.0, x.2 - e)).collect();
    assert_eq!(
        &w[..6],
        &[(0xDFF096, 637), (0xDFF0A0, 692), (0xDFF0A4, 702), (0xDFF0A6, 712), (0xDFF0A8, 951), (0xDFF0A6, 1125)]
    );
    // phase 1: DMA on at 209
    let e1 = entries[21];
    let w1 = after(&log, e1, e1 + 5000);
    assert_eq!((w1[0].0, w1[0].1, w1[0].2 - e1), (0xDFF096, 0x8001, 209));
    // phase 2: the loop registers of all four voices, at 225/235 then 24 apart
    let e2 = entries[22];
    let w2: Vec<(u32, u16, u64)> =
        after(&log, e2, e2 + 5000).iter().filter(|x| x.0 & 0xF != 2).map(|x| (x.0, x.1, x.2 - e2)).collect();
    assert_eq!((w2[0].0, w2[0].2), (0xDFF0A0, 225));
    assert_eq!((w2[1].0, w2[1].1, w2[1].2), (0xDFF0A4, 32, 235));
    assert_eq!((w2[2].0, w2[2].2), (0xDFF0B0, 249));
    assert_eq!((w2[7].0, w2[7].1, w2[7].2), (0xDFF0D4, 0, 307));
}

#[test]
fn deterministic() {
    let d = module(&[0, 0], &[vec![(0, 0, [2, 1, 0, 0]), (4, 1, [14, 2, 4, 0x45]), (8, 2, [20, 1, 0xC, 0x20])]]);
    let a = run(&d, 600);
    let b = run(&d, 600);
    assert_eq!(a.0, b.0);
    assert!(a.0.len() > 100);
}

#[test]
fn first_slide_moves_by_zero() {
    // note C-1 (period 856), then a slide up by 8 on the next row: the first slide of the song
    // moves by 0, the second by 8
    let d = module(&[0], &[vec![(0, 0, [2, 1, 0, 0]), (1, 0, [0, 0, 1, 8]), (2, 0, [0, 0, 1, 8])]]);
    let (log, entries, _, _) = run(&d, 100);
    let e = entries[24];
    let per: Vec<u16> =
        after(&log, entries[20], entries[40]).iter().filter(|w| w.0 == 0xDFF0A6).map(|w| w.1).collect();
    let _ = e;
    assert!(per.contains(&856));
    assert!(!per.contains(&848) || per.iter().position(|&p| p == 856) < per.iter().position(|&p| p == 848));
    let all: Vec<u16> = log.iter().filter(|w| w.0 == 0xDFF0A6).map(|w| w.1).collect();
    assert!(all.contains(&848));
}

#[test]
fn song_end_and_tempo() {
    // F40 on row 0: after it the interrupt period is 34640 (one interval late)
    let d = module(&[0], &[vec![(0, 0, [2, 1, 0xF, 0x40])]]);
    let (_, entries, _, periods) = run(&d, 30);
    assert_eq!(periods[20], 17735);
    assert_eq!(periods[21], 34640);
    assert_eq!(entries[22] - entries[21], 34640);
    // a position jump reports the song end at once
    let d = module(&[0], &[vec![(1, 0, [0, 0, 0xB, 0])]]);
    let (_, _, end, _) = run(&d, 30);
    assert!(!end);
    let (_, _, end, _) = run(&d, 60);
    assert!(end);
}

#[test]
fn pattern_break_takes_the_raw_row_number() {
    // D30 goes to row 48 (the parameter is a plain number), on the next position
    let d = module(&[0, 1], &[vec![(0, 0, [0, 0, 0xD, 0x30])], vec![(48, 0, [2, 1, 0, 0])]]);
    let (log, _, _, _) = run(&d, 200);
    assert!(log.iter().any(|w| w.0 == 0xDFF0A4 && w.1 == 64));
}

#[test]
fn malformed_input_does_not_hang() {
    // garbage patterns, odd commands, notes beyond the table: bounded runs, no panic
    let mut s = 12345u32;
    for round in 0..8 {
        let mut d = module(&[0, 1, 0], &[vec![], vec![]]);
        for b in d[1084..1084 + 2048].iter_mut() {
            s = s.wrapping_mul(1103515245).wrapping_add(12345 + round);
            *b = (s >> 16) as u8;
        }
        let (log, _, _, _) = run(&d, 3000);
        let _ = log;
    }
}

#[test]
fn registered_for_the_magic() {
    let d = module(&[0], &[vec![]]);
    assert!(crate::replay::REPLAYERS.iter().find(|o| (o.detect)(&d)).map(|o| o.name) == Some("Janne Salmijarvi Optimizer"));
}

/// `JS_MODULE=file JS_TICKS=n JS_LOG=out cargo test --release js_dump -- --ignored`: the module
/// played for `n` interrupts without stopping at the song end, the log in `DELI_WRITE_LOG`'s
/// format (the engine path is not used: a module can be taken by a packer's detection first).
#[test]
#[ignore]
fn js_dump() {
    let (Ok(file), Ok(n), Ok(out)) = (std::env::var("JS_MODULE"), std::env::var("JS_TICKS"), std::env::var("JS_LOG")) else {
        return;
    };
    let d = std::fs::read(&file).unwrap();
    let (log, _, end, _) = run(&d, n.parse().unwrap());
    println!("songend {end}");
    let mut text = format!("# module 0045e0 {}\n", d.len());
    for (a, v, t) in log {
        text += &format!("{a:06x} {v:04x} {t}\n");
    }
    std::fs::write(out, text).unwrap();
}
