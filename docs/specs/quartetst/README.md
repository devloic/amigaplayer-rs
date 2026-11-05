# Quartet ST - unknowns and caveats

Files:
- SPEC.md: format and behaviour.
- LOGS.md: what the logs contain.
- logs/: reference write logs, plus the synthetic scores and sets they were made from.

There is no data/ folder: the player holds no tables that a port needs (SPEC 6 lists its few constants).
A private working folder holds the analyst's notes; the implementer does not need it.

## Validation

A model written from the rules and numbers of SPEC.md was checked against 42 reference logs (12 hand-made
modules and 30 random ones, about 975 000 writes). There were no differences in address, value, order or
colour-clock stamp, first-tick clock included. The INTREQR reads were answered by the project's own Paula
model; with it, every INTREQ write and repeat write lands where the reference has it. Caveat: the analyst
who wrote the spec also wrote the model; nobody else has re-implemented the prose.

## Real modules

None. The module index has no `qts.` files, and no `smp.set` belonging to this format was found. Everything
was checked on synthetic files built from the format description. Real Quartet ST music (from Atari ST
conversions such as "Projectyle") may use the format in ways these files do not. Typical frequency values
are about 8 x the sample rate in Hz. The meaning of header word 0 and byte 6 is unknown, and neither is
used by the replay.

## Behaviour of the reference that looks like a bug of the adaptation

- Slide end check (SPEC 4.2 step 3): the reference compares the target against a value derived from the
  period, not against the new frequency. A downward slide therefore jumps to its target after one step, and
  an upward slide usually never ends by itself. This is what the reference does, so a register-exact port
  must copy it. The original game driver probably compared the frequency.
- The `F` command's attempt to reset the loop stack has no effect (SPEC 4.2 `F`).

## Unknowns / not supported

- Frequency values whose (f >> 3) is non-zero with a zero low word: the reference crashes (division by
  zero).
- ST-variant sample records with a length of 0 or 1 make the reference's halving run without end.
- An `L` with an empty loop stack, more than 100 nested open loops, and sample indices above 0x4C all read or
  write player memory outside the voice's own area. Their effect was not determined.
- A `V` index that is not a multiple of 4 (only the first one is checked at detection) reads a misaligned
  slot address. Not tested.
- The `SMP.set` fallback was not run, so its start-up cost is not specified.
- Start-up timing (SPEC 5.3) depends on the host's directory string and file name, because the reference
  builds the companion's path character by character. With different strings, only the beam phase
  changes: the time of Paula's line-aligned DMA start, and so possibly the tick in which an interrupt bit is
  first seen.
- Cycle numbers are the host's static per-instruction costs, not real 68000 timing.
- Host gains other than the default were not exercised.
