# Sean Connolly: reference logs

## How the logs were made

Each log is the write log of the reference 68k player (the Eagleplayer
"Sean Connolly" run through the host with DELI_WRITE_LOG), one line per
hardware register write: `ADDRESS VALUE COLOUR-CLOCK`. The first line is a
comment with the module load address and the module size. Long writes appear
as two lines (high word first) at one clock. Audio was rendered with the
dummy audio driver and a WAV output only; nothing was played on a speaker.
Only chip-register addresses (0xdff...) matter for replay comparison.

Every log is 60 s of emulated time (`--seconds 60`) from a run with
`--subsong N`; subsongs 4, 5 and 6 of the real module are short and the
reference re-initialises them after song end, so their logs contain the
re-init groups too (a group of ten writes: DMACON 0x000f, four times PER and
VOL zero, DMACON 0x000f).

### Host caveat (important for reproduction)

The wrapper waits on the CIA-B TOD low byte. The host used for the logs
returned floor(colour clock / 227) for every read of that register (the
unmodified host did not emulate the TOD register and the reference run
hung). A reproduction host must provide the same scanline-based TOD
behaviour, otherwise the timing of the last DMACON write of each tick (section
9.3 of SPEC.md) differs.

## Files in logs/

| file | module | subsong | play ticks |
| --- | --- | --- | --- |
| surfninjas-sub0.log | real module Surf Ninjas (file prefix scn.) | 0 | 2054 |
| surfninjas-sub1.log | same | 1 | 3000 |
| surfninjas-sub2.log | same | 2 | 3001 |
| surfninjas-sub3.log | same | 3 | 3003 |
| surfninjas-sub4.log | same | 4 | 173 (then song-end re-init) |
| surfninjas-sub5.log | same | 5 | 110 (then re-init) |
| surfninjas-sub6.log | same | 6 | 351 |
| synth1.log .. synth4.log | synthetic variations of the real module (below) | 6 | 324 / 599 / 1444 / 1444 |

The synthetic modules are NOT included: each is the Surf Ninjas module with
subsong 6's four tracks and a few patterns, instruments or tables replaced, so
it carries the whole game module. Their edits, so that the logs can be
interpreted (the replaced data is described, the unchanged remainder is the
real module):

* synth1: pattern tokens. Tracks with tie on/off, rests (0x60 range), slides
  up and down with several speeds, pitch-sequence select with absolute notes,
  with a jump and with a stop, a wave-offset token, hold tokens, and a
  fade token at the start of one track. Instruments use mode 0 with a
  pitch sequence or vibrato.
* synth2: mode 2 (chunk streaming) with an 8SVX sample (chunk 0x200 bytes)
  and a raw sample (chunk 0x100 bytes).
* synth3: mode 3 wave sequence with 6 entries, one noise wave (0xfff), a
  jump word and a wave-offset token.
* synth4: mode 0 whose wave position sweeps through 0xfff (noise) and back, a
  second voice with fade token and a vibrato instrument with envelope delays.

## Validation result

A private reimplementation (written from the same reading notes that SPEC.md
was then written from, not from SPEC.md text alone) was compared with these
logs tick by tick.

* Register values and write order (LC, LEN, PER, VOL, DMACON): 0 differing
  ticks in all of: real module subsongs 0..6 (2054 + 3000 + 3001 + 3003 + 173
  + 110 + 351 ticks) and synthetic modules 1..4 (324 + 599 + 1444 + 1444
  ticks). LC values accepted as module-relative or, for the first four ticks,
  as the literal stale image value.
* Fixed write offsets (section 9.1): identical in all 11664 measured ticks.
* Timing of the final DMACON write: with the event-cost fit of section 9.2
  and the wait simulation of 9.3, 11637 of 11659 real ticks match exactly (22
  ticks off). Synthetic modules match worse (parity of half-cycles,
  inactive-voice cost, mode 2/3 details) and are only approximated.
* The noise generator and the seed rule were checked against the host's DMA
  fetch log for synth4 (first words matched after chaining 7 inits).

Unverified: type 1 and type 3 drivers, other modules with other table
offsets, noise-voice interaction in a mixed active/inactive situation.
