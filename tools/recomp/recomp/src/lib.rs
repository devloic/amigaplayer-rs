//! Static recompiler: 68000 machine code -> Rust source.
//!
//! Pipeline: [`Image`] (relocated sections) -> [`discover`] (functions, basic blocks, jump tables,
//! resume points) -> [`emit_crate`] (one Rust function per 68000 function, plus a dispatch table).
pub mod config;
pub mod coverage;
pub mod emit;
pub mod ir;
/// Structure offsets and LVO name tables, from m68k-recomp's `amiga-rt` (only `names` uses them).
pub mod layout;
pub mod names;
pub mod profile;
pub mod scan;

pub use profile::Profile;

use emit::{emit_instr, is_terminator, Ctx, Mode};
use m68k::decode::decode_model;
use m68k::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub struct Segment {
    pub name: String,
    pub base: u32,
    pub data: Vec<u8>,
    pub code: bool,
}

/// The program as loaded in memory.
#[derive(Default)]
pub struct Image {
    pub segs: Vec<Segment>,
    /// Addresses of relocated longs (from the hunk relocation tables).
    pub relocs: BTreeSet<u32>,
    /// True when `relocs` is authoritative (hunk files); false for raw images.
    pub has_relocs: bool,
    /// `[start, end)` ranges that hold data and must never be decoded as code.
    pub data_ranges: Vec<(u32, u32)>,
    /// `[start, end)` ranges of code that must not be compiled (left to the interpreter).
    pub exclude_ranges: Vec<(u32, u32)>,
    /// The CPU the code is decoded for (`--cpu`; must agree with [`Options::model`]).
    pub model: Model,
}

impl Image {
    pub fn seg(&self, a: u32) -> Option<&Segment> {
        self.segs.iter().find(|s| a >= s.base && a < s.base + s.data.len() as u32)
    }
    pub fn word(&self, a: u32) -> Option<u16> {
        let s = self.seg(a)?;
        let o = (a - s.base) as usize;
        if o + 2 > s.data.len() {
            return None;
        }
        Some(u16::from_be_bytes([s.data[o], s.data[o + 1]]))
    }
    pub fn long(&self, a: u32) -> Option<u32> {
        Some((self.word(a)? as u32) << 16 | self.word(a + 2)? as u32)
    }
    pub fn is_code(&self, a: u32) -> bool {
        self.seg(a).map_or(false, |s| s.code) && !in_ranges(&self.data_ranges, a)
    }
    pub fn is_data_range(&self, a: u32) -> bool {
        in_ranges(&self.data_ranges, a)
    }
    pub fn is_excluded(&self, a: u32) -> bool {
        in_ranges(&self.exclude_ranges, a)
    }
    pub fn decode(&self, pc: u32) -> Option<Instr> {
        self.word(pc)?;
        let ins = decode_model(pc, &mut |a| self.word(a).unwrap_or(0), self.model);
        // The whole instruction must lie inside the image.
        self.word(ins.next().wrapping_sub(2))?;
        // ... and outside data / excluded ranges.
        let end = ins.next();
        let hit = |r: &Vec<(u32, u32)>| r.iter().any(|&(s, e)| pc < e && end > s);
        if hit(&self.data_ranges) || hit(&self.exclude_ranges) {
            return None;
        }
        Some(ins)
    }
}

fn in_ranges(r: &[(u32, u32)], a: u32) -> bool {
    r.iter().any(|&(s, e)| a >= s && a < e)
}

#[derive(Default, Debug)]
pub struct Func {
    pub entry: u32,
    pub insns: BTreeMap<u32, Instr>,
    /// Basic-block start addresses (all of them are valid entry points of the Rust function).
    pub leaders: BTreeSet<u32>,
    /// Addresses right after calls (where callees return).
    pub resume: BTreeSet<u32>,
    pub callees: BTreeSet<u32>,
    pub jump_tables: Vec<(u32, Vec<u32>)>,
    pub dyn_jumps: u32,
    pub dyn_calls: u32,
    /// Addresses of computed `jmp`s with no statically known targets.
    pub unresolved_jumps: Vec<u32>,
    /// Addresses of computed `jsr`s with no statically known targets.
    pub unresolved_calls: Vec<u32>,
    /// Computed transfers resolved by a hint (or coverage): address -> number of targets.
    pub hinted: Vec<(u32, usize)>,
}

