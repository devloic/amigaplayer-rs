# ZoundMonitor: reference logs

All logs in `logs/` are register-write logs (`DELI_WRITE_LOG`: address, value, colour clock) of the
68k reference, the Eagleplayer "ZoundMonitor" (UADE player set, version 1.0 of 2014) run through the
host with `--player ZoundMonitor`, `--wav` output, `SDL_AUDIO_DRIVER=dummy`, default volume and
balance. Each run stops at the song-end signal, plus the ticks the host renders until then.

## Modules

The archive index has no ZoundMonitor module: every `sng`/`.sng` file in it is a ProTracker or
other format, and a scan of all indexed file headers for the detection rule (SPEC 1.5) together
with plausible sample slots found none. All modules are therefore **synthetic**, written by the
generator in `modules/` (`gen.py`, `make_basic.py`, `make_fx.py`; own code from the format
description, no player data). Each module directory holds `sng.<name>` and its `Samples/` files.

| log | speed | positions (start..end) | exercises |
|---|---|---|---|
| basic | 6 | 0..2 | note starts, instrument loads, empty voice, track volume/instrument/transpose offsets, note off by note 63, transposes reaching indices 37..48 (state words), legato restart/no-restart, flags T and N |
| fx | 6 | 1..4 | all effects: arpeggio, slide up/down and over empty rows, portamento with and without transpose of the target, row 31 of a pattern, volume clamping to 0 and 64, instrument offset to 0 and to an empty slot, IFF sample, note off reached by transpose, indices 43/44/66; song wrap to start 1 (ticks after the song end) |
| fx3 | 3 | 0..4 | same data at speed 3 (portamento durations, arpeggio phase against rows) |
| fx2 | 2 | 0..4 | same data at speed 2, trailer `Xamp` (second detection form) |
| fx1 | 1 | 0..2 | speed 1: every tick a row tick, DMA never switched on |

## What they confirm

- A model written from SPEC.md alone reproduces every write of all five logs: address, value,
  order and colour clock, 23 264 writes in 1 660 ticks, zero differences (`tools/replay-cmp.py`:
  identical ticks 379/379, 574/574, 384/384, 259/259, 64/64).
- The tick spacing is 70 940 cc in all logs; the first tick's entry clock is 92 830 (basic) and
  94 344 to 94 378 (the others).
- Together the runs execute every part of the reference's replay routine and of its register
  helpers at least once (checked with the host's execution histogram).
- Sample placement in the logs (host business, not part of the format): each loaded file follows
  the previous one at `previous + size + 16` bytes (all test files are multiples of 8 bytes long, so any rounding is unseen); the first one depends on the
  host (0x4F78, 0x4F98 and 0x4F90 in these runs). The comparison tool handles a constant offset;
  a port placing files differently will differ in LC values only by its own placement.

## Typical gaps (cc), row tick note start (basic)

Pairs differ by the half-cycle rounding of SPEC 5.1.

| voice | DMACON to LC | LC to LEN | LEN to PER | PER to VOL |
|---|---|---|---|---|
| 0 | 38/39 | 10 | 76/77 | 151 |
| 1 | 38/39 | 10 | 92 | 175 |
| 2 | 38/39 | 10 | 107/108 | 194 |
| 3 | 38/39 | 10 | 111/112 | 211/212 |

Effect tick: DMACON (if any) at E+183; the four voices' LC/LEN follow the end of the 9-line wait
at intervals of about 107 cc (no effect), more with a period write.
