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
             at volume 0.8); for HivelyTracker (no Amiga player plays HVL), Pete Gordon's C
             replayer hvl_replay 1.9 (BSD-3-Clause, HVL_REF_DIR) compared TICK BY TICK
             (volume, period, pan and a hash of each voice's waveform loop; see
             render_hvl_ticks), not by audio: the port plays through the Eagleplayer amplifier,
             the C tool through its own mixer, so their audio is not comparable. When no original gives a reference (none runs
             the module, or the one that runs is silent and the port is not), a
             second-choice original of the format (ALT_ORIGINALS: another build
             that runs the module, marked REF-FALLBACK where the port has a
             verified original), else for the formats checked in uade123
             (UADE123_REF) uade123 itself with the port's output stage. A silent
             original is the reference of a port that is silent too. The formats
             no Amiga player plays here (BLACKBOX_REF: 8SVX, 16SV, AIFF, WAV, MPEG audio,
             ZX AY, NSF, Symphonie; and X-Tracker, whose original's output stage the port
             does not have) are checked against sox, ffmpeg, libgme or
             openmpt123 instead; where an original does run (ORIGINAL_BEFORE_BLACKBOX: NSF's
             DeliPlayer, ZX AY's Eagleplayer PlayAY) it comes first and the tool is the fallback, and Fibonacci 8SVX is
             checked on the data the original Eagleplayer's Paula fetches (render_8svx_dma). uade123's
             PTK-Prowiz clears every sample's first long before it plays (ProTracker
             2.3D, the port's original, only a one-shot sample's first word): where
             that changes what plays, both sides render a copy with it cleared
             (PTK_CLEAR_NOTE in the row's note).

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
  NOREF       no original available (neutral: not counted as an inconsistency); also a file the
              port does not render when no original plays it either

  NORATE      the reference tool cannot render at the run's --rate (neutral, like NOREF: nothing
              is verified; a reference is never resampled to the run's rate)

The sample rate (--rate N, default 48000): both sides render at N. The port and every engine
reference (the original 68k players) through `amigaplayer-rs --rate N` (the option is passed only
when N is not 48000, so a 48 kHz run's commands and cache keys are those of before the option);
uade123 with --frequency=N; libxmp through a rate-capable copy of the C harness (XMP_ORACLE_RATE:
`render MODULE OUT.raw SECONDS RATE`, same source, libxmp.a and flags; byte-identical to `render`
at 48000) and AtariAudio through a rate-capable copy of its harness (SNDH_ORACLE_RATE, its 6th
argument; byte-identical at 48000); libgme and openmpt123 at N; sox and ffmpeg decode at the
file's rate and the port's own rate conversion (src/aiff.rs) takes it to N, as at 48 kHz. HVL's
tick dump and Fibonacci 8SVX's Paula-fetch comparison do not depend on the output rate. A
reference whose tool has no build for N (the copies above absent) is NORATE. Every 48000 of
the comparison is the run's rate: the lag search (LAG_MAX frames at 48 kHz, scaled), the
cross-correlation window (10 s), the LENGTH tolerance and the TEMPO minimum length. Cache keys
carry the rate (when not 48000). The baseline is tools/refcmp-baseline.tsv at 48000 and
tools/refcmp-baseline-N.tsv at any other N (--baseline overrides it).

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
# the same harness with the rate as its 4th argument (render.c copied with RATE read from argv,
# built against the same libxmp.a with the same flags; equal to XMP_ORACLE's output at 48000)
XMP_ORACLE_RATE = SCRATCH + '/fixround/rate441/oracles/xmp/render'
LISTEN_PY = SCRATCH + '/listen/listen.py'
BASELINE = os.path.join(HERE, 'refcmp-baseline.tsv')
RATE = 48000            # Hz: the default --rate, at which BASELINE is measured


def baseline_for(rate):
    """The default baseline file of a run at `rate`."""
    return BASELINE if rate == RATE else os.path.join(HERE, f'refcmp-baseline-{rate}.tsv')


def rate_key(rate):
    """What a cache key gets for the rate: nothing at 48000 (the keys of before --rate), else the rate."""
    return [] if rate == RATE else [rate]

# ---- thresholds -------------------------------------------------------------
LAG_MAX = 4000          # frames at 48 kHz: largest shift searched (83 ms; scaled to the run's rate, lag_max)
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

GOOD = ('IDENTICAL', 'LAG-ONLY', 'NEAR', 'SIMILAR', 'NOREF', 'NORATE')
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


def read_raw(path, rate=RATE):
    return rate, np.fromfile(path, dtype='<i2').reshape(-1, 2)


def lag_max(rate):
    """LAG_MAX (frames at 48 kHz) at `rate`: the same 83 ms."""
    return LAG_MAX if rate == RATE else int(round(LAG_MAX * rate / RATE))


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


def _best_lag(a, b, rate=RATE):
    """Shift s (b[i+s] ~ a[i]) with the highest cross-correlation of the mid channels."""
    lm = lag_max(rate)
    n = min(len(a), len(b), 10 * rate)
    if n < 2 * lm:
        return 0
    x = a[:n].mean(axis=1)
    y = b[:n].mean(axis=1)
    size = 1 << (2 * n - 1).bit_length()
    c = np.fft.irfft(np.fft.rfft(y, size) * np.conj(np.fft.rfft(x, size)), size)
    cand = np.concatenate([c[:lm + 1], c[size - lm:]])
    lags = np.concatenate([np.arange(lm + 1), np.arange(-lm, 0)])
    return int(lags[np.argmax(cand)])


def _aligned(a, b, s):
    """a and b with b shifted by s (b[i+s] against a[i])."""
    if s >= 0:
        b2, a2 = b[s:], a
    else:
        a2, b2 = a[-s:], b
    n = min(len(a2), len(b2))
    return a2[:n], b2[:n]


def compare(a, b, rate=RATE):
    """Numbers for the pair a (reference) and b (port), int16 (n, 2) arrays."""
    # A 68k player cannot tell the engine that its song has ended: the original goes on rendering
    # to the time limit while the port reports the end and stops. When all the reference has
    # past the port's end is digital silence, nothing the port would have played is missing,
    # and the two are compared up to the port's end.
    if 0 < len(b) < len(a) and not a[len(b):].any():
        a = a[:len(b)]
    r = {'len_a': len(a), 'len_b': len(b), 'rate': rate}
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
    if not r['identical'] and n > 2 * lag_max(rate) and ra > 0 and rb > 0:
        s = _best_lag(a, b, rate)
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
    rate = r.get('rate', RATE)
    if r['lag'] and r['lag_pct'] >= LAG_PCT and abs(la - lb) <= abs(r['lag']) + 1:
        return 'LAG-ONLY'
    if la == lb and r['maxabs'] <= NEAR_MAXABS:
        return 'NEAR'
    if r['lag'] and r['lag_maxabs'] <= NEAR_MAXABS and abs(la - lb) <= abs(r['lag']) + 1:
        return 'NEAR'
    sa, sb = r['silent_a'], r['silent_b']
    if (r['rms_a'] == 0) != (r['rms_b'] == 0) or abs(sa - sb) > SILENT_FRAC:
        return 'SILENT'
    tol = max(LENGTH_TOL_S * rate, LENGTH_TOL_PCT / 100.0 * max(la, lb))
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
    if max(la, lb) >= 3 * rate and abs(r['tempo_drift_s']) >= TEMPO_DRIFT_S:
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


def stage_maxtrax_samples(mod):
    """A MaxTrax score whose samples are in a companion: the port (after the Wanted Team's 2007
    source) takes SMPL.<name>, SMPL.set or MXTX.main, while EP206's MaxTrax (the original the port
    is checked against) asks for MXSM.<name>, then MXSM, then a file requester (which the engine does
    not have: InitPlayer fails). The staged copy gets the companion the port takes under the name
    EP206's asks for first as well, so that the original runs."""
    d, leaf = os.path.split(mod)
    if not leaf.lower().startswith('mxtx.'):
        return
    names = {f.lower(): f for f in os.listdir(d)}
    if 'mxsm' + leaf[4:].lower() in names:
        return
    for c in ('smpl' + leaf[4:], 'smpl.set', 'mxtx.main'):
        src = names.get(c.lower())
        if src and src != leaf:
            t = os.path.join(d, 'MXSM' + leaf[4:])
            shutil.copy(os.path.join(d, src), t + '.tmp')
            os.replace(t + '.tmp', t)
            return


def early_text_tfmx(d):
    """An early TFMX-SONG module of the fixed tables whose text starts at $0A, right after the
    header ("TFMX-SONG by ERN0/SB ..."): the port's detect_early_text."""
    return len(d) >= 0x1d4 and d[:10] == b'TFMX-SONG ' and d[0x1d0:0x1d4] == b'\0\0\0\0' and d[0xc:0xe] != b'\0\0'


def stage_tfmx_early_text(mod):
    """Such a module (early_text_tfmx) in the NostalgicPlayer form NAME.tfx beside NAME.sam: UADE's
    TFMX player, its original, asks for smpl.NAME.tfx (then SMPL.set) only. The staged copy gets the
    samples under that name as well (the port takes the same file under either name)."""
    d, leaf = os.path.split(mod)
    try:
        with open(mod, 'rb') as fh:
            head = fh.read(0x1d4)
    except OSError:
        return
    if not early_text_tfmx(head):
        return
    names = {f.lower(): f for f in os.listdir(d)}
    if ('smpl.' + leaf).lower() in names or '.' not in leaf:
        return
    stem = leaf.rsplit('.', 1)[0]
    for c in (stem + '.smpl', stem + '.sam'):
        src = names.get(c.lower())
        if src:
            t = os.path.join(d, 'smpl.' + leaf)
            shutil.copy(os.path.join(d, src), t + '.tmp')
            os.replace(t + '.tmp', t)
            return


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
        self.rate = a.rate
        # off 48000 the rate-capable harness copies (at 48000 the fixed ones: the keys of before --rate)
        self.oracle = a.oracle if a.rate == RATE else a.oracle_rate
        self.sndh_oracle = SNDH_ORACLE if a.rate == RATE else SNDH_ORACLE_RATE
        self.hvl_testbin = a.hvl_testbin


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
           *(['--rate', str(ctx.rate)] if ctx.rate != RATE else []), '--wav', '@WAV@', '--seconds', str(ctx.secs), mod]
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
    h = hashlib.sha1(json.dumps([mod, ctx.secs, os.path.getmtime(ctx.oracle), *rate_key(ctx.rate)]).encode()).hexdigest()[:20]
    raw = os.path.join(ctx.cache, h + '.xmp.raw')
    inf = raw + '.txt'
    if not os.path.exists(inf):
        r = subprocess.run([ctx.oracle, mod, raw, str(ctx.secs), *([str(ctx.rate)] if ctx.rate != RATE else [])],
                           capture_output=True, text=True,
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
# DeliPlayer never reports a song end; nor does EP206's A.M.Composer 1.2: amc.jingle_1 and amc.jingle_2
# loop in it for 30 s, the first pass alike the port's up to its song end)
# while the port signals one at the song's end (README), which stops
# the CLI render there. The original is compared over the port's length.
ORIGINAL_PLAYS_ON = {'pretracker', 'scumm', 'jesperolsen', 'aprosys', 'robhubbard2', 'amcomposer'}


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
# through the player's own first hunk (of which the port holds its computed tables and 4 table bytes,
# not the bytes of its code) and 43 KB further. In the full
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
#     companion it cannot find (it asks for MXSM.<name> and MXSM only; stage_maxtrax_samples gives the
#     staged copy that name); the Wanted Team's 2007 build (SOAMC 1.1, 20 Sep 2007; the source the port follows)
#     plays them
#   Fred Editor: EP206's Fred (2.3, 1997) fails InitPlayer on some modules; the 1993 build plays them
#   ADPCM2/3: EP206's ADPCM played silent in the engine before the host Caused a direct-mode
#     AudioStruct's interrupt (its refill on channel 0's blocks, README); it plays now and is the
#     reference; ADPCM-Player (SOAMC, 1995), which decodes the same files, stays the fallback
#   Hippel COSO: EP206's Hippel-COSO plays some modules silent; UADE's Jochen Hippel COSO plays them
#   ProTracker: the port plays a Titanics Player module through ProWizard's conversion (a 31-sample
#     module on the built-in ProTracker); its original is UADE's Titanics Packer, which no name the
#     built-in's search asks for runs. Its check refuses plain modules (they are not Titanics')
#   Ultimate SoundTracker: a plain module plays on UADE's Ultimate Soundtracker
ALT_ORIGINALS = {
    'maxtrax': [('soamc', 'MaxTrax_(Music-X_Compatible)/1.1_(20.Sep.2007)/MaxTrax', None, True)],
    'frededitor': [('soamc', 'FredEditor_also_called_Fredmonitor_and_Fredmon/1.1_(30.Aug.1993)/Fred', None, True)],
    'adpcm2': [('soamc', 'ADPCM_mono/x.x_(xx.xxx.1995)/ADPCM-Player', None, True)],
    'adpcm3': [('soamc', 'ADPCM_mono/x.x_(xx.xxx.1995)/ADPCM-Player', None, True)],
    'hippelcoso': [('uade', None, 'Jochen Hippel COSO', True)],
    'protracker': [('uade', None, 'Titanics Packer', False)],
    'ultimatesoundtracker10': [('uade', None, 'Ultimate Soundtracker', False)],
    # (FastTracker 4ch, a `4CHN` module on the built-in ProTracker, has none: PTK-Prowiz and uade123
    # refuse it (MOD_PC); PS3M plays it through its own mixer, 13 dB under the port and openmpt123,
    # which agree within 1.3 dB as on a plain ProTracker module: another output stage, no reference)
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
    args = ['-1', '-t', str(ctx.secs), f'--frequency={ctx.rate}', f'--filter={filt}', '-p', f'{1 - sep:.3f}', '-G', f'{vol:.3f}']
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
    data = read_raw(raw, ctx.rate)
    if not data[1].any():
        return None
    return dict(kind='uade123', wav=raw, data=data, out='uade123 ' + info, led='', player='uade123: ' + info, o={}, note='')


# UADE's PTK-Prowiz ("Protracker and family", the uade123 reference of plain ProTracker modules) clears the
# first long of every sample before it plays, looped or not: in UADE's DMA fetch log of the 26 corpus
# modules it is the uade123 reference of (20 s each), all 182 samples fetched read 0000 0000 at their first
# 4 bytes, 79 of them not zero in the file (54 with a loop start above 0, 16 with a loop from the start, 112
# one-shot). ProTracker 2.3D, which the port follows, clears only the first word of a sample that does not
# loop (mt_init). A loop from the sample's start then plays 4 zero bytes on each pass in the reference: on
# 16-32 byte loops a different timbre (MOD.Albatrossen: DIFF). Such a module is compared on a copy with
# the first long of each sample cleared, on BOTH sides: the port stays 2.3D-correct and the reference's
# quirk leaves the comparison. Only the 31-sample layouts (the tags the measured files have).
PTK_CLEAR_TAGS = (b'M.K.', b'M!K!', b'M&K!', b'N.T.')
PTK_CLEAR_NOTE = 'PTK-Prowiz clears each sample\'s first long: both sides on a copy with it cleared'


# Where PTK-Prowiz's ProWizard conversion of a packed module is known to misread it: a note on the row (the class
# stays; the module, not the port, is what the reference gets wrong). Checked on the converted module PTK-Prowiz
# leaves in chip memory (a uadecore copy dumping chip RAM, run by uade123 -u), against the port's conversion, row by
# row on the rows that play (up to the first B/D), position by position, on all 66 The Player 4.1A files of the
# corpus (Allister Brimble's games; no P40A/P40B, no other P4x file on disk):
#  - 48 convert identically. In the 17 with a Bxx, PTK-Prowiz writes the stored B parameter as is, the port
#    (NostalgicPlayer's ThePlayer4xFormatBase) halves it. The stored values are all even (2, 4, 18) and in 3 files
#    exceed the song's positions (Primal Rage Game_Over and Ingame_18: 4 of 3 positions; Crash Dummies ingame_3: 18
#    of 10); halved, every one is a position of the song (the last, or 1-2 after the intro). The stored value is the
#    position times 2: the reference's jump target is wrong (beyond the 20 s window in every refcmp row).
#  - Body Blows Galactic p4x.titanica: a note with a negative Axx (stored A F1) loses its note and sample in
#    PTK-Prowiz (00000A10; port 00D66A10), followed by 14 rows of portamento down that only make sense on that note;
#    mutated copies show PTK-Prowiz drops the note whenever the A parameter is >= 0x80.
# libxmp 4.6.3's p40.c (MIT, Asle's ProWizard like PTK-Prowiz) also keeps B as stored; it does not drop the note.
REF_NOTES = {
    # UADE's Titanics Packer runs this one module at 25 Hz (CIA-A timer period 28375 CIA clocks = 2 x 14187 + 1,
    # every interrupt from the first) and the other 11 at 50 Hz (14188 = the DeliTracker default 14187 + 1). The
    # rate is keyed to ONE value: the end address (address + 2 x length) of the last sample entry being exactly
    # 126346, with the file at least that long. Measured on copies (tools: UADE_WRITE_LOG, INTREQ bit 3 spacing):
    # every other end from 126300 to 126398 (step 2) gives 50 Hz, as does end 126346 in a file cut 1 byte short;
    # appending 1..200000 bytes, changing any pattern/sample data, any other entry's fields, or splitting the
    # same end as another address/length keeps 25 Hz; Deluxe View with its last sample lengthened to end at
    # 126346 turns 25 Hz (and at 126344 or its own end 144102 stays 50 Hz). So it is the player's own
    # identification of this demo (a constant), not a field of the format and not a read past the file's end.
    # The module is meant for it: its first row is F04 (speed 4), 187 BPM at 50 Hz and 94 BPM at 25 Hz, where
    # the other 11 files set no speed (6 ticks, 125 BPM). The port's conversion plays it at 50 Hz (twice too
    # fast; a ProTracker module cannot express 25 Hz exactly: F3E = 24.8 Hz, F3F = 25.2 Hz) and is not changed.
    'Titanics Player/New A500 Demo.tip': 'reference runs at 25 Hz by a constant in the original player (last '
                                         'sample end = 126346), port converts to a 50 Hz module: twice too fast',
    # its track data is mostly 68k replay code and period tables (only 0x106-0x18b is track rows, duplicated): the
    # tracks of 8 of its 9 patterns point into code. PTK-Prowiz decodes the four voices row by row, ends the pattern
    # for all four at the first fresh B/D row, and keeps each voice's pending repeat / empty-row count into the next
    # pattern; the port (and libxmp) decode each track on its own from its offset. On the 65 well-formed files the
    # two readings give the same rows (each track's counts end at its pattern's end); here pattern 0's voice 3 is a
    # D01 repeated 122 times (period-table words read as a row), so PTK-Prowiz's patterns are 1 row long and the song ends after 0.7 s (port 12.4 s).
    'Incredible_Crash_Dummies/p4x.unused': 'module broken (track data overwritten by replay code): PTK-Prowiz carries '
                                           'repeat counts across patterns, patterns 1 row, song 0.7 s',
}


def ref_note(path):
    """The REF_NOTES note of the module at `path`, or ''."""
    return next((n for k, n in REF_NOTES.items() if path.endswith('/' + k)), '')


def ptk_cleared_copy(ctx, mod):
    """A copy of the 31-sample ProTracker module `mod` with each sample's first 4 bytes cleared, or None when
    the port already plays the same bytes as PTK-Prowiz (every first long zero, but the first word of a
    one-shot sample, which the port clears too)."""
    try:
        with open(mod, 'rb') as fh:
            d = bytearray(fh.read())
    except OSError:
        return None
    if len(d) < 1084 or bytes(d[1080:1084]) not in PTK_CLEAR_TAGS:
        return None
    off = 1084 + (max(d[952:1080]) + 1) * 1024
    differs = False
    for i in range(31):
        h = 20 + 30 * i
        ln, rep, rl = (int.from_bytes(d[h + k:h + k + 2], 'big') for k in (22, 26, 28))
        if ln:
            first = range(2 if rep + max(rl, 1) <= 1 else 0, 4)   # the port clears the first word of a one-shot
            differs |= any(off + k < len(d) and d[off + k] for k in first)
            d[off:min(off + 4, len(d))] = bytes(max(0, min(4, len(d) - off)))
        off += ln * 2
    if not differs:
        return None
    dst = os.path.join(ctx.cache, 'ptkclr-' + hashlib.sha1(mod.encode()).hexdigest()[:12])
    os.makedirs(dst, exist_ok=True)
    out = os.path.join(dst, os.path.basename(mod))
    with open(out, 'wb') as fh:
        fh.write(d)
    return out


# Atari ST SNDH: the port's reference is AtariAudio 1.30 (README), built in the scratch dir from its
# repository (fixround/port-sndh/mkref.sh: `ref FILE OUT.wav LOG SUBSONG SECONDS`, 48 kHz mono, its
# first line `... subsongs=N default=D ...`). Its level is the engine's at volume 0.8 (measured: the 5
# corpus files within 0.13 dB, every band alike), so it is compared only at that volume; the port's
# SNDH output is mono, the same on both channels, so the separation does not enter.
SNDH_ORACLE = SCRATCH + '/fixround/port-sndh/ref/cbuild/ref'
# the same build with the rate as its 6th argument (main.cpp's 48000s read from argv; equal at 48000)
SNDH_ORACLE_RATE = SCRATCH + '/fixround/rate441/oracles/sndh/ref'


class NoRate(str):
    """A NOREF note that says the reference tool cannot render at the run's rate (class NORATE)."""


def render_sndh_oracle(ctx, mod, po, pplay):
    """AtariAudio's render of an SNDH file for the port's subsong, or a NOREF note (str)."""
    if not os.path.exists(ctx.sndh_oracle):
        return 'no AtariAudio reference build' if ctx.rate == RATE else NoRate(f'no AtariAudio build that renders at {ctx.rate} Hz')
    if po.get('volume') != '0.80' or po.get('mono', '0') != '0':
        return f'AtariAudio is compared at volume 0.80 only (port: volume {po.get("volume")})'
    m = re.search(r'\(current (\d+)\)', pplay or '')
    sub = int(m.group(1)) + 1 if m else 1
    h = hashlib.sha1(json.dumps([int(os.path.getmtime(ctx.sndh_oracle)), mod, sub, ctx.secs, *rate_key(ctx.rate)]).encode()).hexdigest()[:20]
    wav = os.path.join(ctx.cache, h + '.sndh.wav')
    inf = wav + '.txt'
    if not os.path.exists(inf):
        try:
            r = subprocess.run([ctx.sndh_oracle, mod, wav, wav + '.ymlog', str(sub), str(ctx.secs),
                                *([str(ctx.rate)] if ctx.rate != RATE else [])], capture_output=True, text=True,
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


# Formats no Amiga player gives a reference for in the engine (README: the Eagleplayers are silent or
# crash under this hosting), checked against an installed black-box decoder or emulator, which is the
# reference the README names for them: normalised port format -> tool.
#   sox: IFF 8SVX (PCM), AIFF, RIFF WAVE (PCM, float, Microsoft ADPCM), decoded at the file's own
#     rate (sox 14.4.2, no dither, 32-bit float), then brought to 48 kHz by the port's documented rate
#     conversion (src/aiff.rs: linear interpolation up, the mean of the source frames under each
#     output frame down), the port's recorded-audio stage (no filter, no separation, gain volume/0.8).
#     Calibration: on the 44.1 kHz and 48 kHz files the decoded samples agree to 1 LSB; the README
#     checked the decoders sample for sample against sox and ffmpeg at the file's rate. sox does not
#     read Fibonacci-delta 8SVX and ffmpeg decodes it low nibble first (README): those are checked
#     against the original Eagleplayer's data instead (render_8svx_dma below).
#   ffmpeg: MPEG audio layers 1-3, decoded by ffmpeg 6 at the file's rate (float), then as for sox
#     (the port decodes with symphonia-bundle-mp3 onto the same recorded-audio stage). IFF 16SV
#     (16-bit big-endian PCM; sox 14.4.2 refuses a FORM that is not 8SVX), likewise: ffmpeg's iff
#     demuxer plays the whole BODY. Calibration: the 12 archive files (SoundBox's, mono, 16-44.1 kHz)
#     decode sample for sample equal to the port's at the file's rate.
#   gme: ZX Spectrum AY (ZXAYEMUL) and NSF through ffmpeg's libgme, 48 kHz, the port's subsong (only
#     where their original, ORIGINAL_BEFORE_BLACKBOX, gives no render). GME is
#     another emulation (its own AY level curve and output filters): no file plays alike, the class
#     says how far the port is from it, not whether it is exact.
#   openmpt123: Symphonie / Symphonie Pro (the port follows libopenmpt 0.8.7; openmpt123 0.7.3, 48 kHz,
#     linear interpolation, 16-bit, no dither). Calibration: the X-Tracker port, which follows the same
#     libopenmpt, is SIMILAR against it on 4 of 4 modules (level 0.00 dB, 39-77 % of frames equal).
SOX = '/usr/bin/sox'
FFMPEG = '/usr/bin/ffmpeg'
OPENMPT123 = '/usr/bin/openmpt123'
BLACKBOX_REF = {'iff8svx': 'sox', 'iff16sv': 'ffmpeg', 'aiff': 'sox', 'riffwave': 'sox', 'riffwaveadpcm': 'sox',
                'iff8svxfibonacci': None, 'zxayemul': 'gme', 'nsf': 'gme',
                'symphonie': 'openmpt123', 'symphoniepro': 'openmpt123',
                'mpegaudiolayer1': 'ffmpeg', 'mpegaudiolayer2': 'ffmpeg', 'mpegaudiolayer3': 'ffmpeg',
                'delusiondigitalmusicformat': 'openmpt123'}
# X-Tracker (DDMF; the format name carries the file's version and tracker, matched by this prefix): the
# port follows libopenmpt 0.8.7 with its own mixer (no Paula, no filter, the module's panning: output
# stage filter=none sep=libxmp(100) resampler=libxmp-mixer, README). Its Amiga original, the DeliPlayer
# "XTracker" (SOAMC V0.8), mixes in software onto Paula through the A500 filter at separation 0.70: an
# output stage the port does not have (refcmp's STAGE-MISMATCH on all 4 rows), so openmpt123 is the
# reference, at the port's stage (48 kHz, linear interpolation, full stereo, no dither).
BLACKBOX_PREFIX = ('delusiondigitalmusicformat',)
# libxmp-port formats where the port loads modules libxmp 4.7.4 refuses (README): for those modules
# alone (the oracle prints "-\t0\tload") openmpt123 is the reference, at the port's stage. Digitrakker:
# the port lifts libxmp's 512 KB cap on a packed 8-bit sample. Un4seen MO3: libxmp has no loader (the
# port reads the file as libopenmpt 0.8.7 does and plays the module put back into its format on the
# libxmp port's loader of that format). The port plays them on libxmp's mixer, openmpt123 on its own:
# at best SIMILAR. MadTracker 2, DSIK RIFF DSM, Dynamic Studio and Velvet Studio / Extreme's Tracker AMS: no libxmp loader
# either (the port reads them as libopenmpt 0.8.7 does and plays them put into IT or S3M, src/mptconv).
XMP_REFUSED_BLACKBOX = ('digitrakker', 'un4seenmo3', 'madtracker2', 'dsikriffdsm', 'velvetstudioams',
                        'extremestrackerams', 'dynamicstudio')


def blackbox_key(fmt):
    """The BLACKBOX_REF key of a port format name, or None."""
    n = _norm(fmt)
    return n if n in BLACKBOX_REF else next((k for k in BLACKBOX_PREFIX if n.startswith(k)), None)

# Formats of BLACKBOX_REF whose original Amiga player does run in the engine: it is the reference,
# the black-box tool only the fallback (the original renders nothing or silence). Normalised port
# format -> (player directory, --player name, output-stage flags given to BOTH sides).
#   NSF: the PlayerStore DeliPlayer "NSF" (V1.00, 3.8.00, "based on CoolNESs"; no library, config or
#     companion) plays the 4 corpus files: two pulse voices, the triangle and the noise on Paula
#     voices 0, 3, 2, 1 as short looped waveforms, PLAY every 59115 colour clocks (60 Hz, NTSC). It is
#     another emulation, not the hardware: its Paula periods put the NES timers 139 cents flat (Paula
#     period 4.2944 x (timer + 1) with 8-byte pulse waves: an NES clock of 1.652 MHz, where NTSC is 1.789773
#     and PAL 1.662607 MHz; the port and libgme agree on the NTSC pitch to 0.5 Hz), no DMC, Paula's
#     64-step volume for the NES's mixer. Rendered with --filter none: the port models the NES's own
#     output filters, the A500's is no part of the sound (the port's recorded-audio stage ignores the
#     flag: same samples with and without it), and both sides get it so the stages compare equal.
#   ZX AY (ZXAYEMUL): EP206's Eagleplayer "PlayAY" (V2.1, Raxoft; "AY-3-8912 Emulator interface"),
#     which LoadSegs its AYPlayer "AYPlayers/ZXAYEMUL" (Z80 Emul Player 1.1, a Z80 interpreter run as a
#     process of its own, the AY's voices mixed on the amplifier's 4 AudioStructs) from the directory
#     EPG_EagleplayerDirArrayPtr names (the engine: the Eagleplayers directory, where EP206 keeps
#     AYPlayers/). Another emulation, not the hardware. Rendered with --filter none for the reason given
#     for NSF (the Spectrum's sound has no A500 filter). The two "AY Megademo" files (digital samples, no
#     refcmp row) do not play: their Z80 code never lets the emulator's process wait.
# Values: (directory, --player name, output-stage flags given to BOTH sides, the directory holds Eagleplayers).
ORIGINAL_BEFORE_BLACKBOX = {'nsf': (DELI_STORE, 'NSF', ('--filter', 'none'), False),
                            'zxayemul': (EP206, 'PlayAY', ('--filter', 'none'), True)}


# IFF 8SVX Fibonacci: no black-box decoder reads it right, and the original's audio is not comparable
# with the port's (the port plays the decoded samples on the recorded-audio stage at the file's rate,
# linearly interpolated to 48 kHz; the original plays them on Paula at a period). The reference is the
# original's DATA: EP206's Eagleplayer "IFF 8SVX" (V0.84, 08.01.97) runs in the engine (its own
# process, made with dos CreateNewProc, reads the BODY through OpenFromLock and decodes it into a
# double buffer refilled from voice 0's AudioStruct interrupt), and every byte its Paula channels 0
# and 1 fetch (DELI_DMA_LOG) is looked for, in 256-byte chunks, in the port's decoded samples
# (amigaplayer-render at the file's own rate: the samples themselves, no interpolation; left for
# channel 0, right for channel 1). SIMILAR: every non-silent chunk found verbatim but at most two per
# channel: the original's stream moves by 2 bytes at its first buffer change (the chunk across it is
# not found), and a one-shot sample's last chunk runs on, in the original, into the start of its next
# pass; the positions are not all the same, so IDENTICAL is not claimed. Measured on the 4 rows: all
# chunks verbatim but those, decoded high nibble first (as the port, the format's specification and
# NostalgicPlayer); decoded low nibble first (ffmpeg's way) not one is.
FIB8SVX_PLAYER = ('ep206', 'IFF 8SVX')
FIB8SVX_CHUNK = 256
FIB8SVX_MISS = 2   # chunks per channel the original may differ in (see above)


def render_8svx_dma(ctx, mod):
    """The data comparison of a Fibonacci 8SVX file with the original Eagleplayer (dict kind 'dma-bytes',
    cls, note) or a NOREF note (str)."""
    render = os.path.join(os.path.dirname(ctx.bin), 'amigaplayer-render')
    src, name = FIB8SVX_PLAYER
    if not os.path.exists(os.path.join(EP206, name)):
        return f'no {name} Eagleplayer in EP206'
    if not os.path.exists(render):
        return 'no amigaplayer-render beside the binary (it decodes at the file rate)'
    try:
        d = open(mod, 'rb').read()
    except OSError as e:
        return str(e)
    i = d.find(b'VHDR')
    rate = int.from_bytes(d[i + 20:i + 22], 'big') if i >= 0 else 0
    if not 8000 <= rate <= 192000:
        return f'VHDR rate {rate} outside amigaplayer-render\'s range'
    st = os.stat(render)
    key = hashlib.sha1(json.dumps([ctx.binkey, int(st.st_mtime), st.st_size, mod, ctx.secs, 'fib8svx-2']).encode()).hexdigest()[:20]
    res = os.path.join(ctx.cache, key + '.fib8svx.txt')
    if not os.path.exists(res):
        base = os.path.join(ctx.cache, key)
        env = dict(os.environ, SDL_AUDIO_DRIVER='dummy', RUST_LOG='error', DELIPLAYERS=ctx.empty, DELIGENIES=ctx.empty,
                   EAGLEPLAYERS=alone_dir(ctx, src, name), DELI_DMA_LOG=base + '.dma')
        r = p = None
        try:
            r = subprocess.run([ctx.bin, '--68k-players', '--player', name, '--wav', base + '.o.wav', '--seconds', str(ctx.secs), mod],
                               env=env, cwd=os.path.dirname(mod), capture_output=True, text=True, timeout=ctx.secs * 4 + 120)
            orig_ok = r.returncode == 0 and 'backend=68k' in r.stderr and os.path.exists(base + '.dma')
            p = subprocess.run([render, '--format', 'raw', '--rate', str(rate), '--seconds', str(ctx.secs), '--silence', '0',
                                '--out', base + '.p.raw', mod], cwd=os.path.dirname(mod), capture_output=True, text=True,
                               timeout=ctx.secs * 4 + 120)
        except subprocess.TimeoutExpired:
            orig_ok = False
        if not orig_ok:
            text = 'NOREF\tthe original ' + name + ' did not run: ' + ((r.stderr.strip().splitlines() or [''])[-1][:80] if r else 'timeout')
        elif not p or p.returncode != 0:
            text = 'FAIL\tamigaplayer-render: ' + ((p.stderr.strip().splitlines() or [''])[-1][:80] if p else 'timeout')
        else:
            port = np.fromfile(base + '.p.raw', dtype='<i2').reshape(-1, 2)
            fetched = {0: [], 1: []}
            for l in open(base + '.dma'):
                v = l.split()
                if len(v) == 4 and v[1] in ('0', '1'):
                    w = int(v[3], 16)
                    fetched[int(v[1])] += (w >> 8, w & 255)
            found = total = chans = 0
            offs = {}
            for ch in (0, 1):
                g = np.array(fetched[ch], dtype=np.uint8)
                nz = np.nonzero(g)[0]
                if not len(nz):
                    continue
                g = g[nz[0]:].astype(np.int8)
                chans += 1
                side = (port[:, ch] >> 8).astype(np.int8).tobytes()
                for k in range(0, len(g) - FIB8SVX_CHUNK + 1, FIB8SVX_CHUNK):
                    c = g[k:k + FIB8SVX_CHUNK]
                    if not c.any():
                        continue
                    total += 1
                    o = side.find(c.tobytes())
                    if o >= 0:
                        found += 1
                        offs[o - k] = offs.get(o - k, 0) + 1
            top = sorted(offs.items(), key=lambda x: -x[1])[:4]
            what = (f'{found}/{total} of the {FIB8SVX_CHUNK}-byte chunks the original\'s Paula fetched are verbatim in the '
                    f'port\'s samples at {rate} Hz (positions port - original: ' + ', '.join(f'{o:+d} x{n}' for o, n in top) + ')')
            if total == 0:
                text = 'NOREF\tthe original fetched only silence'
            else:
                text = ('SIMILAR' if found >= total - FIB8SVX_MISS * chans else 'DIFF') + '\t' + what
        for ext in ('.dma', '.o.wav', '.p.raw'):
            if os.path.exists(base + ext):
                os.unlink(base + ext)
        open(res, 'w').write(text)
    cls, note = open(res).read().split('\t', 1)
    if cls == 'NOREF':
        return note
    return dict(kind='dma-bytes', cls=cls, note=f'{name} Eagleplayer (EP206) Paula fetches: ' + note,
                player=f'{name} Eagleplayer (EP206): Paula DMA bytes')


def original_before_blackbox(ctx, mod, want):
    """The original's render for a format of ORIGINAL_BEFORE_BLACKBOX (dict), or a note (str) on why
    there is none (then the black-box tool is the reference)."""
    pdir, name, flags, eagle = ORIGINAL_BEFORE_BLACKBOX[want]
    if not os.path.exists(os.path.join(pdir, name)):
        return f'no original {name} in {pdir}'
    rw, rplay, rout, rled, rlines = render_engine(ctx, mod, False, pdir if eagle else ('deli', pdir), ('--player', name, *flags))
    o = parse_output(rout)
    if not (rw and rplay and o.get('backend') == '68k'):
        return f'original {name}: ' + (rlines[-1] if rlines else 'no output')[:80]
    data = read_wav(rw)
    if not data[1].any():
        return f'original {name} renders it silent'
    return dict(kind='engine', wav=rw, data=data, out=rout, led=rled or '', player=f"{'ep206' if eagle else 'deli'}+original: {rplay}", o=o, note='')


def port_rate_conversion(x, src, dst=RATE):
    """src/aiff.rs's rate conversion of float frames x (n, ch) from src to dst Hz: the position steps
    by src/dst from 0 (f64, added up frame by frame); upwards linear interpolation in f32, downwards
    the mean of the source frames from the position's frame to that of position + step."""
    step = src / float(dst)
    n = len(x)
    xf = x.astype(np.float32)
    pos = np.concatenate([[0.0], np.cumsum(np.full(int(n / step) + 3, step))])
    pos = pos[pos < n]
    i = pos.astype(np.int64)
    if step > 1.0:
        end = np.minimum(np.maximum((pos + step).astype(np.int64), i + 1), n)
        cs = np.concatenate([np.zeros((1, xf.shape[1])), np.cumsum(xf.astype(np.float64), axis=0)])
        return ((cs[end] - cs[i]) / (end - i)[:, None]).astype(np.float32)
    t = (pos - i).astype(np.float32)[:, None]
    a, b = xf[i], xf[np.minimum(i + 1, n - 1)]
    return np.where(t == 0, a, a + (b - a) * t)


def render_blackbox(ctx, mod, po, pplay, tool):
    """The black-box reference of a BLACKBOX_REF format (dict), or a NOREF note (str)."""
    if tool is None:
        return 'no black-box decoder reads it (sox does not, ffmpeg decodes Fibonacci low nibble first: README)'
    exe = {'sox': SOX, 'ffmpeg': FFMPEG, 'gme': FFMPEG, 'openmpt123': OPENMPT123}[tool]
    if not os.path.exists(exe):
        return f'{exe} not installed'
    if po.get('volume') != '0.80' or po.get('mono', '0') != '0' or po.get('interp', 'linear') != 'linear':
        return f'{tool} is compared at the default output stage only (volume 0.80, stereo, linear interpolation)'
    m = re.search(r'\(current (\d+)\)', pplay or '')
    sub = int(m.group(1)) if m else 0
    h = hashlib.sha1(json.dumps([tool, int(os.path.getmtime(exe)), mod, sub, ctx.secs, *rate_key(ctx.rate)]).encode()).hexdigest()[:20]
    wav = os.path.join(ctx.cache, h + '.' + tool + '.wav')
    rt = ctx.rate
    inf = wav + '.txt'
    cwd = os.path.dirname(mod)
    if not os.path.exists(inf):
        text = ''
        try:
            if tool in ('sox', 'ffmpeg'):
                raw = wav + '.f32'
                if tool == 'sox':
                    q = lambda o: subprocess.run(['soxi', o, mod], capture_output=True, text=True, timeout=60).stdout.strip()
                    rate, ch = int(float(q('-r'))), int(q('-c'))
                    cmd = [SOX, '-D', mod, '-t', 'f32', '-e', 'floating-point', '-b', '32', raw]
                else:
                    pr = subprocess.run(['ffprobe', '-v', 'error', '-select_streams', 'a:0', '-show_entries',
                                         'stream=sample_rate,channels', '-of', 'csv=p=0', mod],
                                        capture_output=True, text=True, timeout=60).stdout.split()[0].split(',')
                    rate, ch = int(pr[0]), int(pr[1])
                    cmd = [FFMPEG, '-nostdin', '-loglevel', 'error', '-y', '-i', mod, '-f', 'f32le', '-c:a', 'pcm_f32le', raw]
                r = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
                if r.returncode == 0:
                    x = np.fromfile(raw, dtype='<f4')
                    x = x[:len(x) // ch * ch].reshape(-1, ch)
                    x = np.repeat(x, 2, axis=1) if ch == 1 else x[:, :2]
                    y = port_rate_conversion(x, rate, rt) if rate != rt else x
                    y = y[:ctx.secs * rt]
                    s = np.clip(np.round(y.astype(np.float32) * np.float32(32768.0)), -32768, 32767).astype('<i2')
                    with wave.open(wav, 'wb') as w:
                        w.setnchannels(2)
                        w.setsampwidth(2)
                        w.setframerate(rt)
                        w.writeframes(s.tobytes())
                    text = f'{"sox 14.4.2" if tool == "sox" else "ffmpeg"} at {rate} Hz, {ch} channel(s), port rate conversion to {rt}'
                else:
                    text = f'{tool}: ' + r.stderr.strip()[:80]
                if os.path.exists(raw):
                    os.unlink(raw)
            elif tool == 'gme':
                r = subprocess.run([FFMPEG, '-nostdin', '-loglevel', 'error', '-y', '-f', 'libgme', '-track_index', str(sub),
                                    '-sample_rate', str(rt), '-i', mod, '-t', str(ctx.secs), '-c:a', 'pcm_s16le', wav],
                                   capture_output=True, text=True, cwd=cwd, timeout=ctx.secs * 4 + 120)
                text = f'libgme (ffmpeg) track {sub}' if r.returncode == 0 else 'ffmpeg libgme: ' + r.stderr.strip()[:80]
            else:
                r = subprocess.run([OPENMPT123, '-q', '--batch', '--force', '--no-float', '--dither', '0', '--filter', '2',
                                    '--samplerate', str(rt), '--subsong', str(sub), '--end-time', str(ctx.secs), '-o', wav, mod],
                                   capture_output=True, text=True, cwd=cwd, timeout=ctx.secs * 4 + 120)
                text = f'openmpt123 0.7.3 subsong {sub}' if r.returncode == 0 else 'openmpt123: ' + (r.stderr or r.stdout).strip()[:80]
        except (subprocess.TimeoutExpired, ValueError) as e:
            text = f'{tool}: {type(e).__name__}'
        open(inf, 'w').write(text)
    info = open(inf).read()
    if not os.path.exists(wav) or not os.path.getsize(wav):
        return info or f'{tool} rendered nothing'
    data = read_wav(wav)
    if not data[1].any():
        return f'{tool} renders it silent ({info})'
    return dict(kind=tool, wav=wav, data=data, out=info, led='', player=f'{tool} (black box): {info}', o={}, note='')


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
    if fmt in DELI_REF and os.path.isdir(DELI_STORE):
        return [('deli', ('deli', DELI_STORE), ('--player', DELI_REF[fmt]))]
    if fmt == 'tfmx15':
        # an early TFMX-SONG module of the fixed tables (nothing at $1D0) with one smpl.set and no
        # smpl.NAME (Einmal Kanzler sein): the TFMX_1.5 Eagleplayer refuses it, and the port follows
        # UADE's TFMX player (README), which the generic search does not reach (Soundtracker IV
        # takes the file first and plays it silent)
        try:
            with open(mod, 'rb') as fh:
                d = fh.read(0x1d4)
        except OSError:
            return None
        base, name = os.path.dirname(mod), os.path.basename(mod)
        stem = name[5:] if name[:5].lower() == 'mdat.' else name
        if (d[:10] == b'TFMX-SONG ' and d[0x1d0:0x1d4] == b'\0\0\0\0' and os.path.exists(os.path.join(base, 'smpl.set'))
                and not os.path.exists(os.path.join(base, 'smpl.' + stem))):
            return [('uade+early', UADE_PLAYERS, ('--player', 'TFMX'))]
        # one whose text starts at $0A (Erno's "TFMX-SONG by ERN0/SB"): TFMX_Pro and TFMX_1.5 refuse it, the
        # port plays it the same way (its samples given as smpl.NAME by stage_tfmx_early_text)
        if early_text_tfmx(d):
            return [('uade+early', UADE_PLAYERS, ('--player', 'TFMX'))]
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
    bbkey = blackbox_key(po.get('format', ''))
    if bbkey:
        why = ''
        if _norm(po['format']) == 'iff8svxfibonacci':
            ref = render_8svx_dma(ctx, mod)
            return ref if isinstance(ref, dict) else dict(none, note=ref + '; ' + render_blackbox(ctx, mod, po, pplay, None))
        if _norm(po['format']) in ORIGINAL_BEFORE_BLACKBOX:
            orig = original_before_blackbox(ctx, mod, _norm(po['format']))
            if isinstance(orig, dict):
                return orig
            why = orig
        ref = render_blackbox(ctx, mod, po, pplay, BLACKBOX_REF[bbkey])
        if isinstance(ref, dict) and why:
            ref['fallback'] = f'REF-FALLBACK (black box: {why})'
        return ref if isinstance(ref, dict) else dict(none, note=(why + '; ' if why else '') + ref)
    if po.get('backend') == 'libxmp-port':
        if not os.path.exists(ctx.oracle):
            if ctx.rate != RATE:
                return dict(none, note=f'no libxmp harness build that renders at {ctx.rate} Hz', norate=True)
            return dict(none, note='no libxmp oracle')
        raw, info = render_oracle(ctx, mod)
        if not raw and info.startswith('-\t0\tload') and _norm(po.get('format', '')).startswith(XMP_REFUSED_BLACKBOX):
            ref = render_blackbox(ctx, mod, po, pplay, 'openmpt123')
            if isinstance(ref, dict):
                ref['player'] = 'libxmp 4.7.4 refuses it; ' + ref['player']
                return ref
            return dict(none, note='oracle: libxmp 4.7.4 refuses it; ' + ref[:80], noref=False)
        if not raw or '\t' not in info:
            return dict(none, note='oracle: ' + info[:100], noref=False)
        return dict(kind='libxmp', wav=raw, data=read_raw(raw, ctx.rate), out='libxmp 4.7.4 ' + info.replace('\t', ' '),
                    led='', player='libxmp 4.7.4 (oracle)', o={}, note='')
    want = _norm(po.get('format', ''))
    if want == 'hivelytracker':
        ref = render_hvl_ticks(ctx, mod, pplay)
        return ref if isinstance(ref, dict) else dict(none, note=ref)
    if want in NO_AMIGA_PLAYER:
        return dict(none, note='no Amiga player plays this format (README)')
    if want == 'sndh':
        # an Atari ST program: the 68k DeliPlayer chain accepts the files and plays them silent (README)
        ref = render_sndh_oracle(ctx, mod, po, pplay)
        return ref if isinstance(ref, dict) else dict(none, note=str(ref), norate=isinstance(ref, NoRate))
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

# HivelyTracker (HVL): no Amiga player plays it (the THX Eagleplayer refuses it), so the reference is the
# original C replayer, hvl_replay 1.9 by Pete Gordon (BSD-3-Clause, LICENSE-hivelytracker), built in the
# scratch dir with a harness that prints, tick by tick, every voice's volume, period, pan and an FNV hash
# of its 640-byte waveform loop (ports/hvl/ref/dump.c: `dump FILE SUBSONG MAXTICKS`). The port prints the
# same lines from its replay core (the ignored test replay::hvl::tests::dump). Audio is not compared: the
# port plays through the Eagleplayer amplifier, the C tool through its own mixer. Two runs of the C tool:
#   tables=port  USEWAVES = the port's waveform tables (rwaves.bin): the full lines must be identical
#   tables=own   the C replayer's own tables, whose noise ($7F, AHX has $80) and floating-point filters
#                differ: volume, period and pan only (the control state) must be identical
HVL_REF_DIR = SCRATCH + '/ports/hvl'
HVL_REF = HVL_REF_DIR + '/ref/dump'
HVL_TICKS_MAX = 100000
_HVL_BUILD_LOCK = __import__('threading').Lock()


def hvl_testbin(ctx):
    """The unit-test executable that holds the port's tick dump."""
    with _HVL_BUILD_LOCK:
        if ctx.hvl_testbin is None:
            r = subprocess.run(['cargo', 'test', '--release', '-j', '4', '--lib', '--no-run', '--message-format=json'],
                               cwd=os.path.join(HERE, '..'), capture_output=True, text=True)
            exe = None
            for l in r.stdout.splitlines():
                try:
                    j = json.loads(l)
                except ValueError:
                    continue
                if j.get('executable') and j.get('target', {}).get('kind') == ['lib'] and j.get('profile', {}).get('test'):
                    exe = j['executable']
            ctx.hvl_testbin = exe or ''
    return ctx.hvl_testbin


def _hvl_run(cmd, env, out):
    r = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=600)
    if out is None:
        return r.stdout
    return open(out).read() if os.path.exists(out) else ''


def render_hvl_ticks(ctx, mod, pplay):
    """Compares the port's per-tick voice state with hvl_replay's. Returns a result dict
    (kind 'hvl-ticks', cls, note) or a NOREF note (str)."""
    if not (os.path.exists(HVL_REF) and os.path.exists(HVL_REF_DIR + '/rwaves.bin')):
        return 'no hvl_replay reference build'
    tb = hvl_testbin(ctx)
    if not tb or not os.path.exists(tb):
        return 'no unit-test binary for the port tick dump'
    m = re.search(r'\(current (\d+)\)', pplay or '')
    sub = m.group(1) if m else '0'
    key = hashlib.sha1(json.dumps([mod, sub, int(os.path.getmtime(HVL_REF)), int(os.path.getmtime(tb)),
                                   os.path.getsize(tb)]).encode()).hexdigest()[:20]
    res = os.path.join(ctx.cache, key + '.hvl.txt')
    if not os.path.exists(res):
        pout = os.path.join(ctx.cache, key + '.hvl.port')
        env = dict(os.environ, HVL_FILE=mod, HVL_SUB=sub, HVL_TICKS=str(HVL_TICKS_MAX), HVL_OUT=pout)
        subprocess.run([tb, 'replay::hvl::tests::dump', '--exact', '--ignored'], env=env, capture_output=True, text=True, timeout=600)
        port = open(pout).read().split('\n') if os.path.exists(pout) else []
        os.path.exists(pout) and os.unlink(pout)
        refp = _hvl_run([HVL_REF, mod, sub, str(HVL_TICKS_MAX)], dict(os.environ, USEWAVES=HVL_REF_DIR + '/rwaves.bin'), None).split('\n')
        refo = _hvl_run([HVL_REF, mod, sub, str(HVL_TICKS_MAX)], dict(os.environ), None).split('\n')

        def ctl(lines):
            return [' |'.join(','.join(v.split(',')[:3]) if i else v for i, v in enumerate(l.split(' |'))) for l in lines]

        def first_diff(a, b):
            for i, (x, y) in enumerate(zip(a, b)):
                if x != y:
                    return i
            return None if len(a) == len(b) else min(len(a), len(b))
        if len(refp) < 3 or not port or port[0].startswith('nosub'):
            text = 'FAIL\tthe reference or the port dump produced nothing: ' + (refp[0] if refp else '')[:60]
        else:
            d1, d2 = first_diff(refp, port), first_diff(ctl(refo), ctl(port))
            n = len(port) - 2
            if d1 is None:
                text = f'IDENTICAL\t{n} ticks, every voice state equal (tables=port)' + (
                    '' if d2 is None else f'; with the reference own tables: control state differs at line {d2}')
            elif d2 is None:
                text = f'SIMILAR\t{n} ticks: volume, period and pan equal with the reference own tables; waveform hashes differ from line {d1} with the port tables'
            else:
                x, y = refp[d1][:90], port[d1][:90] if d1 < len(port) else '(port ended)'
                text = f'DIFF\tfirst difference at line {d1} (tables=port): ref {x} port {y}'
        open(res, 'w').write(text)
    cls, note = open(res).read().split('\t', 1)
    return dict(kind='hvl-ticks', cls=cls, note='hvl_replay 1.9 tick dump: ' + note)


def coso_twin(path):
    """A Hippel 7V module in the COSO container ("NAME (COSO).hip7[.xz]") has, in the corpus, the
    same tune as the game stores it ("NAME.hip7[.xz]"): its path, if that exists."""
    m = re.match(r'^(.*) \(COSO\)(\.hip7(?:\.xz)?)$', path)
    if not m:
        return None
    twin = m.group(1) + m.group(2)
    return twin if os.path.exists(twin) else None


def process(ctx, f):
    """One file: renders, comparison, class. Returns the result row (a dict)."""
    row = {'id': f['id'], 'fmt': f['fmt'], 'path': f['path'], 'cls': 'FAIL', 'note': ''}
    try:
        path = f['path']
        mod = ctx.stage(path) if ctx.stage else _fallback_stage(path, ctx.cache)
        stage_fixed_companions(path, mod)
        stage_maxtrax_samples(mod)
        stage_tfmx_early_text(mod)
        pw, pplay, pout, pled, plines = render_engine(ctx, mod, True, None)
        if pout and _norm(parse_output(pout).get('format', '')) == 'tfmxpro' and retext_empty(ctx, mod) != mod:
            # a TFMX Pro "(Empty) " module's original runs on a retexted copy (retext_empty): the port too, as
            # the original's start-up time depends on the path it loads the samples from (the port's model
            # of it too), and the port plays both texts alike
            mod = retext_empty(ctx, mod)
            pw, pplay, pout, pled, plines = render_engine(ctx, mod, True, None)
        row['out_port'] = pout or ''
        row['led_port'] = pled or ''
        if not pw or not pplay:
            row['note'] = 'port: ' + (plines[-1] if plines else 'no output')[:120]
            # a file no original plays either (a TFMX song without its sample file: TFMX_Pro cannot load
            # the companion, the generic search finds only a player that stays silent) is no failure of
            # the port: nothing to compare it with
            ref = find_reference(ctx, mod, {})
            if ref['kind'] == 'none' and ref['noref']:
                row['cls'] = 'NOREF'
                row['note'] += '; ' + ref['note']
            return row
        row['player'] = pplay
        po = parse_output(pout)
        stage = ORIGINAL_BEFORE_BLACKBOX.get(_norm(po.get('format', '')), (None, None, (), False))[2]
        if stage:
            # the original is rendered at this output stage: the port too, so that the stages compare
            sw, splay, sout, sled, slines = render_engine(ctx, mod, True, None, stage)
            if sw and splay:
                pw, pplay, pout, pled, plines = sw, splay, sout, sled, slines
                row.update(player=pplay, out_port=pout or '', led_port=pled or '')
                po = parse_output(pout)
        with wave.open(pw, 'rb') as w:
            port_short = w.getnframes() < w.getframerate() // 4
        rb = read_wav(pw)
        ref = find_reference(ctx, mod, po, port_short, port_silent=not rb[1].any(), pplay=pplay)
        if ref['kind'] == 'none' and _norm(po.get('format', '')) == 'hippel7v':
            twin = coso_twin(path)
            if twin:
                # the Eagleplayer refuses the COSO container of a 7-voice module: its TFMX twin
                # (the same tune as the game stores it) is the original's reference (README)
                tmod = ctx.stage(twin) if ctx.stage else _fallback_stage(twin, ctx.cache)
                tref = find_reference(ctx, tmod, po, port_short, port_silent=not rb[1].any(), pplay=pplay)
                if tref['kind'] != 'none':
                    tref['note'] = ''
                    tref['player'] += ' (on the TFMX twin ' + os.path.basename(twin) + ')'
                    # a stand-in: the original ran on another file, so count it with the fallbacks
                    tref['fallback'] = ('REF-FALLBACK (the original refuses the COSO container; reference is '
                                        'the original on the TFMX twin ' + os.path.basename(twin) + ')')
                    ref = tref
        if ref['kind'] == 'none':
            row['cls'] = 'NORATE' if ref.get('norate') else 'NOREF' if ref['noref'] else 'FAIL'
            row['note'] = ref['note']
            return row
        if ref['kind'] == 'hvl-ticks':
            row.update(cls=ref['cls'], note=ref['note'], ref_player='hvl_replay 1.9 (BSD-3) tick dump')
            return row
        if ref['kind'] == 'dma-bytes':
            row.update(cls=ref['cls'], note=ref['note'], ref_player=ref['player'])
            return row
        cut_note = ''
        if ref['kind'] == 'uade123' and ref['player'].startswith('uade123: player=Protracker_and_family'):
            clr = ptk_cleared_copy(ctx, mod)
            if clr:
                cw, cplay, cout, cled, _ = render_engine(ctx, clr, True, None)
                co = parse_output(cout)
                cref = render_uade123(ctx, clr, co, _norm(co.get('format', ''))) if cw and cplay and co == po else None
                if cref:
                    rb, ref = read_wav(cw), cref
                    row['led_port'] = cled or ''
                    cut_note = PTK_CLEAR_NOTE
                else:
                    cut_note = 'PTK-Prowiz clears each sample\'s first long; the cleared copy did not render'
        row.update(out_ref=ref['out'], led_ref=ref['led'], ref_player=ref['player'])
        ra, oref = ref['data'], ref['o']
        if ra[0] != rb[0]:
            row['note'] = f'rates {ra[0]} vs {rb[0]}'
            return row
        if (_norm(po.get('format', '')) in ORIGINAL_PLAYS_ON and any(l.startswith('song end after') for l in plines)
                and len(ra[1]) > len(rb[1])):
            ra = (ra[0], ra[1][:len(rb[1])])
            cut_note = 'port song end; original plays on, cut to the port length'
        if ref['kind'] in ('sox', 'ffmpeg') and len(rb[1]) > len(ra[1]) and not rb[1][len(ra[1]):].any():
            # the CLI renders in 0.1 s blocks: past the sample's end the port's last block holds silence
            ra = (ra[0], np.concatenate([ra[1], np.zeros((len(rb[1]) - len(ra[1]), 2), dtype=ra[1].dtype)]))
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
        row['note'] = ' '.join(mm + [n for n in (cut_note, ref_note(f['path'])) if n])
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
    ap.add_argument('--rate', type=int, default=RATE, help='sample rate of both sides (default 48000; each reference '
                    'tool renders at it, or the row is NORATE)')
    ap.add_argument('--baseline', default=None, help='default: tools/refcmp-baseline.tsv at 48000, '
                    'tools/refcmp-baseline-RATE.tsv otherwise')
    ap.add_argument('--update-baseline', action='store_true')
    ap.add_argument('--listen', default=LISTEN_PY, help='listen.py, for its staging of modules with companions')
    ap.add_argument('--oracle', default=XMP_ORACLE)
    ap.add_argument('--oracle-rate', default=XMP_ORACLE_RATE, help='the libxmp harness that takes the rate (used off 48000)')
    ap.add_argument('--hvl-testbin', default=None, help='the lib unit-test executable (has replay::hvl::tests::dump); '
                    'default: built with `cargo test --release --lib --no-run` (honours CARGO_TARGET_DIR / CARGO_BUILD_BUILD_DIR)')
    a = ap.parse_args()
    if not 8000 <= a.rate <= 192000:
        ap.error('--rate must be 8000 to 192000')
    if a.baseline is None:
        a.baseline = baseline_for(a.rate)
    # the reference side needs the 68k hosting: a default build refuses --68k-players (with
    # --wav so that a build that has it does not open a window)
    probe = subprocess.run([a.bin, '--68k-players', '--wav', os.devnull, os.devnull + '.none'], capture_output=True, text=True,
                           env=dict(os.environ, SDL_AUDIO_DRIVER='dummy'))
    if 'built without 68k player support' in probe.stderr:
        ap.error(f'{a.bin} has no 68k player support: build it with `cargo build --release --features host-68k`')
    if a.rate != RATE:
        # a binary without --rate would take the option for a file name: its usage must name it
        probe = subprocess.run([a.bin, '--help'], capture_output=True, text=True, env=dict(os.environ, SDL_AUDIO_DRIVER='dummy'))
        if '[--rate HZ]' not in probe.stderr:
            ap.error(f'{a.bin} has no --rate option (rebuild it from a checkout that has it)')
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
    if a.rate != RATE:
        text += f'\nsample rate {a.rate} Hz (both sides; NORATE: the reference tool cannot render at it)'
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
