# MIDI - Loriciel: unknowns and caveats

- What the reference outputs: plain Paula writes. The song is a Standard MIDI File (format 0 or 1)
  and the samples come from a separate bank file (`SMPL.<name>` or `<name>.BSP`, magic `BNKS`); the
  player itself holds no instrument data, only a 60-entry period table and a velocity-to-volume curve,
  both given in SPEC.md. No `data/` folder is needed.
- No real module: the archive has no song/bank pair of this format (LOGS.md). Everything is verified
  on synthetic modules only, so the bank layout is what the reference player reads, not something
  checked against Loriciel's own files. Real banks might use the unused region bytes, header length
  bits above 16, region counts of 0, or positive (pre-converted) references; their effect is
  described in SPEC.md where it follows from the player, not observed.
- MIDI coverage is narrow by design of the original: no running status, no controllers, pitch bend,
  aftertouch or loops; note length is the sample length (one-shot samples, the voice then loops on a
  4-byte silence block); only the most recent voice of a channel can be released. Files written by
  ordinary sequencers (running status, 0xD0 channel pressure) desynchronise as SPEC section 4 says.
- The host's volume, balance and voice muting (SPEC 4.5) are described from the player's own volume
  path but not logged (defaults only).
- The host must be given the module with an absolute path; with a relative path it cannot load the
  bank. The start-up timing (SPEC 7.11) depends on the directory and file-name lengths the host
  passes and on the host's call order and service-call cost (16 cycles each), and the start call makes
  a cache-flush service call because the reference host reports a CPU above the 68000. Only the
  reference host was observed; the tick-relative timing does not depend on this except through the
  beam phase, which the waits of SPEC 7.3 follow.
- The second-file load with a lower-case `midi.` prefix or a `.mid` suffix takes slightly different
  (unmeasured) costs; only the upper-case `MIDI.` spelling is in the logs.
- Out-of-range lookups (notes beyond the period table, program numbers past the bank's list,
  channel-prefix values above 15, a note above every region's upper key) read unrelated player or
  bank memory in the reference; a port can treat them as undefined.
- The reference's own reading notes, disassembly and scripts are in a private working folder; the implementer
  does not need them.
