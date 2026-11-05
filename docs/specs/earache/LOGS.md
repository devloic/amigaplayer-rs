# Logs and reproduction

Write-log format (DELI_WRITE_LOG of the reference): first line `# module BASE SIZE` (load address and
size of the module file in the reference's memory; BASE is 0x0045E0 in every log here), then one line
per Paula register write: `ADDRESS VALUE COLOUR-CLOCK`, the clock counted from the emulated reset.
A 32-bit AUDxLC write appears as two lines (high word, low word) with the same clock. Ticks with no
write leave no line; ticks are separated by gaps of more than 20 000 cc.

All logs are of the reference 68k player (the UADE players directory, player `EarAche`) run through
the host with DELI_WRITE_LOG, `--wav` to a file or /dev/null, `SDL_AUDIO_DRIVER=dummy`, 20 or 30 s
cap; the host stops by itself 0 to 4 ticks after the first song-end report (all logs end that way).
Nothing was played through speakers.

## Modules

No real EarAche module exists in the module index available to this work (file names `EA.*` / `MG.*`
found none; the `earache` hits are ProTracker and XM files). All modules in `modules/` are synthetic,
built for this spec by a generator script; they contain only format data (the `branch` header kind
carries the detection words of SPEC 1.2 and two return words, no player code). Families:

| modules | purpose |
|---|---|
| EA.basic, EA.basic_easo, EA.basic_bra | same song with the three data-start kinds (plain, `EASO`, branch header) |
| EA.edge | notes 0..71 incl. note 0 and 60..71, fine tune at every octave, volumes 0/127, speed 0 (256 ticks per row), three chained song-end markers including self-jumps (16-jump limit and skip) |
| EA.fuzz01..24 | random songs: envelopes with 0xFF loops / stop bytes, pitch tables with loop and clamp, arpeggios with zero offsets, cut lengths, speeds 1..8 |
| EA.chain00..20, EA.desc | song end followed through chains of 0..20 end markers (EA.desc: same as chain01) |
| EA.self1, EA.self2 | end markers jumping to themselves (16-jump limit, skip, repeated song-end reports) |
| EA.hold01..12 | held-out set: built after the timing constants were fixed; random header kind, shuffled sample table (K > 0 in 4.1) |

`EA.basic.pathshort.ref.log` and `EA.basic.pathlong.ref.log` are EA.basic loaded from paths of 16 and
233 characters: byte-identical to `EA.basic.ref.log` (no path-length dependency of the start phase).

## Reproduce a reference log (silent)

    SDL_AUDIO_DRIVER=dummy RUST_LOG=error DELIPLAYERS=$EMPTY DELIGENIES=$EMPTY \
    EAGLEPLAYERS=$UADE_PLAYERS DELI_WRITE_LOG=/path/ref.log \
    $REFERENCE_BINARY --player 'EarAche' --wav /dev/null --seconds 30 /ABSOLUTE/PATH/EA.basic

(`$EMPTY` an empty directory, `$UADE_PLAYERS` the UADE players directory, `$REFERENCE_BINARY` the
host binary that runs the 68k players; DELI_TRACE additionally gives the host's call and beam-read
trace that E0 and the wait check were taken from.)

The registered player name is `EarAche` (no space). Port log: `EAGLEPLAYERS=$EMPTY DELI_WRITE_LOG=...
<binary> --open-replayers --wav /dev/null ...`; compare with tools/replay-cmp.py REF.log PORT.log.
The silence buffer address written on a cut (SPEC 3.4) is 0x007A03C0 in every log, a buffer of the
player outside the module; replay-cmp accepts a constant offset for such addresses.

## Tick entry times

E0 below is the first tick's entry clock in the reference (from the host's call trace); it equals the
formula of SPEC 4.1 for every log. L, N, K as defined there. Every later tick is E0 + 70 940 k.

