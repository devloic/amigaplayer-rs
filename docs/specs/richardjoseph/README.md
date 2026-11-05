# Richard Joseph (`RJ.*`): unknowns and caveats

- **No real module.** The archive has no `RJ.*` container (files with an `rj` prefix there are
  `rjp.*`/`smp.*` pairs of the separate Richard Joseph Player format, which fail the detection of
  SPEC 1.2). All test files in `modules/` are synthetic containers built from DeliTracker custom
  rips of Cadaver and Speedball 2 (LOGS.md). They contain the games' driver code and music data
  (third-party material); the lead decides whether they may be shipped or must be rebuilt
  locally from the rips.
- **The start routine (InitSong) is module code.** Real `RJ.*` files made by Wanted Team carry
  their own small start routine; the three used here are a game routine (Cadaver title), the
  driver's song-table routine (Cadaver) and the game's request routine (Speedball 2). SPEC 4
  describes only these three shapes. A port must recognise the start routine of each file it
  supports.
- **Layout parameters** (SPEC 2) were found by reading the two drivers. The prose recipe for
  locating them in another module of the family is untested (no third clean driver: a James Pond
  2 rip has a modified driver and was not used).
- Paths never executed in the reference runs: the Speedball 2 priority rejection (a request with
  a lower priority than a playing voice), the S looped instrument with a non-zero loop offset at a
  trigger (covered on C only), tempo command with operand 0 (divide by zero), 0x98 with a zero
  count, invalid command bytes (not multiples of 4 above 0x7F, or above 0xD4), the start-routine
  scan finding a zero byte for voice 0 (all scans of the C table file clear voices 1..3 only).
- Host behaviour, not format behaviour: the song-end reaction (stop, or next subsong for an
  empty subsong) and the clock of the restart's EndSound (SPEC 7, LOGS.md); the start-up
  timeline of SPEC 8.2 is that of the reference host and adapter.
- The Audio emulation's empty loop points the channel at the image start (SPEC 5.4), whose first
  long the adapter has cleared; the location value is therefore `base` itself. Log values of all
  location registers depend on `base` (0x46A30 / 0x33DE0 in the reference runs).
- Volume and balance settings other than the defaults (gain 64) are not covered.
- Paula DMA output timing (what the hardware plays) is the host's; only register writes are
  specified.
- No data is needed from the player: every table is in the module. No `data/` folder.
- The private notes, disassembly and scripts used to write this spec are kept in a private working folder and
  are not part of the deliverable.
