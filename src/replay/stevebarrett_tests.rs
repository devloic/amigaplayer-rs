use super::super::m68kenc as enc;
use super::*;
use crate::engine::Engine;
use enc::Op::{self, A, Abs, Disp, Imm, Ind, Pc, Post, D};
use std::collections::HashMap;
use std::path::Path;

const BASE: u32 = 0x20000;
/// PERTAB[i] in the synthetic modules (a period table falls).
fn period(i: usize) -> u16 {
    1000 - i as u16
}

fn put_w(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}
fn put_l(d: &mut [u8], o: usize, v: u32) {
    d[o..o + 4].copy_from_slice(&v.to_be_bytes());
}
fn put_code(d: &mut [u8], o: usize, ws: &[u16]) {
    for (i, w) in ws.iter().enumerate() {
        put_w(d, o + 2 * i, *w);
    }
}

/// The flags of the six known builds, in the order Kelly X, MUNGOMUSIC, Artificial Dreams,
/// Advanced Ski Simulator, scape, Street Fighter (a test expectation only: the player takes
/// everything from the code).
fn build_of(c: usize) -> Build {
    let (early, loops, limit, all_voices, portamento, swap, wait) = [
        (true, true, 5, true, false, false, true),
        (true, true, 5, true, false, false, true),
        (false, false, 5, true, true, false, true),
        (false, true, 5, true, true, false, true),
        (false, true, 5, true, false, false, true),
        (false, false, 6, false, true, true, false),
    ][c];
    Build { early, loops, limit, all_voices, portamento, swap, wait }
}

/// The layouts of the six known builds (a test expectation only: the player derives them
/// from the code).
fn layout_of(c: usize) -> Lay {
    macro_rules! col {
        ($($f:ident: [$a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $g:expr]),* $(,)?) => {
            Lay { $($f: [$a, $b, $c, $d, $e, $g][c]),* }
        };
    }
    col! {
        flag: [1686, 1654, 2274, 2300, 2042, 2214],
        pertab: [1688, 1656, 2276, 2302, 2044, 2216],
        perout: [1834, 1802, 2416, 2442, 2224, 2356],
        vrec: [1850, 1818, 2432, 2458, 2240, 2364],
        cnt: [1978, 1946, 2560, 2586, 2368, 2492],
        started: [1979, 1947, 2561, 2587, 2369, 2493],
        speed: [1980, 1948, 2562, 2588, 2370, 2494],
        spdcnt: [1981, 1949, 2563, 2589, 2371, 2495],
        vibdepth: [1982, 1950, 2564, 2590, 2372, 2496],
        vibshift: [1983, 1951, 2565, 2591, 2373, 2497],
        arpparam: [1984, 1952, 2566, 2592, 2374, 2498],
        rowbytes: [1985, 1953, 2567, 2593, 2375, 2499],
        regoff: [1986, 1954, 2568, 2594, 2376, 2500],
        p_steps: [0, 0, 2570, 2596, 0, 2502],
        p_lo: [0, 0, 2571, 2597, 0, 2503],
        p_lo2: [0, 0, 2572, 2598, 0, 2504],
        p_dir: [0, 0, 2573, 2600, 0, 2505],
        p_tgt: [0, 0, 2574, 2601, 0, 2506],
        dmamask: [1994, 1962, 2575, 2602, 2384, 2507],
        arpint: [0, 0, 2576, 2603, 2385, 2508],
        baseper: [1996, 1964, 2584, 2612, 2394, 2516],
        p_delta: [0, 0, 2592, 2620, 0, 2524],
        smpstart: [2014, 1982, 2602, 2630, 2412, 2534],
        replen: [2030, 1998, 2618, 2646, 2428, 2550],
        repoff: [2038, 2006, 2626, 2654, 2436, 2558],
        patptr: [2046, 2014, 2634, 2662, 2444, 2566],
        seqptr: [2062, 2030, 2650, 2678, 2460, 2582],
        curseq: [2078, 2046, 2666, 2694, 2476, 2598],
        curpat: [2082, 2050, 2670, 2698, 2480, 2602],
        vibdelta: [2086, 2054, 2674, 2702, 2484, 2606],
        vibacc: [2090, 2058, 2678, 2706, 2488, 2610],
        arpbuf: [0, 0, 2682, 2710, 2492, 2614],
        smpaddr: [2094, 2062, 2722, 2750, 2502, 2654],
        instr: [2136, 2110, 2812, 2834, 2568, 2744],
        env: [2192, 2174, 2940, 2946, 2656, 2824],
        songs: [2312, 2326, 3076, 3188, 2832, 2968],
        pattab: [2320, 2350, 3092, 3220, 2840, 3008],
        samples: [3596, 4058, 4084, 5394, 3728, 4010],
    }
}

/// What a synthetic module is made of: the layout and flags the code must reveal, and the
/// shape of the code (padding in front of the routines, and their order in the file).
#[derive(Clone)]
struct Spec {
    lay: Lay,
    b: Build,
    /// Words of no-operation padding in front of the first routine.
    pad: usize,
    /// The called routines come before the song init, play and stop routines.
    reversed: bool,
}

fn spec(c: usize) -> Spec {
    Spec { lay: layout_of(c), b: build_of(c), pad: 0, reversed: false }
}

/// A small assembler: instructions placed at an address, labels resolved by running the
/// generator twice.
struct Asm<'a> {
    at: usize,
    w: Vec<u16>,
    lab: &'a HashMap<&'static str, usize>,
    def: HashMap<&'static str, usize>,
}

