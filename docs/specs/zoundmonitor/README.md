# ZoundMonitor: unknowns and caveats

Files: SPEC.md (format and behaviour), LOGS.md (reference logs and what they confirm), `logs/`
(reference write logs), `modules/` (the synthetic test modules and their generator). The spec
author's reading notes are kept in a private folder that implementers do not open. No `data/`
folder: the format needs no data from the player beyond the 37-value period table given in SPEC 3.1.

- **No real modules.** The archive has none (LOGS.md). All validation uses synthetic modules built
  from the format description; the format details were confirmed by the reference accepting and
  playing them, but real editor files may contain trailer contents or slot values not seen here.
- The detection rule (SPEC 1.5) was exercised with both trailer forms (`df0:` and `Xamp`); its
  behaviour on non-ZoundMonitor files that happen to match is not studied (the rule is weak: any
  file whose computed end offset holds `df?:` or `?amp`).
- Out-of-range note indices: indices 37 to 124 read the player's state (SPEC 3.1). The logs cover
  37, 38, 43, 44, 45 to 48 and 66; the other state words follow from the same layout but are not
  logged. Indices of 125 and above, instrument numbers of 16 or more after the offset, pattern
  numbers above P and positions above T read memory outside the song data: undefined, not modelled.
- Slot 16 is loaded but its address is never set; using it plays from the raw file value.
- IFF samples: only the fixed offsets 100 (size) and 104 (data) are used; an IFF file with a
  different chunk layout plays garbage in the reference too. Tested with one 8SVX file built to
  that layout.
- Song loop: the logs end shortly after the song-end signal; the wrap to the start position is
  logged only for `fx` (start 1). `end <= start` is described from the arithmetic only.
- Timing: verified exactly (every write's colour clock) relative to the player entry clock `E` of
  each tick. The entry clock of the first tick and the sample addresses are host properties. The
  raster wait (SPEC 5.4) depends on the absolute clock (line grid of 227 cc), so a port must use the
  same absolute `E` to reproduce it exactly.
- Host volume, balance and voice muting change the AUDxVOL values (SPEC 6); all logs use defaults.
- The host's duration is computed from the end position only (SPEC 1.7) and is wrong when the start
  position is not 0.