#[derive(Clone, Default)]
pub struct Options {
    /// Compiler profile (base register, stubs, jump-table idioms).
    pub profile: Profile,
    /// Small-data base register and its constant value, if any (Manx/Aztec C: a4 = data
    /// section + $7FFE).
    pub base_reg: Option<(u8, u32)>,
    /// Manual or coverage-derived targets of computed `jmp`/`jsr` instructions: instruction
    /// address -> targets. For a `jmp` they become blocks of the function, for a `jsr` callees.
    pub hints: BTreeMap<u32, Vec<u32>>,
    /// Treat every relocated long that points into code as a potential function entry.
    pub reloc_seeds: bool,
    /// Disable dead-flag elimination (for measurements).
    pub no_flag_elim: bool,
    /// Count 68000 clock cycles ([`m68k::cycles`]): every instruction adds its own to
    /// `cpu.cycles` before it runs and tells the bus the new total (`Bus::cycles`), and blocks
    /// also return when `cpu.cycles >= cpu.cycle_budget` (`--cycles`; off: the output is the
    /// same as before the option existed).
    pub cycles: bool,
    /// The CPU the code is emitted for (`--cpu`): the 68020 one also gets the 68020 exception
    /// frames, RTE and privileged MOVE from SR. The 68000 output is as before the option existed.
    pub model: Model,
}

/// Explores one function from `entry` (recursive descent over intra-procedural edges).
pub fn explore(img: &Image, entry: u32, opts: &Options) -> Func {
    let mut f = Func { entry, ..Default::default() };
    let mut work = vec![entry];
    f.leaders.insert(entry);
    while let Some(start) = work.pop() {
        let mut p = start;
        let mut prev: Vec<Instr> = Vec::new();
        loop {
            if f.insns.contains_key(&p) {
                break;
            }
            let ins = match img.decode(p) {
                Some(i) => i,
                None => break, // left the image: the dispatcher/interpreter handles it at run time
            };
            f.insns.insert(p, ins);
            let next = ins.next();
            let add = |f: &mut Func, work: &mut Vec<u32>, t: u32| {
                if f.leaders.insert(t) {
                    work.push(t);
                }
            };
            match ins.op {
                Op::Bcc { target, .. } | Op::Dbcc { target, .. } => {
                    add(&mut f, &mut work, target);
                    add(&mut f, &mut work, next);
                    break;
                }
                Op::Bra { target } => {
                    add(&mut f, &mut work, target);
                    break;
                }
                Op::Bsr { target } => {
                    f.callees.insert(target);
                    f.resume.insert(next);
                    add(&mut f, &mut work, next);
                    break;
                }
                Op::Jsr { ea } => {
                    match ea {
                        Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) => {
                            f.callees.insert(t);
                        }
                        Ea::Disp(r, d) if opts.profile.data_stubs() && opts.base_reg.map_or(false, |(br, _)| br == r) => {
                            f.callees.insert(opts.base_reg.unwrap().1.wrapping_add(d as i32 as u32));
                            f.dyn_calls += 1;
                        }
                        _ => {
                            f.dyn_calls += 1;
                            if let Some(ts) = opts.hints.get(&p) {
                                f.callees.extend(ts.iter().copied());
                                f.hinted.push((p, ts.len()));
                            } else if opts.profile != Profile::Manx {
                                let back = back_chain(&f, p);
                                if let Some(ts) = opts.profile.jump_table(img, &back, &ins) {
                                    f.callees.extend(ts.iter().copied());
                                } else {
                                    f.unresolved_calls.push(p);
                                }
                            } else {
                                f.unresolved_calls.push(p);
                            }
                        }
                    }
                    f.resume.insert(next);
                    add(&mut f, &mut work, next);
                    break;
                }
                Op::Jmp { ea } => {
                    match ea {
                        Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) => {
                            f.callees.insert(t); // tail call
                        }
                        _ => {
                            let back = back_chain(&f, p);
                            let _ = &prev;
                            let hinted = opts.hints.get(&p).cloned();
                            if let Some(ts) = hinted.clone().or_else(|| opts.profile.jump_table(img, &back, &ins)) {
                                for &t in &ts {
                                    add(&mut f, &mut work, t);
                                }
                                if hinted.is_some() {
                                    f.hinted.push((p, ts.len()));
                                }
                                f.jump_tables.push((p, ts));
                            } else {
                                f.dyn_jumps += 1;
                                f.unresolved_jumps.push(p);
                            }
                        }
                    }
                    break;
                }
                Op::Trap { .. } => {
                    // The handler normally returns (RTE) to the next instruction.
                    f.resume.insert(next);
                    add(&mut f, &mut work, next);
                    break;
                }
                _ if is_terminator(&ins.op) => break,
                _ => {}
            }
            prev.push(ins);
            p = next;
        }
    }
    f
}

