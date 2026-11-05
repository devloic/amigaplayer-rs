# GlueMon spec: unknowns and caveats

Folder contents: SPEC.md (format and replay behaviour), LOGS.md (logs and how they were made), logs/
(reference write logs and the module generator), modules/ (synthetic test modules), data/ (player-side
data, below). The spec author's private working notes are in a separate folder that implementers do not
open.

1. **No real modules.** The corpus has no GlueMon module (no file with the `GLUE` magic). Everything was
   validated on 5 synthetic modules built from the format description. The format allows things real
   modules may or may not use; the behaviour on the synthetic modules is exact.
2. **data/drum_wave.bin** (32 bytes) is player data, not module data: the sample that drum kind 0xC8 plays
   (SPEC 1.5). Its content follows a simple rule (described in SPEC 1.5), so it could also be generated
   in code. The lead decides whether to embed the file or generate it.
3. **Unsupported inputs** (the original reads unrelated memory; never exercised): pattern numbers 64 and
   above, note bytes 98..253 (other than 0xFE) on melodic voices, order walks that pass the end of the
   file without an 0xFF, `rows = 0`, `speed = 0`, `N < 2`.
4. **Volume factors.** The reference host sets master volume and both balance levels to 64, so the volume
   factor is 64. The formula for other levels is given (SPEC 3.7) but was not exercised.
5. **Period 0.** Voices run with DMA on from init without a period until their first note, and drum mode
   writes AUD3PER = 0 until the first drum kind; what Paula does with period 0 is the emulator's business
   (the reference logs only show the writes).
6. **Noise timing.** The reference makes the new noise values visible at the tick call; a real 68000 would
   store them during the first ~400 cc of the tick. Matching the reference means replacing the buffer at
   the call (SPEC 3.1).
7. **Host stop rule** (SPEC 4.3) is a property of the reference host, not of the format.
8. The info value "number of patterns" (SPEC 1.4) and the stop entry (SPEC 4.4) were derived from
   the original player, not observed in a log (the host does not display or call them in these
   runs).
