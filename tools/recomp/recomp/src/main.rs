//! m68k-recomp command line.
//!
//!   m68k-recomp fuzz-gen --n N --seed S --out DIR [--block K] [--ir [--fuse]] [--cpu 68020]
//!       N random single instructions, each compiled to its own Rust function (emitter tests).
//!       With --cpu 68020 they are decoded and emitted for the 68020, and half of them are drawn
//!       from the 68020 additions (tools/recomp/tests/emit020.rs runs them).
//!   m68k-recomp hunk [FILE|--adf ADF:PATH] [--config PROG.toml] --out DIR --name CRATE [--base HEX]
//!               [--profile generic|asm|manx|sasc] [--manx] [--flat] [--seeds FILE] [--names FILE]
//!               [--coverage FILE].. [--data A-B].. [--exclude A-B].. [--no-flag-elim] [--ir [--ir-split N]]
//!       Recompile an AmigaDOS executable. Sections are placed from --base (default $20000).
//!   m68k-recomp raw FILE --base HEX --entry HEX [--entry HEX].. --out DIR --name CRATE [same options]
//!   m68k-recomp raw IMAGE --slice LO-HI [--unit] --entry HEX.. | --seeds FILE.. --out DIR --name CRATE
//!       FILE is a memory image (e.g. a 16 MB dump): its bytes [LO, HI) are the input, at base LO.
//!       --unit: the crate is a code unit for a dispatcher that runs it only while its bytes are
//!       in memory (amiga-machine `CodeUnit`): it also gets the image (image.bin), BASE, CODE (the
//!       compiled instructions' byte ranges) and HASH (FNV-1a of those bytes).
//!   m68k-recomp report [hunk|raw] <same input options> [--all]
//!       No output crate: lists unresolved computed jumps/calls (with disassembly) to hint.
//!   m68k-recomp map <same input options>
//!       No output crate: JSON lines, one per discovered function (see `map_dump`).
//!   --static-seeds [FILE]: also search the image for code nothing known reaches (pointer tables,
//!   addresses code loads, function-looking gaps; `recomp::scan`) and seed it; FILE lists it.
//!   --stable-with DUMP (repeatable): bytes of the slice that differ in these images are not used.
//!   --static-only: with it, the crate keeps only the functions the search added (a unit of its own).
//!   Discovery options: --pcrel-seeds (pea/lea func(pc) prologue targets), --gap-seeds
//!   (unreferenced `link` prologues); config keys `pcrel_seeds`, `gap_seeds`, `bases`, `align`,
//!   `hunk_gap` (see config.rs).
//!
//!   --names (no FILE after it; config `names = true`): readable function names derived from the
//!   hardware registers, OS calls and structure of each function (`recomp::names`), doc comments
//!   listing them, and OUT/names.txt (address, name, reason) to edit and pass back as --names FILE.
//!
//!   --cpu 68000|68020: the CPU the input is decoded and emitted for (default 68000; the 68020
//!   one decodes and emits what `m68k::decode` lists, and is not supported with --ir).
//!
//! Command-line values override the config file (input, base, profile, out, name, flat); list
//! options (seeds, names, coverage, data, exclude) add to the config's lists.
use m68k::*;
use recomp::config::{self, Addr, Config};
use recomp::ir::annot;
use recomp::emit::{emit_instr, Ctx, Mode};
use recomp::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn arg(args: &[String], k: &str) -> Option<String> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1).cloned())
}
fn args_all(args: &[String], k: &str) -> Vec<String> {
    args.iter().enumerate().filter(|(_, a)| *a == k).filter_map(|(i, _)| args.get(i + 1).cloned()).collect()
}
fn hexu(s: &str) -> u32 {
    let t = s.trim_start_matches("0x").trim_start_matches('$');
    u32::from_str_radix(t, 16).unwrap_or_else(|_| panic!("bad hex {}", s))
}

fn m68k_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../crates/m68k-rc").canonicalize().unwrap()
}

/// Same PRNG as the harness (xorshift64*).
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
}

/// Makes `w` one of the 68020 additions, or an instruction with a 68020 extension word (it may
/// still decode as illegal, like any random word).
fn fuzz_020(rng: &mut Rng, w: &mut [u16; 11]) {
    let r = rng.u32();
    let ea = (r >> 8) as u16 & 0x3F;
    let n = (r >> 16) as u16 & 7;
    w[0] = match r % 10 {
        0 => 0x4C00 | ea,               // mul.l
        1 => 0x4C40 | ea,               // div.l
        2 => 0x49C0 | n,                // extb.l
        3 => 0x42C0 | ea,               // move ccr,<ea>
        4 => 0x4A00 | (r >> 20) as u16 & 0xC0 | ea, // tst (any mode)
        5 => [0xC100, 0x8100][(r >> 4) as usize & 1] | n << 9 | (r >> 24) as u16 & 0xF, // abcd/sbcd
        6 => [0x4800 | ea, 0x4E57, 0x48A0 | n, 0x48E0 | n][(r >> 4) as usize & 3], // nbcd, link a7, movem -(an)
        7 => 0x60FF | (r >> 4) as u16 & 0x0F00, // bcc.l
        8 if r >> 31 != 0 => {
            // movec, mostly with a 68020 control register
            let codes = [0x000, 0x001, 0x002, 0x800, 0x801, 0x802, 0x803, 0x804];
            let code = if r & 0xE0 != 0 { codes[(r >> 24) as usize & 7] } else { w[1] & 0x0FFF };
            w[1] = (w[1] & 0xF000) | code;
            0x4E7A | (r >> 5) as u16 & 1
        }
        _ => {
            // an EA of mode 6 or (pc,xn) with a scale factor or a full extension word
            let k = w[0] & !0x3F | if r & 0x10 != 0 { 0x30 | n } else { 0x3B };
            w[1] = (w[1] & !0x0700) | if r & 0x20 != 0 { 0x0100 } else { 0x0200 };
            k
        }
    };
}

