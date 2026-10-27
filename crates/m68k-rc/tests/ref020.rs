//! The 68020 additions of the decoder and interpreter ([`Model::M68020`]) against the `m68k`
//! crate's 68020 core, the one the player runs everything else on: random register states, one
//! instruction, then registers, SR, PC and every byte written must be the same.
//!
//! Memory is sparse: an unwritten byte reads as a hash of its address, the same for both cores,
//! and both see a 24-bit bus.
use m68k::{interp, Cpu, Model};
use m68k_ref::{AddressBus, CpuCore, CpuType, NoOpHleHandler};
use std::collections::BTreeMap;

const CODE: u32 = 0x1000;
/// Exception vector `v` points to `HANDLERS + 16 * v`.
const HANDLERS: u32 = 0xF0_0000;

#[derive(Clone, Default)]
struct Mem {
    bytes: BTreeMap<u32, u8>,
    written: BTreeMap<u32, u8>,
}

fn fill(a: u32) -> u8 {
    let mut x = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5555;
    x ^= x >> 29;
    (x.wrapping_mul(0xBF58_476D_1CE4_E5B9) >> 56) as u8
}

impl Mem {
    fn r8(&self, a: u32) -> u8 {
        let a = a & 0xFF_FFFF;
        match self.bytes.get(&a) {
            Some(&v) => v,
            None if a < 0x400 => (((HANDLERS + 16 * (a / 4)) >> (8 * (3 - (a & 3)))) & 0xFF) as u8,
            None => fill(a),
        }
    }
    fn w8(&mut self, a: u32, v: u8) {
        let a = a & 0xFF_FFFF;
        self.bytes.insert(a, v);
        self.written.insert(a, v);
    }
    fn put16(&mut self, a: u32, v: u16) {
        self.bytes.insert(a, (v >> 8) as u8);
        self.bytes.insert(a + 1, v as u8);
    }
}

impl AddressBus for Mem {
    fn read_byte(&mut self, a: u32) -> u8 {
        self.r8(a)
    }
    fn read_word(&mut self, a: u32) -> u16 {
        (self.r8(a) as u16) << 8 | self.r8(a.wrapping_add(1)) as u16
    }
    fn read_long(&mut self, a: u32) -> u32 {
        (AddressBus::read_word(self, a) as u32) << 16 | AddressBus::read_word(self, a.wrapping_add(2)) as u32
    }
    fn write_byte(&mut self, a: u32, v: u8) {
        self.w8(a, v)
    }
    fn write_word(&mut self, a: u32, v: u16) {
        self.w8(a, (v >> 8) as u8);
        self.w8(a.wrapping_add(1), v as u8);
    }
    fn write_long(&mut self, a: u32, v: u32) {
        AddressBus::write_word(self, a, (v >> 16) as u16);
        AddressBus::write_word(self, a.wrapping_add(2), v as u16);
    }
}

impl m68k::Bus for Mem {
    fn read8(&mut self, a: u32) -> u8 {
        self.r8(a)
    }
    fn read16(&mut self, a: u32) -> u16 {
        AddressBus::read_word(self, a)
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.w8(a, v)
    }
    fn write16(&mut self, a: u32, v: u16) {
        AddressBus::write_word(self, a, v)
    }
}

/// xorshift64*.
struct Rng(u64);
impl Rng {
    fn u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.u32() % n
    }
    /// A data value, often one of the edge cases of multiplication and division.
    fn value(&mut self) -> u32 {
        const EDGE: [u32; 12] = [0, 1, 2, 3, 0xFFFF_FFFF, 0xFFFF_FFFE, 0x8000_0000, 0x7FFF_FFFF, 0x8000, 0xFFFF, 0x1_0000, 0x7FFF];
        match self.below(4) {
            0 => EDGE[self.below(12) as usize],
            1 => self.u32() & 0xFFFF,
            2 => (self.u32() as i8) as i32 as u32,
            _ => self.u32(),
        }
    }
}

/// The machine state before the instruction.
#[derive(Clone)]
struct State {
    d: [u32; 8],
    a: [u32; 8],
    /// T S I CCR (68000 bits).
    sr: u16,
    other_sp: u32,
    /// VBR, SFC, DFC, CACR, CAAR, MSP (0 but in the MOVEC tests).
    ctrl: [u32; 6],
    mem: Mem,
}