impl Asm<'_> {
    fn e(&mut self, f: impl FnOnce(usize) -> Vec<u16>) {
        let ws = f(self.at);
        self.at += 2 * ws.len();
        self.w.extend(ws);
    }
    fn label(&mut self, n: &'static str) {
        self.def.insert(n, self.at);
    }
    fn tgt(&self, n: &str) -> usize {
        self.lab.get(n).copied().unwrap_or(self.at)
    }
    fn bcc(&mut self, cond: u16, n: &str) {
        let t = self.tgt(n);
        self.e(|at| enc::bcc_w(cond, (t as i64 - (at as i64 + 2)) as i16));
    }
    fn bsr(&mut self, n: &str) {
        self.bcc(enc::BSR, n);
    }
    fn bra(&mut self, n: &str) {
        self.bcc(enc::BRA, n);
    }
    fn dbra(&mut self, dn: u16, n: &str) {
        let t = self.tgt(n);
        self.e(|at| enc::dbra(dn, at, t));
    }
    fn rts(&mut self) {
        self.e(|_| enc::rts());
    }
    fn mv(&mut self, size: u16, src: Op, dst: Op) {
        self.e(|at| enc::mv(size, src, dst, at));
    }
    fn lea(&mut self, src: Op, an: u16) {
        self.e(|at| enc::lea(src, an, at));
    }
    fn moveq(&mut self, n: u16, v: u8) {
        self.e(|_| enc::moveq(n, v));
    }
}

