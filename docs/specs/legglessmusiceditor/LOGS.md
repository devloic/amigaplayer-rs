# Logs and reproduction

Write-log format: `ADDRESS VALUE COLOUR-CLOCK` per line (the host's DELI_WRITE_LOG), the first line
`# module BASE SIZE` (load address and size of the module file in the reference's memory; base 0x45E0
in all runs). Ticks are separated by gaps of more than 30 000 cc. Logs were made with the reference 68k
player run through the host (the UADE Eagleplayer "Leggless Music Editor"), silent (`--wav` to a null
file, dummy audio driver), 40 s of tick time or until the host stops after the song-end report. The
files in `logs/` are cut to the first 20 s (lines with cc up to 71 000 000).

## Files (logs/)

| file | content |
|---|---|
| lme.game_over.sub0.ref.log | Punisher "game_over", 1 subsong (synth lead, samples, pitch tables, slides, ties) |
| lme.Ninja_Spirit.sub0..sub16.ref.log | Ninja Spirit, 17 subsongs; subsongs 2, 3, 5, 8, 13, 14, 15, 16 end by song end (the host stops three ticks after the report) |
| lme.Ninja_Spirit_Demo.sub5.ref.log | the unused demo version of the same module (386 bytes longer; otherwise the same layout) |

For subsongs other than 0 the log starts with the three host stop groups of SPEC 3.3 (cc 2 790 ...);
the first tick follows. The first tick's entry time (T0 in SPEC 3.1) is the first write of the first
tick minus 951 cc: 72 862 (game_over), 73 716 (Ninja Spirit subsong 0), 74 993 (1), 76 296 (2), 77 625
(3), 78 980 (4), ... growing by about 1 300 per subsong. These start phases matter to the raster-wait
quantisation (SPEC 9.4); a model must be given T0.

## Modules used (real)

Punisher `lme.game_over` (md5 prefix 501ae9aca297), 7 510 bytes; Ninja Spirit `lme.Ninja_Spirit`
(9554f1eb0dfa), 57 342 bytes; Ninja Spirit demo `lme.Ninja_Spirit_Demo` (9035b39f58f1), 57 728 bytes.
These are the only three `lme.*` files in the corpus. Every code path of the replay routine that has
any effect on the writes is executed by them (checked by instruction coverage of the reference binary
over all 35 module/subsong runs).

## Reproduce a reference log (silent)

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR DELI_NATIVE=0 \
    EAGLEPLAYERS=<UADE players dir> DELI_WRITE_LOG=ref.log \
    amigaplayer-rs --player 'Leggless Music Editor' --wav /dev/null --seconds 40 --subsong N /ABSOLUTE/PATH/lme.Ninja_Spirit

The comparison tool of the repo (tools/replay-cmp.py REF.log PORT.log) accepts the model's log in the
same format.

## What was checked

A value-and-time model written from SPEC.md (values from sections 4 to 8; timing from the tables of
section 9) was run tick by tick against the reference logs, comparing (register, value, cc) of every
write in order:

| runs | ticks compared | result |
|---|---|---|
| game_over subsong 0 | 1 999 | 0 differing writes |
| Ninja_Spirit subsongs 0..16 | 23 228 (the song-end subsongs end at the host's stop) | 0 differing writes |
| Ninja_Spirit_Demo subsongs 0, 5, 11 | 4 447 | 0 differing writes |
| 8 modified copies of Ninja_Spirit (instrument fields randomised: vibrato speed, limits, delay, envelope parameters, volume, retrigger period), subsongs 0 and 4, 12 s each | 9 584 | 0 differing writes |

The model was written from the same analysis as the spec; the spec was then read against the model.
Two things about time are only established by the comparison and are in the spec: the 16-cycle cost
and half-cc re-basing of the song-end host call (SPEC 9.5), and the beam sampling at the end of each
read (SPEC 9.4). Both are exact on all eight song-end ticks (the callback tick is the only tick where
they matter).

The contents of the synth buffers (SPEC 5.6), which the write logs cannot show, were compared with
the memory of the reference player executed in a private 68000 interpreter that reproduces the
host's cycle model (it reproduces the reference logs cc-exactly): 4 095 voice-ticks, no difference.

Modified copies are derived from the Ninja Spirit module and are not shipped; the changed fields are
the ones listed above.