fn random_state(r: &mut Rng, words: &[u16]) -> State {
    let mut d = [0; 8];
    for v in d.iter_mut() {
        *v = r.value();
    }
    let mut a = [0; 8];
    for v in a.iter_mut() {
        *v = (0x2_0000 + r.below(0x60_0000)) & !1;
    }
    let s = r.below(4) != 0;
    let sr = (s as u16) << 13 | (r.below(8) as u16) << 8 | r.below(32) as u16;
    let mut mem = Mem::default();
    for (i, &w) in words.iter().enumerate() {
        mem.put16(CODE + 2 * i as u32, w);
    }
    State { d, a, sr, other_sp: (0x70_0000 + r.below(0x8000)) & !1, ctrl: [0; 6], mem }
}

/// Runs one instruction on both cores; panics with `what` on any difference.
fn compare(st: &State, what: &str) {
    compare_n(st, what, 1)
}

/// Runs `steps` instructions on both cores; panics with `what` on any difference.
fn compare_n(st: &State, what: &str, steps: usize) {
    // m68k-rc, 68020 model
    let mut c = Cpu { model: Model::M68020, ..Cpu::default() };
    c.d = st.d;
    c.a = st.a;
    c.pc = CODE;
    c.s = st.sr & 0x2000 != 0;
    c.imask = ((st.sr >> 8) & 7) as u8;
    c.set_ccr(st.sr as u8);
    c.other_sp = st.other_sp;
    [c.vbr, c.sfc, c.dfc, c.cacr, c.caar, c.msp] = st.ctrl;
    let mut m1 = st.mem.clone();
    for _ in 0..steps {
        interp::step(&mut c, &mut m1);
    }

    // the reference
    let mut m2 = st.mem.clone();
    let mut r = CpuCore::new();
    r.set_cpu_type(CpuType::M68020);
    r.reset(&mut m2);
    m2.written.clear();
    r.dar[..8].copy_from_slice(&st.d);
    r.dar[8..].copy_from_slice(&st.a);
    r.set_sr_noint_nosp(st.sr);
    // sp[0] USP, sp[4] ISP: the inactive one (the active one is A7)
    if st.sr & 0x2000 != 0 {
        r.sp[0] = st.other_sp;
    } else {
        r.sp[4] = st.other_sp;
    }
    [r.vbr, r.sfc, r.dfc, r.cacr, r.caar, r.sp[6]] = st.ctrl;
    r.pc = CODE;
    r.invalidate_prefetch();
    // with a handler that declines every trap, they are taken as the hardware takes them
    for _ in 0..steps {
        let _ = r.step_with_hle_handler(&mut m2, &mut NoOpHleHandler);
    }
    let r_other = if r.get_sr() & 0x2000 != 0 { r.sp[0] } else { r.sp[4] };

    if std::env::var_os("REF020_DEBUG").is_some() && (c.pc != r.pc || c.a != r.dar[8..]) {
        let mut m = st.mem.clone();
        let words = |m: &mut Mem, a: u32| (0..8).map(|i| AddressBus::read_word(m, a + 2 * i)).collect::<Vec<_>>();
        eprintln!("sr {:04x} a7 {:x} {:04x?} other {:x} {:04x?}", st.sr, st.a[7], words(&mut m, st.a[7]), st.other_sp, words(&mut m, st.other_sp));
    }
    let mine = (c.d, c.a, c.pc, c.sr(), c.other_sp, [c.vbr, c.sfc, c.dfc, c.cacr, c.caar, c.msp]);
    let theirs = (
        <[u32; 8]>::try_from(&r.dar[..8]).unwrap(),
        <[u32; 8]>::try_from(&r.dar[8..]).unwrap(),
        r.pc,
        r.get_sr() & 0xA71F,
        r_other,
        [r.vbr, r.sfc, r.dfc, r.cacr, r.caar, r.sp[6]],
    );
    assert_eq!(mine, theirs, "{}: registers (d, a, pc, sr, other sp, vbr sfc dfc cacr caar msp)\nstate d={:08x?} a={:08x?} sr={:04x}", what, st.d, st.a, st.sr);
    assert_eq!(m1.written, m2.written, "{}: memory written", what);
}

fn run(seed: u64, n: usize, mut case: impl FnMut(&mut Rng) -> (Vec<u16>, String), mut setup: impl FnMut(&mut Rng, &mut State)) {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    for _ in 0..n {
        let (words, what) = case(&mut r);
        let mut st = random_state(&mut r, &words);
        setup(&mut r, &mut st);
        compare(&st, &format!("{} {:04x?}", what, words));
    }
    if std::env::var_os("REF020_DEBUG").is_some() {
        eprintln!("seed {}: {} cases", seed, n);
    }
}

