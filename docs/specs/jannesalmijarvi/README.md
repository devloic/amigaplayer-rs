# Janne Salmijarvi Optimizer: unknowns and caveats

- No real module: the corpus has no file with the `JS92` magic (none among about 200 000 module
  headers, none named `JS.*`). The modules are original music-disk data ("A Taste Of U4IA",
  Megawatts 1992) that the corpus does not hold. All seven logs come from synthetic modules built
  from the format (`modules/`); they reach every path of the tick code, but real songs may use
  combinations not tried here.
- Timing is the reference host's, not a real 68000's. The host charges one static cycle figure per
  instruction: conditional branches cost the same taken or not, multiplications and the tempo
  division have a fixed cost, so no cost in SPEC section 8 depends on data values (only on the path).
  Calls into the host (song-end notification, timer change) cost 15 cycles each in the tables;
  values from 14 to 17 fit the logs, 15 is the one common to both calls.
- Absolute phase: the clock of the first interrupt (about 21 786 cc after the init call) and the
  pre-play writes (two host volume calls, init) are host properties; only times relative to each
  interrupt's entry are specified.
- Inputs outside the format are not specified: command byte above 15, sample number above 31,
  odd note bytes or note bytes above 72, finetune above 15 (they read outside tables or jump
  through unrelated data). A break row above 63 reads rows from the following patterns (specified
  as arithmetic, not logged). An invert loop on a voice that never had a sample changes memory
  near address 0 (not specified).
- Song end: every B command sends the song-end notification at once, and the host's offline render
  stops at the first one, so the reference never renders what follows a B command in offline
  mode. The player itself keeps going (wrap to position 0 or the jump target).
- The CIA writes of E0 (filter) and init are in the logs at 0xBFE001. Their values assume port A
  reads back what was last written (0x02 after init), which is what the host does.
- Host volume, balance and voice muting change the volume writes (SPEC 8.4); only the defaults
  (64, all voices on) were logged.
- `data/periods.bin` (576 16-bit big-endian values) is the period table as the reference player
  holds it. It equals the standard ProTracker table except one entry (finetune row 9, note index 23:
  230 instead of 238), so an implementation can also derive it from the repository's own ProTracker
  table with that one change; the lead decides whether to embed the file. Its location in the
  original player is recorded in the private notes.
- A private working folder holds the spec author's notes, scripts and the private model; implementers
  do not open it.
