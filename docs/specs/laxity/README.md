# Laxity spec: notes, caveats, unknowns

Files: `SPEC.md` (format and behaviour), `LOGS.md` (reference logs), `logs/` (the logs),
`mods/` (synthetic module generator and the modules), `data/initial_state.bin` (player data, see
below). A private working folder holds the working notes, traces and the validation model; it is not
part of this spec.

## No real modules

Searched: every file of the module index whose name has the `powt` or `pt` prefix or suffix, every
file with "laxity" in its path, and a header scan of the whole collection for the detection bytes
(0xFF at offsets 0x1F and 0x37). Results:

- No `powt.*` file at all. The `pt` names are ProTracker modules (rejected by the detection: their
  bytes 0x1F/0x37 are not 0xFF).
- The six NostalgicPlayer "Laxity Tracker" `.lax` files (Kefrens, Desert Dream and D.A.N.E) are a
  different format (30-byte ProTracker-like sample headers) and are rejected by this player.
- The header scan found 12 files with both 0xFF bytes; four of them would pass this player's check
  (an AHX module, two Dave Hanlon `dsc.*` files and a The Player 6.0A module), none is a Laxity
  module. A host must try this format after formats with stronger magic words.

So all checks use synthetic modules. The format description (offsets, instrument and cell layout)
is inferred from how the player reads the data, not from real files; the meaning of the 16 unused
bytes per instrument (probably a name) and how the original tracker sized things are unknown.

## Player data (lead decides on embedding)

`data/initial_state.bin` is the 190-byte state block of the player file (SPEC 2.1). The reference
behaviour depends on it from the first tick: speed 6, tick counter 2 (so the first call is an
effect tick of left-over cells and writes AUD2PER = 0x00F0 and AUD3PER = 0x202F), and the left-over
voice and synth records (absolute addresses 0x00057D3E..0x00057D9E). It is a short block of
variables, not music data; a port can embed it or write the same values as constants. The period
and finetune tables are given as values in SPEC.md (format-sized tables).

## Host-specific values in the logs

- The one-shot loop address is the zero word of the player's state block: 0x0095505C in the
  reference host. replay-cmp needs the same address, or the port's own zero word at a constant
  distance.
- The stale addresses 0x00057D3E/5E/7E/9E are written as loop addresses (SPEC 3.5) for voices that
  have not had an instrument, and the synth update writes waveforms there; in the reference they lie
  after the module (module loaded at 0x45E0).
- The DMA-on wait is the host's (10 raster lines, SPEC 6.3), as for the other Eagleplayer ports.
- Command 14 writes CIA-A PRA directly (logged as `bfe001`); its value depends on the host's PRA
  (0 in the reference: both writes are 0x00).

## Quirks the spec reproduces on purpose

Tone portamento has no target (always moves the period down to below 0, then writes 0), command 7
is overwritten, command 12 stores into the vibrato byte, command 13 always continues at row 0,
command 11 with parameter 0 jumps to position 256, volume column 0xFE advances the position on every
tick, the finetune of a note without instrument comes from file offset 6, the instrument count and
the sample start use 16-bit arithmetic, pattern numbers >= 0x80 are negative.

## Not determined / not covered

- Paths that cannot execute (upward tone portamento, a negative volume after the zero volume bias)
  are not described.
- Arpeggio with a tick counter of 32 or more (speed above 32) reads bytes after its 32-entry table;
  not described. Synth widths above 32 overwrite memory far past the buffer; not described.
- The host's DMA-off/on handling (whether a note restarts) is Paula's business; the spec gives the
  write times only.
- The song-end signal is the only length information; the reference host stops at the first one,
  so no log shows the restart position being used (the model and the text agree with the player's
  data flow, not with a log).