fn fuzz_gen(args: &[String]) {
    let n: usize = arg(args, "--n").map(|s| s.parse().unwrap()).unwrap_or(5000);
    let seed: u64 = arg(args, "--seed").map(|s| s.parse().unwrap()).unwrap_or(1);
    // --block K: each case is a straight-line block of K instructions compiled as one function
    // (exercises dead-flag elimination and instruction counting across instructions).
    let block: usize = arg(args, "--block").map(|s| s.parse().unwrap()).unwrap_or(1);
    // --ir: compile the cases from the IR instead of with the instruction emitter
    let use_ir = args.iter().any(|a| a == "--ir");
    // --fuse (with --ir and --block K > 1): each block ends with a Bcc or DBcc, often after a
    // flag-setting instruction, so that conditions are fused (computed from the operands); no
    // flag is live after the block, and the cases run with the budget exhausted after the block
    // (`cpu.budget = ticks + 1`), so that every flag is still computed, behind the budget test.
    let fuse = use_ir && block > 1 && args.iter().any(|a| a == "--fuse");
    let model = match arg(args, "--cpu").as_deref() {
        None | Some("68000") => Model::M68000,
        Some("68020") if !use_ir => Model::M68020,
        Some(c) => die(format!("--cpu {}: 68000 or 68020 (not with --ir)", c)),
    };
    let m020 = model == Model::M68020;
    // the longest 68000 instruction is 5 words, the longest 68020 one 11
    let nw = if m020 { 11 } else { 6 };
    let mut nfused = 0;
    let out = PathBuf::from(arg(args, "--out").expect("--out"));
    let mut rng = Rng::new(seed);
    std::fs::create_dir_all(out.join("src")).unwrap();
    std::fs::write(out.join("Cargo.toml"), format!(
        "[package]\nname = \"fuzz_gen\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nm68k = {{ package = \"m68k-rc\", path = \"{}\" }}\n",
        m68k_path().display())).unwrap();
    let empty = BTreeSet::new();
    let nofuncs: BTreeMap<u32, String> = BTreeMap::new();
    let nostubs: BTreeMap<u32, u32> = BTreeMap::new();
    let cx = Ctx { mode: Mode::Single, labels: &empty, funcs: &nofuncs, stub_base: None, stubs: &nostubs, cycles: false, model };
    let mut lib = String::from(HEADER);
    lib.push_str("use m68k::{Bus, Cpu};\nuse m68k::runtime::NativeFn;\n");
    lib.push_str(&format!("pub const BLOCK: usize = {};\n", block));
    let mut cases = String::from("pub const CASES: &[(u32, &[u16])] = &[\n");
    let mut table = String::from("pub fn case<B: Bus>(i: usize) -> NativeFn<B> {\n    match i {\n");
    let per = (500 / block).max(1);
    let mut part = String::new();
    for i in 0..n {
        let pc = rng.u32() & 0x00FF_FFFE;
        let mut words: Vec<u16> = Vec::new();
        let mut insns = Vec::new();
        let mut p = pc;
        for k in 0..block {
            let mut w = [0u16; 11];
            for x in w[..nw].iter_mut() {
                *x = rng.u32() as u16;
            }
            if m020 && rng.u32() & 1 == 0 {
                fuzz_020(&mut rng, &mut w);
            }
            let last = fuse && k + 1 == block;
            if last {
                let c = rng.u32() as u16;
                w[0] = if c & 0x100 == 0 {
                    // Bcc (not BRA/BSR) with a short forward displacement
                    0x6000 | (2 + (c >> 9) % 14) << 8 | (2 + (c & 0x3E))
                } else {
                    w[1] &= 0xFFFE;
                    0x50C8 | ((c >> 9) & 15) << 8 | (c & 7)
                };
            }
            if fuse && k + 2 == block && rng.u32() % 3 == 0 {
                // the edge cases of the conditions: equal operands, zero, -1, the sign boundary
                let (c, n) = (rng.u32(), rng.u32() as u16 & 7);
                let z = (c >> 8) as u16 % 3;
                w[0] = match c % 3 {
                    0 => 0xB000 | n << 9 | z << 6 | n, // cmp.<z> dn,dn
                    1 => 0x9000 | n << 9 | z << 6 | n, // sub.<z> dn,dn
                    _ => 0x7000 | n << 9 | [0x00, 0xFF, 0x01, 0x7F, 0x80][(c >> 16) as usize % 5], // moveq #k,dn
                };
            }
            for _ in 0..40 {
                if last {
                    break;
                }
                if fuse && k + 2 == block && rng.u32() & 1 == 0 {
                    // the instruction before the branch: one that sets flags a condition can use
                    let probe = decode::decode_model(p, &mut |a| w[((a.wrapping_sub(p)) / 2) as usize % nw], model);
                    if !matches!(probe.op, Op::Alu { .. } | Op::Move { .. } | Op::Tst { .. } | Op::Moveq { .. } | Op::Bit { .. } | Op::CmpA { .. } | Op::Cmpm { .. } | Op::Clr { .. }) {
                        w[0] = rng.u32() as u16;
                        continue;
                    }
                }
                let probe = decode::decode_model(p, &mut |a| w[((a.wrapping_sub(p)) / 2) as usize % nw], model);
                let control = recomp::emit::is_terminator(&probe.op)
                    || matches!(probe.op, Op::Bcc { .. } | Op::Dbcc { .. } | Op::Bsr { .. } | Op::Jsr { .. } | Op::Jmp { .. });
                let bad = matches!(probe.op, Op::Illegal | Op::LineA | Op::LineF) || (block > 1 && control);
                if !bad || (block == 1 && rng.u32() % 30 == 0) {
                    break;
                }
                w[0] = rng.u32() as u16;
                if m020 && rng.u32() & 1 == 0 {
                    fuzz_020(&mut rng, &mut w);
                }
            }
            if w[0] & 0xFFF8 == 0xF620 {
                w[0] = 0x4E71; // Musashi executes MOVE16 in 68000 mode (oracle bug), skip
            }
            let ins = decode::decode_model(p, &mut |a| w[((a.wrapping_sub(p)) / 2) as usize % nw], model);
            words.extend_from_slice(&w[..(ins.len / 2) as usize]);
            insns.push(ins);
            p = ins.next();
        }
        // pad so the checker can always place 6 words after the last instruction
        for _ in 0..5 {
            words.push(rng.u32() as u16);
        }
        if block == 1 {
            let ins = insns[0];
            part.push_str(&format!("#[inline(never)]\npub fn c{}<B: Bus>(cpu: &mut Cpu, bus: &mut B) {{\n", i));
            if use_ir {
                ir::emit::emit_single(&mut part, &ir::lift(&ins), &cx);
            } else if emit_instr(&mut part, &ins, &cx, 0, 0x1F) {
                part.push_str(&format!("        cpu.pc = 0x{:x};\n", ins.next()));
            }
            part.push_str("}\n");
        } else {
            let mut f = Func { entry: pc, ..Default::default() };
            f.leaders.insert(pc);
            for ins in &insns {
                f.insns.insert(ins.pc, *ins);
            }
            let code = if use_ir {
                let mut irf = ir::cfg::build(&f, &[]);
                if fuse {
                    let b = irf.blocks.get_mut(&pc).unwrap();
                    ir::cfg::block_liveness(b, 0);
                    nfused += b.insns.iter().filter(|i| i.fused.is_some()).count();
                }
                ir::emit::emit_func(&irf, Mode::Func { direct: false }, &nofuncs, &nostubs, &Options::default(), None, 0)
            } else {
                emit_func(&f, Mode::Func { direct: false }, &nofuncs, &nostubs, &Options { model, ..Options::default() }, None)
            };
            part.push_str("#[inline(never)]\n");
            if fuse {
                part.push_str(&code.replace(&format!("pub fn {}<", fn_name(pc)), &format!("pub fn c{}_f<", i)));
                part.push_str(&format!("#[inline(never)]\npub fn c{i}<B: Bus>(cpu: &mut Cpu, bus: &mut B) {{ let b = cpu.budget; cpu.budget = cpu.ticks + 1; c{i}_f(cpu, bus); cpu.budget = b; }}\n", i = i));
            } else {
                part.push_str(&code.replace(&format!("pub fn {}<", fn_name(pc)), &format!("pub fn c{}<", i)));
            }
        }
        cases.push_str(&format!("    (0x{:x}, &{:?}),\n", pc, words));
        table.push_str(&format!("        {} => p{}::c{}::<B>,\n", i, i / per, i));
        if (i + 1) % per == 0 || i + 1 == n {
            let mut s = String::from(HEADER);
            s.push_str("use m68k::{alu, Bus, Cpu, Sz, ShiftKind};\n\n");
            s.push_str(&part);
            std::fs::write(out.join("src").join(format!("p{}.rs", i / per)), s).unwrap();
            lib.push_str(&format!("mod p{};\n", i / per));
            part.clear();
        }
    }
    cases.push_str("];\n");
    table.push_str("        _ => panic!(),\n    }\n}\n");
    lib.push_str(&cases);
    lib.push_str(&table);
    std::fs::write(out.join("src/lib.rs"), lib).unwrap();
    println!("fuzz-gen: {} cases of {} instruction(s) -> {}{}", n, block, out.display(),
        if fuse { format!(" (from the IR, {} fused conditions)", nfused) } else if use_ir { " (from the IR)".into() } else { String::new() });
}