/// The routines of a module of `s`, the labels they define and their words.
fn routines(s: &Spec, lab: &HashMap<&'static str, usize>) -> (Vec<u16>, HashMap<&'static str, usize>) {
    let (l, b) = (&s.lay, &s.b);
    let full = !b.early;
    let pc = |p: usize| Pc(p as i64);
    // a data location relative to the pointer-base register (a4, the pointer base is -6)
    let v = |p: usize| Disp(4, (p as i64 + 6) as i16);
    let (bb, ww, ll) = (enc::B, enc::W, enc::L);
    let mut a = Asm { at: 0x20, w: Vec::new(), lab, def: HashMap::new() };
    for _ in 0..s.pad {
        a.e(|_| enc::nop());
    }
    let sample_init = |a: &mut Asm| {
        // sample init
        a.label("si");
        a.lea(pc(l.smpaddr), 1);
        a.lea(Pc(-6), 4);
        a.lea(pc(l.samples), 0);
        a.lea(pc(l.instr + if b.loops { 2 } else { 0 }), 2);
        a.label("si_loop");
        a.e(|at| enc::imm_op(6, ll, FORM_WORD, Post(0), at));
        a.bcc(enc::BNE, "si_end");
        a.e(|at| enc::imm_op(6, ll, BODY_WORD, Post(0), at));
        a.bcc(enc::BNE, "si_end");
        a.mv(ll, Post(0), D(0));
        a.mv(ll, D(0), D(1));
        a.e(|_| enc::lsr_w_imm(1, 1));
        if b.loops {
            a.e(|at| enc::clr(ww, Ind(0), at));
        }
        a.mv(ww, D(1), Post(1));
        a.mv(ll, A(0), Post(1));
        a.e(|at| enc::adda(ll, D(0), 0, at));
        a.e(|at| enc::quick(true, 8, ll, A(2), at));
        a.e(|at| enc::imm_op(6, ll, FORM_WORD, Ind(0), at));
        a.bcc(enc::BEQ, "si_loop");
        a.label("si_end");
        a.rts();
    };
    let entry_part = |a: &mut Asm| {
        // song init
        a.label("sg");
        a.lea(Pc(-6), 4);
        a.mv(bb, D(0), v(l.flag));
        a.rts();
        // play
        a.label("pl");
        a.lea(Pc(-6), 4);
        a.mv(bb, pc(l.flag), D(0));
        a.bcc(enc::BNE, "pl_go");
        a.mv(bb, D(0), v(l.started));
        a.bsr("e3");
        a.bra("pl_end");
        a.label("pl_go");
        a.bsr("stopclr");
        a.moveq(1, b.limit);
        a.e(|at| enc::alu(0xB, bb, D(0), 1, at));
        a.bcc(enc::BCS, "pl_end");
        a.lea(pc(l.songs), 0);
        a.lea(pc(l.seqptr), 1);
        for _ in 0..4 {
            a.mv(ll, A(4), A(5));
            a.e(|at| enc::adda(ww, Post(0), 5, at));
            a.mv(ll, A(5), Post(1));
        }
        a.lea(pc(l.vrec), 0);
        a.mv(bb, Ind(0), v(l.regoff));
        a.e(|at| enc::clr(bb, v(l.rowbytes), at));
        a.e(|at| enc::quick(true, 1, bb, v(l.rowbytes), at));
        a.mv(bb, Imm(15), v(l.dmamask));
        a.mv(bb, Imm(3), v(l.cnt));
        a.mv(bb, Imm(1), v(l.started));
        a.e(|at| enc::bit_dyn(true, 1, v(l.dmamask), at));
        a.e(|at| enc::bit_dyn(false, 1, v(l.dmamask), at));
        a.e(|at| enc::clr(bb, v(l.flag), at));
        a.e(|at| enc::imm_op(1, bb, 15, v(l.vibshift), at));
        a.mv(bb, D(0), v(l.vibdepth));
        a.mv(bb, pc(l.vibdepth), D(0));
        a.mv(ww, D(0), v(l.vibdelta));
        a.e(|at| enc::alu(0xD, ww, pc(l.vibdelta), 0, at));
        a.e(|at| enc::alu(0x9, ww, pc(l.vibdelta), 0, at));
        a.mv(ww, Ind(1), v(l.vibacc));
        a.mv(bb, pc(l.speed), v(l.spdcnt));
        a.mv(bb, Disp(0, 100), D(5));
        a.mv(ww, D(5), Abs(DMACON_ADDR));
        if full {
            a.e(|at| enc::imm_op(6, bb, 125, D(0), at));
        }
        a.bsr("seq");
        a.bsr("instr");
        a.bsr("env");
        a.bsr("setper");
        a.lea(pc(l.patptr), 1);
        a.lea(pc(l.pertab), 1);
        a.lea(pc(l.baseper), 1);
        a.lea(pc(l.perout), 1);
        a.lea(pc(l.env), 2);
        if full {
            a.lea(pc(l.arpbuf), 1);
            a.mv(bb, D(0), v(l.arpparam));
        }
        if b.portamento {
            a.bsr("port");
            a.mv(bb, D(0), v(l.p_tgt));
            a.mv(bb, D(0), v(l.p_lo2));
            a.mv(bb, D(0), v(l.p_lo));
            a.e(|at| enc::alu(0xD, bb, pc(l.p_lo2), 0, at));
            a.lea(pc(l.p_delta), 2);
        }
        a.bsr("out");
        // the repeat stage
        if b.wait {
            a.moveq(1, 3);
            a.moveq(2, 12);
            a.moveq(3, 6);
        }
        if b.all_voices {
            a.label("vloop");
            a.lea(pc(l.replen), 6);
            a.e(|at| enc::adda(ll, D(3), 6, at));
            a.mv(ww, Ind(6), D(0));
            a.e(|_| enc::lsr_w_imm(1, 0));
            a.mv(ww, D(0), Ind(5));
            a.lea(pc(l.repoff), 6);
            a.e(|at| enc::adda(ll, D(3), 6, at));
            a.mv(ww, Ind(6), D(6));
            a.e(|at| enc::imm_op(1, ll, 4095, D(6), at));
            a.lea(pc(l.smpstart), 6);
            a.e(|at| enc::adda(ll, D(2), 6, at));
            a.e(|at| enc::alu(0xD, ll, Ind(6), 6, at));
            a.mv(ll, D(6), Ind(5));
            a.dbra(1, "vloop");
        } else {
            a.lea(pc(l.replen), 6);
            a.mv(ww, Ind(6), D(0));
            a.bcc(enc::BEQ, "skip");
            a.mv(ww, D(0), Ind(5));
            a.mv(bb, pc(l.regoff), D(2));
            a.lea(pc(l.repoff), 6);
            a.mv(ww, Ind(6), D(6));
            a.e(|at| enc::imm_op(1, ll, 4095, D(6), at));
            a.lea(pc(l.smpstart), 6);
            a.e(|at| enc::alu(0xD, ll, Ind(6), 6, at));
            a.mv(ll, D(6), Ind(5));
            a.label("skip");
        }
        a.e(|at| enc::quick(false, 1, bb, v(l.spdcnt), at));
        a.e(|at| enc::quick(false, 1, bb, v(l.cnt), at));
        a.e(|at| enc::scc(0, v(l.flag), at));
        a.label("pl_end");
        a.rts();
        // stop
        a.label("e3");
        a.moveq(0, 0);
        a.mv(ll, Imm(AUD0VOL), A(5));
        for dd in [0, 16, 32, 48] {
            a.mv(ww, D(0), if dd == 0 { Ind(5) } else { Disp(5, dd) });
        }
        a.lea(pc(l.vrec + 96), 5);
        a.mv(bb, D(0), Ind(5));
        a.mv(bb, D(0), Disp(5, 1));
        a.rts();
    };
    let called_part = |a: &mut Asm| {
        // stop and clear
        a.label("stopclr");
        a.bsr("e3");
        a.e(|at| enc::clr(bb, v(l.spdcnt), at));
        a.e(|at| enc::clr(bb, v(l.started), at));
        a.rts();
        // set period
        a.label("setper");
        a.lea(pc(l.pertab), 1);
        a.lea(pc(l.baseper), 2);
        a.lea(pc(l.perout), 3);
        a.rts();
        // sequence advance
        a.label("seq");
        a.lea(pc(l.seqptr), 1);
        a.lea(pc(l.pattab), 2);
        a.lea(pc(l.patptr), 3);
        a.mv(ll, Ind(1), v(l.curseq));
        a.mv(bb, Disp(0, 12), D(0));
        a.e(|at| enc::quick(true, 1, bb, Disp(0, 12), at));
        a.e(|at| enc::imm_op(3, bb, 64, D(0), at));
        a.mv(ll, D(0), v(l.curpat));
        a.rts();
        // instrument: the word copies come from displacements 4 then 2
        a.label("instr");
        a.lea(pc(l.instr), 1);
        a.mv(bb, Ind(1), D(4));
        a.mv(bb, Disp(1, 1), Disp(0, 116));
        a.mv(bb, Disp(1, 6), Disp(0, 120));
        a.mv(bb, Disp(1, 7), Disp(0, 124));
        a.lea(pc(if b.swap { l.repoff } else { l.replen }), 6);
        a.mv(ww, Disp(1, 4), Ind(6));
        a.lea(pc(if b.swap { l.replen } else { l.repoff }), 6);
        a.mv(ww, Disp(1, 2), Ind(6));
        a.e(|_| enc::mulu_imm_d(6, 4));
        a.lea(pc(l.smpaddr), 1);
        a.e(|at| enc::adda(ll, D(4), 1, at));
        a.lea(pc(l.smpstart), 5);
        a.mv(ll, Ind(1), Ind(5));
        a.rts();
        // envelope
        a.label("env");
        a.lea(pc(l.env), 2);
        a.mv(bb, Disp(2, 6), v(l.arpparam));
        a.bsr("arp");
        a.rts();
        // arpeggio builder (a stub in the early family)
        a.label("arp");
        if full {
            a.mv(bb, pc(l.arpparam), D(0));
            a.lea(pc(l.arpint), 1);
            a.mv(bb, Ind(1), D(0));
            a.lea(pc(l.arpbuf), 1);
            a.mv(bb, D(0), Ind(1));
        }
        a.rts();
        // output
        a.label("out");
        a.lea(pc(l.perout), 0);
        a.mv(ll, Imm(AUD0PER_ADDR), A(2));
        a.mv(ww, Post(0), Ind(2));
        a.mv(bb, pc(l.dmamask), D(1));
        a.e(|_| enc::not_b(1));
        a.e(|at| enc::imm_op(5, ww, 65535, D(1), at));
        a.mv(ww, D(1), Abs(DMACON_ADDR));
        a.lea(pc(l.vrec + 96), 1);
        a.mv(bb, Post(1), D(0));
        a.rts();
        // portamento
        if b.portamento {
            a.label("port");
            a.mv(bb, pc(l.p_tgt), D(4));
            a.mv(bb, Imm(1), v(l.p_dir));
            a.mv(ww, D(5), v(l.p_delta));
            a.e(|at| enc::clr(bb, v(l.p_dir), at));
            a.mv(bb, pc(l.speed), D(4));
            a.e(|at| enc::alu(0xD, bb, pc(l.p_lo), 0, at));
            a.mv(bb, D(0), v(l.p_steps));
            a.e(|_| enc::rol_w_imm(1, 6));
            a.rts();
        }
    };
    // the sample init stays first (the detection takes the first `lea` of a0)
    sample_init(&mut a);
    if s.reversed {
        called_part(&mut a);
        entry_part(&mut a);
    } else {
        entry_part(&mut a);
        called_part(&mut a);
    }
    (a.w, a.def)
}

