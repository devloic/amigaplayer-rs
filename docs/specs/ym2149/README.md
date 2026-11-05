# YM-2149 / MYST spec: notes, unknowns, caveats

Files: `SPEC.md` (format, host interface, emulator), `LOGS.md` (reference logs), `logs/`, `mods/` (the
synthetic test modules). a private working folder holds the spec author's working notes and is not part of the spec.

## What an implementation needs

- A 68000 interpreter for the module's code, with the same static cycle costs as the host's CPU core, if
  the timing is to match (the emulator's timing in SPEC 5.5 assumes it). The module code is third-party
  data in the module file. No player code is needed.
- The emulator of SPEC 4-5, reading and writing the 0x158-byte block whose layout is the interface
  (modules write voice-record fields directly, and read the pointer fields).
- Calls to services 10-12 (SPEC 4.4) inside module code, routed to the implementation.
- Register contents at the module calls as in SPEC 2. At least one replay family (the Thalion sound-demo replays)
  needs a0/a1 at its relocation call.

No data has to be taken from the player: every table is given as values or as a formula. There is no
`data/` folder.

## Validation (private model written from this text)

- Synthetic modules, 11 of them, 200 ticks each: all 36 583 writes equal the reference in value, order
  and absolute colour clock, including the line waits. The mix buffers and the scratch buffer equal the
  reference memory after every tick of 119 ticks for the 5 mixing modules. The noise and envelope tables
  equal the reference memory.
- Real modules: 901 files in the MYST packs are type 0 or 2. In 19 of them the reference itself fails (an
  illegal instruction in the AmberStar data files and in two AntiChrist files), so they have no reference.
  The remaining 882 were run with the module code in a separate 68000 emulator (not the host's core) and
  the model doing the rest:
  - 793 of 830 compared: every write of 120 ticks identical in value and order;
  - 37 differ. In the two examined, the values the module wrote into the block (or its own memory)
    already differed. These are the Eternal and NoBuddiesLand digital replays, the Union/Cuddly Hippel
    modules, the Iso modules, the Richard Joseph modules and a few others. They are not traced to the
    emulator. The likely causes are self-modifying code, behaviour of that separate CPU emulator, or
    register state at calls;
  - 52 could not run in that CPU emulator (CPU exceptions in self-modifying replays; it runs in user mode).
  - 35 type-0 modules were re-run with the module code charged at the reference's own per-instruction
    costs (taken from its trace): 33 identical including every colour clock. In 2 (Tao) some writes are 24
    cc off, explained by module instructions whose cost depends on data (register shift counts), which that
    check could not reproduce.
- Not checked against the reference: the type-2 timing step (+8 cycles), subsong changes beyond the
  logged one, a voice mask other than 0xF, and volume/balance other than the defaults.

## Caveats and unknowns

- Noise indices: in tone + noise mixing the second half reads `T[i + o]` with `i` up to about 0x7FF plus
  the advance of one tick, and `o` up to 0x7FF. For high tone periods with low noise periods, reads of NL
  can run past its 4096 bytes, past the end of the reference player's memory image (into whatever the host
  allocated after it). The model reads 0 there. No corpus log showed a difference from this, but it was
  not checked byte for byte on real modules.
- SID level words with a level above 15 index past the 4096-byte scaled table (into the following tables
  in the reference). Not seen in the corpus.
- Division by zero (SID period 0 in tone + SID, and a few negative envelope periods): the reference aborts
  playback with a CPU exception. An implementation should not crash; what it does is free.
- The envelope one-shot shapes repeat (SPEC 4.2). This is the original's behaviour, not a modelling error.
- The 4th-voice copy also copies the restart flag, so channel 2 restarts together with channel 3 by default.
- Module code may write the ST sound-chip ports (0xFF8800/0xFF8802) and read 0xFF8800. What the host
  returns there was not studied. The modules that do this matched in the check above.
- The first tick's call time and the delay between the load call and the init writes (1 197 211 cc, first
  module only) belong to the host framework.
- The player's quadrascope bookkeeping (sample info for the Eagleplayer user program) has no effect on
  Paula and is not described.
- The `Y_` default subsong, `YN`, `YQ`, `YV`, `Ye`, `Ya`, `Yb` are not used by the Eagleplayer path.
