# Logs and reproduction

All logs are of the reference 68k player run through the host with DELI_WRITE_LOG (UADE players
directory, player "Nick Pelling Packer"), 40 s or until the host stops after a song-end report, with
`--wav` to a file and `SDL_AUDIO_DRIVER=dummy` (nothing was played on speakers). Line format:
`ADDRESS VALUE COLOUR-CLOCK`; the first line `# module BASE SIZE` gives the load address of the song file.
The sample set is loaded elsewhere; its addresses in a log are absolute (sample set base + offset), and
the base differs from run to run (0x4DE0 .. 0x5A68 in these logs). `tools/replay-cmp.py` accepts this as a
constant offset ("player memory: reference = port + ...").

All modules are synthetic (no real Nick Pelling Packer module was available), made by me for this spec;
they are in `modules/<name>/` (`NPP.<name>` + `SMP.<name>`). Every pattern is packed with literal runs and
back references.

| log | module content |
|---|---|
| basic.ref.log | notes on 4 channels, set volume (C), speed changes (F3, F6), pattern break (D10, D0A, D63), samples with every record kind: looped, partly looped, one-shot (repeat length 1), finetune 3, delta-coded with volume 70 and finetune 15, repeat length 3, a name missing from the set; ends by wrap |
| pitch.ref.log | first-ever slide with slide mask 0, slides 1/2 incl. clamping, tone portamento 3/5 both directions with targets reached, glissando (E31) running off the table, arpeggio, vibrato 4/6 with waves 0..2 and E4x, fine slides E1x/E2x, E5x finetune with notes; ends with F00 |
| volfx.ref.log | tremolo with waves 0..2, volume slides A, fine volume EAx/EBx, note cut ECx (incl. EC0), note delay EDx (incl. ED0), retrigger E8x, pattern delay EEx, pattern loop E6x, command 9, position jump B; loops (no song end within 40 s) |
| alias.ref.log | sample set with a zero-length (alias) entry, an unknown name, empty slots; three positions; ends by wrap |
| speed.ref.log | speeds 1, 2, 3, 4 and 31, position jumps and breaks in a loop that never reaches the last position (no song end) |
| rnd1, rnd2, rnd4 .. rnd16 .ref.log | random but well-defined modules (every effect with random parameters, random breaks/jumps, E0x filter writes, EFx funk values) |

A random module "rnd3" retriggered a channel before any sample was set on it (undefined, see README) and is
not included.

## Reproduce a reference log (silent)

    E=<an empty directory>
    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=$E DELIGENIES=$E DELI_NATIVE=0 \
    EAGLEPLAYERS=<UADE players directory> DELI_WRITE_LOG=/path/name.ref.log \
    <reference binary> --player 'Nick Pelling Packer' --wav /path/out.wav --seconds 40 \
        /ABSOLUTE/PATH/modules/name/NPP.name

Give the absolute path of the `NPP.` file: with a relative path the player fails to load the sample set
("could not load additional files"). Beam reads can be traced with DELI_TRACE (used for section 9.3).

## Log structure

- First group: the song start writes (SPEC 5), including one CIA write to 0xBFE001 and the unpack gap.
- Then one group per tick (every tick writes at least the 8 volume/period writes), 70 940 cc apart.
- The CIA-A port writes of E0x (address 0xBFE001) appear inside tick groups; replay-cmp ignores them.

## What was checked against the logs

1. Values: a private model written from SPEC.md compared (address, value) of every audio write, tick by
   tick, for all 20 logs: 31 125 ticks, 0 differing ticks (the reference plays 1-3 more ticks after a
   song-end report before the host stops; those are not compared).
2. Timing: an absolute-time simulation using only the numbers of SPEC 9 (cost table parsed from SPEC.md,
   the wait model, the output-phase and tail offsets), calibrated once on the first tick of each log:

   | | writes | within 1 cc | within 2 cc |
   |---|---|---|---|
   | all 20 logs | 274 481 | 270 525 (98.56%) | 273 559 (99.66%) |

   The rest: ~330 writes 9-12 cc off (a raster wait detecting the line change one poll later or earlier
   than the reference) and ~110 writes about one raster line (226 cc) off (same, at a line edge), plus a
   few hundred at 2-4 cc where several fitted items add up unfavourably.
3. tools/replay-cmp.py on logs written by that simulation against basic, pitch, volfx, alias, speed, rnd5,
   rnd12, rnd16: every compared tick identical, no DMA-gap differences.