const FORM_WORD: u32 = u32::from_be_bytes(*b"FORM");
const BODY_WORD: u32 = u32::from_be_bytes(*b"BODY");
const AUD0PER_ADDR: u32 = 0xDFF000 + 166;
const DMACON_ADDR: u32 = 0xDFF000 + 150;

/// A synthetic module: four branches to the routines, their code, the variables at their
/// initial values, one song whose four voices play the same sequence (pattern 0, then wrap),
/// one pattern given by the caller, one 16-byte sample with a loop (oneshot 4, repeat 8),
/// instrument 0 and envelope 0. Returns the module and the labels of its code.
fn module_of(s: &Spec, pattern: &[u8]) -> (Vec<u8>, HashMap<&'static str, usize>) {
    let (l, b) = (&s.lay, &s.b);
    let none = HashMap::new();
    let (_, first) = routines(s, &none);
    let (ws, lab) = routines(s, &first);
    let mut d = vec![0u8; l.samples + 104 + 16 + 4];
    let ent = ["si", "sg", "pl", "e3"];
    for (i, n) in ent.iter().enumerate() {
        put_code(&mut d, 4 * i, &enc::bcc_w(enc::BRA, (lab[n] as i64 - (4 * i as i64 + 2)) as i16));
    }
    put_code(&mut d, 0x20, &ws);
    d[l.flag] = 1;
    d[l.speed] = 1;
    d[l.dmamask] = 15;
    for v in 0..4 {
        d[l.vrec + F_REGOFF + v] = 16 * v as u8;
        d[l.vrec + F_DMABIT + v] = 1 << v;
        if !b.early {
            d[l.vrec + F_ARPBASE + v] = 10 * v as u8;
        }
    }
    if !b.early {
        d[l.arpint..l.arpint + 8].copy_from_slice(&[3, 5, 6, 7, 8, 9, 4, 12]);
    }
    for i in 0..73 {
        put_w(&mut d, l.pertab + 2 * i, period(i));
    }
    // one sequence (pattern 0, wrap) and the pattern, after the pattern table
    let seq = l.pattab + 16;
    let pat = l.pattab + 32;
    d[seq] = 0;
    d[seq + 1] = 255;
    d[pat..pat + pattern.len()].copy_from_slice(pattern);
    put_w(&mut d, l.pattab, (pat + 6) as u16);
    for v in 0..4 {
        put_w(&mut d, l.songs + 2 * v, (seq + 6) as u16);
    }
    d[l.env] = 63;
    // the sample record: oneshot at 20, repeat at 24, BODY at 96 with its size
    let r = l.samples;
    d[r..r + 4].copy_from_slice(b"FORM");
    put_l(&mut d, r + 20, 4);
    put_l(&mut d, r + 24, 8);
    d[r + 96..r + 100].copy_from_slice(b"BODY");
    put_l(&mut d, r + 100, 16);
    (d, lab)
}

/// A synthetic module of the known build `c` (see `module_of`).
fn module(c: usize, pattern: &[u8]) -> Vec<u8> {
    module_of(&spec(c), pattern).0
}