/// Instructions leading to the computed transfer at `p`, following fall-through backwards
/// (oldest first).
fn back_chain(f: &Func, p: u32) -> Vec<Instr> {
    let mut back: Vec<Instr> = Vec::new();
    let mut q = p;
    for _ in 0..6 {
        match f.insns.values().find(|i| i.next() == q && i.pc != q) {
            Some(i) => {
                back.push(*i);
                q = i.pc;
            }
            None => break,
        }
    }
    back.reverse();
    back
}

/// Cheap plausibility test for addresses found only in data (relocated pointers): following the
/// fall-through path must reach a return/jump without illegal opcodes.
pub fn plausible_code(img: &Image, mut p: u32) -> bool {
    if p & 1 != 0 || !img.is_code(p) {
        return false;
    }
    for _ in 0..256 {
        let ins = match img.decode(p) {
            Some(i) => i,
            None => return false,
        };
        match ins.op {
            Op::Illegal | Op::LineA | Op::LineF => return false,
            Op::Alu { op: AluOp::Or, sz: Sz::B, src: Ea::Imm(0), dst: Ea::D(0) } => return false, // zeroed data
            Op::Rts | Op::Rte | Op::Rtr | Op::Jmp { .. } | Op::Bra { .. } => return true,
            _ => {}
        }
        p = ins.next();
    }
    false
}

/// Discovers all functions reachable from `seeds` (calls and tail calls add new functions).
pub fn discover(img: &Image, seeds: &[u32], opts: &Options) -> BTreeMap<u32, Func> {
    let mut funcs = BTreeMap::new();
    let mut work: Vec<u32> = seeds.to_vec();
    if opts.reloc_seeds {
        for &r in &img.relocs {
            if let Some(t) = img.long(r) {
                if plausible_code(img, t) {
                    work.push(t);
                }
            }
        }
    }
    while let Some(e) = work.pop() {
        if funcs.contains_key(&e) || e & 1 != 0 || img.word(e).is_none() || img.is_excluded(e) || img.is_data_range(e) {
            continue;
        }
        let f = explore(img, e, opts);
        for &c in &f.callees {
            work.push(c);
        }
        funcs.insert(e, f);
    }
    funcs
}

/// True if the code at `a` starts like a function: `link An,#d` or `movem.l regs,-(a7)`, and
/// passes [`plausible_code`].
pub fn has_prologue(img: &Image, a: u32) -> bool {
    if a & 1 != 0 || !img.is_code(a) {
        return false;
    }
    let ok = match img.decode(a).map(|i| i.op) {
        Some(Op::Link { .. }) => true,
        Some(Op::Movem { to_mem: true, ea: Ea::Pre(7), .. }) => true,
        _ => false,
    };
    ok && plausible_code(img, a)
}

