# Infogrames spec: overview and caveats

Contents: SPEC.md (format and replay behaviour, timing model), LOGS.md (reference logs, how they were
made and checked), logs/ (xz-compressed reference write logs), modules/ (constructed test files and
the synthetic instrument bank, with the scripts that build them), data/ (two small tables from the
player). The spec agent's private reading notes are in a private working folder, which an implementer must not open.

## Player data (lead decides on embedding)

- `data/periods.bin`: the 104-entry period table (208 bytes, big-endian words). Required: notes index
  it directly and its values are the player's own (not a standard ProTracker table).
- `data/tempo_table.bin`: the 12-entry list of known songs (checksum, length, CIA timer value), 144
  bytes. Only matters for the 12 song files it names (none of them was available here; it was
  checked with a constructed file); every other file plays at 0x1A00. Leaving it out would play those
  12 songs at 33 285 cc instead of 47 360 cc (or 35 075 cc for the 0x1B66 entry) per tick.

## Unknowns and caveats

- **No real instrument bank.** All reference logs use a synthetic `.ins`. The layout in SPEC 1.2 is
  what the player reads; the first 4 bytes and bytes 8..11 of each entry are not read and their
  meaning is unknown. Sample data format (8-bit signed) is assumed from Paula, not checked on real
  data. The audio of the logs is therefore not the games' sound; only the register behaviour is.
- **Bank names.** The archive's banks for `rh2.*.dum` are called `rh.ins`, which the reference's
  name rule (SPEC 1.4) does not find. A port could add a fallback (e.g. the prefix before the first
  dot), but that would differ from the reference; the lead decides.
- **Subsongs.** The reference always plays the last song of the table and exposes one subsong. All
  real files have one song; a port may expose all N songs, but subsong N − 1 is the one to compare.
- **Song end.** The reference never signals an end; voices fall silent one by one at their track end
  and the player keeps writing VOL 0 / PER 0. Three files (bivouac, rh2.blood, rh2.bobopat3) are
  still playing at 120 s; it is not known whether they end at all.
- **Undefined cases** (never in real files, not reproduced): event bytes 0xE4..0xFE whose low three
  bits are 4..7; a period index ≥ 104 after transposition; a note before any envelope of that kind
  was selected. The player's volume fade, track-loop option and sound-effect voices are reachable
  only from game code, not from the host; SPEC describes playback as the host drives it.
- **Timing.** The cycle model reproduces every write time of all logs exactly, including the DMA
  waits; the absolute time of the first tick depends on the host (one tick period after the song
  start, whose cost grows with the file length, SPEC 6.1).
- The checksum list was found by reading the player; that 11 of its entries share 0x24FF and one has
  0x1B66 is as stored, with no known reason.
