#!/usr/bin/env python3
"""Recompile 68k players (DeliPlayers, Eagleplayers) to Rust, into crates/players-native/gen.

    tools/native-players.py [--recomp BIN] [--only NAME,...] [--max-cpu 68000|68020] DIR...

Every AmigaOS hunk file under the DIRs (two levels deep) is run through m68k-recomp
(tools/recomp, built by tools/recomp/build.sh) with its own fixed load address, so
all of them can be loaded at once. The host loads a player at those addresses when the
file's FNV-1a hash matches, and runs its calls through the generated code, interpreting
whatever the recompiler did not reach.

A player is recompiled for the 68000 unless the 68000 decoder cannot represent it: it
reaches a 68010+ opcode (an `illegal` that a 68020 decodes as something else) or uses
68020 addressing modes or long branches, which a 68000 decoder would misread without
noticing (found with capstone). Those are recompiled again with `--cpu 68020`, and kept if that
translation has no such opcode left; the rest keep running on the 68020 interpreter.
An `illegal` at an odd address is data reached as code (a real PC is never odd), and
one a 68020 does not decode either is illegal on every CPU; neither counts.

The output is derived from the player files, so it is .gitignored, like the files.
Run `python3 -I`; the recompiler comes from --recomp, $M68K_RECOMP, or the local build
(target-recomp/release/m68k-recomp).
"""
import argparse
import json
import os
import re
import shutil
import struct
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from hunkdis import load  # noqa: E402

OUT = HERE.parent / "crates" / "players-native" / "gen"
# Must match src/machine.rs: NATIVE_LO..NATIVE_HI is kept out of the guest heap.
NATIVE_LO = 0x80_0000
NATIVE_HI = 0xAE_0000
ALIGN, GAP = 0x100, 8


