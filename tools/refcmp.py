"""Bit-compare amigaplayer-rs's open replayers with their originals, and flag inconsistencies.

    python3 -I tools/refcmp.py --all -j 8                 # whole list, against the baseline
    python3 -I tools/refcmp.py --fmt tfmx --fmt 'corpus: emod' -j 4
    python3 -I tools/refcmp.py --all --update-baseline    # accept this run as the baseline

Each file of the list (default: the listen page's files.json: entries
{fmt, path, id}, paths possibly .xz) is rendered twice with identical output
settings, WAV for WAV:

  port       amigaplayer-rs --open-replayers, with every player directory empty
             (so no 68k player stands in for the port)
  reference  the ORIGINAL player: the same binary without --open-replayers,
             running the real Eagleplayer/DeliPlayer on the emulated Amiga with
             the same output stage (first of EP206, then the Eagleplayers UADE
             ships, whose player runs on the 68k); for the formats the libxmp
             port plays, libxmp 4.7.4 itself (the C harness `render`, 48 kHz
             s16, its defaults).

The two sample streams are compared and each file gets a class:

  IDENTICAL   every sample equal, same length
  LAG-ONLY    equal after a shift of at most LAG_MAX frames
  NEAR        differences of at most NEAR_MAXABS LSB (rounding)
  SIMILAR     not equal, but the same sound: level within SIMILAR_DB, every band
              within SIMILAR_DB, same length (write-timing/phase differences)
  inconsistencies (what the baseline gate fails on):
  FAIL        a side did not render
  STAGE-MISMATCH  the two `output:` lines differ in a stage that must be equal
  SILENT      one side silent, or silent for stretches the other is not
  LENGTH      song end / loop differs by more than LENGTH_TOL_S (or LENGTH_TOL_PCT)
  FILTER      spectral tilt: band-energy differences spread more than FILTER_DB
              (the LED / A500 kind: 2-5 kHz vs the low band)
  LEVEL       overall RMS differs by more than LEVEL_DB
  PITCH       dominant spectral peak shifted by more than PITCH_PCT
  TEMPO       the loudness envelopes' alignment drifts by TEMPO_DRIFT_S or more over the render
              (tempo_drift_s, positive: the port lags further behind the original as it goes)
  DIFF        anything else
  NOREF       no original available (neutral: not counted as an inconsistency)

Outputs (in --out): refcmp.tsv, summary.txt, report.html. The baseline
(tools/refcmp-baseline.tsv) holds each file's class and numbers; a run reports
NEW inconsistencies (bad now, good or absent before) and FIXED ones, and exits 1
on a new one. Renders are cached (--cache) by binary mtime and size, settings,
module and player directories.
"""
import argparse, concurrent.futures as cf, hashlib, html, json, lzma, os, re, shutil, subprocess, sys, wave
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
SCRATCH = '/media/lolo/kingston2tb/claude-scratch/deliplayerrust'
DEFAULT_LIST = SCRATCH + '/listen/files.json'
DEFAULT_BIN = os.path.join(HERE, '..', 'target', 'release', 'amigaplayer-rs')
EP206 = SCRATCH + '/eplicense/x/Eagleplayer_2.06/Eagleplayers'
UADE_PLAYERS = os.path.expanduser('~/.local/opt/uade/share/uade/players')
# DeliPlayers of the DeliTracker PlayerStore: the original of the ports verified against one
DELI_STORE = os.path.expanduser('~/.config/deliplayer/DeliPlayers/PlayerStore')
# DeliTracker's NotePlayer genies (4Voices, 8Voices, 32Voices ...)
NOTE_GENIES = SCRATCH + '/extract/DeliTracker_II/Files/x_genies/DeliGenies'
XMP_ORACLE = SCRATCH + '/ports/x01-core/harness/render'
LISTEN_PY = SCRATCH + '/listen/listen.py'
BASELINE = os.path.join(HERE, 'refcmp-baseline.tsv')

