use super::*;
use m68k::CpuType;

const BASE: u32 = 0x10000;

/// Object indices of the test modules (0..3 are the voices' streams).
const W1: u8 = 4;
const W2: u8 = 5;
const VENV: u8 = 6;
const SEQ: u8 = 7;
const PENV: u8 = 8;

/// Builds a module of one subsong: the objects, then the song table, then the 12-byte header.
fn build(objs: &[Vec<u8>]) -> Vec<u8> {
    let mut d = Vec::new();
    let mut offs = Vec::new();
    for o in objs {
        offs.push(d.len());
        d.extend_from_slice(o);
        if d.len() % 2 != 0 {
            d.push(0);
        }
    }
    let table = d.len();
    for o in &offs {
        d.extend_from_slice(&((*o as i32 - table as i32) as u32).to_be_bytes());
    }
    let hdr = d.len();
    d.extend_from_slice(b"VSS0");
    d.extend_from_slice(&1u32.to_be_bytes());
    d.extend_from_slice(&((table as i32 - hdr as i32) as u32).to_be_bytes());
    d
}

/// A module whose voice 0 plays `stream0` and whose other voices play `rest` (a long rest);
/// objects 4.. are `more` (`W1`, `W2`, `VENV`, `SEQ`, `PENV`, `SMP`).
fn module(stream0: &[u8], more: &[Vec<u8>]) -> Vec<u8> {
    let rest = vec![0xFF, 0x00];
    let mut objs = vec![stream0.to_vec(), rest.clone(), rest.clone(), rest];
    objs.extend_from_slice(more);
    build(&objs)
}

fn waves() -> Vec<Vec<u8>> {
    let w1: Vec<u8> = (0..32).map(|i| (i as i8 * 4 - 60) as u8).collect();
    let w2: Vec<u8> = (0..32).map(|i| (100 - i as i32 * 5) as u8).collect();
    // the volume envelope sets 0x30 and holds; the wave sequence holds speed 2
    vec![w1, w2, vec![0x30, 0x00, 0x00, 0xFF], vec![0x02, 0xFF], vec![0x00, 0xFF]]
}

struct Run {
    m: Machine,
    r: Box<dyn Replayer>,
    ticks: u64,
}

fn start(d: &[u8]) -> Run {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.ram[BASE as usize..BASE as usize + d.len()].copy_from_slice(d);
    m.bus.clock = 0x1000;
    m.bus.write_log = Some(Vec::new());
    let r = new(d, BASE, &mut m).expect("recognised");
    Run { m, r, ticks: 0 }
}

impl Run {
    fn tick(&mut self) {
        self.m.bus.clock = 1_000_000 + TICK * self.ticks;
        self.ticks += 1;
        let mut hw = Hw { bus: &mut self.m.bus };
        self.r.tick(&mut hw);
    }

    fn log(&self) -> Vec<(u32, u16, u64)> {
        self.m.bus.write_log.clone().unwrap()
    }

    /// The last AUDxLC written for voice `v`.
    fn lc(&self, v: usize) -> u32 {
        let a = 0xDFF0A0 + 16 * v as u32;
        let l = self.log();
        let k = l.iter().rposition(|x| x.0 == a).unwrap();
        ((l[k].1 as u32) << 16) | l[k + 1].1 as u32
    }

    fn bytes(&self, a: u32, n: usize) -> Vec<u8> {
        (0..n as u32).map(|i| self.m.bus.r8(a + i)).collect()
    }

    fn irq(&mut self, ch: usize) {
        self.m.bus.paula.write_intreq(0x8000 | 0x80 << ch);
        let mut hw = Hw { bus: &mut self.m.bus };
        assert!(self.r.audio_irq_ch(ch, &mut hw));
    }
}

fn obj(i: u8) -> u8 {
    i
}

/// Voice 0 stream: waves, volume envelope, wave sequence with mode `m`, then a note of
/// duration 0x40 and a long rest.
fn synth_stream(m: u8, extra: &[u8]) -> Vec<u8> {
    let mut s = vec![0x85, obj(W1), obj(W2), 0x86, obj(VENV), 0x88, obj(SEQ), m];
    s.extend_from_slice(extra);
    s.extend_from_slice(&[0x00, 0x40, 0xFF, 0x00]);
    s
}

fn per_of(i: u8) -> u16 {
    period_at(i).unwrap()
}

