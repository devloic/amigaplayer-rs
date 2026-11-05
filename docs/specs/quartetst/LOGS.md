# Quartet ST: reference logs

All logs are register-write logs. Each line is `ADDRESS VALUE COLOUR-CLOCK`, and the first line is
`# module BASE SIZE`. They come from the reference 68k player (the Eagleplayer "Quartet ST") run through the
host with DELI_WRITE_LOG and `--player 'Quartet ST'`, audio to a WAV file with SDL_AUDIO_DRIVER=dummy, for 30 s
(named set) or 20 s (random set). A run ends early at the song end. The scores and sets the logs were made
from are next to them (`logs/mods/`, `logs/rnd/mods/`). No real Quartet ST module was available, so all of
them are synthetic (README).

Give the score with an absolute path. The host builds the companion's path from the directory string, and
the first-tick clock depends on that string's length (SPEC 5.3). The named set was run from a directory
string of 26 characters, the random set from one of 30.

The tick period is 17 740 cc at 200 Hz, which is below the default tick gap of `tools/replay-cmp.py`. Run it
with `--tick-gap 10000` (and larger gaps only for the WT variant at low speeds).

## Named set (`logs/*.log`)

| log | what it exercises |
|---|---|
| basic | notes, rests, sample changes (looping, one-shot, loop word 0, a 9-byte sample), downward slides that end at once, an upward slide that runs to the next command, song end |
| loops | nested `l`/`L` with counts 0..5; voice 3 has no `P` (default stream, so no song end in 30 s) |
| periods | frequency edge cases: f >> 3 = 0, quotient overflow (0x9E9B), divisor = low word of f >> 3, huge and negative values; slides with large steps; voice 1 has no `P` |
| empty | `V` of an empty slot (empty sample), streams without `P` (default stream), a stream of one `V` only |
| amiga, amiga100 | `WT` header with speed 50 and 100 (tick period 70 940 and 35 470 cc, no halving) |
| upslide | upward slides that end at once (small target), delta 0, negative targets, slides on all voices |
| unknown | an unknown command code (0x41) on voice 3: the voice waits 65 535 ticks; long loops elsewhere |
| lower | score named with lower-case `qts.` (start-up cost) |
| bit2 | `2BIT` set with names and marks; one sample whose mark is missing (slot dropped) |
| oddlen | odd sample lengths, two slots sharing one record, a record whose data lies beyond the file (played unconverted) |
| proj | set of over 83 230 bytes with the `HfR0`/`MP7E` marks: Amiga mode without `WT`, at 200 Hz |

## Random set (`logs/rnd/r00.log` .. `r29.log`)

30 random scores built from `P` (random durations 1..30 and frequencies, including tiny, huge and
overflowing ones but never a zero divisor), `R`, `S` (random targets and deltas), `V` (including empty
slots), and nested `l`/`L` (depth up to 3, counts 0..4). Each set has 1..8 random samples with lengths
2..3000 and random loop words (-1, 0, the length, random). Every fifth score (r03, r08, ...) has a `WT`
header with speed 0, 25, 50, 60, 100, 150 or 200. Every fourth score (r01, r05, ...) is named with a
lower-case `qts.`. Some voices use the default stream.

## What they confirm

- A model built from SPEC.md matches every write of all 42 logs (about 975 000 writes): address, value,
  order and colour-clock stamp. That includes the first tick's absolute clock and the INTREQ acknowledges
  and repeat writes, which depend on Paula's interrupt timing (the model used the project's Paula model for
  the INTREQR reads).
- `tools/replay-cmp.py --tick-gap 10000` reports every tick identical for the model's logs.
- Load addresses (host layout, not meaningful for a port): score at 0x0045E0, empty sample at 0x6AED88. The
  set is at 0x004DE0 for most scores. Larger random scores move it: r01 0x4E08, r06 0x4EA0, r11 0x4FA8,
  r17 0x4F00, r18 0x4F60, r20 0x4EB8, r25 0x4E70. For the comparison tool these are "other" addresses (one
  constant offset per run).

## Not covered by logs

- Host volume, balance and voice switching other than the defaults (SPEC 2 gives the rule).
- The `SMP.set` fallback and its start-up cost. A score whose name does not start with `QTS.` also goes
  through it.
- What follows a song end: the host stops there.
- The `WT` variant with speed <= 10 (timer overflow, SPEC 1.4): derived from the player, not run.
