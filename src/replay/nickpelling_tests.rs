use super::*;
use m68k::CpuType;

fn put16(d: &mut Vec<u8>, v: u16) {
    d.extend_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut Vec<u8>, v: u32) {
    d.extend_from_slice(&v.to_be_bytes());
}

/// Packs a pattern with literal runs only, then the end marker.
fn pack_literal(p: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    for c in p.chunks(128) {
        o.push((c.len() - 1) as u8);
        o.extend_from_slice(c);
    }
    o.extend_from_slice(&[0x80, 0]);
    o
}

/// One 64-row pattern with `cells` = (row, channel, 4 bytes).
fn pattern(cells: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
    let mut p = vec![0u8; 1024];
    for &(r, c, b) in cells {
        p[r * 16 + c * 4..r * 16 + c * 4 + 4].copy_from_slice(&b);
    }
    p
}

/// A song file: pattern bodies (already packed), positions, sample names.
fn song(packed: &[Vec<u8>], positions: &[u8], names: &[&str]) -> Vec<u8> {
    let p = packed.len();
    let n = 4 * (p + 3);
    let mut head = Vec::new();
    head.extend_from_slice(b"COMP");
    put32(&mut head, n as u32);
    let mut tail = Vec::new();
    tail.extend_from_slice(b"test\0");
    for i in 0..31 {
        tail.extend_from_slice(names.get(i).unwrap_or(&"").as_bytes());
        tail.push(0);
    }
    let q = n + tail.len();
    put32(&mut head, q as u32);
    let mut o = q + 1 + positions.len();
    let mut offs = Vec::new();
    for b in packed {
        offs.push(o as u32);
        o += b.len();
    }
    for &v in &offs {
        put32(&mut head, v);
    }
    let mut d = head;
    d.extend_from_slice(&tail);
    d.push(positions.len() as u8);
    d.extend_from_slice(positions);
    for b in packed {
        d.extend_from_slice(b);
    }
    d
}

struct Smp<'a> {
    name: &'a str,
    len: u16,
    fine: u8,
    vol: u8,
    rstart: u16,
    rlen: u16,
}

/// A sample set; the data of each entry is its index + 1 repeated (or `data` when given).
fn set(entries: &[Smp], data: &[u8]) -> Vec<u8> {
    let mut d = Vec::new();
    put16(&mut d, entries.len() as u16);
    for e in entries {
        let mut n = e.name.as_bytes().to_vec();
        n.resize(22, 0);
        d.extend_from_slice(&n);
        put16(&mut d, e.len);
        d.push(e.fine);
        d.push(e.vol);
        put16(&mut d, e.rstart);
        put16(&mut d, e.rlen);
    }
    d.extend_from_slice(data);
    d
}

fn plain_set() -> Vec<u8> {
    let data: Vec<u8> = (0..40).map(|i| i as u8).collect();
    set(
        &[
            Smp { name: "a", len: 10, fine: 0, vol: 64, rstart: 2, rlen: 8 },
            Smp { name: "b", len: 10, fine: 3, vol: 100, rstart: 0, rlen: 1 },
        ],
        &data,
    )
}

fn run_ticks(r: &mut NickPelling, ticks: usize) -> Vec<(u32, u16, u64)> {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    for k in 0..ticks {
        m.bus.clock = 1_000_000 + TICK * k as u64;
        let mut hw = Hw { bus: &mut m.bus };
        r.tick(&mut hw);
    }
    m.bus.write_log.take().unwrap()
}

fn module(cells: &[(usize, usize, [u8; 4])]) -> (Vec<u8>, Vec<u8>) {
    let d = song(&[pack_literal(&pattern(cells))], &[0], &["a", "b"]);
    (d, plain_set())
}

fn player(cells: &[(usize, usize, [u8; 4])]) -> NickPelling {
    let (d, s) = module(cells);
    let mut r = NickPelling::build(&d, &s).unwrap().0;
    r.unpack();
    r
}

#[test]
fn detect_rules() {
    let (d, _) = module(&[]);
    assert!(detect(&d));
    let mut e = d.clone();
    e[0] = b'X';
    assert!(!detect(&e));
    let mut e = d.clone();
    e[5] = 1;
    assert!(!detect(&e));
    let mut e = d.clone();
    e[7] = 0x0F; // N below 0x10
    assert!(!detect(&e));
    let mut e = d.clone();
    e[7] = 0x15; // not a multiple of 4
    assert!(!detect(&e));
    assert!(!detect(&d[..10]));
    assert!(!detect(&[]));
}