/// `--ir-split N`: the largest function the IR emitter writes in one piece (0: no limit).
fn ir_split(args: &[String], cfg: Option<usize>) -> usize {
    arg(args, "--ir-split").map(|s| s.parse().unwrap_or_else(|_| die(format!("--ir-split {:?}: not a number", s)))).or(cfg).unwrap_or(IR_SPLIT)
}

/// Default of `--ir-split`.
const IR_SPLIT: usize = 120;

fn die(msg: String) -> ! {
    eprintln!("m68k-recomp: {}", msg);
    std::process::exit(1);
}

/// A program ready for discovery: image, seeds, options, and everything needed to emit it.
struct Job {
    img: Image,
    bases: Vec<u32>,
    seeds: Vec<u32>,
    opts: Options,
    names: BTreeMap<u32, String>,
    out: Option<PathBuf>,
    name: String,
    flat: bool,
    extra: String,
    coverage: Vec<String>,
    pcrel_seeds: bool,
    gap_seeds: bool,
    /// Compile through the IR (`--ir` or `ir = true`).
    ir: bool,
    ir_split: Option<usize>,
    /// Entries left to the interpreter (map `KeepInterpreted` tags and the config list).
    keep: BTreeSet<u32>,
    /// Entries that check their code at entry (IR only).
    guarded: BTreeSet<u32>,
    /// Entry -> map name (doc alias).
    aliases: BTreeMap<u32, String>,
    /// Emit the code-unit tables (raw `--unit`).
    unit: bool,
    /// `--static-seeds [FILE]`: the static code search (`recomp::scan`) adds seeds; FILE gets the list.
    static_seeds: Option<Option<String>>,
    /// `--static-only`: the crate keeps only the functions the static search added (an isolated unit).
    static_only: bool,
    /// `--stable-with DUMP`: other memory images of the same run; bytes of the slice that differ in
    /// any of them are not used by the static search.
    stable_with: Vec<String>,
    /// Readable function names (`--names` without a file, or `names = true`).
    readable: bool,
}

fn range(s: &str, bases: &[u32]) -> (u32, u32) {
    let (a, b) = s.split_once('-').unwrap_or_else(|| die(format!("bad range {:?} (START-END)", s)));
    let r = |x: &str| config::parse_addr(x, bases).unwrap_or_else(|e| die(e));
    (r(a), r(b))
}

