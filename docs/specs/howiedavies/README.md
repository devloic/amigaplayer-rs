# Howie Davies - caveats and unknowns

- Modules used: the three real `hd.` modules in the archive (Blazing Thunder,
  Future Bike Simulator, Yogi's Great Escape), 12 subsongs in total. No
  synthetic modules; the archive has no other `hd.` file, so a fourth module of
  the family is untested.
- Validation: a private native model written only from SPEC.md (table
  location, rules and the cycle constants) reproduced the reference write
  logs without a single difference in address, value, order **or colour
  clock** for all 12 subsongs over the full 120 s reference runs (or until the
  host stopped at the song end). The model also reproduces the first
  InitSound call clock of all three modules from the start-up scan formula
  (SPEC 11.1) and the subsong-change sequence. The cycle constants in SPEC 10
  were first measured on the reference player's machine code and then checked
  by this model, so the model validates the numbers but is not an independent
  discovery of the logic.
- Variant A (Blazing Thunder) has no song end within 120 s, so its song-end
  extra cost (83 cycles) is assumed equal to variant C's and is untested.
  Variant B's is 84, variant C's 83 (both measured).
- The cost model needs the odd cycle counts of the host's instruction table
  (some branches cost 9 or 11); the constants are given in cycles and must be
  accumulated before halving.
- Per-channel hook costs (SPEC 10.3) were measured in isolation and are listed
  per channel; the origin of the channel-dependent differences is the wrapper's
  channel dispatch.
- Not determined: the use of the four plain numbers at header offsets
  0x2C..0x38 (not needed); exactly when the host stops after a song end
  (0 to 4 ticks in the logs); behaviour of unseen order or pattern command
  numbers (not present in the modules).
- Stale state: variant B keeps the previous song's initial volume and
  envelope start across subsong changes (SPEC 5); all known songs start with an
  instrument command so it has no effect.
- The header pointers to the module's own code are used only by the start-up
  scan; an implementer needs no 68000 interpreter, the whole replay is
  specified natively.
- No data/ files were produced: everything needed (periods, DMA bits, samples,
  patterns) is inside the module. The lead decides on any embedding.
- Private analysis notes, scripts and the disassembly exist in a separate
  folder that the implementer must not open.