def fnv1a64(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def hunk_files(dirs):
    seen = {}
    for d in dirs:
        d = Path(d)
        for p in sorted(d.iterdir()) + sorted(q for s in sorted(d.iterdir()) if s.is_dir() for q in s.iterdir()):
            if not p.is_file() or p.suffix == ".info":
                continue
            data = p.read_bytes()
            if data[:4] == b"\x00\x00\x03\xf3":
                seen.setdefault(fnv1a64(data), (p, data))
    return list(seen.values())


def capstone_020():
    try:
        import capstone
    except ImportError:
        sys.exit("capstone is required (pip install capstone)")
    return capstone.Cs(capstone.CS_ARCH_M68K, capstone.CS_MODE_BIG_ENDIAN | capstone.CS_MODE_M68K_020)


def unknown_opcodes(src, path: Path, bases):
    """The `illegal`s of a translation that a 68020 would execute: (address, opcode) pairs."""
    md = capstone_020()
    hunks = load(str(path))
    out = []
    for a, w in re.findall(r"// ([0-9a-f]{6}): illegal ; \$([0-9a-f]{4})", src):
        a = int(a, 16)
        if w == "4afc" or a & 1 or (a, w) in out:
            continue
        for hb, h in zip(bases, hunks):
            if hb <= a < hb + len(h["data"]):
                if next(md.disasm(bytes(h["data"][a - hb:a - hb + 22]), a, 1), None) is not None:
                    out.append((a, w))
    return out


def needs_020(gen: Path, path: Path, bases):
    """Why the 68000 translation of this player cannot be trusted, or None."""
    src = "".join(f.read_text() for f in sorted(gen.glob("part*.rs")))
    bad = unknown_opcodes(src, path, bases)
    if bad:
        return f"{len(bad)} 68010+ opcode(s), first ${bad[0][1]} at ${bad[0][0]:06x}"
    md = capstone_020()
    hunks = load(str(path))
    for a in sorted({int(x, 16) for x in re.findall(r"^\s*// ([0-9a-f]{6}): ", src, re.M)}):
        for hb, h in zip(bases, hunks):
            if hb <= a < hb + len(h["data"]):
                for ins in md.disasm(bytes(h["data"][a - hb:a - hb + 22]), a, 1):
                    if re.search(r"\*[248]|\[", ins.op_str):
                        return f"68020 addressing at ${a:06x}: {ins.mnemonic} {ins.op_str}"
                    # Bcc/BRA/BSR with an $FF displacement byte take a 32-bit displacement on a
                    # 68020; a 68000 decodes them as short branches to an odd address.
                    if re.fullmatch(r"b\w+\.l", ins.mnemonic):
                        return f"68020 long branch at ${a:06x}: {ins.mnemonic} {ins.op_str}"
    return None


def unsupported_020(gen: Path, path: Path, bases):
    """Why the 68020 translation of this player cannot be used either, or None."""
    src = "".join(f.read_text() for f in sorted(gen.glob("part*.rs")))
    bad = unknown_opcodes(src, path, bases)
    if bad:
        return f"{len(bad)} 68020 opcode(s) not implemented, first ${bad[0][1]} at ${bad[0][0]:06x}"
    return None


def code_ranges(gen: Path, path: Path, bases, cpu):
    """The byte ranges of every instruction the recompiler translated, merged.

    The generated code is only valid while exactly these bytes are unchanged. Players
    keep their variables in the code hunk, so checking whole hunks would condemn all of
    them; checking these catches real self-modification and nothing else.
    """
    import capstone
    mode = capstone.CS_MODE_M68K_020 if cpu == "68020" else capstone.CS_MODE_M68K_000
    md = capstone.Cs(capstone.CS_ARCH_M68K, capstone.CS_MODE_BIG_ENDIAN | mode)
    src = "".join(f.read_text() for f in sorted(gen.glob("part*.rs")))
    hunks = load(str(path))
    spans = []
    for a in sorted({int(x, 16) for x in re.findall(r"^\s*// ([0-9a-f]{6}): ", src, re.M)}):
        for hb, h in zip(bases, hunks):
            if hb <= a < hb + len(h["data"]):
                ins = next(md.disasm(bytes(h["data"][a - hb:a - hb + 22]), a, 1), None)
                spans.append((a, a + (ins.size if ins else 2)))
    out = []
    for lo, hi in spans:
        if out and lo <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], hi))
        else:
            out.append((lo, hi))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--recomp", default=os.environ.get("M68K_RECOMP",
                    str(HERE.parent / "target-recomp" / "release" / "m68k-recomp")))
    ap.add_argument("--only", help="comma-separated file names to recompile (default: all)")
    ap.add_argument("--max-cpu", choices=["68000", "68020"], default="68020",
                    help="68000: leave the players that need a 68020 to the interpreter")
    a = ap.parse_args()
    only = set(a.only.split(",")) if a.only else None
    if not os.access(a.recomp, os.X_OK):
        sys.exit(f"m68k-recomp not found at {a.recomp} (run tools/recomp/build.sh, or set $M68K_RECOMP or --recomp)")

    tmp = OUT.with_name("gen.new")
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir(parents=True)
    base = NATIVE_LO + ALIGN  # room for the first segment header below it
    entries, skipped = [], []
    for i, (path, data) in enumerate(hunk_files(a.dirs)):
        if only is not None and path.name not in only:
            continue
        mod = f"p{i:03}"
        sizes = [len(h["data"]) for h in load(str(path))]
        bases, p = [], base
        for s in sizes:
            bases.append(p)
            p = (p + max(s, 4) + GAP + ALIGN - 1) & ~(ALIGN - 1)
        if p > NATIVE_HI:
            skipped.append((path.name, "native region full"))
            continue
        gen = tmp / mod

        def recompile(cpu):
            # --cycles: the code counts 68000 cycles, which the host's clock follows
            r = subprocess.run([a.recomp, "hunk", str(path), "--base", f"{base:x}", "--cpu", cpu,
                                "--cycles", "--out", str(tmp / f"{mod}.crate"), "--name", mod],
                               capture_output=True, text=True)
            if r.returncode != 0:
                shutil.rmtree(tmp / f"{mod}.crate", ignore_errors=True)
                return "recompiler: " + (r.stderr.strip().splitlines() or ["?"])[-1]
            shutil.rmtree(gen, ignore_errors=True)
            (tmp / f"{mod}.crate" / "src").rename(gen)
            shutil.rmtree(tmp / f"{mod}.crate")
            got = [int(v) for v in re.search(r"BASES: &\[u32\] = &\[([^\]]*)\]",
                                             (gen / "lib.rs").read_text()).group(1).split(",") if v.strip()]
            if got != bases:
                sys.exit(f"{path.name}: recompiler placed sections at {got}, expected {bases}")
            return None

        cpu = "68000"
        why = recompile(cpu)
        if why is None:
            why020 = needs_020(gen, path, bases)
            if why020 and a.max_cpu == "68000":
                why = why020
            elif why020:
                cpu = "68020"
                why = recompile(cpu) or unsupported_020(gen, path, bases)
                if why:
                    why = f"{why020}; with --cpu 68020: {why}"
        if why:
            skipped.append((path.name, why))
            shutil.rmtree(gen, ignore_errors=True)
            continue
        (gen / "lib.rs").rename(gen / "mod.rs")
        # Generated as a crate of its own; here each player is a module of one crate.
        for f in gen.glob("part*.rs"):
            f.write_text(f.read_text().replace("crate::", "super::"))
        entries.append((mod, path.name, fnv1a64(data), bases, code_ranges(gen, path, bases, cpu), cpu))
        base = p
        print(f"{mod} {path.name}: {sum(sizes)} bytes at ${bases[0]:06x}" + (" (68020)" if cpu == "68020" else ""),
              file=sys.stderr)

    reg = ["// Generated by tools/native-players.py. Do not edit.\n"]
    for mod, *_ in entries:
        reg.append(f'#[allow(dead_code)] #[path = "../gen/{mod}/mod.rs"] mod {mod};\n')
    for mod, *_ in entries:
        reg.append(f"fn l_{mod}(pc: u32) -> Option<Native> {{ {mod}::lookup::<DynBus>(pc) }}\n")
    reg.append("pub static PLAYERS: &[Info] = &[\n")
    for mod, name, h, bases, code, cpu in entries:
        ranges = ", ".join(f"({lo:#x}, {hi:#x})" for lo, hi in code)
        reg.append(f"    Info {{ file: {json.dumps(name)}, hash: {h:#018x}, bases: &{bases!r}, "
                   f"code: &[{ranges}], lookup: l_{mod}, cpu: Model::M{cpu} }},\n")
    reg.append("];\n")
    (tmp / "registry.rs").write_text("".join(reg))

    shutil.rmtree(OUT, ignore_errors=True)
    tmp.rename(OUT)
    print(f"{len(entries)} players recompiled into {OUT}", file=sys.stderr)
    for name, why in skipped:
        print(f"  interpreted: {name}: {why}", file=sys.stderr)


if __name__ == "__main__":
    main()
