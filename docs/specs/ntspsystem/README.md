# NTSP-system: unknowns and caveats

- **No real module.** The module corpus has no `SPNT` file (the one `two.*` file there is a different
  format and is refused by the player's check). Everything was verified with synthetic modules made to
  the format of SPEC 1: random block data (this covers every code path of the decoder, as any bytes decode)
  and one encoded signal. The format has no structure beyond the header, so real modules differ only in
  length and statistics; the stream-end and tail cases were exercised with many lengths.
- **Absolute phase.** The 51 749 cc start constant and the interrupt entry times belong to the host
  framework. Only times relative to the interrupt entry (and, at start-up, relative to the first LC write)
  are modelled. The first two entry times follow the raster-line grid of the host (SPEC 5); an
  implementation on the same host receives the entries from its Paula instead of computing them.
- **Shift cost.** The reference host charges a shift the same for every shift count; a real 68000 charges
  two cycles per position, which would make a block with shift 7 about 30 cc slower. The spec follows
  the reference, which is what the register logs show.
- **s = 0 cost** (+2 cc per block) is empirical.
- **Jitter.** Chunk costs vary by 0 or 2 cc between otherwise equal runs (chip bus contention in the
  reference); the model is within 2 cc of every write.
- **Not covered / unspecified.** `N` below 1800 (the decoder reads beyond the module; undefined); a
  module shorter than `8 + floor(5N/9)` bytes (the decoder reads beyond the file; the bytes it sees depend on
  the host's memory); behaviour of the stop sequence (SPEC 7, taken from the player's description, never
  executed in a log); the first-entry rule of SPEC 5 is a fit to a 38-point sweep (sw modules in LOGS.md), one
  of them an outlier.
- **Output stage.** Four channels carry identical data at volumes 64, 64, 1, 1; the left/right Paula
  pairing (0 and 3 left, 1 and 2 right) makes both sides the same signal at 65/64 gain. Whether to emulate
  Paula's 8-bit DMA path or play the stream directly is the lead's choice; only the register writes and DMA
  contents are specified.
- **No data/ folder.** The format needs no data from the player: no tables, no samples. Nothing was put in
  `data/`.
- `modules/` holds the synthetic modules used for the logs (generated, not taken from a player).
  `tools/replay-cmp.py` compares logs; a private working folder with the reading notes and the model exists
  next to this file.
