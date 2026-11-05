# Maximum Effect: reference logs

No real Maximum Effect module was available (see README), so every log was made from **synthetic
modules** written with `logs/gen_modules.py` (a writer of the formats of SPEC.md sections 2 and 3 with
random but well-formed content; the module files used are in `logs/mods/`). The logs are
register-write logs of the reference (the 68k player run through the host with DELI_WRITE_LOG):
one line `ADDRESS VALUE COLOUR-CLOCK` per write to a custom register (or the CIA port A), absolute colour
clock; the first line `# module BASE SIZE` gives the load address and size of the song file in the
reference's memory. A 32-bit address (AUDxLC) appears as two lines with the same clock (high word at the
even register, low word at +2). Ticks are separated by gaps of more than 30 000 cc.

## Files

| file | content |
|---|---|
| `logs/*.subN.ref.log` | log of sub-song N of the module named in front (table below) |
| `logs/mods/max.NAME`, `logs/mods/SMP.NAME` | the song file and sample bank of that module |
| `logs/gen_modules.py` | module writer; `python3 gen_modules.py NAME` writes `max.NAME` and `SMP.NAME` in the current directory (`t1`, `t2`; `fN` = random module of seed N, short form; `LONG=1` for the long form, `EXT=1` / `EXT=2` for the extended command parameter ranges) |
| `logs/timeline_t1_sub0_first_ticks.txt` | the first two ticks of t1, with every write and the beam-read runs collapsed (shows the two raster waits of SPEC 8.4) |

## The modules

| name | how it was made | what it exercises |
|---|---|---|
| t1 | hand-made: 4 voices of 4 notes, 4 samples | note starts only, song end by wrap |
| t2 | hand-made: 5 songs with 2, 2 (voices 1 and 3), 1, 3 voices (masks 0x03, 0x0A, 0x04, 0x07) and 4 voices + play-once (0x1F) | the active-channel table of SPEC 4.3 |
| f1..f6 | random, short form (`gen_modules.py fN`), 2 songs: song 0 = 4 voices looping, song 1 = 4 voices play-once (mask 0x1F) | all commands 0..8, volume slide, vibrato, portamento, arpeggio, CIA commands, restart offsets, keep-volume bit, song end |
| f7 | `LONG=1 gen_modules.py f7`: 25..45 blocks of 5..12 events per voice | the same over about 1 800 ticks |
| f21 | `LONG=1 EXT=1`: command 3 operand up to 255, vibrato depth shift up to 23, wide slide/step values | volume above 64, depth shifts of 16 and more |
| f31 | `LONG=1 EXT=2`: depth shift bytes up to 255 | shift counts above 63 (only the low 6 bits count) |

## Logs

First call = colour clock of the first tick call. Ticks = tick calls in the run. Writes = log lines
(without the header).

| module | sub-song | first call | ticks | writes | note |
|---|---|---|---|---|---|
| t1 | 0 | 75 233 | 29 | 551 | song end at tick index 24 (0-based) of 29 |
| f1 | 0 | 76 272 | 59 | 1 142 | song end |
| f2 | 0 | 76 417 | 109 | 2 363 | song end |
| f3 | 0 | 76 264 | 99 | 1 977 | song end |
| f4 | 0 | 76 325 | 94 | 1 866 | song end |
| f5 | 0 | 76 217 | 119 | 2 488 | song end |
| f6 | 0 | 76 325 | 99 | 2 112 | song end |
| f7 | 0 | 83 834 | 1 804 | 35 271 | song end |
| f21 | 0 | 86 218 | 2 369 | 46 035 | song end |
| f31 | 0 | 85 044 | 1 969 | 38 339 | song end |
| f1 | 1 | 78 230 | 1 499 | 20 005 | play-once song, run cut at 30 s; starts with the host stop group (SPEC 9.2) |
| f2 | 1 | 78 352 | 1 499 | 19 971 | same |
| t2 | 0 | 76 088 | 999 | 12 987 | masks 0x03: 2 voices, nothing plays |
| t2 | 1 | 77 782 | 999 | 12 992 | mask 0x0A: nothing plays |
| t2 | 2 | 79 398 | 999 | 12 997 | mask 0x04: nothing plays |
| t2 | 3 | 81 208 | 999 | 15 702 | mask 0x07: channels 1 and 2 play |
| t2 | 4 | 83 096 | 999 | 13 079 | mask 0x1F: four voices, play-once |

(t2 sub-songs 0..2 look alike: all four channels only receive the end-of-tick DMACON and loop
writes; their lengths differ only by the host groups at the head of the log.)

## Reproduce a reference log (silent)

    E=<an empty directory>
    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=$E DELIGENIES=$E DELI_NATIVE=0 \
    EAGLEPLAYERS=<the UADE players directory> DELI_WRITE_LOG=ref.log \
    <amigaplayer-rs binary> --player 'Maximum Effect' --wav /dev/null --seconds 30 --subsong N /ABSOLUTE/PATH/max.NAME

The sample bank must be in the same directory (`smp.NAME` is looked up as `SMP.NAME`, see SPEC 1.1; the
files in `logs/mods/` already have that name). Nothing is played through the speakers. Add
DELI_TRACE=trace.txt for the guest-call and beam-read trace (lines `C`/`E` for call and return with the
clock, `R 006` for beam reads, `W` for writes). Compare a port's log with `tools/replay-cmp.py REF PORT`.
The sample addresses in the logs are absolute (bank base + offset; the bank is loaded after the song file,
at 0x4DE0 for the small modules, 0x9860 for f10); replay-cmp reduces them.

(The first-call clocks above, and every clock in the logs before the first tick, depend on the length of
the directory part of the module path: the logs were made with a directory part of 96 characters; see
README 3.)

## What the SPEC reproduces

A value model written from SPEC.md only (all numbers of SPEC section 8 typed in from its tables) was run on
all 28 logs I made (the 17 above plus 11 more random modules not shipped: f8, f9, f10, f22, f23, f24, f32 and
the sub-song 1 logs of f3..f6): every write (register, value, order) and every colour clock equals the
reference, 0 differing lines in 615978 lines. Call clocks were taken from the trace; the model's
first-call formula (README 3) was checked on the same 28 runs. The first bug found this way (the
vibrato phase byte keeps only its masked value) is fixed in SPEC 5.7.
