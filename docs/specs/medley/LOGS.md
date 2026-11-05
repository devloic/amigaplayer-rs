# Medley: reference logs

All logs are register-write logs (`DELI_WRITE_LOG`, lines `ADDRESS VALUE COLOUR-CLOCK`, plus a
`# module BASE SIZE` line) of the reference 68k player run through the host with DELI_WRITE_LOG:
the UADE player "Medley", `--player Medley`, audio to a WAV with `SDL_AUDIO_DRIVER=dummy`,
30 seconds (1 500 ticks) or until the host stops after a song end.

No real Medley module was available (README), so every log comes from a synthetic module built
from the format description in SPEC.md section 1. The modules are in `logs/` next to their logs.

Subsong selection: the host always starts subsong 1 and then switches k - 1 times, so the log of
song k begins with the load writes (4 DMACON) and, for every start 1..k, 4 DMACON stop writes plus
the 4 voice-reset DMACON writes of SPEC 3.2. The tick writes of song k follow.

## Modules and what they exercise

| module | songs | content |
|---|---|---|
| `mso.basic` | 1-6 | type 0 instruments; octave groups 0..7 with slot fallback and waveform octave bytes 0..3; every envelope phase; rests, ties, zero durations, duration byte 0x80; loops; voice volumes 0..63 and master volumes 0, 17, 40, 63; negative global transpose; speeds 0, 1, 2, 0x81; detune patterns 2, 3, 5 and an out-of-range number (9, selects 0) with intervals 1, 2, 3, 7; voices without track; song end (song 6) |
| `mso.mods` | 1-3 | modulator targets 1, 2, 5, 6 with square, invert, free-running, one-shot, delay, both selectors; speeds 1 and 3 |
| `mso.synth` | 1-2 | type 2 at octaves 0..17 (buffer sizes 32..2 bytes, shift cap), zero step nibbles, phase (target 4) and step (target 7) modulation, vibrato on a synth |
| `mso.sample` | 1-2 | type 1 windows (octave shift, second-half flag, window modulation, two window modulators), types 3 and 5 (one-shot) |
| `mso.cmds` | 1-3 | nested calls and loops (loop count 0), commands 1, 2, 3 (incl. entry 0), 8, 9, 12, 15, zero-duration chains, instrument changes between types |
| `mso.rand` | 1-3 | random-triggered voices (several base/range values, list sizes 1, 2, 4), voices going inactive at sequence end; command 4 random jump in a normal track |
| `mso.edge` | 1 | decay borrow, modulator carry, command 4 with m = 0 and with an empty entry, call to an empty entry, call stack full, loop stack full, loop end with no loop |

Lines per log (writes; AUDxLC counts as two):

| log | lines | log | lines |
|---|---|---|---|
| basic song 1 | 8 748 | mods song 1 | 18 828 |
| basic song 2 | 4 700 | mods song 2 | 9 572 |
| basic song 3 | 3 244 | mods song 3 | 4 948 |
| basic song 4 | 4 404 | synth song 1 | 15 708 |
| basic song 5 | 8 780 | synth song 2 | 8 306 |
| basic song 6 | 484 | sample song 1 | 15 588 |
| cmds song 1 | 15 948 | sample song 2 | 5 828 |
| cmds song 2 | 11 972 | rand song 1 | 20 259 |
| cmds song 3 | 484 | rand song 2 | 4 214 |
| edge song 1 | 11 508 | rand song 3 | 3 229 |

## What they confirm

- Values and order: a private model written from SPEC.md (and a second one written independently from the SPEC text alone) produces exactly the reference's
  sequence of (register, value) for all 20 logs above (0 differences, about 150 000 writes),
  including the pre-tick DMACON writes of every subsong start. AUDxLC values: sample addresses are
  module base + offset; the silence block and synth buffers are player memory (compare them as one
  constant offset, as tools/replay-cmp.py does).
- Timing: with the tick call on a 70 940 cc grid (tick 0 phase fitted once per log from the first
  tick write) the colour clock of every tick write equals SPEC section 5 exactly in all 20 logs
  (0 differences, about 8 700 ticks).
- Every cost line of SPEC section 5 was used by at least one of these runs; the rarest (call
  stack full, loop end with no loop, decay borrow, modulator carry) occur 1 to 30 times.
- The song-end re-base (SPEC 5.1) is visible in basic songs 1, 2, 4, 5 (tick 360) and cmds:
  without it those ticks are 1 cc early.
- Random mode depends on the tick counter and seed starting at 0 at load and counting every tick
  (rand songs 1-3 match only with that).

## Not covered

- Pre-tick timing (the spacing of the voice-reset DMACON writes, the first tick's phase) is host
  business and not modelled.
- After a song end the host stops the run within a few ticks; nothing after that is logged.
