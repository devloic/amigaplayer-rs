# The recompiler IR

Status: complete (round 5). `m68k-recomp ... --ir` emits the crate from the IR: every 68000
instruction is lifted, control flow ends in terminators, flag liveness runs across the blocks of
a function, conditions are fused with the instruction that set their flags, and large functions
are split. `crate::emit::emit_instr` is not used under `--ir`. Without `--ir`, the output is
byte-identical to before. Code: `src/ir/`. Tests: `tests/ir_lift.rs` and `tests/ir_annot.rs`;
the differential checks of the generated code: `scripts/ir-check.sh` (section 6).

```
Image ─discover─> Func (insns, leaders, tables) ─cfg::build─> IrFunc ─ir::emit─> Rust crate
                                                   │  lift: Instr -> IrInsn (micro-ops + Term)
                                                   │  fuse: Bcc/DBcc/Scc <- the flag setter
                                                   │  liveness: across the function's blocks
              annot (maps JSON) ───────────────────┘  keyed by (program, section, offset)
```

## 1. Lifted operations (`ir::IrOp`, `ir::Term`, `ir::lift`)

Each instruction becomes an `IrInsn { instr, body: Lifted(ops), term, flag_defs, flag_uses,
live_after, live_exit, fused, export }`. Every opcode word lifts (`tests/ir_lift.rs` checks all
65536).

The micro-ops work on these kinds of state:

- **Registers:** `GetD`/`GetA` (masked to the size), `SetD` (a partial size keeps the upper
  bits), `SetA`, `AddA` (add/subtract), and for MOVE USP the inactive stack pointer
  (`GetOsp`/`SetOsp`).
- **Memory:** `Load` (zero-extended), `Store`, `StorePd32` (MOVE.L to -(An), low word first, as
  Musashi and our interpreter do), `Push`/`Pop` (A7, 16 or 32 bits).
- **Addresses:** `Addr` computes a side-effect-free `AddrExpr` (d16(An), d8(An,Xn), absolute,
  or PC-relative, folded at decode time); `PostInc`/`PreDec` apply the (An)+ / -(An) side
  effect (step 2 for a byte through A7).
- **Values:** `SextW`, `SextB`, and `Bin` (flag-free AND/OR/XOR/AND NOT/ADD/SUB/shift/rotate,
  folded when both operands are constants) for MOVEM address steps, MOVEP bytes, bit masks,
  SWAP, TAS and the SR/CCR immediates.
- **Arithmetic with lazy flags:** `Arith` (ADD/SUB/AND/OR/EOR/CMP), `LogicFlags` (MOVE, TST,
  CLR, MOVEQ, SWAP, EXT, TAS), `Unary` (NEG/NOT), `Shift` (all shifts and rotates), `Mul`
  (MULU/MULS), `BitZ` (the Z flag of BTST/BCHG/BCLR/BSET).