#[test]
fn detection() {
    let d = module(&synth_stream(0, &[]), &waves());
    assert!(detect(&d));
    let mut e = d.clone();
    e[d.len() - 12] = b'X';
    assert!(!detect(&e));
    // odd size, too short, subsong count of 256 or more, song table outside the file
    assert!(!detect(&d[..d.len() - 1]));
    assert!(!detect(&[]));
    assert!(!detect(b"VSS0"));
    let mut e = d.clone();
    let n = e.len();
    e[n - 8..n - 4].copy_from_slice(&256u32.to_be_bytes());
    assert!(!detect(&e));
    let mut e = d.clone();
    e[n - 4..].copy_from_slice(&0x7FFF_0000u32.to_be_bytes());
    assert!(!detect(&e));
    // the header may sit up to 64 bytes from the end, at an even offset
    let mut e = d.clone();
    e.extend_from_slice(&[0u8; 20]);
    assert!(detect(&e));
    e.push(0);
    assert!(!detect(&e));
}

#[test]
fn tables_are_the_players() {
    assert_eq!(PERIODS.len(), 192);
    assert_eq!(RATIOS.len(), 52);
    assert_eq!(per_of(0), 0x7E00);
    assert_eq!(per_of(95), 0x85);
    // past the period table the ratio words follow; past those nothing is specified
    assert_eq!(period_at(96), Some(1));
    assert_eq!(period_at(96 + 25), Some(1));
    assert_eq!(period_at(122), None);
}

#[test]
fn start_up_writes_and_clocks() {
    let d = module(&synth_stream(0, &[]), &waves());
    let r = start(&d);
    let l = r.log();
    let regs: Vec<u32> = l.iter().map(|x| x.0).collect();
    assert_eq!(&regs[..4], &[0xDFF0A4, 0xDFF0B4, 0xDFF0C4, 0xDFF0D4]);
    assert_eq!(&regs[4..8], &[0xDFF0A8, 0xDFF0B8, 0xDFF0C8, 0xDFF0D8]);
    assert!(l[..4].iter().all(|x| x.1 == 0x10) && l[4..8].iter().all(|x| x.1 == 0));
    // LC: the silent area, high word then low word at the same clock
    assert_eq!((l[8].0, l[9].0), (0xDFF0A0, 0xDFF0A2));
    assert_eq!(l[8].2, l[9].2);
    let zero = ((l[8].1 as u32) << 16) | l[9].1 as u32;
    assert_eq!(zero % 2, 0);
    assert_eq!((l[16].0, l[16].1), (0xDFF096, 0x800F));
    assert_eq!((l[17].0, l[17].1), (0xDFF09A, 0x8780));
    let t0 = l[0].2;
    assert_eq!(l[1].2 - t0, 8);
    assert_eq!(l[4].2 - t0, 32);
    assert_eq!(l[8].2 - t0, 192);
    assert_eq!(l[16].2 - t0, 248);
    assert_eq!(l[17].2 - t0, INTENA_AFTER);
}

#[test]
fn first_tick_writes_each_voice_in_order_and_spacing() {
    let d = module(&synth_stream(0, &[]), &waves());
    let mut r = start(&d);
    let n0 = r.log().len();
    r.tick();
    let l = r.log()[n0..].to_vec();
    // per voice: DMACON, PER, VOL, then (voice 0, mode A) LC
    let dma: Vec<usize> = (0..l.len()).filter(|&i| l[i].0 == 0xDFF096).collect();
    assert_eq!(dma.len(), 4);
    for (v, &i) in dma.iter().enumerate() {
        assert_eq!(l[i].1, 0x8000 | 1 << v);
        assert_eq!(l[i + 1].0, 0xDFF0A6 + 16 * v as u32);
        assert_eq!(l[i + 2].0, 0xDFF0A8 + 16 * v as u32);
        // PER 18 colour clocks after DMACON, VOL 15 after PER; the first tick writes zeros
        assert_eq!(l[i + 1].2 - l[i].2, 18);
        assert_eq!(l[i + 2].2 - l[i + 1].2, 15);
        assert_eq!((l[i + 1].1, l[i + 2].1), (0, 0));
    }
    // the second tick writes the period and the volume computed in the first
    r.tick();
    // voice 0 is in mode A: when the wave sequence is not read, its LC follows the VOL by 180
    // colour clocks
    let l2 = r.log()[n0 + l.len()..].to_vec();
    let i = l2.iter().position(|x| x.0 == 0xDFF096).unwrap();
    assert_eq!(l2[i + 3].0, 0xDFF0A0);
    assert_eq!(l2[i + 3].2 - l2[i + 2].2, 180);
    let l = r.log()[n0..].to_vec();
    let pers: Vec<u16> = l.iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    let vols: Vec<u16> = l.iter().filter(|x| x.0 == 0xDFF0A8).map(|x| x.1).collect();
    assert_eq!(pers[1], per_of(0));
    assert_eq!(vols[1], 0x30);
}

