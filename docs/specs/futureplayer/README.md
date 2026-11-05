# Future Player: unknowns and caveats

A private working folder with the author's working notes and scripts exists next to this file; it is not part of the specification and is not needed to implement it.

Files: `SPEC.md` (format and replay behaviour), `LOGS.md` (reference logs, host sequences, first-tick table),
`logs/` (register-write logs of the reference player), `data/periods.bin` (the 97-entry period table as
big-endian words: it lives in the player, not in the modules; the lead decides on embedding it, or on
generating it from the values printed in SPEC.md section 1.5, which are the same numbers), `mods/` (the five
modules the logs were made from: two real tunes from the public corpus and three synthetic ones; the lead
decides on shipping them).

## Unknowns and caveats

- **Absolute phase.** The clock of the first tick (`K0`) is a host property. LOGS.md gives the measured
  values and a rule that predicts them within about 15 cc. Because of the raster wait (SPEC 6.10) the
  clocks of all writes after the first wait in a tick depend on `K0 mod 227`; the values and order of writes
  do not depend on it.
- **Period index above 96.** The note index is reduced to 7 bits and the table has 97 entries; indices 97 to
  127 would read whatever follows the table in the player (host address bytes). Not specified; no module
  of the corpus uses them.
- **Command numbers above 14** have no defined behaviour (the dispatch table has 15 entries); not specified.
- **Silent buffer address.** The address written for the one-shot dummy and for empty lists is a location
  in the player's own chip memory; only its high word (0x0079 in these logs) is observed.
- **Song-end reaction.** SPEC section 7 describes the event and its cost. What the host does afterwards
  (restart, stop, next song) is host behaviour; the logs end at the end of the tick in which the event fires.
- **Pre-run corner cases.** A song that ends during pre-run produces no tick (covered by `syn_ctrl_s5`, which
  checks only the two voice-end writes and their gap).
- **Fade-out.** The player contains a fade-out path in the tick that no module can reach (the fade state is
  never set negative); it is not specified. A voice state between running and ended (a per-voice countdown
  path) is likewise unreachable.
- **Host controls.** Mute switches and volume/balance of the host are ignored; logs use defaults (all voices
  on, full volume).
- **Paths run only by the synthetic modules** (the two real modules never reach them; they are confirmed
  on synthetic logs only): the 255-command abort, command 5 and command 10, waveform lists, the
  modulator negate and stop-flag branches, the address-modulator delay and stop branches. Everything else
  is also exercised by the real modules.
- **Quirks kept on purpose** (all visible in logs): command 1 applies the detune bytes to all four voices;
  the first arpeggio entry plays twice at a wrap; address modulator 3 spends its whole delay in one tick;
  command 10 never falls through.
- **Validation level.** The private model that produced the zero-difference results of LOGS.md was written
  from the player's behaviour in parallel with this text, and SPEC.md was then reviewed against it
  (costs in section 8.3 are generated from the model's tables). It was not rebuilt from the finished
  SPEC.md text alone; an implementer following SPEC.md should treat the first run against the logs as the
  real test.
- **Timing model.** The cycle numbers are the host machine's static per-instruction costs; they reproduce
  the logs exactly but do not model bus contention or Paula DMA timing.
- **Detection.** The detection rule (SPEC 1.2) accepts both corpus modules; other Future Player files are
  assumed to share the same header words.
