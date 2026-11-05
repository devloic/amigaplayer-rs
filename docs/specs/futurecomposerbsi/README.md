# Future Composer BSI: spec folder

- `SPEC.md`: file format, detection, replay behaviour, write order and exact timing.
- `LOGS.md`: the reference logs and what they confirm.
- `logs/`: reference write logs (`*.ref.log`) and `runref.sh` to capture more.
- `modules/`: synthetic modules and `build_syn.py` that builds them (pure data, no code; do not copy
  them into the repo, tests should build modules in code).
- The spec agent's private notes are in a private working folder (not for implementers).

## Player name

UADE player file `FutureComposer-BSI`; it registers in the host as `FutureComposer BSI`, which is the
name `--player` needs. Name prefixes `BFC.`, `BSI.`, `FC-BSI.`; the player itself detects by the
`FUCO` magic only.

## Relation to Future Composer 1.3 / 1.4

A different format with the same name: fixed-size blocks, one-byte sequence entries and one-byte
pattern codes, plain PCM instruments, no macros or envelopes. It shares nothing with
`src/replay/futurecomposer.rs`; write it as a new replayer.

## Data

No `data/` folder: the only table in the player is the 84-entry period table, given as values in
SPEC 1.5 (a format table; the lead decides whether to treat it as such).

## Validation

- A private model of the original's behaviour reproduces all 15 logs exactly (values, order, colour
  clocks), including 3 held-out logs.
- An independent model written from SPEC.md alone (by a separate agent that did not see the private
  notes) also reproduced all 15 logs exactly. Its questions about the wording (clamp boundaries, the
  countdown branch costs, where the 0xFD jump byte is read) were answered in the spec.

## Caveats / unknowns

- No real module of this format was available; all modules are synthetic. Real BSI modules might use
  the format in ways not covered (for instance instrument data running past the end of the file).
- Reads past the end of the module are unspecified (the reference reads whatever follows in memory);
  the spec suggests reading 0, and requiring at least 0x47FC bytes for detection.
- The timing in SPEC section 5 is that of the reference host (fixed cost per player step, E0 = 75 559,
  70 940 cc per tick); a port that matches the beam phase only roughly will place the loop writes
  after the raster wait slightly differently.
- The CIA filter write value assumes the other port bits read as 0, as in the reference host.
- Stopping playback (DMACON off, all volumes 0) is not in any log.
