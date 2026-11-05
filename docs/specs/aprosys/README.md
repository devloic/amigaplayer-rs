# AProSys spec: notes, caveats, unknowns

- Format: AProSys (Petter A. Urkedal), DeliTracker/UADE player "AProSys" (registered name and file
  name are the same), file prefix `aps.`, slug `aprosys`. The module is pure data (no embedded
  driver), so a real replayer port is possible.
- **No real module exists in the available corpus.** All logs come from synthetic modules built
  from the format (`modules/`). Real modules may use field combinations the synthetic ones do not
  (see "Unknowns").
- No player data is needed beyond the 48-entry period table, which SPEC.md gives as values; there
  is no `data/` folder.
- a private working folder holds the spec author's reading notes and scripts; the implementer does not open it.

## For the implementer

- **Audio interrupts.** Sample chains (types 0, 1, 2, 13) are played by a per-channel audio
  interrupt handler (SPEC §10). Implement it as the engine's per-channel interrupt hook (as the
  Thomas Hermann and Voodoo Supreme Synthesizer ports do) and make the trigger writes through the
  bus exactly as the original: INTENA enable for the channel, then AUDxDAT = 0, after the voice's
  DMACON off. The engine's Paula then raises the interrupt (or not, if the channel's DMA has not
  yet stopped), and later raises the follow-up interrupts when the channel reloads its pointer.
  In the reference runs the channel is still in manual mode when the handler turns DMA on, so the
  chained sample only starts after the channel has played out its previous internal pointer and
  length; this is Paula behaviour and comes from the engine, not from the port. Serve interrupts
  requested during a tick after the tick, lowest channel first.
- **Synthesis buffers** must be in guest chip memory (Paula plays them): one 1024-byte area for
  the eight buffers (the comb stage writes one byte into the following buffer, SPEC §8.4) and a
  2-byte zero "silence" buffer. Buffer addresses are compared by a constant offset.
- **Song end**: the original never signals it. Report the end at the tick of SPEC §9; the
  reference keeps writing PER/VOL/LOC every tick afterwards, which a port can reproduce for the
  255 ticks before the undefined part.
- **Integer details matter**: 16-bit wrap of envelope values and filter states, the division
  rule of SPEC §1 (overflow keeps the dividend's low word; it happens in practice in the filter
  stage with bit 23), byte arithmetic in the synthesis operations, and the low-word use of
  products noted in each formula.
- **Hangs in the original**: a comb stage with `n` not a multiple of 4 never ends; a sequence
  loop entry with count 0 loops forever. A port should guard against both (they cannot occur in
  a module that plays).
- **Filter command** (sequence `0xC0`): bit 1 of CIA-A port A (LED/filter).
- **Timing**: LOGS.md gives measured gaps. The synthesis cost depends on mode, operation,
  length and stages; `tools/replay-cmp.py` compares values and order per tick, and DMA off-to-on
  gaps only roughly.

## Unknowns and host-dependent behaviour

- Everything listed in SPEC §11 depends on host memory: period indices 48-49 and above 53,
  envelope numbers past voice 3, mode 2 operations 0, 8 and 10-15 (they read through an address
  the original never sets; its value is whatever an earlier call left in that place, not a
  defined 0), the byte after buffer 7, data read through pointers that were never set, and all
  playback after the song end. A real module that relies on one of these cannot be reproduced
  exactly.
- The per-channel host volume is read from the host at start (64 for every channel in the
  reference). With another host value AUDxVOL scales by value/64.
- The ninth chunk is never read; it might hold names in real modules (unknown).
- The synthesis area is allocated without clearing; it was zero in all reference runs.
- Real modules might use synth length 0 (`n` = 128 while AUDxLEN is written as 0 at row time)
  or bits 6-7 of the synth length byte; neither was tested.
- Tick-handler overruns (long synthesis) delay the next tick in the reference; ports that model
  time per write will see the same shift only if their synthesis costs match LOGS.md.

## Errata found while implementing

- Modules t1-t4 listed in LOGS are missing from the spec's module set, and no DMA logs were shipped.
- Handler and row timings depend on channel index and event type; the port fits them from the logs.
- Unspecified: running-sum width of mode 2 op 7 (port: 16-bit); portamento fall and wrap (port: volume-style).