/// Builds the job from `rest` (the arguments after the subcommand) and an optional config.
fn build_job(rest: &[String], raw_cmd: bool) -> Job {
    let cfg = match arg(rest, "--config") {
        Some(p) => Config::load(Path::new(&p)).unwrap_or_else(|e| die(e)),
        None => Config::default(),
    };
    let raw = raw_cmd || cfg.input.kind.as_deref() == Some("raw");
    if let Some(k) = cfg.input.kind.as_deref() {
        if k != "raw" && k != "hunk" {
            die(format!("input.kind must be \"hunk\" or \"raw\", not {:?}", k));
        }
    }
    let positional = rest.first().filter(|a| !a.starts_with("--")).cloned();
    // Input: CLI --adf > CLI file > config adf+path > config file.
    let bytes = if let Some(spec) = arg(rest, "--adf") {
        let (adf, path) = spec.split_once(':').unwrap_or_else(|| die("--adf ADF:PATH".into()));
        read_adf(adf, path)
    } else if let Some(f) = &positional {
        std::fs::read(f).unwrap_or_else(|e| die(format!("{}: {}", f, e)))
    } else if let Some(adf) = &cfg.input.adf {
        read_adf(adf, cfg.input.path.as_deref().unwrap_or_else(|| die("input.adf needs input.path".into())))
    } else if let Some(f) = &cfg.input.file {
        std::fs::read(f).unwrap_or_else(|e| die(format!("{}: {}", f, e)))
    } else {
        die("no input (FILE, --adf ADF:PATH, or [input] in --config)".into())
    };
    let mut base = match arg(rest, "--base") {
        Some(b) => Some(hexu(&b)),
        None => cfg.base.as_ref().map(|a| a.resolve(&[]).unwrap_or_else(|e| die(e))),
    };
    // --slice LO-HI: the input is a memory image; its bytes [LO, HI) are placed at LO (raw only)
    let mut bytes = bytes;
    if let Some(s) = arg(rest, "--slice") {
        let (lo, hi) = range(&s, &[]);
        if !raw || base.is_some() || lo >= hi || hi as usize > bytes.len() {
            die(format!("--slice {}: raw input only, without --base, inside the {}-byte file", s, bytes.len()));
        }
        bytes = bytes[lo as usize..hi as usize].to_vec();
        base = Some(lo);
    }
    let profile = match arg(rest, "--profile") {
        Some(p) => Profile::parse(&p).unwrap_or_else(|| die(format!("unknown profile {:?} (generic, asm, manx, sasc)", p))),
        None if rest.iter().any(|a| a == "--manx") => Profile::Manx,
        None => match &cfg.profile {
            Some(p) => Profile::parse(p).unwrap_or_else(|| die(format!("unknown profile {:?} (generic, asm, manx, sasc)", p))),
            None => Profile::Generic,
        },
    };
    let no_flag_elim = rest.iter().any(|a| a == "--no-flag-elim") || cfg.no_flag_elim.unwrap_or(false);
    let cycles = rest.iter().any(|a| a == "--cycles") || cfg.cycles.unwrap_or(false);
    let model = match arg(rest, "--cpu").as_deref() {
        None | Some("68000") => Model::M68000,
        Some("68020") => Model::M68020,
        Some(c) => die(format!("--cpu {}: 68000 or 68020", c)),
    };
    if model == Model::M68020 && (rest.iter().any(|a| a == "--ir") || cfg.ir.unwrap_or(false)) {
        die("--ir: the IR is 68000-only (no --cpu 68020)".into());
    }
    let mut img = Image { model, ..Image::default() };
    let bases: Vec<u32>;
    let mut seeds: Vec<u32>;
    let mut opts;
    let mut extra = String::new();
    let mut ov_data: Vec<(u32, u32)> = vec![];
    let mut ov_seeds: Vec<u32> = vec![];
    if raw {
        let base = base.unwrap_or_else(|| die("raw input needs --base".into()));
        bases = vec![base];
        img.segs.push(Segment { name: "raw".into(), base, data: bytes, code: true });
        let cli: Vec<u32> = args_all(rest, "--entry").iter().map(|s| hexu(s)).collect();
        seeds = if cli.is_empty() { cfg.entries.iter().map(|a| a.resolve(&bases).unwrap_or_else(|e| die(e))).collect() } else { cli };
        opts = Options { profile, no_flag_elim, cycles, model, ..Options::default() };
    } else {
        let h = amiga::hunk::parse(&bytes).unwrap_or_else(|e| die(e));
        bases = if !cfg.bases.is_empty() && arg(rest, "--base").is_none() {
            // explicit placement of every section (e.g. a program set sharing an address space)
            let bs: Vec<u32> = cfg.bases.iter().map(|a| a.resolve(&[]).unwrap_or_else(|e| die(e))).collect();
            if bs.len() != h.sections.len() {
                die(format!("`bases` lists {} addresses, the file has {} sections", bs.len(), h.sections.len()));
            }
            bs
        } else {
            // From `base`, aligned; `hunk_gap` bytes stay free after each section for the next
            // section's LoadSeg header (size, next), which the runtime puts just before it.
            let base = base.unwrap_or(0x20000);
            let align = cfg.align.unwrap_or(0x100).max(4);
            if !align.is_power_of_two() {
                die(format!("align {:#x} is not a power of two", align));
            }
            let gap = cfg.hunk_gap.unwrap_or(8);
            let mut bs = Vec::new();
            let mut p = base;
            for s in &h.sections {
                bs.push(p);
                p = (p + s.alloc.max(4) + gap + align - 1) & !(align - 1);
            }
            bs
        };
        if let Some(ov) = &h.overlay {
            eprintln!("note: overlaid executable: {} root sections, {} overlay nodes (sections {}); every section gets its own base",
                h.root, ov.nodes.len(), ov.nodes.iter().map(|n| format!("{}-{}", n.1, n.2)).collect::<Vec<_>>().join(", "));
        }
        if cfg.overlay_stubs.unwrap_or(false) {
            match overlay_stubs(&h) {
                Ok((range, entries)) => {
                    eprintln!("note: {} overlay stubs at {}:{:05x}-{:05x} (kept as data: the supervisor rewrites them), {} overlay entry seeds",
                        entries.len(), range.0, range.1, range.2, entries.len());
                    let sb = bases[range.0];
                    ov_data.push((sb + range.1, sb + range.2));
                    ov_seeds.extend(entries.iter().map(|&(hk, off)| bases[hk] + off));
                }
                Err(e) => die(format!("overlay_stubs: {}", e)),
            }
        }
        let imgs = h.relocate(&bases);
        for (i, s) in h.sections.iter().enumerate() {
            img.segs.push(Segment { name: format!("{}", i), base: bases[i], data: imgs[i].clone(), code: matches!(s.kind, amiga::hunk::Kind::Code) });
        }
        for (at, _, _) in h.reloc_targets(&bases) {
            img.relocs.insert(at);
        }
        img.has_relocs = true;
        let data_idx = h.sections.iter().position(|s| matches!(s.kind, amiga::hunk::Kind::Data));
        let base_reg = match (profile.base_reg(), data_idx) {
            (Some((reg, off)), Some(i)) => {
                let default = bases[i] + off;
                let v = match profile.startup_base(&img, bases[0]) {
                    Some(v) => {
                        if v != default {
                            eprintln!("note: startup code loads a{} = ${:x}, not data+${:x} = ${:x}; using the startup value", reg, v, off, default);
                        }
                        v
                    }
                    None => default,
                };
                Some((reg, v))
            }
            _ => None,
        };
        opts = Options { profile, base_reg, reloc_seeds: true, no_flag_elim, cycles, model, ..Options::default() };
        seeds = vec![bases[0]];
        extra.push_str(&format!("/// Load address of each hunk section.\npub const BASES: &[u32] = &{:?};\n", bases));
        let a4 = base_reg.map(|(_, v)| v);
        if profile == Profile::Manx {
            extra.push_str(&format!("/// A4 assumed by guarded stub calls (Manx small-data base).\npub const A4: Option<u32> = {:?};\n", a4));
        } else {
            extra.push_str(&format!("/// Small-data base register value assumed by the {} profile, if any.\npub const A4: Option<u32> = {:?};\n", profile.name(), a4));
        }
    }
    // Ranges first: they affect what counts as code for everything below.
    img.data_ranges.extend(ov_data.iter().copied());
    seeds.extend(ov_seeds.iter().copied());
    let res = |a: &Addr| a.resolve(&bases).unwrap_or_else(|e| die(e));
    for [a, b] in &cfg.data {
        img.data_ranges.push((res(a), res(b)));
    }
    for [a, b] in &cfg.exclude {
        img.exclude_ranges.push((res(a), res(b)));
    }
    for s in args_all(rest, "--data") {
        img.data_ranges.push(range(&s, &bases));
    }
    for s in args_all(rest, "--exclude") {
        img.exclude_ranges.push(range(&s, &bases));
    }
    // Seeds: config list, config files, then CLI file.
    for a in &cfg.seeds {
        seeds.push(res(a));
    }
    let mut seed_files = cfg.seeds_files.clone();
    seed_files.extend(args_all(rest, "--seeds"));
    for f in &seed_files {
        seeds.extend(config::read_seeds(f, &bases).unwrap_or_else(|e| die(e)));
    }
    let mut names = BTreeMap::new();
    for f in &cfg.names_files {
        config::read_names(f, &bases, &mut names).unwrap_or_else(|e| die(e));
    }
    for (a, n) in &cfg.names.table() {
        names.insert(config::parse_addr(a, &bases).unwrap_or_else(|e| die(e)), n.clone());
    }
    for f in args_all(rest, "--names").into_iter().filter(|f| !f.starts_with("--")) {
        config::read_names(&f, &bases, &mut names).unwrap_or_else(|e| die(e));
    }
    // Annotations: RR notes and game maps give names (doc comments) and tags.
    let mut sets: Vec<annot::AnnotSet> = Vec::new();
    for f in &cfg.rr_notes {
        sets.push(annot::load_rr_notes(Path::new(f), "rr", &bases).unwrap_or_else(|e| die(e)));
    }
    for m in &cfg.maps {
        let p = Path::new(&m.file);
        let pick = |v: Vec<annot::AnnotSet>| -> Vec<annot::AnnotSet> {
            match &m.program {
                Some(n) => v.into_iter().filter(|s| &s.program == n).collect(),
                None => v,
            }
        };
        let mut got = match m.format.as_str() {
            "wings" => vec![annot::load_wings(p).unwrap_or_else(|e| die(e))],
            "dotc" => pick(annot::load_dotc(p).unwrap_or_else(|e| die(e))),
            "sinbad" => pick(annot::load_sinbad(p, None).unwrap_or_else(|e| die(e))),
            o => die(format!("map.format must be wings, dotc or sinbad, not {:?}", o)),
        };
        if got.is_empty() {
            die(format!("{}: no program {:?} in the map", m.file, m.program));
        }
        for g in &mut got {
            // Map offsets are relative to the map's own layout; a map with no absolute
            // addresses is placed on this image's section bases.
            g.rebase(&bases);
        }
        sets.extend(got);
    }
    let mut keep = BTreeSet::new();
    let mut guarded = BTreeSet::new();
    let mut aliases = BTreeMap::new();
    for set in &sets {
        for (a, n) in set.names() {
            names.entry(a).or_insert(n);
        }
        for (a, n) in set.aliases() {
            aliases.entry(a).or_insert(n);
        }
        keep.extend(set.entries_tagged(&annot::Tag::KeepInterpreted));
        guarded.extend(set.entries_tagged(&annot::Tag::Guarded));
    }
    for a in &cfg.keep_interpreted {
        keep.insert(res(a));
    }
    for a in &cfg.guarded {
        guarded.insert(res(a));
    }
    for h in &cfg.hints {
        let (at, ts) = h.resolve(&img, &bases).unwrap_or_else(|e| die(e));
        opts.hints.entry(at).or_default().extend(ts);
    }
    let mut coverage = cfg.coverage.clone();
    coverage.extend(args_all(rest, "--coverage"));
    Job {
        img,
        bases,
        seeds,
        opts,
        names,
        out: arg(rest, "--out").map(PathBuf::from).or(cfg.out.map(PathBuf::from)),
        name: arg(rest, "--name").or(cfg.name).unwrap_or_else(|| "recompiled".into()),
        flat: rest.iter().any(|a| a == "--flat") || cfg.flat.unwrap_or(false),
        extra,
        coverage,
        pcrel_seeds: rest.iter().any(|a| a == "--pcrel-seeds") || cfg.pcrel_seeds.unwrap_or(false),
        gap_seeds: rest.iter().any(|a| a == "--gap-seeds") || cfg.gap_seeds.unwrap_or(false),
        ir: rest.iter().any(|a| a == "--ir") || cfg.ir.unwrap_or(false),
        ir_split: cfg.ir_split,
        keep,
        guarded,
        aliases,
        unit: raw && rest.iter().any(|a| a == "--unit"),
        readable: cfg.names.enabled() || rest.iter().enumerate().any(|(i, a)| a == "--names" && rest.get(i + 1).map_or(true, |n| n.starts_with("--"))),
        static_only: rest.iter().any(|a| a == "--static-only"),
        stable_with: args_all(rest, "--stable-with"),
        static_seeds: rest.iter().position(|a| a == "--static-seeds").map(|i| rest.get(i + 1).filter(|v| !v.starts_with("--")).cloned()),
    }
}

