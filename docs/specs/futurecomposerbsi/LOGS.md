# Future Composer BSI: reference logs

All logs are register-write logs of the 68k reference: the UADE player "FutureComposer BSI" (player file
`FutureComposer-BSI`), run through the host with DELI_WRITE_LOG. One line per write:
`ADDRESS VALUE COLOUR-CLOCK`. The first line, `# module BASE SIZE`, gives the module's load address
(0x0045E0 in every log) and size. An LC write appears as two lines (high and low word) with the same
clock. The audio-filter writes of SPEC 3.6 appear as `bfe001` lines.

Captured with `logs/runref.sh MODULE OUT.log [seconds]` (set `REF_BIN`, `PLAYERS` = the UADE players
directory, `EMPTY` = an empty directory; audio goes to /dev/null with `SDL_AUDIO_DRIVER=dummy`). 30 s
per module (5 s for syn5*). The offline render stops a few ticks after the first song end.

In every log the first tick starts at `E0 = 75 559` and ticks are `70 940` colour clocks apart.

## Modules

No module of this format exists in the available corpus (no file with the name prefixes and no file
with the `FUCO` magic), so all modules are synthetic (`modules/`, built by
`python3 -I modules/build_syn.py modules`). They are pure data (magic, sequences, patterns, instrument
records, generated waveforms) and contain no code. The `H` modules were held out: they were built and
logged together with the others but not used while working out the rules and timing; they were
checked only at the end.

| log | lines | song end tick | exercises |
|---|---|---|---|
| syn1 | 302 | 156 | durations incl. 0 (= 64), looped / one-shot / empty / 64-byte instruments, transpose, 0xFC volume, speeds 0, 1, 3, repeat count, song end |
| syn2 | 2 544 | - | portamento modes and speeds, clamps at 127 and 2048, speed added at note start, repeats, 0xFD jumps, filter entries, volume 0x99 and 0, 0xFF loop |
| syn3 | 52 273 | - | speed 0 (every tick), pattern without end marker (ends at position 255), note at offset 255 reached by commands (reading continues into the next pattern) |
| syn4 | 394 | 54 | read limit: 0xFD jump to itself in voice 0; 203 reads in one voice pass (under the limit) |
| syn5a..d | 13..17 | 0 | 0xFE in voices 3, 2, 1 after 0..3 entries, then a looped note in voice 0 in the same tick (song-end cost and clock parity) |
| synR1..R4 | 1 682..2 053 | - | random (seeded) |
| synH1..H3 | 1 259..2 743 | - | held out, random |

"song end tick" counts from 0 = the first tick; "-" = not reached in 30 s.

## What they confirm

- A model of the original's behaviour with the timing rules of SPEC section 5 reproduces every line of
  all 15 logs: values, order and colour clock, with zero differences (68 427 lines; 5 371 of them in the
  held-out logs).
- Tick spacing is 70 940 colour clocks; tick 0 starts at 75 559.
- The raster waits of looped notes (15 line changes each) are covered at many beam phases.
- `tools/replay-cmp.py` groups writes into ticks by gaps above 20 000 colour clocks. The writes of one
  tick can span up to about 27 000 cc (four looped note starts), but the largest gap between two writes
  of the same tick is 3 470 cc in every log except syn4, and the smallest gap between ticks is 43 825 cc,
  so the default works. The exception is syn4 tick 18, where voice 1 makes 203 reads (sequence and pattern) in one
  pass: its writes have a 35 905 cc gap inside the tick, so replay-cmp splits that tick into two groups
  (the same way for a port with exact timing).
