use super::*;
use m68k::CpuType;

fn put16(d: &mut Vec<u8>, v: u16) {
    d.extend_from_slice(&v.to_be_bytes());
}

/// A module from objects: `tabs[t]` holds the objects of table t (song, sequence, instrument,
/// waveform), `None` for an entry of value 0. Every object starts with a 16-byte name area.
fn build(tabs: [Vec<Option<Vec<u8>>>; 4]) -> Vec<u8> {
    let mut d = vec![0u8; 20];
    d[..4].copy_from_slice(b"MSOB");
    for (t, objs) in tabs.iter().enumerate() {
        while d.len() % 4 != 2 {
            d.push(0);
        }
        put16(&mut d, objs.len().saturating_sub(1) as u16);
        let tab = d.len();
        let hdr = 4 + 4 * t;
        d[hdr..hdr + 4].copy_from_slice(&((tab - hdr) as u32).to_be_bytes());
        d.resize(tab + 4 * objs.len(), 0);
        for (i, o) in objs.iter().enumerate() {
            if let Some(o) = o {
                if d.len() % 2 != 0 {
                    d.push(0);
                }
                let at = d.len();
                d.extend_from_slice(&[0; 16]);
                d.extend_from_slice(o);
                let e = tab + 4 * i;
                d[e..e + 4].copy_from_slice(&((at - e) as u32).to_be_bytes());
            }
        }
    }
    d
}

/// A song object body (from offset 0x10).
struct Song {
    tracks: [u8; 4],
    inst: u8,
    transpose: u8,
    speed: u8,
    det_pat: u8,
    det_int: u8,
    master: u8,
    rbase: [u8; 4],
    rrange: [u8; 4],
    vol: [u8; 4],
}

impl Song {
    fn new(tracks: [u8; 4]) -> Self {
        Song { tracks, inst: 1, transpose: 0, speed: 0, det_pat: 0, det_int: 0, master: 63, rbase: [0; 4], rrange: [0; 4], vol: [63; 4] }
    }
    fn bytes(&self) -> Vec<u8> {
        let mut b = vec![0u8; 0x32 - 0x10];
        b[0..4].copy_from_slice(&self.tracks);
        b[0x0E] = self.inst;
        b[0x0F] = self.transpose;
        b[0x10] = self.speed;
        b[0x12] = self.det_pat;
        b[0x13] = self.det_int;
        b[0x14] = self.master;
        b[0x16..0x1A].copy_from_slice(&self.rbase);
        b[0x1A..0x1E].copy_from_slice(&self.rrange);
        b[0x1E..0x22].copy_from_slice(&self.vol);
        b
    }
}

/// An instrument body (from offset 0x10) of type `ty`, waveform slots `slots`, envelope
/// peak/sustain/attack/decay/release, and up to four modulator records of 18 bytes.
fn instrument(ty: u8, slots: [u8; 8], env: [u16; 5], mods: &[[u8; 18]]) -> Vec<u8> {
    let mut b = vec![0u8; 0x7A - 0x10];
    b[0] = ty;
    b[4..12].copy_from_slice(&slots);
    for (i, e) in env.iter().enumerate() {
        b[0x28 - 0x10 + 2 * i..0x2A - 0x10 + 2 * i].copy_from_slice(&e.to_be_bytes());
    }
    for (k, m) in mods.iter().enumerate() {
        b[0x32 - 0x10 + 18 * k..0x32 - 0x10 + 18 * k + 18].copy_from_slice(m);
    }
    b
}

/// A waveform body (from offset 0x10): length `l`, octave byte, data.
fn wave(l: u16, oct: u8, data: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 8];
    b[0..2].copy_from_slice(&l.to_be_bytes());
    b[4] = oct;
    b.extend_from_slice(data);
    b
}

fn seq(s: &[u8]) -> Vec<u8> {
    s.to_vec()
}

/// One song, one track, a type 0 instrument with its waveform in slot 1.
fn simple(track: &[u8], mut song: Song) -> Vec<u8> {
    song.tracks = [1, 0, 0, 0];
    let data: Vec<u8> = (0..16).map(|i| (i * 7) as u8).collect();
    build([
        vec![None, Some(song.bytes())],
        vec![None, Some(seq(track))],
        vec![None, Some(instrument(0, [0, 1, 0, 0, 0, 0, 0, 0], [0x3F00, 0x2000, 0x3F00, 0x0100, 0x0200], &[]))],
        vec![Some(wave(16, 0, &[0; 16])), Some(wave(16, 0, &data))],
    ])
}

