# Major Tom (ARP.) - unknowns and caveats

Files: SPEC.md (format and behaviour), LOGS.md (log descriptions), logs/ (reference write logs),
modules/ (the synthetic modules of the synthetic logs), data/ (player-resident bytes, see below). A private
working folder exists for the analyst's notes; the implementer does not need it.

## Validation

A model written from the rules and numbers of SPEC.md was checked against 34 reference logs (9 real
modules, complete songs; 5 hand-made and 20 random synthetic modules): zero differences in address,
value, order and colour-clock stamp, start-up writes and first-tick clock included. tools/replay-cmp.py
confirms on the model logs. Caveat: the model was written by the same analyst who wrote the spec; the
prose has not been re-implemented by anyone else.

## Real modules

Nine `*.arp` files (demozoo), all with the same header layout. A header search of the corpus for the
magic found exactly these nine; the UADE prefixes `hn`, `thn`, `mtp2` match no file (the archive's "Major
Tom" directory holds ProTracker modules by an artist of that name, a different format).

## Player-resident data (lead decides on embedding)

- `data/noise.bin` (3000 bytes): the noise block played for waveform value 0xFF (SPEC 4.7, 6). Needed by
  every real module.
- `data/period-overrun.bin` (30 bytes = 15 words): what the reference reads for period-table indices
  89..103 (SPEC 4.8). These are not bytes of the Major Tom player: in the reference host the table is
  followed by the host's allocation padding and the start of the next player it loaded (another
  Eagleplayer). Only king_of_games uses them (indices 89, 93, 96, 103; 96 and 103 give 0x4E75 and 0x2456,
  the others 0), in 390 period writes. Not embedding them changes those writes only. On a real Amiga or
  in UADE the values would differ.
- Index 88 (0x4246, the last word of the player's code) and index 180 (0x0076, host memory, the high
  word of an address) are given as values in SPEC 4.8. Index 180 matters at every voice's first note when
  that note uses porta (the porta starts from 0x0076) and for the idle-voice writes every 256 ticks.

## Unknowns

- Period-table indices 104..390 except 180: host memory, unknown (SPEC says 0). No known module reaches
  them except through note bytes below 0x10 combined with arpeggios.
- Waveform-jump reads (SPEC 4.7) outside the module read host memory; every observed one read 0 (real
  modules land 1.0-2.4 MB beyond the module). A port should read 0.
- Sequence positions that run past 63 after a jump to 63 or more read further module bytes; past the
  module end the reference reads host memory (SPEC: 0).
- Pattern numbers 32..63 address memory before the patterns (signed 16-bit offset); 64 and up wrap. Not
  used by known files; not logged.
- Speed 0 (via effect F with parameter 0, or speed word 0) gives a 65536-tick row; not logged.
- The host's voice-volume scaling (SPEC 4.12) was only exercised with the default 64.
- The cycle numbers are the host's static per-instruction costs, not real 68000 timing.
- The player also offers next/previous-position functions to the host (both reset the row offset; next:
  position + 1, at L back to R with the song-end notification as in SPEC 4.5; previous: position - 1,
  from 0 to L - 1). The host never calls them, so they are not logged.