- **Arithmetic that reads X** (always computed with its flags): `Extend` (ADDX, SUBX, NEGX,
  ABCD, SBCD) and `Nbcd` (whose destination is left unwritten for Musashi's $9A case).
- **Status register:** `GetCcr`/`SetCcr`/`GetSr`/`SetSr` (a change of S swaps the stacks),
  `Scc` (`cond ? $FF : 0`).
- **Exceptions inside an instruction** (they leave the function with the exception frame
  built, after the tick fix-up): `Div` (DIVU/DIVS: Dn unchanged on overflow, the divide-by-zero
  trap), `Chk`, `Priv` (the privilege violation of every privileged instruction in user mode,
  stacking the instruction's own address), `TrapIf` (TRAPV).

The terminators (`Term`) are what control flow does after the micro-ops have done the side
effects (return address pushed, target computed, SR restored):

| term | from |
|---|---|
| `Branch { cond, target }` | Bcc (falls through when not taken) |
| `Dbcc { cond, dn, target }` | DBcc |
| `Goto { target, jmp }` | BRA, JMP abs/pc (`jmp`: may become a direct Rust tail call) |
| `Call { target }` | BSR, JSR abs/pc |
| `DynCall { target, ea }` | computed JSR (`ea` for the `jsr d16(a4)` stub calls) |
| `DynJump { target }` | computed JMP (jump tables included) |
| `Return { pc }` | RTS, RTR, RTE |
| `Exception { vector, pc }` | TRAP, ILLEGAL, line A, line F |
| `Stop` | STOP |

Operands are `V::T(n)` (a temporary, assigned once per instruction) or `V::C(k)` (a constant).
The op order is exactly `interp::exec`'s: resolve the source EA, read it, resolve the
destination EA, read it if needed, compute, set flags, write. The bus therefore sees the same
accesses in the same order. `tests/ir_lift.rs` checks this with `ir::eval` (a micro-op
evaluator) against `interp::exec` on registers, flags, SR, the other stack pointer, the STOP
state, memory and the logged access sequence: 60000 random instructions with 3 random states
each, and every opcode word (random extension words) with 2 states each, each in the three flag
modes below (exact: identical; eliminated: identical except the five flags; behind the budget
test: identical to exact with the budget exhausted and to eliminated without). It also asserts
that every micro-op and terminator kind was exercised.

## 2. Flags: lazy definitions, liveness across blocks, fused conditions

Every flag-setting op carries `Lazy { defs, live, exit }`: the flags it may write (from
`emit::flag_effects`), the flags read after it on paths that stay in native code (`live`), and
the flags read after it if the block were left at its end (`exit`). It is lowered in one of
three ways (`IrOp::flag_mode`):

- **exact** (`defs & live != 0`): through the shared `m68k::alu` helpers;
- **behind the budget test** (otherwise, `defs & exit != 0`): `if cpu.ticks >= cpu.budget {
  exact } else { plain }`;
- **plain** (otherwise): `wrapping_add`, `&`, `<<`, ... with no flag code. Shifts have a plain
  form for immediate counts of every kind but ROXL/ROXR; the ops that read X have none.

**Liveness** (`cfg::liveness`) is a backward fixed point over the function's blocks. Only the
edges that stay in the same Rust function -- `Fall`, `Branch` and `Jump` to a block of the
function, which are `pc = target; continue` -- carry the target's needs. Every other edge makes
all flags live: calls (direct or not: the callee may read flags), computed jumps and jump
tables (the computed target may miss every block and leave through the default arm), returns,
traps, exits, tail calls, and jumps to leaders without code. An instruction that can raise an
exception reads all flags (the frame stacks SR). Conditions read only the flags they test
(`ir::cond_flags`), and intra-function jumps read none (`lift::ir_flag_effects`).

**Why the budget test keeps everything exact.** Generated code leaves to the dispatcher only at
its own exits (where all flags are live) and at the budget check at the top of a block. A flag
that is dead along every native path can still be seen there: the dispatcher, an interrupt
(whose frame stacks SR into memory), the lockstep compare, the interpreter or a block-stepped
(SMC-guarded) run. Between an op behind the budget test and the next block's budget check
nothing changes `cpu.ticks` or `cpu.budget` (calls end blocks and make all flags live; the
exception paths read all flags), so the op computes its flags exactly when that check will
leave. So the state outside native code is always exact, and SMC guards, overlays, exceptions
and the interpreter fallback see what they saw with the instruction emitter.

**Fused conditions** (`cfg::fuse`, before liveness). For a Bcc, DBcc or Scc, the nearest earlier
instruction of the block that touches a flag is looked at; only flag-neutral instructions (no
flag written or read: MOVEA, LEA, ADDA, MOVEM, EXG, ...) may lie between. If it determines the
condition (`IrInsn::cond_source`: CMP/SUB for every condition but VC/VS, ADD for EQ/NE/MI/PL,
AND/OR/EOR/MOVE/TST/CLR/MOVEQ/SWAP/EXT/TAS for all, a bit test for EQ/NE), the producer
exports its operands (`let f{k}a: u32; let f{k}b: u32;` before its scope, assigned inside) and
the condition is computed from them (`emit::fused_cond`: e.g. `(f0b & 0xffff) < (f0a &
0xffff)` for BCS after CMP.W, `(f0a as u16 as i16) > 0` for BGT after TST.W). The consumer then
reads no flags, and the liveness pass decides, as for any definition, whether the producer
still computes them.

## 3. Control-flow graph (`ir::cfg`)

`IrFunc { entry, key, blocks, aliases, dead_labels, callees, jump_tables, dyn_jumps, dyn_calls }`.

Blocks are cut exactly where the instruction emitter cuts them: at every leader of the discovered
`Func`, and after a Bcc, DBcc, BSR, JSR, JMP or terminator. So a block is also the unit of the
instruction budget, and every block start is a dispatch entry.

Edge kinds:

| edge | from |
|---|---|
| `Fall(next)` | the block ends at a label; also the not-taken side of Bcc/DBcc |
| `Branch(t)` | Bcc/DBcc taken |
| `Jump(t)` / `TailCall(t)` | BRA/JMP abs inside the function / to another function |
| `Table(i, t)` | jump-table JMP: one edge per statically resolved entry |
| `Call{target, resume}` | BSR/JSR abs/pc. `resume` is a block of the caller (a return point) |
| `DynCall{resume}` | computed JSR (library vector, a4 stub, function pointer) |
| `DynJump` | computed JMP with no known targets: back to the dispatcher |
| `Return` | RTS/RTE/RTR |
| `Trap{resume}` | TRAP #n: the handler normally comes back to `resume` |
| `Exit` | an exception, STOP, or falling off the decoded code |

**Multi-entry aliases.** `aliases` lists every block start with code: the entry first, then
return points, exception returns, computed-jump targets and labels other functions jump into.
The dispatch table is built from them, the first function to claim an address owns it. A
function is therefore one Rust function with N entries, not N functions. `dead_labels` are
leaders without decodable code; the default arm hands them to the dispatcher.

`key = (section index, offset)` comes from the image's section bases (`cfg::key_of`). It is
printed in each function's doc line (`(from the IR, 0:00006)`) and is the join key with the
annotation layer.

## 4. Emission from the IR (`ir::emit`)

`ir::emit::emit_crate` takes an `IrProgram` and the split limit, and writes the same files as
`emit_crate`: `Cargo.toml`, `part{i}.rs` of 40 functions, and `lib.rs` with `lookup`, `call`,
`FUNCTIONS` and the extras, with the same interface.

- **Instructions:** each one gets a `// pc: disasm` comment and one Rust statement per micro-op
  inside its own `{}`, then its terminator (`ir::emit::lower`, `lower_term`): `pc = t;
  continue` inside the function, direct Rust calls of known callees (resuming in place when the
  callee returns to the expected address), the `jsr d16(a4)` stub calls, and every other exit
  with `cpu.pc` set. The tick fix-up before an exception (`cpu.ticks -= rest`) and the block
  header (budget check, `cpu.ticks += n`) and fall-through are as in the instruction emitter.
- **Split functions** (`--ir-split N`, default 120 instructions; 0 = never): a function with
  more instructions becomes a dispatcher over parts of contiguous blocks (at most N
  instructions each, a block is never split):

  ```text
  pub fn f_xxxxxx(cpu, bus) { loop { let more = match cpu.pc { <blocks of part k> => f_xxxxxx_pk(cpu, bus), .., _ => return }; if !more { return; } } }
  #[inline(never)] fn f_xxxxxx_pk(cpu, bus) -> bool { let mut pc = cpu.pc; loop { match pc { <blocks>, _ => { cpu.pc = pc; return true; } } } }
  ```

  A part runs its blocks as the whole function would; a `continue` to a block of another part
  (or to an address with no block) leaves the part with `cpu.pc` set and `true`, and the
  dispatcher enters the part that has it, or returns. Every other exit returns `false`. Budget
  checks, tick counts and the state at every exit are unchanged.
- **Dispatch table:** `lookup` is a binary search in a sorted `static ENTRIES` table with the
  owning function's index (`OWNER`), and one `match` arm per function (`native`), instead of
  one `match` arm per address.

Single-instruction functions for the fuzz tests (`Mode::Single`) are written by
`ir::emit::emit_single`.

## 5. Annotation layer (`ir::annot`)

The annotations are separate from the IR and refer to it by key (or by absolute address in the
reference layout):

- **`FuncKey { program, section, offset }`.** The section is a hunk, overlay node or module
  section. A key survives relocation: overlays and the DotC modules load at different addresses
  in different crates.
- **`FuncAnnot`** holds:
  - `name`, `note` (role), and `confidence` (`verified` / `evidence` / `guess` / `disassembly`);
  - `abi`;
  - `sig: Option<Signature{args, result}>`;
  - `tags`.
- **`Abi`** is one of:
  - `ManxStack16`: Manx/Aztec C. Arguments on the stack with a 16-bit `int`, the result in d0,
    d4-d7/a2-a5 preserved, and a4 as the small-data base.
  - `RegisterAsm { args, result }`: `register asm` and library-style register arguments.
  - `GreenHillsA6`: a `link a6` frame, 32-bit `int` and absolute globals.
  - `Unknown`.
- **`Tag`**, for context and translation:
  - `Interrupt`, `Callback`, `Task`: the context a function is entered in. They matter for which
    state can be cached in host locals across a call.
  - `Overlay(n)` and `Stub`.
  - `KeepInterpreted`: SMC, or code built at run time. The recompiler should leave it out.
  - `Guarded`: native code only behind a run-time check of the code bytes or entry state.
- **`StructLayout { name, size, fields: [(offset/type, description)] }`.**
- **`AnnotSet { program, bases, funcs, structs }`** provides `by_abs`, `by_key`, `names()` (which
  feeds the emitters' doc comments), `rebase(bases)`, `named()` and `tagged()`.

### Feeding it from the maps

`ir::json` is a small JSON reader, because serde_json is not in the offline dependency set.

| map | loader | notes |
|---|---|---|
| `wings-map/wings-map.json` | `load_wings` | `addr` "S:OOOOO" + `abs`. The `kind` overlayN becomes `Overlay(N)`, and `wrapper_of` becomes `Stub`. Confidence comes from "(verified)" or "guess" in the note. All functions are `ManxStack16`. `structures` (with `size` / `fields`) become layouts. |
| `dotc-map/dotc-map.json` | `load_dotc` | One set per executable that has functions. `where` "S:OOOOO" + `addr` `$…`. Bases come from `sections[].base`. `Defender` is `ManxStack16` and the modules are `GreenHillsA6`. A non-empty `smc_suspect` becomes `KeepInterpreted`. Modules with `same_code_as` reuse the named set. |
| `sinbad-map/sinbad-map.json` | `load_sinbad(path, bases)` | One set per program. Each function has a section number and an absolute address, but the map gives no section bases, so `offset` stays `None` until the caller rebases with the load layout. `role` and `role_source` become note and confidence. `stub` becomes `Stub`. |

Interrupt, callback and task tags are taken only from explicit wording in the notes, so these
counts are a lower bound.

Counts on the local drafts (`cargo test --release -p recomp --test ir_annot -- --nocapture`):

- **Wings:** 735 functions, 34 named, 395 in overlays, 65 stubs, 3 interrupt, 4 callback,
  8 structs.
- **DotC:**
  - Defender: 516 functions, 206 named, 1 keep-interpreted, 4 task, 5 structs.
  - MUD_D.C: 42 functions, 23 named.
  - dcintro: 23 functions, 23 named, 2 task.
- **Sinbad:** 18 programs, 1581 functions, 738 with a role, 564 stubs, 1 callback, 3 task.

The tests skip a map when its file is absent. The maps describe copyrighted games and stay
local.

## 6. Checks (`scripts/ir-check.sh`)

`scripts/ir-check.sh [LOGDIR]` rebuilds `m68k-recomp`, runs `cargo test -p recomp`, the fuzz
checks, regenerates every program with `--ir` into `out/ir/` (never `out/<game>_gen`), checks that
the default output is byte-identical to a base binary's (`BASE=`, default
`build/w2b/m68k-recomp.base`), and runs rr-check and every scripted lockstep run of
`scripts/regress.sh` on harness copies under `build/ir-check/`. Options: `FRAMES`, `LONG=0`,
`GAMES=0`, `SPLIT=N` (`--ir-split`), `REF=DIR` (diff the normalised logs against a regress.sh
log folder), `WASM=1` (the wasm smoke test on the IR crate), `BEFORE=1` (the same on the default
crate, for sizes and build times).

Results (`REF=build/regress-w1 WASM=1 BEFORE=1`, 15 min):

```
cargo test -p recomp: 14 passed, 0 failed
ir fuzz-check: 10000 cases of 1 instruction(s) x 20 states = 200000 runs, 0 mismatches
ir fuzz-check: 3000 cases of 6 instruction(s) x 20 states = 60000 runs, 0 mismatches
ir 2290 fused conditions, fuzz-check: 3000 cases of 6 instruction(s) x 20 states = 60000 runs, 0 mismatches
default output: 30 programs byte-identical to the base binary's (45939626 bytes of Rust)
ir rr: 1 programs, 70722 instructions, 70722 (100.0%) emitted from the IR; lazy flags: 8934 computed, 9669 behind the budget test, 32505 eliminated; 246 functions split into 567 parts
ir wings: 1 programs, 67819 instructions, 67819 (100.0%) emitted from the IR; lazy flags: 8124 computed, 9083 behind the budget test, 27593 eliminated; 135 functions split into 482 parts
ir dotc: 11 programs, 23192 instructions, 23192 (100.0%) emitted from the IR; lazy flags: 2949 computed, 1796 behind the budget test, 6621 eliminated; 37 functions split into 88 parts
ir sinbad: 17 programs, 54833 instructions, 54833 (100.0%) emitted from the IR; lazy flags: 7157 computed, 4555 behind the budget test, 17510 eliminated; 100 functions split into 229 parts
ir rr-check: 0 differences
ir rr-check slice: 0 differences
ir boot0: rr-boot compare: identical
ir boot1: rr-boot compare: identical
ir boot2: rr-boot compare: identical
ir boot3: rr-boot compare: identical
ir wings-boot compare: identical
ir dotc-boot compare: identical
ir sinbad-boot compare: identical
ir wasm smoke: 300 frames, 4153884 instructions; instantiate 42 ms, run 3.529 s, memory 26 MB; 300 frames, hashes identical to native
```

Builds (release, warm dependencies): rr-boot 92.6 s on the default crate, 49.3 s on the IR crate;
wasm32 85.1 s and 56.9 s. Rocket Ranger's wasm: 17759121 bytes on the default crate (largest
functions f_021f6e 166973, f_022aea 131121, lookup 111686, f_0293bc 110140 bytes), 15471997 bytes
on the IR crate (largest f_02663e_p0 70180, then m68k's interpreter 69953, f_023bc2_p0 66425).
The IR wasm runs 300 frames under Node with hashes identical to the native run.

The run logs differ from `build/regress-w1` in OS-call counts (graphics.InitArea no longer a stub,
and the counts that follow) because of round 5's runtime changes; the default-emitter crates give
the same normalised logs as the IR crates with the same runtime (boot0, boot1, boot2, dotc, sinbad
compared).

## 7. Readable names (`--names`, `src/names.rs`)

`m68k-recomp ... --names` (config `names = true`; a bare `--names`, while `--names FILE` still
reads a names file) names each function after what it does and keeps the address in the name,
so names are unique and stable and logs and maps still line up: `blit_copy_050bf4`,
`wait_vblank_04e906`, `irq_level3_05199a`, `alloc_mem_001000`. It works on both emitters (the
analysis reads the discovered `Func`s, before the IR). Without the option the output is
byte-identical. Names change identifiers only: the same statements are emitted under the new
name, and `pub use <name> as f_xxxxxx` keeps the old identifier usable (`tests/names.rs` checks
that stripping the doc lines and the alias and renaming back gives the unnamed output, on both
emitters).

Sources, best first:

1. **Given names**: the annotation sets' identifier names, and the first word of a names-file or
   map description when it reads as a name (`anim_open_views (0:00006)`, `main: ...`). They always
   win; the derived name is then kept in the doc line (`derived: blit_copy (...)`).
2. **Vectors**: a function stored into `$64-$7C` (`move.l #f,$6c.w`) is `irq_level<n>`, into
   `$80-$BC` `trap<n>_handler`; one that returns with RTE is `irq_handler`.
3. **Hardware**: the custom-chip ($DFF000-$DFF1FF, HRM names; an H/L pointer pair written with
   one long is one register: `COP1LC`) and CIA registers (CIAA: odd bytes of $BFE001-$BFEF01,
   CIAB: even bytes of $BFD000-$BFDF00) each function reads and writes: absolute addresses, and
   `(An)`, `(An)+`, `-(An)`, `d16(An)` with An known. Register values come from a forward fixed
   point over the function's instructions (constant loads, LEA/ADDA/ADD arithmetic, EXG, saves
   and restores by MOVEM and `move.l Rn,-(a7)`), the values every static caller passes, the
   values callees return, and per-callee clobber sets (the registers written, transitively,
   minus those saved and restored); `d8(An,Xn)` from a hardware base counts as `REG[idx]`. Code
   outside the crate (other units, computed calls, traps) clobbers everything except the
   small-data base and address registers that hold a hardware base (code that changed those
   would break its callers, which reuse them). The dominant group gives the stem: `blit_*`
   (`clear`, `fill`, `copy`, `masked`, `line`, `area_fill` from BLTCON0/BLTCON1 constants,
   `setup` without BLTSIZE), `copper_set_list`, `audio_play/update/volume/stop[_chN]`,
   `sprite_*`, `display_*`, `palette_set`, `input_mouse/joystick/fire`, `keyboard_read` (CIAA
   SDR), `disk_read/write/drive/wait` (DSKLEN's WRITE bit, CIAB PRB), `irq_ack/enable/disable/
   setup`, `dma_*`, `timer_*`; checked before them: `wait_vblank` (VPOSR/VHPOSR read in a small
   loop), `wait_blit` (DMACONR polled), `random` (a beam-counter or CIA-timer read plus
   MUL/EOR/rotate/SWAP) and `hw_init`/`hw_loop` (three or more groups, without/with a long loop).
   DMACON constants count for the channels they switch.
4. **OS calls**: `jsr/jmp -n(a6)` with a6 a known library base: ExecBase from `4.w`, other bases
   from `OpenLibrary`/`OldOpenLibrary` results (the name string from a1, or the last constant
   pushed for a C stub) stored in globals. Calls through small stubs (at most 12 instructions,
   one library call) count for their callers. LVO names come from `amiga_rt::layout`:
   `file_read`, `file_open`, `load_seg`, `alloc_mem`, `free_mem`, `open_library`, `io_do`,
   `msg_get`, `gfx_*`, `ui_*`, ... In a function of more than 40 instructions only the telling
   calls (read, write, LoadSeg, AllocMem, DoIO, ...) name it.
5. **Structure**: a leaf with no memory access besides the stack and some arithmetic is
   `math_mul`/`math_div`/`math_shift`/`math`. Everything else stays `f_xxxxxx`.

Each function's doc comment gets `Name:` (source and reason), `Hardware:` (registers with read
and write counts, `(polled)`, constants written), `OS calls:`, the hardware or library base
registers every static caller passes (`At entry`), and its callers and callees by their new
names. The crate gets `names.txt`, one line per function: `<address> <name>  # <source>:
<reason>` (`S:OOOOO` for hunk files, `$address` for raw images). It is a names file: edit a name
and pass the file back with `--names FILE` (or `names_files`); the edited name wins, and
`_<address>` is appended if it is missing. Tests: `tests/names.rs` (register tables; tracking
through callers, saves, unknown calls and clobbers; vectors; ExecBase and OpenLibrary bases; the
naming rules; the names.txt round trip; emission equivalence).

## 8. Next steps

1. Register promotion inside a function: keep d/a registers in Rust locals between dispatch
   points. This needs the annotation tags: not across `Interrupt` entries, and spilled at
   calls, according to the ABI's preserved set.
2. Use the annotations in emission (round 5, W4a):
   - `KeepInterpreted`: leave the function out of `lookup`.
   - `Guarded`: emit a check of the code bytes.
   - `Signature`: direct Rust calls with typed arguments for `ManxStack16` leaf functions, and
     flags dead at calls of functions whose signature says they read none.
3. Map loader for Rocket Ranger's own notes (`build/rr_names.txt`), and an `ir` key in the
   TOML config.
4. Fuse more conditions (V from CMP/SUB, C from ADD, across flag-neutral blocks).