#[test]
fn unpack_literal_runs_and_overlapping_back_references() {
    // four literal bytes, then 8 bytes copied from 2 back (overlapping), the rest literal zeros
    let mut packed = vec![3, 1, 2, 3, 4, 0xFE, 8];
    let mut rest = vec![0u8; 1012];
    let mut zeros = Vec::new();
    for c in rest.chunks_mut(128) {
        zeros.push((c.len() - 1) as u8);
        zeros.extend_from_slice(c);
    }
    packed.extend_from_slice(&zeros);
    packed.extend_from_slice(&[0x80, 0]);
    let d = song(&[packed], &[0], &[]);
    let mut r = NickPelling::build(&d, &plain_set()).unwrap().0;
    let cost = r.unpack();
    assert_eq!(&r.buf[..12], &[1, 2, 3, 4, 3, 4, 3, 4, 3, 4, 3, 4]);
    assert_eq!(r.buf[12], 0);
    // 1024 bytes, 1 + 8 literal runs, 1 back reference
    let lits = 1 + 1012usize.div_ceil(128);
    assert_eq!(cost, 1024.0 * 6.0 + lits as f64 * 29.5 + 36.0);
}

#[test]
fn pattern_number_past_the_table_is_empty() {
    let mut d = song(&[pack_literal(&pattern(&[(0, 0, [1, 2, 3, 4])]))], &[7], &[]);
    // position 0 names pattern 7 of 1
    let mut r = NickPelling::build(&d, &plain_set()).unwrap().0;
    assert_eq!(r.unpack(), 0.0);
    assert!(r.buf.iter().all(|&b| b == 0));
    d.truncate(40);
    let _ = NickPelling::build(&d, &plain_set());
}

#[test]
fn sample_slots_resolve_by_name() {
    let data: Vec<u8> = (0..200).map(|i| i as u8).collect();
    let s = set(
        &[
            Smp { name: "one", len: 10, fine: 0x83, vol: 100, rstart: 2, rlen: 8 },
            Smp { name: "two", len: 5, fine: 0, vol: 30, rstart: 0, rlen: 3 },
            Smp { name: "alias", len: 0, fine: 0, vol: 0, rstart: 1, rlen: 0 },
            Smp { name: "three", len: 7, fine: 0x0F, vol: 10, rstart: 1, rlen: 6 },
        ],
        &data,
    );
    let d = song(&[pack_literal(&pattern(&[]))], &[0], &["one", "", "two", "nosuch", "alias", "one"]);
    let (r, samples) = NickPelling::build(&d, &s).unwrap();
    let ds = 2 + 30 * 4;
    let sl = |i: usize| samples.slots[i];
    // found: address = data start + earlier lengths; volume capped; finetune low nibble; repeat
    assert_eq!(sl(1), Rec { addr: ds as u32, len: 10, raddr: (ds + 4) as u32, rlen: 8, vol: 64, fine: 3 });
    // empty name
    assert_eq!(sl(2), Rec::default());
    // repeat length under 4 words: restart at the start, length 2, first four bytes zeroed
    assert_eq!(sl(3), Rec { addr: (ds + 20) as u32, len: 5, raddr: (ds + 20) as u32, rlen: 2, vol: 30, fine: 0 });
    assert_eq!(&samples.data[ds + 20..ds + 24], &[0, 0, 0, 0]);
    // missing
    assert_eq!(sl(4), Rec { addr: 0, len: 2, raddr: 0, rlen: 2, vol: 0, fine: 0 });
    // alias: repeat start 1 -> entry 2 ("two"... counted from 0 + 1 = entry 2 is "alias" itself?)
    // n = 1: the record is built from entry n + 1 = 2, whose length is 0: it is entry 2's own
    // record (length 0), at the address after entries 1..n
    assert_eq!(sl(5).len, 0);
    assert_eq!(sl(5).addr, (ds + 2 * 5) as u32);
    // the same name again resolves again, delta decoding only once
    assert_eq!(sl(6).addr, sl(1).addr);
    assert!(r.slots[1].addr == sl(1).addr);
    // delta: bytes after the first are running sums of the original 1, 2, 3 ...
    assert_eq!(samples.data[ds], 0);
    assert_eq!(samples.data[ds + 1], 1);
    assert_eq!(samples.data[ds + 2], 3);
    assert_eq!(samples.data[ds + 3], 6);
    // the bit is cleared in the set so that decoding does not repeat
    assert_eq!(samples.data[2 + 24] & 0x80, 0);
}