fn start(d: &[u8], base: u32) -> (Machine, Box<dyn Replayer>) {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let r = (OPEN.new)(d, base, &mut m).expect("module accepted");
    (m, r)
}

fn run(d: &[u8], ticks: usize) -> (Vec<(u32, u16, u64)>, bool) {
    let (mut m, mut r) = start(d, 0x10000);
    m.bus.write_log = Some(Vec::new());
    for k in 0..ticks {
        m.bus.clock = 10_000_000 + TICK * k as u64;
        r.tick(&mut Hw { bus: &mut m.bus });
    }
    (m.bus.write_log.take().unwrap(), r.songend())
}

#[test]
fn detection() {
    let d = simple(&[0x30, 4, 0x80, 0], Song::new([1, 0, 0, 0]));
    assert!(detect(&d));
    let mut e = d.clone();
    e[0] = b'X';
    assert!(!detect(&e));
    assert!(!detect(&d[..10]));
    assert!(!detect(&[]));
}

#[test]
fn first_tick_matches_the_specification_example() {
    // specification 5.5: note 0x30, octave group 2, waveform in slot 1, no modulators, no detune
    let d = simple(&[0x30, 4, 0x80, 0], Song::new([1, 0, 0, 0]));
    let (log, _) = run(&d, 1);
    let t0 = 10_000_000u64;
    let w: Vec<(u32, u16, u64)> = log.iter().filter(|w| w.2 >= t0).copied().collect();
    assert_eq!(w.len(), 6, "{w:x?}");
    // the spacing of the writes is the specification's (LC, +6 LEN, +18 PER, +35 VOL, +12 DMACON);
    // the first LC lies near the example's 913 (the example's total counts some items this
    // module's instrument does not take the same way, so only the neighbourhood is checked)
    let lc = w[0].2 - t0;
    assert!((900..1000).contains(&lc), "{lc}");
    assert_eq!(w[0].2, w[1].2);
    assert_eq!((w[0].0, w[2].0), (0xDFF0A0, 0xDFF0A4));
    assert_eq!(w[2].2 - t0, lc + 6);
    assert_eq!(w[3].2 - t0, lc + 24);
    assert_eq!(w[4].2 - t0, lc + 59);
    assert_eq!(w[5].2 - t0, lc + 71);
    // length 16 bytes = 8 words; period index 0x30 + 0x30; envelope at the peak: volume 15
    assert_eq!(w[2].1, 8);
    assert_eq!(w[3].1, period_at(96).unwrap());
    assert_eq!(w[4].1, 0x0F0F);
    assert_eq!(w[5].1, 0x8001);
    // the sample address is the module's offset of the waveform data plus its guest base
    let lc = ((w[0].1 as u32) << 16) | w[1].1 as u32;
    assert!(lc > 0x10000 && lc < 0x10000 + d.len() as u32);
}

#[test]
fn deterministic() {
    let d = simple(&[0x30, 4, 0x24, 3, 0x85, 0x80, 0x00, 0], Song::new([1, 0, 0, 0]));
    assert_eq!(run(&d, 60), run(&d, 60));
}

#[test]
fn song_end_when_a_voice_wraps() {
    let d = simple(&[0x30, 2, 0x80, 0], Song::new([1, 0, 0, 0]));
    assert!(!run(&d, 2).1);
    assert!(run(&d, 6).1);
}

#[test]
fn start_writes_and_subsong_range() {
    let d = simple(&[0x30, 4, 0x80, 0], Song::new([1, 0, 0, 0]));
    let mut m = Machine::new(CpuType::M68000);
    m.bus.write_log = Some(Vec::new());
    let r = (OPEN.new)(&d, 0x10000, &mut m).unwrap();
    let log = m.bus.write_log.take().unwrap();
    // load: four stops; start: four stops, four voice resets (one valid subsong)
    let v: Vec<u16> = log.iter().map(|w| w.1).collect();
    assert_eq!(v, [1, 2, 4, 8, 1, 2, 4, 8, 1, 2, 4, 8]);
    assert_eq!(r.subsongs(), (1, 1, 1));
    assert_eq!(v.len(), 12);
    assert!(log.windows(2).all(|w| w[0].2 <= w[1].2));
}