/// The unloaded overlay stubs of an overlaid executable and the overlay entry points they name.
/// Layout as found in Wings' standard overlay supervisor (UNSURE for other linkers): the overlay
/// table is `count, {node file offset, (first stub offset << 16 | hunk list offset)} x count, ...`;
/// a node's hunk list (at byte `4 + offset` of the table) is `{hunk.w, number of stubs.w}* 0.w`;
/// the stubs are 8 bytes each in the first data section from the node's first stub offset:
/// `bsr.w supervisor ; dc.b node ; dc.b offset[3]` (offset into that hunk). Returns ((section,
/// first stub, end of the last stub), [(hunk, offset)]).
fn overlay_stubs(h: &amiga::hunk::Hunk) -> Result<((usize, u32, u32), Vec<(usize, u32)>), String> {
    let ov = h.overlay.as_ref().ok_or("not an overlaid executable")?;
    let raw: Vec<u8> = ov.table.iter().flat_map(|x| x.to_be_bytes()).collect();
    let w = |o: usize| raw.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as u32).ok_or(format!("table offset {} out of range", o));
    let ds = h.sections.iter().position(|s| s.kind == amiga::hunk::Kind::Data).ok_or("no data section")?;
    let d = &h.sections[ds].data;
    let count = *ov.table.first().ok_or("empty overlay table")? as usize;
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    let mut out = vec![];
    for n in 0..count {
        let x = *ov.table.get(2 + 2 * n).ok_or("overlay table too short")?;
        let (mut a, mut q) = ((x >> 16) as usize, 4 + (x & 0xFFFF) as usize);
        loop {
            let hk = w(q)? as usize;
            if hk == 0 {
                break;
            }
            let ns = w(q + 2)?;
            q += 4;
            if hk >= h.sections.len() {
                return Err(format!("node {}: hunk {} does not exist", n + 1, hk));
            }
            for _ in 0..ns {
                let st = d.get(a..a + 8).ok_or(format!("stub at {}:{:05x} outside the data", ds, a))?;
                if st[0..2] != [0x61, 0x00] {
                    return Err(format!("stub at {}:{:05x} is not bsr.w", ds, a));
                }
                let off = u32::from_be_bytes([0, st[5], st[6], st[7]]);
                out.push((hk, off));
                lo = lo.min(a as u32);
                hi = hi.max(a as u32 + 8);
                a += 8;
            }
        }
    }
    Ok(((ds, lo, hi), out))
}

fn read_adf(adf: &str, path: &str) -> Vec<u8> {
    let img = std::fs::read(adf).unwrap_or_else(|e| die(format!("{}: {}", adf, e)));
    amiga::adf::Adf::new(&img).and_then(|a| a.read(path)).unwrap_or_else(|e| die(format!("{}:{}: {}", adf, path, e)))
}

fn totals(funcs: &BTreeMap<u32, Func>) -> (usize, usize, usize, usize) {
    let mut entries = BTreeSet::new();
    let (mut ins, mut blocks) = (0, 0);
    for f in funcs.values() {
        ins += f.insns.len();
        blocks += f.leaders.len();
        entries.insert(f.entry);
        entries.extend(f.leaders.iter().copied().filter(|l| f.insns.contains_key(l)));
    }
    (funcs.len(), ins, blocks, entries.len())
}

/// Discovery (see [`run_discovery_base`]), then the static code search when `--static-seeds` is given.
fn run_discovery(job: &mut Job) -> BTreeMap<u32, Func> {
    let funcs = run_discovery_base(job);
    match job.static_seeds.clone() {
        Some(out) => static_search(job, funcs, out),
        None => funcs,
    }
}

