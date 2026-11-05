# Paul Summers: reference logs

Everything here was produced by the reference 68k player run through the host with `DELI_WRITE_LOG`
(silent: WAV output with a dummy audio driver). The `logs/` folder holds the results.

## Files

- `logs/NAME_sK.ref.log`: first line `# module ADDR SIZE` (load address and size of the module image), then one
  line per custom-chip write: register address, 16-bit value (hex), colour clock since reference start.
- `logs/NAME_sK.calls`: one number per line, the colour clock at which each tick call started. Tick n of the
  replay starts at line n. These are host-given (SPEC 5.1); the model adds the cost tables to them.
- `K` is the host's 0-based `--subsong` number. It selects song-table entry K + 1 (SPEC 1.4).
- NAME: `fightingsoccer` (K 0-8), `jingles` (0-5), `p47` (0-4), `postmanpat` (0-2): the four corpus modules.
  `syn1` ... `syn5` (K 0-4; `syn3` 0-3): synthetic modules, edited copies of the P47 data; the edited modules
  are in `mods/` (third-party game data: the lead decides whether they may be published).
- Logs stop after a fixed number of seconds or at song end (plus three ticks, SPEC 3.9). Runs of synthetic
  modules that hit an unspecified case (SPEC 6) end early; that is expected.

## Start of each run

`ts` = clock of the first write, `t0` = first tick call.

| module | ts | t0 for K = 0, 1, 2, ... |
|---|---|---|
| Fighting_Soccer | 9 600 | 63 772, 84 720, 104 413, 111 246, 118 079, 111 632, 131 745, 138 578, 126 742 |
| jingles | 6 663 | 84 159, 90 986, 97 813, ... (+ 6 827 per K) |
| P47_Thunderbolt | 6 975 | 82 383, 89 212, 91 121, 93 690, 109 699 |
| Postman_Pat | 5 456 | 83 623, 90 456, 97 289 |

Layout of the start sequence and of the extra passes for K > 0: SPEC 7.1. The first tick of every run is the
song-start tick (SPEC 3.2). Tick period = 5 x (timer + 1) cc of the song-table entry (default 70 940); the
call clocks in the `.calls` files show the host sometimes deviates for the first call only.

## Typical gaps inside a tick (colour clocks)

Measured over all corpus runs; the exact values are given by the cost tables of SPEC section 5.

| step | gap |
|---|---|
| DMACON stop to AUDxLC | median 205, max 368 |
| AUDxLC to AUDxLEN | 6, usually 8 |
| AUDxLEN to AUDxPER | 6 |
| AUDxPER to DMACON start | 1 963 to 2 224 (contains the first raster wait) |
| DMACON start to silent-block AUDxLC | 2 063 to 2 076 (second raster wait) |
| volume write to volume write | up to 254 |

## Song end

Song-end tick (0-based tick count, approximate time): Fighting_Soccer K 2, 3, 7 (6.1, 6.1, 4.0 s);
jingles K 0-4 (4.0, 4.0, 8.0, 8.0, 10.4 s); Postman_Pat K 1 (3.9 s). Other subsongs loop or run longer than
the recorded length.

## Validation

A private model written only from SPEC.md reproduces value, order and absolute colour clock of every write:
all 23 corpus subsong runs (204 692 tick writes) and 21 synthetic runs (10 454 writes), plus the 20-call-deep
run (1 838 writes). `tools/replay-cmp.py` reports the K = 0 runs of all four corpus modules and of syn1
as identical; runs with K > 0 differ only by the host's extra start passes (SPEC 7.1), which a replayer does
not produce.
