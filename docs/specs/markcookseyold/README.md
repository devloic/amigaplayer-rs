# Mark Cooksey old player (`mco`): caveats and unknowns

- Data: the player's period table (40 words) is given as values in SPEC 3.3. The 16 words in front of
  it, which modules made for a different table actually read (Overlander), are code words of the player.
  They are in `data/period_below.bin` (32 bytes) and nowhere else. The lead decides whether a port may
  embed them. Without them, Overlander subsongs 0 and 1 differ in a few period writes (values 0x4E75 etc.).
  The port does not embed them (they are instructions): an index before the table reads entry 0.
- Real modules: none with the `mco` prefix exists in the archive. The four distinct `mcs.*` data files of
  the newer format's engine O are this format exactly and were used renamed. The two synthetic
  modules cover the commands the real files never use (0x04 period, 0x10 rest, 0x34 loop to mark).
- Undefined in the original, not specified: period indices past the end of the table (they read the
  live state of voice 0), odd period offsets, invalid command bytes, stack overflow beyond 40 bytes,
  mixed stack entry types, a note before any instrument command (stale sample pointer), a subsong
  offset of 0.
- Initial volume words 0x14, 0x14, 0x1C, 0x30 come from the player image and persist across subsong
  changes. A port that restarts a subsong should keep each voice's last volume word and sample record.
- Host-specific values in the logs: the module load address (log header), the silence-buffer address
  0x00759F70, the first tick entry (71 761 cc) and the 621 cc per next-song call. The implementer's
  framework supplies its own equivalents.
- The host shows the subsong range as 0..0 because the player reports none. A port should report
  (L - 8)/16 subsongs (SPEC 2.3).
- Volumes are written raw (4 × byte). Values of 0x80 and above, or the period word written by command
  0x04, are passed to Paula unchanged, as in the reference.
- a private working folder holds the spec author's notes, disassembly and scripts. The implementer does not open it.

## Errata found while implementing

- SPEC 5.3 (line wait): the 'add 9' is also taken after the sample that differs. Each spin step is 16, sample, 9, then the test; the other constants (lead-ins 46 and 56, 10 per change, 44 at the end) then fit all write times.
- LOGS: the syn1 reference log stops at about tick 1000, not 60 s.
- Undefined in the spec, clamped by the port: period indices before the table (entry 0) and past its end (last entry).
