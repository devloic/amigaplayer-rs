# Logs and reproduction

The files named below are not shipped; regenerate them as described.
Write-log format: `ADDRESS VALUE COLOUR-CLOCK` per line (DELI_WRITE_LOG), absolute colour clock from
reset; first line `# module BASE SIZE` (load address of the module file in the reference's memory).
Ticks are separated by more than 30 000 cc gaps. All logs are of the 68k reference (the UADE players
directory, `Mark_Cooksey`), 20 s (or until the reference stops at song end) with `--wav /dev/null`,
`SDL_AUDIO_DRIVER=dummy`; nothing was played through speakers.

## Files

| file | content |
|---|---|
| mc.Commando.sub0.ref.log, mc.Commando.sub1.ref.log | N-A, Commando subsongs 0 and 1 (sub 0 ends at song end, tick ~580) |
| mc.Paperboy.sub0.ref.log | N-A |
| mc.title.sub0.ref.log, mc.ingame.sub0.ref.log | N-A, Grand National title / ingame |
| mc.Ghosts_n_Goblins.sub1.ref.log | N-A |
| mc.Mighty_Bomb_Jack.sub0.ref.log, ...sub10.ref.log | N-B (sub 10 ends at song end, tick ~218) |
| mc.Gremins_2_-_The_New_Batch.sub0.ref.log, mc.World_Championship_Soccer.sub0.ref.log | N-B |
| mcr.Aquablast.sub0.ref.log, mcr.Overlander.sub0.ref.log, mcr.A_Question_of_Sport.sub0.ref.log | O (old engine; the `mcs.*` file is loaded from the same directory) |
| *.gaps.tsv | per-write gap tables (tick, index in tick, addr, value, cc from tick start, cc from previous write) from gaps_tsv.py, for Commando sub 1, Mighty Bomb Jack sub 0, Aquablast sub 0 |
| timeline_A_..., timeline_B_..., timeline_Old_... | beam-read and write timelines of the first ticks (DELI_TRACE of the emulator, program counters removed): every raster poll run collapsed to "polls: n first last lasth", each write with its delta from the previous write. The raster-wait model of SPEC 7 was derived from these |
| gaps_tsv.py | the table maker |

Module files (from the Mark Cooksey module corpus; files `mc.*` and `mcr.*`/`mcs.*` per game): md5 prefixes used: Commando d31dd1669cdf, Paperboy 5c3c64987823, Grand National title 02cbeea7a25d / ingame
d6f1c3249807, Ghosts n Goblins bfaa0e06bb66, Gremlins 2 1db2a32bf35d, World Championship Soccer
95183430a368, Mighty Bomb Jack 7c2afb8804b7, Aquablast mcr 099d60f22e44 (mcs e29c0cac1d6b), Overlander
mcr 84a656b8b440 (mcs 29b72580fad8), A Question of Sport mcr 2f41709a6a4f (mcs a7959d545e86).
Live and Let Die and Mike Reads Comp Pop Quiz have the same mcr as Aquablast (same md5).

## Reproduce a reference log (silent)

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR DELI_NATIVE=0 \
    EAGLEPLAYERS=<UADE players dir> DELI_WRITE_LOG=/path/ref.log \
    amigaplayer-rs --wav /dev/null --seconds 20 --subsong N /ABSOLUTE/PATH/mc.Commando

For engine O give the absolute path of the `mcr.` file (a relative bare name without directory makes the
Eagleplayer fail to load the `mcs.` file). Beam trace: add DELI_TRACE=/path/trace.txt (lines `R 006 ...
clk= line= h=` for beam reads, `W reg=value ... clk=` for writes).
Port log: `EAGLEPLAYERS=EMPTY_DIR DELI_WRITE_LOG=/path/port.log <binary> --open-replayers --wav /dev/null ...`;
compare with tools/replay-cmp.py REF.log PORT.log.

Note on the reference's log head: for subsong > 0 and for N-B the log starts with the host groups of
SPEC 3.3 (hardware init / stop group); the first tick follows. The ADDRESS values of sample locations
in the logs are absolute (module base + offset, base 0x45E0 for these runs for N; the `mcs` data base for
O is 0x4F10 (Aquablast, QoS) or 0x5388 (Overlander)); compare modulo that.

## What was checked against the logs

A value model written from SPEC.md was run against the reference logs, tick by tick, comparing (address, value) per write:

| engine | runs (module, subsong) | ticks compared | result |
|---|---|---|---|
| N-A | Commando 0-9, Paperboy 0-3, Grand National title 0 and ingame 0-3, Ghosts n Goblins 0-14 | about 20 100 | 0 differing groups |
| N-B | Gremlins 2 0-7, World Championship Soccer 0-4, Mighty Bomb Jack 0-11 | about 21 250 | 0 differing groups except Mighty Bomb Jack subsong 10: the last 3 groups differ because the reference stops and writes one final volume-0 group at song end (the model keeps running) |
| O | Aquablast 0-1, Overlander 0-4, A Question of Sport 0-2 | about 2 600 | 0 differing groups |

Each run was 30 s (40 s for O) of the reference or until its song end. Ticks without writes are not
logged; the comparison is over the logged groups in order.

Timing was checked only for the note-start sequences and wait primitives (SPEC 7): the segment
lengths and the raster-poll behaviour were read from the beam-read traces of tick 0 of Commando (N-A),
Mighty Bomb Jack (N-B), Aquablast (O) and from gap statistics of the logs (all of the gaps.tsv files and
the longer runs). Gaps with no wait in them are constant per channel to within 1 cc (half-cycle
carry).