/// `--static-seeds`: the static code search (`recomp::scan`) on top of everything discovery and the
/// coverage found, so it only adds.
fn static_search(job: &mut Job, mut funcs: BTreeMap<u32, Func>, out: Option<String>) -> BTreeMap<u32, Func> {
    let mut so = recomp::scan::ScanOptions::default();
    if !job.stable_with.is_empty() {
        // bytes of the slice that another image of the run has differently
        let seg = &job.img.segs[0];
        let (lo, n) = (seg.base as usize, seg.data.len());
        let mut diff = vec![false; n];
        for f in &job.stable_with {
            let m = std::fs::read(f).unwrap_or_else(|e| die(format!("{}: {}", f, e)));
            for (i, d) in diff.iter_mut().enumerate() {
                *d |= m.get(lo + i).map_or(false, |&b| b != seg.data[i]);
            }
        }
        let mut i = 0;
        while i < n {
            if diff[i] {
                let j = (i..n).find(|&k| !diff[k]).unwrap_or(n);
                so.volatile.push((seg.base + i as u32, seg.base + j as u32));
                i = j;
            } else {
                i += 1;
            }
        }
        println!("static: {} bytes in {} ranges differ in {} other images and are left alone", diff.iter().filter(|&&d| d).count(), so.volatile.len(), job.stable_with.len());
    }
    let rep = recomp::scan::scan(&job.img, &job.seeds, &job.opts, &so);
    let (f0, i0, ..) = totals(&funcs);
    let base: BTreeSet<u32> = funcs.keys().copied().collect();
    job.seeds.extend(rep.accepted.iter().map(|c| c.addr));
    funcs = discover(&job.img, &job.seeds, &job.opts);
    let (f1, i1, ..) = totals(&funcs);
    use recomp::scan::Via;
    println!(
        "static: {} seeds in {} passes ({} table, {} operand, {} gap, {} adjacent), +{} functions, +{} instructions ({} -> {} functions); turned down: {:?}",
        rep.accepted.len(), rep.passes, rep.count(Via::Table), rep.count(Via::Operand), rep.count(Via::Gap), rep.count(Via::Adjacent),
        f1 - f0, i1 as isize - i0 as isize, f0, f1, rep.rejected
    );
    if let Some(path) = out {
        let mut t = String::from("# address via instructions (recomp::scan)\n");
        for c in &rep.accepted {
            t.push_str(&format!("{:06x} {} {}\n", c.addr, c.via.name(), c.insns));
        }
        std::fs::write(&path, t).unwrap_or_else(|e| die(format!("{}: {}", path, e)));
    }
    if job.static_only {
        // the unit holds only what the search added: it can go invalid (bytes the game writes) without
        // taking the run-seeded unit with it
        funcs.retain(|e, _| !base.contains(e));
        println!("static: --static-only keeps {} functions", funcs.len());
    }
    funcs
}

/// Discovery, with the optional pc-relative/gap seeding to a fixed point and the coverage
/// feedback applied when there is any.
fn run_discovery_base(job: &mut Job) -> BTreeMap<u32, Func> {
    let mut funcs = discover(&job.img, &job.seeds, &job.opts);
    for pass in 0..16 {
        let mut extra = Vec::new();
        if job.pcrel_seeds {
            extra.extend(recomp::pcrel_seeds(&job.img, &funcs));
        }
        if job.gap_seeds && extra.is_empty() {
            let g = recomp::gap_seeds(&job.img, &funcs);
            for &a in &g {
                job.names.entry(a).or_insert_with(|| format!("unreferenced {}", fn_name(a)));
            }
            extra.extend(g);
        }
        if extra.is_empty() {
            break;
        }
        let (f0, ..) = totals(&funcs);
        job.seeds.extend(extra.iter().copied());
        funcs = discover(&job.img, &job.seeds, &job.opts);
        let (f1, ..) = totals(&funcs);
        eprintln!("seeds: pass {}: {} pc-relative/gap seeds, {} -> {} functions", pass + 1, extra.len(), f0, f1);
    }
    if job.coverage.is_empty() {
        return funcs;
    }
    let mut hits = Vec::new();
    for f in &job.coverage {
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| die(format!("{}: {}", f, e)));
        hits.extend(recomp::coverage::parse(&text).unwrap_or_else(|e| die(format!("{}: {}", f, e))));
    }
    let plan = recomp::coverage::plan(&job.img, &hits, &funcs);
    let njumps = plan.targets.len();
    let ntargets: usize = plan.targets.values().map(|v| v.len()).sum();
    job.seeds.extend(plan.seeds.iter().copied());
    for (at, ts) in &plan.targets {
        let e = job.opts.hints.entry(*at).or_default();
        for t in ts {
            if !e.contains(t) {
                e.push(*t);
            }
        }
    }
    let after = discover(&job.img, &job.seeds, &job.opts);
    let (f0, i0, b0, e0) = totals(&funcs);
    let (f1, i1, b1, e1) = totals(&after);
    println!(
        "coverage: {} addresses ({} duplicates): {} accepted ({} new seeds, {} targets on {} computed jumps/calls), {} already compiled, {} rejected",
        hits.len(), plan.duplicates, plan.accepted, plan.seeds.len(), ntargets, njumps, plan.known, plan.rejected.len()
    );
    for (a, why) in plan.rejected.iter().take(20) {
        println!("coverage:   rejected {:06x}: {}", a, why);
    }
    if plan.rejected.len() > 20 {
        println!("coverage:   ... {} more rejected", plan.rejected.len() - 20);
    }
    println!(
        "coverage: +{} functions, +{} blocks, +{} instructions, +{} dispatch entries ({} -> {} functions, {} -> {} blocks)",
        f1 - f0, b1 as isize - b0 as isize, i1 as isize - i0 as isize, e1 as isize - e0 as isize, f0, f1, b0, b1
    );
    after
}