/// Plays `ticks` ticks of subsong 0 in a bare machine and returns the player and the
/// register writes (address, value) it made, the load-time ones first.
fn run(d: &[u8], ticks: usize) -> (Sb, Vec<(u32, u16)>) {
    let mut e = Engine::new(48000.0);
    e.m.bus.write_log = Some(Vec::new());
    let (mut s, clear) = init(d, BASE).unwrap();
    for a in clear {
        e.m.bus.write_bytes(a, &[0, 0]);
    }
    if s.b.loops {
        s.stop(&mut Hw { bus: &mut e.m.bus }, false);
    }
    for _ in 0..ticks {
        s.tick(&mut Hw { bus: &mut e.m.bus });
    }
    let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
    (s, log)
}

fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
    log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
}

const PER: u32 = 0xDFF0A6;
const VOL: u32 = 0xDFF0A8;
const LEN: u32 = 0xDFF0A4;
const LCL: u32 = 0xDFF0A2;
const DMA: u32 = 0xDFF096;

/// A pattern: duration 4, note 20, stop.
const ONE_NOTE: [u8; 3] = [0x84, 20, 0xFE];

/// Replaces the one occurrence of the instruction words `old` by `new`.
fn patch(d: &mut [u8], old: &[u16], new: &[u16]) {
    assert_eq!(old.len(), new.len());
    let ob: Vec<u8> = old.iter().flat_map(|w| w.to_be_bytes()).collect();
    let nb: Vec<u8> = new.iter().flat_map(|w| w.to_be_bytes()).collect();
    let hits: Vec<usize> = (0..d.len() - ob.len()).step_by(2).filter(|&i| d[i..i + ob.len()] == ob[..]).collect();
    assert_eq!(hits.len(), 1, "occurrences of the patched instruction");
    d[hits[0]..hits[0] + nb.len()].copy_from_slice(&nb);
}

fn spec_with(c: usize, f: impl FnOnce(&mut Spec)) -> Vec<u8> {
    let mut s = spec(c);
    f(&mut s);
    module_of(&s, &ONE_NOTE).0
}

#[test]
fn recognises_every_build_by_structure() {
    for c in 0..6 {
        let d = module(c, &ONE_NOTE);
        assert!(detect(&d), "build {c}");
        let f = find(&d).unwrap();
        assert_eq!(f.pbase, -6);
        assert_eq!(f.lay, layout_of(c), "build {c}");
        assert_eq!(f.build, build_of(c), "build {c}");
        assert_eq!(f.subs, (layout_of(c).pattab - layout_of(c).songs) / 8);
    }
}

/// The layout moved by `by` bytes (every offset that exists).
fn moved(l: &Lay, by: usize) -> Lay {
    let m = |x: usize| if x == 0 { 0 } else { x + by };
    Lay {
        flag: m(l.flag), pertab: m(l.pertab), perout: m(l.perout), vrec: m(l.vrec), cnt: m(l.cnt), started: m(l.started),
        speed: m(l.speed), spdcnt: m(l.spdcnt), vibdepth: m(l.vibdepth), vibshift: m(l.vibshift), arpparam: m(l.arpparam),
        rowbytes: m(l.rowbytes), regoff: m(l.regoff), p_steps: m(l.p_steps), p_lo: m(l.p_lo), p_lo2: m(l.p_lo2),
        p_dir: m(l.p_dir), p_tgt: m(l.p_tgt), dmamask: m(l.dmamask), arpint: m(l.arpint), baseper: m(l.baseper),
        p_delta: m(l.p_delta), smpstart: m(l.smpstart), replen: m(l.replen), repoff: m(l.repoff), patptr: m(l.patptr),
        seqptr: m(l.seqptr), curseq: m(l.curseq), curpat: m(l.curpat), vibdelta: m(l.vibdelta), vibacc: m(l.vibacc),
        arpbuf: m(l.arpbuf), smpaddr: m(l.smpaddr), instr: m(l.instr), env: m(l.env), songs: m(l.songs),
        pattab: m(l.pattab), samples: m(l.samples),
    }
}

#[test]
fn the_layout_follows_the_code_not_the_entries() {
    for c in 0..6 {
        let want = layout_of(c);
        // the code moved and its routines reordered: the same layout
        for (pad, reversed) in [(7, false), (0, true), (30, true)] {
            let d = spec_with(c, |s| {
                s.pad = pad;
                s.reversed = reversed;
            });
            assert_eq!(find(&d).map(|f| f.lay), Some(want), "build {c} pad {pad} reversed {reversed}");
        }
        // the variables and tables moved: the layout moves with them
        let mv = moved(&want, 40);
        let d = spec_with(c, |s| s.lay = mv);
        assert!(find(&d).is_some(), "build {c} moved");
        assert_eq!(find(&d).unwrap().lay, mv, "build {c} moved");
        // variables exchanged: the order of the variables differs from every known build
        let mut x = want;
        std::mem::swap(&mut x.curseq, &mut x.curpat);
        std::mem::swap(&mut x.vibdelta, &mut x.vibacc);
        std::mem::swap(&mut x.replen, &mut x.repoff);
        let d = spec_with(c, |s| s.lay = x);
        assert_eq!(find(&d).unwrap().lay, x, "build {c} exchanged");
    }
}

