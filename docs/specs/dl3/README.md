# README: unknowns and caveats (round 3)

1. F-15 Strike Eagle 2 at `--subsong 2+` (step 1 result: 1810/2041 sequence,
   75 time-equal) and Midwinter 2 at step 2 (1501/1800 seq, 730 time):
   the module sequence itself diverges, not only timing. F-15: re-running the
   module's header fix-up per subsong helps (1897/2041 seq) but values still
   differ. Midwinter 2: suspect an EP "repair" step that redirects several
   module table pointers to EP-internal tables; untested, and the table
   contents are not reproduced in this spec. Subsong 1 is exact for both.
2. Betrayal ingame: after subsong switching one write is 1 cc off (667/666,
   929/928). Cause not found (likely a rounding in the stop-call or the 118 cc
   calls for that module's particular stop routine).
3. Rules never exercised by the corpus: R0, R7, and the "unknown channel"
   (ch4) path of the period/location/length stubs. The costs listed for
   those are extrapolated from the shadow routines and not validated.
4. Master volume/balance other than the defaults (0x40 / mask 0xffff) are
   untested; the value formula in SPEC 2 is from the stub structure.
5. Registers/flags beyond d0/d7/a1 and SR are not modelled for the stubs
   (R3's flags differ from the original; not observable in the corpus).
6. The older_* files (2) use a different, smaller player; the CPU path
   reproduces about 0.3% of their writes. Out of scope.
7. R9's "+32 on ch0" is derived from the module shape and execution counts
   (21 builds); it was not checked whether any module contains the shape off
   the ch0 path in a way that would be channel-dependent.
8. The 4 s/30 s exactness was reached with FIXUP_EXTRA = 0 and no
   timer-based extras; if a later change reintroduces any "first-step extra"
   constant, it should be 0.