#[test]
fn mode_a_mixes_two_waves() {
    let m = waves();
    let d = module(&synth_stream(0x00, &[]), &m);
    let mut r = start(&d);
    r.tick();
    r.tick();
    // the second tick hands over the half filled by the first, with phase 2
    let got = r.bytes(r.lc(0), 32);
    for i in 0..32usize {
        let a = m[0][i] as i8 as i32;
        let b = m[1][(2 + i) & 31] as i8 as i32;
        assert_eq!(got[i], ((a + b) >> 1) as u8, "byte {i}");
    }
    // a mask is OR-ed into the magnitude of a negative result and into a positive one
    let d = module(&synth_stream(0x00, &[0x8D, 0x03]), &m);
    let mut r = start(&d);
    r.tick();
    r.tick();
    let got = r.bytes(r.lc(0), 32);
    for i in 0..32usize {
        let a = m[0][i] as i8 as i32;
        let b = m[1][(2 + i) & 31] as i8 as i32;
        let s = ((a + b) >> 1) as u8;
        let want = if s & 0x80 != 0 { 0u8.wrapping_sub(0u8.wrapping_sub(s) | 3) } else { s | 3 };
        assert_eq!(got[i], want, "byte {i}");
    }
}

#[test]
fn mode_b_xors_a_window_of_wave_two() {
    let m = vec![waves().remove(0), waves().remove(1), waves().remove(2), vec![0x04, 0xFF], vec![0x00, 0xFF]];
    let d = module(&synth_stream(0x40, &[0x8D, 0xFF]), &m);
    let mut r = start(&d);
    r.tick();
    r.tick();
    // speed 4: a window of four bytes at phase 0 over the zeroed previous half
    let got = r.bytes(r.lc(0), 32);
    let w2 = &m[1];
    let mut want = vec![0u8; 32];
    want[..4].copy_from_slice(&w2[..4]);
    assert_eq!(got, want);
    // the next half starts at phase 4, the window xors into the previous half
    r.tick();
    let got2 = r.bytes(r.lc(0), 32);
    let mut want2 = want.clone();
    for i in 4..8 {
        want2[i] ^= w2[i];
    }
    assert_eq!(got2, want2);
    // LC follows VOL by 137 colour clocks in mode B
    let l = r.log();
    let k = l.iter().rposition(|x| x.0 == 0xDFF0A0).unwrap();
    assert_eq!(l[k].2 - l[k - 1].2, 137);
}

#[test]
fn mode_b_copy_and_mode_c_morph() {
    let w = waves();
    // speed bit 7 in mode B copies wave 1
    let m = vec![w[0].clone(), w[1].clone(), w[2].clone(), vec![0x80, 0xFF], vec![0x00, 0xFF]];
    let mut r = start(&module(&synth_stream(0x40, &[]), &m));
    r.tick();
    r.tick();
    assert_eq!(r.bytes(r.lc(0), 32), w[0]);
    // mode C, speed 8 (a window of 8 at phase 0, target wave 2), step 3, over zeros
    let m = vec![w[0].clone(), w[1].clone(), w[2].clone(), vec![0x08, 0x03, 0xFF], vec![0x00, 0xFF]];
    let mut r = start(&module(&synth_stream(0x80, &[]), &m));
    r.tick();
    r.tick();
    let got = r.bytes(r.lc(0), 32);
    for i in 0..32usize {
        let want = if i < 8 {
            let c = 0x80i32;
            let t = (w[1][i] ^ 0x80) as i32;
            (if (t - c).abs() <= 3 { t } else if t < c { c - 3 } else { c + 3 }) as u8 ^ 0x80
        } else {
            0
        };
        assert_eq!(got[i], want, "byte {i}");
    }
    // LC follows VOL by 150 or 151 colour clocks in mode C
    let l = r.log();
    let k = l.iter().rposition(|x| x.0 == 0xDFF0A0).unwrap();
    assert!(matches!(l[k].2 - l[k - 1].2, 150 | 151));
}

