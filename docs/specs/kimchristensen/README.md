# Kim Christensen: unknowns and caveats

- **No real modules.** The module index has no `KIM.*` module of this format. The three files with
  "kim" in the name that do exist are ProTracker/XM files, and the Rotor/Onslaught game files are a
  different packaging. All logs come from synthetic modules (`modules/`, pure data, no driver code;
  `modules/build_syn.py` builds them). Real modules may contain things the synthetic ones do not.
  For example, the bytes at `pos - 8` used as the empty sample are zero in the synthetic modules but
  may not be in real ones, and the module's own copy of the period table is ignored. The format
  rules come from the reference player itself, so they hold for any module it accepts. Real
  modules should be checked when one is available.
- **Layout search.** SPEC 1.2 (data) and 1.3 (detection) scan for the signature words slightly
  differently: detection does not skip the address longs. A module where an address long contains
  a signature word could pass detection and still be read differently. No synthetic module tests this.
- **Out-of-range period indices.** Note or pitch-envelope indices 43 to 54 read the driver's own state
  (SPEC 1.8, verified). Indices 55 to 255 (including a pitch offset that goes below index 0 and
  wraps) read memory past the driver in the reference host. The result depends on the host and is
  not specified. Modules that do this were removed from the log set.
- **Host volume, balance and voice muting** are ignored: the logs use the defaults (volume 64, all
  voices on). The VOL formula in SPEC 3.4 holds only for those defaults.
- **Timing constants** (SPEC 4) are this host's cost of the original's code paths, measured from logs
  and execution traces and checked to the colour clock. Where costs only appear as sums, the split
  between them is a convention. `E0` (the first tick's start) and the 70 940 cc tick spacing come from
  the host. The song-end cost (178 u) includes the host's song-end notification.
- **Song end.** Observed once per run: the offline render stops one tick after the signal. Later wraps
  are predicted (the driver keeps looping from `repeat`).
- **Restart** in the same process: the reference's re-init does not reset the tick counter or the
  DMA and loop masks (SPEC 2). Not logged.
- **Data embedding:** none needed. The only player table is the 43-entry period table, given as values
  in SPEC 1.8. No `data/` folder was produced.
- The spec agent's private working material (reading notes, scripts) is in a private working folder. Implementers
  must not open it.
