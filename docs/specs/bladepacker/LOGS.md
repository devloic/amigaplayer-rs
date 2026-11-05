# Blade Packer reference logs

All logs are register-write logs of the reference 68k player run through the host with DELI_WRITE_LOG
(lines: address, value, colour clock). Audio was rendered to WAV with the dummy driver only.
The reference needs upper-case `UDS.` / `SMP.` file names and an absolute module path; otherwise it
reports that it could not load additional files.

## logs/real

Five corpus modules from the game Obsession, subsong 0 each:
Options, Desert_Run, Aquatic_Adventure, Balls_and_Bats, X-ile_Zone (`NAME.s0.log`).
Also one later subsong each of four of them (`Desert_Run.s5`, `Aquatic_Adventure.s10`,
`Balls_and_Bats.s12`, `X-ile_Zone.s7`). In total 94 runs (every subsong of the five modules) were
checked; only the logs above are shipped.

## logs/syn

Synthetic modules: edited copies of corpus modules that change data bytes only. `logs/syn/README.txt`
describes each one (random commands, command E sub-commands, 8-voice layout, word track entries,
command 8 slot shift, pattern break, restart position, song-end re-init). Each `.uds` is the song file;
the sample file is the one of the base module named there.

## What the logs show

- Log structure of a run: one init burst (four volume zeros, a CIA write, DMA enable), then one write
  group per tick at 70940 colour-clock spacing. Gaps are in SPEC.md section 10.
- The host steps to the next subsong when a song ends (about two ticks after the end signal). A log
  therefore holds one segment per init burst, split at the CIA write; segment index equals subsong
  number. Last-programmed addresses and the fifth-channel state carry over a re-init (SPEC section 9).
- A tool, `tools/tick_timing.py`, prints tick starts and gap statistics for any such log.

## Validation result

A model written from the source-derived rules reproduced values and order of every write:
94 real runs, 38 816 ticks, 0 mismatches; 605 synthetic runs, 571 865 ticks, 0 mismatches
except 4 runs that hit the undefined cases listed in README.md. Colour-clock formulas of SPEC.md
section 10: 73 % of 86 113 writes exact, 99.9 % within 9 colour clocks (bus-contention jitter).

The model was not re-implemented a second time from the SPEC text alone; SPEC.md was written to
carry every rule that model uses.
