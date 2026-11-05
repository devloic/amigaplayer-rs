use super::*;
use crate::engine::Engine;
use std::path::Path;

fn put32(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

const YP: usize = 0xF0;
const YZ: usize = 0x100;
const PLAY: usize = 0x150;

/// The tag value of `Y` followed by the letter.
fn tag(c: u8) -> u32 {
    0x8000_5900 | c as u32
}

/// A MYST file of type `typ`: the tags, an init routine that returns at once and a play routine that
/// loads the block's address from the `YP` slot (a program-counter relative load) and stores the
/// given (block offset, value) words into it.
fn module(typ: u32, stores: &[(u16, u16)], extra: &[(u8, u32)]) -> Vec<u8> {
    module_i(typ, &[], stores, extra)
}

/// The same, with the words the init routine stores into the block as well.
fn module_i(typ: u32, init: &[(u16, u16)], stores: &[(u16, u16)], extra: &[(u8, u32)]) -> Vec<u8> {
    let mut d = vec![0u8; 0x200];
    d[..4].copy_from_slice(b"YMST");
    // (a tag given in `extra` comes first and so wins over the defaults)
    let mut tags = extra.to_vec();
    tags.extend_from_slice(&[(b'M', typ), (b'P', YP as u32), (b'Z', YZ as u32), (b'\\', PLAY as u32), (b']', 1), (b'^', 1)]);
    for (i, &(c, v)) in tags.iter().enumerate() {
        put32(&mut d, 4 + 8 * i, tag(c));
        put32(&mut d, 8 + 8 * i, v);
    }
    let mut o = YZ;
    if !init.is_empty() {
        put16(&mut d, o, 0x207A);
        put16(&mut d, o + 2, (YP as i32 - (o as i32 + 2)) as u16);
        o += 4;
        for &(off, v) in init {
            put16(&mut d, o, 0x317C);
            put16(&mut d, o + 2, v);
            put16(&mut d, o + 4, off);
            o += 6;
        }
    }
    put16(&mut d, o, 0x4E75);
    // load the block address, then the stores
    put16(&mut d, PLAY, 0x207A);
    put16(&mut d, PLAY + 2, (YP as i32 - (PLAY as i32 + 2)) as u16);
    let mut o = PLAY + 4;
    for &(off, v) in stores {
        put16(&mut d, o, 0x317C);
        put16(&mut d, o + 2, v);
        put16(&mut d, o + 4, off);
        o += 6;
    }
    put16(&mut d, o, 0x4E75);
    d
}

type Log = Vec<(u32, u16, u64)>;

fn engine(d: Vec<u8>, name: &str, files: &[(&str, Vec<u8>)]) -> Option<Engine> {
    let dir = Path::new("/ym-test");
    let mut e = Engine::new(48000.0);
    for (n, f) in files {
        e.m.os.mem_files.insert(dir.join(n), f.clone());
    }
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(&dir.join(name), d).ok()?;
    Some(e)
}

fn run(d: Vec<u8>, ticks: usize) -> (Log, Engine) {
    let mut e = engine(d, "ym.test", &[]).expect("plays");
    let mut out = vec![0f32; 2 * 960 * ticks];
    e.render(&mut out);
    (e.m.bus.write_log.take().unwrap(), e)
}

/// The writes of the audio registers and DMACON, split into the start-up group and one group per tick.
fn groups(log: &Log) -> Vec<Vec<(u32, u16, u64)>> {
    let mut g: Vec<Vec<(u32, u16, u64)>> = Vec::new();
    let mut last = 0;
    for &w in log.iter().filter(|w| (0xDFF096..0xDFF0E0).contains(&w.0)) {
        if g.is_empty() || w.2 - last > 20000 {
            g.push(Vec::new());
        }
        last = w.2;
        g.last_mut().unwrap().push(w);
    }
    g
}

fn reg(g: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
    g.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

fn silent_s(log: &Log) -> u32 {
    let l: Vec<_> = log.iter().filter(|w| w.0 == 0xDFF0A0 || w.0 == 0xDFF0A2).take(2).collect();
    (l[0].1 as u32) << 16 | l[1].1 as u32
}

#[test]
fn detection() {
    let d = module(0, &[], &[]);
    assert!((OPEN.detect)(&d));
    // replay-only files (type 1) and files without the type are rejected
    assert!(!detect(&module(1, &[], &[])));
    let mut e = d.clone();
    put32(&mut e, 4, tag(b'N'));
    assert!(!detect(&e));
    // the routines and the slot must be named
    for c in [b'Z', b'\\', b'P'] {
        let mut e = d.clone();
        for i in 0..8 {
            if be32(&e, 4 + 8 * i) == tag(c) {
                put32(&mut e, 4 + 8 * i, tag(b'e'));
            }
        }
        assert!(!detect(&e), "tag {}", c as char);
    }
    // an offset outside the file
    let mut e = d.clone();
    for i in 0..8 {
        if be32(&e, 4 + 8 * i) == tag(b'Z') {
            put32(&mut e, 8 + 8 * i, 0x10_0000);
        }
    }
    assert!(!detect(&e));
    // a magic at an odd offset or after the first 256 bytes is not taken
    let mut e = vec![0u8; 1];
    e.extend_from_slice(&d);
    assert!(!detect(&e));
    let mut e = vec![0u8; 258];
    e.extend_from_slice(&d);
    assert!(!detect(&e));
    // Atari register dumps have another magic
    let mut e = d.clone();
    e[..4].copy_from_slice(b"YM6!");
    assert!(!detect(&e));
    assert!(!detect(&[]));
    assert!(!detect(&d[..6]));
    // type 2 needs the replay's name and the init routine
    let t2 = module(2, &[], &[(b'O', 0x180)]);
    assert!(detect(&t2));
    assert!(!detect(&module(2, &[], &[])));
}

#[test]
fn garbage_never_panics() {
    let mut x = 12345u32;
    for n in 0..200 {
        let mut d: Vec<u8> = (0..(n * 7 % 300)).map(|_| { x = x.wrapping_mul(1103515245).wrapping_add(12345); (x >> 16) as u8 }).collect();
        if d.len() > 12 {
            d[..4].copy_from_slice(b"YMST");
        }
        let _ = detect(&d);
        if detect(&d) {
            let mut m = Machine::new(CpuType::M68000);
            let base = m.alloc(d.len() as u32 + 16);
            m.bus.write_bytes(base, &d);
            let _ = new(&d, base, &mut m);
        }
    }
}

#[test]
fn tables_follow_the_formulas() {
    let mut m = Machine::new(CpuType::M68000);
    let area = m.alloc(AREA);
    let svc = m.alloc(64);
    let s = area + BEFORE;
    let e = Emu { s, blk: off(s, O_BLK), svc };
    e.init(&mut m.bus);
    let b = &m.bus;
    // the scaled-sample table: full level at the top of the byte range, nothing at the bottom
    assert_eq!(b.r8(off(s, O_SC) + 15 * 256 + 0x7F), 127);
    assert_eq!(b.r8(off(s, O_SC) + 15 * 256 + 0x80), 0x80);
    assert_eq!(b.r8(off(s, O_SC) + 0x7F), 0);
    // the square table and the first square samples
    assert_eq!(b.r8(s + 2), 0x7F);
    assert_eq!(e.square(0), s + 2);
    assert_eq!(b.r8(e.square(1)), 0x7F);
    assert_eq!(b.r8(e.square(1) + 2), 0x80);
    assert_eq!(b.r8(e.square(7) + 127), 0x7F);
    assert_eq!(b.r8(e.square(7) + 128), 0x80);
    // noise: the generator's second value, 16807 squared, is odd
    assert_eq!(b.r8(off(s, O_N)), 0x81);
    assert_eq!(b.r8(off(s, O_N) + 2048), 0x81);
    for j in 0..4096 {
        let n = b.r8(off(s, O_N) + j) as i8;
        assert!(n == 127 || n == -127);
        assert_eq!(b.r8(off(s, O_NH) + j) as i8, if n == 127 { 127 } else { 0 });
        assert_eq!(b.r8(off(s, O_NL) + j) as i8, if n == 127 { 0 } else { -127 });
    }
    // envelope pair 0: 15 down to 0, then all zero; pair 1: up, then zero
    assert_eq!(b.r16(e.env_table(0)), 15 << 8);
    assert_eq!(b.r16(e.env_table(0) + 30), 0);
    assert_eq!(b.r16(e.env_table(1) + 2), 1 << 8);
    assert_eq!(b.r16(e.env_table(4) + 2 * 16), 15 << 8);
    assert_eq!(b.r8(e.env_wave(0)), 126);
    // the block
    assert_eq!(b.r8(e.blk + 0x10), 0xFF);
    assert_eq!(b.r16(e.blk + 0x0E), 0x3F);
    assert_eq!(b.r32(e.blk + 0x12A), s);
    assert_eq!(b.r32(e.blk + 0xEE), e.blk + REC[0]);
    assert_eq!(b.r32(e.blk + 0xF2), e.blk + REC[2]);
    assert_eq!(b.r32(e.blk + 0xFA), e.blk + REC[1]);
    assert_eq!(b.r16(e.blk + REC[3] + 0x1C), 4);
    assert_eq!(b.r32(e.blk + REC[1] + 0x18), 0xDFF0D0);
}

#[test]
fn octave_and_period_formulas() {
    assert_eq!(octave(10), 0);
    assert_eq!(octave(0x13), 0);
    assert_eq!(octave(0x14), 1);
    assert_eq!(octave(0x1FC), 5);
    assert_eq!(octave(0x500), 7);
    assert_eq!(octave(0xFFF), 7);
    assert_eq!(amiper(0x1FC, 5), (((0x1FCu32 * 0xE517) >> 5) >> 12) as u16);
    assert_eq!(divu(100, 0), None);
    assert_eq!(divu(100, 7), Some(14));
    // a quotient above 16 bits leaves the destination: its low word is what counts
    assert_eq!(divu(0x11E000, 14), Some(0xE000));
}

#[test]
fn silent_start_and_first_tick() {
    let (log, _) = run(module(0, &[], &[]), 3);
    let g = groups(&log);
    assert!(g.len() >= 3);
    // the start-up: LC, LEN and PER of the four channels, 10 colour clocks apart, then DMACON
    let s = silent_s(&log);
    assert_eq!(g[0].len(), 4 * 2 + 4 + 4 + 2);
    assert_eq!(reg(&g[0], 0xDFF0D4), vec![1]);
    assert_eq!(reg(&g[0], 0xDFF0C6), vec![0x80]);
    assert_eq!(reg(&g[0], 0xDFF096), vec![0x000F, 0x820F]);
    assert_eq!(g[0][16].2 - g[0][0].2, 120);
    assert_eq!(g[0][17].2 - g[0][16].2, 18);
    // the first tick restarts the four voices on the silent sample, B (channel 3) second
    let t = &g[1];
    assert_eq!(reg(t, 0xDFF096), vec![0x0001, 0x0009, 0x000B, 0x000F, 0x820F, 0x0000]);
    for ch in 0..4 {
        let b = 0xDFF0A0 + 16 * ch;
        assert_eq!(reg(t, b), vec![(s >> 16) as u16]);
        assert_eq!(reg(t, b + 2), vec![s as u16]);
        assert_eq!(reg(t, b + 4), vec![1]);
        assert_eq!(reg(t, b + 6), vec![0x8C]);
        assert_eq!(reg(t, b + 8), vec![0]);
    }
    // the voices were computed as silent with the sample already at S: PER and VOL only
    let t = &g[2];
    assert_eq!(reg(t, 0xDFF096), vec![0x820F, 0x0000]);
    assert_eq!(reg(t, 0xDFF0A2), Vec::<u16>::new());
    assert_eq!(reg(t, 0xDFF0A6), vec![0x8C]);
    assert_eq!(reg(t, 0xDFF0A8), vec![0]);
}

#[test]
fn tone_in_voice_a() {
    // tone A on (mixer bit 0 clear), period 0x1FC, level 15
    let d = module(0, &[(0x00, 0x01FC), (0x02, 0x000F), (0x0E, 0x003E)], &[]);
    let (log, _) = run(d, 5);
    let s = silent_s(&log);
    let g = groups(&log);
    // tick 1 commits the start state; tick 2 the first computed state: a restart on square sample 5
    let t = &g[2];
    assert_eq!(reg(t, 0xDFF096)[0], 0x0001);
    let lc = s + 2 + 0x62;
    assert_eq!(reg(t, 0xDFF0A0), vec![(lc >> 16) as u16]);
    assert_eq!(reg(t, 0xDFF0A2), vec![lc as u16]);
    assert_eq!(reg(t, 0xDFF0A4), vec![32]);
    assert_eq!(reg(t, 0xDFF0A6), vec![(((0x1FCu32 * 0xE517) >> 5) >> 12) as u16]);
    assert_eq!(reg(t, 0xDFF0A8), vec![64]);
    // the same octave again: only PER and VOL, no restart, no new address
    let t = &g[3];
    assert_eq!(reg(t, 0xDFF0A2), Vec::<u16>::new());
    assert_eq!(reg(t, 0xDFF0A4), Vec::<u16>::new());
    assert_eq!(reg(t, 0xDFF0A6).len(), 1);
    assert_eq!(reg(t, 0xDFF0A8), vec![64]);
    assert_eq!(reg(t, 0xDFF096), vec![0x820F, 0x0000]);
    // the fourth voice copies voice B, which is silent
    assert_eq!(reg(t, 0xDFF0C8), vec![0]);
}

#[test]
fn run_is_deterministic() {
    let d = module(0, &[(0x00, 0x0123), (0x02, 0x000A), (0x04, 0x0040), (0x06, 0x0010), (0x0C, 0x0005), (0x0E, 0x0006)], &[]);
    let (a, _) = run(d.clone(), 6);
    let (b, _) = run(d, 6);
    assert!(a.len() > 100);
    assert_eq!(a, b);
}

#[test]
fn tone_with_the_envelope_divides_by_the_scaled_period() {
    // envelope period 0 is taken as 1 and the division is fine; a SID period of 0 faults
    let d = module(0, &[(0x00, 0x0100), (0x02, 0x0010), (0x0E, 0x003E), (0x12, 0)], &[]);
    let (_, e) = run(d, 4);
    assert!(!e.songend());
    // SID mode 1 (record A at 0x1E + 2) with period 0 (record + 6)
    let d = module(0, &[(0x00, 0x0100), (0x02, 0x0010), (0x0E, 0x003E), (0x20, 1), (0x24, 0)], &[]);
    let (_, e) = run(d, 6);
    assert!(e.songend());
}

#[test]
fn a_play_routine_that_never_returns_ends_the_song() {
    let mut d = module(0, &[], &[]);
    put16(&mut d, PLAY, 0x60FE);
    let (_, e) = run(d, 3);
    assert!(e.songend());
}

#[test]
fn an_init_routine_that_never_returns_is_not_played() {
    let mut d = module(0, &[], &[]);
    put16(&mut d, YZ, 0x60FE);
    assert!(engine(d, "ym.hang", &[]).is_none());
}

#[test]
fn the_loop_of_a_module_driven_voice_is_written_from_set_b() {
    // voice A driven by the module (flag bit 7 with the restart bit), set A and set B with other
    // addresses and lengths, stored by the init routine: the first commit writes set A in full, the
    // second only set B's address and length, later ones nothing of the voice
    let init = [
        (0x1E, 0x8080),
        (0x3E, 0x0001),
        (0x40, 0x2000),
        (0x42, 0x0010),
        (0x44, 0x0100),
        (0x46, 0x0040),
        (0x48, 0x0002),
        (0x4A, 0x0000),
        (0x4C, 0x0020),
    ];
    let (log, _) = run(module_i(0, &init, &[], &[]), 6);
    let g = groups(&log);
    assert_eq!(reg(&g[1], 0xDFF0A0), vec![0x0001]);
    assert_eq!(reg(&g[1], 0xDFF0A2), vec![0x2000]);
    assert_eq!(reg(&g[1], 0xDFF0A4), vec![0x10]);
    assert_eq!(reg(&g[1], 0xDFF0A6), vec![0x100]);
    assert_eq!(reg(&g[1], 0xDFF0A8), vec![0x40]);
    assert_eq!(reg(&g[2], 0xDFF0A0), vec![0x0002]);
    assert_eq!(reg(&g[2], 0xDFF0A2), vec![0x0000]);
    assert_eq!(reg(&g[2], 0xDFF0A4), vec![0x20]);
    assert_eq!(reg(&g[2], 0xDFF0A6), Vec::<u16>::new());
    assert_eq!(reg(&g[2], 0xDFF0A8), Vec::<u16>::new());
    for t in &g[3..] {
        assert_eq!(reg(t, 0xDFF0A0), Vec::<u16>::new());
        assert_eq!(reg(t, 0xDFF0A4), Vec::<u16>::new());
    }
}

#[test]
fn services_convert_the_bytes() {
    let mut m = Machine::new(CpuType::M68000);
    let area = m.alloc(AREA);
    let svc = m.alloc(64);
    let e = Emu { s: area + BEFORE, blk: off(area + BEFORE, O_BLK), svc };
    e.init(&mut m.bus);
    let buf = m.alloc(16);
    let mut c = Cpu68::new(m.alloc(256) + 256, e);
    for (k, input, want) in [(10u32, 0x0Fu8, 0x7Fu8), (10, 0x00, 0x80), (10, 0x80, 0x80), (11, 0x0F, 0x7F), (11, 0x03, 0xBF), (12, 0x12, 0x92)] {
        m.bus.w8(buf, input);
        c.a(0, buf);
        c.a(1, buf + 1);
        c.service(&mut m.bus, k);
        assert_eq!(m.bus.r8(buf), want, "service {k} on {input:#x}");
    }
}

/// A type 2 pair: the data file's relocation routine returns a0 and a1 so that the length is 4,
/// the replay stores the length it was handed into the tone period of voice A.
fn type2() -> (Vec<u8>, Vec<u8>) {
    let mut d = module(2, &[], &[(b'O', 0x1C0), (b'X', 0x140)]);
    d[0x1C0..0x1C7].copy_from_slice(b"YM.rep\0");
    // load the data address into a0 and the same plus 0x40 into a1
    put16(&mut d, 0x140, 0x41FA);
    put16(&mut d, 0x142, 0x0040 - 2);
    put16(&mut d, 0x144, 0x43FA);
    put16(&mut d, 0x146, 0x0080 - 6);
    put16(&mut d, 0x148, 0x4E75);
    // the start routine of the data file hands the same two addresses over
    put16(&mut d, YZ, 0x41FA);
    put16(&mut d, YZ + 2, (0x180 - (YZ + 2)) as u16);
    put16(&mut d, YZ + 4, 0x43FA);
    put16(&mut d, YZ + 6, (0x1C0 - (YZ + 6)) as u16);
    put16(&mut d, YZ + 8, 0x4E75);
    let mut r = module(0, &[], &[(b'c', 0x1E0), (b'd', 0x1E4)]);
    // play: block address, then copy the low word of the length slot into the tone period, the
    // mixer to tone A only and level 15
    let code: [u16; 12] = [0x207A, (YP as i32 - (PLAY as i32 + 2)) as u16, 0x317A, (0x1E6i32 - (PLAY as i32 + 6)) as u16, 0x0000, 0x317C, 0x000F, 0x0002, 0x317C, 0x003E, 0x000E, 0x4E75];
    for (i, w) in code.iter().enumerate() {
        put16(&mut r, PLAY + 2 * i, *w);
    }
    (d, r)
}

#[test]
fn type_two_hands_the_data_over_to_the_replay() {
    let (d, r) = type2();
    let mut e = engine(d.clone(), "ym.data", &[("YM.rep", r.clone())]).expect("plays with its replay file");
    let mut out = vec![0f32; 2 * 960 * 4];
    e.render(&mut out);
    let g = groups(&e.m.bus.write_log.take().unwrap());
    // the two addresses differ by 0x40: period 0x40 is octave 2: four words
    let t = &g[2];
    assert_eq!(reg(t, 0xDFF0A4), vec![4]);
    // the replay file is looked for by name; without it the module is not played
    assert!(engine(d, "ym.data", &[]).is_none());
}

#[test]
fn subsong_numbers_reach_the_init_routine() {
    let mut d = module(0, &[], &[(b']', 2), (b'^', 4)]);
    // the init routine: block address, then the subsong number into the mixer field
    let code: [u16; 5] = [0x207A, (YP as i32 - (YZ as i32 + 2)) as u16, 0x3140, 0x0018, 0x4E75];
    for (i, w) in code.iter().enumerate() {
        put16(&mut d, YZ + 2 * i, *w);
    }
    let mut m = Machine::new(CpuType::M68000);
    let base = m.alloc(d.len() as u32);
    m.bus.write_bytes(base, &d);
    let mut r = new(&d, base, &mut m).expect("recognised");
    let blk = m.bus.r32(base + YP as u32);
    assert_eq!(r.subsongs(), (2, 4, 2));
    assert_eq!(m.bus.r16(blk + 0x18), 2);
    m.bus.write_log = Some(Vec::new());
    r.set_subsong(4, &mut Hw { bus: &mut m.bus });
    assert_eq!(m.bus.r16(blk + 0x18), 4);
    let log = m.bus.write_log.take().unwrap();
    // the end of the old subsong: DMA off, volumes zero
    assert_eq!(log.iter().map(|w| (w.0, w.1)).collect::<Vec<_>>(), vec![(0xDFF096, 0xF), (0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0)]);
    // a number outside the range is ignored
    r.set_subsong(9, &mut Hw { bus: &mut m.bus });
    assert_eq!(m.bus.r16(blk + 0x18), 4);
}

#[test]
fn the_subsong_map_gives_the_value() {
    let mut d = module(0, &[], &[(b']', 1), (b'^', 2), (b'`', 0x1F0)]);
    put32(&mut d, 0x1F0, 7);
    put32(&mut d, 0x1F4, 9);
    let code: [u16; 5] = [0x207A, (YP as i32 - (YZ as i32 + 2)) as u16, 0x3140, 0x0018, 0x4E75];
    for (i, w) in code.iter().enumerate() {
        put16(&mut d, YZ + 2 * i, *w);
    }
    let mut m = Machine::new(CpuType::M68000);
    let base = m.alloc(d.len() as u32);
    m.bus.write_bytes(base, &d);
    let mut r = new(&d, base, &mut m).expect("recognised");
    let blk = m.bus.r32(base + YP as u32);
    assert_eq!(m.bus.r16(blk + 0x18), 7);
    r.set_subsong(2, &mut Hw { bus: &mut m.bus });
    assert_eq!(m.bus.r16(blk + 0x18), 9);
}
