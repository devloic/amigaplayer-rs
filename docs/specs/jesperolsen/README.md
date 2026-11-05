# What stays unknown / caveats

1. Kalas_Puffs_Expressen jo.title (engine C-T family) was not compared; it uses the same
   player code as Guldkorn jo.title, so SPEC section 5 should apply, but it is untested.
2. Guldkorn jo.ingame subsong 14: the alignment of the first ticks could not be established
   (the reference's start ticks differ from the other subsongs); nothing was compared for it.
3. Several cost entries were never exercised by any available file (they were taken from
   the static cycle sums of the sections, not verified against a write): the jump-marker takes in
   the T/G lists, the T option "instrument map", the G tempo/transpose option takes, and the
   B script "stop" branches in some combinations. Cost sums come from static cycle counting,
   so they are expected right, but no log checked them.
4. Start phase: the first-write time of C-T (8408 cc from reset) and the model's tick-0 offset
   (18 cc) are not explained; tick-relative times do not depend on it. Tick-to-tick spacing
   (about 70 900 cc, three-tick jitter pattern) is host timing, not specified here.
5. Song end: after all voices become inactive the reference keeps ticking (closing DMA-on
   write each tick, DMA-off write on counter == 1 ticks). The port ends the song instead, so
   subsongs 0-8 of Guldkorn jo.ingame stop after 3-4 s where the reference plays on.
   Whether the reference's later ticks write anything else after the song end (it does not in
   subsong 8) is verified only for that file.
6. The closing DMA-on write's host clock jump (2045-2270 cc) varies; the cause (host stub's
   own cost depending on state) is not modelled; it falls between ticks, not inside one.
7. Notes of index 96-127 give period 64*(index-95) in the reference (B); the port panics.
8. The B data-module "start" timing (time between start and first tick) uses a fitted
   formula in the repo (start_return); not re-derived here.