# ---- thresholds -------------------------------------------------------------
LAG_MAX = 4000          # frames: largest shift searched (83 ms at 48 kHz)
LAG_PCT = 99.9          # % of frames equal after the shift for LAG-ONLY
NEAR_MAXABS = 2         # LSB: largest difference of a NEAR file
SIMILAR_DB = 1.0        # dB: band and level tolerance of SIMILAR
LEVEL_DB = 1.5          # dB: RMS difference that is a LEVEL inconsistency
FILTER_DB = 3.0         # dB: spread of the per-band differences for FILTER
LENGTH_TOL_S = 0.5      # s: length difference tolerated ...
LENGTH_TOL_PCT = 2.0    # ... or this % of the longer side, whichever is more
SILENT_RMS = 3.0        # LSB: a 100 ms window below this is silent
SILENT_FRAC = 0.20      # difference of the silent-window fractions for SILENT
PITCH_CENTS = 20        # cents: shift of the whole log-frequency spectrum for PITCH
PITCH_GAIN = 0.02       # ... which must raise the spectra's correlation by this much over no shift
TEMPO_DRIFT_S = 0.04    # s: drift of the loudness envelopes' alignment over the render for TEMPO
TEMPO_SEGMENTS = 8      # segments the drift is measured over
TEMPO_TAU = 0.75        # ... and the alignments must trend (Kendall's tau over the segments) at least this much
TEMPO_MIN_CORR = 0.5    # segments whose envelopes correlate less than this are left out
# ports whose original is UADE's Eagleplayer, not EP206's older one of the same format:
# Hippel 7V follows V1.3 (UADE), EP206 has V1.0, which keeps another tempo and LED;
# Sound Master and SCUMM were written against the 68k players UADE ships (EP206's
# SoundMaster is another build, SCUMM's differs in its sweeps)
UADE_FIRST = ('hippel7v', 'soundmaster', 'scumm')
BAND_EDGES = [20, 500, 2000, 5000, 10000, 20000]   # Hz
BAND_MIN_DB = -75.0     # bands quieter than this (re full scale) are ignored
# builtin-voices: a reference played through our own NotePlayer renderer is not the original's
MUST_MATCH = ('filter', 'led', 'sep', 'mono', 'resampler', 'volume', 'interp', 'xmp-filter', 'builtin-voices')

GOOD = ('IDENTICAL', 'LAG-ONLY', 'NEAR', 'SIMILAR', 'NOREF')
CLASSES = GOOD + ('FAIL', 'STAGE-MISMATCH', 'SILENT', 'LENGTH', 'FILTER', 'LEVEL', 'PITCH', 'TEMPO', 'DIFF')


# ---- comparison -------------------------------------------------------------
def read_wav(path):
    """(rate, int16 array of shape (n, 2))."""
    with wave.open(path, 'rb') as w:
        ch, rate, n = w.getnchannels(), w.getframerate(), w.getnframes()
        a = np.frombuffer(w.readframes(n), dtype='<i2')
    a = a.reshape(-1, ch)
    if ch == 1:
        a = np.repeat(a, 2, axis=1)
    return rate, a[:, :2]


def read_raw(path):
    return 48000, np.fromfile(path, dtype='<i2').reshape(-1, 2)