fn recompile(rest: &[String], raw: bool) {
    let mut job = build_job(rest, raw);
    let out = job.out.clone().unwrap_or_else(|| die("--out (or `out` in the config) is required".into()));
    let t0 = std::time::Instant::now();
    let given = job.names.clone();
    let mut funcs = run_discovery(&mut job);
    // KeepInterpreted: the function stays out of the generated crate (the dispatcher finds no
    // native entry for it and interprets it).
    let kept: Vec<u32> = job.keep.iter().copied().filter(|a| funcs.remove(a).is_some()).collect();
    if !job.keep.is_empty() {
        println!("annotations: {} functions kept on the interpreter path ({} tagged)", kept.len(), job.keep.len());
    }
    let stubs = if job.opts.profile.data_stubs() { find_stubs(&job.img) } else { BTreeMap::new() };
    if job.unit {
        unit_tables(&mut job, &funcs, &out);
    }
    let mode = Mode::Func { direct: !job.flat };
    let naming = job.readable.then(|| {
        let raw = !job.img.has_relocs && job.bases.len() == 1;
        let addr = |a: u32| match job.img.segs.iter().position(|s| a >= s.base && a < s.base + s.data.len() as u32) {
            Some(i) if !raw => format!("{}:{:05x}", i, a - job.bases[i]),
            _ => format!("${:06x}", a),
        };
        let g = recomp::names::Given { aliases: &job.aliases, descriptions: &given, addr: &addr, crate_name: &job.name };
        let n = recomp::names::derive(&funcs, &job.img, job.opts.base_reg, &g);
        println!("names: {} of {} functions named ({}); {}", n.derived(), funcs.len(),
            n.by_source().iter().filter(|(s, _)| s.as_str() != "none").map(|(s, k)| format!("{} {}", k, s)).collect::<Vec<_>>().join(", "), out.join("names.txt").display());
        n
    });
    let st = if job.ir {
        let prog = ir::cfg::build_program(&funcs, &job.img);
        let s = ir::cfg::lift_stats(&prog);
        let split = ir_split(rest, job.ir_split);
        let parts: usize = prog.funcs.values().map(|f| ir::emit::partition(f, split).len()).filter(|&n| n > 1).sum();
        println!("ir: {} instructions, all lifted; lazy flags: {} computed, {} behind the budget test, {} eliminated; split at {} instructions: {} functions in {} parts",
            s.instructions, s.exact, s.guarded, s.plain, split,
            prog.funcs.values().filter(|f| ir::emit::partition(f, split).len() > 1).count(), parts);
        // Guarded entries check every code word of the function at entry.
        let mut extras = ir::emit::Extras { aliases: job.aliases.clone(), naming: naming.clone(), ..Default::default() };
        for (&e, f) in &funcs {
            if job.guarded.contains(&e) {
                let words: Vec<(u32, u16)> = f.insns.values().flat_map(|i| (0..i.len / 2).map(move |k| i.pc + 2 * k)).filter_map(|a| job.img.word(a).map(|w| (a, w))).collect();
                extras.guards.insert(e, words);
            }
        }
        if !extras.guards.is_empty() {
            println!("annotations: {} functions guarded", extras.guards.len());
        }
        ir::emit::emit_crate_with(&out, &job.name, &m68k_path(), &prog, mode, &job.opts, &stubs, &job.names, &job.extra, split, &extras)
    } else {
        emit_crate_named(&out, &job.name, &m68k_path(), &funcs, mode, &job.opts, &stubs, &job.names, &job.extra, naming.as_ref())
    }
    .unwrap_or_else(|e| die(format!("{}: {}", out.display(), e)));
    println!("{}: {} functions, {} instructions, {} blocks, {} dispatch entries, {} jump tables, {} unresolved computed jumps, {} computed calls, {} data stubs; mode {:?}; profile {}; {:.2?}",
        job.name, st.functions, st.instructions, st.blocks, st.entries, st.jump_tables, st.dyn_jumps, st.dyn_calls, stubs.len(), mode, job.opts.profile.name(), t0.elapsed());
}

