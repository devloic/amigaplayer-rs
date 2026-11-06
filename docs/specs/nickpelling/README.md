# Nick Pelling Packer spec: unknowns and caveats

Folder: SPEC.md (format and replay), LOGS.md (reference logs and what was checked), logs/ (reference write
logs), modules/ (the synthetic modules the logs were made from). My own reading notes and scripts are kept
separately in a private working folder (not for the implementer). No data/ folder: the format needs no tables from the
player beyond the standard ProTracker period table and the small tables written out in SPEC 2.

1. No real module. The module index has no file of this format; all 20 logged modules are synthetic. The
   file layout (SPEC 1) is what the reference's loader and replay accept; real files from the games may use
   only part of it (for example only literal runs, or a shared `SMP.set`). Real modules should be checked
   when one turns up.
2. Undefined cases (not modelled, avoid or handle defensively): a note or retrigger on a channel that has
   never had a sample (the reference reads a record at address 0); sample numbers 32..255; note periods
   above 856; break rows 64..99 (rows past the unpacked buffer); a pattern that unpacks to more than
   1024 bytes; arpeggio/glissando indexes beyond the areas listed in SPEC 2.1; an alias entry whose target
   has length 0 (the delta decoder would run 65536 times); sample names of exactly 22 characters with no
   terminating zero.
3. Song end: only a report to the host; the reference host stops a few ticks later. The time cost of the
   report itself (`songend`, about 75 cc) is a rough fit from a handful of cases.
4. Timing (SPEC 9) is a least-squares fit over about 262 000 write gaps; items that always occur together
   were folded, so individual item values are not the original's per-routine costs, only their path sums
   are meaningful. Raster waits: 10 of 413 checked first waits end one poll (12.5 cc) later than the model;
   a few ticks per log are off by one raster line for the same reason.
5. The slide mask (SPEC 7.1) starts at 0 when the player is loaded and is never reset by a song start. A
   host that restarts a song without reloading the player would see 0xFF from the second run on. The
   reference loads the player once per module, so the first slide of every run moves by 0 at its first
   tick.
6. E0x writes the CIA-A port (audio filter/LED bit). The reference also sets that bit once at song start.
   replay-cmp ignores these writes; a port that models the filter should apply them.
7. Host features not specified beyond SPEC 8: volume/balance scaling and voice muting (values only),
   position display, next/previous pattern buttons (they change the position and unpack, which a port may
   implement or not).
8. Detection uses only the song file; the reference fails at load when the sample set is missing. Ports
   should look for `SMP.<rest of name>` and then `SMP.set` in the same directory.

## Errata found by the implementation

- Tone portamento with no target does nothing (the glissando does not run and the period stays); the spec
  said to run it.
- Missing timing terms: an EDy with a note costs about 54.5 cc more than the table says; a tick-0 E8x without
  a note needs about 10.96 cc before the retrigger; the start-up unpack has a constant of about 228.75 cc
  that the formula omits.
- The reference's first-write time (29866..34937 cc) depends on the sample names and the path and is not
  modelled (the port uses 34913; the audio origin delay is 233).
- Delta decoding must read the evolving sample set, not the original.
- Safe choices where the spec is silent: sample numbers above 31 give an empty record, a pattern past the
  table is empty, break rows of 64 or more play as row 0.
- Timing is not exact: with the reference start time supplied about 78% of writes are exact and 99% are
  within 1 cc.

### Later timing refit

A second refit against fresh reference logs (write times 99.98% exact when the reference's own first-write time
is supplied; 98.4% with one fixed first-write constant of 35412 cc) corrects these points:

- Costs are exact multiples of 0.5 cc and the logged time is the floor of the half-cc clock.
- EC0 was missing about 12 cc; an E8 divide costs 77.0 when the divide hits and differs when it misses; the
  tick-0 E8 retrigger costs 14.5; a tick-0 EC with y=0 also pays the hit cost.
- The raster wait is a 9-poll loop over 454-cycle lines. The first wait reads the line register first, at +2
  cycles, and starts at 95; the second starts at 60; the first-channel constants are 62, 70.5, 79 and 87.5.
- The tick starts at the first write plus the first unpack cost plus 71779 (with a half-cc parity rule).
- A CIA write (0xBFE001) for a normal row effect is at +3.5 cc, not -28; the pattern-delay branch is -57 cc
  (-32 on channel 3; the channel 2 value was never observed and is assumed -57).
- The first write of the reference moves with the host's file loading (about 8.5 cc per path character and 10.5
  per suffix character, plus the sample-set name resolution) and cannot be predicted; modules in one run have
  first writes of 35412 or 35440.
- Audio is still SIMILAR on every module even with the exact first-write time; the cause is not found.
