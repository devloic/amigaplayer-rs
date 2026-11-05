# Medley: unknowns and caveats

- **No real module.** The archive has none: the four Medley modules known to the UADE metadata
  (Paul van der Valk, 1990-1991) are not among the indexed files, no file name uses the `mso.`
  prefix or the `.mso` suffix, and no indexed file starts with `MSOB`. Everything was checked with
  synthetic modules made from SPEC section 1 (LOGS.md). Real modules may use the format in ways
  the synthetic ones do not (for example selector values other than 1 and 2, notes beyond the
  period table, or object headers that are not names); such cases are marked undefined in SPEC.
- The player is a 2001 adaptation for UADE ("V0.1"). Behaviour of the original game drivers may
  differ; this spec describes the UADE player only.
- The song's tempo word (offset 0x18) is stored by the player (clamped to at least 0x1000) but
  never applied: the tick rate is the host's fixed 50 Hz. A port that wants the original speed
  would have to guess; the reference does not use it.
- The player has an interface for a second song table and a song-restore mode, and per-voice
  "locked" channels (for game sound effects); the host never uses them, the replay paths they
  enable are not specified (a locked voice would skip all its register writes).
- Undefined inputs: selector bytes other than 0, 1, 2 (SPEC 2.6.1); waveform octave bytes above 7;
  period index above 153 (above 109 it reads the detune table, SPEC 2.4.1); a random list count of
  0 (SPEC 2.7); command 4 in a normal track jumps relative to the sequence table start, not to the
  entry (a quirk, reproduced with specially placed data in `mso.rand`).
- Pre-tick timing (load and subsong-start DMACON writes: groups of four about 43 cc apart, voice
  resets about 5 000-5 100 cc apart) and the tick-0 phase are host properties; only tick-relative
  timing is specified and verified.
- Host volume, balance and voice-mute settings are not used by this player (it never reads them).
- `data/` holds two blocks of the player that the replay needs: `periods.bin` (220 bytes, the
  110-entry period table) and `detune.bin` (88 bytes, the detune patterns). They are the player's
  bytes; the lead decides whether to embed them or derive them (the period table is a rounded
  equal-tempered table, the detune patterns are six short lists given in structure in SPEC 4.3).
- a private working folder holds the spec author's notes and scripts and is not part of the spec.
