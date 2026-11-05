# Quartet: reference logs

All logs are register-write logs (lines `ADDRESS VALUE COLOUR-CLOCK`, first line `# module BASE SIZE`)
of the reference 68k player (the Eagleplayer "Quartet") run through the host with DELI_WRITE_LOG,
`--player 'Quartet'`, audio to a WAV file with SDL_AUDIO_DRIVER=dummy, 60 s (named set) or 30 s
(random set); every run ends early at the song end (the host stops 1-4 ticks after it). The modules
and sets they were made from are next to them (`logs/mods/`, `logs/rnd/mods/`). No real Quartet
module was available: all modules are synthetic (README).

The module must be given with an absolute path (the host builds the companion's path from the
directory string; a relative path made it look in the wrong place). The first-tick clock depends on
the directory string's length (SPEC 5.5): the named set was run from a directory string of 90
characters, the random set from one of 113 characters.

## Named set (`logs/*.log`)

| log | tempo | rows | what it exercises |
|---|---|---|---|
| basic | 3 | 24 | all samples, all note values 0..15, all octave codes, volume edges, empty entries, b0 = 0 entries, samples of length 0, <= 50, > 50, start offsets, special lengths, loop and one-shot samples; length <= 2 (no loop at the end) |
| slides | 2 | 60 | slide up, down, to the same period, not found within 36 rows, slide whose found row is only a volume-bits hit, target notes 0 and 13..15 (signed comparison), octave 0 and 3 targets, slides on all channels |
| loop | 1 | 70 | length > 2: song end and restart in the same tick; tempo 1; 5 padding words |
| tempo30 | 30 | 3 | tempo 30, short song, a second set layout (all samples loop) |
| wrap | 5 | 66 | slide set-up whose search runs into the end-mark rows; song restart |
| dense | 6 | 40 | a note on every channel in every row, no looping samples, 15 padding words |

## Random set (`logs/rnd/r00.log` .. `r29.log`)

30 modules with random tempo (a valid divisor), 3..40 or 64..120 rows, random entries (35 % empty,
15 % slide markers with random b0, the rest random b0 != 0 and b1 != 0xFE), random sets (lengths 0,
1..50 or 51..3000, random loop word, start offsets and special lengths), random padding 0..15 words.
Every third module is named with a lower-case prefix (`qpa.`), the others `QPA.`.

## What they confirm

- Every write (address, value, order, colour clock) of all 36 logs (48 712 writes) equals a model built
  from SPEC.md, including the absolute clock of every tick (first tick per SPEC 5.5) and the beam-phase
  dependent line waits.
- `tools/replay-cmp.py` on the model's log against the reference: no difference (checked on several).
- Load addresses in these runs (host layout, not meaningful for a port): module 0x0045E0, set 0x004DE0,
  the player's 4-byte zero buffer 0x006B57A8. Set addresses are therefore "other" addresses for the
  comparison tool, and the zero buffer is a second, unrelated one.

## Not covered by logs

- Host volume, balance and voice switching other than the defaults (SPEC 2 gives the rule).
- The `SMP.set` fallback and its start-up cost.
- What happens after a song end of a song with length <= 2 (the host stops).