#[test]
fn flags_follow_the_code() {
    // every combination of the build differences the generator can write
    for (loops, all_voices, swap, wait, portamento, limit) in [
        (true, true, true, false, true, 3),
        (false, false, false, false, false, 6),
        (false, true, true, true, true, 4),
        (true, false, false, true, true, 5),
    ] {
        let mut s = spec(2);
        s.b = Build { early: false, loops, limit, all_voices, portamento, swap, wait };
        let f = find(&module_of(&s, &ONE_NOTE).0).unwrap();
        assert_eq!(f.build, s.b);
        let want = Lay {
            p_steps: if portamento { s.lay.p_steps } else { 0 },
            p_lo: if portamento { s.lay.p_lo } else { 0 },
            p_lo2: if portamento { s.lay.p_lo2 } else { 0 },
            p_dir: if portamento { s.lay.p_dir } else { 0 },
            p_tgt: if portamento { s.lay.p_tgt } else { 0 },
            p_delta: if portamento { s.lay.p_delta } else { 0 },
            ..s.lay
        };
        assert_eq!(f.lay, want);
    }
}

#[test]
fn refuses_what_does_not_fit() {
    let (d, lab) = module_of(&spec(0), &ONE_NOTE);
    let l = layout_of(0);
    assert!(!detect(&[]));
    assert!(!detect(&d[..100]));
    // an unknown set of entries (a branch retargeted into data)
    let mut e = d.clone();
    put_code(&mut e, 0, &enc::bcc_w(enc::BRA, 200));
    assert!(!detect(&e));
    // a short branch instead of a word branch
    let mut e = d.clone();
    put_code(&mut e, 0, &enc::bra_s(0x40));
    assert!(!detect(&e));
    // the stop routine does not load the AUD0VOL address
    let mut e = d.clone();
    put_code(&mut e, lab["e3"] + 2, &enc::movea_l_imm(5, 0x1234));
    assert!(!detect(&e));
    // the first lea does not reach `FORM`
    let mut e = d.clone();
    e[l.samples] = b'X';
    assert!(!detect(&e));
    // the instruction before it is not a pc-relative lea
    let mut e = d.clone();
    put_code(&mut e, lab["si"] + 4, &[enc::nop(), enc::nop()].concat());
    assert!(!detect(&e));
    // a variable that does not hold its initial value
    for o in [l.flag, l.dmamask, l.speed, l.started, l.vrec + F_DMABIT + 1] {
        let mut e = d.clone();
        e[o] ^= 0x40;
        assert!(!detect(&e), "offset {o}");
    }
    // the arpeggio interval table of a full build
    let mut e = module(2, &ONE_NOTE);
    e[layout_of(2).arpint] = 9;
    assert!(!detect(&e));
    // a period table that does not fall
    let mut e = d.clone();
    put_w(&mut e, l.pertab + 2 * 10, 0);
    assert!(!detect(&e));
    // a subsong table that is not a whole number of records
    assert!(!detect(&spec_with(0, |s| s.lay.songs += 4)));
    // a sample without a body
    let mut e = d.clone();
    e[l.samples + 96] = 0;
    assert!(!detect(&e));
}

#[test]
fn refuses_code_that_breaks_a_rule() {
    let (bb, ww, ll) = (enc::B, enc::W, enc::L);
    let base = module(2, &ONE_NOTE);
    assert!(detect(&base));
    let l = layout_of(2);
    let v = |p: usize| Disp(4, (p as i64 + 6) as i16);
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut add = |name: &'static str, old: Vec<u16>, new: Vec<u16>| {
        let mut e = base.clone();
        patch(&mut e, &old, &new);
        cases.push((name, e));
    };
    // no routine multiplies by 6
    add("instrument multiply", enc::mulu_imm_d(6, 4), enc::mulu_imm_d(7, 4));
    // a word copy of the instrument from a displacement that is neither 2 nor 4
    add("word copy", enc::mv(ww, Disp(1, 4), Ind(6), 0), enc::mv(ww, Disp(1, 6), Ind(6), 0));
    // the repeat offset mask is not 4095
    add("repeat mask", enc::imm_op(1, ll, 4095, D(6), 0), enc::imm_op(1, ll, 65535, D(6), 0));
    // no byte store of 15
    add("dma mask store", enc::mv(bb, Imm(15), v(l.dmamask), 0), enc::mv(bb, Imm(14), v(l.dmamask), 0));
    // the subsong limit has no compare
    add("limit compare", enc::alu(0xB, bb, D(0), 1, 0), enc::nop());
    // an indirect jump in the play routine
    add("indirect jump", enc::scc(0, v(l.flag), 0), [enc::jmp_ind(0), enc::nop()].concat());
    // the sequence advance writes the pointer-base register (and loses a long store)
    add("pointer base written", enc::mv(ll, D(0), v(l.curpat), 0), [enc::adda(ll, D(0), 4, 0), enc::nop()].concat());
    // a voice-record displacement that is not a multiple of 4
    add("voice displacement", enc::mv(bb, Disp(1, 7), Disp(0, 124), 0), enc::mv(bb, Disp(1, 7), Disp(0, 126), 0));
    // the DMA bit field is not read before the DMACON write
    add("dma bit field", enc::mv(bb, Disp(0, 100), D(5), 0), enc::mv(bb, Disp(0, 104), D(5), 0));
    // the stop routine does not clear the volumes in order
    add("stop volumes", enc::mv(ww, D(0), Disp(5, 32), 0), enc::mv(ww, D(0), Disp(5, 36), 0));
    for (name, e) in &cases {
        assert!(!detect(e), "{name} should be refused");
    }
    // the portamento routine in an early build
    assert!(!detect(&spec_with(0, |s| s.b.portamento = true)));
}

#[test]
fn counts_subsongs_from_the_tables() {
    let f = find(&module(2, &ONE_NOTE)).unwrap();
    assert_eq!(f.subs, (layout_of(2).pattab - layout_of(2).songs) / 8);
    let (s, _) = init(&module(5, &ONE_NOTE), BASE).unwrap();
    assert_eq!(s.subsongs(), (0, 4, 0));
}

