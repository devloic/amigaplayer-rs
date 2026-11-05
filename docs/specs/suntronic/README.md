# SUN-Tronic: unknowns and caveats

- **Same as Tronic?** No. `Tronic` is a data-file format with its own detection rule; SUN-Tronic modules are
  executable driver images. They are separate formats (SPEC, introduction).
- **Corpus:** three files in the archive have the player's file-name prefix `sun` and look like this format
  (first word 0x48E7FFFE): `shades.sun` and `msx-cracktro 2.sun` (both demozoo) play; `pseudo oops.sun` has a third
  driver revision (word at 8 = 0x4A2E07C0) that the player rejects ("unknown module format"). The other `sun.*`
  and `*.sun` files of the archive are XM, ProTracker or other formats; the entry `Sun.mod` is a NostalgicPlayer
  test module. Only variant A (`shades.sun`) and variant B (`msx-cracktro 2.sun`) are specified.
- **Synthetic modules** (`modules/`): the real modules are third-party files and are not shipped. Nine synthetic
  modules were built by overwriting data of the real ones in place (random envelopes, LFO tables, morph tables,
  waves, streams, transpositions; for variant A also random instrument types, instrument count 5). **They contain the
  driver code of the real modules** (that code is part of every module of the format), so the lead decides whether
  they may be published. The generator is not shipped.
- Sample instruments (stream notes 0x01..0x3F) cannot be used in a valid module (their record table overlaps the
  sequence list); not specified.
- Stream command bytes 0x80..0xB7 that are not in the table are skipped; command 0x9C and the like with operands
  running past the stream are undefined (SPEC 7.6). Reads outside the module (period index above 71, wave
  indices, morph values above the wave length, a type-3 divisor of zero) diverge: the model stops at the
  first such read. Variant B synthetic runs b001 and b003 reached one at ticks 1 272 and 1 434, b002 at tick 0.
- The end-of-song marker 0xFFFFFFFF in the sequence list (A) is only partly specified: the master volume is set
  to 1 and the host is notified, but the entry that follows the marker is then read as pointers (undefined);
  the cost of the notification at the marker is not measured (the wrap's 482 cycles are).
- A one-colour-clock deviation in the write that follows the song-end notification in 2 of 7 wrap ticks
  (SPEC 8.4); undetermined.
- The player's second initialisation for variant B (zero first transposition byte) uses other start values
  and a different voice record size; not reproduced, no module uses it.
- Variant B has no song-end notification (the hook is only installed for variant A): a host that needs a song end
  has to count wraps itself; the reference run does not stop.
- Absolute clock of the first call and tick phase (SPEC 8.1) are host properties.
- No data from the player is needed (the period table, arpeggio table and instruments are in the module); no
  `data/` folder was produced.
- The step costs of Appendix T were measured with the host's per-instruction accounting (execution histograms of
  the reference) and summed per step; every step was verified through the stamps of the writes after it.
  The model that checks the SPEC was built next to the text and its step numbers are these; it was not
  re-implemented from the final text by a second person.
- Folder a private working folder holds the reading notes, disassembly and model scripts; it is not part of the published
  material.