/// Address-taken functions: targets of `pea d16(pc)` / `lea d16(pc),An` in the discovered code
/// that lie in a code section, are not yet function entries, and start with a prologue (see
/// [`has_prologue`]). Compilers that pass function pointers pc-relatively (Manx small code:
/// `AddTask`/`CreateProc` entries, callbacks) are otherwise invisible to discovery.
pub fn pcrel_seeds(img: &Image, funcs: &BTreeMap<u32, Func>) -> Vec<u32> {
    let mut out = BTreeSet::new();
    for f in funcs.values() {
        for ins in f.insns.values() {
            let t = match ins.op {
                Op::Pea { src: Ea::PcDisp(t) } | Op::Lea { src: Ea::PcDisp(t), .. } => t,
                _ => continue,
            };
            if !funcs.contains_key(&t) && has_prologue(img, t) {
                out.insert(t);
            }
        }
    }
    out.into_iter().collect()
}

/// Unreferenced functions: `link An,#d` prologues at even addresses of code sections that no
/// discovered instruction covers (dead code or code reached only through pointers the other
/// seeds missed).
pub fn gap_seeds(img: &Image, funcs: &BTreeMap<u32, Func>) -> Vec<u32> {
    let mut covered: BTreeSet<u32> = BTreeSet::new();
    for f in funcs.values() {
        for ins in f.insns.values() {
            let mut a = ins.pc;
            while a < ins.next() {
                covered.insert(a);
                a += 2;
            }
        }
    }
    let mut out = Vec::new();
    for s in img.segs.iter().filter(|s| s.code) {
        let mut a = s.base;
        while a + 4 <= s.base + s.data.len() as u32 {
            if !covered.contains(&a) && matches!(img.word(a), Some(w) if w & 0xFFF8 == 0x4E50) && has_prologue(img, a) {
                out.push(a);
                // skip the function body so a `link` inside it is not taken as another start
                let mut p = a;
                while let Some(i) = img.decode(p) {
                    p = i.next();
                    if is_terminator(&i.op) || covered.contains(&p) {
                        break;
                    }
                }
                a = p.max(a + 2);
                continue;
            }
            a += 2;
        }
    }
    out
}

/// `jsr d16(a4)` stubs in data: address -> target of the `jmp abs.l` found there.
pub fn find_stubs(img: &Image) -> BTreeMap<u32, u32> {
    let mut m = BTreeMap::new();
    for &r in &img.relocs {
        let at = r.wrapping_sub(2);
        if img.word(at) == Some(0x4EF9) {
            if let Some(t) = img.long(r) {
                m.insert(at, t);
            }
        }
    }
    m
}

pub fn fn_name(entry: u32) -> String {
    format!("f_{:06x}", entry)
}

/// Emits one 68000 function as a Rust function.
pub fn emit_func(f: &Func, mode: Mode, funcs: &BTreeMap<u32, String>, stubs: &BTreeMap<u32, u32>, opts: &Options, doc: Option<&str>) -> String {
    emit_func_named(f, mode, funcs, stubs, opts, doc, None)
}