#[test]
fn sample_init_of_loops_builds() {
    let d = module(0, &ONE_NOTE);
    let l = layout_of(0);
    let (s, clear) = init(&d, BASE).unwrap();
    // the record's first word and the data's first word are cleared, the loop goes into instrument 0
    assert_eq!(s.rw(l.samples), 0);
    assert_eq!((s.rw(l.instr + 2), s.rw(l.instr + 4)), (4, 8));
    let data = l.samples + 104;
    assert_eq!(clear, vec![BASE + data as u32]);
    assert_eq!((s.rw(l.smpaddr), s.rl(l.smpaddr + 2)), (8, BASE + data as u32));
    // the load-time stop
    let (_, log) = run(&d, 0);
    assert_eq!(log, (0..4).map(|v| (VOL + 16 * v, 0)).collect::<Vec<_>>());
    // a plain build leaves the record alone and writes nothing
    let (p, log) = run(&module(2, &ONE_NOTE), 0);
    assert_eq!(p.rb(layout_of(2).samples), b'F');
    assert!(log.is_empty());
}

#[test]
fn first_tick_is_the_song_init_second_starts_the_notes() {
    let d = module(0, &ONE_NOTE);
    let l = layout_of(0);
    let (s, log) = run(&d, 1);
    // the stop routine only
    assert_eq!(log.len(), 8);
    assert_eq!((s.rb(l.started), s.rb(l.flag), s.rb(l.dmamask)), (1, 255, 15));
    let (s, log) = run(&d, 2);
    // voices 3, 2, 1, 0: DMA off, length, address; then the output stage
    let w = &log[8..];
    assert_eq!(w[0], (DMA, 8));
    assert_eq!(w[1], (0xDFF0D4, 8));
    assert_eq!((w[2].0, w[3].0), (0xDFF0D0, 0xDFF0D2));
    assert_eq!(reg(&log, PER), vec![period(20)]);
    for v in 0..4u32 {
        assert_eq!(reg(&log, PER + 16 * v), vec![period(20)]);
        assert_eq!(reg(&log, VOL + 16 * v).last(), Some(&63));
    }
    // the DMA-on write, then its masked pair
    let dm = reg(&log, DMA);
    assert_eq!(&dm[dm.len() - 2..], &[0x820F, 0x0000]);
    // the repeat stage: length halved, address plus the repeat offset
    assert_eq!(reg(&log, LEN).last(), Some(&4));
    let smp = (BASE + (l.samples + 104) as u32) & 0xFFFF;
    assert_eq!(reg(&log, LCL).last(), Some(&(smp as u16 + 4)));
    assert_eq!(s.g(F_NOTE, 3), 20);
    assert_eq!(s.g(F_DURCNT, 0), 4);
}

#[test]
fn song_ends_on_the_second_stop() {
    let d = module(1, &ONE_NOTE);
    let (s, log) = run(&d, 30);
    assert!(s.songend());
    // the end writes: DMACON 15, then the four volumes 0
    let n = log.len();
    assert_eq!(log[n - 5], (DMA, 15));
    assert!(log[n - 4..].iter().all(|w| w.1 == 0));
    // the player stays silent afterwards
    let (_, longer) = run(&d, 60);
    assert_eq!(longer.len(), n);
}

#[test]
fn song_ends_when_every_voice_has_wrapped() {
    // the pattern is a note; the sequence wraps every pass
    let d = module(0, &[0x82, 10, 0xFF]);
    let (s, _) = run(&d, 40);
    assert!(s.songend());
}

#[test]
fn byte_classes_of_a_row() {
    // speed 3, instrument 5, envelope 2, duration 5, note 7
    let d = module(0, &[0xF3, 0xE5, 0xC2, 0x85, 7, 0xFE]);
    let (s, log) = run(&d, 2);
    let l = layout_of(0);
    assert_eq!(s.rb(l.speed), 3);
    assert_eq!((s.g(F_INSTR, 1), s.g(F_ENV, 1), s.g(F_DUR, 1), s.g(F_NOTE, 1)), (40, 16, 5, 7));
    assert_eq!(reg(&log, PER + 16), vec![period(7)]);
    // the pattern pointer moved past the five bytes of the row
    assert_eq!(s.rl(l.patptr + 4), s.pabs(s.rw(l.pattab) as u32) + 5);
}

#[test]
fn end_of_pattern_byte_advances_the_sequence() {
    let d = module(0, &[0xFF]);
    let (s, _) = run(&d, 3);
    // the sequence wrapped to the pattern again, so the voices keep reading it
    assert_eq!(s.rb(layout_of(0).flag), 255);
}

#[test]
fn transpose_bytes_of_a_sequence() {
    let mut d = module(0, &ONE_NOTE);
    let l = layout_of(0);
    // sequence: transpose +4 (196), pattern 0
    d[l.pattab + 16..l.pattab + 19].copy_from_slice(&[196, 0, 255]);
    let (s, log) = run(&d, 2);
    assert_eq!(s.g(F_TRANSPOSE, 0), 4);
    assert_eq!(reg(&log, PER), vec![period(24)]);
}

