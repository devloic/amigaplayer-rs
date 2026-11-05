use super::*;
use crate::engine::Engine;
use std::path::Path;

/// `AM_MODULE=file AM_TICKS=n AM_LOG=out cargo test --release dump -- --ignored`:
/// the register log of `n` ticks of a module.
#[test]
#[ignore]
fn dump() {
    let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("AM_MODULE"), std::env::var("AM_TICKS"), std::env::var("AM_LOG")) else {
        return;
    };
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
    println!("format {}", e.playing.as_ref().unwrap().format);
    let mut buf = vec![0f32; 2 * 960];
    let n: usize = ticks.parse().unwrap();
    for _ in 0..n {
        e.render(&mut buf);
    }
    let l = e.m.bus.write_log.take().unwrap();
    let text: String = l.iter().map(|(a, v, t)| format!("{a:06x} {v:04x} {t}\n")).collect();
    std::fs::write(out, text).unwrap();
}

// ---------------------------------------------------------------------------------
// Synthetic modules (built here from the specification's layout; no game data)

const K: u32 = 0x1000;
const PER: usize = 0x600;
const ORDER: usize = 0x700;
const STREAMS: usize = 0x730;
const ARP: usize = 0x8B0;
const INST: usize = 0x940;
const WAVE: usize = 0xE00;
const SAMPLES: usize = 0xE18;
const TABLE: [u16; 96] = [
    32245, 30439, 28726, 27114, 25593, 24155, 22800, 21521, 20313, 19172, 18096, 17081, 16122, 15219, 14363, 13557, 12797, 12078,
    11400, 10760, 10156, 9586, 9048, 8541, 7181, 6779, 6398, 6039, 5700, 5380, 5078, 4793, 4524, 4270, 4031, 3805, 3591, 3389,
    3199, 3019, 2850, 2690, 2539, 2397, 2262, 2135, 2015, 1902, 1795, 1695, 1600, 1510, 1425, 1345, 1270, 1198, 1131, 1068, 1008,
    951, 898, 847, 800, 755, 713, 673, 635, 599, 566, 534, 504, 476, 449, 424, 400, 377, 356, 336, 317, 300, 283, 267, 252, 238,
    224, 212, 200, 189, 178, 168, 159, 150, 141, 133, 126, 119,
];

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// An 8SVX file with the chunk order of the specification.
fn svx(one: u32, rep: u32, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    let mut chunk = |tag: &[u8; 4], data: &[u8]| {
        v.extend_from_slice(tag);
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(data);
    };
    let mut vh = [0u8; 20];
    vh[..4].copy_from_slice(&one.to_be_bytes());
    vh[4..8].copy_from_slice(&rep.to_be_bytes());
    chunk(b"VHDR", &vh);
    chunk(b"NAME", b"test.sample");
    chunk(b"ANNO", b"Protracker 1.1B");
    chunk(b"BODY", body);
    let mut f = b"FORM".to_vec();
    f.extend_from_slice(&((v.len() + 4) as u32).to_be_bytes());
    f.extend_from_slice(b"8SVX");
    f.extend_from_slice(&v);
    f
}

/// What a test module holds: per voice an order list, patterns (streams), instruments, arpeggios
/// and the sample files.
struct Mod {
    orders: [Vec<u8>; 3],
    streams: Vec<Vec<u8>>,
    insts: Vec<[u8; 8]>,
    arps: Vec<Vec<u8>>,
    samples: Vec<(u32, u32, Vec<u8>)>,
    speed: (u16, u16),
    version_b: bool,
}

