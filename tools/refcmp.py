"""Bit-compare amigaplayer-rs's open replayers with their originals, and flag inconsistencies.

    python3 -I tools/refcmp.py --all -j 8                 # whole list, against the baseline
    python3 -I tools/refcmp.py --fmt tfmx --fmt 'corpus: emod' -j 4
    python3 -I tools/refcmp.py --all --update-baseline    # accept this run as the baseline

Each file of the list (default: the listen page's files.json: entries
{fmt, path, id}, paths possibly .xz) is rendered twice with identical output
settings, WAV for WAV:

  port       amigaplayer-rs --open-replayers, with every player directory empty
             (so no 68k player stands in for the port)
  reference  the ORIGINAL player: the same binary with --68k-players (so the binary
             must be built with the 68k hosting: `cargo build --release --features host-68k`;
             the default build has no 68k player support and refuses the option),
             running the real Eagleplayer/DeliPlayer on the emulated Amiga with
             the same output stage (first of EP206, then the Eagleplayers UADE
             ships, whose player runs on the 68k); for the formats the libxmp
             port plays, libxmp 4.7.4 itself (the C harness `render`, 48 kHz
             s16, its defaults); for Atari ST SNDH, AtariAudio 1.30 (SNDH_ORACLE,
             at volume 0.8). When no original gives a reference (none runs
             the module, or the one that runs is silent and the port is not), a
             second-choice original of the format (ALT_ORIGINALS: another build
             that runs the module, marked REF-FALLBACK where the port has a
             verified original), else for the formats checked in uade123
             (UADE123_REF) uade123 itself with the port's output stage. A silent
             original is the reference of a port that is silent too.

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

The binary: `cargo build --release --features host-68k` (the `native` feature, which adds the
recompiled players, is not part of it and must stay off for the baseline).
The baseline is measured with a binary that has NO recompiled players (crates/players-native/gen
absent, or its `gen` feature off, which `host-68k` alone leaves it). With gen compiled in (the
`native` feature in a checkout that has gen/) the machine's heap ends at NATIVE_LO instead of the top of
RAM, the reference's allocations land at other addresses, and 18 rows move (Musicline
IDENTICAL -> SIMILAR, several MED/SoundFactory/ArtOfNoise rows): not a regression of the port.
Check the binary is the right kind: it is about 10 MB without gen, about 33 MB with.
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
# SoundMaster is another build, SCUMM's differs in its sweeps); Mark Cooksey was written against
# UADE's (EP206's is another driver build: other hardware init, DMA sequence and volume scaling,
# silent on Commando); Anders 0land exists only in UADE's set
UADE_FIRST = ('hippel7v', 'soundmaster', 'scumm', 'jesperolsen', 'markcooksey', 'tronic', 'dynamicsynthesizer', 'anders0land',
              # EP206's VSS plays silent in the engine; UADE's is the build the port follows
              'voodoosupremesynthesizer',
              # EP206 ships Laxity-Player (a different build); UADE's Laxity is the one followed
              'laxity',
              # EP206's SUNtronic build does not report the song end; UADE's does
              'suntronic',
              # EP206's MCMD is another build than UADE's, which the port's write logs come from
              'mcmd',
              # EP206's GlueMon differs from UADE's V1.13 build, which the port follows
              'gluemon',
              # EP206 ships another Mark II build; the port follows UADE's write logs
              'markii',
              # Infogrames exists only in UADE's set
              'infogrames',
              # EP206's Major Tom 2.01 is another build; the port follows UADE's MajorTom
              'majortom',
              # MultiMedia Sound exists only in UADE's set (EP206's SoundFX 2.0 takes the same files)
              'multimediasound')
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
        # a companion of the same stem (Infogrames' NAME.ins bank) goes along
        stem = os.path.splitext(os.path.basename(src))[0]
        for f in os.listdir(os.path.dirname(src) or '.'):
            if f.lower().endswith('.ins') and os.path.splitext(f)[0] == stem:
                shutil.copy(os.path.join(os.path.dirname(src), f), os.path.join(d, f))
        # Nick Pelling Packer: the song file NPP.<name> needs its sample set SMP.<name> (or SMP.set)
        if name[:4].lower() == 'npp.':
            for c in ('SMP.' + name[4:], 'SMP.set'):
                cs = os.path.join(os.path.dirname(path), c)
                if os.path.exists(cs):
                    shutil.copy(cs, os.path.join(d, c))
    return mod


# companions of a fixed name (not the module's stem) that a player loads from the module's directory:
# Rob Hubbard 2's sample bank rh.ins (the DeliPlayer asks for that name only)
FIXED_COMPANIONS = ('rh.ins',)


def stage_fixed_companions(path, mod):
    src = os.path.dirname(path)
    dst = os.path.dirname(mod)
    for c in FIXED_COMPANIONS:
        for n in (c, c + '.xz'):
            s = os.path.join(src, n)
            t = os.path.join(dst, c)
            if os.path.exists(s) and not os.path.exists(t):
                b = open(s, 'rb').read()
                if b[:6] == b'\xfd7zXZ\x00':
                    b = lzma.decompress(b)
                open(t + '.tmp', 'wb').write(b)
                os.replace(t + '.tmp', t)


ALIAS_ROOT = '/dev/shm/refcmp'   # 16 characters
ALIAS_LEN = 76                   # characters of a cache alias path


def cache_alias(cache):
    """A symlink of fixed path length to the cache directory. The guest sees the module's
    directory and the host's loading cost depends on the LENGTH of that path (directory and
    file name characters enter the first-write time of the original and of the ports modelled on
    it), so every path handed to a render goes through this alias and a result no longer
    depends on how long the --cache path is."""
    real = os.path.abspath(cache)
    os.makedirs(real, exist_ok=True)
    name = hashlib.sha1(real.encode()).hexdigest()
    name += '_' * (ALIAS_LEN - len(ALIAS_ROOT) - 1 - len(name))
    link = os.path.join(ALIAS_ROOT, name)
    os.makedirs(ALIAS_ROOT, exist_ok=True)
    try:
        os.symlink(real, link)
    except FileExistsError:
        if os.path.realpath(link) != os.path.realpath(real):
            os.unlink(link)
            os.symlink(real, link)
    assert len(link) == ALIAS_LEN
    return link


class Ctx:
    def __init__(self, a):
        self.bin = os.path.abspath(a.bin)
        st = os.stat(self.bin)
        self.binkey = f'{int(st.st_mtime)}-{st.st_size}'
        self.secs = a.seconds
        self.flags = a.flags
        self.cache = cache_alias(a.cache)
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
    deli, given = ctx.empty, eagle
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
    cmd = [ctx.bin, *(['--open-replayers'] if open_replayers else ['--68k-players']), *ctx.flags, *extra,
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
        return render_engine(ctx, mod, open_replayers, given, extra, NOTE_GENIES, fast_cpu)
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
# Rob Hubbard 2: the DeliPlayer RobHubbard_2 (with the 4Voices NotePlayer genie) is the original;
# EP206's Rob Hubbard 2 asks for other bank names and returns silent with an unbalanced stack
DELI_REF = {'amosmusicbank': 'AmosMusicBank', 'robhubbard2': 'RobHubbard_2', 'powertracker': 'PowerTracker'}


# Formats whose modules play past their sample data's end (the port plays silence there): the
# original reads the emulated machine's memory after the module, and with the player found by
# the engine's detection that is the engine's own library tables (A-line stubs), noise the port
# cannot and should not play; with the player named, nothing lies there. Seen in SIDMon 2.0
# (3 of 10 files) and SCUMM (3 of 10).
# Sonic Arranger: EP206's SonicArranger (V4.11) takes only "SOARV1.0" modules; the Lionheart-format
# ones (sa.drachenflug.SA, ...) are played by UADE's player, which names itself "Sonic Arranger"
# (file SonicArranger, prefixes sa/sonic/sa-p/lion/sa_old) and is the original for them.
FORCED_PLAYER = {'sidmon20': 'SIDMon 2.0', 'scumm': 'SCUMM_Player', 'sonicarranger': 'Sonic Arranger',
                 # EP206's "Tronic" is another build; UADE's names itself TronicTracker(?)
                 'tronic': 'TronicTracker(?)',
                 # UADE's generic detection takes the .vss files for Soundtracker IV
                 'voodoosupremesynthesizer': 'VSS',
                 # the UADE players these ports follow (README); the generic search finds them today,
                 # naming them keeps another player from winning (verify-covered, option A)
                 'soundimages': 'Sound Images', 'jeroentel': 'Jeroen Tel',
                 'steveturner': 'Steve Turner', 'richardjosephplayer': 'Richard Joseph Player'}


# Formats no Amiga player plays (README: HVL is checked against the C replayer only): whatever
# player the engine's detection picks for such a file (a tracker that takes it for a module and
# plays noise or silence) is no original.
NO_AMIGA_PLAYER = {'hivelytracker'}


# Formats whose original never reports a song end (UADE's PreTracker and SCUMM players are bare replay
# routines: they play on; so does the Jesper Olsen Eagleplayer: 25 modules looped for 120 s each without
# one, and the port's write log equals the original's over 30-40 s once the CLI ignores its song end;
# AProSys never tells the host, the original then goes on silently past the song end; the Rob Hubbard 2
# DeliPlayer never reports a song end)
# while the port signals one at the song's end (README), which stops
# the CLI render there. The original is compared over the port's length.
ORIGINAL_PLAYS_ON = {'pretracker', 'scumm', 'jesperolsen', 'aprosys', 'robhubbard2'}


# Formats whose original refuses some modules the port plays (Hippel 7V: the five COSO modules, README):
# the engine's detection then falls back to another replayer of the family (the 4-voice COSO one),
# which is no reference for the port's 7-voice replayer.
REFUSED_NO_FALLBACK = {'hippel7v'}


# Formats whose Eagleplayer is pre-empted in the full player directory by another player's looser
# check (UADE's Soundtracker IV takes MYST files before YM-2149 is asked): the original runs from a
# directory holding that player alone.
# SoundMon 2.0 / 2.2 (EP206): a one-shot sample ends on the player's "empty" word with length 0,
# so Paula plays 128 KB of memory from there on: the player's own variables, then whatever was
# loaded after its hunk. In the full directory that is the next Eagleplayer's code, which no port
# can hold; loaded alone, zeros follow and the original is one a port can equal.
# PreTracker (UADE): a wave at a note of octave 9 (prt.m065's wave 2) is generated from an
# oscillator index that runs away, up to 64K past the oscillator buffers in the player's BSS hunk:
# through the player's own first hunk (which the port models) and 43 KB further. In the full
# directory that is Pokeynoise's hunk (200 of the wave's 295 reads past the buffers, 197 of them
# not zero); loaded alone, zeros follow.
SOAMC = SCRATCH + '/soamc/x'
ALONE = {'emsv6': ('uade', 'EMS-6'),   # with all of UADE's players loaded the original's memory differs from 1.5 s on
         'gtgamesystems': ('soamc', 'G&T_Game_Systems/1.0_(08.Aug.2008)/G&T Game Systems'),
         'digitalsoundcreations': ('soamc', 'Digital_Sound_Creations/1.0_(29.Dec.2005)/Digital Sound Creations'),
         'ym2149': ('uade', 'YM-2149'), 'soundmon20': ('ep206', 'SoundMon'), 'soundmon22': ('ep206', 'SoundMon 2.2'),
         # SidMon 1.0 (EP206): a finetuned period read past the table lands in the next player's hunk
         'sidmon10': ('ep206', 'SIDMon 1.0'),
         'pretracker': ('uade', 'PreTracker'),
         # Quadra Composer: UADE's QuadraComposer file (V1.01, 24 Jul 1994; it names itself "IFF EMOD")
         # is the original; EP206's "IFF EMOD" (V1.02) is another build whose DMA gap is 110 colour
         # clocks (a voice at a period like 538 keeps its old sample), and whose one-shot samples
         # end on loop registers 0 / length 0 (Paula plays 128 KB from address 0: beeps and cracks)
         'quadracomposer': ('uade', 'QuadraComposer'),
         # Sound Master: UADE's SoundMaster Eagleplayer (prefixes sm, sm1, sm2, sm3, smpro) plays every build
         # the port knows (sm.*, .smpro, .sm3); in the full directory the default 68k chain (its PlayerStore
         # "SoundMaster") plays only .smpro and part of the .sm files and no .sm3, so the original is run from
         # a directory holding that player alone
         'soundmaster': ('uade', 'SoundMaster')}


# Second-choice originals, tried only when the search above finds no usable reference (no player
# runs the module, or the one that runs is silent while the port is not): normalised format ->
# [(directory, player file alone in its own directory or None for the whole directory, --player
# name or None, stand-in)]. A stand-in is another build than the original the port was checked
# against (README), and its class is marked REF-FALLBACK.
#   MaxTrax: EP206's MaxTrax (the 1997 build) fails InitPlayer on scores whose samples are in a
#     companion; the Wanted Team's 2007 build (SOAMC 1.1, 20 Sep 2007; the source the port follows)
#     plays them
#   Fred Editor: EP206's Fred (2.3, 1997) fails InitPlayer on some modules; the 1993 build plays them
#   ADPCM2/3: EP206's ADPCM plays silent in the engine (the host lacks its refill call on channel 0's
#     audio interrupt, README); ADPCM-Player (SOAMC, 1995) decodes the same files
#   Hippel COSO: EP206's Hippel-COSO plays some modules silent; UADE's Jochen Hippel COSO plays them
#   Master / Ultimate SoundTracker: the port plays a Titanics Player module through ProWizard's
#     conversion; its original is UADE's Titanics Packer. A plain Ultimate SoundTracker module plays
#     on UADE's Ultimate Soundtracker, which also takes Titanics modules (and plays them wrong), so
#     Titanics Packer is asked first
ALT_ORIGINALS = {
    'maxtrax': [('soamc', 'MaxTrax_(Music-X_Compatible)/1.1_(20.Sep.2007)/MaxTrax', None, True)],
    'frededitor': [('soamc', 'FredEditor_also_called_Fredmonitor_and_Fredmon/1.1_(30.Aug.1993)/Fred', None, True)],
    'adpcm2': [('soamc', 'ADPCM_mono/x.x_(xx.xxx.1995)/ADPCM-Player', None, True)],
    'adpcm3': [('soamc', 'ADPCM_mono/x.x_(xx.xxx.1995)/ADPCM-Player', None, True)],
    'hippelcoso': [('uade', None, 'Jochen Hippel COSO', True)],
    'mastersoundtracker': [('uade', None, 'Titanics Packer', False)],
    'ultimatesoundtracker10': [('uade', None, 'Titanics Packer', False), ('uade', None, 'Ultimate Soundtracker', False)],
}

# Formats whose original is UADE's player as uade123 runs it (README), which the engine's host does
# not run (Kris Hatlelid: InitPlayer fails on the modules with a `songplay` companion; MusicMaker
# 8V: silent in the engine) or does not take (a plain ProTracker module: PTK-Prowiz wants the module
# type that UADE's front end passes): normalised format -> the player uade123 must report. Used only
# when the engine has no reference; uade123 gets the port's output stage (UADE's panning is 1 minus
# the separation, its gain the volume; checked equal on a file both play identically: SIMILAR, level
# 0.1 dB, side level 1 %). Its 68020 at full speed is not the engine's 68000 timing, so a
# port can reach SIMILAR at best.
UADE123 = os.path.expanduser('~/.local/opt/uade/bin/uade123')
UADE123_REF = {'krishatlelid': 'Kris Hatlelid', 'musicmaker8v': 'MusicMaker8', 'protracker': 'Protracker and family'}


def render_uade123(ctx, mod, po, want):
    """The uade123 reference for a format of UADE123_REF, or None."""
    if want not in UADE123_REF or not os.path.exists(UADE123):
        return None
    if want == 'protracker' and po.get('backend') != 'builtin-protracker':
        return None
    filt = {'a500': 'A500', 'a1200': 'A1200', 'none': 'NONE'}.get(po.get('filter', '').split('(')[0])
    # only the stages uade123 has: Paula through UADE's resampler, the player's LED, no amplifier
    if (not filt or po.get('resampler') != 'uade-default' or po.get('amplifier', 'none') != 'none'
            or po.get('builtin-voices', 'no') != 'no' or po.get('mono', '0') != '0' or not po.get('led', '').startswith('auto')):
        return None
    try:
        sep, vol = float(po['sep']), float(po['volume'])
    except (KeyError, ValueError):
        return None
    args = ['-1', '-t', str(ctx.secs), '--frequency=48000', f'--filter={filt}', '-p', f'{1 - sep:.3f}', '-G', f'{vol:.3f}']
    h = hashlib.sha1(json.dumps([int(os.path.getmtime(UADE123)), mod, args]).encode()).hexdigest()[:20]
    raw = os.path.join(ctx.cache, h + '.uade.raw')
    inf = raw + '.txt'
    if not os.path.exists(inf):
        cwd = os.path.dirname(mod)
        try:
            g = subprocess.run([UADE123, '-g', mod], capture_output=True, text=True, cwd=cwd, timeout=120)
            info = {k.strip(): v.strip() for k, _, v in (l.partition(':') for l in g.stdout.splitlines()) if v}
            if info.get('playername') == UADE123_REF[want]:
                subprocess.run([UADE123, *args, '-e', 'raw', '-f', raw, mod], capture_output=True, text=True, cwd=cwd,
                               timeout=ctx.secs * 4 + 120)
            text = ' '.join(f'{k}={info.get(n, "").replace(" ", "_")}' for k, n in (('player', 'playername'), ('subsongs', 'subsongs')))
        except subprocess.TimeoutExpired:
            text = 'timeout'
        open(inf, 'w').write(text.strip())
    info = open(inf).read()
    if not os.path.exists(raw) or not os.path.getsize(raw):
        return None
    data = read_raw(raw)
    if not data[1].any():
        return None
    return dict(kind='uade123', wav=raw, data=data, out='uade123 ' + info, led='', player='uade123: ' + info, o={}, note='')


# Atari ST SNDH: the port's reference is AtariAudio 1.30 (README), built in the scratch dir from its
# repository (fixround/port-sndh/mkref.sh: `ref FILE OUT.wav LOG SUBSONG SECONDS`, 48 kHz mono, its
# first line `... subsongs=N default=D ...`). Its level is the engine's at volume 0.8 (measured: the 5
# corpus files within 0.13 dB, every band alike), so it is compared only at that volume; the port's
# SNDH output is mono, the same on both channels, so the separation does not enter.
SNDH_ORACLE = SCRATCH + '/fixround/port-sndh/ref/cbuild/ref'


def render_sndh_oracle(ctx, mod, po, pplay):
    """AtariAudio's render of an SNDH file for the port's subsong, or a NOREF note (str)."""
    if not os.path.exists(SNDH_ORACLE):
        return 'no AtariAudio reference build'
    if po.get('volume') != '0.80' or po.get('mono', '0') != '0':
        return f'AtariAudio is compared at volume 0.80 only (port: volume {po.get("volume")})'
    m = re.search(r'\(current (\d+)\)', pplay or '')
    sub = int(m.group(1)) + 1 if m else 1
    h = hashlib.sha1(json.dumps([int(os.path.getmtime(SNDH_ORACLE)), mod, sub, ctx.secs]).encode()).hexdigest()[:20]
    wav = os.path.join(ctx.cache, h + '.sndh.wav')
    inf = wav + '.txt'
    if not os.path.exists(inf):
        try:
            r = subprocess.run([SNDH_ORACLE, mod, wav, wav + '.ymlog', str(sub), str(ctx.secs)], capture_output=True, text=True,
                               cwd=os.path.dirname(mod), timeout=ctx.secs * 4 + 120)
            text = (r.stdout.splitlines() or [''])[0]
        except subprocess.TimeoutExpired:
            text = 'timeout'
        if os.path.exists(wav + '.ymlog'):
            os.unlink(wav + '.ymlog')
        open(inf, 'w').write(text)
    info = open(inf).read()
    if not os.path.exists(wav) or 'subsongs=' not in info:
        return 'AtariAudio: ' + info[:80]
    data = read_wav(wav)
    if not data[1].any():
        return 'AtariAudio renders it silent'
    return dict(kind='atariaudio', wav=wav, data=data, out=f'AtariAudio 1.30 subsong {sub} ' + info, led='',
                player=f'AtariAudio 1.30 (oracle): subsong {sub} ' + info[:60], o={}, note='')


def alternative_reference(ctx, mod, po, want, why):
    """A second-choice original (ALT_ORIGINALS, then uade123) for a module the search found no
    usable reference for, or None. `why`: what the search found."""
    for src, rel, name, standin in ALT_ORIGINALS.get(want, ()):
        d = alone_dir(ctx, src, rel) if rel else {'ep206': EP206, 'uade': UADE_PLAYERS}[src]
        if rel and not os.path.exists(os.path.join({'ep206': EP206, 'uade': UADE_PLAYERS, 'soamc': SOAMC}[src], rel)):
            continue
        rw, rplay, rout, rled, _ = render_engine(ctx, mod, False, d, ('--player', name) if name else (),
                                                 fast_cpu=po.get('reference-cpu') == 'fast')
        o = parse_output(rout)
        if not (rw and rplay and o.get('backend') == '68k'):
            continue
        data = read_wav(rw)
        if not data[1].any():
            continue
        cand = dict(kind='engine', wav=rw, data=data, out=rout, led=rled or '', player=f'alt-{src}: {rplay}', o=o, note='')
        if standin:
            cand['fallback'] = f'REF-FALLBACK (alternative original {src}:{rel or name}; {why})'
        return cand
    return render_uade123(ctx, mod, po, want)


def alone_dir(ctx, src, name):
    d = os.path.join(ctx.cache, 'alone-' + re.sub(r'[^A-Za-z0-9]', '_', name))
    os.makedirs(d, exist_ok=True)
    link = os.path.join(d, os.path.basename(name))
    if not os.path.lexists(link):
        try:
            os.symlink(os.path.join({'ep206': EP206, 'uade': UADE_PLAYERS, 'soamc': SOAMC}[src], name), link)
        except FileExistsError:   # another job of the same run made it first
            pass
    return d


def retext_empty(ctx, mod):
    """TFMX Pro modules whose text is the editor's "(Empty) ": EP206's TFMX_Pro takes only four it
    knows and leaves the rest to TFMX_1.5, which refuses them, while UADE's newer TFMX_Pro plays
    them but differs (another subsong list and song-end test). The port follows EP206's
    replayer, which plays every one of them with the text changed (README), so the original is
    EP206's on a copy with the text changed (companion files linked). Returns the copy's path,
    or `mod` for any other module."""
    try:
        with open(mod, 'rb') as fh:
            d = bytearray(fh.read())
    except OSError:
        return mod
    if d[:10] != b'TFMX-SONG ' or d[0x10:0x18] != b'(Empty) ' or d[0x1d0:0x1d4] != b'\0\0\0\0':
        return mod
    d[0x10:0x18] = b'Retexted'
    dst = os.path.join(ctx.cache, 'retext-' + hashlib.sha1(mod.encode()).hexdigest()[:12])
    os.makedirs(dst, exist_ok=True)
    out = os.path.join(dst, os.path.basename(mod))
    with open(out, 'wb') as fh:
        fh.write(d)
    src = os.path.dirname(mod)
    for n in os.listdir(src):
        link = os.path.join(dst, n)
        if n != os.path.basename(mod) and not os.path.lexists(link):
            try:
                os.symlink(os.path.join(src, n), link)
            except FileExistsError:
                pass
    return out


def special_reference(mod, fmt):
    """A per-module original where one player does not cover the format: Digital Sound
    Studio modules with a tempo byte play on UADE's DSS 1.0-3.0 player (V1.2), as the
    V1.71 Eagleplayer the port follows for the others reads the tempo as part of the speed."""
    if fmt.startswith('delusiondigitalmusicformat'):
        # X-Tracker (DDMF): the port follows libopenmpt, which libxmp's oracle cannot load; the
        # Amiga original is the DeliPlayer "XTracker" (SOAMC's V0.8, alone in its directory)
        return [('deli', ('deli', SOAMC + '/DDMF_(Delusion_Digital_Music Format-XTracker)/0.8_(26.Aug.1996)'),
                 ('--player', 'XTracker'))]
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


def find_reference(ctx, mod, po, port_short=False, port_silent=False, pplay=''):
    """The original's render for a file whose port output line parses to `po` (`pplay`: its
    `playing` line): dict(kind 'engine'|'libxmp'|'uade123'|'atariaudio'|'none', wav (path),
    data ((rate, samples)), out, led, player, o, note, noref)."""
    none = dict(kind='none', note='', noref=True)
    if po.get('backend') == 'libxmp-port' and not _norm(po.get('format', '')).startswith('delusiondigitalmusicformat'):
        if not os.path.exists(ctx.oracle):
            return dict(none, note='no libxmp oracle')
        raw, info = render_oracle(ctx, mod)
        if not raw or '\t' not in info:
            return dict(none, note='oracle: ' + info[:100], noref=False)
        return dict(kind='libxmp', wav=raw, data=read_raw(raw), out='libxmp 4.7.4 ' + info.replace('\t', ' '),
                    led='', player='libxmp 4.7.4 (oracle)', o={}, note='')
    want = _norm(po.get('format', ''))
    if want in NO_AMIGA_PLAYER:
        return dict(none, note='no Amiga player plays this format (README)')
    if want == 'sndh':
        # an Atari ST program: the 68k DeliPlayer chain accepts the files and plays them silent (README)
        ref = render_sndh_oracle(ctx, mod, po, pplay)
        return ref if isinstance(ref, dict) else dict(none, note=ref)
    dirs = (('ep206', EP206), ('uade', UADE_PLAYERS))
    if want in UADE_FIRST:
        dirs = dirs[::-1]
    if want in ALONE:
        dirs = ((ALONE[want][0], alone_dir(ctx, *ALONE[want])),)
    if want == 'tfmxpro':
        mod = retext_empty(ctx, mod)
    # the player the port was verified against comes first, then the generic search
    verified = VERIFIED.get(want)
    tries = special_reference(mod, want) or []
    if verified:
        tries += [(t + '+verified', d, ('--player', verified)) for t, d in dirs]
    if want in FORCED_PLAYER:
        tries += [(t + '+named', d, ('--player', FORCED_PLAYER[want])) for t, d in dirs]
    first = len(tries)
    tries += [(t, d, ()) for t, d in dirs]
    pn = po.get('player', '')
    names = [pn, pn.replace('_', ' '), pn.replace('_', '-')]
    if po.get('backend') == 'builtin-protracker':
        # The built-in ProTracker takes the module before any 68k player is asked (with or without
        # --open-replayers), so an original only runs when it is named, and --player wants the name
        # the player gives itself, not its file's. UADE's PTK-Prowiz (a ProTracker 3.0b replay behind
        # a ProWizard converter; the README checks the built-in replayer against it) is "Protracker
        # and family": it plays the ProWizard-packed and ChipTracker files itself. A plain module it
        # takes only when UADE's front end names the type through uade.library, which the engine
        # does not provide, so those stay NOREF. Mosh Packer files it refuses: UADE's "Mosh Packer"
        # plays them. (The Protracker4 Eagleplayer plays the IFF MODL format and refuses all of these.)
        names += ['Protracker and family', 'Mosh Packer']
    for variant in dict.fromkeys(names):
        tries += [(t + '+player', d, ('--player', variant)) for t, d in dirs]
    fallback = None
    bads = []
    refused = []   # the preferred (special / verified / named) tries that did not produce a render
    for i, (tag, eagle, extra) in enumerate(tries):
        rw, rplay, rout, rled, rlines = render_engine(ctx, mod, False, eagle, extra, fast_cpu=po.get('reference-cpu') == 'fast')
        o = parse_output(rout)
        if not (rw and rplay and o.get('backend') == '68k'):
            # a preferred try whose player exists in its directory but did not run (crash, refusal) is
            # worth reporting if a generic search then finds another original
            if i < first and not (extra and extra[0] == '--player' and not os.path.exists(os.path.join(eagle[1] if isinstance(eagle, tuple) else eagle, extra[1]))):
                refused.append(f'{tag}: ' + (rlines[-1] if rlines else 'no output')[:90])
            continue
        got = re.sub(r'[^a-z0-9]', '', o.get('format', '').lower())
        cand = dict(kind='engine', wav=rw, data=None, out=rout, led=rled or '', player=f'{tag}: {rplay}', o=o, note='')
        # an Eagleplayer that numbers its subsongs from 0xF00 (EP206's Sean Conran) plays no real
        # subsong, and a render that ends within a quarter second of a longer run is a player the
        # engine could not run (or a different song set): try the next original first
        m = re.search(r'subsongs (\d+)\.\.(\d+)', rplay)
        with wave.open(rw, 'rb') as w:
            # (a port that ends as early itself makes a short original an agreeing one)
            short = w.getnframes() < w.getframerate() // 4 and ctx.secs >= 5 and not port_short
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
        why = 'no original player plays it in the engine'
        return alternative_reference(ctx, mod, po, want, why) or dict(none, note=why)
    if refused and fallback['player'].split(': ')[0] not in [t for t, _, _ in tries[:first]]:
        # the original this port is checked against did not run, and another player stands in for it:
        # its FILTER/DIFF classes are not about the port's original
        fallback['fallback'] = 'REF-FALLBACK (' + '; '.join(refused) + ')'
    if want in REFUSED_NO_FALLBACK:
        got = re.sub(r'[^a-z0-9]', '', fallback['o'].get('format', '').lower())
        if not (want in got or got in want):
            # the verified player refuses the module and the engine's detection found another
            # replayer: its output is not the original of this port
            return dict(none, note=f'the {want} Eagleplayer refuses it; {fallback["player"][:60]} is another replayer')
    fallback['data'] = read_wav(fallback['wav'])
    if not fallback['data'][1].any() and not port_silent:
        # (a silent original is the reference of a port that is silent too: both play nothing)
        why = 'the original is silent in the engine (' + fallback['player'][:60] + ')'
        return alternative_reference(ctx, mod, po, want, why) or dict(none, note=why)
    return fallback


def process(ctx, f):
    """One file: renders, comparison, class. Returns the result row (a dict)."""
    row = {'id': f['id'], 'fmt': f['fmt'], 'path': f['path'], 'cls': 'FAIL', 'note': ''}
    try:
        path = f['path']
        mod = ctx.stage(path) if ctx.stage else _fallback_stage(path, ctx.cache)
        stage_fixed_companions(path, mod)
        pw, pplay, pout, pled, plines = render_engine(ctx, mod, True, None)
        row['out_port'] = pout or ''
        row['led_port'] = pled or ''
        if not pw or not pplay:
            row['note'] = 'port: ' + (plines[-1] if plines else 'no output')[:120]
            return row
        row['player'] = pplay
        po = parse_output(pout)
        with wave.open(pw, 'rb') as w:
            port_short = w.getnframes() < w.getframerate() // 4
        rb = read_wav(pw)
        ref = find_reference(ctx, mod, po, port_short, port_silent=not rb[1].any(), pplay=pplay)
        if ref['kind'] == 'none':
            row['cls'] = 'NOREF' if ref['noref'] else 'FAIL'
            row['note'] = ref['note']
            return row
        row.update(out_ref=ref['out'], led_ref=ref['led'], ref_player=ref['player'])
        ra, oref = ref['data'], ref['o']
        cut_note = ''
        if ra[0] != rb[0]:
            row['note'] = f'rates {ra[0]} vs {rb[0]}'
            return row
        if (_norm(po.get('format', '')) in ORIGINAL_PLAYS_ON and any(l.startswith('song end after') for l in plines)
                and len(ra[1]) > len(rb[1])):
            ra = (ra[0], ra[1][:len(rb[1])])
            cut_note = 'port song end; original plays on, cut to the port length'
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
        if ref.get('fallback'):
            mm.append(ref['fallback'])
        row['note'] = ' '.join(mm + ([cut_note] if cut_note else []))
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
    # the reference side needs the 68k hosting: a default build refuses --68k-players (with
    # --wav so that a build that has it does not open a window)
    probe = subprocess.run([a.bin, '--68k-players', '--wav', os.devnull, os.devnull + '.none'], capture_output=True, text=True,
                           env=dict(os.environ, SDL_AUDIO_DRIVER='dummy'))
    if 'built without 68k player support' in probe.stderr:
        ap.error(f'{a.bin} has no 68k player support: build it with `cargo build --release --features host-68k`')
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
    fbs = [r for r in rows if 'REF-FALLBACK' in r['note']]
    newfb = [r for r in fbs if 'REF-FALLBACK' not in (base.get(r['path']) or {}).get('note', '')]
    if fbs:
        text += f'\nreference fallbacks (the original this port is checked against did not run; classes below are against a stand-in): {len(fbs)}\n'
        for r in fbs:
            text += f'  FALLBACK {r["cls"]:15s} {r["fmt"]} #{r["id"]} {r["note"][:200]}\n'
    text += f'\n\nvs baseline ({len(base)} files): NEW inconsistencies {len(new)}, FIXED {len(fixed)}, class changed {len(changed)}\n'
    for r in new:
        text += f'  NEW   {r["cls"]:15s} {r["fmt"]} #{r["id"]} {r.get("level_db", "")} dB {r["note"]}\n'
    for r in fixed:
        text += f'  FIXED {r["fmt"]} #{r["id"]} now {r["cls"]}\n'
    for r, was in changed:
        text += f'  CHANGED {r["fmt"]} #{r["id"]} {was} -> {r["cls"]}\n'
    for r in newfb:
        text += f'  NEW REFERENCE FALLBACK {r["cls"]} {r["fmt"]} #{r["id"]}\n'
    open(os.path.join(a.out, 'summary.txt'), 'w').write(text)
    write_html(os.path.join(a.out, 'report.html'), rows, new, fixed)
    print('\n' + text)
    if a.update_baseline:
        write_tsv(a.baseline, rows)
        print('baseline updated:', a.baseline)
        return 0
    return 1 if new or newfb else 0


if __name__ == '__main__':
    sys.exit(main())
