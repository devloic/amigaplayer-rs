# Kim Christensen: reference logs

All logs are register-write logs of the 68k reference: the Eagleplayer "Kim Christensen" (UADE
player file `Kim_Christensen`), run through the host with DELI_WRITE_LOG. There is one line per Paula
register write: `ADDRESS VALUE COLOUR-CLOCK`. The first line, `# module BASE SIZE`, gives the module's
load address (`module_base`, 0x0045E0 in every log) and size. An LC write appears as two lines
(high and low word) with the same clock.

Captured with `logs/runref.sh MODULE OUT.log [seconds]`. Set `REF_BIN`, `PLAYERS` (the UADE players
directory) and `EMPTY` (an empty directory). Audio goes to /dev/null with `SDL_AUDIO_DRIVER=dummy`.
The offline render stops one tick after the first song end, or at the time limit.

## Modules

No real module of this format was available, so all modules are synthetic (`modules/`, built by
`modules/build_syn.py`). They are pure data: signature words, the period table, song header,
positions, patterns, instruments, sample list and generated waveforms. They contain no driver code.
Each uses a different `origin` and padding, so the layout search of SPEC 1.2 is exercised. The
`R` and `H` modules are random (seeded) with all instrument fields used. The `H` modules were built and
logged only after every timing constant in SPEC section 4 had been fixed (held out).

| log | writes | speed | repeat | last | song end tick | E0 | exercises |
|---|---|---|---|---|---|---|---|
| syn1 | 3738 | 3 | 0 | 4 | 252 | 74349 | 4-segment volume envelopes, pitch envelopes, vibrato, loop and empty-sample channels, one 0xFF position, period indices up to 54 (table extension of SPEC 1.8 and the divide overflow of 3.3) |
| syn2 | 548 | 0 | 1 | 2 | 31 | 75380 | start at repeat position 1, speed 0 (row every tick), volume target 255 in 1 tick (overflowing delta, VOL 0x3FF), pitch segment of 255 ticks, vibrato step -99 |
| syn3 | 31491 | 2 | 0 | 119 | 5757 (not reached, 60 s) | 74286 | zero song header (defaults 0x02000077), long run |
| synR1 | 2474 | 4 | 1 | 2 | 155 | 74539 | random |
| synR2 | 7715 | 4 | 0 | 6 | 475 | 75137 | random, 0xFF position |
| synR3 | 4749 | 4 | 1 | 4 | 315 | 74591 | random |
| synR4 | 5699 | 5 | 3 | 6 | 378 | 74955 | random, 0xFF position |
| synH1 | 3443 | 2 | 0 | 5 | 189 | 74886 | held out, random, non-zero header byte 1 and instrument byte 17 |
| synH2 | 10784 | 5 | 0 | 7 | 666 | 75327 | held out, random |
| synH3 | 2526 | 0x90 | 0 | 6 | 95 | 75067 | held out, speed >= 0x81 (row every tick) |
| synH4 | 31729 | 0x80 | 0 | 8 | 14319 (not reached, 40 s) | 75614 | held out, speed 0x80 (129 ticks per row), notes 1..42 |

"song end tick" counts from 0 = the first tick. `E0` is the colour clock of the first tick's start
(the host's entry into the player). It is a host property, and tick k starts at `E0 + 70 940 * k` in
every log.

## What they confirm

- Values and order: every write of every log equals SPEC sections 1 to 3 (104 896 log lines:
  56 414 in the 7 fitting logs, 48 482 in the 4 held-out logs).
- Timing: the absolute colour clock of every write equals SPEC section 4, given each log's `E0`.
  That covers 52 439 write events in the fitting logs and 41 667 in the held-out logs (an LC pair
  counts once), with zero differences. The raster-wait model (4.4) is therefore
  checked on 15 168 waits (7 584 ticks) at all beam phases.
- Tick spacing is 70 940 cc.
- `tools/replay-cmp.py` groups writes into ticks by gaps above 20 000 cc. Within one tick the writes
  span at most 12 843 cc (first to last write). The smallest gap between ticks is 57 118 cc, so the
  default works.