/// A data-mode source EA (Dn, (An), d16(An), #imm.l) and its extension words.
fn src_ea(r: &mut Rng) -> (u16, Vec<u16>) {
    match r.below(4) {
        0 => (r.below(8) as u16, vec![]),
        1 => (0x10 | r.below(8) as u16, vec![]),
        2 => (0x28 | r.below(8) as u16, vec![r.u32() as u16]),
        _ => {
            let v = r.value();
            (0x3C, vec![(v >> 16) as u16, v as u16])
        }
    }
}

#[test]
fn mul_long() {
    run(1, 20_000, |r| {
        let (ea, ext) = src_ea(r);
        let x = (r.below(8) << 12 | r.below(2) << 11 | r.below(2) << 10 | r.below(8)) as u16;
        let mut w = vec![0x4C00 | ea, x];
        w.extend(ext);
        (w, "mul.l".into())
    }, |_, _| {});
}

#[test]
fn div_long() {
    run(2, 20_000, |r| {
        let (ea, ext) = src_ea(r);
        let x = (r.below(8) << 12 | r.below(2) << 11 | r.below(2) << 10 | r.below(8)) as u16;
        let mut w = vec![0x4C40 | ea, x];
        w.extend(ext);
        (w, "div.l".into())
    }, |r, st| {
        // small divisors and dividends now and then, so that not every 64-bit division overflows
        if r.below(2) == 0 {
            for v in st.d.iter_mut() {
                if r.below(2) == 0 {
                    *v = r.u32() >> r.below(32);
                }
            }
        }
    });
}

#[test]
fn extb_move_ccr_tst() {
    run(3, 10_000, |r| {
        let n = r.below(8) as u16;
        match r.below(6) {
            0 => (vec![0x49C0 | n], "extb.l".into()),
            1 => (vec![0x42C0 | n], "move ccr,dn".into()),
            2 => (vec![0x42C0 | [0x10, 0x18, 0x20][r.below(3) as usize] | n], "move ccr,mem".into()),
            3 => (vec![[0x4A48, 0x4A88][r.below(2) as usize] | n], "tst.w/l an".into()),
            4 => (vec![0x4A3A | (r.below(3) as u16) << 6, r.u32() as u16], "tst d16(pc)".into()),
            _ => {
                let sz = r.below(3) as u16;
                let mut w = vec![0x4A3C | sz << 6, r.u32() as u16];
                if sz == 2 {
                    w.push(r.u32() as u16);
                }
                (w, "tst #imm".into())
            }
        }
    }, |_, _| {});
}

#[test]
fn branch_long() {
    run(4, 5_000, |r| {
        let d = r.u32() & 0x00FF_FFFE;
        (vec![0x60FF | (r.below(16) as u16) << 8, (d >> 16) as u16, d as u16], "bcc.l/bra.l/bsr.l".into())
    }, |_, _| {});
}

/// A brief extension word with a scale factor, or a full one (any combination of base and index
/// suppression, base and outer displacement sizes and indirection), and its extension words.
fn ext020(r: &mut Rng) -> Vec<u16> {
    let xr = (r.below(16) << 12 | r.below(2) << 11 | r.below(4) << 9) as u16;
    let disp = |r: &mut Rng, size: u32| -> Vec<u16> {
        match size {
            2 => vec![(r.u32() & 0x00FF) as u16],
            3 => vec![0, (r.u32() & 0x7FFE) as u16],
            _ => vec![],
        }
    };
    if r.below(3) == 0 {
        return vec![xr | 0x0200 | (r.u32() & 0xFF) as u16];
    }
    let bs = r.below(2);
    let is = r.below(2);
    let bd = 1 + r.below(3);
    let iis = if is == 1 { r.below(4) } else { r.below(8) };
    let mut w = vec![xr | 0x0100 | (bs << 7 | is << 6 | bd << 4 | iis) as u16];
    w.extend(disp(r, bd));
    w.extend(disp(r, iis & 3));
    w
}

#[test]
fn ea_020() {
    run(5, 30_000, |r| {
        let an = r.below(8) as u16;
        let (mode, name) = if r.below(4) == 0 { (0x3B, "pc") } else { (0x30 | an, "an") };
        let ext = ext020(r);
        let mut w = match r.below(5) {
            0 => vec![0x2000 | mode],                                   // move.l <ea>,d0
            1 => vec![0x41C0 | (r.below(8) as u16) << 9 | mode],         // lea <ea>,an
            2 => vec![0xD080 | (r.below(8) as u16) << 9 | mode],         // add.l <ea>,dn
            3 if mode != 0x3B => vec![0x3001 | (mode & 7) << 9 | 0x180], // move.w d1,<ea>
            _ => vec![0x4A80 | mode],                                    // tst.l <ea>
        };
        w.extend(ext);
        (w, format!("68020 ea ({})", name))
    }, |r, st| {
        // index values that keep most addresses near the registers
        for v in st.d.iter_mut().chain(st.a.iter_mut()) {
            if r.below(2) == 0 {
                *v = r.u32() & 0x3FFE;
            }
        }
    });
}

