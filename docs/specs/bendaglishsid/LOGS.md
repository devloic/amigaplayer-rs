# Benn Daglish SID: reference logs

All logs are register-write logs of the 68k reference (the UADE Eagleplayer "BenDaglish-SID",
internal name "Benn Daglish SID", run through the host with DELI_WRITE_LOG; audio to a temporary
WAV with `SDL_AUDIO_DRIVER=dummy`). Each line is ADDRESS VALUE COLOUR-CLOCK; the first line
`# module 0045e0 SIZE` gives where the host loaded the module file. They can be reproduced with
`logs/runref.sh MODULE OUT.log [seconds] [subsong]` (it reads REF, PLAYERS and EMPTY from the
environment). `--player` needs the name with the spaces: `'Benn Daglish SID'`. The offline render
stops at the end of the 0.1 s chunk that holds the first song end. Runs are deterministic (a
repeated run gives the same bytes).

## Modules

Three real modules (hunk executables with the `DAGLISH!` header), copied into `modules/`:

| module file | log name | subsongs | writes 10 s | writes full | song end |
|---|---|---|---|---|---|
| `bds.Terramex` (8352 bytes) | `terramex.sub0` | 1 | 8252 | 138601 | 193.9 s (path A) |
| `bds.Chubby_Gristle` (18472 bytes) | `chubbygristle.sub0` | 2 | 4466 | 95178 | 134.4 s (path B) |
| same, subsong 1 | `chubbygristle.sub1` | | 7215 | 13922 | 19.2 s (path B) |
| `bds.The_Flintstones` (18940 bytes) | `flintstones.sub0` | 2 | 7076 | 22149 | 32.2 s (path B) |
| same, subsong 1 | `flintstones.sub1` | | 4402 | 104760 | 173.8 s (path B) |

Files in `logs/`: `<name>.10s.ref.log` and `<name>.full.ref.log.xz` (to song end, xz-compressed;
decompress before comparing). The Terramex 10 s log is identical to the lead's first reference run.

## What they show

- Start-up (SPEC 4.1, 4.3): the init routine's own writes (DMACON 0x000F through the SONGEND site,
  periods 0, lengths 4, ADKCON 0x00FF), no host writes before them; first tick 70940 cc after the
  init call ends (Terramex: init writes from 5832, first tick 77461; the other two: from 7542,
  first tick 79171).
- Subsong 1 (SPEC 4.2): after the subsong-0 init, the stop routine (DMACON 0x000F, volume 0 on the
  four voices, 8 cc apart then 6), then the subsong-1 init 3 kcc later.
- Ticks every 70940 cc: a DMA-off write for the voices that change waveform, location and length
  pairs (ADR sites), the three periods, the WAIT site's line wait (about 2000 cc), DMA on, then
  INTENA 0x8380 (audio interrupts of voices 0-2 enabled).
- Audio interrupts: after each voice's DMA restart, and then once per waveform loop, the driver's
  routine writes that voice's volume, then INTENA (disable) and INTREQ (clear) of that voice's bit.
  Several voices raised together are served by one run of the routine. Channel 3 is never enabled.
- Location values are inside the hunk (hunk address + offset).
- Song end, path A (Terramex): DMACON 0x000F from the SONGEND site inside the tick, then the rest of
  the driver's own re-initialisation; writes after the report show the re-basing of SPEC 3.
- Song end, path B (the other four): at the end of a tick, the stop routine's DMACON and four volume
  writes, then a complete init (the song restarts).

## Validation

A private model following SPEC.md (driver run on the repo's 68000 core with the cycle table, sites
as host traps, audio interrupts through the engine's per-channel call) was compared with every log:
all writes equal in address, value, order and colour clock for the five 10 s logs and the five
full-length logs; `tools/replay-cmp.py` reports "no difference" (all ticks identical) for each
full-length log.

## Not covered

Subsong changes during playback (only the start-time switch of SPEC 4.2 is logged), the host's
voice muting, a module with a fourth volume-load word in its audio routine or with more than one
hunk, and a module whose SONGEND shape appears outside its init routine.