| module | size | header | L | N | K | E0 | ticks | writes |
|---|---|---|---|---|---|---|---|---|
| EA.basic | 1844 | plain | 4 | 5 | 0 | 72534 | 214 | 1667 |
| EA.basic_bra | 1868 | branch | 4 | 5 | 0 | 72720 | 214 | 1667 |
| EA.basic_easo | 1848 | EASO | 4 | 5 | 0 | 72572 | 214 | 1667 |
| EA.chain00 | 488 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain01 | 492 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain02 | 496 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain03 | 500 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain07 | 516 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain14 | 544 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain15 | 548 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain16 | 552 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain17 | 556 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.chain20 | 568 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.desc | 492 | plain | 1 | 3 | 0 | 72380 | 9 | 121 |
| EA.edge | 984 | plain | 3 | 3 | 0 | 72427 | 609 | 2060 |
| EA.fuzz01 | 1752 | plain | 7 | 3 | 0 | 72521 | 269 | 1821 |
| EA.fuzz02 | 1166 | plain | 5 | 2 | 0 | 72432 | 139 | 1211 |
| EA.fuzz03 | 1734 | plain | 3 | 3 | 0 | 72427 | 69 | 369 |
| EA.fuzz04 | 1428 | plain | 7 | 3 | 0 | 72521 | 299 | 1811 |
| EA.fuzz05 | 2600 | plain | 4 | 6 | 0 | 72576 | 189 | 946 |
| EA.fuzz06 | 1592 | plain | 2 | 6 | 0 | 72529 | 99 | 552 |
| EA.fuzz07 | 1364 | plain | 4 | 4 | 0 | 72492 | 124 | 767 |
| EA.fuzz08 | 1148 | plain | 6 | 3 | 0 | 72497 | 339 | 2135 |
| EA.fuzz09 | 1274 | plain | 3 | 5 | 0 | 72511 | 79 | 598 |
| EA.fuzz10 | 1828 | plain | 6 | 6 | 0 | 72623 | 319 | 2109 |
| EA.fuzz11 | 2180 | plain | 7 | 5 | 0 | 72605 | 349 | 2656 |
| EA.fuzz12 | 2046 | plain | 4 | 5 | 0 | 72534 | 109 | 717 |
| EA.fuzz13 | 1946 | plain | 2 | 4 | 0 | 72445 | 94 | 568 |
| EA.fuzz14 | 1446 | plain | 5 | 2 | 0 | 72432 | 284 | 1462 |
| EA.fuzz15 | 1592 | plain | 6 | 3 | 0 | 72497 | 184 | 1679 |
| EA.fuzz16 | 2028 | plain | 6 | 4 | 0 | 72539 | 64 | 487 |
| EA.fuzz17 | 2072 | plain | 2 | 6 | 0 | 72529 | 169 | 834 |
| EA.fuzz18 | 1516 | plain | 5 | 3 | 0 | 72474 | 319 | 2074 |
| EA.fuzz19 | 1268 | plain | 3 | 2 | 0 | 72385 | 189 | 866 |
| EA.fuzz20 | 1688 | plain | 6 | 3 | 0 | 72497 | 359 | 2644 |
| EA.fuzz21 | 1994 | plain | 3 | 3 | 0 | 72427 | 89 | 829 |
| EA.fuzz22 | 1404 | plain | 3 | 3 | 0 | 72427 | 74 | 541 |
| EA.fuzz23 | 1256 | plain | 4 | 4 | 0 | 72492 | 269 | 1447 |
| EA.fuzz24 | 1562 | plain | 7 | 5 | 0 | 72605 | 249 | 1649 |
| EA.hold01 | 2086 | plain | 4 | 4 | 2 | 72472 | 209 | 1263 |
| EA.hold02 | 2428 | plain | 7 | 6 | 3 | 72617 | 354 | 1762 |
| EA.hold03 | 1512 | plain | 2 | 3 | 1 | 72393 | 104 | 829 |
| EA.hold04 | 1710 | plain | 3 | 5 | 3 | 72481 | 79 | 548 |
| EA.hold05 | 1234 | branch | 2 | 3 | 1 | 72579 | 44 | 406 |
| EA.hold06 | 1990 | branch | 4 | 5 | 3 | 72690 | 239 | 1663 |
| EA.hold07 | 2218 | EASO | 5 | 5 | 4 | 72555 | 314 | 1593 |
| EA.hold08 | 2016 | plain | 3 | 6 | 4 | 72513 | 99 | 855 |
| EA.hold09 | 1410 | EASO | 3 | 5 | 3 | 72518 | 44 | 328 |
| EA.hold10 | 1974 | branch | 5 | 6 | 4 | 72745 | 239 | 1294 |
| EA.hold11 | 1046 | EASO | 5 | 2 | 0 | 72469 | 269 | 1467 |
| EA.hold12 | 1128 | EASO | 7 | 2 | 0 | 72516 | 184 | 1563 |
| EA.self1 | 328 | plain | 1 | 2 | 0 | 72338 | 4 | 47 |
| EA.self2 | 332 | plain | 1 | 2 | 0 | 72338 | 4 | 47 |

## What was checked against the logs

A private value-and-timing model following SPEC.md was run on every log, tick by tick, comparing
address, value, order and colour clock of each write (absolute clock, with E(k) from the formula):

| set | logs | writes | result |
|---|---|---|---|
| fitting set (basic, edge, fuzz, chain, desc, self) | 41 | 36 798 | all equal |
| held-out set (hold01..12) | 12 | 12 745 | all equal |

The beam-wait model (SPEC 4.4) was also checked poll by poll against the beam-read trace of the
reference (DELI_TRACE): every one of the 2 144 waits in the fitting set reproduces the exact sequence
of read clocks.

Independent check: a second model was written by a separate agent from SPEC.md, LOGS.md, logs/ and
modules/ only (no reading notes, no original code), without tuning. It reproduced all 54 logs
exactly (56 163 lines, value, order and clock), with E0 computed from the 4.1 formula. Its list of
ambiguous readings was used to clarify SPEC 3.3 step 6, 3.4 (cut followed by a pitch-table step on
the same tick re-activates the voice; 129 cases in the logs) and 4.2 (F is part of J).