impl Mod {
    fn build(&self) -> Vec<u8> {
        let mut d = vec![0u8; 0x2000];
        put32(&mut d, 0x1A, EXIT);
        let disp = |d: &mut Vec<u8>, at: usize, to: usize| put16(d, at, (to as i64 - at as i64) as u16);
        for (i, w) in TABLE.iter().enumerate() {
            put16(&mut d, PER + 2 * i, *w);
        }
        let (per_at, st_at, arp_at) = if self.version_b { (0x3A2, 0x478, 0x4EA) } else { (0x3A0, 0x474, 0x4E6) };
        if self.version_b {
            d[0x20E..0x212].copy_from_slice(&[1, 2, 4, 8]);
        }
        let nst = if self.version_b { 64 } else { 96 };
        // data from the instrument table on
        let mut at = INST;
        for i in &self.insts {
            d[at..at + 8].copy_from_slice(i);
            at += 8;
        }
        let mut arp_off = Vec::new();
        for a in &self.arps {
            arp_off.push(at);
            d[at..at + a.len()].copy_from_slice(a);
            at += a.len();
        }
        let mut ol = Vec::new();
        for o in &self.orders {
            ol.push(at);
            d[at..at + o.len()].copy_from_slice(o);
            at += o.len();
        }
        let mut st = vec![at];
        for s in &self.streams {
            st.push(at);
            d[at..at + s.len()].copy_from_slice(s);
            at += s.len();
        }
        assert!(at < WAVE - 8);
        for i in 0..32 {
            put32(&mut d, ARP + 4 * i, K + arp_off.get(i).copied().unwrap_or(INST) as u32);
        }
        for i in 0..nst {
            put32(&mut d, STREAMS + 4 * i, K + st.get(i).copied().unwrap_or(st[0]) as u32);
        }
        for v in 0..3 {
            let e = ORDER + 8 * (v + 1);
            put32(&mut d, e, K + ol[v] as u32);
            put16(&mut d, e + 4, self.speed.0);
            put16(&mut d, e + 6, self.speed.1);
        }
        disp(&mut d, 0x0C, SAMPLES);
        disp(&mut d, 0x14, WAVE);
        disp(&mut d, 0xD4, 0x1F00);
        disp(&mut d, 0xE2, ORDER);
        disp(&mut d, 0xFE, 0x1EF0);
        d[0x1EF0] = 0xFF;
        disp(&mut d, 0x15E, INST);
        disp(&mut d, per_at, PER);
        disp(&mut d, st_at, STREAMS);
        disp(&mut d, arp_at, ARP);
        let mut s = Vec::new();
        for (o, r, b) in &self.samples {
            s.extend(svx(*o, *r, b));
        }
        s.extend_from_slice(b"EXIT");
        d.truncate(SAMPLES);
        d.extend(s);
        d
    }
}

/// An instrument record.
fn inst(vol: u8, vib: u8, vib_delay: u8, flags: u8, detune: u8, rate: u8, mode: u8, wave: u8) -> [u8; 8] {
    [vol, vib, vib_delay, flags, detune, rate, mode, wave]
}

fn sample(n: usize, fill: u8) -> (u32, u32, Vec<u8>) {
    (60, 40, vec![fill; n])
}

/// Voice 1 plays `stream` forever; the other two end at once.
fn basic(stream: Vec<u8>) -> Mod {
    Mod {
        orders: [vec![1, 0xFF, 0], vec![2, 0xFF, 0], vec![2, 0xFF, 0]],
        streams: vec![stream, vec![0xFE]],
        insts: vec![inst(0x40, 0, 0, 0, 0, 0, 0, 0)],
        arps: vec![vec![0, 12, 0xFE]],
        samples: vec![sample(100, 0x11), sample(100, 0x22)],
        speed: (2, 2),
        version_b: false,
    }
}

type Log = Vec<(u32, u16, u64)>;

fn play(d: Vec<u8>, ticks: usize) -> Option<(Log, bool)> {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/aam-test/aam.test"), d).ok()?;
    if e.playing.as_ref()?.format != "Art And Magic" {
        return None;
    }
    let mut out = vec![0f32; 2 * 960];
    let mut end = false;
    for _ in 0..ticks {
        e.render(&mut out);
        end |= e.songend();
    }
    Some((e.m.bus.write_log.take().unwrap(), end))
}

