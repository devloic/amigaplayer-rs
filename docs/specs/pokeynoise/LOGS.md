# Pokeynoise: reference logs

All logs in `logs/` are register-write logs of the reference 68k player run through the host with
DELI_WRITE_LOG (one line per custom-chip write: address, value, colour clock), audio to a WAV file
with the dummy SDL audio driver, Eagleplayer "POKEYNoise V0.25alpha" selected from the UADE player
set. The first line `# module ...` is the host's module address and size. The waveform bank was at
`$706A58` in every run (location writes are that plus the bank offset).

Real modules: the SAP files of ASAP's benchmark set (`test/benchmark/` in the ASAP sources; not
copied here).

| log | module | TYPE | subsong | seconds |
|---|---|---|---|---|
| montezuma_s0, _s1, _s2 | Montezumas_Revenge.sap (SONGS 3) | B | 0, 1, 2 | 20 |
| drunk_s0 | Drunk_Chessboard.sap | B | 0 | 20 |
| karate_s0 | International_Karate.sap | B | 0 | 20 |
| lightness_s0 | Lightness.sap | B | 0 | 20 |
| tempest_s0 | Tempest_2000_Blue_Level.sap (STEREO, second POKEY ignored) | B | 0 | 20 |
| lasermania_s0 .. _s3 | Lasermania.sap (SONGS 4) | C | 0..3 | 20 |
| (none) | Overload.sap | D | - | rejected: initialisation fails |

Synthetic modules (`modules/`, built for this spec; small hand-written 6502 routines):

| log | module | what it exercises |
|---|---|---|
| pn_random1_s0, pn_random2_s0 | pn_random1/2.sap | a table player that copies 9 bytes per tick (AUDF1, AUDC1 .. AUDC4, AUDCTL) to POKEY; random values for all distortions, volumes, the volume-only bit and all AUDCTL bits (joins, clocks, 9-bit flag), channels held for several ticks; 60 s each |
| pn_noise_s0 | pn_noise.sap | same with mostly dist 2 and 6, many changes: random silencing (720 generator steps in 60 s) |
| pn_sweep_s0 | pn_sweep.sap | every second AUDF value for each distortion under AUDCTL 0, 1, $60, $78 (82 s) |
| pn_cpu_s0 | pn_cpu.sap | CPU behaviour probes, one per tick, results written to AUDF1..4: zero flag after ADC/SBC, no decimal mode, BRK skip, V flag, RANDOM through hooked and unhooked reads, `$D01E`/`$D40F` presets, RTC, `JMP ($xxFF)`, `(zp),Y` at `$FF`, TSX in a subroutine, set-vector by JSR and JMP, PHP after PLP, an undocumented opcode (failed tick), JMP to the exit vector, PLA on an empty stack |
| pn_typem_s3 | pn_typem.sap: TYPE M, `SONGS 10`, `DEFSONG 3`, lower-case hex addresses | host reports subsongs 0..15, default 3; the host's `--subsong 3` started subsong 6 (host steps from the default) |
| pn_typec_s0 .. _s2 | pn_typec.sap: TYPE C wrapper (`PLAYER+3` init with A=$70/X,Y=MUSIC then A=0/X=song, `PLAYER+6` play) | type C calling convention, 3 subsongs |
| pn_vcount_s0 | pn_vcount.sap | VCOUNT read at the start of the play routine and after 40 NOPs (4 s) |
| (none) | pn_typer.sap (TYPE R) | rejected: initialisation fails |

## Logs with subsong > 0

The host stops the default song before starting the requested one, so these logs begin with one or
more song-stop blocks (writes of section 2 "Song stop", recognisable by their AUDxDAT writes) before
the first tick. Drop them before comparing.

## What they confirm

- Value and order of every Paula write of every log above except pn_vcount (about 570 000 writes) equal a model
  written from SPEC.md sections 1 to 5, comparing the whole write sequence (the reference has slightly
  fewer writes at the end of each run because overrunning ticks push the last ones past the end).
  Tick grouping by gaps is not usable for the synthetic logs because of 6.5; `tools/replay-cmp.py`
  reports identical ticks for all real-module logs.
- The random generator start values and step (4.5), with JOY1DAT = 0, reproduce every random
  silencing decision and every RANDOM read.
- Every CPU deviation listed in SPEC 3.3 to 3.5 changes the pn_cpu output when undone in the model
  (each was checked by undoing it).
- VCOUNT: values match line / 2 of the raster position at the read (the play routine starts around
  line 28 to 30 in that run, 40 NOPs later VCOUNT is 2 to 3 higher).
- Timing: per-write gaps of SPEC 6.2 to 6.4 are the medians and spreads of all real logs (more
  than 1 900 restarts, 13 000 idle voice writes); 6.5 is from pn_random1 (frame starts against the
  70 940 cc grid).

## Not covered by any run

- Raw binary-load files (`$FFFF` start) with a `.info` icon: no icon-carrying run was made; SPEC 1.5
  is from the player's logic.
- VCOUNT exactness (needs the emulated 6502's run time, which is only estimated).
- Stack overflow by pushing (the code path is the same as the JSR check, untested).
