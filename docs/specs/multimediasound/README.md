# MultiMedia Sound spec: notes and caveats

Files: SPEC.md (format and replay behaviour, timing), LOGS.md (reference logs), `logs/` (the logs),
`modules/` (the synthetic modules made for this spec), `data/` (player tables). The spec agent's


## Distinct replayer, shared file format

MultiMedia Sound plays SoundFX 2.0 files (`SO31`, 31 samples); its detection rule is the SoundFX 2.0
rule, so every `SO31` module is accepted by both players. UADE has no SoundFX 2.0 player and sends
the `sfx20` prefix (and `mms`) to MultiMedia Sound; the existing open replayer `SoundFX2.0` follows
the Eagleplayer SoundFX 2.0. Run on three corpus modules (CafeDeParis, ElefantPolka,
LeonardoSong), that port's write log matches none of the MultiMedia Sound reference ticks. What sets
MultiMedia Sound apart (SPEC sections 5 and 6):

- AUDxVOL is written for every voice on every row whose word is not negative (also empty slots),
  with commands 5/6/C applied once on the row;
- a speed command (F, with its speed-1 stall), a vibrato (4) and a set-volume (C) command; command 3
  does nothing; steps stop at their target and re-arm while the command lasts;
- `BRK` takes effect after the row; 0xFFFB/0xFFFC keep their command, `PIC` (0xFFFD) clears it,
  `STP` switches the voice on again at volume 0;
- subsongs separated by empty patterns (SPEC 3.3); two raster waits around the DMA-on write.

So it is a separate replayer, not an alias of the SoundFX 2.0 port; the module layout (SPEC 1) is
the same and a parser can be shared. Which player a host picks for `SO31` files is a policy question for the host (by prefix:
`sfx20`/`mms` -> MultiMedia Sound as UADE does).

## Modules

The archive has no file with the `mms` prefix. It has 89 `SO31` modules (all pass the check): 43
`SFX2.*` by Roman Werner and one by DJ Tekjam in the AMP collection, and 45 files in the
NostalgicPlayer test set (the same songs). The player itself carries a special case for one of them (Working In The
Mine), so these are the modules it was adapted for. Corpus modules never use commands 4, 5, C, F or
the words 0xFFFB/0xFFFD; the synthetic modules cover those.

## Paths never seen with real data / caveats

- Host volume gains below 64 and voices switched off (SPEC 5.4): taken from the player's structure,
  not exercised by any log (all logs use the defaults).
- The original's test for the filter value 0xFF also depends on state left from earlier work; in
  every reference run 0xFF meant filter off. Other values clear the byte in memory.
- Song length 0: the highest-pattern scan then walks 65 536 order bytes (beyond the module); not
  modelled, no module has it.
- The aborted subsong split (SPEC 3.3) leaves a subsong without an end; the reference keeps reading
  positions past the order list (memory after it). A port may stop such a subsong at position 128
  or at `N`; this changes only that synthetic case.
- During the speed-1 stall (SPEC 6.7) `timer` passes 0 on an effect tick; an arpeggio on that tick
  would take an undefined path in the original. Not reproduced, not exercised.
- The two raster waits make the DMA-off to DMA-on time and the loop writes depend on the tick's
  phase against the 227-cc line grid (SPEC 8.6). The logs are register-exact for the reference
  phase `E0`; a port with another tick phase gets the same values and order, shifted by up to a line.
- The 8 host volume writes and the filter write before the first tick are host behaviour; their
  timing is not specified.

## data/

`data/period_table.bin` (87 big-endian words: 20 pad words, the 66 table words of SPEC 2, the
terminator) and `data/vibrato_sine.bin` (32 bytes) are the player's own tables, byte for byte. SPEC 2
The port types both tables in as values.

## Errata found by the implementation

- SPEC 8.5: the arpeggio command needs 16 cycles on top of the generic 16 (32 after its write);
  pitch bend, vibrato, step set-up and speed match with the generic 16 only.
- The 19-cycle sign test applies only when the special word is not 0xFFFD.
- A subsong left without an end by an aborted split reproduces the reference only if the reader keeps
  going past the order list until the 16-bit position wraps (it reads guest memory past the module).
- Routing: the port claims a module only when the content rule holds and the file name has the prefix
  `mms.` or `sfx20.` (or that suffix); `SFX2.*` files stay with the SoundFX 2.0 replayer.
