"""Compare the audio register writes of two runs of the same module, tick by tick.

    python3 -I tools/replay-cmp.py REFERENCE.log PORT.log [--ticks N]

The logs are DELI_WRITE_LOG output. Sample addresses are made comparable: one
inside the module becomes its offset in the module ("M+off", from the log's
"# module" line), and every other one (a player's own waveforms) must differ
between the two runs by one constant, which is reported. Writes are grouped into
ticks by the gaps between them; ticks are compared in order, and the first one
that differs is printed with both sides.
"""
import sys

TICK_GAP = 20000  # colour clocks: a tick's writes are far closer together than this


def load(path):
    base, size, ticks, last = 0, 0, [], None
    hi = {}
    for line in open(path):
        f = line.split()
        if f[0] == '#':
            if f[1] == 'module':
                base, size = int(f[2], 16), int(f[3])
            continue
        a, v, t = int(f[0], 16), int(f[1], 16), int(f[2])
        if not 0xDFF096 <= a <= 0xDFF0DF:
            continue
        if last is None or t - last > TICK_GAP:
            ticks.append([])
        last = t
        if 0xDFF0A0 <= a < 0xDFF0E0 and (a - 0xDFF0A0) % 16 in (0, 2):
            ch = (a - 0xDFF0A0) // 16
            if (a - 0xDFF0A0) % 16 == 0:
                hi[ch] = v
                continue
            addr = hi.pop(ch, 0) << 16 | v
            if base <= addr < base + size:
                ticks[-1].append((f'LOC{ch}', ('M', addr - base)))
            else:
                ticks[-1].append((f'LOC{ch}', ('X', addr)))
            continue
        name = 'DMACON' if a == 0xDFF096 else f'{["LEN", "PER", "VOL", "DAT"][((a - 0xDFF0A0) % 16 - 4) // 2]}{(a - 0xDFF0A0) // 16}'
        ticks[-1].append((name, v))
    return ticks


def show(w, xoff):
    n, v = w
    if isinstance(v, tuple):
        return f'{n}={v[0]}+{v[1]:x}' if v[0] == 'M' else f'{n}=X:{v[1]:06x}'
    return f'{n}={v:04x}'


def main():
    args = sys.argv[1:]
    limit = None
    if '--ticks' in args:
        i = args.index('--ticks'); limit = int(args[i + 1]); del args[i:i + 2]
    ref, port = load(args[0]), load(args[1])
    n = min(len(ref), len(port)) if limit is None else min(limit, len(ref), len(port))
    xoff, same, first = None, 0, None
    for k in range(n):
        a, b = ref[k], port[k]
        ok = len(a) == len(b)
        if ok:
            for wa, wb in zip(a, b):
                if wa[0] != wb[0]:
                    ok = False; break
                va, vb = wa[1], wb[1]
                if isinstance(va, tuple) and isinstance(vb, tuple) and va[0] == vb[0] == 'X':
                    d = va[1] - vb[1]
                    if xoff is None:
                        xoff = d
                    if d != xoff:
                        ok = False; break
                elif va != vb:
                    ok = False; break
        if ok:
            same += 1
        elif first is None:
            first = k
    print(f'ticks: reference {len(ref)}, port {len(port)}, compared {n}, identical {same}')
    if xoff is not None:
        print(f'player memory: reference = port {xoff:+#x}')
    if first is None:
        print('no difference')
        return
    print(f'first difference at tick {first}:')
    for k in range(max(0, first - 1), min(n, first + 2)):
        print(f'  tick {k}')
        print('    ref : ' + ' '.join(show(w, xoff) for w in ref[k]))
        print('    port: ' + ' '.join(show(w, xoff) for w in port[k]))
    sys.exit(1)


main()