/// The log split into ticks (a gap of more than 20000 colour clocks starts one); the four
/// AUD0DAT writes are the first group.
fn groups(l: &Log) -> Vec<Vec<(u32, u16, u64)>> {
    let mut g: Vec<Vec<(u32, u16, u64)>> = Vec::new();
    let mut last = 0;
    for &w in l {
        if g.is_empty() || w.2 - last > 20000 {
            g.push(Vec::new());
        }
        last = w.2;
        g.last_mut().unwrap().push(w);
    }
    g
}

fn pers(l: &Log, ch: u32) -> Vec<u16> {
    l.iter().filter(|w| w.0 == 0xDFF0A6 + 16 * ch).map(|w| w.1).collect()
}

#[test]
fn detects_by_the_two_exit_marks() {
    let d = basic(vec![0xFF]).build();
    assert!(detect(&d));
    let mut bad = d.clone();
    bad[0x1A] = b'X';
    assert!(!detect(&bad));
    let mut bad = d.clone();
    let n = bad.len();
    bad[n - 1] = 0;
    assert!(!detect(&bad));
    assert!(!detect(&d[..0x100]));
    assert!(!detect(&vec![0u8; 4000]));
}

#[test]
fn locates_the_tables_of_both_versions() {
    let a = locate(&basic(vec![0xFF]).build()).unwrap();
    assert!(!a.b);
    assert_eq!((a.order, a.streams, a.arp, a.inst, a.per, a.wave, a.nstreams), (ORDER, STREAMS, ARP, INST, PER, WAVE, 96));
    assert_eq!(a.k, K as i64);
    let mut m = basic(vec![0xFF]);
    m.version_b = true;
    let b = locate(&m.build()).unwrap();
    assert!(b.b);
    assert_eq!(b.nstreams, 64);
}

#[test]
fn converts_the_sample_section() {
    // five samples: the wave table has room for two, the rest is written over sample 0
    let mut m = basic(vec![0xFF]);
    m.samples = (0..5u8).map(|i| (60, if i == 2 { 0 } else { 40 }, vec![0x10 + i; 100])).collect();
    let d = m.build();
    let lay = locate(&d).unwrap();
    let mut img = d.clone();
    convert(&mut img, 0x4000, &lay);
    let w = |k: usize, i: usize| be32(&img, WAVE + 12 * k + 4 * i);
    // sample 0 starts at the pool start, the loop 60 bytes in, 100 bytes long
    assert_eq!((w(0, 0), w(0, 1), w(0, 2)), (0x4000 + SAMPLES as u32, 0x4000 + SAMPLES as u32 + 60, 100));
    // sample 1 follows the first body
    assert_eq!(w(1, 0), 0x4000 + SAMPLES as u32 + 100);
    // sample 2 has no repeat part: one-shot length 58, loop of 2 bytes at the end
    assert_eq!((w(2, 1) - w(2, 0), w(2, 2)), (58, 60));
    // entries 2.. overlay the start of sample 0's body: entry 2 is at the pool start
    assert_eq!(WAVE + 24, SAMPLES);
    assert_eq!(&img[SAMPLES..SAMPLES + 4], &w(2, 0).to_be_bytes());
    assert_eq!(img[SAMPLES + 12..SAMPLES + 16], w(3, 0).to_be_bytes());
    assert_eq!(img[SAMPLES + 24..SAMPLES + 28], w(4, 0).to_be_bytes());
    // and the rest of the body is still there; the later bodies follow
    assert_eq!(img[SAMPLES + 36], 0x10);
    assert_eq!(img[SAMPLES + 100 + 5], 0x11);
    assert_eq!(img[SAMPLES + 400 + 5], 0x14);
}

#[test]
fn conversion_survives_damaged_sections() {
    let d = basic(vec![0xFF]).build();
    let lay = locate(&d).unwrap();
    for cut in [SAMPLES + 5, SAMPLES + 30, SAMPLES + 60, SAMPLES + 150, d.len() - 8] {
        let mut img = d[..cut].to_vec();
        img.extend_from_slice(b"EXIT");
        convert(&mut img, 0x4000, &lay);
    }
    let mut img = d.clone();
    put32(&mut img, SAMPLES + 4, 0xFFFF_FFF0);
    for p in SAMPLES..SAMPLES + 60 {
        img[p] = 0xFF;
    }
    convert(&mut img, 0x4000, &lay);
}