#[test]
fn ea_020_memory_indirect_jmp() {
    run(6, 5_000, |r| {
        let mut w = vec![0x4EF0 | r.below(8) as u16, 0x0100 | (r.below(16) << 12 | r.below(2) << 11 | r.below(4) << 9 | (1 + r.below(3)) << 4 | 1 + r.below(7)) as u16];
        if w[1] & 0x30 == 0x20 {
            w.push(r.u32() as u16 & 0xFF);
        } else if w[1] & 0x30 == 0x30 {
            w.extend([0, r.u32() as u16 & 0xFFE]);
        }
        match w[1] & 3 {
            2 => w.push(r.u32() as u16),
            3 => w.extend([r.u32() as u16, r.u32() as u16]),
            _ => {}
        }
        (w, "jmp ([..])".into())
    }, |_, _| {});
}

#[test]
fn privileged_move_from_sr() {
    run(7, 2_000, |r| (vec![0x40C0 | r.below(8) as u16], "move sr,dn".into()), |_, _| {});
}

#[test]
fn exceptions_020() {
    run(8, 5_000, |r| match r.below(6) {
        0 => (vec![0x80FC | (r.below(8) as u16) << 9, 0], "divu.w #0".into()),
        1 => (vec![0x4C7C, (r.below(8) << 12 | r.below(2) << 11 | r.below(2) << 10 | r.below(8)) as u16, 0, 0], "divl #0".into()),
        2 => (vec![0x41BC | (r.below(8) as u16) << 9, r.u32() as u16 & 0x8FFF], "chk.w #imm".into()),
        3 => (vec![0x4E76], "trapv".into()),
        4 => (vec![0x4AFC], "illegal".into()),
        _ => (vec![0x4E40 | r.below(16) as u16], "trap".into()),
    }, |_, _| {});
}

#[test]
fn rte_020() {
    run(9, 3_000, |_| (vec![0x4E73], "rte".into()), |r, st| {
        // A frame of format 0, 1 (a throwaway frame, followed by a format 0 one on the same
        // stack), 2, or one a 68020 RTE does not take. Not $A/$B: the bus fault frames are not
        // implemented (see `Cpu::rte_020`). T stays clear: trace is not emulated.
        st.sr |= 0x2000;
        let sp = st.a[7];
        let format = [0u16, 1, 2, 3, 9, 0xF][r.below(6) as usize];
        let mut put = |a: u32, v: u16| st.mem.put16(a, v);
        put(sp, r.u32() as u16 & 0x271F | if format == 1 { 0x2000 } else { 0 });
        put(sp + 2, 0x0010);
        put(sp + 4, (r.u32() as u16) & 0xFFFE);
        put(sp + 6, format << 12 | (r.below(64) as u16) << 2);
        if format == 1 {
            put(sp + 8, 0x2000 | (r.u32() as u16 & 0x071F));
            put(sp + 10, 0x0020);
            put(sp + 12, (r.u32() as u16) & 0xFFFE);
            put(sp + 14, (r.below(64) as u16) << 2);
        }
    });
}

/// Random instruction words, anything the 68020 decoder takes (68000 instructions included: in
/// 68020 mode they must behave as on a 68020). Left out: what writes SR as a whole (MOVE to SR,
/// ORI/ANDI/EORI to SR, RTE, STOP), because the M and T0 bits and tracing are not modelled, and
/// RESET.
#[test]
fn random_020() {
    let mut r = Rng(77);
    let mut n = 0;
    while n < 200_000 {
        // 11 words: the longest 68020 instruction
        let words: Vec<u16> = (0..11).map(|_| r.u32() as u16).collect();
        let ins = m68k::decode::decode_model(CODE, &mut |a| words[((a - CODE) / 2) as usize], Model::M68020);
        use m68k::Op;
        if matches!(ins.op, Op::Illegal | Op::LineA | Op::LineF | Op::Stop { .. } | Op::Reset | Op::Rte | Op::MoveToSr { .. } | Op::LogicSr { sr: true, .. }) {
            continue;
        }
        n += 1;
        let mut st = random_state(&mut r, &words);
        st.sr &= 0x271F;
        compare(&st, &format!("{} {:04x?}", ins, &words[..(ins.len / 2) as usize]));
    }
}