/// `--unit`: the image, the compiled instructions' byte ranges and their hash, for a dispatcher that
/// runs the crate only while those bytes are in memory (amiga-machine `units`).
fn unit_tables(job: &mut Job, funcs: &BTreeMap<u32, Func>, out: &Path) {
    use std::fmt::Write as _;
    let seg = &job.img.segs[0];
    let mut code: Vec<(u32, u32)> = vec![];
    let mut ins: Vec<(u32, u32)> = funcs.values().flat_map(|f| f.insns.values().map(|i| (i.pc, i.next()))).collect();
    ins.sort_unstable();
    for (a, b) in ins {
        match code.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => code.push((a, b)),
        }
    }
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &(a, b) in &code {
        for x in &seg.data[(a - seg.base) as usize..(b - seg.base) as usize] {
            h = (h ^ *x as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    std::fs::create_dir_all(out).unwrap_or_else(|e| die(format!("{}: {}", out.display(), e)));
    std::fs::write(out.join("image.bin"), &seg.data).unwrap_or_else(|e| die(format!("{}: {}", out.display(), e)));
    let bytes: u32 = code.iter().map(|(a, b)| b - a).sum();
    let _ = write!(job.extra, "/// Code unit (`m68k-recomp raw --unit`): the memory it was compiled from, at BASE.\npub static IMAGE: &[u8] = include_bytes!(\"../image.bin\");\npub const BASE: u32 = 0x{:x};\n", seg.base);
    let _ = write!(job.extra, "/// The compiled instructions' bytes ({} in {} ranges, half-open).\npub const CODE: &[(u32, u32)] = &[", bytes, code.len());
    for (a, b) in &code {
        let _ = write!(job.extra, "(0x{:x}, 0x{:x}), ", a, b);
    }
    let _ = write!(job.extra, "];\n/// FNV-1a of the bytes of CODE.\npub const HASH: u64 = 0x{:016x};\n", h);
    println!("{}: unit ${:06x}-${:06x}, {} code bytes in {} ranges, hash {:016x}", job.name, seg.base, seg.base + seg.data.len() as u32, bytes, code.len(), h);
}

/// The address in the most readable form: `sec:offset` for hunk files.
fn where_(job: &Job, a: u32) -> String {
    match job.img.segs.iter().position(|s| a >= s.base && a < s.base + s.data.len() as u32) {
        Some(i) if job.bases.len() > 1 || job.img.has_relocs => format!("${:06x} ({}:{:05x})", a, i, a - job.bases[i]),
        _ => format!("${:06x}", a),
    }
}

fn report(rest: &[String]) {
    let (raw, rest) = match rest.first().map(|s| s.as_str()) {
        Some("raw") => (true, &rest[1..]),
        Some("hunk") => (false, &rest[1..]),
        _ => (false, rest),
    };
    let all = rest.iter().any(|a| a == "--all");
    let mut job = build_job(rest, raw);
    if let Some(at) = arg(rest, "--at") {
        // Plain disassembly listing: --at ADDR [--count N]
        let mut p = config::parse_addr(&at, &job.bases).unwrap_or_else(|e| die(e));
        let n: usize = arg(rest, "--count").map(|s| s.parse().unwrap()).unwrap_or(32);
        for _ in 0..n {
            match job.img.decode(p) {
                Some(i) => {
                    println!("  {}  {}", where_(&job, p), i);
                    p = i.next();
                }
                None => {
                    println!("  {}  (not decodable)", where_(&job, p));
                    break;
                }
            }
        }
        return;
    }
    let funcs = run_discovery(&mut job);
    let (nf, ni, nb, ne) = totals(&funcs);
    println!("report: profile {}, {} functions, {} instructions, {} blocks, {} dispatch entries", job.opts.profile.name(), nf, ni, nb, ne);
    if let Some((r, v)) = job.opts.base_reg {
        println!("report: base register a{} = ${:x}", r, v);
    }
    let fname = |e: u32| match job.names.get(&e) {
        Some(n) => format!("{} \"{}\"", fn_name(e), n.split(" (").next().unwrap_or(n)),
        None => fn_name(e),
    };
    let ctx = |f: &Func, at: u32| {
        // up to 3 instructions falling through to `at`
        let mut v = Vec::new();
        let mut q = at;
        for _ in 0..3 {
            match f.insns.values().find(|i| i.next() == q && i.pc != q && !recomp::emit::is_terminator(&i.op)) {
                Some(i) => {
                    v.push(*i);
                    q = i.pc;
                }
                None => break,
            }
        }
        v.reverse();
        v
    };
    let show = |f: &Func, at: u32| {
        for i in ctx(f, at) {
            println!("        {:06x}  {}", i.pc, i);
        }
        println!("     -> {:06x}  {}", at, f.insns[&at]);
    };
    // Unresolved computed jumps: these are the ones to hint.
    let mut jumps: BTreeMap<u32, &Func> = BTreeMap::new();
    let mut calls: BTreeMap<u32, &Func> = BTreeMap::new();
    let mut hinted: BTreeMap<u32, (usize, &Func)> = BTreeMap::new();
    let mut tables: BTreeMap<u32, (usize, &Func)> = BTreeMap::new();
    for f in funcs.values() {
        for &a in &f.unresolved_jumps {
            jumps.entry(a).or_insert(f);
        }
        for &a in &f.unresolved_calls {
            calls.entry(a).or_insert(f);
        }
        for &(a, n) in &f.hinted {
            hinted.entry(a).or_insert((n, f));
        }
        for (a, t) in &f.jump_tables {
            tables.entry(*a).or_insert((t.len(), f));
        }
    }
    let is_lib = |i: &Instr| matches!(i.op, Op::Jsr { ea: Ea::Disp(6, d) } | Op::Jmp { ea: Ea::Disp(6, d) } if d < 0);
    let jlib = jumps.iter().filter(|(a, f)| is_lib(&f.insns[a])).count();
    println!("\n== {} computed jumps without known targets ({} library tail calls jmp -d(a6){}) ==", jumps.len(), jlib, if all { "" } else { ", not listed; --all lists them" });
    for (&a, f) in &jumps {
        if !all && is_lib(&f.insns[&a]) {
            continue;
        }
        println!("  {} in {}", where_(&job, a), fname(f.entry));
        show(f, a);
    }
    let nlib = calls.iter().filter(|(a, f)| is_lib(&f.insns[a])).count();
    let mut forms: BTreeMap<String, usize> = BTreeMap::new();
    for (a, f) in &calls {
        let s = format!("{}", f.insns[a]);
        let form = s.split_whitespace().nth(1).unwrap_or("").to_string();
        let form = if is_lib(&f.insns[a]) { "-d(a6) (library)".to_string() } else { form };
        *forms.entry(form).or_default() += 1;
    }
    println!("\n== {} computed calls without known targets ({} library calls jsr -d(a6){}) ==", calls.len(), nlib, if all { "" } else { ", not listed; --all lists them" });
    for (form, n) in &forms {
        println!("  {:5}  jsr {}", n, form);
    }
    for (&a, f) in &calls {
        if !all && is_lib(&f.insns[&a]) {
            continue;
        }
        println!("  {} in {}", where_(&job, a), fname(f.entry));
        show(f, a);
    }
    println!("\n== {} computed transfers resolved by hints or coverage ==", hinted.len());
    for (&a, &(n, f)) in &hinted {
        println!("  {} in {}: {} targets   {}", where_(&job, a), fname(f.entry), n, f.insns[&a]);
    }
    println!("\n== {} jump tables recognised by the {} profile or hints ==", tables.len(), job.opts.profile.name());
    for (&a, &(n, f)) in &tables {
        println!("  {} in {}: {} entries   {}", where_(&job, a), fname(f.entry), n, f.insns[&a]);
    }
    if let Some((&a, _)) = jumps.iter().find(|(a, f)| !is_lib(&f.insns[a])) {
        println!("\nTo resolve a jump, add to the config, e.g.:\n  [[hint]]\n  at = 0x{:x}\n  targets = [...]          # or: table = ..., count = N, format = \"rel16\"|\"abs32\"|\"code\"", a);
    }
}

/// `map`: one JSON object per line. First line: section bases, data stubs, names; then one line
/// per discovered function: entry, byte range, size, instruction/block counts, callees,
/// unresolved computed calls/jumps, jump tables (address, entries), `-d(a6)` library calls
/// (address, offset, jsr|jmp), and the instruction list (pc, length). For annotation tooling.
fn map_dump(rest: &[String]) {
    let mut job = build_job(rest, false);
    let funcs = run_discovery(&mut job);
    let stubs = if job.opts.profile.data_stubs() { find_stubs(&job.img) } else { BTreeMap::new() };
    let j = |v: &mut dyn Iterator<Item = u32>| v.map(|x| x.to_string()).collect::<Vec<_>>().join(",");
    let esc = |s: &str| s.chars().flat_map(|c| match c { '"' => vec!['\\', '"'], '\\' => vec!['\\', '\\'], c if (c as u32) < 0x20 => vec![' '], c => vec![c] }).collect::<String>();
    println!("{{\"bases\":[{}],\"profile\":\"{}\",\"stubs\":[{}],\"names\":{{{}}}}}", j(&mut job.bases.iter().copied()), job.opts.profile.name(),
        stubs.iter().map(|(a, t)| format!("[{},{}]", a, t)).collect::<Vec<_>>().join(","),
        job.names.iter().map(|(a, n)| format!("\"{}\":\"{}\"", a, esc(n))).collect::<Vec<_>>().join(","));
    for f in funcs.values() {
        let lo = *f.insns.keys().next().unwrap_or(&f.entry);
        let hi = f.insns.values().map(|i| i.next()).max().unwrap_or(f.entry);
        let bytes: u32 = f.insns.values().map(|i| i.len).sum();
        let a6: Vec<String> = f.insns.values().filter_map(|i| match i.op {
            Op::Jsr { ea: Ea::Disp(6, d) } => Some(format!("[{},{},\"jsr\"]", i.pc, d)),
            Op::Jmp { ea: Ea::Disp(6, d) } => Some(format!("[{},{},\"jmp\"]", i.pc, d)),
            _ => None,
        }).collect();
        let tables: Vec<String> = f.jump_tables.iter().map(|(a, t)| format!("[{},[{}]]", a, j(&mut t.iter().copied()))).collect();
        println!("{{\"entry\":{},\"lo\":{},\"hi\":{},\"bytes\":{},\"insns\":{},\"blocks\":{},\"callees\":[{}],\"ucalls\":[{}],\"ujumps\":[{}],\"tables\":[{}],\"a6\":[{}],\"pcs\":[{}]}}",
            f.entry, lo, hi, bytes, f.insns.len(), f.leaders.len(),
            j(&mut f.callees.iter().copied()), j(&mut f.unresolved_calls.iter().copied()), j(&mut f.unresolved_jumps.iter().copied()),
            tables.join(","), a6.join(","),
            f.insns.values().map(|i| format!("{},{}", i.pc, i.len)).collect::<Vec<_>>().join(","));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("fuzz-gen") => fuzz_gen(&args),
        Some("hunk") => recompile(&args[2..], false),
        Some("raw") => recompile(&args[2..], true),
        Some("report") => report(&args[2..]),
        Some("map") => map_dump(&args[2..]),
        _ => {
            eprintln!("usage: m68k-recomp fuzz-gen|hunk|raw|report|map ... (see src/main.rs)");
            std::process::exit(2);
        }
    }
}