#[test]
fn first_tick_writes_and_note_start() {
    // instrument 0, length 4 steps, note 48
    let (l, _) = play(basic(vec![0xC0, 0x84, 48, 0xFF]).build(), 8).unwrap();
    let g = groups(&l);
    let regs: Vec<u32> = g[0].iter().map(|w| w.0).collect();
    assert_eq!(regs, vec![0xDFF0AA; 4]);
    assert_eq!(g[0][1].2 - g[0][0].2, 17);
    // tick 0: all three voices write period and volume 0; voice 1's event: DMA off, wave address,
    // length, the two CIA writes of the note start (the filter pulse flag is clear: one write)
    let t0: Vec<(u32, u16)> = g[1].iter().map(|w| (w.0, w.1)).collect();
    assert_eq!(t0[0], (0xDFF0B6, 0));
    assert_eq!(t0[1], (0xDFF0B8, 0));
    assert_eq!(t0[2], (0xDFF096, 2));
    assert_eq!((t0[3].0, t0[4].0), (0xDFF0B0, 0xDFF0B2));
    assert_eq!(g[1][3].2, g[1][4].2);
    assert_eq!(t0[5], (0xDFF0B4, 50));
    assert_eq!(t0[6], (0xBFE001, 2));
    // the plain-tick spacing of the specification (11.8) on tick 1... voice 1's period write, then
    // its volume write 143 colour clocks later
    let per1 = g[2].iter().find(|w| w.0 == 0xDFF0B6).unwrap();
    assert_eq!(per1.1, 1795);
    let vol1 = g[2].iter().find(|w| w.0 == 0xDFF0B8).unwrap();
    assert_eq!((vol1.1, vol1.2 - per1.2), (0x40, 143));
    assert!(g[2].iter().any(|w| *w == (0xDFF096, 0x8002, w.2)));
    // the tick after: the loop pointer and length (repeat part 40 bytes: 20 words)
    let t3: Vec<(u32, u16)> = g[3].iter().map(|w| (w.0, w.1)).collect();
    assert_eq!((t3[2].0, t3[3].0, t3[4], t3[2].1), (0xDFF0B0, 0xDFF0B2, (0xDFF0B4, 20), t3[2].1));
    let start = ((t0[3].1 as u32) << 16) | t0[4].1 as u32;
    let lp = ((t3[2].1 as u32) << 16) | t3[3].1 as u32;
    assert_eq!(lp - start, 60);
    // the ticks are 70940 colour clocks apart
    assert_eq!(g[2][0].2 - g[1][0].2, 70940);
}

#[test]
fn voices_two_and_three_end_and_fall_silent() {
    let (l, _) = play(basic(vec![0xC0, 0x84, 48, 0xFF]).build(), 8).unwrap();
    // voice 2 and 3 write on the first tick only (their stream ends the voice)
    assert_eq!(pers(&l, 2).len(), 1);
    assert_eq!(pers(&l, 3).len(), 1);
    assert!(pers(&l, 1).len() >= 7);
    // channel 0 is never touched
    assert!(l.iter().all(|w| !(0xDFF0A0..0xDFF0AA).contains(&w.0)));
}

#[test]
fn step_length_and_rest() {
    // a rest of 2 steps then a note: a step lasts speed + 1 = 3 ticks
    let (l, _) = play(basic(vec![0xF2, 0xC0, 0x82, 48, 0xFF]).build(), 20).unwrap();
    let g = groups(&l);
    // the rest writes the DMA-off, but no note start
    assert!(g[1].iter().all(|w| w.0 != 0xDFF0B0));
    // the note starts when the rest's rows are over: 2 rows of 3 ticks, the first one 2 ticks
    let starts: Vec<usize> = (1..g.len()).filter(|&i| g[i].iter().any(|w| (w.0, w.1) == (0xDFF0B4, 50))).collect();
    assert!(!starts.is_empty());
    assert!(starts[0] >= 5, "{starts:?}");
}