#[test]
fn mode_d_steps_through_the_sample_and_ends() {
    // object 9 is the length word, object 10 the sample data that follows it directly
    let len = 300usize;
    let w = waves();
    let o9 = (len as u16).to_be_bytes().to_vec();
    let rest = vec![0xFF, 0x00];
    let objs = vec![
        vec![0x85, obj(W1), 10, 0x88, obj(SEQ), 0x20, 0x00, 0x40, 0xFF, 0x00],
        rest.clone(),
        rest.clone(),
        rest,
        w[0].clone(),
        w[1].clone(),
        w[2].clone(),
        vec![0x00, 0x00, 0xFF],
        vec![0x00, 0xFF],
        o9,
        (0..len).map(|i| i as u8).collect(),
    ];
    let d = build(&objs);
    let mut r = start(&d);
    r.tick();
    // 0x88 with mode bit 5 writes AUDxLEN = 0x40 words
    assert!(r.log().iter().any(|x| x.0 == 0xDFF0A4 && x.1 == 0x40));
    let n = r.log().len();
    r.irq(0);
    r.irq(0);
    r.irq(0);
    r.irq(0);
    let l = r.log()[n..].to_vec();
    let lcs: Vec<u32> = l.iter().filter(|x| x.0 == 0xDFF0A0).map(|x| x.1 as u32).collect();
    let los: Vec<u32> = l.iter().filter(|x| x.0 == 0xDFF0A2).map(|x| x.1 as u32).collect();
    let addr: Vec<u32> = lcs.iter().zip(&los).map(|(h, l)| h << 16 | l).collect();
    assert_eq!(addr.len(), 4, "{l:?}");
    assert_eq!(addr[1] - addr[0], 0x80);
    // the third step goes past the end (300 bytes): silent area, and the ended state keeps it
    assert_eq!(addr[3], addr[2]);
    assert!(addr[2] != addr[1] && addr[2] < addr[0]);
    // every interrupt is acknowledged, 168 cycles after the LC write in sample mode
    let acks: Vec<_> = l.iter().filter(|x| x.0 == INTREQ).collect();
    assert_eq!(acks.len(), 4);
    assert!(acks.iter().all(|x| x.1 == 0x80));
}

#[test]
fn handler_acknowledges_in_synth_modes() {
    let d = module(&synth_stream(0, &[]), &waves());
    let mut r = start(&d);
    r.tick();
    let n = r.log().len();
    r.m.bus.clock = 5_000_000;
    r.irq(2);
    let l = r.log()[n..].to_vec();
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].0, l[0].1, l[0].2), (INTREQ, 0x200, 5_000_000 + 176));
    assert_eq!(r.m.bus.clock, 5_000_000 + 184);
}

#[test]
fn pitch_commands() {
    let w = waves();
    let mut m = vec![w[0].clone(), w[1].clone(), w[2].clone(), w[3].clone()];
    // add 5 for 255 ticks
    m.push(vec![0x05, 0xFF]);
    let mut r = start(&module(&synth_stream(0, &[0x87, obj(PENV)]), &m));
    for _ in 0..4 {
        r.tick();
    }
    // voice 0's PER writes are every fourth entry: 0, then the previous tick's period
    let voice0: Vec<u16> = r.log().iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    let base = per_of(0);
    assert_eq!(voice0, vec![0, base + 5, base + 10, base + 15]);
    // 0xFE doubles, 0x7F halves, 0x7E with 12 up halves the period once, with 0x8C down doubles
    for (cmd, cnt, want) in [(0xFEu8, 0xFFu8, (base as u32 * 2) as u16), (0x7F, 0xFF, base >> 1), (0x7E, 12, base >> 1), (0x7E, 0x8C, base << 1)] {
        let mut m = m.clone();
        m[4] = vec![cmd, cnt, 0x00, 0xFF];
        let mut r = start(&module(&synth_stream(0, &[0x87, obj(PENV)]), &m));
        r.tick();
        r.tick();
        let l: Vec<u16> = r.log().iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
        assert_eq!(l[1], want, "command {cmd:02x} {cnt:02x}");
    }
}

#[test]
fn portamento_moves_the_period_by_steps() {
    let w = waves();
    let m = vec![w[0].clone(), w[1].clone(), w[2].clone(), w[3].clone()];
    // from note 0 to note 12 over 255 ticks: downward steps of diff / 255
    let s = vec![0x85, obj(W1), obj(W2), 0x86, obj(VENV), 0x88, obj(SEQ), 0x00, 0x89, 0, 12, 255, 0xFF, 0x00];
    let mut r = start(&module(&s, &m));
    for _ in 0..4 {
        r.tick();
    }
    let (p1, p2) = (per_of(0), per_of(12));
    let step = (p1 - p2) / 255;
    let voice0: Vec<u16> = r.log().iter().filter(|x| x.0 == 0xDFF0A6).map(|x| x.1).collect();
    assert_eq!(voice0, vec![0, p1 - step, p1 - 2 * step, p1 - 3 * step]);
}

