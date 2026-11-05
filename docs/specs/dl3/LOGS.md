# LOGS (round 3, Dave Lowe EP side)

None of the logs, scripts or traces named here is shipped. Regenerate reference logs as described in `docs/specs/README.md`.

Reference: the real Eagleplayer run by the 68k engine (EAGLEPLAYERS pointing at the UADE players dir), 4 s logs of the
47 Dave Lowe corpus modules. All runs: SDL_AUDIO_DRIVER=dummy RUST_LOG=error, WAV to /dev/null.
Candidate runs use `DELI_DL_CPU=1 --open-replayers` with an empty player dir (so no Eagleplayer is loaded).

## Scripts
- runall.sh BIN OUTDIR : runs all 47 modules with BIN, compares with reference (cmpall.sh -> OUTDIR/cmp.txt, prints totals).
- runmine.sh / runref.sh : single-file runs. cmp.py : write-log comparison (seq-equal / time-equal counts). cmpdirs.sh, cmpall.sh : batch compare.
- subtest.sh BIN K OUTROOT : `--subsong K` ref vs mine for all non-older files, prints mismatching files.
- resid.py : residual histogram of timing differences.

## Log directories (logs/)
| dir | content |
|---|---|
| base/, base.txt | baseline repo model, 4 s: 32665/56399 time-equal |
| m1 m2 m4 m5 | intermediate models (patch-site costs added incrementally) |
| m10/cmp.txt | final model 4 s, 45/45 exact (only 2 older_* differ) |
| ref30/, m5_30/, m6_30/, m5_30.txt, m6_30.txt | 30 s reference and candidates; m6_30: 432811 writes, seq 432811, time exact on 45 files |
| refsub/, sub1/ sub2/ sub3/ (ref, mine, cmp.txt) | subsong steps 1..3 |
| sites/dl.*.txt | per-file patch sites (`site rule offset words`) and execution counts per rule over 4 s (`counts [R0..R12]`) |
| flow/ | traces: ts*.txt (Time Scanner), ab_s1.txt (After Burner), mcdump.txt, sites_*.txt |

## Reproduce
```
scripts/runall.sh <bin> logs/<dir>        # 4 s comparison, prints "writes N seq-equal N time-equal N"
scripts/subtest.sh <bin> 2 logs/subX      # subsong step 2
```
The implementer builds their own candidate binary and uses the same scripts. Per-file counts in
logs/sites/*.txt show which rules each module exercises (R6 only SF2,
R2 only Time Scanner, R0/R7 never).