#[test]
fn volume_override_and_tie() {
    // 0xFC 0x20: volume 0x20 for this note; the tie keeps the DMA running (no DMA-off write)
    let (l, _) = play(basic(vec![0xFC, 0x20, 0xC0, 0x82, 48, 0xFB, 0x82, 50, 0xFF]).build(), 14).unwrap();
    let vols: Vec<u16> = l.iter().filter(|w| w.0 == 0xDFF0B8).map(|w| w.1).collect();
    assert_eq!(vols[0], 0);
    assert_eq!(vols[1], 0x20);
    let first = l.iter().find(|w| w.0 == 0xDFF096).unwrap().2;
    let off: Vec<_> = l.iter().filter(|w| w.0 == 0xDFF096 && w.1 == 2 && w.2 < first + 11 * 70940).collect();
    // one DMA-off on the first event and none on the tied second (the third event, the end of the
    // pattern, writes one again)
    assert_eq!(off.len(), 1, "{off:?}");
}

#[test]
fn arpeggio_follows_its_list() {
    // instrument 1 has the arpeggio flag; the stream selects arpeggio list 0 (0, +12, stop)
    let mut m = basic(vec![0xC1, 0x88, 0x60, 48, 0xFF]);
    m.insts.push(inst(0x40, 0, 0, 0x40, 0, 0, 0, 0));
    let (l, _) = play(m.build(), 12).unwrap();
    let p = pers(&l, 1);
    // tick 0: 0; tick 1: note 48; tick 2: arpeggio 0 (note 48); tick 3: +12 (note 60); then the
    // list has stopped and the period stays
    assert_eq!(&p[..5], &[0, 1795, 1795, 898, 898]);
}

#[test]
fn vibrato_moves_the_period() {
    // depth shift 1, half period 2, no delay
    let mut m = basic(vec![0xC1, 0x8F, 48, 0xFF]);
    m.insts.push(inst(0x40, 0x12, 0, 0, 0, 0, 0, 0));
    let (l, _) = play(m.build(), 14).unwrap();
    let p = pers(&l, 1);
    assert!(p[2..12].iter().any(|&x| x != 1795), "{p:?}");
    // each step moves the period by (2 * (1795 - 1695)) >> 1 = 100, up and down
    assert!(p[2..12].iter().all(|&x| (1500..2000).contains(&x)), "{p:?}");
}

#[test]
fn slide_reaches_its_target() {
    // slide: speed byte 0x14 (multiplier 2, begins 4 rows before the end), from note 48 to note 52
    let (l, _) = play(basic(vec![0xC0, 0x8F, 0xFD, 0x14, 48, 52, 0xFF]).build(), 50).unwrap();
    let p = pers(&l, 1);
    // falls from note 48 and ends exactly on the period of note 52
    let at = p.iter().position(|&x| x == 1425).expect("arrives");
    assert!(p[..at].windows(2).skip(2).all(|w| w[1] <= w[0]), "{p:?}");
    assert!(p[at..at + 4].iter().all(|&x| x == 1425), "{p:?}");
}

