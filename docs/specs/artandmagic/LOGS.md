# Art & Magic: reference logs

Each file in `logs/` is a write log of the reference 68k player run through the host with
DELI_WRITE_LOG (dummy audio only). Line format: `ADDRESS VALUE COLOUR-CLOCK`, addresses and
values hexadecimal, colour clock decimal. The first line is a comment giving the load address of
the module (0x0045E0) and the file size in bytes. The first data line is the first AUD0DAT write
(its colour clock is I in SPEC section 12). Subsong 0 only.

| log | module | driver | notes |
|---|---|---|---|
| agony_l1_6 | aam.level_1_and_6 | A | real; loops at tick 3528 |
| agony_l2 | aam.level_2 | A | real; loops at tick 3456 |
| agony_l3_5 | aam.level_3_and_5 | B | real; loops at tick 2880 |
| agony_l4 | aam.level_4 | A | real; loops at tick 6048 |
| syn_a2 | synthetic on the level_2 driver | A | see below |
| syn_b3 | synthetic on the level_3_and_5 driver | B | |
| syn_c21, syn_c23 | synthetic on the level_2 driver | A | |
| syn_d21, syn_d22 | synthetic on the level_3_and_5 driver | B | |

The real logs run past the first loop by 0 to 3 ticks (SPEC 10.3: the host checks for song end
once per 0.1 s chunk).

## Synthetic modules

The archive holds only the four real modules. To cover features they never use, synthetic modules
were built by taking a copy of a real module (driver code, tables and samples unchanged) and
replacing the order lists, streams, arpeggio lists and instrument records with generated ones:
random but valid event chains using every prefix, rests, ties, volume overrides, slides, vibrato
settings, arpeggios (with 0xFF jumps and 0xFE ends), decay and attack instruments, negative
volumes, negative arpeggio offsets, the filter-pulse flag, a voice ending with 0xFE, and
different repeat counts. About 34 synthetic modules (versions A and B) were generated and checked;
six of the logs are shipped here. The synthetic modules themselves are not shipped.

## What was checked

A model written from SPEC.md reproduces, for the four real logs and about 34 synthetic ones, every
line: address, value, order and colour clock (both the offset within the tick and the absolute
value given the start phase of SPEC 12), with zero mismatches. The sample memory of the model equals
every audio DMA word the reference fetched in level_2 and level_3_and_5 (wave-table overlay
included).

## Gaps of the real logs

The four real modules never use: rests, slide events, the voice end event, vibrato, arpeggios,
decay envelopes, the filter pulse, and (version B) the colour write. Those paths are validated by the
synthetic logs only. The reference host reads 0 from the CIA port, which fixes the CIA values.
