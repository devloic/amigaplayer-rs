# Jochen Hippel ST - caveats

Private notes, scripts and the validation model exist in a private working folder (not for the implementer).

Validation: a private model written from the reference behaviour reproduces the exact write VALUES and ORDER
of the reference for both real modules (about 500 ticks each), ~400 synthetic COSO modules, ~80 raw+prefix
modules, digital/period tests and multi-subsong tests. Timing is NOT register-exact: section 11 gives
measured per-write gaps and medians only; the per-tick "entry to first write" cost and the wait for a
running Paula channel's audio flag (depends on Paula's period counter phase) are not modelled.

Unknowns / caveats:
* No real Atari ST (hst./sdc.) modules were available; only two Amiga-family modules plus synthetic ones.
* Pitch command $E7 (new type) reloads the program from the image start in the reference because host low
  memory is zero; host-dependent quirk.
* Header reads on raw modules with digital samples under $E7 depend on memory layout; unmatchable.
* Volume indices above 17 read player code bytes (data/volume-table-overread.bin, indices 0..51); indices
  past ~51 unknown. The 52-byte volume table is embedded in the port (project owner decision)..
* Two-file (LSMP) modules: write-compare of my model with the reference failed at tick 2 (model lacks
  correct sample-pointer handling for the companion); values for that path are unverified.
* Raw-pattern conversion (section 3) is described behaviourally; the interpreter may read raw patterns directly.
* The reference host stops at song end.
* Prefix flag patterns (timer/period detection) are only partly described.
