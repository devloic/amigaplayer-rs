# Tronic: reference logs

All logs are register-write logs of the 68k reference (the Eagleplayer "Tronic" running the unmodified
module on the emulated machine, run through the host with `DELI_WRITE_LOG` set; audio to a null WAV with the
dummy SDL driver). One line per Paula register write: `ADDRESS VALUE COLOUR-CLOCK`; the first line is
`# module BASE SIZE` (load address and size of the module). A 32-bit AUDxLC write appears as two lines
with the same clock. They were captured with only the Tronic player installed (with the whole UADE player set the host
picks "Soundtracker IV" for the two real modules, README). Each run was long enough to reach the song end
(the host then stops a few ticks later).
`tools/replay-cmp.py REF.log PORT.log` compares logs tick by tick.

## Modules

| name | kind | ticks in log | writes |
|---|---|---|---|
| `crystal.log` (`crystal.dp`) | real, from the demozoo archive | 10 329 | 116 841 |
| `locomotion.log` (`locomotion.dp`) | real, from the demozoo archive | 11 209 | 129 801 |
| `syn01` to `syn08` (`modules/syn01.dp` ...) | synthetic, long sequence lists (10 to 20 entries per voice) | 439 to 1 099 | 5 600 to 13 900 |
| `syn09` to `syn11` | synthetic, short lists of different lengths per voice (song end after a few hundred ticks, or after 6 ticks) | 94 to 484 | 1 200 to 5 700 |

The two real modules are third-party files and are not shipped; the synthetic modules are random
modules built to the structure of SPEC 1.3 (random sequence lists, patterns with all effects 1 to 8,
sample and wave-list instruments with random envelope, vibrato, drift, wave-list jump markers, arpeggio
lists with 0x80 markers). The module files are in `modules/`. Their generator is not shipped.

## What the logs confirm

- Every register write (address, value, order, tick-relative colour clock) of the logs above equals the
  value produced by SPEC 3 to 8, for both real modules in full (10 329 and 11 209 ticks, the last ticks
  included) and for all eleven shipped synthetic runs. 190 synthetic modules were tried in total: 183 equal over the
  whole run, 7 diverge only after an undefined read (SPEC 7.6).
- Tick spacing is exactly 70 940 cc; the first tick of every run starts at cc 73 949.
- The song-end notification of each wrapping voice costs 16 cycles and re-bases the stamps (SPEC 8.2): the
  last tick of both real modules (all four voices wrap in that tick) is exact only with the re-base.
- Effects 3, 4 and 7, the portamento step, the envelope phase wrap, the wave-list jump marker and the
  0x80 end of an arpeggio list never occur in the real modules; their behaviour and timing are
  confirmed on the synthetic runs only.
- The high byte of the operand of effects 3, 4 and 8 comes from the scratch value of SPEC 7.3: about 2 000
  effect operands with a non-zero scratch high byte are in the synthetic runs and match.
- The initial writes (SPEC 6.2) are identical in all logs.

## Typical timing (cc from the tick start)

Ticks of the two real modules that contain only the nine standard writes (4 PER, 4 VOL, DMACON; 299 to 303
distinct values per write over about 20 000 such ticks):

| write | min | max |
|---|---|---|
| ch0 PER, VOL | 453, 642 | 786, 970 |
| ch1 PER, VOL | 1 019, 1 230 | 1 606, 1 817 |
| ch2 PER, VOL | 1 588, 1 832 | 2 509, 2 747 |
| ch3 PER, VOL | 2 208, 2 445 | 3 383, 3 620 |
| DMACON `0x800F` | 2 497 | 3 672 |

The spread comes from the vibrato, envelope, arpeggio and row branches; SPEC 8 gives the exact cost of
each. First tick of every run (default instrument, wave list at once): LC at 301, LEN at 369, PER at 728,
VOL at 912 for channel 0.

## Not matched

Nothing in the real modules. Synthetic modules that read memory outside the module (SPEC 7.6) differ from
that point on, in the value of one PER write or of an LC write.