#[test]
fn sample_set_validity() {
    let d = song(&[pack_literal(&pattern(&[]))], &[0], &["a"]);
    let good = plain_set();
    assert!(NickPelling::build(&d, &good).is_some());
    assert!(NickPelling::build(&d, &good[..good.len() - 1]).is_none());
    assert!(NickPelling::build(&d, &[0, 0]).is_none());
    let mut many = good.clone();
    many[1] = 32;
    assert!(NickPelling::build(&d, &many).is_none());
    assert!(NickPelling::build(&d, &[]).is_none());
}

#[test]
fn table_reads_outside_the_period_table() {
    assert_eq!(period_at(0), 856);
    assert_eq!(period_at(35), 113);
    assert_eq!(period_at(36), 850);
    assert_eq!(period_at(575), 114);
    assert_eq!(period_at(-1), 0x4080);
    assert_eq!(period_at(-2), 0x202B);
    assert_eq!(period_at(-3), 0x161A);
    assert_eq!(period_at(576), 0x0001);
    assert_eq!(period_at(577), 0x0203);
    assert_eq!(period_at(580), 0x0809);
    assert_eq!(period_at(581), 0xFFFF);
    assert_eq!(period_at(704), 0x4646);
    assert_eq!(period_at(5000), 0);
    assert_eq!(period_at(-100), 0);
    assert_eq!(note_of(856), 0);
    assert_eq!(note_of(113), 35);
    assert_eq!(note_of(0), 35);
    assert_eq!(note_of(500), 9);
}

#[test]
fn first_ticks_write_the_idle_pairs_and_the_first_row_comes_on_the_sixth() {
    let mut r = player(&[(0, 1, [0x01, 0xAC, 0x10, 0x00])]);
    let l = run_ticks(&mut r, 5);
    // five ticks of volume 0 / period 0x7C pairs on four channels
    assert_eq!(l.len(), 5 * 8);
    for t in 0..5 {
        for c in 0..4 {
            assert_eq!(l[t * 8 + 2 * c].0, 0xDFF0A8 + 16 * c as u32);
            assert_eq!(l[t * 8 + 2 * c].1, 0);
            assert_eq!(l[t * 8 + 2 * c + 1].0, 0xDFF0A6 + 16 * c as u32);
            assert_eq!(l[t * 8 + 2 * c + 1].1, 0x7C);
        }
    }
    let l = run_ticks(&mut player(&[(0, 1, [0x01, 0xAC, 0x10, 0x00])]), 6);
    let regs: Vec<u32> = l[40..].iter().map(|w| w.0).collect();
    // note on channel 1: LOC (two words), LEN, PER, VOL, DMA off; then the 8 output writes, DMA on,
    // and the repeat LOC / LEN of the channel
    assert_eq!(&regs[..6], &[0xDFF0B0, 0xDFF0B2, 0xDFF0B4, 0xDFF0B6, 0xDFF0B8, 0xDFF096]);
    assert_eq!(l[40 + 5].1, 0x0002);
    assert_eq!(l[40 + 2].1, 10); // length of sample "a"
    assert_eq!(l[40 + 3].1, 0x1AC); // the period of the note (0x1AC = C-2 row 0)
    assert_eq!(l[40 + 6].0, 0xDFF0A8);
    let on: Vec<_> = l.iter().filter(|w| w.0 == 0xDFF096).collect();
    assert_eq!(on.len(), 2);
    assert_eq!(on[1].1, 0x8002);
    // the repeat of "a": 2 words in, 8 long
    let n = l.len();
    assert_eq!((l[n - 1].0, l[n - 1].1), (0xDFF0B4, 8));
    assert!(l.windows(2).all(|w| w[0].2 <= w[1].2));
}

