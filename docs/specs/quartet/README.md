# Quartet (Amiga) - unknowns and caveats

Files: SPEC.md (format and behaviour), LOGS.md (log descriptions), logs/ (reference write logs plus the
synthetic modules and sets they were made from), data/ (one player-resident word, see below). A private
working folder exists for the analyst's notes; the implementer does not need it.

## Validation

A model written from the rules and numbers of SPEC.md was checked against 36 reference logs (6 hand-made
modules and 30 random ones, 48 712 writes): zero differences in address, value and colour-clock stamp,
first-tick clock included. Caveat: the model was written by the same analyst who wrote the spec; the
prose was not re-implemented by anyone else.

## Real modules

None found: the module index has no `qpa.` files (the matches for "qpa"/"quartet" are ProTracker MODs
and unrelated `smp.` files of other formats), and a header search of the archive for the detection rule
(byte 1 = 0x50, a valid tempo) found only two files of other formats, neither of which passes the end
mark check. Everything was verified on synthetic modules built from the format description; real Quartet
music (and real sets, whose header bytes other than `lock` at offset 4 are unknown) may use the format
in ways the synthetic files do not (e.g. typical set sizes, sample lengths <= 50).

## Player-resident data (lead decides on embedding)

- `data/note0-period.bin`: the 16-bit word the reference uses as the period of note value 0 (it reads
  past its period table). The editor probably never writes note 0; random modules with note 0 confirm
  the value.
- The 4-byte zero buffer for non-looping samples: content is zero, its address is the player's.
- The values for notes 13..15 (0x666C, 0x7279, 0x8088) are given in SPEC 4.3; they are bytes of the
  player's second (byte-sized) period table read as words.

## Unknowns

- The slide search can read beyond the end of the module file (a slide set-up within the last 36 rows
  before the end mark); what lies there is host memory. The model treated bytes past the file as 0
  and matched the one log where the search reached the end mark (the end-mark bytes count as a hit).
  A port should do the same; a different host memory content could change such a slide.
- Song end with length <= 2: SPEC 4.5 describes the reference's behaviour after the end (no restart,
  next check 256 ticks later) from the player's logic; the host stops before it is visible.
- The `SMP.set` fallback (no companion with the module's name) was not run; its extra start-up cost is
  not specified.
- Start-up timing (SPEC 5.5) depends on the host's directory string length and file name, because the
  reference builds the companion's path character by character. A port that wants register-exact clocks
  needs the same strings; otherwise only the beam phase of the line waits (and so the stamps of the
  writes after them) shifts, values and order do not change.
- The cycle numbers are the host's static per-instruction costs, not real 68000 timing.
- Host volume/balance/voice masks other than default were not exercised (the playback command line has
  no option for them); SPEC 2 gives the rule from the player's behaviour.
- Lengths > 0x7FFF words or start offsets pushing a sample past the set: not tested.
