# Quartet PSG: reference logs

All logs are register-write logs of the 68k reference (the UADE Eagleplayer "Quartet_PSG", run
through the host with DELI_WRITE_LOG, audio to a temporary WAV with `SDL_AUDIO_DRIVER=dummy`,
nothing on speakers). Only this one Eagleplayer was in the player directory, so the player's
buffer block is always at 0xADAC60 (SPEC 8). Each line is ADDRESS VALUE COLOUR-CLOCK. The first
line `# module 0045e0 SIZE` gives where the host loaded the file. The logs are `xz`-compressed;
decompress them before comparing with `tools/replay-cmp.py`. Runs are deterministic. Reproduce
with `logs/runref.sh MODULE MODE SECONDS SUBSONG OUT.log` (environment: REF = the amigaplayer-rs
binary, PLAYER = the Eagleplayer file, EMPTY = an empty directory, WORK = a scratch directory;
mode 1 is selected by the configuration file the script writes, SPEC 3.2).

## Modules (all synthetic)

`logs/modules/SQT.*` were built by `logs/modules/gen.py NAME SEED KIND FRAMES SPEED SUBSONGS`
from the driver source `drv_<variant>.s` (copied to `drv.s`, assembled with vasm, 68000, no
optimisation). KIND chooses the random frame content: `pulse` (tone periods, levels, tone mixer
bits), `noise` (noise period, levels, noise mixer bits), `mixed` (all of these and random mixer
values), `rand` (any bytes, sometimes period 0). Subsong s starts at frame s x FRAMES / 4. The
song ends when the frame table wraps (variants with a site).

| log | module | bytes | variant | kind | frames x speed | subsong | mode | I (InitSound start) | end of last InitSound | writes | song end |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `p1_m0` | `SQT.p1` | 18476 | one | pulse | 1500 x 1 | 0 | 0 | 2030619 | 2031635 | 37596 | yes |
| `p1_m1` | `SQT.p1` | 18476 | one | pulse | 1500 x 1 | 0 | 1 | 2030679 | 2031823 | 15065 | yes |
| `n1_m0` | `SQT.n1` | 18476 | one | noise | 1500 x 1 | 0 | 0 | 2030619 | 2031635 | 38652 | yes |
| `n1_m1` | `SQT.n1` | 18476 | one | noise | 1500 x 1 | 0 | 1 | 2030679 | 2031823 | 15065 | yes |
| `x1_m0` | `SQT.x1` | 18476 | one | mixed | 1500 x 1 | 0 | 0 | 2013874 | 2014890 | 32344 | yes |
| `x1_m1` | `SQT.x1` | 18476 | one | mixed | 1500 x 1 | 0 | 1 | 2013934 | 2015078 | 15060 | yes |
| `r1_m0` | `SQT.r1` | 18476 | one | rand | 1500 x 1 | 0 | 0 | 2030619 | 2031635 | 57396 | yes |
| `r1_m1` | `SQT.r1` | 18476 | one | rand | 1500 x 1 | 0 | 1 | 2030679 | 2031823 | 14759 | yes |
| `s1_m0` | `SQT.s1` | 6476 | one | mixed | 500 x 3 | 0 | 0 | 1907619 | 1908635 | 28618 | yes |
| `s1_m1` | `SQT.s1` | 6476 | one | mixed | 500 x 3 | 0 | 1 | 1907679 | 1908823 | 15059 | yes |
| `u1_m0` | `SQT.u1` | 10092 | one | mixed | 800 x 1 | 0 | 0 | 1944679 | 1945695 | 19214 | yes |
| `u1_m1` | `SQT.u1` | 10092 | one | mixed | 800 x 1 | 0 | 1 | 1944739 | 1945883 | 8065 | yes |
| `u1_s1_m0` | `SQT.u1` | 10092 | one | mixed | 800 x 1 | 1 | 0 | 1944679 | 1946995 | 13993 | yes |
| `u1_s1_m1` | `SQT.u1` | 10092 | one | mixed | 800 x 1 | 1 | 1 | 1944739 | 1947311 | 6095 | yes |
| `u1_s2_m0` | `SQT.u1` | 10092 | one | mixed | 800 x 1 | 2 | 0 | 1944679 | 1948295 | 9260 | yes |
| `u1_s2_m1` | `SQT.u1` | 10092 | one | mixed | 800 x 1 | 2 | 1 | 1944739 | 1948739 | 4125 | yes |
| `h1_m0` | `SQT.h1` | 8884 | one | mixed | 700 x 2 | 0 | 0 | 1932299 | 1933315 | 26640 | yes |
| `h1_m1` | `SQT.h1` | 8884 | one | mixed | 700 x 2 | 0 | 1 | 1932359 | 1933503 | 14062 | yes |
| `h1_s1_m0` | `SQT.h1` | 8884 | one | mixed | 700 x 2 | 1 | 0 | 1932299 | 1934615 | 19793 | yes |
| `h1_s1_m1` | `SQT.h1` | 8884 | one | mixed | 700 x 2 | 1 | 1 | 1932359 | 1934931 | 10592 | yes |
| `h2_m0` | `SQT.h2` | 11276 | one | rand | 900 x 1 | 0 | 0 | 1956819 | 1957835 | 34474 | yes |
| `h2_m1` | `SQT.h2` | 11276 | one | rand | 900 x 1 | 0 | 1 | 1956879 | 1958023 | 8871 | yes |
| `h3_m0` | `SQT.h3` | 7680 | two | mixed | 600 x 1 | 0 | 0 | 1920013 | 1921029 | 13236 | yes |
| `h3_m1` | `SQT.h3` | 7680 | two | mixed | 600 x 1 | 0 | 1 | 1920073 | 1921217 | 6065 | yes |
| `h4_m0` | `SQT.h4` | 4076 | none | pulse | 300 x 1 | 0 | 0 | 1883087 | 1884103 | 43746 | no |
| `h4_m1` | `SQT.h4` | 4076 | none | pulse | 300 x 1 | 0 | 1 | 1883147 | 1884291 | 15015 | no |
| `h5_m0` | `SQT.h5` | 7676 | l | noise | 600 x 3 | 0 | 0 | 1919923 | 1920939 | 37872 | no |
| `h5_m1` | `SQT.h5` | 7676 | l | noise | 600 x 3 | 0 | 1 | 1919983 | 1921127 | 15009 | no |
| `h6_m0` | `SQT.h6` | 12476 | none | pulse | 1000 x 1 | 0 | 0 | 1969187 | 1970203 | 32778 | no |
| `h6_m1` | `SQT.h6` | 12476 | none | pulse | 1000 x 1 | 0 | 1 | 1969247 | 1970391 | 15015 | no |
| `h7_m0` | `SQT.h7` | 4076 | one | pulse | 300 x 1 | 0 | 0 | 1883019 | 1884035 | 8808 | yes |
| `h7_m1` | `SQT.h7` | 4076 | one | pulse | 300 x 1 | 0 | 1 | 1883079 | 1884223 | 3065 | yes |