#[test]
fn attack_and_decay_envelopes() {
    // attack: mode bit 7, rate 63 per tick: the volume rises from 0 to 0x40
    let mut m = basic(vec![0xC1, 0x8F, 48, 0xFF]);
    m.insts.push(inst(0x40, 0, 0, 0, 0, 63, 0x80, 0));
    let (l, _) = play(m.build(), 14).unwrap();
    let v: Vec<u16> = l.iter().filter(|w| w.0 == 0xDFF0B8).map(|w| w.1).collect();
    assert_eq!(v[1], 0);
    assert!(v.windows(2).take(8).all(|w| w[1] >= w[0]), "{v:?}");
    assert_eq!(*v[..12].iter().max().unwrap(), 0x40);
    // decay: mode 3, begins with 3 rows left; the volume falls to 0
    let mut m = basic(vec![0xC1, 0x84, 48, 0xFF]);
    m.insts.push(inst(0x40, 0, 0, 0, 0, 63, 3, 0));
    let (l, _) = play(m.build(), 14).unwrap();
    let v: Vec<u16> = l.iter().filter(|w| w.0 == 0xDFF0B8).map(|w| w.1).collect();
    assert_eq!(v[2], 0x40);
    assert!(v[2..].iter().any(|&x| x < 0x40), "{v:?}");
}

#[test]
fn version_b_flags_attack_with_a_colour_write() {
    let mut m = basic(vec![0xC1, 0x8F, 48, 0xFF]);
    m.insts.push(inst(0x40, 0, 0, 0, 0, 8, 0x80, 0));
    m.version_b = true;
    let (l, _) = play(m.build(), 10).unwrap();
    assert!(l.iter().any(|w| (w.0, w.1) == (0xDFF182, 0x0FF0)));
    m.version_b = false;
    let (l, _) = play(m.build(), 10).unwrap();
    assert!(l.iter().all(|w| w.0 != 0xDFF182));
}

#[test]
fn filter_flag_writes_two_cia_values() {
    let mut m = basic(vec![0xC1, 0x82, 48, 0xFF]);
    m.insts.push(inst(0x40, 0, 0, 0x80, 0, 0, 0, 0));
    let (l, _) = play(m.build(), 4).unwrap();
    let c: Vec<u16> = l.iter().filter(|w| w.0 == 0xBFE001).map(|w| w.1).collect();
    assert_eq!(&c[..2], &[0, 2]);
}

#[test]
fn order_list_loop_marks_the_song_end() {
    // a pattern of one 1-step note repeated; every voice loops on the same tick (voices 2 and 3
    // end at once, so only voice 1 loops)
    let (_, end) = play(basic(vec![0xC0, 0x81, 48, 0xFF]).build(), 20).unwrap();
    assert!(end);
}

#[test]
fn repeat_count_and_transposition() {
    let mut m = basic(vec![0xC0, 0x81, 48, 0xFF]);
    // transposition 2, repeat 3 (played 4 times), then loop
    m.orders[0] = vec![0x82, 0x63, 1, 0xFF, 0];
    let (l, _) = play(m.build(), 30).unwrap();
    let p = pers(&l, 1);
    assert!(p.contains(&1600), "{p:?}"); // note 50
    let starts = l.iter().filter(|w| w.0 == 0xDFF0B0 + 2 && w.1 != 0).count();
    assert!(starts >= 4);
}

#[test]
fn renders_deterministically() {
    let a = play(basic(vec![0xC0, 0x84, 48, 0xC0, 0x82, 50, 0xFF]).build(), 60).unwrap();
    let b = play(basic(vec![0xC0, 0x84, 48, 0xC0, 0x82, 50, 0xFF]).build(), 60).unwrap();
    assert_eq!(a.0, b.0);
    assert!(a.0.len() > 100);
}

#[test]
fn malformed_modules_do_not_panic() {
    let good = basic(vec![0xC1, 0x8F, 0x60, 48, 0xFD, 0x12, 48, 52, 0xFC, 0x20, 0xFB, 60, 0xF3, 0xFF]).build();
    let mut seed = 0x1234_5678u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for round in 0..24 {
        let mut d = good.clone();
        for _ in 0..40 {
            let i = (rnd() as usize) % d.len();
            if (0x1A..0x1E).contains(&i) {
                continue;
            }
            d[i] = rnd() as u8;
        }
        if round % 5 == 0 {
            d.truncate(d.len() - 30);
            d.extend_from_slice(b"EXIT");
        }
        let _ = play(d, 40);
    }
    // not a module at all
    assert!(play(vec![0u8; 5000], 2).is_none());
}