#[test]
fn slide_down_effect_of_the_full_family() {
    // duration 4, slide down, note 20
    let d = module(2, &[0x84, 127, 20, 0xFE]);
    let (s, _) = run(&d, 4);
    assert_eq!((s.g(F_SLIDEDOWN, 0), s.g(F_MODE, 0)), (1, 1));
    // the tick after the row has SPDCNT 1; the next one slides
    assert_eq!(s.g(F_NOTE, 0), 19);
    assert_eq!(s.rw(layout_of(2).perout), period(19));
    // the early family reads 127 as the note itself
    let e = module(0, &[0x84, 127, 20, 0xFE]);
    let (s, _) = run(&e, 2);
    assert_eq!(s.g(F_NOTE, 0), 127);
}

#[test]
fn arpeggio_effect_steps_through_the_intervals() {
    // arpeggio parameter 0b101: intervals 3 and 6
    let d = module(3, &[0x88, 125, 0b101, 20, 0xFE]);
    let l = layout_of(3);
    let (s, _) = run(&d, 3);
    assert_eq!(s.g(F_ARP, 0), 5);
    assert_eq!(&s.m[l.arpbuf + 1..l.arpbuf + 4], &[3, 6, 255]);
    // the tick after the row steps the arpeggio: the first interval above the note
    assert_eq!(s.rw(l.perout), period(23));
}

#[test]
fn portamento_moves_the_period_toward_the_target() {
    // duration 8, portamento to note 24 with parameter 0x13, note 20
    let d = module(2, &[0x88, 123, 24, 0x13, 20, 0xFE]);
    let l = layout_of(2);
    let (s, _) = run(&d, 3);
    assert_eq!((s.g(F_PTARGET, 0), s.g(F_PPARAM, 0)), (24, 0x13));
    // the target is a higher note: the period moves by the difference rotated right by 3
    assert_eq!(s.rb(l.p_tgt), 24);
    assert_eq!(s.rb(l.p_lo), 3);
}

#[test]
fn vibrato_follows_the_envelope() {
    // envelope 0: vibrato parameter 0x22
    let mut d = module(0, &[0x88, 20, 0xFE]);
    d[layout_of(0).env + 5] = 0x22;
    let (s, log) = run(&d, 4);
    let l = layout_of(0);
    assert_eq!(s.g(F_VIBPARAM, 0), 0x22);
    // the vibrato ran on the tick between the rows: shift 2, depth 2
    assert_eq!(s.rb(l.vibdepth), 0x02);
    assert_eq!(s.rw(l.vibdelta), ((period(20) - period(21)) / 2 | 1) / 2 | 1);
    assert!(reg(&log, PER).len() >= 2);
}

#[test]
fn loop_counts_and_tick_rate() {
    assert_eq!([0, 1, 128, 129, 255].map(count_first), [0, 1, 128, 0, 0]);
    assert_eq!([0, 1, 128, 129, 255].map(count_after), [1, 2, 129, 1, 1]);
    assert!(hz100(5394 + SAMPLES_SIZE_100HZ - 4, 5394));
    assert!(!hz100(5394 + SAMPLES_SIZE_100HZ, 5394));
    let (s, _) = init(&module(0, &ONE_NOTE), BASE).unwrap();
    assert_eq!(s.tick_clocks(), DT_TICK);
    let mut h = s;
    h.hz100 = true;
    assert_eq!(h.tick_clocks(), DT_TICK / 2);
}

#[test]
fn a_100_hz_song_end_waits_one_tick() {
    let d = module(1, &ONE_NOTE);
    let mut e = Engine::new(48000.0);
    e.m.bus.write_log = Some(Vec::new());
    let (mut s, _) = init(&d, BASE).unwrap();
    s.hz100 = true;
    let mut ended_at = None;
    let mut wrote_at = None;
    for t in 0..40 {
        let n0 = e.m.bus.write_log.as_ref().unwrap().len();
        s.tick(&mut Hw { bus: &mut e.m.bus });
        let log = e.m.bus.write_log.as_ref().unwrap();
        if ended_at.is_none() && s.songend() {
            ended_at = Some(t);
        }
        // the host's DMACON 15 (an output stage writes 15 only right after DMACON 0x8200)
        let host = (n0..log.len()).any(|i| (log[i].0, log[i].1) == (DMA, 15) && (i == 0 || (log[i - 1].0, log[i - 1].1) != (DMA, 0x8200)));
        if wrote_at.is_none() && host {
            wrote_at = Some(t);
        }
    }
    assert_eq!(wrote_at.unwrap(), ended_at.unwrap() + 1);
}

#[test]
fn set_subsong_restarts() {
    let d = module(2, &ONE_NOTE);
    let mut e = Engine::new(48000.0);
    let (mut s, _) = init(&d, BASE).unwrap();
    for _ in 0..30 {
        s.tick(&mut Hw { bus: &mut e.m.bus });
    }
    assert!(s.songend());
    s.set_subsong(1, &mut Hw { bus: &mut e.m.bus });
    assert!(!s.songend());
    assert_eq!(s.subsongs().2, 1);
    assert_eq!(s.rb(layout_of(2).flag), 2);
    s.set_subsong(9, &mut Hw { bus: &mut e.m.bus });
    assert_eq!(s.subsongs().2, 1);
}

#[test]
fn engine_picks_the_player_from_the_content() {
    let mut e = Engine::new(48000.0);
    e.open_replayers = true;
    e.m.bus.write_log = Some(Vec::new());
    e.play_data(Path::new("/sb-test/sb.song"), module(0, &ONE_NOTE)).unwrap();
    assert_eq!(e.playing.as_ref().unwrap().format, "Steve Barrett");
    let mut out = vec![0f32; 2 * 960];
    for _ in 0..30 {
        e.render(&mut out);
    }
    assert!(e.songend());
}