Run lengths: 40 s requested for p1..u1 and 30 s for h1..h7; runs with a song end stop at the end
of the 0.1 s chunk that holds the report (four ticks after it). `h5` would end after 36 s and so
shows no end within 30 s. `h3`'s two adjacent sites give one patched site (SPEC 2.2: no site test
right after a site). `h4`/`h6` have none and never end.

The I column is computed by the start-up formula of SPEC 3.1. In mode 1 it can be checked
directly: the first line of the log (DMACON 0x000F) is at I + 689. In mode 0 InitSound writes
nothing, so I shows only through the first tick, 70 940 cc after the InitSound end. Mode 0 and
mode 1 of the same module always differ by 60 cc in I and by 188 cc in the InitSound end.

## What they show

- **Mode 1** (`*_m1`): InitSound writes DMACON 0x000F and the four voices' set-up (6.1). Each tick
  then writes, for voices 0, 1, 2, an optional period and a volume, then voice 3's period and
  volume and two DMACON writes. That is 10 writes a tick when all periods are ≥ 123, fewer in
  `r1`/`h2` (period 0 or small). The gaps are fixed costs; for example a tick with three
  periods written shows PER0 at +0, VOL0 +201, PER1 +434, VOL1 +675, PER2 +908, VOL2 +1184,
  PER3 +1226, VOL3 +1525, DMACON +1558 and +1570 (p1, a constant mixer).
- **Mode 0** (`*_m0`): every voice whose status changes gets the restart sequence (5.3), with
  three line syncs and two waits on INTREQR. A voice whose tone and noise are both off is
  restarted every tick. After the stop of a playing voice the wait can be as long as one sample
  of its old period (`x1` has waits of tens of thousands of cc). Such ticks run past the next
  due time, and the following tick then starts at once (SPEC 3.5). `n1`, `p1`, `r1` and `x1`
  cover the accumulator counts 0..3 (tone and noise on the same channel) and the noise voice
  going silent again.
- **Subsongs** (`*_s1_*`, `*_s2_*`): the host starts song 0, then EndSound (DMACON 0x000F and the
  four volumes 0) and a start of each following song up to the requested one (SPEC 3.3).

## Validation

A private model written from SPEC.md runs the modules' driver on the repo's 68000 interpreter
(`m68k-rc`, table cycles) and uses the repo's Paula for the DMACON and INTREQ reads. It does the
patches, sites, start-up and conversions as specified. It matches all 32 logs exactly: address,
value, order and colour clock of every one of the 635 407 writes, start-up included, with no
per-log adjustment. `tools/replay-cmp.py` reports no difference for `x1_m0` against the model's
output. Modules h1..h5 were made only after the model matched the first 16 logs, as a hold-out.
h4 (no song-end site) exposed one wrong cost: the site patch of the walk in SPEC 3.1, 72 cycles,
not 76. With that fixed, h1..h5 and the later h6/h7 (no site / one site at the same size) all
match.