/// [`emit_func`] with readable names (`--names`): the function's name, doc lines and address
/// alias from `naming`; the code is the same.
pub fn emit_func_named(f: &Func, mode: Mode, funcs: &BTreeMap<u32, String>, stubs: &BTreeMap<u32, u32>, opts: &Options, doc: Option<&str>, naming: Option<&names::Naming>) -> String {
    let mut o = String::new();
    let labels = &f.leaders;
    let stub_base = if opts.profile.data_stubs() { opts.base_reg } else { None };
    let cx = Ctx { mode, labels, funcs, stub_base, stubs, cycles: opts.cycles, model: opts.model };
    if let Some(d) = doc {
        let _ = writeln!(o, "/// {}", d);
    }
    let cpu = if opts.model == Model::M68020 { "68020" } else { "68000" };
    let _ = writeln!(o, "/// {} function at ${:06x}: {} instructions, {} blocks.", cpu, f.entry, f.insns.len(), labels.len());
    if let Some(n) = naming {
        n.write_doc(&mut o, f.entry);
    }
    let name = naming.map_or_else(|| fn_name(f.entry), |n| n.name(f.entry));
    let _ = writeln!(o, "pub fn {}<B: Bus>(cpu: &mut Cpu, bus: &mut B) {{", name);
    let _ = writeln!(o, "    let mut pc = cpu.pc;");
    let _ = writeln!(o, "    loop {{");
    let _ = writeln!(o, "    match pc {{");
    for &l in labels {
        if !f.insns.contains_key(&l) {
            continue; // undecodable target: the default arm hands it to the dispatcher
        }
        // Collect the block.
        let mut block = Vec::new();
        let mut p = l;
        while let Some(ins) = f.insns.get(&p) {
            block.push(*ins);
            let ends = is_terminator(&ins.op)
                || matches!(ins.op, Op::Bcc { .. } | Op::Dbcc { .. } | Op::Bsr { .. } | Op::Jsr { .. } | Op::Jmp { .. });
            p = ins.next();
            if ends || labels.contains(&p) {
                break;
            }
        }
        let _ = writeln!(o, "    0x{:x} => {{", l);
        let _ = writeln!(o, "        if {} {{ cpu.pc = 0x{:x}; return; }} {}", cx.budget_test(), l, cx.count(block.len(), emit::block_cycles(&block)));
        let mut falls = true;
        let live = if opts.no_flag_elim { vec![0x1F; block.len()] } else { emit::flag_liveness(&block) };
        for (k, ins) in block.iter().enumerate() {
            falls = emit_instr(&mut o, ins, &cx, (block.len() - 1 - k) as u32, live[k]);
        }
        if falls {
            let last = block.last().unwrap().next();
            if labels.contains(&last) && f.insns.contains_key(&last) {
                let _ = writeln!(o, "        pc = 0x{:x}; continue;", last);
            } else {
                let _ = writeln!(o, "        cpu.pc = 0x{:x}; return;", last);
            }
        }
        let _ = writeln!(o, "    }}");
    }
    let _ = writeln!(o, "    _ => {{ cpu.pc = pc; return; }}");
    let _ = writeln!(o, "    }}");
    let _ = writeln!(o, "    }}");
    let _ = writeln!(o, "}}\n");
    if let Some(n) = naming {
        n.write_alias(&mut o, f.entry);
    }
    o
}

pub const HEADER: &str = "// Generated by m68k-recomp. Do not edit.\n#![allow(unused_variables, unused_mut, unused_parens, unused_braces, unreachable_code, unused_imports, unused_assignments, non_snake_case, clippy::all)]\n";

/// Writes a crate with one Rust function per discovered function and the dispatch table.
/// `names`: optional entry -> description, used as doc comments.
pub fn emit_crate(dir: &std::path::Path, crate_name: &str, m68k_path: &std::path::Path, funcs: &BTreeMap<u32, Func>, mode: Mode, opts: &Options, stubs: &BTreeMap<u32, u32>, names: &BTreeMap<u32, String>, extra: &str) -> std::io::Result<Stats> {
    emit_crate_named(dir, crate_name, m68k_path, funcs, mode, opts, stubs, names, extra, None)
}

