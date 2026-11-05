# TFHD (single-file TFMX): spec notes

This folder holds the behaviour spec for the three TFHD Eagleplayers (TFMX-1.5-TFHD,
TFMX-Pro-TFHD, TFMX-7V-TFHD), written so that `src/replay/tfmx.rs` can be extended rather than
re-ported. SPEC.md has the format and the behaviour, LOGS.md the reference logs. A private
folder of reading notes and scripts exists beside this one; the implementer does not need it.

No player data (tables, samples) is needed: the container holds everything. There is no
`data/` folder.

## Results

* 7V: the TFHD player is the player the `Seven` engine already follows. Only the container is
  new.
* 1.5: same engine as `V15`. It differs in where DMA on is written, in its start writes, and
  in its spacing (faster). A model from SPEC 4.2 is register- and order-exact on 9 of 9
  modules.
* Pro: same engine as `Pro`. It differs in master volume and song-start fades, the track mute
  table, the song-end rules, the subsong list, detection, no 9-line waits, its timer and
  start writes. The model from SPEC 5 is register- and order-exact on 14 of 14
  modules. One of them matches without the volume rescale, and pro10 was compared over only
  4 + 194 ticks (SPEC 7).
* Real modules: only two TFHD files exist in the corpus (one 1.5, one 7V). Everything else was
  checked on synthetic TFHD files made from real pairs; no real TFHD Pro file was seen.

## Unknowns and caveats

1. **Timing was not fitted path by path.** SPEC 6 gives per-transition medians, the 2 222 cc
   tick-start shift for 1.5, and the missing 9-line waits for Pro. The exact per-path costs, to
   reach colour-clock equality like `COST15` and `COST` do today, still have to be fitted
   against the logs (the same least-squares method the repo used).
2. **Pro start-up times** (SPEC 5.6) vary by up to ~1 600 cc between runs before the timer
   start, because the host's load path differs. The first tick lands 79 037 cc after the timer
   start every time.
3. **Pro tempo changes**: no test module changed the CIA timer through a track step during
   playback. That case is assumed to behave like `Pro` (timer value $1B51F8 / value) but was not
   seen in a log. The 60 Hz flag of the module header (bit 1 of byte $B, which `Pro` honours)
   was not tested.
4. **Pro song end on loops** (SPEC 5.4) was confirmed on one modified module only. The rule
   comes from reading the player and that one test.
5. **Pro detection quirk**: for fixed-table modules, the 7-voice test reads the step at
   16 × start from the start of the module, not from $800 (SPEC 2). Stated from reading, not
   exercised by a module that would be misclassified.
6. **Pro also takes 1.5 modules**: a "TFMX " module, bare or as type-0 TFHD, passes
   TFMX-Pro-TFHD's check and plays (wrongly) on the Pro engine. A host should give such files
   to the 1.5 player first.
7. The 7V log comparison TFHD vs pair shows a constant shift of up to 160 cc in the start phase
   (SPEC 3), not a difference in the engine.
8. Side note for the lead, not about TFHD: on the 6 synthetic 7V pairs and the real one, the repo's `Seven`
   engine is not tick-equal to its own reference (e.g. Turrican 2 title: it misses the first
   tick's volumes and periods). This was seen while checking. It is a `Seven` port issue,
   unrelated to the container.
