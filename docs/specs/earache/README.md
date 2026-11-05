# EarAche spec: what stays unknown / caveats

Files: SPEC.md (format and behaviour), LOGS.md (logs, E0 table, what was checked), logs/ (reference
write logs), modules/ (the synthetic modules the logs were made from). The spec agent's working notes
are in a private working folder (not for the implementer). No data/ folder: the only player table the format needs is
the 71-entry period table, given as values in SPEC 1.9 (a format-sized table; the lead decides whether
that is acceptable or should move to data/).

1. **No real modules.** The module index here has no EarAche file (`EA.*`, `MG.*`); everything was
   measured on synthetic modules built from the format. Real modules (EarAche editor output, and game
   rips that carry the branch header of SPEC 1.2) may use values the synthetic set did not: notes above
   71, volume tables starting with 0xFF, very long tables. Please re-check against real files if any
   turn up (the format check of SPEC 1.2 is the player's own).
2. **Period index above 71** (note-on with note > 71; arpeggio note + offset > 71) reads the player's
   own per-voice state and code, which hold absolute addresses of the reference's memory. Not
   specified; an implementer may return 0 or clamp and should log it. Index 71 (counter and speed) is
   specified and was tested.
3. **Volume table starting with 0xFF** (or containing only 0xFF and no value or stop byte before
   looping) makes the original loop forever inside the tick. Not specified.
4. **Host volume / balance and voice masking.** The reference scales every volume write by the host's
   master volume and balance (left for channels 0 and 3, right for 1 and 2: `(v & 0x7F) * level / 64`)
   and writes 0 for voices switched off in the host. At the default level 64 (all logs) this is the
   identity, which is what SPEC describes.
5. **Restart in the same process.** The reference's restart re-initialises only the global song
   state (SPEC 2); voice state carries over from the previous run. Only relevant for re-starting a
   song without reloading.
6. **Timing is the reference host's.** Tick period 70 940 cc, the start-up formula for E0 (SPEC 4.1:
   detection, initialisation, host calls in between) and every cycle constant were measured in this
   host's cycle-exact 68000 emulation. The split points between constants are conventions (SPEC 4.5).
   The song-end cost (SPEC 4.2, `145 * R`) includes the host's song-end callback.
7. **Song end and stopping.** The host stops its render 0 to 4 ticks after the first song-end report;
   the player itself never stops. A song whose order list has no end marker never reports song end.
8. **Silence buffer address** on a cut (SPEC 3.4) is the player's own 4-byte chip buffer
   (0x007A03C0 in the logs), not part of the module; any address of 4 zero bytes in chip memory gives
   the same sound.
9. **Detection words.** SPEC 1.2 gives the five 16-bit words that the branch header kind must contain
   at the branch target; they are given as values only, with no statement of what they encode.
10. **Name prefixes.** UADE associates the prefixes `EA.` and `MG.`; the player's own info names `EA.`.
    Detection is by content only.
11. **Independent validation.** See LOGS.md for the checks made with a model written from this text.

- `data/branch_header_key.bin` (12 bytes): the branch-opcode word followed by the five-word key sequence used by branch-header detection (code words of the module's own header). They are kept out of SPEC.md because they are instruction words; the lead decides on embedding.
