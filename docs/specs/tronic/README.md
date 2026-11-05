# Tronic: unknowns and caveats

- Corpus: only two real modules exist, `crystal.dp` and `locomotion.dp` (suffix `.dp`, third-party
  files from the demozoo archive). No file whose name starts with one of the player's configured prefixes
  (`dp`, `trc`, `tro`, `tronic`, `tron`) is a Tronic module: the corpus entries `tron.mod`, `tronic.mod`,
  `MOD.tronic`, `STK.tron`, `MED.DP`, `MOD.*.dp` are other formats (none passes the detection of
  SPEC 1.2). A scan of the header index (first 2 KiB of every module; files were read in full where the rule needed
  more, `.sfx`/`SFX.*` files skipped) with the SPEC 1.2 rule found exactly these two.
- Detection collision: with the full UADE player set the host picks "Soundtracker IV" for both real modules
  (that player's check accepts them), so an automatic player search must try this format first or by
  content; played by the Tronic player they sound and log as described in SPEC.
- The 16-bit addition in the detection (SPEC 1.2) is done by the original on the low half of the absolute
  address; module-relative offsets are used here. Only differs at 64 KiB boundaries of the load address.
- Everything beyond the two modules rests on 190 synthetic modules (random, structurally valid, built from
  the SPEC); the generator is not shipped. They exercise every branch of SPEC 4 and 5 and the timing of
  SPEC 8 but not "musical" content.
- Undefined behaviour (SPEC 7.6): period index above 85, pointers outside the module, effects 9 to 0x80, an
  arpeggio list starting with 0x80 (loops forever in the original), `L_v = 0`. Not reproducible, not
  specified. Seven of the 190 synthetic modules reach such a case and diverge from there.
- The scratch value of SPEC 7.3 (high byte of effect operands, upper half for effect 8) starts each tick at 0.
  That is the host's register value in every reference run; a different host could differ. The
  song-end call is assumed to leave the scratch value untouched.
- Sound stop (SPEC 6.3) was read from the player but never executed by the reference runs (no sign of it in the logs);
  not verified.
- Host volume and balance are ignored: all logs use the default gain (identity, SPEC 5.8). The host's volume
  routine, called twice before the sound start, writes the stored volumes (0 at that time); its exact
  spacing (156, 168, 162 cc) is host/player init behaviour.
- The absolute clock of the first tick (cc 73 949) and the tick spacing (70 940) are host properties.
  Initial writes (SPEC 6.2) are given relative to the sound start; the delay between the sound start
  and the first tick is host business.
- Song end: the notification is given once per voice and wrap. How a host reacts (the reference host stops
  after 1 to 4 ticks) is not part of the format. No position/length information exists in the module.
- Sample memory: the first word of every sample is overwritten with zero the first time its loop is set
  (SPEC 7.2). The register logs do not show this; a sample-accurate replayer needs it, and the DMA
  log was not compared.
- Timing is exact for the paths in the logs; the cycle costs of SPEC 8 were measured as per-instruction
  histograms of the reference and summed per step (all steps verified through the stamps of the writes that
  follow them). The costs after the last write of a tick (`T_CLOSE`) are never visible in the logs.
- Data that lives in the player: the period table (SPEC 1.6, given as values) and the 46-byte default
  instrument (`data/tronic_default_instrument.bin`, described in SPEC 1.6 in numbers; the file is for
  the lead to decide on embedding; the period table is also in `data/tronic_periods.bin`).
- Folder a private working folder holds the reading notes, disassembly and model scripts; it is not part of the
  published material.
