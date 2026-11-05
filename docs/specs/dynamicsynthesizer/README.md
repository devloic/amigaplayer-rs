# Dynamic Synthesizer spec - caveats and unknowns

Deliverables: SPEC.md (format and replay behaviour with cycle timing), LOGS.md, logs/ (13
reference write logs). A private working folder with the reading notes and scripts exists but
is not for the implementer.

## Validation

A model written from the numbers in SPEC.md (value/order model plus the timing table of
section 4.3) reproduces all 13 logs exactly: 5679-7902 writes per log, values, order and colour
clock stamp, including the init groups. It also reproduced the values of three long runs
(about 3700-6900 ticks each, covering the song-end wrap) and the wrap-tick timing to within one
colour clock. The model was developed alongside the spec and the numbers were fitted to the
same logs, so this is a consistency check, not an independent test.

Unobserved paths were checked differentially against the original code executed in an emulator
on randomly generated modules (90 modules - 50 with the title layout, 40 with the in-game layout - up to 300 ticks each, stopping at the song-end tick): values and write
order match in all cases once the 7-bit volume mask was found by this test (including note-off of a never-started track, high boost, clamped
periods, mute masks). Timing on those random modules does NOT always match the model: tick
offsets differ by up to about 500 cycles when a note start uses an unseen combination of
conditions, so the timing table in SPEC.md 4.3 is only established for the combinations that
occur in the two real modules.

## Unknowns

- Table locations for a third module. Only two modules exist in the corpus (both from one
  game); the offsets in SPEC.md section 2 are per module. Only the period table has a
  recognisable signature (its first words).
- Timing of rarely-seen combinations (see above), of ticks before the first note, and the exact
  half-cycle of the wrap extra cost (221 or 222 cycles).
- SongEnd is only observed once per run; whether and when it is raised again on later loops is
  not known. The host's behaviour after the song end (stopping ~2 ticks later) is host policy.
- Speed 0 cannot be produced by the two modules; behaviour not specified. (Negative volumes are covered: the volume register value is masked to 7 bits.)
- Sound-effect entry (slot 3) of the driver is not used by the player wrapper and not covered.

## data/

No player-resident data is needed, so no data/ folder was produced.
