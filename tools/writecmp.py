"""Classify how a port's Paula writes differ from a reference's: values first, then timing.

    python3 -I tools/writecmp.py REFERENCE.log PORT.log [--tick-gap CLOCKS] [--json]
                                 [--dma REFERENCE.dma PORT.dma]

The logs are DELI_WRITE_LOG output (`ADDRESS VALUE COLOUR-CLOCK`, a `# module BASE SIZE`
line) or UADE_WRITE_LOG output of the patched uade123 (`dffRRR VVVV CYCLE`, `F...` fetch
lines, no module line). A line `ffffff 0000 CLOCK` marks the song start (the clock where the
engine's audio output begins, `Engine::song_starts`); the committed engine does not write it,
a survey build that pushes it there does. With a marker on both sides times count from it,
else from each side's first voice DMA on (and the song-start delta then means nothing).
VOL compares as Paula reads it (7 bits, 64 and up full volume).

--dma adds the DELI_DMA_LOG comparison of the same two runs: per voice the first fetched
word that differs and the first fetch made at another time (the sample data a player
writes into chip RAM, a synthesised waveform or a mixing buffer, shows only there).

Only the audio state counts: AUDxLC (LCH and LCL joined, at the LCL write), LEN, PER, VOL,
DAT, each voice's DMA bit (DMACON split per voice: DMA0..DMA3), and an Eagleplayer
amplifier's virtual voices (0xD00000 | voice << 4 | register, as replay-cmp.py; VLOCn ...).
Writes before the song start are left out, but the state they leave is compared
(`prestate_equal`). A write that does not change its register (a voice's volume written
again with the same value, DMA on for a voice already on) is left out too: it does not
change what Paula plays, only when the code runs. What remains is each register's
sequence of values; the two sides compare register by register (so a different order of
writes within a tick is no value difference). A sample address inside the module compares
as its offset in the module (when both logs name it and both addresses are in it); any
other one (a player's own buffers, which a port may keep inside or past its copy of the
module, or a packed module the port converts to another layout) must stand for one port
address and back (a one-to-one map fixed at first use; `addresses_remapped` counts them,
`module_offsets_moved` the ones inside the module on both sides at different offsets).
Bit 0 of an address is dropped, as Paula does. Values: the first register whose sequence differs, at which reference
time; a sequence one side ends earlier within the last LAST_S seconds is the render's cut.

Timing, over the matched writes (each register's k-th change on both sides, up to that
register's first difference): song start (first matched write after the marker, each
side), the reference's tick period (median gap between tick starts; a tick is a run of
writes closer than --tick-gap, 20000 by default) against the port's (its own writes
grouped the same way), the drift of the port's tick starts (least-squares slope,
colour clocks per tick), the in-tick offsets (each write's delay after its tick's first
write, port minus reference: median and max |delta|, and the median per register kind),
the first matched write whose time is not the reference's plus the song-start delta, and
the DMA off-to-on gaps of each voice that differ by more than DMA_SLACK colour clocks.
"""
import json, sys
import statistics as st

TICK_GAP = 20000
VIRT = 0xD00000
START = 0xFFFFFF
DMA_SLACK = 300
LAST_S = 0.3
CC = 3546895
REGS = {0: 'LCH', 2: 'LCL', 4: 'LEN', 6: 'PER', 8: 'VOL', 10: 'DAT'}


def load(path):
    """(events [(name, value, t)], module (base, size) or None, start clock or None)."""
    mod, ev, hi, start = None, [], {}, None
    for line in open(path, errors='replace'):
        f = line.split()
        if not f:
            continue
        if f[0] == '#':
            if len(f) >= 4 and f[1] == 'module':
                mod = (int(f[2], 16), int(f[3]))
            continue
        if f[0][0] == 'F' and len(f[0]) <= 2:
            continue  # uade fetch lines
        try:
            a, v, t = int(f[0], 16), int(f[1], 16), int(f[2])
        except (ValueError, IndexError):
            continue
        if a == START:
            start = t
            continue
        if VIRT <= a < VIRT + 0x200 and a & 0xF <= 0xA:
            ch, r = f'V{(a - VIRT) >> 4}', a & 0xF
            if r == 0xA:
                ev.append((f'DMA{ch}', 1 if v & 0x8000 else 0, t))
                continue
        elif 0xDFF0A0 <= a < 0xDFF0E0:
            ch, r = str((a - 0xDFF0A0) >> 4), (a - 0xDFF0A0) & 0xF
        elif a == 0xDFF096:
            for c in range(4):
                if v & 1 << c:
                    ev.append((f'DMA{c}', 1 if v & 0x8000 else 0, t))
            continue
        else:
            continue
        name = REGS.get(r)
        if name is None:
            continue
        if name == 'LCH':
            hi[ch] = v
            continue
        if name == 'LCL':
            ev.append((f'LOC{ch}', ((hi.get(ch, 0) << 16) | v) & ~1, t))  # Paula ignores bit 0
            continue
        if name == 'VOL':
            v = min(v & 0x7F, 64)  # Paula reads 7 bits, 64 and up is full volume
        ev.append((f'{name}{ch}', v, t))
    return ev, mod, start