/// [`emit_crate`] with readable names (`--names`): functions named and documented by `naming`,
/// and its `names.txt` written next to `Cargo.toml`.
#[allow(clippy::too_many_arguments)]
pub fn emit_crate_named(dir: &std::path::Path, crate_name: &str, m68k_path: &std::path::Path, funcs: &BTreeMap<u32, Func>, mode: Mode, opts: &Options, stubs: &BTreeMap<u32, u32>, names: &BTreeMap<u32, String>, extra: &str, naming: Option<&names::Naming>) -> std::io::Result<Stats> {
    std::fs::create_dir_all(dir.join("src"))?;
    if let Some(n) = naming {
        std::fs::write(dir.join("names.txt"), &n.file)?;
    }
    let fn_name = |e: u32| naming.map_or_else(|| fn_name(e), |n| n.name(e));
    std::fs::write(dir.join("Cargo.toml"), format!(
        "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nm68k = {{ package = \"m68k-rc\", path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
        crate_name, m68k_path.display()))?;
    let fnames: BTreeMap<u32, String> = funcs.keys().map(|&e| (e, format!("crate::{}", fn_name(e)))).collect();
    let mut stats = Stats::default();
    let mut lib = String::from(HEADER);
    lib.push_str("use m68k::{Bus, Cpu};\nuse m68k::runtime::NativeFn;\n\n");
    let chunk = 40;
    let entries: Vec<&Func> = funcs.values().collect();
    for (i, part) in entries.chunks(chunk).enumerate() {
        let mut s = String::from(HEADER);
        s.push_str("use m68k::{alu, Bus, Cpu, Sz, ShiftKind};\n\n");
        for f in part {
            s.push_str(&emit_func_named(f, mode, &fnames, stubs, opts, names.get(&f.entry).map(|s| s.as_str()), naming));
            stats.functions += 1;
            stats.instructions += f.insns.len();
            stats.blocks += f.leaders.len();
            stats.jump_tables += f.jump_tables.len();
            stats.dyn_jumps += f.dyn_jumps as usize;
            stats.dyn_calls += f.dyn_calls as usize;
        }
        std::fs::write(dir.join("src").join(format!("part{}.rs", i)), s)?;
        let _ = writeln!(lib, "mod part{};\npub use part{}::*;", i, i);
    }
    // Dispatch table: every block label of every function is a valid entry point.
    let mut table: BTreeMap<u32, u32> = BTreeMap::new();
    for f in funcs.values() {
        table.entry(f.entry).or_insert(f.entry);
    }
    for f in funcs.values() {
        for &l in &f.leaders {
            if f.insns.contains_key(&l) {
                table.entry(l).or_insert(f.entry);
            }
        }
    }
    stats.entries = table.len();
    lib.push_str("\n/// Native code for a 68000 address, if any.\npub fn lookup<B: Bus>(pc: u32) -> Option<NativeFn<B>> {\n    Some(match pc {\n");
    let mut by_fn: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (&a, &f) in &table {
        by_fn.entry(f).or_default().push(a);
    }
    for (f, addrs) in &by_fn {
        let pats: Vec<String> = addrs.iter().map(|a| format!("0x{:x}", a)).collect();
        for c in pats.chunks(16) {
            let _ = writeln!(lib, "        {} => {}::<B>,", c.join(" | "), fn_name(*f));
        }
    }
    lib.push_str("        _ => return None,\n    })\n}\n\n");
    lib.push_str("/// Calls the native code for `cpu.pc`; false if there is none.\n#[inline]\npub fn call<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> bool {\n    match lookup::<B>(cpu.pc) {\n        Some(f) => { f(cpu, bus); true }\n        None => false,\n    }\n}\n\n");
    lib.push_str("/// Function entry points.\npub const FUNCTIONS: &[u32] = &[");
    for e in funcs.keys() {
        let _ = write!(lib, "0x{:x}, ", e);
    }
    lib.push_str("];\n");
    lib.push_str(extra);
    std::fs::write(dir.join("src/lib.rs"), lib)?;
    Ok(stats)
}

#[derive(Default, Debug)]
pub struct Stats {
    pub functions: usize,
    pub instructions: usize,
    pub blocks: usize,
    pub entries: usize,
    pub jump_tables: usize,
    pub dyn_jumps: usize,
    pub dyn_calls: usize,
}
