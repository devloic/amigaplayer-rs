use super::cpu::{Cpu, End, Rng};
use super::*;
use m68k::CpuType;

const T0: u64 = 1_000_000;
const TICK: u64 = 70_940;

/// A SAP file: the header lines, then the binary part with the given segments (start address,
/// bytes).
fn sap(header: &str, segs: &[(u16, &[u8])]) -> Vec<u8> {
    let mut d = header.replace('\n', "\r\n").into_bytes();
    d.extend_from_slice(&[0xFF, 0xFF]);
    for (a, b) in segs {
        d.extend_from_slice(&a.to_le_bytes());
        d.extend_from_slice(&a.wrapping_add(b.len() as u16).wrapping_sub(1).to_le_bytes());
        d.extend_from_slice(b);
    }
    d
}

/// A type B file whose init routine is a bare return and whose play routine stores the given
/// (address, value) pairs, each as: load the value into A, store A at the absolute address.
fn player(stores: &[(u16, u8)]) -> Vec<u8> {
    let mut code = vec![0x60u8];
    for (a, v) in stores {
        code.extend_from_slice(&[0xA9, *v, 0x8D, *a as u8, (*a >> 8) as u8]);
    }
    code.push(0x60);
    sap("SAP\nTYPE B\nINIT 2000\nPLAYER 2001\n", &[(0x2000, &code)])
}

fn cpu_with(code: &[u8]) -> Cpu {
    let mut c = Cpu::new();
    c.mem[0x3000..0x3000 + code.len()].copy_from_slice(code);
    c
}

#[test]
fn detection() {
    let d = player(&[]);
    assert!(detect(&d));
    for t in ["TYPE M", "TYPE B", "TYPE Bx"] {
        let h = format!("SAP\nTYPE B\nINIT 2000\nPLAYER 2001\n").replace("TYPE B", t);
        assert!(detect(&sap(&h, &[(0x2000, &[0x60])])), "{t}");
    }
    assert!(detect(&sap("SAP\nTYPE C\nPLAYER 2000\nMUSIC 3000\n", &[(0x2000, &[0x60])])));
    // rejected types and missing keywords
    for h in [
        "SAP\nTYPE R\nINIT 2000\nPLAYER 2001\n",
        "SAP\nTYPE D\nINIT 2000\nPLAYER 2001\n",
        "SAP\nTYPE S\nINIT 2000\nPLAYER 2001\n",
        "SAP\nINIT 2000\nPLAYER 2001\n",
        "SAP\nTYPE B\nPLAYER 2001\n",
        "SAP\nTYPE B\nINIT 2000\n",
        "SAP\nTYPE C\nPLAYER 2000\n",
        "SAP\nTYPE C\nMUSIC 2000\n",
    ] {
        assert!(!detect(&sap(h, &[(0x2000, &[0x60])])), "{h:?}");
    }
    // no binary part marker, not a SAP file, a binary-load file
    assert!(!detect(b"SAP\r\nTYPE B\r\nINIT 2000\r\nPLAYER 2001\r\n"));
    assert!(!detect(b"XAP\r\nTYPE B\r\nINIT 2000\r\nPLAYER 2001\r\n\xFF\xFF"));
    assert!(!detect(&[0xFF, 0xFF, 0, 0x20, 0, 0x20, 0x60]));
    assert!(!detect(&[]));
    assert!(!detect(b"SAP"));
}

#[test]
fn header_numbers_are_hex_and_keywords_are_found_anywhere() {
    let d = sap("SAP\nNAME \"x TYPE B\"\nSONGS 10\nDEFSONG 3\nTYPE m\nINIT 20aB\nPLAYER 2C\n", &[(0x2000, &[0x60])]);
    // the first TYPE in the text is the one inside the NAME string
    let s = parse(&d).expect("parsed");
    assert_eq!(s.kind, b'B');
    assert_eq!(s.songs, 16);
    assert_eq!(s.defsong, 3);
    assert_eq!(s.init, 0x20AB);
    assert_eq!(s.play, 0x2C);
    // defaults
    let s = parse(&player(&[])).unwrap();
    assert_eq!((s.songs, s.defsong), (1, 0));
    // type C derives its entries from the player address
    let s = parse(&sap("SAP\nTYPE C\nPLAYER 4000\nMUSIC 5000\n", &[(0x4000, &[0x60])])).unwrap();
    assert_eq!((s.init, s.play, s.music), (0x4003, 0x4006, 0x5000));
}