def rebase(ev, start):
    """Times from `start` (the song-start marker), or else from the first voice's DMA on."""
    if start is None:
        start = next((t for n, v, t in ev if n.startswith('DMA') and v), 0)
    return [(n, v, t - start) for n, v, t in ev]


def effective(ev):
    """(per-register lists of (value, t) of the changes after the start, state at the start)."""
    state, pre, regs = {}, {}, {}
    for n, v, t in ev:
        if t <= 0:  # at the start clock: written before the marker
            pre[n] = v
            state[n] = v
            continue
        if state.get(n, 0) == v:  # Paula's registers are 0 after the engine's reset
            continue
        state[n] = v
        regs.setdefault(n, []).append((v, t))
    return regs, pre


def norm(addr, mod):
    if addr == 0:
        return ('Z', 0)
    if mod and mod[0] <= addr < mod[0] + mod[1]:
        return ('M', addr - mod[0])
    return ('X', addr)


def tick_starts(times, gap):
    out, last = [], None
    for t in sorted(times):
        if last is None or t - last > gap:
            out.append(t)
        last = t
    return out


def compare(ref, port, rmod, pmod, gap=TICK_GAP):
    if not (rmod and pmod):
        rmod = pmod = None
    pages, remapped, moved = {}, set(), set()

    def same(name, a, b):
        if name.startswith('LOC'):
            x, y = norm(a, rmod), norm(b, pmod)
            if x == y:
                return True
            if x[0] == y[0] == 'Z':
                return False
            if 'Z' in (x[0], y[0]):
                return False
            # a player's own buffer (on either side, in or past the module's area), or a sample the
            # port keeps at another offset (a packed module converted to another layout): each
            # reference address stands for one port address and back
            if pages.setdefault(('r', a), b) != b or pages.setdefault(('p', b), a) != a:
                return False
            (moved if x[0] == y[0] == 'M' else remapped).add(a)
            return True
        return a == b

    rr, rpre = effective(ref)
    pr, ppre = effective(port)
    res = {'writes_ref': len(ref), 'writes_port': len(port),
           'changes_ref': sum(map(len, rr.values())), 'changes_port': sum(map(len, pr.values()))}
    pre_keys = rpre.keys() | ppre.keys()
    res['prestate_equal'] = all(same(k, rpre.get(k, 0), ppre.get(k, 0)) for k in pre_keys)
    tend = min(max((t for _, _, t in ref), default=0), max((t for _, _, t in port), default=0)) - LAST_S * CC
    pairs, diffs = [], []
    for name in sorted(rr.keys() | pr.keys()):
        a, b = rr.get(name, []), pr.get(name, [])
        n = min(len(a), len(b))
        k = next((i for i in range(n) if not same(name, a[i][0], b[i][0])), None)
        if k is None and len(a) != len(b):
            longer = a if len(a) > len(b) else b
            if longer[n][1] < tend:
                k = n
        for i in range(n if k is None else k):
            pairs.append((a[i][1], b[i][1], name))
        if k is not None:
            ta = a[k][1] if k < len(a) else None
            tb = b[k][1] if k < len(b) else None
            diffs.append(dict(reg=name, index=k, t_s=round((ta if ta is not None else tb) / CC, 3),
                              ref=(f'{a[k][0]:x}' if k < len(a) else 'none'),
                              port=(f'{b[k][0]:x}' if k < len(b) else 'none'),
                              changes=(len(a), len(b))))
    diffs.sort(key=lambda d: d['t_s'])
    res['values_equal'] = not diffs
    res['addresses_remapped'] = len(remapped)
    res['module_offsets_moved'] = len(moved)
    res['regs_differing'] = len(diffs)
    res['first_value_diff'] = diffs[0] if diffs else None
    res['value_diffs'] = diffs[:6]
    if not pairs:
        return res
    pairs.sort()
    res['matched'] = len(pairs)
    res['matched_until_s'] = round(diffs[0]['t_s'], 2) if diffs else round(pairs[-1][0] / CC, 2)
    d0 = pairs[0][1] - pairs[0][0]
    res['song_start_ref'] = pairs[0][0]
    res['song_start_port'] = pairs[0][1]
    res['song_start_delta'] = d0
    # ticks of the reference
    starts = tick_starts([p[0] for p in pairs], gap)
    gaps = [b - a for a, b in zip(starts, starts[1:])]
    res['tick_period_ref'] = st.median(gaps) if gaps else None
    ps = tick_starts([p[1] for p in pairs], gap)
    gp = [b - a for a, b in zip(ps, ps[1:])]
    res['tick_period_port'] = st.median(gp) if gp else None
    res['ticks'] = (len(starts), len(ps))
    import bisect
    first_of = {}
    for tr, tp, name in pairs:
        k = bisect.bisect_right(starts, tr) - 1
        first_of.setdefault(k, (tr, tp))
    ks = sorted(first_of)
    dk = [first_of[k][1] - first_of[k][0] for k in ks]
    xr = [first_of[k][0] for k in ks]
    if len(ks) > 2 and res['tick_period_ref']:
        mx, my = st.mean(xr), st.mean(dk)
        sxx = sum((x - mx) ** 2 for x in xr)
        slope = sum((x - mx) * (y - my) for x, y in zip(xr, dk)) / sxx if sxx else 0.0
        res['tick_drift_per_tick'] = round(slope * res['tick_period_ref'], 2)
        res['tick_start_delta_median'] = st.median(dk)
        res['tick_start_delta_range'] = (min(dk), max(dk))
    intick, kinds = [], {}
    for tr, tp, name in pairs:
        k = bisect.bisect_right(starts, tr) - 1
        r0, p0 = first_of[k]
        x = (tp - p0) - (tr - r0)
        intick.append(abs(x))
        kinds.setdefault(name.rstrip('0123456789').rstrip('V'), []).append(x)
    res['intick_abs_median'] = st.median(intick)
    res['intick_abs_max'] = max(intick)
    res['intick_nonzero_pct'] = round(100.0 * sum(1 for x in intick if x) / len(intick), 1)
    res['intick_by_reg'] = {k: st.median(v) for k, v in sorted(kinds.items())}
    ft = next(((tr, tp, n) for tr, tp, n in pairs if tp - tr != d0), None)
    if ft:
        res['first_timing_diff'] = dict(reg=ft[2], t_s=round(ft[0] / CC, 3), delta=(ft[1] - ft[0]) - d0)
    # DMA off-to-on gaps: consecutive off/on changes of a voice's DMA bit
    gaps_d, bad = 0, 0
    for name in rr:
        if not name.startswith('DMA'):
            continue
        a, b = rr[name], pr.get(name, [])
        lim = min(len(a), len(b))
        lim = next((i for i in range(lim) if a[i][0] != b[i][0]), lim)
        for i in range(1, lim):
            if a[i][0] == 1 and a[i - 1][0] == 0:
                gaps_d += 1
                if abs((b[i][1] - b[i - 1][1]) - (a[i][1] - a[i - 1][1])) > DMA_SLACK:
                    bad += 1
    res['dma_gaps'] = gaps_d
    res['dma_gaps_off'] = bad
    return res


