# Thomas Hermann - caveats and unknowns

Spec: `SPEC.md`; logs: `logs/` described in `LOGS.md`. Private reading
notes and the validation model exist in a folder that is not part of the
deliverable (single mention here by request of the brief). No player-resident
data is needed, so there is no `data/` folder.

## Validation (private model written from SPEC.md, nothing pinned to the logs)

The model schedules the Play and slide/envelope handlers from the due times in
SPEC.md section 4, uses the offsets of section 9 and the Paula and host rules, and
produces the writes. It is compared with each reference log on address,
value and order (location registers as "same base per instrument"). One
number per log is taken from the log: the phase D0 of the Play ticks
(155 or 190 cc after the timer start, see SPEC.md section 4) and the sample
load address (to resolve out-of-table records). No per-write time is taken
from the logs.

| log | events | order/value mismatches |
|---|---|---|
| Beam sub0 | 38138 | 0 |
| Beam_Unused sub0 / sub1 / sub2 | 30041 / 1849 / 176 | 0 / 0 / 0 |
| Nightdawn sub0 | 29498 | 0 |
| Blue_Angel_69 sub1 / sub2 / sub4 | 11693 / 10933 / 447 | 0 / 0 / 0 |
| Blue_Angel_69 sub3 | 2261 | 9 (first at event 1093) |
| Blue_Angel_69 sub0 | 17141 | 8848 (first at event 3681; one flipped tie, the rest follows) |
| SynEnv | 6260 | 20 (first at event 24) |
| SynMode0 | 6402 | 21 (first at event 2454) |
| SynMix | 6937 | 21 (first at event 10) |

With the Play write times additionally pinned to the logged ones (so that
only the value logic and the interrupt ordering rules are tested) the
synthetic logs give: SynEnv 0 of 6260, SynMode0 0 of 6402, SynMix 12 of
6937 (the 12 are one interrupt-versus-slide tie at event 10 and its
consequences).

Reasons for the remaining mismatches. Every one is an ordering flip between
an audio-interrupt write and a Play-tick write (or, in SynMix, a slide
write) where the two are within a few hundred cc: the order is decided by
whether the interrupt was raised before or after the Play handler started,
and the raise time depends on a DMA start request being taken at a line
boundary (227 cc), so an error of a few cc in a Play write time (a start-tick
offset that depends on the audio DMA state, SPEC.md section 9) flips a whole
line. After one flipped tie the log carries on with a different chain of
lost interrupts, which is why Blue_Angel_69 sub0 reports many mismatches
after the first. The cause of the offset variation (457 / 517 / 533 / 556 /
676 in Blue Angel 69; constant in Beam and Nightdawn) was found only as far
as the table in SPEC.md section 9; it is not predictable there from the enabled
channel set alone.

## Not determined

* The 35 cc difference of the Play phase D0 between runs, and the cause
  of the start-tick offset spread in Blue Angel 69 (SPEC.md section 9).
* The host low-memory record read for an out-of-table instrument number
  (reference host only, SPEC.md section 5.3); only one address was seen with
  non-zero content.
* The end time of the slide/envelope handler (100 cc is a guess).
* The location address of instrument 0 (the placeholder record) when a
  row names instrument 0 or a note uses a reject path.
* "kramer" (10112 bytes, 3 subsongs) is not in the corpus; its subsong
  table is taken from the player and is unchecked.
* The NTSC variant of the duration formula is not described.
* The per-sample file fallback loading is not tested.