#[test]
fn loader_segments() {
    let mut mem = Box::new([0u8; 65536]);
    let mut d = vec![0xFF, 0xFF, 0x00, 0x10, 0x02, 0x10, 1, 2, 3];
    // the optional marker before a segment, and an end below the start copies one byte
    d.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x20, 0x00, 0x00, 9]);
    // a segment cut short by the end of the file
    d.extend_from_slice(&[0x00, 0x30, 0x05, 0x30, 7, 7]);
    load(&mut mem, &d, 0);
    assert_eq!(&mem[0x1000..0x1004], &[1, 2, 3, 0]);
    assert_eq!(mem[0x2000], 9);
    assert_eq!(mem[0x2001], 0);
    assert_eq!(&mem[0x3000..0x3003], &[7, 7, 0]);
    // a header cut off by the end of the file does nothing
    let mut mem = Box::new([0u8; 65536]);
    load(&mut mem, &[0xFF, 0xFF, 0x00, 0x10], 0);
    assert!(mem.iter().all(|b| *b == 0));
}

#[test]
fn rng_steps() {
    let mut r = Rng::new();
    let a = r.step();
    let b = r.step();
    assert_ne!(a, b);
    // the first step by the rules of the specification
    let (s1, s2) = (0x3F3F_751Fu32, 0x1717_9834u32);
    let mut x = s1.wrapping_add(s2).rotate_right(4);
    x = (x & 0xFFFF_0000) | (x as u16).wrapping_sub((s2 >> 16) as u16) as u32;
    assert_eq!(a, x);
    let mut r2 = Rng::new();
    assert_eq!(r2.step(), a);
}

