//! IR lifting: every instruction, evaluated from its micro-ops, must do exactly what the
//! interpreter does (registers, flags, SR, memory and the order of bus accesses), on random
//! instructions and random states; with its flags eliminated it must do the same except for the
//! flags; with them behind the budget test, the same as exact or as eliminated. And the CFG's
//! flag liveness across blocks.
use m68k::{interp, Bus, Cpu};
use recomp::ir::{self, Edge, IrInsn, ALL};
use std::collections::BTreeMap;

const MEM: usize = 0x4000;

/// 16 KB of memory (addresses wrap) that logs every access.
#[derive(Clone)]
struct LogBus {
    mem: Vec<u8>,
    log: Vec<(char, u32, u32)>,
}

impl Bus for LogBus {
    fn read8(&mut self, a: u32) -> u8 {
        let v = self.mem[a as usize % MEM];
        self.log.push(('r', a, v as u32));
        v
    }
    fn read16(&mut self, a: u32) -> u16 {
        let v = (self.mem[a as usize % MEM] as u16) << 8 | self.mem[a.wrapping_add(1) as usize % MEM] as u16;
        self.log.push(('R', a, v as u32));
        v
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.mem[a as usize % MEM] = v;
        self.log.push(('w', a, v as u32));
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.mem[a as usize % MEM] = (v >> 8) as u8;
        self.mem[a.wrapping_add(1) as usize % MEM] = v as u8;
        self.log.push(('W', a, v as u32));
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

fn random_cpu(rng: &mut Rng, pc: u32) -> Cpu {
    let mut cpu = Cpu::default();
    for r in 0..8 {
        // some small values (shift and bit counts, loop counters)
        cpu.d[r] = if rng.next() % 3 == 0 { rng.next() % 70 } else { rng.next() };
        // keep some address registers small and even, some arbitrary
        cpu.a[r] = if rng.next() & 1 == 0 { rng.next() & 0xFFFE } else { rng.next() };
    }
    cpu.other_sp = rng.next();
    cpu.set_sr(rng.next() as u16 & 0xA71F);
    cpu.pc = pc;
    cpu
}

fn decode(words: &[u16], pc: u32) -> m68k::Instr {
    m68k::decode::decode(pc, &mut |a| words[((a.wrapping_sub(pc)) / 2) as usize % words.len()])
}

fn flags_of(c: &Cpu) -> (bool, bool, bool, bool, bool) {
    (c.x, c.n, c.z, c.v, c.c)
}

/// Runs `ins` in the interpreter and `ir` in the evaluator from the same state; returns both.
fn run(ins: &m68k::Instr, ir: &IrInsn, cpu: &Cpu, mem: &[u8]) -> ((Cpu, LogBus), (Cpu, LogBus)) {
    let (mut c1, mut b1) = (cpu.clone(), LogBus { mem: mem.to_vec(), log: vec![] });
    let (mut c2, mut b2) = (cpu.clone(), LogBus { mem: mem.to_vec(), log: vec![] });
    interp::exec(&mut c1, &mut b1, ins);
    assert!(ir::eval::exec(&mut c2, &mut b2, ir));
    ((c1, b1), (c2, b2))
}

fn same(ins: &m68k::Instr, ir: &IrInsn, a: &(Cpu, LogBus), b: &(Cpu, LogBus), what: &str) {
    assert_eq!(a.0, b.0, "{}: {:06x}: {} {:?} {:?}", what, ins.pc, ins, ir.ops(), ir.term);
    assert_eq!(a.1.log, b.1.log, "{}: {:06x}: {} {:?}", what, ins.pc, ins, ir.ops());
    assert!(a.1.mem == b.1.mem, "{}: {}", what, ins);
}

/// Everything but the five flags.
fn same_but_flags(ins: &m68k::Instr, ir: &IrInsn, a: &(Cpu, LogBus), b: &(Cpu, LogBus), what: &str) {
    let mut c = b.0.clone();
    c.set_ccr(a.0.ccr());
    assert_eq!(a.0, c, "{}: {:06x}: {} {:?} {:?}", what, ins.pc, ins, ir.ops(), ir.term);
    assert_eq!(a.1.log, b.1.log, "{}: {:06x}: {} {:?}", what, ins.pc, ins, ir.ops());
    assert!(a.1.mem == b.1.mem, "{}: {}", what, ins);
}

/// Checks one instruction in every flag mode; returns the op and terminator kinds it used.
fn check(ins: &m68k::Instr, rng: &mut Rng, mem: &[u8], states: usize, kinds: &mut BTreeMap<String, usize>) {
    let mut ir = ir::lift(ins);
    for op in ir.ops() {
        *kinds.entry(format!("{:?}", op).split([' ', '{']).next().unwrap().to_string()).or_default() += 1;
    }
    if let Some(t) = &ir.term {
        *kinds.entry(format!("Term::{:?}", t).split([' ', '{']).next().unwrap().to_string()).or_default() += 1;
    }
    for _ in 0..states {
        let cpu = random_cpu(rng, ins.pc);
        // exact (all flags live)
        ir.set_live(ALL, ALL);
        let (a, b) = run(ins, &ir, &cpu, mem);
        same(ins, &ir, &a, &b, "exact");
        // flags eliminated
        ir.set_live(0, 0);
        let (a, b) = run(ins, &ir, &cpu, mem);
        same_but_flags(ins, &ir, &a, &b, "plain");
        let plain = b;
        // behind the budget test: exact when the budget is exhausted, plain otherwise
        ir.set_live(0, ALL);
        let mut c = cpu.clone();
        c.budget = 0;
        let (a, b) = run(ins, &ir, &c, mem);
        same(ins, &ir, &a, &b, "guarded, budget exhausted");
        c.budget = u64::MAX;
        let (_, b) = run(ins, &ir, &c, mem);
        assert_eq!(flags_of(&b.0), flags_of(&plain.0), "guarded, budget left: {}", ins);
        same(ins, &ir, &plain, &b, "guarded, budget left");
    }
}

#[test]
fn lifted_ops_match_the_interpreter() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mem: Vec<u8> = (0..MEM).map(|_| rng.next() as u8).collect();
    let mut kinds = BTreeMap::new();
    for _ in 0..60_000 {
        let words: Vec<u16> = (0..5).map(|_| rng.next() as u16).collect();
        let ins = decode(&words, 0x1000);
        check(&ins, &mut rng, &mem, 3, &mut kinds);
    }
    eprintln!("60000 random instructions lifted, 3 states each");
}

/// Every opcode word (with random extension words), 2 states each; every micro-op and
/// terminator kind must have been exercised.
#[test]
fn every_opcode_matches_the_interpreter() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mem: Vec<u8> = (0..MEM).map(|_| rng.next() as u8).collect();
    let mut kinds = BTreeMap::new();
    let mut mnemonics: BTreeMap<String, usize> = BTreeMap::new();
    for op in 0..=0xFFFFu32 {
        let mut words: Vec<u16> = (0..5).map(|_| rng.next() as u16).collect();
        words[0] = op as u16;
        let ins = decode(&words, 0x2000);
        *mnemonics.entry(format!("{}", ins).split([' ', '.']).next().unwrap().to_string()).or_default() += 1;
        check(&ins, &mut rng, &mem, 2, &mut kinds);
    }
    eprintln!("65536 opcodes lifted ({} mnemonics), 2 states each", mnemonics.len());
    eprintln!("mnemonics: {:?}", mnemonics);
    eprintln!("micro-ops and terminators: {:?}", kinds);
    let all = [
        "Addr", "PostInc", "PreDec", "GetD", "GetA", "Load", "SextW", "SextB", "Bin", "Arith", "LogicFlags", "Unary", "Extend",
        "Nbcd", "Shift", "Mul", "Div", "Chk", "BitZ", "Scc", "GetCcr", "SetCcr", "GetSr", "SetSr", "GetOsp", "SetOsp", "Push",
        "Pop", "SetD", "SetA", "AddA", "Store", "StorePd32", "Priv", "TrapIf", "Term::Branch", "Term::Dbcc", "Term::Goto",
        "Term::Call", "Term::DynCall", "Term::DynJump", "Term::Return", "Term::Exception", "Term::Stop",
    ];
    for k in all {
        assert!(kinds.contains_key(k), "{} never exercised", k);
    }
}

