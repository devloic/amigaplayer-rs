#!/usr/bin/env python3
"""Disassemble an AmigaOS hunk executable (e.g. a DeliPlayer) with capstone.

Each hunk N is relocated to base 0x10000*(N+1) so absolute references are
readable. Usage: hunkdis.py FILE [hunk-index | tags]
"""
import struct
import sys

import capstone

HUNK_CODE, HUNK_DATA, HUNK_BSS = 0x3E9, 0x3EA, 0x3EB
HUNK_RELOC32, HUNK_SYMBOL, HUNK_DEBUG, HUNK_END = 0x3EC, 0x3F0, 0x3F1, 0x3F2
HUNK_HEADER, HUNK_RELOC32SHORT, HUNK_DREL32 = 0x3F3, 0x3FC, 0x3F7


def load(path):
    d = open(path, "rb").read()
    p = 0

    def rl():
        nonlocal p
        v = struct.unpack_from(">I", d, p)[0]
        p += 4
        return v

    assert rl() == HUNK_HEADER
    while rl():
        pass
    _tsize, first, last = rl(), rl(), rl()
    sizes = []
    for _ in range(last - first + 1):
        s = rl()
        if s & 0xC0000000 == 0xC0000000:
            rl()
        sizes.append((s & 0x3FFFFFFF) * 4)
    hunks = []
    cur = None
    while p < len(d):
        t = rl() & 0x3FFFFFFF
        if t in (HUNK_CODE, HUNK_DATA):
            n = rl() * 4
            cur = {"type": t, "data": bytearray(d[p:p + n]), "relocs": []}
            cur["data"] += bytes(sizes[len(hunks)] - n)
            p += n
            hunks.append(cur)
        elif t == HUNK_BSS:
            rl()
            cur = {"type": t, "data": bytearray(sizes[len(hunks)]), "relocs": []}
            hunks.append(cur)
        elif t == HUNK_RELOC32:
            while True:
                c = rl()
                if not c:
                    break
                h = rl()
                for _ in range(c):
                    cur["relocs"].append((rl(), h))
        elif t in (HUNK_RELOC32SHORT, HUNK_DREL32):
            start = p
            while True:
                c = struct.unpack_from(">H", d, p)[0]
                p += 2
                if not c:
                    break
                h = struct.unpack_from(">H", d, p)[0]
                p += 2
                for _ in range(c):
                    cur["relocs"].append((struct.unpack_from(">H", d, p)[0], h))
                    p += 2
            if (p - start) & 2:
                p += 2
        elif t == HUNK_SYMBOL:
            while True:
                n = rl()
                if not n:
                    break
                p += n * 4 + 4
        elif t == HUNK_DEBUG:
            p += rl() * 4
        elif t == HUNK_END:
            pass
        else:
            raise SystemExit("unknown hunk type %x at %x" % (t, p - 4))
    for h in hunks:
        for off, target in h["relocs"]:
            v = struct.unpack_from(">I", h["data"], off)[0]
            struct.pack_into(">I", h["data"], off, (v + 0x10000 * (target + 1)) & 0xFFFFFFFF)
    return hunks


def main():
    if len(sys.argv) > 2 and sys.argv[2] == "tags":
        dump_tags(sys.argv[1])
        return
    hunks = load(sys.argv[1])
    only = int(sys.argv[2]) if len(sys.argv) > 2 else None
    md = capstone.Cs(capstone.CS_ARCH_M68K, capstone.CS_MODE_M68K_040)
    md.skipdata = True
    for i, h in enumerate(hunks):
        if only is not None and i != only:
            continue
        base = 0x10000 * (i + 1)
        kind = {HUNK_CODE: "CODE", HUNK_DATA: "DATA", HUNK_BSS: "BSS"}[h["type"]]
        print("; hunk %d %s size %d base %x relocs %d" % (i, kind, len(h["data"]), base, len(h["relocs"])))
        if h["type"] == HUNK_CODE:
            for ins in md.disasm(bytes(h["data"]), base):
                print("%06x  %-24s %s %s" % (ins.address, ins.bytes.hex(), ins.mnemonic, ins.op_str))
        elif h["type"] == HUNK_DATA:
            data = h["data"]
            for o in range(0, len(data), 16):
                chunk = data[o:o + 16]
                asc = "".join(chr(c) if 32 <= c < 127 else "." for c in chunk)
                print("%06x  %-48s %s" % (base + o, chunk.hex(" "), asc))



TAG_NAMES = ["InternalPlayer", "CustomPlayer", "RequestDTVersion", "RequestKickVersion",
             "PlayerVersion", "PlayerName", "Creator", "Check1", "Check2", "ExtLoad",
             "Interrupt", "Stop", "Config", "UserConfig", "SubSongRange", "InitPlayer",
             "EndPlayer", "InitSound", "EndSound", "StartInt", "StopInt", "Volume", "Balance",
             "Faster", "Slower", "NextPatt", "PrevPatt", "NextSong", "PrevSong", "SubSongTest",
             "NewSubSongRange", "DeliBase", "Flags", "CheckLen", "Description", "Decrunch",
             "Convert", "NotePlayer", "NoteStruct", "NoteInfo", "NoteSignal", "Process",
             "Priority", "StackSize", "MsgPort", "Appear", "Disappear", "ModuleName",
             "FormatName", "AuthorName", "InitNote"]


def dump_tags(path):
    """Print the DTP_ tag array of a DeliPlayer (hunk 0 at base 0x10000)."""
    hunks = load(path)
    code = hunks[0]["data"]
    assert code[4:12] == b"DELIRIUM", "not a DeliPlayer"
    p = struct.unpack_from(">I", code, 12)[0]
    allh = {0x10000 * (i + 1): h["data"] for i, h in enumerate(hunks)}

    def rd(a):
        b = a & ~0xFFFF
        return struct.unpack_from(">I", allh[b], a - b)[0]

    while True:
        t, d = rd(p), rd(p + 4)
        if t == 0:
            break
        i = t - 0x80004454
        name = TAG_NAMES[i] if 0 <= i < len(TAG_NAMES) else "tag+%d" % i
        print("%-18s %08x" % (name, d))
        p += 8


if __name__ == "__main__":
    main()