#[test]
fn invalid_subsong_only_stops_dma() {
    // the song table's entry 2 is 0: subsong 2 stops DMA and starts nothing
    let song = Song::new([1, 0, 0, 0]);
    let d = build([
        vec![None, Some(song.bytes()), None],
        vec![None, Some(seq(&[0x30, 4, 0x80, 0]))],
        vec![None, Some(instrument(0, [0, 1, 0, 0, 0, 0, 0, 0], [0x3F00, 0x2000, 0x3F00, 0x0100, 0x0200], &[]))],
        vec![Some(wave(16, 0, &[0; 16])), Some(wave(16, 0, &[3; 16]))],
    ]);
    let (mut m, mut r) = start(&d, 0x10000);
    m.bus.write_log = Some(Vec::new());
    m.bus.clock = 5_000_000;
    r.set_subsong(2, &mut Hw { bus: &mut m.bus });
    let log = m.bus.write_log.take().unwrap();
    assert_eq!(log.len(), 4);
}

#[test]
fn malformed_input_is_safe() {
    let d = simple(&[0x30, 4, 0x24, 0, 0x85, 1, 0x86, 3, 0x87, 0, 0x84, 3, 0x00, 0], Song::new([1, 0, 0, 0]));
    let mut m = Machine::new(CpuType::M68000);
    for cut in 0..d.len() {
        if let Some(mut r) = (OPEN.new)(&d[..cut], 0x10000, &mut m) {
            for k in 0..5 {
                m.bus.clock += TICK * k;
                r.tick(&mut Hw { bus: &mut m.bus });
            }
        }
    }
    for o in (20..d.len()).step_by(2) {
        for v in [0x00, 0x01, 0x7F, 0x80, 0xFE, 0xFF] {
            let mut e = d.clone();
            e[o] = v;
            if let Some(mut r) = (OPEN.new)(&e, 0x10000, &mut m) {
                for k in 0..50 {
                    m.bus.clock += TICK * k;
                    r.tick(&mut Hw { bus: &mut m.bus });
                }
                r.set_subsong(1, &mut Hw { bus: &mut m.bus });
            }
        }
    }
}

#[test]
fn endless_zero_duration_row_ends_the_voice() {
    // a loop of zero-duration notes that never ends a row: the voice ends instead of hanging
    let d = simple(&[0x30, 0, 0x86, 0, 0x30, 0, 0x87, 0], Song::new([1, 0, 0, 0]));
    // (the data after the sequence is whatever follows it; the point is that this returns)
    let _ = run(&d, 3);
}

#[test]
fn period_beyond_the_tables_ends_the_voice() {
    // note 0x7F in group 7 with a waveform octave byte 0: index 0x7F + 0x30 = 175 > 153
    let song = Song::new([1, 0, 0, 0]);
    let d = build([
        vec![None, Some(song.bytes())],
        vec![None, Some(seq(&[0x7F, 4, 0x80, 0]))],
        vec![None, Some(instrument(0, [1; 8], [0x3F00, 0x2000, 0x3F00, 0x0100, 0x0200], &[]))],
        vec![Some(wave(16, 0, &[0; 16])), Some(wave(16, 0, &[1; 16]))],
    ]);
    let (log, _) = run(&d, 3);
    assert!(!log.iter().any(|w| w.0 == 0xDFF0A6));
    assert!(log.iter().any(|w| w.0 == 0xDFF096 && w.1 == 1));
}

#[test]
fn tables() {
    assert_eq!(period_at(0), Some(15360));
    assert_eq!(period_at(48), Some(960));
    assert_eq!(period_at(60), Some(480));
    assert_eq!(period_at(109), Some(28));
    assert!(period_at(153).is_some());
    assert_eq!(period_at(154), None);
    assert_eq!(vol_table(63)[10], 10);
    assert_eq!(vol_table(0)[10], 1);
    assert_eq!(vol_table(17)[0], 0);
}
