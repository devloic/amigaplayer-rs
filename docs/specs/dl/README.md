# README: Dave Lowe `dl` start-up spec - what is solved, what remains unknown

Files: `SPEC.md` (behaviour spec), `LOGS.md` (commands). The `logs/` data files and the instruction-level traces are not shipped.

## Solved (validated on the corpus)
* The reference's start offset `T` of every UNCLEART file is a formula over the module file (scan length of the volume-write search, subsong count,
  order-list lengths, four special cases) plus two measured per-build / per-module constants. 45 of 45 exact, held-out 12 of 12 exact.
* The first tick is exactly 70940 cc after the start-up work returns for all 47 files; ticks are exactly 70940 apart. There is no timer-programming
  effect; the earlier "+-150 cc" is the post-reset tail of the module's init (2032 to 2983 cc, set by the driver build).
* The port's constant `DL_INIT_CLOCKS` should become `T(file) - 100` (verified indirectly on 5 files against the existing port logs).
* The write order of a voice and the Eagleplayer's per-write intercept costs (build-independent) are described and measured.

## Unknown / not done
* `M` (module-init cycles, 13 classes) is not derived from header fields; the classes are listed. A module outside the 45 needs a new measurement
  (or default 5664). Two classes (IK+, ISS) have several loops; ISS/IK+/Frontier II have no reset burst.
* `X` (sample fix-up of Beneath x2, F-15, Raffles) is known as four numbers; the per-record costs do not follow from the record count.
* The terms `midwinter` (395), `timescanner` (37), `no table` (-290), `bra` for Beneath (20) are each backed by one or two files.
* The 380-byte scan start and the two instruction forms the scan looks for were identified from the corpus; a module whose volume write is of
  neither form would make the real player run on to the end of the module (not seen).
* In-tick costs: only the invariant intercept costs and per-file idle-tick shapes are provided. The state-dependent variation inside a file (for
  example the volume-slot gap taking 3 to 6 different values from tick to tick), the cost of each event type per build, and the tick-entry-to-first-write
  gap `s0` (438 to 718 cc by build) as a function of module data are **not** modelled. The existing `DL_GAPS` fit is Line of Fire only.
* The `older_*` pair: structure measured; the two-frame absolute shift of their chain and its relation to refcmp alignment not checked.
* Write-sequence differences between port and reference (0 to 34 of the first 100 ticks per file) are a functional matter, outside this spec.
* The per-subsong chain (SPEC 7.2) is the engine host's way of selecting a subsong; whether the real Amiga does the same is not claimed.
