//! The emitter's 68020 output against the interpreter's 68020 model: `m68k-recomp fuzz-gen --cpu
//! 68020` compiles random instructions (half of them drawn from the 68020 additions) to one
//! function each, and a checker crate built here runs every function and `m68k::interp::exec` on
//! the same random states; registers, flags, PC, the control registers (MOVEC, VBR) and every
//! byte written must agree. (The
//! interpreter's 68020 model is checked against the `m68k` crate's 68020 core by
//! crates/m68k-rc/tests/ref020.rs.)
//!
//! It builds a crate with cargo, so it takes a minute; `cargo test --release` runs it.
use std::path::Path;
use std::process::Command;

const CHECKER: &str = r#"
use m68k::{decode, interp, Bus, Cpu, Model};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
struct Mem { bytes: BTreeMap<u32, u8>, written: BTreeMap<u32, u8> }

fn fill(a: u32) -> u8 {
    let mut x = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5555;
    x ^= x >> 29;
    (x.wrapping_mul(0xBF58_476D_1CE4_E5B9) >> 56) as u8
}

impl Bus for Mem {
    fn read8(&mut self, a: u32) -> u8 { let a = a & 0xFF_FFFF; *self.bytes.get(&a).unwrap_or(&fill(a)) }
    fn read16(&mut self, a: u32) -> u16 { (self.read8(a) as u16) << 8 | self.read8(a.wrapping_add(1)) as u16 }
    fn write8(&mut self, a: u32, v: u8) { let a = a & 0xFF_FFFF; self.bytes.insert(a, v); self.written.insert(a, v); }
    fn write16(&mut self, a: u32, v: u16) { self.write8(a, (v >> 8) as u8); self.write8(a.wrapping_add(1), v as u8); }
}

struct Rng(u64);
impl Rng {
    fn u32(&mut self) -> u32 {
        let mut x = self.0; x ^= x >> 12; x ^= x << 25; x ^= x >> 27; self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    fn value(&mut self) -> u32 {
        const EDGE: [u32; 8] = [0, 1, 0xFFFF_FFFF, 0x8000_0000, 0x7FFF_FFFF, 0x8000, 0xFFFF, 0x1_0000];
        match self.u32() % 4 { 0 => EDGE[self.u32() as usize % 8], 1 => self.u32() & 0xFFFF, 2 => self.u32() & 0x3FFE, _ => self.u32() }
    }
}

fn main() {
    let mut r = Rng(0x1234_5678_9ABC_DEF1);
    let mut n = 0;
    for (i, &(pc, words)) in fuzz_gen::CASES.iter().enumerate() {
        let f = fuzz_gen::case::<Mem>(i);
        let mut mem = Mem::default();
        for (k, &w) in words.iter().enumerate() {
            let a = pc + 2 * k as u32;
            mem.bytes.insert(a & 0xFF_FFFF, (w >> 8) as u8);
            mem.bytes.insert((a + 1) & 0xFF_FFFF, w as u8);
        }
        // the case's instructions (a block of BLOCK, compiled as one function, ticks counted)
        let mut insns = vec![];
        let mut p = pc;
        for _ in 0..fuzz_gen::BLOCK {
            let ins = decode::decode_model(p, &mut |a| mem.clone().read16(a), Model::M68020);
            p = ins.next();
            insns.push(ins);
        }
        for _ in 0..20 {
            let mut c = Cpu { model: Model::M68020, ..Cpu::default() };
            for k in 0..8 { c.d[k] = r.value(); c.a[k] = r.value() & !1; }
            c.a[7] = (0x40_0000 + (r.u32() & 0xFFFF)) & !1;
            c.other_sp = (0x50_0000 + (r.u32() & 0xFFFF)) & !1;
            c.set_ccr(r.u32() as u8);
            c.s = r.u32() % 4 != 0;
            c.imask = (r.u32() % 8) as u8;
            c.pc = pc;
            // the 68010+ control registers (MOVEC), the VBR often 0
            if r.u32() % 2 == 0 {
                c.vbr = r.u32() & 0xFF_FFFC;
            }
            c.sfc = r.u32() % 8;
            c.dfc = r.u32() % 8;
            c.cacr = r.u32() % 4;
            c.caar = r.u32();
            c.msp = r.u32() & !1;
            let (mut c1, mut m1) = (c.clone(), mem.clone());
            f(&mut c1, &mut m1);
            let (mut c2, mut m2) = (c.clone(), mem.clone());
            for ins in &insns {
                if c2.pc != ins.pc {
                    break; // an exception left the block
                }
                interp::exec(&mut c2, &mut m2, ins);
                if fuzz_gen::BLOCK > 1 {
                    c2.ticks += 1;
                }
            }
            if c1 != c2 || m1.written != m2.written {
                let text: Vec<String> = insns.iter().map(|i| i.to_string()).collect();
                eprintln!("case {} ${:06x} {:?} {:04x?}\n  native {:?}\n  interp {:?}", i, pc, text, words, c1, c2);
                std::process::exit(1);
            }
            n += 1;
        }
    }
    println!("{} runs of {} cases agree", n, fuzz_gen::CASES.len());
}
"#;

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().expect("spawn");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{:?} failed:\n{}", cmd, text);
    text
}

#[test]
fn emitted_68020_code_matches_the_interpreter() {
    // under the target directory build.sh gives (CARGO_TARGET_TMPDIR may be in a shared build-dir)
    let dir = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| Path::new(env!("CARGO_TARGET_TMPDIR")).to_path_buf(), Into::into).join("emit020");
    let _ = std::fs::remove_dir_all(&dir);
    for (seed, block) in [(1, 1), (2, 1), (3, 4)] {
        let gen = dir.join(format!("gen{}", seed));
        run(Command::new(env!("CARGO_BIN_EXE_m68k-recomp")).args(["fuzz-gen", "--cpu", "68020", "--n", "4000", "--seed"])
            .arg(seed.to_string()).args(["--block", &block.to_string(), "--out"]).arg(&gen));
        let chk = dir.join(format!("check{}", seed));
        std::fs::create_dir_all(chk.join("src")).unwrap();
        let rc = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../crates/m68k-rc").canonicalize().unwrap();
        std::fs::write(chk.join("Cargo.toml"), format!(
            "[package]\nname = \"check\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nm68k = {{ package = \"m68k-rc\", path = {:?} }}\nfuzz_gen = {{ path = {:?} }}\n\n[workspace]\n",
            rc, gen)).unwrap();
        std::fs::write(chk.join("src/main.rs"), CHECKER).unwrap();
        let out = run(Command::new(env!("CARGO")).args(["run", "--release", "--offline", "--quiet", "--manifest-path"])
            .arg(chk.join("Cargo.toml")).env("CARGO_TARGET_DIR", dir.join("target")));
        assert!(out.contains("agree"), "{}", out);
        eprintln!("seed {} block {}: {}", seed, block, out.trim());
    }
}