/// A raw image at $1000 from words, discovered from $1000 and lifted.
fn function(words: &[u16]) -> ir::IrFunc {
    let mut img = recomp::Image::default();
    img.segs.push(recomp::Segment { name: "raw".into(), base: 0x1000, data: words.iter().flat_map(|w| w.to_be_bytes()).collect(), code: true });
    let funcs = recomp::discover(&img, &[0x1000], &recomp::Options::default());
    let mut p = ir::cfg::build_program(&funcs, &img);
    p.funcs.remove(&0x1000).unwrap()
}

#[test]
fn liveness_marks_overwritten_flags_dead() {
    // moveq #1,d0 ; moveq #2,d1 ; rts: the first moveq's flags are overwritten, the second's live.
    let f = function(&[0x7001, 0x7202, 0x4E75]);
    let b = &f.blocks[&0x1000];
    assert_eq!(b.insns.len(), 3);
    assert_eq!(b.insns[0].live_after & 0x0F, 0);
    assert_eq!(b.insns[0].live_exit & 0x0F, 0);
    assert_eq!(b.insns[1].live_after, ALL);
    assert_eq!(b.edges, vec![Edge::Return]);
    assert_eq!(f.key, Some((0, 0)));
}

#[test]
fn liveness_across_blocks() {
    // $1000 add.l d2,d3      flags dead in native code (the next block redefines all of them)
    // $1002 bra.s $1006      ... but visible at $1006's budget exit: guarded
    // $1004 nop
    // $1006 add.l d4,d5
    // $1008 rts              all flags live (return)
    let f = function(&[0xD682, 0x6002, 0x4E71, 0xDA84, 0x4E75]);
    let a = &f.blocks[&0x1000];
    assert_eq!(a.edges, vec![Edge::Jump(0x1006)]);
    assert_eq!(a.insns[0].live_after, 0);
    assert_eq!(a.insns[0].live_exit, ALL);
    assert_eq!(a.insns[0].ops().iter().filter_map(|o| o.lazy().map(|_| o.flag_mode())).collect::<Vec<_>>(), vec![ir::FlagMode::Guarded]);
    let b = &f.blocks[&0x1006];
    assert_eq!(b.insns[0].live_after, ALL);
    assert_eq!(b.insns[0].ops().iter().filter_map(|o| o.lazy().map(|_| o.flag_mode())).collect::<Vec<_>>(), vec![ir::FlagMode::Exact]);

    // $1000 subq.w #1,d0 ; bne.s $1000 ; moveq #0,d1 ; bsr.s $100a ; rts ; ($100a) rts
    // bne is fused with subq (computed from d0 - 1, reading no flag); X stays live: moveq
    // leaves it, and the call reads every flag.
    let f = function(&[0x5340, 0x66FC, 0x7200, 0x6102, 0x4E75, 0x4E75]);
    let l = &f.blocks[&0x1000];
    assert_eq!(l.insns[1].fused, Some(ir::Fused { from: 0, src: ir::CondSrc::Sub(m68k::Sz::W) }));
    assert_eq!(l.insns[1].flag_uses, 0);
    assert!(l.insns[0].export.is_some());
    assert_eq!(l.insns[0].live_after, ir::FX);
    let m = &f.blocks[&0x1004];
    assert!(matches!(m.edges[..], [Edge::Call { target: 0x100a, resume: 0x1008 }]));
    assert_eq!(m.insns[0].live_after, ALL);

    // computed jump: every flag live at the jump
    // $1000 moveq #0,d0 ; jmp (a0)
    let f = function(&[0x7000, 0x4ED0]);
    let b = &f.blocks[&0x1000];
    assert_eq!(b.edges, vec![Edge::DynJump]);
    assert_eq!(b.insns[0].live_after, ALL);

    // without fusion (bcc reads C from a shift): asl.w #1,d0 ; bcc.s $1000 ; rts
    let f = function(&[0xE340, 0x64FC, 0x4E75]);
    let l = &f.blocks[&0x1000];
    assert_eq!(l.insns[1].fused, None);
    assert_eq!(l.insns[1].flag_uses, ir::FC);
    assert_eq!(l.insns[0].live_after, ALL);

    // a loop whose exit redefines the flags: tst.w d0 at the head, the loop back edge from beq
    // $1000 tst.w d0 ; beq.s $1008 ; addq.w #1,d0 ; bra.s $1000 ; ($1008) moveq #0,d1 ; rts
    let f = function(&[0x4A40, 0x6706, 0x5240, 0x60F8, 0x7200, 0x4E75]);
    let body = &f.blocks[&0x1004];
    // addq's flags: N Z V C redefined by tst at the head, X live (moveq/rts path keeps it)
    assert_eq!(body.insns[0].live_after, ir::FX);
}