#[test]
fn the_slide_mask_starts_at_zero() {
    // a slide up by 8 on the note's first tick moves by 0 (mask 0), the next one by 8
    let mut r = player(&[(0, 0, [0x01, 0xAC, 0x12, 0x08])]);
    let l = run_ticks(&mut r, 8);
    let per: Vec<u16> = l.iter().filter(|w| w.0 == 0xDFF0A6).map(|w| w.1).collect();
    // tick 6 writes the note's period twice (the trigger, the output phase); tick 7 slides by 0,
    // tick 8 by 8
    assert_eq!(per[5], 0x1AC);
    assert_eq!(per[6], 0x1AC);
    assert_eq!(per[7], 0x1AC);
    assert_eq!(per[8], 0x1AC + 8);
}

#[test]
fn volume_set_and_slide() {
    // C20 sets the volume to 0x20; a row later an A command slides it up by 2 per tick
    let mut r = player(&[(0, 0, [0x01, 0xAC, 0x1C, 0x20]), (1, 0, [0x00, 0x00, 0x0A, 0x20])]);
    let l = run_ticks(&mut r, 14);
    let vol: Vec<u16> = l.iter().filter(|w| w.0 == 0xDFF0A8).map(|w| w.1).collect();
    // tick 6: the trigger writes volume 0, the output phase the row's 0x20
    assert_eq!(vol[5], 0);
    assert_eq!(vol[6], 0x20);
    assert_eq!(vol[12], 0x20);
    assert_eq!(vol[13], 0x22);
}

#[test]
fn song_end_reports_for_f00_and_the_wrap() {
    let mut r = player(&[(0, 0, [0, 0, 0x0F, 0x00])]);
    assert!(!r.songend());
    run_ticks(&mut r, 5);
    assert!(!r.songend());
    run_ticks(&mut r, 6);
    assert!(r.songend());
    // F with a value sets the speed: one row every tick
    let mut r = player(&[(0, 0, [0, 0, 0x0F, 0x01])]);
    run_ticks(&mut r, 6 + 63 + 2);
    assert!(r.songend()); // the pattern ended: one position, so the song wrapped
}

#[test]
fn deterministic() {
    let cells = [(0, 0, [0x01, 0xAC, 0x10, 0x00]), (2, 1, [0x01, 0x58, 0x1E, 0xD2]), (4, 2, [0, 0, 0x04, 0x37]), (6, 3, [0, 0, 0x0E, 0x61])];
    let a = run_ticks(&mut player(&cells), 60);
    let b = run_ticks(&mut player(&cells), 60);
    assert_eq!(a, b);
    assert!(a.len() > 400);
}

#[test]
fn every_command_runs_without_panic() {
    for cmd in 0..16u8 {
        for par in [0u8, 1, 0x0F, 0x10, 0x37, 0x80, 0xFF] {
            let cells = [
                (0, 0, [0x01, 0xAC, 0x10 | cmd, par]),
                (1, 0, [0x01, 0x58, 0x10 | cmd, par]),
                (2, 1, [0x00, 0x00, cmd, par]),
                (3, 2, [0x01, 0xAC, 0x20 | cmd, par]),
                (4, 3, [0x00, 0x71, cmd, par]),
            ];
            let mut r = player(&cells);
            run_ticks(&mut r, 140);
        }
    }
    // every extended command
    for x in 0..16u8 {
        for y in 0..16u8 {
            let cells = [(0, 0, [0x01, 0xAC, 0x1E, x << 4 | y]), (1, 1, [0, 0, 0x0E, x << 4 | y]), (2, 2, [0x01, 0x58, 0x2E, x << 4 | y])];
            let mut r = player(&cells);
            run_ticks(&mut r, 100);
        }
    }
}

#[test]
fn malformed_input_does_not_panic() {
    let (d, s) = module(&[(0, 0, [0x01, 0xAC, 0x10, 0x00]), (3, 1, [0x01, 0x58, 0x13, 0x45])]);
    for n in [0, 1, 11, 12, 20, 60, 120] {
        let _ = detect(&d[..n.min(d.len())]);
        let _ = NickPelling::build(&d[..n.min(d.len())], &s);
    }
    let mut x = 12345u32;
    for round in 0..80 {
        let mut e = d.clone();
        let mut f = s.clone();
        for _ in 0..20 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let o = (x >> 8) as usize % e.len();
            if round % 2 == 0 {
                e[o] = (x >> 24) as u8;
            } else {
                let o = (x >> 8) as usize % f.len();
                f[o] = (x >> 24) as u8;
            }
        }
        if let Some((mut r, _)) = NickPelling::build(&e, &f) {
            run_ticks(&mut r, 80);
        }
    }
}

