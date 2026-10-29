//! Tests on synthetic modules (built here, no corpus bytes).

use super::*;

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

/// One pattern (H = 0), song length `l`, sample 1 of 8 words (volume 40, repeat 0/1), `cells`
/// as (row, channel, period, instrument-and-command byte, argument).
fn module(l: u16, cells: &[(usize, usize, u16, u8, u8)]) -> Vec<u8> {
    let mut d = vec![0u8; HEAD + PAT + 16];
    put16(&mut d, 0, 293);
    put16(&mut d, 2, l);
    put16(&mut d, 4, 0);
    put16(&mut d, 134 + 4, 8);
    put16(&mut d, 134 + 8, 1);
    put16(&mut d, 134 + 10, 40);
    put16(&mut d, 134 + 6, 0);
    // a period in the first cell, so that the pattern passes the detector
    for &(r, c, p, ic, a) in cells {
        let o = HEAD + r * 16 + c * 4;
        put16(&mut d, o, p);
        d[o + 2] = ic;
        d[o + 3] = a;
    }
    if be16(&d, HEAD) == 0 {
        put16(&mut d, HEAD, 428);
    }
    d
}

fn run(e: &mut Ems, n: usize) -> Vec<Vec<W>> {
    (0..n)
        .map(|_| {
            let mut w = Vec::new();
            e.tick(&mut w);
            w.retain(|x| !matches!(x, W::Wait(_)));
            w
        })
        .collect()
}

#[test]
fn detection() {
    let d = module(1, &[]);
    assert!(detect(&d));
    let mut x = d.clone();
    x[0] = 2;
    assert!(!detect(&x));
    let mut x = d.clone();
    put16(&mut x, 2, 0);
    assert!(!detect(&x));
    let mut x = d.clone();
    x[10] = 1; // a sequence entry above H
    assert!(!detect(&x));
    let mut x = d.clone();
    put16(&mut x, HEAD + 4, 429); // not in the period table
    assert!(!detect(&x));
    // sample data missing
    let mut x = d.clone();
    put16(&mut x, 134 + 4, 4000);
    assert!(!detect(&x));
}

#[test]
fn sample_starts_one_word_early_and_clears() {
    let mut d = module(1, &[]);
    put16(&mut d, 134 + 12 + 4, 3); // sample 2
    d.resize(HEAD + PAT + 32, 0);
    let (e, clear) = Ems::new(&d, 0x1000);
    assert_eq!(e.smp[1].0, 0x1000 + 1336);
    assert_eq!(e.smp[2].0, 0x1000 + 1336 + 16);
    assert_eq!(clear, vec![1336, 1352]);
    assert_eq!(e.smp[3].0, 0);
}

#[test]
fn row_lasts_speed_plus_one_ticks() {
    let d = module(2, &[(0, 0, 428, 0x10, 0)]);
    let (mut e, _) = Ems::new(&d, 0x1000);
    let t = run(&mut e, 14);
    // calls 1..6 are in-between ticks: nothing for 6 calls, then a row tick
    assert!(t[..6].iter().all(|w| w.is_empty()));
    assert_eq!(
        t[6],
        vec![
            W::Dma(1),
            W::Vol(0, 40),
            W::Per(0, 428),
            W::Loc(0, 0x1000 + 1336),
            W::Len(0, 8),
            W::Dma(0x8001)
        ]
    );
    // the next in-between tick queues the loop: the cleared word, repeat length 1
    assert_eq!(t[7], vec![W::Loc(0, 0x1000 + 1336), W::Len(0, 1)]);
    // the second row tick is 7 calls after the first; it clears the DMA of no channel
    assert_eq!(t[13], vec![W::Dma(0), W::Dma(0x8000)]);
}

#[test]
fn speed_command_and_song_end() {
    // row 0 sets speed 1 (command 9); the pattern is 64 rows, song length 1
    let d = module(1, &[(0, 0, 428, 0x09, 1)]);
    let (mut e, _) = Ems::new(&d, 0x1000);
    let mut ends = Vec::new();
    for k in 0..200 {
        let mut w = Vec::new();
        e.tick(&mut w);
        if e.songend && ends.is_empty() {
            ends.push(k);
        }
    }
    // first row on call 7 (index 6); then rows of 2 ticks: row 63 is the 64th row tick
    assert_eq!(ends, vec![6 + 2 * 63]);
}

#[test]
fn volume_slide_stops_at_zero() {
    let d = module(1, &[(0, 0, 428, 0x17, 15)]);
    let (mut e, _) = Ems::new(&d, 0x1000);
    let t = run(&mut e, 12);
    assert_eq!(t[6][1], W::Vol(0, 40));
    assert_eq!(t[7][2], W::Vol(0, 25));
    assert_eq!(t[8], vec![W::Vol(0, 10)]);
    assert_eq!(t[9], vec![W::Vol(0, 0)]);
    assert!(t[10].is_empty());
}

#[test]
fn period_slide_down_is_capped() {
    let d = module(1, &[(0, 0, 1700, 0x1C, 10)]);
    let (mut e, _) = Ems::new(&d, 0x1000);
    let t = run(&mut e, 10);
    // 1700 -> 1710, then 1720 is over the limit: nothing more is written
    assert!(t[7].contains(&W::Per(0, 1710)));
    assert!(!t[8].iter().any(|x| matches!(x, W::Per(..))));
}

#[test]
fn break_and_zero_speed() {
    // a break to row 5 on row 0 ends the pattern after row 0; command 9 with 0 changes nothing
    let d = module(2, &[(0, 0, 428, 0x08, 5), (0, 1, 0, 0x09, 0)]);
    let (mut e, _) = Ems::new(&d, 0x1000);
    run(&mut e, 7);
    assert_eq!((e.row, e.pos, e.speed), (5, 1, 6));
}
