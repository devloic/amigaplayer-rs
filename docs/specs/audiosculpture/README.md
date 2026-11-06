# AudioSculpture: unknowns and caveats

- No real companion file. The corpus has the five modules but none of their `.as`/`.nt` companions,
  and the reference does not play a module without one. All logs therefore use synthetic companions
  (`modules/`, AM records with chosen parameters, all user-code slots empty). The real modules'
  note/command data and sample instruments are real; their AM sound is not the original's. If the
  real companions turn up, rerun `logs/runref.sh` with them: any slot without the 0xABCD marker
  means 68000 code (below).
- User-code slots (command E with a non-empty slot) run 68000 code from the companion on the row
  tick and on the row's later ticks. Muppet-Synthetical uses E60, E61 and E92, so its real companion
  may well contain code. Not specified, not portable; a port should treat a non-empty slot like an
  empty one and say so (output then differs from the original for that module). The code is given a
  table of four services (voice DMA off with a short busy wait, voice DMA on with a busy wait, note
  index to period, period to note index); details unknown beyond that.
- Data that lives in the player: the 180-word AM vibrato table is not a clean formula; it is in
  `data/am_vibrato.bin` (360 bytes, big-endian signed words). The lead decides whether a port embeds
  it. The other tables (period table, command-4 sine, three waveforms) are given in SPEC section 5.
- Noise waveform: generated from the beam position, so a port needs its colour-clock timing right
  (SPEC 9.4) to reproduce it. Against the DMA log 4704 of 4706 fetched words equal the model; 2
  fetches stamped about 17 cc before the model's store of the byte they already contain are
  unexplained (a fetch-versus-store race inside the generator loop; shifting the store instant does
  not change them).
- Arpeggio past the period table (SPEC 7.1) writes absolute sample addresses as periods for
  `x` large enough; it happens on channels that never played a note. Reproducible only when the port
  uses the reference's module load address convention for addresses.
- Instrument numbers above 31 (possible in the cell encoding), a waveform number above 3, order
  entries 0x80..0xFF and octave shifts of 16..63 are specified from the arithmetic only or marked
  unsupported; none occurs in the logs.
- Absolute phase: the clock of the first init write is a host property; the first tick entry follows
  from it (SPEC 9.1). Cycle costs are the reference machine's static per-instruction costs (they
  do not depend on branch direction or operand values).
- The subsong rule (SPEC 1.5) has a quirk that collapses the count to 1 when a later B repeats the
  first B's argument, including when the first B's pattern is simply played twice. This is the
  player's (UADE adaptation) behaviour and is kept.
- Song end: the visited table is never cleared, so after the first song end every row tick on an
  already visited row notifies (and pays the 278 cycles) again. The reference render stops a few
  ticks after the first one; later notifications are predicted, not observed.
- Detection collides with StarTrekker modules tagged `FLT4` that have an empty sample header with a
  non-zero volume (SPEC 1.2); the companion requirement keeps them apart in practice.
- The spec author's working notes and scripts are kept in a private working folder, which implementers do not open.

## Errata found by the implementation

- The raster wait adds its 9 cycles on every loop iteration, including the exiting one.
- All four row cells are fetched from the position at the start of the row.
- The gap between a subsong stop and the init is 35 colour clocks.
- With the open replayers, an FLT4 module that has an AudioSculpture companion beside it is taken by this
  replayer, not by the built-in StarTrekker handling.
