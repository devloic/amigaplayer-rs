# LOGS: reproducing the Dave Lowe (`dl`) start-up and tick measurements

The logs and scripts named below are not shipped (see `docs/specs/README.md` for how to regenerate logs). No sound goes to the
speakers: use `--wav` with `SDL_AUDIO_DRIVER=dummy`.
Binary: a build of this repository (the genuine-Paula state of the gaps round, unmodified).
Modules: the Dave Lowe module corpus (47 files: 45 UNCLEART + `dl.older_*` x 2).
Port logs used for comparison: 100+ ticks per file, named by the md5 of the module file.

## Reference trace + write log of one file (reference = UADE's DaveLowe Eagleplayer run by the 68k engine)

    export SDL_AUDIO_DRIVER=dummy RUST_LOG=error
    DELI_TRACE=X.txt DELI_WRITE_LOG=X.log DELIPLAYERS=EMPTY_DIR DELIGENIES=EMPTY_DIR EAGLEPLAYERS=<UADE players dir> \
        amigaplayer-rs --wav X.wav --seconds 3 MODULE

`X.log`: lines `ADDRESS VALUE CLOCK`. `X.txt`: `C`/`E` lines are call entry/return (clock), `T` host calls (clock), `W` register writes, `H` the
per-call instruction histogram (count and CPU cycles per instruction; summing the cycles of a call's `H` lines gives the call's cyc).
The instruction-level traces name code addresses of the original and are not shipped; everything the spec relies on is stated in `SPEC.md`.

## Measurements (all take the traces as input)

    measure_T.py TRACES/dl.*.txt > measure_T.tsv
        per file: A, T (chain of back-to-back calls ending at the call before the first tick), number of calls, per-call durations and
        per-call CPU cycles, first write inside the chain. First tick = first call starting exactly 70940 after the previous call returned.
    model.py             # the T model of SPEC.md section 2 against rows.json (module quantities + measured components): 45 of 45 exact
    idle.py TRACES > idle.tsv     # idle-tick shape per file (SPEC section 9)
    gaps.py TRACES/dl.*.log       # -> gaps_all.json: median/n/min/max of the gap between consecutive Paula writes inside ticks, by
                                  #    (previous write kind+voice, next write kind+voice); event_gaps.tsv is the same as a table
    intick.py REF.log PORT.log [N] # port vs reference, tick by tick: sequence differences, gap differences, first-write offset

`T_components.tsv` = per file: the module quantities (steps, subsongs, zero-term, L, U, ...), `M`, `X`, `c_IP`/`c_IS` (cycles), the predicted and measured `T`,
the held-out flag (12 of the 38 regular files; the held-out check refit the 7 coefficients of the data-dependent
part on the other 26 and predicted the 12 with 0 cycles error). The module quantities were computed by a script that
reads the module bytes; SPEC section 4 defines each quantity so it can be recomputed.

## First divergence seen (port vs reference), from logs

The port's first write of the first tick sat at a clock that did not depend on the file (92736 or 92793 in the port logs for every
file; the reference's first tick sits at `100 + T + 70940` = 71000 + T, i.e. 74 233 (IK+) to 109 816 (Frontier II) in
the same clock). Port minus reference at the first write of tick 1 (examples): After Burner -12646, Altered Beast
+4975, Betrayal ingame +7450, Black Hornet -1227, California Games 2 -209, Count Duckula 2 +311, Frontier II -17766, ISS +8004,
Line of Fire -5..0, Midwinter 2 -3369, Soldier of Light +1953..+2619. `T - 21193` predicts these within 2 to 30 cc on the files checked
(SPEC section 1).

Reproduction of one case: measure_T on the Altered Beast trace gives T = 16247, burst 13435, first tick = A+16247+70940; then run
`intick.py` on that module's reference and port logs.

Subsong > 1: add `--subsong N` to the reference command; the traces then contain n song-init calls (SPEC 7.2).
