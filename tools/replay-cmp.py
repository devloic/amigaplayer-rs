"""Compare the audio register writes of two runs of the same module, tick by tick.

    python3 -I tools/replay-cmp.py REFERENCE.log PORT.log [--ticks N]

The logs are DELI_WRITE_LOG output. Sample addresses are made comparable: one
inside the module becomes its offset in the module ("M+off", from the log's
"# module" line), address 0 stays 0, and every other one (a player's own
waveforms) must differ between the two runs by one constant, which is reported,
or be the same in both (an address past the module's end, computed from it).
Writes are grouped into ticks by the gaps between them; ticks are compared in
order, and the first one that differs is printed with both sides.

The time between a voice's DMA off and its DMA on is compared too: Paula stops
a playing voice only at the end of its word, so with too short a gap a note does
not restart, though every write is the same. A gap counts as different when it
is off by more than DMA_SLACK colour clocks and the shorter of the two is less
than a word of the voice at the period it played (2 * period colour clocks), so
that the note may restart in one run and not in the other. A port's timing is
the original's only roughly, so a few such gaps are reported but pass; more than
DMA_BAD of them (a port writing a whole tick at one moment) count as a difference.
"""
import sys

TICK_GAP = 20000  # colour clocks: a tick's writes are far closer together than this
DMA_SLACK = 300  # colour clocks: a port's timing is the original's only roughly
DMA_BAD = 0.5  # measured: ports with no time there differ in 79-100%, timed ones in 0-29%


def load(path):
    base, size, ticks, last = 0, 0, [], None
    hi, gaps, off, per = {}, [], {}, [0] * 4
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
            gaps.append({})
            off = {}
        last = t
        if a == 0xDFF096:
            for ch in range(4):
                if v & 1 << ch:
                    if v & 0x8000 and ch in off:
                        t0, word = off.pop(ch)
                        gaps[-1].setdefault(ch, (t - t0, word))
                    elif not v & 0x8000:
                        off[ch] = (t, 2 * per[ch])
        if 0xDFF0A0 <= a < 0xDFF0E0 and (a - 0xDFF0A0) % 16 in (0, 2):
            ch = (a - 0xDFF0A0) // 16
            if (a - 0xDFF0A0) % 16 == 0:
                hi[ch] = v
                continue
            addr = hi.pop(ch, 0) << 16 | v
            if addr == 0:  # a pointer never set, not player memory
                ticks[-1].append((f'LOC{ch}', ('Z', 0)))
            elif base <= addr < base + size:
                ticks[-1].append((f'LOC{ch}', ('M', addr - base)))
            else:
                ticks[-1].append((f'LOC{ch}', ('X', addr)))
            continue
        if 0xDFF0A0 <= a < 0xDFF0E0 and (a - 0xDFF0A0) % 16 == 6:
            per[(a - 0xDFF0A0) // 16] = v
        if a < 0xDFF0A0:  # DMACON, INTENA, INTREQ, ADKCON
            name = {0xDFF096: 'DMACON', 0xDFF09A: 'INTENA', 0xDFF09C: 'INTREQ', 0xDFF09E: 'ADKCON'}.get(a, f'{a:06x}')
        else:
            name = f'{["LEN", "PER", "VOL", "DAT"][((a - 0xDFF0A0) % 16 - 4) // 2]}{(a - 0xDFF0A0) // 16}'
        ticks[-1].append((name, v))
    return ticks, gaps


def show(w, xoff):
    n, v = w
    if isinstance(v, tuple):
        return f'{n}={v[0]}+{v[1]:x}' if v[0] == 'M' else f'{n}=0' if v[0] == 'Z' else f'{n}=X:{v[1]:06x}'
    return f'{n}={v:04x}'


def main():
    args = sys.argv[1:]
    limit = None
    if '--ticks' in args:
        i = args.index('--ticks'); limit = int(args[i + 1]); del args[i:i + 2]
    (ref, rgaps), (port, pgaps) = load(args[0]), load(args[1])
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
                    if d == 0:  # the same address in both: past the module's end
                        continue
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
    both = [(k, ch) for k in range(n) for ch in sorted(rgaps[k].keys() & pgaps[k].keys())]
    late = [(k, ch) for k, ch in both
            if abs(rgaps[k][ch][0] - pgaps[k][ch][0]) > DMA_SLACK
            and min(rgaps[k][ch][0], pgaps[k][ch][0]) < rgaps[k][ch][1]]
    bad_timing = len(late) > DMA_BAD * len(both)
    print(f'ticks: reference {len(ref)}, port {len(port)}, compared {n}, identical {same}')
    if xoff is not None:
        print(f'player memory: reference = port {xoff:+#x}')
    if late:
        k, ch = late[0]
        print(f'DMA off to on differs in {len(late)} of {len(both)}, first at tick {k} on voice {ch} '
              f'(a word takes {rgaps[k][ch][1]}): reference {rgaps[k][ch][0]}, '
              f'port {pgaps[k][ch][0]} colour clocks' + (' (too many)' if bad_timing else ''))
    if first is None:
        if bad_timing:
            sys.exit(1)
        print('no difference')
        return
    print(f'first difference at tick {first}:')
    for k in range(max(0, first - 1), min(n, first + 2)):
        print(f'  tick {k}')
        print('    ref : ' + ' '.join(show(w, xoff) for w in ref[k]))
        print('    port: ' + ' '.join(show(w, xoff) for w in port[k]))
    sys.exit(1)


main()
