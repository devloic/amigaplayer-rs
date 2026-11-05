# Tomy Tracker: unknowns and caveats

- No real module: the archive has no `sg.*` file (nor any other file that passes the detection
  rule; all module headers were scanned). Everything is verified on synthetic modules only, so the
  file format is what the reference player reads, not something checked against the original
  tracker's files. Real files might use the low bits of the command byte, instrument bytes that are
  not multiples of 7, or other layouts the player would misread (SPEC 8).
- The porta set-up on a note row loads the porta speed from the high byte of the channel volume
  (SPEC 4.2). In every log that byte is 0, so "high byte of the volume" and "0" cannot be told apart
  from the logs.
- Host volume, balance and voice muting (SPEC 6) are described but not logged (defaults only).
- The filter command's CIA writes are logged as the host emulates the port (reads back the last
  written value, starting at 0); a real CIA-A port reads other input bits.
- Only the first song end of a run and the 1 to 4 ticks the host renders after it are observed;
  later loops are predicted by the same rules.
- Absolute timing (SPEC 7.5) depends on the host's start-up call order and stub costs (16 cycles
  per host call); only the reference host was observed. Tick-relative timing does not depend on
  this except through the beam phase, which the waits of SPEC 7.3 follow.
- Channel state is not cleared when a song is started again in the same process (SPEC 2).
- The reference's own reading notes, disassembly and scripts are in a private working folder; the implementer
  does not need them.
- No `data/` folder: the only tables the player holds (period table, vibrato sine, the 0/1/2
  arpeggio phase sequence) are small and given as values in SPEC.md.
