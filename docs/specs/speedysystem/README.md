# Speedy System - unknowns and caveats

Files: SPEC.md (format and behaviour), LOGS.md (log descriptions), logs/
(reference write logs plus the synthetic modules they were made from). A
private working folder exists for the analyst's notes; the implementer does
not need it. No data/ files were produced: everything the replayer needs
besides the module is described in SPEC.md (the only player-resident item is
the silent-sample address, see below).

## Validation

A cycle-ledger model of the behaviour in SPEC.md (the tables in section 5 are
taken from it programmatically) was checked against the logs: 78487 merged
writes of the real module and 41 synthetic modules, zero differences in
address, value and colour-clock stamp. First-tick epochs (SPEC 6.1) match
for all 42. Caveat: the model was written by the same analyst who wrote the
spec, before it, not by an independent implementer; the prose of SPEC.md was
not re-implemented from scratch. The real-module log can only be reproduced
by someone who has that module (it is not redistributed).

## Covered / not covered

* Speedy System only. Speedy A1 System (prefix `sas.`) has a separate player
  that also looks for the same 0x1190 marker, but it was not analysed beyond
  noticing that, and the corpus holds no such module. It is not specified
  here; assume nothing about it.
* Corpus: exactly one real module matches the prefix (the `ss.` prefix has no
  collisions in the corpus). All other validation uses synthetic modules, so
  commands other than 0x50 / 0x51 and unusual sample shapes are only tested
  with synthetic data.

## Unknowns

* Host stop rule: the host stops one or two ticks after the song-end tick (it
  polls once per 4800 audio frames). This is host behaviour, not part of the
  format. After the song-end tick the player restarts at position 0.
* Silent sample address: sustain-phase voices (release period 0) point
  AUDxLC at a block of zero data inside the reference player (address
  0x00689A40 in all logs, host layout). The value is not meaningful; a port
  needs any zero data of 8 bytes or more. Comparing logs will show this
  one address as the only expected difference class.
* Behaviour for a sample index that is not in 0..0x30, for sample indices
  naming a negative (empty) slot, and for a marker at an offset other than
  0x14 is not tested (the analyst never produced such a case).
* Command bytes 0x52..0x8F are stored and inert as far as tested; they might
  have no effect in the player at all or be handled outside the paths
  exercised by the synthetic modules.
* If all 256 positions are identical (k = 256 in 6.1) the start-up scan runs
  off the table; start-up timing for that case is not specified.
* The cycle numbers are the host's static per-instruction costs; the real
  68000 would differ (memory waits, DMA contention). A port aiming at the
  host logs matches them; a port aiming at hardware cannot rely on them.
* Song length is given only by running the song (end row or position wrap);
  no length is stored in the file.
