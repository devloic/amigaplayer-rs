# Speedy A1 System - unknowns and caveats

Files: SPEC.md (format and behaviour), LOGS.md (log descriptions), logs/ (reference write logs plus the
synthetic modules they were made from). A private working folder exists for the analyst's notes; the
implementer does not need it. No data/ files were produced: everything the replayer needs besides the module
is described in SPEC.md (the only player-resident item is the address of the silent sample, see below; the
period table and envelope map are given as values in SPEC 4.3).

## Validation

A cycle-ledger model of the behaviour in SPEC.md (the numbers in section 6 and 8 were generated from the
model's tables) was checked against the host logs: the one real module (472896 write lines, the whole
song and 17 s of the restarted song), 12 synthetic modules in logs/ and about 100 more synthetic modules
(random rows, start positions other than 0, odd sample tables): zero differences in address, value and
colour-clock stamp. Start-up epochs (SPEC 8) match for all of them. Caveat: the model was written by the
same analyst who wrote the spec, from the numbers that became the spec text; the prose of SPEC.md was not
re-implemented by anyone else. The wait routine model (SPEC 7.4) was additionally checked in isolation on
3000 random entry phases.

## Corrections to the brief

* The brief said the A1 player "also searches for a 0x1190 marker". It does not: that is the Speedy System
  player. Speedy A1 System detects a module by the first word being zero, the long at offset 12 being
  0x200, an order list of 16-bit track numbers and the text `FORM` at a computed position (SPEC 1.1).
  The two formats cannot be confused.
* A real module exists: `digital voyage.sas` (extension form, so the prefix search finds none).

## Unknowns

* Only one real module was available, so conventions such as "table 2 = table 0 + 0x68" are taken from it
  and from the player's use of the numbers; the player itself accepts any values.
* Positions at or beyond 512 (an order list without 0xFFFF and an end position above 511): the player
  would read past the order list into the track data; not specified, not tested.
* Start-up cost when the order list has no 0xFFFF word within its 2048 entries: the scan constants of SPEC 8
  were fitted on lists that end with 0xFFFF; the no-terminator case differs by a small constant and is
  untested (the real module and all synthetic ones have a terminator).
* A note whose sample slot makes the player's own "empty slot" test succeed is skipped by a test that,
  after the load-time table relocation, can never succeed; treated as unreachable.
* Command bytes 0x53..0xFE are stored and inert (tested with randomised rows).
* Volume and timer wrap at 16 bits (SPEC 5.3); volumes that wrap after more than 8000 steps were not tested.
* Silent sample address: voices that are idle, and the park writes of a note start, point AUDxLC at a block
  of zero data inside the reference player (address 0x0068ACB0 in all logs, host layout). The value is not
  meaningful; a port needs 10 or more zero bytes. Comparing logs will show this one address as the only
  expected difference class.
* The cycle numbers are the host's static per-instruction costs; the real 68000 would differ (memory waits,
  DMA contention). A port aiming at the host logs matches them; a port aiming at hardware cannot rely on
  them.
* Host stop rule: the host stops one or two ticks after the song-end tick (it polls once per 4800 audio
  frames). This is host behaviour, not part of the format. After the end the player restarts at the start
  position.
* The module bytes of samples are never read by the player; sample data would be needed by a mixer, taken
  from the objects at TB + table-0 entry.
