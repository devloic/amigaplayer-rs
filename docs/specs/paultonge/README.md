# Paul Tonge (prefix `pat`): spec

Files: `SPEC.md` (format, replay behaviour, timing), `LOGS.md` (reference logs, how they were made, what was
checked), `logs/` (reference write logs and call tables for nine songs), `modules/` (the songs and both sample
banks of one real set), `data/` (two tables of the game's driver, see below). The spec agent's private reading notes
are kept in a private working folder that the implementer does not open.

## Status
- Written from one complete real set (8 in-game tunes and a title tune). No synthetic modules.
- A private model built from SPEC.md reproduces every reference write (register, value, order) and every write time
  and call-return time to the colour clock, for all nine songs up to where the host stopped (PAT.TUNE8 only up to
  tick 864 of 1154, see below). Numbers in LOGS.md.

## data/ (the lead decides on embedding)
The format plays through the game's own driver, which is a separate file of the set (`WantedTeam.bin`); the
songs reference two of its tables:
- `data/volumes.bin` (34 bytes): the 15-step volume curve plus the two words the driver reads past its end
  (index 15 and 16 both occur in normal play).
- `data/periods.bin` (256 bytes): the 36-entry period table plus the 92 words the driver reads past its end when a
  note index exceeds 35 (indices up to 53 occur in the set). Entries 52 and 120..122 hold the value the reference
  reads after the wrapper's load-time patch; entries 49..51 and 117..119 would be an address inside the
  host's wrapper and are not reproducible (unpatched driver bytes are stored; no song uses them).
The driver file itself is not needed by a port and is not included.

## Unknowns and caveats
- Timing: all costs in SPEC 6 come from the reference CPU's per-instruction costs. Paths that no song executes are
  marked "estimated" (same cost rules, not checked against a log): glide, slide-up steps, rate counting of slides,
  most of the unused commands, the chord bytes, fade steps.
- The audio-interrupt race (SPEC 5.11) depends on Paula's DMA fetch timing in the host; a port only gets it right
  if its tick-start writes are exact (they are given exactly).
- PAT.TUNE8: one voice's position list has no terminator; after its only pattern it plays data from memory beyond
  the song (in the reference: from the sample bank, which the host placed 0x810 bytes after the song) and at tick
  865 hits a byte that sends the driver into its own code. A port cannot follow that; it should stop the voice or
  the song there (decision for the implementer/lead). Up to tick 864 it is exact.
- Song end: the host stops 2 to 3 ticks after the song-end report. After command 0x8F (or a fade to 0) the song
  restarts on the next tick (the wrapper turns the driver's stop into a restart).
- Not exercised by any log: commands 0x81, 0x82, 0x86..0x8B, 0x8E, 0x90, 0x91, 0x96, fade to 0, voices with
  offset 0 in other positions than voice 1. Commands 0x98 (a tune built into the driver) and 0x9A..0x9F are not
  specified.
- Sample addresses in the logs are absolute (bank base differs per run: 0x4DE0, 0x4DF0, 0x51E8); compare modulo
  the base or place the bank at the same address.
- The reference's volume scaling by the host's volume/balance and voice mask is in SPEC 5.2; with default settings
  it is the identity except for the out-of-range index 15 value.

## Errata found while implementing

- The first-tick phase differs per module in the reference (98306 for six songs; 98269, 98346 and 97900 for others) and cannot be derived from the files.
- The sample bank lies 0x800, 0x810 or 0xC08 bytes after the song start depending on song size (fitted).

### First-call time (derived later by black-box runs on the reference)

The time of the first call of the play routine is not a constant of the song; it follows from the host's loading
and from three properties of the song data. All values are colour clocks, measured live at one path:

- 25.5 per character of the directory path (final slash included) and 10.5 per character of the file name.
- If a sample bank `SMP.<suffix>` of the song's own exists beside it, the first call is 703 earlier at a 60-character
  directory, and the directory slope is 17 per character instead of 25.5 (the spec's -406 for the title is the same
  effect at the reference's shorter directory, about 25 characters).
- A header list offset that is 0 skips that list's check: -37 for list 1, -45 for list 2 and for list 3
  (-72, -81, -89, -111 for the combinations of two and three lists).
- A present list whose first entry points at a pattern preceded by the byte 0x8F instead of 0x80 costs 27 cycles more
  (13, 27, 40 for one, two and three such lists).

Nothing else tried changes it: file size, list entry values, pattern bytes, bank size, inserted list entries.
Untested: file names that do not start with `PAT.`, and zero offsets combined with first entries preceded by other
bytes.