#[test]
fn cpu_adc_zero_flag_and_basic_alu() {
    // clear carry, load 255, add 1 (result zero but the remembered flag is clear), branch if
    // zero not taken, then load 7 and store it
    let mut c = cpu_with(&[0x18, 0xA9, 0xFF, 0x69, 0x01, 0xF0, 0x04, 0xA9, 0x07, 0x85, 0x40, 0x60]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!(c.mem[0x40], 7, "zero flag stays clear after a zero sum");
    // after a compare that found equality the zero flag is remembered set
    let mut c = cpu_with(&[0xA9, 0x05, 0xC9, 0x05, 0x18, 0xA9, 0xFF, 0x69, 0x01, 0xF0, 0x04, 0xA9, 0x07, 0x85, 0x40, 0x60]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!(c.mem[0x40], 0, "zero flag kept from the compare");
    // binary arithmetic even with the decimal flag set
    let mut c = cpu_with(&[0xF8, 0x18, 0xA9, 0x09, 0x69, 0x01, 0x85, 0x41, 0x60]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!(c.mem[0x41], 0x0A);
}

#[test]
fn cpu_brk_skips_a_byte_and_php_pushes_b_clear() {
    // a break, one skipped byte, then store 1; push the status, pull it into A and store it
    let mut c = cpu_with(&[0x00, 0xFF, 0xA9, 0x01, 0x85, 0x40, 0x08, 0x68, 0x85, 0x41, 0x60]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!(c.mem[0x40], 1);
    assert_eq!(c.mem[0x41] & 0x30, 0x20);
}

#[test]
fn cpu_stack_rules() {
    // a pull on the empty stack reads the byte at $0200; a second one fails the call
    let mut c = cpu_with(&[0x68, 0x60]);
    c.mem[0x200] = 0x5A;
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    let mut c = cpu_with(&[0x68, 0x68, 0x60]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Fail);
    // 256 pushes fit, the 257th fails
    let mut code = vec![0x48u8; 257];
    code.push(0x60);
    let mut c = cpu_with(&code);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Fail);
    // a subroutine sees the stack pointer value of the specification
    let mut c = cpu_with(&[0x20, 0x10, 0x30, 0x60]);
    c.mem[0x3010] = 0xBA;
    c.mem[0x3011] = 0x86;
    c.mem[0x3012] = 0x40;
    c.mem[0x3013] = 0x60;
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!(c.mem[0x40], 0xFD);
}

#[test]
fn cpu_exit_vectors_and_set_vector() {
    // a jump to the OS exit ends the call
    let mut c = cpu_with(&[0xA9, 0x09, 0x85, 0x40, 0x4C, 0x62, 0xE4, 0xA9, 0x01, 0x85, 0x40]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!(c.mem[0x40], 9);
    // the set-vector entry with A = 6 stores Y and X, nothing is pushed
    let mut c = cpu_with(&[0xA9, 0x06, 0xA2, 0x12, 0xA0, 0x34, 0x20, 0x5C, 0xE4, 0x60]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!((c.mem[0x222], c.mem[0x223]), (0x34, 0x12));
    // an undocumented opcode fails the call
    let mut c = cpu_with(&[0x02]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Fail);
}

#[test]
fn cpu_pointer_at_ff_reads_high_byte_from_the_stack_page() {
    let mut c = cpu_with(&[0xA0, 0x00, 0xB1, 0xFF, 0x85, 0x40, 0x60]);
    c.mem[0xFF] = 0x10;
    c.mem[0x100] = 0x31;
    c.mem[0x3110] = 0x77;
    c.mem[0x0010] = 0x11;
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!(c.mem[0x40], 0x77);
}

#[test]
fn cpu_hardware_reads() {
    // RANDOM through a checked form is a generator step; the same through a plain form is memory
    let mut c = cpu_with(&[0xAD, 0x0A, 0xD2, 0x85, 0x40, 0xA9, 0x00, 0x0D, 0x0A, 0xD2, 0x85, 0x41, 0x60]);
    c.mem[0xD20A] = 0x99;
    let mut r = Rng::new();
    let want = ((r.step() & 0xFFFF) >> 5) as u8;
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!(c.mem[0x40], want);
    assert_eq!(c.mem[0x41], 0x99);
    assert_eq!(c.rng.s1, r.s1);
    // the values preset at the start of a call
    let mut c = cpu_with(&[0xAD, 0x1E, 0xD0, 0x85, 0x40, 0xAD, 0x0F, 0xD4, 0x85, 0x41, 0x60]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Ok);
    assert_eq!((c.mem[0x40], c.mem[0x41]), (1, 0x40));
}

#[test]
fn cpu_endless_loop_fails_instead_of_hanging() {
    let mut c = cpu_with(&[0x4C, 0x00, 0x30]);
    assert_eq!(c.call(0x3000, 0, 0, 0, 0), End::Fail);
}

struct Rig {
    m: Machine,
    r: Box<dyn Replayer>,
    k: u64,
}

fn rig(d: &[u8]) -> Rig {
    let mut m = Machine::new(CpuType::M68000);
    m.bus.clock = T0;
    m.bus.write_log = Some(Vec::new());
    let r = new(d, 0x20000, &mut m).expect("recognised");
    Rig { m, r, k: 0 }
}

impl Rig {
    /// The writes so far (address, value, colour clock); the log starts again.
    fn log(&mut self) -> Vec<(u32, u16, u64)> {
        self.m.bus.write_log.replace(Vec::new()).unwrap()
    }
    fn tick(&mut self) {
        self.m.bus.clock = T0 + TICK * (self.k + 1);
        self.k += 1;
        let mut hw = Hw { bus: &mut self.m.bus };
        self.r.tick(&mut hw);
    }
    fn run(&mut self, n: usize) -> Vec<(u32, u16)> {
        for _ in 0..n {
            self.tick();
        }
        self.log().into_iter().map(|(a, v, _)| (a, v)).collect()
    }
}

/// The register writes of the first tick of a restarted tone voice, then the idle ticks.
#[test]
fn first_tick_and_idle_ticks() {
    // channel 1: frequency 16, pure tone (distortion 5), volume 8; AUDCTL 0 (clock mode B)
    let d = player(&[(0xD200, 16), (0xD201, 0xA8), (0xD203, 0x10), (0xD205, 0x10), (0xD207, 0x10)]);
    let mut r = rig(&d);
    let bank = {
        // the bank is the first allocation; read it from the first location write
        r.tick();
        let l = r.m.bus.write_log.as_ref().unwrap();
        let hi = l.iter().find(|w| w.0 == 0xDFF0A0).unwrap().1 as u32;
        let lo = l.iter().find(|w| w.0 == 0xDFF0A2).unwrap().1 as u32;
        (hi << 16 | lo) - 0x0E
    };
    let first: Vec<(u32, u16)> = r.log().into_iter().map(|(a, v, _)| (a, v)).collect();
    // f = 17 * 256 = 4352: waveform $0E, 4 words, period floor(435 * 140 / 256) = 237, volume 34
    let loc = [(0xDFF0A0u32, (bank + 0xE) >> 16), (0xDFF0A2, (bank + 0xE) & 0xFFFF)].map(|(a, v)| (a, v as u16));
    let mut want: Vec<(u32, u16)> = vec![(0xDFF0A6, 1), (0xDFF09C, 0), (0xDFF096, 0)];
    want.extend(loc);
    want.extend([(0xDFF0A4, 4), (0xDFF0A6, 237), (0xDFF0A8, 34), (0xDFF09C, 0x80), (0xDFF096, 0x8001)]);
    want.extend(loc);
    want.extend([(0xDFF0A4, 4), (0xDFF0A6, 237), (0xDFF0A8, 34)]);
    // the other three channels are volume-only (silence): offset and period equal the start
    // state's except the period, which changes once from 0 to $100
    want.extend([(0xDFF0B6, 0x100), (0xDFF0B8, 0), (0xDFF0C6, 0x100), (0xDFF0C8, 0), (0xDFF0D6, 0x100), (0xDFF0D8, 0)]);
    assert_eq!(first, want);
    // the next ticks write only the four volumes
    let idle = r.run(3);
    let vols = [(0xDFF0A8u32, 34u16), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0)];
    let want: Vec<(u32, u16)> = (0..3).flat_map(|_| vols).collect();
    assert_eq!(idle, want);
}

#[test]
fn write_times_follow_the_specified_gaps() {
    let d = player(&[(0xD200, 16), (0xD201, 0xA8), (0xD203, 0x10), (0xD205, 0x10), (0xD207, 0x10)]);
    let mut r = rig(&d);
    r.tick();
    let l = r.log();
    // PER=1, then INTREQ 26 colour clocks later, then DMACON 6 later
    assert_eq!(l[0].0, 0xDFF0A6);
    assert_eq!(l[1].2 - l[0].2, 26);
    assert_eq!(l[2].2 - l[1].2, 6);
    // PER=1 falls on a raster line, past colour clock 22
    assert!(l[0].2 % 227 >= 22);
    // the idle tick: the volumes are 41 colour clocks apart
    r.tick();
    let l = r.log();
    assert_eq!(l.len(), 4);
    assert_eq!(l[1].2 - l[0].2, 41);
    assert_eq!(l[2].2 - l[1].2, 41);
}

#[test]
fn a_failed_call_writes_nothing() {
    // the play routine is an undocumented opcode
    let d = sap("SAP\nTYPE B\nINIT 2000\nPLAYER 2001\n", &[(0x2000, &[0x60, 0x02])]);
    let mut r = rig(&d);
    assert!(r.run(3).is_empty());
}

fn make(d: &[u8]) -> Pokeynoise {
    let sap = parse(d).unwrap();
    let mut cpu = Cpu::new();
    load(&mut cpu.mem, d, sap.body);
    Pokeynoise { cpu, sap, bank: 0, v: Default::default(), started: false, cur: 0 }
}

#[test]
fn subsong_start_calls() {
    // type B: the init routine stores its accumulator (the subsong) at $40, then returns
    let d = sap("SAP\nTYPE B\nSONGS 5\nDEFSONG 2\nINIT 2000\nPLAYER 2004\n", &[(0x2000, &[0x85, 0x40, 0x60, 0x60, 0x60])]);
    let mut p = make(&d);
    p.start(p.sap.defsong as u16, 0);
    assert!(p.started);
    assert_eq!(p.cpu.mem[0x40], 2);
    p.start(4, 0);
    assert_eq!(p.cpu.mem[0x40], 4);
    assert_eq!(p.subsongs(), (0, 4, 4));
    // type C: the routine at PLAYER + 3 stores A, X and Y at $40..$42 and returns; it is called
    // twice, the second time with A = 0, X = the subsong, Y = 0
    let code = [0x60, 0x60, 0x60, 0x85, 0x40, 0x86, 0x41, 0x84, 0x42, 0x60];
    let d = sap("SAP\nTYPE C\nSONGS 3\nPLAYER 2000\nMUSIC 3456\n", &[(0x2000, &code)]);
    let mut p = make(&d);
    // the routine is reached at PLAYER + 3 = $2003, where the stores begin
    p.start(2, 0);
    assert!(p.started);
    assert_eq!((p.cpu.mem[0x40], p.cpu.mem[0x41], p.cpu.mem[0x42]), (0, 2, 0));
    // a failing init call: the song does not start and ticks write nothing
    let d = sap("SAP\nTYPE B\nINIT 2000\nPLAYER 2001\n", &[(0x2000, &[0x02])]);
    let mut p = make(&d);
    p.start(0, 0);
    assert!(!p.started);
}

#[test]
fn conversion_cases() {
    let d = player(&[]);
    let mut p = Pokeynoise {
        cpu: Cpu::new(),
        sap: parse(&d).unwrap(),
        bank: 0,
        v: Default::default(),
        started: true,
        cur: 0,
    };
    let set = |p: &mut Pokeynoise, audf: u8, audc: u8, ctl: u8| {
        p.cpu.mem[0xD200] = audf;
        p.cpu.mem[0xD201] = audc;
        p.cpu.mem[0xD208] = ctl;
    };
    // volume only gives silence
    set(&mut p, 5, 0x1F, 0);
    assert_eq!(p.convert(0), SILENCE);
    // distortion 0: f = 6 * 256 = 1536, period floor(1536 * 137 / 256) = 821
    set(&mut p, 5, 0x0F, 0);
    assert_eq!(p.convert(0), Tuple { off: 0x277E, len: 10247, per: 822, vol: 15 });
    // distortion 4
    set(&mut p, 5, 0x8F, 0);
    assert_eq!(p.convert(0), Tuple { off: 0x778C, len: 9151, per: 792, vol: 15 });
    // distortion 5 below the high-tone limit: volume muted, waveform kept
    set(&mut p, 0, 0xAF, 0);
    let t = p.convert(0);
    assert_eq!((t.off, t.len, t.vol), (0x0008, 1, 0));
    // distortion 1 and 3 give the same waveform choice
    set(&mut p, 20, 0x2F, 0);
    let a = p.convert(0);
    set(&mut p, 20, 0x6F, 0);
    assert_eq!(a, p.convert(0));
    assert_eq!(a.off, 0x006E);
    // clock mode A (AUDCTL bit 0) with distortion 5: f = 17 * 29184 / 28 = 17718
    set(&mut p, 16, 0xA1, 1);
    let t = p.convert(0);
    assert_eq!((t.off, t.len), (0x000E, 4));
    // joined channel 1 (AUDCTL bit 4) takes the previous channel's AUDF as the low byte
    p.cpu.mem[0xD200] = 2;
    p.cpu.mem[0xD202] = 1;
    p.cpu.mem[0xD203] = 0xA8;
    p.cpu.mem[0xD208] = 0x10;
    let t = p.convert(1);
    // M = 1 * 256 + 2 = 258, mode B: f = 259 * 256 = 66304
    assert_eq!((t.off, t.len), (0x0016, 13));
    assert_eq!(t.per as u64, 2 * 66304 / 65 * 140 / 256);
}

#[test]
fn random_silencing_follows_the_generator() {
    // distortion 6, clock mode B, M = 4: the divider count is 140, 140 mod 15 = 5, so the random
    // silence applies when the channel's state changed
    let d = player(&[]);
    let mk = || Pokeynoise {
        cpu: Cpu::new(),
        sap: parse(&d).unwrap(),
        bank: 0,
        v: Default::default(),
        started: true,
        cur: 0,
    };
    let mut p = mk();
    p.cpu.mem[0xD200] = 4;
    p.cpu.mem[0xD201] = 0xCF;
    let mut g = Rng::new();
    let silent = (g.step() & 0xFF) < 0x50;
    let t = p.convert(0);
    assert_eq!(t == SILENCE, silent);
    // the history now matches: no further step is taken, the generator state is unchanged
    let before = (p.cpu.rng.s1, p.cpu.rng.s2);
    let _ = p.convert(0);
    assert_eq!((p.cpu.rng.s1, p.cpu.rng.s2), before);
}

#[test]
fn malformed_input_does_not_panic() {
    let ok = player(&[(0xD200, 3), (0xD201, 0xA8)]);
    for n in 0..ok.len() {
        let _ = detect(&ok[..n]);
        if let Some(_) = parse(&ok[..n]) {
            let mut m = Machine::new(CpuType::M68000);
            if let Some(mut r) = new(&ok[..n], 0x20000, &mut m) {
                m.bus.clock = T0;
                let mut hw = Hw { bus: &mut m.bus };
                r.tick(&mut hw);
            }
        }
    }
    // a header with only separators, and huge numbers
    let d = sap("SAP\nTYPE B\nINIT ,\nPLAYER ,\nSONGS FFFFFFFFFF\n", &[(0xFFFE, &[1, 2, 3])]);
    let mut m = Machine::new(CpuType::M68000);
    m.bus.clock = T0;
    let mut r = new(&d, 0x20000, &mut m).expect("recognised");
    let mut hw = Hw { bus: &mut m.bus };
    r.tick(&mut hw);
    let _ = r.subsongs();
}