#[test]
fn song_end_after_four_jumps_in_a_tick() {
    // every voice jumps to a one-tick note that jumps back
    let sub = |j: u8| vec![0x00, 0x01, 0x8B, j];
    let objs = vec![vec![0x8B, 4], vec![0x8B, 5], vec![0x8B, 6], vec![0x8B, 7], sub(4), sub(5), sub(6), sub(7)];
    let mut r = start(&build(&objs));
    assert!(!r.r.songend());
    r.tick();
    assert!(r.r.songend());
}

#[test]
fn undefined_cases_end_the_voice_and_never_hang() {
    let streams: Vec<Vec<u8>> = vec![
        vec![0x80, 0x00],
        vec![0x8E, 0x00],
        vec![0xFE, 0x00],
        // return with nothing on the stack
        vec![0x82],
        // a call that calls itself: nesting past 20 entries
        vec![0x81, 0],
        // a loop that never ends a read
        vec![0x8B, 0],
        // loop start nested past the stack
        vec![0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0, 0x83, 1, 0],
        // a note past the tables, a portamento with length 0 and with equal notes
        vec![0x7F, 0x05],
        vec![0x89, 0, 12, 0],
        vec![0x89, 3, 3, 4],
        // an envelope that is a loop marker forever
        vec![0x86, 4, 0x00, 0x05],
    ];
    for s in streams {
        let m = vec![vec![0x88, 0x00, 0x88, 0x00], vec![0u8; 4], vec![0xFF; 4]];
        let mut r = start(&module(&s, &m));
        for _ in 0..40 {
            r.tick();
        }
        // after the first tick it is quiet: no volume above the start
        let vols: Vec<u16> = r.log().iter().filter(|x| x.0 == 0xDFF0A8).map(|x| x.1).collect();
        assert!(vols.iter().skip(4).all(|&v| v == 0), "{s:02x?}");
    }
    // a stream that runs off its object reads on into the next one: no panic, no hang
    for s in [vec![0x85], vec![0x88], vec![0x89]] {
        let mut r = start(&module(&s, &[]));
        for _ in 0..40 {
            r.tick();
        }
    }
    // the transpose makes a note index of 122 or more
    let mut r = start(&module(&[0x8A, 200, 0x00, 0x05, 0xFF, 0x00], &[]));
    for _ in 0..4 {
        r.tick();
    }
}

#[test]
fn reads_of_unset_pointers_give_the_hosts_low_memory() {
    let hw = Hw { bus: &mut Machine::new(CpuType::M68000).bus };
    assert_eq!([mem8(&hw, 0), mem8(&hw, 3), mem8(&hw, 6), mem8(&hw, 7)], [0, 0, 0x23, 0x48]);
    assert_eq!([mem8(&hw, 8), mem8(&hw, 10), mem8(&hw, 11), mem8(&hw, 0x3FF)], [0, 4, 4, 4]);
}

#[test]
fn malformed_input_is_not_recognised_or_harmless() {
    // random bytes with the magic near the end must neither panic nor hang
    let mut seed = 0x1234_5678u32;
    for n in 0..40 {
        let len = 70 + 2 * n;
        let mut d: Vec<u8> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        let o = len - 12;
        d[o..o + 4].copy_from_slice(b"VSS0");
        d[o + 4..o + 8].copy_from_slice(&1u32.to_be_bytes());
        d[o + 8..o + 12].copy_from_slice(&(-(o as i32) as u32).to_be_bytes());
        if detect(&d) {
            let mut r = start(&d);
            for _ in 0..8 {
                r.tick();
            }
        }
    }
}

#[test]
fn subsongs_follow_the_header() {
    let d = module(&synth_stream(0, &[]), &waves());
    let r = start(&d);
    assert_eq!(r.r.subsongs(), (0, 0, 0));
}

#[test]
fn renders_the_same_twice_and_is_not_silent() {
    use crate::engine::Engine;
    use std::path::Path;
    let d = module(&synth_stream(0, &[0x8D, 0x00]), &waves());
    let render = |d: &[u8]| {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new("t.vss"), d.to_vec()).unwrap();
        let mut out = Vec::new();
        let mut buf = vec![0f32; 2 * 960];
        for _ in 0..40 {
            e.render(&mut buf);
            out.extend_from_slice(&buf);
        }
        out
    };
    let a = render(&d);
    let b = render(&d);
    assert_eq!(a, b);
    assert!(a.iter().any(|&x| x.abs() > 0.01));
}