def _bands(mid, rate):
    n = 8192
    if len(mid) < n:
        mid = np.pad(mid, (0, n - len(mid)))
    win = np.hanning(n)
    nb = len(mid) // n
    frames = mid[:nb * n].reshape(nb, n) * win
    p = (np.abs(np.fft.rfft(frames, axis=1)) ** 2).mean(axis=0) / (n * n * 0.375) / (32768.0 ** 2)
    f = np.fft.rfftfreq(n, 1.0 / rate)
    out = []
    for lo, hi in zip(BAND_EDGES, BAND_EDGES[1:]):
        out.append(10 * np.log10(p[(f >= lo) & (f < hi)].sum() + 1e-20))
    pk = f[50 * n // rate:][np.argmax(p[50 * n // rate:])] if p.max() > 0 else 0.0
    return out, float(pk)


def _envelope(mid, rate):
    """Onset strength at 100 Hz: the rise of the 10 ms mean absolute level."""
    hop = rate // 100
    n = len(mid) // hop
    env = np.abs(mid[:n * hop]).reshape(n, hop).mean(axis=1)
    rise = np.maximum(np.diff(env, prepend=env[:1]), 0)
    return np.convolve(rise, np.ones(5) / 5, mode='same')  # 50 ms, so a slight drift still correlates


def _tempo_drift(ma, mb, rate):
    """(drift in seconds, segments used): how far the best alignment of the two
    loudness envelopes moves from the first usable segment to the last (the median
    slope of the segments' alignments times the span). A tempo difference makes it
    grow over the render; equal tempi keep it near 0."""
    ea, eb = _envelope(ma, rate), _envelope(mb, rate)
    n = min(len(ea), len(eb))
    seg = n // TEMPO_SEGMENTS
    w = 50  # 0.5 s search either way
    if seg < 4 * w:
        return 0.0, 0
    def align(k, rng):
        """(best lag, its correlation) of segment k's envelope against b's over the lags in rng."""
        x = ea[k * seg:(k + 1) * seg]
        x0 = x - x.mean()
        best, bl = -2.0, 0
        if not x0.any():
            return 0, best
        for l in rng:
            o = k * seg + l
            if o < 0 or o + len(x) > len(eb):
                continue
            y0 = eb[o:o + len(x)] - eb[o:o + len(x)].mean()
            den = np.sqrt((x0 ** 2).sum() * (y0 ** 2).sum())
            c = (x0 * y0).sum() / den if den else -2.0
            if c > best:
                best, bl = c, l
        return bl, best

    # the segment that lines up best (0.5 s either way) is the anchor; from it the neighbours are
    # followed outwards, 80 ms around the last alignment, so that a rhythm's neighbouring beat is
    # never taken instead, and a bad first segment does not spoil the rest
    free = [align(k, range(-w, w + 1)) for k in range(TEMPO_SEGMENTS)]
    a0 = max(range(TEMPO_SEGMENTS), key=lambda k: free[k][1])
    found = {a0: free[a0]} if free[a0][1] >= TEMPO_MIN_CORR else {}
    for step in (1, -1):
        prev = free[a0][0]
        k = a0 + step
        while found and 0 <= k < TEMPO_SEGMENTS:
            bl, c = align(k, range(prev - 8, prev + 9))
            if c >= TEMPO_MIN_CORR:
                found[k] = (bl, c)
                prev = bl
            k += step
    ks = sorted(found)
    lags = [found[k][0] for k in ks]
    if len(lags) < 4:
        return 0.0, len(lags)  # too few segments that line up to tell a drift from noise
    # the median slope of all pairs (Theil-Sen) times the span: one segment aligned a beat off,
    # or a start-up offset between the two sides, must not read as a drift, a steady slope does
    sl = sorted((lags[j] - lags[i]) / (ks[j] - ks[i]) for i in range(len(lags)) for j in range(i + 1, len(lags)))
    tau = (sum(x > 0 for x in sl) - sum(x < 0 for x in sl)) / len(sl)
    if abs(tau) < TEMPO_TAU:
        return 0.0, len(lags)  # the alignments wander, they do not trend: not a tempo difference
    slope = sl[len(sl) // 2] if len(sl) % 2 else (sl[len(sl) // 2 - 1] + sl[len(sl) // 2]) / 2
    return slope * (ks[-1] - ks[0]) / 100.0, len(lags)


def _pitch_shift(ma, mb, rate):
    """(cents, gain): the shift of b's log-frequency spectrum that best matches a's,
    and how much it raises their correlation over no shift."""
    n = 8192
    def spec(m):
        if len(m) < n:
            m = np.pad(m, (0, n - len(m)))
        nb = len(m) // n
        fr = m[:nb * n].reshape(nb, n) * np.hanning(n)
        return (np.abs(np.fft.rfft(fr, axis=1)) ** 2).mean(axis=0)
    f = np.fft.rfftfreq(n, 1.0 / rate)
    grid = 60.0 * 2 ** (np.arange(0, 7 * 120) / 120.0)  # 60 Hz to 7.7 kHz in 10-cent steps
    la = np.log10(np.interp(grid, f, spec(ma)) + 1e-9)
    lb = np.log10(np.interp(grid, f, spec(mb)) + 1e-9)
    def corr(s):
        x, y = (la[s:], lb[:len(lb) - s]) if s >= 0 else (la[:s], lb[-s:])
        x, y = x - x.mean(), y - y.mean()
        d = np.sqrt((x * x).sum() * (y * y).sum())
        return (x * y).sum() / d if d else 0.0
    c0 = corr(0)
    best = max(range(-12, 13), key=corr)  # within ±120 cents
    return best * 10, corr(best) - c0


def _silent_frac(mid, rate):
    w = rate // 10
    n = len(mid) // w
    if n == 0:
        return 1.0
    rms = np.sqrt((mid[:n * w].reshape(n, w) ** 2).mean(axis=1))
    return float((rms < SILENT_RMS).mean())


def _best_lag(a, b):
    """Shift s (b[i+s] ~ a[i]) with the highest cross-correlation of the mid channels."""
    n = min(len(a), len(b), 10 * 48000)
    if n < 2 * LAG_MAX:
        return 0
    x = a[:n].mean(axis=1)
    y = b[:n].mean(axis=1)
    size = 1 << (2 * n - 1).bit_length()
    c = np.fft.irfft(np.fft.rfft(y, size) * np.conj(np.fft.rfft(x, size)), size)
    cand = np.concatenate([c[:LAG_MAX + 1], c[size - LAG_MAX:]])
    lags = np.concatenate([np.arange(LAG_MAX + 1), np.arange(-LAG_MAX, 0)])
    return int(lags[np.argmax(cand)])


def _aligned(a, b, s):
    """a and b with b shifted by s (b[i+s] against a[i])."""
    if s >= 0:
        b2, a2 = b[s:], a
    else:
        a2, b2 = a[-s:], b
    n = min(len(a2), len(b2))
    return a2[:n], b2[:n]


def compare(a, b, rate=48000):
    """Numbers for the pair a (reference) and b (port), int16 (n, 2) arrays."""
    # A 68k player cannot tell the engine that its song has ended: the original goes on rendering
    # to the time limit while the port reports the end and stops. When all the reference has
    # past the port's end is digital silence, nothing the port would have played is missing,
    # and the two are compared up to the port's end.
    if 0 < len(b) < len(a) and not a[len(b):].any():
        a = a[:len(b)]
    r = {'len_a': len(a), 'len_b': len(b)}
    n = min(len(a), len(b))
    x, y = a[:n].astype(np.int32), b[:n].astype(np.int32)
    d = np.abs(x - y)
    r['identical'] = bool(len(a) == len(b) and n and not d.any())
    bad = np.nonzero(d.any(axis=1))[0] if n else []
    r['first_diff'] = int(bad[0]) if len(bad) else -1
    r['first_diff_s'] = round(r['first_diff'] / rate, 4) if len(bad) else -1
    r['pct_identical'] = round(100.0 * (1 - len(bad) / n), 3) if n else 0.0
    r['maxabs'] = int(d.max()) if n else 0
    ma, mb = a.astype(np.float64).mean(axis=1), b.astype(np.float64).mean(axis=1)
    rms = lambda v: float(np.sqrt((v.astype(np.float64) ** 2).mean())) if len(v) else 0.0
    ra, rb = rms(a), rms(b)
    r['rms_a'], r['rms_b'] = round(ra, 1), round(rb, 1)
    r['level_db'] = round(20 * np.log10((rb + 1e-9) / (ra + 1e-9)), 2) if ra > 0 and rb > 0 else None
    # lag
    r['lag'], r['lag_pct'], r['lag_maxabs'] = 0, r['pct_identical'], r['maxabs']
    if not r['identical'] and n > 2 * LAG_MAX and ra > 0 and rb > 0:
        s = _best_lag(a, b)
        if s:
            a2, b2 = _aligned(a, b, s)
            dd = np.abs(a2.astype(np.int32) - b2.astype(np.int32))
            r['lag'] = s
            r['lag_pct'] = round(100.0 * (1 - dd.any(axis=1).mean()), 3) if len(dd) else 0.0
            r['lag_maxabs'] = int(dd.max()) if len(dd) else 0
    # spectrum, silence, tempo drift, pitch shift
    m = min(len(ma), len(mb))
    ba, pa = _bands(ma[:m], rate)
    bb, pb = _bands(mb[:m], rate)
    r['bands_db'] = [round(float(q - p), 2) if min(p, q) > BAND_MIN_DB else None for p, q in zip(ba, bb)]
    r['peak_a'], r['peak_b'] = round(pa, 1), round(pb, 1)
    r['silent_a'], r['silent_b'] = round(_silent_frac(ma, rate), 3), round(_silent_frac(mb, rate), 3)
    r['tempo_drift_s'], r['tempo_segs'] = _tempo_drift(ma[:m], mb[:m], rate)
    r['pitch_cents'], r['pitch_gain'] = _pitch_shift(ma[:m], mb[:m], rate)
    r['pitch_gain'] = round(r['pitch_gain'], 4)
    return r


def classify(r):
    """The class of a compare() result."""
    if r['identical']:
        return 'IDENTICAL'
    la, lb = r['len_a'], r['len_b']
    if r['lag'] and r['lag_pct'] >= LAG_PCT and abs(la - lb) <= abs(r['lag']) + 1:
        return 'LAG-ONLY'
    if la == lb and r['maxabs'] <= NEAR_MAXABS:
        return 'NEAR'
    if r['lag'] and r['lag_maxabs'] <= NEAR_MAXABS and abs(la - lb) <= abs(r['lag']) + 1:
        return 'NEAR'
    sa, sb = r['silent_a'], r['silent_b']
    if (r['rms_a'] == 0) != (r['rms_b'] == 0) or abs(sa - sb) > SILENT_FRAC:
        return 'SILENT'
    tol = max(LENGTH_TOL_S * 48000, LENGTH_TOL_PCT / 100.0 * max(la, lb))
    if abs(la - lb) > tol:
        return 'LENGTH'
    db = [v for v in r['bands_db'] if v is not None]
    lv = r['level_db'] or 0.0
    if db and max(db) - min(db) > FILTER_DB:
        return 'FILTER'
    if abs(lv) > LEVEL_DB:
        return 'LEVEL'
    if abs(r['pitch_cents']) >= PITCH_CENTS and r['pitch_gain'] >= PITCH_GAIN:
        return 'PITCH'
    if max(la, lb) >= 3 * 48000 and abs(r['tempo_drift_s']) >= TEMPO_DRIFT_S:
        return 'TEMPO'
    if abs(lv) <= SIMILAR_DB and all(abs(v) <= SIMILAR_DB for v in db):
        return 'SIMILAR'
    return 'DIFF'


def parse_output(line):
    """`output: k=v k=v ...` as a dict ({} when there is no such line)."""
    if not line:
        return {}
    d = {}
    for w in line.split()[1:]:
        k, _, v = w.partition('=')
        d[k] = v
    return d


def _led_state(line):
    """'on' / 'off' / 'switched' from a `led-switched:` line ('' when unknown)."""
    if 'yes at' in line:
        return 'switched'
    m = re.search(r'led (on|off) throughout', line)
    return m.group(1) if m else ''


def stage_mismatch(oa, ob):
    """The stages in MUST_MATCH whose value differs between two output lines."""
    return [f'{k}:{oa[k]}!={ob[k]}' for k in MUST_MATCH if k in oa and k in ob and oa[k] != ob[k]]


# ---- rendering --------------------------------------------------------------
def _fallback_stage(path, cache):
    d = os.path.join(cache, 'stage', hashlib.md5(path.encode()).hexdigest()[:16])
    name = re.sub(r'\.xz$', '', os.path.basename(path))
    mod = os.path.join(d, name)
    if not os.path.exists(mod):
        os.makedirs(d, exist_ok=True)
        src = path if os.path.exists(path) else path + '.xz'
        b = open(src, 'rb').read()
        if b[:6] == b'\xfd7zXZ\x00':
            b = lzma.decompress(b)
        open(mod, 'wb').write(b)
    return mod


class Ctx:
    def __init__(self, a):
        self.bin = os.path.abspath(a.bin)
        st = os.stat(self.bin)
        self.binkey = f'{int(st.st_mtime)}-{st.st_size}'
        self.secs = a.seconds
        self.flags = a.flags
        self.cache = a.cache
        os.makedirs(self.cache, exist_ok=True)
        self.empty = os.path.join(self.cache, 'empty')
        os.makedirs(self.empty, exist_ok=True)
        self.stage = None
        if os.path.exists(a.listen):
            import importlib.util
            spec = importlib.util.spec_from_file_location('listen_stage', a.listen)
            mod = importlib.util.module_from_spec(spec)
            try:
                spec.loader.exec_module(mod)
                mod.CACHE = os.path.join(self.cache, 'lstage')
                os.makedirs(mod.CACHE, exist_ok=True)
                self.stage = mod.stage
            except Exception as e:  # noqa
                print('listen.py staging unavailable:', e, file=sys.stderr)
        self.oracle = a.oracle


def _run_cached(ctx, key, cmd, env, cwd, wav):
    """Runs cmd (writing wav) unless cached; returns (wav path or None, stderr lines)."""
    h = hashlib.sha1(json.dumps([ctx.binkey, key, cmd, sorted((k, env[k]) for k in env if k in
                                 ('DELIPLAYERS', 'EAGLEPLAYERS', 'DELIGENIES', 'DELI_CPU_SPEED'))]).encode()).hexdigest()[:20]
    out = os.path.join(ctx.cache, h + '.wav')
    err = out + '.err'
    if not (os.path.exists(out) and os.path.exists(err)) and not os.path.exists(err):
        full = [c.replace('@WAV@', out) for c in cmd]
        try:
            r = subprocess.run(full, env=env, cwd=cwd, capture_output=True, text=True, timeout=ctx.secs * 4 + 120)
            text = r.stderr
        except subprocess.TimeoutExpired:
            text = 'timeout\n'
        open(err, 'w').write(text)
    lines = open(err).read().splitlines()
    return (out if os.path.exists(out) else None), lines


def render_engine(ctx, mod, open_replayers, eagle, extra=(), genies=None, fast_cpu=False):
    deli = ctx.empty
    if isinstance(eagle, tuple):   # ('deli', DeliPlayer directory)
        deli, eagle = eagle[1], None
    env = dict(os.environ, SDL_AUDIO_DRIVER='dummy', RUST_LOG='error', DELIPLAYERS=deli,
               DELIGENIES=genies or ctx.empty, EAGLEPLAYERS=eagle or ctx.empty)
    if 'PreTracker' in extra:
        # UADE's PreTracker player has no check: the engine accepts a checkless player for it
        env['DELI_CHECKLESS'] = '1'
    if fast_cpu:
        # the port says its original's code does not fit a 68000's time (see needs_fast_reference_cpu)
        env['DELI_CPU_SPEED'] = '8'
    cmd = [ctx.bin, *(['--open-replayers'] if open_replayers else []), *ctx.flags, *extra,
           '--wav', '@WAV@', '--seconds', str(ctx.secs), mod]
    wav, lines = _run_cached(ctx, mod, cmd, env, os.path.dirname(mod), None)
    play = next((l for l in lines if l.startswith('playing')), None)
    out = next((l for l in lines if l.startswith('output:')), None)
    led = next((l for l in lines if l.startswith('led-switched')), None)
    if (not open_replayers and not genies and out and 'builtin-voices=builtin-noteplayer' in out
            and os.path.isdir(NOTE_GENIES)):
        # a `_note` player the engine played through its own NotePlayer renderer: the original
        # plays it through DeliTracker's NotePlayer genie (the engine picks the cheapest that
        # has enough voices), so give it the genies
        return render_engine(ctx, mod, open_replayers, eagle, extra, NOTE_GENIES, fast_cpu)
    return wav, play, out, led, lines


def render_oracle(ctx, mod):
    h = hashlib.sha1(json.dumps([mod, ctx.secs, os.path.getmtime(ctx.oracle)]).encode()).hexdigest()[:20]
    raw = os.path.join(ctx.cache, h + '.xmp.raw')
    inf = raw + '.txt'
    if not os.path.exists(inf):
        r = subprocess.run([ctx.oracle, mod, raw, str(ctx.secs)], capture_output=True, text=True,
                           cwd=os.path.dirname(mod), timeout=300)
        open(inf, 'w').write(r.stdout.strip())
    return (raw if os.path.exists(raw) else None), open(inf).read()


def _norm(t):
    return re.sub(r'[^a-z0-9]', '', t.lower())


def verified_players(readme=os.path.join(HERE, '..', 'README.md')):
    """{normalised format: player name}: the original each open replayer was checked
    against, from the README's open-replayers table (its reference column starts with
    the player's name and "(Eagleplayer" or "(DeliPlayer")."""
    out = {}
    try:
        lines = open(readme, encoding='utf-8').read().splitlines()
    except OSError:
        return out
    for l in lines:
        c = [x.strip() for x in l.split('|')]
        if not l.startswith('| ') or len(c) < 5:
            continue
        m = re.match(r'^`?([^`(;,]+?)`?\s*\((?:Eagleplayer|DeliPlayer)', c[3])
        if m:
            out[_norm(re.split(r'[(,]', c[1])[0])] = m.group(1)
    return out


VERIFIED = verified_players()


# ports whose original is a PlayerStore DeliPlayer (UADE's Eagleplayer of the format converts
# the bank to a ProTracker module and plays that, so it is no reference): normalised format -> player
DELI_REF = {'amosmusicbank': 'AmosMusicBank'}


def special_reference(mod, fmt):
    """A per-module original where one player does not cover the format: Digital Sound
    Studio modules with a tempo byte play on UADE's DSS 1.0-3.0 player (V1.2), as the
    V1.71 Eagleplayer the port follows for the others reads the tempo as part of the speed."""
    if fmt in DELI_REF and os.path.isdir(DELI_STORE):
        return [('deli', ('deli', DELI_STORE), ('--player', DELI_REF[fmt]))]
    if fmt == 'digitalsoundstudio':
        try:
            with open(mod, 'rb') as fh:
                d = fh.read(10)
        except OSError:
            return None
        if len(d) == 10 and d[8] != 0:
            return [('uade+tempo', UADE_PLAYERS, ('--player', 'Digital Sound Studio'))]
    return None


def find_reference(ctx, mod, po):
    """The original's render for a file whose port output line parses to `po`:
    dict(kind 'engine'|'libxmp'|'none', wav (path), data ((rate, samples)), out, led, player, o, note, noref)."""
    none = dict(kind='none', note='', noref=True)
    if po.get('backend') == 'libxmp-port':
        if not os.path.exists(ctx.oracle):
            return dict(none, note='no libxmp oracle')
        raw, info = render_oracle(ctx, mod)
        if not raw or '\t' not in info:
            return dict(none, note='oracle: ' + info[:100], noref=False)
        return dict(kind='libxmp', wav=raw, data=read_raw(raw), out='libxmp 4.7.4 ' + info.replace('\t', ' '),
                    led='', player='libxmp 4.7.4 (oracle)', o={}, note='')
    want = _norm(po.get('format', ''))
    dirs = (('ep206', EP206), ('uade', UADE_PLAYERS))
    if want in UADE_FIRST:
        dirs = dirs[::-1]
    # the player the port was verified against comes first, then the generic search
    verified = VERIFIED.get(want)
    tries = special_reference(mod, want) or []
    if verified:
        tries += [(t + '+verified', d, ('--player', verified)) for t, d in dirs]
    first = len(tries)
    tries += [(t, d, ()) for t, d in dirs]
    pn = po.get('player', '')
    names = [pn, pn.replace('_', ' '), pn.replace('_', '-')]
    if po.get('backend') == 'builtin-protracker':
        # the built-in ProTracker's original: UADE's PTK-Prowiz replay, the Protracker4 Eagleplayer
        names += ['PTK-Prowiz', 'Protracker4']
    for variant in dict.fromkeys(names):
        tries += [(t + '+player', d, ('--player', variant)) for t, d in dirs]
    fallback = None
    bads = []
    for i, (tag, eagle, extra) in enumerate(tries):
        rw, rplay, rout, rled, rlines = render_engine(ctx, mod, False, eagle, extra, po.get('reference-cpu') == 'fast')
        o = parse_output(rout)
        if not (rw and rplay and o.get('backend') == '68k'):
            continue
        got = re.sub(r'[^a-z0-9]', '', o.get('format', '').lower())
        cand = dict(kind='engine', wav=rw, data=None, out=rout, led=rled or '', player=f'{tag}: {rplay}', o=o, note='')
        # an Eagleplayer that numbers its subsongs from 0xF00 (EP206's Sean Conran) plays no real
        # subsong, and a render that ends within a quarter second of a longer run is a player the
        # engine could not run (or a different song set): try the next original first
        m = re.search(r'subsongs (\d+)\.\.(\d+)', rplay)
        with wave.open(rw, 'rb') as w:
            short = w.getnframes() < w.getframerate() // 4 and ctx.secs >= 5
        bad = short or (m and int(m.group(1)) > 1 and int(m.group(2)) > 255)
        if bad:
            bads.append(cand)
            continue
        if i < first or (want and (want in got or got in want)):
            fallback = cand
            break
        fallback = fallback or cand
    fallback = fallback or (bads[0] if bads else None)
    if not fallback:
        return dict(none, note='no original player plays it in the engine')
    fallback['data'] = read_wav(fallback['wav'])
    if not fallback['data'][1].any():
        return dict(none, note='the original is silent in the engine (' + fallback['player'][:60] + ')')
    return fallback


def process(ctx, f):
    """One file: renders, comparison, class. Returns the result row (a dict)."""
    row = {'id': f['id'], 'fmt': f['fmt'], 'path': f['path'], 'cls': 'FAIL', 'note': ''}
    try:
        path = f['path']
        mod = ctx.stage(path) if ctx.stage else _fallback_stage(path, ctx.cache)
        pw, pplay, pout, pled, plines = render_engine(ctx, mod, True, None)
        row['out_port'] = pout or ''
        row['led_port'] = pled or ''
        if not pw or not pplay:
            row['note'] = 'port: ' + (plines[-1] if plines else 'no output')[:120]
            return row
        row['player'] = pplay
        po = parse_output(pout)
        ref = find_reference(ctx, mod, po)
        if ref['kind'] == 'none':
            row['cls'] = 'NOREF' if ref['noref'] else 'FAIL'
            row['note'] = ref['note']
            return row
        row.update(out_ref=ref['out'], led_ref=ref['led'], ref_player=ref['player'])
        ra, oref = ref['data'], ref['o']
        rb = read_wav(pw)
        if ra[0] != rb[0]:
            row['note'] = f'rates {ra[0]} vs {rb[0]}'
            return row
        r = compare(ra[1], rb[1], ra[0])
        row.update({k: r[k] for k in ('identical', 'first_diff', 'first_diff_s', 'pct_identical', 'maxabs',
                                      'level_db', 'lag', 'lag_pct', 'len_a', 'len_b', 'silent_a', 'silent_b',
                                      'peak_a', 'peak_b', 'tempo_drift_s', 'pitch_cents', 'bands_db')})
        mm = stage_mismatch(oref, po)
        cls = classify(r)
        mm = [m for m in mm]
        if mm and cls not in ('IDENTICAL', 'LAG-ONLY', 'NEAR'):
            cls = 'STAGE-MISMATCH'
        lr, lp = _led_state(row.get('led_ref', '')), _led_state(row.get('led_port', ''))
        if lr and lp and lr != lp:
            mm.append(f'led-start/switch ref={lr} port={lp}')
        row['cls'] = cls
        row['note'] = ' '.join(mm)
    except Exception as e:  # noqa
        row['note'] = f'{type(e).__name__}: {e}'[:160]
    return row


# ---- reports ----------------------------------------------------------------
COLS = ['id', 'fmt', 'cls', 'identical', 'first_diff_s', 'pct_identical', 'maxabs', 'level_db', 'lag', 'lag_pct',
        'len_a', 'len_b', 'silent_a', 'silent_b', 'peak_a', 'peak_b', 'tempo_drift_s', 'pitch_cents', 'bands_db',
        'note', 'path', 'ref_player', 'out_ref', 'out_port', 'led_ref', 'led_port']


def tsv_line(row):
    return '\t'.join(str(row.get(c, '')).replace('\t', ' ').replace('\n', ' ') for c in COLS)


def write_tsv(path, rows):
    with open(path, 'w') as fh:
        fh.write('\t'.join(COLS) + '\n')
        for r in rows:
            fh.write(tsv_line(r) + '\n')


def read_tsv(path):
    if not os.path.exists(path):
        return {}
    lines = open(path).read().splitlines()
    cols = lines[0].split('\t')
    out = {}
    for l in lines[1:]:
        v = l.split('\t')
        d = dict(zip(cols, v))
        out[d['path']] = d
    return out


def summary(rows):
    fm = {}
    for r in rows:
        d = fm.setdefault(r['fmt'], {})
        d[r['cls']] = d.get(r['cls'], 0) + 1
    lines = [f'{"format":42s} exact/total  ' + ' '.join(f'{c}' for c in CLASSES if any(c in d for d in fm.values()))]
    used = [c for c in CLASSES if any(c in d for d in fm.values())]
    lines = [f'{"format":42s} {"exact":>5s}/{"tot":<4s} ' + ' '.join(f'{c[:6]:>6s}' for c in used)]
    for k in sorted(fm):
        d = fm[k]
        lines.append(f'{k[:42]:42s} {d.get("IDENTICAL", 0):>5d}/{sum(d.values()):<4d} ' + ' '.join(f'{d.get(c, 0):>6d}' for c in used))
    tot = {}
    for r in rows:
        tot[r['cls']] = tot.get(r['cls'], 0) + 1
    lines.append('')
    lines.append('total ' + ' '.join(f'{c}={tot[c]}' for c in CLASSES if c in tot) + f' of {len(rows)}')
    bad = sum(v for c, v in tot.items() if c not in GOOD)
    lines.append(f'inconsistencies: {bad}')
    return '\n'.join(lines)


def write_html(path, rows, new, fixed):
    col = {'IDENTICAL': '#2a6', 'LAG-ONLY': '#6b6', 'NEAR': '#8b4', 'SIMILAR': '#bb4', 'NOREF': '#aaa'}
    h = ['<!doctype html><meta charset=utf-8><title>refcmp</title><style>body{font:13px system-ui;margin:16px}'
         'table{border-collapse:collapse}td,th{border:1px solid #ddd;padding:2px 6px;text-align:left}'
         '.m{font-family:monospace;font-size:11px;color:#555}</style>']
    h.append('<h1>refcmp</h1><pre>' + html.escape(summary(rows)) + '</pre>')
    h.append(f'<p>new inconsistencies: {len(new)}, fixed: {len(fixed)}</p>')
    h.append('<table><tr>' + ''.join(f'<th>{c}</th>' for c in ('id', 'format', 'class', 'first diff s', '% identical',
             'maxabs', 'level dB', 'lag', 'bands dB (20-500-2k-5k-10k-20k)', 'ref output', 'port output', 'note')) + '</tr>')
    for r in sorted(rows, key=lambda r: (r['cls'] in GOOD, r['fmt'], r['id'])):
        c = col.get(r['cls'], '#c33')
        h.append(f'<tr><td>{r["id"]}</td><td>{html.escape(r["fmt"])}</td><td style="color:{c};font-weight:600">{r["cls"]}</td>'
                 f'<td>{r.get("first_diff_s", "")}</td><td>{r.get("pct_identical", "")}</td><td>{r.get("maxabs", "")}</td>'
                 f'<td>{r.get("level_db", "")}</td><td>{r.get("lag", "")}</td><td>{r.get("bands_db", "")}</td>'
                 f'<td class=m>{html.escape(str(r.get("out_ref", "")))}</td><td class=m>{html.escape(str(r.get("out_port", "")))}</td>'
                 f'<td>{html.escape(str(r.get("note", "")))}</td></tr>')
    h.append('</table>')
    open(path, 'w').write('\n'.join(h))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--all', action='store_true', help='every file of the list')
    ap.add_argument('--fmt', action='append', help='only formats containing this text (repeatable)')
    ap.add_argument('--id', action='append', type=int, help='only this list id (repeatable)')
    ap.add_argument('--list', default=DEFAULT_LIST)
    ap.add_argument('--bin', default=DEFAULT_BIN)
    ap.add_argument('--seconds', type=int, default=20)
    ap.add_argument('-j', type=int, default=os.cpu_count() // 2 or 1)
    ap.add_argument('--flags', nargs='*', default=[], help='extra amigaplayer-rs flags for both sides, e.g. --flags --filter a1200')
    ap.add_argument('--out', default=SCRATCH + '/refcmp-out')
    ap.add_argument('--cache', default=SCRATCH + '/refcmp-cache')
    ap.add_argument('--baseline', default=BASELINE)
    ap.add_argument('--update-baseline', action='store_true')
    ap.add_argument('--listen', default=LISTEN_PY, help='listen.py, for its staging of modules with companions')
    ap.add_argument('--oracle', default=XMP_ORACLE)
    a = ap.parse_args()
    files = json.load(open(a.list))
    if not a.all:
        if not a.fmt and not a.id:
            ap.error('give --all, --fmt or --id')
        files = [f for f in files if f['id'] in (a.id or []) or any(s.lower() in f['fmt'].lower() for s in a.fmt or [])]
    ctx = Ctx(a)
    os.makedirs(a.out, exist_ok=True)
    rows = []
    with cf.ThreadPoolExecutor(a.j) as ex:
        for i, row in enumerate(ex.map(lambda f: process(ctx, f), files), 1):
            rows.append(row)
            print(f'[{i}/{len(files)}] {row["cls"]:15s} {row["fmt"]} #{row["id"]} {row["note"]}', flush=True)
    write_tsv(os.path.join(a.out, 'refcmp.tsv'), rows)
    base = read_tsv(a.baseline)
    new, fixed, changed = [], [], []
    for r in rows:
        b = base.get(r['path'])
        bad_now = r['cls'] not in GOOD
        bad_then = b is not None and b['cls'] not in GOOD
        if bad_now and not bad_then:
            new.append(r)
        elif bad_now and bad_then and b['cls'] != r['cls']:
            changed.append((r, b['cls']))
        elif not bad_now and bad_then:
            fixed.append(r)
    text = summary(rows)
    text += f'\n\nvs baseline ({len(base)} files): NEW inconsistencies {len(new)}, FIXED {len(fixed)}, class changed {len(changed)}\n'
    for r in new:
        text += f'  NEW   {r["cls"]:15s} {r["fmt"]} #{r["id"]} {r.get("level_db", "")} dB {r["note"]}\n'
    for r in fixed:
        text += f'  FIXED {r["fmt"]} #{r["id"]} now {r["cls"]}\n'
    for r, was in changed:
        text += f'  CHANGED {r["fmt"]} #{r["id"]} {was} -> {r["cls"]}\n'
    open(os.path.join(a.out, 'summary.txt'), 'w').write(text)
    write_html(os.path.join(a.out, 'report.html'), rows, new, fixed)
    print('\n' + text)
    if a.update_baseline:
        write_tsv(a.baseline, rows)
        print('baseline updated:', a.baseline)
        return 0
    return 1 if new else 0


if __name__ == '__main__':
    sys.exit(main())
