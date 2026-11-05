# Anders 0land: caveats

- Scope: SPEC.md (format, detection, replay, write order, values, timing), LOGS.md (log format and runs),
  `logs/` (14 reference write logs), `modules/` (the 14 modules the logs were made from).
- No real module exists in the archive for this player (prefix `hot.`), so all 14 modules are synthetic,
  written by a private generator that covers: all four voice masks used, tempos 0..10, ties, slides,
  transposes, alternate instruments, command words, envelope and vibrato steps, fade, stop and restart
  markers, and every note index 0..69.
- Validation: an independent model was written only from SPEC.md and run against all 14 logs. Every write
  matches in value, order and colour clock (floored), 98 502 writes over 10 506 ticks. A broader set of 84
  internal runs (not shipped) matched in 83; the failure was an out-of-limit pattern count.
- Limits: behaviour is only specified inside the index ranges SPEC.md states (instrument, envelope, sample,
  pattern and command numbers, note index 0..69). Outside them the original's results depend on stale register
  contents and are undefined here.
- Not determined: behaviour on a real module; the host's volume, balance and voice-mute paths (their writes and
  costs were not exercised); the driver's fade-start entry (subsong number with bit 15 set); note indices above
  69; the fourth byte of each chunk tag is assumed to be the digit 1 (the check compares only three bytes).
- The driver code in the first chunk is not executed by the reference player, which uses its own replay copy.
- `data/` was not produced: the only table (70-entry period table) is given as values in SPEC.md, so no
  embedding decision is needed.
- A private working folder with scripts and notes exists beside these files; implementers should not open it.
