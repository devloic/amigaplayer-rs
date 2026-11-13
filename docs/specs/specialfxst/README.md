# Special FX ST (`doda.*`): notes for the implementer

- **No real modules.** The archive and the header index hold no file the player accepts (searched by prefix,
  composer and game names, and by the detection pattern over all indexed headers). Everything was checked
  on synthetic modules (LOGS.md). Real DODA files are Atari ST memory dumps; the table search (SPEC 2.1) is
  written for them, but the absolute-address variant (image base, step 10) was never exercised.
- **Not the YM-2149/MYST approach.** The module's 68000 code is never run: the player has its own replay and
  only reads the module's tables. An open replayer needs the replay (SPEC 3) and the small emulator (SPEC
  4); no 68000 core.
- **Validation.** A private model written from SPEC.md reproduces all 26 reference logs exactly (every
  value, order and colour clock): 10 directed modules and 16 random ones. The interrupt polls of SPEC 4.3
  were modelled with a re-implementation of the host Paula's channel state machine; an implementation in
  the repo can poll the host Paula itself.
- **Song end.** The host stops at the first song-end call, so the restart that follows a song end with no
  active voice (length above 10) was never timed. The cost of the host's song-end function differs between
  the two call sites (SPEC 6.1: 16 cycles after an even rounding inside the replay; the tick-entry value
  430 was measured on one run only and does not fit the same rule).
- **Emulation type 1 ("meynaf")** is selected only by the player's configuration file, which the host never
  provides; it is not specified. It uses a second pulse (bytes 0x70, 0x90 at `B + 0x4400`) and a 16 KB noise
  block after the 1 KB one.
- **Notes outside the period table** (bad data, or a pitch kick / transpose / arpeggio that leaves 0..101)
  read player memory. SPEC 3.1 describes what is read; the bytes are in `data/` (from the player binary).
  The lead decides whether to embed them; with sane music data they are never reached. They are code bytes
  of the player, so the port does not embed them: indices -128..-1 and 122..127 give period 0.
- **Periods** written by the replay are not range-checked: slides can wrap them, and the emulator masks to
  12 bits before converting.
- Private reading notes and scripts exist in a private working folder (not for the implementer).

## Errata found while implementing

- Reference player internal name is 'Special FX ST' (with spaces).
- Drum commands: the 'complete' cost of the 0x90+ entry does not include the duration, which costs another 45 plus 8, as after a note.
- Sequence end: the 'return +44' of the song-end test applies on every pass; the song-end path adds 68, rounds t down to even, then adds 16.
- Disable path: after a disable restart there is no '+10' and no extra tail; the +82 is entirely before the restart.
- Hold path: a voice that needs no restart costs nothing extra, so the +25 belongs to the common cost.
