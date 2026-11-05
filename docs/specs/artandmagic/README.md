# Art & Magic: caveats and unknowns

Spec for the sound driver embedded in the game "Agony" modules (`aam.*`), four real modules
(level_1_and_6, level_2, level_3_and_5, level_4) plus synthetic variants for coverage. See
SPEC.md; logs are described in LOGS.md. Private analysis notes, scripts and disassembly exist in a
separate folder that the implementer must not open (a private working folder).

Result: a private model written from SPEC.md matches the reference logs line for line (values,
order, relative and absolute colour clocks) for all four real modules and about 34 synthetic ones.

Caveats and unknowns:

- Fade-out machinery in the driver is inert here (nothing enables it); it is not specified.
- Table indices outside the real tables (notes outside 0..95, pattern or instrument numbers
  beyond the data) are undefined; the modules never do this.
- The reference host returns 0 on CIA reads; real hardware would give other values for the
  filter-pulse writes (no real module uses them).
- The absolute start phase I (first log line) depends on the host's start-up and on the module's
  copy time; only I-relative times are specified.
- Song-end detection in the reference is per 0.1 s chunk, so logs run 0..3 ticks after the tick
  at which the loop is executed.
- The wave-table overlay over the first sample bytes (SPEC 2.7) must be reproduced for
  bit-exact sample data, even if it looks like a bug.
- Rests, slides, vibrato, arpeggio, decay, voice end, filter pulse and the version B colour write
  are covered by synthetic logs only; the timing recipe in SPEC 11 is fitted/verified there.
- Cycle costs assume the host's static per-instruction model; a more accurate CPU timing would
  differ.
- No data/ folder was produced: all needed data (tables, streams, samples) is in the module
  files. The lead decides on any embedding.
