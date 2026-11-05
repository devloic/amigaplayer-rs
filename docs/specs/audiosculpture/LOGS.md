# AudioSculpture: reference logs

All logs are register-write logs (`DELI_WRITE_LOG`: lines `ADDRESS VALUE COLOUR-CLOCK`, first line
`# module BASE SIZE`) of the reference 68k player "AudioSculpture" run through the host with
DELI_WRITE_LOG, rendered offline (`--wav /dev/null`, `SDL_AUDIO_DRIVER=dummy`) until the host stops
a few ticks after the first song end. `logs/runref.sh` shows the command (set `REF_BIN`,
`EAGLEPLAYERS`, `EMPTY`; module paths must be absolute, the companion must sit next to the module).

## Modules

| log | module | companion | subsong | ticks |
|---|---|---|---|---|
| `Daisy.s0.ref.log` | real `Daisy.adsc` (EXO4, 51 positions, 3 sampled instruments) | synthetic | 1 | 18 290 |
| `Muppet-Synthetical.s0.ref.log` | real `Muppet-Synthetical.adsc` (uses E60/E61/E92) | synthetic, empty slots | 1 | 1 290 |
| `Popelich-Brutalo.s0.ref.log` | real `Popelich-Brutalo.adsc` | synthetic | 1 | 1 290 |
| `Turtle_-_Ready.s0.ref.log` | real `Turtle - Ready.adsc` (restart 7) | synthetic | 1 | 2 570 |
| `Turtle_-_Title.s0.ref.log` | real `Turtle - Title.adsc` | synthetic | 1 | 13 495 |
| `fx1/fx2/fx3.s0.ref.log` | synthetic `fxN.adsc` | synthetic | 1 | 4 155 / 1 760 / 2 955 |
| `sub4.s0.ref.log` | synthetic `sub4.adsc` | synthetic | 1 | 2 385 |
| `multi5.s0/s1/s2.ref.log` | synthetic `multi5.adsc` | synthetic | 1, 2, 3 | 1 385 / 975 / 1 375 |

The real modules are the five corpus files of the format (NostalgicPlayer's test set); no real
companion file is available, so every companion is synthetic (`modules/<module>.as`, made by
`modules/gen_as.py`; rename to the real module's file name + `.as` when placing it next to the real
module, e.g. `Turtle - Ready.adsc.as`). The synthetic modules and their companions are in
`modules/` (made by `modules/gen_mod.py`, seeds 1, 2, 3, 4 (`sub` variant), 5 (`multi` variant)).

What the synthetic modules exercise: sample instruments with and without repeat, a sample header of
length 0 without AM record (voice off), six AM instruments covering all waveforms, all envelope
stages and directions, rates larger than the distance, start levels above 256, octave shifts 0..3,
pitch fall, vibrato depths up to 127 and speeds up to 200 (phase wrap), commands 0 to F with random
arguments on sample and AM channels (tone portamento on AM notes, arpeggio on channels without a
note, which reads past the period table, C above 64, F 0 and F above 31, E with empty slots), D, B,
the restart position, song end; `fx*` revisit the first B's pattern (subsong count collapses to 1),
`sub4` repeats the first B's argument (same), `multi5` has three subsongs.

## What they confirm

- Every write (address, value, order, colour clock) of all 12 logs equals the SPEC model, about
  587 000 writes, 0 differences, including the song-end tick (cost 278) and the 1 to 4 ticks the
  host plays after it. Sample addresses are compared as module offsets, waveform addresses by a
  constant offset (as `tools/replay-cmp.py` does).
- First tick entry = last LED write + 71 281 cc in all 12 logs; tick spacing 70 940 cc.
- The LED write's distance to the first init write (3676 + 2 m cc) holds for all 5 real and 5
  synthetic modules (m = 3 to 20).
- The beam reads of the raster waits and the noise generator (clock and horizontal position) equal
  an execution trace of the Muppet run (777 reads, 14 ticks).
- Noise bytes: DMA log of fx1 (20 s, channel playing waveform 3): 4704 of 4706 fetched words equal
  the model's buffer; 2 fetches that fall inside the generator loop differ (README).
- Every code path of the player except the user-code slots ran in at least one of these runs.

## Typical tick-relative times (cc from the tick entry)

Effect tick, all channels plain (command 0, argument 0): PER0..PER3 at +186, +247, +309, +370.
Row tick: DMACON-on lands about 1 100 to 1 600 cc after the last channel write (raster wait);
the loop writes start about 1 134 cc after DMACON-on.