#[test]
fn companion_names() {
    assert_eq!(companions("/x/NPP.song"), vec!["SMP.song".to_string(), "SMP.set".to_string()]);
    assert_eq!(companions("npp.a"), vec!["SMP.a".to_string(), "SMP.set".to_string()]);
    assert_eq!(companions("x"), vec!["SMP.set".to_string()]);
    // a name that does not start with NPP. only tries the shared set
    assert_eq!(companions("xxx.q"), vec!["SMP.set".to_string()]);
}

#[test]
fn first_write_follows_path_name_and_sample_resolution() {
    // reference values (floor of the half-colour-clock time) measured on the same song layouts
    // at a 60-character and a 61-character directory with the file name NPP.q
    let s = song(&[pattern(&[])], &[0], &["e1"]);
    let e = |len: u16, fine: u8| set(&[Smp { name: "e1", len, fine, vol: 64, rstart: 0, rlen: len }], &vec![1; 2 * len as usize]);
    let (t, td) = (e(32, 0), e(10, 0x80));
    let fw = |s: &[u8], t: &[u8], dir: usize, f: &str, own: bool| first_write_cycles(s, t, dir, f, own).unwrap() / 2;
    assert_eq!(fw(&s, &t, 60, "NPP.q", true), 26743);
    assert_eq!(fw(&s, &t, 61, "NPP.q", true), 26752);
    assert_eq!(fw(&s, &t, 60, "NPP.q", false), 27440);
    assert_eq!(fw(&s, &t, 61, "NPP.q", false), 27458);
    assert_eq!(fw(&s, &td, 60, "NPP.q", true), 27038);
    assert_eq!(fw(&s, &t, 60, "npp.q", true), 26779);
    assert_eq!(fw(&s, &t, 60, "xxx.q", false), 27398);
    // only the leaf of a path counts as the file name
    assert_eq!(fw(&s, &t, 60, "/a/b/NPP.q", true), 26743);
}

#[test]
fn first_write_pays_for_the_letters_of_npp_a_name_starts_with() {
    // differences in cycles between names of the same length, fitted to the reference's first
    // writes at a 60-character directory with a sample set read as SMP.set
    let s = song(&[pattern(&[])], &[0], &["e1"]);
    let t = set(&[Smp { name: "e1", len: 32, fine: 0, vol: 64, rstart: 0, rlen: 32 }], &vec![1; 64]);
    let y = |f: &str| first_write_cycles(&s, &t, 60, f, false).unwrap() as i64;
    assert_eq!(y("NPx.q") - y("xxx.q"), 54);
    assert_eq!(y("NPPx.q") - y("xxxx.q"), 54);
    assert_eq!(y("nPx.q") - y("xxx.q"), 54 + 21);
    assert_eq!(y("npx.q") - y("xxx.q"), 54 + 21 + 25);
    // only the first letter matching: 28, and a cycle more when directory, name and case add up odd
    assert_eq!(y("Nxx.q") - y("xxx.q"), 29);
    assert_eq!(y("Nxxx.q") - y("xxxx.q"), 28);
    assert_eq!(y("nxx.q") - y("xxx.q"), 28 + 21);
}

#[test]
fn plays_through_the_engine_with_its_sample_set() {
    use crate::engine::{Backend, Engine, Playing};
    use std::path::Path;
    let (d, s) = module(&[(0, 0, [0x01, 0xAC, 0x10, 0x00])]);
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    // without the sample set the module is refused
    assert!(e.play_data(Path::new("/npp-test/NPP.song"), d.clone()).is_err());
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.os.mem_files.insert(Path::new("/npp-test/SMP.set").to_path_buf(), s);
    e.play_data(Path::new("/npp-test/NPP.song"), d).unwrap();
    let Some(Playing { backend: Backend::Replay(_), format, .. }) = &e.playing else { panic!("not a replayer") };
    assert_eq!(format, "Nick Pelling Packer");
    let mut buf = vec![0f32; 2 * 960];
    for _ in 0..20 {
        e.render(&mut buf);
    }
    assert!(!e.songend());
}
