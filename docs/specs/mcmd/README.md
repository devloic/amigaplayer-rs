# MCMD spec: notes and caveats

- Format: Eagleplayer "MCMD" (V1.0, 15 Feb 93, "Testversion by Eagleeye of DEFECT"), UADE prefix
  `mcmd_org.`. Files named `mcmd.*` in collections belong to the Jochen Hippel player (a different
  layout; that player rejects these modules). Feasible: yes; the spec is complete for everything the
  player does with a module.
- No real module was available; all validation is on synthetic modules (LOGS.md). Real modules may use
  combinations not generated here, but every command and every path of the play routine except the
  voice-mask volume path was exercised and matched exactly (values and colour clocks).
- Player-resident data needed by a port, given as values in SPEC.md (no data/ folder was produced): the
  73-entry period table (section 8; index 73..127 reads 0 in the reference), the 8-byte default sequence
  (section 4) and the start values of note and loop length for voices 0 and 2 (section 4), which the
  player never initialises and which come from its file. The lead decides whether these are embedded as
  given.
- Song end: the reference reads the position entry used at the wrap from host memory (SPEC 10.3). Values
  in the song-end tick and after are host-dependent and not reproducible; ports should end the song at
  the wrap like the reference host. A stop command (0x8n control byte) silences the song forever without
  a song-end report.
- Unsupported / undefined: effect and volume commands 0xE9..0xFF (the reference jumps into unrelated code),
  sequences that loop without a value or wait (the reference hangs), period indices past the table other
  than as zero, section lengths >= 32 768 bytes (wrap quirk given in SPEC 1), odd pattern sizes.
- Timing: section 11 is an exact cycle model (static per-instruction costs, floor(cycles/2), beam line from
  the clock). The tick-call phase (73 889 + 70 940 k) is the reference host's, not the player's.
- My reading notes, disassembly and scripts are kept in a private folder the implementer does not use.