def load_dma(path, start):
    """DELI_DMA_LOG (`CLOCK CHANNEL ADDRESS WORD`): per voice [(t from start, address, word)]."""
    ch = {}
    for line in open(path):
        f = line.split()
        if len(f) == 4:
            ch.setdefault(int(f[1]), []).append((int(f[0]) - start, int(f[2], 16), int(f[3], 16)))
    return ch


def compare_dma(rpath, ppath, rs, ps, rmod, pmod):
    """What Paula fetched, voice by voice: the first word that differs and the first fetch at another
    time (both from the song start), as seconds of the reference; None where equal over the shorter log."""
    r, p = load_dma(rpath, rs), load_dma(ppath, ps)
    out = {}
    for c in sorted(r.keys() | p.keys()):
        a, b = r.get(c, []), p.get(c, [])
        a = [x for x in a if x[0] >= 0]
        b = [x for x in b if x[0] >= 0]
        n = min(len(a), len(b))
        w = next((i for i in range(n) if a[i][2] != b[i][2]), None)
        t = next((i for i in range(n) if a[i][0] != b[i][0]), None)
        out[c] = dict(fetches=(len(a), len(b)),
                      first_word_diff_s=None if w is None else round(a[w][0] / CC, 4),
                      first_time_diff_s=None if t is None else round(a[t][0] / CC, 4),
                      time_delta_there=None if t is None else b[t][0] - a[t][0])
    return out


def main():
    args = sys.argv[1:]
    gap, js = TICK_GAP, '--json' in args
    if js:
        args.remove('--json')
    if '--tick-gap' in args:
        i = args.index('--tick-gap'); gap = int(args[i + 1]); del args[i:i + 2]
    dma = None
    if '--dma' in args:
        i = args.index('--dma'); dma = args[i + 1:i + 3]; del args[i:i + 3]
    (ref, rmod, rs), (port, pmod, ps) = load(args[0]), load(args[1])
    if rs is None or ps is None:
        rs = ps = None  # no common origin: both from their first DMA on
    res = compare(rebase(ref, rs), rebase(port, ps), rmod, pmod, gap)
    res['clock_origin'] = 'song start marker' if rs is not None else 'first DMA on'
    if dma:
        res['dma'] = compare_dma(dma[0], dma[1], rs or 0, ps or 0, rmod, pmod)
    if js:
        print(json.dumps(res))
    else:
        for k, v in res.items():
            print(f'{k}: {v}')


if __name__ == '__main__':
    main()