/// The 68020 control register codes, and some it does not have (68010-, 68040- and 68060-only
/// ones, and unused ones).
const MOVEC_CODES: [u16; 8] = [0x000, 0x001, 0x002, 0x800, 0x801, 0x802, 0x803, 0x804];
const MOVEC_UNKNOWN: [u16; 8] = [0x003, 0x004, 0x008, 0x805, 0x806, 0x807, 0x808, 0xFFF];

/// Random control registers (the MOVEC tests), the VBR pointing at a vector table of its own.
fn random_ctrl(r: &mut Rng, st: &mut State) {
    let vbr = if r.below(4) == 0 { 0 } else { (0x8_0000 + r.below(0x1_0000) * 0x10) & 0xFF_FFFC };
    st.ctrl = [vbr, r.below(8), r.below(8), r.below(4), r.u32(), (0x78_0000 + r.below(0x8000)) & !1];
    if vbr != 0 {
        // vector v of the relocated table -> HANDLERS + 0x8000 + 16 * v
        for v in 0..256u32 {
            let h = HANDLERS + 0x8000 + 16 * v;
            st.mem.put16(vbr + 4 * v, (h >> 16) as u16);
            st.mem.put16(vbr + 4 * v + 2, h as u16);
        }
    }
}

/// MOVEC Rc,Rn and Rn,Rc with every 68020 control register and every Dn/An (A7 included), in
/// supervisor and user mode (a privilege violation).
#[test]
fn movec() {
    run(10, 20_000, |r| {
        let to = r.below(2) as u16;
        let code = MOVEC_CODES[r.below(8) as usize];
        (vec![0x4E7A | to, (r.below(16) as u16) << 12 | code], format!("movec {} {:03x}", if to == 1 { "rn,rc" } else { "rc,rn" }, code))
    }, |r, st| {
        random_ctrl(r, st);
        // values the masked registers (SFC, DFC, CACR) cut down
        if r.below(2) == 0 {
            for v in st.d.iter_mut() {
                *v = r.u32();
            }
        }
    });
}

/// Codes the 68020 does not have: an illegal instruction in supervisor mode, a privilege
/// violation in user mode (the privilege check comes first), through the VBR.
#[test]
fn movec_unknown() {
    run(11, 5_000, |r| {
        let to = r.below(2) as u16;
        let code = if r.below(2) == 0 { MOVEC_UNKNOWN[r.below(8) as usize] } else { r.u32() as u16 & 0x0FFF };
        (vec![0x4E7A | to, (r.below(16) as u16) << 12 | code], format!("movec unknown {:03x}", code))
    }, random_ctrl);
}

/// Exceptions with a relocated VBR: the vector is read at VBR + 4 * vector.
#[test]
fn vbr_exceptions() {
    run(12, 5_000, |r| match r.below(6) {
        0 => (vec![0x80FC | (r.below(8) as u16) << 9, 0], "divu.w #0".into()),
        1 => (vec![0x4AFC], "illegal".into()),
        2 => (vec![0x46C0 | r.below(8) as u16], "move dn,sr (privilege violation in user mode)".into()),
        3 => (vec![0xA000 | r.u32() as u16 & 0xFFF], "line a".into()),
        4 => (vec![0x41BC | (r.below(8) as u16) << 9, r.u32() as u16 & 0x8FFF], "chk.w #imm".into()),
        _ => (vec![0x4E40 | r.below(16) as u16], "trap".into()),
    }, |r, st| {
        random_ctrl(r, st);
        // MOVE to SR only for the privilege violation: user mode
        if st.mem.r8(CODE) == 0x46 {
            st.sr &= !0x2000;
        }
    });
}

/// `movec d0,vbr` then a TRAP in supervisor mode: the TRAP goes through the new table.
#[test]
fn movec_vbr_then_trap() {
    let mut r = Rng(13);
    for _ in 0..2_000 {
        let n = r.below(16) as u16;
        let words = [0x4E7B, 0x0801, 0x4E40 | n];
        let mut st = random_state(&mut r, &words);
        st.sr |= 0x2000;
        let mut c = State { ctrl: [0; 6], ..st.clone() };
        random_ctrl(&mut r, &mut c);
        // the new VBR comes from D0; the old one is 0
        st.d[0] = c.ctrl[0];
        st.mem = c.mem;
        compare_n(&st, &format!("movec d0,vbr; trap #{} (vbr {:x})", n, st.d[0]), 2);
    }
}
