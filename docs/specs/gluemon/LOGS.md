# Logs and reproduction

Write-log format: `ADDRESS VALUE COLOUR-CLOCK` per line (DELI_WRITE_LOG), absolute colour clock from
reset; first line `# module BASE SIZE` (load address of the module in the reference's memory, 0x45E0 in
all runs). A 32-bit AUDxLC write appears as two lines (high word, low word) with the same clock.

All logs are of the reference 68k player (UADE players directory, player file `GlueMon`) run through the
host with DELI_WRITE_LOG, `--seconds 20` (every run stops earlier at its song end, SPEC 4.3),
`SDL_AUDIO_DRIVER=dummy`, output to a WAV file that was deleted; nothing was played through speakers.

## Modules

No real GlueMon module was found in the corpus (the module index has no file with the `GLUE` magic; the
files named `glue*` are ProTracker/Soundtracker modules). All modules here are synthetic, built by
`logs/mkmods.py OUTDIR` from the format as described in SPEC 1; copies are in `modules/`. They contain no
code.

| module | md5 | mode | what it exercises |
|---|---|---|---|
| gm4voice | 378b415b4fabae5f1115821b43c2556e | four-voice | notes, note off (0xFE) on voices 2 and 3, decay with floors, 3 order entries |
| gmdrums | df7013ac8bdf1b0dcca62dbd59ea001a | drum ("RM") | all eight drum kinds, 0xFE and other bytes on voice 3 (pattern data as sample), decay wrap (voice 1: start 5, step 7, writes up to 0x3FE), decay 64 from 64 |
| gmstart | 16a35d4bdde6ec27e6d4fe2f9a4cefdd | four-voice | `N` = 2 with no end marker inside the list: playback starts at entry 1, the order walk reads the first pattern byte (0xFF) as the end marker |
| gmfast | b5525494e7fccbeb591deafe5d5f928c | four-voice | speed 1 (a row every tick), 32 rows |
| gmwrap | 24502db99877ba926936abf6b3d5eff3 | four-voice | `N` = 0x6A: pattern area start wraps to file offset 9 (patterns overlay the header) |

## Files

| file | content |
|---|---|
| logs/gm4voice.log, gmdrums.log, gmstart.log, gmfast.log, gmwrap.log | reference write logs (481, 652, 354, 516, 434 writes) |
| logs/mkmods.py | the module generator |

## Measured facts

- Tick call period 70 940 cc in all runs (1 156 calls).
- First tick call: 70 940 cc after the init routine returns; init entered at cc 415; first call at cc 73 092
  (four-voice) and 73 081 (drum mode).
- The player-side buffers in the reference: noise buffer at 0x799248, drum wave at 0x799268.
- Song-end report at tick 96 (gm4voice, gmdrums), 72 (gmstart), 64 (gmfast, gmwrap), counting the first
  tick as 0; the logs end 2, 2, 1, 4, 4 ticks later (host output-block boundary).

## Reproduce a reference log (silent)

    E=<empty directory>
    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=$E DELIGENIES=$E \
    EAGLEPLAYERS=<UADE players directory> DELI_WRITE_LOG=ref.log \
    <reference binary> --player GlueMon --wav out.wav --seconds 20 modules/gmdrums

DELI_DMA_LOG adds audio DMA fetches (`clock channel address word`); DELI_TRACE adds per-call cycle data.
Compare a port's log with tools/replay-cmp.py REF.log PORT.log (values per tick) and, for exact timing,
line by line (address, value, clock).

## What was checked

A private value-and-timing model written from SPEC.md was run on the five modules: 2 437 of 2 437 writes
equal in address, value and absolute colour clock, including the init group, the song-end tick and the
host's stopping point. A DMA log of gmdrums (1 s) was compared with the model's noise buffer and the drum
wave: 784 noise-buffer and 80 drum-wave fetches equal.
